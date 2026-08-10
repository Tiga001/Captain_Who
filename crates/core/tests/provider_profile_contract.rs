use mycopilot_core::storage::models::{ModelConfigRecord, ModelSettingsRecord};
use mycopilot_core::storage::service::StorageService;
use mycopilot_core::{
    ProviderProfileConfig, ProviderProfileId, ProviderProfileValidationError,
    ProviderProtocolDialect, ReasoningMode, PROVIDER_PROFILE_CONFIG_SCHEMA_VERSION,
};
use serde_json::json;
use tempfile::tempdir;

fn settings(profile: Option<ProviderProfileConfig>) -> ModelSettingsRecord {
    ModelSettingsRecord {
        api_url: "https://example.test/v1/chat/completions".to_string(),
        api_token: "test-token".to_string(),
        search_mode: "disabled".to_string(),
        tavily_api_key: String::new(),
        models: vec![ModelConfigRecord {
            id: "contract-model".to_string(),
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
fn legacy_absence_resolves_to_generic_without_reasoning_wire_changes() {
    let openai =
        ProviderProfileConfig::resolve(None, ProviderProtocolDialect::OpenAiChatCompletions)
            .unwrap();
    assert_eq!(openai.profile.id, ProviderProfileId::GenericOpenAiChat);
    assert_eq!(openai.reasoning.mode, ReasoningMode::ProviderDefault);

    let anthropic =
        ProviderProfileConfig::resolve(None, ProviderProtocolDialect::AnthropicMessages).unwrap();
    assert_eq!(
        anthropic.profile.id,
        ProviderProfileId::GenericAnthropicMessages
    );
    assert_eq!(anthropic.reasoning.mode, ReasoningMode::ProviderDefault);
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

    let deepseek = ProviderProfileConfig::deepseek_v4_default();
    assert!(matches!(
        deepseek.validate_for_dialect(ProviderProtocolDialect::AnthropicMessages),
        Err(ProviderProfileValidationError::IncompatibleDialect { .. })
    ));
}

#[test]
fn model_storage_round_trips_and_legacy_saves_preserve_explicit_profiles() {
    let fixture = tempdir().unwrap();
    let storage = StorageService::open(&fixture.path().join("storage.sqlite")).unwrap();
    let deepseek = ProviderProfileConfig::deepseek_v4_default();

    storage
        .save_model_settings(settings(Some(deepseek.clone())))
        .unwrap();
    assert_eq!(
        storage.load_model_settings().unwrap().unwrap().models[0].provider_profile_config,
        Some(deepseek.clone())
    );

    storage.save_model_settings(settings(None)).unwrap();
    assert_eq!(
        storage.load_model_settings().unwrap().unwrap().models[0].provider_profile_config,
        Some(deepseek)
    );

    let generic =
        ProviderProfileConfig::generic_for_dialect(ProviderProtocolDialect::OpenAiChatCompletions);
    storage
        .save_model_settings(settings(Some(generic.clone())))
        .unwrap();
    assert_eq!(
        storage.load_model_settings().unwrap().unwrap().models[0].provider_profile_config,
        Some(generic)
    );
}
