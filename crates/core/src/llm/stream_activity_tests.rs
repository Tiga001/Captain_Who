use super::*;
use crate::provider_profile::{
    MoonshotK26ThinkingMode, ProviderFamilySettings, ProviderProfileRef, ProviderProtocolDialect,
    ProviderReasoningEffort, ProviderVendorId,
};

fn activities(events: &[LlmStreamEvent]) -> Vec<AgentModelActivity> {
    events
        .iter()
        .filter_map(|event| match event {
            LlmStreamEvent::ModelActivityChanged(activity) => Some(*activity),
            _ => None,
        })
        .collect()
}

fn consume(accumulator: &mut LlmStreamAccumulator, events: &mut Vec<LlmStreamEvent>, value: Value) {
    process_sse_frame(&format!("data: {value}"), accumulator, &mut |event| {
        events.push(event);
    })
    .unwrap();
}

fn chat_profiles() -> Vec<ProviderProfileConfig> {
    vec![
        ProviderProfileConfig::generic_for_dialect(ProviderProtocolDialect::OpenAiChatCompletions),
        ProviderProfileConfig::deepseek_flash_default(),
        ProviderProfileConfig::deepseek_pro_default(),
        ProviderProfileConfig::from_family_settings(
            ProviderProfileRef::moonshot_k3_chat(),
            ProviderVendorId::Moonshot,
            ProviderFamilySettings::MoonshotK3Chat {
                reasoning_effort: ProviderReasoningEffort::ProviderDefault,
            },
        ),
        ProviderProfileConfig::from_family_settings(
            ProviderProfileRef::moonshot_k2_7_code_chat(),
            ProviderVendorId::Moonshot,
            ProviderFamilySettings::MoonshotK27CodeChat,
        ),
        ProviderProfileConfig::from_family_settings(
            ProviderProfileRef::moonshot_k2_6_chat(),
            ProviderVendorId::Moonshot,
            ProviderFamilySettings::MoonshotK26Chat {
                thinking_mode: MoonshotK26ThinkingMode::Enabled,
            },
        ),
    ]
}

#[test]
fn chat_profiles_publish_reasoning_transitions_without_private_content_or_token_bursts() {
    const PRIVATE: &str = "private-reasoning-canary";
    for profile in chat_profiles() {
        let model = match profile.profile().id {
            ProviderProfileId::DeepSeekV41FlashChat => "deepseek-flash",
            ProviderProfileId::DeepSeekV4Pro0813Chat => "deepseek-v4-pro",
            ProviderProfileId::MoonshotK3Chat => "kimi-k3",
            ProviderProfileId::MoonshotK27CodeChat => "kimi-k2.7-code",
            ProviderProfileId::MoonshotK26Chat => "kimi-k2.6",
            _ => "test-model",
        };
        let protocol = ProviderProtocolKey::new(
            ProviderProtocolDialect::OpenAiChatCompletions,
            &profile,
            model,
            None,
        )
        .unwrap();
        let mut accumulator = LlmStreamAccumulator::for_profile(&profile, &protocol).unwrap();
        let mut events = Vec::new();
        for value in [
            json!({"choices":[{"delta":{"role":"assistant"}}]}),
            json!({"choices":[{"delta":{"reasoning_content":""}}]}),
            json!({"choices":[{"delta":{"reasoning_content":null}}]}),
            json!({"choices":[],"usage":{"completion_tokens_details":{"reasoning_tokens":2}}}),
        ] {
            consume(&mut accumulator, &mut events, value);
        }
        assert!(events.is_empty());
        for _ in 0..20 {
            consume(
                &mut accumulator,
                &mut events,
                json!({"choices":[{"delta":{"reasoning_content":PRIVATE}}]}),
            );
        }
        assert_eq!(activities(&events), [AgentModelActivity::Reasoning]);
        consume(
            &mut accumulator,
            &mut events,
            json!({"choices":[{"delta":{"content":"Visible answer"}}]}),
        );
        consume(
            &mut accumulator,
            &mut events,
            json!({"choices":[{"delta":{},"finish_reason":"stop"}]}),
        );
        assert_eq!(
            activities(&events),
            [AgentModelActivity::Reasoning, AgentModelActivity::Waiting]
        );
        let response = accumulator.finish().unwrap();
        assert_eq!(response.content(), "Visible answer");
        assert!(events.iter().all(|event| match event {
            LlmStreamEvent::Delta(text) => !text.contains(PRIVATE),
            _ => true,
        }));
    }
}

#[test]
fn chat_reasoning_ends_at_tool_start_finish_reason_or_done() {
    for boundary in [
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call-1\",\"function\":{\"name\":\"read_file\",\"arguments\":\"\"}}]}}]}",
        "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}",
        "data: [DONE]",
    ] {
        let mut accumulator = LlmStreamAccumulator::new(AgentApiStyle::OpenAiCompatible);
        let mut events = Vec::new();
        consume(
            &mut accumulator,
            &mut events,
            json!({"choices":[{"delta":{"reasoning_content":"private"}}]}),
        );
        process_sse_frame(boundary, &mut accumulator, &mut |event| events.push(event)).unwrap();
        assert_eq!(
            activities(&events),
            [AgentModelActivity::Reasoning, AgentModelActivity::Waiting]
        );
        assert!(events.iter().all(|event| !matches!(event, LlmStreamEvent::ToolInputProgress { .. })));
    }
}

#[test]
fn chat_unrecognized_reasoning_and_mixed_visible_chunks_do_not_start_reasoning() {
    let mut accumulator = LlmStreamAccumulator::new(AgentApiStyle::OpenAiCompatible);
    let mut events = Vec::new();
    for value in [
        json!({"type":"response.reasoning.delta","delta":"private"}),
        json!({"choices":[{"delta":{"reasoning":"unknown-field"}}]}),
        json!({"choices":[{"delta":{"reasoning_content":"private","content":"visible"}}]}),
        json!({"choices":[{"delta":{"reasoning_content":"private"},"finish_reason":"stop"}]}),
    ] {
        consume(&mut accumulator, &mut events, value);
    }
    assert!(activities(&events).is_empty());
}

#[test]
fn anthropic_thinking_blocks_dedupe_and_end_at_the_matching_block_stop() {
    let mut accumulator = LlmStreamAccumulator::new(AgentApiStyle::AnthropicCompatible);
    let mut events = Vec::new();
    consume(
        &mut accumulator,
        &mut events,
        json!({
            "type":"content_block_start","index":2,"content_block":{"type":"thinking","thinking":""}
        }),
    );
    for _ in 0..20 {
        consume(
            &mut accumulator,
            &mut events,
            json!({
                "type":"content_block_delta","index":2,"delta":{"type":"thinking_delta","thinking":"private-canary"}
            }),
        );
    }
    for value in [
        json!({"type":"ping"}),
        json!({"type":"content_block_delta","index":2,"delta":{"type":"signature_delta","signature":"private-signature"}}),
        json!({"type":"content_block_stop","index":1}),
    ] {
        consume(&mut accumulator, &mut events, value);
    }
    assert_eq!(activities(&events), [AgentModelActivity::Reasoning]);
    consume(
        &mut accumulator,
        &mut events,
        json!({"type":"content_block_stop","index":2}),
    );
    assert_eq!(
        activities(&events),
        [AgentModelActivity::Reasoning, AgentModelActivity::Waiting]
    );
    assert!(events
        .iter()
        .all(|event| !matches!(event, LlmStreamEvent::Delta(_))));
}

#[test]
fn anthropic_reasoning_ends_before_empty_text_tool_or_response_boundaries() {
    for boundary in [
        json!({"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}),
        json!({"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"tool-1","name":"read_file","input":{}}}),
        json!({"type":"message_delta","delta":{"stop_reason":"end_turn"}}),
        json!({"type":"message_stop"}),
    ] {
        let mut accumulator = LlmStreamAccumulator::new(AgentApiStyle::AnthropicCompatible);
        let mut events = Vec::new();
        consume(
            &mut accumulator,
            &mut events,
            json!({
                "type":"content_block_start","index":0,"content_block":{"type":"redacted_thinking","data":"private"}
            }),
        );
        consume(&mut accumulator, &mut events, boundary);
        assert_eq!(
            activities(&events),
            [AgentModelActivity::Reasoning, AgentModelActivity::Waiting]
        );
        assert!(events.iter().all(|event| !matches!(
            event,
            LlmStreamEvent::Delta(_) | LlmStreamEvent::ToolInputProgress { .. }
        )));
    }
}

#[test]
fn anthropic_thinking_activity_keeps_the_stream_alive_but_metadata_does_not() {
    for value in [
        json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"private"}}),
    ] {
        assert!(is_meaningful_model_activity(None, &value));
    }
    for value in [
        json!({"type":"ping"}),
        json!({"type":"message_start","message":{"role":"assistant"}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":""}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"private"}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"unknown","thinking":"private"}}),
    ] {
        assert!(!is_meaningful_model_activity(None, &value));
        let mut accumulator = LlmStreamAccumulator::new(AgentApiStyle::AnthropicCompatible);
        let mut events = Vec::new();
        consume(&mut accumulator, &mut events, value);
        assert!(activities(&events).is_empty());
    }
}
