// Rust agent core.
mod payload;
mod response;
mod stream;

use crate::cancellation::AgentCancellationToken;
use crate::protocol::{AgentApiStyle, AgentError, AgentResult, AgentToolDefinition, AgentUsage};
use crate::usage::extract_usage;
use payload::{build_headers, build_payload, is_sse_response};
use response::{
    extract_api_error, extract_finish_reason, extract_response_text, extract_tool_calls,
    truncate_for_error,
};
use serde_json::Value;
use std::time::Duration;
use stream::parse_sse_response;
#[cfg(test)]
use stream::{process_sse_frame, LlmStreamAccumulator};

#[derive(Debug, Clone)]
pub(crate) struct LlmChatRequest {
    pub api_url: String,
    pub api_token: String,
    pub model: String,
    pub api_style: AgentApiStyle,
    pub max_tokens: u32,
    pub temperature: f32,
    pub stream: bool,
    pub messages: Vec<LlmMessage>,
    pub tools: Vec<AgentToolDefinition>,
}

#[derive(Debug, Clone)]
pub(crate) struct LlmChatResponse {
    pub content: String,
    pub tool_calls: Vec<LlmToolCall>,
    pub usage: Option<AgentUsage>,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct LlmMessage {
    pub role: LlmMessageRole,
    pub content: String,
    pub images: Vec<LlmImage>,
    pub tool_call_id: Option<String>,
    pub tool_calls: Vec<LlmToolCall>,
    pub is_error: bool,
}

impl LlmMessage {
    pub(crate) fn text(role: LlmMessageRole, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
            images: Vec::new(),
            tool_call_id: None,
            tool_calls: Vec::new(),
            is_error: false,
        }
    }

    pub(crate) fn assistant(content: impl Into<String>, tool_calls: Vec<LlmToolCall>) -> Self {
        Self {
            role: LlmMessageRole::Assistant,
            content: content.into(),
            images: Vec::new(),
            tool_call_id: None,
            tool_calls,
            is_error: false,
        }
    }

    pub(crate) fn tool_result(
        tool_call_id: impl Into<String>,
        content: impl Into<String>,
        is_error: bool,
    ) -> Self {
        Self {
            role: LlmMessageRole::Tool,
            content: content.into(),
            images: Vec::new(),
            tool_call_id: Some(tool_call_id.into()),
            tool_calls: Vec::new(),
            is_error,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LlmMessageRole {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone)]
pub(crate) struct LlmImage {
    pub mime_type: String,
    pub data_base64: String,
}

impl LlmMessageRole {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::Tool => "tool",
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct LlmToolCall {
    pub id: String,
    pub name: String,
    pub args: Value,
}

pub(crate) async fn complete_chat(
    request: LlmChatRequest,
    cancellation_token: AgentCancellationToken,
) -> AgentResult<LlmChatResponse> {
    let mut request = request;
    request.stream = false;
    let api_style = request.api_style;
    let response = send_llm_request(&request, cancellation_token.clone()).await?;
    let body = response_text(response, cancellation_token.clone(), "读取模型响应失败").await?;

    let value: Value = serde_json::from_str(&body).map_err(|error| {
        AgentError::new(format!(
            "模型响应不是有效 JSON：{error}；原始响应：{}",
            truncate_for_error(&body)
        ))
    })?;
    if let Some(error) = extract_api_error(&value) {
        return Err(AgentError::new(format!("模型接口返回错误：{error}")));
    }

    let tool_calls = extract_tool_calls(&value, api_style)?;
    let content = extract_response_text(&value).unwrap_or_default();

    validate_llm_response(&content, &tool_calls, &body)?;

    Ok(LlmChatResponse {
        content,
        tool_calls,
        usage: extract_usage(&value),
        finish_reason: extract_finish_reason(&value),
    })
}

pub(crate) async fn complete_chat_streaming<F>(
    request: LlmChatRequest,
    cancellation_token: AgentCancellationToken,
    mut on_delta: F,
) -> AgentResult<LlmChatResponse>
where
    F: FnMut(String) + Send,
{
    let mut request = request;
    request.stream = true;
    let api_style = request.api_style;
    let response = send_llm_request(&request, cancellation_token.clone()).await?;
    if !is_sse_response(&response) {
        let body = response_text(response, cancellation_token.clone(), "读取模型响应失败").await?;
        let value: Value = serde_json::from_str(&body).map_err(|error| {
            AgentError::new(format!(
                "模型响应不是有效 JSON：{error}；原始响应：{}",
                truncate_for_error(&body)
            ))
        })?;
        if let Some(error) = extract_api_error(&value) {
            return Err(AgentError::new(format!("模型接口返回错误：{error}")));
        }

        let tool_calls = extract_tool_calls(&value, api_style)?;
        let content = extract_response_text(&value).unwrap_or_default();
        validate_llm_response(&content, &tool_calls, &body)?;
        if !content.is_empty() {
            on_delta(content.clone());
        }
        return Ok(LlmChatResponse {
            content,
            tool_calls,
            usage: extract_usage(&value),
            finish_reason: extract_finish_reason(&value),
        });
    }

    let streamed = parse_sse_response(response, api_style, cancellation_token, on_delta).await?;

    validate_llm_response(
        &streamed.content,
        &streamed.tool_calls,
        "streaming response",
    )?;
    Ok(streamed)
}

async fn send_llm_request(
    request: &LlmChatRequest,
    cancellation_token: AgentCancellationToken,
) -> AgentResult<reqwest::Response> {
    cancellation_token.check()?;
    validate_request(request)?;
    let payload = build_payload(request);
    let headers = build_headers(request.api_style, request.api_token.trim())?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .map_err(|error| AgentError::new(format!("创建 HTTP 客户端失败：{error}")))?;

    let send = client
        .post(request.api_url.trim())
        .headers(headers)
        .json(&payload)
        .send();
    let response = tokio::select! {
        _ = cancellation_token.cancelled() => return Err(AgentError::cancelled()),
        response = send => response
            .map_err(|error| AgentError::new(format!("请求模型接口失败：{error}")))?,
    };

    let status = response.status();
    if !status.is_success() {
        let body =
            response_text(response, cancellation_token.clone(), "读取模型错误响应失败").await?;
        return Err(AgentError::new(format!(
            "模型接口返回 {}：{}",
            status.as_u16(),
            truncate_for_error(&body)
        )));
    }

    Ok(response)
}

async fn response_text(
    response: reqwest::Response,
    cancellation_token: AgentCancellationToken,
    error_prefix: &str,
) -> AgentResult<String> {
    cancellation_token.check()?;
    let read = response.text();
    tokio::select! {
        _ = cancellation_token.cancelled() => Err(AgentError::cancelled()),
        body = read => body.map_err(|error| AgentError::new(format!("{error_prefix}：{error}"))),
    }
}

fn validate_request(request: &LlmChatRequest) -> AgentResult<()> {
    if request.api_url.trim().is_empty() {
        return Err(AgentError::new("请先在设置 > 配置里填写 API URL。"));
    }
    if request.api_token.trim().is_empty() {
        return Err(AgentError::new("请先在设置 > 配置里填写 API Token。"));
    }
    if request.model.trim().is_empty() {
        return Err(AgentError::new("请选择一个可用模型。"));
    }
    if request.messages.is_empty() {
        return Err(AgentError::new("没有可发送的对话内容。"));
    }

    Ok(())
}

fn validate_llm_response(
    content: &str,
    tool_calls: &[LlmToolCall],
    raw_response: &str,
) -> AgentResult<()> {
    if content.trim().is_empty() && tool_calls.is_empty() {
        return Err(AgentError::new(format!(
            "模型响应里没有可显示文本：{}",
            truncate_for_error(raw_response)
        )));
    }

    Ok(())
}

pub(crate) fn detect_api_style(api_url: &str) -> AgentApiStyle {
    let normalized = api_url.to_ascii_lowercase();

    if normalized.contains("/chat/completions") {
        return AgentApiStyle::OpenAiCompatible;
    }

    if normalized.contains("anthropic") || normalized.ends_with("/messages") {
        return AgentApiStyle::AnthropicCompatible;
    }

    AgentApiStyle::OpenAiCompatible
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::AgentToolSafety;
    use serde_json::json;

    fn message(role: LlmMessageRole, content: &str) -> LlmMessage {
        LlmMessage::text(role, content)
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
        }
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
        assert_eq!(deltas.join(""), "Hello");
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
        assert_eq!(deltas.join(""), "Hi there");
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
}
