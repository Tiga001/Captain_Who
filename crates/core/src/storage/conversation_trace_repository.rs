use crate::storage::agent_command_session_repository;
use crate::{
    ConversationCommandSessionLifecycle, ConversationTurnTrace, ConversationTurnTraceItem,
    ConversationTurnTraceTerminalStatus,
};
use rusqlite::types::Type;
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::HashMap;
use std::io::{Error as IoError, ErrorKind};

#[derive(Debug)]
struct TraceHeader {
    assistant_message_id: String,
    conversation_id: String,
    run_id: String,
    schema_version: u32,
    terminal_status: ConversationTurnTraceTerminalStatus,
    terminal_error: Option<String>,
    truncated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandSessionLifecycleAppendOutcome {
    Appended { sequence: u64 },
    Idempotent { sequence: u64 },
}

pub fn replace_trace(
    connection: &mut Connection,
    trace: &ConversationTurnTrace,
    created_at: i64,
    completed_at: i64,
) -> rusqlite::Result<()> {
    let transaction = connection.transaction()?;
    commit_trace_in_connection(&transaction, trace, created_at, completed_at)?;
    transaction.commit()
}

pub fn append_in_progress_trace(
    connection: &mut Connection,
    trace: &ConversationTurnTrace,
    created_at: i64,
    updated_at: i64,
) -> rusqlite::Result<bool> {
    if trace.terminal_status != ConversationTurnTraceTerminalStatus::InProgress {
        return Err(invalid_trace_input(
            "incremental conversation trace commit requires in_progress status",
        ));
    }
    let transaction = connection.transaction()?;
    let changed = commit_trace_in_connection(&transaction, trace, created_at, updated_at)?;
    transaction.commit()?;
    Ok(changed)
}

pub(crate) fn commit_trace_in_connection(
    connection: &Connection,
    trace: &ConversationTurnTrace,
    created_at: i64,
    committed_at: i64,
) -> rusqlite::Result<bool> {
    trace.validate().map_err(invalid_trace_input)?;
    if committed_at < created_at {
        return Err(invalid_trace_input(
            "conversation trace commit time cannot precede created_at",
        ));
    }

    let message_role = connection
        .query_row(
            "
            SELECT role
            FROM messages
            WHERE id = ?1 AND conversation_id = ?2
            ",
            params![&trace.assistant_message_id, &trace.conversation_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    match message_role.as_deref() {
        Some("assistant") => {}
        Some(_) => {
            return Err(invalid_trace_input(
                "conversation trace message must have the assistant role",
            ))
        }
        None => {
            return Err(invalid_trace_input(
                "conversation trace assistant message does not exist in its conversation",
            ))
        }
    }

    let existing = get_base_trace_for_message(connection, &trace.assistant_message_id)?;
    let existing_item_count = if let Some(existing) = &existing {
        validate_append_only_transition(existing, trace)?;
        existing.items.len()
    } else {
        0
    };
    let state_changed = existing.as_ref().is_none_or(|existing| {
        existing.terminal_status != trace.terminal_status
            || existing.terminal_error != trace.terminal_error
            || existing.truncated != trace.truncated
            || existing.items.len() != trace.items.len()
    });
    if !state_changed {
        let materialized = if trace.terminal_status.is_terminal() {
            let materialized = materialize_pending_command_session_lifecycle(
                connection,
                &trace.assistant_message_id,
                committed_at,
            )?;
            agent_command_session_repository::prune_terminal_sessions_in_connection(
                connection,
                &trace.conversation_id,
            )?;
            materialized
        } else {
            0
        };
        return Ok(materialized > 0);
    }

    if existing.is_none() {
        connection.execute(
            "
            INSERT INTO conversation_turn_traces (
                assistant_message_id,
                conversation_id,
                run_id,
                schema_version,
                terminal_status,
                terminal_error,
                truncated,
                created_at,
                updated_at,
                completed_at
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
            ",
            params![
                &trace.assistant_message_id,
                &trace.conversation_id,
                &trace.run_id,
                i64::from(trace.schema_version),
                trace.terminal_status.as_str(),
                &trace.terminal_error,
                trace.truncated,
                created_at,
                committed_at,
                trace.terminal_status.is_terminal().then_some(committed_at),
            ],
        )?;
    }

    for item in trace.items.iter().skip(existing_item_count) {
        insert_trace_item(connection, &trace.assistant_message_id, item)?;
    }

    if existing.is_some() {
        connection.execute(
            "
            UPDATE conversation_turn_traces
            SET terminal_status = ?1,
                terminal_error = ?2,
                truncated = ?3,
                updated_at = ?4,
                completed_at = COALESCE(completed_at, ?5)
            WHERE assistant_message_id = ?6
            ",
            params![
                trace.terminal_status.as_str(),
                &trace.terminal_error,
                trace.truncated,
                committed_at,
                trace.terminal_status.is_terminal().then_some(committed_at),
                &trace.assistant_message_id,
            ],
        )?;
    }

    if trace.terminal_status.is_terminal() {
        materialize_pending_command_session_lifecycle(
            connection,
            &trace.assistant_message_id,
            committed_at,
        )?;
        agent_command_session_repository::prune_terminal_sessions_in_connection(
            connection,
            &trace.conversation_id,
        )?;
    }

    Ok(true)
}

/// Records one command lifecycle audit item in the append-only sidecar journal.
///
/// Callers hold a write transaction (normally `BEGIN IMMEDIATE`). Active runtime Trace items are
/// never changed. If the source Trace is already terminal, pending sidecar events are
/// deterministically materialized into its sequence domain in the same transaction.
pub(crate) fn append_command_session_lifecycle_in_connection(
    connection: &Connection,
    assistant_message_id: &str,
    conversation_id: &str,
    lifecycle: ConversationCommandSessionLifecycle,
    _require_terminal_trace: bool,
    committed_at: i64,
) -> rusqlite::Result<CommandSessionLifecycleAppendOutcome> {
    if committed_at < 0 || lifecycle.created_at < 0 {
        return Err(invalid_trace_input(
            "command session lifecycle timestamps cannot be negative",
        ));
    }
    let phase = lifecycle_phase_as_str(lifecycle.phase);
    let is_terminal = lifecycle.phase == crate::ConversationCommandSessionLifecyclePhase::Terminal;
    let payload = serde_json::to_string(&lifecycle)
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
    let existing = connection
        .query_row(
            "SELECT event_id, event_json
             FROM agent_command_session_lifecycle_events
             WHERE session_id = ?1 AND phase = ?2",
            params![&lifecycle.session_id, phase],
            |row| Ok((row.get::<_, u64>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?;
    let (event_id, inserted) = if let Some((event_id, existing_payload)) = existing {
        let existing_lifecycle =
            serde_json::from_str::<ConversationCommandSessionLifecycle>(&existing_payload)
                .map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(1, Type::Text, Box::new(error))
                })?;
        if existing_lifecycle != lifecycle {
            return Err(invalid_trace_input(
                "command session lifecycle identity conflicts with an existing event",
            ));
        }
        (event_id, false)
    } else {
        connection.execute(
            "INSERT INTO agent_command_session_lifecycle_events (
                 session_id, phase, conversation_id, assistant_message_id, call_id,
                 event_json, archive_ref, created_at, recorded_at,
                 trace_sequence, materialized_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, NULL, NULL)",
            params![
                &lifecycle.session_id,
                phase,
                conversation_id,
                assistant_message_id,
                &lifecycle.call_id,
                payload,
                &lifecycle.archive.archive_ref,
                lifecycle.created_at,
                committed_at.max(lifecycle.created_at),
            ],
        )?;
        let event_id = u64::try_from(connection.last_insert_rowid())
            .map_err(|_| invalid_trace_input("command session lifecycle event id exceeds u64"))?;
        (event_id, true)
    };
    materialize_pending_command_session_lifecycle(connection, assistant_message_id, committed_at)?;
    if is_terminal {
        agent_command_session_repository::prune_terminal_sessions_in_connection(
            connection,
            conversation_id,
        )?;
    }
    let sequence = connection
        .query_row(
            "SELECT trace_sequence
             FROM agent_command_session_lifecycle_events
             WHERE event_id = ?1",
            [i64::try_from(event_id).map_err(|_| {
                invalid_trace_input("command session lifecycle event id exceeds SQLite INTEGER")
            })?],
            |row| row.get::<_, Option<u64>>(0),
        )
        .optional()?
        .flatten()
        .unwrap_or(event_id);
    Ok(if inserted {
        CommandSessionLifecycleAppendOutcome::Appended { sequence }
    } else {
        CommandSessionLifecycleAppendOutcome::Idempotent { sequence }
    })
}

fn validate_append_only_transition(
    existing: &ConversationTurnTrace,
    next: &ConversationTurnTrace,
) -> rusqlite::Result<()> {
    if existing.schema_version != next.schema_version
        || existing.run_id != next.run_id
        || existing.conversation_id != next.conversation_id
        || existing.assistant_message_id != next.assistant_message_id
    {
        return Err(invalid_trace_input(
            "conversation trace identity cannot change after its first commit",
        ));
    }
    if existing.terminal_status.is_terminal() && existing.terminal_status != next.terminal_status {
        return Err(invalid_trace_input(
            "terminal conversation trace cannot transition to another status",
        ));
    }
    if existing.truncated && !next.truncated {
        return Err(invalid_trace_input(
            "conversation trace truncated state cannot be cleared",
        ));
    }
    if existing.items.len() > next.items.len()
        || existing.items != next.items[..existing.items.len()]
    {
        return Err(invalid_trace_input(
            "conversation trace updates must preserve every committed item as an exact prefix",
        ));
    }
    Ok(())
}

fn insert_trace_item(
    connection: &Connection,
    assistant_message_id: &str,
    item: &ConversationTurnTraceItem,
) -> rusqlite::Result<()> {
    let sequence = i64::try_from(item.sequence()).map_err(|_| {
        invalid_trace_input("conversation trace item sequence exceeds SQLite INTEGER")
    })?;
    let payload = serde_json::to_string(item)
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
    connection.execute(
        "
        INSERT INTO conversation_turn_trace_items (
            assistant_message_id,
            sequence,
            item_kind,
            item_json
        )
        VALUES (?1, ?2, ?3, ?4)
        ",
        params![assistant_message_id, sequence, item.kind(), payload],
    )?;
    Ok(())
}

pub fn get_trace_for_message(
    connection: &Connection,
    assistant_message_id: &str,
) -> rusqlite::Result<Option<ConversationTurnTrace>> {
    get_base_trace_for_message(connection, assistant_message_id)?
        .map(|trace| overlay_pending_command_session_lifecycle(connection, trace))
        .transpose()
}

fn get_base_trace_for_message(
    connection: &Connection,
    assistant_message_id: &str,
) -> rusqlite::Result<Option<ConversationTurnTrace>> {
    let header = connection
        .query_row(
            "
            SELECT
                assistant_message_id,
                conversation_id,
                run_id,
                schema_version,
                terminal_status,
                terminal_error,
                truncated
            FROM conversation_turn_traces
            WHERE assistant_message_id = ?1
            ",
            params![assistant_message_id],
            trace_header_from_row,
        )
        .optional()?;
    header
        .map(|header| load_trace(connection, header))
        .transpose()
}

pub fn list_traces_for_conversation(
    connection: &Connection,
    conversation_id: &str,
) -> rusqlite::Result<Vec<ConversationTurnTrace>> {
    list_trace_records_for_conversation(connection, conversation_id)
        .map(|records| records.into_iter().map(|record| record.trace).collect())
}

pub struct ConversationTurnTraceRecord {
    pub trace: ConversationTurnTrace,
    pub completed_at: Option<i64>,
}

pub fn list_trace_records_for_conversation(
    connection: &Connection,
    conversation_id: &str,
) -> rusqlite::Result<Vec<ConversationTurnTraceRecord>> {
    let mut statement = connection.prepare(
        "
        SELECT
            trace.assistant_message_id,
            trace.conversation_id,
            trace.run_id,
            trace.schema_version,
            trace.terminal_status,
            trace.terminal_error,
            trace.truncated,
            trace.completed_at
        FROM conversation_turn_traces AS trace
        INNER JOIN messages AS message
            ON message.id = trace.assistant_message_id
        WHERE trace.conversation_id = ?1
        ORDER BY
            message.position ASC,
            message.created_at ASC,
            trace.updated_at ASC,
            trace.assistant_message_id ASC
        ",
    )?;
    let headers = statement
        .query_map(params![conversation_id], |row| {
            Ok((trace_header_from_row(row)?, row.get::<_, Option<i64>>(7)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(statement);
    if headers.is_empty() {
        return Ok(Vec::new());
    }

    let mut items_by_message = trace_items_for_conversation(connection, conversation_id)?;
    let mut pending_lifecycle_by_message =
        pending_command_session_lifecycle_for_conversation(connection, conversation_id)?;
    headers
        .into_iter()
        .map(|(header, completed_at)| {
            let assistant_message_id = header.assistant_message_id.clone();
            let trace = trace_from_stored_items(
                header,
                items_by_message
                    .remove(&assistant_message_id)
                    .unwrap_or_default(),
            )?;
            let trace = overlay_pending_command_session_lifecycle_rows(
                trace,
                pending_lifecycle_by_message
                    .remove(&assistant_message_id)
                    .unwrap_or_default(),
            )?;
            Ok(ConversationTurnTraceRecord {
                trace,
                completed_at,
            })
        })
        .collect()
}

type StoredTraceItem = (i64, String, String);

fn trace_items_for_conversation(
    connection: &Connection,
    conversation_id: &str,
) -> rusqlite::Result<HashMap<String, Vec<StoredTraceItem>>> {
    let mut statement = connection.prepare(
        "SELECT
             item.assistant_message_id,
             item.sequence,
             item.item_kind,
             item.item_json
         FROM conversation_turn_trace_items AS item
         INNER JOIN conversation_turn_traces AS trace
           ON trace.assistant_message_id = item.assistant_message_id
         WHERE trace.conversation_id = ?1
         ORDER BY item.assistant_message_id ASC, item.sequence ASC",
    )?;
    let rows = statement
        .query_map([conversation_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut items_by_message = HashMap::<String, Vec<StoredTraceItem>>::new();
    for (assistant_message_id, sequence, kind, json) in rows {
        items_by_message
            .entry(assistant_message_id)
            .or_default()
            .push((sequence, kind, json));
    }
    Ok(items_by_message)
}

pub fn list_in_progress_traces(
    connection: &Connection,
) -> rusqlite::Result<Vec<ConversationTurnTrace>> {
    let mut statement = connection.prepare(
        "
        SELECT
            assistant_message_id,
            conversation_id,
            run_id,
            schema_version,
            terminal_status,
            terminal_error,
            truncated
        FROM conversation_turn_traces
        WHERE terminal_status = 'in_progress'
        ORDER BY created_at ASC, assistant_message_id ASC
        ",
    )?;
    let headers = statement
        .query_map([], trace_header_from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    headers
        .into_iter()
        .map(|header| load_trace(connection, header))
        .collect()
}

fn trace_header_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<TraceHeader> {
    let schema_version = row.get::<_, i64>(3)?;
    let schema_version = u32::try_from(schema_version).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(3, Type::Integer, Box::new(error))
    })?;
    let terminal_status = row.get::<_, String>(4)?;
    let terminal_status = terminal_status_from_str(&terminal_status).ok_or_else(|| {
        corrupt_trace_data(
            4,
            Type::Text,
            format!("unknown conversation trace terminal status: {terminal_status}"),
        )
    })?;

    Ok(TraceHeader {
        assistant_message_id: row.get(0)?,
        conversation_id: row.get(1)?,
        run_id: row.get(2)?,
        schema_version,
        terminal_status,
        terminal_error: row.get(5)?,
        truncated: row.get(6)?,
    })
}

fn load_trace(
    connection: &Connection,
    header: TraceHeader,
) -> rusqlite::Result<ConversationTurnTrace> {
    let mut statement = connection.prepare(
        "
        SELECT sequence, item_kind, item_json
        FROM conversation_turn_trace_items
        WHERE assistant_message_id = ?1
        ORDER BY sequence ASC
        ",
    )?;
    let rows = statement
        .query_map(params![&header.assistant_message_id], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    trace_from_stored_items(header, rows)
}

fn trace_from_stored_items(
    header: TraceHeader,
    rows: Vec<StoredTraceItem>,
) -> rusqlite::Result<ConversationTurnTrace> {
    let mut items = Vec::with_capacity(rows.len());
    for (stored_sequence, stored_kind, raw_item) in rows {
        let item: ConversationTurnTraceItem = serde_json::from_str(&raw_item).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(2, Type::Text, Box::new(error))
        })?;
        let item_sequence = i64::try_from(item.sequence()).map_err(|_| {
            corrupt_trace_data(
                0,
                Type::Integer,
                "conversation trace item sequence exceeds SQLite INTEGER",
            )
        })?;
        if item_sequence != stored_sequence || item.kind() != stored_kind {
            return Err(corrupt_trace_data(
                2,
                Type::Text,
                "conversation trace item metadata does not match its payload",
            ));
        }
        items.push(item);
    }

    let trace = ConversationTurnTrace {
        schema_version: header.schema_version,
        run_id: header.run_id,
        conversation_id: header.conversation_id,
        assistant_message_id: header.assistant_message_id,
        terminal_status: header.terminal_status,
        terminal_error: header.terminal_error,
        truncated: header.truncated,
        items,
    };
    trace
        .validate()
        .map_err(|message| corrupt_trace_data(0, Type::Text, message))?;
    Ok(trace)
}

fn lifecycle_phase_as_str(phase: crate::ConversationCommandSessionLifecyclePhase) -> &'static str {
    match phase {
        crate::ConversationCommandSessionLifecyclePhase::Started => "started",
        crate::ConversationCommandSessionLifecyclePhase::Terminal => "terminal",
    }
}

fn pending_command_session_lifecycle(
    connection: &Connection,
    assistant_message_id: &str,
) -> rusqlite::Result<Vec<(u64, i64, ConversationCommandSessionLifecycle)>> {
    let mut statement = connection.prepare(
        "SELECT event_id, recorded_at, event_json
         FROM agent_command_session_lifecycle_events
         WHERE assistant_message_id = ?1 AND trace_sequence IS NULL
         ORDER BY
             created_at ASC,
             CASE phase WHEN 'started' THEN 0 ELSE 1 END ASC,
             session_id ASC,
             event_id ASC",
    )?;
    let rows = statement
        .query_map([assistant_message_id], |row| {
            Ok((
                row.get::<_, u64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    rows.into_iter()
        .map(|(event_id, recorded_at, payload)| {
            let lifecycle = serde_json::from_str(&payload).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(2, Type::Text, Box::new(error))
            })?;
            Ok((event_id, recorded_at, lifecycle))
        })
        .collect()
}

type PendingCommandSessionLifecycle = (u64, i64, ConversationCommandSessionLifecycle);

fn pending_command_session_lifecycle_for_conversation(
    connection: &Connection,
    conversation_id: &str,
) -> rusqlite::Result<HashMap<String, Vec<PendingCommandSessionLifecycle>>> {
    let mut statement = connection.prepare(
        "SELECT
             lifecycle.assistant_message_id,
             lifecycle.event_id,
             lifecycle.recorded_at,
             lifecycle.event_json
         FROM agent_command_session_lifecycle_events AS lifecycle
         INNER JOIN conversation_turn_traces AS trace
           ON trace.assistant_message_id = lifecycle.assistant_message_id
         WHERE trace.conversation_id = ?1
           AND trace.terminal_status != 'in_progress'
           AND lifecycle.trace_sequence IS NULL
         ORDER BY
             lifecycle.assistant_message_id ASC,
             lifecycle.created_at ASC,
             CASE lifecycle.phase WHEN 'started' THEN 0 ELSE 1 END ASC,
             lifecycle.session_id ASC,
             lifecycle.event_id ASC",
    )?;
    let rows = statement
        .query_map([conversation_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, u64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut lifecycle_by_message = HashMap::<String, Vec<PendingCommandSessionLifecycle>>::new();
    for (assistant_message_id, event_id, recorded_at, payload) in rows {
        let lifecycle = serde_json::from_str(&payload).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(3, Type::Text, Box::new(error))
        })?;
        lifecycle_by_message
            .entry(assistant_message_id)
            .or_default()
            .push((event_id, recorded_at, lifecycle));
    }
    Ok(lifecycle_by_message)
}

fn overlay_pending_command_session_lifecycle(
    connection: &Connection,
    trace: ConversationTurnTrace,
) -> rusqlite::Result<ConversationTurnTrace> {
    if !trace.terminal_status.is_terminal() {
        return Ok(trace);
    }
    let rows = pending_command_session_lifecycle(connection, &trace.assistant_message_id)?;
    overlay_pending_command_session_lifecycle_rows(trace, rows)
}

fn overlay_pending_command_session_lifecycle_rows(
    mut trace: ConversationTurnTrace,
    rows: Vec<PendingCommandSessionLifecycle>,
) -> rusqlite::Result<ConversationTurnTrace> {
    if !trace.terminal_status.is_terminal() && !rows.is_empty() {
        return Err(corrupt_trace_data(
            2,
            Type::Text,
            "nonterminal conversation trace has pending terminal command lifecycle",
        ));
    }
    for (_, _, lifecycle) in rows {
        trace
            .append_command_session_lifecycle(lifecycle)
            .map_err(|message| corrupt_trace_data(2, Type::Text, message))?;
    }
    Ok(trace)
}

fn materialize_pending_command_session_lifecycle(
    connection: &Connection,
    assistant_message_id: &str,
    committed_at: i64,
) -> rusqlite::Result<usize> {
    let Some(mut trace) = get_base_trace_for_message(connection, assistant_message_id)? else {
        return Ok(0);
    };
    if !trace.terminal_status.is_terminal() {
        return Ok(0);
    }
    let pending = pending_command_session_lifecycle(connection, assistant_message_id)?;
    let mut materialized = 0_usize;
    let mut newest_materialized_at = committed_at;
    for (event_id, recorded_at, lifecycle) in pending {
        let previous_len = trace.items.len();
        let sequence = trace
            .append_command_session_lifecycle(lifecycle)
            .map_err(invalid_trace_input)?;
        if trace.items.len() != previous_len {
            let item = trace.items.last().ok_or_else(|| {
                invalid_trace_input("new command session lifecycle item is missing")
            })?;
            insert_trace_item(connection, assistant_message_id, item)?;
        }
        let materialized_at = committed_at.max(recorded_at);
        connection.execute(
            "UPDATE agent_command_session_lifecycle_events
             SET trace_sequence = ?1, materialized_at = ?2
             WHERE event_id = ?3 AND trace_sequence IS NULL",
            params![
                i64::try_from(sequence).map_err(|_| invalid_trace_input(
                    "command session lifecycle trace sequence exceeds SQLite INTEGER"
                ))?,
                materialized_at,
                i64::try_from(event_id).map_err(|_| invalid_trace_input(
                    "command session lifecycle event id exceeds SQLite INTEGER"
                ))?,
            ],
        )?;
        newest_materialized_at = newest_materialized_at.max(materialized_at);
        materialized = materialized.saturating_add(1);
    }
    if materialized > 0 {
        connection.execute(
            "UPDATE conversation_turn_traces
             SET updated_at = MAX(updated_at, ?1)
             WHERE assistant_message_id = ?2",
            params![newest_materialized_at, assistant_message_id],
        )?;
    }
    Ok(materialized)
}

fn terminal_status_from_str(value: &str) -> Option<ConversationTurnTraceTerminalStatus> {
    match value {
        "in_progress" => Some(ConversationTurnTraceTerminalStatus::InProgress),
        "completed" => Some(ConversationTurnTraceTerminalStatus::Completed),
        "failed" => Some(ConversationTurnTraceTerminalStatus::Failed),
        "cancelled" => Some(ConversationTurnTraceTerminalStatus::Cancelled),
        _ => None,
    }
}

fn invalid_trace_input(message: impl Into<String>) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(Box::new(IoError::new(
        ErrorKind::InvalidInput,
        message.into(),
    )))
}

fn corrupt_trace_data(
    column: usize,
    data_type: Type,
    message: impl Into<String>,
) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        column,
        data_type,
        Box::new(IoError::new(ErrorKind::InvalidData, message.into())),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{chat_repository, migrations};
    use crate::{
        AgentApprovalStatus, ConversationTraceToolResultStatus, ConversationTurnTraceItem,
        CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
    };
    use rusqlite::Connection;
    use serde_json::json;

    #[test]
    fn round_trips_trace_and_lists_by_message_position() {
        let mut connection = test_connection();
        insert_conversation(&connection, "conversation-1");
        insert_message(&connection, "conversation-1", "assistant-later", 3);
        insert_message(&connection, "conversation-1", "assistant-earlier", 1);
        let later = trace("conversation-1", "assistant-later", "run-later");
        let earlier = trace("conversation-1", "assistant-earlier", "run-earlier");

        replace_trace(&mut connection, &later, 30, 40).unwrap();
        replace_trace(&mut connection, &earlier, 10, 20).unwrap();

        assert_eq!(
            get_trace_for_message(&connection, "assistant-later").unwrap(),
            Some(later.clone())
        );
        assert_eq!(
            list_traces_for_conversation(&connection, "conversation-1").unwrap(),
            vec![earlier, later]
        );
    }

    #[test]
    fn incremental_commit_and_terminal_finalization_are_atomic_and_idempotent() {
        let mut connection = test_connection();
        insert_conversation(&connection, "conversation-1");
        insert_message(&connection, "conversation-1", "assistant-1", 1);
        let completed = trace("conversation-1", "assistant-1", "run-1");
        let initial = ConversationTurnTrace {
            terminal_status: ConversationTurnTraceTerminalStatus::InProgress,
            items: vec![completed.items[0].clone()],
            ..completed.clone()
        };
        replace_trace(&mut connection, &initial, 10, 20).unwrap();

        let replacement = ConversationTurnTrace {
            terminal_status: ConversationTurnTraceTerminalStatus::Failed,
            terminal_error: Some("model request failed".to_string()),
            truncated: true,
            ..completed
        };
        replace_trace(&mut connection, &replacement, 10, 30).unwrap();
        replace_trace(&mut connection, &replacement, 10, 30).unwrap();

        assert_eq!(
            get_trace_for_message(&connection, "assistant-1").unwrap(),
            Some(replacement)
        );
        let trace_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM conversation_turn_traces", [], |row| {
                row.get(0)
            })
            .unwrap();
        let item_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM conversation_turn_trace_items",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(trace_count, 1);
        assert_eq!(item_count, 3);
    }

    #[test]
    fn append_only_audit_items_do_not_rewrite_the_original_completion_time() {
        let mut connection = test_connection();
        insert_conversation(&connection, "conversation-1");
        insert_message(&connection, "conversation-1", "assistant-1", 1);
        let completed = trace("conversation-1", "assistant-1", "run-1");
        replace_trace(&mut connection, &completed, 10, 20).unwrap();

        let mut with_late_audit_item = completed;
        with_late_audit_item
            .items
            .push(ConversationTurnTraceItem::AssistantNarration {
                provider_turn_id: None,
                first_tool_call_id: None,
                sequence: 3,
                content: "late audit placeholder".to_string(),
                truncated: false,
            });
        replace_trace(&mut connection, &with_late_audit_item, 10, 50).unwrap();

        let (updated_at, completed_at): (i64, i64) = connection
            .query_row(
                "SELECT updated_at, completed_at
                 FROM conversation_turn_traces
                 WHERE assistant_message_id = 'assistant-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(updated_at, 50);
        assert_eq!(completed_at, 20);
    }

    #[test]
    fn failed_replace_rolls_back_header_and_items() {
        let mut connection = test_connection();
        insert_conversation(&connection, "conversation-1");
        insert_message(&connection, "conversation-1", "assistant-1", 1);
        let completed = trace("conversation-1", "assistant-1", "run-1");
        let initial = ConversationTurnTrace {
            terminal_status: ConversationTurnTraceTerminalStatus::InProgress,
            items: vec![completed.items[0].clone()],
            ..completed.clone()
        };
        replace_trace(&mut connection, &initial, 10, 20).unwrap();
        connection
            .execute_batch(
                "
                CREATE TRIGGER fail_conversation_trace_item_insert
                BEFORE INSERT ON conversation_turn_trace_items
                BEGIN
                    SELECT RAISE(ABORT, 'forced trace item insert failure');
                END;
                ",
            )
            .unwrap();

        let mut replacement = completed;
        replacement.terminal_status = ConversationTurnTraceTerminalStatus::Failed;
        replacement.terminal_error = Some("replacement should roll back".to_string());
        assert!(replace_trace(&mut connection, &replacement, 10, 30).is_err());
        connection
            .execute_batch("DROP TRIGGER fail_conversation_trace_item_insert;")
            .unwrap();

        assert_eq!(
            get_trace_for_message(&connection, "assistant-1").unwrap(),
            Some(initial)
        );
    }

    #[test]
    fn committed_prefix_cannot_be_rewritten_shrunk_or_reopened() {
        let mut connection = test_connection();
        insert_conversation(&connection, "conversation-1");
        insert_message(&connection, "conversation-1", "assistant-1", 1);
        let completed = trace("conversation-1", "assistant-1", "run-1");
        let running = ConversationTurnTrace {
            terminal_status: ConversationTurnTraceTerminalStatus::InProgress,
            items: vec![completed.items[0].clone()],
            ..completed.clone()
        };

        assert!(append_in_progress_trace(&mut connection, &running, 10, 20).unwrap());
        assert!(!append_in_progress_trace(&mut connection, &running, 10, 21).unwrap());

        let mut rewritten = running.clone();
        if let ConversationTurnTraceItem::AssistantNarration { content, .. } =
            &mut rewritten.items[0]
        {
            *content = "rewritten".to_string();
        }
        assert!(append_in_progress_trace(&mut connection, &rewritten, 10, 22).is_err());

        let mut shrunk = running.clone();
        shrunk.items.clear();
        assert!(append_in_progress_trace(&mut connection, &shrunk, 10, 23).is_err());

        replace_trace(&mut connection, &completed, 10, 24).unwrap();
        assert!(append_in_progress_trace(&mut connection, &running, 10, 25).is_err());
        assert_eq!(
            get_trace_for_message(&connection, "assistant-1").unwrap(),
            Some(completed)
        );
    }

    #[test]
    fn message_and_conversation_deletes_cascade_to_trace_items() {
        let mut connection = test_connection();
        insert_conversation(&connection, "conversation-message");
        insert_message(&connection, "conversation-message", "assistant-message", 1);
        replace_trace(
            &mut connection,
            &trace("conversation-message", "assistant-message", "run-message"),
            10,
            20,
        )
        .unwrap();

        chat_repository::delete_messages(
            &mut connection,
            "conversation-message",
            &["assistant-message".to_string()],
        )
        .unwrap();
        assert_trace_tables_empty(&connection);

        insert_conversation(&connection, "conversation-delete");
        insert_message(&connection, "conversation-delete", "assistant-delete", 1);
        replace_trace(
            &mut connection,
            &trace("conversation-delete", "assistant-delete", "run-delete"),
            10,
            20,
        )
        .unwrap();
        chat_repository::delete_conversation(&connection, "conversation-delete").unwrap();
        assert_trace_tables_empty(&connection);
    }

    fn test_connection() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        migrations::run_migrations(&connection).unwrap();
        connection
    }

    fn insert_conversation(connection: &Connection, conversation_id: &str) {
        connection
            .execute(
                "
                INSERT INTO conversations (id, title, created_at, updated_at)
                VALUES (?1, ?1, 1, 1)
                ",
                params![conversation_id],
            )
            .unwrap();
    }

    fn insert_message(
        connection: &Connection,
        conversation_id: &str,
        message_id: &str,
        position: i64,
    ) {
        connection
            .execute(
                "
                INSERT INTO messages (
                    id, conversation_id, role, content, created_at, position
                )
                VALUES (?1, ?2, 'assistant', 'answer', ?3, ?3)
                ",
                params![message_id, conversation_id, position],
            )
            .unwrap();
    }

    fn trace(
        conversation_id: &str,
        assistant_message_id: &str,
        run_id: &str,
    ) -> ConversationTurnTrace {
        ConversationTurnTrace {
            schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
            run_id: run_id.to_string(),
            conversation_id: conversation_id.to_string(),
            assistant_message_id: assistant_message_id.to_string(),
            terminal_status: ConversationTurnTraceTerminalStatus::Completed,
            terminal_error: None,
            truncated: false,
            items: vec![
                ConversationTurnTraceItem::AssistantNarration {
                    provider_turn_id: None,
                    first_tool_call_id: None,
                    sequence: 0,
                    content: "I will inspect the file.".to_string(),
                    truncated: false,
                },
                ConversationTurnTraceItem::ToolCall {
                    sequence: 1,
                    call_id: "call-1".to_string(),
                    tool: "read_file".to_string(),
                    provenance: crate::AgentToolIdentity::Builtin {
                        tool_name: "read_file".to_string(),
                    },
                    operation: json!({ "path": "src/lib.rs", "startLine": 1, "endLine": 20 }),
                    approval_status: AgentApprovalStatus::NotRequired,
                    truncated: false,
                },
                ConversationTurnTraceItem::ToolResult {
                    sequence: 2,
                    call_id: "call-1".to_string(),
                    tool: "read_file".to_string(),
                    status: ConversationTraceToolResultStatus::Succeeded,
                    success: true,
                    observation: json!({ "path": "src/lib.rs", "startLine": 1, "endLine": 20 }),
                    approval_status: AgentApprovalStatus::NotRequired,
                    error: None,
                    truncated: false,
                    archive: Default::default(),
                },
            ],
        }
    }

    fn assert_trace_tables_empty(connection: &Connection) {
        for table in ["conversation_turn_traces", "conversation_turn_trace_items"] {
            let count: i64 = connection
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(count, 0, "expected {table} to be empty");
        }
    }
}
