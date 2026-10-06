use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

fn delta(stream: Option<&str>, text: &str) -> AgentEvent {
    AgentEvent::MessageDelta {
        run_id: "run".into(),
        stream_id: stream.map(str::to_owned),
        delta: text.into(),
    }
}

fn done() -> AgentEvent {
    AgentEvent::Done {
        run_id: "run".into(),
        success: true,
        user_interrupted: None,
        status: Some(AgentRunStatus::Completed),
        content: Some("你好 🌍\n```rs\n```".into()),
        usage: Some(serde_json::from_value(json!({"inputTokens": 3, "outputTokens": 7})).unwrap()),
        finish_reason: Some("stop".into()),
        proposed_actions: Vec::new(),
    }
}

fn fixture() -> Vec<AgentEvent> {
    vec![
        AgentEvent::MessageStreamStarted {
            run_id: "run".into(),
            stream_id: "s1".into(),
            attempt: 1,
        },
        delta(Some("s1"), "failed attempt"),
        AgentEvent::MessageStreamReset {
            run_id: "run".into(),
            stream_id: "s1".into(),
            reason: "retry".into(),
        },
        AgentEvent::MessageStreamStarted {
            run_id: "run".into(),
            stream_id: "s2".into(),
            attempt: 2,
        },
        delta(Some("s2"), "你好 "),
        delta(Some("s2"), ""),
        delta(Some("s2"), "🌍\n```rs\n```"),
        AgentEvent::MessageStreamCommitted {
            run_id: "run".into(),
            stream_id: "s2".into(),
            trace_sequence: None,
        },
        delta(None, "non-streaming response"),
        AgentEvent::Error {
            run_id: Some("run".into()),
            trace_sequence: None,
            message: "recoverable".into(),
            recoverable: true,
            code: None,
            details: None,
        },
        done(),
    ]
}

#[test]
fn transient_message_deltas_preserve_live_bytes_boundaries_and_terminal_usage() {
    let captured = Arc::new(Mutex::new(Vec::new()));
    let sink = captured.clone();
    let mut stream = AgentEventStream::new(Some(Arc::new(move |event| {
        sink.lock().unwrap().push(event)
    })))
    .with_transient_message_deltas(true);
    let expected = fixture();
    for event in expected.clone() {
        stream.emit(event);
    }
    assert_eq!(json!(*captured.lock().unwrap()), json!(expected));
    let retained = stream.into_events();
    let expected_retained = expected
        .into_iter()
        .filter(|event| !matches!(event, AgentEvent::MessageDelta { .. }))
        .collect::<Vec<_>>();
    assert_eq!(json!(retained), json!(expected_retained));
    assert_eq!(json!(retained.last().unwrap())["usage"]["outputTokens"], 7);
}

#[test]
fn default_and_no_emitter_keep_the_complete_replay_log() {
    assert!(!AgentRuntimeHostServices::default().transient_message_deltas);
    assert!(
        AgentRuntimeHostServices::new()
            .with_transient_message_deltas()
            .transient_message_deltas
    );
    for (transient, emitter) in [
        (false, None),
        (true, None),
        (
            false,
            Some(Arc::new(|_: AgentEvent| {}) as AgentEventEmitter),
        ),
    ] {
        let mut stream = AgentEventStream::new(emitter).with_transient_message_deltas(transient);
        let expected = fixture();
        for event in expected.clone() {
            stream.emit(event);
        }
        assert_eq!(json!(stream.into_events()), json!(expected));
    }
}

#[test]
fn live_delta_moves_the_original_string_allocation_to_the_emitter() {
    let received = Arc::new(AtomicUsize::new(0));
    let sink = received.clone();
    let mut stream = AgentEventStream::new(Some(Arc::new(move |event| {
        if let AgentEvent::MessageDelta { delta, .. } = event {
            sink.store(delta.as_ptr() as usize, Ordering::Relaxed);
        }
    })))
    .with_transient_message_deltas(true);
    let text = "original allocation".to_owned();
    let original = text.as_ptr() as usize;
    stream.emit(AgentEvent::MessageDelta {
        run_id: "run".into(),
        stream_id: Some("stream".into()),
        delta: text,
    });
    assert_eq!(received.load(Ordering::Relaxed), original);
    assert!(stream.into_events().is_empty());
}

#[tokio::test]
async fn driver_applies_host_retention_only_when_live_delivery_is_available() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    for streaming in [false, true] {
        for (transient, live) in [(false, true), (true, true), (true, false)] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                loop {
                    let mut chunk = [0_u8; 4096];
                    let n = socket.read(&mut chunk).await.unwrap();
                    assert_ne!(n, 0);
                    request.extend_from_slice(&chunk[..n]);
                    if let Some(end) = request.windows(4).position(|v| v == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&request[..end]);
                        let length = headers
                            .lines()
                            .find_map(|line| {
                                line.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(|v| v.trim().parse::<usize>().unwrap())
                            })
                            .unwrap();
                        if request.len() >= end + 4 + length {
                            break;
                        }
                    }
                }
                let body = if streaming {
                    ["你好 ", "🌍\n```rs\n```"]
                        .into_iter()
                        .map(|text| {
                            format!(
                                "data: {}\n\n",
                                json!({"choices":[{"delta":{"content":text},"finish_reason":null}]})
                            )
                        })
                        .collect::<String>()
                        + &format!(
                            "data: {}\n\ndata: [DONE]\n\n",
                            json!({"choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":3,"completion_tokens":7,"total_tokens":10}})
                        )
                } else {
                    json!({"choices":[{"message":{"role":"assistant","content":"你好 🌍\n```rs\n```"},"finish_reason":"stop"}],"usage":{"prompt_tokens":3,"completion_tokens":7,"total_tokens":10}}).to_string()
                };
                let mime = if streaming {
                    "text/event-stream"
                } else {
                    "application/json"
                };
                let header = format!("HTTP/1.1 200 OK\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                socket.write_all(header.as_bytes()).await.unwrap();
                socket.write_all(body.as_bytes()).await.unwrap();
            });
            let mut input: AgentChatInput = serde_json::from_value(json!({
                "apiUrl": format!("http://{address}/v1/chat/completions"), "apiToken":"test-token", "model":"test-model",
                "modelCapabilities": {"imageInput":false}, "stream": streaming,
                "messages":[{"role":"user","content":"hello"}]
            })).unwrap();
            input.provider_profile_config = Some(ProviderProfileConfig::generic_for_dialect(
                ProviderProtocolDialect::OpenAiChatCompletions,
            ));
            let captured = Arc::new(Mutex::new(Vec::new()));
            let sink = captured.clone();
            let emitter: AgentEventEmitter =
                Arc::new(move |event| sink.lock().unwrap().push(event));
            let services = if transient {
                AgentRuntimeHostServices::new().with_transient_message_deltas()
            } else {
                AgentRuntimeHostServices::new()
            };
            let output = AgentRuntime::default()
                .send_chat_with_events_and_cancellation(
                    input,
                    Some("run".into()),
                    live.then_some(emitter),
                    AgentCancellationToken::new(),
                    Some(services),
                )
                .await
                .unwrap();
            server.await.unwrap();
            assert_eq!(output.content, "你好 🌍\n```rs\n```");
            assert_eq!(output.status, AgentRunStatus::Completed);
            assert!(!output
                .events
                .iter()
                .any(|event| matches!(event, AgentEvent::FinalAnswerReady { .. })));
            assert_eq!(
                output
                    .events
                    .iter()
                    .any(|event| matches!(event, AgentEvent::MessageDelta { .. })),
                !transient || !live
            );
            assert!(output.events.iter().any(|event| matches!(event, AgentEvent::Done { usage: Some(usage), .. } if usage.output_tokens == Some(7))));
            if live {
                let text = captured
                    .lock()
                    .unwrap()
                    .iter()
                    .filter_map(|event| match event {
                        AgentEvent::MessageDelta { delta, .. } => Some(delta.as_str()),
                        _ => None,
                    })
                    .collect::<String>();
                assert_eq!(text, output.content);
                let captured = captured.lock().unwrap();
                let ready = captured
                    .iter()
                    .enumerate()
                    .filter_map(|(index, event)| {
                        matches!(event, AgentEvent::FinalAnswerReady { run_id } if run_id == "run")
                            .then_some(index)
                    })
                    .collect::<Vec<_>>();
                assert_eq!(ready.len(), 1);
                let ready = ready[0];
                let last_delta = captured
                    .iter()
                    .rposition(|event| matches!(event, AgentEvent::MessageDelta { .. }))
                    .unwrap();
                assert!(last_delta < ready);
                assert!(
                    matches!(&captured[ready + 1], AgentEvent::State { state, .. } if state.status == AgentRunStatus::Completed)
                );
                assert!(matches!(
                    &captured[ready + 2],
                    AgentEvent::Done {
                        status: Some(AgentRunStatus::Completed),
                        ..
                    }
                ));
            }
        }
    }
}

// Explicit opt-in synthetic microbenchmark; ordinary test runs have no timing assertion.
// The baseline below copies the pre-change emit implementation from 1e711e5395ee17d1.
#[test]
fn stream_retention_benchmark() {
    let Ok(mode) = std::env::var("MYCOPILOT_STREAM_RETENTION_BENCH") else {
        return;
    };
    let agents: usize = std::env::var("MYCOPILOT_STREAM_AGENTS")
        .unwrap()
        .parse()
        .unwrap();
    assert!([1, 4, 8].contains(&agents));
    assert!(["baseline", "live"].contains(&mode.as_str()));
    let live = mode == "live";
    let baseline_rss = rss_kib();
    let started = std::time::Instant::now();
    let workers = (0..agents)
        .map(|_| {
            std::thread::spawn(move || {
                let bytes = Arc::new(AtomicUsize::new(0));
                let count = Arc::new(AtomicUsize::new(0));
                let sink_bytes = bytes.clone();
                let sink_count = count.clone();
                let emitter: AgentEventEmitter = Arc::new(move |event| {
                    sink_count.fetch_add(1, Ordering::Relaxed);
                    if let AgentEvent::MessageDelta { delta, .. } = event {
                        sink_bytes.fetch_add(delta.len(), Ordering::Relaxed);
                    }
                });
                let mut stream = AgentEventStream::new(Some(emitter.clone()))
                    .with_transient_message_deltas(true);
                let mut legacy_events = Vec::new();
                for _ in 0..20_000 {
                    let event = delta(Some("stream"), "你好 🌍\n```rs\nlet x=1;\n```");
                    if live {
                        stream.emit(event);
                    } else {
                        emitter(event.clone());
                        legacy_events.push(event);
                    }
                }
                if live {
                    stream.emit(done());
                } else {
                    let event = done();
                    emitter(event.clone());
                    legacy_events.push(event);
                }
                (
                    if live {
                        stream.into_events()
                    } else {
                        legacy_events
                    },
                    count.load(Ordering::Relaxed),
                    bytes.load(Ordering::Relaxed),
                )
            })
        })
        .collect::<Vec<_>>();
    let results = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .collect::<Vec<_>>();
    let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
    let rss = rss_kib();
    let retained_count = results
        .iter()
        .map(|(events, _, _)| events.len())
        .sum::<usize>();
    let retained_inline_capacity = results
        .iter()
        .map(|(events, _, _)| events.capacity() * std::mem::size_of::<AgentEvent>())
        .sum::<usize>();
    let delta_bytes = results
        .iter()
        .flat_map(|(events, _, _)| events)
        .filter_map(|event| match event {
            AgentEvent::MessageDelta { delta, .. } => Some(delta.len()),
            _ => None,
        })
        .sum::<usize>();
    let delivered = results.iter().map(|(_, count, _)| count).sum::<usize>();
    let delivered_bytes = results.iter().map(|(_, _, bytes)| bytes).sum::<usize>();
    assert_eq!(delivered, agents * 20_001);
    assert_eq!(
        delivered_bytes,
        agents * 20_000 * "你好 🌍\n```rs\nlet x=1;\n```".len()
    );
    assert_eq!(retained_count, if live { agents } else { agents * 20_001 });
    println!(
        "STREAM_RETENTION {}",
        json!({"mode":mode,"agents":agents,"deltasPerAgent":20000,"eventSizeBytes":std::mem::size_of::<AgentEvent>(),"elapsedMs":elapsed_ms,"delivered":delivered,"deliveredBytes":delivered_bytes,"retainedEvents":retained_count,"retainedDeltaBytes":delta_bytes,"retainedEventVecCapacityBytes":retained_inline_capacity,"baselineRssKiB":baseline_rss,"retainedRssKiB":rss,"deltaClones":if live {0} else {agents*20000}})
    );
    drop(results);
    println!(
        "STREAM_RELEASE {}",
        json!({"mode":mode,"agents":agents,"rssKiB":rss_kib()})
    );
}

fn rss_kib() -> Option<u64> {
    let output = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .ok()?;
    String::from_utf8(output.stdout).ok()?.trim().parse().ok()
}
