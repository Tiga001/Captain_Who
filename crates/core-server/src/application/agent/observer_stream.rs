use super::*;
use mycopilot_protocol_rs::{
    AgentObserverLiveStreamDto, AgentObserverLiveStreamSnapshotDto, AgentObserverStreamCursorDto,
};
use serde_json::json;

/// Only provisional text is retained. Committed narration already lives in the trace; the final
/// answer is released after the terminal persistence notification. No per-token database writes.
pub(super) struct ObserverStreamState {
    pub agent_id: String,
    pub root_agent_id: String,
    pub root_conversation_id: String,
    pub snapshot: AgentObserverLiveStreamSnapshotDto,
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
            },
        }
    }

    fn apply(&mut self, event: &AgentEvent, boundary: u64) {
        self.snapshot.cursor.sequence += 1;
        match event {
            AgentEvent::MessageStreamStarted {
                stream_id, attempt, ..
            } => {
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
                self.snapshot.stream = Some(AgentObserverLiveStreamDto {
                    stream_id: format!("observer-{}", self.snapshot.run_id),
                    attempt: 1,
                    content: content.clone(),
                    trace_boundary_sequence: boundary,
                    committed: true,
                });
            }
            _ => {}
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
                .get_conversation_turn_trace(assistant_message_id)
                .ok()
                .flatten()
                .and_then(|trace| trace.items.last().map(|item| item.sequence() + 1))
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
        let mut notification = child_observer_event_notification_from_safe(
            identity,
            run_id,
            assistant_message_id,
            safe_event,
        );
        notification["params"]["streamCursor"] = json!(state.snapshot.cursor);
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
    fn shared_projection_preserves_both_routes_privacy_identity_and_snapshot_cursor() {
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
            delta("你好 🌍\n```rs\n"),
            delta(""),
            delta("```"),
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
            emit_agent_event_notifications(
                &service,
                &sender,
                Some(&identity),
                "run",
                "assistant",
                event,
            );
            let ordinary = receiver.try_recv().unwrap();
            let observer = receiver.try_recv().unwrap();
            assert_eq!(ordinary["method"], AGENT_EVENT_NAME);
            assert_eq!(observer["params"]["event"], ordinary["params"]);
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
            for notification in [ordinary, observer] {
                let serialized = notification.to_string();
                assert!(!serialized.contains("argumentsDigest"));
                assert!(!serialized.contains("host-only-arguments-digest"));
            }
        }
        let snapshots = service.observer_streams.lock().unwrap();
        let snapshot = &snapshots[&identity.conversation_id].snapshot;
        assert_eq!(snapshot.cursor.sequence, 5);
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
