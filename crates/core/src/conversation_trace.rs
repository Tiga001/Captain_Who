//! Append-only, provider-neutral records of agent activity that survives into model context.
//!
//! The trace is the canonical record for both the active tool loop and later conversation turns.
//! It stores the same tool arguments and textual result payload that the model receives. Only
//! binary material that cannot safely live in text context is removed. Presentation events may
//! derive a smaller view, but they are never used to rebuild model context.

use crate::protocol::{
    AgentApprovalStatus, AgentInputAttachment, AgentInputAttachmentKind, AgentProposedAction,
    AgentRunCheckpoint, AgentToolCall, AgentToolResult,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeSet;

pub const CONVERSATION_TURN_TRACE_SCHEMA_VERSION: u32 = 3;
const BINARY_OMITTED_MARKER: &str = "[binary/base64 omitted]";
const BINARY_OMITTED_FROM_HISTORY_KEY: &str = "binaryomittedfromhistory";

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
    },
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
    /// True only when binary material was removed from otherwise canonical context data.
    pub truncated: bool,
    pub items: Vec<ConversationTurnTraceItem>,
}

#[derive(Debug, Clone, Default)]
pub struct ConversationTraceSnapshot {
    pub items: Vec<ConversationTurnTraceItem>,
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
        Self {
            items,
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
pub(crate) fn terminalize_interrupted_conversation_trace(
    trace: ConversationTurnTrace,
    terminal_status: ConversationTurnTraceTerminalStatus,
    reason: &str,
) -> ConversationTurnTrace {
    debug_assert_eq!(
        trace.terminal_status,
        ConversationTurnTraceTerminalStatus::InProgress
    );
    ConversationTraceRecorder::from_checkpoint(trace.items, 0, trace.truncated).finish(
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
    let mut recorder = ConversationTraceRecorder::from_checkpoint(
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
    let mut recorder = ConversationTraceRecorder::from_checkpoint(
        checkpoint.conversation_trace_items.clone(),
        checkpoint.next_conversation_trace_sequence,
        checkpoint.conversation_trace_truncated,
    );
    recorder.record_tool_call(call);
    recorder.record_tool_result(call, result);
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
    let mut recorder = ConversationTraceRecorder::from_checkpoint(
        checkpoint.conversation_trace_items.clone(),
        checkpoint.next_conversation_trace_sequence,
        checkpoint.conversation_trace_truncated,
    );
    recorder.record_tool_call(call);
    recorder.record_tool_result(call, result);
    recorder.snapshot()
}

pub fn cancelled_conversation_trace_from_snapshot(
    snapshot: ConversationTraceSnapshot,
    run_id: &str,
    conversation_id: &str,
    assistant_message_id: &str,
    reason: &str,
) -> ConversationTurnTrace {
    ConversationTraceRecorder::from_checkpoint(
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
    let (terminal_error, truncated) = sanitize_optional_text(terminal_error);
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
    next_sequence: u64,
    truncated: bool,
}

impl ConversationTraceRecorder {
    pub(crate) fn from_checkpoint(
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
            next_sequence: next_sequence.max(inferred_next),
            truncated,
        }
    }

    pub(crate) fn checkpoint(&self) -> (Vec<ConversationTurnTraceItem>, u64, bool) {
        (self.items.clone(), self.next_sequence, self.truncated)
    }

    pub(crate) fn snapshot(&self) -> ConversationTraceSnapshot {
        ConversationTraceSnapshot {
            items: self.items.clone(),
            next_sequence: self.next_sequence,
            truncated: self.truncated,
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
                content,
                truncated: redacted,
            });
        self.truncated |= redacted;
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

    pub(crate) fn record_tool_call(&mut self, call: &AgentToolCall) {
        if self.items.iter().any(|item| {
            matches!(item, ConversationTurnTraceItem::ToolCall { call_id, .. } if call_id == &call.id)
        }) {
            return;
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

    pub(crate) fn record_tool_result(&mut self, call: &AgentToolCall, result: &AgentToolResult) {
        let Some(call_index) = self.items.iter().rposition(
            |item| matches!(item, ConversationTurnTraceItem::ToolCall { call_id, .. } if call_id == &call.id),
        ) else {
            return;
        };
        if self.items.iter().any(|item| {
            matches!(item, ConversationTurnTraceItem::ToolResult { call_id, .. } if call_id == &call.id)
        }) {
            return;
        }
        if let ConversationTurnTraceItem::ToolCall {
            approval_status, ..
        } = &mut self.items[call_index]
        {
            *approval_status = call.approval_status;
        }

        let sequence = self.take_sequence();
        let item = projected_tool_result_trace_item(sequence, call, result);
        self.truncated |= matches!(
            &item,
            ConversationTurnTraceItem::ToolResult {
                truncated: true,
                ..
            }
        );
        self.items.push(item);
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
        let (terminal_error, terminal_redacted) = sanitize_optional_text(terminal_error);
        ConversationTurnTrace {
            schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
            run_id: run_id.to_string(),
            conversation_id: conversation_id.to_string(),
            assistant_message_id: assistant_message_id.to_string(),
            terminal_status,
            terminal_error,
            truncated: recorder.truncated || terminal_redacted,
            items: recorder.items,
        }
    }

    fn close_unresolved(
        &mut self,
        terminal_status: ConversationTurnTraceTerminalStatus,
        terminal_error: Option<&str>,
    ) {
        let Some((call_id, tool, approval_status)) = self.items.iter().rev().find_map(|item| {
            if let ConversationTurnTraceItem::ToolCall {
                call_id,
                tool,
                approval_status,
                ..
            } = item
            {
                let has_result = self.items.iter().any(|candidate| {
                    matches!(candidate, ConversationTurnTraceItem::ToolResult { call_id: result_id, .. } if result_id == call_id)
                });
                (!has_result).then(|| (call_id.clone(), tool.clone(), *approval_status))
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
        let (error, redacted) = sanitize_text(terminal_error.unwrap_or(fallback));
        let sequence = self.take_sequence();
        self.items.push(ConversationTurnTraceItem::ToolResult {
            sequence,
            call_id,
            tool,
            status,
            success: false,
            observation: json!({
                "resultAvailable": false,
                "terminalStatus": terminal_status,
            }),
            approval_status,
            error: Some(error),
            truncated: redacted,
        });
        self.truncated |= redacted;
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
    let (observation, result_redacted) =
        sanitize_value(result.result.as_ref().unwrap_or(&Value::Null));
    let (error, error_redacted) = sanitize_optional_text(result.error.as_deref());
    let redacted = result_redacted || error_redacted;
    ConversationTurnTraceItem::ToolResult {
        sequence,
        call_id: call.id.clone(),
        tool: call.tool.clone(),
        status,
        success,
        observation,
        approval_status: call.approval_status,
        error,
        truncated: redacted,
    }
}

pub(crate) fn canonical_tool_result_for_context(result: &AgentToolResult) -> AgentToolResult {
    let mut canonical = result.clone();
    if let Some(value) = canonical.result.as_mut() {
        *value = sanitize_value(value).0;
    }
    if let Some(error) = canonical.error.as_mut() {
        *error = sanitize_text(error).0;
    }
    canonical
}

pub(crate) fn render_tool_observation(result: &AgentToolResult) -> String {
    let payload = if result.ok {
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
    if value.trim_start().to_ascii_lowercase().starts_with("data:")
        && value.to_ascii_lowercase().contains(";base64,")
    {
        return (BINARY_OMITTED_MARKER.to_string(), true);
    }
    (value.to_string(), false)
}

fn sanitize_value(value: &Value) -> (Value, bool) {
    match value {
        Value::Object(object) => {
            let mut redacted = false;
            let mut output = serde_json::Map::with_capacity(object.len());
            for (key, value) in object {
                let canonical_key = key
                    .chars()
                    .filter(|character| character.is_ascii_alphanumeric())
                    .flat_map(char::to_lowercase)
                    .collect::<String>();
                if canonical_key == BINARY_OMITTED_FROM_HISTORY_KEY && value.as_bool() == Some(true)
                {
                    output.insert(key.clone(), value.clone());
                    redacted = true;
                } else if canonical_key.contains("base64") || canonical_key.contains("dataurl") {
                    output.insert(key.clone(), json!(BINARY_OMITTED_MARKER));
                    redacted = true;
                } else {
                    let (value, item_redacted) = sanitize_value(value);
                    output.insert(key.clone(), value);
                    redacted |= item_redacted;
                }
            }
            (Value::Object(output), redacted)
        }
        Value::Array(items) => {
            let mut redacted = false;
            let items = items
                .iter()
                .map(|item| {
                    let (item, item_redacted) = sanitize_value(item);
                    redacted |= item_redacted;
                    item
                })
                .collect();
            (Value::Array(items), redacted)
        }
        Value::String(value) => {
            let (value, redacted) = sanitize_text(value);
            (Value::String(value), redacted)
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => (value.clone(), false),
    }
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
    fn recorder_preserves_full_textual_tool_context_without_semantic_deduplication() {
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
        assert_eq!(observation["content"].as_str().unwrap().len(), 20_000);
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
}
