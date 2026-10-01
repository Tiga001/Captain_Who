//! Model-facing read projections over the existing durable workflow queue.
//! Reading never claims, acknowledges, resumes, or schedules an input.
use super::*;
use crate::workflow_awareness::{MailboxDirection, MailboxQuery, StateQuery, StateView};
use serde_json::{json, Value};

fn scope(
    c: &Connection,
    conversation_id: &str,
    run_id: &str,
) -> Result<(Graph, ConversationSnapshot), String> {
    let identity = snapshot_for_run(c, conversation_id, run_id)?
        .ok_or("The current run has no active workflow identity")?;
    let graph = graph(c, &identity.instance_id)?.ok_or("Workflow instance no longer exists")?;
    if !graph.enabled
        || graph.execution_version != identity.execution_version
        || graph.bindings.get(&identity.node_id).map(String::as_str) != Some(conversation_id)
    {
        return Err("The current run has no active workflow identity".into());
    }
    Ok((graph, identity))
}

fn kind(node: &Node) -> &'static str {
    match node.config {
        NodeConfig::Agent(_) => "agent",
        NodeConfig::User { .. } => "user",
        NodeConfig::InputGate { .. } => "inputGate",
        NodeConfig::OutputGate { .. } => "outputGate",
    }
}

fn preview_fields(value: &mut Value, fields: &[&str], limit: usize) {
    let mut truncated = Vec::new();
    for field in fields {
        if let Some(text) = value.get(field).and_then(Value::as_str) {
            if text.chars().count() > limit {
                let preview: String = text.chars().take(limit).collect();
                value[field] = json!(preview);
                truncated.push(*field);
            }
        }
    }
    if !truncated.is_empty() {
        value["truncatedFields"] = json!(truncated);
        value["detailsQueryHint"] = json!(
            "Use workflow_get_state with nodeId for complete node details and shared background."
        );
    }
}

fn topology(graph: &Graph, selected: Option<&str>) -> Result<Value, String> {
    let mut nodes = Vec::new();
    for node in &graph.definition.nodes {
        if selected.is_some_and(|selected| selected != node.id) {
            continue;
        }
        let incoming: Vec<_> = graph
            .definition
            .flows
            .iter()
            .filter(|flow| flow.target.node() == Some(&node.id))
            .collect();
        let outgoing: Vec<_> = graph
            .definition
            .flows
            .iter()
            .filter(|flow| flow.source.node() == Some(&node.id))
            .collect();
        let mut value = json!({
            "nodeId": node.id, "nodeName": node.name, "kind": kind(node),
            "conversationId": graph.bindings.get(&node.id),
            "incomingFlowIds": incoming.iter().map(|flow| &flow.id).collect::<Vec<_>>(),
            "outgoingFlowIds": outgoing.iter().map(|flow| &flow.id).collect::<Vec<_>>(),
            "upstreamNodeIds": incoming.iter().filter_map(|flow| flow.source.node()).collect::<BTreeSet<_>>(),
            "downstreamNodeIds": outgoing.iter().filter_map(|flow| flow.target.node()).collect::<BTreeSet<_>>(),
        });
        match &node.config {
            NodeConfig::Agent(config) => {
                value["receives"] = json!(config.receives);
                value["task"] = json!(config.task);
                value["delivers"] = json!(config.delivers);
            }
            NodeConfig::User { task } => value["task"] = json!(task),
            NodeConfig::InputGate {
                processing_mode,
                busy_policy,
            } => {
                value["processingMode"] = json!(processing_mode);
                value["busyPolicy"] = json!(busy_policy);
            }
            NodeConfig::OutputGate { selection } => value["selection"] = json!(selection),
        }
        if node.is_participant() {
            let snapshot = snapshot(graph, &node.id)?;
            value["predecessors"] = json!(snapshot.predecessors);
            value["outputs"] = json!(snapshot.outputs);
            value["inputRule"] = json!(snapshot.input_rule);
            value["outputRule"] = json!(snapshot.output_rule);
        }
        if selected.is_none() {
            preview_fields(&mut value, &["receives", "task", "delivers"], 512);
        }
        nodes.push(value);
    }
    let flows: Vec<_> = graph.definition.flows.iter()
        .filter(|flow| selected.is_none_or(|id| flow.source.node() == Some(id) || flow.target.node() == Some(id)))
        .map(|flow| json!({"flowId": flow.id, "flowName": flow.name, "source": flow.source, "target": flow.target}))
        .collect();
    let mut result = json!({"nodes": nodes, "flows": flows});
    if graph
        .definition
        .flows
        .iter()
        .any(|flow| flow.source.node().is_none())
    {
        result["entry"] = json!({"kind":"boundary","name":"User entry"});
    }
    Ok(result)
}

fn participant_runtime(c: &Connection, graph: &Graph, node: &Node) -> Result<Value, String> {
    let conversation_id = graph.bindings.get(&node.id);
    let paused: bool = c
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM workflow_execution_pauses WHERE conversation_id=?1)",
            [conversation_id],
            |row| row.get(0),
        )
        .map_err(db)?;
    let mut counts = BTreeMap::<String, u64>::new();
    let mut statement = c.prepare("SELECT status,COUNT(*) FROM workflow_execution_inputs
        WHERE instance_id=?1 AND execution_version=?2 AND node_id=?3 AND conversation_id IS ?4 GROUP BY status").map_err(db)?;
    for row in statement
        .query_map(
            params![
                graph.instance_id,
                graph.execution_version,
                node.id,
                conversation_id
            ],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(db)?
    {
        let (status, count) = row.map_err(db)?;
        counts.insert(status, count);
    }
    let mut collecting = BTreeMap::<String, u64>::new();
    let mut statement = c.prepare("SELECT flow_id,COUNT(*) FROM workflow_execution_messages
        WHERE instance_id=?1 AND execution_version=?2 AND node_id=?3 AND input_id IS NULL AND invalidated=0 GROUP BY flow_id").map_err(db)?;
    for row in statement
        .query_map(
            params![graph.instance_id, graph.execution_version, node.id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(db)?
    {
        let (flow, count) = row.map_err(db)?;
        collecting.insert(flow, count);
    }
    let (mode, busy, incoming) = input_config(graph, &node.id)?;
    let missing: Vec<_> = if mode == InputProcessingMode::Batch {
        incoming
            .iter()
            .filter(|flow| !collecting.contains_key(*flow))
            .cloned()
            .collect()
    } else {
        Vec::new()
    };
    let mut statement = c.prepare("SELECT i.input_id,i.status,i.run_id,t.terminal_status,
            json_extract(i.input_json,'$.error'),i.delivery_id,
            (SELECT json_group_array(json_extract(m.value,'$.id')) FROM json_each(i.input_json,'$.messages') m)
        FROM workflow_execution_inputs i
        LEFT JOIN conversation_turn_traces t ON t.run_id=i.run_id AND t.conversation_id=i.conversation_id
        WHERE i.instance_id=?1 AND i.execution_version=?2 AND i.node_id=?3 AND i.conversation_id IS ?4
          AND (i.status IN ('pending','claimed','paused','waiting_user','failed') OR (i.status='applied' AND t.terminal_status='in_progress'))
        ORDER BY i.sequence").map_err(db)?;
    let mut inputs = Vec::new();
    for row in statement
        .query_map(
            params![
                graph.instance_id,
                graph.execution_version,
                node.id,
                conversation_id
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, String>(6)?,
                ))
            },
        )
        .map_err(db)?
    {
        let (id, status, run_id, run_status, error, delivery_id, message_ids) = row.map_err(db)?;
        inputs.push(json!({"inputId":id,"deliveryStatus":status,"runId":run_id,"runStatus":run_status,"error":error,"deliveryId":delivery_id,"messageIds":parse::<Value>(&message_ids)?}));
    }
    let latest_run = c.query_row("SELECT t.run_id,t.terminal_status FROM conversation_turn_traces t
        JOIN workflow_execution_runs r ON r.run_id=t.run_id AND r.conversation_id=t.conversation_id
        WHERE t.conversation_id=?1 AND json_extract(r.snapshot_json,'$.instanceId')=?2
          AND json_extract(r.snapshot_json,'$.executionVersion')=?3 AND json_extract(r.snapshot_json,'$.nodeId')=?4
        ORDER BY t.created_at DESC,t.rowid DESC LIMIT 1",
        params![conversation_id,graph.instance_id,graph.execution_version,node.id],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
    ).optional().map_err(db)?.map(|(id,status)|json!({"runId":id,"status":status}));
    let active: Vec<_> = inputs
        .iter()
        .filter(|input| input["deliveryStatus"] == "claimed" || input["runStatus"] == "in_progress")
        .map(|input| input["inputId"].clone())
        .collect();
    let queued =
        counts.get("pending").copied().unwrap_or(0) + counts.get("paused").copied().unwrap_or(0);
    let collecting_count: u64 = collecting.values().sum();
    let state = if paused {
        "paused"
    } else if counts.get("failed").copied().unwrap_or(0) > 0 {
        "delivery_failed"
    } else if latest_run
        .as_ref()
        .is_some_and(|run| run["status"] == "in_progress")
        || !active.is_empty()
    {
        "running"
    } else if counts.get("waiting_user").copied().unwrap_or(0) > 0 {
        "waiting_user"
    } else if queued > 0 {
        "queued"
    } else if collecting_count > 0 {
        "collecting"
    } else {
        "idle"
    };
    Ok(json!({
        "nodeId":node.id,"nodeName":node.name,"kind":kind(node),"conversationId":conversation_id,
        "state":state,"paused":paused,"processingMode":mode,"busyPolicy":busy,
        "queuedInputCount":queued,"collectingMessageCount":collecting_count,
        "inputCounts":counts,"collectingByFlow":collecting,"missingFlowIds":missing,
        "currentInputIds":active,"inputs":inputs,"latestRun":latest_run,
    }))
}

fn participant_owner<'a>(graph: &'a Graph, node: &'a Node) -> Option<&'a str> {
    match node.config {
        NodeConfig::InputGate { .. } => graph
            .definition
            .flows
            .iter()
            .find(|flow| flow.source.node() == Some(&node.id))
            .and_then(|flow| flow.target.node()),
        NodeConfig::OutputGate { .. } => graph
            .definition
            .flows
            .iter()
            .find(|flow| flow.target.node() == Some(&node.id))
            .and_then(|flow| flow.source.node()),
        _ => Some(&node.id),
    }
}

fn runtime_nodes(
    c: &Connection,
    graph: &Graph,
    selected: Option<&BTreeSet<String>>,
) -> Result<Value, String> {
    let needed: BTreeSet<_> = graph
        .definition
        .nodes
        .iter()
        .filter(|node| selected.is_none_or(|ids| ids.contains(&node.id)))
        .filter_map(|node| participant_owner(graph, node))
        .collect();
    let mut participants = BTreeMap::new();
    for node in graph
        .definition
        .nodes
        .iter()
        .filter(|node| node.is_participant() && needed.contains(node.id.as_str()))
    {
        participants.insert(node.id.clone(), participant_runtime(c, graph, node)?);
    }
    let mut nodes = Vec::new();
    for node in &graph.definition.nodes {
        if selected.is_some_and(|selected| !selected.contains(&node.id)) {
            continue;
        }
        if let Some(value) = participants.get(&node.id) {
            nodes.push(value.clone());
            continue;
        }
        let owner = participant_owner(graph, node);
        let mut value = json!({"nodeId":node.id,"nodeName":node.name,"kind":kind(node),"participantNodeId":owner,"state":"logical_gate"});
        if let Some(participant) = owner.and_then(|owner| participants.get(owner)) {
            for key in [
                "queuedInputCount",
                "collectingMessageCount",
                "missingFlowIds",
                "currentInputIds",
                "paused",
            ] {
                value[key] = participant[key].clone();
            }
            value["participantConversationId"] = participant["conversationId"].clone();
            value["participantState"] = participant["state"].clone();
            value["participantLastRunStatus"] = participant["latestRun"]["status"].clone();
        }
        nodes.push(value);
    }
    Ok(json!({"nodes":nodes}))
}

fn runtime(c: &Connection, graph: &Graph, selected: Option<&str>) -> Result<Value, String> {
    let selected = selected.map(|id| BTreeSet::from([id.to_string()]));
    runtime_nodes(c, graph, selected.as_ref())
}

pub fn state_for_run(
    c: &Connection,
    conversation_id: &str,
    run_id: &str,
    query: &StateQuery,
) -> Result<Value, String> {
    query.validate()?;
    let transaction = c.unchecked_transaction().map_err(db)?;
    let (graph, identity) = scope(&transaction, conversation_id, run_id)?;
    if let Some(selected) = &query.node_id {
        node(&graph, selected)?;
    }
    let mut result = json!({"available":true,"instanceId":identity.instance_id,"workflowName":identity.name,"description":graph.definition.description,"background":graph.definition.background,"executionVersion":identity.execution_version,"currentNodeId":identity.node_id,"observedAt":now_ms()});
    if query.view != StateView::Runtime {
        result["topology"] = topology(&graph, query.node_id.as_deref())?;
    }
    if query.view != StateView::Topology {
        result["runtime"] = runtime(&transaction, &graph, query.node_id.as_deref())?;
    }
    if query.node_id.is_none() {
        preview_fields(&mut result, &["description", "background"], 4096);
    }
    Ok(result)
}

fn compact_node(node: &Value) -> Value {
    let mut result = json!({"nodeId":node["nodeId"],"nodeName":node["nodeName"],"kind":node["kind"],"state":node["state"]});
    for key in [
        "conversationId",
        "participantConversationId",
        "participantState",
        "participantLastRunStatus",
        "paused",
        "queuedInputCount",
        "collectingMessageCount",
    ] {
        if let Some(value) = node.get(key) {
            result[key] = value.clone();
        }
    }
    if let Some(status) = node["latestRun"].get("status") {
        result["lastRunStatus"] = status.clone();
    }
    for (key, count_key, limit) in [
        ("currentInputIds", "currentInputCount", 8),
        ("missingFlowIds", "missingFlowCount", 16),
    ] {
        if let Some(items) = node[key].as_array() {
            result[key] = json!(items.iter().take(limit).collect::<Vec<_>>());
            result[count_key] = json!(items.len());
        }
    }
    result
}

fn compact_nodes(nodes: &[Value], identity: &ConversationSnapshot) -> Vec<Value> {
    let neighbors: BTreeSet<_> = identity
        .predecessors
        .iter()
        .map(|node| node.node_id.as_str())
        .chain(identity.outputs.iter().map(|node| node.node_id.as_str()))
        .collect();
    let mut ordered: Vec<_> = nodes.iter().collect();
    ordered.sort_by_key(|node| {
        let id = node["nodeId"].as_str().unwrap_or_default();
        if id == identity.node_id {
            0
        } else if neighbors.contains(id) {
            1
        } else {
            2
        }
    });
    ordered.into_iter().take(16).map(compact_node).collect()
}

fn awareness(
    c: &Connection,
    graph: &Graph,
    identity: &ConversationSnapshot,
) -> Result<Value, String> {
    // Choose the compact observation before querying the database; a large workflow must not
    // decode every node's outstanding inputs at every model sampling boundary.
    let candidate_nodes: Vec<_> = graph
        .definition
        .nodes
        .iter()
        .map(|node| json!({"nodeId":node.id}))
        .collect();
    let selected: BTreeSet<_> = compact_nodes(&candidate_nodes, identity)
        .iter()
        .filter_map(|node| node["nodeId"].as_str().map(String::from))
        .collect();
    let runtime = runtime_nodes(c, graph, Some(&selected))?;
    let current = runtime["nodes"]
        .as_array()
        .and_then(|nodes| nodes.iter().find(|node| node["nodeId"] == identity.node_id))
        .ok_or("Workflow node no longer exists")?;
    let current = compact_node(current);
    let total_node_count = graph.definition.nodes.len();
    let nodes = compact_nodes(runtime["nodes"].as_array().unwrap(), identity);
    let mut statement=c.prepare("SELECT m.message_id,m.node_id,m.input_id,
        CASE WHEN m.invalidated=1 THEN 'invalidated' ELSE COALESCE(i.status,'collecting') END,t.terminal_status
        FROM workflow_execution_messages m
        LEFT JOIN workflow_execution_inputs i ON i.input_id=m.input_id
        LEFT JOIN conversation_turn_traces t ON t.run_id=i.run_id AND t.conversation_id=i.conversation_id
        LEFT JOIN workflow_instance_bindings b ON b.instance_id=m.instance_id AND b.node_id=m.node_id
        WHERE m.instance_id=?1 AND m.execution_version=?2
          AND json_extract(m.message_json,'$.sourceNodeId')=?3 AND json_extract(m.message_json,'$.sourceConversationId')=?4
          AND (m.input_id IS NULL OR (i.instance_id=m.instance_id AND i.execution_version=m.execution_version AND i.node_id=m.node_id AND i.conversation_id IS b.conversation_id))
        ORDER BY m.sequence DESC LIMIT 3").map_err(db)?;
    let recent_sent=statement.query_map(params![identity.instance_id,identity.execution_version,identity.node_id,graph.bindings.get(&identity.node_id)],|row|Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?,row.get::<_,Option<String>>(2)?,row.get::<_,String>(3)?,row.get::<_,Option<String>>(4)?))).map_err(db)?
        .map(|row|row.map(|(id,target,input,status,run)|json!({"messageId":id,"targetNodeId":target,"inputId":input,"deliveryStatus":status,"runStatus":run})).map_err(db)).collect::<Result<Vec<_>,_>>()?;
    Ok(
        json!({"available":true,"instanceId":identity.instance_id,"executionVersion":identity.execution_version,"currentNodeId":identity.node_id,"currentInputIds":current["currentInputIds"],"currentInputCount":current["currentInputCount"],"queue":{"queuedInputCount":current["queuedInputCount"],"collectingMessageCount":current["collectingMessageCount"],"missingFlowIds":current["missingFlowIds"],"missingFlowCount":current["missingFlowCount"]},"nodes":nodes,"totalNodeCount":total_node_count,"nodesTruncated":total_node_count>16,"queryHint":"Use workflow_get_state for complete workflow metadata and workflow_get_mailbox for message details.","recentSent":recent_sent}),
    )
}

pub fn awareness_for_run(
    c: &Connection,
    conversation_id: &str,
    run_id: &str,
) -> Result<Value, String> {
    let transaction = c.unchecked_transaction().map_err(db)?;
    let (graph, identity) = scope(&transaction, conversation_id, run_id)?;
    awareness(&transaction, &graph, &identity)
}

/// Prompt preview only. Model tools always use the run-bound functions above.
pub fn awareness_for_conversation(c: &Connection, conversation_id: &str) -> Result<Value, String> {
    let transaction = c.unchecked_transaction().map_err(db)?;
    let Some(identity) = snapshot_for_conversation(&transaction, conversation_id)? else {
        return Ok(json!({"available":false,"reason":"not_active_or_unavailable"}));
    };
    let graph =
        graph(&transaction, &identity.instance_id)?.ok_or("Workflow instance no longer exists")?;
    awareness(&transaction, &graph, &identity)
}

pub fn mailbox_for_run(
    c: &Connection,
    conversation_id: &str,
    run_id: &str,
    query: &MailboxQuery,
) -> Result<Value, String> {
    query.validate()?;
    let transaction = c.unchecked_transaction().map_err(db)?;
    let (_, identity) = scope(&transaction, conversation_id, run_id)?;
    let inbox = query.direction == MailboxDirection::Inbox;
    let mut statement=transaction.prepare("SELECT m.sequence,m.message_json,m.input_id,
            CASE WHEN m.invalidated=1 THEN 'invalidated' ELSE COALESCE(i.status,'collecting') END,
            i.run_id,t.terminal_status,json_extract(i.input_json,'$.error'),
            EXISTS(SELECT 1 FROM conversation_turn_trace_items proof JOIN conversation_turn_traces pt ON pt.assistant_message_id=proof.assistant_message_id
              WHERE pt.run_id=i.run_id AND pt.conversation_id=i.conversation_id AND proof.item_kind='workflow_delivery'
                AND json_extract(proof.item_json,'$.inputId')=i.input_id AND json_extract(proof.item_json,'$.instanceId')=i.instance_id
                AND json_extract(proof.item_json,'$.content')=json_extract(i.input_json,'$.content')),
            i.delivery_id
        FROM workflow_execution_messages m
        LEFT JOIN workflow_execution_inputs i ON i.input_id=m.input_id
        LEFT JOIN conversation_turn_traces t ON t.run_id=i.run_id AND t.conversation_id=i.conversation_id
        LEFT JOIN workflow_instance_bindings b ON b.instance_id=m.instance_id AND b.node_id=m.node_id
        WHERE m.instance_id=?1 AND m.execution_version=?2
          AND ((?3=1 AND m.node_id=?4 AND b.conversation_id=?5)
            OR (?3=0 AND json_extract(m.message_json,'$.sourceNodeId')=?4 AND json_extract(m.message_json,'$.sourceConversationId')=?5))
          AND (m.input_id IS NULL OR (i.instance_id=m.instance_id AND i.execution_version=m.execution_version AND i.node_id=m.node_id AND i.conversation_id IS b.conversation_id))
          AND (?6 IS NULL OR m.sequence<?6) AND (?7 IS NULL OR m.message_id=?7) AND (?8 IS NULL OR m.input_id=?8)
        ORDER BY m.sequence DESC LIMIT ?9").map_err(db)?;
    let rows = statement
        .query_map(
            params![
                identity.instance_id,
                identity.execution_version,
                inbox,
                identity.node_id,
                conversation_id,
                query.cursor,
                query.message_id,
                query.input_id,
                query.limit + 1
            ],
            |row| {
                Ok((
                    row.get::<_, u64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, bool>(7)?,
                    row.get::<_, Option<String>>(8)?,
                ))
            },
        )
        .map_err(db)?;
    let mut messages = Vec::new();
    let mut body_bytes = 0;
    for row in rows {
        let (sequence, raw, input_id, status, run_id, run_status, error, applied, delivery_id) =
            row.map_err(db)?;
        let source: SourceMessage = parse(&raw)?;
        let visible = !inbox || applied;
        let mut value = json!({"sequence":sequence,"messageId":source.id,"sourceNodeId":source.source_node_id,"sourceNodeName":source.source_node_name,"targetNodeId":source.target_node_id,"flowId":source.flow_id,"pathFlowIds":source.path_flow_ids,"inputId":input_id,"deliveryId":delivery_id,"deliveryStatus":status,"runId":run_id,"runStatus":run_status,"error":error,"bodyAvailable":visible,"createdAt":source.created_at});
        if visible && body_bytes + source.content.len() <= 256_000 {
            body_bytes += source.content.len();
            value["content"] = json!(source.content);
        } else if visible {
            value["bodyAvailable"] = json!(false);
            value["withholdingReason"] = json!("response_body_budget");
            value["bodyRetrieval"] = json!("Query this messageId to retrieve the complete body.");
        } else {
            value["withholdingReason"] = json!("not_applied_to_model_input");
        }
        messages.push(value);
    }
    let has_more = messages.len() > query.limit;
    messages.truncate(query.limit);
    let next_cursor = if has_more {
        messages.last().map(|message| message["sequence"].clone())
    } else {
        None
    };
    Ok(
        json!({"available":true,"instanceId":identity.instance_id,"executionVersion":identity.execution_version,"nodeId":identity.node_id,"direction":query.direction,"messages":messages,"nextCursor":next_cursor}),
    )
}

#[cfg(test)]
mod tests;
