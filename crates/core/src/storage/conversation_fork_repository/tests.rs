use super::*;
use crate::context::{
    ContextCompactionGeneration, ContextCompactionSummaryDraft, ContextContinuitySnapshot,
};
use crate::storage::migrations;
use crate::{
    AgentApprovalStatus, ConversationTraceToolResultStatus, ConversationTurnTraceItem,
    ConversationTurnTraceTerminalStatus, WorldStateDiff, WorldStateLifetime,
    WorldStateSectionEnvelope, WorldStateSectionId, WorldStateSnapshot,
    CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
};

fn build_assistant_reply_fork_plan(
    connection: &Connection,
    request_id: &str,
    source_conversation_id: &str,
    assistant_message_id: &str,
    created_at: i64,
) -> Result<ConversationForkPlan, ConversationForkError> {
    build_fork_plan_at_point(
        connection,
        request_id,
        source_conversation_id,
        &ConversationForkPoint::AssistantReply {
            assistant_message_id: assistant_message_id.to_string(),
        },
        created_at,
    )
}

#[test]
fn cloned_agent_usage_is_zero_and_ids_are_rewritten() {
    let replacements = HashMap::from([
        ("run-old".to_string(), "run-new".to_string()),
        ("message-old".to_string(), "message-new".to_string()),
    ]);
    let cloned = clone_agent_run_json(
        &json!({
            "runId": "run-old",
            "assistantMessageId": "message-old",
            "usage": { "inputTokens": 10, "totalTokens": 12 },
            "messageStreamCheckpoints": { "1": "partial" },
            "state": { "activeRunId": "run-old", "status": "completed" }
        })
        .to_string(),
        &replacements,
    )
    .unwrap();
    let value: Value = serde_json::from_str(&cloned).unwrap();
    assert_eq!(value["runId"], "run-new");
    assert_eq!(value["assistantMessageId"], "message-new");
    assert_eq!(value["usage"]["inputTokens"], 0);
    assert_eq!(value["usage"]["billableRequestCount"], 0);
    assert_eq!(value["state"]["activeRunId"], Value::Null);
    assert_eq!(value["messageStreamCheckpoints"], json!({}));
}

#[test]
fn unsettled_assistant_state_cannot_enter_a_fork_snapshot() {
    let mut message = ChatMessageRecord {
        id: "assistant-running".to_string(),
        role: "assistant".to_string(),
        content: "partial".to_string(),
        created_at: 1,
        status: Some("sent".to_string()),
        attachments: Vec::new(),
        agent_run_json: Some(json!({ "status": "waiting_for_approval" }).to_string()),
        ui_state_json: None,
    };
    assert!(ensure_settled_assistant(&message, None).is_err());

    message.agent_run_json = Some(json!({ "status": "completed" }).to_string());
    assert!(ensure_settled_assistant(&message, None).is_ok());

    let stale_trace = ConversationTurnTrace {
        schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
        run_id: "run-stale".to_string(),
        conversation_id: "conversation-stale".to_string(),
        assistant_message_id: message.id.clone(),
        terminal_status: ConversationTurnTraceTerminalStatus::InProgress,
        terminal_error: None,
        truncated: false,
        items: Vec::new(),
    };
    assert!(
        ensure_settled_assistant(&message, Some(&stale_trace)).is_err(),
        "a renderer-owned completed status must not bypass an in-progress backend trace"
    );
}

#[test]
fn fork_with_released_provider_history_is_marked_for_safe_adaptation() {
    let mut connection = Connection::open_in_memory().unwrap();
    migrations::run_migrations(&connection).unwrap();
    let source = source_conversation();
    chat_repository::save_conversation(&mut connection, source.clone()).unwrap();
    let record = provider_continuation_repository::ProviderContinuationEnvelopeRecord {
        continuation_id: format!(
            "{}{}",
            provider_continuation_repository::PROVIDER_CONTINUATION_REF_PREFIX,
            uuid::Uuid::new_v4().hyphenated()
        ),
        conversation_id: source.id.clone(),
        assistant_message_id: "assistant-a".to_string(),
        run_id: "run-source-0".to_string(),
        request_index: 0,
        assistant_turn_id: format!("at1_{}", "a".repeat(64)),
        assistant_turn_digest: format!("sha256:{}", "b".repeat(64)),
        provider_protocol_digest: format!("sha256:{}", "c".repeat(64)),
        payload_digest: format!("sha256:{}", "d".repeat(64)),
        nonce: vec![1; 12],
        ciphertext: vec![2; 17],
        decoded_bytes: 1,
        compressed_bytes: 1,
        created_at: 2,
        runtime_tool_calls: vec![
            provider_continuation_repository::ProviderContinuationRuntimeToolIdentity {
                provider_tool_index: 0,
                runtime_call_id: "call-source-a".to_string(),
            },
        ],
    };
    provider_continuation_repository::store_active_in_connection(&connection, &record).unwrap();
    assert_eq!(
        provider_continuation_repository::release(&connection, &record.continuation_id, 10)
            .unwrap(),
        provider_continuation_repository::ProviderContinuationReleaseOutcome::Released
    );

    let plan = build_assistant_reply_fork_plan(
        &connection,
        "fork-released-provider-history",
        &source.id,
        "assistant-a",
        20,
    )
    .unwrap();
    assert!(plan.requires_context_adaptation);
    assert!(plan.provider_continuation_mappings.is_empty());
    commit_fork_plan(&mut connection, &plan).unwrap();
    assert!(
        conversation_context_adaptation_repository::get(&connection, &plan.target.id,)
            .unwrap()
            .is_some()
    );

    let recursive = build_assistant_reply_fork_plan(
        &connection,
        "fork-released-provider-history-recursive",
        &plan.target.id,
        &plan.message_id_map["assistant-a"],
        21,
    )
    .unwrap();
    assert!(recursive.requires_context_adaptation);
    assert!(recursive.provider_continuation_mappings.is_empty());
}

#[test]
fn fork_omits_released_turn_only_when_the_selected_summary_covers_its_complete_exchange() {
    let mut connection = Connection::open_in_memory().unwrap();
    migrations::run_migrations(&connection).unwrap();
    let source = source_conversation();
    chat_repository::save_conversation(&mut connection, source.clone()).unwrap();
    let trace = ConversationTurnTrace {
        schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
        run_id: "run-source-0".to_string(),
        conversation_id: source.id.clone(),
        assistant_message_id: "assistant-a".to_string(),
        terminal_status: ConversationTurnTraceTerminalStatus::Completed,
        terminal_error: None,
        truncated: false,
        items: vec![
            ConversationTurnTraceItem::ToolCall {
                sequence: 0,
                call_id: "call-source-a".to_string(),
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
                call_id: "call-source-a".to_string(),
                tool: "web_fetch".to_string(),
                status: ConversationTraceToolResultStatus::Succeeded,
                success: true,
                observation: json!({ "content": "complete" }),
                approval_status: AgentApprovalStatus::NotRequired,
                error: None,
                truncated: false,
                archive: Default::default(),
            },
        ],
    };
    conversation_trace_repository::replace_trace(&mut connection, &trace, 2, 3).unwrap();
    let record = provider_continuation_repository::ProviderContinuationEnvelopeRecord {
        continuation_id: format!(
            "{}{}",
            provider_continuation_repository::PROVIDER_CONTINUATION_REF_PREFIX,
            uuid::Uuid::new_v4().hyphenated()
        ),
        conversation_id: source.id.clone(),
        assistant_message_id: "assistant-a".to_string(),
        run_id: "run-source-0".to_string(),
        request_index: 0,
        assistant_turn_id: format!("at1_{}", "a".repeat(64)),
        assistant_turn_digest: format!("sha256:{}", "b".repeat(64)),
        provider_protocol_digest: format!("sha256:{}", "c".repeat(64)),
        payload_digest: format!("sha256:{}", "d".repeat(64)),
        nonce: vec![1; 12],
        ciphertext: vec![2; 17],
        decoded_bytes: 1,
        compressed_bytes: 1,
        created_at: 2,
        runtime_tool_calls: vec![
            provider_continuation_repository::ProviderContinuationRuntimeToolIdentity {
                provider_tool_index: 0,
                runtime_call_id: "call-source-a".to_string(),
            },
        ],
    };
    provider_continuation_repository::store_active_in_connection(&connection, &record).unwrap();
    let prefix = context_compaction_repository::prepare_prefix(
        &connection,
        &source.id,
        &ContextJournalCursor::trace_item("assistant-a", 1),
    )
    .unwrap();
    context_compaction_repository::commit_prefix_replacement(
        &mut connection,
        &prefix,
        summary_draft(&prefix, "summary-covers-provider-a", 20),
        "assistant-b",
    )
    .unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT state FROM provider_continuations WHERE continuation_id = ?1",
                [&record.continuation_id],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
        "released"
    );

    let before_summary = build_assistant_reply_fork_plan(
        &connection,
        "fork-before-provider-summary",
        &source.id,
        "assistant-a",
        30,
    )
    .unwrap();
    assert!(before_summary.requires_context_adaptation);
    assert!(before_summary.summaries.is_empty());
    assert!(before_summary.provider_continuation_mappings.is_empty());

    let after_summary = build_assistant_reply_fork_plan(
        &connection,
        "fork-after-provider-summary",
        &source.id,
        "assistant-b",
        40,
    )
    .unwrap();
    assert_eq!(after_summary.summaries.len(), 1);
    assert!(!after_summary.requires_context_adaptation);
    assert!(after_summary.provider_continuation_mappings.is_empty());
    commit_fork_plan(&mut connection, &after_summary).unwrap();
    assert_eq!(
        context_compaction_repository::list_active_summary_chain(
            &connection,
            &after_summary.target.id,
        )
        .unwrap()
        .len(),
        1
    );
}

#[test]
fn explicit_timeline_points_separate_reply_from_transition_and_pin_boundary_model() {
    let mut connection = Connection::open_in_memory().unwrap();
    migrations::run_migrations(&connection).unwrap();
    let mut source = source_conversation();
    source.model_id = Some("model-c".to_string());
    chat_repository::save_conversation(&mut connection, source.clone()).unwrap();
    record_agent_loop_model(
        &connection,
        &source.id,
        "assistant-a",
        "run-source-0",
        "model-a",
        2,
    );

    let first_prefix = context_compaction_repository::prepare_prefix(
        &connection,
        &source.id,
        &ContextJournalCursor::message("assistant-a"),
    )
    .unwrap();
    record_provider_transition_summary(
        &mut connection,
        &first_prefix,
        "summary-transition-b",
        "provider-transition-a-to-b",
        "assistant-a",
        "model-b",
        20,
    );
    let second_prefix = context_compaction_repository::prepare_prefix(
        &connection,
        &source.id,
        &ContextJournalCursor::message("assistant-b"),
    )
    .unwrap();
    record_provider_transition_summary(
        &mut connection,
        &second_prefix,
        "summary-transition-c",
        "provider-transition-b-to-c",
        "assistant-b",
        "model-c",
        30,
    );

    let before_divider = build_fork_plan_at_point(
        &connection,
        "fork-before-divider",
        &source.id,
        &ConversationForkPoint::AssistantReply {
            assistant_message_id: "assistant-a".to_string(),
        },
        40,
    )
    .unwrap();
    assert!(before_divider.summaries.is_empty());
    assert!(before_divider.provider_transition_receipts.is_empty());
    assert_eq!(before_divider.target.model_id.as_deref(), Some("model-a"));

    let after_first_divider = build_fork_plan_at_point(
        &connection,
        "fork-after-first-divider",
        &source.id,
        &ConversationForkPoint::AssistantReply {
            assistant_message_id: "assistant-b".to_string(),
        },
        40,
    )
    .unwrap();
    assert_eq!(after_first_divider.summaries.len(), 1);
    assert_eq!(after_first_divider.provider_transition_receipts.len(), 1);

    let at_second_divider = build_fork_plan_at_point(
        &connection,
        "fork-at-second-divider",
        &source.id,
        &ConversationForkPoint::ProviderTransitionBoundary {
            operation_id: "provider-transition-b-to-c".to_string(),
        },
        40,
    )
    .unwrap();
    assert_eq!(at_second_divider.summaries.len(), 2);
    assert_eq!(at_second_divider.provider_transition_receipts.len(), 2);

    let at_first_divider = build_fork_plan_at_point(
        &connection,
        "fork-at-first-divider",
        &source.id,
        &ConversationForkPoint::ProviderTransitionBoundary {
            operation_id: "provider-transition-a-to-b".to_string(),
        },
        41,
    )
    .unwrap();
    assert_eq!(at_first_divider.summaries.len(), 1);
    assert_eq!(
        at_first_divider.summaries[0].summary.id,
        "summary-transition-b"
    );
    assert_eq!(at_first_divider.target.model_id.as_deref(), Some("model-b"));
    assert_eq!(at_first_divider.target.messages.len(), 2);
    assert_eq!(at_first_divider.provider_transition_receipts.len(), 1);

    commit_fork_plan(&mut connection, &at_first_divider).unwrap();
    let target_chain = context_compaction_repository::list_active_summary_chain(
        &connection,
        &at_first_divider.target.id,
    )
    .unwrap();
    assert_eq!(target_chain.len(), 1);
    assert_eq!(
        target_chain[0].lineage.source_summary_id.as_deref(),
        Some("summary-transition-b")
    );
    let cloned_receipts = context_compaction_receipt_repository::list_provider_transition_receipts(
        &connection,
        &at_first_divider.target.id,
        None,
        100,
    )
    .unwrap();
    assert_eq!(cloned_receipts.len(), 1);
    let cloned_receipt = &cloned_receipts[0];
    assert_ne!(cloned_receipt.operation_id, "provider-transition-a-to-b");
    assert_eq!(cloned_receipt.conversation_id, at_first_divider.target.id);
    assert_eq!(
        cloned_receipt.assistant_message_id,
        at_first_divider.message_id_map["assistant-a"]
    );
    assert_eq!(
        cloned_receipt.summary_id.as_deref(),
        Some(target_chain[0].summary.id.as_str())
    );
    assert_eq!(
        cloned_receipt.plan.covered_through,
        ContextJournalCursor::message(&at_first_divider.message_id_map["assistant-a"])
    );
    let cloned_observation = model_request_observation_repository::get_observation(
        &connection,
        cloned_receipt
            .generation_observation_id
            .as_deref()
            .expect("cloned receipt observation"),
    )
    .unwrap()
    .expect("cloned provider transition observation");
    cloned_receipt
        .validate_generation_observation(&cloned_observation)
        .unwrap();
    let continuation_origin = get_continuation_origin(&connection, &at_first_divider.target.id)
        .unwrap()
        .expect("fork continuation origin");
    assert_eq!(
        continuation_origin.boundary_message_id,
        at_first_divider.message_id_map["assistant-a"]
    );
    assert!(conversation_context_adaptation_repository::get(
        &connection,
        &at_first_divider.target.id,
    )
    .unwrap()
    .is_none());

    let recursive = build_fork_plan_at_point(
        &connection,
        "fork-recursive-provider-divider",
        &at_first_divider.target.id,
        &ConversationForkPoint::ProviderTransitionBoundary {
            operation_id: cloned_receipt.operation_id.clone(),
        },
        50,
    )
    .unwrap();
    assert_eq!(recursive.summaries.len(), 1);
    assert_eq!(recursive.provider_transition_receipts.len(), 1);
    assert_eq!(recursive.target.model_id.as_deref(), Some("model-b"));
    commit_fork_plan(&mut connection, &recursive).unwrap();
    assert_eq!(
        context_compaction_receipt_repository::list_provider_transition_receipts(
            &connection,
            &recursive.target.id,
            None,
            100,
        )
        .unwrap()
        .len(),
        1
    );
}

#[test]
fn fork_clones_only_causally_visible_summary_history_and_remains_recursive() {
    let mut connection = Connection::open_in_memory().unwrap();
    migrations::run_migrations(&connection).unwrap();
    let source = source_conversation();
    chat_repository::save_conversation(&mut connection, source.clone()).unwrap();
    for (index, message) in source
        .messages
        .iter()
        .filter(|message| message.role == "assistant")
        .enumerate()
    {
        let trace = ConversationTurnTrace {
            schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
            run_id: format!("run-source-{index}"),
            conversation_id: source.id.clone(),
            assistant_message_id: message.id.clone(),
            terminal_status: ConversationTurnTraceTerminalStatus::Completed,
            terminal_error: None,
            truncated: false,
            items: vec![ConversationTurnTraceItem::AssistantNarration {
                sequence: 0,
                content: format!("narration {index}"),
                truncated: false,
            }],
        };
        conversation_trace_repository::replace_trace(
            &mut connection,
            &trace,
            message.created_at,
            message.created_at + 1,
        )
        .unwrap();
    }
    file_draft_repository::insert_draft(
        &connection,
        &AgentFileDraftRecord {
            id: "draft-source".to_string(),
            conversation_id: source.id.clone(),
            project_id: None,
            run_id: "run-source-1".to_string(),
            file_path: "notes.md".to_string(),
            mode: "update".to_string(),
            status: "applied".to_string(),
            base_revision: Some("revision-old".to_string()),
            base_content: "before\n".to_string(),
            content: "after\n".to_string(),
            additions: 1,
            deletions: 1,
            line_count: 1,
            byte_count: 6,
            chunk_count: 1,
            next_chunk_index: 1,
            stats_final: true,
            summary: Some("updated notes".to_string()),
            final_action_id: None,
            created_at: 4,
            updated_at: 4,
            expires_at: i64::MAX,
        },
    )
    .unwrap();

    connection
        .execute(
            "INSERT INTO agent_file_draft_chunks (
                     draft_id, chunk_index, content_hash, byte_count, created_at
                 ) VALUES ('draft-source', 0, 'sha256:draft-chunk', 6, 4)",
            [],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO agent_file_draft_operations (
                     draft_id, sequence, operation, payload_hash, created_at
                 ) VALUES ('draft-source', 0, 'append', 'sha256:draft-operation', 4)",
            [],
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
        summary_draft(&first_prefix, "summary-1", 20),
        "assistant-b",
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
        summary_draft(&second_prefix, "summary-2", 40),
        "assistant-d",
    )
    .unwrap();

    let plan =
        build_assistant_reply_fork_plan(&connection, "fork-at-c", &source.id, "assistant-c", 100)
            .unwrap();
    assert_eq!(plan.target.messages.len(), 6);
    assert_eq!(plan.summaries.len(), 1);
    assert_eq!(plan.file_drafts.len(), 1);
    assert_ne!(plan.file_drafts[0].id, "draft-source");
    commit_fork_plan(&mut connection, &plan).unwrap();
    for table in ["agent_file_draft_chunks", "agent_file_draft_operations"] {
        assert_eq!(
            connection
                .query_row(
                    &format!("SELECT COUNT(*) FROM {table} WHERE draft_id = ?1"),
                    [&plan.file_drafts[0].id],
                    |row| row.get::<_, u64>(0),
                )
                .unwrap(),
            1
        );
    }

    let target_chain =
        context_compaction_repository::list_active_summary_chain(&connection, &plan.target.id)
            .unwrap();
    assert_eq!(target_chain.len(), 1);
    assert_eq!(
        target_chain[0].lineage.source_summary_id.as_deref(),
        Some("summary-1")
    );
    assert_eq!(
        target_chain[0].lineage.introduced_by_assistant_message_id,
        plan.message_id_map["assistant-b"]
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM context_compaction_receipts WHERE conversation_id = ?1",
                [&plan.target.id],
                |row| row.get::<_, u64>(0),
            )
            .unwrap(),
        0
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM model_request_observations WHERE conversation_id = ?1",
                [&plan.target.id],
                |row| row.get::<_, u64>(0),
            )
            .unwrap(),
        0
    );
    for message in &plan.target.messages {
        if message.role != "assistant" {
            continue;
        }
        let run: Value = serde_json::from_str(message.agent_run_json.as_deref().unwrap()).unwrap();
        for field in [
            "inputTokens",
            "outputTokens",
            "outputThinkingTokens",
            "totalTokens",
            "cachedInputTokens",
            "cacheCreationInputTokens",
            "billableRequestCount",
        ] {
            assert_eq!(run["usage"][field], 0, "usage field {field}");
        }
    }

    chat_repository::delete_conversation(&connection, &source.id).unwrap();
    assert!(
        chat_repository::get_conversation(&connection, &plan.target.id)
            .unwrap()
            .is_some()
    );
    assert_eq!(
        context_compaction_repository::list_active_summary_chain(&connection, &plan.target.id,)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        file_draft_repository::list_drafts_for_run(&connection, &plan.run_id_map["run-source-1"],)
            .unwrap()
            .len(),
        1
    );

    let before_summary = build_assistant_reply_fork_plan(
        &connection,
        "recursive-before-summary",
        &plan.target.id,
        &plan.message_id_map["assistant-a"],
        110,
    )
    .unwrap();
    assert!(before_summary.summaries.is_empty());
    commit_fork_plan(&mut connection, &before_summary).unwrap();
    assert!(context_compaction_repository::get_active_summary(
        &connection,
        &before_summary.target.id,
    )
    .unwrap()
    .is_none());

    let after_summary = build_assistant_reply_fork_plan(
        &connection,
        "recursive-after-summary",
        &plan.target.id,
        &plan.message_id_map["assistant-b"],
        120,
    )
    .unwrap();
    assert_eq!(after_summary.summaries.len(), 1);
    commit_fork_plan(&mut connection, &after_summary).unwrap();
    assert_eq!(
        context_compaction_repository::list_active_summary_chain(
            &connection,
            &after_summary.target.id,
        )
        .unwrap()
        .len(),
        1
    );
}

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
    file_draft_repository::insert_draft(
        &connection,
        &AgentFileDraftRecord {
            id: "draft-mutated-after-cutoff".to_string(),
            conversation_id: source.id.clone(),
            project_id: None,
            run_id: "run-source-1".to_string(),
            file_path: "late.md".to_string(),
            mode: "update".to_string(),
            status: "applied".to_string(),
            base_revision: None,
            base_content: "before".to_string(),
            content: "after".to_string(),
            additions: 1,
            deletions: 1,
            line_count: 1,
            byte_count: 5,
            chunk_count: 0,
            next_chunk_index: 0,
            stats_final: true,
            summary: None,
            final_action_id: None,
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
        .contains("文件草稿在分叉点后发生过不可版本化的变化"));
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
fn fork_commit_uses_the_file_draft_history_frozen_in_the_plan() {
    let mut connection = Connection::open_in_memory().unwrap();
    migrations::run_migrations(&connection).unwrap();
    let source = source_conversation();
    chat_repository::save_conversation(&mut connection, source.clone()).unwrap();
    let mut draft = AgentFileDraftRecord {
        id: "draft-plan-snapshot".to_string(),
        conversation_id: source.id.clone(),
        project_id: None,
        run_id: "run-source-1".to_string(),
        file_path: "snapshot.md".to_string(),
        mode: "update".to_string(),
        status: "applied".to_string(),
        base_revision: None,
        base_content: "before".to_string(),
        content: "planned".to_string(),
        additions: 1,
        deletions: 1,
        line_count: 1,
        byte_count: 7,
        chunk_count: 1,
        next_chunk_index: 1,
        stats_final: true,
        summary: Some("planned snapshot".to_string()),
        final_action_id: Some("action-0".to_string()),
        created_at: 30,
        updated_at: 40,
        expires_at: i64::MAX,
    };
    file_draft_repository::insert_draft(&connection, &draft).unwrap();
    file_draft_repository::save_draft_progress(
        &mut connection,
        &draft,
        Some(&crate::storage::models::AgentFileDraftChunkRecord {
            draft_id: draft.id.clone(),
            chunk_index: 0,
            content_hash: "chunk-before-plan".to_string(),
            byte_count: 7,
            created_at: 40,
        }),
        Some(&crate::storage::models::AgentFileDraftOperationRecord {
            draft_id: draft.id.clone(),
            sequence: 0,
            operation: "append".to_string(),
            payload_hash: "operation-before-plan".to_string(),
            created_at: 40,
        }),
    )
    .unwrap();

    let plan = build_assistant_reply_fork_plan(
        &connection,
        "fork-frozen-draft-plan",
        &source.id,
        "assistant-c",
        100,
    )
    .unwrap();
    assert_eq!(plan.file_drafts.len(), 1);
    assert_eq!(plan.file_drafts[0].history.chunks.len(), 1);
    assert_eq!(plan.file_drafts[0].history.operations.len(), 1);

    draft.content = "mutated-after-plan".to_string();
    draft.chunk_count = 2;
    draft.next_chunk_index = 2;
    draft.updated_at = 50;
    file_draft_repository::save_draft_progress(
        &mut connection,
        &draft,
        Some(&crate::storage::models::AgentFileDraftChunkRecord {
            draft_id: draft.id.clone(),
            chunk_index: 1,
            content_hash: "chunk-after-plan".to_string(),
            byte_count: 10,
            created_at: 50,
        }),
        Some(&crate::storage::models::AgentFileDraftOperationRecord {
            draft_id: draft.id.clone(),
            sequence: 1,
            operation: "append".to_string(),
            payload_hash: "operation-after-plan".to_string(),
            created_at: 50,
        }),
    )
    .unwrap();

    commit_fork_plan(&mut connection, &plan).unwrap();
    let target_draft =
        file_draft_repository::get_draft(&connection, &plan.file_drafts[0].target.id)
            .unwrap()
            .unwrap();
    assert_eq!(target_draft.content, "planned");
    assert_eq!(target_draft.chunk_count, 1);
    assert_eq!(target_draft.next_chunk_index, 1);
    assert_eq!(
        file_draft_repository::get_chunk_hash(&connection, &target_draft.id, 0)
            .unwrap()
            .as_deref(),
        Some("chunk-before-plan")
    );
    assert!(
        file_draft_repository::get_chunk_hash(&connection, &target_draft.id, 1)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        file_draft_repository::next_operation_sequence(&connection, &target_draft.id).unwrap(),
        1
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
                "diffs": [],
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
