use super::{truncate_chars, AgentTool, ToolExecutionContext, MAX_GIT_DIFF_BYTES};
use crate::protocol::{AgentError, AgentResult, AgentToolDefinition, AgentToolSafety};
use serde::Deserialize;
use serde_json::{json, Value};
use std::process::Command;

pub(super) struct GitDiffTool;

impl AgentTool for GitDiffTool {
    fn exposure(&self) -> super::AgentToolExposure {
        super::AgentToolExposure::Stable
    }

    fn permission_policy(&self) -> super::AgentToolPermissionPolicy {
        super::AgentToolPermissionPolicy::Default
    }

    fn definition(&self) -> AgentToolDefinition {
        AgentToolDefinition {
            name: "git_diff".to_string(),
            description: "Return the current read-only git diff for the selected workspace, optionally scoped to one relative path.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Optional workspace-relative path." }
                }
            }),
            safety: AgentToolSafety::ReadOnly,
            requires_workspace: true,
            requires_approval: false,
            approval_mode: crate::protocol::AgentToolApprovalMode::Never,
        }
    }

    fn execute(&self, context: &ToolExecutionContext, args: Value) -> AgentResult<Value> {
        let args: GitDiffArgs = serde_json::from_value(args)
            .map_err(|error| AgentError::new(format!("git_diff 参数无效：{error}")))?;
        let root = context.workspace_root()?;
        let mut command = Command::new("git");
        command.arg("-C").arg(&root).arg("diff").arg("--");

        if let Some(path) = args.path.as_deref().filter(|path| !path.trim().is_empty()) {
            command.arg(context.validate_relative_path_for_git(path)?);
        }

        let output = command
            .output()
            .map_err(|error| AgentError::new(format!("执行 git diff 失败：{error}")))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(AgentError::new(format!(
                "git diff 返回失败：{}",
                stderr.trim()
            )));
        }

        let diff = String::from_utf8_lossy(&output.stdout);
        let (patch, truncated) = truncate_chars(&diff, MAX_GIT_DIFF_BYTES);

        Ok(json!({
            "path": args.path,
            "patch": patch,
            "truncated": truncated
        }))
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GitDiffArgs {
    path: Option<String>,
}
