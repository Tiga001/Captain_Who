use super::{AgentTool, ToolExecutionContext};
use crate::command::{
    join_process_output_capture, spawn_process_output_capture, ProcessOutputCaptureBudget,
    ProcessOutputCaptureMetadata, ProcessOutputCapturePolicy,
};
use crate::protocol::{
    AgentError, AgentResult, AgentToolDefinition, AgentToolResult, AgentToolSafety,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::process::{Command, Stdio};

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

        let mut child = command
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| AgentError::new(format!("执行 git diff 失败：{error}")))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| AgentError::new("git diff 未提供 stdout 管道。"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| AgentError::new("git diff 未提供 stderr 管道。"))?;
        let capture_policy = ProcessOutputCapturePolicy::process_default();
        let capture_budget = ProcessOutputCaptureBudget::new(capture_policy.max_capture_bytes());
        let stdout_reader =
            spawn_process_output_capture(stdout, capture_budget.clone(), capture_policy);
        let stderr_reader = spawn_process_output_capture(stderr, capture_budget, capture_policy);
        let status = child
            .wait()
            .map_err(|error| AgentError::new(format!("等待 git diff 失败：{error}")))?;
        let stdout = join_process_output_capture(stdout_reader, "git diff stdout")
            .map_err(AgentError::new)?;
        let stderr = join_process_output_capture(stderr_reader, "git diff stderr")
            .map_err(AgentError::new)?;
        context.check_cancelled()?;

        if !status.success() {
            return Err(AgentError::new(format!(
                "git diff 返回失败：{}",
                stderr.preview().trim()
            )));
        }

        let patch = stdout.read_captured_text().map_err(|error| {
            AgentError::new(format!("读取已捕获的 git diff stdout 失败：{error}"))
        })?;
        let stderr_text = stderr.read_captured_text().map_err(|error| {
            AgentError::new(format!("读取已捕获的 git diff stderr 失败：{error}"))
        })?;
        let capture = ProcessOutputCaptureMetadata::from_streams(&stdout, &stderr);

        Ok(json!({
            "path": args.path,
            "patch": patch,
            "stderr": stderr_text,
            "originalBytes": capture.original_bytes,
            "capturedBytes": capture.captured_bytes,
            "omittedBytes": capture.omitted_bytes,
            "truncatedAtSource": capture.truncated_at_source,
            "stopReason": capture.stop_reason,
            "stdoutOriginalBytes": capture.stdout_original_bytes,
            "stdoutCapturedBytes": capture.stdout_captured_bytes,
            "stdoutOmittedBytes": capture.stdout_omitted_bytes,
            "stdoutPreviewTruncated": capture.stdout_preview_truncated,
            "stdoutStopReason": capture.stdout_stop_reason,
            "stderrOriginalBytes": capture.stderr_original_bytes,
            "stderrCapturedBytes": capture.stderr_captured_bytes,
            "stderrOmittedBytes": capture.stderr_omitted_bytes,
            "stderrPreviewTruncated": capture.stderr_preview_truncated,
            "stderrStopReason": capture.stderr_stop_reason,
            "truncated": capture.truncated_at_source
        }))
    }

    fn model_projection(&self, result: &AgentToolResult) -> AgentToolResult {
        let projected = super::model_projection::retain_fields(
            result.result.as_ref(),
            &[
                "path",
                "patch",
                "originalBytes",
                "capturedBytes",
                "omittedBytes",
                "truncatedAtSource",
                "stdoutOriginalBytes",
                "stdoutCapturedBytes",
                "stdoutOmittedBytes",
                "stderrOriginalBytes",
                "stderrCapturedBytes",
                "stderrOmittedBytes",
                "truncated",
                "stopReason",
            ],
        );
        super::model_projection::compact_model_result(result, projected)
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GitDiffArgs {
    path: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::super::{ToolExecutionContext, ToolRegistry};
    use crate::protocol::{
        AgentApprovalStatus, AgentRunContext, AgentToolCall, AgentWorkspaceContext,
    };
    use serde_json::json;
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_WORKSPACE_COUNTER: AtomicU64 = AtomicU64::new(1);

    #[test]
    fn git_diff_captures_a_long_single_line_beyond_the_legacy_limit() {
        let fixture = TestWorkspace::new();
        fixture.write("large.txt", &format!("base-{}\n", "a".repeat(220_000)));
        fixture.git(&["add", "large.txt"]);
        fixture.git(&[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-m",
            "base",
        ]);
        let marker = "TAIL_MARKER_EXACT_HISTORY";
        fixture.write(
            "large.txt",
            &format!("updated-{}-{marker}\n", "b".repeat(220_000)),
        );

        let registry = ToolRegistry::defaults_with_search(None);
        let raw = registry.execute(
            &fixture.context(),
            &AgentToolCall {
                id: "call-git-diff".to_string(),
                tool: "git_diff".to_string(),
                args: json!({ "path": "large.txt" }),
                approval_status: AgentApprovalStatus::NotRequired,
                reason: None,
            },
        );

        assert!(raw.ok, "{:?}", raw.error);
        let value = raw.result.as_ref().unwrap();
        assert!(value["originalBytes"].as_u64().unwrap() > 200 * 1024);
        assert_eq!(value["omittedBytes"], 0);
        assert_eq!(value["truncatedAtSource"], false);
        assert!(value["patch"].as_str().unwrap().contains(marker));

        let archived = registry.archive_projection(&raw);
        assert_eq!(
            archived.result.as_ref().unwrap()["patch"],
            value["patch"],
            "Exact Archive projection must keep the complete captured diff"
        );
        for projection in [
            registry.event_projection(&raw),
            registry.trace_projection(&raw),
            registry.checkpoint_projection(&raw),
        ] {
            assert_eq!(
                projection.result.as_ref().unwrap()["patch"],
                value["patch"],
                "consumer projection must preserve the safely captured diff before its shared downstream limit"
            );
        }
        let model = registry.model_projection(&raw);
        assert!(model.result.as_ref().unwrap()["patch"]
            .as_str()
            .unwrap()
            .contains(marker));
        assert!(
            model.result.as_ref().unwrap()["stderr"].is_null(),
            "success-only stderr audit text must not enter the semantic model projection"
        );
    }

    struct TestWorkspace {
        root: PathBuf,
    }

    impl TestWorkspace {
        fn new() -> Self {
            let unique = TEST_WORKSPACE_COUNTER.fetch_add(1, Ordering::Relaxed);
            let root =
                std::env::temp_dir().join(format!("my-copilot-agent-test-git-diff-{unique}"));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(&root).unwrap();
            let status = Command::new("git")
                .arg("init")
                .arg("-q")
                .arg(&root)
                .status()
                .unwrap();
            assert!(status.success());
            Self { root }
        }

        fn write(&self, path: &str, content: &str) {
            fs::write(self.root.join(path), content).unwrap();
        }

        fn git(&self, args: &[&str]) {
            let status = Command::new("git")
                .arg("-C")
                .arg(&self.root)
                .args(args)
                .status()
                .unwrap();
            assert!(status.success(), "git {args:?} failed");
        }

        fn context(&self) -> ToolExecutionContext {
            ToolExecutionContext::from_run_context(Some(&AgentRunContext {
                conversation_id: None,
                project_id: None,
                workspace: Some(AgentWorkspaceContext {
                    project_id: None,
                    display_name: Some("test".to_string()),
                    root_path: Some(self.root.to_string_lossy().to_string()),
                }),
                attachment_library: None,
                permissions: Default::default(),
            }))
        }
    }

    impl Drop for TestWorkspace {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}
