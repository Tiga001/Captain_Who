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
