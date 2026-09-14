use super::*;

fn seed_turn(
    storage: &StorageService,
    conversation_id: &str,
    run_id: &str,
    user_message_id: &str,
    assistant_message_id: &str,
    prompt: &str,
) {
    storage
        .save_conversation(ChatConversationRecord {
            id: conversation_id.to_string(),
            project_id: None,
            model_id: Some("model-1".to_string()),
            title: "Notification test".to_string(),
            messages: vec![
                ChatMessageRecord {
                    human_interaction_response: None,
                    id: user_message_id.to_string(),
                    role: "user".to_string(),
                    content: prompt.to_string(),
                    created_at: 1,
                    status: Some("sent".to_string()),
                    attachments: Vec::new(),
                    agent_run_json: None,
                    ui_state_json: None,
                },
                ChatMessageRecord {
                    human_interaction_response: None,
                    id: assistant_message_id.to_string(),
                    role: "assistant".to_string(),
                    content: String::new(),
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
                schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
                run_id: run_id.to_string(),
                conversation_id: conversation_id.to_string(),
                assistant_message_id: assistant_message_id.to_string(),
                terminal_status: ConversationTurnTraceTerminalStatus::InProgress,
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
fn completed_human_root_turn_atomically_publishes_one_safe_notification() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let service = AgentService::new_authorized_for_test(Arc::clone(&storage));
    seed_turn(
        &storage,
        "conversation-notification",
        "run-notification",
        "user-notification",
        "assistant-notification",
        "  制定单元设备\n技术报告方案  ",
    );
    let mut output = AgentChatOutput {
        content: "private model result that must not identify the notification".to_string(),
        status: AgentRunStatus::Completed,
        run_id: "run-notification".to_string(),
        events: Vec::new(),
        tool_definitions: Vec::new(),
        todo: None,
        usage: None,
        finish_reason: Some("private provider finish reason".to_string()),
        proposed_actions: Vec::new(),
        conversation_turn_trace: None,
    };

    service
        .persist_final_assistant_output(
            "conversation-notification",
            "assistant-notification",
            &mut output,
            None,
        )
        .unwrap();
    // A restart/recovery replay derives the same dedupe identity and cannot create a second event.
    let replay = service
        .human_root_terminal_notification(
            "run-notification",
            "conversation-notification",
            "assistant-notification",
            AgentRunStatus::Completed,
            now_ms(),
        )
        .unwrap()
        .unwrap();
    storage.enqueue_notification_event(&replay).unwrap();

    let page = storage.list_notifications(None, 100, false, None).unwrap();
    assert_eq!(page.items.len(), 1);
    let event = &page.items[0];
    assert_eq!(event.notification_kind, "task_completed");
    assert_eq!(event.source_kind, "human_root");
    assert_eq!(event.run_id.as_deref(), Some("run-notification"));
    assert_eq!(
        event.conversation_id.as_deref(),
        Some("conversation-notification")
    );
    assert_eq!(event.user_message_id.as_deref(), Some("user-notification"));
    assert_eq!(
        event.assistant_message_id.as_deref(),
        Some("assistant-notification")
    );
    assert_eq!(event.subject_kind, "prompt_excerpt");
    assert_eq!(event.subject_text, "制定单元设备 技术报告方案");
    assert!(!event.subject_text.contains("private"));
    assert!(event.resolved_at.is_some());
}

#[test]
fn failed_human_root_turn_uses_prompt_identity_and_never_the_error() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let service = AgentService::new_authorized_for_test(Arc::clone(&storage));
    seed_turn(
        &storage,
        "conversation-notification-failed",
        "run-notification-failed",
        "user-notification-failed",
        "assistant-notification-failed",
        "检查构建失败原因",
    );
    let mut output = AgentChatOutput {
        content: "raw provider failure body".to_string(),
        status: AgentRunStatus::Failed,
        run_id: "run-notification-failed".to_string(),
        events: Vec::new(),
        tool_definitions: Vec::new(),
        todo: None,
        usage: None,
        finish_reason: Some("secret upstream diagnostic".to_string()),
        proposed_actions: Vec::new(),
        conversation_turn_trace: None,
    };

    service
        .persist_final_assistant_output(
            "conversation-notification-failed",
            "assistant-notification-failed",
            &mut output,
            None,
        )
        .unwrap();

    let page = storage.list_notifications(None, 100, false, None).unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].notification_kind, "task_failed");
    assert_eq!(page.items[0].subject_text, "检查构建失败原因");
    assert!(!page.items[0].subject_text.contains("secret"));
    assert!(!page.items[0].subject_text.contains("provider"));
    assert!(page.items[0].resolved_at.is_none());
}

#[test]
fn emoji_heavy_prompt_persists_notification_without_rolling_back_terminal_turn() {
    use unicode_segmentation::UnicodeSegmentation;

    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let service = AgentService::new_authorized_for_test(Arc::clone(&storage));
    let prompt = std::iter::repeat_n("👨‍👩‍👧‍👦", 80).collect::<String>();
    seed_turn(
        &storage,
        "conversation-notification-emoji",
        "run-notification-emoji",
        "user-notification-emoji",
        "assistant-notification-emoji",
        &prompt,
    );
    let mut output = AgentChatOutput {
        content: "done".to_string(),
        status: AgentRunStatus::Completed,
        run_id: "run-notification-emoji".to_string(),
        events: Vec::new(),
        tool_definitions: Vec::new(),
        todo: None,
        usage: None,
        finish_reason: Some("stop".to_string()),
        proposed_actions: Vec::new(),
        conversation_turn_trace: None,
    };

    service
        .persist_final_assistant_output(
            "conversation-notification-emoji",
            "assistant-notification-emoji",
            &mut output,
            None,
        )
        .unwrap();

    let event = storage
        .list_notifications(None, 100, false, None)
        .unwrap()
        .items
        .into_iter()
        .find(|event| event.run_id.as_deref() == Some("run-notification-emoji"))
        .unwrap();
    assert!(event.subject_text.len() <= 512);
    assert!(event.subject_text.graphemes(true).count() <= 48);
    assert!(event.subject_text.ends_with('…'));
    assert_eq!(
        storage
            .get_conversation_turn_trace("assistant-notification-emoji")
            .unwrap()
            .unwrap()
            .terminal_status,
        ConversationTurnTraceTerminalStatus::Completed
    );
}

#[test]
fn child_conversation_is_not_classified_as_an_ordinary_human_root_turn() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    storage.save_model_settings(test_model_settings()).unwrap();
    storage
        .save_conversation(ChatConversationRecord {
            id: "conversation-parent-notification".to_string(),
            project_id: None,
            model_id: Some("model-1".to_string()),
            title: "Parent".to_string(),
            messages: Vec::new(),
            created_at: 1,
            updated_at: 1,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
    storage
        .ensure_root_agent(&mycopilot_core::EnsureRootAgentInput {
            agent_id: "agent-parent-notification".to_string(),
            conversation_id: "conversation-parent-notification".to_string(),
            creation_request_id: "ensure-parent-notification".to_string(),
            task_name: "Parent".to_string(),
        })
        .unwrap();
    let child = storage
        .create_child_agent(&mycopilot_core::CreateChildAgentInput {
            parent_agent_id: "agent-parent-notification".to_string(),
            creation_request_id: "child-notification".to_string(),
            task_name: "child_notification".to_string(),
            task: "Internal child work".to_string(),
            template_machine_key: None,
            explicit_model_id: None,
            reasoning_effort: None,
            fork_turns: mycopilot_core::AgentForkTurns::None,
        })
        .unwrap();
    let service = AgentService::new_authorized_for_test(storage);

    assert!(service
        .human_root_notification_context(
            "run-child-notification",
            &child.agent.conversation_id,
            "assistant-child-notification",
        )
        .unwrap()
        .is_none());
}
