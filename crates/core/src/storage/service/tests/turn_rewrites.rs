use super::*;
use crate::storage::conversation_turn_rewrite_repository::ConversationTurnRewriteAdmission;

fn completed_source_turn(
    service: &StorageService,
    conversation_id: &str,
) -> (ChatConversationRecord, Option<i64>) {
    service
        .save_conversation(conversation(
            conversation_id,
            Some("project-1"),
            "source-user",
        ))
        .unwrap();
    service
        .ensure_root_agent(&EnsureRootAgentInput {
            agent_id: "rewrite-root".to_string(),
            conversation_id: conversation_id.to_string(),
            creation_request_id: "rewrite-root-request".to_string(),
            task_name: "Rewrite root".to_string(),
        })
        .unwrap();
    let (mut candidate, revision) = service.load_conversation_for_turn(conversation_id).unwrap();
    let mut candidate = candidate.take().unwrap();
    candidate.updated_at = 3;
    candidate.messages.push(ChatMessageRecord {
        id: "source-assistant".to_string(),
        role: "assistant".to_string(),
        content: "Thinking...".to_string(),
        created_at: 2,
        status: Some("pending".to_string()),
        attachments: Vec::new(),
        agent_run_json: None,
        ui_state_json: None,
    });
    let trace = crate::ConversationTraceSnapshot::default().in_progress_trace(
        "source-run",
        conversation_id,
        "source-assistant",
    );
    service
        .save_conversation_and_begin_turn(
            candidate,
            revision,
            None,
            crate::AgentTurnPermissionSource::HostAuthenticatedRoot(
                crate::AgentPermissions::default(),
            ),
            &trace,
            2,
            2,
        )
        .unwrap();
    let completed = crate::completed_conversation_trace_without_items(
        "source-run",
        conversation_id,
        "source-assistant",
    );
    service
        .finalize_chat_message_with_conversation_trace(
            conversation_id,
            "source-assistant",
            "old answer only",
            Some("sent"),
            "completed",
            &completed,
            2,
            3,
        )
        .unwrap();
    let (conversation, revision) = service.load_conversation_for_turn(conversation_id).unwrap();
    (conversation.unwrap(), revision)
}

fn rewrite_admission(conversation_id: &str) -> ConversationTurnRewriteAdmission {
    ConversationTurnRewriteAdmission {
        request_id: "rewrite-request-1".to_string(),
        request_fingerprint: format!("sha256:{}", "a".repeat(64)),
        conversation_id: conversation_id.to_string(),
        source_user_message_id: "source-user".to_string(),
        source_assistant_message_id: "source-assistant".to_string(),
        replacement_user_message_id: "replacement-user".to_string(),
        replacement_assistant_message_id: "replacement-assistant".to_string(),
        run_id: "replacement-run".to_string(),
        response_json: serde_json::json!({
            "runId": "replacement-run",
            "conversationId": conversation_id,
            "userMessageId": "replacement-user",
            "assistantMessageId": "replacement-assistant"
        })
        .to_string(),
        created_at: 5,
    }
}

fn no_rewrite_attachments(
    service: &StorageService,
    conversation_id: &str,
) -> PreparedConversationTurnRewriteAttachments {
    service
        .prepare_conversation_turn_rewrite_attachments(
            conversation_id,
            "replacement-user",
            Some("project-1"),
            &[],
            4,
        )
        .unwrap()
}

fn replacement_candidate(
    service: &StorageService,
    conversation_id: &str,
) -> (
    ChatConversationRecord,
    Option<i64>,
    crate::ConversationTurnTrace,
) {
    let (mut conversation, revision) = service.load_conversation_for_turn(conversation_id).unwrap();
    let mut conversation = conversation.take().unwrap();
    conversation.updated_at = 5;
    conversation.messages.extend([
        ChatMessageRecord {
            id: "replacement-user".to_string(),
            role: "user".to_string(),
            content: "replacement".to_string(),
            created_at: 4,
            status: Some("sent".to_string()),
            attachments: Vec::new(),
            agent_run_json: None,
            ui_state_json: None,
        },
        ChatMessageRecord {
            id: "replacement-assistant".to_string(),
            role: "assistant".to_string(),
            content: "Thinking...".to_string(),
            created_at: 5,
            status: Some("pending".to_string()),
            attachments: Vec::new(),
            agent_run_json: None,
            ui_state_json: None,
        },
    ]);
    let trace = crate::ConversationTraceSnapshot::default().in_progress_trace(
        "replacement-run",
        conversation_id,
        "replacement-assistant",
    );
    (conversation, revision, trace)
}

#[test]
fn rewrite_is_atomic_idempotent_and_keeps_source_receipts_as_raw_facts() {
    let fixture = tempfile::tempdir().unwrap();
    let service = StorageService::open(&fixture.path().join("storage.sqlite")).unwrap();
    service
        .save_project(ProjectRecord {
            id: "project-1".to_string(),
            name: "project-1".to_string(),
            path: Some(fixture.path().to_string_lossy().into_owned()),
            created_at: 1,
            pinned_at: None,
        })
        .unwrap();
    let conversation_id = "conversation-rewrite-atomic";
    let (candidate, revision) = completed_source_turn(&service, conversation_id);
    let initial_world_state = crate::WorldStateSnapshot::new(
        "rewrite-world-state-epoch",
        0,
        vec![crate::WorldStateSectionEnvelope::model_visible(
            crate::WorldStateSectionId::EffectivePermissions,
            crate::WorldStateLifetime::Conversation,
            serde_json::json!({ "value": "old" }),
            serde_json::json!({ "value": "old" }),
        )
        .unwrap()],
    )
    .unwrap();
    let current_world_state = crate::WorldStateSnapshot::new(
        "rewrite-world-state-epoch",
        1,
        vec![crate::WorldStateSectionEnvelope::model_visible(
            crate::WorldStateSectionId::EffectivePermissions,
            crate::WorldStateLifetime::Conversation,
            serde_json::json!({ "value": "current" }),
            serde_json::json!({ "value": "current" }),
        )
        .unwrap()],
    )
    .unwrap();
    let world_state_diff =
        crate::WorldStateDiff::between(&initial_world_state, &current_world_state).unwrap();
    service
        .append_conversation_world_state_record(
            conversation_id,
            1,
            None,
            None,
            &crate::WorldStateRecord::Full(initial_world_state),
            1,
        )
        .unwrap();
    service
        .append_conversation_world_state_record(
            conversation_id,
            1,
            None,
            Some("source-user"),
            &crate::WorldStateRecord::Diff(world_state_diff),
            2,
        )
        .unwrap();
    let mut candidate = candidate;
    candidate.updated_at = 5;
    candidate.messages.extend([
        ChatMessageRecord {
            id: "replacement-user".to_string(),
            role: "user".to_string(),
            content: "new prompt only".to_string(),
            created_at: 4,
            status: Some("sent".to_string()),
            attachments: Vec::new(),
            agent_run_json: None,
            ui_state_json: None,
        },
        ChatMessageRecord {
            id: "replacement-assistant".to_string(),
            role: "assistant".to_string(),
            content: "Thinking...".to_string(),
            created_at: 5,
            status: Some("pending".to_string()),
            attachments: Vec::new(),
            agent_run_json: None,
            ui_state_json: None,
        },
    ]);
    let trace = crate::ConversationTraceSnapshot::default().in_progress_trace(
        "replacement-run",
        conversation_id,
        "replacement-assistant",
    );
    let admission = rewrite_admission(conversation_id);
    let prepared_attachments = no_rewrite_attachments(&service, conversation_id);
    let outcome = service
        .rewrite_conversation_turn_and_begin_turn(
            candidate.clone(),
            revision,
            crate::AgentTurnPermissionSource::HostAuthenticatedRoot(
                crate::AgentPermissions::default(),
            ),
            &[],
            &trace,
            5,
            5,
            &admission,
            &prepared_attachments,
        )
        .unwrap()
        .2;
    assert_eq!(outcome, ConversationTurnRewriteBeginOutcome::Started);

    let active = service.load_conversation(conversation_id).unwrap().unwrap();
    assert_eq!(
        active
            .messages
            .iter()
            .map(|message| message.id.as_str())
            .collect::<Vec<_>>(),
        vec!["replacement-user", "replacement-assistant"]
    );
    let active_traces = service
        .list_conversation_turn_traces(conversation_id)
        .unwrap();
    assert_eq!(active_traces.len(), 1);
    assert_eq!(
        active_traces[0].assistant_message_id,
        "replacement-assistant"
    );

    let connection = service.state.connection().unwrap();
    let counts = connection
        .query_row(
            "SELECT
                (SELECT COUNT(*) FROM messages
                 WHERE id IN ('source-user', 'source-assistant')),
                (SELECT COUNT(*) FROM conversation_turn_traces
                 WHERE assistant_message_id = 'source-assistant'),
                (SELECT COUNT(*) FROM agent_model_batch_receipts
                 WHERE assistant_message_id = 'source-assistant'),
                (SELECT COUNT(*) FROM conversation_turn_rewrites
                 WHERE request_id = 'rewrite-request-1')",
            [],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(counts, (2, 1, 1, 1));
    drop(connection);

    let active_world_state = service
        .list_active_conversation_world_state_records(conversation_id)
        .unwrap();
    assert_eq!(
        active_world_state[1].effective_before_message_id.as_deref(),
        Some("replacement-user")
    );
    let history_filter =
        crate::storage::conversation_history_repository::ConversationHistorySearchFilter {
            include_messages: true,
            ..Default::default()
        };
    assert!(service
        .search_conversation_history(conversation_id, "old answer only", &history_filter, 10)
        .unwrap()
        .is_empty());
    let hidden_source =
        crate::storage::conversation_history_repository::ConversationHistoryRecordRef::Message {
            message_id: "source-assistant".to_string(),
        };
    assert!(service
        .read_conversation_history_record(conversation_id, &hidden_source)
        .unwrap()
        .is_none());
    assert!(service
        .conversation_history_around(conversation_id, &hidden_source, 2, 2)
        .unwrap()
        .is_none());

    let replay = service
        .rewrite_conversation_turn_and_begin_turn(
            candidate,
            revision,
            crate::AgentTurnPermissionSource::HostAuthenticatedRoot(
                crate::AgentPermissions::default(),
            ),
            &[],
            &trace,
            5,
            5,
            &admission,
            &no_rewrite_attachments(&service, conversation_id),
        )
        .unwrap()
        .2;
    assert!(matches!(
        replay,
        ConversationTurnRewriteBeginOutcome::Replayed(ref record)
            if record.run_id == "replacement-run"
    ));

    let mut conflict = admission;
    conflict.request_fingerprint = format!("sha256:{}", "b".repeat(64));
    let error = service
        .rewrite_conversation_turn_and_begin_turn(
            service
                .load_conversation_for_turn(conversation_id)
                .unwrap()
                .0
                .unwrap(),
            service.conversation_revision(conversation_id).unwrap(),
            crate::AgentTurnPermissionSource::HostAuthenticatedRoot(
                crate::AgentPermissions::default(),
            ),
            &[],
            &trace,
            5,
            5,
            &conflict,
            &no_rewrite_attachments(&service, conversation_id),
        )
        .unwrap_err();
    assert!(error.contains("edit_turn_request_conflict"));

    let completed = crate::completed_conversation_trace_without_items(
        "replacement-run",
        conversation_id,
        "replacement-assistant",
    );
    service
        .finalize_chat_message_with_conversation_trace(
            conversation_id,
            "replacement-assistant",
            "new answer only",
            Some("sent"),
            "completed",
            &completed,
            5,
            6,
        )
        .unwrap();
    let fork = service
        .fork_conversation_request_view(assistant_reply_fork_request(
            "fork-rewritten-active-tail",
            conversation_id,
            "replacement-assistant",
        ))
        .unwrap()
        .conversation;
    assert_eq!(
        fork.messages
            .iter()
            .map(|message| message.content.as_str())
            .collect::<Vec<_>>(),
        vec!["new prompt only", "new answer only"]
    );
}

#[test]
fn rewrite_rejects_a_non_latest_or_unsettled_source_without_hiding_history() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let conversation_id = "conversation-rewrite-reject";
    let (mut candidate, revision) = completed_source_turn(&service, conversation_id);
    candidate.messages.push(ChatMessageRecord {
        id: "later-user".to_string(),
        role: "user".to_string(),
        content: "later".to_string(),
        created_at: 4,
        status: Some("sent".to_string()),
        attachments: Vec::new(),
        agent_run_json: None,
        ui_state_json: None,
    });
    candidate.messages.push(ChatMessageRecord {
        id: "later-assistant".to_string(),
        role: "assistant".to_string(),
        content: "Thinking...".to_string(),
        created_at: 5,
        status: Some("pending".to_string()),
        attachments: Vec::new(),
        agent_run_json: None,
        ui_state_json: None,
    });
    let later_trace = crate::ConversationTraceSnapshot::default().in_progress_trace(
        "later-run",
        conversation_id,
        "later-assistant",
    );
    service
        .save_conversation_and_begin_turn(
            candidate,
            revision,
            None,
            crate::AgentTurnPermissionSource::HostAuthenticatedRoot(
                crate::AgentPermissions::default(),
            ),
            &later_trace,
            5,
            5,
        )
        .unwrap();
    let later_completed = crate::completed_conversation_trace_without_items(
        "later-run",
        conversation_id,
        "later-assistant",
    );
    service
        .finalize_chat_message_with_conversation_trace(
            conversation_id,
            "later-assistant",
            "later answer",
            Some("sent"),
            "completed",
            &later_completed,
            5,
            6,
        )
        .unwrap();
    let (mut candidate, revision) = service.load_conversation_for_turn(conversation_id).unwrap();
    let mut candidate = candidate.take().unwrap();
    candidate.messages.push(ChatMessageRecord {
        id: "replacement-user".to_string(),
        role: "user".to_string(),
        content: "new".to_string(),
        created_at: 7,
        status: Some("sent".to_string()),
        attachments: Vec::new(),
        agent_run_json: None,
        ui_state_json: None,
    });
    candidate.messages.push(ChatMessageRecord {
        id: "replacement-assistant".to_string(),
        role: "assistant".to_string(),
        content: "Thinking...".to_string(),
        created_at: 8,
        status: Some("pending".to_string()),
        attachments: Vec::new(),
        agent_run_json: None,
        ui_state_json: None,
    });
    let trace = crate::ConversationTraceSnapshot::default().in_progress_trace(
        "replacement-run",
        conversation_id,
        "replacement-assistant",
    );
    let error = service
        .rewrite_conversation_turn_and_begin_turn(
            candidate,
            revision,
            crate::AgentTurnPermissionSource::HostAuthenticatedRoot(
                crate::AgentPermissions::default(),
            ),
            &[],
            &trace,
            8,
            8,
            &rewrite_admission(conversation_id),
            &no_rewrite_attachments(&service, conversation_id),
        )
        .unwrap_err();
    assert!(error.contains("edit_turn_not_latest"), "{error}");
    assert!(service
        .get_conversation_turn_rewrite("rewrite-request-1")
        .unwrap()
        .is_none());
    let active = service.load_conversation(conversation_id).unwrap().unwrap();
    assert!(active
        .messages
        .iter()
        .any(|message| message.id == "source-user"));
    assert!(active
        .messages
        .iter()
        .any(|message| message.id == "source-assistant"));
}

#[test]
fn rewrite_rejects_a_source_that_owns_the_active_compaction_lineage() {
    let fixture = tempfile::tempdir().unwrap();
    let service = StorageService::open(&fixture.path().join("storage.sqlite")).unwrap();
    service
        .save_project(ProjectRecord {
            id: "project-1".to_string(),
            name: "project-1".to_string(),
            path: Some(fixture.path().to_string_lossy().into_owned()),
            created_at: 1,
            pinned_at: None,
        })
        .unwrap();
    let conversation_id = "conversation-rewrite-summary-owner";
    service
        .save_conversation(conversation(
            conversation_id,
            Some("project-1"),
            "earlier-user",
        ))
        .unwrap();
    service
        .ensure_root_agent(&EnsureRootAgentInput {
            agent_id: "rewrite-summary-root".to_string(),
            conversation_id: conversation_id.to_string(),
            creation_request_id: "rewrite-summary-root-request".to_string(),
            task_name: "Rewrite summary root".to_string(),
        })
        .unwrap();

    let (mut earlier, revision) = service.load_conversation_for_turn(conversation_id).unwrap();
    let mut earlier = earlier.take().unwrap();
    earlier.messages.push(ChatMessageRecord {
        id: "earlier-assistant".to_string(),
        role: "assistant".to_string(),
        content: "Thinking...".to_string(),
        created_at: 2,
        status: Some("pending".to_string()),
        attachments: Vec::new(),
        agent_run_json: None,
        ui_state_json: None,
    });
    let earlier_trace = crate::ConversationTraceSnapshot::default().in_progress_trace(
        "earlier-run",
        conversation_id,
        "earlier-assistant",
    );
    service
        .save_conversation_and_begin_turn(
            earlier,
            revision,
            None,
            crate::AgentTurnPermissionSource::HostAuthenticatedRoot(
                crate::AgentPermissions::default(),
            ),
            &earlier_trace,
            2,
            2,
        )
        .unwrap();
    let earlier_completed = crate::completed_conversation_trace_without_items(
        "earlier-run",
        conversation_id,
        "earlier-assistant",
    );
    service
        .finalize_chat_message_with_conversation_trace(
            conversation_id,
            "earlier-assistant",
            "earlier answer",
            Some("sent"),
            "completed",
            &earlier_completed,
            2,
            3,
        )
        .unwrap();

    let (mut source, revision) = service.load_conversation_for_turn(conversation_id).unwrap();
    let mut source = source.take().unwrap();
    source.messages.extend([
        ChatMessageRecord {
            id: "source-user".to_string(),
            role: "user".to_string(),
            content: "source".to_string(),
            created_at: 4,
            status: Some("sent".to_string()),
            attachments: Vec::new(),
            agent_run_json: None,
            ui_state_json: None,
        },
        ChatMessageRecord {
            id: "source-assistant".to_string(),
            role: "assistant".to_string(),
            content: "Thinking...".to_string(),
            created_at: 5,
            status: Some("pending".to_string()),
            attachments: Vec::new(),
            agent_run_json: None,
            ui_state_json: None,
        },
    ]);
    let source_trace = crate::ConversationTraceSnapshot::default().in_progress_trace(
        "source-run",
        conversation_id,
        "source-assistant",
    );
    service
        .save_conversation_and_begin_turn(
            source,
            revision,
            None,
            crate::AgentTurnPermissionSource::HostAuthenticatedRoot(
                crate::AgentPermissions::default(),
            ),
            &source_trace,
            5,
            5,
        )
        .unwrap();
    let source_completed = crate::completed_conversation_trace_without_items(
        "source-run",
        conversation_id,
        "source-assistant",
    );
    service
        .finalize_chat_message_with_conversation_trace(
            conversation_id,
            "source-assistant",
            "source answer",
            Some("sent"),
            "completed",
            &source_completed,
            5,
            6,
        )
        .unwrap();

    {
        let connection = service.state.connection().unwrap();
        connection
            .execute(
                "INSERT INTO context_compaction_summaries (
                     id, conversation_id, schema_version, source_revision,
                     previous_summary_id, covered_through_kind,
                     covered_through_message_id, covered_through_trace_sequence,
                     content, continuity_schema_version, continuity_json,
                     generation_kind, generation_model, source_input_tokens,
                     summary_input_tokens, continuity_input_tokens,
                     uncovered_tail_input_tokens, replacement_input_tokens, created_at
                 ) VALUES (
                     'summary-owned-by-source', ?1, 1, 'summary-source-revision',
                     NULL, 'message', 'earlier-assistant', NULL,
                     'summary before the editable tail', 1, '{}',
                     'test', NULL, 100, 20, 5, 10, 30, 6
                 )",
                [conversation_id],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO context_compaction_summary_lineage (
                     summary_id, conversation_id, introduced_by_assistant_message_id,
                     source_conversation_id, source_summary_id, created_at
                 ) VALUES (
                     'summary-owned-by-source', ?1, 'source-assistant', NULL, NULL, 6
                 )",
                [conversation_id],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO conversation_context_compaction_heads (
                     conversation_id, summary_id, revision, updated_at
                 ) VALUES (?1, 'summary-owned-by-source', 1, 6)",
                [conversation_id],
            )
            .unwrap();
    }

    let (candidate, revision, trace) = replacement_candidate(&service, conversation_id);
    let error = service
        .rewrite_conversation_turn_and_begin_turn(
            candidate,
            revision,
            crate::AgentTurnPermissionSource::HostAuthenticatedRoot(
                crate::AgentPermissions::default(),
            ),
            &[],
            &trace,
            8,
            8,
            &rewrite_admission(conversation_id),
            &no_rewrite_attachments(&service, conversation_id),
        )
        .unwrap_err();
    assert!(
        error.contains("source Turn owns an active context summary lineage"),
        "{error}"
    );
    assert!(service
        .get_conversation_turn_rewrite("rewrite-request-1")
        .unwrap()
        .is_none());
    let active = service.load_conversation(conversation_id).unwrap().unwrap();
    assert!(active
        .messages
        .iter()
        .any(|message| message.id == "source-user"));
    assert!(active
        .messages
        .iter()
        .any(|message| message.id == "source-assistant"));
}

#[test]
fn rewrite_rejects_an_active_command_session_without_hiding_the_source() {
    let fixture = tempfile::tempdir().unwrap();
    let service = StorageService::open(&fixture.path().join("storage.sqlite")).unwrap();
    service
        .save_project(ProjectRecord {
            id: "project-1".to_string(),
            name: "project-1".to_string(),
            path: Some(fixture.path().to_string_lossy().into_owned()),
            created_at: 1,
            pinned_at: None,
        })
        .unwrap();
    let conversation_id = "conversation-rewrite-command";
    completed_source_turn(&service, conversation_id);
    service
        .state
        .connection()
        .unwrap()
        .execute(
            "INSERT INTO agent_command_sessions (
                 session_id, schema_version, conversation_id, assistant_message_id,
                 origin_run_id, call_id, project_id, command_projection, cwd_projection,
                 command_digest, authorization_source, approval_provenance_json,
                 permission_provenance_json, status, started_at, ended_at, exit_code,
                 latest_sequence, model_read_sequence, transcript_truncated,
                 output_capture_truncated, archive_ref, terminal_reason,
                 created_at, updated_at, settled_at
             ) VALUES (
                 ?1, 1, ?2, 'source-assistant', 'source-run', 'command-call', 'project-1',
                 'printf test', '.', ?3, 'explicit_user', '{}', '{}', 'running',
                 4, NULL, NULL, 0, 0, 0, 0, NULL, NULL, 4, 4, NULL
             )",
            rusqlite::params![
                "cmd_0123456789abcdef0123456789abcdef",
                conversation_id,
                format!("sha256:{}", "c".repeat(64)),
            ],
        )
        .unwrap();
    let (candidate, revision, trace) = replacement_candidate(&service, conversation_id);
    let error = service
        .rewrite_conversation_turn_and_begin_turn(
            candidate,
            revision,
            crate::AgentTurnPermissionSource::HostAuthenticatedRoot(
                crate::AgentPermissions::default(),
            ),
            &[],
            &trace,
            5,
            5,
            &rewrite_admission(conversation_id),
            &no_rewrite_attachments(&service, conversation_id),
        )
        .unwrap_err();
    assert!(error.contains("edit_turn_command_session_busy"), "{error}");
    assert!(service
        .get_conversation_turn_rewrite("rewrite-request-1")
        .unwrap()
        .is_none());
}
