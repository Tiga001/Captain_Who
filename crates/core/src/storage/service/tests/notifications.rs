use super::*;

fn notification(
    conversation_id: &str,
    user_message_id: &str,
    assistant_message_id: &str,
    kind: &str,
) -> notification_repository::NewNotificationEventRecord {
    notification_repository::NewNotificationEventRecord {
        notification_kind: kind.to_string(),
        source_kind: "human_root".to_string(),
        source_id: "run-notification-atomic".to_string(),
        run_id: Some("run-notification-atomic".to_string()),
        automation_id: None,
        conversation_id: Some(conversation_id.to_string()),
        user_message_id: Some(user_message_id.to_string()),
        assistant_message_id: Some(assistant_message_id.to_string()),
        approval_action_id: None,
        subject_kind: "prompt_excerpt".to_string(),
        subject_text: "Safe prompt identity".to_string(),
        dedupe_key: format!("notification-atomic-{kind}"),
        supersession_key: "human-root:run-notification-atomic".to_string(),
        resource_revision: Some(3),
        occurred_at: 3,
        expires_at: i64::MAX,
    }
}

fn seed_notification_turn(storage: &StorageService) {
    storage
        .save_conversation(ChatConversationRecord {
            id: "conversation-notification-atomic".to_string(),
            project_id: None,
            model_id: Some("model-1".to_string()),
            title: "Atomic notification".to_string(),
            messages: vec![
                ChatMessageRecord {
                    human_interaction_response: None,
                    id: "user-notification-atomic".to_string(),
                    role: "user".to_string(),
                    content: "Safe prompt identity".to_string(),
                    created_at: 1,
                    status: Some("sent".to_string()),
                    attachments: Vec::new(),
                    folder_references_json: None,
                    agent_run_json: None,
                    ui_state_json: None,
                },
                ChatMessageRecord {
                    human_interaction_response: None,
                    id: "assistant-notification-atomic".to_string(),
                    role: "assistant".to_string(),
                    content: "Thinking".to_string(),
                    created_at: 2,
                    status: Some("pending".to_string()),
                    attachments: Vec::new(),
                    folder_references_json: None,
                    agent_run_json: None,
                    ui_state_json: None,
                },
            ],
            created_at: 1,
            updated_at: 2,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
    storage
        .append_in_progress_conversation_turn_trace(
            &ConversationTurnTrace {
                schema_version: crate::CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
                run_id: "run-notification-atomic".to_string(),
                conversation_id: "conversation-notification-atomic".to_string(),
                assistant_message_id: "assistant-notification-atomic".to_string(),
                terminal_status: crate::ConversationTurnTraceTerminalStatus::InProgress,
                terminal_error: None,
                truncated: false,
                items: Vec::new(),
            },
            2,
            2,
        )
        .unwrap();
}

fn seed_command_approval(storage: &StorageService) -> AgentPendingActionRecord {
    seed_notification_turn(storage);
    let mut pending = pending_action("command-approval", "conversation-notification-atomic");
    pending.run_id = "run-notification-atomic".to_string();
    pending.assistant_message_id = Some("assistant-notification-atomic".to_string());
    pending.agent_input_json =
        serde_json::json!({"checkpoint": "frozen".repeat(16_384)}).to_string();
    let mut event = notification(
        "conversation-notification-atomic",
        "user-notification-atomic",
        "assistant-notification-atomic",
        "approval_required",
    );
    event.approval_action_id = pending.tool_call_id.clone();
    storage
        .store_pending_agent_action_with_notification(pending.clone(), &event)
        .unwrap();
    pending
}

#[test]
fn command_approval_preserves_checkpoint_and_claims_exact_identity_once() {
    let fixture = StorageFixture::new();
    let storage = fixture.service();
    let pending = seed_command_approval(&storage);
    storage
        .state
        .connection()
        .unwrap()
        .execute_batch(
            "CREATE TRIGGER forbid_checkpoint_rewrite BEFORE UPDATE OF agent_input_json
         ON agent_pending_actions BEGIN SELECT RAISE(ABORT, 'frozen checkpoint rewritten'); END;",
        )
        .unwrap();
    for (run, conversation, message, call) in [
        (
            "other-run",
            "conversation-notification-atomic",
            "assistant-notification-atomic",
            "command-approval",
        ),
        (
            "run-notification-atomic",
            "other-conversation",
            "assistant-notification-atomic",
            "command-approval",
        ),
        (
            "run-notification-atomic",
            "conversation-notification-atomic",
            "other-message",
            "command-approval",
        ),
        (
            "run-notification-atomic",
            "conversation-notification-atomic",
            "assistant-notification-atomic",
            "other-call",
        ),
    ] {
        assert!(storage
            .approve_pending_command_and_resolve_notification(
                &pending.action_id,
                run,
                Some(conversation),
                Some(message),
                call,
                4,
            )
            .is_err());
    }
    let approve = || {
        storage.approve_pending_command_and_resolve_notification(
            &pending.action_id,
            &pending.run_id,
            pending.conversation_id.as_deref(),
            pending.assistant_message_id.as_deref(),
            pending.tool_call_id.as_deref().unwrap(),
            4,
        )
    };
    approve().unwrap();
    assert!(
        approve().is_err(),
        "one durable action grants dispatch authority only once"
    );
    let after = storage
        .get_pending_agent_action(&pending.action_id)
        .unwrap()
        .unwrap();
    assert_eq!(after.status, "approved");
    assert_eq!(after.agent_input_json, pending.agent_input_json);
    assert_eq!(after.action_json, pending.action_json);
    let resolved: bool = storage
        .state
        .connection()
        .unwrap()
        .query_row(
            "SELECT resolved_at IS NOT NULL FROM notification_events WHERE approval_action_id = ?1",
            [&pending.action_id],
            |row| row.get(0),
        )
        .unwrap();
    assert!(resolved);
}

#[test]
fn command_approval_notification_failure_rolls_back_dispatch_authority() {
    let fixture = StorageFixture::new();
    let storage = fixture.service();
    let pending = seed_command_approval(&storage);
    storage
        .state
        .connection()
        .unwrap()
        .execute_batch(
            "CREATE TRIGGER fail_approval_resolution BEFORE UPDATE OF resolved_at
         ON notification_events BEGIN SELECT RAISE(ABORT, 'resolution failed'); END;",
        )
        .unwrap();
    assert!(storage
        .approve_pending_command_and_resolve_notification(
            &pending.action_id,
            &pending.run_id,
            pending.conversation_id.as_deref(),
            pending.assistant_message_id.as_deref(),
            pending.tool_call_id.as_deref().unwrap(),
            4,
        )
        .is_err());
    let after = storage
        .get_pending_agent_action(&pending.action_id)
        .unwrap()
        .unwrap();
    assert_eq!(after.status, "pending");
    assert_eq!(after.agent_input_json, pending.agent_input_json);
}

#[test]
fn concurrent_command_approval_grants_dispatch_once_and_respects_tree_stop() {
    let fixture = StorageFixture::new();
    let storage = fixture.service();
    let pending = seed_command_approval(&storage);
    let barrier = std::sync::Barrier::new(2);
    let outcomes = std::thread::scope(|threads| {
        let handles = (0..2)
            .map(|_| {
                threads.spawn(|| {
                    barrier.wait();
                    storage
                        .approve_pending_command_and_resolve_notification(
                            &pending.action_id,
                            &pending.run_id,
                            pending.conversation_id.as_deref(),
                            pending.assistant_message_id.as_deref(),
                            pending.tool_call_id.as_deref().unwrap(),
                            4,
                        )
                        .is_ok()
                })
            })
            .collect::<Vec<_>>();
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert_eq!(outcomes.iter().filter(|approved| **approved).count(), 1);

    let mut second = pending.clone();
    second.action_id = "command-after-stop".to_string();
    second.tool_call_id = Some("command-after-stop".to_string());
    storage.store_pending_agent_action(second.clone()).unwrap();
    bind_agent_root(
        &storage,
        "agent-approval-root",
        "conversation-notification-atomic",
    );
    storage
        .begin_agent_tree_run_stop_by_root_conversation_at(
            "conversation-notification-atomic",
            &pending.run_id,
            5,
        )
        .unwrap()
        .unwrap();
    assert!(storage
        .approve_pending_command_and_resolve_notification(
            &second.action_id,
            &second.run_id,
            second.conversation_id.as_deref(),
            second.assistant_message_id.as_deref(),
            second.tool_call_id.as_deref().unwrap(),
            6,
        )
        .is_err());
    assert_eq!(
        storage
            .get_pending_agent_action(&second.action_id)
            .unwrap()
            .unwrap()
            .status,
        "pending"
    );
}

#[test]
fn invalid_notification_rolls_back_the_terminal_message_and_trace() {
    let fixture = StorageFixture::new();
    let storage = fixture.service();
    seed_notification_turn(&storage);
    let terminal_trace = ConversationTurnTrace {
        schema_version: crate::CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
        run_id: "run-notification-atomic".to_string(),
        conversation_id: "conversation-notification-atomic".to_string(),
        assistant_message_id: "assistant-notification-atomic".to_string(),
        terminal_status: crate::ConversationTurnTraceTerminalStatus::Completed,
        terminal_error: None,
        truncated: false,
        items: Vec::new(),
    };
    let invalid = notification(
        "conversation-notification-atomic",
        "user-notification-atomic",
        "assistant-notification-atomic",
        "not_a_notification_kind",
    );

    assert!(storage
        .finalize_chat_message_with_conversation_turn_notification(
            "conversation-notification-atomic",
            "assistant-notification-atomic",
            "finished",
            Some("sent"),
            "completed",
            &terminal_trace,
            None,
            2,
            3,
            None,
            &invalid,
            None,
        )
        .is_err());

    let conversation = storage
        .load_conversation("conversation-notification-atomic")
        .unwrap()
        .unwrap();
    let assistant = conversation
        .messages
        .iter()
        .find(|message| message.id == "assistant-notification-atomic")
        .unwrap();
    assert_eq!(assistant.content, "Thinking");
    assert_eq!(assistant.status.as_deref(), Some("pending"));
    assert_eq!(
        storage
            .get_conversation_turn_trace("assistant-notification-atomic")
            .unwrap()
            .unwrap()
            .terminal_status,
        crate::ConversationTurnTraceTerminalStatus::InProgress
    );
    assert!(storage
        .list_notifications(None, 100, false, None)
        .unwrap()
        .items
        .is_empty());
}

#[test]
fn invalid_approval_notification_rolls_back_the_pending_action() {
    let fixture = StorageFixture::new();
    let storage = fixture.service();
    storage
        .save_conversation(conversation(
            "conversation-notification-pending",
            None,
            "user-notification-pending",
        ))
        .unwrap();
    let pending = pending_action(
        "pending-notification-atomic",
        "conversation-notification-pending",
    );
    let invalid = notification(
        "conversation-notification-pending",
        "user-notification-pending",
        "assistant-notification-pending",
        "not_a_notification_kind",
    );

    assert!(storage
        .store_pending_agent_action_with_notification(pending, &invalid)
        .is_err());
    assert!(storage.list_pending_agent_actions().unwrap().is_empty());
    assert!(storage
        .list_notifications(None, 100, false, None)
        .unwrap()
        .items
        .is_empty());
}

#[test]
fn deleting_a_conversation_resolves_its_notification_without_deleting_history() {
    let fixture = StorageFixture::new();
    let storage = fixture.service();
    storage
        .save_conversation(conversation(
            "conversation-notification-delete",
            None,
            "user-notification-delete",
        ))
        .unwrap();
    let mut event = notification(
        "conversation-notification-delete",
        "user-notification-delete",
        "assistant-notification-delete",
        "task_completed",
    );
    event.dedupe_key = "notification-delete".to_string();
    storage.enqueue_notification_event(&event).unwrap();

    storage
        .delete_conversation("conversation-notification-delete")
        .unwrap();

    let page = storage.list_notifications(None, 100, false, None).unwrap();
    assert!(
        page.items.is_empty(),
        "invalid deep links stay out of the user list"
    );
    let connection = rusqlite::Connection::open(fixture.root.join("storage.sqlite")).unwrap();
    let durable_fact: (bool, bool) = connection
        .query_row(
            "SELECT resolved_at IS NOT NULL, superseded_at IS NOT NULL
             FROM notification_events WHERE dedupe_key = 'notification-delete'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(durable_fact, (true, true));
}
