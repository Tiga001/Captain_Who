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

fn staged_tool_exchange(
    sequence: u64,
    call_id: &str,
    tool: &str,
    args: Value,
) -> Vec<ConversationTurnTraceItem> {
    vec![
        ConversationTurnTraceItem::ToolCall {
            sequence,
            call_id: call_id.to_string(),
            tool: tool.to_string(),
            provenance: crate::AgentToolIdentity::Builtin {
                tool_name: tool.to_string(),
            },
            operation: args,
            approval_status: AgentApprovalStatus::NotRequired,
            truncated: false,
        },
        ConversationTurnTraceItem::ToolResult {
            sequence: sequence + 1,
            call_id: call_id.to_string(),
            tool: tool.to_string(),
            status: ConversationTraceToolResultStatus::Succeeded,
            success: true,
            observation: json!({"accepted": true}),
            approval_status: AgentApprovalStatus::NotRequired,
            error: None,
            truncated: false,
            archive: Default::default(),
        },
    ]
}

fn apply_patch_args(request: Value) -> Value {
    json!({ "request": request })
}

fn staged_mutation_receipt(transaction_id: &str, index: u64) -> String {
    serde_json::to_string(&crate::file_change::FileChangeMutationReceipt {
        schema_version: 1,
        transaction_id: transaction_id.to_string(),
        index,
        draft_revision: index + 1,
        next_index: index + 1,
        byte_count: 6,
        line_count: 1,
        allowed_next_actions: vec![
            crate::file_change::FileChangeStagedAction::Append,
            crate::file_change::FileChangeStagedAction::Edit,
            crate::file_change::FileChangeStagedAction::Commit,
            crate::file_change::FileChangeStagedAction::Status,
            crate::file_change::FileChangeStagedAction::Abort,
        ],
        requires_commit_before_response: true,
    })
    .unwrap()
}

fn file_change_observation(
    conversation_id: &str,
    run_id: &str,
    begin_call_id: &str,
    canonical_target: &str,
    revision: &str,
) -> (String, String) {
    use crate::file_change::{
        FileObservationCheckpoint, FileObservationDirectoryIdentity, FileObservationIdentity,
        FileObservationState, FILE_OBSERVATION_CHECKPOINT_SCHEMA_VERSION, FILE_OBSERVATION_TTL_MS,
    };
    let observation_id = format!("fobs_{}", "1".repeat(32));
    let metadata = std::fs::metadata(std::env::temp_dir()).unwrap();
    let checkpoint = FileObservationCheckpoint {
        schema_version: FILE_OBSERVATION_CHECKPOINT_SCHEMA_VERSION,
        observation_id: observation_id.clone(),
        source_tool_call_id: begin_call_id.to_string(),
        conversation_id: conversation_id.to_string(),
        run_id: run_id.to_string(),
        canonical_target: canonical_target.to_string(),
        state: FileObservationState::Existing {
            revision: revision.to_string(),
            identity: FileObservationIdentity::from_metadata(&metadata),
        },
        parent_directory_identity: FileObservationDirectoryIdentity::read(
            &std::fs::canonicalize(std::env::temp_dir()).unwrap(),
        )
        .unwrap(),
        created_at_ms: 1,
        expires_at_ms: 1 + FILE_OBSERVATION_TTL_MS,
    };
    (observation_id, serde_json::to_string(&checkpoint).unwrap())
}

fn apply_patch_observation(
    conversation_id: &str,
    run_id: &str,
    read_call_id: &str,
    canonical_target: &std::path::Path,
    revision: &str,
) -> (String, String) {
    use crate::file_change::{
        FileObservationCheckpoint, FileObservationDirectoryIdentity, FileObservationIdentity,
        FileObservationState, FILE_OBSERVATION_CHECKPOINT_SCHEMA_VERSION, FILE_OBSERVATION_TTL_MS,
    };
    let observation_id = format!("fobs_{}", "2".repeat(32));
    let target_metadata = std::fs::metadata(canonical_target).unwrap();
    let checkpoint = FileObservationCheckpoint {
        schema_version: FILE_OBSERVATION_CHECKPOINT_SCHEMA_VERSION,
        observation_id: observation_id.clone(),
        source_tool_call_id: read_call_id.to_string(),
        conversation_id: conversation_id.to_string(),
        run_id: run_id.to_string(),
        canonical_target: canonical_target.to_string_lossy().into_owned(),
        state: FileObservationState::Existing {
            revision: revision.to_string(),
            identity: FileObservationIdentity::from_metadata(&target_metadata),
        },
        parent_directory_identity: FileObservationDirectoryIdentity::read(
            canonical_target.parent().unwrap(),
        )
        .unwrap(),
        created_at_ms: 1,
        expires_at_ms: 1 + FILE_OBSERVATION_TTL_MS,
    };
    (observation_id, serde_json::to_string(&checkpoint).unwrap())
}

#[test]
fn cloned_agent_usage_is_zero_and_ids_are_rewritten() {
    let replacements = HashMap::from([
        ("run-old".to_string(), "run-new".to_string()),
        ("message-old".to_string(), "message-new".to_string()),
        ("agent-old".to_string(), "agent-new".to_string()),
        ("turn-old".to_string(), "turn-new".to_string()),
    ]);
    let cloned = clone_agent_run_json(
        &json!({
            "runId": "run-old",
            "assistantMessageId": "message-old",
            "usage": { "inputTokens": 10, "totalTokens": 12 },
            "messageStreamCheckpoints": { "1": "partial" },
            "state": { "activeRunId": "run-old", "status": "completed" },
            "collaborationTimelineActivities": [{
                "activityId": "event-stable",
                "agentId": "agent-old",
                "occurredAt": 10,
                "rootAnchorMessageId": "message-old",
                "rootTraceBoundarySequence": 2,
                "runId": "run-old",
                "semantic": "updated",
                "sequence": 3,
                "taskNameSnapshot": "Reviewer",
                "turnId": "turn-old"
            }]
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
    let activity = &value["collaborationTimelineActivities"][0];
    assert_eq!(activity["activityId"], "event-stable");
    assert_eq!(activity["agentId"], "agent-new");
    assert_eq!(activity["rootAnchorMessageId"], "message-new");
    assert_eq!(activity["runId"], "run-new");
    assert_eq!(activity["turnId"], "turn-new");

    let recursive = clone_agent_run_json(
        &cloned,
        &HashMap::from([
            ("run-new".to_string(), "run-recursive".to_string()),
            ("message-new".to_string(), "message-recursive".to_string()),
            ("agent-new".to_string(), "agent-recursive".to_string()),
            ("turn-new".to_string(), "turn-recursive".to_string()),
        ]),
    )
    .unwrap();
    let recursive: Value = serde_json::from_str(&recursive).unwrap();
    let recursive_activity = &recursive["collaborationTimelineActivities"][0];
    assert_eq!(recursive_activity["agentId"], "agent-recursive");
    assert_eq!(
        recursive_activity["rootAnchorMessageId"],
        "message-recursive"
    );
    assert_eq!(recursive_activity["runId"], "run-recursive");
    assert_eq!(recursive_activity["turnId"], "turn-recursive");
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
fn fork_omits_released_ordinary_turn_only_when_summary_covers_its_message() {
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
        runtime_tool_calls: Vec::new(),
    };
    provider_continuation_repository::store_active_with_projection_in_connection(
        &connection,
        &record,
        provider_continuation_repository::ProviderContinuationProjection::ConversationMessage,
    )
    .unwrap();
    let prefix = context_compaction_repository::prepare_prefix(
        &connection,
        &source.id,
        &ContextJournalCursor::message("assistant-a"),
    )
    .unwrap();
    context_compaction_repository::commit_prefix_replacement(
        &mut connection,
        &prefix,
        summary_draft(&prefix, "summary-covers-ordinary-a", 20),
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
        "fork-before-ordinary-summary",
        &source.id,
        "assistant-a",
        30,
    )
    .unwrap();
    assert!(before_summary.requires_context_adaptation);
    assert!(before_summary.summaries.is_empty());

    let after_summary = build_assistant_reply_fork_plan(
        &connection,
        "fork-after-ordinary-summary",
        &source.id,
        "assistant-b",
        40,
    )
    .unwrap();
    assert_eq!(after_summary.summaries.len(), 1);
    assert!(!after_summary.requires_context_adaptation);
    assert!(after_summary.provider_continuation_mappings.is_empty());
}

#[test]
fn fork_clone_preserves_the_exact_steer_boundary_projection() {
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
        items: vec![ConversationTurnTraceItem::UserGuidance {
            sequence: 0,
            guidance_id: "guidance-fork-boundary".to_string(),
            client_message_id: "client-fork-boundary".to_string(),
            content: "continue privately".to_string(),
            attachments: Vec::new(),
            created_at: 15,
            truncated: false,
        }],
    };
    conversation_trace_repository::commit_trace_in_connection(&connection, &trace, 15, 20).unwrap();
    conversation_model_context_repository::commit_items_in_connection(
        &connection,
        &source.id,
        "assistant-a",
        &[ConversationModelContextItem {
            sequence: 0,
            ordinal: 0,
            role: "user".to_string(),
            content: "continue privately".to_string(),
            tool_call_id: None,
            tool_calls: Vec::new(),
            is_error: false,
        }],
    )
    .unwrap();
    let source_record = provider_continuation_repository::ProviderContinuationEnvelopeRecord {
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
        created_at: 20,
        runtime_tool_calls: Vec::new(),
    };
    provider_continuation_repository::store_active_with_projection_in_connection(
        &connection,
        &source_record,
        provider_continuation_repository::ProviderContinuationProjection::ConversationSteerBoundary {
            guidance_sequence: 0,
        },
    )
    .unwrap();

    let plan = build_assistant_reply_fork_plan(
        &connection,
        "fork-steer-boundary",
        &source.id,
        "assistant-a",
        30,
    )
    .unwrap();
    assert_eq!(plan.provider_continuation_mappings.len(), 1);
    assert_eq!(
        plan.provider_continuation_mappings[0]
            .source_record
            .projection,
        Some(
            provider_continuation_repository::ProviderContinuationProjection::ConversationSteerBoundary {
                guidance_sequence: 0,
            }
        )
    );
    let mapping = &plan.provider_continuation_mappings[0];
    let source_ref =
        ProviderContinuationRef::parse(1, source_record.continuation_id.clone()).unwrap();
    let target_ref = ProviderContinuationRef::new();
    let prepared = PreparedProviderContinuationClone {
        source_ref,
        target_ref: target_ref.clone(),
        record: provider_continuation_repository::ProviderContinuationEnvelopeRecord {
            continuation_id: target_ref.id.clone(),
            conversation_id: mapping.target_conversation_id.clone(),
            assistant_message_id: mapping.target_assistant_message_id.clone(),
            run_id: mapping.target_run_id.clone(),
            request_index: mapping.request_index,
            assistant_turn_id: source_record.assistant_turn_id.clone(),
            assistant_turn_digest: source_record.assistant_turn_digest.clone(),
            provider_protocol_digest: source_record.provider_protocol_digest.clone(),
            payload_digest: source_record.payload_digest.clone(),
            nonce: source_record.nonce.clone(),
            ciphertext: source_record.ciphertext.clone(),
            decoded_bytes: source_record.decoded_bytes,
            compressed_bytes: source_record.compressed_bytes,
            created_at: 30,
            runtime_tool_calls: Vec::new(),
        },
    };
    commit_fork_plan_with_provider_continuations(&mut connection, &plan, &[prepared]).unwrap();

    assert_eq!(
        connection
            .query_row(
                "SELECT projection_kind, projection_sequence, projection_ordinal
                 FROM provider_continuations WHERE continuation_id = ?1",
                [&target_ref.id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, u64>(1)?,
                        row.get::<_, Option<u32>>(2)?,
                    ))
                },
            )
            .unwrap(),
        ("conversation_steer_boundary".to_string(), 0, None)
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
        let mut items = vec![ConversationTurnTraceItem::AssistantNarration {
            sequence: 0,
            content: format!("narration {index}"),
            truncated: false,
        }];
        if index == 1 {
            items.extend(staged_tool_exchange(
                1,
                "call-file-begin",
                "apply_patch",
                apply_patch_args(json!({"action":"begin","operation":"update","strategy":"rewrite","filePath":"notes.md","observationId":format!("fobs_{}", "1".repeat(32))})),
            ));
            items.extend(staged_tool_exchange(
                3,
                "call-file-append",
                "apply_patch",
                apply_patch_args(json!({"action":"append","transactionId":"file-change-source","index":0,"expectedDraftRevision":0,"content":"after\n"})),
            ));
            items.extend(staged_tool_exchange(
                5,
                "call-file-commit",
                "apply_patch",
                apply_patch_args(json!({"action":"commit","transactionId":"file-change-source","expectedDraftRevision":1})),
            ));
        }
        let trace = ConversationTurnTrace {
            schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
            run_id: format!("run-source-{index}"),
            conversation_id: source.id.clone(),
            assistant_message_id: message.id.clone(),
            terminal_status: ConversationTurnTraceTerminalStatus::Completed,
            terminal_error: None,
            truncated: false,
            items,
        };
        conversation_trace_repository::replace_trace(
            &mut connection,
            &trace,
            message.created_at,
            message.created_at + 1,
        )
        .unwrap();
    }
    let begin_args = apply_patch_args(
        json!({"action":"begin","operation":"update","strategy":"rewrite","filePath":"notes.md","observationId":format!("fobs_{}", "1".repeat(32))}),
    );
    let append_args = apply_patch_args(
        json!({"action":"append","transactionId":"file-change-source","index":0,"expectedDraftRevision":0,"content":"after\n"}),
    );
    let base_revision = crate::content_revision(b"before\n");
    let (observation_id, observation_json) = file_change_observation(
        &source.id,
        "run-source-1",
        "call-file-begin",
        "/tmp/notes.md",
        &base_revision,
    );
    file_change_repository::insert_file_change(
        &connection,
        &AgentFileChangeRecord {
            schema_version: 1,
            id: "file-change-source".to_string(),
            conversation_id: source.id.clone(),
            project_id: None,
            run_id: "run-source-1".to_string(),
            source_tool_name: "apply_patch".to_string(),
            source_tool_call_id: "call-file-begin".to_string(),
            source_tool_arguments_digest: crate::file_change::proposal_digest(&begin_args).unwrap(),
            permission_revision: "permission-1".to_string(),
            tool_set_revision: "tool-set-1".to_string(),
            provider_wire_revision: "provider-protocol-v1".to_string(),
            observation_id,
            observation_json,
            file_path: "notes.md".to_string(),
            operation: "update".to_string(),
            strategy: Some("rewrite".to_string()),
            status: "applied".to_string(),
            base_revision: Some(base_revision),
            base_content: "before\n".to_string(),
            content: "after\n".to_string(),
            draft_revision: 1,
            next_mutation_index: 1,
            additions: 1,
            deletions: 1,
            line_count: 1,
            byte_count: 6,
            mutation_count: 1,
            stats_final: true,
            summary: Some("updated notes".to_string()),
            final_action_id: Some("call-file-commit".to_string()),
            final_action_arguments_digest: Some("commit-arguments-digest".to_string()),
            final_permission_revision: Some("permission-1".to_string()),
            final_tool_set_revision: Some("tool-set-1".to_string()),
            final_provider_wire_revision: Some("provider-protocol-v1".to_string()),
            created_at: 4,
            updated_at: 4,
            expires_at: i64::MAX,
        },
    )
    .unwrap();

    connection
        .execute(
            "INSERT INTO agent_file_change_chunks (
                     transaction_id, mutation_index, content_digest, byte_count, created_at
                 ) VALUES ('file-change-source', 0, 'content-digest', 6, 4)",
            [],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO agent_file_change_operations (
                     transaction_id, mutation_index, source_tool_call_id,
                     source_tool_arguments_digest, action, payload_digest,
                     draft_revision, receipt_json, created_at
                 ) VALUES (?1, 0, ?2, ?3, 'append', 'payload-digest', 1, ?4, 4)",
            params![
                "file-change-source",
                "call-file-append",
                crate::file_change::proposal_digest(&append_args).unwrap(),
                staged_mutation_receipt("file-change-source", 0),
            ],
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
    assert_eq!(plan.file_changes.len(), 1);
    assert_ne!(plan.file_changes[0].id, "file-change-source");
    commit_fork_plan(&mut connection, &plan).unwrap();
    for table in ["agent_file_change_chunks", "agent_file_change_operations"] {
        assert_eq!(
            connection
                .query_row(
                    &format!("SELECT COUNT(*) FROM {table} WHERE transaction_id = ?1"),
                    [&plan.file_changes[0].id],
                    |row| row.get::<_, u64>(0),
                )
                .unwrap(),
            1
        );
    }
    let target_change =
        file_change_repository::get_file_change(&connection, &plan.file_changes[0].id)
            .unwrap()
            .unwrap();
    let target_operation = file_change_repository::get_operation(&connection, &target_change.id, 0)
        .unwrap()
        .unwrap();
    assert_ne!(target_change.source_tool_call_id, "call-file-begin");
    assert_ne!(target_operation.source_tool_call_id, "call-file-append");
    assert_ne!(
        target_change.final_action_id.as_deref(),
        Some("call-file-commit")
    );
    let receipt: crate::file_change::FileChangeMutationReceipt =
        serde_json::from_str(&target_operation.receipt_json).unwrap();
    receipt.validate().unwrap();
    assert_eq!(receipt.transaction_id, target_change.id);
    let target_trace = conversation_trace_repository::get_trace_for_message(
        &connection,
        &plan.message_id_map["assistant-b"],
    )
    .unwrap()
    .unwrap();
    let call_args = target_trace
        .items
        .iter()
        .filter_map(|item| match item {
            ConversationTurnTraceItem::ToolCall {
                call_id, operation, ..
            } => Some((call_id.as_str(), operation)),
            _ => None,
        })
        .collect::<HashMap<_, _>>();
    assert_eq!(
        crate::file_change::proposal_digest(
            call_args
                .get(target_change.source_tool_call_id.as_str())
                .unwrap(),
        )
        .unwrap(),
        target_change.source_tool_arguments_digest
    );
    assert_eq!(
        crate::file_change::proposal_digest(
            call_args
                .get(target_operation.source_tool_call_id.as_str())
                .unwrap(),
        )
        .unwrap(),
        target_operation.source_tool_arguments_digest
    );
    assert_eq!(
        call_args
            .get(target_operation.source_tool_call_id.as_str())
            .unwrap()["request"]["transactionId"],
        target_change.id
    );
    assert!(file_change_repository::get_file_change_for_owner(
        &connection,
        "file-change-source",
        &plan.target.id,
        None,
        &plan.run_id_map["run-source-1"],
        "apply_patch",
    )
    .unwrap()
    .is_none());

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
        file_change_repository::list_file_changes_for_run(
            &connection,
            &plan.run_id_map["run-source-1"],
        )
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
fn fork_remaps_current_apply_patch_staged_transaction_and_read_observation() {
    use crate::file_change::{
        FileChangeBase, FileChangeDirectBinding, FileChangeMutation, FileChangeMutationReceipt,
        FileChangeOperation, FileChangeOutcome, FileChangePlanRequest, FileChangePlanner,
        FileChangeProposal, FileChangeStatus, FileChangeTransaction, FileObservationCheckpoint,
        FileObservationState, FILE_CHANGE_DIRECT_BINDING_SCHEMA_VERSION,
        FILE_CHANGE_SCHEMA_VERSION,
    };

    const SOURCE_TRANSACTION_ID: &str = "file-change-apply-source";
    const READ_CALL_ID: &str = "call-apply-read";
    const BEGIN_CALL_ID: &str = "call-apply-begin";
    const APPEND_CALL_ID: &str = "call-apply-append";
    const COMMIT_CALL_ID: &str = "call-apply-commit";

    let fixture = tempfile::tempdir().unwrap();
    let canonical_target = fixture.path().join("notes.md");
    std::fs::write(&canonical_target, "before\n").unwrap();
    let base_revision = crate::content_revision(b"before\n");

    let mut connection = Connection::open_in_memory().unwrap();
    migrations::run_migrations(&connection).unwrap();
    let source = source_conversation();
    chat_repository::save_conversation(&mut connection, source.clone()).unwrap();
    let (observation_id, observation_json) = apply_patch_observation(
        &source.id,
        "run-source-1",
        READ_CALL_ID,
        &canonical_target,
        &base_revision,
    );
    let source_observation: FileObservationCheckpoint =
        serde_json::from_str(&observation_json).unwrap();

    let read_args = json!({ "path": "notes.md" });
    let begin_args = apply_patch_args(json!({
        "action": "begin",
        "operation": "update",
        "filePath": "notes.md",
        "observationId": observation_id,
        "strategy": "rewrite",
    }));
    let append_args = apply_patch_args(json!({
        "action": "append",
        "transactionId": SOURCE_TRANSACTION_ID,
        "index": 0,
        "expectedDraftRevision": 0,
        "content": "after\n",
    }));
    let commit_args = apply_patch_args(json!({
        "action": "commit",
        "transactionId": SOURCE_TRANSACTION_ID,
        "expectedDraftRevision": 1,
        "summary": "replace notes",
    }));

    for (index, message) in source
        .messages
        .iter()
        .filter(|message| message.role == "assistant")
        .enumerate()
    {
        let items = if message.id == "assistant-b" {
            let mut items = staged_tool_exchange(0, READ_CALL_ID, "read_file", read_args.clone());
            if let ConversationTurnTraceItem::ToolResult { observation, .. } = &mut items[1] {
                *observation = json!({
                    "filePath": "notes.md",
                    "revision": base_revision,
                    "observationId": observation_id,
                });
            }
            items.extend(staged_tool_exchange(
                2,
                BEGIN_CALL_ID,
                "apply_patch",
                begin_args.clone(),
            ));
            items.extend(staged_tool_exchange(
                4,
                APPEND_CALL_ID,
                "apply_patch",
                append_args.clone(),
            ));
            let mut commit =
                staged_tool_exchange(6, COMMIT_CALL_ID, "apply_patch", commit_args.clone());
            if let ConversationTurnTraceItem::ToolCall {
                approval_status, ..
            } = &mut commit[0]
            {
                *approval_status = AgentApprovalStatus::Required;
            }
            if let ConversationTurnTraceItem::ToolResult {
                approval_status, ..
            } = &mut commit[1]
            {
                *approval_status = AgentApprovalStatus::Approved;
            }
            if let ConversationTurnTraceItem::ToolCall { operation, .. } = &mut commit[0] {
                *operation =
                    crate::file_change_support::apply_patch_trace_operation(&commit_args).unwrap();
            }
            items.extend(commit);
            items
        } else {
            vec![ConversationTurnTraceItem::AssistantNarration {
                sequence: 0,
                content: format!("narration {index}"),
                truncated: false,
            }]
        };
        conversation_trace_repository::replace_trace(
            &mut connection,
            &ConversationTurnTrace {
                schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
                run_id: format!("run-source-{index}"),
                conversation_id: source.id.clone(),
                assistant_message_id: message.id.clone(),
                terminal_status: ConversationTurnTraceTerminalStatus::Completed,
                terminal_error: None,
                truncated: false,
                items,
            },
            message.created_at,
            message.created_at + 1,
        )
        .unwrap();
    }

    file_change_repository::insert_file_change(
        &connection,
        &AgentFileChangeRecord {
            schema_version: 1,
            id: SOURCE_TRANSACTION_ID.to_string(),
            conversation_id: source.id.clone(),
            project_id: None,
            run_id: "run-source-1".to_string(),
            source_tool_name: "apply_patch".to_string(),
            source_tool_call_id: BEGIN_CALL_ID.to_string(),
            source_tool_arguments_digest: crate::file_change::proposal_digest(&begin_args).unwrap(),
            permission_revision: "permission-1".to_string(),
            tool_set_revision: "tool-set-1".to_string(),
            provider_wire_revision: "provider-protocol-v1".to_string(),
            observation_id: observation_id.clone(),
            observation_json,
            file_path: "notes.md".to_string(),
            operation: "update".to_string(),
            strategy: Some("rewrite".to_string()),
            status: "applied".to_string(),
            base_revision: Some(base_revision.clone()),
            base_content: "before\n".to_string(),
            content: "after\n".to_string(),
            draft_revision: 1,
            next_mutation_index: 1,
            additions: 1,
            deletions: 1,
            line_count: 1,
            byte_count: 6,
            mutation_count: 1,
            stats_final: true,
            summary: Some("replace notes".to_string()),
            final_action_id: Some(COMMIT_CALL_ID.to_string()),
            final_action_arguments_digest: Some(
                crate::file_change::proposal_digest(&commit_args).unwrap(),
            ),
            final_permission_revision: Some("permission-1".to_string()),
            final_tool_set_revision: Some("tool-set-1".to_string()),
            final_provider_wire_revision: Some("provider-protocol-v1".to_string()),
            created_at: 35,
            updated_at: 40,
            expires_at: i64::MAX,
        },
    )
    .unwrap();
    connection
        .execute(
            "INSERT INTO agent_file_change_chunks (
                 transaction_id, mutation_index, content_digest, byte_count, created_at
             ) VALUES (?1, 0, ?2, 6, 39)",
            params![
                SOURCE_TRANSACTION_ID,
                crate::file_change::content_digest(b"after\n")
            ],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO agent_file_change_operations (
                 transaction_id, mutation_index, source_tool_call_id,
                 source_tool_arguments_digest, action, payload_digest,
                 draft_revision, receipt_json, created_at
             ) VALUES (?1, 0, ?2, ?3, 'append', ?4, 1, ?5, 39)",
            params![
                SOURCE_TRANSACTION_ID,
                APPEND_CALL_ID,
                crate::file_change::proposal_digest(&append_args).unwrap(),
                crate::file_change::content_digest(b"after\n"),
                staged_mutation_receipt(SOURCE_TRANSACTION_ID, 0),
            ],
        )
        .unwrap();

    let frozen_plan = FileChangePlanner
        .plan(FileChangePlanRequest {
            operation: FileChangeOperation::Update,
            file_path: "notes.md",
            base: FileChangeBase::Existing {
                content: "before\n",
                revision: &base_revision,
            },
            mutation: FileChangeMutation::Complete("after\n".to_string()),
        })
        .unwrap();
    let execution = FileChangeDirectBinding {
        schema_version: FILE_CHANGE_DIRECT_BINDING_SCHEMA_VERSION,
        transaction: FileChangeTransaction {
            schema_version: FILE_CHANGE_SCHEMA_VERSION,
            id: SOURCE_TRANSACTION_ID.to_string(),
            operation: FileChangeOperation::Update,
            file_path: "notes.md".to_string(),
            status: FileChangeStatus::WaitingApproval,
            outcome: FileChangeOutcome::DefinitelyNotExecuted,
            base: frozen_plan.base.clone(),
            target: frozen_plan.target.clone(),
            proposal_digest: frozen_plan.proposal_digest.clone(),
            created_at: 35,
            updated_at: 40,
        },
        proposal: FileChangeProposal {
            schema_version: FILE_CHANGE_SCHEMA_VERSION,
            id: COMMIT_CALL_ID.to_string(),
            transaction_id: SOURCE_TRANSACTION_ID.to_string(),
            operation: FileChangeOperation::Update,
            file_path: "notes.md".to_string(),
            base: frozen_plan.base.clone(),
            target: frozen_plan.target.clone(),
            diff_digest: frozen_plan.diff_digest.clone(),
            proposal_digest: frozen_plan.proposal_digest.clone(),
            additions: frozen_plan.additions,
            deletions: frozen_plan.deletions,
        },
        observation_id: observation_id.clone(),
        observation: source_observation.clone(),
        source_tool_name: "apply_patch".to_string(),
        source_call_id: COMMIT_CALL_ID.to_string(),
        source_args_digest: crate::file_change::proposal_digest(&commit_args).unwrap(),
        trace_args_digest: crate::file_change_support::apply_patch_trace_args_digest(&commit_args)
            .unwrap(),
        staged_transaction_id: Some(SOURCE_TRANSACTION_ID.to_string()),
        conversation_id: source.id.clone(),
        project_id: None,
        run_id: "run-source-1".to_string(),
        staged_transaction_revision: Some(1),
        canonical_target: canonical_target.to_string_lossy().into_owned(),
        base_content: Some("before\n".to_string()),
        target_content: Some("after\n".to_string()),
        delete_journal: None,
        receipt: None,
        permission_revision: "permission-1".to_string(),
        tool_set_revision: "tool-set-1".to_string(),
        provider_wire_revision: "provider-protocol-v1".to_string(),
    };
    execution.validate().unwrap();
    let audit_action = AgentProposedAction::FileChange {
        file_change: crate::AgentFileChangeProposal {
            schema_version: crate::AGENT_FILE_CHANGE_PROTOCOL_SCHEMA_VERSION,
            id: COMMIT_CALL_ID.to_string(),
            transaction_id: SOURCE_TRANSACTION_ID.to_string(),
            operation: crate::AgentFileChangeOperation::Update,
            update_strategy: Some(crate::AgentFileChangeUpdateStrategy::Rewrite),
            file_path: "notes.md".to_string(),
            inline_diff: None,
            base_revision: Some(base_revision.clone()),
            summary: Some("replace notes".to_string()),
            additions: frozen_plan.additions,
            deletions: frozen_plan.deletions,
            line_count: 1,
            byte_count: 6,
            approval_status: AgentApprovalStatus::Approved,
            execution: Box::new(execution),
        },
    };
    if let AgentProposedAction::FileChange { file_change } = &audit_action {
        file_change.validate().unwrap();
    }
    let source_audit_id = crate::canonical_pending_action_id("run-source-1", COMMIT_CALL_ID);
    assert!(
        agent_action_audit_repository::insert_action_audit_record_if_absent(
            &connection,
            &AgentActionAuditRecord {
                action_id: source_audit_id,
                run_id: "run-source-1".to_string(),
                conversation_id: Some(source.id.clone()),
                assistant_message_id: Some("assistant-b".to_string()),
                action_type: "file_change".to_string(),
                tool_name: "apply_patch".to_string(),
                decision: Some("approved".to_string()),
                status: "completed".to_string(),
                action_json: serde_json::to_string(&audit_action).unwrap(),
                file_change_result_json: None,
                command_result_json: None,
                tool_result_json: None,
                error: None,
                created_at: 35,
                decided_at: Some(39),
                completed_at: Some(40),
                effective_permissions_json: Some("{}".to_string()),
                path_scope: Some(canonical_target.to_string_lossy().into_owned()),
                command_cwd_scope: None,
                blocked_reason: None,
                decision_source: Some("manual".to_string()),
            },
        )
        .unwrap()
    );

    let plan = build_assistant_reply_fork_plan(
        &connection,
        "fork-current-apply-patch-staged",
        &source.id,
        "assistant-c",
        100,
    )
    .unwrap();
    assert_eq!(plan.file_changes.len(), 1);
    assert_eq!(plan.action_audits.len(), 1);
    let target_run_id = plan.run_id_map["run-source-1"].clone();
    let target_transaction_id = plan.file_changes[0].id.clone();
    assert_ne!(target_transaction_id, SOURCE_TRANSACTION_ID);
    commit_fork_plan(&mut connection, &plan).unwrap();

    let target_change =
        file_change_repository::get_file_change(&connection, &target_transaction_id)
            .unwrap()
            .unwrap();
    assert_eq!(target_change.conversation_id, plan.target.id);
    assert_eq!(target_change.run_id, target_run_id);
    assert_eq!(target_change.source_tool_name, "apply_patch");
    assert_ne!(target_change.source_tool_call_id, BEGIN_CALL_ID);
    assert_ne!(
        target_change.final_action_id.as_deref(),
        Some(COMMIT_CALL_ID)
    );
    assert_ne!(target_change.observation_id, observation_id);

    let target_observation: FileObservationCheckpoint =
        serde_json::from_str(&target_change.observation_json).unwrap();
    assert_eq!(
        target_observation.observation_id,
        target_change.observation_id
    );
    assert_eq!(target_observation.conversation_id, plan.target.id);
    assert_eq!(target_observation.run_id, target_run_id);
    assert_ne!(target_observation.source_tool_call_id, READ_CALL_ID);
    assert_eq!(target_observation.state, source_observation.state);
    assert_eq!(
        target_observation.parent_directory_identity,
        source_observation.parent_directory_identity
    );
    target_observation
        .validate_frozen_binding(&plan.target.id, &target_run_id, &canonical_target)
        .unwrap();

    let target_history = file_change_repository::load_file_change_history_snapshot(
        &connection,
        &target_transaction_id,
        None,
    )
    .unwrap();
    assert_eq!(target_history.chunks.len(), 1);
    assert_eq!(target_history.operations.len(), 1);
    assert_eq!(
        target_history.chunks[0].transaction_id,
        target_transaction_id
    );
    assert_eq!(
        target_history.chunks[0].content_digest,
        crate::file_change::content_digest(b"after\n")
    );
    let target_operation = &target_history.operations[0];
    assert_eq!(target_operation.transaction_id, target_transaction_id);
    assert_ne!(target_operation.source_tool_call_id, APPEND_CALL_ID);
    let receipt: FileChangeMutationReceipt =
        serde_json::from_str(&target_operation.receipt_json).unwrap();
    receipt.validate().unwrap();
    assert_eq!(receipt.transaction_id, target_transaction_id);

    let target_trace = conversation_trace_repository::get_trace_for_message(
        &connection,
        &plan.message_id_map["assistant-b"],
    )
    .unwrap()
    .unwrap();
    assert_eq!(target_trace.run_id, target_run_id);
    let calls = target_trace
        .items
        .iter()
        .filter_map(|item| match item {
            ConversationTurnTraceItem::ToolCall {
                call_id,
                tool,
                operation,
                ..
            } => Some((call_id, tool, operation)),
            _ => None,
        })
        .collect::<Vec<_>>();
    let target_read = calls
        .iter()
        .find(|(_, tool, _)| tool.as_str() == "read_file")
        .unwrap();
    let target_begin = calls
        .iter()
        .find(|(_, _, operation)| operation["request"]["action"] == "begin")
        .unwrap();
    let target_append = calls
        .iter()
        .find(|(_, _, operation)| operation["request"]["action"] == "append")
        .unwrap();
    let target_commit = calls
        .iter()
        .find(|(_, _, operation)| operation["request"]["action"] == "commit")
        .unwrap();
    assert_eq!(
        target_read.0.as_str(),
        target_observation.source_tool_call_id
    );
    assert_eq!(target_begin.0.as_str(), target_change.source_tool_call_id);
    assert_eq!(
        target_begin.2["request"]["observationId"],
        target_change.observation_id
    );
    assert_eq!(
        target_append.0.as_str(),
        target_operation.source_tool_call_id
    );
    assert_eq!(
        target_append.2["request"]["transactionId"],
        target_transaction_id
    );
    assert_eq!(
        crate::file_change::proposal_digest(target_append.2).unwrap(),
        target_operation.source_tool_arguments_digest
    );
    assert_eq!(
        target_commit.0.as_str(),
        target_change.final_action_id.as_deref().unwrap()
    );
    assert_eq!(
        target_commit.2["request"]["transactionId"],
        target_transaction_id
    );
    let target_read_result = target_trace
        .items
        .iter()
        .find_map(|item| match item {
            ConversationTurnTraceItem::ToolResult {
                call_id,
                tool,
                observation,
                ..
            } if tool == "read_file" => Some((call_id, observation)),
            _ => None,
        })
        .unwrap();
    assert_eq!(target_read_result.0, target_read.0);
    assert_eq!(
        target_read_result.1["observationId"],
        target_change.observation_id
    );

    assert!(file_change_repository::get_file_change_for_owner(
        &connection,
        SOURCE_TRANSACTION_ID,
        &plan.target.id,
        None,
        &target_run_id,
        "apply_patch",
    )
    .unwrap()
    .is_none());
    assert!(file_change_repository::get_file_change_for_owner(
        &connection,
        &target_transaction_id,
        &plan.target.id,
        None,
        &target_run_id,
        "apply_patch",
    )
    .unwrap()
    .is_some());
    assert!(!serde_json::to_string(&target_trace)
        .unwrap()
        .contains(SOURCE_TRANSACTION_ID));
    assert!(matches!(
        target_observation.state,
        FileObservationState::Existing { .. }
    ));

    let target_commit_call_id = plan.id_replacements[COMMIT_CALL_ID].clone();
    let target_audit_id =
        crate::canonical_pending_action_id(&target_run_id, &target_commit_call_id);
    let target_audit =
        agent_action_audit_repository::load_action_audit_record(&connection, &target_audit_id)
            .unwrap()
            .unwrap();
    assert_eq!(
        target_audit.conversation_id.as_deref(),
        Some(plan.target.id.as_str())
    );
    assert_eq!(
        target_audit.assistant_message_id.as_deref(),
        Some(plan.message_id_map["assistant-b"].as_str())
    );
    let target_audit_action: AgentProposedAction =
        serde_json::from_str(&target_audit.action_json).unwrap();
    let AgentProposedAction::FileChange {
        file_change: target_audit_change,
    } = target_audit_action
    else {
        unreachable!()
    };
    assert_eq!(target_audit_change.id, target_commit_call_id);
    assert_eq!(target_audit_change.transaction_id, target_transaction_id);
    assert_eq!(
        target_audit_change.execution.conversation_id,
        plan.target.id
    );
    assert_eq!(target_audit_change.execution.run_id, target_run_id);
    assert_eq!(
        target_audit_change.execution.base_content.as_deref(),
        Some("before\n")
    );
    assert_eq!(
        target_audit_change.execution.target_content.as_deref(),
        Some("after\n")
    );
    assert_eq!(
        crate::file_change::FileChangePlan::from_binding(&target_audit_change.execution)
            .unwrap()
            .diff,
        frozen_plan.diff
    );

    let recursive = build_assistant_reply_fork_plan(
        &connection,
        "fork-current-apply-patch-staged-recursive",
        &plan.target.id,
        &plan.message_id_map["assistant-c"],
        120,
    )
    .unwrap();
    assert_eq!(recursive.action_audits.len(), 1);
    commit_fork_plan(&mut connection, &recursive).unwrap();
    let recursive_run_id = recursive.run_id_map[&target_run_id].clone();
    let recursive_call_id = recursive.id_replacements[&target_commit_call_id].clone();
    let recursive_audit = agent_action_audit_repository::load_action_audit_record(
        &connection,
        &crate::canonical_pending_action_id(&recursive_run_id, &recursive_call_id),
    )
    .unwrap()
    .unwrap();
    let recursive_action: AgentProposedAction =
        serde_json::from_str(&recursive_audit.action_json).unwrap();
    let AgentProposedAction::FileChange {
        file_change: recursive_change,
    } = recursive_action
    else {
        unreachable!()
    };
    assert_eq!(recursive_change.id, recursive_call_id);
    assert_ne!(recursive_change.transaction_id, target_transaction_id);
    assert_eq!(
        recursive_change.execution.conversation_id,
        recursive.target.id
    );
    assert_eq!(recursive_change.execution.run_id, recursive_run_id);
    assert_eq!(
        recursive_change.execution.base_content.as_deref(),
        Some("before\n")
    );
    assert_eq!(
        recursive_change.execution.target_content.as_deref(),
        Some("after\n")
    );
}

#[test]
fn fork_remaps_completed_direct_apply_patch_audit_without_a_staged_transaction() {
    use crate::file_change::{
        FileChangeBase, FileChangeDirectBinding, FileChangeMutation, FileChangeOperation,
        FileChangeOutcome, FileChangePlanRequest, FileChangePlanner, FileChangeProposal,
        FileChangeStatus, FileChangeTransaction, FileObservationCheckpoint,
        FILE_CHANGE_DIRECT_BINDING_SCHEMA_VERSION, FILE_CHANGE_SCHEMA_VERSION,
    };

    const READ_CALL_ID: &str = "call-direct-read";
    const APPLY_CALL_ID: &str = "call-direct-apply";
    const SOURCE_TRANSACTION_ID: &str = "file-change-direct-source";

    let fixture = tempfile::tempdir().unwrap();
    let canonical_target = fixture.path().join("direct.md");
    std::fs::write(&canonical_target, "before\n").unwrap();
    let base_revision = crate::content_revision(b"before\n");
    let mut connection = Connection::open_in_memory().unwrap();
    migrations::run_migrations(&connection).unwrap();
    let source = source_conversation();
    chat_repository::save_conversation(&mut connection, source.clone()).unwrap();
    let (observation_id, observation_json) = apply_patch_observation(
        &source.id,
        "run-source-0",
        READ_CALL_ID,
        &canonical_target,
        &base_revision,
    );
    let observation: FileObservationCheckpoint = serde_json::from_str(&observation_json).unwrap();
    let read_args = json!({ "path": "direct.md" });
    let apply_args = apply_patch_args(json!({
        "action": "apply",
        "operation": "update",
        "filePath": "direct.md",
        "observationId": observation_id,
        "content": "after\n",
    }));
    let trace_apply_args =
        crate::file_change_support::apply_patch_trace_operation(&apply_args).unwrap();
    let mut items = staged_tool_exchange(0, READ_CALL_ID, "read_file", read_args);
    if let ConversationTurnTraceItem::ToolResult {
        observation: result,
        ..
    } = &mut items[1]
    {
        *result = json!({
            "filePath": "direct.md",
            "revision": base_revision,
            "observationId": observation_id,
        });
    }
    let mut apply = staged_tool_exchange(2, APPLY_CALL_ID, "apply_patch", trace_apply_args);
    if let ConversationTurnTraceItem::ToolCall {
        approval_status, ..
    } = &mut apply[0]
    {
        *approval_status = AgentApprovalStatus::Required;
    }
    if let ConversationTurnTraceItem::ToolResult {
        approval_status,
        observation,
        ..
    } = &mut apply[1]
    {
        *approval_status = AgentApprovalStatus::Approved;
        *observation = json!({
            "schemaVersion": 1,
            "status": "applied",
            "outcome": "applied",
            "transactionId": SOURCE_TRANSACTION_ID,
            "operation": "update",
            "updateStrategy": null,
            "filePath": "direct.md",
            "additions": 1,
            "deletions": 1,
            "lineCount": 1,
            "byteCount": 6,
            "revision": crate::content_revision(b"after\n"),
            "errorCode": null,
            "error": null,
            "message": null,
        });
    }
    items.extend(apply);
    conversation_trace_repository::replace_trace(
        &mut connection,
        &ConversationTurnTrace {
            schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
            run_id: "run-source-0".to_string(),
            conversation_id: source.id.clone(),
            assistant_message_id: "assistant-a".to_string(),
            terminal_status: ConversationTurnTraceTerminalStatus::Completed,
            terminal_error: None,
            truncated: true,
            items,
        },
        20,
        21,
    )
    .unwrap();

    let frozen_plan = FileChangePlanner
        .plan(FileChangePlanRequest {
            operation: FileChangeOperation::Update,
            file_path: "direct.md",
            base: FileChangeBase::Existing {
                content: "before\n",
                revision: &base_revision,
            },
            mutation: FileChangeMutation::Complete("after\n".to_string()),
        })
        .unwrap();
    let execution = FileChangeDirectBinding {
        schema_version: FILE_CHANGE_DIRECT_BINDING_SCHEMA_VERSION,
        transaction: FileChangeTransaction {
            schema_version: FILE_CHANGE_SCHEMA_VERSION,
            id: SOURCE_TRANSACTION_ID.to_string(),
            operation: FileChangeOperation::Update,
            file_path: "direct.md".to_string(),
            status: FileChangeStatus::WaitingApproval,
            outcome: FileChangeOutcome::DefinitelyNotExecuted,
            base: frozen_plan.base.clone(),
            target: frozen_plan.target.clone(),
            proposal_digest: frozen_plan.proposal_digest.clone(),
            created_at: 20,
            updated_at: 20,
        },
        proposal: FileChangeProposal {
            schema_version: FILE_CHANGE_SCHEMA_VERSION,
            id: APPLY_CALL_ID.to_string(),
            transaction_id: SOURCE_TRANSACTION_ID.to_string(),
            operation: FileChangeOperation::Update,
            file_path: "direct.md".to_string(),
            base: frozen_plan.base.clone(),
            target: frozen_plan.target.clone(),
            diff_digest: frozen_plan.diff_digest.clone(),
            proposal_digest: frozen_plan.proposal_digest.clone(),
            additions: frozen_plan.additions,
            deletions: frozen_plan.deletions,
        },
        observation_id: observation_id.clone(),
        observation,
        source_tool_name: "apply_patch".to_string(),
        source_call_id: APPLY_CALL_ID.to_string(),
        source_args_digest: crate::file_change::proposal_digest(&apply_args).unwrap(),
        trace_args_digest: crate::file_change_support::apply_patch_trace_args_digest(&apply_args)
            .unwrap(),
        staged_transaction_id: None,
        conversation_id: source.id.clone(),
        project_id: None,
        run_id: "run-source-0".to_string(),
        staged_transaction_revision: None,
        canonical_target: canonical_target.to_string_lossy().into_owned(),
        base_content: Some("before\n".to_string()),
        target_content: Some("after\n".to_string()),
        delete_journal: None,
        receipt: None,
        permission_revision: "permission-1".to_string(),
        tool_set_revision: "tool-set-1".to_string(),
        provider_wire_revision: "provider-protocol-v1".to_string(),
    };
    execution.validate().unwrap();
    let source_args_digest = execution.source_args_digest.clone();
    let inline_patch = frozen_plan.diff.clone();
    let action = AgentProposedAction::FileChange {
        file_change: crate::AgentFileChangeProposal {
            schema_version: crate::AGENT_FILE_CHANGE_PROTOCOL_SCHEMA_VERSION,
            id: APPLY_CALL_ID.to_string(),
            transaction_id: SOURCE_TRANSACTION_ID.to_string(),
            operation: crate::AgentFileChangeOperation::Update,
            update_strategy: None,
            file_path: "direct.md".to_string(),
            inline_diff: Some(crate::AgentGitDiffSnapshot {
                patch: inline_patch.clone(),
                truncated: false,
            }),
            base_revision: Some(base_revision),
            summary: None,
            additions: frozen_plan.additions,
            deletions: frozen_plan.deletions,
            line_count: 1,
            byte_count: 6,
            approval_status: AgentApprovalStatus::Approved,
            execution: Box::new(execution),
        },
    };
    let terminal_result = crate::AgentFileChangeResult {
        schema_version: crate::AGENT_FILE_CHANGE_PROTOCOL_SCHEMA_VERSION,
        status: crate::AgentFileChangeResultStatus::Applied,
        outcome: crate::AgentFileChangeOutcome::Applied,
        transaction_id: SOURCE_TRANSACTION_ID.to_string(),
        operation: crate::AgentFileChangeOperation::Update,
        update_strategy: None,
        file_path: "direct.md".to_string(),
        additions: frozen_plan.additions,
        deletions: frozen_plan.deletions,
        line_count: 1,
        byte_count: 6,
        revision: Some(crate::content_revision(b"after\n")),
        error_code: None,
        error: None,
        message: None,
    };
    terminal_result.validate().unwrap();
    let terminal_tool_result = AgentToolResult {
        call_id: APPLY_CALL_ID.to_string(),
        tool: "apply_patch".to_string(),
        ok: true,
        result: Some(serde_json::to_value(&terminal_result).unwrap()),
        error: None,
        exact_archive_file: None,
    };
    assert!(
        agent_action_audit_repository::insert_action_audit_record_if_absent(
            &connection,
            &AgentActionAuditRecord {
                action_id: crate::canonical_pending_action_id("run-source-0", APPLY_CALL_ID),
                run_id: "run-source-0".to_string(),
                conversation_id: Some(source.id.clone()),
                assistant_message_id: Some("assistant-a".to_string()),
                action_type: "file_change".to_string(),
                tool_name: "apply_patch".to_string(),
                decision: Some("approved".to_string()),
                status: "completed".to_string(),
                action_json: serde_json::to_string(&action).unwrap(),
                file_change_result_json: Some(serde_json::to_string(&terminal_result).unwrap()),
                command_result_json: None,
                tool_result_json: Some(serde_json::to_string(&terminal_tool_result).unwrap()),
                error: None,
                created_at: 20,
                decided_at: Some(20),
                completed_at: Some(21),
                effective_permissions_json: Some("{}".to_string()),
                path_scope: Some(canonical_target.to_string_lossy().into_owned()),
                command_cwd_scope: None,
                blocked_reason: None,
                decision_source: Some("manual".to_string()),
            },
        )
        .unwrap()
    );

    let plan = build_assistant_reply_fork_plan(
        &connection,
        "fork-completed-direct-apply-patch",
        &source.id,
        "assistant-a",
        100,
    )
    .unwrap();
    assert!(plan.file_changes.is_empty());
    assert_eq!(plan.action_audits.len(), 1);
    let target_transaction_id = plan.id_replacements[SOURCE_TRANSACTION_ID].clone();
    let target_observation_id = plan.id_replacements[&observation_id].clone();
    commit_fork_plan(&mut connection, &plan).unwrap();

    let target_run_id = plan.run_id_map["run-source-0"].clone();
    let target_call_id = plan.id_replacements[APPLY_CALL_ID].clone();
    let target_audit = agent_action_audit_repository::load_action_audit_record(
        &connection,
        &crate::canonical_pending_action_id(&target_run_id, &target_call_id),
    )
    .unwrap()
    .unwrap();
    let target_action: AgentProposedAction =
        serde_json::from_str(&target_audit.action_json).unwrap();
    let AgentProposedAction::FileChange {
        file_change: target_change,
    } = target_action
    else {
        unreachable!()
    };
    assert_eq!(target_change.id, target_call_id);
    assert_eq!(target_change.transaction_id, target_transaction_id);
    assert_eq!(target_change.inline_diff.unwrap().patch, inline_patch);
    assert_eq!(target_change.execution.conversation_id, plan.target.id);
    assert_eq!(target_change.execution.run_id, target_run_id);
    assert_eq!(
        target_change.execution.observation_id,
        target_observation_id
    );
    assert_eq!(
        target_change.execution.source_args_digest,
        source_args_digest
    );
    let target_result: crate::AgentFileChangeResult =
        serde_json::from_str(target_audit.file_change_result_json.as_deref().unwrap()).unwrap();
    assert_eq!(target_result.transaction_id, target_transaction_id);
    let target_tool_result: AgentToolResult =
        serde_json::from_str(target_audit.tool_result_json.as_deref().unwrap()).unwrap();
    assert_eq!(target_tool_result.call_id, target_call_id);
    assert_eq!(
        target_tool_result.result.as_ref().unwrap()["transactionId"],
        target_transaction_id
    );
    let target_trace = conversation_trace_repository::get_trace_for_message(
        &connection,
        &plan.message_id_map["assistant-a"],
    )
    .unwrap()
    .unwrap();
    let target_result_transaction_id = target_trace.items.iter().find_map(|item| match item {
        ConversationTurnTraceItem::ToolResult {
            tool, observation, ..
        } if tool == "apply_patch" => observation["transactionId"].as_str(),
        _ => None,
    });
    assert_eq!(
        target_result_transaction_id,
        Some(target_transaction_id.as_str())
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
