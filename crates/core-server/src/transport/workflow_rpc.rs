use super::*;
use mycopilot_core::storage::workflow_repository::Error;
use mycopilot_core::workflow::Request;

pub(crate) fn handle_workflow_request(storage: &StorageService, request: JsonRpcRequest) -> Value {
    let input = match parse_params::<Request>(request.params) {
        Ok(input) => input,
        Err(message) => return response_error(Some(request.id), -32602, message),
    };
    match storage.workflow_request(input) {
        Ok(output) => response_success(request.id, output),
        Err(error) => {
            let code = match &error {
                Error::Invalid(_) => -32602,
                Error::Conflict(_) => -32009,
                Error::Storage(_) => -32000,
            };
            response_error(Some(request.id), code, error.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn call(storage: &StorageService, params: Value) -> Value {
        handle_workflow_request(
            storage,
            JsonRpcRequest {
                jsonrpc: "2.0".into(),
                id: JsonRpcId::Number(1),
                method: "agent.workflows.request".into(),
                params: Some(params),
            },
        )
    }

    #[test]
    fn workflow_rpc_saves_drafts_and_rejects_bad_parameters_and_conflicting_revisions() {
        let directory = tempfile::tempdir().unwrap();
        let storage = StorageService::open(&directory.path().join("workflow.sqlite")).unwrap();
        let definition = json!({
            "schemaVersion": 1, "id": "draft", "name": "Draft", "description": "", "background": "",
            "nodes": [], "flows": [], "viewport": {"x":0,"y":0,"zoom":1}
        });
        let saved = call(
            &storage,
            json!({"operation":"save", "definition":definition,"expectedRevision":0}),
        );
        assert_eq!(saved["result"]["records"][0]["revision"], 1);
        assert_eq!(
            saved["result"]["records"][0]["definition"]["boundaryPositions"]["input"]["x"].as_f64(),
            Some(80.0)
        );
        assert_eq!(
            saved["result"]["records"][0]["definition"]["boundaryPositions"]["output"]["x"]
                .as_f64(),
            Some(760.0)
        );
        assert!(!saved["result"]["records"][0]["issues"]
            .as_array()
            .unwrap()
            .is_empty());
        let conflict = call(
            &storage,
            json!({"operation":"save","definition":definition,"expectedRevision":0}),
        );
        assert_eq!(conflict["error"]["code"], -32009);
        let invalid = call(
            &storage,
            json!({"operation":"delete","id":"draft","expectedRevision":-1}),
        );
        assert_eq!(invalid["error"]["code"], -32602);
        let unexpected = call(&storage, json!({"operation":"list","extra":true}));
        assert_eq!(unexpected["error"]["code"], -32602);
        let mut malformed = definition.clone();
        malformed["boundaryPositions"] = Value::Null;
        let rejected = call(
            &storage,
            json!({"operation":"save","definition":malformed,"expectedRevision":1}),
        );
        assert_eq!(rejected["error"]["code"], -32602);
        let deleted = call(
            &storage,
            json!({"operation":"delete","id":"draft","expectedRevision":1}),
        );
        assert_eq!(deleted["result"]["records"], json!([]));
    }
}
