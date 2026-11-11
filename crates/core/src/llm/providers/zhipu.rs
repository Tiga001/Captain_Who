//! Zhipu GLM-5.3 Chat Completions protocol. Only neutral HTTP/message helpers are shared.
//!
//! Preserved thinking replays the original assistant fields, including field presence and the
//! unmodified JSON argument strings. The versioned snapshot lives exclusively in the encrypted
//! continuation path; runtime-visible text and normalized tool arguments are separate projections.
use super::super::adapter::{stream_state_mismatch, ProviderAdapter};
use super::super::payload::{
    build_openai_headers, build_openai_tools, build_openai_user_content,
    render_backend_observed_state,
};
use super::super::provider_error::{
    LlmProviderFailure, LlmProviderFailureCategory, ProviderErrorClassification,
};
use super::super::stream::ProviderStreamState;
use super::super::{
    LlmAssistantTurn, LlmChatRequest, LlmChatResponse, LlmMessage, LlmMessagePlacement,
    LlmMessageRole, LlmStreamEvent, LlmToolCall, ProviderContinuation,
    ProviderContinuationAttachment, ProviderContinuationFragment, ProviderContinuationPosition,
    ProviderContinuationReplayScope, MAX_PROVIDER_CONTINUATION_BYTES,
};
use crate::protocol::{AgentError, AgentModelActivity, AgentResult, AgentUsage};
use crate::provider_profile::{
    ProviderFamilySettings, ProviderProfileConfig, ProviderProfileId, ProviderProtocolDialect,
    ProviderProtocolKey, ProviderReasoningEffort,
};
use crate::provider_registration::{
    ProviderRegistration, ZHIPU_GLM5_3_CHAT_REGISTRATION, ZHIPU_GLM5_3_FLASH_CHAT_REGISTRATION,
};
use crate::usage::merge_stream_usage;
use reqwest::header::HeaderMap;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};

const SNAPSHOT_V1: &[u8] = b"ZGLM\x01";
const MAX_TOOLS: usize = 128;

pub(in crate::llm) struct ZhipuAdapter {
    registration: &'static ProviderRegistration,
}
pub(in crate::llm) static ZHIPU_GLM5_3_CHAT_ADAPTER: ZhipuAdapter = ZhipuAdapter {
    registration: &ZHIPU_GLM5_3_CHAT_REGISTRATION,
};
pub(in crate::llm) static ZHIPU_GLM5_3_FLASH_CHAT_ADAPTER: ZhipuAdapter = ZhipuAdapter {
    registration: &ZHIPU_GLM5_3_FLASH_CHAT_REGISTRATION,
};

impl ProviderAdapter for ZhipuAdapter {
    fn registration(&self) -> &'static ProviderRegistration {
        self.registration
    }

    fn classify_provider_error(
        &self,
        status: Option<u16>,
        code: Option<&str>,
        body: &str,
    ) -> ProviderErrorClassification {
        ProviderErrorClassification::Authoritative(classify_error(status, code, body))
    }

    fn provider_error_code(&self, body: &str) -> Option<String> {
        numeric_error_code(body)
    }

    fn validate_profile_settings(&self, request: &LlmChatRequest) -> AgentResult<()> {
        request
            .provider_protocol
            .validate_against_config(&request.provider_profile)
            .map_err(|_| invalid("GLM profile 与冻结协议不一致。"))?;
        if request.provider_protocol.profile != self.profile()
            || request.provider_protocol.dialect != self.dialect()
        {
            return Err(invalid("GLM profile 与所选 Adapter 不一致。"));
        }
        let _ = reasoning_effort(&request.provider_profile)?;
        if request.provider_protocol.profile.id == ProviderProfileId::ZhipuGlm53Chat
            && request
                .messages
                .iter()
                .any(|message| !message.images().is_empty())
        {
            return Err(invalid(
                "GLM-5.3 仅支持文本，请选择 GLM-5.3-Flash 系列处理图片。",
            ));
        }
        if request.tools.len() > MAX_TOOLS {
            return Err(invalid("GLM 最多支持 128 个函数工具。"));
        }
        if request
            .max_tokens
            .is_some_and(|tokens| tokens == 0 || tokens > 131_072)
        {
            return Err(invalid("GLM max_tokens 必须介于 1 和 131072 之间。"));
        }
        Ok(())
    }

    fn validate_wire_protocol(&self, request: &LlmChatRequest) -> AgentResult<()> {
        build_messages(request).map(|_| ())
    }

    fn prepare_request(&self, request: &LlmChatRequest) -> AgentResult<Value> {
        self.validate_profile_settings(request)?;
        let mut payload = json!({
            "model": request.model(), "messages": build_messages(request)?,
            "stream": request.stream, "thinking": { "type": "enabled", "clear_thinking": false }
        });
        if let Some(max_tokens) = request.max_tokens {
            payload["max_tokens"] = json!(max_tokens);
        }
        if let Some(effort) = reasoning_effort(&request.provider_profile)? {
            payload["reasoning_effort"] = json!(effort);
        }
        if request.stream {
            payload["tool_stream"] = json!(true);
        }
        if !request.tools.is_empty() {
            payload["tools"] = Value::Array(build_openai_tools(&request.tools));
            payload["tool_choice"] = json!("auto");
        }
        // GLM family settings intentionally expose only reasoning effort. Leave sampling at the
        // documented provider defaults (temperature=1, top_p=.95), not legacy generic UI values.
        Ok(payload)
    }

    fn build_headers(&self, api_token: &str) -> AgentResult<HeaderMap> {
        build_openai_headers(api_token)
    }

    fn parse_non_streaming_response(
        &self,
        _profile: &ProviderProfileConfig,
        protocol: &ProviderProtocolKey,
        value: &Value,
    ) -> AgentResult<LlmAssistantTurn> {
        let choice = single_choice(value)?.ok_or_else(|| invalid("GLM 响应缺少 choice。"))?;
        check_finish(
            protocol,
            choice.get("finish_reason").and_then(Value::as_str),
        )?;
        let message = choice
            .get("message")
            .and_then(Value::as_object)
            .ok_or_else(|| invalid("GLM 响应缺少 assistant message。"))?;
        let snapshot = capture_message(message)?;
        let turn = turn_from_snapshot(protocol, snapshot)?;
        check_tool_finish(
            choice.get("finish_reason").and_then(Value::as_str),
            !turn.provider_tool_calls().is_empty(),
        )?;
        Ok(turn)
    }

    fn new_stream_state(
        &self,
        profile: &ProviderProfileConfig,
    ) -> AgentResult<ProviderStreamState> {
        let _ = reasoning_effort(profile)?;
        Ok(ProviderStreamState::Zhipu(Box::default()))
    }

    fn consume_streaming_event(
        &self,
        state: &mut ProviderStreamState,
        _event: Option<&str>,
        value: &Value,
        on_delta: &mut dyn FnMut(LlmStreamEvent),
    ) -> AgentResult<()> {
        let ProviderStreamState::Zhipu(state) = state else {
            return Err(stream_state_mismatch());
        };
        state.process(value, on_delta)
    }

    fn finalize_assistant_turn(
        &self,
        _profile: &ProviderProfileConfig,
        protocol: &ProviderProtocolKey,
        state: ProviderStreamState,
    ) -> AgentResult<LlmChatResponse> {
        let ProviderStreamState::Zhipu(state) = state else {
            return Err(stream_state_mismatch());
        };
        state.finish(protocol)
    }

    fn has_private_model_action(
        &self,
        protocol: &ProviderProtocolKey,
        turn: &LlmAssistantTurn,
    ) -> AgentResult<bool> {
        Ok(replay_snapshot(protocol, turn)?
            .and_then(|snapshot| {
                snapshot
                    .get("reasoning_content")
                    .and_then(Value::as_str)
                    .map(|s| !s.is_empty())
            })
            .unwrap_or(false))
    }

    fn project_usage(&self, _profile: &ProviderProfileConfig, value: &Value) -> Option<AgentUsage> {
        project_usage(value)
    }

    fn estimate_continuation_tokens(
        &self,
        continuation: Option<&ProviderContinuation>,
    ) -> AgentResult<u64> {
        let Some(continuation) = continuation else {
            return Ok(0);
        };
        let snapshot = decode_snapshot(continuation)?;
        let budget = crate::context::ContextTextBudget::heuristic(u64::MAX);
        let mut tokens = snapshot.get("reasoning_content").map_or(0, |reasoning| {
            budget.estimate(&json!({ "reasoning_content": reasoning }).to_string())
        });
        // The public projection counts parsed, compact JSON. Original tool arguments can contain
        // arbitrarily more whitespace/escapes, so reserve the positive wire difference as well.
        for call in snapshot
            .get("tool_calls")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let raw = call
                .pointer("/function/arguments")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let parsed: Value =
                serde_json::from_str(raw).map_err(|_| invalid("GLM continuation 参数无效。"))?;
            tokens = tokens.saturating_add(
                budget
                    .estimate(&json!(raw).to_string())
                    .saturating_sub(budget.estimate(&parsed.to_string())),
            );
        }
        Ok(tokens)
    }
}

fn reasoning_effort(profile: &ProviderProfileConfig) -> AgentResult<Option<&'static str>> {
    let effort = match profile.family_settings() {
        Some(ProviderFamilySettings::ZhipuGlm53Chat { reasoning_effort })
        | Some(ProviderFamilySettings::ZhipuGlm53FlashChat { reasoning_effort }) => {
            reasoning_effort
        }
        _ => return Err(invalid("GLM profile settings kind 不匹配。")),
    };
    Ok(match effort {
        ProviderReasoningEffort::ProviderDefault => None,
        ProviderReasoningEffort::Low => Some("low"),
        ProviderReasoningEffort::High => Some("high"),
        ProviderReasoningEffort::Max => Some("max"),
    })
}

fn invalid(message: &'static str) -> AgentError {
    AgentError::new(message)
}
fn boundary(reason: &'static str, message_index: usize) -> AgentError {
    AgentError::structured(
        "provider_context_boundary_required",
        "GLM Preserved Thinking 历史缺少可安全回放的 Provider 上下文。",
        json!({
            "type": "provider_context_boundary", "reason": reason, "messageIndex": message_index,
            "recovery": "startNewProviderContext"
        }),
    )
}

fn build_messages(request: &LlmChatRequest) -> AgentResult<Vec<Value>> {
    let mut messages = Vec::new();
    let mut seen_runtime_ids = BTreeSet::new();
    let mut index = 0;
    while index < request.messages.len() {
        let message = &request.messages[index];
        let Some(turn) = message.assistant_turn() else {
            if message.tool_result_fields().is_some() {
                return Err(boundary("unpairedToolResult", index));
            }
            messages.push(ordinary_message(message));
            index += 1;
            continue;
        };
        let snapshot = replay_snapshot(&request.provider_protocol, turn)
            .map_err(|_| boundary("incompatibleContinuation", index))?;
        if turn.provider_tool_calls().is_empty() {
            messages.push(snapshot.unwrap_or_else(
                || json!({ "role": "assistant", "content": turn.provider_visible_text() }),
            ));
            index += 1;
            continue;
        }
        let snapshot = snapshot.ok_or_else(|| boundary("missingToolBearingContinuation", index))?;
        let bindings = turn
            .runtime_tool_bindings()
            .filter(|bindings| bindings.len() == turn.provider_tool_calls().len())
            .ok_or_else(|| boundary("incompleteGroupedToolTurn", index))?;
        let mut results = Vec::new();
        let mut deferred = Vec::new();
        let mut next = index + 1;
        for (provider_index, binding) in bindings.iter().enumerate() {
            let call = &turn.provider_tool_calls()[provider_index];
            if binding.provider_tool_index != provider_index || binding.provider_call_id != call.id
            {
                return Err(boundary("providerToolIdentityMismatch", index));
            }
            super::super::validate_model_tool_call_id(&binding.runtime_call.id)?;
            if !seen_runtime_ids.insert(binding.runtime_call.id.as_str()) {
                return Err(boundary("duplicateRuntimeToolCallId", index));
            }
            loop {
                let candidate = request
                    .messages
                    .get(next)
                    .ok_or_else(|| boundary("missingToolResult", index))?;
                next += 1;
                if let Some((runtime_id, _, _)) = candidate.tool_result_fields() {
                    if runtime_id != binding.runtime_call.id {
                        return Err(boundary("toolResultOrderMismatch", next - 1));
                    }
                    results.push(json!({ "role": "tool", "tool_call_id": call.id, "content": candidate.content() }));
                    break;
                }
                if let Some(interstitial) = candidate.assistant_turn() {
                    if !interstitial.provider_tool_calls().is_empty() {
                        return Err(boundary("nestedToolBearingTurn", next - 1));
                    }
                    deferred.push(replay_snapshot(&request.provider_protocol, interstitial)
                        .map_err(|_| boundary("incompatibleContinuation", next - 1))?
                        .unwrap_or_else(|| json!({ "role": "assistant", "content": interstitial.provider_visible_text() })));
                } else {
                    deferred.push(ordinary_message(candidate));
                }
            }
        }
        messages.push(snapshot);
        messages.extend(results);
        messages.extend(deferred);
        index = next;
    }
    Ok(messages)
}

fn ordinary_message(message: &LlmMessage) -> Value {
    match (message.role(), message.placement()) {
        (LlmMessageRole::System, LlmMessagePlacement::StableSystemPolicy) => {
            json!({"role":"system", "content":message.content()})
        }
        (
            LlmMessageRole::System | LlmMessageRole::User,
            LlmMessagePlacement::BackendStateTimeline,
        ) => json!({"role":"user", "content":render_backend_observed_state(message.content())}),
        (LlmMessageRole::System, _) => json!({"role":"user", "content":message.content()}),
        (LlmMessageRole::User, _) => {
            json!({"role":"user", "content":build_openai_user_content(message)})
        }
        _ => unreachable!("assistant and tool messages use the grouped turn projection"),
    }
}

fn single_choice(value: &Value) -> AgentResult<Option<&Value>> {
    let Some(choices) = value.get("choices").and_then(Value::as_array) else {
        return Ok(None);
    };
    if choices.len() > 1
        || choices
            .first()
            .and_then(|c| c.get("index"))
            .and_then(Value::as_u64)
            .is_some_and(|index| index != 0)
    {
        return Err(invalid("GLM 响应包含不受支持的多个 choice。"));
    }
    Ok(choices.first())
}

fn capture_message(message: &Map<String, Value>) -> AgentResult<Value> {
    if message.get("role").is_some_and(|role| role != "assistant") {
        return Err(invalid("GLM message role 不是 assistant。"));
    }
    let mut snapshot = Map::from_iter([("role".into(), json!("assistant"))]);
    for key in ["content", "reasoning_content"] {
        if let Some(value) = message.get(key) {
            if !value.is_null() && !value.is_string() {
                return Err(invalid("GLM assistant 文本字段类型无效。"));
            }
            snapshot.insert(key.into(), value.clone());
        }
    }
    if let Some(calls) = message.get("tool_calls") {
        if !calls.is_array() && !calls.is_null() {
            return Err(invalid("GLM tool_calls 类型无效。"));
        }
        snapshot.insert("tool_calls".into(), calls.clone());
    }
    Ok(Value::Object(snapshot))
}

fn parse_snapshot(snapshot: &Value) -> AgentResult<(String, Vec<LlmToolCall>)> {
    let message = snapshot
        .as_object()
        .ok_or_else(|| invalid("GLM continuation snapshot 无效。"))?;
    if capture_message(message)? != *snapshot {
        return Err(invalid("GLM continuation snapshot 含未知字段。"));
    }
    let content = snapshot
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let mut calls = Vec::new();
    let mut ids = BTreeSet::new();
    for call in snapshot
        .get("tool_calls")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if call.get("type").and_then(Value::as_str) != Some("function") {
            return Err(invalid("GLM tool call 类型不是 function。"));
        }
        let id = call
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("GLM tool call 缺少 id。"))?;
        super::super::validate_provider_tool_call_id(id)?;
        if !ids.insert(id) {
            return Err(invalid("GLM tool call id 重复。"));
        }
        let name = call
            .pointer("/function/name")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| invalid("GLM tool call 缺少函数名。"))?;
        let arguments = call
            .pointer("/function/arguments")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("GLM tool call 缺少完整参数。"))?;
        let args: Value = serde_json::from_str(arguments)
            .map_err(|_| invalid("GLM tool call 参数不是完整 JSON。"))?;
        if !args.is_object() {
            return Err(invalid("GLM tool call 参数必须为 JSON 对象。"));
        }
        calls.push(LlmToolCall {
            id: id.to_owned(),
            name: name.to_owned(),
            args,
        });
    }
    if !calls.is_empty()
        && !snapshot
            .get("reasoning_content")
            .is_some_and(Value::is_string)
    {
        return Err(AgentError::structured(
            "provider_reasoning_required",
            "GLM 工具响应缺少可回传的 reasoning_content。",
            json!({"type":"providerReasoningBoundary", "reason":"missingPreservedReasoning", "recovery":"retryProviderRequest"}),
        ));
    }
    Ok((content, calls))
}

fn replay_scope(calls: &[LlmToolCall]) -> ProviderContinuationReplayScope {
    if calls.is_empty() {
        ProviderContinuationReplayScope::AssistantTurnV1
    } else {
        ProviderContinuationReplayScope::InteractionV1
    }
}
fn snapshot_position() -> ProviderContinuationPosition {
    ProviderContinuationPosition::new(0, ProviderContinuationAttachment::AssistantTurn)
}

fn turn_from_snapshot(
    protocol: &ProviderProtocolKey,
    snapshot: Value,
) -> AgentResult<LlmAssistantTurn> {
    let (content, calls) = parse_snapshot(&snapshot)?;
    let scope = replay_scope(&calls);
    let turn = LlmAssistantTurn::from_provider(protocol.clone(), content, calls)?;
    let mut opaque = SNAPSHOT_V1.to_vec();
    serde_json::to_writer(&mut opaque, &snapshot)
        .map_err(|_| invalid("GLM continuation 编码失败。"))?;
    if opaque.len() > MAX_PROVIDER_CONTINUATION_BYTES {
        return Err(invalid("GLM continuation 超过存储上限。"));
    }
    let continuation = ProviderContinuation::new(
        protocol.clone(),
        scope,
        turn.digest(),
        vec![ProviderContinuationFragment::new(
            snapshot_position(),
            opaque,
        )],
    )?;
    turn.with_provider_continuation(continuation)
}

fn decode_snapshot(continuation: &ProviderContinuation) -> AgentResult<Value> {
    let key = continuation.provenance();
    if key.dialect != ProviderProtocolDialect::OpenAiChatCompletions
        || !matches!(
            key.profile.id,
            ProviderProfileId::ZhipuGlm53Chat | ProviderProfileId::ZhipuGlm53FlashChat
        )
    {
        return Err(invalid("Continuation 不是 GLM Chat 协议。"));
    }
    let [fragment] = continuation.fragments() else {
        return Err(invalid("GLM continuation 必须包含一个 snapshot。"));
    };
    if fragment.position() != snapshot_position() {
        return Err(invalid("GLM continuation snapshot 位置无效。"));
    }
    let bytes = fragment
        .opaque()
        .strip_prefix(SNAPSHOT_V1)
        .ok_or_else(|| invalid("GLM continuation 版本无效。"))?;
    let snapshot: Value = serde_json::from_slice(bytes)
        .map_err(|_| invalid("GLM continuation snapshot 编码无效。"))?;
    let (_, calls) = parse_snapshot(&snapshot)?;
    if continuation.replay_scope() != replay_scope(&calls) {
        return Err(invalid("GLM continuation replay scope 无效。"));
    }
    Ok(snapshot)
}

fn replay_snapshot(
    protocol: &ProviderProtocolKey,
    turn: &LlmAssistantTurn,
) -> AgentResult<Option<Value>> {
    let Some(continuation) = turn.provider_continuation() else {
        if turn.provider_protocol().is_some() || !turn.provider_tool_calls().is_empty() {
            return Err(boundary("missingPreservedContinuation", 0));
        }
        return Ok(None);
    };
    if turn.provider_protocol() != Some(protocol) {
        return Err(invalid("GLM continuation 来源不匹配。"));
    }
    continuation.validate_for(protocol, turn.digest())?;
    let snapshot = decode_snapshot(continuation)?;
    let (content, calls) = parse_snapshot(&snapshot)?;
    if content != turn.provider_visible_text() || calls != turn.provider_tool_calls() {
        return Err(invalid("GLM continuation 与 assistant turn 不匹配。"));
    }
    Ok(Some(snapshot))
}

fn check_tool_finish(reason: Option<&str>, has_tools: bool) -> AgentResult<()> {
    if has_tools != (reason == Some("tool_calls")) {
        return Err(invalid("GLM 完成原因与工具调用不一致。"));
    }
    Ok(())
}

fn check_finish(protocol: &ProviderProtocolKey, reason: Option<&str>) -> AgentResult<()> {
    match reason {
        Some("stop" | "tool_calls") => Ok(()),
        Some("length") => Err(AgentError::structured(
            "agent.output_limit_reached",
            "输出达到上限，回复未完成",
            json!({"finishReason":"length"}),
        )),
        Some("network_error" | "sensitive" | "model_context_window_exceeded") => {
            let body = json!({"error":{"code":reason, "message":"GLM inference did not complete"}})
                .to_string();
            Err(
                LlmProviderFailure::from_embedded_error_for_protocol(protocol, &body)
                    .to_agent_error(),
            )
        }
        None | Some("") => Err(AgentError::structured(
            "agent.stream_interrupted",
            "GLM 回复未收到完成原因。",
            json!({"type":"missing_provider_finish_reason"}),
        )),
        Some(_) => Err(AgentError::structured(
            "agent.invalid_provider_finish_reason",
            "GLM 返回了未识别的终止原因。",
            json!({"type":"unknown_provider_finish_reason"}),
        )),
    }
}

/// Only documented GLM counters are accepted. Reasoning and cache reads are subsets, never
/// additional totals; cache misses do not imply cache writes. Missing counters remain unknown.
fn project_usage(value: &Value) -> Option<AgentUsage> {
    let mut usage = raw_usage(value)?;
    validate_usage_subsets(&mut usage);
    Some(usage)
}

fn raw_usage(value: &Value) -> Option<AgentUsage> {
    let usage = value.get("usage")?;
    let input_tokens = usage.get("prompt_tokens").and_then(Value::as_u64);
    let output_tokens = usage.get("completion_tokens").and_then(Value::as_u64);
    let cached_input_tokens = usage
        .pointer("/prompt_tokens_details/cached_tokens")
        .and_then(Value::as_u64);
    let output_thinking_tokens = usage
        .pointer("/completion_tokens_details/reasoning_tokens")
        .and_then(Value::as_u64);
    let total_tokens = usage
        .get("total_tokens")
        .and_then(Value::as_u64)
        .or_else(|| {
            input_tokens
                .zip(output_tokens)
                .and_then(|(input, output)| input.checked_add(output))
        });
    if [
        input_tokens,
        output_tokens,
        cached_input_tokens,
        output_thinking_tokens,
        total_tokens,
    ]
    .iter()
    .all(Option::is_none)
    {
        return None;
    }
    Some(AgentUsage {
        input_tokens,
        output_tokens,
        output_thinking_tokens,
        total_tokens,
        cached_input_tokens,
        cache_creation_input_tokens: None,
        billable_request_count: None,
    })
}

fn validate_usage_subsets(usage: &mut AgentUsage) {
    if usage
        .cached_input_tokens
        .zip(usage.input_tokens)
        .is_some_and(|(cached, input)| cached > input)
    {
        usage.cached_input_tokens = None;
    }
    if usage
        .output_thinking_tokens
        .zip(usage.output_tokens)
        .is_some_and(|(reasoning, output)| reasoning > output)
    {
        usage.output_thinking_tokens = None;
    }
}

#[derive(Default)]
pub(in crate::llm) struct ZhipuStreamAccumulator {
    snapshot: Map<String, Value>,
    calls: BTreeMap<usize, Value>,
    finish_reason: Option<String>,
    captured_bytes: usize,
    pub(in crate::llm) projected_usage: Option<AgentUsage>,
}

impl ZhipuStreamAccumulator {
    fn process(
        &mut self,
        value: &Value,
        on_delta: &mut dyn FnMut(LlmStreamEvent),
    ) -> AgentResult<()> {
        merge_stream_usage(&mut self.projected_usage, raw_usage(value));
        if let Some(usage) = &mut self.projected_usage {
            validate_usage_subsets(usage);
        }
        let Some(choice) = single_choice(value)? else {
            return Ok(());
        };
        let was_finished = self.finish_reason.is_some();
        if was_finished
            && choice
                .get("delta")
                .and_then(Value::as_object)
                .is_some_and(|delta| {
                    delta.iter().any(|(key, value)| {
                        matches!(key.as_str(), "content" | "reasoning_content" | "tool_calls")
                            && !value.is_null()
                            && value.as_str().is_none_or(|text| !text.is_empty())
                            && value.as_array().is_none_or(|items| !items.is_empty())
                    })
                })
        {
            return Err(invalid("GLM 已结束的流收到新的内容或工具参数。"));
        }
        if let Some(reason) = choice
            .get("finish_reason")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        {
            if self
                .finish_reason
                .as_deref()
                .is_some_and(|existing| existing != reason)
            {
                return Err(invalid("GLM 流包含冲突的完成原因。"));
            }
            self.finish_reason = Some(reason.to_owned());
            on_delta(LlmStreamEvent::ModelActivityChanged(
                AgentModelActivity::Waiting,
            ));
        }
        if was_finished {
            return Ok(());
        }
        let Some(delta) = choice.get("delta").and_then(Value::as_object) else {
            return Ok(());
        };
        for key in ["reasoning_content", "content"] {
            let Some(fragment) = delta.get(key) else {
                continue;
            };
            match fragment {
                Value::Null => {
                    self.snapshot.entry(key.to_owned()).or_insert(Value::Null);
                }
                Value::String(fragment) => {
                    let entry = self
                        .snapshot
                        .entry(key.to_owned())
                        .or_insert_with(|| json!(""));
                    if entry.is_null() {
                        *entry = json!("");
                    }
                    self.captured_bytes = self.captured_bytes.saturating_add(fragment.len());
                    if self.captured_bytes > MAX_PROVIDER_CONTINUATION_BYTES {
                        return Err(invalid("GLM 流文本超过 continuation 上限。"));
                    }
                    let Value::String(text) = entry else {
                        return Err(invalid("GLM 流文本字段无效。"));
                    };
                    text.push_str(fragment);
                    if !fragment.is_empty() {
                        if key == "content" {
                            on_delta(LlmStreamEvent::ModelActivityChanged(
                                AgentModelActivity::Waiting,
                            ));
                            on_delta(LlmStreamEvent::Delta(fragment.clone()));
                        } else if self.finish_reason.is_none() {
                            on_delta(LlmStreamEvent::ModelActivityChanged(
                                AgentModelActivity::Reasoning,
                            ));
                        }
                    }
                }
                _ => return Err(invalid("GLM 流文本字段不是字符串。")),
            }
        }
        if let Some(calls) = delta.get("tool_calls").filter(|calls| !calls.is_null()) {
            let calls = calls
                .as_array()
                .ok_or_else(|| invalid("GLM 流 tool_calls 不是数组。"))?;
            self.snapshot
                .entry("tool_calls")
                .or_insert_with(|| json!([]));
            for incoming in calls {
                let index = incoming
                    .get("index")
                    .and_then(Value::as_u64)
                    .and_then(|index| usize::try_from(index).ok())
                    .filter(|index| *index < MAX_TOOLS)
                    .ok_or_else(|| invalid("GLM 流 tool call index 无效。"))?;
                let call = self
                    .calls
                    .entry(index)
                    .or_insert_with(|| json!({"function":{}}));
                for key in ["id", "type"] {
                    merge_identity(call.as_object_mut().unwrap(), key, incoming.get(key))?;
                }
                if let Some(function) = incoming.get("function") {
                    let function = function
                        .as_object()
                        .ok_or_else(|| invalid("GLM 流 function 类型无效。"))?;
                    let target = call["function"].as_object_mut().unwrap();
                    if let Some(name) = function.get("name").filter(|name| !name.is_null()) {
                        let fragment = name
                            .as_str()
                            .ok_or_else(|| invalid("GLM 流函数名不是字符串。"))?;
                        let target_name = target.entry("name").or_insert_with(|| json!(""));
                        let Value::String(name) = target_name else {
                            return Err(invalid("GLM 流函数名类型无效。"));
                        };
                        super::super::stream::append_stream_fragment(name, fragment);
                    }
                    if let Some(arguments) =
                        function.get("arguments").filter(|args| !args.is_null())
                    {
                        let fragment = arguments
                            .as_str()
                            .ok_or_else(|| invalid("GLM 流工具参数不是字符串。"))?;
                        let entry = target.entry("arguments").or_insert_with(|| json!(""));
                        self.captured_bytes = self.captured_bytes.saturating_add(fragment.len());
                        if self.captured_bytes > MAX_PROVIDER_CONTINUATION_BYTES {
                            return Err(invalid("GLM 工具参数超过 continuation 上限。"));
                        }
                        let Value::String(combined) = entry else {
                            return Err(invalid("GLM 流工具参数类型无效。"));
                        };
                        combined.push_str(fragment);
                        let received_bytes = combined.len() as u64;
                        if !fragment.is_empty() {
                            on_delta(LlmStreamEvent::ToolInputProgress {
                                tool_call_index: index,
                                tool: target
                                    .get("name")
                                    .and_then(Value::as_str)
                                    .unwrap_or_default()
                                    .to_owned(),
                                input_delta: fragment.to_owned(),
                                received_bytes,
                            });
                        }
                    }
                }
                on_delta(LlmStreamEvent::ModelActivityChanged(
                    AgentModelActivity::Waiting,
                ));
            }
        }
        Ok(())
    }

    fn finish(mut self, protocol: &ProviderProtocolKey) -> AgentResult<LlmChatResponse> {
        let result = (|| {
            check_finish(protocol, self.finish_reason.as_deref())?;
            check_tool_finish(self.finish_reason.as_deref(), !self.calls.is_empty())?;
            if !self.calls.is_empty() {
                if self.calls.keys().copied().ne(0..self.calls.len()) {
                    return Err(invalid("GLM tool call index 序列不完整。"));
                }
                self.snapshot.insert(
                    "tool_calls".into(),
                    Value::Array(self.calls.into_values().collect()),
                );
            }
            self.snapshot.insert("role".into(), json!("assistant"));
            let assistant_turn = turn_from_snapshot(protocol, Value::Object(self.snapshot))?;
            Ok(LlmChatResponse {
                assistant_turn,
                usage: self.projected_usage.clone(),
                finish_reason: self.finish_reason,
            })
        })();
        result.map_err(|error: AgentError| error.with_usage(self.projected_usage))
    }
}

fn merge_identity(
    target: &mut Map<String, Value>,
    key: &str,
    incoming: Option<&Value>,
) -> AgentResult<()> {
    let Some(value) = incoming.filter(|value| !value.is_null()) else {
        return Ok(());
    };
    let text = value
        .as_str()
        .ok_or_else(|| invalid("GLM 工具身份字段不是字符串。"))?;
    if text.is_empty() {
        return Ok(());
    }
    if target.get(key).is_some_and(|existing| existing != value) {
        return Err(invalid("GLM 流工具身份发生变化。"));
    }
    target.insert(key.to_owned(), value.clone());
    Ok(())
}

pub(in crate::llm) fn classify_error(
    status: Option<u16>,
    provider_code: Option<&str>,
    body: &str,
) -> LlmProviderFailureCategory {
    // GLM's numeric error.code is authoritative even if an OpenAI-like error.type accompanies it.
    let numeric_code = numeric_error_code(body);
    let code = numeric_code.as_deref().or(provider_code);
    use LlmProviderFailureCategory as C;
    match code {
        Some("1000" | "1001" | "1003" | "1005") => C::Authentication,
        Some(
            "1113" | "1308" | "1309" | "1310" | "1314" | "1316" | "1317" | "1318" | "1319" | "1320"
            | "1321",
        ) => C::QuotaExhausted,
        Some("1220" | "1311" | "1313" | "1315") => C::PermissionDenied,
        Some("1211" | "1221" | "1222") => C::ResourceNotFound,
        Some("1261" | "model_context_window_exceeded") => C::ContextTooLarge,
        Some("1302") => C::RateLimited,
        Some("1200" | "1230" | "1305") => C::Overloaded,
        Some("1234" | "network_error") => C::Network,
        Some("1210" | "1212" | "1213" | "1214" | "1215" | "1301" | "sensitive") => {
            C::InvalidRequest
        }
        // Unknown vendor codes are not reinterpreted from an unrelated free-form message.
        Some(_) if numeric_code.is_some() => C::Unknown,
        _ => match status {
            Some(401) => C::Authentication,
            Some(403) => C::PermissionDenied,
            Some(404) => C::ResourceNotFound,
            Some(413) => C::ContextTooLarge,
            Some(400 | 405 | 415 | 422) => C::InvalidRequest,
            Some(429) => C::RateLimited,
            Some(408 | 499) => C::Network,
            Some(500 | 502 | 503 | 504 | 529) => C::Overloaded,
            _ => C::Unknown,
        },
    }
}

fn numeric_error_code(body: &str) -> Option<String> {
    let value: Value = serde_json::from_str(body).ok()?;
    let code = value.pointer("/error/code").or_else(|| value.get("code"))?;
    match code {
        Value::Number(number) => Some(number.to_string()),
        Value::String(code)
            if !code.is_empty()
                && code.len() <= 64
                && code.bytes().all(|byte| byte.is_ascii_digit()) =>
        {
            Some(code.clone())
        }
        _ => None,
    }
}
