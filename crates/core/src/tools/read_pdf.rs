use super::{
    resolve_document_path, sanitize_document_max_chars, truncate_chars, AgentTool,
    ToolExecutionContext, MAX_DOCUMENT_TEXT_CHARS,
};
use crate::protocol::{AgentError, AgentResult, AgentToolDefinition, AgentToolSafety};
use serde::Deserialize;
use serde_json::{json, Value};

pub(super) struct ReadPdfTool;

impl AgentTool for ReadPdfTool {
    fn definition(&self) -> AgentToolDefinition {
        AgentToolDefinition {
            name: "read_pdf".to_string(),
            description:
                "Extract text from a PDF file in the selected workspace or an @attachments path."
                    .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Workspace-relative .pdf path or @attachments/... readPath." },
                    "filePath": { "type": "string", "description": "Alias for path." },
                    "maxChars": { "type": "integer", "minimum": 1, "maximum": MAX_DOCUMENT_TEXT_CHARS }
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
        let path = args.path()?;
        let max_chars = sanitize_document_max_chars(args.max_chars);
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
        let (text, truncated) = truncate_chars(&text, max_chars);

        Ok(json!({
            "path": resolved.relative_path,
            "format": "pdf",
            "sizeBytes": resolved.size_bytes,
            "pageCount": pages.len(),
            "truncated": truncated,
            "text": text
        }))
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
