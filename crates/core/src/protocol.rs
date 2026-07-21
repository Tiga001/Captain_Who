use crate::context::ContextCompactionSummary;
use crate::conversation_trace::{ConversationTurnTrace, ConversationTurnTraceItem};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::error::Error;
use std::fmt::{Display, Formatter};

/// Backend-authoritative capabilities frozen for one logical model run.
///
/// This provider-neutral contract is intentionally separate from tool
/// definitions. Tools remain registered consistently for prompt-cache
/// stability and enforce unsupported capabilities at execution time.
#[derive(Debug, Default, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ModelCapabilities {
    #[serde(default)]
    pub image_input: bool,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AgentChatInput {
    pub api_url: String,
    pub api_token: String,
    pub model: String,
    /// Resolved by the backend from the selected model configuration and kept
    /// immutable across approval pause/resume for this logical run.
    #[serde(default)]
    pub model_capabilities: ModelCapabilities,
    pub api_style: Option<AgentApiStyle>,
    #[serde(default)]
    pub context_window_tokens: Option<u32>,
    #[serde(default = "default_true")]
    pub context_window_indicator_enabled: bool,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assistant_message_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_compaction_summary: Option<ContextCompactionSummary>,
    /// Immutable Skill snapshots selected for this logical agent run. The runtime treats this as
    /// dynamic run context; it is deliberately excluded from the stable system prompt and the
    /// conversation context configuration fingerprint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill_activation: Option<AgentSkillActivation>,
    /// Backend-authoritative, run-scoped metadata for globally enabled Skills that the model may
    /// choose to activate. Full Skill instructions and resource authority are intentionally absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill_discovery: Option<crate::skills::AgentSkillDiscoverySnapshot>,
    pub messages: Vec<AgentChatMessage>,
}

#[derive(Default, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentSkillActivation {
    pub activation_revision: String,
    #[serde(default)]
    pub skills: Vec<AgentActivatedSkill>,
}

impl std::fmt::Debug for AgentSkillActivation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AgentSkillActivation")
            .field("activation_revision", &self.activation_revision)
            .field("skills", &self.skills)
            .finish()
    }
}

#[derive(Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentActivatedSkill {
    pub id: String,
    pub name: String,
    pub revision: String,
    pub source: String,
    pub instructions: String,
    /// Exact verified SKILL.md source size used for aggregate activation policy enforcement.
    #[serde(default)]
    pub source_bytes: u64,
    /// Lightweight discovery hint for the run-scoped Resource Runtime. The
    /// resource index and bytes remain behind the host capability.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resources: Option<AgentActivatedSkillResources>,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentActivatedSkillResources {
    pub root_uri: String,
    pub resource_count: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub kinds: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentSkillActivationActor {
    User,
    Model,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentSkillActivatedEvent {
    pub id: String,
    pub name: String,
    pub revision: String,
    pub source: String,
    pub activated_by: AgentSkillActivationActor,
}

impl std::fmt::Debug for AgentActivatedSkill {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AgentActivatedSkill")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("revision", &self.revision)
            .field("source", &self.source)
            .field("instructions_bytes", &self.instructions.len())
            .field("resources", &self.resources)
            .finish()
    }
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
    #[serde(default)]
    pub conversation_trace_items: Vec<ConversationTurnTraceItem>,
    #[serde(default)]
    pub next_conversation_trace_sequence: u64,
    #[serde(default)]
    pub conversation_trace_truncated: bool,
    /// Number of leading trace items already included in a successful main-model request.
    pub model_visible_trace_item_count: usize,
}

#[derive(Deserialize, Serialize, Clone, PartialEq)]
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<AgentContextCheckpointOrigin>,
}

impl std::fmt::Debug for AgentContextCheckpointItem {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AgentContextCheckpointItem")
            .field("role", &self.role)
            .field("content_bytes", &self.content.len())
            .field("image_count", &self.images.len())
            .field("tool_call_id", &self.tool_call_id)
            .field("tool_call_count", &self.tool_calls.len())
            .field("is_error", &self.is_error)
            .field("sources", &self.sources)
            .field("scope", &self.scope)
            .field("retention", &self.retention)
            .field("group", &self.group)
            .field("origin", &self.origin)
            .finish()
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentContextCheckpointOrigin {
    pub kind: String,
    pub id: String,
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
#[serde(rename_all = "camelCase")]
pub struct AgentChatMessage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
    pub role: String,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_turn_trace: Option<ConversationTurnTrace>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversation_turn_trace: Option<ConversationTurnTrace>,
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
/// Approval and command safety are separate inputs. In guarded mode, `AutoApprove` applies only to
/// commands the policy permits automatically; high-impact commands may still require an explicit
/// user approval. Structural validation, cwd checks, timeout, cancellation, and always-denied
/// operations apply to every path.
#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentCommandPermission {
    RequireApproval,
    AutoApprove,
}

/// Controls which command safety policy applies after a command has been authorized.
///
/// `Guarded` limits automatic execution and routes high-impact commands to explicit approval.
/// `FullAccess` permits automatic high-impact commands except operations classified as always
/// denied. This is intentionally independent from [`AgentCommandPermission`], which expresses the
/// user's normal approval preference.
#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum AgentCommandSafetyPolicy {
    #[default]
    Guarded,
    FullAccess,
}

/// Controls whether structured file-write proposals require a human click.
///
/// `AutoApprove` only skips the approval prompt. It still runs through the same safe patch
/// or document executor, write scope checks, path checks, and revision conflict checks as manual
/// approval. This policy covers every tool registered in the file-write permission domain,
/// including Office document writers.
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
    pub command_safety: AgentCommandSafetyPolicy,
    #[serde(default)]
    pub patch: AgentPatchPermission,
}

impl Default for AgentPermissions {
    fn default() -> Self {
        Self {
            read: AgentReadPermission::WorkspaceOnly,
            write: AgentWritePermission::Denied,
            command: AgentCommandPermission::RequireApproval,
            command_safety: AgentCommandSafetyPolicy::Guarded,
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
    DurableCommit,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentContextWindowSnapshot {
    pub model: String,
    pub status: AgentContextWindowStatus,
    pub phase: AgentContextWindowPhase,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window_tokens: Option<u64>,
    pub reserved_output_tokens: u64,
    pub safety_margin_tokens: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// Input capacity left for durable conversation history after fixed request costs.
    pub durable_capacity_tokens: Option<u64>,
    /// Conversation history and trace content that survives into later turns.
    pub durable_input_tokens: u64,
    /// Run-scoped context such as activated Skill instructions. This is measured for the current
    /// preview/request but never contributes to the durable cache revision.
    pub run_transient_input_tokens: u64,
    /// Fully assembled input estimate, including fixed, durable and run-scoped context.
    pub request_input_tokens: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remaining_durable_tokens: Option<i64>,
    /// Opaque fingerprint that changes when the fixed or durable assembled context changes.
    pub persistent_revision: String,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
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

/// Result status for a revision-bound Skill resource materialization.
#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentSkillMaterializationResultStatus {
    Applied,
    AlreadyApplied,
    Conflict,
    Rejected,
    Failed,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentSkillScriptInterpreter {
    Python3,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentSkillScriptPreflightStatus {
    Ready,
    MissingDependencies,
    Unsupported,
    Conflict,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentSkillDependencyKind {
    PythonDistribution,
    Command,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentSkillDependencyStatus {
    Available,
    Missing,
}

/// Version of the persisted, approval-gated Office action envelope.
///
/// Version 4 binds a provider-neutral typed operation request and a normalized,
/// non-empty, user-facing reason of at most [`AGENT_OFFICE_REASON_MAX_CHARS`]
/// characters to the frozen Office action. Provider argv is generated and
/// revalidated by the trusted host. Older actions must be prepared again instead
/// of being interpreted under this stricter contract.
pub const AGENT_OFFICE_OPERATION_SCHEMA_VERSION: u32 = 4;

/// Maximum number of Unicode scalar values accepted in an Office call reason.
pub const AGENT_OFFICE_REASON_MAX_CHARS: usize = 240;

fn is_agent_office_reason_bidi_control(character: char) -> bool {
    matches!(
        character,
        '\u{061c}'
            | '\u{200e}'
            | '\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2066}'..='\u{2069}'
    )
}

pub(crate) fn has_unsafe_agent_office_reason_character(reason: &str) -> bool {
    reason.chars().any(|character| {
        character.is_control()
            || matches!(character, '\u{0085}' | '\u{2028}' | '\u{2029}')
            || is_agent_office_reason_bidi_control(character)
    })
}

pub(crate) fn normalize_agent_office_reason(reason: &str) -> Option<String> {
    if has_unsafe_agent_office_reason_character(reason) {
        return None;
    }
    let reason = reason.trim();
    (!reason.is_empty() && reason.chars().count() <= AGENT_OFFICE_REASON_MAX_CHARS)
        .then(|| reason.to_string())
}

/// Returns whether an Office action reason is valid in its canonical persisted form.
///
/// Tool input is trimmed before an action is frozen. Persisted and host-submitted
/// actions must already contain that normalized value so whitespace cannot be
/// changed after approval without invalidating the snapshot.
pub fn is_valid_agent_office_reason(reason: &str) -> bool {
    normalize_agent_office_reason(reason).as_deref() == Some(reason)
}

/// Wire contract for an approval-gated Office mutation.
///
/// The prepared execution is produced by the trusted Office adapter before an
/// approval is requested. It contains only a normalized, shell-free operation
/// plan and immutable preconditions; executable paths and environment values
/// are deliberately excluded from the persisted action.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentOfficeOperationRequest {
    pub schema_version: u32,
    pub id: String,
    pub prepared: crate::office::OfficePreparedExecution,
    pub approval_status: AgentApprovalStatus,
    pub reason: String,
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

/// Schema version for the best-effort artifact observation attached to `run_command` results.
///
/// Observation is telemetry, not an authorization capability. The host still authorizes and
/// executes the command exclusively through the existing command permission and safety policy.
pub const AGENT_COMMAND_ARTIFACT_OBSERVATION_SCHEMA_VERSION: u32 = 2;

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum AgentCommandArtifactObservationKind {
    Office,
}

/// Optional, model-authored hint describing which command side effects should be observed.
///
/// A selected workspace is always included when this hint is present. Expected outputs and
/// additional roots are resolved relative to the command cwd. Expected outputs are observation
/// and validation hints, not write authorization. The trusted host validates every path against
/// the current permission snapshot; this request cannot widen it.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentCommandArtifactObservationRequest {
    pub kinds: Vec<AgentCommandArtifactObservationKind>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub expected_outputs: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub additional_roots: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentCommandArtifactObservationStatus {
    Complete,
    Partial,
    Failed,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentCommandArtifactObservationPhase {
    Setup,
    Before,
    After,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum AgentCommandArtifactKind {
    Document,
    Spreadsheet,
    Presentation,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentCommandArtifactScope {
    Workspace,
    External,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentCommandArtifactChangeKind {
    Created,
    Modified,
    Replaced,
    Deleted,
    Renamed,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentCommandExpectedArtifactOutcomeKind {
    Created,
    Modified,
    Replaced,
    Renamed,
    Unchanged,
    Missing,
    Unobserved,
    Invalid,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentCommandArtifactValidationStatus {
    Valid,
    Invalid,
    NotApplicable,
    Unchecked,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentCommandArtifactValidation {
    pub status: AgentCommandArtifactValidationStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentCommandArtifactMetadata {
    pub size_bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    pub validation: AgentCommandArtifactValidation,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentCommandArtifactChange {
    pub kind: AgentCommandArtifactChangeKind,
    pub artifact_kind: AgentCommandArtifactKind,
    pub path: String,
    pub scope: AgentCommandArtifactScope,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_scope: Option<AgentCommandArtifactScope>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before: Option<AgentCommandArtifactMetadata>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after: Option<AgentCommandArtifactMetadata>,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentCommandExpectedArtifactOutcome {
    pub requested_path: String,
    pub outcome: AgentCommandExpectedArtifactOutcomeKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<AgentCommandArtifactScope>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact_kind: Option<AgentCommandArtifactKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<AgentCommandArtifactMetadata>,
}

#[derive(Debug, Deserialize, Serialize, Clone, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentCommandArtifactSnapshotCoverage {
    pub roots_scanned: u64,
    pub directory_entries_scanned: u64,
    pub office_files_seen: u64,
    pub files_hashed: u64,
    pub files_unhashed: u64,
    pub bytes_hashed: u64,
    pub symlinks_skipped: u64,
    pub excluded_directories: u64,
    #[serde(default)]
    pub duration_ms: u64,
    #[serde(default)]
    pub time_budget_exceeded: bool,
    #[serde(default)]
    pub cancelled: bool,
    pub truncated: bool,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentCommandArtifactObservationCoverage {
    pub workspace_included: bool,
    pub expected_output_count: u64,
    pub additional_root_count: u64,
    pub before: AgentCommandArtifactSnapshotCoverage,
    pub after: AgentCommandArtifactSnapshotCoverage,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentCommandArtifactObservationWarning {
    pub phase: AgentCommandArtifactObservationPhase,
    pub code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub message: String,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentCommandArtifactObservation {
    pub schema_version: u32,
    pub status: AgentCommandArtifactObservationStatus,
    pub coverage: AgentCommandArtifactObservationCoverage,
    pub changes: Vec<AgentCommandArtifactChange>,
    #[serde(default)]
    pub changes_truncated: bool,
    #[serde(default)]
    pub changes_omitted: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub expected_outputs: Vec<AgentCommandExpectedArtifactOutcome>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<AgentCommandArtifactObservationWarning>,
}

/// Selects a host-owned runtime for one frozen command request.
///
/// This is a resolver hint, not an authorization capability. The original
/// logical command is still evaluated by the normal command policy before the
/// provider is consulted.
#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum AgentCommandRuntimeProvider {
    ManagedArtifact,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum AgentCommandRuntimeKind {
    Node,
    Python,
}

/// Stable, model-visible identifier for an application-owned artifact runtime profile.
///
/// A profile selects a reproducible capability family. It is deliberately not a package
/// request: package names, exact versions, provider identity, and runtime integrity evidence are
/// resolved by the trusted host and frozen in [`AgentCommandRuntimeBinding`].
#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase")]
pub enum AgentCommandRuntimeProfile {
    Documents,
    Spreadsheets,
    Presentations,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentCommandRuntimePackageRequirement {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentCommandRuntimeRequest {
    pub provider: AgentCommandRuntimeProvider,
    pub kind: AgentCommandRuntimeKind,
    pub required_packages: Vec<AgentCommandRuntimePackageRequirement>,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentCommandRuntimeResolvedPackage {
    pub name: String,
    pub version: String,
}

pub const AGENT_COMMAND_RUNTIME_BINDING_SCHEMA_VERSION: u32 = 1;

/// Approval-time identity of a host-resolved runtime profile.
///
/// This value is persisted with the frozen command action. It intentionally excludes executable
/// paths, environment variables, bootstrap paths, and other host-private launch authority. The
/// host resolves the profile again immediately before execution and requires an exact identity
/// match before it starts a process.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentCommandRuntimeBinding {
    pub schema_version: u32,
    pub profile: AgentCommandRuntimeProfile,
    pub profile_revision: String,
    pub provider_id: String,
    pub bundle_version: String,
    pub bundle_revision: String,
    pub kind: AgentCommandRuntimeKind,
    pub runtime_version: String,
    pub runtime_fingerprint: String,
    pub resolved_packages: Vec<AgentCommandRuntimeResolvedPackage>,
}

pub const AGENT_COMMAND_RUNTIME_RESOLUTION_SCHEMA_VERSION: u32 = 2;

/// Public execution evidence for a managed command runtime.
///
/// Executable and component paths are intentionally absent: they are private
/// host implementation details and are never persisted into model-visible
/// tool results.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentCommandRuntimeResolution {
    pub schema_version: u32,
    pub provider_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<AgentCommandRuntimeProfile>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_revision: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bundle_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bundle_revision: Option<String>,
    pub kind: AgentCommandRuntimeKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime_fingerprint: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resolved_packages: Vec<AgentCommandRuntimeResolvedPackage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recovery: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observe: Option<AgentCommandArtifactObservationRequest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Legacy exact-package request retained only so old pending actions can be deserialized and
    /// retired safely. New model calls never populate this field and the host never executes it.
    pub runtime: Option<AgentCommandRuntimeRequest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_binding: Option<Box<AgentCommandRuntimeBinding>>,
}

/// Frozen request to copy one immutable Skill resource, or one resource-tree
/// prefix, into the selected workspace.
///
/// The source is a logical `skill://` URI. Managed-store paths and resource
/// bytes never cross the runtime action protocol.
#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AgentSkillMaterializationRequest {
    pub id: String,
    pub source_uri: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_prefix: Option<String>,
    pub destination: String,
    pub approval_status: AgentApprovalStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AgentSkillMaterializationResult {
    pub status: AgentSkillMaterializationResultStatus,
    pub source_uri: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_prefix: Option<String>,
    pub destination: String,
    pub source_revision: String,
    pub file_count: u64,
    pub byte_count: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Declarative runtime requirements used only for dependency discovery. They
/// never grant permissions and never trigger dependency installation.
#[derive(Debug, Default, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentSkillScriptRequirements {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub python_distributions: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub commands: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentSkillDependencyCheck {
    pub kind: AgentSkillDependencyKind,
    pub name: String,
    pub status: AgentSkillDependencyStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentSkillScriptPreflightReport {
    pub status: AgentSkillScriptPreflightStatus,
    pub interpreter: AgentSkillScriptInterpreter,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interpreter_version: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<AgentSkillDependencyCheck>,
    pub runtime_fingerprint: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Frozen, revision-bound Skill script request. The script URI and digest
/// identify immutable package bytes; arguments remain a structured argv and
/// are never converted to a shell command.
#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AgentSkillScriptRequest {
    pub id: String,
    pub script_uri: String,
    pub skill_id: String,
    pub skill_revision: String,
    pub resource_path: String,
    pub resource_digest: String,
    pub interpreter: AgentSkillScriptInterpreter,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub requirements: AgentSkillScriptRequirements,
    pub preflight: AgentSkillScriptPreflightReport,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    pub approval_status: AgentApprovalStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AgentSkillScriptResult {
    pub script_uri: String,
    pub skill_id: String,
    pub skill_revision: String,
    pub resource_digest: String,
    pub preflight: AgentSkillScriptPreflightReport,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
    pub cancelled: bool,
    pub duration_ms: u64,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum AgentProposedAction {
    ToolCall {
        call: AgentToolCall,
    },
    Diff {
        diff: AgentDiffProposal,
    },
    FileWrite {
        file_write: AgentFileWriteProposal,
    },
    Command {
        command: AgentCommandRequest,
    },
    SkillMaterialization {
        materialization: AgentSkillMaterializationRequest,
    },
    SkillScript {
        script: Box<AgentSkillScriptRequest>,
    },
    OfficeOperation {
        office_operation: Box<AgentOfficeOperationRequest>,
    },
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
    SkillActivated {
        run_id: String,
        skill: AgentSkillActivatedEvent,
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
    ContextCompactionStarted {
        run_id: String,
        operation_id: String,
    },
    ContextCompactionFinished {
        run_id: String,
        operation_id: String,
        outcome: AgentContextCompactionEventOutcome,
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

#[derive(Debug, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentContextCompactionEventOutcome {
    Applied,
    Skipped,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone)]
pub struct AgentError {
    message: String,
    cancelled: bool,
    usage: Option<Box<AgentUsage>>,
    code: Option<String>,
    details: Option<Box<Value>>,
    conversation_turn_trace: Option<Box<ConversationTurnTrace>>,
    model_request_observation: Option<Box<crate::ModelRequestObservation>>,
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
            conversation_turn_trace: None,
            model_request_observation: None,
        }
    }

    pub fn structured(code: impl Into<String>, message: impl Into<String>, details: Value) -> Self {
        Self {
            message: message.into(),
            cancelled: false,
            usage: None,
            code: Some(code.into()),
            details: Some(Box::new(details)),
            conversation_turn_trace: None,
            model_request_observation: None,
        }
    }

    pub fn cancelled() -> Self {
        Self {
            message: "agent run 已取消。".to_string(),
            cancelled: true,
            usage: None,
            code: None,
            details: None,
            conversation_turn_trace: None,
            model_request_observation: None,
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

    pub fn conversation_turn_trace(&self) -> Option<&ConversationTurnTrace> {
        self.conversation_turn_trace.as_deref()
    }

    pub fn model_request_observation(&self) -> Option<&crate::ModelRequestObservation> {
        self.model_request_observation.as_deref()
    }

    pub fn with_usage(mut self, usage: Option<AgentUsage>) -> Self {
        self.usage = usage.map(Box::new);
        self
    }

    pub fn with_conversation_turn_trace(mut self, trace: ConversationTurnTrace) -> Self {
        self.conversation_turn_trace = Some(Box::new(trace));
        self
    }

    pub fn with_model_request_observation(
        mut self,
        observation: crate::ModelRequestObservation,
    ) -> Self {
        self.model_request_observation = Some(Box::new(observation));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::completed_conversation_trace_without_items;
    use serde_json::json;

    #[test]
    fn skill_activated_event_uses_the_stable_frontend_contract() {
        let value = serde_json::to_value(AgentEvent::SkillActivated {
            run_id: "run-1".to_string(),
            skill: AgentSkillActivatedEvent {
                id: "bundled:application:documents".to_string(),
                name: "documents".to_string(),
                revision: "skill-package-sha256-v2:test".to_string(),
                source: "bundled:application".to_string(),
                activated_by: AgentSkillActivationActor::Model,
            },
        })
        .unwrap();

        assert_eq!(value["type"], "skill_activated");
        assert_eq!(value["runId"], "run-1");
        assert_eq!(value["skill"]["activatedBy"], "model");
        assert_eq!(value["skill"]["name"], "documents");
    }

    #[test]
    fn model_capabilities_are_camel_case_and_legacy_agent_inputs_fail_closed() {
        let mut input = serde_json::from_value::<AgentChatInput>(json!({
            "apiUrl": "https://example.test/v1/chat/completions",
            "apiToken": "secret",
            "model": "text-only-model",
            "messages": []
        }))
        .unwrap();

        assert!(!input.model_capabilities.image_input);

        input.model_capabilities.image_input = true;
        let serialized = serde_json::to_value(&input).unwrap();
        assert_eq!(serialized["modelCapabilities"]["imageInput"], true);
        assert!(serialized.get("model_capabilities").is_none());

        let round_trip = serde_json::from_value::<AgentChatInput>(serialized).unwrap();
        assert!(round_trip.model_capabilities.image_input);
    }

    #[test]
    fn command_safety_policy_defaults_to_guarded_for_legacy_permissions() {
        let permissions: AgentPermissions = serde_json::from_value(json!({
            "read": "all",
            "write": "all",
            "command": "auto_approve",
            "patch": "auto_approve"
        }))
        .unwrap();

        assert_eq!(
            permissions.command_safety,
            AgentCommandSafetyPolicy::Guarded
        );
    }

    #[test]
    fn command_safety_policy_uses_stable_camel_case_field_and_wire_value() {
        let permissions = AgentPermissions {
            read: AgentReadPermission::All,
            write: AgentWritePermission::All,
            command: AgentCommandPermission::AutoApprove,
            command_safety: AgentCommandSafetyPolicy::FullAccess,
            patch: AgentPatchPermission::AutoApprove,
        };

        let serialized = serde_json::to_value(permissions).unwrap();

        assert_eq!(serialized["commandSafety"], "full_access");
        assert!(serialized.get("command_safety").is_none());
    }

    #[test]
    fn chat_message_serializes_conversation_trace_with_camel_case_protocol_names() {
        let message = AgentChatMessage {
            message_id: None,
            role: "assistant".to_string(),
            content: "done".to_string(),
            created_at: Some(0),
            conversation_turn_trace: Some(completed_conversation_trace_without_items(
                "run-1",
                "conversation-1",
                "assistant-1",
            )),
        };
        let serialized = serde_json::to_string(&message).unwrap();

        assert!(serialized.contains("\"conversationTurnTrace\""));
        assert!(serialized.contains("\"createdAt\":0"));
        assert!(!serialized.contains("conversation_turn_trace"));
    }

    #[test]
    fn skill_activation_uses_camel_case_and_redacts_instructions_from_debug() {
        let activation = AgentSkillActivation {
            activation_revision: "activation-sha256-v1:test".to_string(),
            skills: vec![AgentActivatedSkill {
                id: "workspace:w:review".to_string(),
                name: "review".to_string(),
                revision: "skill-sha256-v1:test".to_string(),
                source: "workspace".to_string(),
                instructions: "PRIVATE_SKILL_INSTRUCTIONS".to_string(),
                source_bytes: 26,
                resources: None,
            }],
        };

        let serialized = serde_json::to_value(&activation).unwrap();
        assert_eq!(
            serialized["activationRevision"],
            "activation-sha256-v1:test"
        );
        assert_eq!(
            serialized["skills"][0]["instructions"],
            "PRIVATE_SKILL_INSTRUCTIONS"
        );
        assert!(serialized.get("activation_revision").is_none());
        assert!(!format!("{activation:?}").contains("PRIVATE_SKILL_INSTRUCTIONS"));
    }

    #[test]
    fn context_compaction_events_serialize_stable_identity_and_outcome() {
        let started = serde_json::to_value(AgentEvent::ContextCompactionStarted {
            run_id: "run-1".to_string(),
            operation_id: "compaction-1".to_string(),
        })
        .unwrap();
        let finished = serde_json::to_value(AgentEvent::ContextCompactionFinished {
            run_id: "run-1".to_string(),
            operation_id: "compaction-1".to_string(),
            outcome: AgentContextCompactionEventOutcome::Applied,
        })
        .unwrap();

        assert_eq!(started["type"], "context_compaction_started");
        assert_eq!(started["operationId"], "compaction-1");
        assert_eq!(finished["type"], "context_compaction_finished");
        assert_eq!(finished["operationId"], started["operationId"]);
        assert_eq!(finished["outcome"], "applied");
    }
}
