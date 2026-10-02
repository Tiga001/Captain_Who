use super::*;
use mycopilot_core::workflow_execution::SendRequest;

pub(super) fn tool_result(sample: &Value, tool: &str) -> Value {
    let messages = sample["messages"].as_array().unwrap();
    let call = messages
        .iter()
        .rev()
        .filter_map(|message| message["tool_calls"].as_array())
        .flatten()
        .find(|call| call["function"]["name"] == tool)
        .unwrap_or_else(|| panic!("missing call for {tool}"));
    let call_id = call["id"].as_str().unwrap();
    let content = messages
        .iter()
        .find(|message| message["role"] == "tool" && message["tool_call_id"] == call_id)
        .unwrap_or_else(|| panic!("missing result for {tool} / {call_id}"))["content"]
        .as_str()
        .unwrap();
    serde_json::from_str(content).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn organization_configuration_query_exposes_only_executable_choices_and_actual_caller_grants()
{
    let directory = tempdir().unwrap();
    let storage =
        Arc::new(StorageService::open(&directory.path().join("configuration.sqlite")).unwrap());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut settings = test_model_settings();
    settings.api_url = format!(
        "http://{}/v1/chat/completions",
        listener.local_addr().unwrap()
    );
    let mut other = settings.models[0].clone();
    other.id = "other-model".into();
    other.display_name = "Other model".into();
    settings.models.push(other.clone());
    other.id = "disabled-model".into();
    other.display_name = "Disabled model".into();
    other.enabled = false;
    settings.models.push(other);
    storage.save_model_settings(settings).unwrap();
    let mut preferences = storage.load_ui_preferences().unwrap();
    preferences.full_permission_enabled = true;
    preferences.custom_permission_enabled = true;
    preferences.custom_permissions = mycopilot_core::AgentPermissions {
        read: mycopilot_core::AgentReadPermission::All,
        write: mycopilot_core::AgentWritePermission::All,
        ..mycopilot_core::AgentPermissions::default()
    };
    storage.save_ui_preferences(preferences).unwrap();
    let (source, target) = workflow_fixture(&storage);
    let promote = |role: &str| {
        let response = storage
            .workflow_request(mycopilot_core::workflow::Request::Manage(
                mycopilot_core::workflow_management::Request::ListInstances {},
            ))
            .unwrap();
        let instance = &response.instances[0];
        let mut definition = serde_json::to_value(&instance.definition).unwrap();
        definition["nodes"][0]["rank"] = json!(90);
        definition["nodes"][0]["managementRole"] = json!(role);
        storage.workflow_request(serde_json::from_value(json!({"operation":"saveInstance","id":instance.id,"name":instance.name,"color":instance.color,"definition":definition,"bindings":instance.bindings,"expectedRevision":instance.revision})).unwrap()).unwrap();
    };
    promote("organization_admin");
    let mut target_draft = storage.load_composer_draft(&target).unwrap().unwrap();
    target_draft.model_id = Some("other-model".into());
    target_draft.updated_at += 1;
    storage.save_composer_draft(target_draft).unwrap();
    assert_eq!(
        storage
            .load_composer_draft(&target)
            .unwrap()
            .unwrap()
            .model_id
            .as_deref(),
        Some("other-model")
    );
    let (captured, mut requests) = unbounded_channel();
    let provider_storage = storage.clone();
    let provider_source = source.clone();
    let provider = tokio::spawn(async move {
        for sample in 0..3 {
            let (mut stream, _) = listener.accept().await.unwrap();
            captured.send(request_body(&mut stream).await).unwrap();
            if sample == 0 {
                let mut draft = provider_storage
                    .load_composer_draft(&provider_source)
                    .unwrap()
                    .unwrap();
                draft.model_id = Some("other-model".into());
                draft.permission_mode = "full".into();
                draft.updated_at += 1;
                provider_storage.save_composer_draft(draft).unwrap();
                let saved = provider_storage
                    .load_composer_draft(&provider_source)
                    .unwrap()
                    .unwrap();
                assert_eq!(saved.model_id.as_deref(), Some("other-model"));
                assert_eq!(saved.permission_mode, "full");
            } else if sample == 1 {
                let response = provider_storage
                    .workflow_request(mycopilot_core::workflow::Request::Manage(
                        mycopilot_core::workflow_management::Request::ListInstances {},
                    ))
                    .unwrap();
                let instance = &response.instances[0];
                let mut definition = serde_json::to_value(&instance.definition).unwrap();
                definition["nodes"][0]["managementRole"] = json!("member");
                provider_storage.workflow_request(serde_json::from_value(json!({"operation":"saveInstance","id":instance.id,"name":instance.name,"color":instance.color,"definition":definition,"bindings":instance.bindings,"expectedRevision":instance.revision})).unwrap()).unwrap();
            }
            if sample < 2 {
                respond(&mut stream, json!({"role":"assistant","tool_calls":[{"index":0,"id":format!("configuration-{sample}"),"type":"function","function":{"name":"organization_get_state","arguments":json!({"view":"configuration","member":"人事负责人","reason":"Check the member's model before editing"}).to_string()}}]}), "tool_calls").await;
            } else {
                respond(
                    &mut stream,
                    json!({"role":"assistant","content":"Configuration reviewed."}),
                    "stop",
                )
                .await;
            }
        }
    });
    let service = AgentService::new_authorized_for_test(storage);
    let (notifications, mut events) = crate::transport::outbound_channel();
    let mut input = root_input(&source, "Check the organization's model choices");
    input.permissions.write = mycopilot_core::AgentWritePermission::WorkspaceOnly;
    service
        .start_conversation_turn(input, notifications)
        .unwrap();
    let samples = tokio::time::timeout(Duration::from_secs(30), async {
        let mut samples = Vec::new();
        for _ in 0..3 {
            samples.push(requests.recv().await.unwrap());
        }
        while let Some(event) = events.recv().await {
            if event["params"]["type"] == "done" {
                break;
            }
        }
        samples
    })
    .await
    .unwrap();
    provider.await.unwrap();
    assert!(!samples[0]["messages"]
        .to_string()
        .contains("availableModels"));
    let result = tool_result(&samples[1], "organization_get_state");
    let configuration = &result["configuration"];
    assert_eq!(
        configuration["members"][0]["memberDefaults"]["model"],
        "Model 1"
    );
    assert_eq!(
        configuration["members"][0]["nextTurn"]["model"],
        "Other model"
    );
    assert_eq!(
        configuration["availableModels"].as_array().unwrap().len(),
        2
    );
    assert!(!configuration.to_string().contains("disabled-model"));
    assert!(configuration["availableModels"]
        .as_array()
        .unwrap()
        .iter()
        .all(Value::is_string));
    assert_eq!(configuration["callerCurrentRun"]["model"], "Model 1");
    assert_eq!(
        configuration["callerCurrentRun"]["allowedPermissionModes"],
        json!(["default"])
    );
    let denied = tool_result(&samples[2], "organization_get_state");
    assert!(denied.to_string().contains("administrator"));
    assert!(denied.get("configuration").is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn workflow_awareness_tools_query_real_members_and_own_outbox_without_delivery() {
    let directory = tempdir().unwrap();
    let storage =
        Arc::new(StorageService::open(&directory.path().join("awareness.sqlite")).unwrap());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut settings = test_model_settings();
    settings.api_url = format!(
        "http://{}/v1/chat/completions",
        listener.local_addr().unwrap()
    );
    storage.save_model_settings(settings).unwrap();
    let (source, target) = workflow_fixture(&storage);
    storage
        .upsert_chat_messages(
            &source,
            vec![ChatMessageRecord {
                human_interaction_response: None,
                id: "seed-assistant".into(),
                role: "assistant".into(),
                content: String::new(),
                created_at: 1,
                status: Some("sent".into()),
                attachments: vec![],
                folder_references_json: None,
                agent_run_json: None,
                ui_state_json: None,
            }],
            0,
        )
        .unwrap();
    let mut seed_trace = mycopilot_core::ConversationTraceSnapshot::default().in_progress_trace(
        "seed-run",
        &source,
        "seed-assistant",
    );
    storage
        .append_in_progress_conversation_turn_trace(&seed_trace, 1, 1)
        .unwrap();
    let snapshot = storage
        .workflow_execution_bind_run(&source, "seed-run")
        .unwrap()
        .unwrap();
    let receipt = storage
        .workflow_execution_send(&SendRequest {
            model_input: None,
            recipient_versions: Default::default(),
            conversation_id: source.clone(),
            source_run_id: "seed-run".into(),
            tool_call_id: "seed-call".into(),
            execution_version: snapshot.execution_version,
            messages: vec![mycopilot_core::workflow_execution::SendOutput {
                target_node_id: "b".into(),
                reply_to_message_id: None,
                message: "Own sent payload 72310".into(),
            }],
        })
        .unwrap();
    seed_trace.terminal_status = ConversationTurnTraceTerminalStatus::Completed;
    storage
        .replace_conversation_turn_trace(&seed_trace, 1, 2)
        .unwrap();
    storage
        .workflow_execution_pause_conversation(&target)
        .unwrap();
    let message_id = receipt.messages[0].id.clone();
    let (captured, mut requests) = unbounded_channel();
    let provider = tokio::spawn(async move {
        let calls = [
            (
                "inspect-state",
                "organization_get_state",
                json!({"view":"runtime","reason":"Check current member activity"}),
            ),
            (
                "inspect-members",
                "organization_get_state",
                json!({"view":"members","search":"Review","reason":"Find a reviewer by responsibilities"}),
            ),
            (
                "inspect-node",
                "organization_get_state",
                json!({"view":"runtime","member":"人事负责人","includeMail":true,"reason":"Inspect stopped receiver details"}),
            ),
            (
                "inspect-outbox",
                "organization_get_mailbox",
                json!({"direction":"outbox","messageId":message_id}),
            ),
            (
                "inspect-inbox",
                "organization_get_mailbox",
                json!({"direction":"inbox"}),
            ),
        ];
        for index in 0..=calls.len() {
            let (mut stream, _) = listener.accept().await.unwrap();
            captured.send(request_body(&mut stream).await).unwrap();
            if let Some((id, tool, args)) = calls.get(index) {
                respond(&mut stream, json!({"role":"assistant","tool_calls":[{
                    "index":0,"id":id,"type":"function","function":{"name":tool,"arguments":args.to_string()}
                }]}), "tool_calls").await;
            } else {
                respond(
                    &mut stream,
                    json!({"role":"assistant","content":"Inspection complete."}),
                    "stop",
                )
                .await;
            }
        }
    });
    let service = AgentService::new_authorized_for_test(storage.clone());
    let (notifications, mut events) = crate::transport::outbound_channel();
    let turn = service
        .start_conversation_turn(root_input(&source, "Inspect workflow 62930"), notifications)
        .unwrap();
    tokio::time::timeout(Duration::from_secs(20), async {
        while let Some(event) = events.recv().await {
            if event["params"]["type"] == "done" && event["params"]["runId"] == turn.run_id {
                return;
            }
        }
        panic!("notification channel closed before inspection finished");
    })
    .await
    .unwrap();
    provider.await.unwrap();
    let mut samples = Vec::new();
    while let Ok(sample) = requests.try_recv() {
        samples.push(sample);
    }
    assert_eq!(samples.len(), 6);
    for name in [
        "organization_send",
        "organization_get_state",
        "organization_get_mailbox",
        "organization_accept",
        "organization_complete",
        "organization_recall",
    ] {
        assert!(samples[0]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| tool["function"]["name"] == name));
    }
    assert!(samples[0]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .all(|tool| !tool["function"]["name"]
            .as_str()
            .unwrap()
            .starts_with("workflow_")));
    let state = tool_result(&samples[1], "organization_get_state");
    assert!(state.get("members").is_none());
    assert!(state.get("topology").is_none());
    let nodes = state["runtime"]["members"].as_array().unwrap();
    let current = nodes.iter().find(|node| node["member"] == "Boss").unwrap();
    assert!(state["runtime"].get("summaryPolicy").is_none());
    assert_eq!(nodes.len(), 2);
    assert_eq!(current["state"], "running");
    assert_eq!(current["waitingForApproval"], false);
    assert_eq!(current["waitingForInteraction"], false);
    assert_eq!(current["bindingAvailable"], true);
    for key in ["activeRunId", "currentInputIds"] {
        assert!(
            current.get(key).is_none(),
            "internal identity must remain hidden: {key}"
        );
    }
    let model_context = samples[0]["messages"].to_string();
    assert!(!model_context.contains(&turn.run_id));
    assert!(model_context.contains("Boss"));
    assert!(
        !model_context.contains("人事负责人"),
        "peer identities require an explicit query"
    );
    assert!(!model_context.contains("organization.awareness"));
    for key in [
        "workflowName",
        "executionVersion",
        "currentNodeId",
        "background",
    ] {
        assert!(state.get(key).is_none(), "redundant {key}");
    }
    let directory = tool_result(&samples[2], "organization_get_state");
    let members = &directory["members"];
    assert_eq!(members.as_array().unwrap().len(), 2);
    let own = members
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["member"] == "Boss")
        .unwrap();
    assert_eq!(own["task"], "Review quality");
    assert!(own.get("outputs").is_none());
    let other = members
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["member"] == "人事负责人")
        .unwrap();
    assert!(other.get("task").is_some());
    let stopped = nodes
        .iter()
        .find(|node| node["member"] == "人事负责人")
        .unwrap();
    assert_eq!(stopped["state"], "stopped");
    assert!(
        stopped.get("mail").is_none(),
        "runtime mail details are opt-in"
    );
    let focused = tool_result(&samples[3], "organization_get_state");
    assert!(focused["runtime"].get("summaryPolicy").is_none());
    assert_eq!(focused["runtime"]["members"].as_array().unwrap().len(), 1);
    assert_eq!(focused["runtime"]["members"][0]["state"], "stopped");
    assert_eq!(
        focused["runtime"]["members"][0]["mail"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(focused.get("members").is_none());
    let outbox = tool_result(&samples[4], "organization_get_mailbox");
    assert!(outbox.get("workflowName").is_none());
    assert!(outbox["organizationName"].is_string());
    let messages = outbox["messages"].as_array().unwrap();
    assert!(messages[0].get("workflowName").is_none());
    assert!(messages[0].get("organizationName").is_none());
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["messageId"], receipt.messages[0].id);
    assert_eq!(messages[0]["content"], "Own sent payload 72310");
    assert_eq!(messages[0]["from"], "Boss");
    assert_eq!(messages[0]["to"], "人事负责人");
    for field in [
        "id",
        "nodeId",
        "inputId",
        "sourceNodeId",
        "targetNodeId",
        "sourceConversationId",
        "targetConversationId",
    ] {
        assert!(
            messages[0].get(field).is_none(),
            "internal identity leaked: {field}"
        );
    }
    let inbox = tool_result(&samples[5], "organization_get_mailbox");
    assert!(inbox["messages"].as_array().unwrap().is_empty());
    let runtime = storage.workflow_execution_runtime("instance").unwrap();
    assert_eq!(runtime.inputs.len(), 1);
    assert!(runtime.paused_conversation_ids.contains(&target));
    assert!(runtime.inputs[0].run_id.is_none());
    assert!(storage
        .list_conversation_turn_traces(&target)
        .unwrap()
        .is_empty());
}
