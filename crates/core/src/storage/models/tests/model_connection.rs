use crate::provider_profile::{ProviderProfileConfig, ProviderProtocolDialect};

use super::*;

fn model(api_url_override: Option<&str>, api_token_override: Option<&str>) -> ModelConfigRecord {
    ModelConfigRecord {
        id: "model-a".to_string(),
        provider_model_id: "model-a".to_string(),
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
fn display_label_uses_the_unique_display_name_without_exposing_the_internal_id() {
    let mut model = model(None, None);
    model.id = "model-config:private-uuid".to_string();
    model.display_name = "  Kimi K3 Max  ".to_string();

    assert_eq!(model.display_label(), "Kimi K3 Max");
    assert!(!model.display_label().contains("private-uuid"));
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
fn provider_urls_reject_unsafe_authority_and_url_components() {
    for url in [
        "http://provider.example/v1",
        "https://user:password@provider.example/v1",
        "https://provider.example/v1?token=secret",
        "https://provider.example/v1#secret",
    ] {
        let settings = settings(model(Some(url), Some("model-token")));
        assert!(settings
            .effective_connection_for(&settings.models[0])
            .is_err());
    }
}

#[test]
fn provider_urls_enforce_utf8_byte_limit_and_reject_control_characters() {
    let prefix = "https://provider.example/";
    let maximum = format!(
        "{prefix}{}",
        "a".repeat(MODEL_PROVIDER_API_URL_MAX_BYTES - prefix.len())
    );
    assert_eq!(maximum.len(), MODEL_PROVIDER_API_URL_MAX_BYTES);
    assert!(validated_connection("Model A", "专用", &maximum, "token").is_ok());

    let overlong_unicode = format!("{prefix}{}", "界".repeat(1_400));
    assert!(overlong_unicode.chars().count() < MODEL_PROVIDER_API_URL_MAX_BYTES);
    assert!(overlong_unicode.len() > MODEL_PROVIDER_API_URL_MAX_BYTES);
    assert!(validated_connection("Model A", "专用", &overlong_unicode, "token").is_err());

    for url in [
        "https://provider.example/v1\n",
        "https://provider.example/\u{0001}v1",
    ] {
        assert!(validated_connection("Model A", "专用", url, "token").is_err());
    }
}

#[cfg(debug_assertions)]
#[test]
fn development_build_allows_only_explicit_loopback_http() {
    for url in ["http://localhost:11434/v1", "http://127.0.0.1:11434/v1"] {
        let settings = settings(model(Some(url), Some("model-token")));
        assert!(settings
            .effective_connection_for(&settings.models[0])
            .is_ok());
    }
    let settings = settings(model(
        Some("http://192.168.1.20:11434/v1"),
        Some("model-token"),
    ));
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
