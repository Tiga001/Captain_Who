pub const CONVERSATION_TURN_TRACE_SCHEMA_VERSION: u32 = 6;

/// Replay-safe material first introduced by the Host at a causal Run position.
/// This identifies historical content, never current capability or execution authority.
#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConversationContextMaterialKind {
    InputAttachment,
    SkillInstructions,
    RunWorldState,
}

/// Immutable image identity. Binary bytes and local paths are intentionally excluded.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationContextImageRef {
    pub attachment_id: String,
    pub mime_type: String,
    pub sha256: String,
}

pub const MAX_CONTEXT_MATERIAL_CONTENT_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_CONTEXT_MATERIAL_IMAGE_REFS: usize = 256;

impl ConversationContextImageRef {
    pub fn validate(&self) -> Result<(), String> {
        if self.attachment_id.trim().is_empty()
            || self.attachment_id.len() > 1_024
            || self.attachment_id.chars().any(char::is_control)
            || !self.mime_type.starts_with("image/")
            || self.mime_type.len() <= "image/".len()
            || self.mime_type.len() > 128
            || self
                .mime_type
                .chars()
                .any(|ch| ch.is_whitespace() || ch.is_control())
            || self.sha256.strip_prefix("sha256:").is_none_or(|digest| {
                digest.len() != 64
                    || !digest
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            })
        {
            return Err("context material image reference is invalid".to_string());
        }
        ensure_no_binary_text("context material attachment identity", &self.attachment_id)?;
        Ok(())
    }
}

fn validate_context_images(images: &[ConversationContextImageRef]) -> Result<(), String> {
    if images.len() > MAX_CONTEXT_MATERIAL_IMAGE_REFS {
        return Err("context material contains too many image references".to_string());
    }
    let mut identities = BTreeSet::new();
    for image in images {
        image.validate()?;
        if !identities.insert(&image.attachment_id) {
            return Err("context material image identity is duplicated".to_string());
        }
    }
    Ok(())
}

fn validate_context_material(
    event_id: &str,
    material_kind: ConversationContextMaterialKind,
    content: &str,
    images: &[ConversationContextImageRef],
    created_at: i64,
) -> Result<(), String> {
    if event_id.trim().is_empty()
        || event_id.len() > 512
        || event_id.chars().any(char::is_control)
        || created_at < 0
        || content.len() > MAX_CONTEXT_MATERIAL_CONTENT_BYTES
        || (content.trim().is_empty() && images.is_empty())
        || (material_kind != ConversationContextMaterialKind::InputAttachment && !images.is_empty())
    {
        return Err("context material identity or payload is invalid".to_string());
    }
    ensure_no_binary_text("context material event identity", event_id)?;
    ensure_no_binary_text("context material content", content)?;
    validate_context_images(images)
}

/// Temporal placement of a Host-authored state observation relative to the final message.
#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConversationBackendStatePlacement {
    Timeline,
    AfterMessage,
}

pub const MAX_BACKEND_STATE_CONTENT_BYTES: usize = 16 * 1024;

fn validate_backend_state(event_id: &str, content: &str, created_at: i64) -> Result<(), String> {
    if event_id.trim().is_empty()
        || event_id.len() > 512
        || created_at < 0
        || content.len() > MAX_BACKEND_STATE_CONTENT_BYTES
    {
        return Err("Backend state identity or payload size is invalid".to_string());
    }
    ensure_no_binary_text("Backend state event identity", event_id)?;
    ensure_no_binary_text("Backend state content", content)?;
    let value: Value = serde_json::from_str(content)
        .map_err(|_| "Backend state content must be a JSON object".to_string())?;
    if !value.is_object() {
        return Err("Backend state content must be a JSON object".to_string());
    }
    ensure_no_binary_value("Backend state payload", &value)?;
    Ok(())
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConversationTurnTraceTerminalStatus {
    InProgress,
    Completed,
    Failed,
    Cancelled,
}

impl ConversationTurnTraceTerminalStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InProgress => "in_progress",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn is_terminal(self) -> bool {
        !matches!(self, Self::InProgress)
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConversationTraceToolResultStatus {
    Succeeded,
    Failed,
    Rejected,
    Conflict,
    Cancelled,
}

/// Durable audit phase for a Host-owned command Session.
///
/// This is deliberately not a Tool result: `started` records the durable handoff boundary and
/// `terminal` records later Host settlement without pretending that either event was returned to
/// the model.
#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConversationCommandSessionLifecyclePhase {
    Started,
    Terminal,
}

/// Durable phase of a Runtime-owned context compaction presentation row.
///
/// Both phases are append-only audit facts. The Renderer displays one row at the `Started`
/// sequence and uses the later `Finished` item only to settle that row after restart.
#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConversationContextCompactionLifecyclePhase {
    Started,
    Finished,
}

/// Sequence-free lifecycle payload supplied by the managed command owner when it appends audit.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationCommandSessionLifecycle {
    pub phase: ConversationCommandSessionLifecyclePhase,
    pub session_id: String,
    pub call_id: String,
    pub status: AgentCommandSessionStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    pub latest_sequence: u64,
    pub output_truncated: bool,
    #[serde(flatten)]
    pub archive: ConversationHistoryArchiveTraceMetadata,
    pub created_at: i64,
}

/// One provider-neutral message from the durable, replay-safe conversation timeline.
///
/// This is deliberately separate from [`ConversationTurnTraceItem`]. The durable trace is a
/// bounded audit/search projection, while this record preserves the bounded projection that can
/// safely survive a process restart until context compaction covers it. Process-only MCP
/// arguments, raw Tool output, and binary delivery data never enter this record; they belong to
/// transient provider messages or the Exact History Archive as appropriate.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationModelContextItem {
    pub sequence: u64,
    pub ordinal: u32,
    pub role: String,
    pub content: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<ConversationContextImageRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<AgentContextCheckpointToolCall>,
    pub is_error: bool,
}

impl ConversationModelContextItem {
    pub fn validate(&self) -> Result<(), String> {
        ensure_no_binary_text("model context content", &self.content)?;
        validate_context_images(&self.images)?;
        if self.role != "user" && !self.images.is_empty() {
            return Err("model context image references require a user message".to_string());
        }
        match self.role.as_str() {
            "user" => {
                if self.tool_call_id.is_some() || !self.tool_calls.is_empty() || self.is_error {
                    return Err(
                        "model context user message contains tool protocol fields".to_string()
                    );
                }
            }
            "assistant" => {
                if self.tool_call_id.is_some() || self.is_error {
                    return Err(
                        "model context assistant message contains tool-result fields".to_string(),
                    );
                }
                let mut provider_indices = BTreeSet::new();
                let mut runtime_call_ids = BTreeSet::new();
                let mut previous_provider_index = None;
                for call in &self.tool_calls {
                    if call.id.trim().is_empty() || !is_provider_safe_tool_name(&call.name) {
                        return Err("model context tool call identity is invalid".to_string());
                    }
                    validate_provider_tool_call_id(&call.provider_identity.provider_call_id)
                        .map_err(|_| {
                            "model context provider tool call identity is invalid".to_string()
                        })?;
                    if call.provider_identity.runtime_call_id != call.id
                        || !runtime_call_ids.insert(call.provider_identity.runtime_call_id.as_str())
                        || !provider_indices.insert(call.provider_identity.provider_tool_index)
                        || previous_provider_index.is_some_and(|previous| {
                            previous >= call.provider_identity.provider_tool_index
                        })
                    {
                        return Err(
                            "model context Provider/Runtime tool call mapping is inconsistent"
                                .to_string(),
                        );
                    }
                    previous_provider_index = Some(call.provider_identity.provider_tool_index);
                    ensure_no_binary_value("model context tool call", &call.args)?;
                }
            }
            "tool" => {
                if self
                    .tool_call_id
                    .as_deref()
                    .is_none_or(|call_id| call_id.trim().is_empty())
                    || !self.tool_calls.is_empty()
                {
                    return Err("model context tool result identity is invalid".to_string());
                }
            }
            _ => return Err("model context message role is invalid".to_string()),
        }
        Ok(())
    }
}

pub(crate) fn model_context_item_from_message(
    sequence: u64,
    ordinal: u32,
    message: &LlmMessage,
) -> Result<(ConversationModelContextItem, bool), String> {
    model_context_item_from_message_with_identities(sequence, ordinal, message, None)
}

fn model_context_item_from_message_with_identities(
    sequence: u64,
    ordinal: u32,
    message: &LlmMessage,
    explicit_provider_identities: Option<&BTreeMap<String, AgentProviderToolCallIdentity>>,
) -> Result<(ConversationModelContextItem, bool), String> {
    let (content, content_redacted) = sanitize_runtime_text(message.content());
    let effective_tool_calls = message.tool_calls().cloned().collect::<Vec<_>>();
    let provider_identities = if effective_tool_calls.is_empty() {
        BTreeMap::new()
    } else if let Some(provider_identities) = explicit_provider_identities {
        provider_identities.clone()
    } else {
        let assistant_turn = message.assistant_turn().ok_or_else(|| {
            "assistant model context tool calls are missing their source turn".to_string()
        })?;
        assistant_turn
            .checkpoint_identity()
            .map_err(|_| "assistant model context tool identity is invalid".to_string())?
            .tool_call_identities
            .into_iter()
            .map(|identity| (identity.runtime_call_id.clone(), identity))
            .collect::<BTreeMap<_, _>>()
    };
    let mut tool_calls = Vec::with_capacity(effective_tool_calls.len());
    let mut tool_call_redacted = false;
    for call in effective_tool_calls {
        let (args, redacted) = sanitize_runtime_value(&call.args);
        tool_call_redacted |= redacted;
        let provider_identity = provider_identities.get(&call.id).cloned().ok_or_else(|| {
            "assistant model context tool call is missing its Provider/Runtime identity".to_string()
        })?;
        tool_calls.push(AgentContextCheckpointToolCall {
            id: call.id,
            name: call.name,
            args,
            provider_identity,
        });
    }
    let item = ConversationModelContextItem {
        sequence,
        ordinal,
        role: message.role().as_str().to_string(),
        content,
        images: Vec::new(),
        tool_call_id: message.tool_call_id().map(str::to_string),
        tool_calls,
        is_error: message.is_error(),
    };
    item.validate()?;
    Ok((
        item,
        content_redacted || tool_call_redacted || !message.images().is_empty(),
    ))
}

#[derive(Debug, Deserialize, Serialize, Clone, Default, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationModelContextLog {
    pub assistant_message_id: String,
    pub items: Vec<ConversationModelContextItem>,
}

/// Validates the shared identity and boundary contract between the lossy audit trace and the
/// uncompressed model projection.
///
/// One trace sequence may produce more than one provider-neutral message, hence `ordinal`.
/// Nevertheless, persisted model items must cover every trace sequence through a safe boundary;
/// otherwise falling back from the exact prefix to the trace suffix could duplicate or split a
/// tool exchange.
pub(crate) fn validate_model_context_prefix(
    trace: &ConversationTurnTrace,
    items: &[ConversationModelContextItem],
) -> Result<(), String> {
    let mut previous_identity = None;
    let mut covered_sequences = std::collections::BTreeSet::new();
    for item in items {
        item.validate()?;
        let identity = (item.sequence, item.ordinal);
        if previous_identity.is_some_and(|previous| identity <= previous) {
            return Err("model context item identity must be strictly increasing".to_string());
        }
        previous_identity = Some(identity);
        let trace_item = trace
            .items
            .iter()
            .find(|trace_item| trace_item.sequence() == item.sequence)
            .ok_or_else(|| "model context item references a missing trace sequence".to_string())?;
        validate_model_item_against_trace(item, trace_item)?;
        covered_sequences.insert(item.sequence);
    }
    let Some(covered_through) = items.last().map(|item| item.sequence) else {
        return Ok(());
    };
    let expected_sequences = trace
        .items
        .iter()
        .take_while(|item| item.sequence() <= covered_through)
        .filter(|item| item.is_model_visible())
        .map(ConversationTurnTraceItem::sequence)
        .collect::<std::collections::BTreeSet<_>>();
    if covered_sequences != expected_sequences {
        return Err(
            "model context items must cover a complete contiguous trace prefix".to_string(),
        );
    }
    let boundary = trace
        .items
        .iter()
        .find(|item| item.sequence() == covered_through)
        .ok_or_else(|| "model context prefix boundary is missing".to_string())?;
    let is_staged_open_call = trace.terminal_status
        == ConversationTurnTraceTerminalStatus::InProgress
        && matches!(boundary, ConversationTurnTraceItem::ToolCall { .. })
        && trace.items.last().map(ConversationTurnTraceItem::sequence) == Some(covered_through);
    if !boundary.is_safe_compaction_boundary() && !is_staged_open_call {
        return Err("model context prefix cannot end with an unresolved tool call".to_string());
    }
    Ok(())
}

fn validate_model_item_against_trace(
    item: &ConversationModelContextItem,
    trace_item: &ConversationTurnTraceItem,
) -> Result<(), String> {
    if !item.images.is_empty()
        && !matches!(
            trace_item,
            ConversationTurnTraceItem::ContextMaterial { .. }
        )
    {
        return Err("model context images have no matching context material".to_string());
    }
    let valid = match trace_item {
        ConversationTurnTraceItem::AssistantNarration { .. } => {
            item.role == "assistant"
                && item.tool_call_id.is_none()
                && item.tool_calls.is_empty()
                && !item.content.trim().is_empty()
        }
        ConversationTurnTraceItem::UserGuidance { .. }
        | ConversationTurnTraceItem::AgentMailboxDelivery { .. } => {
            item.role == "user"
                && item.tool_call_id.is_none()
                && item.tool_calls.is_empty()
                && !item.content.trim().is_empty()
        }
        ConversationTurnTraceItem::ContextMaterial {
            content, images, ..
        } => {
            item.ordinal == 0
                && item.role == "user"
                && item.tool_call_id.is_none()
                && item.tool_calls.is_empty()
                && !item.is_error
                && item.content == *content
                && item.images == *images
        }
        ConversationTurnTraceItem::BackendState { content, .. } => {
            item.ordinal == 0
                && item.role == "user"
                && item.tool_call_id.is_none()
                && item.tool_calls.is_empty()
                && !item.is_error
                && item.content == *content
        }
        ConversationTurnTraceItem::ToolCall { call_id, tool, .. } => {
            item.role == "assistant"
                && item.tool_call_id.is_none()
                && item.tool_calls.len() == 1
                && item.tool_calls[0].id == *call_id
                && item.tool_calls[0].name == *tool
        }
        ConversationTurnTraceItem::ToolResult { call_id, .. } => {
            item.role == "tool"
                && item.tool_call_id.as_deref() == Some(call_id.as_str())
                && item.tool_calls.is_empty()
        }
        ConversationTurnTraceItem::CommandSessionLifecycle { .. }
        | ConversationTurnTraceItem::ContextCompactionLifecycle { .. }
        | ConversationTurnTraceItem::RuntimeError { .. } => false,
    };
    valid.then_some(()).ok_or_else(|| {
        "model context item identity does not match its durable trace item".to_string()
    })
}

impl ConversationTraceToolResultStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Rejected => "rejected",
            Self::Conflict => "conflict",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ConversationTurnTraceItem {
    /// Host-generated, exact safe model material, retained at its original causal position.
    ContextMaterial {
        sequence: u64,
        event_id: String,
        material_kind: ConversationContextMaterialKind,
        content: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        images: Vec<ConversationContextImageRef>,
        created_at: i64,
    },
    /// Host-generated observation, never a human answer or a permission grant.
    BackendState {
        sequence: u64,
        event_id: String,
        content: String,
        created_at: i64,
        placement: ConversationBackendStatePlacement,
    },
    AssistantNarration {
        sequence: u64,
        content: String,
        /// Links the UI narration to the same complete assistant Provider turn.
        /// Hydration consumes this projection by identity, never by matching text.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_turn_id: Option<String>,
        /// Host canonical first call identity links Generic wire narration to its Tool batch.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        first_tool_call_id: Option<String>,
        truncated: bool,
    },
    UserGuidance {
        sequence: u64,
        guidance_id: String,
        client_message_id: String,
        content: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        attachments: Vec<ConversationTraceAttachment>,
        created_at: i64,
        truncated: bool,
    },
    /// Host-authenticated collaboration input admitted at the single pre-sampling boundary.
    /// The Mailbox row remains transport truth; this item proves which Turn/model timeline
    /// consumed it.
    AgentMailboxDelivery {
        sequence: u64,
        receipt_id: String,
        message_id: String,
        sender_agent_id: String,
        sender_task_name: String,
        sender_task_path: String,
        kind: crate::AgentMailboxKind,
        content: String,
        created_at: i64,
        truncated: bool,
    },
    ToolCall {
        sequence: u64,
        call_id: String,
        tool: String,
        provenance: AgentToolIdentity,
        operation: Value,
        approval_status: AgentApprovalStatus,
        truncated: bool,
    },
    ToolResult {
        sequence: u64,
        call_id: String,
        tool: String,
        status: ConversationTraceToolResultStatus,
        success: bool,
        observation: Value,
        approval_status: AgentApprovalStatus,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
        truncated: bool,
        #[serde(flatten)]
        archive: ConversationHistoryArchiveTraceMetadata,
    },
    /// Host-observed lifecycle metadata for a managed command Session.
    ///
    /// Command output is intentionally absent. Bounded runtime output belongs to the Session
    /// store and terminal output belongs to Exact History; this item is only an append-only audit
    /// link between those projections and the originating Tool call.
    CommandSessionLifecycle {
        sequence: u64,
        phase: ConversationCommandSessionLifecyclePhase,
        session_id: String,
        call_id: String,
        status: AgentCommandSessionStatus,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        exit_code: Option<i32>,
        latest_sequence: u64,
        output_truncated: bool,
        #[serde(flatten)]
        archive: ConversationHistoryArchiveTraceMetadata,
        created_at: i64,
    },
    /// Runtime-owned context compaction lifecycle. It is intentionally invisible to model
    /// context but remains in the same durable sequence domain as narration and Tool activity.
    ContextCompactionLifecycle {
        sequence: u64,
        phase: ConversationContextCompactionLifecyclePhase,
        operation_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        outcome: Option<AgentContextCompactionEventOutcome>,
    },
    /// Bounded Runtime error presentation. Host/transport errors without an active Runtime trace
    /// remain unanchored and are never synthesized into this audit record.
    RuntimeError {
        sequence: u64,
        message: String,
        recoverable: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        code: Option<String>,
        truncated: bool,
    },
}

/// Exact-history metadata is flattened into a tool-result trace item so the canonical trace can
/// jump directly from the bounded durable record to the lossless archive. The archive itself
/// never enters normal model context.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationHistoryArchiveTraceMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archive_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived_bytes: Option<u64>,
    /// Whether every byte of the archive projection received by the backend was persisted.
    ///
    /// This deliberately says nothing about bytes an upstream Tool or Provider omitted before
    /// producing that projection; [`Self::truncated_at_source`] records that independent fact.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived_completely: Option<bool>,
    /// Whether the Tool or Provider irrecoverably omitted content before backend archival.
    #[serde(default, skip_serializing_if = "is_false")]
    pub truncated_at_source: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub model_projection_truncated: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub history_projection_truncated: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub archive_projection_truncated: bool,
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationTraceAttachment {
    pub id: String,
    pub kind: AgentInputAttachmentKind,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    pub size_bytes: u64,
}

impl ConversationTurnTraceItem {
    pub fn sequence(&self) -> u64 {
        match self {
            Self::AssistantNarration { sequence, .. }
            | Self::ContextMaterial { sequence, .. }
            | Self::BackendState { sequence, .. }
            | Self::UserGuidance { sequence, .. }
            | Self::AgentMailboxDelivery { sequence, .. }
            | Self::ToolCall { sequence, .. }
            | Self::ToolResult { sequence, .. }
            | Self::CommandSessionLifecycle { sequence, .. }
            | Self::ContextCompactionLifecycle { sequence, .. }
            | Self::RuntimeError { sequence, .. } => *sequence,
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Self::AssistantNarration { .. } => "assistant_narration",
            Self::BackendState { .. } => "backend_state",
            Self::ContextMaterial { .. } => "context_material",
            Self::UserGuidance { .. } => "user_guidance",
            Self::AgentMailboxDelivery { .. } => "agent_mailbox_delivery",
            Self::ToolCall { .. } => "tool_call",
            Self::ToolResult { .. } => "tool_result",
            Self::CommandSessionLifecycle { .. } => "command_session_lifecycle",
            Self::ContextCompactionLifecycle { .. } => "context_compaction_lifecycle",
            Self::RuntimeError { .. } => "runtime_error",
        }
    }

    /// Whether this audit item has a provider-neutral model-context representation.
    #[must_use]
    pub fn is_model_visible(&self) -> bool {
        !matches!(
            self,
            Self::CommandSessionLifecycle { .. }
                | Self::ContextCompactionLifecycle { .. }
                | Self::RuntimeError { .. }
        )
    }

    pub fn is_safe_compaction_boundary(&self) -> bool {
        !matches!(self, Self::ToolCall { .. })
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationTurnTrace {
    pub schema_version: u32,
    pub run_id: String,
    pub conversation_id: String,
    pub assistant_message_id: String,
    pub terminal_status: ConversationTurnTraceTerminalStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminal_error: Option<String>,
    /// True when bounded durable projection omitted, summarized, or redacted run-scoped material.
    pub truncated: bool,
    pub items: Vec<ConversationTurnTraceItem>,
}

#[derive(Debug, Clone, Default)]
pub struct ConversationTraceSnapshot {
    pub items: Vec<ConversationTurnTraceItem>,
    /// Exact, length-bounded text messages supplied to the model for the same trace prefix.
    ///
    /// The Host persists these in a separate model-context log. They are not serialized into the
    /// durable trace and therefore cannot enlarge audit rows or history-search projections.
    pub model_context_items: Vec<ConversationModelContextItem>,
    pub next_sequence: u64,
    pub truncated: bool,
}
