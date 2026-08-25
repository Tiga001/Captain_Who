use super::*;
use crate::storage::{migrations, provider_continuation_repository, world_state_repository};
use crate::{
    AgentApprovalStatus, AgentCommandSessionStatus, ContextCompactionReceiptStage,
    ConversationCommandSessionLifecycle, ConversationCommandSessionLifecyclePhase,
    ConversationTraceToolResultStatus, ConversationTurnTrace, ConversationTurnTraceItem,
    ConversationTurnTraceTerminalStatus, WorldStateDiff, WorldStateLifetime, WorldStateRecord,
    WorldStateSectionEnvelope, WorldStateSectionId, WorldStateSnapshot,
    CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
};

fn setup() -> Connection {
    let connection = Connection::open_in_memory().unwrap();
    migrations::run_migrations(&connection).unwrap();
    connection
        .execute(
            "INSERT INTO conversations (
                    id, project_id, model_id, title, created_at, updated_at,
                    pinned_at, archived_at, unread_at
                 ) VALUES ('conversation-1', NULL, NULL, 'Test', 1, 1, NULL, NULL, NULL)",
            [],
        )
        .unwrap();
    for (position, id, role, content, status) in [
        (0, "user-1", "user", "first request", "sent"),
        (1, "assistant-1", "assistant", "first answer", "sent"),
        (2, "user-2", "user", "current request", "sent"),
        (3, "assistant-2", "assistant", "", "pending"),
    ] {
        connection
            .execute(
                "INSERT INTO messages (
                        id, conversation_id, role, content, status, agent_run_json,
                        ui_state_json, created_at, position
                     ) VALUES (?1, 'conversation-1', ?2, ?3, ?4, NULL, NULL, ?5, ?5)",
                params![id, role, content, status, position],
            )
            .unwrap();
    }
    let trace = ConversationTurnTrace {
        schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
        run_id: "run-2".to_string(),
        conversation_id: "conversation-1".to_string(),
        assistant_message_id: "assistant-2".to_string(),
        terminal_status: ConversationTurnTraceTerminalStatus::InProgress,
        terminal_error: None,
        truncated: false,
        items: vec![
            ConversationTurnTraceItem::ToolCall {
                sequence: 0,
                call_id: "call-1".to_string(),
                tool: "web_fetch".to_string(),
                provenance: crate::AgentToolIdentity::Builtin {
                    tool_name: "web_fetch".to_string(),
                },
                operation: json!({ "url": "https://example.com" }),
                approval_status: AgentApprovalStatus::NotRequired,
                truncated: false,
            },
            ConversationTurnTraceItem::ToolResult {
                sequence: 1,
                call_id: "call-1".to_string(),
                tool: "web_fetch".to_string(),
                status: ConversationTraceToolResultStatus::Succeeded,
                success: true,
                observation: json!({ "content": "page body" }),
                approval_status: AgentApprovalStatus::NotRequired,
                error: None,
                truncated: false,
                archive: Default::default(),
            },
        ],
    };
    conversation_trace_repository::commit_trace_in_connection(&connection, &trace, 3, 4).unwrap();
    connection
}

fn draft(prefix: &ContextCompactionPrefix, id: &str) -> ContextCompactionSummaryDraft {
    ContextCompactionSummaryDraft {
        id: id.to_string(),
        source_revision: prefix.source_revision.clone(),
        content: format!("summary {id}"),
        continuity: crate::ContextContinuitySnapshot::from_prefix(prefix).unwrap(),
        generation: ContextCompactionGeneration::test(),
        source_input_tokens: 100,
        summary_input_tokens: 10,
        continuity_input_tokens: 20,
        uncovered_tail_input_tokens: 0,
        replacement_input_tokens: 30,
        created_at: 10,
    }
}

fn provider_continuation_record(
    assistant_message_id: &str,
    run_id: &str,
    request_index: u64,
    runtime_call_ids: &[&str],
) -> provider_continuation_repository::ProviderContinuationEnvelopeRecord {
    provider_continuation_repository::ProviderContinuationEnvelopeRecord {
        continuation_id: format!(
            "{}{}",
            provider_continuation_repository::PROVIDER_CONTINUATION_REF_PREFIX,
            uuid::Uuid::new_v4().hyphenated()
        ),
        conversation_id: "conversation-1".to_string(),
        assistant_message_id: assistant_message_id.to_string(),
        run_id: run_id.to_string(),
        request_index,
        assistant_turn_id: format!("at1_{}", "a".repeat(64)),
        assistant_turn_digest: format!("sha256:{}", "b".repeat(64)),
        provider_protocol_digest: format!("sha256:{}", "c".repeat(64)),
        payload_digest: format!("sha256:{}", "d".repeat(64)),
        nonce: vec![7; 12],
        ciphertext: vec![9; 17],
        decoded_bytes: 1,
        compressed_bytes: 1,
        created_at: 2,
        runtime_tool_calls: runtime_call_ids
            .iter()
            .enumerate()
            .map(|(provider_tool_index, runtime_call_id)| {
                provider_continuation_repository::ProviderContinuationRuntimeToolIdentity {
                    provider_tool_index,
                    runtime_call_id: (*runtime_call_id).to_string(),
                }
            })
            .collect(),
    }
}

fn provider_continuation_state(
    connection: &Connection,
    continuation_id: &str,
) -> (String, Option<Vec<u8>>) {
    connection
        .query_row(
            "SELECT state, ciphertext FROM provider_continuations WHERE continuation_id = ?1",
            [continuation_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap()
}

fn planned_receipt() -> ContextCompactionReceipt {
    ContextCompactionReceipt {
        schema_version: crate::CONTEXT_COMPACTION_RECEIPT_SCHEMA_VERSION,
        operation_id: "operation-1".to_string(),
        run_id: "run-2".to_string(),
        conversation_id: "conversation-1".to_string(),
        assistant_message_id: "assistant-2".to_string(),
        request_index: 1,
        attempt_index: 1,
        model: "test-model".to_string(),
        provider_transition_source_model_display_name: None,
        provider_transition_target_model_display_name: None,
        api_style: crate::AgentApiStyle::OpenAiCompatible,
        status: crate::ContextCompactionReceiptStatus::InProgress,
        stage: crate::ContextCompactionReceiptStage::Planned,
        plan: crate::ContextCompactionReceiptPlan {
            context_revision: "0000000000000001".to_string(),
            persistent_revision: "0000000000000001".to_string(),
            request_input_tokens: 120,
            available_input_tokens: Some(120),
            request_trigger_input_tokens: Some(100),
            request_target_input_tokens: Some(30),
            source_input_tokens: 100,
            retained_input_tokens: 0,
            target_replacement_tokens: 30,
            expected_reclaimed_tokens: 70,
            planned_reclaimed_tokens: 70,
            projected_request_input_tokens: 50,
            best_effort: false,
            protected_input_tokens: 0,
            protected_reasons: Default::default(),
            atomic_unit_count: 2,
            previous_summary_id: None,
            covered_through: ContextJournalCursor::message("assistant-1"),
        },
        source_revision: None,
        generation_observation_id: None,
        summary_id: None,
        result: None,
        error: None,
        started_at: 10,
        updated_at: 10,
        completed_at: None,
    }
}

fn completed_observation() -> ModelRequestObservation {
    crate::model_request_observation::ModelRequestObservationBuilder::new(
        "model-request-operation-1",
        "run-2",
        Some("conversation-1".to_string()),
        Some("assistant-2".to_string()),
        Some("operation-1".to_string()),
        1,
        crate::ModelRequestPurpose::ContextCompaction,
        "test-model",
        crate::AgentApiStyle::OpenAiCompatible,
        None,
        10,
    )
    .completed(
        Some(crate::AgentUsage {
            input_tokens: Some(90),
            output_tokens: Some(10),
            output_thinking_tokens: None,
            total_tokens: Some(100),
            cached_input_tokens: None,
            cache_creation_input_tokens: None,
            billable_request_count: Some(1),
        }),
        Some("stop".to_string()),
        11,
    )
    .unwrap()
}

fn applied_receipt(
    mut receipt: ContextCompactionReceipt,
    prefix: &ContextCompactionPrefix,
    draft: &ContextCompactionSummaryDraft,
    observation: &ModelRequestObservation,
) -> ContextCompactionReceipt {
    receipt.attach_prepared_prefix(prefix, 11).unwrap();
    receipt.complete_applied(draft, observation, 12).unwrap();
    receipt
}

fn world_state_snapshot(epoch_id: &str, sequence: u64, permission: &str) -> WorldStateSnapshot {
    WorldStateSnapshot::new(
        epoch_id,
        sequence,
        vec![WorldStateSectionEnvelope::model_visible(
            WorldStateSectionId::EffectivePermissions,
            WorldStateLifetime::Conversation,
            json!({ "permission": permission }),
            json!({ "permission": permission }),
        )
        .unwrap()],
    )
    .unwrap()
}

fn seed_world_state(connection: &mut Connection) -> WorldStateSnapshot {
    let initial = world_state_snapshot("world-state-source", 0, "ask");
    let current = world_state_snapshot("world-state-source", 1, "allow");
    let diff = WorldStateDiff::between(&initial, &current).unwrap();
    world_state_repository::append_record(
        connection,
        &world_state_repository::ConversationWorldStateRecordWrite {
            conversation_id: "conversation-1",
            epoch_generation: 1,
            base_summary_id: None,
            effective_before_message_id: None,
            record: &WorldStateRecord::Full(initial),
            created_at: 5,
        },
    )
    .unwrap();
    world_state_repository::append_record(
        connection,
        &world_state_repository::ConversationWorldStateRecordWrite {
            conversation_id: "conversation-1",
            epoch_generation: 1,
            base_summary_id: None,
            effective_before_message_id: Some("user-2"),
            record: &WorldStateRecord::Diff(diff),
            created_at: 6,
        },
    )
    .unwrap();
    current
}

#[test]
fn summary_commit_atomically_rebases_existing_world_state() {
    let mut connection = setup();
    let expected_state = seed_world_state(&mut connection);
    let expected_at_cutoff = world_state_snapshot("world-state-source", 0, "ask");
    let prefix = prepare_prefix(
        &connection,
        "conversation-1",
        &ContextJournalCursor::message("assistant-1"),
    )
    .unwrap();
    let summary = commit_prefix_replacement(
        &mut connection,
        &prefix,
        draft(&prefix, "summary-world-state"),
        "assistant-1",
    )
    .unwrap();

    let entries =
        world_state_repository::list_active_journal_entries(&connection, "conversation-1").unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].epoch_generation, 2);
    assert_eq!(
        entries[0].base_summary_id.as_deref(),
        Some("summary-world-state")
    );
    assert_eq!(
        entries[0].record.epoch_id(),
        world_state_epoch_id_for_summary("summary-world-state")
    );
    assert_eq!(entries[0].record.sequence(), 0);
    assert_eq!(
        entries[0].record.revision(),
        expected_at_cutoff.revision,
        "a diff anchored after the summary cutoff must not be folded into the new full"
    );
    assert_eq!(
        entries[1].effective_before_message_id.as_deref(),
        Some("user-2")
    );
    assert_eq!(entries[1].record.epoch_id(), entries[0].record.epoch_id());
    assert_eq!(entries[1].record.sequence(), 1);
    assert_eq!(
        entries[1].record.base_revision(),
        Some(entries[0].record.revision())
    );
    assert_eq!(entries[1].record.revision(), expected_state.revision);
    assert_eq!(
        world_state_repository::fold_active_snapshot(&connection, "conversation-1")
            .unwrap()
            .unwrap()
            .revision,
        expected_state.revision
    );
    assert_eq!(summary.created_at, entries[0].created_at);
    assert_eq!(
        world_state_repository::list_records_for_epoch(
            &connection,
            "conversation-1",
            "world-state-source"
        )
        .unwrap()
        .len(),
        2,
        "the previous epoch remains as durable audit history"
    );
}

#[test]
fn summary_commit_releases_covered_provider_continuation_in_the_same_transaction() {
    let mut connection = setup();
    let covered = provider_continuation_record("assistant-2", "run-2", 0, &["call-1"]);
    let uncovered = provider_continuation_record("assistant-2", "run-2", 1, &["call-2"]);
    provider_continuation_repository::store_active_in_connection(&connection, &covered).unwrap();
    provider_continuation_repository::store_active_in_connection(&connection, &uncovered).unwrap();
    let prefix = prepare_prefix(
        &connection,
        "conversation-1",
        &ContextJournalCursor::trace_item("assistant-2", 1),
    )
    .unwrap();

    commit_prefix_replacement(
        &mut connection,
        &prefix,
        draft(&prefix, "summary-provider-release"),
        "assistant-2",
    )
    .unwrap();

    assert_eq!(
        provider_continuation_state(&connection, &covered.continuation_id),
        ("released".to_string(), None)
    );
    assert_eq!(
        provider_continuation_state(&connection, &uncovered.continuation_id).0,
        "active"
    );
}

#[test]
fn rolled_back_summary_transaction_preserves_provider_continuation_ciphertext() {
    let mut connection = setup();
    let covered = provider_continuation_record("assistant-2", "run-2", 0, &["call-1"]);
    provider_continuation_repository::store_active_in_connection(&connection, &covered).unwrap();
    let original_ciphertext = provider_continuation_state(&connection, &covered.continuation_id)
        .1
        .unwrap();
    let prefix = prepare_prefix(
        &connection,
        "conversation-1",
        &ContextJournalCursor::trace_item("assistant-2", 1),
    )
    .unwrap();

    let transaction = connection.transaction().unwrap();
    commit_prefix_replacement_in_transaction(
        &transaction,
        &prefix,
        draft(&prefix, "summary-provider-rollback"),
        "assistant-2",
    )
    .unwrap();
    assert_eq!(
        provider_continuation_state(&transaction, &covered.continuation_id),
        ("released".to_string(), None)
    );
    transaction.rollback().unwrap();

    assert_eq!(
        provider_continuation_state(&connection, &covered.continuation_id),
        ("active".to_string(), Some(original_ciphertext))
    );
    assert!(get_active_summary(&connection, "conversation-1")
        .unwrap()
        .is_none());
}

#[test]
fn trace_item_compaction_keeps_a_turn_with_an_uncovered_call() {
    let mut connection = setup();
    let partial = provider_continuation_record("assistant-2", "run-2", 0, &["call-1", "call-2"]);
    provider_continuation_repository::store_active_in_connection(&connection, &partial).unwrap();
    let prefix = prepare_prefix(
        &connection,
        "conversation-1",
        &ContextJournalCursor::trace_item("assistant-2", 1),
    )
    .unwrap();

    commit_prefix_replacement(
        &mut connection,
        &prefix,
        draft(&prefix, "summary-trace-provider-release"),
        "assistant-2",
    )
    .unwrap();

    assert_eq!(
        provider_continuation_state(&connection, &partial.continuation_id).0,
        "active",
        "one covered ToolResult cannot release a Provider turn that also owns a later call"
    );
}

#[test]
fn summary_rollback_fails_closed_after_provider_replay_was_released() {
    let mut connection = setup();
    let first_prefix = prepare_prefix(
        &connection,
        "conversation-1",
        &ContextJournalCursor::message("user-1"),
    )
    .unwrap();
    commit_prefix_replacement(
        &mut connection,
        &first_prefix,
        draft(&first_prefix, "summary-before-provider-turn"),
        "assistant-1",
    )
    .unwrap();

    let covered = provider_continuation_record("assistant-2", "run-2", 0, &["call-1"]);
    provider_continuation_repository::store_active_in_connection(&connection, &covered).unwrap();
    let provider_prefix = prepare_prefix(
        &connection,
        "conversation-1",
        &ContextJournalCursor::trace_item("assistant-2", 1),
    )
    .unwrap();
    commit_prefix_replacement(
        &mut connection,
        &provider_prefix,
        draft(&provider_prefix, "summary-after-provider-turn"),
        "assistant-2",
    )
    .unwrap();

    let error = rollback_active_summary(
        &mut connection,
        "conversation-1",
        "summary-after-provider-turn",
        20,
    )
    .unwrap_err();
    assert!(error.is_stale());
    assert!(error
        .to_string()
        .contains("provider_context_boundary_required"));
    assert_eq!(
        get_active_summary(&connection, "conversation-1")
            .unwrap()
            .unwrap()
            .id,
        "summary-after-provider-turn",
        "a rejected rollback must leave the active head unchanged"
    );
    assert_eq!(
        provider_continuation_state(&connection, &covered.continuation_id),
        ("released".to_string(), None)
    );
}

#[test]
fn multi_level_summary_rollback_stops_at_released_provider_boundary() {
    let mut connection = setup();
    let first_prefix = prepare_prefix(
        &connection,
        "conversation-1",
        &ContextJournalCursor::message("user-1"),
    )
    .unwrap();
    commit_prefix_replacement(
        &mut connection,
        &first_prefix,
        draft(&first_prefix, "summary-level-1"),
        "assistant-1",
    )
    .unwrap();

    let covered = provider_continuation_record("assistant-2", "run-2", 0, &["call-1"]);
    provider_continuation_repository::store_active_in_connection(&connection, &covered).unwrap();
    let provider_prefix = prepare_prefix(
        &connection,
        "conversation-1",
        &ContextJournalCursor::trace_item("assistant-2", 1),
    )
    .unwrap();
    commit_prefix_replacement(
        &mut connection,
        &provider_prefix,
        draft(&provider_prefix, "summary-level-2-provider"),
        "assistant-2",
    )
    .unwrap();

    connection
        .execute(
            "INSERT INTO messages (
                id, conversation_id, role, content, status, agent_run_json,
                ui_state_json, created_at, position
             ) VALUES (
                'user-3', 'conversation-1', 'user', 'later request', 'sent',
                NULL, NULL, 5, 4
             )",
            [],
        )
        .unwrap();
    let later_prefix = prepare_prefix(
        &connection,
        "conversation-1",
        &ContextJournalCursor::message("user-3"),
    )
    .unwrap();
    commit_prefix_replacement(
        &mut connection,
        &later_prefix,
        draft(&later_prefix, "summary-level-3-generic"),
        "assistant-2",
    )
    .unwrap();

    let restored = rollback_active_summary(
        &mut connection,
        "conversation-1",
        "summary-level-3-generic",
        20,
    )
    .unwrap()
    .unwrap();
    assert_eq!(restored.id, "summary-level-2-provider");

    let error = rollback_active_summary(
        &mut connection,
        "conversation-1",
        "summary-level-2-provider",
        21,
    )
    .unwrap_err();
    assert!(error
        .to_string()
        .contains("provider_context_boundary_required"));
    assert_eq!(
        get_active_summary(&connection, "conversation-1")
            .unwrap()
            .unwrap()
            .id,
        "summary-level-2-provider"
    );
}

#[test]
fn trace_cursor_rebase_keeps_later_message_anchored_diff_in_the_new_epoch_tail() {
    let mut connection = setup();
    let state_at_trace = seed_world_state(&mut connection);
    connection
        .execute(
            "INSERT INTO messages (
                id, conversation_id, role, content, status, agent_run_json,
                ui_state_json, created_at, position
             ) VALUES (
                'user-3', 'conversation-1', 'user', 'future request', 'sent',
                NULL, NULL, 4, 4
             )",
            [],
        )
        .unwrap();
    let future = world_state_snapshot("world-state-source", 2, "deny");
    let future_diff = WorldStateDiff::between(&state_at_trace, &future).unwrap();
    world_state_repository::append_record(
        &mut connection,
        &world_state_repository::ConversationWorldStateRecordWrite {
            conversation_id: "conversation-1",
            epoch_generation: 1,
            base_summary_id: None,
            effective_before_message_id: Some("user-3"),
            record: &WorldStateRecord::Diff(future_diff),
            created_at: 7,
        },
    )
    .unwrap();

    let trace_cursor = ContextJournalCursor::trace_item("assistant-2", 1);
    let prefix = prepare_prefix(&connection, "conversation-1", &trace_cursor).unwrap();
    let summary = commit_prefix_replacement(
        &mut connection,
        &prefix,
        draft(&prefix, "summary-trace-boundary"),
        "assistant-2",
    )
    .unwrap();
    assert_eq!(summary.covered_through, trace_cursor);

    let entries =
        world_state_repository::list_active_journal_entries(&connection, "conversation-1").unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(
        entries[0].record.revision(),
        state_at_trace.revision,
        "the full snapshot must describe state effective at the trace cursor"
    );
    assert_eq!(
        entries[1].effective_before_message_id.as_deref(),
        Some("user-3")
    );
    assert_eq!(entries[1].record.sequence(), 1);
    assert_eq!(
        entries[1].record.base_revision(),
        Some(entries[0].record.revision())
    );
    assert_eq!(entries[1].record.revision(), future.revision);
    assert_eq!(
        world_state_repository::fold_active_snapshot(&connection, "conversation-1")
            .unwrap()
            .unwrap()
            .revision,
        future.revision
    );
}

#[test]
fn summary_rollback_atomically_restores_the_matching_exact_world_state_epoch() {
    let mut connection = setup();
    let expected_state = seed_world_state(&mut connection);
    let prefix = prepare_prefix(
        &connection,
        "conversation-1",
        &ContextJournalCursor::message("assistant-1"),
    )
    .unwrap();
    commit_prefix_replacement(
        &mut connection,
        &prefix,
        draft(&prefix, "summary-to-rollback"),
        "assistant-1",
    )
    .unwrap();

    let restored =
        rollback_active_summary(&mut connection, "conversation-1", "summary-to-rollback", 20)
            .unwrap();
    assert!(restored.is_none());
    assert!(get_active_summary(&connection, "conversation-1")
        .unwrap()
        .is_none());
    let entries =
        world_state_repository::list_active_journal_entries(&connection, "conversation-1").unwrap();
    assert_eq!(entries[0].epoch_generation, 1);
    assert_eq!(entries[0].record.epoch_id(), "world-state-source");
    assert_eq!(
        world_state_repository::fold_active_snapshot(&connection, "conversation-1")
            .unwrap()
            .unwrap(),
        expected_state
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM conversation_world_state_epochs",
                [],
                |row| row.get::<_, u64>(0),
            )
            .unwrap(),
        1
    );
}

#[test]
fn summary_rollback_fails_closed_when_it_would_drop_later_world_state() {
    let mut connection = setup();
    seed_world_state(&mut connection);
    let prefix = prepare_prefix(
        &connection,
        "conversation-1",
        &ContextJournalCursor::message("assistant-1"),
    )
    .unwrap();
    commit_prefix_replacement(
        &mut connection,
        &prefix,
        draft(&prefix, "summary-with-later-state"),
        "assistant-1",
    )
    .unwrap();
    let current = world_state_repository::fold_active_snapshot(&connection, "conversation-1")
        .unwrap()
        .unwrap();
    let later = world_state_snapshot(&current.epoch_id, current.sequence + 1, "deny");
    world_state_repository::append_record(
        &mut connection,
        &world_state_repository::ConversationWorldStateRecordWrite {
            conversation_id: "conversation-1",
            epoch_generation: 2,
            base_summary_id: Some("summary-with-later-state"),
            effective_before_message_id: Some("assistant-2"),
            record: &WorldStateRecord::Diff(WorldStateDiff::between(&current, &later).unwrap()),
            created_at: 21,
        },
    )
    .unwrap();

    let error = rollback_active_summary(
        &mut connection,
        "conversation-1",
        "summary-with-later-state",
        22,
    )
    .unwrap_err();
    assert!(matches!(error, ContextCompactionRepositoryError::Stale(_)));
    assert_eq!(
        get_active_summary(&connection, "conversation-1")
            .unwrap()
            .unwrap()
            .id,
        "summary-with-later-state"
    );
    let entries =
        world_state_repository::list_active_journal_entries(&connection, "conversation-1").unwrap();
    assert_eq!(entries[0].epoch_generation, 2);
    assert_eq!(entries.len(), 3);
    assert_eq!(
        world_state_repository::fold_active_snapshot(&connection, "conversation-1")
            .unwrap()
            .unwrap()
            .revision,
        later.revision
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM conversation_world_state_epochs",
                [],
                |row| row.get::<_, u64>(0),
            )
            .unwrap(),
        2,
        "the failed rollback must restore the summary-owned epoch"
    );
}

#[test]
fn compacts_complete_history_then_advances_inside_current_run() {
    let mut connection = setup();
    let first_cursor = ContextJournalCursor::message("assistant-1");
    let first = prepare_prefix(&connection, "conversation-1", &first_cursor).unwrap();
    commit_prefix_replacement(
        &mut connection,
        &first,
        draft(&first, "summary-1"),
        "assistant-1",
    )
    .unwrap();

    let run_cursor = ContextJournalCursor::trace_item("assistant-2", 1);
    let second = prepare_prefix(&connection, "conversation-1", &run_cursor).unwrap();
    assert!(second.source_items.iter().any(|item| {
            matches!(item, ContextCompactionSourceItem::TraceItem { cursor, .. } if cursor == &run_cursor)
        }));
    let summary = commit_prefix_replacement(
        &mut connection,
        &second,
        draft(&second, "summary-2"),
        "assistant-2",
    )
    .unwrap();
    assert_eq!(summary.previous_summary_id.as_deref(), Some("summary-1"));
    assert_eq!(summary.covered_through, run_cursor);

    let raw_count = connection
        .query_row(
            "SELECT COUNT(*) FROM messages WHERE conversation_id = 'conversation-1'",
            [],
            |row| row.get::<_, u64>(0),
        )
        .unwrap();
    assert_eq!(raw_count, 4);
}

#[test]
fn command_session_lifecycle_stays_in_audit_but_out_of_the_compaction_journal() {
    let connection = setup();
    connection
        .execute(
            "INSERT INTO messages (
                id, conversation_id, role, content, status, agent_run_json,
                ui_state_json, created_at, position
             ) VALUES (
                'assistant-command', 'conversation-1', 'assistant',
                'The managed command was handed off.', 'sent', NULL, NULL, 5, 5
             )",
            [],
        )
        .unwrap();
    let session_id = "cmd_0123456789abcdef0123456789abcdef";
    let call_id = "command-call";
    let mut trace = ConversationTurnTrace {
        schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
        run_id: "run-command".to_string(),
        conversation_id: "conversation-1".to_string(),
        assistant_message_id: "assistant-command".to_string(),
        terminal_status: ConversationTurnTraceTerminalStatus::Completed,
        terminal_error: None,
        truncated: false,
        items: vec![
            ConversationTurnTraceItem::ToolCall {
                sequence: 0,
                call_id: call_id.to_string(),
                tool: "run_command".to_string(),
                provenance: crate::AgentToolIdentity::Builtin {
                    tool_name: "run_command".to_string(),
                },
                operation: json!({ "command": "long-running-command" }),
                approval_status: AgentApprovalStatus::Approved,
                truncated: false,
            },
            ConversationTurnTraceItem::CommandSessionLifecycle {
                sequence: 1,
                phase: ConversationCommandSessionLifecyclePhase::Started,
                session_id: session_id.to_string(),
                call_id: call_id.to_string(),
                status: AgentCommandSessionStatus::Running,
                exit_code: None,
                latest_sequence: 0,
                output_truncated: false,
                archive: Default::default(),
                created_at: 5,
            },
            ConversationTurnTraceItem::ToolResult {
                sequence: 2,
                call_id: call_id.to_string(),
                tool: "run_command".to_string(),
                status: ConversationTraceToolResultStatus::Succeeded,
                success: true,
                observation: json!({
                    "status": "running",
                    "sessionId": session_id,
                }),
                approval_status: AgentApprovalStatus::Approved,
                error: None,
                truncated: false,
                archive: Default::default(),
            },
        ],
    };
    conversation_trace_repository::commit_trace_in_connection(&connection, &trace, 5, 6).unwrap();

    let cursor = ContextJournalCursor::message("assistant-command");
    let before_terminal = prepare_prefix(&connection, "conversation-1", &cursor).unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM conversation_turn_trace_items
                 WHERE assistant_message_id = 'assistant-command'
                   AND item_kind = 'command_session_lifecycle'",
                [],
                |row| row.get::<_, u64>(0),
            )
            .unwrap(),
        1,
        "the started lifecycle event must remain durable audit"
    );
    assert!(before_terminal.source_items.iter().all(|item| {
        !matches!(
            item,
            ContextCompactionSourceItem::TraceItem { item, .. }
                if matches!(&**item, ConversationTurnTraceItem::CommandSessionLifecycle { .. })
        )
    }));

    trace
        .append_command_session_lifecycle(ConversationCommandSessionLifecycle {
            phase: ConversationCommandSessionLifecyclePhase::Terminal,
            session_id: session_id.to_string(),
            call_id: call_id.to_string(),
            status: AgentCommandSessionStatus::Exited,
            exit_code: Some(0),
            latest_sequence: 7,
            output_truncated: false,
            archive: Default::default(),
            created_at: 7,
        })
        .unwrap();
    conversation_trace_repository::commit_trace_in_connection(&connection, &trace, 5, 7).unwrap();

    let after_terminal = prepare_prefix(&connection, "conversation-1", &cursor).unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM conversation_turn_trace_items
                 WHERE assistant_message_id = 'assistant-command'
                   AND item_kind = 'command_session_lifecycle'",
                [],
                |row| row.get::<_, u64>(0),
            )
            .unwrap(),
        2,
        "the terminal lifecycle event must remain durable audit"
    );
    assert_eq!(after_terminal.source_items, before_terminal.source_items);
    assert_eq!(
        after_terminal.source_revision,
        before_terminal.source_revision
    );
    let source_json = serde_json::to_string(&after_terminal.source_items).unwrap();
    assert!(!source_json.contains("command_session_lifecycle"));
    assert!(!source_json.contains("\"phase\":\"terminal\""));
}

#[test]
fn audited_success_commits_observation_summary_head_and_receipt_together() {
    let mut connection = setup();
    let expected_state = seed_world_state(&mut connection);
    let prefix = prepare_prefix(
        &connection,
        "conversation-1",
        &ContextJournalCursor::message("assistant-1"),
    )
    .unwrap();
    let draft = draft(&prefix, "summary-audited");
    let observation = completed_observation();
    let receipt = planned_receipt();
    context_compaction_receipt_repository::record_receipt(&mut connection, &receipt, None).unwrap();
    let receipt = applied_receipt(receipt, &prefix, &draft, &observation);

    let summary = commit_prefix_replacement_with_receipt(
        &mut connection,
        &prefix,
        draft,
        &receipt,
        &observation,
    )
    .unwrap();

    assert_eq!(summary.id, "summary-audited");
    assert_eq!(
            connection
                .query_row(
                    "SELECT summary_id FROM conversation_context_compaction_heads WHERE conversation_id = 'conversation-1'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "summary-audited"
        );
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM model_request_observations",
                [],
                |row| { row.get::<_, u64>(0) }
            )
            .unwrap(),
        1
    );
    assert_eq!(
        context_compaction_receipt_repository::get_receipt(&connection, "operation-1")
            .unwrap()
            .unwrap()
            .status,
        crate::ContextCompactionReceiptStatus::Applied
    );
    let world_state =
        world_state_repository::list_active_journal_entries(&connection, "conversation-1").unwrap();
    assert_eq!(world_state.len(), 2);
    assert_eq!(
        world_state[0].base_summary_id.as_deref(),
        Some("summary-audited")
    );
    assert_eq!(world_state[1].record.revision(), expected_state.revision);

    connection
        .execute("DELETE FROM conversations WHERE id = 'conversation-1'", [])
        .unwrap();
    for table in [
        "model_request_observations",
        "context_compaction_receipts",
        "context_compaction_summaries",
        "conversation_context_compaction_heads",
        "conversation_world_state_epochs",
        "conversation_world_state_records",
    ] {
        assert_eq!(
            connection
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get::<_, u64>(0)
                })
                .unwrap(),
            0,
            "{table} must cascade with its conversation"
        );
    }
}

#[test]
fn audited_success_rolls_every_fact_back_when_terminal_receipt_write_fails() {
    let mut connection = setup();
    let expected_state = seed_world_state(&mut connection);
    let prefix = prepare_prefix(
        &connection,
        "conversation-1",
        &ContextJournalCursor::message("assistant-1"),
    )
    .unwrap();
    let draft = draft(&prefix, "summary-rolled-back");
    let observation = completed_observation();
    let receipt = planned_receipt();
    context_compaction_receipt_repository::record_receipt(&mut connection, &receipt, None).unwrap();
    let receipt = applied_receipt(receipt, &prefix, &draft, &observation);
    connection
        .execute_batch(
            "CREATE TRIGGER reject_applied_receipt
                 BEFORE UPDATE ON context_compaction_receipts
                 WHEN NEW.status = 'applied'
                 BEGIN
                    SELECT RAISE(ABORT, 'forced receipt failure');
                 END;",
        )
        .unwrap();

    let error = commit_prefix_replacement_with_receipt(
        &mut connection,
        &prefix,
        draft,
        &receipt,
        &observation,
    )
    .unwrap_err();

    assert!(matches!(
        error,
        ContextCompactionRepositoryError::Database(_)
    ));
    for table in [
        "context_compaction_summaries",
        "conversation_context_compaction_heads",
        "model_request_observations",
    ] {
        assert_eq!(
            connection
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get::<_, u64>(0)
                })
                .unwrap(),
            0,
            "{table} must roll back"
        );
    }
    assert_eq!(
        context_compaction_receipt_repository::get_receipt(&connection, "operation-1")
            .unwrap()
            .unwrap()
            .status,
        crate::ContextCompactionReceiptStatus::InProgress
    );
    let world_state =
        world_state_repository::list_active_journal_entries(&connection, "conversation-1").unwrap();
    assert_eq!(world_state.len(), 2);
    assert_eq!(world_state[0].epoch_generation, 1);
    assert_eq!(
        world_state_repository::fold_active_snapshot(&connection, "conversation-1")
            .unwrap()
            .unwrap(),
        expected_state
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM conversation_world_state_epochs",
                [],
                |row| row.get::<_, u64>(0),
            )
            .unwrap(),
        1,
        "the failed receipt must roll back the rebased epoch"
    );
}

#[test]
fn world_state_rebase_failure_rolls_back_summary_and_head() {
    let mut connection = setup();
    let expected_state = seed_world_state(&mut connection);
    let prefix = prepare_prefix(
        &connection,
        "conversation-1",
        &ContextJournalCursor::message("assistant-1"),
    )
    .unwrap();
    connection
        .execute_batch(
            "CREATE TRIGGER reject_compaction_world_state_rebase
             BEFORE INSERT ON conversation_world_state_records
             WHEN NEW.record_kind = 'diff' AND NEW.epoch_id != 'world-state-source'
             BEGIN
                SELECT RAISE(ABORT, 'forced migrated world state diff failure');
             END;",
        )
        .unwrap();

    let error = commit_prefix_replacement(
        &mut connection,
        &prefix,
        draft(&prefix, "summary-rebase-failure"),
        "assistant-1",
    )
    .unwrap_err();
    assert!(matches!(
        error,
        ContextCompactionRepositoryError::Database(_)
    ));
    assert!(get_active_summary(&connection, "conversation-1")
        .unwrap()
        .is_none());
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM context_compaction_summaries",
                [],
                |row| row.get::<_, u64>(0),
            )
            .unwrap(),
        0
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM conversation_world_state_epochs",
                [],
                |row| row.get::<_, u64>(0),
            )
            .unwrap(),
        1
    );
    assert_eq!(
        world_state_repository::fold_active_snapshot(&connection, "conversation-1")
            .unwrap()
            .unwrap(),
        expected_state
    );
}

#[test]
fn stale_prefix_rolls_back_without_rebasing_world_state() {
    let mut connection = setup();
    let expected_state = seed_world_state(&mut connection);
    let prefix = prepare_prefix(
        &connection,
        "conversation-1",
        &ContextJournalCursor::message("assistant-1"),
    )
    .unwrap();
    connection
        .execute(
            "UPDATE messages SET content = 'changed during generation' WHERE id = 'user-1'",
            [],
        )
        .unwrap();

    let error = commit_prefix_replacement(
        &mut connection,
        &prefix,
        draft(&prefix, "summary-stale-world-state"),
        "assistant-1",
    )
    .unwrap_err();
    assert!(matches!(error, ContextCompactionRepositoryError::Stale(_)));
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM context_compaction_summaries",
                [],
                |row| row.get::<_, u64>(0),
            )
            .unwrap(),
        0
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM conversation_world_state_epochs",
                [],
                |row| row.get::<_, u64>(0),
            )
            .unwrap(),
        1
    );
    assert_eq!(
        world_state_repository::fold_active_snapshot(&connection, "conversation-1")
            .unwrap()
            .unwrap(),
        expected_state
    );
}

#[test]
fn rejects_a_structurally_valid_but_tampered_continuity_snapshot() {
    let mut connection = setup();
    let cursor = ContextJournalCursor::message("assistant-1");
    let prefix = prepare_prefix(&connection, "conversation-1", &cursor).unwrap();
    let mut tampered = draft(&prefix, "summary-tampered");
    tampered.continuity.task_evidence_refs[0] = crate::ContextHistoryRef::message("assistant-1");
    tampered.continuity.validate().unwrap();

    let error =
        commit_prefix_replacement(&mut connection, &prefix, tampered, "assistant-1").unwrap_err();

    assert!(matches!(
        error,
        ContextCompactionRepositoryError::Invalid(_)
    ));
    assert!(get_active_summary(&connection, "conversation-1")
        .unwrap()
        .is_none());
}

#[test]
fn appending_after_a_trace_cursor_keeps_the_summary_valid() {
    let mut connection = setup();
    let cursor = ContextJournalCursor::trace_item("assistant-2", 1);
    let prefix = prepare_prefix(&connection, "conversation-1", &cursor).unwrap();
    commit_prefix_replacement(
        &mut connection,
        &prefix,
        draft(&prefix, "summary-1"),
        "assistant-2",
    )
    .unwrap();

    let mut trace =
        conversation_trace_repository::get_trace_for_message(&connection, "assistant-2")
            .unwrap()
            .unwrap();
    trace
        .items
        .push(ConversationTurnTraceItem::AssistantNarration {
            sequence: 2,
            content: "continue".to_string(),
            truncated: false,
        });
    conversation_trace_repository::commit_trace_in_connection(&connection, &trace, 3, 5).unwrap();

    assert_eq!(
        get_active_summary(&connection, "conversation-1")
            .unwrap()
            .unwrap()
            .id,
        "summary-1"
    );
}

#[test]
fn mutation_inside_covered_raw_prefix_drops_the_derived_head() {
    let mut connection = setup();
    let cursor = ContextJournalCursor::message("assistant-1");
    let prefix = prepare_prefix(&connection, "conversation-1", &cursor).unwrap();
    commit_prefix_replacement(
        &mut connection,
        &prefix,
        draft(&prefix, "summary-1"),
        "assistant-1",
    )
    .unwrap();
    connection
        .execute(
            "UPDATE messages SET content = 'edited request' WHERE id = 'user-1'",
            [],
        )
        .unwrap();

    assert!(get_active_summary(&connection, "conversation-1")
        .unwrap()
        .is_none());
}

#[test]
fn mutation_cannot_drop_a_summary_after_provider_replay_was_released() {
    let mut connection = setup();
    let covered = provider_continuation_record("assistant-2", "run-2", 0, &["call-1"]);
    provider_continuation_repository::store_active_in_connection(&connection, &covered).unwrap();
    let prefix = prepare_prefix(
        &connection,
        "conversation-1",
        &ContextJournalCursor::trace_item("assistant-2", 1),
    )
    .unwrap();
    commit_prefix_replacement(
        &mut connection,
        &prefix,
        draft(&prefix, "summary-provider-mutation-boundary"),
        "assistant-2",
    )
    .unwrap();
    connection
        .execute(
            "UPDATE messages SET content = 'edited request' WHERE id = 'user-1'",
            [],
        )
        .unwrap();

    let error = get_active_summary(&connection, "conversation-1").unwrap_err();
    assert!(error
        .to_string()
        .contains("provider_context_boundary_required"));
    assert_eq!(
        connection
            .query_row(
                "SELECT summary_id FROM conversation_context_compaction_heads
                 WHERE conversation_id = 'conversation-1'",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
        "summary-provider-mutation-boundary",
        "fail-closed invalidation must leave the summary head intact"
    );
}

#[test]
fn deleting_a_turn_removes_the_summary_generated_by_that_turn() {
    let mut connection = setup();
    let prefix = prepare_prefix(
        &connection,
        "conversation-1",
        &ContextJournalCursor::message("assistant-1"),
    )
    .unwrap();
    let draft = draft(&prefix, "summary-from-discarded-run");
    let observation = completed_observation();
    let receipt = planned_receipt();
    context_compaction_receipt_repository::record_receipt(&mut connection, &receipt, None).unwrap();
    let receipt = applied_receipt(receipt, &prefix, &draft, &observation);
    commit_prefix_replacement_with_receipt(&mut connection, &prefix, draft, &receipt, &observation)
        .unwrap();

    // The summary only covers older messages. Raw-prefix validation alone would therefore
    // leave it active after deleting the turn that actually generated it.
    crate::storage::chat_repository::delete_messages(
        &mut connection,
        "conversation-1",
        &["user-2".to_string(), "assistant-2".to_string()],
    )
    .unwrap();

    assert!(get_active_summary(&connection, "conversation-1")
        .unwrap()
        .is_none());
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM context_compaction_summaries",
                [],
                |row| row.get::<_, u64>(0),
            )
            .unwrap(),
        0
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM context_compaction_receipts",
                [],
                |row| row.get::<_, u64>(0),
            )
            .unwrap(),
        0
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM model_request_observations",
                [],
                |row| row.get::<_, u64>(0),
            )
            .unwrap(),
        0
    );
}

#[test]
fn deleting_a_turn_restores_the_summary_active_before_that_turn() {
    let mut connection = setup();
    let previous_prefix = prepare_prefix(
        &connection,
        "conversation-1",
        &ContextJournalCursor::message("user-1"),
    )
    .unwrap();
    commit_prefix_replacement(
        &mut connection,
        &previous_prefix,
        draft(&previous_prefix, "summary-before-run"),
        "assistant-1",
    )
    .unwrap();

    let prefix = prepare_prefix(
        &connection,
        "conversation-1",
        &ContextJournalCursor::message("assistant-1"),
    )
    .unwrap();
    let draft = draft(&prefix, "summary-from-discarded-run");
    let observation = completed_observation();
    let mut receipt = planned_receipt();
    receipt.plan.previous_summary_id = Some("summary-before-run".to_string());
    context_compaction_receipt_repository::record_receipt(&mut connection, &receipt, None).unwrap();
    let receipt = applied_receipt(receipt, &prefix, &draft, &observation);
    commit_prefix_replacement_with_receipt(&mut connection, &prefix, draft, &receipt, &observation)
        .unwrap();

    crate::storage::chat_repository::delete_messages(
        &mut connection,
        "conversation-1",
        &["user-2".to_string(), "assistant-2".to_string()],
    )
    .unwrap();

    assert_eq!(
        get_active_summary(&connection, "conversation-1")
            .unwrap()
            .unwrap()
            .id,
        "summary-before-run"
    );
    let summaries = {
        let mut statement = connection
            .prepare("SELECT id FROM context_compaction_summaries ORDER BY id")
            .unwrap();
        statement
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
    };
    assert_eq!(summaries, vec!["summary-before-run"]);
}

#[test]
fn deleting_a_summary_owner_cannot_expose_released_provider_history() {
    let mut connection = setup();
    let previous_prefix = prepare_prefix(
        &connection,
        "conversation-1",
        &ContextJournalCursor::message("user-1"),
    )
    .unwrap();
    commit_prefix_replacement(
        &mut connection,
        &previous_prefix,
        draft(&previous_prefix, "summary-before-provider-delete"),
        "assistant-1",
    )
    .unwrap();

    let covered = provider_continuation_record("assistant-2", "run-2", 0, &["call-1"]);
    provider_continuation_repository::store_active_in_connection(&connection, &covered).unwrap();
    let provider_prefix = prepare_prefix(
        &connection,
        "conversation-1",
        &ContextJournalCursor::trace_item("assistant-2", 1),
    )
    .unwrap();
    commit_prefix_replacement(
        &mut connection,
        &provider_prefix,
        draft(&provider_prefix, "summary-owned-by-deleted-turn"),
        "assistant-2",
    )
    .unwrap();

    let error = crate::storage::chat_repository::delete_messages(
        &mut connection,
        "conversation-1",
        &["user-2".to_string(), "assistant-2".to_string()],
    )
    .unwrap_err();
    assert!(error
        .to_string()
        .contains("provider_context_boundary_required"));
    assert_eq!(
        get_active_summary(&connection, "conversation-1")
            .unwrap()
            .unwrap()
            .id,
        "summary-owned-by-deleted-turn"
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM messages WHERE id IN ('user-2', 'assistant-2')",
                [],
                |row| row.get::<_, u64>(0),
            )
            .unwrap(),
        2,
        "the rejected edit/delete transaction must not remove either message"
    );
    assert_eq!(
        provider_continuation_state(&connection, &covered.continuation_id),
        ("released".to_string(), None)
    );
}

fn seed_provider_transition_target(connection: &Connection, target_model_id: &str, revision: &str) {
    let provider_profile =
        serde_json::to_string(&crate::ProviderProfileConfig::generic_for_dialect(
            crate::ProviderProtocolDialect::OpenAiChatCompletions,
        ))
        .unwrap();
    connection
        .execute(
            "INSERT INTO models (
                id, display_name, supports_image, provider_connection_revision,
                provider_protocol_revision, provider_profile_config_json,
                input_price, output_price, enabled, position, created_at, updated_at
             ) VALUES (?1, ?1, 0, 'provider-connection-v1:test-target', ?2, ?3,
                       '0', '0', 1, 0, 1, 1)",
            params![target_model_id, revision, provider_profile],
        )
        .unwrap();
}

fn settle_provider_transition_source_turn(connection: &Connection) {
    connection
        .execute(
            "UPDATE conversation_turn_traces
             SET terminal_status = 'completed', updated_at = 5, completed_at = 5
             WHERE conversation_id = 'conversation-1' AND terminal_status = 'in_progress'",
            [],
        )
        .unwrap();
}

fn conversation_revision(connection: &Connection) -> i64 {
    connection
        .query_row(
            "SELECT revision FROM conversations WHERE id = 'conversation-1'",
            [],
            |row| row.get(0),
        )
        .unwrap()
}

fn applied_provider_transition_receipt(
    prefix: &ContextCompactionPrefix,
    target_model_id: &str,
) -> (
    ContextCompactionSummaryDraft,
    ContextCompactionReceipt,
    ContextCompactionReceipt,
    ModelRequestObservation,
) {
    let mut transition_draft = draft(prefix, "summary-provider-transition");
    transition_draft.created_at = 24;
    let mut receipt = ContextCompactionReceipt::begin_provider_transition(
        "provider-transition-test",
        "provider-transition-test-compaction",
        "conversation-1",
        "assistant-1",
        target_model_id,
        Some("Source".to_string()),
        Some("Target".to_string()),
        crate::AgentApiStyle::OpenAiCompatible,
        prefix,
        100,
        30,
        20,
    )
    .unwrap();
    let planned_receipt = receipt.clone();
    receipt
        .advance_stage(ContextCompactionReceiptStage::Preparing, 21)
        .unwrap();
    receipt.attach_prepared_prefix(prefix, 22).unwrap();
    let observation = crate::model_request_observation::ModelRequestObservationBuilder::new(
        "model-request-provider-transition-test",
        "provider-transition-test-compaction",
        Some("conversation-1".to_string()),
        Some("assistant-1".to_string()),
        Some("provider-transition-test".to_string()),
        1,
        crate::ModelRequestPurpose::ContextCompaction,
        target_model_id,
        crate::AgentApiStyle::OpenAiCompatible,
        None,
        22,
    )
    .completed(None, Some("stop".to_string()), 23)
    .unwrap();
    receipt
        .advance_stage(ContextCompactionReceiptStage::Committing, 23)
        .unwrap();
    receipt
        .complete_applied(&transition_draft, &observation, 24)
        .unwrap();
    (transition_draft, planned_receipt, receipt, observation)
}

#[test]
fn provider_transition_commit_is_atomic_and_preserves_existing_chat_usage() {
    let mut connection = setup();
    settle_provider_transition_source_turn(&connection);
    connection
        .execute(
            "UPDATE conversations SET model_id = 'source-model', updated_at = 7
             WHERE id = 'conversation-1'",
            [],
        )
        .unwrap();
    seed_provider_transition_target(
        &connection,
        "target-model",
        "provider-protocol-v1:target-revision",
    );
    connection
        .execute(
            "INSERT INTO composer_drafts (
                scope_id, message, permission_mode, permission_mode_version,
                model_id, project_id, attachments_json, skills_json,
                queued_messages_json, updated_at
             ) VALUES ('conversation-1', 'unsent text', 'ask', ?1,
                       'source-model', NULL, '[]', '[]', '[]', 100)",
            [crate::storage::models::CURRENT_COMPOSER_PERMISSION_MODE_VERSION],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO agent_usage_records (
                id, conversation_id, message_id, run_id, model_id, model_name,
                created_at, input_tokens, output_tokens, billable_request_count
             ) VALUES (
                'usage-existing', 'conversation-1', 'assistant-1', 'run-existing',
                'source-model', 'Source', 5, 41, 9, 1
             )",
            [],
        )
        .unwrap();
    let usage_before = connection
        .query_row(
            "SELECT id, run_id, model_id, input_tokens, output_tokens
             FROM agent_usage_records WHERE message_id = 'assistant-1'",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, u64>(3)?,
                    row.get::<_, u64>(4)?,
                ))
            },
        )
        .unwrap();
    let prefix = prepare_prefix(
        &connection,
        "conversation-1",
        &ContextJournalCursor::message("assistant-1"),
    )
    .unwrap();
    let (transition_draft, planned_receipt, receipt, observation) =
        applied_provider_transition_receipt(&prefix, "target-model");
    crate::storage::context_compaction_receipt_repository::record_receipt(
        &mut connection,
        &planned_receipt,
        None,
    )
    .unwrap();
    crate::storage::conversation_context_adaptation_repository::insert_in_connection(
        &connection,
        &crate::storage::conversation_context_adaptation_repository::ConversationContextAdaptationRequirement {
            conversation_id: "conversation-1".to_string(),
            reason: crate::storage::conversation_context_adaptation_repository::FORK_RELEASED_PROVIDER_STATE_REASON.to_string(),
            source_conversation_id: "conversation-source".to_string(),
            source_message_id: "assistant-source".to_string(),
            created_at: 8,
            resolved_summary_id: None,
            resolved_at: None,
        },
    )
    .unwrap();

    let expected_revision = conversation_revision(&connection);
    let (summary, committed_updated_at) = commit_provider_transition_with_receipt(
        &mut connection,
        &prefix,
        transition_draft,
        &receipt,
        &observation,
        Some("source-model"),
        7,
        expected_revision,
        "target-model",
        "provider-protocol-v1:target-revision",
    )
    .unwrap();

    assert_eq!(summary.id, "summary-provider-transition");
    let adaptation = crate::storage::conversation_context_adaptation_repository::get(
        &connection,
        "conversation-1",
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        adaptation.resolved_summary_id.as_deref(),
        Some("summary-provider-transition")
    );
    assert!(!adaptation.is_required());
    assert_eq!(
        connection
            .query_row(
                "SELECT model_id FROM conversations WHERE id = 'conversation-1'",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
        "target-model"
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT model_id FROM composer_drafts WHERE scope_id = 'conversation-1'",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
        "target-model"
    );
    let transition_updated_at = connection
        .query_row(
            "SELECT updated_at FROM conversations WHERE id = 'conversation-1'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap();
    assert_eq!(transition_updated_at, committed_updated_at);
    assert!(transition_updated_at > 100);

    // A Renderer metadata write queued before the transition may arrive after the atomic commit.
    // Its older model selection must not resurrect the source model or overwrite newer metadata.
    crate::storage::chat_repository::save_conversation_meta(
        &connection,
        &crate::storage::models::ChatConversationMetaRecord {
            id: "conversation-1".to_string(),
            project_id: None,
            model_id: Some("source-model".to_string()),
            title: "stale title".to_string(),
            created_at: 1,
            updated_at: 7,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        },
    )
    .unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT model_id, title, updated_at FROM conversations WHERE id = 'conversation-1'",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                },
            )
            .unwrap(),
        (
            "target-model".to_string(),
            "Test".to_string(),
            transition_updated_at,
        )
    );
    let draft_transition_updated_at = connection
        .query_row(
            "SELECT updated_at FROM composer_drafts WHERE scope_id = 'conversation-1'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap();
    assert_eq!(draft_transition_updated_at, committed_updated_at);
    crate::storage::composer_draft_repository::save_composer_draft(
        &connection,
        crate::storage::models::ComposerDraftRecord {
            scope_id: "conversation-1".to_string(),
            message: "stale unsent text".to_string(),
            permission_mode: "default".to_string(),
            permission_mode_version: 0,
            model_id: Some("source-model".to_string()),
            project_id: None,
            attachments_json: "[]".to_string(),
            skills_json: "[]".to_string(),
            queued_messages_json: "[]".to_string(),
            updated_at: 7,
        },
    )
    .unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT model_id, message, updated_at
                 FROM composer_drafts WHERE scope_id = 'conversation-1'",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                },
            )
            .unwrap(),
        (
            "target-model".to_string(),
            "unsent text".to_string(),
            draft_transition_updated_at,
        )
    );
    let usage_after = connection
        .query_row(
            "SELECT id, run_id, model_id, input_tokens, output_tokens
             FROM agent_usage_records WHERE message_id = 'assistant-1'",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, u64>(3)?,
                    row.get::<_, u64>(4)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(usage_after, usage_before);
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM model_request_observations
                 WHERE operation_id = 'provider-transition-test'",
                [],
                |row| row.get::<_, u64>(0),
            )
            .unwrap(),
        1
    );
    let rollback = rollback_active_summary(
        &mut connection,
        "conversation-1",
        "summary-provider-transition",
        committed_updated_at.saturating_add(1),
    )
    .unwrap_err();
    assert!(rollback
        .to_string()
        .contains("provider_context_boundary_required"));
}

#[test]
fn provider_transition_releases_every_replayable_turn_before_switching_models() {
    let mut connection = setup();
    settle_provider_transition_source_turn(&connection);
    connection
        .execute(
            "UPDATE conversations SET model_id = 'source-model', updated_at = 7
             WHERE id = 'conversation-1'",
            [],
        )
        .unwrap();
    seed_provider_transition_target(
        &connection,
        "target-model",
        "provider-protocol-v1:target-revision",
    );
    let covered = provider_continuation_record("assistant-2", "run-2", 0, &["call-1"]);
    provider_continuation_repository::store_active_in_connection(&connection, &covered).unwrap();
    let prefix = prepare_prefix(
        &connection,
        "conversation-1",
        &ContextJournalCursor::trace_item("assistant-2", 1),
    )
    .unwrap();
    let (transition_draft, planned_receipt, receipt, observation) =
        applied_provider_transition_receipt(&prefix, "target-model");
    crate::storage::context_compaction_receipt_repository::record_receipt(
        &mut connection,
        &planned_receipt,
        None,
    )
    .unwrap();

    let expected_revision = conversation_revision(&connection);
    commit_provider_transition_with_receipt(
        &mut connection,
        &prefix,
        transition_draft,
        &receipt,
        &observation,
        Some("source-model"),
        7,
        expected_revision,
        "target-model",
        "provider-protocol-v1:target-revision",
    )
    .unwrap();

    assert_eq!(
        provider_continuation_state(&connection, &covered.continuation_id),
        ("released".to_string(), None)
    );
    assert!(
        !provider_continuation_repository::has_replayable_for_conversation(
            &connection,
            "conversation-1"
        )
        .unwrap()
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT model_id FROM conversations WHERE id = 'conversation-1'",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
        "target-model"
    );
}

#[test]
fn provider_transition_rolls_back_when_private_replay_is_not_covered_by_the_summary() {
    let mut connection = setup();
    settle_provider_transition_source_turn(&connection);
    connection
        .execute(
            "UPDATE conversations SET model_id = 'source-model', updated_at = 7
             WHERE id = 'conversation-1'",
            [],
        )
        .unwrap();
    seed_provider_transition_target(
        &connection,
        "target-model",
        "provider-protocol-v1:target-revision",
    );
    let uncovered =
        provider_continuation_record("assistant-1", "run-missing-trace", 0, &["missing-call"]);
    provider_continuation_repository::store_active_in_connection(&connection, &uncovered).unwrap();
    let original_ciphertext = provider_continuation_state(&connection, &uncovered.continuation_id)
        .1
        .unwrap();
    let prefix = prepare_prefix(
        &connection,
        "conversation-1",
        &ContextJournalCursor::message("assistant-1"),
    )
    .unwrap();
    let (transition_draft, planned_receipt, receipt, observation) =
        applied_provider_transition_receipt(&prefix, "target-model");
    crate::storage::context_compaction_receipt_repository::record_receipt(
        &mut connection,
        &planned_receipt,
        None,
    )
    .unwrap();

    let expected_revision = conversation_revision(&connection);
    let error = commit_provider_transition_with_receipt(
        &mut connection,
        &prefix,
        transition_draft,
        &receipt,
        &observation,
        Some("source-model"),
        7,
        expected_revision,
        "target-model",
        "provider-protocol-v1:target-revision",
    )
    .unwrap_err();

    assert!(error.is_stale());
    assert!(error
        .to_string()
        .contains("provider_context_boundary_required"));
    assert_eq!(
        provider_continuation_state(&connection, &uncovered.continuation_id),
        ("active".to_string(), Some(original_ciphertext))
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT model_id FROM conversations WHERE id = 'conversation-1'",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
        "source-model"
    );
    assert!(get_active_summary(&connection, "conversation-1")
        .unwrap()
        .is_none());
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM model_request_observations
                 WHERE operation_id = 'provider-transition-test'",
                [],
                |row| row.get::<_, u64>(0),
            )
            .unwrap(),
        0
    );
}

#[test]
fn stale_provider_transition_rolls_back_summary_model_draft_and_observation() {
    let mut connection = setup();
    settle_provider_transition_source_turn(&connection);
    connection
        .execute(
            "UPDATE conversations SET model_id = 'source-model', updated_at = 7
             WHERE id = 'conversation-1'",
            [],
        )
        .unwrap();
    seed_provider_transition_target(
        &connection,
        "target-model",
        "provider-protocol-v1:new-revision",
    );
    connection
        .execute(
            "INSERT INTO composer_drafts (
                scope_id, message, permission_mode, permission_mode_version,
                model_id, project_id, attachments_json, skills_json,
                queued_messages_json, updated_at
             ) VALUES ('conversation-1', 'unsent text', 'ask', ?1,
                       'source-model', NULL, '[]', '[]', '[]', 7)",
            [crate::storage::models::CURRENT_COMPOSER_PERMISSION_MODE_VERSION],
        )
        .unwrap();
    let prefix = prepare_prefix(
        &connection,
        "conversation-1",
        &ContextJournalCursor::message("assistant-1"),
    )
    .unwrap();
    let (transition_draft, planned_receipt, receipt, observation) =
        applied_provider_transition_receipt(&prefix, "target-model");
    crate::storage::context_compaction_receipt_repository::record_receipt(
        &mut connection,
        &planned_receipt,
        None,
    )
    .unwrap();

    let expected_revision = conversation_revision(&connection);
    let error = commit_provider_transition_with_receipt(
        &mut connection,
        &prefix,
        transition_draft,
        &receipt,
        &observation,
        Some("source-model"),
        7,
        expected_revision,
        "target-model",
        "provider-protocol-v1:stale-revision",
    )
    .unwrap_err();
    assert!(error.is_stale());
    assert_eq!(
        connection
            .query_row(
                "SELECT model_id FROM conversations WHERE id = 'conversation-1'",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
        "source-model"
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT model_id FROM composer_drafts WHERE scope_id = 'conversation-1'",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
        "source-model"
    );
    assert!(get_active_summary(&connection, "conversation-1")
        .unwrap()
        .is_none());
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM model_request_observations
                 WHERE operation_id = 'provider-transition-test'",
                [],
                |row| row.get::<_, u64>(0),
            )
            .unwrap(),
        0
    );
}

#[test]
fn compacting_provider_transition_rechecks_conversation_revision_without_half_state() {
    let mut connection = setup();
    settle_provider_transition_source_turn(&connection);
    connection
        .execute(
            "UPDATE conversations SET model_id = 'source-model', updated_at = 7
             WHERE id = 'conversation-1'",
            [],
        )
        .unwrap();
    seed_provider_transition_target(
        &connection,
        "target-model",
        "provider-protocol-v1:target-revision",
    );
    let prefix = prepare_prefix(
        &connection,
        "conversation-1",
        &ContextJournalCursor::message("assistant-1"),
    )
    .unwrap();
    let (transition_draft, planned_receipt, receipt, observation) =
        applied_provider_transition_receipt(&prefix, "target-model");
    crate::storage::context_compaction_receipt_repository::record_receipt(
        &mut connection,
        &planned_receipt,
        None,
    )
    .unwrap();
    let expected_revision = conversation_revision(&connection);
    // Title changes do not move `updated_at`; the monotonic revision is what must fence this
    // otherwise-invisible cross-Host mutation.
    connection
        .execute(
            "UPDATE conversations SET title = 'Changed on another Host'
             WHERE id = 'conversation-1'",
            [],
        )
        .unwrap();

    let error = commit_provider_transition_with_receipt(
        &mut connection,
        &prefix,
        transition_draft,
        &receipt,
        &observation,
        Some("source-model"),
        7,
        expected_revision,
        "target-model",
        "provider-protocol-v1:target-revision",
    )
    .unwrap_err();

    assert!(error.is_stale());
    assert_eq!(
        connection
            .query_row(
                "SELECT model_id FROM conversations WHERE id = 'conversation-1'",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
        "source-model"
    );
    assert!(get_active_summary(&connection, "conversation-1")
        .unwrap()
        .is_none());
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM model_request_observations
                 WHERE operation_id = 'provider-transition-test'",
                [],
                |row| row.get::<_, u64>(0),
            )
            .unwrap(),
        0
    );
}

#[test]
fn provider_transition_status_filters_before_applying_its_limit() {
    let mut connection = setup();
    let mut transition = planned_receipt();
    transition.operation_id = "provider-transition-visible".to_string();
    transition.run_id = "provider-transition-visible-compaction".to_string();
    crate::storage::context_compaction_receipt_repository::record_receipt(
        &mut connection,
        &transition,
        None,
    )
    .unwrap();
    for index in 0..60 {
        let mut ordinary = planned_receipt();
        ordinary.operation_id = format!("ordinary-compaction-{index:02}");
        ordinary.run_id = format!("ordinary-compaction-run-{index:02}");
        ordinary.started_at = 100 + index;
        ordinary.updated_at = 100 + index;
        crate::storage::context_compaction_receipt_repository::record_receipt(
            &mut connection,
            &ordinary,
            None,
        )
        .unwrap();
    }

    let receipts =
        crate::storage::context_compaction_receipt_repository::list_provider_transition_receipts(
            &connection,
            "conversation-1",
            None,
            1,
        )
        .unwrap();
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0].operation_id, "provider-transition-visible");
}
