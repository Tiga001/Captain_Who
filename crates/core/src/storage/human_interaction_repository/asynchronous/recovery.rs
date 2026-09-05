use super::*;

/// The last durable fence before an initial answer Turn may execute any Runtime work.
pub fn start_async_turn(
    connection: &mut Connection,
    binding: &HumanInteractionAsyncBinding,
    now: i64,
) -> Result<bool> {
    valid_time(now)?;
    let tx = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(unavailable)?;
    let changed=tx.execute("UPDATE human_interaction_async_bindings SET status='executing',updated_at=MAX(updated_at,?1) WHERE response_id=?2 AND claim_id=?3 AND route='new_turn' AND status='bound' AND target_run_id=?4 AND assistant_message_id=?5 AND user_message_id IS ?6 AND EXISTS(SELECT 1 FROM human_interaction_deliveries d WHERE d.response_id=?2 AND d.status='bound' AND d.target_run_id=?4) AND EXISTS(SELECT 1 FROM conversation_turn_traces t WHERE t.run_id=?4 AND t.assistant_message_id=?5 AND t.conversation_id=?7 AND t.terminal_status='in_progress') AND NOT EXISTS(SELECT 1 FROM agent_tree_run_stops s WHERE s.run_id=?4 OR s.run_id=human_interaction_async_bindings.stop_scope_run_id)",params![now,binding.response_id,binding.claim_id,binding.target_run_id,binding.assistant_message_id,binding.user_message_id,binding.conversation_id]).map_err(unavailable)?;
    tx.commit().map_err(unavailable)?;
    Ok(changed == 1)
}

pub fn settle_async_start_failure(
    connection: &mut Connection,
    run: &str,
    now: i64,
) -> Result<Option<HumanInteractionRequestSnapshot>> {
    valid_time(now)?;
    let tx = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(unavailable)?;
    let Some(binding) = load_async_binding_for_run(&tx, run)? else {
        return Ok(None);
    };
    settle_new_turn_in_transaction(&tx, &binding, now)?;
    let request = load_request(&tx, &binding.conversation_id, &binding.request_id)?;
    tx.commit().map_err(unavailable)?;
    Ok(Some(request))
}

fn settle_new_turn_in_transaction(
    connection: &Connection,
    binding: &HumanInteractionAsyncBinding,
    now: i64,
) -> Result<()> {
    let status: String = connection
        .query_row(
            "SELECT status FROM human_interaction_async_bindings WHERE response_id=?1",
            [&binding.response_id],
            |r| r.get(0),
        )
        .map_err(unavailable)?;
    if status == "cancelled" {
        terminalize_cancelled_turn(connection, binding, now)?;
        return Ok(());
    }
    if !matches!(status.as_str(), "bound" | "executing") {
        return Ok(());
    }
    let stopped:bool=connection.query_row("SELECT EXISTS(SELECT 1 FROM agent_tree_run_stops s JOIN human_interaction_async_bindings b ON b.target_run_id=s.run_id OR b.stop_scope_run_id=s.run_id WHERE b.response_id=?1)",[&binding.response_id],|r|r.get(0)).map_err(unavailable)?;
    if stopped {
        set_async_terminal(
            connection,
            &binding.response_id,
            "cancelled",
            "run_cancelled",
            now,
        )?;
        return Ok(());
    }
    let observations: i64 = connection
        .query_row(
            "SELECT (SELECT COUNT(*) FROM model_request_observations WHERE run_id=?1) + (SELECT COUNT(*) FROM conversation_model_context_items WHERE assistant_message_id=?2)",
            params![binding.target_run_id,binding.assistant_message_id],
            |r| r.get(0),
        )
        .map_err(unavailable)?;
    let trace = crate::storage::conversation_trace_repository::get_trace_for_message(
        connection,
        &binding.assistant_message_id,
    )
    .map_err(unavailable)?;
    let safely_unstarted = status == "bound"
        && observations == 0
        && trace.as_ref().is_some_and(|t| {
            t.run_id == binding.target_run_id
                && t.conversation_id == binding.conversation_id
                && t.items.is_empty()
                && matches!(
                    t.terminal_status,
                    crate::ConversationTurnTraceTerminalStatus::InProgress
                        | crate::ConversationTurnTraceTerminalStatus::Failed
                )
        });
    if !safely_unstarted {
        set_async_terminal(
            connection,
            &binding.response_id,
            "failed",
            "async_turn_outcome_unknown",
            now,
        )?;
        return Ok(());
    }
    if let Some(mut trace) = trace {
        if trace.terminal_status == crate::ConversationTurnTraceTerminalStatus::InProgress {
            trace.terminal_status = crate::ConversationTurnTraceTerminalStatus::Failed;
            trace.terminal_error=Some("The application stopped before the accepted answer Turn began; its answer remains pending.".into());
            let created: i64 = connection
                .query_row(
                    "SELECT created_at FROM conversation_turn_traces WHERE assistant_message_id=?1",
                    [&binding.assistant_message_id],
                    |r| r.get(0),
                )
                .map_err(unavailable)?;
            crate::storage::conversation_trace_repository::commit_trace_in_connection(
                connection,
                &trace,
                created,
                now.max(created),
            )
            .map_err(unavailable)?;
            crate::storage::chat_repository::update_message_run_terminal_state(
                connection,
                &binding.conversation_id,
                &binding.assistant_message_id,
                &binding.target_run_id,
                Some("error"),
                "failed",
                now,
                None,
            )
            .map_err(unavailable)?;
            connection.execute("UPDATE agent_usage_records SET status='failed',completed_at=?1 WHERE run_id=?2 AND completed_at IS NULL",params![now,binding.target_run_id]).map_err(unavailable)?;
        }
    }
    connection.execute("UPDATE human_interaction_deliveries SET status='pending',revision=revision+1,target_run_id=NULL,user_message_id=NULL,error_code=NULL WHERE response_id=?1 AND status='bound'",[&binding.response_id]).map_err(unavailable)?;
    connection.execute("UPDATE human_interaction_async_bindings SET status='pending',route=NULL,claim_id=NULL,target_run_id=NULL,assistant_message_id=NULL,updated_at=MAX(updated_at,?1) WHERE response_id=?2 AND status='bound'",params![now,binding.response_id]).map_err(unavailable)?;
    // The stable private projection id remains reserved. Keeping the provisional User in normal
    // history would allow a different natural Turn to consume it before this pending delivery.
    if let Some(user) = &binding.user_message_id {
        connection
            .execute(
                "DELETE FROM messages WHERE id=?1 AND conversation_id=?2 AND role='user'",
                params![user, binding.conversation_id],
            )
            .map_err(unavailable)?;
    }
    Ok(())
}

fn set_async_terminal(
    connection: &Connection,
    response: &str,
    status: &str,
    error: &str,
    now: i64,
) -> Result<()> {
    connection.execute("UPDATE human_interaction_deliveries SET status=?1,error_code=?2,revision=revision+1 WHERE response_id=?3 AND status IN ('pending','bound')",params![status,error,response]).map_err(unavailable)?;
    connection.execute("UPDATE human_interaction_async_bindings SET status=?1,updated_at=MAX(updated_at,?2) WHERE response_id=?3 AND status IN ('pending','bound','executing')",params![status,now,response]).map_err(unavailable)?;
    Ok(())
}

/// Recover after acquiring the process lock, before orphan-Turn cleanup or any new dispatch.
/// Unapplied Guidance has not crossed the durable model boundary and can be routed again.
/// A bound new Turn has definitely not executed; its empty assistant is retired and its stable
/// answer User id reused later. Once execution started, missing consumption proof fails closed.
pub fn reconcile_async(
    connection: &mut Connection,
    now: i64,
) -> Result<Vec<HumanInteractionRequestSnapshot>> {
    valid_time(now)?;
    let tx = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(unavailable)?;
    let rows = {
        let mut statement=tx.prepare("SELECT r.conversation_id,r.request_id,b.response_id,b.route,b.guidance_id FROM human_interaction_async_bindings b JOIN human_interaction_responses a ON a.response_id=b.response_id JOIN human_interaction_requests r ON r.request_id=a.request_id WHERE b.status IN ('pending','bound','executing') ORDER BY a.sequence").map_err(unavailable)?;
        let rows = statement
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, Option<String>>(4)?,
                ))
            })
            .map_err(unavailable)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(unavailable)?;
        rows
    };
    for (_, _, response, route, guidance) in &rows {
        let stopped:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM agent_tree_run_stops s JOIN human_interaction_async_bindings b ON b.target_run_id=s.run_id OR b.stop_scope_run_id=s.run_id WHERE b.response_id=?1)",[response],|r|r.get(0)).map_err(unavailable)?;
        if stopped {
            set_async_terminal(&tx, response, "cancelled", "run_cancelled", now)?;
            continue;
        }
        match route.as_deref() {
            Some("guidance") => {
                if let Some(id) = guidance {
                    crate::storage::guidance_repository::mark_guidance_terminal(
                        &tx,
                        id,
                        crate::AgentGuidanceStatus::Abandoned,
                        "Host restarted before the human answer reached the model boundary.",
                        now,
                    )
                    .map_err(unavailable)?;
                }
            }
            Some("new_turn") => {
                if let Some(binding) = load_async_binding(&tx, response)? {
                    settle_new_turn_in_transaction(&tx, &binding, now)?;
                } else {
                    set_async_terminal(&tx, response, "failed", "async_turn_outcome_unknown", now)?;
                }
            }
            _ => {}
        }
    }
    let snapshots = rows
        .iter()
        .map(|(conversation, id, _, _, _)| load_request(&tx, conversation, id))
        .collect::<Result<Vec<_>>>()?;
    tx.commit().map_err(unavailable)?;
    Ok(snapshots)
}

fn terminalize_cancelled_turn(
    connection: &Connection,
    binding: &HumanInteractionAsyncBinding,
    now: i64,
) -> Result<()> {
    let Some(mut trace) = crate::storage::conversation_trace_repository::get_trace_for_message(
        connection,
        &binding.assistant_message_id,
    )
    .map_err(unavailable)?
    else {
        return Ok(());
    };
    if trace.run_id != binding.target_run_id
        || trace.conversation_id != binding.conversation_id
        || trace.terminal_status != crate::ConversationTurnTraceTerminalStatus::InProgress
    {
        return Ok(());
    }
    let stopped: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM agent_tree_run_stops s JOIN human_interaction_async_bindings b ON b.target_run_id=s.run_id OR b.stop_scope_run_id=s.run_id WHERE b.response_id=?1)",
            [&binding.response_id],
            |r| r.get(0),
        )
        .map_err(unavailable)?;
    if !stopped {
        return Ok(());
    }
    trace.terminal_status = crate::ConversationTurnTraceTerminalStatus::Cancelled;
    trace.terminal_error = Some("The answer Turn was cancelled before launch.".into());
    let created: i64 = connection
        .query_row(
            "SELECT created_at FROM conversation_turn_traces WHERE assistant_message_id=?1",
            [&binding.assistant_message_id],
            |r| r.get(0),
        )
        .map_err(unavailable)?;
    crate::storage::conversation_trace_repository::commit_trace_in_connection(
        connection,
        &trace,
        created,
        now.max(created),
    )
    .map_err(unavailable)?;
    crate::storage::chat_repository::update_message_run_terminal_state(
        connection,
        &binding.conversation_id,
        &binding.assistant_message_id,
        &binding.target_run_id,
        Some("sent"),
        "cancelled",
        now,
        None,
    )
    .map_err(unavailable)?;
    connection.execute("UPDATE agent_usage_records SET status='cancelled',completed_at=?1 WHERE run_id=?2 AND completed_at IS NULL",params![now,binding.target_run_id]).map_err(unavailable)?;
    Ok(())
}
