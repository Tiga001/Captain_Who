mod agent_collaboration;
mod apply_patch;
mod attachments;
mod automation_report;
mod builtin_capability;
mod command_session;
mod context;
mod conversation_history;
mod document_text;
#[cfg(test)]
mod file_change_round6_acceptance;
mod file_change_staged;
mod file_change_stream;
mod filesystem;
pub(crate) mod human_interaction;
mod image_generation;
mod input_stream;
mod limits;
mod mcp;
pub(crate) mod model_projection;
mod office;
mod read_file;
mod read_image;
mod read_presentation;
mod read_spreadsheet;
mod read_word;
mod run_command;
pub(crate) mod schema;
mod search_code;
mod search_cursor;
mod search_files;
mod skills_commit_install;
mod skills_list_resources;
mod skills_materialize_resource;
mod skills_prepare_install;
mod skills_read_resource;
mod skills_script;
mod tool_set;
mod web_fetch;
mod web_search;
mod workspace_map;

use crate::conversation_trace::canonical_tool_result_for_context;
use crate::protocol::{
    AgentError, AgentFileChangePreview, AgentProposedAction, AgentResult, AgentSearchConfig,
    AgentSearchMode, AgentToolCall, AgentToolDefinition, AgentToolIdentity, AgentToolResult,
};
use agent_collaboration::{AgentCollaborationTool, AgentCollaborationToolKind};
use apply_patch::ApplyPatchTool;
pub(crate) use apply_patch::{
    apply_patch_action, apply_patch_request, apply_patch_wire_is_valid,
    attach_successor_observation_to_model_result,
    attach_successor_observation_to_model_result_with_predecessor,
    attach_successor_observation_to_model_result_with_proposal,
    copy_successor_observation_projection,
};
use attachments::{AttachmentsListProjectTool, AttachmentsListTool};
use automation_report::AutomationReportTool;
pub use automation_report::{AutomationReportKind, AutomationReportSink};
pub use builtin_capability::builtin_capability_tool_result_persistence_projection;
pub(crate) use builtin_capability::{
    tool_capability_id as builtin_tool_capability_id, ActivateCapabilityTool,
    BuiltinCapabilityAgentTool, BUILTIN_ACTIVATION_CAPABILITY,
};
use command_session::CommandSessionTool;
use conversation_history::ConversationHistoryTool;
use image_generation::ImageGenerationTool;
pub use image_generation::{
    agent_image_generation_execution_id, agent_image_generation_tool_result_from_execution,
    agent_image_generation_tool_result_from_service_error, normalize_agent_image_generation_reason,
};
pub(crate) use office::validate_frozen_office_trace_args;
use office::{OfficeDocumentTool, OfficePresentationTool, OfficeSpreadsheetTool};
use read_file::ReadFileTool;
use read_image::ReadImageTool;
use read_presentation::ReadPresentationTool;
use read_spreadsheet::ReadSpreadsheetTool;
use read_word::ReadWordTool;
pub(crate) use run_command::validate_frozen_command_trace_args;
use run_command::RunCommandTool;
use schema::validate_portable_tool_input_schema;
use search_code::SearchCodeTool;
use search_files::SearchFilesTool;
use serde_json::Value;
use skills_commit_install::SkillsCommitInstallTool;
pub use skills_commit_install::{
    AgentSkillInstallationCommitPreparationRequest, AgentSkillInstallationCommitPreparer,
};
use skills_list_resources::SkillsListResourcesTool;
pub(crate) use skills_materialize_resource::validate_frozen_materialization_trace_args;
use skills_materialize_resource::SkillsMaterializeResourceTool;
use skills_prepare_install::SkillsPrepareInstallTool;
pub use skills_prepare_install::{
    AgentSkillInstallationPrepareExecutor, AgentSkillInstallationPrepareRequest,
    AgentSkillInstallationPrepareSource,
};
use skills_read_resource::SkillsReadResourceTool;
pub(crate) use skills_script::validate_frozen_skill_script_trace_args;
use skills_script::{SkillsPreflightScriptTool, SkillsRunScriptTool};
use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
pub(crate) use tool_set::{
    validate_tool_set_checkpoint_shape, EffectiveToolSet, ToolCapabilityId, ToolUnavailability,
    IMAGE_GENERATION_CAPABILITY, OFFICE_DOCUMENTS_CAPABILITY, OFFICE_PRESENTATIONS_CAPABILITY,
    OFFICE_SPREADSHEETS_CAPABILITY, SKILL_INSTALLATION_CAPABILITY,
    SKILL_RESOURCES_MATERIALIZE_CAPABILITY, SKILL_RESOURCES_READ_CAPABILITY,
    SKILL_SCRIPTS_CAPABILITY,
};
pub(crate) use web_fetch::WebFetchTool;
pub(crate) use web_search::WebSearchTool;
pub(crate) use tool_set::WEB_SEARCH_CAPABILITY;
use workspace_map::WorkspaceMapTool;

/// Reprojects a durable Trace call into the same presentation-safe shape emitted live.
/// Most Trace projections are already Renderer-safe; tools with a deliberately richer durable
/// contract are adapted here without constructing their execution dependencies.
pub(crate) fn renderer_call_projection_from_trace(call: &AgentToolCall) -> AgentToolCall {
    match call.tool.as_str() {
        image_generation::TOOL_NAME => {
            image_generation::image_generation_event_call_projection(call)
        }
        _ => call.clone(),
    }
}

/// Reprojects a durable Trace result into the same presentation-safe shape emitted live.
pub(crate) fn renderer_result_projection_from_trace(result: &AgentToolResult) -> AgentToolResult {
    match result.tool.as_str() {
        image_generation::TOOL_NAME => image_generation::image_generation_event_projection(result),
        _ => canonical_tool_result_for_context(result),
    }
}

pub(super) use context::ToolExecutionContext;
use document_text::{
    complete_document_text_result, extract_with_textutil, join_named_text, normalize_text_output,
    read_zip_xml_text_parts, reserve_zip_xml_entry, resolve_document_path, NamedText,
};
use filesystem::{
    block_on_tool_future, clean_relative_path, relative_display, sanitize_limit, truncate_chars,
    walk_workspace_with_cancellation, WalkEntry, WalkResult,
};
use limits::*;
use mcp::McpAgentTool;
pub use mcp::{
    mcp_normalized_input_schema_identity, mcp_tool_arguments_digest, mcp_tool_invocation_event,
    mcp_tool_result_from_approved_invocation, mcp_tool_result_from_rejected_approval,
    mcp_tool_result_model_projection, mcp_tool_result_persistence_projection,
    mcp_tool_result_size_summary, validate_mcp_approval_arguments, McpAgentToolAnnotations,
    McpAgentToolDescriptor, McpApprovedToolInvocation, McpNormalizedInputSchemaIdentity,
    McpOmittedContentKind, McpRuntimeProjectionLimits, McpToolApprovalRequest,
    McpToolCatalogContext, McpToolContentBlock, McpToolDiagnosticCode,
    McpToolInvocationEventUpdate, McpToolInvocationFuture, McpToolInvocationResult, McpToolInvoker,
    McpToolRegistrationDiagnostic, McpToolRuntime, MCP_INPUT_SCHEMA_NORMALIZER_VERSION,
    MCP_RUNTIME_MAX_CATALOG_BYTES, MCP_RUNTIME_MAX_TOOL_DEFINITIONS,
};

/// Rebuilds the model-only projection for a host result restored after approval.
///
/// Approval checkpoints persist the rich canonical result for recovery. The live registry is not
/// available while the checkpoint is reconstructed, so approval-capable built-ins share these
/// pure projection functions with their normal `AgentTool::model_projection` hooks.
pub(crate) fn model_projection_for_persisted_continuation(
    result: &AgentToolResult,
) -> AgentToolResult {
    match result.tool.as_str() {
        "run_command" => run_command::run_command_model_projection(result),
        "office_document" | "office_spreadsheet" | "office_presentation" => {
            office::office_model_projection(result)
        }
        "skills_materialize_resource" => {
            skills_materialize_resource::skills_materialize_model_projection(result)
        }
        "skills_run_script" => skills_script::skills_run_script_model_projection(result),
        "image_generation" => image_generation::image_generation_model_projection(result),
        _ => canonical_tool_result_for_context(result),
    }
}

/// Rebuilds the exact-history projection for a host result restored after approval.
///
/// Most approval-capable tools persist their canonical result. Keeping this dispatcher beside
/// the model dispatcher prevents a future tool-specific archive projection from being bypassed
/// solely because the tool completed in the Host rather than the live runtime registry.
pub(crate) fn archive_projection_for_persisted_continuation(
    result: &AgentToolResult,
) -> AgentToolResult {
    match result.tool.as_str() {
        "image_generation" => image_generation::image_generation_history_projection(result),
        "read_image" => read_image::read_image_history_projection(result),
        _ => canonical_tool_result_for_context(result),
    }
}

pub(crate) fn tool_result_truncated_at_source(result: &AgentToolResult) -> bool {
    result
        .result
        .as_ref()
        .is_some_and(value_contains_unrecoverable_source_truncation)
}

// `truncated` is intentionally not in this list: it is the shared page/projection marker and can
// describe Semantic Pagination when a usable continuation is present. These dedicated flags say
// that capture itself omitted source material and therefore remain authoritative.
const DEDICATED_SOURCE_TRUNCATION_FIELDS: &[&str] = &[
    "stdoutTruncated",
    "stdout_truncated",
    "stderrTruncated",
    "stderr_truncated",
    "changesTruncated",
    "changes_truncated",
];

const SOURCE_TRUNCATION_DECLARATION_FIELDS: &[&str] = &["truncatedAtSource", "truncated_at_source"];

const SEMANTIC_RECOVERY_FIELDS: &[&str] = &[
    "continueWith",
    "continue_with",
    "cursor",
    "next",
    "nextCursor",
    "next_cursor",
    "nextStartByte",
    "next_start_byte",
    "nextStartLine",
    "next_start_line",
    "nextAfterPath",
    "next_after_path",
    "historyOpen",
    "history_open",
];

pub(crate) fn value_contains_unrecoverable_source_truncation(value: &Value) -> bool {
    let Value::Object(object) = value else {
        return match value {
            Value::Array(values) => values
                .iter()
                .any(value_contains_unrecoverable_source_truncation),
            _ => false,
        };
    };

    let declared_source_truncation = object
        .get("truncatedAtSource")
        .or_else(|| object.get("truncated_at_source"))
        .and_then(Value::as_bool);
    if declared_source_truncation == Some(true) {
        // An explicit source-truncation declaration is authoritative. A history archive or other
        // continuation can recover the payload that reached this process, but cannot make bytes
        // omitted by the source complete.
        return true;
    }

    // Legacy dedicated flags normally mean the Tool already discarded bytes or observations.
    // New process receipts pair their preview-only legacy flag with an explicit
    // `truncatedAtSource: false`, which authoritatively distinguishes a recoverable preview cut.
    if declared_source_truncation != Some(false)
        && DEDICATED_SOURCE_TRUNCATION_FIELDS
            .iter()
            .any(|key| object.get(*key).is_some_and(value_contains_true))
    {
        return true;
    }

    let has_recovery = SEMANTIC_RECOVERY_FIELDS
        .iter()
        .any(|key| object.get(*key).is_some_and(valid_recovery_value))
        || object
            .get("navigation")
            .and_then(Value::as_object)
            .is_some_and(|navigation| navigation.values().any(valid_recovery_value));
    let generically_truncated = object.get("truncated").is_some_and(value_contains_true);
    if declared_source_truncation != Some(false) && generically_truncated && !has_recovery {
        return true;
    }

    object.iter().any(|(key, value)| {
        key != "truncated"
            && !DEDICATED_SOURCE_TRUNCATION_FIELDS.contains(&key.as_str())
            && !SOURCE_TRUNCATION_DECLARATION_FIELDS.contains(&key.as_str())
            && !SEMANTIC_RECOVERY_FIELDS.contains(&key.as_str())
            && key != "navigation"
            && value_contains_unrecoverable_source_truncation(value)
    })
}

fn valid_recovery_value(value: &Value) -> bool {
    match value {
        Value::Null | Value::Bool(_) => false,
        Value::String(value) => !value.trim().is_empty(),
        Value::Number(_) => true,
        Value::Array(values) => values.iter().any(valid_recovery_value),
        Value::Object(values) => values.values().any(valid_recovery_value),
    }
}

fn value_contains_true(value: &Value) -> bool {
    match value {
        Value::Bool(value) => *value,
        Value::Array(values) => values.iter().any(value_contains_true),
        Value::Object(values) => values.values().any(value_contains_true),
        _ => false,
    }
}

pub(crate) type BoxAgentToolFuture<'a> =
    Pin<Box<dyn Future<Output = AgentResult<AgentToolExecutionValue>> + Send + 'a>>;
pub(crate) type BoxAgentProposedActionFuture<'a> =
    Pin<Box<dyn Future<Output = AgentResult<AgentProposedAction>> + Send + 'a>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AgentToolResultPersistence {
    RuntimeCommits,
    PrecommittedTrace,
}

pub(crate) struct AgentToolExecutionValue {
    pub(crate) value: Value,
    pub(crate) persistence: AgentToolResultPersistence,
}

pub(crate) struct RegisteredToolExecution {
    pub(crate) result: AgentToolResult,
    pub(crate) persistence: AgentToolResultPersistence,
}

impl From<Value> for AgentToolExecutionValue {
    fn from(value: Value) -> Self {
        Self {
            value,
            persistence: AgentToolResultPersistence::RuntimeCommits,
        }
    }
}

/// True asynchronous tool execution boundary.
///
/// Existing built-ins continue implementing [`AgentTool`] and are dispatched through the blocking
/// adapter. Network/protocol-backed tools implement this subtrait so their futures run directly on
/// Tokio and receive the same ToolExecutionContext cancellation authority.
pub(crate) trait AsyncAgentTool: AgentTool {
    fn execute_async<'a>(
        &'a self,
        context: &'a ToolExecutionContext,
        args: Value,
    ) -> BoxAgentToolFuture<'a>;
}

enum AgentToolHandler {
    Blocking(Box<dyn AgentTool>),
    Async(Box<dyn AsyncAgentTool>),
}

impl AgentToolHandler {
    fn tool(&self) -> &dyn AgentTool {
        match self {
            Self::Blocking(tool) => tool.as_ref(),
            Self::Async(tool) => tool.as_ref(),
        }
    }

    fn async_tool(&self) -> Option<&dyn AsyncAgentTool> {
        match self {
            Self::Blocking(_) => None,
            Self::Async(tool) => Some(tool.as_ref()),
        }
    }

    fn definition(&self) -> AgentToolDefinition {
        self.tool().definition()
    }

    fn execute(&self, context: &ToolExecutionContext, args: Value) -> AgentResult<Value> {
        self.tool().execute(context, args)
    }

    fn permission_policy(&self) -> AgentToolPermissionPolicy {
        self.tool().permission_policy()
    }

    fn exposure(&self) -> AgentToolExposure {
        self.tool().exposure()
    }

    fn error_result(&self, error: &AgentError) -> Option<Value> {
        self.tool().error_result(error)
    }

    fn cancellation_settlement(&self) -> AgentToolCancellationSettlement {
        self.tool().cancellation_settlement()
    }

    #[cfg(test)]
    fn proposed_action(
        &self,
        context: &ToolExecutionContext,
        call: &AgentToolCall,
    ) -> AgentResult<AgentProposedAction> {
        self.tool().proposed_action(context, call)
    }

    fn proposed_action_async<'a>(
        &'a self,
        context: &'a ToolExecutionContext,
        call: &'a AgentToolCall,
    ) -> BoxAgentProposedActionFuture<'a> {
        self.tool().proposed_action_async(context, call)
    }

    fn invalidate_proposed_action(&self, action: &AgentProposedAction) -> AgentResult<()> {
        self.tool().invalidate_proposed_action(action)
    }

    fn requires_approval_for_call(&self, args: &Value) -> bool {
        self.tool().requires_approval_for_call(args)
    }

    fn auto_executes_prepared_action(&self) -> bool {
        self.tool().auto_executes_prepared_action()
    }

    fn input_stream_observer(
        &self,
        context: ToolExecutionContext,
    ) -> Option<Box<dyn ToolInputStreamObserver>> {
        self.tool().input_stream_observer(context)
    }

    fn trace_call_projection(&self, call: &AgentToolCall) -> AgentToolCall {
        self.tool().trace_call_projection(call)
    }

    fn event_call_projection(&self, call: &AgentToolCall) -> AgentToolCall {
        self.tool().event_call_projection(call)
    }

    fn model_call_projection(&self, call: &AgentToolCall) -> AgentToolCall {
        self.tool().model_call_projection(call)
    }

    fn checkpoint_call_projection(&self, call: &AgentToolCall) -> AgentToolCall {
        self.tool().checkpoint_call_projection(call)
    }

    fn trace_projection(&self, result: &AgentToolResult) -> AgentToolResult {
        self.tool().trace_projection(result)
    }

    fn archive_projection(&self, result: &AgentToolResult) -> AgentToolResult {
        self.tool().archive_projection(result)
    }

    fn archives_result(&self) -> bool {
        self.tool().archives_result()
    }

    fn model_projection(&self, result: &AgentToolResult) -> AgentToolResult {
        self.tool().model_projection(result)
    }

    fn event_projection(&self, result: &AgentToolResult) -> AgentToolResult {
        self.tool().event_projection(result)
    }

    fn checkpoint_projection(&self, result: &AgentToolResult) -> AgentToolResult {
        self.tool().checkpoint_projection(result)
    }
}

pub struct ToolRegistry {
    tools: BTreeMap<String, AgentToolHandler>,
    owners: BTreeMap<String, String>,
    exposures: BTreeMap<String, AgentToolExposure>,
    identities: BTreeMap<String, crate::protocol::AgentToolIdentity>,
    mcp_diagnostics: Vec<McpToolRegistrationDiagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AgentToolExposure {
    Stable,
    Dynamic,
    RequiresCapability(ToolCapabilityId),
}

/// Declares which host permission policy authorizes a tool's state-changing
/// calls. Keeping this on the tool implementation means future built-in and
/// extension tools opt into the common policy without adding their names to a
/// second, easily-stale allowlist in the runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AgentToolPermissionPolicy {
    Default,
    FileChange(FileChangeToolAccess),
}

/// Controls whether a file-writing tool still has useful read-only calls when
/// writes are disabled. `ReadWrite` tools stay visible, but their write calls
/// must still fail closed in proposal preparation and host execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FileChangeToolAccess {
    WriteOnly,
    ReadWrite,
}

/// Declares how runtime cancellation settles a blocking tool execution.
///
/// Most read-only tools can be interrupted immediately. Tools that may have crossed an external
/// side-effect or durable-commit boundary must instead finish their own cancellation protocol and
/// return the authoritative terminal receipt before the run is allowed to stop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AgentToolCancellationSettlement {
    Interruptible,
    Authoritative,
}

/// Whether an unexecuted model call may be retained across a durable approval boundary.
///
/// External MCP calls need a future authorization/Secret Store aware checkpoint format. Unknown
/// calls are denied too: there is no registered tool-owned projection that can make their
/// arguments safe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AgentToolCallCheckpointPersistence {
    Allowed,
    DeniedMcp,
    DeniedUnknown,
}

fn unknown_tool_call_projection(call: &AgentToolCall) -> AgentToolCall {
    AgentToolCall {
        id: call.id.clone(),
        tool: call.tool.clone(),
        args: Value::Object(serde_json::Map::new()),
        approval_status: call.approval_status,
        reason: None,
    }
}

impl AgentToolPermissionPolicy {
    pub(crate) fn uses_file_change_approval(self) -> bool {
        matches!(self, Self::FileChange(_))
    }

    pub(crate) fn is_available_when_write_denied(self) -> bool {
        !matches!(self, Self::FileChange(FileChangeToolAccess::WriteOnly))
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
        Self::defaults_with_search_office_and_image(search_config, office_engine, None)
    }

    pub(crate) fn defaults_with_search_office_and_image(
        search_config: Option<&AgentSearchConfig>,
        office_engine: Option<Arc<dyn crate::office::OfficeEngine>>,
        image_generation_execution: Option<
            Arc<crate::image_generation::ImageGenerationExecutionService>,
        >,
    ) -> Self {
        let mut registry = Self {
            tools: BTreeMap::new(),
            owners: BTreeMap::new(),
            exposures: BTreeMap::new(),
            identities: BTreeMap::new(),
            mcp_diagnostics: Vec::new(),
        };
        registry.register(AttachmentsListTool);
        registry.register(AttachmentsListProjectTool);
        registry.register(ReadFileTool);
        registry.register(ReadImageTool);
        registry.register(ReadWordTool);
        registry.register(ReadPresentationTool);
        registry.register(ReadSpreadsheetTool);
        if let Some(engine) = office_engine {
            registry.register(OfficeDocumentTool::new(engine.clone()));
            registry.register(OfficeSpreadsheetTool::new(engine.clone()));
            registry.register(OfficePresentationTool::new(engine));
        }
        if let Some(execution) = image_generation_execution {
            registry.register(ImageGenerationTool::new(execution));
        }
        registry.register(WorkspaceMapTool);
        registry.register(SearchFilesTool);
        registry.register(SearchCodeTool);
        if let Some(api_key) = tavily_api_key(search_config) {
            registry.register(WebSearchTool::new(api_key.clone()));
            registry.register(WebFetchTool::new(api_key));
        }
        registry.register(ApplyPatchTool);
        registry.register(RunCommandTool);
        registry.register_async(CommandSessionTool);
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

    pub(crate) fn effective_tool_set(
        &self,
        permitted_definitions: impl IntoIterator<Item = AgentToolDefinition>,
        active_capabilities: &std::collections::BTreeSet<ToolCapabilityId>,
    ) -> AgentResult<EffectiveToolSet> {
        EffectiveToolSet::from_permitted_definitions(
            self,
            permitted_definitions,
            active_capabilities,
        )
    }

    pub(crate) fn exposure(&self, tool_name: &str) -> Option<&AgentToolExposure> {
        self.exposures.get(tool_name)
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

    pub(crate) fn auto_executes_prepared_action(&self, tool_name: &str) -> bool {
        self.tools
            .get(tool_name)
            .is_some_and(AgentToolHandler::auto_executes_prepared_action)
    }

    pub(crate) fn permission_policy(&self, tool_name: &str) -> AgentToolPermissionPolicy {
        self.tools
            .get(tool_name)
            .map(|tool| tool.permission_policy())
            .unwrap_or(AgentToolPermissionPolicy::Default)
    }

    pub(crate) fn cancellation_settlement(
        &self,
        tool_name: &str,
    ) -> AgentToolCancellationSettlement {
        self.tools
            .get(tool_name)
            .map(|tool| tool.cancellation_settlement())
            .unwrap_or(AgentToolCancellationSettlement::Interruptible)
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

    /// Produces the canonical clone stored in the durable conversation trace.
    /// Execution and approval retain the original model call. A frozen action may bind a separate
    /// digest of this body-free clone when durable settlement must prove Trace identity, but the
    /// projection by itself never grants execution authority.
    pub(crate) fn trace_call_projection(&self, call: &AgentToolCall) -> AgentToolCall {
        self.tools
            .get(&call.tool)
            .map(|tool| tool.trace_call_projection(call))
            .unwrap_or_else(|| unknown_tool_call_projection(call))
    }

    /// Produces the presentation-safe clone emitted to event consumers. This
    /// is deliberately separate from durable trace storage so UI redaction
    /// cannot silently alter the model's historical context.
    pub(crate) fn event_call_projection(&self, call: &AgentToolCall) -> AgentToolCall {
        self.tools
            .get(&call.tool)
            .map(|tool| tool.event_call_projection(call))
            .unwrap_or_else(|| unknown_tool_call_projection(call))
    }

    /// Produces the clone retained in the current in-memory model timeline.
    ///
    /// This projection is ephemeral and must not be reused as a persistence projection.
    pub(crate) fn model_call_projection(&self, call: &AgentToolCall) -> AgentToolCall {
        self.tools
            .get(&call.tool)
            .map(|tool| tool.model_call_projection(call))
            .unwrap_or_else(|| unknown_tool_call_projection(call))
    }

    /// Produces the durable-safe clone retained in resumable checkpoints and
    /// persisted model-context history.
    pub(crate) fn checkpoint_call_projection(&self, call: &AgentToolCall) -> AgentToolCall {
        self.tools
            .get(&call.tool)
            .map(|tool| tool.checkpoint_call_projection(call))
            .unwrap_or_else(|| unknown_tool_call_projection(call))
    }

    pub(crate) fn contains_tool(&self, tool_name: &str) -> bool {
        self.tools.contains_key(tool_name)
    }

    pub(crate) fn identity(&self, tool_name: &str) -> Option<&AgentToolIdentity> {
        self.identities.get(tool_name)
    }

    /// Whether this exact registered Tool identity belongs to an external MCP Server.
    ///
    /// Provider-visible names are deliberately not inspected here. A trusted built-in or runtime
    /// extension may legally use an `mcp__...`-looking name, while an MCP Tool may use any
    /// provider-safe model name chosen by the catalog adapter.
    pub(crate) fn is_mcp_tool(&self, tool_name: &str) -> bool {
        matches!(
            self.identities.get(tool_name),
            Some(AgentToolIdentity::Mcp { .. })
        )
    }

    /// Removes external MCP definitions from the Renderer event projection.
    ///
    /// The caller retains its original definitions for provider payloads, checkpoint validation,
    /// and execution authorization. MCP catalog details have a separate bounded management
    /// contract; publishing their untrusted descriptions or schemas through generic Agent events
    /// would create a second, less constrained Renderer surface.
    pub(crate) fn renderer_event_definitions(
        &self,
        definitions: &[AgentToolDefinition],
    ) -> Vec<AgentToolDefinition> {
        definitions
            .iter()
            .filter(|definition| !self.is_mcp_tool(&definition.name))
            .cloned()
            .collect()
    }

    pub(crate) fn checkpoint_persistence(
        &self,
        tool_name: &str,
    ) -> AgentToolCallCheckpointPersistence {
        match self.identities.get(tool_name) {
            Some(AgentToolIdentity::Mcp { .. }) => AgentToolCallCheckpointPersistence::DeniedMcp,
            Some(_) => AgentToolCallCheckpointPersistence::Allowed,
            None => AgentToolCallCheckpointPersistence::DeniedUnknown,
        }
    }

    pub(crate) fn contains_tool_prefix(&self, tool_name: &str) -> bool {
        self.tools.keys().any(|name| name.starts_with(tool_name))
    }

    #[cfg(test)]
    pub fn proposed_action(
        &self,
        context: &ToolExecutionContext,
        call: &AgentToolCall,
    ) -> AgentResult<AgentProposedAction> {
        let Some(tool) = self.tools.get(&call.tool) else {
            return Err(AgentError::new(format!("未知工具：{}", call.tool)));
        };

        // Proposal construction can freeze process-only payloads just like execution. Bind the
        // exact model call identity at this single Registry boundary so every runtime call site
        // gets the same call_id and cannot accidentally approve an unbound invocation.
        let call_context = context.clone().with_tool_call_id(call.id.clone());
        tool.proposed_action(&call_context, call)
    }

    pub async fn proposed_action_async(
        &self,
        context: &ToolExecutionContext,
        call: &AgentToolCall,
    ) -> AgentResult<AgentProposedAction> {
        let Some(tool) = self.tools.get(&call.tool) else {
            return Err(AgentError::new(format!("未知工具：{}", call.tool)));
        };
        let call_context = context.clone().with_tool_call_id(call.id.clone());
        tool.proposed_action_async(&call_context, call).await
    }

    /// Releases Host resources prepared while constructing an action that never reached a durable
    /// pending boundary.
    pub(crate) fn invalidate_proposed_action(
        &self,
        action: &AgentProposedAction,
    ) -> AgentResult<()> {
        let tool_name = match action {
            AgentProposedAction::McpToolCall { approval } => {
                approval.identity.provenance.model_tool_name.as_str()
            }
            AgentProposedAction::ToolCall { call } => call.tool.as_str(),
            AgentProposedAction::BuiltinCapabilityActivation { .. } => "activate_capability",
            AgentProposedAction::BuiltinMcpToolApproval { approval } => {
                approval.identity.model_name.as_str()
            }
            AgentProposedAction::BrowserRiskApproval { approval } => {
                approval.trigger_tool_name.as_str()
            }
            AgentProposedAction::FileChange { .. } => "apply_patch",
            AgentProposedAction::Command { .. } => "run_command",
            AgentProposedAction::SkillMaterialization { .. } => "skills_materialize_resource",
            AgentProposedAction::SkillScript { .. } => "skills_run_script",
            AgentProposedAction::SkillInstallation { .. } => "skills_commit_install",
            AgentProposedAction::OfficeOperation { office_operation } => {
                match office_operation.prepared.request.document_kind {
                    crate::office::OfficeDocumentKind::Document => "office_document",
                    crate::office::OfficeDocumentKind::Spreadsheet => "office_spreadsheet",
                    crate::office::OfficeDocumentKind::Presentation => "office_presentation",
                }
            }
        };
        let Some(tool) = self.tools.get(tool_name) else {
            return Err(AgentError::new(format!(
                "无法释放未知工具 `{tool_name}` 的待审批资源。"
            )));
        };
        tool.invalidate_proposed_action(action)
    }

    pub fn execute(&self, context: &ToolExecutionContext, call: &AgentToolCall) -> AgentToolResult {
        if let Err(error) = context.check_cancelled() {
            return AgentToolResult {
                exact_archive_file: None,
                call_id: call.id.clone(),
                tool: call.tool.clone(),
                ok: false,
                result: None,
                error: Some(error.to_string()),
            };
        }

        let Some(tool) = self.tools.get(&call.tool) else {
            return AgentToolResult {
                exact_archive_file: None,
                call_id: call.id.clone(),
                tool: call.tool.clone(),
                ok: false,
                result: None,
                error: Some(format!("未知工具：{}", call.tool)),
            };
        };

        let call_context = context.clone().with_tool_call_id(call.id.clone());
        let cancellation_settlement = tool.cancellation_settlement();
        match tool.execute(&call_context, call.args.clone()) {
            Ok(result) => match (cancellation_settlement, context.check_cancelled()) {
                (AgentToolCancellationSettlement::Authoritative, _) | (_, Ok(())) => {
                    AgentToolResult {
                        exact_archive_file: None,
                        call_id: call.id.clone(),
                        tool: call.tool.clone(),
                        ok: true,
                        result: Some(result),
                        error: None,
                    }
                }
                (_, Err(error)) => AgentToolResult {
                    exact_archive_file: None,
                    call_id: call.id.clone(),
                    tool: call.tool.clone(),
                    ok: false,
                    result: None,
                    error: Some(error.to_string()),
                },
            },
            Err(error) => AgentToolResult {
                exact_archive_file: None,
                call_id: call.id.clone(),
                tool: call.tool.clone(),
                ok: false,
                result: tool.error_result(&error),
                error: Some(error.to_string()),
            },
        }
    }

    /// Executes a registered Tool through the common runtime boundary.
    ///
    /// Existing synchronous built-ins retain their implementation and are isolated on Tokio's
    /// blocking pool. Protocol-backed Tools are awaited directly so cancellation and transport
    /// progress do not require a nested runtime or an unnecessary blocking worker.
    pub(crate) async fn execute_async(
        self: Arc<Self>,
        context: ToolExecutionContext,
        call: AgentToolCall,
        cancellation_token: crate::AgentCancellationToken,
    ) -> AgentResult<RegisteredToolExecution> {
        if context.check_cancelled().is_err() {
            return Err(AgentError::cancelled());
        }

        let Some(tool) = self.tools.get(&call.tool) else {
            return Ok(RegisteredToolExecution {
                result: failed_tool_result(&call, None, format!("未知工具：{}", call.tool)),
                persistence: AgentToolResultPersistence::RuntimeCommits,
            });
        };

        if let Some(async_tool) = tool.async_tool() {
            let call_context = context.clone().with_tool_call_id(call.id.clone());
            let settlement = tool.cancellation_settlement();
            let execution = async_tool.execute_async(&call_context, call.args.clone());
            tokio::pin!(execution);
            let execution = match settlement {
                AgentToolCancellationSettlement::Interruptible => tokio::select! {
                    biased;
                    _ = cancellation_token.cancelled() => return Err(AgentError::cancelled()),
                    result = &mut execution => result,
                },
                AgentToolCancellationSettlement::Authoritative => execution.await,
            };
            return match execution {
                Err(error) if error.is_cancelled() => Err(error),
                Err(error) => Ok(RegisteredToolExecution {
                    result: failed_tool_result(&call, tool.error_result(&error), error.to_string()),
                    persistence: AgentToolResultPersistence::RuntimeCommits,
                }),
                Ok(_)
                    if settlement == AgentToolCancellationSettlement::Interruptible
                        && cancellation_token.is_cancelled() =>
                {
                    Err(AgentError::cancelled())
                }
                Ok(execution) => Ok(RegisteredToolExecution {
                    result: successful_tool_result(&call, execution.value),
                    persistence: execution.persistence,
                }),
            };
        }

        let settlement = tool.cancellation_settlement();
        let handle = tokio::task::spawn_blocking(move || self.execute(&context, &call));
        match settlement {
            AgentToolCancellationSettlement::Interruptible => tokio::select! {
                _ = cancellation_token.cancelled() => Err(AgentError::cancelled()),
                result = handle => {
                    result
                        .map_err(|error| AgentError::new(format!("工具执行线程失败：{error}")))
                        .map(|result| RegisteredToolExecution {
                            result,
                            persistence: AgentToolResultPersistence::RuntimeCommits,
                        })
                }
            },
            AgentToolCancellationSettlement::Authoritative => handle
                .await
                .map_err(|error| AgentError::new(format!("工具执行线程失败：{error}")))
                .map(|result| RegisteredToolExecution {
                    result,
                    persistence: AgentToolResultPersistence::RuntimeCommits,
                }),
        }
    }

    /// Returns the tool-owned pre-projection for durable history.
    ///
    /// Tools can remove intrinsically non-durable payloads here. The conversation trace recorder
    /// applies the centralized size/tool policy afterward; current-turn model, checkpoint, and
    /// event projections do not inherit those durable limits.
    pub(crate) fn trace_projection(&self, result: &AgentToolResult) -> AgentToolResult {
        self.tools
            .get(&result.tool)
            .map(|tool| tool.trace_projection(result))
            .unwrap_or_else(|| canonical_tool_result_for_context(result))
    }

    /// Returns the security-sanitized, non-length-bounded textual result stored in exact history.
    ///
    /// This projection is evaluated before the bounded durable trace projection. Binary/runtime
    /// delivery fields may still be omitted by the owning tool.
    pub(crate) fn archive_projection(&self, result: &AgentToolResult) -> AgentToolResult {
        self.tools
            .get(&result.tool)
            .map(|tool| tool.archive_projection(result))
            .unwrap_or_else(|| canonical_tool_result_for_context(result))
    }

    pub(crate) fn archives_result(&self, tool_name: &str) -> bool {
        self.tools
            .get(tool_name)
            .is_none_or(|tool| tool.archives_result())
    }

    /// Returns the textual projection supplied to the current model tool-result message.
    ///
    /// This is intentionally distinct from the raw result: a tool may carry transient binary
    /// delivery data that is converted into a provider-native multimodal message instead of being
    /// serialized into tool-result text.
    pub(crate) fn model_projection(&self, result: &AgentToolResult) -> AgentToolResult {
        self.tools
            .get(&result.tool)
            .map(|tool| tool.model_projection(result))
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
        let tool_name = tool.definition().name;
        self.register_handler(
            format!("extension:{extension_id}"),
            AgentToolIdentity::RuntimeExtension {
                extension_id: extension_id.to_string(),
                tool_name,
            },
            AgentToolHandler::Blocking(tool),
        )
    }

    pub(crate) fn register_builtin_capability_tool(
        &mut self,
        tool: BuiltinCapabilityAgentTool,
    ) -> AgentResult<()> {
        let capability_id = tool.capability_id().as_str().to_string();
        let managed_mcp_id = tool.managed_mcp_id().to_string();
        let package_name = tool.package_name().to_string();
        let package_version = tool.package_version().to_string();
        let upstream_catalog_digest = tool.upstream_catalog_digest().to_string();
        let policy_digest = tool.policy_digest().to_string();
        let manifest_digest = tool.manifest_digest().to_string();
        let tool_id = tool.tool_id().to_string();
        let raw_name = tool.raw_name().to_string();
        let model_name = tool.model_name().to_string();
        let upstream_schema_digest = tool.upstream_schema_digest().to_string();
        let host_overlay_digest = tool.host_overlay_digest().to_string();
        let host_input_schema_digest = tool.host_input_schema_digest().to_string();
        self.register_handler(
            format!("builtin-capability:{capability_id}"),
            AgentToolIdentity::BuiltinCapability {
                capability_id: capability_id.into_boxed_str(),
                managed_mcp_id: managed_mcp_id.into_boxed_str(),
                package_name: package_name.into_boxed_str(),
                package_version: package_version.into_boxed_str(),
                upstream_catalog_digest: upstream_catalog_digest.into_boxed_str(),
                policy_digest: policy_digest.into_boxed_str(),
                manifest_digest: manifest_digest.into_boxed_str(),
                tool_id: tool_id.into_boxed_str(),
                raw_name: raw_name.into_boxed_str(),
                model_name: model_name.into_boxed_str(),
                upstream_schema_digest: upstream_schema_digest.into_boxed_str(),
                host_overlay_digest: host_overlay_digest.into_boxed_str(),
                host_input_schema_digest: host_input_schema_digest.into_boxed_str(),
            },
            AgentToolHandler::Async(Box::new(tool)),
        )
    }

    pub(crate) fn register_mcp_runtime(&mut self, runtime: &McpToolRuntime) {
        let mut diagnostics = runtime.initial_diagnostics().to_vec();
        let invoker = runtime.invoker();
        let caller = runtime.catalog_context().clone();
        for descriptor in runtime.tools().iter().cloned() {
            let descriptor_provenance = descriptor.provenance.clone();
            let tool = match McpAgentTool::prepare(descriptor, Arc::clone(&invoker), caller.clone())
            {
                Ok(tool) => tool,
                Err(diagnostic) => {
                    diagnostics.push(diagnostic);
                    continue;
                }
            };
            let provenance = tool.provenance().clone();
            if self
                .register_handler(
                    format!("mcp:{}", provenance.server_id),
                    AgentToolIdentity::Mcp {
                        provenance: provenance.clone(),
                    },
                    AgentToolHandler::Async(Box::new(tool)),
                )
                .is_err()
            {
                diagnostics.push(McpToolRegistrationDiagnostic::name_collision(
                    &descriptor_provenance,
                ));
            }
        }
        self.mcp_diagnostics = diagnostics;
        runtime.invoker().report_diagnostics(&self.mcp_diagnostics);
    }

    #[cfg(test)]
    pub(crate) fn mcp_diagnostics(&self) -> &[McpToolRegistrationDiagnostic] {
        &self.mcp_diagnostics
    }

    pub(crate) fn register_conversation_history(&mut self) {
        if !self.contains_tool("conversation_history") {
            self.register(ConversationHistoryTool);
        }
    }

    pub(crate) fn register_agent_collaboration_tools(&mut self) {
        for kind in [
            AgentCollaborationToolKind::Spawn,
            AgentCollaborationToolKind::SendMessage,
            AgentCollaborationToolKind::FollowupTask,
            AgentCollaborationToolKind::Wait,
            AgentCollaborationToolKind::List,
            AgentCollaborationToolKind::Interrupt,
        ] {
            let tool = AgentCollaborationTool::new(kind);
            if !self.contains_tool(&tool.definition().name) {
                self.register_async(tool);
            }
        }
    }

    pub(crate) fn register_skill_installation_prepare(
        &mut self,
        executor: Arc<dyn AgentSkillInstallationPrepareExecutor>,
    ) {
        if !self.contains_tool("skills_prepare_install") {
            self.register(SkillsPrepareInstallTool::new(executor));
        }
    }

    pub(crate) fn register_skill_installation_commit(
        &mut self,
        preparer: Arc<dyn AgentSkillInstallationCommitPreparer>,
    ) {
        if !self.contains_tool("skills_commit_install") {
            self.register(SkillsCommitInstallTool::new(preparer));
        }
    }

    pub(crate) fn register_automation_report(&mut self, sink: Arc<dyn AutomationReportSink>) {
        if !self.contains_tool("automation_report") {
            self.register(AutomationReportTool::new(sink));
        }
    }

    fn register<T: AgentTool + 'static>(&mut self, tool: T) {
        self.register_boxed("core".to_string(), Box::new(tool))
            .expect("core tool definitions must have valid names and portable input schemas");
    }

    fn register_async<T: AsyncAgentTool + 'static>(&mut self, tool: T) {
        let tool_name = tool.definition().name;
        self.register_handler(
            "core".to_string(),
            AgentToolIdentity::Builtin { tool_name },
            AgentToolHandler::Async(Box::new(tool)),
        )
        .expect("core async tool definitions must have valid names and portable input schemas");
    }

    fn register_boxed(&mut self, owner: String, tool: Box<dyn AgentTool>) -> AgentResult<()> {
        let tool_name = tool.definition().name;
        self.register_handler(
            owner,
            AgentToolIdentity::Builtin { tool_name },
            AgentToolHandler::Blocking(tool),
        )
    }

    fn register_handler(
        &mut self,
        owner: String,
        identity: AgentToolIdentity,
        tool: AgentToolHandler,
    ) -> AgentResult<()> {
        let definition = tool.definition();
        let exposure = tool.exposure();
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
        self.owners.insert(name.clone(), owner);
        self.exposures.insert(name.clone(), exposure);
        self.identities.insert(name, identity);
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn empty() -> Self {
        Self {
            tools: BTreeMap::new(),
            owners: BTreeMap::new(),
            exposures: BTreeMap::new(),
            identities: BTreeMap::new(),
            mcp_diagnostics: Vec::new(),
        }
    }

    #[cfg(test)]
    pub(crate) fn register_test_tool<T: AgentTool + 'static>(&mut self, tool: T) {
        self.register(tool);
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

fn successful_tool_result(call: &AgentToolCall, result: Value) -> AgentToolResult {
    AgentToolResult {
        exact_archive_file: None,
        call_id: call.id.clone(),
        tool: call.tool.clone(),
        ok: true,
        result: Some(result),
        error: None,
    }
}

fn failed_tool_result(
    call: &AgentToolCall,
    result: Option<Value>,
    error: String,
) -> AgentToolResult {
    AgentToolResult {
        exact_archive_file: None,
        call_id: call.id.clone(),
        tool: call.tool.clone(),
        ok: false,
        result,
        error: Some(error),
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

pub(crate) trait AgentTool: Send + Sync {
    fn definition(&self) -> AgentToolDefinition;
    fn execute(&self, context: &ToolExecutionContext, args: Value) -> AgentResult<Value>;
    fn permission_policy(&self) -> AgentToolPermissionPolicy;
    fn exposure(&self) -> AgentToolExposure;

    /// Projects a structured execution error into the canonical ToolResult payload.
    ///
    /// Most tools use the shared diagnostic envelope. A tool with its own closed terminal result
    /// schema may override this hook so generic metadata cannot invalidate that schema.
    fn error_result(&self, error: &AgentError) -> Option<Value> {
        structured_error_result(error)
    }

    fn cancellation_settlement(&self) -> AgentToolCancellationSettlement {
        AgentToolCancellationSettlement::Interruptible
    }

    fn proposed_action(
        &self,
        _context: &ToolExecutionContext,
        call: &AgentToolCall,
    ) -> AgentResult<AgentProposedAction> {
        Ok(AgentProposedAction::ToolCall { call: call.clone() })
    }

    fn proposed_action_async<'a>(
        &'a self,
        context: &'a ToolExecutionContext,
        call: &'a AgentToolCall,
    ) -> BoxAgentProposedActionFuture<'a> {
        Box::pin(async move { self.proposed_action(context, call) })
    }

    fn invalidate_proposed_action(&self, _action: &AgentProposedAction) -> AgentResult<()> {
        Ok(())
    }

    fn requires_approval_for_call(&self, _args: &Value) -> bool {
        self.definition().requires_approval
    }

    /// Whether this Tool must first freeze a typed action even though Host policy skips prompting.
    ///
    /// This is intentionally distinct from `requires_approval_for_call`: automatic MCP calls still
    /// need one-time payload preparation and exact Host revalidation.
    fn auto_executes_prepared_action(&self) -> bool {
        false
    }

    fn input_stream_observer(
        &self,
        _context: ToolExecutionContext,
    ) -> Option<Box<dyn ToolInputStreamObserver>> {
        None
    }

    /// Returns the canonical clone stored in the durable conversation trace.
    /// Implementations may normalize unsafe display-only fields, but must never
    /// use this hook for execution, permission, or approval decisions.
    fn trace_call_projection(&self, call: &AgentToolCall) -> AgentToolCall {
        call.clone()
    }

    /// Returns the presentation-safe clone emitted to event consumers. By
    /// default it shares the trace representation; tools with private event
    /// payloads can redact only this projection without changing history.
    fn event_call_projection(&self, call: &AgentToolCall) -> AgentToolCall {
        self.trace_call_projection(call)
    }

    /// Returns the clone retained in the current in-memory model context.
    ///
    /// The live model must be able to inspect the arguments it generated when a
    /// Tool returns a correctable error. Durable storage uses the separate
    /// checkpoint projection below.
    fn model_call_projection(&self, call: &AgentToolCall) -> AgentToolCall {
        call.clone()
    }

    /// Returns the durable-safe clone retained in checkpoints and persisted
    /// model-context history.
    ///
    /// By default this matches the live projection. Tools whose arguments
    /// cannot be persisted must override this hook explicitly.
    fn checkpoint_call_projection(&self, call: &AgentToolCall) -> AgentToolCall {
        call.clone()
    }

    fn trace_projection(&self, result: &AgentToolResult) -> AgentToolResult {
        canonical_tool_result_for_context(result)
    }

    fn archive_projection(&self, result: &AgentToolResult) -> AgentToolResult {
        canonical_tool_result_for_context(result)
    }

    fn archives_result(&self) -> bool {
        true
    }

    fn model_projection(&self, result: &AgentToolResult) -> AgentToolResult {
        canonical_tool_result_for_context(result)
    }

    fn event_projection(&self, result: &AgentToolResult) -> AgentToolResult {
        self.trace_projection(result)
    }

    fn checkpoint_projection(&self, result: &AgentToolResult) -> AgentToolResult {
        canonical_tool_result_for_context(result)
    }
}

pub(crate) struct ToolInputStreamChunk<'a> {
    pub stream_id: &'a str,
    pub attempt: usize,
    pub tool_call_index: usize,
    pub input_delta: &'a str,
    pub received_bytes: u64,
}

pub(crate) enum ToolInputStreamPreview {
    FileChange(AgentFileChangePreview),
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
        AgentApprovalStatus, AgentAttachmentLibraryContext, AgentAttachmentReference,
        AgentInputAttachmentKind, AgentRunContext, AgentSearchConfig, AgentSearchMode,
        AgentToolCall, AgentToolSafety, AgentWorkspaceContext, ModelCapabilities,
    };
    use image::ImageEncoder;
    use serde_json::json;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_WORKSPACE_COUNTER: AtomicU64 = AtomicU64::new(1);

    fn office_tool_args(request: Value, reason: Option<Value>) -> Value {
        let mut args = request;
        if let Some(reason) = reason {
            args["reason"] = reason;
        }
        args
    }

    fn valid_test_png() -> Vec<u8> {
        let mut bytes = Vec::new();
        image::codecs::png::PngEncoder::new(&mut bytes)
            .write_image(
                &[0x20, 0x40, 0x80, 0xff],
                1,
                1,
                image::ExtendedColorType::Rgba8,
            )
            .unwrap();
        bytes
    }

    struct InvalidSchemaTool;

    impl AgentTool for InvalidSchemaTool {
        fn exposure(&self) -> AgentToolExposure {
            AgentToolExposure::Stable
        }

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

    /// Exercises the contract used by runtime extensions and MCP-style adapters: the provider
    /// payload remains canonical for Exact Archive while the common model gate owns length
    /// control and must retain opaque provider continuation capabilities.
    struct ProviderBackedExtensionTool;

    impl AgentTool for ProviderBackedExtensionTool {
        fn exposure(&self) -> AgentToolExposure {
            AgentToolExposure::Stable
        }

        fn permission_policy(&self) -> AgentToolPermissionPolicy {
            AgentToolPermissionPolicy::Default
        }

        fn definition(&self) -> AgentToolDefinition {
            AgentToolDefinition {
                name: "provider_backed_extension".to_string(),
                description: "test".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {},
                    "additionalProperties": false
                }),
                safety: AgentToolSafety::ReadOnly,
                requires_workspace: false,
                requires_approval: false,
                approval_mode: crate::protocol::AgentToolApprovalMode::Never,
            }
        }

        fn execute(&self, _context: &ToolExecutionContext, _args: Value) -> AgentResult<Value> {
            Ok(json!({
                "content": "provider payload ".repeat(20_000),
                "sourceCompleteness": "unknown",
                "cursor": "provider-cursor-1",
                "next": "provider-cursor-2",
                "continueWith": {
                    "provider": "example",
                    "cursor": "provider-cursor-1"
                }
            }))
        }
    }

    struct ProposalCallIdentityTool;

    impl AgentTool for ProposalCallIdentityTool {
        fn exposure(&self) -> AgentToolExposure {
            AgentToolExposure::Stable
        }

        fn permission_policy(&self) -> AgentToolPermissionPolicy {
            AgentToolPermissionPolicy::Default
        }

        fn definition(&self) -> AgentToolDefinition {
            AgentToolDefinition {
                name: "proposal_call_identity".to_string(),
                description: "test".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {},
                    "additionalProperties": false
                }),
                safety: AgentToolSafety::ReadOnly,
                requires_workspace: false,
                requires_approval: true,
                approval_mode: crate::protocol::AgentToolApprovalMode::Always,
            }
        }

        fn execute(&self, _context: &ToolExecutionContext, _args: Value) -> AgentResult<Value> {
            Ok(json!({}))
        }

        fn proposed_action(
            &self,
            context: &ToolExecutionContext,
            call: &AgentToolCall,
        ) -> AgentResult<AgentProposedAction> {
            if context.tool_call_id()? != call.id {
                return Err(AgentError::new("proposal call identity was not bound"));
            }
            Ok(AgentProposedAction::ToolCall { call: call.clone() })
        }
    }

    #[test]
    fn proposed_action_registry_boundary_binds_the_exact_tool_call_id() {
        let mut registry = ToolRegistry::empty();
        registry.register_test_tool(ProposalCallIdentityTool);
        let context = ToolExecutionContext::from_run_context(None)
            .with_runtime_services("proposal-run".to_string(), None);
        let call = AgentToolCall {
            id: "proposal-call-id".to_string(),
            tool: "proposal_call_identity".to_string(),
            args: json!({}),
            approval_status: crate::protocol::AgentApprovalStatus::Required,
            reason: None,
        };
        let action = registry.proposed_action(&context, &call).unwrap();
        assert!(matches!(
            action,
            AgentProposedAction::ToolCall { call: proposed } if proposed.id == call.id
        ));
    }

    #[test]
    fn rejects_paths_outside_workspace() {
        let fixture = TestWorkspace::new();
        let context = fixture.context();
        let error = context.resolve_existing_path("../outside.txt").unwrap_err();

        assert!(error.to_string().contains("路径不能包含"));
    }

    #[test]
    fn resolves_dot_as_workspace_root() {
        let fixture = TestWorkspace::new();
        let context = fixture.context();

        assert_eq!(
            context.resolve_existing_path(".").unwrap(),
            fixture.root.canonicalize().unwrap()
        );
        assert_eq!(
            context.resolve_existing_path("./").unwrap(),
            fixture.root.canonicalize().unwrap()
        );
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
    fn declarative_file_input_consumers_expose_one_model_friendly_path() {
        let engine =
            crate::office::resolve_office_engine(&crate::office::OfficeCliDiscoveryOptions::new());
        let registry = ToolRegistry::defaults_with_search_and_office(None, Some(engine));
        let command_inputs = &registry.definition_for("run_command").unwrap().input_schema
            ["properties"]["inputs"]["items"];
        assert_eq!(command_inputs["required"], json!(["path"]));
        assert!(command_inputs["properties"]["path"].is_object());
        assert!(command_inputs["properties"]["mountPath"].is_object());
        assert!(command_inputs["properties"].get("source").is_none());
        for tool_name in [
            "office_document",
            "office_spreadsheet",
            "office_presentation",
        ] {
            let definition = registry.definition_for(tool_name).unwrap();
            let properties = definition.input_schema["properties"].as_object().unwrap();
            assert!(properties["filePath"].is_object());
            assert!(
                !properties.contains_key("imagePath"),
                "{tool_name} exposed a retired model-side write input"
            );
            assert!(
                !properties.contains_key("source"),
                "{tool_name} exposed internal file-source routing"
            );
        }
    }

    #[test]
    fn read_image_exposes_one_required_model_friendly_path() {
        let registry = ToolRegistry::defaults_with_search(None);
        let schema = &registry.definition_for("read_image").unwrap().input_schema;
        let properties = schema["properties"].as_object().unwrap();

        assert_eq!(schema["type"], "object");
        assert_eq!(schema["required"], json!(["path"]));
        assert_eq!(schema["additionalProperties"], false);
        assert_eq!(properties.len(), 1);
        assert_eq!(properties["path"]["type"], "string");
        assert_eq!(properties["path"]["minLength"], 1);
        assert!(!properties.contains_key("source"));
        assert!(!properties.contains_key("filePath"));
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
    fn provider_backed_extension_keeps_exact_payload_and_opaque_recovery_through_10k_gate() {
        let mut registry = ToolRegistry::defaults_with_search(None);
        registry
            .register_extension_tool("provider-test", Box::new(ProviderBackedExtensionTool))
            .unwrap();
        assert!(!registry.is_mcp_tool("provider_backed_extension"));
        assert!(registry
            .renderer_event_definitions(&registry.definitions())
            .iter()
            .any(|definition| definition.name == "provider_backed_extension"));
        let call = AgentToolCall {
            id: "provider-call".to_string(),
            tool: "provider_backed_extension".to_string(),
            args: json!({}),
            approval_status: crate::protocol::AgentApprovalStatus::NotRequired,
            reason: None,
        };

        let raw = registry.execute(&ToolExecutionContext::from_run_context(None), &call);
        assert!(registry.archives_result(&call.tool));
        let archive = registry.archive_projection(&raw);
        assert_eq!(archive.result, raw.result);
        assert_eq!(
            archive.result.as_ref().unwrap()["cursor"],
            "provider-cursor-1"
        );
        assert_eq!(
            archive.result.as_ref().unwrap()["next"],
            "provider-cursor-2"
        );
        assert_eq!(
            archive.result.as_ref().unwrap()["continueWith"]["cursor"],
            "provider-cursor-1"
        );
        assert_eq!(
            archive.result.as_ref().unwrap()["sourceCompleteness"],
            "unknown"
        );
        assert!(archive.result.as_ref().unwrap()["truncatedAtSource"].is_null());

        let model = registry.model_projection(&raw);
        let gate = crate::context::ContextCapacityDetector::for_model(
            "test-model",
            crate::protocol::AgentApiStyle::OpenAiCompatible,
            &[],
        )
        .model_tool_result_gate();
        let observation = crate::runtime::finalize_model_tool_observation(
            &gate,
            &call.id,
            false,
            &model,
            &Default::default(),
        )
        .unwrap();
        let projected: Value = serde_json::from_str(&observation).unwrap();

        assert_eq!(projected["truncated"], true);
        assert_eq!(projected["cursor"], "provider-cursor-1");
        assert_eq!(projected["next"], "provider-cursor-2");
        assert_eq!(projected["continueWith"]["cursor"], "provider-cursor-1");
        assert_eq!(projected["sourceCompleteness"], "unknown");
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
            assert_eq!(
                definition.input_schema["properties"]["reason"]["minLength"],
                1
            );
            assert_eq!(
                definition.input_schema["properties"]["reason"]["maxLength"],
                crate::AGENT_OFFICE_REASON_MAX_CHARS
            );
            assert!(definition.input_schema["required"]
                .as_array()
                .is_some_and(|required| required.contains(&json!("reason"))));
            let inspect_args = office_tool_args(
                json!({ "operation": "inspect", "filePath": document_path }),
                Some(json!("Inspect document structure")),
            );
            assert!(!registry.requires_approval_for_call(tool_name, &inspect_args));
            assert!(!registry.requires_approval_for_call(
                tool_name,
                &office_tool_args(
                    json!({ "operation": "inspect", "filePath": document_path, "depth": 2 }),
                    Some(json!("Inspect document details")),
                )
            ));
            let create_args = office_tool_args(
                json!({ "operation": "create", "filePath": document_path }),
                Some(json!("Create the Office file")),
            );
            assert!(
                registry.requires_approval_for_call(tool_name, &create_args),
                "{tool_name} write operations must use the file-edit approval path"
            );
            assert!(registry.requires_approval_for_call(
                tool_name,
                &office_tool_args(
                    json!({
                        "operation": "render",
                        "filePath": document_path,
                        "outputPath": "preview.html"
                    }),
                    Some(json!("Render an HTML preview")),
                )
            ));
            assert!(registry.requires_approval_for_call(
                tool_name,
                &office_tool_args(
                    json!({ "operation": "unsupported" }),
                    Some(json!("Try an unsupported operation")),
                )
            ));
            for invalid in [
                office_tool_args(json!({ "operation": "status" }), None),
                office_tool_args(json!({ "operation": "status" }), Some(json!("   "))),
                office_tool_args(
                    json!({ "operation": "status" }),
                    Some(json!("x".repeat(crate::AGENT_OFFICE_REASON_MAX_CHARS + 1))),
                ),
                office_tool_args(
                    json!({ "operation": "status", "filePath": document_path }),
                    Some(json!("Check Office engine status")),
                ),
                office_tool_args(
                    json!({ "operation": "inspect" }),
                    Some(json!("Inspect the document")),
                ),
                office_tool_args(
                    json!({ "operation": "validate", "filePath": document_path, "arguments": ["--output", "stolen.xlsx"] }),
                    Some(json!("Validate the document")),
                ),
                office_tool_args(
                    json!({ "operation": "validate", "filePath": document_path, "outputPath": "preview.html" }),
                    Some(json!("Validate the document")),
                ),
                office_tool_args(
                    json!({ "operation": "validate", "filePath": document_path, "access": "readOnly" }),
                    Some(json!("Validate the document")),
                ),
                office_tool_args(
                    json!({ "operation": "validate", "filePath": "wrong-extension.bin" }),
                    Some(json!("Validate the document")),
                ),
            ] {
                assert!(
                    registry.requires_approval_for_call(tool_name, &invalid),
                    "invalid Office calls must fail closed during approval routing"
                );
            }
            assert_eq!(
                registry.permission_policy(tool_name),
                AgentToolPermissionPolicy::FileChange(FileChangeToolAccess::ReadWrite)
            );
        }
    }

    #[test]
    fn call_projections_are_tool_owned_and_have_no_execution_authority() {
        let engine =
            crate::office::resolve_office_engine(&crate::office::OfficeCliDiscoveryOptions::new());
        let registry = ToolRegistry::defaults_with_search_and_office(None, Some(engine));
        let unsafe_reason = "Inspect\u{202e}the workbook";
        let office_call = AgentToolCall {
            id: "office-observable".to_string(),
            tool: "office_spreadsheet".to_string(),
            args: office_tool_args(
                json!({
                    "operation": "get",
                    "filePath": "budget.xlsx",
                }),
                Some(json!(unsafe_reason)),
            ),
            approval_status: crate::AgentApprovalStatus::NotRequired,
            reason: Some(unsafe_reason.to_string()),
        };

        let trace = registry.trace_call_projection(&office_call);
        let event = registry.event_call_projection(&office_call);
        for projection in [&trace, &event] {
            assert!(projection.args.get("reason").is_none());
            assert_eq!(projection.reason, None);
        }
        assert_eq!(office_call.args["reason"], unsafe_reason);
        assert_eq!(office_call.reason.as_deref(), Some(unsafe_reason));

        let ordinary_call = AgentToolCall {
            id: "read-observable".to_string(),
            tool: "read_file".to_string(),
            args: json!({ "path": "README.md" }),
            approval_status: crate::AgentApprovalStatus::NotRequired,
            reason: Some("Inspect the project overview".to_string()),
        };
        let ordinary_trace = registry.trace_call_projection(&ordinary_call);
        let ordinary_event = registry.event_call_projection(&ordinary_call);
        assert_eq!(ordinary_trace.args, ordinary_call.args);
        assert_eq!(ordinary_trace.reason, ordinary_call.reason);
        assert_eq!(ordinary_event.id, ordinary_trace.id);
        assert_eq!(ordinary_event.tool, ordinary_trace.tool);
        assert_eq!(ordinary_event.args, ordinary_trace.args);
        assert_eq!(ordinary_event.reason, ordinary_trace.reason);
        assert_eq!(
            registry.checkpoint_call_projection(&ordinary_call),
            ordinary_call
        );
        assert_eq!(
            ordinary_event.approval_status,
            ordinary_trace.approval_status
        );
        assert_eq!(
            registry.checkpoint_persistence("read_file"),
            AgentToolCallCheckpointPersistence::Allowed
        );

        let unknown_secret = "Bearer unknown-tool-secret";
        let unknown_call = AgentToolCall {
            id: "unknown-observable".to_string(),
            tool: "provider_hallucinated_tool".to_string(),
            args: json!({ "text": unknown_secret }),
            approval_status: crate::AgentApprovalStatus::NotRequired,
            reason: Some(unknown_secret.to_string()),
        };
        for projection in [
            registry.trace_call_projection(&unknown_call),
            registry.event_call_projection(&unknown_call),
            registry.model_call_projection(&unknown_call),
            registry.checkpoint_call_projection(&unknown_call),
        ] {
            assert_eq!(projection.args, json!({}));
            assert_eq!(projection.reason, None);
            assert!(!serde_json::to_string(&projection)
                .unwrap()
                .contains(unknown_secret));
        }
        assert_eq!(
            registry.checkpoint_persistence(&unknown_call.tool),
            AgentToolCallCheckpointPersistence::DeniedUnknown
        );
    }

    #[test]
    fn structured_writers_declare_their_exact_shared_file_change_access() {
        let registry = ToolRegistry::defaults_with_search(None);

        // FileChange transactions must remain inspectable and abortable after write authority is
        // tightened. Their mutating calls still enforce current write authority inside the tool
        // and again at approval execution.
        assert_eq!(
            registry.permission_policy("apply_patch"),
            AgentToolPermissionPolicy::FileChange(FileChangeToolAccess::ReadWrite),
            "apply_patch must retain only its safe status/abort surface when writes are denied"
        );
        assert_eq!(
            registry.permission_policy("skills_materialize_resource"),
            AgentToolPermissionPolicy::FileChange(FileChangeToolAccess::WriteOnly),
            "materialization has no read-only settlement action and must disappear when writes are denied"
        );
        assert_eq!(
            registry.permission_policy("read_file"),
            AgentToolPermissionPolicy::Default
        );
    }

    #[test]
    fn write_file_is_not_registered_or_executable() {
        let registry = ToolRegistry::defaults_with_search(None);
        assert!(registry.definition_for("write_file").is_none());
        let result = registry.execute(
            &ToolExecutionContext::from_run_context(None),
            &AgentToolCall {
                id: "legacy-write".to_string(),
                tool: "write_file".to_string(),
                args: json!({ "phase": "begin" }),
                approval_status: AgentApprovalStatus::NotRequired,
                reason: None,
            },
        );
        assert!(!result.ok);
        assert_eq!(result.tool, "write_file");
        assert_eq!(result.error.as_deref(), Some("未知工具：write_file"));
    }

    #[test]
    fn git_diff_is_not_registered_or_executable() {
        let registry = ToolRegistry::defaults_with_search(None);
        assert!(registry.definition_for("git_diff").is_none());
        let effective = registry
            .effective_tool_set(registry.definitions(), &Default::default())
            .unwrap();
        assert!(!effective.contains("git_diff"));

        let result = registry.execute(
            &ToolExecutionContext::from_run_context(None),
            &AgentToolCall {
                id: "removed-git-diff".to_string(),
                tool: "git_diff".to_string(),
                args: json!({}),
                approval_status: AgentApprovalStatus::NotRequired,
                reason: None,
            },
        );
        assert!(!result.ok);
        assert_eq!(result.tool, "git_diff");
        assert_eq!(result.error.as_deref(), Some("未知工具：git_diff"));
    }

    #[test]
    fn approval_restore_reuses_the_live_model_projection_contract() {
        let registry = ToolRegistry::defaults_with_search(None);
        let result = AgentToolResult {
            exact_archive_file: None,
            call_id: "command-1".to_string(),
            tool: "run_command".to_string(),
            ok: true,
            result: Some(json!({
                "command": "printf done",
                "cwd": "/workspace",
                "exitCode": 0,
                "stdout": "done",
                "stderr": "",
                "durationMs": 12,
                "runtime": { "provider": "managed" }
            })),
            error: None,
        };
        assert_eq!(
            serde_json::to_value(registry.model_projection(&result)).unwrap(),
            serde_json::to_value(model_projection_for_persisted_continuation(&result)).unwrap()
        );
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
            collaboration_identity: None,
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
        }))
        .with_runtime_services("attachment-read-run".to_string(), None);
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
    fn read_image_rejects_text_only_models_before_resolving_the_path() {
        let context = ToolExecutionContext::from_run_context(None);
        let registry = ToolRegistry::defaults_with_search(None);
        let result = registry.execute(
            &context,
            &AgentToolCall {
                id: "call-image-unsupported".to_string(),
                tool: "read_image".to_string(),
                args: json!({ "path": "/path/that/must/not/be-resolved.png" }),
                approval_status: crate::protocol::AgentApprovalStatus::NotRequired,
                reason: None,
            },
        );

        assert!(!result.ok);
        assert_eq!(result.call_id, "call-image-unsupported");
        assert_eq!(result.tool, "read_image");
        let value = result.result.as_ref().expect("structured capability error");
        assert_eq!(value["type"], "model_capability");
        assert_eq!(value["code"], "modelCapabilityUnsupported");
        assert_eq!(value["errorCode"], "agent.model_capability_unsupported");
        assert_eq!(value["capability"], "imageInput");
        assert_eq!(value["required"], true);
        assert_eq!(value["actual"], false);
        assert_eq!(value["tool"], "read_image");
        assert_eq!(value["recovery"], "switchToImageCapableModel");
        assert!(result
            .error
            .as_deref()
            .is_some_and(|message| message.contains("文件尚未读取")));
    }

    #[test]
    fn read_image_reads_attachment_visual_payload() {
        let fixture = TestWorkspace::new();
        let attachment_root = fixture.root.join("attachments");
        let storage_rel_path = PathBuf::from("conversations/c1/m1/image1/pixel.png");
        let attachment_path = attachment_root.join(&storage_rel_path);
        fs::create_dir_all(attachment_path.parent().unwrap()).unwrap();
        let image_bytes = valid_test_png();
        fs::write(&attachment_path, &image_bytes).unwrap();

        let context = ToolExecutionContext::from_run_context(Some(&AgentRunContext {
            collaboration_identity: None,
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
                    size_bytes: image_bytes.len() as u64,
                    read_path: "@attachments/image1/pixel.png".to_string(),
                    storage_rel_path: "conversations/c1/m1/image1/pixel.png".to_string(),
                    created_at: 1,
                }],
                project_attachments: Vec::new(),
            }),
            permissions: Default::default(),
        }))
        .with_model_capabilities(ModelCapabilities { image_input: true });
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
        assert!(value["thumbnailDataUrl"]
            .as_str()
            .is_some_and(|thumbnail| thumbnail.starts_with("data:image/png;base64,")));
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
            collaboration_identity: None,
            conversation_id: Some("absolute-read-conversation".to_string()),
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
        }))
        .with_runtime_services("absolute-read-run".to_string(), None);
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

    #[test]
    fn source_truncation_distinguishes_lossless_pages_from_unrecoverable_cuts() {
        let pageable = AgentToolResult {
            exact_archive_file: None,
            call_id: "pageable".to_string(),
            tool: "read_file".to_string(),
            ok: true,
            result: Some(json!({
                "content": "prefix",
                "truncated": true,
                "nextStartByte": 6
            })),
            error: None,
        };
        let unrecoverable = AgentToolResult {
            exact_archive_file: None,
            call_id: "unrecoverable".to_string(),
            tool: "web_fetch".to_string(),
            ok: true,
            result: Some(json!({
                "content": "prefix",
                "truncated": true
            })),
            error: None,
        };
        let skill_page = AgentToolResult {
            exact_archive_file: None,
            call_id: "skills-page".to_string(),
            tool: "skills_list_resources".to_string(),
            ok: true,
            result: Some(json!({
                "resources": [],
                "truncated": true,
                "nextAfterPath": "references/next.md"
            })),
            error: None,
        };
        let explicitly_complete_source = AgentToolResult {
            exact_archive_file: None,
            call_id: "explicit-source".to_string(),
            tool: "custom".to_string(),
            ok: true,
            result: Some(json!({
                "content": "bounded local projection",
                "truncated": true,
                "truncatedAtSource": false
            })),
            error: None,
        };

        assert!(!tool_result_truncated_at_source(&pageable));
        assert!(!tool_result_truncated_at_source(&skill_page));
        assert!(!tool_result_truncated_at_source(
            &explicitly_complete_source
        ));
        assert!(tool_result_truncated_at_source(&unrecoverable));
    }

    #[test]
    fn source_truncation_recognizes_stream_and_artifact_flags_at_any_depth() {
        for (call_id, result) in [
            (
                "stdout",
                json!({
                    "status": "completed",
                    "stdoutTruncated": true,
                    "stderrTruncated": false
                }),
            ),
            (
                "stderr",
                json!({
                    "execution": {
                        "stderr_truncated": true
                    }
                }),
            ),
            (
                "changes",
                json!({
                    "observation": {
                        "coverage": {
                            "changesTruncated": true
                        }
                    }
                }),
            ),
        ] {
            let result = AgentToolResult {
                exact_archive_file: None,
                call_id: call_id.to_string(),
                tool: "custom".to_string(),
                ok: true,
                result: Some(result),
                error: None,
            };
            assert!(
                tool_result_truncated_at_source(&result),
                "{call_id} truncation must be recognized"
            );
        }
    }

    #[test]
    fn source_truncation_accepts_only_non_empty_semantic_recovery_routes() {
        for (call_id, recovery) in [
            ("cursor", json!({ "cursor": "page-2" })),
            (
                "continue-with",
                json!({
                    "continueWith": {
                        "tool": "read_file",
                        "args": { "startByte": 1024 }
                    }
                }),
            ),
            ("history-open", json!({ "historyOpen": "hist_v1_page_2" })),
            (
                "navigation",
                json!({ "navigation": { "older": "hist_v1_older" } }),
            ),
        ] {
            let mut object = recovery.as_object().unwrap().clone();
            object.insert("truncated".to_string(), Value::Bool(true));
            let result = AgentToolResult {
                exact_archive_file: None,
                call_id: call_id.to_string(),
                tool: "custom".to_string(),
                ok: true,
                result: Some(Value::Object(object)),
                error: None,
            };
            assert!(
                !tool_result_truncated_at_source(&result),
                "{call_id} must make the bounded page recoverable"
            );
        }

        for (call_id, recovery) in [
            ("null-cursor", json!({ "cursor": null })),
            ("empty-cursor", json!({ "nextCursor": "   " })),
            ("empty-continuation", json!({ "continueWith": {} })),
            (
                "empty-navigation",
                json!({ "navigation": { "older": null, "newer": "" } }),
            ),
        ] {
            let mut object = recovery.as_object().unwrap().clone();
            object.insert("truncated".to_string(), Value::Bool(true));
            let result = AgentToolResult {
                exact_archive_file: None,
                call_id: call_id.to_string(),
                tool: "custom".to_string(),
                ok: true,
                result: Some(Value::Object(object)),
                error: None,
            };
            assert!(
                tool_result_truncated_at_source(&result),
                "{call_id} is not a usable recovery route"
            );
        }
    }

    #[test]
    fn explicit_source_declarations_are_scoped_and_authoritative() {
        let explicit_true_with_recovery = AgentToolResult {
            exact_archive_file: None,
            call_id: "explicit-true".to_string(),
            tool: "custom".to_string(),
            ok: true,
            result: Some(json!({
                "truncatedAtSource": true,
                "historyOpen": "hist_v1_received_payload"
            })),
            error: None,
        };
        let dedicated_cut_under_explicitly_complete_parent = AgentToolResult {
            exact_archive_file: None,
            call_id: "explicit-false".to_string(),
            tool: "custom".to_string(),
            ok: true,
            result: Some(json!({
                "truncatedAtSource": false,
                "stdoutTruncated": true
            })),
            error: None,
        };
        let nested_source_cut_under_explicitly_complete_parent = AgentToolResult {
            exact_archive_file: None,
            call_id: "nested-source".to_string(),
            tool: "custom".to_string(),
            ok: true,
            result: Some(json!({
                "truncatedAtSource": false,
                "nested": {
                    "stderrTruncated": true
                }
            })),
            error: None,
        };
        let dedicated_cut_with_history_recovery = AgentToolResult {
            exact_archive_file: None,
            call_id: "dedicated-with-history".to_string(),
            tool: "custom".to_string(),
            ok: true,
            result: Some(json!({
                "stdoutTruncated": true,
                "historyOpen": "hist_v1_received_payload"
            })),
            error: None,
        };

        assert!(tool_result_truncated_at_source(
            &explicit_true_with_recovery
        ));
        // Legacy stream flags continue to describe the 128KiB Event preview. An explicit source
        // declaration on the same object disambiguates that recoverable preview cut.
        assert!(!tool_result_truncated_at_source(
            &dedicated_cut_under_explicitly_complete_parent
        ));
        assert!(tool_result_truncated_at_source(
            &nested_source_cut_under_explicitly_complete_parent
        ));
        assert!(tool_result_truncated_at_source(
            &dedicated_cut_with_history_recovery
        ));
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
