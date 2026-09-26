use super::deserialize_required_nullable;
use crate::provider_profile::{
    ProviderProfileConfig, ProviderProfileValidationError, ProviderProtocolDialect,
    ProviderVendorId, ProviderVendorPublicSettings,
};
use crate::AgentModelUnavailableReason;
use reqwest::Url;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::net::IpAddr;
use unicode_normalization::UnicodeNormalization;

/// Backend-authoritative context capacity used when a model configuration omits an override.
///
/// The renderer exposes the same default while editing model settings. Backend callers must use
/// [`ModelConfigRecord::effective_context_window_tokens`] instead of interpreting `None` as an
/// unconfigured runtime.
pub const DEFAULT_MODEL_CONTEXT_WINDOW_TOKENS: u32 = 128_000;
/// Maximum UTF-8 byte length accepted for a provider endpoint URL.
pub const MODEL_PROVIDER_API_URL_MAX_BYTES: usize = 4_096;
/// Matches the strict Renderer parser for typed model-settings validation errors. Keeping the
/// source value bounded guarantees a duplicate-display-name rejection remains actionable.
pub const MODEL_DISPLAY_NAME_MAX_BYTES: usize = 512;

/// Renderer-safe status for a credential owned by the Host.
///
/// The status is intentionally lossy: neither the native-store reference nor any part of the
/// secret crosses the Host boundary.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CredentialStatus {
    Missing,
    Configured,
    Unavailable,
}

/// Explicit write-only credential operation accepted from the Renderer.
#[cfg_attr(test, derive(Clone))]
#[derive(Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum CredentialMutation {
    Keep,
    Replace { value: String },
    Clear,
}

impl std::fmt::Debug for CredentialMutation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Keep => formatter.write_str("Keep"),
            Self::Replace { .. } => formatter.write_str("Replace([REDACTED])"),
            Self::Clear => formatter.write_str("Clear"),
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct ModelConnectionConfig {
    pub api_url: String,
    pub api_token: String,
}

impl std::fmt::Debug for ModelConnectionConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ModelConnectionConfig([REDACTED])")
    }
}

#[cfg_attr(test, derive(Serialize))]
#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelConfigRecord {
    /// Immutable Host-owned identity of this configured model.
    pub id: String,
    /// Opaque identifier sent verbatim as the provider API's `model` value.
    pub provider_model_id: String,
    /// User-facing identity. Its Host-normalized form is globally unique and it never
    /// participates in provider routing.
    pub display_name: String,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub api_url_override: Option<String>,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub api_token_override: Option<String>,
    pub supports_image: bool,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub context_window_tokens: Option<u32>,
    /// Explicit provider wire profile. Every stored model carries one versioned configuration.
    pub provider_profile_config: ProviderProfileConfig,
    pub input_price: String,
    /// Empty means cached input inherits `input_price` when a run freezes its billing snapshot.
    pub cached_input_price: String,
    pub output_price: String,
    pub enabled: bool,
}

/// Host-authoritative execution projection of one configured model.
///
/// The model projection module computes this once per settings snapshot; selectors, settings
/// editors, automation targets, and templates must share it instead of re-deriving availability
/// from raw credential or connection fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ModelExecutionStatus {
    Available,
    Unavailable { reason: AgentModelUnavailableReason },
}

impl ModelExecutionStatus {
    /// Whether this model can be offered to and executed by the Host right now.
    pub fn is_available(&self) -> bool {
        matches!(self, Self::Available)
    }

    /// The reason this model cannot execute, when it is unavailable.
    pub fn unavailable_reason(&self) -> Option<&AgentModelUnavailableReason> {
        match self {
            Self::Available => None,
            Self::Unavailable { reason } => Some(reason),
        }
    }
}

/// Credential-free model configuration returned to the Renderer settings editor.
#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelConfigEditorRecord {
    pub id: String,
    pub provider_model_id: String,
    pub display_name: String,
    pub api_url_override: Option<String>,
    pub api_token_override_status: CredentialStatus,
    pub supports_image: bool,
    pub context_window_tokens: Option<u32>,
    pub provider_profile_config: ProviderProfileConfig,
    pub input_price: String,
    pub cached_input_price: String,
    pub output_price: String,
    pub enabled: bool,
    pub execution: ModelExecutionStatus,
}

impl std::fmt::Debug for ModelConfigRecord {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ModelConfigRecord([REDACTED])")
    }
}

#[cfg_attr(test, derive(Serialize))]
#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelSettingsRecord {
    pub api_url: String,
    pub api_token: String,
    pub search_mode: String,
    pub tavily_api_key: String,
    pub models: Vec<ModelConfigRecord>,
}

/// Credential-free model settings returned by the Host to Renderer.
#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelSettingsEditorRecord {
    /// Opaque compare-and-swap identity of this exact settings snapshot.
    pub configuration_revision: String,
    pub api_url: String,
    pub api_token_status: CredentialStatus,
    pub search_mode: String,
    pub tavily_api_key_status: CredentialStatus,
    pub models: Vec<ModelConfigEditorRecord>,
}

/// SQLite representation of model settings. Every credential field is an opaque native-store
/// reference; this type is deliberately not serializable.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct StoredModelConfigRecord {
    pub id: String,
    pub provider_model_id: String,
    pub display_name: String,
    pub api_url_override: Option<String>,
    pub api_token_override_ref: Option<String>,
    pub supports_image: bool,
    pub context_window_tokens: Option<u32>,
    pub provider_profile_config: ProviderProfileConfig,
    pub input_price: String,
    pub cached_input_price: String,
    pub output_price: String,
    pub enabled: bool,
}

impl std::fmt::Debug for StoredModelConfigRecord {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("StoredModelConfigRecord([REDACTED])")
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct StoredModelSettingsRecord {
    pub api_url: String,
    pub api_token_ref: Option<String>,
    pub search_mode: String,
    pub tavily_api_key_ref: Option<String>,
    pub models: Vec<StoredModelConfigRecord>,
}

impl std::fmt::Debug for StoredModelSettingsRecord {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("StoredModelSettingsRecord([REDACTED])")
    }
}

#[derive(Clone)]
pub(crate) struct StoredModelSettingsSnapshot {
    pub settings: StoredModelSettingsRecord,
    pub configuration_revision: String,
    pub provider_connection_revisions: BTreeMap<String, String>,
    pub provider_protocol_revisions: BTreeMap<String, String>,
    pub search_connection_revision: String,
}

impl std::fmt::Debug for StoredModelSettingsSnapshot {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("StoredModelSettingsSnapshot([REDACTED])")
    }
}

impl std::fmt::Debug for ModelSettingsRecord {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ModelSettingsRecord([REDACTED])")
    }
}

/// Explicit Host-authoritative mutation of a model's Provider Profile.
///
/// Registered selections carry no version or Runtime policy; the Host resolves both from its
/// code-owned Registry.
#[derive(Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ProviderProfileUpdate {
    Unchanged,
    SelectGeneric,
    /// Family-aware selection. Host resolves the exact Profile/version from vendor, model and
    /// dialect; clients cannot submit either internal identity.
    SelectVendor {
        vendor_id: ProviderVendorId,
        settings: ProviderVendorPublicSettings,
    },
}

/// Wire-only model mutation DTO. It is converted into [`ModelConfigRecord`] before persistence,
/// so update intent can never become a second stored fact.
#[cfg_attr(test, derive(Clone))]
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelConfigSaveRequest {
    /// Existing immutable configuration identity, or null when creating a model. New identities
    /// are generated by the Host and cannot be supplied by a client.
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub id: Option<String>,
    pub provider_model_id: String,
    pub display_name: String,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub api_url_override: Option<String>,
    pub api_token_override_mutation: CredentialMutation,
    pub supports_image: bool,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub context_window_tokens: Option<u32>,
    pub provider_profile_update: ProviderProfileUpdate,
    pub input_price: String,
    pub cached_input_price: String,
    pub output_price: String,
    pub enabled: bool,
}

impl ModelConfigSaveRequest {
    pub fn into_record(
        self,
        id: String,
        provider_profile_config: ProviderProfileConfig,
        api_token_override: Option<String>,
    ) -> ModelConfigRecord {
        ModelConfigRecord {
            id,
            provider_model_id: self.provider_model_id,
            display_name: self.display_name,
            api_url_override: self.api_url_override,
            api_token_override,
            supports_image: self.supports_image,
            context_window_tokens: self.context_window_tokens,
            provider_profile_config,
            input_price: self.input_price,
            cached_input_price: self.cached_input_price,
            output_price: self.output_price,
            enabled: self.enabled,
        }
    }
}

/// Canonical identity used exclusively for display-name uniqueness.
///
/// The visible spelling remains independent. Keeping this implementation in Rust gives storage,
/// validation and migrations one authoritative Unicode contract rather than relying on SQLite's
/// ASCII-only `lower()` implementation.
pub fn normalize_model_display_name(value: &str) -> String {
    value
        .nfkc()
        .collect::<String>()
        .to_lowercase()
        .trim()
        .to_string()
}

#[cfg_attr(test, derive(Clone))]
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelSettingsSaveRequest {
    /// Opaque compare-and-swap identity returned by the settings editor. Current clients always
    /// send this field; `None` is valid only for the first save into an empty database, while an
    /// explicitly supplied stale revision is always rejected by the Host.
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub expected_revision: Option<String>,
    /// Explicit existing model edited by the user, even when its fields did not change. Other
    /// untouched legacy models in this full-catalog save do not prevent unrelated edits.
    #[serde(default)]
    pub validate_context_capacity_model_id: Option<String>,
    pub api_url: String,
    pub api_token_mutation: CredentialMutation,
    pub search_mode: String,
    pub tavily_api_key_mutation: CredentialMutation,
    pub models: Vec<ModelConfigSaveRequest>,
}

impl std::fmt::Debug for ModelSettingsSaveRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ModelSettingsSaveRequest([REDACTED])")
    }
}

/// Failure from the authoritative model-settings mutation boundary.
///
/// Only variants carrying an explicit recovery contract may cross the Host boundary. `Other`
/// retains the internal diagnostic for local callers while Core Server must redact it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelSettingsSaveError {
    DuplicateDisplayName {
        display_name: String,
    },
    InvalidContextCapacity {
        model_id: String,
        display_name: String,
        context_window_tokens: u32,
        reserved_output_tokens: u32,
        safety_margin_tokens: u64,
        minimum_context_window_tokens: u64,
    },
    Other(String),
}

impl std::fmt::Display for ModelSettingsSaveError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DuplicateDisplayName { display_name } => {
                write!(formatter, "模型显示名称重复：{display_name}")
            }
            Self::InvalidContextCapacity { display_name, .. } => {
                write!(formatter, "模型 {display_name} 的上下文容量配置无效。")
            }
            Self::Other(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for ModelSettingsSaveError {}

impl From<String> for ModelSettingsSaveError {
    fn from(message: String) -> Self {
        Self::Other(message)
    }
}

/// Host-only runtime snapshot of model settings and the opaque identity of the exact saved
/// revision. A new random revision is generated for every successful settings save, including
/// saves whose visible values happen to be identical. Renderer receives the same opaque identity
/// only through [`ModelSettingsEditorRecord`] for compare-and-swap; runtime-only connection and
/// protocol revisions remain private.
#[derive(Clone)]
pub struct ModelSettingsSnapshot {
    pub settings: ModelSettingsRecord,
    pub configuration_revision: String,
    /// Stable opaque identity of each model's effective endpoint/token pair. Revisions are
    /// Host-only and rotate independently, so editing another model or non-connection metadata
    /// cannot invalidate an already frozen run.
    pub provider_connection_revisions: BTreeMap<String, String>,
    /// Stable opaque identity of each model's complete provider wire protocol: endpoint/token,
    /// dialect, wire model id, Profile/version and reasoning policy.
    pub provider_protocol_revisions: BTreeMap<String, String>,
    /// Stable opaque identity of the effective search mode/credential pair.
    pub search_connection_revision: String,
}

impl std::fmt::Debug for ModelSettingsSnapshot {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ModelSettingsSnapshot([REDACTED])")
    }
}

/// Credential-free persisted state for one image-generation provider profile.
///
/// The secret itself lives in the platform credential store. `credential_ref` is an opaque
/// lookup identity and is safe to persist, but it must never be interpreted as a credential.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ImageGenerationProfileRecord {
    pub id: String,
    pub schema_version: u32,
    pub adapter_id: String,
    pub endpoint_url: String,
    pub model_id: String,
    pub credential_ref: Option<String>,
    pub enabled: bool,
    pub text_to_image: bool,
    pub image_to_image: bool,
    pub default_size_preset: String,
    pub default_watermark: bool,
    pub generation: u64,
    pub created_at: i64,
    pub updated_at: i64,
}

pub(crate) fn validated_connection(
    model_id: &str,
    source_label: &str,
    api_url: &str,
    api_token: &str,
) -> Result<ModelConnectionConfig, String> {
    if api_url.len() > MODEL_PROVIDER_API_URL_MAX_BYTES || api_url.chars().any(char::is_control) {
        return Err(format!(
            "模型 {model_id} 的{source_label} URL 无效或超过 {MODEL_PROVIDER_API_URL_MAX_BYTES} 字节。"
        ));
    }
    let api_url = api_url.trim();
    let api_token = api_token.trim();
    let parsed_url =
        Url::parse(api_url).map_err(|_| format!("模型 {model_id} 的{source_label} URL 无效。"))?;
    if parsed_url.username() != ""
        || parsed_url.password().is_some()
        || parsed_url.query().is_some()
        || parsed_url.fragment().is_some()
    {
        return Err(format!(
            "模型 {model_id} 的{source_label} URL 不得包含用户信息、查询参数或片段。"
        ));
    }
    let secure = parsed_url.scheme() == "https";
    let controlled_loopback = cfg!(debug_assertions)
        && parsed_url.scheme() == "http"
        && parsed_url.host_str().is_some_and(|host| {
            host.eq_ignore_ascii_case("localhost")
                || host
                    .parse::<IpAddr>()
                    .is_ok_and(|address| address.is_loopback())
        });
    if !secure && !controlled_loopback {
        return Err(format!(
            "模型 {model_id} 的{source_label} URL 必须使用 HTTPS；HTTP 仅允许本机回环地址。"
        ));
    }

    Ok(ModelConnectionConfig {
        api_url: api_url.to_string(),
        api_token: api_token.to_string(),
    })
}

impl ModelConfigRecord {
    /// Stable user-facing identity. The immutable configuration id is deliberately never
    /// projected.
    pub fn display_label(&self) -> String {
        self.display_name.trim().to_string()
    }

    pub fn effective_context_window_tokens(&self) -> u32 {
        self.context_window_tokens
            .unwrap_or(DEFAULT_MODEL_CONTEXT_WINDOW_TOKENS)
    }

    /// Resolves the editable inheritance marker into the explicit price frozen for one run.
    pub fn effective_cached_input_price(&self) -> &str {
        if self.cached_input_price.trim().is_empty() {
            &self.input_price
        } else {
            &self.cached_input_price
        }
    }

    pub fn resolved_provider_profile_config(
        &self,
        dialect: ProviderProtocolDialect,
    ) -> Result<ProviderProfileConfig, ProviderProfileValidationError> {
        self.provider_profile_config
            .validate_for_model(&self.provider_model_id, dialect)?;
        Ok(self.provider_profile_config.clone())
    }

    pub fn connection_override(&self) -> Result<Option<ModelConnectionConfig>, String> {
        let api_url = self.api_url_override.as_deref().unwrap_or_default().trim();
        let api_token = self
            .api_token_override
            .as_deref()
            .unwrap_or_default()
            .trim();

        // The override is atomic: both values override the global pair, while two blanks
        // inherit it. A partial pair must never borrow its missing half from global settings.
        match (api_url.is_empty(), api_token.is_empty()) {
            (true, true) => Ok(None),
            (false, false) => {
                validated_connection(&self.display_name, "专用", api_url, api_token).map(Some)
            }
            _ => Err(format!(
                "模型 {} 的专用 URL 和 API Token 必须同时填写或同时留空。",
                self.display_name
            )),
        }
    }
}

impl ModelSettingsRecord {
    pub fn effective_connection_for(
        &self,
        model: &ModelConfigRecord,
    ) -> Result<ModelConnectionConfig, String> {
        if let Some(connection) = model.connection_override()? {
            return Ok(connection);
        }

        let api_url = self.api_url.trim();
        let api_token = self.api_token.trim();
        if api_url.is_empty() || api_token.is_empty() {
            return Err(format!(
                "模型 {} 没有专用连接配置，请先完整配置全局 URL 和 API Token。",
                model.display_name
            ));
        }

        validated_connection(&model.display_name, "全局", api_url, api_token)
    }
}
