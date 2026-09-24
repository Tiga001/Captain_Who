use super::*;

#[test]
fn result_wake_input_and_search_history_only_expose_semantic_task_identity() {
    let mut connection = setup_tree();
    add_grandchild(&mut connection);
    let followup = follow_up_agent(
        &mut connection,
        &SendAgentMessageRequest {
            sender_agent_id: "agent-child".to_string(),
            recipient_agent_id: "agent-grand".to_string(),
            request_id: "nested-result-input".to_string(),
            content: "perform nested check".to_string(),
        },
        30,
    )
    .unwrap();
    let wake_id = followup.deferred_wake.unwrap().wake_id;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();
    project_agent_wake_source_in_transaction(&transaction, &wake_id, "nested-project", 31).unwrap();
    transaction.commit().unwrap();
    claim_next_agent_wake(&mut connection, "agent-grand", "nested-claim", 32)
        .unwrap()
        .unwrap();
    let settled = finish_agent_turn_with_result(
        &mut connection,
        &FinishAgentTurnResultInput {
            wake_id: wake_id.clone(),
            expected_status: AgentWakeStatus::Claimed,
            claim_token: "nested-claim".to_string(),
            terminal_status: AgentWakeStatus::Failed,
            run_id: None,
            assistant_message_id: None,
            terminal_error: Some("The delegated operation was not started.".to_string()),
        },
        33,
    )
    .unwrap();
    let parent_wake = settled.parent_wake.unwrap();
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();
    project_agent_wake_source_in_transaction(
        &transaction,
        &parent_wake.wake_id,
        "nested-result-project",
        34,
    )
    .unwrap();
    transaction.commit().unwrap();
    let trusted = resolve_child_wake_bundle(
        &connection,
        "agent-child",
        &parent_wake.wake_id,
        &settled.result_message.message_id,
    )
    .unwrap();
    assert_eq!(
        trusted.collaboration_identity.source_kind,
        AgentMailboxKind::Result
    );
    let entrusted = &trusted.collaboration_identity.entrusted_task;
    let projected_content: String = connection
        .query_row(
            "SELECT content FROM messages WHERE id = ?1",
            [&settled.result_message.projection_message_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(&projected_content, entrusted);
    assert!(entrusted.contains("agent-grand"));
    let model_content = crate::storage::agent_message_model_projection::project_message(
        &connection,
        "conversation-child",
        &settled.result_message.projection_message_id,
    )
    .unwrap()
    .unwrap();
    let wrapper: serde_json::Value = serde_json::from_str(&model_content).unwrap();
    assert_eq!(wrapper["type"], "agent_collaboration_input");
    assert_eq!(wrapper["senderTaskName"], "details");
    let payload: serde_json::Value =
        serde_json::from_str(wrapper["payload"].as_str().unwrap()).unwrap();
    assert_eq!(payload["taskName"], "details");
    assert_eq!(payload["status"], "failed");
    assert_eq!(payload["summary"], "子智能体本轮失败。");
    assert_eq!(
        payload["terminalError"],
        "The delegated operation was not started."
    );
    for hidden in [
        "agent-grand",
        "agent-child",
        "/root/review/details",
        &wake_id,
    ] {
        assert!(
            !model_content.contains(hidden),
            "result Wake input leaked {hidden}"
        );
    }

    let raw_message = get_agent_message(&connection, &settled.result_message.message_id)
        .unwrap()
        .unwrap();
    assert_eq!(raw_message.content, settled.result_message.content);
    assert!(raw_message.content.contains("agent-grand"));
    let mut history = crate::storage::conversation_history_repository::read_record(
        &connection,
        "conversation-child",
        &crate::storage::conversation_history_repository::ConversationHistoryRecordRef::Message {
            message_id: settled.result_message.projection_message_id.clone(),
        },
    )
    .unwrap()
    .unwrap();
    assert!(history.serialized_json.contains("agent-grand"));
    crate::storage::agent_message_model_projection::project_history_record(
        &connection,
        "conversation-child",
        &mut history,
    )
    .unwrap();
    assert!(!history.serialized_json.contains("agent-grand"));
    let filter = crate::storage::conversation_history_repository::ConversationHistorySearchFilter {
        include_messages: true,
        ..Default::default()
    };
    let mut hits = crate::storage::conversation_history_repository::search_records(
        &connection,
        "conversation-child",
        "子智能体本轮失败",
        &filter,
        8,
    )
    .unwrap();
    assert_eq!(hits.len(), 1);
    for hit in &mut hits {
        crate::storage::agent_message_model_projection::project_history_preview(
            &connection,
            "conversation-child",
            &hit.reference,
            &mut hit.preview,
            &mut hit.preview_truncated,
        )
        .unwrap();
    }
    assert!(!serde_json::to_string(&hits)
        .unwrap()
        .contains("agent-grand"));
    let mut private_matches = crate::storage::conversation_history_repository::search_records(
        &connection,
        "conversation-child",
        "agent-grand",
        &filter,
        8,
    )
    .unwrap();
    assert_eq!(private_matches.len(), 1);
    for hit in &mut private_matches {
        crate::storage::agent_message_model_projection::project_history_preview(
            &connection,
            "conversation-child",
            &hit.reference,
            &mut hit.preview,
            &mut hit.preview_truncated,
        )
        .unwrap();
    }
    assert!(!serde_json::to_string(&private_matches)
        .unwrap()
        .contains("agent-grand"));

    // Detached snapshots do not need a surviving source node/Mailbox. Their frozen Host receipt
    // and typed sender binding are sufficient; ordinary message-shaped JSON is never converted.
    assert_eq!(
        crate::storage::agent_message_model_projection::project_snapshot_result(
            "agent-grand",
            &settled.result_message.message_id,
            &projected_content,
        )
        .unwrap()
        .unwrap(),
        model_content,
    );
    assert!(
        crate::storage::agent_message_model_projection::project_snapshot_result(
            "agent-grand",
            "mailbox-ordinary-message",
            &projected_content,
        )
        .unwrap()
        .is_none()
    );
    assert!(
        crate::storage::agent_message_model_projection::project_snapshot_result(
            "wrong-sender",
            &settled.result_message.message_id,
            &projected_content,
        )
        .is_err()
    );

    connection.execute(
        "INSERT INTO messages (id, conversation_id, role, content, status, created_at, position)
         VALUES ('snapshot-source-assistant', 'conversation-child', 'assistant', 'Result received', 'sent', 35, 1)",
        [],
    ).unwrap();
    crate::storage::conversation_trace_repository::replace_trace(
        &mut connection,
        &crate::completed_conversation_trace_without_items(
            "snapshot-source-run",
            "conversation-child",
            "snapshot-source-assistant",
        ),
        35,
        35,
    )
    .unwrap();
    insert_conversation(&connection, "conversation-snapshot", Some("project-a"));
    create_agent_node(
        &mut connection,
        &child_input(
            "agent-snapshot",
            "agent-root",
            "agent-child",
            "conversation-snapshot",
            "snapshot-review",
            "/root/review/snapshot-review",
        ),
        36,
    )
    .unwrap();
    let plan =
        crate::storage::child_context_snapshot_repository::build_child_context_snapshot_plan(
            &connection,
            "conversation-child",
            "conversation-snapshot",
            &crate::AgentForkTurns::All,
            37,
        )
        .unwrap();
    crate::storage::child_context_snapshot_repository::apply_child_context_snapshot_in_transaction(
        &connection,
        &plan,
    )
    .unwrap();
    let snapshot_message_id: String = connection.query_row(
        "SELECT id FROM messages WHERE conversation_id = 'conversation-snapshot' AND role = 'user'",
        [], |row| row.get(0),
    ).unwrap();
    assert_eq!(
        crate::storage::agent_message_model_projection::project_message(
            &connection,
            "conversation-snapshot",
            &snapshot_message_id,
        )
        .unwrap()
        .unwrap(),
        model_content,
    );

    let fixture = tempfile::tempdir().unwrap();
    let database_path = fixture.path().join("result-model-views.sqlite");
    connection
        .execute("VACUUM INTO ?1", [database_path.to_str().unwrap()])
        .unwrap();
    let storage = crate::storage::service::StorageService::open(&database_path).unwrap();
    for (conversation_id, message_id) in [
        (
            "conversation-child",
            settled.result_message.projection_message_id.as_str(),
        ),
        ("conversation-snapshot", snapshot_message_id.as_str()),
    ] {
        let mut messages = vec![crate::AgentChatMessage {
            conversation_completion_covered: false,
            message_id: Some(message_id.to_string()),
            role: "user".to_string(),
            content: projected_content.clone(),
            created_at: Some(34),
            conversation_turn_trace: None,
            conversation_model_context_items: Vec::new(),
        }];
        storage
            .project_agent_messages_for_model(conversation_id, &mut messages)
            .unwrap();
        assert_eq!(messages[0].content, model_content);
        storage
            .project_agent_messages_for_model(conversation_id, &mut messages)
            .unwrap();
        assert_eq!(
            messages[0].content, model_content,
            "a repeated projection must not add a second wrapper"
        );

        let cursor = crate::ContextJournalCursor::Message {
            message_id: message_id.to_string(),
        };
        let prefix = crate::ContextCompactionPrefix {
            conversation_id: conversation_id.to_string(),
            source_revision: "canonical-raw-prefix-revision".to_string(),
            covered_through: cursor.clone(),
            previous_summary: None,
            source_items: vec![crate::ContextCompactionSourceItem::Message {
                cursor,
                role: "user".to_string(),
                content: projected_content.clone(),
                created_at: 34,
                status: Some("sent".to_string()),
                terminal_status: None,
                terminal_error: None,
            }],
        };
        let canonical = serde_json::to_string(&prefix).unwrap();
        let model_prefix = storage
            .project_context_compaction_prefix_for_model(&prefix)
            .unwrap();
        assert_eq!(model_prefix.source_revision, prefix.source_revision);
        assert_eq!(model_prefix.covered_through, prefix.covered_through);
        assert!(!serde_json::to_string(&model_prefix)
            .unwrap()
            .contains("agent-grand"));
        assert_eq!(serde_json::to_string(&prefix).unwrap(), canonical);

        let reference = crate::storage::conversation_history_repository::ConversationHistoryRecordRef::Message {
            message_id: message_id.to_string(),
        };
        let record = storage
            .read_conversation_history_record(conversation_id, &reference)
            .unwrap()
            .unwrap();
        assert!(!record.serialized_json.contains("agent-grand"));
        let hits = storage
            .search_conversation_history(conversation_id, "agent-grand", &filter, 8)
            .unwrap();
        assert!(!hits.is_empty());
        assert!(!serde_json::to_string(&hits)
            .unwrap()
            .contains("agent-grand"));
        let around = storage
            .conversation_history_around(conversation_id, &reference, 1, 1)
            .unwrap()
            .unwrap();
        assert!(!serde_json::to_string(&around)
            .unwrap()
            .contains("agent-grand"));
        let range = storage
            .conversation_history_range(conversation_id, &reference, &reference, 8)
            .unwrap()
            .unwrap();
        assert!(!serde_json::to_string(&range)
            .unwrap()
            .contains("agent-grand"));
        let mut history = storage.load_conversation(conversation_id).unwrap().unwrap();
        assert!(history
            .messages
            .iter()
            .any(|message| message.content.contains("agent-grand")));
        storage
            .project_history_conversation_for_model(&mut history)
            .unwrap();
        assert!(history
            .messages
            .iter()
            .all(|message| !message.content.contains("agent-grand")));
        assert!(storage
            .load_conversation(conversation_id)
            .unwrap()
            .unwrap()
            .messages
            .iter()
            .any(|message| message.content.contains("agent-grand")));
    }
}

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
    crate::storage::agent_workspace_repository::freeze_run(
        &connection,
        "run-grand",
        Some(&wake_id),
        None,
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
    assert_eq!(settled.envelope.summary, "子智能体本轮已完成。");
    assert!(!settled
        .result_message
        .content
        .contains("nested review complete"));
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
        crate::storage::agent_workspace_repository::freeze_run(
            &connection,
            &run_id,
            Some(&wake_id),
            None,
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

#[test]
fn manual_root_maintenance_leaves_child_wake_queued_until_terminal() {
    use crate::storage::manual_context_compaction_repository as manual;
    use crate::storage::models::ManualContextCompactionOperation;
    let mut connection = setup_tree();
    let mut operation = ManualContextCompactionOperation {
        operation_id: "root-maintenance".into(),
        request_id: "root-maintenance".into(),
        conversation_id: "conversation-root".into(),
        status: "running".into(),
        phase: "preparing".into(),
        assistant_message_id: None,
        covered_through_message_id: None,
        model_id: None,
        summary_id: None,
        source_input_tokens: None,
        replacement_input_tokens: None,
        error: None,
        started_at: 20,
        updated_at: 20,
        completed_at: None,
    };
    manual::claim(&mut connection, &operation).unwrap();
    let followup = follow_up_agent(
        &mut connection,
        &SendAgentMessageRequest {
            sender_agent_id: "agent-root".into(),
            recipient_agent_id: "agent-child".into(),
            request_id: "during-maintenance".into(),
            content: "check after maintenance".into(),
        },
        21,
    )
    .unwrap();
    let wake_id = followup.deferred_wake.unwrap().wake_id;
    assert!(
        claim_next_dispatchable_agent_wake(&mut connection, "global-claim-maintenance", 22)
            .unwrap()
            .is_none()
    );
    assert!(claim_next_agent_wake(
        &mut connection,
        "agent-child",
        "direct-claim-maintenance",
        22
    )
    .unwrap()
    .is_none());
    assert_eq!(
        get_agent_wake(&connection, &wake_id)
            .unwrap()
            .unwrap()
            .status,
        AgentWakeStatus::Queued
    );
    operation.status = "cancelled".into();
    operation.updated_at = 23;
    operation.completed_at = Some(23);
    manual::update(&connection, &operation).unwrap();
    let claimed =
        claim_next_dispatchable_agent_wake(&mut connection, "global-claim-after-maintenance", 24)
            .unwrap()
            .unwrap();
    assert_eq!(claimed.wake_id, wake_id);
    assert_eq!(claimed.status, AgentWakeStatus::Claimed);
}

#[test]
fn long_explicit_report_precedes_the_host_terminal_notification() {
    let mut connection = setup_tree();
    let task = follow_up_agent(
        &mut connection,
        &SendAgentMessageRequest {
            sender_agent_id: "agent-root".into(),
            recipient_agent_id: "agent-child".into(),
            request_id: "large-result-task".into(),
            content: "collect evidence".into(),
        },
        30,
    )
    .unwrap();
    let wake = task.deferred_wake.unwrap();
    let transaction = connection.transaction().unwrap();
    project_agent_wake_source_in_transaction(
        &transaction,
        &wake.wake_id,
        "large-result-projection",
        31,
    )
    .unwrap();
    transaction.commit().unwrap();
    claim_next_agent_wake(&mut connection, "agent-child", "large-result-claim", 32)
        .unwrap()
        .unwrap();
    let report = "完整研究结论🙂".repeat(90_000);
    let sent = send_agent_message(
        &mut connection,
        &SendAgentMessageRequest {
            sender_agent_id: "agent-child".into(),
            recipient_agent_id: "agent-root".into(),
            request_id: "explicit-report".into(),
            content: report.clone(),
        },
        33,
    )
    .unwrap();
    assert!(sent.deferred_wake.is_none());
    let input = FinishAgentTurnResultInput {
        wake_id: wake.wake_id,
        expected_status: AgentWakeStatus::Claimed,
        claim_token: "large-result-claim".into(),
        terminal_status: AgentWakeStatus::Failed,
        run_id: None,
        assistant_message_id: None,
        terminal_error: Some("fixture failure after collecting evidence".into()),
    };
    let settled = finish_agent_turn_with_result(&mut connection, &input, 34).unwrap();
    assert!(settled.parent_wake.is_none());
    let stored = get_agent_message(&connection, &settled.result_message.message_id)
        .unwrap()
        .unwrap();
    let envelope: crate::AgentTurnResultEnvelope = serde_json::from_str(&stored.content).unwrap();
    assert_eq!(envelope.summary, "子智能体本轮失败。");
    assert!(!stored.content.contains("完整研究结论"));
    let (projected, truncated) = crate::conversation_trace::project_agent_mailbox_model_envelope(
        &envelope.child_agent_id,
        &envelope.task_name,
        &envelope.task_path,
        crate::AgentMailboxKind::Result,
        &stored.content,
    )
    .unwrap();
    assert!(!truncated);
    let wrapper: serde_json::Value = serde_json::from_str(&projected).unwrap();
    let body: serde_json::Value =
        serde_json::from_str(wrapper["payload"].as_str().unwrap()).unwrap();
    assert_eq!(body["summary"], "子智能体本轮失败。");
    assert_eq!(
        body["terminalError"],
        "fixture failure after collecting evidence"
    );
    let transaction = connection.transaction().unwrap();
    let delivered = project_pending_agent_messages_in_transaction(
        &transaction,
        "agent-root",
        "report-delivery",
        35,
        2,
    )
    .unwrap();
    transaction.commit().unwrap();
    assert_eq!(delivered.len(), 2);
    assert_eq!(delivered[0].message_id, sent.message.message_id);
    assert_eq!(delivered[0].content, report);
    assert_eq!(delivered[1].message_id, settled.result_message.message_id);
    let (report_for_model, _) = crate::conversation_trace::project_agent_mailbox_model_envelope(
        "agent-child",
        "review",
        "/root/review",
        AgentMailboxKind::Message,
        &delivered[0].content,
    )
    .unwrap();
    let wrapper: serde_json::Value = serde_json::from_str(&report_for_model).unwrap();
    assert_eq!(wrapper["payload"], report);
    assert_eq!(wrapper["payloadTruncated"], false);
}
