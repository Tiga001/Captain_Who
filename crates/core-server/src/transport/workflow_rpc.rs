use super::*;
use mycopilot_core::storage::workflow_repository::Error;
use mycopilot_core::workflow::Request;

pub(crate) fn handle_workflow_request(
    storage: &StorageService,
    agent_service: Option<&AgentService>,
    request: JsonRpcRequest,
) -> Value {
    let input = match parse_params::<Request>(request.params) {
        Ok(input) => input,
        Err(message) => return response_error(Some(request.id), -32602, message),
    };
    let result = match agent_service {
        Some(agent_service) => agent_service.workflow_request(input),
        None => storage.workflow_request(input),
    };
    match result {
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
            None,
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
            "nodes": [], "flows": [], "viewport": {"x":0,"y":0,"zoom":1},
            "boundaryPositions": {"input":{"x":80,"y":220}}
        });
        let saved = call(
            &storage,
            json!({"operation":"save", "definition":definition,"expectedRevision":0}),
        );
        assert_eq!(saved["result"]["records"][0]["revision"], 1);
        assert_eq!(saved["result"]["records"][0]["enabled"], false);
        assert_eq!(
            saved["result"]["records"][0]["definition"]["boundaryPositions"]["input"]["x"].as_f64(),
            Some(80.0)
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
        for enabled in [true, false] {
            let invalid_toggle = call(
                &storage,
                json!({
                    "operation":"setEnabled","id":"draft","enabled":enabled,"expectedRevision":1,
                }),
            );
            assert_eq!(invalid_toggle["error"]["code"], -32602);
        }
        let stale_toggle = call(
            &storage,
            json!({
                "operation":"setEnabled","id":"draft","enabled":true,"expectedRevision":2,
            }),
        );
        assert_eq!(stale_toggle["error"]["code"], -32009);
        let after_toggle = call(&storage, json!({"operation":"list"}));
        assert_eq!(after_toggle["result"]["records"][0]["revision"], 1);
        assert_eq!(after_toggle["result"]["records"][0]["enabled"], false);
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
    #[test]
    fn workflow_rpc_preserves_logic_gates_and_rejects_agent_fields_on_gates() {
        let directory = tempfile::tempdir().unwrap();
        let storage = StorageService::open(&directory.path().join("gates.sqlite")).unwrap();
        let definition: Value = serde_json::from_str(include_str!(
            "../../../../packages/protocol/fixtures/workflow-definition-v1.json"
        ))
        .unwrap();
        let saved = call(
            &storage,
            json!({
                "operation":"save", "definition": definition, "expectedRevision":0
            }),
        );
        assert_eq!(saved["result"]["records"][0]["revision"], 1);
        let record = &saved["result"]["records"][0];
        assert_eq!(record["definition"]["nodes"][2]["kind"], "inputGate");
        assert_eq!(record["definition"]["nodes"][2]["busyPolicy"], "queue");
        assert_eq!(record["definition"]["nodes"][3]["selection"]["mode"], "one");
        assert_eq!(record["issues"].as_array().unwrap().len(), 3);
        assert!(record["issues"]
            .as_array()
            .unwrap()
            .iter()
            .all(|issue| issue["code"] == "node_model"));
        let mut invalid = definition.clone();
        invalid["nodes"][2]["task"] = json!("A gate must not execute a task");
        let rejected = call(
            &storage,
            json!({
                "operation":"save", "definition":invalid, "expectedRevision":1
            }),
        );
        assert_eq!(rejected["error"]["code"], -32602);
        let listed = call(&storage, json!({"operation":"list"}));
        assert_eq!(listed["result"]["records"][0]["revision"], 1);
    }
}

#[cfg(test)]
mod management_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn workflow_rpc_exposes_global_instances_and_isolated_drafts_without_execution_controls() {
        let directory = tempfile::tempdir().unwrap();
        let storage = StorageService::open(&directory.path().join("instances.sqlite")).unwrap();
        let call = |params| {
            handle_workflow_request(
                &storage,
                None,
                JsonRpcRequest {
                    jsonrpc: "2.0".into(),
                    id: JsonRpcId::Number(1),
                    method: "agent.workflows.request".into(),
                    params: Some(params),
                },
            )
        };
        let empty = call(json!({"operation":"listInstances"}));
        assert_eq!(empty["result"]["instances"], json!([]));
        let definition = json!({"schemaVersion":1,"id":"template","name":"Original","description":"","background":"","nodes":[],"flows":[],"viewport":{"x":0,"y":0,"zoom":1},"boundaryPositions":{"input":{"x":0,"y":0}}});
        call(json!({"operation":"save","definition":definition,"expectedRevision":0}));
        let mut edited = definition.clone();
        edited["name"] = json!("Editing draft");
        let stashed = call(
            json!({"operation":"saveDraft","definition":edited,"expectedRevision":1,"expectedDraftRevision":0}),
        );
        assert_eq!(
            stashed["result"]["records"][0]["definition"]["name"],
            "Original"
        );
        assert_eq!(
            stashed["result"]["drafts"][0]["definition"]["name"],
            "Editing draft"
        );
        let copied = call(
            json!({"operation":"duplicate","id":"template","expectedRevision":1,"newId":"copy","name":"Copy"}),
        );
        assert_eq!(copied["result"]["records"].as_array().unwrap().len(), 2);
        for params in [
            json!({"operation":"listInstances","projectId":"p"}),
            json!({"operation":"startInstance","id":"instance"}),
        ] {
            assert_eq!(call(params)["error"]["code"], -32602);
        }
    }
}
