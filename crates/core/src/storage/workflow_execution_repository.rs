//! Transactional organization mail ownership, delivery receipts and idempotent actions.
use crate::storage::now_ms;
use crate::workflow::{Definition, Node, NodeConfig};
use crate::workflow_execution::*;
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde_json::{json as value, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

const MAX_MESSAGE_BYTES: usize = 128_000;
const MAX_DELIVERY_BYTES: usize = 1_000_000;
const MAX_PENDING_MESSAGES: i64 = 1024;
const MAX_PENDING_BYTES: i64 = 16 * 1024 * 1024;
fn db(error: impl std::fmt::Display) -> String {
    format!("Organization mail storage: {error}")
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
    template_id: String,
    template_revision: u64,
    organization_revision: u64,
    definition: Definition,
    bindings: BTreeMap<String, String>,
    memberships: BTreeMap<String, String>,
    enabled: bool,
}
fn graph(c: &Connection, instance_id: &str) -> Result<Option<Graph>, String> {
    let row: Option<(String,String,String,u64,u64,bool)> = c.query_row("SELECT name,definition_json,template_id,template_revision,revision,enabled FROM workflow_instances WHERE instance_id=?1",[instance_id],|r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?))).optional().map_err(db)?;
    let Some((name, raw, template_id, template_revision, organization_revision, enabled)) = row
    else {
        return Ok(None);
    };
    let definition: Definition = parse(&raw)?;
    let mut statement=c.prepare("SELECT node_id,conversation_id,membership_id FROM workflow_instance_bindings WHERE instance_id=?1 ORDER BY node_id").map_err(db)?;
    let rows = statement
        .query_map([instance_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })
        .map_err(db)?;
    let mut bindings = BTreeMap::new();
    let mut memberships = BTreeMap::new();
    for row in rows {
        let (node, conversation, membership) = row.map_err(db)?;
        bindings.insert(node.clone(), conversation);
        memberships.insert(node, membership);
    }
    let models = definition
        .nodes
        .iter()
        .filter_map(|n| match &n.config {
            NodeConfig::Agent(a) => a.model_config_id.clone(),
        })
        .collect();
    let valid = definition.validate(&models)?.is_empty();
    Ok(Some(Graph {
        instance_id: instance_id.into(),
        name,
        template_id,
        template_revision,
        organization_revision,
        definition,
        bindings,
        memberships,
        enabled: enabled && valid,
    }))
}
/// Stable for this member incarnation. Other members, hierarchy and responsibilities can change
/// while this run continues; removing or rebinding this member permanently retires its authority.
fn membership_version(graph: &Graph, node_id: &str) -> Result<String, String> {
    Ok(format!(
        "{:x}",
        Sha256::digest(
            json(&(
                &graph.instance_id,
                node_id,
                graph.bindings.get(node_id),
                graph.memberships.get(node_id)
            ))?
            .as_bytes()
        )
    ))
}
fn node<'a>(graph: &'a Graph, id: &str) -> Result<&'a Node, String> {
    graph
        .definition
        .nodes
        .iter()
        .find(|n| n.id == id)
        .ok_or_else(|| "Organization node is unavailable".into())
}
fn independent(c: &Connection, conversation_id: &str) -> Result<bool, String> {
    c.query_row("SELECT EXISTS(SELECT 1 FROM conversations c WHERE c.id=?1 AND c.archived_at IS NULL AND NOT EXISTS(SELECT 1 FROM agent_nodes n WHERE n.conversation_id=c.id AND n.parent_agent_id IS NOT NULL))",[conversation_id],|r|r.get(0)).map_err(db)
}
fn is_paused(c: &Connection, conversation_id: &str) -> Result<bool, String> {
    c.query_row(
        "SELECT EXISTS(SELECT 1 FROM workflow_mail_pauses WHERE conversation_id=?1)",
        [conversation_id],
        |r| r.get(0),
    )
    .map_err(db)
}
fn snapshot(graph: &Graph, node_id: &str) -> Result<ConversationSnapshot, String> {
    let current = node(graph, node_id)?;
    let (receives, task, delivers) = match &current.config {
        NodeConfig::Agent(a) => (a.receives.clone(), a.task.clone(), a.delivers.clone()),
    };
    let members = graph
        .definition
        .nodes
        .iter()
        .map(|n| {
            Ok(RelatedNode {
                node_id: n.id.clone(),
                node_name: n.name.clone(),
                conversation_id: graph.bindings.get(&n.id).cloned(),
                membership_version: Some(membership_version(graph, &n.id)?),
                rank: n.rank,
                management_role: n.management_role.clone(),
                department_id: n.department_id.clone(),
                task: match &n.config {
                    NodeConfig::Agent(a) => a.task.chars().take(512).collect(),
                },
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(ConversationSnapshot {
        instance_id: graph.instance_id.clone(),
        name: graph.name.clone(),
        template_id: graph.template_id.clone(),
        template_revision: graph.template_revision,
        execution_version: membership_version(graph, node_id)?,
        node_id: node_id.into(),
        node_name: current.name.clone(),
        background: graph.definition.background.clone(),
        receives,
        task,
        delivers,
        members,
        enabled: graph.enabled,
        organization_revision: graph.organization_revision,
        rank: current.rank,
        management_role: current.management_role.clone(),
        department_id: current.department_id.clone(),
        departments: graph.definition.departments.clone(),
    })
}
pub fn snapshot_for_conversation(
    c: &Connection,
    conversation_id: &str,
) -> Result<Option<ConversationSnapshot>, String> {
    if !independent(c, conversation_id)? {
        return Ok(None);
    }
    let row:Option<(String,String)>=c.query_row("SELECT b.instance_id,b.node_id FROM workflow_instance_bindings b JOIN workflow_instances i ON i.instance_id=b.instance_id WHERE b.conversation_id=?1 AND i.enabled=1 AND i.needs_review=0 LIMIT 1",[conversation_id],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(db)?;
    let Some((instance, node_id)) = row else {
        return Ok(None);
    };
    let Some(graph) = graph(c, &instance)?.filter(|g| g.enabled) else {
        return Ok(None);
    };
    snapshot(&graph, &node_id).map(Some)
}
fn mutation_owner(
    c: &Connection,
    conversation_id: &str,
    run_id: &str,
    version: &str,
) -> Result<ConversationSnapshot, String> {
    if is_paused(c, conversation_id)? {
        return Err("This conversation was stopped; a new user turn is required before changing organization mail".into());
    }
    let identity = snapshot_for_run(c, conversation_id, run_id)?
        .ok_or("The current run has no active organization identity")?;
    if identity.execution_version != version {
        return Err(
            "Organization membership changed; wait for updated organization context".into(),
        );
    }
    let active:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM conversation_turn_traces WHERE conversation_id=?1 AND run_id=?2 AND terminal_status='in_progress')",params![conversation_id,run_id],|r|r.get(0)).map_err(db)?;
    if !active {
        return Err("Organization mutations require the current active conversation run".into());
    }
    Ok(identity)
}
fn event(
    c: &Connection,
    instance_id: &str,
    input_id: Option<&str>,
    message: Option<&SourceMessage>,
    kind: &str,
) -> Result<(), String> {
    let (source, target) = if kind == "recalled" {
        (
            message.map(|m| m.target_node_id.as_str()),
            message.map(|m| m.source_node_id.as_str()),
        )
    } else {
        (
            message.map(|m| m.source_node_id.as_str()),
            message.map(|m| m.target_node_id.as_str()),
        )
    };
    c.execute("INSERT INTO workflow_mail_events(instance_id,input_id,message_id,source_node_id,target_node_id,kind,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7)",params![instance_id,input_id,message.map(|m|&m.id),source,target,kind,now_ms()]).map_err(db)?;
    Ok(())
}
fn status_name(status: &InputStatus) -> &'static str {
    match status {
        InputStatus::Pending => "pending",
        InputStatus::Claimed => "claimed",
        InputStatus::Applied => "applied",
        InputStatus::Completed => "completed",
        InputStatus::Paused => "paused",
        InputStatus::Failed => "failed",
        InputStatus::Invalidated => "invalidated",
        InputStatus::Stopped => "stopped",
        InputStatus::Recalled => "recalled",
    }
}
fn current_mail_status(c: &Connection, input_id: &str) -> Result<MailStatus, String> {
    let raw: String = c
        .query_row(
            "SELECT mail_status FROM workflow_mail_messages WHERE input_id=?1",
            [input_id],
            |r| r.get(0),
        )
        .map_err(db)?;
    parse(&json(&raw)?)
}
fn write(c: &Connection, input: &Input) -> Result<(), String> {
    let current = current_mail_status(c, &input.id)?;
    if current.is_terminal() && current != input.mail_status {
        return Err("A settled organization message cannot change its result".into());
    }
    c.execute("UPDATE workflow_mail_inputs SET input_json=?1,status=?2,run_id=?3,delivery_id=?4,updated_at=?5,execution_version=?7 WHERE input_id=?6",params![json(input)?,status_name(&input.status),input.run_id,input.delivery_id,now_ms(),input.id,input.execution_version]).map_err(db)?;
    c.execute(
        "UPDATE workflow_mail_messages SET mail_status=?1 WHERE input_id=?2",
        params![input.mail_status.as_str(), input.id],
    )
    .map_err(db)?;
    Ok(())
}
pub fn send(c: &mut Connection, request: &SendRequest) -> Result<SendReceipt, String> {
    if request.messages.is_empty()
        || request.messages.len() > 128
        || request.messages.iter().any(|m| {
            m.message.trim().is_empty()
                || m.message.len() > MAX_MESSAGE_BYTES
                || m.message
                    .chars()
                    .any(|ch| ch.is_control() && !matches!(ch, '\n' | '\r' | '\t'))
        })
        || request
            .messages
            .iter()
            .map(|m| m.message.len())
            .sum::<usize>()
            > MAX_DELIVERY_BYTES
    {
        return Err("Organization send requires 1 to 128 messages within the 128 KB per-message and 1 MB total limits".into());
    }
    let tx = c
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db)?;
    if let Some(input) = &request.model_input {
        if let Some(crate::WorkflowMailReceipt::Send(mut receipt)) = mail_receipt_for_call(
            &tx,
            &request.conversation_id,
            &request.source_run_id,
            &request.tool_call_id,
            &crate::WorkflowMailReceiptCall::SemanticSend {
                input: input.clone(),
            },
        )? {
            receipt.duplicate = true;
            return Ok(receipt);
        }
    }
    let request_json = json(request)?;
    let existing:Option<(String,String)>=tx.query_row("SELECT request_json,receipt_json FROM workflow_mail_sends WHERE source_run_id=?1 AND tool_call_id=?2",params![request.source_run_id,request.tool_call_id],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(db)?;
    if let Some((old, receipt)) = existing {
        if old != request_json {
            return Err("Organization call identity was reused with different arguments".into());
        }
        let mut receipt: SendReceipt = parse(&receipt)?;
        receipt.duplicate = true;
        return Ok(receipt);
    }
    let source = mutation_owner(
        &tx,
        &request.conversation_id,
        &request.source_run_id,
        &request.execution_version,
    )?;
    let graph = graph(&tx, &source.instance_id)?.ok_or("Organization deleted")?;
    let (count,bytes):(i64,i64)=tx.query_row("SELECT COUNT(*),COALESCE(SUM(length(CAST(message_json AS BLOB))),0) FROM workflow_mail_messages WHERE instance_id=?1 AND mail_status IN ('pending','processing')",[&source.instance_id],|r|Ok((r.get(0)?,r.get(1)?))).map_err(db)?;
    if count + request.messages.len() as i64 > MAX_PENDING_MESSAGES
        || bytes + request_json.len() as i64 > MAX_PENDING_BYTES
    {
        return Err("Organization mailbox backlog is full".into());
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
    for output in &request.messages {
        if output.target_node_id == source.node_id {
            return Err("Choose another member of this organization".into());
        }
        let target = node(&graph, &output.target_node_id)?;
        if request.model_input.is_some()
            && request.recipient_versions.get(&target.id)
                != Some(&membership_version(&graph, &target.id)?)
        {
            return Err("The recipient changed since your directory was read. Check the updated member directory before sending.".into());
        }
        let recipient = graph.bindings.get(&target.id).cloned();
        if !recipient
            .as_deref()
            .map(|chat| independent(&tx, chat))
            .transpose()?
            .unwrap_or(false)
        {
            return Err("Organization target is unbound or archived".into());
        }
        if let Some(reply) = &output.reply_to_message_id {
            let related:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM workflow_mail_messages WHERE message_id=?1 AND instance_id=?2 AND ((recipient_conversation_id=?3 AND json_extract(message_json,'$.sourceNodeId')=?4) OR (json_extract(message_json,'$.sourceConversationId')=?3 AND node_id=?4)))",params![reply,source.instance_id,request.conversation_id,target.id],|r|r.get(0)).map_err(db)?;
            if !related {
                return Err("Reply reference is not a message exchanged with this member".into());
            }
        }
        let target_title = recipient
            .as_deref()
            .map(|chat| {
                tx.query_row("SELECT title FROM conversations WHERE id=?1", [chat], |r| {
                    r.get::<_, String>(0)
                })
                .map_err(db)
            })
            .transpose()?;
        let message = SourceMessage {
            id: id(),
            instance_id: source.instance_id.clone(),
            workflow_name: source.name.clone(),
            source_node_id: source.node_id.clone(),
            source_node_name: source.node_name.clone(),
            source_conversation_id: request.conversation_id.clone(),
            source_conversation_title: title.clone(),
            target_node_id: target.id.clone(),
            target_node_name: target.name.clone(),
            target_conversation_id: recipient.clone(),
            target_conversation_title: target_title,
            reply_to_message_id: output.reply_to_message_id.clone(),
            content: output.message.clone(),
            created_at: now_ms(),
        };
        let input = Input {
            id: id(),
            instance_id: source.instance_id.clone(),
            node_id: target.id.clone(),
            conversation_id: recipient,
            execution_version: membership_version(&graph, &target.id)?,
            content: assemble_message(
                &snapshot(&graph, &target.id)?,
                std::slice::from_ref(&message),
                MailDeliveryContext::Pending,
            ),
            messages: vec![message.clone()],
            mail_status: MailStatus::Pending,
            status: InputStatus::Pending,
            run_id: None,
            delivery_id: None,
            created_at: message.created_at,
            error: None,
        };
        tx.execute("INSERT INTO workflow_mail_inputs(input_id,instance_id,execution_version,node_id,conversation_id,input_json,status,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?8)",params![input.id,input.instance_id,input.execution_version,input.node_id,input.conversation_id,json(&input)?,status_name(&input.status),input.created_at]).map_err(db)?;
        tx.execute("INSERT INTO workflow_mail_messages(message_id,instance_id,execution_version,node_id,recipient_conversation_id,mail_status,message_json,input_id,created_at) VALUES(?1,?2,?3,?4,?5,'pending',?6,?7,?8)",params![message.id,message.instance_id,input.execution_version,message.target_node_id,message.target_conversation_id,json(&message)?,input.id,message.created_at]).map_err(db)?;
        event(
            &tx,
            &source.instance_id,
            Some(&input.id),
            Some(&message),
            "sent",
        )?;
        receipt.messages.push(message);
        receipt.input_ids.push(input.id);
    }
    tx.execute("INSERT INTO workflow_mail_sends(send_id,source_run_id,tool_call_id,source_conversation_id,instance_id,request_json,receipt_json,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",params![receipt.id,request.source_run_id,request.tool_call_id,request.conversation_id,source.instance_id,request_json,json(&receipt)?,now_ms()]).map_err(db)?;
    tx.commit().map_err(db)?;
    Ok(receipt)
}
pub fn mutate(c: &mut Connection, request: &MutationRequest) -> Result<Value, String> {
    let unique: BTreeSet<_> = request.message_ids.iter().collect();
    if unique.len() != request.message_ids.len()
        || unique.is_empty()
        || unique.len() > 50
        || unique.iter().any(|id| id.is_empty() || id.len() > 256)
    {
        return Err("Select 1 to 50 unique message IDs".into());
    }
    let tx = c
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db)?;
    let request_json = json(request)?;
    let existing:Option<(String,String)>=tx.query_row("SELECT request_json,receipt_json FROM workflow_mail_mutations WHERE source_run_id=?1 AND tool_call_id=?2",params![request.source_run_id,request.tool_call_id],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(db)?;
    if let Some((old, receipt)) = existing {
        if old != request_json {
            return Err("Organization call identity was reused with different arguments".into());
        }
        return parse(&receipt);
    }
    let owner = mutation_owner(
        &tx,
        &request.conversation_id,
        &request.source_run_id,
        &request.execution_version,
    )?;
    let mut results = vec![];
    for message_id in &request.message_ids {
        let row:Option<(String,String)>=tx.query_row("SELECT m.message_json,i.input_json FROM workflow_mail_messages m JOIN workflow_mail_inputs i ON i.input_id=m.input_id WHERE m.message_id=?1 AND m.instance_id=?2",params![message_id,owner.instance_id],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(db)?;
        let Some((raw, input_raw)) = row else {
            results.push(
                value!({"messageId":message_id,"success":false,"error":"Message is unavailable"}),
            );
            continue;
        };
        let message: SourceMessage = parse(&raw)?;
        let mut input: Input = parse(&input_raw)?;
        let owns = match request.action {
            MutationAction::Recall => {
                message.source_conversation_id == request.conversation_id
                    && message.source_node_id == owner.node_id
            }
            _ => {
                message.target_conversation_id.as_deref() == Some(&request.conversation_id)
                    && message.target_node_id == owner.node_id
            }
        };
        if !owns {
            results.push(
                value!({"messageId":message_id,"success":false,"error":"Message is unavailable"}),
            );
            continue;
        }
        input.mail_status = current_mail_status(&tx, &input.id)?;
        let changed = match request.action {
            MutationAction::Accept => {
                if input.mail_status == MailStatus::Processing
                    && input.run_id.as_deref() == Some(&request.source_run_id)
                {
                    Ok(false)
                } else if input.mail_status != MailStatus::Pending {
                    Err("Message is no longer pending")
                } else {
                    input.content = assemble_message(
                        &owner,
                        &input.messages,
                        MailDeliveryContext::AcceptedInCurrentTurn {
                            accepted_at: now_ms(),
                        },
                    );
                    input.execution_version = owner.execution_version.clone();
                    input.status = InputStatus::Claimed;
                    input.mail_status = MailStatus::Processing;
                    input.run_id = Some(request.source_run_id.clone());
                    input.delivery_id = Some(format!("workflow-message-{}", input.id));
                    input.error = None;
                    tx.execute("INSERT INTO workflow_mail_message_origins(message_id,conversation_id,input_id) VALUES(?1,?2,?3)",params![input.delivery_id,input.conversation_id,input.id]).map_err(db)?;
                    Ok(true)
                }
            }
            MutationAction::Complete => {
                if input.mail_status == MailStatus::Processed
                    && input.run_id.as_deref() == Some(&request.source_run_id)
                {
                    Ok(false)
                } else if input.mail_status != MailStatus::Processing
                    || input.run_id.as_deref() != Some(&request.source_run_id)
                {
                    Err("Only mail being handled by this turn can be completed")
                } else if !has_delivery_proof(&tx, &input)? {
                    Err("Mail has not reached the current model context yet")
                } else {
                    input.mail_status = MailStatus::Processed;
                    input.status = InputStatus::Completed;
                    input.error = None;
                    Ok(true)
                }
            }
            MutationAction::Recall => {
                if input.mail_status == MailStatus::Recalled {
                    Ok(false)
                } else if input.mail_status != MailStatus::Pending {
                    Err("Only pending mail can be recalled")
                } else {
                    input.mail_status = MailStatus::Recalled;
                    input.status = InputStatus::Recalled;
                    input.error = None;
                    Ok(true)
                }
            }
        };
        let mut result = serde_json::to_value(&message).map_err(db)?;
        result["messageId"] = value!(message.id);
        match changed {
            Ok(changed) => {
                if changed {
                    write(&tx, &input)?;
                    event(
                        &tx,
                        &owner.instance_id,
                        Some(&input.id),
                        Some(&message),
                        match request.action {
                            MutationAction::Accept => "accepted",
                            MutationAction::Complete => "completed",
                            MutationAction::Recall => "recalled",
                        },
                    )?;
                }
                result["success"] = value!(true);
                result["status"] = value!(input.mail_status);
                result["inputId"] = value!(input.id);
            }
            Err(error) => {
                result["success"] = value!(false);
                result["status"] = value!(input.mail_status);
                result["error"] = value!(error);
            }
        }
        results.push(result);
    }
    let receipt = value!({"action":request.action,"instanceId":owner.instance_id,"workflowName":owner.name,"messages":results});
    tx.execute("INSERT INTO workflow_mail_mutations(source_run_id,tool_call_id,request_json,receipt_json,created_at) VALUES(?1,?2,?3,?4,?5)",params![request.source_run_id,request.tool_call_id,request_json,json(&receipt)?,now_ms()]).map_err(db)?;
    tx.commit().map_err(db)?;
    Ok(receipt)
}

mod presentation;
pub use presentation::*;
mod delivery;
pub use delivery::*;
mod runtime_summary;
pub use runtime_summary::runtime_summary;
mod awareness;
pub use awareness::*;
mod receipts;
pub use receipts::*;
#[cfg(test)]
mod tests;
