//! DeepSeek Profile registrations and current official model-family policy.
//!
//! Model ids are exact and current-only. Retired aliases deliberately do not resolve.

use super::{
    ProviderAdapterKind, ProviderCheckpointPrivateArgumentsSemantics,
    ProviderContextProjectionSemantics, ProviderFamilySettingsDescriptor, ProviderImageInputPolicy,
    ProviderModelFamilyId, ProviderModelIdPolicy, ProviderPartialTraceSemantics,
    ProviderPrivateReplaySemantics, ProviderProfileId, ProviderProfileRef,
    ProviderProfileSettingsKind, ProviderProtocolDialect, ProviderRegistration,
    ProviderRuntimeCapabilities, ProviderTerminalBatchSemantics, ProviderToolCallSourceSemantics,
    ProviderToolExchangeSemantics, ProviderUsageSemantics, ProviderVendorDescriptor,
    ProviderVendorId, ProviderVendorSettingsKind,
};
use crate::provider_profile::{
    ProviderFamilyReasoningPolicy, ProviderFamilySettings, ProviderReasoningEffort, ReasoningMode,
    DEEPSEEK_V4_1_FLASH_CHAT_PROFILE_VERSION, DEEPSEEK_V4_PRO_0813_CHAT_PROFILE_VERSION,
};

const FLASH_MODEL_IDS: &[&str] = &["deepseek-flash"];
const PRO_MODEL_IDS: &[&str] = &["deepseek-v4-pro"];

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

fn flash_settings_descriptor() -> ProviderFamilySettingsDescriptor {
    ProviderFamilySettingsDescriptor::DeepseekFlashChat {
        reasoning_modes: reasoning_modes(),
        reasoning_efforts: reasoning_efforts(),
        default_settings: ProviderFamilySettings::DeepseekFlashChat {
            reasoning: ProviderFamilyReasoningPolicy::provider_default(),
        },
    }
}

fn pro_settings_descriptor() -> ProviderFamilySettingsDescriptor {
    ProviderFamilySettingsDescriptor::DeepseekProChat {
        reasoning_modes: reasoning_modes(),
        reasoning_efforts: reasoning_efforts(),
        default_settings: ProviderFamilySettings::DeepseekProChat {
            reasoning: ProviderFamilyReasoningPolicy::provider_default(),
        },
    }
}

fn accepts_flash_settings(settings: ProviderFamilySettings) -> bool {
    matches!(settings, ProviderFamilySettings::DeepseekFlashChat { .. })
}

fn accepts_pro_settings(settings: ProviderFamilySettings) -> bool {
    matches!(settings, ProviderFamilySettings::DeepseekProChat { .. })
}

pub(super) const fn vendor_descriptor() -> ProviderVendorDescriptor {
    ProviderVendorDescriptor {
        vendor_id: ProviderVendorId::DeepSeek,
        display_name: "DeepSeek",
        selectable: true,
    }
}

pub(crate) static DEEPSEEK_V4_1_FLASH_CHAT_REGISTRATION: ProviderRegistration =
    ProviderRegistration::new(
        ProviderProfileRef {
            id: ProviderProfileId::DeepSeekV41FlashChat,
            version: DEEPSEEK_V4_1_FLASH_CHAT_PROFILE_VERSION,
        },
        ProviderVendorId::DeepSeek,
        ProviderModelFamilyId::DeepSeekFlashChat,
        ProviderProtocolDialect::OpenAiChatCompletions,
        ProviderModelIdPolicy::Exact(FLASH_MODEL_IDS),
        runtime_capabilities(),
        ProviderAdapterKind::DeepSeekV41FlashChat,
        "DeepSeek Flash",
        ProviderProfileSettingsKind::None,
        ProviderVendorSettingsKind::Deepseek,
        ProviderImageInputPolicy::Supported,
        flash_settings_descriptor,
        accepts_flash_settings,
        false,
        false,
    );

pub(crate) static DEEPSEEK_V4_PRO_0813_CHAT_REGISTRATION: ProviderRegistration =
    ProviderRegistration::new(
        ProviderProfileRef {
            id: ProviderProfileId::DeepSeekV4Pro0813Chat,
            version: DEEPSEEK_V4_PRO_0813_CHAT_PROFILE_VERSION,
        },
        ProviderVendorId::DeepSeek,
        ProviderModelFamilyId::DeepSeekProChat,
        ProviderProtocolDialect::OpenAiChatCompletions,
        ProviderModelIdPolicy::Exact(PRO_MODEL_IDS),
        runtime_capabilities(),
        ProviderAdapterKind::DeepSeekV4Pro0813Chat,
        "DeepSeek Pro",
        ProviderProfileSettingsKind::None,
        ProviderVendorSettingsKind::Deepseek,
        ProviderImageInputPolicy::Unsupported,
        pro_settings_descriptor,
        accepts_pro_settings,
        false,
        false,
    );
