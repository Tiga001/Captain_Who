use super::*;
use mycopilot_core::{AgentCommandPermission, AgentCommandSafetyPolicy};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const MODEL_CONTENT: &str = "Synthetic model response before checkpoint failure 🐈";

async fn checkpoint_failure_preserves_usage(approval: bool) {
    let directory = tempfile::tempdir().unwrap();
    let database_path = directory.path().join("checkpoint-failure.sqlite");
    let storage = Arc::new(
        StorageService::open_with_model_credentials(
            &database_path,
            Arc::new(mycopilot_core::image_generation::InMemoryCredentialStore::default()),
        )
        .unwrap(),
    );
    let connection = rusqlite::Connection::open(&database_path).unwrap();
    connection.busy_timeout(Duration::from_secs(10)).unwrap();
    // Reject only a new checkpoint revision. Usage, terminal settlement and initial ownership
    // are independent durable writes and remain available, as with a projection-specific fault.
    connection.execute_batch("CREATE TRIGGER fail_checkpoint_revision BEFORE UPDATE OF agent_run_json ON messages
        WHEN json_extract(NEW.agent_run_json,'$.coreCheckpoint.sequence') > json_extract(OLD.agent_run_json,'$.coreCheckpoint.sequence')
        BEGIN SELECT RAISE(ABORT,'synthetic checkpoint revision failure'); END;").unwrap();
    storage
        .save_project(
            mycopilot_core::storage::models::ProjectRecord::with_primary_folder(
                "failure-project",
                "Checkpoint failure fixture",
                directory.path().to_string_lossy().into_owned(),
                1,
            ),
        )
        .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let provider = tokio::spawn(async move {
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
        let delta = if approval {
            serde_json::json!({"role":"assistant", "content":MODEL_CONTENT, "tool_calls":[{
                "index":0, "id":"approval-fixture", "type":"function", "function":{
                    "name":"run_command", "arguments":serde_json::json!({
                        "command":"printf approved > checkpoint-command.txt",
                        "reason":"Synthetic fixture must remain behind explicit approval"
                    }).to_string()
                }
            }]})
        } else {
            serde_json::json!({"role":"assistant", "content":MODEL_CONTENT})
        };
        let frame = serde_json::json!({"choices":[{"index":0,"delta":delta,"finish_reason":null}]});
        let finish = serde_json::json!({"choices":[{"index":0,"delta":{},"finish_reason":if approval {"tool_calls"} else {"stop"}}]});
        let usage = serde_json::json!({"choices":[],"usage":{"prompt_tokens":20,"completion_tokens":8,"total_tokens":28}});
        let body = format!("data: {frame}\n\ndata: {finish}\n\ndata: {usage}\n\ndata: [DONE]\n\n");
        let response = format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len());
        socket.write_all(response.as_bytes()).await.unwrap();
    });
    storage
        .save_model_settings(mycopilot_core::storage::models::ModelSettingsRecord {
            api_url: format!("http://{address}/v1/chat/completions"),
            api_token: "synthetic-nonbillable".into(),
            search_mode: "disabled".into(),
            tavily_api_key: String::new(),
            models: vec![mycopilot_core::storage::models::ModelConfigRecord {
                id: "failure-model".into(),
                provider_model_id: "failure-model".into(),
                display_name: "Failure fixture".into(),
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
    let service = AgentService::new_authorized_for_test(storage.clone());
    let (notifications, receiver) = crate::transport::outbound_channel();
    let started = service
        .start_conversation_turn(
            AgentConversationTurnInput {
                conversation_id: None,
                project_id: Some("failure-project".into()),
                model_id: "failure-model".into(),
                context_window_indicator_enabled: false,
                content: "Synthetic checkpoint failure fixture".into(),
                attachments: vec![],
                folder_references: vec![],
                skills: vec![],
                title: None,
                user_message_id: None,
                assistant_message_id: None,
                max_tokens: None,
                temperature: None,
                prompt_preferences: None,
                permissions: AgentPermissions {
                    write: mycopilot_core::AgentWritePermission::WorkspaceOnly,
                    command: AgentCommandPermission::RequireApproval,
                    command_safety: AgentCommandSafetyPolicy::FullAccess,
                    ..AgentPermissions::default()
                },
            },
            notifications,
        )
        .unwrap();
    drop(receiver);
    let trace = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            if let Some(trace) = storage
                .get_conversation_turn_trace(&started.assistant_message_id)
                .unwrap()
            {
                if trace.terminal_status != ConversationTurnTraceTerminalStatus::InProgress {
                    break trace;
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("checkpoint failure must reach durable terminal settlement");
    provider.await.unwrap();
    assert_eq!(
        trace.terminal_status,
        ConversationTurnTraceTerminalStatus::Failed
    );
    let usage = storage
        .load_agent_usage_for_owner(
            &started.run_id,
            &started.conversation_id,
            &started.assistant_message_id,
        )
        .unwrap()
        .unwrap();
    assert_eq!(usage.input_tokens, Some(20));
    assert_eq!(usage.output_tokens, Some(8));
    assert_eq!(usage.total_tokens, Some(28));
    assert_eq!(
        usage.billable_request_count, 1,
        "staging and terminal settlement must not double count"
    );
    let conversation = storage
        .load_conversation(&started.conversation_id)
        .unwrap()
        .unwrap();
    let message = conversation
        .messages
        .iter()
        .find(|message| message.id == started.assistant_message_id)
        .unwrap();
    assert_eq!(message.content, MODEL_CONTENT);
    let run: Value = serde_json::from_str(message.agent_run_json.as_deref().unwrap()).unwrap();
    assert_eq!(run["status"], "failed");
    assert_eq!(
        run["coreCheckpoint"]["sequence"], 0,
        "no failed checkpoint was ACKed"
    );
    let pending: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM agent_pending_actions WHERE run_id=?1",
            [&started.run_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        pending, 0,
        "no approval can be exposed when its checkpoint barrier fails"
    );
    assert!(
        !directory.path().join("checkpoint-command.txt").exists(),
        "the requested command must never execute"
    );
    if approval {
        assert!(trace.items.iter().any(|item|matches!(item,ConversationTurnTraceItem::ToolCall {tool,..} if tool == "run_command")),"fixture must actually reach the approval path");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn completion_checkpoint_failure_preserves_usage_trace_and_response() {
    checkpoint_failure_preserves_usage(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn approval_checkpoint_failure_preserves_usage_once_without_exposing_action() {
    checkpoint_failure_preserves_usage(true).await;
}
