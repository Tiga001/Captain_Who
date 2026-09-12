use super::*;

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentExtensionSnapshot {
    pub extension_id: String,
    pub version: u32,
    pub state: Value,
}

/// Current durable Agent run checkpoint schema.
///
/// Version 17 uses semantic Agent names throughout model collaboration inputs and outputs.
/// Internal Agent IDs remain Host-only; older model contexts and checkpoints are not compatible.
/// Version 16 retains causal context material and immutable image identities across Run boundaries.
/// Version 15 preserves the exact Conversation World State ledger and request-adoption markers
/// beside its model projection, including direct Core runs without a storage Host.
/// Version 14 distinguishes approval and human-input suspension authority. Human answers arrive
/// exclusively through a native Host resume port and never carry an ApprovalDecision.
/// Version 13 carries the consumed predecessor observation for the exact pending FileChange so a
/// successful approval continuation can renew the same run-scoped id without weakening replay
/// protection. Version 12 carried the exact private reference for a Host-owned Run-scoped
/// FileChange grant.
/// Version 11 totalized unconsumed `read_file` observations referenced by queued `apply_patch`
/// calls. Version 10 froze built-in execution approval authority alongside every other permission
/// dimension. Version 9 froze model-visible Agent collaboration selector capabilities and removed the
/// retired Provider-specific Skill-activation sibling deferral bit. Tools exposed in the request
/// that produced a batch remain executable under that frozen ToolSet; newly activated Skill
/// instructions and Tool capabilities become visible only on the next model request.
/// The referenced payload remains encrypted in the Host vault; raw Provider continuation and
/// reasoning are never serialized into the checkpoint. Any other schema version is rejected at
/// the approval boundary.
pub const AGENT_RUN_CHECKPOINT_SCHEMA_VERSION: u32 = 18;

/// Suspension sources are separate authority domains. A user answer never grants approval.
#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentRunCheckpointPauseReason {
    Approval,
    UserInput,
}

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

#[derive(Deserialize, Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentRunCheckpoint {
    pub version: u32,
    pub pause_reason: AgentRunCheckpointPauseReason,
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
    /// Exact canonical conversation ledger; sanitized context text cannot restore authority or
    /// recover request-boundary epoch identity after approval or human-input suspension.
    pub conversation_world_state_records: Vec<crate::AnchoredWorldStateRecord>,
    /// Approval-record identity when it differs from the Provider Tool Call identity. External MCP
    /// and built-in capability activation both use an application UUID here. Tool kind and
    /// projection authority always come from the frozen typed Tool provenance, never this field.
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub pending_action_id: Option<String>,
    /// Exact private authority reference used only when a FileChange was routed by an already
    /// active Run grant. The granting action itself remains an explicit-user action and stores
    /// `None` here.
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub file_change_run_grant_ref: Option<crate::file_change::FileChangeRunGrantRef>,
    /// Consumed read/apply observation bound to the exact pending update/delete FileChange.
    /// Create carries `null`; queued unconsumed observations remain attached to their calls.
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub pending_file_observation: Option<crate::file_change::FileObservationCheckpoint>,
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

/// Checkpoint diagnostics must never disclose model history, Skill instructions, images,
/// queued Tool arguments, extension snapshots or private authority references.
impl std::fmt::Debug for AgentRunCheckpoint {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AgentRunCheckpoint")
            .field("version", &self.version)
            .field("pause_reason", &self.pause_reason)
            .field("run_id", &self.run_id)
            .field("next_model_request_index", &self.next_model_request_index)
            .field("context_item_count", &self.context_items.len())
            .field("queued_tool_call_count", &self.queued_tool_calls.len())
            .field("extension_snapshot_count", &self.extension_snapshots.len())
            .field(
                "provider_continuation_ref_count",
                &self.provider_continuation_refs.len(),
            )
            .field("pending_tool_call_id", &self.pending_tool_call_id)
            .field(
                "conversation_trace_item_count",
                &self.conversation_trace_items.len(),
            )
            .field(
                "conversation_model_context_item_count",
                &self.conversation_model_context_items.len(),
            )
            .field(
                "next_conversation_trace_sequence",
                &self.next_conversation_trace_sequence,
            )
            .field(
                "conversation_trace_truncated",
                &self.conversation_trace_truncated,
            )
            .finish_non_exhaustive()
    }
}

#[derive(Deserialize, Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentContextCheckpointItem {
    pub role: String,
    pub content: String,
    pub images: Vec<AgentContextCheckpointImage>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub context_image_refs: Vec<ConversationContextImageRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    pub tool_calls: Vec<AgentContextCheckpointToolCall>,
    pub is_error: bool,
    pub sources: Vec<String>,
    pub scope: String,
    pub retention: String,
    /// Provider-layout sequence only; the checkpoint item vector stays in journal order.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_order: Option<u64>,
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
            .field("context_image_ref_count", &self.context_image_refs.len())
            .field("tool_call_id", &self.tool_call_id)
            .field("tool_call_count", &self.tool_calls.len())
            .field("is_error", &self.is_error)
            .field("sources", &self.sources)
            .field("scope", &self.scope)
            .field("retention", &self.retention)
            .field("request_order", &self.request_order)
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
    /// Exact unconsumed Host observation referenced by a queued `apply_patch` call.
    ///
    /// This key is required for every queued call. It is `null` for every other tool, preventing a
    /// missing field from being interpreted as an older checkpoint shape.
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub file_observation: Option<crate::file_change::FileObservationCheckpoint>,
    pub assistant_content: String,
    pub group_id: String,
    /// Reference into `AgentRunCheckpoint.assistant_turn_identity.tool_call_identities`.
    pub assistant_turn_id: String,
    pub provider_tool_index: u32,
}
