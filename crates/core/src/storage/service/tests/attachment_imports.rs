use super::*;
use crate::AttachmentImportInput;

#[test]
fn image_cache_write_failure_keeps_new_image_history_unpublished_and_import_retryable() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let mut encoded = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(image::RgbImage::new(2, 2))
        .write_to(&mut encoded, image::ImageFormat::Png)
        .unwrap();
    let bytes = encoded.into_inner();
    let attachment = input_attachment(
        &service,
        "cache-failure",
        AgentInputAttachmentKind::Image,
        "picture.png",
        Some("image/png"),
        &bytes,
    );
    service
        .save_conversation(conversation("cache-failure", None, "message"))
        .unwrap();
    let cache_root = fixture.root.join("attachment-model-images");
    fs::remove_dir_all(&cache_root).unwrap();
    fs::write(&cache_root, b"blocks directory creation").unwrap();
    assert!(service
        .save_input_attachments(
            "cache-failure",
            "message",
            None,
            std::slice::from_ref(&attachment),
            1,
        )
        .is_err());
    assert!(service
        .load_input_attachments(&["cache-failure".into()])
        .is_err());
    let original_path = service.attachment_root.join(attachment_storage_rel_path(
        "cache-failure",
        "message",
        "cache-failure",
        "picture.png",
    ));
    assert!(!original_path.exists());
    let import_id = service
        .begin_attachment_import(AttachmentImportInput {
            id: "retryable-cache".into(),
            kind: attachment.kind,
            name: attachment.name.clone(),
            mime_type: attachment.mime_type.clone(),
            pasted_text: None,
            size_bytes: bytes.len() as u64,
        })
        .unwrap();
    service
        .append_attachment_import(
            &import_id,
            0,
            &base64::engine::general_purpose::STANDARD.encode(&bytes),
        )
        .unwrap();
    assert!(service.finish_attachment_import(&import_id).is_err());
    fs::remove_file(&cache_root).unwrap();
    let finished = service.finish_attachment_import(&import_id).unwrap();
    service
        .validate_managed_input_attachment(&finished)
        .unwrap();
}

fn begin(service: &StorageService, id: &str, size: u64) -> String {
    service
        .begin_attachment_import(AttachmentImportInput {
            id: id.into(),
            kind: AgentInputAttachmentKind::File,
            name: "large.bin".into(),
            mime_type: Some("application/octet-stream".into()),
            pasted_text: None,
            size_bytes: size,
        })
        .unwrap()
}

#[test]
fn managed_attachment_import_is_chunked_durable_and_saved_without_inline_payload() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let chunk = vec![b'a'; 512 * 1024];
    let encoded = base64::engine::general_purpose::STANDARD.encode(&chunk);
    let total = chunk.len() as u64 * 130; // Exceeds every former ordinary guidance byte cap.
    let import_id = begin(&service, "large-import", total);
    for index in 0..130u64 {
        assert_eq!(
            service
                .append_attachment_import(&import_id, index * chunk.len() as u64, &encoded)
                .unwrap(),
            (index + 1) * chunk.len() as u64
        );
    }
    let attachment = service.finish_attachment_import(&import_id).unwrap();
    assert_eq!(attachment.encoding, AgentInputAttachmentEncoding::Managed);
    assert!(attachment.content_sha256.as_deref().is_some_and(|digest| {
        digest.len() == 71
            && digest.starts_with("sha256:")
            && digest[7..].bytes().all(|byte| byte.is_ascii_hexdigit())
    }));
    assert!(serde_json::to_string(&attachment).unwrap().len() < 512);
    drop(service);
    let service = fixture.service();
    service
        .validate_managed_input_attachment(&attachment)
        .unwrap();
    service
        .save_conversation(conversation(
            "managed-conversation",
            None,
            "managed-message",
        ))
        .unwrap();
    service
        .save_input_attachments(
            "managed-conversation",
            "managed-message",
            None,
            &[attachment],
            10,
        )
        .unwrap();
    let loaded = service
        .load_input_attachments(&["large-import".into()])
        .unwrap();
    assert_eq!(loaded[0].encoding, AgentInputAttachmentEncoding::Managed);
    assert!(loaded[0].data.len() < 64);
    service
        .validate_managed_input_attachment(&loaded[0])
        .unwrap();
    // Edit/rewrite can mint a new attachment identity without copying bytes through IPC.
    let mut edited = loaded[0].clone();
    edited.id = "edited-large-import".into();
    let prepared = service
        .prepare_conversation_turn_rewrite_attachments(
            "managed-conversation",
            "edited-message",
            None,
            &[edited],
            20,
        )
        .unwrap();
    assert_eq!(prepared.records[0].size_bytes, total);
    service.discard_prepared_conversation_turn_rewrite_attachments(prepared);
}

#[test]
fn managed_import_rejects_incomplete_offsets_metadata_forgery_and_changed_content() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let id = begin(&service, "guarded-import", 3);
    assert!(service.finish_attachment_import(&id).is_err());
    assert!(service.append_attachment_import(&id, 1, "YWJj").is_err());
    assert!(service
        .append_attachment_import(&id, 0, "YWJjZA==")
        .is_err());
    assert!(service
        .append_attachment_import("../escape", 0, "YWJj")
        .is_err());
    service.append_attachment_import(&id, 0, "YWJj").unwrap();
    let attachment = service.finish_attachment_import(&id).unwrap();
    assert_eq!(service.finish_attachment_import(&id).unwrap(), attachment);
    assert!(service.append_attachment_import(&id, 3, "").is_err());
    let mut forged = attachment.clone();
    forged.size_bytes = 4;
    assert!(service.validate_managed_input_attachment(&forged).is_err());
    let payload = fixture
        .root
        .join("attachment-imports/v1")
        .join(&id)
        .join("payload");
    fs::write(payload, b"xyz").unwrap();
    assert!(service
        .validate_managed_input_attachment(&attachment)
        .is_err());
    let incomplete = begin(&service, "cancelled-import", 3);
    service.cancel_attachment_import(&incomplete).unwrap();
    service.cancel_attachment_import(&incomplete).unwrap();
    assert!(service.finish_attachment_import(&incomplete).is_err());
}

#[test]
fn import_cleanup_preserves_draft_and_queued_refs_and_collects_only_expired_orphans() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let import = |id: &str| {
        let token = begin(&service, id, 3);
        service.append_attachment_import(&token, 0, "YWJj").unwrap();
        service.finish_attachment_import(&token).unwrap()
    };
    let current = import("current");
    let queued = import("queued");
    let orphan = import("orphan");
    let mut draft = composer_draft("draft-import", None, "message");
    draft.attachments_json = serde_json::to_string(&[&current]).unwrap();
    draft.queued_messages_json = serde_json::json!([{
        "id":"queued-1","clientMessageId":"client-1","content":"guide",
        "attachments":[queued],"modelId":"model-default","permissionMode":"default",
        "projectId":null,"skills":[],"status":"pending","createdAt":1
    }])
    .to_string();
    service.save_composer_draft(draft).unwrap();
    service.cleanup_attachment_imports(now_ms()).unwrap();
    service.validate_managed_input_attachment(&orphan).unwrap();
    service
        .cleanup_attachment_imports(now_ms() + 8 * 24 * 60 * 60 * 1000)
        .unwrap();
    service.validate_managed_input_attachment(&current).unwrap();
    service.validate_managed_input_attachment(&queued).unwrap();
    assert!(service.validate_managed_input_attachment(&orphan).is_err());
}

#[test]
fn managed_image_previews_are_bounded_and_historical_derivatives_rehydrate() {
    use image::{DynamicImage, ImageFormat, Rgb, RgbImage};
    use sha2::{Digest, Sha256};
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let image = DynamicImage::ImageRgb8(RgbImage::from_pixel(2400, 800, Rgb([33, 77, 120])));
    let mut bytes = std::io::Cursor::new(Vec::new());
    image.write_to(&mut bytes, ImageFormat::Jpeg).unwrap();
    let bytes = bytes.into_inner();
    let id = service
        .begin_attachment_import(AttachmentImportInput {
            id: "photo".into(),
            kind: AgentInputAttachmentKind::Image,
            name: "photo.jpg".into(),
            mime_type: Some("image/jpeg".into()),
            pasted_text: None,
            size_bytes: bytes.len() as u64,
        })
        .unwrap();
    for (index, chunk) in bytes.chunks(512 * 1024).enumerate() {
        service
            .append_attachment_import(
                &id,
                (index * 512 * 1024) as u64,
                &base64::engine::general_purpose::STANDARD.encode(chunk),
            )
            .unwrap();
    }
    let attachment = service.finish_attachment_import(&id).unwrap();
    let preview = service
        .load_input_attachment_preview(&attachment)
        .unwrap()
        .unwrap();
    assert_eq!(preview.mime_type, "image/png");
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(preview.data)
        .unwrap();
    let decoded = image::load_from_memory(&decoded).unwrap();
    assert!(decoded.width() <= 256 && decoded.height() <= 256);
    service
        .save_conversation(conversation("image-history", None, "image-message"))
        .unwrap();
    service
        .save_input_attachments(
            "image-history",
            "image-message",
            None,
            std::slice::from_ref(&attachment),
            10,
        )
        .unwrap();
    let delivery = service
        .load_input_attachment_preview_for_display(&attachment, true)
        .unwrap()
        .unwrap();
    let delivery_bytes = base64::engine::general_purpose::STANDARD
        .decode(delivery.data)
        .unwrap();
    let reference = crate::ConversationContextImageRef {
        attachment_id: "photo".into(),
        mime_type: "image/png".into(),
        sha256: format!("sha256:{:x}", Sha256::digest(&delivery_bytes)),
    };
    drop(service);
    let service = fixture.service();
    let restored = service
        .load_context_image_attachments("image-history", &[reference])
        .unwrap();
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(&restored[0].data)
            .unwrap(),
        delivery_bytes
    );
    let source = service.load_input_attachments(&["photo".into()]).unwrap();
    assert_eq!(source[0].mime_type.as_deref(), Some("image/jpeg"));
    assert_eq!(source[0].size_bytes, bytes.len() as u64);
}

pub(super) fn pasted_attachment(
    service: &StorageService,
    id: &str,
    text: &str,
) -> AgentInputAttachment {
    let import_id = service
        .begin_attachment_import(AttachmentImportInput {
            id: id.into(),
            kind: AgentInputAttachmentKind::File,
            name: "pasted-text.txt".into(),
            mime_type: Some("text/plain".into()),
            size_bytes: text.len() as u64,
            pasted_text: Some(crate::AgentPastedTextMetadata {
                preview: text.chars().take(80).collect(),
                character_count: text.encode_utf16().count() as u64,
            }),
        })
        .unwrap();
    for (index, chunk) in text.as_bytes().chunks(512 * 1024).enumerate() {
        service
            .append_attachment_import(
                &import_id,
                (index * 512 * 1024) as u64,
                &base64::engine::general_purpose::STANDARD.encode(chunk),
            )
            .unwrap();
    }
    service.finish_attachment_import(&import_id).unwrap()
}

#[test]
fn pasted_text_restores_original_utf8_and_metadata_survives_restart_and_sent_hydration() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let text = format!("  第一行🙂\r\n{}\n结束\t", "世界🌍\n".repeat(20_000));
    let attachment = pasted_attachment(&service, "pasted-roundtrip", &text);
    assert_eq!(
        service.load_input_attachment_text(&attachment).unwrap(),
        text
    );
    assert_eq!(
        attachment.pasted_text.as_ref().unwrap().character_count,
        text.encode_utf16().count() as u64
    );
    assert!(serde_json::to_string(&attachment).unwrap().len() < 1000);
    drop(service);
    let service = fixture.service();
    assert_eq!(
        service.load_input_attachment_text(&attachment).unwrap(),
        text
    );
    service
        .save_conversation(conversation("paste-chat", None, "paste-message"))
        .unwrap();
    service
        .save_input_attachments(
            "paste-chat",
            "paste-message",
            None,
            std::slice::from_ref(&attachment),
            10,
        )
        .unwrap();
    assert_eq!(
        service.load_input_attachment_text(&attachment).unwrap(),
        text
    );
    let stored = service
        .load_input_attachments(std::slice::from_ref(&attachment.id))
        .unwrap();
    assert_eq!(stored[0].pasted_text, attachment.pasted_text);
    // Durable references are also recovered into editable unsent queues. Reading them must
    // preserve the original sent record and text.
    assert_eq!(
        service.load_input_attachment_text(&stored[0]).unwrap(),
        text
    );
    let library = service
        .build_attachment_library_context("paste-chat", None)
        .unwrap();
    assert_eq!(
        library.conversation_attachments[0].pasted_text,
        attachment.pasted_text
    );
    let conversation = service.load_conversation("paste-chat").unwrap().unwrap();
    assert_eq!(
        conversation.messages[0].attachments[0].pasted_text,
        attachment.pasted_text
    );
    let file = service
        .resolve_attachment_file(&attachment.id, "paste-message")
        .unwrap();
    assert_eq!(fs::read_to_string(&file.path).unwrap(), text);
    assert_eq!(file.conversation_id, "paste-chat");
    assert_eq!(file.message_id, "paste-message");
    assert_eq!(file.mime_type.as_deref(), Some("text/plain"));
    assert_eq!(file.size_bytes, text.len() as u64);
}

#[test]
fn pasted_text_restore_rejects_identity_metadata_digest_and_content_mismatch() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let attachment = pasted_attachment(&service, "pasted-validation", "original 🙂\r\n");
    let mut forged = attachment.clone();
    forged.id = "other-identity".into();
    assert!(service.load_input_attachment_text(&forged).is_err());
    forged = attachment.clone();
    forged.pasted_text.as_mut().unwrap().preview = "different".into();
    assert!(service.load_input_attachment_text(&forged).is_err());
    assert!(service.validate_managed_input_attachment(&forged).is_err());
    forged = attachment.clone();
    forged.content_sha256 = None;
    assert!(service.load_input_attachment_text(&forged).is_err());
    forged = attachment.clone();
    forged.encoding = AgentInputAttachmentEncoding::Utf8;
    assert!(service.load_input_attachment_text(&forged).is_err());
    forged = attachment.clone();
    forged.data = "../outside".into();
    assert!(service.load_input_attachment_text(&forged).is_err());
    let super::super::attachment_imports::AttachmentData::Managed { path, .. } =
        service.attachment_data(&attachment).unwrap();
    fs::write(path, "replaced 🙂\r\n").unwrap();
    assert!(service.load_input_attachment_text(&attachment).is_err());
}

#[test]
fn pasted_text_import_rejects_invalid_metadata_utf8_and_character_count() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let input = AttachmentImportInput {
        id: "invalid-paste".into(),
        kind: AgentInputAttachmentKind::File,
        name: "paste.txt".into(),
        mime_type: Some("text/plain".into()),
        size_bytes: 2,
        pasted_text: Some(crate::AgentPastedTextMetadata {
            preview: "".into(),
            character_count: 1,
        }),
    };
    let mut bad = input.clone();
    bad.kind = AgentInputAttachmentKind::Image;
    assert!(service.begin_attachment_import(bad).is_err());
    let mut bad = input.clone();
    bad.pasted_text.as_mut().unwrap().preview = "a".repeat(81);
    assert!(service.begin_attachment_import(bad).is_err());
    for bytes in [vec![0xff, 0xfe], b"ab".to_vec()] {
        let import_id = service.begin_attachment_import(input.clone()).unwrap();
        service
            .append_attachment_import(
                &import_id,
                0,
                &base64::engine::general_purpose::STANDARD.encode(bytes),
            )
            .unwrap();
        assert!(service.finish_attachment_import(&import_id).is_err());
    }
}

#[test]
fn sent_file_resolver_rejects_wrong_owner_pending_missing_and_escaped_storage() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let attachment = input_attachment(
        &service,
        "preview-file",
        AgentInputAttachmentKind::File,
        "notes.txt",
        Some("text/plain"),
        b"abc",
    );
    service
        .save_conversation(conversation("preview-chat", None, "preview-message"))
        .unwrap();
    service
        .save_input_attachments(
            "preview-chat",
            "preview-message",
            None,
            std::slice::from_ref(&attachment),
            10,
        )
        .unwrap();
    let resolved = service
        .resolve_attachment_file(&attachment.id, "preview-message")
        .unwrap();
    assert_eq!(fs::read(&resolved.path).unwrap(), b"abc");
    assert!(service.load_input_attachment_text(&attachment).is_err());
    assert!(service
        .resolve_attachment_file(&attachment.id, "wrong-message")
        .is_err());
    assert!(service
        .resolve_attachment_file("missing", "preview-message")
        .is_err());
    service
        .state
        .connection()
        .unwrap()
        .execute(
            "UPDATE messages SET status = 'pending' WHERE id = 'preview-message'",
            [],
        )
        .unwrap();
    assert!(service
        .resolve_attachment_file(&attachment.id, "preview-message")
        .is_err());
    service
        .state
        .connection()
        .unwrap()
        .execute(
            "UPDATE messages SET status = 'sent' WHERE id = 'preview-message'",
            [],
        )
        .unwrap();
    service
        .state
        .connection()
        .unwrap()
        .execute(
            "UPDATE attachments SET kind = 'image' WHERE id = 'preview-file'",
            [],
        )
        .unwrap();
    assert!(service
        .resolve_attachment_file(&attachment.id, "preview-message")
        .is_err());
    service.state.connection().unwrap().execute("UPDATE attachments SET kind = 'file', storage_rel_path = '../outside.txt' WHERE id = 'preview-file'", []).unwrap();
    assert!(service
        .resolve_attachment_file(&attachment.id, "preview-message")
        .is_err());
    service
        .state
        .connection()
        .unwrap()
        .execute(
            "UPDATE attachments SET storage_rel_path = 'missing.txt' WHERE id = 'preview-file'",
            [],
        )
        .unwrap();
    assert!(service
        .resolve_attachment_file(&attachment.id, "preview-message")
        .is_err());
}

#[test]
fn pasted_text_metadata_survives_draft_and_queue_storage_validation() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let attachment = pasted_attachment(&service, "pasted-draft", "draft\n原文🙂");
    let mut draft = composer_draft("pasted-draft-scope", None, "");
    draft.attachments_json = serde_json::to_string(&vec![attachment.clone()]).unwrap();
    draft.queued_messages_json = serde_json::json!([{
        "id": "queued-paste", "clientMessageId": "queued-client", "content": "",
        "attachments": [attachment], "modelId": "model-1", "permissionMode": "default",
        "projectId": null, "skills": [], "status": "pending", "createdAt": 1,
    }])
    .to_string();
    service.save_composer_draft(draft.clone()).unwrap();
    let restored = service
        .load_composer_draft("pasted-draft-scope")
        .unwrap()
        .unwrap();
    assert_eq!(restored.attachments_json, draft.attachments_json);
    assert_eq!(restored.queued_messages_json, draft.queued_messages_json);
    let restored_attachment: Vec<AgentInputAttachment> =
        serde_json::from_str(&restored.attachments_json).unwrap();
    assert_eq!(
        service
            .load_input_attachment_text(&restored_attachment[0])
            .unwrap(),
        "draft\n原文🙂"
    );
}
