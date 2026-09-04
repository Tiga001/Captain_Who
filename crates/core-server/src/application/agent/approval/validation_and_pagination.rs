fn approval_scope_conflict_message() -> String {
    "The approval scope differs from the durable decision for this action; the action was not executed again."
        .to_string()
}

fn exact_file_change_for_approval_retry(
    pending: &AgentPendingActionRecord,
    run_id: &str,
    action_id: &str,
    storage_id: &str,
) -> Result<Option<mycopilot_core::AgentFileChangeProposal>, String> {
    if pending.action_id != storage_id
        || pending.run_id != run_id
        || pending.updated_at < pending.created_at
    {
        return Err(
            "The durable pending action identity is invalid; the action was not executed again."
                .to_string(),
        );
    }
    let action: AgentProposedAction = serde_json::from_str(&pending.action_json).map_err(|_| {
        "The durable pending action is invalid; the action was not executed again.".to_string()
    })?;
    let AgentProposedAction::FileChange { file_change } = action else {
        return Ok(None);
    };
    if pending.tool_call_id.as_deref() != Some(action_id)
        || pending.action_type != "file_change"
        || pending.tool_name != "apply_patch"
        || file_change.id != action_id
        || file_change.execution.run_id != run_id
        || file_change.execution.source_tool_name != "apply_patch"
        || file_change.execution.source_call_id != action_id
        || file_change.approval_status != AgentApprovalStatus::Required
        || file_change.validate().is_err()
        || file_change.execution.validate().is_err()
    {
        return Err(
            "The durable FileChange identity is invalid; the action was not executed again."
                .to_string(),
        );
    }
    Ok(Some(file_change))
}

fn validate_file_change_approval_audit_identity(
    audit: &AgentActionAuditRecord,
    pending: &AgentPendingActionRecord,
    file_change: &mycopilot_core::AgentFileChangeProposal,
    run_id: &str,
    action_id: &str,
    storage_id: &str,
) -> Result<(), String> {
    let audit_action: AgentProposedAction =
        serde_json::from_str(&audit.action_json).map_err(|_| {
            "The durable approval receipt action is invalid; the action was not executed again."
                .to_string()
        })?;
    let AgentProposedAction::FileChange {
        file_change: audit_file_change,
    } = audit_action
    else {
        return Err(
            "The durable approval receipt is not a FileChange; the action was not executed again."
                .to_string(),
        );
    };
    let audit_action_value = serde_json::to_value(&audit_file_change).map_err(|_| {
        "The durable approval receipt action could not be verified; the action was not executed again."
            .to_string()
    })?;
    let pending_action_value = serde_json::to_value(file_change).map_err(|_| {
        "The frozen pending action could not be verified; the action was not executed again."
            .to_string()
    })?;
    if audit.action_id != storage_id
        || audit.run_id != run_id
        || audit.conversation_id != pending.conversation_id
        || audit.assistant_message_id != pending.assistant_message_id
        || audit.action_type != "file_change"
        || audit.tool_name != "apply_patch"
        || audit.created_at != pending.created_at
        || audit_file_change.id != action_id
        || audit_file_change.execution.source_call_id != action_id
        || audit_file_change.execution.run_id != run_id
        || audit_action_value != pending_action_value
    {
        return Err(
            "The durable approval receipt identity differs from the pending action; the action was not executed again."
                .to_string(),
        );
    }
    Ok(())
}

fn validate_replayed_file_change_result(
    file_change: &mycopilot_core::AgentFileChangeProposal,
    result: &AgentFileChangeResult,
) -> Result<(), String> {
    if !mycopilot_core::file_change_support::file_change_result_matches_frozen_proposal(
        result,
        file_change,
    ) {
        return Err(
            "The durable FileChange result differs from its frozen proposal; the action was not executed again."
                .to_string(),
        );
    }
    Ok(())
}

fn file_change_result_execution_status(
    status: mycopilot_core::AgentFileChangeResultStatus,
) -> &'static str {
    match status {
        mycopilot_core::AgentFileChangeResultStatus::Applied
        | mycopilot_core::AgentFileChangeResultStatus::AlreadyApplied => "applied",
        mycopilot_core::AgentFileChangeResultStatus::Failed => "failed",
        mycopilot_core::AgentFileChangeResultStatus::Conflict => "conflict",
        mycopilot_core::AgentFileChangeResultStatus::Rejected => "rejected",
        mycopilot_core::AgentFileChangeResultStatus::OutcomeUnknown => "outcome_unknown",
        mycopilot_core::AgentFileChangeResultStatus::Aborted => "cancelled",
        mycopilot_core::AgentFileChangeResultStatus::Expired => "expired",
    }
}

fn in_flight_file_change_approval_output(
    run_id: &str,
    action_id: &str,
    file_change: &mycopilot_core::AgentFileChangeProposal,
) -> AgentActionExecutionOutput {
    let result = file_change_result(
        file_change,
        mycopilot_core::AgentFileChangeResultStatus::OutcomeUnknown,
        mycopilot_core::AgentFileChangeOutcome::OutcomeUnknown,
        None,
        Some("agent.apply_patch.approval_in_flight"),
        Some("原审批正在执行或可能已经执行；系统未再次执行。"),
        Some("请等待权威结果事件；若应用已重启，请等待安全恢复完成后再检查文件。"),
    );
    file_change_approval_output(run_id, action_id, "outcome_unknown", result)
}

fn file_change_approval_output(
    run_id: &str,
    action_id: &str,
    status: &str,
    result: AgentFileChangeResult,
) -> AgentActionExecutionOutput {
    AgentActionExecutionOutput {
        action_id: action_id.to_string(),
        action_type: "file_change".to_string(),
        tool_name: "apply_patch".to_string(),
        status: status.to_string(),
        file_change_result: Some(result),
        command_result: None,
        tool_result: None,
        agent_output: AgentChatOutput {
            content: String::new(),
            status: AgentRunStatus::Running,
            run_id: run_id.to_string(),
            events: Vec::new(),
            tool_definitions: Vec::new(),
            todo: None,
            usage: None,
            finish_reason: None,
            proposed_actions: Vec::new(),
            conversation_turn_trace: None,
        },
    }
}

pub(super) fn paginate_chars(
    value: &str,
    offset: Option<usize>,
    max_chars: Option<usize>,
) -> (String, usize, Option<usize>, bool) {
    let offset = offset.unwrap_or(0);
    let limit = max_chars.unwrap_or(50_000).clamp(1_000, 100_000);
    let total = value.chars().count();
    let start = offset.min(total);
    let end = start.saturating_add(limit).min(total);
    let content = value.chars().skip(start).take(end - start).collect();
    let truncated = end < total;
    (content, start, truncated.then_some(end), truncated)
}
