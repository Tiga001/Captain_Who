//! Request-adopted state across real Host Runs using only a local controlled Provider.
use super::web_search_policy::save_search_policy;
use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};

const CONVERSATION: &str = "world-state-cross-run";
const CREDENTIAL_CANARY: &str = "FAKE_WORLD_STATE_SEARCH_CREDENTIAL";

async fn read_provider_request(stream: &mut TcpStream) -> Value {
    let mut request = Vec::new();
    let mut buffer = [0_u8; 4096];
    let mut body_start = None;
    let mut request_end = None;
    loop {
        let count = stream.read(&mut buffer).await.unwrap();
        assert!(count > 0, "Provider request closed before its body arrived");
        request.extend_from_slice(&buffer[..count]);
        if body_start.is_none() {
            if let Some(index) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                let length = String::from_utf8_lossy(&request[..index])
                    .lines()
                    .find_map(|line| {
                        line.split_once(':').and_then(|(name, value)| {
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().unwrap())
                        })
                    })
                    .unwrap();
                body_start = Some(index + 4);
                request_end = Some(index + 4 + length);
            }
        }
        if request_end.is_some_and(|end| request.len() >= end) {
            return serde_json::from_slice(&request[body_start.unwrap()..request_end.unwrap()])
                .unwrap();
        }
    }
}

async fn wait_for_done(
    receiver: &mut UnboundedReceiver<Value>,
    run_id: &str,
    status: &str,
) -> Value {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let notification = receiver
                .recv()
                .await
                .expect("Host notification stream closed");
            let event = &notification["params"];
            assert_ne!(
                event["type"], "error",
                "unexpected Host failure: {notification}"
            );
            if event["type"] == "done" {
                assert_eq!(event["runId"], run_id, "continuation must retain its Run");
                assert_eq!(
                    event["status"], status,
                    "unexpected worker outcome: {notification}"
                );
                return event.clone();
            }
        }
    })
    .await
    .expect("Host did not reach the expected durable Run boundary")
}
async fn wait_for_worker_release(agent: &AgentService, run_id: &str) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if !agent.cancellations.lock().unwrap().contains_key(run_id) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("a synchronous wait must release its worker segment before restart");
}

fn turn_input(index: usize) -> AgentConversationTurnInput {
    AgentConversationTurnInput {
        conversation_id: Some(CONVERSATION.into()),
        project_id: None,
        model_id: "model-1".into(),
        context_window_indicator_enabled: true,
        content: format!("WORLD_USER_{index}"),
        attachments: Vec::new(),
        skills: Vec::new(),
        title: None,
        user_message_id: Some(format!("world-user-{index}")),
        assistant_message_id: Some(format!("world-assistant-{index}")),
        max_tokens: None,
        temperature: None,
        prompt_preferences: None,
        permissions: AgentPermissions::default(),
    }
}

fn receipts(database: &Path) -> Vec<String> {
    let connection =
        rusqlite::Connection::open_with_flags(database, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    let mut statement = connection.prepare(
        "SELECT payload_json FROM conversation_world_state_request_commits ORDER BY created_at, run_id, request_index",
    ).unwrap();
    statement
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}

fn message_text(message: &Value) -> String {
    message["content"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| message["content"].to_string())
}

fn conversation_records(request: &Value) -> Vec<Value> {
    request["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|message| {
            let content = message["content"].as_str()?;
            // The Provider adapter wraps timeline state in <backend_observed_state>.
            let (_, content) = content.split_once("<backend_world_state_record>")?;
            let (content, _) = content.split_once("</backend_world_state_record>")?;
            content.lines().find_map(|line| {
                let value: Value = serde_json::from_str(line).ok()?;
                (value["lifetime"] == "conversation").then_some(value)
            })
        })
        .collect()
}

fn workspace_changes(records: &[Value]) -> Vec<Value> {
    records
        .iter()
        .filter_map(|record| record["changes"].as_array())
        .flatten()
        .filter(|change| change["sectionId"] == "workspace.binding")
        .cloned()
        .collect()
}

#[tokio::test]
async fn workspace_source_patches_adopt_only_new_root_runs_and_preview_never_writes() {
    use super::managed_command_loop::{write_text_stream, write_tool_call_stream};
    use mycopilot_core::storage::models::{ProjectFolderRecord, ProjectFolderRole};
    use mycopilot_core::{AgentCommandPermission, AgentCommandSafetyPolicy};

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (captured, mut requests) = unbounded_channel();
    let provider = tokio::spawn(async move {
        for index in 0..4 {
            let (mut stream, _) = tokio::time::timeout(Duration::from_secs(20), listener.accept())
                .await
                .unwrap()
                .unwrap();
            captured
                .send(read_provider_request(&mut stream).await)
                .unwrap();
            if index == 0 {
                write_tool_call_stream(
                    &mut stream,
                    "workspace-approval-command",
                    "run_command",
                    json!({"command":"printf workspace-frozen", "reason":"Verify the frozen workspace after approval."}),
                    "Waiting for approval.",
                )
                .await;
            } else {
                write_text_stream(&mut stream, &format!("WORKSPACE_ANSWER_{index}")).await;
            }
        }
    });
    let fixture = tempdir().unwrap();
    let database = fixture.path().join("storage.sqlite");
    let storage = Arc::new(
        StorageService::open_with_model_credentials(
            &database,
            Arc::new(mycopilot_core::image_generation::InMemoryCredentialStore::default()),
        )
        .unwrap(),
    );
    let mut settings = test_model_settings();
    settings.api_url = format!("http://{address}/v1/chat/completions");
    storage.save_model_settings(settings).unwrap();
    let app = fixture.path().join("app");
    let docs = fixture.path().join("docs");
    let replacement = fixture.path().join("replacement");
    for root in [&app, &docs, &replacement] {
        fs::create_dir(root).unwrap();
    }
    let mut project = ProjectRecord::with_primary_folder(
        "workspace-private-project-id",
        "Workspace delta",
        app.to_string_lossy(),
        1,
    );
    project.folders[0].id = "workspace-private-original-id".into();
    project.folders[0].alias = "app".into();
    storage.save_project(project.clone()).unwrap();
    let service = AgentService::new_authorized_for_test(storage.clone());
    let (notifications, mut events) = unbounded_channel();
    let permissions = AgentPermissions {
        command: AgentCommandPermission::RequireApproval,
        command_safety: AgentCommandSafetyPolicy::FullAccess,
        ..AgentPermissions::default()
    };
    let project_id = project.id.clone();
    let input = |index| {
        let mut input = turn_input(index);
        input.project_id = Some(project_id.clone());
        input.permissions = permissions;
        input
    };
    let preview = || {
        service
            .get_context_window_snapshot(AgentContextWindowSnapshotInput {
                conversation_id: Some(CONVERSATION.into()),
                project_id: Some(project_id.clone()),
                model_id: "model-1".into(),
                max_tokens: None,
                prompt_preferences: None,
                permissions,
                skills: vec![],
            })
            .unwrap()
            .snapshot
            .unwrap()
    };
    let first = service
        .start_conversation_turn(input(0), notifications.clone())
        .unwrap();
    wait_for_done(&mut events, &first.run_id, "waiting_for_approval").await;
    wait_for_worker_release(&service, &first.run_id).await;
    let initial_request = requests.recv().await.unwrap();
    let initial_records = conversation_records(&initial_request);
    assert_eq!(initial_records.len(), 1);
    assert_eq!(initial_records[0]["recordType"], "full");
    assert!(workspace_changes(&initial_records).is_empty());
    let frozen = storage.load_agent_workspace_for_run(&first.run_id).unwrap();
    assert_eq!(
        frozen
            .as_ref()
            .and_then(Option::as_ref)
            .and_then(|workspace| workspace.root_path.as_deref()),
        app.to_str()
    );
    let active_preview = preview();

    // Both edits happen while the root is blocked. Only their net result belongs to the next Run.
    project.folders.push(ProjectFolderRecord {
        id: "workspace-private-docs-id".into(),
        alias: "docs".into(),
        path: docs.to_string_lossy().into_owned(),
        role: ProjectFolderRole::Auxiliary,
        sort_order: 1,
        created_at: 2,
    });
    storage.save_project(project.clone()).unwrap();
    project.folders[0].role = ProjectFolderRole::Auxiliary;
    project.folders[1].role = ProjectFolderRole::Primary;
    storage.save_project(project.clone()).unwrap();
    assert_eq!(
        preview(),
        active_preview,
        "an approval preview keeps the frozen sources"
    );
    let pending = service.list_pending_actions();
    assert_eq!(pending.len(), 1);
    service
        .approve_action(&first.run_id, &pending[0].action_id, notifications.clone())
        .unwrap();
    wait_for_done(&mut events, &first.run_id, "completed").await;
    wait_for_worker_release(&service, &first.run_id).await;
    let resumed_request = requests.recv().await.unwrap();
    assert_eq!(
        conversation_records(&resumed_request),
        initial_records,
        "approval resume must not adopt the edited project or publish its patch"
    );
    assert_eq!(
        storage.load_agent_workspace_for_run(&first.run_id).unwrap(),
        frozen,
        "approval must retain the original authoritative workspace"
    );

    for index in 1..=2 {
        let before_replacement = (index == 2).then(&preview);
        if index == 2 {
            // The public alias and logical path stay identical, but old file observations are stale.
            project.folders[0].id = "workspace-private-replacement-id".into();
            project.folders[0].path = replacement.to_string_lossy().into_owned();
            storage.save_project(project.clone()).unwrap();
        }
        let records_before = load_conversation_world_state(&storage, CONVERSATION).unwrap();
        let receipts_before = receipts(&database);
        let current_preview = preview();
        assert_eq!(current_preview, preview(), "an idle preview must be stable");
        if let Some(before) = before_replacement {
            assert!(
                current_preview.input_tokens > before.input_tokens,
                "the hot preview cache must account for a same-alias source replacement patch"
            );
        }
        assert_eq!(
            load_conversation_world_state(&storage, CONVERSATION).unwrap(),
            records_before,
            "preview must not publish workspace edits as observed"
        );
        assert_eq!(receipts(&database), receipts_before);
        let turn = service
            .start_conversation_turn(input(index), notifications.clone())
            .unwrap();
        wait_for_done(&mut events, &turn.run_id, "completed").await;
        wait_for_worker_release(&service, &turn.run_id).await;
        let request = requests.recv().await.unwrap();
        let records = conversation_records(&request);
        assert_eq!(records[0], initial_records[0]);
        let changes = workspace_changes(&records);
        assert_eq!(changes.len(), index, "each new source state is sent once");
        let patch = changes.last().unwrap();
        assert_eq!(patch["op"], "patch");
        assert!(
            patch.get("value").is_none(),
            "do not resend the complete binding"
        );
        let folder_changes = patch["changes"].as_array().unwrap();
        if index == 1 {
            assert_eq!(
                folder_changes.len(),
                2,
                "publish the final net source change"
            );
            assert!(folder_changes.iter().any(|change| {
                change["kind"] == "added"
                    && change["folder"]["alias"] == "docs"
                    && change["folder"]["role"] == "primary"
            }));
            assert!(folder_changes.iter().any(|change| {
                change["kind"] == "updated"
                    && change["folder"]["alias"] == "app"
                    && change["folder"]["role"] == "auxiliary"
            }));
        } else {
            assert_eq!(folder_changes.len(), 1);
            assert_eq!(folder_changes[0]["kind"], "updated");
            assert_eq!(folder_changes[0]["folder"]["alias"], "app");
            assert_eq!(folder_changes[0]["reason"], "source_replaced");
        }
        let serialized = serde_json::to_string(&records).unwrap();
        for private_value in [
            fixture.path().to_str().unwrap(),
            "workspace-private-project-id",
            "workspace-private-original-id",
            "workspace-private-docs-id",
            "workspace-private-replacement-id",
        ] {
            assert!(
                !serialized.contains(private_value),
                "source patch leaked {private_value}"
            );
        }
        let stored = load_conversation_world_state(&storage, CONVERSATION).unwrap();
        let boundary = stored.last().unwrap().request_boundary.as_ref().unwrap();
        assert_eq!(boundary.run_id, turn.run_id);
        assert_eq!(
            boundary.request_index, 1,
            "publish before the first sampling request"
        );
    }
    provider.await.unwrap();
}

#[tokio::test]
async fn cross_run_web_policy_commits_at_request_boundaries_and_preview_never_writes() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (captured, mut requests) = unbounded_channel();
    let provider = tokio::spawn(async move {
        for index in 0..3 {
            let (mut stream, _) = tokio::time::timeout(Duration::from_secs(20), listener.accept())
                .await
                .unwrap()
                .unwrap();
            captured
                .send(read_provider_request(&mut stream).await)
                .unwrap();
            let content = json!({"choices":[{"delta":{"role":"assistant","content":format!("WORLD_ANSWER_{index}")},"finish_reason":null}]});
            let finish = json!({"choices":[{"delta":{},"finish_reason":"stop"}]});
            stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\ndata: {content}\n\ndata: {finish}\n\ndata: [DONE]\n\n").as_bytes()).await.unwrap();
        }
    });
    let fixture = tempdir().unwrap();
    let database = fixture.path().join("storage.sqlite");
    let storage = Arc::new(
        StorageService::open_with_model_credentials(
            &database,
            Arc::new(mycopilot_core::image_generation::InMemoryCredentialStore::default()),
        )
        .unwrap(),
    );
    let mut settings = test_model_settings();
    settings.api_url = format!("http://{address}/v1/chat/completions");
    storage.save_model_settings(settings).unwrap();
    let service = AgentService::new_authorized_for_test(storage.clone());
    let (notifications, mut events) = unbounded_channel();
    let mut turns = Vec::new();
    let mut wires = Vec::new();
    let mut initial_full = None;
    for (index, enabled) in [false, true, false].into_iter().enumerate() {
        if index > 0 {
            save_search_policy(
                &storage,
                if enabled { "tavily" } else { "disabled" },
                if enabled {
                    CredentialMutation::Replace {
                        value: CREDENTIAL_CANARY.into(),
                    }
                } else {
                    CredentialMutation::Keep
                },
            );
            let records_before = load_conversation_world_state(&storage, CONVERSATION).unwrap();
            let receipts_before = receipts(&database);
            for _ in 0..2 {
                let preview = service
                    .get_context_window_snapshot(AgentContextWindowSnapshotInput {
                        conversation_id: Some(CONVERSATION.into()),
                        project_id: None,
                        model_id: "model-1".into(),
                        max_tokens: None,
                        prompt_preferences: None,
                        permissions: AgentPermissions::default(),
                        skills: Vec::new(),
                    })
                    .unwrap()
                    .snapshot
                    .unwrap();
                assert!(preview.input_tokens > 0);
            }
            assert_eq!(
                load_conversation_world_state(&storage, CONVERSATION).unwrap(),
                records_before,
                "preview must not append or mark state as observed"
            );
            assert_eq!(
                receipts(&database),
                receipts_before,
                "preview must not create request receipts"
            );
        }
        let turn = service
            .start_conversation_turn(turn_input(index), notifications.clone())
            .unwrap();
        wait_for_done(&mut events, &turn.run_id, "completed").await;
        wait_for_worker_release(&service, &turn.run_id).await;
        let request = requests.recv().await.unwrap();
        let names = request["tools"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|tool| tool["function"]["name"].as_str())
            .collect::<Vec<_>>();
        for name in ["web_search", "web_fetch"] {
            assert_eq!(names.contains(&name), enabled);
        }
        let messages = request["messages"].as_array().unwrap();
        let text = messages.iter().map(message_text).collect::<Vec<_>>();
        assert_eq!(
            text.iter().any(|content| content.contains("## 联网搜索")),
            enabled,
            "request guidance and schema must use the same current observation"
        );
        assert!(text
            .iter()
            .all(|content| !content.contains("tools.effective")
                && !content.contains(CREDENTIAL_CANARY)));
        let stored = load_conversation_world_state(&storage, CONVERSATION).unwrap();
        assert_eq!(
            stored.len(),
            index + 1,
            "one initial full, then only the actual policy changes"
        );
        assert!(stored.iter().all(|record| record.model_observed));
        assert_eq!(receipts(&database).len(), index + 1);
        assert!(!serde_json::to_string(&stored)
            .unwrap()
            .contains(CREDENTIAL_CANARY));
        assert!(receipts(&database)
            .iter()
            .all(|receipt| !receipt.contains(CREDENTIAL_CANARY)));
        if index == 0 {
            assert!(matches!(
                stored[0].record,
                mycopilot_core::WorldStateRecord::Full(_)
            ));
            assert!(stored[0].request_boundary.is_none());
            initial_full = Some(stored[0].clone());
        } else {
            assert_eq!(
                &stored[0],
                initial_full.as_ref().unwrap(),
                "later Runs must preserve the original full"
            );
            assert!(stored[index].effective_before_message_id.is_none());
            let trace = storage
                .get_conversation_turn_trace(&turn.assistant_message_id)
                .unwrap()
                .unwrap();
            let bootstrap_sequence = trace
                .items
                .iter()
                .find_map(|item| match item {
                    mycopilot_core::ConversationTurnTraceItem::ContextMaterial {
                        sequence,
                        material_kind:
                            mycopilot_core::ConversationContextMaterialKind::RunWorldState,
                        ..
                    } => Some(*sequence),
                    _ => None,
                })
                .expect("initial Run material is journaled before observing this request");
            assert_eq!(
                stored[index].request_boundary.as_ref(),
                Some(&mycopilot_core::WorldStateRequestBoundary {
                    run_id: turn.run_id.clone(),
                    assistant_message_id: turn.assistant_message_id.clone(),
                    request_index: 1,
                    after_trace_sequence: Some(bootstrap_sequence),
                })
            );
        }
        turns.push(turn);
        wires.push(text);
    }
    provider.await.unwrap();
    assert!(turns
        .windows(2)
        .all(|pair| pair[0].run_id != pair[1].run_id));
    let final_wire = &wires[2];
    let full = final_wire
        .iter()
        .position(|text| {
            text.contains("\"lifetime\":\"conversation\"")
                && text.contains("\"recordType\":\"full\"")
        })
        .unwrap();
    let diffs = final_wire
        .iter()
        .enumerate()
        .filter(|(_, text)| {
            text.contains("\"lifetime\":\"conversation\"")
                && text.contains("\"recordType\":\"diff\"")
        })
        .collect::<Vec<_>>();
    assert_eq!(
        diffs.len(),
        2,
        "historical policy changes must remain chronological"
    );
    let position = |marker: &str| {
        final_wire
            .iter()
            // The collaboration guide also quotes the initial task title. Match the actual
            // user/assistant body, whose preceding timing metadata is a separate block.
            .position(|text| text.lines().last() == Some(marker))
            .unwrap()
    };
    let collaboration = final_wire
        .iter()
        .position(|text| text.starts_with("## Agent 协作"))
        .unwrap();
    let capability_guide = final_wire
        .iter()
        .position(|text| text.starts_with("## 人机交互"))
        .unwrap();
    assert!(collaboration < capability_guide && capability_guide < full);
    assert!(full < position("WORLD_USER_0"));
    assert!(position("WORLD_USER_0") < position("WORLD_ANSWER_0"));
    assert!(position("WORLD_ANSWER_0") < position("WORLD_USER_1"));
    assert!(position("WORLD_USER_1") < diffs[0].0);
    assert!(diffs[0].0 < position("WORLD_ANSWER_1"));
    assert!(position("WORLD_ANSWER_1") < position("WORLD_USER_2"));
    assert!(position("WORLD_USER_2") < diffs[1].0);
    let run_full = final_wire
        .iter()
        .rposition(|text| {
            text.contains("\"lifetime\":\"run\"") && text.contains("\"recordType\":\"full\"")
        })
        .unwrap();
    assert!(position("WORLD_USER_2") < run_full && run_full < diffs[1].0);
    assert!(diffs[0].1.contains("\"available\":true"));
    assert!(diffs[1].1.contains("\"reason\":\"disabled_by_user\""));
    let first_full = wires[0]
        .iter()
        .find(|text| text.contains("\"lifetime\":\"conversation\""))
        .unwrap();
    assert_eq!(
        &final_wire[full], first_full,
        "later observations cannot rewrite the historical full"
    );
}
