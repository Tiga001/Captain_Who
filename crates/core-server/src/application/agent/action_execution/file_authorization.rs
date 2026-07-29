use super::*;

pub(crate) fn authorize_structured_file_write(
    agent_input: &AgentChatInput,
    action: &AgentProposedAction,
    source: FileWriteAuthorizationSource,
) -> AgentResult<()> {
    if !proposed_action_uses_file_write_policy(action) {
        return Ok(());
    }

    if source == FileWriteAuthorizationSource::Automatic
        && file_write_action_approval_status(action) != Some(AgentApprovalStatus::Approved)
    {
        return Err(AgentError::structured(
            "agent.file_write_authorization_denied",
            "The file change does not carry an approved action snapshot.",
            serde_json::json!({
                "type": "file_write_policy",
                "code": "actionNotApproved",
                "recovery": "retry",
            }),
        ));
    }

    let permissions = permissions_from_input(agent_input);
    if file_write_authorized(permissions, source) {
        return Ok(());
    }

    let (code, recovery, message) = match file_write_approval_route(permissions) {
        FileWriteApprovalRoute::Denied => (
            "writePermissionDenied",
            "changePermissions",
            "The current permission policy does not allow file changes.",
        ),
        FileWriteApprovalRoute::RequireExplicitApproval => (
            "explicitApprovalRequired",
            "requestApproval",
            "This file change requires explicit user approval.",
        ),
        FileWriteApprovalRoute::AutoApprove => (
            "authorizationSourceInvalid",
            "retry",
            "The file change was routed through an invalid authorization source.",
        ),
    };
    Err(AgentError::structured(
        "agent.file_write_authorization_denied",
        message,
        serde_json::json!({
            "type": "file_write_policy",
            "code": code,
            "recovery": recovery,
        }),
    ))
}
