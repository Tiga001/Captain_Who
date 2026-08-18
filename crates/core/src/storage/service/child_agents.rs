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
mod tests;
