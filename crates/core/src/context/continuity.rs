//! Deterministic continuity records retained beside a lossy compaction summary.
//!
//! Raw messages and trace items remain authoritative. This module projects only bounded identity,
//! chronology and outcome metadata needed to continue work or locate the original record later.

use super::compaction_summary::{
    ContextCompactionPrefix, ContextCompactionSourceItem, ContextJournalCursor,
};
use super::format_message_created_at;
use crate::content_revision;
use crate::conversation_trace::{
    ConversationTraceToolResultStatus, ConversationTurnTraceItem,
    ConversationTurnTraceTerminalStatus,
};
use crate::protocol::{AgentApprovalStatus, AgentError, AgentResult};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeSet;

pub const CONTEXT_CONTINUITY_SCHEMA_VERSION: u32 = 1;

const USER_MESSAGE_INLINE_CHARS: usize = 4_000;
const ASSISTANT_MESSAGE_PREVIEW_CHARS: usize = 800;
const NARRATION_PREVIEW_CHARS: usize = 500;
const ERROR_PREVIEW_CHARS: usize = 1_000;
const METADATA_STRING_CHARS: usize = 500;
const METADATA_MAX_DEPTH: usize = 3;
const METADATA_MAX_OBJECT_FIELDS: usize = 24;
const METADATA_MAX_ARRAY_ITEMS: usize = 8;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ContextContinuitySnapshot {
    pub schema_version: u32,
    pub covered_through: ContextJournalCursor,
    pub entries: Vec<ContextContinuityEntry>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ContextContinuityText {
    pub text: String,
    pub total_chars: usize,
    pub content_revision: String,
    pub truncated: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum ContextContinuityEntry {
    UserMessage {
        cursor: ContextJournalCursor,
        created_at: String,
        text: ContextContinuityText,
        #[serde(skip_serializing_if = "Option::is_none")]
        status: Option<String>,
    },
    AssistantMessage {
        cursor: ContextJournalCursor,
        created_at: String,
        text: ContextContinuityText,
        #[serde(skip_serializing_if = "Option::is_none")]
        status: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        terminal_status: Option<ConversationTurnTraceTerminalStatus>,
        #[serde(skip_serializing_if = "Option::is_none")]
        terminal_error: Option<String>,
    },
    AssistantNarration {
        cursor: ContextJournalCursor,
        run_id: String,
        created_at: String,
        preview: String,
        total_chars: usize,
        truncated: bool,
    },
    ToolExchange {
        call_cursor: ContextJournalCursor,
        result_cursor: ContextJournalCursor,
        run_id: String,
        created_at: String,
        call_id: String,
        tool: String,
        operation: Value,
        status: ConversationTraceToolResultStatus,
        success: bool,
        outcome: Value,
        approval_status: AgentApprovalStatus,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
        truncated: bool,
    },
}

impl ContextContinuitySnapshot {
    pub fn from_prefix(prefix: &ContextCompactionPrefix) -> AgentResult<Self> {
        prefix.validate()?;
        let mut entries = match &prefix.previous_summary {
            Some(previous) => {
                previous.continuity.validate()?;
                if previous.continuity.covered_through != previous.covered_through {
                    return Err(AgentError::new(
                        "上一版上下文连续性骨架与摘要覆盖边界不一致。",
                    ));
                }
                previous.continuity.entries.clone()
            }
            None => Vec::new(),
        };
        let mut pending_call: Option<PendingToolCall> = None;

        for source in &prefix.source_items {
            match source {
                ContextCompactionSourceItem::Message {
                    cursor,
                    role,
                    content,
                    created_at,
                    status,
                    terminal_status,
                    terminal_error,
                } => {
                    ensure_no_pending_call(&pending_call)?;
                    let created_at = format_message_created_at(*created_at)?;
                    match role.as_str() {
                        "user" => entries.push(ContextContinuityEntry::UserMessage {
                            cursor: cursor.clone(),
                            created_at,
                            text: project_text(content, USER_MESSAGE_INLINE_CHARS),
                            status: status.clone(),
                        }),
                        "assistant" => entries.push(ContextContinuityEntry::AssistantMessage {
                            cursor: cursor.clone(),
                            created_at,
                            text: project_text(content, ASSISTANT_MESSAGE_PREVIEW_CHARS),
                            status: status.clone(),
                            terminal_status: *terminal_status,
                            terminal_error: terminal_error
                                .as_deref()
                                .map(|error| truncate_chars(error, ERROR_PREVIEW_CHARS).0),
                        }),
                        _ => return Err(AgentError::new("上下文连续性骨架遇到未知消息角色。")),
                    }
                }
                ContextCompactionSourceItem::TraceItem {
                    cursor,
                    run_id,
                    created_at,
                    item,
                } => match item {
                    ConversationTurnTraceItem::AssistantNarration {
                        content, truncated, ..
                    } => {
                        ensure_no_pending_call(&pending_call)?;
                        let (preview, preview_truncated) =
                            truncate_chars(content, NARRATION_PREVIEW_CHARS);
                        entries.push(ContextContinuityEntry::AssistantNarration {
                            cursor: cursor.clone(),
                            run_id: run_id.clone(),
                            created_at: format_message_created_at(*created_at)?,
                            preview,
                            total_chars: content.chars().count(),
                            truncated: *truncated || preview_truncated,
                        });
                    }
                    ConversationTurnTraceItem::UserGuidance {
                        content,
                        created_at,
                        ..
                    } => {
                        ensure_no_pending_call(&pending_call)?;
                        entries.push(ContextContinuityEntry::UserMessage {
                            cursor: cursor.clone(),
                            created_at: format_message_created_at(*created_at)?,
                            text: project_text(content, USER_MESSAGE_INLINE_CHARS),
                            status: Some("applied".to_string()),
                        });
                    }
                    ConversationTurnTraceItem::ToolCall {
                        call_id,
                        tool,
                        operation,
                        approval_status,
                        truncated,
                        ..
                    } => {
                        ensure_no_pending_call(&pending_call)?;
                        let operation = project_metadata(operation);
                        pending_call = Some(PendingToolCall {
                            cursor: cursor.clone(),
                            run_id: run_id.clone(),
                            created_at: *created_at,
                            call_id: call_id.clone(),
                            tool: tool.clone(),
                            operation: operation.value,
                            approval_status: *approval_status,
                            truncated: *truncated || operation.truncated,
                        });
                    }
                    ConversationTurnTraceItem::ToolResult {
                        call_id,
                        tool,
                        status,
                        success,
                        observation,
                        approval_status,
                        error,
                        truncated,
                        ..
                    } => {
                        let call = pending_call.take().ok_or_else(|| {
                            AgentError::new("上下文连续性骨架遇到没有调用记录的工具结果。")
                        })?;
                        if call.call_id != *call_id || call.tool != *tool {
                            return Err(AgentError::new(
                                "上下文连续性骨架中的工具调用与结果不匹配。",
                            ));
                        }
                        if call.approval_status != *approval_status {
                            return Err(AgentError::new(
                                "上下文连续性骨架中的工具调用与结果审批状态不一致。",
                            ));
                        }
                        let outcome = project_metadata(observation);
                        let (error, error_truncated) = error
                            .as_deref()
                            .map(|error| truncate_chars(error, ERROR_PREVIEW_CHARS))
                            .map(|(error, truncated)| (Some(error), truncated))
                            .unwrap_or((None, false));
                        entries.push(ContextContinuityEntry::ToolExchange {
                            call_cursor: call.cursor,
                            result_cursor: cursor.clone(),
                            run_id: call.run_id,
                            created_at: format_message_created_at(call.created_at)?,
                            call_id: call.call_id,
                            tool: call.tool,
                            operation: call.operation,
                            status: *status,
                            success: *success,
                            outcome: outcome.value,
                            approval_status: *approval_status,
                            error,
                            truncated: call.truncated
                                || *truncated
                                || outcome.truncated
                                || error_truncated,
                        });
                    }
                },
            }
        }
        ensure_no_pending_call(&pending_call)?;

        let snapshot = Self {
            schema_version: CONTEXT_CONTINUITY_SCHEMA_VERSION,
            covered_through: prefix.covered_through.clone(),
            entries,
        };
        snapshot.validate()?;
        Ok(snapshot)
    }

    pub fn validate(&self) -> AgentResult<()> {
        if self.schema_version != CONTEXT_CONTINUITY_SCHEMA_VERSION {
            return Err(AgentError::new(format!(
                "不支持的 ContextContinuitySnapshot schema version：{}。",
                self.schema_version
            )));
        }
        self.covered_through.validate()?;
        if self.entries.is_empty() {
            return Err(AgentError::new("上下文连续性骨架不能为空。"));
        }

        let mut references = BTreeSet::new();
        for entry in &self.entries {
            entry.validate()?;
            for cursor in entry.cursors() {
                if !references.insert(cursor_key(cursor)) {
                    return Err(AgentError::new("上下文连续性骨架包含重复的原始记录引用。"));
                }
            }
        }
        if self
            .entries
            .last()
            .and_then(ContextContinuityEntry::last_cursor)
            != Some(&self.covered_through)
        {
            return Err(AgentError::new(
                "上下文连续性骨架末尾与摘要覆盖边界不一致。",
            ));
        }
        Ok(())
    }

    pub(crate) fn render_json(&self) -> AgentResult<String> {
        self.validate()?;
        serde_json::to_string(self)
            .map_err(|error| AgentError::new(format!("无法序列化上下文连续性骨架：{error}")))
    }
}

impl ContextContinuityEntry {
    fn cursors(&self) -> Vec<&ContextJournalCursor> {
        match self {
            Self::UserMessage { cursor, .. }
            | Self::AssistantMessage { cursor, .. }
            | Self::AssistantNarration { cursor, .. } => vec![cursor],
            Self::ToolExchange {
                call_cursor,
                result_cursor,
                ..
            } => vec![call_cursor, result_cursor],
        }
    }

    fn last_cursor(&self) -> Option<&ContextJournalCursor> {
        match self {
            Self::UserMessage { cursor, .. }
            | Self::AssistantMessage { cursor, .. }
            | Self::AssistantNarration { cursor, .. } => Some(cursor),
            Self::ToolExchange { result_cursor, .. } => Some(result_cursor),
        }
    }

    fn validate(&self) -> AgentResult<()> {
        for cursor in self.cursors() {
            cursor.validate()?;
        }
        match self {
            Self::UserMessage {
                created_at, text, ..
            }
            | Self::AssistantMessage {
                created_at, text, ..
            } => {
                validate_created_at(created_at)?;
                text.validate()
            }
            Self::AssistantNarration {
                run_id,
                created_at,
                preview,
                total_chars,
                ..
            } => {
                validate_identity("run ID", run_id)?;
                validate_created_at(created_at)?;
                if preview.trim().is_empty() || preview.chars().count() > *total_chars {
                    return Err(AgentError::new("上下文连续性叙述预览无效。"));
                }
                Ok(())
            }
            Self::ToolExchange {
                run_id,
                call_id,
                tool,
                created_at,
                success,
                status,
                ..
            } => {
                validate_identity("run ID", run_id)?;
                validate_identity("工具调用 ID", call_id)?;
                validate_identity("工具名", tool)?;
                validate_created_at(created_at)?;
                if *success != (*status == ConversationTraceToolResultStatus::Succeeded) {
                    return Err(AgentError::new(
                        "上下文连续性工具结果的 success/status 不一致。",
                    ));
                }
                Ok(())
            }
        }
    }
}

impl ContextContinuityText {
    fn validate(&self) -> AgentResult<()> {
        if self.content_revision.trim().is_empty()
            || self.text.chars().count() > self.total_chars
            || self.truncated != (self.text.chars().count() < self.total_chars)
        {
            return Err(AgentError::new("上下文连续性消息文本投影无效。"));
        }
        Ok(())
    }
}

struct PendingToolCall {
    cursor: ContextJournalCursor,
    run_id: String,
    created_at: i64,
    call_id: String,
    tool: String,
    operation: Value,
    approval_status: AgentApprovalStatus,
    truncated: bool,
}

struct MetadataProjection {
    value: Value,
    truncated: bool,
}

fn project_text(content: &str, maximum_chars: usize) -> ContextContinuityText {
    let total_chars = content.chars().count();
    let (text, truncated) = truncate_chars(content, maximum_chars);
    ContextContinuityText {
        text,
        total_chars,
        content_revision: content_revision(content.as_bytes()),
        truncated,
    }
}

fn project_metadata(value: &Value) -> MetadataProjection {
    let mut truncated = false;
    let value = project_metadata_value(value, None, 0, &mut truncated).unwrap_or(Value::Null);
    MetadataProjection { value, truncated }
}

fn project_metadata_value(
    value: &Value,
    key: Option<&str>,
    depth: usize,
    truncated: &mut bool,
) -> Option<Value> {
    if key.is_some_and(is_payload_key) {
        *truncated = true;
        return None;
    }
    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) => Some(value.clone()),
        Value::String(value) => {
            let (value, item_truncated) = truncate_chars(value, METADATA_STRING_CHARS);
            *truncated |= item_truncated;
            Some(Value::String(value))
        }
        Value::Array(values) => {
            if depth >= METADATA_MAX_DEPTH {
                *truncated = true;
                return None;
            }
            if values.len() > METADATA_MAX_ARRAY_ITEMS {
                *truncated = true;
            }
            let projected = values
                .iter()
                .take(METADATA_MAX_ARRAY_ITEMS)
                .filter_map(|value| project_metadata_value(value, None, depth + 1, truncated))
                .collect::<Vec<_>>();
            Some(Value::Array(projected))
        }
        Value::Object(values) => {
            if depth >= METADATA_MAX_DEPTH {
                *truncated = true;
                return None;
            }
            let mut keys = values.keys().collect::<Vec<_>>();
            keys.sort_unstable();
            if keys.len() > METADATA_MAX_OBJECT_FIELDS {
                *truncated = true;
            }
            let mut projected = Map::new();
            for key in keys.into_iter().take(METADATA_MAX_OBJECT_FIELDS) {
                if let Some(value) =
                    project_metadata_value(&values[key], Some(key), depth + 1, truncated)
                {
                    projected.insert(key.clone(), value);
                }
            }
            Some(Value::Object(projected))
        }
    }
}

fn is_payload_key(key: &str) -> bool {
    let canonical = key
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect::<String>();
    if matches!(
        canonical.as_str(),
        "contentchars"
            | "contentlength"
            | "charcount"
            | "totalchars"
            | "sizebytes"
            | "byteswritten"
            | "addedlines"
            | "deletedlines"
    ) {
        return false;
    }
    [
        "content",
        "body",
        "text",
        "output",
        "stdout",
        "stderr",
        "base64",
        "dataurl",
        "image",
        "thumbnail",
        "patch",
        "diff",
        "html",
        "markdown",
        "snippet",
        "answer",
        "tail",
    ]
    .iter()
    .any(|fragment| canonical.contains(fragment))
}

fn ensure_no_pending_call(pending: &Option<PendingToolCall>) -> AgentResult<()> {
    if pending.is_some() {
        Err(AgentError::new(
            "上下文连续性骨架不能拆分工具调用和工具结果。",
        ))
    } else {
        Ok(())
    }
}

fn validate_identity(label: &str, value: &str) -> AgentResult<()> {
    if value.trim().is_empty() {
        Err(AgentError::new(format!("上下文连续性记录缺少{label}。")))
    } else {
        Ok(())
    }
}

fn validate_created_at(value: &str) -> AgentResult<()> {
    if value.trim().is_empty() {
        Err(AgentError::new("上下文连续性记录缺少创建时间。"))
    } else {
        Ok(())
    }
}

fn cursor_key(cursor: &ContextJournalCursor) -> String {
    match cursor {
        ContextJournalCursor::Message { message_id } => format!("message:{message_id}"),
        ContextJournalCursor::TraceItem {
            assistant_message_id,
            sequence,
        } => format!("trace:{assistant_message_id}:{sequence}"),
    }
}

fn truncate_chars(value: &str, maximum_chars: usize) -> (String, bool) {
    let mut chars = value.chars();
    let truncated = chars.clone().nth(maximum_chars).is_some();
    let output = chars.by_ref().take(maximum_chars).collect::<String>();
    (output, truncated)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::{
        ContextCompactionGeneration, ContextCompactionSummary,
        CONTEXT_COMPACTION_SUMMARY_SCHEMA_VERSION,
    };
    use serde_json::json;

    fn source_items() -> Vec<ContextCompactionSourceItem> {
        vec![
            ContextCompactionSourceItem::Message {
                cursor: ContextJournalCursor::message("user-1"),
                role: "user".to_string(),
                content: "compare README.md with the previous revision".to_string(),
                created_at: 1_000,
                status: Some("sent".to_string()),
                terminal_status: None,
                terminal_error: None,
            },
            ContextCompactionSourceItem::TraceItem {
                cursor: ContextJournalCursor::trace_item("assistant-1", 0),
                run_id: "run-1".to_string(),
                created_at: 2_000,
                item: ConversationTurnTraceItem::ToolCall {
                    sequence: 0,
                    call_id: "call-1".to_string(),
                    tool: "read_file".to_string(),
                    operation: json!({ "path": "README.md" }),
                    approval_status: AgentApprovalStatus::NotRequired,
                    truncated: false,
                },
            },
            ContextCompactionSourceItem::TraceItem {
                cursor: ContextJournalCursor::trace_item("assistant-1", 1),
                run_id: "run-1".to_string(),
                created_at: 2_000,
                item: ConversationTurnTraceItem::ToolResult {
                    sequence: 1,
                    call_id: "call-1".to_string(),
                    tool: "read_file".to_string(),
                    status: ConversationTraceToolResultStatus::Succeeded,
                    success: true,
                    observation: json!({
                        "path": "README.md",
                        "revision": "v1-exact",
                        "startLine": 1,
                        "endLine": 50,
                        "totalLines": 200,
                        "truncated": true,
                        "content": "large body that must not enter the skeleton"
                    }),
                    approval_status: AgentApprovalStatus::NotRequired,
                    error: None,
                    truncated: false,
                },
            },
        ]
    }

    #[test]
    fn projects_exact_metadata_without_large_payloads() {
        let prefix = ContextCompactionPrefix {
            conversation_id: "conversation-1".to_string(),
            source_revision: "source-1".to_string(),
            covered_through: ContextJournalCursor::trace_item("assistant-1", 1),
            previous_summary: None,
            source_items: source_items(),
        };

        let snapshot = ContextContinuitySnapshot::from_prefix(&prefix).unwrap();
        let rendered = snapshot.render_json().unwrap();

        assert!(rendered.contains("v1-exact"));
        assert!(rendered.contains("README.md"));
        assert!(rendered.contains("totalLines"));
        assert!(!rendered.contains("large body"));
        assert_eq!(snapshot.covered_through, prefix.covered_through);
        let ContextContinuityEntry::UserMessage {
            created_at, text, ..
        } = &snapshot.entries[0]
        else {
            panic!("first continuity entry should be the original user message");
        };
        assert_eq!(created_at, &format_message_created_at(1_000).unwrap());
        assert_eq!(text.text, "compare README.md with the previous revision");
        assert!(!text.truncated);
    }

    #[test]
    fn recursive_projection_keeps_existing_records_and_appends_once() {
        let first_prefix = ContextCompactionPrefix {
            conversation_id: "conversation-1".to_string(),
            source_revision: "source-1".to_string(),
            covered_through: ContextJournalCursor::trace_item("assistant-1", 1),
            previous_summary: None,
            source_items: source_items(),
        };
        let continuity = ContextContinuitySnapshot::from_prefix(&first_prefix).unwrap();
        let previous = ContextCompactionSummary {
            schema_version: CONTEXT_COMPACTION_SUMMARY_SCHEMA_VERSION,
            id: "summary-1".to_string(),
            conversation_id: "conversation-1".to_string(),
            source_revision: "source-1".to_string(),
            previous_summary_id: None,
            covered_through: first_prefix.covered_through.clone(),
            content: "previous summary".to_string(),
            continuity,
            generation: ContextCompactionGeneration::test(),
            source_input_tokens: 100,
            summary_input_tokens: 20,
            continuity_input_tokens: 20,
            replacement_input_tokens: 40,
            created_at: 1,
        };
        let final_cursor = ContextJournalCursor::message("assistant-2");
        let next_prefix = ContextCompactionPrefix {
            conversation_id: "conversation-1".to_string(),
            source_revision: "source-2".to_string(),
            covered_through: final_cursor.clone(),
            previous_summary: Some(previous),
            source_items: vec![ContextCompactionSourceItem::Message {
                cursor: final_cursor,
                role: "assistant".to_string(),
                content: "done".to_string(),
                created_at: 3_000,
                status: Some("sent".to_string()),
                terminal_status: Some(ConversationTurnTraceTerminalStatus::Completed),
                terminal_error: None,
            }],
        };

        let snapshot = ContextContinuitySnapshot::from_prefix(&next_prefix).unwrap();

        assert_eq!(snapshot.entries.len(), 3);
        snapshot.validate().unwrap();
    }
}
