use super::*;
use mycopilot_core::command::{CommandAuthorizationSource, CommandSessionManagerConfig};
use mycopilot_core::storage::agent_command_session_repository::{
    AgentCommandSessionCreate, AgentCommandSessionModelReadRequest,
    AGENT_COMMAND_SESSION_SCHEMA_VERSION,
};
use mycopilot_core::storage::conversation_history_archive_repository::ConversationHistoryArchivePageUnit;
use mycopilot_core::{
    AgentCommandPermission, AgentCommandSafetyPolicy, AgentCommandSessionAction,
    AgentCommandSessionExecutionOutput, AgentCommandSessionExecutionRequest,
    AgentCommandSessionExecutor, AgentCommandSessionGetInput, AgentCommandSessionSnapshot,
    ConversationCommandSessionLifecyclePhase, ConversationHistoryArchiveTraceMetadata,
    AGENT_COMMAND_SESSION_MODEL_OUTPUT_BYTES,
};
use rusqlite::Connection;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Barrier;
use std::thread;
use std::time::{Duration, Instant};

const INITIAL_YIELD: Duration = Duration::from_millis(10);
const TEST_WAIT: Duration = Duration::from_secs(5);
const ZERO_DIGEST: &str = "sha256:0000000000000000000000000000000000000000000000000000000000000000";

struct RunningFixture {
    registry: AgentCommandSessionRegistry,
    storage: Arc<StorageService>,
    database_path: std::path::PathBuf,
    workspace: tempfile::TempDir,
    conversation_id: String,
    assistant_message_id: String,
    run_id: String,
    call_id: String,
    poll_index: AtomicU64,
}

impl RunningFixture {
    fn new(name: &str) -> Self {
        Self::new_with_admission_limits(name, 32, 8)
    }

    fn new_with_admission_limits(
        name: &str,
        max_sessions: usize,
        max_sessions_per_conversation: usize,
    ) -> Self {
        Self::new_with_options(
            name,
            max_sessions,
            max_sessions_per_conversation,
            INITIAL_YIELD,
        )
    }

    fn new_with_initial_yield(name: &str, initial_yield: Duration) -> Self {
        Self::new_with_options(name, 32, 8, initial_yield)
    }

    fn new_with_options(
        name: &str,
        max_sessions: usize,
        max_sessions_per_conversation: usize,
        initial_yield: Duration,
    ) -> Self {
        let workspace = tempdir().unwrap();
        let database_path = workspace.path().join("storage.sqlite");
        let storage = Arc::new(StorageService::open(&database_path).unwrap());
        let conversation_id = format!("conversation-command-session-{name}");
        let assistant_message_id = format!("assistant-command-session-{name}");
        let run_id = format!("run-command-session-{name}");
        let call_id = format!("call-command-session-{name}");
        seed_conversation(&storage, &conversation_id, &assistant_message_id);
        let registry = AgentCommandSessionRegistry::with_manager_and_admission_limits(
            Arc::clone(&storage),
            CommandSessionManager::new(test_manager_config()).unwrap(),
            initial_yield,
            max_sessions,
            max_sessions_per_conversation,
        );
        Self {
            registry,
            storage,
            database_path,
            workspace,
            conversation_id,
            assistant_message_id,
            run_id,
            call_id,
            poll_index: AtomicU64::new(0),
        }
    }

    fn start(
        &self,
        command_text: &str,
        notifications: Option<CoreServerNotificationSender>,
    ) -> (AgentCommandSessionSnapshot, AgentToolResult) {
        let launch = start_owned_session(
            &self.registry,
            self.workspace.path(),
            &self.conversation_id,
            &self.assistant_message_id,
            &self.run_id,
            &self.call_id,
            command_text,
            notifications,
        )
        .unwrap();
        match launch {
            AgentCommandSessionLaunch::Running {
                snapshot,
                tool_result,
            } => (*snapshot, tool_result),
            AgentCommandSessionLaunch::Exited(terminal) => panic!(
                "expected a running command session, got {:?}",
                terminal.snapshot.state
            ),
        }
    }

    fn adopt(&self, snapshot: &AgentCommandSessionSnapshot, command: &str) {
        adopt_owned_session(
            &self.registry,
            &self.storage,
            &self.conversation_id,
            &self.assistant_message_id,
            &self.run_id,
            &self.call_id,
            snapshot,
            command,
        );
    }

    fn poll(&self, session_id: &str, wait: Duration) -> AgentCommandSessionExecutionOutput {
        let poll_index = self.poll_index.fetch_add(1, Ordering::Relaxed);
        self.registry
            .execute_command_session(AgentCommandSessionExecutionRequest {
                conversation_id: self.conversation_id.clone(),
                run_id: format!("{}-poll", self.run_id),
                call_id: format!("{}-poll-{poll_index}", self.call_id),
                session_id: session_id.to_string(),
                action: AgentCommandSessionAction::Poll,
                wait_ms: u64::try_from(wait.as_millis()).unwrap(),
                max_output_bytes: AGENT_COMMAND_SESSION_MODEL_OUTPUT_BYTES,
            })
            .unwrap()
    }
}

#[allow(clippy::too_many_arguments)]
fn start_owned_session(
    registry: &AgentCommandSessionRegistry,
    workspace: &Path,
    conversation_id: &str,
    assistant_message_id: &str,
    run_id: &str,
    call_id: &str,
    command_text: &str,
    notifications: Option<CoreServerNotificationSender>,
) -> Result<AgentCommandSessionLaunch, String> {
    let command = approved_command(call_id, command_text);
    registry.start(StartAgentCommandSession {
        owner: CommandSessionOwner {
            conversation_id: conversation_id.to_string(),
            assistant_message_id: assistant_message_id.to_string(),
            origin_run_id: run_id.to_string(),
            call_id: call_id.to_string(),
            project_id: None,
        },
        workspace_root: Some(workspace),
        command: &command,
        permissions: test_permissions(),
        authorization_source: CommandAuthorizationSource::ExplicitUser,
        approval_provenance: json!({
            "source": "explicit_user",
            "status": "approved"
        }),
        artifact_runtime: None,
        file_inputs: None,
        notifications,
        cancellation_token: AgentCancellationToken::new(),
        cancel_probe: None,
    })
}

#[allow(clippy::too_many_arguments)]
fn adopt_owned_session(
    registry: &AgentCommandSessionRegistry,
    storage: &StorageService,
    conversation_id: &str,
    assistant_message_id: &str,
    run_id: &str,
    call_id: &str,
    snapshot: &AgentCommandSessionSnapshot,
    command: &str,
) {
    let trace = completed_running_trace(
        conversation_id,
        assistant_message_id,
        run_id,
        call_id,
        &snapshot.session_id,
        command,
    );
    let tracker = Arc::new(FileEffectTracker::default());
    let mut guard = tracker.register(None, Some(conversation_id), run_id, call_id);
    guard.mark_effects_started();
    let mut guard = Some(guard);
    let outcome = registry
        .commit_handoff(
            &snapshot.session_id,
            &mut guard,
            || false,
            || storage.replace_conversation_turn_trace(&trace, 1, now_ms()),
        )
        .unwrap();
    assert_eq!(outcome, AgentCommandHandoffOutcome::Adopted);
    assert!(guard.is_none(), "the registry must own the adopted lease");
}

fn test_manager_config() -> CommandSessionManagerConfig {
    CommandSessionManagerConfig {
        max_active_sessions: 8,
        max_active_sessions_per_scope: 4,
        max_retained_terminal_sessions: 32,
        transcript_bytes: 16 * 1024,
        poll_bytes: 16 * 1024,
        drain_grace: Duration::from_millis(300),
        interrupt_grace: Duration::from_millis(100),
        shutdown_grace: Duration::from_secs(2),
    }
}

fn test_permissions() -> AgentPermissions {
    AgentPermissions {
        write: AgentWritePermission::WorkspaceOnly,
        command: AgentCommandPermission::AutoApprove,
        command_safety: AgentCommandSafetyPolicy::FullAccess,
        ..AgentPermissions::default()
    }
}

fn approved_command(call_id: &str, command: &str) -> AgentCommandRequest {
    AgentCommandRequest {
        id: call_id.to_string(),
        command: command.to_string(),
        cwd: None,
        timeout_ms: None,
        approval_status: AgentApprovalStatus::Approved,
        risk_level: Some(AgentCommandRiskLevel::ReadOnly),
        reason: Some("managed command session integration test".to_string()),
        observe: None,
        inputs: Vec::new(),
        runtime: None,
        runtime_binding: None,
    }
}

fn seed_conversation(storage: &StorageService, conversation_id: &str, assistant_message_id: &str) {
    storage
        .save_conversation(ChatConversationRecord {
            id: conversation_id.to_string(),
            project_id: None,
            model_id: Some("test-model".to_string()),
            title: "Managed command session test".to_string(),
            messages: vec![ChatMessageRecord {
                id: assistant_message_id.to_string(),
                role: "assistant".to_string(),
                content: String::new(),
                created_at: 1,
                status: Some("pending".to_string()),
                attachments: Vec::new(),
                agent_run_json: None,
                ui_state_json: None,
            }],
            created_at: 1,
            updated_at: 1,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
}

fn completed_running_trace(
    conversation_id: &str,
    assistant_message_id: &str,
    run_id: &str,
    call_id: &str,
    session_id: &str,
    command: &str,
) -> ConversationTurnTrace {
    ConversationTurnTrace {
        schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
        run_id: run_id.to_string(),
        conversation_id: conversation_id.to_string(),
        assistant_message_id: assistant_message_id.to_string(),
        terminal_status: ConversationTurnTraceTerminalStatus::Completed,
        terminal_error: None,
        truncated: false,
        items: vec![
            ConversationTurnTraceItem::ToolCall {
                sequence: 0,
                call_id: call_id.to_string(),
                tool: "run_command".to_string(),
                provenance: None,
                operation: json!({"command": command}),
                approval_status: AgentApprovalStatus::Approved,
                truncated: false,
            },
            ConversationTurnTraceItem::ToolResult {
                sequence: 1,
                call_id: call_id.to_string(),
                tool: "run_command".to_string(),
                status: ConversationTraceToolResultStatus::Succeeded,
                success: true,
                observation: json!({
                    "status": "running",
                    "sessionId": session_id
                }),
                approval_status: AgentApprovalStatus::Approved,
                error: None,
                truncated: false,
                archive: ConversationHistoryArchiveTraceMetadata::default(),
            },
        ],
    }
}

fn wait_for_terminal_record(
    storage: &StorageService,
    conversation_id: &str,
    session_id: &str,
) -> mycopilot_core::storage::agent_command_session_repository::AgentCommandSessionRecord {
    let deadline = Instant::now() + TEST_WAIT;
    loop {
        let record = storage
            .load_agent_command_session(conversation_id, session_id)
            .unwrap()
            .expect("persisted command session");
        if record.snapshot.status.is_terminal() && record.settled_at.is_some() {
            return record;
        }
        assert!(
            Instant::now() < deadline,
            "command session did not settle before the test deadline: {:?}",
            record.snapshot.status
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn poll_until(
    fixture: &RunningFixture,
    session_id: &str,
    predicate: impl Fn(&AgentCommandSessionExecutionOutput) -> bool,
) -> AgentCommandSessionExecutionOutput {
    let deadline = Instant::now() + TEST_WAIT;
    loop {
        let output = fixture.poll(session_id, Duration::from_millis(200));
        if predicate(&output) {
            return output;
        }
        assert!(
            Instant::now() < deadline,
            "command session poll did not satisfy the predicate: status={:?}, sequence={}",
            output.status,
            output.latest_sequence
        );
    }
}

fn install_terminal_rejection(database: &Connection, trigger_name: &str, session_id: &str) {
    assert!(trigger_name
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_'));
    assert!(session_id
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_'));
    database
        .execute_batch(&format!(
            "CREATE TRIGGER {trigger_name}
             BEFORE UPDATE OF status ON agent_command_sessions
             WHEN OLD.session_id = '{session_id}'
              AND OLD.status IN ('starting', 'running')
              AND NEW.status NOT IN ('starting', 'running')
             BEGIN
               SELECT RAISE(ABORT, 'injected permanent terminal settlement failure');
             END;"
        ))
        .unwrap();
}

fn wait_for_settlement_attempts(registry: &AgentCommandSessionRegistry, minimum_attempts: usize) {
    let deadline = Instant::now() + TEST_WAIT;
    loop {
        if registry.settlement_scheduler_stats().2 >= minimum_attempts {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "settlement scheduler did not reach {minimum_attempts} attempts"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn silent_long_command_returns_running_and_emits_started() {
    let fixture = RunningFixture::new("silent-started");
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let (snapshot, tool_result) = fixture.start("sleep 5", Some(notifications));

    assert_eq!(snapshot.status, AgentCommandSessionStatus::Running);
    assert_eq!(
        tool_result.result.as_ref().unwrap()["status"],
        Value::String("running".to_string())
    );
    assert_eq!(
        tool_result.result.as_ref().unwrap()["sessionId"],
        snapshot.session_id
    );

    let started = receiver.try_recv().expect("command_started notification");
    assert_eq!(started["params"]["type"], "command_started");
    assert_eq!(started["params"]["sessionId"], snapshot.session_id);
    assert_eq!(started["params"]["callId"], fixture.call_id);
    assert!(
        receiver.try_recv().is_err(),
        "a silent process must not need output to publish command_started"
    );

    let terminal = fixture
        .registry
        .abort_before_handoff(&snapshot.session_id)
        .unwrap();
    assert!(matches!(
        terminal.snapshot.state,
        mycopilot_core::command::CommandSessionState::Interrupted
    ));
}

#[test]
fn initial_running_output_cut_is_durable_and_replayable_before_handoff() {
    let fixture = RunningFixture::new_with_initial_yield(
        "initial-running-receipt",
        Duration::from_millis(250),
    );
    let command = "printf initial-receipt-marker; sleep 0.50; printf later-marker; sleep 5";
    let (snapshot, tool_result) = fixture.start(command, None);
    let result = tool_result.result.as_ref().expect("running ToolResult");
    assert!(result["output"]
        .as_str()
        .expect("running output")
        .contains("initial-receipt-marker"));
    assert!(!result["output"]
        .as_str()
        .expect("running output")
        .contains("later-marker"));

    let request = AgentCommandSessionModelReadRequest {
        conversation_id: &fixture.conversation_id,
        session_id: &snapshot.session_id,
        run_id: &fixture.run_id,
        call_id: &fixture.call_id,
        action: AgentCommandSessionAction::Poll,
        max_output_bytes: AGENT_COMMAND_SESSION_MODEL_OUTPUT_BYTES,
        host_output_truncated: false,
        created_at: now_ms(),
    };
    let initial_read = fixture
        .storage
        .load_agent_command_session_model_read(&request)
        .unwrap()
        .expect("initial run_command model-read receipt");
    let initial_output = initial_read
        .chunks
        .iter()
        .map(|chunk| chunk.output.as_str())
        .collect::<String>();
    assert_eq!(initial_output, result["output"]);
    assert_eq!(
        initial_read.receipt.latest_sequence,
        result["latestSequence"].as_u64().unwrap()
    );
    assert_eq!(
        initial_read.receipt.output_truncated,
        result["outputTruncated"].as_bool().unwrap()
    );

    let deadline = Instant::now() + TEST_WAIT;
    loop {
        let host_view = fixture
            .registry
            .get(AgentCommandSessionGetInput {
                conversation_id: fixture.conversation_id.clone(),
                session_id: snapshot.session_id.clone(),
                after_sequence: initial_read.receipt.last_output_sequence,
                max_bytes: None,
            })
            .unwrap();
        if host_view
            .transcript
            .chunks
            .iter()
            .any(|chunk| chunk.output.contains("later-marker"))
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "later output was not durably visible"
        );
        thread::sleep(Duration::from_millis(10));
    }

    // Reopening storage simulates the crash boundary before `commit_handoff`. The original
    // run/call identity must replay the immutable cut, not expand it to include later output.
    let reopened = StorageService::open(&fixture.database_path).unwrap();
    let replay = reopened
        .read_or_create_agent_command_session_model_read(&request)
        .unwrap()
        .expect("replayed initial running receipt");
    assert_eq!(replay, initial_read);

    fixture
        .registry
        .abort_before_handoff(&snapshot.session_id)
        .unwrap();
}

#[test]
fn model_poll_is_incremental_until_terminal() {
    let fixture = RunningFixture::new("incremental-poll");
    let command = "sleep 0.10; printf first; sleep 0.40; printf second";
    let (snapshot, initial_result) = fixture.start(command, None);
    assert_eq!(initial_result.result.as_ref().unwrap()["output"], "");
    fixture.adopt(&snapshot, command);

    let first = poll_until(&fixture, &snapshot.session_id, |output| {
        output.output.contains("first")
    });
    assert!(!first.output.contains("second"));

    let second = poll_until(&fixture, &snapshot.session_id, |output| {
        output.output.contains("second")
    });
    assert!(
        !second.output.contains("first"),
        "model polling must advance one independent durable cursor"
    );

    let terminal = poll_until(&fixture, &snapshot.session_id, |output| {
        output.status.is_terminal()
    });
    assert_eq!(terminal.status, AgentCommandSessionStatus::Exited);
    assert_eq!(terminal.exit_code, Some(0));
}

#[test]
fn concurrent_model_polls_share_one_serialized_durable_cursor() {
    let fixture = RunningFixture::new("concurrent-model-poll");
    let command = "sleep 0.08; printf serialized-marker; sleep 0.40";
    let (snapshot, _) = fixture.start(command, None);
    fixture.adopt(&snapshot, command);

    let deadline = Instant::now() + TEST_WAIT;
    loop {
        let current = fixture
            .registry
            .get(AgentCommandSessionGetInput {
                conversation_id: fixture.conversation_id.clone(),
                session_id: snapshot.session_id.clone(),
                after_sequence: Some(0),
                max_bytes: None,
            })
            .unwrap();
        if current
            .transcript
            .chunks
            .iter()
            .any(|chunk| chunk.output.contains("serialized-marker"))
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "command output was not persisted"
        );
        thread::sleep(Duration::from_millis(10));
    }

    let barrier = Arc::new(Barrier::new(3));
    let workers = (0..2)
        .map(|index| {
            let registry = fixture.registry.clone();
            let barrier = Arc::clone(&barrier);
            let conversation_id = fixture.conversation_id.clone();
            let session_id = snapshot.session_id.clone();
            thread::spawn(move || {
                barrier.wait();
                registry
                    .execute_command_session(AgentCommandSessionExecutionRequest {
                        conversation_id,
                        run_id: format!("run-concurrent-poll-{index}"),
                        call_id: format!("call-concurrent-poll-{index}"),
                        session_id,
                        action: AgentCommandSessionAction::Poll,
                        wait_ms: 0,
                        max_output_bytes: AGENT_COMMAND_SESSION_MODEL_OUTPUT_BYTES,
                    })
                    .unwrap()
            })
        })
        .collect::<Vec<_>>();
    barrier.wait();
    let outputs = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .collect::<Vec<_>>();

    assert_eq!(
        outputs
            .iter()
            .map(|output| output.output.matches("serialized-marker").count())
            .sum::<usize>(),
        1,
        "concurrent polls must not return the same durable output twice"
    );
    wait_for_terminal_record(
        &fixture.storage,
        &fixture.conversation_id,
        &snapshot.session_id,
    );
}

#[test]
fn concurrent_terminal_polls_share_one_serialized_durable_cursor() {
    let fixture = RunningFixture::new("concurrent-terminal-poll");
    let command = "sleep 0.08; printf terminal-serialized-marker";
    let (snapshot, initial_result) = fixture.start(command, None);
    assert_eq!(initial_result.result.as_ref().unwrap()["output"], "");
    fixture.adopt(&snapshot, command);
    let terminal = wait_for_terminal_record(
        &fixture.storage,
        &fixture.conversation_id,
        &snapshot.session_id,
    );
    assert_eq!(terminal.snapshot.status, AgentCommandSessionStatus::Exited);

    // A fresh registry has no live HostCommandSession for this already-terminal row. Cursor
    // serialization must therefore belong to the durable Session identity, not the OS-process
    // wrapper which was removed at terminal settlement.
    let registry = AgentCommandSessionRegistry::with_manager(
        Arc::clone(&fixture.storage),
        CommandSessionManager::new(test_manager_config()).unwrap(),
        INITIAL_YIELD,
    );
    let barrier = Arc::new(Barrier::new(3));
    let workers = (0..2)
        .map(|index| {
            let registry = registry.clone();
            let barrier = Arc::clone(&barrier);
            let conversation_id = fixture.conversation_id.clone();
            let session_id = snapshot.session_id.clone();
            thread::spawn(move || {
                barrier.wait();
                registry
                    .execute_command_session(AgentCommandSessionExecutionRequest {
                        conversation_id,
                        run_id: format!("run-concurrent-terminal-poll-{index}"),
                        call_id: format!("call-concurrent-terminal-poll-{index}"),
                        session_id,
                        action: AgentCommandSessionAction::Poll,
                        wait_ms: 0,
                        max_output_bytes: AGENT_COMMAND_SESSION_MODEL_OUTPUT_BYTES,
                    })
                    .expect("concurrent terminal poll must not expose a cursor CAS conflict")
            })
        })
        .collect::<Vec<_>>();
    barrier.wait();
    let outputs = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .collect::<Vec<_>>();

    assert!(outputs.iter().all(|output| output.status.is_terminal()));
    assert_eq!(
        outputs
            .iter()
            .map(|output| { output.output.matches("terminal-serialized-marker").count() })
            .sum::<usize>(),
        1,
        "terminal output must be returned exactly once across concurrent model polls"
    );
}

#[test]
fn retried_tool_call_replays_its_committed_cut_after_registry_restart() {
    let fixture = RunningFixture::new("poll-receipt-restart");
    let command = "sleep 0.08; printf crash-safe-marker";
    let (snapshot, initial_result) = fixture.start(command, None);
    assert_eq!(initial_result.result.as_ref().unwrap()["output"], "");
    fixture.adopt(&snapshot, command);
    wait_for_terminal_record(
        &fixture.storage,
        &fixture.conversation_id,
        &snapshot.session_id,
    );

    let request = AgentCommandSessionExecutionRequest {
        conversation_id: fixture.conversation_id.clone(),
        run_id: "run-crash-retry".to_string(),
        call_id: "call-crash-retry".to_string(),
        session_id: snapshot.session_id.clone(),
        action: AgentCommandSessionAction::Poll,
        wait_ms: 0,
        max_output_bytes: AGENT_COMMAND_SESSION_MODEL_OUTPUT_BYTES,
    };
    let before_restart = fixture
        .registry
        .execute_command_session(request.clone())
        .unwrap();
    assert_eq!(before_restart.output, "crash-safe-marker");

    let restarted_registry = AgentCommandSessionRegistry::with_manager(
        Arc::clone(&fixture.storage),
        CommandSessionManager::new(test_manager_config()).unwrap(),
        INITIAL_YIELD,
    );
    let after_restart = restarted_registry.execute_command_session(request).unwrap();
    assert_eq!(after_restart, before_restart);

    let next_call = restarted_registry
        .execute_command_session(AgentCommandSessionExecutionRequest {
            conversation_id: fixture.conversation_id.clone(),
            run_id: "run-crash-retry".to_string(),
            call_id: "call-after-crash-retry".to_string(),
            session_id: snapshot.session_id,
            action: AgentCommandSessionAction::Poll,
            wait_ms: 0,
            max_output_bytes: AGENT_COMMAND_SESSION_MODEL_OUTPUT_BYTES,
        })
        .unwrap();
    assert!(next_call.output.is_empty());
}

#[test]
fn cross_conversation_session_access_is_rejected() {
    let fixture = RunningFixture::new("conversation-isolation");
    let other_conversation = "conversation-command-session-other";
    seed_conversation(
        &fixture.storage,
        other_conversation,
        "assistant-command-session-other",
    );
    let (snapshot, _) = fixture.start("sleep 5", None);

    let get = fixture.registry.get(AgentCommandSessionGetInput {
        conversation_id: other_conversation.to_string(),
        session_id: snapshot.session_id.clone(),
        after_sequence: None,
        max_bytes: None,
    });
    assert!(get.is_err());

    let model_access =
        fixture
            .registry
            .execute_command_session(AgentCommandSessionExecutionRequest {
                conversation_id: other_conversation.to_string(),
                run_id: "run-other".to_string(),
                call_id: "call-other".to_string(),
                session_id: snapshot.session_id.clone(),
                action: AgentCommandSessionAction::Poll,
                wait_ms: 0,
                max_output_bytes: AGENT_COMMAND_SESSION_MODEL_OUTPUT_BYTES,
            });
    assert_eq!(
        model_access.unwrap_err().code(),
        Some("agent.command_session_not_found")
    );

    fixture
        .registry
        .abort_before_handoff(&snapshot.session_id)
        .unwrap();
}

#[test]
fn adopted_session_ignores_origin_run_pre_handoff_cancellation() {
    let fixture = RunningFixture::new("adopted-cancel");
    let command = "sleep 0.50";
    let (snapshot, _) = fixture.start(command, None);
    fixture.adopt(&snapshot, command);

    assert_eq!(
        fixture.registry.cancel_pre_handoff_for_run(&fixture.run_id),
        0
    );
    let current = fixture
        .registry
        .get(AgentCommandSessionGetInput {
            conversation_id: fixture.conversation_id.clone(),
            session_id: snapshot.session_id.clone(),
            after_sequence: None,
            max_bytes: None,
        })
        .unwrap();
    assert_eq!(current.session.status, AgentCommandSessionStatus::Running);

    let terminal = wait_for_terminal_record(
        &fixture.storage,
        &fixture.conversation_id,
        &snapshot.session_id,
    );
    assert_eq!(terminal.snapshot.status, AgentCommandSessionStatus::Exited);
    assert_eq!(terminal.snapshot.exit_code, Some(0));
}

#[test]
fn pre_handoff_cancellation_terminates_the_process() {
    let fixture = RunningFixture::new("pending-cancel");
    let (snapshot, _) = fixture.start("sleep 5", None);

    assert_eq!(
        fixture.registry.cancel_pre_handoff_for_run(&fixture.run_id),
        1
    );
    let terminal = fixture
        .registry
        .abort_before_handoff(&snapshot.session_id)
        .unwrap();
    assert_eq!(
        terminal.snapshot.state,
        mycopilot_core::command::CommandSessionState::Interrupted
    );
    let record = wait_for_terminal_record(
        &fixture.storage,
        &fixture.conversation_id,
        &snapshot.session_id,
    );
    assert_eq!(
        record.snapshot.status,
        AgentCommandSessionStatus::Interrupted
    );
}

#[test]
fn handed_off_terminal_output_is_archived_exactly_once() {
    let fixture = RunningFixture::new("terminal-archive");
    let command = "sleep 0.08; printf archive-marker; sleep 0.08";
    let (snapshot, _) = fixture.start(command, None);
    fixture.adopt(&snapshot, command);

    let record = wait_for_terminal_record(
        &fixture.storage,
        &fixture.conversation_id,
        &snapshot.session_id,
    );
    assert_eq!(record.snapshot.status, AgentCommandSessionStatus::Exited);
    assert_eq!(record.snapshot.exit_code, Some(0));
    let archive_ref = record
        .snapshot
        .archive_ref
        .as_deref()
        .expect("terminal archive ref");
    let descriptor = fixture
        .storage
        .find_conversation_history_archive_by_ref(&fixture.conversation_id, archive_ref)
        .unwrap()
        .expect("terminal archive descriptor");
    let page = fixture
        .storage
        .read_conversation_history_archive_page(
            &fixture.conversation_id,
            archive_ref,
            ConversationHistoryArchivePageUnit::Char,
            0,
            descriptor.total_chars,
        )
        .unwrap()
        .expect("terminal archive page");
    assert!(page.content.contains("archive-marker"));

    let connection = Connection::open(&fixture.database_path).unwrap();
    let archive_count: u64 = connection
        .query_row(
            "SELECT COUNT(*) FROM conversation_history_blobs
             WHERE conversation_id = ?1 AND call_id = ?2",
            rusqlite::params![
                &fixture.conversation_id,
                format!("command-session:{}", snapshot.session_id)
            ],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(archive_count, 1);

    let trace = fixture
        .storage
        .get_conversation_turn_trace(&fixture.assistant_message_id)
        .unwrap()
        .expect("command session trace");
    let terminal_events = trace
        .items
        .iter()
        .filter(|item| {
            matches!(
                item,
                ConversationTurnTraceItem::CommandSessionLifecycle {
                    phase: ConversationCommandSessionLifecyclePhase::Terminal,
                    session_id,
                    ..
                } if session_id == &snapshot.session_id
            )
        })
        .count();
    assert_eq!(terminal_events, 1);
}

#[test]
fn transient_terminal_cas_failure_retries_without_duplicate_archive_or_lifecycle() {
    let fixture = RunningFixture::new("terminal-retry");
    let command = "sleep 0.20; printf retry-archive-marker";
    let (snapshot, _) = fixture.start(command, None);
    let database = Connection::open(&fixture.database_path).unwrap();
    database.busy_timeout(Duration::from_secs(2)).unwrap();
    database
        .execute_batch(
            "CREATE TRIGGER reject_command_session_terminal_once_enabled
             BEFORE UPDATE OF status ON agent_command_sessions
             WHEN OLD.status IN ('starting', 'running')
              AND NEW.status NOT IN ('starting', 'running')
             BEGIN
               SELECT RAISE(ABORT, 'injected transient terminal CAS failure');
             END;",
        )
        .unwrap();
    fixture.adopt(&snapshot, command);

    let archive_call_id = format!("command-session:{}", snapshot.session_id);
    let retry_deadline = Instant::now() + TEST_WAIT;
    loop {
        let archive_count: u64 = database
            .query_row(
                "SELECT COUNT(*) FROM conversation_history_blobs
                 WHERE conversation_id = ?1 AND call_id = ?2",
                rusqlite::params![&fixture.conversation_id, &archive_call_id],
                |row| row.get(0),
            )
            .unwrap();
        if archive_count == 1 {
            break;
        }
        assert!(
            Instant::now() < retry_deadline,
            "the first settlement attempt never reached Exact Archive"
        );
        thread::sleep(Duration::from_millis(10));
    }

    // Keep the trigger installed across several retry windows. The immutable Archive has already
    // committed, while the operational row and terminal lifecycle must remain uncommitted.
    thread::sleep(Duration::from_millis(150));
    let blocked = fixture
        .storage
        .load_agent_command_session(&fixture.conversation_id, &snapshot.session_id)
        .unwrap()
        .expect("blocked command session row");
    assert_eq!(blocked.snapshot.status, AgentCommandSessionStatus::Running);
    assert!(blocked.snapshot.archive_ref.is_none());
    let blocked_trace = fixture
        .storage
        .get_conversation_turn_trace(&fixture.assistant_message_id)
        .unwrap()
        .expect("blocked command trace");
    assert_eq!(
        blocked_trace
            .items
            .iter()
            .filter(|item| matches!(
                item,
                ConversationTurnTraceItem::CommandSessionLifecycle {
                    phase: ConversationCommandSessionLifecyclePhase::Terminal,
                    session_id,
                    ..
                } if session_id == &snapshot.session_id
            ))
            .count(),
        0
    );

    database
        .execute_batch("DROP TRIGGER reject_command_session_terminal_once_enabled;")
        .unwrap();
    let settled = wait_for_terminal_record(
        &fixture.storage,
        &fixture.conversation_id,
        &snapshot.session_id,
    );
    assert_eq!(settled.snapshot.status, AgentCommandSessionStatus::Exited);
    assert_eq!(settled.snapshot.exit_code, Some(0));
    assert!(settled.snapshot.archive_ref.is_some());

    let archive_count: u64 = database
        .query_row(
            "SELECT COUNT(*) FROM conversation_history_blobs
             WHERE conversation_id = ?1 AND call_id = ?2",
            rusqlite::params![&fixture.conversation_id, &archive_call_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(archive_count, 1);
    let settled_trace = fixture
        .storage
        .get_conversation_turn_trace(&fixture.assistant_message_id)
        .unwrap()
        .expect("settled command trace");
    let terminal_events = settled_trace
        .items
        .iter()
        .filter(|item| {
            matches!(
                item,
                ConversationTurnTraceItem::CommandSessionLifecycle {
                    phase: ConversationCommandSessionLifecyclePhase::Terminal,
                    session_id,
                    ..
                } if session_id == &snapshot.session_id
            )
        })
        .count();
    assert_eq!(terminal_events, 1);
}

#[test]
fn permanently_unsettled_session_keeps_host_admission_without_spawning_retry_threads() {
    let fixture = RunningFixture::new_with_admission_limits("settlement-admission", 2, 1);
    let command = "sleep 0.20; printf permanently-blocked";
    let (snapshot, _) = fixture.start(command, None);
    let database = Connection::open(&fixture.database_path).unwrap();
    install_terminal_rejection(
        &database,
        "reject_permanent_settlement",
        &snapshot.session_id,
    );
    fixture.adopt(&snapshot, command);
    wait_for_settlement_attempts(&fixture.registry, 3);

    let blocked = fixture
        .storage
        .load_agent_command_session(&fixture.conversation_id, &snapshot.session_id)
        .unwrap()
        .expect("unsettled command Session");
    assert_eq!(blocked.snapshot.status, AgentCommandSessionStatus::Running);
    assert_eq!(fixture.registry.retained_admission_count(), 1);
    let stats = fixture.registry.settlement_scheduler_stats();
    assert_eq!(stats.0, 1, "retries must reuse the one scheduler worker");
    assert_eq!(stats.1, 1, "one failed Session must occupy one retry slot");

    let rejected = start_owned_session(
        &fixture.registry,
        fixture.workspace.path(),
        &fixture.conversation_id,
        &fixture.assistant_message_id,
        "run-over-conversation-limit",
        "call-over-conversation-limit",
        "sleep 5",
        None,
    )
    .unwrap_err();
    assert!(rejected.contains("当前会话"));
    assert_eq!(fixture.registry.retained_admission_count(), 1);

    let other_conversation = "conversation-command-session-admission-other";
    let other_assistant = "assistant-command-session-admission-other";
    seed_conversation(&fixture.storage, other_conversation, other_assistant);
    let other = start_owned_session(
        &fixture.registry,
        fixture.workspace.path(),
        other_conversation,
        other_assistant,
        "run-admission-other",
        "call-admission-other",
        "sleep 5",
        None,
    )
    .unwrap();
    let AgentCommandSessionLaunch::Running {
        snapshot: other_snapshot,
        ..
    } = other
    else {
        panic!("the second admitted command must still be running");
    };
    assert_eq!(fixture.registry.retained_admission_count(), 2);
    let globally_rejected = start_owned_session(
        &fixture.registry,
        fixture.workspace.path(),
        "conversation-not-created-because-global-limit-wins",
        "assistant-not-created-because-global-limit-wins",
        "run-over-global-limit",
        "call-over-global-limit",
        "sleep 5",
        None,
    )
    .unwrap_err();
    assert!(globally_rejected.contains("全局"));
    assert_eq!(fixture.registry.retained_admission_count(), 2);

    fixture
        .registry
        .abort_before_handoff(&other_snapshot.session_id)
        .unwrap();
    assert_eq!(fixture.registry.retained_admission_count(), 1);
    database
        .execute_batch("DROP TRIGGER reject_permanent_settlement;")
        .unwrap();
    wait_for_terminal_record(
        &fixture.storage,
        &fixture.conversation_id,
        &snapshot.session_id,
    );
    assert_eq!(fixture.registry.retained_admission_count(), 0);
}

#[test]
fn delayed_failed_settlement_does_not_starve_a_healthy_session() {
    let fixture = RunningFixture::new_with_admission_limits("settlement-fairness", 2, 2);
    let healthy_conversation = "conversation-command-session-settlement-healthy";
    let healthy_assistant = "assistant-command-session-settlement-healthy";
    seed_conversation(&fixture.storage, healthy_conversation, healthy_assistant);

    let bad_command = "sleep 0.20; printf bad-terminal";
    let (bad_snapshot, _) = fixture.start(bad_command, None);
    let healthy_command = "sleep 0.24; printf healthy-terminal";
    let healthy_launch = start_owned_session(
        &fixture.registry,
        fixture.workspace.path(),
        healthy_conversation,
        healthy_assistant,
        "run-settlement-healthy",
        "call-settlement-healthy",
        healthy_command,
        None,
    )
    .unwrap();
    let AgentCommandSessionLaunch::Running {
        snapshot: healthy_snapshot,
        ..
    } = healthy_launch
    else {
        panic!("healthy command must survive the initial yield");
    };

    let database = Connection::open(&fixture.database_path).unwrap();
    install_terminal_rejection(
        &database,
        "reject_only_bad_settlement",
        &bad_snapshot.session_id,
    );
    fixture.adopt(&bad_snapshot, bad_command);
    adopt_owned_session(
        &fixture.registry,
        &fixture.storage,
        healthy_conversation,
        healthy_assistant,
        "run-settlement-healthy",
        "call-settlement-healthy",
        &healthy_snapshot,
        healthy_command,
    );

    let healthy = wait_for_terminal_record(
        &fixture.storage,
        healthy_conversation,
        &healthy_snapshot.session_id,
    );
    assert_eq!(healthy.snapshot.status, AgentCommandSessionStatus::Exited);
    assert_eq!(healthy.snapshot.exit_code, Some(0));
    let still_blocked = fixture
        .storage
        .load_agent_command_session(&fixture.conversation_id, &bad_snapshot.session_id)
        .unwrap()
        .expect("failed settlement Session");
    assert_eq!(
        still_blocked.snapshot.status,
        AgentCommandSessionStatus::Running
    );
    assert_eq!(fixture.registry.settlement_scheduler_stats().0, 1);
    assert_eq!(fixture.registry.retained_admission_count(), 1);

    database
        .execute_batch("DROP TRIGGER reject_only_bad_settlement;")
        .unwrap();
    wait_for_terminal_record(
        &fixture.storage,
        &fixture.conversation_id,
        &bad_snapshot.session_id,
    );
    assert_eq!(fixture.registry.retained_admission_count(), 0);
}

#[test]
fn shutdown_is_bounded_when_terminal_settlement_keeps_failing() {
    let fixture = RunningFixture::new("settlement-shutdown");
    let command = "sleep 0.15; printf shutdown-blocked";
    let (snapshot, _) = fixture.start(command, None);
    let database = Connection::open(&fixture.database_path).unwrap();
    install_terminal_rejection(
        &database,
        "reject_shutdown_settlement",
        &snapshot.session_id,
    );
    fixture.adopt(&snapshot, command);
    wait_for_settlement_attempts(&fixture.registry, 2);

    let started = Instant::now();
    assert!(!fixture.registry.shutdown(Duration::from_millis(150)));
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "shutdown must not join an unbounded settlement retry"
    );
    assert_eq!(fixture.registry.settlement_scheduler_stats().0, 1);
    assert_eq!(fixture.registry.retained_admission_count(), 1);
}

#[test]
fn synchronous_terminal_failure_uses_the_bounded_shared_retry_scheduler() {
    let fixture = RunningFixture::new_with_admission_limits("synchronous-retry", 1, 1);
    let (snapshot, _) = fixture.start("sleep 0.15; printf synchronous-blocked", None);
    let database = Connection::open(&fixture.database_path).unwrap();
    install_terminal_rejection(
        &database,
        "reject_synchronous_settlement",
        &snapshot.session_id,
    );
    thread::sleep(Duration::from_millis(250));

    fixture
        .registry
        .abort_before_handoff(&snapshot.session_id)
        .expect("the terminal process result remains available despite auxiliary DB failure");
    wait_for_settlement_attempts(&fixture.registry, 3);
    assert_eq!(fixture.registry.settlement_scheduler_stats().0, 1);
    assert_eq!(fixture.registry.settlement_scheduler_stats().1, 1);
    assert_eq!(fixture.registry.retained_admission_count(), 1);
    let rejected = start_owned_session(
        &fixture.registry,
        fixture.workspace.path(),
        &fixture.conversation_id,
        &fixture.assistant_message_id,
        "run-after-synchronous-failure",
        "call-after-synchronous-failure",
        "sleep 5",
        None,
    )
    .unwrap_err();
    assert!(rejected.contains("全局"));

    database
        .execute_batch("DROP TRIGGER reject_synchronous_settlement;")
        .unwrap();
    let settled = wait_for_terminal_record(
        &fixture.storage,
        &fixture.conversation_id,
        &snapshot.session_id,
    );
    assert_eq!(settled.snapshot.status, AgentCommandSessionStatus::Exited);
    assert_eq!(fixture.registry.retained_admission_count(), 0);
}

#[test]
fn every_restart_restores_outcome_unknown_conversation_and_project_fences() {
    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("storage.sqlite");
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    let conversation_id = "conversation-command-session-restart";
    let assistant_message_id = "assistant-command-session-restart";
    let session_id = "cmd_0123456789abcdef0123456789abcdef";
    let project_id = "project-command-session-restart";
    storage
        .save_project(ProjectRecord {
            id: project_id.to_string(),
            name: "Command session restart".to_string(),
            path: Some(fixture.path().to_string_lossy().into_owned()),
            created_at: 1,
            pinned_at: None,
        })
        .unwrap();
    storage
        .save_conversation(ChatConversationRecord {
            id: conversation_id.to_string(),
            project_id: Some(project_id.to_string()),
            model_id: Some("test-model".to_string()),
            title: "Managed command session restart".to_string(),
            messages: vec![ChatMessageRecord {
                id: assistant_message_id.to_string(),
                role: "assistant".to_string(),
                content: String::new(),
                created_at: 1,
                status: Some("pending".to_string()),
                attachments: Vec::new(),
                agent_run_json: None,
                ui_state_json: None,
            }],
            created_at: 1,
            updated_at: 1,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
    storage
        .replace_conversation_turn_trace(
            &completed_running_trace(
                conversation_id,
                assistant_message_id,
                "run-restart",
                "call-restart",
                session_id,
                "sleep 30",
            ),
            1,
            2,
        )
        .unwrap();
    storage
        .create_agent_command_session(&AgentCommandSessionCreate {
            snapshot: AgentCommandSessionSnapshot {
                schema_version: AGENT_COMMAND_SESSION_SCHEMA_VERSION,
                session_id: session_id.to_string(),
                conversation_id: conversation_id.to_string(),
                assistant_message_id: assistant_message_id.to_string(),
                origin_run_id: "run-restart".to_string(),
                call_id: "call-restart".to_string(),
                project_id: Some(project_id.to_string()),
                command: "sleep 30".to_string(),
                cwd: fixture.path().to_string_lossy().into_owned(),
                command_digest: ZERO_DIGEST.to_string(),
                status: AgentCommandSessionStatus::Starting,
                started_at: 10,
                ended_at: None,
                exit_code: None,
                latest_sequence: 0,
                output_truncated: false,
                archive_ref: None,
            },
            authorization_source: CommandAuthorizationSource::ExplicitUser,
            approval_provenance: json!({"status": "approved"}),
            permission_provenance: json!({"command": "auto_approve"}),
            created_at: 10,
        })
        .unwrap();
    storage
        .mark_agent_command_session_running(conversation_id, session_id, 11)
        .unwrap();

    let service = AgentService::new(Arc::clone(&storage));
    let reconciled = storage
        .load_agent_command_session(conversation_id, session_id)
        .unwrap()
        .expect("startup-reconciled command Session");
    assert_eq!(
        reconciled.snapshot.status,
        AgentCommandSessionStatus::OutcomeUnknown
    );
    assert!(reconciled.snapshot.ended_at.is_some());
    assert!(reconciled.snapshot.archive_ref.is_none());
    let restored_effect = format!(
        "run-restart/{}",
        pending_action_storage_id("run-restart", "call-restart")
    );
    assert_eq!(
        service.unsettled_file_effect_ids_for_conversation(conversation_id),
        vec![restored_effect.clone()],
        "an outcome-unknown process must keep its durable deletion fence after restart"
    );
    let deletion_error = service.delete_conversation(conversation_id).unwrap_err();
    assert!(deletion_error.contains(&restored_effect));
    let project_deletion_error = service.delete_project(project_id).unwrap_err();
    assert!(project_deletion_error.contains(&restored_effect));
    assert!(storage
        .load_conversation(conversation_id)
        .unwrap()
        .is_some());
    drop(service);

    let restarted_again = AgentService::new(Arc::clone(&storage));
    assert_eq!(
        restarted_again.unsettled_file_effect_ids_for_conversation(conversation_id),
        vec![restored_effect.clone()],
        "a second restart must rebuild the fence from the persisted outcome-unknown row"
    );
    let second_conversation_error = restarted_again
        .delete_conversation(conversation_id)
        .unwrap_err();
    assert!(second_conversation_error.contains(&restored_effect));
    let second_project_error = restarted_again.delete_project(project_id).unwrap_err();
    assert!(second_project_error.contains(&restored_effect));

    let trace = storage
        .get_conversation_turn_trace(assistant_message_id)
        .unwrap()
        .expect("restart trace");
    let terminal_events = trace
        .items
        .iter()
        .filter(|item| {
            matches!(
                item,
                ConversationTurnTraceItem::CommandSessionLifecycle {
                    phase: ConversationCommandSessionLifecyclePhase::Terminal,
                    session_id: persisted_session_id,
                    status: AgentCommandSessionStatus::OutcomeUnknown,
                    ..
                } if persisted_session_id == session_id
            )
        })
        .count();
    assert_eq!(terminal_events, 1);
}
