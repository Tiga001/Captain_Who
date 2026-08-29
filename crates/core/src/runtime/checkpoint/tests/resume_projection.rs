use super::*;

#[test]
fn checkpoint_round_trip_preserves_skill_activation_siblings_behind_approval() {
    let pending = LlmToolCall {
        id: canonical_test_call_id(0, "round-trip-pending"),
        name: "apply_patch".to_string(),
        args: apply_patch_args(json!({
            "action": "commit",
            "transactionId": "transaction-round-trip",
            "expectedDraftRevision": 1
        })),
    };
    let activation_call_id = canonical_test_call_id(1, "round-trip-activate");
    let queued_call_id = canonical_test_call_id(2, "round-trip-queued");
    let (mut batch, assistant_item) = test_batch_and_context_item(
        "run-1",
        "",
        vec![
            pending.clone(),
            LlmToolCall {
                id: activation_call_id.clone(),
                name: "skills_activate".to_string(),
                args: json!({
                    "skillRef": "s_000000000000000000000000",
                    "reason": "Use the frozen Skill selection after approval"
                }),
            },
            LlmToolCall {
                id: queued_call_id.clone(),
                name: "read_file".to_string(),
                args: json!({ "path": "report.txt" }),
            },
        ],
        true,
    );
    pop_test_call(&mut batch, &pending.id);
    let context = ContextFrame::new(vec![
        ContextItem::text(
            LlmMessageRole::System,
            "rules",
            ContextSource::BackendSystemPrompt,
            ContextScope::Run,
            ContextRetention::Retained,
        ),
        assistant_item,
    ]);
    let trace = current_test_pending_trace(&batch, &pending);
    let checkpoint = create_run_checkpoint(
        "run-1",
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
    let continuation = AgentToolContinuation {
        call: AgentToolCall {
            id: pending.id.clone(),
            tool: "apply_patch".to_string(),
            args: apply_patch_args(json!({
                "action": "commit",
                "transactionId": "transaction-round-trip",
                "expectedDraftRevision": 1
            })),
            approval_status: AgentApprovalStatus::Approved,
            reason: None,
        },
        result: AgentToolResult {
            exact_archive_file: None,
            call_id: pending.id,
            tool: "apply_patch".to_string(),
            ok: true,
            result: Some(json!({ "status": "applied" })),
            error: None,
        },
    };

    let mut restored = restore_run_checkpoint(checkpoint, "run-1", &continuation).unwrap();

    restored
        .context
        .validate_pending_tool_batch(&activation_call_id, std::slice::from_ref(&queued_call_id))
        .unwrap();
    assert_eq!(restored.next_model_request_index, 1);
    assert!(!restored.tool_batch.take_suppressed_narration());
    let activation = restored.tool_batch.pop_front().unwrap();
    assert_eq!(activation.call.id, activation_call_id);
    assert_eq!(activation.call.name, "skills_activate");
    assert!(!restored.tool_batch.take_suppressed_narration());
    let queued = restored.tool_batch.pop_front().unwrap();
    assert_eq!(queued.call.id, queued_call_id);
    assert!(restored.tool_batch.take_suppressed_narration());
}

#[test]
fn approval_resume_preserves_failed_command_observation_for_the_model() {
    let pending = LlmToolCall {
        id: canonical_test_call_id(0, "failed-command-pending"),
        name: "run_command".to_string(),
        args: json!({ "command": "python3 -c 'import openpyxl'" }),
    };
    let (mut batch, assistant_item) =
        test_batch_and_context_item("run-command", "", vec![pending.clone()], false);
    pop_test_call(&mut batch, &pending.id);
    let context = ContextFrame::new(vec![assistant_item]);
    let trace = current_test_pending_trace(&batch, &pending);
    let checkpoint = create_run_checkpoint(
        "run-command",
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
            ok: false,
            result: Some(json!({
                "exitCode": 1,
                "stdout": "dependency check started",
                "stderr": "ModuleNotFoundError: No module named 'openpyxl'",
                "timedOut": false,
                "cancelled": false,
                "stdoutTruncated": false,
                "stderrTruncated": false,
            })),
            error: Some("命令执行失败。".to_string()),
        },
    };

    let restored = restore_run_checkpoint(checkpoint, "run-command", &continuation).unwrap();

    restored.context.validate_complete_tool_protocol().unwrap();
    let messages = restored.context.to_messages();
    let observation = messages.last().expect("restored tool observation");
    assert_eq!(observation.role(), LlmMessageRole::Tool);
    assert!(observation.is_error());
    assert!(observation.content().contains("\"exitCode\":1"));
    assert!(observation.content().contains("dependency check started"));
    assert!(observation.content().contains("ModuleNotFoundError"));
    assert!(!observation.content().contains("\"stdoutTruncated\""));
    assert!(!observation.content().contains("\"stderrTruncated\""));
}

#[test]
fn approval_resume_preserves_compacted_context_without_restoring_raw_history() {
    let pending = LlmToolCall {
        id: canonical_test_call_id(0, "compaction-pending"),
        name: "apply_patch".to_string(),
        args: apply_patch_args(json!({
            "action": "commit",
            "transactionId": "transaction-compacted",
            "expectedDraftRevision": 1
        })),
    };
    let (mut tool_batch, assistant_item) = test_batch_and_context_item(
        "run-compacted",
        "I will write the report.",
        vec![pending.clone()],
        false,
    );
    pop_test_call(&mut tool_batch, &pending.id);
    let active = ContextFrame::new(vec![
        ContextItem::text(
            LlmMessageRole::System,
            "system rules",
            ContextSource::BackendSystemPrompt,
            ContextScope::Run,
            ContextRetention::Retained,
        ),
        ContextItem::text(
            LlmMessageRole::User,
            "RAW_HISTORY_MUST_NOT_RETURN",
            ContextSource::ConversationHistory,
            ContextScope::Conversation,
            ContextRetention::Retained,
        ),
        ContextItem::text(
            LlmMessageRole::Assistant,
            "old answer",
            ContextSource::ConversationHistory,
            ContextScope::Conversation,
            ContextRetention::Retained,
        ),
        ContextItem::text(
            LlmMessageRole::User,
            "current request",
            ContextSource::ConversationHistory,
            ContextScope::Conversation,
            ContextRetention::Retained,
        ),
        assistant_item,
    ]);
    let mut compacted = ContextFrame::new(vec![
        ContextItem::text(
            LlmMessageRole::System,
            "system rules",
            ContextSource::BackendSystemPrompt,
            ContextScope::Run,
            ContextRetention::Retained,
        ),
        ContextItem::text(
            LlmMessageRole::Assistant,
            "COMPACTED_HISTORY_SURVIVES_RESUME",
            ContextSource::ConversationSummary,
            ContextScope::Conversation,
            ContextRetention::Retained,
        ),
        ContextItem::text(
            LlmMessageRole::User,
            "current request",
            ContextSource::ConversationHistory,
            ContextScope::Conversation,
            ContextRetention::Retained,
        ),
    ]);
    let detector =
        ContextCapacityDetector::for_model("test-model", AgentApiStyle::OpenAiCompatible, &[]);
    detector.prepare_frame(&mut compacted);
    let compacted_baseline = compacted.share_measured_persistent_baseline().unwrap();
    let active = active.replace_persistent_baseline(compacted_baseline);
    let conversation_trace = current_test_pending_trace(&tool_batch, &pending);
    let checkpoint = create_run_checkpoint(
        "run-compacted",
        RunCheckpointState {
            context: &active,
            next_model_request_index: 2,
            tool_batch: &tool_batch,
            extension_snapshots: Vec::new(),
            pending_tool_call_id: &pending.id,
            conversation_trace: &conversation_trace,
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
            result: Some(json!({ "status": "applied" })),
            error: None,
        },
    };

    let restored = restore_run_checkpoint(checkpoint, "run-compacted", &continuation).unwrap();
    restored.context.validate_complete_tool_protocol().unwrap();
    let combined_content = restored
        .context
        .to_messages()
        .into_iter()
        .map(|message| message.content().to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(combined_content.contains("COMPACTED_HISTORY_SURVIVES_RESUME"));
    assert!(combined_content.contains("current request"));
    assert!(combined_content.contains("applied"));
    assert!(!combined_content.contains("RAW_HISTORY_MUST_NOT_RETURN"));
}

#[test]
fn approval_resume_preserves_the_complete_committed_trace() {
    let completed_call = AgentToolCall {
        id: canonical_test_call_id(0, "completed-trace"),
        tool: "read_file".to_string(),
        args: json!({ "path": "notes.txt" }),
        approval_status: AgentApprovalStatus::NotRequired,
        reason: None,
    };
    let completed_result = AgentToolResult {
        exact_archive_file: None,
        call_id: completed_call.id.clone(),
        tool: completed_call.tool.clone(),
        ok: true,
        result: Some(json!({ "content": "completed result" })),
        error: None,
    };
    let pending_call = AgentToolCall {
        id: canonical_test_call_id(1, "pending-trace"),
        tool: "apply_patch".to_string(),
        args: apply_patch_args(json!({
            "action": "commit",
            "transactionId": "transaction-pending-trace",
            "expectedDraftRevision": 1
        })),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };
    let completed_llm_call = LlmToolCall {
        id: completed_call.id.clone(),
        name: completed_call.tool.clone(),
        args: completed_call.args.clone(),
    };
    let pending_llm_call = LlmToolCall {
        id: pending_call.id.clone(),
        name: pending_call.tool.clone(),
        args: pending_call.args.clone(),
    };
    let (mut tool_batch, assistant_item) = test_batch_and_context_item(
        "run-multi-tool",
        "",
        vec![completed_llm_call, pending_llm_call],
        false,
    );
    let completed_queued = pop_test_call(&mut tool_batch, &completed_call.id);
    let pending_queued = pop_test_call(&mut tool_batch, &pending_call.id);
    assert_eq!(
        completed_queued.context_group(),
        pending_queued.context_group()
    );
    let group = completed_queued.context_group();
    let context = ContextFrame::new(vec![
        ContextItem::text(
            LlmMessageRole::System,
            "rules",
            ContextSource::BackendSystemPrompt,
            ContextScope::Run,
            ContextRetention::Retained,
        ),
        assistant_item,
        ContextItem::tool_result(
            completed_call.id.clone(),
            build_tool_observation_message(&completed_result),
            false,
            ContextMetadata::new(
                ContextSource::ToolResult,
                ContextScope::Run,
                ContextRetention::Retained,
            )
            .with_group(group),
        ),
    ]);
    let mut trace = ConversationTraceRecorder::default();
    record_current_test_tool_call(&mut trace, &tool_batch, &completed_call);
    let completed_result_sequence = trace
        .record_tool_result(&completed_call, &completed_result)
        .expect("current test Tool Result is recorded once");
    trace
        .record_model_message(
            completed_result_sequence,
            0,
            &LlmMessage::tool_result(
                completed_call.id.clone(),
                build_tool_observation_message(&completed_result),
                false,
            ),
        )
        .expect("current test Tool Result has immutable model context");
    record_current_test_tool_call(&mut trace, &tool_batch, &pending_call);
    let checkpoint = create_run_checkpoint(
        "run-multi-tool",
        RunCheckpointState {
            context: &context,
            next_model_request_index: 1,
            tool_batch: &tool_batch,
            extension_snapshots: Vec::new(),
            pending_tool_call_id: &pending_call.id,
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
    let pending_call_id = pending_call.id.clone();
    let continuation = AgentToolContinuation {
        call: AgentToolCall {
            approval_status: AgentApprovalStatus::Approved,
            ..pending_call
        },
        result: AgentToolResult {
            exact_archive_file: None,
            call_id: pending_call_id,
            tool: "apply_patch".to_string(),
            ok: true,
            result: Some(json!({ "status": "applied" })),
            error: None,
        },
    };

    let restored = restore_run_checkpoint(checkpoint, "run-multi-tool", &continuation).unwrap();

    assert_eq!(restored.conversation_trace.committed_item_count(), 4);
}
