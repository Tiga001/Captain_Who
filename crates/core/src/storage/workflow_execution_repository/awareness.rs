//! Read projections over immutable envelopes and their minimal processing state.
mod configuration;
mod request_observation;
mod summary;
use super::*;
use crate::workflow_awareness::{MailboxDirection, MailboxQuery, StateQuery, StateView};
pub use request_observation::request_observation;

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
fn department_ids(graph: &Graph, query: &StateQuery) -> Result<BTreeSet<String>, String> {
    let Some(selected) = query.department_id.as_deref() else {
        return Ok(graph
            .definition
            .departments
            .iter()
            .map(|d| d.id.clone())
            .collect());
    };
    if !graph
        .definition
        .departments
        .iter()
        .any(|d| d.id == selected)
    {
        return Err("Unknown organization department".into());
    }
    let mut ids = BTreeSet::from([selected.to_owned()]);
    if query.include_descendants {
        loop {
            let before = ids.len();
            for department in &graph.definition.departments {
                if department
                    .parent_id
                    .as_ref()
                    .is_some_and(|id| ids.contains(id))
                {
                    ids.insert(department.id.clone());
                }
            }
            if before == ids.len() {
                break;
            }
        }
    }
    Ok(ids)
}

fn selected_nodes<'a>(graph: &'a Graph, query: &StateQuery) -> Result<Vec<&'a Node>, String> {
    let departments = department_ids(graph, query)?;
    let search = query.search.as_deref().map(|s| s.trim().to_lowercase());
    let mut nodes = graph
        .definition
        .nodes
        .iter()
        .filter(|node| {
            let NodeConfig::Agent(config) = &node.config;
            query.node_id.as_deref().is_none_or(|id| id == node.id)
                && (query.department_id.is_none()
                    || node
                        .department_id
                        .as_ref()
                        .is_some_and(|id| departments.contains(id)))
                && search.as_ref().is_none_or(|text| {
                    [&node.name, &config.task, &config.receives, &config.delivers]
                        .iter()
                        .any(|field| field.to_lowercase().contains(text))
                })
        })
        .collect::<Vec<_>>();
    nodes.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.id.cmp(&b.id))
    });
    Ok(nodes)
}

fn members(nodes: &[&Node], graph: &Graph, focused: bool) -> Vec<Value> {
    nodes.iter().map(|n| {
        let NodeConfig::Agent(config) = &n.config;
        let mut result=value!({"nodeId":n.id,"nodeName":n.name,"conversationId":graph.bindings.get(&n.id),
            "rank":n.rank,"managementRole":n.management_role,"departmentId":n.department_id});
        if focused {
            result["receives"] = value!(config.receives);
            result["task"] = value!(config.task);
            result["delivers"] = value!(config.delivers);
            result["responsibilitiesAvailability"] = value!("complete");
        } else {
            let truncated = config.task.chars().count() > 512;
            result["task"] = value!(config.task.chars().take(512).collect::<String>());
            result["responsibilitiesAvailability"] = value!("summary");
            result["truncatedFields"] = value!(if truncated { vec!["task"] } else { vec![] });
            result["detailsQueryHint"] = value!("Query this member by name for full receives, task and delivers");
        }
        result
    }).collect()
}

fn department_path(graph: &Graph, id: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut cursor = Some(id);
    let mut visited = BTreeSet::new();
    while let Some(id) = cursor {
        if !visited.insert(id) {
            break;
        }
        let Some(department) = graph.definition.departments.iter().find(|d| d.id == id) else {
            break;
        };
        names.push(department.name.clone());
        cursor = department.parent_id.as_deref();
    }
    names.reverse();
    names
}

fn structure(graph: &Graph, query: &StateQuery) -> Result<Value, String> {
    let ids = department_ids(graph, query)?;
    let mut departments = graph
        .definition
        .departments
        .iter()
        .filter(|d| ids.contains(&d.id))
        .collect::<Vec<_>>();
    departments.sort_by_key(|d| (department_path(graph, &d.id).join("/"), d.id.clone()));
    let departments=departments.into_iter().map(|department| {
        let descendants=department_ids(graph,&StateQuery {department_id:Some(department.id.clone()),..Default::default()})?;
        let children=graph.definition.departments.iter().filter(|d| d.parent_id.as_deref()==Some(department.id.as_str()))
            .map(|d| department_path(graph,&d.id).join("/")).collect::<Vec<_>>();
        Ok(value!({"id":department.id,"name":department.name,"parentId":department.parent_id,
            "depth":department_path(graph,&department.id).len().saturating_sub(1),"childDepartments":children,
            "directMemberCount":graph.definition.nodes.iter().filter(|n| n.department_id.as_deref()==Some(department.id.as_str())).count(),
            "memberCount":graph.definition.nodes.iter().filter(|n| n.department_id.as_ref().is_some_and(|id| descendants.contains(id))).count()}))
    }).collect::<Result<Vec<_>,String>>()?;
    Ok(value!({"departments":departments,
        "rootMemberCount":if query.department_id.is_none() {graph.definition.nodes.iter().filter(|n| n.department_id.is_none()).count()} else {0},
        "countPolicy":"directMemberCount counts direct members; memberCount includes every descendant department"}))
}

fn participant_runtime(
    c: &Connection,
    graph: &Graph,
    node: &Node,
    query: &StateQuery,
) -> Result<Value, String> {
    let chat = graph.bindings.get(&node.id);
    let paused = chat
        .map(|id| is_paused(c, id))
        .transpose()?
        .unwrap_or(false);
    let mut counts = BTreeMap::<String, u64>::from_iter(
        [
            "pending",
            "processing",
            "processed",
            "stopped",
            "failed",
            "recalled",
        ]
        .map(|s| (s.into(), 0)),
    );
    let mut s=c.prepare("SELECT mail_status,COUNT(*) FROM workflow_mail_messages WHERE instance_id=?1 AND node_id=?2 AND recipient_conversation_id IS ?3 GROUP BY mail_status").map_err(db)?;
    for row in s
        .query_map(params![graph.instance_id, node.id, chat], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, u64>(1)?))
        })
        .map_err(db)?
    {
        let (status, count) = row.map_err(db)?;
        counts.insert(status, count);
    }
    let latest_run=c.query_row("SELECT t.run_id,t.terminal_status,t.created_at,t.updated_at,t.completed_at FROM conversation_turn_traces t JOIN workflow_mail_runs r ON r.run_id=t.run_id AND r.conversation_id=t.conversation_id WHERE t.conversation_id=?1 AND json_extract(r.snapshot_json,'$.instanceId')=?2 AND json_extract(r.snapshot_json,'$.nodeId')=?3 ORDER BY t.created_at DESC,t.rowid DESC LIMIT 1",params![chat,graph.instance_id,node.id],|r|Ok(value!({"runId":r.get::<_,String>(0)?,"status":r.get::<_,String>(1)?,"startedAt":r.get::<_,i64>(2)?,"updatedAt":r.get::<_,i64>(3)?,"completedAt":r.get::<_,Option<i64>>(4)?}))).optional().map_err(db)?;
    let pending = counts["pending"];
    let processing = counts["processing"];
    let state = if chat.is_none() {
        "unknown"
    } else if paused {
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
    let mut result = value!({"nodeId":node.id,"nodeName":node.name,"conversationId":chat,
        "rank":node.rank,"managementRole":node.management_role,"departmentId":node.department_id,
        "bindingAvailable":chat.is_some(),"state":state,"paused":paused,"waitingForApproval":false,
        "waitingForInteraction":false,"hasPendingInteraction":false,"activeRunId":null,
        "pendingCount":pending,"processingCount":processing,"mailCounts":counts,"latestRun":latest_run,
        "mailAvailability":if query.include_mail {"requested"} else {"not_requested"}});
    if query.include_mail {
        let mut s=c.prepare("SELECT m.sequence,i.input_id,m.message_id,m.mail_status,i.run_id,t.terminal_status,i.delivery_id,json_extract(i.input_json,'$.error'),json_extract(m.message_json,'$.sourceNodeName'),m.created_at FROM workflow_mail_messages m JOIN workflow_mail_inputs i ON i.input_id=m.input_id LEFT JOIN conversation_turn_traces t ON t.run_id=i.run_id AND t.conversation_id=i.conversation_id WHERE m.instance_id=?1 AND m.node_id=?2 AND m.recipient_conversation_id IS ?3 AND m.mail_status IN('pending','processing') AND (?4 IS NULL OR m.sequence>?4) ORDER BY m.sequence LIMIT ?5").map_err(db)?;
        let mut mail=s.query_map(params![graph.instance_id,node.id,chat,query.mail_cursor,query.limit+1],|r|Ok(value!({"sequence":r.get::<_,u64>(0)?,"inputId":r.get::<_,String>(1)?,"messageId":r.get::<_,String>(2)?,"status":r.get::<_,String>(3)?,"runId":r.get::<_,Option<String>>(4)?,"runStatus":r.get::<_,Option<String>>(5)?,"deliveryId":r.get::<_,Option<String>>(6)?,"error":r.get::<_,Option<String>>(7)?,"sourceNodeName":r.get::<_,Option<String>>(8)?,"createdAt":r.get::<_,i64>(9)?}))).map_err(db)?.collect::<rusqlite::Result<Vec<_>>>().map_err(db)?;
        let more = mail.len() > query.limit;
        mail.truncate(query.limit);
        let next = more.then(|| mail.last().unwrap()["sequence"].clone());
        result["mailPage"] = value!({"total":pending+processing,"returned":mail.len(),"limit":query.limit,
            "nextCursor":next,"truncated":more,"availability":if more {"truncated"} else {"complete"},
            "order":"arrival_sequence_ascending","followUp":"Use mailCursor=nextCursor with the same member, runtime view and includeMail=true. Message bodies are available only for letters in your own inbox or outbox via organization_get_mailbox."});
        result["inputs"] = value!(mail);
        result["inputsTruncated"] = value!(more);
    }
    Ok(result)
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
    let departments = department_ids(&graph, query)?;
    let selected = selected_nodes(&graph, query)?;
    let mut result = value!({"available":true,"view":query.view,"instanceId":identity.instance_id,
        "workflowName":identity.name,"executionVersion":identity.execution_version,"organizationRevision":identity.organization_revision,
        "currentNodeId":identity.node_id,"observedAt":now_ms(),
        "scope":{"nodeId":query.node_id,"nodeName":query.node_id.as_deref().and_then(|id| graph.definition.nodes.iter().find(|n| n.id==id).map(|n| &n.name)),
            "departmentId":query.department_id,"includeDescendants":query.include_descendants,
            "search":query.search,"status":query.status,"departmentCount":departments.len()},
        "_directory":{"members":graph.definition.nodes.iter().map(|n| value!({"nodeId":n.id,"nodeName":n.name})).collect::<Vec<_>>(),
            "departments":graph.definition.departments.iter().map(|d| value!({"id":d.id,"name":d.name,"parentId":d.parent_id})).collect::<Vec<_>>()}});
    match query.view {
        StateView::Configuration => {
            result["configuration"] = configuration::members(&tx, &graph, &identity, query)?
        }
        StateView::Members => {
            result["members"] = value!(members(&selected, &graph, query.node_id.is_some()))
        }
        StateView::Structure => result["structure"] = structure(&graph, query)?,
        StateView::Overview | StateView::Runtime => {
            // Intentionally unpaged: Host waiting/approval/compaction enrichment is authoritative
            // for state filters and overview aggregation at the final model boundary.
            result["runtime"] = value!({"nodes":selected.iter().map(|node| participant_runtime(&tx,&graph,node,query)).collect::<Result<Vec<_>,_>>()?});
        }
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
    let focused = query.message_id.is_some();
    let counts = mailbox_counts(&tx, &identity, conversation_id, inbox)?;
    let (messages, next) = mailbox_message_page(&tx, &identity, conversation_id, query)?;
    let mut result = value!({"available":true,"instanceId":identity.instance_id,
        "workflowName":identity.name,"nodeId":identity.node_id,"direction":query.direction,
        "observedAt":now_ms(),"view":if focused {"message"} else {"overview"},
        "counts":counts,"countsScope":"entire_selected_mailbox",
        "messages":messages,"nextCursor":next,
        "handlingRule":"Pending inbox entries are previews only, not accepted by this turn. Reading or paging does not change mail status. Before handling or replying to pending inbox mail in this turn, use organization_accept to assign it to this turn. Otherwise it remains pending and eligible for automatic delivery in a future turn. Sending a reply with organization_send does not accept or complete the original mail. Only claimed mail assigned to this turn can be completed. Processed mail will not be delivered again."});
    if inbox && !focused {
        result["history"] = mailbox_history_page(&tx, &identity, conversation_id, query)?;
    }
    Ok(result)
}

// Both recipient and sender identity include the original bound conversation. Current membership
// alone must not expose mail belonging to a previous occupant or a different organization.
fn mailbox_scope(inbox: bool) -> &'static str {
    if inbox {
        // Keep recipient predicates outside an OR so counts and pages can use the mailbox index.
        "m.instance_id=?1 AND ?2=1 AND m.node_id=?3 AND m.recipient_conversation_id=?4"
    } else {
        "m.instance_id=?1 AND ?2=0 AND json_extract(m.message_json,'$.sourceNodeId')=?3
            AND json_extract(m.message_json,'$.sourceConversationId')=?4"
    }
}

fn mailbox_counts(
    c: &Connection,
    identity: &ConversationSnapshot,
    conversation_id: &str,
    inbox: bool,
) -> Result<Value, String> {
    let scope = mailbox_scope(inbox);
    let mut counts = value!({"total":0,"pending":0,"processing":0,"processed":0,
        "stopped":0,"failed":0,"recalled":0});
    let mut statement = c
        .prepare(&format!(
            "SELECT m.mail_status,COUNT(*) FROM workflow_mail_messages m
         WHERE {scope} GROUP BY m.mail_status"
        ))
        .map_err(db)?;
    let mut total = 0_u64;
    for row in statement
        .query_map(
            params![
                identity.instance_id,
                inbox,
                identity.node_id,
                conversation_id
            ],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, u64>(1)?)),
        )
        .map_err(db)?
    {
        let (status, count) = row.map_err(db)?;
        counts[status] = value!(count);
        total += count;
    }
    counts["total"] = value!(total);
    Ok(counts)
}

fn mailbox_message_page(
    c: &Connection,
    identity: &ConversationSnapshot,
    conversation_id: &str,
    query: &MailboxQuery,
) -> Result<(Vec<Value>, Option<u64>), String> {
    let inbox = query.direction == MailboxDirection::Inbox;
    let scope = mailbox_scope(inbox);
    let focused = query.message_id.is_some();
    // A precise lookup is independent of a previous page/status filter: the mail may have
    // changed state since its index was read. Mailbox ownership remains mandatory.
    let cursor = if focused { None } else { query.cursor };
    let input_id = if focused {
        None
    } else {
        query.input_id.as_deref()
    };
    let status = if focused {
        None
    } else {
        query.status.map(|s| s.as_str())
    };
    let limit = if focused { 1 } else { query.limit };
    let mut s = c.prepare(&format!(
        "SELECT m.sequence,m.message_json,m.mail_status,m.input_id,i.run_id,t.terminal_status,
                json_extract(i.input_json,'$.error'),i.delivery_id
         FROM workflow_mail_messages m
         LEFT JOIN workflow_mail_inputs i ON i.input_id=m.input_id
         LEFT JOIN conversation_turn_traces t ON t.run_id=i.run_id AND t.conversation_id=i.conversation_id
         WHERE {scope}
           AND (?5 IS NULL OR m.sequence<?5) AND (?6 IS NULL OR m.message_id=?6)
           AND (?7 IS NULL OR m.input_id=?7) AND (?9 IS NULL OR m.mail_status=?9)
           AND (?10=0 OR m.mail_status IN ('pending','processing'))
         ORDER BY m.sequence DESC LIMIT ?8"
    )).map_err(db)?;
    let mut messages = vec![];
    let mut bytes = 0;
    for row in s
        .query_map(
            params![
                identity.instance_id,
                inbox,
                identity.node_id,
                conversation_id,
                cursor,
                query.message_id,
                input_id,
                limit + 1,
                status,
                inbox && !focused
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
        if messages.len() == limit {
            let next = messages
                .last()
                .and_then(|item: &Value| item["sequence"].as_u64());
            return Ok((messages, next));
        }
        let message: SourceMessage = parse(&raw)?;
        let mut item = serde_json::to_value(&message).map_err(db)?;
        item["messageId"] = value!(message.id);
        item["sequence"] = value!(sequence);
        item["status"] = value!(status);
        if inbox && status == "pending" {
            item["deliveryStatus"] = value!("preview_only_not_accepted");
        }
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
    Ok((messages, None))
}

fn mailbox_history_page(
    c: &Connection,
    identity: &ConversationSnapshot,
    conversation_id: &str,
    query: &MailboxQuery,
) -> Result<Value, String> {
    let scope = mailbox_scope(true);
    let filter = format!(
        "{scope} AND m.mail_status IN ('processed','stopped','failed','recalled')
         AND (?5 IS NULL OR m.input_id=?5) AND (?6 IS NULL OR m.mail_status=?6)"
    );
    let status = query.status.map(|s| s.as_str());
    let total: u64 = c
        .query_row(
            &format!("SELECT COUNT(*) FROM workflow_mail_messages m WHERE {filter}"),
            params![
                identity.instance_id,
                true,
                identity.node_id,
                conversation_id,
                query.input_id,
                status
            ],
            |r| r.get(0),
        )
        .map_err(db)?;
    // Select only index fields; never deserialize complete historical envelopes to build a list.
    let mut statement = c.prepare(&format!(
        "SELECT m.sequence,m.message_id,json_extract(m.message_json,'$.sourceNodeName'),m.created_at,m.mail_status
         FROM workflow_mail_messages m WHERE {filter}
           AND (?7 IS NULL OR m.sequence<?7) ORDER BY m.sequence DESC LIMIT ?8"
    )).map_err(db)?;
    let mut rows = statement
        .query_map(
            params![
                identity.instance_id,
                true,
                identity.node_id,
                conversation_id,
                query.input_id,
                status,
                query.history_cursor,
                query.limit + 1
            ],
            |r| {
                Ok((
                    r.get::<_, u64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, String>(4)?,
                ))
            },
        )
        .map_err(db)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(db)?;
    let more = rows.len() > query.limit;
    rows.truncate(query.limit);
    let next = more.then(|| rows.last().unwrap().0);
    let messages = rows.into_iter().map(|(_,id,source,created_at,status)|value!({
        "messageId":id,"sourceNodeName":source,"createdAt":created_at,"status":status,
        "bodyRetrieval":{"tool":"organization_get_mailbox","arguments":{"direction":"inbox","messageId":id}}
    })).collect::<Vec<_>>();
    Ok(value!({"total":total,"messages":messages,"nextCursor":next}))
}
#[cfg(test)]
mod tests;
