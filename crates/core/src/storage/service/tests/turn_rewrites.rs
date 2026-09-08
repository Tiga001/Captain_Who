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
        human_interaction_response: None,
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
            human_interaction_response: None,
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
            human_interaction_response: None,
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
    let source_continuation =
        crate::storage::provider_continuation_repository::ProviderContinuationEnvelopeRecord {
            continuation_id: format!(
                "{}{}",
                crate::storage::provider_continuation_repository::PROVIDER_CONTINUATION_REF_PREFIX,
                uuid::Uuid::new_v4().hyphenated()
            ),
            conversation_id: conversation_id.to_string(),
            assistant_message_id: "source-assistant".to_string(),
            run_id: "source-run".to_string(),
            request_index: 0,
            assistant_turn_id: format!("at1_{}", "a".repeat(64)),
            assistant_turn_digest: format!("sha256:{}", "b".repeat(64)),
            provider_protocol_digest: format!("sha256:{}", "c".repeat(64)),
            payload_digest: format!("sha256:{}", "d".repeat(64)),
            nonce: vec![1; 12],
            ciphertext: vec![2; 17],
            decoded_bytes: 1,
            compressed_bytes: 1,
            created_at: 4,
            runtime_tool_calls: Vec::new(),
        };
    {
        let connection = service.state.connection().unwrap();
        crate::storage::provider_continuation_repository::store_active_with_projection_in_connection(
            &connection,
            &source_continuation,
            crate::storage::provider_continuation_repository::ProviderContinuationProjection::ConversationMessage,
        )
        .unwrap();
    }
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
    let request_world_state =
        crate::WorldStateSnapshot::new("rewrite-world-state-epoch", 2, Vec::new()).unwrap();
    let request_boundary = crate::WorldStateRequestBoundary {
        run_id: "source-run".into(),
        assistant_message_id: "source-assistant".into(),
        request_index: 0,
        after_trace_sequence: None,
    };
    {
        let mut connection = service.state.connection().unwrap();
        crate::storage::world_state_repository::append_record(
            &mut connection,
            &crate::storage::world_state_repository::ConversationWorldStateRecordWrite {
                conversation_id,
                epoch_generation: 1,
                base_summary_id: None,
                effective_before_message_id: None,
                request_boundary: Some(&request_boundary),
                model_observed: false,
                record: &crate::WorldStateRecord::Diff(
                    crate::WorldStateDiff::between(&current_world_state, &request_world_state)
                        .unwrap(),
                ),
                created_at: 3,
            },
        )
        .unwrap();
        connection.execute("INSERT INTO conversation_world_state_request_commits
            (conversation_id,run_id,assistant_message_id,request_index,after_trace_sequence,payload_json,epoch_id,sequence,created_at)
            VALUES (?1,'source-run','source-assistant',0,NULL,'{}','rewrite-world-state-epoch',2,3)", [conversation_id]).unwrap();
    }
    let mut candidate = candidate;
    candidate.updated_at = 5;
    candidate.messages.extend([
        ChatMessageRecord {
            human_interaction_response: None,
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
            human_interaction_response: None,
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
    let released_continuation = connection
        .query_row(
            "SELECT state, payload_digest, ciphertext, released_at, activated_at
             FROM provider_continuations WHERE continuation_id = ?1",
            [&source_continuation.continuation_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<Vec<u8>>>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                    row.get::<_, Option<i64>>(4)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(
        released_continuation,
        ("released".to_string(), None, None, Some(5), None)
    );
    drop(connection);

    let active_world_state = service
        .list_active_conversation_world_state_records(conversation_id)
        .unwrap();
    assert_eq!(active_world_state.len(), 1);
    assert!(matches!(
        active_world_state[0].record,
        crate::WorldStateRecord::Full(_)
    ));
    assert_eq!(service.state.connection().unwrap().query_row(
        "SELECT COUNT(*) FROM conversation_world_state_request_commits WHERE conversation_id=?1",
        [conversation_id], |row| row.get::<_, i64>(0),
    ).unwrap(), 0);
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
        human_interaction_response: None,
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
        human_interaction_response: None,
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
        human_interaction_response: None,
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
        human_interaction_response: None,
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
        human_interaction_response: None,
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
            human_interaction_response: None,
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
            human_interaction_response: None,
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
                 ?1, ?4, ?2, 'source-assistant', 'source-run', 'command-call', 'project-1',
                 'printf test', '.', ?3, 'explicit_user', '{}', '{}', 'running',
                 4, NULL, NULL, 0, 0, 0, 0, NULL, NULL, 4, 4, NULL
             )",
            rusqlite::params![
                "cmd_0123456789abcdef0123456789abcdef",
                conversation_id,
                format!("sha256:{}", "c".repeat(64)),
                i64::from(
                    crate::storage::agent_command_session_repository::AGENT_COMMAND_SESSION_SCHEMA_VERSION,
                ),
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

fn complete_fork_evidence_turn(
    service: &StorageService,
    conversation_id: &str,
    name: &str,
    created_at: i64,
    rewrite_latest: bool,
) -> String {
    let (candidate, revision) = service.load_conversation_for_turn(conversation_id).unwrap();
    let mut candidate = candidate.unwrap();
    let assistant_id = format!("{name}-assistant");
    let user_id = format!("{name}-user");
    let run_id = format!("{name}-run");
    let source_assistant_id = candidate.messages.last().unwrap().id.clone();
    let source_user_id = candidate.messages[candidate.messages.len() - 2].id.clone();
    candidate.updated_at = created_at;
    for (id, role, content) in [
        (&user_id, "user", name),
        (&assistant_id, "assistant", "Thinking..."),
    ] {
        candidate.messages.push(ChatMessageRecord {
            human_interaction_response: None,
            id: id.clone(),
            role: role.to_string(),
            content: content.to_string(),
            created_at,
            status: Some(
                if role == "assistant" {
                    "pending"
                } else {
                    "sent"
                }
                .to_string(),
            ),
            attachments: Vec::new(),
            agent_run_json: None,
            ui_state_json: None,
        });
    }
    let trace = crate::ConversationTraceSnapshot::default().in_progress_trace(
        &run_id,
        conversation_id,
        &assistant_id,
    );
    let permission =
        crate::AgentTurnPermissionSource::HostAuthenticatedRoot(crate::AgentPermissions::default());
    if rewrite_latest {
        let admission = ConversationTurnRewriteAdmission {
            request_id: format!("{name}-rewrite"),
            request_fingerprint: format!("sha256:{}", "a".repeat(64)),
            conversation_id: conversation_id.to_string(),
            source_user_message_id: source_user_id,
            source_assistant_message_id: source_assistant_id,
            replacement_user_message_id: user_id.clone(),
            replacement_assistant_message_id: assistant_id.clone(),
            run_id: run_id.clone(),
            response_json: serde_json::json!({
                "runId": run_id,
                "conversationId": conversation_id,
                "userMessageId": user_id,
                "assistantMessageId": assistant_id
            })
            .to_string(),
            created_at,
        };
        let attachments = service
            .prepare_conversation_turn_rewrite_attachments(
                conversation_id,
                &user_id,
                Some("project-1"),
                &[],
                created_at,
            )
            .unwrap();
        service
            .rewrite_conversation_turn_and_begin_turn(
                candidate,
                revision,
                permission,
                &[],
                &trace,
                created_at,
                created_at,
                &admission,
                &attachments,
            )
            .unwrap();
    } else {
        service
            .save_conversation_and_begin_turn(
                candidate, revision, None, permission, &trace, created_at, created_at,
            )
            .unwrap();
    }
    service
        .finalize_chat_message_with_conversation_trace(
            conversation_id,
            &assistant_id,
            name,
            Some("sent"),
            "completed",
            &crate::completed_conversation_trace_without_items(
                &run_id,
                conversation_id,
                &assistant_id,
            ),
            created_at,
            created_at + 1,
        )
        .unwrap();
    assistant_id
}

fn record_fork_evidence(
    fixture: &StorageFixture,
    service: &StorageService,
    conversation_id: &str,
    message_id: &str,
    run_id: &str,
    file: Option<&str>,
) {
    let identity = crate::AgentTurnDiffIdentity {
        run_id: run_id.to_string(),
        conversation_id: conversation_id.to_string(),
        assistant_message_id: message_id.to_string(),
        project_id: "project-1".to_string(),
        workspace_root: fixture
            .root
            .join("project-1")
            .to_string_lossy()
            .into_owned(),
    };
    service.initialize_agent_turn_diff(&identity).unwrap();
    if let Some(file) = file {
        service
            .record_agent_turn_file_change(
                &identity,
                &format!("{run_id}-action"),
                &crate::AgentTurnFileChange {
                    path: file.to_string(),
                    before: crate::AgentTurnFileContent::Missing,
                    after: crate::AgentTurnFileContent::Text(format!("{run_id}\n")),
                },
            )
            .unwrap();
    }
}

#[test]
fn fork_after_rewrite_excludes_superseded_empty_turn_evidence() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let conversation_id = "fork-rewrite-empty-evidence";
    completed_source_turn(&service, conversation_id);
    record_fork_evidence(
        &fixture,
        &service,
        conversation_id,
        "source-assistant",
        "source-run",
        None,
    );
    let replacement = complete_fork_evidence_turn(&service, conversation_id, "edited", 5, true);
    record_fork_evidence(
        &fixture,
        &service,
        conversation_id,
        &replacement,
        "edited-run",
        None,
    );

    let fork = service
        .fork_conversation_request_view(assistant_reply_fork_request(
            "fork-empty-evidence",
            conversation_id,
            &replacement,
        ))
        .unwrap()
        .conversation;
    assert_eq!(fork.messages.len(), 2);
    let copied = service
        .load_agent_turn_diffs_for_messages(&fork.id, "project-1", &[fork.messages[1].id.clone()])
        .unwrap();
    assert_eq!(copied.len(), 1);
    assert!(copied[0].files.is_empty());
    assert_eq!(
        service
            .state
            .connection()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM agent_turn_diffs WHERE conversation_id=?1",
                [conversation_id],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        2,
        "fork must not delete the superseded source's durable evidence"
    );
}

#[test]
fn fork_after_repeated_rewrites_keeps_selected_file_evidence_and_can_fork_again() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let conversation_id = "fork-rewrite-file-evidence";
    completed_source_turn(&service, conversation_id);
    record_fork_evidence(
        &fixture,
        &service,
        conversation_id,
        "source-assistant",
        "source-run",
        Some("prefix.txt"),
    );
    for (name, time, rewrite) in [
        ("old", 5, false),
        ("edited-once", 7, true),
        ("edited-twice", 9, true),
    ] {
        let message_id =
            complete_fork_evidence_turn(&service, conversation_id, name, time, rewrite);
        record_fork_evidence(
            &fixture,
            &service,
            conversation_id,
            &message_id,
            &format!("{name}-run"),
            Some(&format!("{name}.txt")),
        );
    }
    let later = complete_fork_evidence_turn(&service, conversation_id, "later", 11, false);
    record_fork_evidence(
        &fixture,
        &service,
        conversation_id,
        &later,
        "later-run",
        Some("later.txt"),
    );

    for (boundary, expected_files) in [
        (
            "edited-twice-assistant",
            vec!["prefix.txt", "edited-twice.txt"],
        ),
        (
            later.as_str(),
            vec!["prefix.txt", "edited-twice.txt", "later.txt"],
        ),
    ] {
        let first = service
            .fork_conversation_request_view(assistant_reply_fork_request(
                format!("fork-{boundary}"),
                conversation_id,
                boundary,
            ))
            .unwrap()
            .conversation;
        let second = service
            .fork_conversation_request_view(assistant_reply_fork_request(
                format!("fork-again-{boundary}"),
                &first.id,
                &first.messages.last().unwrap().id,
            ))
            .unwrap()
            .conversation;
        for fork in [&first, &second] {
            let assistant_ids = fork
                .messages
                .iter()
                .filter(|message| message.role == "assistant")
                .map(|message| message.id.clone())
                .collect::<Vec<_>>();
            let copies = service
                .load_agent_turn_diffs_for_messages(&fork.id, "project-1", &assistant_ids)
                .unwrap();
            assert_eq!(copies.len(), expected_files.len());
            assert_eq!(
                copies
                    .iter()
                    .flat_map(|copy| copy.files.iter().map(|file| file.path.as_str()))
                    .collect::<Vec<_>>(),
                expected_files
            );
            for copy in &copies {
                assert!(assistant_ids.contains(&copy.identity.assistant_message_id));
                assert_ne!(copy.identity.conversation_id, conversation_id);
                assert_eq!(copy.files.len(), 1);
                assert_eq!(copy.files[0].before, crate::AgentTurnFileContent::Missing);
            }
            assert_eq!(service.state.connection().unwrap().query_row(
                "SELECT COUNT(*) FROM agent_turn_diff_actions AS action JOIN agent_turn_diffs AS turn ON turn.assistant_message_id=action.assistant_message_id WHERE turn.conversation_id=?1", [&fork.id],
                |row| row.get::<_, i64>(0),
            ).unwrap(), expected_files.len() as i64);
        }
    }
    assert_eq!(
        service
            .state
            .connection()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM agent_turn_diffs WHERE conversation_id=?1",
                [conversation_id],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        5
    );
}

#[test]
fn fork_still_rejects_selected_turn_evidence_with_an_unmapped_run() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let conversation_id = "fork-evidence-invalid-run";
    completed_source_turn(&service, conversation_id);
    record_fork_evidence(
        &fixture,
        &service,
        conversation_id,
        "source-assistant",
        "missing-run",
        None,
    );
    let error = service
        .fork_conversation_request_view(assistant_reply_fork_request(
            "fork-invalid-run",
            conversation_id,
            "source-assistant",
        ))
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("文件变更证据所属运行未包含在分叉快照中"),
        "{error}"
    );
}
