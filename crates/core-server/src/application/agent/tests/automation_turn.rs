use super::*;
use crate::application::agent::{
    AutomationExecutionContext, AutomationHumanRootDestination, AutomationHumanRootTurnStart,
    AutomationTurnObservation,
};
use mycopilot_core::storage::automation_repository::{
    self, AutomationCompareAndSetOutcome, AutomationConfigRecord, AutomationCreateOutcome,
    AutomationRunEnqueueOutcome, AutomationRunSettlementInput, NewAutomationRecord,
    NewManualAutomationRunRecord, StoredAutomationRunStatus, StoredAutomationStatus,
};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

fn seed_automation(
    storage: &StorageService,
    id: &str,
) -> mycopilot_core::storage::automation_repository::AutomationRecord {
    let permissions = AgentPermissions::default();
    let outcome = storage
        .create_automation(&NewAutomationRecord {
            id: id.to_string(),
            create_request_id: format!("create-{id}"),
            status: StoredAutomationStatus::Active,
            config: AutomationConfigRecord {
                title: "Automation HumanRoot".to_string(),
                prompt: "Run this durable automation turn.".to_string(),
                health_state: "ok".to_string(),
                blocked_code: None,
                blocked_message: None,
                destination_kind: "new_chat".to_string(),
                target_conversation_id: None,
                project_binding_kind: "none".to_string(),
                project_id: None,
                model_id: Some("model-1".to_string()),
                permission_mode: "default".to_string(),
                permission_mode_version: 2,
                permissions_json: serde_json::to_string(&permissions).unwrap(),
                reasoning_json: Some(r#"{"source":"model_config"}"#.to_string()),
                schedule_kind: "daily".to_string(),
                schedule_json: r#"{"kind":"daily","time":"09:00","timezone":"UTC"}"#.to_string(),
                rrule: "FREQ=DAILY;BYHOUR=9;BYMINUTE=0;BYSECOND=0".to_string(),
                timezone: "UTC".to_string(),
                anchor_at: 1,
                next_run_at: Some(now_ms().saturating_add(86_400_000)),
                notification_policy: "all_runs".to_string(),
                target_project_snapshot: None,
                target_conversation_snapshot: None,
                target_model_snapshot: Some("Model 1".to_string()),
                target_project_id_snapshot: None,
                target_conversation_id_snapshot: None,
                target_model_id_snapshot: Some("model-1".to_string()),
            },
        })
        .unwrap();
    match outcome {
        AutomationCreateOutcome::Created(task) | AutomationCreateOutcome::Existing(task) => task,
    }
}

fn enqueue_and_claim(
    database_path: &std::path::Path,
    storage: &StorageService,
    task: &mycopilot_core::storage::automation_repository::AutomationRecord,
    request_id: &str,
) -> mycopilot_core::storage::automation_repository::AutomationRunRecord {
    let run_id = format!("automation-run-{request_id}");
    let outcome = storage
        .enqueue_manual_automation_run(&NewManualAutomationRunRecord {
            id: run_id.clone(),
            automation_id: task.id.clone(),
            manual_request_id: request_id.to_string(),
            scheduled_for: now_ms(),
            config_revision: task.revision,
            config_snapshot_json: "{}".to_string(),
            expected_revision: task.revision,
        })
        .unwrap();
    assert!(matches!(outcome, AutomationRunEnqueueOutcome::Enqueued(_)));
    let mut connection = rusqlite::Connection::open(database_path).unwrap();
    let claimed = automation_repository::claim_ready_automation_runs(
        &mut connection,
        now_ms().saturating_add(1),
        30_000,
        1,
    )
    .unwrap();
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].id, run_id);
    claimed.into_iter().next().unwrap()
}

fn automation_start(
    task: &mycopilot_core::storage::automation_repository::AutomationRecord,
    run: &mycopilot_core::storage::automation_repository::AutomationRunRecord,
    destination: AutomationHumanRootDestination,
) -> AutomationHumanRootTurnStart {
    AutomationHumanRootTurnStart {
        context: AutomationExecutionContext {
            automation_id: task.id.clone(),
            automation_run_id: run.id.clone(),
            scheduled_for: run.scheduled_for,
            last_run_at: task.last_run_at,
            trigger_kind: run.trigger_kind.clone(),
        },
        admission_token: run
            .admission_token
            .clone()
            .expect("claimed run has an admission token"),
        config_revision: run.config_revision,
        title: task.config.title.clone(),
        prompt: task.config.prompt.clone(),
        destination,
        permission_mode: task.config.permission_mode.clone(),
        permissions: serde_json::from_str(&task.config.permissions_json).unwrap(),
    }
}

async fn read_provider_request(stream: &mut TcpStream) -> Value {
    let mut request = Vec::new();
    let mut buffer = [0_u8; 4_096];
    let mut body_start = None;
    let mut expected_length = None;
    loop {
        let read = stream.read(&mut buffer).await.unwrap();
        assert!(read > 0, "provider fixture closed before request completed");
        request.extend_from_slice(&buffer[..read]);
        if body_start.is_none() {
            if let Some(index) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                let start = index + 4;
                let headers = String::from_utf8_lossy(&request[..index]);
                let content_length = headers
                    .lines()
                    .find_map(|line| {
                        line.split_once(':').and_then(|(name, value)| {
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().ok())
                                .flatten()
                        })
                    })
                    .unwrap();
                body_start = Some(start);
                expected_length = Some(start + content_length);
            }
        }
        if expected_length.is_some_and(|length| request.len() >= length) {
            break;
        }
    }
    serde_json::from_slice(&request[body_start.unwrap()..expected_length.unwrap()]).unwrap()
}

async fn write_provider_stream(stream: &mut TcpStream, content: &str) {
    stream
        .write_all(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
        )
        .await
        .unwrap();
    let frame = json!({
        "choices": [{ "delta": { "role": "assistant", "content": content }, "finish_reason": null }]
    });
    let finish = json!({
        "choices": [{ "delta": {}, "finish_reason": "stop" }]
    });
    stream
        .write_all(format!("data: {frame}\n\ndata: {finish}\n\ndata: [DONE]\n\n").as_bytes())
        .await
        .unwrap();
}

fn automation_provider_system_context(request: &Value) -> &str {
    let matching = request["messages"]
        .as_array()
        .expect("provider request has messages")
        .iter()
        .filter(|message| {
            message["role"] == "system"
                && message["content"]
                    .as_str()
                    .is_some_and(|content| content.contains("AUTOMATION_EXECUTION_CONTEXT_V1"))
        })
        .collect::<Vec<_>>();
    assert_eq!(
        matching.len(),
        1,
        "provider request must contain exactly one Host-only Automation system context: {request}"
    );
    matching[0]["content"].as_str().unwrap()
}

async fn wait_for_done(receiver: &mut tokio::sync::mpsc::UnboundedReceiver<Value>) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let event = receiver.recv().await.expect("Agent event channel closed");
            if event["params"]["type"] == "done" {
                assert_eq!(event["params"]["status"], "completed", "{event:?}");
                return;
            }
        }
    })
    .await
    .expect("timed out waiting for the automation HumanRoot Turn");
}

fn settle_completed_run(
    database_path: &std::path::Path,
    automation_run_id: &str,
    agent_run_id: &str,
) {
    let mut connection = rusqlite::Connection::open(database_path).unwrap();
    let outcome = automation_repository::settle_automation_run_from_trace(
        &mut connection,
        &AutomationRunSettlementInput {
            automation_run_id: automation_run_id.to_string(),
            agent_run_id: agent_run_id.to_string(),
            terminal_status: StoredAutomationRunStatus::Completed,
            report_kind: Some("unknown".to_string()),
            result_preview: Some("Automation complete".to_string()),
            error_code: None,
            error_message: None,
            settled_at: now_ms(),
        },
    )
    .unwrap();
    assert!(matches!(
        outcome,
        automation_repository::AutomationRunMutationOutcome::Updated(_)
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn automation_human_root_uses_atomic_admission_and_new_chat_per_run() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let model_server = tokio::spawn(async move {
        let mut requests = Vec::new();
        for answer in [
            "First automation answer.",
            "Second automation answer.",
            "Existing chat automation answer.",
        ] {
            let (mut stream, _) = listener.accept().await.unwrap();
            requests.push(read_provider_request(&mut stream).await);
            write_provider_stream(&mut stream, answer).await;
        }
        requests
    });

    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("storage.sqlite");
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    let mut settings = test_model_settings();
    settings.api_url = format!("http://{address}/v1/chat/completions");
    storage.save_model_settings(settings).unwrap();
    let service =
        AgentService::try_new_deferred_startup_reconciliation(Arc::clone(&storage)).unwrap();
    assert!(service
        .automation_report_sink_for_agent_run_id("ordinary-human-root-run")
        .unwrap()
        .is_none());
    let task = seed_automation(&storage, "automation-humanroot-new-chat");

    let run_one = enqueue_and_claim(&database_path, &storage, &task, "manual-one");
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let turn_one = service
        .start_automation_human_root_turn(
            automation_start(
                &task,
                &run_one,
                AutomationHumanRootDestination::NewChat {
                    project_id: None,
                    model_id: "model-1".to_string(),
                },
            ),
            notifications,
        )
        .unwrap();
    let admitted = automation_repository::get_automation_run(
        &rusqlite::Connection::open(&database_path).unwrap(),
        &run_one.id,
    )
    .unwrap()
    .unwrap();
    assert_eq!(admitted.status, StoredAutomationRunStatus::Running);
    assert_eq!(
        admitted.agent_run_id.as_deref(),
        Some(turn_one.run_id.as_str())
    );
    assert_eq!(
        admitted.user_message_id.as_deref(),
        Some(turn_one.user_message_id.as_str())
    );
    assert_eq!(
        admitted.assistant_message_id.as_deref(),
        Some(turn_one.assistant_message_id.as_str())
    );
    let report_sink = service
        .automation_report_sink_for_agent_run_id(&turn_one.run_id)
        .unwrap()
        .expect("admitted Automation run has a durable report sink");
    report_sink
        .record(
            mycopilot_core::AutomationReportKind::ImportantUpdate,
            "A durable important update.",
        )
        .unwrap();
    let reported = storage.get_automation_run(&run_one.id).unwrap().unwrap();
    assert_eq!(reported.report_kind.as_deref(), Some("important_update"));
    assert_eq!(
        reported.result_preview.as_deref(),
        Some("A durable important update.")
    );
    wait_for_done(&mut receiver).await;
    assert!(matches!(
        service
            .automation_turn_state(&turn_one.run_id, &turn_one.assistant_message_id)
            .unwrap(),
        AutomationTurnObservation::Terminal {
            status: ConversationTurnTraceTerminalStatus::Completed,
            ..
        }
    ));
    settle_completed_run(&database_path, &run_one.id, &turn_one.run_id);

    let task = storage.get_automation(&task.id).unwrap().unwrap();
    let run_two = enqueue_and_claim(&database_path, &storage, &task, "manual-two");
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let turn_two = service
        .start_automation_human_root_turn(
            automation_start(
                &task,
                &run_two,
                AutomationHumanRootDestination::NewChat {
                    project_id: None,
                    model_id: "model-1".to_string(),
                },
            ),
            notifications,
        )
        .unwrap();
    assert_ne!(turn_one.conversation_id, turn_two.conversation_id);
    assert_ne!(turn_one.user_message_id, turn_two.user_message_id);
    assert_ne!(turn_one.assistant_message_id, turn_two.assistant_message_id);
    wait_for_done(&mut receiver).await;

    let existing_task = seed_automation(&storage, "automation-humanroot-existing-chat");
    let existing_last_run_at = now_ms().saturating_sub(60_000);
    rusqlite::Connection::open(&database_path)
        .unwrap()
        .execute(
            "UPDATE automations SET last_run_at = ?1 WHERE id = ?2",
            rusqlite::params![existing_last_run_at, &existing_task.id],
        )
        .unwrap();
    let existing_task = storage.get_automation(&existing_task.id).unwrap().unwrap();
    let existing_run = enqueue_and_claim(
        &database_path,
        &storage,
        &existing_task,
        "manual-existing-chat",
    );
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let existing_turn = service
        .start_automation_human_root_turn(
            automation_start(
                &existing_task,
                &existing_run,
                AutomationHumanRootDestination::ExistingChat {
                    conversation_id: turn_one.conversation_id.clone(),
                },
            ),
            notifications,
        )
        .unwrap();
    assert_eq!(existing_turn.conversation_id, turn_one.conversation_id);
    wait_for_done(&mut receiver).await;
    let existing_conversation = storage
        .load_conversation(&turn_one.conversation_id)
        .unwrap()
        .unwrap();
    assert_eq!(existing_conversation.messages.len(), 4);
    assert_eq!(
        existing_conversation.messages[2].id,
        existing_turn.user_message_id
    );
    assert_eq!(existing_conversation.messages[2].role, "user");
    assert_eq!(
        existing_conversation.messages[2].content,
        existing_task.config.prompt
    );

    let requests = model_server.await.unwrap();
    assert_eq!(requests.len(), 3);
    assert!(requests.iter().all(
        |request| request["tools"].as_array().is_some_and(|tools| tools
            .iter()
            .any(|tool| tool["function"]["name"].as_str() == Some("automation_report")))
    ));
    assert!(requests.iter().all(
        |request| request["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| message["role"] == "user"
                && message["content"]
                    .as_str()
                    .is_some_and(|content| content.contains(&task.config.prompt)))
    ));
    assert!(requests.iter().all(|request| request["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["role"] == "user")
        .all(|message| !message["content"]
            .as_str()
            .is_some_and(|content| content.contains("AUTOMATION_EXECUTION_CONTEXT_V1")))));

    let first_context = automation_provider_system_context(&requests[0]);
    assert!(first_context.contains(&format!(r#""automationId":"{}""#, task.id)));
    assert!(first_context.contains(&format!(r#""automationRunId":"{}""#, run_one.id)));
    assert!(first_context.contains(r#""lastRunAt":null"#));
    assert!(first_context.contains(r#""triggerKind":"manual""#));

    let existing_context = automation_provider_system_context(&requests[2]);
    assert!(existing_context.contains(&format!(r#""automationId":"{}""#, existing_task.id)));
    assert!(existing_context.contains(&format!(r#""automationRunId":"{}""#, existing_run.id)));
    assert!(existing_context.contains(&format!(r#""scheduledFor":{}"#, existing_run.scheduled_for)));
    assert!(existing_context.contains(&format!(r#""lastRunAt":{existing_last_run_at}"#)));
    assert!(existing_context.contains(r#""triggerKind":"manual""#));
    assert!(storage
        .list_notifications(None, 100, false, None)
        .unwrap()
        .items
        .iter()
        .all(|event| event.source_kind != "human_root"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stale_automation_admission_rolls_back_conversation_messages_and_trace() {
    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("storage.sqlite");
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    storage.save_model_settings(test_model_settings()).unwrap();
    let service = AgentService::try_new(Arc::clone(&storage)).unwrap();
    let task = seed_automation(&storage, "automation-humanroot-stale");
    let run = enqueue_and_claim(&database_path, &storage, &task, "manual-stale");
    let before = storage
        .load_conversation_metas()
        .unwrap()
        .into_iter()
        .map(|conversation| conversation.id)
        .collect::<Vec<_>>();
    let mut start = automation_start(
        &task,
        &run,
        AutomationHumanRootDestination::NewChat {
            project_id: None,
            model_id: "model-1".to_string(),
        },
    );
    start.admission_token = "automation-admission:stale-token".to_string();
    let (notifications, _receiver) = tokio::sync::mpsc::unbounded_channel();
    let error = service
        .start_automation_human_root_turn(start, notifications)
        .unwrap_err();
    assert!(error.to_string().contains("claim is stale"), "{error}");
    assert_eq!(
        storage
            .load_conversation_metas()
            .unwrap()
            .into_iter()
            .map(|conversation| conversation.id)
            .collect::<Vec<_>>(),
        before
    );
    let current = automation_repository::get_automation_run(
        &rusqlite::Connection::open(&database_path).unwrap(),
        &run.id,
    )
    .unwrap()
    .unwrap();
    assert_eq!(current.status, StoredAutomationRunStatus::Admitting);
    assert!(current.agent_run_id.is_none());
    assert!(current.conversation_id.is_none());
    assert!(current.user_message_id.is_none());
    assert!(current.assistant_message_id.is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn permission_revoked_after_precheck_is_blocked_atomically_before_humanroot_admission() {
    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("storage.sqlite");
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    storage.save_model_settings(test_model_settings()).unwrap();

    let original = seed_automation(&storage, "automation-humanroot-permission-toctou");
    let mut config = original.config.clone();
    config.permission_mode = "full".to_string();
    config.permissions_json = serde_json::to_string(&mycopilot_core::AgentPermissions {
        read: mycopilot_core::AgentReadPermission::All,
        write: mycopilot_core::AgentWritePermission::All,
        command: mycopilot_core::AgentCommandPermission::AutoApprove,
        command_safety: mycopilot_core::AgentCommandSafetyPolicy::Guarded,
        patch: mycopilot_core::AgentPatchPermission::AutoApprove,
        builtin_execution: mycopilot_core::AgentBuiltinExecutionPermission::AutoApprove,
    })
    .unwrap();
    let task = match storage
        .replace_automation_config(&original.id, original.revision, &config)
        .unwrap()
    {
        AutomationCompareAndSetOutcome::Updated(task) => task,
        outcome => panic!("permission-race fixture update failed: {outcome:?}"),
    };
    let run = enqueue_and_claim(&database_path, &storage, &task, "permission-toctou");

    // This is the scheduler's read-only precheck. The preference mutation commits after it but
    // before the HumanRoot admission transaction starts.
    let mut preferences = storage.load_ui_preferences().unwrap();
    assert!(preferences.full_permission_enabled);
    preferences.full_permission_enabled = false;
    storage.save_ui_preferences(preferences).unwrap();

    let facts_before = rusqlite::Connection::open(&database_path)
        .unwrap()
        .query_row(
            "SELECT
                 (SELECT COUNT(*) FROM conversations),
                 (SELECT COUNT(*) FROM messages),
                 (SELECT COUNT(*) FROM conversation_turn_traces)",
            [],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .unwrap();
    let service =
        AgentService::try_new_deferred_startup_reconciliation(Arc::clone(&storage)).unwrap();
    let (notifications, _receiver) = tokio::sync::mpsc::unbounded_channel();
    let error = service
        .start_automation_human_root_turn(
            automation_start(
                &task,
                &run,
                AutomationHumanRootDestination::NewChat {
                    project_id: None,
                    model_id: "model-1".to_string(),
                },
            ),
            notifications,
        )
        .unwrap_err();
    assert_eq!(
        error,
        crate::application::agent::AutomationHumanRootStartError::TargetInvalid {
            code: "permission_disabled",
            message: automation_repository::AUTOMATION_PERMISSION_DISABLED_MESSAGE.to_string(),
        }
    );

    let connection = rusqlite::Connection::open(&database_path).unwrap();
    let facts_after = connection
        .query_row(
            "SELECT
                 (SELECT COUNT(*) FROM conversations),
                 (SELECT COUNT(*) FROM messages),
                 (SELECT COUNT(*) FROM conversation_turn_traces)",
            [],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(facts_after, facts_before);

    let blocked_task = storage.get_automation(&task.id).unwrap().unwrap();
    assert_eq!(blocked_task.status, StoredAutomationStatus::Active);
    assert_eq!(blocked_task.config.health_state, "blocked");
    assert_eq!(
        blocked_task.config.blocked_code.as_deref(),
        Some("permission_disabled")
    );
    assert_eq!(
        blocked_task.config.blocked_message.as_deref(),
        Some(automation_repository::AUTOMATION_PERMISSION_DISABLED_MESSAGE)
    );
    assert!(blocked_task.config.next_run_at.is_none());
    assert!(blocked_task.attention_required_at.is_some());

    let failed_run = storage.get_automation_run(&run.id).unwrap().unwrap();
    assert_eq!(failed_run.status, StoredAutomationRunStatus::Failed);
    assert_eq!(
        failed_run.error_code.as_deref(),
        Some("permission_disabled")
    );
    assert_eq!(failed_run.report_kind.as_deref(), Some("unknown"));
    assert!(failed_run.attention_required_at.is_some());
    assert!(failed_run.completed_at.is_some());
    assert!(failed_run.agent_run_id.is_none());
    assert!(failed_run.conversation_id.is_none());
    assert!(failed_run.user_message_id.is_none());
    assert!(failed_run.assistant_message_id.is_none());

    let projection_counts = connection
        .query_row(
            "SELECT
                 (SELECT COUNT(*) FROM automation_events
                  WHERE automation_id = ?1 AND automation_run_id IS NULL
                    AND event_kind = 'attention_changed'),
                 (SELECT COUNT(*) FROM automation_events
                  WHERE automation_id = ?1 AND automation_run_id = ?2
                    AND event_kind = 'run_updated'
                    AND json_extract(payload_json, '$.status') = 'failed'),
                 (SELECT COUNT(*) FROM automation_events
                  WHERE automation_id = ?1 AND automation_run_id = ?2
                    AND event_kind = 'attention_changed'),
                 (SELECT COUNT(*) FROM automation_events
                  WHERE automation_id = ?1 AND automation_run_id IS NULL
                    AND event_kind = 'notification_requested'),
                 (SELECT COUNT(*) FROM notification_events
                  WHERE automation_id = ?1 AND run_id IS NULL
                    AND notification_kind = 'automation_configuration_blocked')",
            rusqlite::params![&task.id, &run.id],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(projection_counts, (1, 1, 1, 1, 1));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn archived_existing_chat_is_a_repairable_target_error_without_admission() {
    const CONVERSATION_ID: &str = "automation-archived-existing-chat";

    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("storage.sqlite");
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    storage.save_model_settings(test_model_settings()).unwrap();
    storage
        .save_conversation(ChatConversationRecord {
            id: CONVERSATION_ID.to_string(),
            project_id: None,
            model_id: Some("model-1".to_string()),
            title: "Archived automation target".to_string(),
            messages: Vec::new(),
            created_at: 1,
            updated_at: 2,
            pinned_at: None,
            archived_at: Some(2),
            unread_at: None,
        })
        .unwrap();
    let service = AgentService::try_new(Arc::clone(&storage)).unwrap();
    let task = seed_automation(&storage, "automation-humanroot-archived-existing");
    let run = enqueue_and_claim(&database_path, &storage, &task, "manual-archived-existing");
    let (notifications, _receiver) = tokio::sync::mpsc::unbounded_channel();
    let error = service
        .start_automation_human_root_turn(
            automation_start(
                &task,
                &run,
                AutomationHumanRootDestination::ExistingChat {
                    conversation_id: CONVERSATION_ID.to_string(),
                },
            ),
            notifications,
        )
        .unwrap_err();
    assert_eq!(
        error,
        crate::application::agent::AutomationHumanRootStartError::TargetInvalid {
            code: "target_archived",
            message: "The target conversation is archived.".to_string(),
        }
    );
    let current = storage.get_automation_run(&run.id).unwrap().unwrap();
    assert_eq!(current.status, StoredAutomationRunStatus::Admitting);
    assert!(current.agent_run_id.is_none());
    assert!(storage
        .list_conversation_turn_traces(CONVERSATION_ID)
        .unwrap()
        .is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exhausted_shared_agent_gate_is_retryable_without_admission() {
    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("storage.sqlite");
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    storage.save_model_settings(test_model_settings()).unwrap();
    let service = AgentService::try_new(Arc::clone(&storage)).unwrap();
    let gate = service.turn_concurrency_gate();
    let _manual_turn_permits = (0..gate.limit())
        .map(|_| gate.try_acquire().unwrap())
        .collect::<Vec<_>>();

    let task = seed_automation(&storage, "automation-humanroot-global-capacity");
    let run = enqueue_and_claim(&database_path, &storage, &task, "manual-global-capacity");
    let (notifications, _receiver) = tokio::sync::mpsc::unbounded_channel();
    let error = service
        .start_automation_human_root_turn(
            automation_start(
                &task,
                &run,
                AutomationHumanRootDestination::NewChat {
                    project_id: None,
                    model_id: "model-1".to_string(),
                },
            ),
            notifications,
        )
        .unwrap_err();
    assert_eq!(
        error,
        crate::application::agent::AutomationHumanRootStartError::RetryableCapacity
    );
    let current = storage.get_automation_run(&run.id).unwrap().unwrap();
    assert_eq!(current.status, StoredAutomationRunStatus::Admitting);
    assert!(current.agent_run_id.is_none());
    assert!(storage.load_conversation_metas().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn existing_chat_with_active_human_root_turn_is_retryable_without_admission() {
    const CONVERSATION_ID: &str = "automation-busy-existing-chat";
    const ACTIVE_RUN_ID: &str = "active-human-root-run";
    const ACTIVE_ASSISTANT_ID: &str = "active-human-root-assistant";

    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("storage.sqlite");
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    storage.save_model_settings(test_model_settings()).unwrap();
    storage
        .save_conversation(ChatConversationRecord {
            id: CONVERSATION_ID.to_string(),
            project_id: None,
            model_id: Some("model-1".to_string()),
            title: "Busy automation target".to_string(),
            messages: vec![
                ChatMessageRecord {
                    human_interaction_response: None,
                    id: "active-human-root-user".to_string(),
                    role: "user".to_string(),
                    content: "Existing active turn".to_string(),
                    created_at: 1,
                    status: Some("sent".to_string()),
                    attachments: Vec::new(),
                    agent_run_json: None,
                    ui_state_json: None,
                },
                ChatMessageRecord {
                    human_interaction_response: None,
                    id: ACTIVE_ASSISTANT_ID.to_string(),
                    role: "assistant".to_string(),
                    content: String::new(),
                    created_at: 2,
                    status: Some("pending".to_string()),
                    attachments: Vec::new(),
                    agent_run_json: None,
                    ui_state_json: None,
                },
            ],
            created_at: 1,
            updated_at: 2,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
    storage
        .append_in_progress_conversation_turn_trace(
            &mycopilot_core::ConversationTraceSnapshot::default().in_progress_trace(
                ACTIVE_RUN_ID,
                CONVERSATION_ID,
                ACTIVE_ASSISTANT_ID,
            ),
            2,
            2,
        )
        .unwrap();

    let service =
        AgentService::try_new_deferred_startup_reconciliation(Arc::clone(&storage)).unwrap();
    let task = seed_automation(&storage, "automation-humanroot-busy-existing");
    let run = enqueue_and_claim(&database_path, &storage, &task, "manual-busy-existing");
    let (notifications, _receiver) = tokio::sync::mpsc::unbounded_channel();
    let error = service
        .start_automation_human_root_turn(
            automation_start(
                &task,
                &run,
                AutomationHumanRootDestination::ExistingChat {
                    conversation_id: CONVERSATION_ID.to_string(),
                },
            ),
            notifications,
        )
        .unwrap_err();
    assert_eq!(
        error,
        crate::application::agent::AutomationHumanRootStartError::RetryableConversationBusy
    );
    let current = automation_repository::get_automation_run(
        &rusqlite::Connection::open(&database_path).unwrap(),
        &run.id,
    )
    .unwrap()
    .unwrap();
    assert_eq!(current.status, StoredAutomationRunStatus::Admitting);
    assert!(current.agent_run_id.is_none());
    let conversation = storage.load_conversation(CONVERSATION_ID).unwrap().unwrap();
    assert_eq!(conversation.messages.len(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn destructive_resource_mutations_terminalize_live_automation_runs_before_trace_deletion() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (accepted_sender, mut accepted_receiver) = tokio::sync::mpsc::unbounded_channel();
    let model_server = tokio::spawn(async move {
        for _ in 0..3 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let accepted_sender = accepted_sender.clone();
            tokio::spawn(async move {
                let _request = read_provider_request(&mut stream).await;
                accepted_sender.send(()).unwrap();
                let mut buffer = [0_u8; 256];
                while stream.read(&mut buffer).await.unwrap_or(0) > 0 {}
            });
        }
    });

    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("storage.sqlite");
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    let mut settings = test_model_settings();
    settings.api_url = format!("http://{address}/v1/chat/completions");
    storage.save_model_settings(settings).unwrap();
    let project_path = fixture.path().join("automation-delete-project");
    std::fs::create_dir_all(&project_path).unwrap();
    storage
        .save_project(ProjectRecord::with_primary_folder(
            "automation-delete-project".to_string(),
            "Automation delete project".to_string(),
            project_path.to_string_lossy().into_owned(),
            1,
        ))
        .unwrap();
    let service =
        AgentService::try_new_deferred_startup_reconciliation(Arc::clone(&storage)).unwrap();

    storage
        .save_conversation(ChatConversationRecord {
            id: "automation-delete-conversation".to_string(),
            project_id: None,
            model_id: Some("model-1".to_string()),
            title: "Delete live automation conversation".to_string(),
            messages: Vec::new(),
            created_at: 1,
            updated_at: 1,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
    let original = seed_automation(&storage, "automation-delete-live-conversation");
    let mut config = original.config.clone();
    config.destination_kind = "existing_chat".to_string();
    config.target_conversation_id = Some("automation-delete-conversation".to_string());
    config.project_binding_kind = "inherit".to_string();
    config.model_id = None;
    config.reasoning_json = None;
    config.target_conversation_snapshot = Some("Delete live automation conversation".to_string());
    config.target_conversation_id_snapshot = Some("automation-delete-conversation".to_string());
    let conversation_task = match storage
        .replace_automation_config(&original.id, original.revision, &config)
        .unwrap()
    {
        AutomationCompareAndSetOutcome::Updated(task) => task,
        outcome => panic!("conversation task update failed: {outcome:?}"),
    };
    let conversation_run = enqueue_and_claim(
        &database_path,
        &storage,
        &conversation_task,
        "delete-live-conversation",
    );
    let (notifications, _receiver) = tokio::sync::mpsc::unbounded_channel();
    service
        .start_automation_human_root_turn(
            automation_start(
                &conversation_task,
                &conversation_run,
                AutomationHumanRootDestination::ExistingChat {
                    conversation_id: "automation-delete-conversation".to_string(),
                },
            ),
            notifications,
        )
        .unwrap();
    accepted_receiver.recv().await.unwrap();
    service
        .delete_conversation("automation-delete-conversation")
        .unwrap();
    let deleted_conversation_run = storage
        .get_automation_run(&conversation_run.id)
        .unwrap()
        .unwrap();
    assert_eq!(
        deleted_conversation_run.status,
        StoredAutomationRunStatus::Cancelled
    );
    assert_eq!(
        deleted_conversation_run.error_code.as_deref(),
        Some("conversation_deleted")
    );
    assert!(deleted_conversation_run.completed_at.is_some());
    assert!(deleted_conversation_run.conversation_id.is_none());
    assert!(deleted_conversation_run.user_message_id.is_none());
    assert!(deleted_conversation_run.assistant_message_id.is_none());
    let blocked_conversation_task = storage
        .get_automation(&conversation_task.id)
        .unwrap()
        .unwrap();
    assert_eq!(blocked_conversation_task.config.health_state, "blocked");
    assert_eq!(
        blocked_conversation_task.config.blocked_code.as_deref(),
        Some("target_missing")
    );
    assert!(blocked_conversation_task.config.next_run_at.is_none());

    let original = seed_automation(&storage, "automation-delete-live-project");
    let mut config = original.config.clone();
    config.project_binding_kind = "project".to_string();
    config.project_id = Some("automation-delete-project".to_string());
    config.target_project_snapshot = Some("Automation delete project".to_string());
    config.target_project_id_snapshot = Some("automation-delete-project".to_string());
    let project_task = match storage
        .replace_automation_config(&original.id, original.revision, &config)
        .unwrap()
    {
        AutomationCompareAndSetOutcome::Updated(task) => task,
        outcome => panic!("project task update failed: {outcome:?}"),
    };
    let project_run = enqueue_and_claim(
        &database_path,
        &storage,
        &project_task,
        "delete-live-project",
    );
    let (notifications, _receiver) = tokio::sync::mpsc::unbounded_channel();
    let project_turn = service
        .start_automation_human_root_turn(
            automation_start(
                &project_task,
                &project_run,
                AutomationHumanRootDestination::NewChat {
                    project_id: Some("automation-delete-project".to_string()),
                    model_id: "model-1".to_string(),
                },
            ),
            notifications,
        )
        .unwrap();
    accepted_receiver.recv().await.unwrap();
    service.delete_project("automation-delete-project").unwrap();
    assert!(storage
        .load_conversation(&project_turn.conversation_id)
        .unwrap()
        .is_none());
    let deleted_project_run = storage
        .get_automation_run(&project_run.id)
        .unwrap()
        .unwrap();
    assert_eq!(
        deleted_project_run.status,
        StoredAutomationRunStatus::Cancelled
    );
    assert_eq!(
        deleted_project_run.error_code.as_deref(),
        Some("project_deleted")
    );
    assert!(deleted_project_run.completed_at.is_some());
    assert!(deleted_project_run.conversation_id.is_none());
    let blocked_project_task = storage.get_automation(&project_task.id).unwrap().unwrap();
    assert_eq!(blocked_project_task.config.health_state, "blocked");
    assert_eq!(
        blocked_project_task.config.blocked_code.as_deref(),
        Some("project_missing")
    );
    assert!(blocked_project_task.config.next_run_at.is_none());

    storage
        .save_conversation(ChatConversationRecord {
            id: "automation-delete-messages".to_string(),
            project_id: None,
            model_id: Some("model-1".to_string()),
            title: "Delete live automation messages".to_string(),
            messages: Vec::new(),
            created_at: 1,
            updated_at: 1,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
    let original = seed_automation(&storage, "automation-delete-live-messages");
    let mut config = original.config.clone();
    config.destination_kind = "existing_chat".to_string();
    config.target_conversation_id = Some("automation-delete-messages".to_string());
    config.project_binding_kind = "inherit".to_string();
    config.model_id = None;
    config.reasoning_json = None;
    config.target_conversation_snapshot = Some("Delete live automation messages".to_string());
    config.target_conversation_id_snapshot = Some("automation-delete-messages".to_string());
    let message_task = match storage
        .replace_automation_config(&original.id, original.revision, &config)
        .unwrap()
    {
        AutomationCompareAndSetOutcome::Updated(task) => task,
        outcome => panic!("message task update failed: {outcome:?}"),
    };
    let message_run = enqueue_and_claim(
        &database_path,
        &storage,
        &message_task,
        "delete-live-messages",
    );
    let (notifications, _receiver) = tokio::sync::mpsc::unbounded_channel();
    let message_turn = service
        .start_automation_human_root_turn(
            automation_start(
                &message_task,
                &message_run,
                AutomationHumanRootDestination::ExistingChat {
                    conversation_id: "automation-delete-messages".to_string(),
                },
            ),
            notifications,
        )
        .unwrap();
    accepted_receiver.recv().await.unwrap();
    service
        .delete_chat_messages(
            "automation-delete-messages",
            &[
                message_turn.user_message_id.clone(),
                message_turn.assistant_message_id.clone(),
            ],
        )
        .unwrap();
    let deleted_message_run = storage
        .get_automation_run(&message_run.id)
        .unwrap()
        .unwrap();
    assert_eq!(
        deleted_message_run.status,
        StoredAutomationRunStatus::Cancelled
    );
    assert_eq!(
        deleted_message_run.error_code.as_deref(),
        Some("messages_deleted")
    );
    assert!(deleted_message_run.completed_at.is_some());
    assert!(deleted_message_run.user_message_id.is_none());
    assert!(deleted_message_run.assistant_message_id.is_none());
    let still_valid_message_task = storage.get_automation(&message_task.id).unwrap().unwrap();
    assert_eq!(still_valid_message_task.config.health_state, "ok");
    assert!(still_valid_message_task.config.next_run_at.is_some());

    assert!(storage
        .list_nonterminal_automation_runs()
        .unwrap()
        .is_empty());
    let connection = rusqlite::Connection::open(&database_path).unwrap();
    for (automation_id, expected_notifications) in [
        (&conversation_task.id, 2_i64),
        (&project_task.id, 2_i64),
        (&message_task.id, 1_i64),
    ] {
        let projected = connection
            .query_row(
                "SELECT COUNT(*) FROM notification_events
                 WHERE source_kind = 'automation' AND automation_id = ?1",
                [automation_id],
                |row| row.get::<_, i64>(0),
            )
            .unwrap();
        assert_eq!(projected, expected_notifications, "{automation_id}");
    }
    model_server.await.unwrap();
}
