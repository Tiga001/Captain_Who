use super::agent_templates::resolve_exact_agent_model;
use super::conversations::cleanup_fork_files;
use super::*;
use crate::storage::child_context_snapshot_repository;
use crate::{
    AgentCollaborationIdentity, AgentForkTurns, AgentGraphError, AgentLifecycle, AgentMailboxKind,
    AgentModelSelectionSource, AgentTemplateError, AgentTemplateSnapshot, ChildAgentSpawnError,
    ChildAgentSpawnRecord, CreateAgentNodeInput, CreateChildAgentInput, EnqueueAgentMessageInput,
    EnqueueAgentWakeInput, IdempotentCreate,
};
use rusqlite::{params, TransactionBehavior};

const MAX_ID_BYTES: usize = 128;
const MAX_TASK_NAME_BYTES: usize = 256;
const MAX_TASK_BYTES: usize = 1_048_576;
const MAX_MODEL_ID_BYTES: usize = 512;
const MAX_TEMPLATE_KEY_BYTES: usize = 64;
type ChildAttachmentStage = (Vec<(PathBuf, PathBuf)>, Vec<PathBuf>);

impl StorageService {
    /// Atomically creates the complete durable identity bundle for one direct child Agent.
    ///
    /// This Host-internal boundary does not claim or execute the queued Wake. Every selector is
    /// resolved under the same SQLite write transaction that freezes the Agent node.
    pub fn create_child_agent(
        &self,
        input: &CreateChildAgentInput,
    ) -> Result<ChildAgentSpawnRecord, ChildAgentSpawnError> {
        validate_spawn_input(input)?;
        let created_at = now_ms();
        let mut connection = self
            .state
            .connection()
            .map_err(ChildAgentSpawnError::StorageUnavailable)?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(spawn_database_error)?;
        let parent = agent_graph_repository::get_agent_node(&transaction, &input.parent_agent_id)
            .map_err(map_graph_error)?
            .ok_or_else(|| ChildAgentSpawnError::ParentNotFound(input.parent_agent_id.clone()))?;
        if let Some(existing) = agent_graph_repository::resolve_child_spawn_by_creation_request(
            &transaction,
            &parent.agent_id,
            &input.creation_request_id,
        )
        .map_err(map_spawn_lookup_error)?
        {
            ensure_idempotent_spawn(&existing, input)?;
            // The snapshot repository owns comparison of the persisted logical-turn selector.
            ensure_existing_child_snapshot_selector(
                &transaction,
                &existing.agent.conversation_id,
                input.fork_turns,
            )?;
            transaction.commit().map_err(spawn_database_error)?;
            return Ok(existing);
        }
        if parent.lifecycle != AgentLifecycle::Active {
            return Err(ChildAgentSpawnError::ParentUnavailable(
                parent.agent_id.clone(),
            ));
        }

        let (template_snapshot, selected_model_id, model_selection_source) =
            select_model_identity(&transaction, &parent, input)?;
        let settings = config_repository::load_model_settings_snapshot_in_connection(&transaction)
            .map_err(spawn_database_error)?
            .ok_or_else(|| ChildAgentSpawnError::ModelUnavailable {
                model_config_id: Some(selected_model_id.clone()),
                reason: crate::AgentModelUnavailableReason::SettingsMissing,
            })?;
        let model_snapshot =
            resolve_exact_agent_model(&settings, &selected_model_id).map_err(|reason| {
                ChildAgentSpawnError::ModelUnavailable {
                    model_config_id: Some(selected_model_id.clone()),
                    reason,
                }
            })?;
        validate_reasoning_effort(&settings, &selected_model_id, input.reasoning_effort)?;

        let child_agent_id = new_spawn_id("agent");
        let child_conversation_id = new_spawn_id("conversation");
        let task_message_id = new_spawn_id("agent-message");
        let projection_message_id = new_spawn_id("message");
        let wake_id = new_spawn_id("agent-wake");
        let claim_token = new_spawn_id("agent-message-claim");
        let task_path = format!("{}/{}", parent.task_path, input.task_name);

        insert_child_conversation(
            &transaction,
            &child_conversation_id,
            parent.project_id.as_deref(),
            &model_snapshot.model_config_id,
            &input.task_name,
            created_at,
        )?;
        let node_input = CreateAgentNodeInput {
            agent_id: child_agent_id.clone(),
            root_agent_id: parent.root_agent_id.clone(),
            parent_agent_id: parent.agent_id.clone(),
            conversation_id: child_conversation_id.clone(),
            creation_request_id: input.creation_request_id.clone(),
            task_name: input.task_name.clone(),
            task_path,
            template_snapshot,
            model_snapshot,
        };
        let agent = match agent_graph_repository::create_agent_node_in_transaction(
            &transaction,
            &node_input,
            model_selection_source,
            input.reasoning_effort,
            created_at,
        )
        .map_err(map_graph_error)?
        {
            IdempotentCreate::Created(agent) => agent,
            IdempotentCreate::Existing(_) => {
                return Err(ChildAgentSpawnError::CorruptRecord(
                    "new spawn unexpectedly resolved as an existing node".to_string(),
                ))
            }
        };

        let mut snapshot_plan =
            child_context_snapshot_repository::build_child_context_snapshot_plan(
                &transaction,
                &parent.conversation_id,
                &child_conversation_id,
                &input.fork_turns,
                created_at,
            )
            .map_err(|error| ChildAgentSpawnError::SnapshotUnavailable(error.to_string()))?;
        let (staged_files, committed_files) =
            self.stage_child_snapshot_attachments(&mut snapshot_plan)?;
        let persistence_result = (|| {
            child_context_snapshot_repository::apply_child_context_snapshot_in_transaction(
                &transaction,
                &snapshot_plan,
            )
            .map_err(|error| ChildAgentSpawnError::SnapshotUnavailable(error.to_string()))?;

            let message = EnqueueAgentMessageInput {
                message_id: task_message_id,
                root_agent_id: parent.root_agent_id.clone(),
                sender_agent_id: parent.agent_id.clone(),
                recipient_agent_id: agent.agent_id.clone(),
                request_id: input.creation_request_id.clone(),
                kind: AgentMailboxKind::Task,
                content: input.task.clone(),
                projection_message_id,
            };
            let wake = EnqueueAgentWakeInput {
                wake_id,
                root_agent_id: parent.root_agent_id.clone(),
                agent_id: agent.agent_id.clone(),
                requester_agent_id: parent.agent_id.clone(),
                request_id: input.creation_request_id.clone(),
                source_agent_message_id: Some(message.message_id.clone()),
            };
            let (task_message, initial_wake) =
                agent_graph_repository::create_initial_agent_task_and_wake_in_transaction(
                    &transaction,
                    &message,
                    &wake,
                    &claim_token,
                    created_at,
                )
                .map_err(map_graph_error)?;
            let collaboration_identity = collaboration_identity(&parent, &agent, &task_message)?;
            let record = ChildAgentSpawnRecord {
                agent,
                model_selection_source,
                task_message,
                initial_wake,
                collaboration_identity,
            };
            transaction.commit().map_err(spawn_database_error)?;
            Ok(record)
        })();
        if persistence_result.is_err() {
            cleanup_fork_files(&staged_files, &committed_files);
        }
        persistence_result
    }

    /// Resolves trusted Wake execution facts exclusively from the durable collaboration bundle.
    pub fn resolve_child_agent_wake(
        &self,
        agent_id: &str,
        wake_id: &str,
        source_agent_message_id: &str,
    ) -> Result<ChildAgentSpawnRecord, AgentGraphError> {
        let connection = self
            .state
            .connection()
            .map_err(AgentGraphError::StorageUnavailable)?;
        agent_graph_repository::resolve_child_wake_bundle(
            &connection,
            agent_id,
            wake_id,
            source_agent_message_id,
        )
    }

    /// Authorizes one execution attempt only after the Wake has been claimed and transitioned to
    /// `running`. The clock is Host-owned and the exact claim token remains a capability checked
    /// against the durable lease.
    pub fn resolve_running_child_agent_wake(
        &self,
        agent_id: &str,
        wake_id: &str,
        source_agent_message_id: &str,
        claim_token: &str,
    ) -> Result<ChildAgentSpawnRecord, AgentGraphError> {
        let connection = self
            .state
            .connection()
            .map_err(AgentGraphError::StorageUnavailable)?;
        let spawn = agent_graph_repository::resolve_running_child_wake_bundle(
            &connection,
            agent_id,
            wake_id,
            source_agent_message_id,
            claim_token,
            now_ms(),
        )?;
        validate_frozen_reasoning_for_wake(&connection, &spawn)?;
        Ok(spawn)
    }

    /// Rehydrates approval-resume authorization from the exact durable collaboration identity.
    pub fn resolve_active_child_agent_wake_by_identity(
        &self,
        identity: &AgentCollaborationIdentity,
    ) -> Result<crate::TrustedActiveChildWakeBundle, AgentGraphError> {
        let connection = self
            .state
            .connection()
            .map_err(AgentGraphError::StorageUnavailable)?;
        let bundle = agent_graph_repository::resolve_active_child_wake_bundle_by_identity(
            &connection,
            identity,
            now_ms(),
        )?;
        validate_frozen_reasoning_for_wake(&connection, &bundle.spawn)?;
        Ok(bundle)
    }

    fn stage_child_snapshot_attachments(
        &self,
        plan: &mut child_context_snapshot_repository::ChildContextSnapshotPlan,
    ) -> Result<ChildAttachmentStage, ChildAgentSpawnError> {
        let mut staged_files = Vec::new();
        let mut committed_files = Vec::new();
        let prepare = (|| -> Result<(), ChildAgentSpawnError> {
            for attachment in &mut plan.attachments {
                let source_path = safe_existing_attachment_storage_path(
                    &self.attachment_root,
                    &attachment.source.storage_rel_path,
                )
                .ok_or_else(|| {
                    ChildAgentSpawnError::SnapshotUnavailable(format!(
                        "source attachment is missing: {}",
                        attachment.source.original_name
                    ))
                })?;
                let target_rel_path = attachment_storage_rel_path(
                    &attachment.target.conversation_id,
                    &attachment.target.message_id,
                    &attachment.target.id,
                    &attachment.target.original_name,
                );
                attachment.target.storage_rel_path = slash_path(&target_rel_path);
                let target_path = self.attachment_root.join(&target_rel_path);
                let parent = target_path.parent().ok_or_else(|| {
                    ChildAgentSpawnError::SnapshotUnavailable(
                        "child snapshot attachment path is invalid".to_string(),
                    )
                })?;
                fs::create_dir_all(parent).map_err(|error| {
                    ChildAgentSpawnError::SnapshotUnavailable(format!(
                        "failed to create child attachment directory: {error}"
                    ))
                })?;
                let staging_path = parent.join(format!(
                    ".{}.child-snapshot-{}",
                    safe_path_component(&attachment.target.id, "attachment"),
                    Uuid::new_v4()
                ));
                staged_files.push((staging_path.clone(), target_path));
                let copied = fs::copy(source_path, &staging_path).map_err(|error| {
                    ChildAgentSpawnError::SnapshotUnavailable(format!(
                        "failed to copy child snapshot attachment: {error}"
                    ))
                })?;
                if copied != attachment.source.size_bytes {
                    return Err(ChildAgentSpawnError::SnapshotUnavailable(format!(
                        "child snapshot attachment size changed: {}",
                        attachment.source.original_name
                    )));
                }
            }
            for (staging_path, target_path) in &staged_files {
                fs::rename(staging_path, target_path).map_err(|error| {
                    ChildAgentSpawnError::SnapshotUnavailable(format!(
                        "failed to publish child snapshot attachment: {error}"
                    ))
                })?;
                committed_files.push(target_path.clone());
            }
            Ok(())
        })();
        if let Err(error) = prepare {
            cleanup_fork_files(&staged_files, &committed_files);
            return Err(error);
        }
        Ok((staged_files, committed_files))
    }
}

fn validate_spawn_input(input: &CreateChildAgentInput) -> Result<(), ChildAgentSpawnError> {
    validate_bounded_trimmed("parent_agent_id", &input.parent_agent_id, MAX_ID_BYTES)?;
    validate_bounded_trimmed("creation_request_id", &input.creation_request_id, 256)?;
    validate_bounded_trimmed("task_name", &input.task_name, MAX_TASK_NAME_BYTES)?;
    if input.task_name.contains('/') || input.task_name.chars().any(char::is_control) {
        return Err(invalid_spawn(
            "task_name",
            "must be one control-free path segment",
        ));
    }
    validate_bounded_trimmed("task", &input.task, MAX_TASK_BYTES)?;
    validate_optional_selector(
        "template_machine_key",
        input.template_machine_key.as_deref(),
        MAX_TEMPLATE_KEY_BYTES,
    )?;
    validate_optional_selector(
        "explicit_model_id",
        input.explicit_model_id.as_deref(),
        MAX_MODEL_ID_BYTES,
    )?;
    input.fork_turns.validate()?;
    Ok(())
}

fn validate_optional_selector(
    field: &'static str,
    value: Option<&str>,
    max_bytes: usize,
) -> Result<(), ChildAgentSpawnError> {
    if let Some(value) = value {
        validate_bounded_trimmed(field, value, max_bytes)?;
    }
    Ok(())
}

fn validate_bounded_trimmed(
    field: &'static str,
    value: &str,
    max_bytes: usize,
) -> Result<(), ChildAgentSpawnError> {
    if value.trim() != value || value.is_empty() || value.len() > max_bytes || value.contains('\0')
    {
        return Err(invalid_spawn(
            field,
            format!("must be trimmed, non-empty, NUL-free, and at most {max_bytes} bytes"),
        ));
    }
    Ok(())
}

fn select_model_identity(
    connection: &rusqlite::Connection,
    parent: &crate::AgentNodeRecord,
    input: &CreateChildAgentInput,
) -> Result<
    (
        Option<AgentTemplateSnapshot>,
        String,
        AgentModelSelectionSource,
    ),
    ChildAgentSpawnError,
> {
    let template = match input.template_machine_key.as_deref() {
        Some(machine_key) => {
            let project_id = parent
                .project_id
                .as_deref()
                .ok_or(ChildAgentSpawnError::ProjectRequiredForTemplate)?;
            let record = agent_template_repository::get_template_by_machine_key(
                connection,
                project_id,
                machine_key,
            )
            .map_err(map_template_error)?;
            if !record.enabled {
                return Err(ChildAgentSpawnError::TemplateDisabled(
                    machine_key.to_string(),
                ));
            }
            Some(AgentTemplateSnapshot {
                template_id: record.template_id,
                project_id: record.project_id,
                machine_key: record.machine_key,
                name: record.name,
                description: record.description,
                instructions: record.instructions,
                template_revision: record.revision,
                model_config_id: record.model_config_id,
            })
        }
        None => None,
    };

    if let Some(model_id) = input.explicit_model_id.as_ref() {
        return Ok((
            template,
            model_id.clone(),
            AgentModelSelectionSource::Explicit,
        ));
    }
    if let Some(template) = template {
        let model_id = template.model_config_id.clone();
        return Ok((
            Some(template),
            model_id,
            AgentModelSelectionSource::Template,
        ));
    }
    let parent_model_id = connection
        .query_row(
            "SELECT model_id FROM conversations WHERE id = ?1",
            [&parent.conversation_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .map_err(spawn_database_error)?;
    if let Some(model_id) = parent_model_id.filter(|model_id| !model_id.trim().is_empty()) {
        return Ok((None, model_id, AgentModelSelectionSource::Parent));
    }

    let settings = config_repository::load_model_settings_snapshot_in_connection(connection)
        .map_err(spawn_database_error)?
        .ok_or(ChildAgentSpawnError::ModelUnavailable {
            model_config_id: None,
            reason: crate::AgentModelUnavailableReason::SettingsMissing,
        })?;
    let default = settings
        .settings
        .models
        .iter()
        .find(|model| model.enabled)
        .ok_or(ChildAgentSpawnError::ModelUnavailable {
            model_config_id: None,
            reason: crate::AgentModelUnavailableReason::NotFound,
        })?;
    Ok((None, default.id.clone(), AgentModelSelectionSource::Default))
}

fn validate_reasoning_effort(
    settings: &crate::storage::models::ModelSettingsSnapshot,
    model_config_id: &str,
    requested: Option<crate::ReasoningEffort>,
) -> Result<(), ChildAgentSpawnError> {
    let Some(requested) = requested else {
        return Ok(());
    };
    if requested == crate::ReasoningEffort::ProviderDefault {
        return Err(ChildAgentSpawnError::UnsupportedReasoningEffort(requested));
    }
    let model = settings
        .settings
        .models
        .iter()
        .find(|model| model.id == model_config_id)
        .ok_or_else(|| ChildAgentSpawnError::ModelUnavailable {
            model_config_id: Some(model_config_id.to_string()),
            reason: crate::AgentModelUnavailableReason::NotFound,
        })?;
    let connection = settings
        .settings
        .effective_connection_for(model)
        .map_err(|_| ChildAgentSpawnError::ModelUnavailable {
            model_config_id: Some(model_config_id.to_string()),
            reason: crate::AgentModelUnavailableReason::InvalidConnection,
        })?;
    let dialect = crate::ProviderProtocolDialect::detect_from_api_url(&connection.api_url);
    let profile = model
        .resolved_provider_profile_config(dialect)
        .map_err(|_| ChildAgentSpawnError::ModelUnavailable {
            model_config_id: Some(model_config_id.to_string()),
            reason: crate::AgentModelUnavailableReason::InvalidProfile,
        })?;
    let protocol_revision = settings
        .provider_protocol_revisions
        .get(model_config_id)
        .cloned()
        .ok_or_else(|| ChildAgentSpawnError::ModelUnavailable {
            model_config_id: Some(model_config_id.to_string()),
            reason: crate::AgentModelUnavailableReason::MissingProtocolIdentity,
        })?;
    let protocol = crate::ProviderProtocolKey::new(
        dialect,
        &profile,
        model.id.clone(),
        Some(protocol_revision),
    )
    .map_err(|_| ChildAgentSpawnError::ModelUnavailable {
        model_config_id: Some(model_config_id.to_string()),
        reason: crate::AgentModelUnavailableReason::InvalidProfile,
    })?;
    crate::resolve_provider_runtime_capabilities(&protocol).map_err(|_| {
        ChildAgentSpawnError::ModelUnavailable {
            model_config_id: Some(model_config_id.to_string()),
            reason: crate::AgentModelUnavailableReason::UnsupportedRuntime,
        }
    })?;
    if profile.reasoning.mode != crate::ReasoningMode::Enabled
        || profile.reasoning.effort != requested
    {
        return Err(ChildAgentSpawnError::UnsupportedReasoningEffort(requested));
    }
    Ok(())
}

fn validate_frozen_reasoning_for_wake(
    connection: &rusqlite::Connection,
    spawn: &ChildAgentSpawnRecord,
) -> Result<(), AgentGraphError> {
    let Some(effort) = spawn.agent.reasoning_effort_snapshot else {
        return Ok(());
    };
    let model_id = spawn
        .agent
        .model_snapshot
        .as_ref()
        .map(|snapshot| snapshot.model_config_id.as_str())
        .ok_or_else(|| {
            AgentGraphError::CorruptRecord(
                "child reasoning snapshot has no model selection snapshot".to_string(),
            )
        })?;
    let settings = config_repository::load_model_settings_snapshot_in_connection(connection)
        .map_err(|error| AgentGraphError::StorageUnavailable(error.to_string()))?
        .ok_or_else(|| {
            AgentGraphError::Conflict(
                "child reasoning constraint cannot resolve current model settings".to_string(),
            )
        })?;
    validate_reasoning_effort(&settings, model_id, Some(effort)).map_err(|error| {
        AgentGraphError::Conflict(format!(
            "child reasoning constraint no longer matches its selected model: {error}"
        ))
    })
}

fn insert_child_conversation(
    connection: &rusqlite::Connection,
    conversation_id: &str,
    project_id: Option<&str>,
    model_id: &str,
    title: &str,
    created_at: i64,
) -> Result<(), ChildAgentSpawnError> {
    connection
        .execute(
            "INSERT INTO conversations (
                 id, project_id, model_id, title, created_at, updated_at,
                 pinned_at, archived_at, unread_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?5, NULL, NULL, NULL)",
            params![conversation_id, project_id, model_id, title, created_at],
        )
        .map_err(spawn_database_error)?;
    Ok(())
}

fn collaboration_identity(
    parent: &crate::AgentNodeRecord,
    agent: &crate::AgentNodeRecord,
    task_message: &crate::AgentMailboxMessageRecord,
) -> Result<AgentCollaborationIdentity, ChildAgentSpawnError> {
    if agent.parent_agent_id.as_deref() != Some(parent.agent_id.as_str()) {
        return Err(ChildAgentSpawnError::CorruptRecord(
            "child identity does not point to its creating parent".to_string(),
        ));
    }
    let identity = AgentCollaborationIdentity {
        agent_id: agent.agent_id.clone(),
        root_agent_id: agent.root_agent_id.clone(),
        root_conversation_id: agent.root_conversation_id.clone(),
        parent_agent_id: parent.agent_id.clone(),
        parent_task_name: parent.task_name.clone(),
        parent_task_path: parent.task_path.clone(),
        conversation_id: agent.conversation_id.clone(),
        task_name: agent.task_name.clone(),
        task_path: agent.task_path.clone(),
        source_agent_message_id: task_message.message_id.clone(),
        entrusted_task: task_message.content.clone(),
        template_instructions: agent
            .template_snapshot
            .as_ref()
            .map(|template| template.instructions.clone()),
    };
    identity.validate()?;
    Ok(identity)
}

fn ensure_idempotent_spawn(
    existing: &ChildAgentSpawnRecord,
    input: &CreateChildAgentInput,
) -> Result<(), ChildAgentSpawnError> {
    let expected_template_key = input.template_machine_key.as_deref();
    let stored_template_key = existing
        .agent
        .template_snapshot
        .as_ref()
        .map(|template| template.machine_key.as_str());
    let selector_matches = match (
        input.explicit_model_id.as_deref(),
        input.template_machine_key.as_deref(),
    ) {
        (Some(model_id), _) => {
            existing.model_selection_source == AgentModelSelectionSource::Explicit
                && existing
                    .agent
                    .model_snapshot
                    .as_ref()
                    .map(|model| model.model_config_id.as_str())
                    == Some(model_id)
        }
        (None, Some(_)) => existing.model_selection_source == AgentModelSelectionSource::Template,
        (None, None) => matches!(
            existing.model_selection_source,
            AgentModelSelectionSource::Parent | AgentModelSelectionSource::Default
        ),
    };
    if existing.agent.parent_agent_id.as_deref() != Some(input.parent_agent_id.as_str())
        || existing.agent.creation_request_id != input.creation_request_id
        || existing.agent.task_name != input.task_name
        || existing.task_message.content != input.task
        || !selector_matches
        || stored_template_key != expected_template_key
        || existing.agent.reasoning_effort_snapshot != input.reasoning_effort
    {
        return Err(ChildAgentSpawnError::IdempotencyConflict(
            "request facts differ from the first committed spawn".to_string(),
        ));
    }
    Ok(())
}

// Kept behind a narrow pair of functions so the logical-turn snapshot implementation can evolve
// independently from child identity/model persistence. The full repository implementation is
// wired here in the same transaction by the context-snapshot change set.
fn ensure_existing_child_snapshot_selector(
    connection: &rusqlite::Connection,
    target_conversation_id: &str,
    fork_turns: AgentForkTurns,
) -> Result<(), ChildAgentSpawnError> {
    let stored = child_context_snapshot_repository::child_context_snapshot_fork_turns(
        connection,
        target_conversation_id,
    )
    .map_err(|error| ChildAgentSpawnError::SnapshotUnavailable(error.to_string()))?;
    if stored == Some(fork_turns) {
        return Ok(());
    }
    Err(ChildAgentSpawnError::IdempotencyConflict(
        "fork_turns differs from the first committed spawn".to_string(),
    ))
}

fn map_graph_error(error: AgentGraphError) -> ChildAgentSpawnError {
    match error {
        AgentGraphError::InvalidInput { field, reason } => {
            ChildAgentSpawnError::InvalidInput { field, reason }
        }
        AgentGraphError::AgentNotFound(id) => ChildAgentSpawnError::ParentNotFound(id),
        AgentGraphError::Conflict(reason) => ChildAgentSpawnError::Conflict(reason),
        AgentGraphError::CorruptRecord(reason) => ChildAgentSpawnError::CorruptRecord(reason),
        AgentGraphError::StorageUnavailable(reason) => {
            ChildAgentSpawnError::StorageUnavailable(reason)
        }
        other => ChildAgentSpawnError::StorageUnavailable(other.to_string()),
    }
}

fn map_spawn_lookup_error(error: AgentGraphError) -> ChildAgentSpawnError {
    match error {
        AgentGraphError::Conflict(reason) => ChildAgentSpawnError::IdempotencyConflict(reason),
        other => map_graph_error(other),
    }
}

fn map_template_error(error: AgentTemplateError) -> ChildAgentSpawnError {
    match error {
        AgentTemplateError::TemplateNotFound(key) => ChildAgentSpawnError::TemplateNotFound(key),
        AgentTemplateError::TemplateDisabled(key) => ChildAgentSpawnError::TemplateDisabled(key),
        AgentTemplateError::InvalidInput { field, reason } => {
            ChildAgentSpawnError::InvalidInput { field, reason }
        }
        AgentTemplateError::CorruptRecord(reason) => ChildAgentSpawnError::CorruptRecord(reason),
        AgentTemplateError::StorageUnavailable(reason) => {
            ChildAgentSpawnError::StorageUnavailable(reason)
        }
        other => ChildAgentSpawnError::StorageUnavailable(other.to_string()),
    }
}

fn spawn_database_error(error: rusqlite::Error) -> ChildAgentSpawnError {
    ChildAgentSpawnError::StorageUnavailable(format!("database operation failed: {error}"))
}

fn invalid_spawn(field: &'static str, reason: impl Into<String>) -> ChildAgentSpawnError {
    ChildAgentSpawnError::InvalidInput {
        field,
        reason: reason.into(),
    }
}

fn new_spawn_id(prefix: &str) -> String {
    format!("{prefix}-{}", Uuid::new_v4())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::models::{
        AttachmentRecord, ChatConversationMetaRecord, ChatConversationRecord, ChatMessageRecord,
        ModelConfigRecord, ModelSettingsRecord, ProjectRecord,
    };
    use crate::{
        AgentMailboxDeliveryStatus, AgentWakeStatus, ConversationTurnTrace,
        ConversationTurnTraceTerminalStatus, CreateAgentTemplateInput, EnsureRootAgentInput,
        ProviderProfileConfig, ProviderProtocolDialect, ReasoningEffort,
        CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
    };
    use tempfile::TempDir;

    struct Fixture {
        _directory: TempDir,
        service: StorageService,
    }

    impl Fixture {
        fn new(root_model_id: Option<&str>) -> Self {
            let directory = tempfile::tempdir().unwrap();
            let service = StorageService::open(&directory.path().join("storage.sqlite")).unwrap();
            service
                .save_project(ProjectRecord {
                    id: "project-a".to_string(),
                    name: "Project A".to_string(),
                    path: None,
                    created_at: 1,
                    pinned_at: None,
                })
                .unwrap();
            service
                .save_model_settings(model_settings(vec![
                    model("model-a", true),
                    model("model-b", true),
                    model("model-c", true),
                ]))
                .unwrap();
            service
                .save_conversation_meta(ChatConversationMetaRecord {
                    id: "root-conversation".to_string(),
                    project_id: Some("project-a".to_string()),
                    model_id: root_model_id.map(ToString::to_string),
                    title: "Root".to_string(),
                    created_at: 1,
                    updated_at: 1,
                    pinned_at: None,
                    archived_at: None,
                    unread_at: None,
                })
                .unwrap();
            service
                .ensure_root_agent(&EnsureRootAgentInput {
                    agent_id: "agent-root".to_string(),
                    conversation_id: "root-conversation".to_string(),
                    creation_request_id: "ensure-root".to_string(),
                    task_name: "Root".to_string(),
                })
                .unwrap();
            service
                .create_agent_template(&CreateAgentTemplateInput {
                    template_id: "template-reviewer".to_string(),
                    project_id: "project-a".to_string(),
                    machine_key: "reviewer".to_string(),
                    name: "Reviewer".to_string(),
                    description: "Review evidence".to_string(),
                    instructions: "Review and report to the parent.".to_string(),
                    model_config_id: "model-b".to_string(),
                    enabled: true,
                })
                .unwrap();
            Self {
                _directory: directory,
                service,
            }
        }
    }

    fn model(id: &str, enabled: bool) -> ModelConfigRecord {
        ModelConfigRecord {
            id: id.to_string(),
            display_name: format!("Display {id}"),
            api_url_override: None,
            api_token_override: None,
            supports_image: false,
            context_window_tokens: Some(64_000),
            provider_profile_config: ProviderProfileConfig::generic_for_dialect(
                ProviderProtocolDialect::OpenAiChatCompletions,
            ),
            input_price: "0".to_string(),
            cached_input_price: String::new(),
            output_price: "0".to_string(),
            enabled,
        }
    }

    fn deepseek_model(
        id: &str,
        enabled: bool,
        mode: crate::ReasoningMode,
        effort: ReasoningEffort,
    ) -> ModelConfigRecord {
        let mut model = model(id, enabled);
        model.provider_profile_config = ProviderProfileConfig {
            schema_version: crate::PROVIDER_PROFILE_CONFIG_SCHEMA_VERSION,
            profile: crate::ProviderProfileRef::deepseek_v4_chat(),
            reasoning: crate::ReasoningPolicy { mode, effort },
        };
        model
    }

    fn model_settings(models: Vec<ModelConfigRecord>) -> ModelSettingsRecord {
        ModelSettingsRecord {
            api_url: "https://provider.example/v1/chat/completions".to_string(),
            api_token: "fixture-secret".to_string(),
            search_mode: "disabled".to_string(),
            tavily_api_key: String::new(),
            models,
        }
    }

    fn spawn_input(request_id: &str, task_name: &str) -> CreateChildAgentInput {
        CreateChildAgentInput {
            parent_agent_id: "agent-root".to_string(),
            creation_request_id: request_id.to_string(),
            task_name: task_name.to_string(),
            task: format!("Perform {task_name} and report evidence."),
            template_machine_key: None,
            explicit_model_id: None,
            reasoning_effort: None,
            fork_turns: AgentForkTurns::None,
        }
    }

    fn save_settled_history(fixture: &Fixture, turn_count: usize, active_tail: bool) {
        let mut messages = Vec::new();
        for turn in 0..turn_count {
            messages.push(ChatMessageRecord {
                id: format!("root-user-{turn}"),
                role: "user".to_string(),
                content: format!("question {turn}"),
                created_at: 10 + turn as i64 * 2,
                status: Some("sent".to_string()),
                attachments: Vec::new(),
                agent_run_json: None,
                ui_state_json: None,
            });
            messages.push(ChatMessageRecord {
                id: format!("root-assistant-{turn}"),
                role: "assistant".to_string(),
                content: format!("answer {turn}"),
                created_at: 11 + turn as i64 * 2,
                status: Some("completed".to_string()),
                attachments: Vec::new(),
                agent_run_json: Some(format!(
                    "{{\"runId\":\"root-run-{turn}\",\"status\":\"completed\",\"usage\":{{\"totalTokens\":99}}}}"
                )),
                ui_state_json: Some("{\"expanded\":true}".to_string()),
            });
        }
        if active_tail {
            messages.push(ChatMessageRecord {
                id: "root-user-active".to_string(),
                role: "user".to_string(),
                content: "must not be copied".to_string(),
                created_at: 100,
                status: Some("sent".to_string()),
                attachments: Vec::new(),
                agent_run_json: None,
                ui_state_json: None,
            });
            messages.push(ChatMessageRecord {
                id: "root-assistant-active".to_string(),
                role: "assistant".to_string(),
                content: String::new(),
                created_at: 101,
                status: Some("pending".to_string()),
                attachments: Vec::new(),
                agent_run_json: Some(
                    "{\"runId\":\"root-run-active\",\"status\":\"running\"}".to_string(),
                ),
                ui_state_json: None,
            });
        }
        fixture
            .service
            .save_conversation(ChatConversationRecord {
                id: "root-conversation".to_string(),
                project_id: Some("project-a".to_string()),
                model_id: Some("model-a".to_string()),
                title: "Root".to_string(),
                messages,
                created_at: 1,
                updated_at: 101,
                pinned_at: None,
                archived_at: None,
                unread_at: None,
            })
            .unwrap();
        for turn in 0..turn_count {
            fixture
                .service
                .replace_conversation_turn_trace(
                    &ConversationTurnTrace {
                        schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
                        run_id: format!("root-run-{turn}"),
                        conversation_id: "root-conversation".to_string(),
                        assistant_message_id: format!("root-assistant-{turn}"),
                        terminal_status: ConversationTurnTraceTerminalStatus::Completed,
                        terminal_error: None,
                        truncated: false,
                        items: Vec::new(),
                    },
                    11 + turn as i64 * 2,
                    12 + turn as i64 * 2,
                )
                .unwrap();
        }
        if active_tail {
            fixture
                .service
                .append_in_progress_conversation_turn_trace(
                    &ConversationTurnTrace {
                        schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
                        run_id: "root-run-active".to_string(),
                        conversation_id: "root-conversation".to_string(),
                        assistant_message_id: "root-assistant-active".to_string(),
                        terminal_status: ConversationTurnTraceTerminalStatus::InProgress,
                        terminal_error: None,
                        truncated: false,
                        items: Vec::new(),
                    },
                    101,
                    101,
                )
                .unwrap();
        }
    }

    fn attach_file_to_first_user_message(
        fixture: &Fixture,
        suffix: &str,
        write_source_file: bool,
    ) -> AttachmentRecord {
        let message_id = "root-user-0";
        let attachment_id = format!("attachment-{suffix}");
        let bytes = format!("attachment bytes for {suffix}").into_bytes();
        let relative_path = attachment_storage_rel_path(
            "root-conversation",
            message_id,
            &attachment_id,
            "evidence.txt",
        );
        let attachment = AttachmentRecord {
            id: attachment_id,
            conversation_id: "root-conversation".to_string(),
            message_id: message_id.to_string(),
            project_id: Some("project-a".to_string()),
            kind: "file".to_string(),
            original_name: "evidence.txt".to_string(),
            mime_type: Some("text/plain".to_string()),
            size_bytes: bytes.len() as u64,
            storage_rel_path: slash_path(&relative_path),
            created_at: 10,
        };
        {
            let connection = fixture.service.state.connection().unwrap();
            attachment_repository::save_attachment(&connection, &attachment).unwrap();
        }
        if write_source_file {
            let path = fixture.service.attachment_root.join(&relative_path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, bytes).unwrap();
        }
        attachment
    }

    fn regular_files_below(path: &Path) -> Vec<PathBuf> {
        let mut files = Vec::new();
        let Ok(entries) = fs::read_dir(path) else {
            return files;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                files.extend(regular_files_below(&path));
            } else if path.is_file() {
                files.push(path);
            }
        }
        files.sort();
        files
    }

    #[test]
    fn snapshot_none_all_and_last_use_complete_settled_turns_and_exclude_active_tail() {
        let fixture = Fixture::new(Some("model-a"));
        save_settled_history(&fixture, 3, true);

        for (request, task, selector, expected_history) in [
            ("snapshot-none", "none", AgentForkTurns::None, 0usize),
            ("snapshot-all", "all", AgentForkTurns::All, 6usize),
            ("snapshot-last", "last", AgentForkTurns::Last(2), 4usize),
            ("snapshot-over", "over", AgentForkTurns::Last(99), 6usize),
        ] {
            let mut input = spawn_input(request, task);
            input.fork_turns = selector;
            let child = fixture.service.create_child_agent(&input).unwrap();
            let conversation = fixture
                .service
                .load_conversation(&child.agent.conversation_id)
                .unwrap()
                .unwrap();
            assert_eq!(conversation.messages.len(), expected_history + 1);
            assert_eq!(conversation.messages.last().unwrap().content, input.task);
            assert!(conversation
                .messages
                .iter()
                .all(|message| message.id != "root-user-active"
                    && message.content != "must not be copied"));
            for historical in conversation.messages.iter().take(expected_history) {
                assert!(historical.ui_state_json.is_none());
                if let Some(run) = historical.agent_run_json.as_deref() {
                    assert_eq!(
                        serde_json::from_str::<serde_json::Value>(run).unwrap()["usage"]
                            ["totalTokens"],
                        0
                    );
                }
            }
        }
    }

    #[test]
    fn grandchild_snapshot_flattens_historical_actor_provenance_without_reusing_mailbox_fk() {
        let fixture = Fixture::new(Some("model-a"));
        save_settled_history(&fixture, 1, false);
        let mut child_input = spawn_input("actor-child", "actor_child");
        child_input.fork_turns = AgentForkTurns::All;
        let child = fixture.service.create_child_agent(&child_input).unwrap();
        let reply_id = "child-assistant-reply";
        {
            let connection = fixture.service.state.connection().unwrap();
            connection
                .execute(
                    "INSERT INTO messages (
                         id, conversation_id, role, content, status, created_at, position
                     ) VALUES (?1, ?2, 'assistant', 'child completed task', 'completed', 200,
                               (SELECT COALESCE(MAX(position), -1) + 1 FROM messages
                                WHERE conversation_id = ?2))",
                    rusqlite::params![reply_id, &child.agent.conversation_id],
                )
                .unwrap();
            conversation_trace_repository::commit_trace_in_connection(
                &connection,
                &crate::completed_conversation_trace_without_items(
                    "child-run",
                    &child.agent.conversation_id,
                    reply_id,
                ),
                200,
                200,
            )
            .unwrap();
        }
        let child_conversation = fixture
            .service
            .load_conversation(&child.agent.conversation_id)
            .unwrap()
            .unwrap();
        let child_root_user = child_conversation
            .messages
            .iter()
            .find(|message| message.content == "question 0")
            .unwrap()
            .id
            .clone();

        let mut grandchild_input = spawn_input("actor-grandchild", "actor_grandchild");
        grandchild_input.parent_agent_id = child.agent.agent_id.clone();
        grandchild_input.fork_turns = AgentForkTurns::All;
        let grandchild = fixture
            .service
            .create_child_agent(&grandchild_input)
            .unwrap();
        let grandchild_conversation = fixture
            .service
            .load_conversation(&grandchild.agent.conversation_id)
            .unwrap()
            .unwrap();
        let grandchild_root_user = grandchild_conversation
            .messages
            .iter()
            .find(|message| message.content == "question 0")
            .unwrap();
        assert_eq!(
            fixture
                .service
                .conversation_message_origin(
                    &grandchild.agent.conversation_id,
                    &grandchild_root_user.id,
                )
                .unwrap(),
            crate::ConversationMessageOrigin::HistoricalSnapshot {
                source_conversation_id: child.agent.conversation_id.clone(),
                source_message_id: child_root_user,
                original: Box::new(crate::ConversationMessageOrigin::Human),
            }
        );
        let grandchild_parent_task = grandchild_conversation
            .messages
            .iter()
            .find(|message| message.content == child_input.task)
            .unwrap();
        assert_eq!(
            fixture
                .service
                .conversation_message_origin(
                    &grandchild.agent.conversation_id,
                    &grandchild_parent_task.id,
                )
                .unwrap(),
            crate::ConversationMessageOrigin::HistoricalSnapshot {
                source_conversation_id: child.agent.conversation_id.clone(),
                source_message_id: child.task_message.projection_message_id.clone(),
                original: Box::new(crate::ConversationMessageOrigin::Agent {
                    sender_agent_id: "agent-root".to_string(),
                    source_agent_message_id: child.task_message.message_id.clone(),
                }),
            }
        );
        let connection = fixture.service.state.connection().unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT source_agent_message_id FROM messages WHERE id = ?1",
                    [&grandchild_parent_task.id],
                    |row| row.get::<_, Option<String>>(0),
                )
                .unwrap(),
            None
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM messages WHERE source_agent_message_id = ?1",
                    [&child.task_message.message_id],
                    |row| row.get::<_, u64>(0),
                )
                .unwrap(),
            1
        );
    }

    #[test]
    fn all_snapshot_copies_attachment_to_independent_path_and_retry_is_idempotent() {
        let fixture = Fixture::new(Some("model-a"));
        save_settled_history(&fixture, 1, false);
        let source_attachment = attach_file_to_first_user_message(&fixture, "all", true);
        let mut input = spawn_input("spawn-all-attachment", "all_attachment");
        input.fork_turns = AgentForkTurns::All;
        let created = fixture.service.create_child_agent(&input).unwrap();

        let connection = fixture.service.state.connection().unwrap();
        let target_attachment = attachment_repository::list_conversation_attachments(
            &connection,
            &created.agent.conversation_id,
        )
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
        assert_ne!(target_attachment.id, source_attachment.id);
        assert_ne!(
            target_attachment.storage_rel_path,
            source_attachment.storage_rel_path
        );
        assert_eq!(
            fs::read(
                fixture
                    .service
                    .attachment_root
                    .join(&target_attachment.storage_rel_path)
            )
            .unwrap(),
            fs::read(
                fixture
                    .service
                    .attachment_root
                    .join(&source_attachment.storage_rel_path)
            )
            .unwrap()
        );
        drop(connection);

        assert_eq!(fixture.service.create_child_agent(&input).unwrap(), created);
        let connection = fixture.service.state.connection().unwrap();
        assert_eq!(
            attachment_repository::list_conversation_attachments(
                &connection,
                &created.agent.conversation_id,
            )
            .unwrap()
            .len(),
            1
        );
    }

    #[test]
    fn missing_snapshot_attachment_rolls_back_all_database_facts_without_files() {
        let fixture = Fixture::new(Some("model-a"));
        save_settled_history(&fixture, 1, false);
        attach_file_to_first_user_message(&fixture, "missing", false);
        let mut input = spawn_input("spawn-missing-attachment", "missing_attachment");
        input.fork_turns = AgentForkTurns::All;
        assert!(matches!(
            fixture.service.create_child_agent(&input),
            Err(ChildAgentSpawnError::SnapshotUnavailable(_))
        ));
        let connection = fixture.service.state.connection().unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM agent_nodes WHERE parent_agent_id IS NOT NULL",
                    [],
                    |row| row.get::<_, u64>(0),
                )
                .unwrap(),
            0
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM conversations WHERE id != 'root-conversation'",
                    [],
                    |row| row.get::<_, u64>(0),
                )
                .unwrap(),
            0
        );
        drop(connection);
        assert!(regular_files_below(&fixture.service.attachment_root).is_empty());
    }

    #[test]
    fn database_failure_after_attachment_publication_removes_copied_file() {
        let fixture = Fixture::new(Some("model-a"));
        save_settled_history(&fixture, 1, false);
        let source = attach_file_to_first_user_message(&fixture, "rollback-file", true);
        {
            let connection = fixture.service.state.connection().unwrap();
            connection
                .execute_batch(
                    "CREATE TEMP TRIGGER fail_child_wake_after_attachment
                     BEFORE INSERT ON agent_wake_requests
                     BEGIN
                         SELECT RAISE(ABORT, 'injected wake failure after attachment');
                     END;",
                )
                .unwrap();
        }
        let mut input = spawn_input("spawn-file-rollback", "file_rollback");
        input.fork_turns = AgentForkTurns::All;
        assert!(matches!(
            fixture.service.create_child_agent(&input),
            Err(ChildAgentSpawnError::StorageUnavailable(_))
        ));
        assert_eq!(
            regular_files_below(&fixture.service.attachment_root),
            vec![fixture
                .service
                .attachment_root
                .join(source.storage_rel_path)]
        );
        let connection = fixture.service.state.connection().unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM agent_nodes WHERE parent_agent_id IS NOT NULL",
                    [],
                    |row| row.get::<_, u64>(0),
                )
                .unwrap(),
            0
        );
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM child_context_snapshots", [], |row| {
                    row.get::<_, u64>(0)
                })
                .unwrap(),
            0
        );
    }

    #[test]
    fn last_zero_is_rejected_before_any_child_fact_is_written() {
        let fixture = Fixture::new(Some("model-a"));
        let mut input = spawn_input("snapshot-zero", "zero");
        input.fork_turns = AgentForkTurns::Last(0);
        assert!(matches!(
            fixture.service.create_child_agent(&input),
            Err(ChildAgentSpawnError::InvalidInput {
                field: "fork_turns",
                ..
            })
        ));
        assert!(fixture
            .service
            .list_agent_children("agent-root", "agent-root")
            .unwrap()
            .is_empty());
    }

    #[test]
    fn unexecutable_identity_names_are_rejected_before_spawn_but_payloads_remain_multiline() {
        let fixture = Fixture::new(Some("model-a"));
        let mut invalid = spawn_input("invalid-identity", "bad\nname");
        invalid.task = "line one\nline two".to_string();
        assert!(matches!(
            fixture.service.create_child_agent(&invalid),
            Err(ChildAgentSpawnError::InvalidInput {
                field: "task_name",
                ..
            })
        ));
        assert!(fixture
            .service
            .list_agent_children("agent-root", "agent-root")
            .unwrap()
            .is_empty());

        let mut multiline = spawn_input("multiline-task", "multiline_task");
        multiline.task = "line one\nline two".to_string();
        assert_eq!(
            fixture
                .service
                .create_child_agent(&multiline)
                .unwrap()
                .collaboration_identity
                .entrusted_task,
            multiline.task
        );
    }

    #[test]
    fn atomic_spawn_persists_independent_conversation_task_projection_and_queued_wake() {
        let fixture = Fixture::new(Some("model-a"));
        let input = spawn_input("spawn-1", "security_review");
        let created = fixture.service.create_child_agent(&input).unwrap();

        assert_eq!(created.agent.parent_agent_id.as_deref(), Some("agent-root"));
        assert_ne!(created.agent.conversation_id, "root-conversation");
        assert_eq!(
            created.model_selection_source,
            AgentModelSelectionSource::Parent
        );
        assert_eq!(
            created
                .agent
                .model_snapshot
                .as_ref()
                .unwrap()
                .model_config_id,
            "model-a"
        );
        assert_eq!(
            created.task_message.delivery_status,
            AgentMailboxDeliveryStatus::Acknowledged
        );
        assert_eq!(created.initial_wake.status, AgentWakeStatus::Queued);
        assert_eq!(
            fixture
                .service
                .conversation_message_origin(
                    &created.agent.conversation_id,
                    &created.task_message.projection_message_id,
                )
                .unwrap(),
            crate::ConversationMessageOrigin::Agent {
                sender_agent_id: "agent-root".to_string(),
                source_agent_message_id: created.task_message.message_id.clone(),
            }
        );
        let trusted = fixture
            .service
            .resolve_child_agent_wake(
                &created.agent.agent_id,
                &created.initial_wake.wake_id,
                &created.task_message.message_id,
            )
            .unwrap();
        assert_eq!(trusted.collaboration_identity.entrusted_task, input.task);

        let retry = fixture.service.create_child_agent(&input).unwrap();
        assert_eq!(retry, created);
        let claim_token = "controlled-turn-claim";
        let claimed = fixture
            .service
            .claim_next_agent_wake(&created.agent.agent_id, claim_token)
            .unwrap()
            .unwrap();
        assert_eq!(claimed.wake_id, created.initial_wake.wake_id);
        fixture
            .service
            .transition_agent_wake(
                &claimed.wake_id,
                AgentWakeStatus::Claimed,
                AgentWakeStatus::Running,
                Some(claim_token),
            )
            .unwrap();
        assert!(matches!(
            fixture.service.resolve_running_child_agent_wake(
                &created.agent.agent_id,
                &created.initial_wake.wake_id,
                &created.task_message.message_id,
                "wrong-claim-token",
            ),
            Err(AgentGraphError::Conflict(_))
        ));
        let authorized = fixture
            .service
            .resolve_running_child_agent_wake(
                &created.agent.agent_id,
                &created.initial_wake.wake_id,
                &created.task_message.message_id,
                claim_token,
            )
            .unwrap();
        assert_eq!(authorized.initial_wake.status, AgentWakeStatus::Running);
        let recovered = fixture
            .service
            .resolve_active_child_agent_wake_by_identity(&authorized.collaboration_identity)
            .unwrap();
        assert_eq!(recovered.claim_token, claim_token);
        assert_eq!(recovered.spawn, authorized);
        fixture
            .service
            .transition_agent_wake(
                &created.initial_wake.wake_id,
                AgentWakeStatus::Running,
                AgentWakeStatus::WaitingForApproval,
                Some(claim_token),
            )
            .unwrap();
        assert_eq!(
            fixture
                .service
                .resolve_active_child_agent_wake_by_identity(&authorized.collaboration_identity)
                .unwrap()
                .spawn
                .initial_wake
                .status,
            AgentWakeStatus::WaitingForApproval
        );
        let mut forged_identity = authorized.collaboration_identity;
        forged_identity.entrusted_task.push_str(" forged");
        assert!(matches!(
            fixture
                .service
                .resolve_active_child_agent_wake_by_identity(&forged_identity),
            Err(AgentGraphError::Conflict(_))
        ));
        let connection = fixture.service.state.connection().unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM agent_nodes WHERE parent_agent_id = 'agent-root'",
                    [],
                    |row| row.get::<_, u64>(0),
                )
                .unwrap(),
            1
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM messages WHERE conversation_id = ?1",
                    [&created.agent.conversation_id],
                    |row| row.get::<_, u64>(0),
                )
                .unwrap(),
            1
        );
    }

    #[test]
    fn request_reuse_with_changed_task_or_selector_is_rejected() {
        let fixture = Fixture::new(Some("model-a"));
        let input = spawn_input("spawn-conflict", "review");
        fixture.service.create_child_agent(&input).unwrap();

        let mut changed_task = input.clone();
        changed_task.task.push_str(" changed");
        assert!(matches!(
            fixture.service.create_child_agent(&changed_task),
            Err(ChildAgentSpawnError::IdempotencyConflict(_))
        ));
        let mut changed_selector = input.clone();
        changed_selector.explicit_model_id = Some("model-c".to_string());
        assert!(matches!(
            fixture.service.create_child_agent(&changed_selector),
            Err(ChildAgentSpawnError::IdempotencyConflict(_))
        ));
        let mut changed_reasoning = input;
        changed_reasoning.reasoning_effort = Some(ReasoningEffort::High);
        assert!(matches!(
            fixture.service.create_child_agent(&changed_reasoning),
            Err(ChildAgentSpawnError::IdempotencyConflict(_))
        ));
    }

    #[test]
    fn creation_request_reuse_under_another_parent_is_typed_idempotency_conflict() {
        let fixture = Fixture::new(Some("model-a"));
        let first = fixture
            .service
            .create_child_agent(&spawn_input("shared-request", "first_child"))
            .unwrap();
        let sibling = fixture
            .service
            .create_child_agent(&spawn_input("sibling-request", "second_child"))
            .unwrap();
        let mut reused = spawn_input("shared-request", "grandchild");
        reused.parent_agent_id = sibling.agent.agent_id.clone();
        assert!(matches!(
            fixture.service.create_child_agent(&reused),
            Err(ChildAgentSpawnError::IdempotencyConflict(_))
        ));
        let tree = fixture.service.list_agent_tree("agent-root").unwrap();
        assert_eq!(tree.len(), 3);
        assert_eq!(
            tree.iter()
                .filter(|node| node.parent_agent_id.as_deref() == Some(&sibling.agent.agent_id))
                .count(),
            0
        );
        assert_eq!(
            first.agent.creation_request_id,
            "shared-request".to_string()
        );
    }

    #[test]
    fn retry_uses_frozen_bundle_after_template_and_model_catalog_change() {
        let fixture = Fixture::new(Some("model-a"));
        let mut input = spawn_input("spawn-frozen-retry", "frozen_review");
        input.template_machine_key = Some("reviewer".to_string());
        let created = fixture.service.create_child_agent(&input).unwrap();
        let template = fixture
            .service
            .get_agent_template("project-a", "template-reviewer")
            .unwrap();
        fixture
            .service
            .set_agent_template_enabled("project-a", "template-reviewer", template.revision, false)
            .unwrap();
        fixture
            .service
            .save_model_settings(model_settings(vec![model("model-a", true)]))
            .unwrap();

        let retry = fixture.service.create_child_agent(&input).unwrap();
        assert_eq!(retry, created);
        assert_eq!(
            retry.agent.model_snapshot.unwrap().model_config_id,
            "model-b"
        );
    }

    #[test]
    fn model_priority_is_explicit_then_template_then_parent_then_ordered_default() {
        let fixture = Fixture::new(Some("model-a"));
        let mut explicit = spawn_input("spawn-explicit", "explicit_review");
        explicit.template_machine_key = Some("reviewer".to_string());
        explicit.explicit_model_id = Some("model-c".to_string());
        let explicit = fixture.service.create_child_agent(&explicit).unwrap();
        assert_eq!(
            explicit.model_selection_source,
            AgentModelSelectionSource::Explicit
        );
        assert_eq!(
            explicit.agent.model_snapshot.unwrap().model_config_id,
            "model-c"
        );
        assert_eq!(
            explicit.agent.template_snapshot.unwrap().machine_key,
            "reviewer"
        );

        let mut template = spawn_input("spawn-template", "template_review");
        template.template_machine_key = Some("reviewer".to_string());
        let template = fixture.service.create_child_agent(&template).unwrap();
        assert_eq!(
            template.model_selection_source,
            AgentModelSelectionSource::Template
        );
        assert_eq!(
            template.agent.model_snapshot.unwrap().model_config_id,
            "model-b"
        );

        let parent = fixture
            .service
            .create_child_agent(&spawn_input("spawn-parent", "parent_review"))
            .unwrap();
        assert_eq!(
            parent.model_selection_source,
            AgentModelSelectionSource::Parent
        );
        assert_eq!(
            parent.agent.model_snapshot.unwrap().model_config_id,
            "model-a"
        );

        let default_fixture = Fixture::new(None);
        let default = default_fixture
            .service
            .create_child_agent(&spawn_input("spawn-default", "default_review"))
            .unwrap();
        assert_eq!(
            default.model_selection_source,
            AgentModelSelectionSource::Default
        );
        assert_eq!(
            default.agent.model_snapshot.unwrap().model_config_id,
            "model-a"
        );
    }

    #[test]
    fn exact_unavailable_model_and_unsupported_reasoning_never_fall_back() {
        let fixture = Fixture::new(Some("model-a"));
        let mut missing = spawn_input("spawn-missing", "missing_model");
        missing.explicit_model_id = Some("does-not-exist".to_string());
        assert_eq!(
            fixture.service.create_child_agent(&missing).unwrap_err(),
            ChildAgentSpawnError::ModelUnavailable {
                model_config_id: Some("does-not-exist".to_string()),
                reason: crate::AgentModelUnavailableReason::NotFound,
            }
        );

        let mut reasoning = spawn_input("spawn-reasoning", "reasoning_override");
        reasoning.reasoning_effort = Some(ReasoningEffort::High);
        assert_eq!(
            fixture.service.create_child_agent(&reasoning).unwrap_err(),
            ChildAgentSpawnError::UnsupportedReasoningEffort(ReasoningEffort::High)
        );
        assert!(fixture
            .service
            .list_agent_children("agent-root", "agent-root")
            .unwrap()
            .is_empty());
    }

    #[test]
    fn exact_enabled_deepseek_reasoning_is_frozen_for_high_and_max() {
        for (index, effort) in [ReasoningEffort::High, ReasoningEffort::Max]
            .into_iter()
            .enumerate()
        {
            let fixture = Fixture::new(Some("model-a"));
            fixture
                .service
                .save_model_settings(model_settings(vec![deepseek_model(
                    "model-a",
                    true,
                    crate::ReasoningMode::Enabled,
                    effort,
                )]))
                .unwrap();
            let mut input = spawn_input(
                &format!("spawn-deepseek-reasoning-{index}"),
                &format!("deepseek_reasoning_{index}"),
            );
            input.reasoning_effort = Some(effort);
            let spawn = fixture.service.create_child_agent(&input).unwrap();
            assert_eq!(spawn.agent.reasoning_effort_snapshot, Some(effort));
        }
    }

    #[test]
    fn provider_default_disabled_and_mismatched_reasoning_are_rejected() {
        for (index, mode, configured, requested) in [
            (
                0,
                crate::ReasoningMode::Enabled,
                ReasoningEffort::High,
                ReasoningEffort::ProviderDefault,
            ),
            (
                1,
                crate::ReasoningMode::Disabled,
                ReasoningEffort::ProviderDefault,
                ReasoningEffort::High,
            ),
            (
                2,
                crate::ReasoningMode::Enabled,
                ReasoningEffort::High,
                ReasoningEffort::Max,
            ),
        ] {
            let fixture = Fixture::new(Some("model-a"));
            fixture
                .service
                .save_model_settings(model_settings(vec![deepseek_model(
                    "model-a", true, mode, configured,
                )]))
                .unwrap();
            let mut input = spawn_input(
                &format!("spawn-rejected-reasoning-{index}"),
                &format!("rejected_reasoning_{index}"),
            );
            input.reasoning_effort = Some(requested);
            assert_eq!(
                fixture.service.create_child_agent(&input).unwrap_err(),
                ChildAgentSpawnError::UnsupportedReasoningEffort(requested)
            );
        }
    }

    #[test]
    fn reasoning_retry_is_frozen_but_wake_fails_closed_after_profile_drift() {
        let fixture = Fixture::new(Some("model-a"));
        fixture
            .service
            .save_model_settings(model_settings(vec![deepseek_model(
                "model-a",
                true,
                crate::ReasoningMode::Enabled,
                ReasoningEffort::High,
            )]))
            .unwrap();
        let mut input = spawn_input("spawn-reasoning-drift", "reasoning_drift");
        input.reasoning_effort = Some(ReasoningEffort::High);
        let created = fixture.service.create_child_agent(&input).unwrap();

        fixture
            .service
            .save_model_settings(model_settings(vec![deepseek_model(
                "model-a",
                true,
                crate::ReasoningMode::Enabled,
                ReasoningEffort::Max,
            )]))
            .unwrap();
        assert_eq!(fixture.service.create_child_agent(&input).unwrap(), created);

        let claim_token = "reasoning-drift-claim";
        fixture
            .service
            .claim_next_agent_wake(&created.agent.agent_id, claim_token)
            .unwrap()
            .unwrap();
        fixture
            .service
            .transition_agent_wake(
                &created.initial_wake.wake_id,
                AgentWakeStatus::Claimed,
                AgentWakeStatus::Running,
                Some(claim_token),
            )
            .unwrap();
        assert!(matches!(
            fixture.service.resolve_running_child_agent_wake(
                &created.agent.agent_id,
                &created.initial_wake.wake_id,
                &created.task_message.message_id,
                claim_token,
            ),
            Err(AgentGraphError::Conflict(reason))
                if reason.contains("reasoning constraint")
        ));
    }

    #[test]
    fn failure_after_projection_rolls_back_every_spawn_fact() {
        let fixture = Fixture::new(Some("model-a"));
        {
            let connection = fixture.service.state.connection().unwrap();
            connection
                .execute_batch(
                    "CREATE TEMP TRIGGER fail_child_initial_wake
                     BEFORE INSERT ON agent_wake_requests
                     BEGIN
                         SELECT RAISE(ABORT, 'injected child wake failure');
                     END;",
                )
                .unwrap();
        }
        assert!(matches!(
            fixture
                .service
                .create_child_agent(&spawn_input("spawn-rollback", "rollback_review")),
            Err(ChildAgentSpawnError::StorageUnavailable(_))
        ));
        let connection = fixture.service.state.connection().unwrap();
        for table in [
            "agent_nodes",
            "agent_mailbox_messages",
            "agent_wake_requests",
        ] {
            let sql = format!("SELECT COUNT(*) FROM {table} WHERE root_agent_id = 'agent-root'");
            let count = connection
                .query_row(&sql, [], |row| row.get::<_, u64>(0))
                .unwrap();
            assert_eq!(count, u64::from(table == "agent_nodes"), "{table}");
        }
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM conversations WHERE id != 'root-conversation'",
                    [],
                    |row| row.get::<_, u64>(0),
                )
                .unwrap(),
            0
        );
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM messages", [], |row| row
                    .get::<_, u64>(0))
                .unwrap(),
            0
        );
    }

    #[test]
    fn projection_failure_rolls_back_conversation_node_and_mailbox() {
        let fixture = Fixture::new(Some("model-a"));
        {
            let connection = fixture.service.state.connection().unwrap();
            connection
                .execute_batch(
                    "CREATE TEMP TRIGGER fail_child_task_projection
                     BEFORE INSERT ON messages
                     WHEN NEW.input_origin_kind = 'agent'
                     BEGIN
                         SELECT RAISE(ABORT, 'injected child projection failure');
                     END;",
                )
                .unwrap();
        }
        assert!(matches!(
            fixture
                .service
                .create_child_agent(&spawn_input("spawn-projection-fail", "projection_failure")),
            Err(ChildAgentSpawnError::StorageUnavailable(_))
        ));
        let connection = fixture.service.state.connection().unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM conversations WHERE id != 'root-conversation'",
                    [],
                    |row| row.get::<_, u64>(0),
                )
                .unwrap(),
            0
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM agent_nodes WHERE parent_agent_id IS NOT NULL",
                    [],
                    |row| row.get::<_, u64>(0),
                )
                .unwrap(),
            0
        );
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM agent_mailbox_messages", [], |row| {
                    row.get::<_, u64>(0)
                })
                .unwrap(),
            0
        );
    }

    #[test]
    fn incomplete_parent_model_context_rejects_spawn_and_rolls_back_every_child_fact() {
        let fixture = Fixture::new(Some("model-a"));
        save_settled_history(&fixture, 1, false);
        {
            let connection = fixture.service.state.connection().unwrap();
            connection
                .execute(
                    "DELETE FROM conversation_turn_traces
                     WHERE assistant_message_id = 'root-assistant-0'",
                    [],
                )
                .unwrap();
            conversation_trace_repository::commit_trace_in_connection(
                &connection,
                &ConversationTurnTrace {
                    schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
                    run_id: "root-run-0".to_string(),
                    conversation_id: "root-conversation".to_string(),
                    assistant_message_id: "root-assistant-0".to_string(),
                    terminal_status: ConversationTurnTraceTerminalStatus::Completed,
                    terminal_error: None,
                    truncated: false,
                    items: vec![crate::ConversationTurnTraceItem::AssistantNarration {
                        sequence: 0,
                        content: "Inspecting the parent history.".to_string(),
                        truncated: false,
                    }],
                },
                11,
                12,
            )
            .unwrap();
        }

        let mut input = spawn_input("spawn-incomplete-context", "incomplete_context");
        input.fork_turns = AgentForkTurns::All;
        let error = fixture.service.create_child_agent(&input).unwrap_err();
        assert!(matches!(
            &error,
            ChildAgentSpawnError::SnapshotUnavailable(message)
                if message.contains("incomplete model context")
        ));

        let connection = fixture.service.state.connection().unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM agent_nodes WHERE parent_agent_id IS NOT NULL",
                    [],
                    |row| row.get::<_, u64>(0),
                )
                .unwrap(),
            0
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM conversations WHERE id != 'root-conversation'",
                    [],
                    |row| row.get::<_, u64>(0),
                )
                .unwrap(),
            0
        );
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM child_context_snapshots", [], |row| {
                    row.get::<_, u64>(0)
                })
                .unwrap(),
            0
        );
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM agent_mailbox_messages", [], |row| {
                    row.get::<_, u64>(0)
                })
                .unwrap(),
            0
        );
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM agent_wake_requests", [], |row| {
                    row.get::<_, u64>(0)
                })
                .unwrap(),
            0
        );
    }
}
