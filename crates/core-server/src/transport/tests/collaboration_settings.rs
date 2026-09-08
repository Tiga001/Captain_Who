use super::*;

#[test]
fn settings_rpc_persists_and_notifies_only_successful_revision_changes() {
    let directory = tempfile::tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&directory.path().join("storage.sqlite")).unwrap());
    let service = AgentService::new(storage.clone());
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let call = |method: &str, params: Value| {
        handle_request(
            &storage,
            &service,
            notifications.clone(),
            JsonRpcRequest {
                jsonrpc: "2.0".into(),
                id: JsonRpcId::Number(1),
                method: method.into(),
                params: Some(params),
            },
        )
    };
    let current = call(
        mycopilot_protocol_rs::AGENT_COLLABORATION_GET_SETTINGS_METHOD,
        json!({}),
    );
    assert_eq!(
        current["result"],
        json!({"enabled":true,"revision":1,"updatedAt":0})
    );
    assert!(call(
        mycopilot_protocol_rs::AGENT_COLLABORATION_GET_SETTINGS_METHOD,
        json!({"enabled":false})
    )["error"]
        .is_object());
    let updated = call(
        mycopilot_protocol_rs::AGENT_COLLABORATION_UPDATE_SETTINGS_METHOD,
        json!({"enabled":false,"expectedRevision":1}),
    );
    assert_eq!(updated["result"]["enabled"], false);
    assert_eq!(updated["result"]["revision"], 2);
    let event = receiver.try_recv().unwrap();
    assert_eq!(
        event["method"],
        mycopilot_protocol_rs::AGENT_COLLABORATION_SETTINGS_CHANGED_METHOD
    );
    assert_eq!(event["params"], updated["result"]);
    assert!(call(
        mycopilot_protocol_rs::AGENT_COLLABORATION_UPDATE_SETTINGS_METHOD,
        json!({"enabled":true,"expectedRevision":1})
    )["error"]
        .is_object());
    assert!(receiver.try_recv().is_err());
    assert!(!storage.load_agent_collaboration_settings().unwrap().enabled);
}
