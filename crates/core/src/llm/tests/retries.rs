use super::*;

#[test]
fn retry_delay_uses_capped_exponential_backoff() {
    assert_eq!(retry_delay(1), Duration::from_millis(350));
    assert_eq!(retry_delay(2), Duration::from_millis(700));
    assert_eq!(retry_delay(10), Duration::from_millis(2_000));
}

#[test]
fn classifies_transient_llm_errors_as_retryable() {
    let transport =
        LlmProviderFailure::from_local_transport_failure("connection reset").to_agent_error();
    let rate_limited = LlmProviderFailure::from_http_response(
        AgentApiStyle::OpenAiCompatible,
        reqwest::StatusCode::TOO_MANY_REQUESTS,
        &reqwest::header::HeaderMap::new(),
        r#"{"error":{"code":"API_KEY_RATE_LIMIT_EXCEEDED"}}"#,
        "",
    )
    .to_agent_error();
    let overloaded = LlmProviderFailure::from_http_response(
        AgentApiStyle::OpenAiCompatible,
        reqwest::StatusCode::SERVICE_UNAVAILABLE,
        &reqwest::header::HeaderMap::new(),
        r#"{"error":{"type":"overloaded_error"}}"#,
        "",
    )
    .to_agent_error();
    let invalid_stream_arguments = AgentError::structured(
        INVALID_STREAM_TOOL_ARGUMENTS_ERROR_CODE,
        "模型返回的流式工具参数不完整或格式无效。",
        json!({
            "type": "invalid_stream_tool_arguments",
            "retryable": true,
            "providerProtocol": "openai",
            "parseCategory": "eof",
        }),
    );

    assert!(is_retryable_llm_error(&transport));
    assert!(is_retryable_llm_error(&rate_limited));
    assert!(is_retryable_llm_error(&overloaded));
    assert!(is_retryable_llm_error(&invalid_stream_arguments));
}

#[test]
fn classifies_configuration_and_client_errors_as_non_retryable() {
    assert!(!is_retryable_llm_error(&AgentError::new(
        "请先在设置 > 配置里填写 API Token。"
    )));
    assert!(!is_retryable_llm_error(&AgentError::new(
        "模型接口返回 401：unauthorized"
    )));
    assert!(!is_retryable_llm_error(&AgentError::new(
        "模型接口返回 400：bad request"
    )));
    assert!(!is_retryable_llm_error(&AgentError::new(
        "模型接口返回 400：Invalid schema for function read_file"
    )));
}

#[tokio::test]
async fn streaming_retries_the_known_upstream_content_type_400_and_recovers() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        for attempt in 1..=2 {
            let (mut stream, _) = listener.accept().await.unwrap();
            read_test_http_request(&mut stream).await;
            if attempt == 1 {
                write_test_http_response(
                        &mut stream,
                        "400 Bad Request",
                        json!({
                            "error": {
                                "message": "upstream status 400: Provider API error: The provided Content Type is invalid or not supported for this model"
                            }
                        }),
                    )
                    .await;
            } else {
                write_test_http_response(
                    &mut stream,
                    "200 OK",
                    json!({
                        "choices": [{
                            "message": { "role": "assistant", "content": "recovered" },
                            "finish_reason": "stop"
                        }],
                        "usage": {
                            "prompt_tokens": 10,
                            "completion_tokens": 1,
                            "total_tokens": 11
                        }
                    }),
                )
                .await;
            }
        }
    });
    let request = LlmChatRequest {
        api_url: format!("http://{address}/v1/chat/completions"),
        api_token: "token".to_string(),
        provider_profile: generic_provider_profile(AgentApiStyle::OpenAiCompatible),
        provider_protocol: generic_provider_protocol(
            AgentApiStyle::OpenAiCompatible,
            "claude-opus-4-7",
        ),
        max_tokens: 1_024,
        temperature: 0.2,
        stream: false,
        messages: vec![message(LlmMessageRole::User, "Hello")],
        tools: Vec::new(),
    };

    let mut attempts_started = 0_usize;
    let mut attempts_reset = 0_usize;
    let mut retries = 0_usize;
    let mut commits = 0_usize;
    let response =
        complete_chat_streaming(
            request,
            AgentCancellationToken::new(),
            |event| match event {
                LlmStreamEvent::AttemptStarted { .. } => attempts_started += 1,
                LlmStreamEvent::AttemptReset { .. } => attempts_reset += 1,
                LlmStreamEvent::Retrying { .. } => retries += 1,
                LlmStreamEvent::Committed => commits += 1,
                _ => {}
            },
        )
        .await
        .unwrap();
    server.await.unwrap();

    assert_eq!(response.content(), "recovered");
    assert_eq!(attempts_started, 2);
    assert_eq!(attempts_reset, 1);
    assert_eq!(retries, 1);
    assert_eq!(commits, 1);
    assert_eq!(
        response
            .usage
            .as_ref()
            .and_then(|usage| usage.billable_request_count),
        Some(2)
    );
}

#[tokio::test]
async fn qizhen_429_retries_from_structured_code_and_recovers() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        for attempt in 1..=2 {
            let (mut stream, _) = listener.accept().await.unwrap();
            read_test_http_request(&mut stream).await;
            if attempt == 1 {
                write_test_http_response_with_headers(
                    &mut stream,
                    "429 Too Many Requests",
                    &[("Retry-After", "0"), ("X-Request-Id", "qizhen-fixture")],
                    json!({
                        "error": {
                            "code": "API_KEY_RATE_LIMIT_EXCEEDED",
                            "type": "RATE_LIMIT",
                            "message": "请求限流超限"
                        }
                    }),
                )
                .await;
            } else {
                write_test_http_response(
                    &mut stream,
                    "200 OK",
                    json!({
                        "choices": [{
                            "message": { "role": "assistant", "content": "recovered" },
                            "finish_reason": "stop"
                        }]
                    }),
                )
                .await;
            }
        }
    });
    let mut request = request_with_messages(vec![message(LlmMessageRole::User, "Hello")]);
    request.api_url = format!("http://{address}/v1/chat/completions");
    request.api_token = "qizhen-retry-token".to_string();
    request.stream = false;

    let response = complete_chat(request, AgentCancellationToken::new())
        .await
        .unwrap();
    server.await.unwrap();
    assert_eq!(response.content(), "recovered");
    assert_eq!(
        response
            .usage
            .as_ref()
            .and_then(|usage| usage.billable_request_count),
        Some(2)
    );
}

#[tokio::test]
async fn cancellation_during_rate_limit_backoff_prevents_the_second_request() {
    const CANARY: &str = "RATE_LIMIT_BODY_SECRET_CANARY";
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_test_http_request(&mut stream).await;
        write_test_http_response_with_headers(
            &mut stream,
            "429 Too Many Requests",
            &[("Retry-After", "5")],
            json!({
                "error": {
                    "code": "API_KEY_RATE_LIMIT_EXCEEDED",
                    "message": CANARY
                }
            }),
        )
        .await;
        assert!(
            tokio::time::timeout(Duration::from_millis(250), listener.accept())
                .await
                .is_err()
        );
    });
    let mut request = request_with_messages(vec![message(LlmMessageRole::User, "Hello")]);
    request.api_url = format!("http://{address}/v1/chat/completions");
    request.api_token = "cancelled-rate-limit-token".to_string();
    let cancellation = AgentCancellationToken::new();
    let event_cancellation = cancellation.clone();
    let mut events = Vec::new();
    let started = std::time::Instant::now();

    let error = complete_chat_streaming(request, cancellation, |event| {
        if matches!(event, LlmStreamEvent::Retrying { .. }) {
            event_cancellation.cancel();
        }
        events.push(event);
    })
    .await
    .unwrap_err();
    server.await.unwrap();

    assert!(error.is_cancelled());
    assert!(started.elapsed() < Duration::from_millis(500));
    assert!(events.iter().all(|event| !matches!(
        event,
        LlmStreamEvent::AttemptReset { reason } if reason.contains(CANARY)
    )));
}

#[tokio::test]
async fn hard_quota_429_is_not_retried() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_test_http_request(&mut stream).await;
        write_test_http_response(
            &mut stream,
            "429 Too Many Requests",
            json!({
                "error": {
                    "code": "insufficient_quota",
                    "message": "billing quota exhausted"
                }
            }),
        )
        .await;
        assert!(
            tokio::time::timeout(Duration::from_millis(100), listener.accept())
                .await
                .is_err()
        );
    });
    let mut request = request_with_messages(vec![message(LlmMessageRole::User, "Hello")]);
    request.api_url = format!("http://{address}/v1/chat/completions");
    request.api_token = "hard-quota-token".to_string();
    request.stream = false;

    let error = complete_chat(request, AgentCancellationToken::new())
        .await
        .unwrap_err();
    server.await.unwrap();
    assert_eq!(error.code(), Some(PROVIDER_FAILURE_ERROR_CODE));
    assert_eq!(
        error
            .details()
            .and_then(|details| details.get("category"))
            .and_then(Value::as_str),
        Some("quota_exhausted")
    );
    assert!(!error.to_string().contains("billing quota exhausted"));
}

#[tokio::test]
async fn long_retry_after_returns_without_sending_a_second_request() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_test_http_request(&mut stream).await;
        write_test_http_response_with_headers(
            &mut stream,
            "429 Too Many Requests",
            &[("Retry-After", "120")],
            json!({"error":{"code":"API_KEY_RATE_LIMIT_EXCEEDED"}}),
        )
        .await;
        assert!(
            tokio::time::timeout(Duration::from_millis(100), listener.accept())
                .await
                .is_err()
        );
    });
    let mut request = request_with_messages(vec![message(LlmMessageRole::User, "Hello")]);
    request.api_url = format!("http://{address}/v1/chat/completions");
    request.api_token = "long-retry-after-token".to_string();
    request.stream = false;

    let error = complete_chat(request, AgentCancellationToken::new())
        .await
        .unwrap_err();
    server.await.unwrap();
    assert_eq!(error.code(), Some(PROVIDER_FAILURE_ERROR_CODE));
    assert_eq!(
        error
            .details()
            .and_then(|details| details.get("retryAfterMs"))
            .and_then(Value::as_u64),
        Some(120_000)
    );
    assert!(error.to_string().contains("暂时限流"));
}

#[tokio::test]
async fn broken_429_body_still_preserves_status_and_retry_after() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_test_http_request(&mut stream).await;
        stream
            .write_all(
                b"HTTP/1.1 429 Too Many Requests\r\nRetry-After: 120\r\nContent-Type: application/json\r\nContent-Length: 100\r\nConnection: close\r\n\r\n{",
            )
            .await
            .unwrap();
        stream.shutdown().await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(100), listener.accept())
                .await
                .is_err()
        );
    });
    let mut request = request_with_messages(vec![message(LlmMessageRole::User, "Hello")]);
    request.api_url = format!("http://{address}/v1/chat/completions");
    request.api_token = "broken-429-body-token".to_string();
    request.stream = false;

    let error = complete_chat(request, AgentCancellationToken::new())
        .await
        .unwrap_err();
    server.await.unwrap();
    assert_eq!(error.code(), Some(PROVIDER_FAILURE_ERROR_CODE));
    assert_eq!(
        error
            .details()
            .and_then(|details| details.get("category"))
            .and_then(Value::as_str),
        Some("rate_limited")
    );
    assert_eq!(
        error
            .details()
            .and_then(|details| details.get("retryAfterMs"))
            .and_then(Value::as_u64),
        Some(120_000)
    );
}

#[tokio::test]
async fn streaming_partial_output_is_reset_before_retrying() {
    const CANARY: &str = "PARTIAL_STREAM_SECRET_CANARY";
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        for attempt in 1..=2 {
            let (mut stream, _) = listener.accept().await.unwrap();
            read_test_http_request(&mut stream).await;
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
                )
                .await
                .unwrap();
            let body = if attempt == 1 {
                format!(
                    "data: {}\n\ndata: {{not-json-{CANARY}\n\n",
                    json!({"choices":[{"delta":{"content":"partial"}}]})
                )
            } else {
                format!(
                    "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
                    json!({"choices":[{"delta":{"content":"recovered"}}]}),
                    json!({"choices":[{"delta":{},"finish_reason":"stop"}]})
                )
            };
            stream.write_all(body.as_bytes()).await.unwrap();
        }
    });
    let mut request = request_with_messages(vec![message(LlmMessageRole::User, "Hello")]);
    request.api_url = format!("http://{address}/v1/chat/completions");
    let mut events = Vec::new();

    let response = complete_chat_streaming(request, AgentCancellationToken::new(), |event| {
        events.push(event);
    })
    .await
    .unwrap();
    server.await.unwrap();

    assert_eq!(response.content(), "recovered");
    assert!(events
        .iter()
        .any(|event| matches!(event, LlmStreamEvent::Delta(delta) if delta == "partial")));
    assert!(events
        .iter()
        .any(|event| matches!(event, LlmStreamEvent::Retrying { attempt: 2, .. })));
    assert!(events
        .iter()
        .any(|event| matches!(event, LlmStreamEvent::Delta(delta) if delta == "recovered")));
    assert!(events.iter().all(|event| !matches!(
        event,
        LlmStreamEvent::AttemptReset { reason } if reason.contains(CANARY)
    )));
}

#[tokio::test]
async fn streaming_invalid_tool_arguments_reset_the_attempt_and_recover() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        for attempt in 1..=2 {
            let (mut stream, _) = listener.accept().await.unwrap();
            read_test_http_request(&mut stream).await;
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
                )
                .await
                .unwrap();
            let body = if attempt == 1 {
                format!(
                    "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
                    json!({
                        "choices": [{
                            "delta": {
                                "tool_calls": [{
                                    "index": 0,
                                    "id": "call-invalid",
                                    "function": {
                                        "name": "run_command",
                                        "arguments": "{\"command\":\"pwd\""
                                    }
                                }]
                            }
                        }]
                    }),
                    json!({"choices":[{"delta":{},"finish_reason":"tool_calls"}]})
                )
            } else {
                format!(
                    "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
                    json!({"choices":[{"delta":{"content":"recovered"}}]}),
                    json!({"choices":[{"delta":{},"finish_reason":"stop"}]})
                )
            };
            stream.write_all(body.as_bytes()).await.unwrap();
        }
    });
    let mut request = request_with_messages(vec![message(LlmMessageRole::User, "Hello")]);
    request.api_url = format!("http://{address}/v1/chat/completions");
    let mut events = Vec::new();

    let response = complete_chat_streaming(request, AgentCancellationToken::new(), |event| {
        events.push(event);
    })
    .await
    .unwrap();
    server.await.unwrap();

    assert_eq!(response.content(), "recovered");
    assert!(events.iter().any(|event| matches!(
        event,
        LlmStreamEvent::AttemptReset { reason }
            if reason == "模型返回的流式工具参数不完整或格式无效。"
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        LlmStreamEvent::Retrying {
            attempt: 2,
            max_attempts: LLM_MAX_ATTEMPTS,
            provider_code: Some(code),
            ..
        } if code == "invalid_stream_tool_arguments"
    )));
}

#[tokio::test]
async fn streaming_activity_refreshes_the_idle_timeout_without_a_total_deadline() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_test_http_request(&mut stream).await;
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
            )
            .await
            .unwrap();
        for content in ["a", "b", "c"] {
            stream
                .write_all(
                    format!(
                        "data: {}\n\n",
                        json!({"choices":[{"delta":{"content":content}}]})
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        stream
            .write_all(
                format!(
                    "data: {}\n\ndata: [DONE]\n\n",
                    json!({"choices":[{"delta":{},"finish_reason":"stop"}]})
                )
                .as_bytes(),
            )
            .await
            .unwrap();
    });
    let mut request = request_with_messages(vec![message(LlmMessageRole::User, "Hello")]);
    request.api_url = format!("http://{address}/v1/chat/completions");
    let mut retries = 0_usize;

    let response = complete_chat_streaming_with_validation_and_timeout(
        request,
        AgentCancellationToken::new(),
        LlmResponseValidation::RequireModelAction,
        Duration::from_millis(40),
        |event| {
            if matches!(event, LlmStreamEvent::Retrying { .. }) {
                retries += 1;
            }
        },
    )
    .await
    .unwrap();
    server.await.unwrap();

    assert_eq!(response.content(), "abc");
    assert_eq!(retries, 0);
}

#[tokio::test]
async fn streaming_response_header_timeout_retries_and_recovers() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        for attempt in 1..=2 {
            let (mut stream, _) = listener.accept().await.unwrap();
            read_test_http_request(&mut stream).await;
            if attempt == 1 {
                tokio::time::sleep(Duration::from_millis(100)).await;
            } else {
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
                            json!({"choices":[{"delta":{"content":"connected"}}]}),
                            json!({"choices":[{"delta":{},"finish_reason":"stop"}]})
                        )
                        .as_bytes(),
                    )
                    .await
                    .unwrap();
            }
        }
    });
    let mut request = request_with_messages(vec![message(LlmMessageRole::User, "Hello")]);
    request.api_url = format!("http://{address}/v1/chat/completions");
    let mut events = Vec::new();

    let response = complete_chat_streaming_with_validation_and_timeout(
        request,
        AgentCancellationToken::new(),
        LlmResponseValidation::RequireModelAction,
        Duration::from_millis(35),
        |event| events.push(event),
    )
    .await
    .unwrap();
    server.await.unwrap();

    assert_eq!(response.content(), "connected");
    assert!(events.iter().any(|event| matches!(
        event,
        LlmStreamEvent::Retrying {
            attempt: 2,
            provider_code: Some(code),
            ..
        } if code == "stream_idle_timeout"
    )));
}

#[tokio::test]
async fn streaming_idle_timeout_rolls_back_partial_tool_input_and_recovers() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        for attempt in 1..=2 {
            let (mut stream, _) = listener.accept().await.unwrap();
            read_test_http_request(&mut stream).await;
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
                )
                .await
                .unwrap();
            if attempt == 1 {
                stream
                    .write_all(
                        format!(
                            "data: {}\n\n",
                            json!({
                                "choices":[{
                                    "delta":{
                                        "tool_calls":[{
                                            "index":0,
                                            "id":"stale-call",
                                            "function":{
                                                "name":"read_file",
                                                "arguments":"{\"path\":\"stale"
                                            }
                                        }]
                                    }
                                }]
                            })
                        )
                        .as_bytes(),
                    )
                    .await
                    .unwrap();
                tokio::time::sleep(Duration::from_millis(100)).await;
            } else {
                stream
                    .write_all(
                        format!(
                            "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
                            json!({"choices":[{"delta":{"content":"recovered"}}]}),
                            json!({"choices":[{"delta":{},"finish_reason":"stop"}]})
                        )
                        .as_bytes(),
                    )
                    .await
                    .unwrap();
            }
        }
    });
    let mut request = request_with_messages(vec![message(LlmMessageRole::User, "Hello")]);
    request.api_url = format!("http://{address}/v1/chat/completions");
    let mut events = Vec::new();

    let response = complete_chat_streaming_with_validation_and_timeout(
        request,
        AgentCancellationToken::new(),
        LlmResponseValidation::RequireModelAction,
        Duration::from_millis(35),
        |event| events.push(event),
    )
    .await
    .unwrap();
    server.await.unwrap();

    assert_eq!(response.content(), "recovered");
    assert!(events
        .iter()
        .any(|event| matches!(event, LlmStreamEvent::ToolInputProgress { .. })));
    assert!(events
        .iter()
        .any(|event| matches!(event, LlmStreamEvent::AttemptReset { .. })));
    assert!(events.iter().any(|event| matches!(
        event,
        LlmStreamEvent::Retrying {
            attempt: 2,
            provider_code: Some(code),
            ..
        } if code == "stream_idle_timeout"
    )));
}

#[tokio::test]
async fn streaming_keepalives_do_not_hide_an_idle_model() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        for attempt in 1..=2 {
            let (mut stream, _) = listener.accept().await.unwrap();
            read_test_http_request(&mut stream).await;
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
                )
                .await
                .unwrap();
            if attempt == 1 {
                for _ in 0..8 {
                    if stream.write_all(b": keepalive\n\n").await.is_err() {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            } else {
                stream
                    .write_all(
                        format!(
                            "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
                            json!({"choices":[{"delta":{"content":"reconnected"}}]}),
                            json!({"choices":[{"delta":{},"finish_reason":"stop"}]})
                        )
                        .as_bytes(),
                    )
                    .await
                    .unwrap();
            }
        }
    });
    let mut request = request_with_messages(vec![message(LlmMessageRole::User, "Hello")]);
    request.api_url = format!("http://{address}/v1/chat/completions");

    let response = complete_chat_streaming_with_validation_and_timeout(
        request,
        AgentCancellationToken::new(),
        LlmResponseValidation::RequireModelAction,
        Duration::from_millis(35),
        |_| {},
    )
    .await
    .unwrap();
    server.await.unwrap();

    assert_eq!(response.content(), "reconnected");
}

#[tokio::test]
async fn streaming_metadata_frames_do_not_refresh_the_idle_timeout() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_test_http_request(&mut stream).await;
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
            )
            .await
            .unwrap();
        let frames = [
            "event: ping\ndata: {\"type\":\"ping\"}\n\n".to_string(),
            "data: {}\n\n".to_string(),
            format!(
                "data: {}\n\n",
                json!({"choices":[],"usage":{"prompt_tokens":1}})
            ),
            format!(
                "data: {}\n\n",
                json!({"choices":[{"delta":{},"finish_reason":"stop"}]})
            ),
        ];
        for index in 0..20 {
            if stream
                .write_all(frames[index % frames.len()].as_bytes())
                .await
                .is_err()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    });
    let mut request = request_with_messages(vec![message(LlmMessageRole::User, "Hello")]);
    request.api_url = format!("http://{address}/v1/chat/completions");

    let result = tokio::time::timeout(
        Duration::from_millis(120),
        complete_chat_streaming_once(
            &request,
            AgentCancellationToken::new(),
            LlmResponseValidation::RequireModelAction,
            Duration::from_millis(35),
            |_| {},
        ),
    )
    .await
    .expect("metadata-only frames must not keep the stream alive");
    let error = result.unwrap_err();
    server.await.unwrap();

    assert_eq!(
        error
            .details()
            .and_then(|details| details.get("providerCode"))
            .and_then(Value::as_str),
        Some("stream_idle_timeout")
    );
}

#[tokio::test]
async fn streaming_idle_window_spans_response_headers_and_body() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_test_http_request(&mut stream).await;
        tokio::time::sleep(Duration::from_millis(60)).await;
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
            )
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
    });
    let mut request = request_with_messages(vec![message(LlmMessageRole::User, "Hello")]);
    request.api_url = format!("http://{address}/v1/chat/completions");

    let result = tokio::time::timeout(
        Duration::from_millis(115),
        complete_chat_streaming_once(
            &request,
            AgentCancellationToken::new(),
            LlmResponseValidation::RequireModelAction,
            Duration::from_millis(80),
            |_| {},
        ),
    )
    .await
    .expect("headers must not restart the model inactivity window");
    let error = result.unwrap_err();
    server.await.unwrap();

    assert_eq!(
        error
            .details()
            .and_then(|details| details.get("providerCode"))
            .and_then(Value::as_str),
        Some("stream_idle_timeout")
    );
}

#[test]
fn rejects_incompatible_tool_schema_before_building_an_http_request() {
    let mut tool = tool_definition();
    tool.input_schema["anyOf"] = json!([{ "required": ["path"] }]);
    let request = LlmChatRequest {
        api_url: "https://example.test/v1/chat/completions".to_string(),
        api_token: "token".to_string(),
        provider_profile: generic_provider_profile(AgentApiStyle::OpenAiCompatible),
        provider_protocol: generic_provider_protocol(AgentApiStyle::OpenAiCompatible, "gpt"),
        max_tokens: 1024,
        temperature: 0.2,
        stream: true,
        messages: vec![message(LlmMessageRole::User, "Read a file")],
        tools: vec![tool],
    };

    let error = validate_request(&request).unwrap_err();

    assert!(error.to_string().contains("read_file"));
    assert!(error.to_string().contains("anyOf"));
}

#[test]
fn accepts_a_complete_tool_exchange_with_an_application_canonical_id() {
    let call_id = model_response_tool_call_id("active-run", 0, 0, "provider-call");
    let request = request_with_messages(vec![
        message(LlmMessageRole::User, "Read a file"),
        LlmMessage::assistant(
            "",
            vec![LlmToolCall {
                id: call_id.clone(),
                name: "read_file".to_string(),
                args: json!({ "path": "src/lib.rs" }),
            }],
        ),
        LlmMessage::tool_result(call_id, "{\"ok\":true}", false),
    ]);

    validate_request(&request).unwrap();
}

#[test]
fn rejects_noncanonical_tool_ids_at_or_below_the_provider_limit_before_network_io() {
    for call_id in ["call-1".to_string(), "a".repeat(64)] {
        let request = request_with_messages(vec![
            LlmMessage::assistant(
                "",
                vec![LlmToolCall {
                    id: call_id.clone(),
                    name: "read_file".to_string(),
                    args: json!({ "path": "src/lib.rs" }),
                }],
            ),
            LlmMessage::tool_result(call_id, "{\"ok\":true}", false),
        ]);

        let error = validate_request(&request).unwrap_err();

        assert_eq!(error.code(), Some("agent.invalid_model_tool_call_id"));
        assert_eq!(
            error.details().and_then(|details| details["code"].as_str()),
            Some("nonCanonicalToolCallId")
        );
        assert_eq!(
            error
                .details()
                .and_then(|details| details["canonicalLength"].as_u64()),
            Some(47)
        );
        assert_eq!(
            error
                .details()
                .and_then(|details| details["canonicalPrefix"].as_str()),
            Some("tc1_")
        );
    }
}

#[test]
fn rejects_tool_ids_over_sixty_four_bytes_before_network_io() {
    let call_id = "a".repeat(65);
    let request = request_with_messages(vec![
        LlmMessage::assistant(
            "",
            vec![LlmToolCall {
                id: call_id.clone(),
                name: "read_file".to_string(),
                args: json!({ "path": "src/lib.rs" }),
            }],
        ),
        LlmMessage::tool_result(call_id, "{\"ok\":true}", false),
    ]);

    let error = validate_request(&request).unwrap_err();

    assert_eq!(error.code(), Some("agent.invalid_model_tool_call_id"));
    assert_eq!(
        error.details().and_then(|details| details["code"].as_str()),
        Some("toolCallIdTooLong")
    );
    assert_eq!(
        error
            .details()
            .and_then(|details| details["maxLength"].as_u64()),
        Some(64)
    );
}

#[test]
fn rejects_unsafe_tool_id_characters_before_network_io() {
    let request = request_with_messages(vec![
        LlmMessage::assistant(
            "",
            vec![LlmToolCall {
                id: "unsafe:id".to_string(),
                name: "read_file".to_string(),
                args: json!({ "path": "src/lib.rs" }),
            }],
        ),
        LlmMessage::tool_result("unsafe:id", "{\"ok\":true}", false),
    ]);

    let error = validate_request(&request).unwrap_err();

    assert_eq!(error.code(), Some("agent.invalid_model_tool_call_id"));
    assert_eq!(
        error.details().and_then(|details| details["code"].as_str()),
        Some("unsafeToolCallIdCharacters")
    );
}

#[test]
fn rejects_duplicate_and_unpaired_tool_protocol_before_network_io() {
    let duplicate_call_id = model_response_tool_call_id("duplicate-run", 0, 0, "provider-call");
    let duplicate = request_with_messages(vec![
        LlmMessage::assistant(
            "",
            vec![
                LlmToolCall {
                    id: duplicate_call_id.clone(),
                    name: "read_file".to_string(),
                    args: json!({ "path": "src/one.rs" }),
                },
                LlmToolCall {
                    id: duplicate_call_id.clone(),
                    name: "read_file".to_string(),
                    args: json!({ "path": "src/two.rs" }),
                },
            ],
        ),
        LlmMessage::tool_result(duplicate_call_id, "{\"ok\":true}", false),
    ]);
    let duplicate_error = validate_request(&duplicate).unwrap_err();
    assert_eq!(
        duplicate_error.code(),
        Some("agent.invalid_model_tool_protocol")
    );
    assert_eq!(
        duplicate_error
            .details()
            .and_then(|details| details["code"].as_str()),
        Some("duplicateToolCallId")
    );

    let orphan_call_id = model_response_tool_call_id("orphan-run", 0, 0, "provider-call");
    let unpaired = request_with_messages(vec![LlmMessage::tool_result(
        orphan_call_id,
        "result",
        true,
    )]);
    let unpaired_error = validate_request(&unpaired).unwrap_err();
    assert_eq!(
        unpaired_error.code(),
        Some("agent.invalid_model_tool_protocol")
    );
    assert_eq!(
        unpaired_error
            .details()
            .and_then(|details| details["code"].as_str()),
        Some("unpairedToolResult")
    );
}

#[tokio::test]
async fn streaming_stop_without_text_or_tools_is_a_repairable_semantic_error() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_test_http_request(&mut stream).await;
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
            )
            .await
            .unwrap();
        let terminal = json!({
            "choices": [{
                "delta": {},
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 9,
                "completion_tokens": 2,
                "total_tokens": 11
            }
        });
        stream
            .write_all(format!("data: {terminal}\n\ndata: [DONE]\n\n").as_bytes())
            .await
            .unwrap();
    });
    let request = LlmChatRequest {
        api_url: format!("http://{address}/v1/chat/completions"),
        api_token: "token".to_string(),
        provider_profile: generic_provider_profile(AgentApiStyle::OpenAiCompatible),
        provider_protocol: generic_provider_protocol(AgentApiStyle::OpenAiCompatible, "test-model"),
        max_tokens: 1_024,
        temperature: 0.2,
        stream: true,
        messages: vec![message(LlmMessageRole::User, "Hello")],
        tools: Vec::new(),
    };
    let mut events = Vec::new();
    let error = complete_chat_streaming(request, AgentCancellationToken::new(), |event| {
        events.push(event);
    })
    .await
    .unwrap_err();
    server.await.unwrap();

    assert_eq!(error.code(), Some(EMPTY_MODEL_ACTION_ERROR_CODE));
    assert!(is_repairable_empty_model_action(&error));
    assert_eq!(
        error.usage().and_then(|usage| usage.billable_request_count),
        Some(1)
    );
    assert!(events
        .iter()
        .any(|event| matches!(event, LlmStreamEvent::AttemptReset { .. })));
    assert!(!events
        .iter()
        .any(|event| matches!(event, LlmStreamEvent::Retrying { .. })));
}

#[test]
fn retry_exhausted_error_mentions_retry_count() {
    let error = retry_exhausted_error(
        LlmProviderFailure::from_local_transport_failure("timeout secret diagnostic")
            .to_agent_error()
            .with_usage(Some(AgentUsage {
                input_tokens: Some(7),
                output_tokens: None,
                output_thinking_tokens: None,
                total_tokens: None,
                cached_input_tokens: None,
                cache_creation_input_tokens: None,
                billable_request_count: Some(3),
            })),
        3,
    );

    assert!(error.to_string().contains("已重试 2 次"));
    assert!(!error.to_string().contains("secret diagnostic"));
    assert_eq!(error.code(), Some(PROVIDER_FAILURE_ERROR_CODE));
    assert_eq!(error.usage().unwrap().input_tokens, Some(7));
    assert_eq!(error.usage().unwrap().billable_request_count, Some(3));
}

#[test]
fn retry_plan_refuses_retry_after_beyond_the_total_sleep_budget() {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::RETRY_AFTER,
        reqwest::header::HeaderValue::from_static("120"),
    );
    let error = LlmProviderFailure::from_http_response(
        AgentApiStyle::OpenAiCompatible,
        reqwest::StatusCode::TOO_MANY_REQUESTS,
        &headers,
        r#"{"error":{"code":"API_KEY_RATE_LIMIT_EXCEEDED"}}"#,
        "",
    )
    .to_agent_error();

    assert!(retry_plan(&error, 1, Duration::ZERO, LLM_LOGICAL_REQUEST_TIMEOUT,).is_none());
}

#[test]
fn rate_limit_retry_plan_has_longer_bounded_delay_and_canonical_metadata() {
    let error = LlmProviderFailure::from_http_response(
        AgentApiStyle::OpenAiCompatible,
        reqwest::StatusCode::TOO_MANY_REQUESTS,
        &reqwest::header::HeaderMap::new(),
        r#"{"error":{"code":"API_KEY_RATE_LIMIT_EXCEEDED"}}"#,
        "",
    )
    .to_agent_error();
    let plan = retry_plan(&error, 1, Duration::ZERO, LLM_LOGICAL_REQUEST_TIMEOUT).unwrap();

    assert_eq!(plan.category, LlmProviderFailureCategory::RateLimited);
    assert_eq!(
        plan.provider_code.as_deref(),
        Some("api_key_rate_limit_exceeded")
    );
    assert_eq!(plan.max_attempts, LLM_MAX_ATTEMPTS);
    assert!(plan.delay >= Duration::from_millis(LLM_RATE_LIMIT_RETRY_BASE_DELAY_MS));
    assert!(plan.delay <= Duration::from_millis(2_500));
    assert!(plan.delay > retry_delay(1));
}

#[test]
fn every_retryable_failure_uses_the_same_attempt_budget_and_exhaustion_boundary() {
    let rate_limited = LlmProviderFailure::from_http_response(
        AgentApiStyle::OpenAiCompatible,
        reqwest::StatusCode::TOO_MANY_REQUESTS,
        &reqwest::header::HeaderMap::new(),
        r#"{"error":{"code":"API_KEY_RATE_LIMIT_EXCEEDED"}}"#,
        "",
    )
    .to_agent_error();
    let network =
        LlmProviderFailure::from_local_transport_failure("connection reset").to_agent_error();
    let invalid_stream_arguments = AgentError::structured(
        INVALID_STREAM_TOOL_ARGUMENTS_ERROR_CODE,
        "模型返回的流式工具参数不完整或格式无效。",
        json!({
            "type": "invalid_stream_tool_arguments",
            "retryable": true,
        }),
    );

    for error in [&rate_limited, &network, &invalid_stream_arguments] {
        let plan = retry_plan(error, 1, Duration::ZERO, LLM_LOGICAL_REQUEST_TIMEOUT).unwrap();
        assert_eq!(plan.max_attempts, LLM_MAX_ATTEMPTS);
        assert!(retry_plan(
            error,
            LLM_MAX_ATTEMPTS,
            Duration::ZERO,
            LLM_LOGICAL_REQUEST_TIMEOUT,
        )
        .is_none());
    }
}

#[test]
fn internal_callers_can_defer_empty_response_validation() {
    let body = json!({
        "choices": [{
            "message": { "role": "assistant", "content": "" },
            "finish_reason": "length"
        }]
    })
    .to_string();
    let protocol = generic_provider_protocol(AgentApiStyle::OpenAiCompatible, "test-model");

    let strict =
        parse_non_stream_response(&body, &protocol, LlmResponseValidation::RequireModelAction);
    let deferred =
        parse_non_stream_response(&body, &protocol, LlmResponseValidation::AllowEmpty).unwrap();

    let strict = strict.unwrap_err();
    assert!(strict.to_string().contains("没有可显示文本"));
    assert_eq!(strict.code(), Some(EMPTY_MODEL_ACTION_ERROR_CODE));
    assert!(!is_repairable_empty_model_action(&strict));
    assert!(deferred.content().is_empty());
    assert_eq!(deferred.finish_reason.as_deref(), Some("length"));

    let normal_stop = json!({
        "choices": [{
            "message": { "role": "assistant", "content": "" },
            "finish_reason": "stop"
        }],
        // Response metadata must not trick the transport retry heuristic into replaying the
        // original empty request before the agent loop sends its one semantic repair request.
        "gatewayDiagnostic": "EMPTY_ACTION_SECRET_CANARY upstream timeout"
    })
    .to_string();
    let repairable = parse_non_stream_response(
        &normal_stop,
        &protocol,
        LlmResponseValidation::RequireModelAction,
    )
    .unwrap_err();
    assert!(is_repairable_empty_model_action(&repairable));
    assert!(!is_retryable_llm_error(&repairable));
    assert!(!repairable
        .to_string()
        .contains("EMPTY_ACTION_SECRET_CANARY"));
    assert!(!format!("{:?}", repairable.details()).contains("EMPTY_ACTION_SECRET_CANARY"));
}
