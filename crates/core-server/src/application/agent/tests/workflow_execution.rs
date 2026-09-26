//! End-to-end workflow delivery through the real Host, independent root Turn and local provider.
use super::*;
use mycopilot_core::workflow_execution::InputStatus;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc::unbounded_channel;

async fn request_body(stream: &mut tokio::net::TcpStream) -> Value {
    let mut bytes = Vec::new();
    loop {
        let mut buffer = [0; 4096];
        let size = stream.read(&mut buffer).await.unwrap();
        assert!(size > 0);
        bytes.extend_from_slice(&buffer[..size]);
        if let Some(end) = bytes.windows(4).position(|value| value == b"\r\n\r\n") {
            let length = String::from_utf8_lossy(&bytes[..end])
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().unwrap())
                })
                .unwrap();
            if bytes.len() >= end + 4 + length {
                return serde_json::from_slice(&bytes[end + 4..end + 4 + length]).unwrap();
            }
        }
    }
}

async fn respond(stream: &mut tokio::net::TcpStream, delta: Value, reason: &str) {
    let content = json!({"choices":[{"delta":delta,"finish_reason":null}]});
    let end = json!({"choices":[{"delta":{},"finish_reason":reason}]});
    stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\ndata: {content}\n\ndata: {end}\n\ndata: [DONE]\n\n").as_bytes()).await.unwrap();
}

fn workflow_fixture(storage: &StorageService, busy: &str) -> (String, String) {
    let agent = |id: &str| json!({"kind":"agent","id":id,"name":id,"x":0,"y":0,"permissionMode":"default","modelConfigId":"model-1","receives":"Receive artifacts","task":"Review quality","delivers":"Review report"});
    let flow = |id: &str, source: Option<&str>, target: &str| json!({"id":id,"name":id,"source":source.map(|v|json!({"kind":"node","nodeId":v})).unwrap_or(json!({"kind":"boundary"})),"target":{"kind":"node","nodeId":target}});
    let definition = json!({"schemaVersion":1,"id":"template","name":"Template","description":"","background":"Workflow shared background 38276","nodes":[agent("a"),agent("b"),{"kind":"inputGate","id":"in","name":"Input","x":0,"y":0,"processingMode":"individual","busyPolicy":busy}],"flows":[flow("entry",None,"a"),flow("direct",Some("a"),"in"),flow("in-bind",Some("in"),"b")],"viewport":{"x":0,"y":0,"zoom":1},"boundaryPositions":{"input":{"x":0,"y":0}}});
    let saved = storage
        .workflow_request(
            serde_json::from_value(
                json!({"operation":"save","definition":definition,"expectedRevision":0}),
            )
            .unwrap(),
        )
        .unwrap();
    assert!(
        saved.records[0].issues.is_empty(),
        "{:?}",
        saved.records[0].issues
    );
    let result=storage.workflow_request(serde_json::from_value(json!({"operation":"saveInstance","id":"instance","templateId":"template","name":"Review workflow","color":"#123456","bindings":[{"nodeId":"a","conversationId":null},{"nodeId":"b","conversationId":null}],"expectedRevision":0,"expectedTemplateRevision":1})).unwrap()).unwrap();
    let bindings = &result.instances[0].bindings;
    (
        bindings
            .iter()
            .find(|b| b.node_id == "a")
            .unwrap()
            .conversation_id
            .clone(),
        bindings
            .iter()
            .find(|b| b.node_id == "b")
            .unwrap()
            .conversation_id
            .clone(),
    )
}
fn root_input(conversation_id: &str, content: &str) -> AgentConversationTurnInput {
    AgentConversationTurnInput {
        conversation_id: Some(conversation_id.into()),
        project_id: None,
        model_id: "model-1".into(),
        context_window_indicator_enabled: false,
        content: content.into(),
        attachments: vec![],
        folder_references: vec![],
        skills: vec![],
        title: None,
        user_message_id: None,
        assistant_message_id: None,
        max_tokens: None,
        temperature: None,
        prompt_preferences: None,
        permissions: AgentPermissions::default(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn workflow_execution_tool_delivers_one_trusted_input_and_starts_independent_root() {
    let directory = tempdir().unwrap();
    let storage =
        Arc::new(StorageService::open(&directory.path().join("workflow.sqlite")).unwrap());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut settings = test_model_settings();
    settings.api_url = format!(
        "http://{}/v1/chat/completions",
        listener.local_addr().unwrap()
    );
    storage.save_model_settings(settings).unwrap();
    let (source, target) = workflow_fixture(&storage, "queue");
    let (captured, mut requests) = unbounded_channel();
    let provider = tokio::spawn(async move {
        let mut sent = false;
        loop {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = request_body(&mut stream).await;
            let raw = request.to_string();
            captured.send(request).unwrap();
            if raw.contains("Human trigger 79318") && !sent {
                sent = true;
                respond(&mut stream,json!({"role":"assistant","tool_calls":[{"index":0,"id":"call-workflow-79318","type":"function","function":{"name":"workflow_send","arguments":"{\"outputs\":[{\"flowId\":\"direct\",\"message\":\"Workflow artifact 51892\"}]}"}}]}),"tool_calls").await;
            } else {
                respond(
                    &mut stream,
                    json!({"role":"assistant","content":"Complete."}),
                    "stop",
                )
                .await;
            }
        }
    });
    let service = AgentService::new_authorized_for_test(storage.clone());
    let (notifications, mut events) = unbounded_channel();
    service
        .start_conversation_turn(
            root_input(&source, "Human trigger 79318"),
            notifications.clone(),
        )
        .unwrap();
    let mut samples = Vec::new();
    let completion = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let request = requests.recv().await.unwrap();
            samples.push(request);
            let snapshot = storage.workflow_execution_runtime("instance").unwrap();
            if snapshot
                .inputs
                .first()
                .is_some_and(|input| input.status == InputStatus::Applied)
                && samples.len() >= 3
            {
                break;
            }
        }
    })
    .await;
    if completion.is_err() {
        let mut all_events = Vec::new();
        while let Ok(event) = events.try_recv() {
            all_events.push(event);
        }
        panic!(
            "workflow source/recipient did not sample: inputs={:?}, sample_count={}, events={}",
            storage
                .workflow_execution_runtime("instance")
                .unwrap()
                .inputs
                .iter()
                .map(|input| (&input.id, &input.status, &input.error))
                .collect::<Vec<_>>(),
            samples.len(),
            serde_json::to_string(
                &all_events
                    .iter()
                    .filter(|event| event["params"]["type"] == "error")
                    .collect::<Vec<_>>()
            )
            .unwrap()
        );
    }
    let snapshot = storage.workflow_execution_runtime("instance").unwrap();
    assert_eq!(snapshot.inputs.len(), 1);
    let input = storage
        .workflow_execution_load_input(&snapshot.inputs[0].id)
        .unwrap()
        .unwrap();
    assert_eq!(input.conversation_id.as_deref(), Some(target.as_str()));
    assert_eq!(input.status, InputStatus::Applied);
    assert!(samples
        .iter()
        .any(|sample| sample.to_string().contains("workflow_send")));
    let recipient = samples
        .iter()
        .find(|sample| {
            sample.to_string().contains("Workflow artifact 51892")
                && !sample.to_string().contains("Human trigger 79318")
        })
        .expect("target model receives collaborator message");
    let raw = recipient.to_string();
    assert!(raw.contains("Workflow shared background 38276"));
    assert!(raw.contains("Review quality"));
    assert!(raw.contains("workflow"));
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let event = events.recv().await.unwrap();
            if event["params"]["type"] == "done"
                && event["params"]["runId"].as_str() == input.run_id.as_deref()
            {
                break;
            }
        }
    })
    .await
    .unwrap();
    let chat = storage.load_conversation(&target).unwrap().unwrap();
    assert_eq!(chat.messages.iter().filter(|m| m.role == "user").count(), 1);
    assert!(chat
        .messages
        .iter()
        .any(|message| message.content.contains("Workflow artifact 51892")));
    let traces = storage.list_conversation_turn_traces(&target).unwrap();
    assert!(traces.iter().flat_map(|trace|&trace.items).any(|item|matches!(item,ConversationTurnTraceItem::WorkflowDelivery {input_id,..} if input_id==&input.id)));
    let mut history =
        crate::application::agent_support::conversation_history_messages_with_model_context(
            &chat,
            &traces,
            &storage
                .list_conversation_model_context_logs(&target)
                .unwrap(),
            None,
            &[],
        )
        .unwrap();
    storage
        .project_agent_messages_for_model(&target, &mut history)
        .unwrap();
    assert!(
        !history
            .iter()
            .any(|message| message.role == "user"
                && message.content.contains("Workflow artifact 51892")),
        "UI projection must never replay as HumanText"
    );
    provider.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn workflow_execution_inject_respects_blocking_user_wait_and_uses_the_same_turn() {
    use mycopilot_core::human_interaction::{
        HumanInteractionAnswer, HumanInteractionListInput, HumanInteractionSubmitInput,
    };
    let directory = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&directory.path().join("inject.sqlite")).unwrap());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut settings = test_model_settings();
    settings.api_url = format!(
        "http://{}/v1/chat/completions",
        listener.local_addr().unwrap()
    );
    storage.save_model_settings(settings).unwrap();
    let (source, target) = workflow_fixture(&storage, "inject");
    let (captured, mut requests) = unbounded_channel();
    let provider = tokio::spawn(async move {
        let mut asked = false;
        let mut sent = false;
        loop {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = request_body(&mut stream).await;
            let raw = request.to_string();
            captured.send(request).unwrap();
            if raw.contains("Target waiting 81762") && !asked {
                asked = true;
                respond(&mut stream,json!({"role":"assistant","tool_calls":[{"index":0,"id":"ask-target","type":"function","function":{"name":"request_user_input","arguments":"{\"questions\":[{\"title\":\"Proceed?\"}]}"}}]}),"tool_calls").await;
            } else if raw.contains("Source trigger 86213") && !sent {
                sent = true;
                respond(&mut stream,json!({"role":"assistant","tool_calls":[{"index":0,"id":"send-to-waiting","type":"function","function":{"name":"workflow_send","arguments":"{\"outputs\":[{\"flowId\":\"direct\",\"message\":\"Injection payload 42931\"}]}"}}]}),"tool_calls").await;
            } else {
                respond(
                    &mut stream,
                    json!({"role":"assistant","content":"Complete."}),
                    "stop",
                )
                .await;
            }
        }
    });
    let service = AgentService::new_authorized_for_test(storage.clone());
    let (notifications, mut events) = unbounded_channel();
    let target_turn = service
        .start_conversation_turn(
            root_input(&target, "Target waiting 81762"),
            notifications.clone(),
        )
        .unwrap();
    let question = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(question) = storage
                .list_human_interaction_requests(&HumanInteractionListInput {
                    conversation_id: target.clone(),
                    cursor: None,
                    limit: 20,
                })
                .unwrap()
                .items
                .into_iter()
                .next()
            {
                break question;
            }
            let _ = events.recv().await.unwrap();
        }
    })
    .await
    .unwrap();
    let source_turn = service
        .start_conversation_turn(
            root_input(&source, "Source trigger 86213"),
            notifications.clone(),
        )
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let event = events.recv().await.unwrap();
            if event["params"]["type"] == "done" && event["params"]["runId"] == source_turn.run_id {
                break;
            }
        }
    })
    .await
    .unwrap();
    service.schedule_workflow_deliveries(notifications.clone());
    let before = storage.workflow_execution_runtime("instance").unwrap();
    assert_eq!(before.inputs.len(), 1);
    assert_eq!(before.inputs[0].status, InputStatus::Pending);
    assert_eq!(
        storage
            .list_conversation_turn_traces(&target)
            .unwrap()
            .len(),
        1,
        "a blocking interaction must not be bypassed by a second turn"
    );
    crate::application::human_interaction::HumanInteractionService::new(&storage, &service)
        .submit(
            HumanInteractionSubmitInput {
                conversation_id: target.clone(),
                request_id: question.request_id,
                expected_revision: question.revision,
                submission_id: "workflow-test-answer".into(),
                answers: vec![HumanInteractionAnswer::Text {
                    question_id: question.questions[0].id.clone(),
                    text: "Proceed".into(),
                }],
            },
            &notifications,
        )
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let event = events.recv().await.unwrap();
            if event["params"]["type"] == "done" && event["params"]["runId"] == target_turn.run_id {
                break;
            }
        }
    })
    .await
    .unwrap();
    let after = storage.workflow_execution_runtime("instance").unwrap();
    assert_eq!(after.inputs[0].status, InputStatus::Applied);
    assert_eq!(
        after.inputs[0].run_id.as_deref(),
        Some(target_turn.run_id.as_str())
    );
    let mut samples = Vec::new();
    while let Ok(request) = requests.try_recv() {
        samples.push(request);
    }
    let incoming = samples
        .iter()
        .filter(|sample| {
            sample.to_string().contains("Target waiting 81762")
                && sample.to_string().contains("Injection payload 42931")
        })
        .count();
    assert_eq!(incoming, 1);
    assert_eq!(
        storage
            .list_conversation_turn_traces(&target)
            .unwrap()
            .len(),
        1
    );
    provider.abort();
}

#[test]
fn workflow_execution_stop_fences_queued_inputs_even_after_the_worker_disappears() {
    let directory = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&directory.path().join("stop.sqlite")).unwrap());
    storage.save_model_settings(test_model_settings()).unwrap();
    let (source, target) = workflow_fixture(&storage, "queue");
    let service = AgentService::new_authorized_for_test(storage.clone());
    for (chat, run, assistant) in [
        (&source, "source-run", "source-assistant"),
        (&target, "target-run", "target-assistant"),
    ] {
        storage
            .upsert_chat_messages(
                chat,
                vec![ChatMessageRecord {
                    human_interaction_response: None,
                    id: assistant.into(),
                    role: "assistant".into(),
                    content: String::new(),
                    created_at: 1,
                    status: Some("pending".into()),
                    attachments: vec![],
                    folder_references_json: None,
                    agent_run_json: None,
                    ui_state_json: None,
                }],
                0,
            )
            .unwrap();
        storage
            .append_in_progress_conversation_turn_trace(
                &mycopilot_core::ConversationTraceSnapshot::default()
                    .in_progress_trace(run, chat, assistant),
                1,
                1,
            )
            .unwrap();
        storage.workflow_execution_bind_run(chat, run).unwrap();
    }
    let snapshot = storage
        .workflow_execution_snapshot(&source)
        .unwrap()
        .unwrap();
    let receipt = storage
        .workflow_execution_send(&mycopilot_core::workflow_execution::SendRequest {
            conversation_id: source,
            source_run_id: "source-run".into(),
            tool_call_id: "send".into(),
            execution_version: snapshot.execution_version,
            outputs: vec![mycopilot_core::workflow_execution::SendOutput {
                flow_id: "direct".into(),
                message: "Queued before stop".into(),
            }],
        })
        .unwrap();
    let (notifications, _events) = unbounded_channel();
    let token = AgentCancellationToken::new();
    service.register_cancellation("target-run", token.clone());
    service.register_active_run_control(
        "target-run",
        &target,
        "target-assistant",
        None,
        ModelCapabilities::default(),
        AgentPermissions::default(),
    );
    service.schedule_workflow_deliveries(notifications.clone());
    assert_eq!(
        storage
            .workflow_execution_load_input(&receipt.input_ids[0])
            .unwrap()
            .unwrap()
            .status,
        InputStatus::Pending
    );
    assert!(service.cancel_run_checked("target-run").unwrap());
    assert!(token.is_cancelled());
    let trace = mycopilot_core::cancelled_conversation_trace_without_items(
        "target-run",
        &target,
        "target-assistant",
    );
    storage
        .replace_conversation_turn_trace(&trace, 1, 2)
        .unwrap();
    service.unregister_cancellation("target-run");
    service.release_conversation_turn_if_current(&target, "target-run");
    service.schedule_workflow_deliveries(notifications);
    assert_eq!(
        storage
            .workflow_execution_load_input(&receipt.input_ids[0])
            .unwrap()
            .unwrap()
            .status,
        InputStatus::Paused
    );
    assert!(storage
        .workflow_execution_pending_inputs()
        .unwrap()
        .is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn workflow_execution_two_sources_form_one_batch_message_and_one_turn() {
    let directory = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&directory.path().join("batch.sqlite")).unwrap());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut settings = test_model_settings();
    settings.api_url = format!(
        "http://{}/v1/chat/completions",
        listener.local_addr().unwrap()
    );
    storage.save_model_settings(settings).unwrap();
    let agent = |id: &str| json!({"kind":"agent","id":id,"name":id,"x":0,"y":0,"permissionMode":"default","modelConfigId":"model-1","receives":"Both source artifacts","task":"Compare both","delivers":"One review"});
    let flow = |id: &str, source: Option<&str>, target: &str| json!({"id":id,"name":id,"source":source.map(|v|json!({"kind":"node","nodeId":v})).unwrap_or(json!({"kind":"boundary"})),"target":{"kind":"node","nodeId":target}});
    let definition = json!({"schemaVersion":1,"id":"template","name":"Batch template","description":"","background":"Shared batch background","nodes":[agent("a"),agent("c"),agent("b"),{"kind":"inputGate","id":"in","name":"Batch","x":0,"y":0,"processingMode":"batch","busyPolicy":"queue"}],"flows":[flow("entry-a",None,"a"),flow("entry-c",None,"c"),flow("from-a",Some("a"),"in"),flow("from-c",Some("c"),"in"),flow("in-bind",Some("in"),"b")],"viewport":{"x":0,"y":0,"zoom":1},"boundaryPositions":{"input":{"x":0,"y":0}}});
    let saved = storage
        .workflow_request(
            serde_json::from_value(
                json!({"operation":"save","definition":definition,"expectedRevision":0}),
            )
            .unwrap(),
        )
        .unwrap();
    assert!(
        saved.records[0].issues.is_empty(),
        "{:?}",
        saved.records[0].issues
    );
    let saved=storage.workflow_request(serde_json::from_value(json!({"operation":"saveInstance","id":"instance","templateId":"template","name":"Batch workflow","color":"#123456","bindings":[{"nodeId":"a","conversationId":null},{"nodeId":"c","conversationId":null},{"nodeId":"b","conversationId":null}],"expectedRevision":0,"expectedTemplateRevision":1})).unwrap()).unwrap();
    let chat = |node: &str| {
        saved.instances[0]
            .bindings
            .iter()
            .find(|binding| binding.node_id == node)
            .unwrap()
            .conversation_id
            .clone()
    };
    let a = chat("a");
    let c = chat("c");
    let b = chat("b");
    let (captured, mut requests) = unbounded_channel();
    let provider = tokio::spawn(async move {
        let mut sent_a = false;
        let mut sent_c = false;
        loop {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = request_body(&mut stream).await;
            let raw = request.to_string();
            captured.send(request).unwrap();
            let delivery = if raw.contains("A trigger 5412") && !sent_a {
                sent_a = true;
                Some(("from-a", "Artifact A 61374"))
            } else if raw.contains("C trigger 6543") && !sent_c {
                sent_c = true;
                Some(("from-c", "Artifact C 92417"))
            } else {
                None
            };
            if let Some((flow, body)) = delivery {
                respond(&mut stream,json!({"role":"assistant","tool_calls":[{"index":0,"id":format!("batch-{flow}"),"type":"function","function":{"name":"workflow_send","arguments":json!({"outputs":[{"flowId":flow,"message":body}]}).to_string()}}]}),"tool_calls").await;
            } else {
                respond(
                    &mut stream,
                    json!({"role":"assistant","content":"Complete."}),
                    "stop",
                )
                .await;
            }
        }
    });
    let service = AgentService::new_authorized_for_test(storage.clone());
    let (notifications, mut events) = unbounded_channel();
    let first = service
        .start_conversation_turn(root_input(&a, "A trigger 5412"), notifications.clone())
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let event = events.recv().await.unwrap();
            if event["params"]["type"] == "done" && event["params"]["runId"] == first.run_id {
                break;
            }
        }
    })
    .await
    .unwrap();
    assert!(storage
        .workflow_execution_runtime("instance")
        .unwrap()
        .inputs
        .is_empty());
    assert!(storage
        .list_conversation_turn_traces(&b)
        .unwrap()
        .is_empty());
    service
        .start_conversation_turn(root_input(&c, "C trigger 6543"), notifications)
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let event = events.recv().await.unwrap();
            let ready = storage.workflow_execution_runtime("instance").unwrap();
            if let Some(input) = ready.inputs.first() {
                if event["params"]["type"] == "done"
                    && event["params"]["runId"].as_str() == input.run_id.as_deref()
                {
                    break;
                }
            }
        }
    })
    .await
    .unwrap();
    let snapshot = storage.workflow_execution_runtime("instance").unwrap();
    assert_eq!(snapshot.inputs.len(), 1);
    let input = storage
        .workflow_execution_load_input(&snapshot.inputs[0].id)
        .unwrap()
        .unwrap();
    assert_eq!(input.status, InputStatus::Applied);
    assert_eq!(input.messages.len(), 2);
    assert_eq!(
        input
            .messages
            .iter()
            .map(|message| message.source_node_id.as_str())
            .collect::<HashSet<_>>(),
        HashSet::from(["a", "c"])
    );
    assert_eq!(
        input
            .content
            .matches("[Workflow collaboration message]")
            .count(),
        1
    );
    assert_eq!(
        input
            .content
            .matches("[Shared workflow background]")
            .count(),
        1
    );
    let chat = storage.load_conversation(&b).unwrap().unwrap();
    let bubbles = chat
        .messages
        .iter()
        .filter(|message| message.role == "user")
        .collect::<Vec<_>>();
    assert_eq!(bubbles.len(), 1);
    assert!(
        bubbles[0].content.contains("Artifact A 61374")
            && bubbles[0].content.contains("Artifact C 92417")
    );
    let traces = storage.list_conversation_turn_traces(&b).unwrap();
    assert_eq!(traces.len(), 1);
    assert_eq!(
        traces[0]
            .items
            .iter()
            .filter(|item| matches!(item, ConversationTurnTraceItem::WorkflowDelivery { .. }))
            .count(),
        1
    );
    let mut target_samples = Vec::new();
    while let Ok(request) = requests.try_recv() {
        let raw = request.to_string();
        if raw.contains("Artifact A 61374") && raw.contains("Artifact C 92417") {
            target_samples.push(raw);
        }
    }
    assert_eq!(target_samples.len(), 1);
    provider.abort();
}
