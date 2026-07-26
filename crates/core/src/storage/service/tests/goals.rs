use super::*;
use crate::ConversationGoalStatus;

#[test]
fn ordinary_conversation_has_no_goal_until_explicit_creation() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    service
        .save_conversation(conversation("conversation-goal", None, "message-goal"))
        .unwrap();

    assert_eq!(
        service
            .load_visible_conversation_goal("conversation-goal")
            .unwrap(),
        None
    );

    let goal = service
        .create_conversation_goal(
            "conversation-goal",
            "Ship the explicitly requested long-running migration.",
            2,
        )
        .unwrap();
    assert_eq!(goal.status, ConversationGoalStatus::Active);
    assert_eq!(goal.source_message_id, "message-goal");
    assert!(service
        .create_conversation_goal("conversation-goal", "A second unfinished goal.", 3)
        .unwrap_err()
        .contains("仍存在尚未结束的 Goal"));
}

#[test]
fn blocked_goal_resumes_only_when_a_new_user_turn_arrives() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    service
        .save_conversation(conversation("conversation-goal", None, "message-goal"))
        .unwrap();
    service
        .create_conversation_goal("conversation-goal", "Finish the migration.", 2)
        .unwrap();

    let blocked = service
        .update_conversation_goal_status(
            "conversation-goal",
            ConversationGoalStatus::Blocked,
            Some("Waiting for user input."),
            3,
        )
        .unwrap();
    assert_eq!(blocked.status, ConversationGoalStatus::Blocked);
    assert_eq!(
        service
            .load_visible_conversation_goal("conversation-goal")
            .unwrap()
            .unwrap()
            .status,
        ConversationGoalStatus::Blocked
    );

    let resumed = service
        .resume_blocked_conversation_goal_for_user_turn("conversation-goal", 4)
        .unwrap()
        .unwrap();
    assert_eq!(resumed.status, ConversationGoalStatus::Active);
    assert_eq!(resumed.stopped_reason, None);
}

#[test]
fn completed_goal_leaves_model_context_and_is_deleted_with_conversation() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    service
        .save_conversation(conversation("conversation-goal", None, "message-goal"))
        .unwrap();
    service
        .create_conversation_goal("conversation-goal", "Finish the migration.", 2)
        .unwrap();
    service
        .update_conversation_goal_status(
            "conversation-goal",
            ConversationGoalStatus::Completed,
            None,
            3,
        )
        .unwrap();

    assert!(service
        .load_visible_conversation_goal("conversation-goal")
        .unwrap()
        .is_none());
    assert_eq!(
        service
            .load_conversation_goal("conversation-goal")
            .unwrap()
            .unwrap()
            .status,
        ConversationGoalStatus::Completed
    );

    service.delete_conversation("conversation-goal").unwrap();
    assert!(service
        .load_conversation_goal("conversation-goal")
        .unwrap()
        .is_none());
}

#[test]
fn fork_copies_only_a_visible_goal_whose_source_message_is_included() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    service
        .save_conversation(ChatConversationRecord {
            id: "conversation-goal-source".to_string(),
            project_id: None,
            model_id: Some("model-1".to_string()),
            title: "goal source".to_string(),
            messages: vec![
                ChatMessageRecord {
                    id: "goal-user".to_string(),
                    role: "user".to_string(),
                    content: "Track this goal across turns.".to_string(),
                    created_at: 1,
                    status: Some("sent".to_string()),
                    attachments: Vec::new(),
                    agent_run_json: None,
                    ui_state_json: None,
                },
                ChatMessageRecord {
                    id: "goal-assistant".to_string(),
                    role: "assistant".to_string(),
                    content: "Understood.".to_string(),
                    created_at: 2,
                    status: Some("sent".to_string()),
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
    service
        .create_conversation_goal(
            "conversation-goal-source",
            "Finish the tracked migration.",
            3,
        )
        .unwrap();

    let forked = service
        .fork_conversation(ForkConversationInput {
            request_id: "fork-visible-goal".to_string(),
            source_conversation_id: "conversation-goal-source".to_string(),
            through_assistant_message_id: "goal-assistant".to_string(),
        })
        .unwrap();
    let forked_goal = service
        .load_visible_conversation_goal(&forked.id)
        .unwrap()
        .unwrap();

    assert_eq!(forked_goal.objective, "Finish the tracked migration.");
    assert_eq!(forked_goal.conversation_id, forked.id);
    assert_ne!(
        forked_goal.goal_id,
        service
            .load_visible_conversation_goal("conversation-goal-source")
            .unwrap()
            .unwrap()
            .goal_id
    );
    assert_eq!(
        forked_goal.source_message_id,
        forked.messages.first().unwrap().id
    );
}
