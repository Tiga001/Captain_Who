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
