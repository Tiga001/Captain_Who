use super::*;

#[test]
fn application_send_and_followup_are_durable_idempotent_and_tree_authorized() {
    let mut connection = setup_tree();
    add_grandchild(&mut connection);

    let send = SendAgentMessageRequest {
        sender_agent_id: "agent-grand".to_string(),
        recipient_agent_id: "agent-root".to_string(),
        request_id: "send-only-1".to_string(),
        content: "status update only".to_string(),
    };
    let sent = send_agent_message(&mut connection, &send, 20).unwrap();
    assert!(sent.deferred_wake.is_none());
    assert_eq!(
        sent.message.delivery_status,
        AgentMailboxDeliveryStatus::Queued
    );
    assert_eq!(
        send_agent_message(&mut connection, &send, 21).unwrap(),
        sent
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM agent_wake_requests
                     WHERE source_agent_message_id = ?1",
                [&sent.message.message_id],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        0
    );

    let followup = SendAgentMessageRequest {
        sender_agent_id: "agent-root".to_string(),
        recipient_agent_id: "agent-grand".to_string(),
        request_id: "followup-1".to_string(),
        content: "please check one more edge".to_string(),
    };
    let followed = follow_up_agent(&mut connection, &followup, 22).unwrap();
    assert_eq!(followed.message.kind, AgentMailboxKind::Followup);
    assert_eq!(
        followed.message.delivery_status,
        AgentMailboxDeliveryStatus::Queued
    );
    assert_eq!(
        followed.deferred_wake.as_ref().unwrap().status,
        AgentWakeStatus::Queued
    );
    assert_eq!(
        follow_up_agent(&mut connection, &followup, 23).unwrap(),
        followed
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM messages WHERE source_agent_message_id = ?1",
                [&followed.message.message_id],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        0,
        "follow-up is not projected before dispatcher/safe-boundary delivery"
    );

    let upward = SendAgentMessageRequest {
        sender_agent_id: "agent-grand".to_string(),
        recipient_agent_id: "agent-child".to_string(),
        request_id: "illegal-followup-upward".to_string(),
        content: "not a management follow-up".to_string(),
    };
    assert!(matches!(
        follow_up_agent(&mut connection, &upward, 24),
        Err(AgentGraphError::Conflict(_))
    ));
    let blank = SendAgentMessageRequest {
        sender_agent_id: "agent-root".to_string(),
        recipient_agent_id: "agent-child".to_string(),
        request_id: "blank-message".to_string(),
        content: "   ".to_string(),
    };
    assert!(matches!(
        send_agent_message(&mut connection, &blank, 24),
        Err(AgentGraphError::InvalidInput {
            field: "content",
            ..
        })
    ));

    insert_conversation(&connection, "conversation-wild", Some("project-a"));
    create_agent_node(
        &mut connection,
        &child_input(
            "agent-wild",
            "agent-root",
            "agent-root",
            "conversation-wild",
            "%_wild",
            "/root/%_wild",
        ),
        25,
    )
    .unwrap();
    let wildcard_sibling = SendAgentMessageRequest {
        sender_agent_id: "agent-wild".to_string(),
        recipient_agent_id: "agent-grand".to_string(),
        request_id: "wildcard-must-not-authorize".to_string(),
        content: "must remain a sibling".to_string(),
    };
    assert!(matches!(
        follow_up_agent(&mut connection, &wildcard_sibling, 26),
        Err(AgentGraphError::Conflict(_))
    ));
    let forged_followup = EnqueueAgentMessageInput {
        message_id: "message-wildcard-forged".to_string(),
        root_agent_id: "agent-root".to_string(),
        sender_agent_id: "agent-wild".to_string(),
        recipient_agent_id: "agent-grand".to_string(),
        request_id: "wildcard-forged-low-level".to_string(),
        kind: AgentMailboxKind::Followup,
        content: "must also fail at the canonical DB boundary".to_string(),
        projection_message_id: "projection-wildcard-forged".to_string(),
    };
    assert!(
        enqueue_agent_message(&mut connection, &forged_followup, 27).is_err(),
        "the canonical trigger must not treat `%`/`_` task names as path wildcards"
    );
    let forged_result = EnqueueAgentMessageInput {
        message_id: "message-forged-result".to_string(),
        root_agent_id: "agent-root".to_string(),
        sender_agent_id: "agent-root".to_string(),
        recipient_agent_id: "agent-child".to_string(),
        request_id: "forged-result-low-level".to_string(),
        kind: AgentMailboxKind::Result,
        content: "a parent cannot forge its child's result".to_string(),
        projection_message_id: "projection-forged-result".to_string(),
    };
    assert!(enqueue_agent_message(&mut connection, &forged_result, 28).is_err());
}

#[test]
fn followup_and_dispatch_projection_faults_leave_one_recoverable_fifo_fact() {
    let mut connection = setup_tree();
    let followup = SendAgentMessageRequest {
        sender_agent_id: "agent-root".to_string(),
        recipient_agent_id: "agent-child".to_string(),
        request_id: "faulted-followup".to_string(),
        content: "deliver exactly once after recovery".to_string(),
    };
    connection
        .execute_batch(
            "CREATE TEMP TRIGGER fail_followup_wake
                 BEFORE INSERT ON agent_wake_requests
                 WHEN NEW.source_agent_message_id IS NOT NULL
                 BEGIN SELECT RAISE(ABORT, 'fault between Mailbox and Wake'); END;",
        )
        .unwrap();
    assert!(follow_up_agent(&mut connection, &followup, 30).is_err());
    let (message_count, wake_count): (i64, i64) = connection
        .query_row(
            "SELECT (SELECT COUNT(*) FROM agent_mailbox_messages),
                        (SELECT COUNT(*) FROM agent_wake_requests)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!((message_count, wake_count), (0, 0));
    connection
        .execute_batch("DROP TRIGGER fail_followup_wake;")
        .unwrap();

    let durable = follow_up_agent(&mut connection, &followup, 31).unwrap();
    let wake = durable.deferred_wake.as_ref().unwrap();
    connection
        .execute_batch(
            "CREATE TEMP TRIGGER fail_dispatch_projection
                 BEFORE INSERT ON messages
                 WHEN NEW.source_agent_message_id IS NOT NULL
                 BEGIN SELECT RAISE(ABORT, 'fault between projection and claim'); END;",
        )
        .unwrap();
    assert!(claim_next_dispatchable_agent_wake(
        &mut connection,
        "claim-before-projection-fault",
        32,
    )
    .is_err());
    assert_eq!(
        get_agent_wake(&connection, &wake.wake_id)
            .unwrap()
            .unwrap()
            .status,
        AgentWakeStatus::Queued
    );
    assert_eq!(
        get_agent_message(&connection, &durable.message.message_id)
            .unwrap()
            .unwrap()
            .delivery_status,
        AgentMailboxDeliveryStatus::Queued
    );
    connection
        .execute_batch("DROP TRIGGER fail_dispatch_projection;")
        .unwrap();

    let claimed =
        claim_next_dispatchable_agent_wake(&mut connection, "claim-after-projection-fault", 33)
            .unwrap()
            .unwrap();
    assert_eq!(claimed.wake_id, wake.wake_id);
    let projected = get_agent_message(&connection, &durable.message.message_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        projected.delivery_status,
        AgentMailboxDeliveryStatus::Acknowledged
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM messages WHERE source_agent_message_id = ?1",
                [&durable.message.message_id],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        1
    );
}

#[test]
fn projection_preserves_fifo_reorders_before_active_assistant_and_satisfies_deferred_wake() {
    let mut connection = setup_tree();
    let active_wake = enqueue_agent_wake(&mut connection, &wake_input("display-active"), 18)
        .unwrap()
        .record()
        .clone();
    claim_next_agent_wake(&mut connection, "agent-child", "display-active-claim", 19)
        .unwrap()
        .unwrap();
    transition_agent_wake(
        &mut connection,
        &active_wake.wake_id,
        AgentWakeStatus::Claimed,
        AgentWakeStatus::Running,
        Some("display-active-claim"),
        20,
    )
    .unwrap();
    connection
        .execute(
            "INSERT INTO messages (
                     id, conversation_id, role, content, status, created_at, position
                 ) VALUES (
                     'assistant-running', 'conversation-child', 'assistant',
                     'Thinking...', 'pending', 20, 0
                 )",
            [],
        )
        .unwrap();
    let trace = crate::ConversationTraceSnapshot::default().in_progress_trace(
        "run-running",
        "conversation-child",
        "assistant-running",
    );
    crate::storage::conversation_trace_repository::append_in_progress_trace(
        &mut connection,
        &trace,
        20,
        20,
    )
    .unwrap();

    let send = send_agent_message(
        &mut connection,
        &SendAgentMessageRequest {
            sender_agent_id: "agent-root".to_string(),
            recipient_agent_id: "agent-child".to_string(),
            request_id: "fifo-send".to_string(),
            content: "earlier send".to_string(),
        },
        21,
    )
    .unwrap();
    let followup = follow_up_agent(
        &mut connection,
        &SendAgentMessageRequest {
            sender_agent_id: "agent-root".to_string(),
            recipient_agent_id: "agent-child".to_string(),
            request_id: "fifo-followup".to_string(),
            content: "later follow-up".to_string(),
        },
        21,
    )
    .unwrap();
    let delivery = crate::storage::agent_delivery_repository::bind_safe_boundary(
        &mut connection,
        &crate::BindAgentSafeBoundaryInput {
            conversation_id: "conversation-child".to_string(),
            run_id: "run-running".to_string(),
            assistant_message_id: "assistant-running".to_string(),
            model_batch_index: 1,
            expected_next_trace_sequence: 0,
            maximum: 16,
        },
        22,
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        delivery
            .messages
            .iter()
            .map(|message| message.message_id.as_str())
            .collect::<Vec<_>>(),
        vec![
            send.message.message_id.as_str(),
            followup.message.message_id.as_str()
        ]
    );
    let satisfied = get_agent_wake(
        &connection,
        &followup.deferred_wake.as_ref().unwrap().wake_id,
    )
    .unwrap()
    .unwrap();
    assert_eq!(satisfied.status, AgentWakeStatus::Satisfied);
    assert_eq!(satisfied.status_revision, 2);
    let retry = crate::storage::agent_delivery_repository::bind_safe_boundary(
        &mut connection,
        &crate::BindAgentSafeBoundaryInput {
            conversation_id: "conversation-child".to_string(),
            run_id: "run-running".to_string(),
            assistant_message_id: "assistant-running".to_string(),
            model_batch_index: 1,
            expected_next_trace_sequence: 0,
            maximum: 16,
        },
        24,
    )
    .unwrap()
    .unwrap();
    assert_eq!(retry, delivery);
    assert_eq!(
        get_agent_wake(
            &connection,
            &followup.deferred_wake.as_ref().unwrap().wake_id,
        )
        .unwrap()
        .unwrap()
        .status_revision,
        2
    );

    let order = connection
        .prepare(
            "SELECT id FROM messages
                 WHERE conversation_id = 'conversation-child' ORDER BY position",
        )
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(
        order,
        vec![
            send.message.projection_message_id,
            followup.message.projection_message_id,
            "assistant-running".to_string(),
        ]
    );
    let running_display = get_agent_display_status(&connection, "agent-child").unwrap();
    assert_eq!(running_display.status, AgentDisplayStatus::Running);
    assert_eq!(
        running_display.latest_wake_id.as_deref(),
        followup
            .deferred_wake
            .as_ref()
            .map(|wake| wake.wake_id.as_str()),
        "cursor facts still describe the newest satisfied Wake"
    );
    assert_eq!(running_display.latest_wake_status_revision, Some(2));

    let before_interrupt =
        agent_collaboration_event_repository::latest_root_sequence(&connection, "agent-root")
            .unwrap();
    transition_agent_wake(
        &mut connection,
        &active_wake.wake_id,
        AgentWakeStatus::Running,
        AgentWakeStatus::Interrupted,
        Some("display-active-claim"),
        25,
    )
    .unwrap();
    let interrupt_events = agent_collaboration_event_repository::list_root_events(
        &connection,
        "agent-root",
        before_interrupt,
        8,
    )
    .unwrap();
    let interrupt_activities = interrupt_events
        .iter()
        .filter_map(|event| event.activity.as_ref())
        .collect::<Vec<_>>();
    assert_eq!(interrupt_activities.len(), 1);
    assert_eq!(
        interrupt_activities[0].semantic,
        crate::AgentCollaborationActivitySemantic::Interrupted
    );
    assert_eq!(interrupt_activities[0].agent_id, "agent-child");
    assert!(interrupt_events.iter().any(|event| {
        event.kind == crate::AgentCollaborationEventKind::WakeUpdated
            && event.activity.as_ref().is_some_and(|activity| {
                activity.semantic == crate::AgentCollaborationActivitySemantic::Interrupted
            })
    }));
    let mut terminal_trace = crate::storage::conversation_trace_repository::get_trace_for_message(
        &connection,
        "assistant-running",
    )
    .unwrap()
    .unwrap();
    terminal_trace.terminal_status = crate::ConversationTurnTraceTerminalStatus::Cancelled;
    terminal_trace.terminal_error = Some("interrupted by test".to_string());
    crate::storage::conversation_trace_repository::replace_trace(
        &mut connection,
        &terminal_trace,
        20,
        25,
    )
    .unwrap();
    assert_eq!(
        get_agent_display_status(&connection, "agent-child")
            .unwrap()
            .status,
        AgentDisplayStatus::LatestInterrupted,
        "satisfied is not itself a completed task and falls back to the last Turn outcome"
    );
}

#[test]
fn root_display_uses_durable_human_turn_and_approval_state_without_a_wake() {
    let mut connection = setup_tree();
    connection
        .execute(
            "INSERT INTO messages (
                     id, conversation_id, role, content, status, created_at, position
                 ) VALUES (
                     'assistant-root-running', 'conversation-root', 'assistant',
                     'Working...', 'pending', 20, 0
                 )",
            [],
        )
        .unwrap();
    let trace = crate::ConversationTraceSnapshot::default().in_progress_trace(
        "run-root-running",
        "conversation-root",
        "assistant-root-running",
    );
    crate::storage::conversation_trace_repository::append_in_progress_trace(
        &mut connection,
        &trace,
        20,
        20,
    )
    .unwrap();

    let running = get_agent_display_status(&connection, "agent-root").unwrap();
    assert_eq!(running.status, AgentDisplayStatus::Running);
    assert!(running.latest_wake_id.is_none());

    connection
        .execute(
            "INSERT INTO agent_pending_actions (
                     action_id, run_id, conversation_id, assistant_message_id,
                     action_type, tool_name, tool_call_id, status, target_status,
                     action_json, agent_input_json, created_at, updated_at
                 ) VALUES (
                     'approval-root-running', 'run-root-running', 'conversation-root',
                     'assistant-root-running', 'tool_approval', 'write_file', 'call-root',
                     'pending', NULL, '{}', '{}', 21, 21
                 )",
            [],
        )
        .unwrap();
    assert_eq!(
        get_agent_display_status(&connection, "agent-root")
            .unwrap()
            .status,
        AgentDisplayStatus::WaitingApproval
    );
    connection
        .execute(
            "UPDATE agent_pending_actions
             SET status = 'approved', updated_at = 22
             WHERE action_id = 'approval-root-running'",
            [],
        )
        .unwrap();
    assert_eq!(
        get_agent_display_status(&connection, "agent-root")
            .unwrap()
            .status,
        AgentDisplayStatus::Running,
        "an accepted approval is execution state, not another user-approval wait"
    );
    connection
        .execute(
            "UPDATE agent_pending_actions
             SET status = 'executing', updated_at = 23
             WHERE action_id = 'approval-root-running'",
            [],
        )
        .unwrap();
    assert_eq!(
        get_agent_display_status(&connection, "agent-root")
            .unwrap()
            .status,
        AgentDisplayStatus::Running
    );

    crate::storage::conversation_trace_repository::replace_trace(
        &mut connection,
        &crate::completed_conversation_trace_without_items(
            "run-root-running",
            "conversation-root",
            "assistant-root-running",
        ),
        20,
        22,
    )
    .unwrap();
    assert_eq!(
        get_agent_display_status(&connection, "agent-root")
            .unwrap()
            .status,
        AgentDisplayStatus::LatestCompleted
    );

    connection
        .execute(
            "INSERT INTO messages (
                     id, conversation_id, role, content, status, created_at, position
                 ) VALUES (
                     'assistant-root-failed', 'conversation-root', 'assistant',
                     'Failed', 'error', 23, 1
                 )",
            [],
        )
        .unwrap();
    let failed_active = crate::ConversationTraceSnapshot::default().in_progress_trace(
        "run-root-failed",
        "conversation-root",
        "assistant-root-failed",
    );
    crate::storage::conversation_trace_repository::append_in_progress_trace(
        &mut connection,
        &failed_active,
        23,
        23,
    )
    .unwrap();
    crate::storage::conversation_trace_repository::replace_trace(
        &mut connection,
        &crate::failed_conversation_trace_without_items(
            "run-root-failed",
            "conversation-root",
            "assistant-root-failed",
            "provider unavailable",
        ),
        23,
        24,
    )
    .unwrap();
    assert_eq!(
        get_agent_display_status(&connection, "agent-root")
            .unwrap()
            .status,
        AgentDisplayStatus::LatestFailed
    );
}
