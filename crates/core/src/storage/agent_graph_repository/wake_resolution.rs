use super::common::{
    conflict, corrupt, read_error, validate_id, validate_request_id, validate_time,
};
use super::message_records::{query_message, query_message_by_request};
use super::node_records::{query_node, query_node_by_request};
use super::nodes::is_strict_descendant;
use super::wake_records::{
    decode_wake, query_wake, query_wake_by_request, read_wake_row, WAKE_SELECT,
};
use crate::{
    AgentCollaborationIdentity, AgentGraphError, AgentLifecycle, AgentMailboxDeliveryStatus,
    AgentMailboxKind, AgentWakeStatus, ChildAgentSpawnRecord, TrustedActiveChildWakeBundle,
};
use rusqlite::{params, Connection};

/// Reconstructs and verifies the durable facts required to execute a child Wake.
///
/// The caller supplies identities already read from a trusted Host request. Model input and task
/// content always come back from SQLite; neither can be supplied by a model-facing argument.
pub(crate) fn resolve_child_wake_bundle(
    connection: &Connection,
    agent_id: &str,
    wake_id: &str,
    source_agent_message_id: &str,
) -> Result<ChildAgentSpawnRecord, AgentGraphError> {
    resolve_child_bundle(connection, agent_id, wake_id, source_agent_message_id, true)
}

pub(crate) fn resolve_running_child_wake_bundle(
    connection: &Connection,
    agent_id: &str,
    wake_id: &str,
    source_agent_message_id: &str,
    claim_token: &str,
    observed_at: i64,
) -> Result<ChildAgentSpawnRecord, AgentGraphError> {
    validate_id("claim_token", claim_token)?;
    validate_time(observed_at)?;
    let bundle =
        resolve_child_bundle(connection, agent_id, wake_id, source_agent_message_id, true)?;
    if bundle.initial_wake.status != AgentWakeStatus::Running
        || bundle.initial_wake.claim_token.as_deref() != Some(claim_token)
        || bundle
            .initial_wake
            .lease_expires_at
            .is_none_or(|lease_expires_at| lease_expires_at <= observed_at)
    {
        return Err(conflict(
            "child Wake execution requires the exact running claim with an unexpired lease",
        ));
    }
    Ok(bundle)
}

/// Resolves the exact durable capability consumed by atomic Turn admission. Dispatcher owns only
/// a claimed Wake; the shared Turn transaction changes it to running while binding run IDs.
pub(crate) fn resolve_claimed_agent_wake_bundle(
    connection: &Connection,
    agent_id: &str,
    wake_id: &str,
    source_agent_message_id: &str,
    claim_token: &str,
    observed_at: i64,
) -> Result<ChildAgentSpawnRecord, AgentGraphError> {
    validate_id("claim_token", claim_token)?;
    validate_time(observed_at)?;
    let bundle =
        resolve_child_bundle(connection, agent_id, wake_id, source_agent_message_id, true)?;
    if bundle.initial_wake.status != AgentWakeStatus::Claimed
        || bundle.initial_wake.claim_token.as_deref() != Some(claim_token)
        || bundle
            .initial_wake
            .lease_expires_at
            .is_none_or(|lease_expires_at| lease_expires_at <= observed_at)
    {
        return Err(conflict(
            "Agent Wake execution requires the exact claimed capability with an unexpired lease",
        ));
    }
    Ok(bundle)
}

/// Recovers the exact active Wake capability carried across an approval checkpoint.
///
/// The checkpoint contributes only the previously authenticated collaboration identity. Wake and
/// claim identities are re-read from SQLite, must be unique and active, and are never accepted
/// from renderer or model input.
pub(crate) fn resolve_active_child_wake_bundle_by_identity(
    connection: &Connection,
    identity: &AgentCollaborationIdentity,
    observed_at: i64,
) -> Result<TrustedActiveChildWakeBundle, AgentGraphError> {
    identity
        .validate()
        .map_err(|error| AgentGraphError::InvalidInput {
            field: "collaboration_identity",
            reason: error.to_string(),
        })?;
    validate_time(observed_at)?;
    let mut statement = connection
        .prepare(&format!(
            "{WAKE_SELECT}
             WHERE agent_id = ?1 AND source_agent_message_id = ?2
               AND status IN ('running', 'waiting_for_approval')
             ORDER BY sequence LIMIT 2"
        ))
        .map_err(read_error)?;
    let rows = statement
        .query_map(
            params![&identity.agent_id, &identity.source_agent_message_id],
            read_wake_row,
        )
        .map_err(read_error)?;
    let wakes = rows
        .map(|row| row.map_err(read_error).and_then(decode_wake))
        .collect::<Result<Vec<_>, _>>()?;
    let [wake] = wakes.as_slice() else {
        return Err(conflict(
            "collaboration identity must resolve to exactly one active child Wake",
        ));
    };
    let claim_token = wake
        .claim_token
        .as_deref()
        .ok_or_else(|| corrupt("active child Wake is missing its claim token"))?;
    if wake
        .lease_expires_at
        .is_none_or(|lease_expires_at| lease_expires_at <= observed_at)
    {
        return Err(conflict("active child Wake lease has expired"));
    }
    let spawn = resolve_child_bundle(
        connection,
        &identity.agent_id,
        &wake.wake_id,
        &identity.source_agent_message_id,
        true,
    )?;
    if &spawn.collaboration_identity != identity
        || spawn.agent.conversation_id != identity.conversation_id
    {
        return Err(conflict(
            "checkpoint collaboration identity does not match durable child facts",
        ));
    }
    Ok(TrustedActiveChildWakeBundle {
        spawn,
        claim_token: claim_token.to_string(),
    })
}

pub(super) fn resolve_child_bundle(
    connection: &Connection,
    agent_id: &str,
    wake_id: &str,
    source_agent_message_id: &str,
    require_active: bool,
) -> Result<ChildAgentSpawnRecord, AgentGraphError> {
    validate_id("agent_id", agent_id)?;
    validate_id("wake_id", wake_id)?;
    validate_id("source_agent_message_id", source_agent_message_id)?;
    let agent = query_node(connection, agent_id)?
        .ok_or_else(|| AgentGraphError::AgentNotFound(agent_id.to_string()))?;
    let parent_id = agent
        .parent_agent_id
        .as_deref()
        .ok_or_else(|| corrupt("trusted child Wake resolved to a root Agent"))?;
    let parent = query_node(connection, parent_id)?
        .ok_or_else(|| corrupt("child Agent parent no longer exists"))?;
    let task_message = query_message(connection, source_agent_message_id)?
        .ok_or_else(|| corrupt("child Wake source Mailbox message no longer exists"))?;
    let initial_wake =
        query_wake(connection, wake_id)?.ok_or_else(|| corrupt("child Wake no longer exists"))?;
    let projection_exists = connection
        .query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM messages
                 WHERE id = ?1 AND conversation_id = ?2 AND role = 'user'
                   AND input_origin_kind = 'agent'
                   AND input_origin_agent_id = ?3
                   AND source_agent_message_id = ?4
             )",
            params![
                &task_message.projection_message_id,
                &agent.conversation_id,
                &task_message.sender_agent_id,
                source_agent_message_id,
            ],
            |row| row.get::<_, bool>(0),
        )
        .map_err(read_error)?;
    let sender = query_node(connection, &task_message.sender_agent_id)?
        .ok_or_else(|| corrupt("Wake source sender Agent no longer exists"))?;
    let source_authorized = match task_message.kind {
        AgentMailboxKind::Task => task_message.sender_agent_id == parent_id,
        AgentMailboxKind::Followup => {
            is_strict_descendant(connection, &sender.agent_id, &agent.agent_id)?
        }
        AgentMailboxKind::Result => {
            sender.parent_agent_id.as_deref() == Some(agent.agent_id.as_str())
        }
        AgentMailboxKind::Message => false,
    };
    if (require_active
        && (agent.lifecycle != AgentLifecycle::Active
            || parent.lifecycle != AgentLifecycle::Active
            || sender.lifecycle != AgentLifecycle::Active))
        || parent.root_agent_id != agent.root_agent_id
        || parent.root_conversation_id != agent.root_conversation_id
        || sender.root_agent_id != agent.root_agent_id
        || task_message.root_agent_id != agent.root_agent_id
        || task_message.recipient_agent_id != agent.agent_id
        || !source_authorized
        || task_message.delivery_status != AgentMailboxDeliveryStatus::Acknowledged
        || initial_wake.root_agent_id != agent.root_agent_id
        || initial_wake.agent_id != agent.agent_id
        || initial_wake.requester_agent_id != task_message.sender_agent_id
        || initial_wake.source_agent_message_id.as_deref() != Some(source_agent_message_id)
        || !projection_exists
    {
        return Err(corrupt(
            "trusted child Wake facts do not form one durable assignment bundle",
        ));
    }
    let model_selection_source = agent
        .model_selection_source
        .ok_or_else(|| corrupt("child Agent is missing model selector provenance"))?;
    Ok(ChildAgentSpawnRecord {
        collaboration_identity: AgentCollaborationIdentity {
            agent_id: agent.agent_id.clone(),
            root_agent_id: agent.root_agent_id.clone(),
            root_conversation_id: agent.root_conversation_id.clone(),
            parent_agent_id: parent.agent_id,
            parent_task_name: parent.task_name,
            parent_task_path: parent.task_path,
            conversation_id: agent.conversation_id.clone(),
            task_name: agent.task_name.clone(),
            task_path: agent.task_path.clone(),
            source_agent_id: sender.agent_id.clone(),
            source_kind: task_message.kind,
            source_task_name: sender.task_name.clone(),
            source_task_path: sender.task_path.clone(),
            source_agent_message_id: task_message.message_id.clone(),
            entrusted_task: task_message.content.clone(),
            template_instructions: agent
                .template_snapshot
                .as_ref()
                .map(|template| template.instructions.clone()),
        },
        agent,
        model_selection_source,
        task_message,
        initial_wake,
    })
}

pub(crate) fn resolve_child_spawn_by_creation_request(
    connection: &Connection,
    parent_agent_id: &str,
    creation_request_id: &str,
) -> Result<Option<ChildAgentSpawnRecord>, AgentGraphError> {
    validate_id("parent_agent_id", parent_agent_id)?;
    validate_request_id(creation_request_id)?;
    let Some(parent) = query_node(connection, parent_agent_id)? else {
        return Ok(None);
    };
    let Some(agent) =
        query_node_by_request(connection, &parent.root_agent_id, creation_request_id)?
    else {
        return Ok(None);
    };
    if agent.parent_agent_id.as_deref() != Some(parent_agent_id) {
        return Err(conflict(
            "creation request ID belongs to a different parent Agent",
        ));
    }
    let message = query_message_by_request(connection, parent_agent_id, creation_request_id)?
        .ok_or_else(|| corrupt("created child Agent is missing its initial task"))?;
    let wake = query_wake_by_request(connection, parent_agent_id, creation_request_id)?
        .ok_or_else(|| corrupt("created child Agent is missing its initial Wake"))?;
    resolve_child_bundle(
        connection,
        &agent.agent_id,
        &wake.wake_id,
        &message.message_id,
        false,
    )
    .map(Some)
}
