//! Isolated mixed child-stream regression through the real bounded queue and writer.
//! These finite bursts measure transport delivery, not RPC execution or unbounded-load safety.

use super::*;
use crate::transport::{outbound_channel, run_outbound_writer};
use mycopilot_core::{AgentProposedAction, AgentRunCheckpoint, AgentToolResult};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::AsyncReadExt;
use tokio::sync::{mpsc, oneshot};

const TEXT_PARTS: usize = 32;

fn child_identity(index: usize) -> AgentCollaborationIdentity {
    AgentCollaborationIdentity {
        agent_id: format!("child-{index}"),
        root_agent_id: "root".into(),
        root_conversation_id: "root-conversation".into(),
        parent_agent_id: "root".into(),
        parent_task_name: "root".into(),
        parent_task_path: "/root".into(),
        conversation_id: format!("child-conversation-{index}"),
        task_name: format!("child-{index}"),
        task_path: format!("/root/child-{index}"),
        source_agent_id: "root".into(),
        source_kind: mycopilot_core::AgentMailboxKind::Task,
        source_task_name: "root".into(),
        source_task_path: "/root".into(),
        source_agent_message_id: format!("task-{index}"),
        entrusted_task: "isolated transport fixture".into(),
        template_instructions: None,
    }
}

fn approval_event(run_id: &str) -> AgentEvent {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../../packages/protocol/fixtures/agent-mcp-renderer-contract-v1.json"
    ))
    .unwrap();
    let mut action = fixture["approvalRequired"]["action"].clone();
    action["approval"]["identity"]["argumentsDigest"] = json!("private-digest-canary");
    action["approval"]["identity"]["runId"] = json!(run_id);
    let action_id = uuid::Uuid::new_v4().to_string();
    action["approval"]["identity"]["actionId"] = json!(action_id);
    action["approval"]["identity"]["invocationId"] = json!(uuid::Uuid::new_v4().to_string());
    let call_id = action["approval"]["identity"]["callId"]
        .as_str()
        .unwrap()
        .to_owned();
    let action: AgentProposedAction = serde_json::from_value(action).unwrap();
    let checkpoint = AgentRunCheckpoint {
        version: mycopilot_core::AGENT_RUN_CHECKPOINT_SCHEMA_VERSION,
        pause_reason: mycopilot_core::AgentRunCheckpointPauseReason::Approval,
        run_id: run_id.into(),
        context_items: Vec::new(),
        next_model_request_index: 1,
        queued_tool_calls: Vec::new(),
        deferred_external_tool_call_count: 0,
        suppressed_narration: false,
        extension_snapshots: Vec::new(),
        tool_set: crate::test_tool_set_checkpoint(),
        run_context: None,
        collaboration_run_snapshot: None,
        model_capabilities: Default::default(),
        provider_profile_config: crate::test_provider_profile_config(),
        provider_protocol_key: crate::test_provider_protocol_key("test-model"),
        assistant_turn_identity: crate::test_assistant_turn_identity(&[call_id.as_str()]),
        provider_continuation_refs: Vec::new(),
        run_world_state: crate::test_run_world_state(),
        conversation_world_state_records: Vec::new(),
        pending_action_id: Some(action_id),
        file_change_run_grant_ref: None,
        pending_file_observation: None,
        pending_tool_call_id: call_id,
        conversation_trace_items: Vec::new(),
        conversation_model_context_items: Vec::new(),
        next_conversation_trace_sequence: 0,
        conversation_trace_truncated: false,
    };
    AgentEvent::ApprovalRequired {
        run_id: run_id.into(),
        action: Box::new(action),
        checkpoint: Box::new(checkpoint),
        segment_usage: None,
    }
}

fn mixed_events(index: usize) -> Vec<AgentEvent> {
    let run_id = format!("run-{index}");
    let stream_id = format!("stream-{index}");
    let mut events = vec![
        AgentEvent::Started {
            run_id: run_id.clone(),
            tool_definitions: Vec::new(),
        },
        AgentEvent::MessageStreamStarted {
            run_id: run_id.clone(),
            stream_id: stream_id.clone(),
            attempt: 1,
        },
    ];
    for part in 0..TEXT_PARTS {
        events.push(AgentEvent::MessageDelta {
            run_id: run_id.clone(),
            stream_id: Some(stream_id.clone()),
            delta: format!("子任务{index} 第{part}段 😀 \\\"quoted\\\"\n"),
        });
        if part % 8 == 7 {
            events.push(AgentEvent::ToolInputProgress {
                run_id: run_id.clone(),
                stream_id: stream_id.clone(),
                attempt: 1,
                tool_call_index: part / 8,
                tool_call_id: Some(format!("call-{index}-{part}")),
                tool: "read_file".into(),
                received_bytes: 256,
            });
            events.push(AgentEvent::ToolResult {
                run_id: run_id.clone(),
                result: AgentToolResult {
                    call_id: format!("call-{index}-{part}"),
                    tool: "read_file".into(),
                    ok: true,
                    result: Some(json!({"content": "synthetic 工具内容\n".repeat(64)})),
                    error: None,
                    exact_archive_file: None,
                },
            });
        }
        if part == 15 {
            events.push(approval_event(&run_id));
            events.push(AgentEvent::State {
                run_id: run_id.clone(),
                state: mycopilot_core::AgentStateSnapshot {
                    status: AgentRunStatus::WaitingForApproval,
                    active_run_id: Some(run_id.clone()),
                    last_error: None,
                    updated_at: 1,
                },
            });
            events.push(AgentEvent::State {
                run_id: run_id.clone(),
                state: mycopilot_core::AgentStateSnapshot {
                    status: AgentRunStatus::Running,
                    active_run_id: Some(run_id.clone()),
                    last_error: None,
                    updated_at: 2,
                },
            });
        }
    }
    events.push(AgentEvent::FinalAnswerReady {
        run_id: run_id.clone(),
    });
    let status = match index % 3 {
        0 => AgentRunStatus::Completed,
        1 => AgentRunStatus::Failed,
        _ => AgentRunStatus::Cancelled,
    };
    events.push(AgentEvent::Done {
        run_id,
        user_interrupted: (index % 3 == 2).then_some(true),
        success: index.is_multiple_of(3),
        status: Some(status),
        content: None,
        usage: None,
        finish_reason: None,
        proposed_actions: Vec::new(),
    });
    events
}

async fn mixed_transport_sample(children: usize) {
    let directory = tempfile::tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&directory.path().join("test.sqlite")).unwrap());
    let service = AgentService::try_new(storage).unwrap();
    let identities: Vec<_> = (0..children).map(child_identity).collect();
    let streams: Vec<_> = (0..children).map(mixed_events).collect();
    let expected: HashMap<_, Vec<_>> = streams
        .iter()
        .enumerate()
        .map(|(index, events)| {
            (
                format!("run-{index}"),
                events
                    .iter()
                    .cloned()
                    .map(|event| agent_event_notification(event)["params"].clone())
                    .collect(),
            )
        })
        .collect();
    let events_per_child = streams[0].len();
    let (sender, receiver) = outbound_channel();
    let (sink, mut source) = tokio::io::duplex(4096);
    let (_images, image_receiver) = mpsc::channel(1);
    let (finish, finishing) = oneshot::channel();
    let (resume_reader, reader_paused) = oneshot::channel();
    let control_sends = Arc::new(Mutex::new(HashMap::<u64, Instant>::new()));
    let read_control_sends = Arc::clone(&control_sends);
    let writer = tokio::spawn(run_outbound_writer(
        sink,
        receiver,
        image_receiver,
        finishing,
    ));
    let reader = tokio::spawn(async move {
        // Deliberately receive nothing until the entire finite mixed burst has been admitted.
        reader_paused.await.unwrap();
        let mut buffer = [0; 8192];
        let mut pending = Vec::new();
        let mut positions = HashMap::<String, (String, usize)>::new();
        let mut bodies = HashMap::<String, String>::new();
        let mut controls = Vec::new();
        let mut child_frames = 0;
        let mut received_bytes = 0;
        let mut old_dual_route_bytes = 0;
        let mut terminals = 0;
        loop {
            let count = source.read(&mut buffer).await.unwrap();
            if count == 0 {
                break;
            }
            received_bytes += count;
            pending.extend_from_slice(&buffer[..count]);
            while let Some(end) = pending.iter().position(|byte| *byte == b'\n') {
                let value: Value = serde_json::from_slice(&pending[..end]).unwrap();
                pending.drain(..=end);
                if let Some(id) = value["id"].as_u64() {
                    controls.push(
                        read_control_sends
                            .lock()
                            .unwrap()
                            .remove(&id)
                            .unwrap()
                            .elapsed(),
                    );
                    old_dual_route_bytes += serde_json::to_vec(&value).unwrap().len() + 1;
                    continue;
                }
                assert_eq!(value["method"], "agent.collaboration.childEvent");
                let params = &value["params"];
                let run = params["runId"].as_str().unwrap();
                let index: usize = run.strip_prefix("run-").unwrap().parse().unwrap();
                assert_eq!(params["agentId"], format!("child-{index}"));
                assert_eq!(
                    params["conversationId"],
                    format!("child-conversation-{index}")
                );
                assert_eq!(params["assistantMessageId"], format!("assistant-{index}"));
                assert_eq!(params["rootAgentId"], "root");
                assert_eq!(params["rootConversationId"], "root-conversation");
                let generation = params["streamCursor"]["generation"].as_str().unwrap();
                let position = positions
                    .entry(run.into())
                    .or_insert((generation.into(), 0));
                assert_eq!(position.0, generation);
                assert_eq!(params["streamCursor"]["sequence"], position.1 + 1);
                assert_eq!(params["event"], expected[run][position.1]);
                position.1 += 1;
                let event = &params["event"];
                if event["type"] == "message_delta" {
                    bodies
                        .entry(run.into())
                        .or_default()
                        .push_str(event["delta"].as_str().unwrap());
                }
                if event["type"] == "done" {
                    assert_eq!(
                        position.1, events_per_child,
                        "terminal preceded trailing work"
                    );
                    let expected_body: String = expected[run]
                        .iter()
                        .filter_map(|event| event["delta"].as_str())
                        .collect();
                    assert_eq!(bodies[run], expected_body);
                    terminals += 1;
                }
                let ordinary = json!({"jsonrpc":"2.0","method":"agent.event","params":event});
                let mut old_observer = value.clone();
                old_observer["method"] = json!("agent.collaboration.observerEvent");
                old_dual_route_bytes += serde_json::to_vec(&ordinary).unwrap().len()
                    + 1
                    + serde_json::to_vec(&old_observer).unwrap().len()
                    + 1;
                let encoded = value.to_string();
                assert!(!encoded.contains("private-digest-canary"));
                assert!(!encoded.contains("argumentsDigest"));
                assert!(!encoded.contains("checkpoint"));
                child_frames += 1;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        assert!(pending.is_empty(), "writer left a partial JSON frame");
        assert_eq!(child_frames, children * events_per_child);
        assert_eq!(terminals, children);
        assert_eq!(positions.len(), children);
        assert!(positions
            .values()
            .all(|(_, count)| *count == events_per_child));
        assert_eq!(controls.len(), events_per_child);
        controls.sort();
        assert!(old_dual_route_bytes > received_bytes);
        json!({
            "children": children, "child_frames": child_frames,
            "old_dual_route_child_frames": child_frames * 2,
            "wire_bytes": received_bytes, "old_dual_route_modeled_wire_bytes": old_dual_route_bytes,
            "control_delivery_p95_ms": controls[controls.len() * 95 / 100].as_secs_f64() * 1000.0,
            "control_delivery_p99_ms": controls[controls.len() * 99 / 100].as_secs_f64() * 1000.0,
        })
    });
    // Round-robin producers ensure every child type is interleaved with the other identities.
    for (round, _) in streams[0].iter().enumerate() {
        for (index, identity) in identities.iter().enumerate() {
            emit_agent_event_notifications(
                &service,
                &sender,
                Some(identity),
                &format!("run-{index}"),
                &format!("assistant-{index}"),
                streams[index][round].clone(),
            );
            assert!(
                !sender.is_closed(),
                "finite test workload unexpectedly saturated transport"
            );
        }
        control_sends
            .lock()
            .unwrap()
            .insert(round as u64, Instant::now());
        sender
            .send(json!({"jsonrpc":"2.0","id":round,"result":{"control":"accepted"}}))
            .unwrap();
        tokio::task::yield_now().await;
    }
    assert!(service.observer_streams.lock().unwrap().is_empty());
    resume_reader.send(()).unwrap();
    finish.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(10), writer)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let mut report = tokio::time::timeout(Duration::from_secs(10), reader)
        .await
        .unwrap()
        .unwrap();
    let stats = sender.stats();
    assert_eq!(
        stats.accepted_events,
        children * events_per_child + events_per_child
    );
    assert_eq!(
        stats.merged_events, 0,
        "child snapshot cursors must not be naively merged"
    );
    assert!(stats.peak_bytes <= 4 * 1024 * 1024 + 512 * 1024);
    assert!(stats.peak_frames <= 8192);
    assert_eq!(stats.peak_oversize_bytes, 0);
    assert!(control_sends.lock().unwrap().is_empty());
    report["queue_peak_bytes"] = json!(stats.peak_bytes);
    report["queue_peak_frames"] = json!(stats.peak_frames);
    eprintln!("isolated_child_transport={report}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn child_event_transport_mixed_load_preserves_single_copy_with_paused_slow_reader() {
    for children in [10, 30, 50] {
        mixed_transport_sample(children).await;
    }
}
