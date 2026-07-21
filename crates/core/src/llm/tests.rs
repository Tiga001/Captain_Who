use super::transport::*;
use super::*;
use crate::context::{
    format_message_created_at, ContextAssembler, ContextAssemblyInput, ContextAttachments,
    ContextGroup, ContextItem, ContextMetadata, ContextRetention, ContextScope, ContextSource,
};
use crate::conversation_trace::{
    ConversationTraceToolResultStatus, ConversationTurnTrace, ConversationTurnTraceItem,
    ConversationTurnTraceTerminalStatus, CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
};
use crate::protocol::{AgentApprovalStatus, AgentChatMessage, AgentToolSafety};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

fn message(role: LlmMessageRole, content: &str) -> LlmMessage {
    LlmMessage::text(role, content)
}

fn joined_text(events: &[LlmStreamEvent]) -> String {
    events
        .iter()
        .filter_map(|event| match event {
            LlmStreamEvent::Delta(delta) => Some(delta.as_str()),
            _ => None,
        })
        .collect()
}

fn tool_definition() -> AgentToolDefinition {
    AgentToolDefinition {
        name: "read_file".to_string(),
        description: "Read a file.".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" }
            },
            "required": ["path"]
        }),
        safety: AgentToolSafety::ReadOnly,
        requires_workspace: true,
        requires_approval: false,
        approval_mode: crate::protocol::AgentToolApprovalMode::Never,
    }
}

async fn read_test_http_request(stream: &mut TcpStream) {
    let mut request = Vec::new();
    let mut content_length = None;
    let mut body_start = None;
    loop {
        let mut chunk = [0_u8; 4_096];
        let read = stream.read(&mut chunk).await.unwrap();
        assert!(
            read > 0,
            "test HTTP request ended before its body was complete"
        );
        request.extend_from_slice(&chunk[..read]);

        if body_start.is_none() {
            if let Some(index) = request.windows(4).position(|part| part == b"\r\n\r\n") {
                let start = index + 4;
                let headers = std::str::from_utf8(&request[..index]).unwrap();
                content_length = headers.lines().find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().unwrap())
                });
                body_start = Some(start);
            }
        }

        if let (Some(start), Some(length)) = (body_start, content_length) {
            if request.len() >= start + length {
                return;
            }
        }
    }
}

async fn write_test_http_response(stream: &mut TcpStream, status: &str, body: Value) {
    let body = serde_json::to_vec(&body).unwrap();
    let headers = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
    stream.write_all(headers.as_bytes()).await.unwrap();
    stream.write_all(&body).await.unwrap();
}

fn chat_message(role: &str, content: &str) -> AgentChatMessage {
    AgentChatMessage {
        message_id: None,
        role: role.to_string(),
        content: content.to_string(),
        created_at: None,
        conversation_turn_trace: None,
    }
}

fn traced_chat_message(content: &str) -> AgentChatMessage {
    AgentChatMessage {
        message_id: Some("assistant-history".to_string()),
        role: "assistant".to_string(),
        content: content.to_string(),
        created_at: None,
        conversation_turn_trace: Some(ConversationTurnTrace {
            schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
            run_id: "historical-run-1".to_string(),
            conversation_id: "conversation-1".to_string(),
            assistant_message_id: "assistant-1".to_string(),
            terminal_status: ConversationTurnTraceTerminalStatus::Completed,
            terminal_error: None,
            truncated: false,
            items: vec![
                ConversationTurnTraceItem::AssistantNarration {
                    sequence: 10,
                    content: "I will inspect src/lib.rs.".to_string(),
                    truncated: false,
                },
                ConversationTurnTraceItem::ToolCall {
                    sequence: 11,
                    call_id: "call-1".to_string(),
                    tool: "read_file".to_string(),
                    operation: json!({ "path": "src/lib.rs", "startLine": 1 }),
                    approval_status: AgentApprovalStatus::NotRequired,
                    truncated: false,
                },
                ConversationTurnTraceItem::ToolResult {
                    sequence: 12,
                    call_id: "call-1".to_string(),
                    tool: "read_file".to_string(),
                    status: ConversationTraceToolResultStatus::Succeeded,
                    success: true,
                    observation: json!({
                        "path": "src/lib.rs",
                        "startLine": 1,
                        "endLine": 20,
                        "truncated": false
                    }),
                    approval_status: AgentApprovalStatus::NotRequired,
                    error: None,
                    truncated: false,
                },
            ],
        }),
    }
}

#[test]
fn retry_delay_uses_capped_exponential_backoff() {
    assert_eq!(retry_delay(1), Duration::from_millis(350));
    assert_eq!(retry_delay(2), Duration::from_millis(700));
    assert_eq!(retry_delay(10), Duration::from_millis(2_000));
}

#[test]
fn classifies_transient_llm_errors_as_retryable() {
    assert!(is_retryable_llm_error(&AgentError::new(
        "读取模型流失败：error decoding response body"
    )));
    assert!(is_retryable_llm_error(&AgentError::new(
        "模型接口返回 429：rate limit"
    )));
    assert!(is_retryable_llm_error(&AgentError::new(
        "模型接口返回 503：upstream overloaded"
    )));
    assert!(is_retryable_llm_error(&AgentError::new(
            "模型接口返回 400：upstream status 400: Provider API error: The provided Content Type is invalid or not supported for this model"
        )));
}

#[test]
fn classifies_configuration_and_client_errors_as_non_retryable() {
    assert!(!is_retryable_llm_error(&AgentError::new(
        "请先在设置 > 配置里填写 API Token。"
    )));
    assert!(!is_retryable_llm_error(&AgentError::new(
        "模型接口返回 401：unauthorized"
    )));
    assert!(!is_retryable_llm_error(&AgentError::new(
        "模型接口返回 400：bad request"
    )));
    assert!(!is_retryable_llm_error(&AgentError::new(
        "模型接口返回 400：Invalid schema for function read_file"
    )));
}

#[tokio::test]
async fn streaming_retries_the_known_upstream_content_type_400_and_recovers() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        for attempt in 1..=2 {
            let (mut stream, _) = listener.accept().await.unwrap();
            read_test_http_request(&mut stream).await;
            if attempt == 1 {
                write_test_http_response(
                        &mut stream,
                        "400 Bad Request",
                        json!({
                            "error": {
                                "message": "upstream status 400: Provider API error: The provided Content Type is invalid or not supported for this model"
                            }
                        }),
                    )
                    .await;
            } else {
                write_test_http_response(
                    &mut stream,
                    "200 OK",
                    json!({
                        "choices": [{
                            "message": { "role": "assistant", "content": "recovered" },
                            "finish_reason": "stop"
                        }],
                        "usage": {
                            "prompt_tokens": 10,
                            "completion_tokens": 1,
                            "total_tokens": 11
                        }
                    }),
                )
                .await;
            }
        }
    });
    let request = LlmChatRequest {
        api_url: format!("http://{address}/v1/chat/completions"),
        api_token: "token".to_string(),
        model: "claude-opus-4-7".to_string(),
        api_style: AgentApiStyle::OpenAiCompatible,
        max_tokens: 1_024,
        temperature: 0.2,
        stream: false,
        messages: vec![message(LlmMessageRole::User, "Hello")],
        tools: Vec::new(),
    };

    let mut attempts_started = 0_usize;
    let mut attempts_reset = 0_usize;
    let mut retries = 0_usize;
    let mut commits = 0_usize;
    let response =
        complete_chat_streaming(
            request,
            AgentCancellationToken::new(),
            |event| match event {
                LlmStreamEvent::AttemptStarted { .. } => attempts_started += 1,
                LlmStreamEvent::AttemptReset { .. } => attempts_reset += 1,
                LlmStreamEvent::Retrying { .. } => retries += 1,
                LlmStreamEvent::Committed => commits += 1,
                _ => {}
            },
        )
        .await
        .unwrap();
    server.await.unwrap();

    assert_eq!(response.content, "recovered");
    assert_eq!(attempts_started, 2);
    assert_eq!(attempts_reset, 1);
    assert_eq!(retries, 1);
    assert_eq!(commits, 1);
    assert_eq!(
        response
            .usage
            .as_ref()
            .and_then(|usage| usage.billable_request_count),
        Some(2)
    );
}

#[test]
fn rejects_incompatible_tool_schema_before_building_an_http_request() {
    let mut tool = tool_definition();
    tool.input_schema["anyOf"] = json!([{ "required": ["path"] }]);
    let request = LlmChatRequest {
        api_url: "https://example.test/v1/chat/completions".to_string(),
        api_token: "token".to_string(),
        model: "gpt".to_string(),
        api_style: AgentApiStyle::OpenAiCompatible,
        max_tokens: 1024,
        temperature: 0.2,
        stream: true,
        messages: vec![message(LlmMessageRole::User, "Read a file")],
        tools: vec![tool],
    };

    let error = validate_request(&request).unwrap_err();

    assert!(error.to_string().contains("read_file"));
    assert!(error.to_string().contains("anyOf"));
}

#[test]
fn retry_exhausted_error_mentions_retry_count() {
    let error = retry_exhausted_error(
        AgentError::new("读取模型响应失败：timeout").with_usage(Some(AgentUsage {
            input_tokens: Some(7),
            output_tokens: None,
            output_thinking_tokens: None,
            total_tokens: None,
            cached_input_tokens: None,
            cache_creation_input_tokens: None,
            billable_request_count: Some(3),
        })),
        3,
    );

    assert!(error.to_string().contains("已重试 2 次"));
    assert!(error.to_string().contains("timeout"));
    assert_eq!(error.usage().unwrap().input_tokens, Some(7));
    assert_eq!(error.usage().unwrap().billable_request_count, Some(3));
}

#[test]
fn internal_callers_can_defer_empty_response_validation() {
    let body = json!({
        "choices": [{
            "message": { "role": "assistant", "content": "" },
            "finish_reason": "length"
        }]
    })
    .to_string();

    let strict = parse_non_stream_response(
        &body,
        AgentApiStyle::OpenAiCompatible,
        LlmResponseValidation::RequireModelAction,
    );
    let deferred = parse_non_stream_response(
        &body,
        AgentApiStyle::OpenAiCompatible,
        LlmResponseValidation::AllowEmpty,
    )
    .unwrap();

    assert!(strict.unwrap_err().to_string().contains("没有可显示文本"));
    assert!(deferred.content.is_empty());
    assert_eq!(deferred.finish_reason.as_deref(), Some("length"));
}

#[test]
fn detects_openai_chat_completion_urls() {
    assert_eq!(
        detect_api_style("https://example.test/v1/chat/completions"),
        AgentApiStyle::OpenAiCompatible
    );
}

#[test]
fn moves_system_messages_to_anthropic_system_field() {
    let request = LlmChatRequest {
        api_url: "https://api.anthropic.com/v1/messages".to_string(),
        api_token: "token".to_string(),
        model: "claude".to_string(),
        api_style: AgentApiStyle::AnthropicCompatible,
        max_tokens: 1024,
        temperature: 0.2,
        stream: false,
        messages: vec![
            message(LlmMessageRole::System, "Safety first."),
            message(LlmMessageRole::User, "Hello"),
            message(LlmMessageRole::Assistant, "Hi"),
        ],
        tools: Vec::new(),
    };

    let payload = build_payload(&request);

    assert_eq!(payload["system"], "Safety first.");
    assert_eq!(payload["messages"].as_array().unwrap().len(), 2);
    assert_eq!(payload["messages"][0]["role"], "user");
    assert_eq!(payload["messages"][0]["content"][0]["type"], "text");
}

#[test]
fn omits_temperature_for_claude_models() {
    let request = LlmChatRequest {
        api_url: "https://example.test/v1/chat/completions".to_string(),
        api_token: "token".to_string(),
        model: "claude-opus-4-7".to_string(),
        api_style: AgentApiStyle::OpenAiCompatible,
        max_tokens: 1024,
        temperature: 0.2,
        stream: true,
        messages: vec![message(LlmMessageRole::User, "Hello")],
        tools: vec![tool_definition()],
    };

    let payload = build_payload(&request);

    assert!(payload.get("temperature").is_none());
    assert_eq!(payload["stream_options"]["include_usage"], true);
}

#[test]
fn extracts_model_error_payloads() {
    let value = json!({
        "error": {
            "code": "BIZ_ERROR",
            "message": "upstream status 400"
        }
    });

    assert_eq!(
        extract_api_error(&value).as_deref(),
        Some("upstream status 400")
    );
}

#[test]
fn builds_openai_native_tool_payload_and_tool_result_messages() {
    let request = LlmChatRequest {
        api_url: "https://example.test/v1/chat/completions".to_string(),
        api_token: "token".to_string(),
        model: "gpt".to_string(),
        api_style: AgentApiStyle::OpenAiCompatible,
        max_tokens: 1024,
        temperature: 0.2,
        stream: false,
        messages: vec![
            message(LlmMessageRole::User, "Read src/lib.rs"),
            LlmMessage::assistant(
                "",
                vec![LlmToolCall {
                    id: "call-1".to_string(),
                    name: "read_file".to_string(),
                    args: json!({ "path": "src/lib.rs" }),
                }],
            ),
            LlmMessage::tool_result("call-1", "{\"ok\":true}", false),
        ],
        tools: vec![tool_definition()],
    };

    let payload = build_payload(&request);

    assert_eq!(payload["tool_choice"], "auto");
    assert_eq!(payload["tools"][0]["type"], "function");
    assert_eq!(payload["tools"][0]["function"]["name"], "read_file");
    assert_eq!(payload["messages"][1]["tool_calls"][0]["id"], "call-1");
    assert_eq!(payload["messages"][2]["role"], "tool");
    assert_eq!(payload["messages"][2]["tool_call_id"], "call-1");
}

#[test]
fn assembled_context_preserves_order_across_provider_payloads() {
    let mut timestamped_history = chat_message("user", "Earlier question");
    timestamped_history.created_at = Some(0);
    let mut timestamped_answer = chat_message("assistant", "Earlier answer");
    timestamped_answer.created_at = Some(1_000);
    let mut timestamped_current = chat_message("user", "Continue the edit");
    timestamped_current.created_at = Some(2_000);
    let mut context = ContextAssembler::assemble(ContextAssemblyInput {
        system_prompt: "System rules".to_string(),
        compaction_summary: None,
        messages: vec![timestamped_history, timestamped_answer, timestamped_current],
        skill_discovery: None,
        skill_activation: None,
        attachments: ContextAttachments::default(),
    })
    .unwrap();
    let group = ContextGroup::tool_exchange("tool-context-1");
    context.push(ContextItem::assistant(
        "",
        vec![LlmToolCall {
            id: "call-context-1".to_string(),
            name: "read_file".to_string(),
            args: json!({ "path": "src/lib.rs" }),
        }],
        ContextMetadata::new(
            ContextSource::ModelResponse,
            ContextScope::Run,
            ContextRetention::Retained,
        )
        .with_group(group.clone()),
    ));
    context.push(ContextItem::tool_result(
        "call-context-1",
        "file contents",
        false,
        ContextMetadata::new(
            ContextSource::ToolResult,
            ContextScope::Run,
            ContextRetention::Retained,
        )
        .with_group(group),
    ));

    let request = |api_style| LlmChatRequest {
        api_url: "https://example.test".to_string(),
        api_token: "token".to_string(),
        model: "model".to_string(),
        api_style,
        max_tokens: 1024,
        temperature: 0.2,
        stream: false,
        messages: context.to_messages(),
        tools: vec![tool_definition()],
    };

    let openai = build_payload(&request(AgentApiStyle::OpenAiCompatible));
    assert_eq!(openai["messages"][0]["role"], "system");
    let expected_timestamped_history = format!(
            "<backend_conversation_timing>\nuser_message_created_at: {}\n</backend_conversation_timing>\nEarlier question",
            format_message_created_at(0).unwrap()
        );
    let expected_timestamped_current = format!(
            "<backend_conversation_timing>\nprevious_assistant_message_created_at: {}\nuser_message_created_at: {}\n</backend_conversation_timing>\nContinue the edit",
            format_message_created_at(1_000).unwrap(),
            format_message_created_at(2_000).unwrap()
        );
    assert_eq!(
        openai["messages"][1]["content"],
        expected_timestamped_history
    );
    assert_eq!(openai["messages"][2]["content"], "Earlier answer");
    assert_eq!(
        openai["messages"][3]["content"],
        expected_timestamped_current
    );
    assert_eq!(
        openai["messages"][4]["tool_calls"][0]["id"],
        "call-context-1"
    );
    assert_eq!(openai["messages"][5]["role"], "tool");

    let anthropic = build_payload(&request(AgentApiStyle::AnthropicCompatible));
    assert_eq!(anthropic["system"], "System rules");
    assert_eq!(
        anthropic["messages"][0]["content"][0]["text"],
        expected_timestamped_history
    );
    assert_eq!(
        anthropic["messages"][1]["content"][0]["text"],
        "Earlier answer"
    );
    assert_eq!(
        anthropic["messages"][2]["content"][0]["text"],
        expected_timestamped_current
    );
    assert_eq!(
        anthropic["messages"][3]["content"][0]["id"],
        "call-context-1"
    );
    assert_eq!(
        anthropic["messages"][4]["content"][0]["tool_use_id"],
        "call-context-1"
    );
}

#[test]
fn conversation_trace_builds_legal_ordered_tool_history_for_both_providers() {
    let context = ContextAssembler::assemble(ContextAssemblyInput {
        system_prompt: "System rules".to_string(),
        compaction_summary: None,
        messages: vec![
            chat_message("user", "Inspect the file"),
            traced_chat_message("The file is valid."),
            chat_message("user", "What did you inspect?"),
        ],
        skill_discovery: None,
        skill_activation: None,
        attachments: ContextAttachments::default(),
    })
    .unwrap();
    context.validate_complete_tool_protocol().unwrap();

    let request = |api_style| LlmChatRequest {
        api_url: "https://example.test".to_string(),
        api_token: "token".to_string(),
        model: "model".to_string(),
        api_style,
        max_tokens: 1024,
        temperature: 0.2,
        stream: false,
        messages: context.to_messages(),
        tools: vec![tool_definition()],
    };

    let openai = build_payload(&request(AgentApiStyle::OpenAiCompatible));
    assert_eq!(openai["messages"][1]["content"], "Inspect the file");
    assert_eq!(
        openai["messages"][2]["content"],
        "I will inspect src/lib.rs."
    );
    let openai_call_id = openai["messages"][3]["tool_calls"][0]["id"]
        .as_str()
        .unwrap();
    assert!(openai_call_id.starts_with("conversation_trace_historical-run-1_"));
    assert_eq!(openai["messages"][3]["content"], Value::Null);
    assert_eq!(openai["messages"][4]["role"], "tool");
    assert_eq!(openai["messages"][4]["tool_call_id"], openai_call_id);
    assert!(openai["messages"][4]["content"]
        .as_str()
        .unwrap()
        .contains("\"ok\": true"));
    assert_eq!(openai["messages"][5]["content"], "The file is valid.");
    assert!(openai["messages"][6]["content"]
        .as_str()
        .unwrap()
        .contains("historical_agent_activity_terminal"));
    assert_eq!(openai["messages"][7]["content"], "What did you inspect?");

    let anthropic = build_payload(&request(AgentApiStyle::AnthropicCompatible));
    assert_eq!(anthropic["system"], "System rules");
    assert_eq!(anthropic["messages"][0]["role"], "user");
    assert_eq!(
        anthropic["messages"][1]["content"][0]["text"],
        "I will inspect src/lib.rs."
    );
    assert_eq!(anthropic["messages"][1]["content"][1]["type"], "tool_use");
    let anthropic_call_id = anthropic["messages"][1]["content"][1]["id"]
        .as_str()
        .unwrap();
    assert_eq!(anthropic_call_id, openai_call_id);
    assert_eq!(anthropic["messages"][2]["role"], "user");
    assert_eq!(
        anthropic["messages"][2]["content"][0]["type"],
        "tool_result"
    );
    assert_eq!(
        anthropic["messages"][2]["content"][0]["tool_use_id"],
        anthropic_call_id
    );
    assert_eq!(anthropic["messages"][2]["content"][0]["is_error"], false);
    assert_eq!(
        anthropic["messages"][3]["content"][0]["text"],
        "The file is valid."
    );
    assert!(anthropic["messages"][3]["content"][1]["text"]
        .as_str()
        .unwrap()
        .contains("historical_agent_activity_terminal"));
    assert_eq!(
        anthropic["messages"][4]["content"][0]["text"],
        "What did you inspect?"
    );
}

#[test]
fn builds_openai_image_messages_with_image_url_parts() {
    let mut image_message = message(LlmMessageRole::User, "Inspect this image.");
    image_message.images.push(LlmImage {
        mime_type: "image/png".to_string(),
        data_base64: "YWJj".to_string(),
    });
    let request = LlmChatRequest {
        api_url: "https://example.test/v1/chat/completions".to_string(),
        api_token: "token".to_string(),
        model: "gpt".to_string(),
        api_style: AgentApiStyle::OpenAiCompatible,
        max_tokens: 1024,
        temperature: 0.2,
        stream: false,
        messages: vec![image_message],
        tools: vec![tool_definition()],
    };

    let payload = build_payload(&request);

    assert_eq!(payload["messages"][0]["content"][0]["type"], "text");
    assert_eq!(payload["messages"][0]["content"][1]["type"], "image_url");
    assert!(payload["messages"][0]["content"][1].get("image").is_none());
}

#[test]
fn builds_anthropic_native_tool_payload_and_tool_result_messages() {
    let request = LlmChatRequest {
        api_url: "https://api.anthropic.com/v1/messages".to_string(),
        api_token: "token".to_string(),
        model: "claude".to_string(),
        api_style: AgentApiStyle::AnthropicCompatible,
        max_tokens: 1024,
        temperature: 0.2,
        stream: false,
        messages: vec![
            message(LlmMessageRole::User, "Read src/lib.rs"),
            LlmMessage::assistant(
                "",
                vec![LlmToolCall {
                    id: "toolu-1".to_string(),
                    name: "read_file".to_string(),
                    args: json!({ "path": "src/lib.rs" }),
                }],
            ),
            LlmMessage::tool_result("toolu-1", "{\"ok\":true}", false),
        ],
        tools: vec![tool_definition()],
    };

    let payload = build_payload(&request);

    assert_eq!(payload["tools"][0]["name"], "read_file");
    assert_eq!(payload["messages"][1]["content"][0]["type"], "tool_use");
    assert_eq!(payload["messages"][2]["role"], "user");
    assert_eq!(payload["messages"][2]["content"][0]["type"], "tool_result");
    assert_eq!(
        payload["messages"][2]["content"][0]["tool_use_id"],
        "toolu-1"
    );
}

#[test]
fn extracts_native_tool_calls() {
    let openai = json!({
        "choices": [{
            "message": {
                "tool_calls": [{
                    "id": "call-1",
                    "type": "function",
                    "function": {
                        "name": "read_file",
                        "arguments": "{\"path\":\"src/lib.rs\"}"
                    }
                }]
            }
        }]
    });
    let anthropic = json!({
        "content": [{
            "type": "tool_use",
            "id": "toolu-1",
            "name": "search_files",
            "input": { "query": "main" }
        }]
    });

    let openai_calls = extract_tool_calls(&openai, AgentApiStyle::OpenAiCompatible).unwrap();
    let anthropic_calls =
        extract_tool_calls(&anthropic, AgentApiStyle::AnthropicCompatible).unwrap();

    assert_eq!(openai_calls[0].id, "call-1");
    assert_eq!(openai_calls[0].name, "read_file");
    assert_eq!(openai_calls[0].args["path"], "src/lib.rs");
    assert_eq!(anthropic_calls[0].id, "toolu-1");
    assert_eq!(anthropic_calls[0].name, "search_files");
    assert_eq!(anthropic_calls[0].args["query"], "main");
}

#[test]
fn openai_stream_accumulates_text_and_tool_calls() {
    let mut accumulator = LlmStreamAccumulator::new(AgentApiStyle::OpenAiCompatible);
    let mut deltas = Vec::new();

    process_sse_frame(
        &format!(
            "data: {}\n\n",
            json!({ "choices": [{ "delta": { "content": "Hel" } }] })
        ),
        &mut accumulator,
        &mut |delta| deltas.push(delta),
    )
    .unwrap();
    process_sse_frame(
        &format!(
            "data: {}\n\n",
            json!({ "choices": [{ "delta": { "content": "lo" } }] })
        ),
        &mut accumulator,
        &mut |delta| deltas.push(delta),
    )
    .unwrap();
    process_sse_frame(
        &format!(
            "data: {}\n\n",
            json!({
                "choices": [{
                    "delta": {
                        "tool_calls": [{
                            "index": 0,
                            "id": "call-1",
                            "type": "function",
                            "function": {
                                "name": "read_file",
                                "arguments": "{\"path\""
                            }
                        }]
                    }
                }]
            })
        ),
        &mut accumulator,
        &mut |delta| deltas.push(delta),
    )
    .unwrap();
    process_sse_frame(
        &format!(
            "data: {}\n\n",
            json!({
                "choices": [{
                    "delta": {
                        "tool_calls": [{
                            "index": 0,
                            "function": {
                                "arguments": ":\"src/lib.rs\"}"
                            }
                        }]
                    },
                    "finish_reason": "tool_calls"
                }]
            })
        ),
        &mut accumulator,
        &mut |delta| deltas.push(delta),
    )
    .unwrap();

    let response = accumulator.finish().unwrap();
    assert_eq!(joined_text(&deltas), "Hello");
    assert!(deltas.iter().any(|event| matches!(
        event,
        LlmStreamEvent::ToolInputProgress {
            tool,
            received_bytes,
            ..
        }
            if tool == "read_file" && *received_bytes > 0
    )));
    assert_eq!(response.content, "Hello");
    assert_eq!(response.finish_reason, Some("tool_calls".to_string()));
    assert_eq!(response.tool_calls[0].id, "call-1");
    assert_eq!(response.tool_calls[0].name, "read_file");
    assert_eq!(response.tool_calls[0].args["path"], "src/lib.rs");
}

#[test]
fn anthropic_stream_accumulates_text_and_tool_calls() {
    let mut accumulator = LlmStreamAccumulator::new(AgentApiStyle::AnthropicCompatible);
    let mut deltas = Vec::new();

    process_sse_frame(
        &format!(
            "event: content_block_start\ndata: {}\n\n",
            json!({
                "type": "content_block_start",
                "index": 0,
                "content_block": { "type": "text", "text": "Hi" }
            })
        ),
        &mut accumulator,
        &mut |delta| deltas.push(delta),
    )
    .unwrap();
    process_sse_frame(
        &format!(
            "event: content_block_delta\ndata: {}\n\n",
            json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": { "type": "text_delta", "text": " there" }
            })
        ),
        &mut accumulator,
        &mut |delta| deltas.push(delta),
    )
    .unwrap();
    process_sse_frame(
        &format!(
            "event: content_block_start\ndata: {}\n\n",
            json!({
                "type": "content_block_start",
                "index": 1,
                "content_block": {
                    "type": "tool_use",
                    "id": "toolu-1",
                    "name": "search_files",
                    "input": {}
                }
            })
        ),
        &mut accumulator,
        &mut |delta| deltas.push(delta),
    )
    .unwrap();
    process_sse_frame(
        &format!(
            "event: content_block_delta\ndata: {}\n\n",
            json!({
                "type": "content_block_delta",
                "index": 1,
                "delta": {
                    "type": "input_json_delta",
                    "partial_json": "{\"query\":\"main\"}"
                }
            })
        ),
        &mut accumulator,
        &mut |delta| deltas.push(delta),
    )
    .unwrap();
    process_sse_frame(
        &format!(
            "event: message_delta\ndata: {}\n\n",
            json!({
                "type": "message_delta",
                "delta": { "stop_reason": "tool_use" },
                "usage": { "output_tokens": 8 }
            })
        ),
        &mut accumulator,
        &mut |delta| deltas.push(delta),
    )
    .unwrap();

    let response = accumulator.finish().unwrap();
    assert_eq!(joined_text(&deltas), "Hi there");
    assert!(deltas.iter().any(|event| matches!(
        event,
        LlmStreamEvent::ToolInputProgress {
            tool,
            received_bytes,
            ..
        }
            if tool == "search_files" && *received_bytes > 0
    )));
    assert_eq!(response.content, "Hi there");
    assert_eq!(response.finish_reason, Some("tool_use".to_string()));
    assert_eq!(response.usage.unwrap().output_tokens, Some(8));
    assert_eq!(response.tool_calls[0].id, "toolu-1");
    assert_eq!(response.tool_calls[0].name, "search_files");
    assert_eq!(response.tool_calls[0].args["query"], "main");
}

#[test]
fn extracts_openai_and_anthropic_usage() {
    let openai = json!({
        "usage": {
            "prompt_tokens": 7,
            "completion_tokens": 5,
            "total_tokens": 12,
            "prompt_tokens_details": {
                "cached_tokens": 2
            }
        }
    });
    let anthropic = json!({
        "usage": {
            "input_tokens": 3,
            "output_tokens": 4,
            "cache_read_input_tokens": 2,
            "cache_creation_input_tokens": 1
        }
    });

    let openai_usage = extract_usage(&openai).unwrap();
    assert_eq!(openai_usage.total_tokens, Some(12));
    assert_eq!(openai_usage.cached_input_tokens, Some(2));
    assert_eq!(openai_usage.billable_request_count, Some(1));

    let anthropic_usage = extract_usage(&anthropic).unwrap();
    assert_eq!(anthropic_usage.total_tokens, Some(7));
    assert_eq!(anthropic_usage.cached_input_tokens, Some(2));
    assert_eq!(anthropic_usage.cache_creation_input_tokens, Some(1));
    assert_eq!(anthropic_usage.billable_request_count, Some(1));
}
