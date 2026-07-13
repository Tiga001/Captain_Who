// Tests for runtime message construction and tool-flow helpers.
use super::*;
use crate::protocol::{
    AgentInputAttachment, AgentInputAttachmentEncoding, AgentInputAttachmentKind, AgentRunContext,
    AgentWorkspaceContext,
};
use crate::runtime::tool_flow::parse_tool_call_request;
use crate::{
    ConversationTraceToolResultStatus, ConversationTurnTrace, ConversationTurnTraceItem,
    ConversationTurnTraceTerminalStatus, CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
};

fn message(role: &str, content: &str) -> AgentChatMessage {
    AgentChatMessage {
        message_id: None,
        role: role.to_string(),
        content: content.to_string(),
        created_at: None,
        conversation_turn_trace: None,
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
        None,
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
        None,
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

#[test]
fn context_window_preview_is_available_independently_of_indicator_events() {
    let input = serde_json::from_value::<AgentChatInput>(serde_json::json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "",
        "model": "test-model",
        "contextWindowTokens": 128000,
        "contextWindowIndicatorEnabled": false,
        "messages": []
    }))
    .unwrap();

    assert!(inspect_context_window(input).unwrap().is_some());
}

fn conversation_context_input(messages: Vec<AgentChatMessage>) -> AgentChatInput {
    AgentChatInput {
        api_url: "https://example.test/v1/chat/completions".to_string(),
        api_token: String::new(),
        model: "test-model".to_string(),
        api_style: Some(crate::protocol::AgentApiStyle::OpenAiCompatible),
        context_window_tokens: Some(128_000),
        context_window_indicator_enabled: true,
        max_tokens: Some(30_000),
        temperature: None,
        stream: Some(true),
        context: None,
        search_config: None,
        prompt_preferences: None,
        approval_decision: None,
        tool_continuation: None,
        attachments: Vec::new(),
        resume_checkpoint: None,
        assistant_message_id: None,
        context_compaction_summary: None,
        messages,
    }
}

fn conversation_context_trace(
    terminal_status: ConversationTurnTraceTerminalStatus,
    items: Vec<ConversationTurnTraceItem>,
) -> ConversationTurnTrace {
    ConversationTurnTrace {
        schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
        run_id: "run-context".to_string(),
        conversation_id: "conversation-context".to_string(),
        assistant_message_id: "assistant-context".to_string(),
        terminal_status,
        terminal_error: None,
        truncated: false,
        items,
    }
}

fn traced_assistant_message(content: &str, trace: ConversationTurnTrace) -> AgentChatMessage {
    AgentChatMessage {
        message_id: Some(trace.assistant_message_id.clone()),
        role: "assistant".to_string(),
        content: content.to_string(),
        created_at: None,
        conversation_turn_trace: Some(trace),
    }
}

fn full_conversation_context_snapshot(
    messages: Vec<AgentChatMessage>,
) -> AgentContextWindowSnapshot {
    create_conversation_context_state(conversation_context_input(messages))
        .unwrap()
        .snapshot(AgentContextWindowPhase::DurableCommit)
}

#[test]
fn conversation_context_state_incremental_updates_match_full_rebuilds() {
    let first_user = message("user", "Inspect the project and update src/lib.rs");
    let mut state =
        create_conversation_context_state(conversation_context_input(vec![first_user.clone()]))
            .unwrap();

    let narration = ConversationTurnTraceItem::AssistantNarration {
        sequence: 0,
        content: "I will inspect the current implementation first.".to_string(),
        truncated: false,
    };
    let narrated_trace = conversation_context_trace(
        ConversationTurnTraceTerminalStatus::InProgress,
        vec![narration.clone()],
    );
    let cursor = state.append_trace_items(&narrated_trace, 0).unwrap();
    assert_eq!(cursor, 1);
    assert_eq!(
        state.snapshot(AgentContextWindowPhase::DurableCommit),
        full_conversation_context_snapshot(vec![
            first_user.clone(),
            traced_assistant_message("", narrated_trace.clone()),
        ])
    );

    let call = ConversationTurnTraceItem::ToolCall {
        sequence: 1,
        call_id: "call-context".to_string(),
        tool: "read_file".to_string(),
        operation: json!({ "path": "src/lib.rs" }),
        approval_status: AgentApprovalStatus::NotRequired,
        truncated: false,
    };
    let result = ConversationTurnTraceItem::ToolResult {
        sequence: 2,
        call_id: "call-context".to_string(),
        tool: "read_file".to_string(),
        status: ConversationTraceToolResultStatus::Succeeded,
        success: true,
        observation: json!({ "path": "src/lib.rs", "endLine": 40 }),
        approval_status: AgentApprovalStatus::NotRequired,
        error: None,
        truncated: false,
    };
    let closed_trace = conversation_context_trace(
        ConversationTurnTraceTerminalStatus::InProgress,
        vec![narration, call, result],
    );
    let cursor = state.append_trace_items(&closed_trace, cursor).unwrap();
    assert_eq!(cursor, 3);
    assert_eq!(
        state.snapshot(AgentContextWindowPhase::DurableCommit),
        full_conversation_context_snapshot(vec![
            first_user.clone(),
            traced_assistant_message("", closed_trace.clone()),
        ])
    );

    let completed_trace = ConversationTurnTrace {
        terminal_status: ConversationTurnTraceTerminalStatus::Completed,
        ..closed_trace
    };
    let final_content = "I updated the implementation and verified the tests.";
    state
        .finalize_conversation_turn(&completed_trace, cursor, final_content)
        .unwrap();
    assert_eq!(
        state.snapshot(AgentContextWindowPhase::DurableCommit),
        full_conversation_context_snapshot(vec![
            first_user.clone(),
            traced_assistant_message(final_content, completed_trace.clone()),
        ])
    );

    let follow_up = "Now explain the change.";
    state.append_user_message(None, follow_up);
    assert_eq!(
        state.snapshot(AgentContextWindowPhase::DurableCommit),
        full_conversation_context_snapshot(vec![
            first_user,
            traced_assistant_message(final_content, completed_trace),
            message("user", follow_up),
        ])
    );
}

#[test]
fn runtime_shared_baseline_matches_full_context_assembly() {
    let input = conversation_context_input(vec![
        message("user", "First question"),
        message("assistant", "First answer"),
        message("user", "Current question"),
    ]);
    let capabilities = prepare_runtime_capabilities(&input, "baseline-test", &[], true).unwrap();
    let full =
        build_llm_request(input.clone(), &capabilities.tool_definitions, None, None).unwrap();
    let mut durable_state = create_conversation_context_state(input.clone()).unwrap();
    let baseline = durable_state.shared_baseline().unwrap();
    let shared =
        build_llm_request(input, &capabilities.tool_definitions, None, Some(baseline)).unwrap();

    assert_eq!(shared.context.to_messages(), full.context.to_messages());
}

#[tokio::test]
async fn durable_compaction_runs_before_capacity_gate_and_then_sends_rebuilt_context() {
    use crate::context::{ContextCompactionGeneration, ContextCompactionSummary};
    use crate::protocol::AgentApiStyle;
    use crate::{AgentUsage, ContextCompactionPrefix, ContextCompactionSummaryDraft};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    async fn read_http_body(stream: &mut TcpStream) -> Vec<u8> {
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4_096];
        let mut expected_length = None;
        loop {
            let read = stream.read(&mut buffer).await.unwrap();
            if read == 0 {
                break;
            }
            request.extend_from_slice(&buffer[..read]);
            if expected_length.is_none() {
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
                        .unwrap_or_default();
                    expected_length = Some(header_end + 4 + content_length);
                }
            }
            if expected_length.is_some_and(|length| request.len() >= length) {
                break;
            }
        }
        let body_start = request
            .windows(4)
            .position(|part| part == b"\r\n\r\n")
            .map(|index| index + 4)
            .unwrap();
        request[body_start..].to_vec()
    }

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (body_sender, body_receiver) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let body = read_http_body(&mut stream).await;
        body_sender.send(body).unwrap();
        let response_body = serde_json::to_vec(&json!({
            "choices": [{
                "message": { "role": "assistant", "content": "done" },
                "finish_reason": "stop"
            }]
        }))
        .unwrap();
        let headers = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            response_body.len()
        );
        stream.write_all(headers.as_bytes()).await.unwrap();
        stream.write_all(&response_body).await.unwrap();
    });

    let old_user = AgentChatMessage {
        message_id: Some("user-old".to_string()),
        role: "user".to_string(),
        content: format!("OLD_USER_MARKER {}", "x".repeat(60_000)),
        created_at: None,
        conversation_turn_trace: None,
    };
    let old_assistant = AgentChatMessage {
        message_id: Some("assistant-old".to_string()),
        role: "assistant".to_string(),
        content: format!("OLD_ASSISTANT_MARKER {}", "y".repeat(60_000)),
        created_at: None,
        conversation_turn_trace: None,
    };
    let current_user = AgentChatMessage {
        message_id: Some("user-current".to_string()),
        role: "user".to_string(),
        content: "continue".to_string(),
        created_at: None,
        conversation_turn_trace: None,
    };
    let input = AgentChatInput {
        api_url: format!("http://{address}/v1/chat/completions"),
        api_token: "test-token".to_string(),
        model: "test-model".to_string(),
        api_style: Some(AgentApiStyle::OpenAiCompatible),
        context_window_tokens: Some(50_000),
        context_window_indicator_enabled: false,
        max_tokens: Some(1_000),
        temperature: None,
        stream: Some(false),
        context: Some(AgentRunContext {
            conversation_id: Some("conversation-1".to_string()),
            project_id: None,
            workspace: None,
            attachment_library: None,
            permissions: Default::default(),
        }),
        search_config: None,
        prompt_preferences: None,
        approval_decision: None,
        tool_continuation: None,
        attachments: Vec::new(),
        resume_checkpoint: None,
        assistant_message_id: Some("assistant-current".to_string()),
        context_compaction_summary: None,
        messages: vec![old_user, old_assistant, current_user.clone()],
    };
    let compacted_summary = ContextCompactionSummary {
        schema_version: crate::CONTEXT_COMPACTION_SUMMARY_SCHEMA_VERSION,
        id: "summary-runtime".to_string(),
        conversation_id: "conversation-1".to_string(),
        source_revision: "source-runtime".to_string(),
        previous_summary_id: None,
        covered_through_message_id: "assistant-old".to_string(),
        covered_message_ids: vec!["user-old".to_string(), "assistant-old".to_string()],
        content: "COMPACTED_HISTORY_MARKER: the old task was completed.".to_string(),
        generation: ContextCompactionGeneration::test(),
        source_input_tokens: 40_000,
        summary_input_tokens: 32,
        created_at: 1,
    };
    let mut compacted_state = create_conversation_context_state(AgentChatInput {
        context_compaction_summary: Some(compacted_summary),
        messages: vec![current_user],
        ..input.clone()
    })
    .unwrap();
    let compacted_baseline = compacted_state.shared_baseline().unwrap();
    let prepare_count = Arc::new(AtomicUsize::new(0));
    let generate_count = Arc::new(AtomicUsize::new(0));
    let commit_count = Arc::new(AtomicUsize::new(0));
    let prepare_counter = prepare_count.clone();
    let generate_counter = generate_count.clone();
    let commit_counter = commit_count.clone();
    let services = AgentContextCompactionServices::new(
        move |request, _| {
            prepare_counter.fetch_add(1, Ordering::SeqCst);
            assert_eq!(
                request.newly_covered_message_ids,
                vec!["user-old".to_string(), "assistant-old".to_string()]
            );
            async move {
                Ok(AgentContextCompactionPrepareOutcome::Ready(Arc::new(
                    ContextCompactionPrefix {
                        conversation_id: "conversation-1".to_string(),
                        source_revision: "source-runtime".to_string(),
                        covered_through_message_id: "assistant-old".to_string(),
                        covered_message_ids: vec![
                            "user-old".to_string(),
                            "assistant-old".to_string(),
                        ],
                        previous_summary: None,
                        source_messages: vec![
                            crate::ContextCompactionSourceMessage {
                                message_id: "user-old".to_string(),
                                role: "user".to_string(),
                                content: "old request".to_string(),
                                created_at: 1,
                                status: Some("sent".to_string()),
                                conversation_turn_trace: None,
                            },
                            crate::ContextCompactionSourceMessage {
                                message_id: "assistant-old".to_string(),
                                role: "assistant".to_string(),
                                content: "old answer".to_string(),
                                created_at: 2,
                                status: Some("sent".to_string()),
                                conversation_turn_trace: None,
                            },
                        ],
                    },
                )))
            }
        },
        move |request, _| {
            generate_counter.fetch_add(1, Ordering::SeqCst);
            async move {
                Ok(AgentContextCompactionGenerationOutput {
                    draft: ContextCompactionSummaryDraft {
                        id: "summary-runtime".to_string(),
                        source_revision: request.prefix.source_revision.clone(),
                        content: "COMPACTED_HISTORY_MARKER: the old task was completed."
                            .to_string(),
                        generation: ContextCompactionGeneration::test(),
                        source_input_tokens: request.source_input_tokens,
                        summary_input_tokens: 32,
                        created_at: 1,
                    },
                    usage: Some(AgentUsage {
                        input_tokens: Some(100),
                        output_tokens: Some(20),
                        output_thinking_tokens: None,
                        total_tokens: Some(120),
                        cached_input_tokens: None,
                        cache_creation_input_tokens: None,
                        billable_request_count: Some(1),
                    }),
                })
            }
        },
        move |request, _| {
            let baseline = compacted_baseline.clone();
            commit_counter.fetch_add(1, Ordering::SeqCst);
            assert_eq!(request.draft.id, "summary-runtime");
            async move {
                Ok(AgentContextCompactionCommitOutcome::Applied {
                    summary_id: "summary-runtime".to_string(),
                    baseline: Box::new(baseline),
                })
            }
        },
    );
    let emitted_events = Arc::new(Mutex::new(Vec::new()));
    let emitted_events_for_callback = emitted_events.clone();
    let emitter: AgentEventEmitter = Arc::new(move |event| {
        emitted_events_for_callback.lock().unwrap().push(event);
    });

    let output = AgentRuntime::default()
        .send_chat_with_events_and_cancellation(
            input,
            Some("run-compaction".to_string()),
            Some(emitter),
            AgentCancellationToken::new(),
            Some(AgentRuntimeHostServices::new().with_context_compaction(services)),
        )
        .await
        .unwrap();
    server.await.unwrap();
    let request_body = String::from_utf8(body_receiver.await.unwrap()).unwrap();

    assert_eq!(output.content, "done");
    assert_eq!(prepare_count.load(Ordering::SeqCst), 1);
    assert_eq!(generate_count.load(Ordering::SeqCst), 1);
    assert_eq!(commit_count.load(Ordering::SeqCst), 1);
    let usage = output.usage.as_ref().unwrap();
    assert_eq!(usage.input_tokens, Some(100));
    assert_eq!(usage.output_tokens, Some(20));
    assert_eq!(usage.billable_request_count, Some(2));
    assert!(request_body.contains("COMPACTED_HISTORY_MARKER"));
    assert!(!request_body.contains("OLD_USER_MARKER"));
    assert!(!request_body.contains("OLD_ASSISTANT_MARKER"));
    let compaction_events = emitted_events
        .lock()
        .unwrap()
        .iter()
        .filter_map(|event| match event {
            AgentEvent::ContextCompactionStarted { .. } => Some("started"),
            AgentEvent::ContextCompactionFinished { .. } => Some("finished"),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(compaction_events, vec!["started", "finished"]);
}

#[tokio::test]
async fn context_capacity_guard_rejects_the_initial_request_before_network_io() {
    use tokio::net::TcpListener;
    use tokio::time::{timeout, Duration};

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let input = AgentChatInput {
        api_url: format!("http://{address}/v1/chat/completions"),
        api_token: "test-token".to_string(),
        model: "test-model".to_string(),
        api_style: Some(crate::protocol::AgentApiStyle::OpenAiCompatible),
        context_window_tokens: Some(8_000),
        context_window_indicator_enabled: false,
        max_tokens: Some(1_000),
        temperature: None,
        stream: Some(false),
        context: None,
        search_config: None,
        prompt_preferences: None,
        approval_decision: None,
        tool_continuation: None,
        attachments: Vec::new(),
        resume_checkpoint: None,
        assistant_message_id: None,
        context_compaction_summary: None,
        messages: vec![AgentChatMessage {
            message_id: None,
            role: "user".to_string(),
            content: "x".repeat(90_000),
            created_at: None,
            conversation_turn_trace: None,
        }],
    };

    let error = AgentRuntime::default().send_chat(input).await.unwrap_err();

    assert_eq!(error.code(), Some("context_capacity_exceeded"));
    assert_eq!(
        error
            .details()
            .and_then(|details| details["status"].as_str()),
        Some("over_budget")
    );
    assert_eq!(
        error
            .details()
            .and_then(|details| details["usage"]["measurementMode"].as_str()),
        Some("incremental_cache")
    );
    assert!(timeout(Duration::from_millis(100), listener.accept())
        .await
        .is_err());
}

#[tokio::test]
async fn context_capacity_guard_rechecks_after_tool_results_before_network_io() {
    use crate::protocol::{
        AgentCommandPermission, AgentPatchPermission, AgentPermissions, AgentReadPermission,
        AgentWritePermission,
    };
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;
    use tempfile::tempdir;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    async fn read_http_request(stream: &mut TcpStream) {
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4_096];
        let mut expected_length = None;
        loop {
            let read = stream.read(&mut buffer).await.unwrap();
            if read == 0 {
                return;
            }
            request.extend_from_slice(&buffer[..read]);
            if expected_length.is_none() {
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
                        .unwrap_or_default();
                    expected_length = Some(header_end + 4 + content_length);
                }
            }
            if expected_length.is_some_and(|length| request.len() >= length) {
                return;
            }
        }
    }

    async fn write_json_response(stream: &mut TcpStream, body: Value) {
        let body = serde_json::to_vec(&body).unwrap();
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(response.as_bytes()).await.unwrap();
        stream.write_all(&body).await.unwrap();
    }

    let fixture = tempdir().unwrap();
    let workspace = fixture.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let large_file = (0..2_000)
        .map(|_| "x".repeat(120))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(workspace.join("large.txt"), large_file).unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let second_request_seen = Arc::new(AtomicBool::new(false));
    let second_request_seen_by_server = second_request_seen.clone();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_http_request(&mut stream).await;
        write_json_response(
            &mut stream,
            json!({
                "choices": [{
                    "message": {
                        "role": "assistant",
                        "content": null,
                        "tool_calls": [{
                            "id": "read-large-file",
                            "type": "function",
                            "function": {
                                "name": "read_file",
                                "arguments": "{\"path\":\"large.txt\",\"maxLines\":2000}"
                            }
                        }]
                    },
                    "finish_reason": "tool_calls"
                }]
            }),
        )
        .await;

        if let Ok(Ok((_stream, _address))) =
            tokio::time::timeout(Duration::from_millis(500), listener.accept()).await
        {
            second_request_seen_by_server.store(true, Ordering::SeqCst);
        }
    });
    let input = AgentChatInput {
        api_url: format!("http://{address}/v1/chat/completions"),
        api_token: "test-token".to_string(),
        model: "test-model".to_string(),
        api_style: Some(crate::protocol::AgentApiStyle::OpenAiCompatible),
        context_window_tokens: Some(80_000),
        context_window_indicator_enabled: true,
        max_tokens: Some(1_000),
        temperature: None,
        stream: Some(false),
        context: Some(AgentRunContext {
            conversation_id: Some("conversation-capacity".to_string()),
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
        assistant_message_id: None,
        context_compaction_summary: None,
        messages: vec![message("user", "Read large.txt and summarize it")],
    };

    let error = AgentRuntime::default().send_chat(input).await.unwrap_err();
    server.await.unwrap();

    assert_eq!(error.code(), Some("context_capacity_exceeded"));
    assert!(!second_request_seen.load(Ordering::SeqCst));
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
        context_window_tokens: Some(128_000),
        context_window_indicator_enabled: true,
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
        assistant_message_id: None,
        context_compaction_summary: None,
        messages: vec![message("user", "create a preview")],
    };
    let output = AgentRuntime::default()
        .send_chat_with_events_and_cancellation(
            input,
            Some("run-preview".to_string()),
            Some(emitter),
            AgentCancellationToken::new(),
            Some(AgentRuntimeHostServices::new().with_storage(storage.clone())),
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
    let context_snapshots = captured
        .lock()
        .unwrap()
        .iter()
        .filter_map(|event| match event {
            AgentEvent::ContextWindowUpdated { snapshot, .. } => Some(snapshot.clone()),
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
    assert!(context_snapshots.is_empty());
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
        context_window_tokens: Some(128_000),
        context_window_indicator_enabled: true,
        max_tokens: Some(1_000),
        temperature: None,
        stream: Some(true),
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
        assistant_message_id: Some("assistant-checkpoint".to_string()),
        context_compaction_summary: None,
        messages: vec![message("user", "collect evidence and write report.txt")],
    };
    let waiting = AgentRuntime::default()
        .send_chat_with_events_and_cancellation(
            base_input.clone(),
            Some("run-checkpoint".to_string()),
            None,
            AgentCancellationToken::new(),
            None,
        )
        .await
        .unwrap();
    assert_eq!(waiting.status, AgentRunStatus::WaitingForApproval);
    assert!(waiting.conversation_turn_trace.is_none());
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
    assert!(checkpoint
        .conversation_trace_items
        .iter()
        .any(|item| matches!(
            item,
            ConversationTurnTraceItem::AssistantNarration { content, .. }
                if content == "I have the evidence and will prepare the report."
        )));

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
    let trace = completed.conversation_turn_trace.as_ref().unwrap();
    trace.validate().unwrap();
    for call_id in [
        "todo-before",
        "read-before",
        "patch-approval",
        "read-queued",
    ] {
        assert_eq!(
            trace
                .items
                .iter()
                .filter(|item| matches!(
                    item,
                    ConversationTurnTraceItem::ToolCall { call_id: item_id, .. }
                        if item_id == call_id
                ))
                .count(),
            1,
            "tool call {call_id} should be retained exactly once"
        );
        assert_eq!(
            trace
                .items
                .iter()
                .filter(|item| matches!(
                    item,
                    ConversationTurnTraceItem::ToolResult { call_id: item_id, .. }
                        if item_id == call_id
                ))
                .count(),
            1,
            "tool result {call_id} should be retained exactly once"
        );
    }
}
