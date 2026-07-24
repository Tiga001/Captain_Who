use super::*;

fn guidance_conversation(
    conversation_id: &str,
    assistant_message_id: &str,
) -> ChatConversationRecord {
    ChatConversationRecord {
        id: conversation_id.to_string(),
        project_id: None,
        model_id: Some("model-1".to_string()),
        title: "guidance".to_string(),
        messages: vec![ChatMessageRecord {
            id: assistant_message_id.to_string(),
            role: "assistant".to_string(),
            content: "pending".to_string(),
            created_at: 1,
            status: Some("pending".to_string()),
            attachments: Vec::new(),
            agent_run_json: None,
            ui_state_json: None,
        }],
        created_at: 1,
        updated_at: 1,
        pinned_at: None,
        archived_at: None,
        unread_at: None,
    }
}

fn guidance_trace(
    conversation_id: &str,
    assistant_message_id: &str,
    guidance_id: &str,
) -> ConversationTurnTrace {
    ConversationTurnTrace {
        schema_version: crate::CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
        run_id: "run-guidance-atomic".to_string(),
        conversation_id: conversation_id.to_string(),
        assistant_message_id: assistant_message_id.to_string(),
        terminal_status: crate::ConversationTurnTraceTerminalStatus::InProgress,
        terminal_error: None,
        truncated: false,
        items: vec![ConversationTurnTraceItem::UserGuidance {
            sequence: 0,
            guidance_id: guidance_id.to_string(),
            client_message_id: "client-guidance-atomic".to_string(),
            content: "Use the updated constraint.".to_string(),
            attachments: Vec::new(),
            created_at: 2,
            truncated: false,
        }],
    }
}

#[test]
fn in_progress_trace_and_guidance_application_commit_atomically() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    service
        .save_conversation(guidance_conversation(
            "conversation-guidance-atomic",
            "assistant-guidance-atomic",
        ))
        .unwrap();
    service
        .store_agent_run_guidance(AgentRunGuidanceRecord {
            guidance_id: "guidance-atomic".to_string(),
            client_message_id: "client-guidance-atomic".to_string(),
            run_id: "run-guidance-atomic".to_string(),
            conversation_id: "conversation-guidance-atomic".to_string(),
            assistant_message_id: "assistant-guidance-atomic".to_string(),
            content: "Use the updated constraint.".to_string(),
            status: crate::AgentGuidanceStatus::Queued,
            attachment_ids: Vec::new(),
            applied_trace_sequence: None,
            terminal_reason: None,
            created_at: 2,
            updated_at: 2,
        })
        .unwrap();

    assert!(service
        .append_in_progress_conversation_turn_trace_and_apply_guidances(
            &guidance_trace(
                "conversation-guidance-atomic",
                "assistant-guidance-atomic",
                "guidance-atomic",
            ),
            1,
            3,
        )
        .unwrap());
    let record = service
        .load_agent_run_guidance("guidance-atomic")
        .unwrap()
        .unwrap();
    assert_eq!(record.status, crate::AgentGuidanceStatus::Applied);
    assert_eq!(record.applied_trace_sequence, Some(0));
    assert!(service
        .get_conversation_turn_trace("assistant-guidance-atomic")
        .unwrap()
        .is_some());
}

#[test]
fn missing_guidance_journal_rolls_back_the_trace_append() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    service
        .save_conversation(guidance_conversation(
            "conversation-guidance-rollback",
            "assistant-guidance-rollback",
        ))
        .unwrap();

    assert!(service
        .append_in_progress_conversation_turn_trace_and_apply_guidances(
            &guidance_trace(
                "conversation-guidance-rollback",
                "assistant-guidance-rollback",
                "guidance-missing",
            ),
            1,
            3,
        )
        .is_err());
    assert!(service
        .get_conversation_turn_trace("assistant-guidance-rollback")
        .unwrap()
        .is_none());
}

#[test]
fn guidance_attachment_ownership_is_atomic_and_hidden_until_application() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    service
        .save_conversation(guidance_conversation(
            "conversation-guidance-attachment",
            "assistant-guidance-attachment",
        ))
        .unwrap();
    let attachment = AgentInputAttachment {
        id: "attachment-guidance-owned".to_string(),
        kind: AgentInputAttachmentKind::File,
        name: "notes.txt".to_string(),
        mime_type: Some("text/plain".to_string()),
        size_bytes: 5,
        encoding: AgentInputAttachmentEncoding::Utf8,
        data: "hello".to_string(),
        truncated: None,
    };
    let record = AgentRunGuidanceRecord {
        guidance_id: "guidance-owned".to_string(),
        client_message_id: "client-owned".to_string(),
        run_id: "run-owned".to_string(),
        conversation_id: "conversation-guidance-attachment".to_string(),
        assistant_message_id: "assistant-guidance-attachment".to_string(),
        content: "Use the attachment.".to_string(),
        status: crate::AgentGuidanceStatus::Queued,
        attachment_ids: vec![attachment.id.clone()],
        applied_trace_sequence: None,
        terminal_reason: None,
        created_at: 2,
        updated_at: 2,
    };

    assert_eq!(
        service
            .store_agent_run_guidance_with_attachments(record, None, &[attachment])
            .unwrap(),
        AgentRunGuidanceStoreOutcome::Inserted
    );
    assert_eq!(
        service
            .agent_run_guidance_attachment_bytes("run-owned")
            .unwrap(),
        5
    );
    assert!(service
        .build_attachment_library_context("conversation-guidance-attachment", None)
        .unwrap()
        .conversation_attachments
        .is_empty());
    let admitted = service
        .build_attachment_library_context_for_active_run(
            "conversation-guidance-attachment",
            None,
            "run-owned",
        )
        .unwrap();
    assert_eq!(admitted.conversation_attachments.len(), 1);
    assert_eq!(
        admitted.conversation_attachments[0].read_path,
        "@attachments/attachment-guidance-owned/notes.txt"
    );
    assert!(service
        .load_conversation("conversation-guidance-attachment")
        .unwrap()
        .unwrap()
        .messages[0]
        .attachments
        .is_empty());

    assert_eq!(
        service
            .mark_agent_run_guidance_applied("guidance-owned", 0, 3)
            .unwrap(),
        AgentRunGuidanceTransitionOutcome::Updated
    );
    assert_eq!(
        service
            .build_attachment_library_context("conversation-guidance-attachment", None)
            .unwrap()
            .conversation_attachments
            .len(),
        1
    );
}
