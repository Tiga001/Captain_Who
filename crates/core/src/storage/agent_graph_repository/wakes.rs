use super::common::{
    conflict, corrupt, immediate, invalid, read_error, stable_fact_id, validate_id,
    validate_request_id, validate_time, write_error, WAKE_LEASE_DURATION_MS,
};
use super::nodes::{ensure_active_agent, is_strict_descendant};
use super::project_agent_wake_source_in_transaction;
use super::wake_commands::{enqueue_wake_in_transaction, validate_wake_input};
use super::wake_records::{
    decode_wake, query_wake, query_wake_by_claim_token, query_wakes, read_wake_row, WAKE_SELECT,
};
use crate::{
    AgentGraphError, AgentNodeRecord, AgentWakeRecoveryAction, AgentWakeRecoveryBatch,
    AgentWakeRequestRecord, AgentWakeStatus, EnqueueAgentWakeInput, IdempotentCreate,
    InterruptAgentExecutionOutcome, InterruptAgentExecutionReceipt, UndispatchedAgentInterrupt,
};
use rusqlite::{params, Connection, OptionalExtension};

pub fn enqueue_agent_wake(
    connection: &mut Connection,
    input: &EnqueueAgentWakeInput,
    created_at: i64,
) -> Result<IdempotentCreate<AgentWakeRequestRecord>, AgentGraphError> {
    validate_wake_input(input, created_at)?;
    let transaction = immediate(connection)?;
    let outcome = enqueue_wake_in_transaction(&transaction, input, created_at)?;
    transaction.commit().map_err(write_error)?;
    Ok(outcome)
}

pub fn get_agent_wake(
    connection: &Connection,
    wake_id: &str,
) -> Result<Option<AgentWakeRequestRecord>, AgentGraphError> {
    validate_id("wake_id", wake_id)?;
    query_wake(connection, wake_id)
}

pub fn claim_next_agent_wake(
    connection: &mut Connection,
    agent_id: &str,
    claim_token: &str,
    claimed_at: i64,
) -> Result<Option<AgentWakeRequestRecord>, AgentGraphError> {
    validate_id("agent_id", agent_id)?;
    validate_id("claim_token", claim_token)?;
    validate_time(claimed_at)?;
    let transaction = immediate(connection)?;
    if let Some(existing) = query_wake_by_claim_token(&transaction, claim_token)? {
        if existing.agent_id != agent_id {
            return Err(conflict("Wake claim token was reused for another Agent"));
        }
        transaction.commit().map_err(write_error)?;
        return Ok(Some(existing));
    }
    ensure_active_agent(&transaction, agent_id)?;
    let lease_expires_at = claimed_at
        .checked_add(WAKE_LEASE_DURATION_MS)
        .ok_or_else(|| invalid("claimed_at", "cannot compute the Wake lease deadline"))?;
    let stale_claimed = transaction
        .query_row(
            "SELECT wake_id FROM agent_wake_requests
             WHERE agent_id = ?1 AND status = 'claimed' AND lease_expires_at <= ?2
             ORDER BY sequence LIMIT 1",
            params![agent_id, claimed_at],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(read_error)?;
    if let Some(stale_wake_id) = stale_claimed {
        transaction
            .execute(
                "UPDATE agent_wake_requests
                 SET claim_token = NULL, lease_expires_at = NULL, claimed_at = NULL,
                     status = 'queued', status_revision = status_revision + 1
                 WHERE wake_id = ?1 AND status = 'claimed' AND lease_expires_at <= ?2",
                params![stale_wake_id, claimed_at],
            )
            .map_err(write_error)?;
    }
    let already_active = transaction
        .query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM agent_wake_requests
                 WHERE agent_id = ?1
                   AND status IN ('claimed', 'running', 'waiting_for_approval')
             )",
            [agent_id],
            |row| row.get::<_, bool>(0),
        )
        .map_err(read_error)?;
    if already_active {
        transaction.commit().map_err(write_error)?;
        return Ok(None);
    }
    let next_id = transaction
        .query_row(
            "SELECT wake_id FROM agent_wake_requests
             WHERE agent_id = ?1 AND status = 'queued'
             ORDER BY sequence LIMIT 1",
            [agent_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(read_error)?;
    let Some(wake_id) = next_id else {
        transaction.commit().map_err(write_error)?;
        return Ok(None);
    };
    transaction
        .execute(
            "UPDATE agent_wake_requests
             SET status = 'claimed', status_revision = status_revision + 1,
                 claim_token = ?1, lease_expires_at = ?2, claimed_at = ?3
             WHERE wake_id = ?4 AND status = 'queued'",
            params![claim_token, lease_expires_at, claimed_at, &wake_id],
        )
        .map_err(write_error)?;
    let wake = query_wake(&transaction, &wake_id)?
        .ok_or_else(|| corrupt("claimed Wake could not be read back"))?;
    transaction.commit().map_err(write_error)?;
    Ok(Some(wake))
}

/// Claims the globally oldest Wake whose Agent has neither another active Wake nor an active
/// Conversation Turn. The Wake source projection and claim share this `BEGIN IMMEDIATE`
/// transaction, closing the projection-without-execution crash window for follow-ups/results.
pub fn claim_next_dispatchable_agent_wake(
    connection: &mut Connection,
    claim_token: &str,
    claimed_at: i64,
) -> Result<Option<AgentWakeRequestRecord>, AgentGraphError> {
    validate_id("claim_token", claim_token)?;
    validate_time(claimed_at)?;
    let transaction = immediate(connection)?;
    if let Some(existing) = query_wake_by_claim_token(&transaction, claim_token)? {
        transaction.commit().map_err(write_error)?;
        return Ok(Some(existing));
    }
    let next_id = transaction
        .query_row(
            "SELECT wake.wake_id
             FROM agent_wake_requests AS wake
             JOIN agent_nodes AS agent ON agent.agent_id = wake.agent_id
             WHERE wake.status = 'queued'
               AND agent.lifecycle = 'active'
               AND NOT EXISTS (
                   SELECT 1 FROM agent_wake_requests AS active
                   WHERE active.agent_id = wake.agent_id
                     AND active.status IN ('claimed', 'running', 'waiting_for_approval')
               )
               AND NOT EXISTS (
                   SELECT 1 FROM conversation_turn_traces AS trace
                   WHERE trace.conversation_id = agent.conversation_id
                     AND trace.terminal_status = 'in_progress'
               )
               AND NOT EXISTS (
                   SELECT 1 FROM agent_mailbox_messages AS mailbox
                   WHERE mailbox.recipient_agent_id = wake.agent_id
                     AND mailbox.delivery_status = 'claimed'
                     AND mailbox.lease_expires_at > ?1
               )
             ORDER BY wake.sequence, wake.wake_id
             LIMIT 1",
            [claimed_at],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(read_error)?;
    let Some(wake_id) = next_id else {
        transaction.commit().map_err(write_error)?;
        return Ok(None);
    };
    let projection_prefix = stable_fact_id("wake-dispatch-projection", &[claim_token, &wake_id]);
    project_agent_wake_source_in_transaction(
        &transaction,
        &wake_id,
        &projection_prefix,
        claimed_at,
    )?;
    let lease_expires_at = claimed_at
        .checked_add(WAKE_LEASE_DURATION_MS)
        .ok_or_else(|| invalid("claimed_at", "cannot compute the Wake lease deadline"))?;
    let changed = transaction
        .execute(
            "UPDATE agent_wake_requests
             SET status = 'claimed', status_revision = status_revision + 1,
                 claim_token = ?1, lease_expires_at = ?2, claimed_at = ?3
             WHERE wake_id = ?4 AND status = 'queued'",
            params![claim_token, lease_expires_at, claimed_at, &wake_id],
        )
        .map_err(write_error)?;
    if changed != 1 {
        return Err(conflict("global Wake claim lost its durable CAS"));
    }
    let wake = query_wake(&transaction, &wake_id)?
        .ok_or_else(|| corrupt("globally claimed Wake disappeared"))?;
    transaction.commit().map_err(write_error)?;
    Ok(Some(wake))
}

/// Reconciles leases left by an earlier Host without replaying a possibly side-effecting Turn.
/// Claimed work has not crossed atomic Turn admission and is safely requeued. Running work always
/// has immutable run/assistant identity: terminal traces and resumable approvals are rebound for
/// observation, a missing trace is definitely pre-Runtime, and an in-progress non-approval trace
/// becomes outcome-unknown.
pub fn recover_agent_wakes(
    connection: &mut Connection,
    recovery_token_prefix: &str,
    recovered_at: i64,
) -> Result<AgentWakeRecoveryBatch, AgentGraphError> {
    validate_id("recovery_token_prefix", recovery_token_prefix)?;
    validate_time(recovered_at)?;
    let transaction = immediate(connection)?;
    let requeued_before_dispatch = transaction
        .execute(
            "UPDATE agent_wake_requests
             SET status = 'queued', status_revision = status_revision + 1,
                 claim_token = NULL, lease_expires_at = NULL, claimed_at = NULL
             WHERE status = 'claimed' AND lease_expires_at <= ?1",
            [recovered_at],
        )
        .map_err(write_error)?;

    let mut statement = transaction
        .prepare(&format!(
            "{WAKE_SELECT}
             WHERE status IN ('running', 'waiting_for_approval')
               AND lease_expires_at <= ?1
             ORDER BY sequence, wake_id"
        ))
        .map_err(read_error)?;
    let expired = statement
        .query_map([recovered_at], read_wake_row)
        .map_err(read_error)?
        .map(|row| row.map_err(read_error).and_then(decode_wake))
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);

    let mut actions = Vec::with_capacity(expired.len());
    for mut wake in expired {
        let (Some(run_id), Some(assistant_message_id)) =
            (wake.run_id.as_deref(), wake.assistant_message_id.as_deref())
        else {
            return Err(corrupt(
                "active Wake is missing its immutable Turn identity",
            ));
        };
        let trace_status = transaction
            .query_row(
                "SELECT terminal_status FROM conversation_turn_traces
                 WHERE assistant_message_id = ?1 AND run_id = ?2",
                params![assistant_message_id, run_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(read_error)?;
        let has_pending_approval = transaction
            .query_row(
                "SELECT EXISTS(
                     SELECT 1 FROM agent_pending_actions
                     WHERE run_id = ?1 AND assistant_message_id = ?2
                       AND status IN ('pending', 'approved', 'executing')
                 )",
                params![run_id, assistant_message_id],
                |row| row.get::<_, bool>(0),
            )
            .map_err(read_error)?;
        let tree_stopped = transaction
            .query_row(
                "SELECT EXISTS(
                     SELECT 1 FROM agent_tree_run_stops WHERE run_id = ?1
                 )",
                [run_id],
                |row| row.get::<_, bool>(0),
            )
            .map_err(read_error)?;
        let action = match (tree_stopped, trace_status.as_deref()) {
            // The Runtime may have committed its real terminal trace immediately before the Host
            // exited but before the Dispatcher settled the Wake. Preserve that outcome; the
            // durable tree stop will still suppress the parent Wake during result settlement.
            (true, Some("completed" | "failed" | "cancelled")) => {
                AgentWakeRecoveryAction::Observe(wake.clone())
            }
            (true, _) => AgentWakeRecoveryAction::TreeStopped(wake.clone()),
            (false, trace_status) => match trace_status {
                None => AgentWakeRecoveryAction::FailBeforeRuntime(wake.clone()),
                Some("completed" | "failed" | "cancelled") => {
                    AgentWakeRecoveryAction::Observe(wake.clone())
                }
                Some("in_progress") if has_pending_approval => {
                    AgentWakeRecoveryAction::Observe(wake.clone())
                }
                Some("in_progress") => AgentWakeRecoveryAction::OutcomeUnknown(wake.clone()),
                Some(_) => {
                    return Err(corrupt(
                        "active Wake references an invalid Turn trace status",
                    ))
                }
            },
        };
        let recovery_token = stable_fact_id(
            "wake-recovery-claim",
            &[recovery_token_prefix, &wake.wake_id],
        );
        let deadline = recovered_at
            .checked_add(WAKE_LEASE_DURATION_MS)
            .ok_or_else(|| invalid("recovered_at", "cannot compute recovery lease"))?;
        let changed = transaction
            .execute(
                "UPDATE agent_wake_requests
                 SET claim_token = ?1, lease_expires_at = ?2, claimed_at = ?6
                 WHERE wake_id = ?3 AND status = ?4 AND claim_token = ?5
                   AND lease_expires_at <= ?6",
                params![
                    &recovery_token,
                    deadline,
                    &wake.wake_id,
                    wake.status.as_str(),
                    &wake.claim_token,
                    recovered_at,
                ],
            )
            .map_err(write_error)?;
        if changed != 1 {
            return Err(conflict("expired Wake recovery lost its ownership CAS"));
        }
        wake.claim_token = Some(recovery_token);
        wake.lease_expires_at = Some(deadline);
        actions.push(match action {
            AgentWakeRecoveryAction::Observe(_) => AgentWakeRecoveryAction::Observe(wake),
            AgentWakeRecoveryAction::FailBeforeRuntime(_) => {
                AgentWakeRecoveryAction::FailBeforeRuntime(wake)
            }
            AgentWakeRecoveryAction::OutcomeUnknown(_) => {
                AgentWakeRecoveryAction::OutcomeUnknown(wake)
            }
            AgentWakeRecoveryAction::TreeStopped(_) => AgentWakeRecoveryAction::TreeStopped(wake),
        });
    }
    transaction.commit().map_err(write_error)?;
    Ok(AgentWakeRecoveryBatch {
        requeued_before_dispatch,
        actions,
    })
}

/// Authorizes an internal management interrupt against one strict descendant. A queued or
/// claimed-before-admission Wake is atomically cancelled. Once atomic Turn admission has bound a
/// run identity, persistence is left untouched here and the Host must propagate cancellation to
/// that exact run; its durable observer later settles the Wake as `interrupted` with a result.
pub fn interrupt_agent_execution(
    connection: &mut Connection,
    caller_agent_id: &str,
    target_agent_id: &str,
    request_id: &str,
    interrupted_at: i64,
) -> Result<InterruptAgentExecutionOutcome, AgentGraphError> {
    interrupt_agent_execution_with_receipt(
        connection,
        caller_agent_id,
        target_agent_id,
        request_id,
        interrupted_at,
    )
    .map(|receipt| receipt.outcome)
}

/// Persists an idempotent interrupt request and returns both its frozen disposition and whether
/// an active-Turn cancellation has already been delivered by the Host.
pub fn interrupt_agent_execution_with_receipt(
    connection: &mut Connection,
    caller_agent_id: &str,
    target_agent_id: &str,
    request_id: &str,
    interrupted_at: i64,
) -> Result<InterruptAgentExecutionReceipt, AgentGraphError> {
    validate_id("caller_agent_id", caller_agent_id)?;
    validate_id("target_agent_id", target_agent_id)?;
    validate_request_id(request_id)?;
    validate_time(interrupted_at)?;
    let transaction = immediate(connection)?;
    let caller = ensure_active_agent(&transaction, caller_agent_id)?;
    let target = ensure_active_agent(&transaction, target_agent_id)?;
    if caller.root_agent_id != target.root_agent_id {
        return Err(conflict("interrupt authority cannot cross Agent trees"));
    }
    if !is_strict_descendant(&transaction, &caller.agent_id, &target.agent_id)? {
        return Err(conflict(
            "interrupt authority is limited to a caller's strict descendants",
        ));
    }
    if let Some((persisted_target, receipt)) =
        query_interrupt_receipt(&transaction, caller_agent_id, request_id)?
    {
        if persisted_target != target_agent_id {
            return Err(conflict(
                "interrupt request ID is already bound to another target",
            ));
        }
        transaction.commit().map_err(write_error)?;
        return Ok(receipt);
    }

    let active = query_wakes(
        &transaction,
        &format!(
            "{WAKE_SELECT}
             WHERE agent_id = ?1
               AND status IN ('claimed', 'running', 'waiting_for_approval')
             ORDER BY sequence, wake_id LIMIT 1"
        ),
        [target_agent_id],
    )?
    .into_iter()
    .next();
    if let Some(wake) = active {
        if matches!(
            wake.status,
            AgentWakeStatus::Running | AgentWakeStatus::WaitingForApproval
        ) {
            let run_id = wake
                .run_id
                .clone()
                .ok_or_else(|| corrupt("active admitted Wake is missing run identity"))?;
            let outcome = InterruptAgentExecutionOutcome::ActiveTurn {
                wake_id: wake.wake_id.clone(),
                run_id: run_id.clone(),
            };
            insert_interrupt_receipt(
                &transaction,
                &caller,
                &target,
                request_id,
                &outcome,
                interrupted_at,
            )?;
            transaction.commit().map_err(write_error)?;
            return Ok(InterruptAgentExecutionReceipt {
                outcome,
                dispatched_at: None,
            });
        }
        let changed = transaction
            .execute(
                "UPDATE agent_wake_requests
                 SET status = 'cancelled', status_revision = status_revision + 1,
                     completed_at = ?1
                 WHERE wake_id = ?2 AND status = 'claimed' AND run_id IS NULL",
                params![interrupted_at, &wake.wake_id],
            )
            .map_err(write_error)?;
        if changed != 1 {
            return Err(conflict("claimed Wake interrupt lost its durable CAS"));
        }
        let outcome = InterruptAgentExecutionOutcome::QueuedWakeCancelled {
            wake_id: wake.wake_id,
        };
        insert_interrupt_receipt(
            &transaction,
            &caller,
            &target,
            request_id,
            &outcome,
            interrupted_at,
        )?;
        transaction.commit().map_err(write_error)?;
        return Ok(InterruptAgentExecutionReceipt {
            outcome,
            dispatched_at: None,
        });
    }

    let queued_id = transaction
        .query_row(
            "SELECT wake_id FROM agent_wake_requests
             WHERE agent_id = ?1 AND status = 'queued'
             ORDER BY sequence, wake_id LIMIT 1",
            [target_agent_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(read_error)?;
    let Some(wake_id) = queued_id else {
        insert_interrupt_receipt(
            &transaction,
            &caller,
            &target,
            request_id,
            &InterruptAgentExecutionOutcome::NoPendingExecution,
            interrupted_at,
        )?;
        transaction.commit().map_err(write_error)?;
        return Ok(InterruptAgentExecutionReceipt {
            outcome: InterruptAgentExecutionOutcome::NoPendingExecution,
            dispatched_at: None,
        });
    };
    let changed = transaction
        .execute(
            "UPDATE agent_wake_requests
             SET status = 'cancelled', status_revision = status_revision + 1,
                 completed_at = ?1
             WHERE wake_id = ?2 AND status = 'queued'",
            params![interrupted_at, &wake_id],
        )
        .map_err(write_error)?;
    if changed != 1 {
        return Err(conflict("queued Wake interrupt lost its durable CAS"));
    }
    let outcome = InterruptAgentExecutionOutcome::QueuedWakeCancelled { wake_id };
    insert_interrupt_receipt(
        &transaction,
        &caller,
        &target,
        request_id,
        &outcome,
        interrupted_at,
    )?;
    transaction.commit().map_err(write_error)?;
    Ok(InterruptAgentExecutionReceipt {
        outcome,
        dispatched_at: None,
    })
}

pub(super) fn query_interrupt_receipt(
    connection: &Connection,
    caller_agent_id: &str,
    request_id: &str,
) -> Result<Option<(String, InterruptAgentExecutionReceipt)>, AgentGraphError> {
    let row = connection
        .query_row(
            "SELECT target_agent_id, disposition, wake_id, run_id, dispatched_at
             FROM agent_interrupt_requests
             WHERE caller_agent_id = ?1 AND request_id = ?2",
            params![caller_agent_id, request_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<i64>>(4)?,
                ))
            },
        )
        .optional()
        .map_err(read_error)?;
    row.map(|(target, disposition, wake_id, run_id, dispatched_at)| {
        let outcome = match (disposition.as_str(), wake_id, run_id) {
            ("no_pending_execution", None, None) => {
                InterruptAgentExecutionOutcome::NoPendingExecution
            }
            ("queued_wake_cancelled", Some(wake_id), None) => {
                InterruptAgentExecutionOutcome::QueuedWakeCancelled { wake_id }
            }
            ("active_turn", Some(wake_id), Some(run_id)) => {
                InterruptAgentExecutionOutcome::ActiveTurn { wake_id, run_id }
            }
            _ => {
                return Err(corrupt(
                    "Agent interrupt receipt has invalid disposition facts",
                ))
            }
        };
        if dispatched_at.is_some()
            && !matches!(&outcome, InterruptAgentExecutionOutcome::ActiveTurn { .. })
        {
            return Err(corrupt(
                "Only an active-Turn Agent interrupt receipt may be marked dispatched",
            ));
        }
        Ok((
            target,
            InterruptAgentExecutionReceipt {
                outcome,
                dispatched_at,
            },
        ))
    })
    .transpose()
}

pub(super) fn insert_interrupt_receipt(
    connection: &Connection,
    caller: &AgentNodeRecord,
    target: &AgentNodeRecord,
    request_id: &str,
    outcome: &InterruptAgentExecutionOutcome,
    created_at: i64,
) -> Result<(), AgentGraphError> {
    let (disposition, wake_id, run_id) = match outcome {
        InterruptAgentExecutionOutcome::NoPendingExecution => ("no_pending_execution", None, None),
        InterruptAgentExecutionOutcome::QueuedWakeCancelled { wake_id } => {
            ("queued_wake_cancelled", Some(wake_id.as_str()), None)
        }
        InterruptAgentExecutionOutcome::ActiveTurn { wake_id, run_id } => {
            ("active_turn", Some(wake_id.as_str()), Some(run_id.as_str()))
        }
    };
    connection
        .execute(
            "INSERT INTO agent_interrupt_requests (
                 caller_agent_id, request_id, root_agent_id, target_agent_id,
                 disposition, wake_id, run_id, created_at, dispatched_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, NULL)",
            params![
                &caller.agent_id,
                request_id,
                &caller.root_agent_id,
                &target.agent_id,
                disposition,
                wake_id,
                run_id,
                created_at
            ],
        )
        .map_err(write_error)?;
    Ok(())
}

/// Marks a persisted active-Turn interrupt as delivered to the process-local executor.
///
/// The update is monotonic and idempotent: once set, later retries preserve the original
/// timestamp and can skip a duplicate executor call.
pub fn mark_agent_interrupt_dispatched(
    connection: &mut Connection,
    caller_agent_id: &str,
    request_id: &str,
    dispatched_at: i64,
) -> Result<InterruptAgentExecutionReceipt, AgentGraphError> {
    validate_id("caller_agent_id", caller_agent_id)?;
    validate_request_id(request_id)?;
    validate_time(dispatched_at)?;
    let transaction = immediate(connection)?;
    let (_, receipt) = query_interrupt_receipt(&transaction, caller_agent_id, request_id)?
        .ok_or_else(|| conflict("Agent interrupt request receipt does not exist"))?;
    if !matches!(
        &receipt.outcome,
        InterruptAgentExecutionOutcome::ActiveTurn { .. }
    ) {
        return Err(conflict(
            "Only an active-Turn Agent interrupt can be marked dispatched",
        ));
    }
    if receipt.dispatched_at.is_some() {
        transaction.commit().map_err(write_error)?;
        return Ok(receipt);
    }
    let changed = transaction
        .execute(
            "UPDATE agent_interrupt_requests
             SET dispatched_at = ?1
             WHERE caller_agent_id = ?2 AND request_id = ?3
               AND disposition = 'active_turn' AND dispatched_at IS NULL",
            params![dispatched_at, caller_agent_id, request_id],
        )
        .map_err(write_error)?;
    if changed != 1 {
        return Err(conflict(
            "Agent interrupt dispatch acknowledgement lost its durable CAS",
        ));
    }
    let (_, marked) = query_interrupt_receipt(&transaction, caller_agent_id, request_id)?
        .ok_or_else(|| corrupt("Agent interrupt receipt disappeared after dispatch marking"))?;
    transaction.commit().map_err(write_error)?;
    Ok(marked)
}

/// Lists active-Turn interrupt receipts which committed before runtime delivery was acknowledged.
///
/// Immutable receipt facts are checked against the referenced Wake before they cross the storage
/// boundary. Terminal Wakes need no runtime delivery and are excluded without pretending that a
/// process-local cancellation signal was dispatched.
pub fn list_undispatched_agent_interrupts(
    connection: &Connection,
) -> Result<Vec<UndispatchedAgentInterrupt>, AgentGraphError> {
    let mut statement = connection
        .prepare(
            "SELECT interrupt.caller_agent_id, interrupt.target_agent_id,
                    interrupt.request_id, interrupt.wake_id, interrupt.run_id,
                    interrupt.root_agent_id, wake.agent_id, wake.root_agent_id, wake.run_id
             FROM agent_interrupt_requests AS interrupt
             JOIN agent_wake_requests AS wake ON wake.wake_id = interrupt.wake_id
             WHERE interrupt.disposition = 'active_turn'
               AND interrupt.dispatched_at IS NULL
               AND wake.status IN ('claimed', 'running', 'waiting_for_approval')
             ORDER BY interrupt.created_at, interrupt.caller_agent_id, interrupt.request_id",
        )
        .map_err(read_error)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, Option<String>>(8)?,
            ))
        })
        .map_err(read_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(read_error)?;
    let mut pending = Vec::with_capacity(rows.len());
    for (
        caller_agent_id,
        target_agent_id,
        request_id,
        wake_id,
        run_id,
        receipt_root_agent_id,
        wake_agent_id,
        wake_root_agent_id,
        wake_run_id,
    ) in rows
    {
        if target_agent_id != wake_agent_id
            || receipt_root_agent_id != wake_root_agent_id
            || wake_run_id.as_deref() != Some(run_id.as_str())
        {
            return Err(corrupt(
                "undispatched Agent interrupt no longer matches its immutable Wake identity",
            ));
        }
        pending.push(UndispatchedAgentInterrupt {
            caller_agent_id,
            target_agent_id,
            request_id,
            wake_id,
            run_id,
        });
    }
    Ok(pending)
}

pub fn renew_agent_wake_lease(
    connection: &mut Connection,
    wake_id: &str,
    claim_token: &str,
    renewed_at: i64,
) -> Result<AgentWakeRequestRecord, AgentGraphError> {
    validate_id("wake_id", wake_id)?;
    validate_id("claim_token", claim_token)?;
    validate_time(renewed_at)?;
    let lease_expires_at = renewed_at
        .checked_add(WAKE_LEASE_DURATION_MS)
        .ok_or_else(|| invalid("renewed_at", "cannot compute the Wake lease deadline"))?;
    let transaction = immediate(connection)?;
    let current = query_wake(&transaction, wake_id)?
        .ok_or_else(|| AgentGraphError::WakeNotFound(wake_id.to_string()))?;
    if !matches!(
        current.status,
        AgentWakeStatus::Claimed | AgentWakeStatus::Running | AgentWakeStatus::WaitingForApproval
    ) || current.claim_token.as_deref() != Some(claim_token)
    {
        return Err(conflict("Wake is not actively held by this claim token"));
    }
    let current_deadline = current
        .lease_expires_at
        .ok_or_else(|| corrupt("active Wake has no lease deadline"))?;
    if renewed_at >= current_deadline {
        return Err(conflict("Wake lease has already expired"));
    }
    let advanced_deadline = lease_expires_at.max(current_deadline.saturating_add(1));
    transaction
        .execute(
            "UPDATE agent_wake_requests
             SET lease_expires_at = ?1
             WHERE wake_id = ?2 AND claim_token = ?3
               AND status IN ('claimed', 'running', 'waiting_for_approval')
               AND lease_expires_at = ?4",
            params![advanced_deadline, wake_id, claim_token, current_deadline],
        )
        .map_err(write_error)?;
    let renewed = query_wake(&transaction, wake_id)?
        .ok_or_else(|| corrupt("renewed Wake could not be read back"))?;
    transaction.commit().map_err(write_error)?;
    Ok(renewed)
}

pub fn transition_agent_wake(
    connection: &mut Connection,
    wake_id: &str,
    expected_status: AgentWakeStatus,
    requested_status: AgentWakeStatus,
    claim_token: Option<&str>,
    transitioned_at: i64,
) -> Result<AgentWakeRequestRecord, AgentGraphError> {
    let transaction = immediate(connection)?;
    let updated = transition_agent_wake_in_connection(
        &transaction,
        wake_id,
        expected_status,
        requested_status,
        claim_token,
        transitioned_at,
    )?;
    transaction.commit().map_err(write_error)?;
    Ok(updated)
}

/// Applies one Wake lifecycle CAS inside a caller-owned transaction.
///
/// Approval resumption uses this primitive so the pending-action decision and the child Wake
/// cannot become externally visible as two contradictory durable facts. Callers own commit or
/// rollback; this helper never starts a nested SQLite transaction.
pub(crate) fn transition_agent_wake_in_connection(
    connection: &Connection,
    wake_id: &str,
    expected_status: AgentWakeStatus,
    requested_status: AgentWakeStatus,
    claim_token: Option<&str>,
    transitioned_at: i64,
) -> Result<AgentWakeRequestRecord, AgentGraphError> {
    validate_id("wake_id", wake_id)?;
    validate_time(transitioned_at)?;
    if let Some(token) = claim_token {
        validate_id("claim_token", token)?;
    }
    let current = query_wake(connection, wake_id)?
        .ok_or_else(|| AgentGraphError::WakeNotFound(wake_id.to_string()))?;
    if current.status != expected_status {
        return Err(conflict(
            "expected Wake status does not match persisted status",
        ));
    }
    if current.status == requested_status {
        if current.claim_token.as_deref() == claim_token || claim_token.is_none() {
            return Ok(current);
        }
        return Err(conflict("Wake is held by another claim token"));
    }
    if !current.status.can_transition_to(requested_status) {
        return Err(AgentGraphError::IllegalTransition {
            current: current.status,
            requested: requested_status,
        });
    }
    if current.status != AgentWakeStatus::Queued && current.claim_token.as_deref() != claim_token {
        return Err(conflict("Wake is held by another claim token"));
    }
    if current.status != AgentWakeStatus::Queued
        && current
            .lease_expires_at
            .is_none_or(|deadline| transitioned_at >= deadline)
    {
        return Err(conflict("Wake lease has expired"));
    }
    if matches!(
        requested_status,
        AgentWakeStatus::Completed | AgentWakeStatus::Satisfied
    ) {
        return Err(conflict(
            "completed/satisfied Wake must be settled by its atomic result or receipt API",
        ));
    }

    let (next_claim_token, lease_expires_at, claimed_at, started_at, completed_at) =
        match requested_status {
            AgentWakeStatus::Queued => (None, None, None, None, None),
            AgentWakeStatus::Running | AgentWakeStatus::WaitingForApproval => (
                current.claim_token.as_deref(),
                current.lease_expires_at,
                current.claimed_at,
                Some(current.started_at.unwrap_or(transitioned_at)),
                None,
            ),
            AgentWakeStatus::Failed
            | AgentWakeStatus::Interrupted
            | AgentWakeStatus::Cancelled
            | AgentWakeStatus::OutcomeUnknown => (
                current.claim_token.as_deref(),
                current.lease_expires_at,
                current.claimed_at,
                current.started_at,
                Some(transitioned_at),
            ),
            AgentWakeStatus::Claimed => {
                return Err(conflict(
                    "queued Wake must be claimed through the claim API",
                ));
            }
            AgentWakeStatus::Completed | AgentWakeStatus::Satisfied => unreachable!(),
        };
    connection
        .execute(
            "UPDATE agent_wake_requests
             SET status = ?1, status_revision = status_revision + 1,
                 claim_token = ?2, lease_expires_at = ?3, claimed_at = ?4,
                 started_at = ?5, completed_at = ?6, terminal_error = ?7
             WHERE wake_id = ?8 AND status = ?9",
            params![
                requested_status.as_str(),
                next_claim_token,
                lease_expires_at,
                claimed_at,
                started_at,
                completed_at,
                Option::<&str>::None,
                wake_id,
                expected_status.as_str(),
            ],
        )
        .map_err(write_error)?;
    let updated = query_wake(connection, wake_id)?
        .ok_or_else(|| corrupt("updated Wake could not be read back"))?;
    Ok(updated)
}
