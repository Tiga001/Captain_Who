use super::*;

#[test]
fn composer_drafts_only_preserve_full_for_current_permission_semantics() {
    let fixture = StorageFixture::new();
    let service = fixture.service();

    let mut legacy = composer_draft("legacy", None, "legacy full");
    legacy.permission_mode = "full".to_string();
    legacy.permission_mode_version = 0;
    let legacy = service.save_composer_draft(legacy).unwrap();
    assert_eq!(legacy.permission_mode, "default");

    let mut current = composer_draft("current", None, "current full");
    current.permission_mode = "full".to_string();
    current.permission_mode_version =
        crate::storage::models::CURRENT_COMPOSER_PERMISSION_MODE_VERSION;
    current.queued_messages_json = r#"[{"id":"queued-1","clientMessageId":"client-1","content":"guide","attachments":[],"modelId":"model-default","permissionMode":"full","projectId":null,"skills":[],"status":"pending","createdAt":1}]"#.to_string();
    let current = service.save_composer_draft(current).unwrap();
    assert_eq!(current.permission_mode, "full");
    assert_eq!(
        current.queued_messages_json,
        r#"[{"id":"queued-1","clientMessageId":"client-1","content":"guide","attachments":[],"modelId":"model-default","permissionMode":"full","projectId":null,"skills":[],"status":"pending","createdAt":1}]"#
    );

    let stored = service.load_composer_drafts().unwrap();
    assert_eq!(
        stored
            .iter()
            .find(|draft| draft.scope_id == "legacy")
            .unwrap()
            .permission_mode,
        "default"
    );
    assert_eq!(
        stored
            .iter()
            .find(|draft| draft.scope_id == "current")
            .unwrap()
            .permission_mode,
        "full"
    );
    assert_eq!(
        stored
            .iter()
            .find(|draft| draft.scope_id == "current")
            .unwrap()
            .queued_messages_json,
        r#"[{"id":"queued-1","clientMessageId":"client-1","content":"guide","attachments":[],"modelId":"model-default","permissionMode":"full","projectId":null,"skills":[],"status":"pending","createdAt":1}]"#
    );
}

#[test]
fn composer_draft_message_save_preserves_heavy_payloads_and_rejects_stale_text() {
    let fixture = StorageFixture::new();
    let service = fixture.service();

    let mut draft = composer_draft("message-only", None, "before");
    draft.updated_at = 10;
    draft.attachments_json = r#"[{"id":"attachment-1","kind":"file","name":"large.bin","sizeBytes":1,"encoding":"base64","data":"YQ=="}]"#.to_string();
    draft.skills_json =
        r#"[{"id":"workspace:workspace:skill-1","revision":"skill-sha256-v1:test"}]"#.to_string();
    service.save_composer_draft(draft.clone()).unwrap();

    assert!(service
        .save_composer_draft_message("message-only", "after", 11)
        .unwrap());
    assert!(service
        .save_composer_draft_message("message-only", "stale", 9)
        .unwrap());
    assert!(!service
        .save_composer_draft_message("missing", "seed me", 1)
        .unwrap());

    let stored = service
        .load_composer_drafts()
        .unwrap()
        .into_iter()
        .find(|candidate| candidate.scope_id == "message-only")
        .unwrap();
    assert_eq!(stored.message, "after");
    assert_eq!(stored.updated_at, 11);
    assert_eq!(stored.attachments_json, draft.attachments_json);
    assert_eq!(stored.skills_json, draft.skills_json);
}

#[test]
fn composer_draft_save_rejects_non_current_nested_payloads() {
    let fixture = StorageFixture::new();
    let service = fixture.service();

    let serialized = serde_json::to_value(composer_draft("draft-wire", None, "draft")).unwrap();
    for missing in ["modelId", "projectId"] {
        let mut incomplete = serialized.clone();
        incomplete.as_object_mut().unwrap().remove(missing);
        assert!(serde_json::from_value::<ComposerDraftRecord>(incomplete).is_err());
    }

    let mut missing_queue_fields = composer_draft("draft-missing-queue", None, "draft");
    missing_queue_fields.queued_messages_json =
        r#"[{"id":"queued-1","clientMessageId":"client-1","content":"guide"}]"#.to_string();
    assert_eq!(
        service
            .save_composer_draft(missing_queue_fields)
            .unwrap_err(),
        "stored_composer_draft_malformed"
    );

    let mut extra_attachment_field = composer_draft("draft-extra-attachment", None, "draft");
    extra_attachment_field.attachments_json = r#"[{"id":"attachment-1","kind":"file","name":"empty.txt","sizeBytes":0,"encoding":"utf8","data":"","future":true}]"#.to_string();
    assert_eq!(
        service
            .save_composer_draft(extra_attachment_field)
            .unwrap_err(),
        "stored_composer_draft_malformed"
    );

    let mut malformed_skill = composer_draft("draft-malformed-skill", None, "draft");
    malformed_skill.skills_json =
        r#"[{"id":"missing-source-shape","revision":"revision"}]"#.to_string();
    assert_eq!(
        service.save_composer_draft(malformed_skill).unwrap_err(),
        "stored_composer_draft_malformed"
    );
}

#[test]
fn composer_draft_load_rejects_a_corrupt_current_row_instead_of_filtering_items() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let connection = rusqlite::Connection::open(fixture.root.join("storage.sqlite")).unwrap();
    connection
        .execute(
            "INSERT INTO composer_drafts (
                scope_id, message, permission_mode, permission_mode_version, model_id, project_id,
                attachments_json, skills_json, queued_messages_json, updated_at
             ) VALUES (?1, '', 'default', ?2, NULL, NULL, '[]', '[]', '[{}]', 1)",
            rusqlite::params![
                "draft-corrupt-load",
                crate::storage::models::CURRENT_COMPOSER_PERMISSION_MODE_VERSION
            ],
        )
        .unwrap();

    assert_eq!(
        service.load_composer_drafts().unwrap_err(),
        "stored_composer_draft_malformed"
    );
}

#[test]
fn renderer_message_insert_service_returns_authoritative_retries() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let initial = conversation("insertion-retry", None, "message-first");
    service.save_conversation(initial.clone()).unwrap();
    let initial = service.load_conversation(&initial.id).unwrap().unwrap();
    let mut stale = initial.messages[0].clone();
    stale.created_at = 9;
    stale.content = "late optimistic text".into();
    let returned = service
        .upsert_chat_messages(&initial.id, vec![stale], 99)
        .unwrap();
    assert_eq!(
        serde_json::to_value(returned).unwrap(),
        serde_json::to_value(&initial.messages).unwrap()
    );
    drop(service);
    let reopened = fixture.service();
    assert_eq!(
        serde_json::to_value(
            reopened
                .load_conversation(&initial.id)
                .unwrap()
                .unwrap()
                .messages
        )
        .unwrap(),
        serde_json::to_value(initial.messages).unwrap()
    );
}
