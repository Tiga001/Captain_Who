use crate::agent_graph::AgentCollaborationIdentity;
use crate::context::ContextCompactionSummary;
use crate::conversation_trace::{
    ConversationModelContextItem, ConversationTraceAttachment, ConversationTurnTrace,
    ConversationTurnTraceItem,
};
use crate::provider_profile::{ProviderProfileConfig, ProviderProtocolKey};
use crate::world_state::{AnchoredWorldStateRecord, WorldStateSnapshot};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::error::Error;
use std::fmt::{Display, Formatter};

pub const PROVIDER_CONTINUATION_REF_VERSION: u32 = 1;

/// Opaque, conversation-bound handle to provider-owned continuation state.
///
/// The referenced payload is stored in the Host's private encrypted vault. This value is safe to
/// persist in an approval checkpoint, but it deliberately carries no payload hash, ciphertext,
/// provider text, or storage location.
#[derive(Deserialize, Serialize, Clone, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderContinuationRef {
    pub version: u32,
    pub id: String,
}

impl ProviderContinuationRef {
    pub fn new() -> Self {
        Self {
            version: PROVIDER_CONTINUATION_REF_VERSION,
            id: format!(
                "provider-continuation-v1:{}",
                uuid::Uuid::new_v4().hyphenated()
            ),
        }
    }

    pub fn parse(version: u32, id: impl Into<String>) -> Result<Self, String> {
        let value = Self {
            version,
            id: id.into(),
        };
        value.validate()?;
        Ok(value)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.version != PROVIDER_CONTINUATION_REF_VERSION {
            return Err("provider continuation ref version is unsupported".to_string());
        }
        let Some(raw_uuid) = self.id.strip_prefix("provider-continuation-v1:") else {
            return Err("provider continuation ref prefix is invalid".to_string());
        };
        let parsed = uuid::Uuid::parse_str(raw_uuid)
            .map_err(|_| "provider continuation ref UUID is invalid".to_string())?;
        if parsed.get_version_num() != 4 || parsed.hyphenated().to_string() != raw_uuid {
            return Err("provider continuation ref UUID is not canonical v4".to_string());
        }
        Ok(())
    }
}

impl Default for ProviderContinuationRef {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for ProviderContinuationRef {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ProviderContinuationRef([REDACTED])")
    }
}

/// Backend-authoritative capabilities frozen for one logical model run.
///
/// This provider-neutral contract is intentionally separate from tool
/// definitions. Tools remain registered consistently for prompt-cache
/// stability and enforce unsupported capabilities at execution time.
#[derive(Debug, Default, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelCapabilities {
    pub image_input: bool,
}

#[derive(Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AgentChatInput {
    pub api_url: String,
    pub api_token: String,
    /// Opaque Host-only identity of this model's effective provider wire protocol.
    ///
    /// This is the current per-model `provider-protocol-v1` revision. Effective wire changes rotate
    /// it independently. It never crosses normal Serde boundaries or provider payloads.
    #[serde(skip)]
    pub provider_configuration_revision: Option<String>,
    /// Host-only stable identity of this model's effective endpoint/token pair. It is kept
    /// separately from the protocol revision for credential restoration and CAS checks.
    #[serde(skip)]
    pub provider_connection_revision: Option<String>,
    /// Host-only stable identity of the effective search mode/credential pair.
    #[serde(skip)]
    pub search_connection_revision: Option<String>,
    /// Host-only provider protocol configuration frozen before runtime preparation.
    #[serde(skip)]
    pub provider_profile_config: Option<ProviderProfileConfig>,
    /// Host-only immutable provenance for provider-owned assistant state. Hosts freeze this before
    /// a run; current direct Core callers may omit the key and derive it from the required Profile.
    #[serde(skip)]
    pub provider_protocol_key: Option<ProviderProtocolKey>,
    pub model: String,
    /// Resolved by the backend from the selected model configuration and kept
    /// immutable across approval pause/resume for this logical run.
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
    /// Backend-owned, provider-neutral world-state journal for the active conversation epoch.
    ///
    /// A full snapshot establishes the epoch prelude and later diffs are anchored immediately
    /// before the conversation message that first observed them. The runtime renders only each
    /// record's explicitly sanitized model projection.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub world_state_records: Vec<AnchoredWorldStateRecord>,
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

impl std::fmt::Debug for AgentChatInput {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("AgentChatInput([REDACTED])")
    }
}

#[derive(Default, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentSkillActivation {
    pub activation_revision: String,
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
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentActivatedSkill {
    pub id: String,
    pub name: String,
    pub revision: String,
    pub source: String,
    pub instructions: String,
    /// Exact verified SKILL.md source size used for aggregate activation policy enforcement.
    pub source_bytes: u64,
    /// Lightweight discovery hint for the run-scoped Resource Runtime. The
    /// resource index and bytes remain behind the host capability.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resources: Option<AgentActivatedSkillResources>,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
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
pub struct AgentSkillSourceSummary {
    pub kind: String,
    pub id: String,
}

/// The same presentation-safe Skill identity returned for explicit activation at turn start.
/// Runtime activation metadata deliberately excludes instructions and resource locations.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentSkillActivatedEvent {
    pub id: String,
    pub name: String,
    pub revision: String,
    pub source: AgentSkillSourceSummary,
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

pub(crate) fn deserialize_required_nullable<'de, D, T>(
    deserializer: D,
) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentExtensionSnapshot {
    pub extension_id: String,
    pub version: u32,
    pub state: Value,
}

/// Current durable Agent run checkpoint schema.
///
/// Version 9 freezes model-visible Agent collaboration selector capabilities and removes the
/// retired Provider-specific Skill-activation sibling deferral bit. Tools exposed in the request
/// that produced a batch remain executable under that frozen ToolSet; newly activated Skill
/// instructions and Tool capabilities become visible only on the next model request.
/// The referenced payload remains encrypted in the Host vault; raw Provider continuation and
/// reasoning are never serialized into the checkpoint. Any other schema version is rejected at
/// the approval boundary.
pub const AGENT_RUN_CHECKPOINT_SCHEMA_VERSION: u32 = 9;

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentRunToolSetCheckpoint {
    pub stable_revision: String,
    pub dynamic_revision: String,
    pub effective_revision: String,
    /// Sorted backend-owned capabilities used to build the exact request contract that produced
    /// the paused Tool batch. Extension snapshots may already include successful effects from an
    /// earlier sibling, so resume must not infer this boundary from their newer state.
    pub active_capability_ids: Vec<String>,
    pub exposed_tool_names: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentRunCheckpoint {
    pub version: u32,
    pub run_id: String,
    pub context_items: Vec<AgentContextCheckpointItem>,
    pub next_model_request_index: usize,
    pub queued_tool_calls: Vec<AgentQueuedToolCallCheckpoint>,
    /// Number of additional MCP calls discarded behind the pending MCP approval.
    ///
    /// These calls had no prepared one-time invocation identity and therefore cannot be resumed
    /// safely. Raw calls and arguments are deliberately absent; restore emits only a fixed Host
    /// diagnostic instructing the model to prepare new calls after the approved continuation.
    pub deferred_external_tool_call_count: u32,
    pub suppressed_narration: bool,
    pub extension_snapshots: Vec<AgentExtensionSnapshot>,
    /// Exact model-facing and execution-authorizing Tool contract for the response being paused.
    pub tool_set: AgentRunToolSetCheckpoint,
    /// Backend execution authority frozen at the approval boundary.
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub run_context: Option<AgentRunContext>,
    /// Host-authenticated collaboration authority for this exact logical Turn. It is the same
    /// bounded directory shown to the model, plus durable admission of wait_agent per batch.
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub collaboration_run_snapshot: Option<crate::AgentCollaborationRunSnapshot>,
    /// Provider-neutral capabilities frozen for the same logical run.
    pub model_capabilities: ModelCapabilities,
    /// Exact provider protocol settings used by the run before it paused.
    pub provider_profile_config: ProviderProfileConfig,
    /// Provider/dialect/model/settings provenance used to reject cross-profile replay.
    pub provider_protocol_key: ProviderProtocolKey,
    /// Ordered identity-only projection of the assistant Tool Call batch. It binds durable
    /// runtime calls to provider calls without persisting raw reasoning or continuation payloads.
    pub assistant_turn_identity: AgentAssistantTurnCheckpointIdentity,
    /// Ordered, payload-free handles for every Provider continuation retained by this checkpoint.
    /// The Host must resolve and validate every handle before executing an approved tool or
    /// sending another Provider request.
    pub provider_continuation_refs: Vec<ProviderContinuationRef>,
    /// Exact authoritative Run-lifetime World State. Resume rebases this snapshot into a fresh
    /// epoch; it never reconstructs authority from rendered model context.
    pub run_world_state: WorldStateSnapshot,
    /// Approval-record identity. MCP uses an application UUID independent of the provider Tool
    /// Call identity. Built-in checkpoints may omit this field and use `pending_tool_call_id`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pending_action_id: Option<String>,
    pub pending_tool_call_id: String,
    pub conversation_trace_items: Vec<ConversationTurnTraceItem>,
    /// Bounded, replay-safe projection of the active run's uncompressed model timeline.
    ///
    /// Process-only MCP arguments may be redacted even when the current in-memory model loop
    /// retains them.
    pub conversation_model_context_items: Vec<ConversationModelContextItem>,
    pub next_conversation_trace_sequence: u64,
    pub conversation_trace_truncated: bool,
}

#[derive(Deserialize, Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
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
    #[serde(skip_serializing_if = "Option::is_none")]
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
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentContextCheckpointOrigin {
    pub kind: String,
    pub id: String,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentContextCheckpointImage {
    pub mime_type: String,
    pub data_base64: String,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentContextCheckpointToolCall {
    /// Application-owned opaque identity shared by execution, audit, persistence, and model I/O.
    ///
    /// This value is never a provider's raw ID and consumers must not parse its representation.
    pub id: String,
    pub name: String,
    pub args: Value,
    /// Exact provider/runtime identity frozen when the Assistant Tool Call entered model context.
    ///
    /// A provider-neutral split projection still has an explicit identity whose provider and
    /// runtime IDs are equal. Persisted context must never reconstruct this mapping from position
    /// or a rendered tool name.
    pub provider_identity: AgentProviderToolCallIdentity,
}

#[derive(Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentProviderToolCallIdentity {
    pub provider_tool_index: u32,
    pub provider_call_id: String,
    pub runtime_call_id: String,
}

impl std::fmt::Debug for AgentProviderToolCallIdentity {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("AgentProviderToolCallIdentity([REDACTED])")
    }
}

#[derive(Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentAssistantTurnCheckpointIdentity {
    pub assistant_turn_id: String,
    pub assistant_turn_digest: String,
    pub tool_call_identities: Vec<AgentProviderToolCallIdentity>,
}

impl std::fmt::Debug for AgentAssistantTurnCheckpointIdentity {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("AgentAssistantTurnCheckpointIdentity([REDACTED])")
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentContextCheckpointGroup {
    pub id: String,
    pub kind: String,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentQueuedToolCallCheckpoint {
    pub call: AgentContextCheckpointToolCall,
    pub assistant_content: String,
    pub group_id: String,
    /// Reference into `AgentRunCheckpoint.assistant_turn_identity.tool_call_identities`.
    pub assistant_turn_id: String,
    pub provider_tool_index: u32,
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
    /// Backend-only uncompressed model projection for this assistant turn. Renderer clients never
    /// author this field; Core validates it against the durable trace before use.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conversation_model_context_items: Vec<ConversationModelContextItem>,
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

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
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

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentGuidanceStatus {
    Queued,
    Applied,
    Rejected,
    Abandoned,
}

impl AgentGuidanceStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Applied => "applied",
            Self::Rejected => "rejected",
            Self::Abandoned => "abandoned",
        }
    }

    pub(crate) fn from_str(value: &str) -> Option<Self> {
        match value {
            "queued" => Some(Self::Queued),
            "applied" => Some(Self::Applied),
            "rejected" => Some(Self::Rejected),
            "abandoned" => Some(Self::Abandoned),
            _ => None,
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentSteerRunInput {
    pub conversation_id: String,
    pub expected_run_id: String,
    pub client_message_id: String,
    pub content: String,
    #[serde(default)]
    pub attachments: Vec<AgentInputAttachment>,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentSteerRunResultStatus {
    Queued,
    Applied,
    Rejected,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentSteerRunRejectionCode {
    RunNotSteerable,
    RunInterrupted,
    ConversationMismatch,
    IdentityConflict,
    AttachmentsNotSupported,
    ModelDoesNotSupportAttachments,
    AttachmentValidationFailed,
    AttachmentLimitExceeded,
    AttachmentPersistenceFailed,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentSteerRunOutput {
    pub guidance_id: String,
    pub status: AgentSteerRunResultStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rejection_code: Option<AgentSteerRunRejectionCode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Validated, run-scoped input consumed by the agent loop at a safe sampling boundary.
///
/// The Host owns durable admission and attachment persistence. The runtime only receives inputs
/// that have already been associated with the expected active run.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentSteerInput {
    pub guidance_id: String,
    pub client_message_id: String,
    pub content: String,
    #[serde(default)]
    pub attachments: Vec<AgentInputAttachment>,
    /// Host-authoritative attachment library including this guidance's persisted attachments.
    ///
    /// The runtime installs this snapshot only when the guidance is applied at a safe model
    /// boundary. Merely admitting an RPC must never expand the active tool context.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachment_library: Option<AgentAttachmentLibraryContext>,
    pub created_at: i64,
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
/// `Denied` blocks every file-changing call at runtime and host boundaries. Stable file-edit
/// tools remain in the model-visible tool prefix so changing the composer permission does not
/// invalidate that prefix; visibility never grants write authority. Dynamic capabilities may
/// still be omitted when they have no permitted operation. `WorkspaceOnly` allows safe writes
/// only inside the selected workspace. `All` also allows safe writes outside the workspace
/// through absolute paths or supported system aliases.
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
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentPermissions {
    pub read: AgentReadPermission,
    pub write: AgentWritePermission,
    pub command: AgentCommandPermission,
    pub command_safety: AgentCommandSafetyPolicy,
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

impl AgentPermissions {
    /// Returns the component-wise intersection of two permission ceilings.
    ///
    /// This is deliberately a meet operation rather than an override: a child, template, project,
    /// or dynamic policy can only retain or remove authority already present in the other input.
    pub fn meet(self, ceiling: Self) -> Self {
        Self {
            read: match (self.read, ceiling.read) {
                (AgentReadPermission::All, AgentReadPermission::All) => AgentReadPermission::All,
                _ => AgentReadPermission::WorkspaceOnly,
            },
            write: match (self.write, ceiling.write) {
                (AgentWritePermission::All, AgentWritePermission::All) => AgentWritePermission::All,
                (AgentWritePermission::Denied, _) | (_, AgentWritePermission::Denied) => {
                    AgentWritePermission::Denied
                }
                _ => AgentWritePermission::WorkspaceOnly,
            },
            command: match (self.command, ceiling.command) {
                (AgentCommandPermission::AutoApprove, AgentCommandPermission::AutoApprove) => {
                    AgentCommandPermission::AutoApprove
                }
                _ => AgentCommandPermission::RequireApproval,
            },
            command_safety: match (self.command_safety, ceiling.command_safety) {
                (AgentCommandSafetyPolicy::FullAccess, AgentCommandSafetyPolicy::FullAccess) => {
                    AgentCommandSafetyPolicy::FullAccess
                }
                _ => AgentCommandSafetyPolicy::Guarded,
            },
            patch: match (self.patch, ceiling.patch) {
                (AgentPatchPermission::AutoApprove, AgentPatchPermission::AutoApprove) => {
                    AgentPatchPermission::AutoApprove
                }
                _ => AgentPatchPermission::RequireApproval,
            },
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
#[serde(rename_all = "camelCase", deny_unknown_fields)]
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

#[derive(Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AgentSearchConfig {
    pub mode: AgentSearchMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tavily_api_key: Option<String>,
}

impl std::fmt::Debug for AgentSearchConfig {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("AgentSearchConfig([REDACTED])")
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AgentApprovalDecision {
    pub action_id: String,
    pub status: AgentApprovalDecisionStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentRunContext {
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub conversation_id: Option<String>,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub project_id: Option<String>,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub workspace: Option<AgentWorkspaceContext>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attachment_library: Option<AgentAttachmentLibraryContext>,
    pub permissions: AgentPermissions,
    /// Host-authenticated collaboration identity for a child Agent turn.
    ///
    /// This is never accepted from renderer RPC input. It is reconstructed from the durable
    /// Agent/Wake/Mailbox bundle and remains inside the shared run context across pause/resume.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collaboration_identity: Option<AgentCollaborationIdentity>,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentWorkspaceContext {
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub project_id: Option<String>,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub display_name: Option<String>,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub root_path: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentAttachmentLibraryContext {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub root_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    pub conversation_attachments: Vec<AgentAttachmentReference>,
    pub project_attachments: Vec<AgentAttachmentReference>,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
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

/// One model-visible, purpose-limited file input.
///
/// This is a logical reference, not a filesystem path grant. Trusted adapters resolve the
/// reference against the current run authority, freeze its content identity, and revalidate that
/// identity before bytes cross an execution boundary.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum AgentFileInputRef {
    /// Exact `readPath` returned by the attachment library.
    Attachment { read_path: String },
    /// Workspace-relative path. Absolute paths are deliberately rejected for this variant.
    Workspace { path: String },
    /// Absolute path or supported system-path alias. Requires read=all.
    External { path: String },
    /// Immutable application Artifact. The URI is resolved through the authoritative publication
    /// registry; `path` is only an exact-consistency hint from the prior Tool Result.
    GeneratedArtifact { uri: String, path: String },
    /// Exact revision-bound `skill://` URI from an activated Skill.
    SkillResource { uri: String },
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentFileInputSpec {
    /// Stable relative path beneath the private input root exposed to the managed process.
    pub mount_path: String,
    pub source: AgentFileInputRef,
}

pub const AGENT_FILE_INPUT_BINDING_SCHEMA_VERSION: u32 = 1;

/// Approval-time content identity for one declarative file input.
///
/// Host-private source paths and the temporary materialization root are intentionally absent.
/// Workspace/external paths remain logical user-authored references; attachment and Skill store
/// paths never enter this contract.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentFileInputBinding {
    pub schema_version: u32,
    pub mount_path: String,
    pub source: AgentFileInputRef,
    pub size_bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentFileInputSourceKind {
    Attachment,
    Workspace,
    External,
    GeneratedArtifact,
    SkillResource,
}

/// Presentation-safe execution evidence for a materialized input.
///
/// This deliberately records no source or temporary filesystem path.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentFileInputEvidence {
    pub mount_path: String,
    pub source_kind: AgentFileInputSourceKind,
    pub size_bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentContextWindowStatus {
    Unconfigured,
    WithinBudget,
    OverBudget,
    InvalidConfiguration,
}

#[derive(Debug, Default, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentContextCostBreakdown {
    pub system_tokens: u64,
    pub tool_schema_tokens: u64,
    pub summary_tokens: u64,
    pub world_state_tokens: u64,
    pub todo_tokens: u64,
    /// Hidden Provider protocol state included in the final wire request. This is a token count
    /// only; no continuation content crosses the API.
    #[serde(default)]
    pub provider_continuation_tokens: u64,
    /// Uncovered history plus current-run messages, attachments, Skills, guards, and tool
    /// protocol that are not represented by the dedicated categories above.
    pub recent_history_tokens: u64,
    pub total_input_tokens: u64,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentContextWindowSnapshot {
    pub model: String,
    pub status: AgentContextWindowStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window_tokens: Option<u64>,
    pub reserved_output_tokens: u64,
    pub safety_margin_tokens: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// Total input capacity after output and safety reserves.
    pub input_capacity_tokens: Option<u64>,
    /// Fully assembled model input after the conversation starts, including fixed contracts,
    /// uncompressed history and current-run overlays. An unstarted conversation publishes zero.
    pub input_tokens: u64,
    #[serde(default)]
    pub cost_breakdown: AgentContextCostBreakdown,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remaining_input_tokens: Option<i64>,
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

/// Stable origin of a tool implementation selected by the trusted registry.
///
/// Provider-visible tool names are presentation/routing keys only. The runtime records this
/// identity separately so durable traces never need to infer authority from a formatted name.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum AgentToolIdentity {
    Builtin {
        tool_name: String,
    },
    RuntimeExtension {
        extension_id: String,
        tool_name: String,
    },
    Mcp {
        provenance: AgentMcpToolProvenance,
    },
    /// Model-authored name that was not present in the frozen trusted registry.
    ///
    /// This identity exists only so rejected calls remain fully attributable in durable audit.
    /// It never grants execution, approval, checkpoint, or resume authority.
    Unregistered {
        tool_name: String,
    },
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum AgentMcpServerScope {
    Builtin,
    User,
    Project { project_id: String },
    Plugin { plugin_id: String },
    Managed,
}

/// Immutable MCP catalog identity frozen into one run's ToolRegistry.
///
/// `model_tool_name` is never parsed to recover the server or raw tool. Invocation uses the
/// remaining typed fields and revalidates the config epoch/revision, digest and Catalog identity
/// at the Host boundary.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentMcpToolProvenance {
    pub server_id: String,
    pub scope: AgentMcpServerScope,
    pub raw_tool_name: String,
    pub model_tool_name: String,
    /// High-entropy identity of this concrete Server configuration instance.
    ///
    /// Unlike `config_digest`, this UUID changes whenever a configuration is replaced, including
    /// an ABA transition back to byte-for-byte identical configuration. Approval and checkpoint
    /// revalidation must require an exact match.
    pub config_epoch: String,
    /// Monotonic Registry revision that published this configuration instance.
    ///
    /// Delayed change events may invalidate only approvals whose frozen revision is older than the
    /// event revision; the epoch remains the cross-restart/ABA authority boundary.
    pub registry_revision: u64,
    pub config_digest: String,
    pub catalog_generation: u64,
    pub catalog_digest: String,
    /// Digest supplied by the MCP Catalog for the raw Server descriptor schemas.
    ///
    /// This identity is used only to revalidate the live Catalog route. It is deliberately
    /// distinct from `schema_digest`, which binds the normalized input schema actually exposed to
    /// the model provider.
    pub catalog_schema_digest: String,
    /// Digest of the normalized, Provider-facing input schema.
    pub schema_digest: String,
    /// Version of the Host normalizer that produced `schema_digest`.
    pub schema_normalizer_version: u32,
}

/// Host-facing risk label for an MCP Tool approval.
///
/// Every value remains approval-gated. Server-authored annotations may select a more specific
/// *claimed* label, but never grant execution authority or suppress the prompt.
#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentMcpToolRisk {
    Unknown,
    ReadOnlyClaimed,
    SideEffectsPossible,
    DestructiveClaimed,
    OpenWorldClaimed,
}

/// MCP invocation policy frozen into one exact external Tool invocation.
///
/// `Auto` is a Host-configured per-Server policy. It skips the user prompt, but does not bypass
/// preparation, one-time payload consumption, typed identity checks, cancellation, or output
/// limits.
#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentMcpApprovalMode {
    Prompt,
    Auto,
    Deny,
}

/// Bounded structural description of model-authored MCP arguments.
///
/// Scalar values and property names are intentionally absent: either can contain a credential or
/// user-private value. The full argument object only crosses the non-serializable Host preparation
/// boundary and is never reconstructed from this summary.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentMcpArgumentSummary {
    pub encoded_bytes: u64,
    pub top_level_property_count: u64,
    pub string_value_count: u64,
    pub number_value_count: u64,
    pub boolean_value_count: u64,
    pub null_value_count: u64,
    pub object_value_count: u64,
    pub array_value_count: u64,
    pub max_depth: u32,
    pub truncated: bool,
}

/// Stable identity of one concrete MCP Tool invocation.
///
/// `invocation_id` is application-generated, high entropy, and independent of provider Tool Call
/// IDs. Routing continues to use typed provenance; `arguments_digest` binds a separately sealed
/// payload without placing the payload itself in Protocol, events, traces, or checkpoints.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentMcpToolInvocationIdentity {
    /// Approval-record identity used for user decisions and pending-action CAS.
    ///
    /// This is application-generated and independent of both the invocation id and provider Tool
    /// Call id.
    pub action_id: String,
    /// One-time execution-grant identity used to seal and consume raw arguments.
    pub invocation_id: String,
    pub run_id: String,
    pub call_id: String,
    pub provenance: AgentMcpToolProvenance,
    pub arguments_digest: String,
}

/// Renderer-safe summary of a pending external MCP Tool approval.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentMcpToolApprovalSummary {
    pub server_id: String,
    pub server_display_name: String,
    pub scope: AgentMcpServerScope,
    pub raw_tool_name: String,
    pub model_tool_name: String,
    /// Bounded model-authored explanation stored independently from Server arguments.
    ///
    /// It is never reconstructed from raw MCP arguments and is never sent to the MCP Server.
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub display_reason: Option<String>,
    pub arguments: AgentMcpArgumentSummary,
    pub risk: AgentMcpToolRisk,
    pub external: bool,
}

/// Safe, non-secret persistence capability frozen for one MCP approval payload.
///
/// This marker is part of the approval's authenticated identity. It never contains an opaque
/// payload reference, ciphertext, credential reference, or key material.
#[derive(Debug, Default, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentMcpApprovalPayloadPersistence {
    #[default]
    ProcessOnly,
    DurableAuthenticatedEnvelope,
}

/// Public, persistable approval DTO for one MCP invocation.
///
/// `call.args` is always the Tool-owned safe projection (currently an empty object). The raw
/// arguments are delivered separately to `McpToolInvoker::prepare_approval` and must be sealed by
/// the Host before this action is published.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentMcpToolApproval {
    pub identity: AgentMcpToolInvocationIdentity,
    pub call: AgentToolCall,
    pub summary: AgentMcpToolApprovalSummary,
    pub approval_mode: AgentMcpApprovalMode,
    /// Frozen Host payload capability. This is safe to persist and present, but does not expose
    /// the payload's location or encrypted representation.
    pub payload_persistence: AgentMcpApprovalPayloadPersistence,
    pub created_at: i64,
    pub expires_at: i64,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentMcpToolInvocationState {
    PendingApproval,
    Approved,
    Dispatching,
    Running,
    Completed,
    Failed,
    Cancelled,
    Rejected,
    Expired,
    PayloadUnavailable,
    PolicyDenied,
    OutcomeUnknown,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentMcpToolInvocationOutcome {
    Succeeded,
    ToolError,
    OutputTooLarge,
    TransportError,
    TimedOut,
    Cancelled,
    Rejected,
    Expired,
    PayloadUnavailable,
    PolicyDenied,
    OutcomeUnknown,
}

/// Host-classified certainty about whether one MCP invocation crossed the external dispatch
/// boundary.
///
/// This is deliberately independent of retryability. In particular, `PossiblyDispatched` must
/// never be interpreted as permission to replay the Tool Call.
#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentMcpDispatchCertainty {
    DefinitelyNotDispatched,
    PossiblyDispatched,
    ResponseReceived,
}

/// Host-classified lifecycle boundary at which an MCP invocation failed.
///
/// This intentionally carries no Server text, transport message, arguments, result content,
/// configuration, or secret reference. It is safe for structured diagnostics and strict IPC.
#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentMcpInvocationFailureStage {
    Preflight,
    ApprovalPayload,
    Policy,
    Dispatch,
    Transport,
    ServerResponse,
    ResultProjection,
    Persistence,
    Shutdown,
}

/// Bounded size-only summary of the Host projection of an MCP Tool response.
///
/// Counts describe the already bounded in-process projection, never the untrusted raw protocol
/// body. No content, MIME data, URI, path, or Server-authored message is retained here.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AgentMcpResultSizeSummary {
    pub content_block_count: u64,
    pub text_bytes: u64,
    pub structured_bytes: u64,
    pub omitted_block_count: u64,
    pub omitted_encoded_bytes: u64,
}

/// Versioned, value-free diagnostics for one MCP invocation lifecycle transition.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AgentMcpInvocationDiagnostics {
    pub schema_version: u32,
    pub argument_encoded_bytes: u64,
    pub argument_value_count: u64,
    pub argument_max_depth: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<AgentMcpResultSizeSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure_stage: Option<AgentMcpInvocationFailureStage>,
}

/// Raw-argument-free lifecycle record suitable for AgentEvent and Renderer projection.
///
/// `display_reason` is bounded model-authored display text. It remains untrusted, may contain
/// user-provided sensitive text, and must only be rendered as plain text or persisted through the
/// explicitly allowlisted chat projection.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentMcpToolInvocationEvent {
    pub action_id: String,
    pub invocation_id: String,
    pub call_id: String,
    pub server_id: String,
    pub server_display_name: String,
    pub raw_tool_name: String,
    pub model_tool_name: String,
    /// Bounded model-authored explanation copied from the frozen approval summary.
    ///
    /// This is never reconstructed from raw MCP arguments and is never forwarded to the Server.
    /// It remains untrusted display text.
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub display_reason: Option<String>,
    pub external: bool,
    pub state: AgentMcpToolInvocationState,
    pub dispatch_certainty: AgentMcpDispatchCertainty,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub outcome: Option<AgentMcpToolInvocationOutcome>,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub is_error: Option<bool>,
    /// Bounded Host-classified code only; never a Server message or transport diagnostic.
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub error_code: Option<String>,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub duration_ms: Option<u64>,
    pub output_truncated: bool,
    /// Size-only and Host-classified diagnostics.
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub diagnostics: Option<AgentMcpInvocationDiagnostics>,
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
/// Version 6 binds the original flat semantic model request, its compiled
/// provider-neutral operation request, and a normalized, non-empty user-facing
/// reason of at most [`AGENT_OFFICE_REASON_MAX_CHARS`] characters to the same
/// frozen Office action. The Host re-parses and recompiles `semantic_args`
/// before execution, so neither approval nor history recovery ever has to infer
/// model intent from provider parameters. Provider argv remains trusted
/// Host-owned state. Older actions must be prepared again.
pub const AGENT_OFFICE_OPERATION_SCHEMA_VERSION: u32 = 6;

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
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentOfficeOperationRequest {
    pub schema_version: u32,
    pub id: String,
    pub semantic_args: Value,
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

/// Renderer-safe lifecycle state for a Host-owned command Session.
///
/// This is intentionally separate from [`AgentRunStatus`] and approval state. A command Session
/// may remain `running` after the Agent Run which created it has completed.
#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentCommandSessionStatus {
    Starting,
    Running,
    Exited,
    Interrupted,
    TimedOut,
    Failed,
    OutcomeUnknown,
}

impl AgentCommandSessionStatus {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Exited | Self::Interrupted | Self::TimedOut | Self::Failed | Self::OutcomeUnknown
        )
    }
}

/// Terminal outcomes carried by `command_exited`; an explicit user/Host interruption uses the
/// separate `command_interrupted` event and therefore cannot be mislabeled here.
#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentCommandExitStatus {
    Exited,
    TimedOut,
    Failed,
    OutcomeUnknown,
}

/// Complete bounded Host projection of one managed command Session.
///
/// Approval payloads, raw permission records, process identifiers, environment variables and
/// unbounded output are deliberately excluded from this cross-process DTO.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AgentCommandSessionSnapshot {
    pub schema_version: u32,
    pub session_id: String,
    pub conversation_id: String,
    pub assistant_message_id: String,
    pub origin_run_id: String,
    pub call_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    pub command: String,
    pub cwd: String,
    pub command_digest: String,
    pub status: AgentCommandSessionStatus,
    pub started_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    pub latest_sequence: u64,
    pub output_truncated: bool,
    /// Presentation-safe immutable receipts for outputs published by this terminal Session.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outputs: Vec<crate::command::AgentCommandPublishedOutput>,
    /// Best-effort, bounded Office file-effect evidence captured for this terminal Session.
    ///
    /// Active Sessions never expose an observation. Absence on a terminal Session means the
    /// command did not request Office observation; it must not be interpreted as "no changes".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact_observation: Option<AgentCommandArtifactObservation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub archive_ref: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AgentCommandSessionOutputChunk {
    pub sequence: u64,
    pub stream: AgentCommandOutputStream,
    pub output: String,
}

/// Cross-process upper bound for one Host transcript projection.
///
/// Keep the TypeScript protocol constant with the same name and value in sync. Host hydration,
/// live transcript retention, and the operational model cursor all use this resource bound, while
/// immutable receipts remain a separate exact-replay projection.
pub const AGENT_COMMAND_SESSION_MAX_TRANSCRIPT_CHUNKS: usize = 2_048;

/// Non-destructive, cursor-addressed transcript projection for Host reload recovery.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AgentCommandSessionTranscript {
    pub requested_after_sequence: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_available_sequence: Option<u64>,
    pub latest_sequence: u64,
    pub truncated_before: bool,
    pub output_capture_truncated: bool,
    pub chunks: Vec<AgentCommandSessionOutputChunk>,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AgentCommandSessionListInput {
    pub conversation_id: String,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AgentCommandSessionListOutput {
    pub sessions: Vec<AgentCommandSessionSnapshot>,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AgentCommandSessionGetInput {
    pub conversation_id: String,
    pub session_id: String,
    #[serde(default)]
    pub after_sequence: Option<u64>,
    #[serde(default)]
    pub max_bytes: Option<usize>,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AgentCommandSessionGetOutput {
    pub session: AgentCommandSessionSnapshot,
    pub transcript: AgentCommandSessionTranscript,
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

#[derive(Deserialize, Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentToolCall {
    /// Application-owned opaque identity shared by the call's entire lifecycle.
    ///
    /// The runtime assigns it once when accepting a model response. Execution, approval, events,
    /// audit, checkpoints, traces, results, and subsequent model requests must reuse it verbatim.
    /// UI and host consumers must not parse or synthesize this value.
    pub id: String,
    pub tool: String,
    pub args: Value,
    pub approval_status: AgentApprovalStatus,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub reason: Option<String>,
}

impl std::fmt::Debug for AgentToolCall {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("AgentToolCall([REDACTED])")
    }
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

#[derive(Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AgentToolResult {
    pub call_id: String,
    pub tool: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Backend-only exact projection materialized from streaming captures.
    ///
    /// This file never crosses Protocol, Event, Trace, Checkpoint, or audit serialization. The
    /// generic Exact History boundary consumes it in preference to serializing the bounded
    /// in-memory result.
    #[serde(skip, default)]
    pub exact_archive_file: Option<crate::exact_capture::ExactToolResultArchiveFile>,
}

impl std::fmt::Debug for AgentToolResult {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("AgentToolResult([REDACTED])")
    }
}

/// Version of the presentation-safe image-generation result returned by the Agent Tool.
///
/// This contract deliberately excludes provider endpoints, credentials, ephemeral or signed
/// output URLs, managed-store paths, and input image bytes. Those values remain inside the
/// configuration, execution, and Artifact boundaries respectively.
pub const AGENT_IMAGE_GENERATION_RESULT_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum AgentImageGenerationResultStatus {
    Succeeded,
    Failed,
    Cancelled,
    OutcomeIndeterminate,
    CommitIndeterminate,
}

/// Model-visible image operation. Provider status probes are intentionally not Agent results.
#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum AgentImageGenerationOperation {
    Generate,
    Edit,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum AgentImageGenerationArtifactKind {
    Image,
}

/// Immutable, presentation-safe identity for one successfully published generated image.
///
/// The URI is an application-owned `image-artifact://` capability, never a provider URL or a
/// filesystem path. A terminal result may expose this structure only when publication completed
/// successfully.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentImageGenerationArtifact {
    pub artifact_id: String,
    pub uri: String,
    pub kind: AgentImageGenerationArtifactKind,
    pub format: crate::image_generation::ImageArtifactFormat,
    pub mime_type: String,
    pub width: u32,
    pub height: u32,
    pub size_bytes: u64,
    pub sha256: String,
}

/// Correlation and execution metadata safe to persist in Agent traces and emit to clients.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentImageGenerationAudit {
    pub execution_id: String,
    pub request_fingerprint: String,
    pub provider_profile_id: String,
    pub adapter_id: String,
    pub profile_revision: u64,
    pub model_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_request_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_status: Option<u16>,
    pub created_at: i64,
    pub completed_at: i64,
    pub duration_ms: u64,
}

/// Stable failure details and explicit uncertainty markers for terminal execution outcomes.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentImageGenerationFailure {
    pub code: crate::image_generation::ImageGenerationExecutionFailureCode,
    pub phase: crate::image_generation::ImageGenerationExecutionPhase,
    pub message: String,
    pub recovery: String,
    pub retryable: bool,
    pub generation_may_have_succeeded: bool,
    pub provider_succeeded: bool,
    pub artifact_commit_may_have_succeeded: bool,
}

/// Terminal Tool result for image generation and image editing.
///
/// `succeeded` requires exactly one `artifact` and no `failure`. Every other status requires a
/// `failure` and must not contain an `artifact`. Producers enforce this invariant before the
/// result is emitted or persisted.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentImageGenerationResult {
    pub schema_version: u32,
    pub status: AgentImageGenerationResultStatus,
    pub operation: AgentImageGenerationOperation,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact: Option<AgentImageGenerationArtifact>,
    pub audit: AgentImageGenerationAudit,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure: Option<AgentImageGenerationFailure>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentDiffProposal {
    pub id: String,
    pub operation: AgentPatchOperation,
    pub file_path: String,
    pub patch: String,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub base_revision: Option<String>,
    #[serde(deserialize_with = "deserialize_required_nullable")]
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
    /// Present only after an application-owned canonical Tool Call ID exists.
    ///
    /// Streaming previews are provisional, so the runtime intentionally leaves this unset instead
    /// of exposing a provider's raw correlation ID.
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
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentFileWriteProposal {
    pub id: String,
    pub draft_id: String,
    pub mode: AgentFileWriteMode,
    pub file_path: String,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub base_revision: Option<String>,
    #[serde(deserialize_with = "deserialize_required_nullable")]
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
pub const AGENT_COMMAND_ARTIFACT_OBSERVATION_SCHEMA_VERSION: u32 = 3;

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
#[serde(rename_all = "camelCase", deny_unknown_fields)]
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
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentCommandArtifactValidation {
    pub status: AgentCommandArtifactValidationStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentCommandArtifactMetadata {
    pub size_bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    pub validation: AgentCommandArtifactValidation,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
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
#[serde(rename_all = "camelCase", deny_unknown_fields)]
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
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentCommandArtifactSnapshotCoverage {
    pub roots_scanned: u64,
    pub directory_entries_scanned: u64,
    pub office_files_seen: u64,
    pub files_hashed: u64,
    pub files_unhashed: u64,
    pub bytes_hashed: u64,
    pub symlinks_skipped: u64,
    pub excluded_directories: u64,
    pub duration_ms: u64,
    pub time_budget_exceeded: bool,
    pub cancelled: bool,
    pub truncated: bool,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentCommandArtifactObservationCoverage {
    pub workspace_included: bool,
    pub expected_output_count: u64,
    pub additional_root_count: u64,
    pub before: AgentCommandArtifactSnapshotCoverage,
    pub after: AgentCommandArtifactSnapshotCoverage,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentCommandArtifactObservationWarning {
    pub phase: AgentCommandArtifactObservationPhase,
    pub code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub message: String,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentCommandArtifactObservation {
    pub schema_version: u32,
    pub status: AgentCommandArtifactObservationStatus,
    /// Unified completeness flag for model, UI, audit, and checkpoint consumers.
    ///
    /// This remains redundant with `status` on purpose and is required by schema v3.
    pub partial: bool,
    /// Stable backend reason codes explaining why `partial` is true.
    pub stop_reasons: Vec<String>,
    /// Office files considered across the before and after snapshots.
    pub scanned: u64,
    /// Change records included in this observation.
    pub returned: u64,
    /// Known change records omitted by the bounded report projection.
    ///
    /// A partial snapshot can additionally have an unknown unobserved suffix; `partial` and
    /// `stopReasons` prevent this known count from being mistaken for complete coverage.
    pub omitted: u64,
    pub coverage: AgentCommandArtifactObservationCoverage,
    pub changes: Vec<AgentCommandArtifactChange>,
    pub changes_truncated: bool,
    pub changes_omitted: u64,
    pub expected_outputs: Vec<AgentCommandExpectedArtifactOutcome>,
    pub warnings: Vec<AgentCommandArtifactObservationWarning>,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum AgentCommandRuntimeKind {
    Node,
    Python,
}

/// Stable identifier for an application-owned artifact runtime profile.
///
/// A profile selects a reproducible capability family. It is deliberately not a package
/// request: package names, exact versions, provider identity, and runtime integrity evidence are
/// resolved by the trusted host and frozen in [`AgentCommandRuntimeBinding`]. `Pdf` is an
/// internal-only binding selected from the exact activated bundled Skill identity; it is
/// intentionally absent from the model-facing `run_command.runtimeProfile` schema.
#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase")]
pub enum AgentCommandRuntimeProfile {
    Documents,
    Spreadsheets,
    Presentations,
    Pdf,
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
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentCommandRequest {
    pub id: String,
    pub command: String,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub cwd: Option<String>,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub timeout_ms: Option<u64>,
    pub approval_status: AgentApprovalStatus,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub risk_level: Option<AgentCommandRiskLevel>,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub reason: Option<String>,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub observe: Option<AgentCommandArtifactObservationRequest>,
    pub inputs: Vec<AgentFileInputBinding>,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub runtime_binding: Option<Box<AgentCommandRuntimeBinding>>,
    /// Backend-only transaction identity for an exact bundled Office Skill script. The model
    /// cannot supply this field; `run_command` derives it from a run-scoped materialization
    /// receipt and freezes the destination before approval.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub managed_office_script: Option<Box<crate::office::OfficeManagedScriptBinding>>,
}

/// Frozen request to copy one immutable Skill resource, or one resource-tree
/// prefix, into the selected workspace.
///
/// The source is a logical `skill://` URI. Managed-store paths and resource
/// bytes never cross the runtime action protocol.
#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentSkillMaterializationRequest {
    pub id: String,
    pub source_uri: String,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub source_prefix: Option<String>,
    pub destination: String,
    pub approval_status: AgentApprovalStatus,
    #[serde(deserialize_with = "deserialize_required_nullable")]
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
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentSkillDependencyCheck {
    pub kind: AgentSkillDependencyKind,
    pub name: String,
    pub status: AgentSkillDependencyStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
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
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentSkillScriptRequest {
    pub id: String,
    pub script_uri: String,
    pub skill_id: String,
    pub skill_revision: String,
    pub resource_path: String,
    pub resource_digest: String,
    pub interpreter: AgentSkillScriptInterpreter,
    pub args: Vec<String>,
    pub requirements: AgentSkillScriptRequirements,
    pub preflight: AgentSkillScriptPreflightReport,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub timeout_ms: Option<u64>,
    pub approval_status: AgentApprovalStatus,
    #[serde(deserialize_with = "deserialize_required_nullable")]
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
    #[serde(flatten, default)]
    pub output_capture: crate::command::ProcessOutputCaptureMetadata,
    /// Backend-only complete stdout capture consumed by Exact History.
    #[serde(skip, default)]
    pub stdout_spool: crate::command::ProcessOutputSpool,
    /// Backend-only complete stderr capture consumed by Exact History.
    #[serde(skip, default)]
    pub stderr_spool: crate::command::ProcessOutputSpool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl AgentSkillScriptResult {
    pub fn output_spool_substitutions(
        &self,
    ) -> Vec<crate::command::ProcessOutputSpoolSubstitution> {
        crate::command::process_output_spool_substitutions(&self.stdout_spool, &self.stderr_spool)
    }
}

pub const AGENT_SKILL_INSTALLATION_SCHEMA_VERSION: u32 = 1;

/// Presentation-safe preview frozen by the Host before a Skill installation enters approval.
///
/// Every string originating in the third-party package remains untrusted display data. Authority
/// such as preparation IDs, destination paths and warning acknowledgements deliberately stays
/// behind `install_ref` in the Host application service.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentSkillInstallationPreview {
    pub name: String,
    pub description: String,
    pub source_summary: serde_json::Value,
    pub resolved_revision: String,
    pub file_count: u64,
    pub total_bytes: u64,
    pub resource_summary: AgentSkillInstallationResourceSummary,
    pub contains_scripts: bool,
    pub warnings: Vec<AgentSkillInstallationWarning>,
    pub compatibility: String,
    pub operation: String,
    pub impact: String,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentSkillInstallationResourceSummary {
    pub total: u64,
    pub references: u64,
    pub assets: u64,
    pub scripts: u64,
    pub bytes: u64,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentSkillInstallationWarning {
    pub code: String,
    pub message: String,
    pub requires_acknowledgement: bool,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentSkillInstallationRequest {
    pub schema_version: u32,
    pub id: String,
    pub install_ref: String,
    pub preview: AgentSkillInstallationPreview,
    pub approval_status: AgentApprovalStatus,
    pub expires_at: u64,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum AgentProposedAction {
    ToolCall {
        call: AgentToolCall,
    },
    McpToolCall {
        approval: Box<AgentMcpToolApproval>,
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
    SkillInstallation {
        installation: Box<AgentSkillInstallationRequest>,
    },
}

#[derive(Debug, Serialize, Clone)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum AgentEvent {
    /// Starts a Renderer-facing Agent event stream.
    ///
    /// `tool_definitions` contains only built-in and Runtime Extension definitions. External MCP
    /// definitions remain in the provider request contract and are exposed to Renderer only
    /// through the bounded MCP management catalog.
    Started {
        run_id: String,
        tool_definitions: Vec<AgentToolDefinition>,
    },
    /// Authoritative model-facing Tool contract for the next request boundary.
    ///
    /// The registry may contain additional implementations, but only these definitions are both
    /// visible to the model and executable for calls produced under this revision.
    ///
    /// Renderer event serialization further omits definitions with typed MCP identity. The
    /// revisions still describe the authoritative full model contract; Renderer must use the MCP
    /// management catalog for bounded external Tool metadata.
    ToolSetChanged {
        run_id: String,
        stable_revision: String,
        dynamic_revision: String,
        effective_revision: String,
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
        #[serde(deserialize_with = "deserialize_required_nullable")]
        trace_sequence: Option<u64>,
    },
    LlmRetry {
        run_id: String,
        stream_id: String,
        attempt: usize,
        max_attempts: usize,
        category: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        provider_code: Option<String>,
        delay_ms: u64,
        retry_at: u64,
    },
    ToolInputProgress {
        run_id: String,
        stream_id: String,
        attempt: usize,
        tool_call_index: usize,
        /// Provisional stream events do not expose provider-owned raw Tool Call IDs.
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
    GuidanceQueued {
        run_id: String,
        guidance_id: String,
        client_message_id: String,
        content: String,
        attachments: Vec<ConversationTraceAttachment>,
        created_at: i64,
    },
    GuidanceApplied {
        run_id: String,
        guidance_id: String,
        client_message_id: String,
        content: String,
        attachments: Vec<ConversationTraceAttachment>,
        created_at: i64,
        sequence: u64,
    },
    GuidanceRejected {
        run_id: String,
        guidance_id: String,
        client_message_id: String,
        content: String,
        rejection_code: AgentSteerRunRejectionCode,
        message: String,
        created_at: i64,
    },
    /// Generic built-in or Runtime Extension Tool call.
    ///
    /// MCP calls use `ApprovalRequired::McpToolCall` plus
    /// `McpToolInvocationStateChanged`; they are never duplicated here.
    ToolCall {
        run_id: String,
        trace_sequence: u64,
        call: AgentToolCall,
    },
    /// Generic built-in or Runtime Extension Tool result.
    ///
    /// MCP results remain in the internal trace/model continuation and are represented to
    /// Renderer only by the bounded typed invocation lifecycle.
    ToolResult {
        run_id: String,
        result: AgentToolResult,
    },
    McpToolInvocationStateChanged {
        run_id: String,
        invocation: AgentMcpToolInvocationEvent,
    },
    TodoUpdated {
        run_id: String,
        todo: AgentTodoState,
    },
    SkillActivated {
        run_id: String,
        activation_revision: String,
        activated_by: AgentSkillActivationActor,
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
        trace_sequence: u64,
    },
    ContextCompactionFinished {
        run_id: String,
        operation_id: String,
        outcome: AgentContextCompactionEventOutcome,
        trace_sequence: u64,
    },
    ApprovalRequired {
        run_id: String,
        action: Box<AgentProposedAction>,
        #[serde(skip)]
        checkpoint: Box<AgentRunCheckpoint>,
    },
    Diff {
        run_id: String,
        diff: AgentDiffProposal,
    },
    CommandStarted {
        run_id: String,
        conversation_id: String,
        assistant_message_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        project_id: Option<String>,
        call_id: String,
        session_id: String,
        started_at: u64,
    },
    CommandOutput {
        run_id: String,
        conversation_id: String,
        assistant_message_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        project_id: Option<String>,
        call_id: String,
        session_id: String,
        sequence: u64,
        stream: AgentCommandOutputStream,
        output: String,
    },
    CommandExited {
        run_id: String,
        conversation_id: String,
        assistant_message_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        project_id: Option<String>,
        call_id: String,
        session_id: String,
        status: AgentCommandExitStatus,
        #[serde(skip_serializing_if = "Option::is_none")]
        exit_code: Option<i32>,
        ended_at: u64,
        latest_sequence: u64,
        output_truncated: bool,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        outputs: Vec<crate::command::AgentCommandPublishedOutput>,
        #[serde(skip_serializing_if = "Option::is_none")]
        artifact_observation: Option<AgentCommandArtifactObservation>,
    },
    CommandInterrupted {
        run_id: String,
        conversation_id: String,
        assistant_message_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        project_id: Option<String>,
        call_id: String,
        session_id: String,
        ended_at: u64,
        latest_sequence: u64,
        output_truncated: bool,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        outputs: Vec<crate::command::AgentCommandPublishedOutput>,
        #[serde(skip_serializing_if = "Option::is_none")]
        artifact_observation: Option<AgentCommandArtifactObservation>,
    },
    Error {
        run_id: Option<String>,
        #[serde(deserialize_with = "deserialize_required_nullable")]
        trace_sequence: Option<u64>,
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

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
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
    // Set only at the runtime's final model-request boundary. This proves the failed work was
    // provisional model sampling, so the Host may safely keep the committed trace prefix.
    model_request_interruption: Option<AgentModelRequestInterruptionReason>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentModelRequestInterruptionReason {
    ServiceConnectionFailed,
    ServiceUnavailable,
    AuthenticationFailed,
    QuotaExhausted,
    ContextLimitExceeded,
    RequestRejected,
    ResponseInvalid,
    RequestFailed,
}

impl AgentModelRequestInterruptionReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ServiceConnectionFailed => "service_connection_failed",
            Self::ServiceUnavailable => "service_unavailable",
            Self::AuthenticationFailed => "authentication_failed",
            Self::QuotaExhausted => "quota_exhausted",
            Self::ContextLimitExceeded => "context_limit_exceeded",
            Self::RequestRejected => "request_rejected",
            Self::ResponseInvalid => "response_invalid",
            Self::RequestFailed => "request_failed",
        }
    }
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
            model_request_interruption: None,
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
            model_request_interruption: None,
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
            model_request_interruption: None,
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

    pub fn model_request_interruption(&self) -> Option<AgentModelRequestInterruptionReason> {
        self.model_request_interruption
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

    pub fn with_model_request_interruption(mut self) -> Self {
        self.model_request_interruption = Some(classify_model_request_interruption(&self));
        self
    }
}

fn classify_model_request_interruption(error: &AgentError) -> AgentModelRequestInterruptionReason {
    if error.code() == Some("agent.llm_provider_failure") {
        return match error
            .details()
            .and_then(|details| details.get("category"))
            .and_then(Value::as_str)
        {
            Some("network") => AgentModelRequestInterruptionReason::ServiceConnectionFailed,
            Some("rate_limited" | "overloaded") => {
                AgentModelRequestInterruptionReason::ServiceUnavailable
            }
            Some("authentication") => AgentModelRequestInterruptionReason::AuthenticationFailed,
            Some("quota_exhausted") => AgentModelRequestInterruptionReason::QuotaExhausted,
            Some("context_too_large") => AgentModelRequestInterruptionReason::ContextLimitExceeded,
            Some("invalid_request") => AgentModelRequestInterruptionReason::RequestRejected,
            _ => AgentModelRequestInterruptionReason::RequestFailed,
        };
    }

    if error.code().is_some_and(|code| {
        code.contains("response") || code.contains("stream") || code.contains("decode")
    }) {
        AgentModelRequestInterruptionReason::ResponseInvalid
    } else {
        AgentModelRequestInterruptionReason::RequestFailed
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
    use crate::image_generation::{
        ImageArtifactFormat, ImageGenerationExecutionFailureCode, ImageGenerationExecutionPhase,
    };
    use serde_json::{json, Value};

    #[test]
    fn model_request_interruption_marker_survives_error_enrichment() {
        let error = AgentError::structured(
            "agent.llm_provider_failure",
            "模型服务网络请求失败。",
            json!({
                "type": "llm_provider_failure",
                "category": "network",
            }),
        )
        .with_model_request_interruption()
        .with_usage(Some(AgentUsage {
            input_tokens: Some(7),
            output_tokens: None,
            output_thinking_tokens: None,
            total_tokens: Some(7),
            cached_input_tokens: None,
            cache_creation_input_tokens: None,
            billable_request_count: Some(1),
        }))
        .with_conversation_turn_trace(completed_conversation_trace_without_items(
            "run-1",
            "conversation-1",
            "assistant-1",
        ));

        assert_eq!(
            error.model_request_interruption(),
            Some(AgentModelRequestInterruptionReason::ServiceConnectionFailed)
        );
        assert_eq!(error.usage().and_then(|usage| usage.total_tokens), Some(7));
        assert!(error.conversation_turn_trace().is_some());
    }

    #[test]
    fn provider_continuation_ref_is_versioned_canonical_and_debug_redacted() {
        let continuation_ref = ProviderContinuationRef::new();
        continuation_ref.validate().unwrap();
        assert!(continuation_ref.id.starts_with("provider-continuation-v1:"));
        assert_eq!(
            format!("{continuation_ref:?}"),
            "ProviderContinuationRef([REDACTED])"
        );
        assert!(!format!("{continuation_ref:?}").contains(&continuation_ref.id));

        let encoded = serde_json::to_value(&continuation_ref).unwrap();
        assert_eq!(encoded["version"], PROVIDER_CONTINUATION_REF_VERSION);
        assert_eq!(encoded["id"], continuation_ref.id);
        assert_eq!(
            serde_json::from_value::<ProviderContinuationRef>(encoded).unwrap(),
            continuation_ref
        );
    }

    #[test]
    fn provider_continuation_ref_rejects_unknown_or_noncanonical_identity() {
        let valid = ProviderContinuationRef::new();
        assert!(ProviderContinuationRef::parse(valid.version + 1, valid.id.clone()).is_err());
        assert!(ProviderContinuationRef::parse(
            PROVIDER_CONTINUATION_REF_VERSION,
            "provider-continuation-v1:00000000-0000-0000-0000-000000000000"
        )
        .is_err());
        assert!(ProviderContinuationRef::parse(
            PROVIDER_CONTINUATION_REF_VERSION,
            valid.id.to_ascii_uppercase()
        )
        .is_err());
    }

    fn image_generation_audit() -> AgentImageGenerationAudit {
        AgentImageGenerationAudit {
            execution_id: format!("agent-v1:{}", "a".repeat(64)),
            request_fingerprint: format!("sha256:{}", "b".repeat(64)),
            provider_profile_id: "default".to_string(),
            adapter_id: "smartmlSeedream".to_string(),
            profile_revision: 7,
            model_id: "seedream-model".to_string(),
            provider_request_id: Some(format!("sha256:{}", "c".repeat(32))),
            http_status: Some(200),
            created_at: 10,
            completed_at: 20,
            duration_ms: 10,
        }
    }

    #[test]
    fn agent_events_match_the_cross_language_golden_contract() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../../packages/protocol/fixtures/agent-contract-v1.json"
        ))
        .unwrap();
        let events = &fixture["events"];

        let message_delta = AgentEvent::MessageDelta {
            run_id: "run-contract-v1".to_string(),
            stream_id: Some("stream-contract-v1".to_string()),
            delta: "hello".to_string(),
        };
        let message_stream_committed = AgentEvent::MessageStreamCommitted {
            run_id: "run-contract-v1".to_string(),
            stream_id: "stream-contract-v1".to_string(),
            trace_sequence: Some(3),
        };
        let tool_call = AgentEvent::ToolCall {
            run_id: "run-contract-v1".to_string(),
            trace_sequence: 4,
            call: AgentToolCall {
                id: "call-contract-v1".to_string(),
                tool: "read_file".to_string(),
                args: json!({ "path": "README.md" }),
                approval_status: AgentApprovalStatus::NotRequired,
                reason: Some("Inspect project documentation.".to_string()),
            },
        };
        let tool_result = AgentEvent::ToolResult {
            run_id: "run-contract-v1".to_string(),
            result: AgentToolResult {
                exact_archive_file: None,
                call_id: "call-contract-v1".to_string(),
                tool: "read_file".to_string(),
                ok: true,
                result: Some(json!({ "content": "MyCopilot Next" })),
                error: None,
            },
        };
        let command_started = AgentEvent::CommandStarted {
            run_id: "run-contract-v1".to_string(),
            conversation_id: "conversation-contract-v1".to_string(),
            assistant_message_id: "assistant-contract-v1".to_string(),
            project_id: Some("project-contract-v1".to_string()),
            call_id: "call-command-contract-v1".to_string(),
            session_id: "cmd_1234567890abcdef1234567890abcdef".to_string(),
            started_at: 10,
        };
        let command_output = AgentEvent::CommandOutput {
            run_id: "run-contract-v1".to_string(),
            conversation_id: "conversation-contract-v1".to_string(),
            assistant_message_id: "assistant-contract-v1".to_string(),
            project_id: Some("project-contract-v1".to_string()),
            call_id: "call-command-contract-v1".to_string(),
            session_id: "cmd_1234567890abcdef1234567890abcdef".to_string(),
            sequence: 1,
            stream: AgentCommandOutputStream::Stdout,
            output: "ready\n".to_string(),
        };
        let command_exited = AgentEvent::CommandExited {
            run_id: "run-contract-v1".to_string(),
            conversation_id: "conversation-contract-v1".to_string(),
            assistant_message_id: "assistant-contract-v1".to_string(),
            project_id: Some("project-contract-v1".to_string()),
            call_id: "call-command-contract-v1".to_string(),
            session_id: "cmd_1234567890abcdef1234567890abcdef".to_string(),
            status: AgentCommandExitStatus::Exited,
            exit_code: Some(0),
            ended_at: 20,
            latest_sequence: 1,
            output_truncated: false,
            outputs: Vec::new(),
            artifact_observation: None,
        };
        let command_interrupted = AgentEvent::CommandInterrupted {
            run_id: "run-contract-v1".to_string(),
            conversation_id: "conversation-contract-v1".to_string(),
            assistant_message_id: "assistant-contract-v1".to_string(),
            project_id: Some("project-contract-v1".to_string()),
            call_id: "call-command-contract-v1".to_string(),
            session_id: "cmd_1234567890abcdef1234567890abcdef".to_string(),
            ended_at: 21,
            latest_sequence: 1,
            output_truncated: false,
            outputs: Vec::new(),
            artifact_observation: None,
        };
        let done = AgentEvent::Done {
            run_id: "run-contract-v1".to_string(),
            success: true,
            status: Some(AgentRunStatus::Completed),
            content: Some("done".to_string()),
            usage: None,
            finish_reason: None,
            proposed_actions: Vec::new(),
        };

        assert_eq!(
            serde_json::to_value(message_delta).unwrap(),
            events["messageDelta"]
        );
        assert_eq!(
            serde_json::to_value(message_stream_committed).unwrap(),
            events["messageStreamCommitted"]
        );
        assert_eq!(serde_json::to_value(tool_call).unwrap(), events["toolCall"]);
        assert_eq!(
            serde_json::to_value(tool_result).unwrap(),
            events["toolResult"]
        );
        assert_eq!(
            serde_json::to_value(command_started).unwrap(),
            events["commandStarted"]
        );
        assert_eq!(
            serde_json::to_value(command_output).unwrap(),
            events["commandOutput"]
        );
        assert_eq!(
            serde_json::to_value(command_exited).unwrap(),
            events["commandExited"]
        );
        assert_eq!(
            serde_json::to_value(command_interrupted).unwrap(),
            events["commandInterrupted"]
        );
        assert_eq!(serde_json::to_value(done).unwrap(), events["done"]);
    }

    #[test]
    fn managed_command_session_projection_is_camel_case_and_process_safe() {
        let snapshot = AgentCommandSessionSnapshot {
            schema_version: 2,
            session_id: "cmd_1234567890abcdef1234567890abcdef".to_string(),
            conversation_id: "conversation-1".to_string(),
            assistant_message_id: "assistant-1".to_string(),
            origin_run_id: "run-1".to_string(),
            call_id: "call-1".to_string(),
            project_id: Some("project-1".to_string()),
            command: "python3 app.py".to_string(),
            cwd: "/workspace".to_string(),
            command_digest: format!("sha256:{}", "a".repeat(64)),
            status: AgentCommandSessionStatus::Running,
            started_at: 10,
            ended_at: None,
            exit_code: None,
            latest_sequence: 2,
            output_truncated: false,
            outputs: Vec::new(),
            artifact_observation: None,
            archive_ref: None,
        };
        let value = serde_json::to_value(&snapshot).unwrap();
        assert_eq!(value["schemaVersion"], 2);
        assert_eq!(value["sessionId"], snapshot.session_id);
        assert_eq!(value["originRunId"], "run-1");
        assert_eq!(value["status"], "running");
        for forbidden in ["pid", "environment", "approvalPayload", "processHandle"] {
            assert!(value.get(forbidden).is_none(), "leaked {forbidden}");
        }

        let started = serde_json::to_value(AgentEvent::CommandStarted {
            run_id: "run-1".to_string(),
            conversation_id: "conversation-1".to_string(),
            assistant_message_id: "assistant-1".to_string(),
            project_id: Some("project-1".to_string()),
            call_id: "call-1".to_string(),
            session_id: "cmd_1234567890abcdef1234567890abcdef".to_string(),
            started_at: 10,
        })
        .unwrap();
        assert_eq!(started["type"], "command_started");
        assert_eq!(started["conversationId"], "conversation-1");
        assert_eq!(started["assistantMessageId"], "assistant-1");
        assert_eq!(started["projectId"], "project-1");
        assert_eq!(started["sessionId"], snapshot.session_id);
    }

    #[test]
    fn image_generation_success_contract_exposes_only_managed_artifact_metadata() {
        let digest = "d".repeat(64);
        let result = AgentImageGenerationResult {
            schema_version: AGENT_IMAGE_GENERATION_RESULT_SCHEMA_VERSION,
            status: AgentImageGenerationResultStatus::Succeeded,
            operation: AgentImageGenerationOperation::Generate,
            artifact: Some(AgentImageGenerationArtifact {
                artifact_id: format!("sha256:{digest}"),
                uri: format!("image-artifact://sha256/{digest}"),
                kind: AgentImageGenerationArtifactKind::Image,
                format: ImageArtifactFormat::Png,
                mime_type: "image/png".to_string(),
                width: 1024,
                height: 1024,
                size_bytes: 42,
                sha256: digest.clone(),
            }),
            audit: image_generation_audit(),
            failure: None,
        };

        let value = serde_json::to_value(&result).unwrap();
        assert_eq!(value["schemaVersion"], 1);
        assert_eq!(value["status"], "succeeded");
        assert_eq!(value["operation"], "generate");
        assert_eq!(value["artifact"]["kind"], "image");
        assert_eq!(value["artifact"]["format"], "png");
        assert_eq!(
            value["artifact"]["uri"],
            format!("image-artifact://sha256/{digest}")
        );
        assert_eq!(value["artifact"].as_object().unwrap().len(), 9);
        assert!(value.get("failure").is_none());

        let serialized = serde_json::to_string(&result).unwrap();
        for forbidden in [
            "endpoint",
            "signedUrl",
            "storageRelativePath",
            "absolutePath",
            "apiKey",
            "inputBytes",
            "https://provider.example/signed-output",
            "PRIVATE_API_KEY",
        ] {
            assert!(!serialized.contains(forbidden), "leaked {forbidden}");
        }

        let round_trip: AgentImageGenerationResult = serde_json::from_value(value).unwrap();
        assert_eq!(round_trip, result);
    }

    #[test]
    fn image_generation_failure_contract_preserves_uncertainty_without_artifact() {
        let result = AgentImageGenerationResult {
            schema_version: AGENT_IMAGE_GENERATION_RESULT_SCHEMA_VERSION,
            status: AgentImageGenerationResultStatus::OutcomeIndeterminate,
            operation: AgentImageGenerationOperation::Edit,
            artifact: None,
            audit: image_generation_audit(),
            failure: Some(AgentImageGenerationFailure {
                code: ImageGenerationExecutionFailureCode::DeadlineExceeded,
                phase: ImageGenerationExecutionPhase::Provider,
                message: "the provider outcome is unknown".to_string(),
                recovery: "Check execution history before retrying.".to_string(),
                retryable: false,
                generation_may_have_succeeded: true,
                provider_succeeded: false,
                artifact_commit_may_have_succeeded: false,
            }),
        };

        let value = serde_json::to_value(&result).unwrap();
        assert_eq!(value["status"], "outcomeIndeterminate");
        assert_eq!(value["operation"], "edit");
        assert!(value.get("artifact").is_none());
        assert_eq!(value["failure"]["code"], "deadlineExceeded");
        assert_eq!(value["failure"]["phase"], "provider");
        assert_eq!(value["failure"]["generationMayHaveSucceeded"], true);
        assert_eq!(value["failure"]["providerSucceeded"], false);
        assert_eq!(value["failure"]["artifactCommitMayHaveSucceeded"], false);
    }

    #[test]
    fn image_generation_terminal_statuses_have_stable_wire_values() {
        let cases = [
            (AgentImageGenerationResultStatus::Succeeded, "succeeded"),
            (AgentImageGenerationResultStatus::Failed, "failed"),
            (AgentImageGenerationResultStatus::Cancelled, "cancelled"),
            (
                AgentImageGenerationResultStatus::OutcomeIndeterminate,
                "outcomeIndeterminate",
            ),
            (
                AgentImageGenerationResultStatus::CommitIndeterminate,
                "commitIndeterminate",
            ),
        ];
        for (status, expected) in cases {
            assert_eq!(serde_json::to_value(status).unwrap(), expected);
        }
    }

    #[test]
    fn skill_activated_event_uses_the_stable_frontend_contract() {
        let value = serde_json::to_value(AgentEvent::SkillActivated {
            run_id: "run-1".to_string(),
            activation_revision: "activation-sha256-v1:test".to_string(),
            activated_by: AgentSkillActivationActor::Model,
            skill: AgentSkillActivatedEvent {
                id: "bundled:application:documents".to_string(),
                name: "documents".to_string(),
                revision: "skill-package-sha256-v2:test".to_string(),
                source: AgentSkillSourceSummary {
                    kind: "bundled".to_string(),
                    id: "bundled:application".to_string(),
                },
            },
        })
        .unwrap();

        assert_eq!(value["type"], "skill_activated");
        assert_eq!(value["runId"], "run-1");
        assert_eq!(value["activationRevision"], "activation-sha256-v1:test");
        assert_eq!(value["activatedBy"], "model");
        assert_eq!(value["skill"]["name"], "documents");
        assert_eq!(value["skill"]["source"]["kind"], "bundled");
        assert_eq!(value["skill"]["source"]["id"], "bundled:application");
    }

    #[test]
    fn model_capabilities_are_required_and_use_current_camel_case_shape() {
        let missing = serde_json::from_value::<AgentChatInput>(json!({
            "apiUrl": "https://example.test/v1/chat/completions",
            "apiToken": "secret",
            "model": "text-only-model",
            "messages": []
        }));
        assert!(missing.is_err());

        let input = serde_json::from_value::<AgentChatInput>(json!({
            "apiUrl": "https://example.test/v1/chat/completions",
            "apiToken": "secret",
            "model": "text-only-model",
            "modelCapabilities": { "imageInput": true },
            "messages": []
        }))
        .unwrap();
        let serialized = serde_json::to_value(&input).unwrap();
        assert_eq!(serialized["modelCapabilities"]["imageInput"], true);
        assert!(serialized.get("model_capabilities").is_none());

        let round_trip = serde_json::from_value::<AgentChatInput>(serialized).unwrap();
        assert!(round_trip.model_capabilities.image_input);
    }

    #[test]
    fn agent_input_calls_and_results_never_expose_values_through_debug() {
        const CANARY: &str = "AGENT_RUNTIME_DEBUG_SECRET_CANARY";
        let input = serde_json::from_value::<AgentChatInput>(json!({
            "apiUrl": "https://example.test/v1/chat/completions",
            "apiToken": CANARY,
            "model": "test-model",
            "modelCapabilities": { "imageInput": false },
            "searchConfig": {
                "mode": "tavily",
                "tavilyApiKey": CANARY
            },
            "messages": []
        }))
        .unwrap();
        let call = AgentToolCall {
            id: "call-safe-id".to_string(),
            tool: "mcp__fixture__echo".to_string(),
            args: json!({"neutral": CANARY}),
            approval_status: AgentApprovalStatus::Required,
            reason: None,
        };
        let result = AgentToolResult {
            call_id: "call-safe-id".to_string(),
            tool: "mcp__fixture__echo".to_string(),
            ok: true,
            result: Some(json!({"neutral": CANARY})),
            error: None,
            exact_archive_file: None,
        };

        let rendered = format!("{input:?}{:?}{call:?}{result:?}", input.search_config);
        assert!(!rendered.contains(CANARY));
        assert!(!rendered.contains("neutral"));
    }

    #[test]
    fn current_permissions_require_explicit_command_safety_policy() {
        let missing = serde_json::from_value::<AgentPermissions>(json!({
            "read": "all",
            "write": "all",
            "command": "auto_approve",
            "patch": "auto_approve"
        }));
        assert!(missing.is_err());

        let permissions: AgentPermissions = serde_json::from_value(json!({
            "read": "all",
            "write": "all",
            "command": "auto_approve",
            "commandSafety": "guarded",
            "patch": "auto_approve"
        }))
        .unwrap();
        assert_eq!(
            permissions.command_safety,
            AgentCommandSafetyPolicy::Guarded
        );
    }

    #[test]
    fn command_artifact_observation_v3_is_total_and_rejects_unknown_fields() {
        let current = AgentCommandArtifactObservation {
            schema_version: AGENT_COMMAND_ARTIFACT_OBSERVATION_SCHEMA_VERSION,
            status: AgentCommandArtifactObservationStatus::Complete,
            partial: false,
            stop_reasons: Vec::new(),
            scanned: 1,
            returned: 0,
            omitted: 0,
            coverage: AgentCommandArtifactObservationCoverage {
                workspace_included: true,
                expected_output_count: 0,
                additional_root_count: 0,
                before: AgentCommandArtifactSnapshotCoverage::default(),
                after: AgentCommandArtifactSnapshotCoverage::default(),
            },
            changes: Vec::new(),
            changes_truncated: false,
            changes_omitted: 0,
            expected_outputs: Vec::new(),
            warnings: Vec::new(),
        };
        let canonical = serde_json::to_value(&current).unwrap();
        let decoded: AgentCommandArtifactObservation =
            serde_json::from_value(canonical.clone()).unwrap();
        assert_eq!(decoded, current);

        for field in [
            "partial",
            "stopReasons",
            "scanned",
            "returned",
            "omitted",
            "changesTruncated",
            "changesOmitted",
            "expectedOutputs",
            "warnings",
        ] {
            let mut missing = canonical.clone();
            missing.as_object_mut().unwrap().remove(field);
            assert!(
                serde_json::from_value::<AgentCommandArtifactObservation>(missing).is_err(),
                "current artifact observation field {field} is required"
            );
        }

        let mut extra = canonical;
        extra
            .as_object_mut()
            .unwrap()
            .insert("executionAuthority".to_string(), json!(true));
        assert!(serde_json::from_value::<AgentCommandArtifactObservation>(extra).is_err());
    }

    #[test]
    fn persisted_action_nested_settings_reject_unknown_fields() {
        let mut observation = json!({
            "kinds": ["office"],
            "expectedOutputs": [],
            "additionalRoots": []
        });
        observation["executionAuthority"] = true.into();
        assert!(
            serde_json::from_value::<AgentCommandArtifactObservationRequest>(observation).is_err()
        );

        let mut preflight = json!({
            "status": "ready",
            "interpreter": "python3",
            "interpreterVersion": "3.13",
            "dependencies": [{
                "kind": "command",
                "name": "python3",
                "status": "available",
                "version": "3.13"
            }],
            "runtimeFingerprint": "sha256:current"
        });
        preflight["dependencies"][0]["runtimeBinding"] = json!({});
        assert!(serde_json::from_value::<AgentSkillScriptPreflightReport>(preflight).is_err());

        let installation = json!({
            "schemaVersion": AGENT_SKILL_INSTALLATION_SCHEMA_VERSION,
            "id": "install-1",
            "installRef": "private-ref",
            "preview": {
                "name": "example",
                "description": "Example Skill",
                "sourceSummary": {},
                "resolvedRevision": "revision-1",
                "fileCount": 1,
                "totalBytes": 10,
                "resourceSummary": {
                    "total": 1,
                    "references": 0,
                    "assets": 0,
                    "scripts": 0,
                    "bytes": 10
                },
                "containsScripts": false,
                "warnings": [{
                    "code": "review",
                    "message": "Review source",
                    "requiresAcknowledgement": true
                }],
                "compatibility": "compatible",
                "operation": "install",
                "impact": "Adds one Skill"
            },
            "approvalStatus": "required",
            "expiresAt": 100
        });
        serde_json::from_value::<AgentSkillInstallationRequest>(installation.clone()).unwrap();
        for path in ["preview", "resourceSummary", "warning"] {
            let mut extra = installation.clone();
            match path {
                "preview" => extra["preview"]["runtimeBinding"] = json!({}),
                "resourceSummary" => {
                    extra["preview"]["resourceSummary"]["runtimeBinding"] = json!({})
                }
                "warning" => extra["preview"]["warnings"][0]["runtimeBinding"] = json!({}),
                _ => unreachable!(),
            }
            assert!(
                serde_json::from_value::<AgentSkillInstallationRequest>(extra).is_err(),
                "nested {path} must reject extra fields"
            );
        }
    }

    #[test]
    fn current_skill_activation_requires_total_skill_snapshots() {
        let activation = AgentSkillActivation {
            activation_revision: "activation-sha256-v1:current".to_string(),
            skills: vec![AgentActivatedSkill {
                id: "bundled:documents".to_string(),
                name: "documents".to_string(),
                revision: "skill-sha256-v1:current".to_string(),
                source: "bundled".to_string(),
                instructions: "Use the current instructions.".to_string(),
                source_bytes: 29,
                resources: None,
            }],
        };
        let canonical = serde_json::to_value(&activation).unwrap();
        serde_json::from_value::<AgentSkillActivation>(canonical.clone()).unwrap();

        let mut missing_skills = canonical.clone();
        missing_skills.as_object_mut().unwrap().remove("skills");
        assert!(serde_json::from_value::<AgentSkillActivation>(missing_skills).is_err());

        let mut missing_source_bytes = canonical.clone();
        missing_source_bytes["skills"][0]
            .as_object_mut()
            .unwrap()
            .remove("sourceBytes");
        assert!(serde_json::from_value::<AgentSkillActivation>(missing_source_bytes).is_err());

        let mut extra = canonical;
        extra["skills"][0]
            .as_object_mut()
            .unwrap()
            .insert("runtimeBinding".to_string(), json!({}));
        assert!(serde_json::from_value::<AgentSkillActivation>(extra).is_err());
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
            conversation_model_context_items: Vec::new(),
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
            trace_sequence: 7,
        })
        .unwrap();
        let finished = serde_json::to_value(AgentEvent::ContextCompactionFinished {
            run_id: "run-1".to_string(),
            operation_id: "compaction-1".to_string(),
            outcome: AgentContextCompactionEventOutcome::Applied,
            trace_sequence: 7,
        })
        .unwrap();

        assert_eq!(started["type"], "context_compaction_started");
        assert_eq!(started["operationId"], "compaction-1");
        assert_eq!(started["traceSequence"], 7);
        assert_eq!(finished["type"], "context_compaction_finished");
        assert_eq!(finished["operationId"], started["operationId"]);
        assert_eq!(finished["outcome"], "applied");
        assert_eq!(finished["traceSequence"], started["traceSequence"]);
    }

    #[test]
    fn sequenced_presentation_events_serialize_required_trace_fields() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../../packages/protocol/fixtures/agent-contract-v1.json"
        ))
        .unwrap();
        assert_eq!(fixture["events"]["toolCall"]["traceSequence"], 4);
        assert_eq!(
            fixture["events"]["messageStreamCommitted"]["traceSequence"],
            3
        );

        let stream_without_narration = serde_json::to_value(AgentEvent::MessageStreamCommitted {
            run_id: "run-1".to_string(),
            stream_id: "stream-1".to_string(),
            trace_sequence: None,
        })
        .unwrap();
        let unanchored_error = serde_json::to_value(AgentEvent::Error {
            run_id: Some("run-1".to_string()),
            trace_sequence: None,
            message: "failed".to_string(),
            recoverable: false,
            code: None,
            details: None,
        })
        .unwrap();
        assert!(stream_without_narration
            .as_object()
            .unwrap()
            .contains_key("traceSequence"));
        assert!(stream_without_narration["traceSequence"].is_null());
        assert!(unanchored_error
            .as_object()
            .unwrap()
            .contains_key("traceSequence"));
        assert!(unanchored_error["traceSequence"].is_null());
    }

    #[test]
    fn llm_retry_event_serializes_structured_safe_retry_metadata() {
        let retry = serde_json::to_value(AgentEvent::LlmRetry {
            run_id: "run-1".to_string(),
            stream_id: "stream-1".to_string(),
            attempt: 2,
            max_attempts: 6,
            category: "rate_limited".to_string(),
            provider_code: Some("rate_limit_exceeded".to_string()),
            delay_ms: 5_000,
            retry_at: 1_800_000_005_000,
        })
        .unwrap();

        assert_eq!(retry["type"], "llm_retry");
        assert_eq!(retry["category"], "rate_limited");
        assert_eq!(retry["providerCode"], "rate_limit_exceeded");
        assert_eq!(retry["delayMs"], 5_000);
        assert_eq!(retry["retryAt"], 1_800_000_005_000_u64);
        assert_eq!(retry["attempt"], 2);
        assert_eq!(retry["maxAttempts"], 6);
        assert!(retry.get("reason").is_none());
    }

    #[test]
    fn agent_permission_meet_is_component_wise_and_never_widens() {
        let full = AgentPermissions {
            read: AgentReadPermission::All,
            write: AgentWritePermission::All,
            command: AgentCommandPermission::AutoApprove,
            command_safety: AgentCommandSafetyPolicy::FullAccess,
            patch: AgentPatchPermission::AutoApprove,
        };
        let custom = AgentPermissions {
            read: AgentReadPermission::All,
            write: AgentWritePermission::WorkspaceOnly,
            command: AgentCommandPermission::RequireApproval,
            command_safety: AgentCommandSafetyPolicy::Guarded,
            patch: AgentPatchPermission::AutoApprove,
        };
        let minimum = AgentPermissions::default();

        assert_eq!(full.meet(custom), custom);
        assert_eq!(custom.meet(full), custom);
        assert_eq!(custom.meet(minimum), minimum);
        assert_eq!(minimum.meet(custom), minimum);
        assert_eq!(full.meet(full), full);
        assert_eq!(minimum.meet(minimum), minimum);
    }
}
