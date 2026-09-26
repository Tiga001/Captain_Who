use super::builtin_fixtures::store_builtin_sensitive_test_pending;
use super::*;

#[test]
fn builtin_sensitive_approval_tick_preserves_ticket_and_turn_ownership() {
    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("builtin-sensitive-expiry-tick.sqlite");
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    save_test_pending_provider(
        &storage,
        "test-model",
        "http://127.0.0.1:9/v1/chat/completions",
        "test-token",
        "disabled",
        "",
    );
    let expiry_clock_ms = mycopilot_core::storage::now_ms().saturating_add(901_000);
    let service = AgentService::new_authorized_for_test(Arc::clone(&storage))
        .with_mcp_approval_clock(move || expiry_clock_ms);
    let run_id = "builtin-sensitive-expiry-tick-run";
    let conversation_id = "builtin-sensitive-expiry-tick-conversation";
    let assistant_message_id = "builtin-sensitive-expiry-tick-assistant";
    let (action_id, call) = store_builtin_sensitive_test_pending(
        &service,
        &storage,
        run_id,
        conversation_id,
        assistant_message_id,
        "builtin-sensitive-expiry-tick-call",
    );
    let had_turn_permit = service
        .active_turn_permits
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .contains_key(run_id);

    assert_eq!(service.list_pending_actions().len(), 1);
    assert_eq!(
        service
            .reconcile_expired_builtin_mcp_tool_approvals()
            .unwrap(),
        0
    );
    assert_eq!(service.list_pending_actions().len(), 1);
    assert_eq!(
        service
            .reconcile_expired_builtin_mcp_tool_approvals()
            .unwrap(),
        0,
        "ticket reconciliation remains a no-op"
    );
    assert!(service
        .has_conversation_turn_occupancy(conversation_id)
        .unwrap());
    assert_eq!(
        service
            .active_turn_permits
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .contains_key(run_id),
        had_turn_permit
    );

    let storage_id = pending_action_storage_id(run_id, &action_id);
    let retained = storage
        .get_pending_agent_action(&storage_id)
        .unwrap()
        .unwrap();
    assert_eq!(retained.status, "pending");
    assert_ne!(retained.action_json, "{}");
    assert_ne!(retained.agent_input_json, "{}");
    let trace = storage
        .get_conversation_turn_trace(assistant_message_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        trace.terminal_status,
        ConversationTurnTraceTerminalStatus::InProgress
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
        0
    );
}

#[tokio::test]
async fn approving_an_expired_builtin_sensitive_action_accepts_a_normal_failed_result() {
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
    let expiry_clock_ms = mycopilot_core::storage::now_ms().saturating_add(901_000);
    let service = AgentService::new_authorized_for_test(Arc::clone(&storage))
        .with_mcp_approval_clock(move || expiry_clock_ms);
    let run_id = "builtin-sensitive-expiry-decision-run";
    let conversation_id = "builtin-sensitive-expiry-decision-conversation";
    let assistant_message_id = "builtin-sensitive-expiry-decision-assistant";
    let (action_id, call) = store_builtin_sensitive_test_pending(
        &service,
        &storage,
        run_id,
        conversation_id,
        assistant_message_id,
        "builtin-sensitive-expiry-decision-call",
    );
    let (notifications, _receiver) = tokio::sync::mpsc::unbounded_channel();

    let output = service
        .queue_action_continuation(
            run_id,
            &action_id,
            AgentApprovalDecisionStatus::Approved,
            None,
            notifications,
        )
        .unwrap();
    assert_eq!(output.status, "failed");
    assert_eq!(output.agent_output.status, AgentRunStatus::Running);
    assert_eq!(
        output
            .tool_result
            .as_ref()
            .and_then(|result| result.result.as_ref())
            .and_then(|result| result.get("errorCode"))
            .and_then(Value::as_str),
        Some("mcp.tool_approval_payload_unavailable")
    );
    assert!(service.list_pending_actions().is_empty());

    let storage_id = pending_action_storage_id(run_id, &action_id);
    let decided = storage
        .get_pending_agent_action(&storage_id)
        .unwrap()
        .unwrap();
    assert_eq!(decided.status, "approved");
    assert_eq!(decided.target_status.as_deref(), Some("failed"));

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
    assert!(service
        .queue_action_continuation(
            run_id,
            &action_id,
            AgentApprovalDecisionStatus::Approved,
            None,
            tokio::sync::mpsc::unbounded_channel().0,
        )
        .is_err());
}

#[test]
fn builtin_sensitive_cancel_reaches_pre_spawn_and_dispatching_process_guards() {
    for (index, target_status) in [
        PendingActionStatus::Approved,
        PendingActionStatus::Executing,
    ]
    .into_iter()
    .enumerate()
    {
        let fixture = tempdir().unwrap();
        let storage =
            Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
        save_test_pending_provider(
            &storage,
            "test-model",
            "http://127.0.0.1:9/v1/chat/completions",
            "test-token",
            "disabled",
            "",
        );
        let service = AgentService::new_authorized_for_test(Arc::clone(&storage));
        let run_id = format!("builtin-sensitive-cancel-run-{index}");
        let conversation_id = format!("builtin-sensitive-cancel-conversation-{index}");
        let assistant_message_id = format!("builtin-sensitive-cancel-assistant-{index}");
        let (action_id, _) = store_builtin_sensitive_test_pending(
            &service,
            &storage,
            &run_id,
            &conversation_id,
            &assistant_message_id,
            &format!("builtin-sensitive-cancel-call-{index}"),
        );
        let storage_id = pending_action_storage_id(&run_id, &action_id);
        let record = service
            .pending_actions
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(&storage_id)
            .unwrap()
            .clone();
        service
            .transition_pending_status(&record, PendingActionStatus::Approved)
            .unwrap();
        if target_status == PendingActionStatus::Executing {
            let approved = service
                .pending_actions
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .get(&storage_id)
                .unwrap()
                .clone();
            service
                .transition_pending_status(&approved, PendingActionStatus::Executing)
                .unwrap();
        }
        let guard = service.process_runs.register(&storage_id, &run_id);
        assert!(service.cancel_action(&run_id, &action_id).unwrap());
        assert!(guard
            .cancel_flag()
            .load(std::sync::atomic::Ordering::SeqCst));
        assert_eq!(
            service
                .pending_actions
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .get(&storage_id)
                .unwrap()
                .snapshot
                .status,
            target_status
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn builtin_sensitive_reject_wins_approve_cancel_and_double_reject_races_once() {
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
    let run_id = "builtin-sensitive-reject-race-run";
    let conversation_id = "builtin-sensitive-reject-race-conversation";
    let assistant_message_id = "builtin-sensitive-reject-race-assistant";
    let (action_id, call) = store_builtin_sensitive_test_pending(
        &service,
        &storage,
        run_id,
        conversation_id,
        assistant_message_id,
        "builtin-sensitive-reject-race-call",
    );
    let storage_id = pending_action_storage_id(run_id, &action_id);
    let (notifications, _receiver) = tokio::sync::mpsc::unbounded_channel();
    let approve_entered = Arc::new(std::sync::Barrier::new(2));
    let approve_release = Arc::new(std::sync::Barrier::new(2));
    crate::application::agent::approval::install_approval_decision_barrier_hook(
        &action_id,
        AgentApprovalDecisionStatus::Approved,
        {
            let entered = Arc::clone(&approve_entered);
            let release = Arc::clone(&approve_release);
            Arc::new(move || {
                entered.wait();
                release.wait();
            })
        },
    );
    let approve = {
        let service = service.clone();
        let action_id = action_id.clone();
        let notifications = notifications.clone();
        tokio::spawn(async move {
            service.queue_action_continuation(
                run_id,
                &action_id,
                AgentApprovalDecisionStatus::Approved,
                None,
                notifications,
            )
        })
    };
    approve_entered.wait();
    let output = service
        .queue_action_continuation(
            run_id,
            &action_id,
            AgentApprovalDecisionStatus::Rejected,
            Some("Use the public workflow instead.".to_string()),
            notifications.clone(),
        )
        .unwrap();
    assert_eq!(output.status, "rejected");
    approve_release.wait();
    assert!(approve.await.unwrap().is_err());
    assert!(service
        .queue_action_continuation(
            run_id,
            &action_id,
            AgentApprovalDecisionStatus::Rejected,
            None,
            notifications,
        )
        .is_err());
    assert!(!service.cancel_action(run_id, &action_id).unwrap());
    let row = storage
        .get_pending_agent_action(&storage_id)
        .unwrap()
        .unwrap();
    assert_eq!(row.status, "rejected");
    assert_eq!(row.target_status.as_deref(), Some("rejected"));
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
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn builtin_sensitive_approve_claim_blocks_late_reject_and_settles_once() {
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
    let run_id = "builtin-sensitive-approve-race-run";
    let conversation_id = "builtin-sensitive-approve-race-conversation";
    let assistant_message_id = "builtin-sensitive-approve-race-assistant";
    let (action_id, call) = store_builtin_sensitive_test_pending(
        &service,
        &storage,
        run_id,
        conversation_id,
        assistant_message_id,
        "builtin-sensitive-approve-race-call",
    );
    let storage_id = pending_action_storage_id(run_id, &action_id);
    let (notifications, _receiver) = tokio::sync::mpsc::unbounded_channel();
    let reject_entered = Arc::new(std::sync::Barrier::new(2));
    let reject_release = Arc::new(std::sync::Barrier::new(2));
    crate::application::agent::approval::install_approval_decision_barrier_hook(
        &action_id,
        AgentApprovalDecisionStatus::Rejected,
        {
            let entered = Arc::clone(&reject_entered);
            let release = Arc::clone(&reject_release);
            Arc::new(move || {
                entered.wait();
                release.wait();
            })
        },
    );
    let reject = {
        let service = service.clone();
        let action_id = action_id.clone();
        let notifications = notifications.clone();
        tokio::spawn(async move {
            service.queue_action_continuation(
                run_id,
                &action_id,
                AgentApprovalDecisionStatus::Rejected,
                None,
                notifications,
            )
        })
    };
    reject_entered.wait();
    let approved = service
        .queue_action_continuation(
            run_id,
            &action_id,
            AgentApprovalDecisionStatus::Approved,
            None,
            notifications.clone(),
        )
        .unwrap();
    assert_eq!(approved.status, "failed");
    assert_eq!(approved.agent_output.status, AgentRunStatus::Running);
    assert_eq!(
        approved
            .tool_result
            .as_ref()
            .and_then(|result| result.result.as_ref())
            .and_then(|result| result.get("errorCode"))
            .and_then(Value::as_str),
        Some("mcp.tool_approval_payload_unavailable")
    );
    reject_release.wait();
    assert!(reject.await.unwrap().is_err());

    for _ in 0..200 {
        if storage
            .get_pending_agent_action(&storage_id)
            .unwrap()
            .is_some_and(|record| record.status == "failed")
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(
        storage
            .get_pending_agent_action(&storage_id)
            .unwrap()
            .unwrap()
            .status,
        "failed"
    );

    drop(service);
    let restarted = AgentService::new_authorized_for_test(Arc::clone(&storage));
    assert!(!restarted
        .pending_actions
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .contains_key(&storage_id));
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
    let rendered = serde_json::to_string(&trace).unwrap();
    assert!(rendered.contains("payload_unavailable"));
    assert!(rendered.contains("definitely_not_dispatched"));
    assert!(!rendered.contains("outcome_unknown"));
}
