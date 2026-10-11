use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const PARTIAL: &str = "child partial 🐈";

async fn request(socket: &mut tokio::net::TcpStream) -> Value {
    let mut bytes = Vec::new();
    let mut buffer = [0; 8192];
    loop {
        let count = socket.read(&mut buffer).await.unwrap();
        assert_ne!(count, 0);
        bytes.extend_from_slice(&buffer[..count]);
        if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
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

async fn response(socket: &mut tokio::net::TcpStream, delta: Value, reason: &str) {
    let body = format!(
        "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
        serde_json::json!({"choices":[{"index":0,"delta":delta,"finish_reason":null}]}),
        serde_json::json!({"choices":[{"index":0,"delta":{},"finish_reason":reason}]})
    );
    socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
}

fn storage(path: &std::path::Path) -> Arc<StorageService> {
    Arc::new(
        StorageService::open_with_model_credentials(
            path,
            Arc::new(mycopilot_core::image_generation::InMemoryCredentialStore::default()),
        )
        .unwrap(),
    )
}

async fn assert_child_checkpoint_without_observer(cancel: bool) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("child-checkpoint.sqlite");
    let storage = storage(&path);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (stop, mut stopping) = tokio::sync::oneshot::channel::<()>();
    let (finish_child, child_finished) = tokio::sync::watch::channel(false);
    let calls = Arc::new(AtomicUsize::new(0));
    let provider_calls = Arc::clone(&calls);
    let provider = tokio::spawn(async move {
        let mut tasks = tokio::task::JoinSet::new();
        loop {
            tokio::select! {
                _ = &mut stopping => break,
                accepted = listener.accept() => {
                    let (mut socket, _) = accepted.unwrap();
                    let mut child_finished = child_finished.clone();
                    let calls = Arc::clone(&provider_calls);
                    tasks.spawn(async move {
                        let input = request(&mut socket).await;
                        calls.fetch_add(1, Ordering::SeqCst);
                        if input.to_string().contains("## 子 Agent 协作身份") {
                            let first = format!("data: {}\n\n", serde_json::json!({"choices":[{"index":0,"delta":{"content":PARTIAL},"finish_reason":null}]}));
                            let last = "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\" complete\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
                            socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", first.len()+last.len()).as_bytes()).await.unwrap();
                            socket.write_all(first.as_bytes()).await.unwrap();
                            socket.flush().await.unwrap();
                            let _ = child_finished.wait_for(|finished| *finished).await;
                            // After cancellation this is deliberately a late provider frame.
                            let _ = socket.write_all(last.as_bytes()).await;
                        } else if input["messages"].as_array().unwrap().iter().any(|message| message["role"] == "tool") {
                            response(&mut socket, serde_json::json!({"content":"Root delegated the synthetic task."}), "stop").await;
                        } else {
                            response(&mut socket, serde_json::json!({"tool_calls":[{
                                "index":0,"id":"spawn-synthetic-child","type":"function","function":{
                                    "name":"spawn_agent", "arguments":serde_json::json!({"task_name":"checkpoint_child","message":"Stream a synthetic checkpoint sample.","fork_turns":"none"}).to_string()
                                }
                            }]}), "tool_calls").await;
                        }
                    });
                }
            }
        }
        tasks.abort_all();
        while let Some(result) = tasks.join_next().await {
            assert!(result.is_ok() || result.unwrap_err().is_cancelled());
        }
    });
    storage
        .save_project(
            mycopilot_core::storage::models::ProjectRecord::with_primary_folder(
                "checkpoint-project",
                "Checkpoint fixture",
                directory.path().to_string_lossy().into_owned(),
                1,
            ),
        )
        .unwrap();
    storage
        .save_model_settings(mycopilot_core::storage::models::ModelSettingsRecord {
            api_url: format!("http://{address}/v1/chat/completions"),
            api_token: "synthetic-nonbillable".into(),
            search_mode: "disabled".into(),
            tavily_api_key: String::new(),
            models: vec![mycopilot_core::storage::models::ModelConfigRecord {
                id: "checkpoint-model".into(),
                provider_model_id: "checkpoint-model".into(),
                display_name: "Checkpoint".into(),
                api_url_override: None,
                api_token_override: None,
                supports_image: false,
                context_window_tokens: Some(128_000),
                provider_profile_config: crate::test_provider_profile_config(),
                input_price: "0".into(),
                cached_input_price: String::new(),
                output_price: "0".into(),
                enabled: true,
            }],
        })
        .unwrap();
    let service = AgentService::new_authorized_for_test(Arc::clone(&storage));
    let (notifications, receiver) = crate::transport::outbound_channel();
    let started = service
        .start_conversation_turn(
            AgentConversationTurnInput {
                conversation_id: None,
                project_id: Some("checkpoint-project".into()),
                model_id: "checkpoint-model".into(),
                context_window_indicator_enabled: false,
                content: "Delegate one synthetic streaming child task.".into(),
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
            },
            notifications,
        )
        .unwrap();
    // No front end event consumer and no observer subscription/load are involved.
    drop(receiver);
    let (conversation_id, assistant_id, run_id) =
        tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                let connection = rusqlite::Connection::open(&path).unwrap();
                let identity = connection.query_row(
                    "SELECT node.conversation_id, trace.assistant_message_id, trace.run_id
                 FROM agent_nodes AS node JOIN conversation_turn_traces AS trace
                   ON trace.conversation_id=node.conversation_id
                 WHERE node.root_conversation_id=?1 AND node.parent_agent_id IS NOT NULL",
                    [&started.conversation_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                        ))
                    },
                );
                if let Ok((conversation_id, assistant_id, run_id)) = identity {
                    let conversation = storage
                        .load_conversation(&conversation_id)
                        .unwrap()
                        .unwrap();
                    if conversation.messages.iter().any(|message| {
                        message.id == assistant_id
                            && message.content == PARTIAL
                            && message.agent_run_json.as_deref().is_some_and(|raw| {
                                let run: Value = serde_json::from_str(raw).unwrap();
                                run["coreCheckpoint"]["sequence"]
                                    .as_u64()
                                    .is_some_and(|sequence| sequence > 0)
                            })
                    }) {
                        break (conversation_id, assistant_id, run_id);
                    }
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("real spawned child must checkpoint partial output without any observer");
    if cancel {
        assert!(service
            .interrupt_agent_wake_run(&run_id)
            .unwrap()
            .turn_termination_confirmed());
    } else {
        finish_child.send(true).unwrap();
    }
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let trace = storage
                .get_conversation_turn_trace(&assistant_id)
                .unwrap()
                .unwrap();
            if trace.terminal_status.is_terminal() {
                assert_eq!(
                    trace.terminal_status,
                    if cancel {
                        ConversationTurnTraceTerminalStatus::Cancelled
                    } else {
                        ConversationTurnTraceTerminalStatus::Completed
                    }
                );
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    finish_child.send(true).unwrap();
    service.shutdown_collaboration_dispatcher().await.unwrap();
    let _ = stop.send(());
    provider.await.unwrap();
    assert_eq!(
        calls.load(Ordering::SeqCst),
        3,
        "one root spawn, one root completion, one child stream"
    );
    drop(service);
    drop(storage);
    let reopened = self::storage(&path);
    let conversation = reopened
        .load_conversation(&conversation_id)
        .unwrap()
        .unwrap();
    let assistant = conversation
        .messages
        .iter()
        .find(|message| message.id == assistant_id)
        .unwrap();
    assert_eq!(
        conversation
            .messages
            .iter()
            .filter(|message| message.id == assistant_id)
            .count(),
        1
    );
    assert_eq!(
        assistant.content,
        if cancel {
            PARTIAL.to_string()
        } else {
            format!("{PARTIAL} complete")
        }
    );
    let run: Value = serde_json::from_str(assistant.agent_run_json.as_deref().unwrap()).unwrap();
    assert_eq!(
        run["status"],
        if cancel { "cancelled" } else { "completed" }
    );
    assert!(run["coreCheckpoint"]["sequence"].as_u64().unwrap() > 0);
    let unanchored = run["timeline"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| {
            item["type"] == "message"
                && item.get("streamId").is_some()
                && item.get("traceSequence").is_none()
        })
        .count();
    assert!(
        unanchored <= usize::from(cancel),
        "terminal recovery must not duplicate live stream fragments"
    );
    assert!(reopened
        .list_in_progress_conversation_turn_traces()
        .unwrap()
        .is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn spawned_child_stream_checkpoints_and_reopens_without_observer() {
    assert_child_checkpoint_without_observer(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn spawned_child_cancel_preserves_checkpoint_and_rejects_late_stream_without_observer() {
    assert_child_checkpoint_without_observer(true).await;
}
