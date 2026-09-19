use super::*;
use crate::provider_profile::MoonshotK26ThinkingMode;

fn optional_budget_profiles() -> Vec<(ProviderProfileConfig, &'static str, &'static str)> {
    let mut profiles = vec![
        (
            generic_provider_profile(AgentApiStyle::OpenAiCompatible),
            "generic-model",
            "max_tokens",
        ),
        (
            ProviderProfileConfig::deepseek_flash_default(),
            "deepseek-flash",
            "max_tokens",
        ),
        (
            ProviderProfileConfig::deepseek_pro_default(),
            "deepseek-v4-pro",
            "max_tokens",
        ),
        (
            ProviderProfileConfig::from_family_settings(
                ProviderProfileRef::moonshot_k2_7_code_chat(),
                ProviderVendorId::Moonshot,
                ProviderFamilySettings::MoonshotK27CodeChat,
            ),
            "kimi-k2.7-code",
            "max_completion_tokens",
        ),
    ];
    for (mode, effort) in [
        (ReasoningMode::Disabled, ReasoningEffort::ProviderDefault),
        (ReasoningMode::Enabled, ReasoningEffort::High),
        (ReasoningMode::Enabled, ReasoningEffort::Max),
    ] {
        profiles.push((
            deepseek_provider_profile(mode, effort),
            "deepseek-flash",
            "max_tokens",
        ));
    }
    for reasoning_effort in [
        ProviderReasoningEffort::ProviderDefault,
        ProviderReasoningEffort::Low,
        ProviderReasoningEffort::High,
        ProviderReasoningEffort::Max,
    ] {
        profiles.push((
            ProviderProfileConfig::from_family_settings(
                ProviderProfileRef::moonshot_k3_chat(),
                ProviderVendorId::Moonshot,
                ProviderFamilySettings::MoonshotK3Chat { reasoning_effort },
            ),
            "kimi-k3",
            "max_completion_tokens",
        ));
    }
    for thinking_mode in [
        MoonshotK26ThinkingMode::ProviderDefault,
        MoonshotK26ThinkingMode::Disabled,
        MoonshotK26ThinkingMode::Enabled,
        MoonshotK26ThinkingMode::EnabledKeepAll,
    ] {
        profiles.push((
            ProviderProfileConfig::from_family_settings(
                ProviderProfileRef::moonshot_k2_6_chat(),
                ProviderVendorId::Moonshot,
                ProviderFamilySettings::MoonshotK26Chat { thinking_mode },
            ),
            "kimi-k2.6",
            "max_completion_tokens",
        ));
    }
    profiles
}

#[test]
fn optional_output_limits_preserve_each_protocols_stream_tools_and_reasoning_fields() {
    for (profile, model, limit_field) in optional_budget_profiles() {
        for stream in [false, true] {
            for with_tools in [false, true] {
                let mut request = request_with_messages(vec![message(LlmMessageRole::User, "Hi")]);
                request.provider_protocol = ProviderProtocolKey::new(
                    AgentApiStyle::OpenAiCompatible.into(),
                    &profile,
                    model,
                    None,
                )
                .unwrap();
                request.provider_profile = profile.clone();
                request.stream = stream;
                if !with_tools {
                    request.tools.clear();
                }
                request.max_tokens = None;
                let omitted = build_payload(&request);
                for forbidden in ["max_tokens", "max_completion_tokens", "max_output_tokens"] {
                    assert!(omitted.get(forbidden).is_none(), "{model} sent {forbidden}");
                }
                assert_eq!(omitted["stream"], stream);
                assert_eq!(omitted.get("tools").is_some(), with_tools);
                if stream {
                    assert_eq!(omitted["stream_options"]["include_usage"], true);
                }

                // Explicit snapshots/internal requests retain their exact limit, including
                // limits above the old universal 128,000 clamp. No fallback field is added.
                for limit in [30_000, 180_000] {
                    request.max_tokens = Some(limit);
                    let mut explicit = build_payload(&request);
                    assert_eq!(explicit[limit_field], limit, "{model}");
                    explicit.as_object_mut().unwrap().remove(limit_field);
                    assert_eq!(
                        explicit, omitted,
                        "output policy changed {model} wire fields"
                    );
                }
            }
        }
    }
}

#[test]
fn anthropic_requires_a_positive_explicit_output_limit_without_a_silent_fallback() {
    let mut request = request_with_messages(vec![message(LlmMessageRole::User, "Hi")]);
    request.provider_profile = generic_provider_profile(AgentApiStyle::AnthropicCompatible);
    request.provider_protocol =
        generic_provider_protocol(AgentApiStyle::AnthropicCompatible, "claude");
    for limit in [None, Some(0)] {
        request.max_tokens = limit;
        let error = super::super::payload::build_anthropic_payload(&request).unwrap_err();
        assert_eq!(error.code(), Some("agent.invalid_output_budget"));
    }
    for stream in [false, true] {
        request.stream = stream;
        for limit in [30_000, 180_000] {
            request.max_tokens = Some(limit);
            let payload = build_payload(&request);
            assert_eq!(payload["max_tokens"], limit);
            assert_eq!(payload["tools"][0]["name"], "read_file");
            assert_eq!(
                payload.get("stream").and_then(Value::as_bool),
                stream.then_some(true)
            );
            assert!(payload.get("max_completion_tokens").is_none());
        }
    }
}

#[tokio::test]
async fn omitted_output_limit_survives_the_http_boundary_and_allows_large_responses() {
    for stream_response in [false, true] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let visible = "visible ".repeat(40_001);
        let server_visible = visible.clone();
        let server = tokio::spawn(async move {
            let (mut connection, _) = listener.accept().await.unwrap();
            let payload = read_test_http_request_json(&mut connection).await;
            assert!(payload.get("max_tokens").is_none());
            assert!(payload.get("max_completion_tokens").is_none());
            assert_eq!(payload["stream"], stream_response);
            if stream_response {
                let frame = json!({
                    "choices": [{"delta": {"content": server_visible}, "finish_reason": "stop"}],
                    "usage": {"prompt_tokens": 7, "completion_tokens": 40_001, "total_tokens": 40_008}
                });
                let body = format!("data: {frame}\n\ndata: [DONE]\n\n");
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                connection.write_all(response.as_bytes()).await.unwrap();
            } else {
                write_test_http_response(
                    &mut connection,
                    "200 OK",
                    json!({
                        "choices": [{"message": {"role": "assistant", "content": server_visible}, "finish_reason": "stop"}],
                        "usage": {"prompt_tokens": 7, "completion_tokens": 40_001, "total_tokens": 40_008}
                    }),
                )
                .await;
            }
        });
        let mut request = request_with_messages(vec![message(LlmMessageRole::User, "Hi")]);
        request.api_url = format!("http://{address}/v1/chat/completions");
        request.max_tokens = None;
        let response = if stream_response {
            let mut deltas = Vec::new();
            let response =
                complete_chat_streaming(request, AgentCancellationToken::new(), |delta| {
                    deltas.push(delta)
                })
                .await
                .unwrap();
            assert_eq!(joined_text(&deltas), visible);
            response
        } else {
            complete_chat(request, AgentCancellationToken::new())
                .await
                .unwrap()
        };
        server.await.unwrap();
        assert_eq!(response.content(), visible);
        assert_eq!(response.finish_reason.as_deref(), Some("stop"));
        assert_eq!(response.usage.unwrap().output_tokens, Some(40_001));
    }
}
