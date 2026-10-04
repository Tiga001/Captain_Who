use super::*;

fn new_turn_messages(user_id: &str, assistant_id: &str) -> Vec<ChatMessageRecord> {
    let mut user = conversation("unused", None, user_id).messages.remove(0);
    user.created_at = 10_001;
    user.content = "new request".into();
    user.folder_references_json = Some("[{\"id\":\"folder-new\",\"name\":\"New folder\"}]".into());
    let mut assistant = user.clone();
    assistant.id = assistant_id.into();
    assistant.role = "assistant".into();
    assistant.content = "Thinking...".into();
    assistant.status = Some("pending".into());
    assistant.folder_references_json = None;
    vec![user, assistant]
}

fn append_turn(
    service: &StorageService,
    mut candidate: ChatConversationRecord,
    revision: Option<i64>,
    messages: &[ChatMessageRecord],
    run_id: &str,
    check_access: Option<&dyn Fn() -> Result<(), String>>,
) -> Result<(ChatConversationRecord, crate::AgentPermissions), String> {
    candidate.updated_at = 10_001;
    candidate.messages.extend(messages.iter().cloned());
    let trace = crate::ConversationTraceSnapshot::default().in_progress_trace(
        run_id,
        &candidate.id,
        &messages.last().unwrap().id,
    );
    service.append_conversation_and_begin_turn_with_execution_access(
        candidate,
        revision,
        None,
        crate::AgentTurnPermissionSource::HostAuthenticatedRoot(crate::AgentPermissions::default()),
        &[],
        messages,
        &trace,
        10_001,
        10_001,
        check_access,
    )
}

#[test]
fn incremental_turn_admission_never_updates_history_and_has_constant_sql_work() {
    let mut statement_counts = Vec::new();
    for history_len in [100, 1_000] {
        let fixture = StorageFixture::new();
        let service = fixture.service();
        let id = "append-history";
        let mut base = conversation(id, None, "history-0");
        base.messages = (0..history_len)
            .map(|index| {
                let mut message = conversation(id, None, &format!("history-{index}"))
                    .messages
                    .remove(0);
                message.content = format!("history text {index}");
                message.ui_state_json = Some("{\"expanded\":true}".into());
                message.folder_references_json = Some("[{\"id\":\"folder-old\"}]".into());
                message
            })
            .collect();
        service.save_conversation(base).unwrap();
        service
            .save_input_attachments(
                id,
                "history-0",
                None,
                &[input_attachment(
                    &service,
                    "history-attachment",
                    AgentInputAttachmentKind::File,
                    "notes.txt",
                    Some("text/plain"),
                    b"preserve attachment",
                )],
                1,
            )
            .unwrap();
        let (candidate, revision) = service.load_conversation_for_turn(id).unwrap();
        let mut candidate = candidate.unwrap();
        let before = serde_json::to_value(&candidate.messages).unwrap();
        // Rendering may omit old rows. Admission must not treat that projection as deletion.
        candidate.messages.clear();
        {
            let connection = service.state.connection().unwrap();
            connection
                .execute_batch(
                    "CREATE TEMP TRIGGER reject_history_update BEFORE UPDATE ON messages
                 WHEN OLD.id LIKE 'history-%' BEGIN SELECT RAISE(ABORT, 'history updated'); END;
                 CREATE TEMP TRIGGER reject_history_delete BEFORE DELETE ON messages
                 WHEN OLD.id LIKE 'history-%' BEGIN SELECT RAISE(ABORT, 'history deleted'); END;",
                )
                .unwrap();
        }
        let messages = new_turn_messages("new-user", "new-assistant");
        let started = std::time::Instant::now();
        let (result, selects) = super::conversations::trace_storage_selects(&service, || {
            append_turn(&service, candidate, revision, &messages, "append-run", None)
        });
        result.unwrap();
        eprintln!(
            "incremental Turn with {history_len} historical messages: {:?}, {} SELECTs",
            started.elapsed(),
            selects.len()
        );
        statement_counts.push(selects.len());
        assert!(!selects
            .iter()
            .any(|sql| sql.contains("SELECT conversation_id, position FROM messages WHERE id")));
        let current = service.load_conversation(id).unwrap().unwrap();
        assert_eq!(current.messages.len(), history_len + 2);
        assert_eq!(
            serde_json::to_value(&current.messages[..history_len]).unwrap(),
            before
        );
        assert_eq!(
            current.messages[history_len].folder_references_json,
            messages[0].folder_references_json
        );
        assert_eq!(current.messages[0].attachments.len(), 1);
        let connection = service.state.connection().unwrap();
        let positions: Vec<i64> = connection.prepare(
            "SELECT position FROM messages WHERE id IN ('new-user','new-assistant') ORDER BY position"
        ).unwrap().query_map([], |row| row.get(0)).unwrap().collect::<rusqlite::Result<_>>().unwrap();
        assert_eq!(positions, vec![history_len as i64, history_len as i64 + 1]);
    }
    assert_eq!(statement_counts[0], statement_counts[1]);
}

#[test]
fn incremental_turn_admission_rolls_back_message_collision_and_metadata() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    service
        .save_conversation(conversation("append-collision", None, "existing-user"))
        .unwrap();
    service
        .save_conversation(conversation("other-conversation", None, "other-user"))
        .unwrap();
    for assistant_id in ["existing-user", "other-user"] {
        let (candidate, revision) = service
            .load_conversation_for_turn("append-collision")
            .unwrap();
        let mut candidate = candidate.unwrap();
        let before = serde_json::to_value(&candidate).unwrap();
        candidate.title = "must roll back".into();
        let messages = new_turn_messages("new-user", assistant_id);
        let error = append_turn(
            &service,
            candidate,
            revision,
            &messages,
            "collision-run",
            None,
        )
        .unwrap_err();
        assert!(error.contains("UNIQUE constraint failed"), "{error}");
        let (current, current_revision) = service
            .load_conversation_for_turn("append-collision")
            .unwrap();
        assert_eq!(serde_json::to_value(current.unwrap()).unwrap(), before);
        assert_eq!(current_revision, revision);
        assert!(service
            .list_conversation_turn_traces("append-collision")
            .unwrap()
            .is_empty());
    }
}

#[test]
fn incremental_turn_admission_rejects_stale_snapshot_and_second_active_turn() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    service
        .save_conversation(conversation("append-revision", None, "history-user"))
        .unwrap();
    let (candidate, revision) = service
        .load_conversation_for_turn("append-revision")
        .unwrap();
    let candidate = candidate.unwrap();
    let messages = new_turn_messages("first-user", "first-assistant");
    append_turn(
        &service,
        candidate.clone(),
        revision,
        &messages,
        "first-run",
        None,
    )
    .unwrap();
    let messages = new_turn_messages("second-user", "second-assistant");
    let error =
        append_turn(&service, candidate, revision, &messages, "second-run", None).unwrap_err();
    assert!(error.contains("Conversation changed"), "{error}");
    let (candidate, revision) = service
        .load_conversation_for_turn("append-revision")
        .unwrap();
    let error = append_turn(
        &service,
        candidate.unwrap(),
        revision,
        &messages,
        "second-run",
        None,
    )
    .unwrap_err();
    assert!(error.contains("active durable Turn"), "{error}");
    let current = service
        .load_conversation("append-revision")
        .unwrap()
        .unwrap();
    assert_eq!(current.messages.len(), 3);
    assert!(service
        .get_conversation_turn_trace("second-assistant")
        .unwrap()
        .is_none());
}

#[test]
fn incremental_turn_admission_preserves_authority_and_rolls_back_late_access_failure() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    service
        .save_conversation(conversation("append-access", None, "history-user"))
        .unwrap();
    bind_agent_root(&service, "append-agent", "append-access");
    let (candidate, revision) = service.load_conversation_for_turn("append-access").unwrap();
    let candidate = candidate.unwrap();
    let messages = new_turn_messages("new-user", "new-assistant");
    let checks = std::cell::Cell::new(0);
    let check_access = || {
        checks.set(checks.get() + 1);
        if checks.get() == 2 {
            Err("access expired before commit".into())
        } else {
            Ok(())
        }
    };
    let error = append_turn(
        &service,
        candidate.clone(),
        revision,
        &messages,
        "access-run",
        Some(&check_access),
    )
    .unwrap_err();
    assert_eq!(error, "access expired before commit");
    let (after, after_revision) = service.load_conversation_for_turn("append-access").unwrap();
    assert_eq!(
        serde_json::to_value(after.unwrap()).unwrap(),
        serde_json::to_value(&candidate).unwrap()
    );
    assert_eq!(revision, after_revision);
    assert!(service
        .get_conversation_turn_trace("new-assistant")
        .unwrap()
        .is_none());
    append_turn(&service, candidate, revision, &messages, "access-run", None).unwrap();
    let authority = service
        .get_agent_effective_permission_snapshot("append-agent")
        .unwrap()
        .unwrap();
    assert_eq!(authority.permissions, crate::AgentPermissions::default());
    assert_eq!(authority.source_run_id, "access-run");
    assert!(service
        .load_agent_context_profile_for_run("access-run")
        .unwrap()
        .is_some());
}

#[test]
fn incremental_turn_admission_supports_first_turn_and_assistant_only_continuation() {
    for assistant_only in [false, true] {
        let fixture = StorageFixture::new();
        let service = fixture.service();
        let mut candidate = conversation("append-new", None, "unused");
        candidate.messages.clear();
        let mut messages = new_turn_messages("new-user", "new-assistant");
        let revision = if assistant_only {
            let base = conversation("append-new", None, "existing-user");
            service.save_conversation(base).unwrap();
            messages.remove(0);
            service.load_conversation_for_turn("append-new").unwrap().1
        } else {
            None
        };
        append_turn(&service, candidate, revision, &messages, "new-run", None).unwrap();
        let current = service.load_conversation("append-new").unwrap().unwrap();
        assert_eq!(current.messages.len(), 2);
        assert_eq!(current.messages[1].id, "new-assistant");
        assert_eq!(
            current.messages[0].id,
            if assistant_only {
                "existing-user"
            } else {
                "new-user"
            }
        );
    }
}

#[test]
fn incremental_turn_admission_rejects_mismatched_trace_and_message_shapes() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    for mismatch in ["assistant", "conversation", "extra-user", "assistant-first"] {
        let mut candidate = conversation("append-invalid", None, "unused");
        candidate.messages.clear();
        let mut messages = new_turn_messages("new-user", "new-assistant");
        let mut trace = crate::ConversationTraceSnapshot::default().in_progress_trace(
            "invalid-run",
            "append-invalid",
            "new-assistant",
        );
        match mismatch {
            "assistant" => trace.assistant_message_id = "different-assistant".into(),
            "conversation" => trace.conversation_id = "different-conversation".into(),
            "extra-user" => messages.insert(0, messages[0].clone()),
            "assistant-first" => messages.reverse(),
            _ => unreachable!(),
        }
        let error = service
            .append_conversation_and_begin_turn_with_execution_access(
                candidate,
                None,
                None,
                crate::AgentTurnPermissionSource::HostAuthenticatedRoot(
                    crate::AgentPermissions::default(),
                ),
                &[],
                &messages,
                &trace,
                10_001,
                10_001,
                None,
            )
            .unwrap_err();
        assert!(error.contains("do not match"), "{error}");
        assert!(service
            .load_conversation("append-invalid")
            .unwrap()
            .is_none());
    }
}
