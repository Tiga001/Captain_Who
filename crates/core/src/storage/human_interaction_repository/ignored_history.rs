//! Ordinary backend history for an ignored asynchronous question. The immutable response is the
//! source fact; this small receipt only prevents its history projection from being repeated.
use super::*;
use crate::storage::{conversation_model_context_repository, conversation_trace_repository};
use crate::{
    AgentHumanInteractionIgnoredEvent, AgentSamplingBoundaryRequest,
    ConversationBackendStatePlacement, ConversationTraceSnapshot,
    ConversationTurnTraceTerminalStatus,
};

pub(super) fn admit_ignored_projection(
    connection: &Connection,
    conversation_id: &str,
    response_id: &str,
    now: i64,
) -> Result<()> {
    // Bind to the conversation's current history, never the potentially much older question Run.
    let (assistant, run, terminal): (String, String, String) = connection
        .query_row(
            "SELECT t.assistant_message_id,t.run_id,t.terminal_status
         FROM conversation_turn_traces t JOIN messages m ON m.id=t.assistant_message_id
         WHERE t.conversation_id=?1 AND NOT EXISTS(
             SELECT 1 FROM conversation_turn_rewrites w WHERE w.conversation_id=?1
             AND w.source_assistant_message_id=t.assistant_message_id)
         ORDER BY m.position DESC LIMIT 1",
            [conversation_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(unavailable)?;
    let placement = if terminal == "in_progress" {
        "timeline"
    } else {
        "after_message"
    };
    connection
        .execute(
            "INSERT INTO human_interaction_ignored_projections
         (response_id,conversation_id,target_assistant_message_id,target_run_id,placement,status)
         VALUES(?1,?2,?3,?4,?5,'pending')",
            params![response_id, conversation_id, assistant, run, placement],
        )
        .map_err(unavailable)?;
    if terminal != "in_progress" {
        materialize(connection, &assistant, None, now)?;
    }
    Ok(())
}

pub fn bind_ignored_at_sampling(
    connection: &mut Connection,
    request: &AgentSamplingBoundaryRequest,
    now: i64,
) -> Result<Vec<AgentHumanInteractionIgnoredEvent>> {
    valid_time(now)?;
    for id in [
        &request.conversation_id,
        &request.assistant_message_id,
        &request.run_id,
    ] {
        validate_human_interaction_id(id)?;
    }
    if request.model_batch_index == 0 {
        return Err(HumanInteractionError::invalid());
    }
    let tx = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(unavailable)?;
    let valid: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM conversation_turn_traces t
         JOIN agent_nodes n ON n.conversation_id=t.conversation_id
         WHERE t.conversation_id=?1 AND t.assistant_message_id=?2 AND t.run_id=?3
         AND t.terminal_status='in_progress' AND n.parent_agent_id IS NULL
         AND n.lifecycle='active' AND NOT EXISTS(
             SELECT 1 FROM agent_tree_run_stops s WHERE s.run_id=t.run_id))",
            params![
                request.conversation_id,
                request.assistant_message_id,
                request.run_id
            ],
            |row| row.get(0),
        )
        .map_err(unavailable)?;
    if !valid {
        return Err(conflict());
    }
    let replay = bound_events(
        &tx,
        &request.assistant_message_id,
        request.model_batch_index,
    )?;
    if !replay.is_empty() {
        // The runtime recorder validates exact identities when adopting a committed retry.
        tx.commit().map_err(unavailable)?;
        return Ok(replay);
    }
    let next: u64 = tx.query_row(
        "SELECT COALESCE(MAX(sequence)+1,0) FROM conversation_turn_trace_items WHERE assistant_message_id=?1",
        [&request.assistant_message_id], |row| row.get(0),
    ).map_err(unavailable)?;
    if next != request.expected_next_trace_sequence {
        return Err(conflict());
    }
    let result = materialize(
        &tx,
        &request.assistant_message_id,
        Some(request.model_batch_index),
        now,
    )?;
    tx.commit().map_err(unavailable)?;
    Ok(result)
}

fn bound_events(
    connection: &Connection,
    assistant: &str,
    batch: u64,
) -> Result<Vec<AgentHumanInteractionIgnoredEvent>> {
    let mut statement = connection.prepare(
        "SELECT p.trace_sequence,p.response_id,a.request_id,a.created_at
         FROM human_interaction_ignored_projections p JOIN human_interaction_responses a USING(response_id)
         WHERE p.target_assistant_message_id=?1 AND p.model_batch_index=?2 AND p.status='materialized'
         ORDER BY p.trace_sequence",
    ).map_err(unavailable)?;
    let result = statement
        .query_map(params![assistant, batch], |row| {
            Ok(AgentHumanInteractionIgnoredEvent {
                trace_sequence: row.get(0)?,
                event_id: format!("ignored:{}", row.get::<_, String>(1)?),
                request_id: row.get(2)?,
                created_at: row.get(3)?,
            })
        })
        .map_err(unavailable)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(unavailable);
    result
}

/// A committed binding can outlive a failed Host reply. Only its exact durable receipt authorizes
/// terminal publication to adopt an event which the Runtime recorder has not acknowledged yet.
pub(crate) fn is_materialized_ignored_backend_state(
    connection: &Connection,
    trace: &crate::ConversationTurnTrace,
    item: &crate::ConversationTurnTraceItem,
) -> Result<bool> {
    let crate::ConversationTurnTraceItem::BackendState {
        sequence,
        event_id,
        content,
        created_at,
        placement,
    } = item
    else {
        return Ok(false);
    };
    let row = connection
        .query_row(
            "SELECT p.response_id,a.request_id,a.created_at,p.placement
         FROM human_interaction_ignored_projections p
         JOIN human_interaction_responses a USING(response_id)
         WHERE p.conversation_id=?1 AND p.target_assistant_message_id=?2
         AND p.target_run_id=?3 AND p.trace_sequence=?4 AND p.status='materialized'
         AND a.kind='ignored'",
            params![
                trace.conversation_id,
                trace.assistant_message_id,
                trace.run_id,
                sequence
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .optional()
        .map_err(unavailable)?;
    let Some((response_id, request_id, response_created_at, recorded_placement)) = row else {
        return Ok(false);
    };
    let expected_placement = match placement {
        ConversationBackendStatePlacement::Timeline => "timeline",
        ConversationBackendStatePlacement::AfterMessage => "after_message",
    };
    Ok(event_id == &format!("ignored:{response_id}")
        && *created_at == response_created_at
        && expected_placement == recorded_placement
        && serde_json::from_str::<serde_json::Value>(content).ok()
            == Some(
                serde_json::json!({"type":"human_interaction_status","requestId":request_id,"status":"ignored"}),
            ))
}

/// Called inside terminal publication/recovery. No worker is woken and no new Turn is admitted.
pub(crate) fn flush_ignored_for_terminal(
    connection: &Connection,
    assistant: &str,
    now: i64,
) -> Result<()> {
    let terminal: bool = connection.query_row(
        "SELECT terminal_status!='in_progress' FROM conversation_turn_traces WHERE assistant_message_id=?1",
        [assistant], |row| row.get(0),
    ).optional().map_err(unavailable)?.unwrap_or(false);
    if terminal {
        materialize(connection, assistant, None, now)?;
    }
    Ok(())
}

fn materialize(
    connection: &Connection,
    assistant: &str,
    batch: Option<u64>,
    now: i64,
) -> Result<Vec<AgentHumanInteractionIgnoredEvent>> {
    let pending = {
        let mut statement = connection.prepare(
            "SELECT p.response_id,a.request_id,a.created_at,p.placement,p.target_run_id
             FROM human_interaction_ignored_projections p JOIN human_interaction_responses a USING(response_id)
             JOIN human_interaction_requests r ON r.request_id=a.request_id
             WHERE p.target_assistant_message_id=?1 AND p.status='pending'
             AND a.kind='ignored' AND r.status='ignored' AND r.mode='async'
             ORDER BY a.sequence",
        ).map_err(unavailable)?;
        let rows = statement
            .query_map([assistant], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })
            .map_err(unavailable)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(unavailable)?;
        rows
    };
    if pending.is_empty() {
        return Ok(Vec::new());
    }
    let mut trace = conversation_trace_repository::get_trace_for_message(connection, assistant)
        .map_err(unavailable)?
        .ok_or_else(conflict)?;
    if batch.is_some() && trace.terminal_status != ConversationTurnTraceTerminalStatus::InProgress {
        return Err(conflict());
    }
    let model_context_items =
        conversation_model_context_repository::get_log_for_message(connection, assistant)
            .map_err(unavailable)?
            .map(|log| log.items)
            .unwrap_or_default();
    let next_sequence = trace
        .items
        .last()
        .map_or(0, |item| item.sequence().saturating_add(1));
    let mut recorder = crate::conversation_trace::ConversationTraceRecorder::from_durable_snapshot(
        ConversationTraceSnapshot {
            items: trace.items.clone(),
            model_context_items,
            next_sequence,
            truncated: trace.truncated,
        },
    );
    let mut events = Vec::new();
    for (response_id, request_id, created_at, placement, run_id) in pending {
        if run_id != trace.run_id {
            return Err(conflict());
        }
        // Only a natural sampling boundary makes the fact visible within the active Turn.
        // Pending observations flushed after its last request belong after that reply instead.
        let (placement, placement_key) = match (batch, placement.as_str()) {
            (Some(_), "timeline") => (ConversationBackendStatePlacement::Timeline, "timeline"),
            (None, "timeline" | "after_message") if trace.terminal_status.is_terminal() => (
                ConversationBackendStatePlacement::AfterMessage,
                "after_message",
            ),
            _ => return Err(conflict()),
        };
        let event_id = format!("ignored:{response_id}");
        let content = json(
            &serde_json::json!({"type":"human_interaction_status","requestId":request_id,"status":"ignored"}),
        )?;
        let sequence = recorder.next_sequence();
        recorder
            .record_backend_state(sequence, &event_id, &content, created_at, placement)
            .map_err(unavailable)?;
        connection.execute(
            "UPDATE human_interaction_ignored_projections SET status='materialized',trace_sequence=?1,model_batch_index=?2,placement=?3
             WHERE response_id=?4 AND status='pending'",
            params![sequence,batch,placement_key,response_id],
        ).map_err(unavailable)?;
        events.push(AgentHumanInteractionIgnoredEvent {
            trace_sequence: sequence,
            event_id,
            request_id,
            created_at,
        });
    }
    let snapshot = recorder.snapshot();
    trace.items = snapshot.items;
    trace.truncated = snapshot.truncated;
    let created_at: i64 = connection
        .query_row(
            "SELECT created_at FROM conversation_turn_traces WHERE assistant_message_id=?1",
            [assistant],
            |row| row.get(0),
        )
        .map_err(unavailable)?;
    conversation_trace_repository::commit_trace_in_connection(
        connection,
        &trace,
        created_at,
        now.max(created_at),
    )
    .map_err(unavailable)?;
    conversation_model_context_repository::commit_items_in_connection(
        connection,
        &trace.conversation_id,
        assistant,
        &snapshot.model_context_items,
    )
    .map_err(unavailable)?;
    Ok(events)
}
