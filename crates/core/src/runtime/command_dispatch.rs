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
            exact_archive_file: None,
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

/// Routes an opaque, revision-bound Skill script through the same user
/// authorization dimensions as shell commands without pretending its body can
/// be classified as a shell command. Until Skill processes have an OS-level
/// filesystem/network sandbox and source trust grants, execution requires the
/// complete unrestricted scope and explicit approval for every script.
pub(super) fn prepare_skill_script_dispatch(
    call: &AgentToolCall,
    action: AgentProposedAction,
    permissions: AgentPermissions,
    workspace_root: Option<&Path>,
    _auto_approve: bool,
) -> CommandDispatch {
    let AgentProposedAction::SkillScript { script } = &action else {
        return CommandDispatch::Reject(failed_tool_call_result(
            call,
            AgentError::new("skills_run_script did not produce a structured Skill script action."),
        ));
    };
    if workspace_root.is_none() || permissions.write == AgentWritePermission::Denied {
        return CommandDispatch::Reject(AgentToolResult {
            exact_archive_file: None,
            call_id: call.id.clone(),
            tool: call.tool.clone(),
            ok: false,
            result: Some(json!({
                "type": "skill_script_policy",
                "code": "skill_script.workspace_write_denied",
                "recovery": "changePermissions",
            })),
            error: Some(
                "Skill scripts require a selected workspace with write permission.".to_string(),
            ),
        });
    }
    if script.preflight.status != AgentSkillScriptPreflightStatus::Ready
        || script.preflight.runtime_fingerprint.trim().is_empty()
    {
        return CommandDispatch::Reject(AgentToolResult {
            exact_archive_file: None,
            call_id: call.id.clone(),
            tool: call.tool.clone(),
            ok: false,
            result: serde_json::to_value(&script.preflight).ok(),
            error: Some("Skill script dependency preflight is not ready.".to_string()),
        });
    }
    if permissions.command_safety != AgentCommandSafetyPolicy::FullAccess
        || permissions.read != AgentReadPermission::All
        || permissions.write != AgentWritePermission::All
    {
        return CommandDispatch::Reject(AgentToolResult {
            exact_archive_file: None,
            call_id: call.id.clone(),
            tool: call.tool.clone(),
            ok: false,
            result: Some(json!({
                "type": "skill_script_policy",
                "code": "skill_script.full_access_required",
                "recovery": "changePermissions",
            })),
            error: Some(
                "Skill script execution requires unrestricted read/write scope and Full Access until OS-level process isolation is available."
                    .to_string(),
            ),
        });
    }
    CommandDispatch::RequireApproval(action)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{
        AgentApprovalStatus, AgentCommandPermission, AgentPatchPermission, AgentReadPermission,
        AgentSkillScriptInterpreter, AgentSkillScriptPreflightReport, AgentSkillScriptRequest,
        AgentSkillScriptRequirements, AgentWritePermission,
    };
    use serde_json::json;

    fn call() -> AgentToolCall {
        AgentToolCall {
            id: "script-1".to_string(),
            tool: "skills_run_script".to_string(),
            args: json!({}),
            approval_status: AgentApprovalStatus::Required,
            reason: None,
        }
    }

    fn action() -> AgentProposedAction {
        AgentProposedAction::SkillScript {
            script: Box::new(AgentSkillScriptRequest {
                id: "script-1".to_string(),
                script_uri: "skill://package/installed%3Auser%3Afixture/revision/scripts/run.py"
                    .to_string(),
                skill_id: "installed:user:fixture".to_string(),
                skill_revision: "revision".to_string(),
                resource_path: "scripts/run.py".to_string(),
                resource_digest: "digest".to_string(),
                interpreter: AgentSkillScriptInterpreter::Python3,
                args: Vec::new(),
                requirements: AgentSkillScriptRequirements::default(),
                preflight: AgentSkillScriptPreflightReport {
                    status: AgentSkillScriptPreflightStatus::Ready,
                    interpreter: AgentSkillScriptInterpreter::Python3,
                    interpreter_version: Some("Python 3".to_string()),
                    dependencies: Vec::new(),
                    runtime_fingerprint: "fingerprint".to_string(),
                    error_code: None,
                    message: None,
                },
                timeout_ms: Some(1_000),
                approval_status: AgentApprovalStatus::Required,
                reason: None,
            }),
        }
    }

    fn permissions(safety: AgentCommandSafetyPolicy) -> AgentPermissions {
        AgentPermissions {
            read: if safety == AgentCommandSafetyPolicy::FullAccess {
                AgentReadPermission::All
            } else {
                AgentReadPermission::WorkspaceOnly
            },
            write: if safety == AgentCommandSafetyPolicy::FullAccess {
                AgentWritePermission::All
            } else {
                AgentWritePermission::WorkspaceOnly
            },
            command: AgentCommandPermission::AutoApprove,
            command_safety: safety,
            patch: AgentPatchPermission::RequireApproval,
        }
    }

    #[test]
    fn guarded_scripts_are_rejected_until_process_isolation_exists() {
        assert!(matches!(
            prepare_skill_script_dispatch(
                &call(),
                action(),
                permissions(AgentCommandSafetyPolicy::Guarded),
                Some(Path::new("/workspace")),
                true,
            ),
            CommandDispatch::Reject(_)
        ));
    }

    #[test]
    fn full_access_scripts_require_explicit_approval_and_write_denied_fails_closed() {
        assert!(matches!(
            prepare_skill_script_dispatch(
                &call(),
                action(),
                permissions(AgentCommandSafetyPolicy::FullAccess),
                Some(Path::new("/workspace")),
                true,
            ),
            CommandDispatch::RequireApproval(_)
        ));
        let mut denied = permissions(AgentCommandSafetyPolicy::FullAccess);
        denied.write = AgentWritePermission::Denied;
        assert!(matches!(
            prepare_skill_script_dispatch(
                &call(),
                action(),
                denied,
                Some(Path::new("/workspace")),
                true,
            ),
            CommandDispatch::Reject(_)
        ));

        assert!(matches!(
            prepare_skill_script_dispatch(
                &call(),
                action(),
                permissions(AgentCommandSafetyPolicy::FullAccess),
                Some(Path::new("/workspace")),
                false,
            ),
            CommandDispatch::RequireApproval(_)
        ));
    }
}
