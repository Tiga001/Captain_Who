//! Read-only access to the authoritative conversation journal.
//!
//! Compaction summaries and continuity records are retrieval aids. Messages and committed trace
//! items remain the source of truth and can be searched or paged back into one agent run through
//! this repository. Every query is scoped by `conversation_id` at the SQL boundary.

use crate::context::format_message_created_at;
use crate::conversation_trace::ConversationTurnTraceItem;
use rusqlite::types::Type;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::{Error as IoError, ErrorKind};

const SEARCH_CANDIDATE_MULTIPLIER: usize = 2;
const SEARCH_PREVIEW_CHARS: usize = 320;
const SEARCH_PREVIEW_CONTEXT_BEFORE: usize = 80;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum ConversationHistoryRecordRef {
    Message {
        message_id: String,
    },
    TraceItem {
        assistant_message_id: String,
        sequence: u64,
    },
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ConversationHistorySearchHit {
    #[serde(rename = "ref")]
    pub reference: ConversationHistoryRecordRef,
    pub created_at: String,
    pub record_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    pub preview: String,
    pub preview_truncated: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ConversationHistoryRecord {
    #[serde(rename = "ref")]
    pub reference: ConversationHistoryRecordRef,
    pub created_at: String,
    pub serialized_json: String,
}

struct OrderedHit {
    position: i64,
    within_message_order: u64,
    hit: ConversationHistorySearchHit,
}

pub fn search_records(
    connection: &Connection,
    conversation_id: &str,
    query: &str,
    include_messages: bool,
    include_trace_items: bool,
    limit: usize,
) -> rusqlite::Result<Vec<ConversationHistorySearchHit>> {
    let candidate_limit = limit.saturating_mul(SEARCH_CANDIDATE_MULTIPLIER).max(limit);
    let pattern = like_contains_pattern(query);
    let mut hits = Vec::with_capacity(candidate_limit);

    if include_messages {
        let mut statement = connection.prepare(
            "SELECT id, role, content, created_at, position
             FROM messages
             WHERE conversation_id = ?1
               AND content <> ''
               AND lower(content) LIKE ?2 ESCAPE '\\'
             ORDER BY position ASC, created_at ASC, id ASC
             LIMIT ?3",
        )?;
        let rows = statement.query_map(
            params![conversation_id, &pattern, candidate_limit as i64],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            },
        )?;
        for row in rows {
            let (message_id, role, content, created_at, position) = row?;
            let (preview, preview_truncated) = make_preview(&content, query);
            hits.push(OrderedHit {
                position,
                within_message_order: if role == "assistant" { u64::MAX } else { 0 },
                hit: ConversationHistorySearchHit {
                    reference: ConversationHistoryRecordRef::Message { message_id },
                    created_at: format_created_at(created_at)?,
                    record_type: "message".to_string(),
                    role: Some(role),
                    item_kind: None,
                    tool: None,
                    preview,
                    preview_truncated,
                },
            });
        }
    }

    if include_trace_items {
        let mut statement = connection.prepare(
            "SELECT i.assistant_message_id, i.sequence, i.item_kind, i.item_json,
                    t.created_at, m.position
             FROM conversation_turn_trace_items i
             JOIN conversation_turn_traces t
               ON t.assistant_message_id = i.assistant_message_id
             JOIN messages m
               ON m.id = i.assistant_message_id
             WHERE t.conversation_id = ?1
               AND lower(i.item_json) LIKE ?2 ESCAPE '\\'
             ORDER BY m.position ASC, i.sequence ASC
             LIMIT ?3",
        )?;
        let rows = statement.query_map(
            params![conversation_id, &pattern, candidate_limit as i64],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, u64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            },
        )?;
        for row in rows {
            let (assistant_message_id, sequence, item_kind, item_json, created_at, position) = row?;
            let item = parse_trace_item(&item_json)?;
            let (preview, preview_truncated) = make_preview(&item_json, query);
            hits.push(OrderedHit {
                position,
                within_message_order: sequence,
                hit: ConversationHistorySearchHit {
                    reference: ConversationHistoryRecordRef::TraceItem {
                        assistant_message_id,
                        sequence,
                    },
                    created_at: format_created_at(created_at)?,
                    record_type: "trace_item".to_string(),
                    role: None,
                    item_kind: Some(item_kind),
                    tool: trace_item_tool(&item).map(str::to_string),
                    preview,
                    preview_truncated,
                },
            });
        }
    }

    hits.sort_by(|left, right| {
        left.position
            .cmp(&right.position)
            .then_with(|| left.within_message_order.cmp(&right.within_message_order))
    });
    hits.truncate(limit);
    Ok(hits.into_iter().map(|ordered| ordered.hit).collect())
}

pub fn read_record(
    connection: &Connection,
    conversation_id: &str,
    reference: &ConversationHistoryRecordRef,
) -> rusqlite::Result<Option<ConversationHistoryRecord>> {
    match reference {
        ConversationHistoryRecordRef::Message { message_id } => connection
            .query_row(
                "SELECT id, role, content, status, created_at, position
                 FROM messages
                 WHERE conversation_id = ?1 AND id = ?2",
                params![conversation_id, message_id],
                |row| {
                    let id = row.get::<_, String>(0)?;
                    let role = row.get::<_, String>(1)?;
                    let content = row.get::<_, String>(2)?;
                    let status = row.get::<_, Option<String>>(3)?;
                    let created_at = row.get::<_, i64>(4)?;
                    let position = row.get::<_, i64>(5)?;
                    let created_at_text = format_created_at(created_at)?;
                    let serialized_json = serialize_record(json!({
                        "kind": "message",
                        "messageId": id,
                        "role": role,
                        "content": content,
                        "status": status,
                        "createdAt": created_at_text,
                        "createdAtUnixMs": created_at,
                        "position": position,
                    }))?;
                    Ok(ConversationHistoryRecord {
                        reference: reference.clone(),
                        created_at: created_at_text,
                        serialized_json,
                    })
                },
            )
            .optional(),
        ConversationHistoryRecordRef::TraceItem {
            assistant_message_id,
            sequence,
        } => connection
            .query_row(
                "SELECT i.item_json, i.item_kind, t.run_id, t.terminal_status,
                        t.terminal_error, t.truncated, t.created_at, t.updated_at
                 FROM conversation_turn_trace_items i
                 JOIN conversation_turn_traces t
                   ON t.assistant_message_id = i.assistant_message_id
                 WHERE t.conversation_id = ?1
                   AND i.assistant_message_id = ?2
                   AND i.sequence = ?3",
                params![conversation_id, assistant_message_id, sequence],
                |row| {
                    let item_json = row.get::<_, String>(0)?;
                    let item_kind = row.get::<_, String>(1)?;
                    let run_id = row.get::<_, String>(2)?;
                    let terminal_status = row.get::<_, String>(3)?;
                    let terminal_error = row.get::<_, Option<String>>(4)?;
                    let trace_truncated = row.get::<_, bool>(5)?;
                    let created_at = row.get::<_, i64>(6)?;
                    let updated_at = row.get::<_, i64>(7)?;
                    let item = parse_trace_item_value(&item_json)?;
                    let created_at_text = format_created_at(created_at)?;
                    let serialized_json = serialize_record(json!({
                        "kind": "trace_item",
                        "assistantMessageId": assistant_message_id,
                        "sequence": sequence,
                        "itemKind": item_kind,
                        "runId": run_id,
                        "createdAt": created_at_text,
                        "createdAtUnixMs": created_at,
                        "committedAtUnixMs": updated_at,
                        "traceTerminalStatus": terminal_status,
                        "traceTerminalError": terminal_error,
                        "traceTruncated": trace_truncated,
                        "item": item,
                    }))?;
                    Ok(ConversationHistoryRecord {
                        reference: reference.clone(),
                        created_at: created_at_text,
                        serialized_json,
                    })
                },
            )
            .optional(),
    }
}

fn trace_item_tool(item: &ConversationTurnTraceItem) -> Option<&str> {
    match item {
        ConversationTurnTraceItem::ToolCall { tool, .. }
        | ConversationTurnTraceItem::ToolResult { tool, .. } => Some(tool),
        ConversationTurnTraceItem::AssistantNarration { .. } => None,
    }
}

fn parse_trace_item(value: &str) -> rusqlite::Result<ConversationTurnTraceItem> {
    serde_json::from_str(value)
        .map_err(|error| rusqlite::Error::FromSqlConversionFailure(0, Type::Text, Box::new(error)))
}

fn parse_trace_item_value(value: &str) -> rusqlite::Result<Value> {
    serde_json::from_str(value)
        .map_err(|error| rusqlite::Error::FromSqlConversionFailure(0, Type::Text, Box::new(error)))
}

fn serialize_record(value: Value) -> rusqlite::Result<String> {
    serde_json::to_string(&value)
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))
}

fn format_created_at(value: i64) -> rusqlite::Result<String> {
    format_message_created_at(value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            Type::Integer,
            Box::new(IoError::new(ErrorKind::InvalidData, error.to_string())),
        )
    })
}

fn like_contains_pattern(query: &str) -> String {
    let mut pattern = String::with_capacity(query.len() + 2);
    pattern.push('%');
    for character in query.to_lowercase().chars() {
        match character {
            '%' | '_' | '\\' => {
                pattern.push('\\');
                pattern.push(character);
            }
            _ => pattern.push(character),
        }
    }
    pattern.push('%');
    pattern
}

fn make_preview(content: &str, query: &str) -> (String, bool) {
    let normalized = content.split_whitespace().collect::<Vec<_>>().join(" ");
    let characters = normalized.chars().collect::<Vec<_>>();
    if characters.len() <= SEARCH_PREVIEW_CHARS {
        return (normalized, false);
    }
    let lowercase = normalized.to_lowercase().chars().collect::<Vec<_>>();
    let lowercase_query = query.to_lowercase().chars().collect::<Vec<_>>();
    let match_character = if lowercase_query.is_empty() {
        0
    } else {
        lowercase
            .windows(lowercase_query.len())
            .position(|window| window == lowercase_query)
            .unwrap_or(0)
    };
    let start = match_character.saturating_sub(SEARCH_PREVIEW_CONTEXT_BEFORE);
    let end = (start + SEARCH_PREVIEW_CHARS).min(characters.len());
    let mut preview = characters[start..end].iter().collect::<String>();
    if start > 0 {
        preview.insert(0, '…');
    }
    if end < characters.len() {
        preview.push('…');
    }
    (preview, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{conversation_trace_repository, migrations};
    use crate::{
        AgentApprovalStatus, ConversationTraceToolResultStatus, ConversationTurnTrace,
        ConversationTurnTraceTerminalStatus, CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
    };

    fn setup() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        migrations::run_migrations(&connection).unwrap();
        connection
            .execute_batch(
                "INSERT INTO conversations (
                    id, project_id, model_id, title, created_at, updated_at,
                    pinned_at, archived_at, unread_at
                 ) VALUES
                    ('conversation-1', NULL, NULL, 'One', 1, 1, NULL, NULL, NULL),
                    ('conversation-2', NULL, NULL, 'Two', 1, 1, NULL, NULL, NULL);
                 INSERT INTO messages (
                    id, conversation_id, role, content, status, agent_run_json,
                    ui_state_json, created_at, position
                 ) VALUES
                    ('user-1', 'conversation-1', 'user', 'first exact request at 09:02', 'sent', NULL, NULL, 1000, 0),
                    ('assistant-1', 'conversation-1', 'assistant', 'final answer', 'sent', NULL, NULL, 2000, 1),
                    ('other-user', 'conversation-2', 'user', 'first exact request at 09:02', 'sent', NULL, NULL, 1000, 0);",
            )
            .unwrap();
        let trace = ConversationTurnTrace {
            schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
            run_id: "run-1".to_string(),
            conversation_id: "conversation-1".to_string(),
            assistant_message_id: "assistant-1".to_string(),
            terminal_status: ConversationTurnTraceTerminalStatus::Completed,
            terminal_error: None,
            truncated: false,
            items: vec![
                ConversationTurnTraceItem::ToolCall {
                    sequence: 0,
                    call_id: "call-1".to_string(),
                    tool: "read_file".to_string(),
                    operation: json!({ "path": "README.md" }),
                    approval_status: AgentApprovalStatus::NotRequired,
                    truncated: false,
                },
                ConversationTurnTraceItem::ToolResult {
                    sequence: 1,
                    call_id: "call-1".to_string(),
                    tool: "read_file".to_string(),
                    status: ConversationTraceToolResultStatus::Succeeded,
                    success: true,
                    observation: json!({ "path": "README.md", "revision": "v1-exact" }),
                    approval_status: AgentApprovalStatus::NotRequired,
                    error: None,
                    truncated: false,
                },
            ],
        };
        conversation_trace_repository::commit_trace_in_connection(
            &connection,
            &trace,
            2_000,
            3_000,
        )
        .unwrap();
        connection
    }

    #[test]
    fn search_is_scoped_and_returns_message_and_trace_references() {
        let connection = setup();
        let messages =
            search_records(&connection, "conversation-1", "09:02", true, true, 20).unwrap();
        assert_eq!(messages.len(), 1);
        assert!(matches!(
            messages[0].reference,
            ConversationHistoryRecordRef::Message { ref message_id } if message_id == "user-1"
        ));

        let trace =
            search_records(&connection, "conversation-1", "v1-exact", true, true, 20).unwrap();
        assert_eq!(trace.len(), 1);
        assert_eq!(trace[0].tool.as_deref(), Some("read_file"));
    }

    #[test]
    fn reads_exact_authoritative_record_and_rejects_cross_conversation_reference() {
        let connection = setup();
        let reference = ConversationHistoryRecordRef::TraceItem {
            assistant_message_id: "assistant-1".to_string(),
            sequence: 1,
        };
        let record = read_record(&connection, "conversation-1", &reference)
            .unwrap()
            .unwrap();
        assert!(record.serialized_json.contains("v1-exact"));
        assert!(record.serialized_json.contains("README.md"));

        let other = ConversationHistoryRecordRef::Message {
            message_id: "other-user".to_string(),
        };
        assert!(read_record(&connection, "conversation-1", &other)
            .unwrap()
            .is_none());
    }
}
