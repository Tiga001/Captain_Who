//! Transactional persistence for the deliberately small Agent parent-child tree.
//!
//! SQLite is the coordination source of truth. Runtime notifications may observe these records,
//! but no in-memory queue is allowed to substitute for them.

use crate::{
    AcknowledgeAgentTaskAndWakeInput, AgentGraphError, AgentLifecycle, AgentMailboxDeliveryStatus,
    AgentMailboxKind, AgentMailboxMessageRecord, AgentModelSelectionSnapshot, AgentNodeRecord,
    AgentTemplateSnapshot, AgentWakeRequestRecord, AgentWakeStatus, ConversationMessageOrigin,
    CreateAgentNodeInput, EnqueueAgentMessageInput, EnqueueAgentWakeInput, EnsureRootAgentInput,
    FinishAgentWakeWithResultInput, IdempotentCreate, AGENT_GRAPH_SCHEMA_VERSION,
};
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};

const MAX_ID_BYTES: usize = 128;
const MAX_REQUEST_ID_BYTES: usize = 256;
const MAX_TASK_NAME_BYTES: usize = 256;
const MAX_TASK_PATH_BYTES: usize = 2_048;
const MAX_MESSAGE_BYTES: usize = 1_048_576;
const WAKE_LEASE_DURATION_MS: i64 = 60_000;

const NODE_SELECT: &str = "
    SELECT agent_id, schema_version, root_agent_id, root_conversation_id,
           parent_agent_id, conversation_id, project_id, creation_request_id,
           task_name, task_path,
           template_id_snapshot, template_project_id_snapshot,
           template_machine_key_snapshot, template_name_snapshot,
           template_description_snapshot, template_instructions_snapshot,
           template_revision_snapshot, template_model_config_id_snapshot,
           model_config_id_snapshot, model_display_name_snapshot,
           model_supports_image_snapshot, model_context_window_tokens_snapshot,
           model_settings_revision_snapshot, provider_connection_revision_snapshot,
           provider_protocol_revision_snapshot, lifecycle, revision, created_at, updated_at
    FROM agent_nodes";

const MESSAGE_SELECT: &str = "
    SELECT sequence, message_id, schema_version, root_agent_id, sender_agent_id,
           recipient_agent_id, request_id, kind, content, projection_message_id,
           delivery_status, claim_token, lease_expires_at, created_at, claimed_at,
           acknowledged_at
    FROM agent_mailbox_messages";

const WAKE_SELECT: &str = "
    SELECT sequence, wake_id, schema_version, root_agent_id, agent_id,
           requester_agent_id, request_id, source_agent_message_id, status,
           claim_token, lease_expires_at, result_message_id, terminal_error,
           created_at, claimed_at, started_at, completed_at
    FROM agent_wake_requests";

pub fn ensure_root_agent(
    connection: &mut Connection,
    input: &EnsureRootAgentInput,
    created_at: i64,
) -> Result<IdempotentCreate<AgentNodeRecord>, AgentGraphError> {
    validate_id("agent_id", &input.agent_id)?;
    validate_id("conversation_id", &input.conversation_id)?;
    validate_request_id(&input.creation_request_id)?;
    validate_trimmed("task_name", &input.task_name, MAX_TASK_NAME_BYTES)?;
    validate_time(created_at)?;

    let transaction = immediate(connection)?;
    let project_id = conversation_project(&transaction, &input.conversation_id)?;
    if let Some(existing) = query_node_by_root_conversation(&transaction, &input.conversation_id)? {
        if existing.agent_id == input.agent_id
            && existing.parent_agent_id.is_none()
            && existing.conversation_id == input.conversation_id
            && existing.creation_request_id == input.creation_request_id
            && existing.task_name == input.task_name
            && existing.task_path == "/root"
            && existing.project_id == project_id
        {
            transaction.commit().map_err(write_error)?;
            return Ok(IdempotentCreate::Existing(existing));
        }
        return Err(conflict(
            "root Conversation is already bound to another root identity",
        ));
    }
    if query_node(&transaction, &input.agent_id)?.is_some() {
        return Err(conflict("Agent ID is already bound to another node"));
    }

    transaction
        .execute(
            "INSERT INTO agent_nodes (
                 agent_id, schema_version, root_agent_id, root_conversation_id,
                 parent_agent_id, conversation_id, project_id, creation_request_id,
                 task_name, task_path,
                 template_id_snapshot, template_project_id_snapshot,
                 template_machine_key_snapshot, template_name_snapshot,
                 template_description_snapshot, template_instructions_snapshot,
                 template_revision_snapshot, template_model_config_id_snapshot,
                 model_config_id_snapshot, model_display_name_snapshot,
                 model_supports_image_snapshot, model_context_window_tokens_snapshot,
                 model_settings_revision_snapshot, provider_connection_revision_snapshot,
                 provider_protocol_revision_snapshot, lifecycle, revision, created_at, updated_at
             ) VALUES (
                 ?1, ?2, ?1, ?3, NULL, ?3, ?4, ?5, ?6, '/root',
                 NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL,
                 NULL, NULL, NULL, NULL, NULL, NULL, NULL, 'active', 1, ?7, ?7
             )",
            params![
                &input.agent_id,
                i64::from(AGENT_GRAPH_SCHEMA_VERSION),
                &input.conversation_id,
                &project_id,
                &input.creation_request_id,
                &input.task_name,
                created_at,
            ],
        )
        .map_err(write_error)?;
    let record = query_node(&transaction, &input.agent_id)?
        .ok_or_else(|| corrupt("created root Agent could not be read back"))?;
    transaction.commit().map_err(write_error)?;
    Ok(IdempotentCreate::Created(record))
}

pub fn create_agent_node(
    connection: &mut Connection,
    input: &CreateAgentNodeInput,
    created_at: i64,
) -> Result<IdempotentCreate<AgentNodeRecord>, AgentGraphError> {
    validate_create_node(input, created_at)?;
    let transaction = immediate(connection)?;

    if let Some(existing) = query_node_by_request(
        &transaction,
        &input.root_agent_id,
        &input.creation_request_id,
    )? {
        if node_matches_create(&existing, input) {
            transaction.commit().map_err(write_error)?;
            return Ok(IdempotentCreate::Existing(existing));
        }
        return Err(conflict(
            "creation request ID was reused with different node facts",
        ));
    }

    let parent = query_node(&transaction, &input.parent_agent_id)?
        .ok_or_else(|| AgentGraphError::AgentNotFound(input.parent_agent_id.clone()))?;
    if parent.root_agent_id != input.root_agent_id || parent.lifecycle != AgentLifecycle::Active {
        return Err(conflict("parent Agent is not active in the requested tree"));
    }
    let expected_path = format!("{}/{}", parent.task_path, input.task_name);
    if input.task_path != expected_path || input.task_name.contains('/') {
        return Err(invalid(
            "task_path",
            "must be exactly one task-name segment below the parent path",
        ));
    }
    let project_id = conversation_project(&transaction, &input.conversation_id)?;
    if project_id != parent.project_id {
        return Err(conflict(
            "child Conversation project does not match its parent tree",
        ));
    }
    let conversation_model_id = transaction
        .query_row(
            "SELECT model_id FROM conversations WHERE id = ?1",
            [&input.conversation_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .map_err(read_error)?;
    if conversation_model_id.as_deref() != Some(input.model_snapshot.model_config_id.as_str()) {
        return Err(conflict(
            "child Conversation model does not match its frozen model snapshot",
        ));
    }
    let has_prior_conversation_execution = transaction
        .query_row(
            "SELECT
                 EXISTS(SELECT 1 FROM messages WHERE conversation_id = ?1)
                 OR EXISTS(
                     SELECT 1 FROM conversation_turn_traces WHERE conversation_id = ?1
                 )
                 OR EXISTS(
                     SELECT 1 FROM agent_pending_actions WHERE conversation_id = ?1
                 )
                 OR EXISTS(
                     SELECT 1 FROM agent_action_audit WHERE conversation_id = ?1
                 )
                 OR EXISTS(
                     SELECT 1 FROM agent_command_sessions WHERE conversation_id = ?1
                 )",
            [&input.conversation_id],
            |row| row.get::<_, bool>(0),
        )
        .map_err(read_error)?;
    if has_prior_conversation_execution {
        return Err(conflict(
            "low-level child binding requires a fresh Conversation with no prior messages or execution facts",
        ));
    }
    validate_template_snapshot(
        &transaction,
        input.template_snapshot.as_ref(),
        project_id.as_deref(),
    )?;

    if query_node(&transaction, &input.agent_id)?.is_some() {
        return Err(conflict("Agent ID is already bound to another node"));
    }
    if query_node_by_conversation(&transaction, &input.conversation_id)?.is_some() {
        return Err(conflict("Conversation is already bound to an Agent"));
    }
    let duplicate_task = transaction
        .query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM agent_nodes
                 WHERE root_agent_id = ?1 AND (task_name = ?2 OR task_path = ?3)
             )",
            params![&input.root_agent_id, &input.task_name, &input.task_path],
            |row| row.get::<_, bool>(0),
        )
        .map_err(read_error)?;
    if duplicate_task {
        return Err(conflict(
            "task name or path is already in use in this Agent tree",
        ));
    }

    let template = input.template_snapshot.as_ref();
    let model = &input.model_snapshot;
    transaction
        .execute(
            "INSERT INTO agent_nodes (
                 agent_id, schema_version, root_agent_id, root_conversation_id,
                 parent_agent_id, conversation_id, project_id, creation_request_id,
                 task_name, task_path,
                 template_id_snapshot, template_project_id_snapshot,
                 template_machine_key_snapshot, template_name_snapshot,
                 template_description_snapshot, template_instructions_snapshot,
                 template_revision_snapshot, template_model_config_id_snapshot,
                 model_config_id_snapshot, model_display_name_snapshot,
                 model_supports_image_snapshot, model_context_window_tokens_snapshot,
                 model_settings_revision_snapshot, provider_connection_revision_snapshot,
                 provider_protocol_revision_snapshot, lifecycle, revision, created_at, updated_at
             ) VALUES (
                 ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10,
                 ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18,
                 ?19, ?20, ?21, ?22, ?23, ?24, ?25, 'active', 1, ?26, ?26
             )",
            params![
                &input.agent_id,
                i64::from(AGENT_GRAPH_SCHEMA_VERSION),
                &input.root_agent_id,
                &parent.root_conversation_id,
                &input.parent_agent_id,
                &input.conversation_id,
                &project_id,
                &input.creation_request_id,
                &input.task_name,
                &input.task_path,
                template.map(|value| value.template_id.as_str()),
                template.map(|value| value.project_id.as_str()),
                template.map(|value| value.machine_key.as_str()),
                template.map(|value| value.name.as_str()),
                template.map(|value| value.description.as_str()),
                template.map(|value| value.instructions.as_str()),
                template
                    .map(|value| revision_to_sql(value.template_revision))
                    .transpose()?,
                template.map(|value| value.model_config_id.as_str()),
                &model.model_config_id,
                &model.display_name,
                model.supports_image,
                i64::from(model.effective_context_window_tokens),
                &model.model_settings_configuration_revision,
                &model.provider_connection_revision,
                &model.provider_protocol_revision,
                created_at,
            ],
        )
        .map_err(write_error)?;
    let record = query_node(&transaction, &input.agent_id)?
        .ok_or_else(|| corrupt("created child Agent could not be read back"))?;
    transaction.commit().map_err(write_error)?;
    Ok(IdempotentCreate::Created(record))
}

pub fn get_agent_node(
    connection: &Connection,
    agent_id: &str,
) -> Result<Option<AgentNodeRecord>, AgentGraphError> {
    validate_id("agent_id", agent_id)?;
    query_node(connection, agent_id)
}

pub fn get_agent_node_by_conversation(
    connection: &Connection,
    conversation_id: &str,
) -> Result<Option<AgentNodeRecord>, AgentGraphError> {
    validate_id("conversation_id", conversation_id)?;
    query_node_by_conversation(connection, conversation_id)
}

pub fn list_agent_children(
    connection: &Connection,
    root_agent_id: &str,
    parent_agent_id: &str,
) -> Result<Vec<AgentNodeRecord>, AgentGraphError> {
    validate_id("root_agent_id", root_agent_id)?;
    validate_id("parent_agent_id", parent_agent_id)?;
    query_nodes(
        connection,
        &format!(
            "{NODE_SELECT} WHERE root_agent_id = ?1 AND parent_agent_id = ?2
             ORDER BY created_at, agent_id"
        ),
        params![root_agent_id, parent_agent_id],
    )
}

pub fn list_agent_tree(
    connection: &Connection,
    root_agent_id: &str,
) -> Result<Vec<AgentNodeRecord>, AgentGraphError> {
    validate_id("root_agent_id", root_agent_id)?;
    query_nodes(
        connection,
        &format!("{NODE_SELECT} WHERE root_agent_id = ?1 ORDER BY created_at, agent_id"),
        [root_agent_id],
    )
}

pub fn transition_agent_lifecycle(
    connection: &mut Connection,
    agent_id: &str,
    expected_revision: u64,
    expected_lifecycle: AgentLifecycle,
    requested_lifecycle: AgentLifecycle,
    updated_at: i64,
) -> Result<AgentNodeRecord, AgentGraphError> {
    validate_id("agent_id", agent_id)?;
    validate_revision(expected_revision)?;
    validate_time(updated_at)?;
    let transaction = immediate(connection)?;
    let current = query_node(&transaction, agent_id)?
        .ok_or_else(|| AgentGraphError::AgentNotFound(agent_id.to_string()))?;
    if current.revision != expected_revision {
        return Err(AgentGraphError::RevisionConflict {
            expected: expected_revision,
            current: current.revision,
        });
    }
    if current.lifecycle != expected_lifecycle {
        return Err(conflict(
            "expected Agent lifecycle does not match persisted lifecycle",
        ));
    }
    if current.lifecycle == requested_lifecycle {
        transaction.commit().map_err(write_error)?;
        return Ok(current);
    }
    if !current.lifecycle.can_transition_to(requested_lifecycle) {
        return Err(AgentGraphError::IllegalLifecycleTransition {
            current: current.lifecycle,
            requested: requested_lifecycle,
        });
    }
    if requested_lifecycle != AgentLifecycle::Active {
        let unsettled_wake = transaction
            .query_row(
                "SELECT EXISTS(
                     SELECT 1 FROM agent_wake_requests
                     WHERE (agent_id = ?1 OR requester_agent_id = ?1)
                       AND status IN ('queued', 'claimed', 'running', 'waiting_for_approval')
                 )",
                [agent_id],
                |row| row.get::<_, bool>(0),
            )
            .map_err(read_error)?;
        if unsettled_wake {
            return Err(conflict("Agent has an unsettled Wake request"));
        }
        let unsettled_message = transaction
            .query_row(
                "SELECT EXISTS(
                     SELECT 1 FROM agent_mailbox_messages
                     WHERE (sender_agent_id = ?1 OR recipient_agent_id = ?1)
                       AND delivery_status != 'acknowledged'
                 )",
                [agent_id],
                |row| row.get::<_, bool>(0),
            )
            .map_err(read_error)?;
        if unsettled_message {
            return Err(conflict("Agent has an unsettled Mailbox message"));
        }
        let active_child = transaction
            .query_row(
                "SELECT EXISTS(
                     SELECT 1 FROM agent_nodes
                     WHERE parent_agent_id = ?1 AND lifecycle = 'active'
                 )",
                [agent_id],
                |row| row.get::<_, bool>(0),
            )
            .map_err(read_error)?;
        if active_child {
            return Err(conflict("Agent has an active child"));
        }
    } else if let Some(parent_agent_id) = current.parent_agent_id.as_deref() {
        let parent = query_node(&transaction, parent_agent_id)?
            .ok_or_else(|| corrupt("Agent parent is missing"))?;
        if parent.lifecycle != AgentLifecycle::Active {
            return Err(conflict(
                "Agent parent must be active before child activation",
            ));
        }
    }
    let next_revision = current
        .revision
        .checked_add(1)
        .ok_or_else(|| conflict("Agent revision is exhausted"))?;
    transaction
        .execute(
            "UPDATE agent_nodes
             SET lifecycle = ?1, revision = ?2, updated_at = ?3
             WHERE agent_id = ?4 AND lifecycle = ?5 AND revision = ?6",
            params![
                requested_lifecycle.as_str(),
                revision_to_sql(next_revision)?,
                updated_at.max(current.updated_at.saturating_add(1)),
                agent_id,
                expected_lifecycle.as_str(),
                revision_to_sql(expected_revision)?,
            ],
        )
        .map_err(write_error)?;
    let updated = query_node(&transaction, agent_id)?
        .ok_or_else(|| corrupt("updated Agent could not be read back"))?;
    transaction.commit().map_err(write_error)?;
    Ok(updated)
}

pub fn ensure_conversation_unbound(
    connection: &Connection,
    conversation_id: &str,
) -> Result<(), AgentGraphError> {
    validate_id("conversation_id", conversation_id)?;
    let bound = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM agent_nodes WHERE conversation_id = ?1)",
            [conversation_id],
            |row| row.get::<_, bool>(0),
        )
        .map_err(read_error)?;
    if bound {
        Err(AgentGraphError::BoundConversation(
            conversation_id.to_string(),
        ))
    } else {
        Ok(())
    }
}

pub fn ensure_project_unbound(
    connection: &Connection,
    project_id: &str,
) -> Result<(), AgentGraphError> {
    validate_bounded_text("project_id", project_id, 1, 1_024)?;
    let bound = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM agent_nodes WHERE project_id = ?1)",
            [project_id],
            |row| row.get::<_, bool>(0),
        )
        .map_err(read_error)?;
    if bound {
        Err(AgentGraphError::BoundProject(project_id.to_string()))
    } else {
        Ok(())
    }
}

pub fn enqueue_agent_message(
    connection: &mut Connection,
    input: &EnqueueAgentMessageInput,
    created_at: i64,
) -> Result<IdempotentCreate<AgentMailboxMessageRecord>, AgentGraphError> {
    validate_message_input(input, created_at)?;
    let transaction = immediate(connection)?;
    let outcome = enqueue_message_in_transaction(&transaction, input, created_at)?;
    transaction.commit().map_err(write_error)?;
    Ok(outcome)
}

pub fn get_agent_message(
    connection: &Connection,
    message_id: &str,
) -> Result<Option<AgentMailboxMessageRecord>, AgentGraphError> {
    validate_id("message_id", message_id)?;
    query_message(connection, message_id)
}

pub fn claim_next_agent_message(
    connection: &mut Connection,
    recipient_agent_id: &str,
    claim_token: &str,
    claimed_at: i64,
) -> Result<Option<AgentMailboxMessageRecord>, AgentGraphError> {
    validate_id("recipient_agent_id", recipient_agent_id)?;
    validate_id("claim_token", claim_token)?;
    validate_time(claimed_at)?;
    let transaction = immediate(connection)?;
    if let Some(existing) = query_message_by_claim_token(&transaction, claim_token)? {
        if existing.recipient_agent_id != recipient_agent_id {
            return Err(conflict(
                "Mailbox claim token was reused for another recipient",
            ));
        }
        if existing.delivery_status == AgentMailboxDeliveryStatus::Claimed
            && existing
                .lease_expires_at
                .is_some_and(|deadline| claimed_at > deadline)
        {
            return Err(conflict("Mailbox claim lease has expired"));
        }
        transaction.commit().map_err(write_error)?;
        return Ok(Some(existing));
    }
    ensure_active_agent(&transaction, recipient_agent_id)?;
    let lease_expires_at = claimed_at
        .checked_add(WAKE_LEASE_DURATION_MS)
        .ok_or_else(|| invalid("claimed_at", "cannot compute the Mailbox lease deadline"))?;
    transaction
        .execute(
            "UPDATE agent_mailbox_messages
             SET delivery_status = 'queued', claim_token = NULL, lease_expires_at = NULL,
                 claimed_at = NULL
             WHERE recipient_agent_id = ?1 AND delivery_status = 'claimed'
               AND lease_expires_at <= ?2",
            params![recipient_agent_id, claimed_at],
        )
        .map_err(write_error)?;
    let already_claimed = transaction
        .query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM agent_mailbox_messages
                 WHERE recipient_agent_id = ?1 AND delivery_status = 'claimed'
             )",
            [recipient_agent_id],
            |row| row.get::<_, bool>(0),
        )
        .map_err(read_error)?;
    if already_claimed {
        transaction.commit().map_err(write_error)?;
        return Ok(None);
    }
    let next_id = transaction
        .query_row(
            "SELECT message_id FROM agent_mailbox_messages
             WHERE recipient_agent_id = ?1 AND delivery_status = 'queued'
             ORDER BY sequence LIMIT 1",
            [recipient_agent_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(read_error)?;
    let Some(message_id) = next_id else {
        transaction.commit().map_err(write_error)?;
        return Ok(None);
    };
    transaction
        .execute(
            "UPDATE agent_mailbox_messages
             SET delivery_status = 'claimed', claim_token = ?1, lease_expires_at = ?2,
                 claimed_at = ?3
             WHERE message_id = ?4 AND delivery_status = 'queued'",
            params![claim_token, lease_expires_at, claimed_at, &message_id],
        )
        .map_err(write_error)?;
    let claimed = query_message(&transaction, &message_id)?
        .ok_or_else(|| corrupt("claimed Mailbox message could not be read back"))?;
    transaction.commit().map_err(write_error)?;
    Ok(Some(claimed))
}

pub fn renew_agent_message_lease(
    connection: &mut Connection,
    message_id: &str,
    claim_token: &str,
    renewed_at: i64,
) -> Result<AgentMailboxMessageRecord, AgentGraphError> {
    validate_id("message_id", message_id)?;
    validate_id("claim_token", claim_token)?;
    validate_time(renewed_at)?;
    let next_deadline = renewed_at
        .checked_add(WAKE_LEASE_DURATION_MS)
        .ok_or_else(|| invalid("renewed_at", "cannot compute the Mailbox lease deadline"))?;
    let transaction = immediate(connection)?;
    let current = query_message(&transaction, message_id)?
        .ok_or_else(|| AgentGraphError::MessageNotFound(message_id.to_string()))?;
    if current.delivery_status != AgentMailboxDeliveryStatus::Claimed
        || current.claim_token.as_deref() != Some(claim_token)
    {
        return Err(conflict(
            "Mailbox message is not actively held by this claim token",
        ));
    }
    let current_deadline = current
        .lease_expires_at
        .ok_or_else(|| corrupt("claimed Mailbox message has no lease deadline"))?;
    if renewed_at > current_deadline {
        return Err(conflict("Mailbox claim lease has expired"));
    }
    let advanced_deadline = next_deadline.max(current_deadline.saturating_add(1));
    transaction
        .execute(
            "UPDATE agent_mailbox_messages SET lease_expires_at = ?1
             WHERE message_id = ?2 AND delivery_status = 'claimed'
               AND claim_token = ?3 AND lease_expires_at = ?4",
            params![advanced_deadline, message_id, claim_token, current_deadline],
        )
        .map_err(write_error)?;
    let renewed = query_message(&transaction, message_id)?
        .ok_or_else(|| corrupt("renewed Mailbox message could not be read back"))?;
    transaction.commit().map_err(write_error)?;
    Ok(renewed)
}

pub fn acknowledge_agent_message_with_projection(
    connection: &mut Connection,
    message_id: &str,
    claim_token: &str,
    acknowledged_at: i64,
) -> Result<AgentMailboxMessageRecord, AgentGraphError> {
    validate_id("message_id", message_id)?;
    validate_id("claim_token", claim_token)?;
    validate_time(acknowledged_at)?;
    let transaction = immediate(connection)?;
    let message = query_message(&transaction, message_id)?
        .ok_or_else(|| AgentGraphError::MessageNotFound(message_id.to_string()))?;
    if matches!(
        message.kind,
        AgentMailboxKind::Task | AgentMailboxKind::Followup
    ) {
        return Err(invalid(
            "message_id",
            "task and followup messages must be acknowledged atomically with a Wake",
        ));
    }
    let acknowledged =
        acknowledge_message_in_transaction(&transaction, message_id, claim_token, acknowledged_at)?;
    transaction.commit().map_err(write_error)?;
    Ok(acknowledged)
}

pub fn acknowledge_agent_task_with_projection_and_wake(
    connection: &mut Connection,
    input: &AcknowledgeAgentTaskAndWakeInput,
    acknowledged_at: i64,
) -> Result<(AgentMailboxMessageRecord, AgentWakeRequestRecord), AgentGraphError> {
    validate_id("message_id", &input.message_id)?;
    validate_id("message_claim_token", &input.message_claim_token)?;
    validate_time(acknowledged_at)?;
    validate_wake_input(&input.wake, acknowledged_at)?;
    if input.wake.source_agent_message_id.as_deref() != Some(input.message_id.as_str()) {
        return Err(invalid(
            "wake.source_agent_message_id",
            "must reference the task message being acknowledged",
        ));
    }
    let transaction = immediate(connection)?;
    let message = query_message(&transaction, &input.message_id)?
        .ok_or_else(|| AgentGraphError::MessageNotFound(input.message_id.clone()))?;
    if !matches!(
        message.kind,
        AgentMailboxKind::Task | AgentMailboxKind::Followup
    ) {
        return Err(invalid(
            "message_id",
            "only task or followup messages may atomically create a Wake",
        ));
    }
    let acknowledged = acknowledge_message_in_transaction(
        &transaction,
        &input.message_id,
        &input.message_claim_token,
        acknowledged_at,
    )?;
    let wake = enqueue_wake_in_transaction(&transaction, &input.wake, acknowledged_at)?;
    transaction.commit().map_err(write_error)?;
    Ok((acknowledged, wake.record().clone()))
}

fn acknowledge_message_in_transaction(
    transaction: &Transaction<'_>,
    message_id: &str,
    claim_token: &str,
    acknowledged_at: i64,
) -> Result<AgentMailboxMessageRecord, AgentGraphError> {
    let current = query_message(transaction, message_id)?
        .ok_or_else(|| AgentGraphError::MessageNotFound(message_id.to_string()))?;
    if current.delivery_status == AgentMailboxDeliveryStatus::Acknowledged {
        if current.claim_token.as_deref() == Some(claim_token) {
            let projection_exists = transaction
                .query_row(
                    "SELECT EXISTS(
                         SELECT 1 FROM messages
                         WHERE id = ?1 AND source_agent_message_id = ?2
                     )",
                    params![&current.projection_message_id, &current.message_id],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(read_error)?;
            if !projection_exists {
                return Err(corrupt(
                    "acknowledged Mailbox message is missing its Conversation projection",
                ));
            }
            return Ok(current);
        }
        return Err(conflict(
            "Mailbox acknowledgement belongs to another claim token",
        ));
    }
    if current.delivery_status != AgentMailboxDeliveryStatus::Claimed
        || current.claim_token.as_deref() != Some(claim_token)
    {
        return Err(conflict("Mailbox message is not held by this claim token"));
    }
    if current
        .lease_expires_at
        .is_none_or(|deadline| acknowledged_at > deadline)
    {
        return Err(conflict("Mailbox claim lease has expired"));
    }
    let recipient = query_node(transaction, &current.recipient_agent_id)?
        .ok_or_else(|| corrupt("Mailbox recipient Agent is missing"))?;
    let position = transaction
        .query_row(
            "SELECT COALESCE(MAX(position), -1) + 1 FROM messages WHERE conversation_id = ?1",
            [&recipient.conversation_id],
            |row| row.get::<_, i64>(0),
        )
        .map_err(read_error)?;
    transaction
        .execute(
            "INSERT INTO messages (
                 id, conversation_id, role, content, status,
                 input_origin_kind, input_origin_agent_id, source_agent_message_id,
                 agent_run_json, ui_state_json, created_at, position
             ) VALUES (?1, ?2, 'user', ?3, 'sent', 'agent', ?4, ?5,
                       NULL, NULL, ?6, ?7)",
            params![
                &current.projection_message_id,
                &recipient.conversation_id,
                &current.content,
                &current.sender_agent_id,
                &current.message_id,
                acknowledged_at,
                position,
            ],
        )
        .map_err(write_error)?;
    transaction
        .execute(
            "UPDATE agent_mailbox_messages
             SET delivery_status = 'acknowledged', acknowledged_at = ?1
             WHERE message_id = ?2 AND delivery_status = 'claimed' AND claim_token = ?3",
            params![acknowledged_at, message_id, claim_token],
        )
        .map_err(write_error)?;
    transaction
        .execute(
            "UPDATE conversations
             SET updated_at = MAX(updated_at, ?1) WHERE id = ?2",
            params![acknowledged_at, &recipient.conversation_id],
        )
        .map_err(write_error)?;
    let acknowledged = query_message(transaction, message_id)?
        .ok_or_else(|| corrupt("acknowledged Mailbox message could not be read back"))?;
    Ok(acknowledged)
}

pub fn conversation_message_origin(
    connection: &Connection,
    conversation_id: &str,
    message_id: &str,
) -> Result<ConversationMessageOrigin, AgentGraphError> {
    validate_id("conversation_id", conversation_id)?;
    validate_id("message_id", message_id)?;
    let stored = connection
        .query_row(
            "SELECT role, input_origin_kind, input_origin_agent_id, source_agent_message_id
             FROM messages WHERE conversation_id = ?1 AND id = ?2",
            params![conversation_id, message_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            },
        )
        .optional()
        .map_err(read_error)?
        .ok_or_else(|| AgentGraphError::MessageNotFound(message_id.to_string()))?;
    match stored {
        (role, _, _, _) if role != "user" => Err(AgentGraphError::InvalidInput {
            field: "message_id",
            reason: "message origin is defined only for user/input messages".to_string(),
        }),
        (_, None, None, None) => Ok(ConversationMessageOrigin::Human),
        (_, Some(kind), None, None) if kind == "human" => Ok(ConversationMessageOrigin::Human),
        (_, Some(kind), Some(sender_agent_id), Some(source_agent_message_id))
            if kind == "agent" =>
        {
            Ok(ConversationMessageOrigin::Agent {
                sender_agent_id,
                source_agent_message_id,
            })
        }
        _ => Err(corrupt(
            "Conversation message origin columns are inconsistent",
        )),
    }
}

pub fn enqueue_agent_wake(
    connection: &mut Connection,
    input: &EnqueueAgentWakeInput,
    created_at: i64,
) -> Result<IdempotentCreate<AgentWakeRequestRecord>, AgentGraphError> {
    validate_wake_input(input, created_at)?;
    let transaction = immediate(connection)?;
    let outcome = enqueue_wake_in_transaction(&transaction, input, created_at)?;
    transaction.commit().map_err(write_error)?;
    Ok(outcome)
}

fn enqueue_wake_in_transaction(
    transaction: &Transaction<'_>,
    input: &EnqueueAgentWakeInput,
    created_at: i64,
) -> Result<IdempotentCreate<AgentWakeRequestRecord>, AgentGraphError> {
    if let Some(existing) =
        query_wake_by_request(transaction, &input.requester_agent_id, &input.request_id)?
    {
        if wake_matches_enqueue(&existing, input) {
            return Ok(IdempotentCreate::Existing(existing));
        }
        return Err(conflict("Wake request ID was reused with different facts"));
    }
    if query_wake(transaction, &input.wake_id)?.is_some() {
        return Err(conflict("Wake ID is already in use"));
    }
    if let Some(source_message_id) = input.source_agent_message_id.as_deref() {
        let existing_source_wake = transaction
            .query_row(
                "SELECT EXISTS(
                     SELECT 1 FROM agent_wake_requests WHERE source_agent_message_id = ?1
                 )",
                [source_message_id],
                |row| row.get::<_, bool>(0),
            )
            .map_err(read_error)?;
        if existing_source_wake {
            return Err(conflict(
                "Wake source message is already bound to another Wake",
            ));
        }
    }
    ensure_active_pair(
        transaction,
        &input.root_agent_id,
        &input.requester_agent_id,
        &input.agent_id,
    )?;
    if let Some(source_message_id) = input.source_agent_message_id.as_deref() {
        let source = query_message(transaction, source_message_id)?
            .ok_or_else(|| AgentGraphError::MessageNotFound(source_message_id.to_string()))?;
        if source.root_agent_id != input.root_agent_id
            || source.sender_agent_id != input.requester_agent_id
            || source.recipient_agent_id != input.agent_id
            || source.delivery_status != AgentMailboxDeliveryStatus::Acknowledged
        {
            return Err(conflict(
                "Wake source message is not an acknowledged delivery for this request",
            ));
        }
    }
    transaction
        .execute(
            "INSERT INTO agent_wake_requests (
                 wake_id, schema_version, root_agent_id, agent_id, requester_agent_id,
                 request_id, source_agent_message_id, status, claim_token, lease_expires_at,
                 result_message_id, terminal_error, created_at, claimed_at, started_at, completed_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'queued', NULL, NULL, NULL,
                       NULL, ?8, NULL, NULL, NULL)",
            params![
                &input.wake_id,
                i64::from(AGENT_GRAPH_SCHEMA_VERSION),
                &input.root_agent_id,
                &input.agent_id,
                &input.requester_agent_id,
                &input.request_id,
                &input.source_agent_message_id,
                created_at,
            ],
        )
        .map_err(write_error)?;
    let record = query_wake(transaction, &input.wake_id)?
        .ok_or_else(|| corrupt("created Wake could not be read back"))?;
    Ok(IdempotentCreate::Created(record))
}

pub fn get_agent_wake(
    connection: &Connection,
    wake_id: &str,
) -> Result<Option<AgentWakeRequestRecord>, AgentGraphError> {
    validate_id("wake_id", wake_id)?;
    query_wake(connection, wake_id)
}

pub fn claim_next_agent_wake(
    connection: &mut Connection,
    agent_id: &str,
    claim_token: &str,
    claimed_at: i64,
) -> Result<Option<AgentWakeRequestRecord>, AgentGraphError> {
    validate_id("agent_id", agent_id)?;
    validate_id("claim_token", claim_token)?;
    validate_time(claimed_at)?;
    let transaction = immediate(connection)?;
    if let Some(existing) = query_wake_by_claim_token(&transaction, claim_token)? {
        if existing.agent_id != agent_id {
            return Err(conflict("Wake claim token was reused for another Agent"));
        }
        transaction.commit().map_err(write_error)?;
        return Ok(Some(existing));
    }
    ensure_active_agent(&transaction, agent_id)?;
    let lease_expires_at = claimed_at
        .checked_add(WAKE_LEASE_DURATION_MS)
        .ok_or_else(|| invalid("claimed_at", "cannot compute the Wake lease deadline"))?;
    let stale_claimed = transaction
        .query_row(
            "SELECT wake_id FROM agent_wake_requests
             WHERE agent_id = ?1 AND status = 'claimed' AND lease_expires_at <= ?2
             ORDER BY sequence LIMIT 1",
            params![agent_id, claimed_at],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(read_error)?;
    if let Some(stale_wake_id) = stale_claimed {
        transaction
            .execute(
                "UPDATE agent_wake_requests
                 SET claim_token = NULL, lease_expires_at = NULL, claimed_at = NULL,
                     status = 'queued'
                 WHERE wake_id = ?1 AND status = 'claimed' AND lease_expires_at <= ?2",
                params![stale_wake_id, claimed_at],
            )
            .map_err(write_error)?;
    }
    let already_active = transaction
        .query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM agent_wake_requests
                 WHERE agent_id = ?1
                   AND status IN ('claimed', 'running', 'waiting_for_approval')
             )",
            [agent_id],
            |row| row.get::<_, bool>(0),
        )
        .map_err(read_error)?;
    if already_active {
        transaction.commit().map_err(write_error)?;
        return Ok(None);
    }
    let next_id = transaction
        .query_row(
            "SELECT wake_id FROM agent_wake_requests
             WHERE agent_id = ?1 AND status = 'queued'
             ORDER BY sequence LIMIT 1",
            [agent_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(read_error)?;
    let Some(wake_id) = next_id else {
        transaction.commit().map_err(write_error)?;
        return Ok(None);
    };
    transaction
        .execute(
            "UPDATE agent_wake_requests
             SET status = 'claimed', claim_token = ?1, lease_expires_at = ?2, claimed_at = ?3
             WHERE wake_id = ?4 AND status = 'queued'",
            params![claim_token, lease_expires_at, claimed_at, &wake_id],
        )
        .map_err(write_error)?;
    let wake = query_wake(&transaction, &wake_id)?
        .ok_or_else(|| corrupt("claimed Wake could not be read back"))?;
    transaction.commit().map_err(write_error)?;
    Ok(Some(wake))
}

pub fn renew_agent_wake_lease(
    connection: &mut Connection,
    wake_id: &str,
    claim_token: &str,
    renewed_at: i64,
) -> Result<AgentWakeRequestRecord, AgentGraphError> {
    validate_id("wake_id", wake_id)?;
    validate_id("claim_token", claim_token)?;
    validate_time(renewed_at)?;
    let lease_expires_at = renewed_at
        .checked_add(WAKE_LEASE_DURATION_MS)
        .ok_or_else(|| invalid("renewed_at", "cannot compute the Wake lease deadline"))?;
    let transaction = immediate(connection)?;
    let current = query_wake(&transaction, wake_id)?
        .ok_or_else(|| AgentGraphError::WakeNotFound(wake_id.to_string()))?;
    if !matches!(
        current.status,
        AgentWakeStatus::Claimed | AgentWakeStatus::Running | AgentWakeStatus::WaitingForApproval
    ) || current.claim_token.as_deref() != Some(claim_token)
    {
        return Err(conflict("Wake is not actively held by this claim token"));
    }
    let current_deadline = current
        .lease_expires_at
        .ok_or_else(|| corrupt("active Wake has no lease deadline"))?;
    if renewed_at > current_deadline {
        return Err(conflict("Wake lease has already expired"));
    }
    let advanced_deadline = lease_expires_at.max(current_deadline.saturating_add(1));
    transaction
        .execute(
            "UPDATE agent_wake_requests
             SET lease_expires_at = ?1
             WHERE wake_id = ?2 AND claim_token = ?3
               AND status IN ('claimed', 'running', 'waiting_for_approval')
               AND lease_expires_at = ?4",
            params![advanced_deadline, wake_id, claim_token, current_deadline],
        )
        .map_err(write_error)?;
    let renewed = query_wake(&transaction, wake_id)?
        .ok_or_else(|| corrupt("renewed Wake could not be read back"))?;
    transaction.commit().map_err(write_error)?;
    Ok(renewed)
}

pub fn transition_agent_wake(
    connection: &mut Connection,
    wake_id: &str,
    expected_status: AgentWakeStatus,
    requested_status: AgentWakeStatus,
    claim_token: Option<&str>,
    transitioned_at: i64,
) -> Result<AgentWakeRequestRecord, AgentGraphError> {
    validate_id("wake_id", wake_id)?;
    validate_time(transitioned_at)?;
    if let Some(token) = claim_token {
        validate_id("claim_token", token)?;
    }
    let transaction = immediate(connection)?;
    let current = query_wake(&transaction, wake_id)?
        .ok_or_else(|| AgentGraphError::WakeNotFound(wake_id.to_string()))?;
    if current.status != expected_status {
        return Err(conflict(
            "expected Wake status does not match persisted status",
        ));
    }
    if current.status == requested_status {
        if current.claim_token.as_deref() == claim_token || claim_token.is_none() {
            transaction.commit().map_err(write_error)?;
            return Ok(current);
        }
        return Err(conflict("Wake is held by another claim token"));
    }
    if !current.status.can_transition_to(requested_status) {
        return Err(AgentGraphError::IllegalTransition {
            current: current.status,
            requested: requested_status,
        });
    }
    if current.status != AgentWakeStatus::Queued && current.claim_token.as_deref() != claim_token {
        return Err(conflict("Wake is held by another claim token"));
    }
    if current.status != AgentWakeStatus::Queued
        && current
            .lease_expires_at
            .is_none_or(|deadline| transitioned_at > deadline)
    {
        return Err(conflict("Wake lease has expired"));
    }
    if requested_status == AgentWakeStatus::Completed {
        return Err(conflict(
            "completed Wake must be settled atomically with a result message",
        ));
    }

    let (next_claim_token, lease_expires_at, claimed_at, started_at, completed_at) =
        match requested_status {
            AgentWakeStatus::Queued => (None, None, None, None, None),
            AgentWakeStatus::Running | AgentWakeStatus::WaitingForApproval => (
                current.claim_token.as_deref(),
                current.lease_expires_at,
                current.claimed_at,
                Some(current.started_at.unwrap_or(transitioned_at)),
                None,
            ),
            AgentWakeStatus::Failed
            | AgentWakeStatus::Interrupted
            | AgentWakeStatus::Cancelled
            | AgentWakeStatus::OutcomeUnknown => (
                current.claim_token.as_deref(),
                current.lease_expires_at,
                current.claimed_at,
                current.started_at,
                Some(transitioned_at),
            ),
            AgentWakeStatus::Claimed => {
                return Err(conflict(
                    "queued Wake must be claimed through the claim API",
                ));
            }
            AgentWakeStatus::Completed => unreachable!(),
        };
    transaction
        .execute(
            "UPDATE agent_wake_requests
             SET status = ?1, claim_token = ?2, lease_expires_at = ?3, claimed_at = ?4,
                 started_at = ?5, completed_at = ?6, terminal_error = ?7
             WHERE wake_id = ?8 AND status = ?9",
            params![
                requested_status.as_str(),
                next_claim_token,
                lease_expires_at,
                claimed_at,
                started_at,
                completed_at,
                Option::<&str>::None,
                wake_id,
                expected_status.as_str(),
            ],
        )
        .map_err(write_error)?;
    let updated = query_wake(&transaction, wake_id)?
        .ok_or_else(|| corrupt("updated Wake could not be read back"))?;
    transaction.commit().map_err(write_error)?;
    Ok(updated)
}

pub fn finish_agent_wake_with_result(
    connection: &mut Connection,
    input: &FinishAgentWakeWithResultInput,
    completed_at: i64,
) -> Result<AgentWakeRequestRecord, AgentGraphError> {
    validate_time(completed_at)?;
    validate_id("claim_token", &input.claim_token)?;
    if !matches!(
        input.terminal_status,
        AgentWakeStatus::Completed
            | AgentWakeStatus::Failed
            | AgentWakeStatus::Interrupted
            | AgentWakeStatus::OutcomeUnknown
    ) {
        return Err(invalid(
            "terminal_status",
            "result settlement requires completed, failed, interrupted, or outcome_unknown",
        ));
    }
    if input.terminal_status == AgentWakeStatus::Completed && input.terminal_error.is_some() {
        return Err(invalid(
            "terminal_error",
            "completed Wake cannot carry a terminal error",
        ));
    }
    if !input
        .expected_status
        .can_transition_to(input.terminal_status)
    {
        return Err(AgentGraphError::IllegalTransition {
            current: input.expected_status,
            requested: input.terminal_status,
        });
    }
    if input.result_message.kind != AgentMailboxKind::Result {
        return Err(invalid("result_message.kind", "must be result"));
    }
    validate_message_input(&input.result_message, completed_at)?;
    let transaction = immediate(connection)?;
    let wake = query_wake(&transaction, &input.wake_id)?
        .ok_or_else(|| AgentGraphError::WakeNotFound(input.wake_id.clone()))?;
    if wake.status.is_terminal() {
        let existing_result = query_message(&transaction, &input.result_message.message_id)?;
        if wake.status == input.terminal_status
            && wake.result_message_id.as_deref() == Some(input.result_message.message_id.as_str())
            && wake.claim_token.as_deref() == Some(input.claim_token.as_str())
            && wake.terminal_error == input.terminal_error
            && existing_result
                .as_ref()
                .is_some_and(|message| message_matches_enqueue(message, &input.result_message))
        {
            transaction.commit().map_err(write_error)?;
            return Ok(wake);
        }
        return Err(conflict("Wake is already terminal with different facts"));
    }
    if wake.status != input.expected_status
        || wake.claim_token.as_deref() != Some(input.claim_token.as_str())
    {
        return Err(conflict(
            "Wake status or claim token does not match settlement",
        ));
    }
    if wake
        .lease_expires_at
        .is_none_or(|deadline| completed_at > deadline)
    {
        return Err(conflict("Wake lease has expired"));
    }
    if input.result_message.root_agent_id != wake.root_agent_id
        || input.result_message.sender_agent_id != wake.agent_id
        || input.result_message.recipient_agent_id != wake.requester_agent_id
    {
        return Err(conflict("Wake result participants do not match the Wake"));
    }
    let result = enqueue_message_in_transaction(&transaction, &input.result_message, completed_at)?;
    let result_id = result.record().message_id.clone();
    transaction
        .execute(
            "UPDATE agent_wake_requests
             SET status = ?1, result_message_id = ?2, terminal_error = ?3, completed_at = ?4
             WHERE wake_id = ?5 AND status = ?6 AND claim_token = ?7",
            params![
                input.terminal_status.as_str(),
                result_id,
                &input.terminal_error,
                completed_at,
                &input.wake_id,
                input.expected_status.as_str(),
                &input.claim_token,
            ],
        )
        .map_err(write_error)?;
    let settled = query_wake(&transaction, &input.wake_id)?
        .ok_or_else(|| corrupt("settled Wake could not be read back"))?;
    transaction.commit().map_err(write_error)?;
    Ok(settled)
}

fn enqueue_message_in_transaction(
    transaction: &Transaction<'_>,
    input: &EnqueueAgentMessageInput,
    created_at: i64,
) -> Result<IdempotentCreate<AgentMailboxMessageRecord>, AgentGraphError> {
    if let Some(existing) =
        query_message_by_request(transaction, &input.sender_agent_id, &input.request_id)?
    {
        if message_matches_enqueue(&existing, input) {
            return Ok(IdempotentCreate::Existing(existing));
        }
        return Err(conflict(
            "Mailbox request ID was reused with different facts",
        ));
    }
    ensure_active_pair(
        transaction,
        &input.root_agent_id,
        &input.sender_agent_id,
        &input.recipient_agent_id,
    )?;
    if query_message(transaction, &input.message_id)?.is_some() {
        return Err(conflict("Mailbox message ID is already in use"));
    }
    let projection_id_in_use = transaction
        .query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM agent_mailbox_messages WHERE projection_message_id = ?1
                 UNION ALL
                 SELECT 1 FROM messages WHERE id = ?1
             )",
            [&input.projection_message_id],
            |row| row.get::<_, bool>(0),
        )
        .map_err(read_error)?;
    if projection_id_in_use {
        return Err(conflict("Mailbox projection message ID is already in use"));
    }
    transaction
        .execute(
            "INSERT INTO agent_mailbox_messages (
                 message_id, schema_version, root_agent_id, sender_agent_id,
                 recipient_agent_id, request_id, kind, content, projection_message_id,
                 delivery_status, claim_token, lease_expires_at, created_at, claimed_at,
                 acknowledged_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9,
                       'queued', NULL, NULL, ?10, NULL, NULL)",
            params![
                &input.message_id,
                i64::from(AGENT_GRAPH_SCHEMA_VERSION),
                &input.root_agent_id,
                &input.sender_agent_id,
                &input.recipient_agent_id,
                &input.request_id,
                input.kind.as_str(),
                &input.content,
                &input.projection_message_id,
                created_at,
            ],
        )
        .map_err(write_error)?;
    let created = query_message(transaction, &input.message_id)?
        .ok_or_else(|| corrupt("created Mailbox message could not be read back"))?;
    Ok(IdempotentCreate::Created(created))
}

fn query_node(
    connection: &Connection,
    agent_id: &str,
) -> Result<Option<AgentNodeRecord>, AgentGraphError> {
    connection
        .query_row(
            &format!("{NODE_SELECT} WHERE agent_id = ?1"),
            [agent_id],
            read_node_row,
        )
        .optional()
        .map_err(read_error)?
        .map(decode_node)
        .transpose()
}

fn query_node_by_conversation(
    connection: &Connection,
    conversation_id: &str,
) -> Result<Option<AgentNodeRecord>, AgentGraphError> {
    connection
        .query_row(
            &format!("{NODE_SELECT} WHERE conversation_id = ?1"),
            [conversation_id],
            read_node_row,
        )
        .optional()
        .map_err(read_error)?
        .map(decode_node)
        .transpose()
}

fn query_node_by_root_conversation(
    connection: &Connection,
    conversation_id: &str,
) -> Result<Option<AgentNodeRecord>, AgentGraphError> {
    connection
        .query_row(
            &format!("{NODE_SELECT} WHERE root_conversation_id = ?1 AND parent_agent_id IS NULL"),
            [conversation_id],
            read_node_row,
        )
        .optional()
        .map_err(read_error)?
        .map(decode_node)
        .transpose()
}

fn query_node_by_request(
    connection: &Connection,
    root_agent_id: &str,
    request_id: &str,
) -> Result<Option<AgentNodeRecord>, AgentGraphError> {
    connection
        .query_row(
            &format!("{NODE_SELECT} WHERE root_agent_id = ?1 AND creation_request_id = ?2"),
            params![root_agent_id, request_id],
            read_node_row,
        )
        .optional()
        .map_err(read_error)?
        .map(decode_node)
        .transpose()
}

fn query_nodes<P: rusqlite::Params>(
    connection: &Connection,
    sql: &str,
    params: P,
) -> Result<Vec<AgentNodeRecord>, AgentGraphError> {
    let mut statement = connection.prepare(sql).map_err(read_error)?;
    let rows = statement
        .query_map(params, read_node_row)
        .map_err(read_error)?;
    rows.map(|row| row.map_err(read_error).and_then(decode_node))
        .collect()
}

#[allow(clippy::type_complexity)]
fn read_node_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<NodeRow> {
    Ok(NodeRow {
        agent_id: row.get(0)?,
        schema_version: row.get(1)?,
        root_agent_id: row.get(2)?,
        root_conversation_id: row.get(3)?,
        parent_agent_id: row.get(4)?,
        conversation_id: row.get(5)?,
        project_id: row.get(6)?,
        creation_request_id: row.get(7)?,
        task_name: row.get(8)?,
        task_path: row.get(9)?,
        template_id: row.get(10)?,
        template_project_id: row.get(11)?,
        template_machine_key: row.get(12)?,
        template_name: row.get(13)?,
        template_description: row.get(14)?,
        template_instructions: row.get(15)?,
        template_revision: row.get(16)?,
        template_model_config_id: row.get(17)?,
        model_config_id: row.get(18)?,
        model_display_name: row.get(19)?,
        model_supports_image: row.get(20)?,
        model_context_window: row.get(21)?,
        model_settings_revision: row.get(22)?,
        provider_connection_revision: row.get(23)?,
        provider_protocol_revision: row.get(24)?,
        lifecycle: row.get(25)?,
        revision: row.get(26)?,
        created_at: row.get(27)?,
        updated_at: row.get(28)?,
    })
}

struct NodeRow {
    agent_id: String,
    schema_version: i64,
    root_agent_id: String,
    root_conversation_id: String,
    parent_agent_id: Option<String>,
    conversation_id: String,
    project_id: Option<String>,
    creation_request_id: String,
    task_name: String,
    task_path: String,
    template_id: Option<String>,
    template_project_id: Option<String>,
    template_machine_key: Option<String>,
    template_name: Option<String>,
    template_description: Option<String>,
    template_instructions: Option<String>,
    template_revision: Option<i64>,
    template_model_config_id: Option<String>,
    model_config_id: Option<String>,
    model_display_name: Option<String>,
    model_supports_image: Option<i64>,
    model_context_window: Option<i64>,
    model_settings_revision: Option<String>,
    provider_connection_revision: Option<String>,
    provider_protocol_revision: Option<String>,
    lifecycle: String,
    revision: i64,
    created_at: i64,
    updated_at: i64,
}

fn decode_node(row: NodeRow) -> Result<AgentNodeRecord, AgentGraphError> {
    validate_schema_version(row.schema_version)?;
    let template_values = (
        row.template_id,
        row.template_project_id,
        row.template_machine_key,
        row.template_name,
        row.template_description,
        row.template_instructions,
        row.template_revision,
        row.template_model_config_id,
    );
    let template_snapshot = match template_values {
        (None, None, None, None, None, None, None, None) => None,
        (
            Some(template_id),
            Some(project_id),
            Some(machine_key),
            Some(name),
            Some(description),
            Some(instructions),
            Some(revision),
            Some(model_config_id),
        ) => Some(AgentTemplateSnapshot {
            template_id,
            project_id,
            machine_key,
            name,
            description,
            instructions,
            template_revision: positive_u64(revision, "template revision")?,
            model_config_id,
        }),
        _ => return Err(corrupt("Agent template snapshot is partial")),
    };
    let model_values = (
        row.model_config_id,
        row.model_display_name,
        row.model_supports_image,
        row.model_context_window,
        row.model_settings_revision,
        row.provider_connection_revision,
        row.provider_protocol_revision,
    );
    let model_snapshot = match model_values {
        (None, None, None, None, None, None, None) => None,
        (
            Some(model_config_id),
            Some(display_name),
            Some(supports_image),
            Some(context_window),
            Some(settings_revision),
            Some(connection_revision),
            Some(protocol_revision),
        ) => Some(AgentModelSelectionSnapshot {
            model_config_id,
            display_name,
            supports_image: decode_bool(supports_image, "model supports image")?,
            effective_context_window_tokens: u32::try_from(context_window)
                .ok()
                .filter(|value| *value > 0)
                .ok_or_else(|| corrupt("model context window is invalid"))?,
            model_settings_configuration_revision: settings_revision,
            provider_connection_revision: connection_revision,
            provider_protocol_revision: protocol_revision,
        }),
        _ => return Err(corrupt("Agent model snapshot is partial")),
    };
    Ok(AgentNodeRecord {
        agent_id: row.agent_id,
        root_agent_id: row.root_agent_id,
        root_conversation_id: row.root_conversation_id,
        parent_agent_id: row.parent_agent_id,
        conversation_id: row.conversation_id,
        project_id: row.project_id,
        creation_request_id: row.creation_request_id,
        task_name: row.task_name,
        task_path: row.task_path,
        template_snapshot,
        model_snapshot,
        lifecycle: AgentLifecycle::parse(&row.lifecycle)?,
        revision: positive_u64(row.revision, "Agent revision")?,
        created_at: row.created_at,
        updated_at: row.updated_at,
    })
}

fn query_message(
    connection: &Connection,
    message_id: &str,
) -> Result<Option<AgentMailboxMessageRecord>, AgentGraphError> {
    connection
        .query_row(
            &format!("{MESSAGE_SELECT} WHERE message_id = ?1"),
            [message_id],
            read_message_row,
        )
        .optional()
        .map_err(read_error)?
        .map(decode_message)
        .transpose()
}

fn query_message_by_request(
    connection: &Connection,
    sender_agent_id: &str,
    request_id: &str,
) -> Result<Option<AgentMailboxMessageRecord>, AgentGraphError> {
    connection
        .query_row(
            &format!("{MESSAGE_SELECT} WHERE sender_agent_id = ?1 AND request_id = ?2"),
            params![sender_agent_id, request_id],
            read_message_row,
        )
        .optional()
        .map_err(read_error)?
        .map(decode_message)
        .transpose()
}

fn query_message_by_claim_token(
    connection: &Connection,
    claim_token: &str,
) -> Result<Option<AgentMailboxMessageRecord>, AgentGraphError> {
    connection
        .query_row(
            &format!("{MESSAGE_SELECT} WHERE claim_token = ?1"),
            [claim_token],
            read_message_row,
        )
        .optional()
        .map_err(read_error)?
        .map(decode_message)
        .transpose()
}

fn read_message_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<MessageRow> {
    Ok(MessageRow {
        sequence: row.get(0)?,
        message_id: row.get(1)?,
        schema_version: row.get(2)?,
        root_agent_id: row.get(3)?,
        sender_agent_id: row.get(4)?,
        recipient_agent_id: row.get(5)?,
        request_id: row.get(6)?,
        kind: row.get(7)?,
        content: row.get(8)?,
        projection_message_id: row.get(9)?,
        delivery_status: row.get(10)?,
        claim_token: row.get(11)?,
        lease_expires_at: row.get(12)?,
        created_at: row.get(13)?,
        claimed_at: row.get(14)?,
        acknowledged_at: row.get(15)?,
    })
}

struct MessageRow {
    sequence: i64,
    message_id: String,
    schema_version: i64,
    root_agent_id: String,
    sender_agent_id: String,
    recipient_agent_id: String,
    request_id: String,
    kind: String,
    content: String,
    projection_message_id: String,
    delivery_status: String,
    claim_token: Option<String>,
    lease_expires_at: Option<i64>,
    created_at: i64,
    claimed_at: Option<i64>,
    acknowledged_at: Option<i64>,
}

fn decode_message(row: MessageRow) -> Result<AgentMailboxMessageRecord, AgentGraphError> {
    validate_schema_version(row.schema_version)?;
    Ok(AgentMailboxMessageRecord {
        sequence: positive_u64(row.sequence, "Mailbox sequence")?,
        message_id: row.message_id,
        root_agent_id: row.root_agent_id,
        sender_agent_id: row.sender_agent_id,
        recipient_agent_id: row.recipient_agent_id,
        request_id: row.request_id,
        kind: AgentMailboxKind::parse(&row.kind)?,
        content: row.content,
        projection_message_id: row.projection_message_id,
        delivery_status: AgentMailboxDeliveryStatus::parse(&row.delivery_status)?,
        claim_token: row.claim_token,
        lease_expires_at: row.lease_expires_at,
        created_at: row.created_at,
        claimed_at: row.claimed_at,
        acknowledged_at: row.acknowledged_at,
    })
}

fn query_wake(
    connection: &Connection,
    wake_id: &str,
) -> Result<Option<AgentWakeRequestRecord>, AgentGraphError> {
    connection
        .query_row(
            &format!("{WAKE_SELECT} WHERE wake_id = ?1"),
            [wake_id],
            read_wake_row,
        )
        .optional()
        .map_err(read_error)?
        .map(decode_wake)
        .transpose()
}

fn query_wake_by_request(
    connection: &Connection,
    requester_agent_id: &str,
    request_id: &str,
) -> Result<Option<AgentWakeRequestRecord>, AgentGraphError> {
    connection
        .query_row(
            &format!("{WAKE_SELECT} WHERE requester_agent_id = ?1 AND request_id = ?2"),
            params![requester_agent_id, request_id],
            read_wake_row,
        )
        .optional()
        .map_err(read_error)?
        .map(decode_wake)
        .transpose()
}

fn query_wake_by_claim_token(
    connection: &Connection,
    claim_token: &str,
) -> Result<Option<AgentWakeRequestRecord>, AgentGraphError> {
    connection
        .query_row(
            &format!("{WAKE_SELECT} WHERE claim_token = ?1"),
            [claim_token],
            read_wake_row,
        )
        .optional()
        .map_err(read_error)?
        .map(decode_wake)
        .transpose()
}

fn read_wake_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<WakeRow> {
    Ok(WakeRow {
        sequence: row.get(0)?,
        wake_id: row.get(1)?,
        schema_version: row.get(2)?,
        root_agent_id: row.get(3)?,
        agent_id: row.get(4)?,
        requester_agent_id: row.get(5)?,
        request_id: row.get(6)?,
        source_agent_message_id: row.get(7)?,
        status: row.get(8)?,
        claim_token: row.get(9)?,
        lease_expires_at: row.get(10)?,
        result_message_id: row.get(11)?,
        terminal_error: row.get(12)?,
        created_at: row.get(13)?,
        claimed_at: row.get(14)?,
        started_at: row.get(15)?,
        completed_at: row.get(16)?,
    })
}

struct WakeRow {
    sequence: i64,
    wake_id: String,
    schema_version: i64,
    root_agent_id: String,
    agent_id: String,
    requester_agent_id: String,
    request_id: String,
    source_agent_message_id: Option<String>,
    status: String,
    claim_token: Option<String>,
    lease_expires_at: Option<i64>,
    result_message_id: Option<String>,
    terminal_error: Option<String>,
    created_at: i64,
    claimed_at: Option<i64>,
    started_at: Option<i64>,
    completed_at: Option<i64>,
}

fn decode_wake(row: WakeRow) -> Result<AgentWakeRequestRecord, AgentGraphError> {
    validate_schema_version(row.schema_version)?;
    Ok(AgentWakeRequestRecord {
        sequence: positive_u64(row.sequence, "Wake sequence")?,
        wake_id: row.wake_id,
        root_agent_id: row.root_agent_id,
        agent_id: row.agent_id,
        requester_agent_id: row.requester_agent_id,
        request_id: row.request_id,
        source_agent_message_id: row.source_agent_message_id,
        status: AgentWakeStatus::parse(&row.status)?,
        claim_token: row.claim_token,
        lease_expires_at: row.lease_expires_at,
        result_message_id: row.result_message_id,
        terminal_error: row.terminal_error,
        created_at: row.created_at,
        claimed_at: row.claimed_at,
        started_at: row.started_at,
        completed_at: row.completed_at,
    })
}

fn validate_create_node(
    input: &CreateAgentNodeInput,
    created_at: i64,
) -> Result<(), AgentGraphError> {
    validate_id("agent_id", &input.agent_id)?;
    validate_id("root_agent_id", &input.root_agent_id)?;
    validate_id("parent_agent_id", &input.parent_agent_id)?;
    validate_id("conversation_id", &input.conversation_id)?;
    validate_request_id(&input.creation_request_id)?;
    validate_trimmed("task_name", &input.task_name, MAX_TASK_NAME_BYTES)?;
    validate_trimmed("task_path", &input.task_path, MAX_TASK_PATH_BYTES)?;
    validate_model_snapshot(&input.model_snapshot)?;
    if let Some(template) = &input.template_snapshot {
        validate_template_fields(template)?;
    }
    validate_time(created_at)
}

fn validate_message_input(
    input: &EnqueueAgentMessageInput,
    created_at: i64,
) -> Result<(), AgentGraphError> {
    validate_id("message_id", &input.message_id)?;
    validate_id("root_agent_id", &input.root_agent_id)?;
    validate_id("sender_agent_id", &input.sender_agent_id)?;
    validate_id("recipient_agent_id", &input.recipient_agent_id)?;
    validate_request_id(&input.request_id)?;
    validate_id("projection_message_id", &input.projection_message_id)?;
    if input.sender_agent_id == input.recipient_agent_id {
        return Err(invalid(
            "recipient_agent_id",
            "must differ from sender Agent",
        ));
    }
    validate_bounded_text("content", &input.content, 1, MAX_MESSAGE_BYTES)?;
    validate_time(created_at)
}

fn validate_wake_input(
    input: &EnqueueAgentWakeInput,
    created_at: i64,
) -> Result<(), AgentGraphError> {
    validate_id("wake_id", &input.wake_id)?;
    validate_id("root_agent_id", &input.root_agent_id)?;
    validate_id("agent_id", &input.agent_id)?;
    validate_id("requester_agent_id", &input.requester_agent_id)?;
    validate_request_id(&input.request_id)?;
    if let Some(message_id) = &input.source_agent_message_id {
        validate_id("source_agent_message_id", message_id)?;
    }
    if input.agent_id == input.requester_agent_id {
        return Err(invalid("agent_id", "must differ from requester Agent"));
    }
    validate_time(created_at)
}

fn validate_template_snapshot(
    connection: &Connection,
    template: Option<&AgentTemplateSnapshot>,
    project_id: Option<&str>,
) -> Result<(), AgentGraphError> {
    let Some(template) = template else {
        return Ok(());
    };
    validate_template_fields(template)?;
    if Some(template.project_id.as_str()) != project_id {
        return Err(conflict("template snapshot belongs to another project"));
    }
    let exact = connection
        .query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM agent_templates
                 WHERE template_id = ?1 AND project_id = ?2 AND machine_key = ?3
                   AND name = ?4 AND description = ?5 AND instructions = ?6
                   AND revision = ?7 AND model_config_id = ?8 AND enabled = 1
             )",
            params![
                &template.template_id,
                &template.project_id,
                &template.machine_key,
                &template.name,
                &template.description,
                &template.instructions,
                revision_to_sql(template.template_revision)?,
                &template.model_config_id,
            ],
            |row| row.get::<_, bool>(0),
        )
        .map_err(read_error)?;
    if !exact {
        return Err(conflict(
            "template snapshot is stale, disabled, or unavailable",
        ));
    }
    Ok(())
}

fn validate_template_fields(template: &AgentTemplateSnapshot) -> Result<(), AgentGraphError> {
    validate_id("template_id", &template.template_id)?;
    validate_id("template_project_id", &template.project_id)?;
    validate_trimmed("template_machine_key", &template.machine_key, 64)?;
    validate_trimmed("template_name", &template.name, 256)?;
    validate_bounded_text("template_description", &template.description, 0, 4 * 1024)?;
    validate_trimmed("template_instructions", &template.instructions, 64 * 1024)?;
    validate_trimmed("template_model_config_id", &template.model_config_id, 512)?;
    validate_revision(template.template_revision)
}

fn validate_model_snapshot(model: &AgentModelSelectionSnapshot) -> Result<(), AgentGraphError> {
    validate_trimmed("model_config_id", &model.model_config_id, 512)?;
    validate_trimmed("model_display_name", &model.display_name, 512)?;
    if model.effective_context_window_tokens == 0 {
        return Err(invalid("model_context_window", "must be positive"));
    }
    for (field, value) in [
        (
            "model_settings_configuration_revision",
            model.model_settings_configuration_revision.as_str(),
        ),
        (
            "provider_connection_revision",
            model.provider_connection_revision.as_str(),
        ),
        (
            "provider_protocol_revision",
            model.provider_protocol_revision.as_str(),
        ),
    ] {
        validate_trimmed(field, value, 512)?;
    }
    Ok(())
}

fn conversation_project(
    connection: &Connection,
    conversation_id: &str,
) -> Result<Option<String>, AgentGraphError> {
    connection
        .query_row(
            "SELECT project_id FROM conversations WHERE id = ?1",
            [conversation_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(read_error)?
        .ok_or_else(|| AgentGraphError::ConversationNotFound(conversation_id.to_string()))
}

fn ensure_active_agent(
    connection: &Connection,
    agent_id: &str,
) -> Result<AgentNodeRecord, AgentGraphError> {
    let agent = query_node(connection, agent_id)?
        .ok_or_else(|| AgentGraphError::AgentNotFound(agent_id.to_string()))?;
    if agent.lifecycle != AgentLifecycle::Active {
        return Err(conflict("Agent is not active"));
    }
    Ok(agent)
}

fn ensure_active_pair(
    connection: &Connection,
    root_agent_id: &str,
    sender_agent_id: &str,
    recipient_agent_id: &str,
) -> Result<(), AgentGraphError> {
    let sender = ensure_active_agent(connection, sender_agent_id)?;
    let recipient = ensure_active_agent(connection, recipient_agent_id)?;
    if sender.root_agent_id != root_agent_id || recipient.root_agent_id != root_agent_id {
        return Err(conflict("Agent communication cannot cross root trees"));
    }
    Ok(())
}

fn node_matches_create(record: &AgentNodeRecord, input: &CreateAgentNodeInput) -> bool {
    record.agent_id == input.agent_id
        && record.root_agent_id == input.root_agent_id
        && record.parent_agent_id.as_deref() == Some(input.parent_agent_id.as_str())
        && record.conversation_id == input.conversation_id
        && record.creation_request_id == input.creation_request_id
        && record.task_name == input.task_name
        && record.task_path == input.task_path
        && record.template_snapshot == input.template_snapshot
        && record.model_snapshot.as_ref() == Some(&input.model_snapshot)
}

fn message_matches_enqueue(
    record: &AgentMailboxMessageRecord,
    input: &EnqueueAgentMessageInput,
) -> bool {
    record.message_id == input.message_id
        && record.root_agent_id == input.root_agent_id
        && record.sender_agent_id == input.sender_agent_id
        && record.recipient_agent_id == input.recipient_agent_id
        && record.request_id == input.request_id
        && record.kind == input.kind
        && record.content == input.content
        && record.projection_message_id == input.projection_message_id
}

fn wake_matches_enqueue(record: &AgentWakeRequestRecord, input: &EnqueueAgentWakeInput) -> bool {
    record.wake_id == input.wake_id
        && record.root_agent_id == input.root_agent_id
        && record.agent_id == input.agent_id
        && record.requester_agent_id == input.requester_agent_id
        && record.request_id == input.request_id
        && record.source_agent_message_id == input.source_agent_message_id
}

fn immediate(connection: &mut Connection) -> Result<Transaction<'_>, AgentGraphError> {
    connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(write_error)
}

fn validate_schema_version(value: i64) -> Result<(), AgentGraphError> {
    if value == i64::from(AGENT_GRAPH_SCHEMA_VERSION) {
        Ok(())
    } else {
        Err(corrupt("unsupported Agent graph record schema version"))
    }
}

fn validate_id(field: &'static str, value: &str) -> Result<(), AgentGraphError> {
    validate_bounded_text(field, value, 1, MAX_ID_BYTES)
}

fn validate_request_id(value: &str) -> Result<(), AgentGraphError> {
    validate_bounded_text("request_id", value, 1, MAX_REQUEST_ID_BYTES)
}

fn validate_trimmed(
    field: &'static str,
    value: &str,
    maximum: usize,
) -> Result<(), AgentGraphError> {
    validate_bounded_text(field, value, 1, maximum)?;
    if value.trim() != value {
        return Err(invalid(
            field,
            "must not contain leading or trailing whitespace",
        ));
    }
    Ok(())
}

fn validate_bounded_text(
    field: &'static str,
    value: &str,
    minimum: usize,
    maximum: usize,
) -> Result<(), AgentGraphError> {
    if value.len() < minimum || value.len() > maximum || value.contains('\0') {
        return Err(invalid(
            field,
            format!("must contain {minimum}..={maximum} bytes and no NUL"),
        ));
    }
    Ok(())
}

fn validate_time(value: i64) -> Result<(), AgentGraphError> {
    if value < 0 {
        Err(invalid("timestamp", "must be non-negative"))
    } else {
        Ok(())
    }
}

fn validate_revision(value: u64) -> Result<(), AgentGraphError> {
    if value == 0 || value > i64::MAX as u64 {
        Err(invalid("revision", "must be a positive SQLite integer"))
    } else {
        Ok(())
    }
}

fn revision_to_sql(value: u64) -> Result<i64, AgentGraphError> {
    i64::try_from(value).map_err(|_| invalid("revision", "is outside SQLite integer range"))
}

fn positive_u64(value: i64, label: &str) -> Result<u64, AgentGraphError> {
    u64::try_from(value)
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| corrupt(format!("{label} is not positive")))
}

fn decode_bool(value: i64, label: &str) -> Result<bool, AgentGraphError> {
    match value {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(corrupt(format!("{label} is not boolean"))),
    }
}

fn invalid(field: &'static str, reason: impl Into<String>) -> AgentGraphError {
    AgentGraphError::InvalidInput {
        field,
        reason: reason.into(),
    }
}

fn conflict(reason: impl Into<String>) -> AgentGraphError {
    AgentGraphError::Conflict(reason.into())
}

fn corrupt(reason: impl Into<String>) -> AgentGraphError {
    AgentGraphError::CorruptRecord(reason.into())
}

fn read_error(error: rusqlite::Error) -> AgentGraphError {
    match error {
        rusqlite::Error::InvalidColumnType(..)
        | rusqlite::Error::FromSqlConversionFailure(..)
        | rusqlite::Error::IntegralValueOutOfRange(..) => {
            corrupt("persisted Agent graph record has an invalid type or range")
        }
        _ => AgentGraphError::StorageUnavailable("database read failed".to_string()),
    }
}

fn write_error(_: rusqlite::Error) -> AgentGraphError {
    AgentGraphError::StorageUnavailable("database transaction failed".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{
        chat_repository, migrations,
        models::{ChatConversationMetaRecord, ChatMessageRecord},
    };

    fn connection() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        migrations::run_migrations(&connection).unwrap();
        connection
    }

    fn insert_project(connection: &Connection, project_id: &str) {
        connection
            .execute(
                "INSERT INTO projects (id, name, path, created_at, pinned_at, updated_at)
                 VALUES (?1, ?1, NULL, 1, NULL, 1)",
                [project_id],
            )
            .unwrap();
    }

    fn insert_conversation(connection: &Connection, id: &str, project_id: Option<&str>) {
        connection
            .execute(
                "INSERT INTO conversations (
                     id, project_id, model_id, title, created_at, updated_at,
                     pinned_at, archived_at, unread_at
                 ) VALUES (?1, ?2, 'model-a', ?1, 1, 1, NULL, NULL, NULL)",
                params![id, project_id],
            )
            .unwrap();
    }

    fn model_snapshot(model_id: &str) -> AgentModelSelectionSnapshot {
        AgentModelSelectionSnapshot {
            model_config_id: model_id.to_string(),
            display_name: format!("Model {model_id}"),
            supports_image: false,
            effective_context_window_tokens: 64_000,
            model_settings_configuration_revision: "model-settings-v1:test".to_string(),
            provider_connection_revision: "provider-connection-v1:test".to_string(),
            provider_protocol_revision: "provider-protocol-v1:test".to_string(),
        }
    }

    fn ensure_root(
        connection: &mut Connection,
        agent_id: &str,
        conversation_id: &str,
    ) -> AgentNodeRecord {
        ensure_root_agent(
            connection,
            &EnsureRootAgentInput {
                agent_id: agent_id.to_string(),
                conversation_id: conversation_id.to_string(),
                creation_request_id: format!("ensure-{agent_id}"),
                task_name: "Root".to_string(),
            },
            10,
        )
        .unwrap()
        .record()
        .clone()
    }

    fn child_input(
        agent_id: &str,
        root_agent_id: &str,
        parent_agent_id: &str,
        conversation_id: &str,
        task_name: &str,
        task_path: &str,
    ) -> CreateAgentNodeInput {
        CreateAgentNodeInput {
            agent_id: agent_id.to_string(),
            root_agent_id: root_agent_id.to_string(),
            parent_agent_id: parent_agent_id.to_string(),
            conversation_id: conversation_id.to_string(),
            creation_request_id: format!("spawn-{agent_id}"),
            task_name: task_name.to_string(),
            task_path: task_path.to_string(),
            template_snapshot: None,
            model_snapshot: model_snapshot("model-a"),
        }
    }

    fn setup_tree() -> Connection {
        let mut connection = connection();
        insert_project(&connection, "project-a");
        for conversation in [
            "conversation-root",
            "conversation-child",
            "conversation-grand",
        ] {
            insert_conversation(&connection, conversation, Some("project-a"));
        }
        ensure_root(&mut connection, "agent-root", "conversation-root");
        create_agent_node(
            &mut connection,
            &child_input(
                "agent-child",
                "agent-root",
                "agent-root",
                "conversation-child",
                "review",
                "/root/review",
            ),
            11,
        )
        .unwrap();
        connection
    }

    fn message_input(
        suffix: &str,
        sender: &str,
        recipient: &str,
        kind: AgentMailboxKind,
    ) -> EnqueueAgentMessageInput {
        EnqueueAgentMessageInput {
            message_id: format!("message-{suffix}"),
            root_agent_id: "agent-root".to_string(),
            sender_agent_id: sender.to_string(),
            recipient_agent_id: recipient.to_string(),
            request_id: format!("request-message-{suffix}"),
            kind,
            content: format!("payload {suffix}"),
            projection_message_id: format!("projection-{suffix}"),
        }
    }

    fn chat_message(id: &str, role: &str, content: &str, created_at: i64) -> ChatMessageRecord {
        ChatMessageRecord {
            id: id.to_string(),
            role: role.to_string(),
            content: content.to_string(),
            created_at,
            status: Some("sent".to_string()),
            attachments: Vec::new(),
            agent_run_json: None,
            ui_state_json: None,
        }
    }

    fn wake_input(suffix: &str) -> EnqueueAgentWakeInput {
        EnqueueAgentWakeInput {
            wake_id: format!("wake-{suffix}"),
            root_agent_id: "agent-root".to_string(),
            agent_id: "agent-child".to_string(),
            requester_agent_id: "agent-root".to_string(),
            request_id: format!("request-wake-{suffix}"),
            source_agent_message_id: None,
        }
    }

    #[test]
    fn root_and_multilevel_tree_are_idempotent_and_tree_scoped() {
        let mut connection = setup_tree();
        let root_retry = ensure_root_agent(
            &mut connection,
            &EnsureRootAgentInput {
                agent_id: "agent-root".to_string(),
                conversation_id: "conversation-root".to_string(),
                creation_request_id: "ensure-agent-root".to_string(),
                task_name: "Root".to_string(),
            },
            99,
        )
        .unwrap();
        assert!(matches!(root_retry, IdempotentCreate::Existing(_)));

        let grandchild = create_agent_node(
            &mut connection,
            &child_input(
                "agent-grand",
                "agent-root",
                "agent-child",
                "conversation-grand",
                "evidence",
                "/root/review/evidence",
            ),
            12,
        )
        .unwrap();
        assert!(matches!(grandchild, IdempotentCreate::Created(_)));
        assert_eq!(list_agent_tree(&connection, "agent-root").unwrap().len(), 3);
        assert_eq!(
            list_agent_children(&connection, "agent-root", "agent-child")
                .unwrap()
                .len(),
            1
        );

        insert_project(&connection, "project-b");
        insert_conversation(&connection, "conversation-root-b", Some("project-b"));
        insert_conversation(&connection, "conversation-cross", Some("project-b"));
        ensure_root(&mut connection, "agent-root-b", "conversation-root-b");
        let cross_tree = create_agent_node(
            &mut connection,
            &child_input(
                "agent-cross",
                "agent-root-b",
                "agent-child",
                "conversation-cross",
                "cross",
                "/root/review/cross",
            ),
            13,
        );
        assert!(matches!(cross_tree, Err(AgentGraphError::Conflict(_))));

        insert_conversation(&connection, "conversation-duplicate", Some("project-a"));
        let duplicate_name = create_agent_node(
            &mut connection,
            &child_input(
                "agent-duplicate",
                "agent-root",
                "agent-root",
                "conversation-duplicate",
                "review",
                "/root/review",
            ),
            14,
        );
        assert!(matches!(duplicate_name, Err(AgentGraphError::Conflict(_))));
    }

    #[test]
    fn child_binding_requires_a_fresh_matching_conversation_and_freezes_only_its_model() {
        let mut connection = connection();
        insert_project(&connection, "project-a");
        insert_conversation(&connection, "conversation-root", Some("project-a"));
        ensure_root(&mut connection, "agent-root", "conversation-root");

        insert_conversation(
            &connection,
            "conversation-model-mismatch",
            Some("project-a"),
        );
        let mut mismatched = child_input(
            "agent-model-mismatch",
            "agent-root",
            "agent-root",
            "conversation-model-mismatch",
            "model_mismatch",
            "/root/model_mismatch",
        );
        mismatched.model_snapshot = model_snapshot("model-b");
        assert!(matches!(
            create_agent_node(&mut connection, &mismatched, 11),
            Err(AgentGraphError::Conflict(reason))
                if reason.contains("model does not match")
        ));
        assert!(get_agent_node(&connection, "agent-model-mismatch")
            .unwrap()
            .is_none());

        insert_conversation(&connection, "conversation-preloaded", Some("project-a"));
        connection
            .execute(
                "INSERT INTO messages (
                     id, conversation_id, role, content, status, created_at, position
                 ) VALUES (
                     'preloaded-human', 'conversation-preloaded', 'user',
                     'existing human history', 'sent', 1, 0
                 )",
                [],
            )
            .unwrap();
        let preloaded = child_input(
            "agent-preloaded",
            "agent-root",
            "agent-root",
            "conversation-preloaded",
            "preloaded",
            "/root/preloaded",
        );
        assert!(matches!(
            create_agent_node(&mut connection, &preloaded, 12),
            Err(AgentGraphError::Conflict(reason)) if reason.contains("fresh Conversation")
        ));
        assert!(get_agent_node(&connection, "agent-preloaded")
            .unwrap()
            .is_none());

        insert_conversation(&connection, "conversation-fresh-child", Some("project-a"));
        create_agent_node(
            &mut connection,
            &child_input(
                "agent-fresh-child",
                "agent-root",
                "agent-root",
                "conversation-fresh-child",
                "fresh_child",
                "/root/fresh_child",
            ),
            13,
        )
        .unwrap();

        let child_before =
            chat_repository::get_conversation(&connection, "conversation-fresh-child")
                .unwrap()
                .unwrap();
        assert!(chat_repository::save_conversation_meta(
            &connection,
            &ChatConversationMetaRecord {
                id: child_before.id.clone(),
                project_id: child_before.project_id.clone(),
                model_id: Some("model-b".to_string()),
                title: child_before.title.clone(),
                created_at: child_before.created_at,
                updated_at: 20,
                pinned_at: child_before.pinned_at,
                archived_at: child_before.archived_at,
                unread_at: child_before.unread_at,
            },
        )
        .is_err());
        let mut child_full_save = child_before;
        child_full_save.model_id = Some("model-b".to_string());
        child_full_save.updated_at = 21;
        assert!(chat_repository::save_conversation(&mut connection, child_full_save).is_err());
        assert_eq!(
            chat_repository::get_conversation(&connection, "conversation-fresh-child")
                .unwrap()
                .unwrap()
                .model_id
                .as_deref(),
            Some("model-a")
        );

        let mut root = chat_repository::get_conversation(&connection, "conversation-root")
            .unwrap()
            .unwrap();
        root.model_id = Some("model-b".to_string());
        root.updated_at = 22;
        chat_repository::save_conversation(&mut connection, root).unwrap();
        assert_eq!(
            chat_repository::get_conversation(&connection, "conversation-root")
                .unwrap()
                .unwrap()
                .model_id
                .as_deref(),
            Some("model-b")
        );
    }

    #[test]
    fn mailbox_fifo_projection_origin_and_recovery_are_atomic() {
        let mut connection = setup_tree();
        let first_input = message_input(
            "first",
            "agent-root",
            "agent-child",
            AgentMailboxKind::Message,
        );
        let second_input = message_input(
            "second",
            "agent-root",
            "agent-child",
            AgentMailboxKind::Message,
        );
        assert!(matches!(
            enqueue_agent_message(&mut connection, &first_input, 20).unwrap(),
            IdempotentCreate::Created(_)
        ));
        assert!(matches!(
            enqueue_agent_message(&mut connection, &first_input, 21).unwrap(),
            IdempotentCreate::Existing(_)
        ));
        enqueue_agent_message(&mut connection, &second_input, 22).unwrap();

        let first =
            claim_next_agent_message(&mut connection, "agent-child", "claim-message-first", 23)
                .unwrap()
                .unwrap();
        assert_eq!(first.message_id, "message-first");
        assert!(acknowledge_agent_message_with_projection(
            &mut connection,
            "message-first",
            "wrong-claim",
            24,
        )
        .is_err());
        let acknowledged = acknowledge_agent_message_with_projection(
            &mut connection,
            "message-first",
            "claim-message-first",
            25,
        )
        .unwrap();
        assert_eq!(
            acknowledged.delivery_status,
            AgentMailboxDeliveryStatus::Acknowledged
        );
        assert_eq!(
            conversation_message_origin(&connection, "conversation-child", "projection-first")
                .unwrap(),
            ConversationMessageOrigin::Agent {
                sender_agent_id: "agent-root".to_string(),
                source_agent_message_id: "message-first".to_string(),
            }
        );
        let role: String = connection
            .query_row(
                "SELECT role FROM messages WHERE id = 'projection-first'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(role, "user");
        assert!(connection
            .execute(
                "UPDATE messages SET content = 'forged' WHERE id = 'projection-first'",
                [],
            )
            .is_err());

        let second =
            claim_next_agent_message(&mut connection, "agent-child", "claim-message-second", 26)
                .unwrap()
                .unwrap();
        assert_eq!(second.message_id, "message-second");
        connection
            .execute_batch(
                "CREATE TRIGGER fail_second_projection
                 BEFORE INSERT ON messages WHEN NEW.id = 'projection-second'
                 BEGIN SELECT RAISE(ABORT, 'forced projection failure'); END;",
            )
            .unwrap();
        assert!(acknowledge_agent_message_with_projection(
            &mut connection,
            "message-second",
            "claim-message-second",
            27,
        )
        .is_err());
        assert_eq!(
            get_agent_message(&connection, "message-second")
                .unwrap()
                .unwrap()
                .delivery_status,
            AgentMailboxDeliveryStatus::Claimed
        );
        let projection_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM messages WHERE id = 'projection-second'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(projection_count, 0);
        connection
            .execute("DROP TRIGGER fail_second_projection", [])
            .unwrap();
        acknowledge_agent_message_with_projection(
            &mut connection,
            "message-second",
            "claim-message-second",
            28,
        )
        .unwrap();
    }

    #[test]
    fn mailbox_claims_preserve_fifo_and_recover_after_a_pre_projection_crash() {
        let mut connection = setup_tree();
        let first = message_input(
            "recover-first",
            "agent-root",
            "agent-child",
            AgentMailboxKind::Message,
        );
        let second = message_input(
            "recover-second",
            "agent-root",
            "agent-child",
            AgentMailboxKind::Message,
        );
        enqueue_agent_message(&mut connection, &first, 100).unwrap();
        enqueue_agent_message(&mut connection, &second, 101).unwrap();

        let crashed = claim_next_agent_message(
            &mut connection,
            "agent-child",
            "claim-before-message-crash",
            102,
        )
        .unwrap()
        .unwrap();
        assert_eq!(crashed.message_id, first.message_id);
        assert!(claim_next_agent_message(
            &mut connection,
            "agent-child",
            "claim-must-not-overtake",
            103,
        )
        .unwrap()
        .is_none());

        let expired_at = crashed.lease_expires_at.unwrap();
        let recovered = claim_next_agent_message(
            &mut connection,
            "agent-child",
            "claim-after-message-restart",
            expired_at,
        )
        .unwrap()
        .unwrap();
        assert_eq!(recovered.message_id, first.message_id);
        assert_eq!(
            recovered.claim_token.as_deref(),
            Some("claim-after-message-restart")
        );
        assert!(acknowledge_agent_message_with_projection(
            &mut connection,
            &first.message_id,
            "claim-before-message-crash",
            expired_at,
        )
        .is_err());
        acknowledge_agent_message_with_projection(
            &mut connection,
            &first.message_id,
            "claim-after-message-restart",
            expired_at + 1,
        )
        .unwrap();

        let next = claim_next_agent_message(
            &mut connection,
            "agent-child",
            "claim-after-first-ack",
            expired_at + 2,
        )
        .unwrap()
        .unwrap();
        assert_eq!(next.message_id, second.message_id);
        let projection_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM messages
                 WHERE source_agent_message_id = 'message-recover-first'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(projection_count, 1);
    }

    #[test]
    fn bound_conversations_keep_normal_loop_messages_while_agent_projections_are_immutable() {
        let mut connection = setup_tree();

        let mut root = chat_repository::get_conversation(&connection, "conversation-root")
            .unwrap()
            .unwrap();
        root.messages = vec![
            chat_message("root-user", "user", "hello", 20),
            chat_message("root-assistant", "assistant", "hello back", 21),
        ];
        root.updated_at = 21;
        chat_repository::save_conversation(&mut connection, root).unwrap();
        let loaded_root = chat_repository::get_conversation(&connection, "conversation-root")
            .unwrap()
            .unwrap();
        assert_eq!(loaded_root.messages.len(), 2);
        assert_eq!(
            conversation_message_origin(&connection, "conversation-root", "root-user").unwrap(),
            ConversationMessageOrigin::Human
        );

        let mut stale_root_snapshot = loaded_root;
        let concurrent_result = message_input(
            "concurrent-result",
            "agent-child",
            "agent-root",
            AgentMailboxKind::Result,
        );
        enqueue_agent_message(&mut connection, &concurrent_result, 22).unwrap();
        claim_next_agent_message(&mut connection, "agent-root", "claim-concurrent-result", 23)
            .unwrap();
        acknowledge_agent_message_with_projection(
            &mut connection,
            "message-concurrent-result",
            "claim-concurrent-result",
            24,
        )
        .unwrap();
        stale_root_snapshot.messages.push(chat_message(
            "root-late-assistant",
            "assistant",
            "turn finished",
            25,
        ));
        stale_root_snapshot.updated_at = 25;
        chat_repository::save_conversation(&mut connection, stale_root_snapshot).unwrap();
        let merged_root = chat_repository::get_conversation(&connection, "conversation-root")
            .unwrap()
            .unwrap();
        assert_eq!(merged_root.messages.len(), 4);
        assert!(merged_root
            .messages
            .iter()
            .any(|message| message.id == "projection-concurrent-result"));
        let positions = connection
            .prepare(
                "SELECT position FROM messages WHERE conversation_id = 'conversation-root'
                 ORDER BY position",
            )
            .unwrap()
            .query_map([], |row| row.get::<_, i64>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(positions, vec![0, 1, 2, 3]);

        let task = message_input(
            "loop-input",
            "agent-root",
            "agent-child",
            AgentMailboxKind::Message,
        );
        enqueue_agent_message(&mut connection, &task, 22).unwrap();
        claim_next_agent_message(&mut connection, "agent-child", "claim-loop-input", 23).unwrap();
        acknowledge_agent_message_with_projection(
            &mut connection,
            "message-loop-input",
            "claim-loop-input",
            24,
        )
        .unwrap();

        let mut child = chat_repository::get_conversation(&connection, "conversation-child")
            .unwrap()
            .unwrap();
        assert_eq!(child.messages[0].role, "user");
        child.messages.push(chat_message(
            "child-assistant",
            "assistant",
            "review complete",
            25,
        ));
        child.updated_at = 25;
        chat_repository::save_conversation(&mut connection, child).unwrap();
        assert_eq!(
            chat_repository::get_conversation(&connection, "conversation-child")
                .unwrap()
                .unwrap()
                .messages
                .len(),
            2
        );

        connection
            .execute(
                "INSERT INTO messages (
                     id, conversation_id, role, content, status, created_at, position
                 ) VALUES (
                     'child-normal-before-projection', 'conversation-child', 'assistant',
                     'ordinary earlier output', 'sent', 20, -1
                 )",
                [],
            )
            .unwrap();
        assert!(connection
            .execute(
                "UPDATE messages SET role = 'user'
                 WHERE id = 'child-normal-before-projection'",
                [],
            )
            .is_err());

        chat_repository::delete_messages(
            &mut connection,
            "conversation-child",
            &["child-assistant".to_string()],
        )
        .unwrap();
        chat_repository::delete_messages(
            &mut connection,
            "conversation-child",
            &["child-normal-before-projection".to_string()],
        )
        .unwrap();
        assert!(chat_repository::delete_messages(
            &mut connection,
            "conversation-child",
            &["projection-loop-input".to_string()],
        )
        .is_err());
        assert!(connection
            .execute(
                "UPDATE messages SET position = 99 WHERE id = 'projection-loop-input'",
                [],
            )
            .is_err());
        let before_forbidden_save =
            chat_repository::get_conversation(&connection, "conversation-child")
                .unwrap()
                .unwrap();
        let mut forbidden_human_input = before_forbidden_save.clone();
        forbidden_human_input.messages.push(chat_message(
            "forbidden-child-human",
            "user",
            "direct user bypass",
            100,
        ));
        assert!(
            chat_repository::save_conversation(&mut connection, forbidden_human_input).is_err()
        );
        let after_forbidden_save =
            chat_repository::get_conversation(&connection, "conversation-child")
                .unwrap()
                .unwrap();
        assert_eq!(
            after_forbidden_save.messages.len(),
            before_forbidden_save.messages.len()
        );
        assert!(!after_forbidden_save
            .messages
            .iter()
            .any(|message| message.id == "forbidden-child-human"));
        assert!(connection
            .execute(
                "DELETE FROM messages WHERE id = 'projection-loop-input'",
                [],
            )
            .is_err());
        assert!(connection
            .execute(
                "INSERT OR REPLACE INTO messages (
                     id, conversation_id, role, content, status, created_at, position
                 ) VALUES (
                     'projection-loop-input', 'conversation-child', 'user',
                     'forged replacement', 'sent', 99, 99
                 )",
                [],
            )
            .is_err());
    }

    #[test]
    fn wake_claim_transition_and_result_settlement_are_single_turn_and_idempotent() {
        let mut connection = setup_tree();
        let source = message_input(
            "wake-source",
            "agent-root",
            "agent-child",
            AgentMailboxKind::Task,
        );
        enqueue_agent_message(&mut connection, &source, 29).unwrap();
        let wake_before_delivery = EnqueueAgentWakeInput {
            source_agent_message_id: Some(source.message_id.clone()),
            ..wake_input("before-delivery")
        };
        assert!(matches!(
            enqueue_agent_wake(&mut connection, &wake_before_delivery, 30),
            Err(AgentGraphError::Conflict(_))
        ));
        claim_next_agent_message(&mut connection, "agent-child", "claim-wake-source", 31).unwrap();
        let (acknowledged_source, atomic_wake) = acknowledge_agent_task_with_projection_and_wake(
            &mut connection,
            &AcknowledgeAgentTaskAndWakeInput {
                message_id: "message-wake-source".to_string(),
                message_claim_token: "claim-wake-source".to_string(),
                wake: wake_before_delivery.clone(),
            },
            32,
        )
        .unwrap();
        assert_eq!(
            acknowledged_source.delivery_status,
            AgentMailboxDeliveryStatus::Acknowledged
        );
        assert_eq!(atomic_wake.wake_id, "wake-before-delivery");
        assert!(matches!(
            enqueue_agent_wake(&mut connection, &wake_before_delivery, 33).unwrap(),
            IdempotentCreate::Existing(_)
        ));

        let wake_one = wake_input("one");
        let wake_two = wake_input("two");
        assert!(matches!(
            enqueue_agent_wake(&mut connection, &wake_one, 34).unwrap(),
            IdempotentCreate::Created(_)
        ));
        assert!(matches!(
            enqueue_agent_wake(&mut connection, &wake_one, 35).unwrap(),
            IdempotentCreate::Existing(_)
        ));
        enqueue_agent_wake(&mut connection, &wake_two, 36).unwrap();

        let claimed =
            claim_next_agent_wake(&mut connection, "agent-child", "claim-before-delivery", 37)
                .unwrap()
                .unwrap();
        assert_eq!(claimed.wake_id, "wake-before-delivery");
        transition_agent_wake(
            &mut connection,
            "wake-before-delivery",
            AgentWakeStatus::Claimed,
            AgentWakeStatus::Cancelled,
            Some("claim-before-delivery"),
            38,
        )
        .unwrap();

        let claimed = claim_next_agent_wake(&mut connection, "agent-child", "claim-wake-one", 39)
            .unwrap()
            .unwrap();
        assert_eq!(claimed.wake_id, "wake-one");
        let original_deadline = claimed.lease_expires_at.unwrap();
        let renewed =
            renew_agent_wake_lease(&mut connection, "wake-one", "claim-wake-one", 40).unwrap();
        assert!(renewed.lease_expires_at.unwrap() > original_deadline);
        assert!(
            claim_next_agent_wake(&mut connection, "agent-child", "claim-wake-two", 41,)
                .unwrap()
                .is_none()
        );
        transition_agent_wake(
            &mut connection,
            "wake-one",
            AgentWakeStatus::Claimed,
            AgentWakeStatus::Running,
            Some("claim-wake-one"),
            42,
        )
        .unwrap();

        let finish = FinishAgentWakeWithResultInput {
            wake_id: "wake-one".to_string(),
            expected_status: AgentWakeStatus::Running,
            claim_token: "claim-wake-one".to_string(),
            terminal_status: AgentWakeStatus::Completed,
            terminal_error: None,
            result_message: message_input(
                "result-one",
                "agent-child",
                "agent-root",
                AgentMailboxKind::Result,
            ),
        };
        let settled = finish_agent_wake_with_result(&mut connection, &finish, 43).unwrap();
        assert_eq!(settled.status, AgentWakeStatus::Completed);
        assert_eq!(
            settled.result_message_id.as_deref(),
            Some("message-result-one")
        );
        assert_eq!(
            finish_agent_wake_with_result(&mut connection, &finish, 44)
                .unwrap()
                .status,
            AgentWakeStatus::Completed
        );
        let second = claim_next_agent_wake(&mut connection, "agent-child", "claim-wake-two", 45)
            .unwrap()
            .unwrap();
        assert_eq!(second.wake_id, "wake-two");
        assert!(matches!(
            transition_agent_wake(
                &mut connection,
                "wake-one",
                AgentWakeStatus::Completed,
                AgentWakeStatus::Running,
                Some("claim-wake-one"),
                46,
            ),
            Err(AgentGraphError::IllegalTransition { .. })
        ));
    }

    #[test]
    fn an_expired_unstarted_wake_claim_is_recovered_without_a_second_active_turn() {
        let mut connection = setup_tree();
        enqueue_agent_wake(&mut connection, &wake_input("recovery"), 50).unwrap();
        let first = claim_next_agent_wake(&mut connection, "agent-child", "claim-before-crash", 51)
            .unwrap()
            .unwrap();
        let first_deadline = first.lease_expires_at.unwrap();

        let recovered = claim_next_agent_wake(
            &mut connection,
            "agent-child",
            "claim-after-restart",
            first_deadline,
        )
        .unwrap()
        .unwrap();
        assert_eq!(recovered.wake_id, "wake-recovery");
        assert_eq!(
            recovered.claim_token.as_deref(),
            Some("claim-after-restart")
        );
        assert!(recovered.lease_expires_at.unwrap() > first_deadline);

        let active_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM agent_wake_requests
                 WHERE agent_id = 'agent-child'
                   AND status IN ('claimed', 'running', 'waiting_for_approval')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(active_count, 1);
        assert!(renew_agent_wake_lease(
            &mut connection,
            "wake-recovery",
            "claim-before-crash",
            first_deadline,
        )
        .is_err());
    }

    #[test]
    fn bound_deletion_and_lifecycle_transitions_fail_closed() {
        let mut connection = setup_tree();
        assert!(matches!(
            ensure_conversation_unbound(&connection, "conversation-child"),
            Err(AgentGraphError::BoundConversation(_))
        ));
        assert!(matches!(
            ensure_project_unbound(&connection, "project-a"),
            Err(AgentGraphError::BoundProject(_))
        ));
        assert!(connection
            .execute(
                "DELETE FROM conversations WHERE id = 'conversation-child'",
                [],
            )
            .is_err());

        assert!(transition_agent_lifecycle(
            &mut connection,
            "agent-root",
            1,
            AgentLifecycle::Active,
            AgentLifecycle::Archived,
            39,
        )
        .is_err());

        let pending = message_input(
            "before-archive",
            "agent-root",
            "agent-child",
            AgentMailboxKind::Message,
        );
        enqueue_agent_message(&mut connection, &pending, 40).unwrap();
        assert!(transition_agent_lifecycle(
            &mut connection,
            "agent-child",
            1,
            AgentLifecycle::Active,
            AgentLifecycle::Archived,
            41,
        )
        .is_err());
        claim_next_agent_message(&mut connection, "agent-child", "claim-before-archive", 42)
            .unwrap();
        acknowledge_agent_message_with_projection(
            &mut connection,
            "message-before-archive",
            "claim-before-archive",
            43,
        )
        .unwrap();

        let archived = transition_agent_lifecycle(
            &mut connection,
            "agent-child",
            1,
            AgentLifecycle::Active,
            AgentLifecycle::Archived,
            44,
        )
        .unwrap();
        assert_eq!(archived.revision, 2);
        assert_eq!(archived.lifecycle, AgentLifecycle::Archived);
        assert!(matches!(
            transition_agent_lifecycle(
                &mut connection,
                "agent-child",
                1,
                AgentLifecycle::Active,
                AgentLifecycle::Disabled,
                45,
            ),
            Err(AgentGraphError::RevisionConflict { .. })
        ));
        assert!(enqueue_agent_wake(&mut connection, &wake_input("inactive"), 46).is_err());
    }
}
