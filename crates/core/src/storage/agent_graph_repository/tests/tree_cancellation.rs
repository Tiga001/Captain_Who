use super::*;
use rusqlite::OptionalExtension;

fn descendant_wake(
    wake_id: &str,
    agent_id: &str,
    requester_agent_id: &str,
) -> EnqueueAgentWakeInput {
    EnqueueAgentWakeInput {
        wake_id: wake_id.to_string(),
        root_agent_id: "agent-root".to_string(),
        agent_id: agent_id.to_string(),
        requester_agent_id: requester_agent_id.to_string(),
        request_id: format!("request-{wake_id}"),
        source_agent_message_id: None,
    }
}

fn insert_active_trace(
    connection: &mut Connection,
    conversation_id: &str,
    run_id: &str,
    assistant_message_id: &str,
    timestamp: i64,
) {
    connection
        .execute(
            "INSERT INTO messages (
                 id, conversation_id, role, content, status, created_at, position
             ) VALUES (
                 ?1, ?2, 'assistant', '', 'pending', ?3,
                 (SELECT COALESCE(MAX(position), -1) + 1 FROM messages
                  WHERE conversation_id = ?2)
             )",
            params![assistant_message_id, conversation_id, timestamp],
        )
        .unwrap();
    let trace = crate::ConversationTraceSnapshot::default().in_progress_trace(
        run_id,
        conversation_id,
        assistant_message_id,
    );
    crate::storage::conversation_trace_repository::append_in_progress_trace(
        connection, &trace, timestamp, timestamp,
    )
    .unwrap();
    let project_id: Option<String> = connection
        .query_row(
            "SELECT project_id FROM conversations WHERE id=?1",
            [conversation_id],
            |row| row.get(0),
        )
        .unwrap();
    crate::storage::agent_workspace_repository::freeze_run(
        connection,
        run_id,
        None,
        project_id.as_deref(),
    )
    .unwrap();
}

#[allow(clippy::too_many_arguments)]
fn admit_wake(
    connection: &mut Connection,
    agent_id: &str,
    conversation_id: &str,
    wake_id: &str,
    claim_token: &str,
    run_id: &str,
    assistant_message_id: &str,
    status: AgentWakeStatus,
    timestamp: i64,
) {
    let claimed = claim_next_agent_wake(connection, agent_id, claim_token, timestamp)
        .unwrap()
        .unwrap();
    assert_eq!(claimed.wake_id, wake_id);
    transition_agent_wake(
        connection,
        wake_id,
        AgentWakeStatus::Claimed,
        AgentWakeStatus::Running,
        Some(claim_token),
        timestamp + 1,
    )
    .unwrap();
    connection
        .execute(
            "INSERT INTO messages (
                 id, conversation_id, role, content, status, created_at, position
             ) VALUES (
                 ?1, ?2, 'assistant', 'running child fixture', 'pending', ?3,
                 (SELECT COALESCE(MAX(position), -1) + 1 FROM messages
                  WHERE conversation_id = ?2)
             )",
            params![assistant_message_id, conversation_id, timestamp + 1],
        )
        .unwrap();
    let trace = crate::ConversationTraceSnapshot::default().in_progress_trace(
        run_id,
        conversation_id,
        assistant_message_id,
    );
    crate::storage::conversation_trace_repository::append_in_progress_trace(
        connection,
        &trace,
        timestamp + 1,
        timestamp + 1,
    )
    .unwrap();
    crate::storage::agent_workspace_repository::freeze_run(connection, run_id, Some(wake_id), None)
        .unwrap();
    connection
        .execute(
            "UPDATE agent_wake_requests
             SET run_id = ?1, assistant_message_id = ?2
             WHERE wake_id = ?3 AND status = 'running'",
            params![run_id, assistant_message_id, wake_id],
        )
        .unwrap();
    if status == AgentWakeStatus::WaitingForApproval {
        transition_agent_wake(
            connection,
            wake_id,
            AgentWakeStatus::Running,
            AgentWakeStatus::WaitingForApproval,
            Some(claim_token),
            timestamp + 2,
        )
        .unwrap();
    } else {
        assert_eq!(status, AgentWakeStatus::Running);
    }
}

#[test]
fn active_interrupt_receipt_retries_until_dispatch_and_then_suppresses_redelivery() {
    let mut connection = setup_tree();
    enqueue_agent_wake(
        &mut connection,
        &descendant_wake("wake-active-interrupt", "agent-child", "agent-root"),
        20,
    )
    .unwrap();
    admit_wake(
        &mut connection,
        "agent-child",
        "conversation-child",
        "wake-active-interrupt",
        "claim-active-interrupt",
        "run-active-interrupt",
        "assistant-active-interrupt",
        AgentWakeStatus::Running,
        21,
    );

    let first = interrupt_agent_execution_with_receipt(
        &mut connection,
        "agent-root",
        "agent-child",
        "interrupt-active-request",
        24,
    )
    .unwrap();
    assert_eq!(
        first.outcome,
        InterruptAgentExecutionOutcome::ActiveTurn {
            wake_id: "wake-active-interrupt".to_string(),
            run_id: "run-active-interrupt".to_string(),
        }
    );
    assert_eq!(first.dispatched_at, None);
    assert_eq!(
        list_undispatched_agent_interrupts(&connection).unwrap(),
        vec![UndispatchedAgentInterrupt {
            caller_agent_id: "agent-root".to_string(),
            target_agent_id: "agent-child".to_string(),
            request_id: "interrupt-active-request".to_string(),
            wake_id: "wake-active-interrupt".to_string(),
            run_id: "run-active-interrupt".to_string(),
        }]
    );

    let retry_before_dispatch = interrupt_agent_execution_with_receipt(
        &mut connection,
        "agent-root",
        "agent-child",
        "interrupt-active-request",
        25,
    )
    .unwrap();
    assert_eq!(retry_before_dispatch, first);

    let marked = mark_agent_interrupt_dispatched(
        &mut connection,
        "agent-root",
        "interrupt-active-request",
        26,
    )
    .unwrap();
    assert_eq!(marked.outcome, first.outcome);
    assert_eq!(marked.dispatched_at, Some(26));
    assert!(list_undispatched_agent_interrupts(&connection)
        .unwrap()
        .is_empty());

    let retry_after_dispatch = interrupt_agent_execution_with_receipt(
        &mut connection,
        "agent-root",
        "agent-child",
        "interrupt-active-request",
        27,
    )
    .unwrap();
    assert_eq!(retry_after_dispatch, marked);

    let repeated_mark = mark_agent_interrupt_dispatched(
        &mut connection,
        "agent-root",
        "interrupt-active-request",
        28,
    )
    .unwrap();
    assert_eq!(
        repeated_mark.dispatched_at,
        Some(26),
        "dispatch acknowledgement is a monotonic first-delivery fact"
    );
}

#[test]
fn tree_cancellation_closes_the_claim_admission_boundary_and_is_tree_scoped() {
    let mut connection = setup_tree();
    add_grandchild(&mut connection);
    for conversation in [
        "conversation-sibling",
        "conversation-other-root",
        "conversation-other-child",
    ] {
        insert_conversation(&connection, conversation, Some("project-a"));
    }
    create_agent_node(
        &mut connection,
        &child_input(
            "agent-sibling",
            "agent-root",
            "agent-root",
            "conversation-sibling",
            "sibling",
            "/root/sibling",
        ),
        13,
    )
    .unwrap();
    ensure_root(
        &mut connection,
        "agent-other-root",
        "conversation-other-root",
    );
    create_agent_node(
        &mut connection,
        &child_input(
            "agent-other-child",
            "agent-other-root",
            "agent-other-root",
            "conversation-other-child",
            "other",
            "/root/other",
        ),
        14,
    )
    .unwrap();

    enqueue_agent_wake(
        &mut connection,
        &descendant_wake("wake-child-claimed", "agent-child", "agent-root"),
        20,
    )
    .unwrap();
    enqueue_agent_wake(
        &mut connection,
        &descendant_wake("wake-child-queued", "agent-child", "agent-root"),
        21,
    )
    .unwrap();
    let claimed = claim_next_agent_wake(&mut connection, "agent-child", "claim-child", 22)
        .unwrap()
        .unwrap();
    assert_eq!(claimed.wake_id, "wake-child-claimed");

    enqueue_agent_wake(
        &mut connection,
        &descendant_wake("wake-grand-waiting", "agent-grand", "agent-child"),
        23,
    )
    .unwrap();
    admit_wake(
        &mut connection,
        "agent-grand",
        "conversation-grand",
        "wake-grand-waiting",
        "claim-grand",
        "run-grand",
        "assistant-grand",
        AgentWakeStatus::WaitingForApproval,
        24,
    );

    enqueue_agent_wake(
        &mut connection,
        &descendant_wake("wake-sibling-running", "agent-sibling", "agent-root"),
        27,
    )
    .unwrap();
    admit_wake(
        &mut connection,
        "agent-sibling",
        "conversation-sibling",
        "wake-sibling-running",
        "claim-sibling",
        "run-sibling",
        "assistant-sibling",
        AgentWakeStatus::Running,
        28,
    );

    enqueue_agent_wake(
        &mut connection,
        &EnqueueAgentWakeInput {
            wake_id: "wake-other-tree".to_string(),
            root_agent_id: "agent-other-root".to_string(),
            agent_id: "agent-other-child".to_string(),
            requester_agent_id: "agent-other-root".to_string(),
            request_id: "request-other-tree".to_string(),
            source_agent_message_id: None,
        },
        31,
    )
    .unwrap();

    let batch =
        cancel_agent_tree_wakes_by_root_conversation(&mut connection, "conversation-root", 32)
            .unwrap();
    assert_eq!(batch.root_agent_id, "agent-root");
    assert_eq!(batch.root_conversation_id, "conversation-root");
    assert_eq!(
        batch
            .cancelled_before_admission
            .iter()
            .map(|wake| (wake.wake_id.as_str(), wake.status))
            .collect::<Vec<_>>(),
        vec![
            ("wake-child-claimed", AgentWakeStatus::Cancelled),
            ("wake-child-queued", AgentWakeStatus::Cancelled),
        ]
    );
    assert_eq!(
        batch
            .active_wakes
            .iter()
            .map(|wake| {
                (
                    wake.agent_id.as_str(),
                    wake.conversation_id.as_str(),
                    wake.wake_id.as_str(),
                    wake.run_id.as_str(),
                    wake.status,
                )
            })
            .collect::<Vec<_>>(),
        vec![
            (
                "agent-grand",
                "conversation-grand",
                "wake-grand-waiting",
                "run-grand",
                AgentWakeStatus::WaitingForApproval,
            ),
            (
                "agent-sibling",
                "conversation-sibling",
                "wake-sibling-running",
                "run-sibling",
                AgentWakeStatus::Running,
            ),
        ]
    );
    assert_eq!(
        get_agent_wake(&connection, "wake-other-tree")
            .unwrap()
            .unwrap()
            .status,
        AgentWakeStatus::Queued,
        "a root-keyed sweep must not touch an adjacent Agent tree"
    );

    let fixed_point =
        cancel_agent_tree_wakes_by_root_agent(&mut connection, "agent-root", 33).unwrap();
    assert!(fixed_point.cancelled_before_admission.is_empty());
    assert_eq!(fixed_point.active_wakes, batch.active_wakes);

    let followup = follow_up_agent(
        &mut connection,
        &SendAgentMessageRequest {
            sender_agent_id: "agent-root".to_string(),
            recipient_agent_id: "agent-child".to_string(),
            request_id: "followup-after-tree-cancel".to_string(),
            content: "a future explicit turn may run".to_string(),
        },
        34,
    )
    .unwrap();
    assert_eq!(
        followup.deferred_wake.unwrap().status,
        AgentWakeStatus::Queued,
        "tree cancellation must not disable reusable Agent identities"
    );
}

#[test]
fn tree_cancellation_rolls_back_all_pending_wakes_when_one_update_fails() {
    let mut connection = setup_tree();
    insert_active_trace(
        &mut connection,
        "conversation-root",
        "run-root-rollback",
        "assistant-root-rollback",
        19,
    );
    enqueue_agent_wake(&mut connection, &wake_input("rollback-one"), 20).unwrap();
    enqueue_agent_wake(&mut connection, &wake_input("rollback-two"), 21).unwrap();
    connection
        .execute_batch(
            "CREATE TEMP TRIGGER fail_second_tree_cancellation
             BEFORE UPDATE OF status ON agent_wake_requests
             WHEN OLD.wake_id = 'wake-rollback-two' AND NEW.status = 'cancelled'
             BEGIN
                 SELECT RAISE(ABORT, 'injected tree cancellation failure');
             END;",
        )
        .unwrap();

    assert!(begin_agent_tree_run_stop_by_root_agent(
        &mut connection,
        "agent-root",
        "run-root-rollback",
        22,
    )
    .is_err());
    assert!(get_agent_tree_run_stop(&connection, "run-root-rollback")
        .unwrap()
        .is_none());
    for wake_id in ["wake-rollback-one", "wake-rollback-two"] {
        assert_eq!(
            get_agent_wake(&connection, wake_id)
                .unwrap()
                .unwrap()
                .status,
            AgentWakeStatus::Queued
        );
    }
}

#[test]
fn interrupted_grandchild_reports_result_without_reawakening_its_active_parent() {
    let mut connection = setup_tree();
    add_grandchild(&mut connection);
    enqueue_agent_wake(&mut connection, &wake_input("active-parent"), 20).unwrap();
    admit_wake(
        &mut connection,
        "agent-child",
        "conversation-child",
        "wake-active-parent",
        "claim-parent",
        "run-parent",
        "assistant-parent",
        AgentWakeStatus::Running,
        21,
    );
    enqueue_agent_wake(
        &mut connection,
        &descendant_wake("wake-interrupted-grand", "agent-grand", "agent-child"),
        24,
    )
    .unwrap();
    admit_wake(
        &mut connection,
        "agent-grand",
        "conversation-grand",
        "wake-interrupted-grand",
        "claim-interrupted-grand",
        "run-interrupted-grand",
        "assistant-interrupted-grand",
        AgentWakeStatus::Running,
        25,
    );

    let settled = finish_agent_turn_with_result(
        &mut connection,
        &FinishAgentTurnResultInput {
            wake_id: "wake-interrupted-grand".to_string(),
            expected_status: AgentWakeStatus::Running,
            claim_token: "claim-interrupted-grand".to_string(),
            terminal_status: AgentWakeStatus::Interrupted,
            run_id: Some("run-interrupted-grand".to_string()),
            assistant_message_id: Some("assistant-interrupted-grand".to_string()),
            terminal_error: Some("cancelled".to_string()),
        },
        28,
    )
    .unwrap();
    assert!(settled.parent_wake.is_none());
    assert_eq!(settled.result_message.recipient_agent_id, "agent-child");
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM agent_wake_requests
                 WHERE agent_id = 'agent-child' AND requester_agent_id = 'agent-grand'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        0,
        "an interrupted descendant must not enqueue executable work on its parent"
    );

    let retry = finish_agent_turn_with_result(
        &mut connection,
        &FinishAgentTurnResultInput {
            wake_id: "wake-interrupted-grand".to_string(),
            expected_status: AgentWakeStatus::Running,
            claim_token: "claim-interrupted-grand".to_string(),
            terminal_status: AgentWakeStatus::Interrupted,
            run_id: Some("run-interrupted-grand".to_string()),
            assistant_message_id: Some("assistant-interrupted-grand".to_string()),
            terminal_error: Some("cancelled".to_string()),
        },
        29,
    )
    .unwrap();
    assert!(retry.parent_wake.is_none());
    assert_eq!(retry.result_message, settled.result_message);

    let result = claim_next_agent_message(&mut connection, "agent-child", "wait-result", 30)
        .unwrap()
        .unwrap();
    assert_eq!(result.message_id, settled.result_message.message_id);
    assert_eq!(result.kind, AgentMailboxKind::Result);
}

#[test]
fn durable_tree_stop_covers_exact_runs_and_serializes_followup_both_ways() {
    let mut connection = setup_tree();
    insert_active_trace(
        &mut connection,
        "conversation-root",
        "run-root-stop",
        "assistant-root-stop",
        20,
    );

    let before_stop = follow_up_agent_from_run(
        &mut connection,
        &SendAgentMessageRequest {
            sender_agent_id: "agent-root".to_string(),
            recipient_agent_id: "agent-child".to_string(),
            request_id: "followup-before-stop".to_string(),
            content: "this commit wins the SQLite order".to_string(),
        },
        "run-root-stop",
        21,
    )
    .unwrap();
    let before_stop_wake = before_stop.deferred_wake.unwrap();
    let direct_before_stop = send_agent_message_from_run(
        &mut connection,
        &SendAgentMessageRequest {
            sender_agent_id: "agent-root".to_string(),
            recipient_agent_id: "agent-child".to_string(),
            request_id: "message-before-stop".to_string(),
            content: "this direct message committed before the stop".to_string(),
        },
        "run-root-stop",
        21,
    )
    .unwrap();
    assert!(direct_before_stop.deferred_wake.is_none());

    let stopped = begin_agent_tree_run_stop_by_root_conversation(
        &mut connection,
        "conversation-root",
        "run-root-stop",
        22,
    )
    .unwrap()
    .unwrap();
    assert_eq!(stopped.stop.run_id, "run-root-stop");
    assert_eq!(stopped.stop.root_run_id, "run-root-stop");
    assert_eq!(
        get_agent_wake(&connection, &before_stop_wake.wake_id)
            .unwrap()
            .unwrap()
            .status,
        AgentWakeStatus::Cancelled,
        "a scheduling commit ordered before stop must be cancelled by its atomic sweep"
    );
    assert_eq!(
        get_agent_tree_run_stop(&connection, "run-root-stop").unwrap(),
        Some(stopped.stop.clone())
    );
    assert!(get_agent_tree_run_stop(&connection, "run-future")
        .unwrap()
        .is_none());

    let messages_before = connection
        .query_row("SELECT COUNT(*) FROM agent_mailbox_messages", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap();
    let wakes_before = connection
        .query_row("SELECT COUNT(*) FROM agent_wake_requests", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap();
    let after_stop = follow_up_agent_from_run(
        &mut connection,
        &SendAgentMessageRequest {
            sender_agent_id: "agent-root".to_string(),
            recipient_agent_id: "agent-child".to_string(),
            request_id: "followup-after-stop".to_string(),
            content: "this entire write must be rejected".to_string(),
        },
        "run-root-stop",
        23,
    )
    .unwrap_err();
    assert_eq!(
        after_stop,
        AgentGraphError::Conflict("origin Agent Run has been stopped".to_string())
    );
    let direct_after_stop = send_agent_message_from_run(
        &mut connection,
        &SendAgentMessageRequest {
            sender_agent_id: "agent-root".to_string(),
            recipient_agent_id: "agent-child".to_string(),
            request_id: "message-after-stop".to_string(),
            content: "this direct message must be rejected".to_string(),
        },
        "run-root-stop",
        23,
    )
    .unwrap_err();
    assert_eq!(
        direct_after_stop,
        AgentGraphError::Conflict("origin Agent Run has been stopped".to_string())
    );
    assert_eq!(
        connection
            .query_row("SELECT COUNT(*) FROM agent_mailbox_messages", [], |row| row
                .get::<_, i64>(0),)
            .unwrap(),
        messages_before,
        "a stop ordered before scheduling must reject both message and Wake"
    );
    assert_eq!(
        connection
            .query_row("SELECT COUNT(*) FROM agent_wake_requests", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
        wakes_before
    );

    connection
        .execute(
            "UPDATE conversation_turn_traces
             SET terminal_status = 'cancelled', completed_at = 24, updated_at = 24
             WHERE run_id = 'run-root-stop'",
            [],
        )
        .unwrap();
    insert_active_trace(
        &mut connection,
        "conversation-root",
        "run-future",
        "assistant-future",
        25,
    );
    let future = follow_up_agent_from_run(
        &mut connection,
        &SendAgentMessageRequest {
            sender_agent_id: "agent-root".to_string(),
            recipient_agent_id: "agent-child".to_string(),
            request_id: "followup-future-run".to_string(),
            content: "a later explicit Turn remains enabled".to_string(),
        },
        "run-future",
        26,
    )
    .unwrap();
    assert_eq!(
        future.deferred_wake.unwrap().status,
        AgentWakeStatus::Queued,
        "the durable fence is scoped to exact Run IDs, not the Agent identity"
    );
}

#[test]
fn durable_tree_stop_blocks_pending_action_dispatch_after_its_commit() {
    let mut connection = setup_tree();
    insert_active_trace(
        &mut connection,
        "conversation-root",
        "run-root-approval-stop",
        "assistant-root-approval-stop",
        20,
    );
    enqueue_agent_wake(
        &mut connection,
        &descendant_wake("wake-approval-stop", "agent-child", "agent-root"),
        21,
    )
    .unwrap();
    admit_wake(
        &mut connection,
        "agent-child",
        "conversation-child",
        "wake-approval-stop",
        "claim-approval-stop",
        "run-approval-stop",
        "assistant-approval-stop",
        AgentWakeStatus::WaitingForApproval,
        22,
    );
    connection
        .execute(
            "INSERT INTO agent_pending_actions (
                 action_id, run_id, conversation_id, assistant_message_id, action_type,
                 tool_name, tool_call_id, status, target_status, action_json,
                 agent_input_json, created_at, updated_at
             ) VALUES (
                 'pending-approval-stop', 'run-approval-stop', 'conversation-child',
                 'assistant-approval-stop', 'command', 'shell', 'call-approval-stop',
                 'pending', NULL, '{}', '{}', 24, 24
             )",
            [],
        )
        .unwrap();
    begin_agent_tree_run_stop_by_root_agent(
        &mut connection,
        "agent-root",
        "run-root-approval-stop",
        25,
    )
    .unwrap()
    .unwrap();
    let changed =
        crate::storage::pending_action_repository::transition_pending_action_for_dispatch(
            &connection,
            "pending-approval-stop",
            "pending",
            "executing",
            "{}",
            26,
        )
        .unwrap();
    assert_eq!(
        changed, 0,
        "a stopped Run cannot acquire dispatch authority"
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT status FROM agent_pending_actions
                 WHERE action_id = 'pending-approval-stop'",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
        "pending"
    );
    let insert_error = connection
        .execute(
            "INSERT INTO agent_pending_actions (
                 action_id, run_id, conversation_id, assistant_message_id, action_type,
                 tool_name, tool_call_id, status, target_status, action_json,
                 agent_input_json, created_at, updated_at
             ) VALUES (
                 'automatic-approval-after-stop', 'run-approval-stop', 'conversation-child',
                 'assistant-approval-stop', 'file_change', 'apply_patch',
                 'call-automatic-after-stop', 'approved', NULL, '{}', '{}', 27, 27
             )",
            [],
        )
        .unwrap_err();
    assert!(insert_error
        .to_string()
        .contains("stopped Agent Run cannot insert dispatch authority"));

    let changed = crate::storage::pending_action_repository::transition_pending_action(
        &connection,
        "pending-approval-stop",
        "pending",
        "executing",
        "{}",
        28,
    )
    .unwrap();
    assert_eq!(
        changed, 1,
        "the unguarded status CAS remains available for cancellation arbitration"
    );
}

#[test]
fn covered_completed_grandchild_reports_result_without_creating_parent_wake() {
    let mut connection = setup_tree();
    add_grandchild(&mut connection);
    insert_active_trace(
        &mut connection,
        "conversation-root",
        "run-root-covered-result",
        "assistant-root-covered-result",
        20,
    );
    enqueue_agent_wake(
        &mut connection,
        &descendant_wake("wake-covered-grand", "agent-grand", "agent-child"),
        21,
    )
    .unwrap();
    admit_wake(
        &mut connection,
        "agent-grand",
        "conversation-grand",
        "wake-covered-grand",
        "claim-covered-grand",
        "run-covered-grand",
        "assistant-covered-grand",
        AgentWakeStatus::Running,
        22,
    );

    let stop = begin_agent_tree_run_stop_by_root_agent(
        &mut connection,
        "agent-root",
        "run-root-covered-result",
        25,
    )
    .unwrap()
    .unwrap();
    assert_eq!(stop.cancellation.active_wakes.len(), 1);
    assert_eq!(
        get_agent_tree_run_stop(&connection, "run-covered-grand")
            .unwrap()
            .unwrap()
            .root_run_id,
        "run-root-covered-result"
    );
    connection
        .execute(
            "UPDATE conversation_turn_traces
             SET terminal_status = 'completed', completed_at = 26, updated_at = 26
             WHERE run_id = 'run-covered-grand'",
            [],
        )
        .unwrap();
    connection
        .execute(
            "UPDATE messages SET status = 'sent'
             WHERE id = 'assistant-covered-grand'",
            [],
        )
        .unwrap();
    connection
        .execute(
            "DELETE FROM messages WHERE id = 'assistant-root-covered-result'",
            [],
        )
        .unwrap();
    assert!(connection
        .query_row(
            "SELECT 1 FROM conversation_turn_traces
             WHERE run_id = 'run-root-covered-result'",
            [],
            |_| Ok(()),
        )
        .optional()
        .unwrap()
        .is_none());
    assert!(
        get_agent_tree_run_stop(&connection, "run-covered-grand")
            .unwrap()
            .is_some(),
        "deleting one stopped root message must not erase a still-converging descendant fence"
    );

    let recovered =
        recover_agent_wakes(&mut connection, "covered-completed-restart", 100_000).unwrap();
    let AgentWakeRecoveryAction::Observe(rebound) = &recovered.actions[0] else {
        panic!("a fenced Run with a real terminal trace must preserve that outcome");
    };
    let finish = FinishAgentTurnResultInput {
        wake_id: rebound.wake_id.clone(),
        expected_status: rebound.status,
        claim_token: rebound.claim_token.clone().unwrap(),
        terminal_status: AgentWakeStatus::Completed,
        run_id: rebound.run_id.clone(),
        assistant_message_id: rebound.assistant_message_id.clone(),
        terminal_error: None,
    };
    let settled = finish_agent_turn_with_result(&mut connection, &finish, 100_001).unwrap();
    assert!(settled.parent_wake.is_none());
    assert_eq!(settled.result_message.recipient_agent_id, "agent-child");
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM agent_wake_requests
                 WHERE requester_agent_id = 'agent-grand' AND agent_id = 'agent-child'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        0,
        "a covered late settlement must remain notification-only"
    );

    let replay = finish_agent_turn_with_result(&mut connection, &finish, 100_002).unwrap();
    assert!(replay.parent_wake.is_none());
    assert_eq!(replay.result_message, settled.result_message);
}

#[test]
fn later_root_stop_reuses_an_older_descendant_fence_without_rolling_back() {
    let mut connection = setup_tree();
    add_grandchild(&mut connection);
    insert_conversation(&connection, "conversation-sibling", Some("project-a"));
    create_agent_node(
        &mut connection,
        &child_input(
            "agent-sibling",
            "agent-root",
            "agent-root",
            "conversation-sibling",
            "sibling",
            "/root/sibling",
        ),
        13,
    )
    .unwrap();
    insert_active_trace(
        &mut connection,
        "conversation-root",
        "run-root-first-stop",
        "assistant-root-first-stop",
        20,
    );
    enqueue_agent_wake(
        &mut connection,
        &descendant_wake("wake-old-child", "agent-child", "agent-root"),
        21,
    )
    .unwrap();
    admit_wake(
        &mut connection,
        "agent-child",
        "conversation-child",
        "wake-old-child",
        "claim-old-child",
        "run-old-child",
        "assistant-old-child",
        AgentWakeStatus::Running,
        22,
    );
    begin_agent_tree_run_stop_by_root_agent(
        &mut connection,
        "agent-root",
        "run-root-first-stop",
        25,
    )
    .unwrap()
    .unwrap();

    connection
        .execute(
            "UPDATE conversation_turn_traces
             SET terminal_status = 'cancelled', completed_at = 26, updated_at = 26
             WHERE run_id = 'run-root-first-stop'",
            [],
        )
        .unwrap();
    insert_active_trace(
        &mut connection,
        "conversation-root",
        "run-root-second-stop",
        "assistant-root-second-stop",
        27,
    );
    enqueue_agent_wake(
        &mut connection,
        &descendant_wake("wake-new-grand", "agent-grand", "agent-child"),
        28,
    )
    .unwrap();
    admit_wake(
        &mut connection,
        "agent-grand",
        "conversation-grand",
        "wake-new-grand",
        "claim-new-grand",
        "run-new-grand",
        "assistant-new-grand",
        AgentWakeStatus::Running,
        29,
    );

    let retried_first = reinforce_agent_tree_run_stop(&mut connection, "run-root-first-stop", 31)
        .unwrap()
        .unwrap();
    assert_eq!(retried_first.cancellation.active_wakes.len(), 1);
    assert_eq!(
        retried_first.cancellation.active_wakes[0].run_id,
        "run-old-child"
    );
    assert_eq!(
        get_agent_wake(&connection, "wake-new-grand")
            .unwrap()
            .unwrap()
            .status,
        AgentWakeStatus::Running,
        "retrying an older stop must not cancel a later explicit Turn"
    );
    assert!(get_agent_tree_run_stop(&connection, "run-new-grand")
        .unwrap()
        .is_none());

    let second = begin_agent_tree_run_stop_by_root_agent(
        &mut connection,
        "agent-root",
        "run-root-second-stop",
        32,
    )
    .expect("the later root stop transaction must succeed")
    .expect("the later root Run is active");
    assert_eq!(second.cancellation.active_wakes.len(), 2);
    assert_eq!(
        get_agent_tree_run_stop(&connection, "run-old-child")
            .unwrap()
            .unwrap()
            .root_run_id,
        "run-root-first-stop",
        "the immutable first stop fact remains authoritative for the old child"
    );
    assert_eq!(
        get_agent_tree_run_stop(&connection, "run-new-grand")
            .unwrap()
            .unwrap()
            .root_run_id,
        "run-root-second-stop"
    );
    assert_eq!(
        get_agent_tree_run_stop(&connection, "run-root-second-stop")
            .unwrap()
            .unwrap()
            .root_run_id,
        "run-root-second-stop"
    );

    let retried_second = reinforce_agent_tree_run_stop(&mut connection, "run-root-second-stop", 33)
        .unwrap()
        .unwrap();
    let mut retried_run_ids = retried_second
        .cancellation
        .active_wakes
        .iter()
        .map(|wake| wake.run_id.as_str())
        .collect::<Vec<_>>();
    retried_run_ids.sort_unstable();
    assert_eq!(retried_run_ids, ["run-new-grand", "run-old-child"]);

    connection
        .execute(
            "UPDATE conversation_turn_traces
             SET terminal_status = 'cancelled', completed_at = 34, updated_at = 34
             WHERE run_id = 'run-root-second-stop'",
            [],
        )
        .unwrap();
    insert_active_trace(
        &mut connection,
        "conversation-root",
        "run-root-third-stop",
        "assistant-root-third-stop",
        35,
    );
    enqueue_agent_wake(
        &mut connection,
        &descendant_wake("wake-third-sibling", "agent-sibling", "agent-root"),
        36,
    )
    .unwrap();
    admit_wake(
        &mut connection,
        "agent-sibling",
        "conversation-sibling",
        "wake-third-sibling",
        "claim-third-sibling",
        "run-third-sibling",
        "assistant-third-sibling",
        AgentWakeStatus::Running,
        37,
    );
    begin_agent_tree_run_stop_by_root_agent(
        &mut connection,
        "agent-root",
        "run-root-third-stop",
        40,
    )
    .unwrap()
    .unwrap();

    let second_after_third =
        reinforce_agent_tree_run_stop(&mut connection, "run-root-second-stop", 41)
            .unwrap()
            .unwrap();
    let mut second_run_ids = second_after_third
        .cancellation
        .active_wakes
        .iter()
        .map(|wake| wake.run_id.as_str())
        .collect::<Vec<_>>();
    second_run_ids.sort_unstable();
    assert_eq!(second_run_ids, ["run-new-grand", "run-old-child"]);
    assert!(second_after_third
        .cancellation
        .active_wakes
        .iter()
        .all(|wake| wake.run_id != "run-third-sibling"));
}

#[test]
fn stale_root_stop_cannot_capture_a_later_active_turn() {
    let mut connection = setup_tree();
    insert_active_trace(
        &mut connection,
        "conversation-root",
        "run-root-stale",
        "assistant-root-stale",
        20,
    );
    connection
        .execute(
            "UPDATE conversation_turn_traces
             SET terminal_status = 'completed', completed_at = 21, updated_at = 21
             WHERE run_id = 'run-root-stale'",
            [],
        )
        .unwrap();
    insert_active_trace(
        &mut connection,
        "conversation-root",
        "run-root-current",
        "assistant-root-current",
        22,
    );
    enqueue_agent_wake(
        &mut connection,
        &descendant_wake("wake-current", "agent-child", "agent-root"),
        23,
    )
    .unwrap();

    let stopped = begin_agent_tree_run_stop_by_root_agent(
        &mut connection,
        "agent-root",
        "run-root-stale",
        24,
    )
    .unwrap();
    assert!(stopped.is_none());
    assert!(get_agent_tree_run_stop(&connection, "run-root-stale")
        .unwrap()
        .is_none());
    assert_eq!(
        get_agent_wake(&connection, "wake-current")
            .unwrap()
            .unwrap()
            .status,
        AgentWakeStatus::Queued,
        "a delayed cancel request for an older root Run must not stop later work"
    );
}

#[test]
fn reinforcement_does_not_expand_into_a_later_uncovered_run() {
    let mut connection = setup_tree();
    insert_active_trace(
        &mut connection,
        "conversation-root",
        "run-root-reinforce",
        "assistant-root-reinforce",
        20,
    );
    begin_agent_tree_run_stop_by_root_agent(
        &mut connection,
        "agent-root",
        "run-root-reinforce",
        21,
    )
    .unwrap()
    .unwrap();

    // This low-level insertion models a later explicit Turn. Production run-bound collaboration
    // tools carry that Turn's own run identity, so retrying an older stop must not absorb it.
    enqueue_agent_wake(&mut connection, &wake_input("late-admitted"), 22).unwrap();
    admit_wake(
        &mut connection,
        "agent-child",
        "conversation-child",
        "wake-late-admitted",
        "claim-late-admitted",
        "run-late-admitted",
        "assistant-late-admitted",
        AgentWakeStatus::Running,
        23,
    );

    let reinforced = reinforce_agent_tree_run_stop(&mut connection, "run-root-reinforce", 26)
        .unwrap()
        .unwrap();
    assert!(reinforced.cancellation.active_wakes.is_empty());
    assert!(get_agent_tree_run_stop(&connection, "run-late-admitted")
        .unwrap()
        .is_none());
    assert!(
        reinforce_agent_tree_run_stop(&mut connection, "run-late-admitted", 27)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        get_agent_wake(&connection, "wake-late-admitted")
            .unwrap()
            .unwrap()
            .status,
        AgentWakeStatus::Running
    );
}

#[test]
fn restart_recovers_a_fenced_waiting_turn_as_interrupted_without_parent_wake() {
    let mut connection = setup_tree();
    insert_active_trace(
        &mut connection,
        "conversation-root",
        "run-root-restart-stop",
        "assistant-root-restart-stop",
        20,
    );
    enqueue_agent_wake(
        &mut connection,
        &descendant_wake("wake-restart-stopped", "agent-child", "agent-root"),
        21,
    )
    .unwrap();
    admit_wake(
        &mut connection,
        "agent-child",
        "conversation-child",
        "wake-restart-stopped",
        "claim-restart-stopped",
        "run-restart-stopped",
        "assistant-restart-stopped",
        AgentWakeStatus::WaitingForApproval,
        22,
    );
    begin_agent_tree_run_stop_by_root_agent(
        &mut connection,
        "agent-root",
        "run-root-restart-stop",
        25,
    )
    .unwrap()
    .unwrap();

    let recovered = recover_agent_wakes(&mut connection, "restart-stop", 100_000).unwrap();
    assert_eq!(recovered.actions.len(), 1);
    let AgentWakeRecoveryAction::TreeStopped(rebound) = &recovered.actions[0] else {
        panic!("a fenced approval must not be restored for observation");
    };
    let settlement = finish_agent_turn_with_result(
        &mut connection,
        &FinishAgentTurnResultInput {
            wake_id: rebound.wake_id.clone(),
            expected_status: rebound.status,
            claim_token: rebound.claim_token.clone().unwrap(),
            terminal_status: AgentWakeStatus::Interrupted,
            run_id: rebound.run_id.clone(),
            assistant_message_id: rebound.assistant_message_id.clone(),
            terminal_error: Some("root Agent tree was stopped".to_string()),
        },
        100_001,
    )
    .unwrap();

    assert!(settlement.parent_wake.is_none());
    assert_eq!(settlement.wake.status, AgentWakeStatus::Interrupted);
    assert_eq!(
        connection
            .query_row(
                "SELECT terminal_status FROM conversation_turn_traces
                 WHERE run_id = 'run-restart-stopped'",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
        "cancelled"
    );
}

#[test]
fn stopped_active_wake_without_runtime_owner_is_atomically_interrupted() {
    let mut connection = setup_tree();
    insert_active_trace(
        &mut connection,
        "conversation-root",
        "run-root-ownerless-stop",
        "assistant-root-ownerless-stop",
        20,
    );
    enqueue_agent_wake(
        &mut connection,
        &descendant_wake("wake-ownerless-stop", "agent-child", "agent-root"),
        21,
    )
    .unwrap();
    admit_wake(
        &mut connection,
        "agent-child",
        "conversation-child",
        "wake-ownerless-stop",
        "claim-ownerless-stop",
        "run-ownerless-stop",
        "assistant-ownerless-stop",
        AgentWakeStatus::Running,
        22,
    );
    connection
        .execute(
            "INSERT INTO agent_pending_actions (
                 action_id, run_id, conversation_id, assistant_message_id, action_type,
                 tool_name, tool_call_id, status, target_status, action_json,
                 agent_input_json, created_at, updated_at
             ) VALUES (
                 'settled-grant-owner', 'run-ownerless-stop', 'conversation-child',
                 'assistant-ownerless-stop', 'file_change', 'apply_patch', 'call-grant-owner',
                 'completed', 'completed', '{}', '{}', 23, 23
             )",
            [],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO agent_file_change_run_grants (
                 schema_version, grant_id, revision, status, run_id, conversation_id,
                 project_id, scope_kind, workspace_identity, canonical_scope_path,
                 scope_directory_identity_json, granting_pending_action_id,
                 base_write_permission, granting_permission_revision,
                 granting_tool_set_revision, granting_provider_wire_revision,
                 apply_patch_contract_revision, activation_result_digest, created_at,
                 activated_at, inactive_at, revoked_at
             ) VALUES (
                 1, 'grant-ownerless-stop', 1, 'active', 'run-ownerless-stop',
                 'conversation-child', 'project-a', 'workspace', 'workspace-a', '/tmp/workspace-a',
                 '{}', 'settled-grant-owner', 'workspace_only', 'permission-v1', 'tools-v1',
                 'provider-v1', 'apply-patch-file-change-v1',
                 'file-change-sha256-v1:0000000000000000000000000000000000000000000000000000000000000000',
                 23, 23, NULL, NULL
             )",
            [],
        )
        .unwrap();
    let stopped = begin_agent_tree_run_stop_by_root_agent(
        &mut connection,
        "agent-root",
        "run-root-ownerless-stop",
        25,
    )
    .unwrap()
    .unwrap();
    let frozen = stopped.cancellation.active_wakes.first().unwrap();

    let outcome = settle_tree_stopped_active_wake(&mut connection, frozen, 100_000).unwrap();
    let AgentTreeStoppedWakeSettlementOutcome::Interrupted(settlement) = outcome else {
        panic!("ownerless stopped Wake should be settled immediately");
    };
    assert_eq!(settlement.wake.status, AgentWakeStatus::Interrupted);
    assert!(settlement.parent_wake.is_none());
    assert_eq!(
        settlement.envelope.run_id.as_deref(),
        Some("run-ownerless-stop")
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT terminal_status FROM conversation_turn_traces
                 WHERE run_id = 'run-ownerless-stop'",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
        "cancelled"
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT status FROM agent_file_change_run_grants
                 WHERE grant_id = 'grant-ownerless-stop'",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
        "revoked",
        "terminalizing the stopped Run must revoke remembered write authority"
    );
}

#[test]
fn stopped_active_wake_fallback_refuses_unsettled_external_action() {
    let mut connection = setup_tree();
    insert_active_trace(
        &mut connection,
        "conversation-root",
        "run-root-unsafe-stop",
        "assistant-root-unsafe-stop",
        20,
    );
    enqueue_agent_wake(
        &mut connection,
        &descendant_wake("wake-unsafe-stop", "agent-child", "agent-root"),
        21,
    )
    .unwrap();
    admit_wake(
        &mut connection,
        "agent-child",
        "conversation-child",
        "wake-unsafe-stop",
        "claim-unsafe-stop",
        "run-unsafe-stop",
        "assistant-unsafe-stop",
        AgentWakeStatus::WaitingForApproval,
        22,
    );
    connection
        .execute(
            "INSERT INTO agent_pending_actions (
                 action_id, run_id, conversation_id, assistant_message_id, action_type,
                 tool_name, tool_call_id, status, target_status, action_json,
                 agent_input_json, created_at, updated_at
             ) VALUES (
                 'pending-unsafe-stop', 'run-unsafe-stop', NULL,
                 NULL, 'command', 'shell', 'call-unsafe-stop',
                 'pending', NULL, '{}', '{}', 24, 24
             )",
            [],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO agent_action_audit (
                 action_id, run_id, conversation_id, assistant_message_id,
                 action_type, tool_name, decision, status, action_json, created_at
             ) VALUES (
                 'audit-unsafe-stop', 'run-unsafe-stop', NULL, NULL,
                 'command', 'shell', 'approved', 'executing', '{}', 24
             )",
            [],
        )
        .unwrap();
    let stopped = begin_agent_tree_run_stop_by_root_agent(
        &mut connection,
        "agent-root",
        "run-root-unsafe-stop",
        25,
    )
    .unwrap()
    .unwrap();
    let frozen = stopped.cancellation.active_wakes.first().unwrap();

    assert_eq!(
        settle_tree_stopped_active_wake(&mut connection, frozen, 26).unwrap(),
        AgentTreeStoppedWakeSettlementOutcome::UnsafeActiveAction { count: 2 }
    );
    assert_eq!(
        get_agent_wake(&connection, "wake-unsafe-stop")
            .unwrap()
            .unwrap()
            .status,
        AgentWakeStatus::WaitingForApproval
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT terminal_status FROM conversation_turn_traces
                 WHERE run_id = 'run-unsafe-stop'",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
        "in_progress"
    );
}

#[test]
fn stopped_active_wake_fallback_preserves_a_terminal_trace_race() {
    let mut connection = setup_tree();
    insert_active_trace(
        &mut connection,
        "conversation-root",
        "run-root-terminal-race",
        "assistant-root-terminal-race",
        20,
    );
    enqueue_agent_wake(
        &mut connection,
        &descendant_wake("wake-terminal-race", "agent-child", "agent-root"),
        21,
    )
    .unwrap();
    admit_wake(
        &mut connection,
        "agent-child",
        "conversation-child",
        "wake-terminal-race",
        "claim-terminal-race",
        "run-terminal-race",
        "assistant-terminal-race",
        AgentWakeStatus::Running,
        22,
    );
    let stopped = begin_agent_tree_run_stop_by_root_agent(
        &mut connection,
        "agent-root",
        "run-root-terminal-race",
        25,
    )
    .unwrap()
    .unwrap();
    connection
        .execute(
            "UPDATE conversation_turn_traces
             SET terminal_status = 'completed', completed_at = 26, updated_at = 26
             WHERE run_id = 'run-terminal-race'",
            [],
        )
        .unwrap();
    let frozen = stopped.cancellation.active_wakes.first().unwrap();

    assert_eq!(
        settle_tree_stopped_active_wake(&mut connection, frozen, 27).unwrap(),
        AgentTreeStoppedWakeSettlementOutcome::AlreadyTerminal
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT terminal_status FROM conversation_turn_traces
                 WHERE run_id = 'run-terminal-race'",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
        "completed",
        "the stop fallback must never overwrite a real terminal result"
    );
}
