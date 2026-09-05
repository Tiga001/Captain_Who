use super::*;

#[test]
fn manual_context_compaction_rpc_rejects_client_selected_context_and_dispatches_status() {
    let directory = tempfile::tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&directory.path().join("storage.sqlite")).unwrap());
    let service = AgentService::new(storage.clone());
    let (notifications, _receiver) = tokio::sync::mpsc::unbounded_channel();
    for field in [
        "coveredThroughMessageId",
        "sourceRevision",
        "modelId",
        "profile",
    ] {
        let mut params =
            serde_json::json!({"conversationId":"conversation", "requestId":"request"});
        params[field] = serde_json::json!("untrusted");
        let response = handle_request(
            &storage,
            &service,
            notifications.clone(),
            JsonRpcRequest {
                jsonrpc: "2.0".into(),
                id: JsonRpcId::Number(1),
                method: mycopilot_protocol_rs::AGENT_START_MANUAL_CONTEXT_COMPACTION_METHOD.into(),
                params: Some(params),
            },
        );
        assert_eq!(response["error"]["code"], -32602, "{response}");
    }
    let response = handle_request(
        &storage,
        &service,
        notifications,
        JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: JsonRpcId::Number(2),
            method: mycopilot_protocol_rs::AGENT_GET_MANUAL_CONTEXT_COMPACTION_STATUS_METHOD.into(),
            params: Some(serde_json::json!({"conversationId":"conversation"})),
        },
    );
    assert_eq!(
        response["result"]["operations"],
        serde_json::json!([]),
        "{response}"
    );
}
