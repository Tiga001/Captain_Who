use super::*;

fn revision_test_settings() -> ModelSettingsRecord {
    ModelSettingsRecord {
        api_url: "https://revision.example/v1".to_string(),
        api_token: "fixed-revision-test-token".to_string(),
        search_mode: "disabled".to_string(),
        tavily_api_key: String::new(),
        models: vec![ModelConfigRecord {
            id: "revision-model".to_string(),
            provider_model_id: "revision-model".to_string(),
            display_name: "Revision Model".to_string(),
            api_url_override: None,
            api_token_override: None,
            supports_image: false,
            context_window_tokens: Some(128_000),
            provider_profile_config: crate::ProviderProfileConfig::generic_for_dialect(
                crate::ProviderProtocolDialect::OpenAiChatCompletions,
            ),
            input_price: "0".to_string(),
            cached_input_price: String::new(),
            output_price: "0".to_string(),
            enabled: true,
        }],
    }
}

fn renderer_save_request(
    settings: &ModelSettingsRecord,
    update: serde_json::Value,
    config_id: Option<&str>,
) -> ModelSettingsSaveRequest {
    let mut value = serde_json::to_value(settings).unwrap();
    let model = value["models"][0].as_object_mut().unwrap();
    model.remove("providerProfileConfig");
    model.insert("providerProfileUpdate".to_string(), update);
    model.insert(
        "id".to_string(),
        config_id
            .map(|value| serde_json::Value::String(value.to_string()))
            .unwrap_or(serde_json::Value::Null),
    );
    serde_json::from_value(value).unwrap()
}

fn official_profile_test_model(
    id: &str,
    api_url_override: Option<&str>,
    api_token_override: Option<&str>,
) -> ModelConfigRecord {
    ModelConfigRecord {
        id: id.to_string(),
        provider_model_id: id.to_string(),
        display_name: format!("Test {id}"),
        api_url_override: api_url_override.map(ToString::to_string),
        api_token_override: api_token_override.map(ToString::to_string),
        supports_image: false,
        context_window_tokens: Some(128_000),
        provider_profile_config: crate::ProviderProfileConfig::generic_for_dialect(
            crate::ProviderProtocolDialect::OpenAiChatCompletions,
        ),
        input_price: "1.25".to_string(),
        cached_input_price: "0.25".to_string(),
        output_price: "2.5".to_string(),
        enabled: true,
    }
}

#[test]
fn duplicate_display_name_is_a_typed_validation_error_with_the_trimmed_name() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let settings = revision_test_settings();
    let mut request = renderer_save_request(
        &settings,
        serde_json::json!({"kind": "select_generic"}),
        None,
    );
    request.models[0].display_name = "  Duplicate Model  ".to_string();
    request.models.push(request.models[0].clone());

    let error = service.save_model_settings_request(request).unwrap_err();
    assert_eq!(
        error,
        ModelSettingsSaveError::DuplicateDisplayName {
            display_name: "Duplicate Model".to_string(),
        }
    );
    assert!(service.load_model_settings().unwrap().is_none());
}

#[test]
fn oversized_display_name_is_rejected_before_duplicate_error_projection() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let settings = revision_test_settings();
    let mut request = renderer_save_request(
        &settings,
        serde_json::json!({"kind": "select_generic"}),
        None,
    );
    request.models[0].display_name =
        "x".repeat(crate::storage::models::MODEL_DISPLAY_NAME_MAX_BYTES + 1);
    request.models.push(request.models[0].clone());

    let error = service.save_model_settings_request(request).unwrap_err();
    assert!(matches!(error, ModelSettingsSaveError::Other(_)));
    assert!(service.load_model_settings().unwrap().is_none());
}

#[test]
fn display_name_at_the_512_byte_boundary_is_accepted() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let settings = revision_test_settings();
    let mut request = renderer_save_request(
        &settings,
        serde_json::json!({"kind": "select_generic"}),
        None,
    );
    request.models[0].display_name =
        "x".repeat(crate::storage::models::MODEL_DISPLAY_NAME_MAX_BYTES);

    let saved = service.save_model_settings_request(request).unwrap();

    assert_eq!(saved.models[0].display_name.len(), 512);
}

#[test]
fn renaming_a_model_onto_an_existing_display_name_returns_the_typed_collision() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let mut settings = revision_test_settings();
    let mut second = settings.models[0].clone();
    second.id = "existing-model".to_string();
    second.display_name = "Existing Model".to_string();
    settings.models.push(second);
    service.save_model_settings(settings.clone()).unwrap();

    let mut value = serde_json::to_value(&settings).unwrap();
    let models = value["models"].as_array_mut().unwrap();
    for model in models.iter_mut() {
        let model = model.as_object_mut().unwrap();
        model.remove("providerProfileConfig");
        model.insert(
            "providerProfileUpdate".to_string(),
            serde_json::json!({"kind": "select_generic"}),
        );
    }
    models[0]["displayName"] = serde_json::json!("Existing Model");
    let request: ModelSettingsSaveRequest = serde_json::from_value(value).unwrap();

    assert_eq!(
        service.save_model_settings_request(request).unwrap_err(),
        ModelSettingsSaveError::DuplicateDisplayName {
            display_name: "Existing Model".to_string(),
        }
    );
    let unchanged = service.load_model_settings().unwrap().unwrap();
    assert_eq!(unchanged.models[0].id, "revision-model");
    assert_eq!(unchanged.models[1].id, "existing-model");
}

#[test]
fn identical_provider_models_can_be_saved_as_distinct_configurations() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let mut settings = revision_test_settings();
    let mut second = settings.models[0].clone();
    second.id = "client-supplied-id-is-ignored".to_string();
    second.display_name = "Secondary".to_string();
    settings.models.push(second);

    let mut value = serde_json::to_value(&settings).unwrap();
    for model in value["models"].as_array_mut().unwrap() {
        let model = model.as_object_mut().unwrap();
        model.insert("id".to_string(), serde_json::Value::Null);
        model.remove("providerProfileConfig");
        model.insert(
            "providerProfileUpdate".to_string(),
            serde_json::json!({"kind": "select_generic"}),
        );
    }
    let request: ModelSettingsSaveRequest = serde_json::from_value(value).unwrap();
    let saved = service.save_model_settings_request(request).unwrap();

    assert_eq!(saved.models.len(), 2);
    assert_ne!(saved.models[0].id, saved.models[1].id);
    assert!(saved
        .models
        .iter()
        .all(|model| model.id.starts_with("model-config:")));
    assert_eq!(
        saved.models[0].provider_model_id,
        saved.models[1].provider_model_id
    );
    assert_eq!(
        saved.models[0].api_url_override,
        saved.models[1].api_url_override
    );
    assert_eq!(
        saved.models[0].api_token_override,
        saved.models[1].api_token_override
    );
    assert_eq!(
        saved.models[0].provider_profile_config,
        saved.models[1].provider_profile_config
    );
}

#[test]
fn normalized_display_name_collision_is_typed_and_does_not_overwrite_storage() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let mut settings = revision_test_settings();
    settings.models[0].display_name = "Kimi K3".to_string();
    service.save_model_settings(settings.clone()).unwrap();

    let mut colliding = settings.models[0].clone();
    colliding.id = "new-client-placeholder".to_string();
    colliding.display_name = "  ＫＩＭＩ　Ｋ３  ".to_string();
    colliding.provider_model_id = "unrecognized-provider-model".to_string();
    settings.models.push(colliding);
    let mut value = serde_json::to_value(&settings).unwrap();
    let models = value["models"].as_array_mut().unwrap();
    for (index, model) in models.iter_mut().enumerate() {
        let model = model.as_object_mut().unwrap();
        model.insert(
            "id".to_string(),
            if index == 0 {
                serde_json::json!("revision-model")
            } else {
                serde_json::Value::Null
            },
        );
        model.remove("providerProfileConfig");
        model.insert(
            "providerProfileUpdate".to_string(),
            if index == 0 {
                serde_json::json!({"kind": "unchanged"})
            } else {
                serde_json::json!({
                    "kind": "select_vendor",
                    "vendorId": "moonshot",
                    "settings": {
                        "kind": "moonshot_k3_chat",
                        "reasoningEffort": "max"
                    }
                })
            },
        );
    }
    let request: ModelSettingsSaveRequest = serde_json::from_value(value).unwrap();

    assert_eq!(
        service.save_model_settings_request(request).unwrap_err(),
        ModelSettingsSaveError::DuplicateDisplayName {
            display_name: "ＫＩＭＩ　Ｋ３".to_string(),
        }
    );
    let unchanged = service.load_model_settings().unwrap().unwrap();
    assert_eq!(unchanged.models.len(), 1);
    assert_eq!(unchanged.models[0].id, "revision-model");
    assert_eq!(unchanged.models[0].display_name, "Kimi K3");
}

#[test]
fn editing_display_name_preserves_config_identity_and_conversation_reference() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let settings = revision_test_settings();
    service.save_model_settings(settings.clone()).unwrap();
    let mut stored_conversation = conversation("conversation-model-id", None, "message-model-id");
    stored_conversation.model_id = Some("revision-model".to_string());
    service.save_conversation(stored_conversation).unwrap();

    let mut edited = settings;
    edited.models[0].display_name = "Renamed display name".to_string();
    let saved = service
        .save_model_settings_request(renderer_save_request(
            &edited,
            serde_json::json!({"kind": "unchanged"}),
            Some("revision-model"),
        ))
        .unwrap();

    assert_eq!(saved.models[0].id, "revision-model");
    assert_eq!(saved.models[0].display_name, "Renamed display name");
    assert_eq!(
        service
            .load_conversation("conversation-model-id")
            .unwrap()
            .unwrap()
            .model_id
            .as_deref(),
        Some("revision-model")
    );
}

#[test]
fn registered_profile_selection_is_host_versioned_normalized_and_authoritative() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let settings = revision_test_settings();
    let saved = service
        .save_model_settings_request(renderer_save_request(
            &settings,
            serde_json::json!({
                "kind": "select_registered_profile",
                "profileId": "deepseek_v4_chat",
                "settings": {
                    "kind": "deepseek_v4_chat",
                    "reasoning": {"mode": "disabled", "effort": "max"}
                }
            }),
            None,
        ))
        .unwrap();

    let config = &saved.models[0].provider_profile_config;
    assert_eq!(
        config.profile(),
        crate::ProviderProfileRef::deepseek_v4_chat()
    );
    let reasoning = config
        .legacy_reasoning()
        .expect("legacy registered selection must remain schema v1");
    assert_eq!(reasoning.mode, crate::ReasoningMode::Disabled);
    assert_eq!(reasoning.effort, crate::ReasoningEffort::ProviderDefault);
    assert_eq!(
        service.load_model_settings().unwrap().unwrap().models[0].provider_profile_config,
        saved.models[0].provider_profile_config
    );
}

#[test]
fn vendor_selection_resolves_moonshot_family_and_persists_v2_settings() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let mut settings = revision_test_settings();
    settings.models[0].provider_model_id = "kimi-k3".to_string();
    settings.models[0].display_name = "Kimi K3".to_string();

    let saved = service
        .save_model_settings_request(renderer_save_request(
            &settings,
            serde_json::json!({
                "kind": "select_vendor",
                "vendorId": "moonshot",
                "settings": {
                    "kind": "moonshot_k3_chat",
                    "reasoningEffort": "low"
                }
            }),
            None,
        ))
        .unwrap();

    assert_eq!(
        serde_json::to_value(&saved.models[0].provider_profile_config).unwrap(),
        serde_json::json!({
            "schemaVersion": 2,
            "profile": {"id": "moonshot_k3_chat", "version": 1},
            "vendorId": "moonshot",
            "settings": {
                "kind": "moonshot_k3_chat",
                "reasoningEffort": "low"
            }
        })
    );
    assert_eq!(
        service.load_model_settings().unwrap().unwrap().models[0].provider_profile_config,
        saved.models[0].provider_profile_config
    );
}

#[test]
fn vendor_selection_fails_closed_for_unknown_model_and_mismatched_family_settings() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let mut settings = revision_test_settings();
    settings.models[0].provider_model_id = "kimi-future".to_string();

    let unknown = renderer_save_request(
        &settings,
        serde_json::json!({
            "kind": "select_vendor",
            "vendorId": "moonshot",
            "settings": {
                "kind": "moonshot_k3_chat",
                "reasoningEffort": "max"
            }
        }),
        None,
    );
    assert!(service.save_model_settings_request(unknown).is_err());
    assert!(service.load_model_settings().unwrap().is_none());

    settings.models[0].provider_model_id = "kimi-k3".to_string();
    let mismatched = renderer_save_request(
        &settings,
        serde_json::json!({
            "kind": "select_vendor",
            "vendorId": "moonshot",
            "settings": {
                "kind": "moonshot_k2_6_chat",
                "thinkingMode": "provider_default"
            }
        }),
        None,
    );
    assert!(service.save_model_settings_request(mismatched).is_err());
    assert!(service.load_model_settings().unwrap().is_none());
}

#[test]
fn generic_vendor_selection_keeps_custom_aliases_on_generic_compatibility() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let mut settings = revision_test_settings();
    settings.models[0].provider_model_id = "private-company-alias".to_string();

    let saved = service
        .save_model_settings_request(renderer_save_request(
            &settings,
            serde_json::json!({
                "kind": "select_vendor",
                "vendorId": "generic",
                "settings": {"kind": "generic"}
            }),
            None,
        ))
        .unwrap();

    assert_eq!(
        serde_json::to_value(&saved.models[0].provider_profile_config).unwrap(),
        serde_json::json!({
            "schemaVersion": 2,
            "profile": {"id": "generic_openai_chat", "version": 1},
            "vendorId": "generic",
            "settings": {"kind": "generic"}
        })
    );
}

#[test]
fn save_boundary_corrects_exact_official_deepseek_flash_and_moonshot_k3_profiles() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let mut deepseek = official_profile_test_model("deepseek-v4-flash", None, None);
    deepseek.display_name = "DeepSeek Flash metadata".to_string();
    deepseek.context_window_tokens = Some(96_000);
    let mut moonshot = official_profile_test_model(
        "kimi-k3",
        Some("https://api.moonshot.cn/v1/chat/completions"),
        Some("moonshot-model-token"),
    );
    moonshot.display_name = "Kimi K3 metadata".to_string();
    moonshot.supports_image = true;
    moonshot.context_window_tokens = Some(64_000);
    moonshot.input_price = "3.75".to_string();
    let settings = ModelSettingsRecord {
        api_url: "https://api.deepseek.com/v1/chat/completions".to_string(),
        api_token: "deepseek-global-token".to_string(),
        search_mode: "tavily".to_string(),
        tavily_api_key: "search-token".to_string(),
        models: vec![deepseek, moonshot],
    };
    let mut expected = settings.clone();
    expected.models[0].provider_profile_config = crate::ProviderProfileConfig::from_family_settings(
        crate::ProviderProfileRef::deepseek_v4_chat(),
        crate::ProviderVendorId::DeepSeek,
        crate::ProviderFamilySettings::DeepseekV4Chat {
            reasoning: crate::ProviderFamilyReasoningPolicy::provider_default(),
        },
    );
    expected.models[1].provider_profile_config = crate::ProviderProfileConfig::from_family_settings(
        crate::ProviderProfileRef::moonshot_k3_chat(),
        crate::ProviderVendorId::Moonshot,
        crate::ProviderFamilySettings::MoonshotK3Chat {
            reasoning_effort: crate::ProviderReasoningEffort::Max,
        },
    );

    service.save_model_settings(settings).unwrap();
    let stored = service.load_model_settings().unwrap().unwrap();

    assert_eq!(
        serde_json::to_value(stored).unwrap(),
        serde_json::to_value(expected).unwrap(),
        "profile-only correction must retain credentials, search settings and every model field"
    );
}

#[test]
fn save_boundary_does_not_infer_official_profiles_from_proxies_aliases_or_custom_models() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let settings = ModelSettingsRecord {
        api_url: "https://deepseek-proxy.example/v1/chat/completions".to_string(),
        api_token: "proxy-global-token".to_string(),
        search_mode: "disabled".to_string(),
        tavily_api_key: "preserved-unused-search-token".to_string(),
        models: vec![
            official_profile_test_model("deepseek-v4-flash", None, None),
            official_profile_test_model(
                "deepseek-v4-flash-latest",
                Some("https://api.deepseek.com/v1/chat/completions"),
                Some("deepseek-alias-token"),
            ),
            official_profile_test_model(
                "kimi-k3",
                Some("https://moonshot-proxy.example/v1/chat/completions"),
                Some("moonshot-proxy-token"),
            ),
            official_profile_test_model(
                "kimi-k3-latest",
                Some("https://api.moonshot.ai/v1/chat/completions"),
                Some("moonshot-alias-token"),
            ),
            official_profile_test_model(
                "company-private-model",
                Some("https://api.moonshot.cn/v1/chat/completions"),
                Some("custom-model-token"),
            ),
        ],
    };
    let expected = serde_json::to_value(&settings).unwrap();

    service.save_model_settings(settings).unwrap();
    let stored = service.load_model_settings().unwrap().unwrap();

    assert_eq!(serde_json::to_value(stored).unwrap(), expected);
}

#[test]
fn save_boundary_does_not_correct_an_incomplete_global_connection() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let settings = ModelSettingsRecord {
        api_url: "https://api.deepseek.com/v1/chat/completions".to_string(),
        api_token: "   ".to_string(),
        search_mode: "disabled".to_string(),
        tavily_api_key: "preserved-search-token".to_string(),
        models: vec![official_profile_test_model("deepseek-v4-flash", None, None)],
    };
    let expected = serde_json::to_value(&settings).unwrap();

    service.save_model_settings(settings).unwrap();
    let stored = service.load_model_settings().unwrap().unwrap();

    assert_eq!(serde_json::to_value(stored).unwrap(), expected);
}

#[test]
fn startup_never_rewrites_historical_official_provider_profiles() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let model = official_profile_test_model(
        "kimi-k3",
        Some("https://moonshot-proxy.example/v1/chat/completions"),
        Some("moonshot-model-token"),
    );
    service
        .save_model_settings(ModelSettingsRecord {
            api_url: "https://global-proxy.example/v1/chat/completions".to_string(),
            api_token: "global-provider-token".to_string(),
            search_mode: "tavily".to_string(),
            tavily_api_key: "search-provider-token".to_string(),
            models: vec![model],
        })
        .unwrap();

    {
        let connection = service.state.connection().unwrap();
        connection
            .execute(
                "UPDATE models SET api_url_override = ?1 WHERE id = 'kimi-k3'",
                ["https://api.moonshot.ai/v1/chat/completions"],
            )
            .unwrap();
    }
    let before = service.load_model_settings_snapshot().unwrap().unwrap();
    drop(service);

    let reopened = fixture.service();
    let after = reopened.load_model_settings_snapshot().unwrap().unwrap();
    assert_eq!(
        serde_json::to_value(after.settings).unwrap(),
        serde_json::to_value(before.settings).unwrap()
    );
    assert_eq!(after.configuration_revision, before.configuration_revision);
    assert_eq!(
        after.provider_connection_revisions,
        before.provider_connection_revisions
    );
    assert_eq!(
        after.provider_protocol_revisions,
        before.provider_protocol_revisions
    );
    assert_eq!(
        after.search_connection_revision,
        before.search_connection_revision
    );
}

#[test]
fn startup_and_unrelated_save_keep_a_future_profile_opaque() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    service
        .save_model_settings(ModelSettingsRecord {
            api_url: "https://api.deepseek.com/v1/chat/completions".to_string(),
            api_token: "deepseek-global-token".to_string(),
            search_mode: "tavily".to_string(),
            tavily_api_key: "search-token".to_string(),
            models: vec![official_profile_test_model("deepseek-v4-flash", None, None)],
        })
        .unwrap();

    let future_profile = serde_json::json!({
        "schemaVersion": 2,
        "profile": {"id": "deepseek_v4_chat", "version": 99},
        "vendorId": "deepseek",
        "settings": {
            "kind": "deepseek_v4_chat",
            "reasoning": {"mode": "enabled", "effort": "low"}
        }
    });
    {
        let connection = service.state.connection().unwrap();
        connection
            .execute(
                "UPDATE models SET provider_profile_config_json = ?1 WHERE id = 'deepseek-v4-flash'",
                [future_profile.to_string()],
            )
            .unwrap();
    }
    let before = service.load_model_settings_snapshot().unwrap().unwrap();
    drop(service);

    let reopened = fixture.service();
    let after_startup = reopened.load_model_settings_snapshot().unwrap().unwrap();
    assert_eq!(
        serde_json::to_value(&after_startup.settings.models[0].provider_profile_config).unwrap(),
        future_profile
    );
    assert_eq!(
        after_startup.configuration_revision,
        before.configuration_revision
    );
    assert_eq!(
        after_startup.provider_connection_revisions,
        before.provider_connection_revisions
    );
    assert_eq!(
        after_startup.provider_protocol_revisions,
        before.provider_protocol_revisions
    );
    assert_eq!(
        after_startup.search_connection_revision,
        before.search_connection_revision
    );

    let mut price_edit = after_startup.settings.clone();
    price_edit.models[0].input_price = "9.5".to_string();
    reopened
        .save_model_settings_request(renderer_save_request(
            &price_edit,
            serde_json::json!({"kind": "unchanged"}),
            Some(&price_edit.models[0].id),
        ))
        .unwrap();
    let after_save = reopened.load_model_settings_snapshot().unwrap().unwrap();
    assert_eq!(
        serde_json::to_value(&after_save.settings.models[0].provider_profile_config).unwrap(),
        future_profile
    );
    assert_ne!(
        after_save.configuration_revision,
        after_startup.configuration_revision
    );
    assert_eq!(
        after_save.provider_connection_revisions,
        after_startup.provider_connection_revisions
    );
    assert_eq!(
        after_save.provider_protocol_revisions,
        after_startup.provider_protocol_revisions
    );
    assert_eq!(
        after_save.search_connection_revision,
        after_startup.search_connection_revision
    );
}

#[test]
fn authoritative_profile_updates_rotate_only_effective_wire_changes() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let settings = revision_test_settings();
    let initial = service
        .save_model_settings_request(renderer_save_request(
            &settings,
            serde_json::json!({
                "kind": "select_registered_profile",
                "profileId": "deepseek_v4_chat",
                "settings": {
                    "kind": "deepseek_v4_chat",
                    "reasoning": {"mode": "enabled", "effort": "high"}
                }
            }),
            None,
        ))
        .unwrap();
    let initial_id = initial.models[0].id.clone();
    let initial_revision = service
        .load_model_settings_snapshot()
        .unwrap()
        .unwrap()
        .provider_protocol_revisions[&initial_id]
        .clone();

    let mut metadata = initial.clone();
    metadata.models[0].display_name = "Metadata only".to_string();
    metadata.models[0].supports_image = true;
    metadata.models[0].input_price = "2".to_string();
    service
        .save_model_settings_request(renderer_save_request(
            &metadata,
            serde_json::json!({"kind": "unchanged"}),
            Some(&metadata.models[0].id),
        ))
        .unwrap();
    assert_eq!(
        service
            .load_model_settings_snapshot()
            .unwrap()
            .unwrap()
            .provider_protocol_revisions[&initial_id],
        initial_revision
    );

    let changed = service
        .save_model_settings_request(renderer_save_request(
            &metadata,
            serde_json::json!({
                "kind": "select_registered_profile",
                "profileId": "deepseek_v4_chat",
                "settings": {
                    "kind": "deepseek_v4_chat",
                    "reasoning": {"mode": "enabled", "effort": "max"}
                }
            }),
            Some(&metadata.models[0].id),
        ))
        .unwrap();
    let changed_snapshot = service.load_model_settings_snapshot().unwrap().unwrap();
    assert_ne!(
        changed_snapshot.provider_protocol_revisions[&initial_id],
        initial_revision
    );
    assert_eq!(
        changed.models[0]
            .provider_profile_config
            .legacy_reasoning()
            .expect("registered legacy selection must write schema v1")
            .effort,
        crate::ReasoningEffort::Max
    );
}

#[test]
fn select_generic_resolves_from_the_effective_anthropic_dialect() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let mut settings = revision_test_settings();
    settings.api_url = "https://api.anthropic.com/v1/messages".to_string();
    let saved = service
        .save_model_settings_request(renderer_save_request(
            &settings,
            serde_json::json!({"kind": "select_generic"}),
            None,
        ))
        .unwrap();

    assert_eq!(
        saved.models[0].provider_profile_config.profile(),
        crate::ProviderProfileRef::generic_for_dialect(
            crate::ProviderProtocolDialect::AnthropicMessages
        )
    );
}

#[test]
fn unchanged_explicit_generic_fails_on_dialect_change_until_generic_is_reselected() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let mut settings = revision_test_settings();
    settings.models[0].provider_profile_config = crate::ProviderProfileConfig::generic_for_dialect(
        crate::ProviderProtocolDialect::OpenAiChatCompletions,
    );
    service.save_model_settings(settings.clone()).unwrap();

    settings.api_url = "https://api.anthropic.com/v1/messages".to_string();
    let unchanged = renderer_save_request(
        &settings,
        serde_json::json!({"kind": "unchanged"}),
        Some(&settings.models[0].id),
    );
    assert!(service.save_model_settings_request(unchanged).is_err());
    assert_eq!(
        service.load_model_settings().unwrap().unwrap().api_url,
        "https://revision.example/v1"
    );

    let saved = service
        .save_model_settings_request(renderer_save_request(
            &settings,
            serde_json::json!({"kind": "select_generic"}),
            Some(&settings.models[0].id),
        ))
        .unwrap();
    assert_eq!(
        saved.models[0].provider_profile_config.profile().id,
        crate::ProviderProfileId::GenericAnthropicMessages
    );
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
        provider_model_id: "override-model".to_string(),
        display_name: "Override Model".to_string(),
        api_url_override: Some("https://override.example/v1".to_string()),
        api_token_override: Some("override-token".to_string()),
        supports_image: false,
        context_window_tokens: Some(128_000),
        provider_profile_config: crate::ProviderProfileConfig::generic_for_dialect(
            crate::ProviderProtocolDialect::OpenAiChatCompletions,
        ),
        input_price: "0".to_string(),
        cached_input_price: String::new(),
        output_price: "0".to_string(),
        enabled: true,
    });
    service.save_model_settings(settings.clone()).unwrap();
    let initial = service.load_model_settings_snapshot().unwrap().unwrap();

    settings.api_token = "rotated-global-token".to_string();
    settings.models[0].display_name = "Metadata-only rename".to_string();
    settings.models[1].input_price = "1.25".to_string();
    settings.models[1].provider_profile_config = crate::ProviderProfileConfig::generic_for_dialect(
        crate::ProviderProtocolDialect::OpenAiChatCompletions,
    );
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

    settings.models[0].provider_profile_config = crate::ProviderProfileConfig::generic_for_dialect(
        crate::ProviderProtocolDialect::OpenAiChatCompletions,
    );
    settings.models[0].output_price = "7.5".to_string();
    service.save_model_settings(settings.clone()).unwrap();
    let metadata_edit = service.load_model_settings_snapshot().unwrap().unwrap();
    assert_eq!(
        initial.provider_connection_revisions,
        metadata_edit.provider_connection_revisions
    );
    assert_eq!(
        initial.provider_protocol_revisions, metadata_edit.provider_protocol_revisions,
        "an equivalent explicit Generic profile must preserve the protocol identity"
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
        provider_model_id: "other-model".to_string(),
        display_name: "Other Model".to_string(),
        api_url_override: Some("https://other.example/v1".to_string()),
        api_token_override: Some("other-token".to_string()),
        supports_image: false,
        context_window_tokens: Some(64_000),
        provider_profile_config: crate::ProviderProfileConfig::generic_for_dialect(
            crate::ProviderProtocolDialect::OpenAiChatCompletions,
        ),
        input_price: "0".to_string(),
        cached_input_price: String::new(),
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
        crate::ProviderProfileConfig::deepseek_v4_default();
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
        crate::ProviderProfileConfig::deepseek_v4_default();
    service.save_model_settings(settings.clone()).unwrap();
    let deepseek_default = service.load_model_settings_snapshot().unwrap().unwrap();
    assert_ne!(
        deepseek_default.provider_protocol_revisions["revision-model"], selected_initial,
        "changing the selected Profile must rotate its protocol identity"
    );

    let mut thinking = crate::ProviderProfileConfig::deepseek_v4_default();
    let crate::ProviderProfileConfig::V1(config) = &mut thinking else {
        unreachable!("legacy DeepSeek constructor must produce schema v1")
    };
    config.reasoning.mode = crate::ReasoningMode::Enabled;
    config.reasoning.effort = crate::ReasoningEffort::High;
    settings.models[0].provider_profile_config = thinking;
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
    settings.models[0].provider_profile_config = crate::ProviderProfileConfig::generic_for_dialect(
        crate::ProviderProtocolDialect::AnthropicMessages,
    );
    service.save_model_settings(settings.clone()).unwrap();
    let endpoint_and_dialect_changed = service.load_model_settings_snapshot().unwrap().unwrap();
    assert_ne!(
        endpoint_and_dialect_changed.provider_protocol_revisions["revision-model"],
        connection_changed.provider_protocol_revisions["revision-model"],
        "changing the endpoint/dialect must rotate the protocol identity"
    );

    settings.models[0].provider_model_id = "revision-model-v2".to_string();
    service.save_model_settings(settings).unwrap();
    let wire_model_changed = service.load_model_settings_snapshot().unwrap().unwrap();
    assert_ne!(
        wire_model_changed.provider_protocol_revisions["revision-model"],
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
        crate::ProviderProfileConfig::deepseek_v4_default();

    service.save_model_settings(settings).unwrap();
    let stored = service.load_model_settings().unwrap().unwrap();

    assert_eq!(
        stored.models[0].provider_profile_config,
        crate::ProviderProfileConfig::deepseek_v4_default()
    );
}

#[test]
fn unchanged_save_preserves_the_legacy_v1_profile_shape() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let mut settings = revision_test_settings();
    settings.models[0].provider_profile_config =
        crate::ProviderProfileConfig::deepseek_v4_default();
    service.save_model_settings(settings.clone()).unwrap();

    settings.models[0].display_name = "Metadata edit".to_string();
    let saved = service
        .save_model_settings_request(renderer_save_request(
            &settings,
            serde_json::json!({"kind": "unchanged"}),
            Some(&settings.models[0].id),
        ))
        .unwrap();

    assert_eq!(
        serde_json::to_value(&saved.models[0].provider_profile_config).unwrap(),
        serde_json::json!({
            "schemaVersion": 1,
            "profile": {"id": "deepseek_v4_chat", "version": 1},
            "reasoning": {"mode": "provider_default", "effort": "provider_default"}
        })
    );
}

#[test]
fn save_wire_requires_an_explicit_profile_update() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    service
        .save_model_settings(revision_test_settings())
        .unwrap();

    let mut payload = serde_json::to_value(revision_test_settings()).unwrap();
    payload["models"][0]
        .as_object_mut()
        .unwrap()
        .remove("providerProfileConfig");
    assert!(serde_json::from_value::<ModelSettingsSaveRequest>(payload).is_err());
}

#[test]
fn explicit_generic_profile_can_replace_a_provider_specific_profile() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let mut settings = revision_test_settings();
    settings.models[0].provider_profile_config =
        crate::ProviderProfileConfig::deepseek_v4_default();
    service.save_model_settings(settings.clone()).unwrap();

    settings.models[0].provider_profile_config = crate::ProviderProfileConfig::generic_for_dialect(
        crate::ProviderProtocolDialect::OpenAiChatCompletions,
    );
    service.save_model_settings(settings.clone()).unwrap();

    let stored = service.load_model_settings().unwrap().unwrap();
    assert_eq!(
        stored.models[0].provider_profile_config,
        settings.models[0].provider_profile_config
    );
}

#[test]
fn unsupported_persisted_provider_profile_remains_visible_but_cannot_resolve() {
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

    let loaded = service.load_model_settings().unwrap().unwrap();
    let config = &loaded.models[0].provider_profile_config;
    assert_eq!(config.profile().id.as_str(), "future_profile");
    assert_eq!(config.profile().version, 1);
    assert!(config.validate().is_err());
}

#[test]
fn unsupported_profile_survives_unrelated_save_and_model_id_rename() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    service
        .save_model_settings(revision_test_settings())
        .unwrap();

    let connection = rusqlite::Connection::open(fixture.root.join("storage.sqlite")).unwrap();
    connection
        .execute(
            "UPDATE models SET provider_profile_config_json = ?1 WHERE id = 'revision-model'",
            [r#"{"schemaVersion":1,"profile":{"id":"future_profile","version":9},"reasoning":{"mode":"provider_default","effort":"provider_default"}}"#],
        )
        .unwrap();
    drop(connection);

    let loaded = service.load_model_settings().unwrap().unwrap();
    let before_revision = service
        .load_model_settings_snapshot()
        .unwrap()
        .unwrap()
        .provider_protocol_revisions["revision-model"]
        .clone();
    let mut price_edit = loaded.clone();
    price_edit.models[0].input_price = "3.5".to_string();
    let saved = service
        .save_model_settings_request(renderer_save_request(
            &price_edit,
            serde_json::json!({"kind": "unchanged"}),
            Some(&price_edit.models[0].id),
        ))
        .unwrap();
    assert_eq!(
        saved.models[0]
            .provider_profile_config
            .profile()
            .id
            .as_str(),
        "future_profile"
    );
    assert_eq!(
        service
            .load_model_settings_snapshot()
            .unwrap()
            .unwrap()
            .provider_protocol_revisions["revision-model"],
        before_revision,
        "an unrelated price save must preserve the unsupported opaque wire identity"
    );

    let mut renamed = saved;
    renamed.models[0].display_name = "Renamed model".to_string();
    let renamed = service
        .save_model_settings_request(renderer_save_request(
            &renamed,
            serde_json::json!({"kind": "unchanged"}),
            Some("revision-model"),
        ))
        .unwrap();
    assert_eq!(
        renamed.models[0]
            .provider_profile_config
            .profile()
            .id
            .as_str(),
        "future_profile"
    );
}

#[test]
fn save_request_rejects_renderer_capabilities_versions_and_unknown_settings() {
    let valid_update = serde_json::json!({
        "kind": "select_registered_profile",
        "profileId": "deepseek_v4_chat",
        "settings": {
            "kind": "deepseek_v4_chat",
            "reasoning": {"mode": "enabled", "effort": "high"}
        }
    });
    for (field, injected) in [
        ("profileVersion", serde_json::json!(1)),
        ("capabilities", serde_json::json!({"privateReplay": true})),
        ("providerConfigurationRevision", serde_json::json!("forged")),
    ] {
        let mut settings = serde_json::to_value(revision_test_settings()).unwrap();
        let mut update = valid_update.clone();
        update
            .as_object_mut()
            .unwrap()
            .insert(field.to_string(), injected);
        settings["models"][0]["providerProfileUpdate"] = update;
        assert!(serde_json::from_value::<ModelSettingsSaveRequest>(settings).is_err());
    }

    let mut settings = serde_json::to_value(revision_test_settings()).unwrap();
    let mut update = valid_update.clone();
    update["settings"]["reasoning"]["replay"] = serde_json::json!(true);
    settings["models"][0]["providerProfileUpdate"] = update;
    assert!(serde_json::from_value::<ModelSettingsSaveRequest>(settings).is_err());

    let mut settings = serde_json::to_value(revision_test_settings()).unwrap();
    let mut update = valid_update;
    update["settings"]["reasoning"]["effort"] = serde_json::json!("xhigh");
    settings["models"][0]["providerProfileUpdate"] = update;
    assert!(serde_json::from_value::<ModelSettingsSaveRequest>(settings).is_err());
}

#[test]
fn registered_selection_rejects_unknown_profiles_and_incompatible_dialects() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let settings = revision_test_settings();
    let unknown = renderer_save_request(
        &settings,
        serde_json::json!({
            "kind": "select_registered_profile",
            "profileId": "future_profile",
            "settings": {
                "kind": "deepseek_v4_chat",
                "reasoning": {"mode": "provider_default", "effort": "provider_default"}
            }
        }),
        None,
    );
    assert!(service.save_model_settings_request(unknown).is_err());

    let mut anthropic = settings;
    anthropic.api_url = "https://api.anthropic.com/v1/messages".to_string();
    let incompatible = renderer_save_request(
        &anthropic,
        serde_json::json!({
            "kind": "select_registered_profile",
            "profileId": "deepseek_v4_chat",
            "settings": {
                "kind": "deepseek_v4_chat",
                "reasoning": {"mode": "provider_default", "effort": "provider_default"}
            }
        }),
        None,
    );
    assert!(service.save_model_settings_request(incompatible).is_err());
    assert!(service.load_model_settings().unwrap().is_none());
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
            provider_model_id: "model-a".to_string(),
            display_name: "Model A".to_string(),
            api_url_override: None,
            api_token_override: None,
            supports_image: false,
            context_window_tokens: Some(128_000),
            provider_profile_config: crate::ProviderProfileConfig::generic_for_dialect(
                crate::ProviderProtocolDialect::OpenAiChatCompletions,
            ),
            input_price: "0.01".to_string(),
            cached_input_price: String::new(),
            output_price: "0.02".to_string(),
            enabled: true,
        }],
    };
    service.save_model_settings(valid.clone()).unwrap();

    let mut invalid = valid.clone();
    invalid.models[0].input_price = "not-a-price".to_string();
    assert!(service.save_model_settings(invalid).is_err());

    let mut invalid_cached = valid;
    invalid_cached.models[0].cached_input_price = "not-a-price".to_string();
    assert!(service.save_model_settings(invalid_cached).is_err());

    let stored = service.load_model_settings().unwrap().unwrap();
    assert_eq!(stored.models[0].input_price, "0.01");
    assert_eq!(stored.models[0].cached_input_price, "");
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
            provider_model_id: "model-a".to_string(),
            display_name: "Model A".to_string(),
            api_url_override: None,
            api_token_override: None,
            supports_image: false,
            context_window_tokens: Some(128_000),
            provider_profile_config: crate::ProviderProfileConfig::generic_for_dialect(
                crate::ProviderProtocolDialect::OpenAiChatCompletions,
            ),
            input_price: "0.01".to_string(),
            cached_input_price: String::new(),
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
            provider_model_id: "model-a".to_string(),
            display_name: "Model A".to_string(),
            api_url_override: Some("https://model.example/v1".to_string()),
            api_token_override: Some("model-token".to_string()),
            supports_image: false,
            context_window_tokens: Some(128_000),
            provider_profile_config: crate::ProviderProfileConfig::generic_for_dialect(
                crate::ProviderProtocolDialect::OpenAiChatCompletions,
            ),
            input_price: "0.01".to_string(),
            cached_input_price: String::new(),
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
