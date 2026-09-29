//! A single read snapshot of the model-visible prefix hidden by an adopted summary.
//!
//! The compaction journal supplies ordering/ownership only. Audit trace previews and full output
//! archives are deliberately not used as message bodies or as a second search corpus.

use super::{ConversationHistoryRecord, ConversationHistoryRecordRef};
use crate::context::{
    format_message_created_at, terminal_record_needed, ContextCompactionSourceItem,
};
use crate::storage::{
    agent_message_model_projection, context_compaction_repository,
    conversation_model_context_repository, conversation_trace_repository,
};
use crate::{ConversationModelContextItem, ConversationTurnTraceItem};
use rusqlite::Connection;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone)]
pub(crate) struct CompactedHistoryEntry {
    pub record: ConversationHistoryRecord,
    pub turn_id: String,
    pub role: String,
    pub item_kind: Option<String>,
    pub tool: Option<String>,
    pub call_id: Option<String>,
    pub status: Option<String>,
    pub search_text: String,
}

#[derive(Debug)]
pub(crate) struct CompactedHistorySnapshot {
    pub entries: Vec<CompactedHistoryEntry>,
}

pub(crate) fn read_compacted_snapshot(
    connection: &Connection,
    conversation_id: &str,
    summary_id: &str,
) -> Result<CompactedHistorySnapshot, String> {
    let prefix = context_compaction_repository::read_adopted_summary_prefix(
        connection,
        conversation_id,
        summary_id,
    )
    .map_err(|error| error.to_string())?;
    let covered_sequences = prefix
        .iter()
        .filter_map(|source| match source {
            ContextCompactionSourceItem::TraceItem { cursor, item, .. } => {
                Some((cursor.message_id().to_string(), item.sequence()))
            }
            _ => None,
        })
        .collect::<HashSet<_>>();
    let mut models = HashMap::<String, Vec<ConversationModelContextItem>>::new();
    let mut entries = Vec::with_capacity(prefix.len());
    let mut turn_id = String::new();
    for source in prefix {
        // Workflow bootstrap may start with assistant-owned context material without a user
        // message. Its real owner is the grouping anchor; do not invent a user request.
        if turn_id.is_empty() {
            turn_id = source.cursor().message_id().to_string();
        }
        let (reference, created_at, role, item_kind, tool, call_id, status, messages) = match source
        {
            ContextCompactionSourceItem::Message {
                cursor,
                role,
                content,
                created_at,
                status,
                ..
            } => {
                let message_id = cursor.message_id().to_string();
                let content = if role == "user" {
                    turn_id.clone_from(&message_id);
                    agent_message_model_projection::project_message(
                        connection,
                        conversation_id,
                        &message_id,
                    )?
                    .unwrap_or(content)
                } else {
                    content
                };
                let mut messages = Vec::new();
                if role != "assistant" || !content.trim().is_empty() {
                    messages.push(json!({"role":role, "content":content}));
                }
                if role == "assistant" {
                    if let Some(trace) = conversation_trace_repository::get_trace_for_message(
                        connection,
                        &message_id,
                    )
                    .map_err(|error| error.to_string())?
                    {
                        if trace.terminal_status
                            != crate::ConversationTurnTraceTerminalStatus::InProgress
                            && terminal_record_needed(&trace, &content)
                        {
                            // Match ContextRenderer's terminal backend fact, which is owned by
                            // this message cursor and precedes any AfterMessage interruption.
                            let terminal = json!({
                                "recordType":"historical_agent_activity_terminal", "runId":trace.run_id,
                                "terminalStatus":trace.terminal_status, "terminalError":trace.terminal_error,
                                "traceTruncated":trace.truncated,
                            });
                            messages.push(json!({"role":"system", "placement":"backend_state", "content":terminal.to_string()}));
                        }
                    }
                }
                (
                    ConversationHistoryRecordRef::Message { message_id },
                    created_at,
                    role,
                    None,
                    None,
                    None,
                    status,
                    messages,
                )
            }
            ContextCompactionSourceItem::TraceItem {
                cursor,
                item,
                created_at,
                ..
            } => {
                let assistant_id = cursor.message_id().to_string();
                if !models.contains_key(&assistant_id) {
                    let log = conversation_model_context_repository::get_log_for_message(
                        connection,
                        &assistant_id,
                    )
                    .map_err(|error| error.to_string())?
                    .ok_or_else(|| {
                        "压缩历史缺少持久模型上下文，无法精确回顾；不会以审计摘要代替原文。"
                            .to_string()
                    })?;
                    let trace = conversation_trace_repository::get_trace_for_message(
                        connection,
                        &assistant_id,
                    )
                    .map_err(|error| error.to_string())?
                    .ok_or_else(|| "压缩历史的 Trace 已不存在。".to_string())?;
                    crate::conversation_trace::validate_model_context_prefix(&trace, &log.items)?;
                    models.insert(assistant_id.clone(), log.items);
                }
                let log = &models[&assistant_id];
                // Mirror Context's ownership-based narration deduplication. A grouped Tool Call
                // already owns this text; equal arbitrary text is never used to infer ownership.
                if let ConversationTurnTraceItem::AssistantNarration {
                    first_tool_call_id: Some(first),
                    ..
                } = item.as_ref()
                {
                    if log.iter().any(|candidate| {
                        candidate.sequence > item.sequence()
                            && covered_sequences
                                .contains(&(assistant_id.clone(), candidate.sequence))
                            && candidate.role == "assistant"
                            && !candidate.content.is_empty()
                            && candidate.tool_calls.iter().any(|call| &call.id == first)
                    }) {
                        continue;
                    }
                }
                let selected = log
                    .iter()
                    .filter(|model| model.sequence == item.sequence())
                    .collect::<Vec<_>>();
                if selected.is_empty() {
                    return Err("压缩历史缺少对应的持久模型消息，无法精确回顾。".into());
                }
                let role = selected[0].role.clone();
                let (tool, call_id, status) = match item.as_ref() {
                    ConversationTurnTraceItem::ToolCall { tool, call_id, .. } => {
                        (Some(tool.clone()), Some(call_id.clone()), None)
                    }
                    ConversationTurnTraceItem::ToolResult {
                        tool,
                        call_id,
                        status,
                        ..
                    } => (
                        Some(tool.clone()),
                        Some(call_id.clone()),
                        Some(format!("{status:?}").to_ascii_lowercase()),
                    ),
                    _ => (None, None, None),
                };
                // Recall must not recursively retrieve its own previous pages.
                if tool.as_deref() == Some("conversation_history") {
                    continue;
                }
                let kind = serde_json::to_value(item.as_ref())
                    .map_err(|error| error.to_string())?
                    .get("type")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                let messages = selected.into_iter().map(model_message_value).collect();
                (
                    ConversationHistoryRecordRef::TraceItem {
                        assistant_message_id: assistant_id,
                        sequence: item.sequence(),
                    },
                    created_at,
                    role,
                    kind,
                    tool,
                    call_id,
                    status,
                    messages,
                )
            }
        };
        let search_text = messages
            .iter()
            .map(|message| {
                let mut text = message
                    .get("content")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                if let Some(calls) = message.get("toolCalls") {
                    text.push_str(&calls.to_string());
                }
                text
            })
            .collect::<Vec<_>>()
            .join("\n");
        let serialized_json = serde_json::to_string(&json!({"messages":messages}))
            .map_err(|error| error.to_string())?;
        entries.push(CompactedHistoryEntry {
            record: ConversationHistoryRecord {
                reference,
                created_at: format_message_created_at(created_at)
                    .map_err(|error| error.to_string())?,
                serialized_json,
            },
            turn_id: turn_id.clone(),
            role,
            item_kind,
            tool,
            call_id,
            status,
            search_text,
        });
    }
    Ok(CompactedHistorySnapshot { entries })
}

fn model_message_value(item: &ConversationModelContextItem) -> Value {
    let mut value = json!({"role":item.role, "content":item.content});
    if !item.images.is_empty() {
        value["images"] = json!(item.images);
    }
    if !item.tool_calls.is_empty() {
        value["toolCalls"] = Value::Array(
            item.tool_calls
                .iter()
                .map(|call| json!({"name":call.name, "args":call.args}))
                .collect(),
        );
    }
    if item.is_error {
        value["isError"] = Value::Bool(true);
    }
    value
}
