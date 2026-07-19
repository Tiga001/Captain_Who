use super::*;

/// Panic recovery for adapter callbacks. The dispatcher catches unwinds, so
/// the workflow must release the reservation before that unwind leaves the
/// worker or the idempotency key would remain permanently busy. The attempt
/// token makes a delayed guard harmless after the same key starts new work.
pub(super) struct PreparingSlotRecovery {
    pub(super) sessions: SkillInstallationSessionStore,
    pub(super) preparation_id: SkillPreparationId,
    pub(super) attempt_id: u64,
    pub(super) armed: bool,
}

impl PreparingSlotRecovery {
    pub(super) fn new(
        sessions: &SkillInstallationSessionStore,
        preparation_id: &SkillPreparationId,
        attempt_id: u64,
    ) -> Self {
        Self {
            sessions: sessions.clone(),
            preparation_id: preparation_id.clone(),
            attempt_id,
            armed: true,
        }
    }

    pub(super) fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for PreparingSlotRecovery {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let recovered = self.sessions.lock().is_ok_and(|mut state| {
            if matches!(
                state.preparations.get(&self.preparation_id),
                Some(PreparationSlot::Preparing { attempt_id, .. })
                    if *attempt_id == self.attempt_id
            ) {
                state.preparations.remove(&self.preparation_id);
                true
            } else {
                false
            }
        });
        if recovered {
            self.sessions.notify_all();
        }
    }
}

impl SkillInstallationWorkflow {
    /// Acquires and validates an exact snapshot, returning the same preview
    /// for a retry with the same preparation ID and byte-for-byte request.
    pub fn inspect(
        &self,
        request: &SkillInstallationPreparationRequest,
    ) -> Result<SkillInstallationPreview, SkillInstallationWorkflowError> {
        let action = PreparedAction::from_intent(&request.intent)?;
        if let Some(preview) = self.existing_preparation(request)? {
            return Ok(preview);
        }
        let current = self.preflight_action(&action)?;
        if let SkillAcquisitionSource::ResolvedCandidate {
            resolution_id,
            candidate_id,
        } = &request.source
        {
            return self.inspect_resolved_candidate(
                request,
                &action,
                current.as_ref(),
                resolution_id,
                candidate_id,
            );
        }
        if matches!(request.source, SkillAcquisitionSource::InstalledSource) {
            return self.inspect_installed_source(request, &action, current.as_ref());
        }
        let provider = request.source.provider();
        let adapter = self.adapters.get(&provider).ok_or_else(|| {
            SkillInstallationWorkflowError::UnknownAcquisitionProvider {
                provider: provider.clone(),
            }
        })?;

        let preparation_attempt_id = loop {
            let now = self.sessions.now();
            let mut registry = self.lock_registry("inspect Skill preparation")?;
            registry.prune_expired(now.monotonic);
            match registry.preparations.get(request.preparation_id()) {
                Some(slot) if slot.request() != request => {
                    return Err(SkillInstallationWorkflowError::PreparationConflict {
                        preparation_id: request.preparation_id.clone(),
                    });
                }
                Some(PreparationSlot::Preparing { .. })
                | Some(PreparationSlot::Committing { .. }) => {
                    drop(self.wait_for_change(registry, "wait for Skill preparation")?);
                    continue;
                }
                Some(PreparationSlot::Ready { preview, .. })
                | Some(PreparationSlot::Committed { preview, .. }) => {
                    return Ok(preview.clone());
                }
                Some(PreparationSlot::Cancelled { .. }) => {
                    return Err(SkillInstallationWorkflowError::PreparationCancelled {
                        preparation_id: request.preparation_id.clone(),
                    });
                }
                None => {
                    break self.reserve_preparation(
                        &mut registry,
                        request.clone(),
                        now.monotonic,
                    )?;
                }
            }
        };

        let mut preparing_recovery = PreparingSlotRecovery::new(
            &self.sessions,
            request.preparation_id(),
            preparation_attempt_id,
        );
        let acquired = adapter.acquire(&request.source).and_then(|acquisition| {
            if acquisition.is_owned_by(provider.as_str()) {
                Ok(acquisition)
            } else {
                Err(invalid_acquisition_provider_output())
            }
        });
        let now = self.sessions.now();
        let mut registry = self.lock_registry("finish Skill preparation")?;
        let result = match acquired {
            Ok(acquisition) => {
                let snapshot_bytes = acquisition_snapshot_bytes(&acquisition);
                if let Err(error) =
                    self.ensure_finished_preparation_capacity(&registry, snapshot_bytes)
                {
                    registry.preparations.remove(request.preparation_id());
                    preparing_recovery.disarm();
                    self.sessions.notify_all();
                    return Err(error);
                }
                let preview = build_preview(
                    request,
                    &action,
                    &acquisition,
                    current.as_ref(),
                    now.unix_ms
                        .saturating_add(duration_millis(self.config.preparation_ttl)),
                );
                registry.preparations.insert(
                    request.preparation_id.clone(),
                    PreparationSlot::Ready {
                        request: request.clone(),
                        preview: preview.clone(),
                        snapshot_bytes,
                        acquisition,
                        expires_at: now.monotonic.saturating_add(self.config.preparation_ttl),
                    },
                );
                Ok(preview)
            }
            Err(source) => {
                registry.preparations.remove(request.preparation_id());
                Err(SkillInstallationWorkflowError::Acquisition {
                    provider,
                    source: Box::new(source),
                })
            }
        };
        preparing_recovery.disarm();
        self.sessions.notify_all();
        result
    }

    pub(super) fn inspect_resolved_candidate(
        &self,
        request: &SkillInstallationPreparationRequest,
        action: &PreparedAction,
        current: Option<&InstalledSkillRecord>,
        resolution_id: &SkillSourceResolutionId,
        candidate_id: &SkillSourceCandidateId,
    ) -> Result<SkillInstallationPreview, SkillInstallationWorkflowError> {
        loop {
            let now = self.sessions.now();
            let mut state = self.lock_registry("inspect resolved Skill candidate")?;
            state.prune_expired(now.monotonic);
            match state.preparations.get(request.preparation_id()) {
                Some(slot) if slot.request() != request => {
                    return Err(SkillInstallationWorkflowError::PreparationConflict {
                        preparation_id: request.preparation_id.clone(),
                    });
                }
                Some(PreparationSlot::Preparing { .. })
                | Some(PreparationSlot::Committing { .. }) => {
                    drop(self.wait_for_change(state, "wait for resolved Skill preparation")?);
                    continue;
                }
                Some(PreparationSlot::Ready { preview, .. })
                | Some(PreparationSlot::Committed { preview, .. }) => {
                    return Ok(preview.clone());
                }
                Some(PreparationSlot::Cancelled { .. }) => {
                    return Err(SkillInstallationWorkflowError::PreparationCancelled {
                        preparation_id: request.preparation_id.clone(),
                    });
                }
                None => {}
            }

            if state.preparations.len() >= self.config.max_preparations {
                return Err(
                    SkillInstallationWorkflowError::PreparationCapacityExceeded {
                        max_preparations: self.config.max_preparations,
                    },
                );
            }

            let Some(slot) = state.resolutions.remove(resolution_id) else {
                return Err(
                    SkillInstallationWorkflowError::SourceResolutionNotFoundOrExpired {
                        resolution_id: resolution_id.clone(),
                    },
                );
            };
            match slot {
                ResolutionSlot::Ready {
                    locator,
                    resolution,
                    mut candidates,
                    snapshot_bytes,
                    expires_at: resolution_expires_at,
                } => {
                    let Some(acquisition) = candidates.remove(candidate_id) else {
                        state.resolutions.insert(
                            resolution_id.clone(),
                            ResolutionSlot::Ready {
                                locator,
                                resolution,
                                candidates,
                                snapshot_bytes,
                                expires_at: resolution_expires_at,
                            },
                        );
                        return Err(SkillInstallationWorkflowError::SourceCandidateNotFound {
                            resolution_id: resolution_id.clone(),
                            candidate_id: candidate_id.clone(),
                        });
                    };
                    let expires_at_unix_ms = now
                        .unix_ms
                        .saturating_add(duration_millis(self.config.preparation_ttl));
                    let preview =
                        build_preview(request, action, &acquisition, current, expires_at_unix_ms);
                    let snapshot_bytes = acquisition_snapshot_bytes(&acquisition);
                    state.preparations.insert(
                        request.preparation_id.clone(),
                        PreparationSlot::Ready {
                            request: request.clone(),
                            preview: preview.clone(),
                            acquisition,
                            snapshot_bytes,
                            expires_at: now.monotonic.saturating_add(self.config.preparation_ttl),
                        },
                    );
                    state.resolutions.insert(
                        resolution_id.clone(),
                        ResolutionSlot::Consumed {
                            locator,
                            candidate_id: candidate_id.clone(),
                            preparation_id: request.preparation_id.clone(),
                            expires_at: resolution_expires_at,
                        },
                    );
                    self.sessions.notify_all();
                    return Ok(preview);
                }
                ResolutionSlot::Resolving {
                    locator,
                    attempt_id,
                    reserved_bytes,
                    expires_at,
                } => {
                    state.resolutions.insert(
                        resolution_id.clone(),
                        ResolutionSlot::Resolving {
                            locator,
                            attempt_id,
                            reserved_bytes,
                            expires_at,
                        },
                    );
                    return Err(SkillInstallationWorkflowError::SourceResolutionBusy {
                        resolution_id: resolution_id.clone(),
                    });
                }
                ResolutionSlot::Consumed {
                    locator,
                    candidate_id: consumed_candidate_id,
                    preparation_id,
                    expires_at,
                } => {
                    state.resolutions.insert(
                        resolution_id.clone(),
                        ResolutionSlot::Consumed {
                            locator,
                            candidate_id: consumed_candidate_id,
                            preparation_id,
                            expires_at,
                        },
                    );
                    return Err(SkillInstallationWorkflowError::SourceResolutionConsumed {
                        resolution_id: resolution_id.clone(),
                    });
                }
                ResolutionSlot::Cancelled {
                    locator,
                    expires_at,
                } => {
                    state.resolutions.insert(
                        resolution_id.clone(),
                        ResolutionSlot::Cancelled {
                            locator,
                            expires_at,
                        },
                    );
                    return Err(SkillInstallationWorkflowError::SourceResolutionCancelled {
                        resolution_id: resolution_id.clone(),
                    });
                }
            }
        }
    }

    pub(super) fn inspect_installed_source(
        &self,
        request: &SkillInstallationPreparationRequest,
        action: &PreparedAction,
        current: Option<&InstalledSkillRecord>,
    ) -> Result<SkillInstallationPreview, SkillInstallationWorkflowError> {
        let installation_id = match action {
            PreparedAction::Update {
                installation_id, ..
            } => installation_id,
            PreparedAction::Install { .. } => {
                return Err(SkillInstallationWorkflowError::InstalledSourceRequiresUpdate)
            }
        };

        let preparation_attempt_id = loop {
            let now = self.sessions.now();
            let mut state = self.lock_registry("inspect installed Skill source")?;
            state.prune_expired(now.monotonic);
            match state.preparations.get(request.preparation_id()) {
                Some(slot) if slot.request() != request => {
                    return Err(SkillInstallationWorkflowError::PreparationConflict {
                        preparation_id: request.preparation_id.clone(),
                    });
                }
                Some(PreparationSlot::Preparing { .. })
                | Some(PreparationSlot::Committing { .. }) => {
                    drop(self.wait_for_change(state, "wait for installed Skill refresh")?);
                    continue;
                }
                Some(PreparationSlot::Ready { preview, .. })
                | Some(PreparationSlot::Committed { preview, .. }) => {
                    return Ok(preview.clone());
                }
                Some(PreparationSlot::Cancelled { .. }) => {
                    return Err(SkillInstallationWorkflowError::PreparationCancelled {
                        preparation_id: request.preparation_id.clone(),
                    });
                }
                None => {
                    break self.reserve_preparation(&mut state, request.clone(), now.monotonic)?;
                }
            }
        };

        let mut preparing_recovery = PreparingSlotRecovery::new(
            &self.sessions,
            request.preparation_id(),
            preparation_attempt_id,
        );
        // Receipt read and lifecycle CAS validation intentionally precede all
        // adapter/network work. Commit repeats the same CAS in the installer.
        let acquired = (|| {
            let record = current.expect("update preflight must return an installed record");
            if record.is_legacy() {
                return Err(SkillInstallationWorkflowError::InstalledSourceLegacy {
                    installation_id: installation_id.clone(),
                });
            }
            let refresh = record.provenance().refresh().ok_or_else(|| {
                SkillInstallationWorkflowError::InstalledSourceNotRefreshable {
                    installation_id: installation_id.clone(),
                }
            })?;
            let authority_provider = record.provenance().authority().provider();
            if refresh.provider() != authority_provider {
                return Err(
                    SkillInstallationWorkflowError::InvalidInstalledSourceProvenance {
                        provider: authority_provider.to_string(),
                    },
                );
            }
            let (provider, adapter) = self.adapter_for_refresh(refresh)?;
            if !adapter
                .refresh_schema_versions()
                .contains(&refresh.schema_version())
            {
                return Err(SkillInstallationWorkflowError::UnsupportedRefreshSchema {
                    provider: refresh.provider().to_string(),
                    schema_version: refresh.schema_version(),
                });
            }
            if matches!(
                self.installed_source_presentation(record.provenance()),
                InstalledSkillSourcePresentation::Unknown { .. }
            ) {
                return Err(
                    SkillInstallationWorkflowError::InvalidInstalledSourceProvenance {
                        provider: record.provenance().authority().provider().to_string(),
                    },
                );
            }
            adapter
                .reacquire(refresh.adapter_view())
                .and_then(|acquisition| {
                    if acquisition.is_owned_by(provider.as_str()) {
                        Ok(acquisition)
                    } else {
                        Err(invalid_acquisition_provider_output())
                    }
                })
                .map_err(|source| SkillInstallationWorkflowError::Acquisition {
                    provider: provider.clone(),
                    source: Box::new(source),
                })
        })();

        let now = self.sessions.now();
        let mut state = self.lock_registry("finish installed Skill refresh")?;
        let result = match acquired {
            Ok(acquisition) => {
                let snapshot_bytes = acquisition_snapshot_bytes(&acquisition);
                if let Err(error) =
                    self.ensure_finished_preparation_capacity(&state, snapshot_bytes)
                {
                    state.preparations.remove(request.preparation_id());
                    preparing_recovery.disarm();
                    self.sessions.notify_all();
                    return Err(error);
                }
                let preview = build_preview(
                    request,
                    action,
                    &acquisition,
                    current,
                    now.unix_ms
                        .saturating_add(duration_millis(self.config.preparation_ttl)),
                );
                state.preparations.insert(
                    request.preparation_id.clone(),
                    PreparationSlot::Ready {
                        request: request.clone(),
                        preview: preview.clone(),
                        snapshot_bytes,
                        acquisition,
                        expires_at: now.monotonic.saturating_add(self.config.preparation_ttl),
                    },
                );
                Ok(preview)
            }
            Err(error) => {
                state.preparations.remove(request.preparation_id());
                Err(error)
            }
        };
        preparing_recovery.disarm();
        self.sessions.notify_all();
        result
    }

    pub(super) fn adapter_for_refresh(
        &self,
        refresh: &SkillInstallationRefresh,
    ) -> Result<
        (&SkillAcquisitionProvider, &Arc<dyn SkillAcquisitionAdapter>),
        SkillInstallationWorkflowError,
    > {
        self.adapters
            .iter()
            .find_map(|(provider, adapter)| {
                (provider.as_str() == refresh.provider()).then_some((provider, adapter))
            })
            .ok_or_else(|| SkillInstallationWorkflowError::UnknownRefreshProvider {
                provider: refresh.provider().to_string(),
            })
    }

    pub(super) fn existing_preparation(
        &self,
        request: &SkillInstallationPreparationRequest,
    ) -> Result<Option<SkillInstallationPreview>, SkillInstallationWorkflowError> {
        loop {
            let now = self.sessions.now();
            let mut state = self.lock_registry("check existing Skill preparation")?;
            state.prune_expired(now.monotonic);
            match state.preparations.get(request.preparation_id()) {
                Some(slot) if slot.request() != request => {
                    return Err(SkillInstallationWorkflowError::PreparationConflict {
                        preparation_id: request.preparation_id.clone(),
                    });
                }
                Some(PreparationSlot::Preparing { .. })
                | Some(PreparationSlot::Committing { .. }) => {
                    drop(self.wait_for_change(state, "wait for existing Skill preparation")?);
                }
                Some(PreparationSlot::Ready { preview, .. })
                | Some(PreparationSlot::Committed { preview, .. }) => {
                    return Ok(Some(preview.clone()));
                }
                Some(PreparationSlot::Cancelled { .. }) => {
                    return Err(SkillInstallationWorkflowError::PreparationCancelled {
                        preparation_id: request.preparation_id.clone(),
                    });
                }
                None => return Ok(None),
            }
        }
    }

    pub(super) fn preflight_action(
        &self,
        action: &PreparedAction,
    ) -> Result<Option<InstalledSkillRecord>, SkillInstallationWorkflowError> {
        let PreparedAction::Update {
            installation_id,
            expected_revision,
            ..
        } = action
        else {
            return Ok(None);
        };
        let record = self
            .installation_service
            .read_installed_skill(installation_id)
            .map_err(|error| SkillInstallationWorkflowError::InstalledSkillRead {
                installation_id: installation_id.clone(),
                reason: error.to_string(),
            })?
            .ok_or_else(|| SkillInstallationWorkflowError::InstalledSkillNotFound {
                installation_id: installation_id.clone(),
            })?;
        if record.installation_revision() != expected_revision {
            return Err(
                SkillInstallationWorkflowError::InstalledSourceRevisionConflict {
                    installation_id: installation_id.clone(),
                    expected_revision: expected_revision.clone(),
                    actual_revision: record.installation_revision().clone(),
                },
            );
        }
        Ok(Some(record))
    }

    pub(super) fn reserve_preparation(
        &self,
        state: &mut InstallationSessionState,
        request: SkillInstallationPreparationRequest,
        now: Duration,
    ) -> Result<u64, SkillInstallationWorkflowError> {
        if state.preparations.len() >= self.config.max_preparations {
            return Err(
                SkillInstallationWorkflowError::PreparationCapacityExceeded {
                    max_preparations: self.config.max_preparations,
                },
            );
        }
        let max_snapshot_bytes = self
            .config
            .max_snapshot_bytes
            .min(self.sessions.config().max_snapshot_bytes());
        if state
            .reserved_snapshot_bytes()
            .saturating_add(MAX_SKILL_PACKAGE_BYTES)
            > max_snapshot_bytes
        {
            return Err(
                SkillInstallationWorkflowError::PreparationMemoryCapacityExceeded {
                    max_snapshot_bytes,
                },
            );
        }
        let attempt_id = state.next_preparation_attempt();
        state.preparations.insert(
            request.preparation_id.clone(),
            PreparationSlot::Preparing {
                request,
                attempt_id,
                _started_at: now,
            },
        );
        Ok(attempt_id)
    }

    pub(super) fn ensure_finished_preparation_capacity(
        &self,
        state: &InstallationSessionState,
        snapshot_bytes: usize,
    ) -> Result<(), SkillInstallationWorkflowError> {
        let max_snapshot_bytes = self
            .config
            .max_snapshot_bytes
            .min(self.sessions.config().max_snapshot_bytes());
        let resident_without_reservation = state
            .reserved_snapshot_bytes()
            .saturating_sub(MAX_SKILL_PACKAGE_BYTES);
        if resident_without_reservation.saturating_add(snapshot_bytes) > max_snapshot_bytes {
            Err(
                SkillInstallationWorkflowError::PreparationMemoryCapacityExceeded {
                    max_snapshot_bytes,
                },
            )
        } else {
            Ok(())
        }
    }
}
