use super::*;

#[test]
fn checkpoint_restore_requires_current_provider_protocol_revision_before_dispatch() {
    let (checkpoint, continuation) = restorable_checkpoint_fixture();

    for revision in [None, Some("model-settings-v1:old".to_string())] {
        let mut malformed = checkpoint.clone();
        malformed
            .provider_protocol_key
            .provider_configuration_revision = revision;
        let error = restore_error(restore_run_checkpoint(
            malformed,
            "checkpoint-validation-run",
            &continuation,
        ));
        assert!(error.to_string().contains("Provider Protocol revision"));
    }
}

#[test]
fn mcp_continuation_keeps_live_model_result_but_redacts_durable_trace_and_checkpoint() {
    const RESULT_CANARY: &str = "MCP_RESULT_CANARY_MUST_NOT_PERSIST";
    let model_tool_name = "mcp__fixture__secret_result".to_string();
    let (mut checkpoint, mut continuation) =
        restorable_checkpoint_fixture_for_pending_tool(&model_tool_name);
    checkpoint.pending_action_id = Some(uuid::Uuid::new_v4().to_string());
    checkpoint
        .tool_set
        .exposed_tool_names
        .push(model_tool_name.clone());
    checkpoint.tool_set.exposed_tool_names.sort();
    checkpoint.tool_set.exposed_tool_names.dedup();
    continuation.result.result = Some(json!({
        "value": RESULT_CANARY,
        "neutral": {"data": RESULT_CANARY},
    }));

    let restored =
        restore_run_checkpoint(checkpoint, "checkpoint-validation-run", &continuation).unwrap();
    let live_messages = restored.context.to_messages();
    assert!(
        live_messages
            .iter()
            .any(|message| message.content().contains(RESULT_CANARY)),
        "the current process must still supply the bounded authoritative result to the model"
    );
    let mut live_context_items = restored.context.checkpoint_items().unwrap();

    let (trace_items, model_items, _, _) = restored.conversation_trace.checkpoint();
    assert!(!serde_json::to_string(&trace_items)
        .unwrap()
        .contains(RESULT_CANARY));
    assert!(!serde_json::to_string(&model_items)
        .unwrap()
        .contains(RESULT_CANARY));

    project_mcp_result_context_for_checkpoint(&mut live_context_items);
    let durable_context = serde_json::to_string(&live_context_items).unwrap();
    assert!(!durable_context.contains(RESULT_CANARY));
    assert!(!durable_context.contains(MCP_DURABLE_RESULT_PLACEHOLDER));
    assert!(durable_context.contains(r#"\"status\":\"completed\""#));
    assert!(durable_context.contains(r#"\"outcome\":\"succeeded\""#));
    assert!(durable_context.contains(r#"\"contentOmitted\":true"#));
}

#[test]
fn checkpoint_v2_is_rejected_without_attempting_identity_migration() {
    let (mut checkpoint, continuation) = restorable_checkpoint_fixture();
    checkpoint.version = 2;
    checkpoint.pending_tool_call_id = "legacy/provider/call".to_string();

    let error = restore_error(restore_run_checkpoint(
        checkpoint,
        "checkpoint-validation-run",
        &continuation,
    ));

    assert!(error.to_string().contains("不支持版本 2"));
    assert!(error
        .to_string()
        .contains(&format!("当前版本为 {AGENT_RUN_CHECKPOINT_SCHEMA_VERSION}")));
}

#[test]
fn checkpoint_v8_skill_barrier_shape_is_rejected_instead_of_reinterpreted() {
    let (mut checkpoint, continuation) = restorable_checkpoint_fixture();
    checkpoint.version = 8;

    let error = restore_error(restore_run_checkpoint(
        checkpoint,
        "checkpoint-validation-run",
        &continuation,
    ));

    assert!(error.to_string().contains("不支持版本 8"));
    assert!(error
        .to_string()
        .contains(&format!("当前版本为 {AGENT_RUN_CHECKPOINT_SCHEMA_VERSION}")));
}

#[test]
fn checkpoint_restore_rejects_unknown_frozen_provider_registration() {
    let (mut checkpoint, continuation) = restorable_checkpoint_fixture();
    checkpoint.provider_profile_config.profile.version = 99;
    checkpoint.provider_protocol_key.profile.version = 99;

    let error = restore_error(restore_run_checkpoint(
        checkpoint,
        "checkpoint-validation-run",
        &continuation,
    ));

    assert_eq!(
        error.to_string(),
        "无法恢复运行检查点：Provider profile 无效：unsupported provider profile generic_openai_chat version 99"
    );
}

#[test]
fn checkpoint_restore_rejects_oversized_raw_provider_call_identity() {
    let (mut checkpoint, continuation) = restorable_checkpoint_fixture();
    checkpoint.assistant_turn_identity.tool_call_identities[0].provider_call_id =
        "x".repeat(crate::llm::MAX_PROVIDER_TOOL_CALL_ID_BYTES + 1);

    let error = restore_error(restore_run_checkpoint(
        checkpoint,
        "checkpoint-validation-run",
        &continuation,
    ));

    assert_eq!(error.code(), Some("agent.invalid_provider_tool_call_id"));
}

#[test]
fn approval_restore_preserves_frozen_run_authority_and_capabilities() {
    let (mut checkpoint, continuation) = restorable_checkpoint_fixture();
    let frozen_context = AgentRunContext {
        collaboration_identity: None,
        conversation_id: Some("conversation-frozen".to_string()),
        project_id: Some("project-frozen".to_string()),
        workspace: Some(crate::protocol::AgentWorkspaceContext {
            project_id: Some("project-frozen".to_string()),
            display_name: Some("Frozen workspace".to_string()),
            root_path: Some("/frozen/workspace".to_string()),
        }),
        attachment_library: None,
        permissions: crate::protocol::AgentPermissions {
            read: crate::protocol::AgentReadPermission::All,
            write: crate::protocol::AgentWritePermission::All,
            ..crate::protocol::AgentPermissions::default()
        },
    };
    checkpoint.run_context = Some(frozen_context.clone());
    checkpoint.model_capabilities = ModelCapabilities { image_input: true };
    checkpoint.run_world_state = test_run_world_state_for(true);

    let restored =
        restore_run_checkpoint(checkpoint, "checkpoint-validation-run", &continuation).unwrap();

    assert_eq!(restored.run_context, Some(frozen_context));
    assert_eq!(
        restored.model_capabilities,
        ModelCapabilities { image_input: true }
    );
    assert_eq!(
        restored
            .run_world_state
            .section(&WorldStateSectionId::ModelCapabilities)
            .unwrap()
            .state["imageInput"],
        true
    );
}

#[test]
fn approval_restore_keeps_backend_history_metadata_out_of_the_model_result() {
    let (checkpoint, continuation) = restorable_checkpoint_fixture();
    let restored = restore_run_checkpoint_with_history_ref(
        checkpoint,
        "checkpoint-validation-run",
        &continuation,
        Some("assistant-approval"),
    )
    .unwrap();
    let messages = restored.context.to_messages();
    let observation = messages.last().unwrap().content();

    assert!(!observation.contains("historyRef"));
    assert!(!observation.contains("assistantMessageId"));
    assert!(!observation.contains("\"callId\""));
}

#[test]
fn approval_restore_rejects_capability_and_world_state_mismatch() {
    let (mut checkpoint, continuation) = restorable_checkpoint_fixture();
    checkpoint.model_capabilities = ModelCapabilities { image_input: true };

    let error = restore_error(restore_run_checkpoint(
        checkpoint,
        "checkpoint-validation-run",
        &continuation,
    ));

    assert!(error.to_string().contains("模型能力"));
    assert!(error.to_string().contains("不一致"));
}

#[test]
fn one_model_response_claims_reason_only_duplicates_once() {
    let mut batch = ToolCallBatch::from_model_response(
        "claim-run",
        0,
        String::new(),
        vec![
            LlmToolCall {
                id: canonical_test_call_id(0, "claim-first"),
                name: "write_file".to_string(),
                args: json!({
                    "filePath": "report.txt",
                    "content": "same",
                    "reason": "Create the report"
                }),
            },
            LlmToolCall {
                id: canonical_test_call_id(1, "claim-duplicate"),
                name: "write_file".to_string(),
                args: json!({
                    "reason": "Write the requested file",
                    "content": "same",
                    "filePath": "report.txt"
                }),
            },
        ],
        false,
        |call| (call.clone(), AgentToolCallCheckpointPersistence::Allowed),
    );
    let first = batch.pop_front().unwrap();
    let duplicate = batch.pop_front().unwrap();

    assert_eq!(batch.claim(&first.call), ToolCallBatchClaim::Execute);
    assert!(matches!(
        batch.claim(&duplicate.call),
        ToolCallBatchClaim::Duplicate { .. }
    ));
}

#[test]
fn approval_restore_reconstructs_all_seen_calls_from_the_same_model_response() {
    let first = LlmToolCall {
        id: canonical_test_call_id(0, "restore-first"),
        name: "read_file".to_string(),
        args: json!({ "path": "source.txt", "reason": "Read source" }),
    };
    let pending = LlmToolCall {
        id: canonical_test_call_id(1, "restore-pending"),
        name: "write_file".to_string(),
        args: json!({
            "filePath": "report.txt",
            "content": "draft",
            "reason": "Write report"
        }),
    };
    let duplicate_first = LlmToolCall {
        id: canonical_test_call_id(2, "restore-duplicate"),
        name: "read_file".to_string(),
        args: json!({ "reason": "Read it again", "path": "source.txt" }),
    };
    let mut batch = ToolCallBatch::from_model_response(
        "checkpoint-validation-run",
        0,
        String::new(),
        vec![first.clone(), pending.clone(), duplicate_first],
        false,
        |call| (call.clone(), AgentToolCallCheckpointPersistence::Allowed),
    );
    let first = batch.pop_front().unwrap();
    assert_eq!(batch.claim(&first.call), ToolCallBatchClaim::Execute);
    let pending_queued = batch.pop_front().unwrap();
    assert_eq!(
        batch.claim(&pending_queued.call),
        ToolCallBatchClaim::Execute
    );

    let batch_group = first.context_group();
    assert_eq!(batch_group, pending_queued.context_group());
    let complete_turn = batch
        .take_assistant_turn()
        .expect("new model response owns one complete assistant turn");
    let context = ContextFrame::new(vec![
        ContextItem::new(
            LlmMessage::from_assistant_turn(complete_turn),
            ContextMetadata::new(
                ContextSource::ModelResponse,
                ContextScope::Run,
                ContextRetention::Retained,
            )
            .with_group(batch_group.clone()),
        ),
        ContextItem::tool_result(
            first.call.id.clone(),
            "{}",
            false,
            ContextMetadata::new(
                ContextSource::ToolResult,
                ContextScope::Run,
                ContextRetention::Retained,
            )
            .with_group(batch_group),
        ),
    ]);
    let first_trace_call = AgentToolCall {
        id: first.call.id.clone(),
        tool: first.call.name.clone(),
        args: first.call.args.clone(),
        approval_status: AgentApprovalStatus::NotRequired,
        reason: None,
    };
    let pending_trace_call = AgentToolCall {
        id: pending_queued.call.id.clone(),
        tool: pending_queued.call.name.clone(),
        args: pending_queued.call.args.clone(),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };
    let mut trace = ConversationTraceRecorder::default();
    record_current_test_tool_call(&mut trace, &batch, &first_trace_call);
    let first_result = AgentToolResult {
        exact_archive_file: None,
        call_id: first_trace_call.id.clone(),
        tool: first_trace_call.tool.clone(),
        ok: true,
        result: Some(json!({})),
        error: None,
    };
    let first_result_sequence = trace
        .record_tool_result(&first_trace_call, &first_result)
        .expect("current completed call has one result");
    trace
        .record_model_message(
            first_result_sequence,
            0,
            &LlmMessage::tool_result(first_trace_call.id.clone(), "{}", false),
        )
        .expect("current completed call result has immutable model context");
    record_current_test_tool_call(&mut trace, &batch, &pending_trace_call);
    let checkpoint = create_run_checkpoint(
        "checkpoint-validation-run",
        RunCheckpointState {
            context: &context,
            next_model_request_index: 1,
            tool_batch: &batch,
            extension_snapshots: Vec::new(),
            pending_tool_call_id: &pending.id,
            conversation_trace: &trace,
            tool_set: &test_tool_set(),
            run_context: None,
            collaboration_run_snapshot: None,
            model_capabilities: ModelCapabilities::default(),
            run_world_state: &test_run_world_state(),
            provider_profile_config: &test_provider_profile(),
            provider_protocol_key: &test_provider_key(),
        },
    )
    .unwrap();
    assert_eq!(
        checkpoint
            .assistant_turn_identity
            .tool_call_identities
            .len(),
        3
    );
    assert_eq!(checkpoint.context_items[0].tool_calls.len(), 3);
    assert_eq!(
        checkpoint.context_items[0].tool_calls[0]
            .provider_identity
            .provider_call_id,
        first.call.id.as_str()
    );
    assert_eq!(
        checkpoint.context_items[0].tool_calls[1]
            .provider_identity
            .provider_call_id,
        pending_queued.call.id.as_str()
    );
    assert_eq!(
        checkpoint.context_items[0].tool_calls[0]
            .provider_identity
            .provider_tool_index,
        0
    );
    assert_eq!(
        checkpoint.context_items[0].tool_calls[1]
            .provider_identity
            .provider_tool_index,
        1
    );
    assert_eq!(
        checkpoint.context_items[1].tool_call_id.as_deref(),
        Some(first.call.id.as_str())
    );
    assert_eq!(checkpoint.queued_tool_calls.len(), 1);
    assert_eq!(
        checkpoint.queued_tool_calls[0].call.id,
        checkpoint.assistant_turn_identity.tool_call_identities[2].runtime_call_id
    );
    let continuation = AgentToolContinuation {
        call: AgentToolCall {
            id: pending.id.clone(),
            tool: pending.name.clone(),
            args: pending.args.clone(),
            approval_status: AgentApprovalStatus::Approved,
            reason: None,
        },
        result: AgentToolResult {
            exact_archive_file: None,
            call_id: pending.id,
            tool: pending.name,
            ok: true,
            result: Some(json!({ "status": "written" })),
            error: None,
        },
    };

    let mut restored =
        restore_run_checkpoint(checkpoint, "checkpoint-validation-run", &continuation).unwrap();
    let queued_duplicate = restored.tool_batch.pop_front().unwrap();
    assert!(matches!(
        restored.tool_batch.claim(&queued_duplicate.call),
        ToolCallBatchClaim::Duplicate { .. }
    ));
}

#[test]
fn approval_checkpoint_uses_provider_index_when_provider_call_ids_repeat() {
    let provider_calls = vec![
        LlmToolCall {
            id: "provider-reused-id".to_string(),
            name: "write_file".to_string(),
            args: json!({ "path": "report.txt" }),
        },
        LlmToolCall {
            id: "provider-reused-id".to_string(),
            name: "read_file".to_string(),
            args: json!({ "path": "report.txt" }),
        },
    ];
    let runtime_calls = [
        LlmToolCall {
            id: canonical_test_call_id(0, "provider-reused-id"),
            name: "write_file".to_string(),
            args: json!({ "path": "report.txt" }),
        },
        LlmToolCall {
            id: canonical_test_call_id(1, "provider-reused-id"),
            name: "read_file".to_string(),
            args: json!({ "path": "report.txt" }),
        },
    ];
    let bindings = provider_calls
        .iter()
        .zip(runtime_calls.iter().cloned())
        .enumerate()
        .map(|(index, (provider_call, runtime_call))| {
            LlmRuntimeToolCallBinding::new(index, provider_call, runtime_call)
        })
        .collect::<Vec<_>>();
    let turn = LlmAssistantTurn::from_split_projection("", provider_calls);
    let mut batch = ToolCallBatch::from_provider_response(
        "checkpoint-validation-run",
        0,
        turn,
        bindings,
        false,
        |call| (call.clone(), AgentToolCallCheckpointPersistence::Allowed),
    )
    .unwrap();
    let checkpoint_message = batch.checkpoint_assistant_message().unwrap().unwrap();
    let group = batch.context_group().unwrap();
    let complete_turn = batch.take_assistant_turn().unwrap();
    let pending = pop_test_call(&mut batch, &runtime_calls[0].id);
    let context = ContextFrame::new(vec![ContextItem::new(
        LlmMessage::from_assistant_turn(complete_turn),
        ContextMetadata::new(
            ContextSource::ModelResponse,
            ContextScope::Run,
            ContextRetention::Retained,
        )
        .with_group(group),
    )
    .with_checkpoint_message(checkpoint_message)]);
    let checkpoint = create_run_checkpoint(
        "checkpoint-validation-run",
        RunCheckpointState {
            context: &context,
            next_model_request_index: 1,
            tool_batch: &batch,
            extension_snapshots: Vec::new(),
            pending_tool_call_id: &pending.call.id,
            conversation_trace: &ConversationTraceRecorder::default(),
            tool_set: &test_tool_set(),
            run_context: None,
            collaboration_run_snapshot: None,
            model_capabilities: ModelCapabilities::default(),
            run_world_state: &test_run_world_state(),
            provider_profile_config: &test_provider_profile(),
            provider_protocol_key: &test_provider_key(),
        },
    )
    .unwrap();
    assert_eq!(
        checkpoint
            .assistant_turn_identity
            .tool_call_identities
            .iter()
            .map(|identity| identity.provider_call_id.as_str())
            .collect::<Vec<_>>(),
        vec!["provider-reused-id", "provider-reused-id"]
    );
    assert_ne!(
        checkpoint.assistant_turn_identity.tool_call_identities[0].runtime_call_id,
        checkpoint.assistant_turn_identity.tool_call_identities[1].runtime_call_id
    );
}

#[test]
fn checkpoint_creation_rejects_invalid_pending_queued_context_and_trace_ids() {
    let invalid_id = "legacy/provider/call".to_string();
    let pending = LlmToolCall {
        id: invalid_id.clone(),
        name: "write_file".to_string(),
        args: json!({ "path": "report.txt" }),
    };
    let (mut batch, assistant_item) =
        test_batch_and_context_item("create-invalid-pending", "", vec![pending.clone()], false);
    pop_test_call(&mut batch, &pending.id);
    let context = ContextFrame::new(vec![assistant_item]);
    let trace = current_test_pending_trace(&batch, &pending);
    let error = create_run_checkpoint(
        "create-invalid-pending",
        RunCheckpointState {
            context: &context,
            next_model_request_index: 1,
            tool_batch: &batch,
            extension_snapshots: Vec::new(),
            pending_tool_call_id: &pending.id,
            conversation_trace: &trace,
            tool_set: &test_tool_set(),
            run_context: None,
            collaboration_run_snapshot: None,
            model_capabilities: ModelCapabilities::default(),
            run_world_state: &test_run_world_state(),
            provider_profile_config: &test_provider_profile(),
            provider_protocol_key: &test_provider_key(),
        },
    )
    .unwrap_err();
    assert_invalid_tool_call_id(error);

    let valid_pending = LlmToolCall {
        id: canonical_test_call_id(0, "create-valid-pending"),
        name: "write_file".to_string(),
        args: json!({ "path": "report.txt" }),
    };
    let (mut invalid_queue, valid_pending_item) = test_batch_and_context_item(
        "create-invalid-queue",
        "",
        vec![
            valid_pending.clone(),
            LlmToolCall {
                id: invalid_id.clone(),
                name: "read_file".to_string(),
                args: json!({ "path": "report.txt" }),
            },
        ],
        false,
    );
    pop_test_call(&mut invalid_queue, &valid_pending.id);
    let valid_pending_context = ContextFrame::new(vec![valid_pending_item]);
    let error = create_run_checkpoint(
        "create-invalid-queue",
        RunCheckpointState {
            context: &valid_pending_context,
            next_model_request_index: 1,
            tool_batch: &invalid_queue,
            extension_snapshots: Vec::new(),
            pending_tool_call_id: &valid_pending.id,
            conversation_trace: &trace,
            tool_set: &test_tool_set(),
            run_context: None,
            collaboration_run_snapshot: None,
            model_capabilities: ModelCapabilities::default(),
            run_world_state: &test_run_world_state(),
            provider_profile_config: &test_provider_profile(),
            provider_protocol_key: &test_provider_key(),
        },
    )
    .unwrap_err();
    assert_invalid_tool_call_id(error);

    let (mut valid_batch, valid_pending_item) = test_batch_and_context_item(
        "create-valid-pending",
        "",
        vec![valid_pending.clone()],
        false,
    );
    pop_test_call(&mut valid_batch, &valid_pending.id);
    let valid_pending_context = ContextFrame::new(vec![valid_pending_item.clone()]);

    let historical_group = ContextGroup::tool_exchange("invalid-history");
    let invalid_context = ContextFrame::new(vec![
        ContextItem::assistant(
            "",
            vec![LlmToolCall {
                id: invalid_id.clone(),
                name: "read_file".to_string(),
                args: json!({ "path": "old.txt" }),
            }],
            ContextMetadata::new(
                ContextSource::ModelResponse,
                ContextScope::Run,
                ContextRetention::Retained,
            )
            .with_group(historical_group.clone()),
        ),
        ContextItem::tool_result(
            invalid_id.clone(),
            "{}",
            false,
            ContextMetadata::new(
                ContextSource::ToolResult,
                ContextScope::Run,
                ContextRetention::Retained,
            )
            .with_group(historical_group),
        ),
        valid_pending_item,
    ]);
    let error = create_run_checkpoint(
        "create-invalid-context",
        RunCheckpointState {
            context: &invalid_context,
            next_model_request_index: 1,
            tool_batch: &valid_batch,
            extension_snapshots: Vec::new(),
            pending_tool_call_id: &valid_pending.id,
            conversation_trace: &trace,
            tool_set: &test_tool_set(),
            run_context: None,
            collaboration_run_snapshot: None,
            model_capabilities: ModelCapabilities::default(),
            run_world_state: &test_run_world_state(),
            provider_profile_config: &test_provider_profile(),
            provider_protocol_key: &test_provider_key(),
        },
    )
    .unwrap_err();
    assert_invalid_tool_call_id(error);

    let mut invalid_trace = ConversationTraceRecorder::default();
    invalid_trace.record_tool_call(&AgentToolCall {
        id: invalid_id,
        tool: "read_file".to_string(),
        args: json!({ "path": "old.txt" }),
        approval_status: AgentApprovalStatus::NotRequired,
        reason: None,
    });
    let error = create_run_checkpoint(
        "create-invalid-trace",
        RunCheckpointState {
            context: &valid_pending_context,
            next_model_request_index: 1,
            tool_batch: &valid_batch,
            extension_snapshots: Vec::new(),
            pending_tool_call_id: &valid_pending.id,
            conversation_trace: &invalid_trace,
            tool_set: &test_tool_set(),
            run_context: None,
            collaboration_run_snapshot: None,
            model_capabilities: ModelCapabilities::default(),
            run_world_state: &test_run_world_state(),
            provider_profile_config: &test_provider_profile(),
            provider_protocol_key: &test_provider_key(),
        },
    )
    .unwrap_err();
    assert_invalid_tool_call_id(error);
}

#[test]
fn checkpoint_restore_validates_every_persisted_and_continuation_id() {
    let (checkpoint, continuation) = restorable_checkpoint_fixture();

    let mut invalid_pending = checkpoint.clone();
    invalid_pending.pending_tool_call_id = "legacy/provider/pending".to_string();
    assert_invalid_tool_call_id(restore_error(restore_run_checkpoint(
        invalid_pending,
        "checkpoint-validation-run",
        &continuation,
    )));

    let mut invalid_context = checkpoint.clone();
    invalid_context.context_items[0].tool_calls[0].id = "legacy/provider/context".to_string();
    assert_invalid_tool_call_id(restore_error(restore_run_checkpoint(
        invalid_context,
        "checkpoint-validation-run",
        &continuation,
    )));

    let mut invalid_queue = checkpoint.clone();
    invalid_queue.queued_tool_calls[0].call.id = "legacy/provider/queued".to_string();
    assert_invalid_tool_call_id(restore_error(restore_run_checkpoint(
        invalid_queue,
        "checkpoint-validation-run",
        &continuation,
    )));

    let mut invalid_trace = checkpoint.clone();
    invalid_trace
        .conversation_trace_items
        .push(ConversationTurnTraceItem::ToolCall {
            sequence: 0,
            call_id: "legacy/provider/trace".to_string(),
            tool: "read_file".to_string(),
            provenance: crate::AgentToolIdentity::Builtin {
                tool_name: "read_file".to_string(),
            },
            operation: json!({ "path": "old.txt" }),
            approval_status: AgentApprovalStatus::NotRequired,
            truncated: false,
        });
    assert_invalid_tool_call_id(restore_error(restore_run_checkpoint(
        invalid_trace,
        "checkpoint-validation-run",
        &continuation,
    )));

    let mut invalid_continuation_call = continuation.clone();
    invalid_continuation_call.call.id = "legacy/provider/continuation".to_string();
    assert_invalid_tool_call_id(restore_error(restore_run_checkpoint(
        checkpoint.clone(),
        "checkpoint-validation-run",
        &invalid_continuation_call,
    )));

    let mut invalid_continuation_result = continuation.clone();
    invalid_continuation_result.result.call_id = "legacy/provider/result".to_string();
    assert_invalid_tool_call_id(restore_error(restore_run_checkpoint(
        checkpoint,
        "checkpoint-validation-run",
        &invalid_continuation_result,
    )));
}

#[test]
fn checkpoint_restore_rejects_a_valid_but_mismatched_continuation_result_id() {
    let (checkpoint, mut continuation) = restorable_checkpoint_fixture();
    continuation.result.call_id = canonical_test_call_id(9, "different-result");

    let error = restore_error(restore_run_checkpoint(
        checkpoint,
        "checkpoint-validation-run",
        &continuation,
    ));

    assert!(error.to_string().contains("续跑调用"));
    assert!(error.to_string().contains("工具结果"));
    assert!(error.to_string().contains("不一致"));
}

#[test]
fn checkpoint_restore_rejects_changed_call_tool_args_and_result_tool() {
    let (checkpoint, continuation) = restorable_checkpoint_fixture();

    let mut changed_tool = continuation.clone();
    changed_tool.call.tool = "run_command".to_string();
    changed_tool.result.tool = "run_command".to_string();
    let error = restore_error(restore_run_checkpoint(
        checkpoint.clone(),
        "checkpoint-validation-run",
        &changed_tool,
    ));
    assert!(error.to_string().contains("工具或参数"));

    let mut changed_args = continuation.clone();
    changed_args.call.args = json!({ "path": "different.txt" });
    let error = restore_error(restore_run_checkpoint(
        checkpoint.clone(),
        "checkpoint-validation-run",
        &changed_args,
    ));
    assert!(error.to_string().contains("工具或参数"));

    let mut changed_result_tool = continuation;
    changed_result_tool.result.tool = "run_command".to_string();
    let error = restore_error(restore_run_checkpoint(
        checkpoint,
        "checkpoint-validation-run",
        &changed_result_tool,
    ));
    assert!(error.to_string().contains("续跑调用工具"));
    assert!(error.to_string().contains("工具结果"));
}
