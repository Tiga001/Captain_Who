use crate::protocol::AgentApiStyle;
use serde::de::Error as DeserializeError;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::error::Error;
use std::fmt::{Display, Formatter};

pub const PROVIDER_PROFILE_CONFIG_SCHEMA_VERSION: u32 = 1;
pub const PROVIDER_PROFILE_CONFIG_V2_SCHEMA_VERSION: u32 = 2;
pub const GENERIC_OPENAI_CHAT_PROFILE_VERSION: u32 = 1;
pub const GENERIC_ANTHROPIC_MESSAGES_PROFILE_VERSION: u32 = 1;
pub const DEEPSEEK_V4_CHAT_PROFILE_VERSION: u32 = 1;
pub const DEEPSEEK_V4_VISION_PROFILE_VERSION: u32 = 1;
pub const MOONSHOT_K3_CHAT_PROFILE_VERSION: u32 = 1;
pub const MOONSHOT_K2_7_CODE_CHAT_PROFILE_VERSION: u32 = 1;
pub const MOONSHOT_K2_6_CHAT_PROFILE_VERSION: u32 = 1;

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

macro_rules! bounded_provider_identity {
    (
        $(#[$meta:meta])*
        $name:ident,
        max_bytes = $max_bytes:expr,
        label = $label:literal,
        constants = { $($constant:ident => $value:literal),+ $(,)? }
    ) => {
        $(#[$meta])*
        #[derive(Clone, Copy, PartialEq, Eq, Hash)]
        pub struct $name {
            bytes: [u8; Self::MAX_BYTES],
            len: u8,
        }

        impl $name {
            pub const MAX_BYTES: usize = $max_bytes;

            $(
                #[allow(non_upper_case_globals)]
                pub const $constant: Self = Self::from_static($value);
            )+

            const fn from_static(value: &str) -> Self {
                let source = value.as_bytes();
                assert!(!source.is_empty() && source.len() <= Self::MAX_BYTES);
                let mut bytes = [0_u8; Self::MAX_BYTES];
                let mut index = 0;
                while index < source.len() {
                    bytes[index] = source[index];
                    index += 1;
                }
                Self {
                    bytes,
                    len: source.len() as u8,
                }
            }

            pub fn parse(value: &str) -> Result<Self, &'static str> {
                if value.is_empty() {
                    return Err(concat!($label, " is empty"));
                }
                if value.len() > Self::MAX_BYTES {
                    return Err(concat!($label, " is too long"));
                }
                if !value.bytes().all(|byte| {
                    byte.is_ascii_lowercase()
                        || byte.is_ascii_digit()
                        || b"._-".contains(&byte)
                }) {
                    return Err(concat!($label, " contains unsupported characters"));
                }
                let mut bytes = [0_u8; Self::MAX_BYTES];
                bytes[..value.len()].copy_from_slice(value.as_bytes());
                Ok(Self {
                    bytes,
                    len: value.len() as u8,
                })
            }

            pub fn as_str(&self) -> &str {
                std::str::from_utf8(&self.bytes[..self.len as usize])
                    .expect(concat!($label, " is constructed from UTF-8"))
            }
        }

        impl std::fmt::Debug for $name {
            fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
                formatter.write_str(self.as_str())
            }
        }

        impl Display for $name {
            fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
                formatter.write_str(self.as_str())
            }
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_str(self.as_str())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                Self::parse(&value).map_err(D::Error::custom)
            }
        }
    };
}

bounded_provider_identity!(
    /// Stable, public identity of a provider vendor.
    ///
    /// The representation is intentionally open so a newer Host can safely project an unknown
    /// vendor to an older client without granting it a registered runtime capability.
    ProviderVendorId,
    max_bytes = 32,
    label = "provider vendor id",
    constants = {
        Generic => "generic",
        DeepSeek => "deepseek",
        Moonshot => "moonshot",
    }
);

bounded_provider_identity!(
    /// Stable, safe identity of a model family selected by the Host resolver.
    ProviderModelFamilyId,
    max_bytes = 64,
    label = "provider model family id",
    constants = {
        GenericOpenAiChat => "generic_openai_chat",
        GenericAnthropicMessages => "generic_anthropic_messages",
        DeepSeekV4Chat => "deepseek_v4_chat",
        DeepSeekV4Vision => "deepseek_v4_vision",
        MoonshotK3Chat => "moonshot_k3_chat",
        MoonshotK27CodeChat => "moonshot_k2_7_code_chat",
        MoonshotK26Chat => "moonshot_k2_6_chat",
    }
);

/// Stable identity of a code-owned provider protocol profile.
///
/// Persisted ids are deliberately lossless so settings created by a newer binary can still be
/// shown as unsupported and replaced explicitly by an older UI. Runtime authority remains the
/// exact code-owned Registration lookup: an unknown id or version can be represented, but can
/// never acquire an Adapter or Runtime Capability.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProviderProfileId {
    bytes: [u8; Self::MAX_BYTES],
    len: u8,
}

impl ProviderProfileId {
    pub const MAX_BYTES: usize = 64;

    #[allow(non_upper_case_globals)]
    pub const GenericOpenAiChat: Self = Self::from_static("generic_openai_chat");
    #[allow(non_upper_case_globals)]
    pub const GenericAnthropicMessages: Self = Self::from_static("generic_anthropic_messages");
    #[allow(non_upper_case_globals)]
    pub const DeepSeekV4Chat: Self = Self::from_static("deepseek_v4_chat");
    #[allow(non_upper_case_globals)]
    pub const DeepSeekV4Vision: Self = Self::from_static("deepseek_v4_vision");
    #[allow(non_upper_case_globals)]
    pub const MoonshotK3Chat: Self = Self::from_static("moonshot_k3_chat");
    #[allow(non_upper_case_globals)]
    pub const MoonshotK27CodeChat: Self = Self::from_static("moonshot_k2_7_code_chat");
    #[allow(non_upper_case_globals)]
    pub const MoonshotK26Chat: Self = Self::from_static("moonshot_k2_6_chat");

    const fn from_static(value: &str) -> Self {
        let source = value.as_bytes();
        assert!(!source.is_empty() && source.len() <= Self::MAX_BYTES);
        let mut bytes = [0_u8; Self::MAX_BYTES];
        let mut index = 0;
        while index < source.len() {
            bytes[index] = source[index];
            index += 1;
        }
        Self {
            bytes,
            len: source.len() as u8,
        }
    }

    pub fn parse(value: &str) -> Result<Self, &'static str> {
        if value.is_empty() {
            return Err("provider profile id is empty");
        }
        if value.len() > Self::MAX_BYTES {
            return Err("provider profile id is too long");
        }
        if !value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._-".contains(&byte)
        }) {
            return Err("provider profile id contains unsupported characters");
        }
        let mut bytes = [0_u8; Self::MAX_BYTES];
        bytes[..value.len()].copy_from_slice(value.as_bytes());
        Ok(Self {
            bytes,
            len: value.len() as u8,
        })
    }

    pub fn as_str(&self) -> &str {
        std::str::from_utf8(&self.bytes[..self.len as usize])
            .expect("ProviderProfileId is constructed from UTF-8")
    }
}

impl std::fmt::Debug for ProviderProfileId {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl Display for ProviderProfileId {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl Serialize for ProviderProfileId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ProviderProfileId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).map_err(D::Error::custom)
    }
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

    pub const fn deepseek_v4_vision() -> Self {
        Self {
            id: ProviderProfileId::DeepSeekV4Vision,
            version: DEEPSEEK_V4_VISION_PROFILE_VERSION,
        }
    }

    pub const fn moonshot_k3_chat() -> Self {
        Self {
            id: ProviderProfileId::MoonshotK3Chat,
            version: MOONSHOT_K3_CHAT_PROFILE_VERSION,
        }
    }

    pub const fn moonshot_k2_7_code_chat() -> Self {
        Self {
            id: ProviderProfileId::MoonshotK27CodeChat,
            version: MOONSHOT_K2_7_CODE_CHAT_PROFILE_VERSION,
        }
    }

    pub const fn moonshot_k2_6_chat() -> Self {
        Self {
            id: ProviderProfileId::MoonshotK26Chat,
            version: MOONSHOT_K2_6_CHAT_PROFILE_VERSION,
        }
    }

    pub fn validate(self) -> Result<(), ProviderProfileValidationError> {
        if !crate::provider_registration::is_registered_provider_profile(self) {
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
        crate::provider_registration::resolve_provider_registration(self, dialect).map(|_| ())
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

/// Provider-family reasoning effort used by current vendor Profiles.
///
/// This is deliberately separate from [`ReasoningEffort`]. The latter is part of the legacy v1
/// Profile and Agent snapshot contract, whose persisted schema does not accept `low`.
#[derive(Debug, Default, Deserialize, Serialize, Clone, Copy, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ProviderReasoningEffort {
    #[default]
    ProviderDefault,
    Low,
    High,
    Max,
}

#[derive(Debug, Default, Deserialize, Serialize, Clone, Copy, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReasoningPolicy {
    pub mode: ReasoningMode,
    pub effort: ReasoningEffort,
}

#[derive(Debug, Default, Deserialize, Serialize, Clone, Copy, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderFamilyReasoningPolicy {
    pub mode: ReasoningMode,
    pub effort: ProviderReasoningEffort,
}

impl ProviderFamilyReasoningPolicy {
    pub const fn provider_default() -> Self {
        Self {
            mode: ReasoningMode::ProviderDefault,
            effort: ProviderReasoningEffort::ProviderDefault,
        }
    }
}

#[derive(Debug, Default, Deserialize, Serialize, Clone, Copy, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum MoonshotK26ThinkingMode {
    #[default]
    ProviderDefault,
    Enabled,
    Disabled,
    EnabledKeepAll,
}

/// Strong, family-specific public settings persisted by Profile configuration schema v2.
///
/// The family discriminator is intentionally explicit. It prevents legal settings for one
/// provider model family from being combined into an invalid configuration for another family.
#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq, Hash)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ProviderFamilySettings {
    Generic,
    DeepseekV4Chat {
        reasoning: ProviderFamilyReasoningPolicy,
    },
    DeepseekV4Vision {
        reasoning: ProviderFamilyReasoningPolicy,
    },
    MoonshotK3Chat {
        reasoning_effort: ProviderReasoningEffort,
    },
    #[serde(rename = "moonshot_k2_7_code_chat")]
    MoonshotK27CodeChat,
    #[serde(rename = "moonshot_k2_6_chat")]
    MoonshotK26Chat {
        thinking_mode: MoonshotK26ThinkingMode,
    },
}

/// Renderer-editable vendor settings. Runtime identities and capabilities remain Host-owned.
pub type ProviderVendorPublicSettings = ProviderFamilySettings;

impl ProviderFamilySettings {
    pub const fn normalized(self) -> Self {
        match self {
            Self::DeepseekV4Chat { mut reasoning } => {
                if matches!(reasoning.mode, ReasoningMode::Disabled) {
                    reasoning.effort = ProviderReasoningEffort::ProviderDefault;
                }
                Self::DeepseekV4Chat { reasoning }
            }
            Self::DeepseekV4Vision { mut reasoning } => {
                if matches!(reasoning.mode, ReasoningMode::Disabled) {
                    reasoning.effort = ProviderReasoningEffort::ProviderDefault;
                }
                Self::DeepseekV4Vision { reasoning }
            }
            settings => settings,
        }
    }

    pub const fn reasoning_mode(self) -> ReasoningMode {
        match self {
            Self::Generic => ReasoningMode::ProviderDefault,
            Self::DeepseekV4Chat { reasoning } | Self::DeepseekV4Vision { reasoning } => {
                reasoning.mode
            }
            Self::MoonshotK3Chat { .. } | Self::MoonshotK27CodeChat => ReasoningMode::Enabled,
            Self::MoonshotK26Chat { thinking_mode } => match thinking_mode {
                MoonshotK26ThinkingMode::ProviderDefault => ReasoningMode::ProviderDefault,
                MoonshotK26ThinkingMode::Enabled | MoonshotK26ThinkingMode::EnabledKeepAll => {
                    ReasoningMode::Enabled
                }
                MoonshotK26ThinkingMode::Disabled => ReasoningMode::Disabled,
            },
        }
    }

    pub const fn reasoning_effort(self) -> ProviderReasoningEffort {
        match self {
            Self::DeepseekV4Chat { reasoning } | Self::DeepseekV4Vision { reasoning } => {
                reasoning.effort
            }
            Self::MoonshotK3Chat { reasoning_effort } => reasoning_effort,
            Self::Generic | Self::MoonshotK27CodeChat | Self::MoonshotK26Chat { .. } => {
                ProviderReasoningEffort::ProviderDefault
            }
        }
    }

    fn validate(self) -> Result<(), ProviderProfileValidationError> {
        match self {
            Self::Generic => Ok(()),
            Self::DeepseekV4Chat { reasoning } | Self::DeepseekV4Vision { reasoning }
                if reasoning.mode == ReasoningMode::Disabled
                    && reasoning.effort != ProviderReasoningEffort::ProviderDefault =>
            {
                Err(ProviderProfileValidationError::EffortWhileReasoningDisabled)
            }
            Self::DeepseekV4Chat { .. }
            | Self::DeepseekV4Vision { .. }
            | Self::MoonshotK3Chat { .. }
            | Self::MoonshotK27CodeChat
            | Self::MoonshotK26Chat { .. } => Ok(()),
        }
    }
}

/// Strict, user-editable settings accepted by the Host Profile-selection boundary.
///
/// This union is intentionally separate from persisted [`ProviderProfileConfig`]. Clients choose
/// a registered profile id and its public settings; the Host supplies the current registered
/// version and every private runtime policy.
#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq, Hash)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProviderProfilePublicSettings {
    DeepseekV4Chat { reasoning: ReasoningPolicy },
}

impl ProviderProfilePublicSettings {
    pub const fn normalized(self) -> Self {
        match self {
            Self::DeepseekV4Chat { mut reasoning } => {
                if matches!(reasoning.mode, ReasoningMode::Disabled) {
                    reasoning.effort = ReasoningEffort::ProviderDefault;
                }
                Self::DeepseekV4Chat { reasoning }
            }
        }
    }

    pub const fn reasoning(self) -> ReasoningPolicy {
        match self {
            Self::DeepseekV4Chat { reasoning } => reasoning,
        }
    }
}

impl ReasoningPolicy {
    pub const fn provider_default() -> Self {
        Self {
            mode: ReasoningMode::ProviderDefault,
            effort: ReasoningEffort::ProviderDefault,
        }
    }
}

/// Exact legacy wire shape. Existing settings remain schema v1 until an explicit vendor update.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderProfileConfigV1 {
    pub schema_version: u32,
    pub profile: ProviderProfileRef,
    pub reasoning: ReasoningPolicy,
}

/// Strong family-specific Profile configuration written by the vendor selection boundary.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderProfileConfigV2 {
    pub schema_version: u32,
    pub profile: ProviderProfileRef,
    pub vendor_id: ProviderVendorId,
    pub settings: ProviderFamilySettings,
}

/// Persisted, user-selectable configuration for a provider profile.
///
/// Untagged decoding preserves the exact v1 JSON contract while allowing new vendor selections to
/// use a strong, family-tagged v2 settings union. No load path upgrades or rewrites v1 values.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq, Hash)]
#[serde(untagged)]
pub enum ProviderProfileConfig {
    V1(ProviderProfileConfigV1),
    V2(ProviderProfileConfigV2),
}

impl ProviderProfileConfig {
    pub const fn generic_for_dialect(dialect: ProviderProtocolDialect) -> Self {
        Self::V1(ProviderProfileConfigV1 {
            schema_version: PROVIDER_PROFILE_CONFIG_SCHEMA_VERSION,
            profile: ProviderProfileRef::generic_for_dialect(dialect),
            reasoning: ReasoningPolicy::provider_default(),
        })
    }

    pub const fn deepseek_v4_default() -> Self {
        Self::V1(ProviderProfileConfigV1 {
            schema_version: PROVIDER_PROFILE_CONFIG_SCHEMA_VERSION,
            profile: ProviderProfileRef::deepseek_v4_chat(),
            reasoning: ReasoningPolicy::provider_default(),
        })
    }

    pub const fn from_family_settings(
        profile: ProviderProfileRef,
        vendor_id: ProviderVendorId,
        settings: ProviderFamilySettings,
    ) -> Self {
        Self::V2(ProviderProfileConfigV2 {
            schema_version: PROVIDER_PROFILE_CONFIG_V2_SCHEMA_VERSION,
            profile,
            vendor_id,
            settings: settings.normalized(),
        })
    }

    pub const fn schema_version(&self) -> u32 {
        match self {
            Self::V1(config) => config.schema_version,
            Self::V2(config) => config.schema_version,
        }
    }

    pub const fn profile(&self) -> ProviderProfileRef {
        match self {
            Self::V1(config) => config.profile,
            Self::V2(config) => config.profile,
        }
    }

    pub const fn vendor_id(&self) -> Option<ProviderVendorId> {
        match self {
            Self::V1(_) => None,
            Self::V2(config) => Some(config.vendor_id),
        }
    }

    pub const fn family_settings(&self) -> Option<&ProviderFamilySettings> {
        match self {
            Self::V1(_) => None,
            Self::V2(config) => Some(&config.settings),
        }
    }

    pub const fn legacy_reasoning(&self) -> Option<ReasoningPolicy> {
        match self {
            Self::V1(config) => Some(config.reasoning),
            Self::V2(_) => None,
        }
    }

    pub const fn reasoning_mode(&self) -> ReasoningMode {
        match self {
            Self::V1(config) => config.reasoning.mode,
            Self::V2(config) => config.settings.reasoning_mode(),
        }
    }

    pub const fn provider_reasoning_effort(&self) -> ProviderReasoningEffort {
        match self {
            Self::V1(config) => match config.reasoning.effort {
                ReasoningEffort::ProviderDefault => ProviderReasoningEffort::ProviderDefault,
                ReasoningEffort::High => ProviderReasoningEffort::High,
                ReasoningEffort::Max => ProviderReasoningEffort::Max,
            },
            Self::V2(config) => config.settings.reasoning_effort(),
        }
    }

    pub fn validate(&self) -> Result<(), ProviderProfileValidationError> {
        match self {
            Self::V1(config) => {
                if config.schema_version != PROVIDER_PROFILE_CONFIG_SCHEMA_VERSION {
                    return Err(
                        ProviderProfileValidationError::UnsupportedConfigurationVersion {
                            version: config.schema_version,
                        },
                    );
                }
                config.profile.validate()?;
                if matches!(
                    config.profile.id,
                    ProviderProfileId::GenericOpenAiChat
                        | ProviderProfileId::GenericAnthropicMessages
                ) && config.reasoning != ReasoningPolicy::provider_default()
                {
                    return Err(ProviderProfileValidationError::GenericReasoningOverride {
                        id: config.profile.id,
                    });
                }
                if config.reasoning.mode == ReasoningMode::Disabled
                    && config.reasoning.effort != ReasoningEffort::ProviderDefault
                {
                    return Err(ProviderProfileValidationError::EffortWhileReasoningDisabled);
                }
            }
            Self::V2(config) => {
                if config.schema_version != PROVIDER_PROFILE_CONFIG_V2_SCHEMA_VERSION {
                    return Err(
                        ProviderProfileValidationError::UnsupportedConfigurationVersion {
                            version: config.schema_version,
                        },
                    );
                }
                config.profile.validate()?;
                config.settings.validate()?;
            }
        }
        Ok(())
    }

    pub fn validate_for_dialect(
        &self,
        dialect: ProviderProtocolDialect,
    ) -> Result<(), ProviderProfileValidationError> {
        self.validate()?;
        let registration =
            crate::provider_registration::resolve_provider_registration(self.profile(), dialect)?;
        registration.validate_profile_config(self)
    }

    pub fn validate_for_model(
        &self,
        model_id: &str,
        dialect: ProviderProtocolDialect,
    ) -> Result<(), ProviderProfileValidationError> {
        self.validate()?;
        let registration =
            crate::provider_registration::resolve_provider_registration(self.profile(), dialect)?;
        registration.validate_profile_config(self)?;
        if matches!(self, Self::V2(_)) {
            registration.validate_model_id(model_id)?;
        }
        Ok(())
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
    /// Opaque `provider-protocol-v1` revision of this model's complete effective wire protocol.
    #[serde(deserialize_with = "crate::protocol::deserialize_required_nullable")]
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
            profile: config.profile(),
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
        config.validate_for_model(&self.model_id, self.dialect)?;
        if self.profile != config.profile() {
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
    ProfileIsNotUiSelectable {
        id: ProviderProfileId,
    },
    PublicSettingsKindMismatch {
        id: ProviderProfileId,
    },
    VendorMismatch {
        id: ProviderProfileId,
        vendor_id: ProviderVendorId,
    },
    FamilySettingsKindMismatch {
        id: ProviderProfileId,
    },
    UnsupportedModelForProfile {
        id: ProviderProfileId,
    },
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
            Self::ProfileIsNotUiSelectable { id } => {
                write!(formatter, "provider profile {id} is not selectable")
            }
            Self::PublicSettingsKindMismatch { id } => {
                write!(formatter, "provider settings do not match profile {id}")
            }
            Self::VendorMismatch { id, vendor_id } => write!(
                formatter,
                "provider vendor {vendor_id} does not match profile {id}"
            ),
            Self::FamilySettingsKindMismatch { id } => {
                write!(
                    formatter,
                    "provider family settings do not match profile {id}"
                )
            }
            Self::UnsupportedModelForProfile { id } => {
                write!(formatter, "model id is not supported by profile {id}")
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
    fn explicit_generic_profiles_match_their_dialect_without_wire_overrides() {
        let openai = ProviderProfileConfig::generic_for_dialect(
            ProviderProtocolDialect::OpenAiChatCompletions,
        );
        openai
            .validate_for_dialect(ProviderProtocolDialect::OpenAiChatCompletions)
            .unwrap();
        assert_eq!(openai.profile().id, ProviderProfileId::GenericOpenAiChat);
        assert_eq!(
            openai.legacy_reasoning(),
            Some(ReasoningPolicy::provider_default())
        );

        let anthropic =
            ProviderProfileConfig::generic_for_dialect(ProviderProtocolDialect::AnthropicMessages);
        anthropic
            .validate_for_dialect(ProviderProtocolDialect::AnthropicMessages)
            .unwrap();
        assert_eq!(
            anthropic.profile().id,
            ProviderProfileId::GenericAnthropicMessages
        );
        assert_eq!(
            anthropic.legacy_reasoning(),
            Some(ReasoningPolicy::provider_default())
        );
    }

    #[test]
    fn explicit_profile_with_unknown_version_fails_closed() {
        let config = ProviderProfileConfig::V1(ProviderProfileConfigV1 {
            schema_version: PROVIDER_PROFILE_CONFIG_SCHEMA_VERSION,
            profile: ProviderProfileRef {
                id: ProviderProfileId::DeepSeekV4Chat,
                version: 99,
            },
            reasoning: ReasoningPolicy::provider_default(),
        });
        assert!(matches!(
            config.validate(),
            Err(ProviderProfileValidationError::UnsupportedProfileVersion { .. })
        ));
    }

    #[test]
    fn explicit_configuration_with_unknown_schema_fails_closed() {
        let mut config = ProviderProfileConfig::deepseek_v4_default();
        let ProviderProfileConfig::V1(v1) = &mut config else {
            panic!("legacy constructor must produce v1");
        };
        v1.schema_version = PROVIDER_PROFILE_CONFIG_SCHEMA_VERSION + 1;
        assert!(matches!(
            config.validate(),
            Err(ProviderProfileValidationError::UnsupportedConfigurationVersion { .. })
        ));
    }

    #[test]
    fn explicit_profile_with_unknown_id_is_preserved_but_fails_registration_validation() {
        let decoded = serde_json::from_value::<ProviderProfileConfig>(json!({
            "schemaVersion": 1,
            "profile": { "id": "future_profile", "version": 1 },
            "reasoning": { "mode": "provider_default", "effort": "provider_default" }
        }))
        .unwrap();
        assert_eq!(decoded.profile().id.as_str(), "future_profile");
        assert!(matches!(
            decoded.validate(),
            Err(ProviderProfileValidationError::UnsupportedProfileVersion { .. })
        ));
    }

    #[test]
    fn current_profile_configuration_requires_complete_reasoning_and_rejects_extra_fields() {
        let current = json!({
            "schemaVersion": 1,
            "profile": { "id": "generic_openai_chat", "version": 1 },
            "reasoning": { "mode": "provider_default", "effort": "provider_default" }
        });
        assert!(serde_json::from_value::<ProviderProfileConfig>(current.clone()).is_ok());

        for path in ["reasoning", "mode", "effort"] {
            let mut incomplete = current.clone();
            if path == "reasoning" {
                incomplete.as_object_mut().unwrap().remove(path);
            } else {
                incomplete["reasoning"]
                    .as_object_mut()
                    .unwrap()
                    .remove(path);
            }
            assert!(serde_json::from_value::<ProviderProfileConfig>(incomplete).is_err());
        }

        let mut extra = current;
        extra["reasoning"]["runtimeCapabilities"] = json!({ "toolReplay": true });
        assert!(serde_json::from_value::<ProviderProfileConfig>(extra).is_err());
    }

    #[test]
    fn v1_round_trip_remains_byte_shape_compatible_and_v2_is_family_tagged() {
        let v1 = ProviderProfileConfig::deepseek_v4_default();
        assert_eq!(
            serde_json::to_value(&v1).unwrap(),
            json!({
                "schemaVersion": 1,
                "profile": {"id": "deepseek_v4_chat", "version": 1},
                "reasoning": {"mode": "provider_default", "effort": "provider_default"}
            })
        );

        let v2 = ProviderProfileConfig::from_family_settings(
            ProviderProfileRef::moonshot_k3_chat(),
            ProviderVendorId::Moonshot,
            ProviderFamilySettings::MoonshotK3Chat {
                reasoning_effort: ProviderReasoningEffort::Low,
            },
        );
        let wire = serde_json::to_value(&v2).unwrap();
        assert_eq!(
            wire,
            json!({
                "schemaVersion": 2,
                "profile": {"id": "moonshot_k3_chat", "version": 1},
                "vendorId": "moonshot",
                "settings": {"kind": "moonshot_k3_chat", "reasoningEffort": "low"}
            })
        );
        assert_eq!(
            serde_json::from_value::<ProviderProfileConfig>(wire).unwrap(),
            v2
        );
    }

    #[test]
    fn v2_family_settings_reject_illegal_deepseek_disabled_effort() {
        let config = ProviderProfileConfig::V2(ProviderProfileConfigV2 {
            schema_version: PROVIDER_PROFILE_CONFIG_V2_SCHEMA_VERSION,
            profile: ProviderProfileRef::deepseek_v4_chat(),
            vendor_id: ProviderVendorId::DeepSeek,
            settings: ProviderFamilySettings::DeepseekV4Chat {
                reasoning: ProviderFamilyReasoningPolicy {
                    mode: ReasoningMode::Disabled,
                    effort: ProviderReasoningEffort::Low,
                },
            },
        });
        assert_eq!(
            config.validate(),
            Err(ProviderProfileValidationError::EffortWhileReasoningDisabled)
        );
    }

    #[test]
    fn moonshot_k2_family_tags_keep_the_public_versioned_names() {
        let k2_7 = serde_json::to_value(ProviderFamilySettings::MoonshotK27CodeChat).unwrap();
        assert_eq!(k2_7, json!({"kind": "moonshot_k2_7_code_chat"}));

        let k2_6 = serde_json::to_value(ProviderFamilySettings::MoonshotK26Chat {
            thinking_mode: MoonshotK26ThinkingMode::EnabledKeepAll,
        })
        .unwrap();
        assert_eq!(
            k2_6,
            json!({
                "kind": "moonshot_k2_6_chat",
                "thinkingMode": "enabled_keep_all"
            })
        );
        assert_eq!(
            serde_json::from_value::<ProviderFamilySettings>(k2_7).unwrap(),
            ProviderFamilySettings::MoonshotK27CodeChat
        );
        assert_eq!(
            serde_json::from_value::<ProviderFamilySettings>(k2_6).unwrap(),
            ProviderFamilySettings::MoonshotK26Chat {
                thinking_mode: MoonshotK26ThinkingMode::EnabledKeepAll,
            }
        );
    }

    #[test]
    fn profile_id_rejects_control_characters_and_oversized_values() {
        assert!(ProviderProfileId::parse("future_profile-v2.1").is_ok());
        assert!(ProviderProfileId::parse("future\nprofile").is_err());
        assert!(ProviderProfileId::parse("future profile").is_err());
        assert!(ProviderProfileId::parse(&"a".repeat(ProviderProfileId::MAX_BYTES + 1)).is_err());
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
        let ProviderProfileConfig::V1(v1) = &mut config else {
            panic!("legacy constructor must produce v1");
        };
        v1.reasoning.mode = ReasoningMode::Enabled;
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
            Some("provider-protocol-v1:test".to_string()),
        )
        .unwrap();
        assert_eq!(key.profile, config.profile());
        assert_eq!(key.model_id, "deepseek-v4-pro");
        assert!(key.validate_against_config(&config).is_ok());
    }

    #[test]
    fn protocol_key_debug_redacts_model_and_configuration_revision() {
        let config = ProviderProfileConfig::deepseek_v4_default();
        let model_canary = "debug-model-canary";
        let revision_canary = "provider-protocol-v1:debug-revision-canary";
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
