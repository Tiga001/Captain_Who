use super::*;
use crate::storage::{migrations, world_state_repository};
use crate::{
    AgentApprovalStatus, ConversationTraceToolResultStatus, ConversationTurnTrace,
    ConversationTurnTraceItem, ConversationTurnTraceTerminalStatus, WorldStateDiff,
    WorldStateLifetime, WorldStateRecord, WorldStateSectionEnvelope, WorldStateSectionId,
    WorldStateSnapshot, CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
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
        (3, "assistant-2", "assistant", "正在思考...", "pending"),
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
            request_pressure: true,
            durable_input_tokens: 100,
            durable_capacity_tokens: Some(120),
            durable_trigger_input_tokens: Some(100),
            durable_target_input_tokens: Some(30),
            durable_pressure: true,
            source_input_tokens: 100,
            target_replacement_tokens: 30,
            expected_reclaimed_tokens: 70,
            planned_reclaimed_tokens: 70,
            projected_request_input_tokens: 50,
            projected_durable_input_tokens: 30,
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
