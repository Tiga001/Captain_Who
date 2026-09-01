use super::*;

#[tokio::test]
async fn runtime_rejects_unknown_frozen_provider_registration_before_transport_or_tools() {
    use crate::storage::service::StorageService;
    use crate::{ProviderProfileConfig, ProviderProtocolDialect, ProviderProtocolKey};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tempfile::tempdir;
    use tokio::net::TcpListener;
    use tokio::time::{timeout, Duration};

    const MODEL_ID: &str = "unknown-provider-registration-model";

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let model_requests = Arc::new(AtomicUsize::new(0));
    let model_requests_for_server = Arc::clone(&model_requests);
    let server = tokio::spawn(async move {
        if let Ok(Ok((mut stream, _))) =
            timeout(Duration::from_millis(300), listener.accept()).await
        {
            model_requests_for_server.fetch_add(1, Ordering::SeqCst);
            write_runtime_test_json_response(
                &mut stream,
                json!({
                    "choices": [{
                        "message": { "role": "assistant", "content": "must not be reached" },
                        "finish_reason": "stop"
                    }]
                }),
            )
            .await;
        }
    });

    let mut profile = ProviderProfileConfig::deepseek_v4_default();
    let mut protocol = ProviderProtocolKey::new(
        ProviderProtocolDialect::OpenAiChatCompletions,
        &profile,
        MODEL_ID,
        None,
    )
    .unwrap();
    match &mut profile {
        ProviderProfileConfig::V1(config) => config.profile.version = 99,
        ProviderProfileConfig::V2(config) => config.profile.version = 99,
    }
    protocol.profile.version = 99;

    let tool_executions = Arc::new(AtomicUsize::new(0));
    let tool_executions_for_executor = Arc::clone(&tool_executions);
    let forbidden_executor: AgentHostActionExecutor = Arc::new(move |_, _, _| {
        tool_executions_for_executor.fetch_add(1, Ordering::SeqCst);
        Err(AgentError::new(
            "tool executor must not run for an unknown Provider registration",
        ))
    });
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("runtime.sqlite")).unwrap());

    let mut input = conversation_context_input(vec![message("user", "Do not execute this run.")]);
    input.api_url = format!("http://{address}/v1/chat/completions");
    input.api_token = "test-token".to_string();
    input.model = MODEL_ID.to_string();
    input.stream = Some(false);
    input.provider_profile_config = Some(profile);
    input.provider_protocol_key = Some(protocol);

    let error = AgentRuntime::default()
        .send_chat_with_events_and_cancellation(
            input,
            Some("run-unknown-provider-registration".to_string()),
            None,
            AgentCancellationToken::new(),
            Some(AgentRuntimeHostServices::new().with_host_actions(forbidden_executor, storage)),
        )
        .await
        .unwrap_err();
    server.await.unwrap();

    assert_eq!(
        error.to_string(),
        "Provider profile configuration is invalid: unsupported provider profile deepseek_v4_chat version 99"
    );
    assert_eq!(model_requests.load(Ordering::SeqCst), 0);
    assert_eq!(tool_executions.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn concurrent_steer_during_sampling_is_fifo_and_turns_a_terminal_response_into_narration() {
    use tokio::net::TcpListener;
    use tokio::sync::oneshot;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let requests_for_server = Arc::clone(&requests);
    let (first_request_seen_tx, first_request_seen_rx) = oneshot::channel();
    let (release_first_response_tx, release_first_response_rx) = oneshot::channel();
    let server = tokio::spawn(async move {
        let mut first_request_seen_tx = Some(first_request_seen_tx);
        let mut release_first_response_rx = Some(release_first_response_rx);
        for request_index in 0..2 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = read_runtime_test_json_request(&mut stream).await;
            requests_for_server.lock().unwrap().push(request);
            if request_index == 0 {
                first_request_seen_tx.take().unwrap().send(()).unwrap();
                release_first_response_rx.take().unwrap().await.unwrap();
            }
            let content = if request_index == 0 {
                "Initial answer before guidance."
            } else {
                "Final answer after guidance."
            };
            write_runtime_test_json_response(
                &mut stream,
                json!({
                    "choices": [{
                        "message": { "role": "assistant", "content": content },
                        "finish_reason": "stop"
                    }]
                }),
            )
            .await;
        }
    });

    let queue = AgentSteerInputQueue::new();
    assert_eq!(
        queue
            .enqueue(runtime_steer_input(
                "guidance-before-stream",
                "client-before-stream",
                "Include the pre-stream constraint."
            ))
            .unwrap(),
        AgentSteerEnqueueOutcome::Queued
    );
    let mut input = conversation_context_input(vec![message("user", "Start the task.")]);
    input.api_url = format!("http://{address}/v1/chat/completions");
    input.api_token = "test-token".to_string();
    input.stream = Some(false);
    input.assistant_message_id = Some("assistant-steer".to_string());
    input.context = Some(AgentRunContext {
        collaboration_identity: None,
        conversation_id: Some("conversation-steer".to_string()),
        project_id: None,
        workspace: None,
        attachment_library: None,
        permissions: Default::default(),
    });
    let runtime_queue = queue.clone();
    let runtime = tokio::spawn(async move {
        AgentRuntime::default()
            .send_chat_with_events_and_cancellation(
                input,
                Some("run-steer".to_string()),
                None,
                AgentCancellationToken::new(),
                Some(AgentRuntimeHostServices::new().with_steer_input(runtime_queue)),
            )
            .await
            .unwrap()
    });

    first_request_seen_rx.await.unwrap();
    assert_eq!(
        queue
            .enqueue(runtime_steer_input(
                "guidance-1",
                "client-1",
                "Also include the migration risk."
            ))
            .unwrap(),
        AgentSteerEnqueueOutcome::Queued
    );
    assert_eq!(
        queue
            .enqueue(runtime_steer_input(
                "guidance-2",
                "client-2",
                "Keep the rollout steps in chronological order."
            ))
            .unwrap(),
        AgentSteerEnqueueOutcome::Queued
    );
    release_first_response_tx.send(()).unwrap();
    let output = runtime.await.unwrap();
    server.await.unwrap();

    assert_eq!(output.content, "Final answer after guidance.");
    let requests = requests.lock().unwrap();
    let second_messages = requests[1]["messages"].as_array().unwrap();
    let intermediate_index = second_messages
        .iter()
        .position(|message| {
            message["role"] == "assistant"
                && message["content"] == "Initial answer before guidance."
        })
        .unwrap();
    let before_stream_guidance_index = second_messages
        .iter()
        .position(|message| {
            message["role"] == "user" && message["content"] == "Include the pre-stream constraint."
        })
        .unwrap();
    let first_guidance_index = second_messages
        .iter()
        .position(|message| {
            message["role"] == "user" && message["content"] == "Also include the migration risk."
        })
        .unwrap();
    let second_guidance_index = second_messages
        .iter()
        .position(|message| {
            message["role"] == "user"
                && message["content"] == "Keep the rollout steps in chronological order."
        })
        .unwrap();
    assert!(intermediate_index < before_stream_guidance_index);
    assert!(before_stream_guidance_index < first_guidance_index);
    assert!(first_guidance_index < second_guidance_index);
    drop(requests);

    let trace = output.conversation_turn_trace.as_ref().unwrap();
    assert!(matches!(
        &trace.items[..],
        [
            ConversationTurnTraceItem::AssistantNarration { content, .. },
            ConversationTurnTraceItem::UserGuidance {
                guidance_id,
                client_message_id,
                ..
            },
            ConversationTurnTraceItem::UserGuidance {
                guidance_id: second_guidance_id,
                client_message_id: second_client_message_id,
                ..
            },
            ConversationTurnTraceItem::UserGuidance {
                guidance_id: third_guidance_id,
                client_message_id: third_client_message_id,
                ..
            }
        ] if content == "Initial answer before guidance."
            && guidance_id == "guidance-before-stream"
            && client_message_id == "client-before-stream"
            && second_guidance_id == "guidance-1"
            && second_client_message_id == "client-1"
            && third_guidance_id == "guidance-2"
            && third_client_message_id == "client-2"
    ));
    assert!(output.events.iter().any(|event| matches!(
        event,
        AgentEvent::GuidanceApplied {
            guidance_id,
            sequence: 1,
            ..
        } if guidance_id == "guidance-before-stream"
    )));
    assert!(output.events.iter().any(|event| matches!(
        event,
        AgentEvent::GuidanceApplied {
            guidance_id,
            sequence: 2,
            ..
        } if guidance_id == "guidance-1"
    )));
    assert!(output.events.iter().any(|event| matches!(
        event,
        AgentEvent::GuidanceApplied {
            guidance_id,
            sequence: 3,
            ..
        } if guidance_id == "guidance-2"
    )));
    assert!(!queue.is_accepting());
    assert_eq!(
        queue
            .enqueue(runtime_steer_input(
                "guidance-late",
                "client-late",
                "too late"
            ))
            .unwrap(),
        AgentSteerEnqueueOutcome::Closed
    );
}

#[tokio::test]
async fn steer_accepted_during_transport_retry_is_applied_after_the_retried_response() {
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpListener;
    use tokio::sync::oneshot;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let requests_for_server = Arc::clone(&requests);
    let (retry_started_tx, retry_started_rx) = oneshot::channel();
    let server = tokio::spawn(async move {
        let mut retry_started_tx = Some(retry_started_tx);
        for connection_index in 0..3 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = read_runtime_test_json_request(&mut stream).await;
            requests_for_server.lock().unwrap().push(request);
            if connection_index == 0 {
                let body = b"temporary upstream failure";
                stream
                    .write_all(
                        format!(
                            "HTTP/1.1 503 Service Unavailable\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        )
                        .as_bytes(),
                    )
                    .await
                    .unwrap();
                stream.write_all(body).await.unwrap();
                retry_started_tx.take().unwrap().send(()).unwrap();
                continue;
            }
            let content = if connection_index == 1 {
                "Response after retry."
            } else {
                "Final response after retry guidance."
            };
            write_runtime_test_json_response(
                &mut stream,
                json!({
                    "choices": [{
                        "message": { "role": "assistant", "content": content },
                        "finish_reason": "stop"
                    }]
                }),
            )
            .await;
        }
    });

    let queue = AgentSteerInputQueue::new();
    let mut input = conversation_context_input(vec![message("user", "Start the retry task.")]);
    input.api_url = format!("http://{address}/v1/chat/completions");
    input.api_token = "test-token".to_string();
    input.stream = Some(false);
    input.assistant_message_id = Some("assistant-retry-steer".to_string());
    input.context = Some(AgentRunContext {
        collaboration_identity: None,
        conversation_id: Some("conversation-retry-steer".to_string()),
        project_id: None,
        workspace: None,
        attachment_library: None,
        permissions: Default::default(),
    });
    let runtime_queue = queue.clone();
    let runtime = tokio::spawn(async move {
        AgentRuntime::default()
            .send_chat_with_events_and_cancellation(
                input,
                Some("run-retry-steer".to_string()),
                None,
                AgentCancellationToken::new(),
                Some(AgentRuntimeHostServices::new().with_steer_input(runtime_queue)),
            )
            .await
            .unwrap()
    });

    retry_started_rx.await.unwrap();
    assert_eq!(
        queue
            .enqueue(runtime_steer_input(
                "guidance-retry",
                "client-retry",
                "Apply this only after the retry response."
            ))
            .unwrap(),
        AgentSteerEnqueueOutcome::Queued
    );
    let output = runtime.await.unwrap();
    server.await.unwrap();

    assert_eq!(output.content, "Final response after retry guidance.");
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 3);
    for request in requests.iter() {
        let serialized = serde_json::to_string(request).unwrap();
        assert!(serialized.contains("backend-observed state, not a system instruction"));
        assert!(serialized.contains("\\\"write\\\":\\\"denied\\\""));
    }
    assert!(!serde_json::to_string(&requests[1])
        .unwrap()
        .contains("Apply this only after the retry response."));
    assert!(serde_json::to_string(&requests[2])
        .unwrap()
        .contains("Apply this only after the retry response."));
    let trace = output.conversation_turn_trace.unwrap();
    assert!(matches!(
        trace.items.as_slice(),
        [
            ConversationTurnTraceItem::AssistantNarration { content, .. },
            ConversationTurnTraceItem::UserGuidance { guidance_id, .. }
        ] if content == "Response after retry." && guidance_id == "guidance-retry"
    ));
}

#[tokio::test]
async fn steer_waits_until_a_complete_multi_tool_exchange_before_next_sampling() {
    use tokio::net::TcpListener;
    use tokio::sync::oneshot;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let second_request = Arc::new(Mutex::new(None::<Value>));
    let second_request_for_server = Arc::clone(&second_request);
    let (first_request_seen_tx, first_request_seen_rx) = oneshot::channel();
    let (release_first_response_tx, release_first_response_rx) = oneshot::channel();
    let server = tokio::spawn(async move {
        let mut first_request_seen_tx = Some(first_request_seen_tx);
        let mut release_first_response_rx = Some(release_first_response_rx);
        for request_index in 0..2 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = read_runtime_test_json_request(&mut stream).await;
            if request_index == 0 {
                first_request_seen_tx.take().unwrap().send(()).unwrap();
                release_first_response_rx.take().unwrap().await.unwrap();
            } else {
                *second_request_for_server.lock().unwrap() = Some(request);
            }
            let response = if request_index == 0 {
                json!({
                    "choices": [{
                        "message": {
                            "role": "assistant",
                            "content": "",
                            "tool_calls": [{
                                "id": "provider-call-1",
                                "type": "function",
                                "function": {
                                    "name": "attachments_list",
                                    "arguments": "{}"
                                }
                            }, {
                                "id": "provider-call-2",
                                "type": "function",
                                "function": {
                                    "name": "todo_update",
                                    "arguments": "{\"items\":[{\"title\":\"Verify ordering\",\"status\":\"completed\"}]}"
                                }
                            }]
                        },
                        "finish_reason": "tool_calls"
                    }]
                })
            } else {
                json!({
                    "choices": [{
                        "message": { "role": "assistant", "content": "Done after the tool." },
                        "finish_reason": "stop"
                    }]
                })
            };
            write_runtime_test_json_response(&mut stream, response).await;
        }
    });

    let queue = AgentSteerInputQueue::new();
    let mut input = conversation_context_input(vec![message("user", "List attachments.")]);
    input.api_url = format!("http://{address}/v1/chat/completions");
    input.api_token = "test-token".to_string();
    input.stream = Some(false);
    input.assistant_message_id = Some("assistant-tool-steer".to_string());
    input.context = Some(AgentRunContext {
        collaboration_identity: None,
        conversation_id: Some("conversation-tool-steer".to_string()),
        project_id: None,
        workspace: None,
        attachment_library: None,
        permissions: Default::default(),
    });
    let runtime_queue = queue.clone();
    let runtime = tokio::spawn(async move {
        AgentRuntime::default()
            .send_chat_with_events_and_cancellation(
                input,
                Some("run-tool-steer".to_string()),
                None,
                AgentCancellationToken::new(),
                Some(AgentRuntimeHostServices::new().with_steer_input(runtime_queue)),
            )
            .await
            .unwrap()
    });

    first_request_seen_rx.await.unwrap();
    queue
        .enqueue(runtime_steer_input(
            "guidance-tool",
            "client-tool",
            "After the tool, summarize the count.",
        ))
        .unwrap();
    release_first_response_tx.send(()).unwrap();
    let output = runtime.await.unwrap();
    server.await.unwrap();

    let second_request = second_request.lock().unwrap().take().unwrap();
    let messages = second_request["messages"].as_array().unwrap();
    let tool_call_index = messages
        .iter()
        .position(|message| message["role"] == "assistant" && message["tool_calls"].is_array())
        .unwrap();
    let tool_result_indices = messages
        .iter()
        .enumerate()
        .filter_map(|(index, message)| (message["role"] == "tool").then_some(index))
        .collect::<Vec<_>>();
    assert_eq!(tool_result_indices.len(), 2);
    let guidance_index = messages
        .iter()
        .position(|message| {
            message["role"] == "user"
                && message["content"] == "After the tool, summarize the count."
        })
        .unwrap();
    assert!(tool_result_indices
        .iter()
        .all(|tool_result_index| tool_call_index < *tool_result_index
            && *tool_result_index < guidance_index));

    let trace = output.conversation_turn_trace.unwrap();
    trace.validate().unwrap();
    assert!(matches!(
        trace.items.last(),
        Some(ConversationTurnTraceItem::UserGuidance {
            guidance_id,
            ..
        }) if guidance_id == "guidance-tool"
    ));
    let tool_call_indices = trace
        .items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            matches!(item, ConversationTurnTraceItem::ToolCall { .. }).then_some(index)
        })
        .collect::<Vec<_>>();
    let tool_result_indices = trace
        .items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            matches!(item, ConversationTurnTraceItem::ToolResult { .. }).then_some(index)
        })
        .collect::<Vec<_>>();
    assert_eq!(tool_call_indices.len(), 2);
    assert_eq!(tool_result_indices.len(), 2);
    let exchange_start = *tool_call_indices.first().unwrap();
    let exchange_end = *tool_result_indices.last().unwrap();
    assert!(trace.items[exchange_start..=exchange_end]
        .iter()
        .all(|item| !matches!(item, ConversationTurnTraceItem::UserGuidance { .. })));
    assert!(exchange_end < trace.items.len() - 1);
}

#[tokio::test]
async fn empty_normal_completion_is_repaired_once_for_openai_and_anthropic() {
    use crate::model_request_observation::ModelRequestObservationStatus;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    async fn read_json_request(stream: &mut TcpStream) -> Value {
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4_096];
        let mut body_start = None;
        let mut expected_length = None;
        loop {
            let read = stream.read(&mut buffer).await.unwrap();
            assert!(read > 0, "connection closed before request completed");
            request.extend_from_slice(&buffer[..read]);
            if body_start.is_none() {
                if let Some(header_end) = request.windows(4).position(|part| part == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&request[..header_end]);
                    let content_length = headers
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().ok())
                                .flatten()
                        })
                        .unwrap_or_default();
                    let start = header_end + 4;
                    body_start = Some(start);
                    expected_length = Some(start + content_length);
                }
            }
            if expected_length.is_some_and(|length| request.len() >= length) {
                break;
            }
        }
        serde_json::from_slice(
            &request[body_start.unwrap()..expected_length.expect("content length")],
        )
        .unwrap()
    }

    async fn write_json_response(stream: &mut TcpStream, body: Value) {
        let body = serde_json::to_vec(&body).unwrap();
        let headers = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(headers.as_bytes()).await.unwrap();
        stream.write_all(&body).await.unwrap();
    }

    async fn run_case(style: crate::protocol::AgentApiStyle) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let captured = Arc::new(Mutex::new(Vec::<Value>::new()));
        let captured_for_server = Arc::clone(&captured);
        let server = tokio::spawn(async move {
            for request_index in 0..2 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let request = read_json_request(&mut stream).await;
                captured_for_server.lock().unwrap().push(request);
                let response = match (style, request_index) {
                    (crate::protocol::AgentApiStyle::OpenAiCompatible, 0) => json!({
                        "choices": [{
                            "message": { "role": "assistant", "content": "" },
                            "finish_reason": "stop"
                        }],
                        "usage": {
                            "prompt_tokens": 11,
                            "completion_tokens": 3,
                            "total_tokens": 14
                        }
                    }),
                    (crate::protocol::AgentApiStyle::OpenAiCompatible, _) => json!({
                        "choices": [{
                            "message": { "role": "assistant", "content": "recovered response" },
                            "finish_reason": "stop"
                        }],
                        "usage": {
                            "prompt_tokens": 12,
                            "completion_tokens": 4,
                            "total_tokens": 16
                        }
                    }),
                    (crate::protocol::AgentApiStyle::AnthropicCompatible, 0) => json!({
                        "content": [],
                        "stop_reason": "end_turn",
                        "usage": { "input_tokens": 11, "output_tokens": 3 }
                    }),
                    (crate::protocol::AgentApiStyle::AnthropicCompatible, _) => json!({
                        "content": [{ "type": "text", "text": "recovered response" }],
                        "stop_reason": "end_turn",
                        "usage": { "input_tokens": 12, "output_tokens": 4 }
                    }),
                };
                write_json_response(&mut stream, response).await;
            }
        });

        let observations = Arc::new(Mutex::new(Vec::new()));
        let observations_for_host = Arc::clone(&observations);
        let observer: AgentModelRequestObserver = Arc::new(move |observation| {
            observations_for_host.lock().unwrap().push(observation);
        });
        let mut input = conversation_context_input(vec![message("user", "complete the task")]);
        input.api_url = format!("http://{address}/v1/messages");
        input.api_token = "test-token".to_string();
        input.api_style = Some(style);
        freeze_runtime_test_generic_provider(&mut input, "empty-normal-completion");
        input.stream = Some(false);
        input.assistant_message_id = Some("assistant-empty-repair".to_string());
        input.context = Some(AgentRunContext {
            collaboration_identity: None,
            conversation_id: Some("conversation-empty-repair".to_string()),
            project_id: None,
            workspace: None,
            attachment_library: None,
            permissions: Default::default(),
        });

        let output = AgentRuntime::default()
            .send_chat_with_events_and_cancellation(
                input,
                Some("run-empty-repair".to_string()),
                None,
                AgentCancellationToken::new(),
                Some(AgentRuntimeHostServices::new().with_model_request_observer(observer)),
            )
            .await
            .unwrap();
        server.await.unwrap();

        assert_eq!(output.content, "recovered response");
        assert_eq!(
            output
                .usage
                .as_ref()
                .and_then(|usage| usage.billable_request_count),
            Some(2)
        );
        let requests = captured.lock().unwrap();
        assert_eq!(requests.len(), 2);
        let first = serde_json::to_string(&requests[0]).unwrap();
        let second = serde_json::to_string(&requests[1]).unwrap();
        assert!(!first.contains("preceding model response ended normally"));
        assert!(second.contains("preceding model response ended normally"));
        match style {
            crate::protocol::AgentApiStyle::OpenAiCompatible => {
                assert!(requests[1]["messages"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|message| {
                        message["role"] == "user"
                            && message["content"]
                                .as_str()
                                .is_some_and(|content| content.contains("do not repeat"))
                    }));
            }
            crate::protocol::AgentApiStyle::AnthropicCompatible => {
                assert!(!requests[1]["system"]
                    .as_str()
                    .is_some_and(|system| system.contains("do not repeat")));
                assert!(requests[1]["messages"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|message| {
                        message["role"] == "user"
                            && message["content"].as_array().is_some_and(|blocks| {
                                blocks.iter().any(|block| {
                                    block["text"]
                                        .as_str()
                                        .is_some_and(|text| text.contains("do not repeat"))
                                })
                            })
                    }));
            }
        }
        drop(requests);

        let observations = observations.lock().unwrap();
        assert_eq!(observations.len(), 2);
        assert_eq!(observations[0].request_index, 1);
        assert_eq!(
            observations[0].status,
            ModelRequestObservationStatus::Failed
        );
        assert_eq!(
            observations[0].error_code.as_deref(),
            Some("agent.empty_model_action")
        );
        assert_eq!(observations[1].request_index, 2);
        assert_eq!(
            observations[1].status,
            ModelRequestObservationStatus::Completed
        );
        let trace = serde_json::to_string(
            output
                .conversation_turn_trace
                .as_ref()
                .expect("completed trace"),
        )
        .unwrap();
        assert!(!trace.contains("preceding model response ended normally"));
    }

    run_case(crate::protocol::AgentApiStyle::OpenAiCompatible).await;
    run_case(crate::protocol::AgentApiStyle::AnthropicCompatible).await;
}

#[tokio::test]
async fn empty_model_action_repair_stops_after_the_second_empty_response() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let request_count = Arc::new(AtomicUsize::new(0));
    let request_count_for_server = Arc::clone(&request_count);
    let server = tokio::spawn(async move {
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buffer = [0_u8; 2_048];
            let expected_length = loop {
                let read = stream.read(&mut buffer).await.unwrap();
                assert!(read > 0);
                request.extend_from_slice(&buffer[..read]);
                if let Some(header_end) = request.windows(4).position(|part| part == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&request[..header_end]);
                    let content_length = headers
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().ok())
                                .flatten()
                        })
                        .unwrap_or_default();
                    let expected = header_end + 4 + content_length;
                    if request.len() >= expected {
                        break expected;
                    }
                }
            };
            assert!(request.len() >= expected_length);
            request_count_for_server.fetch_add(1, Ordering::SeqCst);
            let body = serde_json::to_vec(&json!({
                "choices": [{
                    "message": { "role": "assistant", "content": "" },
                    "finish_reason": "stop"
                }],
                "usage": {
                    "prompt_tokens": 5,
                    "completion_tokens": 1,
                    "total_tokens": 6
                }
            }))
            .unwrap();
            let headers = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            stream.write_all(headers.as_bytes()).await.unwrap();
            stream.write_all(&body).await.unwrap();
        }
    });

    let mut input = conversation_context_input(vec![message("user", "complete the task")]);
    input.api_url = format!("http://{address}/v1/chat/completions");
    input.api_token = "test-token".to_string();
    input.stream = Some(false);
    let error = AgentRuntime::default().send_chat(input).await.unwrap_err();
    server.await.unwrap();

    assert_eq!(error.code(), Some("agent.empty_model_action"));
    assert_eq!(
        error.usage().and_then(|usage| usage.billable_request_count),
        Some(2)
    );
    assert_eq!(request_count.load(Ordering::SeqCst), 2);
}
