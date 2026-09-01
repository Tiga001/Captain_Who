//! Moonshot Profile registrations and model-family policy.
//!
//! Its preserved-thinking capability declaration is local so future protocol changes stay local.

use super::{
    matches_official_https_endpoint, no_official_profile_normalization, ProviderAdapterKind,
    ProviderCheckpointPrivateArgumentsSemantics, ProviderContextProjectionSemantics,
    ProviderFamilySettings, ProviderFamilySettingsDescriptor, ProviderImageInputPolicy,
    ProviderModelFamilyId, ProviderModelIdPolicy, ProviderPartialTraceSemantics,
    ProviderPrivateReplaySemantics, ProviderProfileId, ProviderProfileRef,
    ProviderProfileSettingsKind, ProviderProtocolDialect, ProviderRegistration,
    ProviderRuntimeCapabilities, ProviderTerminalBatchSemantics, ProviderToolCallSourceSemantics,
    ProviderToolExchangeSemantics, ProviderUsageSemantics, ProviderVendorDescriptor,
    ProviderVendorId, ProviderVendorSettingsKind,
};
use crate::provider_profile::{
    MoonshotK26ThinkingMode, ProviderProfileConfig, ProviderReasoningEffort, ReasoningMode,
    MOONSHOT_K2_6_CHAT_PROFILE_VERSION, MOONSHOT_K2_7_CODE_CHAT_PROFILE_VERSION,
    MOONSHOT_K3_CHAT_PROFILE_VERSION,
};

const K3_MODEL_IDS: &[&str] = &["kimi-k3"];
const K2_7_CODE_MODEL_IDS: &[&str] = &["kimi-k2.7-code", "kimi-k2.7-code-highspeed"];
const K2_6_MODEL_IDS: &[&str] = &["kimi-k2.6"];
const OFFICIAL_HOSTS: &[&str] = &["api.moonshot.ai", "api.moonshot.cn"];
const OFFICIAL_CHAT_PATHS: &[&str] = &["/v1/chat/completions"];

fn normalize_official_k3(
    api_url: &str,
    model_id: &str,
    current: &ProviderProfileConfig,
) -> Option<ProviderProfileConfig> {
    if model_id != "kimi-k3"
        || !matches_official_https_endpoint(api_url, OFFICIAL_HOSTS, OFFICIAL_CHAT_PATHS)
    {
        return None;
    }
    let reasoning_effort = match current {
        ProviderProfileConfig::V2(config) if config.vendor_id == ProviderVendorId::Moonshot => {
            match config.settings {
                ProviderFamilySettings::MoonshotK3Chat { reasoning_effort } => reasoning_effort,
                _ => ProviderReasoningEffort::Max,
            }
        }
        _ if current.reasoning_mode() == ReasoningMode::Enabled => {
            match current.provider_reasoning_effort() {
                ProviderReasoningEffort::Low => ProviderReasoningEffort::Low,
                ProviderReasoningEffort::High => ProviderReasoningEffort::High,
                ProviderReasoningEffort::Max => ProviderReasoningEffort::Max,
                ProviderReasoningEffort::ProviderDefault => ProviderReasoningEffort::Max,
            }
        }
        _ => ProviderReasoningEffort::Max,
    };
    Some(ProviderProfileConfig::from_family_settings(
        ProviderProfileRef::moonshot_k3_chat(),
        ProviderVendorId::Moonshot,
        ProviderFamilySettings::MoonshotK3Chat { reasoning_effort },
    ))
}

const fn runtime_capabilities() -> ProviderRuntimeCapabilities {
    ProviderRuntimeCapabilities {
        tool_exchange: ProviderToolExchangeSemantics::ExactProviderGrouped,
        private_replay: ProviderPrivateReplaySemantics::AdapterClassified,
        context_projection: ProviderContextProjectionSemantics::ExactProviderTurn,
        // Moonshot usage is projected by its own adapter from provider-authoritative fields.
        usage: ProviderUsageSemantics::StandardAdditive,
        partial_trace: ProviderPartialTraceSemantics::DeferUntilProviderTurnClosed,
        tool_call_source: ProviderToolCallSourceSemantics::ProviderNativeOnly,
        terminal_batch: ProviderTerminalBatchSemantics::CloseWholeProviderTurn,
        checkpoint_private_arguments:
            ProviderCheckpointPrivateArgumentsSemantics::RehydrateFromAuthenticatedTurn,
    }
}

fn k3_settings_descriptor() -> ProviderFamilySettingsDescriptor {
    ProviderFamilySettingsDescriptor::MoonshotK3Chat {
        reasoning_efforts: vec![
            ProviderReasoningEffort::ProviderDefault,
            ProviderReasoningEffort::Low,
            ProviderReasoningEffort::High,
            ProviderReasoningEffort::Max,
        ],
        default_settings: ProviderFamilySettings::MoonshotK3Chat {
            reasoning_effort: ProviderReasoningEffort::Max,
        },
    }
}

fn accepts_k3_settings(settings: ProviderFamilySettings) -> bool {
    matches!(settings, ProviderFamilySettings::MoonshotK3Chat { .. })
}

fn k2_7_code_settings_descriptor() -> ProviderFamilySettingsDescriptor {
    ProviderFamilySettingsDescriptor::MoonshotK27CodeChat {
        default_settings: ProviderFamilySettings::MoonshotK27CodeChat,
    }
}

fn accepts_k2_7_code_settings(settings: ProviderFamilySettings) -> bool {
    matches!(settings, ProviderFamilySettings::MoonshotK27CodeChat)
}

fn k2_6_settings_descriptor() -> ProviderFamilySettingsDescriptor {
    ProviderFamilySettingsDescriptor::MoonshotK26Chat {
        thinking_modes: vec![
            MoonshotK26ThinkingMode::ProviderDefault,
            MoonshotK26ThinkingMode::Enabled,
            MoonshotK26ThinkingMode::Disabled,
            MoonshotK26ThinkingMode::EnabledKeepAll,
        ],
        default_settings: ProviderFamilySettings::MoonshotK26Chat {
            thinking_mode: MoonshotK26ThinkingMode::ProviderDefault,
        },
    }
}

fn accepts_k2_6_settings(settings: ProviderFamilySettings) -> bool {
    matches!(settings, ProviderFamilySettings::MoonshotK26Chat { .. })
}

pub(super) const fn vendor_descriptor() -> ProviderVendorDescriptor {
    ProviderVendorDescriptor {
        vendor_id: ProviderVendorId::Moonshot,
        display_name: "月之暗面",
        selectable: true,
    }
}

pub(crate) static MOONSHOT_K3_CHAT_REGISTRATION: ProviderRegistration = ProviderRegistration::new(
    ProviderProfileRef {
        id: ProviderProfileId::MoonshotK3Chat,
        version: MOONSHOT_K3_CHAT_PROFILE_VERSION,
    },
    ProviderVendorId::Moonshot,
    ProviderModelFamilyId::MoonshotK3Chat,
    ProviderProtocolDialect::OpenAiChatCompletions,
    ProviderModelIdPolicy::Exact(K3_MODEL_IDS),
    runtime_capabilities(),
    ProviderAdapterKind::MoonshotK3Chat,
    "Moonshot Kimi K3",
    ProviderProfileSettingsKind::None,
    ProviderVendorSettingsKind::Moonshot,
    ProviderImageInputPolicy::Supported,
    normalize_official_k3,
    k3_settings_descriptor,
    accepts_k3_settings,
    false,
    false,
);

pub(crate) static MOONSHOT_K2_7_CODE_CHAT_REGISTRATION: ProviderRegistration =
    ProviderRegistration::new(
        ProviderProfileRef {
            id: ProviderProfileId::MoonshotK27CodeChat,
            version: MOONSHOT_K2_7_CODE_CHAT_PROFILE_VERSION,
        },
        ProviderVendorId::Moonshot,
        ProviderModelFamilyId::MoonshotK27CodeChat,
        ProviderProtocolDialect::OpenAiChatCompletions,
        ProviderModelIdPolicy::Exact(K2_7_CODE_MODEL_IDS),
        runtime_capabilities(),
        ProviderAdapterKind::MoonshotK27CodeChat,
        "Moonshot Kimi K2.7 Code",
        ProviderProfileSettingsKind::None,
        ProviderVendorSettingsKind::Moonshot,
        ProviderImageInputPolicy::Supported,
        no_official_profile_normalization,
        k2_7_code_settings_descriptor,
        accepts_k2_7_code_settings,
        false,
        false,
    );

pub(crate) static MOONSHOT_K2_6_CHAT_REGISTRATION: ProviderRegistration = ProviderRegistration::new(
    ProviderProfileRef {
        id: ProviderProfileId::MoonshotK26Chat,
        version: MOONSHOT_K2_6_CHAT_PROFILE_VERSION,
    },
    ProviderVendorId::Moonshot,
    ProviderModelFamilyId::MoonshotK26Chat,
    ProviderProtocolDialect::OpenAiChatCompletions,
    ProviderModelIdPolicy::Exact(K2_6_MODEL_IDS),
    runtime_capabilities(),
    ProviderAdapterKind::MoonshotK26Chat,
    "Moonshot Kimi K2.6",
    ProviderProfileSettingsKind::None,
    ProviderVendorSettingsKind::Moonshot,
    ProviderImageInputPolicy::Supported,
    no_official_profile_normalization,
    k2_6_settings_descriptor,
    accepts_k2_6_settings,
    false,
    false,
);
