//! Read projections over immutable envelopes and their minimal processing state.
mod configuration;
mod summary;
use super::*;
use crate::workflow_awareness::{MailboxDirection, MailboxQuery, StateQuery, StateView};

fn scope(
    c: &Connection,
    conversation_id: &str,
    run_id: &str,
) -> Result<(Graph, ConversationSnapshot), String> {
    let identity = snapshot_for_run(c, conversation_id, run_id)?
        .ok_or("The current run has no active organization identity")?;
    let graph = graph(c, &identity.instance_id)?.ok_or("Organization is unavailable")?;
    Ok((graph, identity))
}
fn members(graph: &Graph, selected: Option<&str>) -> Vec<Value> {
    graph.definition.nodes.iter().filter(|n|selected.is_none_or(|id|id==n.id)).map(|n|{
        let NodeConfig::Agent(a) = &n.config; let (receives,task,delivers)=(a.receives.as_str(),a.task.as_str(),a.delivers.as_str());
        let mut result=value!({"nodeId":n.id,"nodeName":n.name,"kind":"agent","conversationId":graph.bindings.get(&n.id),"rank":n.rank,"managementRole":n.management_role,"departmentId":n.department_id,"receives":receives,"task":task,"delivers":delivers});
        if selected.is_none(){let mut truncated=vec![];for key in ["receives","task","delivers"]{if let Some(text)=result[key].as_str(){if text.chars().count()>512{let preview:String=text.chars().take(512).collect();result[key]=value!(preview);truncated.push(key);}}}if !truncated.is_empty(){result["truncatedFields"]=value!(truncated);result["detailsQueryHint"]=value!("Query this nodeId for full responsibilities");}}
        result
    }).collect()
}
fn participant_runtime(c: &Connection, graph: &Graph, node: &Node) -> Result<Value, String> {
    let chat = graph.bindings.get(&node.id);
    let paused = chat
        .map(|id| is_paused(c, id))
        .transpose()?
        .unwrap_or(false);
    let mut counts = BTreeMap::<String, u64>::new();
    let mut s=c.prepare("SELECT mail_status,COUNT(*) FROM workflow_mail_messages WHERE instance_id=?1 AND node_id=?2 AND recipient_conversation_id IS ?3 GROUP BY mail_status").map_err(db)?;
    for row in s
        .query_map(params![graph.instance_id, node.id, chat], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, u64>(1)?))
        })
        .map_err(db)?
    {
        let (k, v) = row.map_err(db)?;
        counts.insert(k, v);
    }
    let mut s=c.prepare("SELECT i.input_id,m.message_id,m.mail_status,i.run_id,t.terminal_status,i.delivery_id,json_extract(i.input_json,'$.error') FROM workflow_mail_messages m JOIN workflow_mail_inputs i ON i.input_id=m.input_id LEFT JOIN conversation_turn_traces t ON t.run_id=i.run_id AND t.conversation_id=i.conversation_id WHERE m.instance_id=?1 AND m.node_id=?2 AND m.recipient_conversation_id IS ?3 AND m.mail_status IN('pending','processing') ORDER BY (m.mail_status='processing') DESC,m.sequence LIMIT 128").map_err(db)?;
    let inputs=s.query_map(params![graph.instance_id,node.id,chat],|r|Ok(value!({"inputId":r.get::<_,String>(0)?,"messageId":r.get::<_,String>(1)?,"status":r.get::<_,String>(2)?,"runId":r.get::<_,Option<String>>(3)?,"runStatus":r.get::<_,Option<String>>(4)?,"deliveryId":r.get::<_,Option<String>>(5)?,"error":r.get::<_,Option<String>>(6)?}))).map_err(db)?.collect::<rusqlite::Result<Vec<_>>>().map_err(db)?;
    let latest_run=c.query_row("SELECT t.run_id,t.terminal_status FROM conversation_turn_traces t JOIN workflow_mail_runs r ON r.run_id=t.run_id AND r.conversation_id=t.conversation_id WHERE t.conversation_id=?1 AND json_extract(r.snapshot_json,'$.instanceId')=?2 AND json_extract(r.snapshot_json,'$.nodeId')=?3 ORDER BY t.created_at DESC,t.rowid DESC LIMIT 1",params![chat,graph.instance_id,node.id],|r|Ok(value!({"runId":r.get::<_,String>(0)?,"status":r.get::<_,String>(1)?}))).optional().map_err(db)?;
    let pending = counts.get("pending").copied().unwrap_or(0);
    let processing = counts.get("processing").copied().unwrap_or(0);
    let current: Vec<_> = inputs
        .iter()
        .filter(|i| i["status"] == "processing")
        .map(|i| i["inputId"].clone())
        .collect();
    let state = if paused {
        "stopped"
    } else if processing > 0
        || latest_run
            .as_ref()
            .is_some_and(|r| r["status"] == "in_progress")
    {
        "running"
    } else if pending > 0 {
        "queued"
    } else {
        "idle"
    };
    Ok(
        value!({"nodeId":node.id,"nodeName":node.name,"kind":"agent","conversationId":chat,"state":state,"paused":paused,"pendingCount":pending,"processingCount":processing,"queuedInputCount":pending,"currentInputCount":processing,"currentInputIds":current,"mailCounts":counts,"inputs":inputs,"inputsTruncated":pending+processing>128,"latestRun":latest_run}),
    )
}
fn runtime(c: &Connection, graph: &Graph, selected: Option<&str>) -> Result<Vec<Value>, String> {
    graph
        .definition
        .nodes
        .iter()
        .filter(|n| selected.is_none_or(|id| id == n.id))
        .map(|n| participant_runtime(c, graph, n))
        .collect()
}
pub fn state_for_run(
    c: &Connection,
    conversation_id: &str,
    run_id: &str,
    query: &StateQuery,
) -> Result<Value, String> {
    query.validate()?;
    let tx = c.unchecked_transaction().map_err(db)?;
    let (graph, identity) = scope(&tx, conversation_id, run_id)?;
    if let Some(id) = &query.node_id {
        node(&graph, id)?;
    }
    let mut result = value!({"available":true,"instanceId":identity.instance_id,"workflowName":identity.name,"description":graph.definition.description,"executionVersion":identity.execution_version,"organizationRevision":identity.organization_revision,"currentNodeId":identity.node_id,"observedAt":now_ms()});
    if query.view == StateView::Configuration {
        result["configuration"] =
            configuration::members(&tx, &graph, &identity, query.node_id.as_deref())?;
        return Ok(result);
    }
    if query.view != StateView::Runtime {
        result["members"] = value!(members(&graph, query.node_id.as_deref()));
        result["departments"] = value!(graph.definition.departments.iter().map(|department| value!({"id":department.id,"name":department.name,"parentId":department.parent_id})).collect::<Vec<_>>());
    }
    if query.view != StateView::Members {
        result["runtime"] = value!({"nodes":runtime(&tx,&graph,query.node_id.as_deref())?});
    }
    Ok(result)
}
fn awareness(
    c: &Connection,
    graph: &Graph,
    identity: &ConversationSnapshot,
) -> Result<Value, String> {
    let nodes = summary::nodes(c, graph, &identity.node_id)?;
    let current = nodes
        .iter()
        .find(|node| node["nodeId"] == identity.node_id)
        .ok_or("Organization summary omitted the current member")?;
    let chat = graph.bindings.get(&identity.node_id);
    let (latest,received):(u64,u64)=c.query_row("SELECT COALESCE(MAX(sequence),0),COUNT(*) FROM workflow_mail_messages WHERE instance_id=?1 AND node_id=?2 AND recipient_conversation_id IS ?3",params![graph.instance_id,identity.node_id,chat],|r|Ok((r.get(0)?,r.get(1)?))).map_err(db)?;
    let mut s=c.prepare("SELECT sequence,message_id,json_extract(message_json,'$.sourceNodeId'),json_extract(message_json,'$.sourceNodeName') FROM workflow_mail_messages WHERE instance_id=?1 AND node_id=?2 AND recipient_conversation_id IS ?3 ORDER BY sequence DESC LIMIT 5").map_err(db)?;
    let arrivals=s.query_map(params![graph.instance_id,identity.node_id,chat],|r|Ok(value!({"sequence":r.get::<_,u64>(0)?,"messageId":r.get::<_,String>(1)?,"sourceNodeId":r.get::<_,String>(2)?,"sourceNodeName":r.get::<_,String>(3)?}))).map_err(db)?.collect::<rusqlite::Result<Vec<_>>>().map_err(db)?;
    let mut s=c.prepare("SELECT message_id,node_id,mail_status FROM workflow_mail_messages WHERE instance_id=?1 AND json_extract(message_json,'$.sourceNodeId')=?2 AND json_extract(message_json,'$.sourceConversationId')=?3 ORDER BY sequence DESC LIMIT 3").map_err(db)?;
    let sent=s.query_map(params![graph.instance_id,identity.node_id,chat],|r|Ok(value!({"messageId":r.get::<_,String>(0)?,"targetNodeId":r.get::<_,String>(1)?,"status":r.get::<_,String>(2)?}))).map_err(db)?.collect::<rusqlite::Result<Vec<_>>>().map_err(db)?;
    Ok(
        value!({"available":true,"instanceId":identity.instance_id,"executionVersion":identity.execution_version,"organizationRevision":identity.organization_revision,"currentNodeId":identity.node_id,"currentInputIds":current["currentInputIds"],"currentInputCount":current["currentInputCount"],"nodes":nodes,"totalNodeCount":graph.definition.nodes.len(),"nodesTruncated":graph.definition.nodes.len()>16,"recentSent":sent,"mailbox":{"latestSequence":latest,"receivedCount":received,"pendingCount":current["pendingCount"],"processingCount":current["processingCount"],"recentArrivals":arrivals},"queryHint":"Use organization_get_state for member responsibilities or current runtime details. Use organization_get_mailbox to preview pending mail. Reading does not accept mail; organization_accept assigns selected mail to this turn."}),
    )
}
pub fn awareness_for_run(
    c: &Connection,
    conversation_id: &str,
    run_id: &str,
) -> Result<Value, String> {
    let tx = c.unchecked_transaction().map_err(db)?;
    let (graph, identity) = scope(&tx, conversation_id, run_id)?;
    awareness(&tx, &graph, &identity)
}
pub fn awareness_for_conversation(c: &Connection, conversation_id: &str) -> Result<Value, String> {
    let tx = c.unchecked_transaction().map_err(db)?;
    let Some(identity) = snapshot_for_conversation(&tx, conversation_id)? else {
        return Ok(value!({"available":false,"reason":"not_active_or_unavailable"}));
    };
    let graph = graph(&tx, &identity.instance_id)?.ok_or("Organization is unavailable")?;
    awareness(&tx, &graph, &identity)
}
pub fn mailbox_for_run(
    c: &Connection,
    conversation_id: &str,
    run_id: &str,
    query: &MailboxQuery,
) -> Result<Value, String> {
    query.validate()?;
    let tx = c.unchecked_transaction().map_err(db)?;
    let (_, identity) = scope(&tx, conversation_id, run_id)?;
    let inbox = query.direction == MailboxDirection::Inbox;
    let mut s=tx.prepare("SELECT m.sequence,m.message_json,m.mail_status,m.input_id,i.run_id,t.terminal_status,json_extract(i.input_json,'$.error'),i.delivery_id FROM workflow_mail_messages m LEFT JOIN workflow_mail_inputs i ON i.input_id=m.input_id LEFT JOIN conversation_turn_traces t ON t.run_id=i.run_id AND t.conversation_id=i.conversation_id WHERE m.instance_id=?1 AND ((?2=1 AND m.node_id=?3 AND m.recipient_conversation_id=?4) OR (?2=0 AND json_extract(m.message_json,'$.sourceNodeId')=?3 AND json_extract(m.message_json,'$.sourceConversationId')=?4)) AND (?5 IS NULL OR m.sequence<?5) AND (?6 IS NULL OR m.message_id=?6) AND (?7 IS NULL OR m.input_id=?7) AND (?9 IS NULL OR m.mail_status=?9) ORDER BY m.sequence DESC LIMIT ?8").map_err(db)?;
    let mut messages = vec![];
    let mut bytes = 0;
    for row in s
        .query_map(
            params![
                identity.instance_id,
                inbox,
                identity.node_id,
                conversation_id,
                query.cursor,
                query.message_id,
                query.input_id,
                query.limit + 1,
                query.status.map(|s| s.as_str())
            ],
            |r| {
                Ok((
                    r.get::<_, u64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, Option<String>>(4)?,
                    r.get::<_, Option<String>>(5)?,
                    r.get::<_, Option<String>>(6)?,
                    r.get::<_, Option<String>>(7)?,
                ))
            },
        )
        .map_err(db)?
    {
        let (sequence, raw, status, input_id, processing_run, run_status, error, delivery_id) =
            row.map_err(db)?;
        let message: SourceMessage = parse(&raw)?;
        let mut item = serde_json::to_value(&message).map_err(db)?;
        item["messageId"] = value!(message.id);
        item["sequence"] = value!(sequence);
        item["status"] = value!(status);
        item["inputId"] = value!(input_id);
        item["runId"] = value!(processing_run);
        item["runStatus"] = value!(run_status);
        item["error"] = value!(error);
        item["deliveryId"] = value!(delivery_id);
        if bytes + message.content.len() <= 256_000 {
            bytes += message.content.len();
            item["bodyAvailable"] = value!(true);
        } else {
            item.as_object_mut().unwrap().remove("content");
            item["bodyAvailable"] = value!(false);
            item["withholdingReason"] = value!("response_body_budget");
            item["bodyRetrieval"] = value!("Query this messageId for its complete body");
        }
        messages.push(item);
    }
    let more = messages.len() > query.limit;
    messages.truncate(query.limit);
    let next = more.then(|| messages.last().unwrap()["sequence"].clone());
    Ok(
        value!({"available":true,"instanceId":identity.instance_id,"workflowName":identity.name,"nodeId":identity.node_id,"direction":query.direction,"observedAt":now_ms(),"messages":messages,"nextCursor":next,"handlingRule":"Reading pending mail does not accept it. It remains eligible to wake a future turn; use organization_accept to handle it now. Only claimed mail assigned to this turn can be completed. Processed mail will not be delivered again."}),
    )
}
#[cfg(test)]
mod tests;
