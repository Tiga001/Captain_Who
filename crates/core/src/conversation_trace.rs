//! Append-only, provider-neutral records of agent activity.
//!
//! The active tool loop, approval checkpoint, durable audit trace, uncompressed bounded model log,
//! Exact History Archive, and presentation timeline are deliberately separate views of one
//! execution. The recorder produces both the lossless-within-runtime-budget model projection and
//! the lossy audit projection from one sequence domain, so persistence and reconstruction can
//! prove that they describe the same activity without conflating their retention policies.

use crate::conversation_trace_projection::{
    is_repeat_failure_eligible, project_attachment_text, project_narration, project_terminal_error,
    project_tool_call, project_tool_result, project_user_guidance, sanitize_runtime_text,
    sanitize_runtime_value, DurableTraceProjectionLimits,
};
use crate::llm::{validate_provider_tool_call_id, LlmMessage};
use crate::protocol::{
    AgentApprovalStatus, AgentCommandSessionStatus, AgentContextCheckpointToolCall,
    AgentContextCompactionEventOutcome, AgentInputAttachment, AgentInputAttachmentKind,
    AgentMcpServerScope, AgentProposedAction, AgentProviderToolCallIdentity, AgentRunCheckpoint,
    AgentToolCall, AgentToolIdentity, AgentToolResult,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

pub const CONVERSATION_TURN_TRACE_SCHEMA_VERSION: u32 = 4;

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<AgentContextCheckpointToolCall>,
    pub is_error: bool,
}

impl ConversationModelContextItem {
    pub fn validate(&self) -> Result<(), String> {
        ensure_no_binary_text("model context content", &self.content)?;
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
    AssistantNarration {
        sequence: u64,
        content: String,
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

impl ConversationTurnTrace {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != CONVERSATION_TURN_TRACE_SCHEMA_VERSION {
            return Err(format!(
                "unsupported conversation trace schema version: {}",
                self.schema_version
            ));
        }
        if self.run_id.trim().is_empty()
            || self.conversation_id.trim().is_empty()
            || self.assistant_message_id.trim().is_empty()
        {
            return Err("conversation trace identifiers cannot be empty".to_string());
        }
        if self.terminal_status == ConversationTurnTraceTerminalStatus::InProgress
            && self.terminal_error.is_some()
        {
            return Err("in-progress conversation trace cannot have a terminal error".to_string());
        }

        let mut previous_sequence = None;
        let mut pending_call: Option<(&str, &str, bool)> = None;
        let mut call_ids = BTreeSet::new();
        let mut command_call_ids = BTreeSet::new();
        let mut command_sessions = BTreeMap::<&str, (&str, bool)>::new();
        let mut context_compactions = BTreeMap::<&str, bool>::new();
        for item in &self.items {
            let sequence = item.sequence();
            if previous_sequence.is_some_and(|previous| sequence <= previous) {
                return Err("conversation trace sequence must be strictly increasing".to_string());
            }
            previous_sequence = Some(sequence);

            match item {
                ConversationTurnTraceItem::AssistantNarration { content, .. } => {
                    if pending_call.is_some() {
                        return Err(
                            "conversation trace narration cannot split a tool exchange".to_string()
                        );
                    }
                    if content.trim().is_empty() {
                        return Err("conversation trace narration cannot be empty".to_string());
                    }
                    ensure_no_binary_text("assistant narration", content)?;
                }
                ConversationTurnTraceItem::UserGuidance {
                    guidance_id,
                    client_message_id,
                    content,
                    attachments,
                    created_at,
                    ..
                } => {
                    if pending_call.is_some() {
                        return Err(
                            "conversation trace user guidance cannot split a tool exchange"
                                .to_string(),
                        );
                    }
                    if guidance_id.trim().is_empty()
                        || client_message_id.trim().is_empty()
                        || content.trim().is_empty()
                        || *created_at < 0
                    {
                        return Err(
                            "conversation trace user guidance identity is invalid".to_string()
                        );
                    }
                    ensure_no_binary_text("user guidance", content)?;
                    let mut attachment_ids = BTreeSet::new();
                    for attachment in attachments {
                        if attachment.id.trim().is_empty()
                            || attachment.name.trim().is_empty()
                            || !attachment_ids.insert(attachment.id.as_str())
                        {
                            return Err("conversation trace user guidance attachment is invalid"
                                .to_string());
                        }
                        ensure_no_binary_text("user guidance attachment name", &attachment.name)?;
                        if let Some(mime_type) = &attachment.mime_type {
                            ensure_no_binary_text("user guidance attachment MIME type", mime_type)?;
                        }
                    }
                }
                ConversationTurnTraceItem::AgentMailboxDelivery {
                    receipt_id,
                    message_id,
                    sender_agent_id,
                    sender_task_name,
                    sender_task_path,
                    content,
                    created_at,
                    ..
                } => {
                    if pending_call.is_some() {
                        return Err(
                            "conversation trace Agent mailbox delivery cannot split a tool exchange"
                                .to_string(),
                        );
                    }
                    if receipt_id.trim().is_empty()
                        || message_id.trim().is_empty()
                        || sender_agent_id.trim().is_empty()
                        || sender_task_name.trim().is_empty()
                        || sender_task_path.trim().is_empty()
                        || content.trim().is_empty()
                        || *created_at < 0
                    {
                        return Err(
                            "conversation trace Agent mailbox delivery identity is invalid"
                                .to_string(),
                        );
                    }
                    ensure_no_binary_text("Agent mailbox delivery", content)?;
                }
                ConversationTurnTraceItem::ToolCall {
                    call_id,
                    tool,
                    provenance,
                    operation,
                    ..
                } => {
                    if pending_call.is_some() {
                        return Err(
                            "conversation trace contains an unresolved tool call".to_string()
                        );
                    }
                    if call_id.trim().is_empty() || !is_provider_safe_tool_name(tool) {
                        return Err("conversation trace tool call identity is invalid".to_string());
                    }
                    validate_tool_identity(tool, provenance)?;
                    if !call_ids.insert(call_id.as_str()) {
                        return Err(format!(
                            "conversation trace contains duplicate tool call id: {call_id}"
                        ));
                    }
                    if tool == "run_command" {
                        command_call_ids.insert(call_id.as_str());
                    }
                    ensure_no_binary_value("tool operation", operation)?;
                    pending_call = Some((
                        call_id,
                        tool,
                        matches!(provenance, AgentToolIdentity::Unregistered { .. }),
                    ));
                }
                ConversationTurnTraceItem::ToolResult {
                    call_id,
                    tool,
                    status,
                    success,
                    observation,
                    error,
                    archive,
                    ..
                } => {
                    let Some((expected_id, expected_tool, was_unregistered)) = pending_call.take()
                    else {
                        return Err(
                            "conversation trace tool result has no matching call".to_string()
                        );
                    };
                    if expected_id != call_id || expected_tool != tool {
                        return Err(
                            "conversation trace tool result does not match its call".to_string()
                        );
                    }
                    if *success != (*status == ConversationTraceToolResultStatus::Succeeded) {
                        return Err(
                            "conversation trace tool result success/status is inconsistent"
                                .to_string(),
                        );
                    }
                    if was_unregistered && *success {
                        return Err(
                            "unregistered conversation trace tool call cannot succeed".to_string()
                        );
                    }
                    ensure_no_binary_value("tool observation", observation)?;
                    if let Some(error) = error {
                        ensure_no_binary_text("tool error", error)?;
                    }
                    archive.validate()?;
                }
                ConversationTurnTraceItem::CommandSessionLifecycle {
                    phase,
                    session_id,
                    call_id,
                    status,
                    exit_code,
                    archive,
                    created_at,
                    ..
                } => {
                    if pending_call
                        .is_some_and(|(pending_call_id, _, _)| pending_call_id != call_id)
                    {
                        return Err(
                            "conversation trace command session lifecycle does not match the pending tool call"
                                .to_string(),
                        );
                    }
                    let session_suffix = session_id.strip_prefix("cmd_");
                    if session_suffix.is_none_or(|suffix| {
                        suffix.len() != 32 || !suffix.bytes().all(|byte| byte.is_ascii_hexdigit())
                    }) || call_id.trim().is_empty()
                        || call_id.len() > 1_024
                        || call_id.chars().any(char::is_control)
                        || *created_at < 0
                    {
                        return Err(
                            "conversation trace command session lifecycle identity is invalid"
                                .to_string(),
                        );
                    }
                    if !command_call_ids.contains(call_id.as_str()) {
                        return Err(
                            "conversation trace command session lifecycle references a missing run_command call"
                                .to_string(),
                        );
                    }
                    match phase {
                        ConversationCommandSessionLifecyclePhase::Started => {
                            if !matches!(
                                *status,
                                AgentCommandSessionStatus::Starting
                                    | AgentCommandSessionStatus::Running
                            ) || exit_code.is_some()
                                || *archive != ConversationHistoryArchiveTraceMetadata::default()
                            {
                                return Err(
                                    "conversation trace command session started item is invalid"
                                        .to_string(),
                                );
                            }
                            if command_sessions
                                .insert(session_id.as_str(), (call_id.as_str(), false))
                                .is_some()
                            {
                                return Err(
                                    "conversation trace contains duplicate command session start"
                                        .to_string(),
                                );
                            }
                        }
                        ConversationCommandSessionLifecyclePhase::Terminal => {
                            if !status.is_terminal()
                                || (*status != AgentCommandSessionStatus::Exited
                                    && exit_code.is_some())
                            {
                                return Err(
                                    "conversation trace command session terminal item is invalid"
                                        .to_string(),
                                );
                            }
                            if let Some((started_call_id, terminal_seen)) =
                                command_sessions.get_mut(session_id.as_str())
                            {
                                if *started_call_id != call_id || *terminal_seen {
                                    return Err(
                                        "conversation trace command session terminal identity is inconsistent"
                                            .to_string(),
                                    );
                                }
                                *terminal_seen = true;
                            } else {
                                // Startup reconciliation can append a terminal record to a trace
                                // whose pre-handoff start was not durably observed.
                                command_sessions
                                    .insert(session_id.as_str(), (call_id.as_str(), true));
                            }
                        }
                    }
                    archive.validate()?;
                }
                ConversationTurnTraceItem::ContextCompactionLifecycle {
                    phase,
                    operation_id,
                    outcome,
                    ..
                } => {
                    if pending_call.is_some() {
                        return Err(
                            "conversation trace context compaction cannot split a tool exchange"
                                .to_string(),
                        );
                    }
                    if operation_id.trim().is_empty()
                        || operation_id.len() > 2_048
                        || operation_id.chars().any(char::is_control)
                    {
                        return Err(
                            "conversation trace context compaction identity is invalid".to_string()
                        );
                    }
                    match phase {
                        ConversationContextCompactionLifecyclePhase::Started => {
                            if outcome.is_some()
                                || context_compactions
                                    .insert(operation_id.as_str(), false)
                                    .is_some()
                            {
                                return Err(
                                    "conversation trace context compaction start is invalid"
                                        .to_string(),
                                );
                            }
                        }
                        ConversationContextCompactionLifecyclePhase::Finished => {
                            let Some(finished) = context_compactions.get_mut(operation_id.as_str())
                            else {
                                return Err(
                                    "conversation trace context compaction finish is missing its start"
                                        .to_string(),
                                );
                            };
                            if *finished || outcome.is_none() {
                                return Err(
                                    "conversation trace context compaction finish is invalid"
                                        .to_string(),
                                );
                            }
                            *finished = true;
                        }
                    }
                }
                ConversationTurnTraceItem::RuntimeError { message, code, .. } => {
                    if pending_call.is_some() {
                        return Err(
                            "conversation trace Runtime error cannot split a tool exchange"
                                .to_string(),
                        );
                    }
                    if message.trim().is_empty() {
                        return Err("conversation trace Runtime error cannot be empty".to_string());
                    }
                    ensure_no_binary_text("Runtime error", message)?;
                    if let Some(code) = code {
                        if code.trim().is_empty() || code.len() > 128 {
                            return Err(
                                "conversation trace Runtime error code is invalid".to_string()
                            );
                        }
                        ensure_no_binary_text("Runtime error code", code)?;
                    }
                }
            }
        }
        if pending_call.is_some()
            && self.terminal_status != ConversationTurnTraceTerminalStatus::InProgress
        {
            return Err("conversation trace ends with an unresolved tool call".to_string());
        }
        if self.terminal_status.is_terminal()
            && context_compactions.values().any(|finished| !finished)
        {
            return Err(
                "terminal conversation trace contains an unfinished context compaction".to_string(),
            );
        }
        Ok(())
    }

    /// Number of items that form complete exchanges and may be rendered into model context.
    #[must_use]
    pub fn model_context_item_count(&self) -> usize {
        let unresolved_call_index = (self.terminal_status
            == ConversationTurnTraceTerminalStatus::InProgress)
            .then(|| {
                self.items
                    .iter()
                    .enumerate()
                    .rev()
                    .find_map(|(index, item)| {
                        let ConversationTurnTraceItem::ToolCall { call_id, .. } = item else {
                            return None;
                        };
                        let closed = self.items[index + 1..].iter().any(|candidate| {
                            matches!(
                                candidate,
                                ConversationTurnTraceItem::ToolResult {
                                    call_id: result_id,
                                    ..
                                } if result_id == call_id
                            )
                        });
                        (!closed).then_some(index)
                    })
            })
            .flatten();
        self.items[..unresolved_call_index.unwrap_or(self.items.len())]
            .iter()
            .filter(|item| item.is_model_visible())
            .count()
    }

    /// Validates the complete current model-context projection for this trace.
    ///
    /// A prefix is only valid for an actively unresolved final ToolCall. Every closed
    /// model-visible item must otherwise have been atomically persisted with the trace; missing
    /// suffixes are corruption, not a signal to reconstruct history from the lossy audit view.
    pub fn validate_complete_model_context(
        &self,
        items: &[ConversationModelContextItem],
    ) -> Result<(), String> {
        validate_model_context_prefix(self, items)?;
        let actual_sequences = items
            .iter()
            .map(|item| item.sequence)
            .collect::<BTreeSet<_>>();
        let unresolved_call_sequence = (self.terminal_status
            == ConversationTurnTraceTerminalStatus::InProgress)
            .then(|| {
                self.items.iter().rev().find_map(|item| {
                    let ConversationTurnTraceItem::ToolCall { sequence, call_id, .. } = item else {
                        return None;
                    };
                    let closed = self.items.iter().any(|candidate| {
                        matches!(candidate, ConversationTurnTraceItem::ToolResult { call_id: result_id, .. } if result_id == call_id)
                    });
                    (!closed).then_some(*sequence)
                })
            })
            .flatten();
        let expected_sequences = self
            .items
            .iter()
            .filter(|item| item.is_model_visible())
            .filter(|item| unresolved_call_sequence != Some(item.sequence()))
            .map(ConversationTurnTraceItem::sequence)
            .collect::<BTreeSet<_>>();
        if actual_sequences != expected_sequences {
            return Err(
                "conversation model context must cover every closed trace item".to_string(),
            );
        }
        Ok(())
    }

    /// Returns the closed portion of a persisted model-context log.
    ///
    /// An in-progress trace may durably stage the exact Assistant ToolCall identity needed for
    /// crash recovery. That final open exchange is never model-visible until an authoritative or
    /// synthetic ToolResult closes it.
    pub(crate) fn committed_model_context_prefix<'a>(
        &self,
        items: &'a [ConversationModelContextItem],
    ) -> Result<&'a [ConversationModelContextItem], String> {
        validate_model_context_prefix(self, items)?;
        let unresolved_sequence = (self.terminal_status
            == ConversationTurnTraceTerminalStatus::InProgress)
            .then(|| {
                self.items.iter().rev().find_map(|item| {
                    let ConversationTurnTraceItem::ToolCall { sequence, call_id, .. } = item else {
                        return None;
                    };
                    let closed = self.items.iter().any(|candidate| {
                        matches!(candidate, ConversationTurnTraceItem::ToolResult { call_id: result_id, .. } if result_id == call_id)
                    });
                    (!closed).then_some(*sequence)
                })
            })
            .flatten();
        let committed_len = unresolved_sequence
            .and_then(|sequence| items.iter().position(|item| item.sequence == sequence))
            .unwrap_or(items.len());
        Ok(&items[..committed_len])
    }
}

fn validate_tool_identity(trace_tool: &str, identity: &AgentToolIdentity) -> Result<(), String> {
    match identity {
        AgentToolIdentity::Builtin { tool_name } => {
            if tool_name != trace_tool || tool_name.chars().any(char::is_control) {
                return Err("conversation trace builtin provenance is inconsistent".to_string());
            }
        }
        AgentToolIdentity::RuntimeExtension {
            extension_id,
            tool_name,
        } => {
            if extension_id.trim().is_empty()
                || extension_id.trim() != extension_id
                || extension_id.chars().any(char::is_control)
                || tool_name != trace_tool
            {
                return Err(
                    "conversation trace runtime-extension provenance is invalid".to_string()
                );
            }
        }
        AgentToolIdentity::Mcp { provenance } => {
            let config_epoch_is_valid =
                uuid::Uuid::parse_str(&provenance.config_epoch).is_ok_and(|epoch| {
                    !epoch.is_nil()
                        && epoch.get_version() == Some(uuid::Version::Random)
                        && provenance.config_epoch == epoch.to_string()
                });
            let scoped_id_is_valid = match &provenance.scope {
                AgentMcpServerScope::Project { project_id } => {
                    !project_id.trim().is_empty()
                        && project_id.trim() == project_id
                        && project_id.len() <= 1_024
                        && !project_id.chars().any(char::is_control)
                }
                AgentMcpServerScope::Plugin { plugin_id } => {
                    !plugin_id.trim().is_empty()
                        && plugin_id.trim() == plugin_id
                        && plugin_id.len() <= 1_024
                        && !plugin_id.chars().any(char::is_control)
                }
                AgentMcpServerScope::Builtin
                | AgentMcpServerScope::User
                | AgentMcpServerScope::Managed => true,
            };
            if provenance.model_tool_name != trace_tool
                || uuid::Uuid::parse_str(&provenance.server_id).is_err()
                || !config_epoch_is_valid
                || provenance.registry_revision == 0
                || provenance.raw_tool_name.trim().is_empty()
                || provenance.raw_tool_name.trim() != provenance.raw_tool_name
                || provenance.raw_tool_name.len() > 1_024
                || provenance.raw_tool_name.chars().any(char::is_control)
                || provenance.catalog_generation == 0
                || provenance.config_digest.len() != 64
                || !provenance
                    .config_digest
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                || provenance.catalog_digest.len() != 64
                || !provenance
                    .catalog_digest
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                || provenance.catalog_schema_digest.len() != 64
                || !provenance
                    .catalog_schema_digest
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                || provenance.schema_digest.len() != 64
                || !provenance
                    .schema_digest
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                || provenance.schema_normalizer_version
                    != crate::MCP_INPUT_SCHEMA_NORMALIZER_VERSION
                || !scoped_id_is_valid
            {
                return Err("conversation trace MCP provenance is invalid".to_string());
            }
        }
        AgentToolIdentity::Unregistered { tool_name } => {
            if tool_name != trace_tool || !is_provider_safe_tool_name(tool_name) {
                return Err(
                    "conversation trace unregistered provenance is inconsistent".to_string()
                );
            }
        }
    }
    Ok(())
}

impl ConversationHistoryArchiveTraceMetadata {
    fn validate(&self) -> Result<(), String> {
        let has_archive_identity = self.archive_ref.is_some()
            || self.content_hash.is_some()
            || self.archived_bytes.is_some()
            || self.archived_completely.is_some();
        if !has_archive_identity {
            return Ok(());
        }
        let archive_ref = self
            .archive_ref
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| "conversation trace history archive ref is invalid".to_string())?;
        let content_hash = self
            .content_hash
            .as_deref()
            .filter(|value| {
                value.len() == 71
                    && value.starts_with("sha256:")
                    && value[7..]
                        .bytes()
                        .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            })
            .ok_or_else(|| "conversation trace history archive hash is invalid".to_string())?;
        if archive_ref.len() > 256
            || content_hash.is_empty()
            || self.archived_bytes.is_none()
            || self.archived_completely != Some(true)
        {
            return Err("conversation trace history archive metadata is incomplete".to_string());
        }
        Ok(())
    }
}

impl ConversationTurnTrace {
    /// Appends one idempotent command-Session audit event using the trace sequence domain.
    ///
    /// The candidate is validated before `self` is changed, so a rejected transition cannot
    /// leave a partially invalid trace in memory or storage.
    pub fn append_command_session_lifecycle(
        &mut self,
        lifecycle: ConversationCommandSessionLifecycle,
    ) -> Result<u64, String> {
        if let Some(existing) = self.items.iter().find(|item| {
            matches!(
                item,
                ConversationTurnTraceItem::CommandSessionLifecycle {
                    phase,
                    session_id,
                    ..
                } if *phase == lifecycle.phase && session_id == &lifecycle.session_id
            )
        }) {
            let expected = ConversationTurnTraceItem::CommandSessionLifecycle {
                sequence: existing.sequence(),
                phase: lifecycle.phase,
                session_id: lifecycle.session_id,
                call_id: lifecycle.call_id,
                status: lifecycle.status,
                exit_code: lifecycle.exit_code,
                latest_sequence: lifecycle.latest_sequence,
                output_truncated: lifecycle.output_truncated,
                archive: lifecycle.archive,
                created_at: lifecycle.created_at,
            };
            return (existing == &expected)
                .then_some(existing.sequence())
                .ok_or_else(|| {
                    "conversation trace command session lifecycle conflicts with existing audit"
                        .to_string()
                });
        }

        let sequence = self
            .items
            .last()
            .map(ConversationTurnTraceItem::sequence)
            .map_or(0, |current| current.saturating_add(1));
        let item = ConversationTurnTraceItem::CommandSessionLifecycle {
            sequence,
            phase: lifecycle.phase,
            session_id: lifecycle.session_id,
            call_id: lifecycle.call_id,
            status: lifecycle.status,
            exit_code: lifecycle.exit_code,
            latest_sequence: lifecycle.latest_sequence,
            output_truncated: lifecycle.output_truncated,
            archive: lifecycle.archive,
            created_at: lifecycle.created_at,
        };
        let mut candidate = self.clone();
        candidate.items.push(item.clone());
        candidate.validate()?;
        self.items.push(item);
        Ok(sequence)
    }
}

impl ConversationTraceSnapshot {
    /// Returns the closed prefix that is safe to append to model context.
    ///
    /// The durable audit trace may retain one final unresolved call while the process owns the
    /// run. Keeping that call out of this prefix prevents a half tool exchange from entering
    /// model context, while still allowing startup recovery to correlate a process-owned external
    /// side effect with its eventual terminal receipt.
    pub fn committed_prefix(&self) -> Self {
        let unresolved_call_index = self.items.iter().enumerate().rev().find_map(|(index, item)| {
            let ConversationTurnTraceItem::ToolCall { call_id, .. } = item else {
                return None;
            };
            let closed = self.items[index + 1..].iter().any(|candidate| {
                matches!(candidate, ConversationTurnTraceItem::ToolResult { call_id: result_id, .. } if result_id == call_id)
            });
            (!closed).then_some(index)
        });
        let items = unresolved_call_index
            .map_or_else(|| self.items.clone(), |index| self.items[..index].to_vec());
        let covered_sequence = items.last().map(ConversationTurnTraceItem::sequence);
        let model_context_items = self
            .model_context_items
            .iter()
            .filter(|item| covered_sequence.is_some_and(|sequence| item.sequence <= sequence))
            .cloned()
            .collect();
        Self {
            items,
            model_context_items,
            next_sequence: self.next_sequence,
            truncated: self.truncated,
        }
    }

    pub fn in_progress_trace(
        &self,
        run_id: &str,
        conversation_id: &str,
        assistant_message_id: &str,
    ) -> ConversationTurnTrace {
        let committed = self.committed_prefix();
        ConversationTurnTrace {
            schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
            run_id: run_id.to_string(),
            conversation_id: conversation_id.to_string(),
            assistant_message_id: assistant_message_id.to_string(),
            terminal_status: ConversationTurnTraceTerminalStatus::InProgress,
            terminal_error: None,
            truncated: committed.truncated,
            items: committed.items,
        }
    }

    /// Builds the append-only in-progress audit view, including a final unresolved ToolCall.
    ///
    /// This trace is for durable recovery only. Callers that assemble model context must continue
    /// to use [`Self::in_progress_trace`], which retains closed exchanges only.
    pub fn in_progress_audit_trace(
        &self,
        run_id: &str,
        conversation_id: &str,
        assistant_message_id: &str,
    ) -> ConversationTurnTrace {
        ConversationTurnTrace {
            schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
            run_id: run_id.to_string(),
            conversation_id: conversation_id.to_string(),
            assistant_message_id: assistant_message_id.to_string(),
            terminal_status: ConversationTurnTraceTerminalStatus::InProgress,
            terminal_error: None,
            truncated: self.truncated,
            items: self.items.clone(),
        }
    }
}

/// Appends one authoritative recovery result to the final unresolved ToolCall of an in-progress
/// trace. The identity and ordering checks make the operation safe to retry during startup.
pub fn conversation_trace_with_recovered_tool_result(
    trace: &ConversationTurnTrace,
    result: &AgentToolResult,
) -> Result<ConversationTurnTrace, String> {
    if trace.terminal_status != ConversationTurnTraceTerminalStatus::InProgress {
        return Err("only an in-progress conversation trace can accept a recovered result".into());
    }
    let Some(ConversationTurnTraceItem::ToolCall {
        call_id,
        tool,
        operation,
        approval_status,
        ..
    }) = trace.items.last()
    else {
        return Err("conversation trace has no final unresolved ToolCall".into());
    };
    if result.call_id != *call_id || result.tool != *tool {
        return Err("recovered ToolResult identity does not match the unresolved ToolCall".into());
    }
    let call = AgentToolCall {
        id: call_id.clone(),
        tool: tool.clone(),
        args: operation.clone(),
        approval_status: *approval_status,
        reason: operation
            .get("reason")
            .and_then(Value::as_str)
            .map(str::to_string),
    };
    let mut recorder = ConversationTraceRecorder::from_durable_trace(
        trace.items.clone(),
        trace
            .items
            .last()
            .map(ConversationTurnTraceItem::sequence)
            .unwrap_or(0)
            .saturating_add(1),
        trace.truncated,
    );
    recorder.record_tool_result(&call, result);
    let recovered = recorder.snapshot().in_progress_audit_trace(
        &trace.run_id,
        &trace.conversation_id,
        &trace.assistant_message_id,
    );
    recovered.validate()?;
    Ok(recovered)
}

/// Appends a recovered ToolResult to both durable audit and replay-safe model context.
///
/// Startup recovery must advance these projections together. The immutable ToolCall model item
/// proves that the result closes the exact provider/runtime identity persisted before dispatch.
pub fn conversation_trace_snapshot_with_recovered_tool_result(
    trace: &ConversationTurnTrace,
    model_context_items: Vec<ConversationModelContextItem>,
    result: &AgentToolResult,
) -> Result<ConversationTraceSnapshot, String> {
    if trace.terminal_status != ConversationTurnTraceTerminalStatus::InProgress {
        return Err("only an in-progress conversation trace can accept a recovered result".into());
    }
    let Some(ConversationTurnTraceItem::ToolCall {
        call_id,
        tool,
        operation,
        approval_status,
        ..
    }) = trace.items.last()
    else {
        return Err("conversation trace has no final unresolved ToolCall".into());
    };
    if result.call_id != *call_id || result.tool != *tool {
        return Err("recovered ToolResult identity does not match the unresolved ToolCall".into());
    }
    let call = AgentToolCall {
        id: call_id.clone(),
        tool: tool.clone(),
        args: operation.clone(),
        approval_status: *approval_status,
        reason: operation
            .get("reason")
            .and_then(Value::as_str)
            .map(str::to_string),
    };
    let next_sequence = trace
        .items
        .last()
        .map(ConversationTurnTraceItem::sequence)
        .unwrap_or(0)
        .saturating_add(1);
    let mut recorder =
        ConversationTraceRecorder::from_durable_snapshot(ConversationTraceSnapshot {
            items: trace.items.clone(),
            model_context_items,
            next_sequence,
            truncated: trace.truncated,
        });
    let model_result = crate::tools::model_projection_for_persisted_continuation(result);
    let model_observation = crate::context::ModelToolResultGate::new(
        crate::context::ContextTextBudget::heuristic(crate::context::MODEL_TOOL_RESULT_MAX_TOKENS),
    )
    .project(&call.id, !model_result.ok, &model_result, None)
    .content;
    record_model_tool_exchange_with_projection(
        &mut recorder,
        &call,
        result,
        None,
        Some(&model_observation),
        ConversationHistoryArchiveTraceMetadata::default(),
    )?;
    Ok(recorder.snapshot())
}

#[derive(Debug, Clone, PartialEq)]
pub struct TerminalConversationTraceProjection {
    pub trace: ConversationTurnTrace,
    pub model_context_items: Vec<ConversationModelContextItem>,
}

pub fn cancelled_conversation_trace_from_checkpoint(
    checkpoint: &AgentRunCheckpoint,
    conversation_id: &str,
    assistant_message_id: &str,
    call: &AgentToolCall,
    result: &AgentToolResult,
    model_observation: &str,
    reason: &str,
) -> Result<TerminalConversationTraceProjection, String> {
    terminal_conversation_trace_from_checkpoint(
        checkpoint,
        conversation_id,
        assistant_message_id,
        call,
        result,
        model_observation,
        ConversationTurnTraceTerminalStatus::Cancelled,
        reason,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn terminal_conversation_trace_from_checkpoint(
    checkpoint: &AgentRunCheckpoint,
    conversation_id: &str,
    assistant_message_id: &str,
    call: &AgentToolCall,
    result: &AgentToolResult,
    model_observation: &str,
    terminal_status: ConversationTurnTraceTerminalStatus,
    reason: &str,
) -> Result<TerminalConversationTraceProjection, String> {
    if !terminal_status.is_terminal() {
        return Err("terminal conversation projection requires a terminal status".to_string());
    }
    let mut recorder = ConversationTraceRecorder::from_checkpoint_with_model_context(
        checkpoint.conversation_trace_items.clone(),
        checkpoint.conversation_model_context_items.clone(),
        checkpoint.next_conversation_trace_sequence,
        checkpoint.conversation_trace_truncated,
    );
    record_model_tool_exchange_with_projection(
        &mut recorder,
        call,
        result,
        None,
        Some(model_observation),
        ConversationHistoryArchiveTraceMetadata::default(),
    )?;
    let model_context_items = recorder.snapshot().model_context_items;
    let trace = recorder.finish(
        &checkpoint.run_id,
        conversation_id,
        assistant_message_id,
        terminal_status,
        Some(reason),
    );
    trace.validate_complete_model_context(&model_context_items)?;
    Ok(TerminalConversationTraceProjection {
        trace,
        model_context_items,
    })
}

/// Builds the Host-side approval snapshot with a model observation already finalized by the
/// request's central Tool-result budget gate.
///
/// The Archive pointer and model text enter the append-only snapshot together, preventing a
/// restart from observing a bounded result without its exact recovery route.
pub fn conversation_trace_snapshot_from_checkpoint_and_continuation_with_projection(
    checkpoint: &AgentRunCheckpoint,
    call: &AgentToolCall,
    result: &AgentToolResult,
    assistant_message_id: Option<&str>,
    model_observation: &str,
    archive: ConversationHistoryArchiveTraceMetadata,
) -> Result<ConversationTraceSnapshot, String> {
    let mut recorder = ConversationTraceRecorder::from_checkpoint_with_model_context(
        checkpoint.conversation_trace_items.clone(),
        checkpoint.conversation_model_context_items.clone(),
        checkpoint.next_conversation_trace_sequence,
        checkpoint.conversation_trace_truncated,
    );
    let result_sequence = recorder.require_recorded_tool_call(call)?;
    let history_ref = assistant_message_id.map(|assistant_message_id| {
        crate::ContextHistoryRef::trace_item(
            assistant_message_id,
            result_sequence.saturating_add(1),
        )
    });
    record_model_tool_exchange_with_projection(
        &mut recorder,
        call,
        result,
        history_ref.as_ref(),
        Some(model_observation),
        archive,
    )?;
    Ok(recorder.snapshot())
}

fn record_model_tool_exchange_with_projection(
    recorder: &mut ConversationTraceRecorder,
    call: &AgentToolCall,
    result: &AgentToolResult,
    history_ref: Option<&crate::ContextHistoryRef>,
    model_observation: Option<&str>,
    archive: ConversationHistoryArchiveTraceMetadata,
) -> Result<(), String> {
    let durable_result = canonical_tool_result_for_context(result);
    let llm_result = crate::tools::model_projection_for_persisted_continuation(result);
    let model_observation = model_observation
        .map(ToString::to_string)
        .unwrap_or_else(|| render_tool_observation_with_history_ref(&llm_result, history_ref));
    recorder.require_recorded_tool_call(call)?;
    if let Some(sequence) = recorder.record_tool_result_with_archive(call, &durable_result, archive)
    {
        recorder.record_model_message(
            sequence,
            0,
            &LlmMessage::tool_result(call.id.clone(), model_observation, !llm_result.ok),
        )?;
    }
    Ok(())
}

pub fn cancelled_conversation_trace_from_snapshot(
    snapshot: ConversationTraceSnapshot,
    run_id: &str,
    conversation_id: &str,
    assistant_message_id: &str,
    reason: &str,
) -> Result<TerminalConversationTraceProjection, String> {
    terminal_conversation_trace_from_snapshot(
        snapshot,
        run_id,
        conversation_id,
        assistant_message_id,
        ConversationTurnTraceTerminalStatus::Cancelled,
        reason,
    )
}

pub fn terminal_conversation_trace_from_snapshot(
    snapshot: ConversationTraceSnapshot,
    run_id: &str,
    conversation_id: &str,
    assistant_message_id: &str,
    terminal_status: ConversationTurnTraceTerminalStatus,
    reason: &str,
) -> Result<TerminalConversationTraceProjection, String> {
    terminal_conversation_trace_from_snapshot_with_tool_approval(
        snapshot,
        run_id,
        conversation_id,
        assistant_message_id,
        terminal_status,
        reason,
        None,
    )
}

pub(crate) fn terminal_conversation_trace_from_snapshot_with_tool_approval(
    snapshot: ConversationTraceSnapshot,
    run_id: &str,
    conversation_id: &str,
    assistant_message_id: &str,
    terminal_status: ConversationTurnTraceTerminalStatus,
    reason: &str,
    terminal_tool_approval_status: Option<AgentApprovalStatus>,
) -> Result<TerminalConversationTraceProjection, String> {
    if !terminal_status.is_terminal() {
        return Err("terminal conversation projection requires a terminal status".to_string());
    }
    let mut recorder = ConversationTraceRecorder::from_durable_snapshot(snapshot);
    if let Some(mut call) = recorder.unresolved_tool_call() {
        if let Some(approval_status) = terminal_tool_approval_status {
            call.approval_status = approval_status;
        }
        let result = AgentToolResult {
            exact_archive_file: None,
            call_id: call.id.clone(),
            tool: call.tool.clone(),
            ok: false,
            result: Some(json!({
                "resultAvailable": false,
                "terminalStatus": terminal_status,
            })),
            error: Some(reason.to_string()),
        };
        record_model_tool_exchange_with_projection(
            &mut recorder,
            &call,
            &result,
            None,
            Some(
                if terminal_status == ConversationTurnTraceTerminalStatus::Cancelled {
                    "The Tool Call was cancelled before a verifiable result was available."
                } else {
                    "The Tool Call ended without a verifiable result."
                },
            ),
            ConversationHistoryArchiveTraceMetadata::default(),
        )?;
    }
    let model_context_items = recorder.snapshot().model_context_items;
    let trace = recorder.finish(
        run_id,
        conversation_id,
        assistant_message_id,
        terminal_status,
        Some(reason),
    );
    trace.validate_complete_model_context(&model_context_items)?;
    Ok(TerminalConversationTraceProjection {
        trace,
        model_context_items,
    })
}

pub fn failed_conversation_trace_without_items(
    run_id: &str,
    conversation_id: &str,
    assistant_message_id: &str,
    error: &str,
) -> ConversationTurnTrace {
    terminal_trace_without_items(
        run_id,
        conversation_id,
        assistant_message_id,
        ConversationTurnTraceTerminalStatus::Failed,
        Some(error),
    )
}

pub fn completed_conversation_trace_without_items(
    run_id: &str,
    conversation_id: &str,
    assistant_message_id: &str,
) -> ConversationTurnTrace {
    terminal_trace_without_items(
        run_id,
        conversation_id,
        assistant_message_id,
        ConversationTurnTraceTerminalStatus::Completed,
        None,
    )
}

pub fn cancelled_conversation_trace_without_items(
    run_id: &str,
    conversation_id: &str,
    assistant_message_id: &str,
    reason: &str,
) -> ConversationTurnTrace {
    terminal_trace_without_items(
        run_id,
        conversation_id,
        assistant_message_id,
        ConversationTurnTraceTerminalStatus::Cancelled,
        Some(reason),
    )
}

fn terminal_trace_without_items(
    run_id: &str,
    conversation_id: &str,
    assistant_message_id: &str,
    terminal_status: ConversationTurnTraceTerminalStatus,
    terminal_error: Option<&str>,
) -> ConversationTurnTrace {
    let (terminal_error, truncated) = terminal_error
        .map(project_terminal_error)
        .map(|(value, truncated)| (Some(value), truncated))
        .unwrap_or((None, false));
    ConversationTurnTrace {
        schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
        run_id: run_id.to_string(),
        conversation_id: conversation_id.to_string(),
        assistant_message_id: assistant_message_id.to_string(),
        terminal_status,
        terminal_error,
        truncated,
        items: Vec::new(),
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ConversationTraceRecorder {
    items: Vec<ConversationTurnTraceItem>,
    model_context_items: Vec<ConversationModelContextItem>,
    next_sequence: u64,
    truncated: bool,
    items_are_durable: bool,
}

impl ConversationTraceRecorder {
    pub(crate) fn from_checkpoint_with_model_context(
        items: Vec<ConversationTurnTraceItem>,
        model_context_items: Vec<ConversationModelContextItem>,
        next_sequence: u64,
        truncated: bool,
    ) -> Self {
        let inferred_next = items
            .iter()
            .map(ConversationTurnTraceItem::sequence)
            .max()
            .map(|sequence| sequence.saturating_add(1))
            .unwrap_or(0);
        Self {
            items,
            model_context_items,
            next_sequence: next_sequence.max(inferred_next),
            truncated,
            // A current approval checkpoint is committed with the same append-only Trace prefix.
            // Resuming it may append a ToolResult, but must never rewrite the frozen ToolCall
            // approval state after a restart.
            items_are_durable: true,
        }
    }

    fn from_durable_trace(
        items: Vec<ConversationTurnTraceItem>,
        next_sequence: u64,
        truncated: bool,
    ) -> Self {
        let inferred_next = items
            .iter()
            .map(ConversationTurnTraceItem::sequence)
            .max()
            .map(|sequence| sequence.saturating_add(1))
            .unwrap_or(0);
        Self {
            items,
            model_context_items: Vec::new(),
            next_sequence: next_sequence.max(inferred_next),
            truncated,
            items_are_durable: true,
        }
    }

    pub(crate) fn from_durable_snapshot(snapshot: ConversationTraceSnapshot) -> Self {
        let inferred_next = snapshot
            .items
            .iter()
            .map(ConversationTurnTraceItem::sequence)
            .max()
            .map(|sequence| sequence.saturating_add(1))
            .unwrap_or(0);
        Self {
            items: snapshot.items,
            model_context_items: snapshot.model_context_items,
            next_sequence: snapshot.next_sequence.max(inferred_next),
            truncated: snapshot.truncated,
            items_are_durable: true,
        }
    }

    fn unresolved_tool_call(&self) -> Option<AgentToolCall> {
        self.items.iter().rev().find_map(|item| {
            let ConversationTurnTraceItem::ToolCall {
                call_id,
                tool,
                operation,
                approval_status,
                ..
            } = item
            else {
                return None;
            };
            let has_result = self.items.iter().any(|candidate| {
                matches!(candidate, ConversationTurnTraceItem::ToolResult { call_id: result_id, .. } if result_id == call_id)
            });
            (!has_result).then(|| AgentToolCall {
                id: call_id.clone(),
                tool: tool.clone(),
                args: operation.clone(),
                approval_status: *approval_status,
                reason: operation
                    .get("reason")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            })
        })
    }

    #[cfg(test)]
    pub(crate) fn checkpoint(
        &self,
    ) -> (
        Vec<ConversationTurnTraceItem>,
        Vec<ConversationModelContextItem>,
        u64,
        bool,
    ) {
        (
            self.items.clone(),
            self.model_context_items.clone(),
            self.next_sequence,
            self.truncated,
        )
    }

    pub(crate) fn checkpoint_snapshot(&self) -> ConversationTraceSnapshot {
        ConversationTraceSnapshot {
            items: self.items.clone(),
            model_context_items: self.model_context_items.clone(),
            next_sequence: self.next_sequence,
            truncated: self.truncated,
        }
    }

    pub(crate) fn mark_truncated(&mut self) {
        self.truncated = true;
    }

    pub(crate) fn snapshot(&self) -> ConversationTraceSnapshot {
        let (items, projected_truncated) = if self.items_are_durable {
            (self.items.clone(), false)
        } else {
            project_durable_trace_items(&self.items)
        };
        ConversationTraceSnapshot {
            items,
            model_context_items: self.model_context_items.clone(),
            next_sequence: self.next_sequence,
            truncated: self.truncated || projected_truncated,
        }
    }

    #[cfg(test)]
    pub(crate) fn committed_item_count(&self) -> usize {
        self.snapshot().committed_prefix().items.len()
    }

    pub(crate) fn record_narration(&mut self, content: &str) -> Result<Option<u64>, String> {
        let content = content.trim();
        if content.is_empty() {
            return Ok(None);
        }
        let (content, redacted) = sanitize_text(content);
        let sequence = self.take_sequence();
        self.items
            .push(ConversationTurnTraceItem::AssistantNarration {
                sequence,
                content: content.clone(),
                truncated: redacted,
            });
        self.record_model_message(
            sequence,
            0,
            &LlmMessage::text(crate::llm::LlmMessageRole::Assistant, content),
        )?;
        self.truncated |= redacted;
        Ok(Some(sequence))
    }

    pub(crate) fn record_context_compaction_started(
        &mut self,
        operation_id: &str,
    ) -> Result<u64, String> {
        if operation_id.trim().is_empty()
            || operation_id.len() > 2_048
            || operation_id.chars().any(char::is_control)
        {
            return Err("context compaction trace identity is invalid".to_string());
        }
        if let Some(sequence) = self.items.iter().find_map(|item| match item {
            ConversationTurnTraceItem::ContextCompactionLifecycle {
                sequence,
                phase: ConversationContextCompactionLifecyclePhase::Started,
                operation_id: existing,
                ..
            } if existing == operation_id => Some(*sequence),
            _ => None,
        }) {
            return Ok(sequence);
        }
        if matches!(
            self.items.last(),
            Some(ConversationTurnTraceItem::ToolCall { .. })
        ) {
            return Err("context compaction cannot split an unresolved tool exchange".to_string());
        }
        let sequence = self.take_sequence();
        self.items
            .push(ConversationTurnTraceItem::ContextCompactionLifecycle {
                sequence,
                phase: ConversationContextCompactionLifecyclePhase::Started,
                operation_id: operation_id.to_string(),
                outcome: None,
            });
        Ok(sequence)
    }

    pub(crate) fn record_context_compaction_finished(
        &mut self,
        operation_id: &str,
        outcome: AgentContextCompactionEventOutcome,
    ) -> Result<u64, String> {
        let Some(start_sequence) = self.items.iter().find_map(|item| match item {
            ConversationTurnTraceItem::ContextCompactionLifecycle {
                sequence,
                phase: ConversationContextCompactionLifecyclePhase::Started,
                operation_id: existing,
                ..
            } if existing == operation_id => Some(*sequence),
            _ => None,
        }) else {
            return Err("context compaction finish is missing its durable start".to_string());
        };
        if let Some(existing_outcome) = self.items.iter().find_map(|item| match item {
            ConversationTurnTraceItem::ContextCompactionLifecycle {
                phase: ConversationContextCompactionLifecyclePhase::Finished,
                operation_id: existing,
                outcome,
                ..
            } if existing == operation_id => *outcome,
            _ => None,
        }) {
            return (existing_outcome == outcome)
                .then_some(start_sequence)
                .ok_or_else(|| "context compaction outcome conflicts with its trace".to_string());
        }
        let sequence = self.take_sequence();
        self.items
            .push(ConversationTurnTraceItem::ContextCompactionLifecycle {
                sequence,
                phase: ConversationContextCompactionLifecyclePhase::Finished,
                operation_id: operation_id.to_string(),
                outcome: Some(outcome),
            });
        Ok(start_sequence)
    }

    pub(crate) fn record_runtime_error(
        &mut self,
        message: &str,
        recoverable: bool,
        code: Option<&str>,
    ) -> Result<u64, String> {
        if message.trim().is_empty() {
            return Err("Runtime error trace message cannot be empty".to_string());
        }
        if code.is_some_and(|code| {
            code.trim().is_empty() || code.len() > 128 || code.chars().any(char::is_control)
        }) {
            return Err("Runtime error trace code is invalid".to_string());
        }
        if matches!(
            self.items.last(),
            Some(ConversationTurnTraceItem::ToolCall { .. })
        ) {
            return Err("Runtime error cannot split an unresolved tool exchange".to_string());
        }
        let (message, message_truncated) = project_terminal_error(message);
        let (code, code_truncated) = code
            .map(sanitize_text)
            .map(|(value, truncated)| (Some(value), truncated))
            .unwrap_or((None, false));
        let truncated = message_truncated || code_truncated;
        let sequence = self.take_sequence();
        self.items.push(ConversationTurnTraceItem::RuntimeError {
            sequence,
            message,
            recoverable,
            code,
            truncated,
        });
        self.truncated |= truncated;
        Ok(sequence)
    }

    pub(crate) fn record_model_message(
        &mut self,
        sequence: u64,
        ordinal: u32,
        message: &LlmMessage,
    ) -> Result<(), String> {
        if self
            .model_context_items
            .iter()
            .any(|item| item.sequence == sequence && item.ordinal == ordinal)
        {
            return Ok(());
        }
        let (item, truncated) = model_context_item_from_message(sequence, ordinal, message)?;
        self.model_context_items.push(item);
        self.model_context_items
            .sort_by_key(|item| (item.sequence, item.ordinal));
        self.truncated |= truncated;
        Ok(())
    }

    /// Records one split durable Assistant Tool Call with the exact identity from its original
    /// provider turn.
    ///
    /// The durable model log intentionally stores one Assistant message per Tool Call. For a
    /// grouped provider turn, deriving identity from that split message would renumber calls and
    /// lose the provider-owned call ID. Runtime therefore supplies the frozen mapping explicitly.
    pub(crate) fn record_model_tool_call_message(
        &mut self,
        sequence: u64,
        ordinal: u32,
        message: &LlmMessage,
        provider_identity: AgentProviderToolCallIdentity,
    ) -> Result<(), String> {
        if self
            .model_context_items
            .iter()
            .any(|item| item.sequence == sequence && item.ordinal == ordinal)
        {
            return Ok(());
        }
        let provider_identities = BTreeMap::from([(
            provider_identity.runtime_call_id.clone(),
            provider_identity.clone(),
        )]);
        let (item, truncated) = model_context_item_from_message_with_identities(
            sequence,
            ordinal,
            message,
            Some(&provider_identities),
        )?;
        if item.role != "assistant"
            || item.tool_calls.len() != 1
            || provider_identity.runtime_call_id != item.tool_calls[0].id
        {
            return Err(
                "durable Assistant Tool Call identity does not match its model projection"
                    .to_string(),
            );
        }
        self.model_context_items.push(item);
        self.model_context_items
            .sort_by_key(|item| (item.sequence, item.ordinal));
        self.truncated |= truncated;
        Ok(())
    }

    /// Applies the same durable approval-barrier projection as Context. Raw queued external calls
    /// never enter the persisted model log, while the live provider turn remains in memory.
    pub(crate) fn omit_model_tool_calls(&mut self, omitted_call_ids: &BTreeSet<String>) {
        if omitted_call_ids.is_empty() {
            return;
        }
        for item in &mut self.model_context_items {
            if item.role == "assistant" {
                item.tool_calls
                    .retain(|call| !omitted_call_ids.contains(&call.id));
            }
        }
    }

    pub(crate) fn record_user_guidance(
        &mut self,
        guidance_id: &str,
        client_message_id: &str,
        content: &str,
        attachments: &[AgentInputAttachment],
        created_at: i64,
    ) -> Option<u64> {
        let content = content.trim();
        if content.is_empty()
            || matches!(
                self.items.last(),
                Some(ConversationTurnTraceItem::ToolCall { .. })
            )
        {
            return None;
        }
        let (content, content_redacted) = sanitize_text(content);
        let (attachments, attachment_redacted) = trace_attachments_from_input(attachments);
        let sequence = self.take_sequence();
        self.items.push(ConversationTurnTraceItem::UserGuidance {
            sequence,
            guidance_id: guidance_id.to_string(),
            client_message_id: client_message_id.to_string(),
            content,
            attachments,
            created_at,
            truncated: content_redacted || attachment_redacted,
        });
        self.truncated |= content_redacted || attachment_redacted;
        Some(sequence)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn record_agent_mailbox_delivery(
        &mut self,
        expected_sequence: u64,
        receipt_id: &str,
        message_id: &str,
        sender_agent_id: &str,
        sender_task_name: &str,
        sender_task_path: &str,
        kind: crate::AgentMailboxKind,
        content: &str,
        created_at: i64,
    ) -> Result<Option<String>, String> {
        let content = content.trim();
        if receipt_id.trim().is_empty()
            || message_id.trim().is_empty()
            || sender_agent_id.trim().is_empty()
            || sender_task_name.trim().is_empty()
            || sender_task_path.trim().is_empty()
            || content.is_empty()
            || created_at < 0
        {
            return Err("Agent mailbox delivery identity is invalid".to_string());
        }
        let (model_content, model_truncated) = project_agent_mailbox_model_envelope(
            sender_agent_id,
            sender_task_name,
            sender_task_path,
            kind,
            content,
        )?;
        let (trace_content, trace_truncated) = project_agent_mailbox_envelope_with_budget(
            sender_agent_id,
            sender_task_name,
            sender_task_path,
            kind,
            content,
            DurableTraceProjectionLimits::USER_GUIDANCE_CHARS,
        )?;
        let truncated = model_truncated || trace_truncated;
        if let Some(existing) = self.items.iter().find(|item| {
            matches!(
                item,
                ConversationTurnTraceItem::AgentMailboxDelivery {
                    message_id: existing_message,
                    ..
                } if existing_message == message_id
            )
        }) {
            return match existing {
                ConversationTurnTraceItem::AgentMailboxDelivery {
                    sequence,
                    receipt_id: existing_receipt,
                    message_id: existing_message,
                    sender_agent_id: existing_sender,
                    sender_task_name: existing_task_name,
                    sender_task_path: existing_task_path,
                    kind: existing_kind,
                    content: existing_content,
                    created_at: existing_created_at,
                    truncated: existing_truncated,
                } if *sequence == expected_sequence
                    && existing_receipt == receipt_id
                    && existing_message == message_id
                    && existing_sender == sender_agent_id
                    && existing_task_name == sender_task_name
                    && existing_task_path == sender_task_path
                    && *existing_kind == kind
                    && existing_content == &trace_content
                    && *existing_created_at == created_at
                    && *existing_truncated == truncated =>
                {
                    Ok(None)
                }
                _ => Err("Agent mailbox delivery identity conflicts with the trace".to_string()),
            };
        }
        if self.next_sequence != expected_sequence {
            return Err(format!(
                "Agent mailbox delivery expected trace sequence {expected_sequence}, current is {}",
                self.next_sequence
            ));
        }
        if matches!(
            self.items.last(),
            Some(ConversationTurnTraceItem::ToolCall { .. })
        ) {
            return Err(
                "Agent mailbox delivery cannot split an unresolved tool exchange".to_string(),
            );
        }
        self.items
            .push(ConversationTurnTraceItem::AgentMailboxDelivery {
                sequence: expected_sequence,
                receipt_id: receipt_id.to_string(),
                message_id: message_id.to_string(),
                sender_agent_id: sender_agent_id.to_string(),
                sender_task_name: sender_task_name.to_string(),
                sender_task_path: sender_task_path.to_string(),
                kind,
                content: trace_content,
                created_at,
                truncated,
            });
        self.record_model_message(
            expected_sequence,
            0,
            &LlmMessage::text(crate::llm::LlmMessageRole::User, model_content.clone()),
        )?;
        self.next_sequence = self.next_sequence.saturating_add(1);
        self.truncated |= truncated;
        Ok(Some(model_content))
    }

    pub(crate) fn next_sequence(&self) -> u64 {
        self.next_sequence
    }

    #[cfg(test)]
    pub(crate) fn record_tool_call(&mut self, call: &AgentToolCall) -> Option<u64> {
        self.record_tool_call_with_identity(
            call,
            AgentToolIdentity::Builtin {
                tool_name: call.tool.clone(),
            },
        )
    }

    pub(crate) fn record_tool_call_with_identity(
        &mut self,
        call: &AgentToolCall,
        provenance: AgentToolIdentity,
    ) -> Option<u64> {
        if let Some(sequence) = self.items.iter().find_map(|item| {
            matches!(item, ConversationTurnTraceItem::ToolCall { call_id, .. } if call_id == &call.id)
                .then(|| item.sequence())
        }) {
            return Some(sequence);
        }
        let (operation, redacted) = sanitize_value(&call.args);
        let sequence = self.take_sequence();
        self.items.push(ConversationTurnTraceItem::ToolCall {
            sequence,
            call_id: call.id.clone(),
            tool: call.tool.clone(),
            provenance,
            operation,
            approval_status: call.approval_status,
            truncated: redacted,
        });
        self.truncated |= redacted;
        Some(sequence)
    }

    /// Returns the sequence of a ToolCall already frozen into the current trace.
    ///
    /// Approval and restart continuations must reuse the trusted provenance captured before the
    /// side-effect boundary. They are never allowed to infer a new identity from a tool name.
    pub(crate) fn require_recorded_tool_call(&self, call: &AgentToolCall) -> Result<u64, String> {
        let sequence = self
            .items
            .iter()
            .find_map(|item| match item {
                ConversationTurnTraceItem::ToolCall {
                    sequence,
                    call_id,
                    tool,
                    ..
                } if call_id == &call.id && tool == &call.tool => Some(*sequence),
                _ => None,
            })
            .ok_or_else(|| {
                "current continuation is missing its frozen ToolCall provenance".to_string()
            })?;
        let has_model_context = self.model_context_items.iter().any(|item| {
            item.sequence == sequence
                && item.role == "assistant"
                && item.tool_call_id.is_none()
                && item.tool_calls.len() == 1
                && item.tool_calls[0].id == call.id
                && item.tool_calls[0].name == call.tool
        });
        if !has_model_context {
            return Err(
                "current continuation is missing its immutable ToolCall model context".to_string(),
            );
        }
        Ok(sequence)
    }

    /// Approval enriches the state of the original model call; it does not replace the model's
    /// arguments with a second, presentation-oriented representation.
    pub(crate) fn enrich_tool_call(&mut self, action: &AgentProposedAction) {
        let (call_id, approval_status) = match action {
            AgentProposedAction::Diff { diff } => (&diff.id, diff.approval_status),
            AgentProposedAction::FileWrite { file_write } => {
                (&file_write.id, file_write.approval_status)
            }
            AgentProposedAction::Command { command } => (&command.id, command.approval_status),
            AgentProposedAction::SkillMaterialization { materialization } => {
                (&materialization.id, materialization.approval_status)
            }
            AgentProposedAction::SkillScript { script } => (&script.id, script.approval_status),
            AgentProposedAction::OfficeOperation { office_operation } => {
                (&office_operation.id, office_operation.approval_status)
            }
            AgentProposedAction::SkillInstallation { installation } => {
                (&installation.id, installation.approval_status)
            }
            AgentProposedAction::ToolCall { call } => (&call.id, call.approval_status),
            AgentProposedAction::McpToolCall { approval } => {
                (&approval.identity.call_id, approval.call.approval_status)
            }
        };
        if let Some(ConversationTurnTraceItem::ToolCall {
            approval_status: current,
            ..
        }) = self.items.iter_mut().rev().find(
            |item| matches!(item, ConversationTurnTraceItem::ToolCall { call_id: candidate, .. } if candidate == call_id),
        ) {
            *current = approval_status;
        }
    }

    pub(crate) fn record_tool_result(
        &mut self,
        call: &AgentToolCall,
        result: &AgentToolResult,
    ) -> Option<u64> {
        self.record_tool_result_with_archive(call, result, Default::default())
    }

    pub(crate) fn pending_tool_result_sequence(&self, call_id: &str) -> Option<u64> {
        let has_call = self.items.iter().any(
            |item| matches!(item, ConversationTurnTraceItem::ToolCall { call_id: candidate, .. } if candidate == call_id),
        );
        let has_result = self.items.iter().any(
            |item| matches!(item, ConversationTurnTraceItem::ToolResult { call_id: candidate, .. } if candidate == call_id),
        );
        (has_call && !has_result).then_some(self.next_sequence)
    }

    pub(crate) fn record_tool_result_with_archive(
        &mut self,
        call: &AgentToolCall,
        result: &AgentToolResult,
        archive: ConversationHistoryArchiveTraceMetadata,
    ) -> Option<u64> {
        let call_index = self.items.iter().rposition(
            |item| matches!(item, ConversationTurnTraceItem::ToolCall { call_id, .. } if call_id == &call.id),
        )?;
        if self.items.iter().any(|item| {
            matches!(item, ConversationTurnTraceItem::ToolResult { call_id, .. } if call_id == &call.id)
        }) {
            return self.items.iter().find_map(|item| {
                matches!(item, ConversationTurnTraceItem::ToolResult { call_id, .. } if call_id == &call.id)
                    .then(|| item.sequence())
            });
        }
        if !self.items_are_durable {
            if let ConversationTurnTraceItem::ToolCall {
                approval_status, ..
            } = &mut self.items[call_index]
            {
                *approval_status = call.approval_status;
            }
        }

        let sequence = self.take_sequence();
        let mut item = if self.items_are_durable {
            projected_tool_result_trace_item(sequence, call, result)
        } else {
            checkpoint_tool_result_trace_item(sequence, call, result)
        };
        if let ConversationTurnTraceItem::ToolResult {
            truncated,
            archive: item_archive,
            ..
        } = &mut item
        {
            *item_archive = archive;
            if self.items_are_durable {
                // `projected_tool_result_trace_item` has already crossed the same bounded
                // history-projection boundary that `project_durable_trace_items` applies when a
                // live Runtime snapshot reaches terminal settlement. Preserve that fact in the
                // archive metadata as well as on the item itself. Otherwise a precommitted
                // wait_agent result whose generic payload exceeds the history limit differs from
                // the later Runtime terminal projection only by this bit, and the append-only
                // exact-prefix guard correctly rejects the terminal commit.
                item_archive.history_projection_truncated |= *truncated;
            }
        }
        self.truncated |= matches!(
            &item,
            ConversationTurnTraceItem::ToolResult {
                truncated: true,
                ..
            }
        );
        self.items.push(item);
        Some(sequence)
    }

    pub(crate) fn finish(
        &self,
        run_id: &str,
        conversation_id: &str,
        assistant_message_id: &str,
        terminal_status: ConversationTurnTraceTerminalStatus,
        terminal_error: Option<&str>,
    ) -> ConversationTurnTrace {
        let mut recorder = self.clone();
        recorder.close_unresolved(terminal_status, terminal_error);
        recorder.close_unresolved_context_compactions(terminal_status);
        let recorder_truncated = recorder.truncated;
        let (items, projected_truncated) = if recorder.items_are_durable {
            (recorder.items, false)
        } else {
            project_durable_trace_items(&recorder.items)
        };
        let (terminal_error, terminal_redacted) = terminal_error
            .map(project_terminal_error)
            .map(|(value, truncated)| (Some(value), truncated))
            .unwrap_or((None, false));
        ConversationTurnTrace {
            schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
            run_id: run_id.to_string(),
            conversation_id: conversation_id.to_string(),
            assistant_message_id: assistant_message_id.to_string(),
            terminal_status,
            terminal_error,
            truncated: recorder_truncated || projected_truncated || terminal_redacted,
            items,
        }
    }

    fn close_unresolved(
        &mut self,
        terminal_status: ConversationTurnTraceTerminalStatus,
        terminal_error: Option<&str>,
    ) {
        let Some((call_id, tool, operation, approval_status)) =
            self.items.iter().rev().find_map(|item| {
            if let ConversationTurnTraceItem::ToolCall {
                call_id,
                tool,
                operation,
                approval_status,
                ..
            } = item
            {
                let has_result = self.items.iter().any(|candidate| {
                    matches!(candidate, ConversationTurnTraceItem::ToolResult { call_id: result_id, .. } if result_id == call_id)
                });
                (!has_result).then(|| {
                    (
                        call_id.clone(),
                        tool.clone(),
                        operation.clone(),
                        *approval_status,
                    )
                })
            } else {
                None
            }
        }) else {
            return;
        };
        let status = if terminal_status == ConversationTurnTraceTerminalStatus::Cancelled {
            ConversationTraceToolResultStatus::Cancelled
        } else {
            ConversationTraceToolResultStatus::Failed
        };
        let fallback = if terminal_status == ConversationTurnTraceTerminalStatus::Cancelled {
            "Run cancelled before a verifiable tool result was available."
        } else {
            "Run ended before a verifiable tool result was available."
        };
        let (raw_error, raw_redacted) = sanitize_text(terminal_error.unwrap_or(fallback));
        let raw_observation = json!({
            "resultAvailable": false,
            "terminalStatus": terminal_status,
        });
        let (observation, error, projection_truncated) = if self.items_are_durable {
            let (observation, error, error_truncated) =
                project_tool_result(&tool, Some(&operation), &raw_observation, Some(&raw_error));
            (
                observation.value,
                error,
                observation.truncated || error_truncated,
            )
        } else {
            (raw_observation, Some(raw_error), false)
        };
        let sequence = self.take_sequence();
        let item = ConversationTurnTraceItem::ToolResult {
            sequence,
            call_id,
            tool,
            status,
            success: false,
            observation,
            approval_status,
            error,
            truncated: raw_redacted || projection_truncated,
            archive: Default::default(),
        };
        self.truncated |= matches!(
            item,
            ConversationTurnTraceItem::ToolResult {
                truncated: true,
                ..
            }
        );
        self.items.push(item);
    }

    fn close_unresolved_context_compactions(
        &mut self,
        terminal_status: ConversationTurnTraceTerminalStatus,
    ) {
        if !terminal_status.is_terminal() {
            return;
        }
        let mut operations = Vec::<(String, bool)>::new();
        for item in &self.items {
            let ConversationTurnTraceItem::ContextCompactionLifecycle {
                phase,
                operation_id,
                ..
            } = item
            else {
                continue;
            };
            match phase {
                ConversationContextCompactionLifecyclePhase::Started => {
                    operations.push((operation_id.clone(), false));
                }
                ConversationContextCompactionLifecyclePhase::Finished => {
                    if let Some((_, finished)) = operations
                        .iter_mut()
                        .find(|(candidate, _)| candidate == operation_id)
                    {
                        *finished = true;
                    }
                }
            }
        }
        let outcome = match terminal_status {
            ConversationTurnTraceTerminalStatus::Cancelled => {
                AgentContextCompactionEventOutcome::Cancelled
            }
            ConversationTurnTraceTerminalStatus::Failed => {
                AgentContextCompactionEventOutcome::Failed
            }
            ConversationTurnTraceTerminalStatus::Completed => {
                AgentContextCompactionEventOutcome::Skipped
            }
            ConversationTurnTraceTerminalStatus::InProgress => return,
        };
        for (operation_id, finished) in operations {
            if finished {
                continue;
            }
            let sequence = self.take_sequence();
            self.items
                .push(ConversationTurnTraceItem::ContextCompactionLifecycle {
                    sequence,
                    phase: ConversationContextCompactionLifecyclePhase::Finished,
                    operation_id,
                    outcome: Some(outcome),
                });
        }
    }

    fn take_sequence(&mut self) -> u64 {
        let sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.saturating_add(1);
        sequence
    }
}

pub(crate) fn trace_attachments_from_input(
    attachments: &[AgentInputAttachment],
) -> (Vec<ConversationTraceAttachment>, bool) {
    let mut redacted = false;
    let attachments = attachments
        .iter()
        .map(|attachment| {
            let (name, name_redacted) = sanitize_text(&attachment.name);
            let (mime_type, mime_redacted) =
                sanitize_optional_text(attachment.mime_type.as_deref());
            redacted |= name_redacted || mime_redacted || attachment.truncated.unwrap_or(false);
            ConversationTraceAttachment {
                id: attachment.id.clone(),
                kind: attachment.kind,
                name,
                mime_type,
                size_bytes: attachment.size_bytes,
            }
        })
        .collect();
    (attachments, redacted)
}

pub(crate) fn render_user_guidance_content(
    content: &str,
    attachments: &[ConversationTraceAttachment],
) -> String {
    if attachments.is_empty() {
        return content.to_string();
    }
    let attachment_list = attachments
        .iter()
        .map(|attachment| {
            format!(
                "- {} ({}, {} bytes, id: {})",
                attachment.name,
                match attachment.kind {
                    AgentInputAttachmentKind::File => "file",
                    AgentInputAttachmentKind::Image => "image",
                },
                attachment.size_bytes,
                attachment.id
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!("{content}\n\nAttachments supplied with this user guidance:\n{attachment_list}")
}

fn checkpoint_tool_result_trace_item(
    sequence: u64,
    call: &AgentToolCall,
    result: &AgentToolResult,
) -> ConversationTurnTraceItem {
    let status = result_status(result);
    let success = status == ConversationTraceToolResultStatus::Succeeded;
    let (observation, result_redacted) =
        sanitize_runtime_value(result.result.as_ref().unwrap_or(&Value::Null));
    let (error, error_redacted) = result
        .error
        .as_deref()
        .map(sanitize_runtime_text)
        .map(|(value, redacted)| (Some(value), redacted))
        .unwrap_or((None, false));
    ConversationTurnTraceItem::ToolResult {
        sequence,
        call_id: call.id.clone(),
        tool: call.tool.clone(),
        status,
        success,
        observation,
        approval_status: call.approval_status,
        error,
        truncated: result_redacted || error_redacted,
        archive: Default::default(),
    }
}

fn project_durable_trace_items(
    items: &[ConversationTurnTraceItem],
) -> (Vec<ConversationTurnTraceItem>, bool) {
    let mut projected = Vec::with_capacity(items.len());
    let mut operations = BTreeMap::<String, Value>::new();
    let mut failure_signatures = BTreeMap::<String, String>::new();
    let mut trace_truncated = false;

    for item in items {
        let projected_item = match item {
            ConversationTurnTraceItem::AssistantNarration {
                sequence,
                content,
                truncated,
            } => {
                let (content, projection_truncated) = project_narration(content);
                trace_truncated |= *truncated || projection_truncated;
                ConversationTurnTraceItem::AssistantNarration {
                    sequence: *sequence,
                    content,
                    truncated: *truncated || projection_truncated,
                }
            }
            ConversationTurnTraceItem::UserGuidance {
                sequence,
                guidance_id,
                client_message_id,
                content,
                attachments,
                created_at,
                truncated,
            } => {
                let (content, content_truncated) = project_user_guidance(content);
                let mut attachment_truncated = false;
                let attachments = attachments
                    .iter()
                    .map(|attachment| {
                        let (name, name_truncated) = project_attachment_text(&attachment.name);
                        let (mime_type, mime_truncated) = attachment
                            .mime_type
                            .as_deref()
                            .map(project_attachment_text)
                            .map(|(value, truncated)| (Some(value), truncated))
                            .unwrap_or((None, false));
                        attachment_truncated |= name_truncated || mime_truncated;
                        ConversationTraceAttachment {
                            id: attachment.id.clone(),
                            kind: attachment.kind,
                            name,
                            mime_type,
                            size_bytes: attachment.size_bytes,
                        }
                    })
                    .collect();
                let item_truncated = *truncated || content_truncated || attachment_truncated;
                trace_truncated |= item_truncated;
                ConversationTurnTraceItem::UserGuidance {
                    sequence: *sequence,
                    guidance_id: guidance_id.clone(),
                    client_message_id: client_message_id.clone(),
                    content,
                    attachments,
                    created_at: *created_at,
                    truncated: item_truncated,
                }
            }
            ConversationTurnTraceItem::AgentMailboxDelivery {
                sequence,
                receipt_id,
                message_id,
                sender_agent_id,
                sender_task_name,
                sender_task_path,
                kind,
                content,
                created_at,
                truncated,
            } => {
                let (content, content_truncated) = project_user_guidance(content);
                let item_truncated = *truncated || content_truncated;
                trace_truncated |= item_truncated;
                ConversationTurnTraceItem::AgentMailboxDelivery {
                    sequence: *sequence,
                    receipt_id: receipt_id.clone(),
                    message_id: message_id.clone(),
                    sender_agent_id: sender_agent_id.clone(),
                    sender_task_name: sender_task_name.clone(),
                    sender_task_path: sender_task_path.clone(),
                    kind: *kind,
                    content,
                    created_at: *created_at,
                    truncated: item_truncated,
                }
            }
            ConversationTurnTraceItem::ToolCall {
                sequence,
                call_id,
                tool,
                provenance,
                operation,
                approval_status,
                truncated,
            } => {
                operations.insert(call_id.clone(), operation.clone());
                let operation = project_tool_call(tool, operation);
                let item_truncated = *truncated || operation.truncated;
                trace_truncated |= item_truncated;
                ConversationTurnTraceItem::ToolCall {
                    sequence: *sequence,
                    call_id: call_id.clone(),
                    tool: tool.clone(),
                    provenance: provenance.clone(),
                    operation: operation.value,
                    approval_status: *approval_status,
                    truncated: item_truncated,
                }
            }
            ConversationTurnTraceItem::ToolResult {
                sequence,
                call_id,
                tool,
                status,
                success,
                observation,
                approval_status,
                error,
                truncated,
                archive,
            } => {
                let (observation, projected_error, error_truncated) = project_tool_result(
                    tool,
                    operations.get(call_id),
                    observation,
                    error.as_deref(),
                );
                let mut projected_observation = observation.value;
                let mut projected_error = projected_error;
                let mut projection_truncated = observation.truncated || error_truncated;

                if !*success
                    && is_repeat_failure_eligible(tool)
                    && projected_observation
                        .get("repeatedFailure")
                        .and_then(Value::as_bool)
                        != Some(true)
                {
                    let signature = serde_json::to_string(&json!({
                        "tool": tool,
                        "status": status,
                        "observation": &projected_observation,
                        "error": &projected_error,
                    }))
                    .unwrap_or_default();
                    if let Some(first_call_id) = failure_signatures.get(&signature) {
                        projected_observation = json!({
                            "repeatedFailure": true,
                            "sameAsCallId": first_call_id,
                            "detail": "Unchanged failure omitted from conversation history."
                        });
                        projected_error = None;
                        projection_truncated = true;
                    } else {
                        failure_signatures.insert(signature, call_id.clone());
                    }
                }

                let item_truncated = *truncated || projection_truncated;
                let mut archive = archive.clone();
                // `truncated` can already be true because the Runtime checkpoint sanitizer
                // replaced binary/base64 content before this durable projection runs. Treat the
                // complete canonical item truncation bit as the archive fact as well; otherwise a
                // wait_agent ToolResult precommitted from the raw result and the later terminal
                // projection differ only in archive metadata.
                archive.history_projection_truncated |= item_truncated;
                trace_truncated |= item_truncated;
                ConversationTurnTraceItem::ToolResult {
                    sequence: *sequence,
                    call_id: call_id.clone(),
                    tool: tool.clone(),
                    status: *status,
                    success: *success,
                    observation: projected_observation,
                    approval_status: *approval_status,
                    error: projected_error,
                    truncated: item_truncated,
                    archive,
                }
            }
            ConversationTurnTraceItem::CommandSessionLifecycle {
                sequence,
                phase,
                session_id,
                call_id,
                status,
                exit_code,
                latest_sequence,
                output_truncated,
                archive,
                created_at,
            } => ConversationTurnTraceItem::CommandSessionLifecycle {
                sequence: *sequence,
                phase: *phase,
                session_id: session_id.clone(),
                call_id: call_id.clone(),
                status: *status,
                exit_code: *exit_code,
                latest_sequence: *latest_sequence,
                output_truncated: *output_truncated,
                archive: archive.clone(),
                created_at: *created_at,
            },
            ConversationTurnTraceItem::ContextCompactionLifecycle {
                sequence,
                phase,
                operation_id,
                outcome,
            } => ConversationTurnTraceItem::ContextCompactionLifecycle {
                sequence: *sequence,
                phase: *phase,
                operation_id: operation_id.clone(),
                outcome: *outcome,
            },
            ConversationTurnTraceItem::RuntimeError {
                sequence,
                message,
                recoverable,
                code,
                truncated,
            } => {
                let (message, message_truncated) = project_terminal_error(message);
                let (code, code_truncated) = code
                    .as_deref()
                    .map(sanitize_runtime_text)
                    .map(|(value, truncated)| (Some(value), truncated))
                    .unwrap_or((None, false));
                let item_truncated = *truncated || message_truncated || code_truncated;
                trace_truncated |= item_truncated;
                ConversationTurnTraceItem::RuntimeError {
                    sequence: *sequence,
                    message,
                    recoverable: *recoverable,
                    code,
                    truncated: item_truncated,
                }
            }
        };
        projected.push(projected_item);
    }

    (projected, trace_truncated)
}

/// Produces the canonical persisted trace item for a ToolResult.
///
/// Durable settlement verification uses this same projection so an audit receipt cannot be paired
/// with a trace item carrying different (or less complete) result/error evidence.
pub(crate) fn projected_tool_result_trace_item(
    sequence: u64,
    call: &AgentToolCall,
    result: &AgentToolResult,
) -> ConversationTurnTraceItem {
    let status = result_status(result);
    let success = status == ConversationTraceToolResultStatus::Succeeded;
    let (observation, error, error_truncated) = project_tool_result(
        &call.tool,
        Some(&call.args),
        result.result.as_ref().unwrap_or(&Value::Null),
        result.error.as_deref(),
    );
    let history_projection_truncated = observation.truncated || error_truncated;
    ConversationTurnTraceItem::ToolResult {
        sequence,
        call_id: call.id.clone(),
        tool: call.tool.clone(),
        status,
        success,
        observation: observation.value,
        approval_status: call.approval_status,
        error,
        truncated: history_projection_truncated,
        archive: ConversationHistoryArchiveTraceMetadata {
            history_projection_truncated,
            ..Default::default()
        },
    }
}

pub(crate) fn canonical_tool_result_for_context(result: &AgentToolResult) -> AgentToolResult {
    let mut canonical = result.clone();
    if let Some(value) = canonical.result.as_mut() {
        *value = sanitize_runtime_value(value).0;
    }
    if let Some(error) = canonical.error.as_mut() {
        *error = sanitize_runtime_text(error).0;
    }
    canonical
}

pub(crate) fn render_tool_observation(result: &AgentToolResult) -> String {
    render_tool_observation_with_history_ref(result, None)
}

pub(crate) fn render_tool_observation_with_history_ref(
    result: &AgentToolResult,
    history_ref: Option<&crate::ContextHistoryRef>,
) -> String {
    render_tool_observation_with_projection(result, history_ref, None)
}

pub(crate) fn render_tool_observation_with_projection(
    result: &AgentToolResult,
    _history_ref: Option<&crate::ContextHistoryRef>,
    _archive: Option<&ConversationHistoryArchiveTraceMetadata>,
) -> String {
    let mut payload = result
        .result
        .clone()
        .unwrap_or_else(|| json!({ "status": if result.ok { "completed" } else { "failed" } }));
    if !result.ok {
        if let Some(error) = result
            .error
            .as_deref()
            .map(str::trim)
            .filter(|error| !error.is_empty())
        {
            match payload.as_object_mut() {
                Some(object)
                    if !object
                        .values()
                        .any(|value| value_contains_text(value, error)) =>
                {
                    object.insert("error".to_string(), Value::String(error.to_string()));
                }
                Some(_) => {}
                None => {
                    payload = json!({
                        "result": payload,
                        "error": error,
                    });
                }
            }
        }
    }
    serde_json::to_string(&payload).unwrap_or_else(|_| "{}".to_string())
}

fn value_contains_text(value: &Value, expected: &str) -> bool {
    match value {
        Value::String(value) => value.trim() == expected,
        Value::Array(values) => values
            .iter()
            .any(|value| value_contains_text(value, expected)),
        Value::Object(values) => values
            .values()
            .any(|value| value_contains_text(value, expected)),
        _ => false,
    }
}

fn result_status(result: &AgentToolResult) -> ConversationTraceToolResultStatus {
    let value = result.result.as_ref().unwrap_or(&Value::Null);
    let status = value
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    if status.contains("reject") {
        ConversationTraceToolResultStatus::Rejected
    } else if status.contains("conflict") {
        ConversationTraceToolResultStatus::Conflict
    } else if status.contains("cancel")
        || value
            .get("cancelled")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    {
        ConversationTraceToolResultStatus::Cancelled
    } else if result.ok && result.error.is_none() && !status.contains("fail") {
        ConversationTraceToolResultStatus::Succeeded
    } else {
        ConversationTraceToolResultStatus::Failed
    }
}

fn sanitize_optional_text(value: Option<&str>) -> (Option<String>, bool) {
    value
        .map(sanitize_text)
        .map(|(value, redacted)| (Some(value), redacted))
        .unwrap_or((None, false))
}

/// Maximum size of the canonical model-visible wrapper for a Host-authenticated collaboration
/// fact. The durable trace stores the same envelope shape at the smaller history budget while the
/// Mailbox retains the original payload unchanged.
pub(crate) const AGENT_MAILBOX_MODEL_ENVELOPE_MAX_BYTES: usize = 32 * 1_024;

/// Produces the canonical model-visible representation used by the live Runtime, durable model
/// log and checkpoint/resume. The durable trace uses the same encoder with its history budget.
pub(crate) fn project_agent_mailbox_model_envelope(
    sender_agent_id: &str,
    sender_task_name: &str,
    sender_task_path: &str,
    kind: crate::AgentMailboxKind,
    payload: &str,
) -> Result<(String, bool), String> {
    project_agent_mailbox_envelope_with_budget(
        sender_agent_id,
        sender_task_name,
        sender_task_path,
        kind,
        payload,
        AGENT_MAILBOX_MODEL_ENVELOPE_MAX_BYTES,
    )
}

fn project_agent_mailbox_envelope_with_budget(
    sender_agent_id: &str,
    sender_task_name: &str,
    sender_task_path: &str,
    kind: crate::AgentMailboxKind,
    payload: &str,
    maximum_bytes: usize,
) -> Result<(String, bool), String> {
    const SUFFIX: &str = "\n...[agent mailbox payload truncated]";
    let encode = |payload: &str, truncated: bool| {
        serde_json::to_string(&serde_json::json!({
            "type": "agent_collaboration_input",
            "origin": "agent",
            "isHuman": false,
            "senderAgentId": sender_agent_id,
            "senderTaskName": sender_task_name,
            "senderTaskPath": sender_task_path,
            "kind": kind.as_str(),
            "payload": payload,
            "payloadTruncated": truncated,
        }))
        .map_err(|error| format!("cannot encode Agent collaboration envelope: {error}"))
    };
    let full = encode(payload, false)?;
    if full.len() <= maximum_bytes {
        return Ok((full, false));
    }
    let boundaries = std::iter::once(0)
        .chain(payload.char_indices().map(|(index, _)| index).skip(1))
        .chain(std::iter::once(payload.len()))
        .collect::<Vec<_>>();
    let mut low = 0usize;
    let mut high = boundaries.len().saturating_sub(1);
    let mut best = String::new();
    while low <= high {
        let middle = low + (high - low) / 2;
        let boundary = boundaries[middle];
        let projected = format!("{}{}", &payload[..boundary], SUFFIX);
        let encoded = encode(&projected, true)?;
        if encoded.len() <= maximum_bytes {
            best = encoded;
            low = middle.saturating_add(1);
        } else if middle == 0 {
            break;
        } else {
            high = middle.saturating_sub(1);
        }
    }
    if best.is_empty() {
        return Err("Agent collaboration identity exceeds the model envelope budget".to_string());
    }
    Ok((best, true))
}

fn sanitize_text(value: &str) -> (String, bool) {
    sanitize_runtime_text(value)
}

fn sanitize_value(value: &Value) -> (Value, bool) {
    sanitize_runtime_value(value)
}

fn ensure_no_binary_text(label: &str, value: &str) -> Result<(), String> {
    if sanitize_text(value).1 {
        return Err(format!(
            "conversation trace {label} contains binary material"
        ));
    }
    Ok(())
}

fn ensure_no_binary_value(label: &str, value: &Value) -> Result<(), String> {
    // Durable values may intentionally retain a binary-named field with the canonical omission
    // marker. Validate whether sanitization would change the value instead of treating the
    // already-safe marker as fresh binary material.
    if sanitize_value(value).0 != *value {
        return Err(format!(
            "conversation trace {label} contains binary material"
        ));
    }
    Ok(())
}

fn is_provider_safe_tool_name(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{AgentMcpServerScope, AgentMcpToolProvenance};

    fn call(id: &str) -> AgentToolCall {
        AgentToolCall {
            id: id.to_string(),
            tool: "web_fetch".to_string(),
            args: json!({ "url": "https://example.com" }),
            approval_status: AgentApprovalStatus::NotRequired,
            reason: None,
        }
    }

    fn command_lifecycle(
        phase: ConversationCommandSessionLifecyclePhase,
        status: AgentCommandSessionStatus,
        created_at: i64,
    ) -> ConversationCommandSessionLifecycle {
        ConversationCommandSessionLifecycle {
            phase,
            session_id: "cmd_0123456789abcdef0123456789abcdef".to_string(),
            call_id: "command-call".to_string(),
            status,
            exit_code: (status == AgentCommandSessionStatus::Exited).then_some(0),
            latest_sequence: 7,
            output_truncated: false,
            archive: Default::default(),
            created_at,
        }
    }

    fn command_trace() -> ConversationTurnTrace {
        ConversationTurnTrace {
            schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
            run_id: "run-command".to_string(),
            conversation_id: "conversation-command".to_string(),
            assistant_message_id: "assistant-command".to_string(),
            terminal_status: ConversationTurnTraceTerminalStatus::Completed,
            terminal_error: None,
            truncated: false,
            items: vec![
                ConversationTurnTraceItem::ToolCall {
                    sequence: 0,
                    call_id: "command-call".to_string(),
                    tool: "run_command".to_string(),
                    provenance: AgentToolIdentity::Builtin {
                        tool_name: "run_command".to_string(),
                    },
                    operation: json!({ "command": "long-running" }),
                    approval_status: AgentApprovalStatus::Approved,
                    truncated: false,
                },
                ConversationTurnTraceItem::CommandSessionLifecycle {
                    sequence: 1,
                    phase: ConversationCommandSessionLifecyclePhase::Started,
                    session_id: "cmd_0123456789abcdef0123456789abcdef".to_string(),
                    call_id: "command-call".to_string(),
                    status: AgentCommandSessionStatus::Running,
                    exit_code: None,
                    latest_sequence: 0,
                    output_truncated: false,
                    archive: Default::default(),
                    created_at: 1_000,
                },
                ConversationTurnTraceItem::ToolResult {
                    sequence: 2,
                    call_id: "command-call".to_string(),
                    tool: "run_command".to_string(),
                    status: ConversationTraceToolResultStatus::Succeeded,
                    success: true,
                    observation: json!({
                        "status": "running",
                        "sessionId": "cmd_0123456789abcdef0123456789abcdef"
                    }),
                    approval_status: AgentApprovalStatus::Approved,
                    error: None,
                    truncated: false,
                    archive: Default::default(),
                },
            ],
        }
    }

    fn wait_call() -> AgentToolCall {
        AgentToolCall {
            id: "wait-call".to_string(),
            tool: "wait_agent".to_string(),
            args: json!({ "targets": ["agent-child"], "timeout_ms": 30_000 }),
            approval_status: AgentApprovalStatus::NotRequired,
            reason: None,
        }
    }

    fn wait_result(payload: &str) -> AgentToolResult {
        AgentToolResult {
            exact_archive_file: None,
            call_id: "wait-call".to_string(),
            tool: "wait_agent".to_string(),
            ok: true,
            result: Some(json!({
                "receiptId": "receipt-wait",
                "sourceReceiptId": null,
                "targets": [{
                    "targetAgentId": "agent-child",
                    "messages": [{
                        "messageId": "message-child-result",
                        "senderAgentId": "agent-child",
                        "senderTaskName": "research",
                        "senderTaskPath": "/root/research",
                        "kind": "result",
                        "content": payload,
                        "mailboxSequence": 1,
                        "createdAt": 1
                    }],
                    "targetStatusVersion": 1,
                    "latestWakeSequence": 1,
                    "latestWakeStatusRevision": 1,
                    "latestWakeStatus": "completed",
                    "displayStatus": "idle"
                }]
            })),
            error: None,
        }
    }

    fn assert_precommitted_wait_matches_runtime_terminal(payload: &str, truncated: bool) {
        let call = wait_call();
        let result = wait_result(payload);

        // The live Runtime retains the unbounded checkpoint item until terminal projection.
        let mut runtime = ConversationTraceRecorder::default();
        runtime.record_tool_call(&call).unwrap();
        runtime.record_tool_result(&call, &result);
        let terminal = runtime.finish(
            "run-wait",
            "conversation-wait",
            "assistant-wait",
            ConversationTurnTraceTerminalStatus::Completed,
            None,
        );

        // wait_agent commits the same ToolResult from the durable unresolved call before it
        // returns control to Runtime. This is the prefix which terminal settlement must preserve.
        let mut unresolved = ConversationTraceRecorder::default();
        unresolved.record_tool_call(&call).unwrap();
        let durable_unresolved = unresolved.snapshot().in_progress_audit_trace(
            "run-wait",
            "conversation-wait",
            "assistant-wait",
        );
        let precommitted =
            conversation_trace_with_recovered_tool_result(&durable_unresolved, &result).unwrap();

        assert_eq!(precommitted.items, terminal.items);
        assert_eq!(precommitted.truncated, terminal.truncated);
        assert_eq!(precommitted.truncated, truncated);
        assert!(matches!(
            precommitted.items.last(),
            Some(ConversationTurnTraceItem::ToolResult {
                archive: ConversationHistoryArchiveTraceMetadata {
                    history_projection_truncated,
                    ..
                },
                ..
            }) if *history_projection_truncated == truncated
        ));
    }

    #[test]
    fn precommitted_wait_uses_the_same_archive_metadata_as_runtime_terminal_projection() {
        assert_precommitted_wait_matches_runtime_terminal("small child result", false);
        assert_precommitted_wait_matches_runtime_terminal(&"x".repeat(16_000), true);
        assert_precommitted_wait_matches_runtime_terminal(
            "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAAB",
            true,
        );
    }

    #[test]
    fn command_session_lifecycle_is_a_body_free_audit_item() {
        let mut trace = command_trace();
        trace
            .append_command_session_lifecycle(command_lifecycle(
                ConversationCommandSessionLifecyclePhase::Terminal,
                AgentCommandSessionStatus::Exited,
                2_000,
            ))
            .unwrap();
        trace.validate().unwrap();

        let serialized = serde_json::to_value(trace.items.last().unwrap()).unwrap();
        assert_eq!(serialized["type"], "command_session_lifecycle");
        assert_eq!(serialized["phase"], "terminal");
        assert_eq!(serialized["status"], "exited");
        assert!(serialized.get("output").is_none());
        assert!(serialized.get("observation").is_none());
    }

    #[test]
    fn appending_command_session_lifecycle_is_idempotent_and_conflict_safe() {
        let mut trace = command_trace();
        let terminal = command_lifecycle(
            ConversationCommandSessionLifecyclePhase::Terminal,
            AgentCommandSessionStatus::Exited,
            2_000,
        );
        let sequence = trace
            .append_command_session_lifecycle(terminal.clone())
            .unwrap();
        assert_eq!(
            trace.append_command_session_lifecycle(terminal).unwrap(),
            sequence
        );
        assert_eq!(trace.items.len(), 4);

        let conflict = command_lifecycle(
            ConversationCommandSessionLifecyclePhase::Terminal,
            AgentCommandSessionStatus::TimedOut,
            3_000,
        );
        assert!(trace.append_command_session_lifecycle(conflict).is_err());
        assert_eq!(trace.items.len(), 4);
    }

    #[test]
    fn context_compaction_and_runtime_error_are_append_only_non_model_trace_markers() {
        let mut recorder = ConversationTraceRecorder::default();
        assert_eq!(
            recorder.record_narration("Before compaction.").unwrap(),
            Some(0)
        );
        assert_eq!(
            recorder
                .record_context_compaction_started("compact-1")
                .unwrap(),
            1
        );
        assert_eq!(
            recorder
                .record_context_compaction_started("compact-1")
                .unwrap(),
            1
        );
        assert_eq!(
            recorder
                .record_context_compaction_finished(
                    "compact-1",
                    AgentContextCompactionEventOutcome::Applied,
                )
                .unwrap(),
            1
        );
        assert_eq!(
            recorder
                .record_context_compaction_finished(
                    "compact-1",
                    AgentContextCompactionEventOutcome::Applied,
                )
                .unwrap(),
            1
        );
        assert_eq!(
            recorder
                .record_runtime_error("iteration limit reached", false, Some("iteration_limit"))
                .unwrap(),
            3
        );

        let trace = recorder.finish(
            "run-markers",
            "conversation-markers",
            "assistant-markers",
            ConversationTurnTraceTerminalStatus::Failed,
            Some("iteration limit reached"),
        );
        trace.validate().unwrap();
        assert_eq!(
            trace
                .items
                .iter()
                .map(ConversationTurnTraceItem::sequence)
                .collect::<Vec<_>>(),
            vec![0, 1, 2, 3]
        );
        assert_eq!(trace.model_context_item_count(), 1);
        assert!(trace.items[1..].iter().all(|item| !item.is_model_visible()));
    }

    #[test]
    fn terminal_projection_closes_a_crashed_context_compaction_once() {
        let mut recorder = ConversationTraceRecorder::default();
        recorder
            .record_context_compaction_started("compact-crashed")
            .unwrap();
        let snapshot = recorder.snapshot();
        let projection = terminal_conversation_trace_from_snapshot(
            snapshot,
            "run-crashed-compaction",
            "conversation-crashed-compaction",
            "assistant-crashed-compaction",
            ConversationTurnTraceTerminalStatus::Failed,
            "application exited",
        )
        .unwrap();
        projection.trace.validate().unwrap();
        assert!(matches!(
            projection.trace.items.as_slice(),
            [
                ConversationTurnTraceItem::ContextCompactionLifecycle {
                    sequence: 0,
                    phase: ConversationContextCompactionLifecyclePhase::Started,
                    operation_id,
                    outcome: None,
                },
                ConversationTurnTraceItem::ContextCompactionLifecycle {
                    sequence: 1,
                    phase: ConversationContextCompactionLifecyclePhase::Finished,
                    operation_id: finished_operation_id,
                    outcome: Some(AgentContextCompactionEventOutcome::Failed),
                }
            ] if operation_id == "compact-crashed" && finished_operation_id == operation_id
        ));

        let mut dangling = projection.trace.clone();
        dangling.items.pop();
        assert!(dangling.validate().is_err());
    }

    #[test]
    fn mcp_tool_identity_round_trips_and_missing_or_extra_provenance_is_rejected() {
        let tool_name = "mcp__fixture__echo";
        let call = AgentToolCall {
            id: "mcp-call-1".to_string(),
            tool: tool_name.to_string(),
            args: json!({"text": "hello"}),
            approval_status: AgentApprovalStatus::NotRequired,
            reason: None,
        };
        let identity = AgentToolIdentity::Mcp {
            provenance: AgentMcpToolProvenance {
                server_id: "7f4a2d91-24ab-4d24-9eed-63daf26a6c15".to_string(),
                scope: AgentMcpServerScope::Project {
                    project_id: "project-fixture".to_string(),
                },
                raw_tool_name: "echo/raw".to_string(),
                model_tool_name: tool_name.to_string(),
                config_epoch: "66dcbb6b-92a3-4d4e-9591-f0707e4ca3e3".to_string(),
                registry_revision: 3,
                config_digest: "a".repeat(64),
                catalog_generation: 4,
                catalog_digest: "b".repeat(64),
                catalog_schema_digest: "d".repeat(64),
                schema_digest: "c".repeat(64),
                schema_normalizer_version: crate::MCP_INPUT_SCHEMA_NORMALIZER_VERSION,
            },
        };
        let mut recorder = ConversationTraceRecorder::default();
        recorder.record_tool_call_with_identity(&call, identity);
        recorder.record_tool_result(
            &call,
            &AgentToolResult {
                exact_archive_file: None,
                call_id: call.id.clone(),
                tool: call.tool.clone(),
                ok: true,
                result: Some(json!({"content": "done"})),
                error: None,
            },
        );
        let trace = recorder.finish(
            "run-mcp-identity",
            "conversation-mcp-identity",
            "assistant-mcp-identity",
            ConversationTurnTraceTerminalStatus::Completed,
            None,
        );
        trace.validate().unwrap();
        let serialized = serde_json::to_value(&trace).unwrap();
        assert_eq!(
            serialized["items"][0]["provenance"]["provenance"]["catalogDigest"],
            "b".repeat(64)
        );
        assert_eq!(
            serialized["items"][0]["provenance"]["provenance"]["scope"]["projectId"],
            "project-fixture"
        );

        let mut legacy = serialized["items"][0].clone();
        legacy
            .as_object_mut()
            .expect("serialized trace item object")
            .remove("provenance");
        assert!(serde_json::from_value::<ConversationTurnTraceItem>(legacy).is_err());

        let mut extra = serialized["items"][0].clone();
        extra
            .as_object_mut()
            .expect("serialized trace item object")
            .insert("unexpected".to_string(), Value::Bool(true));
        assert!(serde_json::from_value::<ConversationTurnTraceItem>(extra).is_err());

        let mut extra_mcp_provenance = serialized["items"][0].clone();
        extra_mcp_provenance["provenance"]["provenance"]
            .as_object_mut()
            .unwrap()
            .insert("approvalPolicy".to_string(), json!("forged"));
        assert!(serde_json::from_value::<ConversationTurnTraceItem>(extra_mcp_provenance).is_err());

        let mut extra_mcp_scope = serialized["items"][0].clone();
        extra_mcp_scope["provenance"]["provenance"]["scope"]
            .as_object_mut()
            .unwrap()
            .insert("workspaceId".to_string(), json!("forged"));
        assert!(serde_json::from_value::<ConversationTurnTraceItem>(extra_mcp_scope).is_err());

        let mut mismatched = trace.clone();
        if let ConversationTurnTraceItem::ToolCall { tool, .. } = &mut mismatched.items[0] {
            *tool = "mcp__different__echo".to_string();
        }
        assert!(mismatched.validate().is_err());
    }

    #[test]
    fn unregistered_identity_is_a_strict_rejection_only_audit_record() {
        let call = AgentToolCall {
            id: "unknown-call".to_string(),
            tool: "hallucinated_tool".to_string(),
            args: json!({}),
            approval_status: AgentApprovalStatus::NotRequired,
            reason: None,
        };
        let mut recorder = ConversationTraceRecorder::default();
        recorder.record_tool_call_with_identity(
            &call,
            AgentToolIdentity::Unregistered {
                tool_name: call.tool.clone(),
            },
        );
        recorder.record_tool_result(
            &call,
            &AgentToolResult {
                exact_archive_file: None,
                call_id: call.id.clone(),
                tool: call.tool.clone(),
                ok: false,
                result: Some(json!({ "executed": false })),
                error: Some("unknown tool".to_string()),
            },
        );
        let rejected = recorder.finish(
            "run-unregistered",
            "conversation-unregistered",
            "assistant-unregistered",
            ConversationTurnTraceTerminalStatus::Completed,
            None,
        );
        rejected.validate().unwrap();

        let mut impossible_success = rejected;
        let ConversationTurnTraceItem::ToolResult {
            status,
            success,
            error,
            ..
        } = &mut impossible_success.items[1]
        else {
            panic!("expected tool result");
        };
        *status = ConversationTraceToolResultStatus::Succeeded;
        *success = true;
        *error = None;
        assert!(impossible_success.validate().is_err());

        assert!(serde_json::from_value::<AgentToolIdentity>(json!({
            "type": "unregistered",
            "toolName": "hallucinated_tool",
            "capabilities": []
        }))
        .is_err());
    }

    #[test]
    fn current_model_context_item_requires_total_fields_and_rejects_extra_keys() {
        let current = json!({
            "sequence": 0,
            "ordinal": 0,
            "role": "assistant",
            "content": "done",
            "isError": false
        });
        serde_json::from_value::<ConversationModelContextItem>(current.clone()).unwrap();

        for missing in ["ordinal", "isError"] {
            let mut malformed = current.clone();
            malformed
                .as_object_mut()
                .expect("model context object")
                .remove(missing);
            assert!(serde_json::from_value::<ConversationModelContextItem>(malformed).is_err());
        }

        let mut extra = current;
        extra
            .as_object_mut()
            .expect("model context object")
            .insert("legacyIndex".to_string(), json!(1));
        assert!(serde_json::from_value::<ConversationModelContextItem>(extra).is_err());
    }

    #[test]
    fn model_context_allows_duplicate_raw_provider_ids_but_rejects_duplicate_runtime_ids() {
        let call = |index: u32, runtime_id: &str| AgentContextCheckpointToolCall {
            id: runtime_id.to_string(),
            name: "read_file".to_string(),
            args: json!({ "path": format!("file-{index}.txt") }),
            provider_identity: AgentProviderToolCallIdentity {
                provider_tool_index: index,
                provider_call_id: "provider-reused-id".to_string(),
                runtime_call_id: runtime_id.to_string(),
            },
        };
        let mut item = ConversationModelContextItem {
            sequence: 0,
            ordinal: 0,
            role: "assistant".to_string(),
            content: String::new(),
            tool_call_id: None,
            tool_calls: vec![call(0, "runtime-0"), call(1, "runtime-1")],
            is_error: false,
        };

        item.validate().unwrap();

        item.tool_calls[1].id = "runtime-0".to_string();
        item.tool_calls[1].provider_identity.runtime_call_id = "runtime-0".to_string();
        assert!(item.validate().is_err());
    }

    #[test]
    fn flattened_archive_metadata_rejects_unknown_fields() {
        let mut item = serde_json::to_value(ConversationTurnTraceItem::ToolResult {
            sequence: 1,
            call_id: "call-1".to_string(),
            tool: "read_file".to_string(),
            status: ConversationTraceToolResultStatus::Succeeded,
            success: true,
            observation: json!({ "content": "done" }),
            approval_status: AgentApprovalStatus::NotRequired,
            error: None,
            truncated: false,
            archive: ConversationHistoryArchiveTraceMetadata::default(),
        })
        .unwrap();
        item.as_object_mut()
            .expect("trace item object")
            .insert("archiveFutureField".to_string(), json!(true));

        assert!(serde_json::from_value::<ConversationTurnTraceItem>(item).is_err());
    }

    #[test]
    fn completed_trace_rejects_a_missing_model_context_suffix() {
        let mut recorder = ConversationTraceRecorder::default();
        recorder
            .record_narration("I will inspect the file.")
            .unwrap();
        let call = AgentToolCall {
            id: "read-current".to_string(),
            tool: "read_file".to_string(),
            args: json!({ "path": "README.md" }),
            approval_status: AgentApprovalStatus::NotRequired,
            reason: None,
        };
        let call_sequence = recorder.record_tool_call(&call).unwrap();
        recorder
            .record_model_message(
                call_sequence,
                0,
                &LlmMessage::assistant(
                    "",
                    vec![crate::llm::LlmToolCall {
                        id: call.id.clone(),
                        name: call.tool.clone(),
                        args: call.args.clone(),
                    }],
                ),
            )
            .unwrap();
        let result = AgentToolResult {
            exact_archive_file: None,
            call_id: call.id.clone(),
            tool: call.tool.clone(),
            ok: true,
            result: Some(json!({ "content": "current" })),
            error: None,
        };
        let result_sequence = recorder.record_tool_result(&call, &result).unwrap();
        recorder
            .record_model_message(
                result_sequence,
                0,
                &LlmMessage::tool_result(call.id.clone(), "current", false),
            )
            .unwrap();
        let snapshot = recorder.snapshot();
        let trace = recorder.finish(
            "run-current",
            "conversation-current",
            "assistant-current",
            ConversationTurnTraceTerminalStatus::Completed,
            None,
        );

        trace
            .validate_complete_model_context(&snapshot.model_context_items)
            .unwrap();
        let narration_only = &snapshot.model_context_items[..1];
        assert!(trace
            .validate_complete_model_context(narration_only)
            .unwrap_err()
            .contains("every closed trace item"));
    }

    #[test]
    fn model_observation_is_compact_and_does_not_expose_backend_history_metadata() {
        let result = AgentToolResult {
            exact_archive_file: None,
            call_id: "call-1".to_string(),
            tool: "read_file".to_string(),
            ok: true,
            result: Some(json!({ "content": "bounded body" })),
            error: None,
        };
        let history_ref = crate::ContextHistoryRef::trace_item("assistant-1", 7);
        let rendered = render_tool_observation_with_history_ref(&result, Some(&history_ref));

        assert_eq!(rendered, "{\"content\":\"bounded body\"}");
        assert!(!rendered.contains("historyRef"));
        assert!(!rendered.contains("assistantMessageId"));
        assert!(!rendered.contains("```"));
    }

    #[test]
    fn recorder_keeps_runtime_checkpoint_but_bounds_web_body_in_durable_trace() {
        let mut recorder = ConversationTraceRecorder::default();
        let first = call("call-1");
        recorder.record_narration("I will fetch the page.").unwrap();
        recorder.record_tool_call(&first);
        recorder.record_tool_result(
            &first,
            &AgentToolResult {
                exact_archive_file: None,
                call_id: first.id.clone(),
                tool: first.tool.clone(),
                ok: true,
                result: Some(json!({ "content": "x".repeat(20_000) })),
                error: None,
            },
        );
        let second = call("call-2");
        recorder.record_tool_call(&second);
        recorder.record_tool_result(
            &second,
            &AgentToolResult {
                exact_archive_file: None,
                call_id: second.id.clone(),
                tool: second.tool.clone(),
                ok: false,
                result: None,
                error: Some("same failure".to_string()),
            },
        );

        let (checkpoint_items, _, _, _) = recorder.checkpoint();
        let ConversationTurnTraceItem::ToolResult {
            observation: checkpoint_observation,
            ..
        } = &checkpoint_items[2]
        else {
            panic!("expected checkpoint tool result");
        };
        assert_eq!(
            checkpoint_observation["content"].as_str().unwrap().len(),
            20_000
        );

        let trace = recorder.finish(
            "run-1",
            "conversation-1",
            "assistant-1",
            ConversationTurnTraceTerminalStatus::Completed,
            None,
        );
        trace.validate().unwrap();
        assert_eq!(trace.items.len(), 5);
        let ConversationTurnTraceItem::ToolResult { observation, .. } = &trace.items[2] else {
            panic!("expected tool result");
        };
        assert!(observation.get("content").is_none());
        assert!(observation["summary"].as_str().unwrap().chars().count() < 20_000);
        assert_eq!(observation["bodyTruncatedInHistory"], true);
        assert!(trace.truncated);
    }

    #[test]
    fn recorder_omits_command_session_output_from_durable_trace() {
        const SECRET_OUTPUT: &str = "command-session-secret-marker\nsecond line";
        const OUTPUT_HASH: &str =
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

        let call = AgentToolCall {
            id: "command-session-wait".to_string(),
            tool: "command_session".to_string(),
            args: json!({
                "sessionId": "cmd_0123456789abcdef0123456789abcdef",
                "action": "wait",
            }),
            approval_status: AgentApprovalStatus::NotRequired,
            reason: None,
        };
        let result = AgentToolResult {
            exact_archive_file: None,
            call_id: call.id.clone(),
            tool: call.tool.clone(),
            ok: true,
            result: Some(json!({
                "sessionId": "cmd_0123456789abcdef0123456789abcdef",
                "status": "running",
                "output": SECRET_OUTPUT,
                "exitCode": null,
                "latestSequence": 17,
                "outputTruncated": false,
                "read": {
                    "requestedAfterSequence": 12,
                    "firstSequence": 13,
                    "throughSequence": 17,
                    "truncatedBefore": false,
                    "outputBytes": SECRET_OUTPUT.len(),
                    "outputHash": OUTPUT_HASH,
                    "hostPrivateReadField": "must not survive",
                },
                "hostPrivateField": "must not survive",
            })),
            error: None,
        };

        let mut recorder = ConversationTraceRecorder::default();
        recorder.record_tool_call(&call);
        recorder.record_tool_result(&call, &result);

        let (checkpoint_items, _, _, _) = recorder.checkpoint();
        let ConversationTurnTraceItem::ToolResult {
            observation: checkpoint_observation,
            ..
        } = &checkpoint_items[1]
        else {
            panic!("expected checkpoint command_session result");
        };
        assert_eq!(checkpoint_observation["output"], SECRET_OUTPUT);

        let trace = recorder.finish(
            "run-command-session",
            "conversation-command-session",
            "assistant-command-session",
            ConversationTurnTraceTerminalStatus::Completed,
            None,
        );
        trace.validate().unwrap();
        let ConversationTurnTraceItem::ToolResult {
            observation,
            truncated,
            ..
        } = &trace.items[1]
        else {
            panic!("expected durable command_session result");
        };

        assert!(*truncated);
        assert!(observation.get("output").is_none());
        assert_eq!(observation["sessionId"], call.args["sessionId"]);
        assert_eq!(observation["status"], "running");
        assert_eq!(observation["exitCode"], Value::Null);
        assert_eq!(observation["latestSequence"], 17);
        assert_eq!(observation["outputTruncated"], false);
        assert_eq!(observation["read"]["requestedAfterSequence"], 12);
        assert_eq!(observation["read"]["firstSequence"], 13);
        assert_eq!(observation["read"]["throughSequence"], 17);
        assert_eq!(observation["read"]["truncatedBefore"], false);
        assert_eq!(observation["read"]["outputBytes"], SECRET_OUTPUT.len());
        assert_eq!(observation["read"]["outputHash"], OUTPUT_HASH);
        assert!(observation.get("hostPrivateField").is_none());
        assert!(observation["read"].get("hostPrivateReadField").is_none());
        assert!(trace.truncated);

        let serialized = serde_json::to_string(&trace).unwrap();
        assert!(!serialized.contains(SECRET_OUTPUT));
        assert!(!serialized.contains("must not survive"));
    }

    #[test]
    fn binary_fields_are_removed_but_neighboring_text_is_preserved() {
        let result = canonical_tool_result_for_context(&AgentToolResult {
            exact_archive_file: None,
            call_id: "call-1".to_string(),
            tool: "read_image".to_string(),
            ok: true,
            result: Some(json!({
                "path": "image.png",
                "image": { "mimeType": "image/png", "dataBase64": "SGVsbG8=" },
                "note": "keep me",
            })),
            error: None,
        });
        assert_eq!(
            result.result.as_ref().unwrap()["image"]["dataBase64"],
            "[binary/base64 omitted]"
        );
        assert_eq!(result.result.as_ref().unwrap()["note"], "keep me");
    }

    #[test]
    fn binary_sanitization_is_idempotent_and_the_durable_trace_validates() {
        let call = AgentToolCall {
            id: "call-image".to_string(),
            tool: "read_image".to_string(),
            args: json!({ "path": "image.png" }),
            approval_status: AgentApprovalStatus::NotRequired,
            reason: None,
        };
        let raw = AgentToolResult {
            exact_archive_file: None,
            call_id: call.id.clone(),
            tool: call.tool.clone(),
            ok: true,
            result: Some(json!({
                "path": "image.png",
                "thumbnailDataUrl": "data:image/png;base64,dGh1bWI=",
                "image": {
                    "mimeType": "image/png",
                    "dataBase64": "ZnVsbC1pbWFnZQ=="
                }
            })),
            error: None,
        };
        let once = canonical_tool_result_for_context(&raw);
        let twice = canonical_tool_result_for_context(&once);

        assert_eq!(once.result, twice.result);
        let mut recorder = ConversationTraceRecorder::default();
        recorder.record_tool_call(&call);
        recorder.record_tool_result(&call, &once);
        let trace = recorder.finish(
            "run-image",
            "conversation-image",
            "assistant-image",
            ConversationTurnTraceTerminalStatus::Completed,
            None,
        );
        trace.validate().unwrap();
        assert!(trace.truncated);
        let serialized = serde_json::to_string(&trace).unwrap();
        assert!(!serialized.contains("data:image/png;base64"));
        assert!(!serialized.contains("ZnVsbC1pbWFnZQ=="));
        assert!(!serialized.contains("dGh1bWI="));
    }

    #[test]
    fn failed_tool_observation_preserves_sanitized_structured_result_for_model() {
        let result = canonical_tool_result_for_context(&AgentToolResult {
            exact_archive_file: None,
            call_id: "call-command".to_string(),
            tool: "run_command".to_string(),
            ok: false,
            result: Some(json!({
                "exitCode": 1,
                "stdout": "partial output\n",
                "stderr": "ModuleNotFoundError: No module named 'openpyxl'\n",
                "timedOut": false,
                "cancelled": false,
                "diagnosticBase64": "c2VjcmV0",
            })),
            error: Some("命令执行失败。".to_string()),
        });

        let observation = render_tool_observation(&result);

        assert!(!observation.contains("\"ok\""));
        assert!(!observation.contains("\"callId\""));
        assert!(observation.contains("\"exitCode\":1"));
        assert!(observation.contains("partial output\\n"));
        assert!(observation.contains("ModuleNotFoundError"));
        assert!(observation.contains("\"timedOut\":false"));
        assert!(observation.contains("\"cancelled\":false"));
        assert!(observation.contains("[binary/base64 omitted]"));
        assert!(!observation.contains("c2VjcmV0"));
        assert!(observation.contains("命令执行失败。"));
    }

    #[test]
    fn failed_tool_observation_without_structured_result_is_still_well_formed() {
        let observation = render_tool_observation(&AgentToolResult {
            exact_archive_file: None,
            call_id: "call-failed-before-execution".to_string(),
            tool: "run_command".to_string(),
            ok: false,
            result: None,
            error: Some("命令未执行。".to_string()),
        });

        let observation: Value = serde_json::from_str(&observation).unwrap();
        assert_eq!(observation["status"], "failed");
        assert_eq!(observation["error"], "命令未执行。");
    }

    #[test]
    fn committed_prefix_never_ends_on_a_tool_call() {
        let mut recorder = ConversationTraceRecorder::default();
        recorder.record_narration("Before approval.").unwrap();
        recorder.record_tool_call(&call("pending"));
        let committed = recorder.snapshot().committed_prefix();
        assert_eq!(committed.items.len(), 1);
        assert!(committed.items[0].is_safe_compaction_boundary());
    }

    #[test]
    fn in_progress_audit_keeps_open_call_while_context_prefix_does_not() {
        let mut recorder = ConversationTraceRecorder::default();
        recorder
            .record_narration("I will generate the image.")
            .unwrap();
        let mut pending = call("image-call");
        pending.tool = "image_generation".to_string();
        pending.args = json!({
            "request": { "operation": "generate", "prompt": "private prompt" },
            "reason": "Create the requested image."
        });
        recorder.record_tool_call(&pending);
        let snapshot = recorder.snapshot();

        let audit = snapshot.in_progress_audit_trace("run", "conversation", "assistant");
        let context = snapshot.in_progress_trace("run", "conversation", "assistant");
        audit.validate().unwrap();
        context.validate().unwrap();
        assert!(matches!(
            audit.items.last(),
            Some(ConversationTurnTraceItem::ToolCall { call_id, .. }) if call_id == "image-call"
        ));
        assert!(matches!(
            context.items.last(),
            Some(ConversationTurnTraceItem::AssistantNarration { .. })
        ));
        let mut terminal = audit.clone();
        terminal.terminal_status = ConversationTurnTraceTerminalStatus::Failed;
        terminal.terminal_error = Some("interrupted".to_string());
        assert!(terminal.validate().is_err());
    }

    #[test]
    fn recovered_result_closes_the_same_durable_call_once() {
        let mut recorder = ConversationTraceRecorder::default();
        let mut pending = call("image-call");
        pending.tool = "image_generation".to_string();
        pending.args = json!({
            "request": { "operation": "generate", "prompt": "private prompt" },
            "reason": "Create the requested image."
        });
        recorder.record_tool_call(&pending);
        let audit = recorder
            .snapshot()
            .in_progress_audit_trace("run", "conversation", "assistant");
        let result = AgentToolResult {
            exact_archive_file: None,
            call_id: pending.id.clone(),
            tool: pending.tool.clone(),
            ok: false,
            result: Some(json!({ "status": "outcomeIndeterminate" })),
            error: Some("execution interrupted".to_string()),
        };

        let recovered = conversation_trace_with_recovered_tool_result(&audit, &result).unwrap();
        recovered.validate().unwrap();
        assert!(matches!(
            recovered.items.last(),
            Some(ConversationTurnTraceItem::ToolResult { call_id, .. }) if call_id == "image-call"
        ));
        assert!(conversation_trace_with_recovered_tool_result(&recovered, &result).is_err());
    }

    #[test]
    fn user_guidance_is_canonical_ordered_and_never_persists_attachment_bytes() {
        let mut recorder = ConversationTraceRecorder::default();
        recorder.record_narration("Initial answer.").unwrap();
        let sequence = recorder
            .record_user_guidance(
                "guidance-1",
                "client-1",
                "Please also inspect the image.",
                &[AgentInputAttachment {
                    id: "attachment-1".to_string(),
                    kind: AgentInputAttachmentKind::Image,
                    name: "diagram.png".to_string(),
                    mime_type: Some("image/png".to_string()),
                    size_bytes: 6,
                    encoding: crate::protocol::AgentInputAttachmentEncoding::Base64,
                    data: "c2VjcmV0".to_string(),
                    truncated: None,
                }],
                42,
            )
            .unwrap();
        assert_eq!(sequence, 1);

        let trace = recorder.finish(
            "run-1",
            "conversation-1",
            "assistant-1",
            ConversationTurnTraceTerminalStatus::Completed,
            None,
        );
        trace.validate().unwrap();
        let serialized = serde_json::to_string(&trace).unwrap();
        assert!(serialized.contains("\"type\":\"user_guidance\""));
        assert!(serialized.contains("diagram.png"));
        assert!(!serialized.contains("c2VjcmV0"));
    }

    #[test]
    fn current_trace_attachment_rejects_unknown_fields() {
        assert!(
            serde_json::from_value::<ConversationTraceAttachment>(json!({
                "id": "attachment-1",
                "kind": "image",
                "name": "diagram.png",
                "sizeBytes": 6,
                "futureField": true
            }))
            .is_err()
        );
    }

    #[test]
    fn user_guidance_cannot_split_an_open_tool_exchange() {
        let mut recorder = ConversationTraceRecorder::default();
        recorder.record_tool_call(&call("pending"));
        assert!(recorder
            .record_user_guidance("guidance-1", "client-1", "change course", &[], 42)
            .is_none());
    }

    #[test]
    fn write_and_patch_durable_projection_keeps_effect_metadata_without_payloads() {
        let write_call = AgentToolCall {
            id: "write-1".into(),
            tool: "write_file".into(),
            args: json!({
                "phase": "append",
                "draftId": "draft-1",
                "index": 0,
                "content": "first\nsecond\nSECRET_WRITE_BODY"
            }),
            approval_status: AgentApprovalStatus::NotRequired,
            reason: None,
        };
        let patch_call = AgentToolCall {
            id: "patch-1".into(),
            tool: "apply_patch".into(),
            args: json!({
                "operation": "update",
                "filePath": "src/lib.rs",
                "patch": "--- a/src/lib.rs\n+++ b/src/lib.rs\n-old\n+new\n+extra\n",
                "summary": "Update the implementation"
            }),
            approval_status: AgentApprovalStatus::Approved,
            reason: None,
        };
        let mut recorder = ConversationTraceRecorder::default();
        recorder.record_tool_call(&write_call);
        recorder.record_tool_result(
            &write_call,
            &AgentToolResult {
                exact_archive_file: None,
                call_id: write_call.id.clone(),
                tool: write_call.tool.clone(),
                ok: true,
                result: Some(json!({
                    "draft": {
                        "status": "drafting",
                        "draftId": "draft-1",
                        "mode": "create",
                        "filePath": "notes.txt",
                        "additions": 2,
                        "deletions": 0,
                        "lineCount": 2,
                        "byteCount": 30
                    },
                    "tail": "SECRET_WRITE_BODY"
                })),
                error: None,
            },
        );
        recorder.record_tool_call(&patch_call);
        recorder.record_tool_result(
            &patch_call,
            &AgentToolResult {
                exact_archive_file: None,
                call_id: patch_call.id.clone(),
                tool: patch_call.tool.clone(),
                ok: true,
                result: Some(json!({
                    "status": "applied",
                    "operation": "update",
                    "filePath": "src/lib.rs",
                    "appliedFilePaths": ["src/lib.rs"],
                    "gitDiff": {
                        "patch": "--- a/src/lib.rs\n+++ b/src/lib.rs\n-old\n+new\n+extra\n",
                        "truncated": false
                    }
                })),
                error: None,
            },
        );

        let trace = recorder.finish(
            "run",
            "conversation",
            "assistant",
            ConversationTurnTraceTerminalStatus::Completed,
            None,
        );
        trace.validate().unwrap();
        let serialized = serde_json::to_string(&trace).unwrap();
        assert!(!serialized.contains("SECRET_WRITE_BODY"));
        assert!(!serialized.contains("--- a/src/lib.rs"));

        let ConversationTurnTraceItem::ToolCall {
            operation: write_operation,
            ..
        } = &trace.items[0]
        else {
            panic!("expected write call");
        };
        assert_eq!(write_operation["contentBytes"], 30);
        assert_eq!(write_operation["contentLines"], 3);
        let ConversationTurnTraceItem::ToolResult {
            observation: write_result,
            ..
        } = &trace.items[1]
        else {
            panic!("expected write result");
        };
        assert_eq!(write_result["filePath"], "notes.txt");
        assert_eq!(write_result["additions"], 2);

        let ConversationTurnTraceItem::ToolCall {
            operation: patch_operation,
            ..
        } = &trace.items[2]
        else {
            panic!("expected patch call");
        };
        assert_eq!(patch_operation["filePath"], "src/lib.rs");
        assert_eq!(patch_operation["additions"], 2);
        assert_eq!(patch_operation["deletions"], 1);
        let ConversationTurnTraceItem::ToolResult {
            observation: patch_result,
            approval_status,
            ..
        } = &trace.items[3]
        else {
            panic!("expected patch result");
        };
        assert_eq!(*approval_status, AgentApprovalStatus::Approved);
        assert_eq!(patch_result["status"], "applied");
        assert_eq!(patch_result["additions"], 2);
        assert_eq!(patch_result["deletions"], 1);
        assert_eq!(patch_result["patchOmittedFromHistory"], true);
    }

    #[test]
    fn command_and_read_results_keep_bounded_tail_or_summary() {
        let command_call = AgentToolCall {
            id: "command-1".into(),
            tool: "run_command".into(),
            args: json!({
                "command": "cargo test",
                "cwd": "workspace",
                "reason": "verify"
            }),
            approval_status: AgentApprovalStatus::Approved,
            reason: None,
        };
        let read_call = AgentToolCall {
            id: "read-1".into(),
            tool: "read_file".into(),
            args: json!({ "path": "src/lib.rs", "startLine": 20, "maxLines": 50 }),
            approval_status: AgentApprovalStatus::NotRequired,
            reason: None,
        };
        let mut recorder = ConversationTraceRecorder::default();
        recorder.record_tool_call(&command_call);
        recorder.record_tool_result(
            &command_call,
            &AgentToolResult {
                exact_archive_file: None,
                call_id: command_call.id.clone(),
                tool: command_call.tool.clone(),
                ok: false,
                result: Some(json!({
                    "command": "cargo test",
                    "cwd": "workspace",
                    "exitCode": 1,
                    "stdout": format!("BEGIN_MUST_NOT_SURVIVE{}", "x".repeat(4_000)),
                    "stderr": "the actionable failure is at the end",
                    "timedOut": false,
                    "cancelled": false,
                    "durationMs": 25,
                    "stdoutTruncated": false,
                    "stderrTruncated": false
                })),
                error: Some("command failed".into()),
            },
        );
        recorder.record_tool_call(&read_call);
        recorder.record_tool_result(
            &read_call,
            &AgentToolResult {
                exact_archive_file: None,
                call_id: read_call.id.clone(),
                tool: read_call.tool.clone(),
                ok: true,
                result: Some(json!({
                    "path": "src/lib.rs",
                    "startLine": 20,
                    "endLine": 200,
                    "totalLines": 500,
                    "truncated": true,
                    "content": format!("READ_PREFIX{}", "z".repeat(5_000))
                })),
                error: None,
            },
        );
        let trace = recorder.finish(
            "run",
            "conversation",
            "assistant",
            ConversationTurnTraceTerminalStatus::Completed,
            None,
        );
        let serialized = serde_json::to_string(&trace).unwrap();
        assert!(!serialized.contains("BEGIN_MUST_NOT_SURVIVE"));
        assert!(!serialized.contains(&"z".repeat(2_000)));

        let ConversationTurnTraceItem::ToolResult {
            observation: command,
            ..
        } = &trace.items[1]
        else {
            panic!("expected command result");
        };
        assert_eq!(command["command"], "cargo test");
        assert_eq!(command["cwd"], "workspace");
        assert_eq!(command["exitCode"], 1);
        assert!(command["stdoutTail"]
            .as_str()
            .unwrap()
            .starts_with("[earlier output omitted]"));
        assert!(command["stderrTail"]
            .as_str()
            .unwrap()
            .contains("actionable failure"));

        let ConversationTurnTraceItem::ToolCall {
            operation: read_operation,
            ..
        } = &trace.items[2]
        else {
            panic!("expected read call");
        };
        assert_eq!(read_operation["path"], "src/lib.rs");
        assert_eq!(read_operation["startLine"], 20);
        let ConversationTurnTraceItem::ToolResult {
            observation: read, ..
        } = &trace.items[3]
        else {
            panic!("expected read result");
        };
        assert_eq!(read["path"], "src/lib.rs");
        assert_eq!(read["startLine"], 20);
        assert_eq!(read["contentTruncatedInHistory"], true);
        assert!(read["summary"].as_str().unwrap().contains("READ_PREFIX"));
    }

    #[test]
    fn durable_projection_redacts_embedded_data_urls_and_hidden_reasoning() {
        let call = AgentToolCall {
            id: "unknown-1".into(),
            tool: "extension_tool".into(),
            args: json!({
                "note": "prefix data:image/png;base64,U0VDUkVUX0lNQUdF suffix",
                "nested": { "arbitraryBase64": "U0VDUkVU", "safe": "keep" }
            }),
            approval_status: AgentApprovalStatus::NotRequired,
            reason: None,
        };
        let mut recorder = ConversationTraceRecorder::default();
        recorder.record_tool_call(&call);
        recorder.record_tool_result(
            &call,
            &AgentToolResult {
                exact_archive_file: None,
                call_id: call.id.clone(),
                tool: call.tool.clone(),
                ok: true,
                result: Some(json!({
                    "message": "before data:image/jpeg;base64,QUJDRA== after",
                    "reasoning": "hidden chain must not persist",
                    "safe": "visible evidence"
                })),
                error: None,
            },
        );
        let trace = recorder.finish(
            "run",
            "conversation",
            "assistant",
            ConversationTurnTraceTerminalStatus::Completed,
            None,
        );
        let serialized = serde_json::to_string(&trace).unwrap();
        assert!(!serialized.contains("U0VDUkVUX0lNQUdF"));
        assert!(!serialized.contains("U0VDUkVU"));
        assert!(!serialized.contains("QUJDRA=="));
        assert!(!serialized.contains("hidden chain must not persist"));
        assert!(serialized.contains("visible evidence"));
        trace.validate().unwrap();
    }

    #[test]
    fn repeated_unchanged_read_failure_keeps_paired_reference_instead_of_duplicate_error() {
        let mut recorder = ConversationTraceRecorder::default();
        for id in ["read-1", "read-2"] {
            let call = AgentToolCall {
                id: id.into(),
                tool: "read_file".into(),
                args: json!({ "path": "missing.txt" }),
                approval_status: AgentApprovalStatus::NotRequired,
                reason: None,
            };
            recorder.record_tool_call(&call);
            recorder.record_tool_result(
                &call,
                &AgentToolResult {
                    exact_archive_file: None,
                    call_id: call.id.clone(),
                    tool: call.tool.clone(),
                    ok: false,
                    result: Some(json!({ "path": "missing.txt", "code": "notFound" })),
                    error: Some("file does not exist".into()),
                },
            );
        }
        let trace = recorder.finish(
            "run",
            "conversation",
            "assistant",
            ConversationTurnTraceTerminalStatus::Failed,
            Some("unable to read the requested file"),
        );
        trace.validate().unwrap();
        assert_eq!(trace.items.len(), 4);
        let ConversationTurnTraceItem::ToolResult {
            observation,
            error,
            truncated,
            ..
        } = &trace.items[3]
        else {
            panic!("expected repeated result");
        };
        assert_eq!(observation["repeatedFailure"], true);
        assert_eq!(observation["sameAsCallId"], "read-1");
        assert!(error.is_none());
        assert!(*truncated);
    }

    #[test]
    fn approval_checkpoint_keeps_full_patch_while_durable_snapshot_is_bounded() {
        let call = AgentToolCall {
            id: "patch-approval".into(),
            tool: "apply_patch".into(),
            args: json!({
                "operation": "update",
                "filePath": "src/main.rs",
                "patch": "--- a/src/main.rs\n+++ b/src/main.rs\n-old\n+new\n"
            }),
            approval_status: AgentApprovalStatus::Required,
            reason: None,
        };
        let mut recorder = ConversationTraceRecorder::default();
        recorder.record_tool_call(&call);
        let (checkpoint, _, _, _) = recorder.checkpoint();
        let ConversationTurnTraceItem::ToolCall {
            operation: checkpoint_operation,
            ..
        } = &checkpoint[0]
        else {
            panic!("expected checkpoint call");
        };
        assert!(checkpoint_operation["patch"]
            .as_str()
            .unwrap()
            .contains("-old"));

        let durable =
            recorder
                .snapshot()
                .in_progress_audit_trace("run", "conversation", "assistant");
        let ConversationTurnTraceItem::ToolCall {
            operation: durable_operation,
            ..
        } = &durable.items[0]
        else {
            panic!("expected durable call");
        };
        assert!(durable_operation.get("patch").is_none());
        assert_eq!(durable_operation["filePath"], "src/main.rs");
        assert_eq!(durable_operation["additions"], 1);
        assert_eq!(durable_operation["deletions"], 1);
    }

    #[test]
    fn collaboration_receipt_can_cover_multiple_fifo_messages_with_exact_model_envelopes() {
        let mut recorder = ConversationTraceRecorder::default();
        let first = recorder
            .record_agent_mailbox_delivery(
                0,
                "receipt-1",
                "message-1",
                "agent-parent",
                "Parent",
                "/root",
                crate::AgentMailboxKind::Followup,
                "  first payload  ",
                1,
            )
            .unwrap()
            .unwrap();
        let second = recorder
            .record_agent_mailbox_delivery(
                1,
                "receipt-1",
                "message-2",
                "agent-child",
                "Child",
                "/root/child",
                crate::AgentMailboxKind::Result,
                "second\u{0007}payload",
                2,
            )
            .unwrap()
            .unwrap();
        let snapshot = recorder.snapshot();
        assert_eq!(snapshot.items.len(), 2);
        assert_eq!(snapshot.model_context_items.len(), 2);
        assert_eq!(snapshot.model_context_items[0].content, first);
        assert_eq!(snapshot.model_context_items[1].content, second);
        assert_eq!(
            serde_json::from_str::<Value>(&first).unwrap()["payload"],
            "first payload"
        );
        assert_eq!(
            serde_json::from_str::<Value>(&second).unwrap()["origin"],
            "agent"
        );
    }

    #[test]
    fn collaboration_model_envelope_is_utf8_safe_deterministic_and_bounded() {
        let payload = "蒙".repeat(400_000);
        let first = project_agent_mailbox_model_envelope(
            "agent-parent",
            "Parent",
            "/root",
            crate::AgentMailboxKind::Followup,
            &payload,
        )
        .unwrap();
        let second = project_agent_mailbox_model_envelope(
            "agent-parent",
            "Parent",
            "/root",
            crate::AgentMailboxKind::Followup,
            &payload,
        )
        .unwrap();
        assert_eq!(first, second);
        assert!(first.1);
        assert!(first.0.len() <= AGENT_MAILBOX_MODEL_ENVELOPE_MAX_BYTES);
        let envelope: Value = serde_json::from_str(&first.0).unwrap();
        assert_eq!(envelope["payloadTruncated"], true);
        assert!(envelope["payload"]
            .as_str()
            .unwrap()
            .ends_with("...[agent mailbox payload truncated]"));
    }

    #[test]
    fn large_collaboration_delivery_precommit_is_an_exact_terminal_trace_prefix() {
        let mut recorder = ConversationTraceRecorder::default();
        let model_content = recorder
            .record_agent_mailbox_delivery(
                0,
                "receipt-large",
                "message-large",
                "agent-child",
                "Child",
                "/root/child",
                crate::AgentMailboxKind::Result,
                &"evidence ".repeat(3_000),
                1,
            )
            .unwrap()
            .unwrap();
        let precommitted =
            recorder
                .snapshot()
                .in_progress_trace("run", "conversation", "assistant");
        let terminal = recorder.finish(
            "run",
            "conversation",
            "assistant",
            ConversationTurnTraceTerminalStatus::Completed,
            None,
        );

        let ConversationTurnTraceItem::AgentMailboxDelivery {
            content: trace_content,
            truncated,
            ..
        } = &precommitted.items[0]
        else {
            panic!("expected Agent mailbox delivery");
        };
        assert!(model_content.len() > trace_content.len());
        assert!(*truncated);
        assert_eq!(
            serde_json::from_str::<Value>(trace_content).unwrap()["payloadTruncated"],
            true
        );
        assert_eq!(
            recorder.snapshot().model_context_items[0].content,
            model_content
        );
        assert_eq!(precommitted.items, terminal.items);
        assert!(precommitted.truncated);
    }
}
