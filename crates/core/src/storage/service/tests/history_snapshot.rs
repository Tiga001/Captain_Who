use super::*;

fn seeded_history(service: &StorageService, id: &str) {
    let mut history = conversation(id, None, &format!("{id}-user"));
    let mut assistant = history.messages[0].clone();
    assistant.id = format!("{id}-assistant");
    assistant.role = "assistant".into();
    assistant.status = Some("completed".into());
    history.messages.push(assistant.clone());
    service.save_conversation(history).unwrap();
    let trace = ConversationTurnTrace {
        schema_version: crate::CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
        run_id: format!("{id}-run"),
        conversation_id: id.into(),
        assistant_message_id: assistant.id.clone(),
        terminal_status: crate::ConversationTurnTraceTerminalStatus::Completed,
        terminal_error: None,
        truncated: false,
        items: vec![ConversationTurnTraceItem::AssistantNarration {
            provider_turn_id: None,
            first_tool_call_id: None,
            sequence: 0,
            content: "stored narration".into(),
            truncated: false,
        }],
    };
    service
        .replace_conversation_turn_trace(&trace, 1, 2)
        .unwrap();
    let connection = service.state.connection().unwrap();
    conversation_model_context_repository::commit_items_in_connection(
        &connection,
        id,
        &assistant.id,
        &[ConversationModelContextItem {
            sequence: 0,
            ordinal: 0,
            role: "assistant".into(),
            content: "stored narration".into(),
            images: Vec::new(),
            tool_call_id: None,
            tool_calls: Vec::new(),
            is_error: false,
        }],
    )
    .unwrap();
}

#[test]
fn history_snapshot_hot_read_reuses_validated_raw_history_without_decompression() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    seeded_history(&service, "history");
    let cold = service
        .load_conversation_history_snapshot("history")
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::to_value(cold.conversation.to_record()).unwrap(),
        serde_json::to_value(
            service
                .load_conversation_for_turn("history")
                .unwrap()
                .0
                .unwrap()
        )
        .unwrap()
    );
    assert_eq!(
        serde_json::to_value(&cold.traces).unwrap(),
        serde_json::to_value(service.list_conversation_turn_traces("history").unwrap()).unwrap()
    );
    assert_eq!(
        serde_json::to_value(&cold.model_context_logs).unwrap(),
        serde_json::to_value(
            service
                .list_conversation_model_context_logs("history")
                .unwrap()
        )
        .unwrap()
    );
    crate::storage::trace_performance_metrics::start();
    let hot = service
        .load_conversation_history_snapshot("history")
        .unwrap()
        .unwrap();
    let metrics = crate::storage::trace_performance_metrics::finish();
    assert!(Arc::ptr_eq(&cold, &hot));
    assert_eq!(metrics.decompressions, 0);
    assert_eq!(metrics.context_loads, 0);
}

#[test]
fn history_snapshot_metadata_refresh_reuses_payload_and_preserves_admission_cas() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    seeded_history(&service, "history");
    let original = service
        .load_conversation_history_snapshot("history")
        .unwrap()
        .unwrap();
    let before = service.history_snapshot_diagnostics("history");
    let mut metadata = service.load_conversation_meta("history").unwrap().unwrap();
    for step in 0..4 {
        if step == 1 {
            metadata.title = "Renamed without changing history".into();
            metadata.updated_at += 10;
            metadata.unread_at = Some(metadata.updated_at);
            metadata.pinned_at = Some(metadata.updated_at);
        } else if step == 2 {
            metadata.unread_at = None;
        }
        service.save_conversation_meta(metadata.clone()).unwrap();
        let refreshed = service
            .load_conversation_history_snapshot("history")
            .unwrap()
            .unwrap();
        assert_eq!(original.version, refreshed.version);
        assert!(Arc::ptr_eq(&original.payload, &refreshed.payload));
        assert!(Arc::ptr_eq(
            &original.conversation.messages,
            &refreshed.conversation.messages
        ));
        assert_eq!(refreshed.conversation.title, metadata.title);
        assert_eq!(refreshed.conversation.unread_at, metadata.unread_at);
        assert_eq!(refreshed.conversation.pinned_at, metadata.pinned_at);
        let expected_bytes = serde_json::to_vec(&(
            refreshed.conversation.to_record(),
            &refreshed.traces,
            &refreshed.model_context_logs,
            &refreshed.compaction_summary,
        ))
        .unwrap()
        .len()
            * 2;
        assert_eq!(
            refreshed.estimated_bytes, expected_bytes,
            "metadata refresh must update cache accounting without scanning the payload"
        );
        assert_eq!(
            Some(refreshed.conversation_revision),
            service.conversation_revision("history").unwrap()
        );
    }
    let after = service.history_snapshot_diagnostics("history");
    assert_eq!(before.full_loads, after.full_loads);
    assert_eq!(before.trace_decodes, after.trace_decodes);
    assert_eq!(before.model_context_decodes, after.model_context_decodes);
    assert!(service
        .is_conversation_history_snapshot_current(&original.version)
        .unwrap());
    let current = service
        .load_conversation_history_snapshot("history")
        .unwrap()
        .unwrap();
    assert!(current.conversation_revision > original.conversation_revision);
    let mut candidate = current.conversation.to_record();
    let mut assistant = candidate.messages.last().unwrap().clone();
    assistant.id = "new-assistant".into();
    assistant.status = Some("pending".into());
    candidate.messages.push(assistant.clone());
    let trace = crate::ConversationTraceSnapshot::default().in_progress_trace(
        "new-run",
        "history",
        &assistant.id,
    );
    let admit = |revision| {
        service.append_conversation_and_begin_turn_with_history_version(
            candidate.clone(),
            Some(revision),
            None,
            crate::AgentTurnPermissionSource::HostAuthenticatedRoot(
                crate::AgentPermissions::default(),
            ),
            &[],
            std::slice::from_ref(&assistant),
            &trace,
            3,
            3,
            None,
            Some(&original.version),
        )
    };
    let error = admit(original.conversation_revision).unwrap_err();
    assert!(error.contains("Conversation changed"), "{error}");
    assert!(service
        .get_conversation_turn_trace("new-assistant")
        .unwrap()
        .is_none());
    admit(current.conversation_revision).unwrap();
    assert_eq!(
        service
            .load_conversation_meta("history")
            .unwrap()
            .unwrap()
            .title,
        metadata.title
    );
}

#[test]
fn history_snapshot_external_non_history_writes_do_not_invalidate() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    seeded_history(&service, "history");
    seeded_history(&service, "other");
    let original = service
        .load_conversation_history_snapshot("history")
        .unwrap()
        .unwrap();
    let external = rusqlite::Connection::open(fixture.root.join("storage.sqlite")).unwrap();
    external.execute("INSERT INTO mcp_builtin_capability_policies(schema_version,capability_id,user_allowed,policy_version,policy_revision) VALUES (1,'browser_automation',1,1,1)", []).unwrap();
    external
        .execute(
            "UPDATE messages SET content = 'Other conversation changed' WHERE id = 'other-user'",
            [],
        )
        .unwrap();
    external.execute("UPDATE conversations SET unread_at = 20, updated_at = updated_at + 1 WHERE id = 'history'", []).unwrap();
    let current = service
        .load_conversation_history_snapshot("history")
        .unwrap()
        .unwrap();
    assert_eq!(original.version, current.version);
    assert_eq!(current.conversation.unread_at, Some(20));
    assert!(current.conversation_revision > original.conversation_revision);
    assert!(Arc::ptr_eq(&original.payload, &current.payload));
    assert!(Arc::ptr_eq(
        &original.conversation.messages,
        &current.conversation.messages
    ));
    assert_eq!(
        service.history_snapshot_diagnostics("history").full_loads,
        1
    );
    external
        .execute(
            "UPDATE messages SET content = 'This conversation changed' WHERE id = 'history-user'",
            [],
        )
        .unwrap();
    assert!(!service
        .is_conversation_history_snapshot_current(&current.version)
        .unwrap());
    let changed = service
        .load_conversation_history_snapshot("history")
        .unwrap()
        .unwrap();
    assert_eq!(
        changed.conversation.messages[0].content,
        "This conversation changed"
    );
    assert!(!Arc::ptr_eq(&current.payload, &changed.payload));
}

#[test]
fn history_snapshot_message_identity_versions_real_changes_but_not_noop_updates() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    seeded_history(&service, "history");
    let mut previous = service
        .load_conversation_history_snapshot("history")
        .unwrap()
        .unwrap();
    service.state.connection().unwrap().execute("UPDATE messages SET content=content, status=status, folder_references_json=folder_references_json WHERE id='history-user'", []).unwrap();
    let unchanged = service
        .load_conversation_history_snapshot("history")
        .unwrap()
        .unwrap();
    assert!(Arc::ptr_eq(&previous, &unchanged));
    for sql in [
        "UPDATE messages SET content='Changed' WHERE id='history-user'",
        "UPDATE messages SET status='error' WHERE id='history-user'",
        "UPDATE messages SET created_at=created_at+1, position=position+2 WHERE id='history-user'",
        "INSERT INTO messages(id,conversation_id,role,content,status,created_at,position) VALUES('new-user','history','user','Added','sent',9,9)",
        "DELETE FROM messages WHERE id='new-user'",
    ] {
        service.state.connection().unwrap().execute(sql, []).unwrap();
        assert!(!service.is_conversation_history_snapshot_current(&previous.version).unwrap(), "{sql}");
        previous = service.load_conversation_history_snapshot("history").unwrap().unwrap();
    }
    service
        .state
        .connection()
        .unwrap()
        .execute(
            "DELETE FROM conversation_message_history_revisions WHERE conversation_id='history'",
            [],
        )
        .unwrap();
    assert!(service
        .load_conversation_history_snapshot("history")
        .unwrap_err()
        .contains("消息版本"));
}

#[test]
fn history_snapshot_trace_tamper_invalidates_without_conversation_revision_change() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    seeded_history(&service, "history");
    let old = service
        .load_conversation_history_snapshot("history")
        .unwrap()
        .unwrap();
    {
        let connection = service.state.connection().unwrap();
        connection.execute("UPDATE conversation_model_context_items SET content_hash = 'sha256:0000000000000000000000000000000000000000000000000000000000000000' WHERE assistant_message_id = 'history-assistant'", []).unwrap();
    }
    assert_eq!(
        service.conversation_revision("history").unwrap(),
        Some(old.conversation_revision)
    );
    assert!(!service
        .is_conversation_history_snapshot_current(&old.version)
        .unwrap());
    let error = service
        .load_conversation_history_snapshot("history")
        .unwrap_err();
    assert!(error.contains("hash validation"), "{error}");
}

#[test]
fn history_snapshot_other_conversation_write_does_not_evict_current_history() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    seeded_history(&service, "first");
    let old = service
        .load_conversation_history_snapshot("first")
        .unwrap()
        .unwrap();
    seeded_history(&service, "second");
    let new = service
        .load_conversation_history_snapshot("first")
        .unwrap()
        .unwrap();
    assert!(Arc::ptr_eq(&old, &new));
}

#[test]
fn history_snapshot_external_connection_change_and_folder_change_invalidate() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    seeded_history(&service, "history");
    let old = service
        .load_conversation_history_snapshot("history")
        .unwrap()
        .unwrap();
    let external = rusqlite::Connection::open(fixture.root.join("storage.sqlite")).unwrap();
    external
        .execute(
            "UPDATE messages SET folder_references_json = '[{\"id\":\"changed\"}]' WHERE id = 'history-user'",
            [],
        )
        .unwrap();
    assert!(!service
        .is_conversation_history_snapshot_current(&old.version)
        .unwrap());
    let next = service
        .load_conversation_history_snapshot("history")
        .unwrap()
        .unwrap();
    assert_eq!(
        next.conversation.messages[0]
            .folder_references_json
            .as_deref(),
        Some("[{\"id\":\"changed\"}]")
    );
    service
        .state
        .connection()
        .unwrap()
        .execute(
            "UPDATE messages SET folder_references_json = '[]' WHERE id = 'history-user'",
            [],
        )
        .unwrap();
    assert!(!service
        .is_conversation_history_snapshot_current(&next.version)
        .unwrap());
}

#[test]
fn history_snapshot_recreated_conversation_cannot_reuse_old_epoch() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    seeded_history(&service, "history");
    let old = service
        .load_conversation_history_snapshot("history")
        .unwrap()
        .unwrap();
    service.delete_conversation("history").unwrap();
    seeded_history(&service, "history");
    assert!(!service
        .is_conversation_history_snapshot_current(&old.version)
        .unwrap());
    let recreated = service
        .load_conversation_history_snapshot("history")
        .unwrap()
        .unwrap();
    assert_eq!(recreated.conversation_revision, old.conversation_revision);
    assert_ne!(recreated.version, old.version);
    assert!(!Arc::ptr_eq(&recreated.payload, &old.payload));
}

#[test]
fn history_snapshot_attachment_metadata_changes_remain_part_of_history_identity() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    seeded_history(&service, "history");
    let first = service
        .load_conversation_history_snapshot("history")
        .unwrap()
        .unwrap();
    let external = rusqlite::Connection::open(fixture.root.join("storage.sqlite")).unwrap();
    external.execute("INSERT INTO attachments(id,conversation_id,message_id,kind,original_name,size_bytes,storage_rel_path,created_at) VALUES('attachment','history','history-user','file','original.txt',3,'test/original.txt',3)", []).unwrap();
    assert!(!service
        .is_conversation_history_snapshot_current(&first.version)
        .unwrap());
    let attached = service
        .load_conversation_history_snapshot("history")
        .unwrap()
        .unwrap();
    assert_eq!(
        attached.conversation.messages[0].attachments[0].name,
        "original.txt"
    );
    external
        .execute(
            "UPDATE attachments SET original_name='renamed.txt' WHERE id='attachment'",
            [],
        )
        .unwrap();
    assert!(!service
        .is_conversation_history_snapshot_current(&attached.version)
        .unwrap());
    let renamed = service
        .load_conversation_history_snapshot("history")
        .unwrap()
        .unwrap();
    assert_eq!(
        renamed.conversation.messages[0].attachments[0].name,
        "renamed.txt"
    );
    external
        .execute("DELETE FROM attachments WHERE id='attachment'", [])
        .unwrap();
    assert!(!service
        .is_conversation_history_snapshot_current(&renamed.version)
        .unwrap());
    assert!(service
        .load_conversation_history_snapshot("history")
        .unwrap()
        .unwrap()
        .conversation
        .messages[0]
        .attachments
        .is_empty());
}

#[test]
fn history_snapshot_rechecks_added_human_response_provenance() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    seeded_history(&service, "history");
    service
        .state
        .connection()
        .unwrap()
        .execute(
            "UPDATE messages SET content='{}' WHERE id='history-user'",
            [],
        )
        .unwrap();
    let old = service
        .load_conversation_history_snapshot("history")
        .unwrap()
        .unwrap();
    // The row matches the message bytes, so the SQL identity trigger allows it. The normal
    // reader must still reject its malformed human-answer provenance rather than use a hit.
    let external = rusqlite::Connection::open(fixture.root.join("storage.sqlite")).unwrap();
    external.execute("INSERT INTO human_interaction_message_projections(message_id,request_id,response_id,content_json) VALUES('history-user','request','response','{}')", []).unwrap();
    assert!(!service
        .is_conversation_history_snapshot_current(&old.version)
        .unwrap());
    assert!(service
        .load_conversation_history_snapshot("history")
        .is_err());
}

#[test]
fn history_snapshot_running_turn_is_never_cached() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    seeded_history(&service, "history");
    service.state.connection().unwrap().execute("UPDATE conversation_turn_traces SET terminal_status = 'in_progress', completed_at = NULL WHERE assistant_message_id = 'history-assistant'", []).unwrap();
    let first = service
        .load_conversation_history_snapshot("history")
        .unwrap()
        .unwrap();
    let second = service
        .load_conversation_history_snapshot("history")
        .unwrap()
        .unwrap();
    assert!(!Arc::ptr_eq(&first, &second));
}

#[test]
fn history_snapshot_admission_rejects_trace_only_mutation_transactionally() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    seeded_history(&service, "history");
    let old = service
        .load_conversation_history_snapshot("history")
        .unwrap()
        .unwrap();
    service.state.connection().unwrap().execute("UPDATE conversation_turn_traces SET updated_at = updated_at + 1 WHERE assistant_message_id = 'history-assistant'", []).unwrap();
    let mut candidate = old.conversation.to_record();
    let mut assistant = candidate.messages.last().unwrap().clone();
    assistant.id = "new-assistant".into();
    assistant.status = Some("pending".into());
    candidate.messages.push(assistant.clone());
    let trace = crate::ConversationTraceSnapshot::default().in_progress_trace(
        "new-run",
        "history",
        &assistant.id,
    );
    let error = service
        .append_conversation_and_begin_turn_with_history_version(
            candidate,
            Some(old.conversation_revision),
            None,
            crate::AgentTurnPermissionSource::HostAuthenticatedRoot(
                crate::AgentPermissions::default(),
            ),
            &[],
            &[assistant],
            &trace,
            3,
            3,
            None,
            Some(&old.version),
        )
        .unwrap_err();
    assert!(error.contains("Conversation changed"), "{error}");
    assert!(service
        .get_conversation_turn_trace("new-assistant")
        .unwrap()
        .is_none());
    assert_eq!(
        service
            .load_conversation_for_turn("history")
            .unwrap()
            .0
            .unwrap()
            .messages
            .len(),
        2
    );
}

#[test]
fn history_snapshot_summary_content_and_invalid_prefix_are_revalidated() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    seeded_history(&service, "history");
    let prefix = service
        .prepare_context_compaction_prefix(
            "history",
            &ContextJournalCursor::message("history-user"),
        )
        .unwrap();
    service
        .commit_context_compaction_prefix(
            &prefix,
            ContextCompactionSummaryDraft {
                id: "history-summary".into(),
                source_revision: prefix.source_revision.clone(),
                content: "first summary".into(),
                continuity: crate::ContextContinuitySnapshot::from_prefix(&prefix).unwrap(),
                generation: crate::ContextCompactionGeneration::test(),
                source_input_tokens: 100,
                summary_input_tokens: 10,
                continuity_input_tokens: 20,
                uncovered_tail_input_tokens: 0,
                replacement_input_tokens: 30,
                created_at: 3,
            },
            "history-assistant",
        )
        .unwrap();
    let first = service
        .load_conversation_history_snapshot("history")
        .unwrap()
        .unwrap();
    assert_eq!(
        first.compaction_summary.as_ref().unwrap().content,
        "first summary"
    );
    service.state.connection().unwrap().execute("UPDATE context_compaction_summaries SET content = 'changed summary' WHERE id = 'history-summary'", []).unwrap();
    assert!(!service
        .is_conversation_history_snapshot_current(&first.version)
        .unwrap());
    let changed = service
        .load_conversation_history_snapshot("history")
        .unwrap()
        .unwrap();
    assert_eq!(
        changed.compaction_summary.as_ref().unwrap().content,
        "changed summary"
    );
    service
        .state
        .connection()
        .unwrap()
        .execute(
            "UPDATE messages SET content = 'changed user' WHERE id = 'history-user'",
            [],
        )
        .unwrap();
    let invalidated = service
        .load_conversation_history_snapshot("history")
        .unwrap()
        .unwrap();
    assert!(invalidated.compaction_summary.is_none());
    let count: i64 = service
        .state
        .connection()
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM context_compaction_summaries WHERE id = 'history-summary'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        count, 1,
        "background snapshot reads must never delete a stale summary"
    );
    conversation_context_adaptation_repository::insert_in_connection(
        &service.state.connection().unwrap(),
        &conversation_context_adaptation_repository::ConversationContextAdaptationRequirement {
            conversation_id: "history".into(),
            reason: conversation_context_adaptation_repository::FORK_RELEASED_PROVIDER_STATE_REASON
                .into(),
            source_conversation_id: "source".into(),
            source_message_id: "source-message".into(),
            created_at: 0,
            resolved_summary_id: Some("history-summary".into()),
            resolved_at: Some(3),
        },
    )
    .unwrap();
    assert!(!service
        .is_conversation_history_snapshot_current(&invalidated.version)
        .unwrap());
    assert!(service
        .load_conversation_history_snapshot("history")
        .unwrap_err()
        .contains("provider_context_boundary_required"));
}

#[test]
fn history_snapshot_version_cannot_cross_storage_instances() {
    let first_fixture = StorageFixture::new();
    let first = first_fixture.service();
    first
        .save_conversation(conversation("same-id", None, "same-user"))
        .unwrap();
    let snapshot = first
        .load_conversation_history_snapshot("same-id")
        .unwrap()
        .unwrap();
    let second_fixture = StorageFixture::new();
    let second = second_fixture.service();
    let mut other = conversation("same-id", None, "same-user");
    other.messages[0].content = "different authoritative content".into();
    second.save_conversation(other).unwrap();
    assert_eq!(
        first.conversation_revision("same-id").unwrap(),
        second.conversation_revision("same-id").unwrap()
    );
    assert!(!second
        .is_conversation_history_snapshot_current(&snapshot.version)
        .unwrap());
}

#[test]
fn history_snapshot_tracks_only_workflow_delivery_ids_already_in_history() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    seeded_history(&service, "history");
    let initial = service
        .load_conversation_history_snapshot("history")
        .unwrap()
        .unwrap();
    let mut input = serde_json::json!({
        "id": "mail-input", "instanceId": "organization", "nodeId": "receiver",
        "conversationId": "history", "executionVersion": "epoch", "status": "pending",
        "mailStatus": "pending", "runId": null, "deliveryId": null,
        "content": "organization message", "createdAt": 3, "error": null, "messages": []
    });
    service.state.connection().unwrap().execute(
        "INSERT INTO workflow_mail_inputs(input_id, instance_id, execution_version, node_id, conversation_id, input_json, status, created_at, updated_at) VALUES ('mail-input', 'organization', 'epoch', 'receiver', 'history', ?1, 'pending', 3, 3)",
        [input.to_string()],
    ).unwrap();
    assert!(service
        .is_conversation_history_snapshot_current(&initial.version)
        .unwrap());
    input["deliveryId"] = serde_json::json!("future-delivery");
    input["status"] = serde_json::json!("claimed");
    service.state.connection().unwrap().execute(
        "UPDATE workflow_mail_inputs SET input_json = ?1, status = 'claimed', delivery_id = 'future-delivery', run_id = 'future-run', updated_at = 4 WHERE input_id = 'mail-input'", [input.to_string()],
    ).unwrap();
    assert!(
        service
            .is_conversation_history_snapshot_current(&initial.version)
            .unwrap(),
        "pending claims for a future delivery cannot invalidate old history"
    );
    input["deliveryId"] = serde_json::json!("history-user");
    service.state.connection().unwrap().execute(
        "UPDATE workflow_mail_inputs SET input_json = ?1, delivery_id = 'history-user' WHERE input_id = 'mail-input'", [input.to_string()],
    ).unwrap();
    assert_eq!(
        service.conversation_revision("history").unwrap(),
        Some(initial.conversation_revision)
    );
    assert!(!service
        .is_conversation_history_snapshot_current(&initial.version)
        .unwrap());
    assert_eq!(
        service
            .workflow_execution_inputs_for_conversation("history")
            .unwrap()[0]
            .delivery_id
            .as_deref(),
        Some("history-user")
    );
    let with_delivery = service
        .load_conversation_history_snapshot("history")
        .unwrap()
        .unwrap();
    input["status"] = serde_json::json!("completed");
    service.state.connection().unwrap().execute(
        "UPDATE workflow_mail_inputs SET input_json = ?1, status = 'completed', updated_at = 5 WHERE input_id = 'mail-input'", [input.to_string()],
    ).unwrap();
    assert!(
        service
            .is_conversation_history_snapshot_current(&with_delivery.version)
            .unwrap(),
        "status changes do not change the historical delivery boundary"
    );
    input["deliveryId"] = serde_json::Value::Null;
    service.state.connection().unwrap().execute(
        "UPDATE workflow_mail_inputs SET input_json = ?1, delivery_id = NULL WHERE input_id = 'mail-input'", [input.to_string()],
    ).unwrap();
    assert!(!service
        .is_conversation_history_snapshot_current(&with_delivery.version)
        .unwrap());
}
