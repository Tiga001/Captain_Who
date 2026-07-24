use super::*;
use mycopilot_core::{
    AgentInputAttachment, AgentInputAttachmentEncoding, AgentInputAttachmentKind,
};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;

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

async fn write_approval_tool_stream(stream: &mut TcpStream) {
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
                "content": "I will prepare the requested file.",
                "tool_calls": [{
                    "index": 0,
                    "id": "provider-approval-call",
                    "type": "function",
                    "function": {
                        "name": "apply_patch",
                        "arguments": serde_json::to_string(&json!({
                            "operation": "create",
                            "filePath": "guided.txt",
                            "content": "draft"
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

fn install_active_run(
    service: &AgentService,
    run_id: &str,
    conversation_id: &str,
    assistant_message_id: &str,
    image_input: bool,
) -> AgentSteerInputQueue {
    service
        .storage
        .save_conversation(ChatConversationRecord {
            id: conversation_id.to_string(),
            project_id: None,
            model_id: Some("model-1".to_string()),
            title: "steering".to_string(),
            messages: vec![ChatMessageRecord {
                id: assistant_message_id.to_string(),
                role: "assistant".to_string(),
                content: THINKING_PLACEHOLDER.to_string(),
                created_at: 1,
                status: Some("pending".to_string()),
                attachments: Vec::new(),
                agent_run_json: None,
                ui_state_json: None,
            }],
            created_at: 1,
            updated_at: 1,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
    service.register_active_run_control(
        run_id,
        conversation_id,
        assistant_message_id,
        ModelCapabilities { image_input },
    )
}

fn text_input(run_id: &str, conversation_id: &str, client_message_id: &str) -> AgentSteerRunInput {
    AgentSteerRunInput {
        conversation_id: conversation_id.to_string(),
        expected_run_id: run_id.to_string(),
        client_message_id: client_message_id.to_string(),
        content: "Please use the newer constraint.".to_string(),
        attachments: Vec::new(),
    }
}

#[test]
fn steer_run_durably_queues_once_and_reports_applied_on_retry() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let service = AgentService::new(storage);
    let queue = install_active_run(
        &service,
        "run-steer",
        "conversation-steer",
        "assistant-steer",
        false,
    );
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let input = text_input("run-steer", "conversation-steer", "client-steer");

    let queued = service
        .steer_run(input.clone(), notifications.clone())
        .unwrap();
    assert_eq!(queued.status, AgentSteerRunResultStatus::Queued);
    assert_eq!(queue.pending_len(), 1);
    let queued_event = receiver.try_recv().unwrap();
    assert_eq!(queued_event["params"]["type"], "guidance_queued");
    assert_eq!(
        queued_event["params"]["guidanceId"],
        queued.guidance_id.as_str()
    );

    let duplicate = service.steer_run(input.clone(), notifications).unwrap();
    assert_eq!(duplicate, queued);
    assert_eq!(queue.pending_len(), 1);
    assert!(receiver.try_recv().is_err());

    service
        .storage
        .mark_agent_run_guidance_applied(&queued.guidance_id, 4, now_ms())
        .unwrap();
    let (notifications, _receiver) = tokio::sync::mpsc::unbounded_channel();
    let applied = service.steer_run(input, notifications).unwrap();
    assert_eq!(applied.guidance_id, queued.guidance_id);
    assert_eq!(applied.status, AgentSteerRunResultStatus::Applied);
}

#[test]
fn approval_close_rejects_every_accepted_guidance_and_fences_new_requests() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let service = AgentService::new(storage);
    let queue = install_active_run(
        &service,
        "run-approval-steer",
        "conversation-approval-steer",
        "assistant-approval-steer",
        true,
    );
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let input = text_input(
        "run-approval-steer",
        "conversation-approval-steer",
        "client-before-approval",
    );
    let queued = service.steer_run(input, notifications.clone()).unwrap();
    assert_eq!(queued.status, AgentSteerRunResultStatus::Queued);

    service
        .close_active_run_steering(
            "run-approval-steer",
            AgentSteerRunRejectionCode::RunNotSteerable,
            "waiting for approval",
            &notifications,
        )
        .unwrap();
    assert!(!queue.is_accepting());
    assert_eq!(queue.pending_len(), 0);
    let record = service
        .storage
        .load_agent_run_guidance(&queued.guidance_id)
        .unwrap()
        .unwrap();
    assert_eq!(record.status, AgentGuidanceStatus::Rejected);

    let rejected = service
        .steer_run(
            text_input(
                "run-approval-steer",
                "conversation-approval-steer",
                "client-after-approval",
            ),
            notifications,
        )
        .unwrap();
    assert_eq!(rejected.status, AgentSteerRunResultStatus::Rejected);
    assert_eq!(
        rejected.rejection_code,
        Some(AgentSteerRunRejectionCode::RunNotSteerable)
    );

    let event_types = std::iter::from_fn(|| receiver.try_recv().ok())
        .map(|event| event["params"]["type"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    assert_eq!(
        event_types,
        vec!["guidance_queued", "guidance_rejected", "guidance_rejected"]
    );
}

#[test]
fn steer_run_rejects_wrong_conversation_and_text_beta_attachments() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let service = AgentService::new(storage);
    install_active_run(
        &service,
        "run-validation",
        "conversation-validation",
        "assistant-validation",
        false,
    );
    let (notifications, _receiver) = tokio::sync::mpsc::unbounded_channel();

    let wrong_conversation = service
        .steer_run(
            text_input("run-validation", "conversation-other", "client-wrong"),
            notifications.clone(),
        )
        .unwrap();
    assert_eq!(
        wrong_conversation.rejection_code,
        Some(AgentSteerRunRejectionCode::ConversationMismatch)
    );

    let mut with_image = text_input("run-validation", "conversation-validation", "client-image");
    with_image.attachments.push(AgentInputAttachment {
        id: "attachment-image".to_string(),
        kind: AgentInputAttachmentKind::Image,
        name: "image.png".to_string(),
        mime_type: Some("image/png".to_string()),
        size_bytes: 4,
        encoding: AgentInputAttachmentEncoding::Base64,
        data: "aW1n".to_string(),
        truncated: None,
    });
    let rejected_image = service.steer_run(with_image, notifications).unwrap();
    assert_eq!(
        rejected_image.rejection_code,
        Some(AgentSteerRunRejectionCode::ModelDoesNotSupportAttachments)
    );
}

#[test]
fn service_startup_abandons_guidance_left_queued_by_the_previous_process() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    storage
        .save_conversation(ChatConversationRecord {
            id: "conversation-startup-guidance".to_string(),
            project_id: None,
            model_id: Some("model-1".to_string()),
            title: "startup guidance".to_string(),
            messages: vec![ChatMessageRecord {
                id: "assistant-startup-guidance".to_string(),
                role: "assistant".to_string(),
                content: THINKING_PLACEHOLDER.to_string(),
                created_at: 1,
                status: Some("pending".to_string()),
                attachments: Vec::new(),
                agent_run_json: None,
                ui_state_json: None,
            }],
            created_at: 1,
            updated_at: 1,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
    storage
        .store_agent_run_guidance(AgentRunGuidanceRecord {
            guidance_id: "guidance-startup".to_string(),
            client_message_id: "client-startup".to_string(),
            run_id: "run-startup".to_string(),
            conversation_id: "conversation-startup-guidance".to_string(),
            assistant_message_id: "assistant-startup-guidance".to_string(),
            content: "Queued before restart.".to_string(),
            status: AgentGuidanceStatus::Queued,
            attachment_ids: Vec::new(),
            applied_trace_sequence: None,
            terminal_reason: None,
            created_at: 2,
            updated_at: 2,
        })
        .unwrap();

    let _service = AgentService::new(storage.clone());
    let record = storage
        .load_agent_run_guidance("guidance-startup")
        .unwrap()
        .unwrap();
    assert_eq!(record.status, AgentGuidanceStatus::Abandoned);
    assert!(record.terminal_reason.is_some());
}

#[tokio::test]
async fn conversation_turn_steering_runs_through_rpc_control_trace_and_events() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (first_request_seen_tx, first_request_seen_rx) = oneshot::channel();
    let (release_first_response_tx, release_first_response_rx) = oneshot::channel();
    let second_request = Arc::new(Mutex::new(None::<Value>));
    let second_request_for_server = Arc::clone(&second_request);
    let server = tokio::spawn(async move {
        let mut first_request_seen_tx = Some(first_request_seen_tx);
        let mut release_first_response_rx = Some(release_first_response_rx);
        for index in 0..2 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = read_json_request(&mut stream).await;
            if index == 0 {
                first_request_seen_tx.take().unwrap().send(()).unwrap();
                release_first_response_rx.take().unwrap().await.unwrap();
                write_text_stream(&mut stream, "Initial response.").await;
            } else {
                *second_request_for_server.lock().unwrap() = Some(request);
                write_text_stream(&mut stream, "Final guided response.").await;
            }
        }
    });

    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let mut settings = test_model_settings();
    settings.api_url = format!("http://{address}/v1/chat/completions");
    storage.save_model_settings(settings).unwrap();
    let service = AgentService::new(storage.clone());
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let turn = service
        .start_conversation_turn(
            AgentConversationTurnInput {
                conversation_id: Some("conversation-e2e-steer".to_string()),
                project_id: None,
                model_id: "model-1".to_string(),
                context_window_indicator_enabled: true,
                content: "Start with the original request.".to_string(),
                attachments: Vec::new(),
                skills: Vec::new(),
                title: None,
                user_message_id: Some("user-e2e-steer".to_string()),
                assistant_message_id: Some("assistant-e2e-steer".to_string()),
                max_tokens: None,
                temperature: None,
                prompt_preferences: None,
                permissions: AgentPermissions::default(),
            },
            notifications.clone(),
        )
        .unwrap();

    first_request_seen_rx.await.unwrap();
    let response = crate::server::handle_request(
        storage.as_ref(),
        &service,
        notifications,
        mycopilot_protocol_rs::JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: mycopilot_protocol_rs::JsonRpcId::Number(7),
            method: mycopilot_protocol_rs::AGENT_STEER_RUN_METHOD.to_string(),
            params: Some(
                serde_json::to_value(text_input(
                    &turn.run_id,
                    &turn.conversation_id,
                    "client-e2e-steer",
                ))
                .unwrap(),
            ),
        },
    );
    let steer = serde_json::from_value::<AgentSteerRunOutput>(response["result"].clone()).unwrap();
    assert_eq!(steer.status, AgentSteerRunResultStatus::Queued);
    release_first_response_tx.send(()).unwrap();

    let mut guidance_events = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let notification = receiver.recv().await.unwrap();
            let event_type = notification["params"]["type"].as_str().unwrap();
            if event_type.starts_with("guidance_") {
                guidance_events.push(event_type.to_string());
            }
            if event_type == "done" && notification["params"]["status"] == "completed" {
                break;
            }
        }
    })
    .await
    .unwrap();
    server.await.unwrap();

    assert_eq!(guidance_events, vec!["guidance_queued", "guidance_applied"]);
    let journal = storage
        .load_agent_run_guidance(&steer.guidance_id)
        .unwrap()
        .unwrap();
    assert_eq!(journal.status, AgentGuidanceStatus::Applied);
    assert_eq!(journal.applied_trace_sequence, Some(1));
    let trace = storage
        .get_conversation_turn_trace(&turn.assistant_message_id)
        .unwrap()
        .unwrap();
    assert!(matches!(
        trace.items.as_slice(),
        [
            ConversationTurnTraceItem::AssistantNarration { content, .. },
            ConversationTurnTraceItem::UserGuidance {
                guidance_id,
                client_message_id,
                ..
            }
        ] if content == "Initial response."
            && guidance_id == &steer.guidance_id
            && client_message_id == "client-e2e-steer"
    ));
    let request = second_request.lock().unwrap().take().unwrap();
    assert!(request["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|message| {
            message["role"] == "user" && message["content"] == "Please use the newer constraint."
        }));
}

#[tokio::test]
async fn waiting_for_approval_closes_steering_before_the_approval_event_is_published() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (request_seen_tx, request_seen_rx) = oneshot::channel();
    let (release_response_tx, release_response_rx) = oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let _ = read_json_request(&mut stream).await;
        request_seen_tx.send(()).unwrap();
        release_response_rx.await.unwrap();
        write_approval_tool_stream(&mut stream).await;
    });

    let fixture = tempdir().unwrap();
    let workspace = fixture.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let mut settings = test_model_settings();
    settings.api_url = format!("http://{address}/v1/chat/completions");
    storage.save_model_settings(settings).unwrap();
    storage
        .save_project(ProjectRecord {
            id: "project-approval-steer".to_string(),
            name: "Approval steering".to_string(),
            path: Some(workspace.to_string_lossy().into_owned()),
            created_at: 1,
            pinned_at: None,
        })
        .unwrap();
    let service = AgentService::new(storage.clone());
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let turn = service
        .start_conversation_turn(
            AgentConversationTurnInput {
                conversation_id: Some("conversation-approval-e2e".to_string()),
                project_id: Some("project-approval-steer".to_string()),
                model_id: "model-1".to_string(),
                context_window_indicator_enabled: true,
                content: "Create guided.txt.".to_string(),
                attachments: Vec::new(),
                skills: Vec::new(),
                title: None,
                user_message_id: Some("user-approval-e2e".to_string()),
                assistant_message_id: Some("assistant-approval-e2e".to_string()),
                max_tokens: None,
                temperature: None,
                prompt_preferences: None,
                permissions: AgentPermissions {
                    write: AgentWritePermission::WorkspaceOnly,
                    patch: mycopilot_core::AgentPatchPermission::RequireApproval,
                    ..Default::default()
                },
            },
            notifications.clone(),
        )
        .unwrap();

    request_seen_rx.await.unwrap();
    let queued = service
        .steer_run(
            text_input(&turn.run_id, &turn.conversation_id, "client-approval-e2e"),
            notifications,
        )
        .unwrap();
    assert_eq!(queued.status, AgentSteerRunResultStatus::Queued);
    release_response_tx.send(()).unwrap();

    let mut ordered_events = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let notification = receiver.recv().await.unwrap();
            let event_type = notification["params"]["type"].as_str().unwrap();
            if matches!(
                event_type,
                "guidance_queued" | "guidance_rejected" | "approval_required"
            ) {
                ordered_events.push(event_type.to_string());
            }
            if event_type == "done" && notification["params"]["status"] == "waiting_for_approval" {
                break;
            }
        }
    })
    .await
    .unwrap();
    server.await.unwrap();

    assert_eq!(
        ordered_events,
        vec!["guidance_queued", "guidance_rejected", "approval_required"]
    );
    let journal = storage
        .load_agent_run_guidance(&queued.guidance_id)
        .unwrap()
        .unwrap();
    assert_eq!(journal.status, AgentGuidanceStatus::Rejected);
    let (notifications, _receiver) = tokio::sync::mpsc::unbounded_channel();
    let after_approval = service
        .steer_run(
            text_input(
                &turn.run_id,
                &turn.conversation_id,
                "client-after-approval-e2e",
            ),
            notifications,
        )
        .unwrap();
    assert_eq!(
        after_approval.rejection_code,
        Some(AgentSteerRunRejectionCode::RunNotSteerable)
    );
}
