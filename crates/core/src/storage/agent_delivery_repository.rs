//! Durable model-batch admission for collaboration Mailbox facts.

use crate::storage::{
    agent_graph_repository, conversation_model_context_repository, conversation_trace_repository,
};
use crate::{
    AgentCollaborationCursorRecord, AgentDeliveredMailboxMessage, AgentDeliveryPath,
    AgentGraphError, AgentMailboxKind, AgentModelBatchDeliveryRecord,
    AgentModelBatchReceiptItemRecord, AgentModelBatchReceiptRecord,
    AgentModelBatchReceiptTargetRecord, AgentToolResult, AgentWaitModelProjection,
    AgentWaitReadySnapshot, AgentWaitTargetSnapshot, BindAgentSafeBoundaryInput,
    BindAgentTurnStartInput, ConversationTraceSnapshot, ConversationTurnTraceTerminalStatus,
    PollAgentWaitInput,
};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};

const MAX_BATCH_MESSAGES: usize = 1_024;
const MAX_SAFE_BOUNDARY_MESSAGES: usize = 64;
const MAX_SAFE_BOUNDARY_MODEL_BYTES: usize = 128 * 1_024;
const MAX_WAIT_TARGETS: usize = 32;
// Leave deterministic headroom for 32 bounded status snapshots and the ToolResult envelope. The
// final serialized result is independently checked against the shared 128 KiB hard ceiling.
const MAX_WAIT_MESSAGE_MODEL_BYTES: usize = 96 * 1_024;

#[derive(Debug, Clone)]
struct MailboxCandidate {
    message_id: String,
    sender_agent_id: String,
    sender_task_name: String,
    sender_task_path: String,
    kind: AgentMailboxKind,
    content: String,
    mailbox_sequence: u64,
    created_at: i64,
}

pub fn list_preloaded_agent_message_ids(
    connection: &Connection,
    conversation_id: &str,
    assistant_message_id: &str,
) -> Result<Vec<String>, AgentGraphError> {
    validate_identity(conversation_id, "conversation_id")?;
    validate_identity(assistant_message_id, "assistant_message_id")?;
    let assistant_position =
        active_turn_position(connection, conversation_id, assistant_message_id, None)?;
    let previous_assistant_position =
        previous_assistant_position(connection, conversation_id, assistant_position)?;
    let mut statement = connection
        .prepare(
            "SELECT mailbox.message_id
             FROM messages AS projection
             INNER JOIN agent_mailbox_messages AS mailbox
                ON mailbox.projection_message_id = projection.id
             LEFT JOIN agent_model_batch_receipt_items AS consumed
                ON consumed.message_id = mailbox.message_id
             WHERE projection.conversation_id = ?1
               AND projection.position > ?2
               AND projection.position < ?3
               AND mailbox.delivery_status = 'acknowledged'
               AND consumed.message_id IS NULL
             ORDER BY mailbox.sequence ASC",
        )
        .map_err(read_error)?;
    let records = statement
        .query_map(
            params![
                conversation_id,
                previous_assistant_position,
                assistant_position
            ],
            |row| row.get::<_, String>(0),
        )
        .map_err(read_error)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(read_error)?;
    Ok(records)
}

/// Resolves only the Agent projections which the caller has already selected for its prepared
/// model input. The order follows durable Mailbox sequence, never caller vector order.
pub fn filter_preloaded_agent_message_ids(
    connection: &Connection,
    conversation_id: &str,
    projected_message_ids: &[String],
) -> Result<Vec<String>, AgentGraphError> {
    validate_identity(conversation_id, "conversation_id")?;
    let mut selected = Vec::<(u64, String)>::new();
    let mut seen = HashSet::new();
    for projection_message_id in projected_message_ids {
        if !seen.insert(projection_message_id.as_str()) {
            continue;
        }
        let record = connection
            .query_row(
                "SELECT mailbox.sequence, mailbox.message_id
                 FROM agent_mailbox_messages AS mailbox
                 INNER JOIN messages AS projection
                    ON projection.id = mailbox.projection_message_id
                 LEFT JOIN agent_model_batch_receipt_items AS consumed
                    ON consumed.message_id = mailbox.message_id
                 WHERE projection.id = ?1
                   AND projection.conversation_id = ?2
                   AND mailbox.delivery_status = 'acknowledged'
                   AND consumed.message_id IS NULL",
                params![projection_message_id, conversation_id],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(read_error)?;
        if let Some((sequence, message_id)) = record {
            selected.push((positive_u64(sequence, "Mailbox sequence")?, message_id));
        }
    }
    selected.sort_by_key(|(sequence, _)| *sequence);
    Ok(selected
        .into_iter()
        .map(|(_, message_id)| message_id)
        .collect())
}

pub fn list_trace_bound_projection_message_ids(
    connection: &Connection,
    conversation_id: &str,
) -> Result<Vec<String>, AgentGraphError> {
    validate_identity(conversation_id, "conversation_id")?;
    let mut statement = connection
        .prepare(
            "SELECT mailbox.projection_message_id
             FROM agent_model_batch_receipt_items AS item
             INNER JOIN agent_model_batch_receipts AS receipt
                ON receipt.receipt_id = item.receipt_id
             INNER JOIN agent_mailbox_messages AS mailbox
                ON mailbox.message_id = item.message_id
             WHERE receipt.conversation_id = ?1
               AND (
                    item.delivery_path = 'safe_boundary'
                    OR (
                        item.delivery_path = 'wait_agent'
                        AND receipt.sampling_bound_at IS NOT NULL
                    )
               )
             ORDER BY mailbox.sequence ASC",
        )
        .map_err(read_error)?;
    let records = statement
        .query_map([conversation_id], |row| row.get::<_, String>(0))
        .map_err(read_error)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(read_error)?;
    Ok(records)
}

pub fn bind_turn_start_messages(
    connection: &mut Connection,
    input: &BindAgentTurnStartInput,
    preloaded_message_ids: &[String],
    bound_at: i64,
) -> Result<Option<AgentModelBatchDeliveryRecord>, AgentGraphError> {
    validate_turn_identity(
        &input.conversation_id,
        &input.run_id,
        &input.assistant_message_id,
        input.model_batch_index,
    )?;
    validate_time(bound_at)?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(write_error)?;
    let result = bind_turn_start_messages_in_transaction(
        &transaction,
        input,
        preloaded_message_ids,
        bound_at,
    )?;
    transaction.commit().map_err(write_error)?;
    Ok(result)
}

pub(crate) fn bind_turn_start_messages_in_transaction(
    connection: &Connection,
    input: &BindAgentTurnStartInput,
    preloaded_message_ids: &[String],
    bound_at: i64,
) -> Result<Option<AgentModelBatchDeliveryRecord>, AgentGraphError> {
    let Some(agent_id) = agent_for_active_turn(
        connection,
        &input.conversation_id,
        &input.run_id,
        &input.assistant_message_id,
    )?
    else {
        return Ok(None);
    };
    let receipt = ensure_receipt(
        connection,
        &agent_id,
        &input.conversation_id,
        &input.run_id,
        &input.assistant_message_id,
        input.model_batch_index,
        bound_at,
    )?;
    let mut seen = HashSet::new();
    let mut candidates = Vec::new();
    for message_id in preloaded_message_ids {
        validate_identity(message_id, "message_id")?;
        if !seen.insert(message_id.as_str()) {
            continue;
        }
        if let Some(existing) = query_item_by_message(connection, message_id)? {
            if existing.receipt_id != receipt.receipt_id
                || existing.delivery_path != AgentDeliveryPath::TurnStart
            {
                return Err(conflict(
                    "preloaded Mailbox input was already bound by another delivery path",
                ));
            }
            continue;
        }
        let candidate = query_acknowledged_candidate(connection, message_id)?
            .ok_or_else(|| conflict("preloaded Mailbox input is not acknowledged"))?;
        let recipient = connection
            .query_row(
                "SELECT recipient_agent_id FROM agent_mailbox_messages WHERE message_id = ?1",
                [message_id],
                |row| row.get::<_, String>(0),
            )
            .map_err(read_error)?;
        if recipient != agent_id {
            return Err(conflict(
                "preloaded Mailbox input does not belong to the active Agent",
            ));
        }
        candidates.push(candidate);
    }
    candidates.sort_by_key(|candidate| candidate.mailbox_sequence);
    let mut next_ordinal = next_receipt_ordinal(connection, &receipt.receipt_id)?;
    for candidate in &candidates {
        insert_receipt_item(
            connection,
            &receipt.receipt_id,
            candidate,
            next_ordinal,
            AgentDeliveryPath::TurnStart,
            None,
            bound_at,
        )?;
        next_ordinal = next_ordinal.saturating_add(1);
        // The currently admitted Wake is already running and therefore remains untouched. Any
        // later deferred Wake whose source was preloaded is safely coalesced.
        let _ = agent_graph_repository::satisfy_agent_wake_by_source_message_in_transaction(
            connection,
            &candidate.message_id,
            bound_at,
        )?;
    }
    touch_receipt(connection, &receipt.receipt_id, bound_at)?;
    let delivery = load_delivery(connection, &receipt.receipt_id, None)?;
    Ok(Some(delivery))
}

pub fn bind_safe_boundary(
    connection: &mut Connection,
    input: &BindAgentSafeBoundaryInput,
    bound_at: i64,
) -> Result<Option<AgentModelBatchDeliveryRecord>, AgentGraphError> {
    validate_turn_identity(
        &input.conversation_id,
        &input.run_id,
        &input.assistant_message_id,
        input.model_batch_index,
    )?;
    validate_time(bound_at)?;
    if input.maximum == 0 || input.maximum > MAX_BATCH_MESSAGES {
        return Err(invalid("maximum", "must be between 1 and 1024"));
    }
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(write_error)?;
    let Some(agent_id) = agent_for_active_turn(
        &transaction,
        &input.conversation_id,
        &input.run_id,
        &input.assistant_message_id,
    )?
    else {
        transaction.commit().map_err(write_error)?;
        return Ok(None);
    };
    let receipt = ensure_receipt(
        &transaction,
        &agent_id,
        &input.conversation_id,
        &input.run_id,
        &input.assistant_message_id,
        input.model_batch_index,
        bound_at,
    )?;
    if receipt.sampling_bound_at.is_some() {
        let delivery = load_delivery(
            &transaction,
            &receipt.receipt_id,
            Some(AgentDeliveryPath::SafeBoundary),
        )?;
        transaction.commit().map_err(write_error)?;
        return Ok((!delivery.messages.is_empty()).then_some(delivery));
    }

    let claim_prefix = format!("batch-{}", &receipt.receipt_id);
    let maximum = input.maximum.min(MAX_SAFE_BOUNDARY_MESSAGES);
    let _ = agent_graph_repository::project_pending_agent_messages_in_transaction(
        &transaction,
        &agent_id,
        &claim_prefix,
        bound_at,
        maximum,
    )?;
    let mut candidates = list_unbound_current_turn_candidates(
        &transaction,
        &agent_id,
        &input.conversation_id,
        &input.assistant_message_id,
        maximum,
        None,
    )?;
    let trace = conversation_trace_repository::get_trace_for_message(
        &transaction,
        &input.assistant_message_id,
    )
    .map_err(read_error)?
    .ok_or_else(|| corrupt("active Conversation trace disappeared"))?;
    if trace.run_id != input.run_id
        || trace.conversation_id != input.conversation_id
        || trace.terminal_status != ConversationTurnTraceTerminalStatus::InProgress
    {
        return Err(conflict("model batch no longer belongs to the active Turn"));
    }
    let model_items = conversation_model_context_repository::get_log_for_message(
        &transaction,
        &input.assistant_message_id,
    )
    .map_err(read_error)?
    .map(|log| log.items)
    .unwrap_or_default();
    let next_sequence = trace
        .items
        .last()
        .map(crate::ConversationTurnTraceItem::sequence)
        .map_or(0, |sequence| sequence.saturating_add(1));
    if next_sequence != input.expected_next_trace_sequence {
        return Err(conflict("model batch expected trace sequence is stale"));
    }
    let mut recorder = crate::conversation_trace::ConversationTraceRecorder::from_durable_snapshot(
        ConversationTraceSnapshot {
            items: trace.items,
            model_context_items: model_items,
            next_sequence,
            truncated: trace.truncated,
        },
    );
    candidates.sort_by_key(|candidate| candidate.mailbox_sequence);
    let mut selected = Vec::new();
    let mut model_bytes = 0usize;
    for candidate in candidates {
        let envelope = crate::conversation_trace::project_agent_mailbox_model_envelope(
            &candidate.sender_agent_id,
            &candidate.sender_task_name,
            &candidate.sender_task_path,
            candidate.kind,
            candidate.content.trim(),
        )
        .map_err(corrupt)?
        .0;
        if !selected.is_empty()
            && model_bytes.saturating_add(envelope.len()) > MAX_SAFE_BOUNDARY_MODEL_BYTES
        {
            break;
        }
        model_bytes = model_bytes.saturating_add(envelope.len());
        selected.push(candidate);
    }
    let mut traced = Vec::with_capacity(selected.len());
    for candidate in selected {
        let sequence = recorder.next_sequence();
        let content = recorder
            .record_agent_mailbox_delivery(
                sequence,
                &receipt.receipt_id,
                &candidate.message_id,
                &candidate.sender_agent_id,
                &candidate.sender_task_name,
                &candidate.sender_task_path,
                candidate.kind,
                &candidate.content,
                candidate.created_at,
            )
            .map_err(corrupt)?
            .ok_or_else(|| corrupt("new Mailbox candidate unexpectedly matched trace"))?;
        traced.push((candidate, sequence, content));
    }
    let snapshot = recorder.snapshot();
    if !traced.is_empty() {
        let committed_trace = snapshot.in_progress_trace(
            &input.run_id,
            &input.conversation_id,
            &input.assistant_message_id,
        );
        let trace_created_at = trace_created_at(&transaction, &input.assistant_message_id)?;
        conversation_trace_repository::commit_trace_in_connection(
            &transaction,
            &committed_trace,
            trace_created_at,
            bound_at.max(trace_created_at),
        )
        .map_err(write_error)?;
        conversation_model_context_repository::commit_items_in_connection(
            &transaction,
            &input.conversation_id,
            &input.assistant_message_id,
            &snapshot.model_context_items,
        )
        .map_err(write_error)?;
    }
    let mut next_ordinal = next_receipt_ordinal(&transaction, &receipt.receipt_id)?;
    for (candidate, trace_sequence, _) in &traced {
        insert_receipt_item(
            &transaction,
            &receipt.receipt_id,
            candidate,
            next_ordinal,
            AgentDeliveryPath::SafeBoundary,
            Some(*trace_sequence),
            bound_at,
        )?;
        next_ordinal = next_ordinal.saturating_add(1);
        let _ = agent_graph_repository::satisfy_agent_wake_by_source_message_in_transaction(
            &transaction,
            &candidate.message_id,
            bound_at,
        )?;
    }
    transaction
        .execute(
            "UPDATE agent_model_batch_receipts
             SET sampling_bound_at = ?1, updated_at = MAX(updated_at, ?1)
             WHERE receipt_id = ?2 AND sampling_bound_at IS NULL",
            params![bound_at, &receipt.receipt_id],
        )
        .map_err(write_error)?;
    let delivery = load_delivery(
        &transaction,
        &receipt.receipt_id,
        Some(AgentDeliveryPath::SafeBoundary),
    )?;
    transaction.commit().map_err(write_error)?;
    Ok((!delivery.messages.is_empty()).then_some(delivery))
}

/// Atomically polls caller-visible descendant state and binds first-ready Mailbox items. This
/// function never queries or claims the target's inbox.
///
/// Long-lived waiters should call [`probe_wait_ready`] between settlement attempts. The probe is
/// read-only and intentionally permits false positives; this function remains the sole authority
/// that advances cursors and freezes a model-visible result.
pub fn poll_wait_ready(
    connection: &mut Connection,
    input: &PollAgentWaitInput,
    polled_at: i64,
) -> Result<Option<AgentWaitReadySnapshot>, AgentGraphError> {
    validate_turn_identity(
        &input.conversation_id,
        &input.run_id,
        &input.assistant_message_id,
        input.model_batch_index,
    )?;
    validate_identity(&input.caller_agent_id, "caller_agent_id")?;
    validate_time(polled_at)?;
    if input.target_agent_ids.is_empty() {
        return Err(invalid("target_agent_ids", "must not be empty"));
    }
    if input.maximum_messages == 0 || input.maximum_messages > MAX_SAFE_BOUNDARY_MESSAGES {
        return Err(invalid("maximum_messages", "must be between 1 and 64"));
    }
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(write_error)?;
    let active_agent = agent_for_active_turn(
        &transaction,
        &input.conversation_id,
        &input.run_id,
        &input.assistant_message_id,
    )?
    .ok_or_else(|| conflict("wait caller is not an active Agent Turn"))?;
    if active_agent != input.caller_agent_id {
        return Err(conflict(
            "wait caller identity does not own the active Turn",
        ));
    }
    let mut targets = input.target_agent_ids.clone();
    targets.sort();
    targets.dedup();
    if targets.len() > MAX_WAIT_TARGETS {
        return Err(invalid(
            "target_agent_ids",
            "must contain at most 32 distinct targets",
        ));
    }
    for target in &targets {
        validate_identity(target, "target_agent_id")?;
        if !is_strict_descendant(&transaction, &input.caller_agent_id, target)? {
            return Err(conflict(
                "wait target must be the caller's strict descendant",
            ));
        }
    }
    let receipt = ensure_receipt(
        &transaction,
        &input.caller_agent_id,
        &input.conversation_id,
        &input.run_id,
        &input.assistant_message_id,
        input.model_batch_index,
        polled_at,
    )?;
    let frozen_targets = load_wait_receipt_targets(&transaction, &receipt.receipt_id)?;
    if !frozen_targets.is_empty() {
        let existing_wait = load_delivery(
            &transaction,
            &receipt.receipt_id,
            Some(AgentDeliveryPath::WaitAgent),
        )?;
        let snapshots =
            wait_snapshot_from_frozen(receipt.clone(), frozen_targets, existing_wait.messages)
                .targets;
        let receipt = if receipt.sampling_bound_at.is_none() {
            commit_wait_snapshot_to_model_batch(
                &transaction,
                &receipt,
                &input.run_id,
                &input.conversation_id,
                &input.assistant_message_id,
                &snapshots,
                polled_at,
            )?
        } else {
            receipt
        };
        let source_receipt_id = wait_replay_source(&transaction, &receipt.receipt_id)?;
        let snapshot = AgentWaitReadySnapshot {
            receipt,
            source_receipt_id,
            targets: snapshots,
            model_projection: AgentWaitModelProjection::PrecommittedToolResult,
        };
        transaction.commit().map_err(write_error)?;
        return Ok(Some(snapshot));
    }
    if let Some((mut replay, source_receipt_id)) =
        load_open_wait_snapshot_from_previous_run(&transaction, &receipt, &targets, polled_at)?
    {
        transaction
            .execute(
                "INSERT INTO agent_model_batch_receipt_replays (
                     receipt_id, source_receipt_id, created_at
                 ) VALUES (?1, ?2, ?3)",
                params![&receipt.receipt_id, &source_receipt_id, polled_at],
            )
            .map_err(write_error)?;
        replay.receipt = commit_wait_snapshot_to_model_batch(
            &transaction,
            &receipt,
            &input.run_id,
            &input.conversation_id,
            &input.assistant_message_id,
            &replay.targets,
            polled_at,
        )?;
        replay.source_receipt_id = Some(source_receipt_id.clone());
        transaction
            .execute(
                "UPDATE agent_model_batch_receipts
                 SET sampling_bound_at = ?1, updated_at = MAX(updated_at, ?1)
                 WHERE receipt_id = ?2 AND sampling_bound_at IS NULL",
                params![polled_at, source_receipt_id],
            )
            .map_err(write_error)?;
        transaction.commit().map_err(write_error)?;
        return Ok(Some(replay));
    }
    if receipt.sampling_bound_at.is_some() {
        return Err(conflict(
            "wait cannot bind facts to a model batch whose sampling boundary is closed",
        ));
    }
    let claim_prefix = stable_id("wait-claim", &[&input.run_id, &targets.join("|")]);
    let _ = agent_graph_repository::project_pending_agent_messages_in_transaction(
        &transaction,
        &input.caller_agent_id,
        &claim_prefix,
        polled_at,
        MAX_BATCH_MESSAGES,
    )?;
    let mut states = BTreeMap::new();
    let mut all_candidates = Vec::new();
    for target in &targets {
        let cursor = query_cursor(&transaction, &input.caller_agent_id, &input.run_id, target)?;
        let latest_wake = latest_wake_for_target(&transaction, target)?;
        let target_status_version = target_status_version(&transaction, target)?;
        all_candidates.extend(list_wait_candidates(
            &transaction,
            &input.caller_agent_id,
            target,
            cursor
                .as_ref()
                .map_or(0, |cursor| cursor.last_message_sequence),
            input.maximum_messages,
        )?);
        let display_status =
            agent_graph_repository::get_agent_display_status(&transaction, target)?.status;
        let status_advanced = cursor.as_ref().map_or_else(
            || {
                latest_wake
                    .as_ref()
                    .is_some_and(|wake| wake.status.is_terminal())
                    || matches!(
                        display_status,
                        crate::AgentDisplayStatus::Archived | crate::AgentDisplayStatus::Disabled
                    )
            },
            |cursor| target_status_version > cursor.last_target_status_version,
        );
        states.insert(
            target.clone(),
            WaitTargetPollState {
                latest_wake: latest_wake.clone(),
                target_status_version,
                display_status,
                status_advanced,
            },
        );
        // Establish or advance only the status baseline. Message cursors advance below only for
        // globally admitted FIFO items, so a budget boundary can never strand a later message.
        upsert_cursor(
            &transaction,
            &input.caller_agent_id,
            &input.run_id,
            target,
            target_status_version,
            cursor
                .as_ref()
                .map_or(0, |cursor| cursor.last_message_sequence),
            latest_wake.as_ref().map_or(0, |wake| wake.sequence),
            latest_wake.as_ref().map_or(0, |wake| wake.status_revision),
            polled_at,
        )?;
    }
    all_candidates.sort_by_key(|candidate| candidate.mailbox_sequence);
    all_candidates.dedup_by(|left, right| left.message_id == right.message_id);
    let mut new_candidates = Vec::new();
    let mut projected_by_sender = BTreeMap::<String, Vec<AgentDeliveredMailboxMessage>>::new();
    let mut remaining_model_bytes = MAX_WAIT_MESSAGE_MODEL_BYTES;
    for candidate in all_candidates {
        if new_candidates.len() >= input.maximum_messages {
            break;
        }
        // Wait returns the same authenticated, deterministic 32 KiB envelope as the automatic
        // safe boundary. A one-megabyte Mailbox payload therefore cannot starve the FIFO head.
        let projected = wait_delivered(&candidate)?;
        let serialized = serde_json::to_vec(&projected)
            .map_err(|error| corrupt(format!("wait message serialization failed: {error}")))?;
        if serialized.len() > remaining_model_bytes {
            break;
        }
        remaining_model_bytes = remaining_model_bytes.saturating_sub(serialized.len());
        projected_by_sender
            .entry(candidate.sender_agent_id.clone())
            .or_default()
            .push(projected);
        new_candidates.push(candidate);
    }
    let mut snapshots = Vec::new();
    for target in &targets {
        let state = states
            .get(target)
            .ok_or_else(|| corrupt("wait target state disappeared"))?;
        let messages = projected_by_sender.remove(target).unwrap_or_default();
        if !messages.is_empty() || state.status_advanced {
            snapshots.push(AgentWaitTargetSnapshot {
                target_agent_id: target.clone(),
                messages,
                target_status_version: state.target_status_version,
                latest_wake_sequence: state.latest_wake.as_ref().map(|wake| wake.sequence),
                latest_wake_status_revision: state
                    .latest_wake
                    .as_ref()
                    .map(|wake| wake.status_revision),
                latest_wake_status: state.latest_wake.as_ref().map(|wake| wake.status),
                display_status: state.display_status,
            });
        }
    }
    if snapshots.is_empty() {
        transaction.commit().map_err(write_error)?;
        return Ok(None);
    }
    let mut next_ordinal = next_receipt_ordinal(&transaction, &receipt.receipt_id)?;
    for candidate in &new_candidates {
        if query_item_by_message(&transaction, &candidate.message_id)?.is_none() {
            insert_receipt_item(
                &transaction,
                &receipt.receipt_id,
                candidate,
                next_ordinal,
                AgentDeliveryPath::WaitAgent,
                None,
                polled_at,
            )?;
            let _ = agent_graph_repository::satisfy_agent_wake_by_source_message_in_transaction(
                &transaction,
                &candidate.message_id,
                polled_at,
            )?;
            next_ordinal = next_ordinal.saturating_add(1);
        }
        upsert_cursor(
            &transaction,
            &input.caller_agent_id,
            &input.run_id,
            &candidate.sender_agent_id,
            snapshots
                .iter()
                .find(|snapshot| snapshot.target_agent_id == candidate.sender_agent_id)
                .map_or(0, |snapshot| snapshot.target_status_version),
            candidate.mailbox_sequence,
            snapshots
                .iter()
                .find(|snapshot| snapshot.target_agent_id == candidate.sender_agent_id)
                .and_then(|snapshot| snapshot.latest_wake_sequence)
                .unwrap_or(0),
            snapshots
                .iter()
                .find(|snapshot| snapshot.target_agent_id == candidate.sender_agent_id)
                .and_then(|snapshot| snapshot.latest_wake_status_revision)
                .unwrap_or(0),
            polled_at,
        )?;
    }
    touch_receipt(&transaction, &receipt.receipt_id, polled_at)?;
    let receipt = query_receipt(&transaction, &receipt.receipt_id)?
        .ok_or_else(|| corrupt("wait receipt disappeared"))?;
    // Re-read message payloads from the immutable receipt so retries return exactly the winner.
    let wait_delivery = load_delivery(
        &transaction,
        &receipt.receipt_id,
        Some(AgentDeliveryPath::WaitAgent),
    )?;
    for snapshot in &mut snapshots {
        snapshot.messages = wait_delivery
            .messages
            .iter()
            .filter(|message| message.sender_agent_id == snapshot.target_agent_id)
            .cloned()
            .collect();
    }
    for (ordinal, snapshot) in snapshots.iter().enumerate() {
        insert_wait_receipt_target(
            &transaction,
            &receipt.receipt_id,
            u32::try_from(ordinal).map_err(|_| corrupt("wait target ordinal overflow"))?,
            snapshot,
            polled_at,
        )?;
    }
    let receipt = commit_wait_snapshot_to_model_batch(
        &transaction,
        &receipt,
        &input.run_id,
        &input.conversation_id,
        &input.assistant_message_id,
        &snapshots,
        polled_at,
    )?;
    transaction.commit().map_err(write_error)?;
    Ok(Some(AgentWaitReadySnapshot {
        receipt,
        source_receipt_id: None,
        targets: snapshots,
        model_projection: AgentWaitModelProjection::PrecommittedToolResult,
    }))
}

/// Cheap read-only readiness probe for an already initialized `wait_agent` call.
///
/// `poll_wait_ready` establishes the receipt and status baselines once. Subsequent idle checks use
/// this path so they neither acquire an IMMEDIATE transaction nor rewrite unchanged cursors. A
/// `true` result is only a hint: the caller must re-enter `poll_wait_ready` to settle atomically.
pub fn probe_wait_ready(
    connection: &Connection,
    input: &PollAgentWaitInput,
    probed_at: i64,
) -> Result<bool, AgentGraphError> {
    validate_turn_identity(
        &input.conversation_id,
        &input.run_id,
        &input.assistant_message_id,
        input.model_batch_index,
    )?;
    validate_identity(&input.caller_agent_id, "caller_agent_id")?;
    validate_time(probed_at)?;
    if input.target_agent_ids.is_empty() {
        return Err(invalid("target_agent_ids", "must not be empty"));
    }
    if input.maximum_messages == 0 || input.maximum_messages > MAX_SAFE_BOUNDARY_MESSAGES {
        return Err(invalid("maximum_messages", "must be between 1 and 64"));
    }
    let active_agent = agent_for_active_turn(
        connection,
        &input.conversation_id,
        &input.run_id,
        &input.assistant_message_id,
    )?
    .ok_or_else(|| conflict("wait caller is not an active Agent Turn"))?;
    if active_agent != input.caller_agent_id {
        return Err(conflict(
            "wait caller identity does not own the active Turn",
        ));
    }
    let mut targets = input.target_agent_ids.clone();
    targets.sort();
    targets.dedup();
    if targets.len() > MAX_WAIT_TARGETS {
        return Err(invalid(
            "target_agent_ids",
            "must contain at most 32 distinct targets",
        ));
    }
    for target in &targets {
        validate_identity(target, "target_agent_id")?;
        if !is_strict_descendant(connection, &input.caller_agent_id, target)? {
            return Err(conflict(
                "wait target must be the caller's strict descendant",
            ));
        }
    }

    let Some(receipt) = query_receipt_by_batch(connection, &input.run_id, input.model_batch_index)?
    else {
        // Initialization or crash recovery still belongs to the authoritative transaction.
        return Ok(true);
    };
    if receipt.agent_id != input.caller_agent_id
        || receipt.conversation_id != input.conversation_id
        || receipt.assistant_message_id != input.assistant_message_id
    {
        return Err(conflict("model batch receipt identity conflict"));
    }
    if receipt.sampling_bound_at.is_some()
        || !load_wait_receipt_targets(connection, &receipt.receipt_id)?.is_empty()
        || query_open_wait_receipt_id(connection, &receipt)?.is_some()
    {
        return Ok(true);
    }

    for target in &targets {
        let Some(cursor) = query_cursor(connection, &input.caller_agent_id, &input.run_id, target)?
        else {
            // The initial settlement pass owns baseline creation.
            return Ok(true);
        };
        if target_status_version(connection, target)? > cursor.last_target_status_version {
            return Ok(true);
        }
        let has_message = connection
            .query_row(
                "SELECT EXISTS (
                     SELECT 1
                     FROM agent_mailbox_messages AS mailbox
                     LEFT JOIN agent_model_batch_receipt_items AS consumed
                       ON consumed.message_id = mailbox.message_id
                     WHERE mailbox.recipient_agent_id = ?1
                       AND mailbox.sender_agent_id = ?2
                       AND mailbox.sequence > ?3
                       AND (
                           mailbox.delivery_status IN ('queued', 'acknowledged')
                           OR (
                               mailbox.delivery_status = 'claimed'
                               AND COALESCE(mailbox.lease_expires_at, 0) <= ?4
                           )
                       )
                       AND consumed.message_id IS NULL
                 )",
                params![
                    &input.caller_agent_id,
                    target,
                    u64_to_sql(cursor.last_message_sequence)?,
                    probed_at
                ],
                |row| row.get::<_, bool>(0),
            )
            .map_err(read_error)?;
        if has_message {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Closes the wait Tool exchange and its batch receipt in the same SQLite transaction which
/// froze the first-ready snapshot. A crash can therefore expose either the still-pending Mailbox
/// facts or a complete durable ToolResult/model-context fact, never a consumed cursor without a
/// replayable model-visible result.
fn commit_wait_snapshot_to_model_batch(
    connection: &Connection,
    receipt: &AgentModelBatchReceiptRecord,
    run_id: &str,
    conversation_id: &str,
    assistant_message_id: &str,
    snapshots: &[AgentWaitTargetSnapshot],
    sampled_at: i64,
) -> Result<AgentModelBatchReceiptRecord, AgentGraphError> {
    let trace =
        conversation_trace_repository::get_trace_for_message(connection, assistant_message_id)
            .map_err(read_error)?
            .ok_or_else(|| corrupt("wait Tool trace disappeared"))?;
    if trace.run_id != run_id
        || trace.conversation_id != conversation_id
        || trace.terminal_status != ConversationTurnTraceTerminalStatus::InProgress
    {
        return Err(conflict("wait result no longer belongs to the active Turn"));
    }
    let (call_id, tool) = match trace.items.last() {
        Some(crate::ConversationTurnTraceItem::ToolCall { call_id, tool, .. }) => {
            (call_id.clone(), tool.clone())
        }
        _ => {
            return Err(conflict(
                "wait result requires the final unresolved trusted wait_agent ToolCall",
            ))
        }
    };
    if tool != "wait_agent" {
        return Err(conflict(
            "wait result cannot close a non-wait_agent ToolCall",
        ));
    }
    let model_items = conversation_model_context_repository::get_log_for_message(
        connection,
        assistant_message_id,
    )
    .map_err(read_error)?
    .map(|log| log.items)
    .unwrap_or_default();
    let result = AgentToolResult {
        exact_archive_file: None,
        call_id,
        tool,
        ok: true,
        result: Some(json!({
            "receiptId": receipt.receipt_id,
            "sourceReceiptId": connection
                .query_row(
                    "SELECT source_receipt_id
                     FROM agent_model_batch_receipt_replays WHERE receipt_id = ?1",
                    [&receipt.receipt_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(read_error)?,
            "targets": snapshots,
        })),
        error: None,
    };
    let serialized = serde_json::to_vec(result.result.as_ref().unwrap_or(&serde_json::Value::Null))
        .map_err(|error| corrupt(format!("wait result serialization failed: {error}")))?;
    if serialized.len() > MAX_SAFE_BOUNDARY_MODEL_BYTES {
        return Err(conflict(
            "wait result exceeds the durable model-visible collaboration budget",
        ));
    }
    let snapshot =
        crate::conversation_trace::conversation_trace_snapshot_with_recovered_tool_result(
            &trace,
            model_items,
            &result,
        )
        .map_err(conflict)?;
    let committed_trace =
        snapshot.in_progress_audit_trace(run_id, conversation_id, assistant_message_id);
    let trace_created_at = trace_created_at(connection, assistant_message_id)?;
    conversation_trace_repository::commit_trace_in_connection(
        connection,
        &committed_trace,
        trace_created_at,
        sampled_at.max(trace_created_at),
    )
    .map_err(write_error)?;
    conversation_model_context_repository::commit_items_in_connection(
        connection,
        conversation_id,
        assistant_message_id,
        &snapshot.model_context_items,
    )
    .map_err(write_error)?;
    connection
        .execute(
            "UPDATE agent_model_batch_receipts
             SET sampling_bound_at = ?1, updated_at = MAX(updated_at, ?1)
             WHERE receipt_id = ?2 AND sampling_bound_at IS NULL",
            params![sampled_at, &receipt.receipt_id],
        )
        .map_err(write_error)?;
    query_receipt(connection, &receipt.receipt_id)?
        .ok_or_else(|| corrupt("sampled wait receipt disappeared"))
}

/// Copies an abandoned/open wait fact into the new active Turn's batch without deleting or
/// rewriting the original receipt. This is the status-only crash recovery path: the old cursor
/// may already have advanced, but the immutable target snapshot remains replayable until some
/// batch reaches `sampling_bound_at`.
fn load_open_wait_snapshot_from_previous_run(
    connection: &Connection,
    current_receipt: &AgentModelBatchReceiptRecord,
    requested_targets: &[String],
    rebound_at: i64,
) -> Result<Option<(AgentWaitReadySnapshot, String)>, AgentGraphError> {
    let previous = query_open_wait_receipt_id(connection, current_receipt)?;
    let Some(previous_receipt_id) = previous else {
        return Ok(None);
    };
    let previous_targets = load_wait_receipt_targets(connection, &previous_receipt_id)?;
    let requested = requested_targets
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    if previous_targets
        .iter()
        .any(|target| !requested.contains(target.target_agent_id.as_str()))
    {
        return Err(conflict(
            "an open wait receipt must be recovered with the same target authorization set",
        ));
    }
    let previous_delivery = load_delivery(
        connection,
        &previous_receipt_id,
        Some(AgentDeliveryPath::WaitAgent),
    )?;
    let snapshots = previous_targets
        .iter()
        .map(|target| AgentWaitTargetSnapshot {
            target_agent_id: target.target_agent_id.clone(),
            messages: previous_delivery
                .messages
                .iter()
                .filter(|message| message.sender_agent_id == target.target_agent_id)
                .cloned()
                .collect(),
            target_status_version: target.target_status_version,
            latest_wake_sequence: target.latest_wake_sequence,
            latest_wake_status_revision: target.latest_wake_status_revision,
            latest_wake_status: target.latest_wake_status,
            display_status: target.display_status,
        })
        .collect::<Vec<_>>();
    for (ordinal, snapshot) in snapshots.iter().enumerate() {
        insert_wait_receipt_target(
            connection,
            &current_receipt.receipt_id,
            u32::try_from(ordinal).map_err(|_| corrupt("wait target ordinal overflow"))?,
            snapshot,
            rebound_at,
        )?;
    }
    let receipt = query_receipt(connection, &current_receipt.receipt_id)?
        .ok_or_else(|| corrupt("rebound wait receipt disappeared"))?;
    Ok(Some((
        AgentWaitReadySnapshot {
            receipt,
            source_receipt_id: None,
            targets: snapshots,
            model_projection: AgentWaitModelProjection::PrecommittedToolResult,
        },
        previous_receipt_id,
    )))
}

fn query_open_wait_receipt_id(
    connection: &Connection,
    current_receipt: &AgentModelBatchReceiptRecord,
) -> Result<Option<String>, AgentGraphError> {
    connection
        .query_row(
            "SELECT receipt.receipt_id
             FROM agent_model_batch_receipts AS receipt
             WHERE receipt.agent_id = ?1
               AND receipt.run_id != ?2
               AND receipt.sampling_bound_at IS NULL
               AND EXISTS (
                   SELECT 1 FROM agent_model_batch_receipt_targets AS old_target
                   WHERE old_target.receipt_id = receipt.receipt_id
                     AND NOT EXISTS (
                         SELECT 1
                         FROM agent_model_batch_receipt_targets AS sampled_target
                         JOIN agent_model_batch_receipts AS sampled
                           ON sampled.receipt_id = sampled_target.receipt_id
                         WHERE sampled.agent_id = receipt.agent_id
                           AND sampled.sampling_bound_at IS NOT NULL
                           AND sampled_target.target_agent_id = old_target.target_agent_id
                           AND sampled_target.target_status_version
                               >= old_target.target_status_version
                     )
               )
             ORDER BY receipt.created_at ASC, receipt.receipt_id ASC
             LIMIT 1",
            params![&current_receipt.agent_id, &current_receipt.run_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(read_error)
}

fn wait_replay_source(
    connection: &Connection,
    receipt_id: &str,
) -> Result<Option<String>, AgentGraphError> {
    connection
        .query_row(
            "SELECT source_receipt_id FROM agent_model_batch_receipt_replays WHERE receipt_id = ?1",
            [receipt_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(read_error)
}

fn insert_wait_receipt_target(
    connection: &Connection,
    receipt_id: &str,
    ordinal: u32,
    snapshot: &AgentWaitTargetSnapshot,
    frozen_at: i64,
) -> Result<(), AgentGraphError> {
    connection
        .execute(
            "INSERT INTO agent_model_batch_receipt_targets (
                 receipt_id, target_agent_id, ordinal, target_status_version, latest_wake_sequence,
                 latest_wake_status_revision, latest_wake_status, display_status, frozen_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                receipt_id,
                &snapshot.target_agent_id,
                i64::from(ordinal),
                u64_to_sql(snapshot.target_status_version)?,
                snapshot.latest_wake_sequence.map(u64_to_sql).transpose()?,
                snapshot
                    .latest_wake_status_revision
                    .map(u64_to_sql)
                    .transpose()?,
                snapshot
                    .latest_wake_status
                    .map(crate::AgentWakeStatus::as_str),
                snapshot.display_status.as_str(),
                frozen_at,
            ],
        )
        .map_err(write_error)?;
    Ok(())
}

fn load_wait_receipt_targets(
    connection: &Connection,
    receipt_id: &str,
) -> Result<Vec<AgentModelBatchReceiptTargetRecord>, AgentGraphError> {
    let mut statement = connection
        .prepare(
            "SELECT receipt_id, target_agent_id, ordinal, latest_wake_sequence,
                    target_status_version, latest_wake_status_revision, latest_wake_status,
                    display_status, frozen_at
             FROM agent_model_batch_receipt_targets
             WHERE receipt_id = ?1
             ORDER BY ordinal ASC",
        )
        .map_err(read_error)?;
    let rows = statement
        .query_map([receipt_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(4)?,
                row.get::<_, Option<i64>>(3)?,
                row.get::<_, Option<i64>>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, i64>(8)?,
            ))
        })
        .map_err(read_error)?;
    rows.map(|row| {
        let (
            receipt_id,
            target_agent_id,
            ordinal,
            target_status_version,
            wake_sequence,
            wake_revision,
            wake_status,
            display_status,
            frozen_at,
        ) = row.map_err(read_error)?;
        Ok(AgentModelBatchReceiptTargetRecord {
            receipt_id,
            target_agent_id,
            ordinal: u32::try_from(ordinal)
                .map_err(|_| corrupt("wait target ordinal is invalid"))?,
            target_status_version: positive_u64(target_status_version, "target status version")?,
            latest_wake_sequence: wake_sequence
                .map(|value| positive_u64(value, "wake sequence"))
                .transpose()?,
            latest_wake_status_revision: wake_revision
                .map(|value| positive_u64(value, "wake status revision"))
                .transpose()?,
            latest_wake_status: wake_status
                .as_deref()
                .map(crate::AgentWakeStatus::parse)
                .transpose()?,
            display_status: crate::AgentDisplayStatus::parse(&display_status)?,
            frozen_at,
        })
    })
    .collect()
}

fn wait_snapshot_from_frozen(
    receipt: AgentModelBatchReceiptRecord,
    targets: Vec<AgentModelBatchReceiptTargetRecord>,
    messages: Vec<AgentDeliveredMailboxMessage>,
) -> AgentWaitReadySnapshot {
    AgentWaitReadySnapshot {
        receipt,
        source_receipt_id: None,
        targets: targets
            .into_iter()
            .map(|target| AgentWaitTargetSnapshot {
                messages: messages
                    .iter()
                    .filter(|message| message.sender_agent_id == target.target_agent_id)
                    .cloned()
                    .collect(),
                target_agent_id: target.target_agent_id,
                target_status_version: target.target_status_version,
                latest_wake_sequence: target.latest_wake_sequence,
                latest_wake_status_revision: target.latest_wake_status_revision,
                latest_wake_status: target.latest_wake_status,
                display_status: target.display_status,
            })
            .collect(),
        model_projection: AgentWaitModelProjection::PrecommittedToolResult,
    }
}

fn active_turn_position(
    connection: &Connection,
    conversation_id: &str,
    assistant_message_id: &str,
    run_id: Option<&str>,
) -> Result<i64, AgentGraphError> {
    connection
        .query_row(
            "SELECT message.position
             FROM messages AS message
             INNER JOIN conversation_turn_traces AS trace
                ON trace.assistant_message_id = message.id
             WHERE message.conversation_id = ?1
               AND message.id = ?2
               AND trace.terminal_status = 'in_progress'
               AND (?3 IS NULL OR trace.run_id = ?3)",
            params![conversation_id, assistant_message_id, run_id],
            |row| row.get(0),
        )
        .map_err(read_error)
}

fn previous_assistant_position(
    connection: &Connection,
    conversation_id: &str,
    active_position: i64,
) -> Result<i64, AgentGraphError> {
    connection
        .query_row(
            "SELECT COALESCE(MAX(position), -1)
             FROM messages
             WHERE conversation_id = ?1 AND role = 'assistant' AND position < ?2",
            params![conversation_id, active_position],
            |row| row.get(0),
        )
        .map_err(read_error)
}

fn agent_for_active_turn(
    connection: &Connection,
    conversation_id: &str,
    run_id: &str,
    assistant_message_id: &str,
) -> Result<Option<String>, AgentGraphError> {
    connection
        .query_row(
            "SELECT agent.agent_id
             FROM agent_nodes AS agent
             INNER JOIN conversation_turn_traces AS trace
                ON trace.conversation_id = agent.conversation_id
             WHERE agent.conversation_id = ?1
               AND trace.run_id = ?2
               AND trace.assistant_message_id = ?3
               AND trace.terminal_status = 'in_progress'
               AND agent.lifecycle = 'active'",
            params![conversation_id, run_id, assistant_message_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(read_error)
}

fn ensure_receipt(
    connection: &Connection,
    agent_id: &str,
    conversation_id: &str,
    run_id: &str,
    assistant_message_id: &str,
    model_batch_index: u64,
    created_at: i64,
) -> Result<AgentModelBatchReceiptRecord, AgentGraphError> {
    let receipt_id = stable_id("agent-batch", &[run_id, &model_batch_index.to_string()]);
    if let Some(existing) = query_receipt_by_batch(connection, run_id, model_batch_index)? {
        if existing.receipt_id != receipt_id
            || existing.agent_id != agent_id
            || existing.conversation_id != conversation_id
            || existing.assistant_message_id != assistant_message_id
        {
            return Err(conflict("model batch receipt identity conflict"));
        }
        return Ok(existing);
    }
    connection
        .execute(
            "INSERT INTO agent_model_batch_receipts (
                 receipt_id, agent_id, conversation_id, run_id, assistant_message_id,
                 model_batch_index, sampling_bound_at, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, ?7, ?7)",
            params![
                &receipt_id,
                agent_id,
                conversation_id,
                run_id,
                assistant_message_id,
                u64_to_sql(model_batch_index)?,
                created_at,
            ],
        )
        .map_err(write_error)?;
    query_receipt(connection, &receipt_id)?
        .ok_or_else(|| corrupt("created model batch receipt could not be read back"))
}

fn list_unbound_current_turn_candidates(
    connection: &Connection,
    recipient_agent_id: &str,
    conversation_id: &str,
    assistant_message_id: &str,
    maximum: usize,
    sender_filter: Option<&str>,
) -> Result<Vec<MailboxCandidate>, AgentGraphError> {
    let assistant_position =
        active_turn_position(connection, conversation_id, assistant_message_id, None)?;
    let previous_position =
        previous_assistant_position(connection, conversation_id, assistant_position)?;
    let mut statement = connection
        .prepare(
            "SELECT mailbox.message_id, mailbox.sender_agent_id,
                    sender.task_name, sender.task_path, mailbox.kind,
                    mailbox.content, mailbox.sequence, mailbox.created_at
             FROM agent_mailbox_messages AS mailbox
             INNER JOIN agent_nodes AS sender ON sender.agent_id = mailbox.sender_agent_id
             INNER JOIN messages AS projection
                ON projection.id = mailbox.projection_message_id
             LEFT JOIN agent_model_batch_receipt_items AS consumed
                ON consumed.message_id = mailbox.message_id
             WHERE mailbox.recipient_agent_id = ?1
               AND mailbox.delivery_status = 'acknowledged'
               AND projection.conversation_id = ?2
               AND projection.position > ?3
               AND projection.position < ?4
               AND consumed.message_id IS NULL
               AND (?5 IS NULL OR mailbox.sender_agent_id = ?5)
             ORDER BY mailbox.sequence ASC
             LIMIT ?6",
        )
        .map_err(read_error)?;
    let rows = statement
        .query_map(
            params![
                recipient_agent_id,
                conversation_id,
                previous_position,
                assistant_position,
                sender_filter,
                i64::try_from(maximum).unwrap_or(i64::MAX),
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, i64>(7)?,
                ))
            },
        )
        .map_err(read_error)?;
    rows.map(|row| {
        let (
            message_id,
            sender_agent_id,
            sender_task_name,
            sender_task_path,
            kind,
            content,
            sequence,
            created_at,
        ) = row.map_err(read_error)?;
        Ok(MailboxCandidate {
            message_id,
            sender_agent_id,
            sender_task_name,
            sender_task_path,
            kind: AgentMailboxKind::parse(&kind)?,
            content,
            mailbox_sequence: positive_u64(sequence, "Mailbox sequence")?,
            created_at,
        })
    })
    .collect()
}

fn list_wait_candidates(
    connection: &Connection,
    caller_agent_id: &str,
    target_agent_id: &str,
    after_sequence: u64,
    maximum: usize,
) -> Result<Vec<MailboxCandidate>, AgentGraphError> {
    if maximum == 0 {
        return Ok(Vec::new());
    }
    let mut statement = connection
        .prepare(
            "SELECT mailbox.message_id, mailbox.sender_agent_id,
                    sender.task_name, sender.task_path, mailbox.kind,
                    mailbox.content, mailbox.sequence, mailbox.created_at
             FROM agent_mailbox_messages AS mailbox
             INNER JOIN agent_nodes AS sender ON sender.agent_id = mailbox.sender_agent_id
             LEFT JOIN agent_model_batch_receipt_items AS consumed
                ON consumed.message_id = mailbox.message_id
             WHERE mailbox.recipient_agent_id = ?1
               AND mailbox.sender_agent_id = ?2
               AND mailbox.delivery_status = 'acknowledged'
               AND mailbox.sequence > ?3
               AND consumed.message_id IS NULL
             ORDER BY mailbox.sequence ASC
             LIMIT ?4",
        )
        .map_err(read_error)?;
    let rows = statement
        .query_map(
            params![
                caller_agent_id,
                target_agent_id,
                u64_to_sql(after_sequence)?,
                i64::try_from(maximum).unwrap_or(i64::MAX),
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, i64>(7)?,
                ))
            },
        )
        .map_err(read_error)?;
    rows.map(|row| {
        let (
            message_id,
            sender_agent_id,
            sender_task_name,
            sender_task_path,
            kind,
            content,
            sequence,
            created_at,
        ) = row.map_err(read_error)?;
        Ok(MailboxCandidate {
            message_id,
            sender_agent_id,
            sender_task_name,
            sender_task_path,
            kind: AgentMailboxKind::parse(&kind)?,
            content,
            mailbox_sequence: positive_u64(sequence, "Mailbox sequence")?,
            created_at,
        })
    })
    .collect()
}

fn query_acknowledged_candidate(
    connection: &Connection,
    message_id: &str,
) -> Result<Option<MailboxCandidate>, AgentGraphError> {
    connection
        .query_row(
            "SELECT mailbox.message_id, mailbox.sender_agent_id,
                    sender.task_name, sender.task_path, mailbox.kind, mailbox.content,
                    mailbox.sequence, mailbox.created_at
             FROM agent_mailbox_messages AS mailbox
             INNER JOIN agent_nodes AS sender ON sender.agent_id = mailbox.sender_agent_id
             WHERE mailbox.message_id = ?1 AND mailbox.delivery_status = 'acknowledged'",
            [message_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, i64>(7)?,
                ))
            },
        )
        .optional()
        .map_err(read_error)?
        .map(
            |(
                message_id,
                sender_agent_id,
                sender_task_name,
                sender_task_path,
                kind,
                content,
                sequence,
                created_at,
            )| {
                Ok(MailboxCandidate {
                    message_id,
                    sender_agent_id,
                    sender_task_name,
                    sender_task_path,
                    kind: AgentMailboxKind::parse(&kind)?,
                    content,
                    mailbox_sequence: positive_u64(sequence, "Mailbox sequence")?,
                    created_at,
                })
            },
        )
        .transpose()
}

fn insert_receipt_item(
    connection: &Connection,
    receipt_id: &str,
    candidate: &MailboxCandidate,
    ordinal: u32,
    path: AgentDeliveryPath,
    trace_sequence: Option<u64>,
    bound_at: i64,
) -> Result<(), AgentGraphError> {
    connection
        .execute(
            "INSERT INTO agent_model_batch_receipt_items (
                 receipt_id, message_id, ordinal, mailbox_sequence,
                 delivery_path, trace_sequence, bound_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                receipt_id,
                &candidate.message_id,
                i64::from(ordinal),
                u64_to_sql(candidate.mailbox_sequence)?,
                path.as_str(),
                trace_sequence.map(u64_to_sql).transpose()?,
                bound_at,
            ],
        )
        .map_err(write_error)?;
    Ok(())
}

fn query_item_by_message(
    connection: &Connection,
    message_id: &str,
) -> Result<Option<AgentModelBatchReceiptItemRecord>, AgentGraphError> {
    connection
        .query_row(
            "SELECT receipt_id, message_id, ordinal, mailbox_sequence,
                    delivery_path, trace_sequence, bound_at
             FROM agent_model_batch_receipt_items WHERE message_id = ?1",
            [message_id],
            read_item,
        )
        .optional()
        .map_err(read_error)?
        .map(decode_item)
        .transpose()
}

fn next_receipt_ordinal(connection: &Connection, receipt_id: &str) -> Result<u32, AgentGraphError> {
    let value = connection
        .query_row(
            "SELECT COALESCE(MAX(ordinal), -1) + 1
             FROM agent_model_batch_receipt_items WHERE receipt_id = ?1",
            [receipt_id],
            |row| row.get::<_, i64>(0),
        )
        .map_err(read_error)?;
    u32::try_from(value).map_err(|_| corrupt("receipt ordinal is invalid"))
}

fn touch_receipt(
    connection: &Connection,
    receipt_id: &str,
    updated_at: i64,
) -> Result<(), AgentGraphError> {
    connection
        .execute(
            "UPDATE agent_model_batch_receipts
             SET updated_at = MAX(updated_at, ?1) WHERE receipt_id = ?2",
            params![updated_at, receipt_id],
        )
        .map_err(write_error)?;
    Ok(())
}

fn query_receipt_by_batch(
    connection: &Connection,
    run_id: &str,
    batch_index: u64,
) -> Result<Option<AgentModelBatchReceiptRecord>, AgentGraphError> {
    connection
        .query_row(
            "SELECT receipt_id, agent_id, conversation_id, run_id, assistant_message_id,
                    model_batch_index, sampling_bound_at, created_at, updated_at
             FROM agent_model_batch_receipts
             WHERE run_id = ?1 AND model_batch_index = ?2",
            params![run_id, u64_to_sql(batch_index)?],
            read_receipt,
        )
        .optional()
        .map_err(read_error)?
        .map(decode_receipt)
        .transpose()
}

fn query_receipt(
    connection: &Connection,
    receipt_id: &str,
) -> Result<Option<AgentModelBatchReceiptRecord>, AgentGraphError> {
    connection
        .query_row(
            "SELECT receipt_id, agent_id, conversation_id, run_id, assistant_message_id,
                    model_batch_index, sampling_bound_at, created_at, updated_at
             FROM agent_model_batch_receipts WHERE receipt_id = ?1",
            [receipt_id],
            read_receipt,
        )
        .optional()
        .map_err(read_error)?
        .map(decode_receipt)
        .transpose()
}

fn load_delivery(
    connection: &Connection,
    receipt_id: &str,
    path: Option<AgentDeliveryPath>,
) -> Result<AgentModelBatchDeliveryRecord, AgentGraphError> {
    let receipt = query_receipt(connection, receipt_id)?
        .ok_or_else(|| corrupt("model batch receipt disappeared"))?;
    let mut statement = connection
        .prepare(
            "SELECT mailbox.message_id, mailbox.sender_agent_id,
                    sender.task_name, sender.task_path, mailbox.kind,
                    mailbox.content,
                    mailbox.sequence, item.delivery_path, item.trace_sequence,
                    mailbox.created_at
             FROM agent_model_batch_receipt_items AS item
             INNER JOIN agent_mailbox_messages AS mailbox
                ON mailbox.message_id = item.message_id
             INNER JOIN agent_nodes AS sender ON sender.agent_id = mailbox.sender_agent_id
             WHERE item.receipt_id = ?1
               AND (?2 IS NULL OR item.delivery_path = ?2)
             ORDER BY item.ordinal ASC",
        )
        .map_err(read_error)?;
    let rows = statement
        .query_map(
            params![receipt_id, path.map(AgentDeliveryPath::as_str)],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, Option<i64>>(8)?,
                    row.get::<_, i64>(9)?,
                ))
            },
        )
        .map_err(read_error)?;
    let messages = rows
        .map(|row| {
            let (
                message_id,
                sender_agent_id,
                sender_task_name,
                sender_task_path,
                kind,
                content,
                mailbox_sequence,
                path,
                trace,
                at,
            ) = row.map_err(read_error)?;
            let kind = AgentMailboxKind::parse(&kind)?;
            let delivery_path = AgentDeliveryPath::parse(&path).map_err(corrupt)?;
            let content = if delivery_path == AgentDeliveryPath::WaitAgent {
                crate::conversation_trace::project_agent_mailbox_model_envelope(
                    &sender_agent_id,
                    &sender_task_name,
                    &sender_task_path,
                    kind,
                    content.trim(),
                )
                .map_err(corrupt)?
                .0
            } else {
                content
            };
            Ok(AgentDeliveredMailboxMessage {
                message_id,
                sender_agent_id,
                sender_task_name,
                sender_task_path,
                kind,
                content,
                mailbox_sequence: positive_u64(mailbox_sequence, "Mailbox sequence")?,
                delivery_path,
                trace_sequence: trace
                    .map(|value| nonnegative_u64(value, "trace sequence"))
                    .transpose()?,
                created_at: at,
            })
        })
        .collect::<Result<Vec<_>, AgentGraphError>>()?;
    Ok(AgentModelBatchDeliveryRecord { receipt, messages })
}

fn wait_delivered(
    candidate: &MailboxCandidate,
) -> Result<AgentDeliveredMailboxMessage, AgentGraphError> {
    let mut projected = delivered(candidate, AgentDeliveryPath::WaitAgent, None);
    projected.content = crate::conversation_trace::project_agent_mailbox_model_envelope(
        &candidate.sender_agent_id,
        &candidate.sender_task_name,
        &candidate.sender_task_path,
        candidate.kind,
        candidate.content.trim(),
    )
    .map_err(corrupt)?
    .0;
    Ok(projected)
}

fn delivered(
    candidate: &MailboxCandidate,
    path: AgentDeliveryPath,
    trace_sequence: Option<u64>,
) -> AgentDeliveredMailboxMessage {
    AgentDeliveredMailboxMessage {
        message_id: candidate.message_id.clone(),
        sender_agent_id: candidate.sender_agent_id.clone(),
        sender_task_name: candidate.sender_task_name.clone(),
        sender_task_path: candidate.sender_task_path.clone(),
        kind: candidate.kind,
        content: candidate.content.clone(),
        mailbox_sequence: candidate.mailbox_sequence,
        delivery_path: path,
        trace_sequence,
        created_at: candidate.created_at,
    }
}

#[derive(Debug, Clone)]
struct WakeVersion {
    sequence: u64,
    status_revision: u64,
    status: crate::AgentWakeStatus,
}

#[derive(Debug)]
struct WaitTargetPollState {
    latest_wake: Option<WakeVersion>,
    target_status_version: u64,
    display_status: crate::AgentDisplayStatus,
    status_advanced: bool,
}

fn latest_wake_for_target(
    connection: &Connection,
    target_agent_id: &str,
) -> Result<Option<WakeVersion>, AgentGraphError> {
    connection
        .query_row(
            "SELECT sequence, status_revision, status
             FROM agent_wake_requests
             WHERE agent_id = ?1
             ORDER BY sequence DESC LIMIT 1",
            [target_agent_id],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()
        .map_err(read_error)?
        .map(|(sequence, revision, status)| {
            Ok(WakeVersion {
                sequence: positive_u64(sequence, "Wake sequence")?,
                status_revision: positive_u64(revision, "Wake status revision")?,
                status: crate::AgentWakeStatus::parse(&status)?,
            })
        })
        .transpose()
}

/// A durable monotonic semantic version which cannot be hidden by choosing only the newest Wake.
/// Agent revision advances on lifecycle changes; each Wake contributes its status revision, so an
/// older active Wake completing after a newer coalesced Wake still advances this value.
fn target_status_version(
    connection: &Connection,
    target_agent_id: &str,
) -> Result<u64, AgentGraphError> {
    let value = connection
        .query_row(
            "SELECT agent.revision + COALESCE((
                 SELECT SUM(wake.status_revision)
                 FROM agent_wake_requests AS wake
                 WHERE wake.agent_id = agent.agent_id
             ), 0)
             FROM agent_nodes AS agent WHERE agent.agent_id = ?1",
            [target_agent_id],
            |row| row.get::<_, i64>(0),
        )
        .map_err(read_error)?;
    positive_u64(value, "target status version")
}

fn query_cursor(
    connection: &Connection,
    caller: &str,
    run_id: &str,
    target: &str,
) -> Result<Option<AgentCollaborationCursorRecord>, AgentGraphError> {
    connection
        .query_row(
            "SELECT caller_agent_id, run_id, target_agent_id, last_target_status_version,
                    last_message_sequence, last_wake_sequence, last_wake_status_revision, updated_at
             FROM agent_collaboration_cursors
             WHERE caller_agent_id = ?1 AND run_id = ?2 AND target_agent_id = ?3",
            params![caller, run_id, target],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, i64>(7)?,
                ))
            },
        )
        .optional()
        .map_err(read_error)?
        .map(
            |(caller, run_id, target, status_version, message, wake, revision, updated_at)| {
                Ok(AgentCollaborationCursorRecord {
                    caller_agent_id: caller,
                    run_id,
                    target_agent_id: target,
                    last_target_status_version: nonnegative_u64(
                        status_version,
                        "cursor target status version",
                    )?,
                    last_message_sequence: nonnegative_u64(message, "cursor message sequence")?,
                    last_wake_sequence: nonnegative_u64(wake, "cursor Wake sequence")?,
                    last_wake_status_revision: nonnegative_u64(revision, "cursor Wake revision")?,
                    updated_at,
                })
            },
        )
        .transpose()
}

#[allow(clippy::too_many_arguments)]
fn upsert_cursor(
    connection: &Connection,
    caller: &str,
    run_id: &str,
    target: &str,
    target_status_version: u64,
    message_sequence: u64,
    wake_sequence: u64,
    wake_revision: u64,
    updated_at: i64,
) -> Result<(), AgentGraphError> {
    connection
        .execute(
            "INSERT INTO agent_collaboration_cursors (
                 caller_agent_id, run_id, target_agent_id, last_target_status_version,
                 last_message_sequence,
                 last_wake_sequence, last_wake_status_revision, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(caller_agent_id, run_id, target_agent_id) DO UPDATE SET
                 last_target_status_version = MAX(
                   last_target_status_version, excluded.last_target_status_version
                 ),
                 last_message_sequence = MAX(last_message_sequence, excluded.last_message_sequence),
                 last_wake_sequence = MAX(last_wake_sequence, excluded.last_wake_sequence),
                 last_wake_status_revision = CASE
                   WHEN excluded.last_wake_sequence > last_wake_sequence
                   THEN excluded.last_wake_status_revision
                   WHEN excluded.last_wake_sequence = last_wake_sequence
                   THEN MAX(last_wake_status_revision, excluded.last_wake_status_revision)
                   ELSE last_wake_status_revision
                 END,
                 updated_at = MAX(updated_at, excluded.updated_at)",
            params![
                caller,
                run_id,
                target,
                u64_to_sql(target_status_version)?,
                u64_to_sql(message_sequence)?,
                u64_to_sql(wake_sequence)?,
                u64_to_sql(wake_revision)?,
                updated_at,
            ],
        )
        .map_err(write_error)?;
    Ok(())
}

fn is_strict_descendant(
    connection: &Connection,
    caller: &str,
    target: &str,
) -> Result<bool, AgentGraphError> {
    connection
        .query_row(
            "WITH RECURSIVE ancestors(agent_id, parent_agent_id) AS (
                 SELECT agent_id, parent_agent_id FROM agent_nodes WHERE agent_id = ?2
                 UNION ALL
                 SELECT parent.agent_id, parent.parent_agent_id
                 FROM agent_nodes AS parent
                 JOIN ancestors AS child ON parent.agent_id = child.parent_agent_id
             )
             SELECT EXISTS(
                 SELECT 1 FROM ancestors WHERE agent_id = ?1 AND agent_id != ?2
             )",
            params![caller, target],
            |row| row.get(0),
        )
        .map_err(read_error)
}

fn trace_created_at(
    connection: &Connection,
    assistant_message_id: &str,
) -> Result<i64, AgentGraphError> {
    connection
        .query_row(
            "SELECT created_at FROM conversation_turn_traces WHERE assistant_message_id = ?1",
            [assistant_message_id],
            |row| row.get(0),
        )
        .map_err(read_error)
}

fn read_receipt(row: &rusqlite::Row<'_>) -> rusqlite::Result<ReceiptRow> {
    Ok(ReceiptRow {
        receipt_id: row.get(0)?,
        agent_id: row.get(1)?,
        conversation_id: row.get(2)?,
        run_id: row.get(3)?,
        assistant_message_id: row.get(4)?,
        model_batch_index: row.get(5)?,
        sampling_bound_at: row.get(6)?,
        created_at: row.get(7)?,
        updated_at: row.get(8)?,
    })
}

struct ReceiptRow {
    receipt_id: String,
    agent_id: String,
    conversation_id: String,
    run_id: String,
    assistant_message_id: String,
    model_batch_index: i64,
    sampling_bound_at: Option<i64>,
    created_at: i64,
    updated_at: i64,
}

fn decode_receipt(row: ReceiptRow) -> Result<AgentModelBatchReceiptRecord, AgentGraphError> {
    Ok(AgentModelBatchReceiptRecord {
        receipt_id: row.receipt_id,
        agent_id: row.agent_id,
        conversation_id: row.conversation_id,
        run_id: row.run_id,
        assistant_message_id: row.assistant_message_id,
        model_batch_index: positive_u64(row.model_batch_index, "model batch index")?,
        sampling_bound_at: row.sampling_bound_at,
        created_at: row.created_at,
        updated_at: row.updated_at,
    })
}

fn read_item(row: &rusqlite::Row<'_>) -> rusqlite::Result<ItemRow> {
    Ok(ItemRow {
        receipt_id: row.get(0)?,
        message_id: row.get(1)?,
        ordinal: row.get(2)?,
        mailbox_sequence: row.get(3)?,
        delivery_path: row.get(4)?,
        trace_sequence: row.get(5)?,
        bound_at: row.get(6)?,
    })
}

struct ItemRow {
    receipt_id: String,
    message_id: String,
    ordinal: i64,
    mailbox_sequence: i64,
    delivery_path: String,
    trace_sequence: Option<i64>,
    bound_at: i64,
}

fn decode_item(row: ItemRow) -> Result<AgentModelBatchReceiptItemRecord, AgentGraphError> {
    Ok(AgentModelBatchReceiptItemRecord {
        receipt_id: row.receipt_id,
        message_id: row.message_id,
        ordinal: u32::try_from(row.ordinal).map_err(|_| corrupt("receipt ordinal is invalid"))?,
        mailbox_sequence: positive_u64(row.mailbox_sequence, "Mailbox sequence")?,
        delivery_path: AgentDeliveryPath::parse(&row.delivery_path).map_err(corrupt)?,
        trace_sequence: row
            .trace_sequence
            .map(|value| nonnegative_u64(value, "trace sequence"))
            .transpose()?,
        bound_at: row.bound_at,
    })
}

fn stable_id(namespace: &str, parts: &[&str]) -> String {
    let mut digest = Sha256::new();
    digest.update(namespace.as_bytes());
    for part in parts {
        digest.update((part.len() as u64).to_be_bytes());
        digest.update(part.as_bytes());
    }
    format!("{namespace}-{:x}", digest.finalize())
}

fn validate_turn_identity(
    conversation_id: &str,
    run_id: &str,
    assistant_message_id: &str,
    model_batch_index: u64,
) -> Result<(), AgentGraphError> {
    validate_identity(conversation_id, "conversation_id")?;
    validate_identity(run_id, "run_id")?;
    validate_identity(assistant_message_id, "assistant_message_id")?;
    if model_batch_index == 0 {
        return Err(invalid("model_batch_index", "must be greater than zero"));
    }
    Ok(())
}

fn validate_identity(value: &str, field: &'static str) -> Result<(), AgentGraphError> {
    if value.trim().is_empty() || value.trim() != value || value.len() > 256 {
        return Err(invalid(
            field,
            "must be non-empty, trimmed, and at most 256 bytes",
        ));
    }
    Ok(())
}

fn validate_time(value: i64) -> Result<(), AgentGraphError> {
    if value < 0 {
        Err(invalid("timestamp", "cannot be negative"))
    } else {
        Ok(())
    }
}

fn u64_to_sql(value: u64) -> Result<i64, AgentGraphError> {
    i64::try_from(value).map_err(|_| corrupt("integer exceeds SQLite range"))
}

fn positive_u64(value: i64, field: &str) -> Result<u64, AgentGraphError> {
    if value <= 0 {
        return Err(corrupt(format!("{field} must be positive")));
    }
    Ok(value as u64)
}

fn nonnegative_u64(value: i64, field: &str) -> Result<u64, AgentGraphError> {
    if value < 0 {
        return Err(corrupt(format!("{field} cannot be negative")));
    }
    Ok(value as u64)
}

fn invalid(field: &'static str, reason: impl Into<String>) -> AgentGraphError {
    AgentGraphError::InvalidInput {
        field,
        reason: reason.into(),
    }
}

fn conflict(reason: impl Into<String>) -> AgentGraphError {
    AgentGraphError::Conflict(reason.into())
}

fn corrupt(reason: impl Into<String>) -> AgentGraphError {
    AgentGraphError::CorruptRecord(reason.into())
}

fn read_error(error: rusqlite::Error) -> AgentGraphError {
    AgentGraphError::StorageUnavailable(format!("failed to read Agent delivery state: {error}"))
}

fn write_error(error: rusqlite::Error) -> AgentGraphError {
    AgentGraphError::StorageUnavailable(format!("failed to write Agent delivery state: {error}"))
}

#[cfg(test)]
mod tests;
