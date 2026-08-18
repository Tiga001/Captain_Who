use super::*;

#[test]
fn child_result_is_frozen_direct_parent_outbox_and_root_is_not_auto_woken() {
    let mut connection = setup_tree();
    add_grandchild(&mut connection);
    let followup = follow_up_agent(
        &mut connection,
        &SendAgentMessageRequest {
            sender_agent_id: "agent-root".to_string(),
            recipient_agent_id: "agent-grand".to_string(),
            request_id: "grand-task".to_string(),
            content: "perform nested check".to_string(),
        },
        30,
    )
    .unwrap();
    let wake_id = followup.deferred_wake.unwrap().wake_id;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();
    project_agent_wake_source_in_transaction(&transaction, &wake_id, "grand-project", 31).unwrap();
    transaction.commit().unwrap();
    let _claimed = claim_next_agent_wake(&mut connection, "agent-grand", "grand-claim", 32)
        .unwrap()
        .unwrap();
    transition_agent_wake(
        &mut connection,
        &wake_id,
        AgentWakeStatus::Claimed,
        AgentWakeStatus::Running,
        Some("grand-claim"),
        33,
    )
    .unwrap();
    connection
        .execute(
            "INSERT INTO messages (
                     id, conversation_id, role, content, status, created_at, position
                 ) VALUES (
                     'assistant-grand', 'conversation-grand', 'assistant',
                     'nested review complete', 'sent', 33,
                     (SELECT COALESCE(MAX(position), -1) + 1 FROM messages
                      WHERE conversation_id = 'conversation-grand')
                 )",
            [],
        )
        .unwrap();
    crate::storage::conversation_trace_repository::replace_trace(
        &mut connection,
        &crate::completed_conversation_trace_without_items(
            "run-grand",
            "conversation-grand",
            "assistant-grand",
        ),
        33,
        33,
    )
    .unwrap();
    connection
        .execute(
            "UPDATE agent_wake_requests
                 SET run_id = 'run-grand', assistant_message_id = 'assistant-grand'
                 WHERE wake_id = ?1",
            [&wake_id],
        )
        .unwrap();
    let artifact_id = format!("sha256:{}", "a".repeat(64));
    connection
        .execute(
            "INSERT INTO managed_artifacts (
                     artifact_id, schema_version, kind, storage_relative_path, format,
                     media_type, size_bytes, sha256, width, height, created_at
                 ) VALUES (?1, 1, 'document', 'objects/result.pdf', 'pdf',
                           'application/pdf', 10, ?2, NULL, NULL, 34)",
            params![&artifact_id, "a".repeat(64)],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO managed_artifact_grants (
                     artifact_id, conversation_id, run_id, call_id, created_at
                 ) VALUES (?1, 'conversation-grand', 'run-grand', 'call-one', 34)",
            [&artifact_id],
        )
        .unwrap();
    let finish = FinishAgentTurnResultInput {
        wake_id: wake_id.clone(),
        expected_status: AgentWakeStatus::Running,
        claim_token: "grand-claim".to_string(),
        terminal_status: AgentWakeStatus::Completed,
        run_id: Some("run-grand".to_string()),
        assistant_message_id: Some("assistant-grand".to_string()),
        summary: "nested review complete".to_string(),
        terminal_error: None,
    };
    let before_finish =
        agent_collaboration_event_repository::latest_root_sequence(&connection, "agent-root")
            .unwrap();
    let settled = finish_agent_turn_with_result(&mut connection, &finish, 35).unwrap();
    assert_eq!(settled.result_message.sender_agent_id, "agent-grand");
    assert_eq!(settled.result_message.recipient_agent_id, "agent-child");
    assert_eq!(settled.envelope.artifact_refs.len(), 1);
    assert!(settled.parent_wake.is_some());
    assert!(!settled.result_message.content.contains("usage"));
    let settlement_events = agent_collaboration_event_repository::list_root_events(
        &connection,
        "agent-root",
        before_finish,
        16,
    )
    .unwrap();
    let settlement_activities = settlement_events
        .iter()
        .filter_map(|event| event.activity.as_ref())
        .collect::<Vec<_>>();
    assert_eq!(settlement_activities.len(), 1);
    assert_eq!(
        settlement_activities[0].semantic,
        crate::AgentCollaborationActivitySemantic::Completed
    );
    assert_eq!(settlement_activities[0].agent_id, "agent-grand");
    assert!(settlement_events.iter().any(|event| {
        event.kind == crate::AgentCollaborationEventKind::MailboxEnqueued
            && event.message_id.as_deref() == Some(settled.result_message.message_id.as_str())
            && event.activity.is_none()
    }));
    assert!(settlement_events.iter().all(|event| {
        event.activity.is_none() || event.kind == crate::AgentCollaborationEventKind::WakeUpdated
    }));

    let later_artifact_id = format!("sha256:{}", "b".repeat(64));
    connection
        .execute(
            "INSERT INTO managed_artifacts (
                     artifact_id, schema_version, kind, storage_relative_path, format,
                     media_type, size_bytes, sha256, width, height, created_at
                 ) VALUES (?1, 1, 'document', 'objects/later.pdf', 'pdf',
                           'application/pdf', 10, ?2, NULL, NULL, 36)",
            params![&later_artifact_id, "b".repeat(64)],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO managed_artifact_grants (
                     artifact_id, conversation_id, run_id, call_id, created_at
                 ) VALUES (?1, 'conversation-grand', 'run-grand', 'call-later', 36)",
            [&later_artifact_id],
        )
        .unwrap();
    let retry = finish_agent_turn_with_result(&mut connection, &finish, 37).unwrap();
    assert_eq!(retry.envelope, settled.envelope);
    assert_eq!(retry.envelope.artifact_refs.len(), 1);
    assert_eq!(
        agent_collaboration_event_repository::latest_root_sequence(&connection, "agent-root")
            .unwrap(),
        settlement_events.last().unwrap().root_sequence
    );

    let parent_wake = settled.parent_wake.as_ref().unwrap();
    connection
        .execute(
            "INSERT INTO messages (
                     id, conversation_id, role, content, status, created_at, position
                 ) VALUES (
                     'assistant-parent-running', 'conversation-child', 'assistant',
                     'Handling child result', 'pending', 38,
                     (SELECT COALESCE(MAX(position), -1) + 1 FROM messages
                      WHERE conversation_id = 'conversation-child')
                 )",
            [],
        )
        .unwrap();
    let parent_trace = crate::ConversationTraceSnapshot::default().in_progress_trace(
        "run-parent-running",
        "conversation-child",
        "assistant-parent-running",
    );
    crate::storage::conversation_trace_repository::append_in_progress_trace(
        &mut connection,
        &parent_trace,
        38,
        38,
    )
    .unwrap();
    let delivered_result = crate::storage::agent_delivery_repository::bind_safe_boundary(
        &mut connection,
        &crate::BindAgentSafeBoundaryInput {
            conversation_id: "conversation-child".to_string(),
            run_id: "run-parent-running".to_string(),
            assistant_message_id: "assistant-parent-running".to_string(),
            model_batch_index: 1,
            expected_next_trace_sequence: 0,
            maximum: 16,
        },
        39,
    )
    .unwrap()
    .unwrap();
    assert_eq!(delivered_result.messages.len(), 1);
    assert_eq!(
        delivered_result.messages[0].message_id,
        settled.result_message.message_id
    );
    assert_eq!(
        get_agent_wake(&connection, &parent_wake.wake_id)
            .unwrap()
            .unwrap()
            .status,
        AgentWakeStatus::Satisfied
    );
    let mut terminal_parent_trace =
        crate::storage::conversation_trace_repository::get_trace_for_message(
            &connection,
            "assistant-parent-running",
        )
        .unwrap()
        .unwrap();
    terminal_parent_trace.terminal_status = crate::ConversationTurnTraceTerminalStatus::Completed;
    terminal_parent_trace.terminal_error = None;
    crate::storage::conversation_trace_repository::replace_trace(
        &mut connection,
        &terminal_parent_trace,
        38,
        40,
    )
    .unwrap();

    enqueue_agent_wake(&mut connection, &wake_input("root-result"), 41).unwrap();
    let root_child_wake =
        claim_next_agent_wake(&mut connection, "agent-child", "root-result-claim", 42)
            .unwrap()
            .unwrap();
    let before_failed_finish =
        agent_collaboration_event_repository::latest_root_sequence(&connection, "agent-root")
            .unwrap();
    let root_settlement = finish_agent_turn_with_result(
        &mut connection,
        &FinishAgentTurnResultInput {
            wake_id: root_child_wake.wake_id,
            expected_status: AgentWakeStatus::Claimed,
            claim_token: "root-result-claim".to_string(),
            terminal_status: AgentWakeStatus::Failed,
            run_id: None,
            assistant_message_id: None,
            summary: "provider unavailable before Turn admission".to_string(),
            terminal_error: Some("model disabled".to_string()),
        },
        43,
    )
    .unwrap();
    assert!(root_settlement.parent_wake.is_none());
    assert_eq!(
        root_settlement.result_message.recipient_agent_id,
        "agent-root"
    );
    assert_eq!(
        get_agent_display_status(&connection, "agent-child")
            .unwrap()
            .status,
        AgentDisplayStatus::LatestFailed
    );
    let failed_events = agent_collaboration_event_repository::list_root_events(
        &connection,
        "agent-root",
        before_failed_finish,
        16,
    )
    .unwrap();
    let failed_activities = failed_events
        .iter()
        .filter_map(|event| event.activity.as_ref())
        .collect::<Vec<_>>();
    assert_eq!(failed_activities.len(), 1);
    assert_eq!(
        failed_activities[0].semantic,
        crate::AgentCollaborationActivitySemantic::Failed
    );
    assert_eq!(failed_activities[0].agent_id, "agent-child");
    assert!(failed_events.iter().any(|event| {
        event.kind == crate::AgentCollaborationEventKind::MailboxEnqueued
            && event.message_id.as_deref()
                == Some(root_settlement.result_message.message_id.as_str())
            && event.activity.is_none()
    }));
}

#[test]
fn terminal_result_faults_rollback_and_recover_exactly_once_after_restart() {
    for fault in ["result_outbox", "parent_wake"] {
        let mut connection = setup_tree();
        add_grandchild(&mut connection);
        let wake_id = format!("wake-fault-{fault}");
        enqueue_agent_wake(
            &mut connection,
            &EnqueueAgentWakeInput {
                wake_id: wake_id.clone(),
                root_agent_id: "agent-root".to_string(),
                agent_id: "agent-grand".to_string(),
                requester_agent_id: "agent-child".to_string(),
                request_id: format!("request-fault-{fault}"),
                source_agent_message_id: None,
            },
            20,
        )
        .unwrap();
        let claimed = claim_next_dispatchable_agent_wake(&mut connection, "claim-before-fault", 21)
            .unwrap()
            .unwrap();
        let lease_expires_at = claimed.lease_expires_at.unwrap();
        transition_agent_wake(
            &mut connection,
            &wake_id,
            AgentWakeStatus::Claimed,
            AgentWakeStatus::Running,
            Some("claim-before-fault"),
            22,
        )
        .unwrap();
        let run_id = format!("run-fault-{fault}");
        let assistant_message_id = format!("assistant-fault-{fault}");
        connection
            .execute(
                "INSERT INTO messages (
                         id, conversation_id, role, content, status, created_at, position
                     ) VALUES (?1, 'conversation-grand', 'assistant', 'durable terminal',
                               'sent', 22,
                               (SELECT COALESCE(MAX(position), -1) + 1 FROM messages
                                WHERE conversation_id = 'conversation-grand'))",
                [&assistant_message_id],
            )
            .unwrap();
        crate::storage::conversation_trace_repository::replace_trace(
            &mut connection,
            &crate::completed_conversation_trace_without_items(
                &run_id,
                "conversation-grand",
                &assistant_message_id,
            ),
            22,
            23,
        )
        .unwrap();
        connection
            .execute(
                "UPDATE agent_wake_requests
                     SET run_id = ?1, assistant_message_id = ?2
                     WHERE wake_id = ?3",
                params![&run_id, &assistant_message_id, &wake_id],
            )
            .unwrap();

        match fault {
            "result_outbox" => connection
                .execute_batch(
                    "CREATE TEMP TRIGGER fail_result_outbox
                         BEFORE INSERT ON agent_mailbox_messages
                         WHEN NEW.kind = 'result'
                         BEGIN SELECT RAISE(ABORT, 'fault after terminal trace'); END;",
                )
                .unwrap(),
            "parent_wake" => connection
                .execute_batch(
                    "CREATE TEMP TRIGGER fail_parent_result_wake
                         BEFORE INSERT ON agent_wake_requests
                         WHEN NEW.source_agent_message_id IS NOT NULL
                         BEGIN SELECT RAISE(ABORT, 'fault after result outbox'); END;",
                )
                .unwrap(),
            _ => unreachable!(),
        }
        let first_finish = FinishAgentTurnResultInput {
            wake_id: wake_id.clone(),
            expected_status: AgentWakeStatus::Running,
            claim_token: "claim-before-fault".to_string(),
            terminal_status: AgentWakeStatus::Completed,
            run_id: Some(run_id.clone()),
            assistant_message_id: Some(assistant_message_id.clone()),
            summary: "durable terminal".to_string(),
            terminal_error: None,
        };
        assert!(finish_agent_turn_with_result(&mut connection, &first_finish, 24).is_err());
        assert_eq!(
            get_agent_wake(&connection, &wake_id)
                .unwrap()
                .unwrap()
                .status,
            AgentWakeStatus::Running,
            "{fault} must not partially terminalize the Wake"
        );
        assert_eq!(
            crate::storage::conversation_trace_repository::get_trace_for_message(
                &connection,
                &assistant_message_id,
            )
            .unwrap()
            .unwrap()
            .terminal_status,
            crate::ConversationTurnTraceTerminalStatus::Completed,
            "the pre-existing terminal trace remains the recovery truth"
        );
        let (result_count, wake_count): (i64, i64) = connection
            .query_row(
                "SELECT
                         (SELECT COUNT(*) FROM agent_mailbox_messages WHERE kind = 'result'),
                         (SELECT COUNT(*) FROM agent_wake_requests)",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!((result_count, wake_count), (0, 1));

        connection
            .execute_batch(match fault {
                "result_outbox" => "DROP TRIGGER fail_result_outbox;",
                "parent_wake" => "DROP TRIGGER fail_parent_result_wake;",
                _ => unreachable!(),
            })
            .unwrap();
        let recovered =
            recover_agent_wakes(&mut connection, "claim-after-restart", lease_expires_at).unwrap();
        assert_eq!(recovered.actions.len(), 1);
        let AgentWakeRecoveryAction::Observe(recovered_wake) = &recovered.actions[0] else {
            panic!("terminal trace must be observed, never replayed: {fault}");
        };
        let recovered_finish = FinishAgentTurnResultInput {
            claim_token: recovered_wake.claim_token.clone().unwrap(),
            ..first_finish
        };
        let settled =
            finish_agent_turn_with_result(&mut connection, &recovered_finish, 25).unwrap();
        assert_eq!(
            settled.parent_wake.as_ref().unwrap().agent_id,
            "agent-child"
        );
        let retry = finish_agent_turn_with_result(&mut connection, &recovered_finish, 26).unwrap();
        assert_eq!(retry.envelope, settled.envelope);
        let (result_count, wake_count): (i64, i64) = connection
            .query_row(
                "SELECT
                         (SELECT COUNT(*) FROM agent_mailbox_messages WHERE kind = 'result'),
                         (SELECT COUNT(*) FROM agent_wake_requests)",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!((result_count, wake_count), (1, 2));
    }
}

#[test]
fn interrupt_request_is_tree_scoped_and_idempotent_across_multiple_wakes() {
    let mut connection = setup_tree();
    enqueue_agent_wake(&mut connection, &wake_input("interrupt-one"), 20).unwrap();
    enqueue_agent_wake(&mut connection, &wake_input("interrupt-two"), 21).unwrap();

    let before_queued_interrupt =
        agent_collaboration_event_repository::latest_root_sequence(&connection, "agent-root")
            .unwrap();
    let first = interrupt_agent_execution(
        &mut connection,
        "agent-root",
        "agent-child",
        "interrupt-request-one",
        22,
    )
    .unwrap();
    assert_eq!(
        first,
        InterruptAgentExecutionOutcome::QueuedWakeCancelled {
            wake_id: "wake-interrupt-one".to_string()
        }
    );
    assert_eq!(
        get_agent_wake(&connection, "wake-interrupt-one")
            .unwrap()
            .unwrap()
            .status,
        AgentWakeStatus::Cancelled
    );
    let queued_interrupt_events = agent_collaboration_event_repository::list_root_events(
        &connection,
        "agent-root",
        before_queued_interrupt,
        8,
    )
    .unwrap();
    assert_eq!(
        queued_interrupt_events
            .iter()
            .filter_map(|event| event.activity.as_ref())
            .map(|activity| activity.semantic)
            .collect::<Vec<_>>(),
        vec![crate::AgentCollaborationActivitySemantic::Interrupted]
    );
    assert!(queued_interrupt_events.iter().any(|event| {
        event.kind == crate::AgentCollaborationEventKind::WakeUpdated
            && event.agent_id == "agent-child"
            && event.activity.as_ref().is_some_and(|activity| {
                activity.agent_id == "agent-child" && activity.task_name_snapshot == "review"
            })
    }));
    let after_queued_interrupt =
        agent_collaboration_event_repository::latest_root_sequence(&connection, "agent-root")
            .unwrap();
    let retry = interrupt_agent_execution(
        &mut connection,
        "agent-root",
        "agent-child",
        "interrupt-request-one",
        23,
    )
    .unwrap();
    assert_eq!(retry, first, "a retry cannot cancel the next queued Wake");
    assert_eq!(
        agent_collaboration_event_repository::latest_root_sequence(&connection, "agent-root")
            .unwrap(),
        after_queued_interrupt,
        "the idempotent interrupt receipt cannot duplicate timeline activity"
    );
    assert_eq!(
        get_agent_wake(&connection, "wake-interrupt-two")
            .unwrap()
            .unwrap()
            .status,
        AgentWakeStatus::Queued
    );

    let claimed = claim_next_agent_wake(
        &mut connection,
        "agent-child",
        "interrupt-claimed-token",
        24,
    )
    .unwrap()
    .unwrap();
    assert_eq!(claimed.wake_id, "wake-interrupt-two");
    let before_claimed_interrupt =
        agent_collaboration_event_repository::latest_root_sequence(&connection, "agent-root")
            .unwrap();
    let second = interrupt_agent_execution(
        &mut connection,
        "agent-root",
        "agent-child",
        "interrupt-request-two",
        25,
    )
    .unwrap();
    assert_eq!(
        second,
        InterruptAgentExecutionOutcome::QueuedWakeCancelled {
            wake_id: "wake-interrupt-two".to_string()
        }
    );
    let claimed_interrupt_events = agent_collaboration_event_repository::list_root_events(
        &connection,
        "agent-root",
        before_claimed_interrupt,
        8,
    )
    .unwrap();
    assert_eq!(
        claimed_interrupt_events
            .iter()
            .filter_map(|event| event.activity.as_ref())
            .map(|activity| activity.semantic)
            .collect::<Vec<_>>(),
        vec![crate::AgentCollaborationActivitySemantic::Interrupted]
    );
    assert_eq!(
        get_agent_display_status(&connection, "agent-child")
            .unwrap()
            .status,
        AgentDisplayStatus::LatestInterrupted
    );
    assert!(interrupt_agent_execution(
        &mut connection,
        "agent-child",
        "agent-root",
        "interrupt-upward-forbidden",
        26,
    )
    .is_err());
}

#[test]
fn global_claim_skips_live_mailbox_claim_and_recovers_when_it_expires() {
    let mut connection = setup_tree();
    add_grandchild(&mut connection);
    enqueue_agent_message(
        &mut connection,
        &message_input(
            "live-claim",
            "agent-root",
            "agent-child",
            AgentMailboxKind::Message,
        ),
        20,
    )
    .unwrap();
    let live_mailbox = claim_next_agent_message(&mut connection, "agent-child", "live-mailbox", 21)
        .unwrap()
        .unwrap();
    enqueue_agent_wake(&mut connection, &wake_input("blocked-oldest"), 22).unwrap();
    enqueue_agent_wake(
        &mut connection,
        &EnqueueAgentWakeInput {
            wake_id: "wake-later-agent".to_string(),
            root_agent_id: "agent-root".to_string(),
            agent_id: "agent-grand".to_string(),
            requester_agent_id: "agent-child".to_string(),
            request_id: "request-later-agent".to_string(),
            source_agent_message_id: None,
        },
        23,
    )
    .unwrap();

    let later = claim_next_dispatchable_agent_wake(&mut connection, "claim-later-agent", 24)
        .unwrap()
        .unwrap();
    assert_eq!(later.wake_id, "wake-later-agent");
    transition_agent_wake(
        &mut connection,
        &later.wake_id,
        AgentWakeStatus::Claimed,
        AgentWakeStatus::Cancelled,
        Some("claim-later-agent"),
        25,
    )
    .unwrap();

    let recovered_oldest = claim_next_dispatchable_agent_wake(
        &mut connection,
        "claim-oldest-after-mailbox-expiry",
        live_mailbox.lease_expires_at.unwrap(),
    )
    .unwrap()
    .unwrap();
    assert_eq!(recovered_oldest.wake_id, "wake-blocked-oldest");
}

#[test]
fn expired_running_wake_becomes_outcome_unknown_and_releases_conversation() {
    let mut connection = setup_tree();
    let followup = follow_up_agent(
        &mut connection,
        &SendAgentMessageRequest {
            sender_agent_id: "agent-root".to_string(),
            recipient_agent_id: "agent-child".to_string(),
            request_id: "crash-followup".to_string(),
            content: "perform side-effecting review".to_string(),
        },
        20,
    )
    .unwrap();
    let wake_id = followup.deferred_wake.unwrap().wake_id;
    let claimed = claim_next_dispatchable_agent_wake(&mut connection, "claim-before-crash", 21)
        .unwrap()
        .unwrap();
    assert_eq!(claimed.wake_id, wake_id);
    connection
        .execute(
            "INSERT INTO messages (
                     id, conversation_id, role, content, status, created_at, position
                 ) VALUES (
                     'assistant-crashed', 'conversation-child', 'assistant',
                     'Working...', 'pending', 22,
                     (SELECT COALESCE(MAX(position), -1) + 1 FROM messages
                      WHERE conversation_id = 'conversation-child')
                 )",
            [],
        )
        .unwrap();
    let trace = crate::ConversationTraceSnapshot::default().in_progress_trace(
        "run-crashed",
        "conversation-child",
        "assistant-crashed",
    );
    crate::storage::conversation_trace_repository::append_in_progress_trace(
        &mut connection,
        &trace,
        22,
        22,
    )
    .unwrap();
    connection
        .execute(
            "UPDATE agent_wake_requests
                 SET status = 'running', status_revision = status_revision + 1,
                     run_id = 'run-crashed', assistant_message_id = 'assistant-crashed',
                     started_at = 22
                 WHERE wake_id = ?1",
            [&wake_id],
        )
        .unwrap();
    let revision_before_recovery = get_agent_wake(&connection, &wake_id)
        .unwrap()
        .unwrap()
        .status_revision;

    let recovered = recover_agent_wakes(&mut connection, "replacement-host", 60_022).unwrap();
    assert_eq!(recovered.actions.len(), 1);
    let AgentWakeRecoveryAction::OutcomeUnknown(rebound) = &recovered.actions[0] else {
        panic!("possibly dispatched Turn must not be replayed");
    };
    assert_ne!(rebound.claim_token.as_deref(), Some("claim-before-crash"));
    assert_eq!(rebound.status_revision, revision_before_recovery);
    finish_agent_turn_with_result(
        &mut connection,
        &FinishAgentTurnResultInput {
            wake_id: wake_id.clone(),
            expected_status: AgentWakeStatus::Running,
            claim_token: rebound.claim_token.clone().unwrap(),
            terminal_status: AgentWakeStatus::OutcomeUnknown,
            run_id: Some("run-crashed".to_string()),
            assistant_message_id: Some("assistant-crashed".to_string()),
            summary: "result unknown after Host crash".to_string(),
            terminal_error: Some("possibly dispatched; not replayed".to_string()),
        },
        60_023,
    )
    .unwrap();
    assert_eq!(
        crate::storage::conversation_trace_repository::get_trace_for_message(
            &connection,
            "assistant-crashed"
        )
        .unwrap()
        .unwrap()
        .terminal_status,
        crate::ConversationTurnTraceTerminalStatus::Failed
    );

    let next = follow_up_agent(
        &mut connection,
        &SendAgentMessageRequest {
            sender_agent_id: "agent-root".to_string(),
            recipient_agent_id: "agent-child".to_string(),
            request_id: "after-crash-followup".to_string(),
            content: "continue safely".to_string(),
        },
        60_024,
    )
    .unwrap();
    let claimed_next =
        claim_next_dispatchable_agent_wake(&mut connection, "claim-after-crash", 60_025)
            .unwrap()
            .unwrap();
    assert_eq!(
        Some(claimed_next.wake_id),
        next.deferred_wake.map(|wake| wake.wake_id)
    );
}

#[test]
fn child_binding_requires_a_fresh_matching_conversation_and_freezes_only_its_model() {
    let mut connection = connection();
    insert_project(&connection, "project-a");
    insert_conversation(&connection, "conversation-root", Some("project-a"));
    ensure_root(&mut connection, "agent-root", "conversation-root");

    insert_conversation(
        &connection,
        "conversation-model-mismatch",
        Some("project-a"),
    );
    let mut mismatched = child_input(
        "agent-model-mismatch",
        "agent-root",
        "agent-root",
        "conversation-model-mismatch",
        "model_mismatch",
        "/root/model_mismatch",
    );
    mismatched.model_snapshot = model_snapshot("model-b");
    assert!(matches!(
        create_agent_node(&mut connection, &mismatched, 11),
        Err(AgentGraphError::Conflict(reason))
            if reason.contains("model does not match")
    ));
    assert!(get_agent_node(&connection, "agent-model-mismatch")
        .unwrap()
        .is_none());

    insert_conversation(&connection, "conversation-preloaded", Some("project-a"));
    connection
        .execute(
            "INSERT INTO messages (
                     id, conversation_id, role, content, status, created_at, position
                 ) VALUES (
                     'preloaded-human', 'conversation-preloaded', 'user',
                     'existing human history', 'sent', 1, 0
                 )",
            [],
        )
        .unwrap();
    let preloaded = child_input(
        "agent-preloaded",
        "agent-root",
        "agent-root",
        "conversation-preloaded",
        "preloaded",
        "/root/preloaded",
    );
    assert!(matches!(
        create_agent_node(&mut connection, &preloaded, 12),
        Err(AgentGraphError::Conflict(reason)) if reason.contains("fresh Conversation")
    ));
    assert!(get_agent_node(&connection, "agent-preloaded")
        .unwrap()
        .is_none());

    insert_conversation(&connection, "conversation-fresh-child", Some("project-a"));
    create_agent_node(
        &mut connection,
        &child_input(
            "agent-fresh-child",
            "agent-root",
            "agent-root",
            "conversation-fresh-child",
            "fresh_child",
            "/root/fresh_child",
        ),
        13,
    )
    .unwrap();

    let child_before = chat_repository::get_conversation(&connection, "conversation-fresh-child")
        .unwrap()
        .unwrap();
    assert!(chat_repository::save_conversation_meta(
        &connection,
        &ChatConversationMetaRecord {
            id: child_before.id.clone(),
            project_id: child_before.project_id.clone(),
            model_id: Some("model-b".to_string()),
            title: child_before.title.clone(),
            created_at: child_before.created_at,
            updated_at: 20,
            pinned_at: child_before.pinned_at,
            archived_at: child_before.archived_at,
            unread_at: child_before.unread_at,
        },
    )
    .is_err());
    let mut child_full_save = child_before;
    child_full_save.model_id = Some("model-b".to_string());
    child_full_save.updated_at = 21;
    assert!(chat_repository::save_conversation(&mut connection, child_full_save).is_err());
    assert_eq!(
        chat_repository::get_conversation(&connection, "conversation-fresh-child")
            .unwrap()
            .unwrap()
            .model_id
            .as_deref(),
        Some("model-a")
    );

    let mut root = chat_repository::get_conversation(&connection, "conversation-root")
        .unwrap()
        .unwrap();
    root.model_id = Some("model-b".to_string());
    root.updated_at = 22;
    chat_repository::save_conversation(&mut connection, root).unwrap();
    assert_eq!(
        chat_repository::get_conversation(&connection, "conversation-root")
            .unwrap()
            .unwrap()
            .model_id
            .as_deref(),
        Some("model-b")
    );
}
