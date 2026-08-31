use super::common::{
    conflict, corrupt, immediate, invalid, read_error, revision_to_sql, validate_bounded_text,
    validate_id, validate_identity_task_name, validate_request_id, validate_revision,
    validate_time, validate_trimmed, write_error, MAX_TASK_PATH_BYTES,
};
use super::node_records::{
    query_node, query_node_by_conversation, query_node_by_request, query_node_by_root_conversation,
    query_nodes, reasoning_effort_as_str, NODE_SELECT,
};
use super::wake_records::{decode_wake, read_wake_row, WAKE_SELECT};
use crate::{
    AgentDisplayStatus, AgentDisplayStatusSnapshot, AgentGraphError, AgentLifecycle,
    AgentModelSelectionSnapshot, AgentModelSelectionSource, AgentNodeRecord, AgentTemplateSnapshot,
    AgentWakeStatus, CreateAgentNodeInput, EnsureRootAgentInput, IdempotentCreate, ReasoningEffort,
    AGENT_GRAPH_SCHEMA_VERSION,
};
use rusqlite::{params, Connection, OptionalExtension};

pub fn ensure_root_agent(
    connection: &mut Connection,
    input: &EnsureRootAgentInput,
    created_at: i64,
) -> Result<IdempotentCreate<AgentNodeRecord>, AgentGraphError> {
    let transaction = immediate(connection)?;
    let outcome = ensure_root_agent_in_transaction(&transaction, input, created_at)?;
    transaction.commit().map_err(write_error)?;
    Ok(outcome)
}

/// Creates or resolves a root Agent inside a caller-owned write transaction.
///
/// Conversation forking uses this to make the new Conversation, independent root identity, copied
/// history and fork receipt one crash-atomic database transition. Ordinary callers should keep
/// using [`ensure_root_agent`].
pub(crate) fn ensure_root_agent_in_transaction(
    transaction: &Connection,
    input: &EnsureRootAgentInput,
    created_at: i64,
) -> Result<IdempotentCreate<AgentNodeRecord>, AgentGraphError> {
    validate_id("agent_id", &input.agent_id)?;
    validate_id("conversation_id", &input.conversation_id)?;
    validate_request_id(&input.creation_request_id)?;
    validate_identity_task_name(&input.task_name)?;
    validate_time(created_at)?;

    let project_id = conversation_project(transaction, &input.conversation_id)?;
    if let Some(existing) = query_node_by_root_conversation(transaction, &input.conversation_id)? {
        if existing.agent_id == input.agent_id
            && existing.parent_agent_id.is_none()
            && existing.conversation_id == input.conversation_id
            && existing.creation_request_id == input.creation_request_id
            && existing.task_name == input.task_name
            && existing.task_path == "/root"
            && existing.project_id == project_id
        {
            return Ok(IdempotentCreate::Existing(existing));
        }
        return Err(conflict(
            "root Conversation is already bound to another root identity",
        ));
    }
    if query_node(transaction, &input.agent_id)?.is_some() {
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
                 provider_protocol_revision_snapshot, model_selection_source_snapshot,
                 reasoning_effort_snapshot, lifecycle, revision, created_at, updated_at
             ) VALUES (
                 ?1, ?2, ?1, ?3, NULL, ?3, ?4, ?5, ?6, '/root',
                 NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL,
                 NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 'active', 1, ?7, ?7
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
    let record = query_node(transaction, &input.agent_id)?
        .ok_or_else(|| corrupt("created root Agent could not be read back"))?;
    Ok(IdempotentCreate::Created(record))
}

#[cfg(test)]
pub(crate) fn create_agent_node(
    connection: &mut Connection,
    input: &CreateAgentNodeInput,
    created_at: i64,
) -> Result<IdempotentCreate<AgentNodeRecord>, AgentGraphError> {
    let transaction = immediate(connection)?;
    let outcome = create_agent_node_in_transaction(
        &transaction,
        input,
        AgentModelSelectionSource::Explicit,
        None,
        created_at,
    )?;
    transaction.commit().map_err(write_error)?;
    Ok(outcome)
}

pub(crate) fn create_agent_node_in_transaction(
    transaction: &Connection,
    input: &CreateAgentNodeInput,
    model_selection_source: AgentModelSelectionSource,
    reasoning_effort_snapshot: Option<ReasoningEffort>,
    created_at: i64,
) -> Result<IdempotentCreate<AgentNodeRecord>, AgentGraphError> {
    validate_create_node(input, created_at)?;
    if reasoning_effort_snapshot == Some(ReasoningEffort::ProviderDefault) {
        return Err(invalid(
            "reasoning_effort_snapshot",
            "must be high, max, or absent",
        ));
    }

    if let Some(existing) = query_node_by_request(
        transaction,
        &input.root_agent_id,
        &input.creation_request_id,
    )? {
        if node_matches_create(&existing, input)
            && existing.model_selection_source == Some(model_selection_source)
            && existing.reasoning_effort_snapshot == reasoning_effort_snapshot
        {
            return Ok(IdempotentCreate::Existing(existing));
        }
        return Err(conflict(
            "creation request ID was reused with different node facts",
        ));
    }

    let parent = query_node(transaction, &input.parent_agent_id)?
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
    let project_id = conversation_project(transaction, &input.conversation_id)?;
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
        transaction,
        input.template_snapshot.as_ref(),
        project_id.as_deref(),
    )?;

    if query_node(transaction, &input.agent_id)?.is_some() {
        return Err(conflict("Agent ID is already bound to another node"));
    }
    if query_node_by_conversation(transaction, &input.conversation_id)?.is_some() {
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
                 provider_protocol_revision_snapshot, model_selection_source_snapshot,
                 reasoning_effort_snapshot, lifecycle, revision, created_at, updated_at
             ) VALUES (
                 ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10,
                 ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18,
                 ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, 'active', 1, ?28, ?28
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
                model_selection_source.as_str(),
                reasoning_effort_snapshot.map(reasoning_effort_as_str),
                created_at,
            ],
        )
        .map_err(write_error)?;
    let record = query_node(transaction, &input.agent_id)?
        .ok_or_else(|| corrupt("created child Agent could not be read back"))?;
    Ok(IdempotentCreate::Created(record))
}

/// Inserts one child node while cloning a durable Conversation branch.
///
/// Unlike a new spawn, a fork must preserve the source node's frozen template/model facts even
/// when the current template catalog has changed. The caller supplies fresh tree identities and
/// creates the empty target Conversation first; this function still validates the target parent,
/// project, model and task-path invariants before writing the node.
pub(crate) fn insert_forked_agent_node_in_transaction(
    transaction: &Connection,
    record: &AgentNodeRecord,
) -> Result<AgentNodeRecord, AgentGraphError> {
    validate_id("agent_id", &record.agent_id)?;
    validate_id("root_agent_id", &record.root_agent_id)?;
    validate_id("root_conversation_id", &record.root_conversation_id)?;
    validate_id("conversation_id", &record.conversation_id)?;
    validate_request_id(&record.creation_request_id)?;
    validate_identity_task_name(&record.task_name)?;
    validate_time(record.created_at)?;
    validate_time(record.updated_at)?;
    if record.revision == 0 || record.updated_at < record.created_at {
        return Err(invalid(
            "record",
            "revision and timestamps must describe a valid forked node",
        ));
    }

    let parent_agent_id = record
        .parent_agent_id
        .as_deref()
        .ok_or_else(|| invalid("parent_agent_id", "forked child must have a parent"))?;
    let parent = query_node(transaction, parent_agent_id)?
        .ok_or_else(|| AgentGraphError::AgentNotFound(parent_agent_id.to_string()))?;
    if parent.root_agent_id != record.root_agent_id
        || parent.root_conversation_id != record.root_conversation_id
    {
        return Err(conflict(
            "forked child parent does not belong to the target tree",
        ));
    }
    let expected_path = format!("{}/{}", parent.task_path, record.task_name);
    if record.task_path != expected_path || record.task_name.contains('/') {
        return Err(invalid(
            "task_path",
            "must be exactly one task-name segment below the forked parent",
        ));
    }
    let project_id = conversation_project(transaction, &record.conversation_id)?;
    if project_id != parent.project_id || project_id != record.project_id {
        return Err(conflict(
            "forked child Conversation project does not match the target tree",
        ));
    }
    let conversation_model_id = transaction
        .query_row(
            "SELECT model_id FROM conversations WHERE id = ?1",
            [&record.conversation_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .map_err(read_error)?;
    let model = record
        .model_snapshot
        .as_ref()
        .ok_or_else(|| corrupt("forked child is missing its frozen model snapshot"))?;
    if conversation_model_id.as_deref() != Some(model.model_config_id.as_str()) {
        return Err(conflict(
            "forked child Conversation model does not match its frozen model snapshot",
        ));
    }
    if record.model_selection_source.is_none() {
        return Err(corrupt(
            "forked child is missing model selection provenance",
        ));
    }
    if query_node(transaction, &record.agent_id)?.is_some()
        || query_node_by_conversation(transaction, &record.conversation_id)?.is_some()
    {
        return Err(conflict("forked Agent identity is already in use"));
    }
    if query_node_by_request(
        transaction,
        &record.root_agent_id,
        &record.creation_request_id,
    )?
    .is_some()
    {
        return Err(conflict(
            "forked Agent creation request identity is already in use",
        ));
    }

    let template = record.template_snapshot.as_ref();
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
                 provider_protocol_revision_snapshot, model_selection_source_snapshot,
                 reasoning_effort_snapshot, lifecycle, revision, created_at, updated_at
             ) VALUES (
                 ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10,
                 ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18,
                 ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29, ?30, ?31
             )",
            params![
                &record.agent_id,
                i64::from(AGENT_GRAPH_SCHEMA_VERSION),
                &record.root_agent_id,
                &record.root_conversation_id,
                parent_agent_id,
                &record.conversation_id,
                &record.project_id,
                &record.creation_request_id,
                &record.task_name,
                &record.task_path,
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
                record
                    .model_selection_source
                    .map(AgentModelSelectionSource::as_str),
                record
                    .reasoning_effort_snapshot
                    .map(reasoning_effort_as_str),
                record.lifecycle.as_str(),
                revision_to_sql(record.revision)?,
                record.created_at,
                record.updated_at,
            ],
        )
        .map_err(write_error)?;
    query_node(transaction, &record.agent_id)?
        .ok_or_else(|| corrupt("forked child Agent could not be read back"))
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

pub fn get_agent_display_status(
    connection: &Connection,
    agent_id: &str,
) -> Result<AgentDisplayStatusSnapshot, AgentGraphError> {
    validate_id("agent_id", agent_id)?;
    let agent = query_node(connection, agent_id)?
        .ok_or_else(|| AgentGraphError::AgentNotFound(agent_id.to_string()))?;
    let latest_wake = connection
        .query_row(
            &format!("{WAKE_SELECT} WHERE agent_id = ?1 ORDER BY sequence DESC LIMIT 1"),
            [agent_id],
            read_wake_row,
        )
        .optional()
        .map_err(read_error)?
        .map(decode_wake)
        .transpose()?;
    // A newer deferred follow-up may be satisfied by an already-running Turn's safe sampling
    // boundary. It must not hide that older Turn's still-active state. Keep the newest Wake facts
    // below for cursor/version observation, while deriving display state from active execution
    // first and the latest real Turn outcome second. `satisfied` is a delivery/coalescing outcome,
    // not a completed Agent task.
    let active_wake = connection
        .query_row(
            &format!(
                "{WAKE_SELECT}
                 WHERE agent_id = ?1
                   AND status IN ('queued', 'claimed', 'running', 'waiting_for_approval')
                 ORDER BY CASE status
                     WHEN 'waiting_for_approval' THEN 0
                     WHEN 'running' THEN 1
                     WHEN 'claimed' THEN 2
                     ELSE 3
                 END,
                 sequence
                 LIMIT 1"
            ),
            [agent_id],
            read_wake_row,
        )
        .optional()
        .map_err(read_error)?
        .map(decode_wake)
        .transpose()?;
    // Root Agents do not need a Wake for a human-started Turn. The Conversation trace is the
    // durable execution truth for that path, and pending/approved actions are the durable signal
    // that the active Turn is waiting for approval.
    let active_turn_waiting_for_approval = connection
        .query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM agent_pending_actions AS action
                 WHERE action.run_id = trace.run_id
                   AND action.conversation_id = trace.conversation_id
                   AND action.assistant_message_id = trace.assistant_message_id
                   AND action.status = 'pending'
             )
             FROM conversation_turn_traces AS trace
             WHERE trace.conversation_id = ?1 AND trace.terminal_status = 'in_progress'
             LIMIT 1",
            [&agent.conversation_id],
            |row| row.get::<_, bool>(0),
        )
        .optional()
        .map_err(read_error)?;
    let latest_execution_outcome =
        if active_wake.is_none() && active_turn_waiting_for_approval.is_none() {
            connection
                .query_row(
                    "SELECT status
                 FROM (
                     SELECT status, completed_at AS occurred_at, 0 AS source_priority,
                            sequence AS source_sequence
                     FROM agent_wake_requests
                     WHERE agent_id = ?1
                       AND status IN (
                           'completed', 'failed', 'interrupted', 'cancelled', 'outcome_unknown'
                       )
                     UNION ALL
                     SELECT terminal_status AS status, completed_at AS occurred_at,
                            1 AS source_priority, rowid AS source_sequence
                     FROM conversation_turn_traces
                     WHERE conversation_id = ?2 AND terminal_status != 'in_progress'
                 )
                 ORDER BY occurred_at DESC, source_priority, source_sequence DESC
                 LIMIT 1",
                    params![agent_id, &agent.conversation_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(read_error)?
                .map(|status| match status.as_str() {
                    "completed" => Ok(AgentWakeStatus::Completed),
                    "failed" => Ok(AgentWakeStatus::Failed),
                    "cancelled" => Ok(AgentWakeStatus::Cancelled),
                    "interrupted" => Ok(AgentWakeStatus::Interrupted),
                    "outcome_unknown" => Ok(AgentWakeStatus::OutcomeUnknown),
                    _ => Err(corrupt("latest Agent execution outcome is invalid")),
                })
                .transpose()?
        } else {
            None
        };
    let status = match agent.lifecycle {
        AgentLifecycle::Archived => AgentDisplayStatus::Archived,
        AgentLifecycle::Disabled => AgentDisplayStatus::Disabled,
        AgentLifecycle::Active => {
            let wake_status = active_wake.as_ref().map(|wake| wake.status);
            if wake_status == Some(AgentWakeStatus::WaitingForApproval)
                || active_turn_waiting_for_approval == Some(true)
            {
                AgentDisplayStatus::WaitingApproval
            } else if wake_status == Some(AgentWakeStatus::Running)
                || active_turn_waiting_for_approval == Some(false)
            {
                AgentDisplayStatus::Running
            } else if matches!(
                wake_status,
                Some(AgentWakeStatus::Queued | AgentWakeStatus::Claimed)
            ) {
                AgentDisplayStatus::Queued
            } else {
                match latest_execution_outcome {
                    Some(AgentWakeStatus::Completed) => AgentDisplayStatus::LatestCompleted,
                    Some(AgentWakeStatus::Failed) => AgentDisplayStatus::LatestFailed,
                    Some(AgentWakeStatus::Interrupted | AgentWakeStatus::Cancelled) => {
                        AgentDisplayStatus::LatestInterrupted
                    }
                    Some(AgentWakeStatus::OutcomeUnknown) => {
                        AgentDisplayStatus::LatestOutcomeUnknown
                    }
                    None | Some(AgentWakeStatus::Satisfied) => AgentDisplayStatus::Idle,
                    Some(
                        AgentWakeStatus::Queued
                        | AgentWakeStatus::Claimed
                        | AgentWakeStatus::Running
                        | AgentWakeStatus::WaitingForApproval,
                    ) => {
                        return Err(corrupt(
                            "display outcome query unexpectedly returned an active Wake",
                        ))
                    }
                }
            }
        }
    };
    Ok(AgentDisplayStatusSnapshot {
        agent_id: agent.agent_id,
        status,
        agent_revision: agent.revision,
        latest_wake_id: latest_wake.as_ref().map(|wake| wake.wake_id.clone()),
        latest_wake_sequence: latest_wake.as_ref().map(|wake| wake.sequence),
        latest_wake_status_revision: latest_wake.as_ref().map(|wake| wake.status_revision),
    })
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
        let active_turn = transaction
            .query_row(
                "SELECT EXISTS(
                     SELECT 1 FROM conversation_turn_traces
                     WHERE conversation_id = ?1 AND terminal_status = 'in_progress'
                 )",
                [&current.conversation_id],
                |row| row.get::<_, bool>(0),
            )
            .map_err(read_error)?;
        if active_turn {
            return Err(conflict("Agent has an active Conversation Turn"));
        }
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

pub(super) fn validate_create_node(
    input: &CreateAgentNodeInput,
    created_at: i64,
) -> Result<(), AgentGraphError> {
    validate_id("agent_id", &input.agent_id)?;
    validate_id("root_agent_id", &input.root_agent_id)?;
    validate_id("parent_agent_id", &input.parent_agent_id)?;
    validate_id("conversation_id", &input.conversation_id)?;
    validate_request_id(&input.creation_request_id)?;
    validate_identity_task_name(&input.task_name)?;
    validate_trimmed("task_path", &input.task_path, MAX_TASK_PATH_BYTES)?;
    validate_model_snapshot(&input.model_snapshot)?;
    if let Some(template) = &input.template_snapshot {
        validate_template_fields(template)?;
    }
    validate_time(created_at)
}

pub(super) fn validate_template_snapshot(
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
                 SELECT 1
                 FROM agent_templates AS template
                 INNER JOIN project_agent_template_bindings AS binding
                   ON binding.template_id = template.template_id
                 WHERE template.template_id = ?1 AND binding.project_id = ?2
                   AND template.machine_key = ?3
                   AND template.name = ?4 AND template.description = ?5
                   AND template.instructions = ?6 AND template.revision = ?7
                   AND template.model_config_id = ?8 AND template.enabled = 1
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
            "template snapshot is stale, disabled, unassigned, or unavailable",
        ));
    }
    Ok(())
}

pub(super) fn validate_template_fields(
    template: &AgentTemplateSnapshot,
) -> Result<(), AgentGraphError> {
    validate_id("template_id", &template.template_id)?;
    validate_id("template_project_id", &template.project_id)?;
    validate_trimmed("template_machine_key", &template.machine_key, 64)?;
    validate_trimmed("template_name", &template.name, 256)?;
    validate_bounded_text("template_description", &template.description, 0, 4 * 1024)?;
    validate_trimmed("template_instructions", &template.instructions, 64 * 1024)?;
    validate_trimmed("template_model_config_id", &template.model_config_id, 512)?;
    validate_revision(template.template_revision)
}

pub(super) fn validate_model_snapshot(
    model: &AgentModelSelectionSnapshot,
) -> Result<(), AgentGraphError> {
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

pub(super) fn conversation_project(
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

pub(super) fn ensure_active_agent(
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

pub(super) fn ensure_active_pair(
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

pub(super) fn is_strict_descendant(
    connection: &Connection,
    ancestor_agent_id: &str,
    descendant_agent_id: &str,
) -> Result<bool, AgentGraphError> {
    connection
        .query_row(
            "WITH RECURSIVE ancestors(agent_id, parent_agent_id) AS (
                 SELECT agent_id, parent_agent_id
                 FROM agent_nodes WHERE agent_id = ?1
                 UNION ALL
                 SELECT parent.agent_id, parent.parent_agent_id
                 FROM agent_nodes AS parent
                 JOIN ancestors AS child ON parent.agent_id = child.parent_agent_id
             )
             SELECT EXISTS(
                 SELECT 1 FROM ancestors
                 WHERE agent_id = ?2 AND agent_id != ?1
             )",
            params![descendant_agent_id, ancestor_agent_id],
            |row| row.get(0),
        )
        .map_err(read_error)
}

pub(super) fn node_matches_create(record: &AgentNodeRecord, input: &CreateAgentNodeInput) -> bool {
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
