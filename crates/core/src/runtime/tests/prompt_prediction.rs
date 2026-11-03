use super::*;

fn reply(text: &str) -> Value {
    json!({"choices": [{"message": {"role": "assistant", "content": text}, "finish_reason": "stop"}],
        "usage": {"prompt_tokens": 12, "completion_tokens": 6, "total_tokens": 18}})
}

#[tokio::test]
async fn hot_prompt_prediction_preserves_wire_prefix_and_reports_usage() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!(
        "http://{}/v1/chat/completions",
        listener.local_addr().unwrap()
    );
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let normal = read_runtime_test_json_request(&mut socket).await;
        write_runtime_test_json_response(&mut socket, reply("改动已经完成。")).await;
        let (mut socket, _) = listener.accept().await.unwrap();
        let predicted = read_runtime_test_json_request(&mut socket).await;
        write_runtime_test_json_response(&mut socket, reply(r#"{"suggestion":"再跑一遍测试。"}"#))
            .await;
        (normal, predicted)
    });
    let mut input = conversation_context_input(vec![message("user", "修复错误。")]);
    input.api_token = "mock".into();
    input.api_url.clone_from(&url);
    input.stream = Some(true);
    let seed = Arc::new(Mutex::new(None));
    let captured = seed.clone();
    let services = AgentRuntimeHostServices::new().with_prompt_prediction_seed_observer(Arc::new(
        move |value| {
            *captured.lock().unwrap() = Some(value);
        },
    ));
    let output = AgentRuntime::default()
        .send_chat_with_events_and_cancellation(
            input,
            None,
            None,
            AgentCancellationToken::new(),
            Some(services),
        )
        .await
        .unwrap();
    assert_eq!(output.content, "改动已经完成。");
    let seed = seed.lock().unwrap().take().unwrap();
    assert!(seed.estimated_bytes() > 100);
    assert!(seed
        .predict_observed(
            "https://different.invalid",
            "mock",
            AgentCancellationToken::new(),
            || panic!("endpoint mismatch must never dispatch"),
        )
        .await
        .is_err());
    let cancelled = AgentCancellationToken::new();
    cancelled.cancel();
    assert!(seed
        .predict_observed(&url, "mock", cancelled, || panic!(
            "cancelled request dispatched"
        ))
        .await
        .unwrap_err()
        .is_cancelled());
    let dispatch_count = std::sync::atomic::AtomicUsize::new(0);
    let prediction = seed
        .predict_observed(&url, "mock", AgentCancellationToken::new(), || {
            dispatch_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        })
        .await
        .unwrap();
    assert_eq!(dispatch_count.load(std::sync::atomic::Ordering::Relaxed), 1);
    assert_eq!(prediction.suggestion.as_deref(), Some("再跑一遍测试。"));
    assert_eq!(prediction.usage.unwrap().billable_request_count, Some(1));
    let (normal, predicted) = server.await.unwrap();
    assert_eq!(normal["tools"], predicted["tools"]);
    assert_eq!(normal["model"], predicted["model"]);
    assert_eq!(normal["stream"], predicted["stream"]);
    let original = normal["messages"].as_array().unwrap();
    let branched = predicted["messages"].as_array().unwrap();
    assert_eq!(&branched[..original.len()], original);
    assert_eq!(branched.len(), original.len() + 2);
    assert_eq!(branched[original.len()]["role"], "assistant");
    assert_eq!(branched[original.len()]["content"], "改动已经完成。");
    assert_eq!(branched.last().unwrap()["role"], "user");
}

#[tokio::test]
async fn cold_prompt_prediction_uses_durable_answer_and_discards_tool_output() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!(
        "http://{}/v1/chat/completions",
        listener.local_addr().unwrap()
    );
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let request = read_runtime_test_json_request(&mut socket).await;
        write_runtime_test_json_response(&mut socket, json!({"choices":[{"message":{"role":"assistant",
            "content":r#"{"suggestion":"wrong"}"#,
            "tool_calls":[{"id":"ignored","type":"function","function":{"name":"run_command","arguments":"{}"}}]},
            "finish_reason":"tool_calls"}]})).await;
        request
    });
    let mut input = conversation_context_input(vec![
        message("user", "What changed?"),
        current_assistant_history_message("Durable final answer."),
    ]);
    input.api_url.clone_from(&url);
    input.api_token = "not-retained".into();
    input.stream = Some(false);
    input.context = Some(AgentRunContext {
        conversation_id: Some("conversation-context".into()),
        project_id: None,
        workspace: None,
        attachment_library: None,
        permissions: AgentPermissions::default(),
        collaboration_identity: None,
    });
    let host = AgentRuntimeHostServices::new();
    let projection = prepare_context_window_tool_projection(&input, &host, true).unwrap();
    let seed =
        prepare_prompt_prediction_seed(input, "conversation-context", &host, &projection).unwrap();
    let result = seed
        .predict(&url, "mock", AgentCancellationToken::new())
        .await
        .unwrap();
    assert_eq!(result.suggestion, None);
    assert_eq!(result.usage.unwrap().billable_request_count, Some(1));
    let wire = server.await.unwrap();
    assert!(wire["tools"]
        .as_array()
        .is_some_and(|tools| !tools.is_empty()));
    assert!(wire["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|m| m["role"] == "assistant"
            && m["content"]
                .as_str()
                .unwrap_or_default()
                .contains("Durable final answer.")));
}

#[tokio::test]
async fn prompt_prediction_continues_deepseek_private_turn_without_exposing_it() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!(
        "http://{}/v1/chat/completions",
        listener.local_addr().unwrap()
    );
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let _ = read_runtime_test_json_request(&mut socket).await;
        let mut answer = reply("Keep the current profile.");
        answer["choices"][0]["message"]["reasoning_content"] = json!("private-prefix-token");
        write_runtime_test_json_response(&mut socket, answer).await;
        let (mut socket, _) = listener.accept().await.unwrap();
        let wire = read_runtime_test_json_request(&mut socket).await;
        let mut prediction = reply(r#"{"suggestion":null}"#);
        prediction["choices"][0]["message"]["reasoning_content"] = json!("private-branch-token");
        prediction["usage"]["prompt_cache_hit_tokens"] = json!(7);
        write_runtime_test_json_response(&mut socket, prediction).await;
        wire
    });
    let input = conversation_context_input(vec![message("user", "Inspect the profile")]);
    let tools = prepare_conversation_context(&input)
        .unwrap()
        .tool_definitions;
    assert!(!tools.is_empty());
    let profile =
        deepseek_test_profile(crate::ReasoningMode::Enabled, crate::ReasoningEffort::High);
    let protocol = ProviderProtocolKey::new(
        ProviderProtocolDialect::OpenAiChatCompletions,
        &profile,
        "deepseek-flash",
        None,
    )
    .unwrap();
    let request = LlmChatRequest {
        api_url: url.clone(),
        api_token: "local-test".into(),
        provider_profile: profile,
        provider_protocol: protocol,
        max_tokens: Some(1024),
        temperature: 0.2,
        stream: false,
        messages: vec![LlmMessage::text(
            LlmMessageRole::User,
            "Inspect the profile",
        )],
        tools,
    };
    let response = crate::llm::complete_chat(request.clone(), AgentCancellationToken::new())
        .await
        .unwrap();
    let seed = AgentPromptPredictionSeed::from_final_request(
        request,
        response.assistant_turn,
        Some(128_000),
    );
    let result = seed
        .predict(&url, "local-test", AgentCancellationToken::new())
        .await
        .unwrap();
    assert_eq!(result.suggestion, None);
    assert_eq!(result.usage.unwrap().cached_input_tokens, Some(7));
    let wire = server.await.unwrap();
    assert_eq!(
        wire["messages"][1]["reasoning_content"],
        "private-prefix-token"
    );
    assert_eq!(wire["messages"][1]["content"], "Keep the current profile.");
}

#[tokio::test]
async fn prompt_prediction_retryable_http_failure_dispatches_once() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::AsyncWriteExt;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!(
        "http://{}/v1/chat/completions",
        listener.local_addr().unwrap()
    );
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        read_runtime_test_json_request(&mut socket).await;
        socket.write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await.unwrap();
        // Dropping the listener also ensures an erroneous retry cannot hang the test.
    });
    let profile =
        ProviderProfileConfig::generic_for_dialect(ProviderProtocolDialect::OpenAiChatCompletions);
    let protocol = ProviderProtocolKey::new(
        ProviderProtocolDialect::OpenAiChatCompletions,
        &profile,
        "test-model",
        None,
    )
    .unwrap();
    let seed = AgentPromptPredictionSeed::from_final_request(
        LlmChatRequest {
            api_url: url.clone(),
            api_token: String::new(),
            provider_profile: profile,
            provider_protocol: protocol,
            max_tokens: None,
            temperature: 0.2,
            stream: false,
            messages: vec![LlmMessage::text(LlmMessageRole::User, "test")],
            tools: vec![],
        },
        crate::llm::LlmAssistantTurn::from_split_projection("done", vec![]),
        Some(128_000),
    );
    let sent = AtomicUsize::new(0);
    let error = seed
        .predict_observed(&url, "mock", AgentCancellationToken::new(), || {
            sent.fetch_add(1, Ordering::Relaxed);
        })
        .await
        .unwrap_err();
    assert_eq!(sent.load(Ordering::Relaxed), 1);
    assert_eq!(
        error.usage().and_then(|usage| usage.billable_request_count),
        Some(1)
    );
    server.await.unwrap();
}
