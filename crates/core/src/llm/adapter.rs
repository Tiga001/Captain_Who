//! Code-owned provider protocol adapters.
//!
//! A frozen [`ProviderProtocolKey`] selects an exact adapter. The API-style enum is only a
//! transport compatibility projection and is deliberately not an adapter registry key.

use super::payload::{
    build_anthropic_headers, build_anthropic_payload, build_openai_headers, build_openai_payload,
};
use super::provider_error::ProviderErrorClassification;
use super::providers::deepseek::{DEEPSEEK_V4_CHAT_ADAPTER, DEEPSEEK_V4_VISION_ADAPTER};
use super::providers::moonshot::{
    MOONSHOT_K2_6_CHAT_ADAPTER, MOONSHOT_K2_7_CODE_CHAT_ADAPTER, MOONSHOT_K3_CHAT_ADAPTER,
};
use super::response::{
    extract_anthropic_tool_calls, extract_openai_tool_calls, extract_response_text,
};
use super::stream::{AnthropicStreamAccumulator, OpenAiStreamAccumulator, ProviderStreamState};
use super::{
    LlmAssistantTurn, LlmChatRequest, LlmChatResponse, LlmMessage, LlmStreamEvent, LlmToolCall,
    ProviderContinuation,
};
use crate::protocol::{AgentError, AgentResult, AgentUsage};
use crate::provider_profile::{
    ProviderProfileConfig, ProviderProfileRef, ProviderProtocolDialect, ProviderProtocolKey,
};
use crate::provider_registration::{
    resolve_provider_registration_for_key, ProviderAdapterKind, ProviderRegistration,
    ProviderRuntimeCapabilities, GENERIC_ANTHROPIC_MESSAGES_REGISTRATION,
    GENERIC_OPENAI_CHAT_REGISTRATION,
};
use reqwest::header::HeaderMap;
use serde_json::Value;

#[cfg(test)]
pub(super) use super::providers::deepseek::reasoning_content as deepseek_reasoning_content;

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

    /// Selects the error vocabulary owned by this exact frozen Adapter.
    ///
    /// Generic adapters retain dialect-level compatibility classification. Vendor adapters make
    /// their own mapper authoritative, including an explicit `Unknown` result when a future or
    /// unrecognized vendor discriminator is encountered.
    fn classify_provider_error(
        &self,
        _status: Option<u16>,
        _provider_code: Option<&str>,
        _body: &str,
    ) -> ProviderErrorClassification {
        ProviderErrorClassification::DialectDefault
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

    fn new_stream_state(&self, profile: &ProviderProfileConfig)
        -> AgentResult<ProviderStreamState>;

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

    /// Reports whether an otherwise empty response contains a provider-private model action.
    ///
    /// The shared transport cannot inspect opaque continuations. Each vendor adapter therefore
    /// owns the decision using its exact continuation codec. Generic adapters deliberately retain
    /// the default `false` behavior.
    fn has_private_model_action(
        &self,
        _protocol: &ProviderProtocolKey,
        _turn: &LlmAssistantTurn,
    ) -> AgentResult<bool> {
        Ok(false)
    }

    fn project_usage(&self, profile: &ProviderProfileConfig, value: &Value) -> Option<AgentUsage>;

    fn estimate_continuation_tokens(
        &self,
        continuation: Option<&ProviderContinuation>,
    ) -> AgentResult<u64>;
}

struct GenericOpenAiAdapter;
struct GenericAnthropicAdapter;

static GENERIC_OPENAI_ADAPTER: GenericOpenAiAdapter = GenericOpenAiAdapter;
static GENERIC_ANTHROPIC_ADAPTER: GenericAnthropicAdapter = GenericAnthropicAdapter;

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
            ProviderAdapterKind::DeepSeekV4Vision => &DEEPSEEK_V4_VISION_ADAPTER,
            ProviderAdapterKind::MoonshotK3Chat => &MOONSHOT_K3_CHAT_ADAPTER,
            ProviderAdapterKind::MoonshotK27CodeChat => &MOONSHOT_K2_7_CODE_CHAT_ADAPTER,
            ProviderAdapterKind::MoonshotK26Chat => &MOONSHOT_K2_6_CHAT_ADAPTER,
        };
        debug_assert!(std::ptr::eq(adapter.registration(), registration));
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

    fn new_stream_state(
        &self,
        _profile: &ProviderProfileConfig,
    ) -> AgentResult<ProviderStreamState> {
        Ok(ProviderStreamState::OpenAi(
            OpenAiStreamAccumulator::default(),
        ))
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

    fn project_usage(&self, _profile: &ProviderProfileConfig, value: &Value) -> Option<AgentUsage> {
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

    fn new_stream_state(
        &self,
        _profile: &ProviderProfileConfig,
    ) -> AgentResult<ProviderStreamState> {
        Ok(ProviderStreamState::Anthropic(
            AnthropicStreamAccumulator::default(),
        ))
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

    fn project_usage(&self, _profile: &ProviderProfileConfig, value: &Value) -> Option<AgentUsage> {
        crate::usage::extract_usage(value)
    }

    fn estimate_continuation_tokens(
        &self,
        continuation: Option<&ProviderContinuation>,
    ) -> AgentResult<u64> {
        estimate_generic_continuation_tokens(continuation)
    }
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

pub(super) fn stream_state_mismatch() -> AgentError {
    AgentError::new("Provider Adapter 收到了其他协议的 stream state。")
}

/// Provider-neutral view used by Generic adapters to preserve the pre-upgrade wire sequence.
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
