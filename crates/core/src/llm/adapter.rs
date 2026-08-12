//! Code-owned provider protocol adapters.
//!
//! A frozen [`ProviderProtocolKey`] selects an exact adapter. The API-style enum is only a
//! transport compatibility projection and is deliberately not an adapter registry key.

use super::payload::{
    build_anthropic_headers, build_anthropic_payload, build_deepseek_payload, build_openai_headers,
    build_openai_payload,
};
use super::response::{
    extract_anthropic_tool_calls, extract_openai_tool_calls, extract_response_text,
};
use super::stream::{AnthropicStreamAccumulator, OpenAiStreamAccumulator, ProviderStreamState};
use super::{
    LlmAssistantTurn, LlmChatRequest, LlmChatResponse, LlmMessage, LlmStreamEvent, LlmToolCall,
    ProviderContinuation, ProviderContinuationAttachment, ProviderContinuationFragment,
    ProviderContinuationPosition, ProviderContinuationReplayScope, MAX_PROVIDER_CONTINUATION_BYTES,
};
use crate::protocol::{AgentError, AgentResult, AgentUsage};
use crate::provider_profile::{
    ProviderProfileConfig, ProviderProfileId, ProviderProfileRef, ProviderProtocolDialect,
    ProviderProtocolKey, ReasoningMode,
};
use crate::provider_registration::{
    resolve_provider_registration_for_key, ProviderAdapterKind, ProviderRegistration,
    ProviderRuntimeCapabilities, DEEPSEEK_V4_CHAT_REGISTRATION,
    GENERIC_ANTHROPIC_MESSAGES_REGISTRATION, GENERIC_OPENAI_CHAT_REGISTRATION,
};
use reqwest::header::HeaderMap;
use serde_json::{json, Value};
use std::collections::BTreeSet;

pub(super) trait ProviderAdapter: Sync {
    fn registration(&self) -> &'static ProviderRegistration;

    fn profile(&self) -> ProviderProfileRef {
        self.registration().profile()
    }

    fn dialect(&self) -> ProviderProtocolDialect {
        self.registration().dialect()
    }

    fn capabilities(&self) -> ProviderRuntimeCapabilities {
        self.registration().runtime_capabilities()
    }

    fn validate_profile_settings(&self, request: &LlmChatRequest) -> AgentResult<()>;

    fn validate_wire_protocol(&self, request: &LlmChatRequest) -> AgentResult<()>;

    fn prepare_request(&self, request: &LlmChatRequest) -> AgentResult<Value>;

    fn build_headers(&self, api_token: &str) -> AgentResult<HeaderMap>;

    fn parse_non_streaming_response(
        &self,
        profile: &ProviderProfileConfig,
        protocol: &ProviderProtocolKey,
        value: &Value,
    ) -> AgentResult<LlmAssistantTurn>;

    fn new_stream_state(&self) -> ProviderStreamState;

    fn consume_streaming_event(
        &self,
        state: &mut ProviderStreamState,
        event: Option<&str>,
        value: &Value,
        on_delta: &mut dyn FnMut(LlmStreamEvent),
    ) -> AgentResult<()>;

    fn finalize_assistant_turn(
        &self,
        profile: &ProviderProfileConfig,
        protocol: &ProviderProtocolKey,
        state: ProviderStreamState,
    ) -> AgentResult<LlmChatResponse>;

    fn project_usage(&self, value: &Value) -> Option<AgentUsage>;

    fn estimate_continuation_tokens(
        &self,
        continuation: Option<&ProviderContinuation>,
    ) -> AgentResult<u64>;
}

struct GenericOpenAiAdapter;
struct GenericAnthropicAdapter;
struct DeepSeekV4ChatAdapter;

const DEEPSEEK_REASONING_FRAGMENT_V1: u8 = 1;

static GENERIC_OPENAI_ADAPTER: GenericOpenAiAdapter = GenericOpenAiAdapter;
static GENERIC_ANTHROPIC_ADAPTER: GenericAnthropicAdapter = GenericAnthropicAdapter;
static DEEPSEEK_V4_CHAT_ADAPTER: DeepSeekV4ChatAdapter = DeepSeekV4ChatAdapter;

pub(super) struct ProviderAdapterRegistry;

impl ProviderAdapterRegistry {
    pub(super) fn resolve(request: &LlmChatRequest) -> AgentResult<&'static dyn ProviderAdapter> {
        request
            .provider_protocol
            .validate_against_config(&request.provider_profile)
            .map_err(|error| AgentError::new(format!("Provider protocol 配置无效：{error}")))?;
        let adapter = Self::resolve_key(&request.provider_protocol)?;
        adapter.validate_profile_settings(request)?;
        Ok(adapter)
    }

    pub(super) fn resolve_key(
        protocol: &ProviderProtocolKey,
    ) -> AgentResult<&'static dyn ProviderAdapter> {
        let registration = resolve_provider_registration_for_key(protocol)
            .map_err(|error| AgentError::new(format!("Provider protocol key 无效：{error}")))?;
        let adapter: &'static dyn ProviderAdapter = match registration.adapter_kind() {
            ProviderAdapterKind::GenericOpenAi => &GENERIC_OPENAI_ADAPTER,
            ProviderAdapterKind::GenericAnthropic => &GENERIC_ANTHROPIC_ADAPTER,
            ProviderAdapterKind::DeepSeekV4Chat => &DEEPSEEK_V4_CHAT_ADAPTER,
        };
        debug_assert_eq!(adapter.registration(), registration);
        Ok(adapter)
    }
}

impl ProviderAdapter for GenericOpenAiAdapter {
    fn registration(&self) -> &'static ProviderRegistration {
        &GENERIC_OPENAI_CHAT_REGISTRATION
    }

    fn validate_profile_settings(&self, request: &LlmChatRequest) -> AgentResult<()> {
        validate_generic_profile_settings(self, request)
    }

    fn validate_wire_protocol(&self, request: &LlmChatRequest) -> AgentResult<()> {
        validate_generic_split_wire_protocol(&request.messages)
    }

    fn prepare_request(&self, request: &LlmChatRequest) -> AgentResult<Value> {
        build_openai_payload(request)
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
        LlmAssistantTurn::from_provider(
            protocol.clone(),
            extract_response_text(value).unwrap_or_default(),
            extract_openai_tool_calls(value)?,
        )
    }

    fn new_stream_state(&self) -> ProviderStreamState {
        ProviderStreamState::OpenAi(OpenAiStreamAccumulator::default())
    }

    fn consume_streaming_event(
        &self,
        state: &mut ProviderStreamState,
        _event: Option<&str>,
        value: &Value,
        on_delta: &mut dyn FnMut(LlmStreamEvent),
    ) -> AgentResult<()> {
        let ProviderStreamState::OpenAi(state) = state else {
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
        let ProviderStreamState::OpenAi(state) = state else {
            return Err(stream_state_mismatch());
        };
        state.finish(protocol)
    }

    fn project_usage(&self, value: &Value) -> Option<AgentUsage> {
        crate::usage::extract_usage(value)
    }

    fn estimate_continuation_tokens(
        &self,
        continuation: Option<&ProviderContinuation>,
    ) -> AgentResult<u64> {
        estimate_generic_continuation_tokens(continuation)
    }
}

impl ProviderAdapter for GenericAnthropicAdapter {
    fn registration(&self) -> &'static ProviderRegistration {
        &GENERIC_ANTHROPIC_MESSAGES_REGISTRATION
    }

    fn validate_profile_settings(&self, request: &LlmChatRequest) -> AgentResult<()> {
        validate_generic_profile_settings(self, request)
    }

    fn validate_wire_protocol(&self, request: &LlmChatRequest) -> AgentResult<()> {
        validate_generic_split_wire_protocol(&request.messages)
    }

    fn prepare_request(&self, request: &LlmChatRequest) -> AgentResult<Value> {
        build_anthropic_payload(request)
    }

    fn build_headers(&self, api_token: &str) -> AgentResult<HeaderMap> {
        build_anthropic_headers(api_token)
    }

    fn parse_non_streaming_response(
        &self,
        _profile: &ProviderProfileConfig,
        protocol: &ProviderProtocolKey,
        value: &Value,
    ) -> AgentResult<LlmAssistantTurn> {
        LlmAssistantTurn::from_provider(
            protocol.clone(),
            extract_response_text(value).unwrap_or_default(),
            extract_anthropic_tool_calls(value)?,
        )
    }

    fn new_stream_state(&self) -> ProviderStreamState {
        ProviderStreamState::Anthropic(AnthropicStreamAccumulator::default())
    }

    fn consume_streaming_event(
        &self,
        state: &mut ProviderStreamState,
        event: Option<&str>,
        value: &Value,
        on_delta: &mut dyn FnMut(LlmStreamEvent),
    ) -> AgentResult<()> {
        let ProviderStreamState::Anthropic(state) = state else {
            return Err(stream_state_mismatch());
        };
        state.process(event, value, on_delta)
    }

    fn finalize_assistant_turn(
        &self,
        _profile: &ProviderProfileConfig,
        protocol: &ProviderProtocolKey,
        state: ProviderStreamState,
    ) -> AgentResult<LlmChatResponse> {
        let ProviderStreamState::Anthropic(state) = state else {
            return Err(stream_state_mismatch());
        };
        state.finish(protocol)
    }

    fn project_usage(&self, value: &Value) -> Option<AgentUsage> {
        crate::usage::extract_usage(value)
    }

    fn estimate_continuation_tokens(
        &self,
        continuation: Option<&ProviderContinuation>,
    ) -> AgentResult<u64> {
        estimate_generic_continuation_tokens(continuation)
    }
}

impl ProviderAdapter for DeepSeekV4ChatAdapter {
    fn registration(&self) -> &'static ProviderRegistration {
        &DEEPSEEK_V4_CHAT_REGISTRATION
    }

    fn validate_profile_settings(&self, request: &LlmChatRequest) -> AgentResult<()> {
        validate_deepseek_profile_settings(self, request)
    }

    fn validate_wire_protocol(&self, request: &LlmChatRequest) -> AgentResult<()> {
        let _ = project_deepseek_exchange(
            &request.provider_profile,
            &request.provider_protocol,
            &request.messages,
        )?;
        Ok(())
    }

    fn prepare_request(&self, request: &LlmChatRequest) -> AgentResult<Value> {
        build_deepseek_payload(request)
    }

    fn build_headers(&self, api_token: &str) -> AgentResult<HeaderMap> {
        build_openai_headers(api_token)
    }

    fn parse_non_streaming_response(
        &self,
        profile: &ProviderProfileConfig,
        protocol: &ProviderProtocolKey,
        value: &Value,
    ) -> AgentResult<LlmAssistantTurn> {
        let turn = LlmAssistantTurn::from_provider(
            protocol.clone(),
            extract_response_text(value).unwrap_or_default(),
            extract_openai_tool_calls(value)?,
        )?;
        attach_deepseek_reasoning(
            profile,
            protocol,
            turn,
            extract_deepseek_reasoning_content(value)?,
        )
    }

    fn new_stream_state(&self) -> ProviderStreamState {
        ProviderStreamState::DeepSeek(Box::default())
    }

    fn consume_streaming_event(
        &self,
        state: &mut ProviderStreamState,
        _event: Option<&str>,
        value: &Value,
        on_delta: &mut dyn FnMut(LlmStreamEvent),
    ) -> AgentResult<()> {
        let ProviderStreamState::DeepSeek(state) = state else {
            return Err(stream_state_mismatch());
        };
        state.process(value, on_delta)
    }

    fn finalize_assistant_turn(
        &self,
        profile: &ProviderProfileConfig,
        protocol: &ProviderProtocolKey,
        state: ProviderStreamState,
    ) -> AgentResult<LlmChatResponse> {
        let ProviderStreamState::DeepSeek(state) = state else {
            return Err(stream_state_mismatch());
        };
        let (mut response, reasoning_content) = (*state).finish(protocol)?;
        response.assistant_turn = attach_deepseek_reasoning(
            profile,
            protocol,
            response.assistant_turn,
            reasoning_content.as_deref(),
        )?;
        response.usage = response.usage.map(project_deepseek_usage);
        Ok(response)
    }

    fn project_usage(&self, value: &Value) -> Option<AgentUsage> {
        crate::usage::extract_usage(value).map(project_deepseek_usage)
    }

    fn estimate_continuation_tokens(
        &self,
        continuation: Option<&ProviderContinuation>,
    ) -> AgentResult<u64> {
        estimate_deepseek_continuation_tokens(continuation)
    }
}

/// DeepSeek reports `completion_tokens` as the complete generated output, including private
/// reasoning. Runtime accounting keeps visible and thinking output disjoint, while preserving the
/// provider-authoritative total. Missing or inconsistent detail fails closed instead of guessing.
pub(super) fn project_deepseek_usage(mut usage: AgentUsage) -> AgentUsage {
    usage.output_tokens = match (usage.output_tokens, usage.output_thinking_tokens) {
        (Some(completion_tokens), Some(reasoning_tokens)) => {
            completion_tokens.checked_sub(reasoning_tokens)
        }
        _ => None,
    };
    usage
}

fn validate_generic_profile_settings(
    adapter: &dyn ProviderAdapter,
    request: &LlmChatRequest,
) -> AgentResult<()> {
    request
        .provider_protocol
        .validate_against_config(&request.provider_profile)
        .map_err(|error| AgentError::new(format!("Provider profile 设置无效：{error}")))?;
    if request.provider_protocol.profile != adapter.profile()
        || request.provider_protocol.dialect != adapter.dialect()
    {
        return Err(AgentError::new(
            "Provider profile 设置与所选 Adapter 不一致。",
        ));
    }
    for message in &request.messages {
        if let Some(turn) = message.assistant_turn() {
            let _ = adapter.estimate_continuation_tokens(turn.provider_continuation())?;
        }
    }
    Ok(())
}

fn validate_deepseek_profile_settings(
    adapter: &dyn ProviderAdapter,
    request: &LlmChatRequest,
) -> AgentResult<()> {
    request
        .provider_protocol
        .validate_against_config(&request.provider_profile)
        .map_err(|error| AgentError::new(format!("DeepSeek profile 设置无效：{error}")))?;
    if request.provider_protocol.profile != adapter.profile()
        || request.provider_protocol.dialect != adapter.dialect()
    {
        return Err(AgentError::new(
            "DeepSeek profile 设置与所选 Adapter 不一致。",
        ));
    }
    Ok(())
}

fn extract_deepseek_reasoning_content(value: &Value) -> AgentResult<Option<&str>> {
    let reasoning = value
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("message"))
        .and_then(|message| message.get("reasoning_content"));
    match reasoning {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(reasoning)) => Ok(Some(reasoning)),
        Some(_) => Err(AgentError::new(
            "DeepSeek 响应中的 reasoning_content 不是字符串。",
        )),
    }
}

fn attach_deepseek_reasoning(
    profile: &ProviderProfileConfig,
    protocol: &ProviderProtocolKey,
    turn: LlmAssistantTurn,
    reasoning_content: Option<&str>,
) -> AgentResult<LlmAssistantTurn> {
    let reasoning_content = match reasoning_content {
        Some(reasoning_content) => reasoning_content,
        None if profile.reasoning.mode == ReasoningMode::Enabled
            && !turn.provider_tool_calls().is_empty() =>
        {
            return Err(AgentError::structured(
                "provider_reasoning_required",
                "DeepSeek thinking 响应包含工具调用，但缺少 reasoning_content。",
                json!({
                    "type": "providerReasoningBoundary",
                    "reason": "missingEnabledToolReasoning",
                    "recovery": "retryProviderRequest",
                }),
            ));
        }
        None => return Ok(turn),
    };
    if reasoning_content.len() >= MAX_PROVIDER_CONTINUATION_BYTES {
        return Err(AgentError::new(format!(
            "DeepSeek reasoning_content 连同协议版本标记超过 {} 字节上限。",
            MAX_PROVIDER_CONTINUATION_BYTES,
        )));
    }
    let position =
        ProviderContinuationPosition::new(0, ProviderContinuationAttachment::AssistantTurn);
    let replay_scope = if turn.provider_tool_calls().is_empty() {
        ProviderContinuationReplayScope::AssistantTurnV1
    } else {
        ProviderContinuationReplayScope::InteractionV1
    };
    let mut opaque = Vec::with_capacity(reasoning_content.len() + 1);
    opaque.push(DEEPSEEK_REASONING_FRAGMENT_V1);
    opaque.extend_from_slice(reasoning_content.as_bytes());
    let continuation = ProviderContinuation::new(
        protocol.clone(),
        replay_scope,
        turn.digest(),
        vec![ProviderContinuationFragment::new(position, opaque)],
    )?;
    turn.with_provider_continuation(continuation)
}

pub(super) fn deepseek_reasoning_content<'a>(
    request_protocol: &ProviderProtocolKey,
    turn: &'a LlmAssistantTurn,
) -> AgentResult<Option<&'a str>> {
    let Some(continuation) = turn.provider_continuation() else {
        return Ok(None);
    };
    let turn_protocol = turn.provider_protocol().ok_or_else(|| {
        AgentError::new("Split-projection Assistant Turn 不能回放 DeepSeek continuation。")
    })?;
    continuation.validate_for(turn_protocol, turn.digest())?;
    if continuation.provenance() != request_protocol {
        return Err(AgentError::new(
            "DeepSeek continuation 与当前冻结 Provider 协议不一致。",
        ));
    }
    let expected_scope = if turn.provider_tool_calls().is_empty() {
        ProviderContinuationReplayScope::AssistantTurnV1
    } else {
        ProviderContinuationReplayScope::InteractionV1
    };
    if continuation.replay_scope() != expected_scope {
        return Err(AgentError::new(
            "DeepSeek continuation replay scope 与 Assistant Turn 不一致。",
        ));
    }
    deepseek_reasoning_fragment(continuation).map(Some)
}

fn deepseek_reasoning_fragment(continuation: &ProviderContinuation) -> AgentResult<&str> {
    if continuation.provenance().profile.id != ProviderProfileId::DeepSeekV4Chat
        || continuation.provenance().dialect != ProviderProtocolDialect::OpenAiChatCompletions
    {
        return Err(AgentError::new(
            "Provider continuation 不是 DeepSeek v4 chat reasoning。",
        ));
    }
    let [fragment] = continuation.fragments() else {
        return Err(AgentError::new(
            "DeepSeek continuation 必须包含一个 reasoning fragment。",
        ));
    };
    if fragment.position()
        != ProviderContinuationPosition::new(0, ProviderContinuationAttachment::AssistantTurn)
    {
        return Err(AgentError::new(
            "DeepSeek continuation reasoning fragment 位置无效。",
        ));
    }
    let Some((&version, raw_reasoning)) = fragment.opaque().split_first() else {
        return Err(AgentError::new(
            "DeepSeek continuation reasoning fragment 不能为空。",
        ));
    };
    if version != DEEPSEEK_REASONING_FRAGMENT_V1 {
        return Err(AgentError::new(
            "DeepSeek continuation reasoning fragment 版本不受支持。",
        ));
    }
    let reasoning_content = std::str::from_utf8(raw_reasoning)
        .map_err(|_| AgentError::new("DeepSeek continuation 不是有效 UTF-8 reasoning_content。"))?;
    Ok(reasoning_content)
}

/// Derived DeepSeek wire view. Tool-bearing provider turns stay grouped, while runtime-only
/// interstitial context is moved behind all matching Tool results so the actual provider wire
/// remains a legal `assistant(tool_calls) -> tool results` exchange.
pub(super) enum DeepSeekWireMessage<'a> {
    Original(&'a LlmMessage),
    Assistant {
        turn: &'a LlmAssistantTurn,
        provider_tool_calls: Vec<LlmToolCall>,
    },
    ToolResult {
        message: &'a LlmMessage,
        provider_call_id: String,
    },
}

pub(super) fn project_deepseek_exchange<'a>(
    profile: &ProviderProfileConfig,
    request_protocol: &ProviderProtocolKey,
    messages: &'a [LlmMessage],
) -> AgentResult<Vec<DeepSeekWireMessage<'a>>> {
    let mut projected = Vec::new();
    let mut seen_runtime_call_ids = BTreeSet::new();
    let mut message_index = 0usize;

    while message_index < messages.len() {
        let message = &messages[message_index];
        let Some(turn) = message.assistant_turn() else {
            if message.tool_result_fields().is_some() {
                return Err(provider_context_boundary_required(
                    "unpairedToolResult",
                    message_index,
                ));
            }
            projected.push(DeepSeekWireMessage::Original(message));
            message_index += 1;
            continue;
        };

        if turn.provider_tool_calls().is_empty() {
            projected.push(DeepSeekWireMessage::Assistant {
                turn,
                provider_tool_calls: Vec::new(),
            });
            message_index += 1;
            continue;
        }

        let Some(bindings) = turn
            .runtime_tool_bindings()
            .filter(|bindings| !bindings.is_empty())
        else {
            return Err(provider_context_boundary_required(
                "missingRuntimeToolBindings",
                message_index,
            ));
        };
        if bindings.len() != turn.provider_tool_calls().len() {
            return Err(provider_context_boundary_required(
                "incompleteGroupedToolTurn",
                message_index,
            ));
        }
        match deepseek_reasoning_content(request_protocol, turn) {
            Ok(Some(_)) => {}
            Ok(None) if profile.reasoning.mode == ReasoningMode::Enabled => {
                return Err(provider_context_boundary_required(
                    "missingToolBearingContinuation",
                    message_index,
                ));
            }
            Ok(None) => {}
            Err(_) => {
                return Err(provider_context_boundary_required(
                    "incompatibleToolBearingContinuation",
                    message_index,
                ));
            }
        }

        let mut provider_tool_calls = Vec::with_capacity(bindings.len());
        for (expected_provider_index, binding) in bindings.iter().enumerate() {
            if binding.provider_tool_index != expected_provider_index {
                return Err(provider_context_boundary_required(
                    "providerToolOrderMismatch",
                    message_index,
                ));
            }
            super::validate_model_tool_call_id(&binding.runtime_call.id)?;
            if !seen_runtime_call_ids.insert(binding.runtime_call.id.as_str()) {
                return Err(provider_context_boundary_required(
                    "duplicateRuntimeToolCallId",
                    message_index,
                ));
            }
            let provider_call = turn
                .provider_tool_calls()
                .get(binding.provider_tool_index)
                .ok_or_else(|| {
                    provider_context_boundary_required("invalidProviderToolIndex", message_index)
                })?;
            if provider_call.id != binding.provider_call_id {
                return Err(provider_context_boundary_required(
                    "providerToolIdentityMismatch",
                    message_index,
                ));
            }
            super::validate_provider_tool_call_id(&provider_call.id)?;
            provider_tool_calls.push(provider_call.clone());
        }

        let mut tool_results = Vec::with_capacity(bindings.len());
        let mut interstitial = Vec::new();
        let mut search_index = message_index + 1;
        for binding in bindings {
            let mut matched_result = None;
            while search_index < messages.len() {
                let candidate = &messages[search_index];
                if let Some((tool_call_id, _, _)) = candidate.tool_result_fields() {
                    if tool_call_id != binding.runtime_call.id {
                        return Err(provider_context_boundary_required(
                            "toolResultOrderMismatch",
                            search_index,
                        ));
                    }
                    matched_result = Some(candidate);
                    search_index += 1;
                    break;
                }
                if candidate
                    .assistant_turn()
                    .is_some_and(|candidate_turn| !candidate_turn.provider_tool_calls().is_empty())
                {
                    return Err(provider_context_boundary_required(
                        "nestedToolBearingTurn",
                        search_index,
                    ));
                }
                interstitial.push(candidate);
                search_index += 1;
            }
            let result = matched_result.ok_or_else(|| {
                provider_context_boundary_required("missingToolResult", message_index)
            })?;
            tool_results.push((result, binding.provider_call_id.clone()));
        }

        projected.push(DeepSeekWireMessage::Assistant {
            turn,
            provider_tool_calls,
        });
        projected.extend(tool_results.into_iter().map(|(message, provider_call_id)| {
            DeepSeekWireMessage::ToolResult {
                message,
                provider_call_id,
            }
        }));
        for interstitial_message in interstitial {
            if let Some(interstitial_turn) = interstitial_message.assistant_turn() {
                projected.push(DeepSeekWireMessage::Assistant {
                    turn: interstitial_turn,
                    provider_tool_calls: Vec::new(),
                });
            } else {
                projected.push(DeepSeekWireMessage::Original(interstitial_message));
            }
        }
        message_index = search_index;
    }

    Ok(projected)
}

fn provider_context_boundary_required(reason: &'static str, message_index: usize) -> AgentError {
    AgentError::structured(
        "provider_context_boundary_required",
        "DeepSeek 工具调用历史缺少可安全回放的 Provider 上下文边界。",
        json!({
            "type": "provider_context_boundary",
            "reason": reason,
            "messageIndex": message_index,
            "recovery": "startNewProviderContext",
        }),
    )
}

fn estimate_generic_continuation_tokens(
    continuation: Option<&ProviderContinuation>,
) -> AgentResult<u64> {
    if continuation.is_some() {
        return Err(AgentError::new(
            "Generic Provider Adapter 不接受 provider continuation。",
        ));
    }
    Ok(0)
}

fn estimate_deepseek_continuation_tokens(
    continuation: Option<&ProviderContinuation>,
) -> AgentResult<u64> {
    let Some(continuation) = continuation else {
        return Ok(0);
    };
    if continuation.replay_scope() == ProviderContinuationReplayScope::AssistantTurnV1 {
        // Ordinary no-tool reasoning is deliberately not replayed across a user boundary.
        return Ok(0);
    }
    let reasoning_content = deepseek_reasoning_fragment(continuation)?;
    // Measure the exact JSON escaping and field structure that `build_deepseek_messages` adds to
    // the assistant object. Wrapping the incremental property in an object adds a conservative
    // pair of braces while reusing the same Unicode-aware estimator as the context capacity gate.
    let wire_projection = serde_json::to_string(&json!({
        "reasoning_content": reasoning_content
    }))
    .map_err(|error| {
        AgentError::new(format!(
            "DeepSeek reasoning_content 无法投影为请求 JSON：{error}"
        ))
    })?;
    Ok(crate::context::ContextTextBudget::heuristic(u64::MAX).estimate(&wire_projection))
}

fn stream_state_mismatch() -> AgentError {
    AgentError::new("Provider Adapter 收到了其他协议的 stream state。")
}

/// Provider-neutral view used by Generic adapters to preserve the pre-upgrade wire sequence.
///
/// Runtime stores one complete assistant turn followed by all results. Generic profiles continue
/// to emit the historical `assistant(call) -> result` sequence as a derived projection. A future
/// signed/grouped adapter can intentionally choose a different projection without rewriting the
/// authoritative turn in Context.
pub(super) enum GenericWireMessage<'a> {
    Original(&'a LlmMessage),
    Assistant {
        visible_text: &'a str,
        tool_call: Option<&'a LlmToolCall>,
    },
    GroupedAssistant {
        visible_text: &'a str,
        tool_calls: &'a [LlmToolCall],
    },
}

pub(super) fn project_generic_split_exchange(
    messages: &[LlmMessage],
) -> AgentResult<Vec<GenericWireMessage<'_>>> {
    let mut projected = Vec::new();
    let mut message_index = 0usize;

    while message_index < messages.len() {
        let message = &messages[message_index];
        let Some(turn) = message.assistant_turn() else {
            projected.push(GenericWireMessage::Original(message));
            message_index += 1;
            continue;
        };

        reject_generic_continuation(turn.provider_continuation())?;
        if turn.runtime_tool_bindings().is_none() && turn.provider_tool_calls().len() > 1 {
            projected.push(GenericWireMessage::GroupedAssistant {
                visible_text: turn.visible_text(),
                tool_calls: turn.provider_tool_calls(),
            });
            message_index += 1;
            continue;
        }
        let calls = turn.effective_tool_calls().collect::<Vec<_>>();
        if calls.is_empty() || turn.runtime_tool_bindings().is_none() {
            projected.push(GenericWireMessage::Assistant {
                visible_text: turn.visible_text(),
                tool_call: calls.first().copied(),
            });
            message_index += 1;
            continue;
        }

        let mut search_index = message_index + 1;
        for (call_index, call) in calls.iter().enumerate() {
            let result_index = messages[search_index..]
                .iter()
                .position(|candidate| {
                    candidate
                        .tool_result_fields()
                        .is_some_and(|(call_id, _, _)| call_id == call.id)
                })
                .map(|offset| search_index + offset)
                .ok_or_else(|| {
                    AgentError::new(
                        "Generic 多工具 assistant turn 缺少匹配 runtime call id 的 tool result。",
                    )
                })?;
            for interstitial in &messages[search_index..result_index] {
                if interstitial.tool_result_fields().is_some() {
                    return Err(AgentError::new(
                        "Generic 多工具 assistant turn 的 tool results 身份或顺序不匹配。",
                    ));
                }
                if let Some(interstitial_turn) = interstitial.assistant_turn() {
                    reject_generic_continuation(interstitial_turn.provider_continuation())?;
                    if !interstitial_turn.effective_tool_calls().is_empty() {
                        return Err(AgentError::new(
                            "Generic 多工具 assistant turn 尚未结算时不能嵌套新的 Tool Call。",
                        ));
                    }
                    projected.push(GenericWireMessage::Assistant {
                        visible_text: interstitial_turn.visible_text(),
                        tool_call: None,
                    });
                } else {
                    projected.push(GenericWireMessage::Original(interstitial));
                }
            }
            let result = &messages[result_index];
            if result.tool_call_id() != Some(call.id.as_str()) {
                return Err(AgentError::new(
                    "Generic 多工具 assistant turn 与 tool result 身份或顺序不匹配。",
                ));
            }
            projected.push(GenericWireMessage::Assistant {
                visible_text: if call_index == 0 {
                    turn.visible_text()
                } else {
                    ""
                },
                tool_call: Some(call),
            });
            projected.push(GenericWireMessage::Original(result));
            search_index = result_index + 1;
        }
        message_index = search_index;
    }

    Ok(projected)
}

fn validate_generic_split_wire_protocol(messages: &[LlmMessage]) -> AgentResult<()> {
    let projected = project_generic_split_exchange(messages)?;
    validate_generic_wire_protocol(&projected)
}

fn validate_generic_wire_protocol(messages: &[GenericWireMessage<'_>]) -> AgentResult<()> {
    let messages = messages
        .iter()
        .map(|message| match message {
            GenericWireMessage::Original(message) => (*message).clone(),
            GenericWireMessage::Assistant {
                visible_text,
                tool_call,
            } => LlmMessage::assistant(
                *visible_text,
                tool_call.iter().map(|call| (*call).clone()).collect(),
            ),
            GenericWireMessage::GroupedAssistant {
                visible_text,
                tool_calls,
            } => LlmMessage::assistant(*visible_text, tool_calls.to_vec()),
        })
        .collect::<Vec<_>>();
    super::validate_model_tool_protocol(&messages)
}

fn reject_generic_continuation(continuation: Option<&ProviderContinuation>) -> AgentResult<()> {
    if continuation.is_some() {
        return Err(AgentError::new(
            "Generic Provider Adapter 不会发送 provider continuation。",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::{
        model_response_tool_call_id, LlmImage, LlmMessageRole, LlmRuntimeToolCallBinding,
    };
    use serde_json::json;

    #[test]
    fn generic_projection_pairs_multi_tool_results_across_interstitial_context() {
        let provider_calls = vec![
            LlmToolCall {
                id: "provider-call-1".to_string(),
                name: "read_file".to_string(),
                args: json!({ "path": "one.png" }),
            },
            LlmToolCall {
                id: "provider-call-2".to_string(),
                name: "read_file".to_string(),
                args: json!({ "path": "two.txt" }),
            },
        ];
        let runtime_calls = provider_calls
            .iter()
            .enumerate()
            .map(|(index, provider_call)| LlmToolCall {
                id: model_response_tool_call_id("projection-run", 0, index, &provider_call.id),
                name: provider_call.name.clone(),
                args: provider_call.args.clone(),
            })
            .collect::<Vec<_>>();
        let bindings = provider_calls
            .iter()
            .zip(runtime_calls.iter().cloned())
            .enumerate()
            .map(|(index, (provider_call, runtime_call))| {
                LlmRuntimeToolCallBinding::new(index, provider_call, runtime_call)
            })
            .collect();
        let turn =
            LlmAssistantTurn::from_split_projection("Inspecting both files.", provider_calls)
                .with_runtime_tool_bindings(bindings)
                .unwrap();
        let mut image_context = LlmMessage::text(LlmMessageRole::User, "Image from first result");
        image_context.images_mut().unwrap().push(LlmImage {
            mime_type: "image/png".to_string(),
            data_base64: "AA==".to_string(),
        });
        let extension_context = LlmMessage::text(
            LlmMessageRole::Assistant,
            "Runtime extension state updated.",
        );
        let messages = vec![
            LlmMessage::from_assistant_turn(turn),
            LlmMessage::tool_result(runtime_calls[0].id.clone(), "first result", false),
            image_context,
            extension_context,
            LlmMessage::tool_result(runtime_calls[1].id.clone(), "second result", false),
        ];

        let projected = project_generic_split_exchange(&messages).unwrap();

        assert_eq!(projected.len(), 6);
        assert!(matches!(
            &projected[0],
            GenericWireMessage::Assistant {
                tool_call: Some(call),
                ..
            } if call.id == runtime_calls[0].id
        ));
        assert!(
            matches!(&projected[1], GenericWireMessage::Original(message)
            if message.tool_call_id() == Some(runtime_calls[0].id.as_str()))
        );
        assert!(
            matches!(&projected[2], GenericWireMessage::Original(message)
            if message.role() == LlmMessageRole::User && message.images().len() == 1)
        );
        assert!(matches!(
            &projected[3],
            GenericWireMessage::Assistant {
                tool_call: None,
                ..
            }
        ));
        assert!(matches!(
            &projected[4],
            GenericWireMessage::Assistant {
                tool_call: Some(call),
                ..
            } if call.id == runtime_calls[1].id
        ));
        assert!(
            matches!(&projected[5], GenericWireMessage::Original(message)
            if message.tool_call_id() == Some(runtime_calls[1].id.as_str()))
        );
    }

    #[test]
    fn generic_projection_scans_a_single_remaining_runtime_binding() {
        let provider_calls = vec![
            LlmToolCall {
                id: "provider-omitted".to_string(),
                name: "external_tool".to_string(),
                args: json!({}),
            },
            LlmToolCall {
                id: "provider-retained".to_string(),
                name: "read_file".to_string(),
                args: json!({ "path": "retained.txt" }),
            },
        ];
        let runtime_call = LlmToolCall {
            id: model_response_tool_call_id("single-binding-run", 0, 1, &provider_calls[1].id),
            name: provider_calls[1].name.clone(),
            args: provider_calls[1].args.clone(),
        };
        let binding = LlmRuntimeToolCallBinding::new(1, &provider_calls[1], runtime_call.clone());
        let turn =
            LlmAssistantTurn::from_split_projection("Reading the retained file.", provider_calls)
                .with_runtime_tool_bindings(vec![binding])
                .unwrap();
        let messages = vec![
            LlmMessage::from_assistant_turn(turn),
            LlmMessage::text(LlmMessageRole::User, "Interstitial runtime context"),
            LlmMessage::tool_result(runtime_call.id.clone(), "retained result", false),
        ];

        let projected = project_generic_split_exchange(&messages).unwrap();

        assert_eq!(projected.len(), 3);
        assert!(matches!(
            &projected[0],
            GenericWireMessage::Original(message)
                if message.content() == "Interstitial runtime context"
        ));
        assert!(matches!(
            &projected[1],
            GenericWireMessage::Assistant {
                tool_call: Some(call),
                ..
            } if call.id == runtime_call.id
        ));
        assert!(matches!(
            &projected[2],
            GenericWireMessage::Original(message)
                if message.tool_call_id() == Some(runtime_call.id.as_str())
        ));
        validate_generic_wire_protocol(&projected).unwrap();
    }
}
