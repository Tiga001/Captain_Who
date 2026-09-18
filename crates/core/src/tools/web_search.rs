use super::web_retry;
use super::{block_on_tool_future, truncate_chars, AgentTool, ToolExecutionContext};
use crate::cancellation::AgentCancellationToken;
use crate::protocol::{
    AgentError, AgentResult, AgentToolDefinition, AgentToolResult, AgentToolSafety,
};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use reqwest::Client;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::{Duration, Instant};

const TAVILY_SEARCH_ENDPOINT: &str = "https://api.tavily.com/search";
const DEFAULT_MAX_RESULTS: usize = 5;
const MAX_RESULTS: usize = 8;
const TAVILY_MAX_CHUNKS_PER_SOURCE: usize = 3;
const TAVILY_CHUNK_MAX_CHARS: usize = 500;

// Retry budget for one tool call: a single provider attempt keeps the previous timeout,
// while the call-level budget bounds attempts plus backoff waits so retries cannot hang the tool.
const SEARCH_ATTEMPT_TIMEOUT: Duration = Duration::from_secs(20);
const SEARCH_TOTAL_BUDGET: Duration = Duration::from_secs(35);

pub(crate) struct WebSearchTool {
    policy: Arc<dyn crate::WebSearchPolicySource>,
}

impl WebSearchTool {
    pub fn new(api_key: String) -> Self {
        Self::with_policy(Arc::new(
            crate::FrozenWebSearchPolicySource::from_search_config(Some(
                &crate::AgentSearchConfig {
                    mode: crate::AgentSearchMode::Auto,
                    tavily_api_key: Some(api_key),
                },
            )),
        ))
    }

    pub(crate) fn with_policy(policy: Arc<dyn crate::WebSearchPolicySource>) -> Self {
        Self { policy }
    }
}

impl AgentTool for WebSearchTool {
    fn exposure(&self) -> super::AgentToolExposure {
        super::AgentToolExposure::RequiresCapability(super::ToolCapabilityId::application_owned(
            super::tool_set::WEB_SEARCH_CAPABILITY,
        ))
    }

    fn permission_policy(&self) -> super::AgentToolPermissionPolicy {
        super::AgentToolPermissionPolicy::Default
    }

    fn definition(&self) -> AgentToolDefinition {
        AgentToolDefinition {
            name: "web_search".to_string(),
            description: "Search the public web using Tavily. Sends the query to an external network service.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Search query." },
                    "maxResults": { "type": "integer", "minimum": 1, "maximum": MAX_RESULTS },
                    "searchDepth": { "type": "string", "enum": ["auto", "basic", "advanced"] },
                    "topic": { "type": "string", "enum": ["general", "news", "finance"] },
                    "timeRange": { "type": "string", "description": "Optional Tavily time range such as day, week, month, or year." },
                    "includeAnswer": { "type": "boolean" },
                    "includeDomains": { "type": "array", "items": { "type": "string" } },
                    "excludeDomains": { "type": "array", "items": { "type": "string" } }
                },
                "required": ["query"]
            }),
            safety: AgentToolSafety::ReadOnly,
            requires_workspace: false,
            requires_approval: false,
            approval_mode: crate::protocol::AgentToolApprovalMode::Never,
        }
    }

    fn execute(&self, context: &ToolExecutionContext, args: Value) -> AgentResult<Value> {
        let args: WebSearchArgs = serde_json::from_value(args)
            .map_err(|error| AgentError::new(format!("web_search 参数无效：{error}")))?;
        let request = TavilySearchRequest::from_args(args)?
            .with_model_output_budget(context.text_output_budget());
        let cancellation_token = context.cancellation_token();
        cancellation_token.check()?;
        let api_key = self.policy.authorize_execution()?.into_secret();
        let response = block_on_tool_future(
            TavilySearchClient::new(api_key).search(&request, cancellation_token.clone()),
        )?;

        format_tavily_response(request, response, &cancellation_token)
    }

    fn model_projection(&self, result: &AgentToolResult) -> AgentToolResult {
        let projected = result.result.as_ref().and_then(|value| {
            let mut output = serde_json::Map::new();
            super::model_projection::insert_field(&mut output, value, "answer");
            super::model_projection::insert_field(&mut output, value, "contentKind");
            super::model_projection::insert_field(&mut output, value, "fullContentTool");
            super::model_projection::insert_field(&mut output, value, "sourceCompleteness");
            let mut result_limit_applied = false;
            if let Some(results) = value.get("results").and_then(Value::as_array) {
                let requested_max_results = value
                    .get("requestedMaxResults")
                    .and_then(Value::as_u64)
                    .and_then(|limit| usize::try_from(limit).ok())
                    .unwrap_or(MAX_RESULTS)
                    .clamp(1, MAX_RESULTS);
                let results = results
                    .iter()
                    .take(requested_max_results)
                    .filter_map(|item| {
                        super::model_projection::retain_object_fields(
                            item,
                            &["title", "url", "content", "publishedDate"],
                        )
                    })
                    .collect::<Vec<_>>();
                let returned = results.len();
                let omitted = value
                    .get("results")
                    .and_then(Value::as_array)
                    .map_or(0, |source| source.len().saturating_sub(returned));
                output.insert(
                    "resultCoverage".to_string(),
                    json!({
                        "total": returned.saturating_add(omitted),
                        "returned": returned,
                        "omitted": omitted
                    }),
                );
                result_limit_applied = omitted > 0;
                if !results.is_empty() {
                    output.insert("results".to_string(), Value::Array(results));
                }
            }
            super::model_projection::insert_field(&mut output, value, "images");
            for key in [
                "cursor",
                "next",
                "nextCursor",
                "continueWith",
                "truncatedAtSource",
                "omittedBytes",
                "sourceStopReason",
            ] {
                super::model_projection::insert_field(&mut output, value, key);
            }
            if result_limit_applied {
                output.insert("partial".to_string(), Value::Bool(true));
                output.insert(
                    "partialReason".to_string(),
                    Value::String("model_result_count_limit".to_string()),
                );
                output.insert(
                    "refine".to_string(),
                    Value::String(
                        "Narrow the web_search query for different results, or use web_fetch on a returned URL for exact page content."
                            .to_string(),
                    ),
                );
            }
            super::model_projection::insert_field(&mut output, value, "truncated");
            (!output.is_empty()).then_some(Value::Object(output))
        });
        super::model_projection::compact_model_result(result, projected)
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WebSearchArgs {
    query: String,
    max_results: Option<usize>,
    search_depth: Option<String>,
    topic: Option<String>,
    time_range: Option<String>,
    include_answer: Option<bool>,
    include_domains: Option<Vec<String>>,
    exclude_domains: Option<Vec<String>>,
}

#[derive(Debug)]
struct TavilySearchRequest {
    query: String,
    max_results: usize,
    search_depth: String,
    topic: Option<String>,
    time_range: Option<String>,
    include_answer: bool,
    include_domains: Vec<String>,
    exclude_domains: Vec<String>,
    chunks_per_source: Option<usize>,
}

impl TavilySearchRequest {
    fn from_args(args: WebSearchArgs) -> AgentResult<Self> {
        let query = args.query.trim().to_string();
        if query.is_empty() {
            return Err(AgentError::new("web_search.query 不能为空。"));
        }

        let search_depth = args.search_depth.unwrap_or_else(|| "basic".to_string());
        if !matches!(search_depth.as_str(), "auto" | "basic" | "advanced") {
            return Err(AgentError::new(
                "web_search.searchDepth 只能是 auto、basic 或 advanced。",
            ));
        }

        let topic = args.topic.filter(|topic| !topic.trim().is_empty());
        if let Some(topic) = topic.as_deref() {
            if !matches!(topic, "general" | "news" | "finance") {
                return Err(AgentError::new(
                    "web_search.topic 只能是 general、news 或 finance。",
                ));
            }
        }

        Ok(Self {
            query,
            max_results: args
                .max_results
                .unwrap_or(DEFAULT_MAX_RESULTS)
                .clamp(1, MAX_RESULTS),
            search_depth,
            topic,
            time_range: args.time_range.filter(|value| !value.trim().is_empty()),
            include_answer: args.include_answer.unwrap_or(true),
            include_domains: clean_domains(args.include_domains.unwrap_or_default()),
            exclude_domains: clean_domains(args.exclude_domains.unwrap_or_default()),
            chunks_per_source: None,
        })
    }

    /// Tavily has no generic `max_output_tokens` option. Advanced Search does expose
    /// `chunks_per_source`, so translate the current run's model-result allowance into the
    /// closest provider-native bound instead of applying another local character limit.
    fn with_model_output_budget(mut self, budget: &crate::context::ContextTextBudget) -> Self {
        if self.search_depth == "advanced" {
            let tokens_per_chunk = budget.estimate(&"x".repeat(TAVILY_CHUNK_MAX_CHARS)).max(1);
            let tokens_per_source = budget
                .max_tokens()
                .checked_div(self.max_results as u64)
                .unwrap_or(1)
                .max(1);
            self.chunks_per_source = Some(
                usize::try_from(tokens_per_source / tokens_per_chunk)
                    .unwrap_or(TAVILY_MAX_CHUNKS_PER_SOURCE)
                    .clamp(1, TAVILY_MAX_CHUNKS_PER_SOURCE),
            );
        }
        self
    }

    fn to_payload(&self) -> Value {
        let mut payload = json!({
            "query": self.query,
            "max_results": self.max_results,
            "search_depth": self.search_depth,
            "include_answer": self.include_answer,
            "include_raw_content": false,
        });

        if let Some(topic) = &self.topic {
            payload["topic"] = json!(topic);
        }
        if let Some(time_range) = &self.time_range {
            payload["time_range"] = json!(time_range);
        }
        if !self.include_domains.is_empty() {
            payload["include_domains"] = json!(self.include_domains);
        }
        if !self.exclude_domains.is_empty() {
            payload["exclude_domains"] = json!(self.exclude_domains);
        }
        if let Some(chunks_per_source) = self.chunks_per_source {
            payload["chunks_per_source"] = json!(chunks_per_source);
        }

        payload
    }
}

struct TavilySearchClient {
    api_key: String,
}

impl TavilySearchClient {
    fn new(api_key: String) -> Self {
        Self { api_key }
    }

    async fn search(
        &self,
        request: &TavilySearchRequest,
        cancellation_token: AgentCancellationToken,
    ) -> AgentResult<Value> {
        cancellation_token.check()?;
        let api_key = self.api_key.trim();
        if api_key.is_empty() {
            return Err(AgentError::new(
                "Tavily API Key 为空，无法执行 web_search。",
            ));
        }

        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {api_key}"))
                .map_err(|_| AgentError::new("Tavily API Key 包含非法字符。"))?,
        );

        let client = Client::builder()
            .timeout(SEARCH_ATTEMPT_TIMEOUT)
            .connect_timeout(web_retry::CONNECT_TIMEOUT)
            .build()
            .map_err(|error| AgentError::new(format!("创建 Tavily HTTP 客户端失败：{error}")))?;

        let deadline = Instant::now() + SEARCH_TOTAL_BUDGET;
        let mut last_error: Option<AgentError> = None;
        let mut retry_after: Option<Duration> = None;

        for attempt in 0..web_retry::MAX_ATTEMPTS {
            cancellation_token.check()?;
            if attempt > 0 {
                let wait = match web_retry::retry_delay(attempt as u32, retry_after) {
                    Some(wait) => wait,
                    None => {
                        web_retry::trace_retry_skipped(
                            "web_search",
                            attempt + 1,
                            "retry-after-too-long",
                            "provider Retry-After exceeds the wait budget",
                        );
                        break;
                    }
                };
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining <= wait + web_retry::MIN_RETRY_REMAINING {
                    web_retry::trace_retry_skipped(
                        "web_search",
                        attempt + 1,
                        "budget-exhausted",
                        "insufficient remaining call budget",
                    );
                    break;
                }
                web_retry::trace_retry_wait("web_search", attempt + 1, wait);
                web_retry::wait_before_retry(wait, &cancellation_token).await?;
            }

            let remaining = deadline.saturating_duration_since(Instant::now());
            let attempt_timeout = SEARCH_ATTEMPT_TIMEOUT.min(remaining);
            let outcome = self
                .search_once(
                    &client,
                    &headers,
                    request,
                    attempt_timeout,
                    &cancellation_token,
                )
                .await;
            match outcome {
                Ok(value) => {
                    if attempt > 0 {
                        web_retry::trace_retry_recovered("web_search", attempt + 1);
                    }
                    return Ok(value);
                }
                Err(failure) => {
                    let reason = failure.error.to_string();
                    if !failure.retryable {
                        web_retry::trace_retry_skipped(
                            "web_search",
                            attempt + 1,
                            "terminal",
                            &reason,
                        );
                        return Err(failure.error);
                    }
                    last_error = Some(failure.error);
                    retry_after = failure.retry_after;
                    if attempt + 1 == web_retry::MAX_ATTEMPTS {
                        web_retry::trace_retries_exhausted("web_search", attempt + 1, &reason);
                        break;
                    }
                    web_retry::trace_retry_planned("web_search", attempt + 1, &reason);
                }
            }
        }

        Err(last_error.unwrap_or_else(|| {
            AgentError::new("web_search 请求 Tavily 搜索失败：内部重试未产生可返回结果。")
        }))
    }

    async fn search_once(
        &self,
        client: &Client,
        headers: &HeaderMap,
        request: &TavilySearchRequest,
        attempt_timeout: Duration,
        cancellation_token: &AgentCancellationToken,
    ) -> Result<Value, web_retry::AttemptFailure> {
        let response = client
            .post(TAVILY_SEARCH_ENDPOINT)
            .headers(headers.clone())
            .timeout(attempt_timeout)
            .json(&request.to_payload())
            .send();
        let response = tokio::select! {
            _ = cancellation_token.cancelled() => {
                return Err(web_retry::AttemptFailure::terminal(AgentError::cancelled()));
            }
            response = response => match response {
                Ok(response) => response,
                Err(error) => {
                    return Err(web_retry::AttemptFailure::retryable(AgentError::new(
                        format!("请求 Tavily 搜索失败：{error}"),
                    )));
                }
            },
        };
        let status = response.status();
        let retry_after = web_retry::retry_after_delay(response.headers());
        let body = tokio::select! {
            _ = cancellation_token.cancelled() => {
                return Err(web_retry::AttemptFailure::terminal(AgentError::cancelled()));
            }
            body = response.text() => match body {
                Ok(body) => body,
                Err(error) => {
                    return Err(web_retry::AttemptFailure::retryable(AgentError::new(
                        format!("读取 Tavily 响应失败：{error}"),
                    )));
                }
            },
        };

        if !status.is_success() {
            let (body, _) = truncate_chars(&body, 600);
            let error = AgentError::new(format!("Tavily 搜索返回 {}：{}", status.as_u16(), body));
            return Err(if web_retry::is_retryable_status(status) {
                web_retry::AttemptFailure::retryable(error).with_retry_after(retry_after)
            } else {
                web_retry::AttemptFailure::terminal(error)
            });
        }

        match serde_json::from_str(&body) {
            Ok(value) => Ok(value),
            Err(error) => Err(web_retry::AttemptFailure::retryable(AgentError::new(
                format!("Tavily 响应不是有效 JSON：{error}"),
            ))),
        }
    }
}

fn format_tavily_response(
    request: TavilySearchRequest,
    response: Value,
    cancellation_token: &AgentCancellationToken,
) -> AgentResult<Value> {
    cancellation_token.check()?;
    let answer = response
        .get("answer")
        .and_then(Value::as_str)
        .map(str::to_string);
    let results = response
        .get("results")
        .and_then(Value::as_array)
        .map(|results| {
            results
                .iter()
                .map(|result| {
                    cancellation_token.check()?;
                    Ok(format_tavily_result(result))
                })
                .collect::<AgentResult<Vec<_>>>()
        })
        .transpose()?
        .unwrap_or_default();
    let images = response.get("images").cloned().unwrap_or_else(|| json!([]));
    let response_time = response
        .get("response_time")
        .or_else(|| response.get("responseTime"))
        .cloned();
    let source_completeness = response
        .get("truncated_at_source")
        .or_else(|| response.get("truncatedAtSource"))
        .and_then(Value::as_bool)
        .map_or("unknown", |truncated| {
            if truncated {
                "provider_declared_truncated"
            } else {
                "provider_declared_complete"
            }
        });

    let mut output = serde_json::Map::from_iter([
        ("query".to_string(), json!(request.query)),
        ("provider".to_string(), json!("tavily")),
        ("answer".to_string(), json!(answer)),
        ("contentKind".to_string(), json!("provider_search_summary")),
        ("fullContentTool".to_string(), json!("web_fetch")),
        ("sourceCompleteness".to_string(), json!(source_completeness)),
        (
            "requestedMaxResults".to_string(),
            json!(request.max_results),
        ),
        ("results".to_string(), json!(results)),
        ("images".to_string(), images),
        ("responseTime".to_string(), json!(response_time)),
    ]);
    copy_provider_navigation_and_source_metadata(&response, &mut output);
    Ok(Value::Object(output))
}

fn format_tavily_result(result: &Value) -> Value {
    let content = result
        .get("content")
        .and_then(Value::as_str)
        .map(str::to_string);
    json!({
        "title": result.get("title").and_then(Value::as_str),
        "url": result.get("url").and_then(Value::as_str),
        "content": content,
        "score": result.get("score").cloned(),
        "publishedDate": result
            .get("published_date")
            .or_else(|| result.get("publishedDate"))
            .and_then(Value::as_str),
        "favicon": result.get("favicon").and_then(Value::as_str)
    })
}

/// Retains provider-owned pagination and source-integrity declarations without fabricating
/// completeness when the provider does not expose them.
fn copy_provider_navigation_and_source_metadata(
    response: &Value,
    output: &mut serde_json::Map<String, Value>,
) {
    for (canonical, candidates) in [
        ("cursor", &["cursor"][..]),
        ("next", &["next"][..]),
        ("nextCursor", &["next_cursor", "nextCursor"][..]),
        ("continueWith", &["continue_with", "continueWith"][..]),
        (
            "truncatedAtSource",
            &["truncated_at_source", "truncatedAtSource"][..],
        ),
        ("truncated", &["truncated"][..]),
        ("omittedBytes", &["omitted_bytes", "omittedBytes"][..]),
        (
            "sourceStopReason",
            &["source_stop_reason", "sourceStopReason"][..],
        ),
    ] {
        if let Some(value) = candidates
            .iter()
            .find_map(|candidate| response.get(*candidate))
        {
            output.insert(canonical.to_string(), value.clone());
        }
    }
}

fn clean_domains(domains: Vec<String>) -> Vec<String> {
    domains
        .into_iter()
        .map(|domain| domain.trim().to_string())
        .filter(|domain| !domain.is_empty())
        .take(20)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_bounded_tavily_payload() {
        let request = TavilySearchRequest::from_args(WebSearchArgs {
            query: " rust async ".to_string(),
            max_results: Some(999),
            search_depth: Some("advanced".to_string()),
            topic: Some("general".to_string()),
            time_range: Some("week".to_string()),
            include_answer: None,
            include_domains: Some(vec![" docs.rs ".to_string(), "".to_string()]),
            exclude_domains: None,
        })
        .unwrap()
        .with_model_output_budget(&crate::context::ContextTextBudget::heuristic(10_000));
        let payload = request.to_payload();

        assert_eq!(request.max_results, MAX_RESULTS);
        assert_eq!(payload["query"], "rust async");
        assert_eq!(payload["search_depth"], "advanced");
        assert_eq!(payload["include_answer"], true);
        assert_eq!(payload["include_raw_content"], false);
        assert_eq!(payload["include_domains"][0], "docs.rs");
        assert_eq!(payload["chunks_per_source"], TAVILY_MAX_CHUNKS_PER_SOURCE);
    }

    #[test]
    fn rejects_invalid_search_depth() {
        let error = TavilySearchRequest::from_args(WebSearchArgs {
            query: "rust".to_string(),
            max_results: None,
            search_depth: Some("deep".to_string()),
            topic: None,
            time_range: None,
            include_answer: None,
            include_domains: None,
            exclude_domains: None,
        })
        .unwrap_err();

        assert!(error.to_string().contains("searchDepth"));
    }

    #[test]
    fn formats_results_without_favicon_data_urls() {
        let request = TavilySearchRequest::from_args(WebSearchArgs {
            query: "rust".to_string(),
            max_results: Some(1),
            search_depth: None,
            topic: None,
            time_range: None,
            include_answer: None,
            include_domains: None,
            exclude_domains: None,
        })
        .unwrap();
        let response = json!({
            "results": [{
                "title": "Rust",
                "url": "https://www.rust-lang.org/",
                "content": "Rust language",
                "score": 0.9,
                "favicon": "https://www.rust-lang.org/favicon.ico"
            }],
            "response_time": 0.42
        });

        let formatted =
            format_tavily_response(request, response, &AgentCancellationToken::new()).unwrap();
        let result = &formatted["results"][0];

        assert_eq!(result["favicon"], "https://www.rust-lang.org/favicon.ico");
        assert!(result.get("rawContent").is_none());
        assert!(result.get("faviconDataUrl").is_none());
        assert!(result.get("faviconMimeType").is_none());
    }

    #[test]
    fn preserves_provider_search_summaries_until_the_central_model_gate() {
        let request = TavilySearchRequest::from_args(WebSearchArgs {
            query: "rust".to_string(),
            max_results: Some(MAX_RESULTS),
            search_depth: None,
            topic: None,
            time_range: None,
            include_answer: None,
            include_domains: None,
            exclude_domains: None,
        })
        .unwrap();
        let answer = "a".repeat(12_345);
        let result_content = "b".repeat(6_789);
        let response = json!({
            "answer": answer,
            "results": (0..MAX_RESULTS)
                .map(|index| json!({
                    "title": format!("Result {index}"),
                    "url": format!("https://example.com/{index}"),
                    "content": result_content
                }))
                .collect::<Vec<_>>(),
            "cursor": "provider-page-2",
            "next": { "cursor": "provider-page-2" },
            "continue_with": {
                "provider": "tavily",
                "cursor": "provider-page-2"
            }
        });

        let formatted =
            format_tavily_response(request, response, &AgentCancellationToken::new()).unwrap();

        assert_eq!(formatted["results"].as_array().unwrap().len(), MAX_RESULTS);
        assert_eq!(formatted["answer"], answer);
        assert!(formatted["results"]
            .as_array()
            .unwrap()
            .iter()
            .all(|result| result["content"] == result_content));
        assert_eq!(formatted["contentKind"], "provider_search_summary");
        assert_eq!(formatted["fullContentTool"], "web_fetch");
        assert_eq!(formatted["sourceCompleteness"], "unknown");
        assert_eq!(formatted["cursor"], "provider-page-2");
        assert_eq!(formatted["next"]["cursor"], "provider-page-2");
        assert_eq!(formatted["continueWith"]["cursor"], "provider-page-2");
        assert!(
            formatted.get("truncatedAtSource").is_none(),
            "provider silence must not be converted into a completeness claim"
        );
    }

    #[test]
    fn translates_small_advanced_search_budget_to_provider_chunk_limit() {
        let request = TavilySearchRequest::from_args(WebSearchArgs {
            query: "rust".to_string(),
            max_results: Some(MAX_RESULTS),
            search_depth: Some("advanced".to_string()),
            topic: None,
            time_range: None,
            include_answer: None,
            include_domains: None,
            exclude_domains: None,
        })
        .unwrap()
        .with_model_output_budget(&crate::context::ContextTextBudget::heuristic(1));

        assert_eq!(request.to_payload()["chunks_per_source"], 1);
    }

    #[test]
    fn basic_search_does_not_send_unsupported_chunk_parameter() {
        let request = TavilySearchRequest::from_args(WebSearchArgs {
            query: "rust".to_string(),
            max_results: None,
            search_depth: Some("basic".to_string()),
            topic: None,
            time_range: None,
            include_answer: None,
            include_domains: None,
            exclude_domains: None,
        })
        .unwrap()
        .with_model_output_budget(&crate::context::ContextTextBudget::heuristic(10_000));

        assert!(request.to_payload().get("chunks_per_source").is_none());
    }

    #[test]
    fn provider_pagination_is_not_misreported_as_unrecoverable_source_truncation() {
        let request = TavilySearchRequest::from_args(WebSearchArgs {
            query: "rust".to_string(),
            max_results: None,
            search_depth: None,
            topic: None,
            time_range: None,
            include_answer: None,
            include_domains: None,
            exclude_domains: None,
        })
        .unwrap();
        let formatted = format_tavily_response(
            request,
            json!({
                "results": [],
                "truncated": true,
                "cursor": "provider-next-page"
            }),
            &AgentCancellationToken::new(),
        )
        .unwrap();
        let result = AgentToolResult {
            exact_archive_file: None,
            call_id: "web-search-page".to_string(),
            tool: "web_search".to_string(),
            ok: true,
            result: Some(formatted),
            error: None,
        };

        assert!(!crate::tools::tool_result_truncated_at_source(&result));
    }

    #[test]
    fn model_projection_defensively_enforces_eight_results_without_losing_provider_archive() {
        let raw = AgentToolResult {
            exact_archive_file: None,
            call_id: "web-search-provider-overflow".to_string(),
            tool: "web_search".to_string(),
            ok: true,
            result: Some(json!({
                "requestedMaxResults": MAX_RESULTS,
                "contentKind": "provider_search_summary",
                "fullContentTool": "web_fetch",
                "sourceCompleteness": "unknown",
                "results": (0..(MAX_RESULTS + 3))
                    .map(|index| json!({
                        "title": format!("Result {index}"),
                        "url": format!("https://example.com/{index}"),
                        "content": format!("summary {index}")
                    }))
                    .collect::<Vec<_>>()
            })),
            error: None,
        };

        let model = WebSearchTool::new(String::new()).model_projection(&raw);
        let model_value = model.result.as_ref().unwrap();
        assert_eq!(
            model_value["results"].as_array().unwrap().len(),
            MAX_RESULTS
        );
        assert_eq!(model_value["resultCoverage"]["total"], MAX_RESULTS + 3);
        assert_eq!(model_value["resultCoverage"]["returned"], MAX_RESULTS);
        assert_eq!(model_value["resultCoverage"]["omitted"], 3);
        assert_eq!(model_value["partial"], true);
        assert_eq!(model_value["partialReason"], "model_result_count_limit");
        assert!(model_value.get("truncatedAtSource").is_none());
        assert!(model_value.get("refine").is_some());
        let gate = crate::context::ContextCapacityDetector::for_model(
            "test-model",
            crate::protocol::AgentApiStyle::OpenAiCompatible,
            &[],
        )
        .model_tool_result_gate();
        let gated = gate.project(&raw.call_id, false, &model, None);
        let gated_value: Value = serde_json::from_str(&gated.content).unwrap();
        assert!(!gated.truncated);
        assert!(gated_value.get("truncatedAtSource").is_none());

        let archive = WebSearchTool::new(String::new()).archive_projection(&raw);
        assert_eq!(
            archive.result.as_ref().unwrap()["results"]
                .as_array()
                .unwrap()
                .len(),
            MAX_RESULTS + 3
        );
    }
}
