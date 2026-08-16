// Server-sent event parsing and stream accumulation for LLM responses.
use super::adapter::{ProviderAdapter, ProviderAdapterRegistry};
use super::provider_error::{stream_inactivity_timeout_error, LlmProviderFailure};
use super::response::{extract_api_error, parse_tool_arguments};
use super::{
    LlmAssistantTurn, LlmChatResponse, LlmStreamEvent, LlmToolCall, MAX_PROVIDER_CONTINUATION_BYTES,
};
use crate::cancellation::AgentCancellationToken;
use crate::protocol::{AgentApiStyle, AgentError, AgentResult, AgentUsage};
#[cfg(test)]
use crate::provider_profile::ProviderProfileId;
use crate::provider_profile::{ProviderProfileConfig, ProviderProtocolKey};
use crate::usage::{extract_anthropic_stream_usage, extract_usage, merge_stream_usage};
use futures_util::StreamExt;
use serde_json::{json, Value};
use sha2::Digest;
use std::collections::BTreeMap;
use std::time::Duration;

pub(super) async fn parse_sse_response<F>(
    response: reqwest::Response,
    provider_profile: &ProviderProfileConfig,
    provider_protocol: &ProviderProtocolKey,
    cancellation_token: AgentCancellationToken,
    inactivity_timeout: Duration,
    mut inactivity_deadline: tokio::time::Instant,
    mut on_delta: F,
) -> AgentResult<LlmChatResponse>
where
    F: FnMut(LlmStreamEvent) + Send,
{
    let mut stream = response.bytes_stream();
    let mut buffer = Vec::<u8>::new();
    let mut accumulator = LlmStreamAccumulator::for_profile(provider_profile, provider_protocol)?;

    loop {
        cancellation_token.check()?;
        let chunk = tokio::select! {
            _ = cancellation_token.cancelled() => return Err(AgentError::cancelled()),
            _ = tokio::time::sleep_until(inactivity_deadline) => {
                return Err(
                    stream_inactivity_timeout_error(inactivity_timeout, "stream_body")
                        .with_usage(accumulator.usage().cloned())
                );
            },
            chunk = stream.next() => {
                let Some(chunk) = chunk else {
                    break;
                };
                chunk.map_err(|error| {
                    LlmProviderFailure::from_network_error(error)
                        .to_agent_error()
                        .with_usage(accumulator.usage().cloned())
                })?
            }
        };
        buffer.extend_from_slice(&chunk);

        let mut received_model_activity = false;
        while let Some((frame_end, separator_len)) = find_sse_frame_end(&buffer) {
            cancellation_token.check()?;
            let frame_bytes = buffer[..frame_end].to_vec();
            buffer.drain(..frame_end + separator_len);
            let frame = String::from_utf8(frame_bytes).map_err(|error| {
                LlmProviderFailure::from_local_transport_failure(&format!(
                    "model stream frame was not valid UTF-8: {error}"
                ))
                .to_agent_error()
                .with_usage(accumulator.usage().cloned())
            })?;
            received_model_activity |=
                process_sse_frame(&frame, &mut accumulator, &mut on_delta)
                    .map_err(|error| error.with_usage(accumulator.usage().cloned()))?;
        }
        if received_model_activity {
            inactivity_deadline = tokio::time::Instant::now() + inactivity_timeout;
        }
    }

    cancellation_token.check()?;
    if !buffer.iter().all(u8::is_ascii_whitespace) {
        let frame = String::from_utf8(buffer).map_err(|error| {
            LlmProviderFailure::from_local_transport_failure(&format!(
                "model stream tail was not valid UTF-8: {error}"
            ))
            .to_agent_error()
            .with_usage(accumulator.usage().cloned())
        })?;
        let _ = process_sse_frame(&frame, &mut accumulator, &mut on_delta)
            .map_err(|error| error.with_usage(accumulator.usage().cloned()))?;
    }

    accumulator.finish()
}

pub(super) fn process_sse_frame<F>(
    frame: &str,
    accumulator: &mut LlmStreamAccumulator,
    on_delta: &mut F,
) -> AgentResult<bool>
where
    F: FnMut(LlmStreamEvent),
{
    let frame = parse_sse_frame(frame);
    let data = frame.data.trim();
    if data.is_empty() || data == "[DONE]" {
        return Ok(false);
    }

    let value: Value = serde_json::from_str(data).map_err(|error| {
        LlmProviderFailure::from_local_transport_failure(&format!(
            "model stream event was not valid JSON: {error}; event_hash=sha256:{:x}",
            sha2::Sha256::digest(data.as_bytes())
        ))
        .to_agent_error()
    })?;
    if let Some(error) = extract_api_error(&value) {
        let _ = error;
        return Err(
            LlmProviderFailure::from_embedded_error(accumulator.api_style(), data).to_agent_error(),
        );
    }
    let is_model_activity = is_meaningful_model_activity(frame.event.as_deref(), &value);
    accumulator.process(frame.event.as_deref(), &value, on_delta)?;
    Ok(is_model_activity)
}

fn is_meaningful_model_activity(event: Option<&str>, value: &Value) -> bool {
    if event.is_some_and(|event| event.eq_ignore_ascii_case("ping"))
        || value
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|event| event.eq_ignore_ascii_case("ping"))
    {
        return false;
    }

    if value
        .get("choices")
        .and_then(Value::as_array)
        .is_some_and(|choices| {
            choices.iter().any(|choice| {
                let Some(delta) = choice.get("delta") else {
                    return false;
                };
                delta
                    .get("content")
                    .and_then(Value::as_str)
                    .is_some_and(|content| !content.is_empty())
                    || delta
                        .get("reasoning_content")
                        .and_then(Value::as_str)
                        .is_some_and(|reasoning| !reasoning.is_empty())
                    || delta
                        .get("tool_calls")
                        .and_then(Value::as_array)
                        .is_some_and(|calls| {
                            calls.iter().any(|call| {
                                call.get("id")
                                    .and_then(Value::as_str)
                                    .is_some_and(|id| !id.is_empty())
                                    || call.get("function").is_some_and(|function| {
                                        ["name", "arguments"].iter().any(|field| {
                                            function
                                                .get(field)
                                                .and_then(Value::as_str)
                                                .is_some_and(|value| !value.is_empty())
                                        })
                                    })
                            })
                        })
            })
        })
    {
        return true;
    }

    match event
        .filter(|event| !event.trim().is_empty())
        .or_else(|| value.get("type").and_then(Value::as_str))
        .unwrap_or_default()
    {
        "content_block_start" => value.get("content_block").is_some_and(|block| {
            match block.get("type").and_then(Value::as_str) {
                Some("text") => block
                    .get("text")
                    .and_then(Value::as_str)
                    .is_some_and(|text| !text.is_empty()),
                Some("tool_use") => {
                    block
                        .get("name")
                        .and_then(Value::as_str)
                        .is_some_and(|name| !name.is_empty())
                        || block.get("input").is_some_and(|input| {
                            !input.is_null()
                                && input.as_object().is_none_or(|input| !input.is_empty())
                        })
                }
                _ => false,
            }
        }),
        "content_block_delta" => value.get("delta").is_some_and(|delta| {
            ["text", "partial_json"].iter().any(|field| {
                delta
                    .get(field)
                    .and_then(Value::as_str)
                    .is_some_and(|value| !value.is_empty())
            })
        }),
        _ => false,
    }
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

pub(super) enum ProviderStreamState {
    OpenAi(OpenAiStreamAccumulator),
    Anthropic(AnthropicStreamAccumulator),
    DeepSeek(Box<DeepSeekStreamAccumulator>),
}

pub(super) struct LlmStreamAccumulator {
    adapter: &'static dyn ProviderAdapter,
    provider_profile: ProviderProfileConfig,
    provider_protocol: ProviderProtocolKey,
    state: ProviderStreamState,
}

impl LlmStreamAccumulator {
    #[cfg(test)]
    pub(super) fn for_protocol(provider_protocol: &ProviderProtocolKey) -> AgentResult<Self> {
        let provider_profile = match provider_protocol.profile.id {
            ProviderProfileId::GenericOpenAiChat | ProviderProfileId::GenericAnthropicMessages => {
                ProviderProfileConfig::generic_for_dialect(provider_protocol.dialect)
            }
            ProviderProfileId::DeepSeekV4Chat => ProviderProfileConfig::deepseek_v4_default(),
            _ => {
                return Err(AgentError::new(
                    "Provider profile 未注册，无法创建流式解析器。",
                ));
            }
        };
        Self::for_profile(&provider_profile, provider_protocol)
    }

    pub(super) fn for_profile(
        provider_profile: &ProviderProfileConfig,
        provider_protocol: &ProviderProtocolKey,
    ) -> AgentResult<Self> {
        provider_protocol
            .validate_against_config(provider_profile)
            .map_err(|error| AgentError::new(format!("Provider profile 设置无效：{error}")))?;
        let adapter = ProviderAdapterRegistry::resolve_key(provider_protocol)?;
        Ok(Self {
            adapter,
            provider_profile: provider_profile.clone(),
            provider_protocol: provider_protocol.clone(),
            state: adapter.new_stream_state(),
        })
    }

    #[cfg(test)]
    pub(super) fn new(api_style: AgentApiStyle) -> Self {
        let dialect = api_style.into();
        let config = crate::provider_profile::ProviderProfileConfig::generic_for_dialect(dialect);
        let protocol = ProviderProtocolKey::new(dialect, &config, "test-model", None)
            .expect("generic test provider protocol must be valid");
        Self::for_protocol(&protocol).expect("generic test adapter must be registered")
    }

    fn api_style(&self) -> AgentApiStyle {
        self.provider_protocol.dialect.api_style()
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
        self.adapter
            .consume_streaming_event(&mut self.state, event, value, on_delta)
    }

    pub(super) fn finish(self) -> AgentResult<LlmChatResponse> {
        self.adapter.finalize_assistant_turn(
            &self.provider_profile,
            &self.provider_protocol,
            self.state,
        )
    }

    fn usage(&self) -> Option<&AgentUsage> {
        match &self.state {
            ProviderStreamState::OpenAi(accumulator) => accumulator.usage.as_ref(),
            ProviderStreamState::Anthropic(accumulator) => accumulator.usage.as_ref(),
            ProviderStreamState::DeepSeek(accumulator) => accumulator.projected_usage.as_ref(),
        }
    }
}

/// DeepSeek's Chat Completion stream is OpenAI-compatible except that private reasoning is
/// delivered separately in `delta.reasoning_content`. Keep it out of visible delta events while
/// preserving it exactly for the provider continuation attached during finalization.
#[derive(Default)]
pub(super) struct DeepSeekStreamAccumulator {
    openai: OpenAiStreamAccumulator,
    reasoning_content: String,
    saw_reasoning_content: bool,
    raw_usage: Option<AgentUsage>,
    projected_usage: Option<AgentUsage>,
}

impl DeepSeekStreamAccumulator {
    pub(super) fn process(
        &mut self,
        value: &Value,
        on_delta: &mut dyn FnMut(LlmStreamEvent),
    ) -> AgentResult<()> {
        merge_stream_usage(&mut self.raw_usage, extract_usage(value));
        self.projected_usage = self
            .raw_usage
            .clone()
            .map(super::adapter::project_deepseek_usage);
        if let Some(choices) = value.get("choices").and_then(Value::as_array) {
            for choice in choices {
                let Some(reasoning) = choice
                    .get("delta")
                    .and_then(|delta| delta.get("reasoning_content"))
                else {
                    continue;
                };
                match reasoning {
                    Value::Null => {}
                    Value::String(reasoning) => {
                        self.saw_reasoning_content = true;
                        let next_length = self
                            .reasoning_content
                            .len()
                            .checked_add(reasoning.len())
                            .ok_or_else(|| {
                                AgentError::new("DeepSeek reasoning_content 大小溢出。")
                            })?;
                        if next_length >= MAX_PROVIDER_CONTINUATION_BYTES {
                            return Err(AgentError::new(format!(
                                "DeepSeek reasoning_content 连同协议版本标记超过 {} 字节上限。",
                                MAX_PROVIDER_CONTINUATION_BYTES,
                            )));
                        }
                        self.reasoning_content.push_str(reasoning);
                    }
                    _ => {
                        return Err(AgentError::new(
                            "DeepSeek 流式响应中的 reasoning_content 不是字符串。",
                        ));
                    }
                }
            }
        }
        self.openai.process(value, on_delta)
    }

    pub(super) fn finish(
        self,
        provider_protocol: &ProviderProtocolKey,
    ) -> AgentResult<(LlmChatResponse, Option<String>)> {
        let response = self.openai.finish(provider_protocol)?;
        let reasoning_content = self.saw_reasoning_content.then_some(self.reasoning_content);
        Ok((response, reasoning_content))
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

    pub(super) fn process(
        &mut self,
        value: &Value,
        on_delta: &mut dyn FnMut(LlmStreamEvent),
    ) -> AgentResult<()> {
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

    pub(super) fn finish(
        self,
        provider_protocol: &ProviderProtocolKey,
    ) -> AgentResult<LlmChatResponse> {
        let mut tool_calls = Vec::new();
        let error_usage = self.usage.clone();
        for call in self.tool_calls {
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
                id: call.id.unwrap_or_default(),
                name: call.name,
                args,
            });
        }

        let assistant_turn =
            LlmAssistantTurn::from_provider(provider_protocol.clone(), self.content, tool_calls)?;
        Ok(LlmChatResponse {
            assistant_turn,
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
    pub(super) fn process(
        &mut self,
        event: Option<&str>,
        value: &Value,
        on_delta: &mut dyn FnMut(LlmStreamEvent),
    ) -> AgentResult<()> {
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
                return Err(LlmProviderFailure::from_embedded_error(
                    AgentApiStyle::AnthropicCompatible,
                    &value.to_string(),
                )
                .to_agent_error());
            }
            "ping" | "content_block_stop" | "message_stop" => {}
            _ => {}
        }

        Ok(())
    }

    fn process_content_block_start(
        &mut self,
        value: &Value,
        on_delta: &mut dyn FnMut(LlmStreamEvent),
    ) -> AgentResult<()> {
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

    fn process_content_block_delta(
        &mut self,
        value: &Value,
        on_delta: &mut dyn FnMut(LlmStreamEvent),
    ) -> AgentResult<()> {
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

    pub(super) fn finish(
        self,
        provider_protocol: &ProviderProtocolKey,
    ) -> AgentResult<LlmChatResponse> {
        let mut tool_calls = Vec::new();
        let error_usage = self.usage.clone();
        for block in self.blocks.into_values() {
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
                id: block.id.unwrap_or_default(),
                name,
                args,
            });
        }

        let assistant_turn =
            LlmAssistantTurn::from_provider(provider_protocol.clone(), self.content, tool_calls)?;
        Ok(LlmChatResponse {
            assistant_turn,
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
                    input_delta,
                    ..
                } => {
                    assert_eq!(*tool_call_index, 0);
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
        assert_eq!(response.provider_tool_calls().len(), 1);
        assert_eq!(response.provider_tool_calls()[0].name, "run_command");
        assert_eq!(
            response.provider_tool_calls()[0].args["command"],
            "conda env list"
        );
        assert_eq!(response.provider_tool_calls()[0].args["reason"], "检查环境");
    }

    #[test]
    fn openai_stream_treats_empty_continuation_ids_as_absent() {
        let mut accumulator = LlmStreamAccumulator::new(AgentApiStyle::OpenAiCompatible);
        let mut deltas = Vec::new();

        let frames = [
            json!({
                "choices": [{
                    "delta": {
                        "tool_calls": [{
                            "index": 0,
                            "id": "call-web-search",
                            "function": { "name": "web_search" }
                        }]
                    }
                }]
            }),
            json!({
                "choices": [{
                    "delta": {
                        "tool_calls": [{
                            "index": 0,
                            "id": "",
                            "function": { "name": "", "arguments": "{\"query\":" }
                        }]
                    }
                }]
            }),
            json!({
                "choices": [{
                    "delta": {
                        "tool_calls": [{
                            "index": 0,
                            "id": "",
                            "function": { "name": "", "arguments": "\"浙江大学最新信息\"}" }
                        }]
                    }
                }]
            }),
        ];

        for frame in frames {
            process_sse_frame(
                &format!("data: {frame}\n\n"),
                &mut accumulator,
                &mut |delta| deltas.push(delta),
            )
            .unwrap();
        }

        let response = accumulator.finish().unwrap();
        assert_eq!(
            joined_tool_input(&deltas),
            "{\"query\":\"浙江大学最新信息\"}"
        );
        assert_eq!(response.provider_tool_calls().len(), 1);
        assert_eq!(response.provider_tool_calls()[0].id, "call-web-search");
        assert_eq!(response.provider_tool_calls()[0].name, "web_search");
        assert_eq!(
            response.provider_tool_calls()[0].args["query"],
            "浙江大学最新信息"
        );
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
        assert_eq!(response.provider_tool_calls().len(), 1);
        assert_eq!(response.provider_tool_calls()[0].name, "run_command");
        assert_eq!(
            response.provider_tool_calls()[0].args["command"],
            "conda env list"
        );
        assert_eq!(response.provider_tool_calls()[0].args["reason"], "检查环境");
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
        assert_eq!(
            response.provider_tool_calls()[0].args["items"][0]["title"],
            "A"
        );
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
        assert_eq!(response.provider_tool_calls()[0].args["text"], "aa");
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
                    tool_call_index, ..
                } => Some(*tool_call_index),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(streamed_calls, vec![0, 1]);
        let response = accumulator.finish().unwrap();
        assert_eq!(response.provider_tool_calls().len(), 2);
        assert_eq!(response.provider_tool_calls()[0].id, "call-a");
        assert_eq!(response.provider_tool_calls()[0].args["draftId"], "draft-a");
        assert_eq!(response.provider_tool_calls()[1].id, "call-b");
        assert_eq!(response.provider_tool_calls()[1].args["draftId"], "draft-b");
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

    #[test]
    fn meaningful_activity_includes_reasoning_and_tool_fragments_but_not_metadata() {
        assert!(is_meaningful_model_activity(
            None,
            &json!({"choices":[{"delta":{"reasoning_content":"thinking"}}]})
        ));
        assert!(is_meaningful_model_activity(
            None,
            &json!({
                "choices":[{
                    "delta":{
                        "tool_calls":[{
                            "index":0,
                            "function":{"name":"write_file","arguments":""}
                        }]
                    }
                }]
            })
        ));
        assert!(!is_meaningful_model_activity(
            None,
            &json!({"choices":[],"usage":{"prompt_tokens":12}})
        ));
        assert!(!is_meaningful_model_activity(
            Some("ping"),
            &json!({"type":"ping"})
        ));
    }
}
