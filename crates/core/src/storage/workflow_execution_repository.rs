//! SQLite admission boundary for workflow sends, FIFO joins and idempotent delivery claims.
use crate::storage::now_ms;
use crate::workflow::{BusyPolicy, Definition, InputProcessingMode, Node, NodeConfig, Rule};
use crate::workflow_execution::*;
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

const MAX_MESSAGE_BYTES: usize = 128_000;
const MAX_DELIVERY_BYTES: usize = 1_000_000;
const MAX_PENDING_MESSAGES: i64 = 1024;
const MAX_PENDING_BYTES: i64 = 16 * 1024 * 1024;

fn db(error: impl std::fmt::Display) -> String {
    format!("Workflow execution storage: {error}")
}
fn json<T: serde::Serialize>(value: &T) -> Result<String, String> {
    serde_json::to_string(value).map_err(db)
}
fn parse<T: serde::de::DeserializeOwned>(value: &str) -> Result<T, String> {
    serde_json::from_str(value).map_err(db)
}
fn id() -> String {
    uuid::Uuid::new_v4().to_string()
}

struct Graph {
    instance_id: String,
    name: String,
    template_revision: u64,
    definition: Definition,
    bindings: BTreeMap<String, String>,
    execution_version: String,
    enabled: bool,
}
fn graph(c: &Connection, instance_id: &str) -> Result<Option<Graph>, String> {
    let row: Option<(String,String,u64,u64,bool,bool)> = c.query_row("SELECT i.name,d.definition_json,i.template_revision,d.revision,i.enabled,i.needs_review FROM workflow_instances i JOIN workflow_definitions d ON d.workflow_id=i.template_id WHERE i.instance_id=?1",[instance_id],|r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?))).optional().map_err(db)?;
    let Some((name, definition, template_revision, current_revision, enabled, needs_review)) = row
    else {
        return Ok(None);
    };
    let definition: Definition = parse(&definition)?;
    let mut statement=c.prepare("SELECT node_id,conversation_id FROM workflow_instance_bindings WHERE instance_id=?1 ORDER BY node_id").map_err(db)?;
    let bindings: BTreeMap<String, String> = statement
        .query_map([instance_id], |r| Ok((r.get(0)?, r.get(1)?)))
        .map_err(db)?
        .collect::<rusqlite::Result<_>>()
        .map_err(db)?;
    let version = json(&(instance_id, template_revision, &bindings))?;
    let execution_version = format!("{:x}", Sha256::digest(version.as_bytes()));
    // Model availability is a start-time concern; the published graph's structure must be valid.
    let models = definition
        .nodes
        .iter()
        .filter_map(|n| match &n.config {
            NodeConfig::Agent(a) => a.model_config_id.clone(),
            _ => None,
        })
        .collect();
    let valid = definition.validate(&models)?.is_empty();
    Ok(Some(Graph {
        instance_id: instance_id.into(),
        name,
        template_revision,
        definition,
        bindings,
        execution_version,
        enabled: enabled && !needs_review && template_revision == current_revision && valid,
    }))
}
fn node<'a>(graph: &'a Graph, id: &str) -> Result<&'a Node, String> {
    graph
        .definition
        .nodes
        .iter()
        .find(|n| n.id == id)
        .ok_or_else(|| "Workflow node is unavailable".into())
}
fn independent(c: &Connection, conversation_id: &str) -> Result<bool, String> {
    c.query_row("SELECT EXISTS(SELECT 1 FROM conversations c WHERE c.id=?1 AND c.archived_at IS NULL AND NOT EXISTS(SELECT 1 FROM agent_nodes n WHERE n.conversation_id=c.id AND n.parent_agent_id IS NOT NULL))",[conversation_id],|r|r.get(0)).map_err(db)
}
fn input_config(
    graph: &Graph,
    node_id: &str,
) -> Result<(InputProcessingMode, BusyPolicy, Vec<String>), String> {
    let incoming: Vec<_> = graph
        .definition
        .flows
        .iter()
        .filter(|f| f.target.node() == Some(node_id))
        .collect();
    if incoming.len() == 1 {
        if let Some(source) = incoming[0].source.node() {
            if let NodeConfig::InputGate {
                processing_mode,
                busy_policy,
            } = &node(graph, source)?.config
            {
                return Ok((
                    processing_mode.clone(),
                    busy_policy.clone(),
                    graph
                        .definition
                        .flows
                        .iter()
                        .filter(|f| f.target.node() == Some(source))
                        .map(|f| f.id.clone())
                        .collect(),
                ));
            }
        }
    }
    Ok((
        InputProcessingMode::Individual,
        BusyPolicy::Queue,
        incoming.iter().map(|f| f.id.clone()).collect(),
    ))
}
fn outlets(graph: &Graph, node_id: &str) -> Result<(Vec<Outlet>, Option<Rule>), String> {
    let direct: Vec<_> = graph
        .definition
        .flows
        .iter()
        .filter(|f| f.source.node() == Some(node_id))
        .collect();
    let (flows, prefix, rule) = if direct.len() == 1 {
        let target = node(
            graph,
            direct[0].target.node().ok_or("Workflow target missing")?,
        )?;
        if let NodeConfig::OutputGate { selection } = &target.config {
            (
                graph
                    .definition
                    .flows
                    .iter()
                    .filter(|f| f.source.node() == Some(target.id.as_str()))
                    .collect::<Vec<_>>(),
                vec![direct[0].id.clone()],
                Some(selection.clone()),
            )
        } else {
            (direct, vec![], None)
        }
    } else {
        (direct, vec![], None)
    };
    let mut result = vec![];
    for flow in flows {
        let mut target = node(graph, flow.target.node().ok_or("Workflow target missing")?)?;
        let mut path = prefix.clone();
        path.push(flow.id.clone());
        if matches!(target.config, NodeConfig::InputGate { .. }) {
            let binding = graph
                .definition
                .flows
                .iter()
                .find(|f| f.source.node() == Some(target.id.as_str()))
                .ok_or("Workflow input gate has no binding")?;
            path.push(binding.id.clone());
            target = node(
                graph,
                binding
                    .target
                    .node()
                    .ok_or("Workflow input gate target missing")?,
            )?;
        }
        if !target.is_participant() {
            return Err("Workflow exit does not resolve to a participant".into());
        }
        let (target_mode, target_busy, _) = input_config(graph, &target.id)?;
        result.push(Outlet {
            flow_id: flow.id.clone(),
            flow_name: flow.name.clone(),
            node_id: target.id.clone(),
            node_name: target.name.clone(),
            conversation_id: graph.bindings.get(&target.id).cloned(),
            path_flow_ids: path,
            input_rule: participant_input_rule(&target.config, &target_mode, &target_busy),
        });
    }
    Ok((result, rule))
}
fn participant_input_rule(
    config: &NodeConfig,
    mode: &InputProcessingMode,
    busy: &BusyPolicy,
) -> String {
    if matches!(config, NodeConfig::User { .. }) {
        let intake = match mode {
            InputProcessingMode::Individual => "Each incoming message forms one input.",
            InputProcessingMode::Batch => "The workflow waits for every incoming flow, then takes the oldest message from each to form one input. Remaining messages wait for the next batch.",
        };
        format!("{intake} A complete input waits for the user to confirm completion on their node. Confirmation only ends that wait and never sends a downstream message automatically; the user chooses any follow-up actions manually.")
    } else {
        describe_input_rule(mode, busy)
    }
}
fn snapshot(graph: &Graph, node_id: &str) -> Result<ConversationSnapshot, String> {
    let current = node(graph, node_id)?;
    let (receives, task, delivers) = match &current.config {
        NodeConfig::Agent(a) => (a.receives.clone(), a.task.clone(), a.delivers.clone()),
        NodeConfig::User { task } => (String::new(), task.clone(), String::new()),
        _ => return Err("A logic gate cannot own a conversation".into()),
    };
    let (outputs, rule) = outlets(graph, node_id)?;
    let (mode, busy, incoming) = input_config(graph, node_id)?;
    let mut predecessor_ids = BTreeSet::new();
    for incoming_id in incoming {
        let flow = graph
            .definition
            .flows
            .iter()
            .find(|f| f.id == incoming_id)
            .ok_or("Missing incoming flow")?;
        let Some(source_id) = flow.source.node() else {
            predecessor_ids.insert("__user_entry__".to_owned());
            continue;
        };
        let source = node(graph, source_id)?;
        if matches!(source.config, NodeConfig::OutputGate { .. }) {
            if let Some(parent) = graph
                .definition
                .flows
                .iter()
                .find(|f| f.target.node() == Some(source_id))
                .and_then(|f| f.source.node())
            {
                predecessor_ids.insert(parent.into());
            }
        } else {
            predecessor_ids.insert(source_id.into());
        }
    }
    let predecessors = predecessor_ids
        .into_iter()
        .map(|id| {
            if id == "__user_entry__" {
                return Ok(RelatedNode {
                    node_id: id,
                    node_name: "User entry".into(),
                    conversation_id: None,
                });
            }
            Ok(RelatedNode {
                node_name: node(graph, &id)?.name.clone(),
                conversation_id: graph.bindings.get(&id).cloned(),
                node_id: id,
            })
        })
        .collect::<Result<_, String>>()?;
    Ok(ConversationSnapshot {
        instance_id: graph.instance_id.clone(),
        name: graph.name.clone(),
        template_id: graph.definition.id.clone(),
        template_revision: graph.template_revision,
        execution_version: graph.execution_version.clone(),
        node_id: current.id.clone(),
        node_name: current.name.clone(),
        background: graph.definition.background.clone(),
        receives,
        task,
        delivers,
        predecessors,
        outputs,
        input_rule: participant_input_rule(&current.config, &mode, &busy),
        output_rule: describe_output_rule(rule.as_ref()),
        enabled: graph.enabled,
    })
}
pub fn snapshot_for_conversation(
    c: &Connection,
    conversation_id: &str,
) -> Result<Option<ConversationSnapshot>, String> {
    if !independent(c, conversation_id)? {
        return Ok(None);
    }
    let owner: Option<(String, String)> = c
        .query_row(
            "SELECT instance_id,node_id FROM workflow_instance_bindings WHERE conversation_id=?1",
            [conversation_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(db)?;
    let Some((instance_id, node_id)) = owner else {
        return Ok(None);
    };
    let Some(graph) = graph(c, &instance_id)?.filter(|g| g.enabled) else {
        return Ok(None);
    };
    snapshot(&graph, &node_id).map(Some)
}
fn event(
    c: &Connection,
    instance_id: &str,
    input_id: Option<&str>,
    flows: &[String],
    kind: &str,
) -> Result<(), String> {
    c.execute("INSERT INTO workflow_execution_events(instance_id,input_id,flow_ids_json,kind,created_at) VALUES (?1,?2,?3,?4,?5)",params![instance_id,input_id,json(&flows)?,kind,now_ms()]).map_err(db)?;
    c.execute("DELETE FROM workflow_execution_events WHERE instance_id=?1 AND sequence NOT IN (SELECT sequence FROM workflow_execution_events WHERE instance_id=?1 ORDER BY sequence DESC LIMIT 2000)",[instance_id]).map_err(db)?;
    Ok(())
}

pub fn send(c: &mut Connection, request: &SendRequest) -> Result<SendReceipt, String> {
    if request.source_run_id.is_empty()
        || request.tool_call_id.is_empty()
        || request.source_run_id.len() > 256
        || request.tool_call_id.len() > 256
    {
        return Err("Invalid workflow send identity".into());
    }
    if request.outputs.is_empty()
        || request.outputs.len() > 512
        || request.outputs.iter().any(|o| {
            o.message.trim().is_empty()
                || o.message.len() > MAX_MESSAGE_BYTES
                || o.message
                    .chars()
                    .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
        })
        || request
            .outputs
            .iter()
            .map(|o| o.message.len())
            .sum::<usize>()
            > MAX_DELIVERY_BYTES
    {
        return Err("Workflow send requires non-empty messages within the 128 KB per-message and 1 MB per-delivery limits".into());
    }
    let tx = c
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db)?;
    let request_json = json(request)?;
    let existing:Option<(String,String)>=tx.query_row("SELECT request_json,receipt_json FROM workflow_execution_sends WHERE source_run_id=?1 AND tool_call_id=?2",params![request.source_run_id,request.tool_call_id],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(db)?;
    if let Some((old, receipt)) = existing {
        if old != request_json {
            return Err("Workflow send identity was already used for a different delivery".into());
        }
        let mut receipt: SendReceipt = parse(&receipt)?;
        receipt.duplicate = true;
        return Ok(receipt);
    }
    let paused: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM workflow_execution_pauses WHERE conversation_id=?1)",
            [&request.conversation_id],
            |r| r.get(0),
        )
        .map_err(db)?;
    if paused {
        return Err("This conversation was stopped; workflow sending is paused until the user starts a new turn".into());
    }
    let source = snapshot_for_conversation(&tx, &request.conversation_id)?
        .ok_or("No enabled workflow is bound to this independent conversation")?;
    if source.execution_version != request.execution_version {
        return Err("Workflow template or bindings changed; wait for updated workflow context before sending".into());
    }
    let frozen:Option<String>=tx.query_row("SELECT snapshot_json FROM workflow_execution_runs WHERE run_id=?1 AND conversation_id=?2",params![request.source_run_id,request.conversation_id],|r|r.get(0)).optional().map_err(db)?;
    let frozen: Option<ConversationSnapshot> = frozen.as_deref().map(parse).transpose()?.flatten();
    if !frozen.as_ref().is_some_and(|v| {
        v.execution_version == request.execution_version
            && v.node_id == source.node_id
            && v.instance_id == source.instance_id
    }) {
        return Err("Workflow identity was not admitted for this run".into());
    }
    let active:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM conversation_turn_traces WHERE conversation_id=?1 AND run_id=?2 AND terminal_status='in_progress')",params![request.conversation_id,request.source_run_id],|r|r.get(0)).map_err(db)?;
    if !active {
        return Err("Workflow send requires the caller's current active conversation run".into());
    }
    let graph = graph(&tx, &source.instance_id)?.ok_or("Workflow deleted")?;
    let (outlets, rule) = outlets(&graph, &source.node_id)?;
    validate_selection(
        rule.as_ref(),
        &outlets
            .iter()
            .map(|o| o.flow_id.clone())
            .collect::<Vec<_>>(),
        &request
            .outputs
            .iter()
            .map(|o| o.flow_id.clone())
            .collect::<Vec<_>>(),
    )?;
    let (count,bytes):(i64,i64)=tx.query_row("SELECT COUNT(*),COALESCE(SUM(length(CAST(m.message_json AS BLOB))),0) FROM workflow_execution_messages m LEFT JOIN workflow_execution_inputs i ON i.input_id=m.input_id WHERE m.instance_id=?1 AND m.invalidated=0 AND (m.input_id IS NULL OR i.status IN ('pending','claimed','waiting_user','paused'))",[&source.instance_id],|r|Ok((r.get(0)?,r.get(1)?))).map_err(db)?;
    if count + request.outputs.len() as i64 > MAX_PENDING_MESSAGES
        || bytes + request_json.len() as i64 > MAX_PENDING_BYTES
    {
        return Err(
            "Workflow backlog is full; finish pending inputs before sending more messages".into(),
        );
    }
    let title: String = tx
        .query_row(
            "SELECT title FROM conversations WHERE id=?1",
            [&request.conversation_id],
            |r| r.get(0),
        )
        .map_err(db)?;
    let mut receipt = SendReceipt {
        id: id(),
        instance_id: source.instance_id.clone(),
        duplicate: false,
        messages: vec![],
        input_ids: vec![],
    };
    let mut targets = BTreeSet::new();
    for output in &request.outputs {
        let outlet = outlets
            .iter()
            .find(|o| o.flow_id == output.flow_id)
            .ok_or("Unknown workflow exit")?;
        let target = node(&graph, &outlet.node_id)?;
        if target.is_agent()
            && !outlet
                .conversation_id
                .as_deref()
                .map(|id| independent(&tx, id))
                .transpose()?
                .unwrap_or(false)
        {
            return Err("Workflow target is unbound or archived".into());
        }
        let message = SourceMessage {
            id: id(),
            instance_id: source.instance_id.clone(),
            workflow_name: source.name.clone(),
            source_node_id: source.node_id.clone(),
            source_node_name: source.node_name.clone(),
            source_conversation_id: request.conversation_id.clone(),
            source_conversation_title: title.clone(),
            target_node_id: outlet.node_id.clone(),
            flow_id: outlet.flow_id.clone(),
            path_flow_ids: outlet.path_flow_ids.clone(),
            content: output.message.clone(),
            created_at: now_ms(),
        };
        tx.execute("INSERT INTO workflow_execution_messages(message_id,instance_id,execution_version,node_id,flow_id,message_json,created_at) VALUES (?1,?2,?3,?4,?5,?6,?7)",params![message.id,source.instance_id,source.execution_version,outlet.node_id,outlet.flow_id,json(&message)?,message.created_at]).map_err(db)?;
        // A send reaches the input gate; the final gate-to-node segment moves only on delivery.
        let mut sent_path = message.path_flow_ids.clone();
        if graph
            .definition
            .flows
            .iter()
            .find(|f| f.id == outlet.flow_id)
            .and_then(|f| f.target.node())
            .and_then(|id| graph.definition.nodes.iter().find(|n| n.id == id))
            .is_some_and(|n| matches!(n.config, NodeConfig::InputGate { .. }))
        {
            sent_path.pop();
        }
        event(&tx, &source.instance_id, None, &sent_path, "sent")?;
        targets.insert(outlet.node_id.clone());
        receipt.messages.push(message);
    }
    for target in targets {
        receipt.input_ids.extend(form_inputs(&tx, &graph, &target)?);
    }
    tx.execute("INSERT INTO workflow_execution_sends(send_id,source_run_id,tool_call_id,source_conversation_id,instance_id,request_json,receipt_json,created_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",params![receipt.id,request.source_run_id,request.tool_call_id,request.conversation_id,source.instance_id,request_json,json(&receipt)?,now_ms()]).map_err(db)?;
    tx.commit().map_err(db)?;
    Ok(receipt)
}
fn form_inputs(c: &Connection, graph: &Graph, node_id: &str) -> Result<Vec<String>, String> {
    let (mode, busy, flows) = input_config(graph, node_id)?;
    let mut created = vec![];
    loop {
        let mut messages = vec![];
        match mode {
            InputProcessingMode::Individual => {
                let row:Option<String>=c.query_row("SELECT message_json FROM workflow_execution_messages WHERE instance_id=?1 AND execution_version=?2 AND node_id=?3 AND input_id IS NULL AND invalidated=0 ORDER BY sequence LIMIT 1",params![graph.instance_id,graph.execution_version,node_id],|r|r.get(0)).optional().map_err(db)?;
                if let Some(row) = row {
                    messages.push(parse::<SourceMessage>(&row)?);
                } else {
                    break;
                }
            }
            InputProcessingMode::Batch => {
                for flow in &flows {
                    let row:Option<String>=c.query_row("SELECT message_json FROM workflow_execution_messages WHERE instance_id=?1 AND execution_version=?2 AND node_id=?3 AND flow_id=?4 AND input_id IS NULL AND invalidated=0 ORDER BY sequence LIMIT 1",params![graph.instance_id,graph.execution_version,node_id,flow],|r|r.get(0)).optional().map_err(db)?;
                    if let Some(row) = row {
                        messages.push(parse::<SourceMessage>(&row)?);
                    } else {
                        return Ok(created);
                    }
                }
                if messages.is_empty() {
                    break;
                }
            }
        }
        let target = snapshot(graph, node_id)?;
        let conversation_id = graph.bindings.get(node_id).cloned();
        let paused = if let Some(chat) = &conversation_id {
            c.query_row(
                "SELECT EXISTS(SELECT 1 FROM workflow_execution_pauses WHERE conversation_id=?1)",
                [chat],
                |r| r.get::<_, bool>(0),
            )
            .map_err(db)?
        } else {
            false
        };
        let user = matches!(node(graph, node_id)?.config, NodeConfig::User { .. });
        let status = if user {
            InputStatus::WaitingUser
        } else if paused {
            InputStatus::Paused
        } else {
            InputStatus::Pending
        };
        let input = Input {
            id: id(),
            instance_id: graph.instance_id.clone(),
            node_id: node_id.into(),
            conversation_id,
            execution_version: graph.execution_version.clone(),
            content: assemble_message(&target, &messages),
            messages,
            busy_policy: busy.clone(),
            status,
            run_id: None,
            delivery_id: None,
            created_at: now_ms(),
            error: None,
        };
        c.execute("INSERT INTO workflow_execution_inputs(input_id,instance_id,execution_version,node_id,conversation_id,input_json,status,created_at,updated_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?8)",params![input.id,input.instance_id,input.execution_version,input.node_id,input.conversation_id,json(&input)?,status_name(&input.status),input.created_at]).map_err(db)?;
        for message in &input.messages {
            c.execute("UPDATE workflow_execution_messages SET input_id=?1 WHERE message_id=?2 AND input_id IS NULL",params![input.id,message.id]).map_err(db)?;
        }
        if user {
            event(
                c,
                &input.instance_id,
                Some(&input.id),
                &final_gate_flows(graph, &input),
                "waiting_user",
            )?;
        }
        created.push(input.id);
    }
    Ok(created)
}
fn final_gate_flows(graph: &Graph, input: &Input) -> Vec<String> {
    input
        .messages
        .iter()
        .filter_map(|m| m.path_flow_ids.last())
        .filter(|id| {
            graph
                .definition
                .flows
                .iter()
                .find(|f| &f.id == *id)
                .and_then(|f| f.source.node())
                .and_then(|id| graph.definition.nodes.iter().find(|n| n.id == id))
                .is_some_and(|n| matches!(n.config, NodeConfig::InputGate { .. }))
        })
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}
fn status_name(status: &InputStatus) -> &'static str {
    match status {
        InputStatus::Pending => "pending",
        InputStatus::Claimed => "claimed",
        InputStatus::Applied => "applied",
        InputStatus::WaitingUser => "waiting_user",
        InputStatus::Completed => "completed",
        InputStatus::Paused => "paused",
        InputStatus::Failed => "failed",
        InputStatus::Invalidated => "invalidated",
    }
}

mod delivery;
pub use delivery::*;
#[cfg(test)]
mod tests;
