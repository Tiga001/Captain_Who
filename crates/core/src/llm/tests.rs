use super::adapter::{deepseek_reasoning_content, ProviderAdapterRegistry};
use super::payload::build_payload;
use super::response::extract_tool_calls;
use super::transport::*;
use super::*;

#[test]
fn llm_debug_projections_never_expose_provider_or_tool_payloads() {
    const CANARY: &str = "LLM_DEBUG_SECRET_CANARY";
    let tool_call = LlmToolCall {
        id: "call-safe-id".to_string(),
        name: "mcp__fixture__echo".to_string(),
        args: json!({"neutral": CANARY}),
    };
    let message = LlmMessage::assistant(CANARY, vec![tool_call.clone()]);
    let request = LlmChatRequest {
        api_url: "https://example.test/v1/chat/completions".to_string(),
        api_token: CANARY.to_string(),
        provider_profile: generic_provider_profile(AgentApiStyle::OpenAiCompatible),
        provider_protocol: generic_provider_protocol(AgentApiStyle::OpenAiCompatible, "test-model"),
        max_tokens: 100,
        temperature: 0.0,
        stream: false,
        messages: vec![message.clone()],
        tools: Vec::new(),
    };
    let response = LlmChatResponse {
        assistant_turn: LlmAssistantTurn::from_legacy(CANARY, vec![tool_call.clone()]),
        usage: None,
        finish_reason: None,
    };
    let image = LlmImage {
        mime_type: "image/png".to_string(),
        data_base64: CANARY.to_string(),
    };
    let stream = LlmStreamEvent::ToolInputProgress {
        tool_call_index: 0,
        tool: "mcp__fixture__echo".to_string(),
        input_delta: CANARY.to_string(),
        received_bytes: CANARY.len() as u64,
    };

    let rendered = format!("{request:?}{response:?}{message:?}{image:?}{tool_call:?}{stream:?}");
    assert!(!rendered.contains(CANARY));
    assert!(!rendered.contains("example.test"));
}
use crate::context::{
    format_message_created_at, ContextAssembler, ContextAssemblyInput, ContextAttachments,
    ContextCapacityDetector, ContextFrame, ContextGroup, ContextItem, ContextMetadata,
    ContextRetention, ContextScope, ContextSource,
};
use crate::conversation_trace::{
    ConversationTraceToolResultStatus, ConversationTurnTrace, ConversationTurnTraceItem,
    ConversationTurnTraceTerminalStatus, CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
};
use crate::protocol::{AgentApprovalStatus, AgentChatMessage, AgentToolSafety};
use crate::provider_profile::{
    ProviderProfileRef, ReasoningEffort, ReasoningMode, ReasoningPolicy,
};
use crate::usage::extract_usage;
use crate::world_state::{
    WorldStateDiff, WorldStateLifetime, WorldStateSectionEnvelope, WorldStateSectionId,
    WorldStateSnapshot,
};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

fn message(role: LlmMessageRole, content: &str) -> LlmMessage {
    LlmMessage::text(role, content)
}

fn generic_provider_profile(api_style: AgentApiStyle) -> ProviderProfileConfig {
    ProviderProfileConfig::generic_for_dialect(api_style.into())
}

fn generic_provider_protocol(api_style: AgentApiStyle, model: &str) -> ProviderProtocolKey {
    let profile = generic_provider_profile(api_style);
    ProviderProtocolKey::new(api_style.into(), &profile, model, None).unwrap()
}

fn deepseek_provider_profile(
    mode: ReasoningMode,
    effort: ReasoningEffort,
) -> ProviderProfileConfig {
    let mut profile = ProviderProfileConfig::deepseek_v4_default();
    profile.reasoning = ReasoningPolicy { mode, effort };
    profile
}

fn deepseek_provider_protocol(profile: &ProviderProfileConfig, model: &str) -> ProviderProtocolKey {
    ProviderProtocolKey::new(AgentApiStyle::OpenAiCompatible.into(), profile, model, None).unwrap()
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

fn mcp_tool_definition() -> AgentToolDefinition {
    AgentToolDefinition {
        name: "mcp__fixture__add_numbers".to_string(),
        description: "MCP server: \"Fixture MCP\"\nMCP tool: \"add_numbers\"\nDescription: Add two fixture numbers.".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "left": { "type": "number" },
                "right": { "type": "number" }
            },
            "required": ["left", "right"],
            "additionalProperties": false
        }),
        safety: AgentToolSafety::ReadOnly,
        requires_workspace: false,
        requires_approval: false,
        approval_mode: crate::protocol::AgentToolApprovalMode::Never,
    }
}

fn request_with_messages(messages: Vec<LlmMessage>) -> LlmChatRequest {
    LlmChatRequest {
        api_url: "https://example.test/v1/chat/completions".to_string(),
        api_token: "token".to_string(),
        provider_profile: generic_provider_profile(AgentApiStyle::OpenAiCompatible),
        provider_protocol: generic_provider_protocol(AgentApiStyle::OpenAiCompatible, "gpt"),
        max_tokens: 1024,
        temperature: 0.2,
        stream: true,
        messages,
        tools: vec![tool_definition()],
    }
}

async fn read_test_http_request(stream: &mut TcpStream) {
    let _ = read_test_http_request_json(stream).await;
}

async fn read_test_http_request_json(stream: &mut TcpStream) -> Value {
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
                return serde_json::from_slice(&request[start..start + length]).unwrap();
            }
        }
    }
}

async fn write_test_http_response(stream: &mut TcpStream, status: &str, body: Value) {
    write_test_http_response_with_headers(stream, status, &[], body).await;
}

async fn write_test_http_response_with_headers(
    stream: &mut TcpStream,
    status: &str,
    extra_headers: &[(&str, &str)],
    body: Value,
) {
    let body = serde_json::to_vec(&body).unwrap();
    let extra_headers = extra_headers
        .iter()
        .map(|(name, value)| format!("{name}: {value}\r\n"))
        .collect::<String>();
    let headers = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\n{extra_headers}Content-Length: {}\r\nConnection: close\r\n\r\n",
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
        conversation_model_context_items: Vec::new(),
    }
}

fn traced_chat_message(content: &str) -> AgentChatMessage {
    let call_id = historical_trace_call_id();
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
                    call_id: call_id.clone(),
                    tool: "read_file".to_string(),
                    provenance: None,
                    operation: json!({ "path": "src/lib.rs", "startLine": 1 }),
                    approval_status: AgentApprovalStatus::NotRequired,
                    truncated: false,
                },
                ConversationTurnTraceItem::ToolResult {
                    sequence: 12,
                    call_id,
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
                    archive: Default::default(),
                },
            ],
        }),
        conversation_model_context_items: Vec::new(),
    }
}

fn historical_trace_call_id() -> String {
    model_response_tool_call_id("historical-run-1", 0, 0, "provider-call-1")
}

#[test]
fn retry_delay_uses_capped_exponential_backoff() {
    assert_eq!(retry_delay(1), Duration::from_millis(350));
    assert_eq!(retry_delay(2), Duration::from_millis(700));
    assert_eq!(retry_delay(10), Duration::from_millis(2_000));
}

#[test]
fn classifies_transient_llm_errors_as_retryable() {
    let transport =
        LlmProviderFailure::from_local_transport_failure("connection reset").to_agent_error();
    let rate_limited = LlmProviderFailure::from_http_response(
        AgentApiStyle::OpenAiCompatible,
        reqwest::StatusCode::TOO_MANY_REQUESTS,
        &reqwest::header::HeaderMap::new(),
        r#"{"error":{"code":"API_KEY_RATE_LIMIT_EXCEEDED"}}"#,
        "",
    )
    .to_agent_error();
    let overloaded = LlmProviderFailure::from_http_response(
        AgentApiStyle::OpenAiCompatible,
        reqwest::StatusCode::SERVICE_UNAVAILABLE,
        &reqwest::header::HeaderMap::new(),
        r#"{"error":{"type":"overloaded_error"}}"#,
        "",
    )
    .to_agent_error();

    assert!(is_retryable_llm_error(&transport));
    assert!(is_retryable_llm_error(&rate_limited));
    assert!(is_retryable_llm_error(&overloaded));
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
        provider_profile: generic_provider_profile(AgentApiStyle::OpenAiCompatible),
        provider_protocol: generic_provider_protocol(
            AgentApiStyle::OpenAiCompatible,
            "claude-opus-4-7",
        ),
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

    assert_eq!(response.content(), "recovered");
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

#[tokio::test]
async fn qizhen_429_retries_from_structured_code_and_recovers() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        for attempt in 1..=2 {
            let (mut stream, _) = listener.accept().await.unwrap();
            read_test_http_request(&mut stream).await;
            if attempt == 1 {
                write_test_http_response_with_headers(
                    &mut stream,
                    "429 Too Many Requests",
                    &[("Retry-After", "0"), ("X-Request-Id", "qizhen-fixture")],
                    json!({
                        "error": {
                            "code": "API_KEY_RATE_LIMIT_EXCEEDED",
                            "type": "RATE_LIMIT",
                            "message": "请求限流超限"
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
                        }]
                    }),
                )
                .await;
            }
        }
    });
    let mut request = request_with_messages(vec![message(LlmMessageRole::User, "Hello")]);
    request.api_url = format!("http://{address}/v1/chat/completions");
    request.api_token = "qizhen-retry-token".to_string();
    request.stream = false;

    let response = complete_chat(request, AgentCancellationToken::new())
        .await
        .unwrap();
    server.await.unwrap();
    assert_eq!(response.content(), "recovered");
    assert_eq!(
        response
            .usage
            .as_ref()
            .and_then(|usage| usage.billable_request_count),
        Some(2)
    );
}

#[tokio::test]
async fn cancellation_during_rate_limit_backoff_prevents_the_second_request() {
    const CANARY: &str = "RATE_LIMIT_BODY_SECRET_CANARY";
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_test_http_request(&mut stream).await;
        write_test_http_response_with_headers(
            &mut stream,
            "429 Too Many Requests",
            &[("Retry-After", "5")],
            json!({
                "error": {
                    "code": "API_KEY_RATE_LIMIT_EXCEEDED",
                    "message": CANARY
                }
            }),
        )
        .await;
        assert!(
            tokio::time::timeout(Duration::from_millis(250), listener.accept())
                .await
                .is_err()
        );
    });
    let mut request = request_with_messages(vec![message(LlmMessageRole::User, "Hello")]);
    request.api_url = format!("http://{address}/v1/chat/completions");
    request.api_token = "cancelled-rate-limit-token".to_string();
    let cancellation = AgentCancellationToken::new();
    let event_cancellation = cancellation.clone();
    let mut events = Vec::new();
    let started = std::time::Instant::now();

    let error = complete_chat_streaming(request, cancellation, |event| {
        if matches!(event, LlmStreamEvent::Retrying { .. }) {
            event_cancellation.cancel();
        }
        events.push(event);
    })
    .await
    .unwrap_err();
    server.await.unwrap();

    assert!(error.is_cancelled());
    assert!(started.elapsed() < Duration::from_millis(500));
    assert!(events.iter().all(|event| !matches!(
        event,
        LlmStreamEvent::AttemptReset { reason }
            | LlmStreamEvent::Retrying { reason, .. }
            if reason.contains(CANARY)
    )));
}

#[tokio::test]
async fn hard_quota_429_is_not_retried() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_test_http_request(&mut stream).await;
        write_test_http_response(
            &mut stream,
            "429 Too Many Requests",
            json!({
                "error": {
                    "code": "insufficient_quota",
                    "message": "billing quota exhausted"
                }
            }),
        )
        .await;
        assert!(
            tokio::time::timeout(Duration::from_millis(100), listener.accept())
                .await
                .is_err()
        );
    });
    let mut request = request_with_messages(vec![message(LlmMessageRole::User, "Hello")]);
    request.api_url = format!("http://{address}/v1/chat/completions");
    request.api_token = "hard-quota-token".to_string();
    request.stream = false;

    let error = complete_chat(request, AgentCancellationToken::new())
        .await
        .unwrap_err();
    server.await.unwrap();
    assert_eq!(error.code(), Some(PROVIDER_FAILURE_ERROR_CODE));
    assert_eq!(
        error
            .details()
            .and_then(|details| details.get("category"))
            .and_then(Value::as_str),
        Some("quota_exhausted")
    );
    assert!(!error.to_string().contains("billing quota exhausted"));
}

#[tokio::test]
async fn long_retry_after_returns_without_sending_a_second_request() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_test_http_request(&mut stream).await;
        write_test_http_response_with_headers(
            &mut stream,
            "429 Too Many Requests",
            &[("Retry-After", "120")],
            json!({"error":{"code":"API_KEY_RATE_LIMIT_EXCEEDED"}}),
        )
        .await;
        assert!(
            tokio::time::timeout(Duration::from_millis(100), listener.accept())
                .await
                .is_err()
        );
    });
    let mut request = request_with_messages(vec![message(LlmMessageRole::User, "Hello")]);
    request.api_url = format!("http://{address}/v1/chat/completions");
    request.api_token = "long-retry-after-token".to_string();
    request.stream = false;

    let error = complete_chat(request, AgentCancellationToken::new())
        .await
        .unwrap_err();
    server.await.unwrap();
    assert_eq!(error.code(), Some(PROVIDER_FAILURE_ERROR_CODE));
    assert_eq!(
        error
            .details()
            .and_then(|details| details.get("retryAfterMs"))
            .and_then(Value::as_u64),
        Some(120_000)
    );
    assert!(error.to_string().contains("暂时限流"));
}

#[tokio::test]
async fn broken_429_body_still_preserves_status_and_retry_after() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_test_http_request(&mut stream).await;
        stream
            .write_all(
                b"HTTP/1.1 429 Too Many Requests\r\nRetry-After: 120\r\nContent-Type: application/json\r\nContent-Length: 100\r\nConnection: close\r\n\r\n{",
            )
            .await
            .unwrap();
        stream.shutdown().await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(100), listener.accept())
                .await
                .is_err()
        );
    });
    let mut request = request_with_messages(vec![message(LlmMessageRole::User, "Hello")]);
    request.api_url = format!("http://{address}/v1/chat/completions");
    request.api_token = "broken-429-body-token".to_string();
    request.stream = false;

    let error = complete_chat(request, AgentCancellationToken::new())
        .await
        .unwrap_err();
    server.await.unwrap();
    assert_eq!(error.code(), Some(PROVIDER_FAILURE_ERROR_CODE));
    assert_eq!(
        error
            .details()
            .and_then(|details| details.get("category"))
            .and_then(Value::as_str),
        Some("rate_limited")
    );
    assert_eq!(
        error
            .details()
            .and_then(|details| details.get("retryAfterMs"))
            .and_then(Value::as_u64),
        Some(120_000)
    );
}

#[tokio::test]
async fn streaming_partial_output_is_never_transparently_replayed() {
    const CANARY: &str = "PARTIAL_STREAM_SECRET_CANARY";
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_test_http_request(&mut stream).await;
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
            )
            .await
            .unwrap();
        stream
            .write_all(
                format!(
                    "data: {}\n\ndata: {{not-json-{CANARY}\n\n",
                    json!({"choices":[{"delta":{"content":"partial"}}]})
                )
                .as_bytes(),
            )
            .await
            .unwrap();
    });
    let mut request = request_with_messages(vec![message(LlmMessageRole::User, "Hello")]);
    request.api_url = format!("http://{address}/v1/chat/completions");
    let mut events = Vec::new();

    let error = complete_chat_streaming(request, AgentCancellationToken::new(), |event| {
        events.push(event);
    })
    .await
    .unwrap_err();
    server.await.unwrap();

    assert!(events
        .iter()
        .any(|event| matches!(event, LlmStreamEvent::Delta(delta) if delta == "partial")));
    assert!(!events
        .iter()
        .any(|event| matches!(event, LlmStreamEvent::Retrying { .. })));
    assert!(events.iter().all(|event| !matches!(
        event,
        LlmStreamEvent::AttemptReset { reason } if reason.contains(CANARY)
    )));
    assert!(!error.to_string().contains(CANARY));
    assert!(!format!("{:?}", error.details()).contains(CANARY));
}

#[test]
fn rejects_incompatible_tool_schema_before_building_an_http_request() {
    let mut tool = tool_definition();
    tool.input_schema["anyOf"] = json!([{ "required": ["path"] }]);
    let request = LlmChatRequest {
        api_url: "https://example.test/v1/chat/completions".to_string(),
        api_token: "token".to_string(),
        provider_profile: generic_provider_profile(AgentApiStyle::OpenAiCompatible),
        provider_protocol: generic_provider_protocol(AgentApiStyle::OpenAiCompatible, "gpt"),
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
fn accepts_a_complete_tool_exchange_with_an_application_canonical_id() {
    let call_id = model_response_tool_call_id("active-run", 0, 0, "provider-call");
    let request = request_with_messages(vec![
        message(LlmMessageRole::User, "Read a file"),
        LlmMessage::assistant(
            "",
            vec![LlmToolCall {
                id: call_id.clone(),
                name: "read_file".to_string(),
                args: json!({ "path": "src/lib.rs" }),
            }],
        ),
        LlmMessage::tool_result(call_id, "{\"ok\":true}", false),
    ]);

    validate_request(&request).unwrap();
}

#[test]
fn rejects_noncanonical_tool_ids_at_or_below_the_provider_limit_before_network_io() {
    for call_id in ["call-1".to_string(), "a".repeat(64)] {
        let request = request_with_messages(vec![
            LlmMessage::assistant(
                "",
                vec![LlmToolCall {
                    id: call_id.clone(),
                    name: "read_file".to_string(),
                    args: json!({ "path": "src/lib.rs" }),
                }],
            ),
            LlmMessage::tool_result(call_id, "{\"ok\":true}", false),
        ]);

        let error = validate_request(&request).unwrap_err();

        assert_eq!(error.code(), Some("agent.invalid_model_tool_call_id"));
        assert_eq!(
            error.details().and_then(|details| details["code"].as_str()),
            Some("nonCanonicalToolCallId")
        );
        assert_eq!(
            error
                .details()
                .and_then(|details| details["canonicalLength"].as_u64()),
            Some(47)
        );
        assert_eq!(
            error
                .details()
                .and_then(|details| details["canonicalPrefix"].as_str()),
            Some("tc1_")
        );
    }
}

#[test]
fn rejects_tool_ids_over_sixty_four_bytes_before_network_io() {
    let call_id = "a".repeat(65);
    let request = request_with_messages(vec![
        LlmMessage::assistant(
            "",
            vec![LlmToolCall {
                id: call_id.clone(),
                name: "read_file".to_string(),
                args: json!({ "path": "src/lib.rs" }),
            }],
        ),
        LlmMessage::tool_result(call_id, "{\"ok\":true}", false),
    ]);

    let error = validate_request(&request).unwrap_err();

    assert_eq!(error.code(), Some("agent.invalid_model_tool_call_id"));
    assert_eq!(
        error.details().and_then(|details| details["code"].as_str()),
        Some("toolCallIdTooLong")
    );
    assert_eq!(
        error
            .details()
            .and_then(|details| details["maxLength"].as_u64()),
        Some(64)
    );
}

#[test]
fn rejects_unsafe_tool_id_characters_before_network_io() {
    let request = request_with_messages(vec![
        LlmMessage::assistant(
            "",
            vec![LlmToolCall {
                id: "unsafe:id".to_string(),
                name: "read_file".to_string(),
                args: json!({ "path": "src/lib.rs" }),
            }],
        ),
        LlmMessage::tool_result("unsafe:id", "{\"ok\":true}", false),
    ]);

    let error = validate_request(&request).unwrap_err();

    assert_eq!(error.code(), Some("agent.invalid_model_tool_call_id"));
    assert_eq!(
        error.details().and_then(|details| details["code"].as_str()),
        Some("unsafeToolCallIdCharacters")
    );
}

#[test]
fn rejects_duplicate_and_unpaired_tool_protocol_before_network_io() {
    let duplicate_call_id = model_response_tool_call_id("duplicate-run", 0, 0, "provider-call");
    let duplicate = request_with_messages(vec![
        LlmMessage::assistant(
            "",
            vec![
                LlmToolCall {
                    id: duplicate_call_id.clone(),
                    name: "read_file".to_string(),
                    args: json!({ "path": "src/one.rs" }),
                },
                LlmToolCall {
                    id: duplicate_call_id.clone(),
                    name: "read_file".to_string(),
                    args: json!({ "path": "src/two.rs" }),
                },
            ],
        ),
        LlmMessage::tool_result(duplicate_call_id, "{\"ok\":true}", false),
    ]);
    let duplicate_error = validate_request(&duplicate).unwrap_err();
    assert_eq!(
        duplicate_error.code(),
        Some("agent.invalid_model_tool_protocol")
    );
    assert_eq!(
        duplicate_error
            .details()
            .and_then(|details| details["code"].as_str()),
        Some("duplicateToolCallId")
    );

    let orphan_call_id = model_response_tool_call_id("orphan-run", 0, 0, "provider-call");
    let unpaired = request_with_messages(vec![LlmMessage::tool_result(
        orphan_call_id,
        "result",
        true,
    )]);
    let unpaired_error = validate_request(&unpaired).unwrap_err();
    assert_eq!(
        unpaired_error.code(),
        Some("agent.invalid_model_tool_protocol")
    );
    assert_eq!(
        unpaired_error
            .details()
            .and_then(|details| details["code"].as_str()),
        Some("unpairedToolResult")
    );
}

#[tokio::test]
async fn streaming_stop_without_text_or_tools_is_a_repairable_semantic_error() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_test_http_request(&mut stream).await;
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
            )
            .await
            .unwrap();
        let terminal = json!({
            "choices": [{
                "delta": {},
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 9,
                "completion_tokens": 2,
                "total_tokens": 11
            }
        });
        stream
            .write_all(format!("data: {terminal}\n\ndata: [DONE]\n\n").as_bytes())
            .await
            .unwrap();
    });
    let request = LlmChatRequest {
        api_url: format!("http://{address}/v1/chat/completions"),
        api_token: "token".to_string(),
        provider_profile: generic_provider_profile(AgentApiStyle::OpenAiCompatible),
        provider_protocol: generic_provider_protocol(AgentApiStyle::OpenAiCompatible, "test-model"),
        max_tokens: 1_024,
        temperature: 0.2,
        stream: true,
        messages: vec![message(LlmMessageRole::User, "Hello")],
        tools: Vec::new(),
    };
    let mut events = Vec::new();
    let error = complete_chat_streaming(request, AgentCancellationToken::new(), |event| {
        events.push(event);
    })
    .await
    .unwrap_err();
    server.await.unwrap();

    assert_eq!(error.code(), Some(EMPTY_MODEL_ACTION_ERROR_CODE));
    assert!(is_repairable_empty_model_action(&error));
    assert_eq!(
        error.usage().and_then(|usage| usage.billable_request_count),
        Some(1)
    );
    assert!(events
        .iter()
        .any(|event| matches!(event, LlmStreamEvent::AttemptReset { .. })));
    assert!(!events
        .iter()
        .any(|event| matches!(event, LlmStreamEvent::Retrying { .. })));
}

#[test]
fn retry_exhausted_error_mentions_retry_count() {
    let error = retry_exhausted_error(
        LlmProviderFailure::from_local_transport_failure("timeout secret diagnostic")
            .to_agent_error()
            .with_usage(Some(AgentUsage {
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
    assert!(!error.to_string().contains("secret diagnostic"));
    assert_eq!(error.code(), Some(PROVIDER_FAILURE_ERROR_CODE));
    assert_eq!(error.usage().unwrap().input_tokens, Some(7));
    assert_eq!(error.usage().unwrap().billable_request_count, Some(3));
}

#[test]
fn retry_plan_refuses_retry_after_beyond_the_total_sleep_budget() {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::RETRY_AFTER,
        reqwest::header::HeaderValue::from_static("120"),
    );
    let error = LlmProviderFailure::from_http_response(
        AgentApiStyle::OpenAiCompatible,
        reqwest::StatusCode::TOO_MANY_REQUESTS,
        &headers,
        r#"{"error":{"code":"API_KEY_RATE_LIMIT_EXCEEDED"}}"#,
        "",
    )
    .to_agent_error();

    assert!(retry_plan(&error, 1, Duration::ZERO, LLM_LOGICAL_REQUEST_TIMEOUT,).is_none());
}

#[test]
fn rate_limit_retry_plan_has_longer_bounded_delay_and_canonical_metadata() {
    let error = LlmProviderFailure::from_http_response(
        AgentApiStyle::OpenAiCompatible,
        reqwest::StatusCode::TOO_MANY_REQUESTS,
        &reqwest::header::HeaderMap::new(),
        r#"{"error":{"code":"API_KEY_RATE_LIMIT_EXCEEDED"}}"#,
        "",
    )
    .to_agent_error();
    let plan = retry_plan(&error, 1, Duration::ZERO, LLM_LOGICAL_REQUEST_TIMEOUT).unwrap();

    assert_eq!(plan.category, LlmProviderFailureCategory::RateLimited);
    assert_eq!(
        plan.provider_code.as_deref(),
        Some("api_key_rate_limit_exceeded")
    );
    assert_eq!(plan.max_attempts, LLM_MAX_ATTEMPTS);
    assert!(plan.delay >= Duration::from_millis(LLM_RATE_LIMIT_RETRY_BASE_DELAY_MS));
    assert!(plan.delay <= Duration::from_millis(2_500));
    assert!(plan.delay > retry_delay(1));
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
    let protocol = generic_provider_protocol(AgentApiStyle::OpenAiCompatible, "test-model");

    let strict =
        parse_non_stream_response(&body, &protocol, LlmResponseValidation::RequireModelAction);
    let deferred =
        parse_non_stream_response(&body, &protocol, LlmResponseValidation::AllowEmpty).unwrap();

    let strict = strict.unwrap_err();
    assert!(strict.to_string().contains("没有可显示文本"));
    assert_eq!(strict.code(), Some(EMPTY_MODEL_ACTION_ERROR_CODE));
    assert!(!is_repairable_empty_model_action(&strict));
    assert!(deferred.content().is_empty());
    assert_eq!(deferred.finish_reason.as_deref(), Some("length"));

    let normal_stop = json!({
        "choices": [{
            "message": { "role": "assistant", "content": "" },
            "finish_reason": "stop"
        }],
        // Response metadata must not trick the transport retry heuristic into replaying the
        // original empty request before the agent loop sends its one semantic repair request.
        "gatewayDiagnostic": "EMPTY_ACTION_SECRET_CANARY upstream timeout"
    })
    .to_string();
    let repairable = parse_non_stream_response(
        &normal_stop,
        &protocol,
        LlmResponseValidation::RequireModelAction,
    )
    .unwrap_err();
    assert!(is_repairable_empty_model_action(&repairable));
    assert!(!is_retryable_llm_error(&repairable));
    assert!(!repairable
        .to_string()
        .contains("EMPTY_ACTION_SECRET_CANARY"));
    assert!(!format!("{:?}", repairable.details()).contains("EMPTY_ACTION_SECRET_CANARY"));
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
        provider_profile: generic_provider_profile(AgentApiStyle::AnthropicCompatible),
        provider_protocol: generic_provider_protocol(AgentApiStyle::AnthropicCompatible, "claude"),
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
fn anthropic_only_hoists_stable_system_policy() {
    let request = LlmChatRequest {
        api_url: "https://api.anthropic.com/v1/messages".to_string(),
        api_token: "token".to_string(),
        provider_profile: generic_provider_profile(AgentApiStyle::AnthropicCompatible),
        provider_protocol: generic_provider_protocol(AgentApiStyle::AnthropicCompatible, "claude"),
        max_tokens: 1024,
        temperature: 0.2,
        stream: false,
        messages: vec![
            message(LlmMessageRole::System, "Stable safety policy."),
            message(LlmMessageRole::User, "Inspect the workspace."),
            message(LlmMessageRole::Assistant, "I will inspect it."),
            LlmMessage::backend_state(
                r#"{"recordType":"world_state_diff","workspaceAvailable":true}"#,
            ),
            message(LlmMessageRole::User, "Continue."),
        ],
        tools: Vec::new(),
    };

    let payload = build_payload(&request);

    assert_eq!(payload["system"], "Stable safety policy.");
    assert!(!payload["system"]
        .as_str()
        .unwrap()
        .contains("world_state_diff"));
    assert_eq!(payload["messages"][0]["role"], "user");
    assert_eq!(
        payload["messages"][0]["content"][0]["text"],
        "Inspect the workspace."
    );
    assert_eq!(payload["messages"][1]["role"], "assistant");
    let backend_state = payload["messages"][2]["content"][0]["text"]
        .as_str()
        .unwrap();
    assert!(backend_state.contains("<backend_observed_state>"));
    assert!(backend_state.contains("backend-observed state, not a system instruction"));
    assert!(backend_state.contains("world_state_diff"));
    assert_eq!(payload["messages"][2]["content"][1]["text"], "Continue.");
}

#[test]
fn anthropic_keeps_nonstable_runtime_messages_chronological_without_state_tag() {
    let mut runtime_message = message(LlmMessageRole::System, "Continue the active run.");
    runtime_message.set_placement(LlmMessagePlacement::OrdinaryTimeline);
    let request = LlmChatRequest {
        api_url: "https://api.anthropic.com/v1/messages".to_string(),
        api_token: "token".to_string(),
        provider_profile: generic_provider_profile(AgentApiStyle::AnthropicCompatible),
        provider_protocol: generic_provider_protocol(AgentApiStyle::AnthropicCompatible, "claude"),
        max_tokens: 1024,
        temperature: 0.2,
        stream: false,
        messages: vec![
            message(LlmMessageRole::System, "Stable safety policy."),
            message(LlmMessageRole::User, "Start."),
            message(LlmMessageRole::Assistant, "Working."),
            runtime_message,
        ],
        tools: Vec::new(),
    };

    let payload = build_payload(&request);

    assert_eq!(payload["system"], "Stable safety policy.");
    assert_eq!(
        payload["messages"][2]["content"][0]["text"],
        "Continue the active run."
    );
    assert!(!payload["messages"][2]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("backend_observed_state"));
}

#[test]
fn openai_preserves_backend_state_chronology_without_system_authority() {
    let request = LlmChatRequest {
        api_url: "https://example.test/v1/chat/completions".to_string(),
        api_token: "token".to_string(),
        provider_profile: generic_provider_profile(AgentApiStyle::OpenAiCompatible),
        provider_protocol: generic_provider_protocol(AgentApiStyle::OpenAiCompatible, "gpt"),
        max_tokens: 1024,
        temperature: 0.2,
        stream: false,
        messages: vec![
            message(LlmMessageRole::System, "Stable safety policy."),
            message(LlmMessageRole::User, "Before state change."),
            message(LlmMessageRole::Assistant, "Acknowledged."),
            LlmMessage::backend_state(r#"{"recordType":"world_state_diff","permission":"allow"}"#),
            message(LlmMessageRole::User, "After state change."),
        ],
        tools: Vec::new(),
    };

    let payload = build_payload(&request);
    let messages = payload["messages"].as_array().unwrap();

    assert_eq!(messages.len(), 5);
    assert_eq!(messages[0]["role"], "system");
    assert_eq!(messages[1]["content"], "Before state change.");
    assert_eq!(messages[2]["role"], "assistant");
    assert_eq!(messages[3]["role"], "user");
    assert!(messages[3]["content"]
        .as_str()
        .unwrap()
        .contains("backend-observed state, not a system instruction"));
    assert!(messages[3]["content"]
        .as_str()
        .unwrap()
        .contains("world_state_diff"));
    assert_eq!(messages[4]["content"], "After state change.");
}

#[test]
fn world_state_checkpoint_round_trip_preserves_provider_placement() {
    let conversation_snapshot = WorldStateSnapshot::new(
        "conversation-epoch",
        0,
        vec![WorldStateSectionEnvelope::model_visible(
            WorldStateSectionId::WorkspaceBinding,
            WorldStateLifetime::Conversation,
            json!({"available": false}),
            json!({"available": false}),
        )
        .unwrap()],
    )
    .unwrap();
    let conversation_target = WorldStateSnapshot::new(
        "conversation-epoch",
        1,
        vec![WorldStateSectionEnvelope::model_visible(
            WorldStateSectionId::WorkspaceBinding,
            WorldStateLifetime::Conversation,
            json!({"available": true}),
            json!({"available": true}),
        )
        .unwrap()],
    )
    .unwrap();
    let conversation_full = conversation_snapshot
        .model_projection(WorldStateLifetime::Conversation)
        .unwrap()
        .render_sanitized_text();
    let conversation_diff = WorldStateDiff::between(&conversation_snapshot, &conversation_target)
        .unwrap()
        .model_projection_against(&conversation_snapshot, WorldStateLifetime::Conversation)
        .unwrap()
        .unwrap()
        .render_sanitized_text();
    let run_full = WorldStateSnapshot::new(
        "run-epoch",
        0,
        vec![WorldStateSectionEnvelope::model_visible(
            WorldStateSectionId::EffectiveTools,
            WorldStateLifetime::Run,
            json!({"tools": ["read_file"]}),
            json!({"tools": ["read_file"]}),
        )
        .unwrap()],
    )
    .unwrap()
    .model_projection(WorldStateLifetime::Run)
    .unwrap()
    .render_sanitized_text();
    let frame = crate::context::ContextFrame::new(vec![
        ContextItem::text(
            LlmMessageRole::System,
            "Stable safety policy.",
            ContextSource::BackendSystemPrompt,
            ContextScope::Run,
            ContextRetention::Retained,
        ),
        ContextItem::text(
            LlmMessageRole::System,
            conversation_full,
            ContextSource::WorldStateSnapshot,
            ContextScope::Conversation,
            ContextRetention::Retained,
        ),
        ContextItem::text(
            LlmMessageRole::User,
            "Before state change.",
            ContextSource::ConversationHistory,
            ContextScope::Conversation,
            ContextRetention::Retained,
        ),
        ContextItem::text(
            LlmMessageRole::Assistant,
            "Acknowledged.",
            ContextSource::ConversationHistory,
            ContextScope::Conversation,
            ContextRetention::Retained,
        ),
        ContextItem::text(
            LlmMessageRole::System,
            conversation_diff,
            ContextSource::WorldStateDiff,
            ContextScope::Conversation,
            ContextRetention::Retained,
        ),
        ContextItem::text(
            LlmMessageRole::User,
            "After state change.",
            ContextSource::CurrentTurn,
            ContextScope::Conversation,
            ContextRetention::Retained,
        ),
        ContextItem::text(
            LlmMessageRole::System,
            run_full,
            ContextSource::WorldStateSnapshot,
            ContextScope::Run,
            ContextRetention::Retained,
        ),
    ]);
    frame.validate_cache_layout().unwrap();
    let restored =
        crate::context::ContextFrame::from_checkpoint_items(frame.checkpoint_items().unwrap())
            .unwrap();
    let restored_messages = restored.to_messages();

    assert_eq!(
        restored_messages[0].placement(),
        LlmMessagePlacement::StableSystemPolicy
    );
    assert_eq!(
        restored_messages[1].placement(),
        LlmMessagePlacement::BackendStateTimeline
    );
    assert_eq!(
        restored_messages[4].placement(),
        LlmMessagePlacement::BackendStateTimeline
    );
    assert_eq!(
        restored_messages[6].placement(),
        LlmMessagePlacement::BackendStateTimeline
    );

    let request = |api_style| LlmChatRequest {
        api_url: "https://example.test".to_string(),
        api_token: "token".to_string(),
        provider_profile: generic_provider_profile(api_style),
        provider_protocol: generic_provider_protocol(api_style, "model"),
        max_tokens: 1024,
        temperature: 0.2,
        stream: false,
        messages: restored_messages.clone(),
        tools: Vec::new(),
    };

    let openai = build_payload(&request(AgentApiStyle::OpenAiCompatible));
    assert_eq!(openai["messages"][0]["role"], "system");
    assert_eq!(openai["messages"][1]["role"], "user");
    assert!(openai["messages"][1]["content"]
        .as_str()
        .unwrap()
        .contains("\"lifetime\":\"conversation\""));
    assert_eq!(openai["messages"][2]["content"], "Before state change.");
    assert_eq!(openai["messages"][3]["role"], "assistant");
    assert_eq!(openai["messages"][4]["role"], "user");
    assert!(openai["messages"][4]["content"]
        .as_str()
        .unwrap()
        .contains("\"lifetime\":\"conversation\""));
    assert_eq!(openai["messages"][5]["content"], "After state change.");
    assert_eq!(openai["messages"][6]["role"], "user");
    assert!(openai["messages"][6]["content"]
        .as_str()
        .unwrap()
        .contains("\"lifetime\":\"run\""));

    let anthropic = build_payload(&request(AgentApiStyle::AnthropicCompatible));
    assert_eq!(anthropic["system"], "Stable safety policy.");
    assert!(!anthropic["system"]
        .as_str()
        .unwrap()
        .contains("world_state"));
    assert!(anthropic["messages"][0]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("\"lifetime\":\"conversation\""));
    assert_eq!(
        anthropic["messages"][0]["content"][1]["text"],
        "Before state change."
    );
    assert_eq!(anthropic["messages"][1]["role"], "assistant");
    assert!(anthropic["messages"][2]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("\"lifetime\":\"conversation\""));
    assert_eq!(
        anthropic["messages"][2]["content"][1]["text"],
        "After state change."
    );
    assert!(anthropic["messages"][2]["content"][2]["text"]
        .as_str()
        .unwrap()
        .contains("\"lifetime\":\"run\""));
}

#[test]
fn message_placement_distinguishes_stable_policy_from_timeline_state() {
    let stable = message(LlmMessageRole::System, "policy");
    let backend_state = LlmMessage::backend_state("state");
    let ordinary = message(LlmMessageRole::User, "request");

    assert_eq!(stable.placement(), LlmMessagePlacement::StableSystemPolicy);
    assert_eq!(
        backend_state.placement(),
        LlmMessagePlacement::BackendStateTimeline
    );
    assert_eq!(ordinary.placement(), LlmMessagePlacement::OrdinaryTimeline);
}

#[test]
fn omits_temperature_for_claude_models() {
    let request = LlmChatRequest {
        api_url: "https://example.test/v1/chat/completions".to_string(),
        api_token: "token".to_string(),
        provider_profile: generic_provider_profile(AgentApiStyle::OpenAiCompatible),
        provider_protocol: generic_provider_protocol(
            AgentApiStyle::OpenAiCompatible,
            "claude-opus-4-7",
        ),
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
        provider_profile: generic_provider_profile(AgentApiStyle::OpenAiCompatible),
        provider_protocol: generic_provider_protocol(AgentApiStyle::OpenAiCompatible, "gpt"),
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
fn builds_openai_mcp_namespace_tool_payload_without_internal_catalog_fields() {
    let definition = mcp_tool_definition();
    let expected_schema = definition.input_schema.clone();
    let request = LlmChatRequest {
        api_url: "https://example.test/v1/chat/completions".to_string(),
        api_token: "token".to_string(),
        provider_profile: generic_provider_profile(AgentApiStyle::OpenAiCompatible),
        provider_protocol: generic_provider_protocol(AgentApiStyle::OpenAiCompatible, "gpt"),
        max_tokens: 1024,
        temperature: 0.2,
        stream: false,
        messages: vec![message(LlmMessageRole::User, "Add the numbers")],
        tools: vec![definition],
    };

    let payload = build_payload(&request);
    let tool = &payload["tools"][0];

    assert_eq!(
        tool,
        &json!({
            "type": "function",
            "function": {
                "name": "mcp__fixture__add_numbers",
                "description": "MCP server: \"Fixture MCP\"\nMCP tool: \"add_numbers\"\nDescription: Add two fixture numbers.",
                "parameters": expected_schema
            }
        })
    );
    assert_eq!(tool["function"]["parameters"]["type"], "object");
    assert_eq!(tool["function"].as_object().unwrap().len(), 3);

    let encoded = serde_json::to_string(tool).unwrap();
    for internal_field in [
        "provenance",
        "outputSchema",
        "output_schema",
        "_meta",
        "serverId",
        "rawToolName",
        "configDigest",
        "catalogGeneration",
    ] {
        assert!(
            !encoded.contains(&format!("\"{internal_field}\"")),
            "OpenAI tool payload leaked internal MCP field {internal_field}: {encoded}"
        );
    }
}

#[test]
fn builds_anthropic_mcp_namespace_tool_payload_without_internal_catalog_fields() {
    let definition = mcp_tool_definition();
    let expected_schema = definition.input_schema.clone();
    let request = LlmChatRequest {
        api_url: "https://example.test/v1/messages".to_string(),
        api_token: "token".to_string(),
        provider_profile: generic_provider_profile(AgentApiStyle::AnthropicCompatible),
        provider_protocol: generic_provider_protocol(AgentApiStyle::AnthropicCompatible, "claude"),
        max_tokens: 1024,
        temperature: 0.2,
        stream: false,
        messages: vec![message(LlmMessageRole::User, "Add the numbers")],
        tools: vec![definition],
    };

    let payload = build_payload(&request);
    let tool = &payload["tools"][0];

    assert_eq!(
        tool,
        &json!({
            "name": "mcp__fixture__add_numbers",
            "description": "MCP server: \"Fixture MCP\"\nMCP tool: \"add_numbers\"\nDescription: Add two fixture numbers.",
            "input_schema": expected_schema
        })
    );
    assert_eq!(tool["input_schema"]["type"], "object");
    assert_eq!(tool.as_object().unwrap().len(), 3);

    let encoded = serde_json::to_string(tool).unwrap();
    for internal_field in [
        "provenance",
        "outputSchema",
        "output_schema",
        "_meta",
        "serverId",
        "rawToolName",
        "configDigest",
        "catalogGeneration",
    ] {
        assert!(
            !encoded.contains(&format!("\"{internal_field}\"")),
            "Anthropic tool payload leaked internal MCP field {internal_field}: {encoded}"
        );
    }
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
        world_state_records: Vec::new(),
        goal: None,
        initial_run_world_state: None,
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
        provider_profile: generic_provider_profile(api_style),
        provider_protocol: generic_provider_protocol(api_style, "model"),
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
        world_state_records: Vec::new(),
        goal: None,
        initial_run_world_state: None,
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
        provider_profile: generic_provider_profile(api_style),
        provider_protocol: generic_provider_protocol(api_style, "model"),
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
    assert_eq!(openai_call_id, historical_trace_call_id());
    assert_eq!(openai_call_id.len(), 47);
    assert!(openai_call_id.starts_with("tc1_"));
    assert_eq!(openai["messages"][3]["content"], Value::Null);
    assert_eq!(openai["messages"][4]["role"], "tool");
    assert_eq!(openai["messages"][4]["tool_call_id"], openai_call_id);
    assert!(!openai["messages"][4]["content"]
        .as_str()
        .unwrap()
        .contains("\"ok\""));
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
    assert_eq!(anthropic_call_id, historical_trace_call_id());
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
    image_message.images_mut().unwrap().push(LlmImage {
        mime_type: "image/png".to_string(),
        data_base64: "YWJj".to_string(),
    });
    let request = LlmChatRequest {
        api_url: "https://example.test/v1/chat/completions".to_string(),
        api_token: "token".to_string(),
        provider_profile: generic_provider_profile(AgentApiStyle::OpenAiCompatible),
        provider_protocol: generic_provider_protocol(AgentApiStyle::OpenAiCompatible, "gpt"),
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
        provider_profile: generic_provider_profile(AgentApiStyle::AnthropicCompatible),
        provider_protocol: generic_provider_protocol(AgentApiStyle::AnthropicCompatible, "claude"),
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
                    "id": " call-1 ",
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
            "id": " toolu-1 ",
            "name": "search_files",
            "input": { "query": "main" }
        }]
    });

    let openai_calls = extract_tool_calls(&openai, AgentApiStyle::OpenAiCompatible).unwrap();
    let anthropic_calls =
        extract_tool_calls(&anthropic, AgentApiStyle::AnthropicCompatible).unwrap();

    assert_eq!(openai_calls[0].id, " call-1 ");
    assert_eq!(openai_calls[0].name, "read_file");
    assert_eq!(openai_calls[0].args["path"], "src/lib.rs");
    assert_eq!(anthropic_calls[0].id, " toolu-1 ");
    assert_eq!(anthropic_calls[0].name, "search_files");
    assert_eq!(anthropic_calls[0].args["query"], "main");
}

#[test]
fn provider_tool_call_ids_preserve_the_byte_boundary_and_short_duplicates() {
    let protocol = generic_provider_protocol(AgentApiStyle::OpenAiCompatible, "gpt");
    let boundary_id = "é".repeat(MAX_PROVIDER_TOOL_CALL_ID_BYTES / "é".len());
    assert_eq!(boundary_id.len(), MAX_PROVIDER_TOOL_CALL_ID_BYTES);
    let duplicate_id = " provider-opaque-id ";
    let calls = vec![
        LlmToolCall {
            id: boundary_id.clone(),
            name: "read_file".to_string(),
            args: json!({}),
        },
        LlmToolCall {
            id: duplicate_id.to_string(),
            name: "read_file".to_string(),
            args: json!({}),
        },
        LlmToolCall {
            id: duplicate_id.to_string(),
            name: "read_file".to_string(),
            args: json!({}),
        },
    ];

    let turn = LlmAssistantTurn::from_provider(protocol.clone(), "", calls).unwrap();
    assert_eq!(turn.provider_tool_calls()[0].id, boundary_id);
    assert_eq!(turn.provider_tool_calls()[1].id, duplicate_id);
    assert_eq!(turn.provider_tool_calls()[2].id, duplicate_id);

    let oversized = LlmAssistantTurn::from_provider(
        protocol,
        "",
        vec![LlmToolCall {
            id: "x".repeat(MAX_PROVIDER_TOOL_CALL_ID_BYTES + 1),
            name: "read_file".to_string(),
            args: json!({}),
        }],
    )
    .unwrap_err();
    assert_eq!(
        oversized.code(),
        Some("agent.invalid_provider_tool_call_id")
    );

    let empty = LlmAssistantTurn::from_provider(
        generic_provider_protocol(AgentApiStyle::OpenAiCompatible, "gpt"),
        "",
        vec![LlmToolCall {
            id: String::new(),
            name: "read_file".to_string(),
            args: json!({}),
        }],
    )
    .unwrap_err();
    assert_eq!(empty.code(), Some("agent.invalid_provider_tool_call_id"));

    let forged_provider_call = LlmToolCall {
        id: "x".repeat(MAX_PROVIDER_TOOL_CALL_ID_BYTES + 1),
        name: "read_file".to_string(),
        args: json!({}),
    };
    let forged_binding = LlmRuntimeToolCallBinding::new(
        0,
        &forged_provider_call,
        LlmToolCall {
            id: model_response_tool_call_id("forged-checkpoint", 0, 0, "provider"),
            name: forged_provider_call.name.clone(),
            args: forged_provider_call.args.clone(),
        },
    );
    let forged = LlmAssistantTurn::from_legacy("", vec![forged_provider_call])
        .with_runtime_tool_bindings(vec![forged_binding])
        .unwrap_err();
    assert_eq!(forged.code(), Some("agent.invalid_provider_tool_call_id"));
}

#[test]
fn oversized_provider_tool_call_ids_fail_closed_nonstream_and_stream() {
    let oversized_id = "x".repeat(MAX_PROVIDER_TOOL_CALL_ID_BYTES + 1);
    let protocol = generic_provider_protocol(AgentApiStyle::OpenAiCompatible, "gpt");
    let nonstream = parse_non_stream_response(
        &json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "tool_calls": [{
                        "id": oversized_id,
                        "type": "function",
                        "function": {
                            "name": "read_file",
                            "arguments": "{}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        })
        .to_string(),
        &protocol,
        LlmResponseValidation::RequireModelAction,
    )
    .unwrap_err();
    assert_eq!(
        nonstream.code(),
        Some("agent.invalid_provider_tool_call_id")
    );

    let mut accumulator = LlmStreamAccumulator::new(AgentApiStyle::OpenAiCompatible);
    process_sse_frame(
        &format!(
            "data: {}\n\n",
            json!({
                "choices": [{
                    "delta": {
                        "tool_calls": [{
                            "index": 0,
                            "id": oversized_id,
                            "type": "function",
                            "function": {
                                "name": "read_file",
                                "arguments": "{}"
                            }
                        }]
                    },
                    "finish_reason": "tool_calls"
                }]
            })
        ),
        &mut accumulator,
        &mut |_| {},
    )
    .unwrap();
    let stream = accumulator.finish().unwrap_err();
    assert_eq!(stream.code(), Some("agent.invalid_provider_tool_call_id"));
}

#[test]
fn provider_checkpoint_identity_debug_is_redacted() {
    const CANARY: &str = "RAW_PROVIDER_CALL_ID_DEBUG_CANARY";
    let mapping = AgentProviderToolCallIdentity {
        provider_tool_index: 0,
        provider_call_id: CANARY.to_string(),
        runtime_call_id: "runtime-id".to_string(),
    };
    let identity = AgentAssistantTurnCheckpointIdentity {
        assistant_turn_id: "turn-id".to_string(),
        assistant_turn_digest: "digest".to_string(),
        tool_call_identities: vec![mapping.clone()],
    };

    assert!(!format!("{mapping:?}").contains(CANARY));
    assert!(!format!("{identity:?}").contains(CANARY));
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
    assert_eq!(response.content(), "Hello");
    assert_eq!(response.finish_reason, Some("tool_calls".to_string()));
    assert_eq!(response.provider_tool_calls()[0].id, "call-1");
    assert_eq!(response.provider_tool_calls()[0].name, "read_file");
    assert_eq!(response.provider_tool_calls()[0].args["path"], "src/lib.rs");
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
    assert_eq!(response.content(), "Hi there");
    assert_eq!(response.finish_reason, Some("tool_use".to_string()));
    assert_eq!(response.usage.as_ref().unwrap().output_tokens, Some(8));
    assert_eq!(response.provider_tool_calls()[0].id, "toolu-1");
    assert_eq!(response.provider_tool_calls()[0].name, "search_files");
    assert_eq!(response.provider_tool_calls()[0].args["query"], "main");
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

#[test]
fn generic_adapters_project_complete_multi_tool_turn_to_legacy_wire_order() {
    for api_style in [
        AgentApiStyle::OpenAiCompatible,
        AgentApiStyle::AnthropicCompatible,
    ] {
        let model = if api_style == AgentApiStyle::OpenAiCompatible {
            "gpt"
        } else {
            "claude"
        };
        let profile = generic_provider_profile(api_style);
        let protocol = generic_provider_protocol(api_style, model);
        let provider_calls = vec![
            LlmToolCall {
                id: "provider-call-1".to_string(),
                name: "read_file".to_string(),
                args: json!({"path":"a.txt"}),
            },
            LlmToolCall {
                id: "provider-call-2".to_string(),
                name: "read_file".to_string(),
                args: json!({"path":"b.txt"}),
            },
        ];
        let runtime_calls = [
            LlmToolCall {
                id: "runtime-call-1".to_string(),
                name: "read_file".to_string(),
                args: json!({"path":"a.txt"}),
            },
            LlmToolCall {
                id: "runtime-call-2".to_string(),
                name: "read_file".to_string(),
                args: json!({"path":"b.txt"}),
            },
        ];
        let bindings = provider_calls
            .iter()
            .zip(runtime_calls.iter().cloned())
            .enumerate()
            .map(|(index, (provider_call, runtime_call))| {
                LlmRuntimeToolCallBinding::new(index, provider_call, runtime_call)
            })
            .collect();
        let turn = LlmAssistantTurn::from_provider(
            protocol.clone(),
            "I will read both files.",
            provider_calls,
        )
        .unwrap()
        .with_runtime_tool_bindings(bindings)
        .unwrap();
        let request = LlmChatRequest {
            api_url: "https://example.test".to_string(),
            api_token: "token".to_string(),
            provider_profile: profile,
            provider_protocol: protocol,
            max_tokens: 1024,
            temperature: 0.2,
            stream: false,
            messages: vec![
                LlmMessage::from_assistant_turn(turn),
                LlmMessage::tool_result("runtime-call-1", "a", false),
                LlmMessage::tool_result("runtime-call-2", "b", false),
            ],
            tools: Vec::new(),
        };

        let payload = build_payload(&request);
        let messages = payload["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 4);
        assert_eq!(messages[0]["role"], "assistant");
        assert_eq!(messages[2]["role"], "assistant");
        if api_style == AgentApiStyle::OpenAiCompatible {
            assert_eq!(messages[0]["content"], "I will read both files.");
            assert_eq!(messages[0]["tool_calls"][0]["id"], "runtime-call-1");
            assert_eq!(messages[1]["role"], "tool");
            assert_eq!(messages[1]["tool_call_id"], "runtime-call-1");
            assert!(messages[2]["content"].is_null());
            assert_eq!(messages[2]["tool_calls"][0]["id"], "runtime-call-2");
            assert_eq!(messages[3]["tool_call_id"], "runtime-call-2");
        } else {
            assert_eq!(messages[0]["content"][0]["text"], "I will read both files.");
            assert_eq!(messages[0]["content"][1]["id"], "runtime-call-1");
            assert_eq!(messages[1]["content"][0]["tool_use_id"], "runtime-call-1");
            assert_eq!(messages[2]["content"][0]["id"], "runtime-call-2");
            assert_eq!(messages[3]["content"][0]["tool_use_id"], "runtime-call-2");
        }
    }
}

#[test]
fn generic_adapters_project_interleaved_image_and_runtime_extension_to_legal_wire() {
    for api_style in [
        AgentApiStyle::OpenAiCompatible,
        AgentApiStyle::AnthropicCompatible,
    ] {
        let model = if api_style == AgentApiStyle::OpenAiCompatible {
            "gpt"
        } else {
            "claude"
        };
        let profile = generic_provider_profile(api_style);
        let protocol = generic_provider_protocol(api_style, model);
        let provider_calls = vec![
            LlmToolCall {
                id: "provider-call-1".to_string(),
                name: "read_file".to_string(),
                args: json!({"path":"one.png"}),
            },
            LlmToolCall {
                id: "provider-call-2".to_string(),
                name: "read_file".to_string(),
                args: json!({"path":"two.txt"}),
            },
        ];
        let runtime_calls = provider_calls
            .iter()
            .enumerate()
            .map(|(index, provider_call)| LlmToolCall {
                id: model_response_tool_call_id(
                    "interleaved-projection-run",
                    0,
                    index,
                    &provider_call.id,
                ),
                name: provider_call.name.clone(),
                args: provider_call.args.clone(),
            })
            .collect::<Vec<_>>();
        let bindings = provider_calls
            .iter()
            .zip(runtime_calls.iter().cloned())
            .enumerate()
            .map(|(index, (provider_call, runtime_call))| {
                LlmRuntimeToolCallBinding::new(index, provider_call, runtime_call)
            })
            .collect();
        let turn = LlmAssistantTurn::from_provider(
            protocol.clone(),
            "I will inspect both files.",
            provider_calls,
        )
        .unwrap()
        .with_runtime_tool_bindings(bindings)
        .unwrap();
        let mut image_context = LlmMessage::text(
            LlmMessageRole::User,
            "Image emitted after the first tool result.",
        );
        image_context.images_mut().unwrap().push(LlmImage {
            mime_type: "image/png".to_string(),
            data_base64: "AA==".to_string(),
        });
        let runtime_extension = LlmMessage::text(
            LlmMessageRole::Assistant,
            "Runtime extension state updated.",
        );
        let request = LlmChatRequest {
            api_url: "https://example.test".to_string(),
            api_token: "token".to_string(),
            provider_profile: profile,
            provider_protocol: protocol,
            max_tokens: 1024,
            temperature: 0.2,
            stream: false,
            messages: vec![
                LlmMessage::from_assistant_turn(turn),
                LlmMessage::tool_result(runtime_calls[0].id.clone(), "first result", false),
                image_context,
                runtime_extension,
                LlmMessage::tool_result(runtime_calls[1].id.clone(), "second result", false),
            ],
            tools: Vec::new(),
        };

        assert!(validate_model_tool_protocol(&request.messages).is_err());
        validate_request(&request).unwrap();
        let payload = build_payload(&request);
        let messages = payload["messages"].as_array().unwrap();

        if api_style == AgentApiStyle::OpenAiCompatible {
            assert_eq!(messages.len(), 6);
            assert_eq!(messages[0]["tool_calls"][0]["id"], runtime_calls[0].id);
            assert_eq!(messages[1]["tool_call_id"], runtime_calls[0].id);
            assert_eq!(messages[2]["role"], "user");
            assert_eq!(messages[2]["content"][0]["type"], "text");
            assert_eq!(messages[2]["content"][1]["type"], "image_url");
            assert_eq!(messages[3]["content"], "Runtime extension state updated.");
            assert_eq!(messages[4]["tool_calls"][0]["id"], runtime_calls[1].id);
            assert_eq!(messages[5]["tool_call_id"], runtime_calls[1].id);
        } else {
            assert_eq!(messages.len(), 4);
            assert_eq!(messages[0]["content"][1]["id"], runtime_calls[0].id);
            assert_eq!(
                messages[1]["content"][0]["tool_use_id"],
                runtime_calls[0].id
            );
            assert_eq!(messages[1]["content"][1]["type"], "text");
            assert_eq!(messages[1]["content"][2]["type"], "image");
            assert_eq!(
                messages[2]["content"][0]["text"],
                "Runtime extension state updated."
            );
            assert_eq!(messages[2]["content"][1]["id"], runtime_calls[1].id);
            assert_eq!(
                messages[3]["content"][0]["tool_use_id"],
                runtime_calls[1].id
            );
        }
    }
}

#[test]
fn generic_adapters_preserve_legacy_grouped_multi_tool_wire_shape() {
    for api_style in [
        AgentApiStyle::OpenAiCompatible,
        AgentApiStyle::AnthropicCompatible,
    ] {
        let model = if api_style == AgentApiStyle::OpenAiCompatible {
            "gpt"
        } else {
            "claude"
        };
        let calls = vec![
            LlmToolCall {
                id: "call-1".to_string(),
                name: "read_file".to_string(),
                args: json!({"path":"a.txt"}),
            },
            LlmToolCall {
                id: "call-2".to_string(),
                name: "read_file".to_string(),
                args: json!({"path":"b.txt"}),
            },
        ];
        let request = LlmChatRequest {
            api_url: "https://example.test".to_string(),
            api_token: "token".to_string(),
            provider_profile: generic_provider_profile(api_style),
            provider_protocol: generic_provider_protocol(api_style, model),
            max_tokens: 1024,
            temperature: 0.2,
            stream: false,
            messages: vec![
                LlmMessage::assistant("I will read both files.", calls),
                LlmMessage::tool_result("call-1", "a", false),
                LlmMessage::tool_result("call-2", "b", false),
            ],
            tools: Vec::new(),
        };

        let payload = build_payload(&request);
        let messages = payload["messages"].as_array().unwrap();
        if api_style == AgentApiStyle::OpenAiCompatible {
            assert_eq!(messages.len(), 3);
            assert_eq!(messages[0]["content"], "I will read both files.");
            assert_eq!(messages[0]["tool_calls"].as_array().unwrap().len(), 2);
            assert_eq!(messages[0]["tool_calls"][0]["id"], "call-1");
            assert_eq!(messages[0]["tool_calls"][1]["id"], "call-2");
            assert_eq!(messages[1]["tool_call_id"], "call-1");
            assert_eq!(messages[2]["tool_call_id"], "call-2");
        } else {
            assert_eq!(messages.len(), 2);
            assert_eq!(messages[0]["content"][0]["text"], "I will read both files.");
            assert_eq!(messages[0]["content"][1]["id"], "call-1");
            assert_eq!(messages[0]["content"][2]["id"], "call-2");
            assert_eq!(messages[1]["content"][0]["tool_use_id"], "call-1");
            assert_eq!(messages[1]["content"][1]["tool_use_id"], "call-2");
        }
    }
}

#[test]
fn adapter_registry_selects_deepseek_only_for_the_explicit_profile() {
    let provider_profile = ProviderProfileConfig::deepseek_v4_default();
    let provider_protocol = ProviderProtocolKey::new(
        AgentApiStyle::OpenAiCompatible.into(),
        &provider_profile,
        "deepseek-v4",
        None,
    )
    .unwrap();
    let request = LlmChatRequest {
        api_url: "https://example.test/v1/chat/completions".to_string(),
        api_token: "token".to_string(),
        provider_profile,
        provider_protocol,
        max_tokens: 100,
        temperature: 0.2,
        stream: false,
        messages: vec![LlmMessage::text(LlmMessageRole::User, "hello")],
        tools: Vec::new(),
    };

    let adapter = ProviderAdapterRegistry::resolve(&request).unwrap();
    assert_eq!(adapter.profile(), ProviderProfileRef::deepseek_v4_chat());

    let mut mismatched = request;
    mismatched.provider_profile = generic_provider_profile(AgentApiStyle::OpenAiCompatible);
    assert!(ProviderAdapterRegistry::resolve(&mismatched).is_err());
}

#[test]
fn deepseek_disabled_reasoning_maps_without_generic_tool_choice() {
    let provider_profile =
        deepseek_provider_profile(ReasoningMode::Disabled, ReasoningEffort::ProviderDefault);
    let provider_protocol = deepseek_provider_protocol(&provider_profile, "deepseek-v4-flash");
    let request = LlmChatRequest {
        api_url: "https://api.deepseek.test/chat/completions".to_string(),
        api_token: "token".to_string(),
        provider_profile,
        provider_protocol,
        max_tokens: 512,
        temperature: 0.7,
        stream: false,
        messages: vec![LlmMessage::text(LlmMessageRole::User, "hello")],
        tools: vec![tool_definition()],
    };

    let payload = build_payload(&request);
    assert_eq!(payload["thinking"], json!({ "type": "disabled" }));
    assert!((payload["temperature"].as_f64().unwrap() - 0.7).abs() < f64::from(f32::EPSILON));
    assert!(payload.get("reasoning_effort").is_none());
    assert!(payload.get("tool_choice").is_none());
    assert_eq!(payload["tools"][0]["function"]["name"], "read_file");
}

#[test]
fn deepseek_nonstream_preserves_a_present_empty_reasoning_field() {
    let provider_profile = deepseek_provider_profile(ReasoningMode::Enabled, ReasoningEffort::High);
    let provider_protocol = deepseek_provider_protocol(&provider_profile, "deepseek-v4-flash");
    let response = parse_non_stream_response(
        &json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "Visible.",
                    "reasoning_content": ""
                },
                "finish_reason": "stop"
            }]
        })
        .to_string(),
        &provider_protocol,
        LlmResponseValidation::RequireModelAction,
    )
    .unwrap();

    assert_eq!(response.content(), "Visible.");
    assert!(response.assistant_turn.reasoning().is_empty());
    assert_eq!(
        deepseek_reasoning_content(&provider_protocol, &response.assistant_turn).unwrap(),
        Some("")
    );
    assert_eq!(
        response
            .assistant_turn
            .provider_continuation()
            .unwrap()
            .encoded_bytes(),
        1
    );
}

#[test]
fn deepseek_disabled_tool_response_without_reasoning_preserves_absence() {
    let provider_profile =
        deepseek_provider_profile(ReasoningMode::Disabled, ReasoningEffort::ProviderDefault);
    let provider_protocol = deepseek_provider_protocol(&provider_profile, "deepseek-v4-flash");
    let response = parse_non_stream_response_with_profile(
        &json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "",
                    "tool_calls": [{
                        "id": "non-thinking-deepseek-call",
                        "type": "function",
                        "function": {
                            "name": "read_file",
                            "arguments": "{\"path\":\"src/lib.rs\"}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        })
        .to_string(),
        &provider_profile,
        &provider_protocol,
        LlmResponseValidation::RequireModelAction,
    )
    .unwrap();

    assert!(response.assistant_turn.reasoning().is_empty());
    assert!(response.assistant_turn.provider_continuation().is_none());
    assert_eq!(
        deepseek_reasoning_content(&provider_protocol, &response.assistant_turn).unwrap(),
        None
    );
}

#[test]
fn deepseek_enabled_tool_response_missing_reasoning_fails_nonstream_and_stream() {
    let provider_profile = deepseek_provider_profile(ReasoningMode::Enabled, ReasoningEffort::High);
    let provider_protocol = deepseek_provider_protocol(&provider_profile, "deepseek-v4-pro");
    let missing_reasoning_response = json!({
        "choices": [{
            "message": {
                "role": "assistant",
                "content": "",
                "tool_calls": [{
                    "id": "missing-reasoning-call",
                    "type": "function",
                    "function": {
                        "name": "read_file",
                        "arguments": "{\"path\":\"src/lib.rs\"}"
                    }
                }]
            },
            "finish_reason": "tool_calls"
        }]
    });
    let nonstream_error = parse_non_stream_response_with_profile(
        &missing_reasoning_response.to_string(),
        &provider_profile,
        &provider_protocol,
        LlmResponseValidation::RequireModelAction,
    )
    .unwrap_err();
    assert_eq!(nonstream_error.code(), Some("provider_reasoning_required"));

    let mut accumulator =
        LlmStreamAccumulator::for_profile(&provider_profile, &provider_protocol).unwrap();
    let missing_reasoning_stream_frame = json!({
        "choices": [{
            "delta": {
                "tool_calls": [{
                    "index": 0,
                    "id": "missing-reasoning-call",
                    "type": "function",
                    "function": {
                        "name": "read_file",
                        "arguments": "{\"path\":\"src/lib.rs\"}"
                    }
                }]
            },
            "finish_reason": "tool_calls"
        }]
    });
    process_sse_frame(
        &format!("data: {missing_reasoning_stream_frame}\n\n"),
        &mut accumulator,
        &mut |_| {},
    )
    .unwrap();
    let stream_error = accumulator.finish().unwrap_err();
    assert_eq!(stream_error.code(), Some("provider_reasoning_required"));
}

#[test]
fn deepseek_provider_default_preserves_missing_and_explicit_empty_reasoning() {
    let provider_profile = ProviderProfileConfig::deepseek_v4_default();
    let provider_protocol = deepseek_provider_protocol(&provider_profile, "deepseek-v4-default");
    let response = |reasoning_content: Option<&str>| {
        let mut message = json!({
            "role": "assistant",
            "content": "",
            "tool_calls": [{
                "id": "default-reasoning-call",
                "type": "function",
                "function": {
                    "name": "read_file",
                    "arguments": "{\"path\":\"src/lib.rs\"}"
                }
            }]
        });
        if let Some(reasoning_content) = reasoning_content {
            message["reasoning_content"] = json!(reasoning_content);
        }
        parse_non_stream_response_with_profile(
            &json!({
                "choices": [{
                    "message": message,
                    "finish_reason": "tool_calls"
                }]
            })
            .to_string(),
            &provider_profile,
            &provider_protocol,
            LlmResponseValidation::RequireModelAction,
        )
        .unwrap()
    };

    let missing = response(None);
    assert!(missing.assistant_turn.provider_continuation().is_none());
    assert_eq!(
        deepseek_reasoning_content(&provider_protocol, &missing.assistant_turn).unwrap(),
        None
    );

    let explicit_empty = response(Some(""));
    assert!(explicit_empty
        .assistant_turn
        .provider_continuation()
        .is_some());
    assert_eq!(
        deepseek_reasoning_content(&provider_protocol, &explicit_empty.assistant_turn).unwrap(),
        Some("")
    );
}

#[test]
fn deepseek_disabled_reasoning_supports_multiple_tool_rounds_without_continuations() {
    let provider_profile =
        deepseek_provider_profile(ReasoningMode::Disabled, ReasoningEffort::ProviderDefault);
    let provider_protocol = deepseek_provider_protocol(&provider_profile, "deepseek-v4-disabled");
    let parse_turn = |provider_call_id: &str| {
        parse_non_stream_response_with_profile(
            &json!({
                "choices": [{
                    "message": {
                        "role": "assistant",
                        "content": "",
                        "tool_calls": [{
                            "id": provider_call_id,
                            "type": "function",
                            "function": {
                                "name": "read_file",
                                "arguments": "{\"path\":\"src/lib.rs\"}"
                            }
                        }]
                    },
                    "finish_reason": "tool_calls"
                }]
            })
            .to_string(),
            &provider_profile,
            &provider_protocol,
            LlmResponseValidation::RequireModelAction,
        )
        .unwrap()
        .assistant_turn
    };
    let bind_turn = |turn: LlmAssistantTurn, request_index: usize| {
        let provider_call = turn.provider_tool_calls()[0].clone();
        let runtime_call = LlmToolCall {
            id: model_response_tool_call_id(
                "deepseek-disabled-run",
                request_index,
                0,
                &provider_call.id,
            ),
            name: provider_call.name.clone(),
            args: provider_call.args.clone(),
        };
        (
            turn.with_runtime_tool_bindings(vec![LlmRuntimeToolCallBinding::new(
                0,
                &provider_call,
                runtime_call.clone(),
            )])
            .unwrap(),
            runtime_call,
        )
    };
    let (first_turn, first_runtime_call) = bind_turn(parse_turn("disabled-provider-1"), 0);
    let (second_turn, second_runtime_call) = bind_turn(parse_turn("disabled-provider-2"), 1);
    assert!(first_turn.provider_continuation().is_none());
    assert!(second_turn.provider_continuation().is_none());

    let request = LlmChatRequest {
        api_url: "https://api.deepseek.test/chat/completions".to_string(),
        api_token: "token".to_string(),
        provider_profile,
        provider_protocol,
        max_tokens: 512,
        temperature: 0.7,
        stream: false,
        messages: vec![
            LlmMessage::text(LlmMessageRole::User, "inspect"),
            LlmMessage::from_assistant_turn(first_turn),
            LlmMessage::tool_result(first_runtime_call.id, "first", false),
            LlmMessage::from_assistant_turn(second_turn),
            LlmMessage::tool_result(second_runtime_call.id, "second", false),
        ],
        tools: vec![tool_definition()],
    };
    let payload = build_payload(&request);
    let tool_turns = payload["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["role"] == "assistant" && message.get("tool_calls").is_some())
        .collect::<Vec<_>>();
    assert_eq!(tool_turns.len(), 2);
    assert!(tool_turns
        .iter()
        .all(|message| message.get("reasoning_content").is_none()));
    assert_eq!(tool_turns[0]["tool_calls"][0]["id"], "disabled-provider-1");
    assert_eq!(tool_turns[1]["tool_calls"][0]["id"], "disabled-provider-2");
}

#[test]
fn deepseek_stream_captures_reasoning_without_emitting_it_as_visible_text() {
    let provider_profile = deepseek_provider_profile(ReasoningMode::Enabled, ReasoningEffort::High);
    let provider_protocol = deepseek_provider_protocol(&provider_profile, "deepseek-v4-pro");
    let mut accumulator = LlmStreamAccumulator::for_protocol(&provider_protocol).unwrap();
    let mut events = Vec::new();

    for frame in [
        json!({
            "choices": [{ "delta": { "reasoning_content": "private " } }]
        }),
        json!({
            "choices": [{
                "delta": {
                    "reasoning_content": "reasoning",
                    "content": "Checking. ",
                    "tool_calls": [{
                        "index": 0,
                        "id": "deepseek-provider-call-1",
                        "function": {
                            "name": "read_file",
                            "arguments": "{\"path\":\"src/lib.rs\"}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        }),
        json!({
            "choices": [],
            "usage": {
                "prompt_tokens": 11,
                "completion_tokens": 7,
                "total_tokens": 18,
                "prompt_cache_hit_tokens": 3,
                "prompt_cache_miss_tokens": 8,
                "completion_tokens_details": { "reasoning_tokens": 5 }
            }
        }),
    ] {
        process_sse_frame(
            &format!("data: {frame}\n\n"),
            &mut accumulator,
            &mut |event| events.push(event),
        )
        .unwrap();
    }

    let response = accumulator.finish().unwrap();
    assert_eq!(joined_text(&events), "Checking. ");
    assert!(events.iter().all(|event| !matches!(
        event,
        LlmStreamEvent::Delta(delta) if delta.contains("private") || delta.contains("reasoning")
    )));
    assert_eq!(
        response.provider_tool_calls()[0].id,
        "deepseek-provider-call-1"
    );
    assert!(response.assistant_turn.reasoning().is_empty());
    assert_eq!(
        deepseek_reasoning_content(&provider_protocol, &response.assistant_turn).unwrap(),
        Some("private reasoning")
    );
    assert_eq!(
        response
            .assistant_turn
            .provider_continuation()
            .unwrap()
            .replay_scope(),
        ProviderContinuationReplayScope::InteractionV1
    );
    let usage = response.usage.unwrap();
    assert_eq!(usage.input_tokens, Some(11));
    assert_eq!(usage.output_tokens, Some(2));
    assert_eq!(usage.output_thinking_tokens, Some(5));
    assert_eq!(usage.total_tokens, Some(18));
    assert_eq!(usage.cached_input_tokens, Some(3));
    assert_eq!(usage.cache_creation_input_tokens, Some(8));
}

#[test]
fn deepseek_usage_fails_closed_when_visible_output_cannot_be_split() {
    let profile = deepseek_provider_profile(ReasoningMode::Enabled, ReasoningEffort::High);
    let protocol = deepseek_provider_protocol(&profile, "deepseek-v4-pro");

    let missing_details = parse_non_stream_response(
        &json!({
            "choices": [{
                "message": { "role": "assistant", "content": "visible" },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 7,
                "total_tokens": 17
            }
        })
        .to_string(),
        &protocol,
        LlmResponseValidation::RequireModelAction,
    )
    .unwrap()
    .usage
    .unwrap();
    assert_eq!(missing_details.output_tokens, None);
    assert_eq!(missing_details.output_thinking_tokens, None);
    assert_eq!(missing_details.total_tokens, Some(17));

    let inconsistent_details = parse_non_stream_response(
        &json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "visible",
                    "reasoning_content": "private"
                },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 3,
                "completion_tokens_details": { "reasoning_tokens": 5 },
                "total_tokens": 13
            }
        })
        .to_string(),
        &protocol,
        LlmResponseValidation::RequireModelAction,
    )
    .unwrap()
    .usage
    .unwrap();
    assert_eq!(inconsistent_details.output_tokens, None);
    assert_eq!(inconsistent_details.output_thinking_tokens, Some(5));
    assert_eq!(inconsistent_details.total_tokens, Some(13));

    let generic_protocol =
        generic_provider_protocol(AgentApiStyle::OpenAiCompatible, "generic-openai");
    let generic = parse_non_stream_response(
        &json!({
            "choices": [{
                "message": { "role": "assistant", "content": "visible" },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 7,
                "completion_tokens_details": { "reasoning_tokens": 5 },
                "total_tokens": 17
            }
        })
        .to_string(),
        &generic_protocol,
        LlmResponseValidation::RequireModelAction,
    )
    .unwrap()
    .usage
    .unwrap();
    assert_eq!(generic.output_tokens, Some(7));
    assert_eq!(generic.output_thinking_tokens, Some(5));
    assert_eq!(generic.total_tokens, Some(17));
}

#[test]
fn deepseek_continuation_estimate_measures_replayed_json_and_blocks_local_overflow() {
    let profile = deepseek_provider_profile(ReasoningMode::Enabled, ReasoningEffort::Max);
    let protocol = deepseek_provider_protocol(&profile, "deepseek-v4-pro");
    let reasoning = "大段中文 reasoning：\n\"quoted\" \\\\ slash\t".repeat(1_500);
    let response = parse_non_stream_response(
        &json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "",
                    "reasoning_content": reasoning,
                    "tool_calls": [{
                        "id": "deepseek-estimate-provider-call",
                        "type": "function",
                        "function": {
                            "name": "read_file",
                            "arguments": "{\"path\":\"large.txt\"}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        })
        .to_string(),
        &protocol,
        LlmResponseValidation::RequireModelAction,
    )
    .unwrap();
    let provider_call = response.provider_tool_calls()[0].clone();
    let runtime_call = LlmToolCall {
        id: model_response_tool_call_id("deepseek-estimate-run", 0, 0, &provider_call.id),
        name: provider_call.name.clone(),
        args: provider_call.args.clone(),
    };
    let turn = response
        .assistant_turn
        .with_runtime_tool_bindings(vec![LlmRuntimeToolCallBinding::new(
            0,
            &provider_call,
            runtime_call.clone(),
        )])
        .unwrap();
    let estimated = estimate_assistant_turn_continuation_tokens(&turn).unwrap();
    assert!(
        estimated > u64::try_from(reasoning.len()).unwrap().div_ceil(4),
        "Unicode and JSON escaping must not fall back to opaque bytes/4"
    );

    let mut frame = ContextFrame::new(vec![
        ContextItem::new(
            LlmMessage::from_assistant_turn(turn),
            ContextMetadata::new(
                ContextSource::ModelResponse,
                ContextScope::Run,
                ContextRetention::Retained,
            ),
        ),
        ContextItem::tool_result(
            runtime_call.id,
            "result",
            false,
            ContextMetadata::new(
                ContextSource::ToolResult,
                ContextScope::Run,
                ContextRetention::Retained,
            ),
        ),
    ]);
    let detector =
        ContextCapacityDetector::for_model("deepseek-v4-pro", AgentApiStyle::OpenAiCompatible, &[]);
    let report = detector.inspect(&mut frame, Some(4_096), 512);
    assert!(report.usage.breakdown.total.provider_continuation_tokens >= estimated);
    let error = detector.ensure_sendable(report).unwrap_err();
    assert_eq!(error.code(), Some("context_capacity_exceeded"));
    assert!(error.to_string().contains("请求尚未发送"));
}

#[test]
fn deepseek_groups_tool_history_and_moves_interstitial_context_after_results() {
    let provider_profile = deepseek_provider_profile(ReasoningMode::Enabled, ReasoningEffort::High);
    let provider_protocol = deepseek_provider_protocol(&provider_profile, "deepseek-v4-pro");
    let response = parse_non_stream_response(
        &json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "",
                    "reasoning_content": "Both files are required.",
                    "tool_calls": [
                        {
                            "id": "deepseek-raw-a",
                            "type": "function",
                            "function": {
                                "name": "read_file",
                                "arguments": "{\"path\":\"a.png\"}"
                            }
                        },
                        {
                            "id": "deepseek-raw-b",
                            "type": "function",
                            "function": {
                                "name": "read_file",
                                "arguments": "{\"path\":\"b.txt\"}"
                            }
                        }
                    ]
                },
                "finish_reason": "tool_calls"
            }]
        })
        .to_string(),
        &provider_protocol,
        LlmResponseValidation::RequireModelAction,
    )
    .unwrap();
    let provider_calls = response.provider_tool_calls().to_vec();
    let runtime_calls = provider_calls
        .iter()
        .enumerate()
        .map(|(index, call)| LlmToolCall {
            id: model_response_tool_call_id("deepseek-interstitial-run", 0, index, &call.id),
            name: call.name.clone(),
            args: call.args.clone(),
        })
        .collect::<Vec<_>>();
    let bindings = provider_calls
        .iter()
        .zip(runtime_calls.iter().cloned())
        .enumerate()
        .map(|(index, (provider_call, runtime_call))| {
            LlmRuntimeToolCallBinding::new(index, provider_call, runtime_call)
        })
        .collect();
    let turn = response
        .assistant_turn
        .with_runtime_tool_bindings(bindings)
        .unwrap();
    let mut image_context = LlmMessage::text(LlmMessageRole::User, "First tool image");
    image_context.images_mut().unwrap().push(LlmImage {
        mime_type: "image/png".to_string(),
        data_base64: "AA==".to_string(),
    });
    let extension_context = LlmMessage::text(
        LlmMessageRole::Assistant,
        "Runtime extension state updated.",
    );
    let request = LlmChatRequest {
        api_url: "https://api.deepseek.test/chat/completions".to_string(),
        api_token: "token".to_string(),
        provider_profile,
        provider_protocol,
        max_tokens: 512,
        temperature: 0.0,
        stream: false,
        messages: vec![
            LlmMessage::text(LlmMessageRole::User, "Read both files"),
            LlmMessage::from_assistant_turn(turn),
            LlmMessage::tool_result(runtime_calls[0].id.clone(), "first", false),
            image_context,
            extension_context,
            LlmMessage::tool_result(runtime_calls[1].id.clone(), "second", false),
        ],
        tools: vec![tool_definition()],
    };

    validate_request(&request).unwrap();
    let payload = build_payload(&request);
    let messages = payload["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 6);
    assert_eq!(messages[1]["role"], "assistant");
    assert_eq!(messages[1]["reasoning_content"], "Both files are required.");
    assert_eq!(messages[1]["tool_calls"].as_array().unwrap().len(), 2);
    assert_eq!(messages[1]["tool_calls"][0]["id"], "deepseek-raw-a");
    assert_eq!(messages[1]["tool_calls"][1]["id"], "deepseek-raw-b");
    assert_eq!(messages[2]["tool_call_id"], "deepseek-raw-a");
    assert_eq!(messages[3]["tool_call_id"], "deepseek-raw-b");
    assert_eq!(messages[4]["role"], "user");
    assert_eq!(messages[4]["content"][1]["type"], "image_url");
    assert_eq!(messages[5]["content"], "Runtime extension state updated.");
    assert!(messages[5].get("reasoning_content").is_none());
}

#[test]
fn deepseek_tool_history_without_a_compatible_continuation_requires_a_boundary() {
    let provider_profile = deepseek_provider_profile(ReasoningMode::Enabled, ReasoningEffort::High);
    let provider_protocol = deepseek_provider_protocol(&provider_profile, "deepseek-v4-pro");
    let provider_call = LlmToolCall {
        id: "legacy-provider-call".to_string(),
        name: "read_file".to_string(),
        args: json!({"path":"src/lib.rs"}),
    };
    let runtime_call = LlmToolCall {
        id: model_response_tool_call_id("deepseek-boundary-run", 0, 0, &provider_call.id),
        name: provider_call.name.clone(),
        args: provider_call.args.clone(),
    };
    let legacy_turn = LlmAssistantTurn::from_legacy("", vec![provider_call.clone()])
        .with_runtime_tool_bindings(vec![LlmRuntimeToolCallBinding::new(
            0,
            &provider_call,
            runtime_call.clone(),
        )])
        .unwrap();
    let request = LlmChatRequest {
        api_url: "https://api.deepseek.test/chat/completions".to_string(),
        api_token: "token".to_string(),
        provider_profile,
        provider_protocol,
        max_tokens: 512,
        temperature: 0.0,
        stream: false,
        messages: vec![
            LlmMessage::from_assistant_turn(legacy_turn),
            LlmMessage::tool_result(runtime_call.id, "result", false),
        ],
        tools: vec![tool_definition()],
    };

    let error = validate_request(&request).unwrap_err();
    assert_eq!(error.code(), Some("provider_context_boundary_required"));
    assert_eq!(
        error
            .details()
            .and_then(|details| details["reason"].as_str()),
        Some("missingToolBearingContinuation")
    );
}

#[test]
fn deepseek_tool_history_from_another_frozen_key_requires_a_boundary() {
    let provider_profile = deepseek_provider_profile(ReasoningMode::Enabled, ReasoningEffort::High);
    let prior_protocol = deepseek_provider_protocol(&provider_profile, "deepseek-v4-flash");
    let current_protocol = deepseek_provider_protocol(&provider_profile, "deepseek-v4-pro");
    let prior_response = parse_non_stream_response(
        &json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "",
                    "reasoning_content": "Prior model reasoning.",
                    "tool_calls": [{
                        "id": "prior-deepseek-call",
                        "type": "function",
                        "function": {
                            "name": "read_file",
                            "arguments": "{\"path\":\"src/lib.rs\"}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        })
        .to_string(),
        &prior_protocol,
        LlmResponseValidation::RequireModelAction,
    )
    .unwrap();
    let provider_call = prior_response.provider_tool_calls()[0].clone();
    let runtime_call = LlmToolCall {
        id: model_response_tool_call_id("deepseek-cross-key-run", 0, 0, &provider_call.id),
        name: provider_call.name.clone(),
        args: provider_call.args.clone(),
    };
    let prior_turn = prior_response
        .assistant_turn
        .with_runtime_tool_bindings(vec![LlmRuntimeToolCallBinding::new(
            0,
            &provider_call,
            runtime_call.clone(),
        )])
        .unwrap();
    let request = LlmChatRequest {
        api_url: "https://api.deepseek.test/chat/completions".to_string(),
        api_token: "token".to_string(),
        provider_profile,
        provider_protocol: current_protocol,
        max_tokens: 512,
        temperature: 0.0,
        stream: false,
        messages: vec![
            LlmMessage::from_assistant_turn(prior_turn),
            LlmMessage::tool_result(runtime_call.id, "result", false),
        ],
        tools: vec![tool_definition()],
    };

    let error = validate_request(&request).unwrap_err();
    assert_eq!(error.code(), Some("provider_context_boundary_required"));
    assert_eq!(
        error
            .details()
            .and_then(|details| details["reason"].as_str()),
        Some("incompatibleToolBearingContinuation")
    );
}

#[test]
fn deepseek_does_not_replay_reasoning_from_an_ordinary_prior_model_turn() {
    const PRIOR_REASONING: &str = "PRIOR_NO_TOOL_REASONING_MUST_NOT_REPLAY";
    let provider_profile = deepseek_provider_profile(ReasoningMode::Enabled, ReasoningEffort::High);
    let prior_protocol = deepseek_provider_protocol(&provider_profile, "deepseek-v4-flash");
    let current_protocol = deepseek_provider_protocol(&provider_profile, "deepseek-v4-pro");
    let prior_response = parse_non_stream_response(
        &json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "Prior visible answer.",
                    "reasoning_content": PRIOR_REASONING
                },
                "finish_reason": "stop"
            }]
        })
        .to_string(),
        &prior_protocol,
        LlmResponseValidation::RequireModelAction,
    )
    .unwrap();
    let request = LlmChatRequest {
        api_url: "https://api.deepseek.test/chat/completions".to_string(),
        api_token: "token".to_string(),
        provider_profile,
        provider_protocol: current_protocol,
        max_tokens: 512,
        temperature: 0.0,
        stream: false,
        messages: vec![
            LlmMessage::from_assistant_turn(prior_response.assistant_turn),
            LlmMessage::text(LlmMessageRole::User, "New user turn"),
        ],
        tools: Vec::new(),
    };

    validate_request(&request).unwrap();
    let encoded = serde_json::to_string(&build_payload(&request)).unwrap();
    assert!(!encoded.contains(PRIOR_REASONING));
    assert!(!encoded.contains("reasoning_content"));
}

#[tokio::test]
async fn fake_deepseek_provider_round_trips_reasoning_and_raw_tool_identity() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let mut requests = Vec::new();
        for response_index in 0..3 {
            let (mut stream, _) = listener.accept().await.unwrap();
            requests.push(read_test_http_request_json(&mut stream).await);
            let body = match response_index {
                0 => json!({
                    "choices": [{
                        "message": {
                            "role": "assistant",
                            "content": "",
                            "reasoning_content": "I should inspect the file first.",
                            "tool_calls": [{
                                "id": "deepseek-raw-call-1",
                                "type": "function",
                                "function": {
                                    "name": "read_file",
                                    "arguments": "{\"path\":\"src/lib.rs\"}"
                                }
                            }]
                        },
                        "finish_reason": "tool_calls"
                    }],
                    "usage": {
                        "prompt_tokens": 10,
                        "completion_tokens": 6,
                        "total_tokens": 16,
                        "prompt_cache_hit_tokens": 4,
                        "prompt_cache_miss_tokens": 6,
                        "completion_tokens_details": { "reasoning_tokens": 4 }
                    }
                }),
                1 => json!({
                    "choices": [{
                        "message": {
                            "role": "assistant",
                            "content": "",
                            "reasoning_content": "I should inspect one more file.",
                            "tool_calls": [{
                                "id": "deepseek-raw-call-2",
                                "type": "function",
                                "function": {
                                    "name": "read_file",
                                    "arguments": "{\"path\":\"src/main.rs\"}"
                                }
                            }]
                        },
                        "finish_reason": "tool_calls"
                    }],
                    "usage": {
                        "prompt_tokens": 18,
                        "completion_tokens": 5,
                        "total_tokens": 23,
                        "completion_tokens_details": { "reasoning_tokens": 3 }
                    }
                }),
                2 => json!({
                    "choices": [{
                        "message": {
                            "role": "assistant",
                            "content": "The file is ready.",
                            "reasoning_content": "The tool result is sufficient."
                        },
                        "finish_reason": "stop"
                    }],
                    "usage": {
                        "prompt_tokens": 18,
                        "completion_tokens": 5,
                        "total_tokens": 23,
                        "completion_tokens_details": { "reasoning_tokens": 3 }
                    }
                }),
                _ => unreachable!(),
            };
            write_test_http_response(&mut stream, "200 OK", body).await;
        }
        requests
    });

    let provider_profile = deepseek_provider_profile(ReasoningMode::Enabled, ReasoningEffort::Max);
    let provider_protocol = deepseek_provider_protocol(&provider_profile, "deepseek-v4-pro");
    let first_request = LlmChatRequest {
        api_url: format!("http://{address}/chat/completions"),
        api_token: "deepseek-fake-token".to_string(),
        provider_profile: provider_profile.clone(),
        provider_protocol: provider_protocol.clone(),
        max_tokens: 1_024,
        temperature: 0.3,
        stream: false,
        messages: vec![LlmMessage::text(LlmMessageRole::User, "Inspect src/lib.rs")],
        tools: vec![tool_definition()],
    };
    let first_response = complete_chat(first_request, AgentCancellationToken::new())
        .await
        .unwrap();
    assert_eq!(first_response.content(), "");
    assert_eq!(first_response.provider_tool_calls().len(), 1);
    assert_eq!(
        first_response.usage.as_ref().unwrap().input_tokens,
        Some(10)
    );
    assert_eq!(
        first_response
            .usage
            .as_ref()
            .unwrap()
            .output_thinking_tokens,
        Some(4)
    );
    assert_eq!(
        deepseek_reasoning_content(&provider_protocol, &first_response.assistant_turn).unwrap(),
        Some("I should inspect the file first.")
    );

    let provider_call = first_response.provider_tool_calls()[0].clone();
    let runtime_call = LlmToolCall {
        id: model_response_tool_call_id("deepseek-fake-run", 0, 0, &provider_call.id),
        name: provider_call.name.clone(),
        args: provider_call.args.clone(),
    };
    let first_turn = first_response
        .assistant_turn
        .with_runtime_tool_bindings(vec![LlmRuntimeToolCallBinding::new(
            0,
            &provider_call,
            runtime_call.clone(),
        )])
        .unwrap();
    let first_turn_message = LlmMessage::from_assistant_turn(first_turn);
    let first_result_message =
        LlmMessage::tool_result(runtime_call.id.clone(), "{\"ok\":true}", false);
    let second_request = LlmChatRequest {
        api_url: format!("http://{address}/chat/completions"),
        api_token: "deepseek-fake-token".to_string(),
        provider_profile: provider_profile.clone(),
        provider_protocol: provider_protocol.clone(),
        max_tokens: 1_024,
        temperature: 0.3,
        stream: false,
        messages: vec![
            LlmMessage::text(LlmMessageRole::User, "Inspect src/lib.rs"),
            first_turn_message.clone(),
            first_result_message.clone(),
        ],
        tools: vec![tool_definition()],
    };
    let second_response = complete_chat(second_request, AgentCancellationToken::new())
        .await
        .unwrap();
    assert_eq!(second_response.content(), "");
    assert_eq!(second_response.provider_tool_calls().len(), 1);
    assert_eq!(
        deepseek_reasoning_content(&provider_protocol, &second_response.assistant_turn).unwrap(),
        Some("I should inspect one more file.")
    );

    let second_provider_call = second_response.provider_tool_calls()[0].clone();
    let second_runtime_call = LlmToolCall {
        id: model_response_tool_call_id("deepseek-fake-run", 1, 0, &second_provider_call.id),
        name: second_provider_call.name.clone(),
        args: second_provider_call.args.clone(),
    };
    let second_turn = second_response
        .assistant_turn
        .with_runtime_tool_bindings(vec![LlmRuntimeToolCallBinding::new(
            0,
            &second_provider_call,
            second_runtime_call.clone(),
        )])
        .unwrap();
    let third_request = LlmChatRequest {
        api_url: format!("http://{address}/chat/completions"),
        api_token: "deepseek-fake-token".to_string(),
        provider_profile,
        provider_protocol: provider_protocol.clone(),
        max_tokens: 1_024,
        temperature: 0.3,
        stream: false,
        messages: vec![
            LlmMessage::text(LlmMessageRole::User, "Inspect src/lib.rs"),
            first_turn_message,
            first_result_message,
            LlmMessage::from_assistant_turn(second_turn),
            LlmMessage::tool_result(second_runtime_call.id, "{\"ok\":true}", false),
        ],
        tools: vec![tool_definition()],
    };
    let third_response = complete_chat(third_request, AgentCancellationToken::new())
        .await
        .unwrap();
    let requests = server.await.unwrap();

    assert_eq!(third_response.content(), "The file is ready.");
    assert_eq!(
        third_response
            .assistant_turn
            .provider_continuation()
            .unwrap()
            .replay_scope(),
        ProviderContinuationReplayScope::AssistantTurnV1
    );
    assert_eq!(requests[0]["thinking"], json!({ "type": "enabled" }));
    assert_eq!(requests[0]["reasoning_effort"], "max");
    assert!(requests[0].get("temperature").is_none());
    assert!(requests[0].get("tool_choice").is_none());
    assert_eq!(requests[1]["messages"][1]["role"], "assistant");
    assert_eq!(requests[1]["messages"][1]["content"], "");
    assert_eq!(
        requests[1]["messages"][1]["reasoning_content"],
        "I should inspect the file first."
    );
    assert_eq!(
        requests[1]["messages"][1]["tool_calls"][0]["id"],
        "deepseek-raw-call-1"
    );
    assert_eq!(
        requests[1]["messages"][2]["tool_call_id"],
        "deepseek-raw-call-1"
    );
    assert_eq!(
        requests[2]["messages"][1]["reasoning_content"],
        "I should inspect the file first."
    );
    assert_eq!(
        requests[2]["messages"][3]["reasoning_content"],
        "I should inspect one more file."
    );
    assert_eq!(
        requests[2]["messages"][3]["tool_calls"][0]["id"],
        "deepseek-raw-call-2"
    );
    assert_eq!(
        requests[2]["messages"][4]["tool_call_id"],
        "deepseek-raw-call-2"
    );
}

#[tokio::test]
async fn deepseek_stream_retry_discards_failed_attempt_reasoning() {
    const STALE: &str = "STALE_DEEPSEEK_REASONING";
    const FRESH: &str = "FRESH_DEEPSEEK_REASONING";
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        for attempt in 0..2 {
            let (mut stream, _) = listener.accept().await.unwrap();
            read_test_http_request(&mut stream).await;
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
                )
                .await
                .unwrap();
            if attempt == 0 {
                stream
                    .write_all(
                        format!(
                            "data: {}\n\ndata: {{invalid-json\n\n",
                            json!({
                                "choices":[{"delta":{"reasoning_content":STALE}}],
                                "usage": {
                                    "prompt_tokens": 3,
                                    "completion_tokens": 3,
                                    "total_tokens": 6,
                                    "completion_tokens_details": { "reasoning_tokens": 2 }
                                }
                            })
                        )
                        .as_bytes(),
                    )
                    .await
                    .unwrap();
            } else {
                stream
                    .write_all(
                        format!(
                            "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
                            json!({"choices":[{"delta":{"reasoning_content":FRESH}}]}),
                            json!({
                                "choices":[{
                                    "delta":{"content":"Recovered."},
                                    "finish_reason":"stop"
                                }],
                                "usage": {
                                    "prompt_tokens": 2,
                                    "completion_tokens": 2,
                                    "total_tokens": 4
                                }
                            })
                        )
                        .as_bytes(),
                    )
                    .await
                    .unwrap();
            }
        }
    });

    let provider_profile = deepseek_provider_profile(ReasoningMode::Enabled, ReasoningEffort::High);
    let provider_protocol = deepseek_provider_protocol(&provider_profile, "deepseek-v4-flash");
    let request = LlmChatRequest {
        api_url: format!("http://{address}/chat/completions"),
        api_token: "deepseek-stream-retry-token".to_string(),
        provider_profile,
        provider_protocol: provider_protocol.clone(),
        max_tokens: 128,
        temperature: 0.0,
        stream: true,
        messages: vec![LlmMessage::text(LlmMessageRole::User, "hello")],
        tools: Vec::new(),
    };
    let mut events = Vec::new();
    let response = complete_chat_streaming(request, AgentCancellationToken::new(), |event| {
        events.push(event);
    })
    .await
    .unwrap();
    server.await.unwrap();

    assert_eq!(response.content(), "Recovered.");
    let usage = response.usage.as_ref().expect("retry usage");
    assert_eq!(usage.input_tokens, Some(5));
    assert_eq!(usage.total_tokens, Some(10));
    assert_eq!(usage.output_tokens, None);
    assert_eq!(usage.output_thinking_tokens, None);
    assert_eq!(usage.billable_request_count, Some(2));
    assert_eq!(
        deepseek_reasoning_content(&provider_protocol, &response.assistant_turn).unwrap(),
        Some(FRESH)
    );
    assert!(events.iter().all(|event| !matches!(
        event,
        LlmStreamEvent::Delta(delta) if delta.contains(STALE) || delta.contains(FRESH)
    )));
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, LlmStreamEvent::AttemptStarted { .. }))
            .count(),
        2
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, LlmStreamEvent::AttemptReset { .. }))
            .count(),
        1
    );
}

#[test]
fn provider_continuation_is_bounded_bound_and_debug_redacted() {
    const CANARY: &str = "OPAQUE_PROVIDER_CONTINUATION_CANARY";
    let protocol = generic_provider_protocol(AgentApiStyle::OpenAiCompatible, "gpt");
    let turn = LlmAssistantTurn::from_provider(protocol.clone(), "visible", Vec::new()).unwrap();
    let continuation = ProviderContinuation::new(
        protocol.clone(),
        ProviderContinuationReplayScope::AssistantTurnV1,
        turn.digest(),
        vec![ProviderContinuationFragment::new(
            ProviderContinuationPosition::new(0, ProviderContinuationAttachment::AssistantTurn),
            CANARY.as_bytes().to_vec(),
        )],
    )
    .unwrap();

    assert_eq!(continuation.encoded_bytes(), CANARY.len());
    assert_eq!(
        continuation.replay_scope(),
        ProviderContinuationReplayScope::AssistantTurnV1
    );
    assert_eq!(continuation.payload_hash().len(), "sha256:".len() + 64);
    assert!(!format!("{continuation:?}").contains(CANARY));

    let revision_without_continuation = turn.context_revision_material();
    let turn = turn.with_provider_continuation(continuation).unwrap();
    let revision_with_continuation = turn.context_revision_material();
    assert_eq!(revision_with_continuation.len(), "sha256:".len() + 64);
    assert!(!revision_with_continuation.contains(CANARY));
    assert_ne!(revision_with_continuation, revision_without_continuation);
    let serialized_checkpoint_identity =
        serde_json::to_string(&turn.checkpoint_identity().unwrap()).unwrap();
    assert!(!serialized_checkpoint_identity.contains(CANARY));
    assert!(turn
        .without_raw_continuation_for_checkpoint()
        .provider_continuation()
        .is_none());

    let at_limit = ProviderContinuation::new(
        protocol.clone(),
        ProviderContinuationReplayScope::AssistantTurnV1,
        turn.digest(),
        vec![ProviderContinuationFragment::new(
            ProviderContinuationPosition::new(0, ProviderContinuationAttachment::AssistantTurn),
            vec![0; MAX_PROVIDER_CONTINUATION_BYTES],
        )],
    )
    .unwrap();
    assert_eq!(at_limit.encoded_bytes(), MAX_PROVIDER_CONTINUATION_BYTES);

    let oversized = ProviderContinuation::new(
        protocol,
        ProviderContinuationReplayScope::AssistantTurnV1,
        turn.digest(),
        vec![ProviderContinuationFragment::new(
            ProviderContinuationPosition::new(0, ProviderContinuationAttachment::AssistantTurn),
            vec![0; MAX_PROVIDER_CONTINUATION_BYTES + 1],
        )],
    );
    assert!(oversized.is_err());
}

#[test]
fn provider_continuation_rejects_cross_key_digest_and_reordered_fragments() {
    let protocol = generic_provider_protocol(AgentApiStyle::OpenAiCompatible, "gpt-a");
    let other_protocol = generic_provider_protocol(AgentApiStyle::OpenAiCompatible, "gpt-b");
    let turn = LlmAssistantTurn::from_provider(protocol.clone(), "visible", Vec::new()).unwrap();
    let other_turn =
        LlmAssistantTurn::from_provider(protocol.clone(), "changed", Vec::new()).unwrap();
    let fragment = || {
        vec![ProviderContinuationFragment::new(
            ProviderContinuationPosition::new(0, ProviderContinuationAttachment::AssistantTurn),
            vec![1],
        )]
    };

    let cross_key = ProviderContinuation::new(
        other_protocol,
        ProviderContinuationReplayScope::AssistantTurnV1,
        turn.digest(),
        fragment(),
    )
    .unwrap();
    assert!(turn.clone().with_provider_continuation(cross_key).is_err());

    let cross_digest = ProviderContinuation::new(
        protocol.clone(),
        ProviderContinuationReplayScope::AssistantTurnV1,
        other_turn.digest(),
        fragment(),
    )
    .unwrap();
    assert!(turn
        .clone()
        .with_provider_continuation(cross_digest)
        .is_err());

    let reordered = ProviderContinuation::new(
        protocol,
        ProviderContinuationReplayScope::InteractionV1,
        turn.digest(),
        vec![
            ProviderContinuationFragment::new(
                ProviderContinuationPosition::new(
                    1,
                    ProviderContinuationAttachment::InteractionStep(1),
                ),
                vec![1],
            ),
            ProviderContinuationFragment::new(
                ProviderContinuationPosition::new(
                    0,
                    ProviderContinuationAttachment::InteractionStep(0),
                ),
                vec![2],
            ),
        ],
    );
    assert!(reordered.is_err());

    let bound = turn
        .clone()
        .with_provider_continuation(
            ProviderContinuation::new(
                turn.provider_protocol().unwrap().clone(),
                ProviderContinuationReplayScope::AssistantTurnV1,
                turn.digest(),
                fragment(),
            )
            .unwrap(),
        )
        .unwrap();
    assert!(bound
        .with_reasoning(vec![ReasoningProjection::summary(
            ProviderContinuationPosition::new(0, ProviderContinuationAttachment::ContentBlock(0),),
            "changed reasoning projection",
        )])
        .is_err());
}

#[test]
fn generic_response_parsers_do_not_capture_reasoning_fields() {
    for (api_style, model, response_body) in [
        (
            AgentApiStyle::OpenAiCompatible,
            "gpt",
            json!({
                "choices": [{
                    "message": {
                        "role": "assistant",
                        "content": "visible",
                        "reasoning_content": "MUST_NOT_CAPTURE"
                    },
                    "finish_reason": "stop"
                }]
            }),
        ),
        (
            AgentApiStyle::AnthropicCompatible,
            "claude",
            json!({
                "content": [
                    {
                        "type": "thinking",
                        "thinking": "MUST_NOT_CAPTURE",
                        "signature": "MUST_NOT_CAPTURE_SIGNATURE"
                    },
                    { "type": "text", "text": "visible" }
                ],
                "stop_reason": "end_turn"
            }),
        ),
    ] {
        let protocol = generic_provider_protocol(api_style, model);
        let response = parse_non_stream_response(
            &response_body.to_string(),
            &protocol,
            LlmResponseValidation::RequireModelAction,
        )
        .unwrap();

        assert_eq!(response.content(), "visible");
        assert!(response.assistant_turn.reasoning().is_empty());
        assert!(response.assistant_turn.provider_continuation().is_none());
    }
}

#[test]
fn generic_request_adapters_do_not_send_reasoning_projection() {
    const CANARY: &str = "REASONING_PROJECTION_MUST_NOT_ENTER_GENERIC_WIRE";
    for (api_style, model) in [
        (AgentApiStyle::OpenAiCompatible, "gpt"),
        (AgentApiStyle::AnthropicCompatible, "claude"),
    ] {
        let profile = generic_provider_profile(api_style);
        let protocol = generic_provider_protocol(api_style, model);
        let turn = LlmAssistantTurn::from_provider(protocol.clone(), "visible", Vec::new())
            .unwrap()
            .with_reasoning(vec![ReasoningProjection::summary(
                ProviderContinuationPosition::new(
                    0,
                    ProviderContinuationAttachment::ContentBlock(0),
                ),
                CANARY,
            )])
            .unwrap();
        let request = LlmChatRequest {
            api_url: "https://example.test".to_string(),
            api_token: "token".to_string(),
            provider_profile: profile,
            provider_protocol: protocol,
            max_tokens: 100,
            temperature: 0.2,
            stream: false,
            messages: vec![LlmMessage::from_assistant_turn(turn)],
            tools: Vec::new(),
        };

        let encoded = serde_json::to_string(&build_payload(&request)).unwrap();
        assert!(!encoded.contains(CANARY));
        assert!(!encoded.contains("reasoning"));
        assert!(!encoded.contains("thinking"));
    }
}
