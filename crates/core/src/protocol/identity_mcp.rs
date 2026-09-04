use super::*;

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentFileChangeOperation {
    Create,
    Update,
    Delete,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentFileChangeUpdateStrategy {
    Modify,
    Rewrite,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentFileChangeResultStatus {
    Applied,
    AlreadyApplied,
    Failed,
    Conflict,
    Rejected,
    OutcomeUnknown,
    Aborted,
    Expired,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentFileChangeOutcome {
    DefinitelyNotExecuted,
    Applied,
    OutcomeUnknown,
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
    /// Tool reviewed and packaged as part of one Host-owned built-in capability manifest.
    BuiltinCapability {
        capability_id: Box<str>,
        managed_mcp_id: Box<str>,
        package_name: Box<str>,
        package_version: Box<str>,
        upstream_catalog_digest: Box<str>,
        policy_digest: Box<str>,
        manifest_digest: Box<str>,
        tool_id: Box<str>,
        raw_name: Box<str>,
        model_name: Box<str>,
        upstream_schema_digest: Box<str>,
        host_overlay_digest: Box<str>,
        host_input_schema_digest: Box<str>,
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
pub enum AgentFileChangeStatus {
    Drafting,
    Ready,
    WaitingApproval,
    Applying,
    Applied,
    AlreadyApplied,
    Rejected,
    Conflict,
    Failed,
    OutcomeUnknown,
    Aborted,
    Expired,
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
