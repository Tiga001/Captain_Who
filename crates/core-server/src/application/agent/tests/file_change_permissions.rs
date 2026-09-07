use super::*;
use mycopilot_core::{AgentFileChangeOperation, AgentFileChangeResultStatus};
use mycopilot_protocol_rs::AgentApprovalScopeDto;
use std::path::Path;

const DEFAULT_FILE_CHANGE_REJECTION_MESSAGE: &str = "用户拒绝了文件修改。";

pub(super) const FILE_CHANGE_REJECTION_CASES: [(&str, Option<&str>, &str); 6] = [
    ("missing", None, DEFAULT_FILE_CHANGE_REJECTION_MESSAGE),
    ("empty", Some(""), DEFAULT_FILE_CHANGE_REJECTION_MESSAGE),
    ("spaces", Some("   "), DEFAULT_FILE_CHANGE_REJECTION_MESSAGE),
    (
        "ascii_whitespace",
        Some("\t\r\n"),
        DEFAULT_FILE_CHANGE_REJECTION_MESSAGE,
    ),
    (
        "unicode_whitespace",
        Some("\u{00a0}\u{2003}\u{3000}"),
        DEFAULT_FILE_CHANGE_REJECTION_MESSAGE,
    ),
    (
        "feedback_preserved",
        Some(" \t请保留现有文件，先修改方案。\r\n"),
        " \t请保留现有文件，先修改方案。\r\n",
    ),
];

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
        pause_reason: mycopilot_core::AgentRunCheckpointPauseReason::Approval,
        version: AGENT_RUN_CHECKPOINT_SCHEMA_VERSION,
        run_id: run_id.to_string(),
        context_items: vec![mycopilot_core::AgentContextCheckpointItem {
            context_image_refs: Vec::new(),
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
            request_order: None,
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
        conversation_world_state_records: Vec::new(),
        run_world_state: crate::test_run_world_state(),
        pending_action_id: Some(pending_action_storage_id(run_id, &call.id)),
        file_change_run_grant_ref: None,
        pending_file_observation: None,
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
    let mut conversation = storage
        .load_conversation(conversation_id)
        .unwrap()
        .unwrap_or(ChatConversationRecord {
            id: conversation_id.to_string(),
            project_id: None,
            model_id: Some("test-model".to_string()),
            title: "Direct FileChange crash boundary".to_string(),
            messages: Vec::new(),
            created_at,
            updated_at: created_at,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        });
    if !conversation
        .messages
        .iter()
        .any(|message| message.id == assistant_message_id)
    {
        conversation.messages.push(ChatMessageRecord {
            human_interaction_response: None,
            id: assistant_message_id.to_string(),
            role: "assistant".to_string(),
            content: String::new(),
            created_at,
            status: Some("pending".to_string()),
            attachments: Vec::new(),
            agent_run_json: None,
            ui_state_json: None,
        });
    }
    storage.save_conversation(conversation).unwrap();

    let provider_identity = AgentProviderToolCallIdentity {
        provider_tool_index: 0,
        provider_call_id: call.id.clone(),
        runtime_call_id: call.id.clone(),
    };
    let mut trace = storage
        .get_conversation_turn_trace(assistant_message_id)
        .unwrap()
        .unwrap_or(mycopilot_core::ConversationTurnTrace {
            schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
            run_id: run_id.to_string(),
            conversation_id: conversation_id.to_string(),
            assistant_message_id: assistant_message_id.to_string(),
            terminal_status: ConversationTurnTraceTerminalStatus::InProgress,
            terminal_error: None,
            truncated: false,
            items: Vec::new(),
        });
    let trace_sequence = trace.items.last().map_or(0, |item| item.sequence() + 1);
    let trace_operation =
        mycopilot_core::file_change_support::apply_patch_trace_operation(&call.args)
            .expect("project fixture ToolCall through the production durable Trace boundary");
    let trace_redacted = trace_operation != call.args;
    trace.items.push(ConversationTurnTraceItem::ToolCall {
        sequence: trace_sequence,
        call_id: call.id.clone(),
        tool: call.tool.clone(),
        provenance: AgentToolIdentity::Builtin {
            tool_name: "apply_patch".to_string(),
        },
        operation: trace_operation,
        approval_status: call.approval_status,
        truncated: trace_redacted,
    });
    trace.truncated |= trace_redacted;
    let mut model_context = storage
        .get_conversation_model_context_log(assistant_message_id)
        .unwrap()
        .map(|log| log.items)
        .unwrap_or_default();
    let context_sequence = model_context.last().map_or(0, |item| item.sequence + 1);
    model_context.push(ConversationModelContextItem {
        images: Vec::new(),
        sequence: context_sequence,
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
    });
    let checkpoint = input
        .resume_checkpoint
        .as_mut()
        .expect("Direct crash fixture has a frozen checkpoint");
    checkpoint.conversation_trace_items = trace.items.clone();
    checkpoint.conversation_model_context_items = model_context.clone();
    checkpoint.next_conversation_trace_sequence = trace_sequence + 1;
    checkpoint.conversation_trace_truncated = trace.truncated;
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

fn assert_exact_json_object_keys(value: &serde_json::Value, expected: &[&str]) {
    let mut actual = value
        .as_object()
        .expect("expected a JSON object")
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    actual.sort();
    let mut expected = expected
        .iter()
        .map(|key| (*key).to_string())
        .collect::<Vec<_>>();
    expected.sort();
    assert_eq!(actual, expected);
}

fn assert_strict_file_change_execution_json(
    output: &AgentActionExecutionOutput,
    expected_status: &str,
) -> serde_json::Value {
    let value = serde_json::to_value(output).unwrap();
    assert_exact_json_object_keys(
        &value,
        &[
            "actionId",
            "actionType",
            "toolName",
            "status",
            "fileChangeResult",
            "agentOutput",
        ],
    );
    assert_eq!(value["status"], expected_status);
    assert!(value.get("toolResult").is_none());
    assert!(value.get("commandResult").is_none());
    assert_exact_json_object_keys(
        &value["fileChangeResult"],
        &[
            "schemaVersion",
            "status",
            "outcome",
            "transactionId",
            "operation",
            "updateStrategy",
            "filePath",
            "additions",
            "deletions",
            "lineCount",
            "byteCount",
            "revision",
            "errorCode",
            "error",
            "message",
        ],
    );
    assert_exact_json_object_keys(
        &value["agentOutput"],
        &[
            "content",
            "status",
            "runId",
            "events",
            "toolDefinitions",
            "proposedActions",
        ],
    );
    value["fileChangeResult"].clone()
}

fn assert_file_change_receipt_surfaces_match(
    storage: &StorageService,
    storage_id: &str,
    run_id: &str,
    notifications: &mut tokio::sync::mpsc::UnboundedReceiver<serde_json::Value>,
    receipt: &serde_json::Value,
) {
    let audit = storage
        .get_agent_action_audit(storage_id)
        .unwrap()
        .expect("FileChange has a durable terminal audit");
    let audit_receipt: serde_json::Value = serde_json::from_str(
        audit
            .file_change_result_json
            .as_deref()
            .expect("FileChange audit has a typed result"),
    )
    .unwrap();
    assert_eq!(&audit_receipt, receipt);
    let audit_tool_result: AgentToolResult = serde_json::from_str(
        audit
            .tool_result_json
            .as_deref()
            .expect("FileChange audit has its paired ToolResult"),
    )
    .unwrap();
    assert_eq!(audit_tool_result.result.as_ref(), Some(receipt));

    let trace_results = storage
        .list_agent_tool_results_for_run(run_id, "apply_patch")
        .unwrap();
    assert_eq!(trace_results.len(), 1);
    assert_eq!(trace_results[0].result.as_ref(), Some(receipt));

    let events = std::iter::from_fn(|| notifications.try_recv().ok()).collect::<Vec<_>>();
    let published = events
        .iter()
        .filter(|event| event["params"]["type"] == "tool_result")
        .collect::<Vec<_>>();
    assert_eq!(published.len(), 1);
    assert_eq!(published[0]["params"]["result"]["result"], *receipt);
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

fn structured_file_change_actions(
    approval_status: AgentApprovalStatus,
) -> [AgentProposedAction; 2] {
    [
        apply_patch_action(approval_status),
        staged_file_change_action(approval_status),
    ]
}

#[test]
fn direct_and_staged_body_canary_stays_inside_private_file_change_authority() {
    const BODY_CANARY: &str = "FILE_CHANGE_BODY_PRIVACY_CANARY_7f3e2a91";

    let fixture = tempdir().unwrap();
    let direct_path = fixture.path().join("direct.txt");
    let filler = (1..=16)
        .map(|index| format!("unchanged line {index}"))
        .collect::<Vec<_>>()
        .join("\n");
    let base = format!("{BODY_CANARY}\n{filler}\nstatus: before\n");
    let target = format!("{BODY_CANARY}\n{filler}\nstatus: after\n");
    std::fs::write(&direct_path, &base).unwrap();
    let (direct_action, direct_call) = direct_file_change_fixture(
        "run-private-direct",
        "conversation-private",
        "call-private-direct",
        ("direct.txt", direct_path.to_str().unwrap()),
        Some(&base),
        Some(&target),
        AgentApprovalStatus::Required,
    );
    let AgentProposedAction::FileChange {
        file_change: direct_proposal,
    } = &direct_action
    else {
        unreachable!("Direct fixture is a FileChange")
    };
    assert!(!direct_proposal
        .inline_diff
        .as_ref()
        .unwrap()
        .patch
        .contains(BODY_CANARY));

    let staged_path = fixture.path().join("staged.txt");
    let (staged_proposal, staged_call) = staged_file_change_fixture(
        StagedFileChangeFixtureIdentity {
            run_id: "run-private-staged",
            conversation_id: "conversation-private",
            call_id: "call-private-staged",
            transaction_id: "file-change-private-staged",
        },
        ("staged.txt", staged_path.to_str().unwrap()),
        BODY_CANARY,
        AgentApprovalStatus::Required,
    );
    let staged_action = AgentProposedAction::FileChange {
        file_change: staged_proposal.clone(),
    };

    for (boundary, event) in [
        (
            "direct renderer event",
            agent_event_notification(AgentEvent::FileChangeProposed {
                run_id: "run-private-direct".to_string(),
                file_change: direct_proposal.clone(),
            }),
        ),
        (
            "staged renderer event",
            agent_event_notification(AgentEvent::FileChangeProposed {
                run_id: "run-private-staged".to_string(),
                file_change: staged_proposal.clone(),
            }),
        ),
    ] {
        let encoded = serde_json::to_string(&event).unwrap();
        assert!(!encoded.contains(BODY_CANARY), "{boundary} leaked body");
        assert!(!encoded.contains("\"execution\""));
    }

    // The body is retained only by authority-bearing Host state: Direct exact replay and its
    // frozen proposal, plus the canonical Staged transaction and frozen commit proposal.
    let direct_input = direct_execution_input(
        fixture.path(),
        "run-private-direct",
        "conversation-private",
        AgentPermissions::default(),
        &direct_call,
    );
    assert!(serde_json::to_string(&direct_input)
        .unwrap()
        .contains(BODY_CANARY));
    assert!(serde_json::to_string(&direct_action)
        .unwrap()
        .contains(BODY_CANARY));
    assert!(serde_json::to_string(&staged_action)
        .unwrap()
        .contains(BODY_CANARY));
    let canonical_staged = staged_file_change_record(&staged_proposal, "waiting_approval");
    assert!(canonical_staged.content.contains(BODY_CANARY));
    assert_eq!(staged_call.tool, "apply_patch");
}

fn assert_structured_file_change_authorization(
    input: &AgentChatInput,
    approval_status: AgentApprovalStatus,
    source: FileChangeAuthorizationSource,
    allowed: bool,
) {
    for action in structured_file_change_actions(approval_status) {
        assert_eq!(
            authorize_file_change_action(input, &action, source).is_ok(),
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
    assert_structured_file_change_authorization(
        &manual,
        AgentApprovalStatus::Approved,
        FileChangeAuthorizationSource::Automatic,
        false,
    );

    let automatic = test_input(AgentPermissions {
        write: AgentWritePermission::WorkspaceOnly,
        patch: mycopilot_core::AgentPatchPermission::AutoApprove,
        ..Default::default()
    });
    assert_structured_file_change_authorization(
        &automatic,
        AgentApprovalStatus::Approved,
        FileChangeAuthorizationSource::Automatic,
        true,
    );
    assert_structured_file_change_authorization(
        &automatic,
        AgentApprovalStatus::Required,
        FileChangeAuthorizationSource::Automatic,
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
    assert_structured_file_change_authorization(
        &manual,
        AgentApprovalStatus::Required,
        FileChangeAuthorizationSource::ExplicitUser,
        true,
    );

    let denied = test_input(AgentPermissions {
        write: AgentWritePermission::Denied,
        patch: mycopilot_core::AgentPatchPermission::AutoApprove,
        ..Default::default()
    });
    assert_structured_file_change_authorization(
        &denied,
        AgentApprovalStatus::Approved,
        FileChangeAuthorizationSource::Automatic,
        false,
    );
    assert_structured_file_change_authorization(
        &denied,
        AgentApprovalStatus::Required,
        FileChangeAuthorizationSource::ExplicitUser,
        false,
    );
}

#[test]
fn process_actions_remain_in_their_separate_command_policy_domain() {
    let denied = test_input(AgentPermissions::default());
    let command = AgentProposedAction::Command {
        command: command_request("command-1", "pwd"),
    };

    assert!(authorize_file_change_action(
        &denied,
        &command,
        FileChangeAuthorizationSource::Automatic,
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
    authorize_file_change_action(
        &create_input,
        &create_action,
        FileChangeAuthorizationSource::Automatic,
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
    authorize_file_change_action(
        &update_input,
        &update_action,
        FileChangeAuthorizationSource::ExplicitUser,
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
    authorize_file_change_action(
        &delete_input,
        &delete_action,
        FileChangeAuthorizationSource::Automatic,
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

#[tokio::test]
async fn remaining_run_approval_survives_redacted_content_trace_and_drives_the_next_update() {
    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("storage.sqlite");
    let workspace = fixture.path().join("workspace");
    fs::create_dir(&workspace).unwrap();
    let workspace = fs::canonicalize(workspace).unwrap();
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    let service = AgentService::new(Arc::clone(&storage));
    let run_id = "run-remember-file-change";
    let conversation_id = "conversation-remember-file-change";
    let first_call_id = "call-remember-first";

    store_manual_direct_create(
        &service,
        &storage,
        &workspace,
        run_id,
        conversation_id,
        "assistant-remember-first",
        first_call_id,
        "first.txt",
        "first\n",
    );
    let (notifications, _receiver) = tokio::sync::mpsc::unbounded_channel();
    let first = service
        .approve_action_with_scope(
            run_id,
            first_call_id,
            AgentApprovalScopeDto::RemainingApplyPatchInRun,
            notifications,
        )
        .expect("the explicitly approved create must settle");
    assert_eq!(first.status, "applied");
    let active = storage
        .get_active_file_change_run_grant(run_id)
        .unwrap()
        .expect("the applied granting receipt immediately authorizes the same live Run");
    assert_eq!(
        active.granting_pending_action_id,
        pending_action_storage_id(run_id, first_call_id)
    );

    let second_call_id = "call-remember-second";
    let second_target = workspace.join("first.txt");
    let (mut second_action, second_call) = direct_file_change_fixture(
        run_id,
        conversation_id,
        second_call_id,
        ("first.txt", second_target.to_str().unwrap()),
        Some("first\n"),
        Some("updated\n"),
        AgentApprovalStatus::Approved,
    );
    let mut second_input = direct_execution_input(
        &workspace,
        run_id,
        conversation_id,
        AgentPermissions {
            write: AgentWritePermission::WorkspaceOnly,
            patch: mycopilot_core::AgentPatchPermission::RequireApproval,
            ..Default::default()
        },
        &second_call,
    );
    save_test_pending_provider_for_input(&storage, &mut second_input);
    bind_direct_execution_to_input(&mut second_action, &second_input);
    let AgentProposedAction::FileChange {
        file_change: second_file_change,
    } = &second_action
    else {
        unreachable!()
    };
    let grant_ref = storage
        .resolve_active_file_change_run_grant(
            second_file_change,
            second_input.context.as_ref().unwrap(),
        )
        .unwrap()
        .expect("same-Run update inherits the exact active grant");
    second_input
        .resume_checkpoint
        .as_mut()
        .unwrap()
        .file_change_run_grant_ref = Some(grant_ref);
    seed_durable_direct_file_change_owner(
        &storage,
        &mut second_input,
        conversation_id,
        "assistant-remember-first",
        run_id,
        &second_call,
    );
    let second_result = service
        .execute_auto_approved_action(
            AutoApprovedActionContext::new(
                second_input,
                run_id.to_string(),
                Some(conversation_id.to_string()),
                Some("assistant-remember-first".to_string()),
                None,
            ),
            second_action,
            AgentCancellationToken::new(),
        )
        .expect("the RunGrant route executes through the managed auto journal");
    assert!(second_result.ok);
    assert_eq!(fs::read_to_string(&second_target).unwrap(), "updated\n");
    let second_audit = storage
        .get_agent_action_audit(&pending_action_storage_id(run_id, second_call_id))
        .unwrap()
        .unwrap();
    assert_eq!(second_audit.decision_source.as_deref(), Some("run_grant"));

    let (mut delete_action, delete_call) = direct_file_change_fixture(
        run_id,
        conversation_id,
        "call-remember-delete",
        ("first.txt", second_target.to_str().unwrap()),
        Some("updated\n"),
        None,
        AgentApprovalStatus::Required,
    );
    let delete_input = direct_execution_input(
        &workspace,
        run_id,
        conversation_id,
        AgentPermissions {
            write: AgentWritePermission::WorkspaceOnly,
            patch: mycopilot_core::AgentPatchPermission::RequireApproval,
            ..Default::default()
        },
        &delete_call,
    );
    bind_direct_execution_to_input(&mut delete_action, &delete_input);
    let AgentProposedAction::FileChange {
        file_change: delete,
    } = &delete_action
    else {
        unreachable!()
    };
    assert!(storage
        .resolve_active_file_change_run_grant(delete, delete_input.context.as_ref().unwrap())
        .unwrap()
        .is_none());

    let (mut new_run_action, new_run_call) = direct_file_change_fixture(
        "run-remember-new",
        conversation_id,
        "call-remember-new-run",
        (
            "new-run.txt",
            workspace.join("new-run.txt").to_str().unwrap(),
        ),
        None,
        Some("new run\n"),
        AgentApprovalStatus::Required,
    );
    let new_run_input = direct_execution_input(
        &workspace,
        "run-remember-new",
        conversation_id,
        AgentPermissions {
            write: AgentWritePermission::WorkspaceOnly,
            patch: mycopilot_core::AgentPatchPermission::RequireApproval,
            ..Default::default()
        },
        &new_run_call,
    );
    bind_direct_execution_to_input(&mut new_run_action, &new_run_input);
    let AgentProposedAction::FileChange {
        file_change: new_run_file_change,
    } = &new_run_action
    else {
        unreachable!()
    };
    assert!(storage
        .resolve_active_file_change_run_grant(
            new_run_file_change,
            new_run_input.context.as_ref().unwrap(),
        )
        .unwrap()
        .is_none());

    let granting_action_id = pending_action_storage_id(run_id, first_call_id);
    let connection = rusqlite::Connection::open(&database_path).unwrap();
    let (original_result_json, original_tool_result_json): (String, String) = connection
        .query_row(
            "SELECT file_change_result_json, tool_result_json
             FROM agent_action_audit WHERE action_id = ?1",
            [&granting_action_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    let original_trace_result: String = connection
        .query_row(
            "SELECT item_json FROM conversation_turn_trace_items
             WHERE assistant_message_id = 'assistant-remember-first'
               AND item_kind = 'tool_result'
               AND json_extract(item_json, '$.callId') = ?1",
            [first_call_id],
            |row| row.get(0),
        )
        .unwrap();
    let original_trace_call: String = connection
        .query_row(
            "SELECT item_json FROM conversation_turn_trace_items
             WHERE assistant_message_id = 'assistant-remember-first'
               AND item_kind = 'tool_call'
               AND json_extract(item_json, '$.callId') = ?1",
            [first_call_id],
            |row| row.get(0),
        )
        .unwrap();

    connection
        .execute(
            "UPDATE agent_action_audit
             SET file_change_result_json = json_set(file_change_result_json, '$.filePath', 'tampered.txt')
             WHERE action_id = ?1",
            [&granting_action_id],
        )
        .unwrap();
    assert!(storage
        .get_active_file_change_run_grant(run_id)
        .unwrap()
        .is_none());
    connection
        .execute(
            "UPDATE agent_action_audit SET file_change_result_json = ?2 WHERE action_id = ?1",
            rusqlite::params![&granting_action_id, &original_result_json],
        )
        .unwrap();

    connection
        .execute(
            "UPDATE agent_action_audit
             SET tool_result_json = json_set(tool_result_json, '$.result.filePath', 'tampered.txt')
             WHERE action_id = ?1",
            [&granting_action_id],
        )
        .unwrap();
    assert!(storage
        .get_active_file_change_run_grant(run_id)
        .unwrap()
        .is_none());
    connection
        .execute(
            "UPDATE agent_action_audit SET tool_result_json = ?2 WHERE action_id = ?1",
            rusqlite::params![&granting_action_id, &original_tool_result_json],
        )
        .unwrap();

    connection
        .execute(
            "UPDATE conversation_turn_trace_items
             SET item_json = json_set(item_json, '$.observation.filePath', 'tampered.txt')
             WHERE assistant_message_id = 'assistant-remember-first'
               AND item_kind = 'tool_result'
               AND json_extract(item_json, '$.callId') = ?1",
            [first_call_id],
        )
        .unwrap();
    assert!(storage
        .get_active_file_change_run_grant(run_id)
        .unwrap()
        .is_none());
    connection
        .execute(
            "UPDATE conversation_turn_trace_items SET item_json = ?2
             WHERE assistant_message_id = 'assistant-remember-first'
               AND item_kind = 'tool_result'
               AND json_extract(item_json, '$.callId') = ?1",
            rusqlite::params![first_call_id, original_trace_result],
        )
        .unwrap();
    assert!(storage
        .get_active_file_change_run_grant(run_id)
        .unwrap()
        .is_some());

    connection
        .execute(
            "UPDATE conversation_turn_trace_items
             SET item_json = json_set(
                 item_json,
                 '$.operation.request.contentDigest',
                 'ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff'
             )
             WHERE assistant_message_id = 'assistant-remember-first'
               AND item_kind = 'tool_call'
               AND json_extract(item_json, '$.callId') = ?1",
            [first_call_id],
        )
        .unwrap();
    assert!(
        storage
            .get_active_file_change_run_grant(run_id)
            .unwrap()
            .is_none(),
        "tampering the body-free Trace operation must revoke remembered authority"
    );
    connection
        .execute(
            "UPDATE conversation_turn_trace_items SET item_json = ?2
             WHERE assistant_message_id = 'assistant-remember-first'
               AND item_kind = 'tool_call'
               AND json_extract(item_json, '$.callId') = ?1",
            rusqlite::params![first_call_id, original_trace_call],
        )
        .unwrap();
    assert!(storage
        .get_active_file_change_run_grant(run_id)
        .unwrap()
        .is_some());

    let original_agent_input_json: String = connection
        .query_row(
            "SELECT agent_input_json FROM agent_pending_actions WHERE action_id = ?1",
            [&granting_action_id],
            |row| row.get(0),
        )
        .unwrap();
    let mut tampered_checkpoint_refs =
        serde_json::from_str::<serde_json::Value>(&original_agent_input_json).unwrap();
    tampered_checkpoint_refs["resumeCheckpoint"]["pendingActionId"] =
        json!(pending_action_storage_id(run_id, "forged-granting-call"));
    connection
        .execute(
            "UPDATE agent_pending_actions SET agent_input_json = ?2 WHERE action_id = ?1",
            rusqlite::params![&granting_action_id, tampered_checkpoint_refs.to_string()],
        )
        .unwrap();
    assert!(
        storage
            .get_active_file_change_run_grant(run_id)
            .unwrap()
            .is_none(),
        "the granting checkpoint must reference the exact canonical Pending Action"
    );
    tampered_checkpoint_refs["resumeCheckpoint"]["pendingActionId"] = json!(&granting_action_id);
    tampered_checkpoint_refs["resumeCheckpoint"]["fileChangeRunGrantRef"] =
        serde_json::to_value(active.reference().unwrap()).unwrap();
    connection
        .execute(
            "UPDATE agent_pending_actions SET agent_input_json = ?2 WHERE action_id = ?1",
            rusqlite::params![&granting_action_id, tampered_checkpoint_refs.to_string()],
        )
        .unwrap();
    assert!(
        storage
            .get_active_file_change_run_grant(run_id)
            .unwrap()
            .is_none(),
        "the granting checkpoint cannot bootstrap itself from remembered authority"
    );
    connection
        .execute(
            "UPDATE agent_pending_actions SET agent_input_json = ?2 WHERE action_id = ?1",
            rusqlite::params![&granting_action_id, &original_agent_input_json],
        )
        .unwrap();

    let forged_workspace = fixture.path().join("forged-workspace");
    fs::create_dir(&forged_workspace).unwrap();
    let forged_workspace = fs::canonicalize(forged_workspace).unwrap();
    let mut tampered_agent_input =
        serde_json::from_str::<serde_json::Value>(&original_agent_input_json).unwrap();
    tampered_agent_input["context"]["workspace"]["rootPath"] =
        json!(forged_workspace.to_string_lossy());
    tampered_agent_input["resumeCheckpoint"]["runContext"]["workspace"]["rootPath"] =
        json!(forged_workspace.to_string_lossy());
    connection
        .execute(
            "UPDATE agent_pending_actions SET agent_input_json = ?2 WHERE action_id = ?1",
            rusqlite::params![&granting_action_id, tampered_agent_input.to_string()],
        )
        .unwrap();
    assert!(
        storage
            .get_active_file_change_run_grant(run_id)
            .unwrap()
            .is_none(),
        "the active grant must be re-derived from the persisted granting Run context"
    );
    connection
        .execute(
            "UPDATE agent_pending_actions SET agent_input_json = ?2 WHERE action_id = ?1",
            rusqlite::params![&granting_action_id, &original_agent_input_json],
        )
        .unwrap();

    let forged_scope_identity =
        mycopilot_core::file_change::FileChangeDirectoryIdentity::read(&forged_workspace).unwrap();
    let forged_scope_identity_json = serde_json::to_string(&forged_scope_identity).unwrap();
    let forged_workspace_identity =
        mycopilot_core::file_change::file_change_workspace_identity(None, None, &forged_workspace)
            .unwrap();
    connection
        .execute(
            "UPDATE agent_file_change_run_grants
             SET workspace_identity = ?2,
                 canonical_scope_path = ?3,
                 scope_directory_identity_json = ?4,
                 base_write_permission = 'all'
             WHERE grant_id = ?1",
            rusqlite::params![
                &active.grant_id,
                forged_workspace_identity,
                forged_workspace.to_string_lossy(),
                forged_scope_identity_json,
            ],
        )
        .unwrap();
    assert!(
        storage
            .get_active_file_change_run_grant(run_id)
            .unwrap()
            .is_none(),
        "a self-consistent but forged scope row must not survive granting-action re-derivation"
    );
    connection
        .execute(
            "UPDATE agent_file_change_run_grants
             SET workspace_identity = ?2,
                 canonical_scope_path = ?3,
                 scope_directory_identity_json = ?4,
                 base_write_permission = 'workspace_only'
             WHERE grant_id = ?1",
            rusqlite::params![
                &active.grant_id,
                active.workspace_identity.as_deref(),
                &active.canonical_scope_path,
                serde_json::to_string(&active.scope_directory_identity).unwrap(),
            ],
        )
        .unwrap();
    assert!(storage
        .get_active_file_change_run_grant(run_id)
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn single_action_response_lost_retry_replays_the_exact_receipt_without_a_second_effect() {
    let fixture = tempdir().unwrap();
    let workspace = fixture.path().join("workspace");
    fs::create_dir(&workspace).unwrap();
    let workspace = fs::canonicalize(workspace).unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let service = AgentService::new(Arc::clone(&storage));
    let run_id = "run-single-action-response-lost";
    let conversation_id = "conversation-single-action-response-lost";
    let action_id = "call-single-action-response-lost";
    let target = workspace.join("single.txt");
    store_manual_direct_create(
        &service,
        &storage,
        &workspace,
        run_id,
        conversation_id,
        "assistant-single-action-response-lost",
        action_id,
        "single.txt",
        "written exactly once\n",
    );
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();

    let first = service
        .approve_action_with_scope(
            run_id,
            action_id,
            AgentApprovalScopeDto::SingleAction,
            notifications.clone(),
        )
        .unwrap();
    let first_receipt = assert_strict_file_change_execution_json(&first, "applied");
    let metadata_after_first = fs::metadata(&target).unwrap();
    let retry = service
        .approve_action_with_scope(
            run_id,
            action_id,
            AgentApprovalScopeDto::SingleAction,
            notifications,
        )
        .expect("a response-lost retry must replay the durable terminal receipt");
    let retry_receipt = assert_strict_file_change_execution_json(&retry, "applied");

    assert_eq!(retry_receipt, first_receipt);
    assert_eq!(
        fs::read_to_string(&target).unwrap(),
        "written exactly once\n"
    );
    let metadata_after_retry = fs::metadata(&target).unwrap();
    assert_eq!(metadata_after_retry.len(), metadata_after_first.len());
    assert_eq!(
        metadata_after_retry.modified().unwrap(),
        metadata_after_first.modified().unwrap()
    );
    assert!(storage
        .get_file_change_run_grant_for_pending_action(&pending_action_storage_id(run_id, action_id))
        .unwrap()
        .is_none());
    assert!(service
        .approve_action_with_scope(
            run_id,
            action_id,
            AgentApprovalScopeDto::RemainingApplyPatchInRun,
            tokio::sync::mpsc::unbounded_channel().0,
        )
        .unwrap_err()
        .to_string()
        .contains("approval scope"));
    assert!(service
        .approve_action_with_scope(
            "run-single-action-response-lost-forged",
            action_id,
            AgentApprovalScopeDto::SingleAction,
            tokio::sync::mpsc::unbounded_channel().0,
        )
        .is_err());
    assert!(service
        .approve_action_with_scope(
            run_id,
            "call-single-action-response-lost-forged",
            AgentApprovalScopeDto::SingleAction,
            tokio::sync::mpsc::unbounded_channel().0,
        )
        .is_err());
    assert_eq!(
        fs::read_to_string(&target).unwrap(),
        "written exactly once\n"
    );
    let tool_results = std::iter::from_fn(|| receiver.try_recv().ok())
        .filter(|event| event["params"]["type"] == "tool_result")
        .count();
    assert_eq!(
        tool_results, 1,
        "the replay must not publish a second ToolResult"
    );
}

#[tokio::test]
async fn run_grant_storage_failure_returns_only_typed_safe_approval_rpc_data() {
    let fixture = tempdir().unwrap();
    let workspace = fixture.path().join("workspace");
    fs::create_dir(&workspace).unwrap();
    let workspace = fs::canonicalize(workspace).unwrap();
    let database_path = fixture.path().join("storage.sqlite");
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    let service = AgentService::new(Arc::clone(&storage));
    let run_id = "run-grant-storage-safe-rpc";
    let conversation_id = "conversation-grant-storage-safe-rpc";
    let action_id = "call-grant-storage-safe-rpc";
    let target = workspace.join("never-created.txt");
    store_manual_direct_create(
        &service,
        &storage,
        &workspace,
        run_id,
        conversation_id,
        "assistant-grant-storage-safe-rpc",
        action_id,
        "never-created.txt",
        "must remain unexecuted\n",
    );
    rusqlite::Connection::open(&database_path)
        .unwrap()
        .execute_batch("DROP TABLE agent_file_change_run_grants")
        .unwrap();

    let error = service
        .approve_action_with_scope(
            run_id,
            action_id,
            AgentApprovalScopeDto::RemainingApplyPatchInRun,
            tokio::sync::mpsc::unbounded_channel().0,
        )
        .unwrap_err();

    assert_eq!(
        error.message(),
        "File approval memory is temporarily unavailable; this file change was not executed."
    );
    let data = error.data().expect("typed approval recovery data");
    assert_eq!(data["type"], "fileChangeDecision");
    assert_eq!(data["code"], "runGrantStorageUnavailable");
    assert_eq!(data["outcome"], "definitelyNotExecuted");
    assert_eq!(data["recovery"], "retryApproval");
    assert!(!target.exists());
    let public = format!("{}\n{}", error.message(), data);
    for forbidden in [
        "SQLite",
        "sqlite",
        "no such table",
        "agent_file_change_run_grants",
        "rusqlite",
        "Database(",
        "stack",
    ] {
        assert!(
            !public.contains(forbidden),
            "leaked `{forbidden}`: {public}"
        );
    }
}

#[tokio::test]
async fn remaining_scope_response_lost_retry_preserves_the_exact_grant_and_receipt() {
    let fixture = tempdir().unwrap();
    let workspace = fixture.path().join("workspace");
    fs::create_dir(&workspace).unwrap();
    let workspace = fs::canonicalize(workspace).unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let service = AgentService::new(Arc::clone(&storage));
    let run_id = "run-remaining-response-lost";
    let conversation_id = "conversation-remaining-response-lost";
    let action_id = "call-remaining-response-lost";
    let target = workspace.join("remaining.txt");
    store_manual_direct_create(
        &service,
        &storage,
        &workspace,
        run_id,
        conversation_id,
        "assistant-remaining-response-lost",
        action_id,
        "remaining.txt",
        "remembered exactly once\n",
    );
    let (notifications, _receiver) = tokio::sync::mpsc::unbounded_channel();

    let first = service
        .approve_action_with_scope(
            run_id,
            action_id,
            AgentApprovalScopeDto::RemainingApplyPatchInRun,
            notifications.clone(),
        )
        .unwrap();
    let grant_after_first = storage
        .get_file_change_run_grant_for_pending_action(&pending_action_storage_id(run_id, action_id))
        .unwrap()
        .expect("the remembered approval has one durable grant");
    let retry = service
        .approve_action_with_scope(
            run_id,
            action_id,
            AgentApprovalScopeDto::RemainingApplyPatchInRun,
            notifications,
        )
        .expect("the same remembered scope must be idempotent");
    let grant_after_retry = storage
        .get_file_change_run_grant_for_pending_action(&pending_action_storage_id(run_id, action_id))
        .unwrap()
        .expect("retry must retain the original grant");

    assert_eq!(
        serde_json::to_value(&retry.file_change_result).unwrap(),
        serde_json::to_value(&first.file_change_result).unwrap()
    );
    assert_eq!(grant_after_retry.grant_id, grant_after_first.grant_id);
    assert_eq!(grant_after_retry.revision, grant_after_first.revision);
    assert_eq!(
        fs::read_to_string(&target).unwrap(),
        "remembered exactly once\n"
    );
    assert!(service
        .approve_action_with_scope(
            run_id,
            action_id,
            AgentApprovalScopeDto::SingleAction,
            tokio::sync::mpsc::unbounded_channel().0,
        )
        .unwrap_err()
        .to_string()
        .contains("approval scope"));
    assert_eq!(
        storage
            .get_file_change_run_grant_for_pending_action(&pending_action_storage_id(
                run_id, action_id,
            ))
            .unwrap()
            .unwrap()
            .grant_id,
        grant_after_first.grant_id
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_identical_remaining_approvals_execute_once_and_replay_once() {
    let fixture = tempdir().unwrap();
    let workspace = fixture.path().join("workspace");
    fs::create_dir(&workspace).unwrap();
    let workspace = fs::canonicalize(workspace).unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let service = AgentService::new(Arc::clone(&storage));
    let run_id = "run-concurrent-remaining-retry";
    let conversation_id = "conversation-concurrent-remaining-retry";
    let action_id = "call-concurrent-remaining-retry";
    let target = workspace.join("concurrent.txt");
    store_manual_direct_create(
        &service,
        &storage,
        &workspace,
        run_id,
        conversation_id,
        "assistant-concurrent-remaining-retry",
        action_id,
        "concurrent.txt",
        "one effect\n",
    );
    let entered = Arc::new(std::sync::Barrier::new(2));
    let release = Arc::new(std::sync::Barrier::new(2));
    crate::application::agent::approval::install_approval_decision_barrier_hook(
        action_id,
        AgentApprovalDecisionStatus::Approved,
        {
            let entered = Arc::clone(&entered);
            let release = Arc::clone(&release);
            Arc::new(move || {
                entered.wait();
                release.wait();
            })
        },
    );
    let first = {
        let service = service.clone();
        tokio::spawn(async move {
            service.approve_action_with_scope(
                run_id,
                action_id,
                AgentApprovalScopeDto::RemainingApplyPatchInRun,
                tokio::sync::mpsc::unbounded_channel().0,
            )
        })
    };
    entered.wait();
    let second = {
        let service = service.clone();
        tokio::spawn(async move {
            service.approve_action_with_scope(
                run_id,
                action_id,
                AgentApprovalScopeDto::RemainingApplyPatchInRun,
                tokio::sync::mpsc::unbounded_channel().0,
            )
        })
    };
    release.wait();
    let first = first.await.unwrap().unwrap();
    let second = second.await.unwrap().unwrap();

    assert_eq!(first.status, "applied");
    assert_eq!(second.status, "applied");
    assert_eq!(
        serde_json::to_value(&first.file_change_result).unwrap(),
        serde_json::to_value(&second.file_change_result).unwrap()
    );
    assert_eq!(fs::read_to_string(&target).unwrap(), "one effect\n");
    assert_eq!(
        storage
            .list_agent_tool_results_for_run(run_id, "apply_patch")
            .unwrap()
            .len(),
        1
    );
    let grant = storage
        .get_file_change_run_grant_for_pending_action(&pending_action_storage_id(run_id, action_id))
        .unwrap()
        .unwrap();
    assert_eq!(grant.status, FileChangeRunGrantStatus::Active);
}

#[tokio::test]
async fn restart_retry_of_a_durable_in_flight_approval_is_outcome_unknown_and_never_reexecutes() {
    let fixture = tempdir().unwrap();
    let workspace = fixture.path().join("workspace");
    fs::create_dir(&workspace).unwrap();
    let workspace = fs::canonicalize(workspace).unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let service = AgentService::new(Arc::clone(&storage));
    let run_id = "run-in-flight-approval-retry";
    let conversation_id = "conversation-in-flight-approval-retry";
    let action_id = "call-in-flight-approval-retry";
    let target = workspace.join("in-flight.txt");
    store_manual_direct_create(
        &service,
        &storage,
        &workspace,
        run_id,
        conversation_id,
        "assistant-in-flight-approval-retry",
        action_id,
        "in-flight.txt",
        "must not be replayed\n",
    );
    let pending = service
        .pending_actions
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(&pending_action_storage_id(run_id, action_id))
        .unwrap()
        .clone();
    service
        .transition_pending_status(&pending, PendingActionStatus::Executing)
        .unwrap();

    let output = service
        .approve_action_with_scope(
            run_id,
            action_id,
            AgentApprovalScopeDto::SingleAction,
            tokio::sync::mpsc::unbounded_channel().0,
        )
        .expect("an exact in-flight retry returns a typed uncertainty receipt");
    let receipt = assert_strict_file_change_execution_json(&output, "outcome_unknown");
    assert_eq!(receipt["status"], "outcome_unknown");
    assert_eq!(receipt["outcome"], "outcome_unknown");
    assert_eq!(receipt["errorCode"], "agent.apply_patch.approval_in_flight");
    assert!(receipt["error"].as_str().unwrap().contains("未再次执行"));
    assert!(!target.exists());
    assert!(storage
        .get_file_change_run_grant_for_pending_action(
            &pending_action_storage_id(run_id, action_id,)
        )
        .unwrap()
        .is_none());
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
    authorize_file_change_action(&input, &action, FileChangeAuthorizationSource::Automatic)
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

#[tokio::test]
async fn manual_file_change_approve_rpc_has_only_the_strict_typed_result() {
    let fixture = tempdir().unwrap();
    let workspace = fixture.path().join("workspace");
    fs::create_dir(&workspace).unwrap();
    let workspace = fs::canonicalize(workspace).unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let service = AgentService::new(Arc::clone(&storage));
    let run_id = "run-manual-file-change-approve-json";
    let conversation_id = "conversation-manual-file-change-approve-json";
    let assistant_message_id = "assistant-manual-file-change-approve-json";
    let call_id = "call-manual-file-change-approve-json";
    let target = workspace.join("approve-json.txt");
    store_manual_direct_create(
        &service,
        &storage,
        &workspace,
        run_id,
        conversation_id,
        assistant_message_id,
        call_id,
        "approve-json.txt",
        "strict approve receipt\n",
    );

    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let output = service
        .approve_action(run_id, call_id, notifications)
        .expect("manual FileChange approval succeeds");
    let receipt = assert_strict_file_change_execution_json(&output, "applied");
    assert_eq!(receipt["status"], "applied");
    assert_eq!(receipt["outcome"], "applied");
    assert_eq!(
        fs::read_to_string(target).unwrap(),
        "strict approve receipt\n"
    );
    assert_file_change_receipt_surfaces_match(
        &storage,
        &pending_action_storage_id(run_id, call_id),
        run_id,
        &mut receiver,
        &receipt,
    );
}

#[tokio::test]
async fn manual_file_change_reject_rpc_has_only_the_strict_typed_result() {
    for (case, message, expected_message) in FILE_CHANGE_REJECTION_CASES {
        let fixture = tempdir().unwrap();
        let workspace = fixture.path().join("workspace");
        fs::create_dir(&workspace).unwrap();
        let workspace = fs::canonicalize(workspace).unwrap();
        let storage =
            Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
        let service = AgentService::new(Arc::clone(&storage));
        let run_id = "run-manual-file-change-reject-json";
        let conversation_id = "conversation-manual-file-change-reject-json";
        let assistant_message_id = "assistant-manual-file-change-reject-json";
        let call_id = "call-manual-file-change-reject-json";
        let target = workspace.join("reject-json.txt");
        store_manual_direct_create(
            &service,
            &storage,
            &workspace,
            run_id,
            conversation_id,
            assistant_message_id,
            call_id,
            "reject-json.txt",
            "must never be published\n",
        );

        let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let output = service
            .reject_action(run_id, call_id, message.map(str::to_string), notifications)
            .unwrap_or_else(|error| panic!("{case}: manual FileChange rejection failed: {error}"));
        output
            .file_change_result
            .as_ref()
            .expect("rejection returns a typed FileChange receipt")
            .validate()
            .unwrap_or_else(|error| panic!("{case}: invalid rejection receipt: {error}"));
        let receipt = assert_strict_file_change_execution_json(&output, "rejected");
        assert_eq!(receipt["status"], "rejected", "{case}");
        assert_eq!(receipt["outcome"], "definitely_not_executed", "{case}");
        assert_eq!(receipt["message"], expected_message, "{case}");
        assert!(receipt["error"].is_null(), "{case}");
        assert!(receipt["errorCode"].is_null(), "{case}");
        assert_eq!(
            output.agent_output.status,
            AgentRunStatus::Running,
            "{case}"
        );
        assert!(
            !target.exists(),
            "{case}: rejection must not create the target"
        );
        assert_file_change_receipt_surfaces_match(
            &storage,
            &pending_action_storage_id(run_id, call_id),
            run_id,
            &mut receiver,
            &receipt,
        );
    }
}

#[test]
fn direct_file_change_blank_rejection_never_mutates_create_update_or_delete() {
    for (operation, original, replacement) in [
        (
            AgentFileChangeOperation::Create,
            None,
            Some("new content\n"),
        ),
        (
            AgentFileChangeOperation::Update,
            Some("original content\n"),
            Some("replacement content\n"),
        ),
        (
            AgentFileChangeOperation::Delete,
            Some("original content\n"),
            None,
        ),
    ] {
        let fixture = tempdir().unwrap();
        let workspace = fixture.path().join("workspace");
        fs::create_dir(&workspace).unwrap();
        let workspace = fs::canonicalize(workspace).unwrap();
        let storage = StorageService::open(&fixture.path().join("storage.sqlite")).unwrap();
        let target = workspace.join("unchanged.txt");
        if let Some(content) = original {
            fs::write(&target, content).unwrap();
        }
        let run_id = "run-direct-blank-rejection";
        let conversation_id = "conversation-direct-blank-rejection";
        let (mut action, call) = direct_file_change_fixture(
            run_id,
            conversation_id,
            "call-direct-blank-rejection",
            ("unchanged.txt", target.to_str().unwrap()),
            original,
            replacement,
            AgentApprovalStatus::Required,
        );
        let input = direct_execution_input(
            &workspace,
            run_id,
            conversation_id,
            AgentPermissions::default(),
            &call,
        );
        bind_direct_execution_to_input(&mut action, &input);
        let record = pending_file_change_record(run_id, conversation_id, action, &call, input);
        let restored_call = tool_call_for_pending_record(&record).unwrap();
        let decision = action_execution_for_decision(
            &storage,
            &record,
            &restored_call,
            AgentApprovalDecisionStatus::Rejected,
            Some("\t\r\n"),
        );
        let receipt = decision.file_change_result.as_ref().unwrap();
        receipt.validate().unwrap();
        assert_eq!(receipt.operation, operation);
        assert_eq!(receipt.status, AgentFileChangeResultStatus::Rejected);
        assert_eq!(
            receipt.outcome,
            mycopilot_core::AgentFileChangeOutcome::DefinitelyNotExecuted
        );
        assert_eq!(
            receipt.message.as_deref(),
            Some(DEFAULT_FILE_CHANGE_REJECTION_MESSAGE)
        );
        assert_eq!(decision.final_pending_status, PendingActionStatus::Rejected);
        assert!(decision.file_change.is_none());
        assert!(decision.tool_result.ok);
        assert_eq!(decision.tool_result.error, None);
        assert_eq!(decision.tool_result.result, Some(json!(receipt)));
        assert_eq!(fs::read_to_string(&target).ok().as_deref(), original);
        assert_eq!(
            fs::read_dir(&workspace).unwrap().count(),
            usize::from(original.is_some()),
            "{operation:?}: rejection must not leave temporary files"
        );
    }
}

#[tokio::test]
async fn manual_file_change_audit_failure_publishes_one_typed_outcome_unknown_receipt() {
    let fixture = tempdir().unwrap();
    let workspace = fixture.path().join("workspace");
    fs::create_dir(&workspace).unwrap();
    let workspace = fs::canonicalize(workspace).unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let service = AgentService::new(Arc::clone(&storage));
    let run_id = "run-manual-file-change-audit-failure-json";
    let conversation_id = "conversation-manual-file-change-audit-failure-json";
    let assistant_message_id = "assistant-manual-file-change-audit-failure-json";
    let call_id = "call-manual-file-change-audit-failure-json";
    let target = workspace.join("audit-failure-json.txt");
    store_manual_direct_create(
        &service,
        &storage,
        &workspace,
        run_id,
        conversation_id,
        assistant_message_id,
        call_id,
        "audit-failure-json.txt",
        "effect may already exist\n",
    );
    let storage_id = pending_action_storage_id(run_id, call_id);
    // Exhaust the two commits of the original `applied` receipt. The subsequent fallback commit
    // must use the strict FileChange contract and become the only authoritative terminal fact.
    inject_manual_action_audit_failure(&storage_id, "completed");
    inject_manual_action_audit_failure(&storage_id, "completed");

    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let output = service
        .approve_action(run_id, call_id, notifications)
        .expect("typed outcome_unknown fallback settles durably");
    let receipt = assert_strict_file_change_execution_json(&output, "outcome_unknown");
    assert_eq!(receipt["status"], "outcome_unknown");
    assert_eq!(receipt["outcome"], "outcome_unknown");
    assert_eq!(receipt["errorCode"], "agent.apply_patch.outcome_unknown");
    assert_eq!(
        fs::read_to_string(target).unwrap(),
        "effect may already exist\n"
    );
    assert_file_change_receipt_surfaces_match(
        &storage,
        &storage_id,
        run_id,
        &mut receiver,
        &receipt,
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
    authorize_file_change_action(&input, &action, FileChangeAuthorizationSource::ExplicitUser)
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
    authorize_file_change_action(&input, &action, FileChangeAuthorizationSource::ExplicitUser)
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

fn active_run_grant_continuation_fixture(
    fixture: &tempfile::TempDir,
    run_id: &str,
    conversation_id: &str,
) -> (
    Arc<StorageService>,
    AgentService,
    PendingActionRecord,
    AgentChatInput,
) {
    let database_path = fixture.path().join("storage.sqlite");
    let workspace = fixture.path().join("workspace");
    fs::create_dir(&workspace).unwrap();
    let workspace = fs::canonicalize(workspace).unwrap();
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    let service = AgentService::new(Arc::clone(&storage));
    let call_id = format!("call-{run_id}");
    store_manual_direct_create(
        &service,
        &storage,
        &workspace,
        run_id,
        conversation_id,
        &format!("assistant-{run_id}"),
        &call_id,
        "created.txt",
        "created\n",
    );
    let (notifications, _receiver) = tokio::sync::mpsc::unbounded_channel();
    let result = service
        .approve_action_with_scope(
            run_id,
            &call_id,
            AgentApprovalScopeDto::RemainingApplyPatchInRun,
            notifications,
        )
        .expect("the explicit FileChange must commit before the continuation is scheduled");
    assert_eq!(result.status, "applied");
    assert!(storage
        .get_active_file_change_run_grant(run_id)
        .unwrap()
        .is_some());
    let storage_id = pending_action_storage_id(run_id, &call_id);
    let record = service
        .pending_actions
        .lock()
        .unwrap_or_else(|error| error.into_inner())[&storage_id]
        .clone();
    let resumed_input = record.agent_input.clone();
    (storage, service, record, resumed_input)
}

fn mismatched_collaboration_identity(
    conversation_id: &str,
) -> mycopilot_core::AgentCollaborationIdentity {
    mycopilot_core::AgentCollaborationIdentity {
        agent_id: "agent-untrusted-resume".to_string(),
        root_agent_id: "agent-root".to_string(),
        root_conversation_id: conversation_id.to_string(),
        parent_agent_id: "agent-root".to_string(),
        parent_task_name: "Root".to_string(),
        parent_task_path: "root".to_string(),
        conversation_id: conversation_id.to_string(),
        task_name: "untrusted".to_string(),
        task_path: "root/untrusted".to_string(),
        source_agent_id: "agent-root".to_string(),
        source_kind: mycopilot_core::AgentMailboxKind::Task,
        source_task_name: "Root".to_string(),
        source_task_path: "root".to_string(),
        source_agent_message_id: "mailbox-untrusted".to_string(),
        entrusted_task: "Untrusted resumed identity fixture".to_string(),
        template_instructions: None,
    }
}

#[tokio::test]
async fn identity_mismatch_terminalizes_claimed_continuation_and_revokes_active_run_grant() {
    let fixture = tempdir().unwrap();
    let run_id = "run-grant-identity-mismatch";
    let conversation_id = "conversation-grant-identity-mismatch";
    let (storage, service, record, mut resumed_input) =
        active_run_grant_continuation_fixture(&fixture, run_id, conversation_id);
    resumed_input
        .context
        .as_mut()
        .unwrap()
        .collaboration_identity = Some(mismatched_collaboration_identity(conversation_id));
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();

    service
        .run_action_continuation(
            record.clone(),
            resumed_input,
            notifications,
            PendingActionStatus::Completed,
            None,
        )
        .await;

    assert!(storage
        .get_active_file_change_run_grant(run_id)
        .unwrap()
        .is_none());
    assert!(storage
        .list_active_file_change_run_grant_records()
        .unwrap()
        .is_empty());
    let pending = storage
        .get_pending_agent_action(&record.storage_id)
        .unwrap()
        .unwrap();
    assert_eq!(pending.status, "failed");
    assert_eq!(pending.target_status.as_deref(), Some("failed"));
    assert_eq!(
        storage
            .get_conversation_turn_trace(record.snapshot.assistant_message_id.as_deref().unwrap())
            .unwrap()
            .unwrap()
            .terminal_status,
        ConversationTurnTraceTerminalStatus::Failed
    );
    let events = std::iter::from_fn(|| receiver.try_recv().ok()).collect::<Vec<_>>();
    assert!(events.iter().any(|event| {
        event["params"]["type"] == "error"
            && event["params"]["code"] == "collaboration_identity_mismatch"
            && event["params"]["recoverable"] == false
    }));
    assert!(events.iter().any(|event| {
        event["params"]["type"] == "done"
            && event["params"]["status"] == "failed"
            && event["params"]["success"] == false
    }));
}

#[tokio::test]
async fn foreign_turn_owner_revokes_active_run_grant_but_preserves_exact_recovery_state() {
    let fixture = tempdir().unwrap();
    let run_id = "run-grant-foreign-owner";
    let conversation_id = "conversation-grant-foreign-owner";
    let (storage, service, record, resumed_input) =
        active_run_grant_continuation_fixture(&fixture, run_id, conversation_id);
    service
        .active_conversation_turns
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .insert(
            conversation_id.to_string(),
            ActiveConversationTurn {
                run_id: "foreign-run".to_string(),
                assistant_message_id: "foreign-assistant".to_string(),
            },
        );
    let pending_before = storage
        .get_pending_agent_action(&record.storage_id)
        .unwrap()
        .unwrap();
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();

    service
        .run_action_continuation(
            record.clone(),
            resumed_input,
            notifications,
            PendingActionStatus::Completed,
            None,
        )
        .await;

    assert!(storage
        .get_active_file_change_run_grant(run_id)
        .unwrap()
        .is_none());
    assert!(storage
        .list_active_file_change_run_grant_records()
        .unwrap()
        .is_empty());
    let pending_after = storage
        .get_pending_agent_action(&record.storage_id)
        .unwrap()
        .unwrap();
    assert_eq!(pending_after.status, pending_before.status);
    assert_eq!(pending_after.target_status, pending_before.target_status);
    assert_eq!(pending_after.action_json, pending_before.action_json);
    assert_eq!(
        storage
            .get_conversation_turn_trace(record.snapshot.assistant_message_id.as_deref().unwrap())
            .unwrap()
            .unwrap()
            .terminal_status,
        ConversationTurnTraceTerminalStatus::InProgress
    );
    let events = std::iter::from_fn(|| receiver.try_recv().ok()).collect::<Vec<_>>();
    assert!(events.iter().any(|event| {
        event["params"]["type"] == "error"
            && event["params"]["code"] == "conversation_turn_ownership_conflict"
            && event["params"]["recoverable"] == true
    }));
    assert!(!events.iter().any(|event| {
        event["params"]["type"] == "done" || event["params"]["type"] == "tool_result"
    }));
}

#[tokio::test]
async fn pre_runtime_restore_failure_atomically_terminalizes_and_revokes_active_run_grant() {
    let fixture = tempdir().unwrap();
    let run_id = "run-grant-pre-runtime-failure";
    let conversation_id = "conversation-grant-pre-runtime-failure";
    let (storage, service, record, mut resumed_input) =
        active_run_grant_continuation_fixture(&fixture, run_id, conversation_id);
    resumed_input
        .resume_checkpoint
        .as_mut()
        .unwrap()
        .extension_snapshots
        .push(mycopilot_core::AgentExtensionSnapshot {
            extension_id: "skills".to_string(),
            version: u32::MAX,
            state: json!({}),
        });
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();

    service
        .run_action_continuation(
            record.clone(),
            resumed_input,
            notifications,
            PendingActionStatus::Completed,
            None,
        )
        .await;

    assert!(storage
        .get_active_file_change_run_grant(run_id)
        .unwrap()
        .is_none());
    assert!(storage
        .list_active_file_change_run_grant_records()
        .unwrap()
        .is_empty());
    let pending = storage
        .get_pending_agent_action(&record.storage_id)
        .unwrap()
        .unwrap();
    assert_eq!(pending.status, "failed");
    assert_eq!(pending.target_status.as_deref(), Some("failed"));
    assert_eq!(
        storage
            .get_conversation_turn_trace(record.snapshot.assistant_message_id.as_deref().unwrap())
            .unwrap()
            .unwrap()
            .terminal_status,
        ConversationTurnTraceTerminalStatus::Failed
    );
    let events = std::iter::from_fn(|| receiver.try_recv().ok()).collect::<Vec<_>>();
    assert!(events.iter().any(|event| {
        event["params"]["type"] == "error"
            && event["params"]["code"] == "skill_resource_snapshot_unavailable"
            && event["params"]["recoverable"] == false
    }));
}

#[tokio::test]
async fn cancelled_pre_runtime_continuation_revokes_active_run_grant_before_done() {
    let fixture = tempdir().unwrap();
    let run_id = "run-grant-pre-runtime-cancelled";
    let conversation_id = "conversation-grant-pre-runtime-cancelled";
    let (storage, service, record, resumed_input) =
        active_run_grant_continuation_fixture(&fixture, run_id, conversation_id);
    let cancellation = AgentCancellationToken::new();
    cancellation.cancel();
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();

    service
        .run_action_continuation(
            record.clone(),
            resumed_input,
            notifications,
            PendingActionStatus::Completed,
            Some(cancellation),
        )
        .await;

    assert!(storage
        .get_active_file_change_run_grant(run_id)
        .unwrap()
        .is_none());
    assert!(storage
        .list_active_file_change_run_grant_records()
        .unwrap()
        .is_empty());
    let pending = storage
        .get_pending_agent_action(&record.storage_id)
        .unwrap()
        .unwrap();
    assert_eq!(pending.status, "completed");
    assert_eq!(pending.target_status.as_deref(), Some("completed"));
    assert_eq!(
        storage
            .get_conversation_turn_trace(record.snapshot.assistant_message_id.as_deref().unwrap())
            .unwrap()
            .unwrap()
            .terminal_status,
        ConversationTurnTraceTerminalStatus::Cancelled
    );
    let events = std::iter::from_fn(|| receiver.try_recv().ok()).collect::<Vec<_>>();
    let done = events
        .iter()
        .filter(|event| event["params"]["type"] == "done")
        .collect::<Vec<_>>();
    assert_eq!(done.len(), 1, "events: {events:#?}");
    assert_eq!(done[0]["params"]["status"], "cancelled");
    assert_eq!(done[0]["params"]["success"], false);
}

fn seed_file_change_history_owner(
    storage: &StorageService,
    conversation_id: &str,
    assistant_message_id: &str,
) {
    storage
        .save_conversation(ChatConversationRecord {
            id: conversation_id.to_string(),
            project_id: None,
            model_id: Some("test-model".to_string()),
            title: "FileChange history".to_string(),
            messages: vec![ChatMessageRecord {
                human_interaction_response: None,
                id: assistant_message_id.to_string(),
                role: "assistant".to_string(),
                content: "done".to_string(),
                created_at: 1,
                status: Some("sent".to_string()),
                attachments: Vec::new(),
                agent_run_json: None,
                ui_state_json: None,
            }],
            created_at: 1,
            updated_at: 2,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
}

fn terminal_file_change_history_audit(
    run_id: &str,
    conversation_id: &str,
    assistant_message_id: &str,
    call_id: &str,
    action: &AgentProposedAction,
) -> AgentActionAuditRecord {
    AgentActionAuditRecord {
        action_id: mycopilot_core::canonical_pending_action_id(run_id, call_id),
        run_id: run_id.to_string(),
        conversation_id: Some(conversation_id.to_string()),
        assistant_message_id: Some(assistant_message_id.to_string()),
        action_type: "file_change".to_string(),
        tool_name: "apply_patch".to_string(),
        decision: Some("approved".to_string()),
        status: "completed".to_string(),
        action_json: serde_json::to_string(action).unwrap(),
        file_change_result_json: None,
        command_result_json: None,
        tool_result_json: None,
        error: None,
        created_at: 1,
        decided_at: Some(2),
        completed_at: Some(3),
        effective_permissions_json: Some("{}".to_string()),
        path_scope: None,
        command_cwd_scope: None,
        blocked_reason: None,
        decision_source: Some("manual".to_string()),
    }
}

#[test]
fn completed_direct_file_change_history_returns_only_the_saved_inline_diff() {
    let fixture = tempdir().unwrap();
    let workspace = fixture.path().join("workspace");
    fs::create_dir(&workspace).unwrap();
    let target = workspace.join("history-direct.txt");
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let service = AgentService::new(Arc::clone(&storage));
    let run_id = "run-history-direct";
    let conversation_id = "conversation-history-direct";
    let assistant_message_id = "assistant-history-direct";
    let call_id = "call-history-direct";
    seed_file_change_history_owner(&storage, conversation_id, assistant_message_id);
    let (action, _) = direct_file_change_fixture(
        run_id,
        conversation_id,
        call_id,
        ("history-direct.txt", target.to_str().unwrap()),
        None,
        Some("saved direct content\n"),
        AgentApprovalStatus::Approved,
    );
    let expected_patch = match &action {
        AgentProposedAction::FileChange { file_change } => file_change
            .inline_diff
            .as_ref()
            .expect("Direct proposal has an inline Diff")
            .patch
            .clone(),
        _ => unreachable!(),
    };
    storage
        .insert_agent_action_audit_if_absent(terminal_file_change_history_audit(
            run_id,
            conversation_id,
            assistant_message_id,
            call_id,
            &action,
        ))
        .unwrap();

    let page = service
        .get_file_change_history_diff(&mycopilot_protocol_rs::AgentFileChangeHistoryDiffRequest {
            conversation_id: conversation_id.to_string(),
            assistant_message_id: assistant_message_id.to_string(),
            run_id: run_id.to_string(),
            tool_call_id: call_id.to_string(),
            observer_root_conversation_id: None,
            offset: Some(0),
            max_chars: Some(50_000),
        })
        .unwrap();
    assert_eq!(page.patch, expected_patch);
    let serialized = serde_json::to_value(page).unwrap();
    assert_exact_json_object_keys(
        &serialized,
        &[
            "conversationId",
            "assistantMessageId",
            "runId",
            "toolCallId",
            "patch",
            "offset",
            "nextOffset",
            "truncated",
        ],
    );
    assert!(serialized.get("actionJson").is_none());
    assert!(serialized.get("execution").is_none());
}

#[test]
fn completed_staged_file_change_history_rebuilds_diff_from_the_saved_binding() {
    let fixture = tempdir().unwrap();
    let workspace = fixture.path().join("workspace");
    fs::create_dir(&workspace).unwrap();
    let target = workspace.join("history-staged.txt");
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let service = AgentService::new(Arc::clone(&storage));
    let run_id = "run-history-staged";
    let conversation_id = "conversation-history-staged";
    let assistant_message_id = "assistant-history-staged";
    let call_id = "call-history-staged";
    seed_file_change_history_owner(&storage, conversation_id, assistant_message_id);
    let (proposal, _) = staged_file_change_fixture(
        StagedFileChangeFixtureIdentity {
            run_id,
            conversation_id,
            call_id,
            transaction_id: "transaction-history-staged",
        },
        ("history-staged.txt", target.to_str().unwrap()),
        "saved staged content\n",
        AgentApprovalStatus::Approved,
    );
    let expected_patch =
        mycopilot_core::file_change::FileChangePlan::from_binding(&proposal.execution)
            .unwrap()
            .diff;
    let action = AgentProposedAction::FileChange {
        file_change: proposal,
    };
    storage
        .insert_agent_action_audit_if_absent(terminal_file_change_history_audit(
            run_id,
            conversation_id,
            assistant_message_id,
            call_id,
            &action,
        ))
        .unwrap();

    let page = service
        .get_file_change_history_diff(&mycopilot_protocol_rs::AgentFileChangeHistoryDiffRequest {
            conversation_id: conversation_id.to_string(),
            assistant_message_id: assistant_message_id.to_string(),
            run_id: run_id.to_string(),
            tool_call_id: call_id.to_string(),
            observer_root_conversation_id: None,
            offset: Some(0),
            max_chars: Some(50_000),
        })
        .unwrap();
    assert_eq!(page.patch, expected_patch);
}
