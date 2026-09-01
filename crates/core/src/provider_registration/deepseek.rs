//! DeepSeek Profile registrations and model-family policy.
//!
//! Keep every DeepSeek model id, setting default and runtime capability decision here.

use super::{
    ProviderAdapterKind, ProviderCheckpointPrivateArgumentsSemantics,
    ProviderContextProjectionSemantics, ProviderFamilySettings, ProviderFamilySettingsDescriptor,
    ProviderModelFamilyId, ProviderModelIdPolicy, ProviderPartialTraceSemantics,
    ProviderPrivateReplaySemantics, ProviderProfileId, ProviderProfileRef,
    ProviderProfileSettingsKind, ProviderProtocolDialect, ProviderRegistration,
    ProviderRuntimeCapabilities, ProviderTerminalBatchSemantics, ProviderToolCallSourceSemantics,
    ProviderToolExchangeSemantics, ProviderUsageSemantics, ProviderVendorDescriptor,
    ProviderVendorId, ProviderVendorSettingsKind,
};
use crate::provider_profile::{
    ProviderFamilyReasoningPolicy, ProviderReasoningEffort, ReasoningMode,
    DEEPSEEK_V4_CHAT_PROFILE_VERSION, DEEPSEEK_V4_VISION_PROFILE_VERSION,
};

const V4_CHAT_MODEL_IDS: &[&str] = &["deepseek-v4-flash", "deepseek-v4-pro"];
const V4_VISION_MODEL_IDS: &[&str] = &["deepseek-v4-flash-vision-exp"];

// This is DeepSeek's own versioned runtime contract, not a shared cross-vendor Profile helper.
const fn runtime_capabilities() -> ProviderRuntimeCapabilities {
    ProviderRuntimeCapabilities {
        tool_exchange: ProviderToolExchangeSemantics::ExactProviderGrouped,
        private_replay: ProviderPrivateReplaySemantics::AdapterClassified,
        context_projection: ProviderContextProjectionSemantics::ExactProviderTurn,
        usage: ProviderUsageSemantics::CompletionIncludesReasoning,
        partial_trace: ProviderPartialTraceSemantics::DeferUntilProviderTurnClosed,
        tool_call_source: ProviderToolCallSourceSemantics::ProviderNativeOnly,
        terminal_batch: ProviderTerminalBatchSemantics::CloseWholeProviderTurn,
        checkpoint_private_arguments:
            ProviderCheckpointPrivateArgumentsSemantics::RehydrateFromAuthenticatedTurn,
    }
}

fn chat_settings_descriptor() -> ProviderFamilySettingsDescriptor {
    ProviderFamilySettingsDescriptor::DeepseekV4Chat {
        reasoning_modes: reasoning_modes(),
        reasoning_efforts: reasoning_efforts(),
        default_settings: ProviderFamilySettings::DeepseekV4Chat {
            reasoning: ProviderFamilyReasoningPolicy::provider_default(),
        },
    }
}

fn vision_settings_descriptor() -> ProviderFamilySettingsDescriptor {
    ProviderFamilySettingsDescriptor::DeepseekV4Vision {
        reasoning_modes: reasoning_modes(),
        reasoning_efforts: reasoning_efforts(),
        default_settings: ProviderFamilySettings::DeepseekV4Vision {
            reasoning: ProviderFamilyReasoningPolicy::provider_default(),
        },
    }
}

fn reasoning_modes() -> Vec<ReasoningMode> {
    vec![
        ReasoningMode::ProviderDefault,
        ReasoningMode::Enabled,
        ReasoningMode::Disabled,
    ]
}

fn reasoning_efforts() -> Vec<ProviderReasoningEffort> {
    vec![
        ProviderReasoningEffort::ProviderDefault,
        ProviderReasoningEffort::Low,
        ProviderReasoningEffort::High,
        ProviderReasoningEffort::Max,
    ]
}

fn accepts_chat_settings(settings: ProviderFamilySettings) -> bool {
    matches!(settings, ProviderFamilySettings::DeepseekV4Chat { .. })
}

fn accepts_vision_settings(settings: ProviderFamilySettings) -> bool {
    matches!(settings, ProviderFamilySettings::DeepseekV4Vision { .. })
}

pub(super) const fn vendor_descriptor() -> ProviderVendorDescriptor {
    ProviderVendorDescriptor {
        vendor_id: ProviderVendorId::DeepSeek,
        display_name: "DeepSeek",
        selectable: true,
    }
}

pub(crate) static DEEPSEEK_V4_CHAT_REGISTRATION: ProviderRegistration = ProviderRegistration::new(
    ProviderProfileRef {
        id: ProviderProfileId::DeepSeekV4Chat,
        version: DEEPSEEK_V4_CHAT_PROFILE_VERSION,
    },
    ProviderVendorId::DeepSeek,
    ProviderModelFamilyId::DeepSeekV4Chat,
    ProviderProtocolDialect::OpenAiChatCompletions,
    ProviderModelIdPolicy::Exact(V4_CHAT_MODEL_IDS),
    runtime_capabilities(),
    ProviderAdapterKind::DeepSeekV4Chat,
    "深度求索 / DeepSeek（V4 Chat）",
    ProviderProfileSettingsKind::DeepseekV4Chat,
    ProviderVendorSettingsKind::Deepseek,
    chat_settings_descriptor,
    accepts_chat_settings,
    true,
    true,
);

pub(crate) static DEEPSEEK_V4_VISION_REGISTRATION: ProviderRegistration = ProviderRegistration::new(
    ProviderProfileRef {
        id: ProviderProfileId::DeepSeekV4Vision,
        version: DEEPSEEK_V4_VISION_PROFILE_VERSION,
    },
    ProviderVendorId::DeepSeek,
    ProviderModelFamilyId::DeepSeekV4Vision,
    ProviderProtocolDialect::OpenAiChatCompletions,
    ProviderModelIdPolicy::Exact(V4_VISION_MODEL_IDS),
    runtime_capabilities(),
    ProviderAdapterKind::DeepSeekV4Vision,
    "DeepSeek Vision",
    ProviderProfileSettingsKind::None,
    ProviderVendorSettingsKind::Deepseek,
    vision_settings_descriptor,
    accepts_vision_settings,
    false,
    false,
);
