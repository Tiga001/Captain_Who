//! Durable, provider-neutral records of user-visible agent activity.
//!
//! A conversation trace is deliberately smaller than the runtime observation sent back to the
//! model and independent from presentation events sent to the renderer. Projection happens once,
//! next to the tool execution, and never re-runs a tool.

use crate::protocol::{
    AgentApprovalStatus, AgentProposedAction, AgentRunCheckpoint, AgentToolCall, AgentToolResult,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};

pub const CONVERSATION_TURN_TRACE_SCHEMA_VERSION: u32 = 1;
const TRUNCATION_SUFFIX: &str = "\n...[durable trace truncated]";
const TAIL_PREFIX: &str = "...[durable trace kept tail]\n";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConversationTraceLimits {
    pub narration_chars: usize,
    pub terminal_error_chars: usize,
    pub operation_string_chars: usize,
    pub result_string_chars: usize,
    pub error_lines: usize,
    pub command_chars: usize,
    pub command_output_chars: usize,
    pub web_excerpt_chars: usize,
    pub search_result_count: usize,
    pub search_snippet_chars: usize,
    pub identifier_chars: usize,
    pub tool_name_chars: usize,
    pub todo_item_chars: usize,
    pub base64_token_chars: usize,
    pub wrapped_base64_line_chars: usize,
    pub max_todo_items: usize,
    pub max_nested_collection_items: usize,
    pub max_object_fields: usize,
    pub max_projection_depth: usize,
    pub max_projected_json_chars: usize,
    pub max_trace_json_chars: usize,
    pub max_tool_exchanges: usize,
    pub max_narrations: usize,
}

pub const CONVERSATION_TRACE_LIMITS: ConversationTraceLimits = ConversationTraceLimits {
    narration_chars: 4_000,
    terminal_error_chars: 2_000,
    operation_string_chars: 1_000,
    result_string_chars: 2_000,
    error_lines: 4,
    command_chars: 2_000,
    command_output_chars: 2_000,
    web_excerpt_chars: 1_600,
    search_result_count: 5,
    search_snippet_chars: 500,
    identifier_chars: 256,
    tool_name_chars: 64,
    todo_item_chars: 300,
    base64_token_chars: 128,
    wrapped_base64_line_chars: 48,
    max_todo_items: 32,
    max_nested_collection_items: 8,
    max_object_fields: 32,
    max_projection_depth: 4,
    max_projected_json_chars: 32_000,
    max_trace_json_chars: 2_000_000,
    max_tool_exchanges: 512,
    max_narrations: 512,
};

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

impl ConversationTurnTraceItem {
    pub fn sequence(&self) -> u64 {
        match self {
            Self::AssistantNarration { sequence, .. }
            | Self::ToolCall { sequence, .. }
            | Self::ToolResult { sequence, .. } => *sequence,
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Self::AssistantNarration { .. } => "assistant_narration",
            Self::ToolCall { .. } => "tool_call",
            Self::ToolResult { .. } => "tool_result",
        }
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
        for (label, identifier) in [
            ("run_id", self.run_id.as_str()),
            ("conversation_id", self.conversation_id.as_str()),
            ("assistant_message_id", self.assistant_message_id.as_str()),
        ] {
            validate_durable_identifier(label, identifier)?;
        }
        if let Some(error) = self.terminal_error.as_deref() {
            if self.terminal_status == ConversationTurnTraceTerminalStatus::InProgress {
                return Err(
                    "in-progress conversation trace cannot have a terminal error".to_string(),
                );
            }
            validate_durable_text(
                "terminal_error",
                error,
                CONVERSATION_TRACE_LIMITS.terminal_error_chars,
            )?;
        }

        let narration_count = self
            .items
            .iter()
            .filter(|item| matches!(item, ConversationTurnTraceItem::AssistantNarration { .. }))
            .count();
        let exchange_count = self
            .items
            .iter()
            .filter(|item| matches!(item, ConversationTurnTraceItem::ToolCall { .. }))
            .count();
        let max_items = CONVERSATION_TRACE_LIMITS.max_narrations.saturating_add(
            CONVERSATION_TRACE_LIMITS
                .max_tool_exchanges
                .saturating_mul(2),
        );
        if self.items.len() > max_items
            || narration_count > CONVERSATION_TRACE_LIMITS.max_narrations
            || exchange_count > CONVERSATION_TRACE_LIMITS.max_tool_exchanges
        {
            return Err("conversation trace exceeds the durable item-count limit".to_string());
        }
        let serialized = serde_json::to_string(self)
            .map_err(|error| format!("conversation trace is not serializable: {error}"))?;
        if serialized.chars().count() > CONVERSATION_TRACE_LIMITS.max_trace_json_chars {
            return Err("conversation trace exceeds the aggregate durable size limit".to_string());
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
                    validate_durable_text(
                        "assistant narration",
                        content,
                        CONVERSATION_TRACE_LIMITS.narration_chars,
                    )?;
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
                    if call_id.trim().is_empty() || tool.trim().is_empty() {
                        return Err(
                            "conversation trace tool call id/name cannot be empty".to_string()
                        );
                    }
                    if !call_ids.insert(call_id.as_str()) {
                        return Err(format!(
                            "conversation trace contains duplicate tool call id: {call_id}"
                        ));
                    }
                    validate_durable_identifier("tool call id", call_id)?;
                    validate_durable_tool_name(tool)?;
                    validate_durable_value("tool operation", operation)?;
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
                    validate_durable_value("tool observation", observation)?;
                    if let Some(error) = error.as_deref() {
                        validate_durable_text(
                            "tool error",
                            error,
                            CONVERSATION_TRACE_LIMITS.result_string_chars,
                        )?;
                    }
                }
            }
        }
        if pending_call.is_some() {
            return Err("conversation trace ends with an unresolved tool call".to_string());
        }
        Ok(())
    }
}

impl ConversationTraceSnapshot {
    /// Returns the append-only prefix that is safe to commit while a run is still active.
    /// A pending tool call remains in the runtime checkpoint but is not durable until its result
    /// closes the exchange.
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
    let trace = recorder.finish(
        &checkpoint.run_id,
        conversation_id,
        assistant_message_id,
        ConversationTurnTraceTerminalStatus::Cancelled,
        Some(reason),
    );
    debug_assert!(trace.validate().is_ok());
    trace
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
    let recorder = ConversationTraceRecorder::from_checkpoint(
        snapshot.items,
        snapshot.next_sequence,
        snapshot.truncated,
    );
    let trace = recorder.finish(
        run_id,
        conversation_id,
        assistant_message_id,
        ConversationTurnTraceTerminalStatus::Cancelled,
        Some(reason),
    );
    debug_assert!(trace.validate().is_ok());
    trace
}

pub fn failed_conversation_trace_without_items(
    run_id: &str,
    conversation_id: &str,
    assistant_message_id: &str,
    error: &str,
) -> ConversationTurnTrace {
    let (terminal_error, truncated) = durable_error_text(error);
    ConversationTurnTrace {
        schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
        run_id: run_id.to_string(),
        conversation_id: conversation_id.to_string(),
        assistant_message_id: assistant_message_id.to_string(),
        terminal_status: ConversationTurnTraceTerminalStatus::Failed,
        terminal_error: Some(terminal_error),
        truncated,
        items: Vec::new(),
    }
}

pub fn completed_conversation_trace_without_items(
    run_id: &str,
    conversation_id: &str,
    assistant_message_id: &str,
) -> ConversationTurnTrace {
    ConversationTurnTrace {
        schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
        run_id: run_id.to_string(),
        conversation_id: conversation_id.to_string(),
        assistant_message_id: assistant_message_id.to_string(),
        terminal_status: ConversationTurnTraceTerminalStatus::Completed,
        terminal_error: None,
        truncated: false,
        items: Vec::new(),
    }
}

pub fn cancelled_conversation_trace_without_items(
    run_id: &str,
    conversation_id: &str,
    assistant_message_id: &str,
    reason: &str,
) -> ConversationTurnTrace {
    let (terminal_error, truncated) = durable_error_text(reason);
    ConversationTurnTrace {
        schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
        run_id: run_id.to_string(),
        conversation_id: conversation_id.to_string(),
        assistant_message_id: assistant_message_id.to_string(),
        terminal_status: ConversationTurnTraceTerminalStatus::Cancelled,
        terminal_error: Some(terminal_error),
        truncated,
        items: Vec::new(),
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ConversationTraceRecorder {
    items: Vec<ConversationTurnTraceItem>,
    next_sequence: u64,
    truncated: bool,
    failed_signatures: BTreeSet<String>,
    last_todo_state: Option<Value>,
    original_call_ids: BTreeMap<String, String>,
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
        let mut recorder = Self {
            items,
            next_sequence: next_sequence.max(inferred_next),
            truncated,
            failed_signatures: BTreeSet::new(),
            last_todo_state: None,
            original_call_ids: BTreeMap::new(),
        };
        recorder.rebuild_deduplication_state();
        recorder
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

    pub(crate) fn record_narration(&mut self, content: &str) {
        if content.trim().is_empty() {
            return;
        }
        let narration_count = self
            .items
            .iter()
            .filter(|item| matches!(item, ConversationTurnTraceItem::AssistantNarration { .. }))
            .count();
        if narration_count >= CONVERSATION_TRACE_LIMITS.max_narrations {
            self.truncated = true;
            return;
        }
        let (content, truncated) =
            bounded_text(content.trim(), CONVERSATION_TRACE_LIMITS.narration_chars);
        let sequence = self.take_sequence();
        self.items
            .push(ConversationTurnTraceItem::AssistantNarration {
                sequence,
                content,
                truncated,
            });
        self.truncated |= truncated;
        if !trace_items_fit_aggregate_limit(&self.items) {
            self.items.pop();
            self.truncated = true;
        }
    }

    pub(crate) fn record_tool_call(&mut self, call: &AgentToolCall) {
        let durable_call_id = durable_call_id(&call.id);
        if self.items.iter().any(|item| {
            matches!(item, ConversationTurnTraceItem::ToolCall { call_id, .. } if call_id == &durable_call_id)
        }) {
            return;
        }
        let exchange_count = self
            .items
            .iter()
            .filter(|item| matches!(item, ConversationTurnTraceItem::ToolCall { .. }))
            .count();
        let non_side_effect_exchange_count = self
            .items
            .iter()
            .filter(|item| {
                matches!(item, ConversationTurnTraceItem::ToolCall { tool, .. } if !is_side_effecting_tool(tool))
            })
            .count();
        const SIDE_EFFECT_EXCHANGE_RESERVE: usize = 128;
        let non_side_effect_limit = CONVERSATION_TRACE_LIMITS
            .max_tool_exchanges
            .saturating_sub(SIDE_EFFECT_EXCHANGE_RESERVE);
        if exchange_count >= CONVERSATION_TRACE_LIMITS.max_tool_exchanges
            || (!is_side_effecting_tool(&call.tool)
                && non_side_effect_exchange_count >= non_side_effect_limit)
        {
            self.truncated = true;
            return;
        }
        let (operation, truncated) = project_tool_call(call);
        let sequence = self.take_sequence();
        self.items.push(ConversationTurnTraceItem::ToolCall {
            sequence,
            call_id: durable_call_id.clone(),
            tool: durable_tool_name(&call.tool),
            operation,
            approval_status: call.approval_status,
            truncated,
        });
        self.original_call_ids
            .insert(call.id.clone(), durable_call_id);
        self.truncated |= truncated;
    }

    pub(crate) fn enrich_tool_call(&mut self, action: &AgentProposedAction) {
        let (action_id, raw_operation, max_string_chars) = match action {
            AgentProposedAction::Diff { diff } => {
                let (additions, deletions) = unified_diff_counts(&diff.patch);
                (
                    diff.id.as_str(),
                    json!({
                        "operation": diff.operation,
                        "filePath": diff.file_path,
                        "summary": diff.summary,
                        "baseRevision": diff.base_revision,
                        "additions": additions,
                        "deletions": deletions
                    }),
                    CONVERSATION_TRACE_LIMITS.operation_string_chars,
                )
            }
            AgentProposedAction::FileWrite { file_write } => (
                file_write.id.as_str(),
                json!({
                    "phase": "finish",
                    "draftId": file_write.draft_id,
                    "mode": file_write.mode,
                    "filePath": file_write.file_path,
                    "summary": file_write.summary,
                    "baseRevision": file_write.base_revision,
                    "additions": file_write.additions,
                    "deletions": file_write.deletions,
                    "lineCount": file_write.line_count,
                    "byteCount": file_write.byte_count
                }),
                CONVERSATION_TRACE_LIMITS.operation_string_chars,
            ),
            AgentProposedAction::Command { command } => (
                command.id.as_str(),
                json!({
                    "command": command.command,
                    "cwd": command.cwd,
                    "timeoutMs": command.timeout_ms,
                    "riskLevel": command.risk_level,
                    "reason": command.reason
                }),
                CONVERSATION_TRACE_LIMITS.command_chars,
            ),
            AgentProposedAction::ToolCall { call } => {
                let (operation, operation_truncated) = project_tool_call(call);
                let action_id = self
                    .original_call_ids
                    .get(&call.id)
                    .cloned()
                    .unwrap_or_else(|| durable_call_id(&call.id));
                if let Some(ConversationTurnTraceItem::ToolCall {
                    operation: current,
                    approval_status,
                    truncated,
                    ..
                }) = self.items.iter_mut().rev().find(
                    |item| matches!(item, ConversationTurnTraceItem::ToolCall { call_id, .. } if call_id == &action_id),
                ) {
                    *current = operation;
                    *approval_status = call.approval_status;
                    *truncated |= operation_truncated;
                    self.truncated |= operation_truncated;
                }
                return;
            }
        };
        let mut operation_truncated = false;
        let operation = sanitize_value(
            &raw_operation,
            max_string_chars,
            0,
            &mut operation_truncated,
        );
        let operation = cap_projected_value(operation, &mut operation_truncated);
        let action_id = self
            .original_call_ids
            .get(action_id)
            .cloned()
            .unwrap_or_else(|| durable_call_id(action_id));
        if let Some(ConversationTurnTraceItem::ToolCall {
            operation: current,
            approval_status,
            truncated,
            ..
        }) = self.items.iter_mut().rev().find(
            |item| matches!(item, ConversationTurnTraceItem::ToolCall { call_id, .. } if call_id == &action_id),
        ) {
            *current = operation;
            *approval_status = match action {
                AgentProposedAction::ToolCall { call } => call.approval_status,
                AgentProposedAction::Diff { diff } => diff.approval_status,
                AgentProposedAction::FileWrite { file_write } => file_write.approval_status,
                AgentProposedAction::Command { command } => command.approval_status,
            };
            *truncated |= operation_truncated;
            self.truncated |= operation_truncated;
        }
    }

    pub(crate) fn record_tool_result(&mut self, call: &AgentToolCall, result: &AgentToolResult) {
        let durable_call_id = self
            .original_call_ids
            .get(&call.id)
            .cloned()
            .unwrap_or_else(|| durable_call_id(&call.id));
        let Some(call_index) = self.items.iter().rposition(
            |item| matches!(item, ConversationTurnTraceItem::ToolCall { call_id, .. } if call_id == &durable_call_id),
        ) else {
            return;
        };
        if self.items.iter().any(|item| {
            matches!(item, ConversationTurnTraceItem::ToolResult { call_id, .. } if call_id == &durable_call_id)
        }) {
            return;
        }

        if let ConversationTurnTraceItem::ToolCall {
            approval_status, ..
        } = &mut self.items[call_index]
        {
            *approval_status = call.approval_status;
        }
        let projection = project_tool_result(call, result);
        let operation = match &self.items[call_index] {
            ConversationTurnTraceItem::ToolCall { operation, .. } => operation.clone(),
            _ => Value::Null,
        };
        if call.tool == "todo_update" {
            let todo_state = semantic_todo_state(&projection.observation);
            if self
                .last_todo_state
                .as_ref()
                .is_some_and(|previous| previous == &todo_state)
            {
                self.items.remove(call_index);
                return;
            }
        }

        if !projection.success {
            let signature = failure_signature(&call.tool, &operation, &projection);
            if !self.failed_signatures.insert(signature) {
                self.items.remove(call_index);
                return;
            }
        }
        if call.tool == "todo_update" && projection.success {
            self.last_todo_state = Some(semantic_todo_state(&projection.observation));
        }

        let sequence = self.take_sequence();
        self.items.push(ConversationTurnTraceItem::ToolResult {
            sequence,
            call_id: durable_call_id,
            tool: durable_tool_name(&call.tool),
            status: projection.status,
            success: projection.success,
            observation: projection.observation,
            approval_status: call.approval_status,
            error: projection.error,
            truncated: projection.truncated,
        });
        self.truncated |= projection.truncated;
        if !trace_items_fit_aggregate_limit(&self.items) {
            self.items.pop();
            self.items.remove(call_index);
            self.truncated = true;
        }
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
        let (terminal_error, terminal_error_truncated) = terminal_error
            .map(durable_error_text)
            .map(|(error, truncated)| (Some(error), truncated))
            .unwrap_or((None, false));
        ConversationTurnTrace {
            schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
            run_id: run_id.to_string(),
            conversation_id: conversation_id.to_string(),
            assistant_message_id: assistant_message_id.to_string(),
            terminal_status,
            terminal_error,
            truncated: recorder.truncated || terminal_error_truncated,
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
        let status = match terminal_status {
            ConversationTurnTraceTerminalStatus::Cancelled => {
                ConversationTraceToolResultStatus::Cancelled
            }
            _ => ConversationTraceToolResultStatus::Failed,
        };
        let fallback = match terminal_status {
            ConversationTurnTraceTerminalStatus::Cancelled => {
                "Run cancelled before a verifiable tool result was available."
            }
            _ => "Run ended before a verifiable tool result was available.",
        };
        let (error, truncated) = durable_error_text(terminal_error.unwrap_or(fallback));
        let sequence = self.take_sequence();
        let call_index = self.items.iter().rposition(|item| {
            matches!(item, ConversationTurnTraceItem::ToolCall { call_id: candidate, .. } if candidate == &call_id)
        });
        self.items.push(ConversationTurnTraceItem::ToolResult {
            sequence,
            call_id,
            tool,
            status,
            success: false,
            observation: json!({
                "resultAvailable": false,
                "terminalStatus": terminal_status
            }),
            approval_status,
            error: Some(error),
            truncated,
        });
        self.truncated |= truncated;
        if !trace_items_fit_aggregate_limit(&self.items) {
            self.items.pop();
            if let Some(call_index) = call_index {
                self.items.remove(call_index);
            }
            self.truncated = true;
        }
    }

    fn rebuild_deduplication_state(&mut self) {
        for item in &self.items {
            if let ConversationTurnTraceItem::ToolResult {
                call_id,
                tool,
                status,
                success,
                observation,
                error,
                ..
            } = item
            {
                let projection = ToolResultProjection {
                    status: *status,
                    success: *success,
                    observation: observation.clone(),
                    error: error.clone(),
                    truncated: false,
                };
                if !success {
                    let operation = self.items.iter().find_map(|candidate| match candidate {
                        ConversationTurnTraceItem::ToolCall {
                            call_id: candidate_id,
                            operation,
                            ..
                        } if candidate_id == call_id => Some(operation),
                        _ => None,
                    });
                    self.failed_signatures.insert(failure_signature(
                        tool,
                        operation.unwrap_or(&Value::Null),
                        &projection,
                    ));
                }
                if tool == "todo_update" && *success {
                    self.last_todo_state = Some(semantic_todo_state(observation));
                }
            }
        }
        for item in &self.items {
            if let ConversationTurnTraceItem::ToolCall { call_id, .. } = item {
                self.original_call_ids
                    .entry(call_id.clone())
                    .or_insert_with(|| call_id.clone());
            }
        }
    }

    fn take_sequence(&mut self) -> u64 {
        let sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.saturating_add(1);
        sequence
    }
}

fn trace_items_fit_aggregate_limit(items: &[ConversationTurnTraceItem]) -> bool {
    const TRACE_HEADER_RESERVE_CHARS: usize = 16_384;
    serialized_chars(items)
        <= CONVERSATION_TRACE_LIMITS
            .max_trace_json_chars
            .saturating_sub(TRACE_HEADER_RESERVE_CHARS)
}

struct ToolResultProjection {
    status: ConversationTraceToolResultStatus,
    success: bool,
    observation: Value,
    error: Option<String>,
    truncated: bool,
}

fn project_tool_call(call: &AgentToolCall) -> (Value, bool) {
    let mut truncated = false;
    let args = &call.args;
    let projected = match call.tool.as_str() {
        "write_file" => {
            let mut value = pick_fields(
                args,
                &["phase", "draftId", "filePath", "mode", "index", "summary"],
                CONVERSATION_TRACE_LIMITS.operation_string_chars,
                &mut truncated,
            );
            if let Some(content) = args.get("content").and_then(Value::as_str) {
                insert(&mut value, "contentBytes", json!(content.len()));
                insert(&mut value, "contentLines", json!(content.lines().count()));
            }
            if let Some(edits) = args.get("edits").and_then(Value::as_array) {
                insert(&mut value, "editCount", json!(edits.len()));
            }
            value
        }
        "apply_patch" => {
            let mut value = pick_fields(
                args,
                &[
                    "operation",
                    "filePath",
                    "expectedRevision",
                    "baseRevision",
                    "summary",
                ],
                CONVERSATION_TRACE_LIMITS.operation_string_chars,
                &mut truncated,
            );
            if let Some(content) = args.get("content").and_then(Value::as_str) {
                insert(&mut value, "contentBytes", json!(content.len()));
                insert(&mut value, "contentLines", json!(content.lines().count()));
            }
            if let Some(edits) = args.get("edits").and_then(Value::as_array) {
                insert(&mut value, "editCount", json!(edits.len()));
            }
            if let Some(patch) = args.get("patch").and_then(Value::as_str) {
                let (additions, deletions) = unified_diff_counts(patch);
                insert(&mut value, "additions", json!(additions));
                insert(&mut value, "deletions", json!(deletions));
            }
            value
        }
        "run_command" => pick_fields(
            args,
            &["command", "cwd", "timeoutMs", "riskLevel", "reason"],
            CONVERSATION_TRACE_LIMITS.command_chars,
            &mut truncated,
        ),
        "web_search" => pick_fields(
            args,
            &[
                "query",
                "maxResults",
                "searchDepth",
                "topic",
                "timeRange",
                "includeDomains",
                "excludeDomains",
            ],
            CONVERSATION_TRACE_LIMITS.operation_string_chars,
            &mut truncated,
        ),
        "web_fetch" => pick_fields(
            args,
            &[
                "url",
                "query",
                "chunksPerSource",
                "extractDepth",
                "format",
                "maxChars",
            ],
            CONVERSATION_TRACE_LIMITS.operation_string_chars,
            &mut truncated,
        ),
        "todo_update" => project_todo_call(args, &mut truncated),
        _ => pick_scalar_fields(
            args,
            &[
                "path",
                "filePath",
                "startLine",
                "maxLines",
                "maxChars",
                "query",
                "limit",
                "caseSensitive",
                "focusPath",
                "maxDepth",
                "maxEntries",
                "kind",
                "scope",
            ],
            CONVERSATION_TRACE_LIMITS.operation_string_chars,
            &mut truncated,
        ),
    };
    let projected = cap_projected_value(projected, &mut truncated);
    (projected, truncated)
}

fn project_tool_result(call: &AgentToolCall, result: &AgentToolResult) -> ToolResultProjection {
    let mut truncated = false;
    let value = result.result.as_ref().unwrap_or(&Value::Null);
    let observation = match call.tool.as_str() {
        "write_file" => project_write_result(value, &mut truncated),
        "apply_patch" => project_patch_result(value, &mut truncated),
        "run_command" => project_command_result(value, &mut truncated),
        "web_search" => project_web_search_result(value, &mut truncated),
        "web_fetch" => project_web_fetch_result(value, &mut truncated),
        "read_file" => pick_fields(
            value,
            &[
                "path",
                "revision",
                "startLine",
                "endLine",
                "totalLines",
                "truncated",
            ],
            CONVERSATION_TRACE_LIMITS.result_string_chars,
            &mut truncated,
        ),
        "read_image" => pick_fields(
            value,
            &["path", "format", "mimeType", "sizeBytes", "width", "height"],
            CONVERSATION_TRACE_LIMITS.result_string_chars,
            &mut truncated,
        ),
        "read_pdf" => pick_fields(
            value,
            &["path", "format", "sizeBytes", "pageCount", "truncated"],
            CONVERSATION_TRACE_LIMITS.result_string_chars,
            &mut truncated,
        ),
        "read_word" => pick_fields(
            value,
            &[
                "path",
                "format",
                "sizeBytes",
                "extractor",
                "partCount",
                "truncated",
            ],
            CONVERSATION_TRACE_LIMITS.result_string_chars,
            &mut truncated,
        ),
        "read_presentation" => pick_fields(
            value,
            &[
                "path",
                "format",
                "sizeBytes",
                "extractor",
                "slideCount",
                "truncated",
            ],
            CONVERSATION_TRACE_LIMITS.result_string_chars,
            &mut truncated,
        ),
        "read_spreadsheet" => pick_fields(
            value,
            &[
                "path",
                "format",
                "sizeBytes",
                "extractor",
                "sheetCount",
                "truncated",
            ],
            CONVERSATION_TRACE_LIMITS.result_string_chars,
            &mut truncated,
        ),
        "search_files" | "search_code" => project_search_result(value, &mut truncated),
        "workspace_map" => project_workspace_map_result(value, &mut truncated),
        "git_diff" => project_git_diff_result(value, &mut truncated),
        "attachments_list" | "attachments_list_project" => {
            project_attachment_result(value, &mut truncated)
        }
        "todo_update" => project_todo_result(value, &mut truncated),
        _ => pick_scalar_fields(
            value,
            &[
                "status",
                "path",
                "filePath",
                "url",
                "query",
                "summary",
                "message",
                "count",
                "total",
                "truncated",
            ],
            CONVERSATION_TRACE_LIMITS.result_string_chars,
            &mut truncated,
        ),
    };
    let observation = cap_projected_value(observation, &mut truncated);
    let status = result_status(result.ok, value, result.error.as_deref());
    let error = result.error.as_deref().map(|error| {
        let (error, error_truncated) = durable_error_text(error);
        truncated |= error_truncated;
        error
    });
    ToolResultProjection {
        status,
        success: result.ok && status == ConversationTraceToolResultStatus::Succeeded,
        observation,
        error,
        truncated,
    }
}

fn project_write_result(value: &Value, truncated: &mut bool) -> Value {
    let source = value.get("draft").unwrap_or(value);
    let mut projected = pick_fields(
        source,
        &[
            "status",
            "draftId",
            "mode",
            "filePath",
            "baseRevision",
            "additions",
            "deletions",
            "lineCount",
            "byteCount",
            "chunkCount",
            "nextChunkIndex",
            "statsFinal",
            "summary",
            "revision",
            "message",
        ],
        CONVERSATION_TRACE_LIMITS.result_string_chars,
        truncated,
    );
    if projected.as_object().is_some_and(Map::is_empty) {
        projected = json!({ "status": "no_durable_details" });
    }
    projected
}

fn project_patch_result(value: &Value, truncated: &mut bool) -> Value {
    pick_fields(
        value,
        &[
            "status",
            "operation",
            "filePath",
            "appliedFilePaths",
            "message",
            "gitDiffError",
        ],
        CONVERSATION_TRACE_LIMITS.result_string_chars,
        truncated,
    )
}

fn project_command_result(value: &Value, truncated: &mut bool) -> Value {
    let mut projected = pick_fields(
        value,
        &[
            "command",
            "cwd",
            "exitCode",
            "timedOut",
            "cancelled",
            "durationMs",
            "stdoutTruncated",
            "stderrTruncated",
        ],
        CONVERSATION_TRACE_LIMITS.result_string_chars,
        truncated,
    );
    for (source, target) in [("stdout", "stdoutTail"), ("stderr", "stderrTail")] {
        if let Some(output) = value.get(source).and_then(Value::as_str) {
            let output_characters = output.chars().count();
            insert(
                &mut projected,
                &format!("{source}Characters"),
                json!(output_characters),
            );
            if output_characters > 64 {
                let tail_limit = CONVERSATION_TRACE_LIMITS
                    .command_output_chars
                    .min(output_characters / 2);
                let (tail, _) = bounded_tail_text(output, tail_limit);
                insert(&mut projected, target, json!(tail));
            } else if output_characters > 0 {
                insert(
                    &mut projected,
                    &format!("{source}DurableOmitted"),
                    json!(true),
                );
            }
            if output_characters > 0 {
                insert(&mut projected, "durableOutputTruncated", json!(true));
                *truncated = true;
            }
        }
    }
    projected
}

fn project_web_search_result(value: &Value, truncated: &mut bool) -> Value {
    let mut projected = pick_fields(
        value,
        &["query", "provider", "responseTime", "truncated"],
        CONVERSATION_TRACE_LIMITS.result_string_chars,
        truncated,
    );
    if let Some(answer) = value.get("answer").and_then(Value::as_str) {
        let (answer, was_truncated) =
            bounded_text(answer, CONVERSATION_TRACE_LIMITS.web_excerpt_chars);
        insert(&mut projected, "answer", json!(answer));
        *truncated |= was_truncated;
    }
    let results = value
        .get("results")
        .and_then(Value::as_array)
        .map(|items| {
            if items.len() > CONVERSATION_TRACE_LIMITS.search_result_count {
                *truncated = true;
            }
            items
                .iter()
                .take(CONVERSATION_TRACE_LIMITS.search_result_count)
                .map(|item| {
                    let mut source = pick_fields(
                        item,
                        &["title", "url", "publishedDate", "score"],
                        CONVERSATION_TRACE_LIMITS.search_snippet_chars,
                        truncated,
                    );
                    if let Some(content) = item.get("content").and_then(Value::as_str) {
                        let (snippet, was_truncated) =
                            bounded_text(content, CONVERSATION_TRACE_LIMITS.search_snippet_chars);
                        insert(&mut source, "snippet", json!(snippet));
                        *truncated |= was_truncated;
                    }
                    source
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    insert(&mut projected, "sources", json!(results));
    projected
}

fn project_web_fetch_result(value: &Value, truncated: &mut bool) -> Value {
    let mut projected = pick_fields(
        value,
        &[
            "url",
            "requestedUrl",
            "title",
            "provider",
            "format",
            "extractDepth",
            "responseTime",
            "truncated",
        ],
        CONVERSATION_TRACE_LIMITS.result_string_chars,
        truncated,
    );
    if let Some(content) = value.get("content").and_then(Value::as_str) {
        let content_characters = content.chars().count();
        insert(
            &mut projected,
            "contentCharacters",
            json!(content_characters),
        );
        if content_characters > 1 {
            let excerpt_limit = CONVERSATION_TRACE_LIMITS
                .web_excerpt_chars
                .min(content_characters / 2);
            let (excerpt, _) = bounded_text(content, excerpt_limit);
            insert(&mut projected, "observedExcerpt", json!(excerpt));
            insert(&mut projected, "durableExcerptTruncated", json!(true));
        } else {
            insert(&mut projected, "durableContentOmitted", json!(true));
        }
        *truncated = true;
    }
    projected
}

fn project_search_result(value: &Value, truncated: &mut bool) -> Value {
    let mut projected = pick_fields(
        value,
        &["query", "truncated"],
        CONVERSATION_TRACE_LIMITS.result_string_chars,
        truncated,
    );
    let matches = value
        .get("matches")
        .and_then(Value::as_array)
        .map(|items| {
            insert(&mut projected, "matchCount", json!(items.len()));
            if items.len() > CONVERSATION_TRACE_LIMITS.search_result_count {
                *truncated = true;
            }
            items
                .iter()
                .take(CONVERSATION_TRACE_LIMITS.search_result_count)
                .map(|item| {
                    pick_fields(
                        item,
                        &["path", "lineNumber", "line", "kind", "sizeBytes"],
                        CONVERSATION_TRACE_LIMITS.search_snippet_chars,
                        truncated,
                    )
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    insert(&mut projected, "matches", json!(matches));
    projected
}

fn project_workspace_map_result(value: &Value, truncated: &mut bool) -> Value {
    let summary = value.get("summary").unwrap_or(value);
    pick_fields(
        summary,
        &[
            "focusPath",
            "fileCount",
            "directoryCount",
            "totalBytes",
            "languages",
            "importantFiles",
            "entrypointCandidates",
            "testCandidates",
            "documentationCandidates",
            "truncated",
        ],
        CONVERSATION_TRACE_LIMITS.search_snippet_chars,
        truncated,
    )
}

fn project_git_diff_result(value: &Value, truncated: &mut bool) -> Value {
    let mut projected = pick_fields(
        value,
        &["path", "truncated"],
        CONVERSATION_TRACE_LIMITS.result_string_chars,
        truncated,
    );
    if let Some(patch) = value.get("patch").and_then(Value::as_str) {
        let (additions, deletions) = unified_diff_counts(patch);
        let all_changed_paths = patch
            .lines()
            .filter_map(|line| line.strip_prefix("+++ b/"))
            .collect::<Vec<_>>();
        if all_changed_paths.len() > CONVERSATION_TRACE_LIMITS.search_result_count {
            *truncated = true;
        }
        let changed_paths = all_changed_paths
            .into_iter()
            .take(CONVERSATION_TRACE_LIMITS.search_result_count)
            .map(|path| {
                let (path, path_truncated) =
                    bounded_text(path, CONVERSATION_TRACE_LIMITS.operation_string_chars);
                *truncated |= path_truncated;
                json!(path)
            })
            .collect::<Vec<_>>();
        insert(&mut projected, "additions", json!(additions));
        insert(&mut projected, "deletions", json!(deletions));
        insert(&mut projected, "changedPaths", json!(changed_paths));
    }
    projected
}

fn project_attachment_result(value: &Value, truncated: &mut bool) -> Value {
    let mut projected = pick_fields(
        value,
        &["scope", "total", "truncated"],
        CONVERSATION_TRACE_LIMITS.result_string_chars,
        truncated,
    );
    let attachments = value
        .get("attachments")
        .and_then(Value::as_array)
        .map(|items| {
            if items.len() > CONVERSATION_TRACE_LIMITS.search_result_count {
                *truncated = true;
            }
            items
                .iter()
                .take(CONVERSATION_TRACE_LIMITS.search_result_count)
                .map(|item| {
                    pick_fields(
                        item,
                        &["kind", "name", "mimeType", "sizeBytes", "readPath"],
                        CONVERSATION_TRACE_LIMITS.search_snippet_chars,
                        truncated,
                    )
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    insert(&mut projected, "attachments", json!(attachments));
    projected
}

fn project_todo_call(value: &Value, truncated: &mut bool) -> Value {
    let items = value
        .get("items")
        .and_then(Value::as_array)
        .map(|items| {
            if items.len() > CONVERSATION_TRACE_LIMITS.max_todo_items {
                *truncated = true;
            }
            items
                .iter()
                .take(CONVERSATION_TRACE_LIMITS.max_todo_items)
                .map(|item| {
                    pick_fields(
                        item,
                        &["id", "title", "status", "note"],
                        CONVERSATION_TRACE_LIMITS.todo_item_chars,
                        truncated,
                    )
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    json!({ "items": items })
}

fn project_todo_result(value: &Value, truncated: &mut bool) -> Value {
    let mut projected = pick_fields(
        value,
        &["revision"],
        CONVERSATION_TRACE_LIMITS.result_string_chars,
        truncated,
    );
    let items = value
        .get("items")
        .and_then(Value::as_array)
        .map(|items| {
            if items.len() > CONVERSATION_TRACE_LIMITS.max_todo_items {
                *truncated = true;
            }
            items
                .iter()
                .take(CONVERSATION_TRACE_LIMITS.max_todo_items)
                .map(|item| {
                    pick_fields(
                        item,
                        &["id", "title", "status", "note"],
                        CONVERSATION_TRACE_LIMITS.todo_item_chars,
                        truncated,
                    )
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    insert(&mut projected, "items", json!(items));
    projected
}

fn result_status(
    ok: bool,
    value: &Value,
    error: Option<&str>,
) -> ConversationTraceToolResultStatus {
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
    } else if ok && error.is_none() && !status.contains("fail") {
        ConversationTraceToolResultStatus::Succeeded
    } else {
        ConversationTraceToolResultStatus::Failed
    }
}

fn pick_fields(
    value: &Value,
    fields: &[&str],
    max_string_chars: usize,
    truncated: &mut bool,
) -> Value {
    let mut output = Map::new();
    let Some(input) = value.as_object() else {
        return Value::Object(output);
    };
    for field in fields {
        let Some(item) = input.get(*field) else {
            continue;
        };
        let projected = sanitize_value(item, max_string_chars, 0, truncated);
        output.insert((*field).to_string(), projected);
    }
    Value::Object(output)
}

fn pick_scalar_fields(
    value: &Value,
    fields: &[&str],
    max_string_chars: usize,
    truncated: &mut bool,
) -> Value {
    let mut output = Map::new();
    let Some(input) = value.as_object() else {
        return Value::Object(output);
    };
    for field in fields {
        let Some(item) = input.get(*field) else {
            continue;
        };
        if matches!(item, Value::Array(_) | Value::Object(_)) {
            *truncated = true;
            continue;
        }
        output.insert(
            (*field).to_string(),
            sanitize_value(item, max_string_chars, 0, truncated),
        );
    }
    Value::Object(output)
}

fn cap_projected_value(value: Value, truncated: &mut bool) -> Value {
    let serialized_chars = serialized_chars(&value);
    if serialized_chars <= CONVERSATION_TRACE_LIMITS.max_projected_json_chars
        && validate_durable_value_node("projected value", &value, 0).is_ok()
    {
        return value;
    }
    *truncated = true;
    json!({
        "durableDetailsOmitted": "projection_exceeded_limit",
        "originalJsonCharacters": serialized_chars,
    })
}

fn sanitize_value(
    value: &Value,
    max_string_chars: usize,
    depth: usize,
    truncated: &mut bool,
) -> Value {
    if depth >= CONVERSATION_TRACE_LIMITS.max_projection_depth {
        *truncated = true;
        return json!("[nested value omitted]");
    }
    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) => value.clone(),
        Value::String(value) => {
            let (value, was_truncated) = bounded_text(value, max_string_chars);
            *truncated |= was_truncated;
            json!(value)
        }
        Value::Array(items) => {
            let limit = CONVERSATION_TRACE_LIMITS.max_nested_collection_items;
            if items.len() > limit {
                *truncated = true;
            }
            Value::Array(
                items
                    .iter()
                    .take(limit)
                    .map(|item| sanitize_value(item, max_string_chars, depth + 1, truncated))
                    .collect(),
            )
        }
        Value::Object(object) => {
            let mut output = Map::new();
            for (key, item) in object
                .iter()
                .take(CONVERSATION_TRACE_LIMITS.max_object_fields)
            {
                let normalized = key.to_ascii_lowercase();
                if is_prohibited_durable_field(&normalized) {
                    *truncated = true;
                    continue;
                }
                output.insert(
                    key.clone(),
                    sanitize_value(item, max_string_chars, depth + 1, truncated),
                );
            }
            if object.len() > CONVERSATION_TRACE_LIMITS.max_object_fields {
                *truncated = true;
            }
            Value::Object(output)
        }
    }
}

fn validate_durable_identifier(label: &str, value: &str) -> Result<(), String> {
    if value.chars().count() > CONVERSATION_TRACE_LIMITS.identifier_chars {
        return Err(format!(
            "conversation trace {label} exceeds the durable identifier limit"
        ));
    }
    if redact_binary_text(value) != value {
        return Err(format!(
            "conversation trace {label} contains binary or data-URL material"
        ));
    }
    Ok(())
}

fn validate_durable_tool_name(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.chars().count() > CONVERSATION_TRACE_LIMITS.tool_name_chars
        || !value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        return Err("conversation trace tool name is not provider-safe".to_string());
    }
    Ok(())
}

fn validate_durable_text(label: &str, value: &str, max_chars: usize) -> Result<(), String> {
    let allowed = max_chars.saturating_add(TRUNCATION_SUFFIX.chars().count());
    if value.chars().count() > allowed {
        return Err(format!(
            "conversation trace {label} exceeds its durable text limit"
        ));
    }
    if redact_binary_text(value) != value {
        return Err(format!(
            "conversation trace {label} contains binary or data-URL material"
        ));
    }
    Ok(())
}

fn validate_durable_value(label: &str, value: &Value) -> Result<(), String> {
    let serialized = serde_json::to_string(value)
        .map_err(|error| format!("conversation trace {label} is not serializable: {error}"))?;
    if serialized.chars().count() > CONVERSATION_TRACE_LIMITS.max_projected_json_chars {
        return Err(format!(
            "conversation trace {label} exceeds its projected JSON limit"
        ));
    }
    validate_durable_value_node(label, value, 0)
}

fn validate_durable_value_node(label: &str, value: &Value, depth: usize) -> Result<(), String> {
    if depth > CONVERSATION_TRACE_LIMITS.max_projection_depth {
        return Err(format!(
            "conversation trace {label} exceeds the nesting limit"
        ));
    }
    match value {
        Value::String(value) => {
            let max_chars = CONVERSATION_TRACE_LIMITS
                .result_string_chars
                .max(CONVERSATION_TRACE_LIMITS.command_chars)
                .max(CONVERSATION_TRACE_LIMITS.command_output_chars)
                + TAIL_PREFIX
                    .chars()
                    .count()
                    .max(TRUNCATION_SUFFIX.chars().count());
            if value.chars().count() > max_chars {
                return Err(format!(
                    "conversation trace {label} contains an overlong projected string"
                ));
            }
            if redact_binary_text(value) != *value {
                return Err(format!(
                    "conversation trace {label} contains binary or data-URL material"
                ));
            }
        }
        Value::Array(items) => {
            if items.len() > CONVERSATION_TRACE_LIMITS.max_todo_items {
                return Err(format!(
                    "conversation trace {label} exceeds the collection limit"
                ));
            }
            for item in items {
                validate_durable_value_node(label, item, depth + 1)?;
            }
        }
        Value::Object(object) => {
            if object.len() > CONVERSATION_TRACE_LIMITS.max_object_fields {
                return Err(format!(
                    "conversation trace {label} exceeds the object-field limit"
                ));
            }
            for (key, item) in object {
                let normalized = key.to_ascii_lowercase();
                if is_prohibited_durable_field(&normalized) {
                    return Err(format!(
                        "conversation trace {label} contains prohibited durable field `{key}`"
                    ));
                }
                validate_durable_value_node(label, item, depth + 1)?;
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
    Ok(())
}

fn is_prohibited_durable_field(normalized: &str) -> bool {
    let canonical = normalized
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect::<String>();
    canonical.contains("base64")
        || canonical.contains("dataurl")
        || canonical.contains("reasoning")
        || canonical.contains("thinking")
        || canonical.ends_with("body")
        || canonical.ends_with("content")
        || canonical.ends_with("output")
        || canonical.ends_with("patch")
        || canonical.ends_with("payload")
        || canonical.ends_with("text")
        || matches!(
            canonical.as_str(),
            "body"
                | "content"
                | "fulltext"
                | "image"
                | "images"
                | "output"
                | "patch"
                | "payload"
                | "rawcontent"
                | "stderr"
                | "stdout"
                | "tail"
                | "text"
                | "thumbnaildataurl"
        )
}

fn durable_tool_name(value: &str) -> String {
    let trimmed = value.trim();
    if !trimmed.is_empty()
        && trimmed.chars().count() <= CONVERSATION_TRACE_LIMITS.tool_name_chars
        && trimmed
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        return trimmed.to_string();
    }

    let readable = trimmed
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .take(40)
        .collect::<String>();
    let readable = if readable.is_empty() {
        "historical_tool".to_string()
    } else {
        readable
    };
    format!("{readable}_{:016x}", stable_hash(value.as_bytes()))
}

fn durable_call_id(value: &str) -> String {
    let (bounded, changed) = bounded_text(value.trim(), CONVERSATION_TRACE_LIMITS.identifier_chars);
    if changed || bounded.trim().is_empty() {
        format!("durable_call_{:016x}", stable_hash(value.as_bytes()))
    } else {
        bounded
    }
}

fn semantic_todo_state(observation: &Value) -> Value {
    observation.get("items").cloned().unwrap_or(Value::Null)
}

fn bounded_text(value: &str, max_chars: usize) -> (String, bool) {
    let redacted = redact_binary_text(value);
    let count = redacted.chars().count();
    if count <= max_chars {
        let changed = redacted != value;
        return (redacted, changed);
    }
    let mut output = redacted.chars().take(max_chars).collect::<String>();
    output.push_str(TRUNCATION_SUFFIX);
    (output, true)
}

fn durable_error_text(value: &str) -> (String, bool) {
    let lines = value.lines().collect::<Vec<_>>();
    let line_limited = lines.len() > CONVERSATION_TRACE_LIMITS.error_lines;
    let selected = lines
        .into_iter()
        .take(CONVERSATION_TRACE_LIMITS.error_lines)
        .collect::<Vec<_>>()
        .join("\n");
    let (mut error, text_truncated) = bounded_text(
        &selected,
        CONVERSATION_TRACE_LIMITS
            .terminal_error_chars
            .min(CONVERSATION_TRACE_LIMITS.result_string_chars),
    );
    if line_limited && !error.ends_with(TRUNCATION_SUFFIX) {
        error.push_str(TRUNCATION_SUFFIX);
    }
    (error, line_limited || text_truncated)
}

fn bounded_tail_text(value: &str, max_chars: usize) -> (String, bool) {
    let redacted = redact_binary_text(value);
    let count = redacted.chars().count();
    if count <= max_chars {
        let changed = redacted != value;
        return (redacted, changed);
    }
    let mut output = TAIL_PREFIX.to_string();
    output.extend(redacted.chars().skip(count.saturating_sub(max_chars)));
    (output, true)
}

fn redact_binary_text(value: &str) -> String {
    if looks_like_base64(value.trim()) || looks_like_wrapped_base64(value) {
        return "[binary/base64 omitted]".to_string();
    }
    let lower = value.to_ascii_lowercase();
    if !lower.contains("data:") {
        return redact_base64_tokens(value);
    }

    let mut output = String::with_capacity(value.len().min(4_096));
    let mut cursor = 0;
    while let Some(relative) = lower[cursor..].find("data:") {
        let start = cursor + relative;
        output.push_str(&value[cursor..start]);
        output.push_str("[data-url omitted]");
        let end = value[start..]
            .find(|character: char| {
                character.is_whitespace() || matches!(character, '\"' | '\'' | ')' | ']' | '}')
            })
            .map(|offset| start + offset)
            .unwrap_or(value.len());
        cursor = end;
        if cursor >= value.len() {
            break;
        }
    }
    output.push_str(&value[cursor..]);
    redact_base64_tokens(&output)
}

fn redact_base64_tokens(value: &str) -> String {
    let mut output = String::with_capacity(value.len().min(4_096));
    for segment in value.split_inclusive(char::is_whitespace) {
        let whitespace_start = segment
            .char_indices()
            .find_map(|(index, character)| character.is_whitespace().then_some(index))
            .unwrap_or(segment.len());
        let token = &segment[..whitespace_start];
        let whitespace = &segment[whitespace_start..];
        let candidate = token.trim_matches(|character: char| {
            matches!(
                character,
                '\"' | '\''
                    | '('
                    | ')'
                    | '['
                    | ']'
                    | '{'
                    | '}'
                    | ','
                    | ':'
                    | ';'
                    | '.'
                    | '!'
                    | '?'
                    | '`'
                    | '<'
                    | '>'
            )
        });
        if looks_like_base64(candidate) {
            let start = token.find(candidate).unwrap_or(0);
            output.push_str(&token[..start]);
            output.push_str("[binary/base64 omitted]");
            output.push_str(&token[start + candidate.len()..]);
        } else {
            output.push_str(token);
        }
        output.push_str(whitespace);
    }
    output
}

fn looks_like_base64(value: &str) -> bool {
    let byte_len = value.len();
    let padded_short = byte_len >= 8
        && byte_len.is_multiple_of(4)
        && matches!(value.as_bytes().last(), Some(b'='));
    let has_upper = value
        .chars()
        .any(|character| character.is_ascii_uppercase());
    let has_lower = value
        .chars()
        .any(|character| character.is_ascii_lowercase());
    let has_base64_marker = value
        .chars()
        .any(|character| character.is_ascii_digit() || matches!(character, '+' | '/' | '-' | '_'));
    let unpadded_short =
        byte_len >= 7 && !byte_len.is_multiple_of(4) && has_upper && has_lower && has_base64_marker;
    let aligned_unpadded_short = byte_len >= 8
        && byte_len.is_multiple_of(4)
        && !value.contains('=')
        && has_upper
        && has_lower;
    (byte_len >= CONVERSATION_TRACE_LIMITS.base64_token_chars
        || padded_short
        || unpadded_short
        || aligned_unpadded_short)
        && !value.chars().any(char::is_whitespace)
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '+' | '/' | '=' | '-' | '_')
        })
}

fn looks_like_wrapped_base64(value: &str) -> bool {
    let mut run_lines = 0_usize;
    let mut run_chars = 0_usize;
    for line in value.lines().map(str::trim) {
        let candidate = line.trim_matches(|character: char| {
            matches!(
                character,
                '"' | '\'' | '(' | ')' | '[' | ']' | '{' | '}' | ','
            )
        });
        let is_candidate = candidate.len() >= CONVERSATION_TRACE_LIMITS.wrapped_base64_line_chars
            && candidate.chars().all(|character| {
                character.is_ascii_alphanumeric()
                    || matches!(character, '+' | '/' | '=' | '-' | '_')
            });
        if is_candidate {
            run_lines += 1;
            run_chars = run_chars.saturating_add(candidate.len());
            if run_lines >= 2 && run_chars >= CONVERSATION_TRACE_LIMITS.base64_token_chars {
                return true;
            }
        } else {
            run_lines = 0;
            run_chars = 0;
        }
    }
    false
}

fn serialized_chars(value: &(impl Serialize + ?Sized)) -> usize {
    serde_json::to_string(value)
        .map(|value| value.chars().count())
        .unwrap_or(usize::MAX)
}

fn is_side_effecting_tool(tool: &str) -> bool {
    matches!(tool, "apply_patch" | "write_file" | "run_command")
}

fn insert(target: &mut Value, key: &str, value: Value) {
    if let Some(object) = target.as_object_mut() {
        object.insert(key.to_string(), value);
    }
}

fn unified_diff_counts(patch: &str) -> (u64, u64) {
    let additions = patch
        .lines()
        .filter(|line| line.starts_with('+') && !line.starts_with("+++"))
        .count() as u64;
    let deletions = patch
        .lines()
        .filter(|line| line.starts_with('-') && !line.starts_with("---"))
        .count() as u64;
    (additions, deletions)
}

fn failure_signature(tool: &str, operation: &Value, projection: &ToolResultProjection) -> String {
    serde_json::to_string(&json!({
        "tool": tool,
        "operation": operation,
        "status": projection.status,
        "observation": projection.observation,
        "error": projection.error,
    }))
    .unwrap_or_else(|_| format!("{tool}:{}", projection.status.as_str()))
}

fn stable_hash(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325_u64, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(tool: &str, args: Value) -> AgentToolCall {
        AgentToolCall {
            id: format!("{tool}-1"),
            tool: tool.to_string(),
            args,
            approval_status: AgentApprovalStatus::NotRequired,
            reason: None,
        }
    }

    #[test]
    fn durable_projection_removes_web_body_images_and_data_urls() {
        let call = call("web_fetch", json!({ "url": "https://example.test" }));
        let body = "page ".repeat(2_000);
        let result = AgentToolResult {
            call_id: call.id.clone(),
            tool: call.tool.clone(),
            ok: true,
            result: Some(json!({
                "url": "https://example.test",
                "content": body,
                "images": ["data:image/png;base64,SECRET_BASE64"],
                "favicon": "data:image/png;base64,SECRET_FAVICON",
                "truncated": false
            })),
            error: None,
        };
        let projection = project_tool_result(&call, &result);
        let serialized = serde_json::to_string(&projection.observation).unwrap();

        assert!(!serialized.contains("SECRET_BASE64"));
        assert!(!serialized.contains("SECRET_FAVICON"));
        assert!(!serialized.contains(&"page ".repeat(400)));
        assert_eq!(
            projection.observation["contentCharacters"],
            json!(body.chars().count())
        );
        assert_eq!(projection.observation["durableExcerptTruncated"], true);
        assert!(projection.observation["observedExcerpt"]
            .as_str()
            .unwrap()
            .starts_with("page "));
        assert!(projection.truncated);

        let short = project_tool_result(
            &call,
            &AgentToolResult {
                call_id: call.id.clone(),
                tool: call.tool.clone(),
                ok: true,
                result: Some(json!({
                    "url": "https://example.test/short",
                    "content": "COMPLETE_SHORT_PAGE_BODY"
                })),
                error: None,
            },
        );
        let short = serde_json::to_string(&short.observation).unwrap();
        assert!(!short.contains("COMPLETE_SHORT_PAGE_BODY"));
    }

    #[test]
    fn durable_projection_never_keeps_file_bodies_patch_or_hidden_reasoning() {
        for (tool, args, result) in [
            (
                "write_file",
                json!({ "phase": "append", "content": "PRIVATE_FILE_BODY" }),
                json!({ "draft": { "filePath": "src/a.rs", "status": "drafting" }, "tail": "PRIVATE_FILE_BODY" }),
            ),
            (
                "apply_patch",
                json!({ "filePath": "src/a.rs", "patch": "PRIVATE_PATCH" }),
                json!({ "status": "applied", "filePath": "src/a.rs", "reasoning": "HIDDEN_REASONING" }),
            ),
            (
                "read_file",
                json!({ "path": "src/a.rs" }),
                json!({ "path": "src/a.rs", "content": "WHOLE_FILE" }),
            ),
        ] {
            let call = call(tool, args);
            let (operation, _) = project_tool_call(&call);
            let projection = project_tool_result(
                &call,
                &AgentToolResult {
                    call_id: call.id.clone(),
                    tool: tool.to_string(),
                    ok: true,
                    result: Some(result),
                    error: None,
                },
            );
            let serialized = format!(
                "{} {}",
                serde_json::to_string(&operation).unwrap(),
                serde_json::to_string(&projection.observation).unwrap()
            );
            assert!(!serialized.contains("PRIVATE_FILE_BODY"));
            assert!(!serialized.contains("PRIVATE_PATCH"));
            assert!(!serialized.contains("HIDDEN_REASONING"));
            assert!(!serialized.contains("WHOLE_FILE"));
        }
    }

    #[test]
    fn base64_and_data_urls_never_enter_a_finished_trace() {
        let encoded = "QUJDREVGR0hJSktMTU5PUFFSU1RVVldYWVo=".repeat(8);
        let mut recorder = ConversationTraceRecorder::default();
        recorder.record_narration(&format!("Visual payload: {encoded}"));
        let call = call("read_image", json!({ "path": "diagram.png" }));
        recorder.record_tool_call(&call);
        recorder.record_tool_result(
            &call,
            &AgentToolResult {
                call_id: call.id.clone(),
                tool: call.tool.clone(),
                ok: true,
                result: Some(json!({
                    "path": "diagram.png",
                    "mimeType": "image/png",
                    "thumbnailDataUrl": "data:image/png;base64,THUMBNAIL_SECRET",
                    "image": { "dataBase64": encoded }
                })),
                error: None,
            },
        );
        let trace = recorder.finish(
            "run-image",
            "conversation-image",
            "assistant-image",
            ConversationTurnTraceTerminalStatus::Completed,
            None,
        );

        trace.validate().unwrap();
        let serialized = serde_json::to_string(&trace).unwrap();
        assert!(!serialized.contains("QUJDREVGR0hJSktMTU5PUFFSU1RVVldYWVo"));
        assert!(!serialized.contains("THUMBNAIL_SECRET"));
        assert!(!serialized.contains("data:image"));
        assert!(serialized.contains("binary/base64 omitted"));
    }

    #[test]
    fn command_projection_keeps_bounded_tail_and_exit_status() {
        let call = call(
            "run_command",
            json!({ "command": "pnpm check", "cwd": "." }),
        );
        let output = format!("{}IMPORTANT_TAIL", "line output\n".repeat(1_000));
        let projection = project_tool_result(
            &call,
            &AgentToolResult {
                call_id: call.id.clone(),
                tool: call.tool.clone(),
                ok: false,
                result: Some(json!({
                    "command": "pnpm check",
                    "cwd": ".",
                    "exitCode": 1,
                    "stdout": output,
                    "stderr": "failure",
                    "stdoutTruncated": false,
                    "stderrTruncated": false
                })),
                error: Some("command failed".to_string()),
            },
        );

        assert_eq!(projection.observation["exitCode"], 1);
        assert!(projection.observation["stdoutTail"]
            .as_str()
            .unwrap()
            .ends_with("IMPORTANT_TAIL"));
        assert!(projection.truncated);

        let short = project_tool_result(
            &call,
            &AgentToolResult {
                call_id: call.id.clone(),
                tool: call.tool.clone(),
                ok: true,
                result: Some(json!({ "exitCode": 0, "stdout": "SHORT_COMPLETE_OUTPUT" })),
                error: None,
            },
        );
        let short = serde_json::to_string(&short.observation).unwrap();
        assert!(!short.contains("SHORT_COMPLETE_OUTPUT"));
    }

    #[test]
    fn wrapped_and_short_padded_base64_are_redacted_from_durable_text() {
        let wrapped = format!("{}\n{}", "A".repeat(76), "B".repeat(76));
        let command = call("run_command", json!({ "command": "decode" }));
        let projection = project_tool_result(
            &command,
            &AgentToolResult {
                call_id: command.id.clone(),
                tool: command.tool.clone(),
                ok: true,
                result: Some(json!({
                    "exitCode": 0,
                    "stdout": format!("prefix\n{wrapped}\nsuffix")
                })),
                error: None,
            },
        );
        let serialized = serde_json::to_string(&projection.observation).unwrap();
        assert!(!serialized.contains(&"A".repeat(40)));
        assert!(serialized.contains("binary/base64 omitted"));
        assert_eq!(redact_binary_text("SGVsbG8="), "[binary/base64 omitted]");
        assert_eq!(
            redact_binary_text("payload SGVsbG8=. done"),
            "payload [binary/base64 omitted]. done"
        );
        assert_eq!(
            redact_binary_text("payload SGVsbG8 done"),
            "payload [binary/base64 omitted] done"
        );
        assert_eq!(
            redact_binary_text("payload YWJjZGVm done"),
            "payload [binary/base64 omitted] done"
        );
        assert!(!redact_binary_text("data:image/png;base64,\nSGVsbG8").contains("SGVsbG8"));
    }

    #[test]
    fn maximum_todo_projection_remains_valid_and_bounded() {
        let items = (0..100)
            .map(|index| {
                json!({
                    "id": format!("todo-{index}"),
                    "title": "T".repeat(2_000),
                    "status": "pending",
                    "note": "N".repeat(2_000),
                })
            })
            .collect::<Vec<_>>();
        let call = call("todo_update", json!({ "items": items.clone() }));
        let mut recorder = ConversationTraceRecorder::default();
        recorder.record_tool_call(&call);
        recorder.record_tool_result(
            &call,
            &AgentToolResult {
                call_id: call.id.clone(),
                tool: call.tool.clone(),
                ok: true,
                result: Some(json!({ "revision": 1, "items": items })),
                error: None,
            },
        );
        let trace = recorder.finish(
            "run-todo",
            "conversation-todo",
            "assistant-todo",
            ConversationTurnTraceTerminalStatus::Completed,
            None,
        );

        trace.validate().unwrap();
        assert!(trace.truncated);
        assert!(serialized_chars(&trace) <= CONVERSATION_TRACE_LIMITS.max_trace_json_chars);
    }

    #[test]
    fn unsafe_tool_names_are_canonicalized_for_provider_history() {
        let call = call("bad tool/name with spaces", json!({ "path": "safe.txt" }));
        let mut recorder = ConversationTraceRecorder::default();
        recorder.record_tool_call(&call);
        recorder.record_tool_result(
            &call,
            &AgentToolResult {
                call_id: call.id.clone(),
                tool: call.tool.clone(),
                ok: true,
                result: Some(json!({ "path": "safe.txt" })),
                error: None,
            },
        );
        let trace = recorder.finish(
            "run-tool",
            "conversation-tool",
            "assistant-tool",
            ConversationTurnTraceTerminalStatus::Completed,
            None,
        );

        trace.validate().unwrap();
        let tool = match &trace.items[0] {
            ConversationTurnTraceItem::ToolCall { tool, .. } => tool,
            _ => panic!("expected tool call"),
        };
        assert!(tool.len() <= CONVERSATION_TRACE_LIMITS.tool_name_chars);
        assert!(tool
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_')));
    }

    #[test]
    fn side_effect_exchange_is_retained_after_read_exchange_limit() {
        let mut recorder = ConversationTraceRecorder::default();
        for index in 0..CONVERSATION_TRACE_LIMITS.max_tool_exchanges {
            let call = AgentToolCall {
                id: format!("read-{index}"),
                tool: "read_file".to_string(),
                args: json!({ "path": format!("file-{index}.txt") }),
                approval_status: AgentApprovalStatus::NotRequired,
                reason: None,
            };
            recorder.record_tool_call(&call);
            recorder.record_tool_result(
                &call,
                &AgentToolResult {
                    call_id: call.id.clone(),
                    tool: call.tool.clone(),
                    ok: true,
                    result: Some(json!({ "path": format!("file-{index}.txt") })),
                    error: None,
                },
            );
        }
        let write = AgentToolCall {
            id: "write-after-limit".to_string(),
            tool: "write_file".to_string(),
            args: json!({ "phase": "finish", "filePath": "important.txt" }),
            approval_status: AgentApprovalStatus::Approved,
            reason: None,
        };
        recorder.record_tool_call(&write);
        recorder.record_tool_result(
            &write,
            &AgentToolResult {
                call_id: write.id.clone(),
                tool: write.tool.clone(),
                ok: true,
                result: Some(json!({ "status": "applied", "filePath": "important.txt" })),
                error: None,
            },
        );
        let trace = recorder.finish(
            "run-limit",
            "conversation-limit",
            "assistant-limit",
            ConversationTurnTraceTerminalStatus::Cancelled,
            Some("cancelled after writes"),
        );

        trace.validate().unwrap();
        assert!(trace.truncated);
        assert!(trace.items.iter().any(|item| matches!(
            item,
            ConversationTurnTraceItem::ToolResult { tool, observation, .. }
                if tool == "write_file" && observation["filePath"] == "important.txt"
        )));
        assert!(trace.items.iter().any(|item| matches!(
            item,
            ConversationTurnTraceItem::ToolCall { call_id, .. } if call_id == "read-0"
        )));
    }

    #[test]
    fn narration_limit_never_evicts_an_already_recorded_prefix() {
        let mut recorder = ConversationTraceRecorder::default();
        for index in 0..=CONVERSATION_TRACE_LIMITS.max_narrations {
            recorder.record_narration(&format!("narration-{index}"));
        }
        let trace = recorder.finish(
            "run-narration-limit",
            "conversation-narration-limit",
            "assistant-narration-limit",
            ConversationTurnTraceTerminalStatus::Completed,
            None,
        );

        trace.validate().unwrap();
        assert!(trace.truncated);
        assert_eq!(trace.items.len(), CONVERSATION_TRACE_LIMITS.max_narrations);
        assert!(matches!(
            &trace.items[0],
            ConversationTurnTraceItem::AssistantNarration { content, .. }
                if content == "narration-0"
        ));
        assert!(trace.items.iter().all(|item| !matches!(
            item,
            ConversationTurnTraceItem::AssistantNarration { content, .. }
                if content == &format!("narration-{}", CONVERSATION_TRACE_LIMITS.max_narrations)
        )));
    }

    #[test]
    fn enrichment_uses_the_same_redaction_and_truncation_contract() {
        let mut recorder = ConversationTraceRecorder::default();
        let call = call("run_command", json!({ "command": "placeholder" }));
        recorder.record_tool_call(&call);
        let wrapped = format!("{}\n{}", "A".repeat(76), "B".repeat(76));
        recorder.enrich_tool_call(&AgentProposedAction::Command {
            command: crate::AgentCommandRequest {
                id: call.id.clone(),
                command: wrapped.clone(),
                cwd: Some("C".repeat(5_000)),
                timeout_ms: None,
                approval_status: AgentApprovalStatus::Approved,
                risk_level: None,
                reason: None,
            },
        });
        recorder.record_tool_result(
            &call,
            &AgentToolResult {
                call_id: call.id.clone(),
                tool: call.tool.clone(),
                ok: true,
                result: Some(json!({ "exitCode": 0 })),
                error: None,
            },
        );
        let trace = recorder.finish(
            "run-enrich",
            "conversation-enrich",
            "assistant-enrich",
            ConversationTurnTraceTerminalStatus::Completed,
            None,
        );

        trace.validate().unwrap();
        let serialized = serde_json::to_string(&trace).unwrap();
        assert!(!serialized.contains(&wrapped));
        assert!(trace.truncated);
    }

    #[test]
    fn validation_rejects_external_traces_above_declared_item_limits() {
        let trace = ConversationTurnTrace {
            schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
            run_id: "run-external".to_string(),
            conversation_id: "conversation-external".to_string(),
            assistant_message_id: "assistant-external".to_string(),
            terminal_status: ConversationTurnTraceTerminalStatus::Completed,
            terminal_error: None,
            truncated: false,
            items: (0..=CONVERSATION_TRACE_LIMITS.max_narrations)
                .map(|sequence| ConversationTurnTraceItem::AssistantNarration {
                    sequence: sequence as u64,
                    content: "visible".to_string(),
                    truncated: false,
                })
                .collect(),
        };

        assert!(trace.validate().unwrap_err().contains("item-count"));
    }

    #[test]
    fn malformed_nested_projection_is_capped_and_forbidden_key_variants_are_rejected() {
        let call = call("write_file", json!({ "phase": "finish" }));
        let projection = project_tool_result(
            &call,
            &AgentToolResult {
                call_id: call.id.clone(),
                tool: call.tool.clone(),
                ok: true,
                result: Some(json!({
                    "summary": { "a": { "b": { "c": { "d": "too deep" } } } }
                })),
                error: None,
            },
        );
        assert!(projection.truncated);
        assert_eq!(
            projection.observation["durableDetailsOmitted"],
            "projection_exceeded_limit"
        );

        for key in [
            "raw_content",
            "full_text",
            "pageContent",
            "fileContent",
            "commandOutput",
        ] {
            let mut observation = Map::new();
            observation.insert(key.to_string(), json!("private body"));
            let trace = ConversationTurnTrace {
                schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
                run_id: "run-fields".to_string(),
                conversation_id: "conversation-fields".to_string(),
                assistant_message_id: "assistant-fields".to_string(),
                terminal_status: ConversationTurnTraceTerminalStatus::Completed,
                terminal_error: None,
                truncated: false,
                items: vec![
                    ConversationTurnTraceItem::ToolCall {
                        sequence: 0,
                        call_id: "call-fields".to_string(),
                        tool: "custom_tool".to_string(),
                        operation: json!({}),
                        approval_status: AgentApprovalStatus::NotRequired,
                        truncated: false,
                    },
                    ConversationTurnTraceItem::ToolResult {
                        sequence: 1,
                        call_id: "call-fields".to_string(),
                        tool: "custom_tool".to_string(),
                        status: ConversationTraceToolResultStatus::Succeeded,
                        success: true,
                        observation: Value::Object(observation),
                        approval_status: AgentApprovalStatus::NotRequired,
                        error: None,
                        truncated: false,
                    },
                ],
            };
            assert!(trace.validate().is_err(), "{key} should be rejected");
        }
    }

    #[test]
    fn durable_errors_keep_only_bounded_reason_lines() {
        let error = [
            "clear failure reason",
            "context line 1",
            "context line 2",
            "context line 3",
            "PRIVATE_ECHOED_BODY",
        ]
        .join("\n");
        let (durable, truncated) = durable_error_text(&error);

        assert!(truncated);
        assert!(durable.contains("clear failure reason"));
        assert!(!durable.contains("PRIVATE_ECHOED_BODY"));
    }

    #[test]
    fn recorder_deduplicates_unchanged_failures_without_breaking_pairs() {
        let mut recorder = ConversationTraceRecorder::default();
        for id in ["first", "second"] {
            let call = AgentToolCall {
                id: id.to_string(),
                tool: "read_file".to_string(),
                args: json!({ "path": "missing.txt" }),
                approval_status: AgentApprovalStatus::NotRequired,
                reason: None,
            };
            recorder.record_tool_call(&call);
            recorder.record_tool_result(
                &call,
                &AgentToolResult {
                    call_id: id.to_string(),
                    tool: "read_file".to_string(),
                    ok: false,
                    result: None,
                    error: Some("not found".to_string()),
                },
            );
        }
        let trace = recorder.finish(
            "run-1",
            "conversation-1",
            "assistant-1",
            ConversationTurnTraceTerminalStatus::Failed,
            Some("stopped"),
        );

        trace.validate().unwrap();
        assert_eq!(
            trace
                .items
                .iter()
                .filter(|item| matches!(item, ConversationTurnTraceItem::ToolCall { .. }))
                .count(),
            1
        );
    }

    #[test]
    fn recorder_keeps_same_error_for_distinct_operations() {
        let mut recorder = ConversationTraceRecorder::default();
        for (id, path) in [("first", "a.txt"), ("second", "b.txt")] {
            let call = AgentToolCall {
                id: id.to_string(),
                tool: "read_file".to_string(),
                args: json!({ "path": path }),
                approval_status: AgentApprovalStatus::NotRequired,
                reason: None,
            };
            recorder.record_tool_call(&call);
            recorder.record_tool_result(
                &call,
                &AgentToolResult {
                    call_id: id.to_string(),
                    tool: "read_file".to_string(),
                    ok: false,
                    result: None,
                    error: Some("not found".to_string()),
                },
            );
        }
        let trace = recorder.finish(
            "run-distinct-errors",
            "conversation-distinct-errors",
            "assistant-distinct-errors",
            ConversationTurnTraceTerminalStatus::Failed,
            Some("stopped"),
        );

        trace.validate().unwrap();
        assert_eq!(
            trace
                .items
                .iter()
                .filter(|item| matches!(item, ConversationTurnTraceItem::ToolResult { .. }))
                .count(),
            2
        );
    }

    #[test]
    fn running_snapshot_commits_narration_and_only_closed_tool_exchanges() {
        let mut recorder = ConversationTraceRecorder::default();
        recorder.record_narration("I will inspect the file.");
        let call = call("read_file", json!({ "path": "src/lib.rs" }));
        recorder.record_tool_call(&call);

        let pending = recorder.snapshot();
        assert_eq!(pending.items.len(), 2);
        let committed = pending.in_progress_trace("run", "conversation", "assistant");
        committed.validate().unwrap();
        assert_eq!(committed.items.len(), 1);
        assert_eq!(
            committed.terminal_status,
            ConversationTurnTraceTerminalStatus::InProgress
        );

        recorder.record_tool_result(
            &call,
            &AgentToolResult {
                call_id: call.id.clone(),
                tool: call.tool.clone(),
                ok: true,
                result: Some(json!({ "path": "src/lib.rs", "endLine": 20 })),
                error: None,
            },
        );
        let closed = recorder
            .snapshot()
            .in_progress_trace("run", "conversation", "assistant");
        closed.validate().unwrap();
        assert_eq!(closed.items.len(), 3);
        assert_eq!(closed.items[0], committed.items[0]);
    }
}
