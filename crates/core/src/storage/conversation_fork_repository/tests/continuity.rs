#[test]
fn fork_clones_applied_capacity_compaction_receipt_and_keeps_it_recursive() {
    let mut connection = Connection::open_in_memory().unwrap();
    migrations::run_migrations(&connection).unwrap();
    let source = source_conversation();
    chat_repository::save_conversation(&mut connection, source.clone()).unwrap();

    let visible_prefix = context_compaction_repository::prepare_prefix(
        &connection,
        &source.id,
        &ContextJournalCursor::message("user-b"),
    )
    .unwrap();
    let visible_receipt = record_applied_capacity_compaction(
        &mut connection,
        &visible_prefix,
        "summary-visible-receipt",
        "context-compaction-visible",
        "run-source-1",
        "assistant-b",
        40,
    );
    let after_cutoff_prefix = context_compaction_repository::prepare_prefix(
        &connection,
        &source.id,
        &ContextJournalCursor::message("user-d"),
    )
    .unwrap();
    let after_cutoff_receipt = record_applied_capacity_compaction(
        &mut connection,
        &after_cutoff_prefix,
        "summary-after-receipt-cutoff",
        "context-compaction-after-cutoff",
        "run-source-3",
        "assistant-d",
        80,
    );
    assert_eq!(
        context_compaction_receipt_repository::list_receipts_for_conversation(
            &connection,
            &source.id,
        )
        .unwrap()
        .len(),
        2
    );

    let first = build_assistant_reply_fork_plan(
        &connection,
        "fork-capacity-receipt",
        &source.id,
        "assistant-c",
        100,
    )
    .unwrap();
    assert_eq!(first.compaction_receipts.len(), 1);
    assert_eq!(
        first.compaction_receipts[0].source_receipt.operation_id,
        visible_receipt.operation_id
    );
    assert_ne!(
        first.compaction_receipts[0].source_receipt.operation_id,
        after_cutoff_receipt.operation_id
    );
    commit_fork_plan(&mut connection, &first).unwrap();

    let first_chain =
        context_compaction_repository::list_active_summary_chain(&connection, &first.target.id)
            .unwrap();
    assert_eq!(first_chain.len(), 1);
    assert_eq!(
        first_chain[0].lineage.source_summary_id.as_deref(),
        visible_receipt.summary_id.as_deref()
    );
    let first_receipts = context_compaction_receipt_repository::list_receipts_for_conversation(
        &connection,
        &first.target.id,
    )
    .unwrap();
    assert_eq!(first_receipts.len(), 1);
    let first_receipt = &first_receipts[0];
    assert_cloned_capacity_compaction_receipt(
        &connection,
        &visible_receipt,
        first_receipt,
        &first.target.id,
        &first.message_id_map["assistant-b"],
        &first.message_id_map["user-b"],
        &first_chain[0].summary.id,
        &first.run_id_map["run-source-1"],
    );
    assert_eq!(
        context_compaction_repository::get_active_summary(&connection, &first.target.id)
            .unwrap()
            .unwrap()
            .id,
        first_chain[0].summary.id
    );
    let first_head = connection
        .query_row(
            "SELECT conversation_id, summary_id
                 FROM conversation_context_compaction_heads
                 WHERE conversation_id = ?1",
            [&first.target.id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .unwrap();
    assert_eq!(first_head.0, first.target.id);
    assert_eq!(first_head.1, first_chain[0].summary.id);

    let recursive = build_assistant_reply_fork_plan(
        &connection,
        "fork-capacity-receipt-recursive",
        &first.target.id,
        &first.message_id_map["assistant-c"],
        110,
    )
    .unwrap();
    assert_eq!(recursive.compaction_receipts.len(), 1);
    assert_eq!(
        recursive.compaction_receipts[0].source_receipt.operation_id,
        first_receipt.operation_id
    );
    commit_fork_plan(&mut connection, &recursive).unwrap();

    let recursive_chain =
        context_compaction_repository::list_active_summary_chain(&connection, &recursive.target.id)
            .unwrap();
    assert_eq!(recursive_chain.len(), 1);
    assert_eq!(
        recursive_chain[0].lineage.source_summary_id.as_deref(),
        first_receipt.summary_id.as_deref()
    );
    let recursive_receipts = context_compaction_receipt_repository::list_receipts_for_conversation(
        &connection,
        &recursive.target.id,
    )
    .unwrap();
    assert_eq!(recursive_receipts.len(), 1);
    assert_cloned_capacity_compaction_receipt(
        &connection,
        first_receipt,
        &recursive_receipts[0],
        &recursive.target.id,
        &recursive.message_id_map[&first_receipt.assistant_message_id],
        &recursive.message_id_map[&first.message_id_map["user-b"]],
        &recursive_chain[0].summary.id,
        &recursive.run_id_map[&first_receipt.run_id],
    );
    let recursive_head = connection
        .query_row(
            "SELECT conversation_id, summary_id
                 FROM conversation_context_compaction_heads
                 WHERE conversation_id = ?1",
            [&recursive.target.id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .unwrap();
    assert_eq!(recursive_head.0, recursive.target.id);
    assert_eq!(recursive_head.1, recursive_chain[0].summary.id);
}

#[test]
fn latest_fork_keeps_manual_summary_current_model_and_recursive_boundary_without_usage_copy() {
    use crate::storage::manual_context_compaction_repository as manual;
    use crate::storage::models::ManualContextCompactionOperation;
    let mut connection = Connection::open_in_memory().unwrap();
    migrations::run_migrations(&connection).unwrap();
    let mut source = source_conversation();
    source.model_id = Some("current-model".to_string());
    chat_repository::save_conversation(&mut connection, source.clone()).unwrap();
    let prefix = context_compaction_repository::prepare_prefix(
        &connection,
        &source.id,
        &ContextJournalCursor::message("assistant-d"),
    )
    .unwrap();
    let receipt = record_applied_capacity_compaction(
        &mut connection,
        &prefix,
        "manual-summary-latest",
        "manual-latest",
        "manual-latest-request",
        "assistant-d",
        90,
    );
    let operation = ManualContextCompactionOperation {
        operation_id: receipt.operation_id.clone(),
        request_id: receipt.operation_id.clone(),
        conversation_id: source.id.clone(),
        status: "completed".into(),
        phase: "committing".into(),
        assistant_message_id: Some("assistant-d".into()),
        covered_through_message_id: Some("assistant-d".into()),
        model_id: source.model_id.clone(),
        summary_id: receipt.summary_id.clone(),
        source_input_tokens: Some(1000),
        replacement_input_tokens: Some(200),
        error: None,
        started_at: 88,
        updated_at: 90,
        completed_at: Some(90),
    };
    manual::insert_cloned_completed(&connection, &operation).unwrap();
    // A historical reply boundary precedes the maintenance operation, despite sharing its anchor.
    let historical = build_assistant_reply_fork_plan(
        &connection,
        "historical-manual",
        &source.id,
        "assistant-d",
        100,
    )
    .unwrap();
    assert!(historical.summaries.is_empty());
    let latest = build_fork_plan_at_point(
        &connection,
        "latest-manual",
        &source.id,
        &ConversationForkPoint::Latest {},
        100,
    )
    .unwrap();
    assert_eq!(latest.target.model_id, source.model_id);
    assert_eq!(latest.target.messages.len(), source.messages.len());
    assert_eq!(latest.summaries.len(), 1);
    commit_fork_plan(&mut connection, &latest).unwrap();
    let copied = manual::list(&connection, &latest.target.id).unwrap();
    assert_eq!(copied.len(), 1);
    assert_ne!(copied[0].operation_id, operation.operation_id);
    assert_eq!(
        copied[0].covered_through_message_id.as_deref(),
        Some(latest.message_id_map["assistant-d"].as_str())
    );
    assert_eq!(connection.query_row("SELECT count(*) FROM manual_context_compaction_usage_records WHERE conversation_id=?1", [&latest.target.id], |row| row.get::<_,i64>(0)).unwrap(), 0);
    let child_head =
        context_compaction_repository::get_active_summary(&connection, &latest.target.id)
            .unwrap()
            .unwrap();
    assert_eq!(child_head.content, "summary manual-summary-latest");
    let recursive_historical = build_assistant_reply_fork_plan(
        &connection,
        "recursive-manual-historical",
        &latest.target.id,
        &latest.message_id_map["assistant-d"],
        110,
    )
    .unwrap();
    assert!(recursive_historical.summaries.is_empty());
    let recursive_latest = build_fork_plan_at_point(
        &connection,
        "recursive-manual-latest",
        &latest.target.id,
        &ConversationForkPoint::Latest {},
        110,
    )
    .unwrap();
    assert_eq!(recursive_latest.summaries.len(), 1);
    commit_fork_plan(&mut connection, &recursive_latest).unwrap();
}

#[test]
fn latest_fork_rechecks_boundary_and_rejects_running_maintenance() {
    use crate::storage::manual_context_compaction_repository as manual;
    use crate::storage::models::ManualContextCompactionOperation;
    let mut connection = Connection::open_in_memory().unwrap();
    migrations::run_migrations(&connection).unwrap();
    let source = source_conversation();
    chat_repository::save_conversation(&mut connection, source.clone()).unwrap();
    let plan = build_fork_plan_at_point(
        &connection,
        "latest-stale",
        &source.id,
        &ConversationForkPoint::Latest {},
        100,
    )
    .unwrap();
    connection
        .execute(
            "UPDATE conversations SET model_id='changed-model' WHERE id=?1",
            [&source.id],
        )
        .unwrap();
    assert!(commit_fork_plan(&mut connection, &plan).is_err());
    let operation = ManualContextCompactionOperation {
        operation_id: "manual-running".into(),
        request_id: "manual-running".into(),
        conversation_id: source.id.clone(),
        status: "running".into(),
        phase: "preparing".into(),
        assistant_message_id: None,
        covered_through_message_id: None,
        model_id: None,
        summary_id: None,
        source_input_tokens: None,
        replacement_input_tokens: None,
        error: None,
        started_at: 100,
        updated_at: 100,
        completed_at: None,
    };
    manual::claim(&mut connection, &operation).unwrap();
    assert!(build_fork_plan_at_point(
        &connection,
        "latest-busy",
        &source.id,
        &ConversationForkPoint::Latest {},
        110
    )
    .is_err());
}
