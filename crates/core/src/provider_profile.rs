use crate::protocol::AgentApiStyle;
use serde::{Deserialize, Serialize};
use std::error::Error;
use std::fmt::{Display, Formatter};

pub const PROVIDER_PROFILE_CONFIG_SCHEMA_VERSION: u32 = 1;
pub const GENERIC_OPENAI_CHAT_PROFILE_VERSION: u32 = 1;
pub const GENERIC_ANTHROPIC_MESSAGES_PROFILE_VERSION: u32 = 1;
pub const DEEPSEEK_V4_CHAT_PROFILE_VERSION: u32 = 1;

/// The provider wire dialect selected for one immutable model run.
///
/// This is deliberately narrower than a provider brand. A provider can expose more than one
/// dialect, while a continuation can only be replayed through the exact dialect that created it.
#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProviderProtocolDialect {
    #[serde(rename = "openai_chat_completions")]
    OpenAiChatCompletions,
    #[serde(rename = "anthropic_messages")]
    AnthropicMessages,
}

impl From<AgentApiStyle> for ProviderProtocolDialect {
    fn from(value: AgentApiStyle) -> Self {
        match value {
            AgentApiStyle::OpenAiCompatible => Self::OpenAiChatCompletions,
            AgentApiStyle::AnthropicCompatible => Self::AnthropicMessages,
        }
    }
}

impl ProviderProtocolDialect {
    /// Mirrors the existing Chat transport detection while making the resolved dialect available
    /// to Host code before an `AgentChatInput` enters runtime preparation.
    pub fn detect_from_api_url(api_url: &str) -> Self {
        let normalized = api_url.trim().to_ascii_lowercase();
        if normalized.contains("/chat/completions") {
            return Self::OpenAiChatCompletions;
        }
        if normalized.contains("anthropic") || normalized.ends_with("/messages") {
            return Self::AnthropicMessages;
        }
        Self::OpenAiChatCompletions
    }

    pub fn api_style(self) -> AgentApiStyle {
        match self {
            Self::OpenAiChatCompletions => AgentApiStyle::OpenAiCompatible,
            Self::AnthropicMessages => AgentApiStyle::AnthropicCompatible,
        }
    }
}

/// Stable identity of a code-owned provider protocol profile.
///
/// Unknown ids fail during Serde decoding. Known ids with unknown versions fail validation, so a
/// newer persisted wire contract is never silently interpreted with older replay semantics.
#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProviderProfileId {
    #[serde(rename = "generic_openai_chat")]
    GenericOpenAiChat,
    #[serde(rename = "generic_anthropic_messages")]
    GenericAnthropicMessages,
    #[serde(rename = "deepseek_v4_chat")]
    DeepSeekV4Chat,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderProfileRef {
    pub id: ProviderProfileId,
    pub version: u32,
}

impl ProviderProfileRef {
    pub const fn generic_for_dialect(dialect: ProviderProtocolDialect) -> Self {
        match dialect {
            ProviderProtocolDialect::OpenAiChatCompletions => Self {
                id: ProviderProfileId::GenericOpenAiChat,
                version: GENERIC_OPENAI_CHAT_PROFILE_VERSION,
            },
            ProviderProtocolDialect::AnthropicMessages => Self {
                id: ProviderProfileId::GenericAnthropicMessages,
                version: GENERIC_ANTHROPIC_MESSAGES_PROFILE_VERSION,
            },
        }
    }

    pub const fn deepseek_v4_chat() -> Self {
        Self {
            id: ProviderProfileId::DeepSeekV4Chat,
            version: DEEPSEEK_V4_CHAT_PROFILE_VERSION,
        }
    }

    pub fn validate(self) -> Result<(), ProviderProfileValidationError> {
        let expected = match self.id {
            ProviderProfileId::GenericOpenAiChat => GENERIC_OPENAI_CHAT_PROFILE_VERSION,
            ProviderProfileId::GenericAnthropicMessages => {
                GENERIC_ANTHROPIC_MESSAGES_PROFILE_VERSION
            }
            ProviderProfileId::DeepSeekV4Chat => DEEPSEEK_V4_CHAT_PROFILE_VERSION,
        };
        if self.version != expected {
            return Err(ProviderProfileValidationError::UnsupportedProfileVersion {
                id: self.id,
                version: self.version,
            });
        }
        Ok(())
    }

    pub fn validate_for_dialect(
        self,
        dialect: ProviderProtocolDialect,
    ) -> Result<(), ProviderProfileValidationError> {
        self.validate()?;
        let compatible = matches!(
            (self.id, dialect),
            (
                ProviderProfileId::GenericOpenAiChat | ProviderProfileId::DeepSeekV4Chat,
                ProviderProtocolDialect::OpenAiChatCompletions
            ) | (
                ProviderProfileId::GenericAnthropicMessages,
                ProviderProtocolDialect::AnthropicMessages
            )
        );
        if !compatible {
            return Err(ProviderProfileValidationError::IncompatibleDialect {
                id: self.id,
                dialect,
            });
        }
        Ok(())
    }
}

#[derive(Debug, Default, Deserialize, Serialize, Clone, Copy, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningMode {
    #[default]
    ProviderDefault,
    Enabled,
    Disabled,
}

#[derive(Debug, Default, Deserialize, Serialize, Clone, Copy, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningEffort {
    #[default]
    ProviderDefault,
    High,
    Max,
}

#[derive(Debug, Default, Deserialize, Serialize, Clone, Copy, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReasoningPolicy {
    #[serde(default)]
    pub mode: ReasoningMode,
    #[serde(default)]
    pub effort: ReasoningEffort,
}

impl ReasoningPolicy {
    pub const fn provider_default() -> Self {
        Self {
            mode: ReasoningMode::ProviderDefault,
            effort: ReasoningEffort::ProviderDefault,
        }
    }
}

/// Persisted, user-selectable configuration for a provider profile.
///
/// Generic profiles intentionally accept only provider-default reasoning settings in schema v1.
/// This preserves the existing wire payload until a provider-specific profile is selected.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderProfileConfig {
    pub schema_version: u32,
    pub profile: ProviderProfileRef,
    #[serde(default)]
    pub reasoning: ReasoningPolicy,
}

impl ProviderProfileConfig {
    pub const fn generic_for_dialect(dialect: ProviderProtocolDialect) -> Self {
        Self {
            schema_version: PROVIDER_PROFILE_CONFIG_SCHEMA_VERSION,
            profile: ProviderProfileRef::generic_for_dialect(dialect),
            reasoning: ReasoningPolicy::provider_default(),
        }
    }

    pub const fn deepseek_v4_default() -> Self {
        Self {
            schema_version: PROVIDER_PROFILE_CONFIG_SCHEMA_VERSION,
            profile: ProviderProfileRef::deepseek_v4_chat(),
            reasoning: ReasoningPolicy::provider_default(),
        }
    }

    pub fn validate(&self) -> Result<(), ProviderProfileValidationError> {
        if self.schema_version != PROVIDER_PROFILE_CONFIG_SCHEMA_VERSION {
            return Err(
                ProviderProfileValidationError::UnsupportedConfigurationVersion {
                    version: self.schema_version,
                },
            );
        }
        self.profile.validate()?;
        if matches!(
            self.profile.id,
            ProviderProfileId::GenericOpenAiChat | ProviderProfileId::GenericAnthropicMessages
        ) && self.reasoning != ReasoningPolicy::provider_default()
        {
            return Err(ProviderProfileValidationError::GenericReasoningOverride {
                id: self.profile.id,
            });
        }
        if self.reasoning.mode == ReasoningMode::Disabled
            && self.reasoning.effort != ReasoningEffort::ProviderDefault
        {
            return Err(ProviderProfileValidationError::EffortWhileReasoningDisabled);
        }
        Ok(())
    }

    pub fn validate_for_dialect(
        &self,
        dialect: ProviderProtocolDialect,
    ) -> Result<(), ProviderProfileValidationError> {
        self.validate()?;
        self.profile.validate_for_dialect(dialect)
    }

    pub fn resolve(
        explicit: Option<&Self>,
        dialect: ProviderProtocolDialect,
    ) -> Result<Self, ProviderProfileValidationError> {
        let resolved = explicit
            .cloned()
            .unwrap_or_else(|| Self::generic_for_dialect(dialect));
        resolved.validate_for_dialect(dialect)?;
        Ok(resolved)
    }
}

/// Immutable provenance key for provider-owned assistant protocol state.
///
/// The profile config is frozen separately because policy can change request construction while
/// this key deliberately contains only the identities needed to reject cross-provider replay.
#[derive(Deserialize, Serialize, Clone, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderProtocolKey {
    pub dialect: ProviderProtocolDialect,
    pub profile: ProviderProfileRef,
    pub model_id: String,
    /// Opaque revision of this model's complete effective wire protocol. The serialized field
    /// name is retained for checkpoint compatibility with the earlier broad settings revision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_configuration_revision: Option<String>,
}

impl std::fmt::Debug for ProviderProtocolKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ProviderProtocolKey([REDACTED])")
    }
}

impl ProviderProtocolKey {
    pub fn new(
        dialect: ProviderProtocolDialect,
        config: &ProviderProfileConfig,
        model_id: impl Into<String>,
        provider_configuration_revision: Option<String>,
    ) -> Result<Self, ProviderProfileValidationError> {
        let key = Self {
            dialect,
            profile: config.profile,
            model_id: model_id.into(),
            provider_configuration_revision,
        };
        key.validate_against_config(config)?;
        Ok(key)
    }

    pub fn validate(&self) -> Result<(), ProviderProfileValidationError> {
        self.profile.validate_for_dialect(self.dialect)?;
        if self.model_id.trim().is_empty() {
            return Err(ProviderProfileValidationError::EmptyModelId);
        }
        if self
            .provider_configuration_revision
            .as_deref()
            .is_some_and(|revision| revision.trim().is_empty())
        {
            return Err(ProviderProfileValidationError::EmptyConfigurationRevision);
        }
        Ok(())
    }

    pub fn validate_against_config(
        &self,
        config: &ProviderProfileConfig,
    ) -> Result<(), ProviderProfileValidationError> {
        self.validate()?;
        config.validate_for_dialect(self.dialect)?;
        if self.profile != config.profile {
            return Err(ProviderProfileValidationError::ProfileKeyMismatch);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderProfileValidationError {
    UnsupportedConfigurationVersion {
        version: u32,
    },
    UnsupportedProfileVersion {
        id: ProviderProfileId,
        version: u32,
    },
    IncompatibleDialect {
        id: ProviderProfileId,
        dialect: ProviderProtocolDialect,
    },
    GenericReasoningOverride {
        id: ProviderProfileId,
    },
    EffortWhileReasoningDisabled,
    EmptyModelId,
    EmptyConfigurationRevision,
    ProfileKeyMismatch,
}

impl Display for ProviderProfileValidationError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedConfigurationVersion { version } => {
                write!(
                    formatter,
                    "unsupported provider profile configuration version {version}"
                )
            }
            Self::UnsupportedProfileVersion { id, version } => {
                write!(
                    formatter,
                    "unsupported provider profile {id:?} version {version}"
                )
            }
            Self::IncompatibleDialect { id, dialect } => {
                write!(
                    formatter,
                    "provider profile {id:?} is incompatible with {dialect:?}"
                )
            }
            Self::GenericReasoningOverride { id } => write!(
                formatter,
                "generic provider profile {id:?} cannot override reasoning policy"
            ),
            Self::EffortWhileReasoningDisabled => formatter
                .write_str("reasoning effort must use provider_default when reasoning is disabled"),
            Self::EmptyModelId => formatter.write_str("provider protocol model id is empty"),
            Self::EmptyConfigurationRevision => {
                formatter.write_str("provider configuration revision is empty")
            }
            Self::ProfileKeyMismatch => {
                formatter.write_str("provider protocol key does not match profile configuration")
            }
        }
    }
}

impl Error for ProviderProfileValidationError {}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn missing_config_resolves_to_dialect_generic_without_wire_overrides() {
        let openai =
            ProviderProfileConfig::resolve(None, ProviderProtocolDialect::OpenAiChatCompletions)
                .unwrap();
        assert_eq!(openai.profile.id, ProviderProfileId::GenericOpenAiChat);
        assert_eq!(openai.reasoning, ReasoningPolicy::provider_default());

        let anthropic =
            ProviderProfileConfig::resolve(None, ProviderProtocolDialect::AnthropicMessages)
                .unwrap();
        assert_eq!(
            anthropic.profile.id,
            ProviderProfileId::GenericAnthropicMessages
        );
        assert_eq!(anthropic.reasoning, ReasoningPolicy::provider_default());
    }

    #[test]
    fn explicit_profile_with_unknown_version_fails_closed() {
        let config = ProviderProfileConfig {
            schema_version: PROVIDER_PROFILE_CONFIG_SCHEMA_VERSION,
            profile: ProviderProfileRef {
                id: ProviderProfileId::DeepSeekV4Chat,
                version: 99,
            },
            reasoning: ReasoningPolicy::provider_default(),
        };
        assert!(matches!(
            config.validate(),
            Err(ProviderProfileValidationError::UnsupportedProfileVersion { .. })
        ));
    }

    #[test]
    fn explicit_configuration_with_unknown_schema_fails_closed() {
        let mut config = ProviderProfileConfig::deepseek_v4_default();
        config.schema_version = PROVIDER_PROFILE_CONFIG_SCHEMA_VERSION + 1;
        assert!(matches!(
            config.validate(),
            Err(ProviderProfileValidationError::UnsupportedConfigurationVersion { .. })
        ));
    }

    #[test]
    fn explicit_profile_with_unknown_id_fails_during_decode() {
        let decoded = serde_json::from_value::<ProviderProfileConfig>(json!({
            "schemaVersion": 1,
            "profile": { "id": "future_profile", "version": 1 },
            "reasoning": { "mode": "provider_default", "effort": "provider_default" }
        }));
        assert!(decoded.is_err());
    }

    #[test]
    fn incompatible_profile_and_dialect_fail_closed() {
        let config = ProviderProfileConfig::deepseek_v4_default();
        assert!(matches!(
            config.validate_for_dialect(ProviderProtocolDialect::AnthropicMessages),
            Err(ProviderProfileValidationError::IncompatibleDialect { .. })
        ));
    }

    #[test]
    fn generic_profiles_reject_reasoning_wire_overrides() {
        let mut config = ProviderProfileConfig::generic_for_dialect(
            ProviderProtocolDialect::OpenAiChatCompletions,
        );
        config.reasoning.mode = ReasoningMode::Enabled;
        assert!(matches!(
            config.validate(),
            Err(ProviderProfileValidationError::GenericReasoningOverride { .. })
        ));
    }

    #[test]
    fn protocol_key_binds_profile_dialect_model_and_revision() {
        let config = ProviderProfileConfig::deepseek_v4_default();
        let key = ProviderProtocolKey::new(
            ProviderProtocolDialect::OpenAiChatCompletions,
            &config,
            "deepseek-v4-pro",
            Some("model-settings-v1:test".to_string()),
        )
        .unwrap();
        assert_eq!(key.profile, config.profile);
        assert_eq!(key.model_id, "deepseek-v4-pro");
        assert!(key.validate_against_config(&config).is_ok());
    }

    #[test]
    fn protocol_key_debug_redacts_model_and_configuration_revision() {
        let config = ProviderProfileConfig::deepseek_v4_default();
        let model_canary = "debug-model-canary";
        let revision_canary = "model-settings-v1:debug-revision-canary";
        let key = ProviderProtocolKey::new(
            ProviderProtocolDialect::OpenAiChatCompletions,
            &config,
            model_canary,
            Some(revision_canary.to_string()),
        )
        .unwrap();

        let rendered = format!("{key:?}");
        assert_eq!(rendered, "ProviderProtocolKey([REDACTED])");
        assert!(!rendered.contains(model_canary));
        assert!(!rendered.contains(revision_canary));
    }
}
