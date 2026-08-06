use super::*;
use mycopilot_core::command::CommandSessionManager;
use mycopilot_core::{AgentCommandPermission, AgentCommandSafetyPolicy, AgentCommandSessionStatus};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

async fn read_json_request(stream: &mut TcpStream) -> Value {
    let mut request = Vec::new();
    let mut buffer = [0_u8; 4_096];
    let mut body_start = None;
    let mut expected_length = None;
    loop {
        let read = stream.read(&mut buffer).await.unwrap();
        assert!(read > 0, "connection closed before request completed");
        request.extend_from_slice(&buffer[..read]);
        if body_start.is_none() {
            if let Some(index) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                let start = index + 4;
                let headers = String::from_utf8_lossy(&request[..index]);
                let content_length = headers
                    .lines()
                    .find_map(|line| {
                        line.split_once(':').and_then(|(name, value)| {
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().unwrap())
                        })
                    })
                    .unwrap();
                body_start = Some(start);
                expected_length = Some(start + content_length);
            }
        }
        if expected_length.is_some_and(|length| request.len() >= length) {
            break;
        }
    }
    serde_json::from_slice(&request[body_start.unwrap()..expected_length.unwrap()]).unwrap()
}

async fn write_run_command_stream(stream: &mut TcpStream) {
    stream
        .write_all(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
        )
        .await
        .unwrap();
    let tool_frame = json!({
        "choices": [{
            "delta": {
                "role": "assistant",
                "content": "Starting the long-lived command.",
                "tool_calls": [{
                    "index": 0,
                    "id": "provider-managed-command",
                    "type": "function",
                    "function": {
                        "name": "run_command",
                        "arguments": serde_json::to_string(&json!({
                            "command": "sleep 0.8",
                            "reason": "exercise managed command handoff"
                        })).unwrap()
                    }
                }]
            },
            "finish_reason": null
        }]
    });
    let finish_frame = json!({
        "choices": [{ "delta": {}, "finish_reason": "tool_calls" }]
    });
    stream
        .write_all(
            format!("data: {tool_frame}\n\ndata: {finish_frame}\n\ndata: [DONE]\n\n").as_bytes(),
        )
        .await
        .unwrap();
}

async fn write_text_stream(stream: &mut TcpStream, content: &str) {
    stream
        .write_all(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
        )
        .await
        .unwrap();
    let content_frame = json!({
        "choices": [{
            "delta": { "role": "assistant", "content": content },
            "finish_reason": null
        }]
    });
    let finish_frame = json!({
        "choices": [{ "delta": {}, "finish_reason": "stop" }]
    });
    stream
        .write_all(
            format!("data: {content_frame}\n\ndata: {finish_frame}\n\ndata: [DONE]\n\n").as_bytes(),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn running_command_handoff_continues_loop_consumes_guidance_and_never_auto_wakes_model() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut first, _) = listener.accept().await.unwrap();
        let _first_request = read_json_request(&mut first).await;
        write_run_command_stream(&mut first).await;
        drop(first);

        let (mut second, _) = listener.accept().await.unwrap();
        let second_request = read_json_request(&mut second).await;
        write_text_stream(&mut second, "The command is running; I can continue.").await;
        drop(second);

        let third_request_seen =
            tokio::time::timeout(Duration::from_millis(1_200), listener.accept())
                .await
                .is_ok();
        (second_request, third_request_seen)
    });

    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    storage
        .save_project(ProjectRecord {
            id: "project-managed-loop".to_string(),
            name: "Managed loop".to_string(),
            path: Some(fixture.path().to_string_lossy().into_owned()),
            created_at: 1,
            pinned_at: None,
        })
        .unwrap();
    let mut settings = test_model_settings();
    settings.api_url = format!("http://{address}/v1/chat/completions");
    storage.save_model_settings(settings).unwrap();

    let mut service = AgentService::new(Arc::clone(&storage));
    service.command_sessions = AgentCommandSessionRegistry::with_manager(
        Arc::clone(&storage),
        CommandSessionManager::default(),
        Duration::from_millis(200),
    );
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let permissions = AgentPermissions {
        write: AgentWritePermission::WorkspaceOnly,
        command: AgentCommandPermission::AutoApprove,
        command_safety: AgentCommandSafetyPolicy::FullAccess,
        ..AgentPermissions::default()
    };
    let turn = service
        .start_conversation_turn(
            AgentConversationTurnInput {
                conversation_id: Some("conversation-managed-loop".to_string()),
                project_id: Some("project-managed-loop".to_string()),
                model_id: "model-1".to_string(),
                context_window_indicator_enabled: true,
                content: "Start a long command and then continue.".to_string(),
                attachments: Vec::new(),
                skills: Vec::new(),
                title: None,
                user_message_id: Some("user-managed-loop".to_string()),
                assistant_message_id: Some("assistant-managed-loop".to_string()),
                max_tokens: None,
                temperature: None,
                prompt_preferences: None,
                permissions,
            },
            notifications.clone(),
        )
        .unwrap();

    let session_id = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let notification = receiver.recv().await.unwrap();
            if notification["params"]["type"] == "command_started" {
                break notification["params"]["sessionId"]
                    .as_str()
                    .unwrap()
                    .to_string();
            }
        }
    })
    .await
    .unwrap();

    let guidance = service
        .steer_run(
            AgentSteerRunInput {
                conversation_id: turn.conversation_id.clone(),
                expected_run_id: turn.run_id.clone(),
                client_message_id: "client-managed-command-guidance".to_string(),
                content: "Please use the newer constraint.".to_string(),
                attachments: Vec::new(),
            },
            notifications,
        )
        .unwrap();
    assert_eq!(guidance.status, AgentSteerRunResultStatus::Queued);

    let mut saw_done = false;
    let mut saw_exit = false;
    tokio::time::timeout(Duration::from_secs(5), async {
        while !(saw_done && saw_exit) {
            let notification = receiver.recv().await.unwrap();
            match notification["params"]["type"].as_str() {
                Some("done") => {
                    assert_eq!(notification["params"]["status"], "completed");
                    saw_done = true;
                }
                Some("command_exited") => {
                    assert_eq!(notification["params"]["sessionId"], session_id);
                    assert_eq!(notification["params"]["exitCode"], 0);
                    saw_exit = true;
                }
                _ => {}
            }
        }
    })
    .await
    .unwrap();

    let (second_request, third_request_seen) = server.await.unwrap();
    assert!(
        !third_request_seen,
        "background command exit must not create an automatic model request"
    );
    let messages = second_request["messages"].as_array().unwrap();
    let tool_message = messages
        .iter()
        .find(|message| message["role"] == "tool")
        .expect("running receipt must be sent back through the ordinary Agent loop");
    let receipt: Value = serde_json::from_str(tool_message["content"].as_str().unwrap()).unwrap();
    assert_eq!(receipt["status"], "running");
    assert_eq!(receipt["sessionId"], session_id);
    assert!(messages.iter().any(|message| {
        message["role"] == "user" && message["content"] == "Please use the newer constraint."
    }));

    let record = storage
        .load_agent_command_session(&turn.conversation_id, &session_id)
        .unwrap()
        .unwrap();
    assert_eq!(record.snapshot.status, AgentCommandSessionStatus::Exited);
    assert_eq!(record.snapshot.exit_code, Some(0));
    let conversation = storage
        .load_conversation(&turn.conversation_id)
        .unwrap()
        .unwrap();
    assert_eq!(conversation.messages.len(), 2);
}
