use super::*;
use crate::image_generation::{
    CredentialDeleteOutcome, CredentialReference, CredentialSecret, CredentialStore,
    CredentialStoreBackend, CredentialStoreError, CredentialStoreOperation,
};
use crate::storage::models::{CredentialMutation, CredentialStatus};
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Default)]
struct FaultInjectingModelCredentialStore {
    delegate: crate::image_generation::InMemoryCredentialStore,
    fail_replace: AtomicBool,
    fail_delete: AtomicBool,
}

impl CredentialStore for FaultInjectingModelCredentialStore {
    fn backend(&self) -> CredentialStoreBackend {
        CredentialStoreBackend::InMemoryV1
    }

    fn replace(
        &self,
        reference: &CredentialReference,
        secret: CredentialSecret,
    ) -> Result<(), CredentialStoreError> {
        if self.fail_replace.load(Ordering::SeqCst) {
            return Err(CredentialStoreError::BackendUnavailable {
                operation: CredentialStoreOperation::Replace,
            });
        }
        self.delegate.replace(reference, secret)
    }

    fn get(
        &self,
        reference: &CredentialReference,
    ) -> Result<Option<CredentialSecret>, CredentialStoreError> {
        self.delegate.get(reference)
    }

    fn delete(
        &self,
        reference: &CredentialReference,
    ) -> Result<CredentialDeleteOutcome, CredentialStoreError> {
        if self.fail_delete.load(Ordering::SeqCst) {
            return Err(CredentialStoreError::BackendUnavailable {
                operation: CredentialStoreOperation::Delete,
            });
        }
        self.delegate.delete(reference)
    }
}

fn model_credential_journal_count(service: &StorageService, table: &str) -> i64 {
    assert!(matches!(
        table,
        "model_provider_credential_staging" | "model_provider_credential_cleanup"
    ));
    let connection = service.state.connection().unwrap();
    connection
        .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .unwrap()
}

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

fn renderer_save_request_value<T: serde::Serialize>(settings: &T) -> serde_json::Value {
    let mut value = serde_json::to_value(settings).unwrap();
    let root = value.as_object_mut().unwrap();
    let expected_revision = root
        .remove("configurationRevision")
        .unwrap_or(serde_json::Value::Null);
    root.insert("expectedRevision".to_string(), expected_revision);
    let api_token_mutation = root
        .remove("apiToken")
        .map(|value| match value.as_str().unwrap_or_default() {
            "" => serde_json::json!({"type": "clear"}),
            value => serde_json::json!({"type": "replace", "value": value}),
        })
        .unwrap_or_else(|| serde_json::json!({"type": "keep"}));
    root.remove("apiTokenStatus");
    root.insert("apiTokenMutation".to_string(), api_token_mutation);
    let tavily_mutation = root
        .remove("tavilyApiKey")
        .map(|value| match value.as_str().unwrap_or_default() {
            "" => serde_json::json!({"type": "clear"}),
            value => serde_json::json!({"type": "replace", "value": value}),
        })
        .unwrap_or_else(|| serde_json::json!({"type": "keep"}));
    root.remove("tavilyApiKeyStatus");
    root.insert("tavilyApiKeyMutation".to_string(), tavily_mutation);
    for model in value["models"].as_array_mut().unwrap() {
        let model = model.as_object_mut().unwrap();
        let override_mutation = model
            .remove("apiTokenOverride")
            .map(|value| match value.as_str() {
                Some(value) if !value.is_empty() => {
                    serde_json::json!({"type": "replace", "value": value})
                }
                _ => serde_json::json!({"type": "clear"}),
            })
            .unwrap_or_else(|| serde_json::json!({"type": "keep"}));
        model.remove("apiTokenOverrideStatus");
        model.insert("apiTokenOverrideMutation".to_string(), override_mutation);
    }
    value
}

fn renderer_save_request<T: serde::Serialize>(
    settings: &T,
    update: serde_json::Value,
    config_id: Option<&str>,
) -> ModelSettingsSaveRequest {
    let mut value = renderer_save_request_value(settings);
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

fn renderer_save_request_preserving_credentials<T: serde::Serialize>(
    settings: &T,
    update: serde_json::Value,
    config_id: Option<&str>,
) -> ModelSettingsSaveRequest {
    let mut request = renderer_save_request(settings, update, config_id);
    request.api_token_mutation = CredentialMutation::Keep;
    request.tavily_api_key_mutation = CredentialMutation::Keep;
    for model in &mut request.models {
        model.api_token_override_mutation = CredentialMutation::Keep;
    }
    request
}

fn with_current_configuration_revision(
    service: &StorageService,
    mut request: ModelSettingsSaveRequest,
) -> ModelSettingsSaveRequest {
    request.expected_revision = service
        .load_model_settings_snapshot()
        .unwrap()
        .map(|snapshot| snapshot.configuration_revision);
    request
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

fn delete_stored_credential(fixture: &StorageFixture, column: &str, model_id: Option<&str>) {
    let connection = rusqlite::Connection::open(fixture.root.join("storage.sqlite")).unwrap();
    let query = match model_id {
        Some(_) => format!("SELECT {column} FROM models WHERE id = ?1"),
        None => format!("SELECT {column} FROM model_provider_settings WHERE id = 'default'"),
    };
    let credential_ref: String = match model_id {
        Some(model_id) => connection
            .query_row(&query, [model_id], |row| row.get(0))
            .unwrap(),
        None => connection.query_row(&query, [], |row| row.get(0)).unwrap(),
    };
    let reference = CredentialReference::parse(&credential_ref).unwrap();
    fixture.model_credentials.delete(&reference).unwrap();
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

    let mut value = renderer_save_request_value(&settings);
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
    let request = with_current_configuration_revision(&service, request);

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

    let mut value = renderer_save_request_value(&settings);
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
    let request = with_current_configuration_revision(&service, request);
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
        saved.models[0].api_token_override_status,
        saved.models[1].api_token_override_status
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
    let mut value = renderer_save_request_value(&settings);
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
    let request = with_current_configuration_revision(&service, request);

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
        .save_model_settings_request(with_current_configuration_revision(
            &service,
            renderer_save_request(
                &edited,
                serde_json::json!({"kind": "unchanged"}),
                Some("revision-model"),
            ),
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
fn vendor_selection_resolves_moonshot_family_and_persists_v2_settings() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let mut settings = revision_test_settings();
    settings.models[0].provider_model_id = "kimi-k3".to_string();
    settings.models[0].display_name = "Kimi K3".to_string();
    settings.models[0].supports_image = true;

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
fn save_boundary_preserves_an_incomplete_global_connection_but_execution_fails_closed() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let settings = ModelSettingsRecord {
        api_url: "https://api.deepseek.com/v1/chat/completions".to_string(),
        api_token: "   ".to_string(),
        search_mode: "disabled".to_string(),
        tavily_api_key: "preserved-search-token".to_string(),
        models: vec![official_profile_test_model("deepseek-flash", None, None)],
    };
    service.save_model_settings(settings).unwrap();
    let stored = service.load_model_settings().unwrap().unwrap();
    assert_eq!(
        stored.api_url,
        "https://api.deepseek.com/v1/chat/completions"
    );
    assert!(stored.api_token.is_empty());
    assert!(stored.effective_connection_for(&stored.models[0]).is_err());
}

#[test]
fn global_url_and_credential_can_be_edited_in_either_order_and_clear_keeps_url() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let mut settings = revision_test_settings();
    settings.api_url.clear();
    settings.api_token.clear();
    service.save_model_settings(settings).unwrap();

    let mut current = service.load_model_settings().unwrap().unwrap();
    current.api_url = "https://provider.example/v1/chat/completions".to_string();
    let model_id = current.models[0].id.clone();
    service
        .save_model_settings_request(with_current_configuration_revision(
            &service,
            renderer_save_request(
                &current,
                serde_json::json!({"kind": "unchanged"}),
                Some(&model_id),
            ),
        ))
        .unwrap();
    assert!(service
        .load_model_settings_snapshot_for_model(&model_id, false)
        .unwrap()
        .unwrap()
        .settings
        .effective_connection_for(
            &service
                .load_model_settings_snapshot_for_model(&model_id, false)
                .unwrap()
                .unwrap()
                .settings
                .models[0]
        )
        .is_err());

    current = service.load_model_settings().unwrap().unwrap();
    let mut add_token = with_current_configuration_revision(
        &service,
        renderer_save_request(
            &current,
            serde_json::json!({"kind": "unchanged"}),
            Some(&model_id),
        ),
    );
    add_token.api_token_mutation = CredentialMutation::Replace {
        value: "provider-token".to_string(),
    };
    service.save_model_settings_request(add_token).unwrap();
    let resolved = service
        .load_model_settings_snapshot_for_model(&model_id, false)
        .unwrap()
        .unwrap();
    assert_eq!(
        resolved
            .settings
            .effective_connection_for(&resolved.settings.models[0])
            .unwrap()
            .api_token,
        "provider-token"
    );

    current = service.load_model_settings().unwrap().unwrap();
    let mut clear = with_current_configuration_revision(
        &service,
        renderer_save_request(
            &current,
            serde_json::json!({"kind": "unchanged"}),
            Some(&model_id),
        ),
    );
    clear.api_token_mutation = CredentialMutation::Clear;
    let cleared = service.save_model_settings_request(clear).unwrap();
    assert_eq!(
        cleared.api_url,
        "https://provider.example/v1/chat/completions"
    );
    assert_eq!(cleared.api_token_status, CredentialStatus::Missing);

    current = service.load_model_settings().unwrap().unwrap();
    current.api_url.clear();
    let mut token_first = with_current_configuration_revision(
        &service,
        renderer_save_request(
            &current,
            serde_json::json!({"kind": "unchanged"}),
            Some(&model_id),
        ),
    );
    token_first.api_token_mutation = CredentialMutation::Replace {
        value: "second-token".to_string(),
    };
    service.save_model_settings_request(token_first).unwrap();
    current = service.load_model_settings().unwrap().unwrap();
    assert!(current.api_url.is_empty());
    assert_eq!(current.api_token, "second-token");
    assert!(current
        .effective_connection_for(&current.models[0])
        .is_err());

    current.api_url = "https://second.example/v1/chat/completions".to_string();
    let mut add_url = with_current_configuration_revision(
        &service,
        renderer_save_request(
            &current,
            serde_json::json!({"kind": "unchanged"}),
            Some(&model_id),
        ),
    );
    add_url.api_token_mutation = CredentialMutation::Keep;
    service.save_model_settings_request(add_url).unwrap();
    let resolved = service
        .load_model_settings_snapshot_for_model(&model_id, false)
        .unwrap()
        .unwrap();
    assert_eq!(
        resolved
            .settings
            .effective_connection_for(&resolved.settings.models[0])
            .unwrap()
            .api_url,
        "https://second.example/v1/chat/completions"
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
            models: vec![official_profile_test_model("deepseek-flash", None, None)],
        })
        .unwrap();

    let future_profile = serde_json::json!({
        "schemaVersion": 2,
        "profile": {"id": "deepseek_v4_1_flash_chat", "version": 99},
        "vendorId": "deepseek",
        "settings": {
            "kind": "deepseek_flash_chat",
            "reasoning": {"mode": "enabled", "effort": "low"}
        }
    });
    {
        let connection = service.state.connection().unwrap();
        connection
            .execute(
                "UPDATE models SET provider_profile_config_json = ?1 WHERE id = 'deepseek-flash'",
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
    let mut request = renderer_save_request_preserving_credentials(
        &price_edit,
        serde_json::json!({"kind": "unchanged"}),
        Some(&price_edit.models[0].id),
    );
    request.expected_revision = Some(after_startup.configuration_revision.clone());
    reopened.save_model_settings_request(request).unwrap();
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
    let mut settings = revision_test_settings();
    settings.models[0].provider_model_id = "deepseek-flash".to_string();
    settings.models[0].supports_image = true;
    let initial = service
        .save_model_settings_request(renderer_save_request(
            &settings,
            serde_json::json!({
                "kind": "select_vendor",
                "vendorId": "deepseek",
                "settings": {
                    "kind": "deepseek_flash_chat",
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
    let metadata = service
        .save_model_settings_request(renderer_save_request_preserving_credentials(
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
        .save_model_settings_request(renderer_save_request_preserving_credentials(
            &metadata,
            serde_json::json!({
                "kind": "select_vendor",
                "vendorId": "deepseek",
                "settings": {
                    "kind": "deepseek_flash_chat",
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
    assert!(matches!(
        changed.models[0].provider_profile_config.family_settings(),
        Some(crate::ProviderFamilySettings::DeepseekFlashChat {
            reasoning: crate::ProviderFamilyReasoningPolicy {
                effort: crate::ProviderReasoningEffort::Max,
                ..
            }
        })
    ));
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
    let unchanged = with_current_configuration_revision(
        &service,
        renderer_save_request(
            &settings,
            serde_json::json!({"kind": "unchanged"}),
            Some(&settings.models[0].id),
        ),
    );
    assert!(service.save_model_settings_request(unchanged).is_err());
    assert_eq!(
        service.load_model_settings().unwrap().unwrap().api_url,
        "https://revision.example/v1"
    );

    let saved = service
        .save_model_settings_request(with_current_configuration_revision(
            &service,
            renderer_save_request(
                &settings,
                serde_json::json!({"kind": "select_generic"}),
                Some(&settings.models[0].id),
            ),
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
    settings.models[1].provider_model_id = "deepseek-flash".to_string();
    settings.models[1].provider_profile_config =
        crate::ProviderProfileConfig::deepseek_flash_default();
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

    settings.models[0].provider_model_id = "deepseek-flash".to_string();
    settings.models[0].provider_profile_config =
        crate::ProviderProfileConfig::deepseek_flash_default();
    service.save_model_settings(settings.clone()).unwrap();
    let deepseek_default = service.load_model_settings_snapshot().unwrap().unwrap();
    assert_ne!(
        deepseek_default.provider_protocol_revisions["revision-model"], selected_initial,
        "changing the selected Profile must rotate its protocol identity"
    );

    let mut thinking = crate::ProviderProfileConfig::deepseek_flash_default();
    let crate::ProviderProfileConfig::V2(config) = &mut thinking else {
        unreachable!("DeepSeek family constructor must produce schema v2")
    };
    let crate::ProviderFamilySettings::DeepseekFlashChat { reasoning } = &mut config.settings
    else {
        unreachable!("Flash profile must carry Flash family settings")
    };
    reasoning.mode = crate::ReasoningMode::Enabled;
    reasoning.effort = crate::ProviderReasoningEffort::High;
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
        crate::ProviderProfileConfig::deepseek_flash_default();

    service.save_model_settings(settings).unwrap();
    let stored = service.load_model_settings().unwrap().unwrap();

    assert_eq!(
        stored.models[0].provider_profile_config,
        crate::ProviderProfileConfig::deepseek_flash_default()
    );
}

#[test]
fn unchanged_save_preserves_the_current_deepseek_profile_shape() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let mut settings = revision_test_settings();
    settings.models[0].provider_model_id = "deepseek-flash".to_string();
    settings.models[0].supports_image = true;
    settings.models[0].provider_profile_config =
        crate::ProviderProfileConfig::deepseek_flash_default();
    service.save_model_settings(settings.clone()).unwrap();

    settings.models[0].display_name = "Metadata edit".to_string();
    let saved = service
        .save_model_settings_request(with_current_configuration_revision(
            &service,
            renderer_save_request(
                &settings,
                serde_json::json!({"kind": "unchanged"}),
                Some(&settings.models[0].id),
            ),
        ))
        .unwrap();

    assert_eq!(
        serde_json::to_value(&saved.models[0].provider_profile_config).unwrap(),
        serde_json::json!({
            "schemaVersion": 2,
            "profile": {"id": "deepseek_v4_1_flash_chat", "version": 1},
            "vendorId": "deepseek",
            "settings": {
                "kind": "deepseek_flash_chat",
                "reasoning": {"mode": "provider_default", "effort": "provider_default"}
            }
        })
    );
}

#[test]
fn renderer_save_rejects_an_explicit_stale_configuration_revision() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let settings = revision_test_settings();
    service.save_model_settings(settings.clone()).unwrap();
    let original = service.load_model_settings_for_edit().unwrap().unwrap();

    let mut first_edit = settings.clone();
    first_edit.models[0].display_name = "First committed edit".to_string();
    let mut first_request = renderer_save_request(
        &first_edit,
        serde_json::json!({"kind": "unchanged"}),
        Some(&first_edit.models[0].id),
    );
    first_request.expected_revision = Some(original.configuration_revision.clone());
    let committed = service.save_model_settings_request(first_request).unwrap();
    assert_ne!(
        committed.configuration_revision,
        original.configuration_revision
    );

    let mut stale_edit = settings;
    stale_edit.models[0].display_name = "Stale edit".to_string();
    let mut stale_request = renderer_save_request(
        &stale_edit,
        serde_json::json!({"kind": "unchanged"}),
        Some(&stale_edit.models[0].id),
    );
    stale_request.expected_revision = Some(original.configuration_revision);
    assert!(matches!(
        service.save_model_settings_request(stale_request),
        Err(ModelSettingsSaveError::Other(message)) if message == "model settings revision conflict"
    ));

    let current = service.load_model_settings_for_edit().unwrap().unwrap();
    assert_eq!(
        current.configuration_revision,
        committed.configuration_revision
    );
    assert_eq!(current.models[0].display_name, "First committed edit");
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
fn save_wire_requires_expected_revision_but_accepts_explicit_null() {
    let settings = revision_test_settings();
    let mut payload = renderer_save_request_value(&settings);
    let model = payload["models"][0].as_object_mut().unwrap();
    model.insert("id".to_string(), serde_json::Value::Null);
    model.remove("providerProfileConfig");
    model.insert(
        "providerProfileUpdate".to_string(),
        serde_json::json!({"kind": "select_generic"}),
    );

    let mut missing_revision = payload.clone();
    missing_revision
        .as_object_mut()
        .unwrap()
        .remove("expectedRevision");
    assert!(serde_json::from_value::<ModelSettingsSaveRequest>(missing_revision).is_err());

    payload["expectedRevision"] = serde_json::Value::Null;
    let request = serde_json::from_value::<ModelSettingsSaveRequest>(payload).unwrap();
    assert_eq!(request.expected_revision, None);
}

#[test]
fn explicit_generic_profile_can_replace_a_provider_specific_profile() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let mut settings = revision_test_settings();
    settings.models[0].provider_profile_config =
        crate::ProviderProfileConfig::deepseek_flash_default();
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
        .save_model_settings_request(with_current_configuration_revision(
            &service,
            renderer_save_request_preserving_credentials(
                &price_edit,
                serde_json::json!({"kind": "unchanged"}),
                Some(&price_edit.models[0].id),
            ),
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
        .save_model_settings_request(renderer_save_request_preserving_credentials(
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
fn selected_model_resolution_does_not_open_unrelated_override_credentials() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let mut first = official_profile_test_model(
        "model-a",
        Some("https://provider-a.example/v1/chat/completions"),
        Some("provider-a-secret"),
    );
    first.display_name = "Provider A".to_string();
    let mut second = official_profile_test_model(
        "model-b",
        Some("https://provider-b.example/v1/chat/completions"),
        Some("provider-b-secret"),
    );
    second.display_name = "Provider B".to_string();
    service
        .save_model_settings(ModelSettingsRecord {
            api_url: String::new(),
            api_token: String::new(),
            search_mode: "disabled".to_string(),
            tavily_api_key: String::new(),
            models: vec![first, second],
        })
        .unwrap();
    delete_stored_credential(&fixture, "api_token_override_ref", Some("model-b"));

    let selected = service
        .load_model_settings_snapshot_for_model("model-a", false)
        .unwrap()
        .unwrap();
    assert_eq!(selected.settings.models.len(), 1);
    assert_eq!(selected.settings.models[0].id, "model-a");
    assert_eq!(
        selected.settings.models[0].api_token_override.as_deref(),
        Some("provider-a-secret")
    );
    assert!(service
        .load_model_settings_snapshot_for_model("model-b", false)
        .is_err());
}

#[test]
fn disabled_search_does_not_open_an_unavailable_tavily_credential() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let mut settings = revision_test_settings();
    settings.tavily_api_key = "tavily-secret".to_string();
    service.save_model_settings(settings).unwrap();
    delete_stored_credential(&fixture, "tavily_api_key_ref", None);

    let snapshot = service
        .load_model_settings_snapshot_for_model("revision-model", true)
        .unwrap()
        .unwrap();
    assert!(snapshot.settings.tavily_api_key.is_empty());
    assert_eq!(snapshot.settings.api_token, "fixed-revision-test-token");
}

#[test]
fn model_provider_secrets_never_enter_sqlite_or_debug_output() {
    const GLOBAL: &str = "db-canary-global-18c9";
    const SEARCH: &str = "db-canary-search-27da";
    const OVERRIDE: &str = "db-canary-override-36eb";
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let mut model = official_profile_test_model(
        "model-secret-test",
        Some("https://provider.example/v1/chat/completions"),
        Some(OVERRIDE),
    );
    model.display_name = "Secret storage test".to_string();
    service
        .save_model_settings(ModelSettingsRecord {
            api_url: "https://global.example/v1/chat/completions".to_string(),
            api_token: GLOBAL.to_string(),
            search_mode: "tavily".to_string(),
            tavily_api_key: SEARCH.to_string(),
            models: vec![model],
        })
        .unwrap();
    drop(service);

    for path in [
        fixture.root.join("storage.sqlite"),
        fixture.root.join("storage.sqlite-wal"),
    ] {
        if let Ok(bytes) = std::fs::read(path) {
            for secret in [GLOBAL, SEARCH, OVERRIDE] {
                assert!(!bytes
                    .windows(secret.len())
                    .any(|window| window == secret.as_bytes()));
            }
        }
    }

    let request = renderer_save_request(
        &revision_test_settings(),
        serde_json::json!({"kind": "select_generic"}),
        None,
    );
    let rendered = format!("{request:?}");
    assert!(!rendered.contains("fixed-revision-test-token"));
}

#[test]
fn failed_model_credential_publish_leaves_no_configuration_or_orphaned_staging() {
    let fixture = StorageFixture::new();
    let credentials = Arc::new(FaultInjectingModelCredentialStore::default());
    credentials.fail_replace.store(true, Ordering::SeqCst);
    let service = StorageService::open_with_model_credentials(
        &fixture.root.join("storage.sqlite"),
        credentials,
    )
    .unwrap();
    let request = renderer_save_request(
        &revision_test_settings(),
        serde_json::json!({"kind": "select_generic"}),
        None,
    );

    assert!(matches!(
        service.save_model_settings_request(request),
        Err(ModelSettingsSaveError::Other(message))
            if message == "provider credential store is unavailable"
    ));
    assert!(service.load_model_settings_for_edit().unwrap().is_none());
    assert_eq!(
        model_credential_journal_count(&service, "model_provider_credential_staging"),
        0
    );
    assert_eq!(
        model_credential_journal_count(&service, "model_provider_credential_cleanup"),
        0
    );
}

#[test]
fn failed_sqlite_publication_leaves_a_retryable_staging_receipt() {
    let fixture = StorageFixture::new();
    let credentials = Arc::new(FaultInjectingModelCredentialStore::default());
    let service = StorageService::open_with_model_credentials(
        &fixture.root.join("storage.sqlite"),
        credentials.clone(),
    )
    .unwrap();
    {
        let connection = service.state.connection().unwrap();
        connection
            .execute_batch(
                "CREATE TRIGGER fail_model_settings_publication
                 BEFORE INSERT ON models
                 BEGIN
                     SELECT RAISE(FAIL, 'injected publication failure');
                 END;",
            )
            .unwrap();
    }

    let request = renderer_save_request(
        &revision_test_settings(),
        serde_json::json!({"kind": "select_generic"}),
        None,
    );
    assert!(service.save_model_settings_request(request).is_err());
    assert!(service.load_model_settings_for_edit().unwrap().is_none());
    assert_eq!(
        model_credential_journal_count(&service, "model_provider_credential_staging"),
        1
    );
    let staged = {
        let connection = service.state.connection().unwrap();
        config_repository::list_model_provider_credential_staging(&connection)
            .unwrap()
            .pop()
            .unwrap()
            .credential_ref
    };
    let staged_reference = CredentialReference::parse(staged).unwrap();
    assert!(credentials.get(&staged_reference).unwrap().is_some());

    let report = service.reconcile_model_provider_credentials().unwrap();
    assert_eq!(report.removed_orphaned_credentials, 1);
    assert!(credentials.get(&staged_reference).unwrap().is_none());
    assert_eq!(
        model_credential_journal_count(&service, "model_provider_credential_staging"),
        0
    );
}

#[test]
fn reconciliation_keeps_an_active_credential_after_an_ambiguous_commit_receipt() {
    let fixture = StorageFixture::new();
    let credentials = Arc::new(FaultInjectingModelCredentialStore::default());
    let service = StorageService::open_with_model_credentials(
        &fixture.root.join("storage.sqlite"),
        credentials.clone(),
    )
    .unwrap();
    service
        .save_model_settings_request(renderer_save_request(
            &revision_test_settings(),
            serde_json::json!({"kind": "select_generic"}),
            None,
        ))
        .unwrap();
    let active_ref = {
        let mut connection = service.state.connection().unwrap();
        config_repository::load_model_settings(&mut connection)
            .unwrap()
            .unwrap()
            .api_token_ref
            .unwrap()
    };
    {
        let mut connection = service.state.connection().unwrap();
        config_repository::stage_model_provider_credentials(
            &mut connection,
            std::slice::from_ref(&active_ref),
        )
        .unwrap();
    }
    let active_reference = CredentialReference::parse(active_ref).unwrap();

    let report = service.reconcile_model_provider_credentials().unwrap();
    assert_eq!(report.completed_staging, 1);
    assert_eq!(report.removed_orphaned_credentials, 0);
    assert!(credentials.get(&active_reference).unwrap().is_some());
    assert_eq!(
        model_credential_journal_count(&service, "model_provider_credential_staging"),
        0
    );
}

#[test]
fn failed_retired_credential_delete_keeps_a_retryable_cleanup_receipt() {
    let fixture = StorageFixture::new();
    let credentials = Arc::new(FaultInjectingModelCredentialStore::default());
    let service = StorageService::open_with_model_credentials(
        &fixture.root.join("storage.sqlite"),
        credentials.clone(),
    )
    .unwrap();
    let configured = service
        .save_model_settings_request(renderer_save_request(
            &revision_test_settings(),
            serde_json::json!({"kind": "select_generic"}),
            None,
        ))
        .unwrap();

    credentials.fail_delete.store(true, Ordering::SeqCst);
    let mut clear = renderer_save_request_preserving_credentials(
        &configured,
        serde_json::json!({"kind": "unchanged"}),
        Some(&configured.models[0].id),
    );
    clear.api_token_mutation = CredentialMutation::Clear;
    let cleared = service.save_model_settings_request(clear).unwrap();
    assert_eq!(cleared.api_token_status, CredentialStatus::Missing);
    assert_eq!(
        model_credential_journal_count(&service, "model_provider_credential_cleanup"),
        1
    );

    credentials.fail_delete.store(false, Ordering::SeqCst);
    let report = service.reconcile_model_provider_credentials().unwrap();
    assert_eq!(report.removed_retired_credentials, 1);
    assert_eq!(
        model_credential_journal_count(&service, "model_provider_credential_cleanup"),
        0
    );
}

#[test]
fn startup_reconciliation_deletes_an_unpublished_model_credential() {
    let fixture = StorageFixture::new();
    let credentials = Arc::new(FaultInjectingModelCredentialStore::default());
    let service = StorageService::open_with_model_credentials(
        &fixture.root.join("storage.sqlite"),
        credentials.clone(),
    )
    .unwrap();
    let reference = credentials.new_reference();
    {
        let mut connection = service.state.connection().unwrap();
        config_repository::stage_model_provider_credentials(
            &mut connection,
            &[reference.as_str().to_string()],
        )
        .unwrap();
    }
    credentials
        .replace(
            &reference,
            CredentialSecret::new("unpublished-test-secret").unwrap(),
        )
        .unwrap();

    let report = service.reconcile_model_provider_credentials().unwrap();
    assert_eq!(report.removed_orphaned_credentials, 1);
    assert_eq!(
        model_credential_journal_count(&service, "model_provider_credential_staging"),
        0
    );
    assert!(credentials.get(&reference).unwrap().is_none());
}

#[test]
fn legacy_raw_secret_fields_are_rejected_by_the_save_wire() {
    let mut value = renderer_save_request_value(&revision_test_settings());
    value
        .as_object_mut()
        .unwrap()
        .insert("apiToken".to_string(), serde_json::json!("legacy-secret"));
    assert!(serde_json::from_value::<ModelSettingsSaveRequest>(value).is_err());
}

#[test]
fn save_request_rejects_renderer_capabilities_versions_and_unknown_settings() {
    let valid_update = serde_json::json!({
        "kind": "select_vendor",
        "vendorId": "deepseek",
        "settings": {
            "kind": "deepseek_flash_chat",
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
fn incomplete_model_override_is_saved_for_editing_but_never_mixes_with_global_connection() {
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

    let mut incomplete = valid;
    incomplete.models[0].api_token_override = None;
    service.save_model_settings(incomplete).unwrap();

    let stored = service.load_model_settings().unwrap().unwrap();
    assert_eq!(
        stored.models[0].api_url_override.as_deref(),
        Some("https://model.example/v1")
    );
    assert_eq!(stored.models[0].api_token_override.as_deref(), None);
    assert!(stored.effective_connection_for(&stored.models[0]).is_err());
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

#[test]
fn web_search_policy_resolves_only_current_search_credential_and_degrades_missing_key() {
    const SEARCH: &str = "web-search-policy-secret-canary";
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let mut settings = revision_test_settings();
    settings.search_mode = "auto".to_string();
    settings.tavily_api_key = SEARCH.to_string();
    service.save_model_settings(settings).unwrap();
    delete_stored_credential(&fixture, "api_token_ref", None);

    assert!(service
        .load_web_search_policy_snapshot()
        .unwrap()
        .available());
    let credential = service.authorize_web_search_execution().unwrap();
    assert!(!format!("{credential:?}").contains(SEARCH));
    assert_eq!(credential.into_secret(), SEARCH);

    delete_stored_credential(&fixture, "tavily_api_key_ref", None);
    let snapshot = service.load_web_search_policy_snapshot().unwrap();
    assert!(snapshot.enabled);
    assert!(!snapshot.credential_ready);
    let error = service.authorize_web_search_execution().unwrap_err();
    assert!(!error.to_string().contains(SEARCH));
    assert_eq!(error.code(), Some("web_search.configuration_required"));
}
