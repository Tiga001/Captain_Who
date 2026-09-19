use super::adapter::{deepseek_reasoning_content, ProviderAdapterRegistry};
use super::payload::{build_anthropic_headers, build_openai_headers, build_payload};
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
        max_tokens: Some(100),
        temperature: 0.0,
        stream: false,
        messages: vec![message.clone()],
        tools: Vec::new(),
    };
    let response = LlmChatResponse {
        assistant_turn: LlmAssistantTurn::from_split_projection(CANARY, vec![tool_call.clone()]),
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

#[test]
fn provider_credential_headers_are_marked_sensitive() {
    let openai = build_openai_headers("openai-secret").unwrap();
    assert!(openai
        .get(reqwest::header::AUTHORIZATION)
        .is_some_and(reqwest::header::HeaderValue::is_sensitive));

    let anthropic = build_anthropic_headers("anthropic-secret").unwrap();
    assert!(anthropic
        .get("x-api-key")
        .is_some_and(reqwest::header::HeaderValue::is_sensitive));
}
use crate::context::{
    format_message_created_at, ContextAssembler, ContextAssemblyInput, ContextAttachments,
    ContextCapacityDetector, ContextFrame, ContextGroup, ContextItem, ContextMetadata,
    ContextRetention, ContextScope, ContextSource,
};
use crate::conversation_trace::{
    render_tool_observation, ConversationTraceRecorder, ConversationTurnTraceTerminalStatus,
};
use crate::protocol::{
    AgentApprovalStatus, AgentChatMessage, AgentProviderToolCallIdentity, AgentToolCall,
    AgentToolIdentity, AgentToolResult, AgentToolSafety,
};
use crate::provider_profile::{
    ProviderFamilyReasoningPolicy, ProviderFamilySettings, ProviderProfileRef,
    ProviderReasoningEffort, ProviderVendorId, ReasoningEffort, ReasoningMode,
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
    ProviderProfileConfig::from_family_settings(
        ProviderProfileRef::deepseek_v4_1_flash_chat(),
        ProviderVendorId::DeepSeek,
        ProviderFamilySettings::DeepseekFlashChat {
            reasoning: ProviderFamilyReasoningPolicy {
                mode,
                effort: match effort {
                    ReasoningEffort::ProviderDefault => ProviderReasoningEffort::ProviderDefault,
                    ReasoningEffort::High => ProviderReasoningEffort::High,
                    ReasoningEffort::Max => ProviderReasoningEffort::Max,
                },
            },
        },
    )
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
        max_tokens: Some(1024),
        temperature: 0.2,
        stream: true,
        messages,
        tools: vec![tool_definition()],
    }
}

async fn read_test_http_request(stream: &mut TcpStream) {
    let _ = read_test_http_request_json(stream).await;
}

async fn read_test_http_request_raw(stream: &mut TcpStream) -> Vec<u8> {
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
                return request;
            }
        }
    }
}

async fn read_test_http_request_json(stream: &mut TcpStream) -> Value {
    let request = read_test_http_request_raw(stream).await;
    let body_start = request
        .windows(4)
        .position(|part| part == b"\r\n\r\n")
        .map(|index| index + 4)
        .unwrap();
    serde_json::from_slice(&request[body_start..]).unwrap()
}

fn test_http_request_has_header(request: &[u8], expected_name: &str) -> bool {
    let header_end = request
        .windows(4)
        .position(|part| part == b"\r\n\r\n")
        .unwrap();
    std::str::from_utf8(&request[..header_end])
        .unwrap()
        .lines()
        .skip(1)
        .filter_map(|line| line.split_once(':'))
        .any(|(name, value)| name.eq_ignore_ascii_case(expected_name) && !value.trim().is_empty())
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
        conversation_completion_covered: false,
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
    let call = AgentToolCall {
        id: call_id.clone(),
        tool: "read_file".to_string(),
        args: json!({ "path": "src/lib.rs", "startLine": 1 }),
        approval_status: AgentApprovalStatus::NotRequired,
        reason: None,
    };
    let result = AgentToolResult {
        exact_archive_file: None,
        call_id: call_id.clone(),
        tool: "read_file".to_string(),
        ok: true,
        result: Some(json!({
            "path": "src/lib.rs",
            "startLine": 1,
            "endLine": 20,
            "truncated": false
        })),
        error: None,
    };
    let mut recorder = ConversationTraceRecorder::default();
    recorder
        .record_narration("I will inspect src/lib.rs.")
        .expect("current history narration");
    let call_sequence = recorder
        .record_tool_call_with_identity(
            &call,
            AgentToolIdentity::Builtin {
                tool_name: "read_file".to_string(),
            },
        )
        .expect("current history Tool Call");
    recorder
        .record_model_tool_call_message(
            call_sequence,
            0,
            &LlmMessage::assistant(
                "",
                vec![LlmToolCall {
                    id: call.id.clone(),
                    name: call.tool.clone(),
                    args: call.args.clone(),
                }],
            ),
            AgentProviderToolCallIdentity {
                provider_tool_index: 0,
                provider_call_id: "provider-call-1".to_string(),
                runtime_call_id: call.id.clone(),
            },
        )
        .expect("current history Provider/Runtime identity");
    let result_sequence = recorder
        .record_tool_result(&call, &result)
        .expect("current history Tool Result");
    recorder
        .record_model_message(
            result_sequence,
            0,
            &LlmMessage::tool_result(call.id.clone(), render_tool_observation(&result), false),
        )
        .expect("current history Tool Result model context");
    let (_, conversation_model_context_items, _, _) = recorder.checkpoint();
    let conversation_turn_trace = recorder.finish(
        "historical-run-1",
        "conversation-1",
        "assistant-history",
        ConversationTurnTraceTerminalStatus::Completed,
        None,
    );
    AgentChatMessage {
        conversation_completion_covered: false,
        message_id: Some("assistant-history".to_string()),
        role: "assistant".to_string(),
        content: content.to_string(),
        created_at: None,
        conversation_turn_trace: Some(conversation_turn_trace),
        conversation_model_context_items,
    }
}

fn traced_narration_chat_message(message_id: &str, content: &str) -> AgentChatMessage {
    let recorder = ConversationTraceRecorder::default();
    let (_, conversation_model_context_items, _, _) = recorder.checkpoint();
    let conversation_turn_trace = recorder.finish(
        "historical-narration-run",
        "conversation-1",
        message_id,
        ConversationTurnTraceTerminalStatus::Completed,
        None,
    );
    AgentChatMessage {
        conversation_completion_covered: false,
        message_id: Some(message_id.to_string()),
        role: "assistant".to_string(),
        content: content.to_string(),
        created_at: None,
        conversation_turn_trace: Some(conversation_turn_trace),
        conversation_model_context_items,
    }
}

fn historical_trace_call_id() -> String {
    model_response_tool_call_id("historical-run-1", 0, 0, "provider-call-1")
}

mod deepseek_projection;
mod deepseek_runtime;
mod generic_payloads;
mod moonshot_wire;
mod output_budget;
mod retries;
mod streaming_and_usage;
