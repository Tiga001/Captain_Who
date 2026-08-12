use serde::{Deserialize, Serialize};

/// Stable, safe recovery metadata for a rejected model-settings save.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StorageModelSettingsValidationErrorKindDto {
    ModelSettingsValidation,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StorageModelSettingsValidationErrorCodeDto {
    DuplicateModelId,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StorageModelSettingsValidationErrorData {
    pub kind: StorageModelSettingsValidationErrorKindDto,
    pub code: StorageModelSettingsValidationErrorCodeDto,
    pub model_id: String,
}

impl StorageModelSettingsValidationErrorData {
    pub fn duplicate_model_id(model_id: impl Into<String>) -> Self {
        Self {
            kind: StorageModelSettingsValidationErrorKindDto::ModelSettingsValidation,
            code: StorageModelSettingsValidationErrorCodeDto::DuplicateModelId,
            model_id: model_id.into(),
        }
    }
}
