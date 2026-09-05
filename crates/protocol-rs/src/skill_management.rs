use crate::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillsListManagementRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SkillManagementOperationDto {
    List,
    SetEnabled,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SkillManagementErrorCodeDto {
    NotFound,
    NotManageable,
    StateConflict,
    ConfigurationRequired,
    StorageUnavailable,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SkillManagementRecoveryDto {
    RefreshManagement,
    ConfigureImageGeneration,
    Retry,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillManagementErrorData {
    #[serde(rename = "type")]
    pub error_type: SkillManagementErrorTypeDto,
    pub operation: SkillManagementOperationDto,
    pub code: SkillManagementErrorCodeDto,
    pub recovery: SkillManagementRecoveryDto,
    pub message: String,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SkillManagementErrorTypeDto {
    SkillManagement,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillManagementActionsDto {
    pub can_set_enabled: bool,
    pub can_update: bool,
    pub can_uninstall: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillManagementEntryDto {
    pub id: String,
    pub name: String,
    pub description: String,
    pub source: SkillSourceDto,
    pub package_revision: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub installation_revision: Option<String>,
    pub state_revision: String,
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enablement_block: Option<SkillEnablementBlockDto>,
    pub actions: SkillManagementActionsDto,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acquisition: Option<SkillPreviewSourceDto>,
    pub compatibility: SkillCompatibilityReportDto,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SkillEnablementBlockDto {
    ImageGenerationConfigurationRequired,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillsListManagementResponse {
    pub schema_version: u32,
    pub management_revision: String,
    pub skills: Vec<SkillManagementEntryDto>,
    pub diagnostics: Vec<SkillDiagnosticDto>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillsSetEnabledRequest {
    pub skill_id: String,
    pub expected_state_revision: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SkillSetEnabledOutcomeDto {
    Updated,
    AlreadyCurrent,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillsSetEnabledResponse {
    pub schema_version: u32,
    pub management_revision: String,
    pub skill_id: String,
    pub state_revision: String,
    pub enabled: bool,
    pub outcome: SkillSetEnabledOutcomeDto,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SkillsChangedReasonDto {
    Installed,
    Updated,
    Uninstalled,
    EnablementChanged,
    CatalogChanged,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillsChangedNotification {
    pub schema_version: u32,
    pub management_revision: String,
    pub reason: SkillsChangedReasonDto,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill_id: Option<String>,
}
