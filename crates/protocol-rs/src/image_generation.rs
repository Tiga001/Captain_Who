use serde::{Deserialize, Serialize};
use std::fmt;

pub const IMAGE_GENERATION_CONFIGURATION_SCHEMA_VERSION: u32 = 1;
pub const IMAGE_GENERATION_CONFIGURATION_ERROR_CODE: i64 = -32020;

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
pub enum ImageGenerationAdapterIdDto {
    #[serde(rename = "smartmlSeedream")]
    SmartMlSeedream,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImageGenerationCapabilitiesDto {
    pub text_to_image: bool,
    pub image_to_image: bool,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
pub enum ImageGenerationSizePresetDto {
    #[serde(rename = "2K")]
    TwoK,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImageGenerationDefaultsDto {
    pub size_preset: ImageGenerationSizePresetDto,
    pub watermark: bool,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ImageGenerationCredentialStatusDto {
    Missing,
    Configured,
    Unavailable,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ImageGenerationReadinessDto {
    Disabled,
    MissingEndpoint,
    MissingModel,
    MissingCredential,
    CredentialUnavailable,
    ReadyUnverified,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImageGenerationConfigurationDto {
    pub adapter_id: ImageGenerationAdapterIdDto,
    pub endpoint_url: String,
    pub model_id: String,
    pub capabilities: ImageGenerationCapabilitiesDto,
    pub defaults: ImageGenerationDefaultsDto,
    pub credential_status: ImageGenerationCredentialStatusDto,
    pub enabled: bool,
    pub readiness: ImageGenerationReadinessDto,
    pub revision: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImageGenerationGetConfigurationResponse {
    pub schema_version: u32,
    pub configuration: ImageGenerationConfigurationDto,
}

#[derive(Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum ImageGenerationCredentialMutationDto {
    Keep,
    Replace { value: String },
    Clear,
}

impl fmt::Debug for ImageGenerationCredentialMutationDto {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Keep => formatter.write_str("Keep"),
            Self::Replace { .. } => formatter
                .debug_struct("Replace")
                .field("value", &"[REDACTED]")
                .finish(),
            Self::Clear => formatter.write_str("Clear"),
        }
    }
}

#[derive(Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImageGenerationUpdateConfigurationRequest {
    pub schema_version: u32,
    pub expected_revision: String,
    pub adapter_id: ImageGenerationAdapterIdDto,
    pub endpoint_url: String,
    pub model_id: String,
    pub capabilities: ImageGenerationCapabilitiesDto,
    pub defaults: ImageGenerationDefaultsDto,
    pub credential_mutation: ImageGenerationCredentialMutationDto,
}

impl fmt::Debug for ImageGenerationUpdateConfigurationRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ImageGenerationUpdateConfigurationRequest")
            .field("schema_version", &self.schema_version)
            .field("expected_revision", &self.expected_revision)
            .field("adapter_id", &self.adapter_id)
            .field("endpoint_url", &self.endpoint_url)
            .field("model_id", &self.model_id)
            .field("capabilities", &self.capabilities)
            .field("defaults", &self.defaults)
            .field("credential_mutation", &self.credential_mutation)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ImageGenerationConfigurationMutationOutcomeDto {
    Updated,
    AlreadyCurrent,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImageGenerationUpdateConfigurationResponse {
    pub schema_version: u32,
    pub outcome: ImageGenerationConfigurationMutationOutcomeDto,
    pub configuration: ImageGenerationConfigurationDto,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImageGenerationSetEnabledRequest {
    pub schema_version: u32,
    pub expected_revision: String,
    pub enabled: bool,
}

pub type ImageGenerationSetEnabledResponse = ImageGenerationUpdateConfigurationResponse;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImageGenerationStatusDto {
    pub schema_version: u32,
    pub adapter_id: ImageGenerationAdapterIdDto,
    pub configuration_revision: String,
    pub enabled: bool,
    pub readiness: ImageGenerationReadinessDto,
    pub credential_status: ImageGenerationCredentialStatusDto,
    pub capabilities: ImageGenerationCapabilitiesDto,
    pub defaults: ImageGenerationDefaultsDto,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ImageGenerationConfigurationOperationDto {
    GetConfiguration,
    UpdateConfiguration,
    SetEnabled,
    GetStatus,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ImageGenerationConfigurationErrorCodeDto {
    InvalidRequest,
    RevisionConflict,
    UnsupportedAdapter,
    MissingEndpoint,
    InvalidEndpoint,
    InsecureEndpoint,
    MissingModel,
    InvalidModelId,
    MissingCredential,
    InvalidCredential,
    CredentialReplacementRequired,
    StorageUnavailable,
    CredentialStoreUnavailable,
    CommitIndeterminate,
    Unavailable,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ImageGenerationConfigurationRecoveryDto {
    FixConfiguration,
    RefreshConfiguration,
    ReenterCredential,
    Retry,
    ContactSupport,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ImageGenerationConfigurationErrorTypeDto {
    ImageGenerationConfiguration,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImageGenerationConfigurationErrorData {
    #[serde(rename = "type")]
    pub error_type: ImageGenerationConfigurationErrorTypeDto,
    pub operation: ImageGenerationConfigurationOperationDto,
    pub code: ImageGenerationConfigurationErrorCodeDto,
    pub recovery: ImageGenerationConfigurationRecoveryDto,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_revision: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub configuration_may_have_changed: Option<bool>,
}
