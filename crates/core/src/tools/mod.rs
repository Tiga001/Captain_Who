mod apply_patch;
mod apply_patch_diff;
pub(crate) mod apply_patch_paths;
mod attachments;
mod context;
mod conversation_history;
mod document_text;
mod filesystem;
mod git_diff;
mod input_stream;
mod limits;
mod office;
mod read_file;
mod read_image;
mod read_pdf;
mod read_presentation;
mod read_spreadsheet;
mod read_word;
mod run_command;
pub(crate) mod schema;
mod search_code;
mod search_files;
mod skills_list_resources;
mod skills_materialize_resource;
mod skills_read_resource;
mod skills_script;
mod web_fetch;
mod web_search;
mod workspace_map;
mod write_file;
mod write_file_stream;

use crate::conversation_trace::canonical_tool_result_for_context;
use crate::protocol::{
    AgentError, AgentFileWritePreview, AgentProposedAction, AgentResult, AgentSearchConfig,
    AgentSearchMode, AgentToolCall, AgentToolDefinition, AgentToolResult,
};
use apply_patch::ApplyPatchTool;
use attachments::{AttachmentsListProjectTool, AttachmentsListTool};
use conversation_history::ConversationHistoryTool;
use git_diff::GitDiffTool;
pub(crate) use office::validate_frozen_office_trace_args;
use office::{OfficeDocumentTool, OfficePresentationTool, OfficeSpreadsheetTool};
use read_file::ReadFileTool;
use read_image::ReadImageTool;
use read_pdf::ReadPdfTool;
use read_presentation::ReadPresentationTool;
use read_spreadsheet::ReadSpreadsheetTool;
use read_word::ReadWordTool;
pub(crate) use run_command::validate_frozen_command_trace_args;
use run_command::RunCommandTool;
use schema::validate_portable_tool_input_schema;
use search_code::SearchCodeTool;
use search_files::SearchFilesTool;
use serde_json::Value;
use skills_list_resources::SkillsListResourcesTool;
pub(crate) use skills_materialize_resource::validate_frozen_materialization_trace_args;
use skills_materialize_resource::SkillsMaterializeResourceTool;
use skills_read_resource::SkillsReadResourceTool;
pub(crate) use skills_script::validate_frozen_skill_script_trace_args;
use skills_script::{SkillsPreflightScriptTool, SkillsRunScriptTool};
use std::collections::BTreeMap;
use std::sync::Arc;
use web_fetch::WebFetchTool;
use web_search::WebSearchTool;
use workspace_map::WorkspaceMapTool;
use write_file::WriteFileTool;

pub(super) use context::ToolExecutionContext;
use document_text::{
    extract_with_textutil, join_named_text, normalize_text_output, read_zip_xml_text_parts,
    reserve_zip_xml_entry, resolve_document_path, sanitize_document_max_chars, NamedText,
};
use filesystem::{
    block_on_tool_future, clean_relative_path, relative_display, sanitize_limit, truncate_chars,
    walk_workspace_with_cancellation, WalkEntry, WalkResult,
};
use limits::*;

pub struct ToolRegistry {
    tools: BTreeMap<String, Box<dyn AgentTool>>,
    owners: BTreeMap<String, String>,
}

/// Declares which host permission policy authorizes a tool's state-changing
/// calls. Keeping this on the tool implementation means future built-in and
/// extension tools opt into the common policy without adding their names to a
/// second, easily-stale allowlist in the runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AgentToolPermissionPolicy {
    Default,
    FileWrite(FileWriteToolAccess),
}

/// Controls whether a file-writing tool still has useful read-only calls when
/// writes are disabled. `ReadWrite` tools stay visible, but their write calls
/// must still fail closed in proposal preparation and host execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FileWriteToolAccess {
    WriteOnly,
    ReadWrite,
}

impl AgentToolPermissionPolicy {
    pub(crate) fn uses_file_write_approval(self) -> bool {
        matches!(self, Self::FileWrite(_))
    }

    pub(crate) fn is_available_when_write_denied(self) -> bool {
        !matches!(self, Self::FileWrite(FileWriteToolAccess::WriteOnly))
    }
}

impl ToolRegistry {
    pub fn defaults_with_search(search_config: Option<&AgentSearchConfig>) -> Self {
        Self::defaults_with_search_and_office(search_config, None)
    }

    pub(crate) fn defaults_with_search_and_office(
        search_config: Option<&AgentSearchConfig>,
        office_engine: Option<Arc<dyn crate::office::OfficeEngine>>,
    ) -> Self {
        let mut registry = Self {
            tools: BTreeMap::new(),
            owners: BTreeMap::new(),
        };
        registry.register(AttachmentsListTool);
        registry.register(AttachmentsListProjectTool);
        registry.register(ReadFileTool);
        registry.register(ReadImageTool);
        registry.register(ReadPdfTool);
        registry.register(ReadWordTool);
        registry.register(ReadPresentationTool);
        registry.register(ReadSpreadsheetTool);
        if let Some(engine) = office_engine {
            registry.register(OfficeDocumentTool::new(engine.clone()));
            registry.register(OfficeSpreadsheetTool::new(engine.clone()));
            registry.register(OfficePresentationTool::new(engine));
        }
        registry.register(WorkspaceMapTool);
        registry.register(SearchFilesTool);
        registry.register(SearchCodeTool);
        if let Some(api_key) = tavily_api_key(search_config) {
            registry.register(WebSearchTool::new(api_key.clone()));
            registry.register(WebFetchTool::new(api_key));
        }
        registry.register(GitDiffTool);
        registry.register(ApplyPatchTool);
        registry.register(WriteFileTool);
        registry.register(RunCommandTool);
        registry.register(SkillsListResourcesTool);
        registry.register(SkillsReadResourceTool);
        registry.register(SkillsMaterializeResourceTool);
        registry.register(SkillsPreflightScriptTool);
        registry.register(SkillsRunScriptTool);
        registry
    }

    pub fn definitions(&self) -> Vec<AgentToolDefinition> {
        self.tools.values().map(|tool| tool.definition()).collect()
    }

    #[cfg(test)]
    pub fn definition_for(&self, tool_name: &str) -> Option<AgentToolDefinition> {
        self.tools.get(tool_name).map(|tool| tool.definition())
    }

    pub fn requires_approval_for_call(&self, tool_name: &str, args: &Value) -> bool {
        self.tools
            .get(tool_name)
            .map(|tool| tool.requires_approval_for_call(args))
            .unwrap_or(false)
    }

    pub(crate) fn permission_policy(&self, tool_name: &str) -> AgentToolPermissionPolicy {
        self.tools
            .get(tool_name)
            .map(|tool| tool.permission_policy())
            .unwrap_or(AgentToolPermissionPolicy::Default)
    }

    pub(crate) fn input_stream_observer(
        &self,
        tool_name: &str,
        context: ToolExecutionContext,
    ) -> Option<Box<dyn ToolInputStreamObserver>> {
        self.tools
            .get(tool_name)
            .and_then(|tool| tool.input_stream_observer(context))
    }

    pub(crate) fn contains_tool(&self, tool_name: &str) -> bool {
        self.tools.contains_key(tool_name)
    }

    pub(crate) fn contains_tool_prefix(&self, tool_name: &str) -> bool {
        self.tools.keys().any(|name| name.starts_with(tool_name))
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
                result: structured_error_result(&error),
                error: Some(error.to_string()),
            },
        }
    }

    /// Returns the durable, history-safe projection of a tool result.
    ///
    /// Most tools retain their complete canonical result. Tools that disclose
    /// run-scoped or sensitive payloads can override this hook so the current
    /// model turn receives the payload while conversation history stores only
    /// stable provenance and range metadata.
    pub(crate) fn trace_projection(&self, result: &AgentToolResult) -> AgentToolResult {
        self.tools
            .get(&result.tool)
            .map(|tool| tool.trace_projection(result))
            .unwrap_or_else(|| canonical_tool_result_for_context(result))
    }

    /// Returns the projection safe to publish through runtime events.
    pub(crate) fn event_projection(&self, result: &AgentToolResult) -> AgentToolResult {
        self.tools
            .get(&result.tool)
            .map(|tool| tool.event_projection(result))
            .unwrap_or_else(|| canonical_tool_result_for_context(result))
    }

    /// Returns the projection safe to serialize into an approval checkpoint.
    pub(crate) fn checkpoint_projection(&self, result: &AgentToolResult) -> AgentToolResult {
        self.tools
            .get(&result.tool)
            .map(|tool| tool.checkpoint_projection(result))
            .unwrap_or_else(|| canonical_tool_result_for_context(result))
    }

    pub(crate) fn register_extension_tool(
        &mut self,
        extension_id: &str,
        tool: Box<dyn AgentTool>,
    ) -> AgentResult<()> {
        self.register_boxed(format!("extension:{extension_id}"), tool)
    }

    pub(crate) fn register_conversation_history(&mut self) {
        if !self.contains_tool("conversation_history") {
            self.register(ConversationHistoryTool);
        }
    }

    fn register<T: AgentTool + 'static>(&mut self, tool: T) {
        self.register_boxed("core".to_string(), Box::new(tool))
            .expect("core tool definitions must have valid names and portable input schemas");
    }

    fn register_boxed(&mut self, owner: String, tool: Box<dyn AgentTool>) -> AgentResult<()> {
        let definition = tool.definition();
        let name = definition.name.trim();
        if name.is_empty() {
            return Err(AgentError::new(format!(
                "工具注册失败：{owner} 提供了空工具名。"
            )));
        }
        if name != definition.name {
            return Err(AgentError::new(format!(
                "工具注册失败：`{}` 的名称首尾不能包含空白字符。",
                definition.name
            )));
        }
        validate_portable_tool_input_schema(name, &definition.input_schema)?;
        if let Some(existing_owner) = self.owners.get(name) {
            return Err(AgentError::new(format!(
                "工具注册冲突：`{name}` 已由 {existing_owner} 注册，{owner} 不能重复注册。"
            )));
        }

        let name = name.to_string();
        self.tools.insert(name.clone(), tool);
        self.owners.insert(name, owner);
        Ok(())
    }
}

fn structured_error_result(error: &AgentError) -> Option<Value> {
    error.details().cloned().map(|mut details| {
        if let (Some(code), Some(object)) = (error.code(), details.as_object_mut()) {
            object
                .entry("errorCode".to_string())
                .or_insert_with(|| serde_json::json!(code));
        }
        details
    })
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

pub(crate) trait AgentTool: Send + Sync {
    fn definition(&self) -> AgentToolDefinition;
    fn execute(&self, context: &ToolExecutionContext, args: Value) -> AgentResult<Value>;
    fn permission_policy(&self) -> AgentToolPermissionPolicy;

    fn proposed_action(
        &self,
        _context: &ToolExecutionContext,
        call: &AgentToolCall,
    ) -> AgentResult<AgentProposedAction> {
        Ok(AgentProposedAction::ToolCall { call: call.clone() })
    }

    fn requires_approval_for_call(&self, _args: &Value) -> bool {
        self.definition().requires_approval
    }

    fn input_stream_observer(
        &self,
        _context: ToolExecutionContext,
    ) -> Option<Box<dyn ToolInputStreamObserver>> {
        None
    }

    fn trace_projection(&self, result: &AgentToolResult) -> AgentToolResult {
        canonical_tool_result_for_context(result)
    }

    fn event_projection(&self, result: &AgentToolResult) -> AgentToolResult {
        self.trace_projection(result)
    }

    fn checkpoint_projection(&self, result: &AgentToolResult) -> AgentToolResult {
        self.trace_projection(result)
    }
}

pub(crate) struct ToolInputStreamChunk<'a> {
    pub stream_id: &'a str,
    pub attempt: usize,
    pub tool_call_index: usize,
    pub tool_call_id: Option<&'a str>,
    pub input_delta: &'a str,
    pub received_bytes: u64,
}

pub(crate) enum ToolInputStreamPreview {
    FileWrite(AgentFileWritePreview),
}

pub(crate) trait ToolInputStreamObserver: Send {
    fn on_delta(
        &mut self,
        chunk: &ToolInputStreamChunk<'_>,
    ) -> AgentResult<Option<ToolInputStreamPreview>>;

    fn flush(&mut self) -> AgentResult<Option<ToolInputStreamPreview>>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{
        AgentAttachmentLibraryContext, AgentAttachmentReference, AgentInputAttachmentKind,
        AgentRunContext, AgentSearchConfig, AgentSearchMode, AgentToolCall, AgentToolSafety,
        AgentWorkspaceContext,
    };
    use serde_json::json;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_WORKSPACE_COUNTER: AtomicU64 = AtomicU64::new(1);

    struct InvalidSchemaTool;

    impl AgentTool for InvalidSchemaTool {
        fn permission_policy(&self) -> AgentToolPermissionPolicy {
            AgentToolPermissionPolicy::Default
        }

        fn definition(&self) -> AgentToolDefinition {
            AgentToolDefinition {
                name: "invalid_schema".to_string(),
                description: "test".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string" }
                    },
                    "anyOf": [
                        { "required": ["path"] }
                    ]
                }),
                safety: crate::protocol::AgentToolSafety::ReadOnly,
                requires_workspace: false,
                requires_approval: false,
                approval_mode: crate::protocol::AgentToolApprovalMode::Never,
            }
        }

        fn execute(&self, _context: &ToolExecutionContext, _args: Value) -> AgentResult<Value> {
            Ok(json!({}))
        }
    }

    #[test]
    fn rejects_paths_outside_workspace() {
        let fixture = TestWorkspace::new();
        let context = fixture.context();
        let error = context.resolve_existing_path("../outside.txt").unwrap_err();

        assert!(error.to_string().contains("路径不能包含"));
    }

    #[test]
    fn registers_tavily_tools_when_configured() {
        let registry = ToolRegistry::defaults_with_search(Some(&AgentSearchConfig {
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
    fn every_builtin_tool_uses_the_portable_root_schema_contract() {
        let mut registry = ToolRegistry::defaults_with_search(Some(&AgentSearchConfig {
            mode: AgentSearchMode::Tavily,
            tavily_api_key: Some("tvly-test".to_string()),
        }));
        registry.register_conversation_history();

        for definition in registry.definitions() {
            validate_portable_tool_input_schema(&definition.name, &definition.input_schema)
                .unwrap_or_else(|error| panic!("{}: {error}", definition.name));
        }
    }

    #[test]
    fn rejects_incompatible_extension_tool_schema_during_registration() {
        let mut registry = ToolRegistry::defaults_with_search(None);

        let error = registry
            .register_extension_tool("test", Box::new(InvalidSchemaTool))
            .unwrap_err();

        assert!(error.to_string().contains("invalid_schema"));
        assert!(error.to_string().contains("anyOf"));
        assert!(!registry.contains_tool("invalid_schema"));
    }

    #[test]
    fn registers_run_command_as_approval_tool() {
        let registry = ToolRegistry::defaults_with_search(None);
        let definition = registry.definition_for("run_command").unwrap();

        assert_eq!(definition.name, "run_command");
        assert!(!definition.requires_workspace);
        assert!(definition.requires_approval);
    }

    #[test]
    fn registers_apply_patch_as_approval_tool() {
        let registry = ToolRegistry::defaults_with_search(None);
        let definition = registry.definition_for("apply_patch").unwrap();

        assert_eq!(definition.name, "apply_patch");
        assert!(!definition.requires_workspace);
        assert!(definition.requires_approval);
    }

    #[test]
    fn registers_progressive_skill_runtime_tools_with_separate_safety_boundaries() {
        let registry = ToolRegistry::defaults_with_search(None);
        let list = registry.definition_for("skills_list_resources").unwrap();
        let read = registry.definition_for("skills_read_resource").unwrap();
        let materialize = registry
            .definition_for("skills_materialize_resource")
            .unwrap();
        let preflight = registry.definition_for("skills_preflight_script").unwrap();
        let run = registry.definition_for("skills_run_script").unwrap();

        assert!(!list.requires_approval);
        assert!(!read.requires_approval);
        assert!(materialize.requires_approval);
        assert!(!preflight.requires_approval);
        assert!(run.requires_approval);
        assert!(materialize.requires_workspace);
        assert!(preflight.requires_workspace);
        assert!(run.requires_workspace);
    }

    #[test]
    fn registers_three_native_office_tools_with_dynamic_write_approval() {
        let engine =
            crate::office::resolve_office_engine(&crate::office::OfficeCliDiscoveryOptions::new());
        let registry = ToolRegistry::defaults_with_search_and_office(None, Some(engine));

        for (tool_name, document_path) in [
            ("office_document", "test.docx"),
            ("office_spreadsheet", "test.xlsx"),
            ("office_presentation", "test.pptx"),
        ] {
            let definition = registry.definition_for(tool_name).unwrap();
            assert_eq!(definition.safety, AgentToolSafety::RequiresApproval);
            assert_eq!(
                definition.approval_mode,
                crate::protocol::AgentToolApprovalMode::Dynamic
            );
            assert!(!definition.requires_workspace);
            assert!(
                !registry.requires_approval_for_call(tool_name, &json!({ "operation": "status" }))
            );
            for args in [
                json!({ "operation": "help" }),
                json!({ "operation": "get", "path": document_path }),
                json!({ "operation": "query", "path": document_path, "arguments": ["/sheet[1]"] }),
                json!({ "operation": "validate", "path": document_path }),
            ] {
                assert!(!registry.requires_approval_for_call(tool_name, &args));
            }
            assert!(!registry.requires_approval_for_call(
                tool_name,
                &json!({ "operation": "view", "path": document_path, "arguments": ["text"] })
            ));
            for args in [
                json!({ "operation": "create", "path": document_path }),
                json!({ "operation": "set", "path": document_path, "arguments": ["/sheet[1]"] }),
                json!({ "operation": "add", "path": document_path, "arguments": ["/sheet[1]"] }),
                json!({ "operation": "remove", "path": document_path, "arguments": ["/sheet[1]"] }),
                json!({ "operation": "move", "path": document_path, "arguments": ["/sheet[1]"] }),
                json!({ "operation": "swap", "path": document_path, "arguments": ["/sheet[1]", "/sheet[2]"] }),
            ] {
                assert!(
                    registry.requires_approval_for_call(tool_name, &args),
                    "{tool_name} write operations must use the file-edit approval path"
                );
            }
            assert!(registry.requires_approval_for_call(
                tool_name,
                &json!({
                    "operation": "view",
                    "path": document_path,
                    "arguments": ["html"],
                    "outputPath": "preview.html"
                })
            ));
            assert!(registry
                .requires_approval_for_call(tool_name, &json!({ "operation": "unsupported" })));
            for invalid in [
                json!({ "operation": "status", "path": document_path }),
                json!({ "operation": "help", "path": document_path }),
                json!({ "operation": "query", "path": document_path }),
                json!({ "operation": "validate", "path": document_path, "arguments": ["--output", "stolen.xlsx"] }),
                json!({ "operation": "validate", "path": document_path, "outputPath": "preview.html" }),
                json!({ "operation": "validate", "path": document_path, "access": "readOnly" }),
                json!({ "operation": "validate", "path": "wrong-extension.bin" }),
            ] {
                assert!(
                    registry.requires_approval_for_call(tool_name, &invalid),
                    "invalid Office calls must fail closed during approval routing"
                );
            }
            assert_eq!(
                registry.permission_policy(tool_name),
                AgentToolPermissionPolicy::FileWrite(FileWriteToolAccess::ReadWrite)
            );
        }
    }

    #[test]
    fn structured_writers_declare_the_shared_file_write_permission_policy() {
        let registry = ToolRegistry::defaults_with_search(None);

        for tool_name in ["apply_patch", "write_file", "skills_materialize_resource"] {
            assert_eq!(
                registry.permission_policy(tool_name),
                AgentToolPermissionPolicy::FileWrite(FileWriteToolAccess::WriteOnly),
                "{tool_name} must opt into the common file-write policy"
            );
        }
        assert_eq!(
            registry.permission_policy("read_file"),
            AgentToolPermissionPolicy::Default
        );
    }

    #[test]
    fn write_file_uses_dynamic_finish_approval() {
        let registry = ToolRegistry::defaults_with_search(None);
        let definition = registry.definition_for("write_file").unwrap();

        assert_eq!(
            definition.approval_mode,
            crate::protocol::AgentToolApprovalMode::Dynamic
        );
        assert!(!registry.requires_approval_for_call("write_file", &json!({ "phase": "append" })));
        assert!(registry.requires_approval_for_call("write_file", &json!({ "phase": "finish" })));
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
        let registry = ToolRegistry::defaults_with_search(None);

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
        let registry = ToolRegistry::defaults_with_search(None);
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
        let registry = ToolRegistry::defaults_with_search(None);
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

    #[test]
    fn structured_tool_failures_preserve_machine_readable_details() {
        let error = AgentError::structured(
            "skill_script.dependency_missing",
            "dependency is missing",
            json!({
                "type": "skill_script_preflight",
                "code": "dependencyMissing",
                "missing": ["openpyxl"],
            }),
        );

        let result = structured_error_result(&error).unwrap();
        assert_eq!(result["missing"], json!(["openpyxl"]));
        assert_eq!(result["errorCode"], "skill_script.dependency_missing");
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
