#[derive(Clone, Copy)]
enum AfterPrecommittedWait {
    Cancel,
    Fail,
}

async fn assert_precommitted_wait_survives_terminal_settlement(outcome: AfterPrecommittedWait) {
    const ASSISTANT: &str = "assistant-precommitted-wait-terminal";
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (stop_sender, mut stop_receiver) = tokio::sync::oneshot::channel::<()>();
    let (sample_sender, mut sample_receiver) = tokio::sync::mpsc::unbounded_channel();
    let sample_release = Arc::new(tokio::sync::Notify::new());
    let server_release = Arc::clone(&sample_release);
    let server = tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = &mut stop_receiver => break,
                accepted = listener.accept() => {
                    let (mut stream, _) = accepted.unwrap();
                    let sample_sender = sample_sender.clone();
                    let release = Arc::clone(&server_release);
                    tokio::spawn(async move {
                        let request = read_provider_request(&mut stream).await;
                        if request_text(&request).contains("## 子 Agent 协作身份") {
                            write_text(&mut stream, "Child work is complete.").await;
                            return;
                        }
                        let results = tool_results(&request);
                        if results.iter().any(|result| result["targets"].is_array()) {
                            sample_sender.send(()).unwrap();
                            release.notified().await;
                            if matches!(outcome, AfterPrecommittedWait::Fail) {
                                let body = "{\"error\":{\"message\":\"controlled failure after precommitted wait\",\"type\":\"invalid_request_error\"}}";
                                stream.write_all(format!(
                                    "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                                    body.len(),
                                ).as_bytes()).await.unwrap();
                            }
                        } else if let Some(child) = results.iter()
                            .find_map(|result| result["taskName"].as_str())
                        {
                            write_tool_call(&mut stream, "wait-child", "wait_agent", json!({
                                "targets": [child], "timeout_ms": 5_000,
                            })).await;
                        } else {
                            write_tool_call(&mut stream, "spawn-child", "spawn_agent", json!({
                                "task_name": "wait_terminal_child",
                                "message": "Complete the delegated work.",
                                "fork_turns": "none",
                            })).await;
                        }
                    });
                }
            }
        }
    });

    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("wait-terminal.sqlite");
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    storage
        .save_project(ProjectRecord::with_primary_folder(
            PROJECT_ID.to_string(),
            "Precommitted wait terminal settlement".to_string(),
            fixture.path().to_string_lossy().into_owned(),
            1,
        ))
        .unwrap();
    let mut settings = test_model_settings();
    settings.api_url = format!("http://{address}/v1/chat/completions");
    storage.save_model_settings(settings).unwrap();
    let service = AgentService::try_new_deferred_startup_reconciliation_with_agent_limit(
        Arc::clone(&storage),
        None,
        2,
    )
    .unwrap();
    service.grant_execution_access_for_test();
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let turn = service
        .start_conversation_turn(
            AgentConversationTurnInput {
                conversation_id: Some(ROOT_CONVERSATION_ID.to_string()),
                project_id: Some(PROJECT_ID.to_string()),
                model_id: "model-1".to_string(),
                context_window_indicator_enabled: true,
                content: "Delegate one task and wait for its result.".to_string(),
                attachments: Vec::new(),
                skills: Vec::new(),
                title: Some("Wait terminal settlement".to_string()),
                user_message_id: Some("user-precommitted-wait-terminal".to_string()),
                assistant_message_id: Some(ASSISTANT.to_string()),
                max_tokens: None,
                temperature: None,
                prompt_preferences: None,
                permissions: AgentPermissions::default(),
            },
            notifications,
        )
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), sample_receiver.recv())
        .await
        .expect("root never sampled after the precommitted wait")
        .expect("the sample signal was dropped");

    // The real wait kernel has already consumed its receipt and committed this ToolResult.
    // No later ToolCall, narration or context material should refresh the Host snapshot before
    // the following cancellation/failure settles the Turn.
    let durable_before = storage
        .get_conversation_turn_trace(ASSISTANT)
        .unwrap()
        .unwrap();
    assert!(matches!(durable_before.items.last(), Some(
        ConversationTurnTraceItem::ToolResult { tool, success: true, observation, .. }
    ) if tool == "wait_agent" && observation["targets"].is_array()));
    let model_before = storage
        .get_conversation_model_context_log(ASSISTANT)
        .unwrap()
        .unwrap()
        .items;
    if matches!(outcome, AfterPrecommittedWait::Cancel) {
        assert!(service.cancel_run(&turn.run_id));
    } else {
        sample_release.notify_one();
    }
    let events = tokio::time::timeout(Duration::from_secs(10), async {
        let mut events = Vec::new();
        loop {
            let event = receiver.recv().await.expect("Agent event channel closed");
            assert_ne!(
                event["params"]["code"], "conversation_trace_persistence_failed",
                "terminal settlement must preserve the precommitted wait: {event:#?}"
            );
            let done = event["params"]["type"] == "done" && event["params"]["runId"] == turn.run_id;
            events.push(event);
            if done {
                return events;
            }
        }
    })
    .await
    .expect("root terminal settlement did not finish");
    let expected = match outcome {
        AfterPrecommittedWait::Cancel => ConversationTurnTraceTerminalStatus::Cancelled,
        AfterPrecommittedWait::Fail => ConversationTurnTraceTerminalStatus::Failed,
    };
    assert!(
        events
            .iter()
            .all(|event| event["params"]["code"] != "conversation_trace_persistence_failed"),
        "{events:#?}"
    );
    let terminal = storage
        .get_conversation_turn_trace(ASSISTANT)
        .unwrap()
        .unwrap();
    assert_eq!(terminal.terminal_status, expected);
    assert!(terminal.items.starts_with(&durable_before.items));
    assert_eq!(
        terminal
            .items
            .iter()
            .filter(|item| matches!(item,
                ConversationTurnTraceItem::ToolResult { tool, .. } if tool == "wait_agent"
            ))
            .count(),
        1,
        "wait must remain one authoritative successful ToolResult"
    );
    let model_after = storage
        .get_conversation_model_context_log(ASSISTANT)
        .unwrap()
        .unwrap()
        .items;
    assert!(model_after.starts_with(&model_before));
    terminal
        .validate_complete_model_context(&model_after)
        .unwrap();
    let usage = storage
        .load_agent_usage_for_owner(&turn.run_id, ROOT_CONVERSATION_ID, ASSISTANT)
        .unwrap()
        .expect("terminal Usage must be committed with the message");
    assert_eq!(usage.status.as_deref(), Some(expected.as_str()));
    let conversation = storage
        .load_conversations()
        .unwrap()
        .into_iter()
        .find(|conversation| conversation.id == ROOT_CONVERSATION_ID)
        .unwrap();
    let message = conversation
        .messages
        .iter()
        .find(|message| message.id == ASSISTANT)
        .unwrap();
    assert_ne!(message.status.as_deref(), Some("pending"));
    let agent_run: Value =
        serde_json::from_str(message.agent_run_json.as_deref().unwrap()).unwrap();
    assert_eq!(agent_run["status"], expected.as_str());
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if service.turn_concurrency_gate().active() == 0
                && !service
                    .cancellations
                    .lock()
                    .unwrap()
                    .contains_key(&turn.run_id)
                && storage
                    .list_in_progress_conversation_turn_traces()
                    .unwrap()
                    .is_empty()
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("terminal wait leaked Turn occupancy or a concurrency permit");
    assert!(service
        .shutdown_collaboration_dispatcher()
        .await
        .unwrap()
        .is_some());
    sample_release.notify_one();
    let _ = stop_sender.send(());
    server.await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn precommitted_wait_survives_cancellation_of_the_next_provider_request() {
    assert_precommitted_wait_survives_terminal_settlement(AfterPrecommittedWait::Cancel).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn precommitted_wait_survives_failure_of_the_next_provider_request() {
    assert_precommitted_wait_survives_terminal_settlement(AfterPrecommittedWait::Fail).await;
}
