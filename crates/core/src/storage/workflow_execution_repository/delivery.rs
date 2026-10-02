use super::*;
const UNSETTLED_INPUTS_SQL: &str = "SELECT i.input_json FROM workflow_mail_messages m JOIN workflow_mail_inputs i ON i.input_id=m.input_id WHERE m.instance_id=?1 AND m.mail_status IN('pending','processing') ORDER BY i.sequence";
pub(super) const PENDING_CANDIDATES_SQL: &str = "SELECT i.input_id,i.sequence,i.instance_id,i.execution_version,i.conversation_id FROM workflow_mail_inputs i INDEXED BY workflow_mail_input_pending_sequence JOIN workflow_mail_messages m ON m.input_id=i.input_id WHERE i.status='pending' AND m.mail_status='pending' AND i.sequence>?1 AND NOT EXISTS(SELECT 1 FROM workflow_mail_inputs older JOIN workflow_mail_messages om ON om.input_id=older.input_id WHERE older.conversation_id=i.conversation_id AND om.mail_status='pending' AND older.sequence<i.sequence) ORDER BY i.sequence LIMIT ?2";

pub fn load_input(c: &Connection, input_id: &str) -> Result<Option<Input>, String> {
    c.query_row(
        "SELECT input_json FROM workflow_mail_inputs WHERE input_id=?1",
        [input_id],
        |r| r.get::<_, String>(0),
    )
    .optional()
    .map_err(db)?
    .map(|raw| parse(&raw))
    .transpose()
}
pub fn input_for_delivery(c: &Connection, delivery_id: &str) -> Result<Option<Input>, String> {
    c.query_row(
        "SELECT input_json FROM workflow_mail_inputs WHERE delivery_id=?1",
        [delivery_id],
        |r| r.get::<_, String>(0),
    )
    .optional()
    .map_err(db)?
    .map(|raw| parse(&raw))
    .transpose()
}
fn list(c: &Connection, query: &str, param: &str) -> Result<Vec<Input>, String> {
    let mut s = c.prepare(query).map_err(db)?;
    let result = s
        .query_map([param], |r| r.get::<_, String>(0))
        .map_err(db)?
        .map(|raw| parse(&raw.map_err(db)?))
        .collect();
    result
}
pub fn inputs_for_conversation(
    c: &Connection,
    conversation_id: &str,
) -> Result<Vec<Input>, String> {
    list(
        c,
        "SELECT input_json FROM workflow_mail_inputs WHERE conversation_id=?1 ORDER BY sequence",
        conversation_id,
    )
}
fn eligible(c: &Connection, input: &Input) -> Result<bool, String> {
    let Some(graph) = graph(c, &input.instance_id)? else {
        return Ok(false);
    };
    if !graph.enabled || !graph.definition.nodes.iter().any(|n| n.id == input.node_id) {
        return Ok(false);
    }
    if let Some(chat) = &input.conversation_id {
        if graph.bindings.get(&input.node_id) != Some(chat)
            || !independent(c, chat)?
            || is_paused(c, chat)?
        {
            return Ok(false);
        }
    }
    Ok(true)
}
pub fn pending_candidates(
    c: &Connection,
    after_sequence: u64,
    limit: usize,
) -> Result<Vec<PendingInputCandidate>, String> {
    let mut s = c.prepare(PENDING_CANDIDATES_SQL).map_err(db)?;
    let result = s
        .query_map(params![after_sequence, limit.clamp(1, 128)], |r| {
            Ok(PendingInputCandidate {
                id: r.get(0)?,
                sequence: r.get(1)?,
                instance_id: r.get(2)?,
                execution_version: r.get(3)?,
                conversation_id: r.get(4)?,
            })
        })
        .map_err(db)?
        .collect::<rusqlite::Result<_>>()
        .map_err(db);
    result
}
pub fn eligible_pending_input(c: &Connection, input_id: &str) -> Result<Option<Input>, String> {
    let Some(input) = load_input(c, input_id)? else {
        return Ok(None);
    };
    Ok((input.status == InputStatus::Pending
        && current_mail_status(c, input_id)? == MailStatus::Pending
        && eligible(c, &input)?)
    .then_some(input))
}
pub fn pending_inputs(c: &Connection) -> Result<Vec<Input>, String> {
    pending_candidates(c, 0, 128)?
        .into_iter()
        .map(|hint| eligible_pending_input(c, &hint.id))
        .filter_map(|result| result.transpose())
        .collect()
}
pub fn bound_inputs(c: &Connection, run_id: &str) -> Result<Vec<Input>, String> {
    let mut inputs = vec![];
    for input in list(c,"SELECT input_json FROM workflow_mail_inputs WHERE run_id=?1 AND status='claimed' ORDER BY sequence",run_id)? {
        if input.mail_status==MailStatus::Processing && eligible(c,&input)? {inputs.push(input)}
    }
    Ok(inputs)
}
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
        || current_mail_status(c, input_id)? != MailStatus::Pending
        || !eligible(c, &input)?
        || run_id.is_empty()
        || delivery_id.is_empty()
    {
        return Ok(false);
    }
    let blocked:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM workflow_mail_inputs older JOIN workflow_mail_messages m ON m.input_id=older.input_id WHERE older.conversation_id=?1 AND m.mail_status='pending' AND older.sequence<(SELECT sequence FROM workflow_mail_inputs WHERE input_id=?2))",params![input.conversation_id,input.id],|r|r.get(0)).map_err(db)?;
    if blocked {
        return Ok(false);
    }
    let owner = snapshot_for_run(
        c,
        input.conversation_id.as_deref().unwrap_or_default(),
        run_id,
    )?
    .ok_or("Organization identity was not admitted for this run")?;
    input.content = assemble_message(&owner, &input.messages);
    input.execution_version = owner.execution_version;
    c.execute("INSERT INTO workflow_mail_message_origins(message_id,conversation_id,input_id) VALUES(?1,?2,?3)",params![delivery_id,input.conversation_id,input.id]).map_err(db)?;
    input.status = InputStatus::Claimed;
    input.mail_status = MailStatus::Processing;
    input.run_id = Some(run_id.into());
    input.delivery_id = Some(delivery_id.into());
    write(c, &input)?;
    event(
        c,
        &input.instance_id,
        Some(&input.id),
        input.messages.first(),
        "accepted",
    )?;
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
    let result = bind_input_in_connection(&tx, input_id, run_id, delivery_id)?;
    tx.commit().map_err(db)?;
    Ok(result)
}
pub(super) fn has_delivery_proof(c: &Connection, input: &Input) -> Result<bool, String> {
    c.query_row("SELECT EXISTS(SELECT 1 FROM conversation_turn_trace_items i JOIN conversation_turn_traces t ON t.assistant_message_id=i.assistant_message_id WHERE t.run_id=?1 AND t.conversation_id=?2 AND i.item_kind='workflow_delivery' AND json_extract(i.item_json,'$.inputId')=?3 AND json_extract(i.item_json,'$.instanceId')=?4 AND json_extract(i.item_json,'$.content')=?5)",params![input.run_id,input.conversation_id,input.id,input.instance_id,input.content],|r|r.get(0)).map_err(db)
}
fn apply_proven_input(c: &Connection, input: &mut Input) -> Result<(), String> {
    if current_mail_status(c, &input.id)?.is_terminal() {
        return Ok(());
    }
    if input.status == InputStatus::Applied {
        return Ok(());
    }
    input.status = InputStatus::Applied;
    input.mail_status = MailStatus::Processing;
    input.error = None;
    write(c, input)?;
    if let Some(chat) = &input.conversation_id {
        c.execute(
            "UPDATE conversations SET unread_at=?1,updated_at=MAX(updated_at,?1) WHERE id=?2",
            params![now_ms(), chat],
        )
        .map_err(db)?;
    }
    event(
        c,
        &input.instance_id,
        Some(&input.id),
        input.messages.first(),
        "delivered",
    )
}
pub fn mark_applied(c: &mut Connection, input_id: &str) -> Result<(), String> {
    let tx = c
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db)?;
    let Some(mut input) = load_input(&tx, input_id)? else {
        return Err("Organization input no longer exists".into());
    };
    if current_mail_status(&tx, input_id)?.is_terminal() {
        return Ok(());
    }
    if !has_delivery_proof(&tx, &input)? {
        return Err("Organization input has no durable delivery receipt".into());
    }
    apply_proven_input(&tx, &mut input)?;
    tx.commit().map_err(db)
}
pub fn fail_input(c: &mut Connection, input_id: &str, reason: &str) -> Result<(), String> {
    let tx = c
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db)?;
    let Some(mut input) = load_input(&tx, input_id)? else {
        return Ok(());
    };
    if current_mail_status(&tx, input_id)?.is_terminal() {
        return Ok(());
    }
    input.mail_status = MailStatus::Failed;
    input.status = InputStatus::Failed;
    input.error = Some(reason.chars().take(2000).collect());
    write(&tx, &input)?;
    event(
        &tx,
        &input.instance_id,
        Some(input_id),
        input.messages.first(),
        "failed",
    )?;
    tx.commit().map_err(db)
}
pub fn pause_conversation(c: &mut Connection, conversation_id: &str) -> Result<(), String> {
    let tx = c
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db)?;
    if tx.execute("INSERT INTO workflow_mail_pauses(conversation_id,created_at) VALUES(?1,?2) ON CONFLICT(conversation_id) DO NOTHING",params![conversation_id,now_ms()]).map_err(db)?>0{conversation_event(&tx,conversation_id,"paused")?}
    // Pending mail remains pending. The durable terminal transaction settles processing mail.
    tx.commit().map_err(db)
}
pub fn resume_conversation(c: &mut Connection, conversation_id: &str) -> Result<(), String> {
    let tx = c
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db)?;
    if tx
        .execute(
            "DELETE FROM workflow_mail_pauses WHERE conversation_id=?1",
            [conversation_id],
        )
        .map_err(db)?
        > 0
    {
        conversation_event(&tx, conversation_id, "resumed")?
    }
    tx.commit().map_err(db)
}
fn conversation_event(c: &Connection, conversation_id: &str, kind: &str) -> Result<(), String> {
    let mut s = c
        .prepare("SELECT instance_id FROM workflow_instance_bindings WHERE conversation_id=?1")
        .map_err(db)?;
    for row in s
        .query_map([conversation_id], |r| r.get::<_, String>(0))
        .map_err(db)?
    {
        event(c, &row.map_err(db)?, None, None, kind)?;
    }
    Ok(())
}
/// Structural removal fences pending work; immutable mail and completed results remain inspectable.
pub fn invalidate_instance(c: &Connection, instance_id: &str, reason: &str) -> Result<(), String> {
    for mut input in list(c, UNSETTLED_INPUTS_SQL, instance_id)? {
        input.mail_status = MailStatus::Failed;
        input.status = InputStatus::Invalidated;
        input.error = Some(reason.into());
        write(c, &input)?;
        event(
            c,
            instance_id,
            Some(&input.id),
            input.messages.first(),
            "failed",
        )?;
    }
    Ok(())
}
/// Fences only mail whose frozen recipient no longer exists or no longer owns the node.
pub fn reconcile_recipients(c: &Connection, instance_id: &str) -> Result<(), String> {
    let Some(graph) = graph(c, instance_id)? else {
        return Ok(());
    };
    for mut input in list(c, UNSETTLED_INPUTS_SQL, instance_id)? {
        let recipient_matches = graph
            .definition
            .nodes
            .iter()
            .find(|n| n.id == input.node_id)
            .is_some_and(|n| graph.bindings.get(&n.id) == input.conversation_id.as_ref());
        if !recipient_matches {
            input.mail_status = MailStatus::Failed;
            input.status = InputStatus::Invalidated;
            input.error =
                Some("Original mail recipient is no longer bound to this organization node".into());
            write(c, &input)?;
            event(
                c,
                instance_id,
                Some(&input.id),
                input.messages.first(),
                "failed",
            )?;
        }
    }
    Ok(())
}
/// Called within the same transaction as the authoritative terminal run trace.
pub fn settle_run(c: &Connection, run_id: &str, status: &str) -> Result<(), String> {
    let final_status = match status {
        "completed" => MailStatus::Processed,
        "cancelled" | "stopped" => MailStatus::Stopped,
        "failed" => MailStatus::Failed,
        _ => return Ok(()),
    };
    for mut input in list(
        c,
        "SELECT input_json FROM workflow_mail_inputs WHERE run_id=?1 ORDER BY sequence",
        run_id,
    )? {
        if current_mail_status(c, &input.id)? != MailStatus::Processing {
            continue;
        }
        let proven = has_delivery_proof(c, &input)?;
        let result = if final_status == MailStatus::Processed && !proven {
            MailStatus::Failed
        } else {
            final_status
        };
        input.mail_status = result;
        input.status = match result {
            MailStatus::Processed => InputStatus::Completed,
            MailStatus::Stopped => InputStatus::Stopped,
            _ => InputStatus::Failed,
        };
        input.error = match result {
            MailStatus::Failed => Some(
                if proven {
                    "The processing turn failed"
                } else {
                    "Mail was claimed but its delivery was not durably confirmed"
                }
                .into(),
            ),
            _ => None,
        };
        write(c, &input)?;
        event(
            c,
            &input.instance_id,
            Some(&input.id),
            input.messages.first(),
            match result {
                MailStatus::Processed => "completed",
                MailStatus::Stopped => "stopped",
                _ => "failed",
            },
        )?;
    }
    Ok(())
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
        return Err("Organization instance no longer exists".into());
    }
    let sequence: u64 = c
        .query_row(
            "SELECT COALESCE(MAX(sequence),0) FROM workflow_mail_events WHERE instance_id=?1",
            [instance_id],
            |r| r.get(0),
        )
        .map_err(db)?;
    let mut s=c.prepare("SELECT sequence,input_id,message_id,source_node_id,target_node_id,kind,created_at FROM workflow_mail_events WHERE instance_id=?1 AND sequence>?2 ORDER BY sequence DESC LIMIT 512").map_err(db)?;
    let mut events = s
        .query_map(params![instance_id, after_sequence.unwrap_or(0)], |r| {
            Ok(Event {
                sequence: r.get(0)?,
                instance_id: instance_id.into(),
                input_id: r.get(1)?,
                message_id: r.get(2)?,
                source_node_id: r.get(3)?,
                target_node_id: r.get(4)?,
                kind: r.get(5)?,
                created_at: r.get(6)?,
            })
        })
        .map_err(db)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(db)?;
    events.reverse();
    // Events must always be resolvable by the renderer, including a late terminal event for mail
    // explicitly completed before the newest 128 history rows. Only this bounded event page adds
    // history rows; pending/processing mail and the normal history window retain their limits.
    let referenced_inputs: Vec<_> = events
        .iter()
        .filter_map(|event| event.input_id.as_deref())
        .collect();
    let mut statement=c.prepare("SELECT input_json FROM workflow_mail_inputs WHERE instance_id=?1 AND (input_id IN(SELECT input_id FROM workflow_mail_messages WHERE mail_status IN('pending','processing')) OR sequence IN(SELECT sequence FROM workflow_mail_inputs WHERE instance_id=?1 ORDER BY sequence DESC LIMIT 128) OR input_id IN(SELECT value FROM json_each(?2))) ORDER BY sequence").map_err(db)?;
    let mut inputs: Vec<Input> = statement
        .query_map(params![instance_id, json(&referenced_inputs)?], |row| {
            row.get::<_, String>(0)
        })
        .map_err(db)?
        .map(|row| parse(&row.map_err(db)?))
        .collect::<Result<_, _>>()?;
    for input in &mut inputs {
        input.content.clear();
        for message in &mut input.messages {
            message.content.clear();
        }
    }
    let mut s=c.prepare("SELECT p.conversation_id FROM workflow_mail_pauses p JOIN workflow_instance_bindings b ON b.conversation_id=p.conversation_id WHERE b.instance_id=?1 ORDER BY p.conversation_id").map_err(db)?;
    let paused_conversation_ids = s
        .query_map([instance_id], |r| r.get(0))
        .map_err(db)?
        .collect::<rusqlite::Result<_>>()
        .map_err(db)?;
    let mut input_runs = vec![];
    for input in &inputs {
        if let Some(status)=c.query_row("SELECT terminal_status FROM conversation_turn_traces WHERE run_id=?1 AND conversation_id=?2",params![input.run_id,input.conversation_id],|r|r.get(0)).optional().map_err(db)?{input_runs.push(InputRunState{input_id:input.id.clone(),status});}
    }
    Ok(RuntimeSnapshot {
        instance_id: instance_id.into(),
        sequence,
        inputs,
        events,
        paused_conversation_ids,
        input_runs,
        preference_updates: vec![],
    })
}
pub fn node_messages(
    c: &Connection,
    instance_id: &str,
    node_id: &str,
    before: Option<u64>,
) -> Result<NodeMessages, String> {
    let graph = graph(c, instance_id)?.ok_or("Organization no longer exists")?;
    let target = node(&graph, node_id)?;
    let mut s=c.prepare("SELECT m.sequence,m.message_json,m.input_id,m.mail_status,t.terminal_status,json_extract(i.input_json,'$.error') FROM workflow_mail_messages m LEFT JOIN workflow_mail_inputs i ON i.input_id=m.input_id LEFT JOIN conversation_turn_traces t ON t.run_id=i.run_id AND t.conversation_id=i.conversation_id WHERE m.instance_id=?1 AND m.node_id=?2 AND m.recipient_conversation_id IS ?3 AND (?4 IS NULL OR m.sequence<?4) ORDER BY m.sequence DESC LIMIT 21").map_err(db)?;
    let mut messages = vec![];
    for row in s
        .query_map(
            params![instance_id, target.id, graph.bindings.get(node_id), before],
            |r| {
                Ok((
                    r.get::<_, u64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                ))
            },
        )
        .map_err(db)?
    {
        let (sequence, raw, input_id, status, run_status, error) = row.map_err(db)?;
        messages.push(NodeMessage {
            sequence,
            message: parse(&raw)?,
            input_id,
            status,
            run_status,
            error,
        });
    }
    let has_more = messages.len() > 20;
    messages.truncate(20);
    let next_before_sequence = has_more.then(|| messages.last().unwrap().sequence);
    Ok(NodeMessages {
        instance_id: instance_id.into(),
        node_id: node_id.into(),
        messages,
        next_before_sequence,
    })
}
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
            "SELECT conversation_id,snapshot_json FROM workflow_mail_runs WHERE run_id=?1",
            [run_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(db)?;
    let live = snapshot_for_conversation(&tx, conversation_id)?;
    let result = if let Some((owner, raw)) = existing {
        if owner != conversation_id {
            return Err("Organization run belongs to another conversation".into());
        }
        let old: Option<ConversationSnapshot> = parse(&raw)?;
        live.filter(|new| {
            old.as_ref().is_some_and(|old| {
                new.execution_version == old.execution_version
                    && new.node_id == old.node_id
                    && new.instance_id == old.instance_id
            })
        })
    } else {
        tx.execute("INSERT INTO workflow_mail_runs(run_id,conversation_id,snapshot_json,created_at) VALUES(?1,?2,?3,?4)",params![run_id,conversation_id,json(&live)?,now_ms()]).map_err(db)?;
        live
    };
    tx.commit().map_err(db)?;
    Ok(result)
}
pub fn snapshot_for_run(
    c: &Connection,
    conversation_id: &str,
    run_id: &str,
) -> Result<Option<ConversationSnapshot>, String> {
    let raw: Option<String> = c
        .query_row(
            "SELECT snapshot_json FROM workflow_mail_runs WHERE run_id=?1 AND conversation_id=?2",
            params![run_id, conversation_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(db)?;
    let old: Option<ConversationSnapshot> = raw.as_deref().map(parse).transpose()?.flatten();
    let live = snapshot_for_conversation(c, conversation_id)?;
    Ok(live.filter(|new| {
        old.as_ref().is_some_and(|old| {
            new.instance_id == old.instance_id
                && new.node_id == old.node_id
                && new.execution_version == old.execution_version
        })
    }))
}
pub fn recover_claims(c: &mut Connection) -> Result<(), String> {
    let tx = c
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db)?;
    let mut s=tx.prepare("SELECT DISTINCT i.run_id FROM workflow_mail_inputs i JOIN workflow_mail_messages m ON m.input_id=i.input_id WHERE m.mail_status='processing' AND i.run_id IS NOT NULL").map_err(db)?;
    let runs = s
        .query_map([], |r| r.get::<_, String>(0))
        .map_err(db)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(db)?;
    drop(s);
    for run in runs {
        let status: Option<String> = tx
            .query_row(
                "SELECT terminal_status FROM conversation_turn_traces WHERE run_id=?1",
                [&run],
                |r| r.get(0),
            )
            .optional()
            .map_err(db)?;
        if status.as_deref() != Some("in_progress") {
            settle_run(&tx, &run, status.as_deref().unwrap_or("failed"))?;
            continue;
        }
        for mut input in list(&tx,"SELECT input_json FROM workflow_mail_inputs WHERE run_id=?1 AND status='claimed' ORDER BY sequence",&run)?{if has_delivery_proof(&tx,&input)?{apply_proven_input(&tx,&mut input)?;}}
    }
    tx.commit().map_err(db)
}
pub fn delivery_origins_for_conversation(
    c: &Connection,
    conversation_id: &str,
) -> Result<Vec<(String, Input)>, String> {
    let mut s=c.prepare("SELECT o.message_id,i.input_json FROM workflow_mail_message_origins o JOIN workflow_mail_inputs i ON i.input_id=o.input_id WHERE o.conversation_id=?1 ORDER BY o.message_id").map_err(db)?;
    let result = s
        .query_map([conversation_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })
        .map_err(db)?
        .map(|row| {
            let (id, raw) = row.map_err(db)?;
            Ok((id, parse(&raw)?))
        })
        .collect();
    result
}
pub fn mark_run_unread(c: &mut Connection, run_id: &str) -> Result<(), String> {
    let tx = c
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db)?;
    let terminal: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM conversation_turn_traces WHERE run_id=?1 AND terminal_status!='in_progress')",
        [run_id], |row| row.get(0),
    ).map_err(db)?;
    if !terminal {
        return Ok(());
    }
    let inputs=list(&tx,"SELECT input_json FROM workflow_mail_inputs WHERE run_id=?1 AND completion_notified=0 ORDER BY sequence",run_id)?;
    for input in inputs {
        if !input.mail_status.is_terminal() {
            continue;
        }
        let changed = tx.execute(
            "UPDATE workflow_mail_inputs SET completion_notified=1 WHERE input_id=?1 AND completion_notified=0",
            [&input.id],
        ).map_err(db)?;
        if changed == 0 {
            continue;
        }
        if let Some(chat) = &input.conversation_id {
            tx.execute(
                "UPDATE conversations SET unread_at=?1,updated_at=MAX(updated_at,?1) WHERE id=?2",
                params![now_ms(), chat],
            )
            .map_err(db)?;
        }
        // Explicit organization_complete may have settled this mail before the final answer existed.
        // Notify chat subscribers about the later terminal turn even when Input status is unchanged.
        // No transmission endpoints: this is a local refresh event, not another delivered letter.
        event(
            &tx,
            &input.instance_id,
            Some(&input.id),
            None,
            "run_completed",
        )?;
    }
    tx.commit().map_err(db)
}
