//! Zhipu GLM 5.3 registrations and exact model-family policy.

use super::{
    ProviderAdapterKind, ProviderCheckpointPrivateArgumentsSemantics,
    ProviderContextProjectionSemantics, ProviderFamilySettings, ProviderFamilySettingsDescriptor,
    ProviderImageInputPolicy, ProviderModelFamilyId, ProviderModelIdPolicy,
    ProviderOutputReservation, ProviderPartialTraceSemantics, ProviderPrivateReplaySemantics,
    ProviderProfileId, ProviderProfileRef, ProviderProfileSettingsKind, ProviderProtocolDialect,
    ProviderRegistration, ProviderRuntimeCapabilities, ProviderTerminalBatchSemantics,
    ProviderToolCallSourceSemantics, ProviderToolExchangeSemantics, ProviderUsageSemantics,
    ProviderVendorDescriptor, ProviderVendorId, ProviderVendorSettingsKind,
};
use crate::provider_profile::{
    ProviderReasoningEffort, ZHIPU_GLM5_3_CHAT_PROFILE_VERSION,
    ZHIPU_GLM5_3_FLASH_CHAT_PROFILE_VERSION,
};

const GLM53_MODEL_IDS: &[&str] = &["glm-5.3"];
const GLM53_FLASH_MODEL_IDS: &[&str] = &["glm-5.3-flash", "glm-5.3-flashx", "glm-5.3-flashx-legal"];

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
        // GLM 5.3 and its Flash family share the documented default output allowance.
        // https://docs.bigmodel.cn/cn/guide/models/text/glm-5.3
        // https://docs.bigmodel.cn/cn/guide/models/vlm/glm-5.3-flash
        output_reservation: ProviderOutputReservation::Fixed(65_536),
    }
}

fn reasoning_efforts() -> Vec<ProviderReasoningEffort> {
    vec![
        ProviderReasoningEffort::ProviderDefault,
        ProviderReasoningEffort::Low,
        ProviderReasoningEffort::High,
        ProviderReasoningEffort::Max,
    ]
}

fn glm53_settings_descriptor() -> ProviderFamilySettingsDescriptor {
    ProviderFamilySettingsDescriptor::ZhipuGlm53Chat {
        reasoning_efforts: reasoning_efforts(),
        default_settings: ProviderFamilySettings::ZhipuGlm53Chat {
            reasoning_effort: ProviderReasoningEffort::Max,
        },
    }
}

fn accepts_glm53_settings(settings: ProviderFamilySettings) -> bool {
    matches!(settings, ProviderFamilySettings::ZhipuGlm53Chat { .. })
}

fn flash_settings_descriptor() -> ProviderFamilySettingsDescriptor {
    ProviderFamilySettingsDescriptor::ZhipuGlm53FlashChat {
        reasoning_efforts: reasoning_efforts(),
        default_settings: ProviderFamilySettings::ZhipuGlm53FlashChat {
            reasoning_effort: ProviderReasoningEffort::Max,
        },
    }
}

fn accepts_flash_settings(settings: ProviderFamilySettings) -> bool {
    matches!(settings, ProviderFamilySettings::ZhipuGlm53FlashChat { .. })
}

pub(super) fn vendor_descriptor() -> ProviderVendorDescriptor {
    ProviderVendorDescriptor::new(
        ProviderVendorId::Zhipu,
        "智谱",
        true,
        Some("https://open.bigmodel.cn/api/paas/v4/chat/completions"),
    )
}

pub(crate) static ZHIPU_GLM5_3_CHAT_REGISTRATION: ProviderRegistration = ProviderRegistration::new(
    ProviderProfileRef {
        id: ProviderProfileId::ZhipuGlm53Chat,
        version: ZHIPU_GLM5_3_CHAT_PROFILE_VERSION,
    },
    ProviderVendorId::Zhipu,
    ProviderModelFamilyId::ZhipuGlm53Chat,
    ProviderProtocolDialect::OpenAiChatCompletions,
    ProviderModelIdPolicy::Exact(GLM53_MODEL_IDS),
    runtime_capabilities(),
    ProviderAdapterKind::ZhipuGlm53Chat,
    "Zhipu GLM 5.3",
    ProviderProfileSettingsKind::None,
    ProviderVendorSettingsKind::Zhipu,
    ProviderImageInputPolicy::Unsupported,
    glm53_settings_descriptor,
    accepts_glm53_settings,
    false,
    false,
);

pub(crate) static ZHIPU_GLM5_3_FLASH_CHAT_REGISTRATION: ProviderRegistration =
    ProviderRegistration::new(
        ProviderProfileRef {
            id: ProviderProfileId::ZhipuGlm53FlashChat,
            version: ZHIPU_GLM5_3_FLASH_CHAT_PROFILE_VERSION,
        },
        ProviderVendorId::Zhipu,
        ProviderModelFamilyId::ZhipuGlm53FlashChat,
        ProviderProtocolDialect::OpenAiChatCompletions,
        ProviderModelIdPolicy::Exact(GLM53_FLASH_MODEL_IDS),
        runtime_capabilities(),
        ProviderAdapterKind::ZhipuGlm53FlashChat,
        "Zhipu GLM 5.3 Flash",
        ProviderProfileSettingsKind::None,
        ProviderVendorSettingsKind::Zhipu,
        ProviderImageInputPolicy::Supported,
        flash_settings_descriptor,
        accepts_flash_settings,
        false,
        false,
    );

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider_profile::{ProviderProfileConfig, ReasoningMode};
    use crate::provider_registration::{
        resolve_provider_vendor_model_policy, resolve_provider_vendor_registration,
        ProviderContinuationRequirement, ProviderVendorModelPolicyInput,
    };
    use serde_json::json;

    #[test]
    fn every_zhipu_model_resolves_its_exact_family_and_settings_contract() {
        for (models, expected, image_input, settings) in [
            (
                GLM53_MODEL_IDS,
                &ZHIPU_GLM5_3_CHAT_REGISTRATION,
                "unsupported",
                ProviderFamilySettings::ZhipuGlm53Chat {
                    reasoning_effort: ProviderReasoningEffort::Max,
                },
            ),
            (
                GLM53_FLASH_MODEL_IDS,
                &ZHIPU_GLM5_3_FLASH_CHAT_REGISTRATION,
                "supported",
                ProviderFamilySettings::ZhipuGlm53FlashChat {
                    reasoning_effort: ProviderReasoningEffort::Max,
                },
            ),
        ] {
            for model in models {
                let registration = resolve_provider_vendor_registration(
                    ProviderVendorId::Zhipu,
                    model,
                    ProviderProtocolDialect::OpenAiChatCompletions,
                )
                .unwrap();
                assert!(std::ptr::eq(registration, expected));
                let config = registration.config_from_vendor_settings(settings).unwrap();
                config
                    .validate_for_model(model, ProviderProtocolDialect::OpenAiChatCompletions)
                    .unwrap();
                assert_eq!(config.reasoning_mode(), ReasoningMode::Enabled);
                let encoded = serde_json::to_value(&config).unwrap();
                assert_eq!(encoded["schemaVersion"], 2);
                assert_eq!(
                    encoded["settings"],
                    json!({
                        "kind": expected.profile().id.as_str(), "reasoningEffort": "max"
                    })
                );
                assert_eq!(
                    serde_json::from_value::<ProviderProfileConfig>(encoded).unwrap(),
                    config
                );

                let policy = serde_json::to_value(resolve_provider_vendor_model_policy(
                    &ProviderVendorModelPolicyInput {
                        vendor_id: ProviderVendorId::Zhipu,
                        model_id: model.to_string(),
                        dialect: ProviderProtocolDialect::OpenAiChatCompletions,
                    },
                ))
                .unwrap();
                assert_eq!(policy["status"], "supported");
                assert_eq!(policy["settingsKind"], "zhipu");
                assert_eq!(policy["imageInput"], image_input);
                assert_eq!(
                    policy["settings"]["defaultSettings"]["reasoningEffort"],
                    "max"
                );
                assert!(policy.get("runtimeCapabilities").is_none());
                assert!(policy.get("profileId").is_none());
            }
        }
    }

    #[test]
    fn zhipu_registration_rejects_unknown_models_dialects_and_cross_family_settings() {
        for model in ["glm-5", "glm-5.3-future", "glm-5.3-flash-future", "GLM-5.3"] {
            assert!(resolve_provider_vendor_registration(
                ProviderVendorId::Zhipu,
                model,
                ProviderProtocolDialect::OpenAiChatCompletions,
            )
            .is_err());
        }
        assert!(resolve_provider_vendor_registration(
            ProviderVendorId::Zhipu,
            "glm-5.3",
            ProviderProtocolDialect::AnthropicMessages,
        )
        .is_err());
        for settings in [
            ProviderFamilySettings::ZhipuGlm53FlashChat {
                reasoning_effort: ProviderReasoningEffort::Max,
            },
            ProviderFamilySettings::MoonshotK3Chat {
                reasoning_effort: ProviderReasoningEffort::Max,
            },
        ] {
            assert!(ZHIPU_GLM5_3_CHAT_REGISTRATION
                .config_from_vendor_settings(settings)
                .is_err());
        }
        for invalid in [
            json!({"kind": "zhipu_glm5_3_chat", "reasoningEffort": "medium"}),
            json!({"kind": "zhipu_glm5_3_chat", "reasoningEffort": "max", "thinkingMode": "disabled"}),
        ] {
            assert!(serde_json::from_value::<ProviderFamilySettings>(invalid).is_err());
        }
    }

    #[test]
    fn zhipu_turn_contract_keeps_grouped_private_replay_and_reasoning_subset_usage() {
        for registration in [
            &ZHIPU_GLM5_3_CHAT_REGISTRATION,
            &ZHIPU_GLM5_3_FLASH_CHAT_REGISTRATION,
        ] {
            let capabilities = registration.runtime_capabilities();
            let turn = capabilities.classify_turn(true, true, ReasoningMode::Enabled);
            assert_eq!(
                turn.continuation_requirement(),
                ProviderContinuationRequirement::Required
            );
            assert!(turn.preserves_grouped_turn());
            assert!(turn.requires_private_replay());
            assert!(turn.requires_exact_approval_refs());
            assert!(turn.allows_encrypted_checkpoint_rehydration());
            assert!(turn.settles_entire_batch_on_terminal());
            assert_eq!(
                capabilities.usage(),
                ProviderUsageSemantics::CompletionIncludesReasoning
            );
            assert_eq!(
                capabilities
                    .output_token_reservation(ReasoningMode::Enabled, ProviderReasoningEffort::Max),
                Some(65_536)
            );
        }
        let vendor = serde_json::to_value(vendor_descriptor()).unwrap();
        assert_eq!(
            vendor["fixedApiUrl"],
            "https://open.bigmodel.cn/api/paas/v4/chat/completions"
        );
        assert_eq!(
            vendor["modelIdPolicy"],
            json!({
                "kind": "fixed",
                "modelIds": ["glm-5.3", "glm-5.3-flash", "glm-5.3-flashx", "glm-5.3-flashx-legal"]
            })
        );
    }
}
