//! Moonshot/Kimi Chat Completions wire protocol.
//!
//! This module deliberately owns every Moonshot semantic decision and only reuses neutral OpenAI
//! Chat Completions structures.

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
    MoonshotK26ThinkingMode, ProviderFamilySettings, ProviderProfileConfig, ProviderProfileId,
    ProviderProtocolDialect, ProviderProtocolKey, ProviderReasoningEffort,
};
use crate::provider_registration::{
    ProviderRegistration, MOONSHOT_K2_6_CHAT_REGISTRATION, MOONSHOT_K2_7_CODE_CHAT_REGISTRATION,
    MOONSHOT_K3_CHAT_REGISTRATION,
};
use crate::usage::{extract_usage, merge_stream_usage};
use reqwest::header::HeaderMap;
use serde_json::{json, Map, Value};
use std::collections::BTreeSet;

const K3_REASONING_FRAGMENT_V1: u8 = 0x31;
const K2_7_REASONING_FRAGMENT_V1: u8 = 0x27;
const K2_6_REASONING_FRAGMENT_V1: u8 = 0x26;
const REASONING_FIELD_ABSENT: u8 = 0;
const REASONING_FIELD_PRESENT: u8 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CapturedReasoning<'a> {
    Absent,
    Present(&'a str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MoonshotFamily {
    K3,
    K27Code,
    K26,
}

impl MoonshotFamily {
    fn from_profile_id(profile_id: ProviderProfileId) -> AgentResult<Self> {
        match profile_id {
            ProviderProfileId::MoonshotK3Chat => Ok(Self::K3),
            ProviderProfileId::MoonshotK27CodeChat => Ok(Self::K27Code),
            ProviderProfileId::MoonshotK26Chat => Ok(Self::K26),
            _ => Err(AgentError::new(
                "Provider profile 不是 Moonshot Chat family。",
            )),
        }
    }

    const fn continuation_magic(self) -> u8 {
        match self {
            Self::K3 => K3_REASONING_FRAGMENT_V1,
            Self::K27Code => K2_7_REASONING_FRAGMENT_V1,
            Self::K26 => K2_6_REASONING_FRAGMENT_V1,
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::K3 => "Moonshot K3",
            Self::K27Code => "Moonshot K2.7 Code",
            Self::K26 => "Moonshot K2.6",
        }
    }
}

pub(in crate::llm) struct MoonshotAdapter {
    registration: &'static ProviderRegistration,
    family: MoonshotFamily,
}

pub(in crate::llm) static MOONSHOT_K3_CHAT_ADAPTER: MoonshotAdapter = MoonshotAdapter {
    registration: &MOONSHOT_K3_CHAT_REGISTRATION,
    family: MoonshotFamily::K3,
};
pub(in crate::llm) static MOONSHOT_K2_7_CODE_CHAT_ADAPTER: MoonshotAdapter = MoonshotAdapter {
    registration: &MOONSHOT_K2_7_CODE_CHAT_REGISTRATION,
    family: MoonshotFamily::K27Code,
};
pub(in crate::llm) static MOONSHOT_K2_6_CHAT_ADAPTER: MoonshotAdapter = MoonshotAdapter {
    registration: &MOONSHOT_K2_6_CHAT_REGISTRATION,
    family: MoonshotFamily::K26,
};

impl ProviderAdapter for MoonshotAdapter {
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
            .map_err(|error| AgentError::new(format!("Moonshot profile 设置无效：{error}")))?;
        if request.provider_protocol.profile != self.profile()
            || request.provider_protocol.dialect != self.dialect()
        {
            return Err(AgentError::new(
                "Moonshot profile 设置与所选 Adapter 不一致。",
            ));
        }
        validate_family_settings(&request.provider_profile, self.family)
    }

    fn validate_wire_protocol(&self, request: &LlmChatRequest) -> AgentResult<()> {
        let _ = project_exchange(
            &request.provider_profile,
            &request.provider_protocol,
            &request.messages,
        )?;
        Ok(())
    }

    fn prepare_request(&self, request: &LlmChatRequest) -> AgentResult<Value> {
        build_payload(request, self.family)
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
        profile: &ProviderProfileConfig,
    ) -> AgentResult<ProviderStreamState> {
        validate_family_settings(profile, self.family)?;
        Ok(ProviderStreamState::Moonshot(Box::new(
            MoonshotStreamAccumulator::new(profile)?,
        )))
    }

    fn consume_streaming_event(
        &self,
        state: &mut ProviderStreamState,
        _event: Option<&str>,
        value: &Value,
        on_delta: &mut dyn FnMut(LlmStreamEvent),
    ) -> AgentResult<()> {
        let ProviderStreamState::Moonshot(state) = state else {
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
        let ProviderStreamState::Moonshot(state) = state else {
            return Err(stream_state_mismatch());
        };
        let (mut response, reasoning_content) = (*state).finish(protocol)?;
        response.assistant_turn = attach_reasoning(
            profile,
            protocol,
            response.assistant_turn,
            reasoning_content.as_deref(),
        )?;
        response.usage = response.usage.map(|usage| project_usage(profile, usage));
        Ok(response)
    }

    fn has_private_model_action(
        &self,
        protocol: &ProviderProtocolKey,
        turn: &LlmAssistantTurn,
    ) -> AgentResult<bool> {
        Ok(matches!(
            captured_reasoning(protocol, turn)?,
            Some(CapturedReasoning::Present(reasoning)) if !reasoning.is_empty()
        ))
    }

    fn project_usage(&self, profile: &ProviderProfileConfig, value: &Value) -> Option<AgentUsage> {
        extract_usage(value).map(|usage| project_usage(profile, usage))
    }

    fn estimate_continuation_tokens(
        &self,
        continuation: Option<&ProviderContinuation>,
    ) -> AgentResult<u64> {
        estimate_continuation_tokens(continuation)
    }
}

fn validate_family_settings(
    profile: &ProviderProfileConfig,
    family: MoonshotFamily,
) -> AgentResult<()> {
    let settings_match = matches!(
        (family, profile.family_settings()),
        (
            MoonshotFamily::K3,
            Some(ProviderFamilySettings::MoonshotK3Chat { .. })
        ) | (
            MoonshotFamily::K27Code,
            Some(ProviderFamilySettings::MoonshotK27CodeChat)
        ) | (
            MoonshotFamily::K26,
            Some(ProviderFamilySettings::MoonshotK26Chat { .. })
        )
    );
    if !settings_match {
        return Err(AgentError::new(format!(
            "{} profile settings kind 不匹配。",
            family.label()
        )));
    }
    Ok(())
}

fn build_payload(request: &LlmChatRequest, family: MoonshotFamily) -> AgentResult<Value> {
    validate_family_settings(&request.provider_profile, family)?;
    let messages = build_messages(request)?;
    let mut payload = Map::from_iter([
        ("model".to_string(), json!(request.model())),
        ("messages".to_string(), Value::Array(messages)),
        ("stream".to_string(), json!(request.stream)),
        (
            "max_completion_tokens".to_string(),
            json!(request.max_tokens),
        ),
    ]);

    match (family, request.provider_profile.family_settings()) {
        (MoonshotFamily::K3, Some(ProviderFamilySettings::MoonshotK3Chat { reasoning_effort })) => {
            match reasoning_effort {
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
        }
        (MoonshotFamily::K27Code, Some(ProviderFamilySettings::MoonshotK27CodeChat)) => {
            // K2.7 Code is unconditionally preserved-thinking. The official Chat contract says
            // callers should omit `thinking`; in particular, never send K2.6's disabled shape.
        }
        (MoonshotFamily::K26, Some(ProviderFamilySettings::MoonshotK26Chat { thinking_mode })) => {
            match thinking_mode {
                MoonshotK26ThinkingMode::ProviderDefault => {}
                MoonshotK26ThinkingMode::Enabled => {
                    payload.insert("thinking".to_string(), json!({ "type": "enabled" }));
                }
                MoonshotK26ThinkingMode::Disabled => {
                    payload.insert("thinking".to_string(), json!({ "type": "disabled" }));
                }
                MoonshotK26ThinkingMode::EnabledKeepAll => {
                    payload.insert(
                        "thinking".to_string(),
                        json!({ "type": "enabled", "keep": "all" }),
                    );
                }
            }
        }
        _ => {
            return Err(AgentError::new(
                "Moonshot payload settings 与模型 family 不一致。",
            ));
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
        payload.insert("tool_choice".to_string(), json!("auto"));
    }

    Ok(Value::Object(payload))
}

fn build_messages(request: &LlmChatRequest) -> AgentResult<Vec<Value>> {
    let projected = project_exchange(
        &request.provider_profile,
        &request.provider_protocol,
        &request.messages,
    )?;
    let mut messages = Vec::with_capacity(projected.len());

    for wire_message in projected {
        let message = match wire_message {
            MoonshotWireMessage::Original(message) => match (message.role(), message.placement()) {
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
            MoonshotWireMessage::Assistant {
                turn,
                provider_tool_calls,
            } => {
                let mut object = Map::from_iter([
                    ("role".to_string(), json!("assistant")),
                    ("content".to_string(), json!(turn.provider_visible_text())),
                ]);
                let replay_policy = reasoning_replay_policy(&request.provider_profile)?;
                let requires_reasoning = (replay_policy.replays_ordinary
                    && turn.provider_protocol().is_some())
                    || (!provider_tool_calls.is_empty() && replay_policy.replays_tool_turns);
                match captured_reasoning(&request.provider_protocol, turn)? {
                    Some(CapturedReasoning::Present(reasoning_content)) => {
                        if !replay_policy.accepts_reasoning && !reasoning_content.is_empty() {
                            return Err(provider_reasoning_boundary_required(
                                "reasoningPresentWhileDisabled",
                            ));
                        }
                        if replay_policy.accepts_reasoning && requires_reasoning {
                            object
                                .insert("reasoning_content".to_string(), json!(reasoning_content));
                        }
                    }
                    Some(CapturedReasoning::Absent) => {}
                    None if requires_reasoning => {
                        return Err(provider_reasoning_boundary_required(
                            "missingPreservedReasoning",
                        ));
                    }
                    None => {}
                }
                if !provider_tool_calls.is_empty() {
                    object.insert(
                        "tool_calls".to_string(),
                        Value::Array(build_openai_tool_calls(&provider_tool_calls)),
                    );
                }
                Value::Object(object)
            }
            MoonshotWireMessage::ToolResult {
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

#[derive(Debug, Clone, Copy)]
struct ReasoningReplayPolicy {
    accepts_reasoning: bool,
    replays_ordinary: bool,
    replays_tool_turns: bool,
}

fn reasoning_replay_policy(profile: &ProviderProfileConfig) -> AgentResult<ReasoningReplayPolicy> {
    match profile.family_settings() {
        Some(ProviderFamilySettings::MoonshotK3Chat { .. })
        | Some(ProviderFamilySettings::MoonshotK27CodeChat) => Ok(ReasoningReplayPolicy {
            accepts_reasoning: true,
            replays_ordinary: true,
            replays_tool_turns: true,
        }),
        Some(ProviderFamilySettings::MoonshotK26Chat { thinking_mode }) => match thinking_mode {
            MoonshotK26ThinkingMode::ProviderDefault | MoonshotK26ThinkingMode::Enabled => {
                Ok(ReasoningReplayPolicy {
                    accepts_reasoning: true,
                    replays_ordinary: false,
                    replays_tool_turns: true,
                })
            }
            MoonshotK26ThinkingMode::Disabled => Ok(ReasoningReplayPolicy {
                accepts_reasoning: false,
                replays_ordinary: false,
                replays_tool_turns: false,
            }),
            MoonshotK26ThinkingMode::EnabledKeepAll => Ok(ReasoningReplayPolicy {
                accepts_reasoning: true,
                replays_ordinary: true,
                replays_tool_turns: true,
            }),
        },
        _ => Err(AgentError::new(
            "Provider profile 不包含 Moonshot reasoning replay policy。",
        )),
    }
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
            "Moonshot 响应中的 reasoning_content 不是字符串。",
        )),
    }
}

fn attach_reasoning(
    profile: &ProviderProfileConfig,
    protocol: &ProviderProtocolKey,
    turn: LlmAssistantTurn,
    reasoning_content: Option<&str>,
) -> AgentResult<LlmAssistantTurn> {
    let policy = reasoning_replay_policy(profile)?;
    let family = MoonshotFamily::from_profile_id(protocol.profile.id)?;
    let requires_reasoning = policy.replays_ordinary
        || (!turn.provider_tool_calls().is_empty() && policy.replays_tool_turns);
    let captured = match reasoning_content {
        Some(reasoning_content) if !policy.accepts_reasoning && !reasoning_content.is_empty() => {
            return Err(AgentError::structured(
                "provider_reasoning_forbidden",
                "Moonshot disabled-thinking 响应意外包含 reasoning_content。",
                json!({
                    "type": "providerReasoningBoundary",
                    "reason": "reasoningPresentWhileDisabled",
                    "recovery": "retryProviderRequest",
                }),
            ));
        }
        Some(_) | None if !requires_reasoning => return Ok(turn),
        Some(reasoning_content) if policy.accepts_reasoning => {
            CapturedReasoning::Present(reasoning_content)
        }
        Some(_) => {
            return Err(AgentError::new(
                "Moonshot reasoning_content 与当前 thinking policy 不一致。",
            ));
        }
        None if family == MoonshotFamily::K3 => CapturedReasoning::Absent,
        None => {
            return Err(AgentError::structured(
                "provider_reasoning_required",
                "Moonshot Preserved Thinking 响应缺少 reasoning_content。",
                json!({
                    "type": "providerReasoningBoundary",
                    "reason": "missingPreservedReasoning",
                    "recovery": "retryProviderRequest",
                }),
            ));
        }
    };
    let reasoning_bytes = match captured {
        CapturedReasoning::Absent => 0,
        CapturedReasoning::Present(reasoning_content) => reasoning_content.len(),
    };
    if reasoning_bytes
        .checked_add(2)
        .is_none_or(|encoded| encoded > MAX_PROVIDER_CONTINUATION_BYTES)
    {
        return Err(AgentError::new(format!(
            "Moonshot reasoning_content 连同协议标记超过 {} 字节上限。",
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
    let mut opaque = Vec::with_capacity(reasoning_bytes + 2);
    opaque.push(family.continuation_magic());
    match captured {
        CapturedReasoning::Absent => opaque.push(REASONING_FIELD_ABSENT),
        CapturedReasoning::Present(reasoning_content) => {
            opaque.push(REASONING_FIELD_PRESENT);
            opaque.extend_from_slice(reasoning_content.as_bytes());
        }
    }
    let continuation = ProviderContinuation::new(
        protocol.clone(),
        replay_scope,
        turn.digest(),
        vec![ProviderContinuationFragment::new(position, opaque)],
    )?;
    turn.with_provider_continuation(continuation)
}

#[cfg(test)]
pub(in crate::llm) fn reasoning_content<'a>(
    request_protocol: &ProviderProtocolKey,
    turn: &'a LlmAssistantTurn,
) -> AgentResult<Option<&'a str>> {
    Ok(match captured_reasoning(request_protocol, turn)? {
        Some(CapturedReasoning::Present(reasoning)) => Some(reasoning),
        Some(CapturedReasoning::Absent) | None => None,
    })
}

fn captured_reasoning<'a>(
    request_protocol: &ProviderProtocolKey,
    turn: &'a LlmAssistantTurn,
) -> AgentResult<Option<CapturedReasoning<'a>>> {
    let Some(continuation) = turn.provider_continuation() else {
        return Ok(None);
    };
    let turn_protocol = turn.provider_protocol().ok_or_else(|| {
        AgentError::new("Split-projection Assistant Turn 不能回放 Moonshot continuation。")
    })?;
    continuation.validate_for(turn_protocol, turn.digest())?;
    if continuation.provenance() != request_protocol {
        return Err(AgentError::new(
            "Moonshot continuation 与当前冻结 Provider 协议不一致。",
        ));
    }
    let expected_scope = if turn.provider_tool_calls().is_empty() {
        ProviderContinuationReplayScope::AssistantTurnV1
    } else {
        ProviderContinuationReplayScope::InteractionV1
    };
    if continuation.replay_scope() != expected_scope {
        return Err(AgentError::new(
            "Moonshot continuation replay scope 与 Assistant Turn 不一致。",
        ));
    }
    reasoning_fragment(continuation).map(Some)
}

fn reasoning_fragment(continuation: &ProviderContinuation) -> AgentResult<CapturedReasoning<'_>> {
    if continuation.provenance().dialect != ProviderProtocolDialect::OpenAiChatCompletions {
        return Err(AgentError::new(
            "Provider continuation 不是 Moonshot Chat reasoning。",
        ));
    }
    let family = MoonshotFamily::from_profile_id(continuation.provenance().profile.id)?;
    let [fragment] = continuation.fragments() else {
        return Err(AgentError::new(
            "Moonshot continuation 必须包含一个 reasoning fragment。",
        ));
    };
    if fragment.position()
        != ProviderContinuationPosition::new(0, ProviderContinuationAttachment::AssistantTurn)
    {
        return Err(AgentError::new(
            "Moonshot continuation reasoning fragment 位置无效。",
        ));
    }
    let Some((&version, encoded_reasoning)) = fragment.opaque().split_first() else {
        return Err(AgentError::new(
            "Moonshot continuation reasoning fragment 不能为空。",
        ));
    };
    if version != family.continuation_magic() {
        return Err(AgentError::new(
            "Moonshot continuation reasoning fragment 版本或 family 不受支持。",
        ));
    }
    let Some((&presence, raw_reasoning)) = encoded_reasoning.split_first() else {
        return Err(AgentError::new(
            "Moonshot continuation 缺少 reasoning_content presence 标记。",
        ));
    };
    match presence {
        REASONING_FIELD_ABSENT if raw_reasoning.is_empty() && family == MoonshotFamily::K3 => {
            Ok(CapturedReasoning::Absent)
        }
        REASONING_FIELD_ABSENT => Err(AgentError::new(
            "Moonshot continuation 的 absent reasoning 标记仅适用于 K3。",
        )),
        REASONING_FIELD_PRESENT => std::str::from_utf8(raw_reasoning)
            .map(CapturedReasoning::Present)
            .map_err(|_| {
                AgentError::new("Moonshot continuation 不是有效 UTF-8 reasoning_content。")
            }),
        _ => Err(AgentError::new(
            "Moonshot continuation reasoning_content presence 标记无效。",
        )),
    }
}

fn estimate_continuation_tokens(continuation: Option<&ProviderContinuation>) -> AgentResult<u64> {
    let Some(continuation) = continuation else {
        return Ok(0);
    };
    let CapturedReasoning::Present(reasoning_content) = reasoning_fragment(continuation)? else {
        return Ok(0);
    };
    let wire_projection = serde_json::to_string(&json!({
        "reasoning_content": reasoning_content
    }))
    .map_err(|error| {
        AgentError::new(format!(
            "Moonshot reasoning_content 无法投影为请求 JSON：{error}"
        ))
    })?;
    Ok(crate::context::ContextTextBudget::heuristic(u64::MAX).estimate(&wire_projection))
}

enum MoonshotWireMessage<'a> {
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

fn project_exchange<'a>(
    profile: &ProviderProfileConfig,
    request_protocol: &ProviderProtocolKey,
    messages: &'a [LlmMessage],
) -> AgentResult<Vec<MoonshotWireMessage<'a>>> {
    let replay_policy = reasoning_replay_policy(profile)?;
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
            projected.push(MoonshotWireMessage::Original(message));
            message_index += 1;
            continue;
        };

        if turn.provider_tool_calls().is_empty() {
            if replay_policy.replays_ordinary && turn.provider_protocol().is_some() {
                match captured_reasoning(request_protocol, turn) {
                    Ok(Some(CapturedReasoning::Present(_)))
                    | Ok(Some(CapturedReasoning::Absent)) => {}
                    Ok(None) => {
                        return Err(provider_context_boundary_required(
                            "missingOrdinaryContinuation",
                            message_index,
                        ));
                    }
                    Err(_) => {
                        return Err(provider_context_boundary_required(
                            "incompatibleOrdinaryContinuation",
                            message_index,
                        ));
                    }
                }
            }
            projected.push(MoonshotWireMessage::Assistant {
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
        if replay_policy.replays_tool_turns {
            match captured_reasoning(request_protocol, turn) {
                Ok(Some(CapturedReasoning::Present(_))) | Ok(Some(CapturedReasoning::Absent)) => {}
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

        projected.push(MoonshotWireMessage::Assistant {
            turn,
            provider_tool_calls,
        });
        projected.extend(tool_results.into_iter().map(|(message, provider_call_id)| {
            MoonshotWireMessage::ToolResult {
                message,
                provider_call_id,
            }
        }));
        for interstitial_message in interstitial {
            if let Some(interstitial_turn) = interstitial_message.assistant_turn() {
                projected.push(MoonshotWireMessage::Assistant {
                    turn: interstitial_turn,
                    provider_tool_calls: Vec::new(),
                });
            } else {
                projected.push(MoonshotWireMessage::Original(interstitial_message));
            }
        }
        message_index = search_index;
    }

    Ok(projected)
}

fn provider_context_boundary_required(reason: &'static str, message_index: usize) -> AgentError {
    AgentError::structured(
        "provider_context_boundary_required",
        "Moonshot 工具或 Preserved Thinking 历史缺少可安全回放的 Provider 上下文边界。",
        json!({
            "type": "provider_context_boundary",
            "reason": reason,
            "messageIndex": message_index,
            "recovery": "startNewProviderContext",
        }),
    )
}

fn provider_reasoning_boundary_required(reason: &'static str) -> AgentError {
    AgentError::structured(
        "provider_reasoning_boundary_required",
        "Moonshot reasoning history 无法按当前 family policy 安全回放。",
        json!({
            "type": "providerReasoningBoundary",
            "reason": reason,
            "recovery": "startNewProviderContext",
        }),
    )
}

/// Moonshot Chat Completion usage does not publish a reliable visible/reasoning breakdown.
/// Preserve provider-authoritative input and total counts, but never invent visible output.
fn project_usage(profile: &ProviderProfileConfig, mut usage: AgentUsage) -> AgentUsage {
    let disabled_k2_6 = matches!(
        profile.family_settings(),
        Some(ProviderFamilySettings::MoonshotK26Chat {
            thinking_mode: MoonshotK26ThinkingMode::Disabled,
        })
    );
    if disabled_k2_6 {
        usage.output_thinking_tokens = Some(0);
    } else {
        usage.output_tokens = None;
        usage.output_thinking_tokens = None;
    }
    usage
}

/// Independent Moonshot stream state. The inner accumulator is purely OpenAI-compatible framing;
/// private reasoning and usage semantics are owned here.
pub(in crate::llm) struct MoonshotStreamAccumulator {
    openai: OpenAiStreamAccumulator,
    reasoning_content: String,
    saw_reasoning_content: bool,
    raw_usage: Option<AgentUsage>,
    pub(in crate::llm) projected_usage: Option<AgentUsage>,
    profile: ProviderProfileConfig,
}

impl MoonshotStreamAccumulator {
    fn new(profile: &ProviderProfileConfig) -> AgentResult<Self> {
        let _ = reasoning_replay_policy(profile)?;
        Ok(Self {
            openai: OpenAiStreamAccumulator::default(),
            reasoning_content: String::new(),
            saw_reasoning_content: false,
            raw_usage: None,
            projected_usage: None,
            profile: profile.clone(),
        })
    }

    fn process(
        &mut self,
        value: &Value,
        on_delta: &mut dyn FnMut(LlmStreamEvent),
    ) -> AgentResult<()> {
        merge_stream_usage(&mut self.raw_usage, extract_usage(value));
        self.projected_usage = self
            .raw_usage
            .clone()
            .map(|usage| project_usage(&self.profile, usage));
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
                                AgentError::new("Moonshot reasoning_content 大小溢出。")
                            })?;
                        if next_length >= MAX_PROVIDER_CONTINUATION_BYTES {
                            return Err(AgentError::new(format!(
                                "Moonshot reasoning_content 连同协议版本标记超过 {} 字节上限。",
                                MAX_PROVIDER_CONTINUATION_BYTES,
                            )));
                        }
                        self.reasoning_content.push_str(reasoning);
                    }
                    _ => {
                        return Err(AgentError::new(
                            "Moonshot 流式响应中的 reasoning_content 不是字符串。",
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

/// Moonshot's documented error discriminator mapper. Message inspection is intentionally limited
/// to documented context-overflow forms (plus the retained legacy model-limit wording) that share
/// `invalid_request_error` with unrelated invalid requests.
pub(in crate::llm) fn classify_error(
    status: Option<u16>,
    provider_code: Option<&str>,
    body: &str,
) -> Option<LlmProviderFailureCategory> {
    let code = provider_code.map(str::to_ascii_lowercase);
    match code.as_deref() {
        Some("invalid_authentication_error" | "incorrect_api_key_error") => {
            Some(LlmProviderFailureCategory::Authentication)
        }
        Some("permission_denied_error") => Some(LlmProviderFailureCategory::PermissionDenied),
        Some("resource_not_found_error") => Some(LlmProviderFailureCategory::ResourceNotFound),
        Some("exceeded_current_quota_error") => Some(LlmProviderFailureCategory::QuotaExhausted),
        Some("rate_limit_reached_error") => Some(LlmProviderFailureCategory::RateLimited),
        Some(
            "engine_overloaded_error" | "server_error" | "server_unavailable" | "unexpected_output",
        ) => Some(LlmProviderFailureCategory::Overloaded),
        Some("client_closed_request") => Some(LlmProviderFailureCategory::Network),
        Some("invalid_request_error") if moonshot_context_limit_message(body) => {
            Some(LlmProviderFailureCategory::ContextTooLarge)
        }
        Some("invalid_request_error") => Some(LlmProviderFailureCategory::InvalidRequest),
        _ => match status {
            Some(400 | 405 | 415 | 422) => Some(LlmProviderFailureCategory::InvalidRequest),
            Some(401) => Some(LlmProviderFailureCategory::Authentication),
            Some(403) => Some(LlmProviderFailureCategory::PermissionDenied),
            Some(404) => Some(LlmProviderFailureCategory::ResourceNotFound),
            Some(413) => Some(LlmProviderFailureCategory::ContextTooLarge),
            Some(429) => Some(LlmProviderFailureCategory::RateLimited),
            Some(499) => Some(LlmProviderFailureCategory::Network),
            Some(500 | 502 | 503 | 504 | 529) => Some(LlmProviderFailureCategory::Overloaded),
            _ => None,
        },
    }
}

fn moonshot_context_limit_message(body: &str) -> bool {
    let Ok(value) = serde_json::from_str::<Value>(body) else {
        return false;
    };
    let message = value
        .pointer("/error/message")
        .or_else(|| value.get("message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    message.contains("input token length too long")
        || message.contains("prompt tokens + max_tokens exceeds the model specification")
        || message.contains("your request exceeded model token limit")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moonshot_error_codes_keep_vendor_semantics_distinct() {
        let cases = [
            (
                401,
                "invalid_authentication_error",
                LlmProviderFailureCategory::Authentication,
            ),
            (
                401,
                "incorrect_api_key_error",
                LlmProviderFailureCategory::Authentication,
            ),
            (
                403,
                "permission_denied_error",
                LlmProviderFailureCategory::PermissionDenied,
            ),
            (
                404,
                "resource_not_found_error",
                LlmProviderFailureCategory::ResourceNotFound,
            ),
            (
                429,
                "exceeded_current_quota_error",
                LlmProviderFailureCategory::QuotaExhausted,
            ),
            (
                429,
                "rate_limit_reached_error",
                LlmProviderFailureCategory::RateLimited,
            ),
            (
                429,
                "engine_overloaded_error",
                LlmProviderFailureCategory::Overloaded,
            ),
            (
                400,
                "server_unavailable",
                LlmProviderFailureCategory::Overloaded,
            ),
            (
                500,
                "client_closed_request",
                LlmProviderFailureCategory::Network,
            ),
            (
                400,
                "invalid_request_error",
                LlmProviderFailureCategory::InvalidRequest,
            ),
        ];
        for (status, code, expected) in cases {
            assert_eq!(
                classify_error(
                    Some(status),
                    Some(code),
                    &json!({ "error": { "type": code } }).to_string(),
                ),
                Some(expected),
            );
        }
        assert_eq!(
            classify_error(
                Some(400),
                Some("invalid_request_error"),
                r#"{"error":{"type":"invalid_request_error","message":"Input token length too long"}}"#,
            ),
            Some(LlmProviderFailureCategory::ContextTooLarge)
        );
        assert_eq!(
            classify_error(
                Some(400),
                Some("invalid_request_error"),
                r#"{"error":{"type":"invalid_request_error","message":"prompt tokens + max_tokens exceeds the model specification"}}"#,
            ),
            Some(LlmProviderFailureCategory::ContextTooLarge)
        );
        assert_eq!(
            classify_error(
                Some(400),
                Some("invalid_request_error"),
                r#"{"error":{"type":"invalid_request_error","message":"Your request exceeded model token limit : 1048576"}}"#,
            ),
            Some(LlmProviderFailureCategory::ContextTooLarge)
        );
    }
}
