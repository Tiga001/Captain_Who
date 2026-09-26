use super::builtin_fixtures::{auto_sensitive_test_fixture, store_builtin_sensitive_test_pending};
use super::*;

#[test]
fn automatic_builtin_sensitive_journal_is_durable_cancelable_and_non_replayable() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    save_test_pending_provider(
        &storage,
        "test-model",
        "http://127.0.0.1:9/v1/chat/completions",
        "test-token",
        "disabled",
        "",
    );
    let service = AgentService::new_authorized_for_test(Arc::clone(&storage));
    let run_id = "auto-builtin-sensitive-journal-run";
    let (manual_action_id, _) = store_builtin_sensitive_test_pending(
        &service,
        &storage,
        run_id,
        "auto-builtin-sensitive-conversation",
        "auto-builtin-sensitive-assistant",
        "auto-builtin-sensitive-call",
    );
    let manual_storage_id = pending_action_storage_id(run_id, &manual_action_id);
    let template = service
        .pending_actions
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(&manual_storage_id)
        .cloned()
        .unwrap();

    for (index, outcome) in [
        McpAutoActionJournalTerminalOutcome::Completed,
        McpAutoActionJournalTerminalOutcome::Cancelled,
    ]
    .into_iter()
    .enumerate()
    {
        let action_id = uuid::Uuid::new_v4().to_string();
        let mut action = template.snapshot.action.clone();
        let AgentProposedAction::BuiltinMcpToolApproval { approval } = &mut action else {
            unreachable!("fixture always creates a built-in MCP approval")
        };
        approval.identity.action_id = action_id.clone();
        approval.approval_status = AgentApprovalStatus::Approved;

        let mut input = template.agent_input.clone();
        let run_context = mycopilot_core::AgentRunContext {
            conversation_id: None,
            project_id: None,
            workspace: None,
            attachment_library: None,
            permissions: mycopilot_core::AgentPermissions {
                builtin_execution: mycopilot_core::AgentBuiltinExecutionPermission::AutoApprove,
                ..mycopilot_core::AgentPermissions::default()
            },
            collaboration_identity: None,
        };
        input.context = Some(run_context.clone());
        let checkpoint = input.resume_checkpoint.as_mut().unwrap();
        checkpoint.run_context = Some(run_context);
        checkpoint.pending_action_id = Some(action_id.clone());
        assert!(
            !pending_action_binding_matches(run_id, None, &action, &input),
            "the manual pending-action boundary must never accept an auto-approved action"
        );

        let mut journal = service
            .prepare_auto_mcp_action_journal(run_id, None, None, action.clone(), input.clone())
            .unwrap();
        assert_eq!(journal.snapshot.status, PendingActionStatus::Approved);
        if outcome == McpAutoActionJournalTerminalOutcome::Completed {
            service.claim_auto_mcp_dispatch(&mut journal).unwrap();
            assert_eq!(journal.snapshot.status, PendingActionStatus::Executing);
        }
        service
            .settle_auto_mcp_action_journal(&journal, outcome, None)
            .unwrap();

        let durable = storage
            .get_pending_agent_action(&pending_action_storage_id(run_id, &action_id))
            .unwrap()
            .unwrap();
        assert_eq!(
            durable.status,
            if outcome == McpAutoActionJournalTerminalOutcome::Completed {
                "completed"
            } else {
                "cancelled"
            },
            "case {index} must have an exact durable terminal state"
        );
        assert_eq!(durable.action_json, "{}");
        assert_eq!(durable.agent_input_json, "{}");
        assert!(service
            .prepare_auto_mcp_action_journal(run_id, None, None, action, input)
            .is_err());
    }
}

#[tokio::test]
async fn automatic_builtin_sensitive_execution_claims_before_invoke_and_never_replays() {
    for cancelled in [false, true] {
        let fixture = tempdir().unwrap();
        let storage =
            Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
        save_test_pending_provider(
            &storage,
            "test-model",
            "https://example.test/v1/chat/completions",
            "test-token",
            "disabled",
            "",
        );
        let run_id = if cancelled {
            "auto-sensitive-cancel-run"
        } else {
            "auto-sensitive-success-run"
        };
        let (runtime, provider, approval, input) =
            auto_sensitive_test_fixture(Arc::clone(&storage), run_id, "auto-sensitive-call");
        let service = AgentService::new_authorized_for_test(Arc::clone(&storage))
            .with_builtin_capabilities(runtime);
        let context =
            AutoApprovedActionContext::new(input.clone(), run_id.to_string(), None, None, None);
        let cancellation = AgentCancellationToken::new();
        if cancelled {
            cancellation.cancel();
        }
        let result = service
            .execute_auto_builtin_mcp_tool_action(
                &context,
                Box::new(approval.clone()),
                cancellation,
            )
            .await
            .unwrap();
        assert_eq!(result.ok, !cancelled);
        assert_eq!(
            provider
                .invocations
                .load(std::sync::atomic::Ordering::SeqCst),
            usize::from(!cancelled)
        );
        assert_eq!(
            provider
                .revocations
                .load(std::sync::atomic::Ordering::SeqCst),
            usize::from(cancelled)
        );
        let durable = storage
            .get_pending_agent_action(&pending_action_storage_id(
                run_id,
                &approval.identity.action_id,
            ))
            .unwrap()
            .unwrap();
        assert_eq!(
            durable.status,
            if cancelled { "cancelled" } else { "completed" }
        );
        assert_eq!(durable.action_json, "{}");
        assert_eq!(durable.agent_input_json, "{}");

        let replay = service
            .execute_auto_builtin_mcp_tool_action(
                &context,
                Box::new(approval),
                AgentCancellationToken::new(),
            )
            .await;
        assert!(replay.is_err());
        assert_eq!(
            provider
                .invocations
                .load(std::sync::atomic::Ordering::SeqCst),
            usize::from(!cancelled),
            "a durable terminal action must never invoke twice"
        );
    }
}

#[test]
fn builtin_sensitive_result_commit_failure_terminalizes_and_notifies_once() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    save_test_pending_provider(
        &storage,
        "test-model",
        "http://127.0.0.1:9/v1/chat/completions",
        "test-token",
        "disabled",
        "",
    );
    let service = AgentService::new_authorized_for_test(Arc::clone(&storage));
    let run_id = "builtin-sensitive-commit-failure-run";
    let conversation_id = "builtin-sensitive-commit-failure-conversation";
    let assistant_message_id = "builtin-sensitive-commit-failure-assistant";
    let (action_id, call) = store_builtin_sensitive_test_pending(
        &service,
        &storage,
        run_id,
        conversation_id,
        assistant_message_id,
        "builtin-sensitive-commit-failure-call",
    );
    let storage_id = pending_action_storage_id(run_id, &action_id);
    let mut record = service
        .pending_actions
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(&storage_id)
        .cloned()
        .unwrap();
    service
        .transition_pending_status(&record, PendingActionStatus::Approved)
        .unwrap();
    record.snapshot.status = PendingActionStatus::Approved;
    service
        .transition_pending_status(&record, PendingActionStatus::Executing)
        .unwrap();
    record.snapshot.status = PendingActionStatus::Executing;

    let secret = "BUILTIN_EVALUATE_RESULT_MUST_NOT_REACH_ACTIVITY";
    let live_result = AgentToolResult {
        exact_archive_file: None,
        call_id: call.id.clone(),
        tool: call.tool.clone(),
        ok: true,
        result: Some(json!({ "secret": secret })),
        error: None,
    };
    let mut persisted_input = record.agent_input.clone();
    persisted_input.approval_decision = Some(AgentApprovalDecision {
        action_id: action_id.clone(),
        status: AgentApprovalDecisionStatus::Approved,
        message: None,
    });
    persisted_input.tool_continuation = Some(AgentToolContinuation {
        call: call.clone(),
        result: mycopilot_core::builtin_capability_tool_result_persistence_projection(&live_result),
    });
    let cancellation = AgentCancellationToken::new();
    service.register_cancellation(run_id, cancellation.clone());
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    inject_manual_action_audit_failure(&record.storage_id, "completed");

    let _ = service.commit_builtin_mcp_tool_result_or_terminalize(
        &record,
        &persisted_input,
        PendingActionStatus::Completed,
        &notifications,
        &cancellation,
    );

    assert!(service.list_pending_actions().is_empty());
    assert!(!service
        .cancellations
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .contains_key(run_id));
    let retired = storage
        .get_pending_agent_action(&storage_id)
        .unwrap()
        .unwrap();
    assert_eq!(retired.status, "failed");
    assert_eq!(retired.target_status.as_deref(), Some("failed"));
    let trace = storage
        .get_conversation_turn_trace(assistant_message_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        trace.terminal_status,
        ConversationTurnTraceTerminalStatus::Failed
    );
    assert_eq!(
        trace
            .items
            .iter()
            .filter(|item| matches!(
                item,
                ConversationTurnTraceItem::ToolResult { call_id, .. } if call_id == &call.id
            ))
            .count(),
        1
    );
    let events = std::iter::from_fn(|| receiver.try_recv().ok()).collect::<Vec<_>>();
    assert_eq!(
        events
            .iter()
            .filter(|event| event["params"]["type"] == "tool_result")
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event["params"]["type"] == "done")
            .count(),
        1
    );
    let serialized = serde_json::to_string(&events).unwrap();
    assert!(!serialized.contains(secret));
    assert!(serialized.contains("outcome_unknown"));
}

#[test]
fn builtin_sensitive_post_commit_error_adopts_receipt_and_emits_safe_result() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    save_test_pending_provider(
        &storage,
        "test-model",
        "http://127.0.0.1:9/v1/chat/completions",
        "test-token",
        "disabled",
        "",
    );
    let service = AgentService::new_authorized_for_test(Arc::clone(&storage));
    let run_id = "builtin-sensitive-post-commit-run";
    let conversation_id = "builtin-sensitive-post-commit-conversation";
    let assistant_message_id = "builtin-sensitive-post-commit-assistant";
    let (action_id, call) = store_builtin_sensitive_test_pending(
        &service,
        &storage,
        run_id,
        conversation_id,
        assistant_message_id,
        "builtin-sensitive-post-commit-call",
    );
    let storage_id = pending_action_storage_id(run_id, &action_id);
    let mut record = service
        .pending_actions
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(&storage_id)
        .cloned()
        .unwrap();
    service
        .transition_pending_status(&record, PendingActionStatus::Approved)
        .unwrap();
    record.snapshot.status = PendingActionStatus::Approved;
    service
        .transition_pending_status(&record, PendingActionStatus::Executing)
        .unwrap();
    record.snapshot.status = PendingActionStatus::Executing;

    let secret = "BUILTIN_EVALUATE_LIVE_RESULT_MUST_BE_OMITTED";
    let live_result = AgentToolResult {
        exact_archive_file: None,
        call_id: call.id.clone(),
        tool: call.tool.clone(),
        ok: true,
        result: Some(json!({ "secret": secret })),
        error: None,
    };
    let mut persisted_input = record.agent_input.clone();
    persisted_input.approval_decision = Some(AgentApprovalDecision {
        action_id,
        status: AgentApprovalDecisionStatus::Approved,
        message: None,
    });
    persisted_input.tool_continuation = Some(AgentToolContinuation {
        call: call.clone(),
        result: mycopilot_core::builtin_capability_tool_result_persistence_projection(&live_result),
    });
    let cancellation = AgentCancellationToken::new();
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    inject_manual_action_audit_post_commit_failure(&record.storage_id, "completed");

    let _ = service.commit_builtin_mcp_tool_result_or_terminalize(
        &record,
        &persisted_input,
        PendingActionStatus::Completed,
        &notifications,
        &cancellation,
    );
    service.emit_safe_builtin_mcp_tool_result(&notifications, run_id, &live_result);

    let pending = storage
        .get_pending_agent_action(&storage_id)
        .unwrap()
        .unwrap();
    assert_eq!(pending.status, "executing");
    assert_eq!(pending.target_status.as_deref(), Some("completed"));
    let trace = storage
        .get_conversation_turn_trace(assistant_message_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        trace
            .items
            .iter()
            .filter(|item| matches!(
                item,
                ConversationTurnTraceItem::ToolResult { call_id, .. } if call_id == &call.id
            ))
            .count(),
        1
    );
    let events = std::iter::from_fn(|| receiver.try_recv().ok()).collect::<Vec<_>>();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["params"]["type"], "tool_result");
    assert_eq!(
        events[0]["params"]["result"]["result"]["status"],
        "completed"
    );
    let serialized = serde_json::to_string(&events).unwrap();
    assert!(!serialized.contains(secret));
    service
        .transition_pending_status(&record, PendingActionStatus::Completed)
        .unwrap();
    assert_eq!(
        storage
            .get_pending_agent_action(&storage_id)
            .unwrap()
            .unwrap()
            .status,
        "completed"
    );
}
