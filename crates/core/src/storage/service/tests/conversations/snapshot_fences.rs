use super::*;

#[test]
fn stale_full_conversation_snapshot_cannot_delete_an_active_turn_before_unique_check() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let base = conversation("conversation-stale-turn-admission", None, "message-base");
    service.save_conversation(base.clone()).unwrap();
    let (base, initial_revision) = service
        .load_conversation_for_turn("conversation-stale-turn-admission")
        .unwrap();
    let base = base.unwrap();

    let mut candidate_a = base.clone();
    candidate_a.messages.extend([
        ChatMessageRecord {
            human_interaction_response: None,
            id: "user-candidate-a".to_string(),
            role: "user".to_string(),
            content: "candidate a".to_string(),
            created_at: 2,
            status: Some("sent".to_string()),
            attachments: Vec::new(),
            folder_references_json: None,
            agent_run_json: None,
            ui_state_json: None,
        },
        ChatMessageRecord {
            human_interaction_response: None,
            id: "assistant-candidate-a".to_string(),
            role: "assistant".to_string(),
            content: "Thinking...".to_string(),
            created_at: 3,
            status: Some("pending".to_string()),
            attachments: Vec::new(),
            folder_references_json: None,
            agent_run_json: None,
            ui_state_json: None,
        },
    ]);
    candidate_a.updated_at = 3;
    let trace_a = crate::ConversationTraceSnapshot::default().in_progress_trace(
        "run-candidate-a",
        &candidate_a.id,
        "assistant-candidate-a",
    );
    service
        .save_conversation_and_begin_turn(
            candidate_a.clone(),
            initial_revision,
            None,
            crate::AgentTurnPermissionSource::HostAuthenticatedRoot(
                crate::AgentPermissions::default(),
            ),
            &trace_a,
            3,
            3,
        )
        .unwrap();
    let (candidate_a, active_revision) = service
        .load_conversation_for_turn("conversation-stale-turn-admission")
        .unwrap();
    let candidate_a = candidate_a.unwrap();

    // Candidate B was built from the same stale pre-A snapshot. A full-save implementation that
    // checks uniqueness only after replacing messages would cascade-delete A's trace and win.
    let mut candidate_b = base;
    candidate_b.messages.extend([
        ChatMessageRecord {
            human_interaction_response: None,
            id: "user-candidate-b".to_string(),
            role: "user".to_string(),
            content: "candidate b".to_string(),
            created_at: 2,
            status: Some("sent".to_string()),
            attachments: Vec::new(),
            folder_references_json: None,
            agent_run_json: None,
            ui_state_json: None,
        },
        ChatMessageRecord {
            human_interaction_response: None,
            id: "assistant-candidate-b".to_string(),
            role: "assistant".to_string(),
            content: "Thinking...".to_string(),
            created_at: 3,
            status: Some("pending".to_string()),
            attachments: Vec::new(),
            folder_references_json: None,
            agent_run_json: None,
            ui_state_json: None,
        },
    ]);
    candidate_b.updated_at = 3;
    let trace_b = crate::ConversationTraceSnapshot::default().in_progress_trace(
        "run-candidate-b",
        &candidate_b.id,
        "assistant-candidate-b",
    );
    let error = service
        .save_conversation_and_begin_turn(
            candidate_b,
            active_revision,
            None,
            crate::AgentTurnPermissionSource::HostAuthenticatedRoot(
                crate::AgentPermissions::default(),
            ),
            &trace_b,
            3,
            3,
        )
        .unwrap_err();
    assert!(error.contains("active durable Turn"), "{error}");

    let mut current = service
        .load_conversation("conversation-stale-turn-admission")
        .unwrap()
        .unwrap();
    let projected_run: serde_json::Value = serde_json::from_str(
        current.messages[2]
            .agent_run_json
            .as_deref()
            .expect("the durable active trace should project a running lifecycle"),
    )
    .unwrap();
    assert_eq!(projected_run["runId"], "run-candidate-a");
    assert_eq!(projected_run["status"], "running");
    assert_eq!(projected_run["state"]["status"], "running");
    assert_eq!(projected_run["state"]["activeRunId"], "run-candidate-a");
    assert_eq!(projected_run["timeline"], serde_json::json!([]));
    current.messages[2].agent_run_json = None;
    assert_eq!(
        serde_json::to_value(current).unwrap(),
        serde_json::to_value(candidate_a).unwrap()
    );
    let traces = service
        .list_conversation_turn_traces("conversation-stale-turn-admission")
        .unwrap();
    assert_eq!(traces, vec![trace_a]);
    assert!(service
        .get_conversation_turn_trace("assistant-candidate-b")
        .unwrap()
        .is_none());
}

#[test]
fn completed_turn_revision_fences_a_cross_host_stale_full_snapshot() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    service
        .save_conversation(conversation(
            "conversation-completed-turn-cas",
            None,
            "message-base",
        ))
        .unwrap();
    let (base, initial_revision) = service
        .load_conversation_for_turn("conversation-completed-turn-cas")
        .unwrap();
    let base = base.unwrap();

    let mut candidate_a = base.clone();
    candidate_a.messages.extend([
        ChatMessageRecord {
            human_interaction_response: None,
            id: "user-completed-a".to_string(),
            role: "user".to_string(),
            content: "candidate a".to_string(),
            created_at: 2,
            status: Some("sent".to_string()),
            attachments: Vec::new(),
            folder_references_json: None,
            agent_run_json: None,
            ui_state_json: None,
        },
        ChatMessageRecord {
            human_interaction_response: None,
            id: "assistant-completed-a".to_string(),
            role: "assistant".to_string(),
            content: "Thinking...".to_string(),
            created_at: 3,
            status: Some("pending".to_string()),
            attachments: Vec::new(),
            folder_references_json: None,
            agent_run_json: None,
            ui_state_json: None,
        },
    ]);
    candidate_a.updated_at = 3;
    let trace_a = crate::ConversationTraceSnapshot::default().in_progress_trace(
        "run-completed-a",
        &candidate_a.id,
        "assistant-completed-a",
    );
    service
        .save_conversation_and_begin_turn(
            candidate_a,
            initial_revision,
            None,
            crate::AgentTurnPermissionSource::HostAuthenticatedRoot(
                crate::AgentPermissions::default(),
            ),
            &trace_a,
            3,
            3,
        )
        .unwrap();
    let terminal_a = crate::completed_conversation_trace_without_items(
        "run-completed-a",
        "conversation-completed-turn-cas",
        "assistant-completed-a",
    );
    service
        .finalize_chat_message_with_conversation_trace_and_usage(
            "conversation-completed-turn-cas",
            "assistant-completed-a",
            "candidate a complete",
            Some("sent"),
            "completed",
            &terminal_a,
            3,
            4,
            None,
        )
        .unwrap();
    assert!(service
        .list_conversation_turn_traces("conversation-completed-turn-cas")
        .unwrap()
        .iter()
        .all(|trace| {
            trace.terminal_status != crate::ConversationTurnTraceTerminalStatus::InProgress
        }));
    let completed_a = service
        .load_conversation("conversation-completed-turn-cas")
        .unwrap()
        .unwrap();

    // Host B loaded `base` before A began. A has already released the active trace, so only the
    // durable Conversation revision can stop B from replacing A's completed messages.
    let mut stale_candidate_b = base;
    stale_candidate_b.messages.extend([
        ChatMessageRecord {
            human_interaction_response: None,
            id: "user-stale-b".to_string(),
            role: "user".to_string(),
            content: "candidate b".to_string(),
            created_at: 2,
            status: Some("sent".to_string()),
            attachments: Vec::new(),
            folder_references_json: None,
            agent_run_json: None,
            ui_state_json: None,
        },
        ChatMessageRecord {
            human_interaction_response: None,
            id: "assistant-stale-b".to_string(),
            role: "assistant".to_string(),
            content: "Thinking...".to_string(),
            created_at: 3,
            status: Some("pending".to_string()),
            attachments: Vec::new(),
            folder_references_json: None,
            agent_run_json: None,
            ui_state_json: None,
        },
    ]);
    stale_candidate_b.updated_at = 3;
    let trace_b = crate::ConversationTraceSnapshot::default().in_progress_trace(
        "run-stale-b",
        &stale_candidate_b.id,
        "assistant-stale-b",
    );
    let error = service
        .save_conversation_and_begin_turn(
            stale_candidate_b,
            initial_revision,
            None,
            crate::AgentTurnPermissionSource::HostAuthenticatedRoot(
                crate::AgentPermissions::default(),
            ),
            &trace_b,
            3,
            3,
        )
        .unwrap_err();
    assert!(error.contains("Conversation changed"), "{error}");
    assert_eq!(
        serde_json::to_value(
            service
                .load_conversation("conversation-completed-turn-cas")
                .unwrap()
                .unwrap()
        )
        .unwrap(),
        serde_json::to_value(completed_a).unwrap()
    );
    assert!(service
        .get_conversation_turn_trace("assistant-stale-b")
        .unwrap()
        .is_none());
}

#[test]
fn stale_saves_cannot_recreate_deleted_project_data() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let stale_conversation = conversation("conversation-stale", Some("project-1"), "message-stale");
    service
        .save_conversation(stale_conversation.clone())
        .unwrap();
    service.delete_project("project-1").unwrap();

    assert!(service
        .save_conversation(stale_conversation.clone())
        .is_err());
    assert!(service
        .save_conversation_meta(ChatConversationMetaRecord {
            id: stale_conversation.id.clone(),
            project_id: stale_conversation.project_id.clone(),
            model_id: stale_conversation.model_id.clone(),
            title: stale_conversation.title.clone(),
            created_at: stale_conversation.created_at,
            updated_at: stale_conversation.updated_at,
            pinned_at: stale_conversation.pinned_at,
            archived_at: stale_conversation.archived_at,
            unread_at: stale_conversation.unread_at,
        })
        .is_err());
    assert!(service
        .upsert_chat_messages(
            &stale_conversation.id,
            stale_conversation.messages.clone(),
            0,
        )
        .is_err());
    assert!(service
        .save_composer_draft(composer_draft(
            &stale_conversation.id,
            Some("project-1"),
            "stale draft",
        ))
        .is_err());
    assert!(service.load_conversations().unwrap().is_empty());
    assert!(service.load_composer_drafts().unwrap().is_empty());
}
