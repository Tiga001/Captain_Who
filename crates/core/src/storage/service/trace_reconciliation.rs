use super::*;

const STARTUP_CANCELLED_TRACE_REASON: &str =
    "Agent run was cancelled before its conversation trace was finalized.";
const STARTUP_INTERRUPTED_TRACE_REASON: &str =
    "The application exited before the agent run's conversation trace was finalized.";

#[derive(Debug)]
struct InProgressTraceCandidate {
    assistant_message_id: String,
    conversation_id: String,
    run_id: String,
    created_at: i64,
    updated_at: i64,
    agent_run_json: Option<String>,
}

impl StorageService {
    /// Atomically retires conversation traces left in-progress by an earlier process.
    ///
    /// The caller supplies every run known to be active in the current process. Production calls
    /// this only after taking the database instance lock and reconciling interrupted approvals, so
    /// an empty set means no worker can still publish to these rows. A durable pending approval is
    /// a resumable checkpoint and is deliberately excluded. Renderer state is used only to retain
    /// an explicit cancellation; it can never promote an uncommitted trace to `completed`.
    pub fn reconcile_orphaned_in_progress_conversation_turn_traces(
        &self,
        active_run_ids: &HashSet<String>,
        reconciled_at: i64,
    ) -> Result<usize, String> {
        let mut connection = self.state.connection()?;
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let candidates = {
            let mut statement = transaction
                .prepare(
                    "SELECT
                         trace.assistant_message_id,
                         trace.conversation_id,
                         trace.run_id,
                         trace.created_at,
                         trace.updated_at,
                         message.agent_run_json
                     FROM conversation_turn_traces AS trace
                     INNER JOIN messages AS message
                         ON message.id = trace.assistant_message_id
                        AND message.conversation_id = trace.conversation_id
                     WHERE trace.terminal_status = 'in_progress'
                       AND NOT EXISTS (
                           SELECT 1 FROM agent_wake_requests AS wake
                           WHERE wake.status IN ('running', 'waiting_for_approval')
                             AND wake.run_id = trace.run_id
                             AND wake.assistant_message_id = trace.assistant_message_id
                       )
                     ORDER BY trace.created_at ASC, trace.assistant_message_id ASC",
                )
                .map_err(storage_error)?;
            let candidates = statement
                .query_map([], |row| {
                    Ok(InProgressTraceCandidate {
                        assistant_message_id: row.get(0)?,
                        conversation_id: row.get(1)?,
                        run_id: row.get(2)?,
                        created_at: row.get(3)?,
                        updated_at: row.get(4)?,
                        agent_run_json: row.get(5)?,
                    })
                })
                .map_err(storage_error)?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(storage_error)?;
            candidates
        };

        let mut reconciled = 0;
        for candidate in candidates {
            if active_run_ids.contains(&candidate.run_id)
                || has_resumable_pending_action(&transaction, &candidate)?
            {
                continue;
            }

            let trace = conversation_trace_repository::get_trace_for_message(
                &transaction,
                &candidate.assistant_message_id,
            )
            .map_err(storage_error)?
            .ok_or_else(|| {
                format!(
                    "启动对账无法重新读取 in-progress trace：{}",
                    candidate.assistant_message_id
                )
            })?;
            if trace.terminal_status != crate::ConversationTurnTraceTerminalStatus::InProgress {
                continue;
            }

            let cancellation_completed_at = explicit_cancelled_run_completed_at(
                candidate.agent_run_json.as_deref(),
                &candidate.run_id,
                candidate.updated_at.max(candidate.created_at),
                reconciled_at,
            );
            let was_explicitly_cancelled = cancellation_completed_at.is_some();
            let (terminal_status, message_status, run_status, reason) = if was_explicitly_cancelled
            {
                (
                    crate::ConversationTurnTraceTerminalStatus::Cancelled,
                    "sent",
                    "cancelled",
                    STARTUP_CANCELLED_TRACE_REASON,
                )
            } else {
                (
                    crate::ConversationTurnTraceTerminalStatus::Failed,
                    "error",
                    "failed",
                    STARTUP_INTERRUPTED_TRACE_REASON,
                )
            };
            let completed_at = cancellation_completed_at.unwrap_or_else(|| {
                reconciled_at
                    .max(candidate.created_at)
                    .max(candidate.updated_at)
            });
            let model_context_items = conversation_model_context_repository::get_log_for_message(
                &transaction,
                &candidate.assistant_message_id,
            )
            .map_err(storage_error)?
            .map(|log| log.items)
            .unwrap_or_default();
            let next_sequence = trace
                .items
                .last()
                .map(ConversationTurnTraceItem::sequence)
                .unwrap_or(0)
                .saturating_add(1);
            let terminal = crate::terminal_conversation_trace_from_snapshot(
                crate::ConversationTraceSnapshot {
                    items: trace.items,
                    model_context_items,
                    next_sequence,
                    truncated: trace.truncated,
                },
                &candidate.run_id,
                &candidate.conversation_id,
                &candidate.assistant_message_id,
                terminal_status,
                reason,
            )?;
            conversation_trace_repository::commit_trace_in_connection(
                &transaction,
                &terminal.trace,
                candidate.created_at,
                completed_at,
            )
            .map_err(storage_error)?;
            conversation_model_context_repository::commit_items_in_connection(
                &transaction,
                &candidate.conversation_id,
                &candidate.assistant_message_id,
                &terminal.model_context_items,
            )
            .map_err(storage_error)?;
            chat_repository::reconcile_message_run_terminal_state(
                &transaction,
                &candidate.conversation_id,
                &candidate.assistant_message_id,
                &candidate.run_id,
                message_status,
                run_status,
                completed_at,
            )
            .map_err(storage_error)?;
            transaction
                .execute(
                    "UPDATE agent_usage_records
                     SET status = ?1, error = ?2, completed_at = ?3
                     WHERE run_id = ?4
                       AND conversation_id = ?5
                       AND message_id = ?6",
                    rusqlite::params![
                        run_status,
                        reason,
                        completed_at,
                        candidate.run_id,
                        candidate.conversation_id,
                        candidate.assistant_message_id,
                    ],
                )
                .map_err(storage_error)?;
            reconciled += 1;
        }

        transaction.commit().map_err(storage_error)?;
        Ok(reconciled)
    }
}

fn has_resumable_pending_action(
    connection: &rusqlite::Connection,
    candidate: &InProgressTraceCandidate,
) -> Result<bool, String> {
    connection
        .query_row(
            "SELECT EXISTS(
                 SELECT 1
                 FROM agent_pending_actions AS pending
                 WHERE pending.status IN ('pending', 'approved', 'executing')
                   AND (
                       pending.run_id = ?1
                       OR (
                           pending.conversation_id = ?2
                           AND pending.assistant_message_id = ?3
                       )
                   )
             )",
            rusqlite::params![
                candidate.run_id,
                candidate.conversation_id,
                candidate.assistant_message_id,
            ],
            |row| row.get(0),
        )
        .map_err(storage_error)
}

fn explicit_cancelled_run_completed_at(
    raw: Option<&str>,
    expected_run_id: &str,
    trace_committed_at: i64,
    reconciled_at: i64,
) -> Option<i64> {
    let run = raw
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
        .and_then(|value| value.as_object().cloned())?;
    if run.get("runId").and_then(serde_json::Value::as_str) != Some(expected_run_id) {
        return None;
    }
    if run.get("status").and_then(serde_json::Value::as_str) != Some("cancelled") {
        return None;
    }
    // `state.status` is renderer presentation state and may still say `running` after the
    // top-level cancellation was durably saved. It is deliberately not an authority here.
    let completed_at = run
        .get("completedAt")
        .and_then(serde_json::Value::as_i64)
        .filter(|completed_at| {
            *completed_at >= trace_committed_at && *completed_at <= reconciled_at
        })
        .unwrap_or_else(|| reconciled_at.max(trace_committed_at));
    Some(completed_at)
}
