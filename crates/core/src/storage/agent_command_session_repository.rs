//! Durable, conversation-scoped state for Host-owned command sessions.
//!
//! This repository is an operational projection, not a second audit journal. Lifecycle history
//! belongs to the conversation trace and complete terminal output belongs to Exact History. The
//! rows here answer only "what can the Host currently observe or control?" and retain a bounded
//! transcript so a Renderer reload does not depend on in-process memory.

use crate::command::{
    AgentCommandPublishedOutput, AgentCommandPublishedOutputKind, CommandAuthorizationSource,
};
use crate::storage::command_session_receipt_payload::{
    decode_command_session_receipt_payload, encode_command_session_receipt_payload,
};
use crate::{
    AgentCommandOutputStream, AgentCommandSessionAction, AgentCommandSessionOutputChunk,
    AgentCommandSessionSnapshot, AgentCommandSessionStatus, AgentCommandSessionTranscript,
    AGENT_COMMAND_SESSION_MAX_TRANSCRIPT_CHUNKS, AGENT_COMMAND_SESSION_MODEL_OUTPUT_BYTES,
};
use rusqlite::types::Type;
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::io::{Error as IoError, ErrorKind};

pub const AGENT_COMMAND_SESSION_SCHEMA_VERSION: u32 = 1;
pub const MAX_PERSISTED_COMMAND_TRANSCRIPT_BYTES: usize = 256 * 1024;
const PERSISTED_COMMAND_TRANSCRIPT_HEAD_BYTES: usize = 64 * 1024;
const PERSISTED_COMMAND_TRANSCRIPT_HEAD_CHUNKS: usize =
    AGENT_COMMAND_SESSION_MAX_TRANSCRIPT_CHUNKS / 4;
const MAX_PERSISTED_COMMAND_OUTPUT_CHUNK_BYTES: usize = 64 * 1024;
pub const MAX_RETAINED_TERMINAL_COMMAND_SESSIONS_PER_CONVERSATION: usize = 128;
pub const MAX_RETAINED_MODEL_READ_RECEIPTS_PER_SESSION: usize = 64;

#[derive(Debug, Clone)]
pub struct AgentCommandSessionCreate {
    pub snapshot: AgentCommandSessionSnapshot,
    pub authorization_source: CommandAuthorizationSource,
    pub approval_provenance: Value,
    pub permission_provenance: Value,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AgentCommandSessionRecord {
    pub snapshot: AgentCommandSessionSnapshot,
    pub authorization_source: CommandAuthorizationSource,
    pub approval_provenance: Value,
    pub permission_provenance: Value,
    pub model_read_sequence: u64,
    pub transcript_truncated: bool,
    pub output_capture_truncated: bool,
    pub terminal_reason: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub settled_at: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct AgentCommandSessionOutputAppend<'a> {
    pub conversation_id: &'a str,
    pub session_id: &'a str,
    pub chunks: &'a [AgentCommandSessionOutputChunk],
    pub latest_sequence: u64,
    pub transcript_truncated: bool,
    pub output_capture_truncated: bool,
    pub updated_at: i64,
}

#[derive(Debug, Clone)]
pub struct AgentCommandSessionModelReadRequest<'a> {
    pub conversation_id: &'a str,
    pub session_id: &'a str,
    pub run_id: &'a str,
    pub call_id: &'a str,
    pub action: AgentCommandSessionAction,
    pub max_output_bytes: usize,
    pub host_output_truncated: bool,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentCommandSessionModelReadReceipt {
    pub conversation_id: String,
    pub session_id: String,
    pub run_id: String,
    pub call_id: String,
    pub action: AgentCommandSessionAction,
    pub max_output_bytes: usize,
    pub requested_after_sequence: u64,
    pub first_output_sequence: Option<u64>,
    pub last_output_sequence: Option<u64>,
    pub status: AgentCommandSessionStatus,
    pub exit_code: Option<i32>,
    pub latest_sequence: u64,
    pub truncated_before: bool,
    pub output_truncated: bool,
    pub output_bytes: usize,
    pub output_hash: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentCommandSessionModelRead {
    pub receipt: AgentCommandSessionModelReadReceipt,
    pub chunks: Vec<AgentCommandSessionOutputChunk>,
    pub outputs: Vec<AgentCommandPublishedOutput>,
}

struct StoredAgentCommandSessionModelReadReceipt {
    receipt: AgentCommandSessionModelReadReceipt,
    payload_compression: String,
    payload: Vec<u8>,
    chunk_count: usize,
}

/// Internal ascending model cursor projection. It shares the operational row-count bound but is
/// independent from the Host's latest-tail hydration view, so a reload cannot advance or skip the
/// model's unread retained prefix.
struct AgentCommandSessionModelTranscriptCut {
    requested_after_sequence: u64,
    latest_sequence: u64,
    truncated_before: bool,
    output_capture_truncated: bool,
    chunks: Vec<AgentCommandSessionOutputChunk>,
}

#[derive(Debug, Clone)]
pub struct AgentCommandSessionTerminalUpdate<'a> {
    pub conversation_id: &'a str,
    pub session_id: &'a str,
    pub status: AgentCommandSessionStatus,
    pub ended_at: u64,
    pub exit_code: Option<i32>,
    pub latest_sequence: u64,
    pub transcript_truncated: bool,
    pub output_capture_truncated: bool,
    pub archive_ref: Option<&'a str>,
    pub terminal_reason: Option<&'a str>,
    pub published_outputs: &'a [AgentCommandPublishedOutput],
    pub committed_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentCommandSessionCreateOutcome {
    Inserted,
    Idempotent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentCommandSessionTransitionOutcome {
    Updated,
    Idempotent,
    Conflict {
        current_status: AgentCommandSessionStatus,
    },
    NotFound,
}

pub fn create_session(
    connection: &mut Connection,
    input: &AgentCommandSessionCreate,
) -> rusqlite::Result<AgentCommandSessionCreateOutcome> {
    validate_create(input)?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if let Some(existing) = get_session_in_connection(
        &transaction,
        &input.snapshot.conversation_id,
        &input.snapshot.session_id,
    )? {
        validate_idempotent_create(&existing, input)?;
        transaction.commit()?;
        return Ok(AgentCommandSessionCreateOutcome::Idempotent);
    }
    if session_for_call_exists(
        &transaction,
        &input.snapshot.conversation_id,
        &input.snapshot.assistant_message_id,
        &input.snapshot.call_id,
    )? {
        return Err(invalid_input(
            "command session call identity is already bound to another session",
        ));
    }

    let approval_provenance = serialize_json(&input.approval_provenance)?;
    let permission_provenance = serialize_json(&input.permission_provenance)?;
    transaction.execute(
        "INSERT INTO agent_command_sessions (
             session_id, schema_version, conversation_id, assistant_message_id,
             origin_run_id, call_id, project_id, command_projection, cwd_projection,
             command_digest, authorization_source, approval_provenance_json,
             permission_provenance_json, status, started_at, ended_at, exit_code,
             latest_sequence, model_read_sequence, transcript_truncated,
             output_capture_truncated, archive_ref, terminal_reason,
             created_at, updated_at, settled_at
         ) VALUES (
             ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14,
             ?15, NULL, NULL, ?16, 0, ?17, ?18, NULL, NULL, ?19, ?19, NULL
         )",
        params![
            &input.snapshot.session_id,
            i64::from(AGENT_COMMAND_SESSION_SCHEMA_VERSION),
            &input.snapshot.conversation_id,
            &input.snapshot.assistant_message_id,
            &input.snapshot.origin_run_id,
            &input.snapshot.call_id,
            &input.snapshot.project_id,
            &input.snapshot.command,
            &input.snapshot.cwd,
            &input.snapshot.command_digest,
            authorization_source_as_str(input.authorization_source),
            approval_provenance,
            permission_provenance,
            status_as_str(input.snapshot.status),
            sqlite_integer(input.snapshot.started_at)?,
            sqlite_integer(input.snapshot.latest_sequence)?,
            false,
            input.snapshot.output_truncated,
            input.created_at,
        ],
    )?;
    transaction.commit()?;
    Ok(AgentCommandSessionCreateOutcome::Inserted)
}

pub fn mark_running(
    connection: &mut Connection,
    conversation_id: &str,
    session_id: &str,
    updated_at: i64,
) -> rusqlite::Result<AgentCommandSessionTransitionOutcome> {
    validate_identity(conversation_id, session_id)?;
    validate_timestamp(updated_at, "command session running updated_at")?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let outcome =
        mark_running_in_connection(&transaction, conversation_id, session_id, updated_at)?;
    transaction.commit()?;
    Ok(outcome)
}

pub(crate) fn mark_running_in_connection(
    connection: &Connection,
    conversation_id: &str,
    session_id: &str,
    updated_at: i64,
) -> rusqlite::Result<AgentCommandSessionTransitionOutcome> {
    validate_identity(conversation_id, session_id)?;
    validate_timestamp(updated_at, "command session running updated_at")?;
    let Some(current) = get_session_in_connection(connection, conversation_id, session_id)? else {
        return Ok(AgentCommandSessionTransitionOutcome::NotFound);
    };
    let outcome = match current.snapshot.status {
        AgentCommandSessionStatus::Starting => {
            connection.execute(
                "UPDATE agent_command_sessions
                 SET status = 'running', updated_at = MAX(updated_at, ?1)
                 WHERE conversation_id = ?2 AND session_id = ?3 AND status = 'starting'",
                params![updated_at, conversation_id, session_id],
            )?;
            AgentCommandSessionTransitionOutcome::Updated
        }
        AgentCommandSessionStatus::Running => AgentCommandSessionTransitionOutcome::Idempotent,
        status => AgentCommandSessionTransitionOutcome::Conflict {
            current_status: status,
        },
    };
    Ok(outcome)
}

pub fn append_output(
    connection: &mut Connection,
    input: &AgentCommandSessionOutputAppend<'_>,
) -> rusqlite::Result<AgentCommandSessionTransitionOutcome> {
    validate_identity(input.conversation_id, input.session_id)?;
    validate_timestamp(input.updated_at, "command session output updated_at")?;
    validate_output_chunks(input.chunks, input.latest_sequence)?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let Some(current) =
        get_session_in_connection(&transaction, input.conversation_id, input.session_id)?
    else {
        transaction.commit()?;
        return Ok(AgentCommandSessionTransitionOutcome::NotFound);
    };
    if current.snapshot.status.is_terminal() {
        transaction.commit()?;
        return Ok(AgentCommandSessionTransitionOutcome::Conflict {
            current_status: current.snapshot.status,
        });
    }
    if input.latest_sequence < current.snapshot.latest_sequence {
        return Err(invalid_input(
            "command session output latest sequence cannot move backwards",
        ));
    }

    let mut inserted_chunk = false;
    for chunk in input.chunks {
        let existing = transaction
            .query_row(
                "SELECT stream, output
                 FROM agent_command_session_output_chunks
                 WHERE session_id = ?1 AND sequence = ?2",
                params![input.session_id, sqlite_integer(chunk.sequence)?],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;
        if let Some((stream, output)) = existing {
            if stream != output_stream_as_str(chunk.stream) || output != chunk.output {
                return Err(invalid_input(
                    "command session output sequence was reused for different content",
                ));
            }
            continue;
        }
        transaction.execute(
            "INSERT INTO agent_command_session_output_chunks (
                 session_id, sequence, stream, output, output_bytes, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                input.session_id,
                sqlite_integer(chunk.sequence)?,
                output_stream_as_str(chunk.stream),
                &chunk.output,
                sqlite_integer(chunk.output.len() as u64)?,
                input.updated_at,
            ],
        )?;
        inserted_chunk = true;
    }
    let retention_truncated = prune_transcript_in_connection(&transaction, input.session_id)?;
    let transcript_truncated =
        current.transcript_truncated || input.transcript_truncated || retention_truncated;
    let output_capture_truncated =
        current.output_capture_truncated || input.output_capture_truncated;
    let changed = input.latest_sequence != current.snapshot.latest_sequence
        || transcript_truncated != current.transcript_truncated
        || output_capture_truncated != current.output_capture_truncated;
    transaction.execute(
        "UPDATE agent_command_sessions
         SET latest_sequence = ?1,
             transcript_truncated = ?2,
             output_capture_truncated = ?3,
             updated_at = MAX(updated_at, ?4)
         WHERE conversation_id = ?5 AND session_id = ?6
           AND status IN ('starting', 'running')",
        params![
            sqlite_integer(input.latest_sequence)?,
            transcript_truncated,
            output_capture_truncated,
            input.updated_at,
            input.conversation_id,
            input.session_id,
        ],
    )?;
    transaction.commit()?;
    Ok(if changed || inserted_chunk {
        AgentCommandSessionTransitionOutcome::Updated
    } else {
        AgentCommandSessionTransitionOutcome::Idempotent
    })
}

pub fn commit_terminal(
    connection: &mut Connection,
    input: &AgentCommandSessionTerminalUpdate<'_>,
) -> rusqlite::Result<AgentCommandSessionTransitionOutcome> {
    validate_terminal_update(input)?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let outcome = commit_terminal_in_connection(&transaction, input)?;
    if matches!(
        outcome,
        AgentCommandSessionTransitionOutcome::Updated
            | AgentCommandSessionTransitionOutcome::Idempotent
    ) {
        prune_terminal_sessions_in_connection(&transaction, input.conversation_id)?;
    }
    transaction.commit()?;
    Ok(outcome)
}

/// Applies the terminal state CAS inside a caller-owned transaction.
///
/// This is intentionally crate-visible so `StorageService` can commit the terminal Trace event
/// and the operational Session row atomically after Exact History has already been archived.
pub(crate) fn commit_terminal_in_connection(
    connection: &Connection,
    input: &AgentCommandSessionTerminalUpdate<'_>,
) -> rusqlite::Result<AgentCommandSessionTransitionOutcome> {
    validate_terminal_update(input)?;
    let Some(current) =
        get_session_in_connection(connection, input.conversation_id, input.session_id)?
    else {
        return Ok(AgentCommandSessionTransitionOutcome::NotFound);
    };
    if current.snapshot.status.is_terminal() {
        let published_outputs = load_published_outputs(connection, input.session_id)?;
        let idempotent = current.snapshot.status == input.status
            && current.snapshot.ended_at == Some(input.ended_at)
            && current.snapshot.exit_code == input.exit_code
            && current.snapshot.latest_sequence == input.latest_sequence
            && current.snapshot.archive_ref.as_deref() == input.archive_ref
            && current.terminal_reason.as_deref() == input.terminal_reason
            && published_outputs == input.published_outputs;
        return Ok(if idempotent {
            AgentCommandSessionTransitionOutcome::Idempotent
        } else {
            AgentCommandSessionTransitionOutcome::Conflict {
                current_status: current.snapshot.status,
            }
        });
    }
    if input.latest_sequence < current.snapshot.latest_sequence {
        return Err(invalid_input(
            "command session terminal sequence cannot move backwards",
        ));
    }
    let transcript_truncated = current.transcript_truncated || input.transcript_truncated;
    let output_capture_truncated =
        current.output_capture_truncated || input.output_capture_truncated;
    let affected = connection.execute(
        "UPDATE agent_command_sessions
         SET status = ?1, ended_at = ?2, exit_code = ?3,
             latest_sequence = ?4, transcript_truncated = ?5,
             output_capture_truncated = ?6, archive_ref = ?7,
             terminal_reason = ?8, updated_at = ?9, settled_at = ?9
         WHERE conversation_id = ?10 AND session_id = ?11
           AND status IN ('starting', 'running')",
        params![
            status_as_str(input.status),
            sqlite_integer(input.ended_at)?,
            input.exit_code,
            sqlite_integer(input.latest_sequence)?,
            transcript_truncated,
            output_capture_truncated,
            input.archive_ref,
            input.terminal_reason,
            input.committed_at,
            input.conversation_id,
            input.session_id,
        ],
    )?;
    if affected != 1 {
        return Err(invalid_input(
            "command session terminal transition lost its active-state CAS",
        ));
    }
    insert_published_outputs(connection, input.session_id, input.published_outputs)?;
    Ok(AgentCommandSessionTransitionOutcome::Updated)
}

pub fn get_session(
    connection: &Connection,
    conversation_id: &str,
    session_id: &str,
) -> rusqlite::Result<Option<AgentCommandSessionRecord>> {
    validate_identity(conversation_id, session_id)?;
    get_session_in_connection(connection, conversation_id, session_id)
}

pub fn list_sessions_for_conversation(
    connection: &Connection,
    conversation_id: &str,
    terminal_limit: usize,
) -> rusqlite::Result<Vec<AgentCommandSessionRecord>> {
    if conversation_id.trim().is_empty() {
        return Err(invalid_input("command session conversation id is empty"));
    }
    let terminal_limit =
        terminal_limit.min(MAX_RETAINED_TERMINAL_COMMAND_SESSIONS_PER_CONVERSATION);
    let mut statement = connection.prepare(&format!(
        "{} WHERE conversation_id = ?1
         ORDER BY
             CASE
                 WHEN status IN ('starting', 'running') THEN 0
                 WHEN status = 'outcome_unknown' THEN 1
                 ELSE 2
             END,
             COALESCE(ended_at, started_at) DESC,
             session_id ASC",
        record_select()
    ))?;
    let mut records = statement
        .query_map([conversation_id], record_from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(statement);
    attach_terminal_outputs(connection, &mut records)?;
    let mut terminal_seen = 0_usize;
    Ok(records
        .into_iter()
        .filter(|record| {
            // Every active Session is bounded by the manager's admission limits. All terminal
            // projections, including unresolved outcome-unknown rows, share the caller's recent
            // window so this Host-facing list can never grow with durable history. The complete
            // outcome-unknown set remains in storage and is loaded separately during startup to
            // rebuild deletion fences.
            if !record.snapshot.status.is_terminal() {
                return true;
            }
            terminal_seen = terminal_seen.saturating_add(1);
            terminal_seen <= terminal_limit
        })
        .collect())
}

pub fn read_transcript(
    connection: &Connection,
    conversation_id: &str,
    session_id: &str,
    after_sequence: u64,
    max_bytes: usize,
) -> rusqlite::Result<Option<AgentCommandSessionTranscript>> {
    if max_bytes == 0 {
        return Err(invalid_input(
            "command session transcript max_bytes must be positive",
        ));
    }
    let Some(record) = get_session(connection, conversation_id, session_id)? else {
        return Ok(None);
    };
    // Host reload is a bounded latest-output projection, not the model's incremental cursor.
    // Reading in reverse lets a large number of tiny chunks retain the useful tail while never
    // crossing the Rust/TypeScript 2,048-item contract.
    let mut statement = connection.prepare(
        "SELECT sequence, stream, output
         FROM agent_command_session_output_chunks
         WHERE session_id = ?1 AND sequence > ?2
         ORDER BY sequence DESC
         LIMIT ?3",
    )?;
    let candidates = statement
        .query_map(
            params![
                session_id,
                sqlite_integer(after_sequence)?,
                sqlite_integer(
                    u64::try_from(AGENT_COMMAND_SESSION_MAX_TRANSCRIPT_CHUNKS)
                        .map_err(|_| invalid_input("command transcript chunk limit is invalid"))?
                )?
            ],
            output_chunk_from_row,
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let earliest_requested_sequence = connection.query_row(
        "SELECT MIN(sequence)
         FROM agent_command_session_output_chunks
         WHERE session_id = ?1 AND sequence > ?2",
        params![session_id, sqlite_integer(after_sequence)?],
        |row| row.get::<_, Option<u64>>(0),
    )?;
    let mut chunks_descending = Vec::new();
    let mut bytes = 0_usize;
    for chunk in candidates {
        if !chunks_descending.is_empty() && bytes.saturating_add(chunk.output.len()) > max_bytes {
            break;
        }
        bytes = bytes.saturating_add(chunk.output.len());
        chunks_descending.push(chunk);
    }
    chunks_descending.reverse();
    let chunks = chunks_descending;
    let first_available_sequence = connection.query_row(
        "SELECT MIN(sequence)
             FROM agent_command_session_output_chunks
             WHERE session_id = ?1",
        [session_id],
        |row| row.get::<_, Option<u64>>(0),
    )?;
    let has_gap_after_cursor = chunks
        .first()
        .is_some_and(|chunk| chunk.sequence > after_sequence.saturating_add(1));
    let host_projection_omitted_prefix = earliest_requested_sequence
        .zip(chunks.first().map(|chunk| chunk.sequence))
        .is_some_and(|(earliest, returned)| returned > earliest);
    Ok(Some(AgentCommandSessionTranscript {
        requested_after_sequence: after_sequence,
        first_available_sequence,
        latest_sequence: record.snapshot.latest_sequence,
        truncated_before: (record.transcript_truncated
            && after_sequence < record.snapshot.latest_sequence)
            || has_gap_after_cursor
            || host_projection_omitted_prefix,
        output_capture_truncated: record.output_capture_truncated,
        chunks,
    }))
}

fn read_model_transcript_cut(
    connection: &Connection,
    conversation_id: &str,
    session_id: &str,
    after_sequence: u64,
    max_bytes: usize,
) -> rusqlite::Result<Option<AgentCommandSessionModelTranscriptCut>> {
    if max_bytes == 0 {
        return Err(invalid_input(
            "command session model transcript max_bytes must be positive",
        ));
    }
    let Some(record) = get_session(connection, conversation_id, session_id)? else {
        return Ok(None);
    };
    let mut statement = connection.prepare(
        "SELECT sequence, stream, output
         FROM agent_command_session_output_chunks
         WHERE session_id = ?1 AND sequence > ?2
         ORDER BY sequence ASC
         LIMIT ?3",
    )?;
    let candidates = statement
        .query_map(
            params![
                session_id,
                sqlite_integer(after_sequence)?,
                sqlite_integer(
                    u64::try_from(AGENT_COMMAND_SESSION_MAX_TRANSCRIPT_CHUNKS)
                        .map_err(|_| invalid_input("command transcript chunk limit is invalid"))?
                )?
            ],
            output_chunk_from_row,
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut chunks = Vec::new();
    let mut bytes = 0_usize;
    for chunk in candidates {
        if !chunks.is_empty() && bytes.saturating_add(chunk.output.len()) > max_bytes {
            break;
        }
        bytes = bytes.saturating_add(chunk.output.len());
        chunks.push(chunk);
    }
    let has_gap_after_cursor = chunks
        .first()
        .is_some_and(|chunk| chunk.sequence > after_sequence.saturating_add(1));
    Ok(Some(AgentCommandSessionModelTranscriptCut {
        requested_after_sequence: after_sequence,
        latest_sequence: record.snapshot.latest_sequence,
        truncated_before: (record.transcript_truncated
            && after_sequence < record.snapshot.latest_sequence)
            || has_gap_after_cursor,
        output_capture_truncated: record.output_capture_truncated,
        chunks,
    }))
}

/// Atomically allocates or replays one model-visible transcript cut.
///
/// The receipt stores immutable sequence/status metadata plus a compressed exact chunk payload.
/// Operational Host transcript retention is therefore independent: retrying the same runtime
/// ToolCall remains exact without pinning an unbounded number of SQLite output rows.
pub fn read_or_create_model_read(
    connection: &mut Connection,
    input: &AgentCommandSessionModelReadRequest<'_>,
) -> rusqlite::Result<Option<AgentCommandSessionModelRead>> {
    validate_model_read_request(input)?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if let Some(stored) = get_model_read_receipt_in_connection(
        &transaction,
        input.conversation_id,
        input.session_id,
        input.run_id,
        input.call_id,
    )? {
        validate_model_read_retry(&stored.receipt, input)?;
        let chunks = decode_model_receipt_chunks(&stored)?;
        let receipt = stored.receipt;
        let outputs = if receipt.status.is_terminal() {
            load_published_outputs(&transaction, input.session_id)?
        } else {
            Vec::new()
        };
        transaction.commit()?;
        return Ok(Some(AgentCommandSessionModelRead {
            receipt,
            chunks,
            outputs,
        }));
    }

    let Some(record) =
        get_session_in_connection(&transaction, input.conversation_id, input.session_id)?
    else {
        transaction.commit()?;
        return Ok(None);
    };
    let transcript = read_model_transcript_cut(
        &transaction,
        input.conversation_id,
        input.session_id,
        record.model_read_sequence,
        input.max_output_bytes,
    )?
    .ok_or_else(|| invalid_input("command session disappeared while allocating model receipt"))?;
    let output = transcript
        .chunks
        .iter()
        .map(|chunk| chunk.output.as_str())
        .collect::<String>();
    let receipt = AgentCommandSessionModelReadReceipt {
        conversation_id: input.conversation_id.to_string(),
        session_id: input.session_id.to_string(),
        run_id: input.run_id.to_string(),
        call_id: input.call_id.to_string(),
        action: input.action,
        max_output_bytes: input.max_output_bytes,
        requested_after_sequence: transcript.requested_after_sequence,
        first_output_sequence: transcript.chunks.first().map(|chunk| chunk.sequence),
        last_output_sequence: transcript.chunks.last().map(|chunk| chunk.sequence),
        status: record.snapshot.status,
        exit_code: record.snapshot.exit_code,
        latest_sequence: transcript.latest_sequence,
        truncated_before: transcript.truncated_before,
        output_truncated: record.snapshot.output_truncated
            || transcript.truncated_before
            || transcript.output_capture_truncated
            || input.host_output_truncated,
        output_bytes: output.len(),
        output_hash: format!("{:x}", Sha256::digest(output.as_bytes())),
        created_at: input.created_at,
    };
    insert_model_read_receipt_in_connection(&transaction, &receipt, &transcript.chunks)?;
    if let Some(last_sequence) = receipt.last_output_sequence {
        let advanced = advance_model_read_sequence(
            &transaction,
            input.conversation_id,
            input.session_id,
            receipt.requested_after_sequence,
            last_sequence,
            input.created_at,
        )?;
        if !advanced {
            return Err(invalid_input(
                "command session model cursor changed while allocating a receipt",
            ));
        }
    }
    prune_model_read_receipts_in_connection(&transaction, input.session_id)?;
    if prune_transcript_in_connection(&transaction, input.session_id)? {
        transaction.execute(
            "UPDATE agent_command_sessions
             SET transcript_truncated = 1
             WHERE conversation_id = ?1 AND session_id = ?2",
            params![input.conversation_id, input.session_id],
        )?;
    }
    let outputs = if receipt.status.is_terminal() {
        load_published_outputs(&transaction, input.session_id)?
    } else {
        Vec::new()
    };
    transaction.commit()?;
    Ok(Some(AgentCommandSessionModelRead {
        receipt,
        chunks: transcript.chunks,
        outputs,
    }))
}

/// Replays a previously committed model transcript cut without allocating a new one.
///
/// Callers use this before performing process control so a retried ToolCall cannot send a second
/// interrupt or wait for output which was already observed by the original execution.
pub fn load_model_read(
    connection: &Connection,
    input: &AgentCommandSessionModelReadRequest<'_>,
) -> rusqlite::Result<Option<AgentCommandSessionModelRead>> {
    validate_model_read_request(input)?;
    let Some(stored) = get_model_read_receipt_in_connection(
        connection,
        input.conversation_id,
        input.session_id,
        input.run_id,
        input.call_id,
    )?
    else {
        return Ok(None);
    };
    validate_model_read_retry(&stored.receipt, input)?;
    let chunks = decode_model_receipt_chunks(&stored)?;
    let receipt = stored.receipt;
    let outputs = if receipt.status.is_terminal() {
        load_published_outputs(connection, input.session_id)?
    } else {
        Vec::new()
    };
    Ok(Some(AgentCommandSessionModelRead {
        receipt,
        chunks,
        outputs,
    }))
}

pub fn advance_model_read_sequence(
    connection: &Connection,
    conversation_id: &str,
    session_id: &str,
    expected_sequence: u64,
    next_sequence: u64,
    updated_at: i64,
) -> rusqlite::Result<bool> {
    if next_sequence < expected_sequence {
        return Err(invalid_input(
            "command session model read sequence cannot move backwards",
        ));
    }
    let affected = connection.execute(
        "UPDATE agent_command_sessions
         SET model_read_sequence = ?1, updated_at = MAX(updated_at, ?2)
         WHERE conversation_id = ?3 AND session_id = ?4
           AND model_read_sequence = ?5 AND latest_sequence >= ?1",
        params![
            sqlite_integer(next_sequence)?,
            updated_at,
            conversation_id,
            session_id,
            sqlite_integer(expected_sequence)?,
        ],
    )?;
    Ok(affected == 1)
}

fn get_model_read_receipt_in_connection(
    connection: &Connection,
    conversation_id: &str,
    session_id: &str,
    run_id: &str,
    call_id: &str,
) -> rusqlite::Result<Option<StoredAgentCommandSessionModelReadReceipt>> {
    connection
        .query_row(
            "SELECT
                 conversation_id, session_id, run_id, call_id, action,
                 max_output_bytes, requested_after_sequence,
                 first_output_sequence, last_output_sequence, status, exit_code,
                 latest_sequence, truncated_before, output_truncated,
                 output_bytes, output_hash, created_at,
                 output_payload_compression, output_payload, output_chunk_count
             FROM agent_command_session_model_read_receipts
             WHERE conversation_id = ?1 AND session_id = ?2
               AND run_id = ?3 AND call_id = ?4",
            params![conversation_id, session_id, run_id, call_id],
            stored_model_read_receipt_from_row,
        )
        .optional()
}

fn insert_model_read_receipt_in_connection(
    connection: &Connection,
    receipt: &AgentCommandSessionModelReadReceipt,
    chunks: &[AgentCommandSessionOutputChunk],
) -> rusqlite::Result<()> {
    let encoded = encode_command_session_receipt_payload(chunks)
        .map_err(|error| invalid_input(format!("command session receipt payload: {error}")))?;
    connection.execute(
        "INSERT INTO agent_command_session_model_read_receipts (
             conversation_id, session_id, run_id, call_id, action,
             max_output_bytes, requested_after_sequence,
             first_output_sequence, last_output_sequence, status, exit_code,
             latest_sequence, truncated_before, output_truncated,
             output_bytes, output_hash, created_at,
             output_payload_compression, output_payload, output_chunk_count
         ) VALUES (
             ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
             ?14, ?15, ?16, ?17, ?18, ?19, ?20
         )",
        params![
            &receipt.conversation_id,
            &receipt.session_id,
            &receipt.run_id,
            &receipt.call_id,
            model_read_action_as_str(receipt.action),
            sqlite_integer(receipt.max_output_bytes as u64)?,
            sqlite_integer(receipt.requested_after_sequence)?,
            receipt
                .first_output_sequence
                .map(sqlite_integer)
                .transpose()?,
            receipt
                .last_output_sequence
                .map(sqlite_integer)
                .transpose()?,
            status_as_str(receipt.status),
            receipt.exit_code,
            sqlite_integer(receipt.latest_sequence)?,
            receipt.truncated_before,
            receipt.output_truncated,
            sqlite_integer(receipt.output_bytes as u64)?,
            &receipt.output_hash,
            receipt.created_at,
            encoded.compression,
            encoded.payload,
            sqlite_integer(
                u64::try_from(encoded.chunk_count)
                    .map_err(|_| invalid_input("command session receipt chunk count is invalid"))?
            )?,
        ],
    )?;
    Ok(())
}

fn decode_model_receipt_chunks(
    stored: &StoredAgentCommandSessionModelReadReceipt,
) -> rusqlite::Result<Vec<AgentCommandSessionOutputChunk>> {
    let receipt = &stored.receipt;
    let chunks = decode_command_session_receipt_payload(
        &stored.payload_compression,
        &stored.payload,
        stored.chunk_count,
    )
    .map_err(|error| corrupt_data(0, Type::Blob, &error))?;
    let (Some(first), Some(last)) = (receipt.first_output_sequence, receipt.last_output_sequence)
    else {
        if receipt.first_output_sequence.is_some()
            || receipt.last_output_sequence.is_some()
            || receipt.output_bytes != 0
            || receipt.output_hash != format!("{:x}", Sha256::digest([]))
        {
            return Err(corrupt_data(
                0,
                Type::Text,
                "empty command session model receipt has inconsistent output metadata",
            ));
        }
        if !chunks.is_empty() {
            return Err(corrupt_data(
                0,
                Type::Blob,
                "empty command session model receipt contains output chunks",
            ));
        }
        return Ok(chunks);
    };
    let output = chunks
        .iter()
        .map(|chunk| chunk.output.as_str())
        .collect::<String>();
    if chunks.first().map(|chunk| chunk.sequence) != Some(first)
        || chunks.last().map(|chunk| chunk.sequence) != Some(last)
        || output.len() != receipt.output_bytes
        || format!("{:x}", Sha256::digest(output.as_bytes())) != receipt.output_hash
    {
        return Err(corrupt_data(
            0,
            Type::Text,
            "command session model receipt payload does not match its immutable metadata",
        ));
    }
    Ok(chunks)
}

fn prune_model_read_receipts_in_connection(
    connection: &Connection,
    session_id: &str,
) -> rusqlite::Result<usize> {
    connection.execute(
        "DELETE FROM agent_command_session_model_read_receipts
         WHERE receipt_id IN (
             SELECT receipt_id
             FROM agent_command_session_model_read_receipts
             WHERE session_id = ?1
             ORDER BY receipt_id DESC
             LIMIT -1 OFFSET ?2
         )",
        params![
            session_id,
            i64::try_from(MAX_RETAINED_MODEL_READ_RECEIPTS_PER_SESSION)
                .map_err(|_| invalid_input("command session receipt retention limit is invalid"))?,
        ],
    )
}

/// Marks process-owned sessions from a previous Host instance as outcome-unknown.
///
/// No process is replayed and no terminal success is inferred. Bounded transcript rows remain
/// intact for inspection. The caller is responsible for appending the matching audit-only trace
/// event before orphaned in-progress Agent traces are reconciled.
pub fn reconcile_active_sessions_on_startup(
    connection: &mut Connection,
    reconciled_at: i64,
) -> rusqlite::Result<Vec<AgentCommandSessionRecord>> {
    validate_timestamp(reconciled_at, "command session reconciliation time")?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let active = list_active_sessions_in_connection(&transaction)?;
    let mut records = Vec::with_capacity(active.len());
    for record in active {
        if let Some(reconciled) = reconcile_active_session_in_connection(
            &transaction,
            &record.snapshot.conversation_id,
            &record.snapshot.session_id,
            reconciled_at,
        )? {
            records.push(reconciled);
        }
    }
    transaction.commit()?;
    Ok(records)
}

pub(crate) fn list_active_sessions_in_connection(
    connection: &Connection,
) -> rusqlite::Result<Vec<AgentCommandSessionRecord>> {
    let mut statement = connection.prepare(&format!(
        "{} WHERE status IN ('starting', 'running')
         ORDER BY started_at ASC, session_id ASC",
        record_select()
    ))?;
    let records = statement
        .query_map([], record_from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(records)
}

/// Lists every unresolved Session whose OS-process outcome became unknowable after Host restart.
///
/// These rows are the authoritative durable deletion fence. They deliberately remain distinct
/// from ordinary bounded terminal history until an explicit future reconciliation flow resolves
/// the uncertainty.
pub(crate) fn list_outcome_unknown_sessions_in_connection(
    connection: &Connection,
) -> rusqlite::Result<Vec<AgentCommandSessionRecord>> {
    let mut statement = connection.prepare(&format!(
        "{} WHERE status = 'outcome_unknown'
         ORDER BY started_at ASC, session_id ASC",
        record_select()
    ))?;
    let records = statement
        .query_map([], record_from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(records)
}

/// Proves only that the exact Host-owned process for one approved command can no longer run.
///
/// This is deliberately narrower than action settlement: it does not claim that the command
/// succeeded, that its ToolResult was materialized, or that its file effects are known. It lets
/// startup reconciliation release the active-process deletion fence after the pending action has
/// already been retired to a terminal target, while `outcome_unknown` remains fenced.
pub(crate) fn has_resolved_terminal_session_for_action(
    connection: &Connection,
    conversation_id: &str,
    assistant_message_id: &str,
    origin_run_id: &str,
    call_id: &str,
    command: &str,
) -> rusqlite::Result<bool> {
    if [
        conversation_id,
        assistant_message_id,
        origin_run_id,
        call_id,
        command,
    ]
    .iter()
    .any(|value| value.trim().is_empty())
    {
        return Ok(false);
    }
    connection.query_row(
        "SELECT EXISTS (
                 SELECT 1
                 FROM agent_command_sessions
                 WHERE conversation_id = ?1
                   AND assistant_message_id = ?2
                   AND origin_run_id = ?3
                   AND call_id = ?4
                   AND command_projection = ?5
                   AND status IN ('exited', 'failed', 'timed_out', 'interrupted')
             )",
        params![
            conversation_id,
            assistant_message_id,
            origin_run_id,
            call_id,
            command
        ],
        |row| row.get(0),
    )
}

pub(crate) fn reconcile_active_session_in_connection(
    connection: &Connection,
    conversation_id: &str,
    session_id: &str,
    reconciled_at: i64,
) -> rusqlite::Result<Option<AgentCommandSessionRecord>> {
    validate_identity(conversation_id, session_id)?;
    validate_timestamp(reconciled_at, "command session reconciliation time")?;
    let affected = connection.execute(
        "UPDATE agent_command_sessions
         SET status = 'outcome_unknown', ended_at = ?1,
             terminal_reason = 'Host restarted; the original OS process is no longer controllable and its final outcome is unknown.',
             updated_at = ?1, settled_at = ?1
         WHERE conversation_id = ?2 AND session_id = ?3
           AND status IN ('starting', 'running')",
        params![reconciled_at, conversation_id, session_id],
    )?;
    if affected == 0 {
        return Ok(None);
    }
    get_session_in_connection(connection, conversation_id, session_id)
}

/// Retains a fixed number of recent, resolved terminal operational rows per conversation.
///
/// A Session with an unmaterialized lifecycle sidecar is never eligible: deleting it would
/// cascade the only durable audit event before the owning Trace reaches a terminal state. Exact
/// History and materialized Trace items are not children of the Session row and remain intact.
/// `outcome_unknown` is unresolved process/file-effect uncertainty rather than disposable history,
/// so it is pinned until an explicit future reconciliation flow resolves it.
pub(crate) fn prune_terminal_sessions_in_connection(
    connection: &Connection,
    conversation_id: &str,
) -> rusqlite::Result<usize> {
    if conversation_id.trim().is_empty() {
        return Err(invalid_input("command session conversation id is empty"));
    }
    connection.execute(
        "DELETE FROM agent_command_sessions
         WHERE session_id IN (
             SELECT session.session_id
             FROM agent_command_sessions AS session
             WHERE session.conversation_id = ?1
               AND session.status NOT IN ('starting', 'running', 'outcome_unknown')
               AND NOT EXISTS (
                   SELECT 1
                   FROM agent_command_session_lifecycle_events AS lifecycle
                   WHERE lifecycle.session_id = session.session_id
                     AND lifecycle.trace_sequence IS NULL
               )
             ORDER BY
                 COALESCE(session.ended_at, session.updated_at) DESC,
                 session.updated_at DESC,
                 session.session_id DESC
             LIMIT -1 OFFSET ?2
         )",
        params![
            conversation_id,
            i64::try_from(MAX_RETAINED_TERMINAL_COMMAND_SESSIONS_PER_CONVERSATION)
                .map_err(|_| invalid_input("command session retention limit is invalid"))?,
        ],
    )
}

fn get_session_in_connection(
    connection: &Connection,
    conversation_id: &str,
    session_id: &str,
) -> rusqlite::Result<Option<AgentCommandSessionRecord>> {
    let mut record = connection
        .query_row(
            &format!(
                "{} WHERE conversation_id = ?1 AND session_id = ?2",
                record_select()
            ),
            params![conversation_id, session_id],
            record_from_row,
        )
        .optional()?;
    if let Some(record) = record.as_mut() {
        if record.snapshot.status.is_terminal() {
            record.snapshot.outputs = load_published_outputs(connection, session_id)?;
        }
    }
    Ok(record)
}

fn attach_terminal_outputs(
    connection: &Connection,
    records: &mut [AgentCommandSessionRecord],
) -> rusqlite::Result<()> {
    for record in records {
        if record.snapshot.status.is_terminal() {
            record.snapshot.outputs =
                load_published_outputs(connection, &record.snapshot.session_id)?;
        }
    }
    Ok(())
}

fn session_for_call_exists(
    connection: &Connection,
    conversation_id: &str,
    assistant_message_id: &str,
    call_id: &str,
) -> rusqlite::Result<bool> {
    connection.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM agent_command_sessions
             WHERE conversation_id = ?1 AND assistant_message_id = ?2 AND call_id = ?3
         )",
        params![conversation_id, assistant_message_id, call_id],
        |row| row.get(0),
    )
}

fn prune_transcript_in_connection(
    connection: &Connection,
    session_id: &str,
) -> rusqlite::Result<bool> {
    let mut statement = connection.prepare(
        "SELECT sequence, output_bytes
         FROM agent_command_session_output_chunks
         WHERE session_id = ?1
         ORDER BY sequence ASC",
    )?;
    let rows = statement
        .query_map([session_id], |row| {
            Ok((row.get::<_, u64>(0)?, row.get::<_, usize>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let total = rows
        .iter()
        .fold(0_usize, |total, (_, bytes)| total.saturating_add(*bytes));
    if total <= MAX_PERSISTED_COMMAND_TRANSCRIPT_BYTES
        && rows.len() <= AGENT_COMMAND_SESSION_MAX_TRANSCRIPT_CHUNKS
    {
        return Ok(false);
    }

    let mut keep = HashSet::new();
    let mut head_bytes = 0_usize;
    let mut head_chunks = 0_usize;
    for (sequence, bytes) in &rows {
        if head_bytes > 0
            && head_bytes.saturating_add(*bytes) > PERSISTED_COMMAND_TRANSCRIPT_HEAD_BYTES
        {
            break;
        }
        if head_chunks >= PERSISTED_COMMAND_TRANSCRIPT_HEAD_CHUNKS
            || keep.len() >= AGENT_COMMAND_SESSION_MAX_TRANSCRIPT_CHUNKS
        {
            break;
        }
        keep.insert(*sequence);
        head_bytes = head_bytes.saturating_add(*bytes);
        head_chunks = head_chunks.saturating_add(1);
    }
    let tail_limit = MAX_PERSISTED_COMMAND_TRANSCRIPT_BYTES
        .saturating_sub(PERSISTED_COMMAND_TRANSCRIPT_HEAD_BYTES);
    let mut tail_bytes = 0_usize;
    for (sequence, bytes) in rows.iter().rev() {
        if keep.contains(sequence) {
            continue;
        }
        if tail_bytes > 0 && tail_bytes.saturating_add(*bytes) > tail_limit {
            break;
        }
        if keep.len() >= AGENT_COMMAND_SESSION_MAX_TRANSCRIPT_CHUNKS {
            break;
        }
        keep.insert(*sequence);
        tail_bytes = tail_bytes.saturating_add(*bytes);
    }
    for (sequence, _) in rows {
        if !keep.contains(&sequence) {
            connection.execute(
                "DELETE FROM agent_command_session_output_chunks
                 WHERE session_id = ?1 AND sequence = ?2",
                params![session_id, sqlite_integer(sequence)?],
            )?;
        }
    }
    Ok(true)
}

fn record_select() -> &'static str {
    "SELECT
         session_id, schema_version, conversation_id, assistant_message_id,
         origin_run_id, call_id, project_id, command_projection, cwd_projection,
         command_digest, authorization_source, approval_provenance_json,
         permission_provenance_json, status, started_at, ended_at, exit_code,
         latest_sequence, model_read_sequence, transcript_truncated,
         output_capture_truncated, archive_ref, terminal_reason,
         created_at, updated_at, settled_at
     FROM agent_command_sessions"
}

fn record_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<AgentCommandSessionRecord> {
    let status = status_from_str(&row.get::<_, String>(13)?)?;
    let authorization_source = authorization_source_from_str(&row.get::<_, String>(10)?)?;
    let approval_provenance = deserialize_json(11, row.get::<_, String>(11)?)?;
    let permission_provenance = deserialize_json(12, row.get::<_, String>(12)?)?;
    let transcript_truncated = row.get::<_, bool>(19)?;
    let output_capture_truncated = row.get::<_, bool>(20)?;
    Ok(AgentCommandSessionRecord {
        snapshot: AgentCommandSessionSnapshot {
            schema_version: row.get(1)?,
            session_id: row.get(0)?,
            conversation_id: row.get(2)?,
            assistant_message_id: row.get(3)?,
            origin_run_id: row.get(4)?,
            call_id: row.get(5)?,
            project_id: row.get(6)?,
            command: row.get(7)?,
            cwd: row.get(8)?,
            command_digest: row.get(9)?,
            status,
            started_at: row.get(14)?,
            ended_at: row.get(15)?,
            exit_code: row.get(16)?,
            latest_sequence: row.get(17)?,
            output_truncated: transcript_truncated || output_capture_truncated,
            outputs: Vec::new(),
            archive_ref: row.get(21)?,
        },
        authorization_source,
        approval_provenance,
        permission_provenance,
        model_read_sequence: row.get(18)?,
        transcript_truncated,
        output_capture_truncated,
        terminal_reason: row.get(22)?,
        created_at: row.get(23)?,
        updated_at: row.get(24)?,
        settled_at: row.get(25)?,
    })
}

fn output_chunk_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<AgentCommandSessionOutputChunk> {
    Ok(AgentCommandSessionOutputChunk {
        sequence: row.get(0)?,
        stream: output_stream_from_str(&row.get::<_, String>(1)?)?,
        output: row.get(2)?,
    })
}

fn insert_published_outputs(
    connection: &Connection,
    session_id: &str,
    outputs: &[AgentCommandPublishedOutput],
) -> rusqlite::Result<()> {
    if outputs.len() > 32 {
        return Err(invalid_input(
            "command session published output count exceeds the durable limit",
        ));
    }
    for (ordinal, output) in outputs.iter().enumerate() {
        validate_published_output(output)?;
        connection.execute(
            "INSERT INTO agent_command_session_published_outputs (
                session_id, ordinal, name, kind, read_path, mime_type,
                size_bytes, sha256, width, height
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                session_id,
                sqlite_integer(ordinal as u64)?,
                &output.name,
                published_output_kind_as_str(output.kind),
                &output.read_path,
                &output.mime_type,
                sqlite_integer(output.size_bytes)?,
                &output.sha256,
                output.width.map(i64::from),
                output.height.map(i64::from),
            ],
        )?;
    }
    Ok(())
}

fn load_published_outputs(
    connection: &Connection,
    session_id: &str,
) -> rusqlite::Result<Vec<AgentCommandPublishedOutput>> {
    let mut statement = connection.prepare(
        "SELECT name, kind, read_path, mime_type, size_bytes, sha256, width, height
         FROM agent_command_session_published_outputs
         WHERE session_id = ?1 ORDER BY ordinal ASC",
    )?;
    let outputs = statement
        .query_map([session_id], |row| {
            let size_bytes = u64::try_from(row.get::<_, i64>(4)?).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(4, Type::Integer, Box::new(error))
            })?;
            let width = row
                .get::<_, Option<i64>>(6)?
                .map(u32::try_from)
                .transpose()
                .map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(6, Type::Integer, Box::new(error))
                })?;
            let height = row
                .get::<_, Option<i64>>(7)?
                .map(u32::try_from)
                .transpose()
                .map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(7, Type::Integer, Box::new(error))
                })?;
            Ok(AgentCommandPublishedOutput {
                name: row.get(0)?,
                kind: published_output_kind_from_str(&row.get::<_, String>(1)?)?,
                read_path: row.get(2)?,
                mime_type: row.get(3)?,
                size_bytes,
                sha256: row.get(5)?,
                width,
                height,
            })
        })?
        .collect();
    outputs
}

fn published_output_kind_as_str(kind: AgentCommandPublishedOutputKind) -> &'static str {
    match kind {
        AgentCommandPublishedOutputKind::Image => "image",
        AgentCommandPublishedOutputKind::Document => "document",
    }
}

fn published_output_kind_from_str(
    value: &str,
) -> rusqlite::Result<AgentCommandPublishedOutputKind> {
    match value {
        "image" => Ok(AgentCommandPublishedOutputKind::Image),
        "document" => Ok(AgentCommandPublishedOutputKind::Document),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}

fn validate_published_output(output: &AgentCommandPublishedOutput) -> rusqlite::Result<()> {
    let read_path_prefix = match output.kind {
        AgentCommandPublishedOutputKind::Image => "image-artifact://sha256/",
        AgentCommandPublishedOutputKind::Document => "artifact://sha256/",
    };
    if output.name.is_empty()
        || output.name.len() > 1024
        || output.name.chars().any(char::is_control)
        || output.read_path != format!("{read_path_prefix}{}", output.sha256)
        || output.sha256.len() != 64
        || !output
            .sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        || output.mime_type.is_empty()
        || output.mime_type.len() > 128
        || output.size_bytes == 0
        || matches!(output.kind, AgentCommandPublishedOutputKind::Image)
            != (output.width.is_some() && output.height.is_some())
    {
        return Err(invalid_input(
            "command session published output receipt is invalid",
        ));
    }
    Ok(())
}

fn model_read_receipt_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<AgentCommandSessionModelReadReceipt> {
    Ok(AgentCommandSessionModelReadReceipt {
        conversation_id: row.get(0)?,
        session_id: row.get(1)?,
        run_id: row.get(2)?,
        call_id: row.get(3)?,
        action: model_read_action_from_str(&row.get::<_, String>(4)?)?,
        max_output_bytes: row.get(5)?,
        requested_after_sequence: row.get(6)?,
        first_output_sequence: row.get(7)?,
        last_output_sequence: row.get(8)?,
        status: status_from_str(&row.get::<_, String>(9)?)?,
        exit_code: row.get(10)?,
        latest_sequence: row.get(11)?,
        truncated_before: row.get(12)?,
        output_truncated: row.get(13)?,
        output_bytes: row.get(14)?,
        output_hash: row.get(15)?,
        created_at: row.get(16)?,
    })
}

fn stored_model_read_receipt_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<StoredAgentCommandSessionModelReadReceipt> {
    Ok(StoredAgentCommandSessionModelReadReceipt {
        receipt: model_read_receipt_from_row(row)?,
        payload_compression: row.get(17)?,
        payload: row.get(18)?,
        chunk_count: row.get(19)?,
    })
}

fn validate_create(input: &AgentCommandSessionCreate) -> rusqlite::Result<()> {
    let snapshot = &input.snapshot;
    validate_identity(&snapshot.conversation_id, &snapshot.session_id)?;
    for (label, value) in [
        (
            "assistant message id",
            snapshot.assistant_message_id.as_str(),
        ),
        ("origin run id", snapshot.origin_run_id.as_str()),
        ("call id", snapshot.call_id.as_str()),
        ("command projection", snapshot.command.as_str()),
        ("cwd projection", snapshot.cwd.as_str()),
    ] {
        if value.trim().is_empty() {
            return Err(invalid_input(format!("command session {label} is empty")));
        }
    }
    if snapshot.schema_version != AGENT_COMMAND_SESSION_SCHEMA_VERSION
        || !matches!(
            snapshot.status,
            AgentCommandSessionStatus::Starting | AgentCommandSessionStatus::Running
        )
        || snapshot.ended_at.is_some()
        || snapshot.exit_code.is_some()
        || snapshot.archive_ref.is_some()
        || !snapshot.outputs.is_empty()
        || snapshot.latest_sequence != 0
        || snapshot.command_digest.len() != 71
        || !snapshot.command_digest.starts_with("sha256:")
        || !snapshot.command_digest[7..]
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(invalid_input("command session initial snapshot is invalid"));
    }
    validate_timestamp(input.created_at, "command session created_at")?;
    sqlite_integer(snapshot.started_at)?;
    serialize_json(&input.approval_provenance)?;
    serialize_json(&input.permission_provenance)?;
    Ok(())
}

fn validate_idempotent_create(
    existing: &AgentCommandSessionRecord,
    input: &AgentCommandSessionCreate,
) -> rusqlite::Result<()> {
    let expected = &input.snapshot;
    if existing.snapshot.conversation_id != expected.conversation_id
        || existing.snapshot.assistant_message_id != expected.assistant_message_id
        || existing.snapshot.origin_run_id != expected.origin_run_id
        || existing.snapshot.call_id != expected.call_id
        || existing.snapshot.project_id != expected.project_id
        || existing.snapshot.command != expected.command
        || existing.snapshot.cwd != expected.cwd
        || existing.snapshot.command_digest != expected.command_digest
        || existing.authorization_source != input.authorization_source
        || existing.approval_provenance != input.approval_provenance
        || existing.permission_provenance != input.permission_provenance
        || existing.snapshot.started_at != expected.started_at
    {
        return Err(invalid_input(
            "command session id was reused for a different identity",
        ));
    }
    Ok(())
}

fn validate_output_chunks(
    chunks: &[AgentCommandSessionOutputChunk],
    latest_sequence: u64,
) -> rusqlite::Result<()> {
    let mut previous = None;
    for chunk in chunks {
        if chunk.sequence == 0
            || chunk.sequence > latest_sequence
            || previous.is_some_and(|sequence| chunk.sequence <= sequence)
            || chunk.output.is_empty()
            || chunk.output.len() > MAX_PERSISTED_COMMAND_OUTPUT_CHUNK_BYTES
        {
            return Err(invalid_input("command session output chunk is invalid"));
        }
        previous = Some(chunk.sequence);
    }
    Ok(())
}

fn validate_model_read_request(
    input: &AgentCommandSessionModelReadRequest<'_>,
) -> rusqlite::Result<()> {
    validate_identity(input.conversation_id, input.session_id)?;
    validate_timestamp(input.created_at, "command session model receipt created_at")?;
    for (label, value) in [("run id", input.run_id), ("call id", input.call_id)] {
        if value.trim().is_empty() || value.len() > 1_024 || value.chars().any(char::is_control) {
            return Err(invalid_input(format!(
                "command session model receipt {label} is invalid"
            )));
        }
    }
    if input.max_output_bytes == 0
        || input.max_output_bytes > AGENT_COMMAND_SESSION_MODEL_OUTPUT_BYTES
    {
        return Err(invalid_input(
            "command session model receipt output limit is invalid",
        ));
    }
    Ok(())
}

fn validate_model_read_retry(
    receipt: &AgentCommandSessionModelReadReceipt,
    input: &AgentCommandSessionModelReadRequest<'_>,
) -> rusqlite::Result<()> {
    if receipt.action != input.action || receipt.max_output_bytes != input.max_output_bytes {
        return Err(invalid_input(
            "command session model ToolCall identity was reused with different arguments",
        ));
    }
    Ok(())
}

fn validate_terminal_update(input: &AgentCommandSessionTerminalUpdate<'_>) -> rusqlite::Result<()> {
    validate_identity(input.conversation_id, input.session_id)?;
    validate_timestamp(input.committed_at, "command session terminal committed_at")?;
    if !input.status.is_terminal()
        || input.status == AgentCommandSessionStatus::OutcomeUnknown
        || input.ended_at > i64::MAX as u64
        || (input.status != AgentCommandSessionStatus::Exited && input.exit_code.is_some())
    {
        return Err(invalid_input("command session terminal update is invalid"));
    }
    Ok(())
}

fn validate_identity(conversation_id: &str, session_id: &str) -> rusqlite::Result<()> {
    let session_hex = session_id.strip_prefix("cmd_");
    if conversation_id.trim().is_empty()
        || session_hex.is_none_or(|value| {
            value.len() != 32 || !value.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
    {
        return Err(invalid_input("command session identity is invalid"));
    }
    Ok(())
}

fn validate_timestamp(value: i64, label: &str) -> rusqlite::Result<()> {
    if value < 0 {
        return Err(invalid_input(format!("{label} cannot be negative")));
    }
    Ok(())
}

fn status_as_str(status: AgentCommandSessionStatus) -> &'static str {
    match status {
        AgentCommandSessionStatus::Starting => "starting",
        AgentCommandSessionStatus::Running => "running",
        AgentCommandSessionStatus::Exited => "exited",
        AgentCommandSessionStatus::Interrupted => "interrupted",
        AgentCommandSessionStatus::TimedOut => "timed_out",
        AgentCommandSessionStatus::Failed => "failed",
        AgentCommandSessionStatus::OutcomeUnknown => "outcome_unknown",
    }
}

fn status_from_str(value: &str) -> rusqlite::Result<AgentCommandSessionStatus> {
    match value {
        "starting" => Ok(AgentCommandSessionStatus::Starting),
        "running" => Ok(AgentCommandSessionStatus::Running),
        "exited" => Ok(AgentCommandSessionStatus::Exited),
        "interrupted" => Ok(AgentCommandSessionStatus::Interrupted),
        "timed_out" => Ok(AgentCommandSessionStatus::TimedOut),
        "failed" => Ok(AgentCommandSessionStatus::Failed),
        "outcome_unknown" => Ok(AgentCommandSessionStatus::OutcomeUnknown),
        _ => Err(corrupt_data(
            13,
            Type::Text,
            format!("unknown command session status: {value}"),
        )),
    }
}

fn authorization_source_as_str(source: CommandAuthorizationSource) -> &'static str {
    match source {
        CommandAuthorizationSource::Automatic => "automatic",
        CommandAuthorizationSource::ExplicitUser => "explicit_user",
    }
}

fn authorization_source_from_str(value: &str) -> rusqlite::Result<CommandAuthorizationSource> {
    match value {
        "automatic" => Ok(CommandAuthorizationSource::Automatic),
        "explicit_user" => Ok(CommandAuthorizationSource::ExplicitUser),
        _ => Err(corrupt_data(
            10,
            Type::Text,
            format!("unknown command authorization source: {value}"),
        )),
    }
}

fn model_read_action_as_str(action: AgentCommandSessionAction) -> &'static str {
    match action {
        AgentCommandSessionAction::Poll => "poll",
        AgentCommandSessionAction::Interrupt => "interrupt",
    }
}

fn model_read_action_from_str(value: &str) -> rusqlite::Result<AgentCommandSessionAction> {
    match value {
        "poll" => Ok(AgentCommandSessionAction::Poll),
        "interrupt" => Ok(AgentCommandSessionAction::Interrupt),
        _ => Err(corrupt_data(
            4,
            Type::Text,
            format!("unknown command session model action: {value}"),
        )),
    }
}

fn output_stream_as_str(stream: AgentCommandOutputStream) -> &'static str {
    match stream {
        AgentCommandOutputStream::Stdout => "stdout",
        AgentCommandOutputStream::Stderr => "stderr",
    }
}

fn output_stream_from_str(value: &str) -> rusqlite::Result<AgentCommandOutputStream> {
    match value {
        "stdout" => Ok(AgentCommandOutputStream::Stdout),
        "stderr" => Ok(AgentCommandOutputStream::Stderr),
        _ => Err(corrupt_data(
            1,
            Type::Text,
            format!("unknown command output stream: {value}"),
        )),
    }
}

fn serialize_json(value: &Value) -> rusqlite::Result<String> {
    serde_json::to_string(value)
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))
}

fn deserialize_json(column: usize, value: String) -> rusqlite::Result<Value> {
    serde_json::from_str(&value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(column, Type::Text, Box::new(error))
    })
}

fn sqlite_integer(value: u64) -> rusqlite::Result<i64> {
    i64::try_from(value).map_err(|_| invalid_input("command session integer exceeds SQLite range"))
}

fn invalid_input(message: impl Into<String>) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(Box::new(IoError::new(
        ErrorKind::InvalidInput,
        message.into(),
    )))
}

fn corrupt_data(column: usize, data_type: Type, message: impl Into<String>) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        column,
        data_type,
        Box::new(IoError::new(ErrorKind::InvalidData, message.into())),
    )
}

#[cfg(test)]
mod tests;
