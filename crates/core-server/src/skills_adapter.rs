//! Explicit boundary between the Skill domain model, runtime snapshots, and JSON-RPC DTOs.

use std::path::Path;

use mycopilot_core::skills::{
    ManagedSkillInstallerError, ManagedSkillInstallerErrorCode, ManagedSkillStoreCapacity,
    SkillActivationError, SkillActivationScope, SkillCatalog, SkillDescriptor,
    SkillDiagnosticSeverity, SkillErrorCode, SkillInstallationMutation, SkillInstallationOperation,
    SkillInstallationOutcome, SkillInstallationServiceError, SkillProvenance, SkillRecovery,
    SkillSelection, SkillSourceKind, SkillTrust, SkillsService,
};
use mycopilot_core::{AgentActivatedSkill, AgentSkillActivation};
use mycopilot_protocol_rs::{
    ActivatedSkillSummaryDto, SkillActivationErrorData, SkillDescriptorDto, SkillDiagnosticDto,
    SkillInstallMutationOutcomeDto, SkillInstallationCapacityDto, SkillInstallationErrorCodeDto,
    SkillInstallationErrorData, SkillInstallationErrorTypeDto, SkillInstallationOperationDto,
    SkillInstallationRecoveryDto, SkillMutationResponse, SkillRemovalMutationOutcomeDto,
    SkillSelectionDto, SkillSourceDto, SkillSourceKindDto, SkillTrustDto,
    SkillUpdateMutationOutcomeDto, SkillsListResponse, SKILL_CATALOG_SCHEMA_VERSION,
};

#[derive(Debug, Default)]
pub(crate) struct PreparedSkillActivation {
    pub(crate) runtime: Option<AgentSkillActivation>,
    pub(crate) summaries: Vec<ActivatedSkillSummaryDto>,
    pub(crate) revision: Option<String>,
}

#[derive(Debug)]
pub(crate) struct SkillActivationFailure {
    data: Box<SkillActivationErrorData>,
}

#[derive(Debug)]
pub(crate) struct SkillInstallationFailure {
    data: Box<SkillInstallationErrorData>,
}

impl SkillInstallationFailure {
    pub(crate) fn into_data(self) -> Box<SkillInstallationErrorData> {
        self.data
    }
}

impl std::fmt::Display for SkillInstallationFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.data.message)
    }
}

impl std::error::Error for SkillInstallationFailure {}

pub(crate) fn mutation_response(
    mutation: &SkillInstallationMutation,
) -> Result<SkillMutationResponse, String> {
    let installation_id = mutation.installation_id().as_str().to_string();
    let skill_id = mutation.skill_id().as_str().to_string();
    let revision = || {
        mutation
            .revision()
            .map(|revision| revision.as_str().to_string())
            .ok_or_else(|| "package Skill mutation did not carry a revision".to_string())
    };
    match (mutation.operation(), mutation.outcome()) {
        (SkillInstallationOperation::Install, SkillInstallationOutcome::Installed) => {
            Ok(SkillMutationResponse::install(
                installation_id,
                skill_id,
                revision()?,
                SkillInstallMutationOutcomeDto::Installed,
            ))
        }
        (SkillInstallationOperation::Install, SkillInstallationOutcome::AlreadyInstalled) => {
            Ok(SkillMutationResponse::install(
                installation_id,
                skill_id,
                revision()?,
                SkillInstallMutationOutcomeDto::AlreadyInstalled,
            ))
        }
        (SkillInstallationOperation::Update, SkillInstallationOutcome::Updated) => {
            Ok(SkillMutationResponse::update(
                installation_id,
                skill_id,
                revision()?,
                SkillUpdateMutationOutcomeDto::Updated,
            ))
        }
        (SkillInstallationOperation::Update, SkillInstallationOutcome::AlreadyCurrent) => {
            Ok(SkillMutationResponse::update(
                installation_id,
                skill_id,
                revision()?,
                SkillUpdateMutationOutcomeDto::AlreadyCurrent,
            ))
        }
        (SkillInstallationOperation::Uninstall, SkillInstallationOutcome::Uninstalled)
            if mutation.revision().is_none() =>
        {
            Ok(SkillMutationResponse::removal(
                installation_id,
                skill_id,
                SkillRemovalMutationOutcomeDto::Uninstalled,
            ))
        }
        (SkillInstallationOperation::Uninstall, SkillInstallationOutcome::AlreadyAbsent)
            if mutation.revision().is_none() =>
        {
            Ok(SkillMutationResponse::removal(
                installation_id,
                skill_id,
                SkillRemovalMutationOutcomeDto::AlreadyAbsent,
            ))
        }
        (operation, outcome) => Err(format!(
            "Skill mutation outcome `{}` is incompatible with operation `{}` or its revision contract",
            outcome.stable_name(),
            operation.stable_name()
        )),
    }
}

pub(crate) fn installation_failure(
    error: &SkillInstallationServiceError,
) -> Result<SkillInstallationFailure, String> {
    let operation = installation_operation_dto(error.operation())?;
    let mut data = SkillInstallationErrorData {
        error_type: SkillInstallationErrorTypeDto::SkillInstallation,
        operation,
        code: SkillInstallationErrorCodeDto::InvalidSkill,
        recovery: SkillInstallationRecoveryDto::RefreshCatalog,
        message: String::new(),
        commit_may_have_succeeded: false,
        installation_id: error.installation_id().map(|id| id.as_str().to_string()),
        skill_id: Some(error.skill_id().as_str().to_string()),
        diagnostic_code: None,
        intended_revision: None,
        expected_revision: None,
        actual_revision: None,
        capacity: None,
        limit: None,
    };

    if let Some(source) = error.preparation_error() {
        data.code = SkillInstallationErrorCodeDto::PreparationFailed;
        data.recovery = SkillInstallationRecoveryDto::FixLocalSource;
        data.diagnostic_code = Some(source.code().stable_name().to_string());
    } else if let Some(source) = error.installer_error() {
        map_installer_error(source, &mut data)?;
    }
    data.message = installation_error_message(data.code).to_string();

    Ok(SkillInstallationFailure {
        data: Box::new(data),
    })
}

fn installation_error_message(code: SkillInstallationErrorCodeDto) -> &'static str {
    match code {
        SkillInstallationErrorCodeDto::PreparationFailed => {
            "The selected local Skill directory could not be prepared."
        }
        SkillInstallationErrorCodeDto::InvalidSkill => {
            "The selected Skill is not a user-managed installation."
        }
        SkillInstallationErrorCodeDto::InvalidStore => "The managed Skill store is unavailable.",
        SkillInstallationErrorCodeDto::CapacityExceeded => {
            "The managed Skill store has reached its capacity."
        }
        SkillInstallationErrorCodeDto::InstallationExists => {
            "This installation identity is already in use."
        }
        SkillInstallationErrorCodeDto::InstallationNotFound => {
            "The installed Skill no longer exists."
        }
        SkillInstallationErrorCodeDto::RevisionConflict => {
            "The installed Skill changed; refresh the catalog and retry."
        }
        SkillInstallationErrorCodeDto::StoreCorrupt => "The managed Skill store is corrupted.",
        SkillInstallationErrorCodeDto::Io | SkillInstallationErrorCodeDto::Unavailable => {
            "The Skill operation could not be completed. Retry the same request."
        }
        SkillInstallationErrorCodeDto::CommitIndeterminate => {
            "The Skill operation may have completed. Retry the same request."
        }
        SkillInstallationErrorCodeDto::Cancelled => {
            "The Skill operation was cancelled before it started. Retry the same request."
        }
    }
}

fn installation_operation_dto(
    operation: SkillInstallationOperation,
) -> Result<SkillInstallationOperationDto, String> {
    match operation {
        SkillInstallationOperation::Install => Ok(SkillInstallationOperationDto::Install),
        SkillInstallationOperation::Update => Ok(SkillInstallationOperationDto::Update),
        SkillInstallationOperation::Uninstall => Ok(SkillInstallationOperationDto::Uninstall),
        unsupported => Err(format!(
            "unsupported Skill installation operation `{}`",
            unsupported.stable_name()
        )),
    }
}

fn map_installer_error(
    source: &ManagedSkillInstallerError,
    data: &mut SkillInstallationErrorData,
) -> Result<(), String> {
    (data.code, data.recovery) = match source.code() {
        ManagedSkillInstallerErrorCode::InvalidStore => (
            SkillInstallationErrorCodeDto::InvalidStore,
            SkillInstallationRecoveryDto::RepairStore,
        ),
        ManagedSkillInstallerErrorCode::CapacityExceeded => (
            SkillInstallationErrorCodeDto::CapacityExceeded,
            SkillInstallationRecoveryDto::FreeCapacity,
        ),
        ManagedSkillInstallerErrorCode::InstallationExists => (
            SkillInstallationErrorCodeDto::InstallationExists,
            SkillInstallationRecoveryDto::RefreshCatalog,
        ),
        ManagedSkillInstallerErrorCode::InstallationNotFound => (
            SkillInstallationErrorCodeDto::InstallationNotFound,
            SkillInstallationRecoveryDto::RefreshCatalog,
        ),
        ManagedSkillInstallerErrorCode::RevisionConflict => (
            SkillInstallationErrorCodeDto::RevisionConflict,
            SkillInstallationRecoveryDto::RefreshCatalog,
        ),
        ManagedSkillInstallerErrorCode::StoreCorrupt => (
            SkillInstallationErrorCodeDto::StoreCorrupt,
            SkillInstallationRecoveryDto::RepairStore,
        ),
        ManagedSkillInstallerErrorCode::Io => (
            SkillInstallationErrorCodeDto::Io,
            SkillInstallationRecoveryDto::RetrySameRequest,
        ),
        ManagedSkillInstallerErrorCode::CommitIndeterminate => (
            SkillInstallationErrorCodeDto::CommitIndeterminate,
            SkillInstallationRecoveryDto::RetrySameRequest,
        ),
        unsupported => {
            return Err(format!(
                "unsupported managed Skill installer error `{}`",
                unsupported.stable_name()
            ));
        }
    };
    data.commit_may_have_succeeded = source.commit_may_have_succeeded();

    match source {
        ManagedSkillInstallerError::CapacityExceeded { capacity, limit } => {
            data.capacity = Some(match capacity {
                ManagedSkillStoreCapacity::Installations => {
                    SkillInstallationCapacityDto::Installations
                }
                ManagedSkillStoreCapacity::InstallationDirectory => {
                    SkillInstallationCapacityDto::InstallationDirectory
                }
                ManagedSkillStoreCapacity::Packages => SkillInstallationCapacityDto::Packages,
                unsupported => {
                    return Err(format!(
                        "unsupported managed Skill capacity `{}`",
                        unsupported.stable_name()
                    ));
                }
            });
            data.limit = Some(*limit);
        }
        ManagedSkillInstallerError::InstallationExists {
            existing_revision,
            requested_revision,
            ..
        } => {
            data.actual_revision = Some(existing_revision.as_str().to_string());
            data.intended_revision = Some(requested_revision.as_str().to_string());
        }
        ManagedSkillInstallerError::RevisionConflict {
            expected_revision,
            actual_revision,
            ..
        } => {
            data.expected_revision = Some(expected_revision.as_str().to_string());
            data.actual_revision = Some(actual_revision.as_str().to_string());
        }
        ManagedSkillInstallerError::CommitIndeterminate {
            intended_revision, ..
        } => {
            data.intended_revision = intended_revision
                .as_ref()
                .map(|revision| revision.as_str().to_string());
        }
        _ => {}
    }
    Ok(())
}

impl SkillActivationFailure {
    pub(crate) fn into_data(self) -> Box<SkillActivationErrorData> {
        self.data
    }

    fn invalid_selection(skill_id: Option<String>, message: impl Into<String>) -> Self {
        let message = message.into();
        Self {
            data: Box::new(SkillActivationErrorData {
                error_type: "skillActivation",
                code: "invalidSelection".to_string(),
                recovery: "rejectSelection".to_string(),
                message,
                skill_id,
                expected_revision: None,
                actual_revision: None,
            }),
        }
    }
}

impl std::fmt::Display for SkillActivationFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.data.message)
    }
}

impl std::error::Error for SkillActivationFailure {}

pub(crate) fn catalog_response(catalog: &SkillCatalog) -> Result<SkillsListResponse, String> {
    Ok(SkillsListResponse {
        schema_version: SKILL_CATALOG_SCHEMA_VERSION,
        catalog_revision: catalog.catalog_revision().to_string(),
        skills: catalog
            .skills()
            .iter()
            .map(descriptor_dto)
            .collect::<Result<Vec<_>, _>>()?,
        diagnostics: catalog
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
            .collect(),
        truncated: catalog.truncated(),
    })
}

pub(crate) fn activate_workspace(
    service: &SkillsService,
    workspace_id: &str,
    workspace_root: &Path,
    selections: &[SkillSelectionDto],
) -> Result<PreparedSkillActivation, SkillActivationFailure> {
    if selections.is_empty() {
        return Ok(PreparedSkillActivation::default());
    }

    let selections = selections
        .iter()
        .map(|selection| {
            SkillSelection::parse(&selection.id, &selection.revision).map_err(|error| {
                SkillActivationFailure::invalid_selection(
                    non_empty(&selection.id),
                    format!("Invalid Skill selection: {error}"),
                )
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let activated = service
        .activate_workspace(workspace_id, workspace_root, &selections)
        .map_err(activation_failure)?;

    for skill in activated.skills() {
        validate_protocol_descriptor(skill.descriptor(), Some(workspace_id)).map_err(
            |message| {
                SkillActivationFailure::invalid_selection(
                    Some(skill.id().as_str().to_string()),
                    message,
                )
            },
        )?;
    }

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
            .map(|skill| AgentActivatedSkill {
                id: skill.id().as_str().to_string(),
                name: skill.name().to_string(),
                revision: skill.revision().as_str().to_string(),
                source: skill.id().source_id().as_str().to_string(),
                instructions: skill.instructions().to_string(),
            })
            .collect(),
    };

    Ok(PreparedSkillActivation {
        runtime: Some(runtime),
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

fn descriptor_dto(descriptor: &SkillDescriptor) -> Result<SkillDescriptorDto, String> {
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

fn source_dto(descriptor: &SkillDescriptor) -> Result<SkillSourceDto, String> {
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

fn trust_dto(trust: SkillTrust) -> Result<SkillTrustDto, String> {
    match trust {
        SkillTrust::Untrusted => Ok(SkillTrustDto::Untrusted),
        SkillTrust::Application => Ok(SkillTrustDto::Application),
        unsupported => Err(format!(
            "unsupported Skill trust `{}` for catalog schema v4",
            unsupported.stable_name()
        )),
    }
}

fn activation_scope_name(scope: SkillActivationScope) -> Result<&'static str, String> {
    match scope {
        SkillActivationScope::Run => Ok("run"),
        unsupported => Err(format!(
            "unsupported Skill activation scope `{}` for catalog schema v4",
            unsupported.stable_name()
        )),
    }
}

fn validate_protocol_descriptor(
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

fn supports_protocol_contract(source_kind: SkillSourceKindDto, trust: SkillTrustDto) -> bool {
    matches!(
        (source_kind, trust),
        (SkillSourceKindDto::Workspace, SkillTrustDto::Untrusted)
            | (SkillSourceKindDto::Bundled, SkillTrustDto::Application)
            | (SkillSourceKindDto::Installed, SkillTrustDto::Untrusted)
    )
}

fn activation_failure(error: SkillActivationError) -> SkillActivationFailure {
    let code = match error.code() {
        SkillErrorCode::InvalidReference => "invalidSelection",
        SkillErrorCode::DuplicateSelection => "duplicateSelection",
        SkillErrorCode::TooManySkills => "tooManySkills",
        SkillErrorCode::SourceBudgetExceeded => "activationTooLarge",
        SkillErrorCode::NotFound => "notFound",
        SkillErrorCode::Stale => "stale",
        SkillErrorCode::InvalidSkill => "invalidSkill",
        _ => "sourceUnavailable",
    };
    let recovery = match error.recovery() {
        SkillRecovery::Retry => "retrySameSelection",
        SkillRecovery::RefreshCatalog
        | SkillRecovery::RepairSkill
        | SkillRecovery::ReconfigureSource => "refreshCatalog",
        SkillRecovery::ChangeSelection | SkillRecovery::ReduceSelection => "rejectSelection",
        _ => "rejectSelection",
    };
    SkillActivationFailure {
        data: Box::new(SkillActivationErrorData {
            error_type: "skillActivation",
            code: code.to_string(),
            recovery: recovery.to_string(),
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

fn non_empty(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills_test_support::write_installed_skill;

    #[test]
    fn bundled_activation_crosses_schema_v4_as_an_opaque_selection() {
        let workspace = tempfile::tempdir().unwrap();
        let service = SkillsService::new().with_bundled_source().unwrap();
        let descriptor = service.list().unwrap().skills()[0].clone();
        let prepared = activate_workspace(
            &service,
            "project-1",
            workspace.path(),
            &[SkillSelectionDto {
                id: descriptor.id().as_str().to_string(),
                revision: descriptor.revision().as_str().to_string(),
            }],
        )
        .unwrap();

        assert_eq!(prepared.summaries.len(), 1);
        assert_eq!(
            prepared.summaries[0].source.kind,
            SkillSourceKindDto::Bundled
        );
        assert_eq!(prepared.summaries[0].source.id, "bundled:application");
        let runtime = prepared.runtime.unwrap();
        assert_eq!(runtime.skills.len(), 1);
        assert_eq!(runtime.skills[0].source, "bundled:application");
    }

    #[test]
    fn installed_activation_crosses_schema_v4_as_untrusted_opaque_selection() {
        const INSTALLATION_ID: &str = "0190b0f2-7c50-7cc0-8b25-3bb80f08b334";
        const INSTRUCTION_MARKER: &str = "INSTALLED_SKILL_RUNTIME_MARKER";
        let fixture = tempfile::tempdir().unwrap();
        let store_root = fixture.path().join("skills");
        let source_text = format!(
            concat!(
                "---\n",
                "name: installed-auditor\n",
                "description: Audit a repository from an installed package.\n",
                "---\n",
                "# Instructions\n",
                "{}\n"
            ),
            INSTRUCTION_MARKER
        );
        let revision = write_installed_skill(&store_root, INSTALLATION_ID, &source_text);
        let service = SkillsService::new()
            .with_installed_source(&store_root)
            .unwrap();
        let catalog = service.list().unwrap();
        assert!(catalog.diagnostics().is_empty());
        let descriptor = catalog.skills().first().unwrap();
        assert_eq!(
            descriptor.id().as_str(),
            format!("installed:user:{INSTALLATION_ID}")
        );
        assert_eq!(descriptor.revision().as_str(), revision);

        let response = catalog_response(&catalog).unwrap();
        assert_eq!(response.schema_version, 4);
        assert_eq!(
            response.skills[0].source.kind,
            SkillSourceKindDto::Installed
        );
        assert_eq!(response.skills[0].source.id, "installed:user");
        assert_eq!(response.skills[0].trust, SkillTrustDto::Untrusted);

        let prepared = activate_workspace(
            &service,
            "project-1",
            fixture.path(),
            &[SkillSelectionDto {
                id: descriptor.id().as_str().to_string(),
                revision,
            }],
        )
        .unwrap();

        assert_eq!(prepared.summaries.len(), 1);
        assert_eq!(
            prepared.summaries[0].source.kind,
            SkillSourceKindDto::Installed
        );
        assert_eq!(prepared.summaries[0].source.id, "installed:user");
        let runtime = prepared.runtime.unwrap();
        assert_eq!(runtime.skills.len(), 1);
        assert_eq!(runtime.skills[0].source, "installed:user");
        assert!(runtime.skills[0].instructions.contains(INSTRUCTION_MARKER));
    }

    #[test]
    fn schema_v4_rejects_domain_trust_not_represented_by_the_protocol() {
        let error = trust_dto(SkillTrust::UserApproved).unwrap_err();

        assert!(error.contains("userApproved"));
        assert!(error.contains("schema v4"));
    }

    #[test]
    fn schema_v4_does_not_treat_installation_as_application_trust() {
        assert!(supports_protocol_contract(
            SkillSourceKindDto::Installed,
            SkillTrustDto::Untrusted
        ));
        assert!(!supports_protocol_contract(
            SkillSourceKindDto::Installed,
            SkillTrustDto::Application
        ));
    }

    #[test]
    fn installation_io_details_are_not_exposed_across_json_rpc() {
        use mycopilot_core::skills::{SkillId, SkillInstallationId};

        const INSTALLATION_ID: &str = "0190b0f2-7c50-7cc0-8b25-3bb80f08b334";
        const PRIVATE_PATH: &str = "/Users/private-account/Library/Application Support/skills";
        let installation_id = SkillInstallationId::parse(INSTALLATION_ID).unwrap();
        let skill_id = SkillId::parse(format!("installed:user:{INSTALLATION_ID}")).unwrap();
        let error = SkillInstallationServiceError::Installer {
            operation: SkillInstallationOperation::Install,
            installation_id: installation_id.clone(),
            skill_id,
            source: Box::new(ManagedSkillInstallerError::Io {
                operation: format!("publish package under {PRIVATE_PATH}"),
                reason: format!("permission denied while syncing {PRIVATE_PATH}"),
            }),
        };

        let failure = installation_failure(&error).unwrap();
        let public_message = failure.to_string();
        let data = failure.into_data();
        let serialized = serde_json::to_string(&data).unwrap();

        assert_eq!(data.code, SkillInstallationErrorCodeDto::Io);
        assert_eq!(
            public_message,
            "The Skill operation could not be completed. Retry the same request."
        );
        assert_eq!(data.message, public_message);
        assert!(!serialized.contains(PRIVATE_PATH));
        assert!(!serialized.contains("private-account"));
        assert!(!serialized.contains("permission denied"));
    }

    #[test]
    fn commit_indeterminate_preserves_retry_identity_without_a_path() {
        use mycopilot_core::skills::{
            ManagedSkillMutation, SkillId, SkillInstallationId, SkillRevision,
        };

        const INSTALLATION_ID: &str = "0190b0f2-7c50-7cc0-8b25-3bb80f08b334";
        let installation_id = SkillInstallationId::parse(INSTALLATION_ID).unwrap();
        let skill_id = SkillId::parse(format!("installed:user:{INSTALLATION_ID}")).unwrap();
        let intended_revision = SkillRevision::parse("intended-revision").unwrap();
        let error = SkillInstallationServiceError::Installer {
            operation: SkillInstallationOperation::Update,
            installation_id: installation_id.clone(),
            skill_id: skill_id.clone(),
            source: Box::new(ManagedSkillInstallerError::CommitIndeterminate {
                operation: ManagedSkillMutation::Update,
                installation_id,
                intended_revision: Some(intended_revision.clone()),
                reason: "receipt directory sync acknowledgement was lost".to_string(),
            }),
        };

        let data = installation_failure(&error).unwrap().into_data();

        assert_eq!(
            data.error_type,
            SkillInstallationErrorTypeDto::SkillInstallation
        );
        assert_eq!(data.operation, SkillInstallationOperationDto::Update);
        assert_eq!(
            data.code,
            SkillInstallationErrorCodeDto::CommitIndeterminate
        );
        assert_eq!(
            data.recovery,
            SkillInstallationRecoveryDto::RetrySameRequest
        );
        assert!(data.commit_may_have_succeeded);
        assert_eq!(data.skill_id.as_deref(), Some(skill_id.as_str()));
        assert_eq!(
            data.intended_revision.as_deref(),
            Some(intended_revision.as_str())
        );
        assert!(!serde_json::to_value(data)
            .unwrap()
            .to_string()
            .contains("path"));
    }

    #[test]
    fn non_installed_targets_map_to_a_refreshable_invalid_skill_error() {
        let skill_id = mycopilot_core::skills::SkillId::parse("workspace:project:auditor").unwrap();
        let error = SkillInstallationServiceError::InvalidInstalledSkill {
            operation: SkillInstallationOperation::Uninstall,
            skill_id: skill_id.clone(),
            reason: "not a managed installation".to_string(),
        };

        let data = installation_failure(&error).unwrap().into_data();

        assert_eq!(data.code, SkillInstallationErrorCodeDto::InvalidSkill);
        assert_eq!(data.recovery, SkillInstallationRecoveryDto::RefreshCatalog);
        assert_eq!(data.skill_id.as_deref(), Some(skill_id.as_str()));
        assert!(!data.commit_may_have_succeeded);
    }
}
