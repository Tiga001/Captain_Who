// Server-sent event parsing and stream accumulation for LLM responses.
use super::response::{extract_api_error, parse_tool_arguments, truncate_for_error};
use super::{LlmChatResponse, LlmStreamEvent, LlmToolCall};
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
    F: FnMut(LlmStreamEvent) + Send,
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
                chunk.map_err(|error| {
                    AgentError::new(format!("读取模型流失败：{error}"))
                        .with_usage(accumulator.usage().cloned())
                })?
            }
        };
        buffer.extend_from_slice(&chunk);

        while let Some((frame_end, separator_len)) = find_sse_frame_end(&buffer) {
            cancellation_token.check()?;
            let frame_bytes = buffer[..frame_end].to_vec();
            buffer.drain(..frame_end + separator_len);
            let frame = String::from_utf8(frame_bytes).map_err(|error| {
                AgentError::new(format!("模型流不是有效 UTF-8：{error}"))
                    .with_usage(accumulator.usage().cloned())
            })?;
            process_sse_frame(&frame, &mut accumulator, &mut on_delta)
                .map_err(|error| error.with_usage(accumulator.usage().cloned()))?;
        }
    }

    cancellation_token.check()?;
    if !buffer.iter().all(u8::is_ascii_whitespace) {
        let frame = String::from_utf8(buffer).map_err(|error| {
            AgentError::new(format!("模型流尾部不是有效 UTF-8：{error}"))
                .with_usage(accumulator.usage().cloned())
        })?;
        process_sse_frame(&frame, &mut accumulator, &mut on_delta)
            .map_err(|error| error.with_usage(accumulator.usage().cloned()))?;
    }

    accumulator.finish()
}

pub(super) fn process_sse_frame<F>(
    frame: &str,
    accumulator: &mut LlmStreamAccumulator,
    on_delta: &mut F,
) -> AgentResult<()>
where
    F: FnMut(LlmStreamEvent),
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
        F: FnMut(LlmStreamEvent),
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

    fn usage(&self) -> Option<&AgentUsage> {
        match self {
            Self::OpenAi(accumulator) => accumulator.usage.as_ref(),
            Self::Anthropic(accumulator) => accumulator.usage.as_ref(),
        }
    }
}

#[derive(Default)]
pub(super) struct OpenAiStreamAccumulator {
    content: String,
    tool_calls: Vec<OpenAiToolCallAccumulator>,
    tool_call_slots_by_id: BTreeMap<String, usize>,
    active_tool_call_slots_by_index: BTreeMap<usize, usize>,
    usage: Option<AgentUsage>,
    finish_reason: Option<String>,
}

#[derive(Default)]
struct OpenAiToolCallAccumulator {
    id: Option<String>,
    name: String,
    arguments: OpenAiToolArgumentsAccumulator,
}

#[derive(Default)]
struct OpenAiToolArgumentsAccumulator {
    incremental: String,
    cumulative: Option<String>,
    preview: String,
    preview_mode: OpenAiArgumentStreamMode,
}

#[derive(Default)]
enum OpenAiArgumentStreamMode {
    #[default]
    Unknown,
    Incremental,
    Cumulative,
}

impl OpenAiToolArgumentsAccumulator {
    fn push(&mut self, fragment: &str) -> String {
        if fragment.is_empty() {
            return String::new();
        }

        self.incremental.push_str(fragment);
        match &mut self.cumulative {
            None if self.incremental == fragment => {
                self.cumulative = Some(fragment.to_string());
            }
            Some(current) if fragment.starts_with(current.as_str()) => {
                current.clear();
                current.push_str(fragment);
            }
            Some(current) if current.starts_with(fragment) => {}
            Some(_) | None => {
                self.cumulative = None;
            }
        }

        if self.preview.is_empty() {
            self.preview.push_str(fragment);
            return fragment.to_string();
        }

        match self.preview_mode {
            OpenAiArgumentStreamMode::Unknown
                if fragment.len() > self.preview.len()
                    && fragment.starts_with(self.preview.as_str()) =>
            {
                let delta = fragment[self.preview.len()..].to_string();
                self.preview.clear();
                self.preview.push_str(fragment);
                self.preview_mode = OpenAiArgumentStreamMode::Cumulative;
                delta
            }
            OpenAiArgumentStreamMode::Unknown | OpenAiArgumentStreamMode::Incremental => {
                self.preview_mode = OpenAiArgumentStreamMode::Incremental;
                self.preview.push_str(fragment);
                fragment.to_string()
            }
            OpenAiArgumentStreamMode::Cumulative if fragment.starts_with(self.preview.as_str()) => {
                let delta = fragment[self.preview.len()..].to_string();
                self.preview.clear();
                self.preview.push_str(fragment);
                delta
            }
            OpenAiArgumentStreamMode::Cumulative if self.preview.starts_with(fragment) => {
                String::new()
            }
            OpenAiArgumentStreamMode::Cumulative => {
                self.preview_mode = OpenAiArgumentStreamMode::Incremental;
                self.preview.push_str(fragment);
                fragment.to_string()
            }
        }
    }

    fn parse(&self) -> serde_json::Result<Value> {
        match parse_tool_arguments(&self.incremental) {
            Ok(value) => Ok(value),
            Err(incremental_error) => {
                if let Some(cumulative) = self
                    .cumulative
                    .as_deref()
                    .filter(|cumulative| *cumulative != self.incremental)
                {
                    if let Ok(value) = parse_tool_arguments(cumulative) {
                        return Ok(value);
                    }
                }
                Err(incremental_error)
            }
        }
    }

    fn received_bytes(&self) -> u64 {
        self.preview.len() as u64
    }
}

impl OpenAiStreamAccumulator {
    fn create_tool_call_slot(&mut self, provider_index: usize, id: Option<&str>) -> usize {
        let slot = self.tool_calls.len();
        let id = id.map(ToString::to_string);
        self.tool_calls.push(OpenAiToolCallAccumulator {
            id: id.clone(),
            ..OpenAiToolCallAccumulator::default()
        });
        if let Some(id) = id {
            self.tool_call_slots_by_id.insert(id, slot);
        }
        self.active_tool_call_slots_by_index
            .insert(provider_index, slot);
        slot
    }

    fn resolve_tool_call_slot(&mut self, provider_index: usize, id: Option<&str>) -> usize {
        if let Some(id) = id {
            if let Some(slot) = self.tool_call_slots_by_id.get(id).copied() {
                self.active_tool_call_slots_by_index
                    .insert(provider_index, slot);
                return slot;
            }

            if let Some(slot) = self
                .active_tool_call_slots_by_index
                .get(&provider_index)
                .copied()
            {
                if self.tool_calls[slot].id.is_none() {
                    self.tool_calls[slot].id = Some(id.to_string());
                    self.tool_call_slots_by_id.insert(id.to_string(), slot);
                    return slot;
                }
            }

            return self.create_tool_call_slot(provider_index, Some(id));
        }

        self.active_tool_call_slots_by_index
            .get(&provider_index)
            .copied()
            .unwrap_or_else(|| self.create_tool_call_slot(provider_index, None))
    }

    fn process<F>(&mut self, value: &Value, on_delta: &mut F) -> AgentResult<()>
    where
        F: FnMut(LlmStreamEvent),
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
                    on_delta(LlmStreamEvent::Delta(content.to_string()));
                }
            }

            let Some(tool_calls) = delta.get("tool_calls").and_then(Value::as_array) else {
                continue;
            };
            for (fallback_index, call) in tool_calls.iter().enumerate() {
                let explicit_index = call
                    .get("index")
                    .and_then(Value::as_u64)
                    .map(|index| index as usize);
                let provider_index = explicit_index.unwrap_or(fallback_index);
                let id = call
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|id| !id.is_empty());
                let slot = self.resolve_tool_call_slot(provider_index, id);
                let entry = &mut self.tool_calls[slot];
                if let Some(function) = call.get("function") {
                    if let Some(name) = function.get("name").and_then(Value::as_str) {
                        append_stream_fragment(&mut entry.name, name);
                    }
                    if let Some(arguments) = function.get("arguments").and_then(Value::as_str) {
                        let input_delta = entry.arguments.push(arguments);
                        if !input_delta.is_empty() {
                            on_delta(LlmStreamEvent::ToolInputProgress {
                                tool_call_index: slot,
                                tool_call_id: entry.id.clone(),
                                tool: entry.name.clone(),
                                input_delta,
                                received_bytes: entry.arguments.received_bytes(),
                            });
                        }
                    }
                }
            }
        }

        Ok(())
    }

    fn finish(self) -> AgentResult<LlmChatResponse> {
        let mut tool_calls = Vec::new();
        let error_usage = self.usage.clone();
        for (slot, call) in self.tool_calls.into_iter().enumerate() {
            if call.name.trim().is_empty() {
                continue;
            }
            let args = call.arguments.parse().map_err(|error| {
                AgentError::new(format!(
                    "OpenAI 流式 tool_call `{}` 的 arguments 不是有效 JSON：{error}",
                    call.name
                ))
                .with_usage(error_usage.clone())
            })?;
            tool_calls.push(LlmToolCall {
                id: call
                    .id
                    .unwrap_or_else(|| format!("openai-stream-tool-call-{}", slot + 1)),
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
        F: FnMut(LlmStreamEvent),
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
        F: FnMut(LlmStreamEvent),
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
                        on_delta(LlmStreamEvent::Delta(text.to_string()));
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
                        on_delta(LlmStreamEvent::ToolInputProgress {
                            tool_call_index: index,
                            tool_call_id: block.id.clone(),
                            tool: block.name.clone().unwrap_or_default(),
                            input_delta: block.input_json.clone(),
                            received_bytes: block.input_json.len() as u64,
                        });
                    }
                }
            }
            _ => {}
        }

        Ok(())
    }

    fn process_content_block_delta<F>(&mut self, value: &Value, on_delta: &mut F) -> AgentResult<()>
    where
        F: FnMut(LlmStreamEvent),
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
                    on_delta(LlmStreamEvent::Delta(text.to_string()));
                }
            }
            "input_json_delta" => {
                let partial_json = delta
                    .get("partial_json")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                block.kind = "tool_use".to_string();
                block.input_json.push_str(partial_json);
                if !partial_json.is_empty() {
                    on_delta(LlmStreamEvent::ToolInputProgress {
                        tool_call_index: index,
                        tool_call_id: block.id.clone(),
                        tool: block.name.clone().unwrap_or_default(),
                        input_delta: partial_json.to_string(),
                        received_bytes: block.input_json.len() as u64,
                    });
                }
            }
            _ => {}
        }

        Ok(())
    }

    fn finish(self) -> AgentResult<LlmChatResponse> {
        let mut tool_calls = Vec::new();
        let error_usage = self.usage.clone();
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
                .with_usage(error_usage.clone())
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
    if target.is_empty() {
        target.push_str(fragment);
    } else if fragment.starts_with(target.as_str()) && fragment != target {
        target.clear();
        target.push_str(fragment);
    } else if fragment != target {
        target.push_str(fragment);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn openai_tool_call_frame(arguments: &str) -> String {
        openai_tool_call_frame_for(Some(0), "call-1", "run_command", arguments)
    }

    fn openai_tool_call_frame_for(
        index: Option<usize>,
        id: &str,
        name: &str,
        arguments: &str,
    ) -> String {
        let mut tool_call = json!({
            "id": id,
            "function": {
                "name": name,
                "arguments": arguments
            }
        });
        if let Some(index) = index {
            tool_call["index"] = json!(index);
        }
        format!(
            "data: {}\n\n",
            json!({
                "choices": [
                    {
                        "delta": {
                            "tool_calls": [tool_call]
                        }
                    }
                ]
            })
        )
    }

    fn joined_tool_input(events: &[LlmStreamEvent]) -> String {
        events
            .iter()
            .filter_map(|event| match event {
                LlmStreamEvent::ToolInputProgress {
                    tool_call_index,
                    tool_call_id,
                    input_delta,
                    ..
                } => {
                    assert_eq!(*tool_call_index, 0);
                    assert_eq!(tool_call_id.as_deref(), Some("call-1"));
                    Some(input_delta.as_str())
                }
                _ => None,
            })
            .collect()
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
        assert_eq!(
            joined_tool_input(&deltas),
            "{\"command\":\"conda env list\",\"reason\":\"检查环境\"}"
        );
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
        assert_eq!(
            joined_tool_input(&deltas),
            "{\"command\":\"conda env list\",\"reason\":\"检查环境\"}"
        );
        assert_eq!(response.tool_calls.len(), 1);
        assert_eq!(response.tool_calls[0].name, "run_command");
        assert_eq!(response.tool_calls[0].args["command"], "conda env list");
        assert_eq!(response.tool_calls[0].args["reason"], "检查环境");
    }

    #[test]
    fn openai_stream_preserves_incremental_fragments_that_match_existing_prefixes() {
        let mut accumulator = LlmStreamAccumulator::new(AgentApiStyle::OpenAiCompatible);
        let mut deltas = Vec::new();

        for fragment in [
            "{\"items\": [",
            "{",
            "\"title\":\"A\",\"status\":\"pending\"}]}",
        ] {
            process_sse_frame(
                &openai_tool_call_frame(fragment),
                &mut accumulator,
                &mut |delta| deltas.push(delta),
            )
            .unwrap();
        }

        let response = accumulator.finish().unwrap();
        assert_eq!(
            joined_tool_input(&deltas),
            "{\"items\": [{\"title\":\"A\",\"status\":\"pending\"}]}"
        );
        assert_eq!(response.tool_calls[0].args["items"][0]["title"], "A");
    }

    #[test]
    fn openai_stream_preserves_repeated_incremental_fragments() {
        let mut accumulator = LlmStreamAccumulator::new(AgentApiStyle::OpenAiCompatible);
        let mut deltas = Vec::new();

        for fragment in ["{\"text\":\"a", "a", "\"}"] {
            process_sse_frame(
                &openai_tool_call_frame(fragment),
                &mut accumulator,
                &mut |delta| deltas.push(delta),
            )
            .unwrap();
        }

        let response = accumulator.finish().unwrap();
        assert_eq!(joined_tool_input(&deltas), "{\"text\":\"aa\"}");
        assert_eq!(response.tool_calls[0].args["text"], "aa");
    }

    #[test]
    fn openai_stream_separates_parallel_calls_by_id_when_index_is_missing() {
        let mut accumulator = LlmStreamAccumulator::new(AgentApiStyle::OpenAiCompatible);
        let mut deltas = Vec::new();

        process_sse_frame(
            &openai_tool_call_frame_for(None, "call-a", "write_file", "{\"draftId\":\"draft-a\"}"),
            &mut accumulator,
            &mut |delta| deltas.push(delta),
        )
        .unwrap();
        process_sse_frame(
            &openai_tool_call_frame_for(None, "call-b", "write_file", "{\"draftId\":\"draft-b\"}"),
            &mut accumulator,
            &mut |delta| deltas.push(delta),
        )
        .unwrap();

        let streamed_calls = deltas
            .iter()
            .filter_map(|event| match event {
                LlmStreamEvent::ToolInputProgress {
                    tool_call_index,
                    tool_call_id,
                    ..
                } => Some((*tool_call_index, tool_call_id.as_deref())),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            streamed_calls,
            vec![(0, Some("call-a")), (1, Some("call-b"))]
        );
        let response = accumulator.finish().unwrap();
        assert_eq!(response.tool_calls.len(), 2);
        assert_eq!(response.tool_calls[0].id, "call-a");
        assert_eq!(response.tool_calls[0].args["draftId"], "draft-a");
        assert_eq!(response.tool_calls[1].id, "call-b");
        assert_eq!(response.tool_calls[1].args["draftId"], "draft-b");
    }

    #[test]
    fn openai_stream_keeps_usage_when_tool_arguments_are_invalid() {
        let mut accumulator = LlmStreamAccumulator::new(AgentApiStyle::OpenAiCompatible);
        let mut deltas = Vec::new();

        process_sse_frame(
            &openai_tool_call_frame("{\"items\":["),
            &mut accumulator,
            &mut |delta| deltas.push(delta),
        )
        .unwrap();
        process_sse_frame(
            &format!(
                "data: {}\n\n",
                json!({
                    "choices": [],
                    "usage": {
                        "prompt_tokens": 12,
                        "completion_tokens": 5,
                        "total_tokens": 17
                    }
                })
            ),
            &mut accumulator,
            &mut |delta| deltas.push(delta),
        )
        .unwrap();

        let error = accumulator.finish().unwrap_err();
        let usage = error.usage().unwrap();
        assert_eq!(usage.input_tokens, Some(12));
        assert_eq!(usage.output_tokens, Some(5));
        assert_eq!(usage.total_tokens, Some(17));
        assert_eq!(usage.billable_request_count, Some(1));
    }
}
