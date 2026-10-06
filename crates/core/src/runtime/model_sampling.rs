use super::*;
use crate::protocol::AgentModelActivity;

pub(super) struct ModelSamplingContext<'a> {
    pub(super) run_id: &'a str,
    pub(super) model_request_index: usize,
    pub(super) user_text_blocked: bool,
    pub(super) event_stream: &'a mut AgentEventStream,
    pub(super) tool_registry: &'a ToolRegistry,
    pub(super) tool_context: &'a ToolExecutionContext,
}

pub(super) struct ModelSamplingResult {
    pub(super) response: AgentResult<crate::llm::LlmChatResponse>,
    pub(super) received_model_output: bool,
    pub(super) committed_message_stream_id: Option<String>,
}

// These markers span all transport attempts for one model request. In particular, a retry
// must not erase evidence that the model already produced output before a capacity error.
#[derive(Default)]
struct ModelStreamProgress {
    received_model_output: bool,
    committed_message_stream_id: Option<String>,
    tool_input_stream: ToolInputStreamObservers,
    model_activity: Option<AgentModelActivity>,
}

pub(super) async fn sample_model(
    request: LlmChatRequest,
    cancellation_token: AgentCancellationToken,
    mut context: ModelSamplingContext<'_>,
) -> ModelSamplingResult {
    let mut progress = ModelStreamProgress::default();
    let stream_id = format!(
        "{}-stream-{}",
        context.run_id,
        context.model_request_index + 1
    );
    let response = if request.stream {
        let delta_cancellation_token = cancellation_token.clone();
        complete_chat_streaming(request, cancellation_token.clone(), |stream_event| {
            progress.on_event(
                &mut context,
                &stream_id,
                &delta_cancellation_token,
                stream_event,
            );
        })
        .await
    } else {
        if !cancellation_token.is_cancelled() {
            progress.tool_input_stream.start_attempt(1);
            progress.set_activity(&mut context, &stream_id, AgentModelActivity::Waiting);
        }
        complete_chat(request, cancellation_token.clone()).await
    };
    // Cancellation bypasses the normal callback reset, and all callbacks are suppressed
    // once the token is cancelled. Always release the transient phase when sampling exits.
    progress.set_activity(&mut context, &stream_id, AgentModelActivity::Waiting);
    ModelSamplingResult {
        response,
        received_model_output: progress.received_model_output,
        committed_message_stream_id: progress.committed_message_stream_id,
    }
}

impl ModelStreamProgress {
    fn set_activity(
        &mut self,
        context: &mut ModelSamplingContext<'_>,
        stream_id: &str,
        activity: AgentModelActivity,
    ) {
        let attempt = self.tool_input_stream.attempt();
        if attempt == 0 || self.model_activity == Some(activity) {
            return;
        }
        self.model_activity = Some(activity);
        context
            .event_stream
            .emit_transient(AgentEvent::ModelActivityChanged {
                run_id: context.run_id.to_string(),
                stream_id: stream_id.to_string(),
                attempt,
                activity,
            });
    }

    fn on_event(
        &mut self,
        context: &mut ModelSamplingContext<'_>,
        stream_id: &str,
        cancellation_token: &AgentCancellationToken,
        stream_event: LlmStreamEvent,
    ) {
        if cancellation_token.is_cancelled() {
            return;
        }
        self.received_model_output |= matches!(&stream_event,
            LlmStreamEvent::Delta(delta) if !delta.is_empty())
            || matches!(&stream_event, LlmStreamEvent::ToolInputProgress { .. });
        // These boundaries also apply while visible narration is blocked by a file
        // transaction. Activity is independent of the user-facing text projection.
        if matches!(&stream_event, LlmStreamEvent::Delta(delta) if !delta.is_empty())
            || matches!(&stream_event, LlmStreamEvent::ToolInputProgress { .. })
        {
            self.set_activity(context, stream_id, AgentModelActivity::Waiting);
        }
        match stream_event {
            LlmStreamEvent::AttemptStarted {
                attempt,
                max_attempts,
            } => {
                self.tool_input_stream.start_attempt(attempt);
                self.model_activity = None;
                if !context.user_text_blocked {
                    let _ = max_attempts;
                    context.event_stream.emit(AgentEvent::MessageStreamStarted {
                        run_id: context.run_id.to_string(),
                        stream_id: stream_id.to_string(),
                        attempt,
                    });
                }
                self.set_activity(context, stream_id, AgentModelActivity::Waiting);
            }
            LlmStreamEvent::ModelActivityChanged(activity) => {
                self.set_activity(context, stream_id, activity);
            }
            LlmStreamEvent::Delta(delta) if !context.user_text_blocked && !delta.is_empty() => {
                context.event_stream.emit(AgentEvent::MessageDelta {
                    run_id: context.run_id.to_string(),
                    stream_id: Some(stream_id.to_string()),
                    delta,
                });
            }
            LlmStreamEvent::ToolInputProgress {
                tool_call_index,
                tool,
                input_delta,
                received_bytes,
            } => {
                // Stream fragments are provisional. Correlate them by
                // stream/attempt/index and leave Tool Call ID unset until
                // the complete response receives its canonical identity.
                let observation = self.tool_input_stream.on_delta(
                    context.tool_registry,
                    context.tool_context,
                    stream_id,
                    tool_call_index,
                    None,
                    &tool,
                    &input_delta,
                    received_bytes,
                );
                if observation.as_ref().is_ok_and(|value| !value.handled) {
                    context
                        .event_stream
                        .emit_transient(AgentEvent::ToolInputProgress {
                            run_id: context.run_id.to_string(),
                            stream_id: stream_id.to_string(),
                            attempt: self.tool_input_stream.attempt(),
                            tool_call_index,
                            tool_call_id: None,
                            tool: tool.clone(),
                            received_bytes,
                        });
                }
                if let Ok(observation) = observation {
                    if let Some(preview) = observation.preview {
                        emit_tool_input_preview(context.event_stream, context.run_id, preview);
                    }
                }
            }
            LlmStreamEvent::AttemptReset { reason } => {
                self.set_activity(context, stream_id, AgentModelActivity::Waiting);
                context
                    .event_stream
                    .emit_transient(AgentEvent::FileChangePreviewCleared {
                        run_id: context.run_id.to_string(),
                        stream_id: stream_id.to_string(),
                        attempt: self.tool_input_stream.attempt(),
                    });
                self.tool_input_stream.reset();
                if !context.user_text_blocked {
                    context.event_stream.emit(AgentEvent::MessageStreamReset {
                        run_id: context.run_id.to_string(),
                        stream_id: stream_id.to_string(),
                        reason,
                    });
                }
            }
            LlmStreamEvent::Retrying {
                attempt,
                max_attempts,
                category,
                provider_code,
                delay_ms,
                retry_at,
            } => {
                self.set_activity(context, stream_id, AgentModelActivity::Waiting);
                context.event_stream.emit(AgentEvent::LlmRetry {
                    run_id: context.run_id.to_string(),
                    stream_id: stream_id.to_string(),
                    attempt,
                    max_attempts,
                    category,
                    provider_code,
                    delay_ms,
                    retry_at,
                });
            }
            LlmStreamEvent::Committed => {
                self.set_activity(context, stream_id, AgentModelActivity::Waiting);
                for preview in self.tool_input_stream.flush() {
                    emit_tool_input_preview(context.event_stream, context.run_id, preview);
                }
                if !context.user_text_blocked {
                    self.committed_message_stream_id = Some(stream_id.to_string());
                }
            }
            LlmStreamEvent::Delta(_) => {}
        }
    }
}

#[cfg(test)]
#[path = "model_sampling_activity_tests.rs"]
mod activity_tests;

#[cfg(test)]
mod tests {
    use super::*;

    fn retry() -> LlmStreamEvent {
        LlmStreamEvent::Retrying {
            attempt: 2,
            max_attempts: 3,
            category: "transport".to_string(),
            provider_code: Some("busy".to_string()),
            delay_ms: 100,
            retry_at: 200,
        }
    }

    fn tool_progress() -> LlmStreamEvent {
        LlmStreamEvent::ToolInputProgress {
            tool_call_index: 4,
            tool: "unknown-tool".to_string(),
            input_delta: "{".to_string(),
            received_bytes: 1,
        }
    }

    #[test]
    fn retry_preserves_output_evidence_and_provisional_event_order() {
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
            model_request_index: 2,
            user_text_blocked: false,
            event_stream: &mut events,
            tool_registry: &registry,
            tool_context: &tools,
        };
        for event in [
            LlmStreamEvent::AttemptStarted {
                attempt: 1,
                max_attempts: 3,
            },
            LlmStreamEvent::Delta(String::new()),
        ] {
            progress.on_event(&mut context, "run-stream-3", &cancellation, event);
        }
        assert!(!progress.received_model_output);
        for event in [
            tool_progress(),
            LlmStreamEvent::AttemptReset {
                reason: "connection lost".to_string(),
            },
            retry(),
            LlmStreamEvent::AttemptStarted {
                attempt: 2,
                max_attempts: 3,
            },
        ] {
            progress.on_event(&mut context, "run-stream-3", &cancellation, event);
        }
        assert!(progress.received_model_output);
        assert!(progress.committed_message_stream_id.is_none());
        progress.on_event(&mut context, "run-stream-3", &cancellation, tool_progress());
        progress.on_event(
            &mut context,
            "run-stream-3",
            &cancellation,
            LlmStreamEvent::Committed,
        );
        assert_eq!(
            progress.committed_message_stream_id.as_deref(),
            Some("run-stream-3")
        );

        let captured = captured.lock().unwrap();
        let live = captured
            .iter()
            .filter(|event| !matches!(event, AgentEvent::ModelActivityChanged { .. }))
            .collect::<Vec<_>>();
        assert_eq!(live.len(), 7);
        assert!(matches!(
            &live[0],
            AgentEvent::MessageStreamStarted { attempt: 1, .. }
        ));
        assert!(matches!(
            &live[1],
            AgentEvent::ToolInputProgress {
                attempt: 1,
                tool_call_index: 4,
                tool_call_id: None,
                received_bytes: 1,
                ..
            }
        ));
        assert!(matches!(
            &live[2],
            AgentEvent::FileChangePreviewCleared { attempt: 1, .. }
        ));
        assert!(
            matches!(&live[3], AgentEvent::MessageStreamReset { reason, .. } if reason == "connection lost")
        );
        assert!(matches!(&live[4], AgentEvent::LlmRetry {
            attempt: 2, max_attempts: 3, delay_ms: 100, retry_at: 200, provider_code: Some(code), ..
        } if code == "busy"));
        assert!(matches!(
            &live[5],
            AgentEvent::MessageStreamStarted { attempt: 2, .. }
        ));
        assert!(matches!(
            &live[6],
            AgentEvent::ToolInputProgress {
                attempt: 2,
                tool_call_id: None,
                ..
            }
        ));
        let retained = events.into_events();
        assert_eq!(retained.len(), 4);
        assert!(retained.iter().all(|event| !matches!(
            event,
            AgentEvent::ToolInputProgress { .. }
                | AgentEvent::FileChangePreviewCleared { .. }
                | AgentEvent::ModelActivityChanged { .. }
        )));
    }

    #[test]
    fn file_transaction_fence_suppresses_text_but_keeps_progress_and_retry() {
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
        for event in [
            LlmStreamEvent::AttemptStarted {
                attempt: 1,
                max_attempts: 3,
            },
            LlmStreamEvent::Delta("uncommitted file write narration".to_string()),
        ] {
            progress.on_event(&mut context, "run-stream-1", &cancellation, event);
        }
        assert!(progress.received_model_output);
        assert!(matches!(
            captured.lock().unwrap().as_slice(),
            [AgentEvent::ModelActivityChanged {
                activity: AgentModelActivity::Waiting,
                ..
            }]
        ));
        for event in [
            tool_progress(),
            LlmStreamEvent::AttemptReset {
                reason: "retry".to_string(),
            },
            retry(),
            LlmStreamEvent::Committed,
        ] {
            progress.on_event(&mut context, "run-stream-1", &cancellation, event);
        }
        assert!(progress.committed_message_stream_id.is_none());
        let captured = captured.lock().unwrap();
        let live = captured
            .iter()
            .filter(|event| !matches!(event, AgentEvent::ModelActivityChanged { .. }))
            .collect::<Vec<_>>();
        assert_eq!(live.len(), 3);
        assert!(matches!(&live[0], AgentEvent::ToolInputProgress { .. }));
        assert!(matches!(
            &live[1],
            AgentEvent::FileChangePreviewCleared { attempt: 1, .. }
        ));
        assert!(matches!(&live[2], AgentEvent::LlmRetry { .. }));
        let retained = events.into_events();
        assert_eq!(retained.len(), 1);
        assert!(matches!(&retained[0], AgentEvent::LlmRetry { .. }));
    }

    #[test]
    fn cancelled_callback_cannot_publish_or_change_stream_state() {
        let mut events = AgentEventStream::new(None);
        let registry = ToolRegistry::defaults_with_search(None);
        let tools = ToolExecutionContext::from_run_context(None);
        let cancellation = AgentCancellationToken::new();
        cancellation.cancel();
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
            LlmStreamEvent::AttemptStarted {
                attempt: 1,
                max_attempts: 3,
            },
            LlmStreamEvent::Delta("late delta".to_string()),
            LlmStreamEvent::ModelActivityChanged(AgentModelActivity::Reasoning),
            tool_progress(),
            LlmStreamEvent::AttemptReset {
                reason: "late retry".to_string(),
            },
            retry(),
            LlmStreamEvent::Committed,
        ] {
            progress.on_event(&mut context, "run-stream-1", &cancellation, event);
        }
        assert!(!progress.received_model_output);
        assert!(progress.committed_message_stream_id.is_none());
        assert_eq!(progress.tool_input_stream.attempt(), 0);
        assert!(events.into_events().is_empty());
    }
}
