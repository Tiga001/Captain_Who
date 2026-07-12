use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::error::Error;
use std::fmt::{Display, Formatter};

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AgentChatInput {
    pub api_url: String,
    pub api_token: String,
    pub model: String,
    pub api_style: Option<AgentApiStyle>,
    #[serde(default)]
    pub context_window_tokens: Option<u32>,
    #[serde(default = "default_true")]
    pub context_budget_enabled: bool,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub stream: Option<bool>,
    pub context: Option<AgentRunContext>,
    pub search_config: Option<AgentSearchConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_preferences: Option<AgentPromptPreferences>,
    pub approval_decision: Option<AgentApprovalDecision>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_continuation: Option<AgentToolContinuation>,
    #[serde(default)]
    pub attachments: Vec<AgentInputAttachment>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resume_checkpoint: Option<AgentRunCheckpoint>,
    pub messages: Vec<AgentChatMessage>,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AgentExtensionSnapshot {
    pub extension_id: String,
    pub version: u32,
    pub state: Value,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AgentRunCheckpoint {
    pub version: u32,
    pub run_id: String,
    pub context_items: Vec<AgentContextCheckpointItem>,
    pub next_model_request_index: usize,
    pub queued_tool_calls: Vec<AgentQueuedToolCallCheckpoint>,
    pub suppressed_narration: bool,
    pub extension_snapshots: Vec<AgentExtensionSnapshot>,
    pub pending_tool_call_id: String,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AgentContextCheckpointItem {
    pub role: String,
    pub content: String,
    pub images: Vec<AgentContextCheckpointImage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    pub tool_calls: Vec<AgentContextCheckpointToolCall>,
    pub is_error: bool,
    pub sources: Vec<String>,
    pub scope: String,
    pub retention: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<AgentContextCheckpointGroup>,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentContextCheckpointImage {
    pub mime_type: String,
    pub data_base64: String,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AgentContextCheckpointToolCall {
    pub id: String,
    pub name: String,
    pub args: Value,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentContextCheckpointGroup {
    pub id: String,
    pub kind: String,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AgentQueuedToolCallCheckpoint {
    pub call: AgentContextCheckpointToolCall,
    pub assistant_content: String,
    pub group_id: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct AgentChatMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AgentToolContinuation {
    pub call: AgentToolCall,
    pub result: AgentToolResult,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentInputAttachmentKind {
    File,
    Image,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentInputAttachmentEncoding {
    Utf8,
    Base64,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AgentInputAttachment {
    pub id: String,
    pub kind: AgentInputAttachmentKind,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    pub size_bytes: u64,
    pub encoding: AgentInputAttachmentEncoding,
    pub data: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncated: Option<bool>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AgentChatOutput {
    pub content: String,
    pub status: AgentRunStatus,
    pub run_id: String,
    pub events: Vec<AgentEvent>,
    pub tool_definitions: Vec<AgentToolDefinition>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub todo: Option<AgentTodoState>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<AgentUsage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
    pub proposed_actions: Vec<AgentProposedAction>,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentRunStatus {
    Idle,
    Running,
    WaitingForApproval,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentApiStyle {
    OpenAiCompatible,
    AnthropicCompatible,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentSearchMode {
    Auto,
    Disabled,
    Tavily,
}

/// Controls which local paths read-only tools may inspect.
///
/// `WorkspaceOnly` restricts file reads and searches to the selected workspace plus registered
/// attachment paths. `All` allows absolute local paths and supported system aliases such as
/// `@home`, `@desktop`, `@documents`, and `@downloads`.
#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentReadPermission {
    WorkspaceOnly,
    All,
}

/// Controls where file-changing tools may write.
///
/// `Denied` removes file-edit tools from the agent tool set and blocks patch execution.
/// `WorkspaceOnly` allows safe writes only inside the selected workspace. `All` also allows
/// safe writes outside the workspace through absolute paths or supported system aliases.
#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentWritePermission {
    Denied,
    WorkspaceOnly,
    All,
}

/// Controls whether approved command proposals require a human click.
///
/// `AutoApprove` only skips the approval prompt. It still runs through the same command
/// validation, cwd scope checks, timeout, cancellation, and dangerous-command blocking as manual
/// approval.
#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentCommandPermission {
    RequireApproval,
    AutoApprove,
}

/// Controls whether file edit proposals require a human click.
///
/// `AutoApprove` only skips the approval prompt. It still runs through the same safe patch
/// executor, write scope checks, symlink/path traversal checks, and revision conflict checks as
/// manual approval.
#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum AgentPatchPermission {
    #[default]
    RequireApproval,
    AutoApprove,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentPermissions {
    pub read: AgentReadPermission,
    pub write: AgentWritePermission,
    pub command: AgentCommandPermission,
    #[serde(default)]
    pub patch: AgentPatchPermission,
}

impl Default for AgentPermissions {
    fn default() -> Self {
        Self {
            read: AgentReadPermission::WorkspaceOnly,
            write: AgentWritePermission::Denied,
            command: AgentCommandPermission::RequireApproval,
            patch: AgentPatchPermission::RequireApproval,
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentPromptWorkMode {
    Coding,
    General,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentPromptTone {
    Friendly,
    Pragmatic,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentPromptDetailLevel {
    Low,
    Medium,
    High,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AgentPromptPreferences {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub work_mode: Option<AgentPromptWorkMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tone: Option<AgentPromptTone>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail_level: Option<AgentPromptDetailLevel>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_instructions: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<i64>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AgentSearchConfig {
    pub mode: AgentSearchMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tavily_api_key: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AgentApprovalDecision {
    pub action_id: String,
    pub status: AgentApprovalDecisionStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AgentRunContext {
    pub conversation_id: Option<String>,
    pub project_id: Option<String>,
    pub workspace: Option<AgentWorkspaceContext>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attachment_library: Option<AgentAttachmentLibraryContext>,
    #[serde(default)]
    pub permissions: AgentPermissions,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AgentWorkspaceContext {
    pub project_id: Option<String>,
    pub display_name: Option<String>,
    pub root_path: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AgentAttachmentLibraryContext {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub root_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    #[serde(default)]
    pub conversation_attachments: Vec<AgentAttachmentReference>,
    #[serde(default)]
    pub project_attachments: Vec<AgentAttachmentReference>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AgentAttachmentReference {
    pub id: String,
    pub conversation_id: String,
    pub message_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    pub kind: AgentInputAttachmentKind,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    pub size_bytes: u64,
    pub read_path: String,
    pub storage_rel_path: String,
    pub created_at: i64,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentContextWindowStatus {
    Unconfigured,
    WithinBudget,
    OverBudget,
    InvalidConfiguration,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentContextWindowPhase {
    Idle,
    ModelRequest,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentContextWindowSource {
    Estimated,
    ProviderReported,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentContextWindowSnapshot {
    pub model: String,
    pub status: AgentContextWindowStatus,
    pub phase: AgentContextWindowPhase,
    pub source: AgentContextWindowSource,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window_tokens: Option<u64>,
    pub reserved_output_tokens: u64,
    pub safety_margin_tokens: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub available_input_tokens: Option<u64>,
    pub used_input_tokens: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remaining_input_tokens: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_index: Option<usize>,
    pub context_revision: u64,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AgentUsage {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_thinking_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cached_input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_creation_input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub billable_request_count: Option<u64>,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum AgentUsageSummaryRange {
    Last7Days,
    Last30Days,
    All,
    Custom,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AgentUsageSummaryInput {
    pub range: AgentUsageSummaryRange,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<i64>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AgentUsageModelSummary {
    pub model_id: String,
    pub model_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_path: Option<String>,
    pub is_configured: bool,
    pub request_count: u64,
    pub message_count: u64,
    pub unpriced_message_count: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_thinking_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cached_input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_creation_input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub estimated_cost: Option<f64>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AgentUsageSummaryOutput {
    pub request_count: u64,
    pub message_count: u64,
    pub unpriced_message_count: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_thinking_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cached_input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_creation_input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub estimated_cost: Option<f64>,
    pub models: Vec<AgentUsageModelSummary>,
}

#[derive(Debug, Deserialize, Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct AgentUsageClearInput {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<i64>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AgentUsageClearOutput {
    pub deleted_records: u64,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AgentStateSnapshot {
    pub status: AgentRunStatus,
    pub active_run_id: Option<String>,
    pub last_error: Option<String>,
    pub updated_at: u64,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentApprovalStatus {
    NotRequired,
    Required,
    Approved,
    Rejected,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentApprovalDecisionStatus {
    Approved,
    Rejected,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentPatchOperation {
    Create,
    Update,
    Delete,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentPatchResultStatus {
    Applied,
    Failed,
    Conflict,
    Rejected,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentToolSafety {
    ReadOnly,
    RequiresApproval,
    Destructive,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentToolApprovalMode {
    Never,
    Always,
    Dynamic,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentFileWriteMode {
    Create,
    Rewrite,
    Modify,
    Append,
    Upsert,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentFileDraftStatus {
    Drafting,
    Ready,
    WaitingApproval,
    Applying,
    Applied,
    Rejected,
    Conflict,
    Failed,
    Aborted,
    Expired,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentFileWriteResultStatus {
    Applied,
    Failed,
    Conflict,
    Rejected,
    AlreadyApplied,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentTodoStatus {
    Pending,
    InProgress,
    Completed,
    Blocked,
}

impl AgentTodoStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::InProgress => "in_progress",
            Self::Completed => "completed",
            Self::Blocked => "blocked",
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AgentTodoItem {
    pub id: String,
    pub title: String,
    pub status: AgentTodoStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub created_at: u64,
    pub updated_at: u64,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AgentTodoState {
    pub revision: u64,
    pub items: Vec<AgentTodoItem>,
    pub updated_at: u64,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentCommandOutputStream {
    Stdout,
    Stderr,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentCommandRiskLevel {
    ReadOnly,
    WritesWorkspace,
    Network,
    Destructive,
    Unknown,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AgentToolCall {
    pub id: String,
    pub tool: String,
    pub args: Value,
    pub approval_status: AgentApprovalStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AgentToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    pub safety: AgentToolSafety,
    pub requires_workspace: bool,
    pub requires_approval: bool,
    pub approval_mode: AgentToolApprovalMode,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AgentToolResult {
    pub call_id: String,
    pub tool: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AgentDiffProposal {
    pub id: String,
    pub operation: AgentPatchOperation,
    pub file_path: String,
    pub patch: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_revision: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    pub approval_status: AgentApprovalStatus,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AgentFileDraftSnapshot {
    pub draft_id: String,
    pub conversation_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    pub file_path: String,
    pub mode: AgentFileWriteMode,
    pub status: AgentFileDraftStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_revision: Option<String>,
    pub additions: u64,
    pub deletions: u64,
    pub line_count: u64,
    pub byte_count: u64,
    pub chunk_count: u64,
    pub next_chunk_index: u64,
    pub stats_final: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AgentFileWritePreview {
    pub preview_id: String,
    pub stream_id: String,
    pub attempt: usize,
    pub tool_call_index: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    pub draft_id: String,
    pub file_path: String,
    pub additions: u64,
    pub deletions: u64,
    pub line_count: u64,
    pub byte_count: u64,
    pub generated_bytes: u64,
    pub content_offset_bytes: u64,
    pub content_delta: String,
    pub updated_at: i64,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AgentFileWriteProposal {
    pub id: String,
    pub draft_id: String,
    pub mode: AgentFileWriteMode,
    pub file_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_revision: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    pub additions: u64,
    pub deletions: u64,
    pub line_count: u64,
    pub byte_count: u64,
    pub approval_status: AgentApprovalStatus,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AgentFileWriteResult {
    pub status: AgentFileWriteResultStatus,
    pub draft_id: String,
    pub mode: AgentFileWriteMode,
    pub file_path: String,
    pub additions: u64,
    pub deletions: u64,
    pub line_count: u64,
    pub byte_count: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AgentPatchResult {
    pub status: AgentPatchResultStatus,
    pub operation: AgentPatchOperation,
    pub file_path: String,
    #[serde(default)]
    pub applied_file_paths: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git_diff: Option<AgentGitDiffSnapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git_diff_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AgentGitDiffSnapshot {
    pub patch: String,
    pub truncated: bool,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AgentCommandRequest {
    pub id: String,
    pub command: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    pub approval_status: AgentApprovalStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub risk_level: Option<AgentCommandRiskLevel>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum AgentProposedAction {
    ToolCall { call: AgentToolCall },
    Diff { diff: AgentDiffProposal },
    FileWrite { file_write: AgentFileWriteProposal },
    Command { command: AgentCommandRequest },
}

#[derive(Debug, Serialize, Clone)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum AgentEvent {
    Started {
        run_id: String,
        tool_definitions: Vec<AgentToolDefinition>,
    },
    State {
        run_id: String,
        state: AgentStateSnapshot,
    },
    MessageDelta {
        run_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        stream_id: Option<String>,
        delta: String,
    },
    MessageStreamStarted {
        run_id: String,
        stream_id: String,
        attempt: usize,
    },
    MessageStreamReset {
        run_id: String,
        stream_id: String,
        reason: String,
    },
    MessageStreamCommitted {
        run_id: String,
        stream_id: String,
    },
    LlmRetry {
        run_id: String,
        stream_id: String,
        attempt: usize,
        max_attempts: usize,
        reason: String,
    },
    ToolInputProgress {
        run_id: String,
        stream_id: String,
        attempt: usize,
        tool_call_index: usize,
        #[serde(skip_serializing_if = "Option::is_none")]
        tool_call_id: Option<String>,
        tool: String,
        received_bytes: u64,
    },
    FileWritePreviewUpdated {
        run_id: String,
        preview: AgentFileWritePreview,
    },
    FileWritePreviewCleared {
        run_id: String,
        stream_id: String,
        attempt: usize,
    },
    Message {
        run_id: String,
        content: String,
    },
    ToolCall {
        run_id: String,
        call: AgentToolCall,
    },
    ToolResult {
        run_id: String,
        result: AgentToolResult,
    },
    TodoUpdated {
        run_id: String,
        todo: AgentTodoState,
    },
    FileDraftUpdated {
        run_id: String,
        draft: AgentFileDraftSnapshot,
    },
    ContextWindowUpdated {
        run_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        conversation_id: Option<String>,
        snapshot: AgentContextWindowSnapshot,
    },
    ApprovalRequired {
        run_id: String,
        action: AgentProposedAction,
        #[serde(skip)]
        checkpoint: AgentRunCheckpoint,
    },
    Diff {
        run_id: String,
        diff: AgentDiffProposal,
    },
    CommandOutput {
        run_id: String,
        command: String,
        stream: AgentCommandOutputStream,
        output: String,
    },
    Error {
        run_id: Option<String>,
        message: String,
        recoverable: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        code: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        details: Option<Value>,
    },
    Done {
        run_id: String,
        success: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        status: Option<AgentRunStatus>,
        #[serde(skip_serializing_if = "Option::is_none")]
        content: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        usage: Option<AgentUsage>,
        #[serde(skip_serializing_if = "Option::is_none")]
        finish_reason: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        proposed_actions: Vec<AgentProposedAction>,
    },
}

#[derive(Debug, Clone)]
pub struct AgentError {
    message: String,
    cancelled: bool,
    usage: Option<Box<AgentUsage>>,
    code: Option<String>,
    details: Option<Box<Value>>,
}

pub type AgentResult<T> = Result<T, AgentError>;

impl AgentError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            cancelled: false,
            usage: None,
            code: None,
            details: None,
        }
    }

    pub fn structured(code: impl Into<String>, message: impl Into<String>, details: Value) -> Self {
        Self {
            message: message.into(),
            cancelled: false,
            usage: None,
            code: Some(code.into()),
            details: Some(Box::new(details)),
        }
    }

    pub fn cancelled() -> Self {
        Self {
            message: "agent run 已取消。".to_string(),
            cancelled: true,
            usage: None,
            code: None,
            details: None,
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled
    }

    pub fn usage(&self) -> Option<&AgentUsage> {
        self.usage.as_deref()
    }

    pub fn code(&self) -> Option<&str> {
        self.code.as_deref()
    }

    pub fn details(&self) -> Option<&Value> {
        self.details.as_deref()
    }

    pub fn with_usage(mut self, usage: Option<AgentUsage>) -> Self {
        self.usage = usage.map(Box::new);
        self
    }
}

impl Display for AgentError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for AgentError {}

impl From<String> for AgentError {
    fn from(message: String) -> Self {
        Self::new(message)
    }
}

impl From<&str> for AgentError {
    fn from(message: &str) -> Self {
        Self::new(message)
    }
}
