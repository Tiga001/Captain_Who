use super::*;

/// Restores the exact frozen preview if a store call unwinds while commit
/// state is unknown. Normal completion explicitly disarms the guard before
/// notifying waiters; the attempt token is a second ownership check that
/// prevents a delayed guard from rolling a later commit attempt back.
pub(super) struct CommittingSlotRecovery {
    pub(super) sessions: SkillInstallationSessionStore,
    pub(super) preparation_id: SkillPreparationId,
    pub(super) attempt_id: u64,
    pub(super) armed: bool,
    pub(super) request: Option<SkillInstallationPreparationRequest>,
    pub(super) preview: Option<SkillInstallationPreview>,
    pub(super) acquisition: Option<PreparedSkillAcquisition>,
    pub(super) snapshot_bytes: usize,
    pub(super) expires_at: Duration,
}

impl CommittingSlotRecovery {
    pub(super) fn new(
        sessions: &SkillInstallationSessionStore,
        attempt_id: u64,
        request: &SkillInstallationPreparationRequest,
        preview: &SkillInstallationPreview,
        acquisition: &PreparedSkillAcquisition,
        snapshot_bytes: usize,
        expires_at: Duration,
    ) -> Self {
        Self {
            sessions: sessions.clone(),
            preparation_id: request.preparation_id().clone(),
            attempt_id,
            armed: true,
            request: Some(request.clone()),
            preview: Some(preview.clone()),
            acquisition: Some(acquisition.clone()),
            snapshot_bytes,
            expires_at,
        }
    }

    pub(super) fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for CommittingSlotRecovery {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let recovered = self.sessions.lock().is_ok_and(|mut state| {
            let still_owns_slot = matches!(
                state.preparations.get(&self.preparation_id),
                Some(PreparationSlot::Committing { attempt_id, .. })
                    if *attempt_id == self.attempt_id
            );
            if !still_owns_slot {
                return false;
            }
            let (Some(request), Some(preview), Some(acquisition)) = (
                self.request.take(),
                self.preview.take(),
                self.acquisition.take(),
            ) else {
                return false;
            };
            state.preparations.insert(
                self.preparation_id.clone(),
                PreparationSlot::Ready {
                    request,
                    preview,
                    acquisition,
                    snapshot_bytes: self.snapshot_bytes,
                    expires_at: self.expires_at,
                },
            );
            true
        });
        if recovered {
            self.sessions.notify_all();
        }
    }
}

impl SkillInstallationWorkflow {
    /// Commits the exact package captured by `inspect`. Required warnings
    /// must be acknowledged before any store mutation starts.
    pub fn commit(
        &self,
        request: &SkillInstallationCommitRequest,
    ) -> Result<SkillInstallationCommitResult, SkillInstallationWorkflowError> {
        let (
            preparation_request,
            preview,
            acquisition,
            action,
            snapshot_bytes,
            original_expires_at,
            commit_attempt_id,
        ) = loop {
            let now = self.sessions.now();
            let mut registry = self.lock_registry("commit Skill preparation")?;
            registry.prune_expired(now.monotonic);
            let Some(slot) = registry.preparations.get(request.preparation_id()) else {
                return Err(
                    SkillInstallationWorkflowError::PreparationNotFoundOrExpired {
                        preparation_id: request.preparation_id.clone(),
                    },
                );
            };
            match slot {
                PreparationSlot::Preparing { .. } | PreparationSlot::Committing { .. } => {
                    drop(self.wait_for_change(registry, "wait to commit Skill preparation")?);
                    continue;
                }
                PreparationSlot::Cancelled { .. } => {
                    return Err(SkillInstallationWorkflowError::PreparationCancelled {
                        preparation_id: request.preparation_id.clone(),
                    });
                }
                PreparationSlot::Committed {
                    mutation, preview, ..
                } => {
                    ensure_preview_matches(preview, request)?;
                    ensure_warnings_acknowledged(preview, request)?;
                    return Ok(SkillInstallationCommitResult {
                        preview: preview.clone(),
                        mutation: mutation.clone(),
                        replayed: true,
                    });
                }
                PreparationSlot::Ready {
                    request: preparation_request,
                    preview,
                    acquisition,
                    snapshot_bytes,
                    expires_at,
                } => {
                    ensure_preview_matches(preview, request)?;
                    ensure_warnings_acknowledged(preview, request)?;
                    let frozen = (
                        preparation_request.clone(),
                        preview.clone(),
                        acquisition.clone(),
                        PreparedAction::from_intent(&preparation_request.intent)?,
                        *snapshot_bytes,
                        *expires_at,
                    );
                    let commit_attempt_id = registry.next_preparation_attempt();
                    registry.preparations.insert(
                        request.preparation_id.clone(),
                        PreparationSlot::Committing {
                            request: frozen.0.clone(),
                            attempt_id: commit_attempt_id,
                            snapshot_bytes: frozen.4,
                        },
                    );
                    break (
                        frozen.0,
                        frozen.1,
                        frozen.2,
                        frozen.3,
                        frozen.4,
                        frozen.5,
                        commit_attempt_id,
                    );
                }
            }
        };

        let mut committing_recovery = CommittingSlotRecovery::new(
            &self.sessions,
            commit_attempt_id,
            &preparation_request,
            &preview,
            &acquisition,
            snapshot_bytes,
            original_expires_at,
        );
        let (package, provenance) = acquisition.clone().into_parts();
        let committed = match action {
            PreparedAction::Install {
                installation_id, ..
            } => self.installation_service.install_prepared_with_provenance(
                installation_id,
                package,
                provenance,
            ),
            PreparedAction::Update {
                installation_id,
                expected_revision,
                ..
            } => self.installation_service.update_prepared_exact(
                installation_id,
                expected_revision,
                package,
                provenance,
            ),
        };

        let now = self.sessions.now();
        let mut registry = match self.lock_registry("finish Skill commit") {
            Ok(registry) => registry,
            Err(_) => {
                // The store mutation is authoritative. Losing the in-memory
                // replay cache must never turn a known commit result into an
                // unrelated Internal error or hide commit-indeterminate
                // semantics from the caller.
                committing_recovery.disarm();
                self.sessions.notify_all();
                return match committed {
                    Ok(mutation) => Ok(SkillInstallationCommitResult {
                        preview,
                        mutation,
                        replayed: false,
                    }),
                    Err(source) => Err(SkillInstallationWorkflowError::Installation {
                        preparation_id: request.preparation_id.clone(),
                        source: Box::new(source),
                    }),
                };
            }
        };
        let result = match committed {
            Ok(mutation) => {
                let result_preview = preview.clone();
                registry.preparations.insert(
                    request.preparation_id.clone(),
                    PreparationSlot::Committed {
                        request: preparation_request,
                        preview,
                        mutation: mutation.clone(),
                        expires_at: now.monotonic.saturating_add(self.config.preparation_ttl),
                    },
                );
                Ok(SkillInstallationCommitResult {
                    preview: result_preview,
                    mutation,
                    replayed: false,
                })
            }
            Err(source) => {
                registry.preparations.insert(
                    request.preparation_id.clone(),
                    PreparationSlot::Ready {
                        request: preparation_request,
                        preview,
                        snapshot_bytes,
                        acquisition,
                        expires_at: original_expires_at,
                    },
                );
                Err(SkillInstallationWorkflowError::Installation {
                    preparation_id: request.preparation_id.clone(),
                    source: Box::new(source),
                })
            }
        };
        committing_recovery.disarm();
        self.sessions.notify_all();
        result
    }

    pub fn cancel(
        &self,
        preparation_id: &SkillPreparationId,
    ) -> Result<SkillPreparationCancellation, SkillInstallationWorkflowError> {
        let now = self.sessions.now();
        let mut registry = self.lock_registry("cancel Skill preparation")?;
        registry.prune_expired(now.monotonic);
        let Some(slot) = registry.preparations.get(preparation_id) else {
            return Err(
                SkillInstallationWorkflowError::PreparationNotFoundOrExpired {
                    preparation_id: preparation_id.clone(),
                },
            );
        };
        match slot {
            PreparationSlot::Preparing { .. } | PreparationSlot::Committing { .. } => {
                Err(SkillInstallationWorkflowError::PreparationBusy {
                    preparation_id: preparation_id.clone(),
                })
            }
            PreparationSlot::Committed { .. } => Err(
                SkillInstallationWorkflowError::PreparationAlreadyCommitted {
                    preparation_id: preparation_id.clone(),
                },
            ),
            PreparationSlot::Cancelled { .. } => Ok(SkillPreparationCancellation::AlreadyCancelled),
            PreparationSlot::Ready { request, .. } => {
                let request = request.clone();
                registry.preparations.insert(
                    preparation_id.clone(),
                    PreparationSlot::Cancelled {
                        request,
                        expires_at: now.monotonic.saturating_add(self.config.preparation_ttl),
                    },
                );
                self.sessions.notify_all();
                Ok(SkillPreparationCancellation::Cancelled)
            }
        }
    }
}
