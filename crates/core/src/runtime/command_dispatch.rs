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
#[cfg(test)]
pub(super) fn prepare_command_dispatch(
    call: &AgentToolCall,
    action: AgentProposedAction,
    permissions: AgentPermissions,
    workspace_root: Option<&Path>,
    auto_approve: bool,
) -> CommandDispatch {
    prepare_command_dispatch_in_workspace(
        call,
        action,
        permissions,
        &crate::workspace::WorkspaceResolver::from_primary(workspace_root),
        auto_approve,
    )
}

pub(super) fn prepare_command_dispatch_in_workspace(
    call: &AgentToolCall,
    action: AgentProposedAction,
    permissions: AgentPermissions,
    workspace: &crate::workspace::WorkspaceResolver,
    auto_approve: bool,
) -> CommandDispatch {
    let AgentProposedAction::Command { command } = &action else {
        return CommandDispatch::Reject(failed_tool_call_result(
            call,
            AgentError::new("run_command 未生成结构化命令操作，已拒绝执行。"),
        ));
    };
    let requested_cwd = command.cwd.as_deref();
    let policy_cwd = if requested_cwd.is_some_and(|cwd| cwd.starts_with("@workspace")) {
        match crate::command::resolve_command_cwd_in_workspace(
            workspace,
            requested_cwd,
            permissions.write,
        ) {
            Ok(cwd) => Some(cwd),
            Err(error) => {
                return CommandDispatch::Reject(failed_tool_call_result(
                    call,
                    AgentError::new(error),
                ))
            }
        }
    } else {
        command_policy_cwd(workspace.primary_root(), requested_cwd)
    };
    let evaluation = crate::command::evaluate_command_policy_in_workspace(
        &command.command,
        permissions,
        CommandAuthorizationSource::Automatic,
        workspace,
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
/// be classified as a shell command. Every script requires complete unrestricted
/// scope. Only an exact application-bundled source proof may additionally use
/// the built-in execution preference to skip the human prompt.
pub(super) fn prepare_skill_script_dispatch(
    call: &AgentToolCall,
    action: AgentProposedAction,
    permissions: AgentPermissions,
    workspace_root: Option<&Path>,
    _command_auto_approve: bool,
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
    if permissions.builtin_execution == AgentBuiltinExecutionPermission::AutoApprove
        && is_exact_application_bundled_script(script)
    {
        CommandDispatch::ExecuteAutomatically(action)
    } else {
        CommandDispatch::RequireApproval(action)
    }
}

fn is_exact_application_bundled_script(script: &AgentSkillScriptRequest) -> bool {
    let expected_source = crate::skills::APPLICATION_BUNDLED_SKILL_SOURCE_ID;
    script.source.source_id == expected_source
        && script.source.source_kind == AgentSkillScriptSourceKind::Bundled
        && script.source.trust == AgentSkillScriptTrust::Application
        && crate::skills::SkillId::parse(&script.skill_id)
            .is_ok_and(|skill_id| skill_id.source_id().as_str() == expected_source)
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
        AgentSkillScriptRequirements, AgentSkillScriptSourceKind, AgentSkillScriptSourceProof,
        AgentSkillScriptTrust, AgentWritePermission,
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
                source: AgentSkillScriptSourceProof {
                    source_id: "installed:user".to_string(),
                    source_kind: AgentSkillScriptSourceKind::Installed,
                    trust: AgentSkillScriptTrust::Untrusted,
                },
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
            builtin_execution: AgentBuiltinExecutionPermission::AutoApprove,
        }
    }

    fn application_bundled_action() -> AgentProposedAction {
        let mut action = action();
        let AgentProposedAction::SkillScript { script } = &mut action else {
            unreachable!();
        };
        script.skill_id = "bundled:application:fixture".to_string();
        script.script_uri =
            "skill://package/bundled%3Aapplication%3Afixture/revision/scripts/run.py".to_string();
        script.source = AgentSkillScriptSourceProof {
            source_id: crate::skills::APPLICATION_BUNDLED_SKILL_SOURCE_ID.to_string(),
            source_kind: AgentSkillScriptSourceKind::Bundled,
            trust: AgentSkillScriptTrust::Application,
        };
        action
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

    #[test]
    fn exact_application_bundled_script_uses_builtin_permission_only() {
        assert!(matches!(
            prepare_skill_script_dispatch(
                &call(),
                application_bundled_action(),
                permissions(AgentCommandSafetyPolicy::FullAccess),
                Some(Path::new("/workspace")),
                false,
            ),
            CommandDispatch::ExecuteAutomatically(_)
        ));

        let mut manual = permissions(AgentCommandSafetyPolicy::FullAccess);
        manual.builtin_execution = AgentBuiltinExecutionPermission::RequireApproval;
        assert!(matches!(
            prepare_skill_script_dispatch(
                &call(),
                application_bundled_action(),
                manual,
                Some(Path::new("/workspace")),
                true,
            ),
            CommandDispatch::RequireApproval(_)
        ));
    }

    #[test]
    fn forged_application_trust_or_source_never_auto_executes() {
        for mutate in [0, 1, 2] {
            let mut action = application_bundled_action();
            let AgentProposedAction::SkillScript { script } = &mut action else {
                unreachable!();
            };
            match mutate {
                0 => script.source.source_id = "bundled:other".to_string(),
                1 => script.source.source_kind = AgentSkillScriptSourceKind::Installed,
                _ => script.source.trust = AgentSkillScriptTrust::Untrusted,
            }
            assert!(matches!(
                prepare_skill_script_dispatch(
                    &call(),
                    action,
                    permissions(AgentCommandSafetyPolicy::FullAccess),
                    Some(Path::new("/workspace")),
                    true,
                ),
                CommandDispatch::RequireApproval(_)
            ));
        }
    }
}
