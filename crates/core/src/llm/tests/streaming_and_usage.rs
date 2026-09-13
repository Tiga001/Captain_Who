use super::*;

#[test]
fn openai_stream_accumulates_text_and_tool_calls() {
    let mut accumulator = LlmStreamAccumulator::new(AgentApiStyle::OpenAiCompatible);
    let mut deltas = Vec::new();

    process_sse_frame(
        &format!(
            "data: {}\n\n",
            json!({ "choices": [{ "delta": { "content": "Hel" } }] })
        ),
        &mut accumulator,
        &mut |delta| deltas.push(delta),
    )
    .unwrap();
    process_sse_frame(
        &format!(
            "data: {}\n\n",
            json!({ "choices": [{ "delta": { "content": "lo" } }] })
        ),
        &mut accumulator,
        &mut |delta| deltas.push(delta),
    )
    .unwrap();
    process_sse_frame(
        &format!(
            "data: {}\n\n",
            json!({
                "choices": [{
                    "delta": {
                        "tool_calls": [{
                            "index": 0,
                            "id": "call-1",
                            "type": "function",
                            "function": {
                                "name": "read_file",
                                "arguments": "{\"path\""
                            }
                        }]
                    }
                }]
            })
        ),
        &mut accumulator,
        &mut |delta| deltas.push(delta),
    )
    .unwrap();
    process_sse_frame(
        &format!(
            "data: {}\n\n",
            json!({
                "choices": [{
                    "delta": {
                        "tool_calls": [{
                            "index": 0,
                            "function": {
                                "arguments": ":\"src/lib.rs\"}"
                            }
                        }]
                    },
                    "finish_reason": "tool_calls"
                }]
            })
        ),
        &mut accumulator,
        &mut |delta| deltas.push(delta),
    )
    .unwrap();

    let response = accumulator.finish().unwrap();
    assert_eq!(joined_text(&deltas), "Hello");
    assert!(deltas.iter().any(|event| matches!(
        event,
        LlmStreamEvent::ToolInputProgress {
            tool,
            received_bytes,
            ..
        }
            if tool == "read_file" && *received_bytes > 0
    )));
    assert_eq!(response.content(), "Hello");
    assert_eq!(response.finish_reason, Some("tool_calls".to_string()));
    assert_eq!(response.provider_tool_calls()[0].id, "call-1");
    assert_eq!(response.provider_tool_calls()[0].name, "read_file");
    assert_eq!(response.provider_tool_calls()[0].args["path"], "src/lib.rs");
}

#[test]
fn anthropic_stream_accumulates_text_and_tool_calls() {
    let mut accumulator = LlmStreamAccumulator::new(AgentApiStyle::AnthropicCompatible);
    let mut deltas = Vec::new();

    process_sse_frame(
        &format!(
            "event: content_block_start\ndata: {}\n\n",
            json!({
                "type": "content_block_start",
                "index": 0,
                "content_block": { "type": "text", "text": "Hi" }
            })
        ),
        &mut accumulator,
        &mut |delta| deltas.push(delta),
    )
    .unwrap();
    process_sse_frame(
        &format!(
            "event: content_block_delta\ndata: {}\n\n",
            json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": { "type": "text_delta", "text": " there" }
            })
        ),
        &mut accumulator,
        &mut |delta| deltas.push(delta),
    )
    .unwrap();
    process_sse_frame(
        &format!(
            "event: content_block_start\ndata: {}\n\n",
            json!({
                "type": "content_block_start",
                "index": 1,
                "content_block": {
                    "type": "tool_use",
                    "id": "toolu-1",
                    "name": "search_files",
                    "input": {}
                }
            })
        ),
        &mut accumulator,
        &mut |delta| deltas.push(delta),
    )
    .unwrap();
    process_sse_frame(
        &format!(
            "event: content_block_delta\ndata: {}\n\n",
            json!({
                "type": "content_block_delta",
                "index": 1,
                "delta": {
                    "type": "input_json_delta",
                    "partial_json": "{\"query\":\"main\"}"
                }
            })
        ),
        &mut accumulator,
        &mut |delta| deltas.push(delta),
    )
    .unwrap();
    process_sse_frame(
        &format!(
            "event: message_delta\ndata: {}\n\n",
            json!({
                "type": "message_delta",
                "delta": { "stop_reason": "tool_use" },
                "usage": { "output_tokens": 8 }
            })
        ),
        &mut accumulator,
        &mut |delta| deltas.push(delta),
    )
    .unwrap();

    let response = accumulator.finish().unwrap();
    assert_eq!(joined_text(&deltas), "Hi there");
    assert!(deltas.iter().any(|event| matches!(
        event,
        LlmStreamEvent::ToolInputProgress {
            tool,
            received_bytes,
            ..
        }
            if tool == "search_files" && *received_bytes > 0
    )));
    assert_eq!(response.content(), "Hi there");
    assert_eq!(response.finish_reason, Some("tool_use".to_string()));
    assert_eq!(response.usage.as_ref().unwrap().output_tokens, Some(8));
    assert_eq!(response.provider_tool_calls()[0].id, "toolu-1");
    assert_eq!(response.provider_tool_calls()[0].name, "search_files");
    assert_eq!(response.provider_tool_calls()[0].args["query"], "main");
}

#[test]
fn extracts_openai_and_anthropic_usage() {
    let openai = json!({
        "usage": {
            "prompt_tokens": 7,
            "completion_tokens": 5,
            "total_tokens": 12,
            "prompt_tokens_details": {
                "cached_tokens": 2
            }
        }
    });
    let anthropic = json!({
        "usage": {
            "input_tokens": 3,
            "output_tokens": 4,
            "cache_read_input_tokens": 2,
            "cache_creation_input_tokens": 1
        }
    });

    let openai_usage = extract_usage(&openai).unwrap();
    assert_eq!(openai_usage.total_tokens, Some(12));
    assert_eq!(openai_usage.cached_input_tokens, Some(2));
    assert_eq!(openai_usage.billable_request_count, Some(1));

    let anthropic_usage = extract_usage(&anthropic).unwrap();
    assert_eq!(anthropic_usage.total_tokens, Some(7));
    assert_eq!(anthropic_usage.cached_input_tokens, Some(2));
    assert_eq!(anthropic_usage.cache_creation_input_tokens, Some(1));
    assert_eq!(anthropic_usage.billable_request_count, Some(1));
}

#[test]
fn provider_responses_without_usage_keep_token_counts_unknown() {
    let cases = [
        (
            AgentApiStyle::OpenAiCompatible,
            "gpt",
            json!({
                "choices": [{
                    "message": { "role": "assistant", "content": "done" },
                    "finish_reason": "stop"
                }]
            }),
        ),
        (
            AgentApiStyle::AnthropicCompatible,
            "claude",
            json!({
                "content": [{ "type": "text", "text": "done" }],
                "stop_reason": "end_turn"
            }),
        ),
    ];

    for (api_style, model, body) in cases {
        let protocol = generic_provider_protocol(api_style, model);
        let response = parse_non_stream_response(
            &body.to_string(),
            &protocol,
            LlmResponseValidation::RequireModelAction,
        )
        .unwrap();
        let usage = response.usage.expect("one request usage envelope");

        assert_eq!(usage.input_tokens, None);
        assert_eq!(usage.output_tokens, None);
        assert_eq!(usage.output_thinking_tokens, None);
        assert_eq!(usage.total_tokens, None);
        assert_eq!(usage.cached_input_tokens, None);
        assert_eq!(usage.cache_creation_input_tokens, None);
        assert_eq!(usage.billable_request_count, Some(1));
    }

    let deepseek_profile = deepseek_provider_profile(ReasoningMode::Enabled, ReasoningEffort::High);
    let deepseek_protocol = deepseek_provider_protocol(&deepseek_profile, "deepseek-flash");
    let deepseek = parse_non_stream_response_with_profile(
        &json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "done",
                    "reasoning_content": "private"
                },
                "finish_reason": "stop"
            }]
        })
        .to_string(),
        &deepseek_profile,
        &deepseek_protocol,
        LlmResponseValidation::RequireModelAction,
    )
    .unwrap();
    let usage = deepseek.usage.expect("one DeepSeek request usage envelope");
    assert_eq!(usage.input_tokens, None);
    assert_eq!(usage.output_tokens, None);
    assert_eq!(usage.output_thinking_tokens, None);
    assert_eq!(usage.total_tokens, None);
    assert_eq!(usage.cached_input_tokens, None);
    assert_eq!(usage.cache_creation_input_tokens, None);
    assert_eq!(usage.billable_request_count, Some(1));
}

#[test]
fn generic_adapters_project_complete_multi_tool_turn_to_split_wire_order() {
    for api_style in [
        AgentApiStyle::OpenAiCompatible,
        AgentApiStyle::AnthropicCompatible,
    ] {
        let model = if api_style == AgentApiStyle::OpenAiCompatible {
            "gpt"
        } else {
            "claude"
        };
        let profile = generic_provider_profile(api_style);
        let protocol = generic_provider_protocol(api_style, model);
        let provider_calls = vec![
            LlmToolCall {
                id: "provider-call-1".to_string(),
                name: "read_file".to_string(),
                args: json!({"path":"a.txt"}),
            },
            LlmToolCall {
                id: "provider-call-2".to_string(),
                name: "read_file".to_string(),
                args: json!({"path":"b.txt"}),
            },
        ];
        let runtime_calls = [
            LlmToolCall {
                id: "runtime-call-1".to_string(),
                name: "read_file".to_string(),
                args: json!({"path":"a.txt"}),
            },
            LlmToolCall {
                id: "runtime-call-2".to_string(),
                name: "read_file".to_string(),
                args: json!({"path":"b.txt"}),
            },
        ];
        let bindings = provider_calls
            .iter()
            .zip(runtime_calls.iter().cloned())
            .enumerate()
            .map(|(index, (provider_call, runtime_call))| {
                LlmRuntimeToolCallBinding::new(index, provider_call, runtime_call)
            })
            .collect();
        let turn = LlmAssistantTurn::from_provider(
            protocol.clone(),
            "I will read both files.",
            provider_calls,
        )
        .unwrap()
        .with_runtime_tool_bindings(bindings)
        .unwrap();
        let request = LlmChatRequest {
            api_url: "https://example.test".to_string(),
            api_token: "token".to_string(),
            provider_profile: profile,
            provider_protocol: protocol,
            max_tokens: 1024,
            temperature: 0.2,
            stream: false,
            messages: vec![
                LlmMessage::from_assistant_turn(turn),
                LlmMessage::tool_result("runtime-call-1", "a", false),
                LlmMessage::tool_result("runtime-call-2", "b", false),
            ],
            tools: Vec::new(),
        };

        let payload = build_payload(&request);
        let messages = payload["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 4);
        assert_eq!(messages[0]["role"], "assistant");
        assert_eq!(messages[2]["role"], "assistant");
        if api_style == AgentApiStyle::OpenAiCompatible {
            assert_eq!(messages[0]["content"], "I will read both files.");
            assert_eq!(messages[0]["tool_calls"][0]["id"], "runtime-call-1");
            assert_eq!(messages[1]["role"], "tool");
            assert_eq!(messages[1]["tool_call_id"], "runtime-call-1");
            assert!(messages[2]["content"].is_null());
            assert_eq!(messages[2]["tool_calls"][0]["id"], "runtime-call-2");
            assert_eq!(messages[3]["tool_call_id"], "runtime-call-2");
        } else {
            assert_eq!(messages[0]["content"][0]["text"], "I will read both files.");
            assert_eq!(messages[0]["content"][1]["id"], "runtime-call-1");
            assert_eq!(messages[1]["content"][0]["tool_use_id"], "runtime-call-1");
            assert_eq!(messages[2]["content"][0]["id"], "runtime-call-2");
            assert_eq!(messages[3]["content"][0]["tool_use_id"], "runtime-call-2");
        }
    }
}

#[test]
fn generic_adapters_project_interleaved_image_and_runtime_extension_to_legal_wire() {
    for api_style in [
        AgentApiStyle::OpenAiCompatible,
        AgentApiStyle::AnthropicCompatible,
    ] {
        let model = if api_style == AgentApiStyle::OpenAiCompatible {
            "gpt"
        } else {
            "claude"
        };
        let profile = generic_provider_profile(api_style);
        let protocol = generic_provider_protocol(api_style, model);
        let provider_calls = vec![
            LlmToolCall {
                id: "provider-call-1".to_string(),
                name: "read_file".to_string(),
                args: json!({"path":"one.png"}),
            },
            LlmToolCall {
                id: "provider-call-2".to_string(),
                name: "read_file".to_string(),
                args: json!({"path":"two.txt"}),
            },
        ];
        let runtime_calls = provider_calls
            .iter()
            .enumerate()
            .map(|(index, provider_call)| LlmToolCall {
                id: model_response_tool_call_id(
                    "interleaved-projection-run",
                    0,
                    index,
                    &provider_call.id,
                ),
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
        let turn = LlmAssistantTurn::from_provider(
            protocol.clone(),
            "I will inspect both files.",
            provider_calls,
        )
        .unwrap()
        .with_runtime_tool_bindings(bindings)
        .unwrap();
        let mut image_context = LlmMessage::text(
            LlmMessageRole::User,
            "Image emitted after the first tool result.",
        );
        image_context.images_mut().unwrap().push(LlmImage {
            mime_type: "image/png".to_string(),
            data_base64: "AA==".to_string(),
        });
        let runtime_extension = LlmMessage::text(
            LlmMessageRole::Assistant,
            "Runtime extension state updated.",
        );
        let request = LlmChatRequest {
            api_url: "https://example.test".to_string(),
            api_token: "token".to_string(),
            provider_profile: profile,
            provider_protocol: protocol,
            max_tokens: 1024,
            temperature: 0.2,
            stream: false,
            messages: vec![
                LlmMessage::from_assistant_turn(turn),
                LlmMessage::tool_result(runtime_calls[0].id.clone(), "first result", false),
                image_context,
                runtime_extension,
                LlmMessage::tool_result(runtime_calls[1].id.clone(), "second result", false),
            ],
            tools: Vec::new(),
        };

        assert!(validate_model_tool_protocol(&request.messages).is_err());
        validate_request(&request).unwrap();
        let payload = build_payload(&request);
        let messages = payload["messages"].as_array().unwrap();

        if api_style == AgentApiStyle::OpenAiCompatible {
            assert_eq!(messages.len(), 6);
            assert_eq!(messages[0]["tool_calls"][0]["id"], runtime_calls[0].id);
            assert_eq!(messages[1]["tool_call_id"], runtime_calls[0].id);
            assert_eq!(messages[2]["role"], "user");
            assert_eq!(messages[2]["content"][0]["type"], "text");
            assert_eq!(messages[2]["content"][1]["type"], "image_url");
            assert_eq!(messages[3]["content"], "Runtime extension state updated.");
            assert_eq!(messages[4]["tool_calls"][0]["id"], runtime_calls[1].id);
            assert_eq!(messages[5]["tool_call_id"], runtime_calls[1].id);
        } else {
            assert_eq!(messages.len(), 4);
            assert_eq!(messages[0]["content"][1]["id"], runtime_calls[0].id);
            assert_eq!(
                messages[1]["content"][0]["tool_use_id"],
                runtime_calls[0].id
            );
            assert_eq!(messages[1]["content"][1]["type"], "text");
            assert_eq!(messages[1]["content"][2]["type"], "image");
            assert_eq!(
                messages[2]["content"][0]["text"],
                "Runtime extension state updated."
            );
            assert_eq!(messages[2]["content"][1]["id"], runtime_calls[1].id);
            assert_eq!(
                messages[3]["content"][0]["tool_use_id"],
                runtime_calls[1].id
            );
        }
    }
}

#[test]
fn generic_adapters_preserve_grouped_multi_tool_split_wire_shape() {
    for api_style in [
        AgentApiStyle::OpenAiCompatible,
        AgentApiStyle::AnthropicCompatible,
    ] {
        let model = if api_style == AgentApiStyle::OpenAiCompatible {
            "gpt"
        } else {
            "claude"
        };
        let calls = vec![
            LlmToolCall {
                id: "call-1".to_string(),
                name: "read_file".to_string(),
                args: json!({"path":"a.txt"}),
            },
            LlmToolCall {
                id: "call-2".to_string(),
                name: "read_file".to_string(),
                args: json!({"path":"b.txt"}),
            },
        ];
        let request = LlmChatRequest {
            api_url: "https://example.test".to_string(),
            api_token: "token".to_string(),
            provider_profile: generic_provider_profile(api_style),
            provider_protocol: generic_provider_protocol(api_style, model),
            max_tokens: 1024,
            temperature: 0.2,
            stream: false,
            messages: vec![
                LlmMessage::assistant("I will read both files.", calls),
                LlmMessage::tool_result("call-1", "a", false),
                LlmMessage::tool_result("call-2", "b", false),
            ],
            tools: Vec::new(),
        };

        let payload = build_payload(&request);
        let messages = payload["messages"].as_array().unwrap();
        if api_style == AgentApiStyle::OpenAiCompatible {
            assert_eq!(messages.len(), 3);
            assert_eq!(messages[0]["content"], "I will read both files.");
            assert_eq!(messages[0]["tool_calls"].as_array().unwrap().len(), 2);
            assert_eq!(messages[0]["tool_calls"][0]["id"], "call-1");
            assert_eq!(messages[0]["tool_calls"][1]["id"], "call-2");
            assert_eq!(messages[1]["tool_call_id"], "call-1");
            assert_eq!(messages[2]["tool_call_id"], "call-2");
        } else {
            assert_eq!(messages.len(), 2);
            assert_eq!(messages[0]["content"][0]["text"], "I will read both files.");
            assert_eq!(messages[0]["content"][1]["id"], "call-1");
            assert_eq!(messages[0]["content"][2]["id"], "call-2");
            assert_eq!(messages[1]["content"][0]["tool_use_id"], "call-1");
            assert_eq!(messages[1]["content"][1]["tool_use_id"], "call-2");
        }
    }
}
