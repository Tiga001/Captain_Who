use super::{block_on_tool_future, truncate_chars, AgentTool, ToolExecutionContext};
use crate::cancellation::AgentCancellationToken;
use crate::protocol::{AgentError, AgentResult, AgentToolDefinition, AgentToolSafety};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use reqwest::Client;
use reqwest::Url;
use serde::Deserialize;
use serde_json::{json, Value};
use std::net::{IpAddr, Ipv4Addr};
use std::time::Duration;

const TAVILY_EXTRACT_ENDPOINT: &str = "https://api.tavily.com/extract";
const DEFAULT_MAX_CHARS: usize = 40_000;
const MAX_CONTENT_CHARS: usize = 120_000;
const DEFAULT_TIMEOUT_SECONDS: f64 = 30.0;
const MAX_TIMEOUT_SECONDS: f64 = 60.0;
const DEFAULT_CHUNKS_PER_SOURCE: usize = 3;
const MAX_CHUNKS_PER_SOURCE: usize = 5;
const MAX_IMAGES: usize = 30;
const MAX_FAILED_RESULTS: usize = 10;

pub(super) struct WebFetchTool {
    api_key: String,
}

impl WebFetchTool {
    pub fn new(api_key: String) -> Self {
        Self { api_key }
    }
}

impl AgentTool for WebFetchTool {
    fn definition(&self) -> AgentToolDefinition {
        AgentToolDefinition {
            name: "web_fetch".to_string(),
            description: "Fetch readable content from a public HTTP(S) URL using Tavily Extract. Sends the URL to an external network service.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "url": { "type": "string", "description": "Public http:// or https:// URL to fetch." },
                    "query": { "type": "string", "description": "Optional extraction query for focused chunks." },
                    "chunksPerSource": { "type": "integer", "minimum": 1, "maximum": MAX_CHUNKS_PER_SOURCE },
                    "extractDepth": { "type": "string", "enum": ["basic", "advanced"] },
                    "format": { "type": "string", "enum": ["markdown", "text"] },
                    "includeImages": { "type": "boolean" },
                    "includeFavicon": { "type": "boolean" },
                    "timeoutSeconds": { "type": "number", "minimum": 1, "maximum": MAX_TIMEOUT_SECONDS },
                    "maxChars": { "type": "integer", "minimum": 1, "maximum": MAX_CONTENT_CHARS }
                },
                "required": ["url"]
            }),
            safety: AgentToolSafety::ReadOnly,
            requires_workspace: false,
            requires_approval: false,
            approval_mode: crate::protocol::AgentToolApprovalMode::Never,
        }
    }

    fn execute(&self, context: &ToolExecutionContext, args: Value) -> AgentResult<Value> {
        let args: WebFetchArgs = serde_json::from_value(args)
            .map_err(|error| AgentError::new(format!("web_fetch 参数无效：{error}")))?;
        let request = TavilyExtractRequest::from_args(args)?;
        let cancellation_token = context.cancellation_token();
        cancellation_token.check()?;
        let response = block_on_tool_future(
            TavilyExtractClient::new(self.api_key.clone())
                .extract(&request, cancellation_token.clone()),
        )?;

        format_tavily_extract_response(request, response, &cancellation_token)
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WebFetchArgs {
    url: String,
    query: Option<String>,
    chunks_per_source: Option<usize>,
    extract_depth: Option<String>,
    format: Option<String>,
    include_images: Option<bool>,
    include_favicon: Option<bool>,
    timeout_seconds: Option<f64>,
    max_chars: Option<usize>,
}

#[derive(Debug)]
struct TavilyExtractRequest {
    url: String,
    query: Option<String>,
    chunks_per_source: Option<usize>,
    extract_depth: String,
    format: String,
    include_images: bool,
    include_favicon: bool,
    timeout_seconds: f64,
    max_chars: usize,
}

impl TavilyExtractRequest {
    fn from_args(args: WebFetchArgs) -> AgentResult<Self> {
        let url = normalize_public_url(&args.url)?;
        let query = args
            .query
            .map(|query| query.trim().to_string())
            .filter(|query| !query.is_empty());

        if args.chunks_per_source.is_some() && query.is_none() {
            return Err(AgentError::new(
                "web_fetch.chunksPerSource 只有在提供 query 时可用。",
            ));
        }

        let extract_depth = args.extract_depth.unwrap_or_else(|| "basic".to_string());
        if !matches!(extract_depth.as_str(), "basic" | "advanced") {
            return Err(AgentError::new(
                "web_fetch.extractDepth 只能是 basic 或 advanced。",
            ));
        }

        let format = args.format.unwrap_or_else(|| "markdown".to_string());
        if !matches!(format.as_str(), "markdown" | "text") {
            return Err(AgentError::new(
                "web_fetch.format 只能是 markdown 或 text。",
            ));
        }

        Ok(Self {
            url,
            query,
            chunks_per_source: args
                .chunks_per_source
                .map(|value| value.clamp(1, MAX_CHUNKS_PER_SOURCE)),
            extract_depth,
            format,
            include_images: args.include_images.unwrap_or(false),
            include_favicon: args.include_favicon.unwrap_or(true),
            timeout_seconds: args
                .timeout_seconds
                .unwrap_or(DEFAULT_TIMEOUT_SECONDS)
                .clamp(1.0, MAX_TIMEOUT_SECONDS),
            max_chars: args
                .max_chars
                .unwrap_or(DEFAULT_MAX_CHARS)
                .clamp(1, MAX_CONTENT_CHARS),
        })
    }

    fn to_payload(&self) -> Value {
        let mut payload = json!({
            "urls": [self.url],
            "extract_depth": self.extract_depth,
            "format": self.format,
            "include_images": self.include_images,
            "include_favicon": self.include_favicon,
            "timeout": self.timeout_seconds,
        });

        if let Some(query) = &self.query {
            payload["query"] = json!(query);
            payload["chunks_per_source"] =
                json!(self.chunks_per_source.unwrap_or(DEFAULT_CHUNKS_PER_SOURCE));
        }

        payload
    }
}

struct TavilyExtractClient {
    api_key: String,
}

impl TavilyExtractClient {
    fn new(api_key: String) -> Self {
        Self { api_key }
    }

    async fn extract(
        &self,
        request: &TavilyExtractRequest,
        cancellation_token: AgentCancellationToken,
    ) -> AgentResult<Value> {
        cancellation_token.check()?;
        let api_key = self.api_key.trim();
        if api_key.is_empty() {
            return Err(AgentError::new("Tavily API Key 为空，无法执行 web_fetch。"));
        }

        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {api_key}"))
                .map_err(|_| AgentError::new("Tavily API Key 包含非法字符。"))?,
        );

        let client = Client::builder()
            .timeout(Duration::from_secs(70))
            .build()
            .map_err(|error| AgentError::new(format!("创建 Tavily HTTP 客户端失败：{error}")))?;
        let response = client
            .post(TAVILY_EXTRACT_ENDPOINT)
            .headers(headers)
            .json(&request.to_payload())
            .send();
        let response = tokio::select! {
            _ = cancellation_token.cancelled() => return Err(AgentError::cancelled()),
            response = response => response
                .map_err(|error| AgentError::new(format!("请求 Tavily 抽取失败：{error}")))?,
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
                "Tavily 抽取返回 {}：{}",
                status.as_u16(),
                body
            )));
        }

        serde_json::from_str(&body)
            .map_err(|error| AgentError::new(format!("Tavily 响应不是有效 JSON：{error}")))
    }
}

fn normalize_public_url(input: &str) -> AgentResult<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(AgentError::new("web_fetch.url 不能为空。"));
    }

    let url = Url::parse(trimmed)
        .map_err(|error| AgentError::new(format!("web_fetch.url 不是有效 URL：{error}")))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(AgentError::new(
            "web_fetch.url 只支持 http:// 或 https:// URL。",
        ));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(AgentError::new("web_fetch.url 不能包含用户名或密码。"));
    }

    let host = url
        .host_str()
        .ok_or_else(|| AgentError::new("web_fetch.url 缺少 host。"))?;
    validate_public_host(host)?;

    Ok(url.to_string())
}

fn validate_public_host(host: &str) -> AgentResult<()> {
    let normalized = host.trim_end_matches('.').to_ascii_lowercase();
    if normalized == "localhost" || normalized.ends_with(".localhost") {
        return Err(AgentError::new("web_fetch.url 不允许访问 localhost。"));
    }
    if normalized == "metadata.google.internal" {
        return Err(AgentError::new("web_fetch.url 不允许访问云元数据地址。"));
    }

    let ip_host = normalized
        .strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
        .unwrap_or(&normalized);
    if let Ok(ip) = ip_host.parse::<IpAddr>() {
        if is_blocked_ip(ip) {
            return Err(AgentError::new(
                "web_fetch.url 不允许访问本地、私有、链路本地或组播 IP。",
            ));
        }
    }

    Ok(())
}

fn is_blocked_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => is_blocked_ipv4(ip),
        IpAddr::V6(ip) => {
            if let Some(mapped) = ip.to_ipv4_mapped() {
                return is_blocked_ipv4(mapped);
            }

            ip.is_loopback()
                || ip.is_unspecified()
                || ip.is_unique_local()
                || ip.is_unicast_link_local()
                || ip.is_multicast()
        }
    }
}

fn is_blocked_ipv4(ip: Ipv4Addr) -> bool {
    ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_broadcast()
        || ip.is_multicast()
}

fn format_tavily_extract_response(
    request: TavilyExtractRequest,
    response: Value,
    cancellation_token: &AgentCancellationToken,
) -> AgentResult<Value> {
    cancellation_token.check()?;
    let result = response
        .get("results")
        .and_then(Value::as_array)
        .and_then(|results| results.first());
    let url = result
        .and_then(|result| result.get("url"))
        .and_then(Value::as_str)
        .unwrap_or(&request.url);
    let raw_results_len = response
        .get("results")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);

    let (content, content_truncated) = result
        .and_then(extract_content)
        .map(|content| truncate_chars(content, request.max_chars))
        .map(|(content, truncated)| (Some(content), truncated))
        .unwrap_or((None, false));

    let images = result
        .and_then(|result| result.get("images"))
        .and_then(Value::as_array)
        .map(|images| images.iter().take(MAX_IMAGES).cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    let images_truncated = result
        .and_then(|result| result.get("images"))
        .and_then(Value::as_array)
        .map(|images| images.len() > MAX_IMAGES)
        .unwrap_or(false);
    let failed_results = format_failed_results(&response);
    let failed_results_truncated = response
        .get("failed_results")
        .or_else(|| response.get("failedResults"))
        .and_then(Value::as_array)
        .map(|failed_results| failed_results.len() > MAX_FAILED_RESULTS)
        .unwrap_or(false);
    if content_is_empty(content.as_deref()) && !failed_results.is_empty() {
        return Err(AgentError::new(format_failed_extract_error(
            &request.url,
            &failed_results,
        )));
    }
    let response_time = response
        .get("response_time")
        .or_else(|| response.get("responseTime"))
        .cloned();

    // Tavily calls the extracted body `raw_content`, but after normalization and truncation this
    // is our canonical model-facing content. Do not duplicate it under a `rawContent` alias: every
    // later tool iteration would otherwise resend the same page body twice.
    Ok(json!({
        "url": url,
        "requestedUrl": request.url,
        "provider": "tavily",
        "format": request.format,
        "extractDepth": request.extract_depth,
        "content": content,
        "images": images,
        "favicon": result
            .and_then(|result| result.get("favicon"))
            .and_then(Value::as_str),
        "failedResults": failed_results,
        "responseTime": response_time,
        "truncated": content_truncated
            || images_truncated
            || failed_results_truncated
            || raw_results_len > 1
    }))
}

fn extract_content(result: &Value) -> Option<&str> {
    result
        .get("raw_content")
        .or_else(|| result.get("rawContent"))
        .or_else(|| result.get("content"))
        .and_then(Value::as_str)
}

fn content_is_empty(content: Option<&str>) -> bool {
    content.map(str::trim).unwrap_or_default().is_empty()
}

fn format_failed_results(response: &Value) -> Vec<Value> {
    response
        .get("failed_results")
        .or_else(|| response.get("failedResults"))
        .and_then(Value::as_array)
        .map(|failed_results| {
            failed_results
                .iter()
                .take(MAX_FAILED_RESULTS)
                .map(|result| {
                    json!({
                        "url": result.get("url").and_then(Value::as_str),
                        "error": result
                            .get("error")
                            .or_else(|| result.get("message"))
                            .and_then(Value::as_str)
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

fn format_failed_extract_error(requested_url: &str, failed_results: &[Value]) -> String {
    let first = failed_results.first();
    let failed_url = first
        .and_then(|result| result.get("url"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|url| !url.is_empty());
    let error = first
        .and_then(|result| result.get("error"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|error| !error.is_empty());

    match (failed_url, error) {
        (Some(failed_url), Some(error)) => format!(
            "web_fetch 未能抽取可读正文：requestedUrl={requested_url}，failedUrl={failed_url}，error={error}"
        ),
        (Some(failed_url), None) => format!(
            "web_fetch 未能抽取可读正文：requestedUrl={requested_url}，failedUrl={failed_url}"
        ),
        (None, Some(error)) => {
            format!("web_fetch 未能抽取可读正文：requestedUrl={requested_url}，error={error}")
        }
        (None, None) => {
            format!("web_fetch 未能抽取可读正文：requestedUrl={requested_url}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_bounded_tavily_payload() {
        let request = TavilyExtractRequest::from_args(WebFetchArgs {
            url: " https://example.com/docs ".to_string(),
            query: Some(" rust ".to_string()),
            chunks_per_source: Some(999),
            extract_depth: Some("advanced".to_string()),
            format: Some("text".to_string()),
            include_images: Some(true),
            include_favicon: Some(true),
            timeout_seconds: Some(999.0),
            max_chars: Some(usize::MAX),
        })
        .unwrap();
        let payload = request.to_payload();

        assert_eq!(request.chunks_per_source, Some(MAX_CHUNKS_PER_SOURCE));
        assert_eq!(request.max_chars, MAX_CONTENT_CHARS);
        assert_eq!(payload["urls"][0], "https://example.com/docs");
        assert_eq!(payload["query"], "rust");
        assert_eq!(payload["chunks_per_source"], MAX_CHUNKS_PER_SOURCE);
        assert_eq!(payload["extract_depth"], "advanced");
        assert_eq!(payload["format"], "text");
        assert_eq!(payload["include_images"], true);
        assert_eq!(payload["include_favicon"], true);
        assert_eq!(payload["timeout"], MAX_TIMEOUT_SECONDS);
    }

    #[test]
    fn rejects_unsupported_or_local_urls() {
        for url in [
            "file:///tmp/notes.txt",
            "http://localhost:8000",
            "https://127.0.0.1/private",
            "https://10.0.0.1/private",
            "http://169.254.169.254/latest",
            "http://[::1]/private",
            "https://user:pass@example.com/private",
        ] {
            let error = TavilyExtractRequest::from_args(WebFetchArgs {
                url: url.to_string(),
                query: None,
                chunks_per_source: None,
                extract_depth: None,
                format: None,
                include_images: None,
                include_favicon: None,
                timeout_seconds: None,
                max_chars: None,
            })
            .unwrap_err();

            assert!(
                error.to_string().contains("web_fetch.url"),
                "{url} produced {error}"
            );
        }
    }

    #[test]
    fn rejects_chunks_without_query() {
        let error = TavilyExtractRequest::from_args(WebFetchArgs {
            url: "https://example.com".to_string(),
            query: None,
            chunks_per_source: Some(2),
            extract_depth: None,
            format: None,
            include_images: None,
            include_favicon: None,
            timeout_seconds: None,
            max_chars: None,
        })
        .unwrap_err();

        assert!(error.to_string().contains("chunksPerSource"));
    }

    #[test]
    fn rejects_empty_content_with_failed_results() {
        let request = TavilyExtractRequest::from_args(WebFetchArgs {
            url: "https://example.com/missing".to_string(),
            query: None,
            chunks_per_source: None,
            extract_depth: None,
            format: None,
            include_images: None,
            include_favicon: None,
            timeout_seconds: None,
            max_chars: None,
        })
        .unwrap();
        let response = json!({
            "results": [],
            "failed_results": [{
                "url": "https://example.com/missing",
                "error": "404 Not Found"
            }]
        });

        let error =
            format_tavily_extract_response(request, response, &AgentCancellationToken::new())
                .unwrap_err();

        assert!(error.to_string().contains("未能抽取可读正文"));
        assert!(error.to_string().contains("https://example.com/missing"));
        assert!(error.to_string().contains("404 Not Found"));
    }

    #[test]
    fn formats_extract_response_with_truncation() {
        let request = TavilyExtractRequest::from_args(WebFetchArgs {
            url: "https://example.com/docs".to_string(),
            query: None,
            chunks_per_source: None,
            extract_depth: None,
            format: None,
            include_images: None,
            include_favicon: None,
            timeout_seconds: None,
            max_chars: Some(5),
        })
        .unwrap();
        let response = json!({
            "results": [{
                "url": "https://example.com/docs",
                "raw_content": "hello world",
                "images": ["one", "two"],
                "favicon": "https://example.com/favicon.ico"
            }],
            "failed_results": [],
            "response_time": 1.23
        });

        let formatted =
            format_tavily_extract_response(request, response, &AgentCancellationToken::new())
                .unwrap();

        assert_eq!(formatted["provider"], "tavily");
        assert_eq!(formatted["content"], "hello\n...[truncated]");
        assert!(formatted.get("rawContent").is_none());
        assert_eq!(formatted["favicon"], "https://example.com/favicon.ico");
        assert!(formatted.get("faviconDataUrl").is_none());
        assert!(formatted.get("faviconMimeType").is_none());
        assert_eq!(formatted["truncated"], true);
    }
}
