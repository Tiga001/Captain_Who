use crate::protocol::{
    AgentGuidanceStatus, AgentInputAttachmentEncoding, AgentInputAttachmentKind, AgentPermissions,
};
use crate::provider_profile::{
    ProviderProfileConfig, ProviderProfileId, ProviderProfilePublicSettings,
    ProviderProfileValidationError, ProviderProtocolDialect, ProviderVendorId,
    ProviderVendorPublicSettings,
};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::net::IpAddr;
use unicode_normalization::UnicodeNormalization;

fn deserialize_required_nullable<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

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
    SelectRegisteredProfile {
        profile_id: ProviderProfileId,
        settings: ProviderProfilePublicSettings,
    },
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
    DuplicateDisplayName { display_name: String },
    Other(String),
}

impl std::fmt::Display for ModelSettingsSaveError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DuplicateDisplayName { display_name } => {
                write!(formatter, "模型显示名称重复：{display_name}")
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

#[cfg(test)]
mod model_connection_tests {
    use super::*;

    fn model(
        api_url_override: Option<&str>,
        api_token_override: Option<&str>,
    ) -> ModelConfigRecord {
        ModelConfigRecord {
            id: "model-a".to_string(),
            provider_model_id: "model-a".to_string(),
            display_name: "Model A".to_string(),
            api_url_override: api_url_override.map(ToString::to_string),
            api_token_override: api_token_override.map(ToString::to_string),
            supports_image: false,
            context_window_tokens: None,
            provider_profile_config: ProviderProfileConfig::generic_for_dialect(
                ProviderProtocolDialect::OpenAiChatCompletions,
            ),
            input_price: "0".to_string(),
            cached_input_price: String::new(),
            output_price: "0".to_string(),
            enabled: true,
        }
    }

    fn settings(model: ModelConfigRecord) -> ModelSettingsRecord {
        ModelSettingsRecord {
            api_url: "https://global.example/v1".to_string(),
            api_token: "global-token".to_string(),
            search_mode: "disabled".to_string(),
            tavily_api_key: String::new(),
            models: vec![model],
        }
    }

    #[test]
    fn display_label_uses_the_unique_display_name_without_exposing_the_internal_id() {
        let mut model = model(None, None);
        model.id = "model-config:private-uuid".to_string();
        model.display_name = "  Kimi K3 Max  ".to_string();

        assert_eq!(model.display_label(), "Kimi K3 Max");
        assert!(!model.display_label().contains("private-uuid"));
    }

    #[test]
    fn complete_model_connection_overrides_the_global_pair() {
        let settings = settings(model(Some("https://model.example/v1"), Some("model-token")));
        let connection = settings
            .effective_connection_for(&settings.models[0])
            .unwrap();

        assert_eq!(connection.api_url, "https://model.example/v1");
        assert_eq!(connection.api_token, "model-token");
    }

    #[test]
    fn two_blank_model_values_inherit_the_global_pair() {
        let settings = settings(model(None, None));
        let connection = settings
            .effective_connection_for(&settings.models[0])
            .unwrap();

        assert_eq!(connection.api_url, "https://global.example/v1");
        assert_eq!(connection.api_token, "global-token");
    }

    #[test]
    fn partial_model_connection_never_mixes_with_global_settings() {
        let settings = settings(model(Some("https://model.example/v1"), None));

        assert!(settings
            .effective_connection_for(&settings.models[0])
            .is_err());
    }

    #[test]
    fn provider_urls_reject_unsafe_authority_and_url_components() {
        for url in [
            "http://provider.example/v1",
            "https://user:password@provider.example/v1",
            "https://provider.example/v1?token=secret",
            "https://provider.example/v1#secret",
        ] {
            let settings = settings(model(Some(url), Some("model-token")));
            assert!(settings
                .effective_connection_for(&settings.models[0])
                .is_err());
        }
    }

    #[test]
    fn provider_urls_enforce_utf8_byte_limit_and_reject_control_characters() {
        let prefix = "https://provider.example/";
        let maximum = format!(
            "{prefix}{}",
            "a".repeat(MODEL_PROVIDER_API_URL_MAX_BYTES - prefix.len())
        );
        assert_eq!(maximum.len(), MODEL_PROVIDER_API_URL_MAX_BYTES);
        assert!(validated_connection("Model A", "专用", &maximum, "token").is_ok());

        let overlong_unicode = format!("{prefix}{}", "界".repeat(1_400));
        assert!(overlong_unicode.chars().count() < MODEL_PROVIDER_API_URL_MAX_BYTES);
        assert!(overlong_unicode.len() > MODEL_PROVIDER_API_URL_MAX_BYTES);
        assert!(validated_connection("Model A", "专用", &overlong_unicode, "token").is_err());

        for url in [
            "https://provider.example/v1\n",
            "https://provider.example/\u{0001}v1",
        ] {
            assert!(validated_connection("Model A", "专用", url, "token").is_err());
        }
    }

    #[cfg(debug_assertions)]
    #[test]
    fn development_build_allows_only_explicit_loopback_http() {
        for url in ["http://localhost:11434/v1", "http://127.0.0.1:11434/v1"] {
            let settings = settings(model(Some(url), Some("model-token")));
            assert!(settings
                .effective_connection_for(&settings.models[0])
                .is_ok());
        }
        let settings = settings(model(
            Some("http://192.168.1.20:11434/v1"),
            Some("model-token"),
        ));
        assert!(settings
            .effective_connection_for(&settings.models[0])
            .is_err());
    }

    #[test]
    fn omitted_context_window_uses_the_backend_default() {
        let model = model(None, None);

        assert_eq!(
            model.effective_context_window_tokens(),
            DEFAULT_MODEL_CONTEXT_WINDOW_TOKENS
        );
    }

    #[test]
    fn configured_context_window_overrides_the_backend_default() {
        let mut model = model(None, None);
        model.context_window_tokens = Some(256_000);

        assert_eq!(model.effective_context_window_tokens(), 256_000);
    }

    #[test]
    fn cached_input_price_inherits_input_price_until_explicitly_configured() {
        let mut model = model(None, None);
        model.input_price = "0.012".to_string();

        assert_eq!(model.effective_cached_input_price(), "0.012");

        model.cached_input_price = "0.002".to_string();
        assert_eq!(model.effective_cached_input_price(), "0.002");
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ProjectRecord {
    pub id: String,
    pub name: String,
    pub path: Option<String>,
    pub created_at: i64,
    pub pinned_at: Option<i64>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ChatMessageRecord {
    /// Output-only history proof. Only native answer admission and fork copying may create it.
    #[serde(default, skip_deserializing, skip_serializing_if = "Option::is_none")]
    pub human_interaction_response:
        Option<crate::human_interaction::HumanInteractionResponseDisplay>,
    pub id: String,
    pub role: String,
    pub content: String,
    pub created_at: i64,
    pub status: Option<String>,
    #[serde(default)]
    pub attachments: Vec<ChatMessageAttachmentRecord>,
    pub agent_run_json: Option<String>,
    pub ui_state_json: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ChatMessageStateRecord {
    pub id: String,
    pub content: String,
    pub status: Option<String>,
    pub agent_run_json: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ChatConversationMetaRecord {
    pub id: String,
    pub project_id: Option<String>,
    pub model_id: Option<String>,
    pub title: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub pinned_at: Option<i64>,
    pub archived_at: Option<i64>,
    pub unread_at: Option<i64>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentRecord {
    pub id: String,
    pub conversation_id: String,
    pub message_id: String,
    pub project_id: Option<String>,
    pub kind: String,
    pub original_name: String,
    pub mime_type: Option<String>,
    pub size_bytes: u64,
    pub storage_rel_path: String,
    pub created_at: i64,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ChatMessageAttachmentRecord {
    pub id: String,
    pub kind: String,
    pub name: String,
    pub mime_type: Option<String>,
    pub size_bytes: u64,
    pub preview_data: Option<String>,
    pub preview_mime_type: Option<String>,
    pub created_at: i64,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentImageRecord {
    pub id: String,
    pub name: String,
    pub mime_type: String,
    pub size_bytes: u64,
    pub data: String,
    pub created_at: i64,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ChatConversationRecord {
    pub id: String,
    pub project_id: Option<String>,
    pub model_id: Option<String>,
    pub title: String,
    pub messages: Vec<ChatMessageRecord>,
    pub created_at: i64,
    pub updated_at: i64,
    pub pinned_at: Option<i64>,
    pub archived_at: Option<i64>,
    pub unread_at: Option<i64>,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ConversationContinuationOriginRecord {
    pub source_conversation_id: String,
    pub source_message_id: String,
    pub boundary_message_id: String,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ChatConversationViewRecord {
    #[serde(flatten)]
    pub conversation: ChatConversationRecord,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub continuation_origin: Option<ConversationContinuationOriginRecord>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ForkConversationRequest {
    pub request_id: String,
    pub source_conversation_id: String,
    pub fork_point: ConversationForkPoint,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ConversationForkPoint {
    Latest {},
    AssistantReply { assistant_message_id: String },
    ProviderTransitionBoundary { operation_id: String },
    ManualCompactionBoundary { operation_id: String },
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ChatSearchInput {
    pub query: String,
    pub limit: Option<u32>,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ChatSearchMatchKind {
    Title,
    Message,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ChatSearchResult {
    pub conversation_id: String,
    pub project_id: Option<String>,
    pub title: String,
    pub message_id: Option<String>,
    pub snippet: Option<String>,
    pub match_kind: ChatSearchMatchKind,
    pub updated_at: i64,
}

/// Permission choices persisted before this version predate the current `full` semantics and
/// must not be interpreted as an explicit opt-in to those broader privileges.
pub const CURRENT_COMPOSER_PERMISSION_MODE_VERSION: i64 = 2;
const COMPOSER_DRAFT_PAYLOAD_ERROR: &str = "stored_composer_draft_malformed";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredComposerAttachment {
    id: String,
    kind: AgentInputAttachmentKind,
    name: String,
    mime_type: Option<String>,
    size_bytes: u64,
    encoding: AgentInputAttachmentEncoding,
    data: String,
    truncated: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredComposerSkillSelection {
    id: String,
    revision: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum StoredComposerPermissionMode {
    Default,
    Full,
    Custom,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum StoredComposerQueueStatus {
    Pending,
    Submitting,
    Error,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredComposerQueuedMessage {
    id: String,
    client_message_id: String,
    content: String,
    attachments: Vec<StoredComposerAttachment>,
    model_id: String,
    permission_mode: StoredComposerPermissionMode,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    project_id: Option<String>,
    skills: Vec<StoredComposerSkillSelection>,
    status: StoredComposerQueueStatus,
    error: Option<String>,
    created_at: u64,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ComposerDraftRecord {
    pub scope_id: String,
    pub message: String,
    pub permission_mode: String,
    pub permission_mode_version: i64,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub model_id: Option<String>,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub project_id: Option<String>,
    pub attachments_json: String,
    pub skills_json: String,
    pub queued_messages_json: String,
    pub updated_at: i64,
}

impl ComposerDraftRecord {
    pub(crate) fn validate_current_payloads(&self) -> Result<(), String> {
        if !matches!(self.permission_mode.as_str(), "default" | "full" | "custom")
            || self.permission_mode_version < 0
        {
            return Err(COMPOSER_DRAFT_PAYLOAD_ERROR.to_string());
        }
        let attachments =
            serde_json::from_str::<Vec<StoredComposerAttachment>>(&self.attachments_json)
                .map_err(|_| COMPOSER_DRAFT_PAYLOAD_ERROR.to_string())?;
        let skills = serde_json::from_str::<Vec<StoredComposerSkillSelection>>(&self.skills_json)
            .map_err(|_| COMPOSER_DRAFT_PAYLOAD_ERROR.to_string())?;
        let queued =
            serde_json::from_str::<Vec<StoredComposerQueuedMessage>>(&self.queued_messages_json)
                .map_err(|_| COMPOSER_DRAFT_PAYLOAD_ERROR.to_string())?;

        validate_stored_composer_attachments(&attachments)?;
        validate_stored_composer_skills(&skills)?;
        for message in &queued {
            validate_stored_composer_queued_message(message)?;
        }
        Ok(())
    }

    /// Fails closed when a persisted `full` choice was made under different or unknown
    /// semantics. Other modes do not gain authority from this version marker.
    pub fn normalize_permission_mode(mut self) -> Self {
        if self.permission_mode == "full"
            && self.permission_mode_version != CURRENT_COMPOSER_PERMISSION_MODE_VERSION
        {
            self.permission_mode = "default".to_string();
        }
        self
    }
}

fn validate_stored_composer_attachments(
    attachments: &[StoredComposerAttachment],
) -> Result<(), String> {
    for attachment in attachments {
        // Reading every field here makes the current durable contract explicit. These values are
        // intentionally opaque to storage, but their shape must be complete before persistence.
        let _ = (
            &attachment.id,
            &attachment.kind,
            &attachment.name,
            &attachment.mime_type,
            attachment.size_bytes,
            &attachment.encoding,
            &attachment.data,
            &attachment.truncated,
        );
    }
    Ok(())
}

fn validate_stored_composer_skills(skills: &[StoredComposerSkillSelection]) -> Result<(), String> {
    if skills.len() > 8 {
        return Err(COMPOSER_DRAFT_PAYLOAD_ERROR.to_string());
    }
    let mut ids = BTreeSet::new();
    for selection in skills {
        crate::skills::SkillSelection::parse(&selection.id, &selection.revision)
            .map_err(|_| COMPOSER_DRAFT_PAYLOAD_ERROR.to_string())?;
        if !ids.insert(&selection.id) {
            return Err(COMPOSER_DRAFT_PAYLOAD_ERROR.to_string());
        }
    }
    Ok(())
}

fn validate_stored_composer_queued_message(
    message: &StoredComposerQueuedMessage,
) -> Result<(), String> {
    let _ = (
        &message.id,
        &message.client_message_id,
        &message.content,
        &message.model_id,
        &message.permission_mode,
        &message.project_id,
        &message.status,
        &message.error,
        message.created_at,
    );
    validate_stored_composer_attachments(&message.attachments)?;
    validate_stored_composer_skills(&message.skills)
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct UiPreferencesRecord {
    pub profile_avatar_data_url: Option<String>,
    pub profile_display_name: String,
    pub profile_handle: String,
    pub sidebar_conversation_sort: String,
    pub sidebar_project_sort: String,
    pub sidebar_project_order: Vec<String>,
    pub sidebar_section_order: String,
    pub native_font_smoothing: bool,
    pub show_token_usage_details: bool,
    pub show_context_window_usage: bool,
    pub translucent_sidebar: bool,
    pub translucent_sidebar_transparency: i64,
    pub full_permission_enabled: bool,
    pub custom_permission_enabled: bool,
    pub custom_permissions: AgentPermissions,
    pub updated_at: i64,
}

pub const BROWSER_DOWNLOAD_SCHEMA_VERSION: u32 = 2;

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BrowserDownloadLocationMode {
    System,
    Custom,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BrowserDownloadSource {
    Manual,
    Agent,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BrowserDownloadSettingsRecord {
    pub schema_version: u32,
    pub location_mode: BrowserDownloadLocationMode,
    pub custom_directory: Option<String>,
    pub ask_where_to_save: bool,
    pub revision: u64,
    pub updated_at: i64,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BrowserDownloadSettingsUpdate {
    pub schema_version: u32,
    pub location_mode: BrowserDownloadLocationMode,
    pub custom_directory: Option<String>,
    pub ask_where_to_save: bool,
    pub expected_revision: u64,
    pub updated_at: i64,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BrowserDownloadRegistration {
    pub schema_version: u32,
    pub download_id: String,
    pub source: BrowserDownloadSource,
    pub display_name: String,
    pub mime_type: String,
    pub size_bytes: u64,
    pub sha256: String,
    pub absolute_path: String,
    pub source_origin: Option<String>,
    pub conversation_id: Option<String>,
    pub run_id: Option<String>,
    pub call_id: Option<String>,
    pub created_at: i64,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BrowserDownloadRecord {
    pub schema_version: u32,
    pub download_id: String,
    pub source: BrowserDownloadSource,
    pub display_name: String,
    pub mime_type: String,
    pub size_bytes: u64,
    pub sha256: String,
    pub absolute_path: String,
    pub source_origin: Option<String>,
    pub conversation_id: Option<String>,
    pub project_id: Option<String>,
    pub run_id: Option<String>,
    pub call_id: Option<String>,
    pub created_at: i64,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BrowserDownloadListInput {
    pub schema_version: u32,
    pub query: String,
    pub limit: u32,
}

pub const BROWSER_DATA_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BrowserLinkOpenTarget {
    System,
    Builtin,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BrowserPreferencesRecord {
    pub schema_version: u32,
    pub link_open_target: BrowserLinkOpenTarget,
    pub revision: u64,
    pub updated_at: i64,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BrowserPreferencesUpdate {
    pub schema_version: u32,
    pub link_open_target: BrowserLinkOpenTarget,
    pub expected_revision: u64,
    pub updated_at: i64,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BrowserHistoryRecord {
    pub schema_version: u32,
    pub history_id: String,
    pub url: String,
    pub title: String,
    pub hostname: String,
    pub favicon_url: Option<String>,
    pub visited_at: i64,
}

pub type BrowserHistoryRegistration = BrowserHistoryRecord;

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BrowserHistoryMetadataUpdate {
    pub schema_version: u32,
    pub history_id: String,
    pub title: String,
    pub favicon_url: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BrowserHistoryListInput {
    pub schema_version: u32,
    pub query: String,
    pub limit: u32,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BrowserHistoryDeleteInput {
    pub schema_version: u32,
    pub history_ids: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BrowserOwnedDataRangeInput {
    pub schema_version: u32,
    pub since: Option<i64>,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BrowserOwnedDataSummary {
    pub schema_version: u32,
    pub history_count: u64,
    pub history_site_count: u64,
    pub download_count: u64,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BrowserOwnedDataClearInput {
    pub schema_version: u32,
    pub since: Option<i64>,
    pub clear_history: bool,
    pub clear_downloads: bool,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BrowserOwnedDataClearOutput {
    pub schema_version: u32,
    pub deleted_history_count: u64,
    pub deleted_download_count: u64,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AgentPromptPreferencesRecord {
    pub work_mode: String,
    pub tone: String,
    pub detail_level: String,
    pub custom_instructions: String,
    pub updated_at: i64,
}

#[derive(Debug, Clone)]
pub struct AgentUsageRecordInsert {
    pub id: String,
    pub conversation_id: String,
    pub message_id: String,
    pub run_id: String,
    pub project_id: Option<String>,
    pub model_id: String,
    pub model_name: String,
    pub started_at: Option<i64>,
    pub completed_at: Option<i64>,
    pub status: Option<String>,
    pub error: Option<String>,
    pub created_at: i64,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub output_thinking_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    pub cached_input_tokens: Option<u64>,
    pub cache_creation_input_tokens: Option<u64>,
    pub billable_request_count: u64,
    pub input_price: Option<String>,
    pub cached_input_price: Option<String>,
    pub output_price: Option<String>,
    pub estimated_cost: Option<f64>,
}

/// A maintenance operation owns its lifecycle and billing independently of chat messages.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManualContextCompactionOperation {
    pub operation_id: String,
    pub request_id: String,
    pub conversation_id: String,
    pub status: String,
    pub phase: String,
    pub assistant_message_id: Option<String>,
    pub covered_through_message_id: Option<String>,
    pub model_id: Option<String>,
    pub summary_id: Option<String>,
    pub source_input_tokens: Option<u64>,
    pub replacement_input_tokens: Option<u64>,
    pub error: Option<String>,
    pub started_at: i64,
    pub updated_at: i64,
    pub completed_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ManualContextCompactionUsageRecord {
    pub operation_id: String,
    pub conversation_id: String,
    pub project_id: Option<String>,
    pub model_id: String,
    pub model_name: String,
    pub started_at: Option<i64>,
    pub completed_at: Option<i64>,
    pub status: Option<String>,
    pub error: Option<String>,
    pub created_at: i64,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub output_thinking_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    pub cached_input_tokens: Option<u64>,
    pub cache_creation_input_tokens: Option<u64>,
    pub billable_request_count: u64,
    pub input_price: Option<String>,
    pub cached_input_price: Option<String>,
    pub output_price: Option<String>,
    pub estimated_cost: Option<f64>,
}

#[derive(Clone)]
pub struct AgentActionAuditRecord {
    pub action_id: String,
    pub run_id: String,
    pub conversation_id: Option<String>,
    pub assistant_message_id: Option<String>,
    pub action_type: String,
    pub tool_name: String,
    pub decision: Option<String>,
    pub status: String,
    pub action_json: String,
    pub file_change_result_json: Option<String>,
    pub command_result_json: Option<String>,
    pub tool_result_json: Option<String>,
    pub error: Option<String>,
    pub created_at: i64,
    pub decided_at: Option<i64>,
    pub completed_at: Option<i64>,
    pub effective_permissions_json: Option<String>,
    pub path_scope: Option<String>,
    pub command_cwd_scope: Option<String>,
    pub blocked_reason: Option<String>,
    pub decision_source: Option<String>,
}

impl std::fmt::Debug for AgentActionAuditRecord {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("AgentActionAuditRecord([REDACTED])")
    }
}

/// Persisted file-producing action whose process outcome cannot be proven after restart.
///
/// An `executing` claim is intentionally treated as effects-may-have-occurred. Conversation or
/// project deletion must not erase it until a separate recovery workflow settles the receipt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentUnsettledFileEffect {
    pub project_id: Option<String>,
    pub conversation_id: String,
    pub run_id: String,
    pub action_id: String,
}

#[derive(Clone, PartialEq, Eq)]
pub struct AgentPendingActionRecord {
    pub action_id: String,
    pub run_id: String,
    pub conversation_id: Option<String>,
    pub assistant_message_id: Option<String>,
    pub action_type: String,
    pub tool_name: String,
    pub tool_call_id: Option<String>,
    pub status: String,
    /// Durable write-ahead outcome produced by the action executor.
    ///
    /// This is intentionally independent from the assistant continuation run status: a failed
    /// action may still be followed by a successfully persisted assistant explanation. Startup
    /// reconciliation uses this value when a crash occurs before the lifecycle CAS is committed.
    pub target_status: Option<String>,
    pub action_json: String,
    pub agent_input_json: String,
    pub created_at: i64,
    pub updated_at: i64,
}

impl std::fmt::Debug for AgentPendingActionRecord {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("AgentPendingActionRecord([REDACTED])")
    }
}

/// Authenticated ciphertext envelope for one pending MCP approval payload.
///
/// Core storage accepts only the opaque invocation/action binding, the AEAD nonce and ciphertext,
/// the digest of Host-reconstructed authenticated metadata, and lifecycle timestamps. It cannot
/// accept raw MCP arguments, the authenticated metadata itself, encryption-key references, or a
/// serializable plaintext payload type.
#[derive(Clone, PartialEq, Eq)]
pub struct McpApprovalEnvelopeRecord {
    pub invocation_id: String,
    pub action_id: String,
    pub envelope_version: i64,
    pub nonce_base64: String,
    pub ciphertext_base64: String,
    pub aad_digest: String,
    pub created_at: i64,
    pub expires_at: i64,
}

impl std::fmt::Debug for McpApprovalEnvelopeRecord {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("McpApprovalEnvelopeRecord([REDACTED])")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRunGuidanceRecord {
    pub guidance_id: String,
    pub client_message_id: String,
    pub run_id: String,
    pub conversation_id: String,
    pub assistant_message_id: String,
    pub content: String,
    pub status: AgentGuidanceStatus,
    pub attachment_ids: Vec<String>,
    pub applied_trace_sequence: Option<u64>,
    pub terminal_reason: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Clone, PartialEq, Eq)]
pub struct AgentFileChangeRecord {
    pub schema_version: u32,
    pub id: String,
    pub conversation_id: String,
    pub project_id: Option<String>,
    pub run_id: String,
    pub source_tool_name: String,
    pub source_tool_call_id: String,
    pub source_tool_arguments_digest: String,
    pub permission_revision: String,
    pub tool_set_revision: String,
    pub provider_wire_revision: String,
    pub observation_id: String,
    pub observation_json: String,
    pub file_path: String,
    pub operation: String,
    pub strategy: Option<String>,
    pub status: String,
    pub base_revision: Option<String>,
    pub base_content: String,
    pub content: String,
    pub draft_revision: u64,
    pub next_mutation_index: u64,
    pub additions: u64,
    pub deletions: u64,
    pub line_count: u64,
    pub byte_count: u64,
    pub mutation_count: u64,
    pub stats_final: bool,
    pub summary: Option<String>,
    pub final_action_id: Option<String>,
    pub final_action_arguments_digest: Option<String>,
    pub final_permission_revision: Option<String>,
    pub final_tool_set_revision: Option<String>,
    pub final_provider_wire_revision: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub expires_at: i64,
}

impl std::fmt::Debug for AgentFileChangeRecord {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("AgentFileChangeRecord([REDACTED])")
    }
}

/// Metadata needed to render and enforce the current Run's FileTransactionState.
///
/// This projection deliberately excludes draft bodies, observations, and action bindings. It is
/// not an authorization record; mutations still load and validate the full transaction record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentFileChangeRuntimeState {
    pub id: String,
    pub conversation_id: String,
    pub project_id: Option<String>,
    pub run_id: String,
    pub source_tool_name: String,
    pub file_path: String,
    pub operation: String,
    pub strategy: Option<String>,
    pub status: String,
    pub draft_revision: u64,
    pub next_mutation_index: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentFileChangeChunkRecord {
    pub transaction_id: String,
    pub mutation_index: u64,
    pub content_digest: String,
    pub byte_count: u64,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentFileChangeOperationRecord {
    pub transaction_id: String,
    pub mutation_index: u64,
    pub source_tool_call_id: String,
    pub source_tool_arguments_digest: String,
    pub action: String,
    pub payload_digest: String,
    pub draft_revision: u64,
    pub receipt_json: String,
    pub created_at: i64,
}

pub use crate::file_change::FileChangeRunGrantRecord;

#[cfg(test)]
mod security_tests {
    use super::*;

    const CANARY: &str = "STORAGE_DEBUG_SECRET_CANARY";

    #[test]
    fn sensitive_agent_storage_records_have_constant_redacted_debug() {
        let audit = AgentActionAuditRecord {
            action_id: CANARY.to_string(),
            run_id: CANARY.to_string(),
            conversation_id: Some(CANARY.to_string()),
            assistant_message_id: Some(CANARY.to_string()),
            action_type: CANARY.to_string(),
            tool_name: CANARY.to_string(),
            decision: Some(CANARY.to_string()),
            status: CANARY.to_string(),
            action_json: CANARY.to_string(),
            file_change_result_json: Some(CANARY.to_string()),
            command_result_json: Some(CANARY.to_string()),
            tool_result_json: Some(CANARY.to_string()),
            error: Some(CANARY.to_string()),
            created_at: 1,
            decided_at: Some(2),
            completed_at: Some(3),
            effective_permissions_json: Some(CANARY.to_string()),
            path_scope: Some(CANARY.to_string()),
            command_cwd_scope: Some(CANARY.to_string()),
            blocked_reason: Some(CANARY.to_string()),
            decision_source: Some(CANARY.to_string()),
        };
        let pending = AgentPendingActionRecord {
            action_id: CANARY.to_string(),
            run_id: CANARY.to_string(),
            conversation_id: Some(CANARY.to_string()),
            assistant_message_id: Some(CANARY.to_string()),
            action_type: CANARY.to_string(),
            tool_name: CANARY.to_string(),
            tool_call_id: Some(CANARY.to_string()),
            status: CANARY.to_string(),
            target_status: Some(CANARY.to_string()),
            action_json: CANARY.to_string(),
            agent_input_json: CANARY.to_string(),
            created_at: 1,
            updated_at: 2,
        };
        let file_change = AgentFileChangeRecord {
            schema_version: 1,
            id: CANARY.to_string(),
            conversation_id: CANARY.to_string(),
            project_id: Some(CANARY.to_string()),
            run_id: CANARY.to_string(),
            source_tool_name: "apply_patch".to_string(),
            source_tool_call_id: CANARY.to_string(),
            source_tool_arguments_digest: CANARY.to_string(),
            permission_revision: CANARY.to_string(),
            tool_set_revision: CANARY.to_string(),
            provider_wire_revision: CANARY.to_string(),
            observation_id: CANARY.to_string(),
            observation_json: CANARY.to_string(),
            file_path: CANARY.to_string(),
            operation: "create".to_string(),
            strategy: None,
            status: "drafting".to_string(),
            base_revision: None,
            base_content: CANARY.to_string(),
            content: CANARY.to_string(),
            draft_revision: 0,
            next_mutation_index: 0,
            additions: 0,
            deletions: 0,
            line_count: 1,
            byte_count: CANARY.len() as u64,
            mutation_count: 0,
            stats_final: false,
            summary: Some(CANARY.to_string()),
            final_action_id: None,
            final_action_arguments_digest: None,
            final_permission_revision: None,
            final_tool_set_revision: None,
            final_provider_wire_revision: None,
            created_at: 1,
            updated_at: 2,
            expires_at: 3,
        };

        assert_eq!(format!("{audit:?}"), "AgentActionAuditRecord([REDACTED])");
        assert_eq!(
            format!("{pending:?}"),
            "AgentPendingActionRecord([REDACTED])"
        );
        assert_eq!(
            format!("{file_change:?}"),
            "AgentFileChangeRecord([REDACTED])"
        );
        assert!(!format!("{audit:?}{pending:?}{file_change:?}").contains(CANARY));
    }

    #[test]
    fn mcp_envelope_record_debug_never_exposes_ciphertext_or_identity() {
        let envelope = McpApprovalEnvelopeRecord {
            invocation_id: CANARY.to_string(),
            action_id: CANARY.to_string(),
            envelope_version: 1,
            nonce_base64: CANARY.to_string(),
            ciphertext_base64: CANARY.to_string(),
            aad_digest: CANARY.to_string(),
            created_at: 1,
            expires_at: 2,
        };

        let rendered = format!("{envelope:?}");
        assert_eq!(rendered, "McpApprovalEnvelopeRecord([REDACTED])");
        assert!(!rendered.contains(CANARY));
    }

    #[test]
    fn model_configuration_debug_is_write_only_for_credentials() {
        let model = ModelConfigRecord {
            id: "test-model".to_string(),
            provider_model_id: "test-model".to_string(),
            display_name: "Test Model".to_string(),
            api_url_override: Some("https://example.test/v1".to_string()),
            api_token_override: Some(CANARY.to_string()),
            supports_image: false,
            context_window_tokens: Some(128_000),
            provider_profile_config: ProviderProfileConfig::generic_for_dialect(
                ProviderProtocolDialect::OpenAiChatCompletions,
            ),
            input_price: "0".to_string(),
            cached_input_price: String::new(),
            output_price: "0".to_string(),
            enabled: true,
        };
        let settings = ModelSettingsRecord {
            api_url: "https://example.test/v1".to_string(),
            api_token: CANARY.to_string(),
            search_mode: "tavily".to_string(),
            tavily_api_key: CANARY.to_string(),
            models: vec![model.clone()],
        };
        let connection = ModelConnectionConfig {
            api_url: "https://example.test/v1".to_string(),
            api_token: CANARY.to_string(),
        };

        let rendered = format!("{model:?}{settings:?}{connection:?}");
        assert!(!rendered.contains(CANARY));
        assert!(!rendered.contains("example.test"));
    }
}
