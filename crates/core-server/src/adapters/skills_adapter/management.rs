use super::*;

/// Builds the global settings-page inventory. Workspace Skills deliberately
/// remain outside this view because their lifetime and identity are scoped to
/// one project, while Enabled is a global eligibility preference.
pub(crate) fn management_response(
    storage: &StorageService,
    catalog: &SkillCatalog,
    installations: &SkillInstallationService,
    workflow: Option<&SkillInstallationWorkflow>,
) -> Result<SkillsListManagementResponse, SkillManagementFailure> {
    let (response, _) = management_snapshot(storage, catalog, installations, workflow)?;
    Ok(response)
}

pub(super) fn management_snapshot(
    storage: &StorageService,
    catalog: &SkillCatalog,
    installations: &SkillInstallationService,
    workflow: Option<&SkillInstallationWorkflow>,
) -> Result<
    (
        SkillsListManagementResponse,
        std::collections::BTreeMap<String, SkillEnablementState>,
    ),
    SkillManagementFailure,
> {
    let installed_inventory = installations
        .list_installed_skills()
        .map_err(|_| SkillManagementFailure::list_unavailable())?;
    let installed_records = installed_inventory
        .records()
        .iter()
        .map(|record| (record.skill_id().as_str(), record))
        .collect::<std::collections::BTreeMap<_, _>>();
    let ids = catalog
        .skills()
        .iter()
        .map(|skill| skill.id().as_str().to_string())
        .collect::<Vec<_>>();
    let enablement = storage
        .load_skill_enablement_states(&ids)
        .map_err(|_| SkillManagementFailure::list_unavailable())?;
    let skills = catalog
        .skills()
        .iter()
        .filter(|skill| {
            matches!(
                skill.source_kind(),
                SkillSourceKind::Bundled | SkillSourceKind::Installed
            )
        })
        .map(|skill| {
            let state =
                enablement
                    .get(skill.id().as_str())
                    .copied()
                    .unwrap_or(SkillEnablementState {
                        enabled: false,
                        generation: 0,
                    });
            management_entry(
                skill,
                state.enabled,
                state.generation,
                installed_records.get(skill.id().as_str()).copied(),
                workflow,
            )
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| SkillManagementFailure::list_unavailable())?;
    let management_revision = management_revision(catalog.catalog_revision(), &skills);
    let mut diagnostics = diagnostic_dtos(catalog);
    diagnostics.extend(installed_inventory.issues().iter().map(|issue| {
        SkillDiagnosticDto {
            code: issue.code().stable_name().to_string(),
            severity: match issue.severity() {
                SkillDiagnosticSeverity::Warning => "warning",
                SkillDiagnosticSeverity::Error => "error",
                _ => "error",
            }
            .to_string(),
            message: issue.message().to_string(),
            skill_id: None,
            location: Some(issue.location().to_string()),
        }
    }));
    Ok((
        SkillsListManagementResponse {
            schema_version: SKILL_MANAGEMENT_SCHEMA_VERSION,
            management_revision,
            skills,
            diagnostics,
            truncated: catalog.truncated() || installed_inventory.truncated(),
        },
        enablement,
    ))
}

/// Applies one compare-and-swap enablement mutation against the exact item
/// state the settings page displayed.
pub(crate) fn set_enabled_response(
    storage: &StorageService,
    catalog: &SkillCatalog,
    installations: &SkillInstallationService,
    workflow: Option<&SkillInstallationWorkflow>,
    request: &SkillsSetEnabledRequest,
) -> Result<(SkillsSetEnabledResponse, bool), SkillManagementFailure> {
    let descriptor = catalog
        .skills()
        .iter()
        .find(|skill| skill.id().as_str() == request.skill_id)
        .ok_or_else(|| {
            if request.skill_id.starts_with("workspace:") {
                SkillManagementFailure::new(
                    SkillManagementOperationDto::SetEnabled,
                    SkillManagementErrorCodeDto::NotManageable,
                    SkillManagementRecoveryDto::RefreshManagement,
                    "Workspace Skills are scoped to a project and cannot be globally enabled or disabled.",
                )
            } else {
                SkillManagementFailure::new(
                    SkillManagementOperationDto::SetEnabled,
                    SkillManagementErrorCodeDto::NotFound,
                    SkillManagementRecoveryDto::RefreshManagement,
                    "The requested Skill is no longer installed or available.",
                )
            }
        })?;
    if !matches!(
        descriptor.source_kind(),
        SkillSourceKind::Bundled | SkillSourceKind::Installed
    ) {
        return Err(SkillManagementFailure::new(
            SkillManagementOperationDto::SetEnabled,
            SkillManagementErrorCodeDto::NotManageable,
            SkillManagementRecoveryDto::RefreshManagement,
            "The requested Skill cannot be managed from global settings.",
        ));
    }

    let (current, enablement) = management_snapshot(storage, catalog, installations, workflow)
        .map_err(|_| SkillManagementFailure::set_enabled_unavailable())?;
    let current_state = enablement
        .get(request.skill_id.as_str())
        .copied()
        .expect("the management snapshot contains every catalog Skill id");
    let current_item = current
        .skills
        .iter()
        .find(|skill| skill.id == request.skill_id)
        .expect("the validated global Skill must be present in the management snapshot");
    let current_enabled = current_state.enabled;
    let current_state_revision = current_item.state_revision.clone();

    // A lost successful response must converge when the client retries the
    // same desired state with its original compare token.
    if current_enabled == request.enabled {
        return Ok((
            SkillsSetEnabledResponse {
                schema_version: SKILL_MANAGEMENT_SCHEMA_VERSION,
                management_revision: current.management_revision,
                skill_id: request.skill_id.clone(),
                state_revision: current_state_revision,
                enabled: current_enabled,
                outcome: SkillSetEnabledOutcomeDto::AlreadyCurrent,
            },
            false,
        ));
    }
    if current_state_revision != request.expected_state_revision {
        return Err(SkillManagementFailure::new(
            SkillManagementOperationDto::SetEnabled,
            SkillManagementErrorCodeDto::StateConflict,
            SkillManagementRecoveryDto::RefreshManagement,
            "The Skill state changed. Refresh the Skill management list before retrying.",
        ));
    }

    let outcome = storage
        .compare_and_set_skill_enablement(
            &request.skill_id,
            current_enabled,
            current_state.generation,
            request.enabled,
        )
        .map_err(|_| SkillManagementFailure::set_enabled_unavailable())?;
    let (changed, target_generation) = match outcome {
        SkillEnablementCompareAndSetOutcome::Updated { generation } => (true, generation),
        SkillEnablementCompareAndSetOutcome::AlreadyCurrent { generation } => (false, generation),
        SkillEnablementCompareAndSetOutcome::Conflict => {
            return Err(SkillManagementFailure::new(
                SkillManagementOperationDto::SetEnabled,
                SkillManagementErrorCodeDto::StateConflict,
                SkillManagementRecoveryDto::RefreshManagement,
                "The Skill state changed. Refresh the Skill management list before retrying.",
            ));
        }
    };

    // Every remaining step is infallible. Once SQLite commits, response
    // construction cannot turn success into an ambiguous error.
    let mut refreshed = current;
    let item = refreshed
        .skills
        .iter_mut()
        .find(|skill| skill.id == request.skill_id)
        .expect("the validated global Skill remains present in the same catalog snapshot");
    item.enabled = request.enabled;
    item.state_revision = management_state_revision(
        descriptor,
        item.installation_revision.as_deref(),
        request.enabled,
        target_generation,
    );
    let response_state_revision = item.state_revision.clone();
    refreshed.management_revision =
        management_revision(catalog.catalog_revision(), &refreshed.skills);
    Ok((
        SkillsSetEnabledResponse {
            schema_version: SKILL_MANAGEMENT_SCHEMA_VERSION,
            management_revision: refreshed.management_revision,
            skill_id: request.skill_id.clone(),
            state_revision: response_state_revision,
            enabled: request.enabled,
            outcome: if changed {
                SkillSetEnabledOutcomeDto::Updated
            } else {
                SkillSetEnabledOutcomeDto::AlreadyCurrent
            },
        },
        changed,
    ))
}

pub(super) fn management_entry(
    descriptor: &SkillDescriptor,
    enabled: bool,
    generation: u64,
    installation: Option<&InstalledSkillRecord>,
    workflow: Option<&SkillInstallationWorkflow>,
) -> Result<SkillManagementEntryDto, String> {
    let installed = descriptor.source_kind() == SkillSourceKind::Installed;
    let installation = installation
        .filter(|record| installed && record.package_revision() == descriptor.revision());
    let installation_revision =
        installation.map(|record| record.installation_revision().as_str().to_string());
    let source_presentation = installation.and_then(|record| {
        workflow.map(|workflow| workflow.installed_source_presentation(record.provenance()))
    });
    let can_update = installation.is_some_and(|record| !record.is_legacy())
        && source_presentation
            .as_ref()
            .is_some_and(InstalledSkillSourcePresentation::refreshable);
    let acquisition = source_presentation
        .as_ref()
        .map(|source| management_acquisition(descriptor, source));
    Ok(SkillManagementEntryDto {
        id: descriptor.id().as_str().to_string(),
        name: descriptor.name().to_string(),
        description: descriptor.description().to_string(),
        source: source_dto(descriptor)?,
        package_revision: descriptor.revision().as_str().to_string(),
        installation_revision: installation_revision.clone(),
        state_revision: management_state_revision(
            descriptor,
            installation_revision.as_deref(),
            enabled,
            generation,
        ),
        enabled,
        actions: SkillManagementActionsDto {
            can_set_enabled: true,
            can_update,
            can_uninstall: installation.is_some(),
        },
        // Legacy v1 receipts contain display-only origin strings, not an
        // authority-bearing typed source. Do not turn them back into paths.
        acquisition,
        compatibility: SkillCompatibilityReportDto {
            status: if descriptor.source_kind() == SkillSourceKind::Bundled {
                SkillCompatibilityStatusDto::Compatible
            } else {
                SkillCompatibilityStatusDto::Unknown
            },
            issues: Vec::new(),
        },
    })
}

pub(super) fn management_acquisition(
    descriptor: &SkillDescriptor,
    source: &InstalledSkillSourcePresentation,
) -> SkillPreviewSourceDto {
    match source {
        InstalledSkillSourcePresentation::LocalDirectory => SkillPreviewSourceDto::LocalDirectory {
            display_name: descriptor.name().to_string(),
            refreshable: false,
        },
        InstalledSkillSourcePresentation::GitHub {
            owner,
            repository,
            tracking_reference,
            resolved_commit,
            subdirectory,
            refreshable,
        } => {
            let (reference, reference_supported) = match tracking_reference {
                InstalledGitHubTrackingReference::DefaultBranch => {
                    (SkillGithubReferenceDto::DefaultBranch {}, true)
                }
                InstalledGitHubTrackingReference::Named(value) => (
                    SkillGithubReferenceDto::Named {
                        value: value.clone(),
                    },
                    true,
                ),
                InstalledGitHubTrackingReference::Commit => (
                    SkillGithubReferenceDto::Commit {
                        sha: resolved_commit.clone(),
                    },
                    true,
                ),
                _ => (
                    SkillGithubReferenceDto::Commit {
                        sha: resolved_commit.clone(),
                    },
                    false,
                ),
            };
            SkillPreviewSourceDto::GithubRepository {
                owner: owner.clone(),
                repository: repository.clone(),
                reference,
                resolved_commit: resolved_commit.clone(),
                subdirectory: subdirectory.clone(),
                refreshable: *refreshable && reference_supported,
            }
        }
        InstalledSkillSourcePresentation::Provider {
            display_name,
            refreshable,
            ..
        } => SkillPreviewSourceDto::InstalledSource {
            display_name: display_name.clone(),
            refreshable: *refreshable,
        },
        InstalledSkillSourcePresentation::Unknown { provider, .. } => {
            SkillPreviewSourceDto::InstalledSource {
                display_name: provider.clone(),
                refreshable: false,
            }
        }
        _ => SkillPreviewSourceDto::InstalledSource {
            display_name: descriptor.name().to_string(),
            refreshable: false,
        },
    }
}

pub(super) fn management_state_revision(
    descriptor: &SkillDescriptor,
    installation_revision: Option<&str>,
    enabled: bool,
    generation: u64,
) -> String {
    let generation = generation.to_be_bytes();
    revision_digest(
        b"mycopilot.skill.management-state-v2\0",
        [
            descriptor.id().as_str().as_bytes(),
            descriptor.revision().as_str().as_bytes(),
            installation_revision.unwrap_or("not-installed").as_bytes(),
            generation.as_slice(),
            if enabled { b"enabled" } else { b"disabled" },
        ],
        "skill-management-state-sha256-v2:",
    )
}

pub(super) fn management_revision(
    catalog_revision: &str,
    skills: &[SkillManagementEntryDto],
) -> String {
    let mut digest = Sha256::new();
    digest.update(b"mycopilot.skill.management-catalog-v2\0");
    digest.update(2_u32.to_be_bytes());
    update_digest_bytes(&mut digest, catalog_revision.as_bytes());
    digest.update((skills.len() as u64).to_be_bytes());
    for skill in skills {
        update_digest_bytes(&mut digest, skill.id.as_bytes());
        update_digest_bytes(&mut digest, skill.state_revision.as_bytes());
    }
    format_sha256("skill-management-catalog-sha256-v2:", digest.finalize())
}

pub(super) fn revision_digest<'a>(
    domain: &[u8],
    values: impl IntoIterator<Item = &'a [u8]>,
    prefix: &str,
) -> String {
    let mut digest = Sha256::new();
    digest.update(domain);
    digest.update(1_u32.to_be_bytes());
    for value in values {
        update_digest_bytes(&mut digest, value);
    }
    format_sha256(prefix, digest.finalize())
}

pub(super) fn format_sha256(prefix: &str, bytes: impl AsRef<[u8]>) -> String {
    let mut output = String::from(prefix);
    for byte in bytes.as_ref() {
        write!(&mut output, "{byte:02x}").expect("writing to a String cannot fail");
    }
    output
}

pub(super) fn diagnostic_dtos(catalog: &SkillCatalog) -> Vec<SkillDiagnosticDto> {
    catalog
        .diagnostics()
        .iter()
        .map(|diagnostic| SkillDiagnosticDto {
            code: diagnostic.code().stable_name().to_string(),
            severity: match diagnostic.severity() {
                SkillDiagnosticSeverity::Warning => "warning",
                SkillDiagnosticSeverity::Error => "error",
                _ => "error",
            }
            .to_string(),
            message: diagnostic.message().to_string(),
            skill_id: None,
            location: Some(diagnostic.path().to_string()),
        })
        .collect()
}
