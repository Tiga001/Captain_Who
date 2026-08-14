use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SkillSelectionDto {
    pub id: String,
    pub revision: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillSourceDto {
    pub kind: SkillSourceKindDto,
    pub id: String,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SkillSourceKindDto {
    Workspace,
    Bundled,
    Installed,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SkillTrustDto {
    Untrusted,
    Application,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SkillDescriptorDto {
    pub id: String,
    pub name: String,
    pub description: String,
    pub source: SkillSourceDto,
    pub trust: SkillTrustDto,
    pub activation_scope: String,
    pub revision: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillDiagnosticDto {
    pub code: String,
    pub severity: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skill_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SkillsListResponse {
    pub schema_version: u32,
    pub catalog_revision: String,
    pub skills: Vec<SkillDescriptorDto>,
    pub diagnostics: Vec<SkillDiagnosticDto>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ActivatedSkillSummaryDto {
    pub id: String,
    pub name: String,
    pub revision: String,
    pub source: SkillSourceDto,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SkillActivationErrorData {
    #[serde(rename = "type")]
    pub error_type: &'static str,
    pub code: SkillActivationErrorCodeDto,
    pub recovery: SkillActivationRecoveryDto,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skill_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_revision: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actual_revision: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SkillActivationErrorCodeDto {
    InvalidSelection,
    DuplicateSelection,
    TooManySkills,
    ActivationTooLarge,
    NotFound,
    Stale,
    InvalidSkill,
    Disabled,
    SourceUnavailable,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SkillActivationRecoveryDto {
    RetrySameSelection,
    RefreshCatalog,
    RejectSelection,
}
