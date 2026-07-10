// Rust agent core.
use super::{clean_relative_path, AgentTool, ToolExecutionContext};
use crate::protocol::{
    AgentApprovalStatus, AgentCommandRequest, AgentCommandRiskLevel, AgentError,
    AgentProposedAction, AgentResult, AgentToolCall, AgentToolDefinition, AgentToolSafety,
};
use crate::system_paths::expand_system_path;
use serde::Deserialize;
use serde_json::{json, Value};

const DEFAULT_TIMEOUT_MS: u64 = 120_000;
const MAX_TIMEOUT_MS: u64 = 600_000;
const MAX_COMMAND_CHARS: usize = 2_000;

pub(super) struct RunCommandTool;

impl AgentTool for RunCommandTool {
    fn definition(&self) -> AgentToolDefinition {
        AgentToolDefinition {
            name: "run_command".to_string(),
            description: "Run one single-line shell command through the host for builds, tests, queries, or program execution. Never use this tool to create, update, or delete files; use apply_patch instead. Approval behavior follows the current command permission. The command must not contain literal newlines or null characters.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "A single-line build, test, query, or program-execution command without literal newline or null characters. Do not use shell redirection, printf, echo, cat, tee, sed -i, or scripts to write file content." },
                    "cwd": { "type": "string", "description": "Working directory. May be workspace-relative, absolute, or @home/@desktop/@documents/@downloads when permissions allow. Required when no workspace exists." },
                    "timeoutMs": { "type": "integer", "minimum": 1, "maximum": MAX_TIMEOUT_MS },
                    "reason": { "type": "string", "description": "Why this command is needed and what result is expected." }
                },
                "required": ["command"]
            }),
            safety: AgentToolSafety::RequiresApproval,
            requires_workspace: false,
            requires_approval: true,
            approval_mode: crate::protocol::AgentToolApprovalMode::Always,
        }
    }

    fn execute(&self, _context: &ToolExecutionContext, _args: Value) -> AgentResult<Value> {
        Err(AgentError::new(
            "run_command 需要用户审批，不能由 agent runtime 自动执行。",
        ))
    }

    fn proposed_action(
        &self,
        context: &ToolExecutionContext,
        call: &AgentToolCall,
    ) -> AgentResult<AgentProposedAction> {
        Ok(AgentProposedAction::Command {
            command: command_request_from_call(context, call)?,
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RunCommandArgs {
    command: String,
    cwd: Option<String>,
    timeout_ms: Option<u64>,
    reason: Option<String>,
}

fn command_request_from_call(
    context: &ToolExecutionContext,
    call: &AgentToolCall,
) -> AgentResult<AgentCommandRequest> {
    let args: RunCommandArgs = serde_json::from_value(call.args.clone())
        .map_err(|error| AgentError::new(format!("run_command 参数无效：{error}")))?;
    let command = sanitize_command(&args.command)?;
    let cwd = sanitize_cwd(context, args.cwd)?;
    let reason = args
        .reason
        .or_else(|| call.reason.clone())
        .map(|reason| reason.trim().to_string())
        .filter(|reason| !reason.is_empty());

    Ok(AgentCommandRequest {
        id: call.id.clone(),
        command: command.clone(),
        cwd,
        timeout_ms: Some(
            args.timeout_ms
                .unwrap_or(DEFAULT_TIMEOUT_MS)
                .clamp(1, MAX_TIMEOUT_MS),
        ),
        approval_status: AgentApprovalStatus::Required,
        risk_level: Some(classify_command_risk(&command)),
        reason,
    })
}

fn sanitize_command(command: &str) -> AgentResult<String> {
    let command = command.trim();
    if command.is_empty() {
        return Err(AgentError::new("run_command.command 不能为空。"));
    }
    if command.chars().count() > MAX_COMMAND_CHARS {
        return Err(AgentError::new(format!(
            "run_command.command 过长，最多允许 {MAX_COMMAND_CHARS} 个字符。"
        )));
    }
    if command.contains('\0') || command.contains('\n') || command.contains('\r') {
        return Err(AgentError::new(
            "run_command.command 不能包含空字符或换行符。请改成单行命令；文件创建、编辑或删除必须使用 apply_patch。",
        ));
    }

    Ok(command.to_string())
}

fn sanitize_cwd(
    context: &ToolExecutionContext,
    cwd: Option<String>,
) -> AgentResult<Option<String>> {
    let workspace_exists = context.workspace_root_optional()?.is_some();
    let Some(cwd) = cwd else {
        return if workspace_exists {
            Ok(None)
        } else {
            Err(AgentError::new(
                "当前没有 workspace；run_command.cwd 必须指定绝对目录或系统路径别名。",
            ))
        };
    };
    let cwd = cwd.trim();
    if cwd.is_empty() || cwd == "." {
        return if workspace_exists {
            Ok(None)
        } else {
            Err(AgentError::new(
                "当前没有 workspace；run_command.cwd 不能省略或使用 `.`。",
            ))
        };
    }

    let write_permission = context.permissions().write;
    if let Some(expanded) = expand_system_path(cwd).map_err(AgentError::new)? {
        if write_permission != crate::protocol::AgentWritePermission::All {
            return Err(AgentError::new(
                "命令使用系统路径别名需要将写入范围设为“所有位置”。",
            ));
        }
        return Ok(Some(expanded.to_string_lossy().to_string()));
    }
    if std::path::Path::new(cwd).is_absolute() {
        if write_permission != crate::protocol::AgentWritePermission::All {
            return Err(AgentError::new(
                "命令在 workspace 外运行需要 write=all 权限。",
            ));
        }
        return Ok(Some(cwd.to_string()));
    }

    if !workspace_exists {
        return Err(AgentError::new(
            "没有 workspace 时，run_command.cwd 必须使用绝对目录或系统路径别名。",
        ));
    }

    Ok(Some(
        clean_relative_path(cwd)?
            .components()
            .filter_map(|component| match component {
                std::path::Component::Normal(part) => Some(part.to_string_lossy().to_string()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("/"),
    ))
}

fn classify_command_risk(command: &str) -> AgentCommandRiskLevel {
    let normalized = command.trim().to_ascii_lowercase();
    let program = normalized
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .trim_matches(|character| matches!(character, '"' | '\''));
    let has_shell_operators = [
        " > ", ">>", " 2>", " | ", " && ", " || ", ";", "`", "$(", "<(",
    ]
    .iter()
    .any(|operator| normalized.contains(operator));

    if has_destructive_pattern(&normalized, program) {
        return AgentCommandRiskLevel::Destructive;
    }
    if has_network_pattern(&normalized, program) {
        return AgentCommandRiskLevel::Network;
    }
    if has_write_pattern(&normalized, program) || has_shell_operators {
        return AgentCommandRiskLevel::WritesWorkspace;
    }
    if matches!(
        program,
        "pwd"
            | "ls"
            | "find"
            | "rg"
            | "grep"
            | "cat"
            | "head"
            | "tail"
            | "wc"
            | "git"
            | "cargo"
            | "npm"
            | "pnpm"
            | "yarn"
            | "python"
            | "python3"
            | "node"
    ) {
        return classify_known_program(&normalized, program);
    }

    AgentCommandRiskLevel::Unknown
}

fn classify_known_program(command: &str, program: &str) -> AgentCommandRiskLevel {
    match program {
        "git" => {
            if command.starts_with("git status")
                || command.starts_with("git diff")
                || command.starts_with("git log")
                || command.starts_with("git show")
                || command == "git branch"
                || command.starts_with("git branch --show-current")
                || command.starts_with("git branch --list")
                || command.starts_with("git branch -a")
            {
                AgentCommandRiskLevel::ReadOnly
            } else {
                AgentCommandRiskLevel::WritesWorkspace
            }
        }
        "cargo" => {
            if command.starts_with("cargo install") {
                AgentCommandRiskLevel::Network
            } else if command.starts_with("cargo test")
                || command.starts_with("cargo check")
                || command.starts_with("cargo build")
            {
                AgentCommandRiskLevel::WritesWorkspace
            } else {
                AgentCommandRiskLevel::Unknown
            }
        }
        "npm" | "pnpm" | "yarn" => {
            if command.contains(" install")
                || command.contains(" add")
                || command.contains(" remove")
                || command.contains(" upgrade")
                || command.contains(" update")
            {
                AgentCommandRiskLevel::Network
            } else if command.contains(" test")
                || command.contains(" build")
                || command.contains(" lint")
                || command.contains(" typecheck")
            {
                AgentCommandRiskLevel::WritesWorkspace
            } else {
                AgentCommandRiskLevel::Unknown
            }
        }
        "python" | "python3" | "node" => AgentCommandRiskLevel::Unknown,
        _ => AgentCommandRiskLevel::ReadOnly,
    }
}

fn has_destructive_pattern(command: &str, program: &str) -> bool {
    command.contains(" -delete")
        || matches!(
            program,
            "rm" | "mv"
                | "cp"
                | "chmod"
                | "chown"
                | "ln"
                | "truncate"
                | "dd"
                | "mkfs"
                | "kill"
                | "pkill"
                | "git"
        ) && (program != "git"
            || command.contains(" reset")
            || command.contains(" checkout")
            || command.contains(" clean")
            || command.contains(" merge")
            || command.contains(" rebase")
            || command.contains(" commit")
            || command.contains(" push"))
}

fn has_network_pattern(command: &str, program: &str) -> bool {
    matches!(
        program,
        "curl" | "wget" | "ssh" | "scp" | "rsync" | "brew" | "uv" | "pip" | "pip3"
    ) || command.contains(" npm install")
        || command.contains(" pnpm install")
        || command.contains(" yarn install")
        || command.contains(" cargo install")
}

fn has_write_pattern(command: &str, program: &str) -> bool {
    matches!(program, "touch" | "mkdir" | "tee" | "sed" | "perl" | "make")
        || command.contains(" --fix")
        || command.contains(" -w")
        || command.contains(" --write")
        || command.contains(" --watch")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{
        AgentApprovalStatus, AgentPermissions, AgentRunContext, AgentToolCall,
        AgentWorkspaceContext, AgentWritePermission,
    };
    use serde_json::json;

    #[test]
    fn builds_command_request_for_approval() {
        let call = AgentToolCall {
            id: "tool-1".to_string(),
            tool: "run_command".to_string(),
            args: json!({
                "command": " cargo test ",
                "cwd": "agent/rust",
                "timeoutMs": 999_999,
                "reason": "verify tests"
            }),
            approval_status: AgentApprovalStatus::Required,
            reason: None,
        };
        let context = ToolExecutionContext::from_run_context(Some(&AgentRunContext {
            conversation_id: None,
            project_id: None,
            workspace: Some(AgentWorkspaceContext {
                project_id: None,
                display_name: Some("temp".to_string()),
                root_path: Some(std::env::temp_dir().to_string_lossy().to_string()),
            }),
            attachment_library: None,
            permissions: AgentPermissions {
                write: AgentWritePermission::WorkspaceOnly,
                ..Default::default()
            },
        }));
        let request = command_request_from_call(&context, &call).unwrap();

        assert_eq!(request.id, "tool-1");
        assert_eq!(request.command, "cargo test");
        assert_eq!(request.cwd.as_deref(), Some("agent/rust"));
        assert_eq!(request.timeout_ms, Some(MAX_TIMEOUT_MS));
        assert_eq!(request.approval_status, AgentApprovalStatus::Required);
        assert_eq!(
            request.risk_level,
            Some(AgentCommandRiskLevel::WritesWorkspace)
        );
        assert_eq!(request.reason.as_deref(), Some("verify tests"));
    }

    #[test]
    fn rejects_unsafe_command_shape_before_approval() {
        let error = sanitize_command("echo one\necho two").unwrap_err();

        assert!(error.to_string().contains("换行符"));
    }

    #[test]
    fn rejects_cwd_outside_workspace() {
        let context = ToolExecutionContext::from_run_context(Some(&AgentRunContext {
            conversation_id: None,
            project_id: None,
            workspace: Some(AgentWorkspaceContext {
                project_id: None,
                display_name: Some("temp".to_string()),
                root_path: Some(std::env::temp_dir().to_string_lossy().to_string()),
            }),
            attachment_library: None,
            permissions: AgentPermissions {
                write: AgentWritePermission::WorkspaceOnly,
                ..Default::default()
            },
        }));
        let error = sanitize_cwd(&context, Some("../outside".to_string())).unwrap_err();

        assert!(error.to_string().contains("路径不能包含"));
    }

    #[test]
    fn no_workspace_requires_explicit_cwd_and_accepts_alias_with_full_write() {
        let context = ToolExecutionContext::from_run_context(Some(&AgentRunContext {
            conversation_id: None,
            project_id: None,
            workspace: None,
            attachment_library: None,
            permissions: AgentPermissions {
                write: AgentWritePermission::All,
                ..Default::default()
            },
        }));

        assert!(sanitize_cwd(&context, None).is_err());
        let cwd = sanitize_cwd(&context, Some("@home".to_string()))
            .unwrap()
            .unwrap();
        assert!(std::path::Path::new(&cwd).is_absolute());
    }

    #[test]
    fn classifies_common_risk_levels() {
        assert_eq!(
            classify_command_risk("git diff"),
            AgentCommandRiskLevel::ReadOnly
        );
        assert_eq!(
            classify_command_risk("cargo test"),
            AgentCommandRiskLevel::WritesWorkspace
        );
        assert_eq!(
            classify_command_risk("pnpm install"),
            AgentCommandRiskLevel::Network
        );
        assert_eq!(
            classify_command_risk("rm -rf target"),
            AgentCommandRiskLevel::Destructive
        );
        assert_eq!(
            classify_command_risk("git branch scratch"),
            AgentCommandRiskLevel::WritesWorkspace
        );
        assert_eq!(
            classify_command_risk("find . -delete"),
            AgentCommandRiskLevel::Destructive
        );
    }
}
