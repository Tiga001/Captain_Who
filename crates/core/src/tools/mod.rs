// Rust agent core.
mod apply_patch;
mod apply_patch_diff;
mod apply_patch_paths;
mod attachments;
mod context;
mod document_text;
mod filesystem;
mod git_diff;
mod limits;
mod read_file;
mod read_image;
mod read_pdf;
mod read_presentation;
mod read_spreadsheet;
mod read_word;
mod run_command;
mod search_code;
mod search_files;
mod web_fetch;
mod web_search;
mod workspace_map;

use crate::protocol::{
    AgentError, AgentProposedAction, AgentResult, AgentSearchConfig, AgentSearchMode,
    AgentToolCall, AgentToolDefinition, AgentToolResult,
};
use apply_patch::ApplyPatchTool;
use attachments::{AttachmentsListProjectTool, AttachmentsListTool};
use git_diff::GitDiffTool;
use read_file::ReadFileTool;
use read_image::ReadImageTool;
use read_pdf::ReadPdfTool;
use read_presentation::ReadPresentationTool;
use read_spreadsheet::ReadSpreadsheetTool;
use read_word::ReadWordTool;
use run_command::RunCommandTool;
use search_code::SearchCodeTool;
use search_files::SearchFilesTool;
use serde_json::Value;
use std::collections::BTreeMap;
use web_fetch::WebFetchTool;
use web_search::WebSearchTool;
use workspace_map::WorkspaceMapTool;

pub(super) use context::ToolExecutionContext;
use document_text::{
    extract_with_textutil, join_named_text, normalize_text_output, read_zip_xml_text_parts,
    resolve_document_path, sanitize_document_max_chars, NamedText,
};
use filesystem::{
    block_on_tool_future, clean_relative_path, relative_display, sanitize_limit, truncate_chars,
    walk_workspace_with_cancellation, WalkEntry, WalkResult,
};
use limits::*;

pub struct ToolRegistry {
    tools: BTreeMap<String, Box<dyn AgentTool>>,
}

impl ToolRegistry {
    pub fn read_only_defaults_with_search(search_config: Option<&AgentSearchConfig>) -> Self {
        let mut registry = Self {
            tools: BTreeMap::new(),
        };
        registry.register(AttachmentsListTool);
        registry.register(AttachmentsListProjectTool);
        registry.register(ReadFileTool);
        registry.register(ReadImageTool);
        registry.register(ReadPdfTool);
        registry.register(ReadWordTool);
        registry.register(ReadPresentationTool);
        registry.register(ReadSpreadsheetTool);
        registry.register(WorkspaceMapTool);
        registry.register(SearchFilesTool);
        registry.register(SearchCodeTool);
        if let Some(api_key) = tavily_api_key(search_config) {
            registry.register(WebSearchTool::new(api_key.clone()));
            registry.register(WebFetchTool::new(api_key));
        }
        registry.register(GitDiffTool);
        registry.register(ApplyPatchTool);
        registry.register(RunCommandTool);
        registry
    }

    pub fn definitions(&self) -> Vec<AgentToolDefinition> {
        self.tools.values().map(|tool| tool.definition()).collect()
    }

    pub fn definition_for(&self, tool_name: &str) -> Option<AgentToolDefinition> {
        self.tools.get(tool_name).map(|tool| tool.definition())
    }

    pub fn proposed_action(
        &self,
        context: &ToolExecutionContext,
        call: &AgentToolCall,
    ) -> AgentResult<AgentProposedAction> {
        let Some(tool) = self.tools.get(&call.tool) else {
            return Err(AgentError::new(format!("未知工具：{}", call.tool)));
        };

        tool.proposed_action(context, call)
    }

    pub fn execute(&self, context: &ToolExecutionContext, call: &AgentToolCall) -> AgentToolResult {
        if let Err(error) = context.check_cancelled() {
            return AgentToolResult {
                call_id: call.id.clone(),
                tool: call.tool.clone(),
                ok: false,
                result: None,
                error: Some(error.to_string()),
            };
        }

        let Some(tool) = self.tools.get(&call.tool) else {
            return AgentToolResult {
                call_id: call.id.clone(),
                tool: call.tool.clone(),
                ok: false,
                result: None,
                error: Some(format!("未知工具：{}", call.tool)),
            };
        };

        match tool.execute(context, call.args.clone()) {
            Ok(result) => match context.check_cancelled() {
                Ok(()) => AgentToolResult {
                    call_id: call.id.clone(),
                    tool: call.tool.clone(),
                    ok: true,
                    result: Some(result),
                    error: None,
                },
                Err(error) => AgentToolResult {
                    call_id: call.id.clone(),
                    tool: call.tool.clone(),
                    ok: false,
                    result: None,
                    error: Some(error.to_string()),
                },
            },
            Err(error) => AgentToolResult {
                call_id: call.id.clone(),
                tool: call.tool.clone(),
                ok: false,
                result: None,
                error: Some(error.to_string()),
            },
        }
    }

    fn register<T: AgentTool + 'static>(&mut self, tool: T) {
        self.tools
            .insert(tool.definition().name, Box::new(tool) as Box<dyn AgentTool>);
    }
}

fn tavily_api_key(search_config: Option<&AgentSearchConfig>) -> Option<String> {
    let search_config = search_config?;
    if search_config.mode == AgentSearchMode::Disabled {
        return None;
    }

    let api_key = search_config
        .tavily_api_key
        .as_deref()
        .map(str::trim)
        .filter(|api_key| !api_key.is_empty())?;

    Some(api_key.to_string())
}

pub(super) trait AgentTool: Send + Sync {
    fn definition(&self) -> AgentToolDefinition;
    fn execute(&self, context: &ToolExecutionContext, args: Value) -> AgentResult<Value>;
    fn proposed_action(
        &self,
        _context: &ToolExecutionContext,
        call: &AgentToolCall,
    ) -> AgentResult<AgentProposedAction> {
        Ok(AgentProposedAction::ToolCall { call: call.clone() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{
        AgentAttachmentLibraryContext, AgentAttachmentReference, AgentInputAttachmentKind,
        AgentRunContext, AgentSearchConfig, AgentSearchMode, AgentToolCall, AgentWorkspaceContext,
    };
    use serde_json::json;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_WORKSPACE_COUNTER: AtomicU64 = AtomicU64::new(1);

    #[test]
    fn rejects_paths_outside_workspace() {
        let fixture = TestWorkspace::new();
        let context = fixture.context();
        let error = context.resolve_existing_path("../outside.txt").unwrap_err();

        assert!(error.to_string().contains("路径不能包含"));
    }

    #[test]
    fn registers_tavily_tools_when_configured() {
        let registry = ToolRegistry::read_only_defaults_with_search(Some(&AgentSearchConfig {
            mode: AgentSearchMode::Tavily,
            tavily_api_key: Some("tvly-test".to_string()),
        }));
        let tools = registry
            .definitions()
            .into_iter()
            .map(|definition| definition.name)
            .collect::<Vec<_>>();

        assert!(tools.contains(&"web_search".to_string()));
        assert!(tools.contains(&"web_fetch".to_string()));
    }

    #[test]
    fn registers_run_command_as_approval_tool() {
        let registry = ToolRegistry::read_only_defaults_with_search(None);
        let definition = registry.definition_for("run_command").unwrap();

        assert_eq!(definition.name, "run_command");
        assert!(!definition.requires_workspace);
        assert!(definition.requires_approval);
    }

    #[test]
    fn registers_apply_patch_as_approval_tool() {
        let registry = ToolRegistry::read_only_defaults_with_search(None);
        let definition = registry.definition_for("apply_patch").unwrap();

        assert_eq!(definition.name, "apply_patch");
        assert!(!definition.requires_workspace);
        assert!(definition.requires_approval);
    }

    #[test]
    fn lists_and_reads_attachment_paths() {
        let fixture = TestWorkspace::new();
        let attachment_root = fixture.root.join("attachments");
        let storage_rel_path = PathBuf::from("conversations/c1/m1/a1/notes.txt");
        let attachment_path = attachment_root.join(&storage_rel_path);
        fs::create_dir_all(attachment_path.parent().unwrap()).unwrap();
        fs::write(&attachment_path, "hello from attachment").unwrap();

        let context = ToolExecutionContext::from_run_context(Some(&AgentRunContext {
            conversation_id: Some("c1".to_string()),
            project_id: Some("p1".to_string()),
            workspace: None,
            attachment_library: Some(AgentAttachmentLibraryContext {
                root_path: Some(attachment_root.to_string_lossy().to_string()),
                conversation_id: Some("c1".to_string()),
                project_id: Some("p1".to_string()),
                conversation_attachments: vec![AgentAttachmentReference {
                    id: "a1".to_string(),
                    conversation_id: "c1".to_string(),
                    message_id: "m1".to_string(),
                    project_id: Some("p1".to_string()),
                    kind: AgentInputAttachmentKind::File,
                    name: "notes.txt".to_string(),
                    mime_type: Some("text/plain".to_string()),
                    size_bytes: 21,
                    read_path: "@attachments/a1/notes.txt".to_string(),
                    storage_rel_path: "conversations/c1/m1/a1/notes.txt".to_string(),
                    created_at: 1,
                }],
                project_attachments: Vec::new(),
            }),
            permissions: Default::default(),
        }));
        let registry = ToolRegistry::read_only_defaults_with_search(None);

        let list_result = registry.execute(
            &context,
            &AgentToolCall {
                id: "call-list".to_string(),
                tool: "attachments_list".to_string(),
                args: json!({}),
                approval_status: crate::protocol::AgentApprovalStatus::NotRequired,
                reason: None,
            },
        );
        assert!(list_result.ok, "{:?}", list_result.error);
        assert_eq!(
            list_result.result.as_ref().unwrap()["attachments"][0]["readPath"],
            "@attachments/a1/notes.txt"
        );

        let read_result = registry.execute(
            &context,
            &AgentToolCall {
                id: "call-read".to_string(),
                tool: "read_file".to_string(),
                args: json!({ "path": "@attachments/a1/notes.txt" }),
                approval_status: crate::protocol::AgentApprovalStatus::NotRequired,
                reason: None,
            },
        );
        assert!(read_result.ok, "{:?}", read_result.error);
        assert_eq!(
            read_result.result.as_ref().unwrap()["content"],
            "hello from attachment"
        );

        let traversal_result = registry.execute(
            &context,
            &AgentToolCall {
                id: "call-read-traversal".to_string(),
                tool: "read_file".to_string(),
                args: json!({ "path": "@attachments/a1/../notes.txt" }),
                approval_status: crate::protocol::AgentApprovalStatus::NotRequired,
                reason: None,
            },
        );
        assert!(!traversal_result.ok);
        assert!(traversal_result.error.unwrap().contains("完整 readPath"));
    }

    #[test]
    fn read_image_reads_attachment_visual_payload() {
        let fixture = TestWorkspace::new();
        let attachment_root = fixture.root.join("attachments");
        let storage_rel_path = PathBuf::from("conversations/c1/m1/image1/pixel.png");
        let attachment_path = attachment_root.join(&storage_rel_path);
        fs::create_dir_all(attachment_path.parent().unwrap()).unwrap();
        fs::write(&attachment_path, b"not-a-real-png-but-valid-tool-bytes").unwrap();

        let context = ToolExecutionContext::from_run_context(Some(&AgentRunContext {
            conversation_id: Some("c1".to_string()),
            project_id: Some("p1".to_string()),
            workspace: None,
            attachment_library: Some(AgentAttachmentLibraryContext {
                root_path: Some(attachment_root.to_string_lossy().to_string()),
                conversation_id: Some("c1".to_string()),
                project_id: Some("p1".to_string()),
                conversation_attachments: vec![AgentAttachmentReference {
                    id: "image1".to_string(),
                    conversation_id: "c1".to_string(),
                    message_id: "m1".to_string(),
                    project_id: Some("p1".to_string()),
                    kind: AgentInputAttachmentKind::Image,
                    name: "pixel.png".to_string(),
                    mime_type: Some("image/png".to_string()),
                    size_bytes: 31,
                    read_path: "@attachments/image1/pixel.png".to_string(),
                    storage_rel_path: "conversations/c1/m1/image1/pixel.png".to_string(),
                    created_at: 1,
                }],
                project_attachments: Vec::new(),
            }),
            permissions: Default::default(),
        }));
        let registry = ToolRegistry::read_only_defaults_with_search(None);
        let result = registry.execute(
            &context,
            &AgentToolCall {
                id: "call-image".to_string(),
                tool: "read_image".to_string(),
                args: json!({ "path": "@attachments/image1/pixel.png" }),
                approval_status: crate::protocol::AgentApprovalStatus::NotRequired,
                reason: None,
            },
        );

        assert!(result.ok, "{:?}", result.error);
        let value = result.result.as_ref().unwrap();
        assert_eq!(value["path"], "@attachments/image1/pixel.png");
        assert_eq!(value["mimeType"], "image/png");
        assert!(value["image"]["dataBase64"].as_str().unwrap().len() > 10);
    }

    #[test]
    fn absolute_read_requires_all_permission() {
        let fixture = TestWorkspace::new();
        let outside = std::env::temp_dir().join(format!(
            "my-copilot-agent-outside-read-{}",
            TEST_WORKSPACE_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::write(&outside, "outside content").unwrap();
        let registry = ToolRegistry::read_only_defaults_with_search(None);
        let call = AgentToolCall {
            id: "call-outside".to_string(),
            tool: "read_file".to_string(),
            args: json!({ "path": outside.to_string_lossy() }),
            approval_status: crate::protocol::AgentApprovalStatus::NotRequired,
            reason: None,
        };

        let denied = registry.execute(&fixture.context(), &call);
        assert!(!denied.ok);
        assert!(denied.error.unwrap().contains("仅允许访问 workspace"));

        let allowed = ToolExecutionContext::from_run_context(Some(&AgentRunContext {
            conversation_id: None,
            project_id: None,
            workspace: Some(AgentWorkspaceContext {
                project_id: None,
                display_name: Some("test".to_string()),
                root_path: Some(fixture.root.to_string_lossy().to_string()),
            }),
            attachment_library: None,
            permissions: crate::protocol::AgentPermissions {
                read: crate::protocol::AgentReadPermission::All,
                ..Default::default()
            },
        }));
        let result = registry.execute(&allowed, &call);
        assert!(result.ok, "{:?}", result.error);
        assert_eq!(result.result.unwrap()["content"], "outside content");

        let _ = fs::remove_file(outside);
    }

    struct TestWorkspace {
        root: PathBuf,
    }

    impl TestWorkspace {
        fn new() -> Self {
            let unique = TEST_WORKSPACE_COUNTER.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!("my-copilot-agent-test-tools-{unique}"));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(&root).unwrap();
            Self { root }
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
