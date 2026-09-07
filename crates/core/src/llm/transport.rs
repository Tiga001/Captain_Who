use super::provider_error::stream_inactivity_timeout_error;
use super::*;
use crate::provider_registration::ProviderUsageSemantics;
use futures_util::StreamExt;
use sha2::Digest;
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_PROVIDER_ERROR_RESPONSE_BYTES: usize = 16 * 1024;
pub(super) const LLM_STREAM_INACTIVITY_TIMEOUT: Duration = Duration::from_secs(60);

/// Every recoverable model failure shares one attempt budget and one exhaustion path.
pub(super) const LLM_MAX_ATTEMPTS: usize = 3;
pub(super) const LLM_RETRY_BASE_DELAY_MS: u64 = 350;
pub(super) const LLM_RETRY_MAX_DELAY_MS: u64 = 2_000;
pub(super) const LLM_RATE_LIMIT_RETRY_BASE_DELAY_MS: u64 = 2_000;
pub(super) const LLM_RATE_LIMIT_RETRY_MAX_DELAY_MS: u64 = 30_000;
pub(super) const LLM_OVERLOAD_RETRY_BASE_DELAY_MS: u64 = 1_000;
pub(super) const LLM_OVERLOAD_RETRY_MAX_DELAY_MS: u64 = 10_000;
pub(super) const LLM_MAX_TOTAL_RETRY_SLEEP_MS: u64 = 60_000;
pub(super) const LLM_LOGICAL_REQUEST_TIMEOUT: Duration = Duration::from_secs(180);
pub(crate) const EMPTY_MODEL_ACTION_ERROR_CODE: &str = "agent.empty_model_action";

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
    let usage_semantics = ProviderAdapterRegistry::resolve(&request)?
        .capabilities()
        .usage();

    let mut last_error = None;
    let mut total_usage = None;
    let mut total_retry_sleep = Duration::ZERO;
    let deadline = tokio::time::Instant::now() + LLM_LOGICAL_REQUEST_TIMEOUT;
    for attempt in 1..=LLM_MAX_ATTEMPTS {
        let result = tokio::select! {
            _ = tokio::time::sleep_until(deadline) => Err(logical_request_timeout_error()),
            result = complete_chat_once(&request, cancellation_token.clone(), validation) => result,
        };
        match result {
            Ok(mut response) => {
                merge_provider_attempt_usage(
                    &mut total_usage,
                    response.usage.take(),
                    usage_semantics,
                );
                response.usage = total_usage;
                return Ok(response);
            }
            Err(error) if error.is_cancelled() => {
                return Err(merge_error_usage(error, &mut total_usage, usage_semantics));
            }
            Err(error) => {
                let error = merge_error_usage(error, &mut total_usage, usage_semantics);
                let Some(plan) = retry_plan(
                    &error,
                    attempt,
                    total_retry_sleep,
                    deadline.saturating_duration_since(tokio::time::Instant::now()),
                ) else {
                    return Err(retry_exhausted_error(error, attempt));
                };
                register_retry_cooldown(&request, &plan);
                last_error = Some(error);
                total_retry_sleep += plan.delay;
                wait_before_retry(plan.delay, cancellation_token.clone())
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
    let provider_protocol = request.provider_protocol.clone();
    let provider_profile = request.provider_profile.clone();
    validate_request(request)?;
    let response = send_llm_request(request, cancellation_token.clone())
        .await
        .map_err(with_request_usage)?;
    let body = response_text(response, cancellation_token.clone(), "读取模型响应失败")
        .await
        .map_err(with_request_usage)?;
    parse_non_stream_response_with_profile(&body, &provider_profile, &provider_protocol, validation)
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
    on_event: F,
) -> AgentResult<LlmChatResponse>
where
    F: FnMut(LlmStreamEvent) + Send,
{
    complete_chat_streaming_with_validation_and_timeout(
        request,
        cancellation_token,
        validation,
        LLM_STREAM_INACTIVITY_TIMEOUT,
        on_event,
    )
    .await
}

pub(super) async fn complete_chat_streaming_with_validation_and_timeout<F>(
    request: LlmChatRequest,
    cancellation_token: AgentCancellationToken,
    validation: LlmResponseValidation,
    inactivity_timeout: Duration,
    mut on_event: F,
) -> AgentResult<LlmChatResponse>
where
    F: FnMut(LlmStreamEvent) + Send,
{
    let mut request = request;
    request.stream = true;
    let usage_semantics = ProviderAdapterRegistry::resolve(&request)?
        .capabilities()
        .usage();

    let mut last_error = None;
    let mut total_usage = None;
    let mut total_retry_sleep = Duration::ZERO;
    for attempt in 1..=LLM_MAX_ATTEMPTS {
        on_event(LlmStreamEvent::AttemptStarted {
            attempt,
            max_attempts: LLM_MAX_ATTEMPTS,
        });
        let result = complete_chat_streaming_once(
            &request,
            cancellation_token.clone(),
            validation,
            inactivity_timeout,
            |event| {
                on_event(event);
            },
        )
        .await;

        match result {
            Ok(mut response) => {
                merge_provider_attempt_usage(
                    &mut total_usage,
                    response.usage.take(),
                    usage_semantics,
                );
                response.usage = total_usage;
                on_event(LlmStreamEvent::Committed);
                return Ok(response);
            }
            Err(error) if error.is_cancelled() => {
                return Err(merge_error_usage(error, &mut total_usage, usage_semantics));
            }
            Err(error) => {
                let error = merge_error_usage(error, &mut total_usage, usage_semantics);
                let reason = safe_retry_reason(&error);
                on_event(LlmStreamEvent::AttemptReset {
                    reason: reason.clone(),
                });
                let Some(plan) = retry_plan(&error, attempt, total_retry_sleep, Duration::MAX)
                else {
                    return Err(retry_exhausted_error(error, attempt));
                };
                register_retry_cooldown(&request, &plan);
                last_error = Some(error);
                on_event(LlmStreamEvent::Retrying {
                    attempt: attempt + 1,
                    max_attempts: plan.max_attempts,
                    category: plan.category.as_str().to_string(),
                    provider_code: plan.provider_code.clone(),
                    delay_ms: duration_ms(plan.delay),
                    retry_at: plan.retry_at,
                });
                total_retry_sleep += plan.delay;
                wait_before_retry(plan.delay, cancellation_token.clone())
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
    inactivity_timeout: Duration,
    mut on_delta: F,
) -> AgentResult<LlmChatResponse>
where
    F: FnMut(LlmStreamEvent) + Send,
{
    let provider_protocol = request.provider_protocol.clone();
    let provider_profile = request.provider_profile.clone();
    validate_request(request)?;
    let (response, inactivity_deadline) = send_llm_request_with_stream_timeout(
        request,
        cancellation_token.clone(),
        inactivity_timeout,
    )
    .await
    .map_err(with_request_usage)?;
    if !is_sse_response(&response) {
        let body = response_text_with_inactivity_timeout(
            response,
            cancellation_token.clone(),
            "读取模型响应失败",
            Some((inactivity_timeout, inactivity_deadline)),
        )
        .await
        .map_err(with_request_usage)?;
        let parsed = parse_non_stream_response_with_profile(
            &body,
            &provider_profile,
            &provider_protocol,
            validation,
        )?;
        if !parsed.content().is_empty() {
            on_delta(LlmStreamEvent::Delta(parsed.content().to_string()));
        }
        return Ok(parsed);
    }

    let mut streamed = parse_sse_response(
        response,
        &provider_profile,
        &provider_protocol,
        cancellation_token,
        inactivity_timeout,
        inactivity_deadline,
        on_delta,
    )
    .await
    .map_err(with_request_usage)?;
    streamed.usage = Some(usage_for_request(streamed.usage));

    let diagnostic = streaming_response_diagnostic(&streamed);
    let has_private_model_action = ProviderAdapterRegistry::resolve_key(&provider_protocol)?
        .has_private_model_action(&provider_protocol, &streamed.assistant_turn)
        .map_err(|error| error.with_usage(streamed.usage.clone()))?;
    validate_llm_response(
        streamed.content(),
        streamed.provider_tool_calls(),
        has_private_model_action,
        &diagnostic,
        streamed.finish_reason.as_deref(),
        validation,
    )
    .map_err(|error| error.with_usage(streamed.usage.clone()))?;
    Ok(streamed)
}

#[cfg(test)]
pub(super) fn parse_non_stream_response(
    body: &str,
    provider_protocol: &ProviderProtocolKey,
    validation: LlmResponseValidation,
) -> AgentResult<LlmChatResponse> {
    use crate::provider_profile::{
        MoonshotK26ThinkingMode, ProviderFamilyReasoningPolicy, ProviderFamilySettings,
        ProviderProfileId, ProviderReasoningEffort, ProviderVendorId,
    };
    let provider_profile = match provider_protocol.profile.id {
        ProviderProfileId::GenericOpenAiChat | ProviderProfileId::GenericAnthropicMessages => {
            crate::ProviderProfileConfig::generic_for_dialect(provider_protocol.dialect)
        }
        ProviderProfileId::DeepSeekV4Chat => crate::ProviderProfileConfig::deepseek_v4_default(),
        ProviderProfileId::DeepSeekV4Vision => crate::ProviderProfileConfig::from_family_settings(
            provider_protocol.profile,
            ProviderVendorId::DeepSeek,
            ProviderFamilySettings::DeepseekV4Vision {
                reasoning: ProviderFamilyReasoningPolicy::provider_default(),
            },
        ),
        ProviderProfileId::MoonshotK3Chat => crate::ProviderProfileConfig::from_family_settings(
            provider_protocol.profile,
            ProviderVendorId::Moonshot,
            ProviderFamilySettings::MoonshotK3Chat {
                reasoning_effort: ProviderReasoningEffort::Max,
            },
        ),
        ProviderProfileId::MoonshotK27CodeChat => {
            crate::ProviderProfileConfig::from_family_settings(
                provider_protocol.profile,
                ProviderVendorId::Moonshot,
                ProviderFamilySettings::MoonshotK27CodeChat,
            )
        }
        ProviderProfileId::MoonshotK26Chat => crate::ProviderProfileConfig::from_family_settings(
            provider_protocol.profile,
            ProviderVendorId::Moonshot,
            ProviderFamilySettings::MoonshotK26Chat {
                thinking_mode: MoonshotK26ThinkingMode::ProviderDefault,
            },
        ),
        _ => {
            return Err(AgentError::new(
                "Provider profile 未注册，无法构造测试 response parser。",
            ));
        }
    };
    parse_non_stream_response_with_profile(body, &provider_profile, provider_protocol, validation)
}

pub(super) fn parse_non_stream_response_with_profile(
    body: &str,
    provider_profile: &crate::provider_profile::ProviderProfileConfig,
    provider_protocol: &ProviderProtocolKey,
    validation: LlmResponseValidation,
) -> AgentResult<LlmChatResponse> {
    provider_protocol
        .validate_against_config(provider_profile)
        .map_err(|error| AgentError::new(format!("Provider profile 设置无效：{error}")))?;
    let value: Value = serde_json::from_str(body).map_err(|error| {
        with_request_usage(
            LlmProviderFailure::from_local_transport_failure(&format!(
                "model response was not valid JSON: {error}; body_hash=sha256:{:x}",
                sha2::Sha256::digest(body.as_bytes())
            ))
            .to_agent_error(),
        )
    })?;
    let adapter = ProviderAdapterRegistry::resolve_key(provider_protocol)?;
    let usage = usage_for_request(adapter.project_usage(provider_profile, &value));
    if let Some(error) = extract_api_error(&value) {
        let _ = error;
        return Err(
            LlmProviderFailure::from_embedded_error_for_protocol(provider_protocol, body)
                .to_agent_error()
                .with_usage(Some(usage)),
        );
    }

    let assistant_turn = adapter
        .parse_non_streaming_response(provider_profile, provider_protocol, &value)
        .map_err(|error| error.with_usage(Some(usage.clone())))?;
    let finish_reason = extract_finish_reason(&value);
    let has_private_model_action = adapter
        .has_private_model_action(provider_protocol, &assistant_turn)
        .map_err(|error| error.with_usage(Some(usage.clone())))?;
    validate_llm_response(
        assistant_turn.visible_text(),
        assistant_turn.provider_tool_calls(),
        has_private_model_action,
        body,
        finish_reason.as_deref(),
        validation,
    )
    .map_err(|error| error.with_usage(Some(usage.clone())))?;

    Ok(LlmChatResponse {
        assistant_turn,
        usage: Some(usage),
        finish_reason,
    })
}

pub(super) fn with_request_usage(error: AgentError) -> AgentError {
    let usage = usage_for_request(error.usage().cloned());
    error.with_usage(Some(usage))
}

fn merge_provider_attempt_usage(
    total: &mut Option<AgentUsage>,
    next: Option<AgentUsage>,
    usage_semantics: ProviderUsageSemantics,
) {
    usage_semantics.merge_usage(total, next);
}

pub(super) fn merge_error_usage(
    error: AgentError,
    total: &mut Option<AgentUsage>,
    usage_semantics: ProviderUsageSemantics,
) -> AgentError {
    merge_provider_attempt_usage(total, error.usage().cloned(), usage_semantics);
    error.with_usage(total.clone())
}

pub(super) async fn send_llm_request(
    request: &LlmChatRequest,
    cancellation_token: AgentCancellationToken,
) -> AgentResult<reqwest::Response> {
    let (response, _) = send_llm_request_with_stream_timeout(
        request,
        cancellation_token,
        LLM_STREAM_INACTIVITY_TIMEOUT,
    )
    .await?;
    Ok(response)
}

async fn send_llm_request_with_stream_timeout(
    request: &LlmChatRequest,
    cancellation_token: AgentCancellationToken,
    inactivity_timeout: Duration,
) -> AgentResult<(reqwest::Response, tokio::time::Instant)> {
    cancellation_token.check()?;
    validate_request(request)?;
    let adapter = ProviderAdapterRegistry::resolve(request)?;
    let payload = adapter.prepare_request(request)?;
    let headers = adapter.build_headers(request.api_token.trim())?;
    let client_builder = provider_client_builder(request.api_url.trim());
    let client_builder = if request.stream {
        client_builder
    } else {
        client_builder.timeout(Duration::from_secs(120))
    };
    let client = client_builder
        .build()
        .map_err(|error| AgentError::new(format!("创建 HTTP 客户端失败：{error}")))?;

    let cooldown_permit = acquire_provider_cooldown(
        request.api_url.trim(),
        request.api_token.trim(),
        request.api_style(),
        cancellation_token.clone(),
    )
    .await?;
    super::request_fingerprint::log_request_fingerprint_if_enabled(&payload);
    let send = client
        .post(request.api_url.trim())
        .headers(headers)
        .json(&payload)
        .send();
    // Client construction may read the native trust store and system proxy configuration. That is
    // setup work, not Provider inactivity. Start the single header/body window only when the HTTP
    // request is ready to be polled.
    let inactivity_deadline = tokio::time::Instant::now() + inactivity_timeout;
    let response = if request.stream {
        tokio::select! {
            _ = cancellation_token.cancelled() => {
                complete_provider_cooldown(cooldown_permit);
                return Err(AgentError::cancelled());
            },
            _ = tokio::time::sleep_until(inactivity_deadline) => {
                complete_provider_cooldown(cooldown_permit);
                return Err(stream_inactivity_timeout_error(
                    inactivity_timeout,
                    "response_headers",
                ));
            },
            response = send => response,
        }
    } else {
        tokio::select! {
            _ = cancellation_token.cancelled() => {
                complete_provider_cooldown(cooldown_permit);
                return Err(AgentError::cancelled());
            },
            response = send => response,
        }
    };
    let response = match response {
        Ok(response) => response,
        Err(error) => {
            complete_provider_cooldown(cooldown_permit);
            return Err(LlmProviderFailure::from_network_error(error).to_agent_error());
        }
    };

    let status = response.status();
    if !status.is_success() {
        let response_headers = response.headers().clone();
        let body = match error_response_text_bounded(
            response,
            cancellation_token.clone(),
            request
                .stream
                .then_some((inactivity_timeout, inactivity_deadline)),
        )
        .await
        {
            Ok(body) => body,
            Err(error) if error.is_cancelled() => {
                complete_provider_cooldown(cooldown_permit);
                return Err(error);
            }
            // Status and headers are already authoritative. A broken error body must not erase a
            // 429 or its Retry-After and turn it into an aggressively retried network failure.
            Err(_) => String::new(),
        };
        let failure = LlmProviderFailure::from_http_response_for_protocol(
            &request.provider_protocol,
            status,
            &response_headers,
            &body,
            request.api_token.trim(),
        );
        if failure.category == LlmProviderFailureCategory::RateLimited {
            register_default_rate_limit_cooldown(
                request.api_url.trim(),
                request.api_token.trim(),
                request.api_style(),
                failure.retry_after_ms,
            );
        } else {
            complete_provider_cooldown(cooldown_permit);
        }
        return Err(failure.to_agent_error());
    }

    complete_provider_cooldown(cooldown_permit);
    Ok((response, inactivity_deadline))
}

/// Local model endpoints must never be routed through a desktop/VPN proxy. Besides avoiding an
/// unnecessary trust boundary, this keeps loopback providers usable when macOS system proxy
/// resolution is slow or does not carry the expected bypass list.
fn provider_client_builder(api_url: &str) -> reqwest::ClientBuilder {
    // Provider credentials are scoped to the exact endpoint selected by the Host. Never let an
    // upstream 3xx response replay them to another URL (or downgrade an HTTPS request to HTTP).
    // The Provider response remains visible to the normal status/error classifier instead.
    let builder = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none());
    if provider_url_is_loopback(api_url) {
        // A loopback endpoint neither needs the system proxy nor enterprise Keychain roots. Keep
        // the bundled WebPKI roots available for an explicitly TLS-enabled local endpoint while
        // avoiding synchronous native proxy/certificate discovery on its request path.
        builder.no_proxy().tls_built_in_native_certs(false)
    } else {
        builder
    }
}

fn provider_url_is_loopback(api_url: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(api_url) else {
        return false;
    };
    let Some(host) = url.host_str() else {
        return false;
    };
    let host = host.trim_end_matches('.');
    host.eq_ignore_ascii_case("localhost")
        || host.to_ascii_lowercase().ends_with(".localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

pub(super) async fn response_text(
    response: reqwest::Response,
    cancellation_token: AgentCancellationToken,
    error_prefix: &str,
) -> AgentResult<String> {
    response_text_with_inactivity_timeout(response, cancellation_token, error_prefix, None).await
}

async fn response_text_with_inactivity_timeout(
    response: reqwest::Response,
    cancellation_token: AgentCancellationToken,
    error_prefix: &str,
    inactivity_window: Option<(Duration, tokio::time::Instant)>,
) -> AgentResult<String> {
    cancellation_token.check()?;
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    loop {
        let chunk = if let Some((inactivity_timeout, inactivity_deadline)) = inactivity_window {
            tokio::select! {
                _ = cancellation_token.cancelled() => return Err(AgentError::cancelled()),
                _ = tokio::time::sleep_until(inactivity_deadline) => {
                    return Err(stream_inactivity_timeout_error(
                        inactivity_timeout,
                        "response_body",
                    ));
                },
                chunk = stream.next() => chunk,
            }
        } else {
            tokio::select! {
                _ = cancellation_token.cancelled() => return Err(AgentError::cancelled()),
                chunk = stream.next() => chunk,
            }
        };
        let Some(chunk) = chunk else {
            break;
        };
        let chunk = chunk.map_err(|error| {
            let failure = LlmProviderFailure::from_network_error(error);
            let mut agent_error = failure.to_agent_error();
            if let Some(details) = agent_error.details().cloned() {
                agent_error = AgentError::structured(
                    PROVIDER_FAILURE_ERROR_CODE,
                    format!("{error_prefix}：模型服务网络响应失败。"),
                    details,
                );
            }
            agent_error
        })?;
        body.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8_lossy(&body).into_owned())
}

async fn error_response_text_bounded(
    response: reqwest::Response,
    cancellation_token: AgentCancellationToken,
    inactivity_window: Option<(Duration, tokio::time::Instant)>,
) -> AgentResult<String> {
    cancellation_token.check()?;
    let mut stream = response.bytes_stream();
    let mut body = Vec::with_capacity(MAX_PROVIDER_ERROR_RESPONSE_BYTES.min(4 * 1024));
    while body.len() < MAX_PROVIDER_ERROR_RESPONSE_BYTES {
        let chunk = if let Some((inactivity_timeout, inactivity_deadline)) = inactivity_window {
            tokio::select! {
                _ = cancellation_token.cancelled() => return Err(AgentError::cancelled()),
                _ = tokio::time::sleep_until(inactivity_deadline) => {
                    return Err(stream_inactivity_timeout_error(
                        inactivity_timeout,
                        "error_response_body",
                    ));
                },
                chunk = stream.next() => chunk,
            }
        } else {
            tokio::select! {
                _ = cancellation_token.cancelled() => return Err(AgentError::cancelled()),
                chunk = stream.next() => chunk,
            }
        };
        let Some(chunk) = chunk else {
            break;
        };
        let chunk = chunk
            .map_err(|error| LlmProviderFailure::from_network_error(error).to_agent_error())?;
        let remaining = MAX_PROVIDER_ERROR_RESPONSE_BYTES - body.len();
        body.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
        if chunk.len() > remaining {
            break;
        }
    }
    Ok(String::from_utf8_lossy(&body).into_owned())
}

pub(super) fn validate_request(request: &LlmChatRequest) -> AgentResult<()> {
    if request.api_url.trim().is_empty() {
        return Err(AgentError::new("请先在设置 > 配置里填写 API URL。"));
    }
    if request.api_token.trim().is_empty() {
        return Err(AgentError::new("请先在设置 > 配置里填写 API Token。"));
    }
    if request.model().trim().is_empty() {
        return Err(AgentError::new("请选择一个可用模型。"));
    }
    if request.messages.is_empty() {
        return Err(AgentError::new("没有可发送的对话内容。"));
    }
    ProviderAdapterRegistry::resolve(request)?.validate_wire_protocol(request)?;
    for tool in &request.tools {
        validate_portable_tool_input_schema(&tool.name, &tool.input_schema)?;
    }

    Ok(())
}

pub(super) fn validate_llm_response(
    content: &str,
    tool_calls: &[LlmToolCall],
    has_private_model_action: bool,
    raw_response: &str,
    finish_reason: Option<&str>,
    validation: LlmResponseValidation,
) -> AgentResult<()> {
    if validation == LlmResponseValidation::RequireModelAction
        && content.trim().is_empty()
        && tool_calls.is_empty()
        && !has_private_model_action
    {
        let repairable = matches!(finish_reason, Some("stop" | "end_turn"));
        return Err(AgentError::structured(
            EMPTY_MODEL_ACTION_ERROR_CODE,
            "模型响应里没有可显示文本或工具调用。",
            json!({
                "type": "model_response_validation",
                "code": "emptyModelAction",
                "finishReason": finish_reason,
                "contentLength": content.len(),
                "toolCallCount": tool_calls.len(),
                "responseBytes": raw_response.len(),
                "responseHash": format!("sha256:{:x}", sha2::Sha256::digest(raw_response.as_bytes())),
                "repairable": repairable,
                "recovery": if repairable {
                    "repairRetryOnce"
                } else {
                    "retryOrChangeModel"
                },
            }),
        ));
    }

    Ok(())
}

pub(crate) fn is_repairable_empty_model_action(error: &AgentError) -> bool {
    error.code() == Some(EMPTY_MODEL_ACTION_ERROR_CODE)
        && error
            .details()
            .and_then(|details| details.get("repairable"))
            .and_then(Value::as_bool)
            == Some(true)
}

pub(super) fn retry_exhausted_error(error: AgentError, attempts: usize) -> AgentError {
    if attempts <= 1 {
        return error;
    }

    let usage = error.usage().cloned();
    let message = format!(
        "模型请求失败，已重试 {} 次：{}",
        attempts - 1,
        safe_retry_reason(&error)
    );
    if let (Some(code), Some(details)) = (error.code(), error.details()) {
        AgentError::structured(code, message, details.clone()).with_usage(usage)
    } else {
        AgentError::new(message).with_usage(usage)
    }
}

pub(super) async fn wait_before_retry(
    delay: Duration,
    cancellation_token: AgentCancellationToken,
) -> AgentResult<()> {
    tokio::select! {
        _ = cancellation_token.cancelled() => Err(AgentError::cancelled()),
        _ = tokio::time::sleep(delay) => Ok(()),
    }
}

#[derive(Debug, Clone)]
pub(super) struct LlmRetryPlan {
    pub category: LlmProviderFailureCategory,
    pub provider_code: Option<String>,
    pub max_attempts: usize,
    pub delay: Duration,
    pub retry_at: u64,
}

pub(super) fn retry_plan(
    error: &AgentError,
    attempt: usize,
    total_retry_sleep: Duration,
    wall_clock_remaining: Duration,
) -> Option<LlmRetryPlan> {
    if attempt >= LLM_MAX_ATTEMPTS || !is_retryable_llm_error(error) {
        return None;
    }
    let (category, retry_after_ms, provider_code) = if is_invalid_stream_tool_arguments_error(error)
    {
        (
            LlmProviderFailureCategory::Unknown,
            None,
            Some("invalid_stream_tool_arguments".to_string()),
        )
    } else {
        provider_failure_metadata(error)
            .map(|(category, _, retry_after_ms, provider_code)| {
                (category, retry_after_ms, provider_code)
            })
            .unwrap_or((LlmProviderFailureCategory::Unknown, None, None))
    };
    let delay = retry_after_ms.map_or_else(
        || positive_retry_jitter(retry_delay_for_category(category, attempt), attempt),
        Duration::from_millis,
    );
    let sleep_budget = Duration::from_millis(LLM_MAX_TOTAL_RETRY_SLEEP_MS);
    if delay > sleep_budget.saturating_sub(total_retry_sleep) || delay >= wall_clock_remaining {
        return None;
    }
    Some(LlmRetryPlan {
        category,
        provider_code,
        max_attempts: LLM_MAX_ATTEMPTS,
        delay,
        retry_at: unix_epoch_ms().saturating_add(duration_ms(delay)),
    })
}

fn register_retry_cooldown(request: &LlmChatRequest, plan: &LlmRetryPlan) {
    if plan.category == LlmProviderFailureCategory::RateLimited {
        register_provider_cooldown(
            request.api_url.trim(),
            request.api_token.trim(),
            request.api_style(),
            plan.delay,
        );
    }
}

pub(super) fn retry_delay_for_category(
    category: LlmProviderFailureCategory,
    attempt: usize,
) -> Duration {
    let (base, cap) = match category {
        LlmProviderFailureCategory::RateLimited => (
            LLM_RATE_LIMIT_RETRY_BASE_DELAY_MS,
            LLM_RATE_LIMIT_RETRY_MAX_DELAY_MS,
        ),
        LlmProviderFailureCategory::Overloaded => (
            LLM_OVERLOAD_RETRY_BASE_DELAY_MS,
            LLM_OVERLOAD_RETRY_MAX_DELAY_MS,
        ),
        _ => (LLM_RETRY_BASE_DELAY_MS, LLM_RETRY_MAX_DELAY_MS),
    };
    let multiplier = 1_u64 << attempt.saturating_sub(1).min(8);
    Duration::from_millis(base.saturating_mul(multiplier).min(cap))
}

fn positive_retry_jitter(delay: Duration, attempt: usize) -> Duration {
    let delay_ms = duration_ms(delay);
    let jitter_ceiling = delay_ms / 4;
    if jitter_ceiling == 0 {
        return delay;
    }
    let entropy = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| u64::try_from(duration.as_nanos()).ok())
        .unwrap_or(0)
        ^ (attempt as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    Duration::from_millis(delay_ms.saturating_add(entropy % (jitter_ceiling + 1)))
}

fn duration_ms(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn safe_retry_reason(error: &AgentError) -> String {
    if is_invalid_stream_tool_arguments_error(error) {
        return "模型返回的流式工具参数不完整或格式无效。".to_string();
    }

    public_provider_error_message(error).unwrap_or_else(|| {
        if error.is_cancelled() {
            "模型请求已取消。".to_string()
        } else {
            "模型请求暂时失败，正在按退避策略重试。".to_string()
        }
    })
}

fn logical_request_timeout_error() -> AgentError {
    AgentError::structured(
        PROVIDER_FAILURE_ERROR_CODE,
        "模型请求超过总时间预算，已停止继续等待。",
        json!({
            "type": "llm_provider_failure",
            "category": LlmProviderFailureCategory::Network,
            "retryable": true,
            "retryAfterMs": null,
            "httpStatus": null,
            "providerCode": null,
            "requestId": null,
            "bodyBytes": 0,
            "bodyHash": null,
            "bodyArchived": false,
        }),
    )
}

#[cfg(test)]
pub(super) fn retry_delay(attempt: usize) -> Duration {
    retry_delay_for_category(LlmProviderFailureCategory::Unknown, attempt)
}

pub(super) fn is_retryable_llm_error(error: &AgentError) -> bool {
    // An empty normal model action is repaired by the agent loop with an explicit semantic
    // instruction. Never let transport-level text heuristics replay the original request first:
    // the redacted raw response can itself contain words such as "timeout" or "overloaded".
    if error.code() == Some(EMPTY_MODEL_ACTION_ERROR_CODE) {
        return false;
    }

    if is_invalid_stream_tool_arguments_error(error) {
        return true;
    }

    provider_failure_metadata(error).is_some_and(|(_, retryable, _, _)| retryable)
}

fn is_invalid_stream_tool_arguments_error(error: &AgentError) -> bool {
    error.code() == Some(INVALID_STREAM_TOOL_ARGUMENTS_ERROR_CODE)
        && error
            .details()
            .and_then(|details| details.get("type"))
            .and_then(Value::as_str)
            == Some("invalid_stream_tool_arguments")
        && error
            .details()
            .and_then(|details| details.get("retryable"))
            .and_then(Value::as_bool)
            == Some(true)
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
        "contentLength": response.content().len(),
        "toolCallCount": response.provider_tool_calls().len(),
        "usage": usage,
    }))
    .unwrap_or_else(|_| "streaming response".to_string())
}
