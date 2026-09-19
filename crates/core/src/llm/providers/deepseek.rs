//! DeepSeek Chat Completions wire protocol.
//!
//! DeepSeek policy stays wholly inside this module. The shared OpenAI helpers are provider-neutral.

use super::super::adapter::{stream_state_mismatch, ProviderAdapter};
use super::super::payload::{
    build_openai_headers, build_openai_tool_calls, build_openai_tools, build_openai_user_content,
    render_backend_observed_state,
};
use super::super::provider_error::{LlmProviderFailureCategory, ProviderErrorClassification};
use super::super::response::{extract_openai_tool_calls, extract_response_text};
use super::super::stream::{OpenAiStreamAccumulator, ProviderStreamState};
use super::super::{
    LlmAssistantTurn, LlmChatRequest, LlmChatResponse, LlmMessage, LlmMessagePlacement,
    LlmMessageRole, LlmStreamEvent, LlmToolCall, ProviderContinuation,
    ProviderContinuationAttachment, ProviderContinuationFragment, ProviderContinuationPosition,
    ProviderContinuationReplayScope, MAX_PROVIDER_CONTINUATION_BYTES,
};
use crate::protocol::{AgentError, AgentResult, AgentUsage};
use crate::provider_profile::{
    ProviderFamilyReasoningPolicy, ProviderFamilySettings, ProviderProfileConfig,
    ProviderProfileConfigV2, ProviderProfileId, ProviderProtocolDialect, ProviderProtocolKey,
    ProviderReasoningEffort, ReasoningMode,
};
use crate::provider_registration::{
    ProviderImageInputPolicy, ProviderRegistration, DEEPSEEK_V4_1_FLASH_CHAT_REGISTRATION,
    DEEPSEEK_V4_PRO_0813_CHAT_REGISTRATION,
};
use crate::usage::{extract_usage, merge_stream_usage};
use reqwest::header::HeaderMap;
use serde_json::{json, Map, Value};
use std::collections::BTreeSet;

const DEEPSEEK_REASONING_FRAGMENT_V1: u8 = 1;

pub(in crate::llm) struct DeepSeekAdapter {
    registration: &'static ProviderRegistration,
}

pub(in crate::llm) static DEEPSEEK_V4_1_FLASH_CHAT_ADAPTER: DeepSeekAdapter = DeepSeekAdapter {
    registration: &DEEPSEEK_V4_1_FLASH_CHAT_REGISTRATION,
};
pub(in crate::llm) static DEEPSEEK_V4_PRO_0813_CHAT_ADAPTER: DeepSeekAdapter = DeepSeekAdapter {
    registration: &DEEPSEEK_V4_PRO_0813_CHAT_REGISTRATION,
};

impl ProviderAdapter for DeepSeekAdapter {
    fn registration(&self) -> &'static ProviderRegistration {
        self.registration
    }

    fn classify_provider_error(
        &self,
        status: Option<u16>,
        provider_code: Option<&str>,
        body: &str,
    ) -> ProviderErrorClassification {
        ProviderErrorClassification::Authoritative(
            classify_error(status, provider_code, body)
                .unwrap_or(LlmProviderFailureCategory::Unknown),
        )
    }

    fn validate_profile_settings(&self, request: &LlmChatRequest) -> AgentResult<()> {
        request
            .provider_protocol
            .validate_against_config(&request.provider_profile)
            .map_err(|error| AgentError::new(format!("DeepSeek profile 设置无效：{error}")))?;
        if request.provider_protocol.profile != self.profile()
            || request.provider_protocol.dialect != self.dialect()
        {
            return Err(AgentError::new(
                "DeepSeek profile 设置与所选 Adapter 不一致。",
            ));
        }
        if reasoning_policy(&request.provider_profile).is_none() {
            return Err(AgentError::new(
                "DeepSeek profile 缺少对应 family reasoning policy。",
            ));
        }
        if self.registration.image_input_policy() == ProviderImageInputPolicy::Unsupported
            && request
                .messages
                .iter()
                .any(|message| !message.images().is_empty())
        {
            return Err(AgentError::structured(
                "provider_image_unsupported",
                "当前 DeepSeek model family 不支持图片输入。",
                json!({
                    "type": "providerCapabilityBoundary",
                    "capability": "imageInput",
                    "recovery": "selectVisionFamily",
                }),
            ));
        }
        Ok(())
    }

    fn validate_wire_protocol(&self, request: &LlmChatRequest) -> AgentResult<()> {
        let _ = project_exchange(
            &request.provider_profile,
            &request.provider_protocol,
            &request.messages,
            !request.tools.is_empty(),
        )?;
        Ok(())
    }

    fn prepare_request(&self, request: &LlmChatRequest) -> AgentResult<Value> {
        build_payload(request)
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
        attach_reasoning(profile, protocol, turn, extract_reasoning_content(value)?)
    }

    fn new_stream_state(
        &self,
        _profile: &ProviderProfileConfig,
    ) -> AgentResult<ProviderStreamState> {
        Ok(ProviderStreamState::DeepSeek(Box::default()))
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
        response.assistant_turn = attach_reasoning(
            profile,
            protocol,
            response.assistant_turn,
            reasoning_content.as_deref(),
        )?;
        response.usage = response.usage.map(project_usage);
        Ok(response)
    }

    fn has_private_model_action(
        &self,
        protocol: &ProviderProtocolKey,
        turn: &LlmAssistantTurn,
    ) -> AgentResult<bool> {
        Ok(reasoning_content(protocol, turn)?.is_some_and(|reasoning| !reasoning.is_empty()))
    }

    fn project_usage(&self, _profile: &ProviderProfileConfig, value: &Value) -> Option<AgentUsage> {
        extract_usage(value).map(project_usage)
    }

    fn estimate_continuation_tokens(
        &self,
        continuation: Option<&ProviderContinuation>,
    ) -> AgentResult<u64> {
        estimate_continuation_tokens(continuation)
    }
}

pub(super) fn build_payload(request: &LlmChatRequest) -> AgentResult<Value> {
    let policy = reasoning_policy(&request.provider_profile)
        .ok_or_else(|| AgentError::new("DeepSeek payload 缺少 reasoning policy。"))?;
    let mut payload = Map::from_iter([
        ("model".to_string(), json!(request.model())),
        (
            "messages".to_string(),
            Value::Array(build_messages(request)?),
        ),
        ("stream".to_string(), json!(request.stream)),
    ]);
    if let Some(max_tokens) = request.max_tokens {
        payload.insert("max_tokens".to_string(), json!(max_tokens));
    }

    match policy.mode {
        ReasoningMode::ProviderDefault => {}
        ReasoningMode::Enabled => {
            payload.insert("thinking".to_string(), json!({ "type": "enabled" }));
        }
        ReasoningMode::Disabled => {
            payload.insert("thinking".to_string(), json!({ "type": "disabled" }));
            payload.insert("temperature".to_string(), json!(request.temperature));
        }
    }
    match policy.effort {
        ProviderReasoningEffort::ProviderDefault => {}
        ProviderReasoningEffort::Low => {
            payload.insert("reasoning_effort".to_string(), json!("low"));
        }
        ProviderReasoningEffort::High => {
            payload.insert("reasoning_effort".to_string(), json!("high"));
        }
        ProviderReasoningEffort::Max => {
            payload.insert("reasoning_effort".to_string(), json!("max"));
        }
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
    }

    Ok(Value::Object(payload))
}

fn reasoning_policy(profile: &ProviderProfileConfig) -> Option<ProviderFamilyReasoningPolicy> {
    match profile {
        ProviderProfileConfig::V2(ProviderProfileConfigV2 {
            settings: ProviderFamilySettings::DeepseekFlashChat { reasoning },
            ..
        })
        | ProviderProfileConfig::V2(ProviderProfileConfigV2 {
            settings: ProviderFamilySettings::DeepseekProChat { reasoning },
            ..
        }) => Some(*reasoning),
        _ => None,
    }
}

fn build_messages(request: &LlmChatRequest) -> AgentResult<Vec<Value>> {
    let projected = project_exchange(
        &request.provider_profile,
        &request.provider_protocol,
        &request.messages,
        !request.tools.is_empty(),
    )?;
    let mut messages = Vec::with_capacity(projected.len());

    for wire_message in projected {
        let message = match wire_message {
            DeepSeekWireMessage::Original(message) => match (message.role(), message.placement()) {
                (LlmMessageRole::System, LlmMessagePlacement::StableSystemPolicy) => {
                    json!({ "role": "system", "content": message.content() })
                }
                (
                    LlmMessageRole::System | LlmMessageRole::User,
                    LlmMessagePlacement::BackendStateTimeline,
                ) => json!({
                    "role": "user",
                    "content": render_backend_observed_state(message.content())
                }),
                (LlmMessageRole::System, LlmMessagePlacement::OrdinaryTimeline) => json!({
                    "role": "user",
                    "content": message.content()
                }),
                (LlmMessageRole::User, _) => json!({
                    "role": "user",
                    "content": build_openai_user_content(message)
                }),
                (LlmMessageRole::Tool, _) => unreachable!("tool results use projected view"),
                (LlmMessageRole::Assistant, _) => {
                    unreachable!("assistant messages use turn view")
                }
            },
            DeepSeekWireMessage::Assistant {
                turn,
                provider_tool_calls,
            } => {
                let mut object = Map::from_iter([
                    ("role".to_string(), json!("assistant")),
                    ("content".to_string(), json!(turn.provider_visible_text())),
                ]);
                let replays_reasoning = request.provider_profile.reasoning_mode()
                    != ReasoningMode::Disabled
                    && (!request.tools.is_empty() || !provider_tool_calls.is_empty());
                if replays_reasoning {
                    if let Some(reasoning_content) =
                        reasoning_content(&request.provider_protocol, turn)?
                    {
                        object.insert("reasoning_content".to_string(), json!(reasoning_content));
                    } else if turn.provider_protocol().is_some() {
                        return Err(provider_context_boundary_required(
                            "missingToolsReasoningHistory",
                            0,
                        ));
                    }
                }
                if !provider_tool_calls.is_empty() {
                    object.insert(
                        "tool_calls".to_string(),
                        Value::Array(build_openai_tool_calls(&provider_tool_calls)),
                    );
                }
                Value::Object(object)
            }
            DeepSeekWireMessage::ToolResult {
                message,
                provider_call_id,
            } => json!({
                "role": "tool",
                "tool_call_id": provider_call_id,
                "content": message.content()
            }),
        };
        messages.push(message);
    }
    Ok(messages)
}

fn extract_reasoning_content(value: &Value) -> AgentResult<Option<&str>> {
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

fn attach_reasoning(
    profile: &ProviderProfileConfig,
    protocol: &ProviderProtocolKey,
    turn: LlmAssistantTurn,
    reasoning_content: Option<&str>,
) -> AgentResult<LlmAssistantTurn> {
    let reasoning_content = match reasoning_content {
        Some(reasoning_content)
            if profile.reasoning_mode() == ReasoningMode::Disabled
                && !reasoning_content.is_empty() =>
        {
            return Err(AgentError::structured(
                "provider_reasoning_forbidden",
                "DeepSeek disabled-thinking 响应意外包含 reasoning_content。",
                json!({
                    "type": "providerReasoningBoundary",
                    "reason": "reasoningPresentWhileDisabled",
                    "recovery": "retryProviderRequest",
                }),
            ));
        }
        Some(_) if profile.reasoning_mode() == ReasoningMode::Disabled => return Ok(turn),
        Some(reasoning_content) => reasoning_content,
        None if profile.reasoning_mode() != ReasoningMode::Disabled
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

pub(in crate::llm) fn reasoning_content<'a>(
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
    reasoning_fragment(continuation).map(Some)
}

fn reasoning_fragment(continuation: &ProviderContinuation) -> AgentResult<&str> {
    if !matches!(
        continuation.provenance().profile.id,
        ProviderProfileId::DeepSeekV41FlashChat | ProviderProfileId::DeepSeekV4Pro0813Chat
    ) || continuation.provenance().dialect != ProviderProtocolDialect::OpenAiChatCompletions
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
    std::str::from_utf8(raw_reasoning)
        .map_err(|_| AgentError::new("DeepSeek continuation 不是有效 UTF-8 reasoning_content。"))
}

fn estimate_continuation_tokens(continuation: Option<&ProviderContinuation>) -> AgentResult<u64> {
    let Some(continuation) = continuation else {
        return Ok(0);
    };
    // DeepSeek ignores ordinary reasoning when a request has no tools, but a later request can
    // add tools. Count it conservatively so the context gate remains safe for that transition.
    let reasoning_content = reasoning_fragment(continuation)?;
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

pub(super) fn project_exchange<'a>(
    profile: &ProviderProfileConfig,
    request_protocol: &ProviderProtocolKey,
    messages: &'a [LlmMessage],
    request_has_tools: bool,
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
            if request_has_tools
                && profile.reasoning_mode() != ReasoningMode::Disabled
                && turn.provider_protocol().is_some()
            {
                match reasoning_content(request_protocol, turn) {
                    Ok(Some(_)) => {}
                    Ok(None) => {
                        return Err(provider_context_boundary_required(
                            "missingOrdinaryToolsReasoning",
                            message_index,
                        ));
                    }
                    Err(_) => {
                        return Err(provider_context_boundary_required(
                            "incompatibleOrdinaryToolsReasoning",
                            message_index,
                        ));
                    }
                }
            }
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
        if profile.reasoning_mode() != ReasoningMode::Disabled {
            match reasoning_content(request_protocol, turn) {
                Ok(Some(_)) => {}
                Ok(None) => {
                    return Err(provider_context_boundary_required(
                        "missingToolBearingContinuation",
                        message_index,
                    ));
                }
                Err(_) => {
                    return Err(provider_context_boundary_required(
                        "incompatibleToolBearingContinuation",
                        message_index,
                    ));
                }
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
            super::super::validate_model_tool_call_id(&binding.runtime_call.id)?;
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
            super::super::validate_provider_tool_call_id(&provider_call.id)?;
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

/// DeepSeek reports `completion_tokens` as the complete billable output. Reasoning tokens, when
/// present, are a diagnostic subset rather than another amount to subtract or add. A prompt cache
/// miss is ordinary uncached input, not cache creation.
pub(super) fn project_usage(mut usage: AgentUsage) -> AgentUsage {
    let invalid_reasoning_breakdown = match (usage.output_tokens, usage.output_thinking_tokens) {
        (None, Some(_)) => true,
        (Some(completion), Some(reasoning)) => reasoning > completion,
        _ => false,
    };
    if invalid_reasoning_breakdown {
        usage.output_thinking_tokens = None;
    }
    usage.cache_creation_input_tokens = None;
    usage
}

#[derive(Default)]
pub(in crate::llm) struct DeepSeekStreamAccumulator {
    openai: OpenAiStreamAccumulator,
    reasoning_content: String,
    saw_reasoning_content: bool,
    raw_usage: Option<AgentUsage>,
    pub(in crate::llm) projected_usage: Option<AgentUsage>,
}

impl DeepSeekStreamAccumulator {
    fn process(
        &mut self,
        value: &Value,
        on_delta: &mut dyn FnMut(LlmStreamEvent),
    ) -> AgentResult<()> {
        merge_stream_usage(&mut self.raw_usage, extract_usage(value));
        self.projected_usage = self.raw_usage.clone().map(project_usage);
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

    fn finish(
        self,
        provider_protocol: &ProviderProtocolKey,
    ) -> AgentResult<(LlmChatResponse, Option<String>)> {
        let response = self.openai.finish(provider_protocol)?;
        let reasoning_content = self.saw_reasoning_content.then_some(self.reasoning_content);
        Ok((response, reasoning_content))
    }
}

/// DeepSeek's documented error vocabulary over an OpenAI-compatible envelope.
pub(in crate::llm) fn classify_error(
    status: Option<u16>,
    provider_code: Option<&str>,
    _body: &str,
) -> Option<LlmProviderFailureCategory> {
    match provider_code.map(str::to_ascii_lowercase).as_deref() {
        Some("authentication_error" | "invalid_api_key") => {
            Some(LlmProviderFailureCategory::Authentication)
        }
        Some("permission_denied") => Some(LlmProviderFailureCategory::PermissionDenied),
        Some("model_not_found" | "resource_not_found") => {
            Some(LlmProviderFailureCategory::ResourceNotFound)
        }
        Some("insufficient_balance" | "insufficient_quota") => {
            Some(LlmProviderFailureCategory::QuotaExhausted)
        }
        Some("rate_limit_error" | "rate_limit_exceeded") => {
            Some(LlmProviderFailureCategory::RateLimited)
        }
        Some("server_error" | "overloaded_error") => Some(LlmProviderFailureCategory::Overloaded),
        Some("context_length_exceeded" | "max_tokens_exceeded") => {
            Some(LlmProviderFailureCategory::ContextTooLarge)
        }
        Some("invalid_request_error") => Some(LlmProviderFailureCategory::InvalidRequest),
        _ => match status {
            Some(400 | 405 | 415 | 422) => Some(LlmProviderFailureCategory::InvalidRequest),
            Some(401) => Some(LlmProviderFailureCategory::Authentication),
            Some(402) => Some(LlmProviderFailureCategory::QuotaExhausted),
            Some(403) => Some(LlmProviderFailureCategory::PermissionDenied),
            Some(404) => Some(LlmProviderFailureCategory::ResourceNotFound),
            Some(413) => Some(LlmProviderFailureCategory::ContextTooLarge),
            Some(429) => Some(LlmProviderFailureCategory::RateLimited),
            Some(500 | 502 | 503 | 504 | 529) => Some(LlmProviderFailureCategory::Overloaded),
            _ => None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deepseek_error_codes_keep_vendor_semantics_distinct() {
        let cases = [
            (
                "authentication_error",
                LlmProviderFailureCategory::Authentication,
            ),
            (
                "permission_denied",
                LlmProviderFailureCategory::PermissionDenied,
            ),
            (
                "model_not_found",
                LlmProviderFailureCategory::ResourceNotFound,
            ),
            (
                "insufficient_balance",
                LlmProviderFailureCategory::QuotaExhausted,
            ),
            ("rate_limit_error", LlmProviderFailureCategory::RateLimited),
            ("overloaded_error", LlmProviderFailureCategory::Overloaded),
            (
                "context_length_exceeded",
                LlmProviderFailureCategory::ContextTooLarge,
            ),
            (
                "invalid_request_error",
                LlmProviderFailureCategory::InvalidRequest,
            ),
        ];
        for (code, expected) in cases {
            assert_eq!(classify_error(None, Some(code), ""), Some(expected));
        }
        assert_eq!(
            classify_error(None, Some("foreign_vendor_error"), ""),
            None,
            "unregistered error vocabulary must fail closed"
        );
    }
}
