use super::*;
use base64::Engine;
use mycopilot_core::{
    AgentInputAttachment, AgentInputAttachmentEncoding, AgentInputAttachmentKind,
};
use std::io::Write;
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

async fn write_target_observation_tool_stream(stream: &mut TcpStream) {
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
                "content": "I will inspect the target before proposing a change.",
                "tool_calls": [{
                    "index": 0,
                    "id": "provider-observe-guided-target",
                    "type": "function",
                    "function": {
                        "name": "read_file",
                        "arguments": serde_json::to_string(&json!({
                            "path": "guided.txt"
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
                            "request": {
                                "action": "apply",
                                "operation": "create",
                                "filePath": "guided.txt",
                                "content": "draft"
                            }
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

async fn write_attachment_list_tool_stream(stream: &mut TcpStream) {
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
                "content": "",
                "tool_calls": [{
                    "index": 0,
                    "id": "provider-attachment-list-call",
                    "type": "function",
                    "function": {
                        "name": "attachments_list",
                        "arguments": "{}"
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
                content: String::new(),
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
        None,
        ModelCapabilities { image_input },
        AgentPermissions::default(),
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

fn encoded_attachment(
    id: &str,
    kind: AgentInputAttachmentKind,
    name: &str,
    mime_type: &str,
    bytes: &[u8],
) -> AgentInputAttachment {
    AgentInputAttachment {
        id: id.to_string(),
        kind,
        name: name.to_string(),
        mime_type: Some(mime_type.to_string()),
        size_bytes: bytes.len() as u64,
        encoding: AgentInputAttachmentEncoding::Base64,
        data: base64::engine::general_purpose::STANDARD.encode(bytes),
        truncated: None,
    }
}

fn minimal_ooxml(prefix: &str, text: &str) -> Vec<u8> {
    let mut output = std::io::Cursor::new(Vec::new());
    {
        let mut archive = zip::ZipWriter::new(&mut output);
        archive
            .start_file(
                format!("{prefix}/content.xml"),
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
        write!(archive, "<root>{text}</root>").unwrap();
        archive.finish().unwrap();
    }
    output.into_inner()
}

fn test_image_bytes(format: image::ImageFormat) -> Vec<u8> {
    let mut output = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgba8(1, 1)
        .write_to(&mut output, format)
        .unwrap();
    output.into_inner()
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
    let mut input = text_input(
        "run-approval-steer",
        "conversation-approval-steer",
        "client-before-approval",
    );
    input.attachments = vec![encoded_attachment(
        "attachment-before-approval",
        AgentInputAttachmentKind::File,
        "approval.txt",
        "text/plain",
        b"must not become visible",
    )];
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
    assert!(service
        .storage
        .build_attachment_library_context("conversation-approval-steer", None)
        .unwrap()
        .conversation_attachments
        .is_empty());

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
fn stale_finalizer_cannot_remove_a_new_approval_continuation_queue() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let service = AgentService::new(storage);
    let stale_queue = install_active_run(
        &service,
        "run-approval-resume",
        "conversation-approval-resume",
        "assistant-approval-resume",
        false,
    );
    let continuation_queue = service.register_active_run_control(
        "run-approval-resume",
        "conversation-approval-resume",
        "assistant-approval-resume",
        None,
        ModelCapabilities { image_input: false },
        AgentPermissions::default(),
    );
    let (notifications, _receiver) = tokio::sync::mpsc::unbounded_channel();

    service
        .unregister_active_run_control(
            "run-approval-resume",
            &stale_queue,
            AgentSteerRunRejectionCode::RunNotSteerable,
            "stale run finished",
            &notifications,
        )
        .unwrap();

    assert!(continuation_queue.is_accepting());
    let queued = service
        .steer_run(
            text_input(
                "run-approval-resume",
                "conversation-approval-resume",
                "client-after-approval-resume",
            ),
            notifications,
        )
        .unwrap();
    assert_eq!(queued.status, AgentSteerRunResultStatus::Queued);
    assert_eq!(continuation_queue.pending_len(), 1);
}

#[test]
fn steer_run_rejects_wrong_conversation_and_unsupported_model_images() {
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
fn steer_run_accepts_the_round_four_attachment_matrix_and_mixed_guidance() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let service = AgentService::new(storage.clone());
    install_active_run(
        &service,
        "run-attachment-matrix",
        "conversation-attachment-matrix",
        "assistant-attachment-matrix",
        true,
    );
    let (notifications, _receiver) = tokio::sync::mpsc::unbounded_channel();
    let cases = vec![
        encoded_attachment(
            "attachment-png",
            AgentInputAttachmentKind::Image,
            "image.png",
            "image/png",
            &test_image_bytes(image::ImageFormat::Png),
        ),
        encoded_attachment(
            "attachment-jpeg",
            AgentInputAttachmentKind::Image,
            "image.jpeg",
            "image/jpeg",
            &test_image_bytes(image::ImageFormat::Jpeg),
        ),
        encoded_attachment(
            "attachment-webp",
            AgentInputAttachmentKind::Image,
            "image.webp",
            "image/webp",
            &test_image_bytes(image::ImageFormat::WebP),
        ),
        encoded_attachment(
            "attachment-text",
            AgentInputAttachmentKind::File,
            "notes.md",
            "text/markdown",
            b"# Notes",
        ),
        encoded_attachment(
            "attachment-pdf",
            AgentInputAttachmentKind::File,
            "paper.pdf",
            "application/pdf",
            b"%PDF-1.7\n",
        ),
        encoded_attachment(
            "attachment-docx",
            AgentInputAttachmentKind::File,
            "document.docx",
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            &minimal_ooxml("word", "document"),
        ),
        encoded_attachment(
            "attachment-pptx",
            AgentInputAttachmentKind::File,
            "slides.pptx",
            "application/vnd.openxmlformats-officedocument.presentationml.presentation",
            &minimal_ooxml("ppt", "slides"),
        ),
        encoded_attachment(
            "attachment-xlsx",
            AgentInputAttachmentKind::File,
            "table.xlsx",
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            &minimal_ooxml("xl", "sheet"),
        ),
        encoded_attachment(
            "attachment-csv",
            AgentInputAttachmentKind::File,
            "table.csv",
            "text/csv",
            b"name,value\nalpha,1\n",
        ),
        encoded_attachment(
            "attachment-tsv",
            AgentInputAttachmentKind::File,
            "table.tsv",
            "text/tab-separated-values",
            b"name\tvalue\nalpha\t1\n",
        ),
    ];

    for (index, attachment) in cases.into_iter().enumerate() {
        let mut input = text_input(
            "run-attachment-matrix",
            "conversation-attachment-matrix",
            &format!("client-attachment-{index}"),
        );
        input.attachments = vec![attachment];
        let output = service.steer_run(input, notifications.clone()).unwrap();
        assert_eq!(output.status, AgentSteerRunResultStatus::Queued);
    }

    let mut mixed = text_input(
        "run-attachment-matrix",
        "conversation-attachment-matrix",
        "client-attachment-mixed",
    );
    mixed.attachments = vec![
        encoded_attachment(
            "attachment-mixed-text",
            AgentInputAttachmentKind::File,
            "mixed.txt",
            "text/plain",
            b"mixed text",
        ),
        encoded_attachment(
            "attachment-mixed-image",
            AgentInputAttachmentKind::Image,
            "mixed.png",
            "image/png",
            &test_image_bytes(image::ImageFormat::Png),
        ),
        encoded_attachment(
            "attachment-mixed-pdf",
            AgentInputAttachmentKind::File,
            "mixed.pdf",
            "application/pdf",
            b"%PDF-1.7\nmixed",
        ),
    ];
    let mixed_output = service.steer_run(mixed, notifications).unwrap();
    assert_eq!(mixed_output.status, AgentSteerRunResultStatus::Queued);
    assert_eq!(
        storage
            .load_agent_run_guidance(&mixed_output.guidance_id)
            .unwrap()
            .unwrap()
            .attachment_ids
            .len(),
        3
    );
}

#[test]
fn steer_run_rejects_invalid_attachment_payloads_limits_and_identity_reuse() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let service = AgentService::new(storage);
    install_active_run(
        &service,
        "run-attachment-validation",
        "conversation-attachment-validation",
        "assistant-attachment-validation",
        true,
    );
    let (notifications, _receiver) = tokio::sync::mpsc::unbounded_channel();

    let mut invalid_base64 = text_input(
        "run-attachment-validation",
        "conversation-attachment-validation",
        "client-invalid-base64",
    );
    invalid_base64.attachments = vec![AgentInputAttachment {
        id: "attachment-invalid-base64".to_string(),
        kind: AgentInputAttachmentKind::File,
        name: "notes.txt".to_string(),
        mime_type: Some("text/plain".to_string()),
        size_bytes: 4,
        encoding: AgentInputAttachmentEncoding::Base64,
        data: "%%%".to_string(),
        truncated: None,
    }];
    assert_eq!(
        service
            .steer_run(invalid_base64, notifications.clone())
            .unwrap()
            .rejection_code,
        Some(AgentSteerRunRejectionCode::AttachmentValidationFailed)
    );

    let mut size_mismatch = text_input(
        "run-attachment-validation",
        "conversation-attachment-validation",
        "client-size-mismatch",
    );
    let mut mismatched = encoded_attachment(
        "attachment-size-mismatch",
        AgentInputAttachmentKind::File,
        "notes.txt",
        "text/plain",
        b"hello",
    );
    mismatched.size_bytes += 1;
    size_mismatch.attachments = vec![mismatched];
    assert_eq!(
        service
            .steer_run(size_mismatch, notifications.clone())
            .unwrap()
            .rejection_code,
        Some(AgentSteerRunRejectionCode::AttachmentValidationFailed)
    );

    let mut mime_mismatch = text_input(
        "run-attachment-validation",
        "conversation-attachment-validation",
        "client-mime-mismatch",
    );
    mime_mismatch.attachments = vec![encoded_attachment(
        "attachment-mime-mismatch",
        AgentInputAttachmentKind::Image,
        "image.png",
        "image/jpeg",
        &test_image_bytes(image::ImageFormat::Png),
    )];
    assert_eq!(
        service
            .steer_run(mime_mismatch, notifications.clone())
            .unwrap()
            .rejection_code,
        Some(AgentSteerRunRejectionCode::AttachmentValidationFailed)
    );

    let mut too_many = text_input(
        "run-attachment-validation",
        "conversation-attachment-validation",
        "client-too-many",
    );
    too_many.attachments = (0..=8)
        .map(|index| {
            encoded_attachment(
                &format!("attachment-limit-{index}"),
                AgentInputAttachmentKind::File,
                &format!("file-{index}.txt"),
                "text/plain",
                b"x",
            )
        })
        .collect();
    assert_eq!(
        service
            .steer_run(too_many, notifications.clone())
            .unwrap()
            .rejection_code,
        Some(AgentSteerRunRejectionCode::AttachmentLimitExceeded)
    );

    let mut original = text_input(
        "run-attachment-validation",
        "conversation-attachment-validation",
        "client-attachment-identity",
    );
    original.attachments = vec![encoded_attachment(
        "attachment-identity",
        AgentInputAttachmentKind::File,
        "identity.txt",
        "text/plain",
        b"first",
    )];
    assert_eq!(
        service
            .steer_run(original.clone(), notifications.clone())
            .unwrap()
            .status,
        AgentSteerRunResultStatus::Queued
    );
    original.attachments[0].data = base64::engine::general_purpose::STANDARD.encode(b"other");
    assert_eq!(
        service
            .steer_run(original, notifications)
            .unwrap()
            .rejection_code,
        Some(AgentSteerRunRejectionCode::IdentityConflict)
    );
}

#[test]
fn attachment_persistence_failure_never_enters_the_runtime_queue() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let service = AgentService::new(storage.clone());
    let queue = install_active_run(
        &service,
        "run-attachment-persistence",
        "conversation-attachment-persistence",
        "assistant-attachment-persistence",
        true,
    );
    storage
        .save_input_attachments(
            "conversation-attachment-persistence",
            "assistant-attachment-persistence",
            None,
            &[encoded_attachment(
                "attachment-collision",
                AgentInputAttachmentKind::File,
                "existing.txt",
                "text/plain",
                b"existing",
            )],
            2,
        )
        .unwrap();

    let mut input = text_input(
        "run-attachment-persistence",
        "conversation-attachment-persistence",
        "client-attachment-persistence",
    );
    input.attachments = vec![encoded_attachment(
        "attachment-collision",
        AgentInputAttachmentKind::File,
        "candidate.txt",
        "text/plain",
        b"candidate",
    )];
    let (notifications, _receiver) = tokio::sync::mpsc::unbounded_channel();
    let output = service.steer_run(input, notifications).unwrap();

    assert_eq!(output.status, AgentSteerRunResultStatus::Rejected);
    assert_eq!(
        output.rejection_code,
        Some(AgentSteerRunRejectionCode::AttachmentPersistenceFailed)
    );
    assert_eq!(queue.pending_len(), 0);
    assert!(storage
        .load_agent_run_guidance_by_client_message(
            "run-attachment-persistence",
            "client-attachment-persistence",
        )
        .unwrap()
        .is_none());
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
                content: String::new(),
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
    let conversation = storage
        .load_conversation("conversation-startup-guidance")
        .unwrap()
        .unwrap();
    let run: Value =
        serde_json::from_str(conversation.messages[0].agent_run_json.as_deref().unwrap()).unwrap();
    assert_eq!(run["timeline"][0]["status"], "rejected");
    assert_eq!(run["timeline"][0]["rejectionCode"], "run_interrupted");
    assert_eq!(run["timeline"][0]["recoverable"], true);
}

#[tokio::test]
async fn conversation_turn_steering_runs_through_rpc_control_trace_and_events() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (first_delta_sent_tx, first_delta_sent_rx) = oneshot::channel();
    let (release_first_response_tx, release_first_response_rx) = oneshot::channel();
    let second_request = Arc::new(Mutex::new(None::<Value>));
    let second_request_for_server = Arc::clone(&second_request);
    let server = tokio::spawn(async move {
        let mut first_delta_sent_tx = Some(first_delta_sent_tx);
        let mut release_first_response_rx = Some(release_first_response_rx);
        for index in 0..2 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = read_json_request(&mut stream).await;
            if index == 0 {
                stream
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
                    )
                    .await
                    .unwrap();
                let content_frame = json!({
                    "choices": [{
                        "delta": { "role": "assistant", "content": "Initial response." },
                        "finish_reason": null
                    }]
                });
                stream
                    .write_all(format!("data: {content_frame}\n\n").as_bytes())
                    .await
                    .unwrap();
                stream.flush().await.unwrap();
                first_delta_sent_tx.take().unwrap().send(()).unwrap();
                release_first_response_rx.take().unwrap().await.unwrap();
                let finish_frame = json!({
                    "choices": [{ "delta": {}, "finish_reason": "stop" }]
                });
                stream
                    .write_all(format!("data: {finish_frame}\n\ndata: [DONE]\n\n").as_bytes())
                    .await
                    .unwrap();
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

    first_delta_sent_rx.await.unwrap();
    let response = crate::transport::handle_request(
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
async fn acknowledged_guidance_is_explicitly_rejected_when_network_retries_are_exhausted() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (first_request_seen_tx, first_request_seen_rx) = oneshot::channel();
    let (release_first_failure_tx, release_first_failure_rx) = oneshot::channel();
    let server = tokio::spawn(async move {
        let mut first_request_seen_tx = Some(first_request_seen_tx);
        let mut release_first_failure_rx = Some(release_first_failure_rx);
        for index in 0..3 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let _request = read_json_request(&mut stream).await;
            if index == 0 {
                first_request_seen_tx.take().unwrap().send(()).unwrap();
                release_first_failure_rx.take().unwrap().await.unwrap();
            }
            let body = b"temporary upstream failure";
            stream
                .write_all(
                    format!(
                        "HTTP/1.1 503 Service Unavailable\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
            stream.write_all(body).await.unwrap();
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
                conversation_id: Some("conversation-network-steer".to_string()),
                project_id: None,
                model_id: "model-1".to_string(),
                context_window_indicator_enabled: true,
                content: "Start a request that will fail.".to_string(),
                attachments: Vec::new(),
                skills: Vec::new(),
                title: None,
                user_message_id: Some("user-network-steer".to_string()),
                assistant_message_id: Some("assistant-network-steer".to_string()),
                max_tokens: None,
                temperature: None,
                prompt_preferences: None,
                permissions: AgentPermissions::default(),
            },
            notifications.clone(),
        )
        .unwrap();

    first_request_seen_rx.await.unwrap();
    let queued = service
        .steer_run(
            text_input(&turn.run_id, &turn.conversation_id, "client-network-steer"),
            notifications,
        )
        .unwrap();
    assert_eq!(queued.status, AgentSteerRunResultStatus::Queued);
    release_first_failure_tx.send(()).unwrap();

    let rejected_event = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let notification = receiver.recv().await.unwrap();
            if notification["params"]["type"] == "guidance_rejected" {
                break notification;
            }
        }
    })
    .await
    .unwrap();
    server.await.unwrap();

    assert_eq!(
        rejected_event["params"]["rejectionCode"],
        "run_not_steerable"
    );
    let journal = storage
        .load_agent_run_guidance(&queued.guidance_id)
        .unwrap()
        .unwrap();
    assert_eq!(journal.status, AgentGuidanceStatus::Rejected);
    assert!(journal
        .terminal_reason
        .as_deref()
        .is_some_and(|reason| reason.contains("finished")));
}

#[tokio::test]
async fn guidance_attachments_reach_model_context_and_refresh_runtime_tools_after_apply() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (first_request_seen_tx, first_request_seen_rx) = oneshot::channel();
    let (release_first_response_tx, release_first_response_rx) = oneshot::channel();
    let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let requests_for_server = Arc::clone(&requests);
    let server = tokio::spawn(async move {
        let mut first_request_seen_tx = Some(first_request_seen_tx);
        let mut release_first_response_rx = Some(release_first_response_rx);
        for index in 0..3 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = read_json_request(&mut stream).await;
            requests_for_server.lock().unwrap().push(request);
            match index {
                0 => {
                    first_request_seen_tx.take().unwrap().send(()).unwrap();
                    release_first_response_rx.take().unwrap().await.unwrap();
                    write_text_stream(&mut stream, "Initial response.").await;
                }
                1 => write_attachment_list_tool_stream(&mut stream).await,
                _ => write_text_stream(&mut stream, "Final response with attachments.").await,
            }
        }
    });

    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let mut settings = test_model_settings();
    settings.api_url = format!("http://{address}/v1/chat/completions");
    settings.models[0].supports_image = true;
    storage.save_model_settings(settings).unwrap();
    let service = AgentService::new(storage.clone());
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let turn = service
        .start_conversation_turn(
            AgentConversationTurnInput {
                conversation_id: Some("conversation-e2e-attachments".to_string()),
                project_id: None,
                model_id: "model-1".to_string(),
                context_window_indicator_enabled: true,
                content: "Start the original request.".to_string(),
                attachments: Vec::new(),
                skills: Vec::new(),
                title: None,
                user_message_id: Some("user-e2e-attachments".to_string()),
                assistant_message_id: Some("assistant-e2e-attachments".to_string()),
                max_tokens: None,
                temperature: None,
                prompt_preferences: None,
                permissions: AgentPermissions::default(),
            },
            notifications.clone(),
        )
        .unwrap();

    first_request_seen_rx.await.unwrap();
    let text_attachment = encoded_attachment(
        "attachment-e2e-text",
        AgentInputAttachmentKind::File,
        "guidance.txt",
        "text/plain",
        b"attachment text constraint",
    );
    let image_bytes = test_image_bytes(image::ImageFormat::Png);
    let image_attachment = encoded_attachment(
        "attachment-e2e-image",
        AgentInputAttachmentKind::Image,
        "guidance.png",
        "image/png",
        &image_bytes,
    );
    let mut guidance = text_input(
        &turn.run_id,
        &turn.conversation_id,
        "client-e2e-attachments",
    );
    guidance.attachments = vec![text_attachment, image_attachment];
    let queued = service.steer_run(guidance, notifications).unwrap();
    assert_eq!(queued.status, AgentSteerRunResultStatus::Queued);
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

    let requests = requests.lock().unwrap();
    let second_messages = requests[1]["messages"].as_array().unwrap();
    let guided_message = second_messages
        .iter()
        .find(|message| {
            message["role"] == "user"
                && message["content"].as_array().is_some_and(|parts| {
                    parts.iter().any(|part| {
                        part["type"] == "text"
                            && part["text"]
                                .as_str()
                                .is_some_and(|text| text.contains("attachment text constraint"))
                    })
                })
        })
        .expect("guidance user message with extracted attachment text");
    let expected_image = base64::engine::general_purpose::STANDARD.encode(&image_bytes);
    assert!(guided_message["content"]
        .as_array()
        .unwrap()
        .iter()
        .any(|part| {
            part["type"] == "image_url"
                && part["image_url"]["url"] == format!("data:image/png;base64,{expected_image}")
        }));

    let third_messages = requests[2]["messages"].as_array().unwrap();
    let attachment_tool_result = third_messages
        .iter()
        .find(|message| message["role"] == "tool")
        .expect("attachments_list result");
    let tool_content = attachment_tool_result["content"].as_str().unwrap();
    assert!(tool_content.contains("attachment-e2e-text"));
    assert!(tool_content.contains("attachment-e2e-image"));
    drop(requests);

    let journal = storage
        .load_agent_run_guidance(&queued.guidance_id)
        .unwrap()
        .unwrap();
    assert_eq!(journal.status, AgentGuidanceStatus::Applied);
    assert_eq!(journal.attachment_ids.len(), 2);
    let trace = storage
        .get_conversation_turn_trace(&turn.assistant_message_id)
        .unwrap()
        .unwrap();
    assert!(trace.items.iter().any(|item| matches!(
        item,
        ConversationTurnTraceItem::UserGuidance { attachments, .. }
            if attachments.len() == 2
    )));
    assert!(!serde_json::to_string(&trace)
        .unwrap()
        .contains(&expected_image));
    let library = storage
        .build_attachment_library_context(&turn.conversation_id, None)
        .unwrap();
    assert!(library
        .conversation_attachments
        .iter()
        .any(|attachment| attachment.id == "attachment-e2e-text"));
    assert!(library
        .conversation_attachments
        .iter()
        .any(|attachment| attachment.id == "attachment-e2e-image"));
}

#[tokio::test]
async fn waiting_for_approval_closes_steering_before_the_approval_event_is_published() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (request_seen_tx, request_seen_rx) = oneshot::channel();
    let (release_response_tx, release_response_rx) = oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut observation_stream, _) = listener.accept().await.unwrap();
        let _ = read_json_request(&mut observation_stream).await;
        write_target_observation_tool_stream(&mut observation_stream).await;
        drop(observation_stream);

        let (mut approval_stream, _) = listener.accept().await.unwrap();
        let _ = read_json_request(&mut approval_stream).await;
        request_seen_tx.send(()).unwrap();
        release_response_rx.await.unwrap();
        write_approval_tool_stream(&mut approval_stream).await;
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

#[tokio::test]
async fn approved_run_reopens_steering_and_applies_guidance_to_the_same_turn() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (approval_response_tx, approval_response_rx) = oneshot::channel();
    let (continuation_seen_tx, continuation_seen_rx) = oneshot::channel();
    let (release_continuation_tx, release_continuation_rx) = oneshot::channel();
    let (guided_request_tx, guided_request_rx) = oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut observation_stream, _) = listener.accept().await.unwrap();
        let _ = read_json_request(&mut observation_stream).await;
        write_target_observation_tool_stream(&mut observation_stream).await;
        drop(observation_stream);

        let (mut approval_stream, _) = listener.accept().await.unwrap();
        let _ = read_json_request(&mut approval_stream).await;
        write_approval_tool_stream(&mut approval_stream).await;
        drop(approval_stream);
        approval_response_tx.send(()).unwrap();

        let (mut continuation_stream, _) = listener.accept().await.unwrap();
        let _ = read_json_request(&mut continuation_stream).await;
        continuation_seen_tx.send(()).unwrap();
        release_continuation_rx.await.unwrap();
        write_text_stream(&mut continuation_stream, "Continuing after approval.").await;
        drop(continuation_stream);

        let (mut guided_stream, _) = listener.accept().await.unwrap();
        let guided_request = read_json_request(&mut guided_stream).await;
        guided_request_tx.send(guided_request).unwrap();
        write_text_stream(&mut guided_stream, "Applied the guidance.").await;
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
            id: "project-approval-resume-steer".to_string(),
            name: "Approval resume steering".to_string(),
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
                conversation_id: Some("conversation-approval-resume-e2e".to_string()),
                project_id: Some("project-approval-resume-steer".to_string()),
                model_id: "model-1".to_string(),
                context_window_indicator_enabled: true,
                content: "Create guided.txt.".to_string(),
                attachments: Vec::new(),
                skills: Vec::new(),
                title: None,
                user_message_id: Some("user-approval-resume-e2e".to_string()),
                assistant_message_id: Some("assistant-approval-resume-e2e".to_string()),
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

    approval_response_rx.await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let notification = receiver.recv().await.unwrap();
            assert_ne!(
                notification["params"]["type"], "error",
                "run failed before approval: {notification}"
            );
            if notification["params"]["type"] == "approval_required" {
                break;
            }
        }
    })
    .await
    .unwrap();
    let pending = service.list_pending_actions();
    assert_eq!(pending.len(), 1);
    let approved_tool_call_id = pending[0]
        .tool_call_id
        .clone()
        .expect("FileChange approval retains the exact Tool Call id");
    let approval = service
        .approve_action(&turn.run_id, &pending[0].action_id, notifications.clone())
        .unwrap();
    assert_eq!(approval.agent_output.status, AgentRunStatus::Running);

    continuation_seen_rx.await.unwrap();
    let queued = service
        .steer_run(
            text_input(
                &turn.run_id,
                &turn.conversation_id,
                "client-after-approval-resume-e2e",
            ),
            notifications,
        )
        .unwrap();
    assert_eq!(queued.status, AgentSteerRunResultStatus::Queued);
    release_continuation_tx.send(()).unwrap();

    let mut saw_queued = false;
    let mut saw_applied = false;
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let notification = receiver.recv().await.unwrap();
            match notification["params"]["type"].as_str().unwrap() {
                "guidance_queued" => saw_queued = true,
                "guidance_applied" => saw_applied = true,
                "done" if notification["params"]["status"] == "completed" => break,
                _ => {}
            }
        }
    })
    .await
    .unwrap();
    let guided_request = guided_request_rx.await.unwrap();
    server.await.unwrap();

    assert!(saw_queued);
    assert!(saw_applied);
    let guided_request_json = serde_json::to_string(&guided_request).unwrap();
    assert!(guided_request_json.contains("Please use the newer constraint."));
    assert!(guided_request_json.contains("fileChangeTarget"));
    assert!(guided_request_json.contains("observationId"));
    let journal = storage
        .load_agent_run_guidance(&queued.guidance_id)
        .unwrap()
        .unwrap();
    assert_eq!(journal.status, AgentGuidanceStatus::Applied);
    assert_eq!(
        std::fs::read_to_string(workspace.join("guided.txt")).unwrap(),
        "draft",
        "the approved Direct change must publish the content planned from the read observation"
    );
    let conversation = storage
        .load_conversation(&turn.conversation_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        conversation
            .messages
            .iter()
            .filter(|message| message.role == "assistant")
            .count(),
        1
    );
    let committed_model_context = storage
        .get_conversation_model_context_log(&turn.assistant_message_id)
        .unwrap()
        .expect("approved FileChange retains its committed model-context prefix");
    let committed_tool_result = committed_model_context
        .items
        .iter()
        .find(|item| item.tool_call_id.as_deref() == Some(approved_tool_call_id.as_str()))
        .expect("approved FileChange ToolResult is present in the committed prefix");
    assert!(!committed_tool_result.content.contains("fileChangeTarget"));
    assert!(!committed_tool_result.content.contains("observationId"));
    let committed_trace = storage
        .get_conversation_turn_trace(&turn.assistant_message_id)
        .unwrap()
        .expect("approved FileChange retains its committed Trace");
    let committed_trace_json = serde_json::to_string(&committed_trace).unwrap();
    assert!(!committed_trace_json.contains("fileChangeTarget"));
    assert!(!committed_trace_json.contains("observationId"));
}
