use super::*;
use crate::application::automation::permissions::resolve_automation_permissions;
use crate::application::automation::AutomationService;
use mycopilot_core::storage::automation_repository::{
    self, AutomationConfigRecord, AutomationCreateOutcome, AutomationRunAdmissionInput,
    AutomationRunAdmissionOutcome, AutomationRunEnqueueOutcome, NewAutomationRecord,
    NewManualAutomationRunRecord, StoredAutomationStatus,
};
use mycopilot_core::storage::models::{
    ChatConversationRecord, ChatMessageRecord, ModelConfigRecord, ModelSettingsRecord,
    ProjectRecord,
};
use mycopilot_core::{
    AgentForkTurns, AgentProposedAction, ConversationTraceSnapshot,
    ConversationTurnTraceTerminalStatus, CreateChildAgentInput, EnsureRootAgentInput,
};
use mycopilot_protocol_rs::{
    AutomationBlockedCodeDto, AutomationGetInputDto, AutomationHealthDto,
    AutomationPermissionModeDto, AutomationReasoningEffortDto, AutomationReasoningModeDto,
    AutomationReasoningProjectionDto, AutomationReasoningSourceDto, AutomationScheduleDto,
    AUTOMATION_PERMISSION_MODE_VERSION, AUTOMATION_SCHEMA_VERSION,
};
use rusqlite::TransactionBehavior;
use serde_json::{json, Value};
use tempfile::tempdir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const TEST_MODEL_ID: &str = "model-1";

fn model_settings() -> ModelSettingsRecord {
    ModelSettingsRecord {
        api_url: "https://example.test/v1/chat/completions".to_string(),
        api_token: "token".to_string(),
        search_mode: "disabled".to_string(),
        tavily_api_key: String::new(),
        models: vec![ModelConfigRecord {
            id: TEST_MODEL_ID.to_string(),
            display_name: "Model 1".to_string(),
            api_url_override: None,
            api_token_override: None,
            supports_image: false,
            context_window_tokens: Some(128_000),
            provider_profile_config: mycopilot_core::ProviderProfileConfig::generic_for_dialect(
                mycopilot_core::ProviderProtocolDialect::OpenAiChatCompletions,
            ),
            input_price: "0.01".to_string(),
            cached_input_price: String::new(),
            output_price: "0.02".to_string(),
            enabled: true,
        }],
    }
}

fn scheduler_state(
    storage: Arc<StorageService>,
    agent_service: AgentService,
) -> (
    Arc<AutomationSchedulerState>,
    tokio::sync::mpsc::UnboundedReceiver<Value>,
) {
    let (notifications, receiver) = tokio::sync::mpsc::unbounded_channel();
    (
        Arc::new(AutomationSchedulerState {
            storage,
            agent_service,
            notifications,
            wake: AutomationSchedulerWake::default(),
            startup_recovery_cutoff: now_ms(),
            automation_capacity: Arc::new(Semaphore::new(AUTOMATION_CONCURRENCY_LIMIT)),
            accepting: AtomicBool::new(true),
            observed_runs: Mutex::new(HashSet::new()),
            workers: Mutex::new(Vec::new()),
        }),
        receiver,
    )
}

fn create_task(
    storage: &StorageService,
    id: &str,
    status: StoredAutomationStatus,
    next_run_at: Option<i64>,
    permission_mode: AutomationPermissionModeDto,
    target_conversation_id: Option<&str>,
) -> mycopilot_core::storage::automation_repository::AutomationRecord {
    let permissions = resolve_automation_permissions(
        permission_mode,
        AUTOMATION_PERMISSION_MODE_VERSION,
        &storage.load_ui_preferences().unwrap(),
    )
    .unwrap()
    .projection;
    let anchor_at = next_run_at.unwrap_or_else(now_ms).max(0);
    let schedule = AutomationScheduleDto::Daily {
        time_minutes: 9 * 60,
        anchor_at,
        timezone: "UTC".to_string(),
    };
    let existing_chat = target_conversation_id.is_some();
    let permission_mode = match permission_mode {
        AutomationPermissionModeDto::Default => "default",
        AutomationPermissionModeDto::Full => "full",
        AutomationPermissionModeDto::Custom => "custom",
    };
    let reasoning = (!existing_chat).then(|| {
        serde_json::to_string(&AutomationReasoningProjectionDto {
            source: AutomationReasoningSourceDto::ModelConfig,
            mode: AutomationReasoningModeDto::ProviderDefault,
            effort: AutomationReasoningEffortDto::ProviderDefault,
        })
        .unwrap()
    });
    let outcome = storage
        .create_automation(&NewAutomationRecord {
            id: id.to_string(),
            create_request_id: format!("create-{id}"),
            status,
            config: AutomationConfigRecord {
                title: format!("Task {id}"),
                prompt: format!("Execute {id}."),
                health_state: "ok".to_string(),
                blocked_code: None,
                blocked_message: None,
                destination_kind: if existing_chat {
                    "existing_chat".to_string()
                } else {
                    "new_chat".to_string()
                },
                target_conversation_id: target_conversation_id.map(str::to_string),
                project_binding_kind: if existing_chat {
                    "inherit".to_string()
                } else {
                    "none".to_string()
                },
                project_id: None,
                model_id: (!existing_chat).then(|| TEST_MODEL_ID.to_string()),
                permission_mode: permission_mode.to_string(),
                permission_mode_version: i64::from(AUTOMATION_PERMISSION_MODE_VERSION),
                permissions_json: serde_json::to_string(&permissions).unwrap(),
                reasoning_json: reasoning,
                schedule_kind: "daily".to_string(),
                schedule_json: serde_json::to_string(&schedule).unwrap(),
                rrule: "FREQ=DAILY;BYHOUR=9;BYMINUTE=0;BYSECOND=0".to_string(),
                timezone: "UTC".to_string(),
                anchor_at,
                next_run_at,
                notification_policy: "all_runs".to_string(),
                target_project_snapshot: None,
                target_conversation_snapshot: target_conversation_id
                    .map(|_| "Existing conversation".to_string()),
                target_model_snapshot: (!existing_chat).then(|| "Model 1".to_string()),
                target_project_id_snapshot: None,
                target_conversation_id_snapshot: target_conversation_id.map(str::to_string),
                target_model_id_snapshot: (!existing_chat).then(|| TEST_MODEL_ID.to_string()),
            },
        })
        .unwrap();
    match outcome {
        AutomationCreateOutcome::Created(task) | AutomationCreateOutcome::Existing(task) => task,
    }
}

fn enqueue_manual(
    storage: &StorageService,
    task: &mycopilot_core::storage::automation_repository::AutomationRecord,
    suffix: &str,
) -> AutomationRunRecord {
    let outcome = storage
        .enqueue_manual_automation_run(&NewManualAutomationRunRecord {
            id: format!("automation-run-{suffix}"),
            automation_id: task.id.clone(),
            manual_request_id: format!("manual-request-{suffix}"),
            scheduled_for: now_ms(),
            config_revision: task.revision,
            config_snapshot_json: config_snapshot(task).unwrap(),
            expected_revision: task.revision,
        })
        .unwrap();
    match outcome {
        AutomationRunEnqueueOutcome::Enqueued(run) => run,
        outcome => panic!("unexpected manual enqueue outcome: {outcome:?}"),
    }
}

async fn wait_for_run(
    storage: &StorageService,
    automation_id: &str,
    predicate: impl Fn(&AutomationRunRecord) -> bool,
) -> AutomationRunRecord {
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            if let Some(run) = storage
                .get_latest_automation_run(automation_id)
                .unwrap()
                .filter(|run| predicate(run))
            {
                return run;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("automation run did not reach the expected durable state")
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

async fn write_provider_completion(stream: &mut TcpStream, content: &str) {
    stream
        .write_all(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
        )
        .await
        .unwrap();
    let delta = json!({
        "choices": [{ "delta": { "role": "assistant", "content": content }, "finish_reason": null }]
    });
    let finish = json!({
        "choices": [{ "delta": {}, "finish_reason": "stop" }]
    });
    stream
        .write_all(format!("data: {delta}\n\ndata: {finish}\n\ndata: [DONE]\n\n").as_bytes())
        .await
        .unwrap();
}

async fn write_provider_tool_call(
    stream: &mut TcpStream,
    call_id: &str,
    tool: &str,
    arguments: Value,
    narration: &str,
) {
    stream
        .write_all(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
        )
        .await
        .unwrap();
    let tool_frame = json!({
        "choices": [{
            "delta": {
                "role": "assistant",
                "content": narration,
                "tool_calls": [{
                    "index": 0,
                    "id": call_id,
                    "type": "function",
                    "function": {
                        "name": tool,
                        "arguments": serde_json::to_string(&arguments).unwrap()
                    }
                }]
            },
            "finish_reason": null
        }]
    });
    let finish_frame = json!({
        "choices": [{ "delta": {}, "finish_reason": "tool_calls" }]
    });
    stream
        .write_all(
            format!("data: {tool_frame}\n\ndata: {finish_frame}\n\ndata: [DONE]\n\n").as_bytes(),
        )
        .await
        .unwrap();
}

#[test]
fn safe_errors_are_bounded_and_do_not_expose_control_bytes() {
    assert_eq!(safe_error_code("bad code!"), "badcode");
    assert_eq!(safe_error_code("!!!"), "automation_failed");
    assert_eq!(safe_error_message("a\0b"), "a b");
    assert!(safe_error_message(&"界".repeat(2_000)).len() <= 4_096);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn overdue_task_is_consolidated_as_one_recovery_run_and_executes_human_root() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let provider = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let request = read_provider_request(&mut stream).await;
        write_provider_completion(&mut stream, "Scheduled work completed.").await;
        request
    });

    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let mut settings = model_settings();
    settings.api_url = format!("http://{address}/v1/chat/completions");
    storage.save_model_settings(settings).unwrap();
    let due_at = now_ms().saturating_sub(3 * 86_400_000);
    let task = create_task(
        &storage,
        "scheduler-overdue",
        StoredAutomationStatus::Active,
        Some(due_at),
        AutomationPermissionModeDto::Default,
        None,
    );
    let agent_service =
        AgentService::try_new_deferred_startup_reconciliation(Arc::clone(&storage)).unwrap();
    let (state, _notifications) = scheduler_state(Arc::clone(&storage), agent_service);

    assert!(!state.run_cycle().await.unwrap());
    let run = wait_for_run(&storage, &task.id, |run| run.status.is_terminal()).await;
    assert_eq!(run.trigger_kind, "recovery");
    assert_eq!(run.scheduled_for, due_at);
    assert_eq!(run.status, StoredAutomationRunStatus::Completed);
    assert!(run.agent_run_id.is_some());
    assert!(run.conversation_id.is_some());
    assert!(run.user_message_id.is_some());
    assert!(run.assistant_message_id.is_some());
    let current_task = storage.get_automation(&task.id).unwrap().unwrap();
    assert_eq!(current_task.last_scheduled_at, Some(due_at));
    assert!(current_task
        .config
        .next_run_at
        .is_some_and(|next| next > now_ms()));
    assert_eq!(
        storage
            .list_automation_runs(&task.id, None, 10)
            .unwrap()
            .items
            .len(),
        1,
        "several missed occurrences must collapse into one recovery run"
    );

    let request = provider.await.unwrap();
    assert!(request["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|message| {
            message["role"] == "user"
                && message["content"]
                    .as_str()
                    .is_some_and(|content| content.contains("scheduler-overdue"))
        }));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn admitted_preparation_failure_survives_read_and_trace_settlement_faults_without_duplication(
) {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    storage.save_model_settings(model_settings()).unwrap();
    let task = create_task(
        &storage,
        "scheduler-admitted-preparation-failure",
        StoredAutomationStatus::Paused,
        None,
        AutomationPermissionModeDto::Default,
        None,
    );
    let queued = enqueue_manual(&storage, &task, "admitted-preparation-failure");

    // The first fault fires only after conversation/messages/trace and the running Automation
    // binding commit together. Four local persistence attempts then fail, the scheduler's first
    // persistent settlement pass consumes four more, and a later pass recovers after the ninth.
    crate::application::agent::inject_automation_post_admission_preparation_failure(&queued.id);
    crate::application::agent::inject_automation_start_failure_settlement_failures(&queued.id, 9);
    // A transient failure while classifying Fatal as bound vs. unbound must not exit the claimed
    // worker. Multiple failures cover both the first authoritative read and its retry loop.
    inject_automation_fatal_run_inspection_failures(&queued.id, 3);

    let agent_service =
        AgentService::try_new_deferred_startup_reconciliation(Arc::clone(&storage)).unwrap();
    let (state, _notifications) = scheduler_state(Arc::clone(&storage), agent_service);
    state.run_cycle().await.unwrap();

    let failed = wait_for_run(&storage, &task.id, |run| run.status.is_terminal()).await;
    assert_eq!(failed.id, queued.id);
    assert_eq!(failed.status, StoredAutomationRunStatus::Failed);
    assert_eq!(failed.error_code.as_deref(), Some("agent_turn_failed"));
    assert_eq!(failed.admission_attempt, 1);
    assert!(failed
        .error_message
        .as_deref()
        .is_some_and(|message| message.contains("injected post-admission")));

    let agent_run_id = failed.agent_run_id.as_deref().unwrap();
    let conversation_id = failed.conversation_id.as_deref().unwrap();
    let user_message_id = failed.user_message_id.as_deref().unwrap();
    let assistant_message_id = failed.assistant_message_id.as_deref().unwrap();
    let conversation = storage.load_conversation(conversation_id).unwrap().unwrap();
    let users = conversation
        .messages
        .iter()
        .filter(|message| message.role == "user")
        .collect::<Vec<_>>();
    let assistants = conversation
        .messages
        .iter()
        .filter(|message| message.role == "assistant")
        .collect::<Vec<_>>();
    assert_eq!(
        users.len(),
        1,
        "admission recovery must not duplicate input"
    );
    assert_eq!(assistants.len(), 1, "one Turn has one assistant receipt");
    assert_eq!(users[0].id, user_message_id);
    assert_eq!(users[0].content, task.config.prompt);
    assert_eq!(assistants[0].id, assistant_message_id);

    let trace = storage
        .get_conversation_turn_trace(assistant_message_id)
        .unwrap()
        .unwrap();
    assert_eq!(trace.run_id, agent_run_id);
    assert_eq!(trace.conversation_id, conversation_id);
    assert_eq!(trace.assistant_message_id, assistant_message_id);
    assert_eq!(
        trace.terminal_status,
        ConversationTurnTraceTerminalStatus::Failed
    );
    assert!(trace.items.is_empty());
    assert_eq!(
        storage
            .list_automation_runs(&task.id, None, 10)
            .unwrap()
            .items
            .len(),
        1,
        "recovery must reuse the admitted run rather than enqueue another"
    );

    // Lost acknowledgements are safe: replaying the same durable start-failure settlement does
    // not rewrite the terminal trace or create another message pair.
    let terminal_error = trace.terminal_error.clone();
    state
        .agent_service
        .settle_automation_start_failure(&failed.id, "a replayed settlement must be a no-op")
        .unwrap();
    let replayed_trace = storage
        .get_conversation_turn_trace(assistant_message_id)
        .unwrap()
        .unwrap();
    assert_eq!(replayed_trace.terminal_error, terminal_error);
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if !state
                .observed_runs
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .contains(&failed.id)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("terminal start-failure worker must release its observer registration");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn paused_manual_run_is_claimed_then_durably_deferred_when_global_gate_is_full() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    storage.save_model_settings(model_settings()).unwrap();
    let task = create_task(
        &storage,
        "scheduler-paused-manual",
        StoredAutomationStatus::Paused,
        None,
        AutomationPermissionModeDto::Default,
        None,
    );
    let original = enqueue_manual(&storage, &task, "paused-manual");
    let agent_service = AgentService::try_new_deferred_startup_reconciliation_with_agent_limit(
        Arc::clone(&storage),
        None,
        1,
    )
    .unwrap();
    let global_gate = agent_service.turn_concurrency_gate();
    let global_permit = global_gate.try_acquire().unwrap();
    let (state, _notifications) = scheduler_state(Arc::clone(&storage), agent_service);

    assert!(!state.run_cycle().await.unwrap());
    let deferred = wait_for_run(&storage, &task.id, |run| {
        run.status == StoredAutomationRunStatus::Queued
            && run.retry_at.is_some()
            && run.admission_attempt == 1
    })
    .await;
    assert_eq!(deferred.id, original.id);
    assert_eq!(deferred.trigger_kind, "manual");
    assert_eq!(
        storage.get_automation(&task.id).unwrap().unwrap().status,
        StoredAutomationStatus::Paused,
        "run now may execute a paused task without resuming its schedule"
    );

    // The absolute retry is durable and claimable after capacity returns. Startup recovery also
    // makes an interrupted admission immediately queued, so neither state can become permanent.
    drop(global_permit);
    let retry_at = deferred.retry_at.unwrap();
    let reclaimed = storage
        .claim_ready_automation_runs(retry_at, AUTOMATION_ADMISSION_LEASE_MS, 1)
        .unwrap();
    assert_eq!(reclaimed.len(), 1);
    assert_eq!(reclaimed[0].id, original.id);
    assert_eq!(reclaimed[0].status, StoredAutomationRunStatus::Admitting);
    storage
        .recover_automation_admission_leases_on_startup(retry_at.saturating_add(1))
        .unwrap();
    assert_eq!(
        storage
            .get_automation_run(&original.id)
            .unwrap()
            .unwrap()
            .status,
        StoredAutomationRunStatus::Queued
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn busy_existing_chat_is_retryable_and_preserves_exact_message_history() {
    const CONVERSATION_ID: &str = "scheduler-busy-conversation";
    const ACTIVE_RUN_ID: &str = "scheduler-active-human-run";
    const ACTIVE_ASSISTANT_ID: &str = "scheduler-active-assistant";

    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    storage.save_model_settings(model_settings()).unwrap();
    storage
        .save_conversation(ChatConversationRecord {
            id: CONVERSATION_ID.to_string(),
            project_id: None,
            model_id: Some(TEST_MODEL_ID.to_string()),
            title: "Busy target".to_string(),
            messages: vec![
                ChatMessageRecord {
                    id: "scheduler-active-user".to_string(),
                    role: "user".to_string(),
                    content: "Already running".to_string(),
                    created_at: 1,
                    status: Some("sent".to_string()),
                    attachments: Vec::new(),
                    agent_run_json: None,
                    ui_state_json: None,
                },
                ChatMessageRecord {
                    id: ACTIVE_ASSISTANT_ID.to_string(),
                    role: "assistant".to_string(),
                    content: crate::application::agent::THINKING_PLACEHOLDER.to_string(),
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
            &ConversationTraceSnapshot::default().in_progress_trace(
                ACTIVE_RUN_ID,
                CONVERSATION_ID,
                ACTIVE_ASSISTANT_ID,
            ),
            2,
            2,
        )
        .unwrap();
    let task = create_task(
        &storage,
        "scheduler-busy-chat",
        StoredAutomationStatus::Paused,
        None,
        AutomationPermissionModeDto::Default,
        Some(CONVERSATION_ID),
    );
    let original = enqueue_manual(&storage, &task, "busy-chat");
    let agent_service =
        AgentService::try_new_deferred_startup_reconciliation(Arc::clone(&storage)).unwrap();
    let (state, _notifications) = scheduler_state(Arc::clone(&storage), agent_service);

    state.run_cycle().await.unwrap();
    let deferred = wait_for_run(&storage, &task.id, |run| {
        run.status == StoredAutomationRunStatus::Queued && run.retry_at.is_some()
    })
    .await;
    assert_eq!(deferred.id, original.id);
    assert_eq!(deferred.admission_attempt, 1);
    let conversation = storage.load_conversation(CONVERSATION_ID).unwrap().unwrap();
    assert_eq!(conversation.messages.len(), 2);
    assert_eq!(conversation.messages[0].id, "scheduler-active-user");
    assert_eq!(conversation.messages[1].id, ACTIVE_ASSISTANT_ID);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn child_chat_target_is_blocked_with_a_publicly_projectable_health_code() {
    const ROOT_CONVERSATION_ID: &str = "scheduler-root-conversation";

    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    storage.save_model_settings(model_settings()).unwrap();
    storage
        .save_conversation(ChatConversationRecord {
            id: ROOT_CONVERSATION_ID.to_string(),
            project_id: None,
            model_id: Some(TEST_MODEL_ID.to_string()),
            title: "Root".to_string(),
            messages: Vec::new(),
            created_at: 1,
            updated_at: 1,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
    storage
        .ensure_root_agent(&EnsureRootAgentInput {
            agent_id: "scheduler-root-agent".to_string(),
            conversation_id: ROOT_CONVERSATION_ID.to_string(),
            creation_request_id: "ensure-scheduler-root".to_string(),
            task_name: "root".to_string(),
        })
        .unwrap();
    let child = storage
        .create_child_agent(&CreateChildAgentInput {
            parent_agent_id: "scheduler-root-agent".to_string(),
            creation_request_id: "create-scheduler-child".to_string(),
            task_name: "child".to_string(),
            task: "Child task".to_string(),
            template_machine_key: None,
            explicit_model_id: Some(TEST_MODEL_ID.to_string()),
            reasoning_effort: None,
            fork_turns: AgentForkTurns::None,
        })
        .unwrap();
    let task = create_task(
        &storage,
        "scheduler-child-target",
        StoredAutomationStatus::Paused,
        None,
        AutomationPermissionModeDto::Default,
        Some(&child.agent.conversation_id),
    );
    enqueue_manual(&storage, &task, "child-target");
    let agent_service =
        AgentService::try_new_deferred_startup_reconciliation(Arc::clone(&storage)).unwrap();
    let (state, _notifications) = scheduler_state(Arc::clone(&storage), agent_service);

    state.run_cycle().await.unwrap();
    let failed = wait_for_run(&storage, &task.id, |run| run.status.is_terminal()).await;
    assert_eq!(failed.status, StoredAutomationRunStatus::Failed);
    assert_eq!(failed.error_code.as_deref(), Some("target_invalid"));
    assert_eq!(
        storage
            .get_automation(&task.id)
            .unwrap()
            .unwrap()
            .config
            .blocked_code
            .as_deref(),
        Some("target_invalid")
    );

    // This is the boundary that matters to Renderer: a blocked internal child/root distinction
    // must never leave a value that makes strict automation.get/list parsing fail.
    let projected = AutomationService::new(&storage)
        .get(AutomationGetInputDto {
            schema_version: AUTOMATION_SCHEMA_VERSION,
            automation_id: task.id,
        })
        .unwrap();
    assert!(matches!(
        projected.health,
        AutomationHealthDto::Blocked {
            code: AutomationBlockedCodeDto::TargetInvalid,
            ..
        }
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn revoked_full_permission_blocks_task_and_terminalizes_claim() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    storage.save_model_settings(model_settings()).unwrap();
    let task = create_task(
        &storage,
        "scheduler-permission-revoked",
        StoredAutomationStatus::Paused,
        None,
        AutomationPermissionModeDto::Full,
        None,
    );
    let run = enqueue_manual(&storage, &task, "permission-revoked");
    let mut preferences = storage.load_ui_preferences().unwrap();
    preferences.full_permission_enabled = false;
    storage.save_ui_preferences(preferences).unwrap();
    let agent_service =
        AgentService::try_new_deferred_startup_reconciliation(Arc::clone(&storage)).unwrap();
    let (state, _notifications) = scheduler_state(Arc::clone(&storage), agent_service);

    state.run_cycle().await.unwrap();
    let failed = wait_for_run(&storage, &task.id, |run| run.status.is_terminal()).await;
    assert_eq!(failed.id, run.id);
    assert_eq!(failed.status, StoredAutomationRunStatus::Failed);
    assert_eq!(failed.error_code.as_deref(), Some("permission_disabled"));
    let blocked = storage.get_automation(&task.id).unwrap().unwrap();
    assert_eq!(blocked.status, StoredAutomationStatus::Paused);
    assert_eq!(blocked.config.health_state, "blocked");
    assert_eq!(
        blocked.config.blocked_code.as_deref(),
        Some("permission_disabled")
    );
    assert!(blocked.config.next_run_at.is_none());
    assert!(blocked.attention_required_at.is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terminal_trace_is_compensated_without_restarting_or_duplicating_the_turn() {
    const CONVERSATION_ID: &str = "scheduler-recovery-conversation";
    const USER_ID: &str = "scheduler-recovery-user";
    const ASSISTANT_ID: &str = "scheduler-recovery-assistant";
    const AGENT_RUN_ID: &str = "scheduler-recovery-agent-run";

    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("storage.sqlite");
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    storage.save_model_settings(model_settings()).unwrap();
    // Construct before the synthetic terminal Trace so generic Agent startup reconciliation does
    // not participate; this test exercises only Automation's post-crash compensation pass.
    let agent_service =
        AgentService::try_new_deferred_startup_reconciliation(Arc::clone(&storage)).unwrap();
    let task = create_task(
        &storage,
        "scheduler-terminal-compensation",
        StoredAutomationStatus::Paused,
        None,
        AutomationPermissionModeDto::Default,
        None,
    );
    let queued = enqueue_manual(&storage, &task, "terminal-compensation");
    let claimed = storage
        .claim_ready_automation_runs(now_ms(), AUTOMATION_ADMISSION_LEASE_MS, 1)
        .unwrap()
        .remove(0);
    storage
        .save_conversation(ChatConversationRecord {
            id: CONVERSATION_ID.to_string(),
            project_id: None,
            model_id: Some(TEST_MODEL_ID.to_string()),
            title: "Recovered automation".to_string(),
            messages: vec![
                ChatMessageRecord {
                    id: USER_ID.to_string(),
                    role: "user".to_string(),
                    content: task.config.prompt.clone(),
                    created_at: 1,
                    status: Some("sent".to_string()),
                    attachments: Vec::new(),
                    agent_run_json: None,
                    ui_state_json: None,
                },
                ChatMessageRecord {
                    id: ASSISTANT_ID.to_string(),
                    role: "assistant".to_string(),
                    content: "Recovered result".to_string(),
                    created_at: 2,
                    status: Some("completed".to_string()),
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
            &ConversationTraceSnapshot::default().in_progress_trace(
                AGENT_RUN_ID,
                CONVERSATION_ID,
                ASSISTANT_ID,
            ),
            2,
            2,
        )
        .unwrap();
    let mut connection = rusqlite::Connection::open(&database_path).unwrap();
    {
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        assert!(matches!(
            automation_repository::admit_automation_run_in_transaction(
                &transaction,
                &AutomationRunAdmissionInput {
                    automation_run_id: claimed.id.clone(),
                    admission_token: claimed.admission_token.clone().unwrap(),
                    config_revision: task.revision,
                    permission_mode: "default".to_string(),
                    agent_run_id: AGENT_RUN_ID.to_string(),
                    conversation_id: CONVERSATION_ID.to_string(),
                    user_message_id: USER_ID.to_string(),
                    assistant_message_id: ASSISTANT_ID.to_string(),
                    admitted_at: now_ms(),
                },
            )
            .unwrap(),
            AutomationRunAdmissionOutcome::Admitted(_)
        ));
        transaction.commit().unwrap();
    }
    connection
        .execute(
            "UPDATE conversation_turn_traces
             SET terminal_status = 'completed', completed_at = ?1, updated_at = ?1
             WHERE run_id = ?2 AND assistant_message_id = ?3",
            rusqlite::params![now_ms(), AGENT_RUN_ID, ASSISTANT_ID],
        )
        .unwrap();
    let (state, _notifications) = scheduler_state(Arc::clone(&storage), agent_service);

    state.reconcile_bound_runs_once().await.unwrap();
    let settled = storage.get_automation_run(&queued.id).unwrap().unwrap();
    assert_eq!(settled.status, StoredAutomationRunStatus::Completed);
    assert_eq!(settled.agent_run_id.as_deref(), Some(AGENT_RUN_ID));
    assert_eq!(settled.conversation_id.as_deref(), Some(CONVERSATION_ID));
    assert_eq!(
        storage
            .load_conversation(CONVERSATION_ID)
            .unwrap()
            .unwrap()
            .messages
            .len(),
        2,
        "durable compensation must not recreate the HumanRoot message"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_bounded_startup_batch_uses_the_fixed_recovery_cutoff() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    storage.save_model_settings(model_settings()).unwrap();
    let overdue = now_ms().saturating_sub(86_400_000);
    let tasks = (0..AUTOMATION_SCHEDULER_BATCH_LIMIT + 1)
        .map(|index| {
            create_task(
                &storage,
                &format!("scheduler-recovery-batch-{index}"),
                StoredAutomationStatus::Active,
                Some(overdue),
                AutomationPermissionModeDto::Default,
                None,
            )
        })
        .collect::<Vec<_>>();
    let agent_service =
        AgentService::try_new_deferred_startup_reconciliation(Arc::clone(&storage)).unwrap();
    let (state, _notifications) = scheduler_state(Arc::clone(&storage), agent_service);
    let capacity = Arc::clone(&state.automation_capacity)
        .acquire_many_owned(AUTOMATION_CONCURRENCY_LIMIT as u32)
        .await
        .unwrap();

    // The due frontier is intentionally larger than one repository batch. Both calls represent
    // the same startup drain and must classify against one process-start instant, not a transient
    // "first cycle" boolean.
    state.run_cycle().await.unwrap();
    state.run_cycle().await.unwrap();

    for task in tasks {
        let run = storage
            .get_latest_automation_run(&task.id)
            .unwrap()
            .expect("every overdue task should be durably enqueued");
        assert_eq!(run.trigger_kind, "recovery", "task {}", task.id);
        assert_eq!(run.scheduled_for, overdue);
    }
    drop(capacity);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nonterminal_manual_run_excludes_same_task_due_occurrence_and_zero_capacity_does_not_spin()
{
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    storage.save_model_settings(model_settings()).unwrap();
    let due_at = now_ms().saturating_sub(60_000);
    let task = create_task(
        &storage,
        "scheduler-non-overlap",
        StoredAutomationStatus::Active,
        Some(due_at),
        AutomationPermissionModeDto::Default,
        None,
    );
    let manual = enqueue_manual(&storage, &task, "non-overlap");
    let agent_service =
        AgentService::try_new_deferred_startup_reconciliation(Arc::clone(&storage)).unwrap();
    let (state, _notifications) = scheduler_state(Arc::clone(&storage), agent_service);
    let capacity = Arc::clone(&state.automation_capacity)
        .acquire_many_owned(AUTOMATION_CONCURRENCY_LIMIT as u32)
        .await
        .unwrap();

    assert!(
        !state.run_cycle().await.unwrap(),
        "an unavailable Automation gate must not request an immediate self-wake"
    );
    let runs = storage
        .list_automation_runs(&task.id, None, 10)
        .unwrap()
        .items;
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].id, manual.id);
    assert_eq!(runs[0].status, StoredAutomationRunStatus::Queued);
    assert_eq!(runs[0].admission_attempt, 0);
    let current = storage.get_automation(&task.id).unwrap().unwrap();
    assert_eq!(current.config.next_run_at, Some(due_at));
    assert!(current.last_scheduled_at.is_none());
    drop(capacity);
}

#[derive(Clone, Copy, Debug)]
enum RestartApprovalDecision {
    Approve,
    Reject,
}

impl RestartApprovalDecision {
    fn label(self) -> &'static str {
        match self {
            Self::Approve => "approve",
            Self::Reject => "reject",
        }
    }
}

async fn run_waiting_approval_restart_scenario(decision: RestartApprovalDecision) {
    let label = decision.label();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let provider = tokio::spawn(async move {
        let (mut first_stream, _) = listener.accept().await.unwrap();
        let first_request = read_provider_request(&mut first_stream).await;
        write_provider_tool_call(
            &mut first_stream,
            "provider-automation-restart-command",
            "run_command",
            json!({
                "command": "printf 'automation approval restart\\n' > automation-approval.txt",
                "reason": "exercise durable Automation approval recovery"
            }),
            "I need approval before running this command.",
        )
        .await;
        drop(first_stream);

        let (mut continuation_stream, _) = listener.accept().await.unwrap();
        let continuation_request = read_provider_request(&mut continuation_stream).await;
        write_provider_completion(
            &mut continuation_stream,
            "The scheduled approval decision was handled.",
        )
        .await;
        (first_request, continuation_request)
    });

    let fixture = tempdir().unwrap();
    let database_path = fixture
        .path()
        .join(format!("approval-restart-{label}.sqlite"));
    let workspace = fixture.path().join(format!("workspace-{label}"));
    std::fs::create_dir_all(&workspace).unwrap();
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    let project_id = format!("automation-approval-project-{label}");
    storage
        .save_project(ProjectRecord {
            id: project_id.clone(),
            name: format!("Automation approval {label}"),
            path: Some(workspace.to_string_lossy().into_owned()),
            created_at: 1,
            pinned_at: None,
        })
        .unwrap();
    let mut settings = model_settings();
    settings.api_url = format!("http://{address}/v1/chat/completions");
    storage.save_model_settings(settings).unwrap();

    let original_task = create_task(
        &storage,
        &format!("scheduler-approval-restart-{label}"),
        StoredAutomationStatus::Paused,
        None,
        AutomationPermissionModeDto::Default,
        None,
    );
    let mut config = original_task.config.clone();
    config.project_binding_kind = "project".to_string();
    config.project_id = Some(project_id.clone());
    config.target_project_snapshot = Some(format!("Automation approval {label}"));
    config.target_project_id_snapshot = Some(project_id.clone());
    let task = match storage
        .replace_automation_config(&original_task.id, original_task.revision, &config)
        .unwrap()
    {
        AutomationCompareAndSetOutcome::Updated(task) => task,
        outcome => panic!("approval fixture task update failed: {outcome:?}"),
    };
    let queued = enqueue_manual(&storage, &task, &format!("approval-restart-{label}"));
    let claimed = storage
        .claim_ready_automation_runs(now_ms(), AUTOMATION_ADMISSION_LEASE_MS, 1)
        .unwrap()
        .into_iter()
        .next()
        .expect("manual Automation run should be claimed");
    assert_eq!(claimed.id, queued.id);

    let first_agent =
        AgentService::try_new_deferred_startup_reconciliation(Arc::clone(&storage)).unwrap();
    let (first_notifications, mut first_events) = tokio::sync::mpsc::unbounded_channel();
    let turn = first_agent
        .start_automation_human_root_turn(
            AutomationHumanRootTurnStart {
                context: AutomationExecutionContext {
                    automation_id: task.id.clone(),
                    automation_run_id: claimed.id.clone(),
                    scheduled_for: claimed.scheduled_for,
                    last_run_at: task.last_run_at,
                    trigger_kind: claimed.trigger_kind.clone(),
                },
                admission_token: claimed
                    .admission_token
                    .clone()
                    .expect("claimed run has an admission token"),
                config_revision: claimed.config_revision,
                title: task.config.title.clone(),
                prompt: task.config.prompt.clone(),
                destination: AutomationHumanRootDestination::NewChat {
                    project_id: Some(project_id),
                    model_id: TEST_MODEL_ID.to_string(),
                },
                permission_mode: task.config.permission_mode.clone(),
                permissions: permissions_from_projection(
                    serde_json::from_str(&task.config.permissions_json).unwrap(),
                ),
            },
            first_notifications,
        )
        .unwrap();

    tokio::time::timeout(Duration::from_secs(8), async {
        let mut saw_approval = false;
        let mut saw_waiting_done = false;
        while !(saw_approval && saw_waiting_done) {
            let event = first_events
                .recv()
                .await
                .expect("initial Automation event channel closed");
            assert_ne!(
                event["params"]["type"], "error",
                "Automation failed before approval: {event}"
            );
            match event["params"]["type"].as_str() {
                Some("approval_required") => saw_approval = true,
                Some("done") => saw_waiting_done = true,
                _ => {}
            }
        }
    })
    .await
    .expect("initial Automation turn did not reach durable approval");

    let durable_pending = first_agent.list_pending_actions();
    assert_eq!(durable_pending.len(), 1);
    let pending = durable_pending.into_iter().next().unwrap();
    assert_eq!(pending.run_id, turn.run_id);
    let AgentProposedAction::Command { .. } = &pending.action else {
        panic!("Automation approval fixture should persist a command action");
    };
    let action_id = pending.action_id.clone();
    let admitted = storage.get_automation_run(&queued.id).unwrap().unwrap();
    assert_eq!(admitted.status, StoredAutomationRunStatus::Running);
    assert_eq!(admitted.agent_run_id.as_deref(), Some(turn.run_id.as_str()));
    assert_eq!(
        admitted.user_message_id.as_deref(),
        Some(turn.user_message_id.as_str())
    );
    assert_eq!(
        admitted.assistant_message_id.as_deref(),
        Some(turn.assistant_message_id.as_str())
    );
    let trace = storage
        .get_conversation_turn_trace(&turn.assistant_message_id)
        .unwrap()
        .expect("admitted Automation turn has a durable trace");
    assert_eq!(trace.run_id, turn.run_id);
    assert_eq!(
        trace.terminal_status,
        ConversationTurnTraceTerminalStatus::InProgress
    );
    assert_eq!(
        storage
            .load_conversation(&turn.conversation_id)
            .unwrap()
            .unwrap()
            .messages
            .iter()
            .filter(|message| message.role == "user")
            .count(),
        1,
        "atomic admission creates exactly one HumanRoot user message"
    );

    // Simulate the process boundary only after the original Runtime has returned its durable
    // waiting state. No process-local Agent or Scheduler state is carried into the reconstruction.
    drop(first_events);
    drop(first_agent);
    drop(storage);

    let restarted_storage = Arc::new(StorageService::open(&database_path).unwrap());
    let restarted_agent =
        AgentService::try_new_deferred_startup_reconciliation(Arc::clone(&restarted_storage))
            .unwrap();
    let restarted_pending = restarted_agent.list_pending_actions();
    assert_eq!(restarted_pending.len(), 1);
    assert_eq!(restarted_pending[0].run_id, turn.run_id);
    assert_eq!(restarted_pending[0].action_id, action_id);

    let (restarted_state, _scheduler_notifications) =
        scheduler_state(Arc::clone(&restarted_storage), restarted_agent.clone());
    restarted_state.restore_bound_run_observers().await.unwrap();
    let waiting = wait_for_run(&restarted_storage, &task.id, |run| {
        run.status == StoredAutomationRunStatus::WaitingForApproval
    })
    .await;
    assert_eq!(waiting.id, queued.id);
    assert_eq!(waiting.agent_run_id.as_deref(), Some(turn.run_id.as_str()));

    let attention = restarted_storage
        .list_automation_attentions(None, 10)
        .unwrap();
    assert_eq!(attention.items.len(), 1);
    assert_eq!(attention.items[0].attention_kind, "waiting_for_approval");
    assert_eq!(
        attention.items[0].automation_run_id.as_deref(),
        Some(queued.id.as_str())
    );
    let approval_outbox_count = || {
        rusqlite::Connection::open(&database_path)
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM automation_notification_outbox
                 WHERE automation_run_id = ?1 AND notification_kind = 'approval_required'",
                [&queued.id],
                |row| row.get::<_, i64>(0),
            )
            .unwrap()
    };
    assert_eq!(approval_outbox_count(), 1);

    // A second startup/cycle restoration races with the already registered observer, but the
    // durable projection remains idempotent: one attention transition and one outbox row.
    restarted_state.restore_bound_run_observers().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(approval_outbox_count(), 1);
    let waiting_event_count = restarted_storage
        .list_automation_events_after(0, 1_000)
        .unwrap()
        .into_iter()
        .filter(|event| {
            event.event_kind == "run_updated"
                && serde_json::from_str::<Value>(&event.payload_json)
                    .ok()
                    .and_then(|payload| payload["status"].as_str().map(str::to_string))
                    .as_deref()
                    == Some("waiting_for_approval")
        })
        .count();
    assert_eq!(waiting_event_count, 1);

    let (decision_notifications, _decision_events) = tokio::sync::mpsc::unbounded_channel();
    let decision_output = match decision {
        RestartApprovalDecision::Approve => {
            restarted_agent.approve_action(&turn.run_id, &action_id, decision_notifications)
        }
        RestartApprovalDecision::Reject => restarted_agent.reject_action(
            &turn.run_id,
            &action_id,
            Some("Reject this command after restart.".to_string()),
            decision_notifications,
        ),
    }
    .unwrap();
    assert_eq!(
        decision_output.agent_output.status,
        mycopilot_core::AgentRunStatus::Running
    );

    let settled = wait_for_run(&restarted_storage, &task.id, |run| run.status.is_terminal()).await;
    assert_eq!(settled.id, queued.id);
    assert_eq!(settled.status, StoredAutomationRunStatus::Completed);
    assert_eq!(settled.agent_run_id.as_deref(), Some(turn.run_id.as_str()));
    let terminal_trace = restarted_storage
        .get_conversation_turn_trace(&turn.assistant_message_id)
        .unwrap()
        .expect("approval continuation leaves a durable terminal trace");
    assert_eq!(
        terminal_trace.terminal_status,
        ConversationTurnTraceTerminalStatus::Completed
    );
    assert!(restarted_agent.list_pending_actions().is_empty());

    let conversation = restarted_storage
        .load_conversation(&turn.conversation_id)
        .unwrap()
        .unwrap();
    assert_eq!(conversation.messages.len(), 2);
    assert_eq!(conversation.messages[0].id, turn.user_message_id);
    assert_eq!(conversation.messages[1].id, turn.assistant_message_id);
    assert_eq!(
        conversation
            .messages
            .iter()
            .filter(|message| message.role == "user")
            .count(),
        1,
        "restart recovery must resume the admitted turn, never create another user message"
    );
    assert!(restarted_storage
        .list_automation_attentions(None, 10)
        .unwrap()
        .items
        .is_empty());
    let approval_outbox_status: String = rusqlite::Connection::open(&database_path)
        .unwrap()
        .query_row(
            "SELECT status FROM automation_notification_outbox
             WHERE automation_run_id = ?1 AND notification_kind = 'approval_required'",
            [&queued.id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(approval_outbox_status, "suppressed");

    let (first_request, continuation_request) = provider.await.unwrap();
    let assert_execution_context = |request: &Value| {
        let matching = request["messages"]
            .as_array()
            .unwrap()
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
            "Automation system context must survive restart exactly once: {request}"
        );
        let context = matching[0]["content"].as_str().unwrap();
        assert!(context.contains(&format!(r#""automationId":"{}""#, task.id)));
        assert!(context.contains(&format!(r#""automationRunId":"{}""#, queued.id)));
        assert!(context.contains(&format!(r#""scheduledFor":{}"#, claimed.scheduled_for)));
        assert!(context.contains(r#""lastRunAt":null"#));
        assert!(context.contains(r#""triggerKind":"manual""#));
    };
    assert_execution_context(&first_request);
    assert_execution_context(&continuation_request);
    let prompt_messages = first_request["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| {
            message["role"] == "user"
                && message["content"]
                    .as_str()
                    .is_some_and(|content| content.contains(task.config.prompt.as_str()))
        })
        .collect::<Vec<_>>();
    assert_eq!(
        prompt_messages.len(),
        1,
        "the ordinary HumanRoot timing envelope must contain exactly one unmodified task prompt: {first_request}"
    );
    assert!(
        first_request["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|message| message["role"] == "user")
            .all(|message| {
                !message["content"]
                    .as_str()
                    .is_some_and(|content| content.contains("AUTOMATION_EXECUTION_CONTEXT_V1"))
            }),
        "Automation metadata leaked into a user-role provider message: {first_request}"
    );
    let tool_result = continuation_request["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["role"] == "tool")
        .and_then(|message| message["content"].as_str())
        .unwrap_or_else(|| {
            panic!("restarted continuation contains the paired Tool result: {continuation_request}")
        });
    let tool_result = serde_json::from_str::<Value>(tool_result).unwrap();
    match decision {
        RestartApprovalDecision::Approve => {
            assert!(workspace.join("automation-approval.txt").exists());
        }
        RestartApprovalDecision::Reject => {
            assert_eq!(tool_result["status"], "rejected");
            assert!(!workspace.join("automation-approval.txt").exists());
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn waiting_for_approval_survives_restart_and_approval_without_duplicate_turn() {
    run_waiting_approval_restart_scenario(RestartApprovalDecision::Approve).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn waiting_for_approval_survives_restart_and_rejection_without_duplicate_turn() {
    run_waiting_approval_restart_scenario(RestartApprovalDecision::Reject).await;
}
