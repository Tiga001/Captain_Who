//! Durable storage for the uncompressed, replay-safe conversation projection.
//!
//! These rows are not raw tool output and are not the audit trace. Each payload is a
//! provider-neutral, length-bounded message safe to survive a process restart. Process-only MCP
//! arguments may be redacted from this projection even while the current in-memory model loop
//! retains them for error correction.

use crate::conversation_trace::validate_model_context_prefix;
use crate::storage::conversation_trace_repository;
use crate::{ConversationModelContextItem, ConversationModelContextLog};
use rusqlite::types::Type;
use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};
use std::io::{Error as IoError, ErrorKind};

const ZSTD_LEVEL: i32 = 3;

pub(crate) fn commit_items_in_connection(
    connection: &Connection,
    conversation_id: &str,
    assistant_message_id: &str,
    items: &[ConversationModelContextItem],
) -> rusqlite::Result<bool> {
    validate_log_identity(connection, conversation_id, assistant_message_id)?;
    validate_items(items)?;
    let trace =
        conversation_trace_repository::get_trace_for_message(connection, assistant_message_id)?
            .ok_or_else(|| {
                invalid_input("model context item requires a committed conversation trace")
            })?;
    validate_model_context_prefix(&trace, items).map_err(invalid_input)?;
    let existing = load_items_for_message(connection, assistant_message_id)?;
    if existing.len() > items.len() || existing != items[..existing.len()] {
        let mismatch = existing
            .iter()
            .zip(items)
            .position(|(existing, incoming)| existing != incoming)
            .unwrap_or_else(|| items.len().min(existing.len()));
        let existing_identity = existing
            .get(mismatch)
            .map(|item| format!("{}:{}", item.sequence, item.ordinal))
            .unwrap_or_else(|| "missing".to_string());
        let incoming_identity = items
            .get(mismatch)
            .map(|item| format!("{}:{}", item.sequence, item.ordinal))
            .unwrap_or_else(|| "missing".to_string());
        let existing_hash = existing
            .get(mismatch)
            .map(model_item_hash)
            .transpose()?
            .unwrap_or_else(|| "missing".to_string());
        let incoming_hash = items
            .get(mismatch)
            .map(model_item_hash)
            .transpose()?
            .unwrap_or_else(|| "missing".to_string());
        return Err(invalid_input(format!(
            "model context updates must preserve every committed item as an exact prefix \
             (index={mismatch}, existing={existing_identity}@{existing_hash}, \
             incoming={incoming_identity}@{incoming_hash})"
        )));
    }
    if existing.len() == items.len() {
        return Ok(false);
    }
    for item in &items[existing.len()..] {
        let payload = serde_json::to_vec(item)
            .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
        let content_hash = format!("sha256:{:x}", Sha256::digest(&payload));
        let compressed =
            zstd::stream::encode_all(payload.as_slice(), ZSTD_LEVEL).map_err(|error| {
                invalid_data(format!("cannot compress model context item: {error}"))
            })?;
        connection.execute(
            "
            INSERT INTO conversation_model_context_items (
                assistant_message_id, sequence, ordinal, content_hash,
                uncompressed_bytes, compression, payload
            ) VALUES (?1, ?2, ?3, ?4, ?5, 'zstd', ?6)
            ",
            params![
                assistant_message_id,
                sqlite_integer(item.sequence)?,
                i64::from(item.ordinal),
                content_hash,
                sqlite_integer(payload.len() as u64)?,
                compressed,
            ],
        )?;
    }
    Ok(true)
}

fn model_item_hash(item: &ConversationModelContextItem) -> rusqlite::Result<String> {
    let payload = serde_json::to_vec(item)
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
    Ok(format!("sha256:{:x}", Sha256::digest(payload)))
}

pub(crate) fn list_logs_for_conversation(
    connection: &Connection,
    conversation_id: &str,
) -> rusqlite::Result<Vec<ConversationModelContextLog>> {
    let mut statement = connection.prepare(
        "
        SELECT DISTINCT model.assistant_message_id
        FROM conversation_model_context_items AS model
        INNER JOIN conversation_turn_traces AS trace
            ON trace.assistant_message_id = model.assistant_message_id
        INNER JOIN messages AS message
            ON message.id = trace.assistant_message_id
        WHERE trace.conversation_id = ?1
        ORDER BY message.position ASC, message.created_at ASC, model.assistant_message_id ASC
        ",
    )?;
    let assistant_message_ids = statement
        .query_map([conversation_id], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    assistant_message_ids
        .into_iter()
        .map(|assistant_message_id| {
            Ok(ConversationModelContextLog {
                items: load_items_for_message(connection, &assistant_message_id)?,
                assistant_message_id,
            })
        })
        .collect()
}

pub(crate) fn get_log_for_message(
    connection: &Connection,
    assistant_message_id: &str,
) -> rusqlite::Result<Option<ConversationModelContextLog>> {
    let exists = connection
        .query_row(
            "SELECT 1 FROM conversation_model_context_items
             WHERE assistant_message_id = ?1 LIMIT 1",
            [assistant_message_id],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    exists
        .then(|| {
            Ok(ConversationModelContextLog {
                assistant_message_id: assistant_message_id.to_string(),
                items: load_items_for_message(connection, assistant_message_id)?,
            })
        })
        .transpose()
}

fn validate_log_identity(
    connection: &Connection,
    conversation_id: &str,
    assistant_message_id: &str,
) -> rusqlite::Result<()> {
    let owner = connection
        .query_row(
            "SELECT conversation_id FROM conversation_turn_traces
             WHERE assistant_message_id = ?1",
            [assistant_message_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    match owner.as_deref() {
        Some(owner) if owner == conversation_id => Ok(()),
        Some(_) => Err(invalid_input(
            "model context item belongs to a different conversation",
        )),
        None => Err(invalid_input(
            "model context item requires a committed conversation trace",
        )),
    }
}

fn validate_items(items: &[ConversationModelContextItem]) -> rusqlite::Result<()> {
    let mut previous = None;
    for item in items {
        item.validate().map_err(invalid_input)?;
        let identity = (item.sequence, item.ordinal);
        if previous.is_some_and(|previous| identity <= previous) {
            return Err(invalid_input(
                "model context item identity must be strictly increasing",
            ));
        }
        previous = Some(identity);
    }
    Ok(())
}

fn load_items_for_message(
    connection: &Connection,
    assistant_message_id: &str,
) -> rusqlite::Result<Vec<ConversationModelContextItem>> {
    let mut statement = connection.prepare(
        "
        SELECT sequence, ordinal, content_hash, uncompressed_bytes, compression, payload
        FROM conversation_model_context_items
        WHERE assistant_message_id = ?1
        ORDER BY sequence ASC, ordinal ASC
        ",
    )?;
    let rows = statement
        .query_map([assistant_message_id], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Vec<u8>>(5)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut items = Vec::with_capacity(rows.len());
    for (sequence, ordinal, stored_hash, stored_bytes, compression, payload) in rows {
        if compression != "zstd" {
            return Err(corrupt_data(
                4,
                Type::Text,
                "unknown model context compression",
            ));
        }
        let decoded = zstd::stream::decode_all(payload.as_slice()).map_err(|error| {
            corrupt_data(
                5,
                Type::Blob,
                format!("cannot decompress model context item: {error}"),
            )
        })?;
        if i64::try_from(decoded.len()).ok() != Some(stored_bytes)
            || format!("sha256:{:x}", Sha256::digest(&decoded)) != stored_hash
        {
            return Err(corrupt_data(
                5,
                Type::Blob,
                "model context item failed length or hash validation",
            ));
        }
        let item =
            serde_json::from_slice::<ConversationModelContextItem>(&decoded).map_err(|error| {
                corrupt_data(
                    5,
                    Type::Blob,
                    format!("invalid model context item payload: {error}"),
                )
            })?;
        let stored_sequence = u64::try_from(sequence)
            .map_err(|error| corrupt_data(0, Type::Integer, error.to_string()))?;
        let stored_ordinal = u32::try_from(ordinal)
            .map_err(|error| corrupt_data(1, Type::Integer, error.to_string()))?;
        if item.sequence != stored_sequence || item.ordinal != stored_ordinal {
            return Err(corrupt_data(
                5,
                Type::Blob,
                "model context item identity does not match its payload",
            ));
        }
        item.validate()
            .map_err(|error| corrupt_data(5, Type::Blob, error))?;
        items.push(item);
    }
    Ok(items)
}

fn sqlite_integer(value: u64) -> rusqlite::Result<i64> {
    i64::try_from(value).map_err(|_| invalid_input("value exceeds SQLite INTEGER"))
}

fn invalid_input(message: impl Into<String>) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(Box::new(IoError::new(
        ErrorKind::InvalidInput,
        message.into(),
    )))
}

fn invalid_data(message: impl Into<String>) -> rusqlite::Error {
    corrupt_data(0, Type::Null, message)
}

fn corrupt_data(column: usize, data_type: Type, message: impl Into<String>) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        column,
        data_type,
        Box::new(IoError::new(ErrorKind::InvalidData, message.into())),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{AgentApprovalStatus, AgentContextCheckpointToolCall};
    use crate::storage::{conversation_trace_repository, migrations};
    use crate::{
        ConversationTraceToolResultStatus, ConversationTurnTrace, ConversationTurnTraceItem,
        ConversationTurnTraceTerminalStatus, CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
    };
    use serde_json::json;

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
                    created_at, position
                 ) VALUES
                    ('assistant-1', 'conversation-1', 'assistant', '', 'pending', NULL, 2, 0);",
            )
            .unwrap();
        let trace = ConversationTurnTrace {
            schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
            run_id: "run-model-context-1".to_string(),
            conversation_id: "conversation-1".to_string(),
            assistant_message_id: "assistant-1".to_string(),
            terminal_status: ConversationTurnTraceTerminalStatus::InProgress,
            terminal_error: None,
            truncated: true,
            items: vec![
                ConversationTurnTraceItem::ToolCall {
                    sequence: 0,
                    call_id: "call-1".to_string(),
                    tool: "conversation_history".to_string(),
                    provenance: crate::AgentToolIdentity::Builtin {
                        tool_name: "conversation_history".to_string(),
                    },
                    operation: json!({ "open": "hist_v1_turn" }),
                    approval_status: AgentApprovalStatus::NotRequired,
                    truncated: false,
                },
                ConversationTurnTraceItem::ToolResult {
                    sequence: 1,
                    call_id: "call-1".to_string(),
                    tool: "conversation_history".to_string(),
                    status: ConversationTraceToolResultStatus::Succeeded,
                    success: true,
                    observation: json!({
                        "returnedRecords": 1,
                        "historicalPayloadOmittedFromConversationTrace": true
                    }),
                    approval_status: AgentApprovalStatus::NotRequired,
                    error: None,
                    truncated: true,
                    archive: Default::default(),
                },
            ],
        };
        conversation_trace_repository::commit_trace_in_connection(&connection, &trace, 2, 3)
            .unwrap();
        connection
    }

    fn exact_items() -> Vec<ConversationModelContextItem> {
        vec![
            ConversationModelContextItem {
                images: Vec::new(),
                sequence: 0,
                ordinal: 0,
                role: "assistant".to_string(),
                content: String::new(),
                tool_call_id: None,
                tool_calls: vec![AgentContextCheckpointToolCall {
                    id: "call-1".to_string(),
                    name: "conversation_history".to_string(),
                    args: json!({ "open": "hist_v1_turn" }),
                    provider_identity: crate::AgentProviderToolCallIdentity {
                        provider_tool_index: 0,
                        provider_call_id: "call-1".to_string(),
                        runtime_call_id: "call-1".to_string(),
                    },
                }],
                is_error: false,
            },
            ConversationModelContextItem {
                images: Vec::new(),
                sequence: 1,
                ordinal: 0,
                role: "tool".to_string(),
                content: r#"{"content":"EXACT_HISTORY_BODY","ok":true}"#.to_string(),
                tool_call_id: Some("call-1".to_string()),
                tool_calls: Vec::new(),
                is_error: false,
            },
        ]
    }

    #[test]
    fn round_trips_compressed_exact_projection_and_enforces_append_only_identity() {
        let connection = setup();
        let items = exact_items();

        assert!(
            commit_items_in_connection(&connection, "conversation-1", "assistant-1", &items)
                .unwrap()
        );
        assert!(
            !commit_items_in_connection(&connection, "conversation-1", "assistant-1", &items)
                .unwrap()
        );

        let loaded = get_log_for_message(&connection, "assistant-1")
            .unwrap()
            .unwrap();
        assert_eq!(loaded.items, items);
        assert_eq!(
            list_logs_for_conversation(&connection, "conversation-1")
                .unwrap()
                .len(),
            1
        );
        assert!(list_logs_for_conversation(&connection, "conversation-2")
            .unwrap()
            .is_empty());

        let mut rewritten = items;
        rewritten[1].content = "rewritten".to_string();
        assert!(commit_items_in_connection(
            &connection,
            "conversation-1",
            "assistant-1",
            &rewritten
        )
        .is_err());
        assert!(commit_items_in_connection(
            &connection,
            "conversation-2",
            "assistant-1",
            &exact_items()
        )
        .is_err());
    }

    #[test]
    fn rejects_corrupt_payload_and_cascades_with_the_assistant_message() {
        let connection = setup();
        commit_items_in_connection(&connection, "conversation-1", "assistant-1", &exact_items())
            .unwrap();
        connection
            .execute(
                "UPDATE conversation_model_context_items
                 SET payload = X'000102'
                 WHERE assistant_message_id = 'assistant-1' AND sequence = 1",
                [],
            )
            .unwrap();
        assert!(get_log_for_message(&connection, "assistant-1").is_err());

        connection
            .execute("DELETE FROM messages WHERE id = 'assistant-1'", [])
            .unwrap();
        let remaining: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM conversation_model_context_items",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(remaining, 0);
    }
}
