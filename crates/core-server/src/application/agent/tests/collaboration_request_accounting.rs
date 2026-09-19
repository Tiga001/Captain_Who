//! Account for the real Host/driver request, not two reconstructions of the same preview.
use super::provider_profiles::{
    collect_until_done, read_provider_request, save_provider_profile_fixture, write_provider_stream,
};
use super::*;
use mycopilot_core::{AgentCollaborationSettingsUpdate, ModelRequestEstimate, ModelRequestPurpose};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot};

const CONVERSATION: &str = "collaboration-accounting";
const MAX_TOKENS: u32 = 1024;
const COLLABORATION_TOOLS: [&str; 6] = [
    "spawn_agent",
    "send_message",
    "followup_task",
    "list_agents",
    "wait_agent",
    "interrupt_agent",
];

struct PausedRequest {
    body: Value,
    release: oneshot::Sender<()>,
}

fn accept_provider_connection(listener: &std::net::TcpListener) -> std::net::TcpStream {
    listener.set_nonblocking(true).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        match listener.accept() {
            Ok((stream, _)) => return stream,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if std::time::Instant::now() >= deadline {
                    panic!("Host must reach the local Provider: Elapsed(())");
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => panic!("Host must reach the local Provider: {error}"),
        }
    }
}

async fn provider(
    responses: Vec<(Value, &'static str)>,
) -> (
    String,
    mpsc::UnboundedReceiver<PausedRequest>,
    tokio::task::JoinHandle<()>,
) {
    // These tests pin Tokio to two workers. A listener on that pool can miss TCP accepts while
    // both workers sit in blocking SQLite/preview. Moving a std listener onto another Tokio
    // reactor via from_std is also not reliable here: the dedicated thread's kqueue may never
    // watch the fd, which surfaces as "Provider stopped early" after the 20s accept timeout.
    // Bind and accept on one OS thread with blocking poll; only wrap the accepted stream.
    let (captured, requests) = mpsc::unbounded_channel();
    let (listening_tx, listening_rx) = std::sync::mpsc::channel();
    let (done_tx, done_rx) = oneshot::channel();
    std::thread::Builder::new()
        .name("collaboration-accounting-provider".to_string())
        .spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
                let address = listener.local_addr().unwrap();
                listening_tx
                    .send(address)
                    .expect("test must receive listen address");
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("fake Provider runtime");
                for (delta, finish) in responses {
                    let stream = accept_provider_connection(&listener);
                    stream.set_nonblocking(true).unwrap();
                    runtime.block_on(async {
                        let mut stream = TcpStream::from_std(stream)
                            .expect("accepted Provider stream must join the fake runtime");
                        let body = read_provider_request(&mut stream).await;
                        let (release, released) = oneshot::channel();
                        captured.send(PausedRequest { body, release }).unwrap();
                        released
                            .await
                            .expect("test must release the captured request");
                        write_provider_stream(&mut stream, delta, finish).await;
                    });
                }
            }));
            let _ = done_tx.send(result);
        })
        .expect("fake Provider thread");
    let address = listening_rx
        .recv()
        .expect("fake Provider must start listening");
    let task = tokio::spawn(async move {
        match done_rx.await {
            Ok(Ok(())) => {}
            Ok(Err(payload)) => std::panic::resume_unwind(payload),
            Err(_) => panic!("fake Provider thread vanished"),
        }
    });
    (
        format!("http://{address}/v1/chat/completions"),
        requests,
        task,
    )
}

async fn next_request(requests: &mut mpsc::UnboundedReceiver<PausedRequest>) -> PausedRequest {
    tokio::time::timeout(Duration::from_secs(10), requests.recv())
        .await
        .expect("real driver must send the next request")
        .expect("Provider stopped early")
}

struct Fixture {
    directory: tempfile::TempDir,
    storage: Arc<StorageService>,
    service: AgentService,
    permissions: AgentPermissions,
}

impl Fixture {
    fn new(url: &str, enabled: bool) -> Self {
        let directory = tempdir().unwrap();
        let storage =
            Arc::new(StorageService::open(&directory.path().join("storage.sqlite")).unwrap());
        save_provider_profile_fixture(&storage, url, None);
        storage
            .save_conversation(ChatConversationRecord {
                id: CONVERSATION.into(),
                project_id: None,
                model_id: Some("model-1".into()),
                title: "Collaboration request accounting".into(),
                messages: vec![],
                created_at: 1,
                updated_at: 1,
                pinned_at: None,
                archived_at: None,
                unread_at: None,
            })
            .unwrap();
        set_enabled(&storage, enabled);
        let service = AgentService::new_authorized_for_test(storage.clone());
        Self {
            directory,
            storage,
            service,
            permissions: AgentPermissions::default(),
        }
    }

    fn start(&self, index: usize) -> (AgentConversationTurnOutput, mpsc::UnboundedReceiver<Value>) {
        let mut input = super::provider_profiles::turn_input("model-1");
        input.conversation_id = Some(CONVERSATION.into());
        input.user_message_id = Some(format!("accounting-user-{index}"));
        input.assistant_message_id = Some(format!("accounting-assistant-{index}"));
        input.content = format!("ACCOUNTING_USER_{index}");
        input.max_tokens = Some(MAX_TOKENS);
        input.permissions = self.permissions;
        let (notifications, events) = mpsc::unbounded_channel();
        (
            self.service
                .start_conversation_turn(input, notifications)
                .unwrap(),
            events,
        )
    }

    fn preview(&self) -> AgentContextWindowSnapshot {
        self.service
            .get_context_window_snapshot(AgentContextWindowSnapshotInput {
                conversation_id: Some(CONVERSATION.into()),
                project_id: None,
                model_id: "model-1".into(),
                max_tokens: Some(MAX_TOKENS),
                prompt_preferences: None,
                permissions: self.permissions,
                skills: vec![],
            })
            .unwrap()
            .snapshot
            .unwrap()
    }

    fn estimates(&self, run_id: &str) -> Vec<ModelRequestEstimate> {
        let connection = rusqlite::Connection::open_with_flags(
            self.directory.path().join("storage.sqlite"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        let mut observations = mycopilot_core::storage::model_request_observation_repository::list_observations_for_conversation(
            &connection, CONVERSATION,
        ).unwrap();
        observations.sort_by_key(|observation| observation.request_index);
        observations
            .into_iter()
            .filter(|observation| {
                observation.run_id == run_id
                    && observation.purpose == ModelRequestPurpose::AgentLoop
            })
            .map(|observation| {
                observation
                    .estimate
                    .expect("real send boundary must be measured")
            })
            .collect()
    }

    async fn finish(
        &self,
        run_id: &str,
        events: &mut mpsc::UnboundedReceiver<Value>,
    ) -> Vec<Value> {
        let events = collect_until_done(events).await;
        assert_eq!(
            events.last().unwrap()["params"]["status"],
            "completed",
            "{:?}",
            terminal_diagnostics(&events)
        );
        tokio::time::timeout(Duration::from_secs(5), async {
            while self
                .service
                .cancellations
                .lock()
                .unwrap()
                .contains_key(run_id)
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("completed Host worker must release its run");
        events
    }

    fn assert_terminal_reads(&self, events: &[Value]) -> AgentContextWindowSnapshot {
        let terminal: AgentContextWindowSnapshot = serde_json::from_value(
            events
                .iter()
                .rev()
                .find(|event| event["params"]["type"] == "context_window_updated")
                .expect("real terminal worker must publish context usage")["params"]["snapshot"]
                .clone(),
        )
        .unwrap();
        for _ in 0..2 {
            assert_eq!(
                self.preview(),
                terminal,
                "terminal event and repeated reads must agree"
            );
        }
        self.service
            .invalidate_conversation_context_state(CONVERSATION);
        assert_eq!(
            self.preview(),
            terminal,
            "cold read must count the same complete request"
        );
        terminal
    }
}

fn set_enabled(storage: &StorageService, enabled: bool) {
    let current = storage.load_agent_collaboration_settings().unwrap();
    if current.enabled != enabled {
        storage
            .update_agent_collaboration_settings(&AgentCollaborationSettingsUpdate {
                enabled,
                expected_revision: current.revision,
            })
            .unwrap();
    }
}

fn terminal_diagnostics(events: &[Value]) -> Vec<Value> {
    events
        .iter()
        .filter(|event| matches!(event["params"]["type"].as_str(), Some("done" | "error")))
        .map(|event| {
            let params = &event["params"];
            json!({"type":params["type"], "status":params["status"], "code":params["code"], "message":params["message"]})
        })
        .collect()
}

fn message_texts(request: &Value) -> Vec<&str> {
    request["messages"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|message| {
            let content = &message["content"];
            match content.as_str() {
                Some(text) => vec![text],
                None => content
                    .as_array()
                    .map(|parts| {
                        parts
                            .iter()
                            .filter_map(|part| part["text"].as_str())
                            .collect()
                    })
                    .unwrap_or_default(),
            }
        })
        .collect()
}

fn collaboration_directory(request: &Value) -> Value {
    let text = message_texts(request).join("\n");
    let directory = text
        .split_once("<agent_collaboration_directory>")
        .unwrap()
        .1
        .split_once("</agent_collaboration_directory>")
        .unwrap()
        .0;
    serde_json::from_str(directory).unwrap()
}

fn assert_collaboration_wire(request: &Value, enabled: bool) {
    assert_collaboration_world_state(request, enabled);
    let tools = request["tools"].as_array().unwrap();
    for name in COLLABORATION_TOOLS {
        assert_eq!(
            tools
                .iter()
                .filter(|tool| tool["function"]["name"] == name)
                .count(),
            usize::from(enabled),
            "actual wire schema presence for {name}"
        );
    }
    let text = message_texts(request).join("\n");
    assert_eq!(
        text.matches("## Agent 协作").count(),
        usize::from(enabled),
        "collaboration rules must occur exactly once when enabled"
    );
    assert_eq!(
        text.matches("委派是常设授权").count(),
        usize::from(enabled),
        "eager delegation authorization must occur exactly once when enabled"
    );
    assert_eq!(
        text.matches("<agent_collaboration_directory>").count(),
        usize::from(enabled),
        "collaboration directory must follow the same policy as schemas"
    );
    if enabled {
        let directory = collaboration_directory(request);
        assert!(
            directory["models"]
                .as_array()
                .unwrap()
                .iter()
                .any(|model| model["modelConfigId"] == "model-1"),
            "use the real nonempty Host selector directory"
        );
    }
}

fn assert_collaboration_world_state(request: &Value, enabled: bool) {
    let mut adopted = None;
    for text in message_texts(request) {
        if !text.contains("<backend_world_state_record>") {
            continue;
        }
        for record in text
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        {
            for section in record["sections"].as_array().into_iter().flatten() {
                if section["id"] == "agent.collaboration" {
                    adopted = Some(section["value"].clone());
                }
            }
            for change in record["changes"].as_array().into_iter().flatten() {
                if change["sectionId"] == "agent.collaboration" {
                    adopted = Some(change["value"].clone());
                }
            }
        }
    }
    let adopted =
        adopted.expect("wire World State must explicitly describe collaboration availability");
    assert_eq!(adopted["enabled"], enabled);
    assert_eq!(adopted["available"], enabled);
    assert_eq!(
        adopted["reason"],
        if enabled {
            "available"
        } else {
            "disabled_by_user"
        }
    );
}

fn assert_estimate(preview: &AgentContextWindowSnapshot, sent: &ModelRequestEstimate) {
    assert_eq!(preview.input_tokens, sent.estimated_input_tokens,
        "staged preview must include every layer counted at the real driver send boundary; preview={preview:?}, actual={sent:?}");
    assert_eq!(
        preview.cost_breakdown,
        mycopilot_core::protocol::AgentContextCostBreakdown {
            system_tokens: sent.system_tokens,
            tool_schema_tokens: sent.tool_schema_tokens,
            summary_tokens: sent.summary_tokens,
            world_state_tokens: sent.world_state_tokens,
            todo_tokens: sent.todo_tokens,
            provider_continuation_tokens: sent.provider_continuation_tokens,
            recent_history_tokens: sent.recent_history_tokens,
            total_input_tokens: sent.total_input_tokens,
        }
    );
    assert_eq!(preview.reserved_output_tokens, sent.reserved_output_tokens);
    assert_eq!(preview.safety_margin_tokens, sent.safety_margin_tokens);
    assert_eq!(
        preview.input_capacity_tokens,
        sent.context_window_tokens
            .map(|capacity| capacity - sent.reserved_output_tokens - sent.safety_margin_tokens)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_root_requests_and_staged_previews_count_enabled_and_disabled_collaboration_once() {
    for enabled in [true, false] {
        let (url, mut requests, server) = provider(
            (0..2)
                .map(|index| {
                    (
                        json!({"role":"assistant", "content":format!("ACCOUNTING_ANSWER_{index}")}),
                        "stop",
                    )
                })
                .collect(),
        )
        .await;
        let fixture = Fixture::new(&url, enabled);
        let mut previous_terminal: Option<AgentContextWindowSnapshot> = None;
        for index in 0..2 {
            let (turn, mut events) = fixture.start(index);
            let captured = next_request(&mut requests).await;
            assert_collaboration_wire(&captured.body, enabled);
            let staged_preview = fixture.preview();
            captured.release.send(()).unwrap();
            let events = fixture.finish(&turn.run_id, &mut events).await;
            let estimates = fixture.estimates(&turn.run_id);
            assert_eq!(estimates.len(), 1);
            assert_estimate(&staged_preview, &estimates[0]);
            let terminal = fixture.assert_terminal_reads(&events);
            if let Some(previous) = previous_terminal {
                assert!(
                    terminal.input_tokens > previous.input_tokens,
                    "a plain text turn cannot drop the collaboration layer between read paths"
                );
                assert_eq!(
                    terminal.cost_breakdown.tool_schema_tokens,
                    previous.cost_breakdown.tool_schema_tokens
                );
            }
            previous_terminal = Some(terminal);
        }
        server.await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn collaboration_switch_does_not_change_the_active_run_but_applies_to_the_next_root_run() {
    let (url, mut requests, server) = provider(vec![
        (json!({"role":"assistant", "tool_calls":[{"index":0,"id":"accounting-list", "type":"function", "function":{"name":"list_agents", "arguments":"{}"}}]}), "tool_calls"),
        (json!({"role":"assistant", "content":"ACTIVE_TREE_COMPLETE"}), "stop"),
        (json!({"role":"assistant", "content":"NEW_ROOT_COMPLETE"}), "stop"),
        (json!({"role":"assistant", "content":"NEW_DIRECTORY_ADOPTED"}), "stop"),
    ]).await;
    let fixture = Fixture::new(&url, true);
    let (turn, mut events) = fixture.start(0);
    let first = next_request(&mut requests).await;
    assert_collaboration_wire(&first.body, true);
    let initial_directory = collaboration_directory(&first.body);
    let first_preview = fixture.preview();
    set_enabled(&fixture.storage, false);
    let mut settings = fixture.storage.load_model_settings().unwrap().unwrap();
    let mut added_model = settings.models[0].clone();
    added_model.id = "model-added-after-run-start".into();
    added_model.provider_model_id = "added-provider-model".into();
    added_model.display_name = "Added after run start".into();
    settings.models.push(added_model);
    fixture.storage.save_model_settings(settings).unwrap();
    assert_eq!(
        fixture.preview(),
        first_preview,
        "changing global settings cannot rewrite the active request preview"
    );
    fixture
        .service
        .invalidate_conversation_context_state(CONVERSATION);
    assert_eq!(
        fixture.preview(),
        first_preview,
        "an active cold read must retain the run's policy and selector directory"
    );
    first.release.send(()).unwrap();
    let second = tokio::select! {
        request = next_request(&mut requests) => request,
        terminal = collect_until_done(&mut events) => {
            panic!("active run must continue after preview: {:?}", terminal_diagnostics(&terminal));
        }
    };
    assert_collaboration_wire(&second.body, true);
    assert_eq!(collaboration_directory(&second.body), initial_directory);
    let second_preview = fixture.preview();
    assert!(
        second.body["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| {
                message["role"] == "tool"
                    && message["content"]
                        .as_str()
                        .and_then(|content| serde_json::from_str::<Value>(content).ok())
                        .and_then(|result| result["agents"].as_array().cloned())
                        .is_some_and(|agents| {
                            agents.iter().any(|agent| agent["taskName"] == "主智能体")
                        })
            }),
        "already-started run must settle its collaboration tool normally"
    );
    second.release.send(()).unwrap();
    let events = fixture.finish(&turn.run_id, &mut events).await;
    let estimates = fixture.estimates(&turn.run_id);
    assert_eq!(estimates.len(), 2);
    assert_estimate(&first_preview, &estimates[0]);
    assert_estimate(&second_preview, &estimates[1]);
    fixture.assert_terminal_reads(&events);

    let (next_turn, mut next_events) = fixture.start(1);
    let next = next_request(&mut requests).await;
    assert_collaboration_wire(&next.body, false);
    let next_preview = fixture.preview();
    next.release.send(()).unwrap();
    let next_events = fixture.finish(&next_turn.run_id, &mut next_events).await;
    assert_estimate(&next_preview, &fixture.estimates(&next_turn.run_id)[0]);
    fixture.assert_terminal_reads(&next_events);

    set_enabled(&fixture.storage, true);
    let (enabled_turn, mut enabled_events) = fixture.start(2);
    let enabled = next_request(&mut requests).await;
    assert_collaboration_wire(&enabled.body, true);
    assert!(collaboration_directory(&enabled.body)["models"]
        .as_array()
        .unwrap()
        .iter()
        .any(|model| model["modelConfigId"] == "model-added-after-run-start"));
    let enabled_preview = fixture.preview();
    enabled.release.send(()).unwrap();
    let enabled_events = fixture
        .finish(&enabled_turn.run_id, &mut enabled_events)
        .await;
    assert_estimate(
        &enabled_preview,
        &fixture.estimates(&enabled_turn.run_id)[0],
    );
    fixture.assert_terminal_reads(&enabled_events);
    server.await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn automatic_compaction_plans_and_sent_requests_use_the_same_collaboration_accounting() {
    let (url, mut requests, server) = provider(vec![
        (
            json!({"role":"assistant", "content":"OLD_RESULT_DETAIL ".repeat(12_000)}),
            "stop",
        ),
        (
            json!({"role":"assistant", "content":"COMPACTION_ACCOUNTING_COMPLETE"}),
            "stop",
        ),
    ])
    .await;
    let mut fixture = Fixture::new(&url, true);
    // Use a real completed turn so the historical trace, request observations and World State
    // journal have exactly the same boundaries as production compaction inputs.
    let (history_turn, mut history_events) = fixture.start(99);
    next_request(&mut requests).await.release.send(()).unwrap();
    fixture
        .finish(&history_turn.run_id, &mut history_events)
        .await;
    let mut settings = test_model_settings();
    settings.api_url = url;
    settings.models[0].context_window_tokens = Some(64_000);
    fixture.storage.save_model_settings(settings).unwrap();

    let (generating, mut generation_requests) = mpsc::unbounded_channel();
    let generator = test_context_compaction_generator();
    let first_generation = Arc::new(std::sync::atomic::AtomicBool::new(true));
    fixture.service = fixture
        .service
        .clone()
        .with_context_compaction_summary_generator(Arc::new(move |request, cancellation| {
            let generator = generator.clone();
            let generating = generating.clone();
            let first_generation = first_generation.clone();
            Box::pin(async move {
                if first_generation.swap(false, Ordering::SeqCst) {
                    let (release, released) = oneshot::channel();
                    generating
                        .send((
                            request.operation_id.clone(),
                            request.prefix.clone(),
                            release,
                        ))
                        .unwrap();
                    released.await.unwrap();
                }
                generator(request, cancellation).await
            })
        }));
    let (turn, mut events) = fixture.start(0);
    let (operation_id, prefix, release) =
        tokio::time::timeout(Duration::from_secs(10), generation_requests.recv())
            .await
            .expect("long history must trigger real automatic compaction")
            .unwrap();
    let before_compaction = fixture.preview();
    let current_prefix = fixture
        .storage
        .prepare_context_compaction_prefix(CONVERSATION, &prefix.covered_through)
        .unwrap();
    assert_eq!(
        prefix.source_revision, current_prefix.source_revision,
        "a read-only staged preview must not invalidate the prepared compaction prefix"
    );
    let connection = rusqlite::Connection::open_with_flags(
        fixture.directory.path().join("storage.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let receipt = mycopilot_core::storage::context_compaction_receipt_repository::get_receipt(
        &connection,
        &operation_id,
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        receipt.plan.request_input_tokens, before_compaction.input_tokens,
        "automatic-compaction threshold must inspect the complete request also counted by the UI"
    );
    assert_eq!(
        receipt.plan.available_input_tokens,
        before_compaction.input_capacity_tokens
    );
    assert!(
        receipt.plan.request_input_tokens >= receipt.plan.request_trigger_input_tokens.unwrap()
    );
    release.send(()).unwrap();

    let sent = tokio::select! {
        sent = next_request(&mut requests) => sent,
        terminal = collect_until_done(&mut events) => {
            panic!("compaction must reach Provider instead of ending early: {:?}", terminal_diagnostics(&terminal));
        }
    };
    assert_collaboration_wire(&sent.body, true);
    let after_compaction = fixture.preview();
    assert!(after_compaction.input_tokens < before_compaction.input_tokens);
    assert!(after_compaction.cost_breakdown.summary_tokens > 0);
    assert_eq!(
        before_compaction.cost_breakdown.tool_schema_tokens,
        after_compaction.cost_breakdown.tool_schema_tokens
    );
    sent.release.send(()).unwrap();
    let events = fixture.finish(&turn.run_id, &mut events).await;
    let estimates = fixture.estimates(&turn.run_id);
    assert_eq!(estimates.len(), 1);
    assert_estimate(&after_compaction, &estimates[0]);
    fixture.assert_terminal_reads(&events);
    server.await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn approval_resume_keeps_frozen_collaboration_and_counts_the_resumed_request() {
    let (url, mut requests, server) = provider(vec![
        (json!({"role":"assistant", "tool_calls":[{"index":0,"id":"accounting-approval", "type":"function", "function":{"name":"run_command", "arguments":json!({"command":"printf accounting-command-must-not-run","cwd":std::env::temp_dir(),"reason":"Test approval rejection without executing a command"}).to_string()}}]}), "tool_calls"),
        (json!({"role":"assistant", "content":"APPROVAL_REJECTION_OBSERVED"}), "stop"),
    ]).await;
    let mut fixture = Fixture::new(&url, true);
    fixture.permissions.write = AgentWritePermission::All;
    let (turn, mut events) = fixture.start(0);
    let initial = next_request(&mut requests).await;
    assert_collaboration_wire(&initial.body, true);
    let initial_preview = fixture.preview();
    initial.release.send(()).unwrap();
    let waiting = collect_until_done(&mut events).await;
    assert_eq!(
        waiting.last().unwrap()["params"]["status"],
        "waiting_for_approval",
        "{:?}",
        terminal_diagnostics(&waiting)
    );
    let approval = fixture
        .service
        .list_pending_actions()
        .into_iter()
        .next()
        .expect("the real command request must be waiting for approval");
    set_enabled(&fixture.storage, false);
    let (notifications, mut resumed_events) = mpsc::unbounded_channel();
    let decision = fixture
        .service
        .reject_action(&turn.run_id, &approval.action_id, None, notifications)
        .unwrap();
    assert_eq!(decision.agent_output.status, AgentRunStatus::Running);
    let resumed = next_request(&mut requests).await;
    assert_collaboration_wire(&resumed.body, true);
    let resumed_preview = fixture.preview();
    let result = resumed.body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .rev()
        .find(|message| message["role"] == "tool")
        .unwrap()["content"]
        .as_str()
        .unwrap();
    let result: Value = serde_json::from_str(result).unwrap();
    assert_eq!(result["executionAttempted"], false);
    assert_eq!(result["code"], "command.approval_rejected");
    resumed.release.send(()).unwrap();
    let resumed_events = fixture.finish(&turn.run_id, &mut resumed_events).await;
    let estimates = fixture.estimates(&turn.run_id);
    assert_eq!(estimates.len(), 2);
    assert_estimate(&initial_preview, &estimates[0]);
    assert_estimate(&resumed_preview, &estimates[1]);
    fixture.assert_terminal_reads(&resumed_events);
    assert!(fixture
        .storage
        .list_agent_command_sessions(CONVERSATION, 10)
        .unwrap()
        .is_empty());
    server.await.unwrap();
}
