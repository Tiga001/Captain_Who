use super::*;
use reqwest::header::{HeaderMap, RETRY_AFTER};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use time::format_description::well_known::Rfc2822;
use time::OffsetDateTime;

pub(super) const PROVIDER_FAILURE_ERROR_CODE: &str = "agent.llm_provider_failure";

const MAX_ERROR_BODY_BYTES: usize = 4 * 1024;
const MAX_HEADER_VALUE_BYTES: usize = 512;
const MAX_PROVIDER_CODE_BYTES: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum LlmProviderFailureCategory {
    RateLimited,
    QuotaExhausted,
    Overloaded,
    Authentication,
    InvalidRequest,
    ContextTooLarge,
    Network,
    Unknown,
}

impl LlmProviderFailureCategory {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::RateLimited => "rate_limited",
            Self::QuotaExhausted => "quota_exhausted",
            Self::Overloaded => "overloaded",
            Self::Authentication => "authentication",
            Self::InvalidRequest => "invalid_request",
            Self::ContextTooLarge => "context_too_large",
            Self::Network => "network",
            Self::Unknown => "unknown",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "rate_limited" => Some(Self::RateLimited),
            "quota_exhausted" => Some(Self::QuotaExhausted),
            "overloaded" => Some(Self::Overloaded),
            "authentication" => Some(Self::Authentication),
            "invalid_request" => Some(Self::InvalidRequest),
            "context_too_large" => Some(Self::ContextTooLarge),
            "network" => Some(Self::Network),
            "unknown" => Some(Self::Unknown),
            _ => None,
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct LlmProviderRawError {
    pub http_status: Option<u16>,
    pub headers: BTreeMap<String, String>,
    pub body: String,
    pub request_id: Option<String>,
    pub provider_code: Option<String>,
}

impl std::fmt::Debug for LlmProviderRawError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LlmProviderRawError")
            .field("http_status", &self.http_status)
            .field("header_count", &self.headers.len())
            .field("body_bytes", &self.body.len())
            .field("has_request_id", &self.request_id.is_some())
            .field("has_provider_code", &self.provider_code.is_some())
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct LlmProviderFailure {
    pub category: LlmProviderFailureCategory,
    pub retryable: bool,
    pub retry_after_ms: Option<u64>,
    pub raw: LlmProviderRawError,
}

impl std::fmt::Debug for LlmProviderFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LlmProviderFailure")
            .field("category", &self.category)
            .field("retryable", &self.retryable)
            .field("retry_after_ms", &self.retry_after_ms)
            .field("raw", &self.raw)
            .finish()
    }
}

impl LlmProviderFailure {
    pub(super) fn from_http_response(
        api_style: AgentApiStyle,
        status: reqwest::StatusCode,
        headers: &HeaderMap,
        body: &str,
        api_token: &str,
    ) -> Self {
        let provider_code = extract_provider_code(body);
        let retry_after_ms = retry_after_ms(headers);
        let category = classify_provider_failure(
            api_style,
            Some(status.as_u16()),
            provider_code.as_deref(),
            body,
        );
        let retryable = retryable_category(category, Some(status.as_u16()), body);
        let sanitized_headers = sanitized_diagnostic_headers(headers, api_token);
        let request_id = request_id_from_headers(&sanitized_headers);
        let sanitized_body = sanitize_error_body(body, api_token);

        Self {
            category,
            retryable,
            retry_after_ms,
            raw: LlmProviderRawError {
                http_status: Some(status.as_u16()),
                headers: sanitized_headers,
                body: sanitized_body,
                request_id,
                provider_code,
            },
        }
    }

    pub(super) fn from_embedded_error(api_style: AgentApiStyle, body: &str) -> Self {
        let provider_code = extract_provider_code(body);
        let category = classify_provider_failure(api_style, None, provider_code.as_deref(), body);
        let retryable = retryable_category(category, None, body);
        Self {
            category,
            retryable,
            retry_after_ms: None,
            raw: LlmProviderRawError {
                http_status: None,
                headers: BTreeMap::new(),
                // A HTTP-200 error envelope is discovered after transport has released its
                // credential-scoped request state. Retain only its structured discriminator,
                // never an arbitrary provider message which could echo request content.
                body: embedded_error_projection(body),
                request_id: None,
                provider_code,
            },
        }
    }

    pub(super) fn from_network_error(error: reqwest::Error) -> Self {
        let retryable = error.is_timeout()
            || error.is_connect()
            || error.is_request()
            || error.is_body()
            || error.is_decode();
        let diagnostic = if error.is_timeout() {
            "network request timed out".to_string()
        } else if error.is_connect() {
            "network connection failed".to_string()
        } else {
            // A reqwest error can contain the request URL. Strip it before retaining a bounded
            // diagnostic so query-string credentials cannot enter traces or UI errors.
            error.without_url().to_string()
        };
        Self {
            category: LlmProviderFailureCategory::Network,
            retryable,
            retry_after_ms: None,
            raw: LlmProviderRawError {
                http_status: None,
                headers: BTreeMap::new(),
                body: bounded_utf8(&diagnostic, MAX_ERROR_BODY_BYTES),
                request_id: None,
                provider_code: None,
            },
        }
    }

    pub(super) fn from_local_transport_failure(diagnostic: &str) -> Self {
        Self {
            category: LlmProviderFailureCategory::Network,
            retryable: true,
            retry_after_ms: None,
            raw: LlmProviderRawError {
                http_status: None,
                headers: BTreeMap::new(),
                body: bounded_utf8(diagnostic, MAX_ERROR_BODY_BYTES),
                request_id: None,
                provider_code: None,
            },
        }
    }

    pub(super) fn to_agent_error(&self) -> AgentError {
        let status_suffix = self
            .raw
            .http_status
            .map_or_else(String::new, |status| format!("（HTTP {status}）"));
        let body_digest = format!("sha256:{:x}", Sha256::digest(self.raw.body.as_bytes()));
        let request_id = self.raw.request_id.as_deref().map(opaque_identifier_digest);
        let provider_code = self
            .raw
            .provider_code
            .as_deref()
            .and_then(canonical_provider_code);
        AgentError::structured(
            PROVIDER_FAILURE_ERROR_CODE,
            format!("{}{status_suffix}", public_category_message(self.category)),
            json!({
                "type": "llm_provider_failure",
                "category": self.category,
                "retryable": self.retryable,
                "retryAfterMs": self.retry_after_ms,
                "httpStatus": self.raw.http_status,
                "providerCode": provider_code,
                "requestId": request_id,
                "bodyBytes": self.raw.body.len(),
                "bodyHash": body_digest,
                "bodyArchived": false,
            }),
        )
    }
}

fn canonical_provider_code(value: &str) -> Option<String> {
    let value = value.trim().to_ascii_lowercase();
    (!value.is_empty()
        && value.len() <= 128
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._-".contains(&byte)
        }))
    .then_some(value)
}

fn opaque_identifier_digest(value: &str) -> String {
    format!("sha256:{:x}", Sha256::digest(value.as_bytes()))
}

fn public_category_message(category: LlmProviderFailureCategory) -> &'static str {
    match category {
        LlmProviderFailureCategory::RateLimited => "模型服务暂时限流，请稍后重试。",
        LlmProviderFailureCategory::QuotaExhausted => "模型服务额度已经耗尽，请检查账户额度。",
        LlmProviderFailureCategory::Overloaded => "模型服务暂时繁忙，请稍后重试。",
        LlmProviderFailureCategory::Authentication => "模型服务鉴权失败，请检查连接配置。",
        LlmProviderFailureCategory::InvalidRequest => "模型服务拒绝了当前请求。",
        LlmProviderFailureCategory::ContextTooLarge => "发送给模型的上下文超过服务限制。",
        LlmProviderFailureCategory::Network => "模型服务网络请求失败。",
        LlmProviderFailureCategory::Unknown => "模型服务返回了无法识别的错误。",
    }
}

pub(super) fn public_provider_error_message(error: &AgentError) -> Option<String> {
    let (category, _, _, _) = provider_failure_metadata(error)?;
    let status_suffix = error
        .details()
        .and_then(|details| details.get("httpStatus"))
        .and_then(Value::as_u64)
        .map_or_else(String::new, |status| format!("（HTTP {status}）"));
    Some(format!(
        "{}{status_suffix}",
        public_category_message(category)
    ))
}

pub(super) fn provider_failure_metadata(
    error: &AgentError,
) -> Option<(
    LlmProviderFailureCategory,
    bool,
    Option<u64>,
    Option<String>,
)> {
    if error.code() != Some(PROVIDER_FAILURE_ERROR_CODE) {
        return None;
    }
    let details = error.details()?;
    let category = details
        .get("category")
        .and_then(Value::as_str)
        .and_then(LlmProviderFailureCategory::parse)?;
    let retryable = details
        .get("retryable")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let retry_after_ms = details.get("retryAfterMs").and_then(Value::as_u64);
    let provider_code = details
        .get("providerCode")
        .and_then(Value::as_str)
        .map(str::to_string);
    Some((category, retryable, retry_after_ms, provider_code))
}

struct ProviderErrorContext<'a> {
    api_style: AgentApiStyle,
    status: Option<u16>,
    provider_code: Option<&'a str>,
    body: &'a str,
}

trait ProviderErrorAdapter {
    fn classify(context: &ProviderErrorContext<'_>) -> Option<LlmProviderFailureCategory>;
}

struct QizhenProviderErrorAdapter;

impl ProviderErrorAdapter for QizhenProviderErrorAdapter {
    fn classify(context: &ProviderErrorContext<'_>) -> Option<LlmProviderFailureCategory> {
        match context.provider_code {
            Some(code) if code.eq_ignore_ascii_case("API_KEY_RATE_LIMIT_EXCEEDED") => {
                Some(LlmProviderFailureCategory::RateLimited)
            }
            _ => None,
        }
    }
}

struct OpenAiCompatibleErrorAdapter;

impl ProviderErrorAdapter for OpenAiCompatibleErrorAdapter {
    fn classify(context: &ProviderErrorContext<'_>) -> Option<LlmProviderFailureCategory> {
        if context.api_style != AgentApiStyle::OpenAiCompatible {
            return None;
        }
        classify_exact_provider_code(context.provider_code)
    }
}

struct AnthropicCompatibleErrorAdapter;

impl ProviderErrorAdapter for AnthropicCompatibleErrorAdapter {
    fn classify(context: &ProviderErrorContext<'_>) -> Option<LlmProviderFailureCategory> {
        if context.api_style != AgentApiStyle::AnthropicCompatible {
            return None;
        }
        classify_exact_provider_code(context.provider_code)
    }
}

fn classify_exact_provider_code(code: Option<&str>) -> Option<LlmProviderFailureCategory> {
    match code.map(str::to_ascii_lowercase).as_deref() {
        Some(
            "insufficient_quota"
            | "quota_exceeded"
            | "billing_hard_limit"
            | "billing_hard_limit_reached"
            | "credits_exhausted",
        ) => Some(LlmProviderFailureCategory::QuotaExhausted),
        Some("context_length_exceeded" | "prompt_too_long" | "max_tokens_exceeded") => {
            Some(LlmProviderFailureCategory::ContextTooLarge)
        }
        Some("authentication_error" | "invalid_api_key" | "permission_denied") => {
            Some(LlmProviderFailureCategory::Authentication)
        }
        Some("overloaded" | "overloaded_error" | "service_unavailable") => {
            Some(LlmProviderFailureCategory::Overloaded)
        }
        Some(
            "rate_limit"
            | "rate_limit_error"
            | "rate_limit_exceeded"
            | "too_many_requests"
            | "resource_exhausted",
        ) => Some(LlmProviderFailureCategory::RateLimited),
        Some("invalid_request_error" | "invalid_request") => {
            Some(LlmProviderFailureCategory::InvalidRequest)
        }
        _ => None,
    }
}

struct LegacyGatewayContentTypeAdapter;

impl ProviderErrorAdapter for LegacyGatewayContentTypeAdapter {
    fn classify(context: &ProviderErrorContext<'_>) -> Option<LlmProviderFailureCategory> {
        // A deployed OpenAI-compatible gateway routes valid JSON through Bedrock and sometimes
        // emits this one exact upstream compatibility failure as HTTP 400. Isolate the legacy
        // exception here; no other provider message text participates in retry decisions.
        (context.status == Some(400)
            && context
                .body
                .to_ascii_lowercase()
                .contains("the provided content type is invalid or not supported for this model"))
        .then_some(LlmProviderFailureCategory::Overloaded)
    }
}

fn classify_provider_failure(
    api_style: AgentApiStyle,
    status: Option<u16>,
    provider_code: Option<&str>,
    body: &str,
) -> LlmProviderFailureCategory {
    let context = ProviderErrorContext {
        api_style,
        status,
        provider_code,
        body,
    };
    QizhenProviderErrorAdapter::classify(&context)
        .or_else(|| OpenAiCompatibleErrorAdapter::classify(&context))
        .or_else(|| AnthropicCompatibleErrorAdapter::classify(&context))
        .or_else(|| LegacyGatewayContentTypeAdapter::classify(&context))
        .unwrap_or(match status {
            Some(401 | 403) => LlmProviderFailureCategory::Authentication,
            Some(413) => LlmProviderFailureCategory::ContextTooLarge,
            Some(429) => LlmProviderFailureCategory::RateLimited,
            Some(408 | 409 | 425) => LlmProviderFailureCategory::Network,
            Some(500 | 502 | 503 | 504 | 520 | 522 | 524 | 529) => {
                LlmProviderFailureCategory::Overloaded
            }
            Some(400 | 404 | 405 | 410 | 415 | 422) => LlmProviderFailureCategory::InvalidRequest,
            _ => LlmProviderFailureCategory::Unknown,
        })
}

fn retryable_category(
    category: LlmProviderFailureCategory,
    status: Option<u16>,
    _body: &str,
) -> bool {
    match category {
        LlmProviderFailureCategory::RateLimited
        | LlmProviderFailureCategory::Overloaded
        | LlmProviderFailureCategory::Network => true,
        LlmProviderFailureCategory::Unknown => matches!(
            status,
            Some(408 | 409 | 425 | 429 | 500 | 502 | 503 | 504 | 520 | 522 | 524 | 529)
        ),
        LlmProviderFailureCategory::QuotaExhausted
        | LlmProviderFailureCategory::Authentication
        | LlmProviderFailureCategory::InvalidRequest
        | LlmProviderFailureCategory::ContextTooLarge => false,
    }
}

fn extract_provider_code(body: &str) -> Option<String> {
    let value: Value = serde_json::from_str(body).ok()?;
    let candidates = [
        value.pointer("/error/code"),
        value.pointer("/error/type"),
        value.pointer("/error/status"),
        value.get("code"),
        value.get("type"),
        value.get("status"),
    ];
    // A numeric gateway error code often accompanies the portable discriminator in type/status.
    // Prefer a non-empty string from any canonical slot before falling back to a number.
    candidates
        .iter()
        .flatten()
        .find_map(|candidate| {
            candidate.as_str().filter(|value| {
                let value = value.trim();
                !value.is_empty() && !value.bytes().all(|byte| byte.is_ascii_digit())
            })
        })
        .map(str::to_string)
        .or_else(|| {
            candidates
                .iter()
                .flatten()
                .find_map(|candidate| candidate.as_str().filter(|value| !value.trim().is_empty()))
                .map(str::to_string)
        })
        .or_else(|| {
            candidates
                .into_iter()
                .flatten()
                .find_map(|candidate| candidate.as_number().map(ToString::to_string))
        })
        .map(|value| bounded_utf8(value.trim(), MAX_PROVIDER_CODE_BYTES))
        .filter(|value| !value.is_empty())
}

pub(super) fn retry_after_ms(headers: &HeaderMap) -> Option<u64> {
    // RFC Retry-After is authoritative when a gateway also emits a vendor millisecond hint.
    if let Some(value) = headers
        .get(RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
    {
        let value = value.trim();
        if let Ok(seconds) = value.parse::<u64>() {
            return Some(seconds.saturating_mul(1_000));
        }
        if let Ok(retry_at) = OffsetDateTime::parse(value, &Rfc2822) {
            let now = OffsetDateTime::now_utc();
            let milliseconds = (retry_at - now).whole_milliseconds().max(0);
            if let Ok(milliseconds) = u64::try_from(milliseconds) {
                return Some(milliseconds);
            }
        }
    }
    ["retry-after-ms", "x-ms-retry-after-ms"]
        .into_iter()
        .find_map(|name| headers.get(name))
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<u64>().ok())
}

fn sanitized_diagnostic_headers(headers: &HeaderMap, api_token: &str) -> BTreeMap<String, String> {
    const EXACT_HEADERS: &[&str] = &[
        "content-type",
        "retry-after",
        "retry-after-ms",
        "x-ms-retry-after-ms",
        "x-request-id",
        "request-id",
        "x-trace-id",
        "x-amzn-requestid",
        "x-goog-request-id",
        "cf-ray",
    ];
    let mut sanitized = BTreeMap::new();
    for (name, value) in headers {
        let name = name.as_str().to_ascii_lowercase();
        if !EXACT_HEADERS.contains(&name.as_str())
            && !name.starts_with("ratelimit-")
            && !name.starts_with("x-ratelimit-")
        {
            continue;
        }
        let Ok(value) = value.to_str() else {
            continue;
        };
        let value = sanitize_literal(value, api_token);
        sanitized.insert(name, bounded_utf8(value.trim(), MAX_HEADER_VALUE_BYTES));
    }
    sanitized
}

fn request_id_from_headers(headers: &BTreeMap<String, String>) -> Option<String> {
    [
        "x-request-id",
        "request-id",
        "x-trace-id",
        "x-amzn-requestid",
        "x-goog-request-id",
        "cf-ray",
    ]
    .into_iter()
    .find_map(|name| headers.get(name))
    .filter(|value| !value.is_empty() && value.as_str() != "[REDACTED]")
    .cloned()
}

fn sanitize_error_body(body: &str, api_token: &str) -> String {
    let mut sanitized = sanitize_literal(body, api_token);
    if let Ok(mut value) = serde_json::from_str::<Value>(&sanitized) {
        redact_sensitive_json_fields(&mut value);
        if let Ok(serialized) = serde_json::to_string(&value) {
            sanitized = serialized;
        }
    } else {
        sanitized = redact_bearer_values(&sanitized);
    }
    bounded_utf8(&sanitized, MAX_ERROR_BODY_BYTES)
}

fn embedded_error_projection(body: &str) -> String {
    let Ok(value) = serde_json::from_str::<Value>(body) else {
        return "provider returned an unstructured error envelope".to_string();
    };
    serde_json::to_string(&json!({
        "error": {
            "code": value.pointer("/error/code"),
            "type": value.pointer("/error/type"),
        },
        "code": value.get("code"),
        "type": value.get("type"),
        "status": value.get("status"),
    }))
    .unwrap_or_else(|_| "provider returned a structured error envelope".to_string())
}

fn sanitize_literal(value: &str, api_token: &str) -> String {
    if api_token.is_empty() {
        value.to_string()
    } else {
        value.replace(api_token, "[REDACTED]")
    }
}

fn redact_sensitive_json_fields(value: &mut Value) {
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                let key = key.to_ascii_lowercase();
                if key.contains("authorization")
                    || key.contains("api_key")
                    || key.contains("apikey")
                    || key.contains("token")
                    || key.contains("secret")
                    || key.contains("credential")
                {
                    *value = Value::String("[REDACTED]".to_string());
                } else {
                    redact_sensitive_json_fields(value);
                }
            }
        }
        Value::Array(values) => values.iter_mut().for_each(redact_sensitive_json_fields),
        _ => {}
    }
}

fn redact_bearer_values(value: &str) -> String {
    let mut output = Vec::new();
    let mut redact_next = false;
    for segment in value.split_whitespace() {
        if redact_next {
            output.push("[REDACTED]");
            redact_next = false;
        } else {
            output.push(segment);
            redact_next = segment.eq_ignore_ascii_case("bearer");
        }
    }
    output.join(" ")
}

fn bounded_utf8(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    let mut boundary = max_bytes;
    while boundary > 0 && !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    format!("{}...[truncated]", &value[..boundary])
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::{HeaderName, HeaderValue};

    #[test]
    fn qizhen_private_code_maps_to_portable_rate_limit_category() {
        let mut headers = HeaderMap::new();
        headers.insert(RETRY_AFTER, HeaderValue::from_static("12"));
        headers.insert("x-request-id", HeaderValue::from_static("req_fixture"));
        headers.insert("authorization", HeaderValue::from_static("Bearer secret"));
        let failure = LlmProviderFailure::from_http_response(
            AgentApiStyle::OpenAiCompatible,
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            &headers,
            r#"{"error":{"code":"API_KEY_RATE_LIMIT_EXCEEDED","message":"slow down"}}"#,
            "secret",
        );

        assert_eq!(failure.category, LlmProviderFailureCategory::RateLimited);
        assert!(failure.retryable);
        assert_eq!(failure.retry_after_ms, Some(12_000));
        assert_eq!(
            failure.raw.provider_code.as_deref(),
            Some("API_KEY_RATE_LIMIT_EXCEEDED")
        );
        assert_eq!(failure.raw.request_id.as_deref(), Some("req_fixture"));
        assert!(!failure.raw.headers.contains_key("authorization"));
        assert_eq!(
            failure
                .to_agent_error()
                .details()
                .and_then(|details| details.get("providerCode"))
                .and_then(Value::as_str),
            Some("api_key_rate_limit_exceeded")
        );
    }

    #[test]
    fn hard_quota_and_context_failures_are_not_retried() {
        let quota = LlmProviderFailure::from_http_response(
            AgentApiStyle::OpenAiCompatible,
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            &HeaderMap::new(),
            r#"{"error":{"code":"insufficient_quota","message":"exceeded your current quota"}}"#,
            "",
        );
        let context = LlmProviderFailure::from_http_response(
            AgentApiStyle::OpenAiCompatible,
            reqwest::StatusCode::BAD_REQUEST,
            &HeaderMap::new(),
            r#"{"error":{"code":"context_length_exceeded"}}"#,
            "",
        );

        assert_eq!(quota.category, LlmProviderFailureCategory::QuotaExhausted);
        assert!(!quota.retryable);
        assert_eq!(
            context.category,
            LlmProviderFailureCategory::ContextTooLarge
        );
        assert!(!context.retryable);
    }

    #[test]
    fn retry_after_accepts_delta_seconds_milliseconds_and_http_date() {
        let mut seconds = HeaderMap::new();
        seconds.insert(RETRY_AFTER, HeaderValue::from_static("7"));
        seconds.insert(
            HeaderName::from_static("retry-after-ms"),
            HeaderValue::from_static("10"),
        );
        assert_eq!(retry_after_ms(&seconds), Some(7_000));

        let mut milliseconds = HeaderMap::new();
        milliseconds.insert(
            HeaderName::from_static("retry-after-ms"),
            HeaderValue::from_static("1750"),
        );
        assert_eq!(retry_after_ms(&milliseconds), Some(1_750));

        let mut date = HeaderMap::new();
        date.insert(
            RETRY_AFTER,
            HeaderValue::from_static("Sun, 06 Nov 2094 08:49:37 GMT"),
        );
        assert!(retry_after_ms(&date).is_some_and(|value| value > 60_000));
    }

    #[test]
    fn retained_diagnostics_are_bounded_and_secret_cleaned() {
        let token = "super-secret-token";
        let body = json!({
            "error": {
                "code": "rate_limit_exceeded",
                "message": format!("{}{}", token, "x".repeat(MAX_ERROR_BODY_BYTES * 2)),
                "api_token": token,
            }
        })
        .to_string();
        let failure = LlmProviderFailure::from_http_response(
            AgentApiStyle::OpenAiCompatible,
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            &HeaderMap::new(),
            &body,
            token,
        );

        assert!(!failure.raw.body.contains(token));
        assert!(failure.raw.body.len() <= MAX_ERROR_BODY_BYTES + "...[truncated]".len());

        let presented = failure.to_agent_error();
        let rendered = format!("{presented} {:?}", presented.details());
        assert!(!rendered.contains(token));
        assert!(!rendered.contains(&"x".repeat(128)));
        assert!(presented
            .details()
            .and_then(|details| details.get("bodyHash"))
            .and_then(Value::as_str)
            .is_some_and(|value| value.starts_with("sha256:")));
        assert_eq!(
            presented
                .details()
                .and_then(|details| details.get("providerCode"))
                .and_then(Value::as_str),
            Some("rate_limit_exceeded")
        );
    }
}
