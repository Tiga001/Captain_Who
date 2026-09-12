pub(super) fn agent_input_with_run_checkpoint(
    agent_input: &AgentChatInput,
    checkpoint: &AgentRunCheckpoint,
) -> AgentChatInput {
    let mut resume_input = agent_input.clone();
    resume_input.messages.clear();
    resume_input.attachments.clear();
    resume_input.approval_decision = None;
    resume_input.tool_continuation = None;
    resume_input.resume_checkpoint = Some(checkpoint.clone());
    resume_input
}

pub(super) fn pending_status_from_label(value: &str) -> Option<PendingActionStatus> {
    match value {
        "pending" => Some(PendingActionStatus::Pending),
        "approved" => Some(PendingActionStatus::Approved),
        "executing" => Some(PendingActionStatus::Executing),
        "rejected" => Some(PendingActionStatus::Rejected),
        "cancelled" => Some(PendingActionStatus::Cancelled),
        "completed" => Some(PendingActionStatus::Completed),
        "failed" => Some(PendingActionStatus::Failed),
        _ => None,
    }
}

pub(super) fn ensure_pending_status_transition(
    current: PendingActionStatus,
    next: PendingActionStatus,
) -> Result<(), String> {
    let allowed = matches!(
        (current, next),
        (PendingActionStatus::Pending, PendingActionStatus::Approved)
            | (PendingActionStatus::Pending, PendingActionStatus::Executing)
            | (PendingActionStatus::Pending, PendingActionStatus::Rejected)
            | (PendingActionStatus::Pending, PendingActionStatus::Cancelled)
            | (PendingActionStatus::Approved, PendingActionStatus::Executing)
            | (PendingActionStatus::Approved, PendingActionStatus::Completed)
            | (PendingActionStatus::Approved, PendingActionStatus::Failed)
            | (PendingActionStatus::Approved, PendingActionStatus::Cancelled)
            | (PendingActionStatus::Executing, PendingActionStatus::Completed)
            | (PendingActionStatus::Executing, PendingActionStatus::Failed)
            | (PendingActionStatus::Executing, PendingActionStatus::Rejected)
            | (PendingActionStatus::Executing, PendingActionStatus::Cancelled)
            // Compensating rollback: cancellation is not made visible unless its paired
            // assistant/trace commit succeeds.
            | (PendingActionStatus::Executing, PendingActionStatus::Pending)
    );
    if allowed {
        Ok(())
    } else {
        Err(format!(
            "非法待审批状态迁移：{} -> {}",
            pending_status_label(current),
            pending_status_label(next)
        ))
    }
}

pub(super) fn path_scope_for_action(
    input: &AgentChatInput,
    action: &AgentProposedAction,
) -> Option<String> {
    if let AgentProposedAction::OfficeOperation { office_operation } = action {
        if !office_operation.prepared.paths.is_empty() {
            // A valid Office request is bounded by the provider argument limit. Keep a second
            // defensive bound for persisted/corrupt snapshots so audit generation itself cannot
            // amplify untrusted data. Valid plans retain every slot's purpose and scope.
            const MAX_AUDITED_OFFICE_PATHS: usize = 260;
            let paths = office_operation
                .prepared
                .paths
                .iter()
                .take(MAX_AUDITED_OFFICE_PATHS)
                .map(|path| {
                    serde_json::json!({
                        "slot": path.slot,
                        "purpose": path.purpose,
                        "scope": path.scope,
                    })
                })
                .collect::<Vec<_>>();
            return Some(serialize_json(&serde_json::json!({
                "schemaVersion": 1,
                "totalPathCount": office_operation.prepared.paths.len(),
                "truncated": office_operation.prepared.paths.len() > MAX_AUDITED_OFFICE_PATHS,
                "paths": paths,
            })));
        }
    }

    let path = match action {
        AgentProposedAction::FileChange { file_change } => Some(file_change.file_path.as_str()),
        AgentProposedAction::SkillMaterialization { materialization } => {
            Some(materialization.destination.as_str())
        }
        AgentProposedAction::SkillScript { .. } => None,
        AgentProposedAction::OfficeOperation { office_operation } => office_operation
            .prepared
            .request
            .destination_path
            .as_deref()
            .or(office_operation
                .prepared
                .request
                .output_path
                .as_deref()
                .or(office_operation.prepared.request.document_path.as_deref())),
        _ => None,
    }?;
    Some(scope_for_path(input, path))
}

pub(super) fn command_cwd_scope_for_action(
    input: &AgentChatInput,
    action: &AgentProposedAction,
) -> Option<String> {
    let cwd = match action {
        AgentProposedAction::Command { command } => command.cwd.as_deref().unwrap_or("."),
        AgentProposedAction::SkillScript { .. } => ".",
        AgentProposedAction::OfficeOperation { .. } => ".",
        AgentProposedAction::ToolCall { call } if call.tool == "run_command" => call
            .args
            .get("cwd")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("."),
        _ => return None,
    };
    Some(scope_for_path(input, cwd))
}

pub(super) fn scope_for_path(input: &AgentChatInput, path: &str) -> String {
    let workspace = input.context.as_ref().and_then(|context|context.workspace.as_ref());
    let resolver = mycopilot_core::workspace::WorkspaceResolver::from_context(workspace);
    if resolver.primary_root().is_none() { return "no_workspace".to_string(); }
    let trimmed = path.trim();
    if trimmed.starts_with("@workspace/") {
        return if resolver.resolve_input(trimmed).is_ok() { "workspace" } else { "unavailable_workspace" }.to_string();
    }
    if trimmed.starts_with('@') { return "system_alias".to_string(); }
    if Path::new(trimmed).is_absolute() {
        let inside = resolver.resolve_input(trimmed).ok().and_then(|path| {
            resolver.containing_root(&path).ok().flatten()
        }).is_some();
        return if inside { "workspace" } else { "outside_workspace" }.to_string();
    }
    "workspace".to_string()
}

pub(super) fn agent_input_project_id(input: &AgentChatInput) -> Option<&str> {
    input
        .context
        .as_ref()
        .and_then(|context| context.project_id.as_deref())
}

pub(super) fn agent_input_conversation_id(input: &AgentChatInput) -> Option<&str> {
    input
        .context
        .as_ref()
        .and_then(|context| context.conversation_id.as_deref())
}
