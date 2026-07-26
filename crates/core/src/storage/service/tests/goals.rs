use super::*;
use crate::{ConversationGoalMutationActor, ConversationGoalStatus};

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
            ConversationGoalMutationActor::User,
            "conversation-goal",
            "Ship the explicitly requested long-running migration.",
            2,
        )
        .unwrap();
    assert_eq!(goal.status, ConversationGoalStatus::Active);
    assert_eq!(goal.source_message_id, "message-goal");
    assert!(service
        .create_conversation_goal(
            ConversationGoalMutationActor::User,
            "conversation-goal",
            "A second unfinished goal.",
            3,
        )
        .unwrap_err()
        .contains("仍存在尚未结束的 Goal"));
}

#[test]
fn blocked_goal_changes_only_through_an_explicit_actor() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    service
        .save_conversation(conversation("conversation-goal", None, "message-goal"))
        .unwrap();
    service
        .create_conversation_goal(
            ConversationGoalMutationActor::User,
            "conversation-goal",
            "Finish the migration.",
            2,
        )
        .unwrap();

    let blocked = service
        .update_conversation_goal_status(
            ConversationGoalMutationActor::Model,
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
        .update_conversation_goal_status(
            ConversationGoalMutationActor::Model,
            "conversation-goal",
            ConversationGoalStatus::Active,
            None,
            4,
        )
        .unwrap();
    assert_eq!(resumed.status, ConversationGoalStatus::Active);
    assert_eq!(resumed.stopped_reason, None);
}

#[test]
fn restart_preserves_goal_status_without_reinterpreting_it() {
    let fixture = StorageFixture::new();
    let database_path = fixture.root.join("storage.sqlite");
    {
        let service = fixture.service();
        service
            .save_conversation(conversation(
                "conversation-goal-restart",
                None,
                "message-goal-restart",
            ))
            .unwrap();
        service
            .create_conversation_goal(
                ConversationGoalMutationActor::User,
                "conversation-goal-restart",
                "Keep the explicit goal across restart.",
                2,
            )
            .unwrap();
        service
            .update_conversation_goal_status(
                ConversationGoalMutationActor::Model,
                "conversation-goal-restart",
                ConversationGoalStatus::Blocked,
                Some("Waiting for a user decision."),
                3,
            )
            .unwrap();
    }

    let reopened = StorageService::open(&database_path).unwrap();
    let goal = reopened
        .load_conversation_goal("conversation-goal-restart")
        .unwrap()
        .unwrap();
    assert_eq!(goal.status, ConversationGoalStatus::Blocked);
    assert_eq!(
        goal.stopped_reason.as_deref(),
        Some("Waiting for a user decision.")
    );
}

#[test]
fn completed_goal_leaves_model_context_and_is_deleted_with_conversation() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    service
        .save_conversation(conversation("conversation-goal", None, "message-goal"))
        .unwrap();
    service
        .create_conversation_goal(
            ConversationGoalMutationActor::User,
            "conversation-goal",
            "Finish the migration.",
            2,
        )
        .unwrap();
    service
        .update_conversation_goal_status(
            ConversationGoalMutationActor::Model,
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
fn fork_does_not_copy_an_active_goal_without_an_explicit_actor() {
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
            ConversationGoalMutationActor::User,
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
    assert!(service
        .load_conversation_goal(&forked.id)
        .unwrap()
        .is_none());
    assert_eq!(
        service
            .load_visible_conversation_goal("conversation-goal-source")
            .unwrap()
            .unwrap()
            .objective,
        "Finish the tracked migration."
    );
}

#[test]
fn terminal_and_blocked_goals_do_not_cross_a_fork() {
    for (suffix, status) in [
        ("blocked", ConversationGoalStatus::Blocked),
        ("cancelled", ConversationGoalStatus::Cancelled),
    ] {
        let fixture = StorageFixture::new();
        let service = fixture.service();
        let conversation_id = format!("conversation-{suffix}");
        let message_id = format!("message-{suffix}");
        service
            .save_conversation(conversation(&conversation_id, None, &message_id))
            .unwrap();
        service
            .create_conversation_goal(
                ConversationGoalMutationActor::User,
                &conversation_id,
                "Keep the active goal only.",
                2,
            )
            .unwrap();
        if status == ConversationGoalStatus::Cancelled {
            service
                .update_conversation_goal_status(
                    ConversationGoalMutationActor::Model,
                    &conversation_id,
                    ConversationGoalStatus::Cancelled,
                    None,
                    3,
                )
                .unwrap();
        } else {
            service
                .update_conversation_goal_status(
                    ConversationGoalMutationActor::Model,
                    &conversation_id,
                    ConversationGoalStatus::Blocked,
                    Some("blocked"),
                    3,
                )
                .unwrap();
        }
        let assistant_id = format!("assistant-{suffix}");
        let mut saved = service
            .load_conversation(&conversation_id)
            .unwrap()
            .unwrap();
        saved.messages.push(ChatMessageRecord {
            id: assistant_id.clone(),
            role: "assistant".to_string(),
            content: "done".to_string(),
            created_at: 4,
            status: Some("sent".to_string()),
            attachments: Vec::new(),
            agent_run_json: None,
            ui_state_json: None,
        });
        saved.updated_at = 4;
        service.save_conversation(saved).unwrap();

        let forked = service
            .fork_conversation(ForkConversationInput {
                request_id: format!("fork-{suffix}"),
                source_conversation_id: conversation_id,
                through_assistant_message_id: assistant_id,
            })
            .unwrap();
        assert!(service
            .load_conversation_goal(&forked.id)
            .unwrap()
            .is_none());
    }
}
