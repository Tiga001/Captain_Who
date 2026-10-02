//! Compact automatic observations. Never load per-message bodies or detail rows to discard them.
use super::*;

pub(super) fn nodes(
    c: &Connection,
    graph: &Graph,
    current_node_id: &str,
) -> Result<Vec<Value>, String> {
    let mut selected: Vec<_> = graph.definition.nodes.iter().collect();
    selected.sort_by_key(|node| (node.id != current_node_id, node.id.as_str()));
    selected.truncate(16);
    let ids = json(&selected.iter().map(|node| &node.id).collect::<Vec<_>>())?;

    let mut counts = BTreeMap::<String, (u64, u64)>::new();
    let mut statement = c.prepare("SELECT m.node_id,m.mail_status,COUNT(*) FROM workflow_mail_messages m JOIN workflow_instance_bindings b ON b.instance_id=m.instance_id AND b.node_id=m.node_id AND b.conversation_id=m.recipient_conversation_id WHERE m.instance_id=?1 AND m.node_id IN(SELECT value FROM json_each(?2)) AND m.mail_status IN('pending','processing') GROUP BY m.node_id,m.mail_status").map_err(db)?;
    for row in statement
        .query_map(params![graph.instance_id, ids], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, u64>(2)?,
            ))
        })
        .map_err(db)?
    {
        let (node, status, count) = row.map_err(db)?;
        let entry = counts.entry(node).or_default();
        if status == "pending" {
            entry.0 = count;
        } else {
            entry.1 = count;
        }
    }

    let mut statement = c.prepare("SELECT b.node_id FROM workflow_instance_bindings b JOIN workflow_mail_pauses p ON p.conversation_id=b.conversation_id WHERE b.instance_id=?1 AND b.node_id IN(SELECT value FROM json_each(?2))").map_err(db)?;
    let paused: BTreeSet<String> = statement
        .query_map(params![graph.instance_id, ids], |row| row.get(0))
        .map_err(db)?
        .collect::<rusqlite::Result<_>>()
        .map_err(db)?;

    let mut statement = c.prepare("WITH ranked AS (SELECT b.node_id,t.terminal_status,ROW_NUMBER() OVER(PARTITION BY b.node_id ORDER BY t.created_at DESC,t.rowid DESC) AS position FROM workflow_instance_bindings b JOIN conversation_turn_traces t ON t.conversation_id=b.conversation_id JOIN workflow_mail_runs r ON r.run_id=t.run_id AND r.conversation_id=t.conversation_id WHERE b.instance_id=?1 AND b.node_id IN(SELECT value FROM json_each(?2)) AND json_extract(r.snapshot_json,'$.instanceId')=b.instance_id AND json_extract(r.snapshot_json,'$.nodeId')=b.node_id) SELECT node_id,terminal_status FROM ranked WHERE position=1").map_err(db)?;
    let latest: BTreeMap<String, String> = statement
        .query_map(params![graph.instance_id, ids], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .map_err(db)?
        .collect::<rusqlite::Result<_>>()
        .map_err(db)?;

    let mut statement = c.prepare("WITH ranked AS (SELECT m.node_id,m.input_id,ROW_NUMBER() OVER(PARTITION BY m.node_id ORDER BY m.sequence) AS position FROM workflow_mail_messages m JOIN workflow_instance_bindings b ON b.instance_id=m.instance_id AND b.node_id=m.node_id AND b.conversation_id=m.recipient_conversation_id WHERE m.instance_id=?1 AND m.node_id IN(SELECT value FROM json_each(?2)) AND m.mail_status='processing') SELECT node_id,input_id FROM ranked WHERE position<=8 ORDER BY node_id,position").map_err(db)?;
    let mut current = BTreeMap::<String, Vec<String>>::new();
    for row in statement
        .query_map(params![graph.instance_id, ids], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(db)?
    {
        let (node, input) = row.map_err(db)?;
        current.entry(node).or_default().push(input);
    }

    Ok(selected.into_iter().map(|node| {
        let (pending, processing) = counts.get(&node.id).copied().unwrap_or_default();
        let paused = paused.contains(&node.id);
        let last_run = latest.get(&node.id);
        let state = if paused { "stopped" } else if processing > 0 || last_run.is_some_and(|status| status == "in_progress") { "running" } else if pending > 0 { "queued" } else { "idle" };
        value!({"nodeId":node.id,"nodeName":node.name,"kind":"agent","conversationId":graph.bindings.get(&node.id),"state":state,"paused":paused,"pendingCount":pending,"processingCount":processing,"currentInputCount":processing,"currentInputIds":current.get(&node.id).cloned().unwrap_or_default(),"lastRunStatus":last_run})
    }).collect())
}
