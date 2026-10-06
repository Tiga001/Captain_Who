use super::*;
use crate::protocol::AgentApiStyle;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

async fn read_request(stream: &mut TcpStream) {
    let mut request = Vec::new();
    loop {
        let mut chunk = [0_u8; 4096];
        let read = stream.read(&mut chunk).await.unwrap();
        assert!(read > 0);
        request.extend_from_slice(&chunk[..read]);
        if let Some(end) = request.windows(4).position(|part| part == b"\r\n\r\n") {
            let headers = std::str::from_utf8(&request[..end]).unwrap();
            let length = headers
                .lines()
                .find_map(|line| {
                    let (key, value) = line.split_once(':')?;
                    key.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().unwrap())
                })
                .unwrap();
            if request.len() >= end + 4 + length {
                return;
            }
        }
    }
}

fn activities(events: &[AgentEvent]) -> Vec<(usize, AgentModelActivity)> {
    events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::ModelActivityChanged {
                run_id,
                stream_id,
                attempt,
                activity,
            } => {
                assert_eq!(run_id, "run");
                assert_eq!(stream_id, "run-stream-1");
                Some((*attempt, *activity))
            }
            _ => None,
        })
        .collect()
}

fn start(attempt: usize) -> LlmStreamEvent {
    LlmStreamEvent::AttemptStarted {
        attempt,
        max_attempts: 3,
    }
}

fn reasoning() -> LlmStreamEvent {
    LlmStreamEvent::ModelActivityChanged(AgentModelActivity::Reasoning)
}

#[test]
fn model_activity_clears_at_every_sampling_boundary_even_when_text_is_blocked() {
    for boundary in [
        LlmStreamEvent::Delta("suppressed narration".to_string()),
        LlmStreamEvent::ToolInputProgress {
            tool_call_index: 0,
            tool: "unknown-tool".to_string(),
            input_delta: "{".to_string(),
            received_bytes: 1,
        },
        LlmStreamEvent::AttemptReset {
            reason: "retry".to_string(),
        },
        LlmStreamEvent::Retrying {
            attempt: 2,
            max_attempts: 3,
            category: "transport".to_string(),
            provider_code: None,
            delay_ms: 10,
            retry_at: 20,
        },
        LlmStreamEvent::Committed,
        LlmStreamEvent::ModelActivityChanged(AgentModelActivity::Waiting),
    ] {
        let captured = Arc::new(Mutex::new(Vec::new()));
        let sink = captured.clone();
        let mut events = AgentEventStream::new(Some(Arc::new(move |event| {
            sink.lock().unwrap().push(event);
        })));
        let registry = ToolRegistry::defaults_with_search(None);
        let tools = ToolExecutionContext::from_run_context(None);
        let cancellation = AgentCancellationToken::new();
        let mut progress = ModelStreamProgress::default();
        let mut context = ModelSamplingContext {
            run_id: "run",
            model_request_index: 0,
            user_text_blocked: true,
            event_stream: &mut events,
            tool_registry: &registry,
            tool_context: &tools,
        };
        for event in [start(1), reasoning(), reasoning(), boundary] {
            progress.on_event(&mut context, "run-stream-1", &cancellation, event);
        }
        // Teardown and repeated phase receipts must not generate duplicate IPC events.
        progress.set_activity(&mut context, "run-stream-1", AgentModelActivity::Waiting);
        assert_eq!(
            activities(&captured.lock().unwrap()),
            [
                (1, AgentModelActivity::Waiting),
                (1, AgentModelActivity::Reasoning),
                (1, AgentModelActivity::Waiting),
            ]
        );
        assert!(!captured.lock().unwrap().iter().any(|event| matches!(
            event,
            AgentEvent::MessageDelta { .. } | AgentEvent::MessageStreamStarted { .. }
        )));
        assert!(!events
            .into_events()
            .iter()
            .any(|event| matches!(event, AgentEvent::ModelActivityChanged { .. })));
    }
}

#[test]
fn model_activity_starts_waiting_for_each_attempt_and_cleanup_survives_cancellation() {
    let captured = Arc::new(Mutex::new(Vec::new()));
    let sink = captured.clone();
    let mut events = AgentEventStream::new(Some(Arc::new(move |event| {
        sink.lock().unwrap().push(event);
    })));
    let registry = ToolRegistry::defaults_with_search(None);
    let tools = ToolExecutionContext::from_run_context(None);
    let cancellation = AgentCancellationToken::new();
    let mut progress = ModelStreamProgress::default();
    let mut context = ModelSamplingContext {
        run_id: "run",
        model_request_index: 0,
        user_text_blocked: false,
        event_stream: &mut events,
        tool_registry: &registry,
        tool_context: &tools,
    };
    for event in [
        start(1),
        reasoning(),
        LlmStreamEvent::AttemptReset {
            reason: "retry".to_string(),
        },
        start(2),
        reasoning(),
    ] {
        progress.on_event(&mut context, "run-stream-1", &cancellation, event);
    }
    cancellation.cancel();
    progress.on_event(
        &mut context,
        "run-stream-1",
        &cancellation,
        LlmStreamEvent::Committed,
    );
    // The callback is suppressed after cancellation; the unconditional sampling cleanup
    // is responsible for clearing the phase instead.
    assert_eq!(progress.model_activity, Some(AgentModelActivity::Reasoning));
    progress.set_activity(&mut context, "run-stream-1", AgentModelActivity::Waiting);
    progress.on_event(&mut context, "run-stream-1", &cancellation, reasoning());
    assert_eq!(
        activities(&captured.lock().unwrap()),
        [
            (1, AgentModelActivity::Waiting),
            (1, AgentModelActivity::Reasoning),
            (1, AgentModelActivity::Waiting),
            (2, AgentModelActivity::Waiting),
            (2, AgentModelActivity::Reasoning),
            (2, AgentModelActivity::Waiting),
        ]
    );
    assert!(!progress.received_model_output);
    assert!(progress.committed_message_stream_id.is_none());
}

#[test]
fn model_activity_event_has_only_the_bounded_public_status_contract() {
    assert_eq!(
        serde_json::to_value(AgentEvent::ModelActivityChanged {
            run_id: "run".to_string(),
            stream_id: "run-stream-1".to_string(),
            attempt: 2,
            activity: AgentModelActivity::Reasoning,
        })
        .unwrap(),
        json!({
            "type":"model_activity_changed",
            "runId":"run",
            "streamId":"run-stream-1",
            "attempt":2,
            "activity":"reasoning",
        })
    );
    assert_eq!(
        serde_json::to_value(AgentModelActivity::Waiting).unwrap(),
        json!("waiting")
    );
}

#[tokio::test]
async fn sampling_transport_cancellation_retry_and_nonstream_keep_activity_transient() {
    for mode in ["cancel", "retry", "nonstream", "json_fallback"] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let cancellation = AgentCancellationToken::new();
        let server_cancellation = cancellation.clone();
        let server = tokio::spawn(async move {
            for attempt in 1..=if mode == "retry" { 2 } else { 1 } {
                let (mut stream, _) = listener.accept().await.unwrap();
                read_request(&mut stream).await;
                if matches!(mode, "nonstream" | "json_fallback") {
                    let body = json!({"choices":[{"message":{
                        "role":"assistant","content":"visible","reasoning_content":"private"
                    },"finish_reason":"stop"}]})
                    .to_string();
                    stream.write_all(format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    ).as_bytes()).await.unwrap();
                    continue;
                }
                stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n").await.unwrap();
                stream.write_all(b"data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"private\"}}]}\n\n").await.unwrap();
                if mode == "cancel" {
                    server_cancellation.cancelled().await;
                } else if attempt == 1 {
                    stream.write_all(b"data: {invalid-json\n\n").await.unwrap();
                } else {
                    stream.write_all(b"data: {\"choices\":[{\"delta\":{\"content\":\"visible\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n").await.unwrap();
                }
            }
        });
        let captured = Arc::new(Mutex::new(Vec::new()));
        let sink = captured.clone();
        let event_cancellation = cancellation.clone();
        let mut events = AgentEventStream::new(Some(Arc::new(move |event| {
            if mode == "cancel"
                && matches!(
                    event,
                    AgentEvent::ModelActivityChanged {
                        activity: AgentModelActivity::Reasoning,
                        ..
                    }
                )
            {
                event_cancellation.cancel();
            }
            sink.lock().unwrap().push(event);
        })));
        let registry = ToolRegistry::defaults_with_search(None);
        let tools = ToolExecutionContext::from_run_context(None);
        let profile =
            ProviderProfileConfig::generic_for_dialect(AgentApiStyle::OpenAiCompatible.into());
        let protocol = ProviderProtocolKey::new(
            AgentApiStyle::OpenAiCompatible.into(),
            &profile,
            "test-model",
            None,
        )
        .unwrap();
        let request = LlmChatRequest {
            api_url: format!("http://{address}/v1/chat/completions"),
            api_token: "token".to_string(),
            provider_profile: profile,
            provider_protocol: protocol,
            max_tokens: Some(1024),
            temperature: 0.0,
            stream: mode != "nonstream",
            messages: vec![LlmMessage::text(LlmMessageRole::User, "Hello")],
            tools: Vec::new(),
        };
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            sample_model(
                request,
                cancellation,
                ModelSamplingContext {
                    run_id: "run",
                    model_request_index: 0,
                    user_text_blocked: true,
                    event_stream: &mut events,
                    tool_registry: &registry,
                    tool_context: &tools,
                },
            ),
        )
        .await
        .unwrap();
        server.await.unwrap();
        let expected = match mode {
            "cancel" => {
                assert!(result.response.unwrap_err().is_cancelled());
                vec![
                    (1, AgentModelActivity::Waiting),
                    (1, AgentModelActivity::Reasoning),
                    (1, AgentModelActivity::Waiting),
                ]
            }
            "retry" => {
                assert_eq!(result.response.unwrap().content(), "visible");
                vec![
                    (1, AgentModelActivity::Waiting),
                    (1, AgentModelActivity::Reasoning),
                    (1, AgentModelActivity::Waiting),
                    (2, AgentModelActivity::Waiting),
                    (2, AgentModelActivity::Reasoning),
                    (2, AgentModelActivity::Waiting),
                ]
            }
            _ => {
                assert_eq!(result.response.unwrap().content(), "visible");
                vec![(1, AgentModelActivity::Waiting)]
            }
        };
        assert_eq!(activities(&captured.lock().unwrap()), expected, "{mode}");
        assert!(!events
            .into_events()
            .iter()
            .any(|event| matches!(event, AgentEvent::ModelActivityChanged { .. })));
    }
}
