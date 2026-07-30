use super::*;

fn revision_test_settings() -> ModelSettingsRecord {
    ModelSettingsRecord {
        api_url: "https://revision.example/v1".to_string(),
        api_token: "fixed-revision-test-token".to_string(),
        search_mode: "disabled".to_string(),
        tavily_api_key: String::new(),
        models: vec![ModelConfigRecord {
            id: "revision-model".to_string(),
            display_name: "Revision Model".to_string(),
            api_url_override: None,
            api_token_override: None,
            supports_image: false,
            context_window_tokens: Some(128_000),
            input_price: "0".to_string(),
            output_price: "0".to_string(),
            enabled: true,
        }],
    }
}

#[test]
fn every_model_settings_save_rotates_an_opaque_host_revision() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let settings = revision_test_settings();

    service.save_model_settings(settings.clone()).unwrap();
    let first = service.load_model_settings_snapshot().unwrap().unwrap();
    service.save_model_settings(settings).unwrap();
    let second = service.load_model_settings_snapshot().unwrap().unwrap();

    assert!(
        crate::storage::config_repository::is_model_settings_revision(
            &first.configuration_revision
        )
    );
    assert!(
        crate::storage::config_repository::is_model_settings_revision(
            &second.configuration_revision
        )
    );
    assert_ne!(first.configuration_revision, second.configuration_revision);
    assert_eq!(first.settings.models[0].id, second.settings.models[0].id);
    assert_eq!(format!("{first:?}"), "ModelSettingsSnapshot([REDACTED])");
}

#[test]
fn rejects_invalid_model_prices_without_overwriting_saved_settings() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let valid = ModelSettingsRecord {
        api_url: "https://example.com".to_string(),
        api_token: "token".to_string(),
        search_mode: "disabled".to_string(),
        tavily_api_key: String::new(),
        models: vec![ModelConfigRecord {
            id: "model-a".to_string(),
            display_name: "Model A".to_string(),
            api_url_override: None,
            api_token_override: None,
            supports_image: false,
            context_window_tokens: Some(128_000),
            input_price: "0.01".to_string(),
            output_price: "0.02".to_string(),
            enabled: true,
        }],
    };
    service.save_model_settings(valid.clone()).unwrap();

    let mut invalid = valid;
    invalid.models[0].input_price = "not-a-price".to_string();
    assert!(service.save_model_settings(invalid).is_err());

    let stored = service.load_model_settings().unwrap().unwrap();
    assert_eq!(stored.models[0].input_price, "0.01");
    assert_eq!(stored.models[0].context_window_tokens, Some(128_000));
}

#[test]
fn rejects_zero_context_window_without_overwriting_saved_settings() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let valid = ModelSettingsRecord {
        api_url: "https://example.com".to_string(),
        api_token: "token".to_string(),
        search_mode: "disabled".to_string(),
        tavily_api_key: String::new(),
        models: vec![ModelConfigRecord {
            id: "model-a".to_string(),
            display_name: "Model A".to_string(),
            api_url_override: None,
            api_token_override: None,
            supports_image: false,
            context_window_tokens: Some(128_000),
            input_price: "0.01".to_string(),
            output_price: "0.02".to_string(),
            enabled: true,
        }],
    };
    service.save_model_settings(valid.clone()).unwrap();

    let mut invalid = valid;
    invalid.models[0].context_window_tokens = Some(0);
    assert!(service.save_model_settings(invalid).is_err());

    let stored = service.load_model_settings().unwrap().unwrap();
    assert_eq!(stored.models[0].context_window_tokens, Some(128_000));
}

#[test]
fn requires_model_connection_overrides_to_be_saved_as_a_complete_pair() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let valid = ModelSettingsRecord {
        api_url: "https://global.example/v1".to_string(),
        api_token: "global-token".to_string(),
        search_mode: "disabled".to_string(),
        tavily_api_key: String::new(),
        models: vec![ModelConfigRecord {
            id: "model-a".to_string(),
            display_name: "Model A".to_string(),
            api_url_override: Some("https://model.example/v1".to_string()),
            api_token_override: Some("model-token".to_string()),
            supports_image: false,
            context_window_tokens: Some(128_000),
            input_price: "0.01".to_string(),
            output_price: "0.02".to_string(),
            enabled: true,
        }],
    };
    service.save_model_settings(valid.clone()).unwrap();

    let mut invalid = valid;
    invalid.models[0].api_token_override = None;
    assert!(service.save_model_settings(invalid).is_err());

    let stored = service.load_model_settings().unwrap().unwrap();
    assert_eq!(
        stored.models[0].api_url_override.as_deref(),
        Some("https://model.example/v1")
    );
    assert_eq!(
        stored.models[0].api_token_override.as_deref(),
        Some("model-token")
    );
}

#[test]
fn skill_enablement_defaults_to_true_and_persists_explicit_overrides() {
    let fixture = StorageFixture::new();
    let first = "bundled:application:documents".to_string();
    let second = "installed:user:01234567-89ab-4def-8123-456789abcdef".to_string();

    {
        let service = fixture.service();
        let initial = service
            .load_skill_enablement(&[first.clone(), second.clone(), first.clone()])
            .unwrap();
        assert_eq!(initial.len(), 2);
        assert_eq!(initial.get(&first), Some(&true));
        assert_eq!(initial.get(&second), Some(&true));

        assert!(service
            .set_skill_enablement_override(&first, false)
            .unwrap());
        assert!(!service
            .set_skill_enablement_override(&first, false)
            .unwrap());
        assert!(service
            .set_skill_enablement_override(&second, true)
            .unwrap());
    }

    let reopened = fixture.service();
    let persisted = reopened
        .load_skill_enablement(&[second.clone(), first.clone()])
        .unwrap();
    assert_eq!(persisted.get(&first), Some(&false));
    assert_eq!(persisted.get(&second), Some(&true));

    assert!(reopened.delete_skill_enablement_override(&first).unwrap());
    assert!(!reopened.delete_skill_enablement_override(&first).unwrap());
    assert_eq!(
        reopened
            .load_skill_enablement(std::slice::from_ref(&first))
            .unwrap()
            .get(&first),
        Some(&true)
    );
}

#[test]
fn skill_enablement_rejects_empty_and_oversized_ids_before_storage() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let oversized = "x".repeat(MAX_SKILL_ENABLEMENT_ID_BYTES + 1);

    assert!(service.set_skill_enablement_override("", false).is_err());
    assert!(service.delete_skill_enablement_override("").is_err());
    assert!(service
        .load_skill_enablement(&["valid:id".to_string(), String::new()])
        .is_err());
    assert!(service
        .set_skill_enablement_override(&oversized, false)
        .is_err());
    assert!(service
        .delete_skill_enablement_override(&oversized)
        .is_err());

    let maximum = "x".repeat(MAX_SKILL_ENABLEMENT_ID_BYTES);
    assert!(service
        .set_skill_enablement_override(&maximum, false)
        .unwrap());
    assert_eq!(
        service
            .load_skill_enablement(std::slice::from_ref(&maximum))
            .unwrap()
            .get(&maximum),
        Some(&false)
    );
}
