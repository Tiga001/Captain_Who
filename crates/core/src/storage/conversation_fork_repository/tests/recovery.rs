#[test]
fn root_fork_time_cutoff_excludes_late_summary_receipt_and_future_unanchored_epoch() {
    let mut connection = Connection::open_in_memory().unwrap();
    migrations::run_migrations(&connection).unwrap();
    let source = source_conversation();
    chat_repository::save_conversation(&mut connection, source.clone()).unwrap();

    let visible = fork_world_state_snapshot("world-visible-at-cutoff", 0, "visible");
    world_state_repository::append_record(
        &mut connection,
        &world_state_repository::ConversationWorldStateRecordWrite {
            conversation_id: &source.id,
            epoch_generation: 1,
            base_summary_id: None,
            effective_before_message_id: None,
            request_boundary: None,
            model_observed: true,
            record: &WorldStateRecord::Full(visible.clone()),
            created_at: 10,
        },
    )
    .unwrap();

    let late_prefix = context_compaction_repository::prepare_prefix(
        &connection,
        &source.id,
        &ContextJournalCursor::message("user-b"),
    )
    .unwrap();
    let late_receipt = record_applied_capacity_compaction(
        &mut connection,
        &late_prefix,
        "summary-owned-by-b-but-completed-after-c",
        "context-compaction-after-root-cutoff",
        "run-source-1",
        "assistant-b",
        70,
    );
    let future = fork_world_state_snapshot("world-future-unanchored", 0, "future");
    world_state_repository::append_record(
        &mut connection,
        &world_state_repository::ConversationWorldStateRecordWrite {
            conversation_id: &source.id,
            epoch_generation: 3,
            base_summary_id: None,
            effective_before_message_id: None,
            request_boundary: None,
            model_observed: true,
            record: &WorldStateRecord::Full(future.clone()),
            created_at: 80,
        },
    )
    .unwrap();

    assert_eq!(
        authoritative_fork_cutoff_at(
            &connection,
            &source.id,
            &ConversationForkPoint::AssistantReply {
                assistant_message_id: "assistant-c".to_string(),
            },
        )
        .unwrap(),
        60
    );
    let source_chain =
        context_compaction_repository::list_active_summary_chain(&connection, &source.id).unwrap();
    assert_eq!(source_chain.len(), 1);
    assert_eq!(source_chain[0].summary.created_at, 70);
    assert!(
        summaries_visible_at_time(&connection, &source.id, source_chain, 60)
            .unwrap()
            .is_empty()
    );

    let plan = build_assistant_reply_fork_plan(
        &connection,
        "fork-before-late-root-facts",
        &source.id,
        "assistant-c",
        100,
    )
    .unwrap();
    assert!(plan.summaries.is_empty());
    assert!(plan.compaction_receipts.is_empty());
    assert_eq!(plan.world_state_records.len(), 1);
    assert_eq!(
        plan.world_state_records[0].record.revision(),
        visible.revision
    );
    commit_fork_plan(&mut connection, &plan).unwrap();

    assert!(
        context_compaction_repository::get_active_summary(&connection, &plan.target.id)
            .unwrap()
            .is_none()
    );
    assert!(
        context_compaction_receipt_repository::list_receipts_for_conversation(
            &connection,
            &plan.target.id,
        )
        .unwrap()
        .is_empty()
    );
    assert!(context_compaction_receipt_repository::get_receipt(
        &connection,
        &late_receipt.operation_id,
    )
    .unwrap()
    .is_some());
    let forked_world = world_state_repository::fold_active_snapshot(&connection, &plan.target.id)
        .unwrap()
        .unwrap();
    assert_eq!(forked_world.revision, visible.revision);
    assert_ne!(forked_world.revision, future.revision);
}

#[test]
fn root_fork_rejects_a_visible_run_draft_mutated_after_the_time_cutoff() {
    let mut connection = Connection::open_in_memory().unwrap();
    migrations::run_migrations(&connection).unwrap();
    let source = source_conversation();
    chat_repository::save_conversation(&mut connection, source.clone()).unwrap();
    let base_revision = crate::content_revision(b"before");
    let (observation_id, observation_json) = file_change_observation(
        &source.id,
        "run-source-1",
        "call-late-begin",
        "/tmp/late.md",
        &base_revision,
    );
    file_change_repository::insert_file_change(
        &connection,
        &AgentFileChangeRecord {
            schema_version: 1,
            id: "file-change-mutated-after-cutoff".to_string(),
            conversation_id: source.id.clone(),
            project_id: None,
            run_id: "run-source-1".to_string(),
            source_tool_name: "apply_patch".to_string(),
            source_tool_call_id: "call-late-begin".to_string(),
            source_tool_arguments_digest: "not-reached".to_string(),
            permission_revision: "permission-1".to_string(),
            tool_set_revision: "tool-set-1".to_string(),
            provider_wire_revision: "provider-protocol-v1".to_string(),
            observation_id,
            observation_json,
            file_path: "late.md".to_string(),
            operation: "update".to_string(),
            strategy: Some("rewrite".to_string()),
            status: "drafting".to_string(),
            base_revision: Some(base_revision),
            base_content: "before".to_string(),
            content: "after".to_string(),
            draft_revision: 0,
            next_mutation_index: 0,
            additions: 1,
            deletions: 1,
            line_count: 1,
            byte_count: 5,
            mutation_count: 0,
            stats_final: false,
            summary: None,
            final_action_id: None,
            final_action_arguments_digest: None,
            final_permission_revision: None,
            final_tool_set_revision: None,
            final_provider_wire_revision: None,
            created_at: 30,
            updated_at: 70,
            expires_at: i64::MAX,
        },
    )
    .unwrap();

    let error = build_assistant_reply_fork_plan(
        &connection,
        "fork-reject-late-draft",
        &source.id,
        "assistant-c",
        100,
    )
    .unwrap_err();
    assert!(error
        .message()
        .contains("文件变更事务在分叉点后发生过不可版本化的变化"));
    assert_eq!(
        connection
            .query_row("SELECT COUNT(*) FROM conversations", [], |row| {
                row.get::<_, u64>(0)
            })
            .unwrap(),
        1
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM conversation_forks WHERE request_id = ?1",
                ["fork-reject-late-draft"],
                |row| row.get::<_, u64>(0),
            )
            .unwrap(),
        0
    );
}

#[test]
fn fork_commit_uses_the_file_change_history_frozen_in_the_plan() {
    let mut connection = Connection::open_in_memory().unwrap();
    migrations::run_migrations(&connection).unwrap();
    let source = source_conversation();
    chat_repository::save_conversation(&mut connection, source.clone()).unwrap();
    let transaction_id = "file-change-plan-snapshot";
    let begin_args = apply_patch_args(
        json!({"action":"begin","operation":"update","strategy":"rewrite","filePath":"snapshot.md","observationId":format!("fobs_{}", "1".repeat(32))}),
    );
    let append_args = apply_patch_args(
        json!({"action":"append","transactionId":transaction_id,"index":0,"expectedDraftRevision":0,"content":"planned"}),
    );
    let mut trace_items =
        staged_tool_exchange(0, "call-plan-begin", "apply_patch", begin_args.clone());
    trace_items.extend(staged_tool_exchange(
        2,
        "call-plan-append",
        "apply_patch",
        append_args.clone(),
    ));
    trace_items.extend(staged_tool_exchange(
        4,
        "call-plan-commit",
        "apply_patch",
        apply_patch_args(
            json!({"action":"commit","transactionId":transaction_id,"expectedDraftRevision":1}),
        ),
    ));
    conversation_trace_repository::replace_trace(
        &mut connection,
        &ConversationTurnTrace {
            schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
            run_id: "run-source-1".to_string(),
            conversation_id: source.id.clone(),
            assistant_message_id: "assistant-b".to_string(),
            terminal_status: ConversationTurnTraceTerminalStatus::Completed,
            terminal_error: None,
            truncated: false,
            items: trace_items,
        },
        20,
        21,
    )
    .unwrap();
    let base_revision = crate::content_revision(b"before");
    let (observation_id, observation_json) = file_change_observation(
        &source.id,
        "run-source-1",
        "call-plan-begin",
        "/tmp/snapshot.md",
        &base_revision,
    );
    let mut change = AgentFileChangeRecord {
        schema_version: 1,
        id: transaction_id.to_string(),
        conversation_id: source.id.clone(),
        project_id: None,
        run_id: "run-source-1".to_string(),
        source_tool_name: "apply_patch".to_string(),
        source_tool_call_id: "call-plan-begin".to_string(),
        source_tool_arguments_digest: crate::file_change::proposal_digest(&begin_args).unwrap(),
        permission_revision: "permission-1".to_string(),
        tool_set_revision: "tool-set-1".to_string(),
        provider_wire_revision: "provider-protocol-v1".to_string(),
        observation_id,
        observation_json,
        file_path: "snapshot.md".to_string(),
        operation: "update".to_string(),
        strategy: Some("rewrite".to_string()),
        status: "drafting".to_string(),
        base_revision: Some(base_revision),
        base_content: "before".to_string(),
        content: "planned".to_string(),
        draft_revision: 1,
        next_mutation_index: 1,
        additions: 1,
        deletions: 1,
        line_count: 1,
        byte_count: 7,
        mutation_count: 1,
        stats_final: false,
        summary: Some("planned snapshot".to_string()),
        final_action_id: None,
        final_action_arguments_digest: None,
        final_permission_revision: None,
        final_tool_set_revision: None,
        final_provider_wire_revision: None,
        created_at: 30,
        updated_at: 40,
        expires_at: i64::MAX,
    };
    let mut initial_change = change.clone();
    initial_change.content.clear();
    initial_change.draft_revision = 0;
    initial_change.next_mutation_index = 0;
    initial_change.mutation_count = 0;
    initial_change.byte_count = 0;
    initial_change.status = "drafting".to_string();
    file_change_repository::insert_file_change(&connection, &initial_change).unwrap();
    let first_operation = crate::storage::models::AgentFileChangeOperationRecord {
        transaction_id: transaction_id.to_string(),
        mutation_index: 0,
        source_tool_call_id: "call-plan-append".to_string(),
        source_tool_arguments_digest: crate::file_change::proposal_digest(&append_args).unwrap(),
        action: "append".to_string(),
        payload_digest: "operation-before-plan".to_string(),
        draft_revision: 1,
        receipt_json: staged_mutation_receipt(transaction_id, 0),
        created_at: 40,
    };
    let first_chunk = crate::storage::models::AgentFileChangeChunkRecord {
        transaction_id: transaction_id.to_string(),
        mutation_index: 0,
        content_digest: "chunk-before-plan".to_string(),
        byte_count: 7,
        created_at: 40,
    };
    file_change_repository::save_file_change_progress(
        &mut connection,
        0,
        &change,
        Some(&first_chunk),
        &first_operation,
    )
    .unwrap();

    let plan = build_assistant_reply_fork_plan(
        &connection,
        "fork-frozen-file-change-plan",
        &source.id,
        "assistant-c",
        100,
    )
    .unwrap();
    assert_eq!(plan.file_changes.len(), 1);
    assert_eq!(plan.file_changes[0].history.operations.len(), 1);

    change.content = "mutated-after-plan".to_string();
    change.draft_revision = 2;
    change.next_mutation_index = 2;
    change.mutation_count = 2;
    change.updated_at = 50;
    let second_args = apply_patch_args(
        json!({"action":"append","transactionId":transaction_id,"index":1,"expectedDraftRevision":1,"content":"later"}),
    );
    let second_operation = crate::storage::models::AgentFileChangeOperationRecord {
        transaction_id: transaction_id.to_string(),
        mutation_index: 1,
        source_tool_call_id: "call-after-plan".to_string(),
        source_tool_arguments_digest: crate::file_change::proposal_digest(&second_args).unwrap(),
        action: "append".to_string(),
        payload_digest: "operation-after-plan".to_string(),
        draft_revision: 2,
        receipt_json: staged_mutation_receipt(transaction_id, 1),
        created_at: 50,
    };
    let second_chunk = crate::storage::models::AgentFileChangeChunkRecord {
        transaction_id: transaction_id.to_string(),
        mutation_index: 1,
        content_digest: "chunk-after-plan".to_string(),
        byte_count: 10,
        created_at: 50,
    };
    file_change_repository::save_file_change_progress(
        &mut connection,
        1,
        &change,
        Some(&second_chunk),
        &second_operation,
    )
    .unwrap();

    commit_fork_plan(&mut connection, &plan).unwrap();
    let target_change =
        file_change_repository::get_file_change(&connection, &plan.file_changes[0].target.id)
            .unwrap()
            .unwrap();
    assert_eq!(target_change.content, "planned");
    assert_eq!(target_change.draft_revision, 1);
    assert!(
        file_change_repository::get_operation(&connection, &target_change.id, 0)
            .unwrap()
            .is_some()
    );
    assert!(
        file_change_repository::get_operation(&connection, &target_change.id, 1)
            .unwrap()
            .is_none()
    );
}

#[test]
fn fork_clones_only_world_state_visible_at_cutoff_and_remaps_anchors_and_summary() {
    let mut connection = Connection::open_in_memory().unwrap();
    migrations::run_migrations(&connection).unwrap();
    let source = source_conversation();
    chat_repository::save_conversation(&mut connection, source.clone()).unwrap();

    let initial = fork_world_state_snapshot("source-world-state", 0, "initial");
    world_state_repository::append_record(
        &mut connection,
        &world_state_repository::ConversationWorldStateRecordWrite {
            conversation_id: &source.id,
            epoch_generation: 1,
            base_summary_id: None,
            effective_before_message_id: None,
            request_boundary: None,
            model_observed: true,
            record: &WorldStateRecord::Full(initial),
            created_at: 10,
        },
    )
    .unwrap();
    let prefix = context_compaction_repository::prepare_prefix(
        &connection,
        &source.id,
        &ContextJournalCursor::message("user-b"),
    )
    .unwrap();
    context_compaction_repository::commit_prefix_replacement(
        &mut connection,
        &prefix,
        summary_draft(&prefix, "summary-world-state", 40),
        "assistant-b",
    )
    .unwrap();

    let rebased = world_state_repository::fold_active_snapshot(&connection, &source.id)
        .unwrap()
        .unwrap();
    let at_c = fork_world_state_snapshot(&rebased.epoch_id, rebased.sequence + 1, "visible-at-c");
    let diff_at_c = WorldStateDiff::between(&rebased, &at_c).unwrap();
    world_state_repository::append_record(
        &mut connection,
        &world_state_repository::ConversationWorldStateRecordWrite {
            conversation_id: &source.id,
            epoch_generation: 2,
            base_summary_id: Some("summary-world-state"),
            effective_before_message_id: Some("user-c"),
            request_boundary: None,
            model_observed: true,
            record: &WorldStateRecord::Diff(diff_at_c),
            created_at: 50,
        },
    )
    .unwrap();
    let after_cutoff = fork_world_state_snapshot(&at_c.epoch_id, at_c.sequence + 1, "after-cutoff");
    let diff_after_cutoff = WorldStateDiff::between(&at_c, &after_cutoff).unwrap();
    world_state_repository::append_record(
        &mut connection,
        &world_state_repository::ConversationWorldStateRecordWrite {
            conversation_id: &source.id,
            epoch_generation: 2,
            base_summary_id: Some("summary-world-state"),
            effective_before_message_id: Some("user-d"),
            request_boundary: None,
            model_observed: true,
            record: &WorldStateRecord::Diff(diff_after_cutoff),
            created_at: 70,
        },
    )
    .unwrap();

    let plan = build_assistant_reply_fork_plan(
        &connection,
        "fork-world-state-at-c",
        &source.id,
        "assistant-c",
        100,
    )
    .unwrap();
    assert_eq!(plan.summaries.len(), 1);
    assert_eq!(plan.world_state_records.len(), 2);
    assert_eq!(
        plan.world_state_records[1]
            .effective_before_message_id
            .as_deref(),
        Some("user-c")
    );
    commit_fork_plan(&mut connection, &plan).unwrap();

    let target_chain =
        context_compaction_repository::list_active_summary_chain(&connection, &plan.target.id)
            .unwrap();
    assert_eq!(target_chain.len(), 1);
    assert_eq!(
        target_chain[0].lineage.source_summary_id.as_deref(),
        Some("summary-world-state")
    );
    let target_records =
        world_state_repository::list_active_journal_entries(&connection, &plan.target.id).unwrap();
    assert_eq!(target_records.len(), 2);
    assert_eq!(target_records[0].epoch_generation, 1);
    assert_eq!(
        target_records[0].base_summary_id.as_deref(),
        Some(target_chain[0].summary.id.as_str())
    );
    assert_eq!(target_records[0].effective_before_message_id, None);
    assert_eq!(
        target_records[1].effective_before_message_id.as_deref(),
        Some(plan.message_id_map["user-c"].as_str())
    );
    assert!(target_records
        .iter()
        .all(|entry| entry.effective_before_message_id.as_deref() != Some("user-d")));
    assert_eq!(
        world_state_repository::fold_active_snapshot(&connection, &plan.target.id)
            .unwrap()
            .unwrap()
            .revision,
        at_c.revision
    );
}

#[test]
fn fork_selects_the_latest_world_state_epoch_whose_summary_is_visible_at_cutoff() {
    let mut connection = Connection::open_in_memory().unwrap();
    migrations::run_migrations(&connection).unwrap();
    let source = source_conversation();
    chat_repository::save_conversation(&mut connection, source.clone()).unwrap();

    let initial = fork_world_state_snapshot("world-state-epoch-1", 0, "initial");
    let at_b = fork_world_state_snapshot("world-state-epoch-1", 1, "at-b");
    world_state_repository::append_record(
        &mut connection,
        &world_state_repository::ConversationWorldStateRecordWrite {
            conversation_id: &source.id,
            epoch_generation: 1,
            base_summary_id: None,
            effective_before_message_id: None,
            request_boundary: None,
            model_observed: true,
            record: &WorldStateRecord::Full(initial.clone()),
            created_at: 10,
        },
    )
    .unwrap();
    world_state_repository::append_record(
        &mut connection,
        &world_state_repository::ConversationWorldStateRecordWrite {
            conversation_id: &source.id,
            epoch_generation: 1,
            base_summary_id: None,
            effective_before_message_id: Some("user-b"),
            request_boundary: None,
            model_observed: true,
            record: &WorldStateRecord::Diff(WorldStateDiff::between(&initial, &at_b).unwrap()),
            created_at: 30,
        },
    )
    .unwrap();

    let first_prefix = context_compaction_repository::prepare_prefix(
        &connection,
        &source.id,
        &ContextJournalCursor::message("user-b"),
    )
    .unwrap();
    context_compaction_repository::commit_prefix_replacement(
        &mut connection,
        &first_prefix,
        summary_draft(&first_prefix, "summary-world-state-1", 40),
        "assistant-b",
    )
    .unwrap();
    let epoch_two = world_state_repository::fold_active_snapshot(&connection, &source.id)
        .unwrap()
        .unwrap();
    assert_eq!(epoch_two.revision, at_b.revision);
    let at_c = fork_world_state_snapshot(&epoch_two.epoch_id, epoch_two.sequence + 1, "at-c");
    world_state_repository::append_record(
        &mut connection,
        &world_state_repository::ConversationWorldStateRecordWrite {
            conversation_id: &source.id,
            epoch_generation: 2,
            base_summary_id: Some("summary-world-state-1"),
            effective_before_message_id: Some("user-c"),
            request_boundary: None,
            model_observed: true,
            record: &WorldStateRecord::Diff(WorldStateDiff::between(&epoch_two, &at_c).unwrap()),
            created_at: 50,
        },
    )
    .unwrap();

    let second_prefix = context_compaction_repository::prepare_prefix(
        &connection,
        &source.id,
        &ContextJournalCursor::message("user-d"),
    )
    .unwrap();
    context_compaction_repository::commit_prefix_replacement(
        &mut connection,
        &second_prefix,
        summary_draft(&second_prefix, "summary-world-state-2", 80),
        "assistant-d",
    )
    .unwrap();
    let active =
        world_state_repository::list_active_journal_entries(&connection, &source.id).unwrap();
    assert_eq!(active[0].epoch_generation, 3);
    assert_eq!(
        active[0].base_summary_id.as_deref(),
        Some("summary-world-state-2")
    );

    let early = build_assistant_reply_fork_plan(
        &connection,
        "fork-before-all-summaries",
        &source.id,
        "assistant-a",
        100,
    )
    .unwrap();
    assert!(early.summaries.is_empty());
    assert_eq!(early.world_state_records.len(), 1);
    assert_eq!(early.world_state_records[0].epoch_generation, 1);
    assert_eq!(early.world_state_records[0].base_summary_id, None);
    assert_eq!(
        early.world_state_records[0].record.revision(),
        initial.revision
    );
    commit_fork_plan(&mut connection, &early).unwrap();
    assert_eq!(
        world_state_repository::fold_active_snapshot(&connection, &early.target.id)
            .unwrap()
            .unwrap()
            .revision,
        initial.revision
    );

    let middle = build_assistant_reply_fork_plan(
        &connection,
        "fork-between-world-state-summaries",
        &source.id,
        "assistant-c",
        110,
    )
    .unwrap();
    assert_eq!(middle.summaries.len(), 1);
    assert_eq!(middle.world_state_records.len(), 2);
    assert!(middle
        .world_state_records
        .iter()
        .all(|entry| entry.epoch_generation == 2));
    assert_eq!(
        middle.world_state_records[0].base_summary_id.as_deref(),
        Some("summary-world-state-1")
    );
    assert_eq!(
        middle.world_state_records[1]
            .effective_before_message_id
            .as_deref(),
        Some("user-c")
    );
    commit_fork_plan(&mut connection, &middle).unwrap();
    let middle_target =
        world_state_repository::list_active_journal_entries(&connection, &middle.target.id)
            .unwrap();
    assert_eq!(middle_target.len(), 2);
    assert_eq!(
        middle_target[0].base_summary_id.as_deref(),
        context_compaction_repository::get_active_summary(&connection, &middle.target.id)
            .unwrap()
            .as_ref()
            .map(|summary| summary.id.as_str())
    );
    assert_eq!(
        middle_target[1].effective_before_message_id.as_deref(),
        Some(middle.message_id_map["user-c"].as_str())
    );
    assert_eq!(
        world_state_repository::fold_active_snapshot(&connection, &middle.target.id)
            .unwrap()
            .unwrap()
            .revision,
        at_c.revision
    );
}

fn fork_world_state_snapshot(epoch_id: &str, sequence: u64, value: &str) -> WorldStateSnapshot {
    WorldStateSnapshot::new(
        epoch_id,
        sequence,
        vec![WorldStateSectionEnvelope::model_visible(
            WorldStateSectionId::EffectivePermissions,
            WorldStateLifetime::Conversation,
            json!({ "value": value }),
            json!({ "value": value }),
        )
        .unwrap()],
    )
    .unwrap()
}

fn source_conversation() -> ChatConversationRecord {
    let messages = [
        ("user-a", "user", 10),
        ("assistant-a", "assistant", 20),
        ("user-b", "user", 30),
        ("assistant-b", "assistant", 40),
        ("user-c", "user", 50),
        ("assistant-c", "assistant", 60),
        ("user-d", "user", 70),
        ("assistant-d", "assistant", 80),
    ]
    .into_iter()
    .map(|(id, role, created_at)| ChatMessageRecord {
        human_interaction_response: None,
        id: id.to_string(),
        role: role.to_string(),
        content: format!("content {id}"),
        created_at,
        status: Some("sent".to_string()),
        attachments: Vec::new(),
        agent_run_json: (role == "assistant").then(|| {
            json!({
                "runId": format!("run-source-{}", (created_at / 20) - 1),
                "status": "completed",
                "toolDefinitions": [],
                "toolCalls": [],
                "toolResults": [],
                "approvals": [],
                "fileChangeProposals": [],
                "fileChanges": [],
                "timeline": [],
                "usage": {
                    "inputTokens": 100,
                    "outputTokens": 20,
                    "outputThinkingTokens": 5,
                    "totalTokens": 125,
                    "cachedInputTokens": 10,
                    "cacheCreationInputTokens": 2,
                    "billableRequestCount": 1
                }
            })
            .to_string()
        }),
        ui_state_json: None,
    })
    .collect();
    ChatConversationRecord {
        id: "conversation-source".to_string(),
        project_id: None,
        model_id: Some("model-1".to_string()),
        title: "Source task".to_string(),
        messages,
        created_at: 10,
        updated_at: 80,
        pinned_at: None,
        archived_at: None,
        unread_at: None,
    }
}

fn summary_draft(
    prefix: &crate::ContextCompactionPrefix,
    id: &str,
    created_at: i64,
) -> ContextCompactionSummaryDraft {
    ContextCompactionSummaryDraft {
        id: id.to_string(),
        source_revision: prefix.source_revision.clone(),
        content: format!("summary {id}"),
        continuity: ContextContinuitySnapshot::from_prefix(prefix).unwrap(),
        generation: ContextCompactionGeneration::test(),
        source_input_tokens: 1_000,
        summary_input_tokens: 100,
        continuity_input_tokens: 100,
        uncovered_tail_input_tokens: 0,
        replacement_input_tokens: 200,
        created_at,
    }
}

#[allow(clippy::too_many_arguments)]
fn record_applied_capacity_compaction(
    connection: &mut Connection,
    prefix: &crate::ContextCompactionPrefix,
    summary_id: &str,
    operation_id: &str,
    run_id: &str,
    assistant_message_id: &str,
    completed_at: i64,
) -> ContextCompactionReceipt {
    assert!(!operation_id.starts_with("provider-transition-"));
    let started_at = completed_at.saturating_sub(2);
    let mut receipt = ContextCompactionReceipt {
        schema_version: crate::CONTEXT_COMPACTION_RECEIPT_SCHEMA_VERSION,
        operation_id: operation_id.to_string(),
        run_id: run_id.to_string(),
        conversation_id: prefix.conversation_id.clone(),
        assistant_message_id: assistant_message_id.to_string(),
        request_index: 1,
        attempt_index: 1,
        model_config_id: None,
        model: "model-1".to_string(),
        provider_transition_source_model_display_name: None,
        provider_transition_target_model_display_name: None,
        api_style: crate::AgentApiStyle::OpenAiCompatible,
        status: ContextCompactionReceiptStatus::InProgress,
        stage: ContextCompactionReceiptStage::Planned,
        plan: crate::ContextCompactionReceiptPlan {
            context_revision: prefix.source_revision.clone(),
            persistent_revision: prefix.source_revision.clone(),
            request_input_tokens: 1_000,
            available_input_tokens: Some(1_000),
            request_trigger_input_tokens: Some(900),
            request_target_input_tokens: Some(200),
            source_input_tokens: 1_000,
            retained_input_tokens: 0,
            target_replacement_tokens: 200,
            expected_reclaimed_tokens: 800,
            planned_reclaimed_tokens: 800,
            projected_request_input_tokens: 200,
            best_effort: false,
            protected_input_tokens: 0,
            protected_reasons: Default::default(),
            atomic_unit_count: prefix.source_items.len().max(1),
            previous_summary_id: prefix
                .previous_summary
                .as_ref()
                .map(|summary| summary.id.clone()),
            covered_through: prefix.covered_through.clone(),
        },
        source_revision: None,
        generation_observation_id: None,
        summary_id: None,
        result: None,
        error: None,
        started_at,
        updated_at: started_at,
        completed_at: None,
    };
    receipt.validate().unwrap();
    context_compaction_receipt_repository::record_receipt(connection, &receipt, None).unwrap();
    receipt
        .attach_prepared_prefix(prefix, completed_at.saturating_sub(1))
        .unwrap();
    let draft = summary_draft(prefix, summary_id, completed_at);
    let observation = crate::model_request_observation::ModelRequestObservationBuilder::new(
        format!("observation-{operation_id}"),
        run_id,
        Some(prefix.conversation_id.clone()),
        Some(assistant_message_id.to_string()),
        Some(operation_id.to_string()),
        1,
        crate::ModelRequestPurpose::ContextCompaction,
        "model-1",
        crate::AgentApiStyle::OpenAiCompatible,
        None,
        completed_at.saturating_sub(1),
    )
    .completed(None, Some("stop".to_string()), completed_at)
    .unwrap();
    receipt
        .complete_applied(&draft, &observation, completed_at)
        .unwrap();
    context_compaction_repository::commit_prefix_replacement_with_receipt(
        connection,
        prefix,
        draft,
        &receipt,
        &observation,
    )
    .unwrap();
    receipt
}

#[allow(clippy::too_many_arguments)]
fn assert_cloned_capacity_compaction_receipt(
    connection: &Connection,
    source: &ContextCompactionReceipt,
    target: &ContextCompactionReceipt,
    target_conversation_id: &str,
    target_assistant_message_id: &str,
    target_covered_message_id: &str,
    target_summary_id: &str,
    target_run_id: &str,
) {
    assert_eq!(target.status, ContextCompactionReceiptStatus::Applied);
    assert_eq!(target.stage, ContextCompactionReceiptStage::Completed);
    assert_ne!(target.operation_id, source.operation_id);
    assert_ne!(target.run_id, source.run_id);
    assert_ne!(target.conversation_id, source.conversation_id);
    assert_ne!(target.assistant_message_id, source.assistant_message_id);
    assert_ne!(target.summary_id, source.summary_id);
    assert_ne!(
        target.generation_observation_id,
        source.generation_observation_id
    );
    assert_eq!(target.run_id, target_run_id);
    assert_eq!(target.conversation_id, target_conversation_id);
    assert_eq!(target.assistant_message_id, target_assistant_message_id);
    assert_eq!(target.summary_id.as_deref(), Some(target_summary_id));
    assert_eq!(
        target
            .result
            .as_ref()
            .map(|result| result.summary_id.as_str()),
        Some(target_summary_id)
    );
    assert_eq!(
        target.plan.covered_through,
        ContextJournalCursor::message(target_covered_message_id)
    );
    let observation = model_request_observation_repository::get_observation(
        connection,
        target
            .generation_observation_id
            .as_deref()
            .expect("cloned capacity receipt observation"),
    )
    .unwrap()
    .expect("cloned capacity receipt observation row");
    assert_eq!(observation.run_id, target_run_id);
    assert_eq!(
        observation.conversation_id.as_deref(),
        Some(target_conversation_id)
    );
    assert_eq!(
        observation.assistant_message_id.as_deref(),
        Some(target_assistant_message_id)
    );
    assert_eq!(
        observation.operation_id.as_deref(),
        Some(target.operation_id.as_str())
    );
    target
        .validate_generation_observation(&observation)
        .unwrap();
}

fn record_agent_loop_model(
    connection: &Connection,
    conversation_id: &str,
    assistant_message_id: &str,
    run_id: &str,
    model_id: &str,
    completed_at: i64,
) {
    let observation = crate::model_request_observation::ModelRequestObservationBuilder::new(
        format!("observation-{assistant_message_id}"),
        run_id,
        Some(conversation_id.to_string()),
        Some(assistant_message_id.to_string()),
        None,
        1,
        crate::ModelRequestPurpose::AgentLoop,
        model_id,
        crate::AgentApiStyle::OpenAiCompatible,
        None,
        completed_at.saturating_sub(1),
    )
    .completed(None, Some("stop".to_string()), completed_at)
    .unwrap();
    crate::storage::model_request_observation_repository::insert_observation(
        connection,
        &observation,
    )
    .unwrap();
}

#[allow(clippy::too_many_arguments)]
fn record_provider_transition_summary(
    connection: &mut Connection,
    prefix: &crate::ContextCompactionPrefix,
    summary_id: &str,
    operation_id: &str,
    assistant_message_id: &str,
    target_model_id: &str,
    created_at: i64,
) {
    let mut receipt = crate::ContextCompactionReceipt::begin_provider_transition(
        operation_id,
        format!("run-{operation_id}"),
        &prefix.conversation_id,
        assistant_message_id,
        target_model_id,
        target_model_id,
        Some("Source".to_string()),
        Some("Target".to_string()),
        crate::AgentApiStyle::OpenAiCompatible,
        prefix,
        1_000,
        200,
        created_at.saturating_sub(4),
    )
    .unwrap();
    crate::storage::context_compaction_receipt_repository::record_receipt(
        connection, &receipt, None,
    )
    .unwrap();
    receipt
        .advance_stage(
            crate::ContextCompactionReceiptStage::Preparing,
            created_at.saturating_sub(3),
        )
        .unwrap();
    crate::storage::context_compaction_receipt_repository::record_receipt(
        connection, &receipt, None,
    )
    .unwrap();
    receipt
        .attach_prepared_prefix(prefix, created_at.saturating_sub(2))
        .unwrap();
    crate::storage::context_compaction_receipt_repository::record_receipt(
        connection, &receipt, None,
    )
    .unwrap();
    receipt
        .advance_stage(
            crate::ContextCompactionReceiptStage::Committing,
            created_at.saturating_sub(1),
        )
        .unwrap();
    crate::storage::context_compaction_receipt_repository::record_receipt(
        connection, &receipt, None,
    )
    .unwrap();
    let draft = summary_draft(prefix, summary_id, created_at);
    context_compaction_repository::commit_prefix_replacement(
        connection,
        prefix,
        draft.clone(),
        assistant_message_id,
    )
    .unwrap();
    let observation = crate::model_request_observation::ModelRequestObservationBuilder::new(
        format!("observation-{operation_id}"),
        format!("run-{operation_id}"),
        Some(prefix.conversation_id.clone()),
        Some(assistant_message_id.to_string()),
        Some(operation_id.to_string()),
        1,
        crate::ModelRequestPurpose::ContextCompaction,
        target_model_id,
        crate::AgentApiStyle::OpenAiCompatible,
        None,
        created_at.saturating_sub(2),
    )
    .completed(None, Some("stop".to_string()), created_at)
    .unwrap();
    receipt
        .complete_applied(&draft, &observation, created_at)
        .unwrap();
    crate::storage::context_compaction_receipt_repository::record_receipt(
        connection,
        &receipt,
        Some(&observation),
    )
    .unwrap();
}

#[test]
fn request_world_state_fork_uses_trace_cutoff_and_remaps_history_identity_without_prepared_receipts(
) {
    let mut connection = Connection::open_in_memory().unwrap();
    migrations::run_migrations(&connection).unwrap();
    let source = source_conversation();
    chat_repository::save_conversation(&mut connection, source.clone()).unwrap();
    conversation_trace_repository::replace_trace(
        &mut connection,
        &ConversationTurnTrace {
            schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
            run_id: "run-source-1".into(),
            conversation_id: source.id.clone(),
            assistant_message_id: "assistant-b".into(),
            terminal_status: ConversationTurnTraceTerminalStatus::Completed,
            terminal_error: None,
            truncated: false,
            items: vec![
                ConversationTurnTraceItem::AssistantNarration {
                    sequence: 0,
                    content: "first".into(),
                    truncated: false,
                },
                ConversationTurnTraceItem::AssistantNarration {
                    sequence: 1,
                    content: "second".into(),
                    truncated: false,
                },
            ],
        },
        30,
        40,
    )
    .unwrap();
    let initial = fork_world_state_snapshot("request-world", 0, "initial");
    let before = fork_world_state_snapshot("request-world", 1, "before-request");
    let after = fork_world_state_snapshot("request-world", 2, "after-first-narration");
    let first_boundary = crate::WorldStateRequestBoundary {
        run_id: "run-source-1".into(),
        assistant_message_id: "assistant-b".into(),
        request_index: 0,
        after_trace_sequence: None,
    };
    let second_boundary = crate::WorldStateRequestBoundary {
        request_index: 1,
        after_trace_sequence: Some(0),
        ..first_boundary.clone()
    };
    for (record, boundary) in [
        (WorldStateRecord::Full(initial.clone()), None),
        (
            WorldStateRecord::Diff(WorldStateDiff::between(&initial, &before).unwrap()),
            Some(&first_boundary),
        ),
        (
            WorldStateRecord::Diff(WorldStateDiff::between(&before, &after).unwrap()),
            Some(&second_boundary),
        ),
    ] {
        world_state_repository::append_record(
            &mut connection,
            &world_state_repository::ConversationWorldStateRecordWrite {
                conversation_id: &source.id,
                epoch_generation: 1,
                base_summary_id: None,
                effective_before_message_id: None,
                request_boundary: boundary,
                model_observed: true,
                record: &record,
                created_at: 30,
            },
        )
        .unwrap();
    }
    let positions = source
        .messages
        .iter()
        .enumerate()
        .map(|(i, m)| (m.id.clone(), i))
        .collect();
    let visible = world_state_records_visible_at_cutoff(
        &connection,
        &source.id,
        &positions,
        &ContextJournalCursor::trace_item("assistant-b", 0),
        &[],
        None,
    )
    .unwrap();
    assert_eq!(visible.len(), 2);
    assert_eq!(visible[1].request_boundary.as_ref(), Some(&first_boundary));
    let plan = build_assistant_reply_fork_plan(
        &connection,
        "fork-request-world",
        &source.id,
        "assistant-b",
        100,
    )
    .unwrap();
    commit_fork_plan(&mut connection, &plan).unwrap();
    let target =
        world_state_repository::list_active_journal_entries(&connection, &plan.target.id).unwrap();
    assert_eq!(target.len(), 3);
    let boundary = target[2].request_boundary.as_ref().unwrap();
    assert_eq!(
        boundary.assistant_message_id,
        plan.message_id_map["assistant-b"]
    );
    assert_ne!(boundary.run_id, second_boundary.run_id);
    assert_eq!(boundary.request_index, 1);
    assert_eq!(boundary.after_trace_sequence, Some(0));
    assert_eq!(connection.query_row("SELECT COUNT(*) FROM conversation_world_state_request_commits WHERE conversation_id=?1", [&plan.target.id], |row| row.get::<_, i64>(0)).unwrap(),0);
    assert_eq!(connection.query_row("SELECT COUNT(*) FROM agent_effective_permission_snapshots WHERE conversation_id=?1", [&plan.target.id], |row| row.get::<_, i64>(0)).unwrap(),0);
}
