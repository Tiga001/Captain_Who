use super::*;
use crate::provider_profile::{
    MoonshotK26ThinkingMode, ProviderFamilySettings, ProviderReasoningEffort, ProviderVendorId,
};

fn k3_profile(effort: ProviderReasoningEffort) -> ProviderProfileConfig {
    ProviderProfileConfig::from_family_settings(
        ProviderProfileRef::moonshot_k3_chat(),
        ProviderVendorId::Moonshot,
        ProviderFamilySettings::MoonshotK3Chat {
            reasoning_effort: effort,
        },
    )
}

fn k27_profile() -> ProviderProfileConfig {
    ProviderProfileConfig::from_family_settings(
        ProviderProfileRef::moonshot_k2_7_code_chat(),
        ProviderVendorId::Moonshot,
        ProviderFamilySettings::MoonshotK27CodeChat,
    )
}

fn k26_profile(thinking_mode: MoonshotK26ThinkingMode) -> ProviderProfileConfig {
    ProviderProfileConfig::from_family_settings(
        ProviderProfileRef::moonshot_k2_6_chat(),
        ProviderVendorId::Moonshot,
        ProviderFamilySettings::MoonshotK26Chat { thinking_mode },
    )
}

fn protocol(profile: &ProviderProfileConfig, model: &str) -> ProviderProtocolKey {
    ProviderProtocolKey::new(AgentApiStyle::OpenAiCompatible.into(), profile, model, None).unwrap()
}

fn request(
    profile: ProviderProfileConfig,
    model: &str,
    messages: Vec<LlmMessage>,
    tools: Vec<AgentToolDefinition>,
) -> LlmChatRequest {
    let provider_protocol = protocol(&profile, model);
    LlmChatRequest {
        api_url: "https://api.moonshot.test/v1/chat/completions".to_string(),
        api_token: "test-token".to_string(),
        provider_profile: profile,
        provider_protocol,
        max_tokens: 128 * 1024,
        temperature: 0.37,
        stream: false,
        messages,
        tools,
    }
}

fn moonshot_single_tool_response(
    profile: &ProviderProfileConfig,
    provider_protocol: &ProviderProtocolKey,
    reasoning_content: &str,
    provider_call_id: &str,
) -> LlmChatResponse {
    parse_non_stream_response_with_profile(
        &json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "",
                    "reasoning_content": reasoning_content,
                    "tool_calls": [{
                        "id": provider_call_id,
                        "type": "function",
                        "function": {
                            "name": "read_file",
                            "arguments": "{\"path\":\"one.txt\"}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        })
        .to_string(),
        profile,
        provider_protocol,
        LlmResponseValidation::RequireModelAction,
    )
    .unwrap()
}

fn bind_single_tool_turn(
    response: LlmChatResponse,
    run_id: &str,
    turn_index: usize,
) -> (LlmAssistantTurn, LlmToolCall) {
    let provider_call = response.provider_tool_calls()[0].clone();
    let runtime_call = LlmToolCall {
        id: model_response_tool_call_id(run_id, turn_index, 0, &provider_call.id),
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
    (turn, runtime_call)
}

fn expected_read_file_tool() -> Value {
    json!({
        "type": "function",
        "function": {
            "name": "read_file",
            "description": "Read a file.",
            "parameters": {
                "type": "object",
                "properties": {
                    "path": { "type": "string" }
                },
                "required": ["path"]
            }
        }
    })
}

#[test]
fn k3_payload_matches_complete_golden() {
    let k3 = request(
        k3_profile(ProviderReasoningEffort::Max),
        "kimi-k3",
        vec![message(LlmMessageRole::User, "hello")],
        vec![tool_definition()],
    );
    assert_eq!(
        build_payload(&k3),
        json!({
            "model": "kimi-k3",
            "messages": [{ "role": "user", "content": "hello" }],
            "stream": false,
            "max_completion_tokens": 128 * 1024,
            "reasoning_effort": "max",
            "tools": [expected_read_file_tool()],
            "tool_choice": "auto"
        })
    );
}

#[test]
fn k27_payload_matches_complete_golden() {
    let k27 = request(
        k27_profile(),
        "kimi-k2.7-code",
        vec![message(LlmMessageRole::User, "hello")],
        vec![tool_definition()],
    );
    assert_eq!(
        build_payload(&k27),
        json!({
            "model": "kimi-k2.7-code",
            "messages": [{ "role": "user", "content": "hello" }],
            "stream": false,
            "max_completion_tokens": 128 * 1024,
            "tools": [expected_read_file_tool()],
            "tool_choice": "auto"
        })
    );
}

#[test]
fn k26_payload_matches_complete_golden() {
    let k26 = request(
        k26_profile(MoonshotK26ThinkingMode::EnabledKeepAll),
        "kimi-k2.6",
        vec![message(LlmMessageRole::User, "hello")],
        vec![tool_definition()],
    );
    assert_eq!(
        build_payload(&k26),
        json!({
            "model": "kimi-k2.6",
            "messages": [{ "role": "user", "content": "hello" }],
            "stream": false,
            "max_completion_tokens": 128 * 1024,
            "thinking": { "type": "enabled", "keep": "all" },
            "tools": [expected_read_file_tool()],
            "tool_choice": "auto"
        })
    );
}

#[test]
fn k3_payload_uses_completion_limit_efforts_and_no_k2_sampling_fields() {
    let cases = [
        (ProviderReasoningEffort::ProviderDefault, None),
        (ProviderReasoningEffort::Low, Some("low")),
        (ProviderReasoningEffort::High, Some("high")),
        (ProviderReasoningEffort::Max, Some("max")),
    ];
    for (effort, expected) in cases {
        let request = request(
            k3_profile(effort),
            "kimi-k3",
            vec![message(LlmMessageRole::User, "hello")],
            vec![tool_definition()],
        );
        let payload = build_payload(&request);
        assert_eq!(payload["max_completion_tokens"], 128 * 1024);
        assert_eq!(
            payload.get("reasoning_effort").and_then(Value::as_str),
            expected
        );
        assert_eq!(payload["tool_choice"], "auto");
        for forbidden in [
            "thinking",
            "max_tokens",
            "temperature",
            "top_p",
            "n",
            "presence_penalty",
            "frequency_penalty",
        ] {
            assert!(
                payload.get(forbidden).is_none(),
                "K3 unexpectedly sent {forbidden}"
            );
        }
    }
}

#[test]
fn k27_code_payload_omits_thinking_effort_and_fixed_sampling_fields() {
    for model in ["kimi-k2.7-code", "kimi-k2.7-code-highspeed"] {
        let request = request(
            k27_profile(),
            model,
            vec![message(LlmMessageRole::User, "hello")],
            vec![tool_definition()],
        );
        let payload = build_payload(&request);
        assert_eq!(payload["max_completion_tokens"], 128 * 1024);
        assert_eq!(payload["tool_choice"], "auto");
        for forbidden in [
            "thinking",
            "reasoning_effort",
            "max_tokens",
            "temperature",
            "top_p",
            "n",
            "presence_penalty",
            "frequency_penalty",
        ] {
            assert!(
                payload.get(forbidden).is_none(),
                "K2.7 unexpectedly sent {forbidden}"
            );
        }
    }
}

#[test]
fn k27_nonstream_preserves_required_reasoning() {
    let profile = k27_profile();
    let provider_protocol = protocol(&profile, "kimi-k2.7-code");
    let response = parse_non_stream_response_with_profile(
        &json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "Visible.",
                    "reasoning_content": "K2.7 private reasoning"
                },
                "finish_reason": "stop"
            }]
        })
        .to_string(),
        &profile,
        &provider_protocol,
        LlmResponseValidation::RequireModelAction,
    )
    .unwrap();
    assert_eq!(response.content(), "Visible.");
    assert_eq!(
        super::super::providers::moonshot::reasoning_content(
            &provider_protocol,
            &response.assistant_turn,
        )
        .unwrap(),
        Some("K2.7 private reasoning")
    );
}

#[test]
fn moonshot_thinking_families_do_not_invent_visible_or_reasoning_usage() {
    for (profile, model) in [
        (k3_profile(ProviderReasoningEffort::Max), "kimi-k3"),
        (k27_profile(), "kimi-k2.7-code"),
        (
            k26_profile(MoonshotK26ThinkingMode::EnabledKeepAll),
            "kimi-k2.6",
        ),
    ] {
        let provider_protocol = protocol(&profile, model);
        let response = parse_non_stream_response_with_profile(
            &json!({
                "choices": [{
                    "message": {
                        "role": "assistant",
                        "content": "Visible.",
                        "reasoning_content": "Private."
                    },
                    "finish_reason": "stop"
                }],
                "usage": {
                    "prompt_tokens": 11,
                    "completion_tokens": 13,
                    "reasoning_tokens": 5,
                    "total_tokens": 24
                }
            })
            .to_string(),
            &profile,
            &provider_protocol,
            LlmResponseValidation::RequireModelAction,
        )
        .unwrap();
        let usage = response.usage.unwrap();
        assert_eq!(usage.input_tokens, Some(11));
        assert_eq!(usage.total_tokens, Some(24));
        assert_eq!(usage.output_tokens, None);
        assert_eq!(usage.output_thinking_tokens, None);
    }
}

#[test]
fn k26_payload_maps_all_four_thinking_modes_exactly() {
    let cases = [
        (MoonshotK26ThinkingMode::ProviderDefault, None),
        (
            MoonshotK26ThinkingMode::Enabled,
            Some(json!({ "type": "enabled" })),
        ),
        (
            MoonshotK26ThinkingMode::Disabled,
            Some(json!({ "type": "disabled" })),
        ),
        (
            MoonshotK26ThinkingMode::EnabledKeepAll,
            Some(json!({ "type": "enabled", "keep": "all" })),
        ),
    ];
    for (thinking_mode, expected) in cases {
        let request = request(
            k26_profile(thinking_mode),
            "kimi-k2.6",
            vec![message(LlmMessageRole::User, "hello")],
            Vec::new(),
        );
        let payload = build_payload(&request);
        assert_eq!(payload.get("thinking").cloned(), expected);
        assert!(payload.get("reasoning_effort").is_none());
        assert!(payload.get("temperature").is_none());
        assert!(payload.get("max_tokens").is_none());
    }
}

#[test]
fn moonshot_unknown_model_and_cross_family_settings_fail_closed() {
    let profile = k3_profile(ProviderReasoningEffort::Max);
    assert!(ProviderProtocolKey::new(
        AgentApiStyle::OpenAiCompatible.into(),
        &profile,
        "kimi-k3-future",
        None,
    )
    .is_err());

    let mismatched = ProviderProfileConfig::from_family_settings(
        ProviderProfileRef::moonshot_k3_chat(),
        ProviderVendorId::Moonshot,
        ProviderFamilySettings::MoonshotK26Chat {
            thinking_mode: MoonshotK26ThinkingMode::Enabled,
        },
    );
    assert!(ProviderProtocolKey::new(
        AgentApiStyle::OpenAiCompatible.into(),
        &mismatched,
        "kimi-k3",
        None,
    )
    .is_err());
}

#[test]
fn preserved_families_require_provider_reasoning_but_allow_host_split_assistant_context() {
    let profile = k3_profile(ProviderReasoningEffort::Max);
    let provider_protocol = protocol(&profile, "kimi-k3");
    let split_request = request(
        profile.clone(),
        "kimi-k3",
        vec![
            LlmMessage::assistant("host-generated context", Vec::new()),
            message(LlmMessageRole::User, "continue"),
        ],
        Vec::new(),
    );
    validate_request(&split_request).unwrap();
    let split_payload = build_payload(&split_request);
    assert_eq!(split_payload["messages"][0]["role"], "assistant");
    assert!(split_payload["messages"][0]
        .get("reasoning_content")
        .is_none());

    let provider_turn = LlmAssistantTurn::from_provider(
        provider_protocol,
        "provider answer without captured reasoning",
        Vec::new(),
    )
    .unwrap();
    let provider_request = request(
        profile,
        "kimi-k3",
        vec![
            LlmMessage::from_assistant_turn(provider_turn),
            message(LlmMessageRole::User, "continue"),
        ],
        Vec::new(),
    );
    let error = validate_request(&provider_request).unwrap_err();
    assert_eq!(error.code(), Some("provider_context_boundary_required"));
}

#[test]
fn every_exact_moonshot_family_projects_image_url() {
    let image_message = || {
        let mut message = LlmMessage::text(LlmMessageRole::User, "inspect this image");
        message.images_mut().unwrap().push(LlmImage {
            mime_type: "image/png".to_string(),
            data_base64: "AA==".to_string(),
        });
        message
    };

    for (profile, model) in [
        (k3_profile(ProviderReasoningEffort::Max), "kimi-k3"),
        (k27_profile(), "kimi-k2.7-code"),
        (
            k26_profile(MoonshotK26ThinkingMode::EnabledKeepAll),
            "kimi-k2.6",
        ),
    ] {
        let request = request(profile, model, vec![image_message()], Vec::new());
        validate_request(&request).unwrap();
        let payload = build_payload(&request);
        assert_eq!(payload["messages"][0]["content"][0]["type"], "text");
        assert_eq!(payload["messages"][0]["content"][1]["type"], "image_url");
        assert_eq!(
            payload["messages"][0]["content"][1]["image_url"]["url"],
            "data:image/png;base64,AA=="
        );
    }
}

#[test]
fn k3_nonstream_captures_and_replays_present_ordinary_reasoning_without_exposing_it() {
    const PRIVATE: &str = "K3_PRIVATE_REASONING_CANARY";
    let profile = k3_profile(ProviderReasoningEffort::Max);
    let provider_protocol = protocol(&profile, "kimi-k3");
    let response = parse_non_stream_response_with_profile(
        &json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "Visible answer.",
                    "reasoning_content": PRIVATE
                },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 3,
                "completion_tokens": 7,
                "total_tokens": 10
            }
        })
        .to_string(),
        &profile,
        &provider_protocol,
        LlmResponseValidation::RequireModelAction,
    )
    .unwrap();
    assert_eq!(response.content(), "Visible answer.");
    assert_eq!(response.usage.as_ref().unwrap().output_tokens, None);
    assert_eq!(response.usage.as_ref().unwrap().total_tokens, Some(10));
    assert!(!response.content().contains(PRIVATE));
    assert_eq!(
        super::super::providers::moonshot::reasoning_content(
            &provider_protocol,
            &response.assistant_turn,
        )
        .unwrap(),
        Some(PRIVATE)
    );

    let next = request(
        profile,
        "kimi-k3",
        vec![
            LlmMessage::from_assistant_turn(response.assistant_turn),
            message(LlmMessageRole::User, "follow up"),
        ],
        Vec::new(),
    );
    let payload = build_payload(&next);
    assert_eq!(payload["messages"][0]["reasoning_content"], PRIVATE);
}

#[test]
fn k3_absent_reasoning_field_is_captured_and_replayed_as_absent() {
    let profile = k3_profile(ProviderReasoningEffort::ProviderDefault);
    let provider_protocol = protocol(&profile, "kimi-k3");
    let response = parse_non_stream_response_with_profile(
        &json!({
            "choices": [{
                "message": { "role": "assistant", "content": "Direct answer." },
                "finish_reason": "stop"
            }]
        })
        .to_string(),
        &profile,
        &provider_protocol,
        LlmResponseValidation::RequireModelAction,
    )
    .unwrap();
    assert!(response.assistant_turn.provider_continuation().is_some());
    assert_eq!(
        super::super::providers::moonshot::reasoning_content(
            &provider_protocol,
            &response.assistant_turn,
        )
        .unwrap(),
        None
    );

    let next = request(
        profile,
        "kimi-k3",
        vec![
            LlmMessage::from_assistant_turn(response.assistant_turn),
            message(LlmMessageRole::User, "follow up"),
        ],
        Vec::new(),
    );
    let payload = build_payload(&next);
    assert!(payload["messages"][0].get("reasoning_content").is_none());
}

#[test]
fn k3_and_k27_nonstream_reasoning_only_counts_as_private_model_action() {
    for (profile, model, private_reasoning) in [
        (
            k3_profile(ProviderReasoningEffort::Max),
            "kimi-k3",
            "K3_REASONING_ONLY_PRIVATE",
        ),
        (
            k27_profile(),
            "kimi-k2.7-code",
            "K27_REASONING_ONLY_PRIVATE",
        ),
    ] {
        let provider_protocol = protocol(&profile, model);
        let response = parse_non_stream_response_with_profile(
            &json!({
                "choices": [{
                    "message": {
                        "role": "assistant",
                        "content": "",
                        "reasoning_content": private_reasoning
                    },
                    "finish_reason": "stop"
                }]
            })
            .to_string(),
            &profile,
            &provider_protocol,
            LlmResponseValidation::RequireModelAction,
        )
        .unwrap();
        assert_eq!(response.content(), "");
        assert_eq!(
            super::super::providers::moonshot::reasoning_content(
                &provider_protocol,
                &response.assistant_turn,
            )
            .unwrap(),
            Some(private_reasoning)
        );
        assert!(!response.content().contains(private_reasoning));
        assert!(!format!("{response:?}").contains(private_reasoning));
    }
}

#[test]
fn k3_missing_or_empty_reasoning_does_not_bypass_empty_model_action_guard() {
    let profile = k3_profile(ProviderReasoningEffort::Max);
    let provider_protocol = protocol(&profile, "kimi-k3");
    for message in [
        json!({ "role": "assistant", "content": "" }),
        json!({ "role": "assistant", "content": "", "reasoning_content": "" }),
    ] {
        let error = parse_non_stream_response_with_profile(
            &json!({
                "choices": [{
                    "message": message,
                    "finish_reason": "stop"
                }]
            })
            .to_string(),
            &profile,
            &provider_protocol,
            LlmResponseValidation::RequireModelAction,
        )
        .unwrap_err();
        assert_eq!(error.code(), Some(EMPTY_MODEL_ACTION_ERROR_CODE));
    }
}

#[tokio::test]
async fn k3_and_k27_streaming_reasoning_only_counts_as_private_model_action() {
    const K3_PRIVATE: &str = "K3_STREAM_REASONING_ONLY_PRIVATE";
    const K27_PRIVATE: &str = "K27_STREAM_REASONING_ONLY_PRIVATE";
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        for private_reasoning in [K3_PRIVATE, K27_PRIVATE] {
            let (mut stream, _) = listener.accept().await.unwrap();
            read_test_http_request(&mut stream).await;
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
                )
                .await
                .unwrap();
            stream
                .write_all(
                    format!(
                        "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
                        json!({ "choices": [{ "delta": { "reasoning_content": private_reasoning } }] }),
                        json!({ "choices": [{ "delta": {}, "finish_reason": "stop" }] })
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
        }
    });

    for (profile, model, private_reasoning) in [
        (
            k3_profile(ProviderReasoningEffort::Max),
            "kimi-k3",
            K3_PRIVATE,
        ),
        (k27_profile(), "kimi-k2.7-code", K27_PRIVATE),
    ] {
        let mut request = request(
            profile,
            model,
            vec![message(LlmMessageRole::User, "reason privately")],
            Vec::new(),
        );
        request.api_url = format!("http://{address}/v1/chat/completions");
        let provider_protocol = request.provider_protocol.clone();
        let mut events = Vec::new();
        let response = complete_chat_streaming(request, AgentCancellationToken::new(), |event| {
            events.push(event)
        })
        .await
        .unwrap();
        assert_eq!(response.content(), "");
        assert!(events
            .iter()
            .all(|event| !matches!(event, LlmStreamEvent::Delta(_))));
        assert_eq!(
            super::super::providers::moonshot::reasoning_content(
                &provider_protocol,
                &response.assistant_turn,
            )
            .unwrap(),
            Some(private_reasoning)
        );
    }
    server.await.unwrap();
}

#[tokio::test]
async fn k3_streaming_missing_or_empty_reasoning_remains_an_empty_model_action() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        for reasoning_delta in [None, Some("")] {
            let (mut stream, _) = listener.accept().await.unwrap();
            read_test_http_request(&mut stream).await;
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
                )
                .await
                .unwrap();
            if let Some(reasoning_delta) = reasoning_delta {
                stream
                    .write_all(
                        format!(
                            "data: {}\n\n",
                            json!({ "choices": [{ "delta": { "reasoning_content": reasoning_delta } }] })
                        )
                        .as_bytes(),
                    )
                    .await
                    .unwrap();
            }
            stream
                .write_all(
                    format!(
                        "data: {}\n\ndata: [DONE]\n\n",
                        json!({ "choices": [{ "delta": {}, "finish_reason": "stop" }] })
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
        }
    });

    for _ in 0..2 {
        let mut request = request(
            k3_profile(ProviderReasoningEffort::Max),
            "kimi-k3",
            vec![message(LlmMessageRole::User, "reason privately")],
            Vec::new(),
        );
        request.api_url = format!("http://{address}/v1/chat/completions");
        let error = complete_chat_streaming(request, AgentCancellationToken::new(), |_| {})
            .await
            .unwrap_err();
        assert_eq!(error.code(), Some(EMPTY_MODEL_ACTION_ERROR_CODE));
    }
    server.await.unwrap();
}

#[test]
fn k27_missing_reasoning_fails_closed_for_nonstream_and_stream() {
    let profile = k27_profile();
    let provider_protocol = protocol(&profile, "kimi-k2.7-code");
    let body = json!({
        "choices": [{
            "message": { "role": "assistant", "content": "Visible." },
            "finish_reason": "stop"
        }]
    });
    let error = parse_non_stream_response_with_profile(
        &body.to_string(),
        &profile,
        &provider_protocol,
        LlmResponseValidation::RequireModelAction,
    )
    .unwrap_err();
    assert_eq!(error.code(), Some("provider_reasoning_required"));

    let mut accumulator = LlmStreamAccumulator::for_profile(&profile, &provider_protocol).unwrap();
    process_sse_frame(
        &format!(
            "data: {}\n\n",
            json!({
                "choices": [{
                    "delta": { "content": "Visible." },
                    "finish_reason": "stop"
                }]
            })
        ),
        &mut accumulator,
        &mut |_| {},
    )
    .unwrap();
    let error = accumulator.finish().unwrap_err();
    assert_eq!(error.code(), Some("provider_reasoning_required"));
}

#[test]
fn k27_stream_preserves_reasoning_without_visible_projection() {
    const PRIVATE: &str = "K27_STREAM_PRIVATE";
    let profile = k27_profile();
    let provider_protocol = protocol(&profile, "kimi-k2.7-code-highspeed");
    let mut accumulator = LlmStreamAccumulator::for_profile(&profile, &provider_protocol).unwrap();
    let mut events = Vec::new();
    for value in [
        json!({ "choices": [{ "delta": { "reasoning_content": PRIVATE } }] }),
        json!({
            "choices": [{
                "delta": { "content": "Visible." },
                "finish_reason": "stop"
            }]
        }),
    ] {
        process_sse_frame(
            &format!("data: {value}\n\n"),
            &mut accumulator,
            &mut |event| events.push(event),
        )
        .unwrap();
    }
    let response = accumulator.finish().unwrap();
    assert_eq!(joined_text(&events), "Visible.");
    assert!(events.iter().all(|event| !matches!(
        event,
        LlmStreamEvent::Delta(delta) if delta.contains(PRIVATE)
    )));
    assert_eq!(
        super::super::providers::moonshot::reasoning_content(
            &provider_protocol,
            &response.assistant_turn,
        )
        .unwrap(),
        Some(PRIVATE)
    );
}

#[test]
fn k27_single_tool_result_continuation_replays_provider_state() {
    let profile = k27_profile();
    let provider_protocol = protocol(&profile, "kimi-k2.7-code");
    let response = moonshot_single_tool_response(
        &profile,
        &provider_protocol,
        "K2.7 tool reasoning",
        "k27-tool-call",
    );
    let (turn, runtime_call) = bind_single_tool_turn(response, "k27-tool-run", 0);
    let next = request(
        profile,
        "kimi-k2.7-code",
        vec![
            LlmMessage::from_assistant_turn(turn),
            LlmMessage::tool_result(runtime_call.id, "tool result", false),
        ],
        vec![tool_definition()],
    );
    let payload = build_payload(&next);
    assert_eq!(
        payload["messages"][0]["reasoning_content"],
        "K2.7 tool reasoning"
    );
    assert_eq!(
        payload["messages"][0]["tool_calls"][0]["id"],
        "k27-tool-call"
    );
    assert_eq!(payload["messages"][1]["tool_call_id"], "k27-tool-call");
}

#[test]
fn k26_default_and_enabled_drop_ordinary_reasoning_but_keep_all_replays_it() {
    for thinking_mode in [
        MoonshotK26ThinkingMode::ProviderDefault,
        MoonshotK26ThinkingMode::Enabled,
    ] {
        let profile = k26_profile(thinking_mode);
        let provider_protocol = protocol(&profile, "kimi-k2.6");
        let response = parse_non_stream_response_with_profile(
            &json!({
                "choices": [{
                    "message": {
                        "role": "assistant",
                        "content": "Visible.",
                        "reasoning_content": "ordinary reasoning must not cross user boundary"
                    },
                    "finish_reason": "stop"
                }]
            })
            .to_string(),
            &profile,
            &provider_protocol,
            LlmResponseValidation::RequireModelAction,
        )
        .unwrap();
        assert!(response.assistant_turn.provider_continuation().is_none());
        let next = request(
            profile,
            "kimi-k2.6",
            vec![
                LlmMessage::from_assistant_turn(response.assistant_turn),
                message(LlmMessageRole::User, "next"),
            ],
            Vec::new(),
        );
        assert!(build_payload(&next)["messages"][0]
            .get("reasoning_content")
            .is_none());
    }

    let profile = k26_profile(MoonshotK26ThinkingMode::EnabledKeepAll);
    let provider_protocol = protocol(&profile, "kimi-k2.6");
    let response = parse_non_stream_response_with_profile(
        &json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "Visible.",
                    "reasoning_content": "kept reasoning"
                },
                "finish_reason": "stop"
            }]
        })
        .to_string(),
        &profile,
        &provider_protocol,
        LlmResponseValidation::RequireModelAction,
    )
    .unwrap();
    let next = request(
        profile,
        "kimi-k2.6",
        vec![
            LlmMessage::from_assistant_turn(response.assistant_turn),
            message(LlmMessageRole::User, "next"),
        ],
        Vec::new(),
    );
    assert_eq!(
        build_payload(&next)["messages"][0]["reasoning_content"],
        "kept reasoning"
    );
}

#[test]
fn k26_enabled_preserves_reasoning_inside_a_tool_interaction() {
    let profile = k26_profile(MoonshotK26ThinkingMode::Enabled);
    let provider_protocol = protocol(&profile, "kimi-k2.6");
    let response = parse_non_stream_response_with_profile(
        &json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "",
                    "reasoning_content": "tool-local reasoning",
                    "tool_calls": [{
                        "id": "k26-call",
                        "type": "function",
                        "function": {
                            "name": "read_file",
                            "arguments": "{\"path\":\"one.txt\"}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        })
        .to_string(),
        &profile,
        &provider_protocol,
        LlmResponseValidation::RequireModelAction,
    )
    .unwrap();
    let provider_call = response.provider_tool_calls()[0].clone();
    let runtime_call = LlmToolCall {
        id: model_response_tool_call_id("k26-run", 0, 0, &provider_call.id),
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
    let next = request(
        profile,
        "kimi-k2.6",
        vec![
            LlmMessage::from_assistant_turn(turn),
            LlmMessage::tool_result(runtime_call.id, "result", false),
        ],
        vec![tool_definition()],
    );
    let payload = build_payload(&next);
    assert_eq!(
        payload["messages"][0]["reasoning_content"],
        "tool-local reasoning"
    );
    assert_eq!(payload["messages"][0]["tool_calls"][0]["id"], "k26-call");
}

#[test]
fn k26_disabled_nonstream_rejects_private_reasoning_and_projects_visible_usage() {
    let profile = k26_profile(MoonshotK26ThinkingMode::Disabled);
    let provider_protocol = protocol(&profile, "kimi-k2.6");
    let error = parse_non_stream_response_with_profile(
        &json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "Visible.",
                    "reasoning_content": "unexpected private reasoning"
                },
                "finish_reason": "stop"
            }]
        })
        .to_string(),
        &profile,
        &provider_protocol,
        LlmResponseValidation::RequireModelAction,
    )
    .unwrap_err();
    assert_eq!(error.code(), Some("provider_reasoning_forbidden"));

    let response = parse_non_stream_response_with_profile(
        &json!({
            "choices": [{
                "message": { "role": "assistant", "content": "Visible." },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 4,
                "completion_tokens": 6,
                "total_tokens": 10
            }
        })
        .to_string(),
        &profile,
        &provider_protocol,
        LlmResponseValidation::RequireModelAction,
    )
    .unwrap();
    let usage = response.usage.unwrap();
    assert_eq!(usage.output_tokens, Some(6));
    assert_eq!(usage.output_thinking_tokens, Some(0));
}

#[test]
fn k3_stream_hides_reasoning_and_preserves_it_for_replay() {
    const PRIVATE: &str = "STREAM_PRIVATE_REASONING";
    let profile = k3_profile(ProviderReasoningEffort::High);
    let provider_protocol = protocol(&profile, "kimi-k3");
    let mut accumulator = LlmStreamAccumulator::for_profile(&profile, &provider_protocol).unwrap();
    let mut events = Vec::new();
    for value in [
        json!({ "choices": [{ "delta": { "reasoning_content": PRIVATE } }] }),
        json!({
            "choices": [{
                "delta": { "content": "Visible." },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 2,
                "completion_tokens": 5,
                "total_tokens": 7
            }
        }),
    ] {
        process_sse_frame(
            &format!("data: {value}\n\n"),
            &mut accumulator,
            &mut |event| events.push(event),
        )
        .unwrap();
    }
    let response = accumulator.finish().unwrap();
    assert_eq!(joined_text(&events), "Visible.");
    assert!(events.iter().all(|event| !matches!(
        event,
        LlmStreamEvent::Delta(delta) if delta.contains(PRIVATE)
    )));
    assert_eq!(response.usage.as_ref().unwrap().output_tokens, None);
    assert_eq!(
        super::super::providers::moonshot::reasoning_content(
            &provider_protocol,
            &response.assistant_turn,
        )
        .unwrap(),
        Some(PRIVATE)
    );
}

#[test]
fn k3_stream_without_reasoning_captures_absence_and_replays_no_field() {
    let profile = k3_profile(ProviderReasoningEffort::ProviderDefault);
    let provider_protocol = protocol(&profile, "kimi-k3");
    let mut accumulator = LlmStreamAccumulator::for_profile(&profile, &provider_protocol).unwrap();
    process_sse_frame(
        &format!(
            "data: {}\n\n",
            json!({
                "choices": [{
                    "delta": { "content": "Direct stream answer." },
                    "finish_reason": "stop"
                }]
            })
        ),
        &mut accumulator,
        &mut |_| {},
    )
    .unwrap();
    let response = accumulator.finish().unwrap();
    assert!(response.assistant_turn.provider_continuation().is_some());
    assert_eq!(
        super::super::providers::moonshot::reasoning_content(
            &provider_protocol,
            &response.assistant_turn,
        )
        .unwrap(),
        None
    );

    let next = request(
        profile,
        "kimi-k3",
        vec![
            LlmMessage::from_assistant_turn(response.assistant_turn),
            message(LlmMessageRole::User, "follow up"),
        ],
        Vec::new(),
    );
    assert!(build_payload(&next)["messages"][0]
        .get("reasoning_content")
        .is_none());
}

#[test]
fn k26_disabled_stream_usage_matches_nonstream_visible_projection() {
    let profile = k26_profile(MoonshotK26ThinkingMode::Disabled);
    let provider_protocol = protocol(&profile, "kimi-k2.6");
    let mut accumulator = LlmStreamAccumulator::for_profile(&profile, &provider_protocol).unwrap();
    process_sse_frame(
        &format!(
            "data: {}\n\n",
            json!({
                "choices": [{
                    "delta": { "content": "Visible." },
                    "finish_reason": "stop"
                }],
                "usage": {
                    "prompt_tokens": 4,
                    "completion_tokens": 6,
                    "total_tokens": 10
                }
            })
        ),
        &mut accumulator,
        &mut |_| {},
    )
    .unwrap();
    let usage = accumulator.finish().unwrap().usage.unwrap();
    assert_eq!(usage.output_tokens, Some(6));
    assert_eq!(usage.output_thinking_tokens, Some(0));
    assert_eq!(usage.total_tokens, Some(10));
}

#[test]
fn k26_enabled_and_keep_all_stream_modes_apply_distinct_ordinary_replay_policy() {
    const PRIVATE: &str = "K26_STREAM_PRIVATE";
    for (thinking_mode, preserves_ordinary) in [
        (MoonshotK26ThinkingMode::Enabled, false),
        (MoonshotK26ThinkingMode::EnabledKeepAll, true),
    ] {
        let profile = k26_profile(thinking_mode);
        let provider_protocol = protocol(&profile, "kimi-k2.6");
        let mut accumulator =
            LlmStreamAccumulator::for_profile(&profile, &provider_protocol).unwrap();
        let mut events = Vec::new();
        for value in [
            json!({ "choices": [{ "delta": { "reasoning_content": PRIVATE } }] }),
            json!({
                "choices": [{
                    "delta": { "content": "Visible." },
                    "finish_reason": "stop"
                }]
            }),
        ] {
            process_sse_frame(
                &format!("data: {value}\n\n"),
                &mut accumulator,
                &mut |event| events.push(event),
            )
            .unwrap();
        }
        let response = accumulator.finish().unwrap();
        assert_eq!(joined_text(&events), "Visible.");
        assert!(events.iter().all(|event| !matches!(
            event,
            LlmStreamEvent::Delta(delta) if delta.contains(PRIVATE)
        )));
        assert_eq!(
            response.assistant_turn.provider_continuation().is_some(),
            preserves_ordinary
        );
        if preserves_ordinary {
            assert_eq!(
                super::super::providers::moonshot::reasoning_content(
                    &provider_protocol,
                    &response.assistant_turn,
                )
                .unwrap(),
                Some(PRIVATE)
            );
        }
    }
}

#[test]
fn k3_parallel_tool_turn_replays_reasoning_and_provider_tool_ids() {
    let profile = k3_profile(ProviderReasoningEffort::Max);
    let provider_protocol = protocol(&profile, "kimi-k3");
    let response = parse_non_stream_response_with_profile(
        &json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "",
                    "reasoning_content": "use both tools",
                    "tool_calls": [
                        {
                            "id": "moonshot-call-1",
                            "type": "function",
                            "function": {
                                "name": "read_file",
                                "arguments": "{\"path\":\"one.txt\"}"
                            }
                        },
                        {
                            "id": "moonshot-call-2",
                            "type": "function",
                            "function": {
                                "name": "read_file",
                                "arguments": "{\"path\":\"two.txt\"}"
                            }
                        }
                    ]
                },
                "finish_reason": "tool_calls"
            }]
        })
        .to_string(),
        &profile,
        &provider_protocol,
        LlmResponseValidation::RequireModelAction,
    )
    .unwrap();
    let provider_calls = response.provider_tool_calls().to_vec();
    let runtime_calls = provider_calls
        .iter()
        .enumerate()
        .map(|(index, provider_call)| LlmToolCall {
            id: model_response_tool_call_id("moonshot-parallel-run", 0, index, &provider_call.id),
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
    let turn = response
        .assistant_turn
        .with_runtime_tool_bindings(bindings)
        .unwrap();
    let next = request(
        profile,
        "kimi-k3",
        vec![
            LlmMessage::from_assistant_turn(turn),
            LlmMessage::tool_result(runtime_calls[0].id.clone(), "one", false),
            LlmMessage::tool_result(runtime_calls[1].id.clone(), "two", false),
        ],
        vec![tool_definition()],
    );
    let payload = build_payload(&next);
    assert_eq!(
        payload["messages"][0]["reasoning_content"],
        "use both tools"
    );
    assert_eq!(
        payload["messages"][0]["tool_calls"][0]["id"],
        "moonshot-call-1"
    );
    assert_eq!(
        payload["messages"][0]["tool_calls"][1]["id"],
        "moonshot-call-2"
    );
    assert_eq!(payload["messages"][1]["tool_call_id"], "moonshot-call-1");
    assert_eq!(payload["messages"][2]["tool_call_id"], "moonshot-call-2");
}

#[test]
fn k3_two_sequential_tool_rounds_keep_each_reasoning_and_result_in_order() {
    let profile = k3_profile(ProviderReasoningEffort::Max);
    let provider_protocol = protocol(&profile, "kimi-k3");
    let first = moonshot_single_tool_response(
        &profile,
        &provider_protocol,
        "first-round reasoning",
        "k3-sequential-call-1",
    );
    let second = moonshot_single_tool_response(
        &profile,
        &provider_protocol,
        "second-round reasoning",
        "k3-sequential-call-2",
    );
    let (first_turn, first_runtime_call) = bind_single_tool_turn(first, "k3-sequential", 0);
    let (second_turn, second_runtime_call) = bind_single_tool_turn(second, "k3-sequential", 1);
    let next = request(
        profile,
        "kimi-k3",
        vec![
            LlmMessage::from_assistant_turn(first_turn),
            LlmMessage::tool_result(first_runtime_call.id, "first result", false),
            LlmMessage::from_assistant_turn(second_turn),
            LlmMessage::tool_result(second_runtime_call.id, "second result", false),
        ],
        vec![tool_definition()],
    );
    let payload = build_payload(&next);
    assert_eq!(
        payload["messages"][0]["reasoning_content"],
        "first-round reasoning"
    );
    assert_eq!(
        payload["messages"][0]["tool_calls"][0]["id"],
        "k3-sequential-call-1"
    );
    assert_eq!(
        payload["messages"][1]["tool_call_id"],
        "k3-sequential-call-1"
    );
    assert_eq!(
        payload["messages"][2]["reasoning_content"],
        "second-round reasoning"
    );
    assert_eq!(
        payload["messages"][2]["tool_calls"][0]["id"],
        "k3-sequential-call-2"
    );
    assert_eq!(
        payload["messages"][3]["tool_call_id"],
        "k3-sequential-call-2"
    );
}
