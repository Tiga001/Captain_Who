use super::*;

#[test]
fn adapter_registry_selects_deepseek_only_for_the_explicit_profile() {
    let provider_profile = ProviderProfileConfig::deepseek_v4_default();
    let provider_protocol = ProviderProtocolKey::new(
        AgentApiStyle::OpenAiCompatible.into(),
        &provider_profile,
        "deepseek-v4",
        None,
    )
    .unwrap();
    let request = LlmChatRequest {
        api_url: "https://example.test/v1/chat/completions".to_string(),
        api_token: "token".to_string(),
        provider_profile,
        provider_protocol,
        max_tokens: 100,
        temperature: 0.2,
        stream: false,
        messages: vec![LlmMessage::text(LlmMessageRole::User, "hello")],
        tools: Vec::new(),
    };

    let adapter = ProviderAdapterRegistry::resolve(&request).unwrap();
    assert_eq!(adapter.profile(), ProviderProfileRef::deepseek_v4_chat());

    let mut mismatched = request;
    mismatched.provider_profile = generic_provider_profile(AgentApiStyle::OpenAiCompatible);
    assert!(ProviderAdapterRegistry::resolve(&mismatched).is_err());
}

#[test]
fn deepseek_disabled_reasoning_maps_without_generic_tool_choice() {
    let provider_profile =
        deepseek_provider_profile(ReasoningMode::Disabled, ReasoningEffort::ProviderDefault);
    let provider_protocol = deepseek_provider_protocol(&provider_profile, "deepseek-v4-flash");
    let request = LlmChatRequest {
        api_url: "https://api.deepseek.test/chat/completions".to_string(),
        api_token: "token".to_string(),
        provider_profile,
        provider_protocol,
        max_tokens: 512,
        temperature: 0.7,
        stream: false,
        messages: vec![LlmMessage::text(LlmMessageRole::User, "hello")],
        tools: vec![tool_definition()],
    };

    let payload = build_payload(&request);
    assert_eq!(payload["thinking"], json!({ "type": "disabled" }));
    assert!((payload["temperature"].as_f64().unwrap() - 0.7).abs() < f64::from(f32::EPSILON));
    assert!(payload.get("reasoning_effort").is_none());
    assert!(payload.get("tool_choice").is_none());
    assert_eq!(payload["tools"][0]["function"]["name"], "read_file");
}

#[test]
fn deepseek_reasoning_settings_map_to_the_exact_request_fields() {
    let cases = [
        (
            ReasoningMode::ProviderDefault,
            ReasoningEffort::ProviderDefault,
            None,
            None,
        ),
        (
            ReasoningMode::Enabled,
            ReasoningEffort::ProviderDefault,
            Some("enabled"),
            None,
        ),
        (
            ReasoningMode::Enabled,
            ReasoningEffort::High,
            Some("enabled"),
            Some("high"),
        ),
        (
            ReasoningMode::Enabled,
            ReasoningEffort::Max,
            Some("enabled"),
            Some("max"),
        ),
        (
            ReasoningMode::Disabled,
            ReasoningEffort::ProviderDefault,
            Some("disabled"),
            None,
        ),
    ];

    for (mode, effort, expected_thinking, expected_effort) in cases {
        let provider_profile = deepseek_provider_profile(mode, effort);
        let provider_protocol = deepseek_provider_protocol(&provider_profile, "deepseek-v4-flash");
        let request = LlmChatRequest {
            api_url: "https://api.deepseek.test/chat/completions".to_string(),
            api_token: "token".to_string(),
            provider_profile,
            provider_protocol,
            max_tokens: 512,
            temperature: 0.7,
            stream: false,
            messages: vec![LlmMessage::text(LlmMessageRole::User, "hello")],
            tools: vec![tool_definition()],
        };

        let payload = build_payload(&request);
        assert_eq!(
            payload
                .get("thinking")
                .and_then(|value| value.get("type"))
                .and_then(serde_json::Value::as_str),
            expected_thinking,
            "unexpected thinking mapping for {mode:?}/{effort:?}"
        );
        assert_eq!(
            payload
                .get("reasoning_effort")
                .and_then(serde_json::Value::as_str),
            expected_effort,
            "unexpected reasoning_effort mapping for {mode:?}/{effort:?}"
        );
        assert!(payload.get("tool_choice").is_none());
    }
}

#[test]
fn deepseek_nonstream_preserves_a_present_empty_reasoning_field() {
    let provider_profile = deepseek_provider_profile(ReasoningMode::Enabled, ReasoningEffort::High);
    let provider_protocol = deepseek_provider_protocol(&provider_profile, "deepseek-v4-flash");
    let response = parse_non_stream_response(
        &json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "Visible.",
                    "reasoning_content": ""
                },
                "finish_reason": "stop"
            }]
        })
        .to_string(),
        &provider_protocol,
        LlmResponseValidation::RequireModelAction,
    )
    .unwrap();

    assert_eq!(response.content(), "Visible.");
    assert!(response.assistant_turn.reasoning().is_empty());
    assert_eq!(
        deepseek_reasoning_content(&provider_protocol, &response.assistant_turn).unwrap(),
        Some("")
    );
    assert_eq!(
        response
            .assistant_turn
            .provider_continuation()
            .unwrap()
            .encoded_bytes(),
        1
    );
}

#[test]
fn deepseek_disabled_tool_response_without_reasoning_preserves_absence() {
    let provider_profile =
        deepseek_provider_profile(ReasoningMode::Disabled, ReasoningEffort::ProviderDefault);
    let provider_protocol = deepseek_provider_protocol(&provider_profile, "deepseek-v4-flash");
    let response = parse_non_stream_response_with_profile(
        &json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "",
                    "tool_calls": [{
                        "id": "non-thinking-deepseek-call",
                        "type": "function",
                        "function": {
                            "name": "read_file",
                            "arguments": "{\"path\":\"src/lib.rs\"}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        })
        .to_string(),
        &provider_profile,
        &provider_protocol,
        LlmResponseValidation::RequireModelAction,
    )
    .unwrap();

    assert!(response.assistant_turn.reasoning().is_empty());
    assert!(response.assistant_turn.provider_continuation().is_none());
    assert_eq!(
        deepseek_reasoning_content(&provider_protocol, &response.assistant_turn).unwrap(),
        None
    );
}

#[test]
fn deepseek_enabled_tool_response_missing_reasoning_fails_nonstream_and_stream() {
    let provider_profile = deepseek_provider_profile(ReasoningMode::Enabled, ReasoningEffort::High);
    let provider_protocol = deepseek_provider_protocol(&provider_profile, "deepseek-v4-pro");
    let missing_reasoning_response = json!({
        "choices": [{
            "message": {
                "role": "assistant",
                "content": "",
                "tool_calls": [{
                    "id": "missing-reasoning-call",
                    "type": "function",
                    "function": {
                        "name": "read_file",
                        "arguments": "{\"path\":\"src/lib.rs\"}"
                    }
                }]
            },
            "finish_reason": "tool_calls"
        }]
    });
    let nonstream_error = parse_non_stream_response_with_profile(
        &missing_reasoning_response.to_string(),
        &provider_profile,
        &provider_protocol,
        LlmResponseValidation::RequireModelAction,
    )
    .unwrap_err();
    assert_eq!(nonstream_error.code(), Some("provider_reasoning_required"));

    let mut accumulator =
        LlmStreamAccumulator::for_profile(&provider_profile, &provider_protocol).unwrap();
    let missing_reasoning_stream_frame = json!({
        "choices": [{
            "delta": {
                "tool_calls": [{
                    "index": 0,
                    "id": "missing-reasoning-call",
                    "type": "function",
                    "function": {
                        "name": "read_file",
                        "arguments": "{\"path\":\"src/lib.rs\"}"
                    }
                }]
            },
            "finish_reason": "tool_calls"
        }]
    });
    process_sse_frame(
        &format!("data: {missing_reasoning_stream_frame}\n\n"),
        &mut accumulator,
        &mut |_| {},
    )
    .unwrap();
    let stream_error = accumulator.finish().unwrap_err();
    assert_eq!(stream_error.code(), Some("provider_reasoning_required"));
}

#[test]
fn deepseek_provider_default_preserves_missing_and_explicit_empty_reasoning() {
    let provider_profile = ProviderProfileConfig::deepseek_v4_default();
    let provider_protocol = deepseek_provider_protocol(&provider_profile, "deepseek-v4-default");
    let response = |reasoning_content: Option<&str>| {
        let mut message = json!({
            "role": "assistant",
            "content": "",
            "tool_calls": [{
                "id": "default-reasoning-call",
                "type": "function",
                "function": {
                    "name": "read_file",
                    "arguments": "{\"path\":\"src/lib.rs\"}"
                }
            }]
        });
        if let Some(reasoning_content) = reasoning_content {
            message["reasoning_content"] = json!(reasoning_content);
        }
        parse_non_stream_response_with_profile(
            &json!({
                "choices": [{
                    "message": message,
                    "finish_reason": "tool_calls"
                }]
            })
            .to_string(),
            &provider_profile,
            &provider_protocol,
            LlmResponseValidation::RequireModelAction,
        )
        .unwrap()
    };

    let missing = response(None);
    assert!(missing.assistant_turn.provider_continuation().is_none());
    assert_eq!(
        deepseek_reasoning_content(&provider_protocol, &missing.assistant_turn).unwrap(),
        None
    );

    let explicit_empty = response(Some(""));
    assert!(explicit_empty
        .assistant_turn
        .provider_continuation()
        .is_some());
    assert_eq!(
        deepseek_reasoning_content(&provider_protocol, &explicit_empty.assistant_turn).unwrap(),
        Some("")
    );
}

#[test]
fn deepseek_disabled_reasoning_supports_multiple_tool_rounds_without_continuations() {
    let provider_profile =
        deepseek_provider_profile(ReasoningMode::Disabled, ReasoningEffort::ProviderDefault);
    let provider_protocol = deepseek_provider_protocol(&provider_profile, "deepseek-v4-disabled");
    let parse_turn = |provider_call_id: &str| {
        parse_non_stream_response_with_profile(
            &json!({
                "choices": [{
                    "message": {
                        "role": "assistant",
                        "content": "",
                        "tool_calls": [{
                            "id": provider_call_id,
                            "type": "function",
                            "function": {
                                "name": "read_file",
                                "arguments": "{\"path\":\"src/lib.rs\"}"
                            }
                        }]
                    },
                    "finish_reason": "tool_calls"
                }]
            })
            .to_string(),
            &provider_profile,
            &provider_protocol,
            LlmResponseValidation::RequireModelAction,
        )
        .unwrap()
        .assistant_turn
    };
    let bind_turn = |turn: LlmAssistantTurn, request_index: usize| {
        let provider_call = turn.provider_tool_calls()[0].clone();
        let runtime_call = LlmToolCall {
            id: model_response_tool_call_id(
                "deepseek-disabled-run",
                request_index,
                0,
                &provider_call.id,
            ),
            name: provider_call.name.clone(),
            args: provider_call.args.clone(),
        };
        (
            turn.with_runtime_tool_bindings(vec![LlmRuntimeToolCallBinding::new(
                0,
                &provider_call,
                runtime_call.clone(),
            )])
            .unwrap(),
            runtime_call,
        )
    };
    let (first_turn, first_runtime_call) = bind_turn(parse_turn("disabled-provider-1"), 0);
    let (second_turn, second_runtime_call) = bind_turn(parse_turn("disabled-provider-2"), 1);
    assert!(first_turn.provider_continuation().is_none());
    assert!(second_turn.provider_continuation().is_none());

    let request = LlmChatRequest {
        api_url: "https://api.deepseek.test/chat/completions".to_string(),
        api_token: "token".to_string(),
        provider_profile,
        provider_protocol,
        max_tokens: 512,
        temperature: 0.7,
        stream: false,
        messages: vec![
            LlmMessage::text(LlmMessageRole::User, "inspect"),
            LlmMessage::from_assistant_turn(first_turn),
            LlmMessage::tool_result(first_runtime_call.id, "first", false),
            LlmMessage::from_assistant_turn(second_turn),
            LlmMessage::tool_result(second_runtime_call.id, "second", false),
        ],
        tools: vec![tool_definition()],
    };
    let payload = build_payload(&request);
    let tool_turns = payload["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["role"] == "assistant" && message.get("tool_calls").is_some())
        .collect::<Vec<_>>();
    assert_eq!(tool_turns.len(), 2);
    assert!(tool_turns
        .iter()
        .all(|message| message.get("reasoning_content").is_none()));
    assert_eq!(tool_turns[0]["tool_calls"][0]["id"], "disabled-provider-1");
    assert_eq!(tool_turns[1]["tool_calls"][0]["id"], "disabled-provider-2");
}

#[test]
fn deepseek_stream_captures_reasoning_without_emitting_it_as_visible_text() {
    let provider_profile = deepseek_provider_profile(ReasoningMode::Enabled, ReasoningEffort::High);
    let provider_protocol = deepseek_provider_protocol(&provider_profile, "deepseek-v4-pro");
    let mut accumulator = LlmStreamAccumulator::for_protocol(&provider_protocol).unwrap();
    let mut events = Vec::new();

    for frame in [
        json!({
            "choices": [{ "delta": { "reasoning_content": "private " } }]
        }),
        json!({
            "choices": [{
                "delta": {
                    "reasoning_content": "reasoning",
                    "content": "Checking. ",
                    "tool_calls": [{
                        "index": 0,
                        "id": "deepseek-provider-call-1",
                        "function": {
                            "name": "read_file",
                            "arguments": "{\"path\":\"src/lib.rs\"}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        }),
        json!({
            "choices": [],
            "usage": {
                "prompt_tokens": 11,
                "completion_tokens": 7,
                "total_tokens": 18,
                "prompt_cache_hit_tokens": 3,
                "prompt_cache_miss_tokens": 8,
                "completion_tokens_details": { "reasoning_tokens": 5 }
            }
        }),
    ] {
        process_sse_frame(
            &format!("data: {frame}\n\n"),
            &mut accumulator,
            &mut |event| events.push(event),
        )
        .unwrap();
    }

    let response = accumulator.finish().unwrap();
    assert_eq!(joined_text(&events), "Checking. ");
    assert!(events.iter().all(|event| !matches!(
        event,
        LlmStreamEvent::Delta(delta) if delta.contains("private") || delta.contains("reasoning")
    )));
    assert_eq!(
        response.provider_tool_calls()[0].id,
        "deepseek-provider-call-1"
    );
    assert!(response.assistant_turn.reasoning().is_empty());
    assert_eq!(
        deepseek_reasoning_content(&provider_protocol, &response.assistant_turn).unwrap(),
        Some("private reasoning")
    );
    assert_eq!(
        response
            .assistant_turn
            .provider_continuation()
            .unwrap()
            .replay_scope(),
        ProviderContinuationReplayScope::InteractionV1
    );
    let usage = response.usage.unwrap();
    assert_eq!(usage.input_tokens, Some(11));
    assert_eq!(usage.output_tokens, Some(2));
    assert_eq!(usage.output_thinking_tokens, Some(5));
    assert_eq!(usage.total_tokens, Some(18));
    assert_eq!(usage.cached_input_tokens, Some(3));
    assert_eq!(usage.cache_creation_input_tokens, Some(8));
}

#[test]
fn deepseek_usage_fails_closed_when_visible_output_cannot_be_split() {
    let profile = deepseek_provider_profile(ReasoningMode::Enabled, ReasoningEffort::High);
    let protocol = deepseek_provider_protocol(&profile, "deepseek-v4-pro");

    let missing_details = parse_non_stream_response(
        &json!({
            "choices": [{
                "message": { "role": "assistant", "content": "visible" },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 7,
                "total_tokens": 17
            }
        })
        .to_string(),
        &protocol,
        LlmResponseValidation::RequireModelAction,
    )
    .unwrap()
    .usage
    .unwrap();
    assert_eq!(missing_details.output_tokens, None);
    assert_eq!(missing_details.output_thinking_tokens, None);
    assert_eq!(missing_details.total_tokens, Some(17));

    let inconsistent_details = parse_non_stream_response(
        &json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "visible",
                    "reasoning_content": "private"
                },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 3,
                "completion_tokens_details": { "reasoning_tokens": 5 },
                "total_tokens": 13
            }
        })
        .to_string(),
        &protocol,
        LlmResponseValidation::RequireModelAction,
    )
    .unwrap()
    .usage
    .unwrap();
    assert_eq!(inconsistent_details.output_tokens, None);
    assert_eq!(inconsistent_details.output_thinking_tokens, Some(5));
    assert_eq!(inconsistent_details.total_tokens, Some(13));

    let generic_protocol =
        generic_provider_protocol(AgentApiStyle::OpenAiCompatible, "generic-openai");
    let generic = parse_non_stream_response(
        &json!({
            "choices": [{
                "message": { "role": "assistant", "content": "visible" },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 7,
                "completion_tokens_details": { "reasoning_tokens": 5 },
                "total_tokens": 17
            }
        })
        .to_string(),
        &generic_protocol,
        LlmResponseValidation::RequireModelAction,
    )
    .unwrap()
    .usage
    .unwrap();
    assert_eq!(generic.output_tokens, Some(7));
    assert_eq!(generic.output_thinking_tokens, Some(5));
    assert_eq!(generic.total_tokens, Some(17));
}

#[test]
fn deepseek_continuation_estimate_measures_replayed_json_and_blocks_local_overflow() {
    let profile = deepseek_provider_profile(ReasoningMode::Enabled, ReasoningEffort::Max);
    let protocol = deepseek_provider_protocol(&profile, "deepseek-v4-pro");
    let reasoning = "大段中文 reasoning：\n\"quoted\" \\\\ slash\t".repeat(1_500);
    let response = parse_non_stream_response(
        &json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "",
                    "reasoning_content": reasoning,
                    "tool_calls": [{
                        "id": "deepseek-estimate-provider-call",
                        "type": "function",
                        "function": {
                            "name": "read_file",
                            "arguments": "{\"path\":\"large.txt\"}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        })
        .to_string(),
        &protocol,
        LlmResponseValidation::RequireModelAction,
    )
    .unwrap();
    let provider_call = response.provider_tool_calls()[0].clone();
    let runtime_call = LlmToolCall {
        id: model_response_tool_call_id("deepseek-estimate-run", 0, 0, &provider_call.id),
        name: provider_call.name.clone(),
        args: provider_call.args.clone(),
    };
    let turn = response
        .assistant_turn
        .with_runtime_tool_bindings(vec![LlmRuntimeToolCallBinding::new(
            0,
            &provider_call,
            runtime_call.clone(),
        )])
        .unwrap();
    let estimated = estimate_assistant_turn_continuation_tokens(&turn).unwrap();
    assert!(
        estimated > u64::try_from(reasoning.len()).unwrap().div_ceil(4),
        "Unicode and JSON escaping must not fall back to opaque bytes/4"
    );

    let mut frame = ContextFrame::new(vec![
        ContextItem::new(
            LlmMessage::from_assistant_turn(turn),
            ContextMetadata::new(
                ContextSource::ModelResponse,
                ContextScope::Run,
                ContextRetention::Retained,
            ),
        ),
        ContextItem::tool_result(
            runtime_call.id,
            "result",
            false,
            ContextMetadata::new(
                ContextSource::ToolResult,
                ContextScope::Run,
                ContextRetention::Retained,
            ),
        ),
    ]);
    let detector =
        ContextCapacityDetector::for_model("deepseek-v4-pro", AgentApiStyle::OpenAiCompatible, &[]);
    let report = detector.inspect(&mut frame, Some(4_096), 512);
    assert!(report.usage.breakdown.total.provider_continuation_tokens >= estimated);
    let error = detector.ensure_sendable(report).unwrap_err();
    assert_eq!(error.code(), Some("context_capacity_exceeded"));
    assert!(error.to_string().contains("请求尚未发送"));
}

#[test]
fn deepseek_groups_tool_history_and_moves_interstitial_context_after_results() {
    let provider_profile = deepseek_provider_profile(ReasoningMode::Enabled, ReasoningEffort::High);
    let provider_protocol = deepseek_provider_protocol(&provider_profile, "deepseek-v4-pro");
    let response = parse_non_stream_response(
        &json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "",
                    "reasoning_content": "Both files are required.",
                    "tool_calls": [
                        {
                            "id": "deepseek-raw-a",
                            "type": "function",
                            "function": {
                                "name": "read_file",
                                "arguments": "{\"path\":\"a.png\"}"
                            }
                        },
                        {
                            "id": "deepseek-raw-b",
                            "type": "function",
                            "function": {
                                "name": "read_file",
                                "arguments": "{\"path\":\"b.txt\"}"
                            }
                        }
                    ]
                },
                "finish_reason": "tool_calls"
            }]
        })
        .to_string(),
        &provider_protocol,
        LlmResponseValidation::RequireModelAction,
    )
    .unwrap();
    let provider_calls = response.provider_tool_calls().to_vec();
    let runtime_calls = provider_calls
        .iter()
        .enumerate()
        .map(|(index, call)| LlmToolCall {
            id: model_response_tool_call_id("deepseek-interstitial-run", 0, index, &call.id),
            name: call.name.clone(),
            args: call.args.clone(),
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
    let turn = response
        .assistant_turn
        .with_runtime_tool_bindings(bindings)
        .unwrap();
    let mut image_context = LlmMessage::text(LlmMessageRole::User, "First tool image");
    image_context.images_mut().unwrap().push(LlmImage {
        mime_type: "image/png".to_string(),
        data_base64: "AA==".to_string(),
    });
    let extension_context = LlmMessage::text(
        LlmMessageRole::Assistant,
        "Runtime extension state updated.",
    );
    let request = LlmChatRequest {
        api_url: "https://api.deepseek.test/chat/completions".to_string(),
        api_token: "token".to_string(),
        provider_profile,
        provider_protocol,
        max_tokens: 512,
        temperature: 0.0,
        stream: false,
        messages: vec![
            LlmMessage::text(LlmMessageRole::User, "Read both files"),
            LlmMessage::from_assistant_turn(turn),
            LlmMessage::tool_result(runtime_calls[0].id.clone(), "first", false),
            image_context,
            extension_context,
            LlmMessage::tool_result(runtime_calls[1].id.clone(), "second", false),
        ],
        tools: vec![tool_definition()],
    };

    validate_request(&request).unwrap();
    let payload = build_payload(&request);
    let messages = payload["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 6);
    assert_eq!(messages[1]["role"], "assistant");
    assert_eq!(messages[1]["reasoning_content"], "Both files are required.");
    assert_eq!(messages[1]["tool_calls"].as_array().unwrap().len(), 2);
    assert_eq!(messages[1]["tool_calls"][0]["id"], "deepseek-raw-a");
    assert_eq!(messages[1]["tool_calls"][1]["id"], "deepseek-raw-b");
    assert_eq!(messages[2]["tool_call_id"], "deepseek-raw-a");
    assert_eq!(messages[3]["tool_call_id"], "deepseek-raw-b");
    assert_eq!(messages[4]["role"], "user");
    assert_eq!(messages[4]["content"][1]["type"], "image_url");
    assert_eq!(messages[5]["content"], "Runtime extension state updated.");
    assert!(messages[5].get("reasoning_content").is_none());
}

#[test]
fn deepseek_tool_history_without_a_compatible_continuation_requires_a_boundary() {
    let provider_profile = deepseek_provider_profile(ReasoningMode::Enabled, ReasoningEffort::High);
    let provider_protocol = deepseek_provider_protocol(&provider_profile, "deepseek-v4-pro");
    let provider_call = LlmToolCall {
        id: "legacy-provider-call".to_string(),
        name: "read_file".to_string(),
        args: json!({"path":"src/lib.rs"}),
    };
    let runtime_call = LlmToolCall {
        id: model_response_tool_call_id("deepseek-boundary-run", 0, 0, &provider_call.id),
        name: provider_call.name.clone(),
        args: provider_call.args.clone(),
    };
    let split_turn = LlmAssistantTurn::from_split_projection("", vec![provider_call.clone()])
        .with_runtime_tool_bindings(vec![LlmRuntimeToolCallBinding::new(
            0,
            &provider_call,
            runtime_call.clone(),
        )])
        .unwrap();
    let request = LlmChatRequest {
        api_url: "https://api.deepseek.test/chat/completions".to_string(),
        api_token: "token".to_string(),
        provider_profile,
        provider_protocol,
        max_tokens: 512,
        temperature: 0.0,
        stream: false,
        messages: vec![
            LlmMessage::from_assistant_turn(split_turn),
            LlmMessage::tool_result(runtime_call.id, "result", false),
        ],
        tools: vec![tool_definition()],
    };

    let error = validate_request(&request).unwrap_err();
    assert_eq!(error.code(), Some("provider_context_boundary_required"));
    assert_eq!(
        error
            .details()
            .and_then(|details| details["reason"].as_str()),
        Some("missingToolBearingContinuation")
    );
}

#[test]
fn deepseek_tool_history_from_another_frozen_key_requires_a_boundary() {
    let provider_profile = deepseek_provider_profile(ReasoningMode::Enabled, ReasoningEffort::High);
    let prior_protocol = deepseek_provider_protocol(&provider_profile, "deepseek-v4-flash");
    let current_protocol = deepseek_provider_protocol(&provider_profile, "deepseek-v4-pro");
    let prior_response = parse_non_stream_response(
        &json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "",
                    "reasoning_content": "Prior model reasoning.",
                    "tool_calls": [{
                        "id": "prior-deepseek-call",
                        "type": "function",
                        "function": {
                            "name": "read_file",
                            "arguments": "{\"path\":\"src/lib.rs\"}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        })
        .to_string(),
        &prior_protocol,
        LlmResponseValidation::RequireModelAction,
    )
    .unwrap();
    let provider_call = prior_response.provider_tool_calls()[0].clone();
    let runtime_call = LlmToolCall {
        id: model_response_tool_call_id("deepseek-cross-key-run", 0, 0, &provider_call.id),
        name: provider_call.name.clone(),
        args: provider_call.args.clone(),
    };
    let prior_turn = prior_response
        .assistant_turn
        .with_runtime_tool_bindings(vec![LlmRuntimeToolCallBinding::new(
            0,
            &provider_call,
            runtime_call.clone(),
        )])
        .unwrap();
    let request = LlmChatRequest {
        api_url: "https://api.deepseek.test/chat/completions".to_string(),
        api_token: "token".to_string(),
        provider_profile,
        provider_protocol: current_protocol,
        max_tokens: 512,
        temperature: 0.0,
        stream: false,
        messages: vec![
            LlmMessage::from_assistant_turn(prior_turn),
            LlmMessage::tool_result(runtime_call.id, "result", false),
        ],
        tools: vec![tool_definition()],
    };

    let error = validate_request(&request).unwrap_err();
    assert_eq!(error.code(), Some("provider_context_boundary_required"));
    assert_eq!(
        error
            .details()
            .and_then(|details| details["reason"].as_str()),
        Some("incompatibleToolBearingContinuation")
    );
}

#[test]
fn deepseek_does_not_replay_reasoning_from_an_ordinary_prior_model_turn() {
    const PRIOR_REASONING: &str = "PRIOR_NO_TOOL_REASONING_MUST_NOT_REPLAY";
    let provider_profile = deepseek_provider_profile(ReasoningMode::Enabled, ReasoningEffort::High);
    let prior_protocol = deepseek_provider_protocol(&provider_profile, "deepseek-v4-flash");
    let current_protocol = deepseek_provider_protocol(&provider_profile, "deepseek-v4-pro");
    let prior_response = parse_non_stream_response(
        &json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "Prior visible answer.",
                    "reasoning_content": PRIOR_REASONING
                },
                "finish_reason": "stop"
            }]
        })
        .to_string(),
        &prior_protocol,
        LlmResponseValidation::RequireModelAction,
    )
    .unwrap();
    let request = LlmChatRequest {
        api_url: "https://api.deepseek.test/chat/completions".to_string(),
        api_token: "token".to_string(),
        provider_profile,
        provider_protocol: current_protocol,
        max_tokens: 512,
        temperature: 0.0,
        stream: false,
        messages: vec![
            LlmMessage::from_assistant_turn(prior_response.assistant_turn),
            LlmMessage::text(LlmMessageRole::User, "New user turn"),
        ],
        tools: Vec::new(),
    };

    validate_request(&request).unwrap();
    let encoded = serde_json::to_string(&build_payload(&request)).unwrap();
    assert!(!encoded.contains(PRIOR_REASONING));
    assert!(!encoded.contains("reasoning_content"));
}
