use super::agent_templates::resolve_exact_agent_model;
use super::conversations::cleanup_fork_files;
use super::*;
use crate::storage::child_context_snapshot_repository;
use crate::{
    AgentCollaborationIdentity, AgentForkTurns, AgentGraphError, AgentLifecycle, AgentMailboxKind,
    AgentModelSelectionSource, AgentTemplateError, AgentTemplateSnapshot, AgentTreeResourceLimits,
    ChildAgentSpawnError, ChildAgentSpawnRecord, CreateAgentNodeInput, CreateChildAgentInput,
    EnqueueAgentMessageInput, EnqueueAgentWakeInput, IdempotentCreate,
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
        self.create_child_agent_with_limits(input, AgentTreeResourceLimits::default())
    }

    pub fn create_child_agent_with_limits(
        &self,
        input: &CreateChildAgentInput,
        limits: AgentTreeResourceLimits,
    ) -> Result<ChildAgentSpawnRecord, ChildAgentSpawnError> {
        self.create_child_agent_with_limits_and_expected_model_capabilities(input, limits, None)
    }

    /// Creates a child only if the selected model still has the capability fact exposed to the
    /// caller at its sampling boundary. The comparison runs inside the same immediate transaction
    /// that resolves settings and commits the child, closing the settings-change race without
    /// adding model-editable capability input or another persistence column.
    pub fn create_child_agent_with_limits_and_expected_model_capabilities(
        &self,
        input: &CreateChildAgentInput,
        limits: AgentTreeResourceLimits,
        expected_model_capabilities: Option<crate::ModelCapabilities>,
    ) -> Result<ChildAgentSpawnRecord, ChildAgentSpawnError> {
        let limits = limits.validate()?;
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
            ensure_expected_model_capabilities(
                existing.agent.model_snapshot.as_ref(),
                expected_model_capabilities,
            )?;
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
        enforce_task_resource_limit(input, limits)?;
        enforce_tree_resource_limits(&transaction, &parent, limits)?;

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
        ensure_expected_model_capabilities(Some(&model_snapshot), expected_model_capabilities)?;
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

fn enforce_task_resource_limit(
    input: &CreateChildAgentInput,
    limits: AgentTreeResourceLimits,
) -> Result<(), ChildAgentSpawnError> {
    if input.task.len() > limits.max_task_bytes {
        return Err(ChildAgentSpawnError::ResourceLimit {
            resource: "task_bytes",
            limit: limits.max_task_bytes as u64,
        });
    }
    Ok(())
}

fn enforce_tree_resource_limits(
    transaction: &rusqlite::Transaction<'_>,
    parent: &crate::AgentNodeRecord,
    limits: AgentTreeResourceLimits,
) -> Result<(), ChildAgentSpawnError> {
    let node_count = transaction
        .query_row(
            "SELECT COUNT(*) FROM agent_nodes WHERE root_agent_id = ?1",
            [&parent.root_agent_id],
            |row| row.get::<_, u64>(0),
        )
        .map_err(spawn_database_error)?;
    if node_count >= u64::from(limits.max_nodes) {
        return Err(ChildAgentSpawnError::ResourceLimit {
            resource: "tree_nodes",
            limit: u64::from(limits.max_nodes),
        });
    }

    let parent_depth = transaction
        .query_row(
            "WITH RECURSIVE ancestors(agent_id, parent_agent_id, depth) AS (
                 SELECT agent_id, parent_agent_id, 0
                   FROM agent_nodes
                  WHERE agent_id = ?1
                 UNION ALL
                 SELECT parent.agent_id, parent.parent_agent_id, ancestors.depth + 1
                   FROM agent_nodes AS parent
                   JOIN ancestors ON parent.agent_id = ancestors.parent_agent_id
                  WHERE ancestors.depth <= 32
             )
             SELECT COALESCE(MAX(depth), 0) FROM ancestors",
            [&parent.agent_id],
            |row| row.get::<_, u32>(0),
        )
        .map_err(spawn_database_error)?;
    if parent_depth >= limits.max_depth {
        return Err(ChildAgentSpawnError::ResourceLimit {
            resource: "tree_depth",
            limit: u64::from(limits.max_depth),
        });
    }
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

fn ensure_expected_model_capabilities(
    model: Option<&crate::AgentModelSelectionSnapshot>,
    expected: Option<crate::ModelCapabilities>,
) -> Result<(), ChildAgentSpawnError> {
    let Some(expected) = expected else {
        return Ok(());
    };
    let model = model.ok_or(ChildAgentSpawnError::ModelUnavailable {
        model_config_id: None,
        reason: crate::AgentModelUnavailableReason::CapabilitiesChanged,
    })?;
    if model.supports_image != expected.image_input {
        return Err(ChildAgentSpawnError::ModelUnavailable {
            model_config_id: Some(model.model_config_id.clone()),
            reason: crate::AgentModelUnavailableReason::CapabilitiesChanged,
        });
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
        source_agent_id: parent.agent_id.clone(),
        source_kind: task_message.kind,
        source_task_name: parent.task_name.clone(),
        source_task_path: parent.task_path.clone(),
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
        ConversationForkPoint, ForkConversationRequest, ModelConfigRecord, ModelSettingsRecord,
        ProjectRecord,
    };
    use crate::{
        AgentGraphError, AgentMailboxDeliveryStatus, AgentMailboxKind, AgentWakeStatus,
        ConversationMessageOrigin, ConversationTurnTrace, ConversationTurnTraceTerminalStatus,
        CreateAgentTemplateInput, EnqueueAgentMessageInput, EnsureRootAgentInput,
        FinishAgentWakeWithResultInput, ProviderProfileConfig, ProviderProtocolDialect,
        ReasoningEffort, SendAgentMessageRequest, CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
    };
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Instant;
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

    fn attach_file_to_tree_member_message(
        fixture: &Fixture,
        conversation_id: &str,
        message_id: &str,
        attachment_id: &str,
        bytes: &[u8],
        created_at: i64,
    ) -> AttachmentRecord {
        let original_name = "member-evidence.txt";
        let relative_path =
            attachment_storage_rel_path(conversation_id, message_id, attachment_id, original_name);
        let attachment = AttachmentRecord {
            id: attachment_id.to_string(),
            conversation_id: conversation_id.to_string(),
            message_id: message_id.to_string(),
            project_id: Some("project-a".to_string()),
            kind: "file".to_string(),
            original_name: original_name.to_string(),
            mime_type: Some("text/plain".to_string()),
            size_bytes: bytes.len() as u64,
            storage_rel_path: slash_path(&relative_path),
            created_at,
        };
        {
            let connection = fixture.service.state.connection().unwrap();
            attachment_repository::save_attachment(&connection, &attachment).unwrap();
        }
        let path = fixture.service.attachment_root.join(&relative_path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
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

    #[allow(clippy::too_many_arguments)]
    fn append_terminal_assistant_for_tree_fork(
        fixture: &Fixture,
        conversation_id: &str,
        message_id: &str,
        run_id: &str,
        content: &str,
        terminal_status: ConversationTurnTraceTerminalStatus,
        terminal_error: Option<&str>,
        created_at: i64,
    ) {
        assert!(terminal_status.is_terminal());
        let status = terminal_status.as_str();
        let agent_run_json = serde_json::json!({
            "runId": run_id,
            "status": status,
        })
        .to_string();
        let connection = fixture.service.state.connection().unwrap();
        connection
            .execute(
                "INSERT INTO messages (
                     id, conversation_id, role, content, status, agent_run_json,
                     created_at, position
                 ) VALUES (
                     ?1, ?2, 'assistant', ?3, ?4, ?5, ?6,
                     (SELECT COALESCE(MAX(position), -1) + 1 FROM messages
                      WHERE conversation_id = ?2)
                 )",
                rusqlite::params![
                    message_id,
                    conversation_id,
                    content,
                    status,
                    agent_run_json,
                    created_at,
                ],
            )
            .unwrap();
        conversation_trace_repository::commit_trace_in_connection(
            &connection,
            &ConversationTurnTrace {
                schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
                run_id: run_id.to_string(),
                conversation_id: conversation_id.to_string(),
                assistant_message_id: message_id.to_string(),
                terminal_status,
                terminal_error: terminal_error.map(ToString::to_string),
                truncated: false,
                items: vec![crate::ConversationTurnTraceItem::AssistantNarration {
                    sequence: 0,
                    content: format!("trace: {content}"),
                    truncated: false,
                }],
            },
            created_at,
            created_at,
        )
        .unwrap();
    }

    fn tree_by_task_path(
        fixture: &Fixture,
        root_agent_id: &str,
    ) -> std::collections::BTreeMap<String, crate::AgentNodeRecord> {
        fixture
            .service
            .list_agent_tree(root_agent_id)
            .unwrap()
            .into_iter()
            .map(|node| (node.task_path.clone(), node))
            .collect()
    }

    fn tree_history_identity_sets(
        fixture: &Fixture,
        tree: &std::collections::BTreeMap<String, crate::AgentNodeRecord>,
    ) -> (
        std::collections::HashSet<String>,
        std::collections::HashSet<String>,
    ) {
        let mut message_ids = std::collections::HashSet::new();
        let mut run_ids = std::collections::HashSet::new();
        for node in tree.values() {
            let conversation = fixture
                .service
                .load_conversation(&node.conversation_id)
                .unwrap()
                .expect("every tree member owns a readable conversation");
            for message in conversation.messages {
                assert!(message_ids.insert(message.id.clone()));
                if let Some(agent_run_json) = message.agent_run_json.as_deref() {
                    let agent_run =
                        serde_json::from_str::<serde_json::Value>(agent_run_json).unwrap();
                    if let Some(run_id) = agent_run.get("runId").and_then(|value| value.as_str()) {
                        run_ids.insert(run_id.to_string());
                    }
                }
                if let Some(trace) = fixture
                    .service
                    .get_conversation_turn_trace(&message.id)
                    .unwrap()
                {
                    assert_eq!(trace.conversation_id, node.conversation_id);
                    run_ids.insert(trace.run_id);
                }
            }
        }
        (message_ids, run_ids)
    }

    fn assert_tree_identities_are_fresh(
        fixture: &Fixture,
        source: &std::collections::BTreeMap<String, crate::AgentNodeRecord>,
        target: &std::collections::BTreeMap<String, crate::AgentNodeRecord>,
    ) {
        assert_eq!(
            source.keys().collect::<Vec<_>>(),
            target.keys().collect::<Vec<_>>()
        );
        let source_agent_ids = source
            .values()
            .map(|node| node.agent_id.as_str())
            .collect::<std::collections::HashSet<_>>();
        let target_agent_ids = target
            .values()
            .map(|node| node.agent_id.as_str())
            .collect::<std::collections::HashSet<_>>();
        assert!(source_agent_ids.is_disjoint(&target_agent_ids));
        let source_conversation_ids = source
            .values()
            .map(|node| node.conversation_id.as_str())
            .collect::<std::collections::HashSet<_>>();
        let target_conversation_ids = target
            .values()
            .map(|node| node.conversation_id.as_str())
            .collect::<std::collections::HashSet<_>>();
        assert!(source_conversation_ids.is_disjoint(&target_conversation_ids));

        let (source_message_ids, source_run_ids) = tree_history_identity_sets(fixture, source);
        let (target_message_ids, target_run_ids) = tree_history_identity_sets(fixture, target);
        assert!(source_message_ids.is_disjoint(&target_message_ids));
        assert!(source_run_ids.is_disjoint(&target_run_ids));
    }

    fn member_fork_receipt_count(fixture: &Fixture, request_id: &str) -> u64 {
        fixture
            .service
            .state
            .connection()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM agent_member_conversation_forks
                 WHERE root_fork_request_id = ?1",
                [request_id],
                |row| row.get(0),
            )
            .unwrap()
    }

    fn turn_diff_action_ids(fixture: &Fixture, assistant_message_id: &str) -> Vec<String> {
        let connection = fixture.service.state.connection().unwrap();
        let mut statement = connection
            .prepare(
                "SELECT action_id FROM agent_turn_diff_actions
                 WHERE assistant_message_id = ?1
                 ORDER BY action_id ASC",
            )
            .unwrap();
        statement
            .query_map([assistant_message_id], |row| row.get(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
    }

    fn member_compaction_summary_draft(
        prefix: &crate::ContextCompactionPrefix,
        summary_id: &str,
        created_at: i64,
    ) -> crate::ContextCompactionSummaryDraft {
        crate::ContextCompactionSummaryDraft {
            id: summary_id.to_string(),
            source_revision: prefix.source_revision.clone(),
            content: format!("summary {summary_id}"),
            continuity: crate::ContextContinuitySnapshot::from_prefix(prefix).unwrap(),
            generation: crate::ContextCompactionGeneration::test(),
            source_input_tokens: 1_000,
            summary_input_tokens: 100,
            continuity_input_tokens: 100,
            uncovered_tail_input_tokens: 0,
            replacement_input_tokens: 200,
            created_at,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn record_applied_member_compaction(
        connection: &mut rusqlite::Connection,
        prefix: &crate::ContextCompactionPrefix,
        summary_id: &str,
        operation_id: &str,
        run_id: &str,
        assistant_message_id: &str,
        model_id: &str,
        completed_at: i64,
    ) -> crate::ContextCompactionReceipt {
        assert!(!operation_id.starts_with("provider-transition-"));
        let started_at = completed_at.saturating_sub(2);
        let mut receipt = crate::ContextCompactionReceipt {
            schema_version: crate::CONTEXT_COMPACTION_RECEIPT_SCHEMA_VERSION,
            operation_id: operation_id.to_string(),
            run_id: run_id.to_string(),
            conversation_id: prefix.conversation_id.clone(),
            assistant_message_id: assistant_message_id.to_string(),
            request_index: 1,
            attempt_index: 1,
            model: model_id.to_string(),
            provider_transition_source_model_display_name: None,
            provider_transition_target_model_display_name: None,
            api_style: crate::AgentApiStyle::OpenAiCompatible,
            status: crate::ContextCompactionReceiptStatus::InProgress,
            stage: crate::ContextCompactionReceiptStage::Planned,
            plan: crate::ContextCompactionReceiptPlan {
                context_revision: prefix.source_revision.clone(),
                persistent_revision: prefix.source_revision.clone(),
                request_input_tokens: 1_000,
                available_input_tokens: Some(1_000),
                request_trigger_input_tokens: Some(900),
                request_target_input_tokens: Some(200),
                source_input_tokens: 1_000,
                retained_input_tokens: 0,
                target_replacement_tokens: 200,
                expected_reclaimed_tokens: 800,
                planned_reclaimed_tokens: 800,
                projected_request_input_tokens: 200,
                best_effort: false,
                protected_input_tokens: 0,
                protected_reasons: Default::default(),
                atomic_unit_count: prefix.source_items.len().max(1),
                previous_summary_id: prefix
                    .previous_summary
                    .as_ref()
                    .map(|summary| summary.id.clone()),
                covered_through: prefix.covered_through.clone(),
            },
            source_revision: None,
            generation_observation_id: None,
            summary_id: None,
            result: None,
            error: None,
            started_at,
            updated_at: started_at,
            completed_at: None,
        };
        receipt.validate().unwrap();
        crate::storage::context_compaction_receipt_repository::record_receipt(
            connection, &receipt, None,
        )
        .unwrap();
        receipt
            .attach_prepared_prefix(prefix, completed_at.saturating_sub(1))
            .unwrap();
        let draft = member_compaction_summary_draft(prefix, summary_id, completed_at);
        let observation = crate::model_request_observation::ModelRequestObservationBuilder::new(
            format!("observation-{operation_id}"),
            run_id,
            Some(prefix.conversation_id.clone()),
            Some(assistant_message_id.to_string()),
            Some(operation_id.to_string()),
            1,
            crate::ModelRequestPurpose::ContextCompaction,
            model_id,
            crate::AgentApiStyle::OpenAiCompatible,
            None,
            completed_at.saturating_sub(1),
        )
        .completed(None, Some("stop".to_string()), completed_at)
        .unwrap();
        receipt
            .complete_applied(&draft, &observation, completed_at)
            .unwrap();
        crate::storage::context_compaction_repository::commit_prefix_replacement_with_receipt(
            connection,
            prefix,
            draft,
            &receipt,
            &observation,
        )
        .unwrap();
        receipt
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
                    let run = serde_json::from_str::<serde_json::Value>(run).unwrap();
                    assert!(
                        run.get("usage").is_none()
                            || run["usage"]["totalTokens"].as_u64() == Some(0),
                        "a context snapshot must not copy parent usage"
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
    fn observer_snapshot_bulk_loads_large_history_and_every_actor_origin_from_one_read_cut() {
        const TURN_COUNT: usize = 1_000;

        let fixture = Fixture::new(Some("model-a"));
        save_settled_history(&fixture, TURN_COUNT, false);

        let root = fixture
            .service
            .load_conversation_observer_snapshot("root-conversation")
            .unwrap()
            .unwrap();
        assert_eq!(root.conversation.messages.len(), TURN_COUNT * 2);
        assert_eq!(root.input_origins.len(), TURN_COUNT);
        assert!(root
            .input_origins
            .values()
            .all(|origin| matches!(origin, crate::ConversationMessageOrigin::Human)));

        let mut input = spawn_input("bulk-observer-child", "bulk_observer");
        input.fork_turns = AgentForkTurns::All;
        let child = fixture.service.create_child_agent(&input).unwrap();
        let observer = fixture
            .service
            .load_conversation_observer_snapshot(&child.agent.conversation_id)
            .unwrap()
            .unwrap();
        assert_eq!(observer.conversation.messages.len(), TURN_COUNT * 2 + 1);
        assert_eq!(observer.input_origins.len(), TURN_COUNT + 1);
        assert_eq!(
            observer
                .input_origins
                .get(&child.task_message.projection_message_id),
            Some(&crate::ConversationMessageOrigin::Agent {
                sender_agent_id: "agent-root".to_string(),
                source_agent_message_id: child.task_message.message_id.clone(),
            })
        );

        let snapshots = observer
            .input_origins
            .iter()
            .filter(|(message_id, _)| *message_id != &child.task_message.projection_message_id)
            .collect::<Vec<_>>();
        assert_eq!(snapshots.len(), TURN_COUNT);
        for (message_id, origin) in snapshots {
            let crate::ConversationMessageOrigin::HistoricalSnapshot {
                source_conversation_id,
                source_message_id,
                original,
            } = origin
            else {
                panic!("input {message_id} lost its historical snapshot provenance");
            };
            assert_eq!(source_conversation_id, "root-conversation");
            assert!(source_message_id.starts_with("root-user-"));
            assert!(matches!(
                original.as_ref(),
                crate::ConversationMessageOrigin::Human
            ));
        }
        assert!(observer
            .conversation
            .messages
            .iter()
            .filter(|message| message.role == "user")
            .all(|message| observer.input_origins.contains_key(&message.id)));
    }

    #[test]
    fn user_facing_root_reload_hides_only_agent_projection_while_observer_keeps_origins() {
        let fixture = Fixture::new(Some("model-a"));
        save_settled_history(&fixture, 1, false);
        // Exercise reload projection from the durable trace, not the intentionally minimal
        // legacy AgentRun fixture used by the fork characterization helper.
        fixture
            .service
            .state
            .connection()
            .unwrap()
            .execute(
                "UPDATE messages SET agent_run_json = NULL WHERE id = 'root-assistant-0'",
                [],
            )
            .unwrap();

        let mut child_input = spawn_input("observer-origin-child", "observer_origin");
        child_input.fork_turns = AgentForkTurns::All;
        let child = fixture.service.create_child_agent(&child_input).unwrap();
        let result = EnqueueAgentMessageInput {
            message_id: "mailbox-child-result".to_string(),
            root_agent_id: "agent-root".to_string(),
            sender_agent_id: child.agent.agent_id.clone(),
            recipient_agent_id: "agent-root".to_string(),
            request_id: "request-child-result".to_string(),
            kind: AgentMailboxKind::Result,
            content: "durable child result".to_string(),
            projection_message_id: "projection-child-result".to_string(),
        };
        fixture.service.enqueue_agent_message(&result).unwrap();
        let claimed = fixture
            .service
            .claim_next_agent_message("agent-root", "claim-child-result")
            .unwrap()
            .unwrap();
        assert_eq!(claimed.message_id, result.message_id);
        fixture
            .service
            .acknowledge_agent_message_with_projection(&result.message_id, "claim-child-result")
            .unwrap();

        // The transport fact remains part of the durable Conversation and model context.
        let raw_root = fixture
            .service
            .load_conversation("root-conversation")
            .unwrap()
            .unwrap();
        assert!(raw_root
            .messages
            .iter()
            .any(|message| message.id == result.projection_message_id));

        // Reopen the database to characterize renderer reloads rather than an in-memory cache.
        let reopened =
            StorageService::open(&fixture._directory.path().join("storage.sqlite")).unwrap();
        let single_root = reopened
            .load_conversation_view("root-conversation")
            .unwrap()
            .unwrap()
            .conversation;
        assert!(single_root
            .messages
            .iter()
            .any(|message| message.id == "root-user-0"));
        assert!(!single_root
            .messages
            .iter()
            .any(|message| message.id == result.projection_message_id));

        let listed_roots = reopened.load_conversation_views().unwrap();
        assert_eq!(listed_roots.len(), 1);
        assert!(listed_roots[0]
            .conversation
            .messages
            .iter()
            .any(|message| message.id == "root-user-0"));
        assert!(!listed_roots[0]
            .conversation
            .messages
            .iter()
            .any(|message| message.id == result.projection_message_id));

        reopened
            .state
            .connection()
            .unwrap()
            .execute(
                "UPDATE conversations
                 SET title = 'Durable child result title', updated_at = updated_at + 1
                 WHERE id = 'root-conversation'",
                [],
            )
            .unwrap();
        let hidden_transport_hits = reopened
            .search_chats(&ChatSearchInput {
                query: "durable child result".to_string(),
                limit: Some(10),
            })
            .unwrap();
        assert_eq!(hidden_transport_hits.len(), 1);
        assert_eq!(
            hidden_transport_hits[0].conversation_id,
            "root-conversation"
        );
        assert_eq!(hidden_transport_hits[0].message_id, None);
        assert_eq!(hidden_transport_hits[0].snippet, None);
        let human_hits = reopened
            .search_chats(&ChatSearchInput {
                query: "question 0".to_string(),
                limit: Some(10),
            })
            .unwrap();
        assert_eq!(human_hits.len(), 1);
        assert_eq!(human_hits[0].message_id.as_deref(), Some("root-user-0"));

        // The exact child observer read remains unfiltered and preserves both live Agent and
        // historical snapshot provenance for renderer labels and audit.
        let observer = reopened
            .load_conversation_observer_snapshot(&child.agent.conversation_id)
            .unwrap()
            .unwrap();
        assert_eq!(
            observer
                .input_origins
                .get(&child.task_message.projection_message_id),
            Some(&crate::ConversationMessageOrigin::Agent {
                sender_agent_id: "agent-root".to_string(),
                source_agent_message_id: child.task_message.message_id.clone(),
            })
        );
        let snapshot_message = observer
            .conversation
            .messages
            .iter()
            .find(|message| message.content == "question 0")
            .expect("historical input remains visible to the child observer");
        assert_eq!(
            observer.input_origins.get(&snapshot_message.id),
            Some(&crate::ConversationMessageOrigin::HistoricalSnapshot {
                source_conversation_id: "root-conversation".to_string(),
                source_message_id: "root-user-0".to_string(),
                original: Box::new(crate::ConversationMessageOrigin::Human),
            })
        );
    }

    #[test]
    fn three_level_agent_tree_fork_is_recursive_idempotent_and_independent() {
        const FIRST_FORK_REQUEST: &str = "three-level-tree-fork";
        const SECOND_FORK_REQUEST: &str = "three-level-tree-fork-recursive";
        const ROOT_BOUNDARY_CONTENT: &str = "three-level root fork boundary";
        const CHILD_FAILED_CONTENT: &str = "child failed before tree fork";
        const CHILD_AFTER_CUTOFF_CONTENT: &str = "child completed after tree fork cutoff";
        const GRANDCHILD_COMPLETED_CONTENT: &str = "grandchild completed before tree fork";
        const CHILD_TERMINAL_ERROR: &str = "child terminal failure before tree fork";

        let fixture = Fixture::new(Some("model-a"));
        save_settled_history(&fixture, 1, false);
        let child = fixture
            .service
            .create_child_agent(&spawn_input("tree-fork-child", "tree_fork_child"))
            .unwrap();
        let mut grandchild_input = spawn_input("tree-fork-grandchild", "tree_fork_grandchild");
        grandchild_input.parent_agent_id = child.agent.agent_id.clone();
        let grandchild = fixture
            .service
            .create_child_agent(&grandchild_input)
            .unwrap();
        for wake_id in [
            &child.initial_wake.wake_id,
            &grandchild.initial_wake.wake_id,
        ] {
            fixture
                .service
                .transition_agent_wake(
                    wake_id,
                    AgentWakeStatus::Queued,
                    AgentWakeStatus::Cancelled,
                    None,
                )
                .unwrap();
        }

        let member_history_at = now_ms().saturating_add(1);
        append_terminal_assistant_for_tree_fork(
            &fixture,
            &child.agent.conversation_id,
            "tree-child-failed-before",
            "tree-child-run-before",
            CHILD_FAILED_CONTENT,
            ConversationTurnTraceTerminalStatus::Failed,
            Some(CHILD_TERMINAL_ERROR),
            member_history_at,
        );
        append_terminal_assistant_for_tree_fork(
            &fixture,
            &grandchild.agent.conversation_id,
            "tree-grandchild-completed-before",
            "tree-grandchild-run-before",
            GRANDCHILD_COMPLETED_CONTENT,
            ConversationTurnTraceTerminalStatus::Completed,
            None,
            member_history_at.saturating_add(1),
        );
        let fork_boundary_at = now_ms().max(member_history_at.saturating_add(2));
        append_terminal_assistant_for_tree_fork(
            &fixture,
            "root-conversation",
            "tree-root-boundary",
            "tree-root-boundary-run",
            ROOT_BOUNDARY_CONTENT,
            ConversationTurnTraceTerminalStatus::Completed,
            None,
            fork_boundary_at,
        );
        append_terminal_assistant_for_tree_fork(
            &fixture,
            &child.agent.conversation_id,
            "tree-child-completed-after",
            "tree-child-run-after",
            CHILD_AFTER_CUTOFF_CONTENT,
            ConversationTurnTraceTerminalStatus::Completed,
            None,
            fork_boundary_at.saturating_add(1),
        );

        let source_tree = tree_by_task_path(&fixture, "agent-root");
        assert_eq!(source_tree.len(), 3);
        let request = ForkConversationRequest {
            request_id: FIRST_FORK_REQUEST.to_string(),
            source_conversation_id: "root-conversation".to_string(),
            fork_point: ConversationForkPoint::AssistantReply {
                assistant_message_id: "tree-root-boundary".to_string(),
            },
        };
        let first_fork = fixture
            .service
            .fork_conversation_request_view(request.clone())
            .unwrap();
        let first_root = fixture
            .service
            .get_agent_node_by_conversation(&first_fork.conversation.id)
            .unwrap()
            .expect("the first fork owns an independent root Agent");
        let first_tree = tree_by_task_path(&fixture, &first_root.root_agent_id);
        assert_eq!(first_tree.len(), 3);
        assert_tree_identities_are_fresh(&fixture, &source_tree, &first_tree);

        let first_child = first_tree
            .get(&child.agent.task_path)
            .expect("the child task path is preserved");
        let first_grandchild = first_tree
            .get(&grandchild.agent.task_path)
            .expect("the grandchild task path is preserved");
        assert_eq!(
            first_child.parent_agent_id.as_deref(),
            Some(first_root.agent_id.as_str())
        );
        assert_eq!(
            first_grandchild.parent_agent_id.as_deref(),
            Some(first_child.agent_id.as_str())
        );
        assert!(first_tree.values().all(|member| {
            member.root_agent_id == first_root.agent_id
                && member.root_conversation_id == first_root.conversation_id
        }));

        let first_child_conversation = fixture
            .service
            .load_conversation(&first_child.conversation_id)
            .unwrap()
            .unwrap();
        let first_child_failed = first_child_conversation
            .messages
            .iter()
            .find(|message| message.content == CHILD_FAILED_CONTENT)
            .expect("the pre-cutoff failed child reply is cloned");
        assert_eq!(first_child_failed.status.as_deref(), Some("failed"));
        assert!(first_child_conversation
            .messages
            .iter()
            .all(|message| message.content != CHILD_AFTER_CUTOFF_CONTENT));
        let first_child_trace = fixture
            .service
            .get_conversation_turn_trace(&first_child_failed.id)
            .unwrap()
            .expect("the child terminal trace is cloned");
        assert_eq!(
            first_child_trace.terminal_status,
            ConversationTurnTraceTerminalStatus::Failed
        );
        assert_eq!(
            first_child_trace.terminal_error.as_deref(),
            Some(CHILD_TERMINAL_ERROR)
        );
        assert_eq!(first_child_trace.items.len(), 1);
        assert_ne!(first_child_trace.run_id, "tree-child-run-before");

        let first_grandchild_conversation = fixture
            .service
            .load_conversation(&first_grandchild.conversation_id)
            .unwrap()
            .unwrap();
        let first_grandchild_completed = first_grandchild_conversation
            .messages
            .iter()
            .find(|message| message.content == GRANDCHILD_COMPLETED_CONTENT)
            .expect("the pre-cutoff grandchild reply is cloned");
        let first_grandchild_trace = fixture
            .service
            .get_conversation_turn_trace(&first_grandchild_completed.id)
            .unwrap()
            .expect("the grandchild terminal trace is cloned");
        assert_eq!(
            first_grandchild_trace.terminal_status,
            ConversationTurnTraceTerminalStatus::Completed
        );
        assert_ne!(first_grandchild_trace.run_id, "tree-grandchild-run-before");
        assert_eq!(member_fork_receipt_count(&fixture, FIRST_FORK_REQUEST), 2);

        let retried = fixture
            .service
            .fork_conversation_request_view(request)
            .unwrap();
        assert_eq!(retried.conversation.id, first_fork.conversation.id);
        assert_eq!(
            tree_by_task_path(&fixture, &first_root.root_agent_id),
            first_tree
        );
        assert_eq!(member_fork_receipt_count(&fixture, FIRST_FORK_REQUEST), 2);

        let first_root_conversation = fixture
            .service
            .load_conversation(&first_root.conversation_id)
            .unwrap()
            .unwrap();
        let first_root_boundary = first_root_conversation
            .messages
            .iter()
            .find(|message| message.content == ROOT_BOUNDARY_CONTENT)
            .expect("the first fork preserves its root boundary");
        let second_fork = fixture
            .service
            .fork_conversation_request_view(ForkConversationRequest {
                request_id: SECOND_FORK_REQUEST.to_string(),
                source_conversation_id: first_root.conversation_id.clone(),
                fork_point: ConversationForkPoint::AssistantReply {
                    assistant_message_id: first_root_boundary.id.clone(),
                },
            })
            .unwrap();
        let second_root = fixture
            .service
            .get_agent_node_by_conversation(&second_fork.conversation.id)
            .unwrap()
            .expect("the recursive fork owns another independent root Agent");
        let second_tree = tree_by_task_path(&fixture, &second_root.root_agent_id);
        assert_eq!(second_tree.len(), 3);
        assert_tree_identities_are_fresh(&fixture, &first_tree, &second_tree);
        assert_tree_identities_are_fresh(&fixture, &source_tree, &second_tree);
        let second_grandchild = second_tree
            .get(&grandchild.agent.task_path)
            .expect("the recursive fork preserves the grandchild task path");
        let second_grandchild_conversation = fixture
            .service
            .load_conversation(&second_grandchild.conversation_id)
            .unwrap()
            .unwrap();
        let second_grandchild_completed = second_grandchild_conversation
            .messages
            .iter()
            .find(|message| message.content == GRANDCHILD_COMPLETED_CONTENT)
            .expect("the recursive fork preserves grandchild history");
        let second_grandchild_trace = fixture
            .service
            .get_conversation_turn_trace(&second_grandchild_completed.id)
            .unwrap()
            .expect("the recursive fork preserves the grandchild trace");
        assert_eq!(
            second_grandchild_trace.terminal_status,
            ConversationTurnTraceTerminalStatus::Completed
        );
        assert_ne!(
            second_grandchild_trace.run_id,
            first_grandchild_trace.run_id
        );
        assert_eq!(member_fork_receipt_count(&fixture, SECOND_FORK_REQUEST), 2);

        fixture
            .service
            .delete_conversation("root-conversation")
            .unwrap();
        for source in source_tree.values() {
            assert!(fixture
                .service
                .load_conversation(&source.conversation_id)
                .unwrap()
                .is_none());
            assert!(fixture
                .service
                .get_agent_node(&source.agent_id)
                .unwrap()
                .is_none());
        }
        for independent in first_tree.values().chain(second_tree.values()) {
            assert!(fixture
                .service
                .load_conversation(&independent.conversation_id)
                .unwrap()
                .is_some());
            assert_eq!(
                fixture
                    .service
                    .get_agent_node(&independent.agent_id)
                    .unwrap()
                    .as_ref(),
                Some(independent)
            );
        }
        let connection = fixture.service.state.connection().unwrap();
        let violations = connection
            .prepare("PRAGMA foreign_key_check")
            .unwrap()
            .query_map([], |_| Ok(()))
            .unwrap()
            .count();
        assert_eq!(violations, 0);
    }

    #[test]
    fn member_agent_fork_preserves_attachment_and_complete_turn_diff_recursively() {
        const MEMBER_CONTENT: &str = "member reply with attachment and turn diff";
        const ROOT_BOUNDARY_CONTENT: &str = "root boundary after member artifacts";
        const SOURCE_MESSAGE_ID: &str = "member-artifacts-assistant";
        const SOURCE_RUN_ID: &str = "member-artifacts-run";
        const SOURCE_ATTACHMENT_ID: &str = "member-artifacts-attachment";
        const SOURCE_BYTES: &[u8] = b"durable member attachment bytes";
        const ACTION_IDS: [&str; 2] = ["member-action-binary", "member-action-text"];

        let fixture = Fixture::new(Some("model-a"));
        save_settled_history(&fixture, 1, false);
        let child = fixture
            .service
            .create_child_agent(&spawn_input(
                "member-artifacts-child",
                "member_artifacts_child",
            ))
            .unwrap();
        fixture
            .service
            .transition_agent_wake(
                &child.initial_wake.wake_id,
                AgentWakeStatus::Queued,
                AgentWakeStatus::Cancelled,
                None,
            )
            .unwrap();

        let member_history_at = now_ms().saturating_add(1);
        append_terminal_assistant_for_tree_fork(
            &fixture,
            &child.agent.conversation_id,
            SOURCE_MESSAGE_ID,
            SOURCE_RUN_ID,
            MEMBER_CONTENT,
            ConversationTurnTraceTerminalStatus::Completed,
            None,
            member_history_at,
        );
        let source_attachment = attach_file_to_tree_member_message(
            &fixture,
            &child.agent.conversation_id,
            SOURCE_MESSAGE_ID,
            SOURCE_ATTACHMENT_ID,
            SOURCE_BYTES,
            member_history_at,
        );
        let workspace_root = fixture
            ._directory
            .path()
            .join("project-a")
            .to_string_lossy()
            .to_string();
        let source_diff_identity = crate::AgentTurnDiffIdentity {
            run_id: SOURCE_RUN_ID.to_string(),
            conversation_id: child.agent.conversation_id.clone(),
            assistant_message_id: SOURCE_MESSAGE_ID.to_string(),
            project_id: "project-a".to_string(),
            workspace_root: workspace_root.clone(),
        };
        fixture
            .service
            .initialize_agent_turn_diff(&source_diff_identity)
            .unwrap();
        let expected_files = vec![
            crate::AgentTurnFileChange {
                path: "assets/member.bin".to_string(),
                before: crate::AgentTurnFileContent::Missing,
                after: crate::AgentTurnFileContent::Binary,
            },
            crate::AgentTurnFileChange {
                path: "src/member.rs".to_string(),
                before: crate::AgentTurnFileContent::Text("before member edit\n".to_string()),
                after: crate::AgentTurnFileContent::Text("after member edit\n".to_string()),
            },
        ];
        for (action_id, change) in ACTION_IDS.iter().zip(&expected_files) {
            assert!(fixture
                .service
                .record_agent_turn_file_change(&source_diff_identity, action_id, change)
                .unwrap());
        }
        {
            let connection = fixture.service.state.connection().unwrap();
            assert_eq!(
                connection
                    .execute(
                        "UPDATE agent_turn_diffs
                         SET truncated = 1, updated_at = MAX(updated_at, ?2)
                         WHERE assistant_message_id = ?1",
                        rusqlite::params![SOURCE_MESSAGE_ID, member_history_at.saturating_add(1)],
                    )
                    .unwrap(),
                1
            );
        }
        assert_eq!(
            turn_diff_action_ids(&fixture, SOURCE_MESSAGE_ID),
            ACTION_IDS.map(ToString::to_string)
        );

        let fork_boundary_at = now_ms().max(member_history_at.saturating_add(2));
        append_terminal_assistant_for_tree_fork(
            &fixture,
            "root-conversation",
            "member-artifacts-root-boundary",
            "member-artifacts-root-run",
            ROOT_BOUNDARY_CONTENT,
            ConversationTurnTraceTerminalStatus::Completed,
            None,
            fork_boundary_at,
        );
        let first_fork = fixture
            .service
            .fork_conversation_request_view(ForkConversationRequest {
                request_id: "member-artifacts-first-fork".to_string(),
                source_conversation_id: "root-conversation".to_string(),
                fork_point: ConversationForkPoint::AssistantReply {
                    assistant_message_id: "member-artifacts-root-boundary".to_string(),
                },
            })
            .unwrap();
        let first_root = fixture
            .service
            .get_agent_node_by_conversation(&first_fork.conversation.id)
            .unwrap()
            .unwrap();
        let source_tree = tree_by_task_path(&fixture, "agent-root");
        let first_tree = tree_by_task_path(&fixture, &first_root.root_agent_id);
        assert_tree_identities_are_fresh(&fixture, &source_tree, &first_tree);
        let first_child = first_tree.get(&child.agent.task_path).unwrap();
        let first_child_conversation = fixture
            .service
            .load_conversation(&first_child.conversation_id)
            .unwrap()
            .unwrap();
        let first_message = first_child_conversation
            .messages
            .iter()
            .find(|message| message.content == MEMBER_CONTENT)
            .expect("the member artifact owner message is cloned");
        assert_ne!(first_message.id, SOURCE_MESSAGE_ID);
        assert_eq!(first_message.attachments.len(), 1);
        assert_ne!(first_message.attachments[0].id, SOURCE_ATTACHMENT_ID);
        let first_attachment = {
            let connection = fixture.service.state.connection().unwrap();
            attachment_repository::list_conversation_attachments(
                &connection,
                &first_child.conversation_id,
            )
            .unwrap()
            .into_iter()
            .next()
            .unwrap()
        };
        assert_eq!(first_message.attachments[0].id, first_attachment.id);
        assert_eq!(first_attachment.message_id, first_message.id);
        assert_eq!(
            first_attachment.conversation_id,
            first_child.conversation_id
        );
        assert_ne!(first_attachment.id, source_attachment.id);
        assert_ne!(
            first_attachment.storage_rel_path,
            source_attachment.storage_rel_path
        );
        assert_eq!(
            first_attachment.storage_rel_path,
            slash_path(&attachment_storage_rel_path(
                &first_child.conversation_id,
                &first_message.id,
                &first_attachment.id,
                &first_attachment.original_name,
            ))
        );
        assert_eq!(
            fs::read(
                fixture
                    .service
                    .attachment_root
                    .join(&first_attachment.storage_rel_path)
            )
            .unwrap(),
            SOURCE_BYTES
        );

        let first_diffs = fixture
            .service
            .load_agent_turn_diffs_for_messages(
                &first_child.conversation_id,
                "project-a",
                std::slice::from_ref(&first_message.id),
            )
            .unwrap();
        assert_eq!(first_diffs.len(), 1);
        let first_diff = &first_diffs[0];
        assert_eq!(
            first_diff.identity.conversation_id,
            first_child.conversation_id
        );
        assert_eq!(first_diff.identity.assistant_message_id, first_message.id);
        assert_ne!(first_diff.identity.run_id, SOURCE_RUN_ID);
        assert_eq!(first_diff.identity.workspace_root, workspace_root);
        assert_eq!(first_diff.files, expected_files);
        assert!(first_diff.truncated);
        assert_eq!(
            turn_diff_action_ids(&fixture, &first_message.id),
            ACTION_IDS.map(ToString::to_string)
        );
        assert!(!fixture
            .service
            .record_agent_turn_file_change(&first_diff.identity, ACTION_IDS[0], &expected_files[0],)
            .unwrap());
        assert_eq!(
            member_fork_receipt_count(&fixture, "member-artifacts-first-fork"),
            1
        );

        let first_root_conversation = fixture
            .service
            .load_conversation(&first_root.conversation_id)
            .unwrap()
            .unwrap();
        let first_boundary = first_root_conversation
            .messages
            .iter()
            .find(|message| message.content == ROOT_BOUNDARY_CONTENT)
            .unwrap();
        let recursive_fork = fixture
            .service
            .fork_conversation_request_view(ForkConversationRequest {
                request_id: "member-artifacts-recursive-fork".to_string(),
                source_conversation_id: first_root.conversation_id.clone(),
                fork_point: ConversationForkPoint::AssistantReply {
                    assistant_message_id: first_boundary.id.clone(),
                },
            })
            .unwrap();
        let recursive_root = fixture
            .service
            .get_agent_node_by_conversation(&recursive_fork.conversation.id)
            .unwrap()
            .unwrap();
        let recursive_tree = tree_by_task_path(&fixture, &recursive_root.root_agent_id);
        assert_tree_identities_are_fresh(&fixture, &first_tree, &recursive_tree);
        let recursive_child = recursive_tree.get(&child.agent.task_path).unwrap();
        let recursive_child_conversation = fixture
            .service
            .load_conversation(&recursive_child.conversation_id)
            .unwrap()
            .unwrap();
        let recursive_message = recursive_child_conversation
            .messages
            .iter()
            .find(|message| message.content == MEMBER_CONTENT)
            .unwrap();
        let recursive_attachment = {
            let connection = fixture.service.state.connection().unwrap();
            attachment_repository::list_conversation_attachments(
                &connection,
                &recursive_child.conversation_id,
            )
            .unwrap()
            .into_iter()
            .next()
            .unwrap()
        };
        assert_ne!(recursive_attachment.id, source_attachment.id);
        assert_ne!(recursive_attachment.id, first_attachment.id);
        assert_eq!(recursive_attachment.message_id, recursive_message.id);
        assert_eq!(recursive_message.attachments[0].id, recursive_attachment.id);
        assert_eq!(
            recursive_attachment.storage_rel_path,
            slash_path(&attachment_storage_rel_path(
                &recursive_child.conversation_id,
                &recursive_message.id,
                &recursive_attachment.id,
                &recursive_attachment.original_name,
            ))
        );
        assert_eq!(
            fs::read(
                fixture
                    .service
                    .attachment_root
                    .join(&recursive_attachment.storage_rel_path)
            )
            .unwrap(),
            SOURCE_BYTES
        );
        let recursive_diff = fixture
            .service
            .load_agent_turn_diffs_for_messages(
                &recursive_child.conversation_id,
                "project-a",
                std::slice::from_ref(&recursive_message.id),
            )
            .unwrap()
            .pop()
            .unwrap();
        assert_ne!(recursive_diff.identity.run_id, first_diff.identity.run_id);
        assert_eq!(recursive_diff.files, expected_files);
        assert!(recursive_diff.truncated);
        assert_eq!(
            turn_diff_action_ids(&fixture, &recursive_message.id),
            ACTION_IDS.map(ToString::to_string)
        );
        assert_eq!(
            member_fork_receipt_count(&fixture, "member-artifacts-recursive-fork"),
            1
        );
    }

    #[test]
    fn member_compaction_respects_root_cutoff_and_remains_recursive() {
        const VISIBLE_CONTENT: &str = "member reply before compaction cutoff";
        const AFTER_CUTOFF_CONTENT: &str = "member reply after compaction cutoff";
        const ROOT_BOUNDARY_CONTENT: &str = "root boundary between member compactions";
        const VISIBLE_MESSAGE_ID: &str = "member-compaction-visible-assistant";
        const VISIBLE_RUN_ID: &str = "member-compaction-visible-run";
        const VISIBLE_SUMMARY_ID: &str = "member-compaction-visible-summary";
        const VISIBLE_OPERATION_ID: &str = "member-compaction-visible-operation";
        const AFTER_MESSAGE_ID: &str = "member-compaction-after-assistant";
        const AFTER_RUN_ID: &str = "member-compaction-after-run";
        const AFTER_SUMMARY_ID: &str = "member-compaction-after-summary";
        const AFTER_OPERATION_ID: &str = "member-compaction-after-operation";
        const FIRST_FORK_REQUEST: &str = "member-compaction-first-fork";
        const RECURSIVE_FORK_REQUEST: &str = "member-compaction-recursive-fork";

        let fixture = Fixture::new(Some("model-a"));
        save_settled_history(&fixture, 1, false);
        let child_input = spawn_input("member-compaction-child", "member_compaction_child");
        let child = fixture.service.create_child_agent(&child_input).unwrap();
        fixture
            .service
            .transition_agent_wake(
                &child.initial_wake.wake_id,
                AgentWakeStatus::Queued,
                AgentWakeStatus::Cancelled,
                None,
            )
            .unwrap();
        let member_model_id = child
            .agent
            .model_snapshot
            .as_ref()
            .unwrap()
            .model_config_id
            .clone();

        let visible_at = now_ms().saturating_add(1);
        append_terminal_assistant_for_tree_fork(
            &fixture,
            &child.agent.conversation_id,
            VISIBLE_MESSAGE_ID,
            VISIBLE_RUN_ID,
            VISIBLE_CONTENT,
            ConversationTurnTraceTerminalStatus::Completed,
            None,
            visible_at,
        );
        let visible_receipt = {
            let mut connection = fixture.service.state.connection().unwrap();
            let prefix = crate::storage::context_compaction_repository::prepare_prefix(
                &connection,
                &child.agent.conversation_id,
                &crate::ContextJournalCursor::message(&child.task_message.projection_message_id),
            )
            .unwrap();
            record_applied_member_compaction(
                &mut connection,
                &prefix,
                VISIBLE_SUMMARY_ID,
                VISIBLE_OPERATION_ID,
                VISIBLE_RUN_ID,
                VISIBLE_MESSAGE_ID,
                &member_model_id,
                visible_at.saturating_add(1),
            )
        };

        let root_cutoff_at = now_ms().max(visible_at.saturating_add(3));
        append_terminal_assistant_for_tree_fork(
            &fixture,
            "root-conversation",
            "member-compaction-root-boundary",
            "member-compaction-root-run",
            ROOT_BOUNDARY_CONTENT,
            ConversationTurnTraceTerminalStatus::Completed,
            None,
            root_cutoff_at,
        );
        append_terminal_assistant_for_tree_fork(
            &fixture,
            &child.agent.conversation_id,
            AFTER_MESSAGE_ID,
            AFTER_RUN_ID,
            AFTER_CUTOFF_CONTENT,
            ConversationTurnTraceTerminalStatus::Completed,
            None,
            root_cutoff_at.saturating_add(1),
        );
        let after_cutoff_receipt = {
            let mut connection = fixture.service.state.connection().unwrap();
            let prefix = crate::storage::context_compaction_repository::prepare_prefix(
                &connection,
                &child.agent.conversation_id,
                &crate::ContextJournalCursor::message(VISIBLE_MESSAGE_ID),
            )
            .unwrap();
            record_applied_member_compaction(
                &mut connection,
                &prefix,
                AFTER_SUMMARY_ID,
                AFTER_OPERATION_ID,
                AFTER_RUN_ID,
                AFTER_MESSAGE_ID,
                &member_model_id,
                root_cutoff_at.saturating_add(2),
            )
        };
        {
            let connection = fixture.service.state.connection().unwrap();
            assert_eq!(
                crate::storage::context_compaction_repository::list_active_summary_chain(
                    &connection,
                    &child.agent.conversation_id,
                )
                .unwrap()
                .len(),
                2
            );
            assert_eq!(
                crate::storage::context_compaction_receipt_repository::list_receipts_for_conversation(
                    &connection,
                    &child.agent.conversation_id,
                )
                .unwrap()
                .len(),
                2
            );
            assert_eq!(
                crate::storage::model_request_observation_repository::list_observations_for_conversation(
                    &connection,
                    &child.agent.conversation_id,
                )
                .unwrap()
                .len(),
                2
            );
        }

        let first_fork = fixture
            .service
            .fork_conversation_request_view(ForkConversationRequest {
                request_id: FIRST_FORK_REQUEST.to_string(),
                source_conversation_id: "root-conversation".to_string(),
                fork_point: ConversationForkPoint::AssistantReply {
                    assistant_message_id: "member-compaction-root-boundary".to_string(),
                },
            })
            .unwrap();
        let first_root = fixture
            .service
            .get_agent_node_by_conversation(&first_fork.conversation.id)
            .unwrap()
            .unwrap();
        let first_tree = tree_by_task_path(&fixture, &first_root.root_agent_id);
        let first_child = first_tree.get(&child.agent.task_path).unwrap();
        let first_conversation = fixture
            .service
            .load_conversation(&first_child.conversation_id)
            .unwrap()
            .unwrap();
        assert_eq!(first_conversation.messages.len(), 2);
        let first_task = first_conversation
            .messages
            .iter()
            .find(|message| message.content == child_input.task)
            .unwrap();
        let first_visible = first_conversation
            .messages
            .iter()
            .find(|message| message.content == VISIBLE_CONTENT)
            .unwrap();
        assert!(first_conversation
            .messages
            .iter()
            .all(|message| message.content != AFTER_CUTOFF_CONTENT));
        assert_eq!(
            fixture
                .service
                .conversation_message_origin(&first_child.conversation_id, &first_task.id)
                .unwrap(),
            ConversationMessageOrigin::HistoricalSnapshot {
                source_conversation_id: child.agent.conversation_id.clone(),
                source_message_id: child.task_message.projection_message_id.clone(),
                original: Box::new(ConversationMessageOrigin::Agent {
                    sender_agent_id: "agent-root".to_string(),
                    source_agent_message_id: child.task_message.message_id.clone(),
                }),
            }
        );
        let first_visible_run_id = serde_json::from_str::<serde_json::Value>(
            first_visible.agent_run_json.as_deref().unwrap(),
        )
        .unwrap()["runId"]
            .as_str()
            .unwrap()
            .to_string();
        let (first_chain, first_receipts, first_observations, first_head) = {
            let connection = fixture.service.state.connection().unwrap();
            let snapshot_count = connection
                .query_row(
                    "SELECT COUNT(*) FROM messages
                     WHERE conversation_id = ?1 AND input_origin_kind = 'snapshot'",
                    [&first_child.conversation_id],
                    |row| row.get::<_, usize>(0),
                )
                .unwrap();
            assert_eq!(snapshot_count, first_conversation.messages.len());
            let chain = crate::storage::context_compaction_repository::list_active_summary_chain(
                &connection,
                &first_child.conversation_id,
            )
            .unwrap();
            let receipts = crate::storage::context_compaction_receipt_repository::list_receipts_for_conversation(
                &connection,
                &first_child.conversation_id,
            )
            .unwrap();
            let observations = crate::storage::model_request_observation_repository::list_observations_for_conversation(
                &connection,
                &first_child.conversation_id,
            )
            .unwrap();
            let head = connection
                .query_row(
                    "SELECT summary_id FROM conversation_context_compaction_heads
                     WHERE conversation_id = ?1",
                    [&first_child.conversation_id],
                    |row| row.get::<_, String>(0),
                )
                .unwrap();
            (chain, receipts, observations, head)
        };
        assert_eq!(first_chain.len(), 1);
        let first_summary = &first_chain[0];
        assert_ne!(first_summary.summary.id, VISIBLE_SUMMARY_ID);
        assert_ne!(first_summary.summary.id, AFTER_SUMMARY_ID);
        assert_eq!(
            first_summary.lineage.source_summary_id.as_deref(),
            visible_receipt.summary_id.as_deref()
        );
        assert_ne!(
            first_summary.lineage.source_summary_id.as_deref(),
            after_cutoff_receipt.summary_id.as_deref()
        );
        assert_eq!(
            first_summary.summary.content,
            format!("summary {VISIBLE_SUMMARY_ID}")
        );
        assert_eq!(
            first_summary.summary.conversation_id,
            first_child.conversation_id
        );
        assert_eq!(
            first_summary.summary.covered_through,
            crate::ContextJournalCursor::message(&first_task.id)
        );
        assert_eq!(first_head, first_summary.summary.id);

        assert_eq!(first_receipts.len(), 1);
        let first_receipt = &first_receipts[0];
        assert!(!first_receipt
            .operation_id
            .starts_with("provider-transition-"));
        assert_ne!(first_receipt.operation_id, visible_receipt.operation_id);
        assert_ne!(
            first_receipt.operation_id,
            after_cutoff_receipt.operation_id
        );
        assert_eq!(first_receipt.run_id, first_visible_run_id);
        assert_ne!(first_receipt.run_id, visible_receipt.run_id);
        assert_eq!(first_receipt.conversation_id, first_child.conversation_id);
        assert_eq!(first_receipt.assistant_message_id, first_visible.id);
        assert_eq!(
            first_receipt.status,
            crate::ContextCompactionReceiptStatus::Applied
        );
        assert_eq!(
            first_receipt.stage,
            crate::ContextCompactionReceiptStage::Completed
        );
        assert_eq!(
            first_receipt.plan.covered_through,
            crate::ContextJournalCursor::message(&first_task.id)
        );
        assert_eq!(
            first_receipt.summary_id.as_deref(),
            Some(first_summary.summary.id.as_str())
        );
        assert_eq!(first_receipt.completed_at, visible_receipt.completed_at);
        assert_eq!(first_observations.len(), 1);
        let first_observation = &first_observations[0];
        assert_eq!(
            first_receipt.generation_observation_id.as_deref(),
            Some(first_observation.id.as_str())
        );
        assert_ne!(
            first_observation.id,
            visible_receipt.generation_observation_id.clone().unwrap()
        );
        assert_eq!(first_observation.run_id, first_receipt.run_id);
        assert_eq!(
            first_observation.conversation_id.as_deref(),
            Some(first_child.conversation_id.as_str())
        );
        assert_eq!(
            first_observation.assistant_message_id.as_deref(),
            Some(first_visible.id.as_str())
        );
        assert_eq!(
            first_observation.operation_id.as_deref(),
            Some(first_receipt.operation_id.as_str())
        );
        assert_eq!(
            first_observation.purpose,
            crate::ModelRequestPurpose::ContextCompaction
        );
        assert_eq!(member_fork_receipt_count(&fixture, FIRST_FORK_REQUEST), 1);

        let first_root_conversation = fixture
            .service
            .load_conversation(&first_root.conversation_id)
            .unwrap()
            .unwrap();
        let first_root_boundary = first_root_conversation
            .messages
            .iter()
            .find(|message| message.content == ROOT_BOUNDARY_CONTENT)
            .unwrap();
        let recursive_fork = fixture
            .service
            .fork_conversation_request_view(ForkConversationRequest {
                request_id: RECURSIVE_FORK_REQUEST.to_string(),
                source_conversation_id: first_root.conversation_id.clone(),
                fork_point: ConversationForkPoint::AssistantReply {
                    assistant_message_id: first_root_boundary.id.clone(),
                },
            })
            .unwrap();
        let recursive_root = fixture
            .service
            .get_agent_node_by_conversation(&recursive_fork.conversation.id)
            .unwrap()
            .unwrap();
        let recursive_tree = tree_by_task_path(&fixture, &recursive_root.root_agent_id);
        let recursive_child = recursive_tree.get(&child.agent.task_path).unwrap();
        let recursive_conversation = fixture
            .service
            .load_conversation(&recursive_child.conversation_id)
            .unwrap()
            .unwrap();
        assert_eq!(recursive_conversation.messages.len(), 2);
        let recursive_task = recursive_conversation
            .messages
            .iter()
            .find(|message| message.content == child_input.task)
            .unwrap();
        let recursive_visible = recursive_conversation
            .messages
            .iter()
            .find(|message| message.content == VISIBLE_CONTENT)
            .unwrap();
        assert!(recursive_conversation
            .messages
            .iter()
            .all(|message| message.content != AFTER_CUTOFF_CONTENT));
        assert_eq!(
            fixture
                .service
                .conversation_message_origin(&recursive_child.conversation_id, &recursive_task.id)
                .unwrap(),
            ConversationMessageOrigin::HistoricalSnapshot {
                source_conversation_id: first_child.conversation_id.clone(),
                source_message_id: first_task.id.clone(),
                original: Box::new(ConversationMessageOrigin::Agent {
                    sender_agent_id: "agent-root".to_string(),
                    source_agent_message_id: child.task_message.message_id.clone(),
                }),
            }
        );
        let recursive_visible_run_id = serde_json::from_str::<serde_json::Value>(
            recursive_visible.agent_run_json.as_deref().unwrap(),
        )
        .unwrap()["runId"]
            .as_str()
            .unwrap()
            .to_string();
        let (recursive_chain, recursive_receipts, recursive_observations, recursive_head) = {
            let connection = fixture.service.state.connection().unwrap();
            let snapshot_count = connection
                .query_row(
                    "SELECT COUNT(*) FROM messages
                     WHERE conversation_id = ?1 AND input_origin_kind = 'snapshot'",
                    [&recursive_child.conversation_id],
                    |row| row.get::<_, usize>(0),
                )
                .unwrap();
            assert_eq!(snapshot_count, recursive_conversation.messages.len());
            let chain = crate::storage::context_compaction_repository::list_active_summary_chain(
                &connection,
                &recursive_child.conversation_id,
            )
            .unwrap();
            let receipts = crate::storage::context_compaction_receipt_repository::list_receipts_for_conversation(
                &connection,
                &recursive_child.conversation_id,
            )
            .unwrap();
            let observations = crate::storage::model_request_observation_repository::list_observations_for_conversation(
                &connection,
                &recursive_child.conversation_id,
            )
            .unwrap();
            let head = connection
                .query_row(
                    "SELECT summary_id FROM conversation_context_compaction_heads
                     WHERE conversation_id = ?1",
                    [&recursive_child.conversation_id],
                    |row| row.get::<_, String>(0),
                )
                .unwrap();
            (chain, receipts, observations, head)
        };
        assert_eq!(recursive_chain.len(), 1);
        let recursive_summary = &recursive_chain[0];
        assert_ne!(recursive_summary.summary.id, first_summary.summary.id);
        assert_eq!(
            recursive_summary.lineage.source_summary_id.as_deref(),
            Some(first_summary.summary.id.as_str())
        );
        assert_eq!(
            recursive_summary.summary.content,
            first_summary.summary.content
        );
        assert_eq!(
            recursive_summary.summary.covered_through,
            crate::ContextJournalCursor::message(&recursive_task.id)
        );
        assert_eq!(recursive_head, recursive_summary.summary.id);
        assert_eq!(recursive_receipts.len(), 1);
        let recursive_receipt = &recursive_receipts[0];
        assert_ne!(recursive_receipt.operation_id, first_receipt.operation_id);
        assert_eq!(recursive_receipt.run_id, recursive_visible_run_id);
        assert_ne!(recursive_receipt.run_id, first_receipt.run_id);
        assert_eq!(
            recursive_receipt.conversation_id,
            recursive_child.conversation_id
        );
        assert_eq!(recursive_receipt.assistant_message_id, recursive_visible.id);
        assert_eq!(
            recursive_receipt.plan.covered_through,
            crate::ContextJournalCursor::message(&recursive_task.id)
        );
        assert_eq!(
            recursive_receipt.summary_id.as_deref(),
            Some(recursive_summary.summary.id.as_str())
        );
        assert_eq!(recursive_observations.len(), 1);
        let recursive_observation = &recursive_observations[0];
        assert_eq!(
            recursive_receipt.generation_observation_id.as_deref(),
            Some(recursive_observation.id.as_str())
        );
        assert_ne!(recursive_observation.id, first_observation.id);
        assert_eq!(recursive_observation.run_id, recursive_receipt.run_id);
        assert_eq!(
            recursive_observation.conversation_id.as_deref(),
            Some(recursive_child.conversation_id.as_str())
        );
        assert_eq!(
            recursive_observation.assistant_message_id.as_deref(),
            Some(recursive_visible.id.as_str())
        );
        assert_eq!(
            recursive_observation.operation_id.as_deref(),
            Some(recursive_receipt.operation_id.as_str())
        );
        assert_eq!(
            member_fork_receipt_count(&fixture, RECURSIVE_FORK_REQUEST),
            1
        );
    }

    #[test]
    fn active_root_fork_is_atomic_idempotent_and_preserves_agent_actor_without_crossing_trees() {
        let fixture = Fixture::new(Some("model-a"));
        save_settled_history(&fixture, 1, false);
        let child = fixture
            .service
            .create_child_agent(&spawn_input("fork-child", "fork_child"))
            .unwrap();
        fixture
            .service
            .transition_agent_wake(
                &child.initial_wake.wake_id,
                AgentWakeStatus::Queued,
                AgentWakeStatus::Cancelled,
                None,
            )
            .unwrap();
        let result = EnqueueAgentMessageInput {
            message_id: "mailbox-fork-result".to_string(),
            root_agent_id: "agent-root".to_string(),
            sender_agent_id: child.agent.agent_id.clone(),
            recipient_agent_id: "agent-root".to_string(),
            request_id: "request-fork-result".to_string(),
            kind: AgentMailboxKind::Result,
            content: "internal child evidence".to_string(),
            projection_message_id: "projection-fork-result".to_string(),
        };
        fixture.service.enqueue_agent_message(&result).unwrap();
        fixture
            .service
            .claim_next_agent_message("agent-root", "claim-fork-result")
            .unwrap()
            .unwrap();
        fixture
            .service
            .acknowledge_agent_message_with_projection(&result.message_id, "claim-fork-result")
            .unwrap();
        let fork_boundary_at = now_ms().saturating_add(1);
        {
            let connection = fixture.service.state.connection().unwrap();
            connection
                .execute(
                    "INSERT INTO messages (
                         id, conversation_id, role, content, status, created_at, position
                     ) VALUES (
                         'root-assistant-after-result', 'root-conversation', 'assistant',
                         'combined answer', 'completed', ?1,
                         (SELECT COALESCE(MAX(position), -1) + 1 FROM messages
                          WHERE conversation_id = 'root-conversation')
                     )",
                    [fork_boundary_at],
                )
                .unwrap();
        }
        let request = ForkConversationRequest {
            request_id: "collaboration-root-fork".to_string(),
            source_conversation_id: "root-conversation".to_string(),
            fork_point: ConversationForkPoint::AssistantReply {
                assistant_message_id: "root-assistant-after-result".to_string(),
            },
        };

        let created = fixture
            .service
            .fork_conversation_request_view(request.clone())
            .unwrap();
        assert!(created
            .conversation
            .messages
            .iter()
            .all(|message| message.content != "internal child evidence"));
        let target_id = created.conversation.id.clone();
        let target_root = fixture
            .service
            .get_agent_node_by_conversation(&target_id)
            .unwrap()
            .expect("fork target is bound to an independent root");
        assert_eq!(target_root.parent_agent_id, None);
        assert_eq!(target_root.root_conversation_id, target_id);
        assert_ne!(target_root.root_agent_id, "agent-root");
        assert_eq!(target_root.project_id.as_deref(), Some("project-a"));

        let raw_target = fixture
            .service
            .load_conversation(&target_id)
            .unwrap()
            .unwrap();
        let copied_agent_input = raw_target
            .messages
            .iter()
            .find(|message| message.content == "internal child evidence")
            .expect("transport fact remains available to context assembly");
        assert_eq!(
            fixture
                .service
                .conversation_message_origin(&target_id, &copied_agent_input.id)
                .unwrap(),
            ConversationMessageOrigin::HistoricalSnapshot {
                source_conversation_id: "root-conversation".to_string(),
                source_message_id: result.projection_message_id.clone(),
                original: Box::new(ConversationMessageOrigin::Agent {
                    sender_agent_id: child.agent.agent_id.clone(),
                    source_agent_message_id: result.message_id.clone(),
                }),
            }
        );
        assert_eq!(
            fixture.service.list_agent_tree("agent-root").unwrap().len(),
            2
        );
        let target_tree = fixture
            .service
            .list_agent_tree(&target_root.root_agent_id)
            .unwrap();
        assert_eq!(target_tree.len(), 2);
        let target_child = target_tree
            .iter()
            .find(|agent| agent.parent_agent_id.is_some())
            .expect("visible source child is cloned into the independent target tree")
            .clone();
        assert_eq!(
            target_child.parent_agent_id.as_deref(),
            Some(target_root.agent_id.as_str())
        );
        assert_eq!(target_child.task_path, child.agent.task_path);
        assert_ne!(target_child.agent_id, child.agent.agent_id);
        assert_ne!(target_child.conversation_id, child.agent.conversation_id);
        // The immutable receipt remains the idempotency truth even if lifecycle display state
        // changes after the successful fork. Admission required an active source at creation;
        // retry must not attempt a second target or depend on mutable lifecycle.
        fixture
            .service
            .transition_agent_lifecycle(
                &target_child.agent_id,
                target_child.revision,
                AgentLifecycle::Active,
                AgentLifecycle::Disabled,
            )
            .unwrap();
        let target_root = fixture
            .service
            .transition_agent_lifecycle(
                &target_root.agent_id,
                target_root.revision,
                AgentLifecycle::Active,
                AgentLifecycle::Disabled,
            )
            .unwrap();

        let reopened =
            StorageService::open(&fixture._directory.path().join("storage.sqlite")).unwrap();
        let retried = reopened.fork_conversation_request_view(request).unwrap();
        assert_eq!(retried.conversation.id, target_id);
        assert!(retried
            .conversation
            .messages
            .iter()
            .all(|message| message.content != "internal child evidence"));
        assert_eq!(
            reopened
                .get_agent_node_by_conversation(&target_id)
                .unwrap()
                .unwrap(),
            target_root
        );
        let receipt_update = reopened
            .state
            .connection()
            .unwrap()
            .execute(
                "UPDATE conversation_forks
                 SET source_root_agent_id = target_root_agent_id
                 WHERE request_id = 'collaboration-root-fork'",
                [],
            )
            .unwrap_err();
        assert!(receipt_update
            .to_string()
            .contains("Conversation fork receipt is immutable"));
        let hidden_hits = reopened
            .search_chats(&ChatSearchInput {
                query: "internal child evidence".to_string(),
                limit: Some(10),
            })
            .unwrap();
        assert!(hidden_hits.is_empty());

        let child_fork = reopened
            .fork_conversation_request_view(ForkConversationRequest {
                request_id: "forbidden-child-fork".to_string(),
                source_conversation_id: child.agent.conversation_id,
                fork_point: ConversationForkPoint::AssistantReply {
                    assistant_message_id: "missing".to_string(),
                },
            })
            .unwrap_err();
        assert!(child_fork.message().contains("子 Agent 保持只读"));
    }

    #[test]
    fn deleting_a_root_conversation_removes_its_child_tree_but_preserves_an_independent_fork() {
        let fixture = Fixture::new(Some("model-a"));
        save_settled_history(&fixture, 1, false);
        let child = fixture
            .service
            .create_child_agent(&spawn_input("delete-tree-child", "delete_tree_child"))
            .unwrap();
        let forked = fixture
            .service
            .fork_conversation_request_view(ForkConversationRequest {
                request_id: "delete-tree-independent-fork".to_string(),
                source_conversation_id: "root-conversation".to_string(),
                fork_point: ConversationForkPoint::AssistantReply {
                    assistant_message_id: "root-assistant-0".to_string(),
                },
            })
            .unwrap();
        let forked_root = fixture
            .service
            .get_agent_node_by_conversation(&forked.conversation.id)
            .unwrap()
            .unwrap();

        let child_delete_error = fixture
            .service
            .delete_conversation(&child.agent.conversation_id)
            .unwrap_err();
        assert!(child_delete_error.contains("owned by their root task"));

        fixture
            .service
            .delete_conversation("root-conversation")
            .unwrap();
        assert!(fixture
            .service
            .load_conversation("root-conversation")
            .unwrap()
            .is_none());
        assert!(fixture
            .service
            .load_conversation(&child.agent.conversation_id)
            .unwrap()
            .is_none());
        assert!(fixture
            .service
            .get_agent_node("agent-root")
            .unwrap()
            .is_none());
        assert!(fixture
            .service
            .get_agent_node(&child.agent.agent_id)
            .unwrap()
            .is_none());

        assert!(fixture
            .service
            .load_conversation(&forked.conversation.id)
            .unwrap()
            .is_some());
        assert_eq!(
            fixture
                .service
                .get_agent_node(&forked_root.agent_id)
                .unwrap(),
            Some(forked_root)
        );
        let connection = fixture.service.state.connection().unwrap();
        let violations = connection
            .prepare("PRAGMA foreign_key_check")
            .unwrap()
            .query_map([], |_| Ok(()))
            .unwrap()
            .count();
        assert_eq!(violations, 0);
    }

    #[test]
    fn active_root_turn_rejects_fork_without_writing_target_or_receipt() {
        let fixture = Fixture::new(Some("model-a"));
        save_settled_history(&fixture, 1, true);
        let before = {
            let connection = fixture.service.state.connection().unwrap();
            (
                connection
                    .query_row("SELECT COUNT(*) FROM conversations", [], |row| {
                        row.get::<_, u64>(0)
                    })
                    .unwrap(),
                connection
                    .query_row("SELECT COUNT(*) FROM agent_nodes", [], |row| {
                        row.get::<_, u64>(0)
                    })
                    .unwrap(),
                connection
                    .query_row("SELECT COUNT(*) FROM conversation_forks", [], |row| {
                        row.get::<_, u64>(0)
                    })
                    .unwrap(),
            )
        };
        let error = fixture
            .service
            .fork_conversation_request_view(ForkConversationRequest {
                request_id: "active-root-fork".to_string(),
                source_conversation_id: "root-conversation".to_string(),
                fork_point: ConversationForkPoint::AssistantReply {
                    assistant_message_id: "root-assistant-0".to_string(),
                },
            })
            .unwrap_err();
        assert!(error.message().contains("活跃 Turn"));
        let after = {
            let connection = fixture.service.state.connection().unwrap();
            (
                connection
                    .query_row("SELECT COUNT(*) FROM conversations", [], |row| {
                        row.get::<_, u64>(0)
                    })
                    .unwrap(),
                connection
                    .query_row("SELECT COUNT(*) FROM agent_nodes", [], |row| {
                        row.get::<_, u64>(0)
                    })
                    .unwrap(),
                connection
                    .query_row("SELECT COUNT(*) FROM conversation_forks", [], |row| {
                        row.get::<_, u64>(0)
                    })
                    .unwrap(),
            )
        };
        assert_eq!(after, before);
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

    #[test]
    fn atomic_tree_limits_preserve_idempotent_retry_and_reject_depth_nodes_and_task_bytes() {
        let fixture = Fixture::new(Some("model-a"));
        let limits = AgentTreeResourceLimits {
            max_depth: 2,
            max_nodes: 2,
            max_task_bytes: 64,
        };
        let first_input = spawn_input("spawn-limited-first", "first");
        let first = fixture
            .service
            .create_child_agent_with_limits(&first_input, limits)
            .unwrap();
        let retry = fixture
            .service
            .create_child_agent_with_limits(
                &first_input,
                AgentTreeResourceLimits {
                    max_depth: 1,
                    max_nodes: 2,
                    max_task_bytes: 1,
                },
            )
            .unwrap();
        assert_eq!(retry.agent.agent_id, first.agent.agent_id);

        let node_error = fixture
            .service
            .create_child_agent_with_limits(&spawn_input("spawn-limited-second", "second"), limits)
            .unwrap_err();
        assert_eq!(
            node_error,
            ChildAgentSpawnError::ResourceLimit {
                resource: "tree_nodes",
                limit: 2,
            }
        );

        let depth_fixture = Fixture::new(Some("model-a"));
        let child = depth_fixture
            .service
            .create_child_agent_with_limits(
                &spawn_input("spawn-depth-parent", "depth_parent"),
                AgentTreeResourceLimits {
                    max_depth: 1,
                    max_nodes: 8,
                    max_task_bytes: 64,
                },
            )
            .unwrap();
        let mut grandchild_input = spawn_input("spawn-depth-child", "depth_child");
        grandchild_input.parent_agent_id = child.agent.agent_id;
        let depth_error = depth_fixture
            .service
            .create_child_agent_with_limits(
                &grandchild_input,
                AgentTreeResourceLimits {
                    max_depth: 1,
                    max_nodes: 8,
                    max_task_bytes: 64,
                },
            )
            .unwrap_err();
        assert_eq!(
            depth_error,
            ChildAgentSpawnError::ResourceLimit {
                resource: "tree_depth",
                limit: 1,
            }
        );

        let mut oversized = spawn_input("spawn-oversized-task", "oversized");
        oversized.task = "x".repeat(65);
        assert!(matches!(
            depth_fixture.service.create_child_agent_with_limits(
                &oversized,
                AgentTreeResourceLimits {
                    max_depth: 2,
                    max_nodes: 8,
                    max_task_bytes: 64,
                },
            ),
            Err(ChildAgentSpawnError::ResourceLimit {
                resource: "task_bytes",
                limit: 64,
            })
        ));
    }

    /// Deterministic release profile for the collaboration persistence boundary.
    ///
    /// Kept ignored in the ordinary unit suite because it intentionally performs at least ten
    /// thousand durable SQLite commits. The repository release-gate script runs it explicitly and
    /// rejects environment overrides below the documented minimums.
    #[test]
    #[ignore = "run with pnpm test:multi-agent-release"]
    fn release_profile_tree_mailbox_event_contention_and_restart_recovery() {
        const CONFIGURED_TREE_NODES: usize = 64;
        const WRITERS: usize = 4;
        let fact_count = release_profile_usize("MYCOPILOT_MULTI_AGENT_PROFILE_FACTS", 10_000);
        let restart_count = release_profile_usize("MYCOPILOT_MULTI_AGENT_PROFILE_RESTARTS", 20);
        assert!(
            fact_count >= 10_000,
            "release profile requires >=10000 facts"
        );
        assert!(
            restart_count >= 20,
            "release profile requires >=20 restarts"
        );

        let profile_started = Instant::now();
        let fixture = Fixture::new(Some("model-a"));
        let database_path = fixture._directory.path().join("storage.sqlite");
        let limits = AgentTreeResourceLimits::default();
        assert_eq!(limits.max_nodes as usize, CONFIGURED_TREE_NODES);

        let mut children = Vec::with_capacity(CONFIGURED_TREE_NODES - 1);
        for index in 0..(CONFIGURED_TREE_NODES - 1) {
            let created = fixture
                .service
                .create_child_agent_with_limits(
                    &spawn_input(
                        &format!("release-spawn-{index}"),
                        &format!("release_child_{index:02}"),
                    ),
                    limits,
                )
                .unwrap();
            children.push(created);
        }
        assert_eq!(
            fixture.service.list_agent_tree("agent-root").unwrap().len(),
            64
        );
        assert_eq!(
            fixture
                .service
                .create_child_agent_with_limits(
                    &spawn_input("release-spawn-over-limit", "release_child_over_limit"),
                    limits,
                )
                .unwrap_err(),
            ChildAgentSpawnError::ResourceLimit {
                resource: "tree_nodes",
                limit: 64,
            }
        );

        // Settle every initial Wake through the durable result/outbox transaction. The root stays
        // idle: results are pending Mailbox facts and never create a root Wake.
        for (index, child) in children.iter().enumerate() {
            let token = format!("release-claim-{index}");
            let claimed = fixture
                .service
                .claim_next_agent_wake(&child.agent.agent_id, &token)
                .unwrap()
                .unwrap();
            fixture
                .service
                .transition_agent_wake(
                    &claimed.wake_id,
                    AgentWakeStatus::Claimed,
                    AgentWakeStatus::Running,
                    Some(&token),
                )
                .unwrap();
            fixture
                .service
                .finish_agent_wake_with_result(&FinishAgentWakeWithResultInput {
                    wake_id: claimed.wake_id,
                    expected_status: AgentWakeStatus::Running,
                    claim_token: token,
                    terminal_status: AgentWakeStatus::Completed,
                    terminal_error: None,
                    result_message: EnqueueAgentMessageInput {
                        message_id: format!("release-result-{index}"),
                        root_agent_id: "agent-root".to_string(),
                        sender_agent_id: child.agent.agent_id.clone(),
                        recipient_agent_id: "agent-root".to_string(),
                        request_id: format!("release-result-request-{index}"),
                        kind: AgentMailboxKind::Result,
                        content: format!("result {index}"),
                        projection_message_id: format!("release-result-projection-{index}"),
                    },
                })
                .unwrap();
        }

        let child_ids = Arc::new(
            children
                .iter()
                .map(|child| child.agent.agent_id.clone())
                .collect::<Vec<_>>(),
        );
        let errors = Arc::new(AtomicUsize::new(0));
        let busy_errors = Arc::new(AtomicUsize::new(0));
        let maximum_enqueue_micros = Arc::new(AtomicU64::new(0));
        std::thread::scope(|scope| {
            for writer_index in 0..WRITERS {
                // Production serializes writes through StorageState's SQLite mutex. Four caller
                // threads still exercise the public contention boundary without inventing a
                // second in-process database owner that Core Server never creates.
                let service = &fixture.service;
                let child_ids = Arc::clone(&child_ids);
                let errors = Arc::clone(&errors);
                let busy_errors = Arc::clone(&busy_errors);
                let maximum_enqueue_micros = Arc::clone(&maximum_enqueue_micros);
                scope.spawn(move || {
                    for sequence in (writer_index..fact_count).step_by(WRITERS) {
                        let input = SendAgentMessageRequest {
                            sender_agent_id: "agent-root".to_string(),
                            recipient_agent_id: child_ids[sequence % child_ids.len()].clone(),
                            request_id: format!("release-message-{sequence}"),
                            content: format!("release payload {sequence}"),
                        };
                        let started = Instant::now();
                        if let Err(error) = service.send_agent_message(&input) {
                            errors.fetch_add(1, Ordering::Relaxed);
                            if matches!(&error, AgentGraphError::StorageUnavailable(message) if message.contains("busy") || message.contains("locked"))
                            {
                                busy_errors.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                        maximum_enqueue_micros.fetch_max(
                            started.elapsed().as_micros().try_into().unwrap_or(u64::MAX),
                            Ordering::Relaxed,
                        );
                    }
                });
            }
        });
        assert_eq!(
            errors.load(Ordering::Relaxed),
            0,
            "all durable sends converge (sqlite_busy_errors={})",
            busy_errors.load(Ordering::Relaxed)
        );
        assert_eq!(
            busy_errors.load(Ordering::Relaxed),
            0,
            "no SQLITE_BUSY escapes"
        );

        // Retry a deterministic sample after the concurrent writers have committed. Idempotency
        // returns the original fact and does not advance the event sequence.
        for sequence in (0..fact_count).step_by(997) {
            let dispatch = fixture
                .service
                .send_agent_message(&SendAgentMessageRequest {
                    sender_agent_id: "agent-root".to_string(),
                    recipient_agent_id: child_ids[sequence % child_ids.len()].clone(),
                    request_id: format!("release-message-{sequence}"),
                    content: format!("release payload {sequence}"),
                })
                .unwrap();
            assert_eq!(
                dispatch.message.request_id,
                format!("release-message-{sequence}")
            );
        }

        // Twenty deterministic pre-dispatch crashes. Each cycle leaves one claimed Wake behind,
        // opens a new StorageService (the process-restart boundary), advances the fake recovery
        // clock past the half-open lease, reclaims the same durable Wake, and cancels it without a
        // second Turn. This is deliberately stronger than repeatedly opening an idle database.
        let restart_clock_base = now_ms();
        let mut maximum_restart_micros = 0_u128;
        for restart in 0..restart_count {
            let target = &child_ids[restart % child_ids.len()];
            fixture
                .service
                .follow_up_agent(&SendAgentMessageRequest {
                    sender_agent_id: "agent-root".to_string(),
                    recipient_agent_id: target.clone(),
                    request_id: format!("release-restart-followup-{restart}"),
                    content: format!("recover this durable follow-up {restart}"),
                })
                .unwrap();
            let claimed_at = restart_clock_base + (restart as i64 * 100_000);
            let stale_claim = format!("release-stale-claim-{restart}");
            let claimed = fixture
                .service
                .claim_next_dispatchable_agent_wake_at(&stale_claim, claimed_at)
                .unwrap()
                .unwrap();
            assert_eq!(claimed.agent_id, *target);

            let started = Instant::now();
            let reopened = StorageService::open(&database_path).unwrap();
            let recovered_at = claimed_at + 60_001;
            let recovered = reopened
                .recover_agent_wakes_at(&format!("release-recovery-{restart}"), recovered_at)
                .unwrap();
            assert_eq!(recovered.requeued_before_dispatch, 1);
            assert!(recovered.actions.is_empty());
            let retry_claim = format!("release-retry-claim-{restart}");
            let retry = reopened
                .claim_next_dispatchable_agent_wake_at(&retry_claim, recovered_at + 1)
                .unwrap()
                .unwrap();
            assert_eq!(retry.wake_id, claimed.wake_id);
            reopened
                .transition_agent_wake_at(
                    &retry.wake_id,
                    AgentWakeStatus::Claimed,
                    AgentWakeStatus::Cancelled,
                    Some(&retry_claim),
                    recovered_at + 2,
                )
                .unwrap();
            maximum_restart_micros = maximum_restart_micros.max(started.elapsed().as_micros());
        }

        let query_started = Instant::now();
        let connection = fixture.service.state.connection().unwrap();
        let ordinary_messages = connection
            .query_row(
                "SELECT COUNT(*) FROM agent_mailbox_messages WHERE kind = 'message'",
                [],
                |row| row.get::<_, usize>(0),
            )
            .unwrap();
        let result_messages = connection
            .query_row(
                "SELECT COUNT(*) FROM agent_mailbox_messages WHERE kind = 'result'",
                [],
                |row| row.get::<_, usize>(0),
            )
            .unwrap();
        let (event_count, maximum_event_sequence) = connection
            .query_row(
                "SELECT COUNT(*), COALESCE(MAX(root_sequence), 0)
                   FROM agent_collaboration_events
                  WHERE root_agent_id = 'agent-root'",
                [],
                |row| Ok((row.get::<_, usize>(0)?, row.get::<_, usize>(1)?)),
            )
            .unwrap();
        let ghost_wakes = connection
            .query_row(
                "SELECT COUNT(*) FROM agent_wake_requests
                  WHERE status IN ('claimed', 'running', 'waiting_for_approval')",
                [],
                |row| row.get::<_, usize>(0),
            )
            .unwrap();
        let query_micros = query_started.elapsed().as_micros();
        drop(connection);

        assert_eq!(ordinary_messages, fact_count);
        assert_eq!(result_messages, CONFIGURED_TREE_NODES - 1);
        assert!(
            event_count >= fact_count,
            "every message commit invalidates the tree"
        );
        assert_eq!(
            event_count, maximum_event_sequence,
            "root sequence has no gap"
        );
        assert_eq!(ghost_wakes, 0);

        let event_catchup_started = Instant::now();
        let mut event_cursor = 0_u64;
        let mut observed_events = 0_usize;
        let mut maximum_event_page_micros = 0_u128;
        loop {
            let page_started = Instant::now();
            let page = fixture
                .service
                .list_agent_collaboration_events("agent-root", event_cursor, 512)
                .unwrap();
            maximum_event_page_micros =
                maximum_event_page_micros.max(page_started.elapsed().as_micros());
            if page.is_empty() {
                break;
            }
            for event in &page {
                event_cursor += 1;
                assert_eq!(event.root_sequence, event_cursor, "catch-up sequence gap");
            }
            observed_events += page.len();
        }
        let event_catchup_micros = event_catchup_started.elapsed().as_micros();
        assert_eq!(observed_events, event_count);

        let reopened = StorageService::open(&database_path).unwrap();
        assert_eq!(reopened.list_agent_tree("agent-root").unwrap().len(), 64);
        assert!(
            reopened
                .latest_agent_collaboration_event_sequence("agent-root")
                .unwrap()
                >= fact_count as u64
        );

        let database_bytes = std::fs::metadata(&database_path).unwrap().len();
        eprintln!(
            "ROUND6_METRIC tree_nodes=64 facts={fact_count} results={result_messages} events={event_count} writers={WRITERS} sqlite_busy_errors=0 max_enqueue_us={} query_us={query_micros} event_catchup_us={event_catchup_micros} max_event_page_us={maximum_event_page_micros} restarts={restart_count} max_restart_us={maximum_restart_micros} database_bytes={database_bytes} wall_ms={}",
            maximum_enqueue_micros.load(Ordering::Relaxed),
            profile_started.elapsed().as_millis(),
        );
    }

    fn release_profile_usize(name: &str, default: usize) -> usize {
        std::env::var(name)
            .ok()
            .map(|value| {
                value
                    .parse::<usize>()
                    .unwrap_or_else(|_| panic!("{name} must be a positive integer"))
            })
            .unwrap_or(default)
    }
}
