use serde::{Deserialize, Serialize};

pub const OFFICE_ENGINE_STATUS_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum OfficeDocumentKindDto {
    Document,
    Spreadsheet,
    Presentation,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum OfficeOperationDto {
    Help,
    Create,
    View,
    Get,
    Query,
    Validate,
    Set,
    Add,
    Remove,
    Move,
    Swap,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum OfficeEngineAvailabilityDto {
    Available,
    Unavailable,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum OfficeEngineSourceDto {
    Configured,
    PackagedComponent,
    DevelopmentPath,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OfficeEngineCapabilitiesDto {
    pub provider_id: String,
    pub document_kinds: Vec<OfficeDocumentKindDto>,
    pub operations: Vec<OfficeOperationDto>,
    pub supports_rendering: bool,
    pub supports_validation: bool,
    pub supports_structured_output: bool,
}

/// Authoritative status of the single Office engine shared by the Agent runtime.
///
/// Optional probe metadata is omitted from JSON rather than serialized as null.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OfficeEngineStatusDto {
    pub schema_version: u32,
    pub provider_id: String,
    pub availability: OfficeEngineAvailabilityDto,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<OfficeEngineSourceDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine_revision: Option<String>,
    pub capabilities: OfficeEngineCapabilitiesDto,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}
