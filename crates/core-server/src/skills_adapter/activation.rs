use super::*;

#[cfg(test)]
pub(crate) fn activate_workspace(
    service: &SkillsService,
    workspace_id: &str,
    workspace_root: &Path,
    selections: &[SkillSelectionDto],
) -> Result<PreparedSkillActivation, SkillActivationFailure> {
    if selections.is_empty() {
        return Ok(PreparedSkillActivation::default());
    }

    let selections = parse_selections(selections)?;
    let activated = service
        .activate_workspace(workspace_id, workspace_root, &selections)
        .map_err(activation_failure)?;

    prepare_activated_skills(service, activated, Some(workspace_id))
}

/// Resolves one run's selected Skills while enforcing durable enablement at
/// the last server-side boundary before instructions enter the model context.
///
/// Workspace sources are request-scoped and therefore require a concrete
/// workspace. Bundled and installed sources are global: they remain usable in
/// project-less conversations, but only after their complete opaque ids have
/// been checked against persistent enablement state.
pub(crate) fn activate_selected_skills(
    storage: &StorageService,
    service: &SkillsService,
    workspace: Option<(&str, &Path)>,
    selections: &[SkillSelectionDto],
) -> Result<PreparedSkillActivation, SkillActivationFailure> {
    if selections.is_empty() {
        return Ok(PreparedSkillActivation::default());
    }

    let selections = parse_selections(selections)?;
    let mut managed_ids = Vec::new();
    let mut requires_workspace = false;
    for selection in &selections {
        let source_kind = selection
            .skill_id()
            .source_id()
            .as_str()
            .split_once(':')
            .map(|(kind, _)| kind);
        match source_kind {
            Some("bundled" | "installed") => {
                managed_ids.push(selection.skill_id().as_str().to_string());
            }
            Some("workspace") => requires_workspace = true,
            _ => {}
        }
    }

    if requires_workspace && workspace.is_none() {
        return Err(missing_workspace_failure());
    }

    let enablement = storage.load_skill_enablement(&managed_ids).map_err(|_| {
        SkillActivationFailure::enablement_unavailable(managed_ids.first().cloned())
    })?;
    if let Some(disabled_id) = managed_ids
        .iter()
        .find(|skill_id| enablement.get(*skill_id) != Some(&true))
    {
        return Err(SkillActivationFailure::disabled(disabled_id.clone()));
    }

    let (activated, expected_workspace_id) = match workspace {
        Some((workspace_id, workspace_root)) => (
            service
                .activate_workspace(workspace_id, workspace_root, &selections)
                .map_err(activation_failure)?,
            Some(workspace_id),
        ),
        None => (
            service.activate(&selections).map_err(activation_failure)?,
            None,
        ),
    };
    prepare_activated_skills(service, activated, expected_workspace_id)
}

pub(super) fn parse_selections(
    selections: &[SkillSelectionDto],
) -> Result<Vec<SkillSelection>, SkillActivationFailure> {
    selections
        .iter()
        .map(|selection| {
            SkillSelection::parse(&selection.id, &selection.revision).map_err(|error| {
                SkillActivationFailure::invalid_selection(
                    non_empty(&selection.id),
                    format!("Invalid Skill selection: {error}"),
                )
            })
        })
        .collect()
}

pub(super) fn prepare_activated_skills(
    service: &SkillsService,
    activated: mycopilot_core::skills::ActivatedSkillSet,
    expected_workspace_id: Option<&str>,
) -> Result<PreparedSkillActivation, SkillActivationFailure> {
    for skill in activated.skills() {
        validate_protocol_descriptor(skill.descriptor(), expected_workspace_id).map_err(
            |message| {
                SkillActivationFailure::invalid_selection(
                    Some(skill.id().as_str().to_string()),
                    message,
                )
            },
        )?;
    }

    let resource_session = service.resource_session(&activated).map_err(|error| {
        SkillActivationFailure::invalid_selection(
            error.skill_id().map(|id| id.as_str().to_string()),
            format!("Cannot prepare activated Skill resources: {error}"),
        )
    })?;
    let revision = activated.revision().as_str().to_string();
    let summaries = activated
        .skills()
        .iter()
        .map(|skill| {
            Ok(ActivatedSkillSummaryDto {
                id: skill.id().as_str().to_string(),
                name: skill.name().to_string(),
                revision: skill.revision().as_str().to_string(),
                source: source_dto(skill.descriptor())?,
            })
        })
        .collect::<Result<Vec<_>, String>>()
        .map_err(|message| SkillActivationFailure::invalid_selection(None, message))?;
    let runtime = AgentSkillActivation {
        activation_revision: revision.clone(),
        skills: activated
            .skills()
            .iter()
            .map(|skill| {
                let resources = (!skill.resources().is_empty()).then(|| {
                    let mut kinds = skill
                        .resources()
                        .entries()
                        .iter()
                        .map(|resource| resource.kind().stable_name().to_string())
                        .collect::<std::collections::BTreeSet<_>>()
                        .into_iter()
                        .collect::<Vec<_>>();
                    kinds.shrink_to_fit();
                    AgentActivatedSkillResources {
                        root_uri: mycopilot_core::skills::SkillPackageUri::new(
                            skill.id().clone(),
                            skill.revision().clone(),
                        )
                        .to_string(),
                        resource_count: u64::try_from(skill.resources().len()).unwrap_or(u64::MAX),
                        kinds,
                    }
                });
                AgentActivatedSkill {
                    id: skill.id().as_str().to_string(),
                    name: skill.name().to_string(),
                    revision: skill.revision().as_str().to_string(),
                    source: skill.id().source_id().as_str().to_string(),
                    instructions: skill.instructions().to_string(),
                    resources,
                }
            })
            .collect(),
    };

    Ok(PreparedSkillActivation {
        runtime: Some(runtime),
        resources: Some(std::sync::Arc::new(resource_session)),
        summaries,
        revision: Some(revision),
    })
}

pub(crate) fn missing_workspace_failure() -> SkillActivationFailure {
    SkillActivationFailure::invalid_selection(
        None,
        "Activating a Skill requires a project with a local workspace.",
    )
}

pub(super) fn descriptor_dto(descriptor: &SkillDescriptor) -> Result<SkillDescriptorDto, String> {
    let location = validate_protocol_descriptor(descriptor, None)?;
    Ok(SkillDescriptorDto {
        id: descriptor.id().as_str().to_string(),
        name: descriptor.name().to_string(),
        description: descriptor.description().to_string(),
        source: source_dto(descriptor)?,
        trust: trust_dto(descriptor.trust())?,
        activation_scope: activation_scope_name(descriptor.activation_scope())?.to_string(),
        revision: descriptor.revision().as_str().to_string(),
        location,
    })
}

pub(super) fn source_dto(descriptor: &SkillDescriptor) -> Result<SkillSourceDto, String> {
    let kind = match descriptor.source_kind() {
        SkillSourceKind::Workspace => SkillSourceKindDto::Workspace,
        SkillSourceKind::Bundled => SkillSourceKindDto::Bundled,
        SkillSourceKind::Installed => SkillSourceKindDto::Installed,
        unsupported => {
            return Err(format!(
                "Skill `{}` uses unsupported source kind `{}` for catalog schema v4",
                descriptor.id(),
                unsupported.stable_name()
            ));
        }
    };
    Ok(SkillSourceDto {
        kind,
        id: descriptor.id().source_id().as_str().to_string(),
    })
}

pub(super) fn trust_dto(trust: SkillTrust) -> Result<SkillTrustDto, String> {
    match trust {
        SkillTrust::Untrusted => Ok(SkillTrustDto::Untrusted),
        SkillTrust::Application => Ok(SkillTrustDto::Application),
        unsupported => Err(format!(
            "unsupported Skill trust `{}` for catalog schema v4",
            unsupported.stable_name()
        )),
    }
}

pub(super) fn activation_scope_name(scope: SkillActivationScope) -> Result<&'static str, String> {
    match scope {
        SkillActivationScope::Run => Ok("run"),
        unsupported => Err(format!(
            "unsupported Skill activation scope `{}` for catalog schema v4",
            unsupported.stable_name()
        )),
    }
}

pub(super) fn validate_protocol_descriptor(
    descriptor: &SkillDescriptor,
    expected_workspace_id: Option<&str>,
) -> Result<Option<String>, String> {
    let source_kind = source_dto(descriptor)?.kind;
    let trust = trust_dto(descriptor.trust())?;
    activation_scope_name(descriptor.activation_scope())?;

    let supported_contract = supports_protocol_contract(source_kind, trust);
    if !supported_contract {
        return Err(format!(
            "Skill `{}` cannot cross catalog schema v4 with source `{}` and trust `{}`",
            descriptor.id(),
            descriptor.source_kind().stable_name(),
            descriptor.trust().stable_name()
        ));
    }

    match descriptor.provenance() {
        SkillProvenance::Workspace {
            workspace_id,
            relative_path,
        } if source_kind == SkillSourceKindDto::Workspace
            && expected_workspace_id.is_none_or(|expected| expected == workspace_id) =>
        {
            Ok(Some(relative_path.clone()))
        }
        SkillProvenance::Workspace { .. } => Err(format!(
            "Skill `{}` has workspace provenance incompatible with its source or project",
            descriptor.id()
        )),
        SkillProvenance::Bundled {
            source_id,
            relative_path,
        } if source_kind == SkillSourceKindDto::Bundled
            && source_id == descriptor.id().source_id() =>
        {
            Ok(Some(relative_path.clone()))
        }
        SkillProvenance::Bundled { .. } => Err(format!(
            "Skill `{}` has bundled provenance incompatible with its source",
            descriptor.id()
        )),
        SkillProvenance::Installed {
            source_id,
            installation_id,
            relative_path,
        } if source_kind == SkillSourceKindDto::Installed
            && source_id == descriptor.id().source_id()
            && descriptor.id().as_str() == format!("{source_id}:{}", installation_id.as_str()) =>
        {
            Ok(Some(relative_path.clone()))
        }
        SkillProvenance::Installed { .. } => Err(format!(
            "Skill `{}` has installed provenance incompatible with its source or installation",
            descriptor.id()
        )),
        SkillProvenance::Other { .. } => Err(format!(
            "Skill `{}` uses unsupported provenance for catalog schema v4",
            descriptor.id()
        )),
        _ => Err(format!(
            "Skill `{}` uses unknown provenance for catalog schema v4",
            descriptor.id()
        )),
    }
}

pub(super) fn supports_protocol_contract(
    source_kind: SkillSourceKindDto,
    trust: SkillTrustDto,
) -> bool {
    matches!(
        (source_kind, trust),
        (SkillSourceKindDto::Workspace, SkillTrustDto::Untrusted)
            | (SkillSourceKindDto::Bundled, SkillTrustDto::Application)
            | (SkillSourceKindDto::Installed, SkillTrustDto::Untrusted)
    )
}

pub(super) fn activation_failure(error: SkillActivationError) -> SkillActivationFailure {
    let code = match error.code() {
        SkillErrorCode::InvalidReference => SkillActivationErrorCodeDto::InvalidSelection,
        SkillErrorCode::DuplicateSelection => SkillActivationErrorCodeDto::DuplicateSelection,
        SkillErrorCode::TooManySkills => SkillActivationErrorCodeDto::TooManySkills,
        SkillErrorCode::SourceBudgetExceeded => SkillActivationErrorCodeDto::ActivationTooLarge,
        SkillErrorCode::NotFound => SkillActivationErrorCodeDto::NotFound,
        SkillErrorCode::Stale => SkillActivationErrorCodeDto::Stale,
        SkillErrorCode::InvalidSkill => SkillActivationErrorCodeDto::InvalidSkill,
        _ => SkillActivationErrorCodeDto::SourceUnavailable,
    };
    let recovery = match error.recovery() {
        SkillRecovery::Retry => SkillActivationRecoveryDto::RetrySameSelection,
        SkillRecovery::RefreshCatalog
        | SkillRecovery::RepairSkill
        | SkillRecovery::ReconfigureSource => SkillActivationRecoveryDto::RefreshCatalog,
        SkillRecovery::ChangeSelection | SkillRecovery::ReduceSelection => {
            SkillActivationRecoveryDto::RejectSelection
        }
        _ => SkillActivationRecoveryDto::RejectSelection,
    };
    SkillActivationFailure {
        data: Box::new(SkillActivationErrorData {
            error_type: "skillActivation",
            code,
            recovery,
            message: error.message(),
            skill_id: error.skill_id().map(|id| id.as_str().to_string()),
            expected_revision: error
                .expected_revision()
                .map(|revision| revision.as_str().to_string()),
            actual_revision: error
                .actual_revision()
                .map(|revision| revision.as_str().to_string()),
        }),
    }
}

pub(super) fn non_empty(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}
