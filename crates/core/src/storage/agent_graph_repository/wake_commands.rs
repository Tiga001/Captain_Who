use super::common::{
    conflict, corrupt, invalid, read_error, validate_id, validate_request_id, validate_time,
    write_error,
};
use super::message_records::query_message;
use super::nodes::{ensure_active_agent, ensure_active_pair, is_strict_descendant};
use super::wake_records::{query_wake, query_wake_by_request};
use crate::{
    AgentGraphError, AgentMailboxKind, AgentMailboxMessageRecord, AgentWakeRequestRecord,
    EnqueueAgentWakeInput, IdempotentCreate, AGENT_GRAPH_SCHEMA_VERSION,
};
use rusqlite::{params, Connection};

pub(super) fn enqueue_wake_in_transaction(
    transaction: &Connection,
    input: &EnqueueAgentWakeInput,
    created_at: i64,
) -> Result<IdempotentCreate<AgentWakeRequestRecord>, AgentGraphError> {
    if let Some(existing) =
        query_wake_by_request(transaction, &input.requester_agent_id, &input.request_id)?
    {
        if wake_matches_enqueue(&existing, input) {
            return Ok(IdempotentCreate::Existing(existing));
        }
        return Err(conflict("Wake request ID was reused with different facts"));
    }
    if query_wake(transaction, &input.wake_id)?.is_some() {
        return Err(conflict("Wake ID is already in use"));
    }
    if let Some(source_message_id) = input.source_agent_message_id.as_deref() {
        let existing_source_wake = transaction
            .query_row(
                "SELECT EXISTS(
                     SELECT 1 FROM agent_wake_requests WHERE source_agent_message_id = ?1
                 )",
                [source_message_id],
                |row| row.get::<_, bool>(0),
            )
            .map_err(read_error)?;
        if existing_source_wake {
            return Err(conflict(
                "Wake source message is already bound to another Wake",
            ));
        }
    }
    ensure_active_pair(
        transaction,
        &input.root_agent_id,
        &input.requester_agent_id,
        &input.agent_id,
    )?;
    if let Some(source_message_id) = input.source_agent_message_id.as_deref() {
        let source = query_message(transaction, source_message_id)?
            .ok_or_else(|| AgentGraphError::MessageNotFound(source_message_id.to_string()))?;
        if source.root_agent_id != input.root_agent_id
            || source.sender_agent_id != input.requester_agent_id
            || source.recipient_agent_id != input.agent_id
        {
            return Err(conflict(
                "Wake source message does not belong to this request",
            ));
        }
        validate_wake_source_authority(transaction, &source, input.agent_id.as_str())?;
    }
    transaction
        .execute(
            "INSERT INTO agent_wake_requests (
                 wake_id, schema_version, root_agent_id, agent_id, requester_agent_id,
                 request_id, source_agent_message_id, status, status_revision, claim_token,
                 lease_expires_at, result_message_id, terminal_error, run_id,
                 assistant_message_id, created_at, claimed_at, started_at, completed_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'queued', 1, NULL, NULL, NULL,
                       NULL, NULL, NULL, ?8, NULL, NULL, NULL)",
            params![
                &input.wake_id,
                i64::from(AGENT_GRAPH_SCHEMA_VERSION),
                &input.root_agent_id,
                &input.agent_id,
                &input.requester_agent_id,
                &input.request_id,
                &input.source_agent_message_id,
                created_at,
            ],
        )
        .map_err(write_error)?;
    let record = query_wake(transaction, &input.wake_id)?
        .ok_or_else(|| corrupt("created Wake could not be read back"))?;
    Ok(IdempotentCreate::Created(record))
}

pub(super) fn validate_wake_input(
    input: &EnqueueAgentWakeInput,
    created_at: i64,
) -> Result<(), AgentGraphError> {
    validate_id("wake_id", &input.wake_id)?;
    validate_id("root_agent_id", &input.root_agent_id)?;
    validate_id("agent_id", &input.agent_id)?;
    validate_id("requester_agent_id", &input.requester_agent_id)?;
    validate_request_id(&input.request_id)?;
    if let Some(message_id) = &input.source_agent_message_id {
        validate_id("source_agent_message_id", message_id)?;
    }
    if input.agent_id == input.requester_agent_id {
        return Err(invalid("agent_id", "must differ from requester Agent"));
    }
    validate_time(created_at)
}

pub(super) fn validate_wake_source_authority(
    connection: &Connection,
    source: &AgentMailboxMessageRecord,
    target_agent_id: &str,
) -> Result<(), AgentGraphError> {
    let sender = ensure_active_agent(connection, &source.sender_agent_id)?;
    let target = ensure_active_agent(connection, target_agent_id)?;
    let authorized = match source.kind {
        AgentMailboxKind::Task => {
            target.parent_agent_id.as_deref() == Some(sender.agent_id.as_str())
        }
        AgentMailboxKind::Followup => {
            is_strict_descendant(connection, &sender.agent_id, &target.agent_id)?
        }
        AgentMailboxKind::Result => {
            sender.parent_agent_id.as_deref() == Some(target.agent_id.as_str())
        }
        AgentMailboxKind::Message => false,
    };
    if !authorized {
        return Err(conflict(
            "Wake source violates task/follow-up/result tree authority",
        ));
    }
    Ok(())
}

pub(super) fn wake_matches_enqueue(
    record: &AgentWakeRequestRecord,
    input: &EnqueueAgentWakeInput,
) -> bool {
    record.wake_id == input.wake_id
        && record.root_agent_id == input.root_agent_id
        && record.agent_id == input.agent_id
        && record.requester_agent_id == input.requester_agent_id
        && record.request_id == input.request_id
        && record.source_agent_message_id == input.source_agent_message_id
}
