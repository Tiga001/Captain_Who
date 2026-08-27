use crate::protocol::{
    AgentGuidanceStatus, AgentInputAttachmentEncoding, AgentInputAttachmentKind, AgentPermissions,
};
use crate::provider_profile::{
    ProviderProfileConfig, ProviderProfileId, ProviderProfilePublicSettings,
    ProviderProfileValidationError, ProviderProtocolDialect,
};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

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

#[derive(Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelConfigRecord {
    /// Opaque identifier sent verbatim as the provider API's `model` value.
    pub id: String,
    /// User-facing label only; it never participates in provider routing.
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

impl std::fmt::Debug for ModelConfigRecord {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ModelConfigRecord([REDACTED])")
    }
}

#[derive(Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelSettingsRecord {
    pub api_url: String,
    pub api_token: String,
    pub search_mode: String,
    pub tavily_api_key: String,
    pub models: Vec<ModelConfigRecord>,
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
}

/// Wire-only model mutation DTO. It is converted into [`ModelConfigRecord`] before persistence,
/// so update intent can never become a second stored fact.
#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelConfigSaveRequest {
    pub id: String,
    /// Stable edit identity used only to merge an id rename with the persisted record. It is
    /// null for a newly created model and never participates in Provider wire identity.
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub previous_model_id: Option<String>,
    pub display_name: String,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub api_url_override: Option<String>,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub api_token_override: Option<String>,
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
    pub fn into_record(self, provider_profile_config: ProviderProfileConfig) -> ModelConfigRecord {
        ModelConfigRecord {
            id: self.id,
            display_name: self.display_name,
            api_url_override: self.api_url_override,
            api_token_override: self.api_token_override,
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

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelSettingsSaveRequest {
    pub api_url: String,
    pub api_token: String,
    pub search_mode: String,
    pub tavily_api_key: String,
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
    DuplicateModelId { model_id: String },
    Other(String),
}

impl std::fmt::Display for ModelSettingsSaveError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DuplicateModelId { model_id } => {
                write!(formatter, "模型 ID 重复：{model_id}")
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

/// Host-only snapshot of model settings and the opaque identity of the exact saved revision.
///
/// `configuration_revision` is deliberately not part of [`ModelSettingsRecord`], which is also
/// used by Renderer-facing settings APIs. A new random revision is generated for every successful
/// settings save, including saves whose visible values happen to be identical.
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

fn validated_connection(
    model_id: &str,
    source_label: &str,
    api_url: &str,
    api_token: &str,
) -> Result<ModelConnectionConfig, String> {
    let api_url = api_url.trim();
    let api_token = api_token.trim();
    let parsed_url =
        Url::parse(api_url).map_err(|_| format!("模型 {model_id} 的{source_label} URL 无效。"))?;
    if !matches!(parsed_url.scheme(), "http" | "https") {
        return Err(format!(
            "模型 {model_id} 的{source_label} URL 只支持 http 或 https。"
        ));
    }

    Ok(ModelConnectionConfig {
        api_url: api_url.to_string(),
        api_token: api_token.to_string(),
    })
}

impl ModelConfigRecord {
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
        self.provider_profile_config.validate_for_dialect(dialect)?;
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
            (false, false) => validated_connection(&self.id, "专用", api_url, api_token).map(Some),
            _ => Err(format!(
                "模型 {} 的专用 URL 和 API Token 必须同时填写或同时留空。",
                self.id
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
                model.id
            ));
        }

        validated_connection(&model.id, "全局", api_url, api_token)
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
    pub ui_state_json: Option<String>,
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
    AssistantReply { assistant_message_id: String },
    ProviderTransitionBoundary { operation_id: String },
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
    pub patch_result_json: Option<String>,
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

#[derive(Debug, Clone)]
pub struct AgentFileDraftRecord {
    pub id: String,
    pub conversation_id: String,
    pub project_id: Option<String>,
    pub run_id: String,
    pub file_path: String,
    pub mode: String,
    pub status: String,
    pub base_revision: Option<String>,
    pub base_content: String,
    pub content: String,
    pub additions: u64,
    pub deletions: u64,
    pub line_count: u64,
    pub byte_count: u64,
    pub chunk_count: u64,
    pub next_chunk_index: u64,
    pub stats_final: bool,
    pub summary: Option<String>,
    pub final_action_id: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub expires_at: i64,
}

#[derive(Debug, Clone)]
pub struct AgentFileDraftChunkRecord {
    pub draft_id: String,
    pub chunk_index: u64,
    pub content_hash: String,
    pub byte_count: u64,
    pub created_at: i64,
}

#[derive(Debug, Clone)]
pub struct AgentFileDraftOperationRecord {
    pub draft_id: String,
    pub sequence: u64,
    pub operation: String,
    pub payload_hash: String,
    pub created_at: i64,
}

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
            patch_result_json: Some(CANARY.to_string()),
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

        assert_eq!(format!("{audit:?}"), "AgentActionAuditRecord([REDACTED])");
        assert_eq!(
            format!("{pending:?}"),
            "AgentPendingActionRecord([REDACTED])"
        );
        assert!(!format!("{audit:?}{pending:?}").contains(CANARY));
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
