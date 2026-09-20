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
