use super::*;

#[test]
fn profile_save_notifies_only_persisted_metadata_and_preserves_preferences() {
    let directory = tempfile::tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&directory.path().join("storage.sqlite")).unwrap());
    let service = AgentService::new_authorized_for_test(storage.clone());
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let save = |profile: &str| {
        handle_request(
            &storage,
            &service,
            notifications.clone(),
            JsonRpcRequest {
                jsonrpc: "2.0".into(),
                id: JsonRpcId::Number(1),
                method: mycopilot_protocol_rs::STORAGE_SAVE_AGENT_PROMPT_PREFERENCES_METHOD.into(),
                params: Some(json!({ "contextProfile": profile, "workMode": "general",
                "tone": "friendly", "detailLevel": "high", "customInstructions": "Keep private preference", "updatedAt": 0 })),
            },
        )
    };
    for profile in ["minimal", "full"] {
        let response = save(profile);
        assert_eq!(response["result"]["contextProfile"], profile);
        assert_eq!(
            response["result"]["customInstructions"],
            "Keep private preference"
        );
        let event = receiver.try_recv().unwrap();
        assert_eq!(
            event["method"],
            mycopilot_protocol_rs::AGENT_PROMPT_PREFERENCES_CHANGED_METHOD
        );
        assert_eq!(
            event["params"],
            json!({"contextProfile": profile, "updatedAt": response["result"]["updatedAt"]})
        );
    }
    assert!(save("invalid")["error"].is_object());
    assert!(receiver.try_recv().is_err());
    assert_eq!(
        storage
            .load_agent_prompt_preferences()
            .unwrap()
            .context_profile,
        mycopilot_core::AgentContextProfile::Full
    );
}
