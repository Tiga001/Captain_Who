use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn storage(path: &std::path::Path) -> Arc<StorageService> {
    Arc::new(
        StorageService::open_with_model_credentials(
            path,
            Arc::new(mycopilot_core::image_generation::InMemoryCredentialStore::default()),
        )
        .unwrap(),
    )
}

async fn streaming_fixture(
    path: &std::path::Path,
) -> (
    AgentService,
    AgentConversationTurnOutput,
    tokio::sync::oneshot::Sender<()>,
) {
    let storage = storage(path);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!(
        "http://{}/v1/chat/completions",
        listener.local_addr().unwrap()
    );
    let (finish, finished) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut buffer = [0; 8192];
        loop {
            let count = socket.read(&mut buffer).await.unwrap();
            assert_ne!(count, 0);
            request.extend_from_slice(&buffer[..count]);
            if let Some(end) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                let length = String::from_utf8_lossy(&request[..end])
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().unwrap())
                    })
                    .unwrap();
                if request.len() >= end + 4 + length {
                    break;
                }
            }
        }
        let first = "data: {\"id\":\"checkpoint\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"已保存 partial 🐈\"},\"finish_reason\":null}]}\n\n";
        let last = "data: {\"id\":\"checkpoint\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\" complete\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
        let headers = format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",first.len()+last.len());
        socket.write_all(headers.as_bytes()).await.unwrap();
        socket.write_all(first.as_bytes()).await.unwrap();
        socket.flush().await.unwrap();
        let _ = finished.await;
        let _ = socket.write_all(last.as_bytes()).await;
    });
    storage
        .save_model_settings(mycopilot_core::storage::models::ModelSettingsRecord {
            api_url: url,
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
    let service = AgentService::new_authorized_for_test(storage);
    let (notifications, receiver) = crate::transport::outbound_channel();
    let started = service
        .start_conversation_turn(
            AgentConversationTurnInput {
                conversation_id: None,
                project_id: None,
                model_id: "checkpoint-model".into(),
                context_window_indicator_enabled: false,
                content: "synthetic local test only".into(),
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
    // No Renderer and no notification consumer participate in checkpointing or completion.
    drop(receiver);
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let conversation = service
                .storage
                .load_conversation(&started.conversation_id)
                .unwrap()
                .unwrap();
            if conversation.messages.iter().any(|message| {
                message.id == started.assistant_message_id
                    && message.content == "已保存 partial 🐈"
                    && message.agent_run_json.as_deref().is_some_and(|raw| {
                        let run: Value = serde_json::from_str(raw).unwrap();
                        run["coreCheckpoint"]["sequence"]
                            .as_u64()
                            .is_some_and(|s| s > 0)
                            && run["toolDefinitions"]
                                .as_array()
                                .is_some_and(|a| !a.is_empty())
                    })
            }) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("Core must checkpoint streamed text without any frontend subscriber");
    (service, started, finish)
}

#[tokio::test]
async fn frontend_closed_stream_is_saved_and_terminal_reopens_exactly() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("core-only.sqlite");
    let (service, started, finish) = streaming_fixture(&path).await;
    finish.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            if service
                .storage
                .get_conversation_turn_trace(&started.assistant_message_id)
                .unwrap()
                .unwrap()
                .terminal_status
                == ConversationTurnTraceTerminalStatus::Completed
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let reopened = storage(&path);
    let conversation = reopened
        .load_conversation(&started.conversation_id)
        .unwrap()
        .unwrap();
    let message = conversation
        .messages
        .iter()
        .find(|m| m.id == started.assistant_message_id)
        .unwrap();
    assert_eq!(message.content, "已保存 partial 🐈 complete");
    let run: Value = serde_json::from_str(message.agent_run_json.as_deref().unwrap()).unwrap();
    assert_eq!(run["status"], "completed");
    assert_eq!(run["finishReason"], "stop");
    assert_eq!(run["messageStreamCheckpoints"], serde_json::json!({}));
    assert!(run["toolDefinitions"]
        .as_array()
        .is_some_and(|items| !items.is_empty()));
    assert_eq!(
        conversation
            .messages
            .iter()
            .filter(|m| m.role == "user")
            .count(),
        1
    );
}

// A self-spawned test process, deliberately killed by the parent after a durable checkpoint.
// This is not a graceful Drop/reopen approximation of a process crash.
#[tokio::test]
async fn process_crash_fixture() {
    let Ok(directory) = std::env::var("CAPTAIN_CHECKPOINT_CRASH_FIXTURE") else {
        return;
    };
    let directory = std::path::PathBuf::from(directory);
    let (_service, started, _finish) = streaming_fixture(&directory.join("crash.sqlite")).await;
    std::fs::write(directory.join("ready.json"),serde_json::to_vec(&serde_json::json!({
        "conversationId":started.conversation_id,"assistantMessageId":started.assistant_message_id,
    })).unwrap()).unwrap();
    std::future::pending::<()>().await;
}

#[test]
fn committed_stream_survives_process_kill_and_startup_reconciliation() {
    let directory = tempfile::tempdir().unwrap();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "application::agent::runtime_checkpoint_tests::process_crash_fixture",
            "--nocapture",
        ])
        .env("CAPTAIN_CHECKPOINT_CRASH_FIXTURE", directory.path())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    while !directory.path().join("ready.json").exists() && Instant::now() < deadline {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let ready = directory.path().join("ready.json").exists();
    let _ = child.kill();
    let output = child.wait_with_output().unwrap();
    assert!(
        ready,
        "child checkpoint failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!output.status.success());
    let ids: Value =
        serde_json::from_slice(&std::fs::read(directory.path().join("ready.json")).unwrap())
            .unwrap();
    let reopened = storage(&directory.path().join("crash.sqlite"));
    reopened
        .reconcile_orphaned_in_progress_conversation_turn_traces(
            &HashSet::new(),
            mycopilot_core::storage::now_ms(),
        )
        .unwrap();
    let conversation = reopened
        .load_conversation(ids["conversationId"].as_str().unwrap())
        .unwrap()
        .unwrap();
    let message = conversation
        .messages
        .iter()
        .find(|m| Some(m.id.as_str()) == ids["assistantMessageId"].as_str())
        .unwrap();
    assert_eq!(message.content, "已保存 partial 🐈");
    let run: Value = serde_json::from_str(message.agent_run_json.as_deref().unwrap()).unwrap();
    assert_eq!(run["status"], "failed");
    assert!(run["coreCheckpoint"]["sequence"].as_u64().unwrap() > 0);
    assert!(run["toolDefinitions"]
        .as_array()
        .is_some_and(|items| !items.is_empty()));
    assert_eq!(
        run["timeline"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|item| item["type"] == "message" && item["content"] == "已保存 partial 🐈")
            .count(),
        1
    );
}
