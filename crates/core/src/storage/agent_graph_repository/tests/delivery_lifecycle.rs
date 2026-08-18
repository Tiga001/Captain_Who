use super::*;

#[test]
fn mailbox_fifo_projection_origin_and_recovery_are_atomic() {
    let mut connection = setup_tree();
    let first_input = message_input(
        "first",
        "agent-root",
        "agent-child",
        AgentMailboxKind::Message,
    );
    let second_input = message_input(
        "second",
        "agent-root",
        "agent-child",
        AgentMailboxKind::Message,
    );
    assert!(matches!(
        enqueue_agent_message(&mut connection, &first_input, 20).unwrap(),
        IdempotentCreate::Created(_)
    ));
    assert!(matches!(
        enqueue_agent_message(&mut connection, &first_input, 21).unwrap(),
        IdempotentCreate::Existing(_)
    ));
    enqueue_agent_message(&mut connection, &second_input, 22).unwrap();

    let first = claim_next_agent_message(&mut connection, "agent-child", "claim-message-first", 23)
        .unwrap()
        .unwrap();
    assert_eq!(first.message_id, "message-first");
    assert!(acknowledge_agent_message_with_projection(
        &mut connection,
        "message-first",
        "wrong-claim",
        24,
    )
    .is_err());
    let acknowledged = acknowledge_agent_message_with_projection(
        &mut connection,
        "message-first",
        "claim-message-first",
        25,
    )
    .unwrap();
    assert_eq!(
        acknowledged.delivery_status,
        AgentMailboxDeliveryStatus::Acknowledged
    );
    assert_eq!(
        conversation_message_origin(&connection, "conversation-child", "projection-first").unwrap(),
        ConversationMessageOrigin::Agent {
            sender_agent_id: "agent-root".to_string(),
            source_agent_message_id: "message-first".to_string(),
        }
    );
    let role: String = connection
        .query_row(
            "SELECT role FROM messages WHERE id = 'projection-first'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(role, "user");
    assert!(connection
        .execute(
            "UPDATE messages SET content = 'forged' WHERE id = 'projection-first'",
            [],
        )
        .is_err());

    let second =
        claim_next_agent_message(&mut connection, "agent-child", "claim-message-second", 26)
            .unwrap()
            .unwrap();
    assert_eq!(second.message_id, "message-second");
    connection
        .execute_batch(
            "CREATE TRIGGER fail_second_projection
                 BEFORE INSERT ON messages WHEN NEW.id = 'projection-second'
                 BEGIN SELECT RAISE(ABORT, 'forced projection failure'); END;",
        )
        .unwrap();
    assert!(acknowledge_agent_message_with_projection(
        &mut connection,
        "message-second",
        "claim-message-second",
        27,
    )
    .is_err());
    assert_eq!(
        get_agent_message(&connection, "message-second")
            .unwrap()
            .unwrap()
            .delivery_status,
        AgentMailboxDeliveryStatus::Claimed
    );
    let projection_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM messages WHERE id = 'projection-second'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(projection_count, 0);
    connection
        .execute("DROP TRIGGER fail_second_projection", [])
        .unwrap();
    acknowledge_agent_message_with_projection(
        &mut connection,
        "message-second",
        "claim-message-second",
        28,
    )
    .unwrap();
}

#[test]
fn mailbox_claims_preserve_fifo_and_recover_after_a_pre_projection_crash() {
    let mut connection = setup_tree();
    let first = message_input(
        "recover-first",
        "agent-root",
        "agent-child",
        AgentMailboxKind::Message,
    );
    let second = message_input(
        "recover-second",
        "agent-root",
        "agent-child",
        AgentMailboxKind::Message,
    );
    enqueue_agent_message(&mut connection, &first, 100).unwrap();
    enqueue_agent_message(&mut connection, &second, 101).unwrap();

    let crashed = claim_next_agent_message(
        &mut connection,
        "agent-child",
        "claim-before-message-crash",
        102,
    )
    .unwrap()
    .unwrap();
    assert_eq!(crashed.message_id, first.message_id);
    assert!(claim_next_agent_message(
        &mut connection,
        "agent-child",
        "claim-must-not-overtake",
        103,
    )
    .unwrap()
    .is_none());

    let expired_at = crashed.lease_expires_at.unwrap();
    let recovered = claim_next_agent_message(
        &mut connection,
        "agent-child",
        "claim-after-message-restart",
        expired_at,
    )
    .unwrap()
    .unwrap();
    assert_eq!(recovered.message_id, first.message_id);
    assert_eq!(
        recovered.claim_token.as_deref(),
        Some("claim-after-message-restart")
    );
    assert!(acknowledge_agent_message_with_projection(
        &mut connection,
        &first.message_id,
        "claim-before-message-crash",
        expired_at,
    )
    .is_err());
    acknowledge_agent_message_with_projection(
        &mut connection,
        &first.message_id,
        "claim-after-message-restart",
        expired_at + 1,
    )
    .unwrap();

    let next = claim_next_agent_message(
        &mut connection,
        "agent-child",
        "claim-after-first-ack",
        expired_at + 2,
    )
    .unwrap()
    .unwrap();
    assert_eq!(next.message_id, second.message_id);
    let projection_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM messages
                 WHERE source_agent_message_id = 'message-recover-first'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(projection_count, 1);
}

#[test]
fn bound_conversations_keep_normal_loop_messages_while_agent_projections_are_immutable() {
    let mut connection = setup_tree();

    let mut root = chat_repository::get_conversation(&connection, "conversation-root")
        .unwrap()
        .unwrap();
    root.messages = vec![
        chat_message("root-user", "user", "hello", 20),
        chat_message("root-assistant", "assistant", "hello back", 21),
    ];
    root.updated_at = 21;
    chat_repository::save_conversation(&mut connection, root).unwrap();
    let loaded_root = chat_repository::get_conversation(&connection, "conversation-root")
        .unwrap()
        .unwrap();
    assert_eq!(loaded_root.messages.len(), 2);
    assert_eq!(
        conversation_message_origin(&connection, "conversation-root", "root-user").unwrap(),
        ConversationMessageOrigin::Human
    );

    let mut stale_root_snapshot = loaded_root;
    let concurrent_result = message_input(
        "concurrent-result",
        "agent-child",
        "agent-root",
        AgentMailboxKind::Result,
    );
    enqueue_agent_message(&mut connection, &concurrent_result, 22).unwrap();
    claim_next_agent_message(&mut connection, "agent-root", "claim-concurrent-result", 23).unwrap();
    acknowledge_agent_message_with_projection(
        &mut connection,
        "message-concurrent-result",
        "claim-concurrent-result",
        24,
    )
    .unwrap();
    stale_root_snapshot.messages.push(chat_message(
        "root-late-assistant",
        "assistant",
        "turn finished",
        25,
    ));
    stale_root_snapshot.updated_at = 25;
    chat_repository::save_conversation(&mut connection, stale_root_snapshot).unwrap();
    let merged_root = chat_repository::get_conversation(&connection, "conversation-root")
        .unwrap()
        .unwrap();
    assert_eq!(merged_root.messages.len(), 4);
    assert!(merged_root
        .messages
        .iter()
        .any(|message| message.id == "projection-concurrent-result"));
    let positions = connection
        .prepare(
            "SELECT position FROM messages WHERE conversation_id = 'conversation-root'
                 ORDER BY position",
        )
        .unwrap()
        .query_map([], |row| row.get::<_, i64>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(positions, vec![0, 1, 2, 3]);

    let task = message_input(
        "loop-input",
        "agent-root",
        "agent-child",
        AgentMailboxKind::Message,
    );
    enqueue_agent_message(&mut connection, &task, 22).unwrap();
    claim_next_agent_message(&mut connection, "agent-child", "claim-loop-input", 23).unwrap();
    acknowledge_agent_message_with_projection(
        &mut connection,
        "message-loop-input",
        "claim-loop-input",
        24,
    )
    .unwrap();

    let mut child = chat_repository::get_conversation(&connection, "conversation-child")
        .unwrap()
        .unwrap();
    assert_eq!(child.messages[0].role, "user");
    child.messages.push(chat_message(
        "child-assistant",
        "assistant",
        "review complete",
        25,
    ));
    child.updated_at = 25;
    chat_repository::save_conversation(&mut connection, child).unwrap();
    assert_eq!(
        chat_repository::get_conversation(&connection, "conversation-child")
            .unwrap()
            .unwrap()
            .messages
            .len(),
        2
    );

    connection
        .execute(
            "INSERT INTO messages (
                     id, conversation_id, role, content, status, created_at, position
                 ) VALUES (
                     'child-normal-before-projection', 'conversation-child', 'assistant',
                     'ordinary earlier output', 'sent', 20, -1
                 )",
            [],
        )
        .unwrap();
    assert!(connection
        .execute(
            "UPDATE messages SET role = 'user'
                 WHERE id = 'child-normal-before-projection'",
            [],
        )
        .is_err());

    chat_repository::delete_messages(
        &mut connection,
        "conversation-child",
        &["child-assistant".to_string()],
    )
    .unwrap();
    chat_repository::delete_messages(
        &mut connection,
        "conversation-child",
        &["child-normal-before-projection".to_string()],
    )
    .unwrap();
    assert!(chat_repository::delete_messages(
        &mut connection,
        "conversation-child",
        &["projection-loop-input".to_string()],
    )
    .is_err());
    assert!(connection
        .execute(
            "UPDATE messages SET position = 99 WHERE id = 'projection-loop-input'",
            [],
        )
        .is_err());
    let before_forbidden_save =
        chat_repository::get_conversation(&connection, "conversation-child")
            .unwrap()
            .unwrap();
    let mut forbidden_human_input = before_forbidden_save.clone();
    forbidden_human_input.messages.push(chat_message(
        "forbidden-child-human",
        "user",
        "direct user bypass",
        100,
    ));
    assert!(chat_repository::save_conversation(&mut connection, forbidden_human_input).is_err());
    let after_forbidden_save = chat_repository::get_conversation(&connection, "conversation-child")
        .unwrap()
        .unwrap();
    assert_eq!(
        after_forbidden_save.messages.len(),
        before_forbidden_save.messages.len()
    );
    assert!(!after_forbidden_save
        .messages
        .iter()
        .any(|message| message.id == "forbidden-child-human"));
    assert!(connection
        .execute(
            "DELETE FROM messages WHERE id = 'projection-loop-input'",
            [],
        )
        .is_err());
    assert!(connection
        .execute(
            "INSERT OR REPLACE INTO messages (
                     id, conversation_id, role, content, status, created_at, position
                 ) VALUES (
                     'projection-loop-input', 'conversation-child', 'user',
                     'forged replacement', 'sent', 99, 99
                 )",
            [],
        )
        .is_err());
}

#[test]
fn wake_claim_transition_and_result_settlement_are_single_turn_and_idempotent() {
    let mut connection = setup_tree();
    let source = message_input(
        "wake-source",
        "agent-root",
        "agent-child",
        AgentMailboxKind::Task,
    );
    enqueue_agent_message(&mut connection, &source, 29).unwrap();
    let wake_before_delivery = EnqueueAgentWakeInput {
        source_agent_message_id: Some(source.message_id.clone()),
        ..wake_input("before-delivery")
    };
    assert!(matches!(
        enqueue_agent_wake(&mut connection, &wake_before_delivery, 30).unwrap(),
        IdempotentCreate::Created(_)
    ));
    assert!(
        claim_next_agent_wake(&mut connection, "agent-child", "claim-undelivered-wake", 30,)
            .is_err()
    );
    claim_next_agent_message(&mut connection, "agent-child", "claim-wake-source", 31).unwrap();
    let (acknowledged_source, atomic_wake) = acknowledge_agent_task_with_projection_and_wake(
        &mut connection,
        &AcknowledgeAgentTaskAndWakeInput {
            message_id: "message-wake-source".to_string(),
            message_claim_token: "claim-wake-source".to_string(),
            wake: wake_before_delivery.clone(),
        },
        32,
    )
    .unwrap();
    assert_eq!(
        acknowledged_source.delivery_status,
        AgentMailboxDeliveryStatus::Acknowledged
    );
    assert_eq!(atomic_wake.wake_id, "wake-before-delivery");
    assert!(matches!(
        enqueue_agent_wake(&mut connection, &wake_before_delivery, 33).unwrap(),
        IdempotentCreate::Existing(_)
    ));

    let wake_one = wake_input("one");
    let wake_two = wake_input("two");
    assert!(matches!(
        enqueue_agent_wake(&mut connection, &wake_one, 34).unwrap(),
        IdempotentCreate::Created(_)
    ));
    assert!(matches!(
        enqueue_agent_wake(&mut connection, &wake_one, 35).unwrap(),
        IdempotentCreate::Existing(_)
    ));
    enqueue_agent_wake(&mut connection, &wake_two, 36).unwrap();

    let claimed =
        claim_next_agent_wake(&mut connection, "agent-child", "claim-before-delivery", 37)
            .unwrap()
            .unwrap();
    assert_eq!(claimed.wake_id, "wake-before-delivery");
    transition_agent_wake(
        &mut connection,
        "wake-before-delivery",
        AgentWakeStatus::Claimed,
        AgentWakeStatus::Cancelled,
        Some("claim-before-delivery"),
        38,
    )
    .unwrap();

    let claimed = claim_next_agent_wake(&mut connection, "agent-child", "claim-wake-one", 39)
        .unwrap()
        .unwrap();
    assert_eq!(claimed.wake_id, "wake-one");
    let original_deadline = claimed.lease_expires_at.unwrap();
    let renewed =
        renew_agent_wake_lease(&mut connection, "wake-one", "claim-wake-one", 40).unwrap();
    assert!(renewed.lease_expires_at.unwrap() > original_deadline);
    assert!(
        claim_next_agent_wake(&mut connection, "agent-child", "claim-wake-two", 41,)
            .unwrap()
            .is_none()
    );
    transition_agent_wake(
        &mut connection,
        "wake-one",
        AgentWakeStatus::Claimed,
        AgentWakeStatus::Running,
        Some("claim-wake-one"),
        42,
    )
    .unwrap();

    let finish = FinishAgentWakeWithResultInput {
        wake_id: "wake-one".to_string(),
        expected_status: AgentWakeStatus::Running,
        claim_token: "claim-wake-one".to_string(),
        terminal_status: AgentWakeStatus::Completed,
        terminal_error: None,
        result_message: message_input(
            "result-one",
            "agent-child",
            "agent-root",
            AgentMailboxKind::Result,
        ),
    };
    let settled = finish_agent_wake_with_result(&mut connection, &finish, 43).unwrap();
    assert_eq!(settled.status, AgentWakeStatus::Completed);
    assert_eq!(
        settled.result_message_id.as_deref(),
        Some("message-result-one")
    );
    assert_eq!(
        finish_agent_wake_with_result(&mut connection, &finish, 44)
            .unwrap()
            .status,
        AgentWakeStatus::Completed
    );
    let second = claim_next_agent_wake(&mut connection, "agent-child", "claim-wake-two", 45)
        .unwrap()
        .unwrap();
    assert_eq!(second.wake_id, "wake-two");
    assert!(matches!(
        transition_agent_wake(
            &mut connection,
            "wake-one",
            AgentWakeStatus::Completed,
            AgentWakeStatus::Running,
            Some("claim-wake-one"),
            46,
        ),
        Err(AgentGraphError::IllegalTransition { .. })
    ));
}

#[test]
fn an_expired_unstarted_wake_claim_is_recovered_without_a_second_active_turn() {
    let mut connection = setup_tree();
    enqueue_agent_wake(&mut connection, &wake_input("recovery"), 50).unwrap();
    let first = claim_next_agent_wake(&mut connection, "agent-child", "claim-before-crash", 51)
        .unwrap()
        .unwrap();
    let first_deadline = first.lease_expires_at.unwrap();

    let recovered = claim_next_agent_wake(
        &mut connection,
        "agent-child",
        "claim-after-restart",
        first_deadline,
    )
    .unwrap()
    .unwrap();
    assert_eq!(recovered.wake_id, "wake-recovery");
    assert_eq!(
        recovered.claim_token.as_deref(),
        Some("claim-after-restart")
    );
    assert!(recovered.lease_expires_at.unwrap() > first_deadline);

    let active_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM agent_wake_requests
                 WHERE agent_id = 'agent-child'
                   AND status IN ('claimed', 'running', 'waiting_for_approval')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(active_count, 1);
    assert!(renew_agent_wake_lease(
        &mut connection,
        "wake-recovery",
        "claim-before-crash",
        first_deadline,
    )
    .is_err());
}

#[test]
fn lease_deadline_is_half_open_and_fences_old_mailbox_and_wake_holders() {
    let mut connection = setup_tree();

    let mailbox = message_input(
        "half-open-mailbox",
        "agent-root",
        "agent-child",
        AgentMailboxKind::Message,
    );
    enqueue_agent_message(&mut connection, &mailbox, 100).unwrap();
    let claimed_message =
        claim_next_agent_message(&mut connection, "agent-child", "old-mailbox-holder", 101)
            .unwrap()
            .unwrap();
    let mailbox_deadline = claimed_message.lease_expires_at.unwrap();
    assert!(renew_agent_message_lease(
        &mut connection,
        &mailbox.message_id,
        "old-mailbox-holder",
        mailbox_deadline,
    )
    .is_err());
    assert!(acknowledge_agent_message_with_projection(
        &mut connection,
        &mailbox.message_id,
        "old-mailbox-holder",
        mailbox_deadline,
    )
    .is_err());
    let new_message_holder = claim_next_agent_message(
        &mut connection,
        "agent-child",
        "new-mailbox-holder",
        mailbox_deadline,
    )
    .unwrap()
    .unwrap();
    assert_eq!(new_message_holder.message_id, mailbox.message_id);

    enqueue_agent_wake(&mut connection, &wake_input("half-open-wake"), 200).unwrap();
    let claimed = claim_next_agent_wake(&mut connection, "agent-child", "old-wake-holder", 201)
        .unwrap()
        .unwrap();
    let wake_deadline = claimed.lease_expires_at.unwrap();
    assert!(renew_agent_wake_lease(
        &mut connection,
        &claimed.wake_id,
        "old-wake-holder",
        wake_deadline,
    )
    .is_err());
    assert!(transition_agent_wake(
        &mut connection,
        &claimed.wake_id,
        AgentWakeStatus::Claimed,
        AgentWakeStatus::Running,
        Some("old-wake-holder"),
        wake_deadline,
    )
    .is_err());
    let legacy_finish = FinishAgentWakeWithResultInput {
        wake_id: claimed.wake_id.clone(),
        expected_status: AgentWakeStatus::Claimed,
        claim_token: "old-wake-holder".to_string(),
        terminal_status: AgentWakeStatus::Failed,
        terminal_error: Some("expired".to_string()),
        result_message: message_input(
            "half-open-result",
            "agent-child",
            "agent-root",
            AgentMailboxKind::Result,
        ),
    };
    assert!(
        finish_agent_wake_with_result(&mut connection, &legacy_finish, wake_deadline,).is_err()
    );
    let typed_finish = FinishAgentTurnResultInput {
        wake_id: claimed.wake_id.clone(),
        expected_status: AgentWakeStatus::Claimed,
        claim_token: "old-wake-holder".to_string(),
        terminal_status: AgentWakeStatus::Failed,
        run_id: None,
        assistant_message_id: None,
        summary: "expired before admission".to_string(),
        terminal_error: Some("expired".to_string()),
    };
    assert!(finish_agent_turn_with_result(&mut connection, &typed_finish, wake_deadline,).is_err());

    let recovery =
        recover_agent_wakes(&mut connection, "deadline-recovery", wake_deadline).unwrap();
    assert_eq!(recovery.requeued_before_dispatch, 1);
    let replacement = claim_next_agent_wake(
        &mut connection,
        "agent-child",
        "new-wake-holder",
        wake_deadline,
    )
    .unwrap()
    .unwrap();
    assert_eq!(replacement.wake_id, claimed.wake_id);
    assert_eq!(replacement.claim_token.as_deref(), Some("new-wake-holder"));
}

#[test]
fn bound_deletion_and_lifecycle_transitions_fail_closed() {
    let mut connection = setup_tree();
    assert!(matches!(
        ensure_conversation_unbound(&connection, "conversation-child"),
        Err(AgentGraphError::BoundConversation(_))
    ));
    assert!(matches!(
        ensure_project_unbound(&connection, "project-a"),
        Err(AgentGraphError::BoundProject(_))
    ));
    assert!(connection
        .execute(
            "DELETE FROM conversations WHERE id = 'conversation-child'",
            [],
        )
        .is_err());

    assert!(transition_agent_lifecycle(
        &mut connection,
        "agent-root",
        1,
        AgentLifecycle::Active,
        AgentLifecycle::Archived,
        39,
    )
    .is_err());

    let pending = message_input(
        "before-archive",
        "agent-root",
        "agent-child",
        AgentMailboxKind::Message,
    );
    enqueue_agent_message(&mut connection, &pending, 40).unwrap();
    assert!(transition_agent_lifecycle(
        &mut connection,
        "agent-child",
        1,
        AgentLifecycle::Active,
        AgentLifecycle::Archived,
        41,
    )
    .is_err());
    claim_next_agent_message(&mut connection, "agent-child", "claim-before-archive", 42).unwrap();
    acknowledge_agent_message_with_projection(
        &mut connection,
        "message-before-archive",
        "claim-before-archive",
        43,
    )
    .unwrap();

    let archived = transition_agent_lifecycle(
        &mut connection,
        "agent-child",
        1,
        AgentLifecycle::Active,
        AgentLifecycle::Archived,
        44,
    )
    .unwrap();
    assert_eq!(archived.revision, 2);
    assert_eq!(archived.lifecycle, AgentLifecycle::Archived);
    assert!(matches!(
        transition_agent_lifecycle(
            &mut connection,
            "agent-child",
            1,
            AgentLifecycle::Active,
            AgentLifecycle::Disabled,
            45,
        ),
        Err(AgentGraphError::RevisionConflict { .. })
    ));
    assert!(enqueue_agent_wake(&mut connection, &wake_input("inactive"), 46).is_err());
}

#[test]
fn active_conversation_turn_fences_lifecycle_deactivation_until_terminal() {
    let mut connection = setup_tree();
    connection
        .execute(
            "INSERT INTO messages (
                     id, conversation_id, role, content, status, created_at, position
                 ) VALUES (
                     'assistant-active-root', 'conversation-child', 'assistant',
                     'Thinking...', 'pending', 20, 0
                 )",
            [],
        )
        .unwrap();
    let active = crate::ConversationTraceSnapshot::default().in_progress_trace(
        "run-active-root",
        "conversation-child",
        "assistant-active-root",
    );
    crate::storage::conversation_trace_repository::append_in_progress_trace(
        &mut connection,
        &active,
        20,
        20,
    )
    .unwrap();

    let error = transition_agent_lifecycle(
        &mut connection,
        "agent-child",
        1,
        AgentLifecycle::Active,
        AgentLifecycle::Disabled,
        21,
    )
    .unwrap_err();
    assert!(matches!(
        error,
        AgentGraphError::Conflict(reason) if reason.contains("active Conversation Turn")
    ));
    assert!(connection
        .execute(
            "UPDATE agent_nodes
                 SET lifecycle = 'disabled', revision = revision + 1, updated_at = 22
                 WHERE agent_id = 'agent-child'",
            [],
        )
        .is_err());

    let terminal = crate::completed_conversation_trace_without_items(
        "run-active-root",
        "conversation-child",
        "assistant-active-root",
    );
    crate::storage::conversation_trace_repository::replace_trace(
        &mut connection,
        &terminal,
        20,
        23,
    )
    .unwrap();
    let disabled = transition_agent_lifecycle(
        &mut connection,
        "agent-child",
        1,
        AgentLifecycle::Active,
        AgentLifecycle::Disabled,
        24,
    )
    .unwrap();
    assert_eq!(disabled.lifecycle, AgentLifecycle::Disabled);
}
