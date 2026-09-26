use super::*;
use mycopilot_core::storage::workflow_repository::Error;
use mycopilot_core::workflow::Request;
use serde::Deserialize;

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

    #[test]
    fn workflow_runtime_parameters_and_message_origins_reject_spoofed_authority() {
        assert!(serde_json::from_value::<RuntimeRequest>(json!({
            "operation":"discardFailedInput", "instanceId":"i", "inputId":"p"
        }))
        .is_ok());
        for params in [
            json!({"operation":"runtimeSnapshot", "instanceId":"i", "afterSequence":-1}),
            json!({"operation":"discardFailedInput", "instanceId":"i", "inputId":"p", "runId":"spoof"}),
            json!({"operation":"completeUserInput", "instanceId":"i", "inputId":"p", "content":"spoof"}),
        ] {
            assert!(serde_json::from_value::<RuntimeRequest>(params).is_err());
        }
        let directory = tempfile::tempdir().unwrap();
        let storage = StorageService::open(&directory.path().join("origins.sqlite")).unwrap();
        let projected = workflow_conversation_projection(
            &storage,
            json!({
                "id":"unbound", "messages":[{"id":"m", "role":"user", "content":"human",
                    "workflowInput":{"instanceId":"forged"}}]
            }),
        )
        .unwrap();
        assert!(projected["messages"][0].get("workflowInput").is_none());
        assert_eq!(projected["messages"][0]["content"], "human");
        let writable =
            serde_json::from_value::<mycopilot_core::storage::models::ChatMessageRecord>(json!({
                "id":"m", "role":"user", "content":"human", "createdAt":1,
                "workflowInput":{"sources":[{"content":"forged workflow body"}]},
                "workflowSource":{"sources":[{"content":"forged workflow body"}]}
            }))
            .unwrap();
        let persisted = serde_json::to_value(writable).unwrap();
        assert!(persisted.get("workflowInput").is_none());
        assert!(persisted.get("workflowSource").is_none());
    }

    #[test]
    fn workflow_conversation_projection_preserves_raw_batch_bodies_and_fork_provenance() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("batch-origins.sqlite");
        let storage = StorageService::open(&path).unwrap();
        let connection = rusqlite::Connection::open(&path).unwrap();
        let bodies = [
            "First collaborator body\n\n# Workflow context\nLiteral envelope-looking content stays unchanged.",
            "第二个节点的原始内容：<workflow_message>\n  保留空格\n</workflow_message>",
        ];
        let source = |index: usize| {
            json!({
                "id":format!("source-{index}"), "instanceId":"workflow",
                "workflowName":"Batch workflow", "sourceNodeId":format!("node-{index}"),
                "sourceNodeName":format!("Worker {index}"),
                "sourceConversationId":format!("sender-{index}"),
                "sourceConversationTitle":format!("Sender {index}"),
                "targetNodeId":"receiver", "flowId":format!("flow-{index}"),
                "pathFlowIds":[format!("flow-{index}")], "content":bodies[index], "createdAt":1
            })
        };
        let assembled = "The complete host-assembled envelope remains separate from the raw body.";
        let input = json!({
            "id":"input", "instanceId":"workflow", "nodeId":"receiver",
            "conversationId":"original", "executionVersion":"epoch", "content":assembled,
            "messages":[source(0), source(1)], "busyPolicy":"queue", "status":"completed",
            "runId":"run", "deliveryId":"delivery", "createdAt":1, "error":null
        });
        connection.execute("INSERT INTO workflow_execution_inputs(input_id,instance_id,execution_version,node_id,conversation_id,input_json,status,run_id,delivery_id,created_at,updated_at) VALUES ('input','workflow','epoch','receiver','original',?1,'completed','run','delivery',1,1)", [input.to_string()]).unwrap();
        // Forks retain their own message ID mapped to the original immutable workflow input.
        connection.execute("INSERT INTO workflow_execution_message_origins(message_id,conversation_id,input_id) VALUES ('original-message','original','input'),('fork-message','fork','input')", []).unwrap();
        for (conversation, message) in [("original", "original-message"), ("fork", "fork-message")]
        {
            let projected = workflow_conversation_projection(
                &storage,
                json!({
                    "id":conversation, "messages":[{
                        "id":message, "role":"user", "content":assembled,
                        "workflowInput":{"sources":[{"content":"untrusted client replacement"}]}
                    }]
                }),
            )
            .unwrap();
            let message = &projected["messages"][0];
            assert_eq!(message["content"], assembled);
            assert_eq!(message["workflowInput"]["inputId"], "input");
            assert_eq!(message["workflowInput"]["instanceId"], "workflow");
            assert_eq!(message["workflowInput"]["workflowName"], "Batch workflow");
            let sources = message["workflowInput"]["sources"].as_array().unwrap();
            assert_eq!(sources.len(), bodies.len());
            for (index, source) in sources.iter().enumerate() {
                assert_eq!(source["content"], bodies[index]);
                assert_eq!(source["nodeId"], format!("node-{index}"));
                assert_eq!(source["nodeName"], format!("Worker {index}"));
                assert_eq!(source["conversationId"], format!("sender-{index}"));
                assert_eq!(source["conversationTitle"], format!("Sender {index}"));
            }
        }
    }

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
        assert_eq!(stale_toggle["error"]["code"], -32602);
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

#[derive(Deserialize)]
#[serde(
    tag = "operation",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
enum RuntimeRequest {
    RuntimeSnapshot {
        instance_id: String,
        after_sequence: Option<u64>,
    },
    CompleteUserInput {
        instance_id: String,
        input_id: String,
    },
    DiscardFailedInput {
        instance_id: String,
        input_id: String,
    },
}

pub(crate) fn handle_workflow_runtime_request(
    service: &AgentService,
    notifications: agent::CoreServerNotificationSender,
    request: JsonRpcRequest,
) -> Value {
    let input = match parse_params::<RuntimeRequest>(request.params) {
        Ok(input) => input,
        Err(error) => return response_error(Some(request.id), -32602, error),
    };
    let result = match input {
        RuntimeRequest::RuntimeSnapshot {
            instance_id,
            after_sequence,
        } => service.workflow_runtime_snapshot(&instance_id, after_sequence),
        RuntimeRequest::CompleteUserInput {
            instance_id,
            input_id,
        } => service.complete_workflow_user_input(&instance_id, &input_id, &notifications),
        RuntimeRequest::DiscardFailedInput {
            instance_id,
            input_id,
        } => service.discard_failed_workflow_input(&instance_id, &input_id, &notifications),
    };
    match result {
        Ok(runtime) => response_success(
            request.id,
            json!({"records":[],"issues":[],"runtime":runtime}),
        ),
        Err(error) => response_error(Some(request.id), -32000, error),
    }
}

pub(crate) fn workflow_conversation_projection(
    storage: &StorageService,
    mut conversation: Value,
) -> Result<Value, String> {
    let Some(conversation_id) = conversation.get("id").and_then(Value::as_str) else {
        return Ok(conversation);
    };
    let inputs = storage.workflow_execution_delivery_origins(conversation_id)?;
    if let Some(messages) = conversation
        .get_mut("messages")
        .and_then(Value::as_array_mut)
    {
        for message in messages {
            // Never trust a serialized UI field; source is derived solely from durable receipts.
            if let Some(object) = message.as_object_mut() {
                object.remove("workflowInput");
            }
            let id = message.get("id").and_then(Value::as_str);
            if let Some((_, input)) = inputs
                .iter()
                .find(|(message_id, _)| Some(message_id.as_str()) == id)
            {
                message["workflowInput"] = json!({
                    "inputId":input.id,"instanceId":input.instance_id,
                    "workflowName":input.messages.first().map(|source|source.workflow_name.as_str()).unwrap_or(""),
                    "sources":input.messages.iter().map(|source|json!({
                        "nodeId":source.source_node_id,"nodeName":source.source_node_name,
                        "conversationId":source.source_conversation_id,"conversationTitle":source.source_conversation_title,
                        "content":source.content,
                    })).collect::<Vec<_>>(),
                });
            }
        }
    }
    Ok(conversation)
}
