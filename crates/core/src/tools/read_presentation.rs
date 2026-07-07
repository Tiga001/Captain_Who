// Rust agent core.
use super::{
    extract_with_textutil, read_zip_xml_text_parts, resolve_document_path,
    sanitize_document_max_chars, truncate_chars, AgentTool, NamedText, ToolExecutionContext,
    MAX_DOCUMENT_TEXT_CHARS,
};
use crate::protocol::{AgentError, AgentResult, AgentToolDefinition, AgentToolSafety};
use serde::Deserialize;
use serde_json::{json, Value};

pub(super) struct ReadPresentationTool;

impl AgentTool for ReadPresentationTool {
    fn definition(&self) -> AgentToolDefinition {
        AgentToolDefinition {
            name: "read_presentation".to_string(),
            description:
                "Extract text from presentation files (.pptx, .ppt) in the selected workspace or an @attachments path."
                    .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Workspace-relative .pptx/.ppt path or @attachments/... readPath." },
                    "filePath": { "type": "string", "description": "Alias for path." },
                    "maxChars": { "type": "integer", "minimum": 1, "maximum": MAX_DOCUMENT_TEXT_CHARS }
                },
                "required": ["path"]
            }),
            safety: AgentToolSafety::ReadOnly,
            requires_workspace: false,
            requires_approval: false,
        }
    }

    fn execute(&self, context: &ToolExecutionContext, args: Value) -> AgentResult<Value> {
        context.check_cancelled()?;
        let args: ReadPresentationArgs = serde_json::from_value(args)
            .map_err(|error| AgentError::new(format!("read_presentation 参数无效：{error}")))?;
        let path = args.path()?;
        let max_chars = sanitize_document_max_chars(args.max_chars);
        let resolved = resolve_document_path(context, path, &["pptx", "ppt"])?;
        let cancellation_token = context.cancellation_token();
        let (text, slides, extractor) = match resolved.extension.as_str() {
            "pptx" => {
                let mut slides =
                    read_zip_xml_text_parts(&resolved.file_path, &cancellation_token, |name| {
                        name.starts_with("ppt/slides/slide") && name.ends_with(".xml")
                    })?;
                cancellation_token.check()?;
                sort_slide_parts(&mut slides);
                let text = slides
                    .iter()
                    .enumerate()
                    .map(|(index, slide)| format!("## Slide {}\n{}", index + 1, slide.text))
                    .collect::<Vec<_>>()
                    .join("\n\n");
                (text, slides.len(), "ooxml")
            }
            "ppt" => (
                extract_with_textutil(&resolved.file_path, &cancellation_token)?,
                1,
                "textutil",
            ),
            _ => unreachable!("extension validated before dispatch"),
        };
        cancellation_token.check()?;
        let (text, truncated) = truncate_chars(&text, max_chars);

        Ok(json!({
            "path": resolved.relative_path,
            "format": resolved.extension,
            "sizeBytes": resolved.size_bytes,
            "extractor": extractor,
            "slideCount": slides,
            "truncated": truncated,
            "text": text
        }))
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReadPresentationArgs {
    path: Option<String>,
    file_path: Option<String>,
    max_chars: Option<usize>,
}

impl ReadPresentationArgs {
    fn path(&self) -> AgentResult<&str> {
        self.path
            .as_deref()
            .or(self.file_path.as_deref())
            .map(str::trim)
            .filter(|path| !path.is_empty())
            .ok_or_else(|| AgentError::new("read_presentation.path 不能为空。"))
    }
}

fn sort_slide_parts(slides: &mut [NamedText]) {
    slides.sort_by_key(|slide| {
        slide
            .name
            .trim_start_matches("ppt/slides/slide")
            .trim_end_matches(".xml")
            .parse::<usize>()
            .unwrap_or(usize::MAX)
    });
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
    fn read_presentation_extracts_pptx_slide_text() {
        let fixture = TestWorkspace::new();
        fixture.write_pptx("deck.pptx", "Slide text");
        let context = fixture.context();
        let registry = ToolRegistry::read_only_defaults_with_search(None);
        let call = AgentToolCall {
            id: "call-1".to_string(),
            tool: "read_presentation".to_string(),
            args: json!({ "path": "deck.pptx" }),
            approval_status: AgentApprovalStatus::NotRequired,
            reason: None,
        };

        let result = registry.execute(&context, &call);

        assert!(result.ok, "{:?}", result.error);
        assert!(result.result.unwrap()["text"]
            .as_str()
            .unwrap()
            .contains("Slide text"));
    }

    struct TestWorkspace {
        root: PathBuf,
    }

    impl TestWorkspace {
        fn new() -> Self {
            let unique = TEST_WORKSPACE_COUNTER.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir()
                .join(format!("my-copilot-agent-test-read-presentation-{unique}"));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(&root).unwrap();
            Self { root }
        }

        fn write_pptx(&self, path: &str, text: &str) {
            let file_path = self.root.join(path);
            let file = File::create(file_path).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            let options = SimpleFileOptions::default();
            zip.start_file("ppt/slides/slide1.xml", options).unwrap();
            write!(
                zip,
                r#"<p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"><p:cSld><p:spTree><a:t xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">{text}</a:t></p:spTree></p:cSld></p:sld>"#
            )
            .unwrap();
            zip.finish().unwrap();
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
