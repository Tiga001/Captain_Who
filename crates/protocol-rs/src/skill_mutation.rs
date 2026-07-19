use crate::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SkillMutationResponse {
    schema_version: u32,
    installation_id: String,
    skill_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    revision: Option<String>,
    outcome: SkillMutationOutcomeDto,
}

impl SkillMutationResponse {
    pub fn install(
        installation_id: String,
        skill_id: String,
        revision: String,
        outcome: SkillInstallMutationOutcomeDto,
    ) -> Self {
        Self {
            schema_version: SKILL_MUTATION_SCHEMA_VERSION,
            installation_id,
            skill_id,
            revision: Some(revision),
            outcome: outcome.into(),
        }
    }

    pub fn update(
        installation_id: String,
        skill_id: String,
        revision: String,
        outcome: SkillUpdateMutationOutcomeDto,
    ) -> Self {
        Self {
            schema_version: SKILL_MUTATION_SCHEMA_VERSION,
            installation_id,
            skill_id,
            revision: Some(revision),
            outcome: outcome.into(),
        }
    }

    pub fn removal(
        installation_id: String,
        skill_id: String,
        outcome: SkillRemovalMutationOutcomeDto,
    ) -> Self {
        Self {
            schema_version: SKILL_MUTATION_SCHEMA_VERSION,
            installation_id,
            skill_id,
            revision: None,
            outcome: outcome.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum SkillMutationOutcomeDto {
    Installed,
    AlreadyInstalled,
    Updated,
    AlreadyCurrent,
    Uninstalled,
    AlreadyAbsent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillInstallMutationOutcomeDto {
    Installed,
    AlreadyInstalled,
}

impl From<SkillInstallMutationOutcomeDto> for SkillMutationOutcomeDto {
    fn from(value: SkillInstallMutationOutcomeDto) -> Self {
        match value {
            SkillInstallMutationOutcomeDto::Installed => Self::Installed,
            SkillInstallMutationOutcomeDto::AlreadyInstalled => Self::AlreadyInstalled,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillUpdateMutationOutcomeDto {
    Updated,
    AlreadyCurrent,
}

impl From<SkillUpdateMutationOutcomeDto> for SkillMutationOutcomeDto {
    fn from(value: SkillUpdateMutationOutcomeDto) -> Self {
        match value {
            SkillUpdateMutationOutcomeDto::Updated => Self::Updated,
            SkillUpdateMutationOutcomeDto::AlreadyCurrent => Self::AlreadyCurrent,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillRemovalMutationOutcomeDto {
    Uninstalled,
    AlreadyAbsent,
}

impl From<SkillRemovalMutationOutcomeDto> for SkillMutationOutcomeDto {
    fn from(value: SkillRemovalMutationOutcomeDto) -> Self {
        match value {
            SkillRemovalMutationOutcomeDto::Uninstalled => Self::Uninstalled,
            SkillRemovalMutationOutcomeDto::AlreadyAbsent => Self::AlreadyAbsent,
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SkillInstallationOperationDto {
    Install,
    Update,
    Uninstall,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SkillInstallationErrorCodeDto {
    PreparationFailed,
    InvalidSkill,
    InvalidStore,
    CapacityExceeded,
    InstallationExists,
    InstallationRetired,
    InstallationNotFound,
    RevisionConflict,
    StoreCorrupt,
    Io,
    Unavailable,
    CommitIndeterminate,
    Cancelled,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SkillInstallationRecoveryDto {
    FixLocalSource,
    RetrySameRequest,
    NewInstallationIdentity,
    RefreshCatalog,
    FreeCapacity,
    ContactSupport,
    RepairStore,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SkillInstallationCapacityDto {
    Installations,
    InstallationDirectory,
    RetiredInstallationIds,
    Packages,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SkillInstallationErrorData {
    #[serde(rename = "type")]
    pub error_type: SkillInstallationErrorTypeDto,
    pub operation: SkillInstallationOperationDto,
    pub code: SkillInstallationErrorCodeDto,
    pub recovery: SkillInstallationRecoveryDto,
    pub message: String,
    pub commit_may_have_succeeded: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub installation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skill_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostic_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intended_revision: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_revision: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actual_revision: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capacity: Option<SkillInstallationCapacityDto>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SkillInstallationErrorTypeDto {
    SkillInstallation,
}
