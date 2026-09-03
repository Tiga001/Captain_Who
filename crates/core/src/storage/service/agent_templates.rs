use super::*;
use crate::{
    AgentModelSelectionSnapshot, AgentModelUnavailableReason, AgentTemplateError,
    AgentTemplateModelUnavailableReason, AgentTemplateRecord, AgentTemplateSnapshot,
    CreateAgentTemplateInput, ResolvedAgentTemplateForSpawn, UpdateAgentTemplateInput,
};

impl StorageService {
    pub fn create_agent_template(
        &self,
        input: &CreateAgentTemplateInput,
    ) -> Result<AgentTemplateRecord, AgentTemplateError> {
        let mut connection = self.template_connection()?;
        agent_template_repository::create_template(&mut connection, input)
    }

    pub fn list_agent_templates(
        &self,
        include_disabled: bool,
    ) -> Result<Vec<AgentTemplateRecord>, AgentTemplateError> {
        let connection = self.template_connection()?;
        agent_template_repository::list_templates(&connection, include_disabled)
    }

    pub fn get_agent_template(
        &self,
        template_id: &str,
    ) -> Result<AgentTemplateRecord, AgentTemplateError> {
        let connection = self.template_connection()?;
        agent_template_repository::get_template(&connection, template_id)
    }

    pub fn list_project_agent_templates(
        &self,
        project_id: &str,
        include_disabled: bool,
    ) -> Result<Vec<AgentTemplateRecord>, AgentTemplateError> {
        let connection = self.template_connection()?;
        agent_template_repository::list_project_templates(&connection, project_id, include_disabled)
    }

    pub fn get_project_agent_template_by_machine_key(
        &self,
        project_id: &str,
        machine_key: &str,
    ) -> Result<AgentTemplateRecord, AgentTemplateError> {
        let connection = self.template_connection()?;
        agent_template_repository::get_project_template_by_machine_key(
            &connection,
            project_id,
            machine_key,
        )
    }

    pub fn update_agent_template(
        &self,
        input: &UpdateAgentTemplateInput,
    ) -> Result<AgentTemplateRecord, AgentTemplateError> {
        let mut connection = self.template_connection()?;
        agent_template_repository::update_template(&mut connection, input)
    }

    pub fn set_agent_template_enabled(
        &self,
        template_id: &str,
        expected_revision: u64,
        enabled: bool,
    ) -> Result<AgentTemplateRecord, AgentTemplateError> {
        let mut connection = self.template_connection()?;
        agent_template_repository::set_template_enabled(
            &mut connection,
            template_id,
            expected_revision,
            enabled,
        )
    }

    pub fn delete_agent_template(
        &self,
        template_id: &str,
        expected_revision: u64,
    ) -> Result<AgentTemplateRecord, AgentTemplateError> {
        let mut connection = self.template_connection()?;
        agent_template_repository::delete_template(&mut connection, template_id, expected_revision)
    }

    pub fn set_agent_template_project_assignment(
        &self,
        project_id: &str,
        template_id: &str,
        assigned: bool,
    ) -> Result<AgentTemplateRecord, AgentTemplateError> {
        let mut connection = self.template_connection()?;
        agent_template_repository::set_template_project_assignment(
            &mut connection,
            project_id,
            template_id,
            assigned,
        )
    }

    /// Resolves an exact, enabled template and freezes its credential-free spawn identity.
    ///
    /// This is the only template-to-model resolution boundary. It never chooses another model and
    /// never exposes connection details, credentials, prices, or Provider-specific configuration.
    pub fn resolve_template_for_spawn(
        &self,
        project_id: &str,
        machine_key: &str,
    ) -> Result<ResolvedAgentTemplateForSpawn, AgentTemplateError> {
        // Keep both reads under the single StorageState mutex. Model settings already use their own
        // SQLite read transaction, and no in-process settings/template mutation can interleave.
        let connection = self.template_connection()?;
        let template = agent_template_repository::get_project_template_by_machine_key(
            &connection,
            project_id,
            machine_key,
        )?;
        if !template.enabled {
            return Err(AgentTemplateError::TemplateDisabled(
                template.machine_key.clone(),
            ));
        }
        let settings = self
            .model_settings_catalog_snapshot_in_connection(&connection)
            .map_err(|_| template_storage_unavailable())?
            .ok_or_else(|| {
                model_unavailable(
                    &template,
                    AgentTemplateModelUnavailableReason::SettingsMissing,
                )
            })?;
        let model = resolve_model_selection(&template, &settings)?;
        Ok(ResolvedAgentTemplateForSpawn {
            template: AgentTemplateSnapshot {
                template_id: template.template_id,
                project_id: project_id.to_string(),
                machine_key: template.machine_key,
                name: template.name,
                description: template.description,
                instructions: template.instructions,
                template_revision: template.revision,
                model_config_id: template.model_config_id,
            },
            model,
        })
    }

    fn template_connection(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, rusqlite::Connection>, AgentTemplateError> {
        self.state
            .connection()
            .map_err(|_| template_storage_unavailable())
    }
}

fn resolve_model_selection(
    template: &AgentTemplateRecord,
    snapshot: &ModelSettingsSnapshot,
) -> Result<AgentModelSelectionSnapshot, AgentTemplateError> {
    resolve_exact_agent_model(snapshot, &template.model_config_id)
        .map_err(|reason| model_unavailable(template, reason))
}

/// Resolves one exact configured model to the credential-free identity frozen on an Agent node.
///
/// This function never chooses a fallback. Callers own selection priority; once an identity has
/// been selected, every unavailable condition is terminal for that spawn attempt.
pub(crate) fn resolve_exact_agent_model(
    snapshot: &ModelSettingsSnapshot,
    model_config_id: &str,
) -> Result<AgentModelSelectionSnapshot, AgentModelUnavailableReason> {
    let model = snapshot
        .settings
        .models
        .iter()
        .find(|model| model.id == model_config_id)
        .ok_or(AgentModelUnavailableReason::NotFound)?;
    if !model.enabled {
        return Err(AgentModelUnavailableReason::Disabled);
    }
    let connection = snapshot
        .settings
        .effective_connection_for(model)
        .map_err(|_| AgentModelUnavailableReason::InvalidConnection)?;
    let dialect = crate::ProviderProtocolDialect::detect_from_api_url(&connection.api_url);
    let profile = model
        .resolved_provider_profile_config(dialect)
        .map_err(|_| AgentModelUnavailableReason::InvalidProfile)?;
    let provider_connection_revision = snapshot
        .provider_connection_revisions
        .get(&model.id)
        .cloned()
        .ok_or(AgentModelUnavailableReason::MissingConnectionIdentity)?;
    let provider_protocol_revision = snapshot
        .provider_protocol_revisions
        .get(&model.id)
        .cloned()
        .ok_or(AgentModelUnavailableReason::MissingProtocolIdentity)?;
    let protocol = crate::ProviderProtocolKey::new(
        dialect,
        &profile,
        model.provider_model_id.clone(),
        Some(provider_protocol_revision.clone()),
    )
    .map_err(|_| AgentModelUnavailableReason::InvalidProfile)?;
    crate::resolve_provider_runtime_capabilities(&protocol)
        .map_err(|_| AgentModelUnavailableReason::UnsupportedRuntime)?;

    Ok(AgentModelSelectionSnapshot {
        model_config_id: model.id.clone(),
        display_name: model.display_label(),
        supports_image: model.supports_image,
        effective_context_window_tokens: model.effective_context_window_tokens(),
        model_settings_configuration_revision: snapshot.configuration_revision.clone(),
        provider_connection_revision,
        provider_protocol_revision,
    })
}

fn model_unavailable(
    template: &AgentTemplateRecord,
    reason: AgentTemplateModelUnavailableReason,
) -> AgentTemplateError {
    AgentTemplateError::ModelUnavailable {
        model_config_id: template.model_config_id.clone(),
        reason,
    }
}

fn template_storage_unavailable() -> AgentTemplateError {
    AgentTemplateError::StorageUnavailable("database operation failed".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::models::{
        ChatConversationRecord, ModelConfigRecord, ModelSettingsRecord, ProjectRecord,
    };
    use crate::{
        AgentGraphError, CreateAgentNodeInput, EnsureRootAgentInput, ProviderProfileConfig,
        ProviderProtocolDialect,
    };
    use std::collections::BTreeMap;
    use tempfile::TempDir;

    struct Fixture {
        _directory: TempDir,
        service: StorageService,
    }

    impl Fixture {
        fn new() -> Self {
            let directory = tempfile::tempdir().unwrap();
            let service = StorageService::open(&directory.path().join("storage.sqlite")).unwrap();
            for project_id in ["project-a", "project-b"] {
                service
                    .save_project(ProjectRecord {
                        id: project_id.to_string(),
                        name: project_id.to_string(),
                        path: None,
                        created_at: 1,
                        pinned_at: None,
                    })
                    .unwrap();
            }
            Self {
                _directory: directory,
                service,
            }
        }
    }

    fn create_input(
        template_id: &str,
        _project_id: &str,
        machine_key: &str,
        name: &str,
        model_config_id: &str,
    ) -> CreateAgentTemplateInput {
        CreateAgentTemplateInput {
            template_id: template_id.to_string(),
            machine_key: machine_key.to_string(),
            name: name.to_string(),
            description: "Focused reviewer".to_string(),
            instructions: "Review the assigned scope and report evidence.".to_string(),
            model_config_id: model_config_id.to_string(),
            enabled: true,
        }
    }

    fn model(model_id: &str, enabled: bool) -> ModelConfigRecord {
        ModelConfigRecord {
            id: model_id.to_string(),
            provider_model_id: model_id.to_string(),
            display_name: format!("Display {model_id}"),
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

    fn settings(models: Vec<ModelConfigRecord>) -> ModelSettingsRecord {
        ModelSettingsRecord {
            api_url: "https://provider.example/v1/chat/completions".to_string(),
            api_token: "owned-secret-token".to_string(),
            search_mode: "disabled".to_string(),
            tavily_api_key: String::new(),
            models,
        }
    }

    fn empty_conversation(id: &str, project_id: &str) -> ChatConversationRecord {
        empty_conversation_with_model(id, project_id, "model-a")
    }

    fn empty_conversation_with_model(
        id: &str,
        project_id: &str,
        model_id: &str,
    ) -> ChatConversationRecord {
        ChatConversationRecord {
            id: id.to_string(),
            project_id: Some(project_id.to_string()),
            model_id: Some(model_id.to_string()),
            title: id.to_string(),
            messages: Vec::new(),
            created_at: 1,
            updated_at: 1,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        }
    }

    fn assign(service: &StorageService, project_id: &str, template_id: &str) {
        service
            .set_agent_template_project_assignment(project_id, template_id, true)
            .unwrap();
    }

    #[test]
    fn global_crud_owns_machine_identity_assignments_and_compare_and_set_revision() {
        let fixture = Fixture::new();
        let service = &fixture.service;
        let first = service
            .create_agent_template(&create_input(
                "template-1",
                "project-a",
                "security_review",
                "Security review",
                "model-a",
            ))
            .unwrap();
        assert_eq!(first.revision, 1);
        assert_eq!(first.machine_key, "security_review");
        assert!(first.project_ids.is_empty());
        assert_eq!(
            service.list_agent_templates(false).unwrap(),
            vec![first.clone()]
        );
        assert_eq!(service.get_agent_template("template-1").unwrap(), first);
        let assigned = service
            .set_agent_template_project_assignment("project-a", "template-1", true)
            .unwrap();
        assert_eq!(assigned.project_ids, vec!["project-a"]);
        let assigned_again = service
            .set_agent_template_project_assignment("project-a", "template-1", true)
            .unwrap();
        assert_eq!(assigned_again, assigned);

        let machine_conflict = service
            .create_agent_template(&create_input(
                "template-2",
                "project-a",
                "security_review",
                "Other name",
                "model-a",
            ))
            .unwrap_err();
        assert_eq!(
            machine_conflict,
            AgentTemplateError::MachineKeyConflict("security_review".to_string())
        );
        let name_conflict = service
            .create_agent_template(&create_input(
                "template-2",
                "project-a",
                "other_reviewer",
                "Security review",
                "model-a",
            ))
            .unwrap_err();
        assert_eq!(
            name_conflict,
            AgentTemplateError::NameConflict("Security review".to_string())
        );
        let second = service
            .create_agent_template(&create_input(
                "template-2",
                "project-a",
                "other_reviewer",
                "Other reviewer",
                "model-a",
            ))
            .unwrap();

        let updated = service
            .update_agent_template(&UpdateAgentTemplateInput {
                template_id: "template-1".to_string(),
                expected_revision: 1,
                name: "Security specialist".to_string(),
                description: "Updated".to_string(),
                instructions: "Return file paths and concise findings.".to_string(),
                model_config_id: "model-b".to_string(),
            })
            .unwrap();
        assert_eq!(updated.revision, 2);
        assert_eq!(updated.machine_key, "security_review");
        assert_eq!(
            service
                .update_agent_template(&UpdateAgentTemplateInput {
                    template_id: "template-1".to_string(),
                    expected_revision: 2,
                    name: second.name,
                    description: "Must roll back".to_string(),
                    instructions: "This conflicting edit must not be stored.".to_string(),
                    model_config_id: "model-a".to_string(),
                })
                .unwrap_err(),
            AgentTemplateError::NameConflict("Other reviewer".to_string())
        );
        assert_eq!(
            service
                .update_agent_template(&UpdateAgentTemplateInput {
                    template_id: "template-1".to_string(),
                    expected_revision: 1,
                    name: "Stale".to_string(),
                    description: String::new(),
                    instructions: "This must not be stored.".to_string(),
                    model_config_id: "model-a".to_string(),
                })
                .unwrap_err(),
            AgentTemplateError::RevisionConflict {
                expected: 1,
                current: 2
            }
        );
        assert_eq!(service.get_agent_template("template-1").unwrap(), updated);

        let disabled = service
            .set_agent_template_enabled("template-1", 2, false)
            .unwrap();
        assert_eq!(disabled.revision, 3);
        assert!(!disabled.enabled);
        assert_eq!(
            service
                .resolve_template_for_spawn("project-a", "security_review")
                .unwrap_err(),
            AgentTemplateError::TemplateDisabled("security_review".to_string())
        );
        assert_eq!(service.list_agent_templates(false).unwrap().len(), 1);
        assert_eq!(service.list_agent_templates(true).unwrap().len(), 2);
        assert!(service
            .list_project_agent_templates("project-a", false)
            .unwrap()
            .is_empty());
        assert_eq!(
            service
                .list_project_agent_templates("project-a", true)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            service.delete_agent_template("template-1", 2).unwrap_err(),
            AgentTemplateError::RevisionConflict {
                expected: 2,
                current: 3
            }
        );
        service.delete_agent_template("template-1", 3).unwrap();
        assert_eq!(
            service.get_agent_template("template-1").unwrap_err(),
            AgentTemplateError::TemplateNotFound("template-1".to_string())
        );
        assert!(service
            .list_project_agent_templates("project-a", true)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn project_delete_cascades_only_bindings_and_preserves_global_templates() {
        let fixture = Fixture::new();
        let service = &fixture.service;
        let template = service
            .create_agent_template(&create_input(
                "template-a",
                "project-a",
                "reviewer",
                "Reviewer A",
                "model-a",
            ))
            .unwrap();
        service
            .set_agent_template_project_assignment("project-a", &template.template_id, true)
            .unwrap();
        service
            .set_agent_template_project_assignment("project-b", &template.template_id, true)
            .unwrap();

        service.delete_project("project-a").unwrap();

        assert_eq!(
            service
                .list_project_agent_templates("project-a", true)
                .unwrap_err(),
            AgentTemplateError::ProjectNotFound("project-a".to_string())
        );
        let preserved = service.get_agent_template("template-a").unwrap();
        assert_eq!(preserved.project_ids, vec!["project-b"]);
        assert_eq!(service.list_agent_templates(true).unwrap(), vec![preserved]);
    }

    #[test]
    fn project_template_assignments_are_idempotent_and_bounded_to_32() {
        let fixture = Fixture::new();
        let service = &fixture.service;
        for index in 0..=32 {
            let template_id = format!("template-{index}");
            service
                .create_agent_template(&create_input(
                    &template_id,
                    "project-a",
                    &format!("reviewer_{index}"),
                    &format!("Reviewer {index}"),
                    "model-a",
                ))
                .unwrap();
            if index < 32 {
                assign(service, "project-a", &template_id);
            }
        }

        let idempotent = service
            .set_agent_template_project_assignment("project-a", "template-0", true)
            .unwrap();
        assert_eq!(idempotent.project_ids, vec!["project-a"]);
        assert_eq!(
            service
                .set_agent_template_project_assignment("project-a", "template-32", true)
                .unwrap_err(),
            AgentTemplateError::ProjectTemplateLimit {
                project_id: "project-a".to_string(),
                limit: 32,
            }
        );
        {
            let connection = service.state.connection().unwrap();
            assert!(connection
                .execute(
                    "INSERT INTO project_agent_template_bindings (
                         project_id, template_id, created_at
                     ) VALUES ('project-a', 'template-32', 1)",
                    [],
                )
                .is_err());
        }

        let unassigned = service
            .set_agent_template_project_assignment("project-a", "template-0", false)
            .unwrap();
        assert!(unassigned.project_ids.is_empty());
        assert_eq!(
            service
                .set_agent_template_project_assignment("project-a", "template-0", false)
                .unwrap(),
            unassigned
        );
        let newly_assigned = service
            .set_agent_template_project_assignment("project-a", "template-32", true)
            .unwrap();
        assert_eq!(newly_assigned.project_ids, vec!["project-a"]);
        assert_eq!(
            service
                .list_project_agent_templates("project-a", true)
                .unwrap()
                .len(),
            32
        );
    }

    #[test]
    fn display_name_uniqueness_is_exact_while_machine_keys_remain_canonical() {
        let fixture = Fixture::new();
        let service = &fixture.service;
        service
            .create_agent_template(&create_input(
                "template-1",
                "project-a",
                "reviewer_one",
                "Reviewer",
                "model-a",
            ))
            .unwrap();
        service
            .create_agent_template(&create_input(
                "template-2",
                "project-a",
                "reviewer_two",
                "reviewer",
                "model-a",
            ))
            .unwrap();

        assert_eq!(service.list_agent_templates(true).unwrap().len(), 2);
    }

    #[test]
    fn spawn_resolution_is_exact_enabled_and_credential_free() {
        let fixture = Fixture::new();
        let service = &fixture.service;
        service
            .save_model_settings(settings(vec![model("model-a", true)]))
            .unwrap();
        service
            .create_agent_template(&create_input(
                "template-1",
                "project-a",
                "reviewer",
                "Reviewer",
                "model-a",
            ))
            .unwrap();
        assert_eq!(
            service
                .resolve_template_for_spawn("project-a", "reviewer")
                .unwrap_err(),
            AgentTemplateError::TemplateNotFound("reviewer".to_string())
        );
        assign(service, "project-a", "template-1");

        let resolved = service
            .resolve_template_for_spawn("project-a", "reviewer")
            .unwrap();
        assert_eq!(resolved.template.template_revision, 1);
        assert_eq!(resolved.model.model_config_id, "model-a");
        assert_eq!(resolved.model.display_name, "Display model-a");
        assert_eq!(resolved.model.effective_context_window_tokens, 64_000);
        assert!(resolved
            .model
            .provider_connection_revision
            .starts_with("provider-connection-v1:"));
        assert!(resolved
            .model
            .provider_protocol_revision
            .starts_with("provider-protocol-v1:"));
        let safe_json = serde_json::to_string(&resolved).unwrap();
        assert!(!safe_json.contains("owned-secret-token"));
        assert!(!safe_json.contains("provider.example"));

        let before = resolved.clone();
        service
            .update_agent_template(&UpdateAgentTemplateInput {
                template_id: "template-1".to_string(),
                expected_revision: 1,
                name: "Renamed reviewer".to_string(),
                description: "New description".to_string(),
                instructions: "New instructions for future Agents.".to_string(),
                model_config_id: "model-a".to_string(),
            })
            .unwrap();
        let mut edited_settings = settings(vec![model("model-a", true)]);
        edited_settings.models[0].display_name = "New display".to_string();
        service.save_model_settings(edited_settings).unwrap();
        let after = service
            .resolve_template_for_spawn("project-a", "reviewer")
            .unwrap();
        assert_eq!(before.template.template_revision, 1);
        assert_eq!(before.template.name, "Reviewer");
        assert_eq!(before.model.display_name, "Display model-a");
        assert_eq!(after.template.template_revision, 2);
        assert_eq!(after.template.name, "Renamed reviewer");
        assert_eq!(after.model.display_name, "New display");
    }

    #[test]
    fn spawn_resolution_uses_provider_model_id_not_config_id_for_protocol_validation() {
        let fixture = Fixture::new();
        let service = &fixture.service;
        let mut configured = model("config-moonshot", true);
        configured.provider_model_id = "kimi-k3".to_string();
        configured.provider_profile_config = crate::ProviderProfileConfig::from_family_settings(
            crate::ProviderProfileRef::moonshot_k3_chat(),
            crate::ProviderVendorId::Moonshot,
            crate::ProviderFamilySettings::MoonshotK3Chat {
                reasoning_effort: crate::ProviderReasoningEffort::Max,
            },
        );
        service
            .save_model_settings(settings(vec![configured]))
            .unwrap();
        service
            .create_agent_template(&create_input(
                "template-moonshot",
                "project-a",
                "moonshot_reviewer",
                "Moonshot reviewer",
                "config-moonshot",
            ))
            .unwrap();
        assign(service, "project-a", "template-moonshot");

        let resolved = service
            .resolve_template_for_spawn("project-a", "moonshot_reviewer")
            .unwrap();
        assert_eq!(resolved.model.model_config_id, "config-moonshot");
    }

    #[test]
    fn missing_disabled_and_invalid_models_are_explicitly_unavailable_without_fallback() {
        let fixture = Fixture::new();
        let service = &fixture.service;
        service
            .create_agent_template(&create_input(
                "template-1",
                "project-a",
                "reviewer",
                "Reviewer",
                "model-a",
            ))
            .unwrap();
        assign(service, "project-a", "template-1");
        assert_unavailable(
            service
                .resolve_template_for_spawn("project-a", "reviewer")
                .unwrap_err(),
            "model-a",
            AgentTemplateModelUnavailableReason::SettingsMissing,
        );

        service
            .save_model_settings(settings(vec![model("model-b", true)]))
            .unwrap();
        assert_unavailable(
            service
                .resolve_template_for_spawn("project-a", "reviewer")
                .unwrap_err(),
            "model-a",
            AgentTemplateModelUnavailableReason::NotFound,
        );

        service
            .save_model_settings(settings(vec![model("model-a", false)]))
            .unwrap();
        assert_unavailable(
            service
                .resolve_template_for_spawn("project-a", "reviewer")
                .unwrap_err(),
            "model-a",
            AgentTemplateModelUnavailableReason::Disabled,
        );

        let mut invalid_connection = settings(vec![model("model-a", true)]);
        invalid_connection.api_url.clear();
        invalid_connection.api_token.clear();
        service.save_model_settings(invalid_connection).unwrap();
        assert_unavailable(
            service
                .resolve_template_for_spawn("project-a", "reviewer")
                .unwrap_err(),
            "model-a",
            AgentTemplateModelUnavailableReason::InvalidConnection,
        );

        let mut incompatible_profile = settings(vec![model("model-a", true)]);
        incompatible_profile.models[0].provider_profile_config =
            ProviderProfileConfig::generic_for_dialect(ProviderProtocolDialect::AnthropicMessages);
        service.save_model_settings(incompatible_profile).unwrap();
        assert_unavailable(
            service
                .resolve_template_for_spawn("project-a", "reviewer")
                .unwrap_err(),
            "model-a",
            AgentTemplateModelUnavailableReason::InvalidProfile,
        );
    }

    #[test]
    fn model_catalog_replacement_is_not_blocked_by_template_references() {
        let fixture = Fixture::new();
        let service = &fixture.service;
        service
            .save_model_settings(settings(vec![model("model-a", true)]))
            .unwrap();
        service
            .create_agent_template(&create_input(
                "template-1",
                "project-a",
                "reviewer",
                "Reviewer",
                "model-a",
            ))
            .unwrap();
        assign(service, "project-a", "template-1");

        // Model settings intentionally replace every `models` row. This succeeds only because a
        // template is a soft exact reference rather than an FK with fallback behavior.
        service
            .save_model_settings(settings(vec![model("model-b", true)]))
            .unwrap();
        assert_unavailable(
            service
                .resolve_template_for_spawn("project-a", "reviewer")
                .unwrap_err(),
            "model-a",
            AgentTemplateModelUnavailableReason::NotFound,
        );
    }

    #[test]
    fn snapshotted_template_cannot_be_deleted_but_can_be_disabled() {
        let fixture = Fixture::new();
        let service = &fixture.service;
        service
            .save_model_settings(settings(vec![
                model("model-a", true),
                model("model-b", true),
            ]))
            .unwrap();
        service
            .create_agent_template(&create_input(
                "template-1",
                "project-a",
                "reviewer",
                "Reviewer",
                "model-a",
            ))
            .unwrap();
        assign(service, "project-a", "template-1");
        service
            .create_agent_template(&create_input(
                "template-override-source",
                "project-a",
                "override_source",
                "Override source",
                "model-b",
            ))
            .unwrap();
        assign(service, "project-a", "template-override-source");
        let resolved = service
            .resolve_template_for_spawn("project-a", "reviewer")
            .unwrap();
        let model_override = service
            .resolve_template_for_spawn("project-a", "override_source")
            .unwrap()
            .model;
        service
            .save_conversation(empty_conversation("root-conversation", "project-a"))
            .unwrap();
        service
            .save_conversation(empty_conversation_with_model(
                "child-conversation",
                "project-a",
                "model-b",
            ))
            .unwrap();
        service
            .save_conversation(empty_conversation_with_model(
                "disabled-template-child-conversation",
                "project-a",
                "model-b",
            ))
            .unwrap();
        service
            .ensure_root_agent(&EnsureRootAgentInput {
                agent_id: "root-agent".to_string(),
                conversation_id: "root-conversation".to_string(),
                creation_request_id: "ensure-root-1".to_string(),
                task_name: "root".to_string(),
            })
            .unwrap();
        service
            .create_agent_node(&CreateAgentNodeInput {
                agent_id: "child-agent".to_string(),
                root_agent_id: "root-agent".to_string(),
                parent_agent_id: "root-agent".to_string(),
                conversation_id: "child-conversation".to_string(),
                creation_request_id: "spawn-child-1".to_string(),
                task_name: "security_review".to_string(),
                task_path: "/root/security_review".to_string(),
                template_snapshot: Some(resolved.template.clone()),
                model_snapshot: model_override,
            })
            .unwrap();

        let disabled = service
            .set_agent_template_enabled("template-1", 1, false)
            .unwrap();
        assert!(!disabled.enabled);
        assert_eq!(disabled.revision, 2);
        let child = service.get_agent_node("child-agent").unwrap().unwrap();
        assert_eq!(
            child.template_snapshot.as_ref().unwrap(),
            &resolved.template
        );
        assert_eq!(
            child.template_snapshot.as_ref().unwrap().model_config_id,
            "model-a"
        );
        assert_eq!(
            child.model_snapshot.as_ref().unwrap().model_config_id,
            "model-b"
        );
        assert!(matches!(
            service
                .create_agent_node(&CreateAgentNodeInput {
                    agent_id: "disabled-template-child".to_string(),
                    root_agent_id: "root-agent".to_string(),
                    parent_agent_id: "root-agent".to_string(),
                    conversation_id: "disabled-template-child-conversation".to_string(),
                    creation_request_id: "spawn-disabled-template-child".to_string(),
                    task_name: "disabled_template_child".to_string(),
                    task_path: "/root/disabled_template_child".to_string(),
                    template_snapshot: Some(resolved.template.clone()),
                    model_snapshot: child.model_snapshot.clone().unwrap(),
                })
                .unwrap_err(),
            AgentGraphError::Conflict(reason)
                if reason.contains("template snapshot is stale, disabled, unassigned, or unavailable")
        ));
        assert_eq!(
            service.delete_agent_template("template-1", 2).unwrap_err(),
            AgentTemplateError::TemplateInUse {
                template_id: "template-1".to_string(),
                agent_count: 1,
            }
        );

        service.delete_project("project-a").unwrap();
        let preserved = service.get_agent_template("template-1").unwrap();
        assert!(preserved.project_ids.is_empty());
        assert!(service.get_agent_node("root-agent").unwrap().is_none());
        assert!(service.get_agent_node("child-agent").unwrap().is_none());
    }

    #[test]
    fn resolver_reports_missing_model_identities_from_a_frozen_settings_snapshot() {
        let template = AgentTemplateRecord {
            template_id: "template-1".to_string(),
            project_ids: vec!["project-a".to_string()],
            machine_key: "reviewer".to_string(),
            name: "Reviewer".to_string(),
            description: String::new(),
            instructions: "Review.".to_string(),
            model_config_id: "model-a".to_string(),
            enabled: true,
            revision: 1,
            created_at: 1,
            updated_at: 1,
        };
        let settings = settings(vec![model("model-a", true)]);
        let snapshot = ModelSettingsSnapshot {
            settings,
            configuration_revision: "model-settings-v1:test".to_string(),
            provider_connection_revisions: BTreeMap::new(),
            provider_protocol_revisions: BTreeMap::new(),
            search_connection_revision: "search-connection-v1:test".to_string(),
        };
        assert_unavailable(
            resolve_model_selection(&template, &snapshot).unwrap_err(),
            "model-a",
            AgentTemplateModelUnavailableReason::MissingConnectionIdentity,
        );

        let mut snapshot = snapshot;
        snapshot.provider_connection_revisions.insert(
            "model-a".to_string(),
            "provider-connection-v1:test".to_string(),
        );
        assert_unavailable(
            resolve_model_selection(&template, &snapshot).unwrap_err(),
            "model-a",
            AgentTemplateModelUnavailableReason::MissingProtocolIdentity,
        );
    }

    #[test]
    fn template_validation_rejects_noncanonical_machine_keys_and_oversized_fields() {
        let fixture = Fixture::new();
        let service = &fixture.service;
        let mut input = create_input(
            "template-1",
            "project-a",
            "Security Review",
            "Reviewer",
            "model-a",
        );
        assert!(matches!(
            service.create_agent_template(&input),
            Err(AgentTemplateError::InvalidInput {
                field: "machine_key",
                ..
            })
        ));
        input.machine_key = "reviewer".to_string();
        input.instructions = "x".repeat(65_537);
        assert!(matches!(
            service.create_agent_template(&input),
            Err(AgentTemplateError::InvalidInput {
                field: "instructions",
                ..
            })
        ));
    }

    #[test]
    fn revision_exhaustion_fails_without_mutating_the_template() {
        let fixture = Fixture::new();
        let service = &fixture.service;
        {
            let connection = service.state.connection().unwrap();
            connection
                .execute(
                    "INSERT INTO agent_templates (
                        template_id, schema_version, machine_key, name, description,
                        instructions, model_config_id, enabled, revision, created_at, updated_at
                     ) VALUES (
                        'template-1', 1, 'reviewer', 'Reviewer', '',
                        'Review.', 'model-a', 1, ?1, 1, 1
                     )",
                    [i64::MAX],
                )
                .unwrap();
        }

        assert_eq!(
            service
                .set_agent_template_enabled("template-1", u64::try_from(i64::MAX).unwrap(), false,)
                .unwrap_err(),
            AgentTemplateError::RevisionExhausted
        );
        let unchanged = service.get_agent_template("template-1").unwrap();
        assert!(unchanged.enabled);
        assert_eq!(unchanged.revision, u64::try_from(i64::MAX).unwrap());
    }

    #[test]
    fn database_rejects_machine_key_identity_rewrites() {
        let fixture = Fixture::new();
        let service = &fixture.service;
        service
            .create_agent_template(&create_input(
                "template-1",
                "project-a",
                "reviewer",
                "Reviewer",
                "model-a",
            ))
            .unwrap();
        let connection = service.state.connection().unwrap();
        assert!(connection
            .execute(
                "UPDATE agent_templates
                 SET machine_key = 'renamed', revision = 2, updated_at = 2
                 WHERE template_id = 'template-1'",
                [],
            )
            .is_err());
        drop(connection);
        let unchanged = service.get_agent_template("template-1").unwrap();
        assert_eq!(unchanged.machine_key, "reviewer");
        assert_eq!(unchanged.revision, 1);
    }

    fn assert_unavailable(
        error: AgentTemplateError,
        expected_model: &str,
        expected_reason: AgentTemplateModelUnavailableReason,
    ) {
        assert_eq!(
            error,
            AgentTemplateError::ModelUnavailable {
                model_config_id: expected_model.to_string(),
                reason: expected_reason,
            }
        );
    }
}
