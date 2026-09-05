fn applied_manual_boundary(
    connection: &mut Connection,
    conversation_id: &str,
    assistant_id: &str,
    operation_id: &str,
    completed_at: i64,
) -> crate::storage::models::ManualContextCompactionOperation {
    let prefix = context_compaction_repository::prepare_prefix(
        connection,
        conversation_id,
        &ContextJournalCursor::message(assistant_id),
    )
    .unwrap();
    let mut receipt = ContextCompactionReceipt::begin_manual_context_compaction(
        operation_id,
        conversation_id,
        assistant_id,
        "model-1",
        "model-1",
        crate::AgentApiStyle::OpenAiCompatible,
        &prefix,
        1000,
        250,
        completed_at - 2,
    )
    .unwrap();
    context_compaction_receipt_repository::record_receipt(connection, &receipt, None).unwrap();
    receipt
        .attach_prepared_prefix(&prefix, completed_at - 1)
        .unwrap();
    let draft = summary_draft(&prefix, &format!("summary-{operation_id}"), completed_at);
    let observation = crate::model_request_observation::ModelRequestObservationBuilder::new(
        format!("observation-{operation_id}"),
        receipt.run_id.clone(),
        Some(conversation_id.to_string()),
        Some(assistant_id.to_string()),
        Some(operation_id.to_string()),
        1,
        crate::ModelRequestPurpose::ContextCompaction,
        "model-1",
        crate::AgentApiStyle::OpenAiCompatible,
        None,
        completed_at - 1,
    )
    .completed(None, Some("stop".into()), completed_at)
    .unwrap();
    receipt
        .complete_applied(&draft, &observation, completed_at)
        .unwrap();
    context_compaction_repository::commit_prefix_replacement_with_receipt(
        connection,
        &prefix,
        draft,
        &receipt,
        &observation,
    )
    .unwrap();
    let operation = crate::storage::models::ManualContextCompactionOperation {
        operation_id: operation_id.into(),
        request_id: operation_id.into(),
        conversation_id: conversation_id.into(),
        status: "completed".into(),
        phase: "committing".into(),
        assistant_message_id: Some(assistant_id.into()),
        covered_through_message_id: Some(assistant_id.into()),
        model_id: Some("model-1".into()),
        summary_id: receipt.summary_id,
        source_input_tokens: Some(1000),
        replacement_input_tokens: Some(200),
        error: None,
        started_at: completed_at - 2,
        updated_at: completed_at,
        completed_at: Some(completed_at),
    };
    crate::storage::manual_context_compaction_repository::insert_cloned_completed(
        connection, &operation,
    )
    .unwrap();
    operation
}

fn manual_boundary_request(operation_id: &str) -> ConversationForkPoint {
    ConversationForkPoint::ManualCompactionBoundary {
        operation_id: operation_id.into(),
    }
}

#[test]
fn manual_boundary_uses_operation_completion_time_for_world_state_cutoff() {
    let mut connection = Connection::open_in_memory().unwrap();
    migrations::run_migrations(&connection).unwrap();
    let source = source_conversation();
    chat_repository::save_conversation(&mut connection, source.clone()).unwrap();
    let visible = fork_world_state_snapshot("world-before-manual-completion", 0, "visible");
    world_state_repository::append_record(
        &mut connection,
        &world_state_repository::ConversationWorldStateRecordWrite {
            conversation_id: &source.id,
            epoch_generation: 1,
            base_summary_id: None,
            effective_before_message_id: None,
            record: &WorldStateRecord::Full(visible.clone()),
            created_at: 42,
        },
    )
    .unwrap();
    let operation = applied_manual_boundary(
        &mut connection,
        &source.id,
        "assistant-b",
        "manual-world-cutoff",
        45,
    );
    let future = fork_world_state_snapshot("world-after-manual-completion", 0, "future");
    world_state_repository::append_record(
        &mut connection,
        &world_state_repository::ConversationWorldStateRecordWrite {
            conversation_id: &source.id,
            epoch_generation: 3,
            base_summary_id: None,
            effective_before_message_id: None,
            record: &WorldStateRecord::Full(future.clone()),
            created_at: 46,
        },
    )
    .unwrap();
    let fork_point = manual_boundary_request(&operation.operation_id);
    assert_eq!(
        authoritative_fork_cutoff_at(&connection, &source.id, &fork_point).unwrap(),
        45
    );
    let plan = build_fork_plan_at_point(
        &connection,
        "fork-manual-world-cutoff",
        &source.id,
        &fork_point,
        100,
    )
    .unwrap();
    assert_eq!(plan.world_state_records.len(), 1);
    assert_eq!(
        plan.world_state_records[0].record.revision(),
        visible.revision
    );
    commit_fork_plan(&mut connection, &plan).unwrap();
    let forked_world = world_state_repository::fold_active_snapshot(&connection, &plan.target.id)
        .unwrap()
        .unwrap();
    assert_eq!(forked_world.revision, visible.revision);
    assert_ne!(forked_world.revision, future.revision);
}

#[test]
fn manual_boundary_fork_preserves_exact_history_model_summary_and_recursive_identity() {
    let mut connection = Connection::open_in_memory().unwrap();
    migrations::run_migrations(&connection).unwrap();
    let source = source_conversation();
    chat_repository::save_conversation(&mut connection, source.clone()).unwrap();
    let first = applied_manual_boundary(
        &mut connection,
        &source.id,
        "assistant-b",
        "manual-first",
        45,
    );
    let second = applied_manual_boundary(
        &mut connection,
        &source.id,
        "assistant-d",
        "manual-second",
        85,
    );
    crate::storage::manual_context_compaction_repository::record_usage(
        &connection,
        &serde_json::from_value(serde_json::json!({
            "operationId": first.operation_id, "conversationId": source.id,
            "modelId": "model-1", "modelName": "Model 1", "createdAt": 45,
            "inputTokens": 1000, "outputTokens": 200, "totalTokens": 1200,
            "billableRequestCount": 1, "estimatedCost": 0.12,
        }))
        .unwrap(),
    )
    .unwrap();
    connection
        .execute(
            "UPDATE conversations SET model_id='new-current-model' WHERE id=?1",
            [&source.id],
        )
        .unwrap();
    let fork = build_fork_plan_at_point(
        &connection,
        "manual-boundary-first",
        &source.id,
        &manual_boundary_request(&first.operation_id),
        100,
    )
    .unwrap();
    assert_eq!(fork.target.messages.len(), 4);
    assert_eq!(fork.target.model_id.as_deref(), Some("model-1"));
    assert_eq!(fork.summaries.len(), 1);
    assert_eq!(
        fork.summaries[0].summary.id,
        first.summary_id.clone().unwrap()
    );
    assert_eq!(fork.compaction_receipts.len(), 1);
    commit_fork_plan(&mut connection, &fork).unwrap();
    let copied =
        crate::storage::manual_context_compaction_repository::list(&connection, &fork.target.id)
            .unwrap();
    assert_eq!(copied.len(), 1);
    assert!(!copied[0].operation_id.starts_with("manual-"));
    assert_eq!(copied[0].completed_at, first.completed_at);
    let recursive = build_fork_plan_at_point(
        &connection,
        "manual-boundary-recursive",
        &fork.target.id,
        &manual_boundary_request(&copied[0].operation_id),
        110,
    )
    .unwrap();
    assert_eq!(recursive.target.messages.len(), 4);
    assert_eq!(recursive.summaries.len(), 1);
    commit_fork_plan(&mut connection, &recursive).unwrap();
    let newest = build_fork_plan_at_point(
        &connection,
        "manual-boundary-second",
        &source.id,
        &manual_boundary_request(&second.operation_id),
        120,
    )
    .unwrap();
    assert_eq!(newest.target.messages.len(), 8);
    assert_eq!(newest.summaries.len(), 2);
    let latest = build_fork_plan_at_point(
        &connection,
        "latest-after-two-manual",
        &source.id,
        &ConversationForkPoint::Latest {},
        130,
    )
    .unwrap();
    assert_eq!(latest.target.model_id.as_deref(), Some("new-current-model"));
    assert_eq!(latest.target.messages.len(), 8);
    let retained_usage: (u64,u64,f64) = connection.query_row(
        "SELECT COUNT(*), SUM(input_tokens), SUM(estimated_cost) FROM manual_context_compaction_usage_records", [],
        |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?)),
    ).unwrap();
    assert_eq!(retained_usage, (1, 1000, 0.12));
    assert_eq!(connection.query_row("SELECT COUNT(*) FROM manual_context_compaction_usage_records WHERE conversation_id != ?1", [&source.id], |row| row.get::<_,u64>(0)).unwrap(), 0);
}

#[test]
fn manual_boundary_rejects_missing_noncompleted_cross_conversation_and_inconsistent_records() {
    let mut connection = Connection::open_in_memory().unwrap();
    migrations::run_migrations(&connection).unwrap();
    let source = source_conversation();
    chat_repository::save_conversation(&mut connection, source.clone()).unwrap();
    let mut operation = applied_manual_boundary(
        &mut connection,
        &source.id,
        "assistant-b",
        "manual-invalid",
        45,
    );
    assert!(build_fork_plan_at_point(
        &connection,
        "missing-boundary",
        &source.id,
        &manual_boundary_request("missing"),
        100
    )
    .is_err());
    let mut other = source.clone();
    other.id = "other-conversation".into();
    other.messages.clear();
    chat_repository::save_conversation(&mut connection, other.clone()).unwrap();
    assert!(build_fork_plan_at_point(
        &connection,
        "cross-boundary",
        &other.id,
        &manual_boundary_request(&operation.operation_id),
        100
    )
    .is_err());
    for status in ["running", "noop", "failed", "cancelled", "interrupted"] {
        operation.status = status.into();
        connection.execute("UPDATE manual_context_compaction_operations SET status=?1,operation_json=?2 WHERE operation_id=?3", params![status,serde_json::to_string(&operation).unwrap(),operation.operation_id]).unwrap();
        assert!(build_fork_plan_at_point(
            &connection,
            "not-completed",
            &source.id,
            &manual_boundary_request(&operation.operation_id),
            100
        )
        .is_err());
    }
    operation.status = "completed".into();
    operation.model_id = Some("tampered-model".into());
    connection.execute("UPDATE manual_context_compaction_operations SET status='completed',operation_json=?1 WHERE operation_id=?2", params![serde_json::to_string(&operation).unwrap(),operation.operation_id]).unwrap();
    assert!(build_fork_plan_at_point(
        &connection,
        "wrong-model",
        &source.id,
        &manual_boundary_request(&operation.operation_id),
        100
    )
    .is_err());
    operation.model_id = Some("model-1".into());
    operation.covered_through_message_id = Some("assistant-d".into());
    connection.execute("UPDATE manual_context_compaction_operations SET operation_json=?1 WHERE operation_id=?2", params![serde_json::to_string(&operation).unwrap(),operation.operation_id]).unwrap();
    assert!(build_fork_plan_at_point(
        &connection,
        "wrong-cursor",
        &source.id,
        &manual_boundary_request(&operation.operation_id),
        100
    )
    .is_err());
}

#[test]
fn manual_boundary_rechecks_active_chain_at_commit_without_falling_back_to_latest() {
    let mut connection = Connection::open_in_memory().unwrap();
    migrations::run_migrations(&connection).unwrap();
    let source = source_conversation();
    chat_repository::save_conversation(&mut connection, source.clone()).unwrap();
    let operation = applied_manual_boundary(
        &mut connection,
        &source.id,
        "assistant-b",
        "manual-stale-boundary",
        45,
    );
    let plan = build_fork_plan_at_point(
        &connection,
        "manual-stale-commit",
        &source.id,
        &manual_boundary_request(&operation.operation_id),
        100,
    )
    .unwrap();
    context_compaction_repository::rollback_active_summary(
        &mut connection,
        &source.id,
        operation.summary_id.as_deref().unwrap(),
        101,
    )
    .unwrap();
    assert!(commit_fork_plan(&mut connection, &plan).is_err());
    assert!(build_fork_plan_at_point(
        &connection,
        "manual-off-chain",
        &source.id,
        &manual_boundary_request(&operation.operation_id),
        102
    )
    .is_err());
    assert!(
        chat_repository::get_conversation(&connection, &plan.target.id)
            .unwrap()
            .is_none()
    );
}

#[test]
fn manual_boundary_does_not_inherit_a_later_provider_adaptation_requirement() {
    let mut connection = Connection::open_in_memory().unwrap();
    migrations::run_migrations(&connection).unwrap();
    let source = source_conversation();
    chat_repository::save_conversation(&mut connection, source.clone()).unwrap();
    let manual = applied_manual_boundary(
        &mut connection,
        &source.id,
        "assistant-b",
        "manual-before-transition",
        45,
    );
    let later = applied_manual_boundary(
        &mut connection,
        &source.id,
        "assistant-d",
        "later-summary",
        85,
    );
    conversation_context_adaptation_repository::insert_in_connection(
        &connection,
        &conversation_context_adaptation_repository::ConversationContextAdaptationRequirement {
            conversation_id: source.id.clone(),
            reason: conversation_context_adaptation_repository::FORK_RELEASED_PROVIDER_STATE_REASON
                .into(),
            source_conversation_id: "original-provider-source".into(),
            source_message_id: "original-provider-message".into(),
            created_at: 80,
            resolved_summary_id: later.summary_id,
            resolved_at: Some(85),
        },
    )
    .unwrap();
    let plan = build_fork_plan_at_point(
        &connection,
        "manual-before-later-adaptation",
        &source.id,
        &manual_boundary_request(&manual.operation_id),
        100,
    )
    .unwrap();
    assert!(
        !plan.requires_context_adaptation,
        "a validated manual summary covers this entire selected historical prefix"
    );
    commit_fork_plan(&mut connection, &plan).unwrap();
    assert!(
        conversation_context_adaptation_repository::get(&connection, &plan.target.id)
            .unwrap()
            .is_none_or(|requirement| !requirement.is_required())
    );
}
