use super::*;

pub(super) const LLM_MAX_ATTEMPTS: usize = 3;
pub(super) const LLM_RETRY_BASE_DELAY_MS: u64 = 350;
pub(super) const LLM_RETRY_MAX_DELAY_MS: u64 = 2_000;
pub(super) const RETRYABLE_UPSTREAM_CONTENT_TYPE_ERROR: &str =
    "the provided content type is invalid or not supported for this model";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LlmResponseValidation {
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

pub(super) async fn complete_chat_with_validation(
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

pub(super) async fn complete_chat_once(
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

pub(super) async fn complete_chat_streaming_with_validation<F>(
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

pub(super) async fn complete_chat_streaming_once<F>(
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

pub(super) fn parse_non_stream_response(
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

pub(super) fn with_request_usage(error: AgentError) -> AgentError {
    let usage = usage_for_request(error.usage().cloned());
    error.with_usage(Some(usage))
}

pub(super) fn merge_error_usage(error: AgentError, total: &mut Option<AgentUsage>) -> AgentError {
    merge_total_usage(total, error.usage().cloned());
    error.with_usage(total.clone())
}

pub(super) async fn send_llm_request(
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

pub(super) async fn response_text(
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

pub(super) fn validate_request(request: &LlmChatRequest) -> AgentResult<()> {
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
    for tool in &request.tools {
        validate_portable_tool_input_schema(&tool.name, &tool.input_schema)?;
    }

    Ok(())
}

pub(super) fn validate_llm_response(
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

pub(super) fn retry_exhausted_error(error: AgentError, attempts: usize) -> AgentError {
    if attempts <= 1 {
        return error;
    }

    let usage = error.usage().cloned();
    AgentError::new(format!("模型请求失败，已重试 {} 次：{error}", attempts - 1)).with_usage(usage)
}

pub(super) async fn wait_before_retry(
    attempt: usize,
    cancellation_token: AgentCancellationToken,
) -> AgentResult<()> {
    let delay = retry_delay(attempt);
    tokio::select! {
        _ = cancellation_token.cancelled() => Err(AgentError::cancelled()),
        _ = tokio::time::sleep(delay) => Ok(()),
    }
}

pub(super) fn retry_delay(attempt: usize) -> Duration {
    let multiplier = 1_u64 << attempt.saturating_sub(1).min(8);
    Duration::from_millis((LLM_RETRY_BASE_DELAY_MS * multiplier).min(LLM_RETRY_MAX_DELAY_MS))
}

pub(super) fn is_retryable_llm_error(error: &AgentError) -> bool {
    let message = error.to_string().to_ascii_lowercase();
    if message.trim().is_empty() {
        return false;
    }

    // Some OpenAI-compatible gateways intermittently fail while routing an otherwise valid
    // JSON request to Bedrock. Keep this exception exact so other client-side 400s still fail fast.
    if message.contains("400") && message.contains(RETRYABLE_UPSTREAM_CONTENT_TYPE_ERROR) {
        return true;
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

pub(super) fn retryable_http_status_in_message(message: &str) -> bool {
    [408, 409, 425, 429, 500, 502, 503, 504, 520, 522, 524]
        .iter()
        .any(|status| message.contains(&status.to_string()))
}

pub(super) fn streaming_response_diagnostic(response: &LlmChatResponse) -> String {
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
