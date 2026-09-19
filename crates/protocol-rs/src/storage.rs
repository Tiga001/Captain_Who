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
    InvalidContextCapacityConfiguration,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StorageModelSettingsValidationErrorData {
    pub kind: StorageModelSettingsValidationErrorKindDto,
    pub code: StorageModelSettingsValidationErrorCodeDto,
    pub display_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reserved_output_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub safety_margin_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minimum_context_window_tokens: Option<u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StorageModelSettingsValidationErrorDataWire {
    kind: StorageModelSettingsValidationErrorKindDto,
    code: StorageModelSettingsValidationErrorCodeDto,
    display_name: String,
    model_id: Option<String>,
    context_window_tokens: Option<u32>,
    reserved_output_tokens: Option<u32>,
    safety_margin_tokens: Option<u64>,
    minimum_context_window_tokens: Option<u64>,
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
        let valid_capacity_fields = match wire.code {
            StorageModelSettingsValidationErrorCodeDto::DuplicateDisplayName => {
                wire.model_id.is_none()
                    && wire.context_window_tokens.is_none()
                    && wire.reserved_output_tokens.is_none()
                    && wire.safety_margin_tokens.is_none()
                    && wire.minimum_context_window_tokens.is_none()
            }
            StorageModelSettingsValidationErrorCodeDto::InvalidContextCapacityConfiguration => {
                wire.model_id
                    .as_deref()
                    .is_some_and(|id| !id.is_empty() && id.trim() == id)
                    && wire.context_window_tokens.is_some_and(|value| value > 0)
                    && wire.reserved_output_tokens.is_some_and(|value| value > 0)
                    && wire.safety_margin_tokens.is_some_and(|value| value > 0)
                    && wire
                        .minimum_context_window_tokens
                        .is_some_and(|value| value > 0)
            }
        };
        if !valid_capacity_fields {
            return Err(serde::de::Error::custom(
                "invalid model settings validation metadata",
            ));
        }
        Ok(Self {
            kind: wire.kind,
            code: wire.code,
            display_name: wire.display_name,
            model_id: wire.model_id,
            context_window_tokens: wire.context_window_tokens,
            reserved_output_tokens: wire.reserved_output_tokens,
            safety_margin_tokens: wire.safety_margin_tokens,
            minimum_context_window_tokens: wire.minimum_context_window_tokens,
        })
    }
}

impl StorageModelSettingsValidationErrorData {
    pub fn duplicate_display_name(display_name: impl Into<String>) -> Self {
        Self {
            kind: StorageModelSettingsValidationErrorKindDto::ModelSettingsValidation,
            code: StorageModelSettingsValidationErrorCodeDto::DuplicateDisplayName,
            display_name: display_name.into(),
            model_id: None,
            context_window_tokens: None,
            reserved_output_tokens: None,
            safety_margin_tokens: None,
            minimum_context_window_tokens: None,
        }
    }

    pub fn invalid_context_capacity(
        model_id: String,
        display_name: String,
        context_window_tokens: u32,
        reserved_output_tokens: u32,
        safety_margin_tokens: u64,
        minimum_context_window_tokens: u64,
    ) -> Self {
        Self {
            kind: StorageModelSettingsValidationErrorKindDto::ModelSettingsValidation,
            code: StorageModelSettingsValidationErrorCodeDto::InvalidContextCapacityConfiguration,
            model_id: Some(model_id),
            display_name,
            context_window_tokens: Some(context_window_tokens),
            reserved_output_tokens: Some(reserved_output_tokens),
            safety_margin_tokens: Some(safety_margin_tokens),
            minimum_context_window_tokens: Some(minimum_context_window_tokens),
        }
    }
}
