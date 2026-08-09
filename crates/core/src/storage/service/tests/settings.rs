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
            provider_profile_config: None,
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
    assert_eq!(
        first.provider_connection_revisions, second.provider_connection_revisions,
        "an identical save must preserve each effective provider connection identity"
    );
    assert!(first
        .provider_connection_revisions
        .values()
        .all(
            |revision| crate::storage::config_repository::is_provider_connection_revision(revision)
        ));
    assert_eq!(
        first.provider_protocol_revisions, second.provider_protocol_revisions,
        "an identical save must preserve each effective provider protocol identity"
    );
    assert!(first.provider_protocol_revisions.values().all(|revision| {
        crate::storage::config_repository::is_provider_protocol_revision(revision)
    }));
    assert!(first
        .provider_protocol_revisions
        .values()
        .all(|revision| revision.starts_with("provider-protocol-v1:")));
    assert_eq!(
        first.search_connection_revision, second.search_connection_revision,
        "an identical save must preserve the search connection identity"
    );
    assert_eq!(first.settings.models[0].id, second.settings.models[0].id);
    assert_eq!(format!("{first:?}"), "ModelSettingsSnapshot([REDACTED])");
}

#[test]
fn provider_connection_revisions_follow_only_each_models_effective_connection() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let mut settings = revision_test_settings();
    settings.models.push(ModelConfigRecord {
        id: "override-model".to_string(),
        display_name: "Override Model".to_string(),
        api_url_override: Some("https://override.example/v1".to_string()),
        api_token_override: Some("override-token".to_string()),
        supports_image: false,
        context_window_tokens: Some(128_000),
        provider_profile_config: None,
        input_price: "0".to_string(),
        output_price: "0".to_string(),
        enabled: true,
    });
    service.save_model_settings(settings.clone()).unwrap();
    let initial = service.load_model_settings_snapshot().unwrap().unwrap();

    settings.api_token = "rotated-global-token".to_string();
    settings.models[0].display_name = "Metadata-only rename".to_string();
    settings.models[1].input_price = "1.25".to_string();
    settings.models[1].provider_profile_config =
        Some(crate::ProviderProfileConfig::generic_for_dialect(
            crate::ProviderProtocolDialect::OpenAiChatCompletions,
        ));
    service.save_model_settings(settings.clone()).unwrap();
    let after_global_change = service.load_model_settings_snapshot().unwrap().unwrap();

    assert_ne!(
        initial.provider_connection_revisions["revision-model"],
        after_global_change.provider_connection_revisions["revision-model"],
        "the inheriting model must rotate when its global token changes"
    );
    assert_eq!(
        initial.provider_connection_revisions["override-model"],
        after_global_change.provider_connection_revisions["override-model"],
        "a complete model override is isolated from global connection changes"
    );

    settings.models[1].api_url_override = Some("https://override-2.example/v1".to_string());
    service.save_model_settings(settings).unwrap();
    let after_override_change = service.load_model_settings_snapshot().unwrap().unwrap();
    assert_eq!(
        after_global_change.provider_connection_revisions["revision-model"],
        after_override_change.provider_connection_revisions["revision-model"],
        "editing another model's override must not rotate this model"
    );
    assert_ne!(
        after_global_change.provider_connection_revisions["override-model"],
        after_override_change.provider_connection_revisions["override-model"]
    );
}

#[test]
fn profile_price_and_search_edits_rotate_only_their_owned_connection_identity() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let mut settings = revision_test_settings();
    service.save_model_settings(settings.clone()).unwrap();
    let initial = service.load_model_settings_snapshot().unwrap().unwrap();

    settings.models[0].provider_profile_config =
        Some(crate::ProviderProfileConfig::generic_for_dialect(
            crate::ProviderProtocolDialect::OpenAiChatCompletions,
        ));
    settings.models[0].output_price = "7.5".to_string();
    service.save_model_settings(settings.clone()).unwrap();
    let metadata_edit = service.load_model_settings_snapshot().unwrap().unwrap();
    assert_eq!(
        initial.provider_connection_revisions,
        metadata_edit.provider_connection_revisions
    );
    assert_eq!(
        initial.provider_protocol_revisions, metadata_edit.provider_protocol_revisions,
        "omitted Generic and explicit Generic are the same effective protocol"
    );
    assert_eq!(
        initial.search_connection_revision,
        metadata_edit.search_connection_revision
    );

    settings.search_mode = "tavily".to_string();
    settings.tavily_api_key = "search-token".to_string();
    service.save_model_settings(settings).unwrap();
    let search_edit = service.load_model_settings_snapshot().unwrap().unwrap();
    assert_eq!(
        metadata_edit.provider_connection_revisions,
        search_edit.provider_connection_revisions
    );
    assert_eq!(
        metadata_edit.provider_protocol_revisions,
        search_edit.provider_protocol_revisions
    );
    assert_ne!(
        metadata_edit.search_connection_revision,
        search_edit.search_connection_revision
    );
}

#[test]
fn provider_protocol_revision_tracks_only_the_selected_models_effective_wire_contract() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let mut settings = revision_test_settings();
    settings.models.push(ModelConfigRecord {
        id: "other-model".to_string(),
        display_name: "Other Model".to_string(),
        api_url_override: Some("https://other.example/v1".to_string()),
        api_token_override: Some("other-token".to_string()),
        supports_image: false,
        context_window_tokens: Some(64_000),
        provider_profile_config: None,
        input_price: "0".to_string(),
        output_price: "0".to_string(),
        enabled: true,
    });
    service.save_model_settings(settings.clone()).unwrap();
    let initial = service.load_model_settings_snapshot().unwrap().unwrap();
    let selected_initial = initial.provider_protocol_revisions["revision-model"].clone();

    settings.models[0].display_name = "Renamed only".to_string();
    settings.models[0].supports_image = true;
    settings.models[0].context_window_tokens = Some(256_000);
    settings.models[0].input_price = "1.25".to_string();
    settings.models[0].output_price = "2.5".to_string();
    settings.search_mode = "tavily".to_string();
    settings.tavily_api_key = "search-only-token".to_string();
    settings.models[1].api_url_override =
        Some("https://other-changed.example/v1/chat/completions".to_string());
    settings.models[1].api_token_override = Some("other-changed-token".to_string());
    settings.models[1].provider_profile_config =
        Some(crate::ProviderProfileConfig::deepseek_v4_default());
    service.save_model_settings(settings.clone()).unwrap();
    let metadata_and_other_model = service.load_model_settings_snapshot().unwrap().unwrap();
    assert_eq!(
        metadata_and_other_model.provider_protocol_revisions["revision-model"], selected_initial,
        "metadata, search and another model must not rotate this model's protocol identity"
    );
    assert_ne!(
        metadata_and_other_model.provider_protocol_revisions["other-model"],
        initial.provider_protocol_revisions["other-model"]
    );

    settings.models[0].provider_profile_config =
        Some(crate::ProviderProfileConfig::deepseek_v4_default());
    service.save_model_settings(settings.clone()).unwrap();
    let deepseek_default = service.load_model_settings_snapshot().unwrap().unwrap();
    assert_ne!(
        deepseek_default.provider_protocol_revisions["revision-model"], selected_initial,
        "changing the selected Profile must rotate its protocol identity"
    );

    let mut thinking = crate::ProviderProfileConfig::deepseek_v4_default();
    thinking.reasoning.mode = crate::ReasoningMode::Enabled;
    thinking.reasoning.effort = crate::ReasoningEffort::High;
    settings.models[0].provider_profile_config = Some(thinking);
    service.save_model_settings(settings.clone()).unwrap();
    let deepseek_thinking = service.load_model_settings_snapshot().unwrap().unwrap();
    assert_ne!(
        deepseek_thinking.provider_protocol_revisions["revision-model"],
        deepseek_default.provider_protocol_revisions["revision-model"],
        "changing reasoning controls must rotate the protocol identity"
    );

    settings.api_token = " rotated-provider-token ".to_string();
    service.save_model_settings(settings.clone()).unwrap();
    let connection_changed = service.load_model_settings_snapshot().unwrap().unwrap();
    assert_ne!(
        connection_changed.provider_protocol_revisions["revision-model"],
        deepseek_thinking.provider_protocol_revisions["revision-model"],
        "changing the effective provider credential must rotate the protocol identity"
    );

    settings.api_url = "https://api.anthropic.com/v1/messages".to_string();
    settings.models[0].provider_profile_config =
        Some(crate::ProviderProfileConfig::generic_for_dialect(
            crate::ProviderProtocolDialect::AnthropicMessages,
        ));
    service.save_model_settings(settings.clone()).unwrap();
    let endpoint_and_dialect_changed = service.load_model_settings_snapshot().unwrap().unwrap();
    assert_ne!(
        endpoint_and_dialect_changed.provider_protocol_revisions["revision-model"],
        connection_changed.provider_protocol_revisions["revision-model"],
        "changing the endpoint/dialect must rotate the protocol identity"
    );

    settings.models[0].id = "revision-model-v2".to_string();
    service.save_model_settings(settings).unwrap();
    let wire_model_changed = service.load_model_settings_snapshot().unwrap().unwrap();
    assert!(!wire_model_changed
        .provider_protocol_revisions
        .contains_key("revision-model"));
    assert_ne!(
        wire_model_changed.provider_protocol_revisions["revision-model-v2"],
        endpoint_and_dialect_changed.provider_protocol_revisions["revision-model"],
        "changing the provider wire model id must create a new protocol identity"
    );
}

#[test]
fn provider_profile_config_round_trips_through_model_storage() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let mut settings = revision_test_settings();
    settings.models[0].provider_profile_config =
        Some(crate::ProviderProfileConfig::deepseek_v4_default());

    service.save_model_settings(settings).unwrap();
    let stored = service.load_model_settings().unwrap().unwrap();

    assert_eq!(
        stored.models[0].provider_profile_config,
        Some(crate::ProviderProfileConfig::deepseek_v4_default())
    );
}

#[test]
fn legacy_client_omission_preserves_an_existing_explicit_provider_profile() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let mut explicit = revision_test_settings();
    explicit.models[0].provider_profile_config =
        Some(crate::ProviderProfileConfig::deepseek_v4_default());
    service.save_model_settings(explicit).unwrap();

    let legacy_payload = revision_test_settings();
    assert!(legacy_payload.models[0].provider_profile_config.is_none());
    service.save_model_settings(legacy_payload).unwrap();

    let stored = service.load_model_settings().unwrap().unwrap();
    assert_eq!(
        stored.models[0].provider_profile_config,
        Some(crate::ProviderProfileConfig::deepseek_v4_default())
    );
}

#[test]
fn explicit_generic_profile_can_replace_a_provider_specific_profile() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let mut settings = revision_test_settings();
    settings.models[0].provider_profile_config =
        Some(crate::ProviderProfileConfig::deepseek_v4_default());
    service.save_model_settings(settings.clone()).unwrap();

    settings.models[0].provider_profile_config =
        Some(crate::ProviderProfileConfig::generic_for_dialect(
            crate::ProviderProtocolDialect::OpenAiChatCompletions,
        ));
    service.save_model_settings(settings.clone()).unwrap();

    let stored = service.load_model_settings().unwrap().unwrap();
    assert_eq!(
        stored.models[0].provider_profile_config,
        settings.models[0].provider_profile_config
    );
}

#[test]
fn invalid_persisted_provider_profile_fails_closed() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    service
        .save_model_settings(revision_test_settings())
        .unwrap();

    let connection = rusqlite::Connection::open(fixture.root.join("storage.sqlite")).unwrap();
    connection
        .execute(
            "UPDATE models SET provider_profile_config_json = ?1 WHERE id = 'revision-model'",
            [r#"{"schemaVersion":1,"profile":{"id":"future_profile","version":1},"reasoning":{"mode":"provider_default","effort":"provider_default"}}"#],
        )
        .unwrap();

    assert!(service.load_model_settings().is_err());
}

#[test]
fn invalid_persisted_provider_protocol_revision_fails_closed() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    service
        .save_model_settings(revision_test_settings())
        .unwrap();

    let connection = rusqlite::Connection::open(fixture.root.join("storage.sqlite")).unwrap();
    connection
        .execute(
            "UPDATE models SET provider_protocol_revision = 'provider-protocol-v1:not-a-uuid'
             WHERE id = 'revision-model'",
            [],
        )
        .unwrap();

    assert!(service.load_model_settings_snapshot().is_err());
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
            provider_profile_config: None,
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
            provider_profile_config: None,
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
            provider_profile_config: None,
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
