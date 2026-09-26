use super::*;

pub fn load_input(c: &Connection, input_id: &str) -> Result<Option<Input>, String> {
    c.query_row(
        "SELECT input_json FROM workflow_execution_inputs WHERE input_id=?1",
        [input_id],
        |r| r.get::<_, String>(0),
    )
    .optional()
    .map_err(db)?
    .map(|v| parse(&v))
    .transpose()
}
pub fn input_for_delivery(c: &Connection, delivery_id: &str) -> Result<Option<Input>, String> {
    c.query_row(
        "SELECT input_json FROM workflow_execution_inputs WHERE delivery_id=?1",
        [delivery_id],
        |r| r.get::<_, String>(0),
    )
    .optional()
    .map_err(db)?
    .map(|v| parse(&v))
    .transpose()
}
fn list(c: &Connection, query: &str, param: &str) -> Result<Vec<Input>, String> {
    let mut statement = c.prepare(query).map_err(db)?;
    let rows = statement
        .query_map([param], |r| r.get::<_, String>(0))
        .map_err(db)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(db)?;
    rows.iter().map(|row| parse(row)).collect()
}
pub fn inputs_for_conversation(
    c: &Connection,
    conversation_id: &str,
) -> Result<Vec<Input>, String> {
    list(c,"SELECT input_json FROM workflow_execution_inputs WHERE conversation_id=?1 ORDER BY sequence",conversation_id)
}
fn eligible(c: &Connection, input: &Input) -> Result<bool, String> {
    let Some(graph) = graph(c, &input.instance_id)? else {
        return Ok(false);
    };
    if !graph.enabled || graph.execution_version != input.execution_version {
        return Ok(false);
    }
    if let Some(chat) = input.conversation_id.as_deref() {
        if graph.bindings.get(&input.node_id).map(String::as_str) != Some(chat)
            || !independent(c, chat)?
        {
            return Ok(false);
        }
        let paused: bool = c
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM workflow_execution_pauses WHERE conversation_id=?1)",
                [chat],
                |r| r.get(0),
            )
            .map_err(db)?;
        if paused {
            return Ok(false);
        }
    }
    Ok(true)
}
/// Only the oldest ready input per conversation is offered. A claim blocks later inputs
/// until durable model receipt, making worker races and batched FIFO deterministic.
pub fn pending_inputs(c: &Connection) -> Result<Vec<Input>, String> {
    let mut statement=c.prepare("SELECT input_json FROM workflow_execution_inputs i WHERE status='pending' AND NOT EXISTS(SELECT 1 FROM workflow_execution_inputs older WHERE older.conversation_id=i.conversation_id AND older.status IN ('pending','claimed','paused','failed') AND older.sequence<i.sequence) ORDER BY sequence LIMIT 128").map_err(db)?;
    let rows = statement
        .query_map([], |r| r.get::<_, String>(0))
        .map_err(db)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(db)?;
    let mut result = vec![];
    for row in rows {
        let input: Input = parse(&row)?;
        if eligible(c, &input)? {
            result.push(input);
        }
    }
    Ok(result)
}
pub fn bound_inputs(c: &Connection, run_id: &str) -> Result<Vec<Input>, String> {
    let inputs=list(c,"SELECT input_json FROM workflow_execution_inputs WHERE run_id=?1 AND status='claimed' ORDER BY sequence",run_id)?;
    let mut result = vec![];
    for input in inputs {
        if eligible(c, &input)? {
            result.push(input);
        }
    }
    Ok(result)
}
fn write(c: &Connection, input: &Input) -> Result<(), String> {
    c.execute("UPDATE workflow_execution_inputs SET input_json=?1,status=?2,run_id=?3,delivery_id=?4,updated_at=?5 WHERE input_id=?6",params![json(input)?,status_name(&input.status),input.run_id,input.delivery_id,now_ms(),input.id]).map_err(db)?;
    Ok(())
}
/// Caller can include this transition in a larger admission transaction. No external effects
/// may happen before this succeeds. An uncertain claimed delivery is never automatically retried.
pub fn bind_input_in_connection(
    c: &Connection,
    input_id: &str,
    run_id: &str,
    delivery_id: &str,
) -> Result<bool, String> {
    let Some(mut input) = load_input(c, input_id)? else {
        return Ok(false);
    };
    if input.status == InputStatus::Claimed
        && input.run_id.as_deref() == Some(run_id)
        && input.delivery_id.as_deref() == Some(delivery_id)
    {
        return eligible(c, &input);
    }
    if input.status != InputStatus::Pending
        || !eligible(c, &input)?
        || run_id.is_empty()
        || delivery_id.is_empty()
    {
        return Ok(false);
    }
    let blocked:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM workflow_execution_inputs older WHERE older.conversation_id=?1 AND older.status IN ('pending','claimed','paused','failed') AND older.sequence<(SELECT sequence FROM workflow_execution_inputs WHERE input_id=?2))",params![input.conversation_id,input.id],|r|r.get(0)).map_err(db)?;
    if blocked {
        return Ok(false);
    }
    c.execute("INSERT INTO workflow_execution_message_origins(message_id,conversation_id,input_id) VALUES (?1,?2,?3)",params![delivery_id,input.conversation_id,input.id]).map_err(db)?;
    input.status = InputStatus::Claimed;
    input.run_id = Some(run_id.into());
    input.delivery_id = Some(delivery_id.into());
    write(c, &input)?;
    Ok(true)
}
pub fn bind_input(
    c: &mut Connection,
    input_id: &str,
    run_id: &str,
    delivery_id: &str,
) -> Result<bool, String> {
    let tx = c
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db)?;
    let bound = bind_input_in_connection(&tx, input_id, run_id, delivery_id)?;
    tx.commit().map_err(db)?;
    Ok(bound)
}
fn has_delivery_proof(c: &Connection, input: &Input) -> Result<bool, String> {
    c.query_row("SELECT EXISTS(SELECT 1 FROM conversation_turn_trace_items i JOIN conversation_turn_traces t ON t.assistant_message_id=i.assistant_message_id WHERE t.run_id=?1 AND t.conversation_id=?2 AND i.item_kind='workflow_delivery' AND json_extract(i.item_json,'$.inputId')=?3 AND json_extract(i.item_json,'$.instanceId')=?4 AND json_extract(i.item_json,'$.content')=?5)",params![input.run_id,input.conversation_id,input.id,input.instance_id,input.content],|r|r.get(0)).map_err(db)
}
fn apply_proven_input(c: &Connection, input: &mut Input) -> Result<(), String> {
    input.status = InputStatus::Applied;
    input.error = None;
    write(c, input)?;
    if let Some(conversation_id) = &input.conversation_id {
        c.execute(
            "UPDATE conversations SET unread_at=?1,updated_at=MAX(updated_at,?1) WHERE id=?2",
            params![now_ms(), conversation_id],
        )
        .map_err(db)?;
    }
    let flows = graph(c, &input.instance_id)?
        .map(|g| final_gate_flows(&g, input))
        .unwrap_or_default();
    event(c, &input.instance_id, Some(&input.id), &flows, "delivered")
}
pub fn mark_applied(c: &mut Connection, input_id: &str) -> Result<(), String> {
    let tx = c
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db)?;
    let Some(mut input) = load_input(&tx, input_id)? else {
        return Err("Workflow input no longer exists".into());
    };
    if input.status == InputStatus::Applied {
        return Ok(());
    }
    if !has_delivery_proof(&tx, &input)? {
        return Err("Workflow input has no durable delivery receipt".into());
    }
    // The trace observer calls this only after the model receipt was persisted. A disable
    // racing that receipt cannot erase the fact that this input has already been applied.
    apply_proven_input(&tx, &mut input)?;
    tx.commit().map_err(db)
}
pub fn fail_input(c: &mut Connection, input_id: &str, reason: &str) -> Result<(), String> {
    let tx = c
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db)?;
    let Some(mut input) = load_input(&tx, input_id)? else {
        return Err("Workflow input no longer exists".into());
    };
    if matches!(
        input.status,
        InputStatus::Applied
            | InputStatus::Completed
            | InputStatus::Invalidated
            | InputStatus::Failed
    ) {
        return Ok(());
    }
    input.status = InputStatus::Failed;
    input.error = Some(reason.chars().take(2000).collect());
    write(&tx, &input)?;
    event(&tx, &input.instance_id, Some(input_id), &[], "failed")?;
    tx.commit().map_err(db)
}
pub fn pause_conversation(c: &mut Connection, conversation_id: &str) -> Result<(), String> {
    let tx = c
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db)?;
    tx.execute("INSERT INTO workflow_execution_pauses(conversation_id,created_at) VALUES (?1,?2) ON CONFLICT(conversation_id) DO NOTHING",params![conversation_id,now_ms()]).map_err(db)?;
    for mut input in list(&tx,"SELECT input_json FROM workflow_execution_inputs WHERE conversation_id=?1 AND status IN ('pending','claimed')",conversation_id)? {
        if input.status==InputStatus::Claimed {
            if has_delivery_proof(&tx,&input)? {apply_proven_input(&tx,&mut input)?;continue;} else {input.status=InputStatus::Failed;input.error=Some("Conversation stopped before the workflow input was durably applied".into());}
        }else{input.status=InputStatus::Paused;}
        write(&tx,&input)?;
    }
    tx.commit().map_err(db)
}
pub fn resume_conversation(c: &mut Connection, conversation_id: &str) -> Result<(), String> {
    let tx = c
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db)?;
    tx.execute(
        "DELETE FROM workflow_execution_pauses WHERE conversation_id=?1",
        [conversation_id],
    )
    .map_err(db)?;
    for mut input in list(&tx,"SELECT input_json FROM workflow_execution_inputs WHERE conversation_id=?1 AND status='failed'",conversation_id)? {
        if input.error.as_deref()==Some("Conversation stopped before the workflow input was durably applied") {
            input.status=InputStatus::Invalidated;write(&tx,&input)?;
        }
    }
    for mut input in list(&tx,"SELECT input_json FROM workflow_execution_inputs WHERE conversation_id=?1 AND status='paused'",conversation_id)? {input.status=InputStatus::Pending;write(&tx,&input)?;}
    tx.commit().map_err(db)
}
/// Called when graph or bindings are replaced, never for a plain enable/disable switch.
/// Old queued inputs remain inspectable and cannot migrate to a new destination.
pub fn invalidate_instance(c: &Connection, instance_id: &str, reason: &str) -> Result<(), String> {
    for mut input in list(c,"SELECT input_json FROM workflow_execution_inputs WHERE instance_id=?1 AND status IN ('pending','claimed','paused','waiting_user','failed')",instance_id)? {
        input.status=InputStatus::Invalidated;input.error=Some(reason.into());write(c,&input)?;
    }
    c.execute("UPDATE workflow_execution_messages SET invalidated=1 WHERE instance_id=?1 AND input_id IS NULL",[instance_id]).map_err(db)?;
    Ok(())
}
pub fn complete_user_input(c: &mut Connection, input_id: &str) -> Result<(), String> {
    let tx = c
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db)?;
    let Some(mut input) = load_input(&tx, input_id)? else {
        return Err("Workflow input no longer exists".into());
    };
    if input.status == InputStatus::Completed {
        return Ok(());
    }
    if input.status != InputStatus::WaitingUser || input.conversation_id.is_some() {
        return Err("This input is not waiting for user completion".into());
    }
    if !eligible(&tx, &input)? {
        return Err("Enable and review the workflow before completing this input".into());
    }
    input.status = InputStatus::Completed;
    write(&tx, &input)?;
    event(&tx, &input.instance_id, Some(input_id), &[], "completed")?;
    tx.commit().map_err(db)
}
pub fn runtime_snapshot(
    c: &Connection,
    instance_id: &str,
    after_sequence: Option<u64>,
) -> Result<RuntimeSnapshot, String> {
    let exists: bool = c
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM workflow_instances WHERE instance_id=?1)",
            [instance_id],
            |r| r.get(0),
        )
        .map_err(db)?;
    if !exists {
        return Err("Workflow instance no longer exists".into());
    }
    let sequence: u64 = c
        .query_row(
            "SELECT COALESCE(MAX(sequence),0) FROM workflow_execution_events WHERE instance_id=?1",
            [instance_id],
            |r| r.get(0),
        )
        .map_err(db)?;
    // All outstanding work must remain visible even after many newer deliveries. Only the
    // historical tail is capped. The monitor is a lightweight view, not a message-body API.
    let mut inputs=list(c,"SELECT input_json FROM workflow_execution_inputs WHERE instance_id=?1 AND (status IN ('pending','claimed','paused','waiting_user','failed') OR sequence IN (SELECT sequence FROM workflow_execution_inputs WHERE instance_id=?1 ORDER BY sequence DESC LIMIT 128) OR input_id IN (SELECT input_id FROM workflow_execution_events WHERE instance_id=?1 ORDER BY sequence DESC LIMIT 512)) ORDER BY sequence",instance_id)?;
    for input in &mut inputs {
        input.content.clear();
        for message in &mut input.messages {
            message.content.clear();
        }
    }

    let mut statement=c.prepare("SELECT sequence,input_id,flow_ids_json,kind,created_at FROM workflow_execution_events WHERE instance_id=?1 AND sequence>?2 ORDER BY sequence DESC LIMIT 512").map_err(db)?;
    let rows = statement
        .query_map(params![instance_id, after_sequence.unwrap_or(0)], |r| {
            Ok((
                r.get::<_, u64>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, i64>(4)?,
            ))
        })
        .map_err(db)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(db)?;
    let mut events = vec![];
    for (sequence, input_id, flows, kind, created_at) in rows {
        events.push(Event {
            sequence,
            instance_id: instance_id.into(),
            input_id,
            flow_ids: parse(&flows)?,
            kind,
            created_at,
        });
    }
    events.reverse();
    Ok(RuntimeSnapshot {
        instance_id: instance_id.into(),
        sequence,
        inputs,
        events,
    })
}

/// Freeze even an absent identity at run admission: enabling or rebinding mid-turn cannot
/// grant an old task new cross-conversation authority.
pub fn bind_run(
    c: &mut Connection,
    conversation_id: &str,
    run_id: &str,
) -> Result<Option<ConversationSnapshot>, String> {
    let tx = c
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db)?;
    let existing: Option<(String, String)> = tx
        .query_row(
            "SELECT conversation_id,snapshot_json FROM workflow_execution_runs WHERE run_id=?1",
            [run_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(db)?;
    let live = snapshot_for_conversation(&tx, conversation_id)?;
    let snapshot = if let Some((owner, stored)) = existing {
        if owner != conversation_id {
            return Err("Workflow run belongs to another conversation".into());
        }
        let frozen: Option<ConversationSnapshot> = parse(&stored)?;
        frozen.filter(|old| {
            live.as_ref().is_some_and(|current| {
                current.execution_version == old.execution_version
                    && current.node_id == old.node_id
                    && current.instance_id == old.instance_id
            })
        })
    } else {
        tx.execute("INSERT INTO workflow_execution_runs(run_id,conversation_id,snapshot_json,created_at) VALUES (?1,?2,?3,?4)",params![run_id,conversation_id,json(&live)?,now_ms()]).map_err(db)?;
        live
    };
    tx.commit().map_err(db)?;
    Ok(snapshot)
}

/// Recover only settled runs. In-progress traces may resume an approval/checkpoint and retain
/// their claim. Missing proof after a terminal run is ambiguous and never causes a replay.
pub fn recover_claims(c: &mut Connection) -> Result<(), String> {
    let tx = c
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db)?;
    let inputs = list(
        &tx,
        "SELECT input_json FROM workflow_execution_inputs WHERE status=?1 ORDER BY sequence",
        "claimed",
    )?;
    for mut input in inputs {
        if has_delivery_proof(&tx, &input)? {
            apply_proven_input(&tx, &mut input)?;
            continue;
        }
        let active:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM conversation_turn_traces WHERE run_id=?1 AND conversation_id=?2 AND terminal_status='in_progress')",params![input.run_id,input.conversation_id],|r|r.get(0)).map_err(db)?;
        if !active {
            input.status = InputStatus::Failed;
            input.error=Some("Delivery outcome is unknown after interruption; it will not be retried automatically".into());
            write(&tx, &input)?;
            event(&tx, &input.instance_id, Some(&input.id), &[], "failed")?;
        }
    }
    tx.commit().map_err(db)
}

/// Read-only check for a previously admitted run. Legacy/restored runs without a receipt
/// do not gain an identity merely because a workflow was enabled later.
pub fn snapshot_for_run(
    c: &Connection,
    conversation_id: &str,
    run_id: &str,
) -> Result<Option<ConversationSnapshot>, String> {
    let stored:Option<String>=c.query_row("SELECT snapshot_json FROM workflow_execution_runs WHERE run_id=?1 AND conversation_id=?2",params![run_id,conversation_id],|r|r.get(0)).optional().map_err(db)?;
    let frozen: Option<ConversationSnapshot> = stored.as_deref().map(parse).transpose()?.flatten();
    let live = snapshot_for_conversation(c, conversation_id)?;
    Ok(frozen.filter(|old| {
        live.as_ref().is_some_and(|current| {
            current.execution_version == old.execution_version
                && current.node_id == old.node_id
                && current.instance_id == old.instance_id
        })
    }))
}
/// Returns chat-projection IDs including inherited fork bubbles. The durable Input remains
/// owned by the original workflow; only the historical message projection is copied.
pub fn delivery_origins_for_conversation(
    c: &Connection,
    conversation_id: &str,
) -> Result<Vec<(String, Input)>, String> {
    let mut statement=c.prepare("SELECT o.message_id,i.input_json FROM workflow_execution_message_origins o JOIN workflow_execution_inputs i ON i.input_id=o.input_id WHERE o.conversation_id=?1 ORDER BY o.message_id").map_err(db)?;
    let rows = statement
        .query_map([conversation_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })
        .map_err(db)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(db)?;
    rows.into_iter()
        .map(|(id, input)| Ok((id, parse(&input)?)))
        .collect()
}

/// The receipt flag survives event-log compaction, so a terminal callback replay cannot
/// resurrect unread state after the user already read the completed conversation.
pub fn mark_run_unread(c: &mut Connection, run_id: &str) -> Result<(), String> {
    let tx = c
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db)?;
    let inputs = list(&tx,"SELECT input_json FROM workflow_execution_inputs WHERE run_id=?1 AND status='applied' AND completion_notified=0 ORDER BY sequence",run_id)?;
    let mut conversations = BTreeSet::new();
    for input in inputs {
        tx.execute(
            "UPDATE workflow_execution_inputs SET completion_notified=1 WHERE input_id=?1",
            [&input.id],
        )
        .map_err(db)?;
        if let Some(conversation_id) = input.conversation_id {
            conversations.insert(conversation_id);
        }
        event(
            &tx,
            &input.instance_id,
            Some(&input.id),
            &[],
            "run_completed",
        )?;
    }
    for conversation_id in conversations {
        tx.execute(
            "UPDATE conversations SET unread_at=?1,updated_at=MAX(updated_at,?1) WHERE id=?2",
            params![now_ms(), conversation_id],
        )
        .map_err(db)?;
    }
    tx.commit().map_err(db)
}

/// Explicitly abandon an ambiguous/failed input without replaying it. This releases FIFO
/// successors while retaining the original failure and its authenticated message provenance.
pub fn discard_failed(c: &mut Connection, input_id: &str) -> Result<(), String> {
    let tx = c
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db)?;
    let Some(mut input) = load_input(&tx, input_id)? else {
        return Err("Workflow input no longer exists".into());
    };
    let discarded: bool = tx
        .query_row(
            "SELECT discarded FROM workflow_execution_inputs WHERE input_id=?1",
            [input_id],
            |r| r.get(0),
        )
        .map_err(db)?;
    if discarded {
        return Ok(());
    }
    if input.status != InputStatus::Failed {
        return Err("Only a failed workflow input can be skipped".into());
    }
    input.status = InputStatus::Invalidated;
    write(&tx, &input)?;
    tx.execute(
        "UPDATE workflow_execution_inputs SET discarded=1 WHERE input_id=?1",
        [input_id],
    )
    .map_err(db)?;
    event(&tx, &input.instance_id, Some(input_id), &[], "discarded")?;
    tx.commit().map_err(db)
}
