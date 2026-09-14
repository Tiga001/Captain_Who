use super::*;
#[cfg(target_os = "macos")]
use mycopilot_core::artifact_runtime::{ArtifactRuntimeDiscoveryOptions, ArtifactRuntimeProvider};
#[cfg(target_os = "macos")]
use mycopilot_core::command::CommandRuntimeProfileResolver;
use mycopilot_core::command::{CommandAuthorizationSource, CommandSessionManagerConfig};
use mycopilot_core::storage::agent_command_session_repository::{
    AgentCommandSessionCreate, AgentCommandSessionModelReadRequest,
    AgentCommandSessionOutputAppend, AgentCommandSessionTerminalUpdate,
    AGENT_COMMAND_SESSION_SCHEMA_VERSION,
};
use mycopilot_core::storage::conversation_history_archive_repository::ConversationHistoryArchivePageUnit;
use mycopilot_core::{
    AgentCommandArtifactKind, AgentCommandArtifactObservationKind,
    AgentCommandArtifactObservationRequest, AgentCommandArtifactValidationStatus,
    AgentCommandExpectedArtifactOutcomeKind, AgentCommandOutputStream, AgentCommandPermission,
    AgentCommandSafetyPolicy, AgentCommandSessionAction, AgentCommandSessionExecutionControl,
    AgentCommandSessionExecutionOutput, AgentCommandSessionExecutionRequest,
    AgentCommandSessionExecutor, AgentCommandSessionGetInput, AgentCommandSessionOutputChunk,
    AgentCommandSessionSnapshot, AgentDisplayStatus, AgentModelBatchReceiptRecord,
    AgentWaitModelProjection, AgentWaitReadySnapshot, AgentWaitTargetSnapshot,
    ConversationCommandSessionLifecyclePhase, ConversationHistoryArchiveTraceMetadata,
    AGENT_COMMAND_SESSION_MODEL_OUTPUT_BYTES,
};
use rusqlite::Connection;
#[cfg(target_os = "macos")]
use sha2::{Digest, Sha256};
use std::io::Write;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Barrier;
use std::thread;
use std::time::{Duration, Instant};
use std::{collections::VecDeque, sync::Mutex as StdMutex};

const INITIAL_YIELD: Duration = Duration::from_millis(10);
const TEST_WAIT: Duration = Duration::from_secs(5);
const ZERO_DIGEST: &str = "sha256:0000000000000000000000000000000000000000000000000000000000000000";

fn observation_control() -> AgentCommandSessionExecutionControl {
    AgentCommandSessionExecutionControl::new(mycopilot_core::AgentCancellationToken::new(), None)
}

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
    handoff_guards: Mutex<HashMap<String, AgentCommandSessionHandoffGuard>>,
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

    fn new_with_handoff_timeout(
        name: &str,
        initial_yield: Duration,
        pending_handoff_timeout: Duration,
    ) -> Self {
        let workspace = tempdir().unwrap();
        let database_path = workspace.path().join("storage.sqlite");
        let storage = Arc::new(StorageService::open(&database_path).unwrap());
        let conversation_id = format!("conversation-command-session-{name}");
        let assistant_message_id = format!("assistant-command-session-{name}");
        let run_id = format!("run-command-session-{name}");
        let call_id = format!("call-command-session-{name}");
        seed_conversation(&storage, &conversation_id, &assistant_message_id);
        let registry = AgentCommandSessionRegistry::with_manager_and_handoff_timeout(
            Arc::clone(&storage),
            CommandSessionManager::new(test_manager_config()).unwrap(),
            initial_yield,
            pending_handoff_timeout,
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
            handoff_guards: Mutex::new(HashMap::new()),
        }
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
            handoff_guards: Mutex::new(HashMap::new()),
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
                handoff_guard,
            } => {
                self.handoff_guards
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .insert(snapshot.session_id.clone(), handoff_guard);
                (*snapshot, tool_result)
            }
            AgentCommandSessionLaunch::Exited(terminal) => panic!(
                "expected a running command session, got {:?}",
                terminal.snapshot.state
            ),
        }
    }

    fn adopt(&self, snapshot: &AgentCommandSessionSnapshot, command: &str) {
        let mut handoff_guard = self
            .handoff_guards
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&snapshot.session_id)
            .expect("running fixture retains the handoff ownership token");
        adopt_owned_session(
            &self.storage,
            &self.conversation_id,
            &self.assistant_message_id,
            &self.run_id,
            &self.call_id,
            snapshot,
            command,
            &mut handoff_guard,
        );
    }

    fn abort(&self, session_id: &str) -> mycopilot_core::command::CommandTerminalResult {
        self.abort_result(session_id).unwrap()
    }

    fn abort_result(
        &self,
        session_id: &str,
    ) -> Result<mycopilot_core::command::CommandTerminalResult, String> {
        self.handoff_guards
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(session_id)
            .expect("running fixture retains the handoff ownership token")
            .abort_before_handoff()
    }

    fn poll(&self, session_id: &str, wait: Duration) -> AgentCommandSessionExecutionOutput {
        let poll_index = self.poll_index.fetch_add(1, Ordering::Relaxed);
        self.registry
            .execute_command_session(
                AgentCommandSessionExecutionRequest {
                    conversation_id: self.conversation_id.clone(),
                    run_id: format!("{}-poll", self.run_id),
                    call_id: format!("{}-poll-{poll_index}", self.call_id),
                    session_id: session_id.to_string(),
                    action: AgentCommandSessionAction::Poll,
                    wait_ms: u64::try_from(wait.as_millis()).unwrap(),
                    max_output_bytes: AGENT_COMMAND_SESSION_MODEL_OUTPUT_BYTES,
                },
                observation_control(),
            )
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
    let tracker = Arc::new(FileEffectTracker::default());
    start_owned_session_with_tracker(
        registry,
        workspace,
        conversation_id,
        assistant_message_id,
        run_id,
        call_id,
        command_text,
        notifications,
        &tracker,
    )
}

#[allow(clippy::too_many_arguments)]
fn start_owned_session_with_tracker(
    registry: &AgentCommandSessionRegistry,
    workspace: &Path,
    conversation_id: &str,
    assistant_message_id: &str,
    run_id: &str,
    call_id: &str,
    command_text: &str,
    notifications: Option<CoreServerNotificationSender>,
    tracker: &Arc<FileEffectTracker>,
) -> Result<AgentCommandSessionLaunch, String> {
    let command = approved_command(call_id, command_text);
    start_owned_session_with_request_and_tracker(
        registry,
        workspace,
        conversation_id,
        assistant_message_id,
        run_id,
        &command,
        notifications,
        tracker,
    )
}

#[allow(clippy::too_many_arguments)]
fn start_owned_session_with_request_and_tracker(
    registry: &AgentCommandSessionRegistry,
    workspace: &Path,
    conversation_id: &str,
    assistant_message_id: &str,
    run_id: &str,
    command: &AgentCommandRequest,
    notifications: Option<CoreServerNotificationSender>,
    tracker: &Arc<FileEffectTracker>,
) -> Result<AgentCommandSessionLaunch, String> {
    let mut file_effect_guard = tracker.register(None, Some(conversation_id), run_id, &command.id);
    file_effect_guard.mark_effects_started();
    let mut file_effect_guard = Some(file_effect_guard);
    registry.start(StartAgentCommandSession {
        owner: CommandSessionOwner {
            conversation_id: conversation_id.to_string(),
            assistant_message_id: assistant_message_id.to_string(),
            origin_run_id: run_id.to_string(),
            call_id: command.id.clone(),
            project_id: None,
        },
        workspace_root: Some(workspace),
        command,
        permissions: test_permissions(),
        authorization_source: CommandAuthorizationSource::ExplicitUser,
        approval_provenance: json!({
            "source": "explicit_user",
            "status": "approved"
        }),
        artifact_runtime: None,
        office_engine: None,
        file_inputs: None,
        notifications,
        cancellation_token: AgentCancellationToken::new(),
        cancel_probe: None,
        file_effect_guard: &mut file_effect_guard,
    })
}

#[allow(clippy::too_many_arguments)]
fn adopt_owned_session(
    storage: &StorageService,
    conversation_id: &str,
    assistant_message_id: &str,
    run_id: &str,
    call_id: &str,
    snapshot: &AgentCommandSessionSnapshot,
    command: &str,
    handoff_guard: &mut AgentCommandSessionHandoffGuard,
) {
    let trace = completed_running_trace(
        conversation_id,
        assistant_message_id,
        run_id,
        call_id,
        &snapshot.session_id,
        command,
    );
    let outcome = handoff_guard
        .commit(
            || false,
            || storage.replace_conversation_turn_trace(&trace, 1, now_ms()),
        )
        .unwrap();
    assert_eq!(outcome, AgentCommandHandoffOutcome::Adopted);
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
        runtime_binding: None,
        managed_office_script: None,
    }
}

fn write_minimal_presentation(path: &Path) {
    let file = std::fs::File::create(path).unwrap();
    let mut archive = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default();
    archive.start_file("[Content_Types].xml", options).unwrap();
    archive
        .write_all(
            br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="xml" ContentType="application/xml"/>
  <Override PartName="/ppt/presentation.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml"/>
</Types>"#,
        )
        .unwrap();
    archive.start_file("ppt/presentation.xml", options).unwrap();
    archive
        .write_all(
            br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:presentation xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"/>"#,
        )
        .unwrap();
    archive.finish().unwrap();
}

fn seed_conversation(storage: &StorageService, conversation_id: &str, assistant_message_id: &str) {
    storage
        .save_conversation(ChatConversationRecord {
            id: conversation_id.to_string(),
            project_id: None,
            model_id: Some("test-model".to_string()),
            title: "Managed command session test".to_string(),
            messages: vec![ChatMessageRecord {
                human_interaction_response: None,
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
                provenance: AgentToolIdentity::Builtin {
                    tool_name: "run_command".to_string(),
                },
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
    wait_for_terminal_record_with_timeout(storage, conversation_id, session_id, TEST_WAIT)
}

fn wait_for_terminal_record_with_timeout(
    storage: &StorageService,
    conversation_id: &str,
    session_id: &str,
    wait: Duration,
) -> mycopilot_core::storage::agent_command_session_repository::AgentCommandSessionRecord {
    let deadline = Instant::now() + wait;
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

fn install_session_create_rejection(database: &Connection, trigger_name: &str) {
    assert!(trigger_name
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_'));
    database
        .execute_batch(&format!(
            "CREATE TRIGGER {trigger_name}
             BEFORE INSERT ON agent_command_sessions
             BEGIN
               SELECT RAISE(ABORT, 'injected command session create failure');
             END;"
        ))
        .unwrap();
}

fn install_session_running_rejection(database: &Connection, trigger_name: &str) {
    assert!(trigger_name
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_'));
    database
        .execute_batch(&format!(
            "CREATE TRIGGER {trigger_name}
             BEFORE UPDATE OF status ON agent_command_sessions
             WHEN OLD.status = 'starting' AND NEW.status = 'running'
             BEGIN
               SELECT RAISE(ABORT, 'injected command session running transition failure');
             END;"
        ))
        .unwrap();
}

fn install_archive_rejection(database: &Connection, trigger_name: &str) {
    assert!(trigger_name
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_'));
    database
        .execute_batch(&format!(
            "CREATE TRIGGER {trigger_name}
             BEFORE INSERT ON conversation_history_blobs
             BEGIN
               SELECT RAISE(ABORT, 'injected command session archive failure');
             END;"
        ))
        .unwrap();
}

fn assert_session_create_failure_is_fail_closed(
    fixture: &RunningFixture,
    tracker: &Arc<FileEffectTracker>,
    command_text: &str,
) -> mycopilot_core::command::CommandTerminalResult {
    let command = approved_command(&fixture.call_id, command_text);
    let mut file_effect_guard = tracker.register(
        None,
        Some(&fixture.conversation_id),
        &fixture.run_id,
        &fixture.call_id,
    );
    file_effect_guard.mark_effects_started();
    let mut file_effect_guard = Some(file_effect_guard);
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let launch = fixture
        .registry
        .start(StartAgentCommandSession {
            owner: CommandSessionOwner {
                conversation_id: fixture.conversation_id.clone(),
                assistant_message_id: fixture.assistant_message_id.clone(),
                origin_run_id: fixture.run_id.clone(),
                call_id: fixture.call_id.clone(),
                project_id: None,
            },
            workspace_root: Some(fixture.workspace.path()),
            command: &command,
            permissions: test_permissions(),
            authorization_source: CommandAuthorizationSource::ExplicitUser,
            approval_provenance: json!({
                "source": "explicit_user",
                "status": "approved"
            }),
            artifact_runtime: None,
            office_engine: None,
            file_inputs: None,
            notifications: Some(notifications),
            cancellation_token: AgentCancellationToken::new(),
            cancel_probe: None,
            file_effect_guard: &mut file_effect_guard,
        })
        .expect("a confirmed termination projects the durable-start failure as an execution");
    let AgentCommandSessionLaunch::Exited(terminal) = launch else {
        panic!("a failed durable create must never return a Running receipt")
    };

    assert!(matches!(
        terminal.snapshot.state,
        mycopilot_core::command::CommandSessionState::Failed
    ));
    assert!(terminal
        .execution
        .error
        .as_deref()
        .is_some_and(|error| error.contains("durable start state could not be persisted")));
    assert!(!terminal.execution.cancelled);
    assert!(!terminal.execution.timed_out);
    assert!(
        file_effect_guard.is_some(),
        "missing-row failure keeps File Effect ownership with the ordinary action audit"
    );
    assert!(fixture
        .storage
        .load_agent_command_session(
            &fixture.conversation_id,
            terminal.snapshot.session_id.as_str(),
        )
        .unwrap()
        .is_none());
    assert_eq!(fixture.registry.retained_live_session_count(), 0);
    assert_eq!(fixture.registry.retained_core_session_count(), 0);
    assert_eq!(fixture.registry.retained_admission_count(), 0);
    assert_eq!(fixture.registry.settlement_scheduler_stats().1, 0);
    assert!(
        receiver.try_recv().is_err(),
        "an undurable Session must not publish process lifecycle events"
    );

    // This is the same ownership step performed by the automatic/manual action audit after it
    // persists the failed ToolResult returned above. It must leave neither an active lease nor an
    // unresolved deletion fence.
    file_effect_guard
        .as_mut()
        .expect("caller-owned File Effect guard")
        .mark_durably_settled();
    drop(file_effect_guard);
    assert!(tracker
        .active_run_ids_for_conversation(&fixture.conversation_id)
        .is_empty());
    assert!(tracker
        .unsettled_effect_ids_for_conversation(&fixture.conversation_id)
        .is_empty());

    *terminal
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

fn wait_for_retained_admission_count(
    registry: &AgentCommandSessionRegistry,
    expected_count: usize,
) {
    let deadline = Instant::now() + TEST_WAIT;
    loop {
        if registry.retained_admission_count() == expected_count {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "command Session admission count did not reach {expected_count}"
        );
        thread::yield_now();
    }
}

fn wait_for_terminal_cleanup(registry: &AgentCommandSessionRegistry) {
    let deadline = Instant::now() + TEST_WAIT;
    loop {
        let retained = (
            registry.retained_admission_count(),
            registry.retained_live_session_count(),
            registry.retained_core_session_count(),
            registry.settlement_scheduler_stats().1,
        );
        if retained == (0, 0, 0, 0) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "terminal cleanup retained Host/Core ownership: {retained:?}"
        );
        thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn short_command_exits_through_the_same_managed_session_entry() {
    let fixture =
        RunningFixture::new_with_initial_yield("short-managed-entry", Duration::from_millis(500));
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let launch = start_owned_session(
        &fixture.registry,
        fixture.workspace.path(),
        &fixture.conversation_id,
        &fixture.assistant_message_id,
        &fixture.run_id,
        &fixture.call_id,
        "printf short-managed-output",
        Some(notifications),
    )
    .unwrap();
    let AgentCommandSessionLaunch::Exited(terminal) = launch else {
        panic!("short command must exit inside the initial yield")
    };
    assert_eq!(terminal.snapshot.exit_code, Some(0));
    assert_eq!(terminal.execution.stdout, "short-managed-output");

    let record = fixture
        .storage
        .load_agent_command_session(
            &fixture.conversation_id,
            terminal.snapshot.session_id.as_str(),
        )
        .unwrap()
        .expect("short command still has one durable managed Session receipt");
    assert_eq!(record.snapshot.status, AgentCommandSessionStatus::Exited);
    assert_eq!(record.snapshot.exit_code, Some(0));

    let event_types = std::iter::from_fn(|| receiver.try_recv().ok())
        .map(|event| event["params"]["type"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    assert!(event_types.iter().any(|kind| kind == "command_started"));
    assert!(event_types.iter().any(|kind| kind == "command_exited"));
}

#[test]
fn handed_off_office_artifact_observation_reaches_event_snapshot_and_model_wait() {
    let fixture = RunningFixture::new("office-artifact-observation");
    write_minimal_presentation(&fixture.workspace.path().join("source.pptx"));
    let command_text = "sleep 0.08; cp source.pptx edited.pptx; sleep 0.08";
    let mut command = approved_command(&fixture.call_id, command_text);
    command.observe = Some(AgentCommandArtifactObservationRequest {
        kinds: vec![AgentCommandArtifactObservationKind::Office],
        expected_outputs: vec!["edited.pptx".to_string()],
        additional_roots: Vec::new(),
    });
    let tracker = Arc::new(FileEffectTracker::default());
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let launch = start_owned_session_with_request_and_tracker(
        &fixture.registry,
        fixture.workspace.path(),
        &fixture.conversation_id,
        &fixture.assistant_message_id,
        &fixture.run_id,
        &command,
        Some(notifications),
        &tracker,
    )
    .unwrap();
    let AgentCommandSessionLaunch::Running {
        snapshot,
        mut handoff_guard,
        ..
    } = launch
    else {
        panic!("the delayed presentation copy must hand off")
    };
    adopt_owned_session(
        &fixture.storage,
        &fixture.conversation_id,
        &fixture.assistant_message_id,
        &fixture.run_id,
        &fixture.call_id,
        &snapshot,
        command_text,
        &mut handoff_guard,
    );

    let record = wait_for_terminal_record(
        &fixture.storage,
        &fixture.conversation_id,
        &snapshot.session_id,
    );
    assert_eq!(record.snapshot.status, AgentCommandSessionStatus::Exited);
    assert_eq!(record.snapshot.exit_code, Some(0));
    let observation = record
        .snapshot
        .artifact_observation
        .as_ref()
        .expect("terminal Session snapshot retains the validated Office observation");
    let expected = observation
        .expected_outputs
        .first()
        .expect("the approved presentation destination remains explicit");
    assert_eq!(expected.path.as_deref(), Some("edited.pptx"));
    assert_eq!(
        expected.outcome,
        AgentCommandExpectedArtifactOutcomeKind::Created
    );
    assert_eq!(
        expected.artifact_kind,
        Some(AgentCommandArtifactKind::Presentation)
    );
    assert_eq!(
        expected
            .metadata
            .as_ref()
            .map(|metadata| metadata.validation.status),
        Some(AgentCommandArtifactValidationStatus::Valid)
    );

    let reopened = StorageService::open(&fixture.database_path).unwrap();
    let restored = reopened
        .load_agent_command_session(&fixture.conversation_id, &snapshot.session_id)
        .unwrap()
        .expect("terminal Session survives Host reopen");
    assert_eq!(
        restored.snapshot.artifact_observation,
        Some(observation.clone())
    );

    let model = fixture.poll(&snapshot.session_id, Duration::ZERO);
    assert_eq!(model.status, AgentCommandSessionStatus::Exited);
    assert_eq!(model.exit_code, Some(0));
    assert_eq!(model.artifact_observation, Some(observation.clone()));

    let terminal_event = std::iter::from_fn(|| receiver.try_recv().ok())
        .find(|event| event["params"]["type"] == "command_exited")
        .expect("terminal notification");
    assert_eq!(terminal_event["params"]["status"], "exited");
    assert_eq!(terminal_event["params"]["exitCode"], 0);
    assert_eq!(
        terminal_event["params"]["artifactObservation"]["expectedOutputs"][0]["artifactKind"],
        "presentation"
    );
    assert_eq!(
        terminal_event["params"]["artifactObservation"]["expectedOutputs"][0]["metadata"]
            ["validation"]["status"],
        "valid"
    );
}

#[test]
fn short_command_create_failure_returns_failed_execution_without_session_leaks() {
    let fixture =
        RunningFixture::new_with_initial_yield("short-create-failure", Duration::from_secs(1));
    let database = Connection::open(&fixture.database_path).unwrap();
    install_session_create_rejection(&database, "reject_short_session_create");
    let tracker = Arc::new(FileEffectTracker::default());

    let terminal = assert_session_create_failure_is_fail_closed(
        &fixture,
        &tracker,
        "printf short-create-failure-output",
    );
    // Preserve the real process evidence, but never let exit 0 turn the ToolResult into success.
    assert_eq!(terminal.snapshot.exit_code, Some(0));
    assert_eq!(terminal.execution.exit_code, Some(0));
    assert_eq!(terminal.execution.stdout, "short-create-failure-output");
}

#[test]
fn long_command_create_failure_is_terminated_without_session_leaks() {
    let fixture = RunningFixture::new("long-create-failure");
    let database = Connection::open(&fixture.database_path).unwrap();
    install_session_create_rejection(&database, "reject_long_session_create");
    let tracker = Arc::new(FileEffectTracker::default());

    let terminal = assert_session_create_failure_is_fail_closed(&fixture, &tracker, "sleep 30");
    assert!(terminal.snapshot.exit_code.is_none());
    assert!(terminal.execution.exit_code.is_none());
}

#[test]
fn create_commit_unknown_is_recovered_by_authoritative_owner_readback() {
    let fixture = RunningFixture::new("create-commit-unknown");
    fixture
        .registry
        .set_after_durable_create_hook(Arc::new(|_| {
            Err("injected post-commit create error".to_string())
        }));

    let (snapshot, tool_result) = fixture.start("sleep 30", None);
    assert_eq!(snapshot.status, AgentCommandSessionStatus::Running);
    assert_eq!(tool_result.result.as_ref().unwrap()["status"], "running");
    let record = fixture
        .storage
        .load_agent_command_session(&fixture.conversation_id, &snapshot.session_id)
        .unwrap()
        .expect("the post-commit row is authoritatively recovered");
    assert_eq!(record.snapshot.status, AgentCommandSessionStatus::Running);
    assert_eq!(record.snapshot.conversation_id, fixture.conversation_id);
    assert_eq!(
        record.snapshot.assistant_message_id,
        fixture.assistant_message_id
    );
    assert_eq!(record.snapshot.origin_run_id, fixture.run_id);
    assert_eq!(record.snapshot.call_id, fixture.call_id);

    let terminal = fixture.abort(&snapshot.session_id);
    assert!(matches!(
        terminal.snapshot.state,
        mycopilot_core::command::CommandSessionState::Interrupted
    ));
    let record = wait_for_terminal_record(
        &fixture.storage,
        &fixture.conversation_id,
        &snapshot.session_id,
    );
    assert_eq!(
        record.snapshot.status,
        AgentCommandSessionStatus::Interrupted
    );
    assert_eq!(fixture.registry.retained_admission_count(), 0);
    assert_eq!(fixture.registry.retained_live_session_count(), 0);
}

#[test]
fn indeterminate_reconciliation_cannot_settle_before_file_effect_install() {
    let fixture =
        RunningFixture::new_with_initial_yield("indeterminate-start-fence", Duration::from_secs(2));
    fixture
        .registry
        .set_after_durable_create_hook(Arc::new(|_| {
            Err("injected post-commit create error".to_string())
        }));
    let inspection_attempts = Arc::new(AtomicUsize::new(0));
    let reconcile_entered = Arc::new(Barrier::new(2));
    let reconcile_release = Arc::new(Barrier::new(2));
    let hook_attempts = Arc::clone(&inspection_attempts);
    let hook_entered = Arc::clone(&reconcile_entered);
    let hook_release = Arc::clone(&reconcile_release);
    fixture
        .registry
        .set_durable_start_inspection_hook(Arc::new(move |_| {
            match hook_attempts.fetch_add(1, Ordering::SeqCst) {
                0 => return Err("injected initial authoritative read failure".to_string()),
                1 => {
                    hook_entered.wait();
                    hook_release.wait();
                }
                _ => {}
            }
            Ok(())
        }));

    let tracker = Arc::new(FileEffectTracker::default());
    let registry = fixture.registry.clone();
    let workspace = fixture.workspace.path().to_path_buf();
    let conversation_id = fixture.conversation_id.clone();
    let assistant_message_id = fixture.assistant_message_id.clone();
    let run_id = fixture.run_id.clone();
    let call_id = fixture.call_id.clone();
    let tracker_for_start = Arc::clone(&tracker);
    let start = thread::spawn(move || {
        start_owned_session_with_tracker(
            &registry,
            &workspace,
            &conversation_id,
            &assistant_message_id,
            &run_id,
            &call_id,
            "printf indeterminate-start-fence",
            None,
            &tracker_for_start,
        )
    });

    reconcile_entered.wait();
    // The worker has reached authoritative readback but cannot settle yet. File Effect and Host
    // admission must already be installed, closing the exact wait-boundary race.
    assert_eq!(fixture.registry.retained_admission_count(), 1);
    assert_eq!(fixture.registry.retained_live_session_count(), 1);
    assert_eq!(
        tracker.active_run_ids_for_conversation(&fixture.conversation_id),
        vec![fixture.run_id.clone()]
    );
    reconcile_release.wait();

    let launch = start.join().expect("indeterminate start thread").unwrap();
    let AgentCommandSessionLaunch::Exited(terminal) = launch else {
        panic!("the recovered starting row must settle as one Failed terminal")
    };
    assert!(matches!(
        terminal.snapshot.state,
        mycopilot_core::command::CommandSessionState::Failed
    ));
    let record = wait_for_terminal_record(
        &fixture.storage,
        &fixture.conversation_id,
        terminal.snapshot.session_id.as_str(),
    );
    assert_eq!(record.snapshot.status, AgentCommandSessionStatus::Failed);
    assert_eq!(fixture.registry.retained_admission_count(), 0);
    assert_eq!(fixture.registry.retained_live_session_count(), 0);
    assert_eq!(fixture.registry.settlement_scheduler_stats().1, 0);
    assert!(tracker
        .active_run_ids_for_conversation(&fixture.conversation_id)
        .is_empty());
    assert!(tracker
        .unsettled_effect_ids_for_conversation(&fixture.conversation_id)
        .is_empty());
}

#[test]
fn indeterminate_then_absent_synthesizes_one_settled_failed_session() {
    let fixture = RunningFixture::new("indeterminate-then-absent");
    let database = Connection::open(&fixture.database_path).unwrap();
    install_session_create_rejection(&database, "reject_indeterminate_absent_create");
    let inspection_attempts = Arc::new(AtomicUsize::new(0));
    let hook_attempts = Arc::clone(&inspection_attempts);
    let database_path = fixture.database_path.clone();
    fixture
        .registry
        .set_durable_start_inspection_hook(Arc::new(move |_| {
            if hook_attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                Err("injected initial authoritative read failure".to_string())
            } else {
                Connection::open(&database_path)
                    .unwrap()
                    .execute_batch("DROP TRIGGER reject_indeterminate_absent_create;")
                    .unwrap();
                Ok(())
            }
        }));
    let tracker = Arc::new(FileEffectTracker::default());
    let command = approved_command(&fixture.call_id, "printf early-undurable-output; sleep 30");
    let mut file_effect_guard = tracker.register(
        None,
        Some(&fixture.conversation_id),
        &fixture.run_id,
        &fixture.call_id,
    );
    file_effect_guard.mark_effects_started();
    let mut file_effect_guard = Some(file_effect_guard);

    let launch = fixture
        .registry
        .start(StartAgentCommandSession {
            owner: CommandSessionOwner {
                conversation_id: fixture.conversation_id.clone(),
                assistant_message_id: fixture.assistant_message_id.clone(),
                origin_run_id: fixture.run_id.clone(),
                call_id: fixture.call_id.clone(),
                project_id: None,
            },
            workspace_root: Some(fixture.workspace.path()),
            command: &command,
            permissions: test_permissions(),
            authorization_source: CommandAuthorizationSource::ExplicitUser,
            approval_provenance: json!({
                "source": "explicit_user",
                "status": "approved"
            }),
            artifact_runtime: None,
            office_engine: None,
            file_inputs: None,
            notifications: None,
            cancellation_token: AgentCancellationToken::new(),
            cancel_probe: None,
            file_effect_guard: &mut file_effect_guard,
        })
        .unwrap();
    let AgentCommandSessionLaunch::Exited(terminal) = launch else {
        panic!("authoritative Absent reconciliation must return a failed execution")
    };
    assert!(matches!(
        terminal.snapshot.state,
        mycopilot_core::command::CommandSessionState::Failed
    ));
    assert!(terminal.snapshot.latest_output_sequence > 0);
    assert!(terminal.snapshot.output_truncated);
    assert!(file_effect_guard.is_none());
    assert_eq!(fixture.registry.retained_admission_count(), 0);
    assert_eq!(fixture.registry.retained_live_session_count(), 0);
    assert_eq!(fixture.registry.retained_core_session_count(), 0);
    assert_eq!(fixture.registry.settlement_scheduler_stats().1, 0);

    drop(file_effect_guard);
    let record = fixture
        .storage
        .load_agent_command_session(
            &fixture.conversation_id,
            terminal.snapshot.session_id.as_str(),
        )
        .unwrap()
        .expect("reconciliation synthesizes a recoverable Session row");
    assert_eq!(record.snapshot.status, AgentCommandSessionStatus::Failed);
    assert!(record.snapshot.latest_sequence > 0);
    assert!(record.snapshot.output_truncated);
    assert!(record.snapshot.archive_ref.is_some());
    assert!(tracker
        .active_run_ids_for_conversation(&fixture.conversation_id)
        .is_empty());
    assert!(tracker
        .unsettled_effect_ids_for_conversation(&fixture.conversation_id)
        .is_empty());
}

#[test]
fn indeterminate_mismatched_start_identity_is_never_adopted() {
    let fixture = RunningFixture::new_with_initial_yield(
        "indeterminate-mismatched-identity",
        Duration::from_secs(2),
    );
    fixture
        .registry
        .set_after_durable_create_hook(Arc::new(|_| {
            Err("injected post-commit create error".to_string())
        }));
    let inspection_attempts = Arc::new(AtomicUsize::new(0));
    let hook_attempts = Arc::clone(&inspection_attempts);
    let database_path = fixture.database_path.clone();
    fixture
        .registry
        .set_durable_start_inspection_hook(Arc::new(move |session_id| {
            let database = Connection::open(&database_path).unwrap();
            match hook_attempts.fetch_add(1, Ordering::SeqCst) {
                0 => {
                    database
                        .execute_batch(
                            "DROP TRIGGER prevent_agent_command_session_identity_update;",
                        )
                        .unwrap();
                    database
                        .execute(
                            "UPDATE agent_command_sessions
                             SET command_projection = 'tampered-command'
                             WHERE session_id = ?1",
                            rusqlite::params![session_id],
                        )
                        .unwrap();
                }
                1 => {
                    database
                        .execute(
                            "DELETE FROM agent_command_sessions WHERE session_id = ?1",
                            rusqlite::params![session_id],
                        )
                        .unwrap();
                }
                _ => {}
            }
            Ok(())
        }));
    let tracker = Arc::new(FileEffectTracker::default());
    let command = "printf identity-mismatch";

    let launch = start_owned_session_with_tracker(
        &fixture.registry,
        fixture.workspace.path(),
        &fixture.conversation_id,
        &fixture.assistant_message_id,
        &fixture.run_id,
        &fixture.call_id,
        command,
        None,
        &tracker,
    )
    .unwrap();
    let AgentCommandSessionLaunch::Exited(terminal) = launch else {
        panic!("an identity-mismatched durable row must never produce a Running receipt")
    };
    assert!(matches!(
        terminal.snapshot.state,
        mycopilot_core::command::CommandSessionState::Failed
    ));
    let record = fixture
        .storage
        .load_agent_command_session(
            &fixture.conversation_id,
            terminal.snapshot.session_id.as_str(),
        )
        .unwrap()
        .expect("the canonical immutable start identity is reconstructed");
    assert_eq!(record.snapshot.command, command);
    assert_ne!(record.snapshot.command, "tampered-command");
    assert_eq!(record.snapshot.status, AgentCommandSessionStatus::Failed);
    assert_eq!(fixture.registry.retained_admission_count(), 0);
    assert_eq!(fixture.registry.retained_live_session_count(), 0);
    assert_eq!(fixture.registry.settlement_scheduler_stats().1, 0);
    assert!(tracker
        .unsettled_effect_ids_for_conversation(&fixture.conversation_id)
        .is_empty());
}

#[test]
fn running_transition_failure_has_one_failed_terminal_across_host_and_storage() {
    let fixture = RunningFixture::new("running-transition-failure");
    let database = Connection::open(&fixture.database_path).unwrap();
    install_session_running_rejection(&database, "reject_session_running_transition");
    let tracker = Arc::new(FileEffectTracker::default());
    let command = approved_command(&fixture.call_id, "printf early-undurable-output; sleep 30");
    let mut file_effect_guard = tracker.register(
        None,
        Some(&fixture.conversation_id),
        &fixture.run_id,
        &fixture.call_id,
    );
    file_effect_guard.mark_effects_started();
    let mut file_effect_guard = Some(file_effect_guard);
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();

    let launch = fixture
        .registry
        .start(StartAgentCommandSession {
            owner: CommandSessionOwner {
                conversation_id: fixture.conversation_id.clone(),
                assistant_message_id: fixture.assistant_message_id.clone(),
                origin_run_id: fixture.run_id.clone(),
                call_id: fixture.call_id.clone(),
                project_id: None,
            },
            workspace_root: Some(fixture.workspace.path()),
            command: &command,
            permissions: test_permissions(),
            authorization_source: CommandAuthorizationSource::ExplicitUser,
            approval_provenance: json!({
                "source": "explicit_user",
                "status": "approved"
            }),
            artifact_runtime: None,
            office_engine: None,
            file_inputs: None,
            notifications: Some(notifications),
            cancellation_token: AgentCancellationToken::new(),
            cancel_probe: None,
            file_effect_guard: &mut file_effect_guard,
        })
        .unwrap();
    let AgentCommandSessionLaunch::Exited(terminal) = launch else {
        panic!("a failed running transition must not produce a Running receipt")
    };
    assert!(matches!(
        terminal.snapshot.state,
        mycopilot_core::command::CommandSessionState::Failed
    ));
    assert!(terminal
        .execution
        .error
        .as_deref()
        .is_some_and(|error| error.contains("durable start state could not be persisted")));
    assert!(terminal.snapshot.latest_output_sequence > 0);
    assert!(terminal.snapshot.output_truncated);
    assert!(
        file_effect_guard.is_none(),
        "an existing Session row transfers File Effect ownership to terminal settlement"
    );

    let record = fixture
        .storage
        .load_agent_command_session(
            &fixture.conversation_id,
            terminal.snapshot.session_id.as_str(),
        )
        .unwrap()
        .expect("the created Session row is terminally settled");
    assert_eq!(record.snapshot.status, AgentCommandSessionStatus::Failed);
    assert!(record.snapshot.latest_sequence > 0);
    assert!(record.snapshot.output_truncated);
    assert!(record
        .terminal_reason
        .as_deref()
        .is_some_and(|error| error.contains("durable start state could not be persisted")));
    assert!(record.snapshot.archive_ref.is_some());
    let host = fixture
        .registry
        .get(AgentCommandSessionGetInput {
            conversation_id: fixture.conversation_id.clone(),
            session_id: terminal.snapshot.session_id.to_string(),
            after_sequence: None,
            max_bytes: None,
        })
        .unwrap();
    assert!(host.session.output_truncated);
    assert!(host.transcript.chunks.is_empty());
    let model = fixture
        .registry
        .execute_command_session(
            AgentCommandSessionExecutionRequest {
                conversation_id: fixture.conversation_id.clone(),
                run_id: format!("{}-failure-poll", fixture.run_id),
                call_id: format!("{}-failure-poll", fixture.call_id),
                session_id: terminal.snapshot.session_id.to_string(),
                action: AgentCommandSessionAction::Poll,
                wait_ms: 0,
                max_output_bytes: AGENT_COMMAND_SESSION_MODEL_OUTPUT_BYTES,
            },
            observation_control(),
        )
        .unwrap();
    assert_eq!(model.status, AgentCommandSessionStatus::Failed);
    assert!(model.output_truncated);
    assert!(!terminal.execution.stdout_spool.is_present());
    assert!(!terminal.execution.stderr_spool.is_present());
    let ordinary_tool_result =
        mycopilot_core::command::command_tool_result(&fixture.call_id, &terminal.execution);
    assert!(ordinary_tool_result.exact_archive_file.is_none());
    let archive_count: u64 = database
        .query_row(
            "SELECT COUNT(*) FROM conversation_history_blobs WHERE conversation_id = ?1",
            rusqlite::params![&fixture.conversation_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(archive_count, 1);

    let events = std::iter::from_fn(|| receiver.try_recv().ok()).collect::<Vec<_>>();
    assert!(!events
        .iter()
        .any(|event| event["params"]["type"] == "command_started"));
    let terminal_events = events
        .iter()
        .filter(|event| event["params"]["type"] == "command_exited")
        .collect::<Vec<_>>();
    assert_eq!(terminal_events.len(), 1);
    assert_eq!(terminal_events[0]["params"]["status"], "failed");
    assert!(!events
        .iter()
        .any(|event| event["params"]["type"] == "command_interrupted"));
    assert_eq!(fixture.registry.retained_live_session_count(), 0);
    assert_eq!(fixture.registry.retained_core_session_count(), 0);
    assert_eq!(fixture.registry.retained_admission_count(), 0);
    assert_eq!(fixture.registry.settlement_scheduler_stats().1, 0);
    assert!(tracker
        .active_run_ids_for_conversation(&fixture.conversation_id)
        .is_empty());
    assert!(tracker
        .unsettled_effect_ids_for_conversation(&fixture.conversation_id)
        .is_empty());
}

#[test]
fn synchronous_archive_failure_returns_recoverable_session_until_unique_archive_recovers() {
    let fixture = RunningFixture::new_with_initial_yield(
        "synchronous-archive-failure",
        Duration::from_secs(2),
    );
    let database = Connection::open(&fixture.database_path).unwrap();
    install_archive_rejection(&database, "reject_synchronous_session_archive");
    let tracker = Arc::new(FileEffectTracker::default());
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let registry = fixture.registry.clone();
    let workspace = fixture.workspace.path().to_path_buf();
    let conversation_id = fixture.conversation_id.clone();
    let assistant_message_id = fixture.assistant_message_id.clone();
    let run_id = fixture.run_id.clone();
    let call_id = fixture.call_id.clone();
    let tracker_for_start = Arc::clone(&tracker);
    let command = r#"awk 'BEGIN { for (i = 0; i < 20000; i++) printf "archive-retry-%05d\n", i }'"#;
    let command_for_start = command.to_string();
    let start = thread::spawn(move || {
        start_owned_session_with_tracker(
            &registry,
            &workspace,
            &conversation_id,
            &assistant_message_id,
            &run_id,
            &call_id,
            &command_for_start,
            Some(notifications),
            &tracker_for_start,
        )
    });

    let deadline = Instant::now() + TEST_WAIT;
    let session_id = loop {
        if let Ok(event) = receiver.try_recv() {
            if event["params"]["type"] == "command_started" {
                break event["params"]["sessionId"].as_str().unwrap().to_string();
            }
        }
        assert!(Instant::now() < deadline, "missing command_started");
        thread::sleep(Duration::from_millis(5));
    };
    let launch = start
        .join()
        .expect("synchronous archive failure thread")
        .expect("archive failure remains a recoverable Session launch");
    let AgentCommandSessionLaunch::Running {
        snapshot,
        tool_result,
        mut handoff_guard,
    } = launch
    else {
        panic!("an unsettled terminal archive must return a recoverable Session")
    };
    assert_eq!(snapshot.session_id, session_id);
    let running = tool_result.result.as_ref().expect("running receipt");
    assert_eq!(running["status"], "running");
    assert_eq!(running["sessionId"], session_id);
    assert_eq!(running["continueWith"]["tool"], "command_session");
    assert_eq!(
        running["continueWith"]["args"],
        json!({ "sessionId": session_id, "action": "wait" })
    );
    adopt_owned_session(
        &fixture.storage,
        &fixture.conversation_id,
        &fixture.assistant_message_id,
        &fixture.run_id,
        &fixture.call_id,
        &snapshot,
        command,
        &mut handoff_guard,
    );
    wait_for_settlement_attempts(&fixture.registry, 1);
    assert_eq!(fixture.registry.retained_admission_count(), 1);
    assert_eq!(fixture.registry.settlement_scheduler_stats().1, 1);

    database
        .execute_batch("DROP TRIGGER reject_synchronous_session_archive;")
        .unwrap();
    let record = wait_for_terminal_record(&fixture.storage, &fixture.conversation_id, &session_id);
    assert_eq!(record.snapshot.status, AgentCommandSessionStatus::Exited);
    let archive_ref = record
        .snapshot
        .archive_ref
        .as_deref()
        .expect("recovered Session owns the unique Exact Archive");
    let archive_count: u64 = database
        .query_row(
            "SELECT COUNT(*) FROM conversation_history_blobs WHERE conversation_id = ?1",
            rusqlite::params![&fixture.conversation_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(archive_count, 1);
    let descriptor = fixture
        .storage
        .find_conversation_history_archive_by_ref(&fixture.conversation_id, archive_ref)
        .unwrap()
        .expect("recovered Exact Archive descriptor");
    assert!(descriptor.total_bytes > 200_000);
    // The SQLite terminal commit precedes release of in-memory process ownership. Wait for that
    // separate cleanup boundary before checking for retained leases and worker entries.
    wait_for_terminal_cleanup(&fixture.registry);
    assert_eq!(fixture.registry.retained_admission_count(), 0);
    assert_eq!(fixture.registry.retained_live_session_count(), 0);
    assert_eq!(fixture.registry.retained_core_session_count(), 0);
    assert_eq!(fixture.registry.settlement_scheduler_stats().1, 0);
    assert!(tracker
        .active_run_ids_for_conversation(&fixture.conversation_id)
        .is_empty());
    assert!(tracker
        .unsettled_effect_ids_for_conversation(&fixture.conversation_id)
        .is_empty());

    let restarted_storage = Arc::new(StorageService::open(&fixture.database_path).unwrap());
    let restarted = AgentCommandSessionRegistry::with_manager(
        Arc::clone(&restarted_storage),
        CommandSessionManager::new(test_manager_config()).unwrap(),
        INITIAL_YIELD,
    );
    let replay = restarted
        .execute_command_session(
            AgentCommandSessionExecutionRequest {
                conversation_id: fixture.conversation_id.clone(),
                run_id: "run-synchronous-archive-recovery-read".to_string(),
                call_id: "call-synchronous-archive-recovery-read".to_string(),
                session_id,
                action: AgentCommandSessionAction::Poll,
                wait_ms: 0,
                max_output_bytes: AGENT_COMMAND_SESSION_MODEL_OUTPUT_BYTES,
            },
            observation_control(),
        )
        .unwrap();
    assert_eq!(replay.status, AgentCommandSessionStatus::Exited);
    let history_open = replay
        .history_open
        .as_deref()
        .expect("restarted Host exposes the recovered archive route");
    let tail = restarted_storage
        .read_conversation_history_archive_page_from_open(
            &fixture.conversation_id,
            history_open,
            u64::MAX,
        )
        .unwrap()
        .expect("recovered archive remains readable after a true Storage restart");
    assert!(tail.content.contains("archive-retry-19999"));
}

#[test]
fn synchronous_settlement_failure_serializes_concurrent_cancel_with_recovery_receipt() {
    let fixture = RunningFixture::new_with_initial_yield(
        "synchronous-settlement-cancel-fence",
        Duration::from_secs(2),
    );
    let database = Connection::open(&fixture.database_path).unwrap();
    install_archive_rejection(&database, "reject_cancel_fenced_session_archive");
    let tracker = Arc::new(FileEffectTracker::default());
    let settlement_entered = Arc::new(Barrier::new(2));
    let settlement_release = Arc::new(Barrier::new(2));
    let entered_for_hook = Arc::clone(&settlement_entered);
    let release_for_hook = Arc::clone(&settlement_release);
    fixture
        .registry
        .set_before_synchronous_settlement_hook(Arc::new(move |_| {
            entered_for_hook.wait();
            release_for_hook.wait();
        }));

    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let registry = fixture.registry.clone();
    let workspace = fixture.workspace.path().to_path_buf();
    let conversation_id = fixture.conversation_id.clone();
    let assistant_message_id = fixture.assistant_message_id.clone();
    let run_id = fixture.run_id.clone();
    let call_id = fixture.call_id.clone();
    let tracker_for_start = Arc::clone(&tracker);
    let command = "printf cancel-fenced-terminal";
    let start = thread::spawn(move || {
        start_owned_session_with_tracker(
            &registry,
            &workspace,
            &conversation_id,
            &assistant_message_id,
            &run_id,
            &call_id,
            command,
            Some(notifications),
            &tracker_for_start,
        )
    });

    settlement_entered.wait();
    let cancel_started = Arc::new(Barrier::new(2));
    let cancel_started_in_thread = Arc::clone(&cancel_started);
    let cancel_registry = fixture.registry.clone();
    let cancel_run_id = fixture.run_id.clone();
    let (cancelled_tx, cancelled_rx) = std::sync::mpsc::channel();
    let cancel = thread::spawn(move || {
        cancel_started_in_thread.wait();
        cancelled_tx
            .send(cancel_registry.cancel_pre_handoff_for_run(&cancel_run_id))
            .unwrap();
    });
    cancel_started.wait();
    assert!(
        cancelled_rx
            .recv_timeout(Duration::from_millis(50))
            .is_err(),
        "cancellation must wait until the recoverable receipt is complete"
    );
    settlement_release.wait();
    assert_eq!(cancelled_rx.recv_timeout(TEST_WAIT).unwrap(), 1);
    cancel.join().unwrap();

    let launch = start.join().unwrap().unwrap();
    let AgentCommandSessionLaunch::Running {
        snapshot,
        tool_result,
        mut handoff_guard,
    } = launch
    else {
        panic!("settlement failure must expose its stable recovery receipt before cancellation")
    };
    let running = tool_result.result.as_ref().expect("running receipt");
    assert_eq!(running["status"], "running");
    assert_eq!(running["sessionId"], snapshot.session_id);
    assert_eq!(
        running["continueWith"]["args"]["sessionId"],
        snapshot.session_id
    );

    database
        .execute_batch("DROP TRIGGER reject_cancel_fenced_session_archive;")
        .unwrap();
    assert_eq!(
        handoff_guard.commit(|| false, || Ok(())).unwrap(),
        AgentCommandHandoffOutcome::CancelledBeforeCommit
    );
    let terminal = handoff_guard.abort_before_handoff().unwrap();
    assert_eq!(terminal.snapshot.session_id.as_str(), snapshot.session_id);
    let record = wait_for_terminal_record(
        &fixture.storage,
        &fixture.conversation_id,
        &snapshot.session_id,
    );
    assert_eq!(record.snapshot.status, AgentCommandSessionStatus::Exited);

    let archive_count: u64 = database
        .query_row(
            "SELECT COUNT(*) FROM conversation_history_blobs WHERE conversation_id = ?1",
            rusqlite::params![&fixture.conversation_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(archive_count, 1);
    let terminal_events = std::iter::from_fn(|| receiver.try_recv().ok())
        .filter(|event| {
            matches!(
                event["params"]["type"].as_str(),
                Some("command_exited" | "command_interrupted")
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(terminal_events.len(), 1);
    assert_eq!(fixture.registry.retained_live_session_count(), 0);
    assert_eq!(fixture.registry.retained_core_session_count(), 0);
    assert_eq!(fixture.registry.retained_admission_count(), 0);
    assert_eq!(fixture.registry.settlement_scheduler_stats().1, 0);
    assert!(tracker
        .active_run_ids_for_conversation(&fixture.conversation_id)
        .is_empty());
    assert!(tracker
        .unsettled_effect_ids_for_conversation(&fixture.conversation_id)
        .is_empty());
}

#[test]
fn cancelled_pending_handoff_guard_drop_retries_one_terminal_settlement() {
    let fixture = RunningFixture::new("cancelled-handoff-drop-retry");
    let database = Connection::open(&fixture.database_path).unwrap();
    install_archive_rejection(&database, "reject_cancelled_handoff_drop_archive");
    let tracker = Arc::new(FileEffectTracker::default());
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let launch = start_owned_session_with_tracker(
        &fixture.registry,
        fixture.workspace.path(),
        &fixture.conversation_id,
        &fixture.assistant_message_id,
        &fixture.run_id,
        &fixture.call_id,
        "sleep 30",
        Some(notifications),
        &tracker,
    )
    .unwrap();
    let AgentCommandSessionLaunch::Running {
        snapshot,
        handoff_guard,
        ..
    } = launch
    else {
        panic!("the pending handoff command must still be running")
    };

    // The explicit cancellation wins first and records Pending -> Aborted. A caller panic then
    // drops the still-armed guard. The Aborted drop path must open the settlement fence; otherwise
    // the terminal process, FileEffect guard, and admission lease remain retained forever.
    assert_eq!(
        fixture.registry.cancel_pre_handoff_for_run(&fixture.run_id),
        1
    );
    drop(handoff_guard);
    wait_for_settlement_attempts(&fixture.registry, 1);
    let pending = fixture
        .storage
        .load_agent_command_session(&fixture.conversation_id, &snapshot.session_id)
        .unwrap()
        .expect("failed terminal commit keeps the durable running row");
    assert_eq!(pending.snapshot.status, AgentCommandSessionStatus::Running);
    assert_eq!(fixture.registry.retained_live_session_count(), 1);
    assert_eq!(fixture.registry.retained_admission_count(), 1);
    assert_eq!(
        tracker.active_run_ids_for_conversation(&fixture.conversation_id),
        vec![fixture.run_id.clone()]
    );

    database
        .execute_batch("DROP TRIGGER reject_cancelled_handoff_drop_archive;")
        .unwrap();
    let record = wait_for_terminal_record(
        &fixture.storage,
        &fixture.conversation_id,
        &snapshot.session_id,
    );
    assert_eq!(
        record.snapshot.status,
        AgentCommandSessionStatus::Interrupted
    );
    let archive_count: u64 = database
        .query_row(
            "SELECT COUNT(*) FROM conversation_history_blobs WHERE conversation_id = ?1",
            rusqlite::params![&fixture.conversation_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(archive_count, 1);
    let terminal_events = std::iter::from_fn(|| receiver.try_recv().ok())
        .filter(|event| {
            matches!(
                event["params"]["type"].as_str(),
                Some("command_exited" | "command_interrupted")
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(terminal_events.len(), 1);
    assert_eq!(fixture.registry.retained_live_session_count(), 0);
    assert_eq!(fixture.registry.retained_core_session_count(), 0);
    assert_eq!(fixture.registry.retained_admission_count(), 0);
    assert_eq!(fixture.registry.settlement_scheduler_stats().1, 0);
    assert!(tracker
        .active_run_ids_for_conversation(&fixture.conversation_id)
        .is_empty());
    assert!(tracker
        .unsettled_effect_ids_for_conversation(&fixture.conversation_id)
        .is_empty());
}

#[test]
fn durable_start_failure_archive_retry_returns_one_cleared_failed_execution() {
    let fixture = RunningFixture::new("durable-failure-archive-retry");
    let database = Connection::open(&fixture.database_path).unwrap();
    install_session_running_rejection(&database, "reject_retry_session_running");
    install_archive_rejection(&database, "reject_durable_failure_archive_once");
    let tracker = Arc::new(FileEffectTracker::default());
    let registry = fixture.registry.clone();
    let workspace = fixture.workspace.path().to_path_buf();
    let conversation_id = fixture.conversation_id.clone();
    let assistant_message_id = fixture.assistant_message_id.clone();
    let run_id = fixture.run_id.clone();
    let call_id = fixture.call_id.clone();
    let tracker_for_start = Arc::clone(&tracker);
    let start = thread::spawn(move || {
        start_owned_session_with_tracker(
            &registry,
            &workspace,
            &conversation_id,
            &assistant_message_id,
            &run_id,
            &call_id,
            r#"awk 'BEGIN { for (i = 0; i < 20000; i++) printf "durable-failure-%05d\n", i }'; sleep 30"#,
            None,
            &tracker_for_start,
        )
    });

    wait_for_settlement_attempts(&fixture.registry, 1);
    database
        .execute_batch("DROP TRIGGER reject_durable_failure_archive_once;")
        .unwrap();
    let launch = start
        .join()
        .expect("durable failure archive retry")
        .unwrap();
    let AgentCommandSessionLaunch::Exited(terminal) = launch else {
        panic!("durable-start failure must return one terminal execution after settlement")
    };
    assert!(matches!(
        terminal.snapshot.state,
        mycopilot_core::command::CommandSessionState::Failed
    ));
    assert!(!terminal.execution.stdout_spool.is_present());
    assert!(!terminal.execution.stderr_spool.is_present());
    let ordinary_tool_result =
        mycopilot_core::command::command_tool_result(&fixture.call_id, &terminal.execution);
    assert!(ordinary_tool_result.exact_archive_file.is_none());
    let record = wait_for_terminal_record(
        &fixture.storage,
        &fixture.conversation_id,
        terminal.snapshot.session_id.as_str(),
    );
    assert_eq!(record.snapshot.status, AgentCommandSessionStatus::Failed);
    assert!(record.snapshot.archive_ref.is_some());
    let archive_count: u64 = database
        .query_row(
            "SELECT COUNT(*) FROM conversation_history_blobs WHERE conversation_id = ?1",
            rusqlite::params![&fixture.conversation_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(archive_count, 1);
    assert_eq!(fixture.registry.retained_admission_count(), 0);
    assert_eq!(fixture.registry.settlement_scheduler_stats().1, 0);
    assert!(tracker
        .active_run_ids_for_conversation(&fixture.conversation_id)
        .is_empty());
    assert!(tracker
        .unsettled_effect_ids_for_conversation(&fixture.conversation_id)
        .is_empty());
}

#[test]
fn short_large_output_is_archived_before_terminal_visibility_without_a_second_exact_body() {
    let fixture = RunningFixture::new_with_initial_yield("short-exact-cut", Duration::from_secs(2));
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let launch = start_owned_session(
        &fixture.registry,
        fixture.workspace.path(),
        &fixture.conversation_id,
        &fixture.assistant_message_id,
        &fixture.run_id,
        &fixture.call_id,
        r#"awk 'BEGIN { for (i = 0; i < 20000; i++) printf "archive-line-%05d\n", i }'"#,
        Some(notifications),
    )
    .unwrap();
    let AgentCommandSessionLaunch::Exited(terminal) = launch else {
        panic!("large but short command must exit inside the initial yield")
    };
    assert_eq!(terminal.snapshot.exit_code, Some(0));
    assert!(terminal.execution.stdout_truncated);
    assert!(!terminal.execution.stdout_spool.is_present());
    assert!(!terminal.execution.stderr_spool.is_present());

    // `start` must not expose the terminal outcome until the immutable archive and authoritative
    // Session/Trace cut have both committed.
    let record = fixture
        .storage
        .load_agent_command_session(
            &fixture.conversation_id,
            terminal.snapshot.session_id.as_str(),
        )
        .unwrap()
        .expect("short command Session receipt");
    let archive_ref = record
        .snapshot
        .archive_ref
        .as_deref()
        .expect("short terminal cut includes its Exact History ref");
    let descriptor = fixture
        .storage
        .find_conversation_history_archive_by_ref(&fixture.conversation_id, archive_ref)
        .unwrap()
        .expect("short command Exact History descriptor");
    let tail_start = descriptor.total_chars.saturating_sub(8 * 1024);
    let tail = fixture
        .storage
        .read_conversation_history_archive_page(
            &fixture.conversation_id,
            archive_ref,
            ConversationHistoryArchivePageUnit::Char,
            tail_start,
            8 * 1024,
        )
        .unwrap()
        .expect("short command Exact History tail");
    assert!(tail.content.contains("archive-line-19999"));

    // The ordinary run_command audit can retain its bounded preview, but the already-consumed
    // complete-output spools must not materialize a second full-body archive sidecar.
    let ordinary_tool_result =
        mycopilot_core::command::command_tool_result(&fixture.call_id, &terminal.execution);
    assert!(ordinary_tool_result.exact_archive_file.is_none());
    let ordinary_body = ordinary_tool_result
        .result
        .as_ref()
        .expect("terminal run_command body");
    let history_open = ordinary_body["historyOpen"]
        .as_str()
        .expect("terminal result carries the authoritative history route");
    assert!(history_open.starts_with("hist_v1_"));
    assert_eq!(ordinary_body["continueWith"]["args"]["open"], history_open);
    let routed_page = fixture
        .storage
        .read_conversation_history_archive_page_from_open(
            &fixture.conversation_id,
            history_open,
            u64::MAX,
        )
        .unwrap()
        .expect("conversation_history-compatible route resolves");
    assert!(routed_page.content.contains("archive-line-19999"));
    assert_eq!(routed_page.descriptor.archive_ref, archive_ref);
    let connection = Connection::open(&fixture.database_path).unwrap();
    let archive_count: u64 = connection
        .query_row(
            "SELECT COUNT(*) FROM conversation_history_blobs WHERE conversation_id = ?1",
            rusqlite::params![&fixture.conversation_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(archive_count, 1);

    let events = std::iter::from_fn(|| receiver.try_recv().ok()).collect::<Vec<_>>();
    assert!(events.iter().any(|event| {
        event["params"]["type"] == "command_exited"
            && event["params"]["sessionId"] == terminal.snapshot.session_id.as_str()
    }));
}

#[test]
fn fast_four_megabyte_output_hands_off_until_the_host_queue_reaches_terminal() {
    let fixture = RunningFixture::new_with_initial_yield(
        "fast-four-megabyte-handoff",
        Duration::from_secs(2),
    );
    let persisted_chunks = Arc::new(AtomicUsize::new(0));
    let observed_chunks = Arc::clone(&persisted_chunks);
    fixture
        .registry
        .set_before_output_persistence_hook(Arc::new(move |_, _| {
            observed_chunks.fetch_add(1, Ordering::Relaxed);
            // A 4+ MiB producer fills the bounded lifecycle queue. Slowing each durable append
            // makes Core's terminal callback observable before the Host writer reaches the final
            // queue item, reproducing the real pdftotext settlement cut deterministically.
            thread::sleep(Duration::from_millis(21));
        }));
    let command = r#"awk 'BEGIN { for (i = 0; i < 100000; i++) printf "pdftotext-page-%06d-abcdefghijklmnopqrstuvwxyz\n", i }'"#;

    let launch = start_owned_session(
        &fixture.registry,
        fixture.workspace.path(),
        &fixture.conversation_id,
        &fixture.assistant_message_id,
        &fixture.run_id,
        &fixture.call_id,
        command,
        None,
    )
    .expect("a delayed Host terminal must remain a recoverable launch");
    let AgentCommandSessionLaunch::Running {
        snapshot,
        tool_result,
        mut handoff_guard,
    } = launch
    else {
        panic!("the delayed Host terminal must not be projected as an empty exited result")
    };
    let running = tool_result
        .result
        .as_ref()
        .expect("recoverable running ToolResult");
    assert_eq!(running["status"], "running");
    assert_eq!(running["sessionId"], snapshot.session_id);
    assert_eq!(running["continueWith"]["tool"], "command_session");
    assert_eq!(
        running["continueWith"]["args"],
        json!({
            "sessionId": snapshot.session_id,
            "action": "wait"
        })
    );
    assert!(
        running["outputTruncated"].as_bool().unwrap(),
        "a saturated Host lifecycle queue must be explicit"
    );

    adopt_owned_session(
        &fixture.storage,
        &fixture.conversation_id,
        &fixture.assistant_message_id,
        &fixture.run_id,
        &fixture.call_id,
        &snapshot,
        command,
        &mut handoff_guard,
    );
    let record = wait_for_terminal_record_with_timeout(
        &fixture.storage,
        &fixture.conversation_id,
        &snapshot.session_id,
        Duration::from_secs(15),
    );
    assert_eq!(record.snapshot.status, AgentCommandSessionStatus::Exited);
    assert_eq!(record.snapshot.exit_code, Some(0));
    assert!(persisted_chunks.load(Ordering::Relaxed) > 1);

    let request = AgentCommandSessionExecutionRequest {
        conversation_id: fixture.conversation_id.clone(),
        run_id: "run-fast-four-megabyte-terminal-read".to_string(),
        call_id: "call-fast-four-megabyte-terminal-read".to_string(),
        session_id: snapshot.session_id.clone(),
        action: AgentCommandSessionAction::Poll,
        wait_ms: 0,
        max_output_bytes: AGENT_COMMAND_SESSION_MODEL_OUTPUT_BYTES,
    };
    let terminal = fixture
        .registry
        .execute_command_session(request.clone(), observation_control())
        .unwrap();
    assert_eq!(terminal.status, AgentCommandSessionStatus::Exited);
    let history_open = terminal
        .history_open
        .as_deref()
        .expect("terminal wait exposes the exact-output recovery route");
    let tail = fixture
        .storage
        .read_conversation_history_archive_page_from_open(
            &fixture.conversation_id,
            history_open,
            u64::MAX,
        )
        .unwrap()
        .expect("the exact output remains readable through conversation_history");
    assert!(tail
        .content
        .contains("pdftotext-page-099999-abcdefghijklmnopqrstuvwxyz"));

    let connection = Connection::open(&fixture.database_path).unwrap();
    let archive_count: u64 = connection
        .query_row(
            "SELECT COUNT(*) FROM conversation_history_blobs WHERE conversation_id = ?1",
            rusqlite::params![&fixture.conversation_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(archive_count, 1);

    let restarted = AgentCommandSessionRegistry::with_manager(
        Arc::clone(&fixture.storage),
        CommandSessionManager::new(test_manager_config()).unwrap(),
        INITIAL_YIELD,
    );
    let replay = restarted
        .execute_command_session(request, observation_control())
        .unwrap();
    assert_eq!(replay, terminal);
    assert_eq!(replay.history_open.as_deref(), Some(history_open));
}

#[test]
fn caller_panic_drops_pending_handoff_and_settles_its_process_and_file_effect() {
    let fixture = RunningFixture::new("handoff-panic");
    let tracker = Arc::new(FileEffectTracker::default());
    let captured_session_id = Arc::new(Mutex::new(None::<String>));
    let unwind_session_id = Arc::clone(&captured_session_id);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let launch = start_owned_session_with_tracker(
            &fixture.registry,
            fixture.workspace.path(),
            &fixture.conversation_id,
            &fixture.assistant_message_id,
            &fixture.run_id,
            &fixture.call_id,
            "sleep 5",
            None,
            &tracker,
        )
        .unwrap();
        let AgentCommandSessionLaunch::Running {
            snapshot,
            handoff_guard: _handoff_guard,
            ..
        } = launch
        else {
            panic!("panic-boundary command must still be running")
        };
        *unwind_session_id
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(snapshot.session_id.clone());
        panic!("injected caller panic after the running receipt")
    }));
    assert!(result.is_err());
    let session_id = captured_session_id
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone()
        .expect("running Session identity survives caller unwind");
    let record = wait_for_terminal_record(&fixture.storage, &fixture.conversation_id, &session_id);
    assert_eq!(
        record.snapshot.status,
        AgentCommandSessionStatus::Interrupted
    );
    // The durable terminal row is committed immediately before the worker releases its
    // in-memory admission lease; observe both boundaries instead of racing the latter.
    wait_for_retained_admission_count(&fixture.registry, 0);
    assert!(tracker
        .active_run_ids_for_conversation(&fixture.conversation_id)
        .is_empty());
    assert!(tracker
        .unsettled_effect_ids_for_conversation(&fixture.conversation_id)
        .is_empty());
}

#[test]
fn initial_yield_cancellation_cannot_settle_before_file_effect_ownership_is_installed() {
    let fixture =
        RunningFixture::new_with_initial_yield("initial-yield-cancel", Duration::from_secs(2));
    let tracker = Arc::new(FileEffectTracker::default());
    let hook_entered = Arc::new(Barrier::new(2));
    let hook_release = Arc::new(Barrier::new(2));
    let hook_entered_for_start = Arc::clone(&hook_entered);
    let hook_release_for_start = Arc::clone(&hook_release);
    fixture
        .registry
        .set_before_file_effect_install_hook(Arc::new(move |_| {
            hook_entered_for_start.wait();
            hook_release_for_start.wait();
        }));

    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let registry = fixture.registry.clone();
    let workspace = fixture.workspace.path().to_path_buf();
    let conversation_id = fixture.conversation_id.clone();
    let assistant_message_id = fixture.assistant_message_id.clone();
    let run_id = fixture.run_id.clone();
    let call_id = fixture.call_id.clone();
    let tracker_for_start = Arc::clone(&tracker);
    let start = thread::spawn(move || {
        start_owned_session_with_tracker(
            &registry,
            &workspace,
            &conversation_id,
            &assistant_message_id,
            &run_id,
            &call_id,
            "sleep 5",
            Some(notifications),
            &tracker_for_start,
        )
    });

    let event_deadline = Instant::now() + TEST_WAIT;
    let session_id = loop {
        if let Ok(event) = receiver.try_recv() {
            if event["params"]["type"] == "command_started" {
                break event["params"]["sessionId"]
                    .as_str()
                    .expect("command_started Session identity")
                    .to_string();
            }
        }
        assert!(
            Instant::now() < event_deadline,
            "initial-yield command did not publish command_started"
        );
        thread::sleep(Duration::from_millis(5));
    };
    assert_eq!(
        fixture.registry.cancel_pre_handoff_for_run(&fixture.run_id),
        1
    );

    // The hook is reached only after the Core manager has observed the interrupted terminal result,
    // while the caller-owned FileEffectGuard is deliberately still outside Host state.
    hook_entered.wait();
    assert!(fixture
        .registry
        .wait_for_live_terminal(&session_id, TEST_WAIT));
    thread::sleep(Duration::from_millis(100));
    let pre_install = fixture
        .storage
        .load_agent_command_session(&fixture.conversation_id, &session_id)
        .unwrap()
        .expect("pre-install Session row remains owned by the Host");
    assert_eq!(
        pre_install.snapshot.status,
        AgentCommandSessionStatus::Running
    );
    assert!(pre_install.snapshot.archive_ref.is_none());
    assert_eq!(fixture.registry.retained_admission_count(), 1);
    assert_eq!(
        tracker.active_run_ids_for_conversation(&fixture.conversation_id),
        vec![fixture.run_id.clone()]
    );

    hook_release.wait();
    let launch = start.join().expect("command start thread").unwrap();
    let AgentCommandSessionLaunch::Exited(terminal) = launch else {
        panic!("initial-yield cancellation must return a terminal launch outcome")
    };
    assert!(matches!(
        terminal.snapshot.state,
        mycopilot_core::command::CommandSessionState::Interrupted
    ));
    let record = wait_for_terminal_record(&fixture.storage, &fixture.conversation_id, &session_id);
    assert_eq!(
        record.snapshot.status,
        AgentCommandSessionStatus::Interrupted
    );
    assert!(record.snapshot.archive_ref.is_some());
    assert_eq!(fixture.registry.retained_admission_count(), 0);
    assert!(tracker
        .active_run_ids_for_conversation(&fixture.conversation_id)
        .is_empty());
    assert!(tracker
        .unsettled_effect_ids_for_conversation(&fixture.conversation_id)
        .is_empty());
}

#[test]
fn absolute_handoff_deadline_reclaims_a_noisy_session_while_guard_is_alive() {
    let fixture = RunningFixture::new_with_handoff_timeout(
        "handoff-deadline",
        Duration::from_millis(10),
        Duration::from_millis(500),
    );
    let tracker = Arc::new(FileEffectTracker::default());
    let launch = start_owned_session_with_tracker(
        &fixture.registry,
        fixture.workspace.path(),
        &fixture.conversation_id,
        &fixture.assistant_message_id,
        &fixture.run_id,
        &fixture.call_id,
        r#"awk 'BEGIN { for (i = 0; i < 12000; i++) print "deadline-noise" }'; sleep 5"#,
        None,
        &tracker,
    )
    .unwrap();
    let AgentCommandSessionLaunch::Running {
        snapshot,
        handoff_guard,
        ..
    } = launch
    else {
        panic!("deadline-bound command must still be running")
    };

    // Keep the caller token alive and deliberately omit commit/drop. The Host's absolute deadline
    // must still win over an always-readable output queue.
    let record = wait_for_terminal_record(
        &fixture.storage,
        &fixture.conversation_id,
        &snapshot.session_id,
    );
    std::hint::black_box(&handoff_guard);
    assert_eq!(
        record.snapshot.status,
        AgentCommandSessionStatus::Interrupted
    );
    assert_eq!(fixture.registry.retained_admission_count(), 0);
    assert!(tracker
        .active_run_ids_for_conversation(&fixture.conversation_id)
        .is_empty());
    assert!(tracker
        .unsettled_effect_ids_for_conversation(&fixture.conversation_id)
        .is_empty());
    drop(handoff_guard);
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

    let terminal = fixture.abort(&snapshot.session_id);
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

    fixture.abort(&snapshot.session_id);
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
fn model_wait_ignores_noisy_output_until_its_deadline() {
    let fixture = RunningFixture::new("quiet-noisy-wait");
    let command = "printf 'tick-0\\n'; sleep 0.08; printf 'tick-1\\n'; sleep 0.08; \
                   printf 'tick-2\\n'; sleep 0.08; printf 'tick-3\\n'; sleep 0.08; \
                   printf 'tick-4\\n'; sleep 0.40";
    let (snapshot, _) = fixture.start(command, None);
    fixture.adopt(&snapshot, command);

    let started = Instant::now();
    let output = fixture.poll(&snapshot.session_id, Duration::from_millis(300));
    let elapsed = started.elapsed();

    assert!(
        elapsed >= Duration::from_millis(220),
        "ordinary output returned the model wait early after {elapsed:?}"
    );
    assert_eq!(output.status, AgentCommandSessionStatus::Running);
    assert!(output.output.contains("tick-"));
    wait_for_terminal_record(
        &fixture.storage,
        &fixture.conversation_id,
        &snapshot.session_id,
    );
}

#[test]
fn cancelling_model_observation_does_not_terminate_handed_off_process() {
    let fixture = RunningFixture::new("cancel-observation-keeps-process");
    let command = "sleep 0.05; printf before; sleep 0.80; printf after";
    let (snapshot, _) = fixture.start(command, None);
    fixture.adopt(&snapshot, command);
    let cursor_before = fixture
        .storage
        .load_agent_command_session(&fixture.conversation_id, &snapshot.session_id)
        .unwrap()
        .unwrap()
        .model_read_sequence;

    let cancellation = mycopilot_core::AgentCancellationToken::new();
    let worker_cancellation = cancellation.clone();
    let registry = fixture.registry.clone();
    let conversation_id = fixture.conversation_id.clone();
    let session_id = snapshot.session_id.clone();
    let worker = thread::spawn(move || {
        registry.execute_command_session(
            AgentCommandSessionExecutionRequest {
                conversation_id,
                run_id: "run-cancel-observation".to_string(),
                call_id: "call-cancel-observation".to_string(),
                session_id,
                action: AgentCommandSessionAction::Poll,
                wait_ms: 300_000,
                max_output_bytes: AGENT_COMMAND_SESSION_MODEL_OUTPUT_BYTES,
            },
            AgentCommandSessionExecutionControl::new(worker_cancellation, None),
        )
    });
    thread::sleep(Duration::from_millis(100));
    let cancelled_at = Instant::now();
    cancellation.cancel();
    let error = worker.join().unwrap().unwrap_err();
    assert!(error.is_cancelled());
    assert!(
        cancelled_at.elapsed() < Duration::from_millis(500),
        "cancelling a model observation must release it promptly"
    );

    let live = fixture
        .registry
        .get(AgentCommandSessionGetInput {
            conversation_id: fixture.conversation_id.clone(),
            session_id: snapshot.session_id.clone(),
            after_sequence: None,
            max_bytes: None,
        })
        .unwrap();
    assert_eq!(live.session.status, AgentCommandSessionStatus::Running);
    let durable_after_cancel = fixture
        .storage
        .load_agent_command_session(&fixture.conversation_id, &snapshot.session_id)
        .unwrap()
        .unwrap();
    assert_eq!(durable_after_cancel.model_read_sequence, cursor_before);
    assert!(fixture
        .storage
        .load_agent_command_session_model_read(&AgentCommandSessionModelReadRequest {
            conversation_id: &fixture.conversation_id,
            session_id: &snapshot.session_id,
            run_id: "run-cancel-observation",
            call_id: "call-cancel-observation",
            action: AgentCommandSessionAction::Poll,
            max_output_bytes: AGENT_COMMAND_SESSION_MODEL_OUTPUT_BYTES,
            host_output_truncated: false,
            created_at: 0,
        })
        .unwrap()
        .is_none());

    let terminal = wait_for_terminal_record(
        &fixture.storage,
        &fixture.conversation_id,
        &snapshot.session_id,
    );
    assert_eq!(terminal.snapshot.status, AgentCommandSessionStatus::Exited);
    let final_read = fixture.poll(&snapshot.session_id, Duration::ZERO);
    assert!(final_read.output.contains("before"));
    assert!(final_read.output.contains("after"));
}

#[test]
fn explicit_run_cancel_releases_model_wait_and_interrupts_its_handed_off_process() {
    let fixture = RunningFixture::new("explicit-cancel-during-model-wait");
    let mut service = AgentService::new_authorized_for_test(Arc::clone(&fixture.storage));
    service.command_sessions = fixture.registry.clone();
    let command = "sleep 5";
    let tracker = Arc::new(FileEffectTracker::default());
    let launch = start_owned_session_with_tracker(
        &fixture.registry,
        fixture.workspace.path(),
        &fixture.conversation_id,
        &fixture.assistant_message_id,
        &fixture.run_id,
        &fixture.call_id,
        command,
        None,
        &tracker,
    )
    .unwrap();
    let (snapshot, mut handoff_guard) = match launch {
        AgentCommandSessionLaunch::Running {
            snapshot,
            handoff_guard,
            ..
        } => (*snapshot, handoff_guard),
        AgentCommandSessionLaunch::Exited(terminal) => panic!(
            "command must still be running, got {:?}",
            terminal.snapshot.state
        ),
    };
    adopt_owned_session(
        &fixture.storage,
        &fixture.conversation_id,
        &fixture.assistant_message_id,
        &fixture.run_id,
        &fixture.call_id,
        &snapshot,
        command,
        &mut handoff_guard,
    );
    let cursor_before = fixture
        .storage
        .load_agent_command_session(&fixture.conversation_id, &snapshot.session_id)
        .unwrap()
        .unwrap()
        .model_read_sequence;

    let cancellation = mycopilot_core::AgentCancellationToken::new();
    service.register_cancellation(&fixture.run_id, cancellation.clone());
    let request = AgentCommandSessionExecutionRequest {
        conversation_id: fixture.conversation_id.clone(),
        run_id: fixture.run_id.clone(),
        call_id: "call-explicit-cancel-wait".to_string(),
        session_id: snapshot.session_id.clone(),
        action: AgentCommandSessionAction::Poll,
        wait_ms: 300_000,
        max_output_bytes: AGENT_COMMAND_SESSION_MODEL_OUTPUT_BYTES,
    };
    let worker_request = request.clone();
    let worker_registry = fixture.registry.clone();
    let worker_cancellation = cancellation.clone();
    let worker = thread::spawn(move || {
        worker_registry.execute_command_session(
            worker_request,
            AgentCommandSessionExecutionControl::new(worker_cancellation, None),
        )
    });
    thread::sleep(Duration::from_millis(100));

    let cancelled_at = Instant::now();
    assert!(service.cancel_run(&fixture.run_id));
    let error = worker.join().unwrap().unwrap_err();
    assert!(error.is_cancelled());
    assert!(
        cancelled_at.elapsed() < Duration::from_millis(500),
        "input-composer stop must release a quiet model wait promptly"
    );

    let terminal = wait_for_terminal_record(
        &fixture.storage,
        &fixture.conversation_id,
        &snapshot.session_id,
    );
    assert_eq!(
        terminal.snapshot.status,
        AgentCommandSessionStatus::Interrupted
    );
    assert_eq!(terminal.model_read_sequence, cursor_before);
    assert!(fixture
        .storage
        .load_agent_command_session_model_read(&AgentCommandSessionModelReadRequest {
            conversation_id: &request.conversation_id,
            session_id: &request.session_id,
            run_id: &request.run_id,
            call_id: &request.call_id,
            action: request.action,
            max_output_bytes: request.max_output_bytes,
            host_output_truncated: false,
            created_at: 0,
        })
        .unwrap()
        .is_none());
    assert!(tracker
        .active_run_ids_for_conversation(&fixture.conversation_id)
        .is_empty());
    assert_eq!(fixture.registry.retained_admission_count(), 0);
    assert_eq!(fixture.registry.retained_live_session_count(), 0);
    service.unregister_cancellation_if_current(&fixture.run_id, &cancellation);
}

#[test]
fn trusted_child_interrupt_terminates_its_handed_off_command_session() {
    let fixture = RunningFixture::new("child-interrupt-adopted-session");
    let mut service = AgentService::new_authorized_for_test(Arc::clone(&fixture.storage));
    service.command_sessions = fixture.registry.clone();
    let command = "sleep 5";
    let (snapshot, _) = fixture.start(command, None);
    fixture.adopt(&snapshot, command);

    assert!(service
        .interrupt_agent_wake_run(&fixture.run_id)
        .unwrap()
        .any_effect());
    let terminal = wait_for_terminal_record(
        &fixture.storage,
        &fixture.conversation_id,
        &snapshot.session_id,
    );
    assert_eq!(
        terminal.snapshot.status,
        AgentCommandSessionStatus::Interrupted
    );
    assert_eq!(fixture.registry.retained_live_session_count(), 0);
}

#[test]
fn queued_guidance_releases_a_noisy_running_wait_without_stopping_process() {
    let fixture = RunningFixture::new("guidance-releases-wait");
    let command = "printf 'noise-0\\n'; sleep 0.10; printf 'noise-1\\n'; sleep 0.10; \
                   printf 'noise-2\\n'; sleep 0.10; printf 'noise-3\\n'; sleep 0.10; \
                   printf 'noise-4\\n'; sleep 0.10; printf 'noise-5\\n'; sleep 0.40";
    let (snapshot, _) = fixture.start(command, None);
    fixture.adopt(&snapshot, command);

    let guidance = mycopilot_core::AgentSteerInputQueue::new();
    let registry = fixture.registry.clone();
    let conversation_id = fixture.conversation_id.clone();
    let session_id = snapshot.session_id.clone();
    let worker_guidance = guidance.clone();
    let worker = thread::spawn(move || {
        registry.execute_command_session(
            AgentCommandSessionExecutionRequest {
                conversation_id,
                run_id: "run-guidance-observation".to_string(),
                call_id: "call-guidance-observation".to_string(),
                session_id,
                action: AgentCommandSessionAction::Poll,
                wait_ms: 300_000,
                max_output_bytes: AGENT_COMMAND_SESSION_MODEL_OUTPUT_BYTES,
            },
            AgentCommandSessionExecutionControl::new(
                mycopilot_core::AgentCancellationToken::new(),
                Some(worker_guidance),
            ),
        )
    });
    thread::sleep(Duration::from_millis(120));
    let queued_at = Instant::now();
    guidance
        .enqueue(mycopilot_core::AgentSteerInput {
            guidance_id: "guidance-command-wait".to_string(),
            client_message_id: "client-command-wait".to_string(),
            content: "Apply this updated constraint.".to_string(),
            attachments: Vec::new(),
            attachment_library: None,
            created_at: 10,
        })
        .unwrap();
    let output = worker.join().unwrap().unwrap();

    assert!(
        queued_at.elapsed() < Duration::from_millis(500),
        "queued guidance must release the current model wait promptly"
    );
    assert_eq!(output.status, AgentCommandSessionStatus::Running);
    assert!(guidance.pending_len() > 0);
    let live = fixture
        .registry
        .get(AgentCommandSessionGetInput {
            conversation_id: fixture.conversation_id.clone(),
            session_id: snapshot.session_id.clone(),
            after_sequence: None,
            max_bytes: None,
        })
        .unwrap();
    assert_eq!(live.session.status, AgentCommandSessionStatus::Running);
    wait_for_terminal_record(
        &fixture.storage,
        &fixture.conversation_id,
        &snapshot.session_id,
    );
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
                    .execute_command_session(
                        AgentCommandSessionExecutionRequest {
                            conversation_id,
                            run_id: format!("run-concurrent-poll-{index}"),
                            call_id: format!("call-concurrent-poll-{index}"),
                            session_id,
                            action: AgentCommandSessionAction::Poll,
                            wait_ms: 0,
                            max_output_bytes: AGENT_COMMAND_SESSION_MODEL_OUTPUT_BYTES,
                        },
                        observation_control(),
                    )
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
                    .execute_command_session(
                        AgentCommandSessionExecutionRequest {
                            conversation_id,
                            run_id: format!("run-concurrent-terminal-poll-{index}"),
                            call_id: format!("call-concurrent-terminal-poll-{index}"),
                            session_id,
                            action: AgentCommandSessionAction::Poll,
                            wait_ms: 0,
                            max_output_bytes: AGENT_COMMAND_SESSION_MODEL_OUTPUT_BYTES,
                        },
                        observation_control(),
                    )
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
        .execute_command_session(request.clone(), observation_control())
        .unwrap();
    assert_eq!(before_restart.output, "crash-safe-marker");

    let restarted_registry = AgentCommandSessionRegistry::with_manager(
        Arc::clone(&fixture.storage),
        CommandSessionManager::new(test_manager_config()).unwrap(),
        INITIAL_YIELD,
    );
    let after_restart = restarted_registry
        .execute_command_session(request, observation_control())
        .unwrap();
    assert_eq!(after_restart, before_restart);

    let next_call = restarted_registry
        .execute_command_session(
            AgentCommandSessionExecutionRequest {
                conversation_id: fixture.conversation_id.clone(),
                run_id: "run-crash-retry".to_string(),
                call_id: "call-after-crash-retry".to_string(),
                session_id: snapshot.session_id,
                action: AgentCommandSessionAction::Poll,
                wait_ms: 0,
                max_output_bytes: AGENT_COMMAND_SESSION_MODEL_OUTPUT_BYTES,
            },
            observation_control(),
        )
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

    let model_access = fixture.registry.execute_command_session(
        AgentCommandSessionExecutionRequest {
            conversation_id: other_conversation.to_string(),
            run_id: "run-other".to_string(),
            call_id: "call-other".to_string(),
            session_id: snapshot.session_id.clone(),
            action: AgentCommandSessionAction::Poll,
            wait_ms: 0,
            max_output_bytes: AGENT_COMMAND_SESSION_MODEL_OUTPUT_BYTES,
        },
        observation_control(),
    );
    assert_eq!(
        model_access.unwrap_err().code(),
        Some("agent.command_session_not_found")
    );

    fixture.abort(&snapshot.session_id);
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
fn explicit_run_interrupt_is_scoped_by_conversation_and_origin_run() {
    let fixture = RunningFixture::new("explicit-run-interrupt-scope");
    let command = "sleep 5";
    let (snapshot, _) = fixture.start(command, None);
    fixture.adopt(&snapshot, command);

    assert_eq!(
        fixture
            .registry
            .interrupt_origin_run("conversation-other", &fixture.run_id),
        0
    );
    assert_eq!(
        fixture
            .registry
            .interrupt_origin_run(&fixture.conversation_id, "run-other"),
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

    assert_eq!(
        fixture
            .registry
            .interrupt_origin_run(&fixture.conversation_id, &fixture.run_id),
        1
    );
    let terminal = wait_for_terminal_record(
        &fixture.storage,
        &fixture.conversation_id,
        &snapshot.session_id,
    );
    assert_eq!(
        terminal.snapshot.status,
        AgentCommandSessionStatus::Interrupted
    );
}

#[test]
fn explicit_run_interrupt_closes_pending_handoff_before_receipt_commit() {
    let fixture = RunningFixture::new("explicit-stop-before-handoff");
    let (snapshot, _) = fixture.start("sleep 5", None);

    assert_eq!(
        fixture
            .registry
            .interrupt_origin_run(&fixture.conversation_id, &fixture.run_id),
        1
    );
    let mut handoff_guard = fixture
        .handoff_guards
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .remove(&snapshot.session_id)
        .expect("running fixture retains the handoff ownership token");
    let outcome = handoff_guard
        .commit(
            || false,
            || panic!("an explicit stop which wins the fence must forbid receipt persistence"),
        )
        .unwrap();
    assert_eq!(outcome, AgentCommandHandoffOutcome::CancelledBeforeCommit);
    let terminal = handoff_guard.abort_before_handoff().unwrap();
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
fn explicit_run_interrupt_stops_every_session_owned_by_the_turn() {
    let fixture = RunningFixture::new("explicit-stop-multiple");
    let command = "sleep 5";
    let (first, _) = fixture.start(command, None);
    fixture.adopt(&first, command);

    let second_assistant_message_id = format!("{}-second", fixture.assistant_message_id);
    let second_call_id = format!("{}-second", fixture.call_id);
    fixture
        .storage
        .upsert_chat_messages(
            &fixture.conversation_id,
            vec![ChatMessageRecord {
                human_interaction_response: None,
                id: second_assistant_message_id.clone(),
                role: "assistant".to_string(),
                content: String::new(),
                created_at: 2,
                status: Some("pending".to_string()),
                attachments: Vec::new(),
                agent_run_json: None,
                ui_state_json: None,
            }],
            1,
        )
        .unwrap();
    let second_launch = start_owned_session(
        &fixture.registry,
        fixture.workspace.path(),
        &fixture.conversation_id,
        &second_assistant_message_id,
        &fixture.run_id,
        &second_call_id,
        command,
        None,
    )
    .unwrap();
    let (second, mut second_handoff_guard) = match second_launch {
        AgentCommandSessionLaunch::Running {
            snapshot,
            handoff_guard,
            ..
        } => (*snapshot, handoff_guard),
        AgentCommandSessionLaunch::Exited(terminal) => panic!(
            "second command must still be running, got {:?}",
            terminal.snapshot.state
        ),
    };
    assert_eq!(
        second_handoff_guard.commit(|| false, || Ok(())).unwrap(),
        AgentCommandHandoffOutcome::Adopted
    );

    assert_eq!(
        fixture
            .registry
            .interrupt_origin_run(&fixture.conversation_id, &fixture.run_id),
        2
    );
    for session_id in [&first.session_id, &second.session_id] {
        let terminal =
            wait_for_terminal_record(&fixture.storage, &fixture.conversation_id, session_id);
        assert_eq!(
            terminal.snapshot.status,
            AgentCommandSessionStatus::Interrupted
        );
    }
}

#[test]
fn pre_handoff_cancellation_terminates_the_process() {
    let fixture = RunningFixture::new("pending-cancel");
    let (snapshot, _) = fixture.start("sleep 5", None);

    assert_eq!(
        fixture.registry.cancel_pre_handoff_for_run(&fixture.run_id),
        1
    );
    let terminal = fixture.abort(&snapshot.session_id);
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
fn pre_handoff_abort_uses_host_terminal_after_core_cleanup() {
    let fixture = RunningFixture::new("pending-cancel-core-cleanup");
    let (snapshot, _) = fixture.start("sleep 5", None);
    assert_eq!(
        fixture.registry.cancel_pre_handoff_for_run(&fixture.run_id),
        1
    );
    assert!(fixture
        .registry
        .wait_for_live_terminal(&snapshot.session_id, TEST_WAIT));
    let deadline = Instant::now() + TEST_WAIT;
    while !fixture
        .registry
        .remove_terminal_core_session_for_test(&snapshot.session_id)
    {
        assert!(
            Instant::now() < deadline,
            "Core did not release its active process lease"
        );
        thread::yield_now();
    }
    assert_eq!(fixture.registry.retained_core_session_count(), 0);
    let terminal = fixture.abort(&snapshot.session_id);
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
    assert!(record.snapshot.archive_ref.is_some());
    wait_for_retained_admission_count(&fixture.registry, 0);
    assert_eq!(fixture.registry.retained_live_session_count(), 0);
}

#[test]
fn model_interrupt_terminates_an_adopted_session_and_publishes_one_terminal_event() {
    let fixture = RunningFixture::new("model-interrupt");
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let command = "sleep 5";
    let (snapshot, _) = fixture.start(command, Some(notifications));
    fixture.adopt(&snapshot, command);

    let request = AgentCommandSessionExecutionRequest {
        conversation_id: fixture.conversation_id.clone(),
        run_id: "run-model-interrupt".to_string(),
        call_id: "call-model-interrupt".to_string(),
        session_id: snapshot.session_id.clone(),
        action: AgentCommandSessionAction::Interrupt,
        wait_ms: 2_000,
        max_output_bytes: AGENT_COMMAND_SESSION_MODEL_OUTPUT_BYTES,
    };
    let output = fixture
        .registry
        .execute_command_session(request.clone(), observation_control())
        .unwrap();
    assert_eq!(output.status, AgentCommandSessionStatus::Interrupted);
    assert_eq!(output.exit_code, None);
    Connection::open(&fixture.database_path)
        .unwrap()
        .execute(
            "DELETE FROM agent_command_session_output_chunks WHERE session_id = ?1",
            [&snapshot.session_id],
        )
        .unwrap();
    assert_eq!(
        fixture
            .registry
            .execute_command_session(request, observation_control())
            .expect("an interrupt ToolCall retry replays its receipt without signaling twice"),
        output
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

    let events = std::iter::from_fn(|| receiver.try_recv().ok()).collect::<Vec<_>>();
    assert_eq!(
        events
            .iter()
            .filter(|event| event["params"]["type"] == "command_interrupted")
            .count(),
        1
    );
    assert!(events.iter().any(|event| {
        event["params"]["type"] == "command_interrupted"
            && event["params"]["sessionId"] == snapshot.session_id
            && event["params"]["callId"] == fixture.call_id
    }));
}

#[test]
fn interrupt_is_sticky_when_the_first_receipt_write_fails() {
    let fixture = RunningFixture::new("interrupt-receipt-failure");
    let marker_path = fixture.workspace.path().join("interrupt-count.txt");
    let script_path = fixture.workspace.path().join("interrupt-target.sh");
    std::fs::write(
        &script_path,
        format!(
            "#!/bin/sh\nmarker='{}'\ntrap 'printf x >> \"$marker\"' INT\nprintf ready\n\
             while true; do sleep 1; done\n",
            marker_path.to_string_lossy().replace('\'', "'\\''")
        ),
    )
    .unwrap();
    let command = format!(
        "sh '{}'",
        script_path.to_string_lossy().replace('\'', "'\\''")
    );
    let (snapshot, _) = fixture.start(&command, None);
    fixture.adopt(&snapshot, &command);

    let ready_deadline = Instant::now() + TEST_WAIT;
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
            .any(|chunk| chunk.output.contains("ready"))
        {
            break;
        }
        assert!(
            Instant::now() < ready_deadline,
            "command did not become ready"
        );
        thread::sleep(Duration::from_millis(10));
    }

    let failure_connection = Connection::open(&fixture.database_path).unwrap();
    failure_connection
        .execute_batch(
            "CREATE TRIGGER fail_test_command_session_receipt_insert
             BEFORE INSERT ON agent_command_session_model_read_receipts
             BEGIN
                 SELECT RAISE(ABORT, 'injected receipt persistence failure');
             END;",
        )
        .unwrap();
    let request = AgentCommandSessionExecutionRequest {
        conversation_id: fixture.conversation_id.clone(),
        run_id: "run-interrupt-receipt-failure".to_string(),
        call_id: "call-interrupt-receipt-failure".to_string(),
        session_id: snapshot.session_id.clone(),
        action: AgentCommandSessionAction::Interrupt,
        wait_ms: 0,
        max_output_bytes: AGENT_COMMAND_SESSION_MODEL_OUTPUT_BYTES,
    };
    assert!(fixture
        .registry
        .execute_command_session(request.clone(), observation_control())
        .is_err());
    failure_connection
        .execute_batch("DROP TRIGGER fail_test_command_session_receipt_insert;")
        .unwrap();

    let retry = fixture
        .registry
        .execute_command_session(request.clone(), observation_control())
        .expect("retry should observe the sticky interrupt and persist its receipt");
    assert_eq!(
        fixture
            .registry
            .execute_command_session(request, observation_control())
            .expect("the committed interrupt receipt must replay exactly"),
        retry
    );
    let terminal = wait_for_terminal_record(
        &fixture.storage,
        &fixture.conversation_id,
        &snapshot.session_id,
    );
    assert_eq!(
        terminal.snapshot.status,
        AgentCommandSessionStatus::Interrupted
    );
    assert_eq!(std::fs::read_to_string(marker_path).unwrap(), "x");
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

struct CrossDomainAgentWaitStore {
    polls: AtomicUsize,
    replies: StdMutex<VecDeque<Option<AgentWaitReadySnapshot>>>,
}

impl CrossDomainAgentWaitStore {
    fn pending() -> Self {
        Self {
            polls: AtomicUsize::new(0),
            replies: StdMutex::new(VecDeque::from([None, None])),
        }
    }

    fn publish(&self, ready: AgentWaitReadySnapshot) {
        self.replies
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push_back(Some(ready));
    }
}

impl crate::application::agent_wait::AgentWaitStore for CrossDomainAgentWaitStore {
    fn poll_ready(
        &self,
        _input: &mycopilot_core::PollAgentWaitInput,
    ) -> Result<Option<AgentWaitReadySnapshot>, mycopilot_core::AgentGraphError> {
        self.polls.fetch_add(1, Ordering::SeqCst);
        Ok(self
            .replies
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .pop_front()
            .flatten())
    }
}

fn cross_domain_ready() -> AgentWaitReadySnapshot {
    AgentWaitReadySnapshot {
        receipt: AgentModelBatchReceiptRecord {
            receipt_id: "agent-wait-receipt".into(),
            agent_id: "caller".into(),
            conversation_id: "agent-conversation".into(),
            run_id: "agent-run".into(),
            assistant_message_id: "agent-assistant".into(),
            model_batch_index: 2,
            sampling_bound_at: Some(1),
            created_at: 1,
            updated_at: 1,
        },
        source_receipt_id: None,
        targets: vec![AgentWaitTargetSnapshot {
            target_agent_id: "target".into(),
            target_task_name: "review".into(),
            messages: Vec::new(),
            target_status_version: 2,
            latest_wake_sequence: Some(1),
            latest_wake_status_revision: Some(2),
            latest_wake_status: Some(mycopilot_core::AgentWakeStatus::Completed),
            display_status: AgentDisplayStatus::LatestCompleted,
        }],
        model_projection: AgentWaitModelProjection::PrecommittedToolResult,
    }
}

fn persist_wait_only_command(
    fixture: &RunningFixture,
    session_id: &str,
    call_id: &str,
    started_at: u64,
    with_output: bool,
) {
    fixture
        .storage
        .create_agent_command_session(&AgentCommandSessionCreate {
            snapshot: AgentCommandSessionSnapshot {
                schema_version: AGENT_COMMAND_SESSION_SCHEMA_VERSION,
                session_id: session_id.into(),
                conversation_id: fixture.conversation_id.clone(),
                assistant_message_id: fixture.assistant_message_id.clone(),
                origin_run_id: fixture.run_id.clone(),
                call_id: call_id.into(),
                project_id: None,
                command: "deterministic fake command".into(),
                cwd: fixture.workspace.path().to_string_lossy().into_owned(),
                command_digest: ZERO_DIGEST.into(),
                status: AgentCommandSessionStatus::Starting,
                started_at,
                ended_at: None,
                exit_code: None,
                latest_sequence: 0,
                output_truncated: false,
                outputs: Vec::new(),
                artifact_observation: None,
                archive_ref: None,
            },
            authorization_source: CommandAuthorizationSource::ExplicitUser,
            approval_provenance: json!({"decision":"approved"}),
            permission_provenance: json!({"mode":"test"}),
            created_at: i64::try_from(started_at).unwrap(),
        })
        .unwrap();
    fixture
        .storage
        .mark_agent_command_session_running(
            &fixture.conversation_id,
            session_id,
            i64::try_from(started_at + 1).unwrap(),
        )
        .unwrap();
    if with_output {
        fixture
            .storage
            .append_agent_command_session_output(&AgentCommandSessionOutputAppend {
                conversation_id: &fixture.conversation_id,
                session_id,
                chunks: &[AgentCommandSessionOutputChunk {
                    sequence: 1,
                    stream: AgentCommandOutputStream::Stdout,
                    output: "durable fake output".into(),
                }],
                latest_sequence: 1,
                transcript_truncated: false,
                output_capture_truncated: false,
                updated_at: i64::try_from(started_at + 2).unwrap(),
            })
            .unwrap();
    }
    fixture.registry.install_wait_only_test_session(
        session_id,
        &fixture.conversation_id,
        &fixture.assistant_message_id,
        &fixture.run_id,
        call_id,
    );
}

fn settle_wait_only_command(
    fixture: &RunningFixture,
    session_id: &str,
    latest_sequence: u64,
    committed_at: i64,
) {
    fixture
        .storage
        .settle_agent_command_session(&AgentCommandSessionTerminalUpdate {
            conversation_id: &fixture.conversation_id,
            session_id,
            status: AgentCommandSessionStatus::Exited,
            ended_at: u64::try_from(committed_at).unwrap(),
            exit_code: Some(0),
            latest_sequence,
            transcript_truncated: false,
            output_capture_truncated: false,
            archive_ref: None,
            terminal_reason: None,
            published_outputs: &[],
            artifact_observation: None,
            committed_at,
        })
        .unwrap();
    fixture.registry.signal_wait_only_test_terminal(session_id);
}

#[tokio::test]
async fn real_registry_command_wait_and_agent_wait_are_isolated_without_shell() {
    use crate::application::agent_wait::{
        AgentWaitKernel, AgentWaitNotifications, AgentWaitOutcome,
    };

    let fixture = Arc::new(RunningFixture::new("dual-wait-registry"));
    let notifications = AgentWaitNotifications::default();
    const FIRST: &str = "cmd_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    persist_wait_only_command(&fixture, FIRST, "call-command-first", 100, true);
    let command_wait = {
        let fixture = Arc::clone(&fixture);
        tokio::task::spawn_blocking(move || {
            fixture
                .registry
                .wait_for_live_terminal(FIRST, Duration::from_secs(2))
        })
    };
    let store = Arc::new(CrossDomainAgentWaitStore::pending());
    let kernel = AgentWaitKernel::test_with_store(store.clone(), notifications.clone());
    let agent_wait = tokio::spawn(async move {
        kernel
            .wait(
                mycopilot_core::PollAgentWaitInput {
                    caller_agent_id: "caller".into(),
                    conversation_id: "agent-conversation".into(),
                    run_id: "agent-run".into(),
                    assistant_message_id: "agent-assistant".into(),
                    model_batch_index: 2,
                    target_agent_ids: vec!["target".into()],
                    maximum_messages: 1,
                },
                Duration::from_secs(2),
                mycopilot_core::AgentCancellationToken::new(),
                mycopilot_core::AgentCancellationToken::new(),
                None,
            )
            .await
            .unwrap()
    });
    while store.polls.load(Ordering::SeqCst) < 2 {
        tokio::task::yield_now().await;
    }
    settle_wait_only_command(&fixture, FIRST, 1, 104);
    assert!(command_wait.await.unwrap());
    assert!(
        !agent_wait.is_finished(),
        "command terminal must not wake Agent wait"
    );
    let read = fixture
        .storage
        .read_or_create_agent_command_session_model_read(&AgentCommandSessionModelReadRequest {
            conversation_id: &fixture.conversation_id,
            session_id: FIRST,
            run_id: "command-read-run",
            call_id: "command-read-call",
            action: AgentCommandSessionAction::Poll,
            max_output_bytes: 1_024,
            host_output_truncated: false,
            created_at: 105,
        })
        .unwrap()
        .unwrap();
    assert_eq!(read.receipt.status, AgentCommandSessionStatus::Exited);
    assert_eq!(read.chunks[0].output, "durable fake output");
    store.publish(cross_domain_ready());
    notifications.notify_caller("caller");
    assert!(matches!(
        agent_wait.await.unwrap(),
        AgentWaitOutcome::Ready(_)
    ));

    const SECOND: &str = "cmd_bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    persist_wait_only_command(&fixture, SECOND, "call-command-second", 110, false);
    let second_command_wait = {
        let fixture = Arc::clone(&fixture);
        tokio::task::spawn_blocking(move || {
            fixture
                .registry
                .wait_for_live_terminal(SECOND, Duration::from_secs(2))
        })
    };
    let second_store = Arc::new(CrossDomainAgentWaitStore::pending());
    let second_kernel =
        AgentWaitKernel::test_with_store(second_store.clone(), notifications.clone());
    let second_agent_wait = tokio::spawn(async move {
        second_kernel
            .wait(
                mycopilot_core::PollAgentWaitInput {
                    caller_agent_id: "caller".into(),
                    conversation_id: "agent-conversation".into(),
                    run_id: "agent-run-second".into(),
                    assistant_message_id: "agent-assistant-second".into(),
                    model_batch_index: 2,
                    target_agent_ids: vec!["target".into()],
                    maximum_messages: 1,
                },
                Duration::from_secs(2),
                mycopilot_core::AgentCancellationToken::new(),
                mycopilot_core::AgentCancellationToken::new(),
                None,
            )
            .await
            .unwrap()
    });
    while second_store.polls.load(Ordering::SeqCst) < 2 {
        tokio::task::yield_now().await;
    }
    second_store.publish(cross_domain_ready());
    notifications.notify_caller("caller");
    assert!(matches!(
        second_agent_wait.await.unwrap(),
        AgentWaitOutcome::Ready(_)
    ));
    assert!(
        !second_command_wait.is_finished(),
        "Agent result must not wake the production Registry command waiter"
    );
    settle_wait_only_command(&fixture, SECOND, 0, 114);
    assert!(second_command_wait.await.unwrap());
    let second_read = fixture
        .storage
        .read_or_create_agent_command_session_model_read(&AgentCommandSessionModelReadRequest {
            conversation_id: &fixture.conversation_id,
            session_id: SECOND,
            run_id: "command-read-run-second",
            call_id: "command-read-call-second",
            action: AgentCommandSessionAction::Poll,
            max_output_bytes: 1_024,
            host_output_truncated: false,
            created_at: 115,
        })
        .unwrap()
        .unwrap();
    assert_eq!(
        second_read.receipt.status,
        AgentCommandSessionStatus::Exited
    );
    assert!(second_read.chunks.is_empty());
}

#[test]
fn background_terminal_model_read_exposes_deterministic_full_history_route() {
    let fixture = RunningFixture::new("terminal-model-history-route");
    let command = r#"sleep 0.08; awk 'BEGIN { for (i = 0; i < 20000; i++) printf "background-line-%05d\n", i }'"#;
    let (snapshot, initial_result) = fixture.start(command, None);
    assert_eq!(initial_result.result.as_ref().unwrap()["status"], "running");

    let running = fixture.poll(&snapshot.session_id, Duration::ZERO);
    assert_eq!(running.status, AgentCommandSessionStatus::Running);
    assert!(running.history_open.is_none());
    fixture.adopt(&snapshot, command);
    wait_for_terminal_record(
        &fixture.storage,
        &fixture.conversation_id,
        &snapshot.session_id,
    );

    let request = AgentCommandSessionExecutionRequest {
        conversation_id: fixture.conversation_id.clone(),
        run_id: "run-terminal-history-route".to_string(),
        call_id: "call-terminal-history-route".to_string(),
        session_id: snapshot.session_id.clone(),
        action: AgentCommandSessionAction::Poll,
        wait_ms: 0,
        max_output_bytes: AGENT_COMMAND_SESSION_MODEL_OUTPUT_BYTES,
    };
    let terminal = fixture
        .registry
        .execute_command_session(request.clone(), observation_control())
        .unwrap();
    assert_eq!(terminal.status, AgentCommandSessionStatus::Exited);
    assert!(terminal.output.len() <= AGENT_COMMAND_SESSION_MODEL_OUTPUT_BYTES);
    let history_open = terminal
        .history_open
        .as_deref()
        .expect("terminal command_session result exposes opaque full-history route");
    assert!(history_open.starts_with("hist_v1_"));
    let full = fixture
        .storage
        .read_conversation_history_archive_page_from_open(
            &fixture.conversation_id,
            history_open,
            u64::MAX,
        )
        .unwrap()
        .expect("conversation_history route resolves after background completion");
    assert!(full.content.contains("background-line-19999"));

    let restarted = AgentCommandSessionRegistry::with_manager(
        Arc::clone(&fixture.storage),
        CommandSessionManager::new(test_manager_config()).unwrap(),
        INITIAL_YIELD,
    );
    let replay = restarted
        .execute_command_session(request, observation_control())
        .unwrap();
    assert_eq!(replay, terminal);
    let replay_open = replay.history_open.as_deref().unwrap();
    let replayed_full = fixture
        .storage
        .read_conversation_history_archive_page_from_open(
            &fixture.conversation_id,
            replay_open,
            u64::MAX,
        )
        .unwrap()
        .expect("same opaque route remains readable after registry restart");
    assert!(replayed_full.content.contains("background-line-19999"));
}

#[cfg(target_os = "macos")]
#[test]
#[ignore = "manual Aspen PDF smoke; requires MYCOPILOT_ARTIFACT_RUNTIME_TEST_COMPONENT and MYCOPILOT_ASPEN_PDF"]
fn aspen_pdf_runs_a_five_step_managed_session_workflow() {
    let component = std::env::var_os("MYCOPILOT_ARTIFACT_RUNTIME_TEST_COMPONENT")
        .expect("set MYCOPILOT_ARTIFACT_RUNTIME_TEST_COMPONENT");
    let source_aspen = std::path::PathBuf::from(
        std::env::var_os("MYCOPILOT_ASPEN_PDF").expect("set MYCOPILOT_ASPEN_PDF"),
    )
    .canonicalize()
    .expect("canonical Aspen PDF path");
    let file_name = source_aspen
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .expect("Aspen PDF file name is UTF-8")
        .to_string();
    let bytes = std::fs::read(&source_aspen).expect("read Aspen PDF fixture");
    let fixture =
        RunningFixture::new_with_initial_yield("aspen-five-step-smoke", Duration::from_secs(120));
    let workspace = fixture.workspace.path();
    std::fs::write(workspace.join(&file_name), &bytes)
        .expect("copy Aspen PDF into smoke workspace");
    let input = mycopilot_core::AgentFileInputBinding {
        schema_version: mycopilot_core::AGENT_FILE_INPUT_BINDING_SCHEMA_VERSION,
        mount_path: "aspen.pdf".to_string(),
        source: mycopilot_core::AgentFileInputRef::Workspace { path: file_name },
        size_bytes: bytes.len() as u64,
        sha256: format!("{:x}", Sha256::digest(&bytes)),
    };
    let provider = Arc::new(
        ArtifactRuntimeProvider::discover(
            &ArtifactRuntimeDiscoveryOptions::new().with_configured_component_dir(component),
        )
        .expect("discover prepared Artifact Runtime"),
    );
    let binding = provider
        .resolve_profile(
            mycopilot_core::AgentCommandRuntimeProfile::Pdf,
            mycopilot_core::AgentCommandRuntimeKind::Python,
        )
        .expect("resolve frozen PDF Runtime profile");
    let file_inputs = mycopilot_core::file_input::AgentFileInputExecutionContext::default();

    let execute = |suffix: &str,
                   command: &str,
                   inputs: &[mycopilot_core::AgentFileInputBinding]|
     -> mycopilot_core::command::AgentCommandExecutionResult {
        let call_id = format!("{}-{suffix}", fixture.call_id);
        let request = AgentCommandRequest {
            id: call_id.clone(),
            command: command.to_string(),
            cwd: None,
            timeout_ms: None,
            approval_status: AgentApprovalStatus::Approved,
            risk_level: Some(AgentCommandRiskLevel::ReadOnly),
            reason: Some("Aspen managed PDF release smoke".to_string()),
            observe: None,
            inputs: inputs.to_vec(),
            runtime_binding: Some(Box::new(binding.clone())),
            managed_office_script: None,
        };
        let tracker = Arc::new(FileEffectTracker::default());
        let mut file_effect_guard = Some(tracker.register(
            None,
            Some(&fixture.conversation_id),
            &fixture.run_id,
            &call_id,
        ));
        file_effect_guard.as_mut().unwrap().mark_effects_started();
        let launch = fixture
            .registry
            .start(StartAgentCommandSession {
                owner: CommandSessionOwner {
                    conversation_id: fixture.conversation_id.clone(),
                    assistant_message_id: fixture.assistant_message_id.clone(),
                    origin_run_id: fixture.run_id.clone(),
                    call_id,
                    project_id: None,
                },
                workspace_root: Some(workspace),
                command: &request,
                permissions: AgentPermissions {
                    read: mycopilot_core::AgentReadPermission::WorkspaceOnly,
                    write: AgentWritePermission::WorkspaceOnly,
                    command: AgentCommandPermission::RequireApproval,
                    command_safety: AgentCommandSafetyPolicy::FullAccess,
                    ..AgentPermissions::default()
                },
                authorization_source: CommandAuthorizationSource::ExplicitUser,
                approval_provenance: json!({
                    "source": "explicit_user",
                    "status": "approved",
                    "manualSmoke": true
                }),
                artifact_runtime: Some(Arc::clone(&provider)),
                office_engine: None,
                file_inputs: Some(&file_inputs),
                notifications: None,
                cancellation_token: AgentCancellationToken::new(),
                cancel_probe: None,
                file_effect_guard: &mut file_effect_guard,
            })
            .expect("start managed Aspen command");
        let AgentCommandSessionLaunch::Exited(terminal) = launch else {
            panic!("Aspen smoke step exceeded the 120 second synchronous test yield")
        };
        terminal.execution
    };

    let metadata = execute(
        "metadata",
        "pdfinfo \"$MYCOPILOT_INPUT_ROOT/aspen.pdf\"",
        std::slice::from_ref(&input),
    );
    assert_eq!(metadata.exit_code, Some(0), "stderr={}", metadata.stderr);
    assert!(metadata.stdout.contains("Pages:"));

    let search = execute(
        "search",
        "pdftotext -layout \"$MYCOPILOT_INPUT_ROOT/aspen.pdf\" - | rg -n -i -C 4 --max-count 20 'steady-state unit operation|RStoic|RCSTR'",
        std::slice::from_ref(&input),
    );
    assert_eq!(search.exit_code, Some(0), "stderr={}", search.stderr);
    assert!(search.stdout.contains("RStoic"));
    assert!(search.stdout.contains("RCSTR"));

    let locate = execute(
        "locate",
        "pdftotext -f 306 -l 314 -layout \"$MYCOPILOT_INPUT_ROOT/aspen.pdf\" - | rg -n -i -C 3 --max-count 40 'Dupl|Flash2|Heater|Mixer|RStoic|RCSTR'",
        std::slice::from_ref(&input),
    );
    assert_eq!(
        locate.exit_code,
        Some(0),
        "stderr={} error={:?} runtime={:?} timedOut={} cancelled={}",
        locate.stderr,
        locate.error,
        locate.runtime,
        locate.timed_out,
        locate.cancelled
    );
    for expected in ["Dupl", "Flash2", "Heater", "Mixer", "RStoic", "RCSTR"] {
        assert!(
            locate.stdout.contains(expected),
            "missing {expected} from located Aspen unit models: {}",
            locate.stdout
        );
    }

    let render = execute(
        "render",
        "pdftoppm -f 307 -l 308 -r 96 -png \"$MYCOPILOT_INPUT_ROOT/aspen.pdf\" outputs/aspen-steady",
        std::slice::from_ref(&input),
    );
    assert_eq!(render.exit_code, Some(0), "stderr={}", render.stderr);
    assert_eq!(render.outputs.len(), 2);
    assert!(render.outputs.iter().all(|output| {
        serde_json::to_value(output.kind).unwrap() == json!("image")
            && output.read_path.starts_with("image-artifact://sha256/")
    }));

    for output in &render.outputs {
        let digest = output
            .read_path
            .strip_prefix("image-artifact://sha256/")
            .expect("published page exposes an image Artifact read path");
        let artifact = fixture
            .storage
            .read_authorized_managed_artifact(&format!("sha256:{digest}"), &fixture.conversation_id)
            .unwrap()
            .expect("published page remains readable by its conversation");
        assert_eq!(artifact.sha256, digest);
        assert_eq!(artifact.media_type, "image/png");
        assert!(!artifact.bytes.is_empty());
    }

    let private_path = fixture
        .workspace
        .path()
        .join("managed-command-runs")
        .to_string_lossy()
        .into_owned();
    for execution in [&metadata, &search, &locate, &render] {
        let projected = serde_json::to_string(&mycopilot_core::command::command_tool_result(
            "aspen-smoke",
            execution,
        ))
        .unwrap();
        assert!(!projected.contains(&private_path));
        assert!(!projected.contains("managed-command-runs"));
    }
    let sessions = fixture
        .storage
        .list_agent_command_sessions(&fixture.conversation_id, 16)
        .unwrap();
    assert_eq!(sessions.len(), 4);
    assert!(sessions
        .iter()
        .any(|record| record.snapshot.outputs.len() == 2));

    // Freeze one additional approval input, mutate it before the Host execution boundary, and
    // prove the same production Session path fails closed before spawning a sixth process.
    let changed_path = workspace.join("approval-race.pdf");
    std::fs::write(&changed_path, &bytes).unwrap();
    let changed_input = mycopilot_core::AgentFileInputBinding {
        schema_version: mycopilot_core::AGENT_FILE_INPUT_BINDING_SCHEMA_VERSION,
        mount_path: "approval-race.pdf".to_string(),
        source: mycopilot_core::AgentFileInputRef::Workspace {
            path: "approval-race.pdf".to_string(),
        },
        size_bytes: bytes.len() as u64,
        sha256: format!("{:x}", Sha256::digest(&bytes)),
    };
    std::fs::write(&changed_path, b"%PDF-1.7\nchanged after approval\n%%EOF\n").unwrap();
    let changed = execute(
        "changed-input",
        "pdfinfo \"$MYCOPILOT_INPUT_ROOT/approval-race.pdf\"",
        std::slice::from_ref(&changed_input),
    );
    assert_eq!(changed.exit_code, None);
    assert_eq!(
        changed
            .runtime
            .as_ref()
            .and_then(|runtime| runtime.error_code.as_deref()),
        Some("agent.fileInput.integrityMismatch")
    );
    assert_eq!(
        fixture
            .storage
            .list_agent_command_sessions(&fixture.conversation_id, 16)
            .unwrap()
            .len(),
        4,
        "changed approval input must fail before creating a process Session"
    );
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
        snapshot: _other_snapshot,
        handoff_guard: mut other_handoff_guard,
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

    other_handoff_guard.abort_before_handoff().unwrap();
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
        handoff_guard: mut healthy_handoff_guard,
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
        &fixture.storage,
        healthy_conversation,
        healthy_assistant,
        "run-settlement-healthy",
        "call-settlement-healthy",
        &healthy_snapshot,
        healthy_command,
        &mut healthy_handoff_guard,
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
    // A durable terminal commit precedes the worker's admission release. Await the same
    // ownership boundary used below before checking scheduler fairness under parallel load.
    wait_for_retained_admission_count(&fixture.registry, 1);
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
    // The durable terminal row commits immediately before the worker releases its admission
    // lease. Wait for that second, in-memory ownership boundary instead of racing it.
    wait_for_retained_admission_count(&fixture.registry, 0);
}

#[test]
fn host_shutdown_terminates_and_settles_an_adopted_session() {
    let fixture = RunningFixture::new("adopted-shutdown");
    let command = "sleep 5";
    let (snapshot, _) = fixture.start(command, None);
    fixture.adopt(&snapshot, command);

    assert!(fixture.registry.shutdown(Duration::from_secs(3)));
    let record = wait_for_terminal_record(
        &fixture.storage,
        &fixture.conversation_id,
        &snapshot.session_id,
    );
    assert_eq!(
        record.snapshot.status,
        AgentCommandSessionStatus::Interrupted
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

    let error = fixture.abort_result(&snapshot.session_id).unwrap_err();
    assert!(error.contains("完整输出由后台 Session 继续持有"));
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
        .save_project(ProjectRecord::with_primary_folder(
            project_id.to_string(),
            "Command session restart".to_string(),
            fixture.path().to_string_lossy().into_owned(),
            1,
        ))
        .unwrap();
    storage
        .save_conversation(ChatConversationRecord {
            id: conversation_id.to_string(),
            project_id: Some(project_id.to_string()),
            model_id: Some("test-model".to_string()),
            title: "Managed command session restart".to_string(),
            messages: vec![ChatMessageRecord {
                human_interaction_response: None,
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
                outputs: Vec::new(),
                artifact_observation: None,
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

    let service = AgentService::new_authorized_for_test(Arc::clone(&storage));
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

    let restarted_again = AgentService::new_authorized_for_test(Arc::clone(&storage));
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
