use super::*;
use mycopilot_core::{
    AgentCommandSessionAction, AgentCommandSessionExecutionControl,
    AgentCommandSessionExecutionRequest, AgentCommandSessionExecutor,
    AGENT_COMMAND_SESSION_MODEL_OUTPUT_BYTES,
};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

fn terminal_test_checkpoint_item(
    call: &AgentToolCall,
) -> mycopilot_core::AgentContextCheckpointItem {
    mycopilot_core::AgentContextCheckpointItem {
        role: "assistant".to_string(),
        content: String::new(),
        images: Vec::new(),
        tool_call_id: None,
        tool_calls: vec![terminal_test_checkpoint_call(call)],
        is_error: false,
        sources: vec!["model_response".to_string()],
        scope: "run".to_string(),
        retention: "retained".to_string(),
        group: None,
        origin: None,
    }
}

fn terminal_test_checkpoint_call(call: &AgentToolCall) -> AgentContextCheckpointToolCall {
    AgentContextCheckpointToolCall {
        id: call.id.clone(),
        name: call.tool.clone(),
        args: call.args.clone(),
        provider_identity: AgentProviderToolCallIdentity {
            provider_tool_index: 0,
            provider_call_id: call.id.clone(),
            runtime_call_id: call.id.clone(),
        },
    }
}

fn terminal_test_model_context(call: &AgentToolCall) -> ConversationModelContextItem {
    ConversationModelContextItem {
        sequence: 0,
        ordinal: 0,
        role: "assistant".to_string(),
        content: String::new(),
        tool_call_id: None,
        tool_calls: vec![terminal_test_checkpoint_call(call)],
        is_error: false,
    }
}

fn run_current_command_session(
    workspace_root: &Path,
    request: &AgentCommandRequest,
    permissions: AgentPermissions,
    authorization_source: CommandAuthorizationSource,
) -> AgentCommandExecutionResult {
    let manager = mycopilot_core::command::CommandSessionManager::default();
    let outcome = manager
        .start_authorized_command(
            mycopilot_core::command::CommandSessionScopeId::new(format!(
                "test-command:{}",
                request.id
            ))
            .unwrap(),
            Some(workspace_root),
            request,
            permissions,
            authorization_source,
            mycopilot_core::command::CommandStartOptions::default(),
            None,
        )
        .unwrap();
    match outcome {
        mycopilot_core::command::CommandStartOutcome::Exited(terminal) => terminal.execution,
        mycopilot_core::command::CommandStartOutcome::Running(snapshot) => {
            manager
                .wait_terminal_result(&snapshot.session_id, Duration::from_secs(10))
                .unwrap()
                .expect("test command Session must become terminal")
                .execution
        }
    }
}

fn durable_auto_command_context(
    storage: &StorageService,
    mut input: AgentChatInput,
    run_id: &str,
    assistant_message_id: &str,
) -> AutoApprovedActionContext {
    let conversation_id = input
        .context
        .as_ref()
        .and_then(|context| context.conversation_id.clone())
        .expect("automatic command test input has a conversation identity");
    let project_id = input
        .context
        .as_ref()
        .and_then(|context| context.project_id.clone());
    if let Some(project_id) = project_id.as_ref() {
        let project_path = input
            .context
            .as_ref()
            .and_then(|context| context.workspace.as_ref())
            .and_then(|workspace| workspace.root_path.clone());
        storage
            .save_project(ProjectRecord {
                id: project_id.clone(),
                name: "Automatic command Session fixture".to_string(),
                path: project_path,
                created_at: 1,
                pinned_at: None,
            })
            .unwrap();
    }
    storage
        .save_conversation(ChatConversationRecord {
            id: conversation_id.clone(),
            project_id,
            model_id: Some("test-model".to_string()),
            title: "Automatic command Session fixture".to_string(),
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
    input
        .context
        .as_mut()
        .expect("automatic command test input has run context")
        .conversation_id = Some(conversation_id.clone());
    AutoApprovedActionContext::new(
        input,
        run_id.to_string(),
        Some(conversation_id),
        Some(assistant_message_id.to_string()),
        None,
    )
}

fn wait_for_terminal_auto_command_session(
    storage: &StorageService,
    conversation_id: &str,
) -> mycopilot_core::storage::agent_command_session_repository::AgentCommandSessionRecord {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(record) = storage
            .list_agent_command_sessions(conversation_id, 8)
            .unwrap()
            .into_iter()
            .find(|record| record.snapshot.status.is_terminal() && record.settled_at.is_some())
        {
            return record;
        }
        assert!(
            Instant::now() < deadline,
            "automatic command Session did not durably settle before the test deadline"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn command_session_archive_content(
    storage: &StorageService,
    conversation_id: &str,
    archive_ref: &str,
) -> String {
    let descriptor = storage
        .find_conversation_history_archive_by_ref(conversation_id, archive_ref)
        .unwrap()
        .expect("terminal command Session owns its Exact History archive");
    storage
        .read_conversation_history_archive_page(
            conversation_id,
            archive_ref,
            mycopilot_core::storage::conversation_history_archive_repository::ConversationHistoryArchivePageUnit::Char,
            0,
            descriptor.total_chars,
        )
        .unwrap()
        .expect("terminal command Session Exact History page")
        .content
}

#[test]
fn automatic_server_path_requires_explicit_approval_for_guarded_writes() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let service = AgentService::new(Arc::clone(&storage));
    let input = command_test_input(fixture.path());

    let automatic_request = command_request("automatic-command", "mkdir automatic-blocked");
    let automatic_result = service
        .execute_auto_approved_action(
            durable_auto_command_context(
                &storage,
                input.clone(),
                "run-automatic-command",
                "assistant-automatic-command",
            ),
            AgentProposedAction::Command {
                command: automatic_request,
            },
            AgentCancellationToken::new(),
        )
        .unwrap();

    assert!(!automatic_result.ok);
    assert!(automatic_result
        .error
        .as_deref()
        .is_some_and(|error| error.contains("command.explicit_approval_required")));
    assert!(!fixture.path().join("automatic-blocked").exists());
}

#[test]
fn automatic_command_streams_bounded_output_with_stable_call_identity() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let conversation_id = "conversation-live-command";
    let assistant_message_id = "assistant-live-command";
    storage
        .save_conversation(ChatConversationRecord {
            id: conversation_id.to_string(),
            project_id: None,
            model_id: Some("test-model".to_string()),
            title: "Live command events".to_string(),
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
    let service = AgentService::new(storage);
    let mut input = command_test_input(fixture.path());
    let context = input.context.as_mut().expect("command test context");
    context.conversation_id = Some(conversation_id.to_string());
    context.permissions.command_safety = mycopilot_core::AgentCommandSafetyPolicy::FullAccess;
    let run_id = "run-live-command";
    let call_id = "live-command";
    let command = command_request(
        call_id,
        "printf 'stdout-live\\n'; printf 'stderr-live\\n' >&2",
    );
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();

    let result = service
        .execute_auto_approved_action(
            AutoApprovedActionContext::new(
                input,
                run_id.to_string(),
                Some(conversation_id.to_string()),
                Some(assistant_message_id.to_string()),
                None,
            )
            .with_notifications(notifications),
            AgentProposedAction::Command { command },
            AgentCancellationToken::new(),
        )
        .unwrap();

    assert!(result.ok);
    let mut events = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        while let Ok(notification) = receiver.try_recv() {
            if notification["params"]["type"] == "command_output" {
                events.push(notification["params"].clone());
            }
        }
        if !events.is_empty() || Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(!events.is_empty());
    assert!(events.iter().all(|event| event["runId"] == run_id));
    assert!(events.iter().all(|event| event["callId"] == call_id));
    assert!(events
        .windows(2)
        .all(|pair| pair[0]["sequence"].as_u64() < pair[1]["sequence"].as_u64()));
    let output = events
        .iter()
        .filter_map(|event| event["output"].as_str())
        .collect::<String>();
    assert!(output.contains("stdout-live"));
    assert!(output.contains("stderr-live"));
}

#[test]
fn automatic_fast_large_output_returns_a_recoverable_session_instead_of_empty_exit() {
    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("fast-large-automatic.sqlite");
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    let conversation_id = "conversation-fast-large-automatic";
    let assistant_message_id = "assistant-fast-large-automatic";
    storage
        .save_conversation(ChatConversationRecord {
            id: conversation_id.to_string(),
            project_id: None,
            model_id: Some("test-model".to_string()),
            title: "Fast large automatic command".to_string(),
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
    let mut service = AgentService::new(Arc::clone(&storage));
    service.command_sessions = AgentCommandSessionRegistry::with_manager(
        Arc::clone(&storage),
        CommandSessionManager::default(),
        Duration::from_secs(2),
    );
    let persisted_chunks = Arc::new(AtomicUsize::new(0));
    let observed_chunks = Arc::clone(&persisted_chunks);
    service
        .command_sessions
        .set_before_output_persistence_hook(Arc::new(move |_, _| {
            observed_chunks.fetch_add(1, Ordering::Relaxed);
            thread::sleep(Duration::from_millis(21));
        }));

    let mut input = command_test_input(fixture.path());
    let context = input.context.as_mut().expect("command test context");
    context.conversation_id = Some(conversation_id.to_string());
    context.permissions.command_safety = mycopilot_core::AgentCommandSafetyPolicy::FullAccess;
    let run_id = "run-fast-large-automatic";
    let call_id = "command-fast-large-automatic";
    let command_text = r#"awk 'BEGIN { for (i = 0; i < 100000; i++) printf "pdftotext-page-%06d-abcdefghijklmnopqrstuvwxyz\n", i }'"#;
    let result = service
        .execute_auto_approved_action(
            AutoApprovedActionContext::new(
                input,
                run_id.to_string(),
                Some(conversation_id.to_string()),
                Some(assistant_message_id.to_string()),
                None,
            ),
            AgentProposedAction::Command {
                command: command_request(call_id, command_text),
            },
            AgentCancellationToken::new(),
        )
        .unwrap();

    assert!(result.ok);
    let running = result
        .result
        .as_ref()
        .expect("automatic command returns its durable running receipt");
    assert_eq!(running["status"], "running");
    let session_id = running["sessionId"]
        .as_str()
        .expect("the recovery receipt retains a stable Session identity");
    assert_eq!(running["continueWith"]["tool"], "command_session");
    assert_eq!(
        running["continueWith"]["args"],
        json!({ "sessionId": session_id, "action": "wait" })
    );
    assert!(running["outputTruncated"].as_bool().unwrap());
    assert!(persisted_chunks.load(Ordering::Relaxed) > 1);

    let persisted = storage
        .list_agent_tool_results_for_run(run_id, "run_command")
        .unwrap();
    assert_eq!(persisted.len(), 1);
    assert_eq!(persisted[0].result.as_ref().unwrap()["status"], "running");
    assert_eq!(
        persisted[0].result.as_ref().unwrap()["sessionId"],
        session_id
    );

    let deadline = Instant::now() + Duration::from_secs(15);
    let terminal_record = loop {
        let record = storage
            .load_agent_command_session(conversation_id, session_id)
            .unwrap()
            .expect("automatic command Session remains durable");
        if record.snapshot.status.is_terminal() && record.settled_at.is_some() {
            break record;
        }
        assert!(
            Instant::now() < deadline,
            "large automatic command remained permanently stuck: {:?}",
            record.snapshot.status
        );
        thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(
        terminal_record.snapshot.status,
        AgentCommandSessionStatus::Exited
    );
    assert_eq!(terminal_record.snapshot.exit_code, Some(0));

    let terminal = service
        .command_sessions
        .execute_command_session(
            AgentCommandSessionExecutionRequest {
                conversation_id: conversation_id.to_string(),
                run_id: "run-fast-large-terminal-read".to_string(),
                call_id: "call-fast-large-terminal-read".to_string(),
                session_id: session_id.to_string(),
                action: AgentCommandSessionAction::Poll,
                wait_ms: 0,
                max_output_bytes: AGENT_COMMAND_SESSION_MODEL_OUTPUT_BYTES,
            },
            AgentCommandSessionExecutionControl::new(AgentCancellationToken::new(), None),
        )
        .unwrap();
    let history_open = terminal
        .history_open
        .as_deref()
        .expect("terminal command Session exposes the exact output route");
    let tail = storage
        .read_conversation_history_archive_page_from_open(conversation_id, history_open, u64::MAX)
        .unwrap()
        .expect("conversation_history opens the automatic command archive");
    assert!(tail
        .content
        .contains("pdftotext-page-099999-abcdefghijklmnopqrstuvwxyz"));
}

#[test]
fn automatic_running_command_is_aborted_when_handoff_audit_is_definitely_uncommitted() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let conversation_id = "conversation-running-audit-failure";
    let assistant_message_id = "assistant-running-audit-failure";
    storage
        .save_conversation(ChatConversationRecord {
            id: conversation_id.to_string(),
            project_id: None,
            model_id: Some("test-model".to_string()),
            title: "Running command audit failure".to_string(),
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
    let mut service = AgentService::new(Arc::clone(&storage));
    service.command_sessions = AgentCommandSessionRegistry::with_manager(
        Arc::clone(&storage),
        CommandSessionManager::default(),
        Duration::from_millis(20),
    );
    let mut input = command_test_input(fixture.path());
    let context = input.context.as_mut().expect("command test context");
    context.conversation_id = Some(conversation_id.to_string());
    context.permissions.command_safety = mycopilot_core::AgentCommandSafetyPolicy::FullAccess;
    let run_id = "run-running-audit-failure";
    let call_id = "command-running-audit-failure";
    let mut command = command_request(call_id, "sleep 2");
    command.timeout_ms = Some(5_000);
    inject_auto_action_audit_failure(run_id, call_id, "completed");

    let result = service
        .execute_auto_approved_action(
            AutoApprovedActionContext::new(
                input,
                run_id.to_string(),
                Some(conversation_id.to_string()),
                Some(assistant_message_id.to_string()),
                None,
            ),
            AgentProposedAction::Command { command },
            AgentCancellationToken::new(),
        )
        .unwrap();

    assert!(!result.ok);
    assert!(result
        .error
        .as_deref()
        .is_some_and(|error| error.contains("running receipt was not durable")));
    let sessions = service
        .command_sessions
        .list(AgentCommandSessionListInput {
            conversation_id: conversation_id.to_string(),
        })
        .unwrap();
    assert_eq!(sessions.sessions.len(), 1);
    assert!(sessions.sessions[0].status.is_terminal());
    assert_ne!(
        sessions.sessions[0].status,
        AgentCommandSessionStatus::Running
    );
}

#[test]
fn explicit_run_cancel_interrupts_handed_off_command_after_active_control_is_retired() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let conversation_id = "conversation-running-post-commit";
    let assistant_message_id = "assistant-running-post-commit";
    storage
        .save_conversation(ChatConversationRecord {
            id: conversation_id.to_string(),
            project_id: None,
            model_id: Some("test-model".to_string()),
            title: "Running command post-commit".to_string(),
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
    let mut service = AgentService::new(Arc::clone(&storage));
    service.command_sessions = AgentCommandSessionRegistry::with_manager(
        Arc::clone(&storage),
        CommandSessionManager::default(),
        Duration::from_millis(20),
    );
    let mut input = command_test_input(fixture.path());
    let context = input.context.as_mut().expect("command test context");
    context.conversation_id = Some(conversation_id.to_string());
    context.permissions.command_safety = mycopilot_core::AgentCommandSafetyPolicy::FullAccess;
    let run_id = "run-running-post-commit";
    let call_id = "command-running-post-commit";
    let mut command = command_request(call_id, "sleep 2");
    command.timeout_ms = Some(5_000);
    let cancellation = AgentCancellationToken::new();
    service.register_cancellation(run_id, cancellation.clone());
    inject_auto_action_audit_post_commit_failure(run_id, call_id, "completed");

    let result = service
        .execute_auto_approved_action(
            AutoApprovedActionContext::new(
                input,
                run_id.to_string(),
                Some(conversation_id.to_string()),
                Some(assistant_message_id.to_string()),
                None,
            ),
            AgentProposedAction::Command { command },
            cancellation.clone(),
        )
        .unwrap();

    assert!(result.ok);
    assert_eq!(result.result.as_ref().unwrap()["status"], "running");
    let sessions = service
        .command_sessions
        .list(AgentCommandSessionListInput {
            conversation_id: conversation_id.to_string(),
        })
        .unwrap();
    assert_eq!(sessions.sessions.len(), 1);
    let session_id = sessions.sessions[0].session_id.clone();
    assert_eq!(
        sessions.sessions[0].status,
        AgentCommandSessionStatus::Running
    );
    assert!(service
        .active_runs
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(run_id)
        .is_none());

    assert!(service.cancel_run(run_id));
    assert!(cancellation.is_cancelled());
    let deadline = Instant::now() + Duration::from_secs(5);
    let terminal = loop {
        let sessions = service
            .command_sessions
            .list(AgentCommandSessionListInput {
                conversation_id: conversation_id.to_string(),
            })
            .unwrap();
        if let Some(session) = sessions
            .sessions
            .into_iter()
            .find(|session| session.session_id == session_id && session.status.is_terminal())
        {
            break session;
        }
        assert!(
            Instant::now() < deadline,
            "explicit Run cancellation did not settle the adopted Session"
        );
        std::thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(
        terminal.status,
        AgentCommandSessionStatus::Interrupted,
        "input-composer stop is a process-lifecycle boundary even after durable handoff"
    );
    service.unregister_cancellation_if_current(run_id, &cancellation);
}

#[test]
fn action_cancellation_fence_does_not_abort_a_sibling_command_session() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let conversation_id = "conversation-action-scoped-session-cancel";
    let assistant_message_id = "assistant-action-scoped-session-cancel";
    storage
        .save_conversation(ChatConversationRecord {
            id: conversation_id.to_string(),
            project_id: None,
            model_id: Some("test-model".to_string()),
            title: "Action-scoped Session cancellation".to_string(),
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
    let registry = AgentCommandSessionRegistry::with_manager(
        Arc::clone(&storage),
        CommandSessionManager::default(),
        Duration::from_millis(20),
    );
    let mut first = command_request("command-action-scoped-first", "sleep 2");
    first.approval_status = AgentApprovalStatus::Approved;
    let mut second = command_request("command-action-scoped-second", "sleep 2");
    second.approval_status = AgentApprovalStatus::Approved;
    let start = |command: &AgentCommandRequest| {
        let tracker = Arc::new(FileEffectTracker::default());
        let mut file_effect_guard = tracker.register(
            None,
            Some(conversation_id),
            "run-action-scoped-session-cancel",
            &command.id,
        );
        file_effect_guard.mark_effects_started();
        let mut file_effect_guard = Some(file_effect_guard);
        registry
            .start(StartAgentCommandSession {
                owner: CommandSessionOwner {
                    conversation_id: conversation_id.to_string(),
                    assistant_message_id: assistant_message_id.to_string(),
                    origin_run_id: "run-action-scoped-session-cancel".to_string(),
                    call_id: command.id.clone(),
                    project_id: None,
                },
                workspace_root: Some(fixture.path()),
                command,
                permissions: AgentPermissions {
                    command_safety: mycopilot_core::AgentCommandSafetyPolicy::FullAccess,
                    ..AgentPermissions::default()
                },
                authorization_source: CommandAuthorizationSource::Automatic,
                approval_provenance: serde_json::json!({"source": "test"}),
                artifact_runtime: None,
                office_engine: None,
                file_inputs: None,
                notifications: None,
                cancellation_token: AgentCancellationToken::new(),
                cancel_probe: None,
                file_effect_guard: &mut file_effect_guard,
            })
            .unwrap()
    };
    let (first_session_id, mut first_handoff_guard) = match start(&first) {
        AgentCommandSessionLaunch::Running {
            snapshot,
            handoff_guard,
            ..
        } => (snapshot.session_id, handoff_guard),
        AgentCommandSessionLaunch::Exited(_) => panic!("first command must still be running"),
    };
    let (second_session_id, mut second_handoff_guard) = match start(&second) {
        AgentCommandSessionLaunch::Running {
            snapshot,
            handoff_guard,
            ..
        } => (snapshot.session_id, handoff_guard),
        AgentCommandSessionLaunch::Exited(_) => panic!("second command must still be running"),
    };

    assert_eq!(
        registry.cancel_pre_handoff_for_action(
            "run-action-scoped-session-cancel",
            "command-action-scoped-first",
        ),
        1
    );
    first_handoff_guard.abort_before_handoff().unwrap();
    let sessions = registry
        .list(AgentCommandSessionListInput {
            conversation_id: conversation_id.to_string(),
        })
        .unwrap();
    let first_snapshot = sessions
        .sessions
        .iter()
        .find(|session| session.session_id == first_session_id)
        .unwrap();
    let second_snapshot = sessions
        .sessions
        .iter()
        .find(|session| session.session_id == second_session_id)
        .unwrap();
    assert!(first_snapshot.status.is_terminal());
    assert_eq!(second_snapshot.status, AgentCommandSessionStatus::Running);

    second_handoff_guard.abort_before_handoff().unwrap();
}

#[test]
fn nonzero_automatic_command_persists_office_artifacts_in_tool_result_audit() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let service = AgentService::new(storage.clone());
    let mut input = command_test_input(fixture.path());
    input
        .context
        .as_mut()
        .expect("command test context")
        .permissions
        .command_safety = mycopilot_core::AgentCommandSafetyPolicy::FullAccess;
    let run_id = "run-command-artifact-audit";
    let mut command = command_request(
        "command-artifact-audit",
        "printf audit-output > audit-output.csv; false",
    );
    command.observe = Some(mycopilot_core::AgentCommandArtifactObservationRequest {
        kinds: vec![mycopilot_core::AgentCommandArtifactObservationKind::Office],
        expected_outputs: vec!["audit-output.csv".to_string()],
        additional_roots: Vec::new(),
    });

    let result = service
        .execute_auto_approved_action(
            durable_auto_command_context(
                &storage,
                input,
                run_id,
                "assistant-command-artifact-audit",
            ),
            AgentProposedAction::Command { command },
            AgentCancellationToken::new(),
        )
        .unwrap();

    assert!(!result.ok);
    assert_eq!(result.result.as_ref().unwrap()["exitCode"], 1);
    assert_eq!(
        result.result.as_ref().unwrap()["artifactObservation"]["changes"][0]["kind"],
        "created"
    );
    assert_eq!(
        result.result.as_ref().unwrap()["artifactObservation"]["expectedOutputs"][0]["outcome"],
        "created"
    );
    let persisted = storage
        .list_agent_tool_results_for_run(run_id, "run_command")
        .unwrap();
    assert_eq!(persisted.len(), 1);
    assert_eq!(
        persisted[0].result.as_ref().unwrap()["artifactObservation"]["changes"][0]["path"],
        "audit-output.csv"
    );
    assert_eq!(
        persisted[0].result.as_ref().unwrap()["artifactObservation"]["coverage"]
            ["workspaceIncluded"],
        true
    );
    let persisted_commands = storage.list_agent_command_results_for_run(run_id).unwrap();
    assert_eq!(persisted_commands.len(), 1);
    assert_eq!(persisted_commands[0].exit_code, Some(1));
    let persisted_observation = persisted_commands[0]
        .artifact_observation
        .as_ref()
        .expect("command_result_json must retain artifact observation");
    assert!(persisted_observation.changes.iter().any(|change| {
        change.path == "audit-output.csv"
            && change.kind == mycopilot_core::AgentCommandArtifactChangeKind::Created
    }));
}

#[test]
fn automatic_command_requires_durable_execution_claim_before_side_effects() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let service = AgentService::new(storage);
    let mut input = command_test_input(fixture.path());
    input
        .context
        .as_mut()
        .expect("command test context")
        .permissions
        .command_safety = mycopilot_core::AgentCommandSafetyPolicy::FullAccess;
    let run_id = "run-command-claim-failure";
    let call_id = "command-claim-failure";
    let command = command_request(call_id, "mkdir must-not-exist");
    inject_auto_action_audit_failure(run_id, call_id, "executing");

    let result = service
        .execute_auto_approved_action(
            AutoApprovedActionContext::new(input, run_id.to_string(), None, None, None),
            AgentProposedAction::Command { command },
            AgentCancellationToken::new(),
        )
        .unwrap();

    assert!(!result.ok);
    assert_eq!(
        result.result.as_ref().unwrap()["code"],
        "auditPersistenceFailed"
    );
    assert_eq!(result.result.as_ref().unwrap()["phase"], "beforeExecution");
    assert_eq!(result.result.as_ref().unwrap()["executionAttempted"], false);
    assert!(!fixture.path().join("must-not-exist").exists());
}

#[test]
fn automatic_command_final_audit_failure_preserves_and_then_replays_effect_evidence() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let service = AgentService::new(Arc::clone(&storage));
    let mut input = command_test_input(fixture.path());
    input
        .context
        .as_mut()
        .expect("command test context")
        .permissions
        .command_safety = mycopilot_core::AgentCommandSafetyPolicy::FullAccess;
    let run_id = "run-command-final-audit-failure";
    let call_id = "command-final-audit-failure";
    let mut command = command_request(call_id, "printf durable-evidence > durable-evidence.csv");
    command.observe = Some(mycopilot_core::AgentCommandArtifactObservationRequest {
        kinds: vec![mycopilot_core::AgentCommandArtifactObservationKind::Office],
        expected_outputs: vec!["durable-evidence.csv".to_string()],
        additional_roots: Vec::new(),
    });
    inject_auto_action_audit_failure(run_id, call_id, "completed");
    let context = durable_auto_command_context(
        &storage,
        input,
        run_id,
        "assistant-command-final-audit-failure",
    );
    let action = AgentProposedAction::Command {
        command: command.clone(),
    };

    let result = service
        .execute_auto_approved_action(
            context.clone(),
            action.clone(),
            AgentCancellationToken::new(),
        )
        .unwrap();

    assert!(!result.ok);
    let evidence = result.result.as_ref().unwrap();
    assert_eq!(evidence["code"], "auditPersistenceFailed");
    assert_eq!(evidence["phase"], "afterExecution");
    assert_eq!(evidence["executionAttempted"], true);
    assert_eq!(evidence["effectsMayHaveOccurred"], true);
    assert_eq!(evidence["execution"]["exitCode"], 0);
    assert_eq!(
        evidence["execution"]["artifactObservation"]["changes"][0]["kind"],
        "created"
    );
    let persisted = storage.list_agent_command_results_for_run(run_id).unwrap();
    assert_eq!(persisted.len(), 1);
    assert!(persisted[0].artifact_observation.is_some());

    let effect_id = pending_action_storage_id(run_id, call_id);
    service.file_effects.restore_unsettled(
        None,
        Some("conversation-command-policy"),
        run_id,
        &effect_id,
    );
    assert_eq!(
        service.unsettled_file_effect_ids_for_conversation("conversation-command-policy"),
        vec![format!("{run_id}/{effect_id}")]
    );

    let replay = service
        .execute_auto_approved_action(context, action, AgentCancellationToken::new())
        .unwrap();
    assert_eq!(replay.call_id, result.call_id);
    assert_eq!(replay.tool, result.tool);
    assert_eq!(replay.ok, result.ok);
    assert_eq!(replay.result, result.result);
    assert_eq!(replay.error, result.error);
    assert!(service
        .unsettled_file_effect_ids_for_conversation("conversation-command-policy")
        .is_empty());
}

#[test]
fn automatic_command_reconciles_a_terminal_receipt_after_post_commit_error() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let service = AgentService::new(Arc::clone(&storage));
    let mut input = command_test_input(fixture.path());
    input
        .context
        .as_mut()
        .expect("command test context")
        .permissions
        .command_safety = mycopilot_core::AgentCommandSafetyPolicy::FullAccess;
    let run_id = "run-command-post-commit-reconciliation";
    let call_id = "command-post-commit-reconciliation";
    let mut command = command_request(call_id, "printf once >> exactly-once.csv");
    command.observe = Some(mycopilot_core::AgentCommandArtifactObservationRequest {
        kinds: vec![mycopilot_core::AgentCommandArtifactObservationKind::Office],
        expected_outputs: vec!["exactly-once.csv".to_string()],
        additional_roots: Vec::new(),
    });
    inject_auto_action_audit_post_commit_failure(run_id, call_id, "completed");
    let context = durable_auto_command_context(
        &storage,
        input,
        run_id,
        "assistant-command-post-commit-reconciliation",
    );
    let action = AgentProposedAction::Command {
        command: command.clone(),
    };

    let result = service
        .execute_auto_approved_action(
            context.clone(),
            action.clone(),
            AgentCancellationToken::new(),
        )
        .unwrap();

    assert!(result.ok, "durable terminal receipt is authoritative");
    assert_eq!(
        std::fs::read_to_string(fixture.path().join("exactly-once.csv")).unwrap(),
        "once"
    );
    let persisted = storage.list_agent_command_results_for_run(run_id).unwrap();
    assert_eq!(persisted.len(), 1);
    assert!(persisted[0].artifact_observation.is_some());

    let effect_id = pending_action_storage_id(run_id, call_id);
    service.file_effects.restore_unsettled(
        None,
        Some("conversation-command-policy"),
        run_id,
        &effect_id,
    );
    assert_eq!(
        service.unsettled_file_effect_ids_for_conversation("conversation-command-policy"),
        vec![format!("{run_id}/{effect_id}")]
    );

    let replay = service
        .execute_auto_approved_action(context, action, AgentCancellationToken::new())
        .unwrap();
    assert_eq!(replay.call_id, result.call_id);
    assert_eq!(replay.tool, result.tool);
    assert_eq!(replay.ok, result.ok);
    assert_eq!(replay.result, result.result);
    assert_eq!(replay.error, result.error);
    assert_eq!(
        std::fs::read_to_string(fixture.path().join("exactly-once.csv")).unwrap(),
        "once",
        "retry must replay the receipt instead of executing the process again"
    );
    assert!(service
        .unsettled_file_effect_ids_for_conversation("conversation-command-policy")
        .is_empty());
}

#[test]
fn command_completion_waits_for_a_failed_project_deletion_then_persists_evidence() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let service = AgentService::new(Arc::clone(&storage));
    let mut input = command_test_input(fixture.path());
    let context = input.context.as_mut().expect("command test context");
    context.project_id = Some("project-deletion-barrier".to_string());
    context.permissions.command_safety = mycopilot_core::AgentCommandSafetyPolicy::FullAccess;
    context.workspace.as_mut().unwrap().project_id = Some("project-deletion-barrier".to_string());
    let run_id = "run-project-deletion-barrier";
    let call_id = "command-project-deletion-barrier";
    let mut command = command_request(
        call_id,
        "printf barrier-evidence > deletion-barrier.csv; sleep 0.1",
    );
    command.observe = Some(mycopilot_core::AgentCommandArtifactObservationRequest {
        kinds: vec![mycopilot_core::AgentCommandArtifactObservationKind::Office],
        expected_outputs: vec!["deletion-barrier.csv".to_string()],
        additional_roots: Vec::new(),
    });
    let execution_service = service.clone();
    let execution_context = durable_auto_command_context(
        &storage,
        input,
        run_id,
        "assistant-project-deletion-barrier",
    );
    let execution = std::thread::spawn(move || {
        execution_service.execute_auto_approved_action(
            execution_context,
            AgentProposedAction::Command { command },
            AgentCancellationToken::new(),
        )
    });

    let artifact_path = fixture.path().join("deletion-barrier.csv");
    for _ in 0..100 {
        if artifact_path.exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        artifact_path.exists(),
        "command must cross its side-effect boundary"
    );

    inject_project_deletion_failure("project-deletion-barrier");
    let deletion_error = service
        .delete_project("project-deletion-barrier")
        .unwrap_err();
    assert!(
        deletion_error.contains("injected project deletion failure"),
        "storage deletion must run only after the command effect is durably settled"
    );
    assert!(!service.is_project_deleting(Some("project-deletion-barrier")));
    let result = execution.join().unwrap().unwrap();
    assert_eq!(result.call_id, call_id);
    assert_eq!(result.tool, "run_command");
    let terminal = wait_for_terminal_auto_command_session(&storage, "conversation-command-policy");
    let archive_ref = terminal
        .snapshot
        .archive_ref
        .as_deref()
        .expect("settled command Session has an Exact History reference");
    let archive =
        command_session_archive_content(&storage, "conversation-command-policy", archive_ref);
    assert!(archive.contains("artifactObservation"));
    assert!(archive.contains("deletion-barrier.csv"));
}

#[test]
fn project_deletion_timeout_preserves_late_command_observation() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let service = AgentService::new(Arc::clone(&storage));
    let mut input = command_test_input(fixture.path());
    let context = input.context.as_mut().expect("command test context");
    context.project_id = Some("project-deletion-timeout".to_string());
    context.permissions.command_safety = mycopilot_core::AgentCommandSafetyPolicy::FullAccess;
    context.workspace.as_mut().unwrap().project_id = Some("project-deletion-timeout".to_string());
    let run_id = "run-project-deletion-timeout";
    let call_id = "command-project-deletion-timeout";
    let mut command = command_request(
        call_id,
        "printf lifecycle-output; printf timeout-evidence > deletion-timeout.csv; sleep 5",
    );
    command.observe = Some(mycopilot_core::AgentCommandArtifactObservationRequest {
        kinds: vec![mycopilot_core::AgentCommandArtifactObservationKind::Office],
        expected_outputs: vec!["deletion-timeout.csv".to_string()],
        additional_roots: Vec::new(),
    });
    let persistence_entered = Arc::new(std::sync::Barrier::new(2));
    let persistence_release = Arc::new(std::sync::Barrier::new(2));
    let hook_entered = Arc::clone(&persistence_entered);
    let hook_release = Arc::clone(&persistence_release);
    let hook_calls = Arc::new(AtomicUsize::new(0));
    let observed_hook_calls = Arc::clone(&hook_calls);
    service
        .command_sessions
        .set_before_output_persistence_hook(Arc::new(move |_, _| {
            if observed_hook_calls.fetch_add(1, Ordering::SeqCst) == 0 {
                hook_entered.wait();
                hook_release.wait();
            }
        }));
    let execution_service = service.clone();
    let execution_context = durable_auto_command_context(
        &storage,
        input,
        run_id,
        "assistant-project-deletion-timeout",
    );
    let execution = std::thread::spawn(move || {
        execution_service.execute_auto_approved_action(
            execution_context,
            AgentProposedAction::Command { command },
            AgentCancellationToken::new(),
        )
    });

    let artifact_path = fixture.path().join("deletion-timeout.csv");
    for _ in 0..100 {
        if artifact_path.exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(artifact_path.exists());
    persistence_entered.wait();

    let deletion_error = service
        .delete_project("project-deletion-timeout")
        .unwrap_err();
    assert!(deletion_error.contains("did not finish execution and durable settlement"));
    assert!(!service.is_project_deleting(Some("project-deletion-timeout")));
    assert_eq!(hook_calls.load(Ordering::SeqCst), 1);

    persistence_release.wait();
    let result = execution.join().unwrap().unwrap();
    assert_eq!(result.call_id, call_id);
    assert_eq!(result.tool, "run_command");
    let terminal = wait_for_terminal_auto_command_session(&storage, "conversation-command-policy");
    let archive_ref = terminal
        .snapshot
        .archive_ref
        .as_deref()
        .expect("late terminal settlement has an Exact History reference");
    let archive =
        command_session_archive_content(&storage, "conversation-command-policy", archive_ref);
    assert!(archive.contains("artifactObservation"));
    assert!(archive.contains("deletion-timeout.csv"));
}

#[test]
fn durable_command_session_receipt_allows_deletion_after_action_audit_is_indeterminate() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let service = AgentService::new(Arc::clone(&storage));
    let mut input = command_test_input(fixture.path());
    let context = input.context.as_mut().expect("command test context");
    context.project_id = Some("project-unsettled-command".to_string());
    context.permissions.command_safety = mycopilot_core::AgentCommandSafetyPolicy::FullAccess;
    context.workspace.as_mut().unwrap().project_id = Some("project-unsettled-command".to_string());
    let run_id = "run-project-unsettled-command";
    let call_id = "command-project-unsettled-command";
    let mut command = command_request(
        call_id,
        "printf unsettled-evidence > unsettled-evidence.csv",
    );
    command.observe = Some(mycopilot_core::AgentCommandArtifactObservationRequest {
        kinds: vec![mycopilot_core::AgentCommandArtifactObservationKind::Office],
        expected_outputs: vec!["unsettled-evidence.csv".to_string()],
        additional_roots: Vec::new(),
    });
    inject_auto_action_audit_failure(run_id, call_id, "completed");
    inject_auto_action_audit_failure(run_id, call_id, "failed");
    inject_auto_action_audit_failure(run_id, call_id, "failed");

    let result = service
        .execute_auto_approved_action(
            durable_auto_command_context(
                &storage,
                input,
                run_id,
                "assistant-project-unsettled-command",
            ),
            AgentProposedAction::Command { command },
            AgentCancellationToken::new(),
        )
        .unwrap();

    assert!(!result.ok);
    assert_eq!(
        result.result.as_ref().unwrap()["code"],
        "finalizationIndeterminate"
    );
    assert!(fixture.path().join("unsettled-evidence.csv").exists());
    let terminal = wait_for_terminal_auto_command_session(&storage, "conversation-command-policy");
    let archive_ref = terminal
        .snapshot
        .archive_ref
        .as_deref()
        .expect("the durable Session owns its terminal Exact History receipt");
    let archive =
        command_session_archive_content(&storage, "conversation-command-policy", archive_ref);
    assert!(archive.contains("artifactObservation"));
    assert!(archive.contains("unsettled-evidence.csv"));

    service.delete_project("project-unsettled-command").unwrap();
    assert!(storage
        .load_projects()
        .unwrap()
        .iter()
        .all(|project| project.id != "project-unsettled-command"));
}

#[test]
fn settled_effect_in_another_run_cannot_clear_an_unsettled_identity() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let service = AgentService::new(storage);
    let mut input = command_test_input(fixture.path());
    let context = input.context.as_mut().expect("command test context");
    context.project_id = Some("project-cross-run-effect".to_string());
    context.workspace.as_mut().unwrap().project_id = Some("project-cross-run-effect".to_string());

    let first_effect_id = pending_action_storage_id("run-a", "call-1");
    let second_effect_id = pending_action_storage_id("run-b", "call-1");
    let mut first = service
        .register_file_effect(&input, "run-a", &first_effect_id)
        .unwrap();
    first.mark_effects_started();
    drop(first);

    let mut second = service
        .register_file_effect(&input, "run-b", &second_effect_id)
        .unwrap();
    second.mark_effects_started();
    second.mark_durably_settled();
    drop(second);

    let error = service
        .delete_project("project-cross-run-effect")
        .unwrap_err();
    assert!(error.contains(&first_effect_id));
    assert!(!error.contains(&second_effect_id));
}

#[test]
fn conversation_deletion_rejects_unsettled_effects_without_a_project() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let service = AgentService::new(storage);
    let mut input = command_test_input(fixture.path());
    let context = input.context.as_mut().expect("command test context");
    context.conversation_id = Some("conversation-unsettled-effect".to_string());
    context.project_id = None;
    context.workspace.as_mut().unwrap().project_id = None;

    let effect_id = pending_action_storage_id("run-conversation-effect", "call-effect");
    let mut guard = service
        .register_file_effect(&input, "run-conversation-effect", &effect_id)
        .unwrap();
    guard.mark_effects_started();
    drop(guard);

    let error = service
        .delete_conversation("conversation-unsettled-effect")
        .unwrap_err();
    assert!(error.contains("without a confirmed durable terminal receipt"));
    assert!(error.contains(&effect_id));
    assert!(!service.is_conversation_deleting(Some("conversation-unsettled-effect")));
}

#[test]
fn successful_conversation_deletion_tombstone_rejects_stale_file_effect_context() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let service = AgentService::new(storage);
    let mut input = command_test_input(fixture.path());
    let context = input.context.as_mut().expect("command test context");
    context.conversation_id = Some("conversation-deleted-generation".to_string());
    context.permissions.command_safety = mycopilot_core::AgentCommandSafetyPolicy::FullAccess;

    service
        .delete_conversation("conversation-deleted-generation")
        .unwrap();
    assert!(service.is_conversation_deleting(Some("conversation-deleted-generation")));
    let error = service
        .execute_auto_approved_action(
            AutoApprovedActionContext::new(
                input,
                "run-deleted-conversation".to_string(),
                Some("conversation-deleted-generation".to_string()),
                None,
                None,
            ),
            AgentProposedAction::Command {
                command: command_request(
                    "command-deleted-conversation",
                    "printf forbidden > deleted-conversation-must-not-exist.csv",
                ),
            },
            AgentCancellationToken::new(),
        )
        .unwrap_err();

    assert!(error.is_cancelled());
    assert!(!fixture
        .path()
        .join("deleted-conversation-must-not-exist.csv")
        .exists());
}

#[test]
fn message_deletion_waits_for_a_durable_file_effect_receipt() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    storage
        .save_conversation(ChatConversationRecord {
            id: "conversation-message-effect".to_string(),
            project_id: None,
            model_id: Some("test-model".to_string()),
            title: "Message effect barrier".to_string(),
            messages: vec![ChatMessageRecord {
                human_interaction_response: None,
                id: "assistant-message-effect".to_string(),
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
    let service = AgentService::new(Arc::clone(&storage));
    let mut input = command_test_input(fixture.path());
    input
        .context
        .as_mut()
        .expect("command test context")
        .conversation_id = Some("conversation-message-effect".to_string());
    let effect_id = pending_action_storage_id("run-message-effect", "call-message-effect");
    let mut effect = service
        .register_file_effect(&input, "run-message-effect", &effect_id)
        .unwrap();
    effect.mark_effects_started();

    let deleting_service = service.clone();
    let (result_tx, result_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let result = deleting_service.delete_chat_messages(
            "conversation-message-effect",
            &["assistant-message-effect".to_string()],
        );
        result_tx.send(result).unwrap();
    });

    assert!(result_rx.recv_timeout(Duration::from_millis(60)).is_err());
    assert_eq!(
        storage
            .load_conversation("conversation-message-effect")
            .unwrap()
            .unwrap()
            .messages
            .len(),
        1
    );

    effect.mark_durably_settled();
    drop(effect);
    result_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("message deletion should resume after durable settlement")
        .unwrap();
    assert!(storage
        .load_conversation("conversation-message-effect")
        .unwrap()
        .unwrap()
        .messages
        .is_empty());
}

#[test]
fn message_deletion_rejects_an_unsettled_file_effect_and_preserves_the_owner() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    storage
        .save_conversation(ChatConversationRecord {
            id: "conversation-message-unsettled".to_string(),
            project_id: None,
            model_id: Some("test-model".to_string()),
            title: "Message unsettled barrier".to_string(),
            messages: vec![ChatMessageRecord {
                human_interaction_response: None,
                id: "assistant-message-unsettled".to_string(),
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
    let service = AgentService::new(Arc::clone(&storage));
    let mut input = command_test_input(fixture.path());
    input
        .context
        .as_mut()
        .expect("command test context")
        .conversation_id = Some("conversation-message-unsettled".to_string());
    let effect_id = pending_action_storage_id("run-message-unsettled", "call-message-unsettled");
    let mut effect = service
        .register_file_effect(&input, "run-message-unsettled", &effect_id)
        .unwrap();
    effect.mark_effects_started();
    drop(effect);

    let error = service
        .delete_chat_messages(
            "conversation-message-unsettled",
            &["assistant-message-unsettled".to_string()],
        )
        .unwrap_err();
    assert!(error.contains("without a confirmed durable terminal receipt"));
    assert!(error.contains(&effect_id));
    assert!(!service.is_conversation_deleting(Some("conversation-message-unsettled")));
    assert_eq!(
        storage
            .load_conversation("conversation-message-unsettled")
            .unwrap()
            .unwrap()
            .messages
            .len(),
        1
    );
}

#[test]
fn message_deletion_blocks_pending_and_approved_processes_then_retires_terminal_memory() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    storage
        .save_conversation(ChatConversationRecord {
            id: "conversation-message-pending".to_string(),
            project_id: None,
            model_id: Some("test-model".to_string()),
            title: "Message pending barrier".to_string(),
            messages: vec![
                ChatMessageRecord {
                    human_interaction_response: None,
                    id: "assistant-message-pending".to_string(),
                    role: "assistant".to_string(),
                    content: String::new(),
                    created_at: 1,
                    status: Some("pending".to_string()),
                    attachments: Vec::new(),
                    agent_run_json: None,
                    ui_state_json: None,
                },
                ChatMessageRecord {
                    human_interaction_response: None,
                    id: "assistant-message-retained".to_string(),
                    role: "assistant".to_string(),
                    content: String::new(),
                    created_at: 2,
                    status: Some("sent".to_string()),
                    attachments: Vec::new(),
                    agent_run_json: None,
                    ui_state_json: None,
                },
            ],
            created_at: 1,
            updated_at: 1,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
    let service = AgentService::new(Arc::clone(&storage));
    let mut input = command_test_input(fixture.path());
    input
        .context
        .as_mut()
        .expect("command test context")
        .conversation_id = Some("conversation-message-pending".to_string());
    let action_id = "command-message-pending";
    let storage_id = pending_action_storage_id("run-message-pending", action_id);
    service
        .pending_actions
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .insert(
            storage_id.clone(),
            PendingActionRecord {
                storage_id: storage_id.clone(),
                snapshot: PendingAgentActionSnapshot {
                    action_id: action_id.to_string(),
                    action_type: "command".to_string(),
                    tool_name: "run_command".to_string(),
                    tool_call_id: Some(action_id.to_string()),
                    run_id: "run-message-pending".to_string(),
                    conversation_id: Some("conversation-message-pending".to_string()),
                    assistant_message_id: Some("assistant-message-pending".to_string()),
                    action: AgentProposedAction::Command {
                        command: command_request(action_id, "printf safe"),
                    },
                    created_at: 1,
                    status: PendingActionStatus::Pending,
                },
                agent_input: input,
            },
        );

    let error = service
        .delete_chat_messages(
            "conversation-message-pending",
            &["assistant-message-pending".to_string()],
        )
        .unwrap_err();
    assert!(error.contains("pending actions"));
    assert!(service
        .pending_actions
        .lock()
        .unwrap_or_else(|lock_error| lock_error.into_inner())
        .contains_key(&storage_id));

    service
        .pending_actions
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get_mut(&storage_id)
        .unwrap()
        .snapshot
        .status = PendingActionStatus::Approved;

    // Approval owns this guard before the async worker starts. There is deliberately no
    // cancellation token or file-effect guard yet: deletion must still see the queued process.
    let approved_process_guard = service
        .process_runs
        .register(&storage_id, "run-message-pending");
    let error = service
        .delete_chat_messages(
            "conversation-message-pending",
            &["assistant-message-pending".to_string()],
        )
        .unwrap_err();
    assert!(error.contains("cancelled agent runs did not reach a safe terminal boundary"));
    assert!(service
        .pending_actions
        .lock()
        .unwrap_or_else(|lock_error| lock_error.into_inner())
        .contains_key(&storage_id));
    assert_eq!(
        storage
            .load_conversation("conversation-message-pending")
            .unwrap()
            .unwrap()
            .messages
            .len(),
        2
    );
    drop(approved_process_guard);

    service
        .pending_actions
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get_mut(&storage_id)
        .unwrap()
        .snapshot
        .status = PendingActionStatus::Completed;
    for (run_id, assistant_message_id) in [
        ("run-message-pending", "assistant-message-pending"),
        ("run-message-retained", "assistant-message-retained"),
    ] {
        service.register_usage_context(
            run_id,
            AgentRunUsageContext {
                conversation_id: "conversation-message-pending".to_string(),
                assistant_message_id: assistant_message_id.to_string(),
                run_id: run_id.to_string(),
                project_id: None,
                model_id: "test-model".to_string(),
                model_name: "Test model".to_string(),
                provider_usage_semantics: ProviderUsageSemantics::StandardAdditive,
                input_price: None,
                cached_input_price: None,
                output_price: None,
                started_at: 1,
            },
        );
        service
            .trace_snapshots
            .lock()
            .unwrap_or_else(|lock_error| lock_error.into_inner())
            .insert(run_id.to_string(), ConversationTraceSnapshot::default());
    }
    service
        .delete_chat_messages(
            "conversation-message-pending",
            &["assistant-message-pending".to_string()],
        )
        .unwrap();
    assert!(!service
        .pending_actions
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .contains_key(&storage_id));
    let stored = storage
        .load_conversation("conversation-message-pending")
        .unwrap()
        .unwrap();
    assert_eq!(stored.messages.len(), 1);
    assert_eq!(stored.messages[0].id, "assistant-message-retained");

    let usage_contexts = service
        .usage_contexts
        .lock()
        .unwrap_or_else(|lock_error| lock_error.into_inner());
    assert!(!usage_contexts.contains_key("run-message-pending"));
    assert!(usage_contexts.contains_key("run-message-retained"));
    drop(usage_contexts);
    let trace_snapshots = service
        .trace_snapshots
        .lock()
        .unwrap_or_else(|lock_error| lock_error.into_inner());
    assert!(!trace_snapshots.contains_key("run-message-pending"));
    assert!(trace_snapshots.contains_key("run-message-retained"));
}

#[test]
fn restart_restores_executing_auto_command_as_project_deletion_blocker() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    storage
        .save_project(ProjectRecord {
            id: "project-restart-unsettled".to_string(),
            name: "Restart unsettled".to_string(),
            path: Some(fixture.path().to_string_lossy().into_owned()),
            created_at: 1,
            pinned_at: None,
        })
        .unwrap();
    storage
        .save_conversation(ChatConversationRecord {
            id: "conversation-restart-unsettled".to_string(),
            project_id: Some("project-restart-unsettled".to_string()),
            model_id: Some("test-model".to_string()),
            title: "Restart unsettled".to_string(),
            messages: vec![ChatMessageRecord {
                human_interaction_response: None,
                id: "assistant-restart-unsettled".to_string(),
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
    let service = AgentService::new(Arc::clone(&storage));
    let mut input = command_test_input(fixture.path());
    let context = input.context.as_mut().expect("command test context");
    context.conversation_id = Some("conversation-restart-unsettled".to_string());
    context.project_id = Some("project-restart-unsettled".to_string());
    context.permissions.command_safety = mycopilot_core::AgentCommandSafetyPolicy::FullAccess;
    context.workspace.as_mut().unwrap().project_id = Some("project-restart-unsettled".to_string());
    let run_id = "run-restart-unsettled";
    let call_id = "command-restart-unsettled";
    let command = command_request(call_id, "printf restart-evidence > restart-unsettled.csv");
    inject_auto_action_audit_failure(run_id, call_id, "completed");
    inject_auto_action_audit_failure(run_id, call_id, "failed");
    inject_auto_action_audit_failure(run_id, call_id, "failed");

    let result = service
        .execute_auto_approved_action(
            AutoApprovedActionContext::new(
                input,
                run_id.to_string(),
                Some("conversation-restart-unsettled".to_string()),
                Some("assistant-restart-unsettled".to_string()),
                None,
            ),
            AgentProposedAction::Command { command },
            AgentCancellationToken::new(),
        )
        .unwrap();
    assert_eq!(
        result.result.as_ref().unwrap()["code"],
        "finalizationIndeterminate"
    );
    drop(service);

    let restarted = AgentService::new(storage);
    let deletion_error = restarted
        .delete_project("project-restart-unsettled")
        .unwrap_err();
    let effect_id = pending_action_storage_id(run_id, call_id);
    assert!(deletion_error.contains("without a confirmed durable terminal receipt"));
    assert!(deletion_error.contains(&effect_id));
}

#[test]
fn restart_conservatively_blocks_interrupted_manual_command_deletion() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    storage
        .save_project(ProjectRecord {
            id: "project-restart-manual".to_string(),
            name: "Restart manual".to_string(),
            path: Some(fixture.path().to_string_lossy().into_owned()),
            created_at: 1,
            pinned_at: None,
        })
        .unwrap();
    storage
        .save_conversation(ChatConversationRecord {
            id: "conversation-restart-manual".to_string(),
            project_id: Some("project-restart-manual".to_string()),
            model_id: Some("test-model".to_string()),
            title: "Restart manual".to_string(),
            messages: vec![ChatMessageRecord {
                human_interaction_response: None,
                id: "assistant-restart-manual".to_string(),
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
    let run_id = "run-restart-manual";
    let call_id = "command-restart-manual";
    let storage_id = pending_action_storage_id(run_id, call_id);
    let action = AgentProposedAction::Command {
        command: command_request(call_id, "printf maybe-ran > restart-manual.csv"),
    };
    let AgentProposedAction::Command { command } = &action else {
        unreachable!("fixture is a command action")
    };
    let call = checkpoint_call_for_command(command);
    let mut checkpoint = AgentRunCheckpoint {
        pause_reason: mycopilot_core::AgentRunCheckpointPauseReason::Approval,
        version: AGENT_RUN_CHECKPOINT_SCHEMA_VERSION,
        run_id: run_id.to_string(),
        pending_action_id: None,
        file_change_run_grant_ref: None,
        pending_file_observation: None,
        context_items: vec![terminal_test_checkpoint_item(&call)],
        next_model_request_index: 1,
        queued_tool_calls: Vec::new(),
        deferred_external_tool_call_count: 0,
        suppressed_narration: false,
        extension_snapshots: Vec::new(),
        tool_set: crate::test_tool_set_checkpoint(),
        run_context: None,
        collaboration_run_snapshot: None,
        model_capabilities: ModelCapabilities::default(),
        provider_profile_config: crate::test_provider_profile_config(),
        provider_protocol_key: crate::test_provider_protocol_key("test-model"),
        assistant_turn_identity: crate::test_assistant_turn_identity(&[call.id.as_str()]),
        provider_continuation_refs: Vec::new(),
        run_world_state: crate::test_run_world_state(),
        pending_tool_call_id: call.id.clone(),
        conversation_trace_items: vec![ConversationTurnTraceItem::ToolCall {
            sequence: 0,
            call_id: call.id.clone(),
            tool: call.tool.clone(),
            operation: call.args.clone(),
            provenance: AgentToolIdentity::Builtin {
                tool_name: call.tool.clone(),
            },
            approval_status: AgentApprovalStatus::Required,
            truncated: false,
        }],
        conversation_model_context_items: vec![terminal_test_model_context(&call)],
        next_conversation_trace_sequence: 1,
        conversation_trace_truncated: false,
    };
    let mut agent_input = command_test_input(fixture.path());
    let context = agent_input.context.as_mut().expect("command context");
    context.conversation_id = Some("conversation-restart-manual".to_string());
    context.project_id = Some("project-restart-manual".to_string());
    context.workspace.as_mut().expect("workspace").project_id =
        Some("project-restart-manual".to_string());
    checkpoint.run_context = agent_input.context.clone();
    agent_input.resume_checkpoint = Some(checkpoint);
    save_test_pending_provider_for_input(&storage, &mut agent_input);
    let agent_input_json = PersistedAgentResumeInput::from_agent_input(&agent_input)
        .unwrap()
        .encode();
    let durable_trace = ConversationTurnTrace {
        schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
        run_id: run_id.to_string(),
        conversation_id: "conversation-restart-manual".to_string(),
        assistant_message_id: "assistant-restart-manual".to_string(),
        terminal_status: ConversationTurnTraceTerminalStatus::InProgress,
        terminal_error: None,
        truncated: false,
        items: agent_input
            .resume_checkpoint
            .as_ref()
            .unwrap()
            .conversation_trace_items
            .clone(),
    };
    storage
        .append_in_progress_conversation_turn_trace_and_apply_guidances(
            &durable_trace,
            &agent_input
                .resume_checkpoint
                .as_ref()
                .unwrap()
                .conversation_model_context_items,
            1,
            2,
        )
        .unwrap();
    let action_json = serde_json::to_string(&action).unwrap();
    storage
        .store_pending_agent_action(AgentPendingActionRecord {
            action_id: storage_id.clone(),
            run_id: run_id.to_string(),
            conversation_id: Some("conversation-restart-manual".to_string()),
            assistant_message_id: Some("assistant-restart-manual".to_string()),
            action_type: "command".to_string(),
            tool_name: "run_command".to_string(),
            tool_call_id: Some(call_id.to_string()),
            status: "approved".to_string(),
            target_status: None,
            action_json: action_json.clone(),
            agent_input_json,
            created_at: 1,
            updated_at: 2,
        })
        .unwrap();
    storage
        .upsert_agent_action_audit(AgentActionAuditRecord {
            action_id: storage_id.clone(),
            run_id: run_id.to_string(),
            conversation_id: Some("conversation-restart-manual".to_string()),
            assistant_message_id: Some("assistant-restart-manual".to_string()),
            action_type: "command".to_string(),
            tool_name: "run_command".to_string(),
            decision: Some("approved".to_string()),
            status: "approved".to_string(),
            action_json,
            file_change_result_json: None,
            command_result_json: None,
            tool_result_json: None,
            error: None,
            created_at: 1,
            decided_at: Some(2),
            completed_at: None,
            effective_permissions_json: Some("{}".to_string()),
            path_scope: Some("workspace".to_string()),
            command_cwd_scope: Some("workspace".to_string()),
            blocked_reason: None,
            decision_source: Some("manual".to_string()),
        })
        .unwrap();

    let restarted = AgentService::new(storage);
    let deletion_error = restarted
        .delete_project("project-restart-manual")
        .unwrap_err();
    assert!(deletion_error.contains("without a confirmed durable terminal receipt"));
    assert!(deletion_error.contains(&storage_id));
}

#[test]
fn successful_project_deletion_tombstone_rejects_old_command_context() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let service = AgentService::new(storage);
    let mut input = command_test_input(fixture.path());
    let context = input.context.as_mut().expect("command test context");
    context.project_id = Some("project-deleted-generation".to_string());
    context.permissions.command_safety = mycopilot_core::AgentCommandSafetyPolicy::FullAccess;
    context.workspace.as_mut().unwrap().project_id = Some("project-deleted-generation".to_string());

    service
        .delete_project("project-deleted-generation")
        .unwrap();
    assert!(service.is_project_deleting(Some("project-deleted-generation")));
    let error = service
        .execute_auto_approved_action(
            AutoApprovedActionContext::new(
                input,
                "run-deleted-generation".to_string(),
                None,
                None,
                None,
            ),
            AgentProposedAction::Command {
                command: command_request(
                    "command-deleted-generation",
                    "printf forbidden > must-not-exist.csv",
                ),
            },
            AgentCancellationToken::new(),
        )
        .unwrap_err();

    assert!(error.is_cancelled());
    assert!(!fixture.path().join("must-not-exist.csv").exists());
}

#[test]
fn manually_approved_command_reconciles_two_post_commit_errors_and_keeps_observation() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let service = AgentService::new(Arc::clone(&storage));
    let run_id = "run-manual-command-audit-failure";
    let call_id = "manual-command-audit-failure";
    let mut command = command_request(call_id, "printf manual-evidence > manual-evidence.csv");
    command.observe = Some(mycopilot_core::AgentCommandArtifactObservationRequest {
        kinds: vec![mycopilot_core::AgentCommandArtifactObservationKind::Office],
        expected_outputs: vec!["manual-evidence.csv".to_string()],
        additional_roots: Vec::new(),
    });
    storage
        .save_conversation(ChatConversationRecord {
            id: "conversation-manual-command".to_string(),
            project_id: None,
            model_id: Some("test-model".to_string()),
            title: "Manual command".to_string(),
            messages: vec![ChatMessageRecord {
                human_interaction_response: None,
                id: "assistant-manual-command".to_string(),
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
    let call = checkpoint_call_for_command(&command);
    let mut checkpoint = AgentRunCheckpoint {
        pause_reason: mycopilot_core::AgentRunCheckpointPauseReason::Approval,
        version: AGENT_RUN_CHECKPOINT_SCHEMA_VERSION,
        run_id: run_id.to_string(),
        pending_action_id: None,
        file_change_run_grant_ref: None,
        pending_file_observation: None,
        context_items: vec![terminal_test_checkpoint_item(&call)],
        next_model_request_index: 1,
        queued_tool_calls: Vec::new(),
        deferred_external_tool_call_count: 0,
        suppressed_narration: false,
        extension_snapshots: Vec::new(),
        tool_set: crate::test_tool_set_checkpoint(),
        run_context: None,
        collaboration_run_snapshot: None,
        model_capabilities: ModelCapabilities::default(),
        provider_profile_config: crate::test_provider_profile_config(),
        provider_protocol_key: crate::test_provider_protocol_key("model-1"),
        assistant_turn_identity: crate::test_assistant_turn_identity(&[call.id.as_str()]),
        provider_continuation_refs: Vec::new(),
        run_world_state: crate::test_run_world_state(),
        pending_tool_call_id: call.id.clone(),
        conversation_model_context_items: vec![terminal_test_model_context(&call)],
        conversation_trace_items: vec![ConversationTurnTraceItem::ToolCall {
            sequence: 0,
            call_id: call.id.clone(),
            tool: call.tool.clone(),
            operation: call.args.clone(),
            provenance: AgentToolIdentity::Builtin {
                tool_name: call.tool.clone(),
            },
            approval_status: AgentApprovalStatus::Required,
            truncated: false,
        }],
        next_conversation_trace_sequence: 1,
        conversation_trace_truncated: false,
    };
    let mut agent_input = command_test_input(fixture.path());
    let context = agent_input.context.as_mut().unwrap();
    context.conversation_id = Some("conversation-manual-command".to_string());
    checkpoint.run_context = agent_input.context.clone();
    agent_input.resume_checkpoint = Some(checkpoint);
    save_test_pending_provider_for_input(&storage, &mut agent_input);
    let record = PendingActionRecord {
        storage_id: pending_action_storage_id(run_id, call_id),
        snapshot: PendingAgentActionSnapshot {
            action_id: call_id.to_string(),
            action_type: "command".to_string(),
            tool_name: "run_command".to_string(),
            tool_call_id: Some(call_id.to_string()),
            run_id: run_id.to_string(),
            conversation_id: Some("conversation-manual-command".to_string()),
            assistant_message_id: Some("assistant-manual-command".to_string()),
            action: AgentProposedAction::Command {
                command: command.clone(),
            },
            created_at: 1,
            status: PendingActionStatus::Approved,
        },
        agent_input,
    };
    service.persist_pending_action(&record).unwrap();
    service
        .persist_action_audit(
            &record,
            Some("approved"),
            "approved",
            None,
            None,
            None,
            None,
            Some(2),
            None,
        )
        .unwrap();
    let mut command_result = run_current_command_session(
        fixture.path(),
        &command,
        permissions_from_input(&record.agent_input),
        CommandAuthorizationSource::ExplicitUser,
    );
    assert_eq!(command_result.exit_code, Some(0));
    // A Host continuation can carry substantially more output than the model projection. Keep
    // the fixture deterministic while proving the approval boundary archives the exact result.
    let full_stdout = "approval-stdout-evidence\n".repeat(4_096);
    let capture_policy = mycopilot_core::command::ProcessOutputCapturePolicy::process_default();
    let capture_budget = mycopilot_core::command::ProcessOutputCaptureBudget::new(
        capture_policy.max_capture_bytes(),
    );
    let stdout_capture = mycopilot_core::command::join_process_output_capture(
        mycopilot_core::command::spawn_process_output_capture(
            std::io::Cursor::new(full_stdout.as_bytes().to_vec()),
            capture_budget.clone(),
            capture_policy,
        ),
        "test stdout",
    )
    .unwrap();
    let stderr_capture = mycopilot_core::command::join_process_output_capture(
        mycopilot_core::command::spawn_process_output_capture(
            std::io::Cursor::new(Vec::<u8>::new()),
            capture_budget,
            capture_policy,
        ),
        "test stderr",
    )
    .unwrap();
    command_result.stdout = stdout_capture.preview().to_string();
    command_result.stderr = stderr_capture.preview().to_string();
    command_result.stdout_truncated = stdout_capture.preview_truncated();
    command_result.stderr_truncated = stderr_capture.preview_truncated();
    command_result.output_capture =
        mycopilot_core::command::ProcessOutputCaptureMetadata::from_streams(
            &stdout_capture,
            &stderr_capture,
        );
    command_result.stdout_spool = stdout_capture.spool();
    command_result.stderr_spool = stderr_capture.spool();
    let successful_tool_result = command_tool_result(call_id, &command_result);
    let mut continuation_input = record.agent_input.clone();
    continuation_input.approval_decision = Some(AgentApprovalDecision {
        action_id: call_id.to_string(),
        status: AgentApprovalDecisionStatus::Approved,
        message: None,
    });
    continuation_input.tool_continuation = Some(AgentToolContinuation {
        call,
        result: successful_tool_result.clone(),
    });
    let (notifications, _receiver) = tokio::sync::mpsc::unbounded_channel();

    inject_manual_action_audit_failure(&record.storage_id, "completed");
    let pre_commit_error = service
        .commit_audited_command_result_trace_with_continuation(
            &record,
            &continuation_input,
            PendingActionStatus::Completed,
            &command_result,
            3,
            &notifications,
        )
        .unwrap_err();
    assert!(pre_commit_error.contains("injected manual action audit persistence failure"));
    assert_eq!(
        service
            .inspect_audited_command_result_trace_with_continuation(
                &record,
                &continuation_input,
                PendingActionStatus::Completed,
                &command_result,
                3,
            )
            .unwrap(),
        AgentPendingActionSettlementInspection::DefinitelyUncommitted
    );

    inject_manual_action_audit_post_commit_failure(&record.storage_id, "completed");
    inject_manual_action_audit_post_commit_failure(&record.storage_id, "completed");
    let first_error = service
        .commit_audited_command_result_trace_with_continuation(
            &record,
            &continuation_input,
            PendingActionStatus::Completed,
            &command_result,
            3,
            &notifications,
        )
        .unwrap_err();
    assert!(first_error.contains("injected post-commit manual action audit failure"));
    let second_error = service
        .commit_audited_command_result_trace_with_continuation(
            &record,
            &continuation_input,
            PendingActionStatus::Completed,
            &command_result,
            3,
            &notifications,
        )
        .unwrap_err();
    assert!(second_error.contains("injected post-commit manual action audit failure"));
    assert_eq!(
        service
            .inspect_audited_command_result_trace_with_continuation(
                &record,
                &continuation_input,
                PendingActionStatus::Completed,
                &command_result,
                3,
            )
            .unwrap(),
        AgentPendingActionSettlementInspection::CommittedAtBoundary
    );

    let persisted = storage.list_agent_command_results_for_run(run_id).unwrap();
    assert_eq!(persisted.len(), 1);
    assert_eq!(persisted[0].exit_code, Some(0));
    assert_eq!(
        persisted[0]
            .artifact_observation
            .as_ref()
            .unwrap()
            .expected_outputs[0]
            .outcome,
        mycopilot_core::AgentCommandExpectedArtifactOutcomeKind::Created
    );
    let persisted_tool = storage
        .list_agent_tool_results_for_run(run_id, "run_command")
        .unwrap();
    assert_eq!(persisted_tool.len(), 1);
    assert!(persisted_tool[0].ok);
    let trace = storage
        .get_conversation_turn_trace("assistant-manual-command")
        .unwrap()
        .unwrap();
    trace.validate().unwrap();
    let (result_sequence, archive_ref, content_hash, archived_bytes) = match trace.items.last() {
        Some(ConversationTurnTraceItem::ToolResult {
            sequence,
            call_id,
            success: true,
            archive,
            ..
        }) if call_id == "manual-command-audit-failure" => (
            *sequence,
            archive
                .archive_ref
                .clone()
                .expect("approved Host result must carry an exact-history ref"),
            archive
                .content_hash
                .clone()
                .expect("approved Host result must carry an exact-history hash"),
            archive
                .archived_bytes
                .expect("approved Host result must carry its archived size"),
        ),
        other => panic!("unexpected terminal trace item: {other:?}"),
    };
    assert!(archived_bytes > 64 * 1_024);
    let archive = storage
        .find_conversation_history_archive_for_trace_item(
            "conversation-manual-command",
            "assistant-manual-command",
            result_sequence,
        )
        .unwrap()
        .expect("approved Host result archive");
    assert_eq!(archive.archive_ref, archive_ref);
    assert_eq!(archive.content_hash, content_hash);
    assert_eq!(archive.total_bytes, archived_bytes);
    assert!(
        archive.model_projection_truncated,
        "the immutable Archive descriptor must record central-gate projection loss"
    );
    let archived_page = storage
        .read_conversation_history_archive_page(
            "conversation-manual-command",
            &archive_ref,
            mycopilot_core::storage::conversation_history_archive_repository::ConversationHistoryArchivePageUnit::Char,
            0,
            archive.total_chars,
        )
        .unwrap()
        .expect("approved Host result archive page");
    assert_eq!(
        serde_json::from_str::<Value>(&archived_page.content).unwrap(),
        serde_json::to_value(&successful_tool_result).unwrap()
    );
    let archive_metadata = mycopilot_core::ConversationHistoryArchiveTraceMetadata {
        archive_ref: Some(archive.archive_ref.clone()),
        content_hash: Some(archive.content_hash.clone()),
        archived_bytes: Some(archive.total_bytes),
        archived_completely: Some(archive.archived_completely),
        truncated_at_source: archive.truncated_at_source,
        model_projection_truncated: archive.model_projection_truncated,
        history_projection_truncated: false,
        archive_projection_truncated: archive.archive_projection_truncated,
    };
    let expected_model_observation = project_persisted_continuation_observation(
        &continuation_input.model,
        &continuation_input.api_url,
        continuation_input.api_style,
        &successful_tool_result,
        &archive_metadata,
    )
    .unwrap();
    let model_log = storage
        .get_conversation_model_context_log("assistant-manual-command")
        .unwrap()
        .expect("approved Host model log");
    let persisted_model_observation = model_log
        .items
        .iter()
        .find(|item| item.tool_call_id.as_deref() == Some(successful_tool_result.call_id.as_str()))
        .expect("approved Host ToolResult model item")
        .content
        .clone();
    assert_eq!(persisted_model_observation, expected_model_observation);
    let bounded: Value = serde_json::from_str(&persisted_model_observation).unwrap();
    assert_eq!(bounded["truncated"], true);
    assert_eq!(bounded["continueWith"]["tool"], "conversation_history");
    assert_eq!(
        bounded["historyOpen"].as_str(),
        bounded["continueWith"]["args"]["open"].as_str()
    );

    // The archive and its Trace pointer survive a process boundary and remain directly readable.
    drop(service);
    drop(storage);
    let restarted =
        StorageService::open(&fixture.path().join("storage.sqlite")).expect("restart storage");
    let restarted_trace = restarted
        .get_conversation_turn_trace("assistant-manual-command")
        .unwrap()
        .expect("trace after restart");
    assert!(matches!(
        restarted_trace.items.last(),
        Some(ConversationTurnTraceItem::ToolResult { archive, .. })
            if archive.archive_ref.as_deref() == Some(archive_ref.as_str())
                && archive.content_hash.as_deref() == Some(content_hash.as_str())
    ));
    let restarted_page = restarted
        .read_conversation_history_archive_page(
            "conversation-manual-command",
            &archive_ref,
            mycopilot_core::storage::conversation_history_archive_repository::ConversationHistoryArchivePageUnit::Char,
            0,
            archive.total_chars,
        )
        .unwrap()
        .expect("archive page after restart");
    assert_eq!(restarted_page.content, archived_page.content);
    let restarted_model_log = restarted
        .get_conversation_model_context_log("assistant-manual-command")
        .unwrap()
        .expect("model log after restart");
    assert_eq!(
        restarted_model_log
            .items
            .iter()
            .find(|item| {
                item.tool_call_id.as_deref() == Some(successful_tool_result.call_id.as_str())
            })
            .expect("restarted approved Host ToolResult")
            .content,
        persisted_model_observation
    );
}

#[test]
fn terminal_event_gate_defers_settled_state_and_done_until_commit() {
    let gate = AgentTerminalEventGate::default();
    let message = AgentEvent::Message {
        run_id: "run-terminal-gate".to_string(),
        content: "still streaming".to_string(),
    };
    assert!(matches!(
        gate.route(message),
        Some(AgentEvent::Message { .. })
    ));

    assert!(gate
        .route(AgentEvent::State {
            run_id: "run-terminal-gate".to_string(),
            state: mycopilot_core::AgentStateSnapshot {
                status: AgentRunStatus::Completed,
                active_run_id: None,
                last_error: None,
                updated_at: 1,
            },
        })
        .is_none());
    assert!(gate
        .route(terminal_done_event(&completed_output_for_terminal_gate()))
        .is_none());

    let committed = gate.take_after_persistence(&completed_output_for_terminal_gate());
    assert!(matches!(committed.as_slice(), [
        AgentEvent::State { state, .. },
        AgentEvent::Done { success: true, .. }
    ] if state.status == AgentRunStatus::Completed));
}

#[test]
fn terminal_event_gate_replaces_deferred_segment_usage_with_run_total() {
    let gate = AgentTerminalEventGate::default();
    let mut output = completed_output_for_terminal_gate();
    output.usage = Some(AgentUsage {
        input_tokens: Some(160),
        output_tokens: Some(30),
        output_thinking_tokens: Some(12),
        total_tokens: Some(190),
        cached_input_tokens: Some(8),
        cache_creation_input_tokens: Some(2),
        billable_request_count: Some(3),
    });
    assert!(gate
        .route(AgentEvent::Done {
            run_id: output.run_id.clone(),
            success: true,
            status: Some(AgentRunStatus::Completed),
            content: Some(output.content.clone()),
            usage: Some(AgentUsage {
                input_tokens: Some(60),
                output_tokens: Some(10),
                output_thinking_tokens: Some(4),
                total_tokens: Some(70),
                cached_input_tokens: Some(3),
                cache_creation_input_tokens: Some(2),
                billable_request_count: Some(1),
            }),
            finish_reason: Some("stop".to_string()),
            proposed_actions: Vec::new(),
        })
        .is_none());

    let committed = gate.take_after_persistence(&output);
    assert!(matches!(
        committed.as_slice(),
        [AgentEvent::Done { usage, .. }]
            if usage == &output.usage
    ));
}

#[test]
fn terminal_event_gate_never_exposes_a_nonrecoverable_error_before_commit() {
    let gate = AgentTerminalEventGate::default();
    assert!(gate
        .route(AgentEvent::Error {
            run_id: Some("run-terminal-error-gate".to_string()),
            trace_sequence: None,
            message: "terminal model failure".to_string(),
            recoverable: false,
            code: Some("terminal_model_failure".to_string()),
            details: None,
        })
        .is_none());
    assert!(matches!(
        gate.route(AgentEvent::Error {
            run_id: Some("run-terminal-error-gate".to_string()),
            trace_sequence: None,
            message: "retryable persistence failure".to_string(),
            recoverable: true,
            code: Some("persistence_failure".to_string()),
            details: None,
        }),
        Some(AgentEvent::Error {
            recoverable: true,
            ..
        })
    ));
    gate.discard();
    let released = gate.take_after_persistence(&completed_output_for_terminal_gate());
    assert!(released.iter().all(|event| !matches!(
        event,
        AgentEvent::Error {
            recoverable: false,
            ..
        }
    )));
}

#[test]
fn runtime_terminal_error_keeps_its_durable_trace_sequence_across_commit_and_reload() {
    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("runtime-error-sequence.sqlite");
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    let service = AgentService::new(Arc::clone(&storage));
    let conversation_id = "conversation-runtime-error-sequence";
    let assistant_message_id = "assistant-runtime-error-sequence";
    let run_id = "run-runtime-error-sequence";
    let message = "tool iteration limit reached";
    storage
        .save_conversation(ChatConversationRecord {
            id: conversation_id.to_string(),
            project_id: None,
            model_id: Some("test-model".to_string()),
            title: "Runtime error trace sequence".to_string(),
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
    let trace = ConversationTurnTrace {
        schema_version: mycopilot_core::CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
        run_id: run_id.to_string(),
        conversation_id: conversation_id.to_string(),
        assistant_message_id: assistant_message_id.to_string(),
        terminal_status: ConversationTurnTraceTerminalStatus::Failed,
        terminal_error: Some(message.to_string()),
        truncated: false,
        items: vec![ConversationTurnTraceItem::RuntimeError {
            sequence: 0,
            message: message.to_string(),
            recoverable: false,
            code: Some("iteration_limit".to_string()),
            truncated: false,
        }],
    };
    let gate = AgentTerminalEventGate::default();
    assert!(gate
        .route(AgentEvent::Error {
            run_id: Some(run_id.to_string()),
            trace_sequence: Some(0),
            message: message.to_string(),
            recoverable: false,
            code: Some("iteration_limit".to_string()),
            details: None,
        })
        .is_none());

    service
        .persist_assistant_error(conversation_id, assistant_message_id, message, None, &trace)
        .unwrap();
    let released = gate
        .take_error_after_persistence()
        .expect("durable Runtime error should be released after commit");
    assert!(matches!(
        released,
        AgentEvent::Error {
            trace_sequence: Some(0),
            recoverable: false,
            ..
        }
    ));
    assert!(gate.take_error_after_persistence().is_none());
    drop(service);
    drop(storage);

    let reopened = StorageService::open(&database_path).unwrap();
    let conversation = reopened
        .load_conversation(conversation_id)
        .unwrap()
        .unwrap();
    let run: Value = serde_json::from_str(
        conversation.messages[0]
            .agent_run_json
            .as_deref()
            .expect("failed trace should rebuild the observer run"),
    )
    .unwrap();
    assert_eq!(
        run["timeline"],
        json!([{
            "id": "trace-error-0",
            "type": "error",
            "message": message,
            "traceSequence": 0
        }])
    );
}

#[test]
fn terminal_event_gate_does_not_delay_approval_waiting_done() {
    let gate = AgentTerminalEventGate::default();
    let routed = gate.route(AgentEvent::Done {
        run_id: "run-terminal-gate".to_string(),
        success: false,
        status: Some(AgentRunStatus::WaitingForApproval),
        content: None,
        usage: None,
        finish_reason: None,
        proposed_actions: Vec::new(),
    });

    assert!(matches!(
        routed,
        Some(AgentEvent::Done {
            status: Some(AgentRunStatus::WaitingForApproval),
            ..
        })
    ));
}

#[test]
fn assistant_persistence_failure_never_releases_pending_terminal_done() {
    assert!(!pending_terminal_commit_is_publishable(false, true, true));
    assert!(!pending_terminal_commit_is_publishable(true, false, true));
    assert!(!pending_terminal_commit_is_publishable(true, true, false));
    assert!(pending_terminal_commit_is_publishable(true, true, true));
    assert!(ensure_pending_status_transition(
        PendingActionStatus::Cancelled,
        PendingActionStatus::Completed,
    )
    .is_err());
}
