use super::*;
use crate::storage::models::ConversationContinuationOriginRecord;

#[test]
fn input_attachments_are_persisted_and_rehydrated() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    service
        .save_conversation(conversation(
            "conversation-1",
            Some("project-1"),
            "message-1",
        ))
        .unwrap();
    service
        .save_input_attachments(
            "conversation-1",
            "message-1",
            Some("project-1"),
            &[input_attachment(
                "attachment-1",
                AgentInputAttachmentKind::Image,
                "pixel.png",
                Some("image/png"),
                b"png-bytes",
            )],
            10,
        )
        .unwrap();

    let conversation = service
        .load_conversation("conversation-1")
        .unwrap()
        .unwrap();
    let attachment = &conversation.messages[0].attachments[0];

    assert_eq!(attachment.id, "attachment-1");
    assert_eq!(attachment.kind, "image");
    assert_eq!(attachment.name, "pixel.png");
    assert_eq!(attachment.preview_mime_type.as_deref(), Some("image/png"));
    assert!(attachment.preview_data.is_some());

    let library = service
        .build_attachment_library_context("conversation-1", Some("project-1"))
        .unwrap();
    assert_eq!(library.conversation_attachments.len(), 1);
    assert_eq!(
        library.conversation_attachments[0].read_path,
        "@attachments/attachment-1/pixel.png"
    );
    assert!(PathBuf::from(library.root_path.unwrap())
        .join(&library.conversation_attachments[0].storage_rel_path)
        .is_file());
}

#[test]
fn forked_conversation_owns_independent_attachment_files_and_is_idempotent() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let mut source = conversation("conversation-source", Some("project-1"), "message-user");
    source.messages.push(ChatMessageRecord {
        id: "message-assistant".to_string(),
        role: "assistant".to_string(),
        content: "done".to_string(),
        created_at: 2,
        status: Some("sent".to_string()),
        attachments: Vec::new(),
        agent_run_json: Some(
            serde_json::json!({
                "runId": "run-guidance-fork",
                "status": "completed",
                "state": { "status": "completed" },
                "timeline": []
            })
            .to_string(),
        ),
        ui_state_json: None,
    });
    source.updated_at = 2;
    service.save_conversation(source).unwrap();
    service
        .save_input_attachments(
            "conversation-source",
            "message-user",
            Some("project-1"),
            &[input_attachment(
                "attachment-source",
                AgentInputAttachmentKind::File,
                "notes.txt",
                Some("text/plain"),
                b"independent fork attachment",
            )],
            1,
        )
        .unwrap();

    let input =
        assistant_reply_fork_request("fork-request-1", "conversation-source", "message-assistant");
    let forked = service
        .fork_conversation_request_view(input.clone())
        .unwrap()
        .conversation;
    assert_eq!(forked.project_id.as_deref(), Some("project-1"));
    assert_eq!(forked.messages.len(), 2);
    let forked_boundary_message_id = forked.messages[1].id.clone();
    let forked_attachment = forked.messages[0].attachments.as_slice();
    assert_eq!(forked_attachment.len(), 1);
    assert_ne!(forked_attachment[0].id, "attachment-source");
    let forked_view = service.load_conversation_view(&forked.id).unwrap().unwrap();
    assert_eq!(
        forked_view.continuation_origin,
        Some(ConversationContinuationOriginRecord {
            source_conversation_id: "conversation-source".to_string(),
            source_message_id: "message-assistant".to_string(),
            boundary_message_id: forked_boundary_message_id,
        })
    );

    let retry = service
        .fork_conversation_request_view(input)
        .unwrap()
        .conversation;
    assert_eq!(retry.id, forked.id);
    assert_eq!(service.load_conversations().unwrap().len(), 2);

    service.delete_conversation("conversation-source").unwrap();
    let retained_view = service.load_conversation_view(&forked.id).unwrap().unwrap();
    assert!(
        retained_view.continuation_origin.is_some(),
        "deleting the source keeps the broken lineage so the renderer can report it"
    );
    let retained = retained_view.conversation;
    let retained_attachment_id = retained.messages[0].attachments[0].id.clone();
    let retained_payload = service
        .load_input_attachments(&[retained_attachment_id])
        .unwrap();
    assert_eq!(retained_payload.len(), 1);
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(&retained_payload[0].data)
            .unwrap(),
        b"independent fork attachment"
    );

    let retained_assistant_id = retained
        .messages
        .iter()
        .find(|message| message.role == "assistant")
        .unwrap()
        .id
        .clone();
    let recursive = service
        .fork_conversation_request_view(assistant_reply_fork_request(
            "fork-request-2",
            retained.id.clone(),
            retained_assistant_id,
        ))
        .unwrap()
        .conversation;
    service.delete_conversation(&retained.id).unwrap();

    let recursive = service.load_conversation(&recursive.id).unwrap().unwrap();
    let recursive_attachment_id = recursive.messages[0].attachments[0].id.clone();
    let recursive_payload = service
        .load_input_attachments(&[recursive_attachment_id])
        .unwrap();
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(&recursive_payload[0].data)
            .unwrap(),
        b"independent fork attachment"
    );
}

#[test]
fn fork_clones_applied_guidance_attachments_but_not_abandoned_ones() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let mut source = conversation(
        "conversation-guidance-fork",
        Some("project-1"),
        "message-user-guidance-fork",
    );
    source.messages.push(ChatMessageRecord {
        id: "message-assistant-guidance-fork".to_string(),
        role: "assistant".to_string(),
        content: "done".to_string(),
        created_at: 2,
        status: Some("sent".to_string()),
        attachments: Vec::new(),
        agent_run_json: None,
        ui_state_json: None,
    });
    source.updated_at = 2;
    service.save_conversation(source).unwrap();

    let applied = input_attachment(
        "attachment-guidance-applied",
        AgentInputAttachmentKind::File,
        "applied.txt",
        Some("text/plain"),
        b"applied guidance payload",
    );
    let abandoned = input_attachment(
        "attachment-guidance-abandoned",
        AgentInputAttachmentKind::File,
        "abandoned.txt",
        Some("text/plain"),
        b"abandoned guidance payload",
    );
    for (record, attachment) in [
        (
            AgentRunGuidanceRecord {
                guidance_id: "guidance-applied-fork".to_string(),
                client_message_id: "client-applied-fork".to_string(),
                run_id: "run-guidance-fork".to_string(),
                conversation_id: "conversation-guidance-fork".to_string(),
                assistant_message_id: "message-assistant-guidance-fork".to_string(),
                content: "Applied guidance.".to_string(),
                status: crate::AgentGuidanceStatus::Queued,
                attachment_ids: vec![applied.id.clone()],
                applied_trace_sequence: None,
                terminal_reason: None,
                created_at: 3,
                updated_at: 3,
            },
            applied.clone(),
        ),
        (
            AgentRunGuidanceRecord {
                guidance_id: "guidance-abandoned-fork".to_string(),
                client_message_id: "client-abandoned-fork".to_string(),
                run_id: "run-guidance-fork".to_string(),
                conversation_id: "conversation-guidance-fork".to_string(),
                assistant_message_id: "message-assistant-guidance-fork".to_string(),
                content: "Abandoned guidance.".to_string(),
                status: crate::AgentGuidanceStatus::Queued,
                attachment_ids: vec![abandoned.id.clone()],
                applied_trace_sequence: None,
                terminal_reason: None,
                created_at: 4,
                updated_at: 4,
            },
            abandoned.clone(),
        ),
    ] {
        service
            .store_agent_run_guidance_with_attachments(record, Some("project-1"), &[attachment])
            .unwrap();
    }
    service
        .mark_agent_run_guidance_applied("guidance-applied-fork", 0, 5)
        .unwrap();
    service
        .mark_agent_run_guidance_terminal(
            "guidance-abandoned-fork",
            crate::AgentGuidanceStatus::Abandoned,
            "run interrupted",
            5,
        )
        .unwrap();
    service
        .replace_conversation_turn_trace(
            &ConversationTurnTrace {
                schema_version: crate::CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
                run_id: "run-guidance-fork".to_string(),
                conversation_id: "conversation-guidance-fork".to_string(),
                assistant_message_id: "message-assistant-guidance-fork".to_string(),
                terminal_status: crate::ConversationTurnTraceTerminalStatus::Completed,
                terminal_error: None,
                truncated: false,
                items: vec![ConversationTurnTraceItem::UserGuidance {
                    sequence: 0,
                    guidance_id: "guidance-applied-fork".to_string(),
                    client_message_id: "client-applied-fork".to_string(),
                    content: "Applied guidance.".to_string(),
                    attachments: vec![crate::ConversationTraceAttachment {
                        id: "attachment-guidance-applied".to_string(),
                        kind: AgentInputAttachmentKind::File,
                        name: "applied.txt".to_string(),
                        mime_type: Some("text/plain".to_string()),
                        size_bytes: applied.size_bytes,
                    }],
                    created_at: 3,
                    truncated: false,
                }],
            },
            2,
            5,
        )
        .unwrap();

    let library = service
        .build_attachment_library_context("conversation-guidance-fork", Some("project-1"))
        .unwrap();
    assert_eq!(library.conversation_attachments.len(), 1);
    assert_eq!(
        library.conversation_attachments[0].id,
        "attachment-guidance-applied"
    );
    let attachment_root = PathBuf::from(library.root_path.unwrap());
    let applied_source_path = attachment_root.join(attachment_storage_rel_path(
        "conversation-guidance-fork",
        "message-assistant-guidance-fork",
        "attachment-guidance-applied",
        "applied.txt",
    ));
    let abandoned_source_path = attachment_root.join(attachment_storage_rel_path(
        "conversation-guidance-fork",
        "message-assistant-guidance-fork",
        "attachment-guidance-abandoned",
        "abandoned.txt",
    ));
    assert!(applied_source_path.is_file());
    assert!(abandoned_source_path.is_file());

    let forked = service
        .fork_conversation_request_view(assistant_reply_fork_request(
            "fork-guidance-attachments",
            "conversation-guidance-fork",
            "message-assistant-guidance-fork",
        ))
        .unwrap()
        .conversation;
    let forked_assistant = forked.messages.last().unwrap();
    assert!(forked_assistant.attachments.is_empty());
    let forked_run: serde_json::Value =
        serde_json::from_str(forked_assistant.agent_run_json.as_deref().unwrap()).unwrap();
    assert_eq!(forked_run["timeline"].as_array().unwrap().len(), 1);
    assert_eq!(forked_run["timeline"][0]["type"], "user_guidance");
    assert_eq!(forked_run["timeline"][0]["status"], "applied");
    let forked_guidance_id = forked_run["timeline"][0]["guidanceId"]
        .as_str()
        .unwrap()
        .to_string();
    let forked_attachment_id = forked_run["timeline"][0]["attachments"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_ne!(forked_guidance_id, "guidance-applied-fork");
    assert_ne!(forked_attachment_id, "attachment-guidance-applied");
    let forked_guidance = service
        .load_agent_run_guidance(&forked_guidance_id)
        .unwrap()
        .unwrap();
    assert_eq!(forked_guidance.status, crate::AgentGuidanceStatus::Applied);
    assert_eq!(
        forked_guidance.attachment_ids,
        vec![forked_attachment_id.clone()]
    );

    service
        .delete_conversation("conversation-guidance-fork")
        .unwrap();
    assert!(!applied_source_path.exists());
    assert!(!abandoned_source_path.exists());
    assert!(service
        .load_input_attachments(&[
            "attachment-guidance-applied".to_string(),
            "attachment-guidance-abandoned".to_string(),
        ])
        .is_err());
    let forked_payload = service
        .load_input_attachments(&[forked_attachment_id])
        .unwrap();
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(&forked_payload[0].data)
            .unwrap(),
        b"applied guidance payload"
    );
}

#[test]
fn opening_storage_removes_orphan_attachments_and_preserves_referenced_files() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    service
        .save_conversation(conversation("conversation-1", None, "message-1"))
        .unwrap();
    service
        .save_input_attachments(
            "conversation-1",
            "message-1",
            None,
            &[input_attachment(
                "referenced",
                AgentInputAttachmentKind::File,
                "referenced.txt",
                Some("text/plain"),
                b"referenced",
            )],
            10,
        )
        .unwrap();
    let library = service
        .build_attachment_library_context("conversation-1", None)
        .unwrap();
    let referenced_path = PathBuf::from(library.root_path.unwrap())
        .join(&library.conversation_attachments[0].storage_rel_path);
    drop(service);

    let orphan_path = fixture.root.join("attachments/orphan/nested.txt");
    fs::create_dir_all(orphan_path.parent().unwrap()).unwrap();
    fs::write(&orphan_path, b"orphan").unwrap();

    let _reopened = fixture.service();
    assert!(referenced_path.is_file());
    assert!(!orphan_path.exists());
}

#[test]
fn project_attachment_library_excludes_current_conversation_and_delete_cleans_files() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    service
        .save_conversation(conversation(
            "conversation-current",
            Some("project-1"),
            "message-current",
        ))
        .unwrap();
    service
        .save_conversation(conversation(
            "conversation-other",
            Some("project-1"),
            "message-other",
        ))
        .unwrap();
    service
        .save_input_attachments(
            "conversation-current",
            "message-current",
            Some("project-1"),
            &[input_attachment(
                "current",
                AgentInputAttachmentKind::File,
                "current.txt",
                Some("text/plain"),
                b"current",
            )],
            10,
        )
        .unwrap();
    service
        .save_input_attachments(
            "conversation-other",
            "message-other",
            Some("project-1"),
            &[input_attachment(
                "other",
                AgentInputAttachmentKind::File,
                "other.txt",
                Some("text/plain"),
                b"other",
            )],
            20,
        )
        .unwrap();

    let library = service
        .build_attachment_library_context("conversation-current", Some("project-1"))
        .unwrap();

    assert_eq!(library.conversation_attachments.len(), 1);
    assert_eq!(library.conversation_attachments[0].id, "current");
    assert_eq!(library.project_attachments.len(), 1);
    assert_eq!(library.project_attachments[0].id, "other");

    let other_path = PathBuf::from(library.root_path.unwrap())
        .join(&library.project_attachments[0].storage_rel_path);
    assert!(other_path.is_file());

    service.delete_conversation("conversation-other").unwrap();

    assert!(!other_path.exists());
    let library = service
        .build_attachment_library_context("conversation-current", Some("project-1"))
        .unwrap();
    assert!(library.project_attachments.is_empty());
}
