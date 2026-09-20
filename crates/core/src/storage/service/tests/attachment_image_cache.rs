use super::*;
use sha2::{Digest, Sha256};

#[test]
fn model_image_cache_preserves_the_exact_recorded_encoding_across_reopen() {
    let fixture = StorageFixture::new();
    let source = fixture.root.join("source-image");
    fs::write(&source, b"original image bytes").unwrap();
    let source_digest = format!("{:x}", Sha256::digest(b"original image bytes"));
    // The cache must preserve already-recorded bytes, not rerun today's image encoder.
    let rendered = b"immutable version-one PNG bytes";
    let reference = crate::ConversationContextImageRef {
        attachment_id: "picture".into(),
        mime_type: "image/png".into(),
        sha256: format!("sha256:{:x}", Sha256::digest(rendered)),
    };
    fixture
        .service()
        .cache_model_image(&source, rendered)
        .unwrap();
    let reopened = fixture.service();
    assert_eq!(
        reopened
            .read_cached_model_image(&source_digest, &reference)
            .unwrap()
            .unwrap(),
        rendered
    );
    let other_source = format!("{:x}", Sha256::digest(b"a different original"));
    assert!(reopened
        .read_cached_model_image(&other_source, &reference)
        .unwrap()
        .is_none());
    let mut invalid = reference.clone();
    invalid.sha256 = "sha256:../outside".into();
    assert!(reopened
        .read_cached_model_image(&source_digest, &invalid)
        .is_err());
}

#[test]
fn model_image_cache_rejects_tampering_instead_of_replacing_historical_bytes() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let source = fixture.root.join("source-image");
    fs::write(&source, b"source").unwrap();
    let source_digest = format!("{:x}", Sha256::digest(b"source"));
    let image_digest = format!("{:x}", Sha256::digest(b"frozen"));
    let reference = crate::ConversationContextImageRef {
        attachment_id: "picture".into(),
        mime_type: "image/png".into(),
        sha256: format!("sha256:{image_digest}"),
    };
    service.cache_model_image(&source, b"frozen").unwrap();
    let cached = fixture
        .root
        .join("attachment-model-images/v1")
        .join(&source_digest)
        .join(format!("{image_digest}.png"));
    fs::write(&cached, b"edited").unwrap();
    assert!(service
        .read_cached_model_image(&source_digest, &reference)
        .unwrap_err()
        .contains("integrity_mismatch"));
    assert!(service.cache_model_image(&source, b"frozen").is_err());
}

#[test]
fn model_image_cache_collection_preserves_fork_sources_then_removes_orphans() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let source = fixture.root.join("original-source");
    let mut source_bytes = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgb8(1, 1)
        .write_to(&mut source_bytes, image::ImageFormat::Png)
        .unwrap();
    let source_bytes = source_bytes.into_inner();
    fs::write(&source, &source_bytes).unwrap();
    let source_digest = format!("{:x}", Sha256::digest(&source_bytes));
    let reference = crate::ConversationContextImageRef {
        attachment_id: "fork-picture".into(),
        mime_type: "image/png".into(),
        sha256: format!("sha256:{:x}", Sha256::digest(b"frozen")),
    };
    service.cache_model_image(&source, b"frozen").unwrap();
    service.cleanup_model_image_cache().unwrap();
    assert!(service
        .read_cached_model_image(&source_digest, &reference)
        .unwrap()
        .is_some());

    // Forks copy original files and retain the same model-image digest without re-encoding.
    // A copied source with no cache-origin hint must still keep these exact historical bytes.
    service
        .save_conversation(conversation("fork", None, "fork-message"))
        .unwrap();
    let relative = attachment_storage_rel_path("fork", "fork-message", "fork-picture", "fork.png");
    let copied = service.attachment_root.join(&relative);
    fs::create_dir_all(copied.parent().unwrap()).unwrap();
    fs::copy(&source, &copied).unwrap();
    attachment_repository::save_attachment(
        &service.state.connection().unwrap(),
        &AttachmentRecord {
            id: "fork-picture".into(),
            conversation_id: "fork".into(),
            message_id: "fork-message".into(),
            project_id: None,
            kind: "image".into(),
            original_name: "fork.png".into(),
            mime_type: Some("image/png".into()),
            size_bytes: source_bytes.len() as u64,
            storage_rel_path: slash_path(&relative),
            created_at: 1,
        },
    )
    .unwrap();
    fs::remove_file(source).unwrap();
    service.cleanup_model_image_cache().unwrap();
    assert_eq!(
        service
            .read_cached_model_image(&source_digest, &reference)
            .unwrap()
            .unwrap(),
        b"frozen"
    );

    let connection = service.state.connection().unwrap();
    let records =
        attachment_repository::list_conversation_attachments(&connection, "fork").unwrap();
    connection
        .execute("DELETE FROM attachments WHERE conversation_id = 'fork'", [])
        .unwrap();
    drop(connection);
    service.cleanup_attachment_files(records).unwrap();
    service.cleanup_model_image_cache().unwrap();
    assert!(service
        .read_cached_model_image(&source_digest, &reference)
        .unwrap()
        .is_none());
}
