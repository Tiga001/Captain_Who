use super::{
    complete_document_text_result, read_zip_xml_text_parts, resolve_document_path, AgentTool,
    NamedText, ToolExecutionContext,
};
use crate::protocol::{
    AgentError, AgentResult, AgentToolDefinition, AgentToolResult, AgentToolSafety,
};
use serde::Deserialize;
use serde_json::{json, Value};

pub(super) struct ReadPresentationTool;

impl AgentTool for ReadPresentationTool {
    fn exposure(&self) -> super::AgentToolExposure {
        super::AgentToolExposure::RequiresCapability(super::ToolCapabilityId::application_owned(
            super::OFFICE_PRESENTATIONS_CAPABILITY,
        ))
    }

    fn permission_policy(&self) -> super::AgentToolPermissionPolicy {
        super::AgentToolPermissionPolicy::Default
    }

    fn definition(&self) -> AgentToolDefinition {
        AgentToolDefinition {
            name: "read_presentation".to_string(),
            description:
                "Extract text from .pptx presentation files in the selected workspace or an @attachments path. Legacy .ppt files are not supported."
                    .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "minLength": 1, "description": "Workspace-relative .pptx path or @attachments/... readPath." }
                },
                "required": ["path"],
                "additionalProperties": false
            }),
            safety: AgentToolSafety::ReadOnly,
            requires_workspace: false,
            requires_approval: false,
            approval_mode: crate::protocol::AgentToolApprovalMode::Never,
        }
    }

    fn execute(&self, context: &ToolExecutionContext, args: Value) -> AgentResult<Value> {
        context.check_cancelled()?;
        let args: ReadPresentationArgs = serde_json::from_value(args)
            .map_err(|error| AgentError::new(format!("read_presentation 参数无效：{error}")))?;
        let path = args.path()?;
        let resolved = resolve_document_path(context, path, &["pptx"])?;
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
            _ => unreachable!("extension validated before dispatch"),
        };
        cancellation_token.check()?;

        Ok(complete_document_text_result(
            json!({
                "path": resolved.relative_path,
                "format": resolved.extension,
                "sizeBytes": resolved.size_bytes,
                "extractor": extractor,
                "slideCount": slides
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
                "slideCount",
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
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReadPresentationArgs {
    path: String,
}

impl ReadPresentationArgs {
    fn path(&self) -> AgentResult<&str> {
        let path = self.path.trim();
        if path.is_empty() {
            Err(AgentError::new("read_presentation.path 不能为空。"))
        } else {
            Ok(path)
        }
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
    use super::{AgentTool, ReadPresentationArgs, ReadPresentationTool};
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
        let registry = ToolRegistry::defaults_with_search(None);
        let call = AgentToolCall {
            id: "call-1".to_string(),
            tool: "read_presentation".to_string(),
            args: json!({ "path": "deck.pptx" }),
            approval_status: AgentApprovalStatus::NotRequired,
            reason: None,
        };

        let result = registry.execute(&context, &call);

        assert!(result.ok, "{:?}", result.error);
        let value = result.result.unwrap();
        assert!(value["text"].as_str().unwrap().contains("Slide text"));
        assert_eq!(value["truncatedAtSource"], false);
        assert_eq!(value["omittedBytes"], 0);
    }

    #[test]
    fn schema_and_wire_accept_only_current_path() {
        let definition = ReadPresentationTool.definition();
        assert_eq!(definition.input_schema["required"], json!(["path"]));
        assert_eq!(definition.input_schema["additionalProperties"], false);
        assert_eq!(
            definition.input_schema["properties"]
                .as_object()
                .unwrap()
                .keys()
                .collect::<Vec<_>>(),
            vec!["path"]
        );
        for value in [
            json!({}),
            json!({ "path": null }),
            json!({ "filePath": "legacy.pptx" }),
            json!({ "path": "deck.pptx", "maxChars": 1 }),
        ] {
            assert!(serde_json::from_value::<ReadPresentationArgs>(value).is_err());
        }
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
                collaboration_identity: None,
                conversation_id: None,
                project_id: None,
                workspace: Some(AgentWorkspaceContext {
                    folders: Vec::new(),
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
