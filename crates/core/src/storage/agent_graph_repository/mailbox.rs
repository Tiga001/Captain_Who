use super::common::{
    conflict, corrupt, immediate, invalid, read_error, stable_fact_id, validate_bounded_text,
    validate_id, validate_request_id, validate_time, validate_trimmed, write_error,
    MAX_MESSAGE_BYTES, MAX_PROJECT_BATCH, MAX_UNBOUND_MAILBOX_BYTES_PER_RECIPIENT,
    MAX_UNBOUND_MAILBOX_MESSAGES_PER_RECIPIENT, MAX_UNBOUND_ORDINARY_MAILBOX_BYTES_PER_RECIPIENT,
    MAX_UNBOUND_ORDINARY_MAILBOX_MESSAGES_PER_RECIPIENT, WAKE_LEASE_DURATION_MS,
};
use super::message_records::{
    query_message, query_message_by_claim_token, query_message_by_request,
};
use super::node_records::query_node;
use super::nodes::{ensure_active_agent, ensure_active_pair, is_strict_descendant};
use super::wake_commands::{enqueue_wake_in_transaction, validate_wake_input};
use super::wake_records::{decode_wake, query_wake, read_wake_row, WAKE_SELECT};
use crate::{
    AcknowledgeAgentTaskAndWakeInput, AgentGraphError, AgentMailboxDeliveryStatus,
    AgentMailboxKind, AgentMailboxMessageRecord, AgentMessageDispatch, AgentWakeRequestRecord,
    AgentWakeStatus, ConversationMessageOrigin, EnqueueAgentMessageInput, EnqueueAgentWakeInput,
    IdempotentCreate, SendAgentMessageRequest, AGENT_GRAPH_SCHEMA_VERSION,
};
use rusqlite::{params, Connection, OptionalExtension};

pub fn enqueue_agent_message(
    connection: &mut Connection,
    input: &EnqueueAgentMessageInput,
    created_at: i64,
) -> Result<IdempotentCreate<AgentMailboxMessageRecord>, AgentGraphError> {
    validate_message_input(input, created_at)?;
    let transaction = immediate(connection)?;
    let outcome = enqueue_message_in_transaction(&transaction, input, created_at)?;
    transaction.commit().map_err(write_error)?;
    Ok(outcome)
}

/// Application-level send: one immutable same-tree Mailbox fact, no projection and no Wake.
pub fn send_agent_message(
    connection: &mut Connection,
    input: &SendAgentMessageRequest,
    created_at: i64,
) -> Result<AgentMessageDispatch, AgentGraphError> {
    send_agent_message_internal(connection, input, None, created_at)
}

/// Run-bound application message used by collaboration Tool execution.
///
/// The durable stop check and Mailbox insert share one immediate transaction. A root-tree stop
/// therefore either observes this committed message or prevents a stopped Turn from creating it.
pub fn send_agent_message_from_run(
    connection: &mut Connection,
    input: &SendAgentMessageRequest,
    origin_run_id: &str,
    created_at: i64,
) -> Result<AgentMessageDispatch, AgentGraphError> {
    send_agent_message_internal(connection, input, Some(origin_run_id), created_at)
}

fn send_agent_message_internal(
    connection: &mut Connection,
    input: &SendAgentMessageRequest,
    origin_run_id: Option<&str>,
    created_at: i64,
) -> Result<AgentMessageDispatch, AgentGraphError> {
    let transaction = immediate(connection)?;
    let sender = ensure_active_agent(&transaction, &input.sender_agent_id)?;
    if let Some(origin_run_id) = origin_run_id {
        super::ensure_agent_tree_origin_run_can_schedule_in_transaction(
            &transaction,
            origin_run_id,
            &sender.agent_id,
        )?;
    }
    let message = enqueue_application_message_in_transaction(
        &transaction,
        input,
        AgentMailboxKind::Message,
        created_at,
    )?;
    transaction.commit().map_err(write_error)?;
    Ok(AgentMessageDispatch {
        message,
        deferred_wake: None,
    })
}

/// Application-level follow-up: persists the message and its deferred Wake in one transaction.
/// Delivery remains queued until Dispatcher admission or a running Turn's safe sampling boundary.
pub fn follow_up_agent(
    connection: &mut Connection,
    input: &SendAgentMessageRequest,
    created_at: i64,
) -> Result<AgentMessageDispatch, AgentGraphError> {
    follow_up_agent_internal(connection, input, None, created_at)
}

/// Run-bound application follow-up used by collaboration Tool execution.
///
/// The origin-run stop guard shares the same immediate transaction as the message and Wake, so a
/// concurrent root-tree stop either rejects this entire write or observes and cancels its Wake.
pub fn follow_up_agent_from_run(
    connection: &mut Connection,
    input: &SendAgentMessageRequest,
    origin_run_id: &str,
    created_at: i64,
) -> Result<AgentMessageDispatch, AgentGraphError> {
    follow_up_agent_internal(connection, input, Some(origin_run_id), created_at)
}

fn follow_up_agent_internal(
    connection: &mut Connection,
    input: &SendAgentMessageRequest,
    origin_run_id: Option<&str>,
    created_at: i64,
) -> Result<AgentMessageDispatch, AgentGraphError> {
    let transaction = immediate(connection)?;
    let sender = ensure_active_agent(&transaction, &input.sender_agent_id)?;
    if let Some(origin_run_id) = origin_run_id {
        super::ensure_agent_tree_origin_run_can_schedule_in_transaction(
            &transaction,
            origin_run_id,
            &sender.agent_id,
        )?;
    }
    let target = ensure_active_agent(&transaction, &input.recipient_agent_id)?;
    if !is_strict_descendant(&transaction, &sender.agent_id, &target.agent_id)? {
        return Err(conflict(
            "follow-up authority is limited to a caller's strict descendants",
        ));
    }
    let message = enqueue_application_message_in_transaction(
        &transaction,
        input,
        AgentMailboxKind::Followup,
        created_at,
    )?;
    let wake_input = EnqueueAgentWakeInput {
        wake_id: stable_fact_id("wake", &[&input.sender_agent_id, &input.request_id]),
        root_agent_id: sender.root_agent_id,
        agent_id: input.recipient_agent_id.clone(),
        requester_agent_id: input.sender_agent_id.clone(),
        request_id: stable_fact_id(
            "followup-request",
            &[&input.sender_agent_id, &input.request_id],
        ),
        source_agent_message_id: Some(message.message_id.clone()),
    };
    let deferred_wake = enqueue_wake_in_transaction(&transaction, &wake_input, created_at)?
        .record()
        .clone();
    if let Some(origin_run_id) = origin_run_id {
        crate::storage::agent_collaboration_run_policy_repository::inherit_run_for_wake(
            &transaction,
            origin_run_id,
            &deferred_wake.wake_id,
        )
        .map_err(write_error)?;
    }
    transaction.commit().map_err(write_error)?;
    Ok(AgentMessageDispatch {
        message,
        deferred_wake: Some(deferred_wake),
    })
}

pub(super) fn enqueue_application_message_in_transaction(
    connection: &Connection,
    input: &SendAgentMessageRequest,
    kind: AgentMailboxKind,
    created_at: i64,
) -> Result<AgentMailboxMessageRecord, AgentGraphError> {
    validate_id("sender_agent_id", &input.sender_agent_id)?;
    validate_id("recipient_agent_id", &input.recipient_agent_id)?;
    validate_request_id(&input.request_id)?;
    validate_trimmed("content", &input.content, MAX_MESSAGE_BYTES)?;
    let sender = ensure_active_agent(connection, &input.sender_agent_id)?;
    ensure_active_pair(
        connection,
        &sender.root_agent_id,
        &input.sender_agent_id,
        &input.recipient_agent_id,
    )?;
    let message = EnqueueAgentMessageInput {
        message_id: stable_fact_id("mailbox", &[&input.sender_agent_id, &input.request_id]),
        root_agent_id: sender.root_agent_id,
        sender_agent_id: input.sender_agent_id.clone(),
        recipient_agent_id: input.recipient_agent_id.clone(),
        request_id: input.request_id.clone(),
        kind,
        content: input.content.clone(),
        projection_message_id: stable_fact_id(
            "message",
            &[&input.sender_agent_id, &input.request_id],
        ),
    };
    Ok(
        enqueue_message_in_transaction(connection, &message, created_at)?
            .record()
            .clone(),
    )
}

pub fn get_agent_message(
    connection: &Connection,
    message_id: &str,
) -> Result<Option<AgentMailboxMessageRecord>, AgentGraphError> {
    validate_id("message_id", message_id)?;
    query_message(connection, message_id)
}

pub fn claim_next_agent_message(
    connection: &mut Connection,
    recipient_agent_id: &str,
    claim_token: &str,
    claimed_at: i64,
) -> Result<Option<AgentMailboxMessageRecord>, AgentGraphError> {
    validate_id("recipient_agent_id", recipient_agent_id)?;
    validate_id("claim_token", claim_token)?;
    validate_time(claimed_at)?;
    let transaction = immediate(connection)?;
    if let Some(existing) = query_message_by_claim_token(&transaction, claim_token)? {
        if existing.recipient_agent_id != recipient_agent_id {
            return Err(conflict(
                "Mailbox claim token was reused for another recipient",
            ));
        }
        if existing.delivery_status == AgentMailboxDeliveryStatus::Claimed
            && existing
                .lease_expires_at
                .is_some_and(|deadline| claimed_at >= deadline)
        {
            return Err(conflict("Mailbox claim lease has expired"));
        }
        transaction.commit().map_err(write_error)?;
        return Ok(Some(existing));
    }
    ensure_active_agent(&transaction, recipient_agent_id)?;
    let lease_expires_at = claimed_at
        .checked_add(WAKE_LEASE_DURATION_MS)
        .ok_or_else(|| invalid("claimed_at", "cannot compute the Mailbox lease deadline"))?;
    transaction
        .execute(
            "UPDATE agent_mailbox_messages
             SET delivery_status = 'queued', claim_token = NULL, lease_expires_at = NULL,
                 claimed_at = NULL
             WHERE recipient_agent_id = ?1 AND delivery_status = 'claimed'
               AND lease_expires_at <= ?2",
            params![recipient_agent_id, claimed_at],
        )
        .map_err(write_error)?;
    let already_claimed = transaction
        .query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM agent_mailbox_messages
                 WHERE recipient_agent_id = ?1 AND delivery_status = 'claimed'
             )",
            [recipient_agent_id],
            |row| row.get::<_, bool>(0),
        )
        .map_err(read_error)?;
    if already_claimed {
        transaction.commit().map_err(write_error)?;
        return Ok(None);
    }
    let next_id = transaction
        .query_row(
            "SELECT message_id FROM agent_mailbox_messages
             WHERE recipient_agent_id = ?1 AND delivery_status = 'queued'
             ORDER BY sequence LIMIT 1",
            [recipient_agent_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(read_error)?;
    let Some(message_id) = next_id else {
        transaction.commit().map_err(write_error)?;
        return Ok(None);
    };
    transaction
        .execute(
            "UPDATE agent_mailbox_messages
             SET delivery_status = 'claimed', claim_token = ?1, lease_expires_at = ?2,
                 claimed_at = ?3
             WHERE message_id = ?4 AND delivery_status = 'queued'",
            params![claim_token, lease_expires_at, claimed_at, &message_id],
        )
        .map_err(write_error)?;
    let claimed = query_message(&transaction, &message_id)?
        .ok_or_else(|| corrupt("claimed Mailbox message could not be read back"))?;
    transaction.commit().map_err(write_error)?;
    Ok(Some(claimed))
}

pub fn renew_agent_message_lease(
    connection: &mut Connection,
    message_id: &str,
    claim_token: &str,
    renewed_at: i64,
) -> Result<AgentMailboxMessageRecord, AgentGraphError> {
    validate_id("message_id", message_id)?;
    validate_id("claim_token", claim_token)?;
    validate_time(renewed_at)?;
    let next_deadline = renewed_at
        .checked_add(WAKE_LEASE_DURATION_MS)
        .ok_or_else(|| invalid("renewed_at", "cannot compute the Mailbox lease deadline"))?;
    let transaction = immediate(connection)?;
    let current = query_message(&transaction, message_id)?
        .ok_or_else(|| AgentGraphError::MessageNotFound(message_id.to_string()))?;
    if current.delivery_status != AgentMailboxDeliveryStatus::Claimed
        || current.claim_token.as_deref() != Some(claim_token)
    {
        return Err(conflict(
            "Mailbox message is not actively held by this claim token",
        ));
    }
    let current_deadline = current
        .lease_expires_at
        .ok_or_else(|| corrupt("claimed Mailbox message has no lease deadline"))?;
    if renewed_at >= current_deadline {
        return Err(conflict("Mailbox claim lease has expired"));
    }
    let advanced_deadline = next_deadline.max(current_deadline.saturating_add(1));
    transaction
        .execute(
            "UPDATE agent_mailbox_messages SET lease_expires_at = ?1
             WHERE message_id = ?2 AND delivery_status = 'claimed'
               AND claim_token = ?3 AND lease_expires_at = ?4",
            params![advanced_deadline, message_id, claim_token, current_deadline],
        )
        .map_err(write_error)?;
    let renewed = query_message(&transaction, message_id)?
        .ok_or_else(|| corrupt("renewed Mailbox message could not be read back"))?;
    transaction.commit().map_err(write_error)?;
    Ok(renewed)
}

pub fn acknowledge_agent_message_with_projection(
    connection: &mut Connection,
    message_id: &str,
    claim_token: &str,
    acknowledged_at: i64,
) -> Result<AgentMailboxMessageRecord, AgentGraphError> {
    validate_id("message_id", message_id)?;
    validate_id("claim_token", claim_token)?;
    validate_time(acknowledged_at)?;
    let transaction = immediate(connection)?;
    let message = query_message(&transaction, message_id)?
        .ok_or_else(|| AgentGraphError::MessageNotFound(message_id.to_string()))?;
    if matches!(
        message.kind,
        AgentMailboxKind::Task | AgentMailboxKind::Followup
    ) {
        return Err(invalid(
            "message_id",
            "task and followup messages must be acknowledged atomically with a Wake",
        ));
    }
    let acknowledged =
        acknowledge_message_in_transaction(&transaction, message_id, claim_token, acknowledged_at)?;
    transaction.commit().map_err(write_error)?;
    Ok(acknowledged)
}

pub fn acknowledge_agent_task_with_projection_and_wake(
    connection: &mut Connection,
    input: &AcknowledgeAgentTaskAndWakeInput,
    acknowledged_at: i64,
) -> Result<(AgentMailboxMessageRecord, AgentWakeRequestRecord), AgentGraphError> {
    validate_id("message_id", &input.message_id)?;
    validate_id("message_claim_token", &input.message_claim_token)?;
    validate_time(acknowledged_at)?;
    validate_wake_input(&input.wake, acknowledged_at)?;
    if input.wake.source_agent_message_id.as_deref() != Some(input.message_id.as_str()) {
        return Err(invalid(
            "wake.source_agent_message_id",
            "must reference the task message being acknowledged",
        ));
    }
    let transaction = immediate(connection)?;
    let message = query_message(&transaction, &input.message_id)?
        .ok_or_else(|| AgentGraphError::MessageNotFound(input.message_id.clone()))?;
    if !matches!(
        message.kind,
        AgentMailboxKind::Task | AgentMailboxKind::Followup
    ) {
        return Err(invalid(
            "message_id",
            "only task or followup messages may atomically create a Wake",
        ));
    }
    let acknowledged = acknowledge_message_in_transaction(
        &transaction,
        &input.message_id,
        &input.message_claim_token,
        acknowledged_at,
    )?;
    let wake = enqueue_wake_in_transaction(&transaction, &input.wake, acknowledged_at)?;
    transaction.commit().map_err(write_error)?;
    Ok((acknowledged, wake.record().clone()))
}

pub(super) fn acknowledge_message_in_transaction(
    transaction: &Connection,
    message_id: &str,
    claim_token: &str,
    acknowledged_at: i64,
) -> Result<AgentMailboxMessageRecord, AgentGraphError> {
    let current = query_message(transaction, message_id)?
        .ok_or_else(|| AgentGraphError::MessageNotFound(message_id.to_string()))?;
    if current.delivery_status == AgentMailboxDeliveryStatus::Acknowledged {
        if current.claim_token.as_deref() == Some(claim_token) {
            let projection_exists = transaction
                .query_row(
                    "SELECT EXISTS(
                         SELECT 1 FROM messages
                         WHERE id = ?1 AND source_agent_message_id = ?2
                     )",
                    params![&current.projection_message_id, &current.message_id],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(read_error)?;
            if !projection_exists {
                return Err(corrupt(
                    "acknowledged Mailbox message is missing its Conversation projection",
                ));
            }
            return Ok(current);
        }
        return Err(conflict(
            "Mailbox acknowledgement belongs to another claim token",
        ));
    }
    if current.delivery_status != AgentMailboxDeliveryStatus::Claimed
        || current.claim_token.as_deref() != Some(claim_token)
    {
        return Err(conflict("Mailbox message is not held by this claim token"));
    }
    if current
        .lease_expires_at
        .is_none_or(|deadline| acknowledged_at >= deadline)
    {
        return Err(conflict("Mailbox claim lease has expired"));
    }
    let recipient = query_node(transaction, &current.recipient_agent_id)?
        .ok_or_else(|| corrupt("Mailbox recipient Agent is missing"))?;
    let active_assistant_position = transaction
        .query_row(
            "SELECT message.position
             FROM conversation_turn_traces AS trace
             JOIN messages AS message ON message.id = trace.assistant_message_id
             WHERE trace.conversation_id = ?1 AND trace.terminal_status = 'in_progress'
             LIMIT 1",
            [&recipient.conversation_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(read_error)?;
    let append_position = transaction
        .query_row(
            "SELECT COALESCE(MAX(position), -1) + 1 FROM messages WHERE conversation_id = ?1",
            [&recipient.conversation_id],
            |row| row.get::<_, i64>(0),
        )
        .map_err(read_error)?;
    let position = active_assistant_position.unwrap_or(append_position);
    if let Some(active_position) = active_assistant_position {
        // Runtime-visible collaboration input belongs immediately before the pending assistant.
        // Projected mailbox rows already inserted for this Turn remain before it and are never
        // rewritten; only the pending assistant and any later ordinary rows move right.
        transaction
            .execute(
                "UPDATE messages SET position = position + 1
                 WHERE conversation_id = ?1 AND position >= ?2",
                params![&recipient.conversation_id, active_position],
            )
            .map_err(write_error)?;
    }
    transaction
        .execute(
            "INSERT INTO messages (
                 id, conversation_id, role, content, status,
                 input_origin_kind, input_origin_agent_id, source_agent_message_id,
                 agent_run_json, created_at, position
             ) VALUES (?1, ?2, 'user', ?3, 'sent', 'agent', ?4, ?5,
                       NULL, ?6, ?7)",
            params![
                &current.projection_message_id,
                &recipient.conversation_id,
                &current.content,
                &current.sender_agent_id,
                &current.message_id,
                acknowledged_at,
                position,
            ],
        )
        .map_err(write_error)?;
    transaction
        .execute(
            "UPDATE agent_mailbox_messages
             SET delivery_status = 'acknowledged', acknowledged_at = ?1
             WHERE message_id = ?2 AND delivery_status = 'claimed' AND claim_token = ?3",
            params![acknowledged_at, message_id, claim_token],
        )
        .map_err(write_error)?;
    transaction
        .execute(
            "UPDATE conversations
             SET updated_at = MAX(updated_at, ?1) WHERE id = ?2",
            params![acknowledged_at, &recipient.conversation_id],
        )
        .map_err(write_error)?;
    let acknowledged = query_message(transaction, message_id)?
        .ok_or_else(|| corrupt("acknowledged Mailbox message could not be read back"))?;
    Ok(acknowledged)
}

/// Projects queued Mailbox facts at a caller-owned transaction boundary. This is shared by the
/// Dispatcher admission path and the model-batch receipt path so projection ordering is defined
/// once. A claimed-but-unexpired row is never stolen; an expired claim is recoverable.
pub(crate) fn project_pending_agent_messages_in_transaction(
    connection: &Connection,
    recipient_agent_id: &str,
    claim_token_prefix: &str,
    projected_at: i64,
    maximum: usize,
) -> Result<Vec<AgentMailboxMessageRecord>, AgentGraphError> {
    validate_id("recipient_agent_id", recipient_agent_id)?;
    validate_id("claim_token_prefix", claim_token_prefix)?;
    validate_time(projected_at)?;
    if maximum == 0 || maximum > MAX_PROJECT_BATCH {
        return Err(invalid(
            "maximum",
            format!("must be between 1 and {MAX_PROJECT_BATCH}"),
        ));
    }
    ensure_active_agent(connection, recipient_agent_id)?;
    project_queued_agent_messages_through_sequence(
        connection,
        recipient_agent_id,
        claim_token_prefix,
        projected_at,
        None,
        maximum,
    )
}

/// Marks a deferred Wake satisfied when its exact source message was bound to an already-running
/// model batch. The caller owns the surrounding receipt transaction, making duplicate delivery
/// and a second Turn mutually exclusive across crashes.
pub(crate) fn satisfy_agent_wake_by_source_message_in_transaction(
    connection: &Connection,
    source_message_id: &str,
    satisfied_at: i64,
) -> Result<Option<AgentWakeRequestRecord>, AgentGraphError> {
    validate_id("source_message_id", source_message_id)?;
    validate_time(satisfied_at)?;
    let wake = connection
        .query_row(
            &format!("{WAKE_SELECT} WHERE source_agent_message_id = ?1"),
            [source_message_id],
            read_wake_row,
        )
        .optional()
        .map_err(read_error)?
        .map(decode_wake)
        .transpose()?;
    let Some(wake) = wake else {
        return Ok(None);
    };
    if wake.status == AgentWakeStatus::Satisfied {
        return Ok(Some(wake));
    }
    if wake.status != AgentWakeStatus::Queued {
        // Claimed/running means the dispatcher won the race; another terminal state is immutable.
        return Ok(None);
    }
    connection
        .execute(
            "UPDATE agent_wake_requests
             SET status = 'satisfied', status_revision = status_revision + 1,
                 completed_at = ?1
             WHERE wake_id = ?2 AND status = 'queued'",
            params![satisfied_at, &wake.wake_id],
        )
        .map_err(write_error)?;
    query_wake(connection, &wake.wake_id)
}

pub(super) fn project_queued_agent_messages_through_sequence(
    connection: &Connection,
    recipient_agent_id: &str,
    claim_token_prefix: &str,
    projected_at: i64,
    through_sequence: Option<u64>,
    maximum: usize,
) -> Result<Vec<AgentMailboxMessageRecord>, AgentGraphError> {
    let through_sequence = through_sequence
        .map(|value| i64::try_from(value).map_err(|_| corrupt("Mailbox sequence is too large")))
        .transpose()?;
    let live_claim = connection
        .query_row(
            "SELECT message_id, lease_expires_at
             FROM agent_mailbox_messages
             WHERE recipient_agent_id = ?1 AND delivery_status = 'claimed'
             ORDER BY sequence LIMIT 1",
            [recipient_agent_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()
        .map_err(read_error)?;
    if let Some((message_id, deadline)) = live_claim {
        if projected_at < deadline {
            return Err(conflict(format!(
                "Mailbox message `{message_id}` is held by another live claim"
            )));
        }
        connection
            .execute(
                "UPDATE agent_mailbox_messages
                 SET delivery_status = 'queued', claim_token = NULL, lease_expires_at = NULL,
                     claimed_at = NULL
                 WHERE message_id = ?1 AND delivery_status = 'claimed'
                   AND lease_expires_at <= ?2",
                params![message_id, projected_at],
            )
            .map_err(write_error)?;
    }

    let mut projected = Vec::new();
    while projected.len() < maximum {
        let next = connection
            .query_row(
                "SELECT message_id, sequence
                 FROM agent_mailbox_messages
                 WHERE recipient_agent_id = ?1 AND delivery_status = 'queued'
                   AND (?2 IS NULL OR sequence <= ?2)
                 ORDER BY sequence LIMIT 1",
                params![recipient_agent_id, through_sequence],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
            )
            .optional()
            .map_err(read_error)?;
        let Some((message_id, _sequence)) = next else {
            break;
        };
        let claim_token = stable_fact_id(
            "mailbox-claim",
            &[claim_token_prefix, recipient_agent_id, message_id.as_str()],
        );
        let deadline = projected_at
            .checked_add(WAKE_LEASE_DURATION_MS)
            .ok_or_else(|| invalid("projected_at", "cannot compute Mailbox lease"))?;
        let changed = connection
            .execute(
                "UPDATE agent_mailbox_messages
                 SET delivery_status = 'claimed', claim_token = ?1, lease_expires_at = ?2,
                     claimed_at = ?3
                 WHERE message_id = ?4 AND delivery_status = 'queued'",
                params![claim_token, deadline, projected_at, message_id],
            )
            .map_err(write_error)?;
        if changed != 1 {
            return Err(conflict("Mailbox FIFO projection lost its claim race"));
        }
        projected.push(acknowledge_message_in_transaction(
            connection,
            &message_id,
            &claim_token,
            projected_at,
        )?);
    }
    Ok(projected)
}

pub(super) fn ensure_projection_exists(
    connection: &Connection,
    message: &AgentMailboxMessageRecord,
) -> Result<(), AgentGraphError> {
    let exists = connection
        .query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM messages
                 WHERE id = ?1 AND source_agent_message_id = ?2
             )",
            params![&message.projection_message_id, &message.message_id],
            |row| row.get::<_, bool>(0),
        )
        .map_err(read_error)?;
    if exists {
        Ok(())
    } else {
        Err(corrupt(
            "acknowledged Mailbox message is missing its Conversation projection",
        ))
    }
}

/// Persists the first parent task, its unique model/history projection and the initial queued
/// Wake as one part of a caller-owned spawn transaction.
pub(crate) fn create_initial_agent_task_and_wake_in_transaction(
    transaction: &Connection,
    message: &EnqueueAgentMessageInput,
    wake: &EnqueueAgentWakeInput,
    claim_token: &str,
    created_at: i64,
) -> Result<(AgentMailboxMessageRecord, AgentWakeRequestRecord), AgentGraphError> {
    validate_message_input(message, created_at)?;
    validate_wake_input(wake, created_at)?;
    validate_id("claim_token", claim_token)?;
    if message.kind != AgentMailboxKind::Task {
        return Err(invalid(
            "message.kind",
            "initial child assignment must be a task",
        ));
    }
    if wake.source_agent_message_id.as_deref() != Some(message.message_id.as_str())
        || wake.root_agent_id != message.root_agent_id
        || wake.requester_agent_id != message.sender_agent_id
        || wake.agent_id != message.recipient_agent_id
    {
        return Err(invalid(
            "wake",
            "initial Wake must reference the exact parent-to-child task",
        ));
    }
    let inserted = enqueue_message_in_transaction(transaction, message, created_at)?;
    if matches!(inserted, IdempotentCreate::Existing(_)) {
        return Err(conflict(
            "initial task already exists outside the completed spawn bundle",
        ));
    }
    let lease_expires_at = created_at
        .checked_add(WAKE_LEASE_DURATION_MS)
        .ok_or_else(|| invalid("created_at", "cannot compute initial task lease"))?;
    transaction
        .execute(
            "UPDATE agent_mailbox_messages
             SET delivery_status = 'claimed', claim_token = ?1, lease_expires_at = ?2,
                 claimed_at = ?3
             WHERE message_id = ?4 AND delivery_status = 'queued'",
            params![
                claim_token,
                lease_expires_at,
                created_at,
                &message.message_id
            ],
        )
        .map_err(write_error)?;
    let acknowledged = acknowledge_message_in_transaction(
        transaction,
        &message.message_id,
        claim_token,
        created_at,
    )?;
    let wake = enqueue_wake_in_transaction(transaction, wake, created_at)?;
    if matches!(wake, IdempotentCreate::Existing(_)) {
        return Err(conflict(
            "initial Wake already exists outside the completed spawn bundle",
        ));
    }
    Ok((acknowledged, wake.record().clone()))
}

pub fn conversation_message_origin(
    connection: &Connection,
    conversation_id: &str,
    message_id: &str,
) -> Result<ConversationMessageOrigin, AgentGraphError> {
    validate_id("conversation_id", conversation_id)?;
    validate_id("message_id", message_id)?;
    let stored = connection
        .query_row(
            "SELECT role, input_origin_kind, input_origin_agent_id, source_agent_message_id,
                    snapshot_source_conversation_id, snapshot_source_message_id,
                    snapshot_original_origin_kind, snapshot_original_agent_id,
                    snapshot_original_mailbox_message_id
             FROM messages WHERE conversation_id = ?1 AND id = ?2",
            params![conversation_id, message_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, Option<String>>(8)?,
                ))
            },
        )
        .optional()
        .map_err(read_error)?
        .ok_or_else(|| AgentGraphError::MessageNotFound(message_id.to_string()))?;
    decode_conversation_message_origin(stored)
}

/// Loads every user/input origin for one Conversation in one ordered SQLite query. Callers that
/// need a conversation snapshot should invoke this through the Storage read transaction API so
/// message content and actor provenance share the same read cut.
pub fn conversation_message_origins(
    connection: &Connection,
    conversation_id: &str,
) -> Result<Vec<(String, ConversationMessageOrigin)>, AgentGraphError> {
    validate_id("conversation_id", conversation_id)?;
    let mut statement = connection
        .prepare(
            "SELECT id, role, input_origin_kind, input_origin_agent_id,
                    source_agent_message_id, snapshot_source_conversation_id,
                    snapshot_source_message_id, snapshot_original_origin_kind,
                    snapshot_original_agent_id, snapshot_original_mailbox_message_id
             FROM messages
             WHERE conversation_id = ?1 AND role = 'user'
             ORDER BY position, id",
        )
        .map_err(read_error)?;
    let rows = statement
        .query_map([conversation_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                (
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, Option<String>>(9)?,
                ),
            ))
        })
        .map_err(read_error)?;
    let mut origins = Vec::new();
    for row in rows {
        let (message_id, stored) = row.map_err(read_error)?;
        origins.push((message_id, decode_conversation_message_origin(stored)?));
    }
    Ok(origins)
}

type StoredConversationMessageOrigin = (
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
);

pub(super) fn decode_conversation_message_origin(
    stored: StoredConversationMessageOrigin,
) -> Result<ConversationMessageOrigin, AgentGraphError> {
    match stored {
        (role, _, _, _, _, _, _, _, _) if role != "user" => Err(AgentGraphError::InvalidInput {
            field: "message_id",
            reason: "message origin is defined only for user/input messages".to_string(),
        }),
        (_, None, None, None, None, None, None, None, None) => Ok(ConversationMessageOrigin::Human),
        (_, Some(kind), None, None, None, None, None, None, None) if kind == "human" => {
            Ok(ConversationMessageOrigin::Human)
        }
        (
            _,
            Some(kind),
            Some(sender_agent_id),
            Some(source_agent_message_id),
            None,
            None,
            None,
            None,
            None,
        ) if kind == "agent" => Ok(ConversationMessageOrigin::Agent {
            sender_agent_id,
            source_agent_message_id,
        }),
        (
            _,
            Some(kind),
            None,
            None,
            Some(source_conversation_id),
            Some(source_message_id),
            Some(original_kind),
            original_agent_id,
            original_mailbox_message_id,
        ) if kind == "snapshot" => {
            let original = match (
                original_kind.as_str(),
                original_agent_id,
                original_mailbox_message_id,
            ) {
                ("human", None, None) => ConversationMessageOrigin::Human,
                ("agent", Some(sender_agent_id), Some(source_agent_message_id)) => {
                    ConversationMessageOrigin::Agent {
                        sender_agent_id,
                        source_agent_message_id,
                    }
                }
                _ => {
                    return Err(corrupt(
                        "Historical snapshot origin columns are inconsistent",
                    ))
                }
            };
            Ok(ConversationMessageOrigin::HistoricalSnapshot {
                source_conversation_id,
                source_message_id,
                original: Box::new(original),
            })
        }
        _ => Err(corrupt(
            "Conversation message origin columns are inconsistent",
        )),
    }
}

pub(super) fn enqueue_message_in_transaction(
    transaction: &Connection,
    input: &EnqueueAgentMessageInput,
    created_at: i64,
) -> Result<IdempotentCreate<AgentMailboxMessageRecord>, AgentGraphError> {
    if let Some(existing) =
        query_message_by_request(transaction, &input.sender_agent_id, &input.request_id)?
    {
        if message_matches_enqueue(&existing, input) {
            return Ok(IdempotentCreate::Existing(existing));
        }
        return Err(conflict(
            "Mailbox request ID was reused with different facts",
        ));
    }
    ensure_active_pair(
        transaction,
        &input.root_agent_id,
        &input.sender_agent_id,
        &input.recipient_agent_id,
    )?;
    if query_message(transaction, &input.message_id)?.is_some() {
        return Err(conflict("Mailbox message ID is already in use"));
    }
    let (unbound_count, unbound_bytes, ordinary_count, ordinary_bytes): (i64, i64, i64, i64) =
        transaction
            .query_row(
                "SELECT COUNT(*), COALESCE(SUM(length(CAST(mailbox.content AS BLOB))), 0),
                    COALESCE(SUM(CASE WHEN mailbox.kind IN ('message', 'followup')
                                      THEN 1 ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN mailbox.kind IN ('message', 'followup')
                                      THEN length(CAST(mailbox.content AS BLOB)) ELSE 0 END), 0)
             FROM agent_mailbox_messages AS mailbox
             WHERE mailbox.recipient_agent_id = ?1
               AND NOT EXISTS (
                   SELECT 1 FROM agent_model_batch_receipt_items AS item
                   WHERE item.message_id = mailbox.message_id
               )",
                [&input.recipient_agent_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .map_err(read_error)?;
    let unbound_count = u64::try_from(unbound_count)
        .map_err(|_| corrupt("unbound Mailbox message count is invalid"))?;
    let unbound_bytes = u64::try_from(unbound_bytes)
        .map_err(|_| corrupt("unbound Mailbox byte count is invalid"))?;
    let ordinary_count = u64::try_from(ordinary_count)
        .map_err(|_| corrupt("ordinary unbound Mailbox message count is invalid"))?;
    let ordinary_bytes = u64::try_from(ordinary_bytes)
        .map_err(|_| corrupt("ordinary unbound Mailbox byte count is invalid"))?;
    let ordinary = matches!(
        input.kind,
        AgentMailboxKind::Message | AgentMailboxKind::Followup
    );
    if ordinary && ordinary_count >= MAX_UNBOUND_ORDINARY_MAILBOX_MESSAGES_PER_RECIPIENT {
        return Err(AgentGraphError::ResourceLimit {
            resource: "ordinary unbound Mailbox messages per recipient",
            limit: MAX_UNBOUND_ORDINARY_MAILBOX_MESSAGES_PER_RECIPIENT,
        });
    }
    if ordinary
        && ordinary_bytes.saturating_add(input.content.len() as u64)
            > MAX_UNBOUND_ORDINARY_MAILBOX_BYTES_PER_RECIPIENT
    {
        return Err(AgentGraphError::ResourceLimit {
            resource: "ordinary unbound Mailbox bytes per recipient",
            limit: MAX_UNBOUND_ORDINARY_MAILBOX_BYTES_PER_RECIPIENT,
        });
    }
    if unbound_count >= MAX_UNBOUND_MAILBOX_MESSAGES_PER_RECIPIENT {
        return Err(AgentGraphError::ResourceLimit {
            resource: "unbound Mailbox messages per recipient",
            limit: MAX_UNBOUND_MAILBOX_MESSAGES_PER_RECIPIENT,
        });
    }
    if unbound_bytes.saturating_add(input.content.len() as u64)
        > MAX_UNBOUND_MAILBOX_BYTES_PER_RECIPIENT
    {
        return Err(AgentGraphError::ResourceLimit {
            resource: "unbound Mailbox bytes per recipient",
            limit: MAX_UNBOUND_MAILBOX_BYTES_PER_RECIPIENT,
        });
    }
    let projection_id_in_use = transaction
        .query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM agent_mailbox_messages WHERE projection_message_id = ?1
                 UNION ALL
                 SELECT 1 FROM messages WHERE id = ?1
             )",
            [&input.projection_message_id],
            |row| row.get::<_, bool>(0),
        )
        .map_err(read_error)?;
    if projection_id_in_use {
        return Err(conflict("Mailbox projection message ID is already in use"));
    }
    transaction
        .execute(
            "INSERT INTO agent_mailbox_messages (
                 message_id, schema_version, root_agent_id, sender_agent_id,
                 recipient_agent_id, request_id, kind, content, projection_message_id,
                 delivery_status, claim_token, lease_expires_at, created_at, claimed_at,
                 acknowledged_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9,
                       'queued', NULL, NULL, ?10, NULL, NULL)",
            params![
                &input.message_id,
                i64::from(AGENT_GRAPH_SCHEMA_VERSION),
                &input.root_agent_id,
                &input.sender_agent_id,
                &input.recipient_agent_id,
                &input.request_id,
                input.kind.as_str(),
                &input.content,
                &input.projection_message_id,
                created_at,
            ],
        )
        .map_err(write_error)?;
    let created = query_message(transaction, &input.message_id)?
        .ok_or_else(|| corrupt("created Mailbox message could not be read back"))?;
    Ok(IdempotentCreate::Created(created))
}

pub(super) fn validate_message_input(
    input: &EnqueueAgentMessageInput,
    created_at: i64,
) -> Result<(), AgentGraphError> {
    validate_id("message_id", &input.message_id)?;
    validate_id("root_agent_id", &input.root_agent_id)?;
    validate_id("sender_agent_id", &input.sender_agent_id)?;
    validate_id("recipient_agent_id", &input.recipient_agent_id)?;
    validate_request_id(&input.request_id)?;
    validate_id("projection_message_id", &input.projection_message_id)?;
    if input.sender_agent_id == input.recipient_agent_id {
        return Err(invalid(
            "recipient_agent_id",
            "must differ from sender Agent",
        ));
    }
    validate_bounded_text("content", &input.content, 1, MAX_MESSAGE_BYTES)?;
    validate_time(created_at)
}

pub(super) fn message_matches_enqueue(
    record: &AgentMailboxMessageRecord,
    input: &EnqueueAgentMessageInput,
) -> bool {
    record.message_id == input.message_id
        && record.root_agent_id == input.root_agent_id
        && record.sender_agent_id == input.sender_agent_id
        && record.recipient_agent_id == input.recipient_agent_id
        && record.request_id == input.request_id
        && record.kind == input.kind
        && record.content == input.content
        && record.projection_message_id == input.projection_message_id
}
