use super::*;

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
        agent_run_json: None,
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

    let input = ForkConversationInput {
        request_id: "fork-request-1".to_string(),
        source_conversation_id: "conversation-source".to_string(),
        through_assistant_message_id: "message-assistant".to_string(),
    };
    let forked = service.fork_conversation(input.clone()).unwrap();
    assert_eq!(forked.project_id.as_deref(), Some("project-1"));
    assert_eq!(forked.messages.len(), 2);
    let forked_attachment = forked.messages[0].attachments.as_slice();
    assert_eq!(forked_attachment.len(), 1);
    assert_ne!(forked_attachment[0].id, "attachment-source");

    let retry = service.fork_conversation(input).unwrap();
    assert_eq!(retry.id, forked.id);
    assert_eq!(service.load_conversations().unwrap().len(), 2);

    service.delete_conversation("conversation-source").unwrap();
    let retained = service.load_conversation(&forked.id).unwrap().unwrap();
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
        .fork_conversation(ForkConversationInput {
            request_id: "fork-request-2".to_string(),
            source_conversation_id: retained.id.clone(),
            through_assistant_message_id: retained_assistant_id,
        })
        .unwrap();
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
