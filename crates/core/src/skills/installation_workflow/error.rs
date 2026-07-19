use super::*;

#[non_exhaustive]
pub enum SkillInstallationWorkflowError {
    UnknownAcquisitionProvider {
        provider: SkillAcquisitionProvider,
    },
    InvalidUpdateTarget {
        skill_id: SkillId,
        reason: String,
    },
    InstalledSourceRequiresUpdate,
    InstalledSkillNotFound {
        installation_id: SkillInstallationId,
    },
    InstalledSkillRead {
        installation_id: SkillInstallationId,
        reason: String,
    },
    InstalledSourceRevisionConflict {
        installation_id: SkillInstallationId,
        expected_revision: SkillInstallationRevision,
        actual_revision: SkillInstallationRevision,
    },
    InstalledSourceLegacy {
        installation_id: SkillInstallationId,
    },
    InstalledSourceNotRefreshable {
        installation_id: SkillInstallationId,
    },
    UnknownRefreshProvider {
        provider: String,
    },
    UnsupportedRefreshSchema {
        provider: String,
        schema_version: u32,
    },
    InvalidInstalledSourceProvenance {
        provider: String,
    },
    PreparationConflict {
        preparation_id: SkillPreparationId,
    },
    PreparationNotFoundOrExpired {
        preparation_id: SkillPreparationId,
    },
    PreparationCancelled {
        preparation_id: SkillPreparationId,
    },
    PreparationAlreadyCommitted {
        preparation_id: SkillPreparationId,
    },
    PreparationBusy {
        preparation_id: SkillPreparationId,
    },
    PreviewMismatch {
        preparation_id: SkillPreparationId,
        expected: SkillPreviewRevision,
        actual: SkillPreviewRevision,
    },
    PreparationCapacityExceeded {
        max_preparations: usize,
    },
    PreparationMemoryCapacityExceeded {
        max_snapshot_bytes: usize,
    },
    SourceResolutionNotFoundOrExpired {
        resolution_id: SkillSourceResolutionId,
    },
    SourceResolutionBusy {
        resolution_id: SkillSourceResolutionId,
    },
    SourceResolutionConsumed {
        resolution_id: SkillSourceResolutionId,
    },
    SourceResolutionCancelled {
        resolution_id: SkillSourceResolutionId,
    },
    SourceCandidateNotFound {
        resolution_id: SkillSourceResolutionId,
        candidate_id: SkillSourceCandidateId,
    },
    WarningAcknowledgementRequired {
        preparation_id: SkillPreparationId,
        missing: Vec<SkillInstallationWarningCode>,
    },
    Acquisition {
        provider: SkillAcquisitionProvider,
        source: Box<SkillAcquisitionAdapterError>,
    },
    Installation {
        preparation_id: SkillPreparationId,
        source: Box<SkillInstallationServiceError>,
    },
    Internal {
        operation: &'static str,
        reason: String,
    },
}

impl SkillInstallationWorkflowError {
    pub fn preparation_id(&self) -> Option<&SkillPreparationId> {
        match self {
            Self::PreparationConflict { preparation_id }
            | Self::PreparationNotFoundOrExpired { preparation_id }
            | Self::PreparationCancelled { preparation_id }
            | Self::PreparationAlreadyCommitted { preparation_id }
            | Self::PreparationBusy { preparation_id }
            | Self::PreviewMismatch { preparation_id, .. }
            | Self::WarningAcknowledgementRequired { preparation_id, .. }
            | Self::Installation { preparation_id, .. } => Some(preparation_id),
            _ => None,
        }
    }

    pub fn acquisition_error(&self) -> Option<&SkillAcquisitionAdapterError> {
        match self {
            Self::Acquisition { source, .. } => Some(source),
            _ => None,
        }
    }

    pub fn installation_error(&self) -> Option<&SkillInstallationServiceError> {
        match self {
            Self::Installation { source, .. } => Some(source),
            _ => None,
        }
    }

    pub fn missing_warning_acknowledgements(&self) -> Option<&[SkillInstallationWarningCode]> {
        match self {
            Self::WarningAcknowledgementRequired { missing, .. } => Some(missing),
            _ => None,
        }
    }

    pub fn internal_reason(&self) -> Option<&str> {
        match self {
            Self::Internal { reason, .. } => Some(reason),
            _ => None,
        }
    }
}

impl fmt::Debug for SkillInstallationWorkflowError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut debug = formatter.debug_struct("SkillInstallationWorkflowError");
        match self {
            Self::UnknownAcquisitionProvider { provider } => {
                debug.field("kind", &"unknownAcquisitionProvider");
                debug.field("provider", provider);
            }
            Self::InvalidUpdateTarget { skill_id, .. } => {
                debug.field("kind", &"invalidUpdateTarget");
                debug.field("skill_id", skill_id);
            }
            Self::InstalledSourceRequiresUpdate => {
                debug.field("kind", &"installedSourceRequiresUpdate");
            }
            Self::InstalledSkillNotFound { installation_id } => {
                debug.field("kind", &"installedSkillNotFound");
                debug.field("installation_id", installation_id);
            }
            Self::InstalledSkillRead {
                installation_id, ..
            } => {
                debug.field("kind", &"installedSkillRead");
                debug.field("installation_id", installation_id);
            }
            Self::InstalledSourceRevisionConflict {
                installation_id,
                expected_revision,
                actual_revision,
            } => {
                debug.field("kind", &"installedSourceRevisionConflict");
                debug.field("installation_id", installation_id);
                debug.field("expected_revision", expected_revision);
                debug.field("actual_revision", actual_revision);
            }
            Self::InstalledSourceLegacy { installation_id } => {
                debug.field("kind", &"installedSourceLegacy");
                debug.field("installation_id", installation_id);
            }
            Self::InstalledSourceNotRefreshable { installation_id } => {
                debug.field("kind", &"installedSourceNotRefreshable");
                debug.field("installation_id", installation_id);
            }
            Self::UnknownRefreshProvider { provider } => {
                debug.field("kind", &"unknownRefreshProvider");
                debug.field("provider", provider);
            }
            Self::UnsupportedRefreshSchema {
                provider,
                schema_version,
            } => {
                debug.field("kind", &"unsupportedRefreshSchema");
                debug.field("provider", provider);
                debug.field("schema_version", schema_version);
            }
            Self::InvalidInstalledSourceProvenance { provider } => {
                debug.field("kind", &"invalidInstalledSourceProvenance");
                debug.field("provider", provider);
            }
            Self::PreparationConflict { preparation_id } => {
                debug.field("kind", &"preparationConflict");
                debug.field("preparation_id", preparation_id);
            }
            Self::PreparationNotFoundOrExpired { preparation_id } => {
                debug.field("kind", &"preparationNotFoundOrExpired");
                debug.field("preparation_id", preparation_id);
            }
            Self::PreparationCancelled { preparation_id } => {
                debug.field("kind", &"preparationCancelled");
                debug.field("preparation_id", preparation_id);
            }
            Self::PreparationAlreadyCommitted { preparation_id } => {
                debug.field("kind", &"preparationAlreadyCommitted");
                debug.field("preparation_id", preparation_id);
            }
            Self::PreparationBusy { preparation_id } => {
                debug.field("kind", &"preparationBusy");
                debug.field("preparation_id", preparation_id);
            }
            Self::PreviewMismatch {
                preparation_id,
                expected,
                actual,
            } => {
                debug.field("kind", &"previewMismatch");
                debug.field("preparation_id", preparation_id);
                debug.field("expected", expected);
                debug.field("actual", actual);
            }
            Self::PreparationCapacityExceeded { max_preparations } => {
                debug.field("kind", &"preparationCapacityExceeded");
                debug.field("max_preparations", max_preparations);
            }
            Self::PreparationMemoryCapacityExceeded { max_snapshot_bytes } => {
                debug.field("kind", &"preparationMemoryCapacityExceeded");
                debug.field("max_snapshot_bytes", max_snapshot_bytes);
            }
            Self::SourceResolutionNotFoundOrExpired { resolution_id } => {
                debug.field("kind", &"sourceResolutionNotFoundOrExpired");
                debug.field("resolution_id", resolution_id);
            }
            Self::SourceResolutionBusy { resolution_id } => {
                debug.field("kind", &"sourceResolutionBusy");
                debug.field("resolution_id", resolution_id);
            }
            Self::SourceResolutionConsumed { resolution_id } => {
                debug.field("kind", &"sourceResolutionConsumed");
                debug.field("resolution_id", resolution_id);
            }
            Self::SourceResolutionCancelled { resolution_id } => {
                debug.field("kind", &"sourceResolutionCancelled");
                debug.field("resolution_id", resolution_id);
            }
            Self::SourceCandidateNotFound {
                resolution_id,
                candidate_id,
            } => {
                debug.field("kind", &"sourceCandidateNotFound");
                debug.field("resolution_id", resolution_id);
                debug.field("candidate_id", candidate_id);
            }
            Self::WarningAcknowledgementRequired {
                preparation_id,
                missing,
            } => {
                debug.field("kind", &"warningAcknowledgementRequired");
                debug.field("preparation_id", preparation_id);
                debug.field("missing", missing);
            }
            Self::Acquisition {
                provider, source, ..
            } => {
                debug.field("kind", &"acquisition");
                debug.field("provider", provider);
                debug.field("source", source);
            }
            Self::Installation { preparation_id, .. } => {
                debug.field("kind", &"installation");
                debug.field("preparation_id", preparation_id);
            }
            Self::Internal { operation, .. } => {
                debug.field("kind", &"internal");
                debug.field("operation", operation);
            }
        }
        debug.finish()
    }
}

impl fmt::Display for SkillInstallationWorkflowError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownAcquisitionProvider { provider } => {
                write!(
                    formatter,
                    "No Skill acquisition adapter is registered for `{provider}`"
                )
            }
            Self::InvalidUpdateTarget { reason, .. } => reason.fmt(formatter),
            Self::InstalledSourceRequiresUpdate => {
                formatter.write_str("Installed-source acquisition is valid only for updates")
            }
            Self::InstalledSkillNotFound { .. } => {
                formatter.write_str("The installed Skill receipt was not found")
            }
            Self::InstalledSkillRead { .. } => {
                formatter.write_str("The installed Skill receipt could not be read")
            }
            Self::InstalledSourceRevisionConflict { .. } => {
                formatter.write_str("The installed Skill changed before source refresh could begin")
            }
            Self::InstalledSourceLegacy { .. } => {
                formatter.write_str("Legacy Skill installations do not contain refresh provenance")
            }
            Self::InstalledSourceNotRefreshable { .. } => {
                formatter.write_str("The installed Skill source is immutable or not refreshable")
            }
            Self::UnknownRefreshProvider { provider } => write!(
                formatter,
                "No Skill refresh adapter is registered for `{provider}`"
            ),
            Self::UnsupportedRefreshSchema {
                provider,
                schema_version,
            } => write!(
                formatter,
                "Skill refresh provider `{provider}` does not support schema {schema_version}"
            ),
            Self::InvalidInstalledSourceProvenance { provider } => write!(
                formatter,
                "Installed Skill provenance for `{provider}` is invalid or inconsistent"
            ),
            Self::PreparationConflict { .. } => {
                formatter.write_str("Preparation ID is already bound to a different request")
            }
            Self::PreparationNotFoundOrExpired { .. } => {
                formatter.write_str("Skill preparation was not found or has expired")
            }
            Self::PreparationCancelled { .. } => {
                formatter.write_str("Skill preparation was cancelled")
            }
            Self::PreparationAlreadyCommitted { .. } => {
                formatter.write_str("A committed Skill preparation cannot be cancelled")
            }
            Self::PreparationBusy { .. } => {
                formatter.write_str("Skill preparation is currently busy")
            }
            Self::PreviewMismatch { .. } => {
                formatter.write_str("Skill preview revision does not match the prepared preview")
            }
            Self::PreparationCapacityExceeded { max_preparations } => write!(
                formatter,
                "Skill preparation registry reached its {max_preparations}-entry limit",
            ),
            Self::PreparationMemoryCapacityExceeded { max_snapshot_bytes } => write!(
                formatter,
                "Skill preparation registry reached its {max_snapshot_bytes}-byte snapshot limit",
            ),
            Self::SourceResolutionNotFoundOrExpired { .. } => {
                formatter.write_str("Skill source resolution was not found or has expired")
            }
            Self::SourceResolutionBusy { .. } => {
                formatter.write_str("Skill source resolution is still in progress")
            }
            Self::SourceResolutionConsumed { .. } => {
                formatter.write_str("Skill source resolution was already consumed")
            }
            Self::SourceResolutionCancelled { .. } => {
                formatter.write_str("Skill source resolution was cancelled")
            }
            Self::SourceCandidateNotFound { .. } => {
                formatter.write_str("Resolved Skill candidate was not found")
            }
            Self::WarningAcknowledgementRequired { .. } => {
                formatter.write_str("Required Skill installation warnings were not acknowledged")
            }
            Self::Acquisition { provider, .. } => {
                write!(formatter, "Skill acquisition through `{provider}` failed")
            }
            Self::Installation { .. } => formatter.write_str("Skill installation commit failed"),
            Self::Internal { operation, .. } => {
                write!(
                    formatter,
                    "Internal failure while attempting to {operation}"
                )
            }
        }
    }
}

impl Error for SkillInstallationWorkflowError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Acquisition { source, .. } => Some(source.as_ref()),
            Self::Installation { source, .. } => Some(source.as_ref()),
            _ => None,
        }
    }
}
