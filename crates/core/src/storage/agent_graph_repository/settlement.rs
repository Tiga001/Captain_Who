use super::common::{
    conflict, corrupt, immediate, invalid, read_error, stable_fact_id, validate_id, validate_time,
    validate_trimmed, write_error, MAX_RESULT_ARTIFACTS, MAX_RESULT_SUMMARY_BYTES,
    MAX_TERMINAL_ERROR_BYTES,
};
use super::mailbox::{
    enqueue_message_in_transaction, message_matches_enqueue, validate_message_input,
};
use super::message_records::query_message;
use super::node_records::query_node;
use super::wake_commands::enqueue_wake_in_transaction;
use super::wake_records::{query_wake, query_wake_by_request};
use crate::storage::{
    chat_repository, conversation_model_context_repository, conversation_trace_repository,
};
use crate::{
    AgentGraphError, AgentMailboxKind, AgentResultArtifactKind, AgentResultArtifactReference,
    AgentTurnResultEnvelope, AgentTurnResultSettlement, AgentWakeRequestRecord, AgentWakeStatus,
    EnqueueAgentMessageInput, EnqueueAgentWakeInput, FinishAgentTurnResultInput,
    FinishAgentWakeWithResultInput, AGENT_RESULT_ENVELOPE_SCHEMA_VERSION,
};
use rusqlite::{params, Connection};

pub fn finish_agent_wake_with_result(
    connection: &mut Connection,
    input: &FinishAgentWakeWithResultInput,
    completed_at: i64,
) -> Result<AgentWakeRequestRecord, AgentGraphError> {
    // Round-1 repository compatibility primitive. Production Dispatcher/Host paths must call
    // `finish_agent_turn_with_result`, which constructs and freezes the typed result envelope.
    validate_time(completed_at)?;
    validate_id("claim_token", &input.claim_token)?;
    if !matches!(
        input.terminal_status,
        AgentWakeStatus::Completed
            | AgentWakeStatus::Failed
            | AgentWakeStatus::Interrupted
            | AgentWakeStatus::OutcomeUnknown
    ) {
        return Err(invalid(
            "terminal_status",
            "result settlement requires completed, failed, interrupted, or outcome_unknown",
        ));
    }
    if input.terminal_status == AgentWakeStatus::Completed && input.terminal_error.is_some() {
        return Err(invalid(
            "terminal_error",
            "completed Wake cannot carry a terminal error",
        ));
    }
    if !input
        .expected_status
        .can_transition_to(input.terminal_status)
    {
        return Err(AgentGraphError::IllegalTransition {
            current: input.expected_status,
            requested: input.terminal_status,
        });
    }
    if input.result_message.kind != AgentMailboxKind::Result {
        return Err(invalid("result_message.kind", "must be result"));
    }
    validate_message_input(&input.result_message, completed_at)?;
    let transaction = immediate(connection)?;
    let wake = query_wake(&transaction, &input.wake_id)?
        .ok_or_else(|| AgentGraphError::WakeNotFound(input.wake_id.clone()))?;
    if wake.status.is_terminal() {
        let existing_result = query_message(&transaction, &input.result_message.message_id)?;
        if wake.status == input.terminal_status
            && wake.result_message_id.as_deref() == Some(input.result_message.message_id.as_str())
            && wake.claim_token.as_deref() == Some(input.claim_token.as_str())
            && wake.terminal_error == input.terminal_error
            && existing_result
                .as_ref()
                .is_some_and(|message| message_matches_enqueue(message, &input.result_message))
        {
            transaction.commit().map_err(write_error)?;
            return Ok(wake);
        }
        return Err(conflict("Wake is already terminal with different facts"));
    }
    if wake.status != input.expected_status
        || wake.claim_token.as_deref() != Some(input.claim_token.as_str())
    {
        return Err(conflict(
            "Wake status or claim token does not match settlement",
        ));
    }
    if wake
        .lease_expires_at
        .is_none_or(|deadline| completed_at >= deadline)
    {
        return Err(conflict("Wake lease has expired"));
    }
    let child = query_node(&transaction, &wake.agent_id)?
        .ok_or_else(|| corrupt("Wake child Agent is missing"))?;
    let direct_parent_id = child
        .parent_agent_id
        .as_deref()
        .ok_or_else(|| conflict("root Agent Wake cannot emit a child result"))?;
    if input.result_message.root_agent_id != wake.root_agent_id
        || input.result_message.sender_agent_id != wake.agent_id
        || input.result_message.recipient_agent_id != direct_parent_id
    {
        return Err(conflict("Wake result participants do not match the Wake"));
    }
    let result = enqueue_message_in_transaction(&transaction, &input.result_message, completed_at)?;
    let result_id = result.record().message_id.clone();
    transaction
        .execute(
            "UPDATE agent_wake_requests
             SET status = ?1, status_revision = status_revision + 1,
                 result_message_id = ?2, terminal_error = ?3, completed_at = ?4
             WHERE wake_id = ?5 AND status = ?6 AND claim_token = ?7",
            params![
                input.terminal_status.as_str(),
                result_id,
                &input.terminal_error,
                completed_at,
                &input.wake_id,
                input.expected_status.as_str(),
                &input.claim_token,
            ],
        )
        .map_err(write_error)?;
    let settled = query_wake(&transaction, &input.wake_id)?
        .ok_or_else(|| corrupt("settled Wake could not be read back"))?;
    transaction.commit().map_err(write_error)?;
    Ok(settled)
}

/// Atomically records a delegated Turn's durable terminal fact and its direct-parent result
/// Outbox. Non-root parents receive a deferred Wake in the same transaction; root parents never
/// start a background model Turn merely because a child reported a result.
pub fn finish_agent_turn_with_result(
    connection: &mut Connection,
    input: &FinishAgentTurnResultInput,
    completed_at: i64,
) -> Result<AgentTurnResultSettlement, AgentGraphError> {
    validate_time(completed_at)?;
    validate_id("wake_id", &input.wake_id)?;
    validate_id("claim_token", &input.claim_token)?;
    validate_trimmed("summary", &input.summary, MAX_RESULT_SUMMARY_BYTES)?;
    if let Some(error) = input.terminal_error.as_deref() {
        validate_trimmed("terminal_error", error, MAX_TERMINAL_ERROR_BYTES)?;
    }
    if input.run_id.is_some() != input.assistant_message_id.is_some() {
        return Err(invalid(
            "run_id",
            "run_id and assistant_message_id must both be present or both be absent",
        ));
    }
    if let Some(run_id) = input.run_id.as_deref() {
        validate_trimmed("run_id", run_id, 2_048)?;
    }
    if let Some(turn_id) = input.assistant_message_id.as_deref() {
        validate_trimmed("assistant_message_id", turn_id, 2_048)?;
    }
    if !matches!(
        input.terminal_status,
        AgentWakeStatus::Completed
            | AgentWakeStatus::Failed
            | AgentWakeStatus::Interrupted
            | AgentWakeStatus::OutcomeUnknown
    ) {
        return Err(invalid(
            "terminal_status",
            "result settlement requires completed, failed, interrupted, or outcome_unknown",
        ));
    }
    if input.terminal_status == AgentWakeStatus::Completed && input.terminal_error.is_some() {
        return Err(invalid(
            "terminal_error",
            "a completed child Turn cannot carry a terminal error",
        ));
    }

    let transaction = immediate(connection)?;
    let wake = query_wake(&transaction, &input.wake_id)?
        .ok_or_else(|| AgentGraphError::WakeNotFound(input.wake_id.clone()))?;
    let child = query_node(&transaction, &wake.agent_id)?
        .ok_or_else(|| corrupt("settling Wake child Agent is missing"))?;
    let parent_id = child
        .parent_agent_id
        .as_deref()
        .ok_or_else(|| conflict("a root Agent Wake cannot emit a delegated child result"))?;
    let parent = query_node(&transaction, parent_id)?
        .ok_or_else(|| corrupt("settling Wake direct parent Agent is missing"))?;
    if wake.root_agent_id != child.root_agent_id || parent.root_agent_id != child.root_agent_id {
        return Err(corrupt("settling Wake does not belong to one Agent tree"));
    }
    if wake.run_id != input.run_id || wake.assistant_message_id != input.assistant_message_id {
        return Err(conflict(
            "result execution identity does not match the durable Wake",
        ));
    }

    // Conservative crash recovery owns the exact child trace that generic startup reconciliation
    // deliberately skipped. Terminalize that trace and the result Outbox in this one transaction,
    // otherwise an `outcome_unknown` Wake would leave the Agent Conversation permanently busy.
    if matches!(
        input.terminal_status,
        AgentWakeStatus::OutcomeUnknown | AgentWakeStatus::Failed
    ) {
        if let (Some(run_id), Some(assistant_message_id), Some(reason)) = (
            input.run_id.as_deref(),
            input.assistant_message_id.as_deref(),
            input.terminal_error.as_deref(),
        ) {
            terminalize_recovered_agent_trace_in_transaction(
                &transaction,
                &child.conversation_id,
                run_id,
                assistant_message_id,
                reason,
                completed_at,
            )?;
        }
    }

    if wake.status.is_terminal() {
        if wake.status != input.terminal_status
            || wake.claim_token.as_deref() != Some(input.claim_token.as_str())
            || wake.terminal_error != input.terminal_error
        {
            return Err(conflict(
                "Wake already settled with different terminal facts",
            ));
        }
        let result_id = wake
            .result_message_id
            .as_deref()
            .ok_or_else(|| corrupt("terminal delegated Wake is missing its result Outbox"))?;
        let result_message = query_message(&transaction, result_id)?
            .ok_or_else(|| corrupt("terminal delegated Wake result Outbox is missing"))?;
        let envelope: AgentTurnResultEnvelope = serde_json::from_str(&result_message.content)
            .map_err(|_| corrupt("terminal delegated Wake result envelope is invalid"))?;
        if envelope.schema_version != AGENT_RESULT_ENVELOPE_SCHEMA_VERSION
            || envelope.child_agent_id != child.agent_id
            || envelope.task_name != child.task_name
            || envelope.task_path != child.task_path
            || envelope.wake_id != wake.wake_id
            || envelope.turn_id != input.assistant_message_id
            || envelope.run_id != input.run_id
            || envelope.status != input.terminal_status
            || envelope.summary != input.summary
            || envelope.terminal_error != input.terminal_error
            || result_message.kind != AgentMailboxKind::Result
            || result_message.sender_agent_id != child.agent_id
            || result_message.recipient_agent_id != parent.agent_id
            || result_message.root_agent_id != wake.root_agent_id
            || result_message.message_id != stable_fact_id("mailbox-result", &[&wake.wake_id])
            || result_message.request_id != format!("result:{}", wake.wake_id)
            || result_message.projection_message_id
                != stable_fact_id("message-result", &[&wake.wake_id])
        {
            return Err(conflict("terminal delegated Wake result payload differs"));
        }
        validate_result_artifact_refs(&envelope.artifact_refs)?;
        let parent_wake = query_wake_by_request(
            &transaction,
            &child.agent_id,
            &format!("result-wake:{}", wake.wake_id),
        )?;
        if parent.parent_agent_id.is_some() != parent_wake.is_some() {
            return Err(corrupt(
                "terminal delegated result has an invalid direct-parent deferred Wake",
            ));
        }
        transaction.commit().map_err(write_error)?;
        return Ok(AgentTurnResultSettlement {
            wake,
            result_message,
            parent_wake,
            envelope,
        });
    }

    let artifact_refs = list_result_artifacts_for_run(
        &transaction,
        &child.conversation_id,
        input.run_id.as_deref(),
    )?;
    let envelope = AgentTurnResultEnvelope {
        schema_version: AGENT_RESULT_ENVELOPE_SCHEMA_VERSION,
        child_agent_id: child.agent_id.clone(),
        task_name: child.task_name.clone(),
        task_path: child.task_path.clone(),
        wake_id: wake.wake_id.clone(),
        turn_id: input.assistant_message_id.clone(),
        run_id: input.run_id.clone(),
        status: input.terminal_status,
        summary: input.summary.clone(),
        artifact_refs,
        terminal_error: input.terminal_error.clone(),
    };
    let content = serde_json::to_string(&envelope)
        .map_err(|_| corrupt("Agent result envelope could not be serialized"))?;
    let result_input = EnqueueAgentMessageInput {
        message_id: stable_fact_id("mailbox-result", &[&wake.wake_id]),
        root_agent_id: wake.root_agent_id.clone(),
        sender_agent_id: child.agent_id.clone(),
        recipient_agent_id: parent.agent_id.clone(),
        request_id: format!("result:{}", wake.wake_id),
        kind: AgentMailboxKind::Result,
        content,
        projection_message_id: stable_fact_id("message-result", &[&wake.wake_id]),
    };

    if wake.status != input.expected_status
        || wake.claim_token.as_deref() != Some(input.claim_token.as_str())
    {
        return Err(conflict(
            "Wake status or claim token does not match settlement",
        ));
    }
    if wake
        .lease_expires_at
        .is_none_or(|deadline| completed_at >= deadline)
    {
        return Err(conflict("Wake lease has expired"));
    }
    if !wake.status.can_transition_to(input.terminal_status) {
        return Err(AgentGraphError::IllegalTransition {
            current: wake.status,
            requested: input.terminal_status,
        });
    }

    let result_message = enqueue_message_in_transaction(&transaction, &result_input, completed_at)?
        .record()
        .clone();
    transaction
        .execute(
            "UPDATE agent_wake_requests
             SET status = ?1, status_revision = status_revision + 1,
                 result_message_id = ?2, terminal_error = ?3, completed_at = ?4
             WHERE wake_id = ?5 AND status = ?6 AND claim_token = ?7",
            params![
                input.terminal_status.as_str(),
                &result_message.message_id,
                &input.terminal_error,
                completed_at,
                &wake.wake_id,
                input.expected_status.as_str(),
                &input.claim_token,
            ],
        )
        .map_err(write_error)?;
    let settled_wake = query_wake(&transaction, &wake.wake_id)?
        .ok_or_else(|| corrupt("settled delegated Wake disappeared"))?;

    let parent_wake = if parent.parent_agent_id.is_some() {
        Some(
            enqueue_wake_in_transaction(
                &transaction,
                &EnqueueAgentWakeInput {
                    wake_id: stable_fact_id("wake-result", &[&wake.wake_id]),
                    root_agent_id: wake.root_agent_id,
                    agent_id: parent.agent_id,
                    requester_agent_id: child.agent_id,
                    request_id: format!("result-wake:{}", wake.wake_id),
                    source_agent_message_id: Some(result_message.message_id.clone()),
                },
                completed_at,
            )?
            .record()
            .clone(),
        )
    } else {
        None
    };
    transaction.commit().map_err(write_error)?;
    Ok(AgentTurnResultSettlement {
        wake: settled_wake,
        result_message,
        parent_wake,
        envelope,
    })
}

pub(super) fn terminalize_recovered_agent_trace_in_transaction(
    connection: &Connection,
    conversation_id: &str,
    run_id: &str,
    assistant_message_id: &str,
    reason: &str,
    completed_at: i64,
) -> Result<(), AgentGraphError> {
    let Some(trace) =
        conversation_trace_repository::get_trace_for_message(connection, assistant_message_id)
            .map_err(read_error)?
    else {
        // A failed pre-Runtime preparation intentionally has no surviving trace.
        return Ok(());
    };
    if trace.run_id != run_id || trace.conversation_id != conversation_id {
        return Err(conflict(
            "recovered trace identity does not match Wake identity",
        ));
    }
    if trace.terminal_status != crate::ConversationTurnTraceTerminalStatus::InProgress {
        return Ok(());
    }
    let model_context_items = conversation_model_context_repository::get_log_for_message(
        connection,
        assistant_message_id,
    )
    .map_err(read_error)?
    .map(|log| log.items)
    .unwrap_or_default();
    let next_sequence = trace
        .items
        .last()
        .map(crate::ConversationTurnTraceItem::sequence)
        .unwrap_or(0)
        .saturating_add(1);
    let terminal = crate::terminal_conversation_trace_from_snapshot(
        crate::ConversationTraceSnapshot {
            items: trace.items,
            model_context_items,
            next_sequence,
            truncated: trace.truncated,
        },
        run_id,
        conversation_id,
        assistant_message_id,
        crate::ConversationTurnTraceTerminalStatus::Failed,
        reason,
    )
    .map_err(|error| {
        corrupt(format!(
            "could not terminalize recovered Agent trace: {error}"
        ))
    })?;
    let created_at = connection
        .query_row(
            "SELECT created_at FROM conversation_turn_traces WHERE assistant_message_id = ?1",
            [assistant_message_id],
            |row| row.get::<_, i64>(0),
        )
        .map_err(read_error)?;
    conversation_trace_repository::commit_trace_in_connection(
        connection,
        &terminal.trace,
        created_at,
        completed_at.max(created_at),
    )
    .map_err(write_error)?;
    conversation_model_context_repository::commit_items_in_connection(
        connection,
        conversation_id,
        assistant_message_id,
        &terminal.model_context_items,
    )
    .map_err(write_error)?;
    chat_repository::reconcile_message_run_terminal_state(
        connection,
        conversation_id,
        assistant_message_id,
        run_id,
        "error",
        "failed",
        completed_at.max(created_at),
    )
    .map_err(write_error)?;
    connection
        .execute(
            "UPDATE agent_usage_records
             SET status = 'failed', error = ?1, completed_at = ?2
             WHERE run_id = ?3 AND conversation_id = ?4 AND message_id = ?5",
            params![
                reason,
                completed_at.max(created_at),
                run_id,
                conversation_id,
                assistant_message_id
            ],
        )
        .map_err(write_error)?;
    Ok(())
}

pub(super) fn list_result_artifacts_for_run(
    connection: &Connection,
    conversation_id: &str,
    run_id: Option<&str>,
) -> Result<Vec<AgentResultArtifactReference>, AgentGraphError> {
    let Some(run_id) = run_id else {
        return Ok(Vec::new());
    };
    let mut statement = connection
        .prepare(
            "SELECT DISTINCT artifact.artifact_id, artifact.kind, artifact.media_type
             FROM managed_artifact_grants AS grant_record
             JOIN managed_artifacts AS artifact
               ON artifact.artifact_id = grant_record.artifact_id
             WHERE grant_record.conversation_id = ?1 AND grant_record.run_id = ?2
             ORDER BY artifact.artifact_id, artifact.kind, artifact.media_type
             LIMIT ?3",
        )
        .map_err(read_error)?;
    let rows = statement
        .query_map(
            params![conversation_id, run_id, (MAX_RESULT_ARTIFACTS + 1) as i64],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .map_err(read_error)?;
    let refs = rows
        .map(|row| {
            let (artifact_id, kind, media_type) = row.map_err(read_error)?;
            let kind = match kind.as_str() {
                "image" => AgentResultArtifactKind::Image,
                "document" => AgentResultArtifactKind::Document,
                _ => return Err(corrupt("managed Artifact kind is invalid")),
            };
            Ok(AgentResultArtifactReference {
                artifact_id,
                kind,
                media_type,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    validate_result_artifact_refs(&refs)?;
    Ok(refs)
}

pub(super) fn validate_result_artifact_refs(
    refs: &[AgentResultArtifactReference],
) -> Result<(), AgentGraphError> {
    if refs.len() > MAX_RESULT_ARTIFACTS {
        return Err(conflict(format!(
            "a child result may reference at most {MAX_RESULT_ARTIFACTS} managed Artifacts"
        )));
    }
    let mut previous = None;
    for artifact in refs {
        validate_trimmed("artifact_id", &artifact.artifact_id, 256)?;
        validate_trimmed("artifact_media_type", &artifact.media_type, 256)?;
        if previous.is_some_and(|value: &str| value >= artifact.artifact_id.as_str()) {
            return Err(corrupt(
                "Agent result Artifact references are not strictly sorted and unique",
            ));
        }
        previous = Some(artifact.artifact_id.as_str());
    }
    Ok(())
}
