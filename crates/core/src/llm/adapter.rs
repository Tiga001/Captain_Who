//! Code-owned provider protocol adapters.
//!
//! A frozen [`ProviderProtocolKey`] selects an exact adapter. The API-style enum is only a
//! transport compatibility projection and is deliberately not an adapter registry key.

use super::payload::{
    build_anthropic_headers, build_anthropic_payload, build_openai_headers, build_openai_payload,
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
    ProviderProfileId, ProviderProfileRef, ProviderProtocolDialect, ProviderProtocolKey,
    GENERIC_ANTHROPIC_MESSAGES_PROFILE_VERSION, GENERIC_OPENAI_CHAT_PROFILE_VERSION,
};
use reqwest::header::HeaderMap;
use serde_json::Value;

pub(super) trait ProviderAdapter: Sync {
    fn profile(&self) -> ProviderProfileRef;

    fn dialect(&self) -> ProviderProtocolDialect;

    fn validate_profile_settings(&self, request: &LlmChatRequest) -> AgentResult<()>;

    fn validate_wire_protocol(&self, request: &LlmChatRequest) -> AgentResult<()>;

    fn prepare_request(&self, request: &LlmChatRequest) -> AgentResult<Value>;

    fn build_headers(&self, api_token: &str) -> AgentResult<HeaderMap>;

    fn parse_non_streaming_response(
        &self,
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

static GENERIC_OPENAI_ADAPTER: GenericOpenAiAdapter = GenericOpenAiAdapter;
static GENERIC_ANTHROPIC_ADAPTER: GenericAnthropicAdapter = GenericAnthropicAdapter;
static PROVIDER_ADAPTERS: [&'static dyn ProviderAdapter; 2] =
    [&GENERIC_OPENAI_ADAPTER, &GENERIC_ANTHROPIC_ADAPTER];

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
        protocol
            .validate()
            .map_err(|error| AgentError::new(format!("Provider protocol key 无效：{error}")))?;
        let adapter = PROVIDER_ADAPTERS
            .iter()
            .copied()
            .find(|adapter| {
                adapter.profile() == protocol.profile && adapter.dialect() == protocol.dialect
            })
            .ok_or_else(|| {
                AgentError::new(format!(
                    "当前版本没有注册 {:?} v{} / {:?} 的 Provider Adapter。",
                    protocol.profile.id, protocol.profile.version, protocol.dialect
                ))
            })?;
        Ok(adapter)
    }
}

impl ProviderAdapter for GenericOpenAiAdapter {
    fn profile(&self) -> ProviderProfileRef {
        ProviderProfileRef {
            id: ProviderProfileId::GenericOpenAiChat,
            version: GENERIC_OPENAI_CHAT_PROFILE_VERSION,
        }
    }

    fn dialect(&self) -> ProviderProtocolDialect {
        ProviderProtocolDialect::OpenAiChatCompletions
    }

    fn validate_profile_settings(&self, request: &LlmChatRequest) -> AgentResult<()> {
        validate_generic_profile_settings(self, request)
    }

    fn validate_wire_protocol(&self, request: &LlmChatRequest) -> AgentResult<()> {
        validate_legacy_generic_wire_protocol(&request.messages)
    }

    fn prepare_request(&self, request: &LlmChatRequest) -> AgentResult<Value> {
        build_openai_payload(request)
    }

    fn build_headers(&self, api_token: &str) -> AgentResult<HeaderMap> {
        build_openai_headers(api_token)
    }

    fn parse_non_streaming_response(
        &self,
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
    fn profile(&self) -> ProviderProfileRef {
        ProviderProfileRef {
            id: ProviderProfileId::GenericAnthropicMessages,
            version: GENERIC_ANTHROPIC_MESSAGES_PROFILE_VERSION,
        }
    }

    fn dialect(&self) -> ProviderProtocolDialect {
        ProviderProtocolDialect::AnthropicMessages
    }

    fn validate_profile_settings(&self, request: &LlmChatRequest) -> AgentResult<()> {
        validate_generic_profile_settings(self, request)
    }

    fn validate_wire_protocol(&self, request: &LlmChatRequest) -> AgentResult<()> {
        validate_legacy_generic_wire_protocol(&request.messages)
    }

    fn prepare_request(&self, request: &LlmChatRequest) -> AgentResult<Value> {
        build_anthropic_payload(request)
    }

    fn build_headers(&self, api_token: &str) -> AgentResult<HeaderMap> {
        build_anthropic_headers(api_token)
    }

    fn parse_non_streaming_response(
        &self,
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

pub(super) fn project_legacy_generic_exchange(
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

fn validate_legacy_generic_wire_protocol(messages: &[LlmMessage]) -> AgentResult<()> {
    let projected = project_legacy_generic_exchange(messages)?;
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
        let turn = LlmAssistantTurn::from_legacy("Inspecting both files.", provider_calls)
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

        let projected = project_legacy_generic_exchange(&messages).unwrap();

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
        let turn = LlmAssistantTurn::from_legacy("Reading the retained file.", provider_calls)
            .with_runtime_tool_bindings(vec![binding])
            .unwrap();
        let messages = vec![
            LlmMessage::from_assistant_turn(turn),
            LlmMessage::text(LlmMessageRole::User, "Interstitial runtime context"),
            LlmMessage::tool_result(runtime_call.id.clone(), "retained result", false),
        ];

        let projected = project_legacy_generic_exchange(&messages).unwrap();

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
