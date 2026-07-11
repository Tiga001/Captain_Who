use super::{
    AgentTool, ToolExecutionContext, DEFAULT_READ_MAX_LINES, MAX_READ_FILE_BYTES, MAX_READ_LINES,
};
use crate::protocol::{AgentError, AgentResult, AgentToolDefinition, AgentToolSafety};
use crate::revision::content_revision;
use serde::Deserialize;
use serde_json::{json, Value};
use std::fs;

pub(super) struct ReadFileTool;

impl AgentTool for ReadFileTool {
    fn definition(&self) -> AgentToolDefinition {
        AgentToolDefinition {
            name: "read_file".to_string(),
            description:
                "Read a UTF-8 text file from the selected workspace or an @attachments path with optional line bounds."
                    .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Workspace-relative file path or @attachments/... readPath." },
                    "filePath": { "type": "string", "description": "Alias for path." },
                    "startLine": { "type": "integer", "minimum": 1 },
                    "maxLines": { "type": "integer", "minimum": 1, "maximum": MAX_READ_LINES }
                },
                "required": ["path"]
            }),
            safety: AgentToolSafety::ReadOnly,
            requires_workspace: false,
            requires_approval: false,
            approval_mode: crate::protocol::AgentToolApprovalMode::Never,
        }
    }

    fn execute(&self, context: &ToolExecutionContext, args: Value) -> AgentResult<Value> {
        context.check_cancelled()?;
        let args: ReadFileArgs = serde_json::from_value(args)
            .map_err(|error| AgentError::new(format!("read_file 参数无效：{error}")))?;
        let path = args.path()?;
        let file_path = context.resolve_existing_path(path)?;
        context.check_cancelled()?;
        let metadata = fs::metadata(&file_path)
            .map_err(|error| AgentError::new(format!("读取文件元数据失败：{error}")))?;

        if !metadata.is_file() {
            return Err(AgentError::new("read_file 只能读取文件。"));
        }

        if metadata.len() > MAX_READ_FILE_BYTES {
            return Err(AgentError::new(format!(
                "文件过大：{} bytes，超过 {} bytes 限制。",
                metadata.len(),
                MAX_READ_FILE_BYTES
            )));
        }

        let content = fs::read_to_string(&file_path)
            .map_err(|error| AgentError::new(format!("读取文件失败：{error}")))?;
        context.check_cancelled()?;
        let lines: Vec<&str> = content.lines().collect();
        let total_lines = lines.len();
        let start_line = args.start_line.unwrap_or(1).max(1);
        let max_lines = args
            .max_lines
            .unwrap_or(DEFAULT_READ_MAX_LINES)
            .clamp(1, MAX_READ_LINES);
        let start_index = start_line.saturating_sub(1).min(total_lines);
        let end_index = (start_index + max_lines).min(total_lines);
        let selected = lines[start_index..end_index].join("\n");

        Ok(json!({
            "path": context.display_path(path, &file_path)?,
            "revision": content_revision(content.as_bytes()),
            "startLine": start_index + 1,
            "endLine": end_index,
            "totalLines": total_lines,
            "truncated": end_index < total_lines,
            "content": selected
        }))
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReadFileArgs {
    path: Option<String>,
    file_path: Option<String>,
    start_line: Option<usize>,
    max_lines: Option<usize>,
}

impl ReadFileArgs {
    fn path(&self) -> AgentResult<&str> {
        self.path
            .as_deref()
            .or(self.file_path.as_deref())
            .map(str::trim)
            .filter(|path| !path.is_empty())
            .ok_or_else(|| AgentError::new("read_file.path 不能为空。"))
    }
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
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_WORKSPACE_COUNTER: AtomicU64 = AtomicU64::new(1);

    #[test]
    fn read_file_respects_line_bounds() {
        let fixture = TestWorkspace::new();
        fixture.write_file("notes.txt", "one\ntwo\nthree\n");
        let context = fixture.context();
        let registry = ToolRegistry::defaults_with_search(None);
        let call = AgentToolCall {
            id: "call-1".to_string(),
            tool: "read_file".to_string(),
            args: json!({ "path": "notes.txt", "startLine": 2, "maxLines": 1 }),
            approval_status: AgentApprovalStatus::NotRequired,
            reason: None,
        };

        let result = registry.execute(&context, &call);

        assert!(result.ok, "{:?}", result.error);
        let value = result.result.unwrap();
        assert_eq!(value["content"], "two");
        assert!(value["revision"].as_str().unwrap().starts_with("v1-"));
    }

    struct TestWorkspace {
        root: PathBuf,
    }

    impl TestWorkspace {
        fn new() -> Self {
            let unique = TEST_WORKSPACE_COUNTER.fetch_add(1, Ordering::Relaxed);
            let root =
                std::env::temp_dir().join(format!("my-copilot-agent-test-read-file-{unique}"));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(&root).unwrap();
            Self { root }
        }

        fn write_file(&self, path: &str, content: &str) {
            let file_path = self.root.join(path);
            fs::create_dir_all(file_path.parent().unwrap()).unwrap();
            fs::write(file_path, content).unwrap();
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
