use super::*;

pub(crate) fn authorize_file_change_action(
    agent_input: &AgentChatInput,
    action: &AgentProposedAction,
    source: FileChangeAuthorizationSource,
) -> AgentResult<()> {
    if !proposed_action_uses_file_change_policy(action) {
        return Ok(());
    }

    if source == FileChangeAuthorizationSource::Automatic
        && file_change_action_approval_status(action) != Some(AgentApprovalStatus::Approved)
    {
        return Err(AgentError::structured(
            "agent.file_change_authorization_denied",
            "The file change does not carry an approved action snapshot.",
            serde_json::json!({
                "type": "file_change_policy",
                "code": "actionNotApproved",
                "recovery": "retry",
            }),
        ));
    }
    if source == FileChangeAuthorizationSource::RunGrant
        && (!matches!(action, AgentProposedAction::FileChange { .. })
            || file_change_action_approval_status(action) != Some(AgentApprovalStatus::Approved))
    {
        return Err(AgentError::structured(
            "agent.file_change_authorization_denied",
            "The Run grant does not authorize this action.",
            serde_json::json!({
                "type": "file_change_policy",
                "code": "runGrantActionInvalid",
                "recovery": "requestApproval",
            }),
        ));
    }

    let permissions = permissions_from_input(agent_input);
    if file_change_authorized(permissions, source) {
        return Ok(());
    }

    let (code, recovery, message) = match file_change_approval_route(permissions) {
        FileChangeApprovalRoute::Denied => (
            "writePermissionDenied",
            "changePermissions",
            "The current permission policy does not allow file changes.",
        ),
        FileChangeApprovalRoute::RequireExplicitApproval => (
            "explicitApprovalRequired",
            "requestApproval",
            "This file change requires explicit user approval.",
        ),
        FileChangeApprovalRoute::AutoApprove => (
            "authorizationSourceInvalid",
            "retry",
            "The file change was routed through an invalid authorization source.",
        ),
    };
    Err(AgentError::structured(
        "agent.file_change_authorization_denied",
        message,
        serde_json::json!({
            "type": "file_change_policy",
            "code": code,
            "recovery": recovery,
        }),
    ))
}

pub(crate) fn file_change_authorization_source(
    agent_input: &AgentChatInput,
    action: &AgentProposedAction,
) -> AgentResult<FileChangeAuthorizationSource> {
    let grant_ref = agent_input
        .resume_checkpoint
        .as_ref()
        .and_then(|checkpoint| checkpoint.file_change_run_grant_ref.as_ref());
    match (action, grant_ref) {
        (AgentProposedAction::FileChange { .. }, Some(grant_ref)) => {
            grant_ref.validate().map_err(|_| {
                AgentError::structured(
                    "agent.file_change_run_grant_invalid",
                    "The FileChange Run grant is invalid or no longer active.",
                    serde_json::json!({
                        "type": "file_change_policy",
                        "code": "runGrantInvalid",
                        "outcome": "definitely_not_executed",
                        "recovery": "requestApproval",
                    }),
                )
            })?;
            Ok(FileChangeAuthorizationSource::RunGrant)
        }
        (_, Some(_)) => Err(AgentError::structured(
            "agent.file_change_run_grant_invalid",
            "A FileChange Run grant cannot authorize this action.",
            serde_json::json!({
                "type": "file_change_policy",
                "code": "runGrantActionInvalid",
                "outcome": "definitely_not_executed",
                "recovery": "requestApproval",
            }),
        )),
        _ => Ok(FileChangeAuthorizationSource::Automatic),
    }
}

pub(crate) fn validate_file_change_run_grant_at_effect_boundary(
    storage: &StorageService,
    agent_input: &AgentChatInput,
    action: &AgentProposedAction,
) -> AgentResult<()> {
    let Some(grant_ref) = agent_input
        .resume_checkpoint
        .as_ref()
        .and_then(|checkpoint| checkpoint.file_change_run_grant_ref.as_ref())
    else {
        return Ok(());
    };
    let AgentProposedAction::FileChange { file_change } = action else {
        return Err(AgentError::new(
            "FileChange Run grant was attached to a non-FileChange action.",
        ));
    };
    let run_context = agent_input.context.as_ref().ok_or_else(|| {
        AgentError::new("FileChange Run grant is missing its frozen Run context.")
    })?;
    if storage
        .validate_file_change_run_grant(grant_ref, file_change, run_context)
        .map_err(
            mycopilot_core::storage::service::FileChangeRunGrantServiceError::into_agent_error,
        )?
    {
        Ok(())
    } else {
        Err(AgentError::structured(
            "agent.file_change_run_grant_invalid",
            "The FileChange Run grant changed or no longer authorizes this target.",
            serde_json::json!({
                "type": "file_change_policy",
                "code": "runGrantStale",
                "outcome": "definitely_not_executed",
                "recovery": "requestApproval",
            }),
        ))
    }
}
