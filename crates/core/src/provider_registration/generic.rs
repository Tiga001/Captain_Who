//! Generic OpenAI/Anthropic Profile registrations.
//!
//! This module owns Generic compatibility defaults. Vendor-specific modules must not import it.

use super::{
    ProviderAdapterKind, ProviderCheckpointPrivateArgumentsSemantics,
    ProviderContextProjectionSemantics, ProviderFamilySettings, ProviderFamilySettingsDescriptor,
    ProviderImageInputPolicy, ProviderModelFamilyId, ProviderModelIdPolicy,
    ProviderPartialTraceSemantics, ProviderPrivateReplaySemantics, ProviderProfileId,
    ProviderProfileRef, ProviderProfileSettingsKind, ProviderProtocolDialect, ProviderRegistration,
    ProviderRuntimeCapabilities, ProviderTerminalBatchSemantics, ProviderToolCallSourceSemantics,
    ProviderToolExchangeSemantics, ProviderUsageSemantics, ProviderVendorDescriptor,
    ProviderVendorId, ProviderVendorSettingsKind,
};
use crate::provider_profile::{
    GENERIC_ANTHROPIC_MESSAGES_PROFILE_VERSION, GENERIC_OPENAI_CHAT_PROFILE_VERSION,
};

const fn runtime_capabilities() -> ProviderRuntimeCapabilities {
    ProviderRuntimeCapabilities {
        tool_exchange: ProviderToolExchangeSemantics::SplitToolExchange,
        private_replay: ProviderPrivateReplaySemantics::None,
        context_projection: ProviderContextProjectionSemantics::GenericEffectiveCalls,
        usage: ProviderUsageSemantics::StandardAdditive,
        partial_trace: ProviderPartialTraceSemantics::IncrementalBaseline,
        tool_call_source: ProviderToolCallSourceSemantics::TextFallbackAllowed,
        terminal_batch: ProviderTerminalBatchSemantics::IndependentCalls,
        checkpoint_private_arguments: ProviderCheckpointPrivateArgumentsSemantics::Reject,
    }
}

fn settings_descriptor() -> ProviderFamilySettingsDescriptor {
    ProviderFamilySettingsDescriptor::Generic {
        default_settings: ProviderFamilySettings::Generic,
    }
}

fn accepts_settings(settings: ProviderFamilySettings) -> bool {
    matches!(settings, ProviderFamilySettings::Generic)
}

pub(super) const fn vendor_descriptor() -> ProviderVendorDescriptor {
    ProviderVendorDescriptor {
        vendor_id: ProviderVendorId::Generic,
        display_name: "通用兼容",
        selectable: true,
    }
}

pub(crate) static GENERIC_OPENAI_CHAT_REGISTRATION: ProviderRegistration =
    ProviderRegistration::new(
        ProviderProfileRef {
            id: ProviderProfileId::GenericOpenAiChat,
            version: GENERIC_OPENAI_CHAT_PROFILE_VERSION,
        },
        ProviderVendorId::Generic,
        ProviderModelFamilyId::GenericOpenAiChat,
        ProviderProtocolDialect::OpenAiChatCompletions,
        ProviderModelIdPolicy::AnyNonEmpty,
        runtime_capabilities(),
        ProviderAdapterKind::GenericOpenAi,
        "通用兼容",
        ProviderProfileSettingsKind::None,
        ProviderVendorSettingsKind::None,
        ProviderImageInputPolicy::UserConfigurable,
        settings_descriptor,
        accepts_settings,
        true,
        false,
    );

pub(crate) static GENERIC_ANTHROPIC_MESSAGES_REGISTRATION: ProviderRegistration =
    ProviderRegistration::new(
        ProviderProfileRef {
            id: ProviderProfileId::GenericAnthropicMessages,
            version: GENERIC_ANTHROPIC_MESSAGES_PROFILE_VERSION,
        },
        ProviderVendorId::Generic,
        ProviderModelFamilyId::GenericAnthropicMessages,
        ProviderProtocolDialect::AnthropicMessages,
        ProviderModelIdPolicy::AnyNonEmpty,
        runtime_capabilities(),
        ProviderAdapterKind::GenericAnthropic,
        "通用兼容",
        ProviderProfileSettingsKind::None,
        ProviderVendorSettingsKind::None,
        ProviderImageInputPolicy::UserConfigurable,
        settings_descriptor,
        accepts_settings,
        true,
        false,
    );
