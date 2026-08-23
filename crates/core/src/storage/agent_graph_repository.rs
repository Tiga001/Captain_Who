//! Transactional persistence for the deliberately small Agent parent-child tree.
//!
//! SQLite is the coordination source of truth. Runtime notifications may observe these records,
//! but no in-memory queue is allowed to substitute for them.

use crate::storage::{
    chat_repository, conversation_model_context_repository, conversation_trace_repository,
};
use crate::{
    AcknowledgeAgentTaskAndWakeInput, AgentBuiltinExecutionPermission, AgentCollaborationIdentity,
    AgentCommandPermission, AgentCommandSafetyPolicy, AgentDisplayStatus,
    AgentDisplayStatusSnapshot, AgentEffectivePermissionSnapshot, AgentGraphError, AgentLifecycle,
    AgentMailboxDeliveryStatus, AgentMailboxKind, AgentMailboxMessageRecord, AgentMessageDispatch,
    AgentModelSelectionSnapshot, AgentModelSelectionSource, AgentNodeRecord, AgentPatchPermission,
    AgentPermissions, AgentReadPermission, AgentResultArtifactKind, AgentResultArtifactReference,
    AgentTemplateSnapshot, AgentTurnResultEnvelope, AgentTurnResultSettlement,
    AgentWakeRecoveryAction, AgentWakeRecoveryBatch, AgentWakeRequestRecord, AgentWakeStatus,
    AgentWritePermission, ChildAgentSpawnRecord, ConversationMessageOrigin, CreateAgentNodeInput,
    EnqueueAgentMessageInput, EnqueueAgentWakeInput, EnsureRootAgentInput,
    FinishAgentTurnResultInput, FinishAgentWakeWithResultInput, IdempotentCreate,
    InterruptAgentExecutionOutcome, ReasoningEffort, SendAgentMessageRequest,
    TrustedActiveChildWakeBundle, AGENT_EFFECTIVE_PERMISSION_SNAPSHOT_SCHEMA_VERSION,
    AGENT_GRAPH_SCHEMA_VERSION, AGENT_RESULT_ENVELOPE_SCHEMA_VERSION,
};
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use sha2::{Digest, Sha256};

const MAX_ID_BYTES: usize = 128;
const MAX_REQUEST_ID_BYTES: usize = 256;
const MAX_TASK_NAME_BYTES: usize = 256;
const MAX_TASK_PATH_BYTES: usize = 2_048;
const MAX_MESSAGE_BYTES: usize = 1_048_576;
const MAX_UNBOUND_MAILBOX_MESSAGES_PER_RECIPIENT: u64 = 1_024;
const MAX_UNBOUND_MAILBOX_BYTES_PER_RECIPIENT: u64 = 16 * 1_024 * 1_024;
const MAX_UNBOUND_ORDINARY_MAILBOX_MESSAGES_PER_RECIPIENT: u64 = 960;
const MAX_UNBOUND_ORDINARY_MAILBOX_BYTES_PER_RECIPIENT: u64 = 15 * 1_024 * 1_024;
const MAX_RESULT_SUMMARY_BYTES: usize = crate::AGENT_RESULT_SUMMARY_MAX_BYTES;
const MAX_TERMINAL_ERROR_BYTES: usize = crate::AGENT_RESULT_TERMINAL_ERROR_MAX_BYTES;
const MAX_RESULT_ARTIFACTS: usize = 256;
const MAX_PROJECT_BATCH: usize = 1_024;
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
           provider_protocol_revision_snapshot, model_selection_source_snapshot,
           reasoning_effort_snapshot, lifecycle, revision, created_at, updated_at
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
           status_revision, claim_token, lease_expires_at, result_message_id, terminal_error,
           run_id, assistant_message_id, created_at, claimed_at, started_at, completed_at
    FROM agent_wake_requests";

const EFFECTIVE_PERMISSION_SELECT: &str = "
    SELECT snapshot.agent_id, snapshot.schema_version, snapshot.root_agent_id,
           snapshot.conversation_id, snapshot.source_run_id,
           snapshot.source_assistant_message_id, snapshot.read_permission,
           snapshot.write_permission, snapshot.command_permission,
           snapshot.command_safety_policy, snapshot.patch_permission,
           snapshot.builtin_execution_permission, snapshot.revision, snapshot.created_at,
           snapshot.updated_at
    FROM agent_effective_permission_snapshots AS snapshot";

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

pub fn get_agent_effective_permission_snapshot(
    connection: &Connection,
    agent_id: &str,
) -> Result<Option<AgentEffectivePermissionSnapshot>, AgentGraphError> {
    validate_id("agent_id", agent_id)?;
    query_effective_permission_snapshot(connection, agent_id)
}

/// Persists the Host-authenticated permissions of an exact active Turn. This covers the lazy-root
/// case where the root node is materialized only while constructing collaboration Host services.
#[allow(clippy::too_many_arguments)]
pub fn record_agent_effective_permissions_for_active_turn(
    connection: &mut Connection,
    agent_id: &str,
    conversation_id: &str,
    run_id: &str,
    assistant_message_id: &str,
    permissions: AgentPermissions,
    updated_at: i64,
) -> Result<AgentEffectivePermissionSnapshot, AgentGraphError> {
    let transaction = immediate(connection)?;
    let snapshot = record_agent_effective_permissions_in_transaction(
        &transaction,
        agent_id,
        conversation_id,
        run_id,
        assistant_message_id,
        permissions,
        updated_at,
    )?;
    transaction.commit().map_err(write_error)?;
    Ok(snapshot)
}

/// Resolves a child permission set from the direct parent and every durable ancestor. The direct
/// parent is the inheritance baseline; older ancestor snapshots are dynamic ceilings which stop a
/// stale intermediate Agent from retaining authority after the root has tightened it.
pub(crate) fn inherit_agent_permissions_in_transaction(
    transaction: &Connection,
    child_agent_id: &str,
) -> Result<AgentPermissions, AgentGraphError> {
    validate_id("child_agent_id", child_agent_id)?;
    let child = ensure_active_agent(transaction, child_agent_id)?;
    let direct_parent_id = child
        .parent_agent_id
        .as_deref()
        .ok_or_else(|| conflict("root Agent cannot inherit child Wake permissions"))?;
    let mut statement = transaction
        .prepare(&format!(
            "WITH RECURSIVE ancestors(agent_id, parent_agent_id, depth) AS (
                 SELECT agent_id, parent_agent_id, 0
                 FROM agent_nodes WHERE agent_id = ?1
                 UNION ALL
                 SELECT parent.agent_id, parent.parent_agent_id, child.depth + 1
                 FROM agent_nodes AS parent
                 JOIN ancestors AS child ON parent.agent_id = child.parent_agent_id
             )
             {EFFECTIVE_PERMISSION_SELECT}
             JOIN ancestors ON ancestors.agent_id = snapshot.agent_id
             WHERE ancestors.depth > 0
             ORDER BY ancestors.depth"
        ))
        .map_err(read_error)?;
    let snapshots = statement
        .query_map([child_agent_id], read_effective_permission_row)
        .map_err(read_error)?
        .map(|row| {
            row.map_err(read_error)
                .and_then(decode_effective_permission_snapshot)
        })
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);

    let ancestor_count = transaction
        .query_row(
            "WITH RECURSIVE ancestors(agent_id, parent_agent_id) AS (
                 SELECT agent_id, parent_agent_id
                 FROM agent_nodes WHERE agent_id = ?1
                 UNION ALL
                 SELECT parent.agent_id, parent.parent_agent_id
                 FROM agent_nodes AS parent
                 JOIN ancestors AS child ON parent.agent_id = child.parent_agent_id
             )
             SELECT COUNT(*) - 1 FROM ancestors",
            [child_agent_id],
            |row| row.get::<_, i64>(0),
        )
        .map_err(read_error)?;
    if ancestor_count <= 0
        || snapshots.len() != usize::try_from(ancestor_count).unwrap_or(usize::MAX)
    {
        return Err(conflict(
            "trusted child permission inheritance is missing a durable ancestor snapshot",
        ));
    }
    let direct_parent = snapshots
        .first()
        .filter(|snapshot| snapshot.agent_id == direct_parent_id)
        .ok_or_else(|| corrupt("direct parent permission snapshot is missing or misordered"))?;
    if snapshots.iter().any(|snapshot| {
        snapshot.root_agent_id != child.root_agent_id || snapshot.conversation_id.is_empty()
    }) {
        return Err(corrupt(
            "ancestor permission snapshot crosses the child Agent root tree",
        ));
    }
    Ok(snapshots
        .iter()
        .skip(1)
        .fold(direct_parent.permissions, |effective, ancestor| {
            effective.meet(ancestor.permissions)
        }))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn record_agent_effective_permissions_in_transaction(
    transaction: &Connection,
    agent_id: &str,
    conversation_id: &str,
    run_id: &str,
    assistant_message_id: &str,
    permissions: AgentPermissions,
    updated_at: i64,
) -> Result<AgentEffectivePermissionSnapshot, AgentGraphError> {
    for (field, value) in [
        ("agent_id", agent_id),
        ("conversation_id", conversation_id),
        ("run_id", run_id),
        ("assistant_message_id", assistant_message_id),
    ] {
        validate_id(field, value)?;
    }
    validate_time(updated_at)?;
    let node = ensure_active_agent(transaction, agent_id)?;
    if node.conversation_id != conversation_id {
        return Err(conflict(
            "Agent effective permissions do not match the bound Conversation",
        ));
    }
    let exact_active_turn = transaction
        .query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM conversation_turn_traces
                 WHERE conversation_id = ?1 AND run_id = ?2
                   AND assistant_message_id = ?3 AND terminal_status = 'in_progress'
             )",
            params![conversation_id, run_id, assistant_message_id],
            |row| row.get::<_, bool>(0),
        )
        .map_err(read_error)?;
    if !exact_active_turn {
        return Err(conflict(
            "Agent effective permissions require the exact active Turn identity",
        ));
    }
    let existing = query_effective_permission_snapshot(transaction, agent_id)?;
    let changed = existing.as_ref().is_none_or(|snapshot| {
        snapshot.source_run_id != run_id
            || snapshot.source_assistant_message_id != assistant_message_id
            || snapshot.permissions != permissions
    });
    if changed {
        let created_at = existing
            .as_ref()
            .map_or(updated_at, |snapshot| snapshot.created_at);
        let next_updated_at = existing.as_ref().map_or(updated_at, |snapshot| {
            updated_at.max(snapshot.updated_at.saturating_add(1))
        });
        transaction
            .execute(
                "INSERT INTO agent_effective_permission_snapshots (
                     agent_id, schema_version, root_agent_id, conversation_id, source_run_id,
                     source_assistant_message_id, read_permission, write_permission,
                     command_permission, command_safety_policy, patch_permission,
                     builtin_execution_permission, revision, created_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 1, ?13, ?14)
                 ON CONFLICT(agent_id) DO UPDATE SET
                     source_run_id = excluded.source_run_id,
                     source_assistant_message_id = excluded.source_assistant_message_id,
                     read_permission = excluded.read_permission,
                     write_permission = excluded.write_permission,
                     command_permission = excluded.command_permission,
                     command_safety_policy = excluded.command_safety_policy,
                     patch_permission = excluded.patch_permission,
                     builtin_execution_permission = excluded.builtin_execution_permission,
                     revision = agent_effective_permission_snapshots.revision + 1,
                     updated_at = excluded.updated_at",
                params![
                    agent_id,
                    i64::from(AGENT_EFFECTIVE_PERMISSION_SNAPSHOT_SCHEMA_VERSION),
                    &node.root_agent_id,
                    conversation_id,
                    run_id,
                    assistant_message_id,
                    read_permission_as_str(permissions.read),
                    write_permission_as_str(permissions.write),
                    command_permission_as_str(permissions.command),
                    command_safety_as_str(permissions.command_safety),
                    patch_permission_as_str(permissions.patch),
                    builtin_execution_permission_as_str(permissions.builtin_execution),
                    created_at,
                    next_updated_at,
                ],
            )
            .map_err(write_error)?;
    }
    query_effective_permission_snapshot(transaction, agent_id)?
        .ok_or_else(|| corrupt("Agent effective permission snapshot disappeared after upsert"))
}

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

fn resolve_child_bundle(
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

/// Application-level send: one immutable same-tree Mailbox fact, no projection and no Wake.
pub fn send_agent_message(
    connection: &mut Connection,
    input: &SendAgentMessageRequest,
    created_at: i64,
) -> Result<AgentMessageDispatch, AgentGraphError> {
    let transaction = immediate(connection)?;
    let message = enqueue_application_message_in_transaction(
        &transaction,
        input,
        AgentMailboxKind::Message,
        created_at,
    )?;
    transaction.commit().map_err(write_error)?;
    Ok(AgentMessageDispatch {
        message,
        deferred_wake: None,
    })
}

/// Application-level follow-up: persists the message and its deferred Wake in one transaction.
/// Delivery remains queued until Dispatcher admission or a running Turn's safe sampling boundary.
pub fn follow_up_agent(
    connection: &mut Connection,
    input: &SendAgentMessageRequest,
    created_at: i64,
) -> Result<AgentMessageDispatch, AgentGraphError> {
    let transaction = immediate(connection)?;
    let sender = ensure_active_agent(&transaction, &input.sender_agent_id)?;
    let target = ensure_active_agent(&transaction, &input.recipient_agent_id)?;
    if !is_strict_descendant(&transaction, &sender.agent_id, &target.agent_id)? {
        return Err(conflict(
            "follow-up authority is limited to a caller's strict descendants",
        ));
    }
    let message = enqueue_application_message_in_transaction(
        &transaction,
        input,
        AgentMailboxKind::Followup,
        created_at,
    )?;
    let wake_input = EnqueueAgentWakeInput {
        wake_id: stable_fact_id("wake", &[&input.sender_agent_id, &input.request_id]),
        root_agent_id: sender.root_agent_id,
        agent_id: input.recipient_agent_id.clone(),
        requester_agent_id: input.sender_agent_id.clone(),
        request_id: stable_fact_id(
            "followup-request",
            &[&input.sender_agent_id, &input.request_id],
        ),
        source_agent_message_id: Some(message.message_id.clone()),
    };
    let deferred_wake = enqueue_wake_in_transaction(&transaction, &wake_input, created_at)?
        .record()
        .clone();
    transaction.commit().map_err(write_error)?;
    Ok(AgentMessageDispatch {
        message,
        deferred_wake: Some(deferred_wake),
    })
}

fn enqueue_application_message_in_transaction(
    connection: &Connection,
    input: &SendAgentMessageRequest,
    kind: AgentMailboxKind,
    created_at: i64,
) -> Result<AgentMailboxMessageRecord, AgentGraphError> {
    validate_id("sender_agent_id", &input.sender_agent_id)?;
    validate_id("recipient_agent_id", &input.recipient_agent_id)?;
    validate_request_id(&input.request_id)?;
    validate_trimmed("content", &input.content, MAX_MESSAGE_BYTES)?;
    let sender = ensure_active_agent(connection, &input.sender_agent_id)?;
    ensure_active_pair(
        connection,
        &sender.root_agent_id,
        &input.sender_agent_id,
        &input.recipient_agent_id,
    )?;
    let message = EnqueueAgentMessageInput {
        message_id: stable_fact_id("mailbox", &[&input.sender_agent_id, &input.request_id]),
        root_agent_id: sender.root_agent_id,
        sender_agent_id: input.sender_agent_id.clone(),
        recipient_agent_id: input.recipient_agent_id.clone(),
        request_id: input.request_id.clone(),
        kind,
        content: input.content.clone(),
        projection_message_id: stable_fact_id(
            "message",
            &[&input.sender_agent_id, &input.request_id],
        ),
    };
    Ok(
        enqueue_message_in_transaction(connection, &message, created_at)?
            .record()
            .clone(),
    )
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
                .is_some_and(|deadline| claimed_at >= deadline)
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
    if renewed_at >= current_deadline {
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
    transaction: &Connection,
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
        .is_none_or(|deadline| acknowledged_at >= deadline)
    {
        return Err(conflict("Mailbox claim lease has expired"));
    }
    let recipient = query_node(transaction, &current.recipient_agent_id)?
        .ok_or_else(|| corrupt("Mailbox recipient Agent is missing"))?;
    let active_assistant_position = transaction
        .query_row(
            "SELECT message.position
             FROM conversation_turn_traces AS trace
             JOIN messages AS message ON message.id = trace.assistant_message_id
             WHERE trace.conversation_id = ?1 AND trace.terminal_status = 'in_progress'
             LIMIT 1",
            [&recipient.conversation_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(read_error)?;
    let append_position = transaction
        .query_row(
            "SELECT COALESCE(MAX(position), -1) + 1 FROM messages WHERE conversation_id = ?1",
            [&recipient.conversation_id],
            |row| row.get::<_, i64>(0),
        )
        .map_err(read_error)?;
    let position = active_assistant_position.unwrap_or(append_position);
    if let Some(active_position) = active_assistant_position {
        // Runtime-visible collaboration input belongs immediately before the pending assistant.
        // Projected mailbox rows already inserted for this Turn remain before it and are never
        // rewritten; only the pending assistant and any later ordinary rows move right.
        transaction
            .execute(
                "UPDATE messages SET position = position + 1
                 WHERE conversation_id = ?1 AND position >= ?2",
                params![&recipient.conversation_id, active_position],
            )
            .map_err(write_error)?;
    }
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

/// Projects queued Mailbox facts at a caller-owned transaction boundary. This is shared by the
/// Dispatcher admission path and the model-batch receipt path so projection ordering is defined
/// once. A claimed-but-unexpired row is never stolen; an expired claim is recoverable.
pub(crate) fn project_pending_agent_messages_in_transaction(
    connection: &Connection,
    recipient_agent_id: &str,
    claim_token_prefix: &str,
    projected_at: i64,
    maximum: usize,
) -> Result<Vec<AgentMailboxMessageRecord>, AgentGraphError> {
    validate_id("recipient_agent_id", recipient_agent_id)?;
    validate_id("claim_token_prefix", claim_token_prefix)?;
    validate_time(projected_at)?;
    if maximum == 0 || maximum > MAX_PROJECT_BATCH {
        return Err(invalid(
            "maximum",
            format!("must be between 1 and {MAX_PROJECT_BATCH}"),
        ));
    }
    ensure_active_agent(connection, recipient_agent_id)?;
    project_queued_agent_messages_through_sequence(
        connection,
        recipient_agent_id,
        claim_token_prefix,
        projected_at,
        None,
        maximum,
    )
}

/// Makes the exact source of a queued Wake model/history-visible before the same outer
/// transaction claims that Wake. FIFO is preserved by projecting every earlier queued item for
/// the recipient first. Initial tasks already projected by ChildAgentFactory are idempotent.
pub(crate) fn project_agent_wake_source_in_transaction(
    connection: &Connection,
    wake_id: &str,
    claim_token_prefix: &str,
    projected_at: i64,
) -> Result<Vec<AgentMailboxMessageRecord>, AgentGraphError> {
    validate_id("wake_id", wake_id)?;
    let wake = query_wake(connection, wake_id)?
        .ok_or_else(|| AgentGraphError::WakeNotFound(wake_id.to_string()))?;
    if wake.status != AgentWakeStatus::Queued {
        return Err(conflict(
            "only a queued Wake source can be projected for dispatch",
        ));
    }
    let Some(source_id) = wake.source_agent_message_id.as_deref() else {
        return Ok(Vec::new());
    };
    let source = query_message(connection, source_id)?
        .ok_or_else(|| corrupt("queued Wake source Mailbox message is missing"))?;
    if source.recipient_agent_id != wake.agent_id
        || source.sender_agent_id != wake.requester_agent_id
        || source.root_agent_id != wake.root_agent_id
    {
        return Err(corrupt(
            "queued Wake source identity does not match its Mailbox fact",
        ));
    }
    validate_wake_source_authority(connection, &source, &wake.agent_id)?;
    if source.delivery_status == AgentMailboxDeliveryStatus::Acknowledged {
        ensure_projection_exists(connection, &source)?;
        return Ok(Vec::new());
    }
    let projected = project_queued_agent_messages_through_sequence(
        connection,
        &wake.agent_id,
        claim_token_prefix,
        projected_at,
        Some(source.sequence),
        usize::MAX,
    )?;
    let delivered = query_message(connection, source_id)?
        .ok_or_else(|| corrupt("projected Wake source disappeared"))?;
    if delivered.delivery_status != AgentMailboxDeliveryStatus::Acknowledged {
        return Err(conflict(
            "Wake source could not be projected without overtaking an earlier Mailbox item",
        ));
    }
    Ok(projected)
}

/// Marks a deferred Wake satisfied when its exact source message was bound to an already-running
/// model batch. The caller owns the surrounding receipt transaction, making duplicate delivery
/// and a second Turn mutually exclusive across crashes.
pub(crate) fn satisfy_agent_wake_by_source_message_in_transaction(
    connection: &Connection,
    source_message_id: &str,
    satisfied_at: i64,
) -> Result<Option<AgentWakeRequestRecord>, AgentGraphError> {
    validate_id("source_message_id", source_message_id)?;
    validate_time(satisfied_at)?;
    let wake = connection
        .query_row(
            &format!("{WAKE_SELECT} WHERE source_agent_message_id = ?1"),
            [source_message_id],
            read_wake_row,
        )
        .optional()
        .map_err(read_error)?
        .map(decode_wake)
        .transpose()?;
    let Some(wake) = wake else {
        return Ok(None);
    };
    if wake.status == AgentWakeStatus::Satisfied {
        return Ok(Some(wake));
    }
    if wake.status != AgentWakeStatus::Queued {
        // Claimed/running means the dispatcher won the race; another terminal state is immutable.
        return Ok(None);
    }
    connection
        .execute(
            "UPDATE agent_wake_requests
             SET status = 'satisfied', status_revision = status_revision + 1,
                 completed_at = ?1
             WHERE wake_id = ?2 AND status = 'queued'",
            params![satisfied_at, &wake.wake_id],
        )
        .map_err(write_error)?;
    query_wake(connection, &wake.wake_id)
}

fn project_queued_agent_messages_through_sequence(
    connection: &Connection,
    recipient_agent_id: &str,
    claim_token_prefix: &str,
    projected_at: i64,
    through_sequence: Option<u64>,
    maximum: usize,
) -> Result<Vec<AgentMailboxMessageRecord>, AgentGraphError> {
    let through_sequence = through_sequence
        .map(|value| i64::try_from(value).map_err(|_| corrupt("Mailbox sequence is too large")))
        .transpose()?;
    let live_claim = connection
        .query_row(
            "SELECT message_id, lease_expires_at
             FROM agent_mailbox_messages
             WHERE recipient_agent_id = ?1 AND delivery_status = 'claimed'
             ORDER BY sequence LIMIT 1",
            [recipient_agent_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()
        .map_err(read_error)?;
    if let Some((message_id, deadline)) = live_claim {
        if projected_at < deadline {
            return Err(conflict(format!(
                "Mailbox message `{message_id}` is held by another live claim"
            )));
        }
        connection
            .execute(
                "UPDATE agent_mailbox_messages
                 SET delivery_status = 'queued', claim_token = NULL, lease_expires_at = NULL,
                     claimed_at = NULL
                 WHERE message_id = ?1 AND delivery_status = 'claimed'
                   AND lease_expires_at <= ?2",
                params![message_id, projected_at],
            )
            .map_err(write_error)?;
    }

    let mut projected = Vec::new();
    while projected.len() < maximum {
        let next = connection
            .query_row(
                "SELECT message_id, sequence
                 FROM agent_mailbox_messages
                 WHERE recipient_agent_id = ?1 AND delivery_status = 'queued'
                   AND (?2 IS NULL OR sequence <= ?2)
                 ORDER BY sequence LIMIT 1",
                params![recipient_agent_id, through_sequence],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
            )
            .optional()
            .map_err(read_error)?;
        let Some((message_id, _sequence)) = next else {
            break;
        };
        let claim_token = stable_fact_id(
            "mailbox-claim",
            &[claim_token_prefix, recipient_agent_id, message_id.as_str()],
        );
        let deadline = projected_at
            .checked_add(WAKE_LEASE_DURATION_MS)
            .ok_or_else(|| invalid("projected_at", "cannot compute Mailbox lease"))?;
        let changed = connection
            .execute(
                "UPDATE agent_mailbox_messages
                 SET delivery_status = 'claimed', claim_token = ?1, lease_expires_at = ?2,
                     claimed_at = ?3
                 WHERE message_id = ?4 AND delivery_status = 'queued'",
                params![claim_token, deadline, projected_at, message_id],
            )
            .map_err(write_error)?;
        if changed != 1 {
            return Err(conflict("Mailbox FIFO projection lost its claim race"));
        }
        projected.push(acknowledge_message_in_transaction(
            connection,
            &message_id,
            &claim_token,
            projected_at,
        )?);
    }
    Ok(projected)
}

fn ensure_projection_exists(
    connection: &Connection,
    message: &AgentMailboxMessageRecord,
) -> Result<(), AgentGraphError> {
    let exists = connection
        .query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM messages
                 WHERE id = ?1 AND source_agent_message_id = ?2
             )",
            params![&message.projection_message_id, &message.message_id],
            |row| row.get::<_, bool>(0),
        )
        .map_err(read_error)?;
    if exists {
        Ok(())
    } else {
        Err(corrupt(
            "acknowledged Mailbox message is missing its Conversation projection",
        ))
    }
}

/// Persists the first parent task, its unique model/history projection and the initial queued
/// Wake as one part of a caller-owned spawn transaction.
pub(crate) fn create_initial_agent_task_and_wake_in_transaction(
    transaction: &Connection,
    message: &EnqueueAgentMessageInput,
    wake: &EnqueueAgentWakeInput,
    claim_token: &str,
    created_at: i64,
) -> Result<(AgentMailboxMessageRecord, AgentWakeRequestRecord), AgentGraphError> {
    validate_message_input(message, created_at)?;
    validate_wake_input(wake, created_at)?;
    validate_id("claim_token", claim_token)?;
    if message.kind != AgentMailboxKind::Task {
        return Err(invalid(
            "message.kind",
            "initial child assignment must be a task",
        ));
    }
    if wake.source_agent_message_id.as_deref() != Some(message.message_id.as_str())
        || wake.root_agent_id != message.root_agent_id
        || wake.requester_agent_id != message.sender_agent_id
        || wake.agent_id != message.recipient_agent_id
    {
        return Err(invalid(
            "wake",
            "initial Wake must reference the exact parent-to-child task",
        ));
    }
    let inserted = enqueue_message_in_transaction(transaction, message, created_at)?;
    if matches!(inserted, IdempotentCreate::Existing(_)) {
        return Err(conflict(
            "initial task already exists outside the completed spawn bundle",
        ));
    }
    let lease_expires_at = created_at
        .checked_add(WAKE_LEASE_DURATION_MS)
        .ok_or_else(|| invalid("created_at", "cannot compute initial task lease"))?;
    transaction
        .execute(
            "UPDATE agent_mailbox_messages
             SET delivery_status = 'claimed', claim_token = ?1, lease_expires_at = ?2,
                 claimed_at = ?3
             WHERE message_id = ?4 AND delivery_status = 'queued'",
            params![
                claim_token,
                lease_expires_at,
                created_at,
                &message.message_id
            ],
        )
        .map_err(write_error)?;
    let acknowledged = acknowledge_message_in_transaction(
        transaction,
        &message.message_id,
        claim_token,
        created_at,
    )?;
    let wake = enqueue_wake_in_transaction(transaction, wake, created_at)?;
    if matches!(wake, IdempotentCreate::Existing(_)) {
        return Err(conflict(
            "initial Wake already exists outside the completed spawn bundle",
        ));
    }
    Ok((acknowledged, wake.record().clone()))
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
            "SELECT role, input_origin_kind, input_origin_agent_id, source_agent_message_id,
                    snapshot_source_conversation_id, snapshot_source_message_id,
                    snapshot_original_origin_kind, snapshot_original_agent_id,
                    snapshot_original_mailbox_message_id
             FROM messages WHERE conversation_id = ?1 AND id = ?2",
            params![conversation_id, message_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, Option<String>>(8)?,
                ))
            },
        )
        .optional()
        .map_err(read_error)?
        .ok_or_else(|| AgentGraphError::MessageNotFound(message_id.to_string()))?;
    decode_conversation_message_origin(stored)
}

/// Loads every user/input origin for one Conversation in one ordered SQLite query. Callers that
/// need a conversation snapshot should invoke this through the Storage read transaction API so
/// message content and actor provenance share the same read cut.
pub fn conversation_message_origins(
    connection: &Connection,
    conversation_id: &str,
) -> Result<Vec<(String, ConversationMessageOrigin)>, AgentGraphError> {
    validate_id("conversation_id", conversation_id)?;
    let mut statement = connection
        .prepare(
            "SELECT id, role, input_origin_kind, input_origin_agent_id,
                    source_agent_message_id, snapshot_source_conversation_id,
                    snapshot_source_message_id, snapshot_original_origin_kind,
                    snapshot_original_agent_id, snapshot_original_mailbox_message_id
             FROM messages
             WHERE conversation_id = ?1 AND role = 'user'
             ORDER BY position, id",
        )
        .map_err(read_error)?;
    let rows = statement
        .query_map([conversation_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                (
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, Option<String>>(9)?,
                ),
            ))
        })
        .map_err(read_error)?;
    let mut origins = Vec::new();
    for row in rows {
        let (message_id, stored) = row.map_err(read_error)?;
        origins.push((message_id, decode_conversation_message_origin(stored)?));
    }
    Ok(origins)
}

type StoredConversationMessageOrigin = (
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
);

fn decode_conversation_message_origin(
    stored: StoredConversationMessageOrigin,
) -> Result<ConversationMessageOrigin, AgentGraphError> {
    match stored {
        (role, _, _, _, _, _, _, _, _) if role != "user" => Err(AgentGraphError::InvalidInput {
            field: "message_id",
            reason: "message origin is defined only for user/input messages".to_string(),
        }),
        (_, None, None, None, None, None, None, None, None) => Ok(ConversationMessageOrigin::Human),
        (_, Some(kind), None, None, None, None, None, None, None) if kind == "human" => {
            Ok(ConversationMessageOrigin::Human)
        }
        (
            _,
            Some(kind),
            Some(sender_agent_id),
            Some(source_agent_message_id),
            None,
            None,
            None,
            None,
            None,
        ) if kind == "agent" => Ok(ConversationMessageOrigin::Agent {
            sender_agent_id,
            source_agent_message_id,
        }),
        (
            _,
            Some(kind),
            None,
            None,
            Some(source_conversation_id),
            Some(source_message_id),
            Some(original_kind),
            original_agent_id,
            original_mailbox_message_id,
        ) if kind == "snapshot" => {
            let original = match (
                original_kind.as_str(),
                original_agent_id,
                original_mailbox_message_id,
            ) {
                ("human", None, None) => ConversationMessageOrigin::Human,
                ("agent", Some(sender_agent_id), Some(source_agent_message_id)) => {
                    ConversationMessageOrigin::Agent {
                        sender_agent_id,
                        source_agent_message_id,
                    }
                }
                _ => {
                    return Err(corrupt(
                        "Historical snapshot origin columns are inconsistent",
                    ))
                }
            };
            Ok(ConversationMessageOrigin::HistoricalSnapshot {
                source_conversation_id,
                source_message_id,
                original: Box::new(original),
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
    transaction: &Connection,
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
        {
            return Err(conflict(
                "Wake source message does not belong to this request",
            ));
        }
        validate_wake_source_authority(transaction, &source, input.agent_id.as_str())?;
    }
    transaction
        .execute(
            "INSERT INTO agent_wake_requests (
                 wake_id, schema_version, root_agent_id, agent_id, requester_agent_id,
                 request_id, source_agent_message_id, status, status_revision, claim_token,
                 lease_expires_at, result_message_id, terminal_error, run_id,
                 assistant_message_id, created_at, claimed_at, started_at, completed_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'queued', 1, NULL, NULL, NULL,
                       NULL, NULL, NULL, ?8, NULL, NULL, NULL)",
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
                     status = 'queued', status_revision = status_revision + 1
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
             SET status = 'claimed', status_revision = status_revision + 1,
                 claim_token = ?1, lease_expires_at = ?2, claimed_at = ?3
             WHERE wake_id = ?4 AND status = 'queued'",
            params![claim_token, lease_expires_at, claimed_at, &wake_id],
        )
        .map_err(write_error)?;
    let wake = query_wake(&transaction, &wake_id)?
        .ok_or_else(|| corrupt("claimed Wake could not be read back"))?;
    transaction.commit().map_err(write_error)?;
    Ok(Some(wake))
}

/// Claims the globally oldest Wake whose Agent has neither another active Wake nor an active
/// Conversation Turn. The Wake source projection and claim share this `BEGIN IMMEDIATE`
/// transaction, closing the projection-without-execution crash window for follow-ups/results.
pub fn claim_next_dispatchable_agent_wake(
    connection: &mut Connection,
    claim_token: &str,
    claimed_at: i64,
) -> Result<Option<AgentWakeRequestRecord>, AgentGraphError> {
    validate_id("claim_token", claim_token)?;
    validate_time(claimed_at)?;
    let transaction = immediate(connection)?;
    if let Some(existing) = query_wake_by_claim_token(&transaction, claim_token)? {
        transaction.commit().map_err(write_error)?;
        return Ok(Some(existing));
    }
    let next_id = transaction
        .query_row(
            "SELECT wake.wake_id
             FROM agent_wake_requests AS wake
             JOIN agent_nodes AS agent ON agent.agent_id = wake.agent_id
             WHERE wake.status = 'queued'
               AND agent.lifecycle = 'active'
               AND NOT EXISTS (
                   SELECT 1 FROM agent_wake_requests AS active
                   WHERE active.agent_id = wake.agent_id
                     AND active.status IN ('claimed', 'running', 'waiting_for_approval')
               )
               AND NOT EXISTS (
                   SELECT 1 FROM conversation_turn_traces AS trace
                   WHERE trace.conversation_id = agent.conversation_id
                     AND trace.terminal_status = 'in_progress'
               )
               AND NOT EXISTS (
                   SELECT 1 FROM agent_mailbox_messages AS mailbox
                   WHERE mailbox.recipient_agent_id = wake.agent_id
                     AND mailbox.delivery_status = 'claimed'
                     AND mailbox.lease_expires_at > ?1
               )
             ORDER BY wake.sequence, wake.wake_id
             LIMIT 1",
            [claimed_at],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(read_error)?;
    let Some(wake_id) = next_id else {
        transaction.commit().map_err(write_error)?;
        return Ok(None);
    };
    let projection_prefix = stable_fact_id("wake-dispatch-projection", &[claim_token, &wake_id]);
    project_agent_wake_source_in_transaction(
        &transaction,
        &wake_id,
        &projection_prefix,
        claimed_at,
    )?;
    let lease_expires_at = claimed_at
        .checked_add(WAKE_LEASE_DURATION_MS)
        .ok_or_else(|| invalid("claimed_at", "cannot compute the Wake lease deadline"))?;
    let changed = transaction
        .execute(
            "UPDATE agent_wake_requests
             SET status = 'claimed', status_revision = status_revision + 1,
                 claim_token = ?1, lease_expires_at = ?2, claimed_at = ?3
             WHERE wake_id = ?4 AND status = 'queued'",
            params![claim_token, lease_expires_at, claimed_at, &wake_id],
        )
        .map_err(write_error)?;
    if changed != 1 {
        return Err(conflict("global Wake claim lost its durable CAS"));
    }
    let wake = query_wake(&transaction, &wake_id)?
        .ok_or_else(|| corrupt("globally claimed Wake disappeared"))?;
    transaction.commit().map_err(write_error)?;
    Ok(Some(wake))
}

/// Reconciles leases left by an earlier Host without replaying a possibly side-effecting Turn.
/// Claimed work has not crossed atomic Turn admission and is safely requeued. Running work always
/// has immutable run/assistant identity: terminal traces and resumable approvals are rebound for
/// observation, a missing trace is definitely pre-Runtime, and an in-progress non-approval trace
/// becomes outcome-unknown.
pub fn recover_agent_wakes(
    connection: &mut Connection,
    recovery_token_prefix: &str,
    recovered_at: i64,
) -> Result<AgentWakeRecoveryBatch, AgentGraphError> {
    validate_id("recovery_token_prefix", recovery_token_prefix)?;
    validate_time(recovered_at)?;
    let transaction = immediate(connection)?;
    let requeued_before_dispatch = transaction
        .execute(
            "UPDATE agent_wake_requests
             SET status = 'queued', status_revision = status_revision + 1,
                 claim_token = NULL, lease_expires_at = NULL, claimed_at = NULL
             WHERE status = 'claimed' AND lease_expires_at <= ?1",
            [recovered_at],
        )
        .map_err(write_error)?;

    let mut statement = transaction
        .prepare(&format!(
            "{WAKE_SELECT}
             WHERE status IN ('running', 'waiting_for_approval')
               AND lease_expires_at <= ?1
             ORDER BY sequence, wake_id"
        ))
        .map_err(read_error)?;
    let expired = statement
        .query_map([recovered_at], read_wake_row)
        .map_err(read_error)?
        .map(|row| row.map_err(read_error).and_then(decode_wake))
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);

    let mut actions = Vec::with_capacity(expired.len());
    for mut wake in expired {
        let (Some(run_id), Some(assistant_message_id)) =
            (wake.run_id.as_deref(), wake.assistant_message_id.as_deref())
        else {
            return Err(corrupt(
                "active Wake is missing its immutable Turn identity",
            ));
        };
        let trace_status = transaction
            .query_row(
                "SELECT terminal_status FROM conversation_turn_traces
                 WHERE assistant_message_id = ?1 AND run_id = ?2",
                params![assistant_message_id, run_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(read_error)?;
        let has_pending_approval = transaction
            .query_row(
                "SELECT EXISTS(
                     SELECT 1 FROM agent_pending_actions
                     WHERE run_id = ?1 AND assistant_message_id = ?2
                       AND status IN ('pending', 'approved', 'executing')
                 )",
                params![run_id, assistant_message_id],
                |row| row.get::<_, bool>(0),
            )
            .map_err(read_error)?;
        let action = match trace_status.as_deref() {
            None => AgentWakeRecoveryAction::FailBeforeRuntime(wake.clone()),
            Some("completed" | "failed" | "cancelled") => {
                AgentWakeRecoveryAction::Observe(wake.clone())
            }
            Some("in_progress") if has_pending_approval => {
                AgentWakeRecoveryAction::Observe(wake.clone())
            }
            Some("in_progress") => AgentWakeRecoveryAction::OutcomeUnknown(wake.clone()),
            Some(_) => {
                return Err(corrupt(
                    "active Wake references an invalid Turn trace status",
                ))
            }
        };
        let recovery_token = stable_fact_id(
            "wake-recovery-claim",
            &[recovery_token_prefix, &wake.wake_id],
        );
        let deadline = recovered_at
            .checked_add(WAKE_LEASE_DURATION_MS)
            .ok_or_else(|| invalid("recovered_at", "cannot compute recovery lease"))?;
        let changed = transaction
            .execute(
                "UPDATE agent_wake_requests
                 SET claim_token = ?1, lease_expires_at = ?2, claimed_at = ?6
                 WHERE wake_id = ?3 AND status = ?4 AND claim_token = ?5
                   AND lease_expires_at <= ?6",
                params![
                    &recovery_token,
                    deadline,
                    &wake.wake_id,
                    wake.status.as_str(),
                    &wake.claim_token,
                    recovered_at,
                ],
            )
            .map_err(write_error)?;
        if changed != 1 {
            return Err(conflict("expired Wake recovery lost its ownership CAS"));
        }
        wake.claim_token = Some(recovery_token);
        wake.lease_expires_at = Some(deadline);
        actions.push(match action {
            AgentWakeRecoveryAction::Observe(_) => AgentWakeRecoveryAction::Observe(wake),
            AgentWakeRecoveryAction::FailBeforeRuntime(_) => {
                AgentWakeRecoveryAction::FailBeforeRuntime(wake)
            }
            AgentWakeRecoveryAction::OutcomeUnknown(_) => {
                AgentWakeRecoveryAction::OutcomeUnknown(wake)
            }
        });
    }
    transaction.commit().map_err(write_error)?;
    Ok(AgentWakeRecoveryBatch {
        requeued_before_dispatch,
        actions,
    })
}

/// Authorizes an internal management interrupt against one strict descendant. A queued or
/// claimed-before-admission Wake is atomically cancelled. Once atomic Turn admission has bound a
/// run identity, persistence is left untouched here and the Host must propagate cancellation to
/// that exact run; its durable observer later settles the Wake as `interrupted` with a result.
pub fn interrupt_agent_execution(
    connection: &mut Connection,
    caller_agent_id: &str,
    target_agent_id: &str,
    request_id: &str,
    interrupted_at: i64,
) -> Result<InterruptAgentExecutionOutcome, AgentGraphError> {
    validate_id("caller_agent_id", caller_agent_id)?;
    validate_id("target_agent_id", target_agent_id)?;
    validate_request_id(request_id)?;
    validate_time(interrupted_at)?;
    let transaction = immediate(connection)?;
    let caller = ensure_active_agent(&transaction, caller_agent_id)?;
    let target = ensure_active_agent(&transaction, target_agent_id)?;
    if caller.root_agent_id != target.root_agent_id {
        return Err(conflict("interrupt authority cannot cross Agent trees"));
    }
    if !is_strict_descendant(&transaction, &caller.agent_id, &target.agent_id)? {
        return Err(conflict(
            "interrupt authority is limited to a caller's strict descendants",
        ));
    }
    if let Some((persisted_target, disposition)) =
        query_interrupt_receipt(&transaction, caller_agent_id, request_id)?
    {
        if persisted_target != target_agent_id {
            return Err(conflict(
                "interrupt request ID is already bound to another target",
            ));
        }
        transaction.commit().map_err(write_error)?;
        return Ok(disposition);
    }

    let active = query_wakes(
        &transaction,
        &format!(
            "{WAKE_SELECT}
             WHERE agent_id = ?1
               AND status IN ('claimed', 'running', 'waiting_for_approval')
             ORDER BY sequence, wake_id LIMIT 1"
        ),
        [target_agent_id],
    )?
    .into_iter()
    .next();
    if let Some(wake) = active {
        if matches!(
            wake.status,
            AgentWakeStatus::Running | AgentWakeStatus::WaitingForApproval
        ) {
            let run_id = wake
                .run_id
                .clone()
                .ok_or_else(|| corrupt("active admitted Wake is missing run identity"))?;
            let outcome = InterruptAgentExecutionOutcome::ActiveTurn {
                wake_id: wake.wake_id.clone(),
                run_id: run_id.clone(),
            };
            insert_interrupt_receipt(
                &transaction,
                &caller,
                &target,
                request_id,
                &outcome,
                interrupted_at,
            )?;
            transaction.commit().map_err(write_error)?;
            return Ok(outcome);
        }
        let changed = transaction
            .execute(
                "UPDATE agent_wake_requests
                 SET status = 'cancelled', status_revision = status_revision + 1,
                     completed_at = ?1
                 WHERE wake_id = ?2 AND status = 'claimed' AND run_id IS NULL",
                params![interrupted_at, &wake.wake_id],
            )
            .map_err(write_error)?;
        if changed != 1 {
            return Err(conflict("claimed Wake interrupt lost its durable CAS"));
        }
        let outcome = InterruptAgentExecutionOutcome::QueuedWakeCancelled {
            wake_id: wake.wake_id,
        };
        insert_interrupt_receipt(
            &transaction,
            &caller,
            &target,
            request_id,
            &outcome,
            interrupted_at,
        )?;
        transaction.commit().map_err(write_error)?;
        return Ok(outcome);
    }

    let queued_id = transaction
        .query_row(
            "SELECT wake_id FROM agent_wake_requests
             WHERE agent_id = ?1 AND status = 'queued'
             ORDER BY sequence, wake_id LIMIT 1",
            [target_agent_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(read_error)?;
    let Some(wake_id) = queued_id else {
        insert_interrupt_receipt(
            &transaction,
            &caller,
            &target,
            request_id,
            &InterruptAgentExecutionOutcome::NoPendingExecution,
            interrupted_at,
        )?;
        transaction.commit().map_err(write_error)?;
        return Ok(InterruptAgentExecutionOutcome::NoPendingExecution);
    };
    let changed = transaction
        .execute(
            "UPDATE agent_wake_requests
             SET status = 'cancelled', status_revision = status_revision + 1,
                 completed_at = ?1
             WHERE wake_id = ?2 AND status = 'queued'",
            params![interrupted_at, &wake_id],
        )
        .map_err(write_error)?;
    if changed != 1 {
        return Err(conflict("queued Wake interrupt lost its durable CAS"));
    }
    let outcome = InterruptAgentExecutionOutcome::QueuedWakeCancelled { wake_id };
    insert_interrupt_receipt(
        &transaction,
        &caller,
        &target,
        request_id,
        &outcome,
        interrupted_at,
    )?;
    transaction.commit().map_err(write_error)?;
    Ok(outcome)
}

fn query_interrupt_receipt(
    connection: &Connection,
    caller_agent_id: &str,
    request_id: &str,
) -> Result<Option<(String, InterruptAgentExecutionOutcome)>, AgentGraphError> {
    let row = connection
        .query_row(
            "SELECT target_agent_id, disposition, wake_id, run_id
             FROM agent_interrupt_requests
             WHERE caller_agent_id = ?1 AND request_id = ?2",
            params![caller_agent_id, request_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            },
        )
        .optional()
        .map_err(read_error)?;
    row.map(|(target, disposition, wake_id, run_id)| {
        let outcome = match (disposition.as_str(), wake_id, run_id) {
            ("no_pending_execution", None, None) => {
                InterruptAgentExecutionOutcome::NoPendingExecution
            }
            ("queued_wake_cancelled", Some(wake_id), None) => {
                InterruptAgentExecutionOutcome::QueuedWakeCancelled { wake_id }
            }
            ("active_turn", Some(wake_id), Some(run_id)) => {
                InterruptAgentExecutionOutcome::ActiveTurn { wake_id, run_id }
            }
            _ => {
                return Err(corrupt(
                    "Agent interrupt receipt has invalid disposition facts",
                ))
            }
        };
        Ok((target, outcome))
    })
    .transpose()
}

fn insert_interrupt_receipt(
    connection: &Connection,
    caller: &AgentNodeRecord,
    target: &AgentNodeRecord,
    request_id: &str,
    outcome: &InterruptAgentExecutionOutcome,
    created_at: i64,
) -> Result<(), AgentGraphError> {
    let (disposition, wake_id, run_id) = match outcome {
        InterruptAgentExecutionOutcome::NoPendingExecution => ("no_pending_execution", None, None),
        InterruptAgentExecutionOutcome::QueuedWakeCancelled { wake_id } => {
            ("queued_wake_cancelled", Some(wake_id.as_str()), None)
        }
        InterruptAgentExecutionOutcome::ActiveTurn { wake_id, run_id } => {
            ("active_turn", Some(wake_id.as_str()), Some(run_id.as_str()))
        }
    };
    connection
        .execute(
            "INSERT INTO agent_interrupt_requests (
                 caller_agent_id, request_id, root_agent_id, target_agent_id,
                 disposition, wake_id, run_id, created_at, dispatched_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, NULL)",
            params![
                &caller.agent_id,
                request_id,
                &caller.root_agent_id,
                &target.agent_id,
                disposition,
                wake_id,
                run_id,
                created_at
            ],
        )
        .map_err(write_error)?;
    Ok(())
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
    if renewed_at >= current_deadline {
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
    let transaction = immediate(connection)?;
    let updated = transition_agent_wake_in_connection(
        &transaction,
        wake_id,
        expected_status,
        requested_status,
        claim_token,
        transitioned_at,
    )?;
    transaction.commit().map_err(write_error)?;
    Ok(updated)
}

/// Applies one Wake lifecycle CAS inside a caller-owned transaction.
///
/// Approval resumption uses this primitive so the pending-action decision and the child Wake
/// cannot become externally visible as two contradictory durable facts. Callers own commit or
/// rollback; this helper never starts a nested SQLite transaction.
pub(crate) fn transition_agent_wake_in_connection(
    connection: &Connection,
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
    let current = query_wake(connection, wake_id)?
        .ok_or_else(|| AgentGraphError::WakeNotFound(wake_id.to_string()))?;
    if current.status != expected_status {
        return Err(conflict(
            "expected Wake status does not match persisted status",
        ));
    }
    if current.status == requested_status {
        if current.claim_token.as_deref() == claim_token || claim_token.is_none() {
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
            .is_none_or(|deadline| transitioned_at >= deadline)
    {
        return Err(conflict("Wake lease has expired"));
    }
    if matches!(
        requested_status,
        AgentWakeStatus::Completed | AgentWakeStatus::Satisfied
    ) {
        return Err(conflict(
            "completed/satisfied Wake must be settled by its atomic result or receipt API",
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
            AgentWakeStatus::Completed | AgentWakeStatus::Satisfied => unreachable!(),
        };
    connection
        .execute(
            "UPDATE agent_wake_requests
             SET status = ?1, status_revision = status_revision + 1,
                 claim_token = ?2, lease_expires_at = ?3, claimed_at = ?4,
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
    let updated = query_wake(connection, wake_id)?
        .ok_or_else(|| corrupt("updated Wake could not be read back"))?;
    Ok(updated)
}

pub fn finish_agent_wake_with_result(
    connection: &mut Connection,
    input: &FinishAgentWakeWithResultInput,
    completed_at: i64,
) -> Result<AgentWakeRequestRecord, AgentGraphError> {
    // Round-1 repository compatibility primitive. Production Dispatcher/Host paths must call
    // `finish_agent_turn_with_result`, which constructs and freezes the typed result envelope.
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
        .is_none_or(|deadline| completed_at >= deadline)
    {
        return Err(conflict("Wake lease has expired"));
    }
    let child = query_node(&transaction, &wake.agent_id)?
        .ok_or_else(|| corrupt("Wake child Agent is missing"))?;
    let direct_parent_id = child
        .parent_agent_id
        .as_deref()
        .ok_or_else(|| conflict("root Agent Wake cannot emit a child result"))?;
    if input.result_message.root_agent_id != wake.root_agent_id
        || input.result_message.sender_agent_id != wake.agent_id
        || input.result_message.recipient_agent_id != direct_parent_id
    {
        return Err(conflict("Wake result participants do not match the Wake"));
    }
    let result = enqueue_message_in_transaction(&transaction, &input.result_message, completed_at)?;
    let result_id = result.record().message_id.clone();
    transaction
        .execute(
            "UPDATE agent_wake_requests
             SET status = ?1, status_revision = status_revision + 1,
                 result_message_id = ?2, terminal_error = ?3, completed_at = ?4
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

/// Atomically records a delegated Turn's durable terminal fact and its direct-parent result
/// Outbox. Non-root parents receive a deferred Wake in the same transaction; root parents never
/// start a background model Turn merely because a child reported a result.
pub fn finish_agent_turn_with_result(
    connection: &mut Connection,
    input: &FinishAgentTurnResultInput,
    completed_at: i64,
) -> Result<AgentTurnResultSettlement, AgentGraphError> {
    validate_time(completed_at)?;
    validate_id("wake_id", &input.wake_id)?;
    validate_id("claim_token", &input.claim_token)?;
    validate_trimmed("summary", &input.summary, MAX_RESULT_SUMMARY_BYTES)?;
    if let Some(error) = input.terminal_error.as_deref() {
        validate_trimmed("terminal_error", error, MAX_TERMINAL_ERROR_BYTES)?;
    }
    if input.run_id.is_some() != input.assistant_message_id.is_some() {
        return Err(invalid(
            "run_id",
            "run_id and assistant_message_id must both be present or both be absent",
        ));
    }
    if let Some(run_id) = input.run_id.as_deref() {
        validate_trimmed("run_id", run_id, 2_048)?;
    }
    if let Some(turn_id) = input.assistant_message_id.as_deref() {
        validate_trimmed("assistant_message_id", turn_id, 2_048)?;
    }
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
            "a completed child Turn cannot carry a terminal error",
        ));
    }

    let transaction = immediate(connection)?;
    let wake = query_wake(&transaction, &input.wake_id)?
        .ok_or_else(|| AgentGraphError::WakeNotFound(input.wake_id.clone()))?;
    let child = query_node(&transaction, &wake.agent_id)?
        .ok_or_else(|| corrupt("settling Wake child Agent is missing"))?;
    let parent_id = child
        .parent_agent_id
        .as_deref()
        .ok_or_else(|| conflict("a root Agent Wake cannot emit a delegated child result"))?;
    let parent = query_node(&transaction, parent_id)?
        .ok_or_else(|| corrupt("settling Wake direct parent Agent is missing"))?;
    if wake.root_agent_id != child.root_agent_id || parent.root_agent_id != child.root_agent_id {
        return Err(corrupt("settling Wake does not belong to one Agent tree"));
    }
    if wake.run_id != input.run_id || wake.assistant_message_id != input.assistant_message_id {
        return Err(conflict(
            "result execution identity does not match the durable Wake",
        ));
    }

    // Conservative crash recovery owns the exact child trace that generic startup reconciliation
    // deliberately skipped. Terminalize that trace and the result Outbox in this one transaction,
    // otherwise an `outcome_unknown` Wake would leave the Agent Conversation permanently busy.
    if matches!(
        input.terminal_status,
        AgentWakeStatus::OutcomeUnknown | AgentWakeStatus::Failed
    ) {
        if let (Some(run_id), Some(assistant_message_id), Some(reason)) = (
            input.run_id.as_deref(),
            input.assistant_message_id.as_deref(),
            input.terminal_error.as_deref(),
        ) {
            terminalize_recovered_agent_trace_in_transaction(
                &transaction,
                &child.conversation_id,
                run_id,
                assistant_message_id,
                reason,
                completed_at,
            )?;
        }
    }

    if wake.status.is_terminal() {
        if wake.status != input.terminal_status
            || wake.claim_token.as_deref() != Some(input.claim_token.as_str())
            || wake.terminal_error != input.terminal_error
        {
            return Err(conflict(
                "Wake already settled with different terminal facts",
            ));
        }
        let result_id = wake
            .result_message_id
            .as_deref()
            .ok_or_else(|| corrupt("terminal delegated Wake is missing its result Outbox"))?;
        let result_message = query_message(&transaction, result_id)?
            .ok_or_else(|| corrupt("terminal delegated Wake result Outbox is missing"))?;
        let envelope: AgentTurnResultEnvelope = serde_json::from_str(&result_message.content)
            .map_err(|_| corrupt("terminal delegated Wake result envelope is invalid"))?;
        if envelope.schema_version != AGENT_RESULT_ENVELOPE_SCHEMA_VERSION
            || envelope.child_agent_id != child.agent_id
            || envelope.task_name != child.task_name
            || envelope.task_path != child.task_path
            || envelope.wake_id != wake.wake_id
            || envelope.turn_id != input.assistant_message_id
            || envelope.run_id != input.run_id
            || envelope.status != input.terminal_status
            || envelope.summary != input.summary
            || envelope.terminal_error != input.terminal_error
            || result_message.kind != AgentMailboxKind::Result
            || result_message.sender_agent_id != child.agent_id
            || result_message.recipient_agent_id != parent.agent_id
            || result_message.root_agent_id != wake.root_agent_id
            || result_message.message_id != stable_fact_id("mailbox-result", &[&wake.wake_id])
            || result_message.request_id != format!("result:{}", wake.wake_id)
            || result_message.projection_message_id
                != stable_fact_id("message-result", &[&wake.wake_id])
        {
            return Err(conflict("terminal delegated Wake result payload differs"));
        }
        validate_result_artifact_refs(&envelope.artifact_refs)?;
        let parent_wake = query_wake_by_request(
            &transaction,
            &child.agent_id,
            &format!("result-wake:{}", wake.wake_id),
        )?;
        if parent.parent_agent_id.is_some() != parent_wake.is_some() {
            return Err(corrupt(
                "terminal delegated result has an invalid direct-parent deferred Wake",
            ));
        }
        transaction.commit().map_err(write_error)?;
        return Ok(AgentTurnResultSettlement {
            wake,
            result_message,
            parent_wake,
            envelope,
        });
    }

    let artifact_refs = list_result_artifacts_for_run(
        &transaction,
        &child.conversation_id,
        input.run_id.as_deref(),
    )?;
    let envelope = AgentTurnResultEnvelope {
        schema_version: AGENT_RESULT_ENVELOPE_SCHEMA_VERSION,
        child_agent_id: child.agent_id.clone(),
        task_name: child.task_name.clone(),
        task_path: child.task_path.clone(),
        wake_id: wake.wake_id.clone(),
        turn_id: input.assistant_message_id.clone(),
        run_id: input.run_id.clone(),
        status: input.terminal_status,
        summary: input.summary.clone(),
        artifact_refs,
        terminal_error: input.terminal_error.clone(),
    };
    let content = serde_json::to_string(&envelope)
        .map_err(|_| corrupt("Agent result envelope could not be serialized"))?;
    let result_input = EnqueueAgentMessageInput {
        message_id: stable_fact_id("mailbox-result", &[&wake.wake_id]),
        root_agent_id: wake.root_agent_id.clone(),
        sender_agent_id: child.agent_id.clone(),
        recipient_agent_id: parent.agent_id.clone(),
        request_id: format!("result:{}", wake.wake_id),
        kind: AgentMailboxKind::Result,
        content,
        projection_message_id: stable_fact_id("message-result", &[&wake.wake_id]),
    };

    if wake.status != input.expected_status
        || wake.claim_token.as_deref() != Some(input.claim_token.as_str())
    {
        return Err(conflict(
            "Wake status or claim token does not match settlement",
        ));
    }
    if wake
        .lease_expires_at
        .is_none_or(|deadline| completed_at >= deadline)
    {
        return Err(conflict("Wake lease has expired"));
    }
    if !wake.status.can_transition_to(input.terminal_status) {
        return Err(AgentGraphError::IllegalTransition {
            current: wake.status,
            requested: input.terminal_status,
        });
    }

    let result_message = enqueue_message_in_transaction(&transaction, &result_input, completed_at)?
        .record()
        .clone();
    transaction
        .execute(
            "UPDATE agent_wake_requests
             SET status = ?1, status_revision = status_revision + 1,
                 result_message_id = ?2, terminal_error = ?3, completed_at = ?4
             WHERE wake_id = ?5 AND status = ?6 AND claim_token = ?7",
            params![
                input.terminal_status.as_str(),
                &result_message.message_id,
                &input.terminal_error,
                completed_at,
                &wake.wake_id,
                input.expected_status.as_str(),
                &input.claim_token,
            ],
        )
        .map_err(write_error)?;
    let settled_wake = query_wake(&transaction, &wake.wake_id)?
        .ok_or_else(|| corrupt("settled delegated Wake disappeared"))?;

    let parent_wake = if parent.parent_agent_id.is_some() {
        Some(
            enqueue_wake_in_transaction(
                &transaction,
                &EnqueueAgentWakeInput {
                    wake_id: stable_fact_id("wake-result", &[&wake.wake_id]),
                    root_agent_id: wake.root_agent_id,
                    agent_id: parent.agent_id,
                    requester_agent_id: child.agent_id,
                    request_id: format!("result-wake:{}", wake.wake_id),
                    source_agent_message_id: Some(result_message.message_id.clone()),
                },
                completed_at,
            )?
            .record()
            .clone(),
        )
    } else {
        None
    };
    transaction.commit().map_err(write_error)?;
    Ok(AgentTurnResultSettlement {
        wake: settled_wake,
        result_message,
        parent_wake,
        envelope,
    })
}

fn terminalize_recovered_agent_trace_in_transaction(
    connection: &Connection,
    conversation_id: &str,
    run_id: &str,
    assistant_message_id: &str,
    reason: &str,
    completed_at: i64,
) -> Result<(), AgentGraphError> {
    let Some(trace) =
        conversation_trace_repository::get_trace_for_message(connection, assistant_message_id)
            .map_err(read_error)?
    else {
        // A failed pre-Runtime preparation intentionally has no surviving trace.
        return Ok(());
    };
    if trace.run_id != run_id || trace.conversation_id != conversation_id {
        return Err(conflict(
            "recovered trace identity does not match Wake identity",
        ));
    }
    if trace.terminal_status != crate::ConversationTurnTraceTerminalStatus::InProgress {
        return Ok(());
    }
    let model_context_items = conversation_model_context_repository::get_log_for_message(
        connection,
        assistant_message_id,
    )
    .map_err(read_error)?
    .map(|log| log.items)
    .unwrap_or_default();
    let next_sequence = trace
        .items
        .last()
        .map(crate::ConversationTurnTraceItem::sequence)
        .unwrap_or(0)
        .saturating_add(1);
    let terminal = crate::terminal_conversation_trace_from_snapshot(
        crate::ConversationTraceSnapshot {
            items: trace.items,
            model_context_items,
            next_sequence,
            truncated: trace.truncated,
        },
        run_id,
        conversation_id,
        assistant_message_id,
        crate::ConversationTurnTraceTerminalStatus::Failed,
        reason,
    )
    .map_err(|error| {
        corrupt(format!(
            "could not terminalize recovered Agent trace: {error}"
        ))
    })?;
    let created_at = connection
        .query_row(
            "SELECT created_at FROM conversation_turn_traces WHERE assistant_message_id = ?1",
            [assistant_message_id],
            |row| row.get::<_, i64>(0),
        )
        .map_err(read_error)?;
    conversation_trace_repository::commit_trace_in_connection(
        connection,
        &terminal.trace,
        created_at,
        completed_at.max(created_at),
    )
    .map_err(write_error)?;
    conversation_model_context_repository::commit_items_in_connection(
        connection,
        conversation_id,
        assistant_message_id,
        &terminal.model_context_items,
    )
    .map_err(write_error)?;
    chat_repository::reconcile_message_run_terminal_state(
        connection,
        conversation_id,
        assistant_message_id,
        run_id,
        "error",
        "failed",
        completed_at.max(created_at),
    )
    .map_err(write_error)?;
    connection
        .execute(
            "UPDATE agent_usage_records
             SET status = 'failed', error = ?1, completed_at = ?2
             WHERE run_id = ?3 AND conversation_id = ?4 AND message_id = ?5",
            params![
                reason,
                completed_at.max(created_at),
                run_id,
                conversation_id,
                assistant_message_id
            ],
        )
        .map_err(write_error)?;
    Ok(())
}

fn list_result_artifacts_for_run(
    connection: &Connection,
    conversation_id: &str,
    run_id: Option<&str>,
) -> Result<Vec<AgentResultArtifactReference>, AgentGraphError> {
    let Some(run_id) = run_id else {
        return Ok(Vec::new());
    };
    let mut statement = connection
        .prepare(
            "SELECT DISTINCT artifact.artifact_id, artifact.kind, artifact.media_type
             FROM managed_artifact_grants AS grant_record
             JOIN managed_artifacts AS artifact
               ON artifact.artifact_id = grant_record.artifact_id
             WHERE grant_record.conversation_id = ?1 AND grant_record.run_id = ?2
             ORDER BY artifact.artifact_id, artifact.kind, artifact.media_type
             LIMIT ?3",
        )
        .map_err(read_error)?;
    let rows = statement
        .query_map(
            params![conversation_id, run_id, (MAX_RESULT_ARTIFACTS + 1) as i64],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .map_err(read_error)?;
    let refs = rows
        .map(|row| {
            let (artifact_id, kind, media_type) = row.map_err(read_error)?;
            let kind = match kind.as_str() {
                "image" => AgentResultArtifactKind::Image,
                "document" => AgentResultArtifactKind::Document,
                _ => return Err(corrupt("managed Artifact kind is invalid")),
            };
            Ok(AgentResultArtifactReference {
                artifact_id,
                kind,
                media_type,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    validate_result_artifact_refs(&refs)?;
    Ok(refs)
}

fn validate_result_artifact_refs(
    refs: &[AgentResultArtifactReference],
) -> Result<(), AgentGraphError> {
    if refs.len() > MAX_RESULT_ARTIFACTS {
        return Err(conflict(format!(
            "a child result may reference at most {MAX_RESULT_ARTIFACTS} managed Artifacts"
        )));
    }
    let mut previous = None;
    for artifact in refs {
        validate_trimmed("artifact_id", &artifact.artifact_id, 256)?;
        validate_trimmed("artifact_media_type", &artifact.media_type, 256)?;
        if previous.is_some_and(|value: &str| value >= artifact.artifact_id.as_str()) {
            return Err(corrupt(
                "Agent result Artifact references are not strictly sorted and unique",
            ));
        }
        previous = Some(artifact.artifact_id.as_str());
    }
    Ok(())
}

fn enqueue_message_in_transaction(
    transaction: &Connection,
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
    let (unbound_count, unbound_bytes, ordinary_count, ordinary_bytes): (i64, i64, i64, i64) =
        transaction
            .query_row(
                "SELECT COUNT(*), COALESCE(SUM(length(CAST(mailbox.content AS BLOB))), 0),
                    COALESCE(SUM(CASE WHEN mailbox.kind IN ('message', 'followup')
                                      THEN 1 ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN mailbox.kind IN ('message', 'followup')
                                      THEN length(CAST(mailbox.content AS BLOB)) ELSE 0 END), 0)
             FROM agent_mailbox_messages AS mailbox
             WHERE mailbox.recipient_agent_id = ?1
               AND NOT EXISTS (
                   SELECT 1 FROM agent_model_batch_receipt_items AS item
                   WHERE item.message_id = mailbox.message_id
               )",
                [&input.recipient_agent_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .map_err(read_error)?;
    let unbound_count = u64::try_from(unbound_count)
        .map_err(|_| corrupt("unbound Mailbox message count is invalid"))?;
    let unbound_bytes = u64::try_from(unbound_bytes)
        .map_err(|_| corrupt("unbound Mailbox byte count is invalid"))?;
    let ordinary_count = u64::try_from(ordinary_count)
        .map_err(|_| corrupt("ordinary unbound Mailbox message count is invalid"))?;
    let ordinary_bytes = u64::try_from(ordinary_bytes)
        .map_err(|_| corrupt("ordinary unbound Mailbox byte count is invalid"))?;
    let ordinary = matches!(
        input.kind,
        AgentMailboxKind::Message | AgentMailboxKind::Followup
    );
    if ordinary && ordinary_count >= MAX_UNBOUND_ORDINARY_MAILBOX_MESSAGES_PER_RECIPIENT {
        return Err(AgentGraphError::ResourceLimit {
            resource: "ordinary unbound Mailbox messages per recipient",
            limit: MAX_UNBOUND_ORDINARY_MAILBOX_MESSAGES_PER_RECIPIENT,
        });
    }
    if ordinary
        && ordinary_bytes.saturating_add(input.content.len() as u64)
            > MAX_UNBOUND_ORDINARY_MAILBOX_BYTES_PER_RECIPIENT
    {
        return Err(AgentGraphError::ResourceLimit {
            resource: "ordinary unbound Mailbox bytes per recipient",
            limit: MAX_UNBOUND_ORDINARY_MAILBOX_BYTES_PER_RECIPIENT,
        });
    }
    if unbound_count >= MAX_UNBOUND_MAILBOX_MESSAGES_PER_RECIPIENT {
        return Err(AgentGraphError::ResourceLimit {
            resource: "unbound Mailbox messages per recipient",
            limit: MAX_UNBOUND_MAILBOX_MESSAGES_PER_RECIPIENT,
        });
    }
    if unbound_bytes.saturating_add(input.content.len() as u64)
        > MAX_UNBOUND_MAILBOX_BYTES_PER_RECIPIENT
    {
        return Err(AgentGraphError::ResourceLimit {
            resource: "unbound Mailbox bytes per recipient",
            limit: MAX_UNBOUND_MAILBOX_BYTES_PER_RECIPIENT,
        });
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

fn query_effective_permission_snapshot(
    connection: &Connection,
    agent_id: &str,
) -> Result<Option<AgentEffectivePermissionSnapshot>, AgentGraphError> {
    connection
        .query_row(
            &format!("{EFFECTIVE_PERMISSION_SELECT} WHERE snapshot.agent_id = ?1"),
            [agent_id],
            read_effective_permission_row,
        )
        .optional()
        .map_err(read_error)?
        .map(decode_effective_permission_snapshot)
        .transpose()
}

fn read_effective_permission_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<EffectivePermissionRow> {
    Ok(EffectivePermissionRow {
        agent_id: row.get(0)?,
        schema_version: row.get(1)?,
        root_agent_id: row.get(2)?,
        conversation_id: row.get(3)?,
        source_run_id: row.get(4)?,
        source_assistant_message_id: row.get(5)?,
        read_permission: row.get(6)?,
        write_permission: row.get(7)?,
        command_permission: row.get(8)?,
        command_safety_policy: row.get(9)?,
        patch_permission: row.get(10)?,
        builtin_execution_permission: row.get(11)?,
        revision: row.get(12)?,
        created_at: row.get(13)?,
        updated_at: row.get(14)?,
    })
}

struct EffectivePermissionRow {
    agent_id: String,
    schema_version: i64,
    root_agent_id: String,
    conversation_id: String,
    source_run_id: String,
    source_assistant_message_id: String,
    read_permission: String,
    write_permission: String,
    command_permission: String,
    command_safety_policy: String,
    patch_permission: String,
    builtin_execution_permission: String,
    revision: i64,
    created_at: i64,
    updated_at: i64,
}

fn decode_effective_permission_snapshot(
    row: EffectivePermissionRow,
) -> Result<AgentEffectivePermissionSnapshot, AgentGraphError> {
    if row.schema_version != i64::from(AGENT_EFFECTIVE_PERMISSION_SNAPSHOT_SCHEMA_VERSION) {
        return Err(corrupt(
            "unsupported Agent effective permission snapshot schema version",
        ));
    }
    Ok(AgentEffectivePermissionSnapshot {
        agent_id: row.agent_id,
        root_agent_id: row.root_agent_id,
        conversation_id: row.conversation_id,
        source_run_id: row.source_run_id,
        source_assistant_message_id: row.source_assistant_message_id,
        permissions: AgentPermissions {
            read: parse_read_permission(&row.read_permission)?,
            write: parse_write_permission(&row.write_permission)?,
            command: parse_command_permission(&row.command_permission)?,
            command_safety: parse_command_safety(&row.command_safety_policy)?,
            patch: parse_patch_permission(&row.patch_permission)?,
            builtin_execution: parse_builtin_execution_permission(
                &row.builtin_execution_permission,
            )?,
        },
        revision: positive_u64(row.revision, "Agent effective permission revision")?,
        created_at: row.created_at,
        updated_at: row.updated_at,
    })
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
        model_selection_source: row.get(25)?,
        reasoning_effort_snapshot: row.get(26)?,
        lifecycle: row.get(27)?,
        revision: row.get(28)?,
        created_at: row.get(29)?,
        updated_at: row.get(30)?,
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
    model_selection_source: Option<String>,
    reasoning_effort_snapshot: Option<String>,
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
    let model_selection_source = row
        .model_selection_source
        .as_deref()
        .map(crate::AgentModelSelectionSource::parse)
        .transpose()?;
    let reasoning_effort_snapshot = row
        .reasoning_effort_snapshot
        .as_deref()
        .map(reasoning_effort_from_str)
        .transpose()?;
    match (
        row.parent_agent_id.is_some(),
        model_selection_source,
        template_snapshot.as_ref(),
        model_snapshot.as_ref(),
    ) {
        (false, None, None, None) => {}
        (true, Some(AgentModelSelectionSource::Explicit), _, Some(_)) => {}
        (true, Some(AgentModelSelectionSource::Template), Some(template), Some(model))
            if template.model_config_id == model.model_config_id => {}
        (
            true,
            Some(AgentModelSelectionSource::Parent | AgentModelSelectionSource::Default),
            None,
            Some(_),
        ) => {}
        _ => return Err(corrupt("Agent model selector provenance is inconsistent")),
    }
    if row.parent_agent_id.is_none() && reasoning_effort_snapshot.is_some() {
        return Err(corrupt("root Agent cannot freeze a reasoning effort"));
    }
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
        model_selection_source,
        reasoning_effort_snapshot,
        lifecycle: AgentLifecycle::parse(&row.lifecycle)?,
        revision: positive_u64(row.revision, "Agent revision")?,
        created_at: row.created_at,
        updated_at: row.updated_at,
    })
}

fn reasoning_effort_as_str(effort: ReasoningEffort) -> &'static str {
    match effort {
        ReasoningEffort::High => "high",
        ReasoningEffort::Max => "max",
        ReasoningEffort::ProviderDefault => "provider_default",
    }
}

fn reasoning_effort_from_str(value: &str) -> Result<ReasoningEffort, AgentGraphError> {
    match value {
        "high" => Ok(ReasoningEffort::High),
        "max" => Ok(ReasoningEffort::Max),
        _ => Err(corrupt("Agent reasoning effort snapshot is invalid")),
    }
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

fn query_wakes<P: rusqlite::Params>(
    connection: &Connection,
    sql: &str,
    params: P,
) -> Result<Vec<AgentWakeRequestRecord>, AgentGraphError> {
    let mut statement = connection.prepare(sql).map_err(read_error)?;
    let rows = statement
        .query_map(params, read_wake_row)
        .map_err(read_error)?;
    rows.map(|row| row.map_err(read_error).and_then(decode_wake))
        .collect()
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
        status_revision: row.get(9)?,
        claim_token: row.get(10)?,
        lease_expires_at: row.get(11)?,
        result_message_id: row.get(12)?,
        terminal_error: row.get(13)?,
        run_id: row.get(14)?,
        assistant_message_id: row.get(15)?,
        created_at: row.get(16)?,
        claimed_at: row.get(17)?,
        started_at: row.get(18)?,
        completed_at: row.get(19)?,
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
    status_revision: i64,
    claim_token: Option<String>,
    lease_expires_at: Option<i64>,
    result_message_id: Option<String>,
    terminal_error: Option<String>,
    run_id: Option<String>,
    assistant_message_id: Option<String>,
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
        status_revision: positive_u64(row.status_revision, "Wake status revision")?,
        claim_token: row.claim_token,
        lease_expires_at: row.lease_expires_at,
        result_message_id: row.result_message_id,
        terminal_error: row.terminal_error,
        run_id: row.run_id,
        assistant_message_id: row.assistant_message_id,
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
    validate_identity_task_name(&input.task_name)?;
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

fn is_strict_descendant(
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

fn validate_wake_source_authority(
    connection: &Connection,
    source: &AgentMailboxMessageRecord,
    target_agent_id: &str,
) -> Result<(), AgentGraphError> {
    let sender = ensure_active_agent(connection, &source.sender_agent_id)?;
    let target = ensure_active_agent(connection, target_agent_id)?;
    let authorized = match source.kind {
        AgentMailboxKind::Task => {
            target.parent_agent_id.as_deref() == Some(sender.agent_id.as_str())
        }
        AgentMailboxKind::Followup => {
            is_strict_descendant(connection, &sender.agent_id, &target.agent_id)?
        }
        AgentMailboxKind::Result => {
            sender.parent_agent_id.as_deref() == Some(target.agent_id.as_str())
        }
        AgentMailboxKind::Message => false,
    };
    if !authorized {
        return Err(conflict(
            "Wake source violates task/follow-up/result tree authority",
        ));
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

fn read_permission_as_str(value: AgentReadPermission) -> &'static str {
    match value {
        AgentReadPermission::WorkspaceOnly => "workspace_only",
        AgentReadPermission::All => "all",
    }
}

fn parse_read_permission(value: &str) -> Result<AgentReadPermission, AgentGraphError> {
    match value {
        "workspace_only" => Ok(AgentReadPermission::WorkspaceOnly),
        "all" => Ok(AgentReadPermission::All),
        _ => Err(corrupt("unknown effective read permission")),
    }
}

fn write_permission_as_str(value: AgentWritePermission) -> &'static str {
    match value {
        AgentWritePermission::Denied => "denied",
        AgentWritePermission::WorkspaceOnly => "workspace_only",
        AgentWritePermission::All => "all",
    }
}

fn parse_write_permission(value: &str) -> Result<AgentWritePermission, AgentGraphError> {
    match value {
        "denied" => Ok(AgentWritePermission::Denied),
        "workspace_only" => Ok(AgentWritePermission::WorkspaceOnly),
        "all" => Ok(AgentWritePermission::All),
        _ => Err(corrupt("unknown effective write permission")),
    }
}

fn command_permission_as_str(value: AgentCommandPermission) -> &'static str {
    match value {
        AgentCommandPermission::RequireApproval => "require_approval",
        AgentCommandPermission::AutoApprove => "auto_approve",
    }
}

fn parse_command_permission(value: &str) -> Result<AgentCommandPermission, AgentGraphError> {
    match value {
        "require_approval" => Ok(AgentCommandPermission::RequireApproval),
        "auto_approve" => Ok(AgentCommandPermission::AutoApprove),
        _ => Err(corrupt("unknown effective command permission")),
    }
}

fn command_safety_as_str(value: AgentCommandSafetyPolicy) -> &'static str {
    match value {
        AgentCommandSafetyPolicy::Guarded => "guarded",
        AgentCommandSafetyPolicy::FullAccess => "full_access",
    }
}

fn parse_command_safety(value: &str) -> Result<AgentCommandSafetyPolicy, AgentGraphError> {
    match value {
        "guarded" => Ok(AgentCommandSafetyPolicy::Guarded),
        "full_access" => Ok(AgentCommandSafetyPolicy::FullAccess),
        _ => Err(corrupt("unknown effective command safety policy")),
    }
}

fn patch_permission_as_str(value: AgentPatchPermission) -> &'static str {
    match value {
        AgentPatchPermission::RequireApproval => "require_approval",
        AgentPatchPermission::AutoApprove => "auto_approve",
    }
}

fn parse_patch_permission(value: &str) -> Result<AgentPatchPermission, AgentGraphError> {
    match value {
        "require_approval" => Ok(AgentPatchPermission::RequireApproval),
        "auto_approve" => Ok(AgentPatchPermission::AutoApprove),
        _ => Err(corrupt("unknown effective patch permission")),
    }
}

fn builtin_execution_permission_as_str(value: AgentBuiltinExecutionPermission) -> &'static str {
    match value {
        AgentBuiltinExecutionPermission::RequireApproval => "require_approval",
        AgentBuiltinExecutionPermission::AutoApprove => "auto_approve",
    }
}

fn parse_builtin_execution_permission(
    value: &str,
) -> Result<AgentBuiltinExecutionPermission, AgentGraphError> {
    match value {
        "require_approval" => Ok(AgentBuiltinExecutionPermission::RequireApproval),
        "auto_approve" => Ok(AgentBuiltinExecutionPermission::AutoApprove),
        _ => Err(corrupt("unknown effective built-in execution permission")),
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

fn validate_identity_task_name(value: &str) -> Result<(), AgentGraphError> {
    validate_trimmed("task_name", value, MAX_TASK_NAME_BYTES)?;
    if value.contains('/') || value.chars().any(char::is_control) {
        return Err(invalid(
            "task_name",
            "must be one control-free Agent task-path segment",
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

fn stable_fact_id(prefix: &str, parts: &[&str]) -> String {
    let mut digest = Sha256::new();
    for part in parts {
        digest.update((part.len() as u64).to_be_bytes());
        digest.update(part.as_bytes());
    }
    format!("{prefix}-{:x}", digest.finalize())
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
mod tests;
