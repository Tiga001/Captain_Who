//! Read-only access to the authoritative conversation journal.
//!
//! Compaction summaries and continuity records are retrieval aids. Messages and committed trace
//! items remain the source of truth and can be searched or paged back into one agent run through
//! this repository. Every query is scoped by `conversation_id` at the SQL boundary.

use crate::context::format_message_created_at;
use rusqlite::types::Type;
use rusqlite::{params, params_from_iter, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::{Error as IoError, ErrorKind};

const MAX_TIMELINE_RECORDS: usize = 100;

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
    Archive {
        archive_ref: String,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    pub preview: String,
    pub preview_truncated: bool,
}

#[derive(Debug, Clone, Default)]
pub struct ConversationHistorySearchFilter {
    pub include_messages: bool,
    pub include_trace_items: bool,
    pub include_archives: bool,
    pub tool: Option<String>,
    pub exclude_tool: Option<String>,
    pub status: Option<String>,
    pub run_id: Option<String>,
    pub exclude_run_id: Option<String>,
    pub created_at_from: Option<i64>,
    pub created_at_to: Option<i64>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ConversationHistoryTimelineRecord {
    #[serde(rename = "ref")]
    pub reference: ConversationHistoryRecordRef,
    pub created_at: String,
    pub record_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
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

pub fn search_records(
    connection: &Connection,
    conversation_id: &str,
    query: &str,
    filter: &ConversationHistorySearchFilter,
    limit: usize,
) -> rusqlite::Result<Vec<ConversationHistorySearchHit>> {
    let query = query.trim();
    let use_match = query.chars().count() >= 3;
    let mut sql = String::from(
        "SELECT
            record_type, item_kind, message_id, assistant_message_id, sequence,
            archive_ref, tool, status, run_id, created_at,
            snippet(conversation_history_fts, 15, '', '', '…', 64),
            length(content)
         FROM conversation_history_fts
         WHERE conversation_id = ?",
    );
    let mut values = Vec::<rusqlite::types::Value>::new();
    values.push(conversation_id.to_string().into());
    if use_match {
        sql.push_str(" AND conversation_history_fts MATCH ?");
        values.push(fts_phrase(query).into());
    } else {
        sql.push_str(" AND content LIKE ? ESCAPE '\\'");
        values.push(like_contains_pattern(query).into());
    }
    append_record_type_filter(&mut sql, &mut values, filter);
    append_optional_filter(&mut sql, &mut values, "tool", filter.tool.as_deref());
    append_optional_exclusion(
        &mut sql,
        &mut values,
        "tool",
        filter.exclude_tool.as_deref(),
    );
    append_optional_filter(&mut sql, &mut values, "status", filter.status.as_deref());
    append_optional_filter(&mut sql, &mut values, "run_id", filter.run_id.as_deref());
    append_optional_exclusion(
        &mut sql,
        &mut values,
        "run_id",
        filter.exclude_run_id.as_deref(),
    );
    if let Some(from) = filter.created_at_from {
        sql.push_str(" AND created_at >= ?");
        values.push(from.into());
    }
    if let Some(to) = filter.created_at_to {
        sql.push_str(" AND created_at <= ?");
        values.push(to.into());
    }
    sql.push_str(
        " ORDER BY bm25(conversation_history_fts), position, within_message_order LIMIT ?",
    );
    values.push(i64::try_from(limit).unwrap_or(i64::MAX).into());

    let mut statement = connection.prepare(&sql)?;
    let hits = statement
        .query_map(params_from_iter(values), |row| {
            let record_type = row.get::<_, String>(0)?;
            let message_id = row.get::<_, Option<String>>(2)?;
            let assistant_message_id = row.get::<_, Option<String>>(3)?;
            let sequence = row.get::<_, Option<u64>>(4)?;
            let archive_ref = row.get::<_, Option<String>>(5)?;
            let reference = reference_from_columns(
                &record_type,
                message_id,
                assistant_message_id,
                sequence,
                archive_ref,
            )?;
            let preview = row.get::<_, String>(10)?;
            let content_length = row.get::<_, usize>(11)?;
            let role = match &reference {
                ConversationHistoryRecordRef::Message { message_id } => connection
                    .query_row(
                        "SELECT role FROM messages WHERE conversation_id = ?1 AND id = ?2",
                        params![conversation_id, message_id],
                        |role_row| role_row.get::<_, String>(0),
                    )
                    .optional()?,
                _ => None,
            };
            Ok(ConversationHistorySearchHit {
                reference,
                created_at: format_created_at(row.get(9)?)?,
                record_type,
                role,
                item_kind: row.get(1)?,
                tool: row.get(6)?,
                status: row.get(7)?,
                run_id: row.get(8)?,
                preview_truncated: preview.chars().count() < content_length,
                preview,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(hits)
}

fn append_record_type_filter(
    sql: &mut String,
    values: &mut Vec<rusqlite::types::Value>,
    filter: &ConversationHistorySearchFilter,
) {
    let kinds = [
        (filter.include_messages, "message"),
        (filter.include_trace_items, "trace_item"),
        (filter.include_archives, "archive"),
    ]
    .into_iter()
    .filter_map(|(included, kind)| included.then_some(kind))
    .collect::<Vec<_>>();
    sql.push_str(" AND record_type IN (");
    for (index, kind) in kinds.iter().enumerate() {
        if index > 0 {
            sql.push(',');
        }
        sql.push('?');
        values.push((*kind).to_string().into());
    }
    sql.push(')');
}

fn append_optional_filter(
    sql: &mut String,
    values: &mut Vec<rusqlite::types::Value>,
    column: &str,
    value: Option<&str>,
) {
    if let Some(value) = value {
        sql.push_str(" AND ");
        sql.push_str(column);
        sql.push_str(" = ?");
        values.push(value.to_string().into());
    }
}

fn append_optional_exclusion(
    sql: &mut String,
    values: &mut Vec<rusqlite::types::Value>,
    column: &str,
    value: Option<&str>,
) {
    if let Some(value) = value {
        sql.push_str(" AND (");
        sql.push_str(column);
        sql.push_str(" IS NULL OR ");
        sql.push_str(column);
        sql.push_str(" != ?)");
        values.push(value.to_string().into());
    }
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
        ConversationHistoryRecordRef::Archive { archive_ref } => connection
            .query_row(
                "SELECT archive_ref, assistant_message_id, sequence, call_id, tool,
                        content_type, content_hash, uncompressed_bytes, uncompressed_chars,
                        truncated_at_source, archived_completely, created_at
                 FROM conversation_history_blobs
                 WHERE conversation_id = ?1 AND archive_ref = ?2",
                params![conversation_id, archive_ref],
                |row| {
                    let created_at = row.get::<_, i64>(11)?;
                    let created_at_text = format_created_at(created_at)?;
                    let serialized_json = serialize_record(json!({
                        "kind": "archive",
                        "archiveRef": row.get::<_, String>(0)?,
                        "assistantMessageId": row.get::<_, String>(1)?,
                        "sequence": row.get::<_, u64>(2)?,
                        "callId": row.get::<_, String>(3)?,
                        "tool": row.get::<_, String>(4)?,
                        "contentType": row.get::<_, String>(5)?,
                        "contentHash": row.get::<_, String>(6)?,
                        "totalBytes": row.get::<_, u64>(7)?,
                        "totalChars": row.get::<_, u64>(8)?,
                        "truncatedAtSource": row.get::<_, bool>(9)?,
                        "archivedCompletely": row.get::<_, bool>(10)?,
                        "createdAt": created_at_text,
                        "createdAtUnixMs": created_at,
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

pub fn records_around(
    connection: &Connection,
    conversation_id: &str,
    reference: &ConversationHistoryRecordRef,
    before: usize,
    after: usize,
) -> rusqlite::Result<Option<Vec<ConversationHistoryTimelineRecord>>> {
    let Some((position, within)) = reference_order(connection, conversation_id, reference)? else {
        return Ok(None);
    };
    let before = before.min(MAX_TIMELINE_RECORDS);
    let after = after.min(MAX_TIMELINE_RECORDS);
    let mut records = query_timeline(
        connection,
        conversation_id,
        Some((position, within)),
        None,
        before,
        true,
    )?;
    records.reverse();
    let mut tail = query_timeline(
        connection,
        conversation_id,
        Some((position, within)),
        None,
        after.saturating_add(1),
        false,
    )?;
    records.append(&mut tail);
    Ok(Some(records))
}

pub fn records_in_range(
    connection: &Connection,
    conversation_id: &str,
    start: &ConversationHistoryRecordRef,
    end: &ConversationHistoryRecordRef,
    limit: usize,
) -> rusqlite::Result<Option<Vec<ConversationHistoryTimelineRecord>>> {
    let Some(start_order) = reference_order(connection, conversation_id, start)? else {
        return Ok(None);
    };
    let Some(end_order) = reference_order(connection, conversation_id, end)? else {
        return Ok(None);
    };
    let (start_order, end_order) = if start_order <= end_order {
        (start_order, end_order)
    } else {
        (end_order, start_order)
    };
    query_timeline(
        connection,
        conversation_id,
        Some(start_order),
        Some(end_order),
        limit.min(MAX_TIMELINE_RECORDS),
        false,
    )
    .map(Some)
}

pub fn get_tool_exchange(
    connection: &Connection,
    conversation_id: &str,
    reference: Option<&ConversationHistoryRecordRef>,
    call_id: Option<&str>,
    run_id: Option<&str>,
) -> rusqlite::Result<Option<Vec<ConversationHistoryRecord>>> {
    let identity = match reference {
        Some(ConversationHistoryRecordRef::TraceItem {
            assistant_message_id,
            sequence,
        }) => connection
            .query_row(
                "SELECT json_extract(item_json, '$.callId')
                 FROM conversation_turn_trace_items AS item
                 INNER JOIN conversation_turn_traces AS trace
                    ON trace.assistant_message_id = item.assistant_message_id
                 WHERE trace.conversation_id = ?1
                   AND item.assistant_message_id = ?2
                   AND item.sequence = ?3",
                params![conversation_id, assistant_message_id, sequence],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten()
            .map(|call_id| (assistant_message_id.clone(), call_id)),
        Some(ConversationHistoryRecordRef::Archive { archive_ref }) => connection
            .query_row(
                "SELECT assistant_message_id, call_id
                 FROM conversation_history_blobs
                 WHERE conversation_id = ?1 AND archive_ref = ?2",
                params![conversation_id, archive_ref],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?,
        Some(ConversationHistoryRecordRef::Message { .. }) => None,
        None => {
            let Some(call_id) = call_id else {
                return Ok(None);
            };
            connection
                .query_row(
                    "SELECT item.assistant_message_id, json_extract(item.item_json, '$.callId')
                     FROM conversation_turn_trace_items AS item
                     INNER JOIN conversation_turn_traces AS trace
                        ON trace.assistant_message_id = item.assistant_message_id
                     WHERE trace.conversation_id = ?1
                       AND json_extract(item.item_json, '$.callId') = ?2
                       AND (?3 IS NULL OR trace.run_id = ?3)
                     ORDER BY trace.created_at DESC, item.sequence DESC
                     LIMIT 1",
                    params![conversation_id, call_id, run_id],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )
                .optional()?
        }
    };
    let Some((assistant_message_id, call_id)) = identity else {
        return Ok(None);
    };
    let refs = {
        let mut statement = connection.prepare(
            "SELECT item.sequence
             FROM conversation_turn_trace_items AS item
             INNER JOIN conversation_turn_traces AS trace
                ON trace.assistant_message_id = item.assistant_message_id
             WHERE trace.conversation_id = ?1
               AND item.assistant_message_id = ?2
               AND json_extract(item.item_json, '$.callId') = ?3
             ORDER BY item.sequence ASC",
        )?;
        let refs = statement
            .query_map(
                params![conversation_id, &assistant_message_id, &call_id],
                |row| {
                    Ok(ConversationHistoryRecordRef::TraceItem {
                        assistant_message_id: assistant_message_id.clone(),
                        sequence: row.get(0)?,
                    })
                },
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        refs
    };
    let mut records = Vec::with_capacity(refs.len());
    for reference in refs {
        let record = read_record(connection, conversation_id, &reference)?
            .ok_or_else(|| invalid_history_data("tool exchange record disappeared"))?;
        records.push(record);
    }
    Ok(Some(records))
}

fn reference_order(
    connection: &Connection,
    conversation_id: &str,
    reference: &ConversationHistoryRecordRef,
) -> rusqlite::Result<Option<(i64, i64)>> {
    let ref_key = reference_key(reference);
    connection
        .query_row(
            "SELECT position, within_message_order
             FROM conversation_history_fts
             WHERE conversation_id = ?1 AND ref_key = ?2",
            params![conversation_id, ref_key],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
}

fn query_timeline(
    connection: &Connection,
    conversation_id: &str,
    start: Option<(i64, i64)>,
    end: Option<(i64, i64)>,
    limit: usize,
    backwards: bool,
) -> rusqlite::Result<Vec<ConversationHistoryTimelineRecord>> {
    let mut sql = String::from(
        "SELECT
            record_type, item_kind, message_id, assistant_message_id, sequence,
            archive_ref, tool, status, run_id, created_at,
            substr(content, 1, 321), length(content)
         FROM conversation_history_fts
         WHERE conversation_id = ? AND record_type != 'archive'",
    );
    let mut values = vec![rusqlite::types::Value::from(conversation_id.to_string())];
    if let Some((position, within)) = start {
        if backwards {
            sql.push_str(" AND (position < ? OR (position = ? AND within_message_order < ?))");
        } else {
            sql.push_str(" AND (position > ? OR (position = ? AND within_message_order >= ?))");
        }
        values.extend([position.into(), position.into(), within.into()]);
    }
    if let Some((position, within)) = end {
        sql.push_str(" AND (position < ? OR (position = ? AND within_message_order <= ?))");
        values.extend([position.into(), position.into(), within.into()]);
    }
    if backwards {
        sql.push_str(" ORDER BY position DESC, within_message_order DESC");
    } else {
        sql.push_str(" ORDER BY position ASC, within_message_order ASC");
    }
    sql.push_str(" LIMIT ?");
    values.push(i64::try_from(limit).unwrap_or(i64::MAX).into());

    let mut statement = connection.prepare(&sql)?;
    let records = statement
        .query_map(params_from_iter(values), |row| {
            let record_type = row.get::<_, String>(0)?;
            let preview = row.get::<_, String>(10)?;
            let content_length = row.get::<_, usize>(11)?;
            Ok(ConversationHistoryTimelineRecord {
                reference: reference_from_columns(
                    &record_type,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                )?,
                created_at: format_created_at(row.get(9)?)?,
                record_type,
                item_kind: row.get(1)?,
                tool: row.get(6)?,
                status: row.get(7)?,
                run_id: row.get(8)?,
                preview_truncated: content_length > preview.chars().count(),
                preview,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(records)
}

fn reference_from_columns(
    record_type: &str,
    message_id: Option<String>,
    assistant_message_id: Option<String>,
    sequence: Option<u64>,
    archive_ref: Option<String>,
) -> rusqlite::Result<ConversationHistoryRecordRef> {
    match record_type {
        "message" => Ok(ConversationHistoryRecordRef::Message {
            message_id: message_id
                .ok_or_else(|| invalid_history_data("FTS message is missing message_id"))?,
        }),
        "trace_item" => Ok(ConversationHistoryRecordRef::TraceItem {
            assistant_message_id: assistant_message_id.ok_or_else(|| {
                invalid_history_data("FTS trace item is missing message identity")
            })?,
            sequence: sequence
                .ok_or_else(|| invalid_history_data("FTS trace item is missing sequence"))?,
        }),
        "archive" => Ok(ConversationHistoryRecordRef::Archive {
            archive_ref: archive_ref
                .ok_or_else(|| invalid_history_data("FTS archive is missing archive_ref"))?,
        }),
        _ => Err(invalid_history_data("FTS record has unknown type")),
    }
}

fn reference_key(reference: &ConversationHistoryRecordRef) -> String {
    match reference {
        ConversationHistoryRecordRef::Message { message_id } => format!("message:{message_id}"),
        ConversationHistoryRecordRef::TraceItem {
            assistant_message_id,
            sequence,
        } => format!("trace:{assistant_message_id}:{sequence}"),
        ConversationHistoryRecordRef::Archive { archive_ref } => {
            format!("archive:{archive_ref}")
        }
    }
}

fn invalid_history_data(message: impl Into<String>) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        0,
        Type::Text,
        Box::new(IoError::new(ErrorKind::InvalidData, message.into())),
    )
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

fn fts_phrase(query: &str) -> String {
    format!("\"{}\"", query.replace('"', "\"\""))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{
        conversation_history_archive_repository, conversation_trace_repository, migrations,
    };
    use crate::{
        AgentApprovalStatus, ConversationTraceToolResultStatus, ConversationTurnTrace,
        ConversationTurnTraceItem, ConversationTurnTraceTerminalStatus,
        CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
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
                    provenance: None,
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
                    archive: Default::default(),
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
        let filter = ConversationHistorySearchFilter {
            include_messages: true,
            include_trace_items: true,
            include_archives: true,
            ..Default::default()
        };
        let messages = search_records(&connection, "conversation-1", "09:02", &filter, 20).unwrap();
        assert_eq!(messages.len(), 1);
        assert!(matches!(
            messages[0].reference,
            ConversationHistoryRecordRef::Message { ref message_id } if message_id == "user-1"
        ));

        let trace = search_records(&connection, "conversation-1", "v1-exact", &filter, 20).unwrap();
        assert_eq!(trace.len(), 1);
        assert_eq!(trace[0].tool.as_deref(), Some("read_file"));
    }

    #[test]
    fn search_exclusions_are_applied_before_the_result_limit() {
        let connection = setup();
        let exclude_tool = ConversationHistorySearchFilter {
            include_trace_items: true,
            exclude_tool: Some("read_file".to_string()),
            ..Default::default()
        };
        assert!(
            search_records(&connection, "conversation-1", "v1-exact", &exclude_tool, 1)
                .unwrap()
                .is_empty()
        );

        let exclude_run = ConversationHistorySearchFilter {
            include_trace_items: true,
            exclude_run_id: Some("run-1".to_string()),
            ..Default::default()
        };
        assert!(
            search_records(&connection, "conversation-1", "v1-exact", &exclude_run, 1)
                .unwrap()
                .is_empty()
        );

        let messages_remain_visible = ConversationHistorySearchFilter {
            include_messages: true,
            exclude_tool: Some("conversation_history".to_string()),
            exclude_run_id: Some("run-1".to_string()),
            ..Default::default()
        };
        assert_eq!(
            search_records(
                &connection,
                "conversation-1",
                "first exact request",
                &messages_remain_visible,
                1
            )
            .unwrap()
            .len(),
            1
        );
    }

    #[test]
    fn trigram_search_finds_contiguous_cjk_text() {
        let connection = setup();
        connection
            .execute(
                "UPDATE messages
                 SET content = '请修复审批结束后的引导问题'
                 WHERE conversation_id = 'conversation-1' AND id = 'user-1'",
                [],
            )
            .unwrap();
        let filter = ConversationHistorySearchFilter {
            include_messages: true,
            ..Default::default()
        };

        let hits = search_records(
            &connection,
            "conversation-1",
            "审批结束后的引导",
            &filter,
            20,
        )
        .unwrap();

        assert_eq!(hits.len(), 1);
        assert!(matches!(
            hits[0].reference,
            ConversationHistoryRecordRef::Message { ref message_id } if message_id == "user-1"
        ));
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

    #[test]
    fn fts_finds_exact_archive_phrase_with_structured_filters() {
        let mut connection = setup();
        let archived = conversation_history_archive_repository::store_archive(
            &mut connection,
            &conversation_history_archive_repository::ConversationHistoryArchiveInput {
                conversation_id: "conversation-1".to_string(),
                assistant_message_id: "assistant-1".to_string(),
                sequence: 1,
                call_id: "call-1".to_string(),
                tool: "read_file".to_string(),
                content_type: "application/json".to_string(),
                content: r#"{"content":"压缩后仍可检索的精确短语 exact-archive-needle"}"#
                    .to_string(),
                truncated_at_source: false,
                model_projection_truncated: true,
                archive_projection_truncated: false,
                created_at: 2_000,
            },
        )
        .unwrap();
        connection
            .execute(
                "DELETE FROM conversation_history_fts WHERE archive_ref = ?1",
                [&archived.archive_ref],
            )
            .unwrap();
        migrations::run_migrations(&connection).unwrap();
        let filter = ConversationHistorySearchFilter {
            include_archives: true,
            tool: Some("read_file".to_string()),
            status: Some("succeeded".to_string()),
            run_id: Some("run-1".to_string()),
            created_at_from: Some(1_500),
            created_at_to: Some(2_500),
            ..Default::default()
        };

        let hits = search_records(
            &connection,
            "conversation-1",
            "exact-archive-needle",
            &filter,
            20,
        )
        .unwrap();

        assert_eq!(hits.len(), 1);
        assert_eq!(
            hits[0].reference,
            ConversationHistoryRecordRef::Archive {
                archive_ref: archived.archive_ref
            }
        );
        assert!(search_records(
            &connection,
            "conversation-2",
            "exact-archive-needle",
            &filter,
            20
        )
        .unwrap()
        .is_empty());
    }

    #[test]
    fn around_range_and_tool_exchange_restore_order_without_archive_duplicates() {
        let connection = setup();
        let result_ref = ConversationHistoryRecordRef::TraceItem {
            assistant_message_id: "assistant-1".to_string(),
            sequence: 1,
        };

        let around = records_around(&connection, "conversation-1", &result_ref, 2, 2)
            .unwrap()
            .unwrap();
        assert_eq!(around.len(), 4);
        assert_eq!(around[2].reference, result_ref);
        assert_eq!(around[0].record_type, "message");
        assert_eq!(around[1].item_kind.as_deref(), Some("tool_call"));
        assert_eq!(around[3].record_type, "message");

        let range = records_in_range(
            &connection,
            "conversation-1",
            &ConversationHistoryRecordRef::Message {
                message_id: "user-1".to_string(),
            },
            &ConversationHistoryRecordRef::Message {
                message_id: "assistant-1".to_string(),
            },
            20,
        )
        .unwrap()
        .unwrap();
        assert_eq!(range.len(), 4);

        let exchange =
            get_tool_exchange(&connection, "conversation-1", Some(&result_ref), None, None)
                .unwrap()
                .unwrap();
        assert_eq!(exchange.len(), 2);
        assert!(exchange[0]
            .serialized_json
            .contains("\"type\":\"tool_call\""));
        assert!(exchange[1]
            .serialized_json
            .contains("\"type\":\"tool_result\""));
    }
}
