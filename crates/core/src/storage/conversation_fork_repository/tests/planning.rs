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
        &HashMap::new(),
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
        &HashMap::new(),
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
        human_interaction_response: None,
        id: "assistant-running".to_string(),
        role: "assistant".to_string(),
        content: "partial".to_string(),
        created_at: 1,
        status: Some("sent".to_string()),
        attachments: Vec::new(),
        folder_references_json: None,
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
fn ordinary_fork_copies_message_ui_state_overlay() {
    let mut connection = Connection::open_in_memory().unwrap();
    migrations::run_migrations(&connection).unwrap();
    let mut source = source_conversation();
    source.messages[1].ui_state_json = Some(r#"{"favorited":true}"#.to_string());
    chat_repository::save_conversation(&mut connection, source.clone()).unwrap();

    let plan = build_assistant_reply_fork_plan(
        &connection,
        "fork-ui-state-overlay",
        &source.id,
        "assistant-a",
        20,
    )
    .unwrap();
    let target_message_id = plan.message_id_map["assistant-a"].clone();
    commit_fork_plan(&mut connection, &plan).unwrap();

    let target = chat_repository::get_conversation(&connection, &plan.target.id)
        .unwrap()
        .unwrap();
    assert_eq!(
        target
            .messages
            .iter()
            .find(|message| message.id == target_message_id)
            .unwrap()
            .ui_state_json
            .as_deref(),
        Some(r#"{"favorited":true}"#)
    );
}

#[test]
fn fork_does_not_inherit_context_profile_admission_policy_and_next_run_uses_current_setting() {
    use crate::storage::agent_context_profile_repository;
    use crate::AgentContextProfile;

    let mut connection = Connection::open_in_memory().unwrap();
    migrations::run_migrations(&connection).unwrap();
    let source = source_conversation();
    chat_repository::save_conversation(&mut connection, source.clone()).unwrap();
    conversation_trace_repository::replace_trace(
        &mut connection,
        &ConversationTurnTrace {
            schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
            run_id: "run-source-0".into(),
            conversation_id: source.id.clone(),
            assistant_message_id: "assistant-a".into(),
            terminal_status: ConversationTurnTraceTerminalStatus::Completed,
            terminal_error: None,
            truncated: false,
            items: vec![ConversationTurnTraceItem::AssistantNarration {
                provider_turn_id: None,
                first_tool_call_id: None,
                sequence: 0,
                content: "content assistant-a".into(),
                truncated: false,
            }],
        },
        19,
        20,
    )
    .unwrap();
    connection
        .execute(
            "INSERT INTO agent_prompt_preferences
         (id, work_mode, tone, detail_level, custom_instructions, updated_at, context_profile)
         VALUES ('default', 'coding', 'pragmatic', 'medium', '', 20, 'minimal')
         ON CONFLICT(id) DO UPDATE SET context_profile='minimal'",
            [],
        )
        .unwrap();
    assert_eq!(
        agent_context_profile_repository::freeze_run(&connection, "run-source-0", None).unwrap(),
        AgentContextProfile::Minimal
    );

    crate::storage::agent_workspace_repository::freeze_run(&connection, "run-source-0", None, None).unwrap();
    let plan = build_assistant_reply_fork_plan(
        &connection,
        "fork-context-profile-policy",
        &source.id,
        "assistant-a",
        21,
    )
    .unwrap();
    commit_fork_plan(&mut connection, &plan).unwrap();
    let cloned_trace = conversation_trace_repository::get_trace_for_message(
        &connection,
        &plan.message_id_map["assistant-a"],
    )
    .unwrap()
    .unwrap();
    assert_ne!(cloned_trace.run_id, "run-source-0");
    assert_eq!(crate::storage::agent_workspace_repository::load_run(&connection, &cloned_trace.run_id).unwrap(), Some(None),
        "historical file identity bindings are copied independently of executable mode policy");

    assert_eq!(
        agent_context_profile_repository::load_run(&connection, &cloned_trace.run_id).unwrap(),
        None,
        "a copied historical trace must not create executable Run policy"
    );
    assert_eq!(
        agent_context_profile_repository::load_run(&connection, "run-source-0").unwrap(),
        Some(AgentContextProfile::Minimal),
        "fork must leave the original Run's immutable policy untouched"
    );

    connection
        .execute(
            "UPDATE agent_prompt_preferences SET context_profile='full' WHERE id='default'",
            [],
        )
        .unwrap();
    connection.execute(
        "INSERT INTO messages (id, conversation_id, role, content, status, created_at, position)
         VALUES ('assistant-after-profile-fork', ?1, 'assistant', '', 'pending', 22, 2)",
        [&plan.target.id],
    ).unwrap();
    conversation_trace_repository::append_in_progress_trace(
        &mut connection,
        &crate::ConversationTraceSnapshot::default().in_progress_trace(
            "run-after-profile-fork",
            &plan.target.id,
            "assistant-after-profile-fork",
        ),
        22,
        22,
    )
    .unwrap();
    assert_eq!(
        agent_context_profile_repository::freeze_run(&connection, "run-after-profile-fork", None)
            .unwrap(),
        AgentContextProfile::Full,
        "a new root Run in the fork must freeze the current setting, not its source Run's mode"
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
                folder_references: Vec::new(),
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
            images: Vec::new(),
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
            provider_turn_id: None,
            first_tool_call_id: None,
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
fn fork_rebinds_narration_to_its_cloned_tool_call_without_changing_provider_turn() {
    let mut connection = Connection::open_in_memory().unwrap();
    migrations::run_migrations(&connection).unwrap();
    let source = source_conversation();
    chat_repository::save_conversation(&mut connection, source.clone()).unwrap();
    let source_call = crate::llm::model_response_tool_call_id("run-source-0", 0, 0, "source-call");
    let provider_turn_id = format!("at1_{}", "a".repeat(64));
    let mut trace = ConversationTurnTrace {
        schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
        run_id: "run-source-0".into(),
        conversation_id: source.id.clone(),
        assistant_message_id: "assistant-a".into(),
        terminal_status: ConversationTurnTraceTerminalStatus::Completed,
        terminal_error: None,
        truncated: false,
        items: vec![ConversationTurnTraceItem::AssistantNarration {
            sequence: 0,
            content: "Inspecting evidence.".into(),
            provider_turn_id: Some(provider_turn_id.clone()),
            first_tool_call_id: Some(source_call.clone()),
            truncated: false,
        }],
    };
    trace.items.extend(staged_tool_exchange(
        1,
        &source_call,
        "read_file",
        json!({"path":"evidence.txt"}),
    ));
    conversation_trace_repository::replace_trace(&mut connection, &trace, 20, 21).unwrap();
    let narration = ConversationModelContextItem {
        sequence: 0,
        ordinal: 0,
        role: "assistant".into(),
        content: "Inspecting evidence.".into(),
        images: Vec::new(),
        tool_call_id: None,
        tool_calls: Vec::new(),
        is_error: false,
    };
    let mut call = narration.clone();
    call.sequence = 1;
    call.tool_calls.push(crate::AgentContextCheckpointToolCall {
        id: source_call.clone(),
        name: "read_file".into(),
        args: json!({"path":"evidence.txt"}),
        provider_identity: crate::AgentProviderToolCallIdentity {
            provider_tool_index: 0,
            provider_call_id: "provider-call".into(),
            runtime_call_id: source_call.clone(),
        },
    });
    let mut result = narration.clone();
    result.sequence = 2;
    result.role = "tool".into();
    result.tool_call_id = Some(source_call.clone());
    result.content = "{\"accepted\":true}".into();
    conversation_model_context_repository::commit_items_in_connection(
        &connection,
        &source.id,
        "assistant-a",
        &[narration, call, result],
    )
    .unwrap();
    let plan = build_assistant_reply_fork_plan(
        &connection,
        "fork-narration-binding",
        &source.id,
        "assistant-a",
        30,
    )
    .unwrap();
    commit_fork_plan(&mut connection, &plan).unwrap();
    let target_trace = conversation_trace_repository::get_trace_for_message(
        &connection,
        &plan.message_id_map["assistant-a"],
    )
    .unwrap()
    .unwrap();
    let ConversationTurnTraceItem::AssistantNarration {
        first_tool_call_id: Some(bound_call),
        provider_turn_id: Some(bound_turn),
        ..
    } = &target_trace.items[0]
    else {
        panic!("missing narration identity")
    };
    let ConversationTurnTraceItem::ToolCall { call_id, .. } = &target_trace.items[1] else {
        panic!("missing cloned tool call")
    };
    assert_ne!(bound_call, &source_call);
    assert_eq!(bound_call, call_id);
    assert_eq!(bound_turn, &provider_turn_id);
    let log = conversation_model_context_repository::get_log_for_message(
        &connection,
        &plan.message_id_map["assistant-a"],
    )
    .unwrap()
    .unwrap();
    assert_eq!(&log.items[1].tool_calls[0].id, bound_call);
    assert_eq!(log.items[2].tool_call_id.as_ref(), Some(bound_call));
    target_trace
        .validate_complete_model_context(&log.items)
        .unwrap();
}
