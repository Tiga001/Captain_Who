mod payload;
mod response;
mod stream;

use crate::cancellation::AgentCancellationToken;
use crate::protocol::{AgentApiStyle, AgentError, AgentResult, AgentToolDefinition, AgentUsage};
use crate::usage::{extract_usage, merge_total_usage, usage_for_request};
use payload::{build_headers, build_payload, is_sse_response};
use response::{
    extract_api_error, extract_finish_reason, extract_response_text, extract_tool_calls,
    truncate_for_error,
};
use serde_json::{json, Value};
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

#[derive(Debug, Clone, PartialEq)]
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

#[derive(Debug, Clone, PartialEq, Eq)]
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

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LlmToolCall {
    pub id: String,
    pub name: String,
    pub args: Value,
}

#[derive(Debug, Clone)]
pub(crate) enum LlmStreamEvent {
    AttemptStarted {
        attempt: usize,
        max_attempts: usize,
    },
    Delta(String),
    ToolInputProgress {
        tool_call_index: usize,
        tool_call_id: Option<String>,
        tool: String,
        input_delta: String,
        received_bytes: u64,
    },
    AttemptReset {
        reason: String,
    },
    Retrying {
        attempt: usize,
        max_attempts: usize,
        reason: String,
    },
    Committed,
}

const LLM_MAX_ATTEMPTS: usize = 3;
const LLM_RETRY_BASE_DELAY_MS: u64 = 350;
const LLM_RETRY_MAX_DELAY_MS: u64 = 2_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LlmResponseValidation {
    RequireModelAction,
    AllowEmpty,
}

pub(crate) async fn complete_chat(
    request: LlmChatRequest,
    cancellation_token: AgentCancellationToken,
) -> AgentResult<LlmChatResponse> {
    complete_chat_with_validation(
        request,
        cancellation_token,
        LlmResponseValidation::RequireModelAction,
    )
    .await
}

pub(crate) async fn complete_chat_allow_empty(
    request: LlmChatRequest,
    cancellation_token: AgentCancellationToken,
) -> AgentResult<LlmChatResponse> {
    complete_chat_with_validation(
        request,
        cancellation_token,
        LlmResponseValidation::AllowEmpty,
    )
    .await
}

async fn complete_chat_with_validation(
    request: LlmChatRequest,
    cancellation_token: AgentCancellationToken,
    validation: LlmResponseValidation,
) -> AgentResult<LlmChatResponse> {
    let mut request = request;
    request.stream = false;

    let mut last_error = None;
    let mut total_usage = None;
    for attempt in 1..=LLM_MAX_ATTEMPTS {
        match complete_chat_once(&request, cancellation_token.clone(), validation).await {
            Ok(mut response) => {
                merge_total_usage(&mut total_usage, response.usage.take());
                response.usage = total_usage;
                return Ok(response);
            }
            Err(error) if error.is_cancelled() => {
                return Err(merge_error_usage(error, &mut total_usage));
            }
            Err(error) => {
                let error = merge_error_usage(error, &mut total_usage);
                if attempt >= LLM_MAX_ATTEMPTS || !is_retryable_llm_error(&error) {
                    return Err(retry_exhausted_error(error, attempt));
                }
                last_error = Some(error);
                wait_before_retry(attempt, cancellation_token.clone())
                    .await
                    .map_err(|error| error.with_usage(total_usage.clone()))?;
            }
        }
    }

    Err(retry_exhausted_error(
        last_error.unwrap_or_else(|| AgentError::new("模型请求失败。")),
        LLM_MAX_ATTEMPTS,
    ))
}

async fn complete_chat_once(
    request: &LlmChatRequest,
    cancellation_token: AgentCancellationToken,
    validation: LlmResponseValidation,
) -> AgentResult<LlmChatResponse> {
    let api_style = request.api_style;
    validate_request(request)?;
    let response = send_llm_request(request, cancellation_token.clone())
        .await
        .map_err(with_request_usage)?;
    let body = response_text(response, cancellation_token.clone(), "读取模型响应失败")
        .await
        .map_err(with_request_usage)?;
    parse_non_stream_response(&body, api_style, validation)
}

pub(crate) async fn complete_chat_streaming<F>(
    request: LlmChatRequest,
    cancellation_token: AgentCancellationToken,
    on_event: F,
) -> AgentResult<LlmChatResponse>
where
    F: FnMut(LlmStreamEvent) + Send,
{
    complete_chat_streaming_with_validation(
        request,
        cancellation_token,
        LlmResponseValidation::RequireModelAction,
        on_event,
    )
    .await
}

pub(crate) async fn complete_chat_streaming_allow_empty<F>(
    request: LlmChatRequest,
    cancellation_token: AgentCancellationToken,
    on_event: F,
) -> AgentResult<LlmChatResponse>
where
    F: FnMut(LlmStreamEvent) + Send,
{
    complete_chat_streaming_with_validation(
        request,
        cancellation_token,
        LlmResponseValidation::AllowEmpty,
        on_event,
    )
    .await
}

async fn complete_chat_streaming_with_validation<F>(
    request: LlmChatRequest,
    cancellation_token: AgentCancellationToken,
    validation: LlmResponseValidation,
    mut on_event: F,
) -> AgentResult<LlmChatResponse>
where
    F: FnMut(LlmStreamEvent) + Send,
{
    let mut request = request;
    request.stream = true;

    let mut last_error = None;
    let mut total_usage = None;
    for attempt in 1..=LLM_MAX_ATTEMPTS {
        on_event(LlmStreamEvent::AttemptStarted {
            attempt,
            max_attempts: LLM_MAX_ATTEMPTS,
        });
        let result = complete_chat_streaming_once(
            &request,
            cancellation_token.clone(),
            validation,
            &mut on_event,
        )
        .await;

        match result {
            Ok(mut response) => {
                merge_total_usage(&mut total_usage, response.usage.take());
                response.usage = total_usage;
                on_event(LlmStreamEvent::Committed);
                return Ok(response);
            }
            Err(error) if error.is_cancelled() => {
                return Err(merge_error_usage(error, &mut total_usage));
            }
            Err(error) => {
                let error = merge_error_usage(error, &mut total_usage);
                let reason = error.to_string();
                on_event(LlmStreamEvent::AttemptReset {
                    reason: reason.clone(),
                });
                if attempt >= LLM_MAX_ATTEMPTS || !is_retryable_llm_error(&error) {
                    return Err(retry_exhausted_error(error, attempt));
                }
                last_error = Some(error);
                on_event(LlmStreamEvent::Retrying {
                    attempt: attempt + 1,
                    max_attempts: LLM_MAX_ATTEMPTS,
                    reason,
                });
                wait_before_retry(attempt, cancellation_token.clone())
                    .await
                    .map_err(|error| error.with_usage(total_usage.clone()))?;
            }
        }
    }

    Err(retry_exhausted_error(
        last_error.unwrap_or_else(|| AgentError::new("模型请求失败。")),
        LLM_MAX_ATTEMPTS,
    ))
}

async fn complete_chat_streaming_once<F>(
    request: &LlmChatRequest,
    cancellation_token: AgentCancellationToken,
    validation: LlmResponseValidation,
    mut on_delta: F,
) -> AgentResult<LlmChatResponse>
where
    F: FnMut(LlmStreamEvent) + Send,
{
    let api_style = request.api_style;
    validate_request(request)?;
    let response = send_llm_request(request, cancellation_token.clone())
        .await
        .map_err(with_request_usage)?;
    if !is_sse_response(&response) {
        let body = response_text(response, cancellation_token.clone(), "读取模型响应失败")
            .await
            .map_err(with_request_usage)?;
        let parsed = parse_non_stream_response(&body, api_style, validation)?;
        if !parsed.content.is_empty() {
            on_delta(LlmStreamEvent::Delta(parsed.content.clone()));
        }
        return Ok(parsed);
    }

    let mut streamed = parse_sse_response(response, api_style, cancellation_token, on_delta)
        .await
        .map_err(with_request_usage)?;
    streamed.usage = Some(usage_for_request(streamed.usage));

    let diagnostic = streaming_response_diagnostic(&streamed);
    validate_llm_response(
        &streamed.content,
        &streamed.tool_calls,
        &diagnostic,
        validation,
    )
    .map_err(|error| error.with_usage(streamed.usage.clone()))?;
    Ok(streamed)
}

fn parse_non_stream_response(
    body: &str,
    api_style: AgentApiStyle,
    validation: LlmResponseValidation,
) -> AgentResult<LlmChatResponse> {
    let value: Value = serde_json::from_str(body).map_err(|error| {
        with_request_usage(AgentError::new(format!(
            "模型响应不是有效 JSON：{error}；原始响应：{}",
            truncate_for_error(body)
        )))
    })?;
    let usage = usage_for_request(extract_usage(&value));
    if let Some(error) = extract_api_error(&value) {
        return Err(AgentError::new(format!("模型接口返回错误：{error}")).with_usage(Some(usage)));
    }

    let tool_calls = extract_tool_calls(&value, api_style)
        .map_err(|error| error.with_usage(Some(usage.clone())))?;
    let content = extract_response_text(&value).unwrap_or_default();
    validate_llm_response(&content, &tool_calls, body, validation)
        .map_err(|error| error.with_usage(Some(usage.clone())))?;

    Ok(LlmChatResponse {
        content,
        tool_calls,
        usage: Some(usage),
        finish_reason: extract_finish_reason(&value),
    })
}

fn with_request_usage(error: AgentError) -> AgentError {
    let usage = usage_for_request(error.usage().cloned());
    error.with_usage(Some(usage))
}

fn merge_error_usage(error: AgentError, total: &mut Option<AgentUsage>) -> AgentError {
    merge_total_usage(total, error.usage().cloned());
    error.with_usage(total.clone())
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
    validation: LlmResponseValidation,
) -> AgentResult<()> {
    if validation == LlmResponseValidation::RequireModelAction
        && content.trim().is_empty()
        && tool_calls.is_empty()
    {
        return Err(AgentError::new(format!(
            "模型响应里没有可显示文本：{}",
            truncate_for_error(raw_response)
        )));
    }

    Ok(())
}

fn retry_exhausted_error(error: AgentError, attempts: usize) -> AgentError {
    if attempts <= 1 {
        return error;
    }

    let usage = error.usage().cloned();
    AgentError::new(format!("模型请求失败，已重试 {} 次：{error}", attempts - 1)).with_usage(usage)
}

async fn wait_before_retry(
    attempt: usize,
    cancellation_token: AgentCancellationToken,
) -> AgentResult<()> {
    let delay = retry_delay(attempt);
    tokio::select! {
        _ = cancellation_token.cancelled() => Err(AgentError::cancelled()),
        _ = tokio::time::sleep(delay) => Ok(()),
    }
}

fn retry_delay(attempt: usize) -> Duration {
    let multiplier = 1_u64 << attempt.saturating_sub(1).min(8);
    Duration::from_millis((LLM_RETRY_BASE_DELAY_MS * multiplier).min(LLM_RETRY_MAX_DELAY_MS))
}

fn is_retryable_llm_error(error: &AgentError) -> bool {
    let message = error.to_string().to_ascii_lowercase();
    if message.trim().is_empty() {
        return false;
    }

    if message.contains("请先在设置")
        || message.contains("请选择一个可用模型")
        || message.contains("没有可发送的对话内容")
        || message.contains("不支持的消息角色")
        || message.contains("api token")
        || message.contains("401")
        || message.contains("403")
        || message.contains("404")
        || message.contains("400")
    {
        return false;
    }

    if retryable_http_status_in_message(&message) {
        return true;
    }

    [
        "请求模型接口失败",
        "读取模型响应失败",
        "读取模型错误响应失败",
        "读取模型流失败",
        "模型响应不是有效 json",
        "模型流事件不是有效 json",
        "模型流不是有效 utf-8",
        "模型流尾部不是有效 utf-8",
        "error decoding response body",
        "connection",
        "connect",
        "timeout",
        "timed out",
        "deadline",
        "temporarily",
        "temporary",
        "overloaded",
        "unavailable",
        "try again",
        "rate limit",
        "rate_limit",
        "too many requests",
        "connection reset",
        "connection closed",
        "broken pipe",
        "unexpected eof",
        "incomplete message",
        "body write aborted",
    ]
    .iter()
    .any(|pattern| message.contains(pattern))
}

fn retryable_http_status_in_message(message: &str) -> bool {
    [408, 409, 425, 429, 500, 502, 503, 504, 520, 522, 524]
        .iter()
        .any(|status| message.contains(&status.to_string()))
}

fn streaming_response_diagnostic(response: &LlmChatResponse) -> String {
    let usage = response.usage.as_ref().map(|usage| {
        json!({
            "inputTokens": usage.input_tokens,
            "outputTokens": usage.output_tokens,
            "outputThinkingTokens": usage.output_thinking_tokens,
            "totalTokens": usage.total_tokens,
            "cachedInputTokens": usage.cached_input_tokens,
            "cacheCreationInputTokens": usage.cache_creation_input_tokens,
        })
    });

    serde_json::to_string(&json!({
        "type": "streaming_response",
        "finishReason": response.finish_reason,
        "contentLength": response.content.len(),
        "toolCallCount": response.tool_calls.len(),
        "usage": usage,
    }))
    .unwrap_or_else(|_| "streaming response".to_string())
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
        let mut context = ContextAssembler::assemble(ContextAssemblyInput {
            system_prompt: "System rules".to_string(),
            compaction_summary: None,
            messages: vec![
                timestamped_history,
                chat_message("assistant", "Earlier answer"),
                chat_message("user", "Continue the edit"),
            ],
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
            "[Message created at: {}]\nEarlier question",
            format_message_created_at(0).unwrap()
        );
        assert_eq!(
            openai["messages"][1]["content"],
            expected_timestamped_history
        );
        assert_eq!(openai["messages"][3]["content"], "Continue the edit");
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
            anthropic["messages"][2]["content"][0]["text"],
            "Continue the edit"
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
}
