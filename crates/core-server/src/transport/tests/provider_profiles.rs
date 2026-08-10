use super::*;

fn request(method: &str, params: Option<Value>) -> JsonRpcRequest {
    JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: JsonRpcId::Number(71),
        method: method.to_string(),
        params,
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
                    "displayName": "DeepSeek Chat",
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
