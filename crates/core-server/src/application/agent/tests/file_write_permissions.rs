use super::*;
use mycopilot_core::{AgentFileChangeOperation, AgentFileChangeResultStatus};
use std::path::Path;

fn test_input(permissions: AgentPermissions) -> AgentChatInput {
    let mut input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://should-not-be-called.test/v1/chat/completions",
        "apiToken": "secret",
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "messages": []
    }))
    .unwrap();
    input.context = Some(AgentRunContext {
        collaboration_identity: None,
        conversation_id: Some("conversation-file-policy".to_string()),
        project_id: None,
        workspace: Some(AgentWorkspaceContext {
            project_id: None,
            display_name: Some("file-policy-test".to_string()),
            root_path: Some("/tmp/file-policy-test".to_string()),
        }),
        attachment_library: None,
        permissions,
    });
    input
}

pub(super) fn direct_execution_input(
    workspace: &Path,
    run_id: &str,
    conversation_id: &str,
    permissions: AgentPermissions,
    call: &AgentToolCall,
) -> AgentChatInput {
    direct_execution_input_with_workspace(
        Some(workspace),
        run_id,
        conversation_id,
        permissions,
        call,
    )
}

fn direct_execution_input_without_workspace(
    run_id: &str,
    conversation_id: &str,
    permissions: AgentPermissions,
    call: &AgentToolCall,
) -> AgentChatInput {
    direct_execution_input_with_workspace(None, run_id, conversation_id, permissions, call)
}

fn direct_execution_input_with_workspace(
    workspace: Option<&Path>,
    run_id: &str,
    conversation_id: &str,
    permissions: AgentPermissions,
    call: &AgentToolCall,
) -> AgentChatInput {
    let mut input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://should-not-be-called.test/v1/chat/completions",
        "apiToken": "secret",
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "messages": []
    }))
    .unwrap();
    let context = AgentRunContext {
        collaboration_identity: None,
        conversation_id: Some(conversation_id.to_string()),
        project_id: None,
        workspace: workspace.map(|workspace| AgentWorkspaceContext {
            project_id: None,
            display_name: Some("direct-file-change-test".to_string()),
            root_path: Some(workspace.to_string_lossy().into_owned()),
        }),
        attachment_library: None,
        permissions,
    };
    let profile = crate::test_provider_profile_config();
    let protocol_key = crate::test_provider_protocol_key("test-model");
    input.context = Some(context.clone());
    input.provider_profile_config = Some(profile.clone());
    input.provider_protocol_key = Some(protocol_key.clone());
    input.resume_checkpoint = Some(AgentRunCheckpoint {
        version: AGENT_RUN_CHECKPOINT_SCHEMA_VERSION,
        run_id: run_id.to_string(),
        context_items: vec![mycopilot_core::AgentContextCheckpointItem {
            role: "assistant".to_string(),
            content: String::new(),
            images: Vec::new(),
            tool_call_id: None,
            tool_calls: vec![AgentContextCheckpointToolCall {
                id: call.id.clone(),
                name: call.tool.clone(),
                args: call.args.clone(),
                provider_identity: AgentProviderToolCallIdentity {
                    provider_tool_index: 0,
                    provider_call_id: call.id.clone(),
                    runtime_call_id: call.id.clone(),
                },
            }],
            is_error: false,
            sources: vec!["model_response".to_string()],
            scope: "conversation".to_string(),
            retention: "retained".to_string(),
            group: None,
            origin: None,
        }],
        next_model_request_index: 1,
        queued_tool_calls: Vec::new(),
        deferred_external_tool_call_count: 0,
        suppressed_narration: false,
        extension_snapshots: Vec::new(),
        tool_set: crate::test_tool_set_checkpoint(),
        run_context: Some(context),
        collaboration_run_snapshot: None,
        model_capabilities: ModelCapabilities::default(),
        provider_profile_config: profile,
        provider_protocol_key: protocol_key,
        assistant_turn_identity: crate::test_assistant_turn_identity(&[call.id.as_str()]),
        provider_continuation_refs: Vec::new(),
        run_world_state: crate::test_run_world_state(),
        pending_action_id: Some(pending_action_storage_id(run_id, &call.id)),
        pending_tool_call_id: call.id.clone(),
        conversation_trace_items: Vec::new(),
        conversation_model_context_items: Vec::new(),
        next_conversation_trace_sequence: 0,
        conversation_trace_truncated: false,
    });
    input
}

pub(super) fn bind_direct_execution_to_input(
    action: &mut AgentProposedAction,
    input: &AgentChatInput,
) {
    let AgentProposedAction::FileChange { file_change } = action else {
        panic!("Direct execution fixture must be a FileChange action");
    };
    file_change.execution.permission_revision =
        mycopilot_core::file_change::proposal_digest(&permissions_from_input(input))
            .expect("digest the exact frozen permission snapshot");
    file_change.execution.tool_set_revision = input
        .resume_checkpoint
        .as_ref()
        .expect("Direct execution fixture has a frozen checkpoint")
        .tool_set
        .effective_revision
        .clone();
    file_change.execution.provider_wire_revision = input
        .provider_configuration_revision
        .clone()
        .or_else(|| {
            input.provider_protocol_key.as_ref().map(|protocol| {
                mycopilot_core::file_change::proposal_digest(protocol)
                    .expect("digest the exact Provider wire snapshot")
            })
        })
        .expect("Direct execution fixture has Provider wire identity");
    file_change
        .execution
        .validate()
        .expect("Direct execution binding remains internally valid");
}

pub(super) fn seed_durable_direct_file_change_owner(
    storage: &StorageService,
    input: &mut AgentChatInput,
    conversation_id: &str,
    assistant_message_id: &str,
    run_id: &str,
    call: &AgentToolCall,
) {
    let created_at = 1;
    storage
        .save_conversation(ChatConversationRecord {
            id: conversation_id.to_string(),
            project_id: None,
            model_id: Some("test-model".to_string()),
            title: "Direct FileChange crash boundary".to_string(),
            messages: vec![ChatMessageRecord {
                id: assistant_message_id.to_string(),
                role: "assistant".to_string(),
                content: String::new(),
                created_at,
                status: Some("pending".to_string()),
                attachments: Vec::new(),
                agent_run_json: None,
                ui_state_json: None,
            }],
            created_at,
            updated_at: created_at,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();

    let provider_identity = AgentProviderToolCallIdentity {
        provider_tool_index: 0,
        provider_call_id: call.id.clone(),
        runtime_call_id: call.id.clone(),
    };
    let trace = mycopilot_core::ConversationTurnTrace {
        schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
        run_id: run_id.to_string(),
        conversation_id: conversation_id.to_string(),
        assistant_message_id: assistant_message_id.to_string(),
        terminal_status: ConversationTurnTraceTerminalStatus::InProgress,
        terminal_error: None,
        truncated: false,
        items: vec![ConversationTurnTraceItem::ToolCall {
            sequence: 0,
            call_id: call.id.clone(),
            tool: call.tool.clone(),
            provenance: AgentToolIdentity::Builtin {
                tool_name: "apply_patch".to_string(),
            },
            operation: call.args.clone(),
            approval_status: call.approval_status,
            truncated: false,
        }],
    };
    let model_context = vec![ConversationModelContextItem {
        sequence: 0,
        ordinal: 0,
        role: "assistant".to_string(),
        content: String::new(),
        tool_call_id: None,
        tool_calls: vec![AgentContextCheckpointToolCall {
            id: call.id.clone(),
            name: call.tool.clone(),
            args: call.args.clone(),
            provider_identity,
        }],
        is_error: false,
    }];
    let checkpoint = input
        .resume_checkpoint
        .as_mut()
        .expect("Direct crash fixture has a frozen checkpoint");
    checkpoint.conversation_trace_items = trace.items.clone();
    checkpoint.conversation_model_context_items = model_context.clone();
    checkpoint.next_conversation_trace_sequence = 1;
    storage
        .append_in_progress_conversation_turn_trace_and_apply_guidances(
            &trace,
            &model_context,
            created_at,
            created_at,
        )
        .unwrap();
}

#[allow(clippy::too_many_arguments)]
fn store_manual_direct_create(
    service: &AgentService,
    storage: &Arc<StorageService>,
    workspace: &Path,
    run_id: &str,
    conversation_id: &str,
    assistant_message_id: &str,
    call_id: &str,
    file_name: &str,
    content: &str,
) -> AgentToolCall {
    let target = workspace.join(file_name);
    let (mut action, call) = direct_file_change_fixture(
        run_id,
        conversation_id,
        call_id,
        (file_name, target.to_str().unwrap()),
        None,
        Some(content),
        AgentApprovalStatus::Required,
    );
    let mut input = direct_execution_input(
        workspace,
        run_id,
        conversation_id,
        AgentPermissions {
            write: AgentWritePermission::WorkspaceOnly,
            patch: mycopilot_core::AgentPatchPermission::RequireApproval,
            ..Default::default()
        },
        &call,
    );
    save_test_pending_provider_for_input(storage, &mut input);
    bind_direct_execution_to_input(&mut action, &input);
    seed_durable_direct_file_change_owner(
        storage,
        &mut input,
        conversation_id,
        assistant_message_id,
        run_id,
        &call,
    );
    assert!(service
        .store_pending_action(run_id, conversation_id, assistant_message_id, action, input,)
        .unwrap());
    call
}

fn pending_file_change_record(
    run_id: &str,
    conversation_id: &str,
    action: AgentProposedAction,
    call: &AgentToolCall,
    agent_input: AgentChatInput,
) -> PendingActionRecord {
    PendingActionRecord {
        storage_id: pending_action_storage_id(run_id, &call.id),
        snapshot: PendingAgentActionSnapshot {
            action_id: call.id.clone(),
            action_type: "file_change".to_string(),
            tool_name: call.tool.clone(),
            tool_call_id: Some(call.id.clone()),
            run_id: run_id.to_string(),
            conversation_id: Some(conversation_id.to_string()),
            assistant_message_id: Some(format!("assistant-{run_id}")),
            action,
            created_at: 1,
            status: PendingActionStatus::Pending,
        },
        agent_input,
    }
}

fn assert_direct_change_applied(
    decision: &ActionExecutionDecision,
    operation: AgentFileChangeOperation,
) {
    let result = decision
        .file_change_result
        .as_ref()
        .expect("Direct file change returns a FileChange result");
    assert_eq!(
        decision.status, "applied",
        "Direct execution failed: code={:?}, error={:?}",
        result.error_code, result.error
    );
    assert_eq!(
        decision.final_pending_status,
        PendingActionStatus::Completed
    );
    assert_eq!(result.status, AgentFileChangeResultStatus::Applied);
    assert_eq!(result.operation, operation);
    assert!(decision.file_change.is_some());
    assert!(decision.tool_result.ok);
}

fn staged_file_change_action(approval_status: AgentApprovalStatus) -> AgentProposedAction {
    let (file_change, _) = staged_file_change_fixture(
        StagedFileChangeFixtureIdentity {
            run_id: "run-file-policy",
            conversation_id: "conversation-file-policy",
            call_id: "write-1",
            transaction_id: "draft-1",
        },
        ("report.txt", "/tmp/file-policy-test/report.txt"),
        "hello",
        approval_status,
    );
    AgentProposedAction::FileChange { file_change }
}

fn apply_patch_action(approval_status: AgentApprovalStatus) -> AgentProposedAction {
    direct_file_change_fixture(
        "run-file-policy",
        "conversation-file-policy",
        "patch-1",
        ("report.txt", "/tmp/file-policy-test/report.txt"),
        None,
        Some("hello\n"),
        approval_status,
    )
    .0
}

fn structured_file_write_actions(approval_status: AgentApprovalStatus) -> [AgentProposedAction; 2] {
    [
        apply_patch_action(approval_status),
        staged_file_change_action(approval_status),
    ]
}

fn assert_structured_file_write_authorization(
    input: &AgentChatInput,
    approval_status: AgentApprovalStatus,
    source: FileWriteAuthorizationSource,
    allowed: bool,
) {
    for action in structured_file_write_actions(approval_status) {
        assert_eq!(
            authorize_structured_file_write(input, &action, source).is_ok(),
            allowed
        );
    }
}

#[test]
fn automatic_host_writes_require_auto_approve_and_an_approved_snapshot() {
    let manual = test_input(AgentPermissions {
        write: AgentWritePermission::WorkspaceOnly,
        patch: mycopilot_core::AgentPatchPermission::RequireApproval,
        ..Default::default()
    });
    assert_structured_file_write_authorization(
        &manual,
        AgentApprovalStatus::Approved,
        FileWriteAuthorizationSource::Automatic,
        false,
    );

    let automatic = test_input(AgentPermissions {
        write: AgentWritePermission::WorkspaceOnly,
        patch: mycopilot_core::AgentPatchPermission::AutoApprove,
        ..Default::default()
    });
    assert_structured_file_write_authorization(
        &automatic,
        AgentApprovalStatus::Approved,
        FileWriteAuthorizationSource::Automatic,
        true,
    );
    assert_structured_file_write_authorization(
        &automatic,
        AgentApprovalStatus::Required,
        FileWriteAuthorizationSource::Automatic,
        false,
    );
}

#[test]
fn explicit_user_approval_authorizes_manual_writes_but_never_overrides_write_denied() {
    let manual = test_input(AgentPermissions {
        write: AgentWritePermission::WorkspaceOnly,
        patch: mycopilot_core::AgentPatchPermission::RequireApproval,
        ..Default::default()
    });
    assert_structured_file_write_authorization(
        &manual,
        AgentApprovalStatus::Required,
        FileWriteAuthorizationSource::ExplicitUser,
        true,
    );

    let denied = test_input(AgentPermissions {
        write: AgentWritePermission::Denied,
        patch: mycopilot_core::AgentPatchPermission::AutoApprove,
        ..Default::default()
    });
    assert_structured_file_write_authorization(
        &denied,
        AgentApprovalStatus::Approved,
        FileWriteAuthorizationSource::Automatic,
        false,
    );
    assert_structured_file_write_authorization(
        &denied,
        AgentApprovalStatus::Required,
        FileWriteAuthorizationSource::ExplicitUser,
        false,
    );
}

#[test]
fn process_actions_remain_in_their_separate_command_policy_domain() {
    let denied = test_input(AgentPermissions::default());
    let command = AgentProposedAction::Command {
        command: command_request("command-1", "pwd"),
    };

    assert!(authorize_structured_file_write(
        &denied,
        &command,
        FileWriteAuthorizationSource::Automatic,
    )
    .is_ok());
}

#[test]
fn direct_create_update_delete_share_the_committer_across_auto_and_manual_approval() {
    let fixture = tempdir().unwrap();
    let workspace = fixture.path().join("workspace");
    fs::create_dir(&workspace).unwrap();
    let workspace = fs::canonicalize(workspace).unwrap();
    let storage = StorageService::open(&fixture.path().join("storage.sqlite")).unwrap();
    let target = workspace.join("report.txt");
    let canonical_target = target.to_string_lossy().into_owned();
    let conversation_id = "conversation-direct-file-commit";

    let automatic_permissions = AgentPermissions {
        write: AgentWritePermission::WorkspaceOnly,
        patch: mycopilot_core::AgentPatchPermission::AutoApprove,
        ..Default::default()
    };
    let create_run_id = "run-direct-create-auto";
    let (mut create_action, create_call) = direct_file_change_fixture(
        create_run_id,
        conversation_id,
        "call-direct-create-auto",
        ("report.txt", &canonical_target),
        None,
        Some("version one\n"),
        AgentApprovalStatus::Approved,
    );
    let create_input = direct_execution_input(
        &workspace,
        create_run_id,
        conversation_id,
        automatic_permissions,
        &create_call,
    );
    bind_direct_execution_to_input(&mut create_action, &create_input);
    authorize_structured_file_write(
        &create_input,
        &create_action,
        FileWriteAuthorizationSource::Automatic,
    )
    .unwrap();
    let AgentProposedAction::FileChange {
        file_change: create,
    } = &create_action
    else {
        unreachable!()
    };
    let create_decision = approved_direct_file_change_execution_for_input(
        &create_input,
        create_run_id,
        &create_call.id,
        create,
    );
    assert_direct_change_applied(&create_decision, AgentFileChangeOperation::Create);
    assert_eq!(fs::read_to_string(&target).unwrap(), "version one\n");

    let manual_permissions = AgentPermissions {
        write: AgentWritePermission::WorkspaceOnly,
        patch: mycopilot_core::AgentPatchPermission::RequireApproval,
        ..Default::default()
    };
    let update_run_id = "run-direct-update-manual";
    let (mut update_action, update_call) = direct_file_change_fixture(
        update_run_id,
        conversation_id,
        "call-direct-update-manual",
        ("report.txt", &canonical_target),
        Some("version one\n"),
        Some("version two\n"),
        AgentApprovalStatus::Required,
    );
    let update_input = direct_execution_input(
        &workspace,
        update_run_id,
        conversation_id,
        manual_permissions,
        &update_call,
    );
    bind_direct_execution_to_input(&mut update_action, &update_input);
    authorize_structured_file_write(
        &update_input,
        &update_action,
        FileWriteAuthorizationSource::ExplicitUser,
    )
    .unwrap();
    let update_record = pending_file_change_record(
        update_run_id,
        conversation_id,
        update_action,
        &update_call,
        update_input,
    );
    let restored_update_call = tool_call_for_pending_record(&update_record).unwrap();
    assert_eq!(restored_update_call.args, update_call.args);
    let update_decision = action_execution_for_decision(
        &storage,
        &update_record,
        &restored_update_call,
        AgentApprovalDecisionStatus::Approved,
        None,
    );
    assert_direct_change_applied(&update_decision, AgentFileChangeOperation::Update);
    assert_eq!(fs::read_to_string(&target).unwrap(), "version two\n");

    let delete_run_id = "run-direct-delete-auto";
    let (delete_action, delete_call) = direct_file_change_fixture(
        delete_run_id,
        conversation_id,
        "call-direct-delete-auto",
        ("report.txt", &canonical_target),
        Some("version two\n"),
        None,
        AgentApprovalStatus::Approved,
    );
    let encoded_delete = serde_json::to_string(&delete_action).unwrap();
    let mut delete_action: AgentProposedAction = serde_json::from_str(&encoded_delete).unwrap();
    let AgentProposedAction::FileChange { file_change } = &delete_action else {
        unreachable!()
    };
    assert!(file_change.execution.delete_journal.is_some());
    let delete_input = direct_execution_input(
        &workspace,
        delete_run_id,
        conversation_id,
        automatic_permissions,
        &delete_call,
    );
    bind_direct_execution_to_input(&mut delete_action, &delete_input);
    authorize_structured_file_write(
        &delete_input,
        &delete_action,
        FileWriteAuthorizationSource::Automatic,
    )
    .unwrap();
    let AgentProposedAction::FileChange {
        file_change: delete,
    } = &delete_action
    else {
        unreachable!()
    };
    let delete_decision = approved_direct_file_change_execution_for_input(
        &delete_input,
        delete_run_id,
        &delete_call.id,
        delete,
    );
    assert_direct_change_applied(&delete_decision, AgentFileChangeOperation::Delete);
    assert!(!target.exists());
    assert!(fs::read_dir(&workspace).unwrap().next().is_none());
}

#[test]
fn no_workspace_absolute_direct_create_executes_with_full_write_scope() {
    let fixture = tempdir().unwrap();
    let target = fixture
        .path()
        .canonicalize()
        .unwrap()
        .join("absolute-direct.txt");
    let canonical_target = target.to_string_lossy().into_owned();
    let run_id = "run-direct-no-workspace";
    let conversation_id = "conversation-direct-no-workspace";
    let (mut action, call) = direct_file_change_fixture(
        run_id,
        conversation_id,
        "call-direct-no-workspace",
        (&canonical_target, &canonical_target),
        None,
        Some("created without a workspace\n"),
        AgentApprovalStatus::Approved,
    );
    let input = direct_execution_input_without_workspace(
        run_id,
        conversation_id,
        AgentPermissions {
            write: AgentWritePermission::All,
            patch: mycopilot_core::AgentPatchPermission::AutoApprove,
            ..Default::default()
        },
        &call,
    );
    bind_direct_execution_to_input(&mut action, &input);
    authorize_structured_file_write(&input, &action, FileWriteAuthorizationSource::Automatic)
        .unwrap();
    let AgentProposedAction::FileChange { file_change } = &action else {
        unreachable!()
    };

    let decision =
        approved_direct_file_change_execution_for_input(&input, run_id, &call.id, file_change);

    assert_direct_change_applied(&decision, AgentFileChangeOperation::Create);
    assert_eq!(
        fs::read_to_string(target).unwrap(),
        "created without a workspace\n"
    );
}

#[test]
fn automatic_direct_outcome_unknown_keeps_the_claim_executing_without_a_tool_result() {
    let fixture = tempdir().unwrap();
    let workspace = fixture.path().join("workspace");
    fs::create_dir(&workspace).unwrap();
    let workspace = fs::canonicalize(workspace).unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let service = AgentService::new(Arc::clone(&storage));
    let target = workspace.join("uncertain.txt");
    let run_id = "run-direct-outcome-unknown";
    let conversation_id = "conversation-direct-outcome-unknown";
    let call_id = "call-direct-outcome-unknown";
    let assistant_message_id = "assistant-direct-outcome-unknown";
    let (mut action, call) = direct_file_change_fixture(
        run_id,
        conversation_id,
        call_id,
        ("uncertain.txt", target.to_str().unwrap()),
        None,
        Some("possibly published\n"),
        AgentApprovalStatus::Approved,
    );
    let mut input = direct_execution_input(
        &workspace,
        run_id,
        conversation_id,
        AgentPermissions {
            write: AgentWritePermission::WorkspaceOnly,
            patch: mycopilot_core::AgentPatchPermission::AutoApprove,
            ..Default::default()
        },
        &call,
    );
    save_test_pending_provider_for_input(&storage, &mut input);
    bind_direct_execution_to_input(&mut action, &input);
    seed_durable_direct_file_change_owner(
        &storage,
        &mut input,
        conversation_id,
        assistant_message_id,
        run_id,
        &call,
    );
    inject_direct_file_change_outcome_unknown(call_id);

    let error = service
        .execute_auto_approved_action(
            AutoApprovedActionContext::new(
                input,
                run_id.to_string(),
                Some(conversation_id.to_string()),
                Some(assistant_message_id.to_string()),
                None,
            ),
            action,
            AgentCancellationToken::new(),
        )
        .unwrap_err();

    assert_eq!(error.code(), Some("agent.apply_patch.recovery_pending"));
    assert!(!target.exists());
    let claims = storage.list_executing_file_change_action_audits().unwrap();
    assert_eq!(claims.len(), 1);
    assert_eq!(claims[0].status, "executing");
    assert!(claims[0].tool_result_json.is_none());

    drop(service);
    let _restarted = AgentService::try_new_deferred_startup_reconciliation(Arc::clone(&storage))
        .expect("the frozen missing Base must settle as definitely not executed");
    assert!(!target.exists());
    let durable = storage
        .get_pending_agent_action(&pending_action_storage_id(run_id, call_id))
        .unwrap()
        .unwrap();
    assert_eq!(durable.status, "failed");
    let results = storage
        .list_agent_tool_results_for_run(run_id, "apply_patch")
        .unwrap();
    assert_eq!(results.len(), 1);
    assert!(!results[0].ok);
    assert_eq!(
        results[0].result.as_ref().unwrap()["outcome"],
        "definitely_not_executed"
    );
    let trace = storage
        .get_conversation_turn_trace(assistant_message_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        trace
            .items
            .iter()
            .filter(|item| matches!(
                item,
                ConversationTurnTraceItem::ToolResult { call_id: durable_call_id, .. }
                    if durable_call_id == call_id
            ))
            .count(),
        1
    );
}

#[test]
fn automatic_direct_post_commit_binding_failure_recovers_without_replaying() {
    let fixture = tempdir().unwrap();
    let workspace = fixture.path().join("workspace");
    fs::create_dir(&workspace).unwrap();
    let workspace = fs::canonicalize(workspace).unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let service = AgentService::new(Arc::clone(&storage));
    let target = workspace.join("post-commit-binding.txt");
    let run_id = "run-direct-post-commit-binding";
    let conversation_id = "conversation-direct-post-commit-binding";
    let call_id = "call-direct-post-commit-binding";
    let (mut action, call) = direct_file_change_fixture(
        run_id,
        conversation_id,
        call_id,
        ("post-commit-binding.txt", target.to_str().unwrap()),
        None,
        Some("published before binding failure\n"),
        AgentApprovalStatus::Approved,
    );
    let mut input = direct_execution_input(
        &workspace,
        run_id,
        conversation_id,
        AgentPermissions {
            write: AgentWritePermission::WorkspaceOnly,
            patch: mycopilot_core::AgentPatchPermission::AutoApprove,
            ..Default::default()
        },
        &call,
    );
    save_test_pending_provider_for_input(&storage, &mut input);
    bind_direct_execution_to_input(&mut action, &input);
    let assistant_message_id = "assistant-direct-post-commit-binding";
    seed_durable_direct_file_change_owner(
        &storage,
        &mut input,
        conversation_id,
        assistant_message_id,
        run_id,
        &call,
    );
    inject_direct_file_change_post_commit_binding_failure(call_id);

    let error = service
        .execute_auto_approved_action(
            AutoApprovedActionContext::new(
                input,
                run_id.to_string(),
                Some(conversation_id.to_string()),
                Some(assistant_message_id.to_string()),
                None,
            ),
            action,
            AgentCancellationToken::new(),
        )
        .unwrap_err();

    assert_eq!(error.code(), Some("agent.apply_patch.recovery_pending"));
    assert_eq!(
        fs::read_to_string(&target).unwrap(),
        "published before binding failure\n"
    );
    let published_metadata = fs::metadata(&target).unwrap();
    let claims = storage.list_executing_file_change_action_audits().unwrap();
    assert_eq!(claims.len(), 1);
    assert!(claims[0].tool_result_json.is_none());

    assert_eq!(
        reconcile_interrupted_file_changes(&storage, mycopilot_core::storage::now_ms(),).unwrap(),
        1
    );
    assert!(storage
        .list_executing_file_change_action_audits()
        .unwrap()
        .is_empty());
    let recovered = storage
        .list_agent_tool_results_for_run(run_id, "apply_patch")
        .unwrap();
    assert_eq!(recovered.len(), 1);
    assert!(recovered[0].ok);
    let recovered_metadata = fs::metadata(&target).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        assert_eq!(published_metadata.dev(), recovered_metadata.dev());
        assert_eq!(published_metadata.ino(), recovered_metadata.ino());
    }
    #[cfg(not(unix))]
    let _ = (published_metadata, recovered_metadata);
}

#[test]
fn automatic_direct_reconciles_a_terminal_receipt_after_post_commit_error() {
    let fixture = tempdir().unwrap();
    let workspace = fixture.path().join("workspace");
    fs::create_dir(&workspace).unwrap();
    let workspace = fs::canonicalize(workspace).unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let service = AgentService::new(Arc::clone(&storage));
    let target = workspace.join("post-commit.txt");
    let run_id = "run-direct-post-commit-reconciliation";
    let conversation_id = "conversation-direct-post-commit-reconciliation";
    let call_id = "call-direct-post-commit-reconciliation";
    let assistant_message_id = "assistant-direct-post-commit-reconciliation";
    let (mut action, call) = direct_file_change_fixture(
        run_id,
        conversation_id,
        call_id,
        ("post-commit.txt", target.to_str().unwrap()),
        None,
        Some("published exactly once\n"),
        AgentApprovalStatus::Approved,
    );
    let mut input = direct_execution_input(
        &workspace,
        run_id,
        conversation_id,
        AgentPermissions {
            write: AgentWritePermission::WorkspaceOnly,
            patch: mycopilot_core::AgentPatchPermission::AutoApprove,
            ..Default::default()
        },
        &call,
    );
    save_test_pending_provider_for_input(&storage, &mut input);
    bind_direct_execution_to_input(&mut action, &input);
    seed_durable_direct_file_change_owner(
        &storage,
        &mut input,
        conversation_id,
        assistant_message_id,
        run_id,
        &call,
    );
    // Exhaust both exact commit-unknown adoption attempts to model a process loss after the
    // atomic receipt + Timeline boundary but before the Pending status transition/ToolResult
    // return. Both injections observe the same already-committed receipt.
    inject_auto_action_audit_post_commit_failure(run_id, call_id, "completed");
    inject_auto_action_audit_post_commit_failure(run_id, call_id, "completed");

    let error = service
        .execute_auto_approved_action(
            AutoApprovedActionContext::new(
                input,
                run_id.to_string(),
                Some(conversation_id.to_string()),
                Some(assistant_message_id.to_string()),
                None,
            ),
            action,
            AgentCancellationToken::new(),
        )
        .unwrap_err();

    assert_eq!(error.code(), Some("agent.apply_patch.recovery_pending"));
    assert_eq!(
        fs::read_to_string(&target).unwrap(),
        "published exactly once\n"
    );
    let published_metadata = fs::metadata(&target).unwrap();
    let storage_id = pending_action_storage_id(run_id, call_id);
    let durable = storage
        .get_pending_agent_action(&storage_id)
        .unwrap()
        .unwrap();
    assert_eq!(durable.status, "executing");
    assert_eq!(durable.target_status.as_deref(), Some("completed"));
    let trace = storage
        .get_conversation_turn_trace(assistant_message_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        trace
            .items
            .iter()
            .filter(|item| matches!(
                item,
                ConversationTurnTraceItem::ToolResult { call_id: durable_call_id, .. }
                    if durable_call_id == call_id
            ))
            .count(),
        1,
        "the terminal receipt and exact ToolResult Timeline must commit atomically"
    );
    let audited = storage
        .list_agent_tool_results_for_run(run_id, "apply_patch")
        .unwrap();
    assert_eq!(audited.len(), 1);
    assert_eq!(audited[0].call_id, call_id);
    assert_eq!(audited[0].tool, "apply_patch");
    assert!(audited[0].ok);
    assert!(storage
        .list_executing_file_change_action_audits()
        .unwrap()
        .is_empty());

    let _restarted = AgentService::try_new_deferred_startup_reconciliation(Arc::clone(&storage))
        .expect("the authoritative FileChange receipt must settle after restart");
    assert_eq!(
        storage
            .get_pending_agent_action(&storage_id)
            .unwrap()
            .unwrap()
            .status,
        "completed"
    );
    assert_eq!(
        storage
            .list_agent_tool_results_for_run(run_id, "apply_patch")
            .unwrap()
            .len(),
        1,
        "startup settlement must not duplicate the ToolResult receipt"
    );
    let recovered_trace = storage
        .get_conversation_turn_trace(assistant_message_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        recovered_trace
            .items
            .iter()
            .filter(|item| matches!(
                item,
                ConversationTurnTraceItem::ToolResult { call_id: durable_call_id, .. }
                    if durable_call_id == call_id
            ))
            .count(),
        1
    );
    let recovered_metadata = fs::metadata(&target).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        assert_eq!(published_metadata.dev(), recovered_metadata.dev());
        assert_eq!(published_metadata.ino(), recovered_metadata.ino());
    }
    #[cfg(not(unix))]
    let _ = (published_metadata, recovered_metadata);
}

#[test]
fn manual_direct_durable_dispatch_before_publication_recovers_as_not_executed() {
    let fixture = tempdir().unwrap();
    let workspace = fixture.path().join("workspace");
    fs::create_dir(&workspace).unwrap();
    let workspace = fs::canonicalize(workspace).unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let service = AgentService::new(Arc::clone(&storage));
    let run_id = "run-manual-direct-pre-effect";
    let conversation_id = "conversation-manual-direct-pre-effect";
    let assistant_message_id = "assistant-manual-direct-pre-effect";
    let call_id = "call-manual-direct-pre-effect";
    let target = workspace.join("pre-effect.txt");
    store_manual_direct_create(
        &service,
        &storage,
        &workspace,
        run_id,
        conversation_id,
        assistant_message_id,
        call_id,
        "pre-effect.txt",
        "must not be published\n",
    );
    inject_direct_file_change_outcome_unknown(call_id);

    let (notifications, _receiver) = tokio::sync::mpsc::unbounded_channel();
    let error = service
        .approve_action(run_id, call_id, notifications)
        .unwrap_err();
    assert!(
        error.contains("无法确认文件修改结果"),
        "unexpected error: {error}"
    );
    assert!(!target.exists());
    let storage_id = pending_action_storage_id(run_id, call_id);
    let durable = storage
        .get_pending_agent_action(&storage_id)
        .unwrap()
        .unwrap();
    assert_eq!(durable.status, "executing");
    assert_eq!(durable.target_status, None);
    assert!(storage
        .list_agent_tool_results_for_run(run_id, "apply_patch")
        .unwrap()
        .is_empty());

    drop(service);
    let _restarted = AgentService::try_new_deferred_startup_reconciliation(Arc::clone(&storage))
        .expect("the frozen Base digest must prove that publication never happened");
    assert!(!target.exists());
    let durable = storage
        .get_pending_agent_action(&storage_id)
        .unwrap()
        .unwrap();
    assert_eq!(durable.status, "failed");
    let results = storage
        .list_agent_tool_results_for_run(run_id, "apply_patch")
        .unwrap();
    assert_eq!(results.len(), 1);
    assert!(!results[0].ok);
    assert_eq!(
        results[0].result.as_ref().unwrap()["outcome"],
        "definitely_not_executed"
    );
}

#[test]
fn manual_direct_publication_before_binding_receipt_recovers_without_replay() {
    let fixture = tempdir().unwrap();
    let workspace = fixture.path().join("workspace");
    fs::create_dir(&workspace).unwrap();
    let workspace = fs::canonicalize(workspace).unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let service = AgentService::new(Arc::clone(&storage));
    let run_id = "run-manual-direct-post-effect";
    let conversation_id = "conversation-manual-direct-post-effect";
    let assistant_message_id = "assistant-manual-direct-post-effect";
    let call_id = "call-manual-direct-post-effect";
    let target = workspace.join("post-effect.txt");
    let expected_content = "published once before receipt failure\n";
    store_manual_direct_create(
        &service,
        &storage,
        &workspace,
        run_id,
        conversation_id,
        assistant_message_id,
        call_id,
        "post-effect.txt",
        expected_content,
    );
    inject_direct_file_change_post_commit_binding_failure(call_id);

    let (notifications, _receiver) = tokio::sync::mpsc::unbounded_channel();
    let error = service
        .approve_action(run_id, call_id, notifications)
        .unwrap_err();
    assert!(
        error.contains("无法确认文件修改结果"),
        "unexpected error: {error}"
    );
    assert_eq!(fs::read_to_string(&target).unwrap(), expected_content);
    let published_metadata = fs::metadata(&target).unwrap();
    let storage_id = pending_action_storage_id(run_id, call_id);
    assert!(storage
        .list_agent_tool_results_for_run(run_id, "apply_patch")
        .unwrap()
        .is_empty());

    drop(service);
    let _restarted = AgentService::try_new_deferred_startup_reconciliation(Arc::clone(&storage))
        .expect("the frozen Target digest must adopt the publication without replay");
    assert_eq!(fs::read_to_string(&target).unwrap(), expected_content);
    let recovered_metadata = fs::metadata(&target).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        assert_eq!(published_metadata.dev(), recovered_metadata.dev());
        assert_eq!(published_metadata.ino(), recovered_metadata.ino());
    }
    #[cfg(not(unix))]
    let _ = (published_metadata, recovered_metadata);
    assert_eq!(
        storage
            .get_pending_agent_action(&storage_id)
            .unwrap()
            .unwrap()
            .status,
        "completed"
    );
    let results = storage
        .list_agent_tool_results_for_run(run_id, "apply_patch")
        .unwrap();
    assert_eq!(results.len(), 1);
    assert!(results[0].ok);
    assert_eq!(
        results[0].result.as_ref().unwrap()["status"],
        "already_applied"
    );
    let trace = storage
        .get_conversation_turn_trace(assistant_message_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        trace
            .items
            .iter()
            .filter(|item| matches!(
                item,
                ConversationTurnTraceItem::ToolResult { call_id: durable_call_id, .. }
                    if durable_call_id == call_id
            ))
            .count(),
        1
    );
}

#[tokio::test]
async fn manual_direct_post_receipt_error_adopts_exact_timeline_once() {
    let fixture = tempdir().unwrap();
    let workspace = fixture.path().join("workspace");
    fs::create_dir(&workspace).unwrap();
    let workspace = fs::canonicalize(workspace).unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let service = AgentService::new(Arc::clone(&storage));
    let run_id = "run-manual-direct-post-receipt";
    let conversation_id = "conversation-manual-direct-post-receipt";
    let assistant_message_id = "assistant-manual-direct-post-receipt";
    let call_id = "call-manual-direct-post-receipt";
    let target = workspace.join("post-receipt.txt");
    store_manual_direct_create(
        &service,
        &storage,
        &workspace,
        run_id,
        conversation_id,
        assistant_message_id,
        call_id,
        "post-receipt.txt",
        "receipt and Timeline are one fact\n",
    );
    let storage_id = pending_action_storage_id(run_id, call_id);
    inject_manual_action_audit_post_commit_failure(&storage_id, "completed");

    let (notifications, _receiver) = tokio::sync::mpsc::unbounded_channel();
    let output = service
        .approve_action(run_id, call_id, notifications)
        .expect("the post-commit error must be adopted from the authoritative receipt");
    assert_eq!(output.status, "applied");
    assert_eq!(
        fs::read_to_string(&target).unwrap(),
        "receipt and Timeline are one fact\n"
    );
    let durable = storage
        .get_pending_agent_action(&storage_id)
        .unwrap()
        .unwrap();
    // The exact receipt is authoritative before the asynchronous model continuation advances
    // the Pending lifecycle. An `executing` row with this terminal target is therefore the
    // intended crash boundary, not evidence that the filesystem effect should be replayed.
    assert_eq!(durable.status, "executing");
    assert_eq!(durable.target_status.as_deref(), Some("completed"));
    assert_eq!(
        storage
            .list_agent_tool_results_for_run(run_id, "apply_patch")
            .unwrap()
            .len(),
        1
    );
    let trace = storage
        .get_conversation_turn_trace(assistant_message_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        trace
            .items
            .iter()
            .filter(|item| matches!(
                item,
                ConversationTurnTraceItem::ToolResult { call_id: durable_call_id, .. }
                    if durable_call_id == call_id
            ))
            .count(),
        1
    );
}

#[test]
fn direct_update_revision_conflict_after_approval_wait_has_no_side_effect() {
    let fixture = tempdir().unwrap();
    let workspace = fixture.path().join("workspace");
    fs::create_dir(&workspace).unwrap();
    let workspace = fs::canonicalize(workspace).unwrap();
    let storage = StorageService::open(&fixture.path().join("storage.sqlite")).unwrap();
    let target = workspace.join("report.txt");
    fs::write(&target, "frozen before approval\n").unwrap();
    let canonical_target = target.to_string_lossy().into_owned();
    let run_id = "run-direct-update-conflict";
    let conversation_id = "conversation-direct-update-conflict";
    let (mut action, call) = direct_file_change_fixture(
        run_id,
        conversation_id,
        "call-direct-update-conflict",
        ("report.txt", &canonical_target),
        Some("frozen before approval\n"),
        Some("approved target content\n"),
        AgentApprovalStatus::Required,
    );
    let input = direct_execution_input(
        &workspace,
        run_id,
        conversation_id,
        AgentPermissions {
            write: AgentWritePermission::WorkspaceOnly,
            patch: mycopilot_core::AgentPatchPermission::RequireApproval,
            ..Default::default()
        },
        &call,
    );
    bind_direct_execution_to_input(&mut action, &input);
    authorize_structured_file_write(&input, &action, FileWriteAuthorizationSource::ExplicitUser)
        .unwrap();

    fs::write(&target, "external change while approval was pending\n").unwrap();
    let record = pending_file_change_record(run_id, conversation_id, action, &call, input);
    let restored_call = tool_call_for_pending_record(&record).unwrap();
    assert_eq!(restored_call.args, call.args);
    let decision = action_execution_for_decision(
        &storage,
        &record,
        &restored_call,
        AgentApprovalDecisionStatus::Approved,
        None,
    );
    let result = decision
        .file_change_result
        .as_ref()
        .expect("conflict returns a FileChange result");
    assert_eq!(
        decision.status, "conflict",
        "expected revision conflict: code={:?}, error={:?}",
        result.error_code, result.error
    );
    assert_eq!(decision.final_pending_status, PendingActionStatus::Failed);
    assert_eq!(result.status, AgentFileChangeResultStatus::Conflict);
    assert!(decision.file_change.is_none());
    assert!(!decision.tool_result.ok);
    assert_eq!(
        fs::read_to_string(&target).unwrap(),
        "external change while approval was pending\n"
    );
    let workspace_entries = fs::read_dir(&workspace)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect::<Vec<_>>();
    assert_eq!(workspace_entries, vec![target.file_name().unwrap()]);
}

#[test]
fn direct_update_rejects_an_identical_replacement_after_approval_wait() {
    let fixture = tempdir().unwrap();
    let workspace = fixture.path().join("workspace");
    fs::create_dir(&workspace).unwrap();
    let workspace = fs::canonicalize(workspace).unwrap();
    let storage = StorageService::open(&fixture.path().join("storage.sqlite")).unwrap();
    let target = workspace.join("report.txt");
    fs::write(&target, "same bytes, different file identity\n").unwrap();
    let canonical_target = target.to_string_lossy().into_owned();
    let run_id = "run-direct-update-identity-conflict";
    let conversation_id = "conversation-direct-update-identity-conflict";
    let (mut action, call) = direct_file_change_fixture(
        run_id,
        conversation_id,
        "call-direct-update-identity-conflict",
        ("report.txt", &canonical_target),
        Some("same bytes, different file identity\n"),
        Some("approved target content\n"),
        AgentApprovalStatus::Required,
    );
    let input = direct_execution_input(
        &workspace,
        run_id,
        conversation_id,
        AgentPermissions {
            write: AgentWritePermission::WorkspaceOnly,
            patch: mycopilot_core::AgentPatchPermission::RequireApproval,
            ..Default::default()
        },
        &call,
    );
    bind_direct_execution_to_input(&mut action, &input);
    authorize_structured_file_write(&input, &action, FileWriteAuthorizationSource::ExplicitUser)
        .unwrap();

    let displaced = workspace.join("displaced.txt");
    fs::rename(&target, &displaced).unwrap();
    fs::write(&target, "same bytes, different file identity\n").unwrap();
    fs::remove_file(displaced).unwrap();

    let record = pending_file_change_record(run_id, conversation_id, action, &call, input);
    let restored_call = tool_call_for_pending_record(&record).unwrap();
    let decision = action_execution_for_decision(
        &storage,
        &record,
        &restored_call,
        AgentApprovalDecisionStatus::Approved,
        None,
    );
    let result = decision.file_change_result.as_ref().unwrap();
    assert_eq!(decision.status, "conflict");
    assert_eq!(result.status, AgentFileChangeResultStatus::Conflict);
    assert_eq!(
        result.error_code.as_deref(),
        Some("agent.apply_patch.observation_stale")
    );
    assert_eq!(
        fs::read_to_string(&target).unwrap(),
        "same bytes, different file identity\n"
    );
}
