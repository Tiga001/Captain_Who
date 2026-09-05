//! Host-only blocking question admission and delivery. No renderer input carries these bindings.
use super::*;
use crate::storage::{chat_repository, models::AgentUsageRecordInsert, usage_repository};

#[derive(Debug, Clone)]
pub struct HumanInteractionSyncAdmission {
    pub owner: HostHumanInteractionOwner,
    pub input: HumanInteractionToolInput,
    /// Versioned, executable checkpoint and private Host resume envelope, validated by the Host.
    pub checkpoint: serde_json::Value,
    /// Cumulative logical-run usage, including this segment. Never add this snapshot twice.
    pub usage: Option<AgentUsageRecordInsert>,
    pub content: String,
    pub predecessor: Option<HumanInteractionSyncBinding>,
    pub pending_action_predecessor: Option<HumanInteractionApprovalPredecessor>,
}

/// Existing approval settlement evidence; this grants no approval or ToolResult authority.
#[derive(Debug, Clone)]
pub struct HumanInteractionApprovalPredecessor {
    pub storage_id: String,
    pub renderer_action_id: String,
    pub expected_status: String,
    pub terminal_status: String,
    pub terminal_agent_input_json: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HumanInteractionSyncBinding {
    pub request_id: String,
    pub response_id: String,
    pub conversation_id: String,
    pub run_id: String,
    pub assistant_message_id: String,
    pub tool_call_id: String,
    pub claim_id: String,
}

#[derive(Debug, Clone)]
pub struct HumanInteractionSyncResume {
    pub binding: HumanInteractionSyncBinding,
    pub checkpoint: serde_json::Value,
    pub request: HumanInteractionRequestSnapshot,
}

/// Questions, exact checkpoint, billed segment and visible waiting state share one commit.
/// An identical late admission only reads existing facts; it cannot rewind a submitted answer.
pub fn admit_sync(
    connection: &mut Connection,
    input: &HumanInteractionSyncAdmission,
    now: i64,
) -> Result<HumanInteractionRequestSnapshot> {
    valid_time(now)?;
    validate_human_interaction_tool_input(&input.input)?;
    let checkpoint = json(&input.checkpoint)?;
    if !input.checkpoint.is_object() || checkpoint.len() > 1_048_576 {
        return Err(HumanInteractionError::invalid());
    }
    let owner = &input.owner;
    if let Some(usage) = &input.usage {
        if usage.conversation_id != owner.conversation_id
            || usage.message_id != owner.assistant_message_id
            || usage.run_id != owner.run_id
            || usage.status.as_deref() != Some("waiting_for_user_input")
            || usage.completed_at.is_some()
        {
            return Err(HumanInteractionError::invalid());
        }
    }
    let tx = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(unavailable)?;
    validate_owner(&tx, owner)?;
    let existing: Option<(String, String)> = tx
        .query_row(
            "SELECT r.request_id,s.checkpoint_json FROM human_interaction_requests r
         JOIN human_interaction_suspensions s ON s.request_id=r.request_id
         WHERE r.run_id=?1 AND r.tool_call_id=?2 AND r.mode='sync'
           AND r.conversation_id=?3 AND r.assistant_message_id=?4 AND r.agent_id=?5",
            params![
                owner.run_id,
                owner.tool_call_id,
                owner.conversation_id,
                owner.assistant_message_id,
                owner.agent_id
            ],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(unavailable)?;
    if let Some((id, frozen)) = existing {
        let snapshot = load_request(&tx, &owner.conversation_id, &id)?;
        let original: Vec<_> = snapshot
            .questions
            .iter()
            .map(|q| HumanInteractionQuestionInput {
                title: q.title.clone(),
                options: q
                    .options
                    .as_ref()
                    .map(|options| options.iter().map(|o| o.label.clone()).collect()),
            })
            .collect();
        if original != input.input.questions
            || parse::<serde_json::Value>(&frozen)? != input.checkpoint
        {
            return Err(conflict());
        }
        tx.commit().map_err(unavailable)?;
        return Ok(snapshot);
    }
    if let Some(binding) = &input.predecessor {
        if binding.conversation_id != owner.conversation_id
            || binding.run_id != owner.run_id
            || binding.assistant_message_id != owner.assistant_message_id
        {
            return Err(conflict());
        }
        let changed = tx.execute("UPDATE human_interaction_suspensions SET status='applied',revision=revision+1,updated_at=MAX(updated_at,?1) WHERE request_id=?2 AND run_id=?3 AND assistant_message_id=?4 AND tool_call_id=?5 AND claim_id=?6 AND status IN ('executing','model_in_flight') AND EXISTS(SELECT 1 FROM human_interaction_responses a JOIN human_interaction_deliveries d ON d.response_id=a.response_id WHERE a.request_id=?2 AND a.response_id=?7 AND d.status='bound' AND d.target_run_id=?3)",params![now,binding.request_id,binding.run_id,binding.assistant_message_id,binding.tool_call_id,binding.claim_id,binding.response_id]).map_err(unavailable)?;
        if changed != 1 {
            return Err(conflict());
        }
        tx.execute("UPDATE human_interaction_deliveries SET status='applied',revision=revision+1 WHERE response_id=?1",[&binding.response_id]).map_err(unavailable)?;
    }
    if let Some(predecessor) = &input.pending_action_predecessor {
        settle_approval_predecessor(&tx, owner, predecessor, now)?;
    }
    // A paused approval or another unresolved synchronous response still owns this conversation.
    let occupied: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM human_interaction_suspensions s
         JOIN human_interaction_requests r ON r.request_id=s.request_id
         WHERE r.conversation_id=?1 AND s.status IN ('waiting','claimed','executing','model_in_flight'))
         OR EXISTS(SELECT 1 FROM agent_pending_actions WHERE conversation_id=?1 AND status IN ('pending','approved','executing'))",
        [&owner.conversation_id], |row| row.get(0),
    ).map_err(unavailable)?;
    if occupied {
        return Err(conflict());
    }
    let snapshot =
        create_request_in_transaction(&tx, owner, HumanInteractionMode::Sync, &input.input, now)?;
    tx.execute("INSERT INTO human_interaction_suspensions(request_id,run_id,assistant_message_id,tool_call_id,checkpoint_json,status,revision,created_at,updated_at)
        VALUES(?1,?2,?3,?4,?5,'waiting',0,?6,?6)",
        params![snapshot.request_id,owner.run_id,owner.assistant_message_id,owner.tool_call_id,checkpoint,now]).map_err(unavailable)?;
    if let Some(usage) = &input.usage {
        usage_repository::upsert_usage_record(&tx, usage).map_err(unavailable)?;
    }
    project_run(
        &tx,
        &owner.conversation_id,
        &owner.assistant_message_id,
        &owner.run_id,
        "waiting_for_user_input",
        now,
    )?;
    tx.execute(
        "UPDATE messages SET content=?1 WHERE id=?2 AND conversation_id=?3",
        params![
            input.content,
            owner.assistant_message_id,
            owner.conversation_id
        ],
    )
    .map_err(unavailable)?;
    tx.commit().map_err(unavailable)?;
    Ok(snapshot)
}

fn project_run(
    connection: &Connection,
    conversation: &str,
    message: &str,
    run: &str,
    status: &str,
    now: i64,
) -> Result<()> {
    let (existing, started): (Option<String>, i64) = connection.query_row("SELECT agent_run_json,created_at FROM messages WHERE id=?1 AND conversation_id=?2 AND role='assistant'", params![message,conversation], |r| Ok((r.get(0)?,r.get(1)?))).map_err(unavailable)?;
    let value = chat_repository::canonical_agent_run_lifecycle_projection(
        existing.as_deref(),
        run,
        status,
        started,
        now,
        None,
    )
    .map_err(unavailable)?;
    connection.execute("UPDATE messages SET status='pending',agent_run_json=?1 WHERE id=?2 AND conversation_id=?3", params![value,message,conversation]).map_err(unavailable)?;
    connection
        .execute(
            "UPDATE conversations SET updated_at=MAX(updated_at,?1) WHERE id=?2",
            params![now, conversation],
        )
        .map_err(unavailable)?;
    Ok(())
}

fn active_sync(connection: &Connection, request: &HumanInteractionRequestSnapshot) -> Result<bool> {
    connection.query_row("SELECT EXISTS(SELECT 1 FROM conversation_turn_traces WHERE conversation_id=?1 AND assistant_message_id=?2 AND run_id=?3 AND terminal_status='in_progress') AND NOT EXISTS(SELECT 1 FROM agent_tree_run_stops WHERE run_id=?3)",
        params![request.conversation_id,request.assistant_message_id,request.run_id], |r| r.get(0)).map_err(unavailable)
}

pub fn claim_sync(
    connection: &mut Connection,
    request_id: &str,
    now: i64,
) -> Result<Option<HumanInteractionSyncResume>> {
    validate_human_interaction_id(request_id)?;
    valid_time(now)?;
    let tx = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(unavailable)?;
    let row: Option<(String,String)> = tx.query_row("SELECT r.conversation_id,s.checkpoint_json FROM human_interaction_requests r JOIN human_interaction_suspensions s ON s.request_id=r.request_id JOIN human_interaction_responses a ON a.request_id=r.request_id JOIN human_interaction_deliveries d ON d.response_id=a.response_id WHERE r.request_id=?1 AND r.mode='sync' AND r.status='submitted' AND s.status='waiting' AND d.status='pending'", [request_id], |r| Ok((r.get(0)?,r.get(1)?))).optional().map_err(unavailable)?;
    let Some((conversation, checkpoint)) = row else {
        return Ok(None);
    };
    let request = load_request(&tx, &conversation, request_id)?;
    if !active_sync(&tx, &request)? {
        return Ok(None);
    }
    let response_id = request
        .response
        .as_ref()
        .ok_or_else(conflict)?
        .response_id
        .clone();
    let claim_id = Uuid::new_v4().to_string();
    tx.execute("UPDATE human_interaction_suspensions SET status='claimed',claim_id=?1,revision=revision+1,updated_at=MAX(updated_at,?2) WHERE request_id=?3 AND status='waiting'",params![claim_id,now,request_id]).map_err(unavailable)?;
    tx.execute("UPDATE human_interaction_deliveries SET status='bound',target_run_id=?1,revision=revision+1 WHERE response_id=?2 AND status='pending'",params![request.run_id,response_id]).map_err(unavailable)?;
    let binding = HumanInteractionSyncBinding {
        request_id: request_id.into(),
        response_id,
        conversation_id: conversation,
        run_id: request.run_id.clone(),
        assistant_message_id: request.assistant_message_id.clone(),
        tool_call_id: request.tool_call_id.clone(),
        claim_id,
    };
    let request = load_request(&tx, &binding.conversation_id, request_id)?;
    tx.commit().map_err(unavailable)?;
    Ok(Some(HumanInteractionSyncResume {
        binding,
        checkpoint: parse(&checkpoint)?,
        request,
    }))
}

/// This CAS is the last fence before Runtime is allowed to execute resumed tool effects.
pub fn advance_sync(
    connection: &mut Connection,
    binding: &HumanInteractionSyncBinding,
    transition: HumanInteractionSyncTransition,
    now: i64,
) -> Result<bool> {
    valid_time(now)?;
    let tx = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(unavailable)?;
    let request = load_request(&tx, &binding.conversation_id, &binding.request_id)?;
    if request.run_id != binding.run_id
        || request.assistant_message_id != binding.assistant_message_id
        || request.tool_call_id != binding.tool_call_id
        || request.response.as_ref().map(|r| &r.response_id) != Some(&binding.response_id)
    {
        return Ok(false);
    }
    if !(active_sync(&tx, &request)?
        || transition == HumanInteractionSyncTransition::Applied
            && has_terminal_consumption(&tx, &request)?)
    {
        return Ok(false);
    }
    let (from, to) = match transition {
        HumanInteractionSyncTransition::ExecutionStarted => ("claimed", "executing"),
        HumanInteractionSyncTransition::ModelInFlight => ("executing", "model_in_flight"),
        HumanInteractionSyncTransition::Applied => ("model_in_flight", "applied"),
    };
    let changed = tx.execute("UPDATE human_interaction_suspensions SET status=?1,revision=revision+1,updated_at=MAX(updated_at,?2) WHERE request_id=?3 AND claim_id=?4 AND (status=?5 OR (?1='applied' AND status='executing')) AND EXISTS(SELECT 1 FROM human_interaction_deliveries WHERE response_id=?6 AND status='bound' AND target_run_id=?7)",params![to,now,binding.request_id,binding.claim_id,from,binding.response_id,binding.run_id]).map_err(unavailable)?;
    if changed != 1 {
        return Ok(false);
    }
    if transition == HumanInteractionSyncTransition::ExecutionStarted {
        project_run(
            &tx,
            &binding.conversation_id,
            &binding.assistant_message_id,
            &binding.run_id,
            "running",
            now,
        )?;
        tx.execute("UPDATE agent_usage_records SET status='running' WHERE conversation_id=?1 AND message_id=?2 AND run_id=?3 AND status='waiting_for_user_input'",params![binding.conversation_id,binding.assistant_message_id,binding.run_id]).map_err(unavailable)?;
    }
    if transition == HumanInteractionSyncTransition::Applied {
        tx.execute("UPDATE human_interaction_deliveries SET status='applied',revision=revision+1 WHERE response_id=?1 AND status='bound'",[&binding.response_id]).map_err(unavailable)?;
    }
    tx.commit().map_err(unavailable)?;
    Ok(true)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HumanInteractionSyncTransition {
    ExecutionStarted,
    ModelInFlight,
    Applied,
}

pub fn cancel_sync_for_run(
    connection: &mut Connection,
    run_id: &str,
    now: i64,
) -> Result<Vec<HumanInteractionRequestSnapshot>> {
    validate_human_interaction_id(run_id)?;
    valid_time(now)?;
    let tx = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(unavailable)?;
    let ids = sync_request_ids(&tx, Some(run_id))?;
    for (_, id) in &ids {
        cancel_sync_in_transaction(&tx, id, now)?;
    }
    let result = ids
        .iter()
        .map(|(conversation, id)| load_request(&tx, conversation, id))
        .collect::<Result<Vec<_>>>()?;
    tx.commit().map_err(unavailable)?;
    Ok(result)
}

fn cancel_sync_in_transaction(connection: &Connection, request: &str, now: i64) -> Result<()> {
    connection.execute("UPDATE human_interaction_requests SET status='cancelled',revision=revision+1,updated_at=MAX(updated_at,?1) WHERE request_id=?2 AND status='open'",params![now,request]).map_err(unavailable)?;
    connection.execute("UPDATE human_interaction_deliveries SET status='cancelled',revision=revision+1,error_code='run_cancelled' WHERE response_id IN (SELECT response_id FROM human_interaction_responses WHERE request_id=?1) AND status IN ('pending','bound')",[request]).map_err(unavailable)?;
    connection.execute("UPDATE human_interaction_suspensions SET status='cancelled',revision=revision+1,updated_at=MAX(updated_at,?1) WHERE request_id=?2 AND status IN ('waiting','claimed','executing','model_in_flight')",params![now,request]).map_err(unavailable)?;
    Ok(())
}

fn sync_request_ids(connection: &Connection, run: Option<&str>) -> Result<Vec<(String, String)>> {
    let mut statement = connection.prepare("SELECT r.conversation_id,r.request_id FROM human_interaction_requests r JOIN human_interaction_suspensions s ON s.request_id=r.request_id WHERE r.mode='sync' AND (?1 IS NULL OR r.run_id=?1) AND s.status IN ('waiting','claimed','executing','model_in_flight') ORDER BY r.sequence").map_err(unavailable)?;
    let result = statement
        .query_map([run], |r| Ok((r.get(0)?, r.get(1)?)))
        .map_err(unavailable)?
        .collect::<rusqlite::Result<_>>()
        .map_err(unavailable);
    result
}

/// Called once after acquiring the process lock, before orphan-trace reconciliation.
/// Claimed but unexecuted work is safe to retry. Any started execution has unknown effects and
/// fails closed; its immutable accepted answer remains visible with an explicit delivery error.
pub fn reconcile_sync(
    connection: &mut Connection,
    now: i64,
) -> Result<Vec<HumanInteractionRequestSnapshot>> {
    valid_time(now)?;
    let tx = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(unavailable)?;
    let ids = sync_request_ids(&tx, None)?;
    for (conversation, id) in &ids {
        let request = load_request(&tx, conversation, id)?;
        if has_terminal_consumption(&tx, &request)? {
            tx.execute("UPDATE human_interaction_suspensions SET status='applied',revision=revision+1,updated_at=MAX(updated_at,?1) WHERE request_id=?2 AND status IN ('executing','model_in_flight')",params![now,id]).map_err(unavailable)?;
            tx.execute("UPDATE human_interaction_deliveries SET status='applied',revision=revision+1 WHERE response_id=(SELECT response_id FROM human_interaction_responses WHERE request_id=?1) AND status='bound'",[id]).map_err(unavailable)?;
            continue;
        }
        if !active_sync(&tx, &request)? {
            cancel_sync_in_transaction(&tx, id, now)?;
            continue;
        }
        let status: String = tx
            .query_row(
                "SELECT status FROM human_interaction_suspensions WHERE request_id=?1",
                [id],
                |r| r.get(0),
            )
            .map_err(unavailable)?;
        if status == "claimed" {
            tx.execute("UPDATE human_interaction_suspensions SET status='waiting',claim_id=NULL,revision=revision+1,updated_at=MAX(updated_at,?1) WHERE request_id=?2",params![now,id]).map_err(unavailable)?;
            tx.execute("UPDATE human_interaction_deliveries SET status='pending',target_run_id=NULL,revision=revision+1 WHERE response_id=(SELECT response_id FROM human_interaction_responses WHERE request_id=?1) AND status='bound'",[id]).map_err(unavailable)?;
        } else if status == "executing" && has_approval_handoff(&tx, &request)? {
            tx.execute("UPDATE human_interaction_suspensions SET status='applied',revision=revision+1,updated_at=MAX(updated_at,?1) WHERE request_id=?2",params![now,id]).map_err(unavailable)?;
            tx.execute("UPDATE human_interaction_deliveries SET status='applied',revision=revision+1 WHERE response_id=(SELECT response_id FROM human_interaction_responses WHERE request_id=?1) AND status='bound'",[id]).map_err(unavailable)?;
        } else if status == "executing" || status == "model_in_flight" {
            tx.execute("UPDATE human_interaction_suspensions SET status='failed',revision=revision+1,updated_at=MAX(updated_at,?1) WHERE request_id=?2",params![now,id]).map_err(unavailable)?;
            tx.execute("UPDATE human_interaction_deliveries SET status='failed',error_code='resume_outcome_unknown',revision=revision+1 WHERE response_id=(SELECT response_id FROM human_interaction_responses WHERE request_id=?1) AND status='bound'",[id]).map_err(unavailable)?;
        }
    }
    let result = ids
        .iter()
        .map(|(conversation, id)| load_request(&tx, conversation, id))
        .collect::<Result<Vec<_>>>()?;
    tx.commit().map_err(unavailable)?;
    Ok(result)
}

/// Durable occupancy includes submitted answers awaiting dispatch; absence of a worker is not idle.
pub fn has_sync_wait(connection: &Connection, conversation: &str) -> Result<bool> {
    connection.query_row("SELECT EXISTS(SELECT 1 FROM human_interaction_suspensions s JOIN human_interaction_requests r ON r.request_id=s.request_id WHERE r.conversation_id=?1 AND s.status IN ('waiting','claimed','executing','model_in_flight'))",[conversation],|r| r.get(0)).map_err(unavailable)
}

/// Unclaimed submitted answers, in immutable Host response-admission order.
pub fn list_sync_ready(connection: &Connection) -> Result<Vec<HumanInteractionRequestSnapshot>> {
    let mut statement = connection.prepare("SELECT r.conversation_id,r.request_id FROM human_interaction_requests r JOIN human_interaction_suspensions s ON s.request_id=r.request_id JOIN human_interaction_responses a ON a.request_id=r.request_id JOIN human_interaction_deliveries d ON d.response_id=a.response_id WHERE r.mode='sync' AND r.status='submitted' AND s.status='waiting' AND d.status='pending' ORDER BY a.sequence").map_err(unavailable)?;
    let ids = statement
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .map_err(unavailable)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(unavailable)?;
    ids.iter()
        .map(|(conversation, id)| load_request(connection, conversation, id))
        .collect()
}

pub fn is_sync_run_waiting(connection: &Connection, run: &str) -> Result<bool> {
    connection.query_row("SELECT EXISTS(SELECT 1 FROM human_interaction_suspensions WHERE run_id=?1 AND status IN ('waiting','claimed'))",[run],|r| r.get(0)).map_err(unavailable)
}

fn settle_approval_predecessor(
    connection: &Connection,
    owner: &HostHumanInteractionOwner,
    evidence: &HumanInteractionApprovalPredecessor,
    now: i64,
) -> Result<()> {
    use crate::storage::{notification_repository, pending_action_repository};
    if !matches!(
        evidence.terminal_status.as_str(),
        "rejected" | "cancelled" | "completed" | "failed"
    ) {
        return Err(conflict());
    }
    let record = pending_action_repository::load_pending_action(connection, &evidence.storage_id)
        .map_err(unavailable)?
        .ok_or_else(conflict)?;
    if record.run_id != owner.run_id
        || record.conversation_id.as_ref() != Some(&owner.conversation_id)
        || record.assistant_message_id.as_ref() != Some(&owner.assistant_message_id)
        || record.target_status.as_ref() != Some(&evidence.terminal_status)
    {
        return Err(conflict());
    }
    if record.status != evidence.terminal_status {
        if record.status != evidence.expected_status {
            return Err(conflict());
        }
        let changed = pending_action_repository::transition_pending_action(
            connection,
            &evidence.storage_id,
            &evidence.expected_status,
            &evidence.terminal_status,
            &evidence.terminal_agent_input_json,
            now,
        )
        .map_err(unavailable)?;
        if changed != 1 {
            return Err(conflict());
        }
    }
    notification_repository::resolve_notification_events_by_run_and_approval_action_id_in_transaction(connection,&owner.run_id,&evidence.renderer_action_id,now).map_err(unavailable)?;
    Ok(())
}

fn has_approval_handoff(
    connection: &Connection,
    request: &HumanInteractionRequestSnapshot,
) -> Result<bool> {
    if request.response.is_none() {
        return Ok(false);
    }
    let mut statement = connection.prepare("SELECT agent_input_json FROM agent_pending_actions WHERE run_id=?1 AND conversation_id=?2 AND assistant_message_id=?3 AND status IN ('pending','approved','executing') AND target_status IS NULL").map_err(unavailable)?;
    let envelopes = statement
        .query_map(
            params![
                request.run_id,
                request.conversation_id,
                request.assistant_message_id
            ],
            |r| r.get::<_, String>(0),
        )
        .map_err(unavailable)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(unavailable)?;
    for envelope in envelopes {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&envelope) else {
            continue;
        };
        let Some(checkpoint) = value.get("resumeCheckpoint") else {
            continue;
        };
        let Ok(checkpoint) =
            serde_json::from_value::<crate::AgentRunCheckpoint>(checkpoint.clone())
        else {
            continue;
        };
        if checkpoint.version != crate::AGENT_RUN_CHECKPOINT_SCHEMA_VERSION
            || checkpoint.run_id != request.run_id
            || checkpoint.pause_reason != crate::AgentRunCheckpointPauseReason::Approval
        {
            continue;
        }
        if has_exact_answer_projection(
            request,
            &checkpoint.conversation_trace_items,
            &checkpoint.conversation_model_context_items,
        )? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn has_terminal_consumption(
    connection: &Connection,
    request: &HumanInteractionRequestSnapshot,
) -> Result<bool> {
    if request.response.is_none() {
        return Ok(false);
    }
    let stopped: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM agent_tree_run_stops WHERE run_id=?1)",
            [&request.run_id],
            |r| r.get(0),
        )
        .map_err(unavailable)?;
    if stopped {
        return Ok(false);
    }
    let terminal: bool = connection.query_row("SELECT EXISTS(SELECT 1 FROM conversation_turn_traces WHERE run_id=?1 AND assistant_message_id=?2 AND conversation_id=?3 AND terminal_status IN ('completed','failed')) AND EXISTS(SELECT 1 FROM human_interaction_suspensions WHERE request_id=?4 AND status IN ('executing','model_in_flight'))",params![request.run_id,request.assistant_message_id,request.conversation_id,request.request_id],|r| r.get(0)).map_err(unavailable)?;
    if !terminal {
        return Ok(false);
    }
    let Some(trace) = crate::storage::conversation_trace_repository::get_trace_for_message(
        connection,
        &request.assistant_message_id,
    )
    .map_err(unavailable)?
    else {
        return Ok(false);
    };
    if trace.run_id != request.run_id
        || trace.conversation_id != request.conversation_id
        || !matches!(
            trace.terminal_status,
            crate::ConversationTurnTraceTerminalStatus::Completed
                | crate::ConversationTurnTraceTerminalStatus::Failed
        )
    {
        return Ok(false);
    }
    let Some(context) = crate::storage::conversation_model_context_repository::get_log_for_message(
        connection,
        &request.assistant_message_id,
    )
    .map_err(unavailable)?
    else {
        return Ok(false);
    };
    has_exact_answer_projection(request, &trace.items, &context.items)
}

/// Private immutable checkpoints for cancellation and startup; this never claims execution.
pub fn list_sync_waits(
    connection: &Connection,
) -> Result<Vec<(HumanInteractionRequestSnapshot, serde_json::Value)>> {
    let ids = sync_request_ids(connection, None)?;
    ids.iter()
        .map(|(conversation, id)| {
            let snapshot = load_request(connection, conversation, id)?;
            let checkpoint: String = connection
                .query_row(
                    "SELECT checkpoint_json FROM human_interaction_suspensions WHERE request_id=?1",
                    [id],
                    |r| r.get(0),
                )
                .map_err(unavailable)?;
            Ok((snapshot, parse(&checkpoint)?))
        })
        .collect()
}

pub fn load_sync_for_run(
    connection: &Connection,
    run: &str,
) -> Result<Option<(HumanInteractionRequestSnapshot, serde_json::Value)>> {
    validate_human_interaction_id(run)?;
    let row = connection.query_row("SELECT r.conversation_id,r.request_id,s.checkpoint_json FROM human_interaction_requests r JOIN human_interaction_suspensions s ON s.request_id=r.request_id WHERE r.run_id=?1 AND r.mode='sync' ORDER BY r.sequence DESC LIMIT 1",[run],|r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?))).optional().map_err(unavailable)?;
    row.map(|(conversation, id, checkpoint)| {
        Ok((
            load_request(connection, &conversation, &id)?,
            parse(&checkpoint)?,
        ))
    })
    .transpose()
}

/// A matching ID cannot certify delivery of a different or missing answer. Recovery requires the
/// full frozen response in both the durable ToolResult and its replay-safe model projection.
fn has_exact_answer_projection(
    request: &HumanInteractionRequestSnapshot,
    trace: &[crate::ConversationTurnTraceItem],
    context: &[crate::ConversationModelContextItem],
) -> Result<bool> {
    let expected =
        serde_json::to_value(human_interaction_answer_display(request)?).map_err(unavailable)?;
    Ok(trace.iter().any(|item| {
        let crate::ConversationTurnTraceItem::ToolResult {
            sequence,
            call_id,
            tool,
            status,
            success,
            observation,
            truncated,
            ..
        } = item
        else {
            return false;
        };
        call_id == &request.tool_call_id
            && tool == "request_user_input"
            && *success
            && *status == crate::ConversationTraceToolResultStatus::Succeeded
            && !truncated
            && observation == &expected
            && context.iter().any(|message| {
                message.sequence == *sequence
                    && message.role == "tool"
                    && !message.is_error
                    && message.tool_call_id.as_deref() == Some(request.tool_call_id.as_str())
                    && serde_json::from_str::<serde_json::Value>(&message.content)
                        .ok()
                        .as_ref()
                        == Some(&expected)
            })
    }))
}
