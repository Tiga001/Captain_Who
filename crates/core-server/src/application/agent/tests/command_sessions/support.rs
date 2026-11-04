//! Shared command-session fixtures and fault-injection helpers.

use super::*;

pub(super) const INITIAL_YIELD: Duration = Duration::from_millis(10);
pub(super) const TEST_WAIT: Duration = Duration::from_secs(5);
pub(super) const ZERO_DIGEST: &str =
    "sha256:0000000000000000000000000000000000000000000000000000000000000000";

pub(super) fn observation_control() -> AgentCommandSessionExecutionControl {
    AgentCommandSessionExecutionControl::new(mycopilot_core::AgentCancellationToken::new(), None)
}

pub(super) struct RunningFixture {
    pub(super) registry: AgentCommandSessionRegistry,
    pub(super) storage: Arc<StorageService>,
    pub(super) database_path: std::path::PathBuf,
    pub(super) workspace: tempfile::TempDir,
    pub(super) conversation_id: String,
    pub(super) assistant_message_id: String,
    pub(super) run_id: String,
    pub(super) call_id: String,
    poll_index: AtomicU64,
    pub(super) handoff_guards: Mutex<HashMap<String, AgentCommandSessionHandoffGuard>>,
}

impl RunningFixture {
    pub(super) fn new(name: &str) -> Self {
        Self::new_with_admission_limits(name, 32, 8)
    }

    pub(super) fn new_with_admission_limits(
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

    pub(super) fn new_with_initial_yield(name: &str, initial_yield: Duration) -> Self {
        Self::new_with_options(name, 32, 8, initial_yield)
    }

    pub(super) fn new_with_handoff_timeout(
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

    pub(super) fn new_with_options(
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

    pub(super) fn start(
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

    pub(super) fn adopt(&self, snapshot: &AgentCommandSessionSnapshot, command: &str) {
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

    pub(super) fn abort(&self, session_id: &str) -> mycopilot_core::command::CommandTerminalResult {
        self.abort_result(session_id).unwrap()
    }

    pub(super) fn abort_result(
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

    pub(super) fn poll(
        &self,
        session_id: &str,
        wait: Duration,
    ) -> AgentCommandSessionExecutionOutput {
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
pub(super) fn start_owned_session(
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
pub(super) fn start_owned_session_with_tracker(
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
pub(super) fn start_owned_session_with_request_and_tracker(
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
pub(super) fn adopt_owned_session(
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

pub(super) fn test_manager_config() -> CommandSessionManagerConfig {
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

pub(super) fn test_permissions() -> AgentPermissions {
    AgentPermissions {
        write: AgentWritePermission::WorkspaceOnly,
        command: AgentCommandPermission::AutoApprove,
        command_safety: AgentCommandSafetyPolicy::FullAccess,
        ..AgentPermissions::default()
    }
}

pub(super) fn approved_command(call_id: &str, command: &str) -> AgentCommandRequest {
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

pub(super) fn write_minimal_presentation(path: &Path) {
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

pub(super) fn seed_conversation(
    storage: &StorageService,
    conversation_id: &str,
    assistant_message_id: &str,
) {
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
                folder_references_json: None,
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

pub(super) fn completed_running_trace(
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

pub(super) fn wait_for_terminal_record(
    storage: &StorageService,
    conversation_id: &str,
    session_id: &str,
) -> mycopilot_core::storage::agent_command_session_repository::AgentCommandSessionRecord {
    wait_for_terminal_record_with_timeout(storage, conversation_id, session_id, TEST_WAIT)
}

pub(super) fn wait_for_terminal_record_with_timeout(
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

pub(super) fn poll_until(
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

pub(super) fn install_terminal_rejection(
    database: &Connection,
    trigger_name: &str,
    session_id: &str,
) {
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

pub(super) fn install_session_create_rejection(database: &Connection, trigger_name: &str) {
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

pub(super) fn install_session_running_rejection(database: &Connection, trigger_name: &str) {
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

pub(super) fn install_archive_rejection(database: &Connection, trigger_name: &str) {
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

pub(super) fn assert_session_create_failure_is_fail_closed(
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
    let (notifications, mut receiver) = crate::transport::outbound_channel();
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

pub(super) fn wait_for_settlement_attempts(
    registry: &AgentCommandSessionRegistry,
    minimum_attempts: usize,
) {
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

pub(super) fn wait_for_retained_admission_count(
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

pub(super) fn wait_for_terminal_cleanup(registry: &AgentCommandSessionRegistry) {
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
