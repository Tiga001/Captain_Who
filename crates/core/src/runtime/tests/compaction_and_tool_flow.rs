use super::*;

#[tokio::test]
async fn durable_compaction_runs_before_capacity_gate_and_then_sends_rebuilt_context() {
    use crate::context::{ContextCompactionGeneration, ContextCompactionSummary};
    use crate::protocol::AgentApiStyle;
    use crate::{
        AgentUsage, ContextCompactionPrefix, ContextCompactionSourceItem,
        ContextCompactionSummaryDraft, ContextJournalCursor,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    async fn read_http_body(stream: &mut TcpStream) -> Vec<u8> {
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4_096];
        let mut expected_length = None;
        loop {
            let read = stream.read(&mut buffer).await.unwrap();
            if read == 0 {
                break;
            }
            request.extend_from_slice(&buffer[..read]);
            if expected_length.is_none() {
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
                    expected_length = Some(header_end + 4 + content_length);
                }
            }
            if expected_length.is_some_and(|length| request.len() >= length) {
                break;
            }
        }
        let body_start = request
            .windows(4)
            .position(|part| part == b"\r\n\r\n")
            .map(|index| index + 4)
            .unwrap();
        request[body_start..].to_vec()
    }

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let request_bodies = Arc::new(Mutex::new(Vec::new()));
    let request_bodies_for_server = request_bodies.clone();
    let server = tokio::spawn(async move {
        for request_index in 0..2 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let body = read_http_body(&mut stream).await;
            request_bodies_for_server.lock().unwrap().push(body);
            let response = if request_index == 0 {
                json!({
                    "choices": [{
                        "message": {
                            "role": "assistant",
                            "content": null,
                            "tool_calls": [{
                                "id": "todo-after-compaction",
                                "type": "function",
                                "function": {
                                    "name": "todo_update",
                                    "arguments": serde_json::to_string(&json!({
                                        "items": [{
                                            "title": "Verify compacted context",
                                            "status": "completed"
                                        }]
                                    }))
                                    .unwrap()
                                }
                            }]
                        },
                        "finish_reason": "tool_calls"
                    }]
                })
            } else {
                json!({
                    "choices": [{
                        "message": { "role": "assistant", "content": "done" },
                        "finish_reason": "stop"
                    }]
                })
            };
            let response_body = serde_json::to_vec(&response).unwrap();
            let headers = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                response_body.len()
            );
            stream.write_all(headers.as_bytes()).await.unwrap();
            stream.write_all(&response_body).await.unwrap();
        }
    });

    let old_user = AgentChatMessage {
        message_id: Some("user-old".to_string()),
        role: "user".to_string(),
        content: format!("OLD_USER_MARKER {}", "x".repeat(60_000)),
        created_at: None,
        conversation_turn_trace: None,
        conversation_model_context_items: Vec::new(),
    };
    let old_assistant_content = format!("OLD_ASSISTANT_MARKER {}", "y".repeat(60_000));
    let mut old_assistant_trace = conversation_context_trace(
        ConversationTurnTraceTerminalStatus::Completed,
        vec![ConversationTurnTraceItem::AssistantNarration {
            sequence: 0,
            content: old_assistant_content.clone(),
            truncated: false,
        }],
    );
    old_assistant_trace.run_id = "run-old".to_string();
    old_assistant_trace.conversation_id = "conversation-1".to_string();
    old_assistant_trace.assistant_message_id = "assistant-old".to_string();
    let old_assistant = traced_assistant_message(&old_assistant_content, old_assistant_trace);
    let current_user = AgentChatMessage {
        message_id: Some("user-current".to_string()),
        role: "user".to_string(),
        content: "continue".to_string(),
        created_at: None,
        conversation_turn_trace: None,
        conversation_model_context_items: Vec::new(),
    };
    let mut input = AgentChatInput {
        api_url: format!("http://{address}/v1/chat/completions"),
        api_token: "test-token".to_string(),
        provider_configuration_revision: None,
        provider_connection_revision: None,
        search_connection_revision: None,
        provider_profile_config: None,
        provider_protocol_key: None,
        model: "test-model".to_string(),
        model_capabilities: crate::ModelCapabilities::default(),
        api_style: Some(AgentApiStyle::OpenAiCompatible),
        context_window_tokens: Some(50_000),
        context_window_indicator_enabled: false,
        max_tokens: Some(1_000),
        temperature: None,
        stream: Some(false),
        context: Some(AgentRunContext {
            collaboration_identity: None,
            conversation_id: Some("conversation-1".to_string()),
            project_id: None,
            workspace: None,
            attachment_library: None,
            permissions: Default::default(),
        }),
        search_config: None,
        prompt_preferences: None,
        approval_decision: None,
        tool_continuation: None,
        attachments: Vec::new(),
        resume_checkpoint: None,
        assistant_message_id: Some("assistant-current".to_string()),
        context_compaction_summary: None,
        world_state_records: Vec::new(),
        skill_activation: None,
        skill_discovery: None,
        messages: vec![old_user, old_assistant, current_user.clone()],
    };
    freeze_runtime_test_generic_provider(&mut input, "durable-compaction-capacity");
    let durable_prefix = Arc::new(ContextCompactionPrefix {
        conversation_id: "conversation-1".to_string(),
        source_revision: "source-runtime".to_string(),
        covered_through: ContextJournalCursor::message("user-current"),
        previous_summary: None,
        source_items: vec![
            ContextCompactionSourceItem::Message {
                cursor: ContextJournalCursor::message("user-old"),
                role: "user".to_string(),
                content: "old request".to_string(),
                created_at: 1,
                status: Some("sent".to_string()),
                terminal_status: None,
                terminal_error: None,
            },
            ContextCompactionSourceItem::Message {
                cursor: ContextJournalCursor::message("assistant-old"),
                role: "assistant".to_string(),
                content: "old answer".to_string(),
                created_at: 2,
                status: Some("sent".to_string()),
                terminal_status: None,
                terminal_error: None,
            },
            ContextCompactionSourceItem::Message {
                cursor: ContextJournalCursor::message("user-current"),
                role: "user".to_string(),
                content: "continue".to_string(),
                created_at: 3,
                status: Some("sent".to_string()),
                terminal_status: None,
                terminal_error: None,
            },
        ],
    });
    let compacted_continuity = crate::ContextContinuitySnapshot::from_prefix(&durable_prefix)
        .expect("test durable prefix should produce continuity records");
    let compacted_summary = ContextCompactionSummary {
        schema_version: crate::CONTEXT_COMPACTION_SUMMARY_SCHEMA_VERSION,
        id: "summary-runtime".to_string(),
        conversation_id: "conversation-1".to_string(),
        source_revision: "source-runtime".to_string(),
        previous_summary_id: None,
        covered_through: ContextJournalCursor::message("user-current"),
        content: "COMPACTED_HISTORY_MARKER: the old task was completed.".to_string(),
        continuity: compacted_continuity,
        generation: ContextCompactionGeneration::test(),
        source_input_tokens: 40_000,
        summary_input_tokens: 32,
        continuity_input_tokens: 64,
        uncovered_tail_input_tokens: 0,
        replacement_input_tokens: 96,
        created_at: 1,
    };
    let mut compacted_state = create_conversation_context_state(AgentChatInput {
        context_compaction_summary: Some(compacted_summary),
        messages: vec![current_user],
        ..input.clone()
    })
    .unwrap();
    let compacted_baseline = compacted_state.shared_baseline().unwrap();
    let mut uncompacted_state = create_conversation_context_state(input.clone()).unwrap();
    let uncompacted_baseline = uncompacted_state.shared_baseline().unwrap();
    let prepare_count = Arc::new(AtomicUsize::new(0));
    let generate_count = Arc::new(AtomicUsize::new(0));
    let commit_count = Arc::new(AtomicUsize::new(0));
    let trace_publish_count = Arc::new(AtomicUsize::new(0));
    let trace_snapshots = Arc::new(Mutex::new(Vec::new()));
    let prepare_counter = prepare_count.clone();
    let generate_counter = generate_count.clone();
    let commit_counter = commit_count.clone();
    let commit_count_for_trace = commit_count.clone();
    let trace_publish_counter = trace_publish_count.clone();
    let trace_snapshots_for_observer = trace_snapshots.clone();
    let compacted_baseline_for_commit = compacted_baseline.clone();
    let compacted_baseline_for_trace = compacted_baseline.clone();
    let durable_prefix_for_prepare = durable_prefix.clone();
    let steer_input = AgentSteerInputQueue::new();
    let steer_input_during_compaction = steer_input.clone();
    let services = AgentContextCompactionServices::new(
        move |request, _| {
            prepare_counter.fetch_add(1, Ordering::SeqCst);
            assert_eq!(
                request.covered_through,
                ContextJournalCursor::message("user-current")
            );
            let durable_prefix = durable_prefix_for_prepare.clone();
            async move { Ok(AgentContextCompactionPrepareOutcome::Ready(durable_prefix)) }
        },
        move |request, _| {
            generate_counter.fetch_add(1, Ordering::SeqCst);
            let large_attachment_text =
                format!("LARGE_GUIDANCE_ATTACHMENT_MARKER {}", "z".repeat(20_000));
            assert_eq!(
                steer_input_during_compaction
                    .enqueue(crate::AgentSteerInput {
                        guidance_id: "guidance-during-compaction".to_string(),
                        client_message_id: "client-during-compaction".to_string(),
                        content: "Preserve this constraint across compaction.".to_string(),
                        attachments: vec![AgentInputAttachment {
                            id: "attachment-during-compaction".to_string(),
                            kind: AgentInputAttachmentKind::File,
                            name: "large-guidance.txt".to_string(),
                            mime_type: Some("text/plain".to_string()),
                            size_bytes: large_attachment_text.len() as u64,
                            encoding: AgentInputAttachmentEncoding::Utf8,
                            data: large_attachment_text,
                            truncated: None,
                        }],
                        attachment_library: Some(crate::AgentAttachmentLibraryContext {
                            root_path: None,
                            conversation_id: Some("conversation-1".to_string()),
                            project_id: None,
                            conversation_attachments: Vec::new(),
                            project_attachments: Vec::new(),
                        }),
                        created_at: 42,
                    })
                    .unwrap(),
                AgentSteerEnqueueOutcome::Queued
            );
            async move {
                let observation =
                    crate::model_request_observation::ModelRequestObservationBuilder::new(
                        format!("model-request-{}", request.operation_id),
                        request.run_id.clone(),
                        Some(request.conversation_id.clone()),
                        Some(request.assistant_message_id.clone()),
                        Some(request.operation_id.clone()),
                        request.request_index,
                        crate::ModelRequestPurpose::ContextCompaction,
                        "test-model",
                        AgentApiStyle::OpenAiCompatible,
                        None,
                        1,
                    )
                    .completed(
                        Some(AgentUsage {
                            input_tokens: Some(100),
                            output_tokens: Some(20),
                            output_thinking_tokens: None,
                            total_tokens: Some(120),
                            cached_input_tokens: None,
                            cache_creation_input_tokens: None,
                            billable_request_count: Some(1),
                        }),
                        Some("stop".to_string()),
                        2,
                    )
                    .unwrap();
                Ok(AgentContextCompactionGenerationOutput {
                    draft: ContextCompactionSummaryDraft {
                        id: "summary-runtime".to_string(),
                        source_revision: request.prefix.source_revision.clone(),
                        content: "COMPACTED_HISTORY_MARKER: the old task was completed."
                            .to_string(),
                        continuity: request.continuity,
                        generation: ContextCompactionGeneration::test(),
                        source_input_tokens: request.source_input_tokens,
                        summary_input_tokens: 32,
                        continuity_input_tokens: 64,
                        uncovered_tail_input_tokens: request.uncovered_tail_input_tokens,
                        replacement_input_tokens: 96,
                        created_at: 1,
                    },
                    observation,
                })
            }
        },
        move |request, _| {
            let baseline = compacted_baseline_for_commit.clone();
            commit_counter.fetch_add(1, Ordering::SeqCst);
            assert_eq!(request.draft.id, "summary-runtime");
            async move {
                Ok(AgentContextCompactionCommitOutcome::Applied {
                    summary_id: "summary-runtime".to_string(),
                    baseline: Box::new(baseline),
                })
            }
        },
        |_, _| async { Ok(()) },
    );
    let emitted_events = Arc::new(Mutex::new(Vec::new()));
    let emitted_events_for_callback = emitted_events.clone();
    let emitter: AgentEventEmitter = Arc::new(move |event| {
        emitted_events_for_callback.lock().unwrap().push(event);
    });
    let observations = Arc::new(Mutex::new(Vec::new()));
    let observations_for_callback = observations.clone();
    let model_request_observer: AgentModelRequestObserver = Arc::new(move |observation| {
        observations_for_callback.lock().unwrap().push(observation);
    });
    let trace_observer: AgentConversationTraceObserver = Arc::new(move |snapshot| {
        trace_publish_counter.fetch_add(1, Ordering::SeqCst);
        trace_snapshots_for_observer.lock().unwrap().push(snapshot);
        let baseline = if commit_count_for_trace.load(Ordering::SeqCst) == 0 {
            uncompacted_baseline.clone()
        } else {
            compacted_baseline_for_trace.clone()
        };
        Ok(Some(baseline))
    });

    let output = AgentRuntime::default()
        .send_chat_with_events_and_cancellation(
            input,
            Some("run-compaction".to_string()),
            Some(emitter),
            AgentCancellationToken::new(),
            Some(
                AgentRuntimeHostServices::new()
                    .with_context_compaction(services)
                    .with_trace_observer(trace_observer)
                    .with_model_request_observer(model_request_observer)
                    .with_steer_input(steer_input),
            ),
        )
        .await
        .unwrap();
    server.await.unwrap();
    let request_bodies = request_bodies
        .lock()
        .unwrap()
        .iter()
        .cloned()
        .map(|body| String::from_utf8(body).unwrap())
        .collect::<Vec<_>>();

    assert_eq!(output.content, "done");
    assert_eq!(request_bodies.len(), 2);
    assert_eq!(prepare_count.load(Ordering::SeqCst), 1);
    assert_eq!(generate_count.load(Ordering::SeqCst), 1);
    assert_eq!(commit_count.load(Ordering::SeqCst), 1);
    assert_eq!(trace_publish_count.load(Ordering::SeqCst), 8);
    let usage = output.usage.as_ref().unwrap();
    assert_eq!(usage.input_tokens, Some(100));
    assert_eq!(usage.output_tokens, Some(20));
    assert_eq!(usage.billable_request_count, Some(3));
    let observations = observations.lock().unwrap();
    assert_eq!(observations.len(), 2);
    assert!(observations.iter().all(|observation| {
        observation.purpose == crate::ModelRequestPurpose::AgentLoop
            && observation.estimate.is_some()
    }));
    for request_body in &request_bodies {
        assert!(request_body.contains("COMPACTED_HISTORY_MARKER"));
        assert!(!request_body.contains("OLD_USER_MARKER"));
        assert!(!request_body.contains("OLD_ASSISTANT_MARKER"));
    }
    assert!(!request_bodies[0].contains("Preserve this constraint across compaction."));
    assert!(request_bodies[1].contains("Preserve this constraint across compaction."));
    assert!(request_bodies[1].contains("LARGE_GUIDANCE_ATTACHMENT_MARKER"));
    assert!(output.events.iter().any(|event| matches!(
        event,
        AgentEvent::GuidanceApplied { guidance_id, .. }
            if guidance_id == "guidance-during-compaction"
    )));
    let events = emitted_events.lock().unwrap();
    let compaction_events = events
        .iter()
        .filter(|event| {
            matches!(
                event,
                AgentEvent::ContextCompactionStarted { .. }
                    | AgentEvent::ContextCompactionFinished { .. }
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(compaction_events.len(), 2);
    let AgentEvent::ContextCompactionStarted {
        operation_id,
        trace_sequence,
        ..
    } = compaction_events[0]
    else {
        panic!("first compaction event should start the operation");
    };
    let AgentEvent::ContextCompactionFinished {
        operation_id: finished_operation_id,
        outcome,
        trace_sequence: finished_trace_sequence,
        ..
    } = compaction_events[1]
    else {
        panic!("second compaction event should finish the operation");
    };
    assert_eq!(finished_operation_id, operation_id);
    assert_eq!(*outcome, AgentContextCompactionEventOutcome::Applied);
    assert_eq!(finished_trace_sequence, trace_sequence);
    let snapshots = trace_snapshots.lock().unwrap();
    let settled_snapshot = snapshots
        .iter()
        .rev()
        .find(|snapshot| {
            snapshot.items.iter().any(|item| matches!(
                item,
                ConversationTurnTraceItem::ContextCompactionLifecycle {
                    phase: crate::conversation_trace::ConversationContextCompactionLifecyclePhase::Finished,
                    ..
                }
            ))
        })
        .expect("compaction settlement must be published durably before its live event");
    let compaction_items = settled_snapshot
        .items
        .iter()
        .filter(|item| {
            matches!(
                item,
                ConversationTurnTraceItem::ContextCompactionLifecycle { .. }
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(compaction_items.len(), 2);
    assert_eq!(compaction_items[0].sequence(), *trace_sequence);
    assert!(compaction_items[1].sequence() > compaction_items[0].sequence());
}

#[tokio::test]
async fn recursive_compaction_starts_when_the_assembled_system_summary_is_already_present() {
    use crate::context::{
        ContextCompactionGeneration, ContextCompactionPlanStatus, ContextCompactionPlanner,
        ContextCompactionSummary,
    };
    use crate::protocol::AgentApiStyle;
    use crate::{
        AgentUsage, ContextCompactionPrefix, ContextCompactionSourceItem,
        ContextCompactionSummaryDraft, ContextJournalCursor,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    async fn read_http_body(stream: &mut TcpStream) -> Vec<u8> {
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4_096];
        let mut expected_length = None;
        loop {
            let read = stream.read(&mut buffer).await.unwrap();
            if read == 0 {
                break;
            }
            request.extend_from_slice(&buffer[..read]);
            if expected_length.is_none() {
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
                    expected_length = Some(header_end + 4 + content_length);
                }
            }
            if expected_length.is_some_and(|length| request.len() >= length) {
                break;
            }
        }
        let body_start = request
            .windows(4)
            .position(|part| part == b"\r\n\r\n")
            .map(|index| index + 4)
            .unwrap();
        request[body_start..].to_vec()
    }

    let previous_summary_prefix = ContextCompactionPrefix {
        conversation_id: "conversation-1".to_string(),
        source_revision: "source-previous".to_string(),
        covered_through: ContextJournalCursor::message("assistant-old"),
        previous_summary: None,
        source_items: vec![
            ContextCompactionSourceItem::Message {
                cursor: ContextJournalCursor::message("user-old"),
                role: "user".to_string(),
                content: "old request".to_string(),
                created_at: 1,
                status: Some("sent".to_string()),
                terminal_status: None,
                terminal_error: None,
            },
            ContextCompactionSourceItem::Message {
                cursor: ContextJournalCursor::message("assistant-old"),
                role: "assistant".to_string(),
                content: "old answer".to_string(),
                created_at: 2,
                status: Some("sent".to_string()),
                terminal_status: None,
                terminal_error: None,
            },
        ],
    };
    let previous_summary = ContextCompactionSummary {
        schema_version: crate::CONTEXT_COMPACTION_SUMMARY_SCHEMA_VERSION,
        id: "summary-previous".to_string(),
        conversation_id: "conversation-1".to_string(),
        source_revision: previous_summary_prefix.source_revision.clone(),
        previous_summary_id: None,
        covered_through: previous_summary_prefix.covered_through.clone(),
        content: "PREVIOUS_SUMMARY_MARKER: the old task was completed.".to_string(),
        continuity: crate::ContextContinuitySnapshot::from_prefix(&previous_summary_prefix)
            .expect("previous summary prefix should produce continuity records"),
        generation: ContextCompactionGeneration::test(),
        source_input_tokens: 40_000,
        summary_input_tokens: 32,
        continuity_input_tokens: 64,
        uncovered_tail_input_tokens: 0,
        replacement_input_tokens: 96,
        created_at: 1,
    };
    let grown_user = AgentChatMessage {
        message_id: Some("user-grown".to_string()),
        role: "user".to_string(),
        content: format!("GROWN_USER_MARKER {}", "x".repeat(60_000)),
        created_at: None,
        conversation_turn_trace: None,
        conversation_model_context_items: Vec::new(),
    };
    let grown_assistant_content = format!("GROWN_HISTORY_MARKER {}", "y".repeat(60_000));
    let mut grown_assistant_trace = conversation_context_trace(
        ConversationTurnTraceTerminalStatus::Completed,
        vec![ConversationTurnTraceItem::AssistantNarration {
            sequence: 0,
            content: grown_assistant_content.clone(),
            truncated: false,
        }],
    );
    grown_assistant_trace.run_id = "run-grown".to_string();
    grown_assistant_trace.conversation_id = "conversation-1".to_string();
    grown_assistant_trace.assistant_message_id = "assistant-grown".to_string();
    let grown_assistant = traced_assistant_message(&grown_assistant_content, grown_assistant_trace);
    let current_user = AgentChatMessage {
        message_id: Some("user-current".to_string()),
        role: "user".to_string(),
        content: "continue after the first compaction".to_string(),
        created_at: None,
        conversation_turn_trace: None,
        conversation_model_context_items: Vec::new(),
    };
    let mut input = AgentChatInput {
        api_url: "http://127.0.0.1:0/v1/chat/completions".to_string(),
        api_token: "test-token".to_string(),
        provider_configuration_revision: None,
        provider_connection_revision: None,
        search_connection_revision: None,
        provider_profile_config: None,
        provider_protocol_key: None,
        model: "test-model".to_string(),
        model_capabilities: crate::ModelCapabilities::default(),
        api_style: Some(AgentApiStyle::OpenAiCompatible),
        context_window_tokens: Some(50_000),
        context_window_indicator_enabled: false,
        max_tokens: Some(1_000),
        temperature: None,
        stream: Some(false),
        context: Some(AgentRunContext {
            collaboration_identity: None,
            conversation_id: Some("conversation-1".to_string()),
            project_id: None,
            workspace: None,
            attachment_library: None,
            permissions: Default::default(),
        }),
        search_config: None,
        prompt_preferences: None,
        approval_decision: None,
        tool_continuation: None,
        attachments: Vec::new(),
        resume_checkpoint: None,
        assistant_message_id: Some("assistant-current".to_string()),
        context_compaction_summary: Some(previous_summary.clone()),
        world_state_records: Vec::new(),
        skill_activation: None,
        skill_discovery: None,
        messages: vec![
            grown_user.clone(),
            grown_assistant.clone(),
            current_user.clone(),
        ],
    };
    freeze_runtime_test_generic_provider(&mut input, "recursive-compaction-system-summary");

    let mut assembled_state = create_conversation_context_state(input.clone()).unwrap();
    let mut assembled_frame = assembled_state.shared_baseline().unwrap().into_frame();
    let detector =
        ContextCapacityDetector::for_model(&input.model, AgentApiStyle::OpenAiCompatible, &[]);
    let report = detector.inspect(
        &mut assembled_frame,
        input.context_window_tokens,
        sanitize_max_tokens(input.max_tokens),
    );
    let assembled_plan = ContextCompactionPlanner::for_tools(&[]).plan(
        &report.compaction_query(),
        &assembled_frame.planning_items().unwrap(),
        true,
    );
    assert_eq!(assembled_plan.status, ContextCompactionPlanStatus::Required);
    assert_eq!(
        assembled_plan.steps[0]
            .durable_prefix
            .as_ref()
            .and_then(|prefix| prefix.previous_summary_id.as_deref()),
        Some("summary-previous")
    );

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    input.api_url = format!("http://{address}/v1/chat/completions");
    freeze_runtime_test_generic_provider(&mut input, "recursive-compaction-system-summary");
    let request_bodies = Arc::new(Mutex::new(Vec::new()));
    let request_bodies_for_server = request_bodies.clone();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let body = read_http_body(&mut stream).await;
        request_bodies_for_server.lock().unwrap().push(body);
        let response_body = serde_json::to_vec(&json!({
            "choices": [{
                "message": { "role": "assistant", "content": "done after recursive compaction" },
                "finish_reason": "stop"
            }]
        }))
        .unwrap();
        let headers = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            response_body.len()
        );
        stream.write_all(headers.as_bytes()).await.unwrap();
        stream.write_all(&response_body).await.unwrap();
    });

    let recursive_prefix = Arc::new(ContextCompactionPrefix {
        conversation_id: "conversation-1".to_string(),
        source_revision: "source-recursive".to_string(),
        covered_through: ContextJournalCursor::message("user-current"),
        previous_summary: Some(previous_summary.clone()),
        source_items: vec![
            ContextCompactionSourceItem::Message {
                cursor: ContextJournalCursor::message("user-grown"),
                role: "user".to_string(),
                content: "grown request".to_string(),
                created_at: 3,
                status: Some("sent".to_string()),
                terminal_status: None,
                terminal_error: None,
            },
            ContextCompactionSourceItem::Message {
                cursor: ContextJournalCursor::message("assistant-grown"),
                role: "assistant".to_string(),
                content: "grown history".to_string(),
                created_at: 4,
                status: Some("sent".to_string()),
                terminal_status: None,
                terminal_error: None,
            },
            ContextCompactionSourceItem::Message {
                cursor: ContextJournalCursor::message("user-current"),
                role: "user".to_string(),
                content: "continue after the first compaction".to_string(),
                created_at: 5,
                status: Some("sent".to_string()),
                terminal_status: None,
                terminal_error: None,
            },
        ],
    });
    let recursive_summary = ContextCompactionSummary {
        schema_version: crate::CONTEXT_COMPACTION_SUMMARY_SCHEMA_VERSION,
        id: "summary-recursive".to_string(),
        conversation_id: "conversation-1".to_string(),
        source_revision: recursive_prefix.source_revision.clone(),
        previous_summary_id: Some("summary-previous".to_string()),
        covered_through: recursive_prefix.covered_through.clone(),
        content: "RECURSIVE_SUMMARY_MARKER: later history was compacted.".to_string(),
        continuity: crate::ContextContinuitySnapshot::from_prefix(&recursive_prefix)
            .expect("recursive prefix should produce continuity records"),
        generation: ContextCompactionGeneration::test(),
        source_input_tokens: 40_000,
        summary_input_tokens: 32,
        continuity_input_tokens: 64,
        uncovered_tail_input_tokens: 0,
        replacement_input_tokens: 96,
        created_at: 2,
    };
    let mut compacted_state = create_conversation_context_state(AgentChatInput {
        context_compaction_summary: Some(recursive_summary.clone()),
        messages: vec![current_user.clone()],
        ..input.clone()
    })
    .unwrap();
    let compacted_baseline = compacted_state.shared_baseline().unwrap();
    let prepare_count = Arc::new(AtomicUsize::new(0));
    let generate_count = Arc::new(AtomicUsize::new(0));
    let commit_count = Arc::new(AtomicUsize::new(0));
    let prepare_counter = prepare_count.clone();
    let generate_counter = generate_count.clone();
    let commit_counter = commit_count.clone();
    let compacted_baseline_for_commit = compacted_baseline.clone();
    let recursive_prefix_for_prepare = recursive_prefix.clone();
    let services = AgentContextCompactionServices::new(
        move |request, _| {
            prepare_counter.fetch_add(1, Ordering::SeqCst);
            assert_eq!(
                request.expected_previous_summary_id.as_deref(),
                Some("summary-previous")
            );
            assert_eq!(
                request.covered_through,
                ContextJournalCursor::message("user-current")
            );
            let recursive_prefix = recursive_prefix_for_prepare.clone();
            async move {
                Ok(AgentContextCompactionPrepareOutcome::Ready(
                    recursive_prefix,
                ))
            }
        },
        move |request, _| {
            generate_counter.fetch_add(1, Ordering::SeqCst);
            assert_eq!(
                request
                    .prefix
                    .previous_summary
                    .as_ref()
                    .map(|summary| summary.id.as_str()),
                Some("summary-previous")
            );
            async move {
                let observation =
                    crate::model_request_observation::ModelRequestObservationBuilder::new(
                        format!("model-request-{}", request.operation_id),
                        request.run_id.clone(),
                        Some(request.conversation_id.clone()),
                        Some(request.assistant_message_id.clone()),
                        Some(request.operation_id.clone()),
                        request.request_index,
                        crate::ModelRequestPurpose::ContextCompaction,
                        "test-model",
                        AgentApiStyle::OpenAiCompatible,
                        None,
                        1,
                    )
                    .completed(
                        Some(AgentUsage {
                            input_tokens: Some(80),
                            output_tokens: Some(16),
                            output_thinking_tokens: None,
                            total_tokens: Some(96),
                            cached_input_tokens: None,
                            cache_creation_input_tokens: None,
                            billable_request_count: Some(1),
                        }),
                        Some("stop".to_string()),
                        2,
                    )
                    .unwrap();
                Ok(AgentContextCompactionGenerationOutput {
                    draft: ContextCompactionSummaryDraft {
                        id: "summary-recursive".to_string(),
                        source_revision: request.prefix.source_revision.clone(),
                        content: "RECURSIVE_SUMMARY_MARKER: later history was compacted."
                            .to_string(),
                        continuity: request.continuity,
                        generation: ContextCompactionGeneration::test(),
                        source_input_tokens: request.source_input_tokens,
                        summary_input_tokens: 32,
                        continuity_input_tokens: 64,
                        uncovered_tail_input_tokens: request.uncovered_tail_input_tokens,
                        replacement_input_tokens: 96,
                        created_at: 2,
                    },
                    observation,
                })
            }
        },
        move |request, _| {
            let baseline = compacted_baseline_for_commit.clone();
            commit_counter.fetch_add(1, Ordering::SeqCst);
            assert_eq!(request.draft.id, "summary-recursive");
            async move {
                Ok(AgentContextCompactionCommitOutcome::Applied {
                    summary_id: "summary-recursive".to_string(),
                    baseline: Box::new(baseline),
                })
            }
        },
        |_, _| async { Ok(()) },
    );
    let emitted_events = Arc::new(Mutex::new(Vec::new()));
    let emitted_events_for_callback = emitted_events.clone();
    let emitter: AgentEventEmitter = Arc::new(move |event| {
        emitted_events_for_callback.lock().unwrap().push(event);
    });

    let output = AgentRuntime::default()
        .send_chat_with_events_and_cancellation(
            input,
            Some("run-recursive-compaction".to_string()),
            Some(emitter),
            AgentCancellationToken::new(),
            Some(AgentRuntimeHostServices::new().with_context_compaction(services)),
        )
        .await
        .unwrap();
    server.await.unwrap();
    let request_body = String::from_utf8(request_bodies.lock().unwrap()[0].clone()).unwrap();

    assert_eq!(output.content, "done after recursive compaction");
    assert_eq!(prepare_count.load(Ordering::SeqCst), 1);
    assert_eq!(generate_count.load(Ordering::SeqCst), 1);
    assert_eq!(commit_count.load(Ordering::SeqCst), 1);
    assert!(request_body.contains("RECURSIVE_SUMMARY_MARKER"));
    assert!(!request_body.contains("GROWN_USER_MARKER"));
    assert!(!request_body.contains("GROWN_HISTORY_MARKER"));
    assert!(!request_body.contains("PREVIOUS_SUMMARY_MARKER"));
    let events = emitted_events.lock().unwrap();
    assert!(events
        .iter()
        .any(|event| matches!(event, AgentEvent::ContextCompactionStarted { .. })));
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::ContextCompactionFinished {
            outcome: AgentContextCompactionEventOutcome::Applied,
            ..
        }
    )));
}

#[tokio::test]
async fn context_capacity_guard_rejects_the_initial_request_before_network_io() {
    use tokio::net::TcpListener;
    use tokio::time::{timeout, Duration};

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let mut input = AgentChatInput {
        api_url: format!("http://{address}/v1/chat/completions"),
        api_token: "test-token".to_string(),
        provider_configuration_revision: None,
        provider_connection_revision: None,
        search_connection_revision: None,
        provider_profile_config: None,
        provider_protocol_key: None,
        model: "test-model".to_string(),
        model_capabilities: crate::ModelCapabilities::default(),
        api_style: Some(crate::protocol::AgentApiStyle::OpenAiCompatible),
        context_window_tokens: Some(8_000),
        context_window_indicator_enabled: false,
        max_tokens: Some(1_000),
        temperature: None,
        stream: Some(false),
        context: None,
        search_config: None,
        prompt_preferences: None,
        approval_decision: None,
        tool_continuation: None,
        attachments: Vec::new(),
        resume_checkpoint: None,
        assistant_message_id: None,
        context_compaction_summary: None,
        world_state_records: Vec::new(),
        skill_activation: None,
        skill_discovery: None,
        messages: vec![AgentChatMessage {
            message_id: None,
            role: "user".to_string(),
            content: "x".repeat(90_000),
            created_at: None,
            conversation_turn_trace: None,
            conversation_model_context_items: Vec::new(),
        }],
    };
    freeze_runtime_test_generic_provider(&mut input, "capacity-guard-initial");

    let error = AgentRuntime::default().send_chat(input).await.unwrap_err();

    assert_eq!(error.code(), Some("context_capacity_exceeded"));
    assert_eq!(
        error
            .details()
            .and_then(|details| details["status"].as_str()),
        Some("over_budget")
    );
    assert_eq!(
        error
            .details()
            .and_then(|details| details["usage"]["measurementMode"].as_str()),
        Some("incremental_cache")
    );
    assert!(timeout(Duration::from_millis(100), listener.accept())
        .await
        .is_err());
}
#[tokio::test]
async fn context_capacity_guard_accepts_budgeted_tool_results_for_the_next_request() {
    use crate::protocol::{
        AgentCommandPermission, AgentPatchPermission, AgentPermissions, AgentReadPermission,
        AgentWritePermission,
    };
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;
    use tempfile::tempdir;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    async fn read_http_request(stream: &mut TcpStream) {
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4_096];
        let mut expected_length = None;
        loop {
            let read = stream.read(&mut buffer).await.unwrap();
            if read == 0 {
                return;
            }
            request.extend_from_slice(&buffer[..read]);
            if expected_length.is_none() {
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
                    expected_length = Some(header_end + 4 + content_length);
                }
            }
            if expected_length.is_some_and(|length| request.len() >= length) {
                return;
            }
        }
    }

    async fn write_json_response(stream: &mut TcpStream, body: Value) {
        let body = serde_json::to_vec(&body).unwrap();
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(response.as_bytes()).await.unwrap();
        stream.write_all(&body).await.unwrap();
    }

    let fixture = tempdir().unwrap();
    let workspace = fixture.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let large_file = (0..2_000)
        .map(|_| "x".repeat(120))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(workspace.join("large.txt"), large_file).unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let second_request_seen = Arc::new(AtomicBool::new(false));
    let second_request_seen_by_server = second_request_seen.clone();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_http_request(&mut stream).await;
        write_json_response(
            &mut stream,
            json!({
                "choices": [{
                    "message": {
                        "role": "assistant",
                        "content": null,
                        "tool_calls": [{
                            "id": "read-large-file",
                            "type": "function",
                            "function": {
                                "name": "read_file",
                                "arguments": "{\"path\":\"large.txt\",\"maxLines\":2000}"
                            }
                        }]
                    },
                    "finish_reason": "tool_calls"
                }]
            }),
        )
        .await;

        let (mut stream, _) = tokio::time::timeout(Duration::from_secs(2), listener.accept())
            .await
            .expect("budgeted read_file result should permit a second model request")
            .unwrap();
        second_request_seen_by_server.store(true, Ordering::SeqCst);
        read_http_request(&mut stream).await;
        write_json_response(
            &mut stream,
            json!({
                "choices": [{
                    "message": { "role": "assistant", "content": "summarized" },
                    "finish_reason": "stop"
                }]
            }),
        )
        .await;
    });
    let mut input = AgentChatInput {
        api_url: format!("http://{address}/v1/chat/completions"),
        api_token: "test-token".to_string(),
        provider_configuration_revision: None,
        provider_connection_revision: None,
        search_connection_revision: None,
        provider_profile_config: None,
        provider_protocol_key: None,
        model: "test-model".to_string(),
        model_capabilities: crate::ModelCapabilities::default(),
        api_style: Some(crate::protocol::AgentApiStyle::OpenAiCompatible),
        context_window_tokens: Some(80_000),
        context_window_indicator_enabled: true,
        max_tokens: Some(1_000),
        temperature: None,
        stream: Some(false),
        context: Some(AgentRunContext {
            collaboration_identity: None,
            conversation_id: Some("conversation-capacity".to_string()),
            project_id: None,
            workspace: Some(AgentWorkspaceContext {
                project_id: None,
                display_name: Some("workspace".to_string()),
                root_path: Some(workspace.to_string_lossy().to_string()),
            }),
            attachment_library: None,
            permissions: AgentPermissions {
                read: AgentReadPermission::WorkspaceOnly,
                write: AgentWritePermission::WorkspaceOnly,
                command: AgentCommandPermission::RequireApproval,
                command_safety: Default::default(),
                patch: AgentPatchPermission::RequireApproval,
                builtin_execution: Default::default(),
            },
        }),
        search_config: None,
        prompt_preferences: None,
        approval_decision: None,
        tool_continuation: None,
        attachments: Vec::new(),
        resume_checkpoint: None,
        assistant_message_id: None,
        context_compaction_summary: None,
        world_state_records: Vec::new(),
        skill_activation: None,
        skill_discovery: None,
        messages: vec![message("user", "Read large.txt and summarize it")],
    };
    freeze_runtime_test_generic_provider(&mut input, "capacity-guard-tool-results");

    let output = AgentRuntime::default().send_chat(input).await.unwrap();
    server.await.unwrap();

    assert_eq!(output.content, "summarized");
    assert!(second_request_seen.load(Ordering::SeqCst));
}

#[tokio::test]
async fn active_run_keeps_earlier_exact_tool_exchanges_across_later_model_samples() {
    use tokio::net::TcpListener;

    const FIRST_EXCHANGE_MARKER: &str = "FIRST_TODO_EXACT_MARKER";

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let requests_for_server = Arc::clone(&requests);
    let server = tokio::spawn(async move {
        for request_index in 0..3 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = read_runtime_test_json_request(&mut stream).await;
            requests_for_server.lock().unwrap().push(request);
            let response = match request_index {
                0 => json!({
                    "choices": [{
                        "message": {
                            "role": "assistant",
                            "content": null,
                            "tool_calls": [{
                                "id": "provider-first-todo",
                                "type": "function",
                                "function": {
                                    "name": "todo_update",
                                    "arguments": serde_json::to_string(&json!({
                                        "items": [{
                                            "title": FIRST_EXCHANGE_MARKER,
                                            "status": "completed"
                                        }]
                                    }))
                                    .unwrap()
                                }
                            }]
                        },
                        "finish_reason": "tool_calls"
                    }]
                }),
                1 => json!({
                    "choices": [{
                        "message": {
                            "role": "assistant",
                            "content": "Checking one more source.",
                            "tool_calls": [{
                                "id": "provider-list-attachments",
                                "type": "function",
                                "function": {
                                    "name": "attachments_list",
                                    "arguments": "{}"
                                }
                            }]
                        },
                        "finish_reason": "tool_calls"
                    }]
                }),
                _ => json!({
                    "choices": [{
                        "message": {
                            "role": "assistant",
                            "content": "Both exchanges are still available."
                        },
                        "finish_reason": "stop"
                    }]
                }),
            };
            write_runtime_test_json_response(&mut stream, response).await;
        }
    });

    let mut input =
        conversation_context_input(vec![message("user", "Exercise two tools, then answer.")]);
    input.api_url = format!("http://{address}/v1/chat/completions");
    input.api_token = "test-token".to_string();
    input.stream = Some(false);
    input.assistant_message_id = Some("assistant-active-run-exact".to_string());

    let output = AgentRuntime::default().send_chat(input).await.unwrap();
    server.await.unwrap();

    assert_eq!(output.content, "Both exchanges are still available.");
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 3);
    let third_request = serde_json::to_string(&requests[2]).unwrap();
    assert!(
        third_request.contains(FIRST_EXCHANGE_MARKER),
        "a successful intermediate sample must not demote an earlier exact tool exchange"
    );
    assert!(third_request.contains("attachments_list"));
}

#[tokio::test]
async fn streams_apply_patch_previews_end_to_end_without_persisting_them() {
    use crate::protocol::{
        AgentCommandPermission, AgentPatchPermission, AgentPermissions, AgentReadPermission,
        AgentWritePermission,
    };
    use crate::storage::models::{AgentFileChangeRecord, ChatConversationRecord};
    use crate::storage::service::StorageService;
    use std::time::Duration;
    use tempfile::tempdir;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    async fn read_request(stream: &mut TcpStream) {
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4096];
        let mut expected_len = None;
        loop {
            let read = stream.read(&mut buffer).await.unwrap();
            if read == 0 {
                return;
            }
            request.extend_from_slice(&buffer[..read]);
            if expected_len.is_none() {
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
                        .unwrap_or(0);
                    expected_len = Some(header_end + 4 + content_length);
                }
            }
            if expected_len.is_some_and(|length| request.len() >= length) {
                return;
            }
        }
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

    fn tool_completion(id: &str, arguments: Value) -> Value {
        json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": id,
                        "type": "function",
                        "function": {
                            "name": "apply_patch",
                            "arguments": serde_json::to_string(&arguments).unwrap()
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        })
    }

    async fn write_append_stream(stream: &mut TcpStream, transaction_id: &str) {
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
            )
            .await
            .unwrap();
        let fragments = [
            format!(
                "{{\"request\":{{\"action\":\"append\",\"transactionId\":\"{transaction_id}\",\"index\":0,\"expectedDraftRevision\":0,\"content\":\"line 1\\n"
            ),
            "line 2\\n".to_string(),
            "line 3\\n".to_string(),
            "line 4\\n\"}}".to_string(),
        ];
        let tool_name_fragments = ["apply_", "patch", "", ""];
        for (fragment, tool_name) in fragments.into_iter().zip(tool_name_fragments) {
            let frame = json!({
                "choices": [{
                    "delta": {
                        "tool_calls": [{
                            "index": 0,
                            "id": "call-append",
                            "type": "function",
                            "function": {
                                "name": tool_name,
                                "arguments": fragment
                            }
                        }]
                    }
                }]
            });
            stream
                .write_all(format!("data: {frame}\n\n").as_bytes())
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
        stream
            .write_all(
                format!(
                    "data: {}\n\ndata: [DONE]\n\n",
                    json!({ "choices": [{ "delta": {}, "finish_reason": "tool_calls" }] })
                )
                .as_bytes(),
            )
            .await
            .unwrap();
    }

    let fixture = tempdir().unwrap();
    let workspace = fixture.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("app.db")).unwrap());
    storage
        .save_conversation(ChatConversationRecord {
            id: "conversation-preview".to_string(),
            project_id: None,
            model_id: None,
            title: "Preview".to_string(),
            messages: Vec::new(),
            created_at: 1,
            updated_at: 1,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
    storage
        .create_agent_file_change(AgentFileChangeRecord {
            schema_version: crate::file_change::FILE_CHANGE_SCHEMA_VERSION,
            id: "file-change-preview".to_string(),
            conversation_id: "conversation-preview".to_string(),
            project_id: None,
            run_id: "run-preview".to_string(),
            source_tool_name: "apply_patch".to_string(),
            source_tool_call_id: "call-begin".to_string(),
            source_tool_arguments_digest: crate::file_change::proposal_digest(&json!({
                "request": {
                    "action": "begin",
                    "operation": "create",
                    "filePath": "preview.md",
                    "observationId": "fobs-preview",
                }
            }))
            .unwrap(),
            permission_revision: "permission-v1".to_string(),
            tool_set_revision: "tool-set-v1".to_string(),
            provider_wire_revision: "provider-protocol-v1".to_string(),
            observation_id: "fobs-preview".to_string(),
            observation_json: "{}".to_string(),
            file_path: "preview.md".to_string(),
            operation: "create".to_string(),
            strategy: None,
            status: "drafting".to_string(),
            base_revision: None,
            base_content: String::new(),
            content: String::new(),
            draft_revision: 0,
            next_mutation_index: 0,
            additions: 0,
            deletions: 0,
            line_count: 0,
            byte_count: 0,
            mutation_count: 0,
            stats_final: false,
            summary: None,
            final_action_id: None,
            final_action_arguments_digest: None,
            final_permission_revision: None,
            final_tool_set_revision: None,
            final_provider_wire_revision: None,
            created_at: 1,
            updated_at: 1,
            expires_at: i64::MAX,
        })
        .unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        for request_index in 0..3 {
            let (mut stream, _) = listener.accept().await.unwrap();
            read_request(&mut stream).await;
            match request_index {
                0 => {
                    write_append_stream(&mut stream, "file-change-preview").await;
                }
                1 => {
                    write_json_response(
                        &mut stream,
                        tool_completion(
                            "call-abort",
                            json!({
                                "request": {
                                    "action": "abort",
                                    "transactionId": "file-change-preview"
                                }
                            }),
                        ),
                    )
                    .await;
                }
                _ => {
                    write_json_response(
                        &mut stream,
                        json!({
                            "choices": [{
                                "message": { "role": "assistant", "content": "done" },
                                "finish_reason": "stop"
                            }]
                        }),
                    )
                    .await;
                }
            }
        }
    });

    let captured = Arc::new(std::sync::Mutex::new(Vec::<AgentEvent>::new()));
    let captured_for_emitter = captured.clone();
    let emitter: AgentEventEmitter = Arc::new(move |event| {
        captured_for_emitter.lock().unwrap().push(event);
    });
    let mut input = AgentChatInput {
        api_url: format!("http://{address}/v1/chat/completions"),
        api_token: "test-token".to_string(),
        provider_configuration_revision: None,
        provider_connection_revision: None,
        search_connection_revision: None,
        provider_profile_config: None,
        provider_protocol_key: None,
        model: "test-model".to_string(),
        model_capabilities: crate::ModelCapabilities::default(),
        api_style: Some(crate::protocol::AgentApiStyle::OpenAiCompatible),
        context_window_tokens: Some(128_000),
        context_window_indicator_enabled: true,
        max_tokens: Some(1_000),
        temperature: None,
        stream: Some(true),
        context: Some(AgentRunContext {
            collaboration_identity: None,
            conversation_id: Some("conversation-preview".to_string()),
            project_id: None,
            workspace: Some(AgentWorkspaceContext {
                project_id: None,
                display_name: Some("workspace".to_string()),
                root_path: Some(workspace.to_string_lossy().to_string()),
            }),
            attachment_library: None,
            permissions: AgentPermissions {
                read: AgentReadPermission::WorkspaceOnly,
                write: AgentWritePermission::WorkspaceOnly,
                command: AgentCommandPermission::RequireApproval,
                command_safety: Default::default(),
                patch: AgentPatchPermission::RequireApproval,
                builtin_execution: Default::default(),
            },
        }),
        search_config: None,
        prompt_preferences: None,
        approval_decision: None,
        tool_continuation: None,
        attachments: Vec::new(),
        resume_checkpoint: None,
        assistant_message_id: Some("assistant-preview".to_string()),
        context_compaction_summary: None,
        world_state_records: Vec::new(),
        skill_activation: None,
        skill_discovery: None,
        messages: vec![message("user", "create a preview")],
    };
    freeze_runtime_test_generic_provider(&mut input, "apply-patch-preview-stream");
    let output = AgentRuntime::default()
        .send_chat_with_events_and_cancellation(
            input,
            Some("run-preview".to_string()),
            Some(emitter),
            AgentCancellationToken::new(),
            Some(AgentRuntimeHostServices::new().with_storage(storage.clone())),
        )
        .await
        .unwrap();
    server.await.unwrap();

    let previews = captured
        .lock()
        .unwrap()
        .iter()
        .filter_map(|event| match event {
            AgentEvent::FileChangePreviewUpdated { preview, .. } => Some(preview.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let context_snapshots = captured
        .lock()
        .unwrap()
        .iter()
        .filter_map(|event| match event {
            AgentEvent::ContextWindowUpdated { snapshot, .. } => Some(snapshot.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let preview_additions = previews
        .iter()
        .map(|preview| preview.additions)
        .collect::<Vec<_>>();
    let mut preview_content = String::new();
    for preview in &previews {
        assert_eq!(preview.content_offset_bytes, preview_content.len() as u64);
        preview_content.push_str(&preview.content_delta);
    }
    assert!(preview_additions.len() >= 3, "{preview_additions:?}");
    assert_eq!(preview_additions.last().copied(), Some(4));
    assert_eq!(preview_content, "line 1\nline 2\nline 3\nline 4\n");
    assert!(context_snapshots.is_empty());
    assert!(output.events.iter().all(|event| !matches!(
        event,
        AgentEvent::FileChangePreviewUpdated { .. } | AgentEvent::FileChangePreviewCleared { .. }
    )));
    assert_eq!(output.content, "done");
    let append_event = output
        .events
        .iter()
        .find_map(|event| match event {
            AgentEvent::ToolCall { call, .. }
                if call.tool == "apply_patch" && call.args["request"]["action"] == "append" =>
            {
                Some(call)
            }
            _ => None,
        })
        .expect("apply_patch append event");
    assert_runtime_owned_tool_call_id(&append_event.id);
    assert!(append_event.args["request"].get("content").is_none());
    assert_eq!(append_event.args["request"]["contentBytes"], 28);
    assert!(append_event.args["request"].get("contentDigest").is_some());
    let append_call_id = append_event.id.clone();

    let append_trace = output
        .conversation_turn_trace
        .as_ref()
        .expect("durable conversation trace")
        .items
        .iter()
        .find_map(|item| match item {
            ConversationTurnTraceItem::ToolCall {
                call_id, operation, ..
            } if call_id == &append_call_id => Some(operation),
            _ => None,
        })
        .expect("apply_patch append trace item");
    assert!(append_trace["request"].get("content").is_none());
    assert_eq!(append_trace["request"]["contentBytes"], 28);
    assert_eq!(
        storage
            .list_agent_file_changes_for_run("run-preview")
            .unwrap()[0]
            .status,
        "aborted"
    );
}
