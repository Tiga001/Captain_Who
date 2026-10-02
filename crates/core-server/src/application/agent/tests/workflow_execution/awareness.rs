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
                "workflow_get_state",
                json!({"view":"all","reason":"Check workflow progress"}),
            ),
            (
                "inspect-node",
                "workflow_get_state",
                json!({"view":"runtime","nodeId":"b","reason":"Inspect stopped receiver details"}),
            ),
            (
                "inspect-outbox",
                "workflow_get_mailbox",
                json!({"direction":"outbox","messageId":message_id}),
            ),
            (
                "inspect-inbox",
                "workflow_get_mailbox",
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
    assert_eq!(samples.len(), 5);
    for name in [
        "workflow_send",
        "workflow_get_state",
        "workflow_get_mailbox",
    ] {
        assert!(samples[0]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| tool["function"]["name"] == name));
    }
    let state = tool_result(&samples[1], "workflow_get_state");
    let members = &state["members"];
    assert_eq!(members.as_array().unwrap().len(), 2);
    assert!(state.get("topology").is_none());
    let nodes = state["runtime"]["nodes"].as_array().unwrap();
    let current = nodes.iter().find(|node| node["nodeId"] == "a").unwrap();
    assert_eq!(
        state["runtime"]["summaryPolicy"],
        "unchanged_from_world_state_omitted"
    );
    for key in ["state", "activeRunId", "currentInputIds"] {
        assert!(
            current.get(key).is_none(),
            "unchanged {key} must come from World State"
        );
    }
    assert!(samples[0]["messages"].to_string().contains(&turn.run_id));
    for key in [
        "workflowName",
        "executionVersion",
        "currentNodeId",
        "background",
    ] {
        assert!(state.get(key).is_none(), "redundant {key}");
    }
    let own = members
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["nodeId"] == "a")
        .unwrap();
    assert!(own.get("task").is_none());
    assert!(own.get("outputs").is_none());
    let other = members
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["nodeId"] == "b")
        .unwrap();
    assert!(other.get("task").is_some());
    let stopped = nodes.iter().find(|node| node["nodeId"] == "b").unwrap();
    assert!(stopped.get("state").is_none());
    assert_eq!(stopped["inputs"].as_array().unwrap().len(), 1);
    let focused = tool_result(&samples[2], "workflow_get_state");
    assert_eq!(focused["runtime"]["summaryPolicy"], "complete");
    assert_eq!(focused["runtime"]["nodes"].as_array().unwrap().len(), 1);
    assert_eq!(focused["runtime"]["nodes"][0]["state"], "stopped");
    assert!(focused.get("members").is_none());
    let outbox = tool_result(&samples[3], "workflow_get_mailbox");
    let messages = outbox["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["messageId"], receipt.messages[0].id);
    assert_eq!(messages[0]["content"], "Own sent payload 72310");
    assert_eq!(messages[0]["inputId"], receipt.input_ids[0]);
    let inbox = tool_result(&samples[4], "workflow_get_mailbox");
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
