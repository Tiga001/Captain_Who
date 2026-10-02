//! End-to-end workflow delivery through the real Host, independent root Turn and local provider.
use super::*;
use mycopilot_core::workflow_execution::{InputStatus, MailStatus};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc::unbounded_channel;

mod awareness;

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

fn workflow_fixture(storage: &StorageService) -> (String, String) {
    let agent = |id: &str| json!({"kind":"agent","id":id,"name":id,"x":0,"y":0,"permissionMode":"default","modelConfigId":"model-1","receives":"Receive artifacts","task":"Review quality","delivers":"Review report"});
    let definition = json!({"schemaVersion":1,"id":"template","name":"Template","description":"","background":"Workflow shared background 38276","nodes":[agent("a"),agent("b")],"viewport":{"x":0,"y":0,"zoom":1}});
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
    let (source, target) = workflow_fixture(&storage);
    let (captured, mut requests) = unbounded_channel();
    let provider_storage = storage.clone();
    let provider = tokio::spawn(async move {
        let mut sent = false;
        let mut completed = false;
        loop {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = request_body(&mut stream).await;
            let raw = request.to_string();
            captured.send(request).unwrap();
            if raw.contains("Human trigger 79318") && !sent {
                sent = true;
                respond(&mut stream,json!({"role":"assistant","tool_calls":[{"index":0,"id":"call-workflow-79318","type":"function","function":{"name":"workflow_send","arguments":"{\"messages\":[{\"targetNodeId\":\"b\",\"message\":\"Workflow artifact 51892\"}]}"}}]}),"tool_calls").await;
            } else if raw.contains("Workflow artifact 51892")
                && !raw.contains("Human trigger 79318")
                && !completed
            {
                completed = true;
                let runtime = provider_storage
                    .workflow_execution_runtime("instance")
                    .unwrap();
                let message_id = &runtime.inputs[0].messages[0].id;
                respond(&mut stream,json!({"role":"assistant","tool_calls":[{"index":0,"id":"complete-startup","type":"function","function":{"name":"workflow_complete","arguments":json!({"messageIds":[message_id]}).to_string()}}]}),"tool_calls").await;
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
    let (notifications, mut events) = crate::transport::outbound_channel();
    let scheduler = service
        .start_workflow_delivery_scheduler_with_interval(
            notifications.clone(),
            Duration::from_secs(60),
        )
        .unwrap();
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
            if snapshot.inputs.first().is_some_and(|input| {
                matches!(input.status, InputStatus::Applied | InputStatus::Completed)
            }) && samples.len() >= 3
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
    assert!(matches!(
        input.status,
        InputStatus::Applied | InputStatus::Completed
    ));
    assert!(samples
        .iter()
        .any(|sample| sample.to_string().contains("workflow_send")));
    for tool in [
        "workflow_get_state",
        "workflow_get_mailbox",
        "workflow_accept",
        "workflow_complete",
        "workflow_recall",
    ] {
        assert!(samples.iter().any(|sample| sample["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|definition| definition["function"]["name"] == tool)));
    }
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
    scheduler.shutdown().await.unwrap();
    provider.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn workflow_execution_accept_claims_pending_mail_at_safe_boundary_in_same_turn() {
    use mycopilot_core::human_interaction::{
        HumanInteractionAnswer, HumanInteractionListInput, HumanInteractionSubmitInput,
    };
    let directory = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&directory.path().join("accept.sqlite")).unwrap());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut settings = test_model_settings();
    settings.api_url = format!(
        "http://{}/v1/chat/completions",
        listener.local_addr().unwrap()
    );
    storage.save_model_settings(settings).unwrap();
    let (source, target) = workflow_fixture(&storage);
    let (captured, mut requests) = unbounded_channel();
    let provider = tokio::spawn(async move {
        let mut target_stage = 0;
        let mut sent = false;
        loop {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = request_body(&mut stream).await;
            let raw = request.to_string();
            captured.send(request.clone()).unwrap();
            let call = if raw.contains("Target waiting 81762") {
                target_stage += 1;
                match target_stage {
                    1 => Some((
                        "ask-target",
                        "request_user_input",
                        json!({"questions":[{"title":"Proceed?"}]}),
                    )),
                    2 => {
                        assert!(
                            !raw.contains("Accepted payload 42931"),
                            "pending mail must never auto-inject into an active turn"
                        );
                        assert!(
                            raw.contains("newMessageCount"),
                            "next sampling must observe a compact arrival hint"
                        );
                        Some((
                            "inspect-pending",
                            "workflow_get_mailbox",
                            json!({"direction":"inbox"}),
                        ))
                    }
                    3 => {
                        let mail = awareness::tool_result(&request, "workflow_get_mailbox");
                        assert_eq!(mail["messages"][0]["content"], "Accepted payload 42931");
                        assert_eq!(mail["messages"][0]["status"], "pending");
                        Some((
                            "accept-pending",
                            "workflow_accept",
                            json!({"messageIds":[mail["messages"][0]["messageId"]]}),
                        ))
                    }
                    4 => {
                        let receipt = awareness::tool_result(&request, "workflow_accept");
                        assert_eq!(receipt["messages"][0]["success"], true);
                        assert!(
                            receipt["messages"][0].get("content").is_none(),
                            "the receipt must not duplicate the trusted delivery body"
                        );
                        Some((
                            "complete-accepted",
                            "workflow_complete",
                            json!({"messageIds":[receipt["messages"][0]["messageId"]]}),
                        ))
                    }
                    5 => {
                        let receipt = awareness::tool_result(&request, "workflow_complete");
                        assert_eq!(receipt["messages"][0]["success"], true);
                        assert_eq!(receipt["messages"][0]["status"], "processed");
                        None
                    }
                    _ => panic!("recipient unexpectedly started another turn"),
                }
            } else if raw.contains("Source trigger 86213") && !sent {
                sent = true;
                Some((
                    "send-to-waiting",
                    "workflow_send",
                    json!({"messages":[{"targetNodeId":"b","message":"Accepted payload 42931"}]}),
                ))
            } else {
                None
            };
            if let Some((id, tool, args)) = call {
                respond(&mut stream,json!({"role":"assistant","tool_calls":[{"index":0,"id":id,"type":"function","function":{"name":tool,"arguments":args.to_string()}}]}),"tool_calls").await;
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
    let (notifications, mut events) = crate::transport::outbound_channel();
    let scheduler = service
        .start_workflow_delivery_scheduler_with_interval(
            notifications.clone(),
            Duration::from_secs(60),
        )
        .unwrap();
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
    service.wake_workflow_deliveries();
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
    assert_eq!(after.inputs[0].status, InputStatus::Completed);
    assert_eq!(
        after.inputs[0].run_id.as_deref(),
        Some(target_turn.run_id.as_str())
    );
    let mut samples = Vec::new();
    while let Ok(request) = requests.try_recv() {
        samples.push(request);
    }
    assert!(
        samples.iter().any(|sample| {
            let raw = sample.to_string();
            raw.contains("Source trigger 86213") && raw.contains("waiting_interaction")
        }),
        "workflow awareness must include another root's pending human interaction"
    );
    let traces = storage.list_conversation_turn_traces(&target).unwrap();
    assert_eq!(traces.len(), 1);
    assert_eq!(
        traces[0]
            .items
            .iter()
            .filter(|item| matches!(item, ConversationTurnTraceItem::WorkflowDelivery { .. }))
            .count(),
        1,
        "explicit acceptance delivers exactly once"
    );
    assert_eq!(after.inputs[0].mail_status, MailStatus::Processed);
    assert_eq!(
        samples
            .iter()
            .filter(|sample| sample.to_string().contains("Target waiting 81762"))
            .count(),
        5
    );
    scheduler.shutdown().await.unwrap();
    provider.abort();
}

#[test]
fn workflow_execution_stop_fences_queued_inputs_even_after_the_worker_disappears() {
    let directory = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&directory.path().join("stop.sqlite")).unwrap());
    storage.save_model_settings(test_model_settings()).unwrap();
    let (source, target) = workflow_fixture(&storage);
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
            messages: vec![mycopilot_core::workflow_execution::SendOutput {
                target_node_id: "b".into(),
                reply_to_message_id: None,
                message: "Queued before stop".into(),
            }],
        })
        .unwrap();
    let (notifications, _events) = crate::transport::outbound_channel();
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
    service.dispatch_workflow_deliveries(notifications.clone());
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
    service.dispatch_workflow_deliveries(notifications);
    assert_eq!(
        storage
            .workflow_execution_load_input(&receipt.input_ids[0])
            .unwrap()
            .unwrap()
            .status,
        InputStatus::Pending
    );
    assert!(storage
        .workflow_execution_pending_inputs()
        .unwrap()
        .is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn workflow_execution_sources_deliver_independent_letters_without_waiting_for_peers() {
    let directory = tempdir().unwrap();
    let storage =
        Arc::new(StorageService::open(&directory.path().join("independent.sqlite")).unwrap());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut settings = test_model_settings();
    settings.api_url = format!(
        "http://{}/v1/chat/completions",
        listener.local_addr().unwrap()
    );
    storage.save_model_settings(settings).unwrap();
    let agent = |id: &str| json!({"kind":"agent","id":id,"name":id,"x":0,"y":0,"permissionMode":"default","modelConfigId":"model-1","receives":"Artifacts","task":"Review received work","delivers":"Review"});
    let definition = json!({"schemaVersion":1,"id":"template","name":"Mail template","description":"","background":"Independent correspondence","nodes":[agent("a"),agent("c"),agent("b")],"viewport":{"x":0,"y":0,"zoom":1}});
    let saved = storage
        .workflow_request(
            serde_json::from_value(
                json!({"operation":"save","definition":definition,"expectedRevision":0}),
            )
            .unwrap(),
        )
        .unwrap();
    assert!(saved.records[0].issues.is_empty());
    let saved=storage.workflow_request(serde_json::from_value(json!({"operation":"saveInstance","id":"instance","templateId":"template","name":"Mail workflow","color":"#123456","bindings":[{"nodeId":"a","conversationId":null},{"nodeId":"c","conversationId":null},{"nodeId":"b","conversationId":null}],"expectedRevision":0,"expectedTemplateRevision":1})).unwrap()).unwrap();
    let chat = |node: &str| {
        saved.instances[0]
            .bindings
            .iter()
            .find(|binding| binding.node_id == node)
            .unwrap()
            .conversation_id
            .clone()
    };
    let (a, c, b) = (chat("a"), chat("c"), chat("b"));
    let provider = tokio::spawn(async move {
        let mut sent = HashSet::new();
        loop {
            let (mut stream, _) = listener.accept().await.unwrap();
            let raw = request_body(&mut stream).await.to_string();
            let source = if raw.contains("A trigger 5412") {
                Some(("a", "Artifact A 61374"))
            } else if raw.contains("C trigger 6543") {
                Some(("c", "Artifact C 92417"))
            } else {
                None
            };
            if let Some((id, body)) = source.filter(|(id, _)| sent.insert(*id)) {
                respond(&mut stream,json!({"role":"assistant","tool_calls":[{"index":0,"id":format!("send-{id}"),"type":"function","function":{"name":"workflow_send","arguments":json!({"messages":[{"targetNodeId":"b","message":body}]}).to_string()}}]}),"tool_calls").await;
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
    let (notifications, mut events) = crate::transport::outbound_channel();
    let scheduler = service
        .start_workflow_delivery_scheduler_with_interval(
            notifications.clone(),
            Duration::from_secs(60),
        )
        .unwrap();
    for (index, (source, trigger)) in [(&a, "A trigger 5412"), (&c, "C trigger 6543")]
        .into_iter()
        .enumerate()
    {
        service
            .start_conversation_turn(root_input(source, trigger), notifications.clone())
            .unwrap();
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let event = events.recv().await.unwrap();
                let ready = storage.workflow_execution_runtime("instance").unwrap();
                if event["params"]["type"] == "done"
                    && ready.inputs.len() == index + 1
                    && ready
                        .inputs
                        .iter()
                        .all(|input| input.mail_status == MailStatus::Processed)
                {
                    break;
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(
            storage.list_conversation_turn_traces(&b).unwrap().len(),
            index + 1,
            "a recipient starts with the available letter; it never waits for another sender"
        );
    }
    let runtime = storage.workflow_execution_runtime("instance").unwrap();
    assert_eq!(runtime.inputs.len(), 2);
    assert!(runtime.inputs.iter().all(|input| input.messages.len() == 1));
    assert_ne!(runtime.inputs[0].run_id, runtime.inputs[1].run_id);
    let chat = storage.load_conversation(&b).unwrap().unwrap();
    let bubbles = chat
        .messages
        .iter()
        .filter(|message| message.role == "user")
        .collect::<Vec<_>>();
    assert_eq!(bubbles.len(), 2);
    assert!(
        bubbles[0].content.contains("Artifact A 61374")
            && !bubbles[0].content.contains("Artifact C 92417")
    );
    assert!(
        bubbles[1].content.contains("Artifact C 92417")
            && !bubbles[1].content.contains("Artifact A 61374")
    );
    scheduler.shutdown().await.unwrap();
    provider.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn workflow_scheduler_wakes_queued_recipient_after_busy_turn_finishes_without_polling() {
    let directory = tempdir().unwrap();
    let storage =
        Arc::new(StorageService::open(&directory.path().join("busy-recovery.sqlite")).unwrap());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut settings = test_model_settings();
    settings.api_url = format!(
        "http://{}/v1/chat/completions",
        listener.local_addr().unwrap()
    );
    storage.save_model_settings(settings).unwrap();
    let (source, target) = workflow_fixture(&storage);
    let (captured, mut requests) = unbounded_channel();
    let (finish_busy, busy_finished) = tokio::sync::oneshot::channel();
    let provider = tokio::spawn(async move {
        let mut busy_finished = Some(busy_finished);
        let mut source_sent = false;
        let mut workers = tokio::task::JoinSet::new();
        loop {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = request_body(&mut stream).await;
            let raw = request.to_string();
            captured.send(raw.clone()).unwrap();
            if raw.contains("Busy target 92317") && busy_finished.is_some() {
                let busy_finished = busy_finished.take().unwrap();
                workers.spawn(async move {
                    busy_finished.await.unwrap();
                    respond(
                        &mut stream,
                        json!({"role":"assistant","content":"Busy work complete."}),
                        "stop",
                    )
                    .await;
                });
            } else if raw.contains("Queue trigger 76234") && !source_sent {
                source_sent = true;
                respond(&mut stream, json!({"role":"assistant","tool_calls":[{"index":0,"id":"queue-send","type":"function","function":{"name":"workflow_send","arguments":"{\"messages\":[{\"targetNodeId\":\"b\",\"message\":\"Queued artifact 92134\"}]}"}}]}), "tool_calls").await;
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
    let (notifications, mut events) = crate::transport::outbound_channel();
    let scheduler = service
        .start_workflow_delivery_scheduler_with_interval(
            notifications.clone(),
            Duration::from_secs(60),
        )
        .unwrap();
    service
        .start_conversation_turn(
            root_input(&target, "Busy target 92317"),
            notifications.clone(),
        )
        .unwrap();
    let first = tokio::time::timeout(Duration::from_secs(5), requests.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(first.contains("Busy target 92317"));
    let source_turn = service
        .start_conversation_turn(
            root_input(&source, "Queue trigger 76234"),
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
    let queued = storage.workflow_execution_runtime("instance").unwrap();
    assert_eq!(queued.inputs.len(), 1);
    assert_eq!(queued.inputs[0].status, InputStatus::Pending);
    assert_eq!(
        storage
            .list_conversation_turn_traces(&target)
            .unwrap()
            .len(),
        1
    );
    finish_busy.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let event = events.recv().await.unwrap();
            if event["params"]["type"] == "done"
                && storage
                    .workflow_execution_runtime("instance")
                    .unwrap()
                    .inputs[0]
                    .status
                    == InputStatus::Completed
            {
                break;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(
        storage
            .list_conversation_turn_traces(&target)
            .unwrap()
            .len(),
        2
    );
    scheduler.shutdown().await.unwrap();
    provider.abort();
}

#[test]
fn workflow_scheduler_read_microbenchmark() {
    if std::env::var_os("MYCOPILOT_WORKFLOW_SCHEDULER_BENCH").as_deref()
        != Some(std::ffi::OsStr::new("1"))
    {
        return;
    }
    let directory = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&directory.path().join("reads.sqlite")).unwrap());
    storage.save_model_settings(test_model_settings()).unwrap();
    let (_source, _) = workflow_fixture(&storage);
    let service = AgentService::new_authorized_for_test(storage.clone());
    let (notifications, _) = crate::transport::outbound_channel();
    let read = || {
        service
            .workflow_request(mycopilot_core::workflow::Request::List)
            .unwrap();
    };
    for _ in 0..100 {
        read();
        service.dispatch_workflow_deliveries(notifications.clone());
    }
    let mut old = Vec::with_capacity(1000);
    let mut new = Vec::with_capacity(1000);
    // Alternate pair order to avoid systematically assigning warm-cache/time effects to one path.
    for index in 0..1000 {
        for inline_scan in if index % 2 == 0 {
            [true, false]
        } else {
            [false, true]
        } {
            let started = Instant::now();
            read();
            if inline_scan {
                service.dispatch_workflow_deliveries(notifications.clone());
            }
            let elapsed = started.elapsed().as_nanos() as u64;
            if inline_scan {
                old.push(elapsed);
            } else {
                new.push(elapsed);
            }
        }
    }
    old.sort_unstable();
    new.sort_unstable();
    // Replays the original handler's exact list + schedule sequence on the same current build;
    // this isolates removed work rather than claiming an end-to-end application speedup.
    let old = (old[500], old[950]);
    let new = (new[500], new[950]);
    println!("workflow list, 1000 samples, ns: old inline-scan p50={} p95={}; new read-only p50={} p95={}", old.0, old.1, new.0, new.1);
}

// Synthetic databases only. The reference freezes the pre-13 path up to the exact
// missing-model return: global 128 query, occupancy, full conversation, every draft.
fn pre13_missing_model_scan(service: &AgentService) {
    for input in service.storage.workflow_execution_pending_inputs().unwrap() {
        let target = input.conversation_id.as_deref().unwrap();
        if service.has_conversation_turn_occupancy(target).unwrap() {
            continue;
        }
        let conversation = service.storage.load_conversation(target).unwrap().unwrap();
        assert!(conversation.archived_at.is_none());
        let draft = service
            .storage
            .load_composer_drafts()
            .unwrap()
            .into_iter()
            .find(|draft| draft.scope_id == target)
            .unwrap();
        assert!(draft.model_id.or(conversation.model_id).is_none());
    }
}
fn scan_complete_workflow_population(
    service: &AgentService,
    notifications: &CoreServerNotificationSender,
) {
    for _ in 0..100 {
        service.dispatch_workflow_deliveries(notifications.clone());
        if service.workflow_retry.lock().unwrap().cursor == 0 {
            return;
        }
    }
    panic!("workflow candidate traversal did not finish");
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn workflow_waiting_population_microbenchmark() {
    if std::env::var_os("MYCOPILOT_WORKFLOW_WAITING_BENCH").is_none() {
        return;
    }
    for count in [100, 1000] {
        let directory = tempdir().unwrap();
        let path = directory.path().join("waiting.sqlite");
        let storage = Arc::new(StorageService::open(&path).unwrap());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut settings = test_model_settings();
        settings.api_url = format!(
            "http://{}/v1/chat/completions",
            listener.local_addr().unwrap()
        );
        storage.save_model_settings(settings).unwrap();
        let mut last_target = String::new();
        workflow_fixture(&storage);
        let mut db = rusqlite::Connection::open(&path).unwrap();
        for index in 0..count {
            let result = storage.workflow_request(serde_json::from_value(json!({
                "operation":"saveInstance", "id":if index == 0 { "instance".into() } else { format!("waiting-{index}") },
                "templateId":"template", "name":"Waiting", "color":format!("#{:06x}", index + 1),
                "bindings":[{"nodeId":"a","conversationId":null},{"nodeId":"b","conversationId":null}],
                "expectedRevision":if index == 0 { 1 } else { 0 },"expectedTemplateRevision":1
            })).unwrap()).unwrap();
            let target = &result.instances[0]
                .bindings
                .iter()
                .find(|b| b.node_id == "b")
                .unwrap()
                .conversation_id;
            last_target = target.clone();
            let mut input =
                synthetic_workflow_input(&storage, target, &format!("waiting-input-{index}"));
            input.content = "synthetic waiting input".into();
            let tx = db.transaction().unwrap();
            tx.execute(
                "UPDATE composer_drafts SET model_id=NULL WHERE scope_id=?1",
                [target],
            )
            .unwrap();
            tx.execute(
                "UPDATE conversations SET model_id=NULL WHERE id=?1",
                [target],
            )
            .unwrap();
            for message in 0..40 {
                tx.execute("INSERT INTO messages(id,conversation_id,role,content,created_at,position) VALUES(?1,?2,'user',?3,1,?4)",rusqlite::params![format!("history-{index}-{message}"),target,"x".repeat(256),message]).unwrap();
            }
            insert_workflow_test_input(&tx, &input);
            tx.commit().unwrap();
        }

        let service = AgentService::new_authorized_for_test(storage.clone());
        let (notifications, mut events) = crate::transport::outbound_channel();
        let started = Instant::now();
        scan_complete_workflow_population(&service, &notifications);
        let initial_us = started.elapsed().as_micros();
        assert_eq!(service.workflow_retry.lock().unwrap().attempts, count);
        let mut old = Vec::new();
        let mut new = Vec::new();
        for index in 0..20 {
            for legacy in if index % 2 == 0 {
                [true, false]
            } else {
                [false, true]
            } {
                let started = Instant::now();
                if legacy {
                    pre13_missing_model_scan(&service);
                } else {
                    scan_complete_workflow_population(&service, &notifications);
                }
                if legacy {
                    old.push(started.elapsed().as_micros());
                } else {
                    new.push(started.elapsed().as_micros());
                }
            }
        }
        old.sort_unstable();
        new.sort_unstable();
        let mut draft = storage.load_composer_draft(&last_target).unwrap().unwrap();
        draft.model_id = Some("model-1".into());
        draft.updated_at += 1;
        storage.save_composer_draft(draft).unwrap();
        db.execute(
            "UPDATE conversations SET model_id='model-1' WHERE id=?1",
            [&last_target],
        )
        .unwrap();
        let restored_at = Instant::now();
        service.workflow_readiness_changed(Some(&last_target));
        scan_complete_workflow_population(&service, &notifications);
        let (mut stream, _) = tokio::time::timeout(Duration::from_secs(5), listener.accept())
            .await
            .unwrap()
            .unwrap();
        let request = request_body(&mut stream).await;
        assert!(request.to_string().contains("synthetic waiting input"));
        let recovery_us = restored_at.elapsed().as_micros();
        respond(
            &mut stream,
            json!({"role":"assistant","content":"recovered"}),
            "stop",
        )
        .await;
        drop(stream);
        tokio::time::timeout(Duration::from_secs(5), async {
            while let Some(event) = events.recv().await {
                if event["params"]["type"] == "done" {
                    break;
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(
            storage
                .list_conversation_turn_traces(&last_target)
                .unwrap()
                .len(),
            1
        );
        println!("workflow-waiting count={count} history=40x256B samples=20 old_first128_p50_us={} old_first128_p95_us={} new_full_population_initial_us={initial_us} new_full_population_p50_us={} new_full_population_p95_us={} repaired_last_owner_to_provider_us={recovery_us}",old[10],old[19],new[10],new[19]);
    }
}

fn synthetic_workflow_input(
    storage: &StorageService,
    target: &str,
    id: &str,
) -> mycopilot_core::workflow_execution::Input {
    let snapshot = storage
        .workflow_execution_snapshot(target)
        .unwrap()
        .unwrap();
    let source = snapshot
        .members
        .iter()
        .find(|member| member.node_id != snapshot.node_id && member.conversation_id.is_some())
        .unwrap();
    let message = mycopilot_core::workflow_execution::SourceMessage {
        id: format!("source-{id}"),
        instance_id: snapshot.instance_id.clone(),
        workflow_name: snapshot.name.clone(),
        source_node_id: source.node_id.clone(),
        source_node_name: source.node_name.clone(),
        source_conversation_id: source.conversation_id.clone().unwrap(),
        source_conversation_title: source.node_name.clone(),
        target_node_id: snapshot.node_id.clone(),
        target_node_name: snapshot.node_name.clone(),
        target_conversation_id: Some(target.into()),
        target_conversation_title: Some(snapshot.node_name.clone()),
        reply_to_message_id: None,
        content: "Authoritative workflow delivery 61391".into(),
        created_at: 1,
    };
    mycopilot_core::workflow_execution::Input {
        id: id.into(),
        instance_id: snapshot.instance_id,
        node_id: snapshot.node_id,
        conversation_id: Some(target.into()),
        execution_version: snapshot.execution_version,
        content: "Authoritative workflow delivery 61391".into(),
        messages: vec![message],
        mail_status: MailStatus::Pending,
        status: InputStatus::Pending,
        run_id: None,
        delivery_id: None,
        created_at: 1,
        error: None,
    }
}
fn insert_workflow_test_input(
    db: &rusqlite::Connection,
    input: &mycopilot_core::workflow_execution::Input,
) {
    let mut input = input.clone();
    input.messages[0].id = format!("source-{}", input.id);
    input.messages[0].target_conversation_id = input.conversation_id.clone();
    let message = &input.messages[0];
    db.execute("INSERT INTO workflow_mail_inputs(input_id,instance_id,execution_version,node_id,conversation_id,input_json,status,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,'pending',1,1)",rusqlite::params![input.id,input.instance_id,input.execution_version,input.node_id,input.conversation_id,serde_json::to_string(&input).unwrap()]).unwrap();
    db.execute("INSERT INTO workflow_mail_messages(message_id,instance_id,execution_version,node_id,recipient_conversation_id,mail_status,message_json,input_id,created_at) VALUES(?1,?2,?3,?4,?5,'pending',?6,?7,1)",rusqlite::params![message.id,input.instance_id,input.execution_version,input.node_id,input.conversation_id,serde_json::to_string(message).unwrap(),input.id]).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn workflow_config_repair_wakes_before_fallback_and_uses_current_model_once() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("repair.sqlite");
    let storage = Arc::new(StorageService::open(&path).unwrap());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut settings = test_model_settings();
    settings.api_url = format!(
        "http://{}/v1/chat/completions",
        listener.local_addr().unwrap()
    );
    let mut second = settings.models[0].clone();
    second.id = "model-2".into();
    second.provider_model_id = "repaired-model".into();
    second.display_name = "Repaired".into();
    settings.models.push(second);
    storage.save_model_settings(settings).unwrap();
    let (_, target) = workflow_fixture(&storage);
    let db = rusqlite::Connection::open(&path).unwrap();
    insert_workflow_test_input(
        &db,
        &synthetic_workflow_input(&storage, &target, "repair-input"),
    );
    db.execute(
        "UPDATE composer_drafts SET model_id=NULL WHERE scope_id=?1",
        [&target],
    )
    .unwrap();
    db.execute(
        "UPDATE conversations SET model_id=NULL WHERE id=?1",
        [&target],
    )
    .unwrap();
    let service = AgentService::new_authorized_for_test(storage.clone());
    let (notifications, mut events) = crate::transport::outbound_channel();
    let scheduler = service
        .start_workflow_delivery_scheduler_with_interval(notifications, Duration::from_secs(60))
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if service.workflow_retry.lock().unwrap().attempts > 0 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    // Let the first check finish so this verifies early invalidation of an installed cooldown.
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert_eq!(
        storage
            .workflow_execution_load_input("repair-input")
            .unwrap()
            .unwrap()
            .status,
        InputStatus::Pending
    );
    let mut draft = storage.load_composer_draft(&target).unwrap().unwrap();
    draft.model_id = Some("model-2".into());
    draft.updated_at += 1;
    storage.save_composer_draft(draft).unwrap();
    let repaired_at = Instant::now();
    service.workflow_readiness_changed(Some(&target));
    let (mut stream, _) = tokio::time::timeout(Duration::from_secs(3), listener.accept())
        .await
        .unwrap_or_else(|error| {
            panic!(
                "{error}; retry={:?}; preflight={:?}",
                service.workflow_retry.lock().unwrap(),
                service.preflight_provider_transition(AgentProviderTransitionPreflightInput {
                    conversation_id: target.clone(),
                    target_model_id: "model-2".into()
                })
            )
        })
        .unwrap();
    let request = request_body(&mut stream).await;
    assert_eq!(request["model"], "repaired-model");
    assert!(request
        .to_string()
        .contains("Authoritative workflow delivery 61391"));
    respond(
        &mut stream,
        json!({"role":"assistant","content":"Done"}),
        "stop",
    )
    .await;
    drop(stream);
    tokio::time::timeout(Duration::from_secs(3), async {
        while let Some(event) = events.recv().await {
            if event["params"]["type"] == "done" {
                break;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(
        storage
            .workflow_execution_load_input("repair-input")
            .unwrap()
            .unwrap()
            .status,
        InputStatus::Completed
    );
    assert_eq!(
        storage
            .load_conversation_meta(&target)
            .unwrap()
            .unwrap()
            .model_id
            .as_deref(),
        Some("model-2")
    );
    assert_eq!(
        storage
            .list_conversation_turn_traces(&target)
            .unwrap()
            .len(),
        1
    );
    assert!(repaired_at.elapsed() < Duration::from_secs(5));
    scheduler.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn workflow_scheduler_reaches_ready_owner_after_thousand_ineligible_heads() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("fair.sqlite");
    let storage = Arc::new(StorageService::open(&path).unwrap());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut settings = test_model_settings();
    settings.api_url = format!(
        "http://{}/v1/chat/completions",
        listener.local_addr().unwrap()
    );
    storage.save_model_settings(settings).unwrap();
    let (_, target) = workflow_fixture(&storage);
    let db = rusqlite::Connection::open(&path).unwrap();
    let input = synthetic_workflow_input(&storage, &target, "fair-ready");
    for index in 0..1000 {
        let mut other = input.clone();
        other.id = format!("ineligible-{index}");
        other.conversation_id = Some(format!("removed-{index}"));
        insert_workflow_test_input(&db, &other);
    }
    insert_workflow_test_input(&db, &input);
    let service = AgentService::new_authorized_for_test(storage.clone());
    let (notifications, mut events) = crate::transport::outbound_channel();
    let scheduler = service
        .start_workflow_delivery_scheduler_with_interval(notifications, Duration::from_secs(60))
        .unwrap();
    let (mut stream, _) = tokio::time::timeout(Duration::from_secs(5), listener.accept())
        .await
        .unwrap_or_else(|error| {
            panic!(
                "{error}; preflight={:?}",
                service.preflight_provider_transition(AgentProviderTransitionPreflightInput {
                    conversation_id: target.clone(),
                    target_model_id: "model-1".into()
                })
            )
        })
        .unwrap();
    assert!(request_body(&mut stream)
        .await
        .to_string()
        .contains("Authoritative workflow delivery 61391"));
    respond(
        &mut stream,
        json!({"role":"assistant","content":"Done"}),
        "stop",
    )
    .await;
    drop(stream);
    tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(event) = events.recv().await {
            if event["params"]["type"] == "done" {
                break;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(
        storage
            .workflow_execution_load_input(&input.id)
            .unwrap()
            .unwrap()
            .status,
        InputStatus::Completed
    );
    assert!(service.workflow_retry.lock().unwrap().attempts >= 1001);
    scheduler.shutdown().await.unwrap();
}

#[test]
fn workflow_capacity_release_retries_waiting_owner_without_clearing_configuration() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("capacity.sqlite");
    let storage = Arc::new(StorageService::open(&path).unwrap());
    storage.save_model_settings(test_model_settings()).unwrap();
    let (_, target) = workflow_fixture(&storage);
    let db = rusqlite::Connection::open(&path).unwrap();
    insert_workflow_test_input(
        &db,
        &synthetic_workflow_input(&storage, &target, "capacity-input"),
    );
    let service = AgentService::try_new_deferred_startup_reconciliation_with_agent_limit(
        storage.clone(),
        None,
        1,
    )
    .unwrap();
    service.grant_execution_access_for_test();
    let permit = service.turn_concurrency_gate().try_acquire().unwrap();
    let remaining_clone = permit.clone();
    service
        .register_turn_concurrency_permit("other-owner", permit)
        .unwrap();
    let (notifications, _events) = crate::transport::outbound_channel();
    service.dispatch_workflow_deliveries(notifications.clone());
    let candidate = storage
        .workflow_execution_pending_candidates(0, 128)
        .unwrap()
        .remove(0);
    let mut config = candidate.clone();
    config.id = "configuration-wait".into();
    config.conversation_id = Some("different".into());
    let old_generation;
    {
        let mut retries = service.workflow_retry.lock().unwrap();
        old_generation = retries.generation;
        assert!(!retries.is_due(&candidate, Instant::now()));
        retries.defer(
            &config,
            crate::application::agent::workflow_retry::RetryReason::Configuration,
            old_generation,
            Instant::now(),
        );
    }
    service.release_turn_concurrency_permit("other-owner");
    {
        let mut retries = service.workflow_retry.lock().unwrap();
        assert!(retries.is_due(&candidate, Instant::now()));
        assert!(!retries.is_due(&config, Instant::now()));
        retries.defer(
            &candidate,
            crate::application::agent::workflow_retry::RetryReason::Capacity,
            old_generation,
            Instant::now(),
        );
        assert!(
            retries.is_due(&candidate, Instant::now()),
            "release during a check wins over the old capacity observation"
        );
    }
    // A remaining Dispatcher lease may keep the slot full despite the early hint. Never start
    // on the hint alone; retry at the original one-second bound after the final clone drops.
    service.dispatch_workflow_deliveries(notifications);
    assert_eq!(
        storage
            .workflow_execution_load_input(&candidate.id)
            .unwrap()
            .unwrap()
            .status,
        InputStatus::Pending
    );
    drop(remaining_clone);
    assert!(service
        .workflow_retry
        .lock()
        .unwrap()
        .is_due(&candidate, Instant::now() + Duration::from_secs(1)));
}
