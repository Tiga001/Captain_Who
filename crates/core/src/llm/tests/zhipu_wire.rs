use super::*;
use crate::llm::provider_error::{LlmProviderFailure, LlmProviderFailureCategory};

fn profile(flash: bool, reasoning_effort: ProviderReasoningEffort) -> ProviderProfileConfig {
    let (reference, settings) = if flash {
        (
            ProviderProfileRef::zhipu_glm5_3_flash_chat(),
            ProviderFamilySettings::ZhipuGlm53FlashChat { reasoning_effort },
        )
    } else {
        (
            ProviderProfileRef::zhipu_glm5_3_chat(),
            ProviderFamilySettings::ZhipuGlm53Chat { reasoning_effort },
        )
    };
    ProviderProfileConfig::from_family_settings(reference, ProviderVendorId::Zhipu, settings)
}
fn request(model: &str) -> LlmChatRequest {
    let provider_profile = profile(model != "glm-5.3", ProviderReasoningEffort::Max);
    let provider_protocol = ProviderProtocolKey::new(
        AgentApiStyle::OpenAiCompatible.into(),
        &provider_profile,
        model,
        None,
    )
    .unwrap();
    LlmChatRequest {
        api_url: "https://open.bigmodel.test/api/paas/v4/chat/completions".into(),
        api_token: "test-token".into(),
        provider_profile,
        provider_protocol,
        max_tokens: None,
        temperature: 0.37,
        stream: true,
        messages: vec![message(LlmMessageRole::User, "hello")],
        tools: vec![tool_definition()],
    }
}
fn parse(request: &LlmChatRequest, message: Value, reason: &str) -> AgentResult<LlmChatResponse> {
    parse_non_stream_response_with_profile(
        &json!({"choices":[{"message":message,"finish_reason":reason}]}).to_string(),
        &request.provider_profile,
        &request.provider_protocol,
        LlmResponseValidation::RequireModelAction,
    )
}
fn call(id: &str, arguments: &str) -> Value {
    json!({"id":id, "type":"function", "function":{"name":"read_file", "arguments":arguments}})
}
fn bind(turn: LlmAssistantTurn, run: &str) -> (LlmAssistantTurn, Vec<LlmMessage>) {
    let mut results = Vec::new();
    let bindings = turn
        .provider_tool_calls()
        .iter()
        .enumerate()
        .map(|(index, call)| {
            let runtime = LlmToolCall {
                id: model_response_tool_call_id(run, 0, index, &call.id),
                name: call.name.clone(),
                args: call.args.clone(),
            };
            results.push(LlmMessage::tool_result(
                runtime.id.clone(),
                format!("result {index}"),
                false,
            ));
            LlmRuntimeToolCallBinding::new(index, call, runtime)
        })
        .collect();
    (turn.with_runtime_tool_bindings(bindings).unwrap(), results)
}
fn stream(
    request: &LlmChatRequest,
    frames: Vec<Value>,
) -> (AgentResult<LlmChatResponse>, Vec<LlmStreamEvent>) {
    let mut state =
        LlmStreamAccumulator::for_profile(&request.provider_profile, &request.provider_protocol)
            .unwrap();
    let mut events = Vec::new();
    for value in frames {
        if let Err(error) =
            process_sse_frame(&format!("data: {value}\n\n"), &mut state, &mut |event| {
                events.push(event)
            })
        {
            return (Err(error), events);
        }
    }
    process_sse_frame("data: [DONE]\n\n", &mut state, &mut |event| {
        events.push(event)
    })
    .unwrap();
    (state.finish(), events)
}

#[test]
fn zhipu_four_model_payloads_use_glm_settings_and_optional_budget() {
    for model in [
        "glm-5.3",
        "glm-5.3-flash",
        "glm-5.3-flashx",
        "glm-5.3-flashx-legal",
    ] {
        let mut request = request(model);
        let payload = build_payload(&request);
        assert_eq!(
            payload,
            json!({
                "model":model, "messages":[{"role":"user","content":"hello"}],
                "thinking":{"type":"enabled","clear_thinking":false}, "reasoning_effort":"max",
                "stream":true, "tool_stream":true, "tool_choice":"auto",
                "tools":[{"type":"function","function":{"name":"read_file","description":"Read a file.","parameters":{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}}}]
            })
        );
        request.max_tokens = Some(131072);
        request.stream = false;
        let payload = build_payload(&request);
        assert_eq!(payload["max_tokens"], 131072);
        assert!(payload.get("tool_stream").is_none());
        assert!(payload.get("temperature").is_none());
        assert!(payload.get("stream_options").is_none());
        assert!(payload.get("parallel_tool_calls").is_none());
    }
}

#[test]
fn zhipu_provider_default_effort_is_omitted_and_local_limits_are_checked() {
    for (effort, expected) in [
        (ProviderReasoningEffort::ProviderDefault, None),
        (ProviderReasoningEffort::Low, Some("low")),
        (ProviderReasoningEffort::High, Some("high")),
    ] {
        let mut req = request("glm-5.3");
        req.provider_profile = profile(false, effort);
        req.provider_protocol = ProviderProtocolKey::new(
            AgentApiStyle::OpenAiCompatible.into(),
            &req.provider_profile,
            "glm-5.3",
            None,
        )
        .unwrap();
        assert_eq!(
            build_payload(&req)
                .get("reasoning_effort")
                .and_then(Value::as_str),
            expected
        );
    }
    let mut req = request("glm-5.3");
    req.max_tokens = Some(131073);
    assert!(ProviderAdapterRegistry::resolve(&req).is_err());
    req.max_tokens = None;
    req.tools = (0..129).map(|_| tool_definition()).collect();
    assert!(ProviderAdapterRegistry::resolve(&req).is_err());
}

#[test]
fn zhipu_flagship_rejects_images_flash_sends_native_image_blocks() {
    for model in ["glm-5.3", "glm-5.3-flash", "glm-5.3-flashx-legal"] {
        let mut req = request(model);
        req.messages[0].images_mut().unwrap().push(LlmImage {
            mime_type: "image/png".into(),
            data_base64: "IMAGE_CANARY".into(),
        });
        if model == "glm-5.3" {
            assert!(ProviderAdapterRegistry::resolve(&req).is_err());
        } else {
            assert_eq!(
                build_payload(&req)["messages"][0]["content"][1]["image_url"]["url"],
                "data:image/png;base64,IMAGE_CANARY"
            );
        }
    }
}

#[test]
fn zhipu_preserved_turn_replays_exact_presence_and_raw_tool_arguments() {
    const PRIVATE: &str = "GLM_PRIVATE_REASONING_CANARY";
    let mut req = request("glm-5.3");
    let snapshot = json!({"role":"assistant", "content":null, "reasoning_content":PRIVATE,
        "tool_calls":[call("glm-1", "{ \"path\" : \"one.txt\" }"), call("glm-2", "{\n\"path\":\"two.txt\"\n}")]});
    let response = parse(&req, snapshot.clone(), "tool_calls").unwrap();
    assert!(response.content().is_empty());
    assert!(!format!("{response:?}").contains(PRIVATE));
    let (mut turn, results) = bind(response.assistant_turn, "glm-round");
    turn.set_runtime_visible_text("runtime narration must not replace provider content");
    req.messages = vec![LlmMessage::from_assistant_turn(turn)];
    req.messages.extend(results);
    let payload = build_payload(&req);
    assert_eq!(payload["messages"][0], snapshot);
    assert_eq!(payload["messages"][1]["tool_call_id"], "glm-1");
    assert_eq!(payload["messages"][2]["tool_call_id"], "glm-2");
}

#[test]
fn zhipu_ordinary_missing_null_and_empty_reasoning_remain_distinct() {
    for reasoning in [
        None,
        Some(Value::Null),
        Some(json!("")),
        Some(json!("private")),
    ] {
        let mut req = request("glm-5.3-flash");
        let mut snapshot = json!({"role":"assistant","content":"visible"});
        if let Some(reasoning) = reasoning {
            snapshot["reasoning_content"] = reasoning;
        }
        let response = parse(&req, snapshot.clone(), "stop").unwrap();
        req.messages = vec![
            LlmMessage::from_assistant_turn(response.assistant_turn),
            message(LlmMessageRole::User, "follow up"),
        ];
        assert_eq!(build_payload(&req)["messages"][0], snapshot);
    }
}

#[test]
fn zhipu_authenticated_snapshot_restore_is_required_and_protocol_bound() {
    let mut req = request("glm-5.3");
    let turn = parse(
        &req,
        json!({"content":"visible","reasoning_content":"secret"}),
        "stop",
    )
    .unwrap()
    .assistant_turn;
    let saved = turn.provider_continuation().unwrap().clone();
    let stripped = turn.without_raw_continuation_for_checkpoint();
    req.messages = vec![LlmMessage::from_assistant_turn(stripped.clone())];
    assert_eq!(
        ProviderAdapterRegistry::resolve(&req)
            .unwrap()
            .prepare_request(&req)
            .unwrap_err()
            .code(),
        Some("provider_context_boundary_required")
    );
    req.messages = vec![LlmMessage::from_assistant_turn(
        stripped.with_provider_continuation(saved.clone()).unwrap(),
    )];
    assert_eq!(
        build_payload(&req)["messages"][0]["reasoning_content"],
        "secret"
    );
    let modified =
        LlmAssistantTurn::from_provider(req.provider_protocol.clone(), "modified", vec![]).unwrap();
    assert!(modified.with_provider_continuation(saved).is_err());
    let mut other = request("glm-5.3-flash");
    other.messages = req.messages;
    assert!(ProviderAdapterRegistry::resolve(&other)
        .unwrap()
        .prepare_request(&other)
        .is_err());
}

#[test]
fn zhipu_grouped_replay_requires_all_results_and_defers_interstitial_state() {
    let mut req = request("glm-5.3");
    let response = parse(
        &req,
        json!({"reasoning_content":"r","tool_calls":[call("a","{}"),call("b","{}")] }),
        "tool_calls",
    )
    .unwrap();
    let (turn, mut results) = bind(response.assistant_turn, "grouped");
    req.messages = vec![
        LlmMessage::from_assistant_turn(turn),
        results.remove(0),
        message(LlmMessageRole::User, "steering"),
        results.remove(0),
    ];
    let payload = build_payload(&req);
    assert_eq!(payload["messages"][2]["tool_call_id"], "b");
    assert_eq!(payload["messages"][3]["content"], "steering");
    req.messages.pop();
    assert!(ProviderAdapterRegistry::resolve(&req)
        .unwrap()
        .prepare_request(&req)
        .is_err());
}

#[test]
fn zhipu_tool_turn_requires_reasoning_and_matching_finish() {
    let req = request("glm-5.3");
    for reasoning in [None, Some(Value::Null)] {
        let mut snapshot = json!({"tool_calls":[call("tool", "{}")]});
        if let Some(reasoning) = reasoning {
            snapshot["reasoning_content"] = reasoning;
        }
        assert_eq!(
            parse(&req, snapshot, "tool_calls").unwrap_err().code(),
            Some("provider_reasoning_required")
        );
    }
    assert!(parse(
        &req,
        json!({"reasoning_content":"","tool_calls":[call("tool", "{}")] }),
        "stop"
    )
    .is_err());
    assert!(parse(
        &req,
        json!({"content":"visible","reasoning_content":"r"}),
        "tool_calls"
    )
    .is_err());
}

#[test]
fn zhipu_sse_fragments_keep_exact_tool_bytes_private_reasoning_and_final_usage() {
    let mut req = request("glm-5.3-flashx");
    let (response, events) = stream(
        &req,
        vec![
            json!({"choices":[{"delta":{"reasoning_content":"PRIVATE "}}]}),
            json!({"choices":[{"delta":{"reasoning_content":"REASONING","content":null,"tool_calls":[{"index":0,"id":"sse-tool","type":"function","function":{"name":"read_file","arguments":"{ \"path\""}}]}}]}),
            json!({"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":" : \"one.txt\" }"}}]},"finish_reason":"tool_calls"}]}),
            json!({"choices":[],"usage":{"prompt_tokens":100,"completion_tokens":25,"completion_tokens_details":{"reasoning_tokens":20},"prompt_tokens_details":{"cached_tokens":64},"total_tokens":125}}),
        ],
    );
    let response = response.unwrap();
    assert_eq!(joined_text(&events), "");
    assert!(!format!("{events:?}{response:?}").contains("PRIVATE"));
    let usage = response.usage.unwrap();
    assert_eq!(usage.output_tokens, Some(25));
    assert_eq!(usage.output_thinking_tokens, Some(20));
    assert_eq!(usage.total_tokens, Some(125));
    assert_eq!(usage.cached_input_tokens, Some(64));
    assert_eq!(usage.cache_creation_input_tokens, None);
    let (turn, results) = bind(response.assistant_turn, "sse");
    req.messages = vec![LlmMessage::from_assistant_turn(turn)];
    req.messages.extend(results);
    let snapshot = &build_payload(&req)["messages"][0];
    assert_eq!(snapshot["reasoning_content"], "PRIVATE REASONING");
    assert!(snapshot["content"].is_null());
    assert_eq!(
        snapshot["tool_calls"][0]["function"]["arguments"],
        "{ \"path\" : \"one.txt\" }"
    );
}

#[test]
fn zhipu_stream_never_delivers_partial_or_abnormally_terminated_tools() {
    let req = request("glm-5.3");
    for reason in [
        None,
        Some("length"),
        Some("network_error"),
        Some("sensitive"),
        Some("model_context_window_exceeded"),
        Some("future_unknown"),
    ] {
        let mut final_choice = json!({"delta":{}});
        if let Some(reason) = reason {
            final_choice["finish_reason"] = json!(reason);
        }
        let (result, _) = stream(
            &req,
            vec![
                json!({"choices":[{"delta":{"reasoning_content":"r","tool_calls":[{"index":0,"id":"partial","type":"function","function":{"name":"read_file","arguments":"{\"path\":"}}]}}]}),
                json!({"choices":[final_choice],"usage":{"prompt_tokens":2,"completion_tokens":3}}),
            ],
        );
        let error = result.unwrap_err();
        assert_eq!(error.usage().unwrap().output_tokens, Some(3));
        if reason.is_none() {
            assert_eq!(error.code(), Some("agent.stream_interrupted"));
        }
        if reason == Some("length") {
            assert_eq!(error.code(), Some("agent.output_limit_reached"));
        }
        if reason == Some("network_error") {
            assert_eq!(error.details().unwrap()["retryable"], true);
        }
    }
    let (result, _) = stream(
        &req,
        vec![
            json!({"choices":[{"delta":{"reasoning_content":"r","tool_calls":[{"index":0,"id":"bad-json","type":"function","function":{"name":"read_file","arguments":"{"}}]},"finish_reason":"tool_calls"}]}),
        ],
    );
    assert!(result.is_err());
}

#[test]
fn zhipu_stream_rejects_changing_tool_identity_and_missing_indices() {
    let req = request("glm-5.3");
    for incoming in [
        json!({"index":0,"id":"changed"}),
        json!({"index":0,"type":"different"}),
        json!({"function":{"arguments":"}"}}),
    ] {
        let (result, _) = stream(
            &req,
            vec![
                json!({"choices":[{"delta":{"reasoning_content":"r","tool_calls":[{"index":0,"id":"original","type":"function","function":{"name":"read_file","arguments":"{"}}]}}]}),
                json!({"choices":[{"delta":{"tool_calls":[incoming]},"finish_reason":"tool_calls"}]}),
            ],
        );
        assert!(result.is_err());
    }
}

#[test]
fn zhipu_usage_keeps_missing_breakdown_unknown_and_ignores_foreign_counters() {
    let req = request("glm-5.3");
    let adapter = ProviderAdapterRegistry::resolve(&req).unwrap();
    let usage = adapter.project_usage(&req.provider_profile, &json!({"usage":{"prompt_tokens":100,"completion_tokens":30,"prompt_cache_miss_tokens":80,"cache_creation_input_tokens":40}})).unwrap();
    assert_eq!(usage.total_tokens, Some(130));
    assert_eq!(usage.cached_input_tokens, None);
    assert_eq!(usage.output_thinking_tokens, None);
    assert_eq!(usage.cache_creation_input_tokens, None);
    let invalid_subsets = adapter.project_usage(&req.provider_profile,&json!({"usage":{"prompt_tokens":1,"completion_tokens":2,"prompt_tokens_details":{"cached_tokens":3},"completion_tokens_details":{"reasoning_tokens":4}}})).unwrap();
    assert_eq!(invalid_subsets.cached_input_tokens, None);
    assert_eq!(invalid_subsets.output_thinking_tokens, None);
    let (result, _) = stream(
        &req,
        vec![json!({"choices":[{"delta":{"content":"visible"},"finish_reason":"stop"}]})],
    );
    assert!(result.unwrap().usage.is_none());
    let mut total = None;
    let semantics = adapter.capabilities().usage();
    semantics.merge_usage(&mut total, adapter.project_usage(&req.provider_profile, &json!({"usage":{"completion_tokens":10,"completion_tokens_details":{"reasoning_tokens":8}}})));
    semantics.merge_usage(&mut total, Some(usage));
    let total = total.unwrap();
    assert_eq!(total.output_tokens, Some(40));
    assert_eq!(total.output_thinking_tokens, None);
    assert_eq!(semantics.billable_output_tokens(&total), Some(40));
}

#[test]
fn zhipu_numeric_error_code_wins_over_http_and_openai_type() {
    let req = request("glm-5.3");
    for (code, category, retryable) in [
        (1113, LlmProviderFailureCategory::QuotaExhausted, false),
        (1302, LlmProviderFailureCategory::RateLimited, true),
        (1305, LlmProviderFailureCategory::Overloaded, true),
        (1309, LlmProviderFailureCategory::QuotaExhausted, false),
        (1311, LlmProviderFailureCategory::PermissionDenied, false),
        (1313, LlmProviderFailureCategory::PermissionDenied, false),
        (1315, LlmProviderFailureCategory::PermissionDenied, false),
        (1261, LlmProviderFailureCategory::ContextTooLarge, false),
        (1003, LlmProviderFailureCategory::Authentication, false),
        (1214, LlmProviderFailureCategory::InvalidRequest, false),
    ] {
        for code in [json!(code), json!(code.to_string())] {
            let body =
                json!({"error":{"code":code,"type":"rate_limit_error","message":"API_SECRET"}})
                    .to_string();
            let failure = LlmProviderFailure::from_http_response_for_protocol(
                &req.provider_protocol,
                reqwest::StatusCode::TOO_MANY_REQUESTS,
                &reqwest::header::HeaderMap::new(),
                &body,
                "API_SECRET",
            );
            assert_eq!(
                failure.raw.provider_code,
                Some(
                    code.as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| code.to_string())
                )
            );
            assert_eq!(failure.category, category);
            assert_eq!(failure.retryable, retryable);
            assert!(!failure.raw.body.contains("API_SECRET"));
        }
    }
}

#[test]
fn zhipu_stream_names_can_be_fragmented_and_terminal_deltas_are_rejected() {
    let req = request("glm-5.3");
    let (response, _) = stream(
        &req,
        vec![
            json!({"choices":[{"delta":{"reasoning_content":"r","tool_calls":[{"index":0,"id":"split-name","type":"function","function":{"name":"read_","arguments":"{"}}]}}]}),
            json!({"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"name":"file","arguments":"}"}}]},"finish_reason":"tool_calls"}]}),
        ],
    );
    assert_eq!(response.unwrap().provider_tool_calls()[0].name, "read_file");
    for delta in [
        json!({"content":"late"}),
        json!({"reasoning_content":"late"}),
        json!({"tool_calls":[{"index":0,"function":{"arguments":"{}"}}]}),
    ] {
        let (response, _) = stream(
            &req,
            vec![
                json!({"choices":[{"delta":{"content":"done"},"finish_reason":"stop"}]}),
                json!({"choices":[{"delta":delta}]}),
            ],
        );
        assert!(response.is_err());
    }
}

#[test]
fn zhipu_stream_usage_subsets_are_validated_after_cross_frame_merge() {
    let req = request("glm-5.3");
    let (response, _) = stream(
        &req,
        vec![
            json!({"choices":[],"usage":{"prompt_tokens_details":{"cached_tokens":80},"completion_tokens_details":{"reasoning_tokens":80}}}),
            json!({"choices":[{"delta":{"content":"done"},"finish_reason":"stop"}],"usage":{"prompt_tokens":50,"completion_tokens":50,"total_tokens":100}}),
        ],
    );
    let usage = response.unwrap().usage.unwrap();
    assert_eq!(usage.input_tokens, Some(50));
    assert_eq!(usage.output_tokens, Some(50));
    assert_eq!(usage.cached_input_tokens, None);
    assert_eq!(usage.output_thinking_tokens, None);
}

#[test]
fn zhipu_raw_argument_whitespace_is_reserved_in_continuation_budget() {
    let req = request("glm-5.3");
    let ordinary = parse(
        &req,
        json!({"reasoning_content":"r","tool_calls":[call("a","{}")]}),
        "tool_calls",
    )
    .unwrap();
    let raw = format!("{{{} }}", " ".repeat(16000));
    let spacious = parse(
        &req,
        json!({"reasoning_content":"r","tool_calls":[call("a",&raw)]}),
        "tool_calls",
    )
    .unwrap();
    let adapter = ProviderAdapterRegistry::resolve(&req).unwrap();
    let normal_tokens = adapter
        .estimate_continuation_tokens(ordinary.assistant_turn.provider_continuation())
        .unwrap();
    let spacious_tokens = adapter
        .estimate_continuation_tokens(spacious.assistant_turn.provider_continuation())
        .unwrap();
    assert!(spacious_tokens > normal_tokens + 1000);
}

#[test]
fn zhipu_final_invalid_usage_cannot_resurrect_stale_valid_subsets() {
    let req = request("glm-5.3");
    let (response, _) = stream(
        &req,
        vec![
            json!({"choices":[],"usage":{"prompt_tokens":100,"completion_tokens":100,"prompt_tokens_details":{"cached_tokens":10},"completion_tokens_details":{"reasoning_tokens":10}}}),
            json!({"choices":[{"delta":{"content":"done"},"finish_reason":"stop"}],"usage":{"prompt_tokens":50,"completion_tokens":50,"prompt_tokens_details":{"cached_tokens":80},"completion_tokens_details":{"reasoning_tokens":80}}}),
        ],
    );
    let usage = response.unwrap().usage.unwrap();
    assert_eq!(usage.input_tokens, Some(50));
    assert_eq!(usage.output_tokens, Some(50));
    assert_eq!(usage.cached_input_tokens, None);
    assert_eq!(usage.output_thinking_tokens, None);
}
