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
            AutoApprovedActionContext::new(
                input.clone(),
                "run-automatic-command".to_string(),
                None,
                None,
                None,
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
            AutoApprovedActionContext::new(
                full_access_input,
                "run-automatic-full-access".to_string(),
                None,
                None,
                None,
            ),
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
            AutoApprovedActionContext::new(input, run_id.to_string(), None, None, None),
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
    let context = AutoApprovedActionContext::new(input, run_id.to_string(), None, None, None);
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
    let context = AutoApprovedActionContext::new(input, run_id.to_string(), None, None, None);
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
    let execution = std::thread::spawn(move || {
        execution_service.execute_auto_approved_action(
            AutoApprovedActionContext::new(input, run_id.to_string(), None, None, None),
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
    let persisted = storage.list_agent_command_results_for_run(run_id).unwrap();
    assert_eq!(persisted.len(), 1);
    assert_eq!(
        persisted[0].artifact_observation.as_ref().unwrap().changes[0].path,
        "deletion-barrier.csv"
    );
    let result = execution.join().unwrap().unwrap();
    assert!(result.ok);
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
        "printf timeout-evidence > deletion-timeout.csv; sleep 0.6",
    );
    command.observe = Some(mycopilot_core::AgentCommandArtifactObservationRequest {
        kinds: vec![mycopilot_core::AgentCommandArtifactObservationKind::Office],
        expected_outputs: vec!["deletion-timeout.csv".to_string()],
        additional_roots: Vec::new(),
    });
    let execution_service = service.clone();
    let execution = std::thread::spawn(move || {
        execution_service.execute_auto_approved_action(
            AutoApprovedActionContext::new(input, run_id.to_string(), None, None, None),
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

    let deletion_error = service
        .delete_project("project-deletion-timeout")
        .unwrap_err();
    assert!(deletion_error.contains("did not finish execution and durable settlement"));
    assert!(!service.is_project_deleting(Some("project-deletion-timeout")));
    assert!(
        !execution.is_finished(),
        "a drain timeout must refuse deletion instead of pretending the command finished"
    );

    let result = execution.join().unwrap().unwrap();
    assert!(result.ok);
    let persisted = storage.list_agent_command_results_for_run(run_id).unwrap();
    assert_eq!(persisted.len(), 1);
    assert_eq!(
        persisted[0].artifact_observation.as_ref().unwrap().changes[0].path,
        "deletion-timeout.csv"
    );
}

#[test]
fn project_deletion_rejects_unsettled_command_effects() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let service = AgentService::new(storage);
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
            AutoApprovedActionContext::new(input, run_id.to_string(), None, None, None),
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
    let deletion_error = service
        .delete_project("project-unsettled-command")
        .unwrap_err();
    assert!(deletion_error.contains("without a confirmed durable terminal receipt"));
    assert!(deletion_error.contains(&pending_action_storage_id(run_id, call_id)));
    assert!(!service.is_project_deleting(Some("project-unsettled-command")));
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
                input_price: None,
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
            agent_input_json: "{}".to_string(),
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
            patch_result_json: None,
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
    let call = command_tool_call(&command);
    let checkpoint = AgentRunCheckpoint {
        version: AGENT_RUN_CHECKPOINT_SCHEMA_VERSION,
        run_id: run_id.to_string(),
        context_items: Vec::new(),
        next_model_request_index: 1,
        queued_tool_calls: Vec::new(),
        suppressed_narration: false,
        extension_snapshots: Vec::new(),
        tool_set: crate::test_tool_set_checkpoint(),
        run_context: None,
        model_capabilities: ModelCapabilities::default(),
        run_world_state: crate::test_run_world_state(),
        pending_tool_call_id: call.id.clone(),
        conversation_trace_items: vec![ConversationTurnTraceItem::ToolCall {
            sequence: 0,
            call_id: call.id.clone(),
            tool: call.tool.clone(),
            operation: call.args.clone(),
            approval_status: AgentApprovalStatus::Required,
            truncated: false,
        }],
        next_conversation_trace_sequence: 1,
        conversation_trace_truncated: false,
        model_visible_trace_item_count: 0,
    };
    let mut agent_input = command_test_input(fixture.path());
    let context = agent_input.context.as_mut().unwrap();
    context.conversation_id = Some("conversation-manual-command".to_string());
    agent_input.resume_checkpoint = Some(checkpoint);
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
    let command_result =
        run_explicitly_approved_command_from_snapshot(&record, AgentCancellationToken::new(), None)
            .unwrap();
    assert_eq!(command_result.exit_code, Some(0));
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
    assert!(matches!(
        trace.items.last(),
        Some(ConversationTurnTraceItem::ToolResult {
            call_id,
            success: true,
            ..
        }) if call_id == "manual-command-audit-failure"
    ));
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
