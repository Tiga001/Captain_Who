use super::*;

#[test]
fn automatic_and_explicit_user_server_paths_use_distinct_authorization_sources() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let service = AgentService::new(storage);
    let input = command_test_input(fixture.path());

    let automatic_request = command_request("automatic-command", "mkdir automatic-blocked");
    let automatic_result = service
        .execute_auto_approved_action(
            input.clone(),
            "run-automatic-command".to_string(),
            None,
            None,
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

    let mut full_access_input = input.clone();
    full_access_input
        .context
        .as_mut()
        .expect("command test context")
        .permissions
        .command_safety = mycopilot_core::AgentCommandSafetyPolicy::FullAccess;
    let full_access_request =
        command_request("automatic-full-access", "mkdir automatic-full-access");
    let full_access_result = service
        .execute_auto_approved_action(
            full_access_input,
            "run-automatic-full-access".to_string(),
            None,
            None,
            AgentProposedAction::Command {
                command: full_access_request,
            },
            AgentCancellationToken::new(),
        )
        .unwrap();

    assert!(full_access_result.ok);
    assert!(fixture.path().join("automatic-full-access").is_dir());

    let explicit_request = command_request("explicit-command", "mkdir explicit-allowed");
    let record = PendingActionRecord {
        storage_id: pending_action_storage_id("run-explicit-command", &explicit_request.id),
        snapshot: PendingAgentActionSnapshot {
            action_id: explicit_request.id.clone(),
            action_type: "command".to_string(),
            tool_name: "run_command".to_string(),
            tool_call_id: Some(explicit_request.id.clone()),
            run_id: "run-explicit-command".to_string(),
            conversation_id: Some("conversation-command-policy".to_string()),
            assistant_message_id: Some("assistant-command-policy".to_string()),
            action: AgentProposedAction::Command {
                command: explicit_request,
            },
            created_at: 1,
            status: PendingActionStatus::Approved,
        },
        agent_input: input,
    };
    let explicit_result =
        run_explicitly_approved_command_from_snapshot(&record, AgentCancellationToken::new(), None)
            .unwrap();

    assert_eq!(explicit_result.exit_code, Some(0));
    assert!(fixture.path().join("explicit-allowed").is_dir());
}

#[test]
fn explicit_user_execution_is_bound_to_the_backend_action_snapshot() {
    let fixture = tempdir().unwrap();
    let frozen_request = command_request("frozen-command", "mkdir frozen-snapshot");
    let record = PendingActionRecord {
        storage_id: pending_action_storage_id("run-frozen-command", &frozen_request.id),
        snapshot: PendingAgentActionSnapshot {
            action_id: frozen_request.id.clone(),
            action_type: "command".to_string(),
            tool_name: "run_command".to_string(),
            tool_call_id: Some(frozen_request.id.clone()),
            run_id: "run-frozen-command".to_string(),
            conversation_id: Some("conversation-frozen-command".to_string()),
            assistant_message_id: Some("assistant-frozen-command".to_string()),
            action: AgentProposedAction::Command {
                command: frozen_request,
            },
            created_at: 1,
            status: PendingActionStatus::Approved,
        },
        agent_input: command_test_input(fixture.path()),
    };
    let untrusted_replacement_call = AgentToolCall {
        id: "frozen-command".to_string(),
        tool: "run_command".to_string(),
        args: json!({ "command": "mkdir untrusted-replacement" }),
        approval_status: AgentApprovalStatus::Approved,
        reason: None,
    };
    assert_eq!(
        untrusted_replacement_call.args["command"],
        "mkdir untrusted-replacement"
    );

    let result =
        run_explicitly_approved_command_from_snapshot(&record, AgentCancellationToken::new(), None)
            .unwrap();

    assert_eq!(result.exit_code, Some(0));
    assert!(fixture.path().join("frozen-snapshot").is_dir());
    assert!(!fixture.path().join("untrusted-replacement").exists());
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
fn terminal_event_gate_never_exposes_a_nonrecoverable_error_before_commit() {
    let gate = AgentTerminalEventGate::default();
    assert!(gate
        .route(AgentEvent::Error {
            run_id: Some("run-terminal-error-gate".to_string()),
            message: "terminal model failure".to_string(),
            recoverable: false,
            code: Some("terminal_model_failure".to_string()),
            details: None,
        })
        .is_none());
    assert!(matches!(
        gate.route(AgentEvent::Error {
            run_id: Some("run-terminal-error-gate".to_string()),
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
