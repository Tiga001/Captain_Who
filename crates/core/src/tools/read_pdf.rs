use super::{
    complete_document_text_result, resolve_document_path, AgentTool, ToolExecutionContext,
};
use crate::protocol::{
    AgentError, AgentResult, AgentToolDefinition, AgentToolResult, AgentToolSafety,
};
use serde::Deserialize;
use serde_json::{json, Value};

pub(super) struct ReadPdfTool;

impl AgentTool for ReadPdfTool {
    fn exposure(&self) -> super::AgentToolExposure {
        super::AgentToolExposure::Stable
    }

    fn permission_policy(&self) -> super::AgentToolPermissionPolicy {
        super::AgentToolPermissionPolicy::Default
    }

    fn definition(&self) -> AgentToolDefinition {
        AgentToolDefinition {
            name: "read_pdf".to_string(),
            description: "Extract text from an authorized PDF file. Paths may be workspace-relative, absolute, use a supported system alias, or reference @attachments; the current read permission is enforced at execution time.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Workspace-relative .pdf path, absolute local path, @home/@desktop/@documents/@downloads, or an exact @attachments/... readPath. Availability depends on the current read permission." },
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
        let args: ReadPdfArgs = serde_json::from_value(args)
            .map_err(|error| AgentError::new(format!("read_pdf 参数无效：{error}")))?;
        let _requested_max_chars = args.max_chars;
        let path = args.path()?;
        let resolved = resolve_document_path(context, path, &["pdf"])?;
        context.check_cancelled()?;
        let pages = pdf_extract::extract_text_by_pages(&resolved.file_path)
            .map_err(|error| AgentError::new(format!("提取 PDF 文本失败：{error}")))?;
        context.check_cancelled()?;
        let text = pages
            .iter()
            .enumerate()
            .map(|(index, page)| format!("## Page {}\n{}", index + 1, page.trim()))
            .collect::<Vec<_>>()
            .join("\n\n");

        Ok(complete_document_text_result(
            json!({
                "path": resolved.relative_path,
                "format": "pdf",
                "sizeBytes": resolved.size_bytes,
                "pageCount": pages.len()
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
                "pageCount",
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
struct ReadPdfArgs {
    path: Option<String>,
    file_path: Option<String>,
    max_chars: Option<usize>,
}

impl ReadPdfArgs {
    fn path(&self) -> AgentResult<&str> {
        self.path
            .as_deref()
            .or(self.file_path.as_deref())
            .map(str::trim)
            .filter(|path| !path.is_empty())
            .ok_or_else(|| AgentError::new("read_pdf.path 不能为空。"))
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
    fn read_pdf_keeps_complete_text_despite_legacy_max_chars_input() {
        let fixture = TestWorkspace::new();
        fixture.write_pdf("sample.pdf", "Hello from PDF");
        let registry = ToolRegistry::defaults_with_search(None);
        let result = registry.execute(
            &fixture.context(),
            &AgentToolCall {
                id: "call-pdf".to_string(),
                tool: "read_pdf".to_string(),
                args: json!({ "path": "sample.pdf", "maxChars": 1 }),
                approval_status: AgentApprovalStatus::NotRequired,
                reason: None,
            },
        );

        assert!(result.ok, "{:?}", result.error);
        let value = result.result.unwrap();
        assert!(value["text"].as_str().unwrap().contains("Hello from PDF"));
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
                std::env::temp_dir().join(format!("my-copilot-agent-test-read-pdf-{unique}"));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(&root).unwrap();
            Self { root }
        }

        fn write_pdf(&self, path: &str, text: &str) {
            let content = format!("BT /F1 12 Tf 72 720 Td ({text}) Tj ET");
            let objects = [
                "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
                "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>".to_string(),
                "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
                format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
            ];
            let mut pdf = b"%PDF-1.4\n".to_vec();
            let mut offsets = vec![0_usize];
            for (index, object) in objects.iter().enumerate() {
                offsets.push(pdf.len());
                pdf.extend_from_slice(
                    format!("{} 0 obj\n{object}\nendobj\n", index + 1).as_bytes(),
                );
            }
            let xref = pdf.len();
            pdf.extend_from_slice(format!("xref\n0 {}\n", objects.len() + 1).as_bytes());
            pdf.extend_from_slice(b"0000000000 65535 f \n");
            for offset in offsets.iter().skip(1) {
                pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
            }
            pdf.extend_from_slice(
                format!(
                    "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                    objects.len() + 1
                )
                .as_bytes(),
            );
            fs::write(self.root.join(path), pdf).unwrap();
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
