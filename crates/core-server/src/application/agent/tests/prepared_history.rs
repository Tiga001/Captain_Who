use super::*;

const ID: &str = "prepared-history-server";

fn fixture() -> (tempfile::TempDir, Arc<StorageService>, AgentService) {
    let directory = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&directory.path().join("history.sqlite")).unwrap());
    storage.save_model_settings(test_model_settings()).unwrap();
    storage
        .save_conversation(ChatConversationRecord {
            id: ID.into(),
            project_id: None,
            model_id: Some("model-1".into()),
            title: "History".into(),
            messages: vec![
                ChatMessageRecord {
                    id: "old-user".into(),
                    role: "user".into(),
                    content: "Keep these requirements.".repeat(100),
                    created_at: 1000,
                    status: Some("sent".into()),
                    human_interaction_response: None,
                    attachments: vec![],
                    folder_references_json: None,
                    agent_run_json: None,
                    ui_state_json: None,
                },
                ChatMessageRecord {
                    id: "old-assistant".into(),
                    role: "assistant".into(),
                    content: "The requirements are recorded.".into(),
                    created_at: 2000,
                    status: Some("sent".into()),
                    human_interaction_response: None,
                    attachments: vec![],
                    folder_references_json: None,
                    agent_run_json: None,
                    ui_state_json: None,
                },
            ],
            created_at: 1000,
            updated_at: 2000,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
    storage
        .finalize_chat_message_with_conversation_trace(
            ID,
            "old-assistant",
            "The requirements are recorded.",
            Some("sent"),
            "completed",
            &completed_conversation_trace_without_items("old-run", ID, "old-assistant"),
            2000,
            2001,
        )
        .unwrap();
    let service = AgentService::new_authorized_for_test(storage.clone());
    (directory, storage, service)
}
fn input() -> AgentConversationTurnInput {
    AgentConversationTurnInput {
        conversation_id: Some(ID.into()),
        project_id: None,
        model_id: "model-1".into(),
        context_window_indicator_enabled: true,
        content: "Next request".into(),
        attachments: vec![],
        folder_references: vec![],
        skills: vec![],
        title: None,
        user_message_id: Some("new-user".into()),
        assistant_message_id: Some("new-assistant".into()),
        max_tokens: None,
        temperature: None,
        prompt_preferences: None,
        permissions: AgentPermissions::default(),
    }
}
fn warm_preview(service: &AgentService) -> AgentContextWindowSnapshot {
    service
        .get_context_window_snapshot(AgentContextWindowSnapshotInput {
            conversation_id: Some(ID.into()),
            project_id: None,
            model_id: "model-1".into(),
            max_tokens: None,
            prompt_preferences: None,
            permissions: AgentPermissions::default(),
            skills: vec![],
        })
        .unwrap()
        .snapshot
        .unwrap()
}

#[test]
fn history_warm_cache_reuses_projection_and_detects_message_change_without_ui_changes() {
    let (_directory, storage, service) = fixture();
    let before = storage.load_conversation(ID).unwrap();
    let cold = service.prepare_cached_history(ID).unwrap().unwrap();
    let warm = service.prepare_cached_history(ID).unwrap().unwrap();
    assert!(Arc::ptr_eq(&cold, &warm));
    assert_eq!(
        serde_json::to_value(before.as_ref()).unwrap(),
        serde_json::to_value(storage.load_conversation(ID).unwrap()).unwrap()
    );
    warm_preview(&service);
    assert!(service
        .conversation_context_states
        .lock()
        .unwrap()
        .contains_key(ID));
    let mut changed = before.unwrap();
    changed.messages[0].content = "Updated authoritative requirement".into();
    storage.save_conversation(changed).unwrap();
    let next = service.prepare_cached_history(ID).unwrap().unwrap();
    assert!(!Arc::ptr_eq(&cold, &next));
    assert!(next
        .messages
        .iter()
        .any(|m| m.content == "Updated authoritative requirement"));
    // Preparation may leave a previous terminal preview in memory, but the preview boundary
    // must validate its history identity before serving it again.
    let refreshed = warm_preview(&service);
    assert_eq!(
        service
            .conversation_context_states
            .lock()
            .unwrap()
            .get(ID)
            .unwrap()
            .history_version
            .as_ref(),
        Some(&next.source.version)
    );
    let cold_service = AgentService::new_authorized_for_test(storage.clone());
    assert_eq!(
        serde_json::to_value(refreshed).unwrap(),
        serde_json::to_value(warm_preview(&cold_service)).unwrap(),
        "a preview after an authoritative rewrite matches a freshly reconstructed preview"
    );
}

#[test]
fn history_warm_admission_preserves_cold_history_and_reuses_measured_prefix() {
    let (_directory, storage, service) = fixture();
    warm_preview(&service);
    let history = service.prepare_cached_history(ID).unwrap().unwrap();
    let expected = conversation_history_messages_with_model_context(
        &history.source.conversation.to_record(),
        &history.source.traces,
        &history.source.model_context_logs,
        history.source.compaction_summary.as_ref(),
        &[],
    )
    .unwrap();
    assert_eq!(
        serde_json::to_value(history.messages.as_ref()).unwrap(),
        serde_json::to_value(expected).unwrap()
    );
    let prepared = prepare_reserved_human_turn_with_history(
        &storage,
        &service.skills,
        input(),
        "run-warm-server",
        Some(history.source.conversation.to_record()),
        Some(history.source.conversation_revision),
        None,
        None,
        &|| Ok(()),
        Some(history),
    )
    .unwrap();
    assert!(prepared.prepared_history.is_some());
    assert_eq!(
        prepared
            .agent_input
            .messages
            .iter()
            .filter(|m| m.message_id.as_deref() == Some("new-user"))
            .count(),
        1
    );
    assert!(service.install_prepared_history(&prepared).unwrap());
    let mut cold = create_conversation_context_state(prepared.agent_input.clone()).unwrap();
    let mut states = service.conversation_context_states.lock().unwrap();
    let warm = states.get_mut(ID).unwrap();
    assert_eq!(warm.active_run_id.as_deref(), Some("run-warm-server"));
    assert_eq!(
        serde_json::to_value(warm.state.snapshot()).unwrap(),
        serde_json::to_value(cold.snapshot()).unwrap()
    );
}

#[test]
fn history_warm_baseline_does_not_survive_a_changed_model_budget() {
    let (_directory, storage, service) = fixture();
    warm_preview(&service);
    let history = service.prepare_cached_history(ID).unwrap().unwrap();
    let mut request = input();
    request.max_tokens = Some(1024);
    let prepared = prepare_reserved_human_turn_with_history(
        &storage,
        &service.skills,
        request,
        "run-warm-new-budget",
        Some(history.source.conversation.to_record()),
        Some(history.source.conversation_revision),
        None,
        None,
        &|| Ok(()),
        Some(history),
    )
    .unwrap();
    assert!(!service.install_prepared_history(&prepared).unwrap());
}

#[test]
fn history_warm_same_version_republish_preserves_measured_baseline() {
    let (_directory, storage, service) = fixture();
    let before_measurement = service.prepare_cached_history(ID).unwrap().unwrap();
    warm_preview(&service);
    assert!(service.has_measured_prepared_history_for_test(ID));
    // A slow earlier read publishes after another caller has finished measuring this version.
    assert!(service.republish_prepared_history_for_test(before_measurement.clone()));
    assert!(service.has_measured_prepared_history_for_test(ID));
    let prepared = prepare_reserved_human_turn_with_history(
        &storage,
        &service.skills,
        input(),
        "same-version-late-publish",
        Some(before_measurement.source.conversation.to_record()),
        Some(before_measurement.source.conversation_revision),
        None,
        None,
        &|| Ok(()),
        Some(before_measurement),
    )
    .unwrap();
    assert!(service.install_prepared_history(&prepared).unwrap());
}

#[test]
fn history_warm_cold_concurrent_readers_share_one_assembly() {
    let (_directory, _storage, service) = fixture();
    let before = service.prepared_history_diagnostics_for_test(ID);
    let gate = Arc::new(std::sync::Barrier::new(8));
    let (send, receive) = std::sync::mpsc::channel();
    for _ in 0..8 {
        let service = service.clone();
        let gate = gate.clone();
        let send = send.clone();
        std::thread::spawn(move || {
            gate.wait();
            send.send(service.prepare_cached_history(ID)).unwrap();
        });
    }
    drop(send);
    let histories = (0..8)
        .map(|_| {
            receive
                .recv_timeout(Duration::from_secs(10))
                .expect("concurrent preparation must release every waiter")
                .unwrap()
                .unwrap()
        })
        .collect::<Vec<_>>();
    assert!(histories
        .iter()
        .all(|entry| Arc::ptr_eq(entry, &histories[0])));
    assert_eq!(
        service
            .prepared_history_diagnostics_for_test(ID)
            .history_assemblies,
        before.history_assemblies + 1,
        "concurrent cold readers share the validated assembly"
    );
}

#[test]
fn history_warm_authoritative_change_cannot_reuse_old_measured_prefix() {
    let (_directory, storage, service) = fixture();
    warm_preview(&service);
    let stale = service.prepare_cached_history(ID).unwrap().unwrap();
    let mut changed = storage.load_conversation(ID).unwrap().unwrap();
    changed.messages[0].content = "New authoritative request after prewarming".into();
    storage.save_conversation(changed).unwrap();
    let current = service.prepare_cached_history(ID).unwrap().unwrap();
    assert_ne!(current.source.version, stale.source.version);
    assert!(!service.has_measured_prepared_history_for_test(ID));
    let prepared = prepare_reserved_human_turn_with_history(
        &storage,
        &service.skills,
        input(),
        "changed-history-falls-back",
        Some(current.source.conversation.to_record()),
        Some(current.source.conversation_revision),
        None,
        None,
        &|| Ok(()),
        Some(current),
    )
    .unwrap();
    assert!(!service.install_prepared_history(&prepared).unwrap());
    assert!(prepared
        .agent_input
        .messages
        .iter()
        .any(|message| { message.content == "New authoritative request after prewarming" }));
}

fn assert_terminal_update_rejects_stale_measurement(change_user_prefix: bool) {
    let (_directory, storage, service) = fixture();
    warm_preview(&service);
    let mut input: AgentChatInput = serde_json::from_value(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "test", "model": "model-1", "messages": [],
        "modelCapabilities": { "imageInput": false },
        "contextWindowTokens": 128000
    }))
    .unwrap();
    input.provider_protocol_key = Some(
        ProviderProtocolKey::new(
            ProviderProtocolDialect::OpenAiChatCompletions,
            &test_model_settings().models[0].provider_profile_config,
            "model-1",
            None,
        )
        .unwrap(),
    );
    let persisted = service
        .persisted_conversation_context_state(&input, ID)
        .unwrap();
    let old_input = persisted.preview_input;
    let old_history = persisted.prepared_history.unwrap();
    let old_trace = persisted.full_traces.last().unwrap().clone();
    let old_items = persisted
        .full_model_context_logs
        .iter()
        .find(|log| log.assistant_message_id == old_trace.assistant_message_id)
        .map(|log| log.items.clone())
        .unwrap_or_default();
    let old_assistant = old_history
        .source
        .conversation
        .messages
        .iter()
        .find(|message| message.id == old_trace.assistant_message_id)
        .unwrap()
        .clone();
    assert!(
        service
            .remember_terminal_measured_history(
                &old_input,
                ID,
                &old_trace,
                &old_items,
                &old_assistant.content,
                old_assistant.created_at,
            )
            .unwrap(),
        "the unchanged terminal source is eligible for publication"
    );

    let mut changed = old_history.source.conversation.to_record();
    let target = if change_user_prefix {
        "old-user"
    } else {
        "old-assistant"
    };
    changed
        .messages
        .iter_mut()
        .find(|message| message.id == target)
        .unwrap()
        .content = "Authoritative content edited while terminal publication was waiting".into();
    storage.save_conversation(changed).unwrap();
    assert!(
        !service
            .remember_terminal_measured_history(
                &old_input,
                ID,
                &old_trace,
                &old_items,
                &old_assistant.content,
                old_assistant.created_at,
            )
            .unwrap(),
        "an old terminal state must not certify newly changed persisted history"
    );
    let current = service.prepare_cached_history(ID).unwrap().unwrap();
    assert_ne!(current.source.version, old_history.source.version);
    assert!(!service.has_measured_prepared_history_for_test(ID));
    assert_eq!(
        service
            .conversation_context_states
            .lock()
            .unwrap()
            .get(ID)
            .unwrap()
            .history_version
            .as_ref(),
        Some(&old_history.source.version),
        "the rejected publication must not stamp the new history version on the old state"
    );
}

#[test]
fn history_warm_terminal_publication_rejects_changed_user_prefix_with_same_assistant_id() {
    assert_terminal_update_rejects_stale_measurement(true);
}

#[test]
fn history_warm_terminal_publication_rejects_changed_assistant_content_with_same_id() {
    assert_terminal_update_rejects_stale_measurement(false);
}
