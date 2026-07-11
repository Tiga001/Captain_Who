// Tests for runtime message construction and tool-flow helpers.
use super::*;
use crate::protocol::{
    AgentInputAttachment, AgentInputAttachmentEncoding, AgentInputAttachmentKind, AgentRunContext,
    AgentWorkspaceContext,
};
use crate::runtime::tool_flow::parse_tool_call_request;

fn message(role: &str, content: &str) -> AgentChatMessage {
    AgentChatMessage {
        role: role.to_string(),
        content: content.to_string(),
    }
}

fn empty_attachment_context() -> AttachmentContext {
    AttachmentContext {
        text: String::new(),
        images: Vec::new(),
    }
}

#[test]
fn runtime_messages_add_backend_system_prompt() {
    let context = AgentRunContext {
        conversation_id: Some("conversation-1".to_string()),
        project_id: Some("project-1".to_string()),
        workspace: Some(AgentWorkspaceContext {
            project_id: Some("project-1".to_string()),
            display_name: Some("Workspace".to_string()),
            root_path: Some("/private/path".to_string()),
        }),
        attachment_library: None,
        permissions: Default::default(),
    };
    let context = assemble_initial_context(
        vec![message("user", "Read src/main.rs")],
        empty_attachment_context(),
        Some(&context),
        None,
        &ToolRegistry::defaults_with_search(None).definitions(),
    )
    .unwrap();
    let messages = context.to_messages();

    assert_eq!(messages[0].role.as_str(), "system");
    assert!(messages[0].content.contains("MyCopilot"));
    assert!(!messages[0].content.contains("/private/path"));
    assert_eq!(messages[1].role.as_str(), "user");
}

#[test]
fn runtime_messages_include_text_attachment_content() {
    let context = assemble_initial_context(
        vec![message("user", "Summarize this attachment")],
        AttachmentContext {
            text: "用户输入框附件内容如下。\n\n### notes.txt\nhello from attachment".to_string(),
            images: Vec::new(),
        },
        None,
        None,
        &ToolRegistry::defaults_with_search(None).definitions(),
    )
    .unwrap();
    let messages = context.to_messages();

    assert!(messages
        .iter()
        .any(|message| message.role == LlmMessageRole::User
            && message.content.contains("hello from attachment")));
}

#[test]
fn attachment_context_reads_text_with_registered_tool() {
    let context = build_attachment_context(&[AgentInputAttachment {
        id: "attachment-1".to_string(),
        kind: AgentInputAttachmentKind::File,
        name: "notes.txt".to_string(),
        mime_type: Some("text/plain".to_string()),
        size_bytes: 16,
        encoding: AgentInputAttachmentEncoding::Utf8,
        data: "hello from file".to_string(),
        truncated: None,
    }])
    .unwrap();

    assert!(context.text.contains("读取工具：read_file"));
    assert!(context.text.contains("hello from file"));
}

#[test]
fn read_image_tool_result_is_redacted_but_creates_visual_message() {
    let result = AgentToolResult {
        call_id: "call-image".to_string(),
        tool: "read_image".to_string(),
        ok: true,
        result: Some(json!({
            "path": "@attachments/image1/pixel.png",
            "format": "png",
            "mimeType": "image/png",
            "sizeBytes": 3,
            "image": {
                "mimeType": "image/png",
                "dataBase64": "YWJj"
            }
        })),
        error: None,
    };

    let event_result = redact_tool_result_for_event(&result);
    assert_eq!(
        event_result.result.as_ref().unwrap()["image"]["dataBase64"],
        "[redacted]"
    );

    let image_message = llm_image_message_from_tool_result(&result).unwrap();
    assert_eq!(image_message.role, LlmMessageRole::User);
    assert_eq!(image_message.images.len(), 1);
    assert_eq!(image_message.images[0].mime_type, "image/png");
    assert_eq!(image_message.images[0].data_base64, "YWJj");
}

#[test]
fn file_write_tail_is_available_to_llm_but_not_persisted_in_events() {
    let result = AgentToolResult {
        call_id: "call-write".to_string(),
        tool: "write_file".to_string(),
        ok: true,
        result: Some(json!({
            "draft": { "draftId": "draft-1" },
            "tail": "private generated content"
        })),
        error: None,
    };

    let llm_result = redact_tool_result_for_llm(&result);
    let event_result = redact_tool_result_for_event(&result);

    assert_eq!(
        llm_result.result.as_ref().unwrap()["tail"],
        "private generated content"
    );
    assert!(event_result.result.as_ref().unwrap().get("tail").is_none());
    assert_eq!(
        event_result.result.as_ref().unwrap()["draft"]["draftId"],
        "draft-1"
    );
}

#[test]
fn parses_plain_and_fenced_tool_calls() {
    let plain = parse_tool_call_request(
        r#"{"type":"tool_call","tool":"search_files","args":{"query":"main"}}"#,
    )
    .unwrap();
    let fenced = parse_tool_call_request(
            "```json\n{\"type\":\"tool_call\",\"tool\":\"read_file\",\"args\":{\"path\":\"src/lib.rs\"}}\n```",
        )
        .unwrap();

    assert_eq!(plain.tool, "search_files");
    assert_eq!(plain.args["query"], "main");
    assert_eq!(fenced.tool, "read_file");
    assert_eq!(fenced.args["path"], "src/lib.rs");
}

#[test]
fn transient_events_are_emitted_without_entering_output_history() {
    let captured = Arc::new(std::sync::Mutex::new(Vec::<AgentEvent>::new()));
    let captured_for_emitter = captured.clone();
    let emitter: AgentEventEmitter = Arc::new(move |event| {
        captured_for_emitter.lock().unwrap().push(event);
    });
    let stream = AgentEventStream::new(Some(emitter));

    stream.emit_transient(AgentEvent::ToolInputProgress {
        run_id: "run-1".to_string(),
        stream_id: "stream-1".to_string(),
        attempt: 1,
        tool_call_index: 0,
        tool_call_id: Some("call-1".to_string()),
        tool: "write_file".to_string(),
        received_bytes: 128,
    });

    assert_eq!(captured.lock().unwrap().len(), 1);
    assert!(stream.into_events().is_empty());
}

#[tokio::test]
async fn streams_write_file_previews_end_to_end_without_persisting_them() {
    use crate::protocol::{
        AgentCommandPermission, AgentPatchPermission, AgentPermissions, AgentReadPermission,
        AgentWritePermission,
    };
    use crate::storage::models::ChatConversationRecord;
    use crate::storage::service::StorageService;
    use std::time::Duration;
    use tempfile::tempdir;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    async fn read_request(stream: &mut TcpStream) {
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4096];
        let mut expected_len = None;
        loop {
            let read = stream.read(&mut buffer).await.unwrap();
            if read == 0 {
                return;
            }
            request.extend_from_slice(&buffer[..read]);
            if expected_len.is_none() {
                if let Some(header_end) = request.windows(4).position(|part| part == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&request[..header_end]);
                    let content_length = headers
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().ok())
                                .flatten()
                        })
                        .unwrap_or(0);
                    expected_len = Some(header_end + 4 + content_length);
                }
            }
            if expected_len.is_some_and(|length| request.len() >= length) {
                return;
            }
        }
    }

    async fn write_json_response(stream: &mut TcpStream, body: Value) {
        let body = serde_json::to_vec(&body).unwrap();
        let headers = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(headers.as_bytes()).await.unwrap();
        stream.write_all(&body).await.unwrap();
    }

    fn tool_completion(id: &str, arguments: Value) -> Value {
        json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": id,
                        "type": "function",
                        "function": {
                            "name": "write_file",
                            "arguments": serde_json::to_string(&arguments).unwrap()
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        })
    }

    async fn write_append_stream(stream: &mut TcpStream, draft_id: &str) {
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
            )
            .await
            .unwrap();
        let fragments = [
            format!(
                "{{\"phase\":\"append\",\"draftId\":\"{draft_id}\",\"index\":0,\"content\":\"line 1\\n"
            ),
            "line 2\\n".to_string(),
            "line 3\\n".to_string(),
            "line 4\\n\"}".to_string(),
        ];
        let tool_name_fragments = ["write_", "file", "", ""];
        for (fragment, tool_name) in fragments.into_iter().zip(tool_name_fragments) {
            let frame = json!({
                "choices": [{
                    "delta": {
                        "tool_calls": [{
                            "index": 0,
                            "id": "call-append",
                            "type": "function",
                            "function": {
                                "name": tool_name,
                                "arguments": fragment
                            }
                        }]
                    }
                }]
            });
            stream
                .write_all(format!("data: {frame}\n\n").as_bytes())
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
        stream
            .write_all(
                format!(
                    "data: {}\n\ndata: [DONE]\n\n",
                    json!({ "choices": [{ "delta": {}, "finish_reason": "tool_calls" }] })
                )
                .as_bytes(),
            )
            .await
            .unwrap();
    }

    let fixture = tempdir().unwrap();
    let workspace = fixture.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("app.db")).unwrap());
    storage
        .save_conversation(ChatConversationRecord {
            id: "conversation-preview".to_string(),
            project_id: None,
            model_id: None,
            title: "Preview".to_string(),
            messages: Vec::new(),
            created_at: 1,
            updated_at: 1,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server_storage = storage.clone();
    let server = tokio::spawn(async move {
        for request_index in 0..4 {
            let (mut stream, _) = listener.accept().await.unwrap();
            read_request(&mut stream).await;
            match request_index {
                0 => {
                    write_json_response(
                        &mut stream,
                        tool_completion(
                            "call-begin",
                            json!({
                                "phase": "begin",
                                "filePath": "preview.md",
                                "mode": "create"
                            }),
                        ),
                    )
                    .await;
                }
                1 => {
                    let draft_id = server_storage
                        .list_agent_file_drafts_for_run("run-preview")
                        .unwrap()[0]
                        .id
                        .clone();
                    write_append_stream(&mut stream, &draft_id).await;
                }
                2 => {
                    let draft_id = server_storage
                        .list_agent_file_drafts_for_run("run-preview")
                        .unwrap()[0]
                        .id
                        .clone();
                    write_json_response(
                        &mut stream,
                        tool_completion(
                            "call-abort",
                            json!({ "phase": "abort", "draftId": draft_id }),
                        ),
                    )
                    .await;
                }
                _ => {
                    write_json_response(
                        &mut stream,
                        json!({
                            "choices": [{
                                "message": { "role": "assistant", "content": "done" },
                                "finish_reason": "stop"
                            }]
                        }),
                    )
                    .await;
                }
            }
        }
    });

    let captured = Arc::new(std::sync::Mutex::new(Vec::<AgentEvent>::new()));
    let captured_for_emitter = captured.clone();
    let emitter: AgentEventEmitter = Arc::new(move |event| {
        captured_for_emitter.lock().unwrap().push(event);
    });
    let input = AgentChatInput {
        api_url: format!("http://{address}/v1/chat/completions"),
        api_token: "test-token".to_string(),
        model: "test-model".to_string(),
        api_style: Some(crate::protocol::AgentApiStyle::OpenAiCompatible),
        max_tokens: Some(1_000),
        temperature: None,
        stream: Some(true),
        context: Some(AgentRunContext {
            conversation_id: Some("conversation-preview".to_string()),
            project_id: None,
            workspace: Some(AgentWorkspaceContext {
                project_id: None,
                display_name: Some("workspace".to_string()),
                root_path: Some(workspace.to_string_lossy().to_string()),
            }),
            attachment_library: None,
            permissions: AgentPermissions {
                read: AgentReadPermission::WorkspaceOnly,
                write: AgentWritePermission::WorkspaceOnly,
                command: AgentCommandPermission::RequireApproval,
                patch: AgentPatchPermission::RequireApproval,
            },
        }),
        search_config: None,
        prompt_preferences: None,
        approval_decision: None,
        tool_continuation: None,
        attachments: Vec::new(),
        resume_checkpoint: None,
        messages: vec![message("user", "create a preview")],
    };
    let output = AgentRuntime::default()
        .send_chat_with_events_and_cancellation(
            input,
            Some("run-preview".to_string()),
            Some(emitter),
            AgentCancellationToken::new(),
            None,
            Some(storage.clone()),
        )
        .await
        .unwrap();
    server.await.unwrap();

    let previews = captured
        .lock()
        .unwrap()
        .iter()
        .filter_map(|event| match event {
            AgentEvent::FileWritePreviewUpdated { preview, .. } => Some(preview.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let preview_additions = previews
        .iter()
        .map(|preview| preview.additions)
        .collect::<Vec<_>>();
    let mut preview_content = String::new();
    for preview in &previews {
        assert_eq!(preview.content_offset_bytes, preview_content.len() as u64);
        preview_content.push_str(&preview.content_delta);
    }
    assert!(preview_additions.len() >= 3, "{preview_additions:?}");
    assert_eq!(preview_additions.last().copied(), Some(4));
    assert_eq!(preview_content, "line 1\nline 2\nline 3\nline 4\n");
    assert!(output.events.iter().all(|event| !matches!(
        event,
        AgentEvent::FileWritePreviewUpdated { .. } | AgentEvent::FileWritePreviewCleared { .. }
    )));
    assert_eq!(output.content, "done");
    assert_eq!(
        storage
            .list_agent_file_drafts_for_run("run-preview")
            .unwrap()[0]
            .status,
        "aborted"
    );
}

#[tokio::test]
async fn approval_resume_restores_prior_context_and_continues_queued_tools() {
    use crate::protocol::{
        AgentApprovalDecision, AgentApprovalDecisionStatus, AgentCommandPermission,
        AgentPatchPermission, AgentPermissions, AgentReadPermission, AgentToolContinuation,
        AgentWritePermission,
    };
    use tempfile::tempdir;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    async fn read_json_request(stream: &mut TcpStream) -> Value {
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4096];
        let mut body_start = None;
        let mut expected_len = None;
        loop {
            let read = stream.read(&mut buffer).await.unwrap();
            assert!(read > 0, "connection closed before request completed");
            request.extend_from_slice(&buffer[..read]);
            if body_start.is_none() {
                if let Some(header_end) = request.windows(4).position(|part| part == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&request[..header_end]);
                    let content_length = headers
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().ok())
                                .flatten()
                        })
                        .unwrap_or(0);
                    let start = header_end + 4;
                    body_start = Some(start);
                    expected_len = Some(start + content_length);
                }
            }
            if expected_len.is_some_and(|length| request.len() >= length) {
                break;
            }
        }
        serde_json::from_slice(&request[body_start.unwrap()..expected_len.unwrap()]).unwrap()
    }

    async fn write_response(stream: &mut TcpStream, body: Value) {
        let body = serde_json::to_vec(&body).unwrap();
        let headers = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(headers.as_bytes()).await.unwrap();
        stream.write_all(&body).await.unwrap();
    }

    fn native_tool_call(id: &str, name: &str, args: Value) -> Value {
        json!({
            "id": id,
            "type": "function",
            "function": {
                "name": name,
                "arguments": serde_json::to_string(&args).unwrap()
            }
        })
    }

    let fixture = tempdir().unwrap();
    let workspace = fixture.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(workspace.join("source.txt"), "evidence-before-approval").unwrap();
    std::fs::write(workspace.join("queued.txt"), "evidence-from-queued-tool").unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let final_request = Arc::new(std::sync::Mutex::new(None::<Value>));
    let server_final_request = final_request.clone();
    let server = tokio::spawn(async move {
        for request_index in 0..3 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = read_json_request(&mut stream).await;
            let response = match request_index {
                0 => json!({
                    "choices": [{
                        "message": {
                            "role": "assistant",
                            "content": "I will read the source first.",
                            "tool_calls": [
                                native_tool_call(
                                    "todo-before",
                                    "todo_update",
                                    json!({
                                        "items": [{
                                            "title": "Collect evidence and write report",
                                            "status": "in_progress"
                                        }]
                                    })
                                ),
                                native_tool_call(
                                    "read-before",
                                    "read_file",
                                    json!({ "path": "source.txt" })
                                )
                            ]
                        },
                        "finish_reason": "tool_calls"
                    }]
                }),
                1 => json!({
                    "choices": [{
                        "message": {
                            "role": "assistant",
                            "content": "I have the evidence and will prepare the report.",
                            "tool_calls": [
                                native_tool_call(
                                    "patch-approval",
                                    "apply_patch",
                                    json!({
                                        "operation": "create",
                                        "filePath": "report.txt",
                                        "content": "draft report"
                                    })
                                ),
                                native_tool_call(
                                    "read-queued",
                                    "read_file",
                                    json!({ "path": "queued.txt" })
                                )
                            ]
                        },
                        "finish_reason": "tool_calls"
                    }]
                }),
                _ => {
                    *server_final_request.lock().unwrap() = Some(request);
                    json!({
                        "choices": [{
                            "message": { "role": "assistant", "content": "resumed with evidence" },
                            "finish_reason": "stop"
                        }]
                    })
                }
            };
            write_response(&mut stream, response).await;
        }
    });

    let base_input = AgentChatInput {
        api_url: format!("http://{address}/v1/chat/completions"),
        api_token: "test-token".to_string(),
        model: "test-model".to_string(),
        api_style: Some(crate::protocol::AgentApiStyle::OpenAiCompatible),
        max_tokens: Some(1_000),
        temperature: None,
        stream: Some(false),
        context: Some(AgentRunContext {
            conversation_id: Some("conversation-checkpoint".to_string()),
            project_id: None,
            workspace: Some(AgentWorkspaceContext {
                project_id: None,
                display_name: Some("workspace".to_string()),
                root_path: Some(workspace.to_string_lossy().to_string()),
            }),
            attachment_library: None,
            permissions: AgentPermissions {
                read: AgentReadPermission::WorkspaceOnly,
                write: AgentWritePermission::WorkspaceOnly,
                command: AgentCommandPermission::RequireApproval,
                patch: AgentPatchPermission::RequireApproval,
            },
        }),
        search_config: None,
        prompt_preferences: None,
        approval_decision: None,
        tool_continuation: None,
        attachments: Vec::new(),
        resume_checkpoint: None,
        messages: vec![message("user", "collect evidence and write report.txt")],
    };
    let waiting = AgentRuntime::default()
        .send_chat_with_events_and_cancellation(
            base_input.clone(),
            Some("run-checkpoint".to_string()),
            None,
            AgentCancellationToken::new(),
            None,
            None,
        )
        .await
        .unwrap();
    assert_eq!(waiting.status, AgentRunStatus::WaitingForApproval);
    let checkpoint = waiting
        .events
        .iter()
        .find_map(|event| match event {
            AgentEvent::ApprovalRequired { checkpoint, .. } => Some(checkpoint.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(checkpoint.pending_tool_call_id, "patch-approval");
    assert_eq!(checkpoint.queued_tool_calls.len(), 1);
    assert_eq!(checkpoint.extension_snapshots[0].extension_id, "todo");

    let mut resume_input = base_input;
    resume_input.messages.clear();
    resume_input.resume_checkpoint = Some(checkpoint);
    resume_input.approval_decision = Some(AgentApprovalDecision {
        action_id: "patch-approval".to_string(),
        status: AgentApprovalDecisionStatus::Rejected,
        message: Some("Keep the evidence but revise the report first.".to_string()),
    });
    resume_input.tool_continuation = Some(AgentToolContinuation {
        call: AgentToolCall {
            id: "patch-approval".to_string(),
            tool: "apply_patch".to_string(),
            args: json!({ "operation": "create", "filePath": "report.txt" }),
            approval_status: AgentApprovalStatus::Rejected,
            reason: None,
        },
        result: AgentToolResult {
            call_id: "patch-approval".to_string(),
            tool: "apply_patch".to_string(),
            ok: false,
            result: None,
            error: Some(
                "user rejected: Keep the evidence but revise the report first.".to_string(),
            ),
        },
    });

    let completed = AgentRuntime::default()
        .send_chat_with_events_and_cancellation(
            resume_input,
            Some("run-checkpoint".to_string()),
            None,
            AgentCancellationToken::new(),
            None,
            None,
        )
        .await
        .unwrap();
    server.await.unwrap();

    assert_eq!(completed.status, AgentRunStatus::Completed);
    assert_eq!(completed.content, "resumed with evidence");
    assert_eq!(completed.todo.as_ref().unwrap().revision, 1);
    let request = final_request.lock().unwrap().take().unwrap();
    let messages = serde_json::to_string(&request["messages"]).unwrap();
    assert!(messages.contains("evidence-before-approval"));
    assert!(messages.contains("evidence-from-queued-tool"));
    assert!(messages.contains("user rejected"));
    assert!(messages.contains("read-before"));
    assert!(messages.contains("patch-approval"));
    assert!(messages.contains("read-queued"));
}
