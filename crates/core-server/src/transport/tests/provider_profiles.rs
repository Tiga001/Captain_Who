use super::*;

fn request(method: &str, params: Option<Value>) -> JsonRpcRequest {
    JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: JsonRpcId::Number(71),
        method: method.to_string(),
        params,
    }
}

fn generic_model_save_payload(models: Value) -> Value {
    serde_json::json!({
        "apiUrl": "https://provider-secret.example/v1/chat/completions",
        "apiToken": "fixed-secret-token-must-not-cross",
        "searchMode": "disabled",
        "tavilyApiKey": "fixed-search-secret-must-not-cross",
        "models": models,
    })
}

fn generic_model(model_id: &str, input_price: &str) -> Value {
    serde_json::json!({
        "id": model_id,
        "previousModelId": null,
        "displayName": "Generic Model",
        "apiUrlOverride": null,
        "apiTokenOverride": null,
        "supportsImage": false,
        "contextWindowTokens": 128000,
        "providerProfileUpdate": {"kind": "select_generic"},
        "inputPrice": input_price,
        "cachedInputPrice": "",
        "outputPrice": "0",
        "enabled": true,
    })
}

#[test]
fn duplicate_model_id_is_returned_as_safe_stable_validation_data() {
    let temp = tempfile::tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&temp.path().join("storage.sqlite")).unwrap());
    let agent_service = AgentService::new(Arc::clone(&storage));
    let (notifications, _receiver) = tokio::sync::mpsc::unbounded_channel();

    let response = handle_request(
        storage.as_ref(),
        &agent_service,
        notifications,
        request(
            STORAGE_SAVE_MODEL_SETTINGS_METHOD,
            Some(generic_model_save_payload(serde_json::json!([
                generic_model("  deepseek-v4-flash  ", "0"),
                generic_model("  deepseek-v4-flash  ", "0"),
            ]))),
        ),
    );

    assert_eq!(response["error"]["code"], -32000);
    assert_eq!(
        response["error"]["message"],
        "Model settings validation failed."
    );
    assert_eq!(
        response["error"]["data"],
        serde_json::json!({
            "kind": "model_settings_validation",
            "code": "duplicate_model_id",
            "modelId": "deepseek-v4-flash",
        })
    );
    let encoded = response.to_string();
    assert!(!encoded.contains("fixed-secret-token-must-not-cross"));
    assert!(!encoded.contains("provider-secret.example"));
}

#[test]
fn unknown_model_settings_failures_are_redacted_without_error_data() {
    let temp = tempfile::tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&temp.path().join("storage.sqlite")).unwrap());
    let agent_service = AgentService::new(Arc::clone(&storage));
    let (notifications, _receiver) = tokio::sync::mpsc::unbounded_channel();

    let response = handle_request(
        storage.as_ref(),
        &agent_service,
        notifications,
        request(
            STORAGE_SAVE_MODEL_SETTINGS_METHOD,
            Some(generic_model_save_payload(serde_json::json!([
                generic_model("model-a", "not-a-price-fixed-canary")
            ]))),
        ),
    );

    assert_eq!(response["error"]["code"], -32000);
    assert_eq!(
        response["error"]["message"],
        "Model settings could not be saved."
    );
    assert!(response["error"].get("data").is_none());
    let encoded = response.to_string();
    for secret in [
        "not-a-price-fixed-canary",
        "fixed-secret-token-must-not-cross",
        "fixed-search-secret-must-not-cross",
        "provider-secret.example",
        "SELECT",
        "SQLITE",
    ] {
        assert!(!encoded.contains(secret), "leaked unsafe detail: {secret}");
    }
}

#[test]
fn provider_profile_descriptor_projection_and_authoritative_save_are_strict() {
    let temp = tempfile::tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&temp.path().join("storage.sqlite")).unwrap());
    let agent_service = AgentService::new(Arc::clone(&storage));
    let (notifications, _receiver) = tokio::sync::mpsc::unbounded_channel();

    let descriptors = handle_request(
        storage.as_ref(),
        &agent_service,
        notifications.clone(),
        request(STORAGE_LOAD_PROVIDER_PROFILE_UI_DESCRIPTORS_METHOD, None),
    );
    let deepseek = descriptors["result"]
        .as_array()
        .unwrap()
        .iter()
        .find(|descriptor| descriptor["profileId"] == "deepseek_v4_chat")
        .unwrap();
    assert_eq!(deepseek["profileVersion"], 1);
    assert_eq!(deepseek["settingsKind"], "deepseek_v4_chat");
    assert!(deepseek["selectable"].as_bool().unwrap());
    assert!(deepseek.get("runtimeCapabilities").is_none());
    assert!(deepseek.get("continuationRequirement").is_none());

    let response = handle_request(
        storage.as_ref(),
        &agent_service,
        notifications,
        request(
            STORAGE_SAVE_MODEL_SETTINGS_METHOD,
            Some(serde_json::json!({
                "apiUrl": "https://api.deepseek.com/v1/chat/completions",
                "apiToken": "test-only-token",
                "searchMode": "disabled",
                "tavilyApiKey": "",
                "models": [{
                    "id": "deepseek-chat",
                    "previousModelId": null,
                    "displayName": "DeepSeek Chat",
                    "apiUrlOverride": null,
                    "apiTokenOverride": null,
                    "supportsImage": false,
                    "contextWindowTokens": 128000,
                    "providerProfileUpdate": {
                        "kind": "select_registered_profile",
                        "profileId": "deepseek_v4_chat",
                        "settings": {
                            "kind": "deepseek_v4_chat",
                            "reasoning": {"mode": "enabled", "effort": "high"}
                        }
                    },
                    "inputPrice": "0",
                    "cachedInputPrice": "",
                    "outputPrice": "0",
                    "enabled": true
                }]
            })),
        ),
    );

    assert_eq!(
        response["result"]["models"][0]["providerProfileConfig"],
        serde_json::json!({
            "schemaVersion": 1,
            "profile": {"id": "deepseek_v4_chat", "version": 1},
            "reasoning": {"mode": "enabled", "effort": "high"}
        })
    );
    assert!(response["result"]["models"][0]
        .get("providerProfileUpdate")
        .is_none());
}

#[test]
fn renderer_cannot_submit_profile_version_revision_or_capabilities() {
    let temp = tempfile::tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&temp.path().join("storage.sqlite")).unwrap());
    let agent_service = AgentService::new(Arc::clone(&storage));
    let (notifications, _receiver) = tokio::sync::mpsc::unbounded_channel();
    let response = handle_request(
        storage.as_ref(),
        &agent_service,
        notifications,
        request(
            STORAGE_SAVE_MODEL_SETTINGS_METHOD,
            Some(serde_json::json!({
                "apiUrl": "https://api.deepseek.com/v1/chat/completions",
                "apiToken": "test-only-token",
                "searchMode": "disabled",
                "tavilyApiKey": "",
                "providerConfigurationRevision": "forged",
                "models": []
            })),
        ),
    );
    assert_eq!(response["error"]["code"], -32602);
}
