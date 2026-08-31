use super::common::{positive_u64, read_error, validate_schema_version};
use crate::{
    AgentGraphError, AgentMailboxDeliveryStatus, AgentMailboxKind, AgentMailboxMessageRecord,
};
use rusqlite::{params, Connection, OptionalExtension};

const MESSAGE_SELECT: &str = "
    SELECT sequence, message_id, schema_version, root_agent_id, sender_agent_id,
           recipient_agent_id, request_id, kind, content, projection_message_id,
           delivery_status, claim_token, lease_expires_at, created_at, claimed_at,
           acknowledged_at
    FROM agent_mailbox_messages";

pub(super) fn query_message(
    connection: &Connection,
    message_id: &str,
) -> Result<Option<AgentMailboxMessageRecord>, AgentGraphError> {
    connection
        .query_row(
            &format!("{MESSAGE_SELECT} WHERE message_id = ?1"),
            [message_id],
            read_message_row,
        )
        .optional()
        .map_err(read_error)?
        .map(decode_message)
        .transpose()
}

pub(super) fn query_message_by_request(
    connection: &Connection,
    sender_agent_id: &str,
    request_id: &str,
) -> Result<Option<AgentMailboxMessageRecord>, AgentGraphError> {
    connection
        .query_row(
            &format!("{MESSAGE_SELECT} WHERE sender_agent_id = ?1 AND request_id = ?2"),
            params![sender_agent_id, request_id],
            read_message_row,
        )
        .optional()
        .map_err(read_error)?
        .map(decode_message)
        .transpose()
}

pub(super) fn query_message_by_claim_token(
    connection: &Connection,
    claim_token: &str,
) -> Result<Option<AgentMailboxMessageRecord>, AgentGraphError> {
    connection
        .query_row(
            &format!("{MESSAGE_SELECT} WHERE claim_token = ?1"),
            [claim_token],
            read_message_row,
        )
        .optional()
        .map_err(read_error)?
        .map(decode_message)
        .transpose()
}

pub(super) fn read_message_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<MessageRow> {
    Ok(MessageRow {
        sequence: row.get(0)?,
        message_id: row.get(1)?,
        schema_version: row.get(2)?,
        root_agent_id: row.get(3)?,
        sender_agent_id: row.get(4)?,
        recipient_agent_id: row.get(5)?,
        request_id: row.get(6)?,
        kind: row.get(7)?,
        content: row.get(8)?,
        projection_message_id: row.get(9)?,
        delivery_status: row.get(10)?,
        claim_token: row.get(11)?,
        lease_expires_at: row.get(12)?,
        created_at: row.get(13)?,
        claimed_at: row.get(14)?,
        acknowledged_at: row.get(15)?,
    })
}

pub(super) struct MessageRow {
    sequence: i64,
    message_id: String,
    schema_version: i64,
    root_agent_id: String,
    sender_agent_id: String,
    recipient_agent_id: String,
    request_id: String,
    kind: String,
    content: String,
    projection_message_id: String,
    delivery_status: String,
    claim_token: Option<String>,
    lease_expires_at: Option<i64>,
    created_at: i64,
    claimed_at: Option<i64>,
    acknowledged_at: Option<i64>,
}

pub(super) fn decode_message(
    row: MessageRow,
) -> Result<AgentMailboxMessageRecord, AgentGraphError> {
    validate_schema_version(row.schema_version)?;
    Ok(AgentMailboxMessageRecord {
        sequence: positive_u64(row.sequence, "Mailbox sequence")?,
        message_id: row.message_id,
        root_agent_id: row.root_agent_id,
        sender_agent_id: row.sender_agent_id,
        recipient_agent_id: row.recipient_agent_id,
        request_id: row.request_id,
        kind: AgentMailboxKind::parse(&row.kind)?,
        content: row.content,
        projection_message_id: row.projection_message_id,
        delivery_status: AgentMailboxDeliveryStatus::parse(&row.delivery_status)?,
        claim_token: row.claim_token,
        lease_expires_at: row.lease_expires_at,
        created_at: row.created_at,
        claimed_at: row.claimed_at,
        acknowledged_at: row.acknowledged_at,
    })
}
