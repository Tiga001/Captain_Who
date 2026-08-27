use super::*;

#[tokio::test]
async fn fake_deepseek_provider_round_trips_reasoning_and_raw_tool_identity() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let mut requests = Vec::new();
        for response_index in 0..3 {
            let (mut stream, _) = listener.accept().await.unwrap();
            requests.push(read_test_http_request_json(&mut stream).await);
            let body = match response_index {
                0 => json!({
                    "choices": [{
                        "message": {
                            "role": "assistant",
                            "content": "",
                            "reasoning_content": "I should inspect the file first.",
                            "tool_calls": [{
                                "id": "deepseek-raw-call-1",
                                "type": "function",
                                "function": {
                                    "name": "read_file",
                                    "arguments": "{\"path\":\"src/lib.rs\"}"
                                }
                            }]
                        },
                        "finish_reason": "tool_calls"
                    }],
                    "usage": {
                        "prompt_tokens": 10,
                        "completion_tokens": 6,
                        "total_tokens": 16,
                        "prompt_cache_hit_tokens": 4,
                        "prompt_cache_miss_tokens": 6,
                        "completion_tokens_details": { "reasoning_tokens": 4 }
                    }
                }),
                1 => json!({
                    "choices": [{
                        "message": {
                            "role": "assistant",
                            "content": "",
                            "reasoning_content": "I should inspect one more file.",
                            "tool_calls": [{
                                "id": "deepseek-raw-call-2",
                                "type": "function",
                                "function": {
                                    "name": "read_file",
                                    "arguments": "{\"path\":\"src/main.rs\"}"
                                }
                            }]
                        },
                        "finish_reason": "tool_calls"
                    }],
                    "usage": {
                        "prompt_tokens": 18,
                        "completion_tokens": 5,
                        "total_tokens": 23,
                        "completion_tokens_details": { "reasoning_tokens": 3 }
                    }
                }),
                2 => json!({
                    "choices": [{
                        "message": {
                            "role": "assistant",
                            "content": "The file is ready.",
                            "reasoning_content": "The tool result is sufficient."
                        },
                        "finish_reason": "stop"
                    }],
                    "usage": {
                        "prompt_tokens": 18,
                        "completion_tokens": 5,
                        "total_tokens": 23,
                        "completion_tokens_details": { "reasoning_tokens": 3 }
                    }
                }),
                _ => unreachable!(),
            };
            write_test_http_response(&mut stream, "200 OK", body).await;
        }
        requests
    });

    let provider_profile = deepseek_provider_profile(ReasoningMode::Enabled, ReasoningEffort::Max);
    let provider_protocol = deepseek_provider_protocol(&provider_profile, "deepseek-v4-pro");
    let registry = crate::tools::ToolRegistry::defaults_with_search(None);
    let tools = ["read_file", "apply_patch", "write_file"]
        .into_iter()
        .map(|name| {
            registry
                .definition_for(name)
                .unwrap_or_else(|| panic!("missing stable tool definition for {name}"))
        })
        .collect::<Vec<_>>();
    let first_request = LlmChatRequest {
        api_url: format!("http://{address}/chat/completions"),
        api_token: "deepseek-fake-token".to_string(),
        provider_profile: provider_profile.clone(),
        provider_protocol: provider_protocol.clone(),
        max_tokens: 1_024,
        temperature: 0.3,
        stream: false,
        messages: vec![LlmMessage::text(LlmMessageRole::User, "Inspect src/lib.rs")],
        tools: tools.clone(),
    };
    let first_response = complete_chat(first_request, AgentCancellationToken::new())
        .await
        .unwrap();
    assert_eq!(first_response.content(), "");
    assert_eq!(first_response.provider_tool_calls().len(), 1);
    assert_eq!(
        first_response.usage.as_ref().unwrap().input_tokens,
        Some(10)
    );
    assert_eq!(
        first_response
            .usage
            .as_ref()
            .unwrap()
            .output_thinking_tokens,
        Some(4)
    );
    assert_eq!(
        deepseek_reasoning_content(&provider_protocol, &first_response.assistant_turn).unwrap(),
        Some("I should inspect the file first.")
    );

    let provider_call = first_response.provider_tool_calls()[0].clone();
    let runtime_call = LlmToolCall {
        id: model_response_tool_call_id("deepseek-fake-run", 0, 0, &provider_call.id),
        name: provider_call.name.clone(),
        args: provider_call.args.clone(),
    };
    let first_turn = first_response
        .assistant_turn
        .with_runtime_tool_bindings(vec![LlmRuntimeToolCallBinding::new(
            0,
            &provider_call,
            runtime_call.clone(),
        )])
        .unwrap();
    let first_turn_message = LlmMessage::from_assistant_turn(first_turn);
    let first_result_message =
        LlmMessage::tool_result(runtime_call.id.clone(), "{\"ok\":true}", false);
    let second_request = LlmChatRequest {
        api_url: format!("http://{address}/chat/completions"),
        api_token: "deepseek-fake-token".to_string(),
        provider_profile: provider_profile.clone(),
        provider_protocol: provider_protocol.clone(),
        max_tokens: 1_024,
        temperature: 0.3,
        stream: false,
        messages: vec![
            LlmMessage::text(LlmMessageRole::User, "Inspect src/lib.rs"),
            first_turn_message.clone(),
            first_result_message.clone(),
        ],
        tools: tools.clone(),
    };
    let second_response = complete_chat(second_request, AgentCancellationToken::new())
        .await
        .unwrap();
    assert_eq!(second_response.content(), "");
    assert_eq!(second_response.provider_tool_calls().len(), 1);
    assert_eq!(
        deepseek_reasoning_content(&provider_protocol, &second_response.assistant_turn).unwrap(),
        Some("I should inspect one more file.")
    );

    let second_provider_call = second_response.provider_tool_calls()[0].clone();
    let second_runtime_call = LlmToolCall {
        id: model_response_tool_call_id("deepseek-fake-run", 1, 0, &second_provider_call.id),
        name: second_provider_call.name.clone(),
        args: second_provider_call.args.clone(),
    };
    let second_turn = second_response
        .assistant_turn
        .with_runtime_tool_bindings(vec![LlmRuntimeToolCallBinding::new(
            0,
            &second_provider_call,
            second_runtime_call.clone(),
        )])
        .unwrap();
    let third_request = LlmChatRequest {
        api_url: format!("http://{address}/chat/completions"),
        api_token: "deepseek-fake-token".to_string(),
        provider_profile,
        provider_protocol: provider_protocol.clone(),
        max_tokens: 1_024,
        temperature: 0.3,
        stream: false,
        messages: vec![
            LlmMessage::text(LlmMessageRole::User, "Inspect src/lib.rs"),
            first_turn_message,
            first_result_message,
            LlmMessage::from_assistant_turn(second_turn),
            LlmMessage::tool_result(second_runtime_call.id, "{\"ok\":true}", false),
        ],
        tools,
    };
    let third_response = complete_chat(third_request, AgentCancellationToken::new())
        .await
        .unwrap();
    let requests = server.await.unwrap();

    assert_eq!(third_response.content(), "The file is ready.");
    assert_eq!(
        third_response
            .assistant_turn
            .provider_continuation()
            .unwrap()
            .replay_scope(),
        ProviderContinuationReplayScope::AssistantTurnV1
    );
    assert_eq!(requests[0]["thinking"], json!({ "type": "enabled" }));
    assert_eq!(requests[0]["reasoning_effort"], "max");
    assert!(requests[0].get("temperature").is_none());
    assert!(requests[0].get("tool_choice").is_none());
    assert_eq!(requests[0]["tools"].as_array().unwrap().len(), 3);
    assert_eq!(requests[0]["tools"][0]["function"]["name"], "read_file");
    assert_eq!(requests[0]["tools"][1]["function"]["name"], "apply_patch");
    assert_eq!(requests[0]["tools"][2]["function"]["name"], "write_file");
    assert_eq!(
        requests[0]["tools"][1]["function"]["parameters"],
        registry.definition_for("apply_patch").unwrap().input_schema
    );
    assert_eq!(
        requests[0]["tools"][2]["function"]["parameters"],
        registry.definition_for("write_file").unwrap().input_schema
    );
    assert_eq!(requests[1]["messages"][1]["role"], "assistant");
    assert_eq!(requests[1]["messages"][1]["content"], "");
    assert_eq!(
        requests[1]["messages"][1]["reasoning_content"],
        "I should inspect the file first."
    );
    assert_eq!(
        requests[1]["messages"][1]["tool_calls"][0]["id"],
        "deepseek-raw-call-1"
    );
    assert_eq!(
        requests[1]["messages"][2]["tool_call_id"],
        "deepseek-raw-call-1"
    );
    assert_eq!(
        requests[2]["messages"][1]["reasoning_content"],
        "I should inspect the file first."
    );
    assert_eq!(
        requests[2]["messages"][3]["reasoning_content"],
        "I should inspect one more file."
    );
    assert_eq!(
        requests[2]["messages"][3]["tool_calls"][0]["id"],
        "deepseek-raw-call-2"
    );
    assert_eq!(
        requests[2]["messages"][4]["tool_call_id"],
        "deepseek-raw-call-2"
    );
}

#[tokio::test]
async fn deepseek_stream_retry_discards_failed_attempt_reasoning() {
    const STALE: &str = "STALE_DEEPSEEK_REASONING";
    const FRESH: &str = "FRESH_DEEPSEEK_REASONING";
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        for attempt in 0..2 {
            let (mut stream, _) = listener.accept().await.unwrap();
            read_test_http_request(&mut stream).await;
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
                )
                .await
                .unwrap();
            if attempt == 0 {
                stream
                    .write_all(
                        format!(
                            "data: {}\n\ndata: {{invalid-json\n\n",
                            json!({
                                "choices":[{"delta":{"reasoning_content":STALE}}],
                                "usage": {
                                    "prompt_tokens": 3,
                                    "completion_tokens": 3,
                                    "total_tokens": 6,
                                    "completion_tokens_details": { "reasoning_tokens": 2 }
                                }
                            })
                        )
                        .as_bytes(),
                    )
                    .await
                    .unwrap();
            } else {
                stream
                    .write_all(
                        format!(
                            "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
                            json!({"choices":[{"delta":{"reasoning_content":FRESH}}]}),
                            json!({
                                "choices":[{
                                    "delta":{"content":"Recovered."},
                                    "finish_reason":"stop"
                                }],
                                "usage": {
                                    "prompt_tokens": 2,
                                    "completion_tokens": 2,
                                    "total_tokens": 4
                                }
                            })
                        )
                        .as_bytes(),
                    )
                    .await
                    .unwrap();
            }
        }
    });

    let provider_profile = deepseek_provider_profile(ReasoningMode::Enabled, ReasoningEffort::High);
    let provider_protocol = deepseek_provider_protocol(&provider_profile, "deepseek-v4-flash");
    let request = LlmChatRequest {
        api_url: format!("http://{address}/chat/completions"),
        api_token: "deepseek-stream-retry-token".to_string(),
        provider_profile,
        provider_protocol: provider_protocol.clone(),
        max_tokens: 128,
        temperature: 0.0,
        stream: true,
        messages: vec![LlmMessage::text(LlmMessageRole::User, "hello")],
        tools: Vec::new(),
    };
    let mut events = Vec::new();
    let response = complete_chat_streaming(request, AgentCancellationToken::new(), |event| {
        events.push(event);
    })
    .await
    .unwrap();
    server.await.unwrap();

    assert_eq!(response.content(), "Recovered.");
    let usage = response.usage.as_ref().expect("retry usage");
    assert_eq!(usage.input_tokens, Some(5));
    assert_eq!(usage.total_tokens, Some(10));
    assert_eq!(usage.output_tokens, None);
    assert_eq!(usage.output_thinking_tokens, None);
    assert_eq!(usage.billable_request_count, Some(2));
    assert_eq!(
        deepseek_reasoning_content(&provider_protocol, &response.assistant_turn).unwrap(),
        Some(FRESH)
    );
    assert!(events.iter().all(|event| !matches!(
        event,
        LlmStreamEvent::Delta(delta) if delta.contains(STALE) || delta.contains(FRESH)
    )));
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, LlmStreamEvent::AttemptStarted { .. }))
            .count(),
        2
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, LlmStreamEvent::AttemptReset { .. }))
            .count(),
        1
    );
}

#[test]
fn provider_continuation_is_bounded_bound_and_debug_redacted() {
    const CANARY: &str = "OPAQUE_PROVIDER_CONTINUATION_CANARY";
    let protocol = generic_provider_protocol(AgentApiStyle::OpenAiCompatible, "gpt");
    let turn = LlmAssistantTurn::from_provider(protocol.clone(), "visible", Vec::new()).unwrap();
    let continuation = ProviderContinuation::new(
        protocol.clone(),
        ProviderContinuationReplayScope::AssistantTurnV1,
        turn.digest(),
        vec![ProviderContinuationFragment::new(
            ProviderContinuationPosition::new(0, ProviderContinuationAttachment::AssistantTurn),
            CANARY.as_bytes().to_vec(),
        )],
    )
    .unwrap();

    assert_eq!(continuation.encoded_bytes(), CANARY.len());
    assert_eq!(
        continuation.replay_scope(),
        ProviderContinuationReplayScope::AssistantTurnV1
    );
    assert_eq!(continuation.payload_hash().len(), "sha256:".len() + 64);
    assert!(!format!("{continuation:?}").contains(CANARY));

    let revision_without_continuation = turn.context_revision_material();
    let turn = turn.with_provider_continuation(continuation).unwrap();
    let revision_with_continuation = turn.context_revision_material();
    assert_eq!(revision_with_continuation.len(), "sha256:".len() + 64);
    assert!(!revision_with_continuation.contains(CANARY));
    assert_ne!(revision_with_continuation, revision_without_continuation);
    let serialized_checkpoint_identity =
        serde_json::to_string(&turn.checkpoint_identity().unwrap()).unwrap();
    assert!(!serialized_checkpoint_identity.contains(CANARY));
    assert!(turn
        .without_raw_continuation_for_checkpoint()
        .provider_continuation()
        .is_none());

    let at_limit = ProviderContinuation::new(
        protocol.clone(),
        ProviderContinuationReplayScope::AssistantTurnV1,
        turn.digest(),
        vec![ProviderContinuationFragment::new(
            ProviderContinuationPosition::new(0, ProviderContinuationAttachment::AssistantTurn),
            vec![0; MAX_PROVIDER_CONTINUATION_BYTES],
        )],
    )
    .unwrap();
    assert_eq!(at_limit.encoded_bytes(), MAX_PROVIDER_CONTINUATION_BYTES);

    let oversized = ProviderContinuation::new(
        protocol,
        ProviderContinuationReplayScope::AssistantTurnV1,
        turn.digest(),
        vec![ProviderContinuationFragment::new(
            ProviderContinuationPosition::new(0, ProviderContinuationAttachment::AssistantTurn),
            vec![0; MAX_PROVIDER_CONTINUATION_BYTES + 1],
        )],
    );
    assert!(oversized.is_err());
}

#[test]
fn provider_continuation_rejects_cross_key_digest_and_reordered_fragments() {
    let protocol = generic_provider_protocol(AgentApiStyle::OpenAiCompatible, "gpt-a");
    let other_protocol = generic_provider_protocol(AgentApiStyle::OpenAiCompatible, "gpt-b");
    let turn = LlmAssistantTurn::from_provider(protocol.clone(), "visible", Vec::new()).unwrap();
    let other_turn =
        LlmAssistantTurn::from_provider(protocol.clone(), "changed", Vec::new()).unwrap();
    let fragment = || {
        vec![ProviderContinuationFragment::new(
            ProviderContinuationPosition::new(0, ProviderContinuationAttachment::AssistantTurn),
            vec![1],
        )]
    };

    let cross_key = ProviderContinuation::new(
        other_protocol,
        ProviderContinuationReplayScope::AssistantTurnV1,
        turn.digest(),
        fragment(),
    )
    .unwrap();
    assert!(turn.clone().with_provider_continuation(cross_key).is_err());

    let cross_digest = ProviderContinuation::new(
        protocol.clone(),
        ProviderContinuationReplayScope::AssistantTurnV1,
        other_turn.digest(),
        fragment(),
    )
    .unwrap();
    assert!(turn
        .clone()
        .with_provider_continuation(cross_digest)
        .is_err());

    let reordered = ProviderContinuation::new(
        protocol,
        ProviderContinuationReplayScope::InteractionV1,
        turn.digest(),
        vec![
            ProviderContinuationFragment::new(
                ProviderContinuationPosition::new(
                    1,
                    ProviderContinuationAttachment::InteractionStep(1),
                ),
                vec![1],
            ),
            ProviderContinuationFragment::new(
                ProviderContinuationPosition::new(
                    0,
                    ProviderContinuationAttachment::InteractionStep(0),
                ),
                vec![2],
            ),
        ],
    );
    assert!(reordered.is_err());

    let bound = turn
        .clone()
        .with_provider_continuation(
            ProviderContinuation::new(
                turn.provider_protocol().unwrap().clone(),
                ProviderContinuationReplayScope::AssistantTurnV1,
                turn.digest(),
                fragment(),
            )
            .unwrap(),
        )
        .unwrap();
    assert!(bound
        .with_reasoning(vec![ReasoningProjection::summary(
            ProviderContinuationPosition::new(0, ProviderContinuationAttachment::ContentBlock(0),),
            "changed reasoning projection",
        )])
        .is_err());
}

#[test]
fn generic_response_parsers_do_not_capture_reasoning_fields() {
    for (api_style, model, response_body) in [
        (
            AgentApiStyle::OpenAiCompatible,
            "gpt",
            json!({
                "choices": [{
                    "message": {
                        "role": "assistant",
                        "content": "visible",
                        "reasoning_content": "MUST_NOT_CAPTURE"
                    },
                    "finish_reason": "stop"
                }]
            }),
        ),
        (
            AgentApiStyle::AnthropicCompatible,
            "claude",
            json!({
                "content": [
                    {
                        "type": "thinking",
                        "thinking": "MUST_NOT_CAPTURE",
                        "signature": "MUST_NOT_CAPTURE_SIGNATURE"
                    },
                    { "type": "text", "text": "visible" }
                ],
                "stop_reason": "end_turn"
            }),
        ),
    ] {
        let protocol = generic_provider_protocol(api_style, model);
        let response = parse_non_stream_response(
            &response_body.to_string(),
            &protocol,
            LlmResponseValidation::RequireModelAction,
        )
        .unwrap();

        assert_eq!(response.content(), "visible");
        assert!(response.assistant_turn.reasoning().is_empty());
        assert!(response.assistant_turn.provider_continuation().is_none());
    }
}

#[test]
fn generic_request_adapters_do_not_send_reasoning_projection() {
    const CANARY: &str = "REASONING_PROJECTION_MUST_NOT_ENTER_GENERIC_WIRE";
    for (api_style, model) in [
        (AgentApiStyle::OpenAiCompatible, "gpt"),
        (AgentApiStyle::AnthropicCompatible, "claude"),
    ] {
        let profile = generic_provider_profile(api_style);
        let protocol = generic_provider_protocol(api_style, model);
        let turn = LlmAssistantTurn::from_provider(protocol.clone(), "visible", Vec::new())
            .unwrap()
            .with_reasoning(vec![ReasoningProjection::summary(
                ProviderContinuationPosition::new(
                    0,
                    ProviderContinuationAttachment::ContentBlock(0),
                ),
                CANARY,
            )])
            .unwrap();
        let request = LlmChatRequest {
            api_url: "https://example.test".to_string(),
            api_token: "token".to_string(),
            provider_profile: profile,
            provider_protocol: protocol,
            max_tokens: 100,
            temperature: 0.2,
            stream: false,
            messages: vec![LlmMessage::from_assistant_turn(turn)],
            tools: Vec::new(),
        };

        let encoded = serde_json::to_string(&build_payload(&request)).unwrap();
        assert!(!encoded.contains(CANARY));
        assert!(!encoded.contains("reasoning"));
        assert!(!encoded.contains("thinking"));
    }
}
