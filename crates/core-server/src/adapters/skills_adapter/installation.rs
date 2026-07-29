use super::*;

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

    if let SkillInstallationServiceError::LegacyRevisionConflict {
        expected_revision,
        actual_revision,
        ..
    } = error
    {
        data.code = SkillInstallationErrorCodeDto::RevisionConflict;
        data.recovery = SkillInstallationRecoveryDto::RefreshCatalog;
        data.expected_revision = Some(expected_revision.as_str().to_string());
        data.actual_revision = Some(actual_revision.as_str().to_string());
    } else if let Some(source) = error.preparation_error() {
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

pub(super) fn installation_error_message(code: SkillInstallationErrorCodeDto) -> &'static str {
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
        SkillInstallationErrorCodeDto::InstallationRetired => {
            "This installation identity was already used and permanently retired."
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

pub(super) fn installation_operation_dto(
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

pub(super) fn map_installer_error(
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
        ManagedSkillInstallerErrorCode::InstallationRetired => (
            SkillInstallationErrorCodeDto::InstallationRetired,
            SkillInstallationRecoveryDto::NewInstallationIdentity,
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
                ManagedSkillStoreCapacity::RetiredInstallationIds => {
                    data.recovery = SkillInstallationRecoveryDto::ContactSupport;
                    SkillInstallationCapacityDto::RetiredInstallationIds
                }
                ManagedSkillStoreCapacity::Packages => {
                    data.recovery = SkillInstallationRecoveryDto::ContactSupport;
                    SkillInstallationCapacityDto::Packages
                }
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
