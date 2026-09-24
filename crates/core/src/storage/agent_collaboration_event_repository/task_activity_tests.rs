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
    .unwrap_or_else(|error| panic!("finishing {status:?}: {error:?}"))
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
            terminal.activities.is_empty(),
            "Result wake ended as {status:?}"
        );
        let frozen = list_message_activities(&connection, "root-active").unwrap();
        assert!(frozen.iter().all(|event| event.event_id != event_id));
        let grand_terminal = root_events
            .iter()
            .filter_map(|event| event.activities.first())
            .find(|activity| {
                activity.agent_id == "agent-grand"
                    && activity.semantic == AgentCollaborationActivitySemantic::Completed
            })
            .unwrap();
        assert_eq!(grand_terminal.owner_agent_id, "agent-child");
        assert_eq!(
            grand_terminal.anchor_message_id.as_deref(),
            Some("child-active")
        );
        assert!(list_message_activities(&connection, "child-active")
            .unwrap()
            .iter()
            .any(|event| event.activities.first() == Some(grand_terminal)));
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
                        .activities
                        .first()
                        .is_some_and(|activity| activity.semantic == expected)
                })
                .unwrap();
            assert_eq!(terminal.message_id.as_deref(), Some("task-source"));
            let activity = terminal.activities.first().unwrap();
            assert_eq!(activity.agent_id, "agent-grand");
            assert_eq!(activity.owner_agent_id, sender);
            assert_eq!(activity.task_message_id.as_deref(), Some("task-source"));
            let (owner_conversation, owner_message, other_message) = if sender == "agent-root" {
                ("conversation-root", "root-active", "child-active")
            } else {
                ("conversation-child", "child-active", "root-active")
            };
            assert_eq!(activity.owner_conversation_id, owner_conversation);
            assert_eq!(activity.trace_boundary_sequence, Some(0));
            assert!(list_message_activities(&connection, owner_message)
                .unwrap()
                .contains(terminal));
            assert!(list_message_activities(&connection, other_message)
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
            event.activities.first().is_some_and(|activity| {
                activity.semantic == AgentCollaborationActivitySemantic::Completed
            })
        })
        .unwrap();
    let activity = terminal.activities.first().unwrap();
    assert_eq!(activity.agent_id, "agent-child");
    assert_eq!(activity.owner_agent_id, "agent-root");
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
    let events = assert_all_event_cursors_unchanged(&connection);
    let terminal = events
        .iter()
        .find(|event| {
            event.kind == AgentCollaborationEventKind::WakeUpdated && event.created_at == 30
        })
        .unwrap();
    assert!(terminal.activities.is_empty());
    assert!(list_message_activities(&connection, "root-active")
        .unwrap()
        .iter()
        .all(|event| event.event_id != terminal.event_id));
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
            !terminal.activities.is_empty(),
            expected_activity,
            "{kind:?}, bound={bind}"
        );
        let frozen = list_message_activities(&connection, "root-active").unwrap();
        assert_eq!(
            frozen
                .iter()
                .find(|event| event.event_id == terminal.event_id)
                .map(|event| event.activities.clone())
                .unwrap_or_default(),
            terminal.activities
        );
    }
}

#[test]
fn one_run_routes_each_bound_assignment_to_its_actual_dispatcher_and_freezes_each_owner() {
    for status in [
        AgentWakeStatus::Completed,
        AgentWakeStatus::Failed,
        AgentWakeStatus::Interrupted,
    ] {
        let mut connection = tree_with_active_parents();
        let wake = enqueue_task(
            &mut connection,
            AgentMailboxKind::Task,
            "agent-child",
            "agent-grand",
        );
        start_wake(&mut connection, &wake, 22);
        connection.execute_batch("INSERT INTO messages(id,conversation_id,role,content,status,created_at,position)
            VALUES ('grand-assistant','conversation-grand','assistant','','pending',25,
                (SELECT COALESCE(MAX(position),-1)+1 FROM messages WHERE conversation_id='conversation-grand'));").unwrap();
        crate::storage::conversation_trace_repository::append_in_progress_trace(
            &mut connection,
            &crate::ConversationTraceSnapshot::default().in_progress_trace(
                "grand-run",
                "conversation-grand",
                "grand-assistant",
            ),
            25,
            25,
        )
        .unwrap();
        crate::storage::agent_workspace_repository::freeze_run(
            &connection,
            "grand-run",
            None,
            None,
        )
        .unwrap();
        connection.execute("UPDATE agent_wake_requests SET run_id='grand-run',assistant_message_id='grand-assistant' WHERE wake_id=?1", [&wake.wake_id]).unwrap();
        delivery::bind_turn_start_messages(
            &mut connection,
            &BindAgentTurnStartInput {
                conversation_id: "conversation-grand".into(),
                run_id: "grand-run".into(),
                assistant_message_id: "grand-assistant".into(),
                model_batch_index: 1,
            },
            &["task-source".into()],
            25,
        )
        .unwrap();
        let mut assignments = vec![("task-source".to_string(), "agent-child")];
        for (request_id, sender) in [("root-extra", "agent-root"), ("child-extra", "agent-child")] {
            let sent = graph::follow_up_agent(
                &mut connection,
                &SendAgentMessageRequest {
                    sender_agent_id: sender.into(),
                    recipient_agent_id: "agent-grand".into(),
                    request_id: request_id.into(),
                    content: "additional explicit task".into(),
                },
                26,
            )
            .unwrap();
            assignments.push((sent.message.message_id, sender));
        }
        let ordinary = graph::send_agent_message(
            &mut connection,
            &SendAgentMessageRequest {
                sender_agent_id: "agent-root".into(),
                recipient_agent_id: "agent-grand".into(),
                request_id: "ordinary-context".into(),
                content: "context only, no task subscription".into(),
            },
            27,
        )
        .unwrap();
        delivery::bind_safe_boundary(
            &mut connection,
            &BindAgentSafeBoundaryInput {
                conversation_id: "conversation-grand".into(),
                run_id: "grand-run".into(),
                assistant_message_id: "grand-assistant".into(),
                model_batch_index: 2,
                expected_next_trace_sequence: 0,
                maximum: 64,
            },
            128,
        )
        .unwrap(); // Simulate a later wall-clock rollback before approval/terminal.
        let unbound = graph::follow_up_agent(
            &mut connection,
            &SendAgentMessageRequest {
                sender_agent_id: "agent-root".into(),
                recipient_agent_id: "agent-grand".into(),
                request_id: "next-round".into(),
                content: "not delivered to this Run".into(),
            },
            29,
        )
        .unwrap();
        connection.execute("INSERT INTO agent_pending_actions(action_id,run_id,conversation_id,assistant_message_id,
            action_type,tool_name,tool_call_id,status,action_json,agent_input_json,created_at,updated_at)
            VALUES ('grand-approval','grand-run','conversation-grand','grand-assistant','tool_call','read_file','approval-call','pending','{}','{}',30,30)", []).unwrap();
        let approval = list_root_events(&connection, "agent-root", 0, 512)
            .unwrap()
            .into_iter()
            .find(|event| event.kind == AgentCollaborationEventKind::ApprovalProjected)
            .unwrap();
        assert_eq!(approval.activities.len(), 3);
        for (task, owner) in &assignments {
            assert!(approval
                .activities
                .iter()
                .any(|activity| activity.task_message_id.as_ref() == Some(task)
                    && activity.owner_agent_id == *owner
                    && activity.semantic == AgentCollaborationActivitySemantic::WaitingApproval));
        }
        connection.execute("UPDATE agent_pending_actions SET status='rejected',updated_at=31 WHERE action_id='grand-approval'",[]).unwrap();
        let trace_status = match status {
            AgentWakeStatus::Failed => "failed",
            AgentWakeStatus::Interrupted => "cancelled",
            _ => "completed",
        };
        if status != AgentWakeStatus::Failed {
            connection.execute("UPDATE conversation_turn_traces SET terminal_status=?1,completed_at=32,updated_at=32 WHERE run_id='grand-run'",[trace_status]).unwrap();
        }
        let wake = graph::get_agent_wake(&connection, &wake.wake_id)
            .unwrap()
            .unwrap();
        let settled = finish_wake(&mut connection, &wake, status, 33);
        assert_eq!(settled.result_message.recipient_agent_id, "agent-child");
        let events = assert_all_event_cursors_unchanged(&connection);
        let terminal = events
            .iter()
            .find(|event| {
                event.kind == AgentCollaborationEventKind::WakeUpdated
                    && event.run_id.as_deref() == Some("grand-run")
                    && event.created_at == 33
            })
            .unwrap();
        assert_eq!(terminal.activities.len(), 3);
        for (task, owner) in &assignments {
            let activity = terminal
                .activities
                .iter()
                .find(|activity| activity.task_message_id.as_ref() == Some(task))
                .unwrap();
            assert_eq!(activity.owner_agent_id, *owner);
            assert_eq!(activity.agent_id, "agent-grand");
            assert_eq!(
                activity.anchor_message_id.as_deref(),
                Some(if *owner == "agent-root" {
                    "root-active"
                } else {
                    "child-active"
                })
            );
        }
        assert!(terminal
            .activities
            .iter()
            .all(|activity| activity.task_message_id.as_deref()
                != Some(&unbound.message.message_id)
                && activity.task_message_id.as_deref() != Some(&ordinary.message.message_id)));
        assert_eq!(
            terminal
                .activities
                .iter()
                .map(|activity| &activity.activity_id)
                .collect::<std::collections::HashSet<_>>()
                .len(),
            3
        );
        for (owner, message, expected) in [
            ("agent-root", "root-active", 1),
            ("agent-child", "child-active", 2),
        ] {
            let frozen = list_message_activities(&connection, message).unwrap();
            let projected = frozen
                .iter()
                .find(|event| event.event_id == terminal.event_id)
                .unwrap();
            assert_eq!(projected.activities.len(), expected);
            assert!(projected
                .activities
                .iter()
                .all(|activity| activity.owner_agent_id == owner));
            assert!(projected
                .activities
                .iter()
                .all(|activity| terminal.activities.contains(activity)));
        }
        assert!(connection.execute("UPDATE agent_collaboration_event_activities SET owner_agent_id='agent-root' WHERE activity_id=?1",[&terminal.activities[0].activity_id]).is_err());
    }
}

#[test]
fn ordinary_cross_level_messages_have_real_sender_recipient_but_no_task_identity() {
    let mut connection = tree_with_active_parents();
    for (sender, recipient) in [
        ("agent-root", "agent-grand"),
        ("agent-grand", "agent-root"),
        ("agent-child", "agent-root"),
    ] {
        let sent = graph::send_agent_message(
            &mut connection,
            &SendAgentMessageRequest {
                sender_agent_id: sender.into(),
                recipient_agent_id: recipient.into(),
                request_id: format!("{sender}-{recipient}"),
                content: "message only".into(),
            },
            20,
        )
        .unwrap();
        let event = list_root_events(&connection, "agent-root", 0, 512)
            .unwrap()
            .into_iter()
            .find(|event| {
                event.kind == AgentCollaborationEventKind::MailboxEnqueued
                    && event.message_id.as_deref() == Some(&sent.message.message_id)
            })
            .unwrap();
        assert_eq!(event.activities.len(), 1);
        let activity = &event.activities[0];
        assert_eq!(activity.agent_id, sender);
        assert_eq!(activity.owner_agent_id, recipient);
        assert_eq!(
            activity.semantic,
            AgentCollaborationActivitySemantic::Updated
        );
        assert!(activity.task_message_id.is_none());
    }
}

#[test]
fn frozen_activity_window_counts_assignments_inside_one_event() {
    let mut connection = tree_with_active_parents();
    enqueue_task(
        &mut connection,
        AgentMailboxKind::Task,
        "agent-root",
        "agent-child",
    );
    let mut event = list_root_events(&connection, "agent-root", 0, 512)
        .unwrap()
        .into_iter()
        .find(|event| !event.activities.is_empty())
        .unwrap();
    let first = event.activities[0].clone();
    event.activities = (0..2050)
        .map(|index| {
            let mut activity = first.clone();
            activity.activity_id = format!("activity-{index:04}");
            activity.task_message_id = Some(format!("task-{index:04}"));
            activity
        })
        .collect();
    let mut events = vec![event];
    retain_latest_activities(&mut events, 2048);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].activities.len(), 2048);
    assert_eq!(
        events[0].activities[0].task_message_id.as_deref(),
        Some("task-0002")
    );
    assert_eq!(
        events[0].activities[2047].task_message_id.as_deref(),
        Some("task-2049")
    );
}
