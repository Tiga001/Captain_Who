use super::*;
use mycopilot_protocol_rs::{
    AgentObserverLiveStreamDto, AgentObserverLiveStreamSnapshotDto, AgentObserverModelActivityDto,
    AgentObserverModelActivityKindDto, AgentObserverStreamCursorDto,
};
#[cfg(test)]
use serde_json::json;

#[cfg(test)]
#[path = "child_event_transport_tests.rs"]
mod child_event_transport_tests;

/// Retains provisional text and content-free activity. Committed narration lives in the trace;
/// this process-local state is released after terminal persistence. No per-token database writes.
pub(super) struct ObserverStreamState {
    pub agent_id: String,
    pub root_agent_id: String,
    pub root_conversation_id: String,
    pub snapshot: AgentObserverLiveStreamSnapshotDto,
    pub model_activity_attempts: HashMap<String, usize>,
}

impl ObserverStreamState {
    fn new(identity: &AgentCollaborationIdentity, run_id: &str, message_id: &str) -> Self {
        Self {
            agent_id: identity.agent_id.clone(),
            root_agent_id: identity.root_agent_id.clone(),
            root_conversation_id: identity.root_conversation_id.clone(),
            snapshot: AgentObserverLiveStreamSnapshotDto {
                run_id: run_id.to_string(),
                assistant_message_id: message_id.to_string(),
                cursor: AgentObserverStreamCursorDto {
                    generation: uuid::Uuid::new_v4().to_string(),
                    sequence: 0,
                },
                stream: None,
                model_activity: None,
                final_answer_ready: false,
            },
            model_activity_attempts: HashMap::new(),
        }
    }

    fn apply(&mut self, event: &AgentEvent, boundary: u64) {
        self.snapshot.cursor.sequence += 1;
        match event {
            AgentEvent::FinalAnswerReady { .. } => {
                self.snapshot.final_answer_ready = true;
                self.snapshot.model_activity = None;
            }
            AgentEvent::MessageStreamStarted {
                stream_id, attempt, ..
            } => {
                if !self
                    .snapshot
                    .model_activity
                    .as_ref()
                    .is_some_and(|activity| {
                        activity.stream_id == *stream_id && activity.attempt == *attempt
                    })
                {
                    self.set_model_activity(
                        stream_id,
                        *attempt,
                        AgentObserverModelActivityKindDto::Waiting,
                    );
                }
                self.snapshot.stream = Some(AgentObserverLiveStreamDto {
                    stream_id: stream_id.clone(),
                    attempt: *attempt,
                    content: String::new(),
                    trace_boundary_sequence: boundary,
                    committed: false,
                });
            }
            AgentEvent::MessageDelta {
                stream_id, delta, ..
            } => {
                if !delta.is_empty() {
                    if let Some(activity) =
                        self.snapshot.model_activity.as_mut().filter(|activity| {
                            stream_id
                                .as_ref()
                                .is_none_or(|id| id == &activity.stream_id)
                        })
                    {
                        activity.activity = AgentObserverModelActivityKindDto::Waiting;
                    }
                }
                let id = stream_id
                    .clone()
                    .unwrap_or_else(|| format!("observer-{}", self.snapshot.run_id));
                let stream =
                    self.snapshot
                        .stream
                        .get_or_insert_with(|| AgentObserverLiveStreamDto {
                            stream_id: id,
                            attempt: 1,
                            content: String::new(),
                            trace_boundary_sequence: boundary,
                            committed: false,
                        });
                if stream_id.as_ref().is_none_or(|id| id == &stream.stream_id) {
                    stream.content.push_str(delta);
                }
            }
            AgentEvent::MessageStreamReset { stream_id, .. } => {
                self.snapshot.final_answer_ready = false;
                self.clear_model_activity_for_stream(stream_id);
                if self
                    .snapshot
                    .stream
                    .as_ref()
                    .is_some_and(|stream| stream.stream_id == *stream_id)
                {
                    self.snapshot.stream = None;
                }
            }
            AgentEvent::MessageStreamCommitted {
                stream_id,
                trace_sequence,
                ..
            } => {
                self.clear_model_activity_for_stream(stream_id);
                if let Some(stream) = self
                    .snapshot
                    .stream
                    .as_mut()
                    .filter(|stream| stream.stream_id == *stream_id)
                {
                    if trace_sequence.is_some() {
                        self.snapshot.stream = None;
                    } else {
                        stream.committed = true;
                    }
                }
            }
            AgentEvent::Message { content, .. } => {
                if let Some(activity) = self.snapshot.model_activity.as_mut() {
                    activity.activity = AgentObserverModelActivityKindDto::Waiting;
                }
                self.snapshot.stream = Some(AgentObserverLiveStreamDto {
                    stream_id: format!("observer-{}", self.snapshot.run_id),
                    attempt: 1,
                    content: content.clone(),
                    trace_boundary_sequence: boundary,
                    committed: true,
                });
            }
            AgentEvent::ModelActivityChanged {
                stream_id,
                attempt,
                activity,
                ..
            } => {
                let activity = match activity {
                    mycopilot_core::AgentModelActivity::Reasoning => {
                        AgentObserverModelActivityKindDto::Reasoning
                    }
                    mycopilot_core::AgentModelActivity::Waiting => {
                        AgentObserverModelActivityKindDto::Waiting
                    }
                };
                self.set_model_activity(stream_id, *attempt, activity);
            }
            AgentEvent::ToolInputProgress {
                stream_id, attempt, ..
            } => {
                self.snapshot.final_answer_ready = false;
                if let Some(activity) = self.snapshot.model_activity.as_mut().filter(|activity| {
                    activity.stream_id == *stream_id && activity.attempt == *attempt
                }) {
                    activity.activity = AgentObserverModelActivityKindDto::Waiting;
                }
            }
            AgentEvent::FileChangePreviewUpdated { preview, .. } => {
                self.snapshot.final_answer_ready = false;
                if let Some(activity) = self.snapshot.model_activity.as_mut().filter(|activity| {
                    activity.stream_id == preview.stream_id && activity.attempt == preview.attempt
                }) {
                    activity.activity = AgentObserverModelActivityKindDto::Waiting;
                }
            }
            AgentEvent::ToolCall { .. } | AgentEvent::McpToolInvocationStateChanged { .. } => {
                self.snapshot.final_answer_ready = false;
                if let Some(activity) = self.snapshot.model_activity.as_mut() {
                    activity.activity = AgentObserverModelActivityKindDto::Waiting;
                }
            }
            AgentEvent::LlmRetry {
                stream_id, attempt, ..
            } => {
                let stale = self
                    .snapshot
                    .model_activity
                    .as_ref()
                    .is_some_and(|activity| activity.stream_id != *stream_id)
                    || *attempt
                        <= self
                            .model_activity_attempts
                            .get(stream_id)
                            .copied()
                            .unwrap_or(0);
                if !stale {
                    self.snapshot.model_activity = None;
                    self.snapshot.final_answer_ready = false;
                }
            }
            AgentEvent::ContextCompactionStarted { .. } => {
                self.snapshot.final_answer_ready = false;
            }
            AgentEvent::Started { .. }
            | AgentEvent::ApprovalRequired { .. }
            | AgentEvent::Error { .. }
            | AgentEvent::Done { .. } => {
                self.snapshot.model_activity = None;
                self.snapshot.final_answer_ready = false;
            }
            AgentEvent::State { state, .. } if state.status != AgentRunStatus::Running => {
                self.snapshot.model_activity = None;
                self.snapshot.final_answer_ready = false;
            }
            _ => {}
        }
    }

    fn set_model_activity(
        &mut self,
        stream_id: &str,
        attempt: usize,
        activity: AgentObserverModelActivityKindDto,
    ) {
        let matches_active = self
            .snapshot
            .model_activity
            .as_ref()
            .is_some_and(|current| current.stream_id == stream_id && current.attempt == attempt);
        let starts_attempt = activity == AgentObserverModelActivityKindDto::Waiting
            && attempt
                > self
                    .model_activity_attempts
                    .get(stream_id)
                    .copied()
                    .unwrap_or(0);
        if matches_active || starts_attempt {
            self.snapshot.final_answer_ready = false;
            self.model_activity_attempts
                .insert(stream_id.to_string(), attempt);
            self.snapshot.model_activity = Some(AgentObserverModelActivityDto {
                stream_id: stream_id.to_string(),
                attempt,
                activity,
            });
        }
    }

    fn clear_model_activity_for_stream(&mut self, stream_id: &str) {
        if self
            .snapshot
            .model_activity
            .as_ref()
            .is_some_and(|activity| activity.stream_id == stream_id)
        {
            self.snapshot.model_activity = None;
        }
    }
}

impl AgentService {
    pub(crate) fn emit_child_observer_event(
        &self,
        notifications: &CoreServerNotificationSender,
        identity: &AgentCollaborationIdentity,
        run_id: &str,
        assistant_message_id: &str,
        event: &AgentEvent,
        safe_event: RendererSafeAgentEvent,
    ) {
        // Publishing and snapshot reads share this lock. A snapshot contains the complete text
        // through its cursor, and every later notification has a strictly greater sequence.
        let mut streams = self
            .observer_streams
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let state = streams
            .entry(identity.conversation_id.clone())
            .or_insert_with(|| ObserverStreamState::new(identity, run_id, assistant_message_id));
        if state.snapshot.run_id != run_id
            || state.snapshot.assistant_message_id != assistant_message_id
        {
            *state = ObserverStreamState::new(identity, run_id, assistant_message_id);
        }
        let starts_stream = matches!(
            event,
            AgentEvent::MessageStreamStarted { .. } | AgentEvent::Message { .. }
        ) || (state.snapshot.stream.is_none()
            && matches!(event, AgentEvent::MessageDelta { .. }));
        let boundary = if starts_stream {
            self.storage
                .get_conversation_turn_trace_next_sequence(assistant_message_id)
                .ok()
                .flatten()
                .unwrap_or(0)
        } else {
            0
        };
        state.apply(event, boundary);
        let terminal = matches!(
            event,
            AgentEvent::Done {
                status: None
                    | Some(
                        AgentRunStatus::Completed
                            | AgentRunStatus::Failed
                            | AgentRunStatus::Cancelled
                    ),
                ..
            }
        );
        let notification = child_event_notification_from_safe(
            identity,
            run_id,
            assistant_message_id,
            &state.snapshot.cursor,
            safe_event,
        );
        let _ = notifications.send(notification);
        if terminal {
            streams.remove(&identity.conversation_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> AgentCollaborationIdentity {
        AgentCollaborationIdentity {
            agent_id: "child".into(),
            root_agent_id: "root".into(),
            root_conversation_id: "root-conversation".into(),
            parent_agent_id: "root".into(),
            parent_task_name: "root".into(),
            parent_task_path: "/root".into(),
            conversation_id: "child-conversation".into(),
            task_name: "child".into(),
            task_path: "/root/child".into(),
            source_agent_id: "root".into(),
            source_kind: mycopilot_core::AgentMailboxKind::Task,
            source_task_name: "root".into(),
            source_task_path: "/root".into(),
            source_agent_message_id: "task".into(),
            entrusted_task: "test".into(),
            template_instructions: None,
        }
    }

    fn started(attempt: usize) -> AgentEvent {
        AgentEvent::MessageStreamStarted {
            run_id: "run".into(),
            stream_id: "stream".into(),
            attempt,
        }
    }

    fn delta(content: &str) -> AgentEvent {
        AgentEvent::MessageDelta {
            run_id: "run".into(),
            stream_id: Some("stream".into()),
            delta: content.into(),
        }
    }

    fn activity(attempt: usize, activity: mycopilot_core::AgentModelActivity) -> AgentEvent {
        AgentEvent::ModelActivityChanged {
            run_id: "run".into(),
            stream_id: "stream".into(),
            attempt,
            activity,
        }
    }

    #[test]
    fn observer_snapshot_retains_content_free_activity_without_public_text() {
        use mycopilot_core::AgentModelActivity::{Reasoning, Waiting};

        let mut state = ObserverStreamState::new(&identity(), "run", "assistant");
        state.apply(&activity(1, Waiting), 0);
        state.apply(&activity(1, Reasoning), 0);
        assert!(state.snapshot.stream.is_none());
        assert_eq!(state.snapshot.cursor.sequence, 2);
        assert_eq!(
            serde_json::to_value(&state.snapshot).unwrap()["modelActivity"],
            json!({"streamId": "stream", "attempt": 1, "activity": "reasoning"})
        );
        state.apply(&started(1), 0);
        assert_eq!(
            state.snapshot.model_activity.as_ref().unwrap().activity,
            AgentObserverModelActivityKindDto::Reasoning
        );
        state.apply(&delta(""), 0);
        assert_eq!(
            state.snapshot.model_activity.as_ref().unwrap().activity,
            AgentObserverModelActivityKindDto::Reasoning
        );
        state.apply(&delta("visible text"), 0);
        assert_eq!(
            state.snapshot.model_activity.as_ref().unwrap().activity,
            AgentObserverModelActivityKindDto::Waiting
        );
        state.apply(&activity(1, Reasoning), 0);
        state.apply(
            &AgentEvent::ToolInputProgress {
                run_id: "run".into(),
                stream_id: "stream".into(),
                attempt: 1,
                tool_call_index: 0,
                tool_call_id: None,
                tool: "read_file".into(),
                received_bytes: 1,
            },
            0,
        );
        assert_eq!(
            state.snapshot.model_activity.as_ref().unwrap().activity,
            AgentObserverModelActivityKindDto::Waiting
        );
    }

    #[test]
    fn observer_activity_cannot_return_from_a_retired_attempt() {
        use mycopilot_core::AgentModelActivity::{Reasoning, Waiting};

        let mut state = ObserverStreamState::new(&identity(), "run", "assistant");
        state.apply(&activity(1, Waiting), 0);
        state.apply(&activity(1, Reasoning), 0);
        state.apply(
            &AgentEvent::MessageStreamReset {
                run_id: "run".into(),
                stream_id: "stream".into(),
                reason: "retrying_model_request".into(),
            },
            0,
        );
        state.apply(&activity(1, Waiting), 0);
        state.apply(&activity(1, Reasoning), 0);
        assert!(state.snapshot.model_activity.is_none());
        state.apply(&activity(2, Waiting), 0);
        state.apply(&activity(2, Reasoning), 0);
        state.apply(&activity(1, Waiting), 0);
        assert_eq!(state.snapshot.model_activity.as_ref().unwrap().attempt, 2);
        assert_eq!(
            state.snapshot.model_activity.as_ref().unwrap().activity,
            AgentObserverModelActivityKindDto::Reasoning
        );
        state.apply(
            &AgentEvent::MessageStreamCommitted {
                run_id: "run".into(),
                stream_id: "stream".into(),
                trace_sequence: None,
            },
            0,
        );
        assert!(state.snapshot.model_activity.is_none());
        assert!(serde_json::to_value(&state.snapshot)
            .unwrap()
            .get("modelActivity")
            .is_none());
    }

    #[test]
    fn observer_activity_is_cleared_at_retry_error_and_done_boundaries() {
        use mycopilot_core::AgentModelActivity::{Reasoning, Waiting};

        let boundaries = [
            AgentEvent::LlmRetry {
                run_id: "run".into(),
                stream_id: "stream".into(),
                attempt: 2,
                max_attempts: 3,
                category: "network".into(),
                provider_code: None,
                delay_ms: 1,
                retry_at: 1,
            },
            AgentEvent::Error {
                run_id: Some("run".into()),
                trace_sequence: None,
                message: "request failed".into(),
                recoverable: true,
                code: None,
                details: None,
            },
            AgentEvent::Done {
                run_id: "run".into(),
                user_interrupted: Some(true),
                success: false,
                status: Some(AgentRunStatus::Cancelled),
                content: None,
                usage: None,
                finish_reason: None,
                proposed_actions: vec![],
            },
        ];
        for event in boundaries {
            let mut state = ObserverStreamState::new(&identity(), "run", "assistant");
            state.apply(&activity(1, Waiting), 0);
            state.apply(&activity(1, Reasoning), 0);
            state.apply(&event, 0);
            assert!(state.snapshot.model_activity.is_none());
        }
    }

    #[test]
    fn observer_activity_survives_retry_notifications_from_an_older_attempt_or_stream() {
        use mycopilot_core::AgentModelActivity::{Reasoning, Waiting};

        let mut state = ObserverStreamState::new(&identity(), "run", "assistant");
        state.apply(&activity(2, Waiting), 0);
        state.apply(&activity(2, Reasoning), 0);
        for (stream_id, attempt) in [("stream", 1), ("stream", 2), ("old-stream", 3)] {
            state.apply(
                &AgentEvent::LlmRetry {
                    run_id: "run".into(),
                    stream_id: stream_id.into(),
                    attempt,
                    max_attempts: 3,
                    category: "network".into(),
                    provider_code: None,
                    delay_ms: 1,
                    retry_at: 1,
                },
                0,
            );
            assert_eq!(
                state.snapshot.model_activity.as_ref().unwrap().activity,
                AgentObserverModelActivityKindDto::Reasoning
            );
            assert_eq!(state.snapshot.model_activity.as_ref().unwrap().attempt, 2);
        }
    }

    #[test]
    fn observer_final_answer_marker_requires_runtime_confirmation_and_survives_persistence_wait() {
        let mut state = ObserverStreamState::new(&identity(), "run", "assistant");
        state.apply(&started(1), 0);
        state.apply(&delta("answer"), 0);
        state.apply(
            &AgentEvent::MessageStreamCommitted {
                run_id: "run".into(),
                stream_id: "stream".into(),
                trace_sequence: None,
            },
            0,
        );
        assert!(!state.snapshot.final_answer_ready);
        assert!(serde_json::to_value(&state.snapshot)
            .unwrap()
            .get("finalAnswerReady")
            .is_none());
        state.apply(
            &AgentEvent::FinalAnswerReady {
                run_id: "run".into(),
            },
            0,
        );
        assert!(state.snapshot.final_answer_ready);
        assert!(state.snapshot.model_activity.is_none());
        assert_eq!(
            serde_json::to_value(&state.snapshot).unwrap()["finalAnswerReady"],
            true
        );
        state.apply(
            &AgentEvent::State {
                run_id: "run".into(),
                state: mycopilot_core::AgentStateSnapshot {
                    status: AgentRunStatus::Running,
                    active_run_id: Some("run".into()),
                    last_error: None,
                    updated_at: 1,
                },
            },
            0,
        );
        assert!(state.snapshot.final_answer_ready);
        assert_eq!(state.snapshot.stream.as_ref().unwrap().content, "answer");
    }

    #[test]
    fn observer_final_answer_marker_without_text_clears_at_new_work_and_settlement_boundaries() {
        let boundaries = [
            activity(2, mycopilot_core::AgentModelActivity::Waiting),
            AgentEvent::MessageStreamReset {
                run_id: "run".into(),
                stream_id: "stream".into(),
                reason: "retrying_model_request".into(),
            },
            AgentEvent::LlmRetry {
                run_id: "run".into(),
                stream_id: "stream".into(),
                attempt: 2,
                max_attempts: 3,
                category: "network".into(),
                provider_code: None,
                delay_ms: 1,
                retry_at: 1,
            },
            AgentEvent::Error {
                run_id: Some("run".into()),
                trace_sequence: None,
                message: "failed".into(),
                recoverable: true,
                code: None,
                details: None,
            },
            AgentEvent::State {
                run_id: "run".into(),
                state: mycopilot_core::AgentStateSnapshot {
                    status: AgentRunStatus::WaitingForApproval,
                    active_run_id: Some("run".into()),
                    last_error: None,
                    updated_at: 1,
                },
            },
            AgentEvent::Done {
                run_id: "run".into(),
                user_interrupted: None,
                success: true,
                status: Some(AgentRunStatus::Completed),
                content: None,
                usage: None,
                finish_reason: None,
                proposed_actions: vec![],
            },
        ];
        for boundary in boundaries {
            let mut state = ObserverStreamState::new(&identity(), "run", "assistant");
            state.apply(&activity(1, mycopilot_core::AgentModelActivity::Waiting), 0);
            state.apply(
                &AgentEvent::FinalAnswerReady {
                    run_id: "run".into(),
                },
                0,
            );
            assert!(state.snapshot.stream.is_none());
            assert!(state.snapshot.final_answer_ready);
            state.apply(&boundary, 0);
            assert!(!state.snapshot.final_answer_ready);
        }
    }

    #[test]
    fn observer_stream_accumulates_complete_text_and_resets_failed_attempts() {
        let mut state = ObserverStreamState::new(&identity(), "run", "assistant");
        state.apply(&started(1), 7);
        state.apply(&delta("完整前缀"), 0);
        state.apply(&delta(" + suffix"), 0);
        let snapshot = state.snapshot.clone();
        assert_eq!(snapshot.cursor.sequence, 3);
        assert_eq!(snapshot.stream.unwrap().content, "完整前缀 + suffix");
        state.apply(
            &AgentEvent::MessageStreamReset {
                run_id: "run".into(),
                stream_id: "stream".into(),
                reason: "retrying_model_request".into(),
            },
            0,
        );
        assert!(state.snapshot.stream.is_none());
        state.apply(&started(2), 7);
        state.apply(&delta("replacement"), 0);
        let stream = state.snapshot.stream.unwrap();
        assert_eq!(stream.content, "replacement");
        assert_eq!(stream.attempt, 2);
        assert_eq!(stream.trace_boundary_sequence, 7);
    }

    #[test]
    fn observer_stream_retires_durable_narration_but_keeps_final_text_until_done() {
        let mut state = ObserverStreamState::new(&identity(), "run", "assistant");
        state.apply(&started(1), 4);
        state.apply(&delta("narration"), 0);
        state.apply(
            &AgentEvent::MessageStreamCommitted {
                run_id: "run".into(),
                stream_id: "stream".into(),
                trace_sequence: Some(4),
            },
            0,
        );
        assert!(state.snapshot.stream.is_none());
        state.apply(&started(1), 8);
        state.apply(&delta("final"), 0);
        state.apply(
            &AgentEvent::MessageStreamCommitted {
                run_id: "run".into(),
                stream_id: "stream".into(),
                trace_sequence: None,
            },
            0,
        );
        assert!(state.snapshot.stream.as_ref().unwrap().committed);
        assert_eq!(state.snapshot.stream.unwrap().content, "final");
    }

    #[test]
    fn single_child_notification_preserves_privacy_identity_and_snapshot_cursor() {
        let directory = tempfile::tempdir().unwrap();
        let storage =
            Arc::new(StorageService::open(&directory.path().join("test.sqlite")).unwrap());
        let service = AgentService::try_new(storage).unwrap();
        let (sender, mut receiver) = crate::transport::outbound_channel();
        let identity = identity();
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../../packages/protocol/fixtures/agent-mcp-renderer-contract-v1.json"
        ))
        .unwrap();
        let mut action = fixture["approvalRequired"]["action"].clone();
        action["approval"]["identity"]["argumentsDigest"] = json!("host-only-arguments-digest");
        let events = [
            started(1),
            activity(1, mycopilot_core::AgentModelActivity::Waiting),
            activity(1, mycopilot_core::AgentModelActivity::Reasoning),
            delta("你好 🌍\n```rs\n"),
            delta(""),
            delta("```"),
            AgentEvent::FinalAnswerReady {
                run_id: "run".into(),
            },
            AgentEvent::Done {
                run_id: "run".into(),
                user_interrupted: None,
                success: false,
                status: Some(AgentRunStatus::WaitingForApproval),
                content: None,
                usage: None,
                finish_reason: None,
                proposed_actions: vec![serde_json::from_value(action).unwrap()],
            },
        ];
        let mut generation = None;
        for (index, event) in events.into_iter().enumerate() {
            let expected_event = agent_event_notification(event.clone())["params"].clone();
            emit_agent_event_notifications(
                &service,
                &sender,
                Some(&identity),
                "run",
                "assistant",
                event,
            );
            let observer = receiver.try_recv().unwrap();
            assert_eq!(
                observer["method"],
                mycopilot_protocol_rs::AGENT_COLLABORATION_CHILD_EVENT_NOTIFICATION_METHOD
            );
            assert_eq!(observer["params"]["event"], expected_event);
            assert!(
                receiver.try_recv().is_err(),
                "a child event must cross the transport only once"
            );
            assert_eq!(observer["params"]["agentId"], identity.agent_id);
            assert_eq!(
                observer["params"]["conversationId"],
                identity.conversation_id
            );
            assert_eq!(observer["params"]["rootAgentId"], identity.root_agent_id);
            assert_eq!(
                observer["params"]["rootConversationId"],
                identity.root_conversation_id
            );
            assert_eq!(observer["params"]["runId"], "run");
            assert_eq!(observer["params"]["assistantMessageId"], "assistant");
            assert_eq!(observer["params"]["streamCursor"]["sequence"], index + 1);
            let cursor = &observer["params"]["streamCursor"]["generation"];
            if let Some(previous) = generation.as_ref() {
                assert_eq!(cursor, previous);
            }
            generation = Some(cursor.clone());
            let serialized = observer.to_string();
            assert!(!serialized.contains("argumentsDigest"));
            assert!(!serialized.contains("host-only-arguments-digest"));
        }
        let snapshots = service.observer_streams.lock().unwrap();
        let snapshot = &snapshots[&identity.conversation_id].snapshot;
        assert_eq!(snapshot.cursor.sequence, 8);
        assert!(snapshot.model_activity.is_none());
        assert!(!snapshot.final_answer_ready);
        assert_eq!(
            snapshot.stream.as_ref().unwrap().content,
            "你好 🌍\n```rs\n```"
        );
        drop(snapshots);
        emit_agent_event_notifications(
            &service,
            &sender,
            None,
            "root-run",
            "root-assistant",
            delta("root only"),
        );
        assert_eq!(receiver.try_recv().unwrap()["params"]["delta"], "root only");
        assert!(receiver.try_recv().is_err());
        assert_eq!(service.observer_streams.lock().unwrap().len(), 1);
    }

    #[test]
    fn observer_stream_publication_cursors_are_monotonic_and_terminal_cache_is_released() {
        let directory = tempfile::tempdir().unwrap();
        let storage =
            Arc::new(StorageService::open(&directory.path().join("test.sqlite")).unwrap());
        let service = AgentService::try_new(storage).unwrap();
        let (sender, mut receiver) = crate::transport::outbound_channel();
        let identity = identity();
        let event = started(1);
        service.emit_child_observer_event(
            &sender,
            &identity,
            "run",
            "assistant",
            &event,
            RendererSafeAgentEvent::new(&event),
        );
        let event = delta("prefix");
        service.emit_child_observer_event(
            &sender,
            &identity,
            "run",
            "assistant",
            &event,
            RendererSafeAgentEvent::new(&event),
        );
        let snapshot = service.observer_streams.lock().unwrap()["child-conversation"]
            .snapshot
            .clone();
        assert_eq!(snapshot.stream.unwrap().content, "prefix");
        let first = receiver.try_recv().unwrap();
        let second = receiver.try_recv().unwrap();
        assert_eq!(first["params"]["streamCursor"]["sequence"], 1);
        assert_eq!(second["params"]["streamCursor"]["sequence"], 2);
        assert_eq!(
            second["params"]["streamCursor"]["generation"],
            snapshot.cursor.generation
        );
        let event = AgentEvent::Done {
            run_id: "run".into(),
            user_interrupted: None,
            success: true,
            status: Some(AgentRunStatus::Completed),
            content: Some("prefix final".into()),
            usage: None,
            finish_reason: None,
            proposed_actions: Vec::new(),
        };
        service.emit_child_observer_event(
            &sender,
            &identity,
            "run",
            "assistant",
            &event,
            RendererSafeAgentEvent::new(&event),
        );
        assert!(service.observer_streams.lock().unwrap().is_empty());
        assert_eq!(
            receiver.try_recv().unwrap()["params"]["event"]["content"],
            "prefix final"
        );
    }
}
