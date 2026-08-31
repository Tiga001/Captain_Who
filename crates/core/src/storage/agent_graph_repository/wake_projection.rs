use super::common::{conflict, corrupt, validate_id};
use super::mailbox::{ensure_projection_exists, project_queued_agent_messages_through_sequence};
use super::message_records::query_message;
use super::wake_commands::validate_wake_source_authority;
use super::wake_records::query_wake;
use crate::{
    AgentGraphError, AgentMailboxDeliveryStatus, AgentMailboxMessageRecord, AgentWakeStatus,
};
use rusqlite::Connection;

/// Makes the exact source of a queued Wake model/history-visible before the same outer
/// transaction claims that Wake. FIFO is preserved by projecting every earlier queued item for
/// the recipient first. Initial tasks already projected by ChildAgentFactory are idempotent.
pub(crate) fn project_agent_wake_source_in_transaction(
    connection: &Connection,
    wake_id: &str,
    claim_token_prefix: &str,
    projected_at: i64,
) -> Result<Vec<AgentMailboxMessageRecord>, AgentGraphError> {
    validate_id("wake_id", wake_id)?;
    let wake = query_wake(connection, wake_id)?
        .ok_or_else(|| AgentGraphError::WakeNotFound(wake_id.to_string()))?;
    if wake.status != AgentWakeStatus::Queued {
        return Err(conflict(
            "only a queued Wake source can be projected for dispatch",
        ));
    }
    let Some(source_id) = wake.source_agent_message_id.as_deref() else {
        return Ok(Vec::new());
    };
    let source = query_message(connection, source_id)?
        .ok_or_else(|| corrupt("queued Wake source Mailbox message is missing"))?;
    if source.recipient_agent_id != wake.agent_id
        || source.sender_agent_id != wake.requester_agent_id
        || source.root_agent_id != wake.root_agent_id
    {
        return Err(corrupt(
            "queued Wake source identity does not match its Mailbox fact",
        ));
    }
    validate_wake_source_authority(connection, &source, &wake.agent_id)?;
    if source.delivery_status == AgentMailboxDeliveryStatus::Acknowledged {
        ensure_projection_exists(connection, &source)?;
        return Ok(Vec::new());
    }
    let projected = project_queued_agent_messages_through_sequence(
        connection,
        &wake.agent_id,
        claim_token_prefix,
        projected_at,
        Some(source.sequence),
        usize::MAX,
    )?;
    let delivered = query_message(connection, source_id)?
        .ok_or_else(|| corrupt("projected Wake source disappeared"))?;
    if delivered.delivery_status != AgentMailboxDeliveryStatus::Acknowledged {
        return Err(conflict(
            "Wake source could not be projected without overtaking an earlier Mailbox item",
        ));
    }
    Ok(projected)
}
