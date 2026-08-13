use super::{
    complete_document_text_result, extract_with_textutil, join_named_text, read_zip_xml_text_parts,
    resolve_document_path, AgentTool, ToolExecutionContext,
};
use crate::protocol::{
    AgentError, AgentResult, AgentToolDefinition, AgentToolResult, AgentToolSafety,
};
use serde::Deserialize;
use serde_json::{json, Value};

pub(super) struct ReadWordTool;

impl AgentTool for ReadWordTool {
    fn exposure(&self) -> super::AgentToolExposure {
        super::AgentToolExposure::RequiresCapability(super::ToolCapabilityId::application_owned(
            super::OFFICE_DOCUMENTS_CAPABILITY,
        ))
    }

    fn permission_policy(&self) -> super::AgentToolPermissionPolicy {
        super::AgentToolPermissionPolicy::Default
    }

    fn definition(&self) -> AgentToolDefinition {
        AgentToolDefinition {
            name: "read_word".to_string(),
            description:
                "Extract text from Word documents in the selected workspace or an @attachments path. .docx works cross-platform; legacy .doc requires macOS textutil."
                    .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Workspace-relative .docx path, legacy .doc path on macOS, or @attachments/... readPath." },
                    "filePath": { "type": "string", "description": "Alias for path." },
                    "maxChars": { "type": "integer", "minimum": 1, "description": "Deprecated soft compatibility hint. Exact History capture is never limited by this value; model output uses the shared 10K gate." }
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
        let args: ReadWordArgs = serde_json::from_value(args)
            .map_err(|error| AgentError::new(format!("read_word 参数无效：{error}")))?;
        let _requested_max_chars = args.max_chars;
        let path = args.path()?;
        let resolved = resolve_document_path(context, path, &["docx", "doc"])?;
        let cancellation_token = context.cancellation_token();
        let (text, part_count, extractor) = match resolved.extension.as_str() {
            "docx" => {
                let parts =
                    read_zip_xml_text_parts(&resolved.file_path, &cancellation_token, |name| {
                        name == "word/document.xml"
                            || name.starts_with("word/header")
                            || name.starts_with("word/footer")
                            || name.starts_with("word/footnotes")
                            || name.starts_with("word/endnotes")
                            || name.starts_with("word/comments")
                    })?;
                let text = join_named_text(&parts);
                (text, parts.len(), "ooxml")
            }
            "doc" => (
                extract_with_textutil(&resolved.file_path, &cancellation_token)?,
                1,
                "textutil",
            ),
            _ => unreachable!("extension validated before dispatch"),
        };
        cancellation_token.check()?;

        Ok(complete_document_text_result(
            json!({
                "path": resolved.relative_path,
                "format": resolved.extension,
                "sizeBytes": resolved.size_bytes,
                "extractor": extractor,
                "partCount": part_count
            }),
            text,
        ))
    }

    fn model_projection(&self, result: &AgentToolResult) -> AgentToolResult {
        let projected = super::model_projection::retain_fields(
            result.result.as_ref(),
            &[
                "path",
                "format",
                "partCount",
                "originalBytes",
                "capturedBytes",
                "omittedBytes",
                "sourceStopReason",
                "truncatedAtSource",
                "truncated",
                "text",
            ],
        );
        super::model_projection::compact_model_result(result, projected)
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReadWordArgs {
    path: Option<String>,
    file_path: Option<String>,
    max_chars: Option<usize>,
}

impl ReadWordArgs {
    fn path(&self) -> AgentResult<&str> {
        self.path
            .as_deref()
            .or(self.file_path.as_deref())
            .map(str::trim)
            .filter(|path| !path.is_empty())
            .ok_or_else(|| AgentError::new("read_word.path 不能为空。"))
    }
}

#[cfg(test)]
mod tests {
    use super::super::{ToolExecutionContext, ToolRegistry};
    use crate::protocol::{
        AgentApprovalStatus, AgentRunContext, AgentToolCall, AgentWorkspaceContext,
    };
    use serde_json::json;
    use std::fs::{self, File};
    use std::io::Write;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use zip::write::SimpleFileOptions;

    static TEST_WORKSPACE_COUNTER: AtomicU64 = AtomicU64::new(1);

    #[test]
    fn read_word_extracts_docx_text() {
        let fixture = TestWorkspace::new();
        fixture.write_docx("sample.docx", "Hello from docx");
        let context = fixture.context();
        let registry = ToolRegistry::defaults_with_search(None);
        let call = AgentToolCall {
            id: "call-1".to_string(),
            tool: "read_word".to_string(),
            args: json!({ "path": "sample.docx", "maxChars": 1 }),
            approval_status: AgentApprovalStatus::NotRequired,
            reason: None,
        };

        let result = registry.execute(&context, &call);

        assert!(result.ok, "{:?}", result.error);
        for projection in [
            registry.archive_projection(&result),
            registry.event_projection(&result),
            registry.trace_projection(&result),
            registry.checkpoint_projection(&result),
            registry.model_projection(&result),
        ] {
            assert!(
                projection.result.as_ref().unwrap()["text"]
                    .as_str()
                    .unwrap()
                    .contains("Hello from docx"),
                "every consumer projection must retain the document body at its own boundary"
            );
        }
        let value = result.result.unwrap();
        assert!(value["text"].as_str().unwrap().contains("Hello from docx"));
        assert_eq!(value["truncatedAtSource"], false);
        assert_eq!(value["omittedBytes"], 0);
    }

    struct TestWorkspace {
        root: PathBuf,
    }

    impl TestWorkspace {
        fn new() -> Self {
            let unique = TEST_WORKSPACE_COUNTER.fetch_add(1, Ordering::Relaxed);
            let root =
                std::env::temp_dir().join(format!("my-copilot-agent-test-read-word-{unique}"));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(&root).unwrap();
            Self { root }
        }

        fn write_docx(&self, path: &str, text: &str) {
            let file_path = self.root.join(path);
            let file = File::create(file_path).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            let options = SimpleFileOptions::default();
            zip.start_file("word/document.xml", options).unwrap();
            write!(
                zip,
                r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>{text}</w:t></w:r></w:p></w:body></w:document>"#
            )
            .unwrap();
            zip.finish().unwrap();
        }

        fn context(&self) -> ToolExecutionContext {
            ToolExecutionContext::from_run_context(Some(&AgentRunContext {
                collaboration_identity: None,
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
