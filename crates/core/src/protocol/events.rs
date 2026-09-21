use super::*;

pub const BROWSER_RISK_APPROVAL_SCHEMA_VERSION: u32 = 1;
pub const BROWSER_RISK_APPROVAL_TTL_SECONDS: u64 = 15 * 60;

/// Host-classified browser risks. These values are display hints and frozen grant identity;
/// Renderer text and a Server/page claim never decide whether a destination is safe.
#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "snake_case")]
pub enum BrowserRiskKind {
    InsecureHttp,
    Localhost,
    Loopback,
    PrivateNetwork,
    LinkLocal,
    CloudMetadata,
    NonStandardPort,
    UrlUserinfo,
    DnsPrivateResolution,
    RiskEscalation,
    NewWindow,
    FileUpload,
    FileDownload,
    LocalServiceRequest,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum BrowserResolvedAddressClass {
    Public,
    Loopback,
    Private,
    LinkLocal,
    CloudMetadata,
    Unresolved,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum BrowserRiskTrigger {
    ToolArgument,
    MainFrame,
    Redirect,
    NewWindow,
    Subresource,
    Upload,
    Download,
}

/// Credential-free, normalized identity frozen before a risky browser boundary is crossed.
///
/// `normalized_url` never contains URL userinfo, query, or fragment. Resolver identity is kept
/// behind the Host boundary and is never serialized into this Renderer-facing projection.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BrowserDestinationIdentity {
    pub normalized_url: String,
    pub origin: String,
    pub scheme: String,
    pub ascii_host: String,
    pub effective_port: u16,
    pub address_class: BrowserResolvedAddressClass,
}

/// One exact, task-scoped browser risk decision. No header, Cookie, request body, upload path,
/// CDP endpoint, webContents identity, or resolved address is part of this public projection.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentBrowserRiskApproval {
    pub schema_version: u32,
    pub action_id: String,
    pub risk_approval_id: String,
    pub run_id: String,
    pub call_id: String,
    pub trigger_tool_name: String,
    pub capability_id: String,
    pub capability_activation_id: String,
    pub display_name: String,
    pub reason: String,
    pub destination: BrowserDestinationIdentity,
    pub trigger: BrowserRiskTrigger,
    pub risk_kinds: Vec<BrowserRiskKind>,
    pub manifest_digest: String,
    pub policy_revision: u64,
    pub created_at: u64,
    pub expires_at: u64,
    pub approval_status: AgentApprovalStatus,
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
    BuiltinCapabilityActivation {
        approval: Box<AgentBuiltinCapabilityActivationApproval>,
    },
    BuiltinMcpToolApproval {
        approval: Box<AgentBuiltinMcpToolApproval>,
    },
    BrowserRiskApproval {
        approval: Box<AgentBrowserRiskApproval>,
    },
    FileChange {
        file_change: AgentFileChangeProposal,
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
    FileChangePreviewUpdated {
        run_id: String,
        preview: AgentFileChangePreview,
    },
    FileChangePreviewCleared {
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
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        folder_references: Vec<crate::AgentFolderReference>,
        created_at: i64,
    },
    GuidanceApplied {
        run_id: String,
        guidance_id: String,
        client_message_id: String,
        content: String,
        attachments: Vec<ConversationTraceAttachment>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        folder_references: Vec<crate::AgentFolderReference>,
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
        /// Backend-authoritative identity of the exact Tool contract used for this call.
        ///
        /// Renderer must route specialized activity from this typed value and must never infer
        /// provenance from a model-visible Tool name.
        identity: AgentToolIdentity,
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
    FileChangeUpdated {
        run_id: String,
        file_change: AgentFileChangeSnapshot,
    },
    ContextWindowUpdated {
        run_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        conversation_id: Option<String>,
        /// Stable local model configuration identity. `snapshot.model` remains the Provider wire ID.
        model_config_id: String,
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
        /// Usage for the Runtime segment which produced this approval boundary.
        ///
        /// This stays inside Core/Host so the Host can account for the segment before the pending
        /// action becomes externally actionable. Renderer continues to receive the existing
        /// approval payload without any private accounting field.
        #[serde(skip)]
        segment_usage: Option<AgentUsage>,
    },
    FileChangeProposed {
        run_id: String,
        file_change: AgentFileChangeProposal,
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
