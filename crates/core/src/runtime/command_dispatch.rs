use super::*;

#[derive(Debug)]
pub(super) enum CommandDispatch {
    ExecuteAutomatically(AgentProposedAction),
    RequireApproval(AgentProposedAction),
    Reject(AgentToolResult),
}

/// Applies the command safety policy to the exact host-action snapshot that will either be
/// executed or persisted for approval.
///
/// Keeping policy evaluation and dispatch selection in one place prevents the runtime from
/// validating one command string and later recreating a different action for execution.
pub(super) fn prepare_command_dispatch(
    call: &AgentToolCall,
    action: AgentProposedAction,
    permissions: AgentPermissions,
    workspace_root: Option<&Path>,
    auto_approve: bool,
) -> CommandDispatch {
    let AgentProposedAction::Command { command } = &action else {
        return CommandDispatch::Reject(failed_tool_call_result(
            call,
            AgentError::new("run_command 未生成结构化命令操作，已拒绝执行。"),
        ));
    };
    let policy_cwd = command_policy_cwd(workspace_root, command.cwd.as_deref());
    let evaluation = evaluate_command_policy_with_context(
        &command.command,
        permissions,
        CommandAuthorizationSource::Automatic,
        workspace_root,
        policy_cwd.as_deref(),
    );
    match evaluation.decision {
        CommandPolicyDecision::Deny => CommandDispatch::Reject(AgentToolResult {
            call_id: call.id.clone(),
            tool: call.tool.clone(),
            ok: false,
            result: Some(json!({
                "type": "command_policy",
                "decision": evaluation.decision,
                "code": evaluation.code,
                "reason": evaluation.reason,
                "riskLevel": evaluation.risk_level,
                "findings": evaluation.findings,
            })),
            error: Some("命令已被不可绕过的安全策略拒绝。".to_string()),
        }),
        CommandPolicyDecision::RequireExplicitApproval => CommandDispatch::RequireApproval(action),
        CommandPolicyDecision::Allow if auto_approve => {
            CommandDispatch::ExecuteAutomatically(action)
        }
        CommandPolicyDecision::Allow => CommandDispatch::RequireApproval(action),
    }
}

pub(super) fn command_policy_cwd(
    workspace_root: Option<&Path>,
    requested_cwd: Option<&str>,
) -> Option<PathBuf> {
    match requested_cwd.map(Path::new) {
        None => workspace_root.map(Path::to_path_buf),
        Some(cwd) if cwd.is_absolute() => Some(cwd.to_path_buf()),
        Some(cwd) => workspace_root.map(|root| root.join(cwd)),
    }
}
