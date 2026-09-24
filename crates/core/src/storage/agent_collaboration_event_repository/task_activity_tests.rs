//! Regression coverage for explicit task lifecycle projections.

use super::*;
use crate::storage::{agent_delivery_repository as delivery, agent_graph_repository as graph};
use crate::{
    AgentMailboxKind, AgentTurnResultSettlement, AgentWakeRequestRecord, AgentWakeStatus,
    BindAgentSafeBoundaryInput, BindAgentTurnStartInput, CreateAgentNodeInput,
    EnqueueAgentMessageInput, EnqueueAgentWakeInput, FinishAgentTurnResultInput,
    SendAgentMessageRequest,
};

fn tree_with_active_parents() -> Connection {
    let mut connection = super::tests::tree();
    connection
        .execute(
            "INSERT INTO conversations (
                 id, project_id, model_id, title, created_at, updated_at, revision
             ) VALUES ('conversation-grand', 'project-a', 'model-a', 'grand', 3, 3, 0)",
            [],
        )
        .unwrap();
    let model_snapshot = graph::get_agent_node(&connection, "agent-child")
        .unwrap()
        .unwrap()
        .model_snapshot
        .unwrap();
    graph::create_agent_node(
        &mut connection,
        &CreateAgentNodeInput {
            agent_id: "agent-grand".into(),
            root_agent_id: "agent-root".into(),
            parent_agent_id: "agent-child".into(),
            conversation_id: "conversation-grand".into(),
            creation_request_id: "spawn-grand".into(),
            task_name: "nested-review".into(),
            task_path: "/root/review/nested-review".into(),
            template_snapshot: None,
            model_snapshot,
        },
        3,
    )
    .unwrap();
    for (message, conversation, run) in [
        ("root-active", "conversation-root", "run-root"),
        ("child-active", "conversation-child", "run-child"),
    ] {
        connection
            .execute(
                "INSERT INTO messages (
                     id, conversation_id, role, content, status, created_at, position
                 ) VALUES (?1, ?2, 'assistant', '', 'pending', 10, 0)",
                params![message, conversation],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO conversation_turn_traces (
                     assistant_message_id, conversation_id, run_id, schema_version,
                     terminal_status, truncated, created_at, updated_at
                 ) VALUES (?1, ?2, ?3, 1, 'in_progress', 0, 10, 10)",
                params![message, conversation, run],
            )
            .unwrap();
    }
    connection
}

fn enqueue_task(
    connection: &mut Connection,
    kind: AgentMailboxKind,
    sender: &str,
    recipient: &str,
) -> AgentWakeRequestRecord {
    let message = graph::enqueue_agent_message(
        connection,
        &EnqueueAgentMessageInput {
            message_id: "task-source".into(),
            root_agent_id: "agent-root".into(),
            sender_agent_id: sender.into(),
            recipient_agent_id: recipient.into(),
            request_id: "task-request".into(),
            kind,
            content: "perform the delegated work".into(),
            projection_message_id: "task-projection".into(),
        },
        20,
    )
    .unwrap()
    .record()
    .clone();
    graph::enqueue_agent_wake(
        connection,
        &EnqueueAgentWakeInput {
            wake_id: "explicit-task-wake".into(),
            root_agent_id: "agent-root".into(),
            agent_id: recipient.into(),
            requester_agent_id: sender.into(),
            request_id: "explicit-wake-request".into(),
            source_agent_message_id: Some(message.message_id),
        },
        21,
    )
    .unwrap()
    .record()
    .clone()
}

fn start_wake(connection: &mut Connection, wake: &AgentWakeRequestRecord, now: i64) {
    let claim = format!("claim-{}", wake.wake_id);
    let transaction = connection.transaction().unwrap();
    graph::project_agent_wake_source_in_transaction(
        &transaction,
        &wake.wake_id,
        &format!("projection-{}", wake.wake_id),
        now,
    )
    .unwrap();
    transaction.commit().unwrap();
    let claimed = graph::claim_next_agent_wake(connection, &wake.agent_id, &claim, now + 1)
        .unwrap()
        .unwrap();
    assert_eq!(claimed.wake_id, wake.wake_id);
    graph::transition_agent_wake(
        connection,
        &wake.wake_id,
        AgentWakeStatus::Claimed,
        AgentWakeStatus::Running,
        Some(&claim),
        now + 2,
    )
    .unwrap();
}

fn finish_wake(
    connection: &mut Connection,
    wake: &AgentWakeRequestRecord,
    status: AgentWakeStatus,
    now: i64,
) -> AgentTurnResultSettlement {
    graph::finish_agent_turn_with_result(
        connection,
        &FinishAgentTurnResultInput {
            wake_id: wake.wake_id.clone(),
            expected_status: AgentWakeStatus::Running,
            claim_token: format!("claim-{}", wake.wake_id),
            terminal_status: status,
            run_id: wake.run_id.clone(),
            assistant_message_id: wake.assistant_message_id.clone(),
            terminal_error: (status == AgentWakeStatus::Failed).then(|| "fixture failure".into()),
        },
        now,
    )
    .unwrap()
}

fn assert_all_event_cursors_unchanged(
    connection: &Connection,
) -> Vec<AgentCollaborationEventRecord> {
    let root_cursor = latest_root_sequence(connection, "agent-root").unwrap();
    let global_cursor = latest_global_sequence(connection).unwrap();
    let root = list_root_events(connection, "agent-root", 0, 512).unwrap();
    let global = list_global_events(connection, 0, 512).unwrap();
    assert_eq!(root, global);
    assert_eq!(root.len() as u64, root_cursor);
    assert_eq!(root_cursor, global_cursor);
    for (index, event) in root.iter().enumerate() {
        assert_eq!(event.root_sequence, index as u64 + 1);
        assert_eq!(event.global_sequence, index as u64 + 1);
    }
    let paged = root
        .iter()
        .flat_map(|event| {
            list_root_events(connection, "agent-root", event.root_sequence - 1, 1).unwrap()
        })
        .collect::<Vec<_>>();
    assert_eq!(root, paged);
    assert_eq!(
        latest_root_sequence(connection, "agent-root").unwrap(),
        root_cursor
    );
    assert_eq!(latest_global_sequence(connection).unwrap(), global_cursor);
    root
}

#[test]
fn result_wake_terminal_processing_never_repeats_the_parent_task_status() {
    for status in [
        AgentWakeStatus::Completed,
        AgentWakeStatus::Failed,
        AgentWakeStatus::Interrupted,
        AgentWakeStatus::Cancelled,
    ] {
        let mut connection = tree_with_active_parents();
        let grand = enqueue_task(
            &mut connection,
            AgentMailboxKind::Task,
            "agent-child",
            "agent-grand",
        );
        start_wake(&mut connection, &grand, 22);
        let settled = finish_wake(&mut connection, &grand, AgentWakeStatus::Completed, 30);
        assert_eq!(settled.result_message.kind, AgentMailboxKind::Result);
        assert_eq!(settled.result_message.recipient_agent_id, "agent-child");
        let result_wake = settled.parent_wake.unwrap();
        assert_eq!(
            result_wake.source_agent_message_id.as_deref(),
            Some(settled.result_message.message_id.as_str())
        );
        connection
            .execute(
                "UPDATE conversation_turn_traces SET terminal_status = 'completed',
                 completed_at = 31, updated_at = 31 WHERE run_id = 'run-child'",
                [],
            )
            .unwrap();
        if status == AgentWakeStatus::Cancelled {
            graph::transition_agent_wake(
                &mut connection,
                &result_wake.wake_id,
                AgentWakeStatus::Queued,
                status,
                None,
                40,
            )
            .unwrap();
        } else {
            start_wake(&mut connection, &result_wake, 32);
            let parent_settled = finish_wake(&mut connection, &result_wake, status, 40);
            assert!(parent_settled.parent_wake.is_none());
            assert_eq!(
                parent_settled.result_message.recipient_agent_id,
                "agent-root"
            );
        }
        let (event_id, persisted_semantic): (String, String) = connection
            .query_row(
                "SELECT event_id, activity_semantic FROM agent_collaboration_events
                 WHERE kind = 'wake_updated' AND message_id = ?1
                   AND activity_semantic IS NOT NULL",
                [&settled.result_message.message_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert!(!persisted_semantic.is_empty());
        let root_events = assert_all_event_cursors_unchanged(&connection);
        let terminal = root_events
            .iter()
            .find(|event| event.event_id == event_id)
            .unwrap();
        assert_eq!(terminal.agent_id, "agent-child");
        assert!(
            terminal.activity.is_none(),
            "Result wake ended as {status:?}"
        );
        let frozen = list_message_activities(&connection, "root-active").unwrap();
        assert!(frozen
            .iter()
            .find(|event| event.event_id == event_id)
            .unwrap()
            .activity
            .is_none());
        let grand_terminal = root_events
            .iter()
            .filter_map(|event| event.activity.as_ref())
            .find(|activity| {
                activity.agent_id == "agent-grand"
                    && activity.semantic == AgentCollaborationActivitySemantic::Completed
            })
            .unwrap();
        assert_eq!(grand_terminal.parent_agent_id, "agent-child");
        assert_eq!(
            grand_terminal.anchor_message_id.as_deref(),
            Some("child-active")
        );
        assert!(list_message_activities(&connection, "child-active")
            .unwrap()
            .iter()
            .any(|event| event.activity.as_ref() == Some(grand_terminal)));
    }
}

#[test]
fn direct_tasks_and_ancestor_followups_keep_real_terminal_activities() {
    for (kind, sender) in [
        (AgentMailboxKind::Task, "agent-child"),
        (AgentMailboxKind::Followup, "agent-root"),
    ] {
        for status in [
            AgentWakeStatus::Completed,
            AgentWakeStatus::Failed,
            AgentWakeStatus::Interrupted,
        ] {
            let mut connection = tree_with_active_parents();
            let wake = enqueue_task(&mut connection, kind, sender, "agent-grand");
            start_wake(&mut connection, &wake, 22);
            finish_wake(&mut connection, &wake, status, 30);
            let expected = match status {
                AgentWakeStatus::Completed => AgentCollaborationActivitySemantic::Completed,
                AgentWakeStatus::Failed => AgentCollaborationActivitySemantic::Failed,
                _ => AgentCollaborationActivitySemantic::Interrupted,
            };
            let root = assert_all_event_cursors_unchanged(&connection);
            let terminal = root
                .iter()
                .find(|event| {
                    event
                        .activity
                        .as_ref()
                        .is_some_and(|activity| activity.semantic == expected)
                })
                .unwrap();
            assert_eq!(terminal.message_id.as_deref(), Some("task-source"));
            let activity = terminal.activity.as_ref().unwrap();
            assert_eq!(activity.agent_id, "agent-grand");
            assert_eq!(activity.parent_agent_id, "agent-child");
            assert_eq!(activity.parent_conversation_id, "conversation-child");
            assert_eq!(activity.trace_boundary_sequence, Some(0));
            assert!(list_message_activities(&connection, "child-active")
                .unwrap()
                .contains(terminal));
            assert!(list_message_activities(&connection, "root-active")
                .unwrap()
                .iter()
                .all(|event| event.event_id != terminal.event_id));
        }
    }
}

#[test]
fn genuine_task_completion_after_the_parent_reply_stays_visible() {
    let mut connection = tree_with_active_parents();
    let wake = enqueue_task(
        &mut connection,
        AgentMailboxKind::Task,
        "agent-root",
        "agent-child",
    );
    start_wake(&mut connection, &wake, 22);
    connection
        .execute(
            "UPDATE conversation_turn_traces SET terminal_status = 'completed',
             completed_at = 29, updated_at = 29 WHERE run_id = 'run-root'",
            [],
        )
        .unwrap();
    finish_wake(&mut connection, &wake, AgentWakeStatus::Completed, 30);
    let events = assert_all_event_cursors_unchanged(&connection);
    let terminal = events
        .iter()
        .find(|event| {
            event.activity.as_ref().is_some_and(|activity| {
                activity.semantic == AgentCollaborationActivitySemantic::Completed
            })
        })
        .unwrap();
    let activity = terminal.activity.as_ref().unwrap();
    assert_eq!(activity.agent_id, "agent-child");
    assert_eq!(activity.parent_agent_id, "agent-root");
    assert_eq!(activity.anchor_message_id.as_deref(), Some("root-active"));
    assert_eq!(activity.trace_boundary_sequence, None);
    assert!(list_message_activities(&connection, "root-active")
        .unwrap()
        .iter()
        .all(|event| event.event_id != terminal.event_id));
}

#[test]
fn wake_without_an_explicit_task_source_does_not_emit_a_terminal_card() {
    let mut connection = tree_with_active_parents();
    let wake = graph::enqueue_agent_wake(
        &mut connection,
        &EnqueueAgentWakeInput {
            wake_id: "unbound-wake".into(),
            root_agent_id: "agent-root".into(),
            agent_id: "agent-child".into(),
            requester_agent_id: "agent-root".into(),
            request_id: "unbound-request".into(),
            source_agent_message_id: None,
        },
        20,
    )
    .unwrap()
    .record()
    .clone();
    start_wake(&mut connection, &wake, 22);
    finish_wake(&mut connection, &wake, AgentWakeStatus::Completed, 30);
    for events in [
        assert_all_event_cursors_unchanged(&connection),
        list_message_activities(&connection, "root-active").unwrap(),
    ] {
        let terminal = events
            .iter()
            .find(|event| {
                event.kind == AgentCollaborationEventKind::WakeUpdated && event.created_at == 30
            })
            .unwrap();
        assert!(terminal.activity.is_none());
    }
}

#[test]
fn result_turn_only_becomes_task_activity_after_a_followup_is_actually_bound() {
    for (kind, bind, expected_activity) in [
        (AgentMailboxKind::Followup, true, true),
        (AgentMailboxKind::Followup, false, false),
        (AgentMailboxKind::Message, true, false),
    ] {
        let mut connection = tree_with_active_parents();
        let grand = enqueue_task(
            &mut connection,
            AgentMailboxKind::Task,
            "agent-child",
            "agent-grand",
        );
        start_wake(&mut connection, &grand, 22);
        let settled = finish_wake(&mut connection, &grand, AgentWakeStatus::Completed, 30);
        connection
            .execute(
                "UPDATE conversation_turn_traces SET terminal_status = 'completed',
                 completed_at = 31, updated_at = 31 WHERE run_id = 'run-child'",
                [],
            )
            .unwrap();
        let parent_wake = settled.parent_wake.unwrap();
        start_wake(&mut connection, &parent_wake, 32);
        connection
            .execute_batch(
                "INSERT INTO messages (
                     id, conversation_id, role, content, status, created_at, position
                 ) VALUES ('result-assistant', 'conversation-child', 'assistant', '', 'pending', 35,
                     (SELECT MAX(position) + 1 FROM messages WHERE conversation_id = 'conversation-child'));",
            )
            .unwrap();
        crate::storage::conversation_trace_repository::append_in_progress_trace(
            &mut connection,
            &crate::ConversationTraceSnapshot::default().in_progress_trace(
                "result-run",
                "conversation-child",
                "result-assistant",
            ),
            35,
            35,
        )
        .unwrap();
        connection
            .execute(
                "UPDATE agent_wake_requests SET run_id = 'result-run',
                 assistant_message_id = 'result-assistant' WHERE wake_id = ?1",
                [&parent_wake.wake_id],
            )
            .unwrap();
        delivery::bind_turn_start_messages(
            &mut connection,
            &BindAgentTurnStartInput {
                conversation_id: "conversation-child".into(),
                run_id: "result-run".into(),
                assistant_message_id: "result-assistant".into(),
                model_batch_index: 1,
            },
            std::slice::from_ref(&settled.result_message.message_id),
            35,
        )
        .unwrap()
        .unwrap();
        let request = SendAgentMessageRequest {
            sender_agent_id: "agent-root".into(),
            recipient_agent_id: "agent-child".into(),
            request_id: "additional-work".into(),
            content: "additional input for this running turn".into(),
        };
        let sent = if kind == AgentMailboxKind::Followup {
            graph::follow_up_agent(&mut connection, &request, 36).unwrap()
        } else {
            graph::send_agent_message(&mut connection, &request, 36).unwrap()
        };
        if bind {
            let bound = delivery::bind_safe_boundary(
                &mut connection,
                &BindAgentSafeBoundaryInput {
                    conversation_id: "conversation-child".into(),
                    run_id: "result-run".into(),
                    assistant_message_id: "result-assistant".into(),
                    model_batch_index: 2,
                    expected_next_trace_sequence: 0,
                    maximum: 64,
                },
                37,
            )
            .unwrap()
            .unwrap();
            assert!(bound
                .messages
                .iter()
                .any(|message| message.message_id == sent.message.message_id));
        }
        if let Some(deferred) = sent.deferred_wake {
            assert_eq!(
                graph::get_agent_wake(&connection, &deferred.wake_id)
                    .unwrap()
                    .unwrap()
                    .status,
                if bind {
                    AgentWakeStatus::Satisfied
                } else {
                    AgentWakeStatus::Queued
                }
            );
        }
        connection
            .execute(
                "UPDATE conversation_turn_traces SET terminal_status = 'completed',
                 completed_at = 39, updated_at = 39 WHERE run_id = 'result-run'",
                [],
            )
            .unwrap();
        let parent_wake = graph::get_agent_wake(&connection, &parent_wake.wake_id)
            .unwrap()
            .unwrap();
        finish_wake(
            &mut connection,
            &parent_wake,
            AgentWakeStatus::Completed,
            40,
        );
        let events = assert_all_event_cursors_unchanged(&connection);
        let terminal = events
            .iter()
            .find(|event| {
                event.kind == AgentCollaborationEventKind::WakeUpdated
                    && event.run_id.as_deref() == Some("result-run")
                    && event.created_at == 40
            })
            .unwrap();
        assert_eq!(
            terminal.activity.is_some(),
            expected_activity,
            "{kind:?}, bound={bind}"
        );
        let frozen = list_message_activities(&connection, "root-active").unwrap();
        assert_eq!(
            frozen
                .iter()
                .find(|event| event.event_id == terminal.event_id)
                .unwrap()
                .activity,
            terminal.activity
        );
    }
}
