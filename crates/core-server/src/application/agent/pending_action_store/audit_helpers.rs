pub(super) fn file_change_result_for_audit(
    record: &PendingActionRecord,
    tool_result: &AgentToolResult,
) -> Result<Option<AgentFileChangeResult>, String> {
    let AgentProposedAction::FileChange { file_change } = &record.snapshot.action else {
        return Ok(None);
    };
    if tool_result.call_id != file_change.id
        || tool_result.tool != "apply_patch"
        || record.snapshot.action_type != "file_change"
        || record.snapshot.tool_name != "apply_patch"
    {
        return Err("FileChange terminal ToolResult identity is invalid".to_string());
    }
    let result = serde_json::from_value::<AgentFileChangeResult>(
        tool_result
            .result
            .clone()
            .ok_or_else(|| "FileChange terminal ToolResult is missing its result".to_string())?,
    )
    .map_err(|_| "FileChange terminal ToolResult shape is invalid".to_string())?;
    if result.transaction_id != file_change.transaction_id
        || result.operation != file_change.operation
        || result.update_strategy != file_change.update_strategy
        || result.file_path != file_change.file_path
        || result.additions != file_change.additions
        || result.deletions != file_change.deletions
        || result.line_count != file_change.line_count
        || result.byte_count != file_change.byte_count
    {
        return Err("FileChange terminal ToolResult differs from its frozen proposal".to_string());
    }
    Ok(Some(result))
}

#[allow(clippy::too_many_arguments)]
pub(super) fn action_audit_record(
    record: &PendingActionRecord,
    decision: Option<&str>,
    status: &str,
    file_change_result: Option<&AgentFileChangeResult>,
    command_result: Option<&AgentCommandExecutionResult>,
    tool_result: Option<&AgentToolResult>,
    error: Option<&str>,
    decided_at: Option<i64>,
    completed_at: Option<i64>,
) -> AgentActionAuditRecord {
    AgentActionAuditRecord {
        action_id: record.storage_id.clone(),
        run_id: record.snapshot.run_id.clone(),
        conversation_id: record.snapshot.conversation_id.clone(),
        assistant_message_id: record.snapshot.assistant_message_id.clone(),
        action_type: record.snapshot.action_type.clone(),
        tool_name: record.snapshot.tool_name.clone(),
        decision: decision.map(ToString::to_string),
        status: status.to_string(),
        action_json: serialize_json(&record.snapshot.action),
        file_change_result_json: file_change_result.map(serialize_json),
        command_result_json: command_result.map(serialize_json),
        tool_result_json: tool_result.map(serialize_json),
        error: error.map(ToString::to_string),
        created_at: record.snapshot.created_at,
        decided_at,
        completed_at,
        effective_permissions_json: Some(serialize_json(&permissions_from_input(
            &record.agent_input,
        ))),
        path_scope: path_scope_for_action(&record.agent_input, &record.snapshot.action),
        command_cwd_scope: command_cwd_scope_for_action(
            &record.agent_input,
            &record.snapshot.action,
        ),
        blocked_reason: error.map(ToString::to_string),
        decision_source: Some(
            if decision.is_none() && status == "pending" {
                "manual_pending"
            } else {
                "manual"
            }
            .to_string(),
        ),
    }
}

#[allow(clippy::too_many_arguments)]
fn auto_action_audit_record(
    run_id: &str,
    conversation_id: Option<&str>,
    assistant_message_id: Option<&str>,
    agent_input: &AgentChatInput,
    action: &AgentProposedAction,
    status: &str,
    file_change_result: Option<&AgentFileChangeResult>,
    command_result: Option<&AgentCommandExecutionResult>,
    tool_result: Option<&AgentToolResult>,
    error: Option<&str>,
    created_at: i64,
    completed_at: Option<i64>,
) -> AgentActionAuditRecord {
    let provider_action_id = action_id_for_action(action);
    let decision_source = if matches!(action, AgentProposedAction::FileChange { .. })
        && agent_input
            .resume_checkpoint
            .as_ref()
            .and_then(|checkpoint| checkpoint.file_change_run_grant_ref.as_ref())
            .is_some()
    {
        "run_grant"
    } else {
        "auto"
    };
    AgentActionAuditRecord {
        action_id: pending_action_storage_id(run_id, &provider_action_id),
        run_id: run_id.to_string(),
        conversation_id: conversation_id.map(ToString::to_string),
        assistant_message_id: assistant_message_id.map(ToString::to_string),
        action_type: action_type_for_action(action).to_string(),
        tool_name: tool_name_for_action(action),
        decision: Some("approved".to_string()),
        status: status.to_string(),
        action_json: serialize_json(action),
        file_change_result_json: file_change_result.map(serialize_json),
        command_result_json: command_result.map(serialize_json),
        tool_result_json: tool_result.map(serialize_json),
        error: error.map(ToString::to_string),
        created_at,
        decided_at: Some(created_at),
        completed_at,
        effective_permissions_json: Some(serialize_json(&permissions_from_input(agent_input))),
        path_scope: path_scope_for_action(agent_input, action),
        command_cwd_scope: command_cwd_scope_for_action(agent_input, action),
        blocked_reason: error.map(ToString::to_string),
        decision_source: Some(decision_source.to_string()),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BuiltinMcpBindingApprovalStatus {
    Required,
    Approved,
}

pub(super) fn pending_action_binding_matches(
    run_id: &str,
    record: Option<&AgentPendingActionRecord>,
    action: &AgentProposedAction,
    agent_input: &AgentChatInput,
) -> bool {
    pending_action_binding_matches_with_builtin_status(
        run_id,
        record,
        action,
        agent_input,
        BuiltinMcpBindingApprovalStatus::Required,
    )
}

fn pending_action_binding_matches_for_auto_journal(
    run_id: &str,
    record: Option<&AgentPendingActionRecord>,
    action: &AgentProposedAction,
    agent_input: &AgentChatInput,
) -> bool {
    pending_action_binding_matches_with_builtin_status(
        run_id,
        record,
        action,
        agent_input,
        BuiltinMcpBindingApprovalStatus::Approved,
    )
}

fn pending_action_binding_matches_with_builtin_status(
    run_id: &str,
    record: Option<&AgentPendingActionRecord>,
    action: &AgentProposedAction,
    agent_input: &AgentChatInput,
    builtin_status: BuiltinMcpBindingApprovalStatus,
) -> bool {
    if matches!(
        action,
        AgentProposedAction::BuiltinCapabilityActivation { approval }
            if approval.run_id != run_id
    ) || matches!(
        action,
        AgentProposedAction::BuiltinMcpToolApproval { approval }
            if approval.identity.run_id != run_id
    ) {
        return false;
    }
    let action_id = action_id_for_action(action);
    let tool_name = tool_name_for_action(action);
    let (tool_call_id, pending_action_id) = match action {
        AgentProposedAction::McpToolCall { approval } => (
            approval.identity.call_id.as_str(),
            Some(approval.identity.action_id.clone()),
        ),
        AgentProposedAction::BuiltinCapabilityActivation { approval } => {
            (approval.call_id.as_str(), Some(approval.action_id.clone()))
        }
        AgentProposedAction::BuiltinMcpToolApproval { approval } => (
            approval.identity.call_id.as_str(),
            Some(approval.identity.action_id.clone()),
        ),
        AgentProposedAction::FileChange { file_change } => (
            file_change.id.as_str(),
            Some(pending_action_storage_id(run_id, &file_change.id)),
        ),
        _ => (action_id.as_str(), None),
    };
    let Some(checkpoint) = agent_input.resume_checkpoint.as_ref() else {
        return false;
    };
    if checkpoint.run_id != run_id
        || checkpoint.pending_action_id.as_deref() != pending_action_id.as_deref()
        || checkpoint.pending_tool_call_id != tool_call_id
    {
        return false;
    }
    let mut checkpoint_calls = checkpoint
        .context_items
        .iter()
        .flat_map(|item| item.tool_calls.iter())
        .filter(|call| call.id == tool_call_id);
    let Some(checkpoint_call) = checkpoint_calls.next() else {
        return false;
    };
    if checkpoint_calls.next().is_some()
        || checkpoint_call.name != tool_name
        || checkpoint_call.provider_identity.runtime_call_id != tool_call_id
    {
        return false;
    }
    if !record.is_none_or(|record| {
        record.action_type == action_type_for_action(action)
            && record.run_id == run_id
            && record.action_id == pending_action_storage_id(run_id, &action_id)
            && record.tool_call_id.as_deref() == Some(tool_call_id)
            && record.tool_name == tool_name
    }) {
        return false;
    }

    if let AgentProposedAction::BuiltinCapabilityActivation { approval } = action {
        return mycopilot_core::validate_frozen_builtin_capability_activation_args(
            approval,
            &checkpoint_call.args,
        )
        .is_ok();
    }

    if let AgentProposedAction::BuiltinMcpToolApproval { approval } = action {
        return mycopilot_core::validate_builtin_mcp_tool_approval_shape(approval).is_ok()
            && approval.approval_status
                == match builtin_status {
                    BuiltinMcpBindingApprovalStatus::Required => AgentApprovalStatus::Required,
                    BuiltinMcpBindingApprovalStatus::Approved => AgentApprovalStatus::Approved,
                }
            && approval.identity.run_id == run_id
            && approval.identity.call_id == checkpoint_call.id
            && approval.identity.model_name == checkpoint_call.name
            && checkpoint_call.args == serde_json::json!({});
    }

    if let AgentProposedAction::FileChange { file_change } = action {
        let Some(context) = agent_input.context.as_ref() else {
            return false;
        };
        let expected_approval_status = match builtin_status {
            BuiltinMcpBindingApprovalStatus::Required => AgentApprovalStatus::Required,
            BuiltinMcpBindingApprovalStatus::Approved => AgentApprovalStatus::Approved,
        };
        let args_digest = match mycopilot_core::file_change::proposal_digest(&checkpoint_call.args)
        {
            Ok(digest) => digest,
            Err(_) => return false,
        };
        let run_grant_shape_valid = match (
            builtin_status,
            checkpoint.file_change_run_grant_ref.as_ref(),
        ) {
            (BuiltinMcpBindingApprovalStatus::Required, None) => true,
            (BuiltinMcpBindingApprovalStatus::Required, Some(_)) => false,
            (BuiltinMcpBindingApprovalStatus::Approved, None) => true,
            (BuiltinMcpBindingApprovalStatus::Approved, Some(grant_ref)) => {
                grant_ref.validate().is_ok()
                    && matches!(
                        file_change.operation,
                        mycopilot_core::AgentFileChangeOperation::Create
                            | mycopilot_core::AgentFileChangeOperation::Update
                    )
            }
        };
        return file_change.validate().is_ok()
            && run_grant_shape_valid
            && file_change.approval_status == expected_approval_status
            && file_change.execution.source_tool_name == "apply_patch"
            && file_change.execution.source_call_id == checkpoint_call.id
            && file_change.execution.source_args_digest == args_digest
            && file_change.execution.run_id == run_id
            && context.conversation_id.as_deref()
                == Some(file_change.execution.conversation_id.as_str())
            && context.project_id.as_deref() == file_change.execution.project_id.as_deref();
    }

    let AgentProposedAction::McpToolCall { approval } = action else {
        return true;
    };
    let lifecycle_state = match approval.approval_mode {
        mycopilot_core::AgentMcpApprovalMode::Prompt => {
            AgentMcpToolInvocationState::PendingApproval
        }
        mycopilot_core::AgentMcpApprovalMode::Auto => AgentMcpToolInvocationState::Approved,
        mycopilot_core::AgentMcpApprovalMode::Deny => return false,
    };
    if mcp_tool_invocation_event(
        approval,
        McpToolInvocationEventUpdate {
            state: lifecycle_state,
            dispatch_certainty: AgentMcpDispatchCertainty::DefinitelyNotDispatched,
            outcome: None,
            is_error: None,
            error_code: None,
            duration_ms: None,
            output_truncated: false,
            result_size: None,
            failure_stage: None,
        },
    )
    .is_err()
    {
        return false;
    }
    let identity = &approval.identity;
    if identity.run_id != run_id
        || approval.call.id != identity.call_id
        || approval.call.tool != identity.provenance.model_tool_name
    {
        return false;
    }
    identity.run_id == checkpoint.run_id
}
