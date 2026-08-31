use super::common::{positive_u64, read_error, validate_schema_version};
use crate::{AgentGraphError, AgentWakeRequestRecord, AgentWakeStatus};
use rusqlite::{params, Connection, OptionalExtension};

pub(super) const WAKE_SELECT: &str = "
    SELECT sequence, wake_id, schema_version, root_agent_id, agent_id,
           requester_agent_id, request_id, source_agent_message_id, status,
           status_revision, claim_token, lease_expires_at, result_message_id, terminal_error,
           run_id, assistant_message_id, created_at, claimed_at, started_at, completed_at
    FROM agent_wake_requests";

pub(super) fn query_wake(
    connection: &Connection,
    wake_id: &str,
) -> Result<Option<AgentWakeRequestRecord>, AgentGraphError> {
    connection
        .query_row(
            &format!("{WAKE_SELECT} WHERE wake_id = ?1"),
            [wake_id],
            read_wake_row,
        )
        .optional()
        .map_err(read_error)?
        .map(decode_wake)
        .transpose()
}

pub(super) fn query_wakes<P: rusqlite::Params>(
    connection: &Connection,
    sql: &str,
    params: P,
) -> Result<Vec<AgentWakeRequestRecord>, AgentGraphError> {
    let mut statement = connection.prepare(sql).map_err(read_error)?;
    let rows = statement
        .query_map(params, read_wake_row)
        .map_err(read_error)?;
    rows.map(|row| row.map_err(read_error).and_then(decode_wake))
        .collect()
}

pub(super) fn query_wake_by_request(
    connection: &Connection,
    requester_agent_id: &str,
    request_id: &str,
) -> Result<Option<AgentWakeRequestRecord>, AgentGraphError> {
    connection
        .query_row(
            &format!("{WAKE_SELECT} WHERE requester_agent_id = ?1 AND request_id = ?2"),
            params![requester_agent_id, request_id],
            read_wake_row,
        )
        .optional()
        .map_err(read_error)?
        .map(decode_wake)
        .transpose()
}

pub(super) fn query_wake_by_claim_token(
    connection: &Connection,
    claim_token: &str,
) -> Result<Option<AgentWakeRequestRecord>, AgentGraphError> {
    connection
        .query_row(
            &format!("{WAKE_SELECT} WHERE claim_token = ?1"),
            [claim_token],
            read_wake_row,
        )
        .optional()
        .map_err(read_error)?
        .map(decode_wake)
        .transpose()
}

pub(super) fn read_wake_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<WakeRow> {
    Ok(WakeRow {
        sequence: row.get(0)?,
        wake_id: row.get(1)?,
        schema_version: row.get(2)?,
        root_agent_id: row.get(3)?,
        agent_id: row.get(4)?,
        requester_agent_id: row.get(5)?,
        request_id: row.get(6)?,
        source_agent_message_id: row.get(7)?,
        status: row.get(8)?,
        status_revision: row.get(9)?,
        claim_token: row.get(10)?,
        lease_expires_at: row.get(11)?,
        result_message_id: row.get(12)?,
        terminal_error: row.get(13)?,
        run_id: row.get(14)?,
        assistant_message_id: row.get(15)?,
        created_at: row.get(16)?,
        claimed_at: row.get(17)?,
        started_at: row.get(18)?,
        completed_at: row.get(19)?,
    })
}

pub(super) struct WakeRow {
    sequence: i64,
    wake_id: String,
    schema_version: i64,
    root_agent_id: String,
    agent_id: String,
    requester_agent_id: String,
    request_id: String,
    source_agent_message_id: Option<String>,
    status: String,
    status_revision: i64,
    claim_token: Option<String>,
    lease_expires_at: Option<i64>,
    result_message_id: Option<String>,
    terminal_error: Option<String>,
    run_id: Option<String>,
    assistant_message_id: Option<String>,
    created_at: i64,
    claimed_at: Option<i64>,
    started_at: Option<i64>,
    completed_at: Option<i64>,
}

pub(super) fn decode_wake(row: WakeRow) -> Result<AgentWakeRequestRecord, AgentGraphError> {
    validate_schema_version(row.schema_version)?;
    Ok(AgentWakeRequestRecord {
        sequence: positive_u64(row.sequence, "Wake sequence")?,
        wake_id: row.wake_id,
        root_agent_id: row.root_agent_id,
        agent_id: row.agent_id,
        requester_agent_id: row.requester_agent_id,
        request_id: row.request_id,
        source_agent_message_id: row.source_agent_message_id,
        status: AgentWakeStatus::parse(&row.status)?,
        status_revision: positive_u64(row.status_revision, "Wake status revision")?,
        claim_token: row.claim_token,
        lease_expires_at: row.lease_expires_at,
        result_message_id: row.result_message_id,
        terminal_error: row.terminal_error,
        run_id: row.run_id,
        assistant_message_id: row.assistant_message_id,
        created_at: row.created_at,
        claimed_at: row.claimed_at,
        started_at: row.started_at,
        completed_at: row.completed_at,
    })
}
