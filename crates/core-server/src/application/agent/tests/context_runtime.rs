use super::*;

#[tokio::test]
async fn compaction_host_prepares_generates_commits_and_rebuilds_running_state() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    storage.save_model_settings(test_model_settings()).unwrap();
    storage
        .save_conversation(ChatConversationRecord {
            id: "conversation-compaction-host".to_string(),
            project_id: None,
            model_id: Some("model-1".to_string()),
            title: "Compaction host".to_string(),
            messages: vec![
                ChatMessageRecord {
                    id: "user-old".to_string(),
                    role: "user".to_string(),
                    content: "An old request with substantial detail.".repeat(200),
                    created_at: 1,
                    status: Some("sent".to_string()),
                    attachments: Vec::new(),
                    agent_run_json: None,
                    ui_state_json: None,
                },
                ChatMessageRecord {
                    id: "assistant-old".to_string(),
                    role: "assistant".to_string(),
                    content: "The old request was completed with detailed results.".repeat(200),
                    created_at: 2,
                    status: Some("sent".to_string()),
                    attachments: Vec::new(),
                    agent_run_json: None,
                    ui_state_json: None,
                },
                ChatMessageRecord {
                    id: "user-current".to_string(),
                    role: "user".to_string(),
                    content: "Continue the work.".to_string(),
                    created_at: 3,
                    status: Some("sent".to_string()),
                    attachments: Vec::new(),
                    agent_run_json: None,
                    ui_state_json: None,
                },
                ChatMessageRecord {
                    id: "assistant-current".to_string(),
                    role: "assistant".to_string(),
                    content: String::new(),
                    created_at: 4,
                    status: Some("pending".to_string()),
                    attachments: Vec::new(),
                    agent_run_json: None,
                    ui_state_json: None,
                },
            ],
            created_at: 1,
            updated_at: 4,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
    // Construct the service before publishing the current run's trace. Any in-progress trace
    // already present when AgentService starts is, by definition, owned by the previous process
    // and is retired by startup reconciliation.
    let service = AgentService::new(storage.clone())
        .with_context_compaction_summary_generator(test_context_compaction_generator());
    storage
        .append_in_progress_conversation_turn_trace(
            &ConversationTurnTrace {
                schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
                run_id: "run-compaction-host".to_string(),
                conversation_id: "conversation-compaction-host".to_string(),
                assistant_message_id: "assistant-current".to_string(),
                terminal_status: ConversationTurnTraceTerminalStatus::InProgress,
                terminal_error: None,
                truncated: false,
                items: Vec::new(),
            },
            4,
            4,
        )
        .unwrap();
    let mut agent_input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "secret",
        "model": "model-1",
        "modelCapabilities": { "imageInput": false },
        "contextWindowTokens": 16000,
        "contextWindowIndicatorEnabled": true,
        "maxTokens": 1000,
        "assistantMessageId": "assistant-current",
        "skillActivation": {
            "activationRevision": "skill-activation-sha256-v1:compaction",
            "skills": [{
                "id": "workspace:project:compaction-review",
                "name": "compaction-review",
                "revision": "skill-package-sha256-v1:compaction",
                "source": "workspace:project",
                "instructions": "SKILL_COMPACTION_OVERLAY_MARKER",
                "sourceBytes": 31
            }]
        },
        "context": {
            "conversationId": "conversation-compaction-host",
            "projectId": null,
            "workspace": null,
            "permissions": {
                "read": "workspace_only",
                "write": "denied",
                "command": "require_approval",
                "commandSafety": "guarded",
                "patch": "require_approval",
                "builtinExecution": "require_approval"
            }
        },
        "messages": []
    }))
    .unwrap();
    freeze_test_pending_provider_configuration(&storage, &mut agent_input);
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let services = service.context_compaction_services(
        "run-compaction-host",
        "conversation-compaction-host",
        "assistant-current",
        agent_input,
        RunContextToolProjection::pending(),
        notifications,
    );
    let cancellation = AgentCancellationToken::new();
    let prepare_request = mycopilot_core::AgentContextCompactionPrepareRequest {
        run_id: "run-compaction-host".to_string(),
        conversation_id: "conversation-compaction-host".to_string(),
        assistant_message_id: "assistant-current".to_string(),
        expected_previous_summary_id: None,
        covered_through: ContextJournalCursor::message("assistant-old"),
        source_input_tokens: 5_000,
        retained_input_tokens: 0,
        uncovered_tail_input_tokens: 1_000,
        target_replacement_tokens: 750,
    };
    let receipt_plan = mycopilot_core::ContextCompactionReceiptPlan {
        context_revision: "0000000000000001".to_string(),
        persistent_revision: "0000000000000001".to_string(),
        request_input_tokens: 6_000,
        available_input_tokens: Some(6_000),
        request_trigger_input_tokens: Some(5_000),
        request_target_input_tokens: Some(1_000),
        source_input_tokens: 5_000,
        retained_input_tokens: 0,
        target_replacement_tokens: 750,
        expected_reclaimed_tokens: 4_250,
        planned_reclaimed_tokens: 4_250,
        projected_request_input_tokens: 1_750,
        best_effort: false,
        protected_input_tokens: 0,
        protected_reasons: std::collections::BTreeMap::new(),
        atomic_unit_count: 2,
        previous_summary_id: None,
        covered_through: prepare_request.covered_through.clone(),
    };
    let planned_receipt = mycopilot_core::ContextCompactionReceipt {
        schema_version: mycopilot_core::CONTEXT_COMPACTION_RECEIPT_SCHEMA_VERSION,
        operation_id: "operation-compaction-host".to_string(),
        run_id: prepare_request.run_id.clone(),
        conversation_id: prepare_request.conversation_id.clone(),
        assistant_message_id: prepare_request.assistant_message_id.clone(),
        request_index: 1,
        attempt_index: 1,
        model_config_id: None,
        model: "model-1".to_string(),
        provider_transition_source_model_display_name: None,
        provider_transition_target_model_display_name: None,
        api_style: mycopilot_core::AgentApiStyle::OpenAiCompatible,
        status: mycopilot_core::ContextCompactionReceiptStatus::InProgress,
        stage: mycopilot_core::ContextCompactionReceiptStage::Planned,
        plan: receipt_plan.clone(),
        source_revision: None,
        generation_observation_id: None,
        summary_id: None,
        result: None,
        error: None,
        started_at: 1,
        updated_at: 1,
        completed_at: None,
    };
    services
        .record_receipt(planned_receipt, None)
        .await
        .unwrap();
    let prefix = match services
        .prepare(prepare_request.clone(), cancellation.clone())
        .await
        .unwrap()
    {
        AgentContextCompactionPrepareOutcome::Ready(prefix) => prefix,
        AgentContextCompactionPrepareOutcome::Refresh(_) => {
            panic!("fresh plan unexpectedly required a refresh")
        }
    };
    let generated = services
        .generate(
            AgentContextCompactionGenerationRequest {
                operation_id: "operation-compaction-host".to_string(),
                run_id: prepare_request.run_id.clone(),
                conversation_id: prepare_request.conversation_id.clone(),
                assistant_message_id: prepare_request.assistant_message_id.clone(),
                request_index: 1,
                prefix: prefix.clone(),
                continuity: mycopilot_core::ContextContinuitySnapshot::from_prefix(&prefix)
                    .unwrap(),
                source_input_tokens: prepare_request.source_input_tokens,
                uncovered_tail_input_tokens: prepare_request.uncovered_tail_input_tokens,
                target_replacement_tokens: prepare_request.target_replacement_tokens,
            },
            cancellation.clone(),
        )
        .await
        .unwrap();
    let expected_summary_id = generated.draft.id.clone();
    let applied_receipt = mycopilot_core::ContextCompactionReceipt {
        schema_version: mycopilot_core::CONTEXT_COMPACTION_RECEIPT_SCHEMA_VERSION,
        operation_id: "operation-compaction-host".to_string(),
        run_id: prepare_request.run_id.clone(),
        conversation_id: prepare_request.conversation_id.clone(),
        assistant_message_id: prepare_request.assistant_message_id.clone(),
        request_index: 1,
        attempt_index: 1,
        model_config_id: None,
        model: "model-1".to_string(),
        provider_transition_source_model_display_name: None,
        provider_transition_target_model_display_name: None,
        api_style: mycopilot_core::AgentApiStyle::OpenAiCompatible,
        status: mycopilot_core::ContextCompactionReceiptStatus::Applied,
        stage: mycopilot_core::ContextCompactionReceiptStage::Completed,
        plan: receipt_plan,
        source_revision: Some(prefix.source_revision.clone()),
        generation_observation_id: Some(generated.observation.id.clone()),
        summary_id: Some(expected_summary_id.clone()),
        result: Some(mycopilot_core::ContextCompactionReceiptResult {
            summary_id: expected_summary_id.clone(),
            source_input_tokens: generated.draft.source_input_tokens,
            summary_input_tokens: generated.draft.summary_input_tokens,
            continuity_input_tokens: generated.draft.continuity_input_tokens,
            uncovered_tail_input_tokens: generated.draft.uncovered_tail_input_tokens,
            replacement_input_tokens: generated.draft.replacement_input_tokens,
            reclaimed_input_tokens: generated
                .draft
                .source_input_tokens
                .saturating_sub(generated.draft.replacement_input_tokens),
        }),
        error: None,
        started_at: 1,
        updated_at: now_ms(),
        completed_at: Some(now_ms()),
    };
    let committed = services
        .commit(
            AgentContextCompactionCommitRequest {
                run_id: prepare_request.run_id,
                conversation_id: prepare_request.conversation_id,
                assistant_message_id: prepare_request.assistant_message_id,
                prefix,
                draft: generated.draft,
                receipt: applied_receipt,
                observation: generated.observation,
            },
            cancellation,
        )
        .await
        .unwrap();
    match committed {
        AgentContextCompactionCommitOutcome::Applied { summary_id, .. } => {
            assert_eq!(summary_id, expected_summary_id)
        }
        AgentContextCompactionCommitOutcome::Refresh(_) => {
            panic!("fresh summary unexpectedly became stale")
        }
    }

    let active = storage
        .get_active_context_compaction_summary("conversation-compaction-host")
        .unwrap()
        .unwrap();
    assert_eq!(active.id, expected_summary_id);
    assert_eq!(
        active.covered_through,
        ContextJournalCursor::message("assistant-old")
    );
    assert_eq!(active.uncovered_tail_input_tokens, 1_000);
    assert_eq!(active.continuity.schema_version, 2);
    assert_eq!(
        storage
            .load_conversation("conversation-compaction-host")
            .unwrap()
            .unwrap()
            .messages
            .len(),
        4
    );
    let states = service
        .conversation_context_states
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let state = states.get("conversation-compaction-host").unwrap();
    assert_eq!(state.active_run_id.as_deref(), Some("run-compaction-host"));
    assert!(!state.terminal);
    drop(states);
    let context_event = receiver.try_recv().unwrap();
    assert_eq!(
        context_event["params"]["type"].as_str(),
        Some("context_window_updated")
    );
    assert_eq!(
        context_event["params"]["modelConfigId"].as_str(),
        Some("model-1")
    );
    assert!(context_event["params"]["snapshot"]["inputTokens"]
        .as_u64()
        .is_some_and(|tokens| tokens > 0));
}

#[test]
fn running_trace_commits_drive_monotonic_context_window_events() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    storage.save_model_settings(test_model_settings()).unwrap();
    storage
        .save_conversation(ChatConversationRecord {
            id: "conversation-live".to_string(),
            project_id: None,
            model_id: Some("model-1".to_string()),
            title: "Live trace".to_string(),
            messages: vec![
                ChatMessageRecord {
                    id: "user-live".to_string(),
                    role: "user".to_string(),
                    content: "Inspect the project".to_string(),
                    created_at: 1,
                    status: Some("sent".to_string()),
                    attachments: Vec::new(),
                    agent_run_json: None,
                    ui_state_json: None,
                },
                ChatMessageRecord {
                    id: "assistant-live".to_string(),
                    role: "assistant".to_string(),
                    content: String::new(),
                    created_at: 2,
                    status: Some("pending".to_string()),
                    attachments: Vec::new(),
                    agent_run_json: None,
                    ui_state_json: None,
                },
            ],
            created_at: 1,
            updated_at: 2,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
    let service = AgentService::new(storage.clone());
    let mut agent_input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "secret",
        "model": "model-1",
        "modelCapabilities": { "imageInput": false },
        "contextWindowTokens": 128000,
        "contextWindowIndicatorEnabled": true,
        "maxTokens": 30000,
        "skillActivation": {
            "activationRevision": "skill-activation-sha256-v1:live",
            "skills": [{
                "id": "workspace:project:live-review",
                "name": "live-review",
                "revision": "skill-package-sha256-v1:live",
                "source": "workspace:project",
                "instructions": "SKILL_LIVE_OVERLAY_MARKER",
                "sourceBytes": 25
            }]
        },
        "context": {
            "conversationId": "conversation-live",
            "projectId": null,
            "workspace": null,
            "permissions": {
                "read": "workspace_only",
                "write": "denied",
                "command": "require_approval",
                "commandSafety": "guarded",
                "patch": "require_approval",
                "builtinExecution": "require_approval"
            }
        },
        "messages": []
    }))
    .unwrap();
    freeze_test_pending_provider_configuration(&storage, &mut agent_input);
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let observer = service.trace_observer(
        "run-live",
        "conversation-live",
        "assistant-live",
        2,
        agent_input,
        RunContextToolProjection::pending(),
        notifications,
    );

    observer(ConversationTraceSnapshot::default()).unwrap();
    let initial = receiver.try_recv().unwrap();
    let initial_tokens = initial["params"]["snapshot"]["inputTokens"]
        .as_u64()
        .unwrap();
    assert!(initial_tokens > 0);

    let narration = ConversationTurnTraceItem::AssistantNarration {
        sequence: 0,
        content: "I will inspect the relevant files.".to_string(),
        truncated: false,
    };
    let narration_context = ConversationModelContextItem {
        sequence: 0,
        ordinal: 0,
        role: "assistant".to_string(),
        content: "I will inspect the relevant files.".to_string(),
        tool_call_id: None,
        tool_calls: Vec::new(),
        is_error: false,
    };
    observer(ConversationTraceSnapshot {
        items: vec![narration.clone()],
        model_context_items: vec![narration_context.clone()],
        next_sequence: 1,
        truncated: false,
    })
    .unwrap();
    let narrated = receiver.try_recv().unwrap();
    let narrated_tokens = narrated["params"]["snapshot"]["inputTokens"]
        .as_u64()
        .unwrap();
    assert!(narrated_tokens > initial_tokens);
    {
        let states = service
            .conversation_context_states
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let entry = states.get("conversation-live").unwrap();
        assert_eq!(entry.committed_activity_items, 1);
        assert!(!entry.terminal);
    }

    let canonical_call_id = format!("tc1_{}", "A".repeat(43));
    let call = ConversationTurnTraceItem::ToolCall {
        sequence: 1,
        call_id: canonical_call_id.clone(),
        tool: "image_generation".to_string(),
        operation: json!({
            "request": { "operation": "generate", "prompt": "private prompt" },
            "reason": "Create the requested image."
        }),
        provenance: AgentToolIdentity::Builtin {
            tool_name: "image_generation".to_string(),
        },
        approval_status: AgentApprovalStatus::NotRequired,
        truncated: false,
    };
    let call_context = ConversationModelContextItem {
        sequence: 1,
        ordinal: 0,
        role: "assistant".to_string(),
        content: String::new(),
        tool_call_id: None,
        tool_calls: vec![AgentContextCheckpointToolCall {
            id: canonical_call_id.clone(),
            name: "image_generation".to_string(),
            args: json!({
                "request": { "operation": "generate", "prompt": "private prompt" },
                "reason": "Create the requested image."
            }),
            provider_identity: AgentProviderToolCallIdentity {
                provider_tool_index: 0,
                provider_call_id: canonical_call_id.clone(),
                runtime_call_id: canonical_call_id.clone(),
            },
        }],
        is_error: false,
    };
    observer(ConversationTraceSnapshot {
        items: vec![narration.clone(), call.clone()],
        model_context_items: vec![narration_context.clone(), call_context.clone()],
        next_sequence: 2,
        truncated: false,
    })
    .unwrap();
    assert!(receiver.try_recv().is_err());
    assert_eq!(
        service
            .conversation_context_states
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get("conversation-live")
            .unwrap()
            .committed_activity_items,
        1
    );
    let durable_open_call = storage
        .get_conversation_turn_trace("assistant-live")
        .unwrap()
        .unwrap();
    assert_eq!(durable_open_call.items.len(), 2);
    assert!(matches!(
        durable_open_call.items.last(),
        Some(ConversationTurnTraceItem::ToolCall { call_id, .. })
            if call_id == &canonical_call_id
    ));

    let result_observation = json!({ "status": "succeeded" });
    let result = ConversationTurnTraceItem::ToolResult {
        sequence: 2,
        call_id: canonical_call_id.clone(),
        tool: "image_generation".to_string(),
        status: ConversationTraceToolResultStatus::Succeeded,
        success: true,
        observation: result_observation.clone(),
        approval_status: AgentApprovalStatus::NotRequired,
        error: None,
        truncated: false,
        archive: Default::default(),
    };
    let result_context = ConversationModelContextItem {
        sequence: 2,
        ordinal: 0,
        role: "tool".to_string(),
        content: result_observation.to_string(),
        tool_call_id: Some(canonical_call_id),
        tool_calls: Vec::new(),
        is_error: false,
    };
    observer(ConversationTraceSnapshot {
        items: vec![narration, call, result],
        model_context_items: vec![narration_context, call_context, result_context],
        next_sequence: 3,
        truncated: false,
    })
    .unwrap();
    let closed = receiver.try_recv().unwrap();
    let closed_tokens = closed["params"]["snapshot"]["inputTokens"]
        .as_u64()
        .unwrap();
    assert!(closed_tokens > narrated_tokens);

    let trace = storage
        .get_conversation_turn_trace("assistant-live")
        .unwrap()
        .unwrap();
    assert_eq!(
        trace.terminal_status,
        ConversationTurnTraceTerminalStatus::InProgress
    );
    assert_eq!(trace.items.len(), 3);
    assert_eq!(
        service
            .conversation_context_states
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get("conversation-live")
            .unwrap()
            .committed_activity_items,
        3
    );
}

#[test]
fn durable_trace_append_is_distinguished_from_a_failed_derived_context_refresh() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    storage.save_model_settings(test_model_settings()).unwrap();
    storage
        .save_conversation(ChatConversationRecord {
            id: "conversation-derived-refresh-failure".to_string(),
            project_id: None,
            model_id: Some("model-1".to_string()),
            title: "Derived refresh failure".to_string(),
            messages: vec![
                ChatMessageRecord {
                    id: "user-corrupt-history".to_string(),
                    role: "user".to_string(),
                    content: "Earlier request".to_string(),
                    created_at: 1,
                    status: Some("sent".to_string()),
                    attachments: Vec::new(),
                    agent_run_json: None,
                    ui_state_json: None,
                },
                // A terminal Assistant without its required Trace represents durable history
                // corruption. It is deliberately introduced here so only the rebuildable
                // context projection fails after the current run's append commits.
                ChatMessageRecord {
                    id: "assistant-corrupt-history".to_string(),
                    role: "assistant".to_string(),
                    content: "Earlier answer".to_string(),
                    created_at: 2,
                    status: Some("sent".to_string()),
                    attachments: Vec::new(),
                    agent_run_json: None,
                    ui_state_json: None,
                },
                ChatMessageRecord {
                    id: "user-derived-refresh-failure".to_string(),
                    role: "user".to_string(),
                    content: "Continue".to_string(),
                    created_at: 3,
                    status: Some("sent".to_string()),
                    attachments: Vec::new(),
                    agent_run_json: None,
                    ui_state_json: None,
                },
                ChatMessageRecord {
                    id: "assistant-derived-refresh-failure".to_string(),
                    role: "assistant".to_string(),
                    content: String::new(),
                    created_at: 4,
                    status: Some("pending".to_string()),
                    attachments: Vec::new(),
                    agent_run_json: None,
                    ui_state_json: None,
                },
            ],
            created_at: 1,
            updated_at: 4,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
    let service = AgentService::new(storage.clone());
    let mut agent_input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "secret",
        "model": "model-1",
        "modelCapabilities": { "imageInput": false },
        "contextWindowTokens": 128000,
        "contextWindowIndicatorEnabled": true,
        "maxTokens": 30000,
        "context": {
            "conversationId": "conversation-derived-refresh-failure",
            "projectId": null,
            "workspace": null,
            "permissions": {
                "read": "workspace_only",
                "write": "denied",
                "command": "require_approval",
                "commandSafety": "guarded",
                "patch": "require_approval",
                "builtinExecution": "require_approval"
            }
        },
        "messages": []
    }))
    .unwrap();
    freeze_test_pending_provider_configuration(&storage, &mut agent_input);
    let exact_request_snapshot = serde_json::from_value::<AgentContextWindowSnapshot>(json!({
        "model": "model-1",
        "status": "within_budget",
        "contextWindowTokens": 128000,
        "reservedOutputTokens": 30000,
        "safetyMarginTokens": 1024,
        "inputCapacityTokens": 96976,
        "inputTokens": 321,
        "costBreakdown": {
            "systemTokens": 0,
            "toolSchemaTokens": 0,
            "summaryTokens": 0,
            "worldStateTokens": 0,
            "todoTokens": 0,
            "providerContinuationTokens": 0,
            "recentHistoryTokens": 321,
            "totalInputTokens": 321
        },
        "remainingInputTokens": 96655
    }))
    .unwrap();
    service
        .running_context_window_snapshots
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .insert(
            "run-derived-refresh-failure".to_string(),
            exact_request_snapshot.clone(),
        );
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let observer = service.trace_observer(
        "run-derived-refresh-failure",
        "conversation-derived-refresh-failure",
        "assistant-derived-refresh-failure",
        4,
        agent_input,
        RunContextToolProjection::pending(),
        notifications,
    );
    let call_id = format!("tc1_{}", "D".repeat(43));
    let call = ConversationTurnTraceItem::ToolCall {
        sequence: 0,
        call_id: call_id.clone(),
        tool: "skills_prepare_install".to_string(),
        provenance: AgentToolIdentity::Builtin {
            tool_name: "skills_prepare_install".to_string(),
        },
        operation: json!({ "source": ".agents/skills/social" }),
        approval_status: AgentApprovalStatus::NotRequired,
        truncated: false,
    };
    let call_context = ConversationModelContextItem {
        sequence: 0,
        ordinal: 0,
        role: "assistant".to_string(),
        content: String::new(),
        tool_call_id: None,
        tool_calls: vec![AgentContextCheckpointToolCall {
            id: call_id.clone(),
            name: "skills_prepare_install".to_string(),
            args: json!({ "source": ".agents/skills/social" }),
            provider_identity: AgentProviderToolCallIdentity {
                provider_tool_index: 0,
                provider_call_id: call_id.clone(),
                runtime_call_id: call_id,
            },
        }],
        is_error: false,
    };

    let error = observer(ConversationTraceSnapshot {
        items: vec![call.clone()],
        model_context_items: vec![call_context.clone()],
        next_sequence: 1,
        truncated: false,
    })
    .unwrap_err();

    assert_eq!(
        error.code(),
        Some("conversation_trace_derived_context_failed")
    );
    assert_eq!(
        error
            .details()
            .and_then(|details| details.get("durableAppendCommitted"))
            .and_then(serde_json::Value::as_bool),
        Some(true)
    );
    let trace = storage
        .get_conversation_turn_trace("assistant-derived-refresh-failure")
        .unwrap()
        .expect("the authoritative trace append must remain committed");
    assert_eq!(trace.items, vec![call]);
    let model_context = storage
        .get_conversation_model_context_log("assistant-derived-refresh-failure")
        .unwrap()
        .expect("the authoritative model-context append must remain committed");
    assert_eq!(model_context.items, vec![call_context]);
    assert!(!service
        .conversation_context_states
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .contains_key("conversation-derived-refresh-failure"));
    assert_eq!(
        service
            .running_context_window_snapshots
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get("run-derived-refresh-failure"),
        Some(&exact_request_snapshot),
        "a failed approximation must not erase the last exact Provider request accounting"
    );
    assert!(
        receiver.try_recv().is_err(),
        "a failed derived refresh must not publish a misleading context-window update"
    );
}

#[test]
fn trace_observer_still_fails_closed_when_the_authoritative_append_does_not_commit() {
    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("storage.sqlite");
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    storage.save_model_settings(test_model_settings()).unwrap();
    storage
        .save_conversation(ChatConversationRecord {
            id: "conversation-trace-append-failure".to_string(),
            project_id: None,
            model_id: Some("model-1".to_string()),
            title: "Trace append failure".to_string(),
            messages: vec![
                ChatMessageRecord {
                    id: "user-trace-append-failure".to_string(),
                    role: "user".to_string(),
                    content: "Continue".to_string(),
                    created_at: 1,
                    status: Some("sent".to_string()),
                    attachments: Vec::new(),
                    agent_run_json: None,
                    ui_state_json: None,
                },
                ChatMessageRecord {
                    id: "assistant-trace-append-failure".to_string(),
                    role: "assistant".to_string(),
                    content: String::new(),
                    created_at: 2,
                    status: Some("pending".to_string()),
                    attachments: Vec::new(),
                    agent_run_json: None,
                    ui_state_json: None,
                },
            ],
            created_at: 1,
            updated_at: 2,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
    let service = AgentService::new(storage.clone());
    let mut agent_input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "secret",
        "model": "model-1",
        "modelCapabilities": { "imageInput": false },
        "contextWindowTokens": 128000,
        "contextWindowIndicatorEnabled": true,
        "maxTokens": 30000,
        "context": {
            "conversationId": "conversation-trace-append-failure",
            "projectId": null,
            "workspace": null,
            "permissions": {
                "read": "workspace_only",
                "write": "denied",
                "command": "require_approval",
                "commandSafety": "guarded",
                "patch": "require_approval",
                "builtinExecution": "require_approval"
            }
        },
        "messages": []
    }))
    .unwrap();
    freeze_test_pending_provider_configuration(&storage, &mut agent_input);
    rusqlite::Connection::open(&database_path)
        .unwrap()
        .execute_batch(
            "CREATE TRIGGER fail_test_in_progress_trace_append
             BEFORE INSERT ON conversation_turn_traces
             BEGIN
               SELECT RAISE(ABORT, 'injected trace append failure');
             END;",
        )
        .unwrap();
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let observer = service.trace_observer(
        "run-trace-append-failure",
        "conversation-trace-append-failure",
        "assistant-trace-append-failure",
        2,
        agent_input,
        RunContextToolProjection::pending(),
        notifications,
    );

    let error = observer(ConversationTraceSnapshot::default()).unwrap_err();

    assert_eq!(error.code(), Some("conversation_trace_persistence_failed"));
    assert_eq!(
        error
            .details()
            .and_then(|details| details.get("durableAppendCommitted"))
            .and_then(serde_json::Value::as_bool),
        Some(false)
    );
    assert!(storage
        .get_conversation_turn_trace("assistant-trace-append-failure")
        .unwrap()
        .is_none());
    assert!(!service
        .conversation_context_states
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .contains_key("conversation-trace-append-failure"));
    assert!(receiver.try_recv().is_err());
}

#[test]
fn terminal_cache_rebuild_drops_the_completed_run_skill_overlay() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    storage.save_model_settings(test_model_settings()).unwrap();
    storage
        .save_conversation(ChatConversationRecord {
            id: "conversation-terminal-skill".to_string(),
            project_id: None,
            model_id: Some("model-1".to_string()),
            title: "Terminal Skill".to_string(),
            messages: vec![
                ChatMessageRecord {
                    id: "user-terminal-skill".to_string(),
                    role: "user".to_string(),
                    content: "Review the completed run.".to_string(),
                    created_at: 1,
                    status: Some("sent".to_string()),
                    attachments: Vec::new(),
                    agent_run_json: None,
                    ui_state_json: None,
                },
                ChatMessageRecord {
                    id: "assistant-terminal-skill".to_string(),
                    role: "assistant".to_string(),
                    content: String::new(),
                    created_at: 2,
                    status: Some("pending".to_string()),
                    attachments: Vec::new(),
                    agent_run_json: None,
                    ui_state_json: None,
                },
            ],
            created_at: 1,
            updated_at: 2,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
    let trace = completed_conversation_trace_without_items(
        "run-terminal-skill",
        "conversation-terminal-skill",
        "assistant-terminal-skill",
    );
    storage
        .finalize_chat_message_with_conversation_trace(
            "conversation-terminal-skill",
            "assistant-terminal-skill",
            "The review is complete.",
            Some("sent"),
            "completed",
            &trace,
            2,
            3,
        )
        .unwrap();

    let service = AgentService::new(storage.clone());
    let mut agent_input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "secret",
        "model": "model-1",
        "modelCapabilities": { "imageInput": false },
        "contextWindowTokens": 128000,
        "contextWindowIndicatorEnabled": true,
        "maxTokens": 30000,
        "skillActivation": {
            "activationRevision": "skill-activation-sha256-v1:terminal",
            "skills": [{
                "id": "workspace:project:terminal-review",
                "name": "terminal-review",
                "revision": "skill-package-sha256-v1:terminal",
                "source": "workspace:project",
                "instructions": "SKILL_TERMINAL_OVERLAY_MARKER",
                "sourceBytes": 29
            }]
        },
        "context": {
            "conversationId": "conversation-terminal-skill",
            "projectId": null,
            "workspace": null,
            "permissions": {
                "read": "workspace_only",
                "write": "denied",
                "command": "require_approval",
                "commandSafety": "guarded",
                "patch": "require_approval",
                "builtinExecution": "require_approval"
            }
        },
        "messages": []
    }))
    .unwrap();
    freeze_test_pending_provider_configuration(&storage, &mut agent_input);

    let snapshot = service
        .finalize_conversation_context_state(
            &agent_input,
            "run-terminal-skill",
            "conversation-terminal-skill",
            "assistant-terminal-skill",
            "The review is complete.",
        )
        .unwrap()
        .unwrap();

    assert!(snapshot.input_tokens > 0);
}

#[test]
fn disabled_indicator_still_builds_runtime_context_baseline() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    storage.save_model_settings(test_model_settings()).unwrap();
    storage
        .save_conversation(ChatConversationRecord {
            id: "conversation-hidden-indicator".to_string(),
            project_id: None,
            model_id: Some("model-1".to_string()),
            title: "Hidden indicator".to_string(),
            messages: vec![
                ChatMessageRecord {
                    id: "user-hidden-indicator".to_string(),
                    role: "user".to_string(),
                    content: "Inspect the durable context".to_string(),
                    created_at: 1,
                    status: Some("sent".to_string()),
                    attachments: Vec::new(),
                    agent_run_json: None,
                    ui_state_json: None,
                },
                ChatMessageRecord {
                    id: "assistant-hidden-indicator".to_string(),
                    role: "assistant".to_string(),
                    content: String::new(),
                    created_at: 2,
                    status: Some("pending".to_string()),
                    attachments: Vec::new(),
                    agent_run_json: None,
                    ui_state_json: None,
                },
            ],
            created_at: 1,
            updated_at: 2,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
    let service = AgentService::new(storage.clone());
    let mut agent_input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "secret",
        "model": "model-1",
        "modelCapabilities": { "imageInput": false },
        "contextWindowTokens": 128000,
        "contextWindowIndicatorEnabled": false,
        "maxTokens": 30000,
        "context": {
            "conversationId": "conversation-hidden-indicator",
            "projectId": null,
            "workspace": null,
            "permissions": {
                "read": "workspace_only",
                "write": "denied",
                "command": "require_approval",
                "commandSafety": "guarded",
                "patch": "require_approval",
                "builtinExecution": "require_approval"
            }
        },
        "messages": []
    }))
    .unwrap();
    freeze_test_pending_provider_configuration(&storage, &mut agent_input);
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let observer = service.trace_observer(
        "run-hidden-indicator",
        "conversation-hidden-indicator",
        "assistant-hidden-indicator",
        2,
        agent_input.clone(),
        RunContextToolProjection::pending(),
        notifications,
    );

    let baseline = observer(ConversationTraceSnapshot::default()).unwrap();

    assert!(baseline.is_some());
    assert!(receiver.try_recv().is_err());
    assert!(service
        .conversation_context_states
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .contains_key("conversation-hidden-indicator"));
    let projection = service
        .context_window_tool_projection(&agent_input, None)
        .unwrap();
    assert!(service
        .context_window_snapshot_with_projection_cache(
            &agent_input,
            "conversation-hidden-indicator",
            &projection,
        )
        .unwrap()
        .is_none());
    assert!(service
        .conversation_context_states
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .contains_key("conversation-hidden-indicator"));
}

#[test]
fn deleting_messages_invalidates_the_conversation_context_state() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    storage.save_model_settings(test_model_settings()).unwrap();
    storage
        .save_conversation(ChatConversationRecord {
            id: "conversation-delete-context".to_string(),
            project_id: None,
            model_id: Some("model-1".to_string()),
            title: "Delete context".to_string(),
            messages: vec![ChatMessageRecord {
                id: "user-delete-context".to_string(),
                role: "user".to_string(),
                content: "Old durable content".to_string(),
                created_at: 1,
                status: Some("sent".to_string()),
                attachments: Vec::new(),
                agent_run_json: None,
                ui_state_json: None,
            }],
            created_at: 1,
            updated_at: 1,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
    let service = AgentService::new(storage.clone());
    let mut input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "secret",
        "model": "model-1",
        "modelCapabilities": { "imageInput": false },
        "contextWindowTokens": 128000,
        "contextWindowIndicatorEnabled": true,
        "maxTokens": 30000,
        "context": {
            "conversationId": "conversation-delete-context",
            "projectId": null,
            "workspace": null,
            "permissions": {
                "read": "workspace_only",
                "write": "denied",
                "command": "require_approval",
                "commandSafety": "guarded",
                "patch": "require_approval",
                "builtinExecution": "require_approval"
            }
        },
        "messages": []
    }))
    .unwrap();
    freeze_test_pending_provider_configuration(&storage, &mut input);

    let projection = service
        .context_window_tool_projection(&input, None)
        .unwrap();
    assert!(service
        .context_window_snapshot_with_projection_cache(
            &input,
            "conversation-delete-context",
            &projection,
        )
        .unwrap()
        .is_some());
    assert!(service
        .conversation_context_states
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .contains_key("conversation-delete-context"));

    service
        .delete_chat_messages(
            "conversation-delete-context",
            &["user-delete-context".to_string()],
        )
        .unwrap();

    assert!(!service
        .conversation_context_states
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .contains_key("conversation-delete-context"));
}
