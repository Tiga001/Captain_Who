use super::*;

/// Stateful two-phase installation boundary intended for backend RPC use.
pub struct SkillInstallationWorkflow {
    pub(super) installation_service: SkillInstallationService,
    pub(super) adapters: BTreeMap<SkillAcquisitionProvider, Arc<dyn SkillAcquisitionAdapter>>,
    pub(super) sessions: SkillInstallationSessionStore,
    pub(super) config: SkillInstallationWorkflowConfig,
}

impl fmt::Debug for SkillInstallationWorkflow {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SkillInstallationWorkflow")
            .field(
                "adapter_providers",
                &self.adapters.keys().collect::<Vec<_>>(),
            )
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl SkillInstallationWorkflow {
    pub fn new(installation_service: SkillInstallationService) -> Self {
        Self::with_config(
            installation_service,
            SkillInstallationWorkflowConfig::default(),
        )
    }

    pub fn with_config(
        installation_service: SkillInstallationService,
        config: SkillInstallationWorkflowConfig,
    ) -> Self {
        let sessions = SkillInstallationSessionStore::new(
            SkillInstallationSessionConfig::new(
                super::super::installation_session::DEFAULT_MAX_SKILL_SOURCE_RESOLUTIONS,
                super::super::installation_session::DEFAULT_MAX_SKILL_SOURCE_RESOLUTION_CANDIDATES,
                config.max_snapshot_bytes,
                super::super::installation_session::DEFAULT_SKILL_SOURCE_RESOLUTION_TTL,
            )
            .expect("workflow configuration must form a valid session configuration"),
        );
        Self::with_session_store(installation_service, config, sessions)
    }

    #[cfg(test)]
    pub(super) fn with_clock(
        installation_service: SkillInstallationService,
        config: SkillInstallationWorkflowConfig,
        clock: Arc<dyn SessionClock>,
    ) -> Self {
        let session_config = SkillInstallationSessionConfig::new(
            super::super::installation_session::DEFAULT_MAX_SKILL_SOURCE_RESOLUTIONS,
            super::super::installation_session::DEFAULT_MAX_SKILL_SOURCE_RESOLUTION_CANDIDATES,
            config.max_snapshot_bytes,
            super::super::installation_session::DEFAULT_SKILL_SOURCE_RESOLUTION_TTL,
        )
        .expect("workflow configuration must form a valid session configuration");
        let sessions = SkillInstallationSessionStore::with_clock(session_config, clock);
        Self::with_session_store(installation_service, config, sessions)
    }

    pub fn with_session_store(
        installation_service: SkillInstallationService,
        config: SkillInstallationWorkflowConfig,
        sessions: SkillInstallationSessionStore,
    ) -> Self {
        let mut adapters: BTreeMap<SkillAcquisitionProvider, Arc<dyn SkillAcquisitionAdapter>> =
            BTreeMap::new();
        let local: Arc<dyn SkillAcquisitionAdapter> = Arc::new(LocalDirectoryAcquisitionAdapter);
        adapters.insert(local.provider(), local);
        Self {
            installation_service,
            adapters,
            sessions,
            config,
        }
    }

    pub fn session_store(&self) -> SkillInstallationSessionStore {
        self.sessions.clone()
    }

    /// Returns whether this workflow has an adapter that can safely decode the
    /// receipt's exact refresh provider and schema. Payload bytes remain
    /// private to the selected adapter.
    pub fn can_refresh(&self, provenance: &SkillInstallationProvenance) -> bool {
        self.installed_source_presentation(provenance).refreshable()
    }

    /// Returns a provider-validated, payload-free source projection. Unknown
    /// providers or semantically invalid provenance fail closed.
    pub fn installed_source_presentation(
        &self,
        provenance: &SkillInstallationProvenance,
    ) -> InstalledSkillSourcePresentation {
        let authority_provider = provenance.authority().provider();
        if provenance
            .refresh()
            .is_some_and(|refresh| refresh.provider() != authority_provider)
        {
            return InstalledSkillSourcePresentation::Unknown {
                provider: authority_provider.to_string(),
                schema_version: provenance.authority().schema_version(),
            };
        }
        let refresh_capable = provenance.refresh().is_some_and(|refresh| {
            self.adapter_for_refresh(refresh).is_ok_and(|(_, adapter)| {
                adapter
                    .refresh_schema_versions()
                    .contains(&refresh.schema_version())
            })
        });
        let presentation = self
            .adapters
            .iter()
            .find_map(|(provider, adapter)| {
                (provider.as_str() == authority_provider).then(|| {
                    adapter
                        .installed_source_presentation(provenance.adapter_view(), refresh_capable)
                })
            })
            .flatten();
        match presentation {
            Some(presentation)
                if presentation.provider() == authority_provider
                    && (!presentation.refreshable() || refresh_capable)
                    && presentation.has_valid_boundary_fields() =>
            {
                presentation
            }
            _ => InstalledSkillSourcePresentation::Unknown {
                provider: authority_provider.to_string(),
                schema_version: provenance.authority().schema_version(),
            },
        }
    }

    /// Registers one additional acquisition provider before the workflow is
    /// shared. Provider replacement is rejected so dispatch cannot silently
    /// change after requests have been prepared.
    pub fn register_adapter(
        &mut self,
        adapter: Arc<dyn SkillAcquisitionAdapter>,
    ) -> Result<(), SkillInstallationWorkflowConfigurationError> {
        let provider = adapter.provider();
        if self.adapters.contains_key(&provider) {
            return Err(SkillInstallationWorkflowConfigurationError::new(format!(
                "acquisition provider `{provider}` is already registered",
            )));
        }
        self.adapters.insert(provider, adapter);
        Ok(())
    }

    pub fn inspect_local_directory_install(
        &self,
        preparation_id: SkillPreparationId,
        installation_id: SkillInstallationId,
        directory: impl Into<PathBuf>,
    ) -> Result<SkillInstallationPreview, SkillInstallationWorkflowError> {
        self.inspect(&SkillInstallationPreparationRequest::install(
            preparation_id,
            installation_id,
            SkillAcquisitionSource::local_directory(directory),
        ))
    }

    pub fn inspect_local_directory_update(
        &self,
        preparation_id: SkillPreparationId,
        skill_id: SkillId,
        expected_revision: SkillInstallationRevision,
        directory: impl Into<PathBuf>,
    ) -> Result<SkillInstallationPreview, SkillInstallationWorkflowError> {
        self.inspect(&SkillInstallationPreparationRequest::update(
            preparation_id,
            skill_id,
            expected_revision,
            SkillAcquisitionSource::local_directory(directory),
        ))
    }

    pub(super) fn lock_registry(
        &self,
        operation: &'static str,
    ) -> Result<MutexGuard<'_, InstallationSessionState>, SkillInstallationWorkflowError> {
        self.sessions
            .lock()
            .map_err(|_| SkillInstallationWorkflowError::Internal {
                operation,
                reason: "installation session registry lock is poisoned".to_string(),
            })
    }

    pub(super) fn wait_for_change<'a>(
        &self,
        registry: MutexGuard<'a, InstallationSessionState>,
        operation: &'static str,
    ) -> Result<MutexGuard<'a, InstallationSessionState>, SkillInstallationWorkflowError> {
        self.sessions
            .wait(registry)
            .map_err(|_| SkillInstallationWorkflowError::Internal {
                operation,
                reason: "preparation registry lock is poisoned while waiting".to_string(),
            })
    }
}
