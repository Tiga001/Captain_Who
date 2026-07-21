//! Explicit boundary between the Skill domain model, runtime snapshots, and JSON-RPC DTOs.

use std::fmt::Write;
use std::path::Path;

use mycopilot_core::skills::{
    InstalledGitHubTrackingReference, InstalledSkillRecord, InstalledSkillSourcePresentation,
    ManagedSkillInstallerError, ManagedSkillInstallerErrorCode, ManagedSkillStoreCapacity,
    SkillActivationError, SkillActivationScope, SkillCatalog, SkillDescriptor, SkillDiagnosticCode,
    SkillDiagnosticSeverity, SkillErrorCode, SkillInstallationMutation, SkillInstallationOperation,
    SkillInstallationOutcome, SkillInstallationService, SkillInstallationServiceError,
    SkillInstallationWorkflow, SkillProvenance, SkillRecovery, SkillResourceSession,
    SkillSelection, SkillSourceKind, SkillTrust, SkillsService,
};
use mycopilot_core::storage::service::StorageService;
use mycopilot_core::storage::skill_enablement_repository::SkillEnablementCompareAndSetOutcome;
use mycopilot_core::storage::skill_enablement_repository::SkillEnablementState;
use mycopilot_core::{
    AgentActivatedSkill, AgentActivatedSkillResources, AgentError, AgentResolvedSkillActivation,
    AgentSkillActivation, AgentSkillActivationResolver,
};
use mycopilot_protocol_rs::{
    ActivatedSkillSummaryDto, SkillActivationErrorCodeDto, SkillActivationErrorData,
    SkillActivationRecoveryDto, SkillCompatibilityReportDto, SkillCompatibilityStatusDto,
    SkillDescriptorDto, SkillDiagnosticDto, SkillGithubReferenceDto,
    SkillInstallMutationOutcomeDto, SkillInstallationCapacityDto, SkillInstallationErrorCodeDto,
    SkillInstallationErrorData, SkillInstallationErrorTypeDto, SkillInstallationOperationDto,
    SkillInstallationRecoveryDto, SkillManagementActionsDto, SkillManagementEntryDto,
    SkillManagementErrorCodeDto, SkillManagementErrorData, SkillManagementErrorTypeDto,
    SkillManagementOperationDto, SkillManagementRecoveryDto, SkillMutationResponse,
    SkillPreviewSourceDto, SkillRemovalMutationOutcomeDto, SkillSelectionDto,
    SkillSetEnabledOutcomeDto, SkillSourceDto, SkillSourceKindDto, SkillTrustDto,
    SkillUpdateMutationOutcomeDto, SkillsListManagementResponse, SkillsListResponse,
    SkillsSetEnabledRequest, SkillsSetEnabledResponse, SKILL_CATALOG_SCHEMA_VERSION,
    SKILL_MANAGEMENT_SCHEMA_VERSION,
};
use sha2::{Digest, Sha256};

mod activation;
mod catalog;
mod discovery;
mod installation;
mod management;

pub(crate) use activation::*;
pub(crate) use catalog::*;
pub(crate) use discovery::*;
pub(crate) use installation::*;
pub(crate) use management::*;

#[derive(Debug, Default)]
pub(crate) struct PreparedSkillActivation {
    pub(crate) runtime: Option<AgentSkillActivation>,
    pub(crate) resources: Option<std::sync::Arc<SkillResourceSession>>,
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

#[derive(Debug)]
pub(crate) struct SkillManagementFailure {
    data: Box<SkillManagementErrorData>,
}

impl SkillManagementFailure {
    fn new(
        operation: SkillManagementOperationDto,
        code: SkillManagementErrorCodeDto,
        recovery: SkillManagementRecoveryDto,
        message: impl Into<String>,
    ) -> Self {
        Self {
            data: Box::new(SkillManagementErrorData {
                error_type: SkillManagementErrorTypeDto::SkillManagement,
                operation,
                code,
                recovery,
                message: message.into(),
            }),
        }
    }

    pub(crate) fn list_unavailable() -> Self {
        Self::new(
            SkillManagementOperationDto::List,
            SkillManagementErrorCodeDto::StorageUnavailable,
            SkillManagementRecoveryDto::Retry,
            "The Skill management inventory is temporarily unavailable.",
        )
    }

    pub(crate) fn set_enabled_unavailable() -> Self {
        Self::new(
            SkillManagementOperationDto::SetEnabled,
            SkillManagementErrorCodeDto::StorageUnavailable,
            SkillManagementRecoveryDto::Retry,
            "The Skill management inventory is temporarily unavailable.",
        )
    }

    pub(crate) fn into_data(self) -> Box<SkillManagementErrorData> {
        self.data
    }
}

impl std::fmt::Display for SkillManagementFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.data.message)
    }
}

impl std::error::Error for SkillManagementFailure {}

impl SkillInstallationFailure {
    pub(crate) fn commit_may_have_succeeded(&self) -> bool {
        self.data.commit_may_have_succeeded
    }

    pub(crate) fn skill_id(&self) -> Option<&str> {
        self.data.skill_id.as_deref()
    }

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

impl SkillActivationFailure {
    pub(crate) fn into_data(self) -> Box<SkillActivationErrorData> {
        self.data
    }

    fn invalid_selection(skill_id: Option<String>, message: impl Into<String>) -> Self {
        let message = message.into();
        Self {
            data: Box::new(SkillActivationErrorData {
                error_type: "skillActivation",
                code: SkillActivationErrorCodeDto::InvalidSelection,
                recovery: SkillActivationRecoveryDto::RejectSelection,
                message,
                skill_id,
                expected_revision: None,
                actual_revision: None,
            }),
        }
    }

    fn disabled(skill_id: String) -> Self {
        Self {
            data: Box::new(SkillActivationErrorData {
                error_type: "skillActivation",
                code: SkillActivationErrorCodeDto::Disabled,
                recovery: SkillActivationRecoveryDto::RejectSelection,
                message: "The selected Skill is disabled. Enable it before using it.".to_string(),
                skill_id: Some(skill_id),
                expected_revision: None,
                actual_revision: None,
            }),
        }
    }

    fn enablement_unavailable(skill_id: Option<String>) -> Self {
        Self {
            data: Box::new(SkillActivationErrorData {
                error_type: "skillActivation",
                code: SkillActivationErrorCodeDto::SourceUnavailable,
                recovery: SkillActivationRecoveryDto::RetrySameSelection,
                message: "Skill enablement could not be verified. Retry the same selection."
                    .to_string(),
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

#[cfg(test)]
mod tests;
