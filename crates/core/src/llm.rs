// Rust agent core.
use crate::cancellation::AgentCancellationToken;
use crate::protocol::{AgentApiStyle, AgentError, AgentResult, AgentToolDefinition, AgentUsage};
use crate::usage::{extract_anthropic_stream_usage, extract_usage, merge_stream_usage};
use futures_util::StreamExt;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::time::Duration;

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

fn extract_api_error(value: &Value) -> Option<String> {
    let error = value.get("error")?;
    if let Some(message) = error
        .as_str()
        .map(str::trim)
        .filter(|message| !message.is_empty())
    {
        return Some(message.to_string());
    }

    let message = error
        .get("message")
        .and_then(Value::as_str)
        .or_else(|| error.get("error").and_then(Value::as_str))
        .or_else(|| error.get("type").and_then(Value::as_str))
        .map(str::trim)
        .filter(|message| !message.is_empty());
    if let Some(message) = message {
        return Some(message.to_string());
    }

    Some(truncate_for_error(&error.to_string()))
}

async fn parse_sse_response<F>(
    response: reqwest::Response,
    api_style: AgentApiStyle,
    cancellation_token: AgentCancellationToken,
    mut on_delta: F,
) -> AgentResult<LlmChatResponse>
where
    F: FnMut(String) + Send,
{
    let mut stream = response.bytes_stream();
    let mut buffer = Vec::<u8>::new();
    let mut accumulator = LlmStreamAccumulator::new(api_style);

    loop {
        cancellation_token.check()?;
        let chunk = tokio::select! {
            _ = cancellation_token.cancelled() => return Err(AgentError::cancelled()),
            chunk = stream.next() => {
                let Some(chunk) = chunk else {
                    break;
                };
                chunk.map_err(|error| AgentError::new(format!("读取模型流失败：{error}")))?
            }
        };
        buffer.extend_from_slice(&chunk);

        while let Some((frame_end, separator_len)) = find_sse_frame_end(&buffer) {
            cancellation_token.check()?;
            let frame_bytes = buffer[..frame_end].to_vec();
            buffer.drain(..frame_end + separator_len);
            let frame = String::from_utf8(frame_bytes)
                .map_err(|error| AgentError::new(format!("模型流不是有效 UTF-8：{error}")))?;
            process_sse_frame(&frame, &mut accumulator, &mut on_delta)?;
        }
    }

    cancellation_token.check()?;
    if !buffer.iter().all(u8::is_ascii_whitespace) {
        let frame = String::from_utf8(buffer)
            .map_err(|error| AgentError::new(format!("模型流尾部不是有效 UTF-8：{error}")))?;
        process_sse_frame(&frame, &mut accumulator, &mut on_delta)?;
    }

    accumulator.finish()
}

fn process_sse_frame<F>(
    frame: &str,
    accumulator: &mut LlmStreamAccumulator,
    on_delta: &mut F,
) -> AgentResult<()>
where
    F: FnMut(String),
{
    let frame = parse_sse_frame(frame);
    let data = frame.data.trim();
    if data.is_empty() || data == "[DONE]" {
        return Ok(());
    }

    let value: Value = serde_json::from_str(data).map_err(|error| {
        AgentError::new(format!(
            "模型流事件不是有效 JSON：{error}；事件：{}",
            truncate_for_error(data)
        ))
    })?;
    if let Some(error) = extract_api_error(&value) {
        return Err(AgentError::new(format!("模型接口返回错误：{error}")));
    }
    accumulator.process(frame.event.as_deref(), &value, on_delta)
}

#[derive(Debug)]
struct SseFrame {
    event: Option<String>,
    data: String,
}

fn parse_sse_frame(frame: &str) -> SseFrame {
    let mut event = None;
    let mut data_lines = Vec::new();

    for raw_line in frame.lines() {
        let line = raw_line.trim_end_matches('\r');
        if line.starts_with(':') {
            continue;
        }
        if let Some(value) = line.strip_prefix("event:") {
            event = Some(value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("data:") {
            data_lines.push(value.trim_start().to_string());
        }
    }

    SseFrame {
        event,
        data: data_lines.join("\n"),
    }
}

fn find_sse_frame_end(buffer: &[u8]) -> Option<(usize, usize)> {
    let lf = find_bytes(buffer, b"\n\n").map(|index| (index, 2));
    let crlf = find_bytes(buffer, b"\r\n\r\n").map(|index| (index, 4));

    match (lf, crlf) {
        (Some(left), Some(right)) => Some(if left.0 <= right.0 { left } else { right }),
        (Some(value), None) | (None, Some(value)) => Some(value),
        (None, None) => None,
    }
}

fn find_bytes(buffer: &[u8], needle: &[u8]) -> Option<usize> {
    buffer
        .windows(needle.len())
        .position(|window| window == needle)
}

enum LlmStreamAccumulator {
    OpenAi(OpenAiStreamAccumulator),
    Anthropic(AnthropicStreamAccumulator),
}

impl LlmStreamAccumulator {
    fn new(api_style: AgentApiStyle) -> Self {
        match api_style {
            AgentApiStyle::OpenAiCompatible => Self::OpenAi(OpenAiStreamAccumulator::default()),
            AgentApiStyle::AnthropicCompatible => {
                Self::Anthropic(AnthropicStreamAccumulator::default())
            }
        }
    }

    fn process<F>(
        &mut self,
        event: Option<&str>,
        value: &Value,
        on_delta: &mut F,
    ) -> AgentResult<()>
    where
        F: FnMut(String),
    {
        match self {
            Self::OpenAi(accumulator) => accumulator.process(value, on_delta),
            Self::Anthropic(accumulator) => accumulator.process(event, value, on_delta),
        }
    }

    fn finish(self) -> AgentResult<LlmChatResponse> {
        match self {
            Self::OpenAi(accumulator) => accumulator.finish(),
            Self::Anthropic(accumulator) => accumulator.finish(),
        }
    }
}

#[derive(Default)]
struct OpenAiStreamAccumulator {
    content: String,
    tool_calls: BTreeMap<usize, OpenAiToolCallAccumulator>,
    usage: Option<AgentUsage>,
    finish_reason: Option<String>,
}

#[derive(Default)]
struct OpenAiToolCallAccumulator {
    id: Option<String>,
    name: String,
    arguments: String,
}

impl OpenAiStreamAccumulator {
    fn process<F>(&mut self, value: &Value, on_delta: &mut F) -> AgentResult<()>
    where
        F: FnMut(String),
    {
        merge_stream_usage(&mut self.usage, extract_usage(value));

        let Some(choices) = value.get("choices").and_then(Value::as_array) else {
            return Ok(());
        };

        for choice in choices {
            if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
                self.finish_reason = Some(reason.to_string());
            }

            let Some(delta) = choice.get("delta") else {
                continue;
            };
            if let Some(content) = delta.get("content").and_then(Value::as_str) {
                if !content.is_empty() {
                    self.content.push_str(content);
                    on_delta(content.to_string());
                }
            }

            let Some(tool_calls) = delta.get("tool_calls").and_then(Value::as_array) else {
                continue;
            };
            for (fallback_index, call) in tool_calls.iter().enumerate() {
                let index = call
                    .get("index")
                    .and_then(Value::as_u64)
                    .map(|index| index as usize)
                    .unwrap_or(fallback_index);
                let entry = self.tool_calls.entry(index).or_default();
                if let Some(id) = call.get("id").and_then(Value::as_str) {
                    if !id.trim().is_empty() {
                        entry.id = Some(id.to_string());
                    }
                }
                if let Some(function) = call.get("function") {
                    if let Some(name) = function.get("name").and_then(Value::as_str) {
                        append_stream_fragment(&mut entry.name, name);
                    }
                    if let Some(arguments) = function.get("arguments").and_then(Value::as_str) {
                        entry.arguments.push_str(arguments);
                    }
                }
            }
        }

        Ok(())
    }

    fn finish(self) -> AgentResult<LlmChatResponse> {
        let mut tool_calls = Vec::new();
        for (index, call) in self.tool_calls {
            if call.name.trim().is_empty() {
                continue;
            }
            let args = parse_tool_arguments(&call.arguments).map_err(|error| {
                AgentError::new(format!(
                    "OpenAI 流式 tool_call `{}` 的 arguments 不是有效 JSON：{error}",
                    call.name
                ))
            })?;
            tool_calls.push(LlmToolCall {
                id: call
                    .id
                    .unwrap_or_else(|| format!("openai-stream-tool-call-{}", index + 1)),
                name: call.name,
                args,
            });
        }

        Ok(LlmChatResponse {
            content: self.content,
            tool_calls,
            usage: self.usage,
            finish_reason: self.finish_reason,
        })
    }
}

#[derive(Default)]
struct AnthropicStreamAccumulator {
    content: String,
    blocks: BTreeMap<usize, AnthropicBlockAccumulator>,
    usage: Option<AgentUsage>,
    finish_reason: Option<String>,
}

#[derive(Default)]
struct AnthropicBlockAccumulator {
    kind: String,
    id: Option<String>,
    name: Option<String>,
    input_json: String,
}

impl AnthropicStreamAccumulator {
    fn process<F>(
        &mut self,
        event: Option<&str>,
        value: &Value,
        on_delta: &mut F,
    ) -> AgentResult<()>
    where
        F: FnMut(String),
    {
        let event_kind = event
            .filter(|event| !event.trim().is_empty())
            .or_else(|| value.get("type").and_then(Value::as_str))
            .unwrap_or_default();

        match event_kind {
            "message_start" => {
                merge_stream_usage(&mut self.usage, extract_anthropic_stream_usage(value));
            }
            "content_block_start" => {
                self.process_content_block_start(value, on_delta)?;
            }
            "content_block_delta" => {
                self.process_content_block_delta(value, on_delta)?;
            }
            "message_delta" => {
                if let Some(reason) = value
                    .get("delta")
                    .and_then(|delta| delta.get("stop_reason"))
                    .and_then(Value::as_str)
                {
                    self.finish_reason = Some(reason.to_string());
                }
                merge_stream_usage(&mut self.usage, extract_anthropic_stream_usage(value));
            }
            "error" => {
                let message = value
                    .get("error")
                    .and_then(|error| error.get("message"))
                    .and_then(Value::as_str)
                    .or_else(|| value.get("message").and_then(Value::as_str))
                    .unwrap_or("Anthropic stream error");
                return Err(AgentError::new(format!("模型流返回错误：{message}")));
            }
            "ping" | "content_block_stop" | "message_stop" => {}
            _ => {}
        }

        Ok(())
    }

    fn process_content_block_start<F>(&mut self, value: &Value, on_delta: &mut F) -> AgentResult<()>
    where
        F: FnMut(String),
    {
        let index = value
            .get("index")
            .and_then(Value::as_u64)
            .map(|index| index as usize)
            .unwrap_or_else(|| self.blocks.len());
        let content_block = value.get("content_block").unwrap_or(&Value::Null);
        let kind = content_block
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let block = self.blocks.entry(index).or_default();
        block.kind = kind.to_string();

        match kind {
            "text" => {
                if let Some(text) = content_block.get("text").and_then(Value::as_str) {
                    if !text.is_empty() {
                        self.content.push_str(text);
                        on_delta(text.to_string());
                    }
                }
            }
            "tool_use" => {
                block.id = content_block
                    .get("id")
                    .and_then(Value::as_str)
                    .map(ToString::to_string);
                block.name = content_block
                    .get("name")
                    .and_then(Value::as_str)
                    .map(ToString::to_string);
                if let Some(input) = content_block.get("input") {
                    if !input.is_null() && input != &json!({}) {
                        block.input_json = serde_json::to_string(input).unwrap_or_default();
                    }
                }
            }
            _ => {}
        }

        Ok(())
    }

    fn process_content_block_delta<F>(&mut self, value: &Value, on_delta: &mut F) -> AgentResult<()>
    where
        F: FnMut(String),
    {
        let index = value
            .get("index")
            .and_then(Value::as_u64)
            .map(|index| index as usize)
            .unwrap_or_else(|| self.blocks.len().saturating_sub(1));
        let delta = value.get("delta").unwrap_or(&Value::Null);
        let kind = delta
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let block = self.blocks.entry(index).or_default();

        match kind {
            "text_delta" => {
                let text = delta
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if !text.is_empty() {
                    block.kind = "text".to_string();
                    self.content.push_str(text);
                    on_delta(text.to_string());
                }
            }
            "input_json_delta" => {
                let partial_json = delta
                    .get("partial_json")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                block.kind = "tool_use".to_string();
                block.input_json.push_str(partial_json);
            }
            _ => {}
        }

        Ok(())
    }

    fn finish(self) -> AgentResult<LlmChatResponse> {
        let mut tool_calls = Vec::new();
        for (index, block) in self.blocks {
            if block.kind != "tool_use" {
                continue;
            }
            let Some(name) = block.name.filter(|name| !name.trim().is_empty()) else {
                continue;
            };
            let args = parse_tool_arguments(&block.input_json).map_err(|error| {
                AgentError::new(format!(
                    "Anthropic 流式 tool_use `{name}` 的 input 不是有效 JSON：{error}"
                ))
            })?;
            tool_calls.push(LlmToolCall {
                id: block
                    .id
                    .unwrap_or_else(|| format!("anthropic-stream-tool-use-{}", index + 1)),
                name,
                args,
            });
        }

        Ok(LlmChatResponse {
            content: self.content,
            tool_calls,
            usage: self.usage,
            finish_reason: self.finish_reason,
        })
    }
}

fn append_stream_fragment(target: &mut String, fragment: &str) {
    if fragment.is_empty() {
        return;
    }
    if target.is_empty() || !target.ends_with(fragment) {
        target.push_str(fragment);
    }
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

fn build_payload(request: &LlmChatRequest) -> Value {
    match request.api_style {
        AgentApiStyle::OpenAiCompatible => {
            let mut payload = Map::from_iter([
                ("model".to_string(), json!(request.model)),
                (
                    "messages".to_string(),
                    Value::Array(build_openai_messages(&request.messages)),
                ),
                ("stream".to_string(), json!(request.stream)),
                ("max_tokens".to_string(), json!(request.max_tokens)),
            ]);
            if should_send_temperature(&request.model) {
                payload.insert("temperature".to_string(), json!(request.temperature));
            }
            if request.stream {
                payload.insert(
                    "stream_options".to_string(),
                    json!({ "include_usage": true }),
                );
            }

            if !request.tools.is_empty() {
                payload.insert(
                    "tools".to_string(),
                    Value::Array(build_openai_tools(&request.tools)),
                );
                payload.insert("tool_choice".to_string(), json!("auto"));
            }

            Value::Object(payload)
        }
        AgentApiStyle::AnthropicCompatible => {
            let (system, messages) = split_anthropic_messages(&request.messages);
            let mut payload = Map::from_iter([
                ("model".to_string(), json!(request.model)),
                ("max_tokens".to_string(), json!(request.max_tokens)),
                ("messages".to_string(), json!(messages)),
            ]);
            if should_send_temperature(&request.model) {
                payload.insert("temperature".to_string(), json!(request.temperature));
            }

            if request.stream {
                payload.insert("stream".to_string(), json!(true));
            }
            if let Some(system) = system {
                payload.insert("system".to_string(), json!(system));
            }
            if !request.tools.is_empty() {
                payload.insert(
                    "tools".to_string(),
                    Value::Array(build_anthropic_tools(&request.tools)),
                );
            }

            Value::Object(payload)
        }
    }
}

fn should_send_temperature(model: &str) -> bool {
    !model.to_ascii_lowercase().contains("claude")
}

fn is_sse_response(response: &reqwest::Response) -> bool {
    response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.to_ascii_lowercase().contains("text/event-stream"))
        .unwrap_or(false)
}

fn build_openai_messages(messages: &[LlmMessage]) -> Vec<Value> {
    messages
        .iter()
        .map(|message| match message.role {
            LlmMessageRole::System => json!({ "role": "system", "content": message.content }),
            LlmMessageRole::User => json!({
                "role": "user",
                "content": build_openai_user_content(message)
            }),
            LlmMessageRole::Assistant => {
                let mut object = Map::from_iter([(
                    "role".to_string(),
                    Value::String(message.role.as_str().to_string()),
                )]);
                if message.tool_calls.is_empty() {
                    object.insert("content".to_string(), json!(message.content));
                } else {
                    object.insert(
                        "content".to_string(),
                        if message.content.trim().is_empty() {
                            Value::Null
                        } else {
                            json!(message.content)
                        },
                    );
                    object.insert(
                        "tool_calls".to_string(),
                        Value::Array(build_openai_tool_calls(&message.tool_calls)),
                    );
                }
                Value::Object(object)
            }
            LlmMessageRole::Tool => json!({
                "role": "tool",
                "tool_call_id": message.tool_call_id.as_deref().unwrap_or_default(),
                "content": message.content
            }),
        })
        .collect()
}

fn build_openai_user_content(message: &LlmMessage) -> Value {
    if message.images.is_empty() {
        return json!(message.content);
    }

    let mut parts = Vec::new();
    if !message.content.trim().is_empty() {
        parts.push(json!({
            "type": "text",
            "text": message.content
        }));
    }
    parts.extend(message.images.iter().map(|image| {
        json!({
            "type": "image_url",
            "image_url": {
                "url": format!("data:{};base64,{}", image.mime_type, image.data_base64)
            }
        })
    }));

    Value::Array(parts)
}

fn build_openai_tools(tools: &[AgentToolDefinition]) -> Vec<Value> {
    tools
        .iter()
        .map(|tool| {
            json!({
                "type": "function",
                "function": {
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": normalize_json_schema(&tool.input_schema)
                }
            })
        })
        .collect()
}

fn build_openai_tool_calls(tool_calls: &[LlmToolCall]) -> Vec<Value> {
    tool_calls
        .iter()
        .map(|call| {
            json!({
                "id": call.id,
                "type": "function",
                "function": {
                    "name": call.name,
                    "arguments": serde_json::to_string(&call.args).unwrap_or_else(|_| "{}".to_string())
                }
            })
        })
        .collect()
}

fn split_anthropic_messages(messages: &[LlmMessage]) -> (Option<String>, Vec<Value>) {
    let mut system_parts = Vec::new();
    let mut chat_messages = Vec::new();

    for message in messages {
        match message.role {
            LlmMessageRole::System => system_parts.push(message.content.as_str()),
            LlmMessageRole::User => {
                let mut blocks = Vec::new();
                if !message.content.trim().is_empty() {
                    blocks.push(json!({ "type": "text", "text": message.content }));
                }
                blocks.extend(message.images.iter().map(|image| {
                    json!({
                        "type": "image",
                        "source": {
                            "type": "base64",
                            "media_type": image.mime_type,
                            "data": image.data_base64
                        }
                    })
                }));
                push_anthropic_message(&mut chat_messages, "user", blocks);
            }
            LlmMessageRole::Assistant => {
                let mut blocks = Vec::new();
                if !message.content.trim().is_empty() {
                    blocks.push(json!({ "type": "text", "text": message.content }));
                }
                blocks.extend(message.tool_calls.iter().map(|call| {
                    json!({
                        "type": "tool_use",
                        "id": call.id,
                        "name": call.name,
                        "input": call.args
                    })
                }));
                push_anthropic_message(&mut chat_messages, "assistant", blocks);
            }
            LlmMessageRole::Tool => {
                push_anthropic_message(
                    &mut chat_messages,
                    "user",
                    vec![json!({
                        "type": "tool_result",
                        "tool_use_id": message.tool_call_id.as_deref().unwrap_or_default(),
                        "content": message.content,
                        "is_error": message.is_error
                    })],
                );
            }
        }
    }

    let system = if system_parts.is_empty() {
        None
    } else {
        Some(system_parts.join("\n\n"))
    };

    (system, chat_messages)
}

fn push_anthropic_message(messages: &mut Vec<Value>, role: &str, content_blocks: Vec<Value>) {
    if content_blocks.is_empty() {
        return;
    }

    if let Some(last) = messages.last_mut() {
        let same_role = last
            .get("role")
            .and_then(Value::as_str)
            .map(|value| value == role)
            .unwrap_or(false);
        if same_role {
            if let Some(content) = last.get_mut("content").and_then(Value::as_array_mut) {
                content.extend(content_blocks);
                return;
            }
        }
    }

    messages.push(json!({
        "role": role,
        "content": content_blocks
    }));
}

fn build_anthropic_tools(tools: &[AgentToolDefinition]) -> Vec<Value> {
    tools
        .iter()
        .map(|tool| {
            json!({
                "name": tool.name,
                "description": tool.description,
                "input_schema": normalize_json_schema(&tool.input_schema)
            })
        })
        .collect()
}

fn normalize_json_schema(schema: &Value) -> Value {
    match schema {
        Value::Object(_) => schema.clone(),
        _ => json!({ "type": "object", "properties": {} }),
    }
}

fn build_headers(api_style: AgentApiStyle, api_token: &str) -> AgentResult<HeaderMap> {
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

    match api_style {
        AgentApiStyle::OpenAiCompatible => {
            let bearer = format!("Bearer {api_token}");
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&bearer)
                    .map_err(|_| AgentError::new("API Token 包含非法字符。"))?,
            );
        }
        AgentApiStyle::AnthropicCompatible => {
            headers.insert(
                "x-api-key",
                HeaderValue::from_str(api_token)
                    .map_err(|_| AgentError::new("API Token 包含非法字符。"))?,
            );
            headers.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));
        }
    }

    Ok(headers)
}

fn extract_response_text(value: &Value) -> Option<String> {
    if let Some(text) = value.get("content").and_then(extract_content_text) {
        return Some(text);
    }

    if let Some(text) = value
        .get("message")
        .and_then(|message| message.get("content"))
        .and_then(extract_content_text)
    {
        return Some(text);
    }

    if let Some(text) = value.get("completion").and_then(Value::as_str) {
        return Some(text.to_string());
    }

    let choice = value.get("choices")?.as_array()?.first()?;

    if let Some(text) = choice
        .get("message")
        .and_then(|message| message.get("content"))
        .and_then(extract_content_text)
    {
        return Some(text);
    }

    choice
        .get("text")
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn extract_content_text(content: &Value) -> Option<String> {
    if let Some(text) = content.as_str() {
        return Some(text.to_string());
    }

    let parts = content.as_array()?;
    let text = parts
        .iter()
        .filter_map(|part| {
            part.get("text")
                .and_then(Value::as_str)
                .or_else(|| part.get("content").and_then(Value::as_str))
        })
        .collect::<Vec<_>>()
        .join("");

    Some(text)
}

fn extract_finish_reason(value: &Value) -> Option<String> {
    if let Some(reason) = value.get("stop_reason").and_then(Value::as_str) {
        return Some(reason.to_string());
    }

    let choice = value.get("choices")?.as_array()?.first()?;
    choice
        .get("finish_reason")
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn extract_tool_calls(value: &Value, api_style: AgentApiStyle) -> AgentResult<Vec<LlmToolCall>> {
    match api_style {
        AgentApiStyle::OpenAiCompatible => extract_openai_tool_calls(value),
        AgentApiStyle::AnthropicCompatible => extract_anthropic_tool_calls(value),
    }
}

fn extract_openai_tool_calls(value: &Value) -> AgentResult<Vec<LlmToolCall>> {
    let Some(calls) = value
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("message"))
        .and_then(|message| message.get("tool_calls"))
        .and_then(Value::as_array)
    else {
        return Ok(Vec::new());
    };

    calls
        .iter()
        .enumerate()
        .map(|(index, call)| {
            let function = call
                .get("function")
                .ok_or_else(|| AgentError::new("OpenAI tool_call 缺少 function 字段。"))?;
            let name = function
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .ok_or_else(|| AgentError::new("OpenAI tool_call 缺少 function.name。"))?;
            let arguments = function
                .get("arguments")
                .and_then(Value::as_str)
                .unwrap_or("{}");
            let args = parse_tool_arguments(arguments).map_err(|error| {
                AgentError::new(format!(
                    "OpenAI tool_call `{name}` 的 arguments 不是有效 JSON：{error}"
                ))
            })?;
            let id = call
                .get("id")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|id| !id.is_empty())
                .map(ToString::to_string)
                .unwrap_or_else(|| format!("openai-tool-call-{}", index + 1));

            Ok(LlmToolCall {
                id,
                name: name.to_string(),
                args,
            })
        })
        .collect()
}

fn extract_anthropic_tool_calls(value: &Value) -> AgentResult<Vec<LlmToolCall>> {
    let Some(content) = value.get("content").and_then(Value::as_array) else {
        return Ok(Vec::new());
    };

    content
        .iter()
        .filter(|part| {
            part.get("type")
                .and_then(Value::as_str)
                .map(|kind| kind == "tool_use")
                .unwrap_or(false)
        })
        .enumerate()
        .map(|(index, part)| {
            let name = part
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .ok_or_else(|| AgentError::new("Anthropic tool_use 缺少 name。"))?;
            let id = part
                .get("id")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|id| !id.is_empty())
                .map(ToString::to_string)
                .unwrap_or_else(|| format!("anthropic-tool-use-{}", index + 1));
            let args = part.get("input").cloned().unwrap_or_else(|| json!({}));

            Ok(LlmToolCall {
                id,
                name: name.to_string(),
                args,
            })
        })
        .collect()
}

fn parse_tool_arguments(arguments: &str) -> serde_json::Result<Value> {
    let arguments = arguments.trim();
    if arguments.is_empty() {
        return Ok(json!({}));
    }

    serde_json::from_str(arguments)
}

fn truncate_for_error(value: &str) -> String {
    const MAX_CHARS: usize = 600;

    let mut truncated = value.chars().take(MAX_CHARS).collect::<String>();
    if value.chars().count() > MAX_CHARS {
        truncated.push('…');
    }

    truncated
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::AgentToolSafety;

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
