use super::*;
use mycopilot_core::storage::workflow_repository::{Error, MEMBER_MODELS_UNAVAILABLE_CODE};
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
        Err(Error::MemberModelsUnavailable { members }) => serde_json::to_value(error_with_data(
            Some(request.id),
            -32602,
            MEMBER_MODELS_UNAVAILABLE_CODE,
            serde_json::json!({"code": MEMBER_MODELS_UNAVAILABLE_CODE, "members": members}),
        ))
        .expect("Organization model availability error must serialize"),
        Err(Error::Invalid(message)) => response_error(Some(request.id), -32602, message),
        Err(Error::Conflict(message)) => response_error(Some(request.id), -32009, message),
        Err(Error::Storage(message)) => response_error(Some(request.id), -32000, message),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn workflow_runtime_parameters_and_message_origins_reject_spoofed_authority() {
        assert!(serde_json::from_value::<RuntimeRequest>(
            json!({"operation":"runtimeSnapshot", "instanceId":"i", "afterSequence":20})
        )
        .is_ok());
        assert!(matches!(
            serde_json::from_value::<RuntimeRequest>(json!({
                "operation":"runtimeSnapshot", "instanceId":"i", "summaryOnly":true
            }))
            .unwrap(),
            RuntimeRequest::RuntimeSnapshot {
                summary_only: true,
                ..
            }
        ));
        assert!(serde_json::from_value::<RuntimeRequest>(json!({
            "operation":"discardFailedInput", "instanceId":"i", "inputId":"p"
        }))
        .is_err());
        for params in [
            json!({"operation":"runtimeSnapshot", "instanceId":"i", "resume":true}),
            json!({"operation":"runtimeSnapshot", "instanceId":"i", "afterSequence":-1}),
            json!({"operation":"runtimeSnapshot", "instanceId":"i", "summaryOnly":"true"}),
            json!({"operation":"runtimeSnapshot", "instanceId":"i", "summaryOnly":null}),
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
    fn workflow_conversation_projection_preserves_raw_mail_bodies_and_fork_provenance() {
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
                "workflowName":"Mail workflow", "sourceNodeId":format!("node-{index}"),
                "sourceNodeName":format!("Worker {index}"),
                "sourceConversationId":format!("sender-{index}"),
                "sourceConversationTitle":format!("Sender {index}"),
                "targetNodeId":"receiver", "targetNodeName":"Receiver", "targetConversationId":"original", "targetConversationTitle":"Original", "replyToMessageId":null, "content":bodies[index], "createdAt":1
            })
        };
        let assembled = "The complete host-assembled envelope remains separate from the raw body.";
        let input = json!({
            "id":"input", "instanceId":"workflow", "nodeId":"receiver",
            "conversationId":"original", "executionVersion":"epoch", "content":assembled,
            "messages":[source(0)], "mailStatus":"processed", "status":"completed",
            "runId":"run", "deliveryId":"delivery", "createdAt":1, "error":null
        });
        connection.execute("INSERT INTO workflow_mail_inputs(input_id,instance_id,execution_version,node_id,conversation_id,input_json,status,run_id,delivery_id,created_at,updated_at) VALUES ('input','workflow','epoch','receiver','original',?1,'completed','run','delivery',1,1)", [input.to_string()]).unwrap();
        // Forks retain their own message ID mapped to the original immutable workflow input.
        connection.execute("INSERT INTO workflow_mail_message_origins(message_id,conversation_id,input_id) VALUES ('original-message','original','input'),('fork-message','fork','input')", []).unwrap();
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
            assert_eq!(message["workflowInput"]["workflowName"], "Mail workflow");
            let sources = message["workflowInput"]["sources"].as_array().unwrap();
            assert_eq!(sources.len(), 1);
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
    fn workflow_rpc_reads_one_board_through_the_json_request_entry() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("single-board.sqlite");
        let storage = StorageService::open(&path).unwrap();
        let connection = rusqlite::Connection::open(&path).unwrap();
        for id in ["board", "other-board"] {
            let definition = json!({
                "schemaVersion":1,"id":id,"name":id,"description":"","background":"",
                "nodes":[],"viewport":{"x":0,"y":0,"zoom":1}
            });
            connection.execute(
                "INSERT INTO workflow_instances(instance_id,template_id,template_revision,name,color,revision,updated_at,needs_review,running,last_request_json,enabled,definition_json) VALUES(?1,'template',1,?1,'#123456',1,1,0,0,'{}',1,?2)",
                rusqlite::params![id, definition.to_string()],
            ).unwrap();
        }
        let query = json!({"operation":"getInstance","instanceId":"board"});
        for params in [
            query.clone(),
            json!({"operation":"getInstance","instanceId":"board","includeActivity":false}),
            json!({"operation":"getInstance","instanceId":"board","includeActivity":true}),
        ] {
            let response = call(&storage, params);
            assert!(response.get("error").is_none(), "{response}");
            let result = &response["result"];
            let instances = result["instances"].as_array().unwrap();
            assert_eq!(instances.len(), 1);
            assert_eq!(instances[0]["id"], "board");
            assert_eq!(instances[0]["definition"]["id"], "board");
            assert_eq!(result["records"], json!([]));
            assert_eq!(result["drafts"], json!([]));
        }

        // Keep the optimized board route independent of authoring and activity reads,
        // exercising the same JSON entry as the sidebar instead of constructing an enum.
        connection.execute_batch("DROP TABLE workflow_editing_drafts; DROP TABLE workflow_definitions; DROP TABLE workflow_mail_runs;").unwrap();
        let response = call(&storage, query);
        assert!(response.get("error").is_none(), "{response}");
        assert!(response["result"]["instances"][0].get("activity").is_none());
        let missing = call(
            &storage,
            json!({"operation":"getInstance","instanceId":"missing"}),
        );
        assert!(missing.get("error").is_none(), "{missing}");
        assert_eq!(missing["result"]["instances"], json!([]));
    }

    #[test]
    fn workflow_rpc_rejects_malformed_single_board_requests() {
        let directory = tempfile::tempdir().unwrap();
        let storage = StorageService::open(&directory.path().join("board-params.sqlite")).unwrap();
        for params in [
            json!({"operation":"getInstance"}),
            json!({"operation":"getInstance","instanceId":42}),
            json!({"operation":"getInstance","instanceId":""}),
            json!({"operation":"getInstance","instanceId":"board","includeActivity":"false"}),
            json!({"operation":"getInstance","instanceId":"board","includeActivity":null}),
            json!({"operation":"getInstance","instanceId":"board","extra":true}),
        ] {
            let response = call(&storage, params.clone());
            assert_eq!(response["error"]["code"], -32602, "{params}: {response}");
        }
    }

    #[test]
    fn organization_enable_rpc_reports_model_details_from_execution_availability() {
        use mycopilot_core::storage::models::{ModelConfigRecord, ModelSettingsRecord};
        use mycopilot_core::{ProviderProfileConfig, ProviderProtocolDialect};

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("member-models.sqlite");
        let storage = StorageService::open(&path).unwrap();
        let settings = |enabled: bool, token: &str| ModelSettingsRecord {
            api_url: "https://provider.example/v1/chat/completions".into(),
            api_token: token.into(),
            search_mode: "disabled".into(),
            tavily_api_key: String::new(),
            models: vec![ModelConfigRecord {
                id: "configured-model".into(),
                provider_model_id: "provider-model".into(),
                display_name: "Named model".into(),
                api_url_override: None,
                api_token_override: None,
                supports_image: false,
                context_window_tokens: Some(64_000),
                provider_profile_config: ProviderProfileConfig::generic_for_dialect(
                    ProviderProtocolDialect::OpenAiChatCompletions,
                ),
                input_price: "0".into(),
                cached_input_price: String::new(),
                output_price: "0".into(),
                enabled,
            }],
        };
        storage
            .save_model_settings(settings(true, "owned-test-token"))
            .unwrap();
        let nodes = ["a", "b"].map(|id| {
            json!({
                "kind":"agent","id":id,"name":format!("Member {id}"),"x":0,"y":0,
                "permissionMode":"default","modelConfigId":"configured-model",
                "receives":"Task","task":"Work","delivers":"Result"
            })
        });
        let definition = json!({
            "schemaVersion":1,"id":"instance","name":"Instance","description":"","background":"",
            "nodes":nodes,
            "viewport":{"x":0,"y":0,"zoom":1}
        });
        let created = call(
            &storage,
            json!({
                "operation":"saveInstance","id":"instance","definition":definition,"name":"Instance",
                "color":"#4A82E8","bindings":[],"expectedRevision":0
            }),
        );
        assert_eq!(created["result"]["instances"][0]["enabled"], true);
        let disabled = call(
            &storage,
            json!({
                "operation":"setInstanceEnabled","id":"instance","enabled":false,"expectedRevision":1
            }),
        );
        assert_eq!(disabled["result"]["instances"][0]["revision"], 2);
        let connection = rusqlite::Connection::open(&path).unwrap();
        let snapshot = || {
            connection.query_row(
                "SELECT enabled,revision,updated_at,last_request_json FROM workflow_instances WHERE instance_id='instance'",
                [],
                |row| Ok((row.get::<_, bool>(0)?, row.get::<_, u64>(1)?, row.get::<_, i64>(2)?, row.get::<_, String>(3)?)),
            ).unwrap()
        };
        let before = snapshot();
        let enable = json!({"operation":"setInstanceEnabled","id":"instance","enabled":true,"expectedRevision":2});
        // Exercise both disabled configuration and enabled configuration with missing
        // credentials through the real service, not a SQL-only model availability check.
        for configured in [settings(false, "owned-test-token"), settings(true, "")] {
            storage.save_model_settings(configured).unwrap();
            let response = call(&storage, enable.clone());
            assert_eq!(
                response,
                json!({
                    "jsonrpc":"2.0","id":1,"error":{
                        "code":-32602,"message":"organization_member_models_unavailable",
                        "data":{"code":"organization_member_models_unavailable","members":[
                            {"nodeId":"a","nodeName":"Member a","modelConfigId":"configured-model","modelDisplayName":"Named model"},
                            {"nodeId":"b","nodeName":"Member b","modelConfigId":"configured-model","modelDisplayName":"Named model"}
                        ]}
                    }
                })
            );
            assert_eq!(snapshot(), before);
        }
        let mut deleted = settings(true, "owned-test-token");
        deleted.models.clear();
        storage.save_model_settings(deleted).unwrap();
        let response = call(&storage, enable.clone());
        let members = response["error"]["data"]["members"].as_array().unwrap();
        assert_eq!(members.len(), 2);
        assert!(members
            .iter()
            .all(|member| member["modelDisplayName"].is_null()));
        assert_eq!(snapshot(), before);

        storage
            .save_model_settings(settings(true, "owned-test-token"))
            .unwrap();
        let enabled = call(&storage, enable);
        assert_eq!(enabled["result"]["instances"][0]["enabled"], true);
        assert_eq!(enabled["result"]["instances"][0]["revision"], 3);
    }

    #[test]
    fn workflow_rpc_saves_drafts_and_rejects_bad_parameters_and_conflicting_revisions() {
        let directory = tempfile::tempdir().unwrap();
        let storage = StorageService::open(&directory.path().join("workflow.sqlite")).unwrap();
        let definition = json!({
            "schemaVersion": 1, "id": "draft", "name": "Draft", "description": "", "background": "",
            "nodes": [], "viewport": {"x":80,"y":220,"zoom":1}
        });
        let saved = call(
            &storage,
            json!({"operation":"save", "definition":definition,"expectedRevision":0}),
        );
        assert_eq!(saved["result"]["records"][0]["revision"], 1);
        assert_eq!(saved["result"]["records"][0]["enabled"], false);
        assert_eq!(
            saved["result"]["records"][0]["definition"]["viewport"]["x"].as_f64(),
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
    fn workflow_rpc_preserves_free_members_and_rejects_removed_policies() {
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
        assert_eq!(record["definition"]["nodes"].as_array().unwrap().len(), 3);
        assert!(record["definition"].get("flows").is_none());
        assert_eq!(record["issues"].as_array().unwrap().len(), 3);
        assert!(record["issues"]
            .as_array()
            .unwrap()
            .iter()
            .all(|issue| issue["code"] == "node_model"));
        let mut invalid = definition.clone();
        invalid["nodes"][2]["busyPolicy"] = json!("queue");
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
        let definition = json!({"schemaVersion":1,"id":"template","name":"Original","description":"","background":"","nodes":[],"viewport":{"x":0,"y":0,"zoom":1}});
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
        #[serde(default)]
        summary_only: bool,
    },
}

pub(crate) fn handle_workflow_runtime_request(
    service: &AgentService,
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
            summary_only,
        } => service.workflow_runtime_snapshot(&instance_id, after_sequence, summary_only),
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
