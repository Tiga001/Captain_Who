use super::*;

#[test]
fn rollback_turn_preparation_removes_only_the_exact_empty_provisional_trace_and_messages() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let previous = conversation("conversation-turn-rollback", None, "message-existing");
    service.save_conversation(previous.clone()).unwrap();
    let previous = service.load_conversation(&previous.id).unwrap().unwrap();
    let mut prepared = previous.clone();
    prepared.model_id = Some("model-2".to_string());
    prepared.updated_at = 10;
    prepared.messages.extend([
        ChatMessageRecord {
            human_interaction_response: None,
            id: "message-provisional-user".to_string(),
            role: "user".to_string(),
            content: "provisional".to_string(),
            created_at: 9,
            status: Some("sent".to_string()),
            attachments: Vec::new(),
            folder_references_json: None,
            agent_run_json: None,
            ui_state_json: None,
        },
        ChatMessageRecord {
            human_interaction_response: None,
            id: "message-provisional-assistant".to_string(),
            role: "assistant".to_string(),
            content: "Thinking...".to_string(),
            created_at: 10,
            status: Some("pending".to_string()),
            attachments: Vec::new(),
            folder_references_json: None,
            agent_run_json: None,
            ui_state_json: None,
        },
    ]);
    service.save_conversation(prepared).unwrap();
    let trace = crate::ConversationTraceSnapshot::default().in_progress_trace(
        "run-provisional",
        "conversation-turn-rollback",
        "message-provisional-assistant",
    );
    service
        .append_in_progress_conversation_turn_trace(&trace, 10, 10)
        .unwrap();

    service
        .rollback_conversation_turn_preparation(
            "conversation-turn-rollback",
            "message-provisional-user",
            "message-provisional-assistant",
            Some("run-provisional"),
            Some(&previous),
            true,
        )
        .unwrap();

    assert_eq!(
        serde_json::to_value(
            service
                .load_conversation("conversation-turn-rollback")
                .unwrap()
                .unwrap()
        )
        .unwrap(),
        serde_json::to_value(previous).unwrap()
    );
    assert!(service
        .get_conversation_turn_trace("message-provisional-assistant")
        .unwrap()
        .is_none());
}

#[test]
fn rollback_turn_preparation_refuses_to_delete_a_nonempty_or_foreign_trace() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let previous = conversation("conversation-turn-rollback-foreign", None, "existing");
    service.save_conversation(previous.clone()).unwrap();
    let mut prepared = previous.clone();
    prepared.messages.push(ChatMessageRecord {
        human_interaction_response: None,
        id: "assistant-foreign".to_string(),
        role: "assistant".to_string(),
        content: "Thinking...".to_string(),
        created_at: 2,
        status: Some("pending".to_string()),
        attachments: Vec::new(),
        folder_references_json: None,
        agent_run_json: None,
        ui_state_json: None,
    });
    service.save_conversation(prepared).unwrap();
    let trace = crate::ConversationTraceSnapshot::default().in_progress_trace(
        "run-owner",
        "conversation-turn-rollback-foreign",
        "assistant-foreign",
    );
    service
        .append_in_progress_conversation_turn_trace(&trace, 2, 2)
        .unwrap();

    let error = service
        .rollback_agent_wake_turn_preparation(
            "conversation-turn-rollback-foreign",
            "assistant-foreign",
            Some("run-not-owner"),
            &previous,
            true,
        )
        .unwrap_err();
    assert!(error.contains("owned by another run"), "{error}");
    assert!(service
        .get_conversation_turn_trace("assistant-foreign")
        .unwrap()
        .is_some());
    assert_eq!(
        service
            .load_conversation("conversation-turn-rollback-foreign")
            .unwrap()
            .unwrap()
            .messages
            .len(),
        2
    );
}

#[test]
fn rollback_removes_only_provisional_facts_and_preserves_concurrent_metadata() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let previous = conversation(
        "conversation-turn-rollback-meta",
        None,
        "message-existing-meta",
    );
    service.save_conversation(previous.clone()).unwrap();
    let mut prepared = previous;
    prepared.model_id = Some("model-2".to_string());
    prepared.updated_at = 10;
    prepared.messages.extend([
        ChatMessageRecord {
            human_interaction_response: None,
            id: "message-provisional-meta-user".to_string(),
            role: "user".to_string(),
            content: "provisional".to_string(),
            created_at: 9,
            status: Some("sent".to_string()),
            attachments: Vec::new(),
            folder_references_json: None,
            agent_run_json: None,
            ui_state_json: None,
        },
        ChatMessageRecord {
            human_interaction_response: None,
            id: "message-provisional-meta-assistant".to_string(),
            role: "assistant".to_string(),
            content: "Thinking...".to_string(),
            created_at: 10,
            status: Some("pending".to_string()),
            attachments: Vec::new(),
            folder_references_json: None,
            agent_run_json: None,
            ui_state_json: None,
        },
    ]);
    service.save_conversation(prepared).unwrap();
    let trace = crate::ConversationTraceSnapshot::default().in_progress_trace(
        "run-provisional-meta",
        "conversation-turn-rollback-meta",
        "message-provisional-meta-assistant",
    );
    service
        .append_in_progress_conversation_turn_trace(&trace, 10, 10)
        .unwrap();

    let mut concurrent = service
        .load_conversation_metas()
        .unwrap()
        .into_iter()
        .find(|conversation| conversation.id == "conversation-turn-rollback-meta")
        .unwrap();
    concurrent.title = "Pinned while preparing".to_string();
    concurrent.pinned_at = Some(11);
    concurrent.updated_at = 11;
    service.save_conversation_meta(concurrent).unwrap();

    service
        .rollback_conversation_turn_preparation(
            "conversation-turn-rollback-meta",
            "message-provisional-meta-user",
            "message-provisional-meta-assistant",
            Some("run-provisional-meta"),
            Some(&conversation(
                "conversation-turn-rollback-meta",
                None,
                "message-existing-meta",
            )),
            true,
        )
        .unwrap();

    let current = service
        .load_conversation("conversation-turn-rollback-meta")
        .unwrap()
        .unwrap();
    assert_eq!(current.title, "Pinned while preparing");
    assert_eq!(current.pinned_at, Some(11));
    assert_eq!(current.updated_at, 11);
    assert_eq!(current.model_id.as_deref(), Some("model-2"));
    assert_eq!(current.messages.len(), 1);
    assert_eq!(current.messages[0].id, "message-existing-meta");
    assert!(service
        .get_conversation_turn_trace("message-provisional-meta-assistant")
        .unwrap()
        .is_none());
}

#[test]
fn turn_commit_rechecks_graph_identity_and_lifecycle_inside_the_write_transaction() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let conversation_id = "conversation-turn-graph-fence";
    service
        .save_conversation(conversation(conversation_id, None, "message-graph-fence"))
        .unwrap();
    let root = service
        .ensure_root_agent(&EnsureRootAgentInput {
            agent_id: "agent-turn-graph-fence".to_string(),
            conversation_id: conversation_id.to_string(),
            creation_request_id: "ensure-turn-graph-fence".to_string(),
            task_name: "Root".to_string(),
        })
        .unwrap()
        .record()
        .clone();
    let (snapshot, revision) = service.load_conversation_for_turn(conversation_id).unwrap();
    let mut candidate = snapshot.unwrap();
    candidate.messages.push(ChatMessageRecord {
        human_interaction_response: None,
        id: "assistant-graph-fence".to_string(),
        role: "assistant".to_string(),
        content: "Thinking...".to_string(),
        created_at: 2,
        status: Some("pending".to_string()),
        attachments: Vec::new(),
        folder_references_json: None,
        agent_run_json: None,
        ui_state_json: None,
    });
    let trace = crate::ConversationTraceSnapshot::default().in_progress_trace(
        "run-graph-fence",
        conversation_id,
        "assistant-graph-fence",
    );

    // Lifecycle changes do not rewrite the Conversation row itself. Admission must nevertheless
    // re-read the bound Agent under the same IMMEDIATE transaction as the Turn write.
    service
        .transition_agent_lifecycle(
            &root.agent_id,
            root.revision,
            crate::AgentLifecycle::Active,
            crate::AgentLifecycle::Disabled,
        )
        .unwrap();
    let error = service
        .save_conversation_and_begin_turn(
            candidate,
            revision,
            None,
            crate::AgentTurnPermissionSource::HostAuthenticatedRoot(
                crate::AgentPermissions::default(),
            ),
            &trace,
            2,
            2,
        )
        .unwrap_err();
    assert!(error.contains("active root Agent"), "{error}");
    assert!(service
        .get_conversation_turn_trace("assistant-graph-fence")
        .unwrap()
        .is_none());
    assert!(service
        .load_conversation(conversation_id)
        .unwrap()
        .unwrap()
        .messages
        .iter()
        .all(|message| message.id != "assistant-graph-fence"));
}

#[test]
fn root_permission_snapshot_is_atomic_with_turn_admission_and_survives_reopen() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let conversation_id = "conversation-permission-reopen";
    service
        .save_conversation(conversation(
            conversation_id,
            None,
            "message-permission-reopen-user",
        ))
        .unwrap();
    service
        .ensure_root_agent(&EnsureRootAgentInput {
            agent_id: "agent-permission-reopen".to_string(),
            conversation_id: conversation_id.to_string(),
            creation_request_id: "ensure-permission-reopen".to_string(),
            task_name: "Root".to_string(),
        })
        .unwrap();
    let (candidate, revision) = service.load_conversation_for_turn(conversation_id).unwrap();
    let mut candidate = candidate.unwrap();
    candidate.messages.push(ChatMessageRecord {
        human_interaction_response: None,
        id: "assistant-permission-reopen".to_string(),
        role: "assistant".to_string(),
        content: "Thinking...".to_string(),
        created_at: 2,
        status: Some("pending".to_string()),
        attachments: Vec::new(),
        folder_references_json: None,
        agent_run_json: None,
        ui_state_json: None,
    });
    candidate.updated_at = 2;
    let trace = crate::ConversationTraceSnapshot::default().in_progress_trace(
        "run-permission-reopen",
        conversation_id,
        "assistant-permission-reopen",
    );
    let full = crate::AgentPermissions {
        read: crate::AgentReadPermission::All,
        write: crate::AgentWritePermission::All,
        command: crate::AgentCommandPermission::AutoApprove,
        command_safety: crate::AgentCommandSafetyPolicy::FullAccess,
        patch: crate::AgentPatchPermission::AutoApprove,
        builtin_execution: crate::AgentBuiltinExecutionPermission::AutoApprove,
    };
    let (_, admitted) = service
        .save_conversation_and_begin_turn(
            candidate,
            revision,
            None,
            crate::AgentTurnPermissionSource::HostAuthenticatedRoot(full),
            &trace,
            2,
            2,
        )
        .unwrap();
    assert_eq!(admitted, full);
    drop(service);

    let reopened = fixture.service();
    let snapshot = reopened
        .get_agent_effective_permission_snapshot("agent-permission-reopen")
        .unwrap()
        .unwrap();
    assert_eq!(snapshot.permissions, full);
    assert_eq!(snapshot.source_run_id, "run-permission-reopen");
    assert_eq!(
        snapshot.source_assistant_message_id,
        "assistant-permission-reopen"
    );
}
