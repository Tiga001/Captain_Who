// Server-sent event parsing and stream accumulation for LLM responses.
use super::response::{extract_api_error, parse_tool_arguments, truncate_for_error};
use super::{LlmChatResponse, LlmToolCall};
use crate::cancellation::AgentCancellationToken;
use crate::protocol::{AgentApiStyle, AgentError, AgentResult, AgentUsage};
use crate::usage::{extract_anthropic_stream_usage, extract_usage, merge_stream_usage};
use futures_util::StreamExt;
use serde_json::{json, Value};
use std::collections::BTreeMap;

pub(super) async fn parse_sse_response<F>(
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

pub(super) fn process_sse_frame<F>(
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

pub(super) enum LlmStreamAccumulator {
    OpenAi(OpenAiStreamAccumulator),
    Anthropic(AnthropicStreamAccumulator),
}

impl LlmStreamAccumulator {
    pub(super) fn new(api_style: AgentApiStyle) -> Self {
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

    pub(super) fn finish(self) -> AgentResult<LlmChatResponse> {
        match self {
            Self::OpenAi(accumulator) => accumulator.finish(),
            Self::Anthropic(accumulator) => accumulator.finish(),
        }
    }
}

#[derive(Default)]
pub(super) struct OpenAiStreamAccumulator {
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
                        append_argument_stream_fragment(&mut entry.arguments, arguments);
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
pub(super) struct AnthropicStreamAccumulator {
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

fn append_argument_stream_fragment(target: &mut String, fragment: &str) {
    if fragment.is_empty() {
        return;
    }

    // Some OpenAI-compatible streaming APIs send cumulative function arguments snapshots instead
    // of strict deltas. Appending those snapshots creates `{}{}`
    // and later fails JSON parsing with "trailing characters".
    if target.is_empty() {
        target.push_str(fragment);
    } else if fragment.starts_with(target.as_str()) {
        target.clear();
        target.push_str(fragment);
    } else if !target.ends_with(fragment) {
        target.push_str(fragment);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn openai_tool_call_frame(arguments: &str) -> String {
        format!(
            "data: {}\n\n",
            json!({
                "choices": [
                    {
                        "delta": {
                            "tool_calls": [
                                {
                                    "index": 0,
                                    "id": "call-1",
                                    "function": {
                                        "name": "run_command",
                                        "arguments": arguments
                                    }
                                }
                            ]
                        }
                    }
                ]
            })
        )
    }

    #[test]
    fn openai_stream_accepts_incremental_tool_arguments() {
        let mut accumulator = LlmStreamAccumulator::new(AgentApiStyle::OpenAiCompatible);
        let mut deltas = Vec::new();

        process_sse_frame(
            &openai_tool_call_frame("{\"command\":\"conda env list\""),
            &mut accumulator,
            &mut |delta| deltas.push(delta),
        )
        .unwrap();
        process_sse_frame(
            &openai_tool_call_frame(",\"reason\":\"检查环境\"}"),
            &mut accumulator,
            &mut |delta| deltas.push(delta),
        )
        .unwrap();

        let response = accumulator.finish().unwrap();
        assert_eq!(response.tool_calls.len(), 1);
        assert_eq!(response.tool_calls[0].name, "run_command");
        assert_eq!(response.tool_calls[0].args["command"], "conda env list");
        assert_eq!(response.tool_calls[0].args["reason"], "检查环境");
    }

    #[test]
    fn openai_stream_accepts_cumulative_tool_arguments() {
        let mut accumulator = LlmStreamAccumulator::new(AgentApiStyle::OpenAiCompatible);
        let mut deltas = Vec::new();

        process_sse_frame(
            &openai_tool_call_frame("{\"command\":\"conda env list\""),
            &mut accumulator,
            &mut |delta| deltas.push(delta),
        )
        .unwrap();
        process_sse_frame(
            &openai_tool_call_frame("{\"command\":\"conda env list\",\"reason\":\"检查环境\"}"),
            &mut accumulator,
            &mut |delta| deltas.push(delta),
        )
        .unwrap();

        let response = accumulator.finish().unwrap();
        assert_eq!(response.tool_calls.len(), 1);
        assert_eq!(response.tool_calls[0].name, "run_command");
        assert_eq!(response.tool_calls[0].args["command"], "conda env list");
        assert_eq!(response.tool_calls[0].args["reason"], "检查环境");
    }
}
