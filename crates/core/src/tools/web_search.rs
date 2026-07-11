use super::{block_on_tool_future, truncate_chars, AgentTool, ToolExecutionContext};
use crate::cancellation::AgentCancellationToken;
use crate::protocol::{AgentError, AgentResult, AgentToolDefinition, AgentToolSafety};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use reqwest::Client;
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::Duration;

const TAVILY_SEARCH_ENDPOINT: &str = "https://api.tavily.com/search";
const DEFAULT_MAX_RESULTS: usize = 5;
const MAX_RESULTS: usize = 8;
const DEFAULT_RESULT_CONTENT_CHARS: usize = 600;
const DEFAULT_ANSWER_CHARS: usize = 1_200;

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
        })
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
                    let (result, result_truncated) = format_tavily_result(result);
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

fn format_tavily_result(result: &Value) -> (Value, bool) {
    let (content, content_truncated) = result
        .get("content")
        .and_then(Value::as_str)
        .map(|content| truncate_chars(content, DEFAULT_RESULT_CONTENT_CHARS))
        .map(|(content, truncated)| (Some(content), truncated))
        .unwrap_or((None, false));
    (
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
        }),
        content_truncated,
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
            include_domains: Some(vec![" docs.rs ".to_string(), "".to_string()]),
            exclude_domains: None,
        })
        .unwrap();
        let payload = request.to_payload();

        assert_eq!(request.max_results, MAX_RESULTS);
        assert_eq!(payload["query"], "rust async");
        assert_eq!(payload["search_depth"], "advanced");
        assert_eq!(payload["include_answer"], true);
        assert_eq!(payload["include_raw_content"], false);
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
    fn bounds_answer_and_result_content_for_model_context() {
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
        let response = json!({
            "answer": "a".repeat(DEFAULT_ANSWER_CHARS + 100),
            "results": (0..MAX_RESULTS)
                .map(|index| json!({
                    "title": format!("Result {index}"),
                    "url": format!("https://example.com/{index}"),
                    "content": "b".repeat(DEFAULT_RESULT_CONTENT_CHARS + 100)
                }))
                .collect::<Vec<_>>()
        });

        let formatted =
            format_tavily_response(request, response, &AgentCancellationToken::new()).unwrap();
        let truncation_suffix_chars = "\n...[truncated]".chars().count();

        assert_eq!(formatted["results"].as_array().unwrap().len(), MAX_RESULTS);
        assert_eq!(
            formatted["answer"].as_str().unwrap().chars().count(),
            DEFAULT_ANSWER_CHARS + truncation_suffix_chars
        );
        assert!(formatted["results"]
            .as_array()
            .unwrap()
            .iter()
            .all(|result| {
                result["content"].as_str().unwrap().chars().count()
                    == DEFAULT_RESULT_CONTENT_CHARS + truncation_suffix_chars
            }));
        assert_eq!(formatted["truncated"], true);
    }
}
