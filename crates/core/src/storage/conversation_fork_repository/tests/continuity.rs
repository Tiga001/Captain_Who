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
