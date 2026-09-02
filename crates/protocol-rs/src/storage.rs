use serde::{Deserialize, Serialize};

pub const STORAGE_MODEL_DISPLAY_NAME_MAX_BYTES: usize = 512;

/// Stable, safe recovery metadata for a rejected model-settings save.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StorageModelSettingsValidationErrorKindDto {
    ModelSettingsValidation,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StorageModelSettingsValidationErrorCodeDto {
    DuplicateDisplayName,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StorageModelSettingsValidationErrorData {
    pub kind: StorageModelSettingsValidationErrorKindDto,
    pub code: StorageModelSettingsValidationErrorCodeDto,
    pub display_name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StorageModelSettingsValidationErrorDataWire {
    kind: StorageModelSettingsValidationErrorKindDto,
    code: StorageModelSettingsValidationErrorCodeDto,
    display_name: String,
}

impl<'de> Deserialize<'de> for StorageModelSettingsValidationErrorData {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = StorageModelSettingsValidationErrorDataWire::deserialize(deserializer)?;
        if wire.display_name.is_empty()
            || wire.display_name.len() > STORAGE_MODEL_DISPLAY_NAME_MAX_BYTES
            || wire.display_name.trim() != wire.display_name
        {
            return Err(serde::de::Error::custom(
                "displayName must be a trimmed non-empty string no longer than 512 bytes",
            ));
        }
        Ok(Self {
            kind: wire.kind,
            code: wire.code,
            display_name: wire.display_name,
        })
    }
}

impl StorageModelSettingsValidationErrorData {
    pub fn duplicate_display_name(display_name: impl Into<String>) -> Self {
        Self {
            kind: StorageModelSettingsValidationErrorKindDto::ModelSettingsValidation,
            code: StorageModelSettingsValidationErrorCodeDto::DuplicateDisplayName,
            display_name: display_name.into(),
        }
    }
}
