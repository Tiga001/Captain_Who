use super::*;
use crate::storage::models::ConversationContinuationOriginRecord;

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
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(attachment.preview_data.as_deref().unwrap())
            .unwrap(),
        original_png
    );

    let original = service
        .load_attachment_image("attachment-1")
        .unwrap()
        .unwrap();
    assert_eq!(original.mime_type, "image/png");
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(&original.data)
            .unwrap(),
        original_png
    );

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

    writer.write_all(&original_png).unwrap();
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
    let preview_data = conversation.messages[0].attachments[0]
        .preview_data
        .as_deref()
        .unwrap();
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(preview_data)
            .unwrap(),
        original_png
    );
}

#[cfg(unix)]
#[test]
fn input_attachment_file_write_does_not_hold_the_storage_connection() {
    use std::ffi::CString;
    use std::io::Read;
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::OpenOptionsExt;
    use std::sync::{mpsc, Arc};
    use std::thread;
    use std::time::{Duration, Instant};

    let fixture = StorageFixture::new();
    let service = Arc::new(fixture.service());
    service
        .save_conversation(conversation(
            "conversation-write-lock",
            None,
            "message-write-lock",
        ))
        .unwrap();
    let payload = vec![0x5a; 2 * 1024 * 1024];
    let attachment = input_attachment(
        "attachment-write-lock",
        AgentInputAttachmentKind::File,
        "slow.bin",
        Some("application/octet-stream"),
        &payload,
    );
    let storage_path = service.attachment_root.join(attachment_storage_rel_path(
        "conversation-write-lock",
        "message-write-lock",
        &attachment.id,
        &attachment.name,
    ));
    fs::create_dir_all(storage_path.parent().unwrap()).unwrap();
    let fifo_path = CString::new(storage_path.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(fifo_path.as_ptr(), 0o600) }, 0);
    let mut reader = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(&storage_path)
        .unwrap();

    let (save_tx, save_rx) = mpsc::channel();
    let save_service = Arc::clone(&service);
    let saver = thread::spawn(move || {
        let result = save_service.save_input_attachments(
            "conversation-write-lock",
            "message-write-lock",
            None,
            &[attachment],
            10,
        );
        save_tx.send(result).unwrap();
    });

    let deadline = Instant::now() + Duration::from_secs(2);
    let mut first_byte = [0_u8; 1];
    loop {
        match reader.read(&mut first_byte) {
            Ok(1) => break,
            Ok(_) if Instant::now() < deadline => thread::sleep(Duration::from_millis(5)),
            Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline =>
            {
                thread::sleep(Duration::from_millis(5));
            }
            Ok(_) => panic!("timed out waiting for the attachment writer"),
            Err(error) => panic!("failed to rendezvous with attachment writer: {error}"),
        }
    }

    let (query_tx, query_rx) = mpsc::channel();
    let query_service = Arc::clone(&service);
    let query = thread::spawn(move || {
        query_tx.send(query_service.load_projects()).unwrap();
    });
    let concurrent_query = query_rx.recv_timeout(Duration::from_millis(500));

    let flags = unsafe { libc::fcntl(reader.as_raw_fd(), libc::F_GETFL) };
    assert!(flags >= 0);
    assert_eq!(
        unsafe { libc::fcntl(reader.as_raw_fd(), libc::F_SETFL, flags & !libc::O_NONBLOCK) },
        0
    );
    let mut remaining = Vec::new();
    reader.read_to_end(&mut remaining).unwrap();
    assert_eq!(remaining.len() + 1, payload.len());
    save_rx
        .recv_timeout(Duration::from_secs(2))
        .unwrap()
        .unwrap();
    saver.join().unwrap();
    query.join().unwrap();

    assert!(
        concurrent_query.is_ok(),
        "a blocked attachment write must not retain the global SQLite connection guard"
    );
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
