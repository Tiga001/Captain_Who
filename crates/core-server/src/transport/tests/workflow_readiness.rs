use super::*;

fn request(storage: &StorageService, service: &AgentService, method: &str, params: Value) -> Value {
    let (notifications, _receiver) = crate::transport::outbound_channel();
    handle_request(
        storage,
        service,
        notifications,
        JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: JsonRpcId::Number(1),
            method: method.into(),
            params: Some(params),
        },
    )
}

fn meta() -> mycopilot_core::storage::models::ChatConversationMetaRecord {
    mycopilot_core::storage::models::ChatConversationMetaRecord {
        id: "workflow-target".into(),
        project_id: None,
        model_id: None,
        title: "Target".into(),
        created_at: 1,
        updated_at: 1,
        pinned_at: None,
        archived_at: None,
        unread_at: None,
    }
}

#[test]
fn workflow_readiness_composer_only_invalidates_changed_configuration_after_success() {
    let directory = tempfile::tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&directory.path().join("storage.sqlite")).unwrap());
    storage.save_conversation_meta(meta()).unwrap();
    let service = AgentService::new_authorized_for_test(storage.clone());
    let generation = || service.workflow_readiness_generation_for_test();
    let mut draft = json!({
        "scopeId": "workflow-target", "message": "first", "permissionMode": "default",
        "permissionModeVersion": 2, "modelId": null, "projectId": null,
        "attachmentsJson": "[]", "folderReferencesJson": "[]", "skillsJson": "[]",
        "queuedMessagesJson": "[]", "updatedAt": 1
    });
    let save = |draft: &Value| {
        request(
            &storage,
            &service,
            STORAGE_SAVE_COMPOSER_DRAFT_METHOD,
            json!({"draft": draft}),
        )
    };
    let initial = generation();
    assert!(save(&draft)["result"].is_object());
    assert!(generation() > initial);
    let after_create = generation();
    draft["message"] = json!("typing must not defeat a workflow backoff");
    draft["updatedAt"] = json!(2);
    assert!(save(&draft)["result"].is_object());
    assert_eq!(generation(), after_create);
    assert!(request(
        &storage,
        &service,
        STORAGE_SAVE_COMPOSER_DRAFT_MESSAGE_METHOD,
        json!({"scopeId":"workflow-target", "message":"more typing", "updatedAt":3})
    )["error"]
        .is_null());
    assert_eq!(generation(), after_create);
    draft["modelId"] = json!("changed-model");
    draft["updatedAt"] = json!(4);
    assert!(save(&draft)["result"].is_object());
    assert!(generation() > after_create);
    let after_model = generation();
    draft["modelId"] = json!("stale-model");
    draft["updatedAt"] = json!(1);
    assert!(save(&draft)["result"].is_object());
    assert_eq!(generation(), after_model);
    draft["permissionMode"] = json!("invalid-permission");
    assert!(save(&draft)["error"].is_object());
    assert_eq!(generation(), after_model);
    assert_eq!(
        storage
            .load_composer_draft("workflow-target")
            .unwrap()
            .unwrap()
            .permission_mode,
        "default"
    );
}

#[test]
fn workflow_readiness_meta_ignores_unread_and_title_but_observes_model_and_archive() {
    let directory = tempfile::tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&directory.path().join("storage.sqlite")).unwrap());
    let service = AgentService::new_authorized_for_test(storage.clone());
    let mut conversation = meta();
    service
        .save_conversation_meta_checked(conversation.clone())
        .unwrap();
    let created = service.workflow_readiness_generation_for_test();
    conversation.title = "Renamed".into();
    conversation.unread_at = Some(2);
    conversation.updated_at = 2;
    service
        .save_conversation_meta_checked(conversation.clone())
        .unwrap();
    assert_eq!(service.workflow_readiness_generation_for_test(), created);
    conversation.model_id = Some("new-model".into());
    conversation.updated_at = 3;
    service
        .save_conversation_meta_checked(conversation.clone())
        .unwrap();
    let changed_model = service.workflow_readiness_generation_for_test();
    assert!(changed_model > created);
    conversation.archived_at = Some(3);
    conversation.updated_at = 4;
    service
        .save_conversation_meta_checked(conversation.clone())
        .unwrap();
    assert!(service.workflow_readiness_generation_for_test() > changed_model);
    assert_eq!(
        storage
            .load_conversation_meta("workflow-target")
            .unwrap()
            .unwrap()
            .archived_at,
        Some(3)
    );
    let archived = service.workflow_readiness_generation_for_test();
    conversation.model_id = Some("ignored-old-model".into());
    conversation.updated_at = 1;
    service
        .save_conversation_meta_checked(conversation)
        .unwrap();
    assert_eq!(service.workflow_readiness_generation_for_test(), archived);
}

#[test]
fn workflow_readiness_ui_permissions_ignore_cosmetic_preferences_and_reads() {
    let directory = tempfile::tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&directory.path().join("storage.sqlite")).unwrap());
    let service = AgentService::new_authorized_for_test(storage.clone());
    let mut preferences = storage.load_ui_preferences().unwrap();
    let initial = service.workflow_readiness_generation_for_test();
    preferences.profile_display_name = "Cosmetic change".into();
    let save = |preferences: &UiPreferencesRecord| {
        request(
            &storage,
            &service,
            STORAGE_SAVE_UI_PREFERENCES_METHOD,
            serde_json::to_value(preferences).unwrap(),
        )
    };
    assert!(save(&preferences)["result"].is_object());
    assert_eq!(service.workflow_readiness_generation_for_test(), initial);
    preferences.full_permission_enabled = !preferences.full_permission_enabled;
    assert!(save(&preferences)["result"].is_object());
    let changed = service.workflow_readiness_generation_for_test();
    assert!(changed > initial);
    assert!(request(
        &storage,
        &service,
        STORAGE_LOAD_UI_PREFERENCES_METHOD,
        json!({})
    )["result"]
        .is_object());
    assert_eq!(service.workflow_readiness_generation_for_test(), changed);
}
