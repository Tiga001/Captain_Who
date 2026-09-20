use super::*;
use crate::storage::models::ConversationContinuationOriginRecord;

fn resolved_payload(service: &StorageService, attachment: &AgentInputAttachment) -> Vec<u8> {
    service
        .validate_managed_input_attachment(attachment)
        .unwrap();
    match service.attachment_data(attachment).unwrap() {
        super::super::attachment_imports::AttachmentData::Managed { path, .. } => {
            fs::read(path).unwrap()
        }
    }
}

fn png_image(width: u32, height: u32) -> Vec<u8> {
    use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};
    use std::io::Cursor;

    let image = RgbaImage::from_fn(width, height, |x, y| {
        Rgba([(x % 255) as u8, (y % 255) as u8, 120, 255])
    });
    let mut bytes = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(image)
        .write_to(&mut bytes, ImageFormat::Png)
        .unwrap();
    bytes.into_inner()
}

#[test]
fn input_attachments_are_persisted_and_rehydrated() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let original_png = png_image(640, 320);
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
                &service,
                "attachment-1",
                AgentInputAttachmentKind::Image,
                "pixel.png",
                Some("image/png"),
                &original_png,
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
    let thumbnail = base64::engine::general_purpose::STANDARD
        .decode(attachment.preview_data.as_deref().unwrap())
        .unwrap();
    let thumbnail = image::load_from_memory(&thumbnail).unwrap();
    assert_eq!((thumbnail.width(), thumbnail.height()), (256, 128));

    let original = service
        .load_attachment_image("attachment-1")
        .unwrap()
        .unwrap();
    assert_eq!(original.mime_type, "image/png");
    let delivered = image::load_from_memory(
        &base64::engine::general_purpose::STANDARD
            .decode(&original.data)
            .unwrap(),
    )
    .unwrap();
    assert_eq!(delivered, image::load_from_memory(&original_png).unwrap());

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
fn projectless_agent_tree_shares_attachments_without_leaking_to_other_conversations() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    for conversation_id in [
        "conversation-root",
        "conversation-child",
        "conversation-sibling",
        "conversation-other",
    ] {
        let mut record = conversation(conversation_id, None, "unused-message");
        record.messages.clear();
        service.save_conversation(record).unwrap();
    }
    bind_agent_root(&service, "agent-root", "conversation-root");
    bind_agent_child(
        &service,
        "agent-child",
        "conversation-child",
        "agent-root",
        "conversation-root",
        "child",
    );
    bind_agent_child(
        &service,
        "agent-sibling",
        "conversation-sibling",
        "agent-root",
        "conversation-root",
        "sibling",
    );

    service
        .save_input_attachments(
            "conversation-root",
            "message-root",
            None,
            &[input_attachment(
                &service,
                "attachment-root",
                AgentInputAttachmentKind::File,
                "root.txt",
                Some("text/plain"),
                b"root",
            )],
            10,
        )
        .unwrap();
    service
        .save_input_attachments(
            "conversation-child",
            "message-child",
            None,
            &[input_attachment(
                &service,
                "attachment-child",
                AgentInputAttachmentKind::File,
                "child.txt",
                Some("text/plain"),
                b"child",
            )],
            11,
        )
        .unwrap();

    let child_library = service
        .build_attachment_library_context("conversation-child", None)
        .unwrap();
    assert_eq!(child_library.project_attachments.len(), 1);
    assert_eq!(child_library.project_attachments[0].id, "attachment-root");

    let sibling_library = service
        .build_attachment_library_context("conversation-sibling", None)
        .unwrap();
    assert_eq!(sibling_library.project_attachments.len(), 2);
    assert_eq!(
        sibling_library.project_attachments[0].id,
        "attachment-child"
    );
    assert_eq!(sibling_library.project_attachments[1].id, "attachment-root");

    let root_library = service
        .build_attachment_library_context("conversation-root", None)
        .unwrap();
    assert_eq!(root_library.project_attachments.len(), 1);
    assert_eq!(root_library.project_attachments[0].id, "attachment-child");

    assert!(service
        .build_attachment_library_context("conversation-other", None)
        .unwrap()
        .project_attachments
        .is_empty());
}

#[test]
fn forked_conversation_owns_independent_attachment_files_and_is_idempotent() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let mut source = conversation("conversation-source", Some("project-1"), "message-user");
    source.messages.push(ChatMessageRecord {
        human_interaction_response: None,
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
                &service,
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
        resolved_payload(&service, &retained_payload[0]),
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
        resolved_payload(&service, &recursive_payload[0]),
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
        human_interaction_response: None,
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
        &service,
        "attachment-guidance-applied",
        AgentInputAttachmentKind::File,
        "applied.txt",
        Some("text/plain"),
        b"applied guidance payload",
    );
    let abandoned = input_attachment(
        &service,
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
        resolved_payload(&service, &forked_payload[0]),
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
                &service,
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
                &service,
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
                &service,
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

#[cfg(unix)]
#[test]
fn conversation_preview_file_read_does_not_hold_the_storage_connection() {
    use std::ffi::CString;
    use std::io::Write;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::OpenOptionsExt;
    use std::sync::{mpsc, Arc};
    use std::thread;
    use std::time::{Duration, Instant};

    let fixture = StorageFixture::new();
    let service = Arc::new(fixture.service());
    let original_png = png_image(64, 32);
    service
        .save_conversation(conversation("conversation-lock", None, "message-lock"))
        .unwrap();
    service
        .save_input_attachments(
            "conversation-lock",
            "message-lock",
            None,
            &[input_attachment(
                &service,
                "attachment-lock",
                AgentInputAttachmentKind::Image,
                "lock.png",
                Some("image/png"),
                &original_png,
            )],
            10,
        )
        .unwrap();
    let library = service
        .build_attachment_library_context("conversation-lock", None)
        .unwrap();
    let storage_path = PathBuf::from(library.root_path.unwrap())
        .join(&library.conversation_attachments[0].storage_rel_path);
    fs::remove_file(&storage_path).unwrap();
    let fifo_path = CString::new(storage_path.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(fifo_path.as_ptr(), 0o600) }, 0);

    let (load_tx, load_rx) = mpsc::channel();
    let load_service = Arc::clone(&service);
    let loader = thread::spawn(move || {
        let result = load_service.load_conversation("conversation-lock");
        load_tx.send(result).unwrap();
    });

    let deadline = Instant::now() + Duration::from_secs(2);
    let mut writer = loop {
        match fs::OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&storage_path)
        {
            Ok(writer) => break writer,
            Err(error)
                if error.raw_os_error() == Some(libc::ENXIO) && Instant::now() < deadline =>
            {
                thread::sleep(Duration::from_millis(5));
            }
            Err(error) => panic!("failed to rendezvous with attachment preview reader: {error}"),
        }
    };

    let (query_tx, query_rx) = mpsc::channel();
    let query_service = Arc::clone(&service);
    let query = thread::spawn(move || {
        query_tx.send(query_service.load_projects()).unwrap();
    });
    let concurrent_query = query_rx.recv_timeout(Duration::from_millis(500));

    if let Err(error) = writer.write_all(&original_png) {
        assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe);
    }
    drop(writer);
    let conversation = load_rx
        .recv_timeout(Duration::from_secs(2))
        .unwrap()
        .unwrap()
        .unwrap();
    loader.join().unwrap();
    query.join().unwrap();

    assert!(
        concurrent_query.is_ok(),
        "a blocked attachment read must not retain the global SQLite connection guard"
    );
    // A FIFO cannot provide the seekable image decoder contract. It is safely omitted after
    // the blocked read, while the independent SQLite operation still completes.
    assert!(conversation.messages[0].attachments[0]
        .preview_data
        .is_none());
}

#[cfg(unix)]
#[test]
fn managed_attachment_write_rejects_non_regular_existing_target() {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let fixture = StorageFixture::new();
    let service = fixture.service();
    service
        .save_conversation(conversation("non-regular", None, "message"))
        .unwrap();
    let attachment = input_attachment(
        &service,
        "non-regular",
        AgentInputAttachmentKind::File,
        "file.txt",
        Some("text/plain"),
        b"source",
    );
    let target = service.attachment_root.join(attachment_storage_rel_path(
        "non-regular",
        "message",
        &attachment.id,
        &attachment.name,
    ));
    fs::create_dir_all(target.parent().unwrap()).unwrap();
    let path = CString::new(target.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
    assert!(service
        .save_input_attachments("non-regular", "message", None, &[attachment], 1)
        .is_err());
    assert!(service.load_projects().is_ok());
}

#[test]
fn failed_attachment_database_commit_removes_new_file() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    service
        .save_conversation(conversation(
            "conversation-commit-failure",
            None,
            "message-commit-failure",
        ))
        .unwrap();
    let attachment = input_attachment(
        &service,
        "attachment-commit-failure",
        AgentInputAttachmentKind::File,
        "cleanup.bin",
        Some("application/octet-stream"),
        b"cleanup-after-database-failure",
    );
    let storage_path = service.attachment_root.join(attachment_storage_rel_path(
        "conversation-commit-failure",
        "message-commit-failure",
        &attachment.id,
        &attachment.name,
    ));
    {
        let connection = service.state.connection().unwrap();
        connection
            .execute_batch(
                "CREATE TRIGGER reject_attachment_insert
                 BEFORE INSERT ON attachments
                 BEGIN
                     SELECT RAISE(ABORT, 'forced attachment insert failure');
                 END;",
            )
            .unwrap();
    }

    assert!(service
        .save_input_attachments(
            "conversation-commit-failure",
            "message-commit-failure",
            None,
            &[attachment],
            10,
        )
        .is_err());
    assert!(!storage_path.exists());
    let connection = service.state.connection().unwrap();
    assert!(
        attachment_repository::get_attachment(&connection, "attachment-commit-failure")
            .unwrap()
            .is_none()
    );
}

fn context_image_ref(id: &str, bytes: &[u8]) -> crate::ConversationContextImageRef {
    use sha2::{Digest, Sha256};
    crate::ConversationContextImageRef {
        attachment_id: id.to_string(),
        mime_type: "image/png".to_string(),
        sha256: format!(
            "sha256:{:x}",
            Sha256::digest(
                crate::file_input::image_delivery::prepare_model_image(std::io::Cursor::new(bytes))
                    .unwrap()
                    .bytes
            )
        ),
    }
}

#[test]
fn context_images_require_visible_owner_exact_mime_and_immutable_bytes() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let bytes = png_image(8, 8);
    service
        .save_conversation(conversation("image-owner", None, "image-user"))
        .unwrap();
    service
        .save_input_attachments(
            "image-owner",
            "image-user",
            None,
            &[input_attachment(
                &service,
                "history-image",
                AgentInputAttachmentKind::Image,
                "picture.png",
                Some("image/png"),
                &bytes,
            )],
            1,
        )
        .unwrap();
    let reference = context_image_ref("history-image", &bytes);
    let loaded = service
        .load_context_image_attachments("image-owner", &[reference.clone(), reference.clone()])
        .unwrap();
    assert_eq!(
        loaded.len(),
        1,
        "shared image lookup must not duplicate payload bytes"
    );
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(&loaded[0].data)
            .unwrap(),
        bytes
    );
    assert!(service
        .load_context_image_attachments("foreign", std::slice::from_ref(&reference))
        .unwrap_err()
        .contains("scope_mismatch"));
    let mut wrong_mime = reference.clone();
    wrong_mime.mime_type = "image/jpeg".into();
    assert!(service
        .load_context_image_attachments("image-owner", &[wrong_mime])
        .is_err());
    let mut wrong_hash = reference.clone();
    wrong_hash.sha256 = format!("sha256:{}", "0".repeat(64));
    assert!(service
        .load_context_image_attachments("image-owner", &[reference.clone(), wrong_hash.clone()])
        .unwrap_err()
        .contains("reference_conflict"));
    assert!(service
        .load_context_image_attachments("image-owner", &[wrong_hash])
        .unwrap_err()
        .contains("integrity_mismatch"));
    let library = service
        .build_attachment_library_context("image-owner", None)
        .unwrap();
    let path = PathBuf::from(library.root_path.unwrap())
        .join(&library.conversation_attachments[0].storage_rel_path);
    fs::write(&path, vec![0; bytes.len()]).unwrap();
    assert!(service
        .load_context_image_attachments("image-owner", std::slice::from_ref(&reference))
        .unwrap_err()
        .contains("integrity_mismatch"));
    fs::remove_file(path).unwrap();
    assert!(service
        .load_context_image_attachments("image-owner", &[reference])
        .unwrap_err()
        .contains("unavailable"));
}

#[test]
fn context_material_and_images_survive_restart_recursive_fork_and_source_deletion() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let bytes = png_image(8, 8);
    let mut source = conversation("material-source", None, "material-user");
    source.messages.push(ChatMessageRecord {
        human_interaction_response: None,
        id: "material-assistant".into(),
        role: "assistant".into(),
        content: "done".into(),
        created_at: 2,
        status: Some("sent".into()),
        attachments: Vec::new(),
        agent_run_json: None,
        ui_state_json: None,
    });
    source.updated_at = 3;
    service.save_conversation(source).unwrap();
    service
        .save_input_attachments(
            "material-source",
            "material-user",
            None,
            &[input_attachment(
                &service,
                "material-image",
                AgentInputAttachmentKind::Image,
                "picture.png",
                Some("image/png"),
                &bytes,
            )],
            1,
        )
        .unwrap();
    let reference = context_image_ref("material-image", &bytes);
    let trace = ConversationTurnTrace {
        schema_version: crate::CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
        run_id: "material-run".into(),
        conversation_id: "material-source".into(),
        assistant_message_id: "material-assistant".into(),
        terminal_status: crate::ConversationTurnTraceTerminalStatus::Completed,
        terminal_error: None,
        truncated: false,
        items: vec![
            ConversationTurnTraceItem::ContextMaterial {
                sequence: 0,
                event_id: "material-input".into(),
                material_kind: crate::ConversationContextMaterialKind::InputAttachment,
                content: "Original image context".into(),
                images: vec![reference],
                created_at: 2,
            },
            ConversationTurnTraceItem::ContextMaterial {
                sequence: 1,
                event_id: "material-world".into(),
                material_kind: crate::ConversationContextMaterialKind::RunWorldState,
                content: "Historical browser activation was allowed".into(),
                images: Vec::new(),
                created_at: 2,
            },
        ],
    };
    service
        .replace_conversation_turn_trace(&trace, 2, 3)
        .unwrap();
    let items = trace
        .items
        .iter()
        .map(|event| {
            let ConversationTurnTraceItem::ContextMaterial {
                sequence,
                content,
                images,
                ..
            } = event
            else {
                unreachable!()
            };
            crate::ConversationModelContextItem {
                sequence: *sequence,
                ordinal: 0,
                role: "user".into(),
                content: content.clone(),
                tool_calls: Vec::new(),
                tool_call_id: None,
                is_error: false,
                images: images.clone(),
            }
        })
        .collect::<Vec<_>>();
    {
        let connection = service.state.connection().unwrap();
        conversation_model_context_repository::commit_items_in_connection(
            &connection,
            "material-source",
            "material-assistant",
            &items,
        )
        .unwrap();
    }
    drop(service);
    let service = fixture.service();
    assert_eq!(
        service
            .get_conversation_model_context_log("material-assistant")
            .unwrap()
            .unwrap()
            .items,
        items
    );
    let mut source_id = "material-source".to_string();
    let mut assistant_id = "material-assistant".to_string();
    for index in 0..2 {
        let child = service
            .fork_conversation_request_view(assistant_reply_fork_request(
                format!("material-fork-{index}"),
                &source_id,
                &assistant_id,
            ))
            .unwrap()
            .conversation;
        let new_assistant = child
            .messages
            .iter()
            .find(|message| message.role == "assistant")
            .unwrap();
        let log = service
            .get_conversation_model_context_log(&new_assistant.id)
            .unwrap()
            .unwrap();
        assert_eq!(log.items.len(), 2);
        assert_eq!(log.items[0].content, items[0].content);
        assert_eq!(log.items[1].content, items[1].content);
        let child_image = &log.items[0].images[0];
        assert_ne!(child_image.attachment_id, "material-image");
        assert_eq!(child_image.sha256, items[0].images[0].sha256);
        assert_eq!(
            child_image.attachment_id,
            child.messages[0].attachments[0].id
        );
        assert_eq!(
            service
                .load_context_image_attachments(&child.id, std::slice::from_ref(child_image))
                .unwrap()
                .len(),
            1
        );
        service.delete_conversation(&source_id).unwrap();
        assert!(service
            .load_context_image_attachments(&child.id, std::slice::from_ref(child_image))
            .is_ok());
        source_id = child.id;
        assistant_id = new_assistant.id.clone();
    }
    service.delete_conversation(&source_id).unwrap();
    assert!(service
        .get_conversation_model_context_log(&assistant_id)
        .unwrap()
        .is_none());
}
