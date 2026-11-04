use mycopilot_core::storage::models::{ModelConfigRecord, ModelSettingsRecord};
use mycopilot_core::storage::service::StorageService;
use mycopilot_core::{
    ProviderFamilySettings, ProviderProfileConfig, ProviderProfileId, ProviderProfileRef,
    ProviderProfileValidationError, ProviderProtocolDialect, ProviderReasoningEffort,
    ProviderVendorId, ReasoningMode, PROVIDER_PROFILE_CONFIG_SCHEMA_VERSION,
};
use serde_json::json;
use tempfile::tempdir;

fn settings(profile: ProviderProfileConfig) -> ModelSettingsRecord {
    ModelSettingsRecord {
        api_url: "https://example.test/v1/chat/completions".to_string(),
        api_token: "test-token".to_string(),
        search_mode: "disabled".to_string(),
        tavily_api_key: String::new(),
        models: vec![ModelConfigRecord {
            id: "contract-model".to_string(),
            provider_model_id: "contract-model".to_string(),
            display_name: "Contract Model".to_string(),
            api_url_override: None,
            api_token_override: None,
            supports_image: false,
            context_window_tokens: Some(128_000),
            provider_profile_config: profile,
            input_price: "0".to_string(),
            cached_input_price: String::new(),
            output_price: "0".to_string(),
            enabled: true,
        }],
    }
}

#[test]
fn explicit_generic_profiles_validate_without_reasoning_wire_changes() {
    let openai =
        ProviderProfileConfig::generic_for_dialect(ProviderProtocolDialect::OpenAiChatCompletions);
    openai
        .validate_for_dialect(ProviderProtocolDialect::OpenAiChatCompletions)
        .unwrap();
    assert_eq!(openai.profile().id, ProviderProfileId::GenericOpenAiChat);
    assert_eq!(openai.reasoning_mode(), ReasoningMode::ProviderDefault);

    let anthropic =
        ProviderProfileConfig::generic_for_dialect(ProviderProtocolDialect::AnthropicMessages);
    anthropic
        .validate_for_dialect(ProviderProtocolDialect::AnthropicMessages)
        .unwrap();
    assert_eq!(
        anthropic.profile().id,
        ProviderProfileId::GenericAnthropicMessages
    );
    assert_eq!(anthropic.reasoning_mode(), ReasoningMode::ProviderDefault);
}

#[test]
fn unknown_or_incompatible_profiles_fail_closed() {
    let unknown_id = serde_json::from_value::<ProviderProfileConfig>(json!({
        "schemaVersion": PROVIDER_PROFILE_CONFIG_SCHEMA_VERSION,
        "profile": { "id": "future_profile", "version": 1 },
        "reasoning": { "mode": "provider_default", "effort": "provider_default" }
    }))
    .unwrap();
    assert!(matches!(
        unknown_id.validate(),
        Err(ProviderProfileValidationError::UnsupportedProfileVersion { .. })
    ));

    let deepseek = ProviderProfileConfig::deepseek_flash_default();
    assert!(matches!(
        deepseek.validate_for_dialect(ProviderProtocolDialect::AnthropicMessages),
        Err(ProviderProfileValidationError::IncompatibleDialect { .. })
    ));
}

#[test]
fn model_storage_round_trips_explicit_profiles() {
    let fixture = tempdir().unwrap();
    let storage = StorageService::open(&fixture.path().join("storage.sqlite")).unwrap();
    let deepseek = ProviderProfileConfig::deepseek_flash_default();

    storage
        .save_model_settings(settings(deepseek.clone()))
        .unwrap();
    assert_eq!(
        storage.load_model_settings().unwrap().unwrap().models[0].provider_profile_config,
        deepseek
    );

    let generic =
        ProviderProfileConfig::generic_for_dialect(ProviderProtocolDialect::OpenAiChatCompletions);
    storage
        .save_model_settings(settings(generic.clone()))
        .unwrap();
    assert_eq!(
        storage.load_model_settings().unwrap().unwrap().models[0].provider_profile_config,
        generic
    );
}

#[test]
fn adding_zhipu_preserves_existing_model_configuration_across_storage_restart() {
    use mycopilot_core::image_generation::{CredentialStore, InMemoryCredentialStore};
    use std::sync::Arc;

    let fixture = tempdir().unwrap();
    let path = fixture.path().join("storage.sqlite");
    let credentials = Arc::new(InMemoryCredentialStore::default()) as Arc<dyn CredentialStore>;
    let storage =
        StorageService::open_with_model_credentials(&path, Arc::clone(&credentials)).unwrap();
    let generic =
        ProviderProfileConfig::generic_for_dialect(ProviderProtocolDialect::OpenAiChatCompletions);
    let mut existing = settings(generic);
    // A GLM model previously configured as generic must never be auto-migrated.
    existing.models[0].provider_model_id = "glm-5.3".into();
    existing.models[0].context_window_tokens = Some(600_000);
    existing.models[0].input_price = "0.00123".into();
    existing.models[0].cached_input_price = "0.00004".into();
    existing.models[0].output_price = "0.00567".into();
    let mut deepseek = existing.models[0].clone();
    deepseek.id = "existing-deepseek".into();
    deepseek.display_name = "Existing DeepSeek".into();
    deepseek.provider_model_id = "deepseek-flash".into();
    deepseek.supports_image = true;
    deepseek.provider_profile_config = ProviderProfileConfig::deepseek_flash_default();
    existing.models.push(deepseek);
    let mut moonshot = existing.models[0].clone();
    moonshot.id = "existing-moonshot".into();
    moonshot.display_name = "Existing Moonshot".into();
    moonshot.provider_model_id = "kimi-k3.1-pro".into();
    moonshot.supports_image = true;
    moonshot.provider_profile_config = ProviderProfileConfig::from_family_settings(
        ProviderProfileRef::moonshot_k3_chat(),
        ProviderVendorId::Moonshot,
        ProviderFamilySettings::MoonshotK3Chat {
            reasoning_effort: ProviderReasoningEffort::Low,
        },
    );
    existing.models.push(moonshot);
    storage.save_model_settings(existing).unwrap();
    let before = storage.load_model_settings().unwrap().unwrap();
    let mut extended = before.clone();
    for model in [
        "glm-5.3",
        "glm-5.3-flash",
        "glm-5.3-flashx",
        "glm-5.3-flashx-legal",
    ] {
        let mut added = before.models[0].clone();
        added.id = format!("zhipu-{model}");
        added.display_name = format!("Zhipu {model}");
        added.provider_model_id = model.into();
        added.supports_image = model != "glm-5.3";
        let (profile, settings) = if model == "glm-5.3" {
            (
                ProviderProfileRef::zhipu_glm5_3_chat(),
                ProviderFamilySettings::ZhipuGlm53Chat {
                    reasoning_effort: ProviderReasoningEffort::High,
                },
            )
        } else {
            (
                ProviderProfileRef::zhipu_glm5_3_flash_chat(),
                ProviderFamilySettings::ZhipuGlm53FlashChat {
                    reasoning_effort: ProviderReasoningEffort::High,
                },
            )
        };
        added.provider_profile_config =
            ProviderProfileConfig::from_family_settings(profile, ProviderVendorId::Zhipu, settings);
        extended.models.push(added);
    }
    storage.save_model_settings(extended.clone()).unwrap();
    drop(storage);
    let restarted = StorageService::open_with_model_credentials(&path, credentials).unwrap();
    let restored = restarted.load_model_settings().unwrap().unwrap();
    fn model_configuration(model: &ModelConfigRecord) -> serde_json::Value {
        json!({
            "id": model.id,
            "providerModelId": model.provider_model_id,
            "displayName": model.display_name,
            "apiUrlOverride": model.api_url_override,
            "apiTokenOverride": model.api_token_override,
            "supportsImage": model.supports_image,
            "contextWindowTokens": model.context_window_tokens,
            "profile": model.provider_profile_config,
            "inputPrice": model.input_price,
            "cachedInputPrice": model.cached_input_price,
            "outputPrice": model.output_price,
            "enabled": model.enabled,
        })
    }
    assert_eq!(restored.models.len(), extended.models.len());
    for (actual, expected) in restored.models.iter().zip(&extended.models) {
        assert_eq!(model_configuration(actual), model_configuration(expected));
    }
    for (actual, expected) in restored.models.iter().zip(&before.models) {
        assert_eq!(model_configuration(actual), model_configuration(expected));
    }
    assert_eq!(restored.api_url, before.api_url);
    assert_eq!(restored.api_token, before.api_token);
}
