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
    sanitize_runtime_value,
};
use crate::llm::LlmMessage;
use crate::protocol::{
    AgentApprovalStatus, AgentContextCheckpointToolCall, AgentInputAttachment,
    AgentInputAttachmentKind, AgentProposedAction, AgentRunCheckpoint, AgentToolCall,
    AgentToolResult,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

pub const CONVERSATION_TURN_TRACE_SCHEMA_VERSION: u32 = 3;

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

/// One provider-neutral message from the uncompressed model-visible conversation timeline.
///
/// This is deliberately separate from [`ConversationTurnTraceItem`]. The durable trace is a
/// bounded audit/search projection, while this record preserves the exact text projection that
/// was supplied to the model until context compaction covers it. Raw tool output and binary
/// delivery data never enter this record; those belong to Exact History Archive and transient
/// provider messages respectively.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationModelContextItem {
    pub sequence: u64,
    #[serde(default)]
    pub ordinal: u32,
    pub role: String,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<AgentContextCheckpointToolCall>,
    #[serde(default)]
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
                for call in &self.tool_calls {
                    if call.id.trim().is_empty() || !is_provider_safe_tool_name(&call.name) {
                        return Err("model context tool call identity is invalid".to_string());
                    }
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

#[derive(Debug, Deserialize, Serialize, Clone, Default, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationModelContextLog {
    pub assistant_message_id: String,
    #[serde(default)]
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
    if !boundary.is_safe_compaction_boundary() {
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
        ConversationTurnTraceItem::UserGuidance { .. } => {
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
    rename_all_fields = "camelCase"
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
    ToolCall {
        sequence: u64,
        call_id: String,
        tool: String,
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
}

/// Exact-history metadata is flattened into a tool-result trace item so older readers can ignore
/// it while current readers can jump directly from the bounded durable record to the lossless
/// archive. The archive itself never enters normal model context.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ConversationHistoryArchiveTraceMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archive_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived_completely: Option<bool>,
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
#[serde(rename_all = "camelCase")]
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
            | Self::ToolCall { sequence, .. }
            | Self::ToolResult { sequence, .. } => *sequence,
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Self::AssistantNarration { .. } => "assistant_narration",
            Self::UserGuidance { .. } => "user_guidance",
            Self::ToolCall { .. } => "tool_call",
            Self::ToolResult { .. } => "tool_result",
        }
    }

    pub fn is_safe_compaction_boundary(&self) -> bool {
        !matches!(self, Self::ToolCall { .. })
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
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
        let mut pending_call: Option<(&str, &str)> = None;
        let mut call_ids = BTreeSet::new();
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
                ConversationTurnTraceItem::ToolCall {
                    call_id,
                    tool,
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
                    if !call_ids.insert(call_id.as_str()) {
                        return Err(format!(
                            "conversation trace contains duplicate tool call id: {call_id}"
                        ));
                    }
                    ensure_no_binary_value("tool operation", operation)?;
                    pending_call = Some((call_id, tool));
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
                    let Some((expected_id, expected_tool)) = pending_call.take() else {
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
                    ensure_no_binary_value("tool observation", observation)?;
                    if let Some(error) = error {
                        ensure_no_binary_text("tool error", error)?;
                    }
                    archive.validate()?;
                }
            }
        }
        if pending_call.is_some()
            && self.terminal_status != ConversationTurnTraceTerminalStatus::InProgress
        {
            return Err("conversation trace ends with an unresolved tool call".to_string());
        }
        Ok(())
    }

    /// Number of items that form complete exchanges and may be rendered into model context.
    #[must_use]
    pub fn model_context_item_count(&self) -> usize {
        if self.terminal_status == ConversationTurnTraceTerminalStatus::InProgress
            && matches!(
                self.items.last(),
                Some(ConversationTurnTraceItem::ToolCall { .. })
            )
        {
            self.items.len().saturating_sub(1)
        } else {
            self.items.len()
        }
    }
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

/// Closes any process-interrupted ToolCall before an in-progress trace becomes terminal.
pub fn terminalize_interrupted_conversation_trace(
    trace: ConversationTurnTrace,
    terminal_status: ConversationTurnTraceTerminalStatus,
    reason: &str,
) -> ConversationTurnTrace {
    debug_assert_eq!(
        trace.terminal_status,
        ConversationTurnTraceTerminalStatus::InProgress
    );
    ConversationTraceRecorder::from_durable_trace(trace.items, 0, trace.truncated).finish(
        &trace.run_id,
        &trace.conversation_id,
        &trace.assistant_message_id,
        terminal_status,
        Some(reason),
    )
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

pub fn cancelled_conversation_trace_from_checkpoint(
    checkpoint: &AgentRunCheckpoint,
    conversation_id: &str,
    assistant_message_id: &str,
    call: &AgentToolCall,
    result: &AgentToolResult,
    reason: &str,
) -> ConversationTurnTrace {
    let mut recorder = ConversationTraceRecorder::from_checkpoint_with_model_context(
        checkpoint.conversation_trace_items.clone(),
        checkpoint.conversation_model_context_items.clone(),
        checkpoint.next_conversation_trace_sequence,
        checkpoint.conversation_trace_truncated,
    );
    record_model_tool_exchange(&mut recorder, call, result, None);
    recorder.finish(
        &checkpoint.run_id,
        conversation_id,
        assistant_message_id,
        ConversationTurnTraceTerminalStatus::Cancelled,
        Some(reason),
    )
}

pub fn conversation_trace_snapshot_from_checkpoint_and_continuation(
    checkpoint: &AgentRunCheckpoint,
    call: &AgentToolCall,
    result: &AgentToolResult,
) -> ConversationTraceSnapshot {
    conversation_trace_snapshot_from_checkpoint_and_continuation_with_history_ref(
        checkpoint, call, result, None,
    )
}

/// Builds an approval-continuation snapshot using the same backend-generated history reference
/// that runtime checkpoint restoration will place in the model-visible ToolResult.
///
/// Settlement and runtime resume must persist byte-identical model projections. Otherwise the
/// append-only model log would correctly reject the resumed request as a rewrite.
pub fn conversation_trace_snapshot_from_checkpoint_and_continuation_with_history_ref(
    checkpoint: &AgentRunCheckpoint,
    call: &AgentToolCall,
    result: &AgentToolResult,
    assistant_message_id: Option<&str>,
) -> ConversationTraceSnapshot {
    let mut recorder = ConversationTraceRecorder::from_checkpoint_with_model_context(
        checkpoint.conversation_trace_items.clone(),
        checkpoint.conversation_model_context_items.clone(),
        checkpoint.next_conversation_trace_sequence,
        checkpoint.conversation_trace_truncated,
    );
    let result_sequence = checkpoint
        .next_conversation_trace_sequence
        .saturating_add(u64::from(!checkpoint.conversation_trace_items.iter().any(
            |item| {
                matches!(
                    item,
                    ConversationTurnTraceItem::ToolCall { call_id, .. }
                        if call_id == &call.id
                )
            },
        )));
    let history_ref = assistant_message_id.map(|assistant_message_id| {
        crate::ContextHistoryRef::trace_item(assistant_message_id, result_sequence)
    });
    record_model_tool_exchange(&mut recorder, call, result, history_ref.as_ref());
    recorder.snapshot()
}

fn record_model_tool_exchange(
    recorder: &mut ConversationTraceRecorder,
    call: &AgentToolCall,
    result: &AgentToolResult,
    history_ref: Option<&crate::ContextHistoryRef>,
) {
    let llm_result = canonical_tool_result_for_context(result);
    if let Some(sequence) = recorder.record_tool_call(call) {
        recorder.record_model_message(
            sequence,
            0,
            &LlmMessage::assistant(
                "",
                vec![crate::llm::LlmToolCall {
                    id: call.id.clone(),
                    name: call.tool.clone(),
                    args: call.args.clone(),
                }],
            ),
        );
    }
    if let Some(sequence) = recorder.record_tool_result(call, &llm_result) {
        recorder.record_model_message(
            sequence,
            0,
            &LlmMessage::tool_result(
                call.id.clone(),
                render_tool_observation_with_history_ref(&llm_result, history_ref),
                !llm_result.ok,
            ),
        );
    }
}

pub fn cancelled_conversation_trace_from_snapshot(
    snapshot: ConversationTraceSnapshot,
    run_id: &str,
    conversation_id: &str,
    assistant_message_id: &str,
    reason: &str,
) -> ConversationTurnTrace {
    ConversationTraceRecorder::from_durable_trace(
        snapshot.items,
        snapshot.next_sequence,
        snapshot.truncated,
    )
    .finish(
        run_id,
        conversation_id,
        assistant_message_id,
        ConversationTurnTraceTerminalStatus::Cancelled,
        Some(reason),
    )
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
            items_are_durable: false,
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

    pub(crate) fn committed_item_count(&self) -> usize {
        self.snapshot().committed_prefix().items.len()
    }

    pub(crate) fn record_narration(&mut self, content: &str) {
        let content = content.trim();
        if content.is_empty() {
            return;
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
        );
        self.truncated |= redacted;
    }

    pub(crate) fn record_model_message(
        &mut self,
        sequence: u64,
        ordinal: u32,
        message: &LlmMessage,
    ) {
        if self
            .model_context_items
            .iter()
            .any(|item| item.sequence == sequence && item.ordinal == ordinal)
        {
            return;
        }
        let (content, redacted) = sanitize_runtime_text(&message.content);
        let mut tool_calls = Vec::with_capacity(message.tool_calls.len());
        let mut tool_call_redacted = false;
        for call in &message.tool_calls {
            let (args, redacted) = sanitize_runtime_value(&call.args);
            tool_call_redacted |= redacted;
            tool_calls.push(AgentContextCheckpointToolCall {
                id: call.id.clone(),
                name: call.name.clone(),
                args,
            });
        }
        let item = ConversationModelContextItem {
            sequence,
            ordinal,
            role: message.role.as_str().to_string(),
            content,
            tool_call_id: message.tool_call_id.clone(),
            tool_calls,
            is_error: message.is_error,
        };
        if item.validate().is_ok() {
            self.model_context_items.push(item);
            self.model_context_items
                .sort_by_key(|item| (item.sequence, item.ordinal));
        } else {
            self.truncated = true;
        }
        self.truncated |= redacted || tool_call_redacted || !message.images.is_empty();
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

    pub(crate) fn record_tool_call(&mut self, call: &AgentToolCall) -> Option<u64> {
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
            operation,
            approval_status: call.approval_status,
            truncated: redacted,
        });
        self.truncated |= redacted;
        Some(sequence)
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
            AgentProposedAction::ToolCall { call } => (&call.id, call.approval_status),
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
        if let ConversationTurnTraceItem::ToolCall {
            approval_status, ..
        } = &mut self.items[call_index]
        {
            *approval_status = call.approval_status;
        }

        let sequence = self.take_sequence();
        let mut item = if self.items_are_durable {
            projected_tool_result_trace_item(sequence, call, result)
        } else {
            checkpoint_tool_result_trace_item(sequence, call, result)
        };
        if let ConversationTurnTraceItem::ToolResult {
            archive: item_archive,
            ..
        } = &mut item
        {
            *item_archive = archive;
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
            ConversationTurnTraceItem::ToolCall {
                sequence,
                call_id,
                tool,
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
                archive.history_projection_truncated |= projection_truncated;
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
    ConversationTurnTraceItem::ToolResult {
        sequence,
        call_id: call.id.clone(),
        tool: call.tool.clone(),
        status,
        success,
        observation: observation.value,
        approval_status: call.approval_status,
        error,
        truncated: observation.truncated || error_truncated,
        archive: Default::default(),
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
    let mut payload = if result.ok {
        json!({
            "type": "tool_result",
            "tool": result.tool,
            "callId": result.call_id,
            "ok": true,
            "result": result.result,
        })
    } else {
        json!({
            "type": "tool_result",
            "tool": result.tool,
            "callId": result.call_id,
            "ok": false,
            "result": result.result,
            "error": result.error,
        })
    };
    if let (Some(history_ref), Some(object)) = (history_ref, payload.as_object_mut()) {
        object.insert(
            "historyRef".to_string(),
            serde_json::to_value(history_ref).unwrap_or(Value::Null),
        );
    }
    let payload = serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "{}".to_string());
    format!(
        "Tool result observation. Use this result to continue. Do not repeat the same tool call unless more information is needed.\n```json\n{payload}\n```"
    )
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

    fn call(id: &str) -> AgentToolCall {
        AgentToolCall {
            id: id.to_string(),
            tool: "web_fetch".to_string(),
            args: json!({ "url": "https://example.com" }),
            approval_status: AgentApprovalStatus::NotRequired,
            reason: None,
        }
    }

    #[test]
    fn model_observation_exposes_backend_generated_history_ref_outside_tool_result() {
        let result = AgentToolResult {
            call_id: "call-1".to_string(),
            tool: "read_file".to_string(),
            ok: true,
            result: Some(json!({ "content": "bounded body" })),
            error: None,
        };
        let history_ref = crate::ContextHistoryRef::trace_item("assistant-1", 7);
        let rendered = render_tool_observation_with_history_ref(&result, Some(&history_ref));

        assert!(rendered.contains("\"historyRef\""));
        assert!(rendered.contains("\"assistantMessageId\": \"assistant-1\""));
        assert!(rendered.contains("\"sequence\": 7"));
        assert!(rendered.contains("\"result\""));
    }

    #[test]
    fn recorder_keeps_runtime_checkpoint_but_bounds_web_body_in_durable_trace() {
        let mut recorder = ConversationTraceRecorder::default();
        let first = call("call-1");
        recorder.record_narration("I will fetch the page.");
        recorder.record_tool_call(&first);
        recorder.record_tool_result(
            &first,
            &AgentToolResult {
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
    fn binary_fields_are_removed_but_neighboring_text_is_preserved() {
        let result = canonical_tool_result_for_context(&AgentToolResult {
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

        assert!(observation.contains("\"ok\": false"));
        assert!(observation.contains("\"exitCode\": 1"));
        assert!(observation.contains("partial output\\n"));
        assert!(observation.contains("ModuleNotFoundError"));
        assert!(observation.contains("\"timedOut\": false"));
        assert!(observation.contains("\"cancelled\": false"));
        assert!(observation.contains("[binary/base64 omitted]"));
        assert!(!observation.contains("c2VjcmV0"));
        assert!(observation.contains("命令执行失败。"));
    }

    #[test]
    fn failed_tool_observation_without_structured_result_is_still_well_formed() {
        let observation = render_tool_observation(&AgentToolResult {
            call_id: "call-failed-before-execution".to_string(),
            tool: "run_command".to_string(),
            ok: false,
            result: None,
            error: Some("命令未执行。".to_string()),
        });

        assert!(observation.contains("\"ok\": false"));
        assert!(observation.contains("\"result\": null"));
        assert!(observation.contains("命令未执行。"));
    }

    #[test]
    fn committed_prefix_never_ends_on_a_tool_call() {
        let mut recorder = ConversationTraceRecorder::default();
        recorder.record_narration("Before approval.");
        recorder.record_tool_call(&call("pending"));
        let committed = recorder.snapshot().committed_prefix();
        assert_eq!(committed.items.len(), 1);
        assert!(committed.items[0].is_safe_compaction_boundary());
    }

    #[test]
    fn in_progress_audit_keeps_open_call_while_context_prefix_does_not() {
        let mut recorder = ConversationTraceRecorder::default();
        recorder.record_narration("I will generate the image.");
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
        recorder.record_narration("Initial answer.");
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
}
