// Rust agent core.
use super::{
    block_on_tool_future, truncate_chars, web_favicon::FaviconFetcher, AgentTool,
    ToolExecutionContext,
};
use crate::cancellation::AgentCancellationToken;
use crate::protocol::{AgentError, AgentResult, AgentToolDefinition, AgentToolSafety};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use reqwest::Client;
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::Duration;

const TAVILY_SEARCH_ENDPOINT: &str = "https://api.tavily.com/search";
const DEFAULT_MAX_RESULTS: usize = 5;
const MAX_RESULTS: usize = 10;
const DEFAULT_RESULT_CONTENT_CHARS: usize = 1_500;
const MAX_RESULT_CONTENT_CHARS: usize = 4_000;
const DEFAULT_ANSWER_CHARS: usize = 4_000;

pub(super) struct WebSearchTool {
    api_key: String,
}

impl WebSearchTool {
    pub fn new(api_key: String) -> Self {
        Self { api_key }
    }
}

impl AgentTool for WebSearchTool {
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
                    "includeRawContent": { "type": "boolean" },
                    "includeDomains": { "type": "array", "items": { "type": "string" } },
                    "excludeDomains": { "type": "array", "items": { "type": "string" } }
                },
                "required": ["query"]
            }),
            safety: AgentToolSafety::ReadOnly,
            requires_workspace: false,
            requires_approval: false,
        }
    }

    fn execute(&self, context: &ToolExecutionContext, args: Value) -> AgentResult<Value> {
        let args: WebSearchArgs = serde_json::from_value(args)
            .map_err(|error| AgentError::new(format!("web_search 参数无效：{error}")))?;
        let request = TavilySearchRequest::from_args(args)?;
        let cancellation_token = context.cancellation_token();
        cancellation_token.check()?;
        let response = block_on_tool_future(
            TavilySearchClient::new(self.api_key.clone())
                .search(&request, cancellation_token.clone()),
        )?;

        format_tavily_response(request, response, &cancellation_token)
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
    include_raw_content: Option<bool>,
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
    include_raw_content: bool,
    include_domains: Vec<String>,
    exclude_domains: Vec<String>,
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
            include_raw_content: args.include_raw_content.unwrap_or(false),
            include_domains: clean_domains(args.include_domains.unwrap_or_default()),
            exclude_domains: clean_domains(args.exclude_domains.unwrap_or_default()),
        })
    }

    fn to_payload(&self) -> Value {
        let mut payload = json!({
            "query": self.query,
            "max_results": self.max_results,
            "search_depth": self.search_depth,
            "include_answer": self.include_answer,
            "include_raw_content": self.include_raw_content,
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
            .timeout(Duration::from_secs(20))
            .build()
            .map_err(|error| AgentError::new(format!("创建 Tavily HTTP 客户端失败：{error}")))?;
        let response = client
            .post(TAVILY_SEARCH_ENDPOINT)
            .headers(headers)
            .json(&request.to_payload())
            .send();
        let response = tokio::select! {
            _ = cancellation_token.cancelled() => return Err(AgentError::cancelled()),
            response = response => response
                .map_err(|error| AgentError::new(format!("请求 Tavily 搜索失败：{error}")))?,
        };
        let status = response.status();
        let body = tokio::select! {
            _ = cancellation_token.cancelled() => return Err(AgentError::cancelled()),
            body = response.text() => body
                .map_err(|error| AgentError::new(format!("读取 Tavily 响应失败：{error}")))?,
        };

        if !status.is_success() {
            let (body, _) = truncate_chars(&body, 600);
            return Err(AgentError::new(format!(
                "Tavily 搜索返回 {}：{}",
                status.as_u16(),
                body
            )));
        }

        serde_json::from_str(&body)
            .map_err(|error| AgentError::new(format!("Tavily 响应不是有效 JSON：{error}")))
    }
}

fn format_tavily_response(
    request: TavilySearchRequest,
    response: Value,
    cancellation_token: &AgentCancellationToken,
) -> AgentResult<Value> {
    cancellation_token.check()?;
    let favicon_fetcher = FaviconFetcher::new();
    let (answer, answer_truncated) = response
        .get("answer")
        .and_then(Value::as_str)
        .map(|answer| truncate_chars(answer, DEFAULT_ANSWER_CHARS))
        .map(|(answer, truncated)| (Some(answer), truncated))
        .unwrap_or((None, false));
    let raw_results_len = response
        .get("results")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    let mut truncated = answer_truncated || raw_results_len > request.max_results;
    let results = response
        .get("results")
        .and_then(Value::as_array)
        .map(|results| {
            results
                .iter()
                .take(request.max_results)
                .map(|result| {
                    cancellation_token.check()?;
                    let (result, result_truncated) =
                        format_tavily_result(result, request.include_raw_content, &favicon_fetcher);
                    truncated |= result_truncated;
                    Ok(result)
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

    Ok(json!({
        "query": request.query,
        "provider": "tavily",
        "answer": answer,
        "results": results,
        "images": images,
        "responseTime": response_time,
        "truncated": truncated
    }))
}

fn format_tavily_result(
    result: &Value,
    include_raw_content: bool,
    favicon_fetcher: &FaviconFetcher,
) -> (Value, bool) {
    let (content, content_truncated) = result
        .get("content")
        .and_then(Value::as_str)
        .map(|content| truncate_chars(content, DEFAULT_RESULT_CONTENT_CHARS))
        .map(|(content, truncated)| (Some(content), truncated))
        .unwrap_or((None, false));
    let (raw_content, raw_content_truncated) = if include_raw_content {
        result
            .get("raw_content")
            .or_else(|| result.get("rawContent"))
            .and_then(Value::as_str)
            .map(|content| truncate_chars(content, MAX_RESULT_CONTENT_CHARS))
            .map(|(content, truncated)| (Some(content), truncated))
            .unwrap_or((None, false))
    } else {
        (None, false)
    };
    let favicon = result
        .get("url")
        .and_then(Value::as_str)
        .and_then(|url| favicon_fetcher.fetch(result, url));

    (
        json!({
            "title": result.get("title").and_then(Value::as_str),
            "url": result.get("url").and_then(Value::as_str),
            "content": content,
            "rawContent": raw_content,
            "score": result.get("score").cloned(),
            "publishedDate": result
                .get("published_date")
                .or_else(|| result.get("publishedDate"))
                .and_then(Value::as_str),
            "faviconDataUrl": favicon.as_ref().map(|asset| asset.data_url.as_str()),
            "faviconMimeType": favicon.as_ref().map(|asset| asset.mime_type.as_str())
        }),
        content_truncated || raw_content_truncated,
    )
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
            include_raw_content: Some(true),
            include_domains: Some(vec![" docs.rs ".to_string(), "".to_string()]),
            exclude_domains: None,
        })
        .unwrap();
        let payload = request.to_payload();

        assert_eq!(request.max_results, MAX_RESULTS);
        assert_eq!(payload["query"], "rust async");
        assert_eq!(payload["search_depth"], "advanced");
        assert_eq!(payload["include_answer"], true);
        assert_eq!(payload["include_raw_content"], true);
        assert_eq!(payload["include_domains"][0], "docs.rs");
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
            include_raw_content: None,
            include_domains: None,
            exclude_domains: None,
        })
        .unwrap_err();

        assert!(error.to_string().contains("searchDepth"));
    }
}
