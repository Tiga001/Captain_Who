use super::mcp_fixtures::{
    seed_durable_mcp_pending_owner, test_mcp_envelope, test_mcp_pending_action,
    test_mcp_resume_checkpoint, InvalidatingMcpInvoker, RecoverableApprovalRaceInvoker,
    StaticMcpStartupInspector,
};
use super::*;

fn assert_recovered_approved_cancellation() {
    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("storage.sqlite");
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    save_test_pending_provider(
        &storage,
        "test-model",
        "https://example.test/v1/chat/completions",
        "test-token",
        "disabled",
        "",
    );
    let service = AgentService::new_authorized_for_test(Arc::clone(&storage));
    let now = mycopilot_core::storage::now_ms();
    let run_id = "recovered-approved-cancel-run";
    let action_id = uuid::Uuid::new_v4().to_string();
    let invocation_id = uuid::Uuid::new_v4().to_string();
    let storage_id = pending_action_storage_id(run_id, &action_id);
    let mut input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "test-token",
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "messages": []
    }))
    .unwrap();
    input.resume_checkpoint = Some(test_mcp_resume_checkpoint(&storage, run_id, &action_id));
    freeze_test_pending_provider_configuration(&storage, &mut input);
    let conversation_id = format!("conversation-{run_id}");
    let assistant_message_id = format!("assistant-{run_id}");
    let action = test_mcp_pending_action(run_id, &action_id, &invocation_id, now);
    seed_durable_mcp_pending_owner(
        &storage,
        &conversation_id,
        &assistant_message_id,
        run_id,
        &action,
        now,
    );
    assert!(service
        .store_pending_action(
            run_id,
            &conversation_id,
            &assistant_message_id,
            action,
            input,
        )
        .unwrap());
    let pending = service
        .pending_actions
        .lock()
        .unwrap_or_else(|error| error.into_inner())[&storage_id]
        .clone();
    service
        .transition_pending_status(&pending, PendingActionStatus::Approved)
        .unwrap();
    storage
        .store_mcp_approval_envelope(test_mcp_envelope(
            &invocation_id,
            &storage_id,
            now,
            now + 60_000,
        ))
        .unwrap();
    drop(service);

    let invoker = Arc::new(InvalidatingMcpInvoker {
        storage: Some(Arc::clone(&storage)),
        ..InvalidatingMcpInvoker::default()
    });
    let restarted = AgentService::try_new_deferred_startup_reconciliation(Arc::clone(&storage))
        .unwrap()
        .with_mcp_startup_inspector(Arc::new(StaticMcpStartupInspector(
            McpApprovalStartupPayloadState::DurableAvailable,
        )))
        .with_mcp_tool_invoker(invoker.clone() as Arc<dyn McpToolInvoker>);
    assert_eq!(restarted.reconcile_startup_mcp_actions().unwrap(), 0);
    assert_eq!(restarted.list_pending_actions().len(), 1);

    let (notifications, _receiver) = tokio::sync::mpsc::unbounded_channel();
    assert!(restarted.cancel_action(run_id, &action_id).unwrap());

    assert!(restarted.list_pending_actions().is_empty());
    assert!(restarted
        .approve_action(run_id, &action_id, notifications.clone())
        .is_err());
    assert!(restarted
        .reject_action(run_id, &action_id, None, notifications)
        .is_err());
    assert!(!restarted.cancel_action(run_id, &action_id).unwrap());
    assert_eq!(
        invoker
            .invalidations
            .load(std::sync::atomic::Ordering::SeqCst),
        1
    );
    assert!(storage
        .load_mcp_approval_envelope(&invocation_id)
        .unwrap()
        .is_none());

    let connection = rusqlite::Connection::open(database_path).unwrap();
    let terminal: (String, Option<String>, String, String, String) = connection
        .query_row(
            "SELECT pending.status, pending.target_status, pending.action_json,
                    pending.agent_input_json, audit.error
             FROM agent_pending_actions pending
             JOIN agent_action_audit audit ON audit.action_id = pending.action_id
             WHERE pending.action_id = ?1",
            [&storage_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .unwrap();
    let (expected_status, expected_error) = ("cancelled", "mcp.approval_cancelled");
    assert_eq!(terminal.0, expected_status);
    assert_eq!(terminal.1.as_deref(), Some(expected_status));
    assert_eq!(terminal.2, "{}");
    assert_eq!(terminal.3, "{}");
    assert_eq!(terminal.4, expected_error);
}

#[test]
fn recovered_approved_mcp_can_be_cancelled_with_a_definitely_not_dispatched_terminal_cas() {
    assert_recovered_approved_cancellation();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn recovered_approved_mcp_approve_reject_cancel_race_has_one_durable_winner() {
    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("storage.sqlite");
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    save_test_pending_provider(
        &storage,
        "test-model",
        "https://example.test/v1/chat/completions",
        "test-token",
        "disabled",
        "",
    );
    let initial = AgentService::new_authorized_for_test(Arc::clone(&storage));
    let now = mycopilot_core::storage::now_ms();
    let run_id = "recovered-approved-decision-race";
    let action_id = uuid::Uuid::new_v4().to_string();
    let invocation_id = uuid::Uuid::new_v4().to_string();
    let storage_id = pending_action_storage_id(run_id, &action_id);
    let mut input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "test-token",
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "messages": []
    }))
    .unwrap();
    input.resume_checkpoint = Some(test_mcp_resume_checkpoint(&storage, run_id, &action_id));
    freeze_test_pending_provider_configuration(&storage, &mut input);
    let action = test_mcp_pending_action(run_id, &action_id, &invocation_id, now);
    seed_durable_mcp_pending_owner(
        &storage,
        "recovered-approved-race-conversation",
        "recovered-approved-race-assistant",
        run_id,
        &action,
        now,
    );
    assert!(initial
        .store_pending_action(
            run_id,
            "recovered-approved-race-conversation",
            "recovered-approved-race-assistant",
            action,
            input,
        )
        .unwrap());
    let pending = initial
        .pending_actions
        .lock()
        .unwrap_or_else(|error| error.into_inner())[&storage_id]
        .clone();
    initial
        .transition_pending_status(&pending, PendingActionStatus::Approved)
        .unwrap();
    storage
        .store_mcp_approval_envelope(test_mcp_envelope(
            &invocation_id,
            &storage_id,
            now,
            now + 60_000,
        ))
        .unwrap();
    drop(initial);

    // Mirror the production invoker's invalidation boundary: every winning decision deletes the
    // encrypted payload envelope. A counter-only mock made the approve winner leave the fixture's
    // separately seeded envelope behind, so the race test passed or timed out depending on which
    // decision happened to win.
    let invoker = Arc::new(RecoverableApprovalRaceInvoker {
        storage: Some(Arc::clone(&storage)),
        ..RecoverableApprovalRaceInvoker::default()
    });
    let recovered = (0..3)
        .map(|_| {
            let service =
                AgentService::try_new_deferred_startup_reconciliation(Arc::clone(&storage))
                    .unwrap()
                    .with_mcp_startup_inspector(Arc::new(StaticMcpStartupInspector(
                        McpApprovalStartupPayloadState::DurableAvailable,
                    )))
                    .with_mcp_tool_invoker(invoker.clone() as Arc<dyn McpToolInvoker>);
            assert_eq!(service.reconcile_startup_mcp_actions().unwrap(), 0);
            service
        })
        .collect::<Vec<_>>();
    let barrier = Arc::new(tokio::sync::Barrier::new(4));

    let approve_service = recovered[0].clone();
    let approve_barrier = Arc::clone(&barrier);
    let approve_action_id = action_id.clone();
    let approve = tokio::spawn(async move {
        approve_barrier.wait().await;
        let (notifications, _receiver) = tokio::sync::mpsc::unbounded_channel();
        approve_service
            .approve_action(run_id, &approve_action_id, notifications)
            .is_ok()
    });
    let reject_service = recovered[1].clone();
    let reject_barrier = Arc::clone(&barrier);
    let reject_action_id = action_id.clone();
    let reject = tokio::spawn(async move {
        reject_barrier.wait().await;
        let (notifications, _receiver) = tokio::sync::mpsc::unbounded_channel();
        reject_service
            .reject_action(run_id, &reject_action_id, None, notifications)
            .is_ok()
    });
    let cancel_service = recovered[2].clone();
    let cancel_barrier = Arc::clone(&barrier);
    let cancel_action_id = action_id.clone();
    let cancel = tokio::spawn(async move {
        cancel_barrier.wait().await;
        cancel_service
            .cancel_action(run_id, &cancel_action_id)
            .is_ok_and(|cancelled| cancelled)
    });
    barrier.wait().await;
    let winners = usize::from(approve.await.unwrap())
        + usize::from(reject.await.unwrap())
        + usize::from(cancel.await.unwrap());
    assert_eq!(winners, 1);

    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let lifecycle = storage
                .list_pending_agent_actions()
                .unwrap()
                .into_iter()
                .find(|record| record.action_id == storage_id)
                .map(|record| (record.status, record.target_status));
            let terminal = lifecycle.as_ref().is_none_or(|(status, target_status)| {
                matches!(
                    status.as_str(),
                    "completed" | "failed" | "rejected" | "cancelled"
                ) || target_status.as_deref().is_some_and(|target_status| {
                    matches!(
                        target_status,
                        "completed" | "failed" | "rejected" | "cancelled"
                    )
                })
            });
            let envelope_released = storage
                .load_mcp_approval_envelope(&invocation_id)
                .unwrap()
                .is_none();
            if terminal && envelope_released {
                break;
            }
            // An approved MCP call commits its terminal target before the following model
            // continuation advances the row itself to that target. The target is the durable
            // recovery boundary this race is proving; do not make the assertion depend on an
            // unrelated Provider continuation completing. Avoid a tight loop of synchronous
            // SQLite reads starving the background Runtime task under full-workspace load.
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the winning recovered decision must reach a durable terminal state");
    assert!(storage
        .load_mcp_approval_envelope(&invocation_id)
        .unwrap()
        .is_none());
    assert!(
        invoker
            .invocations
            .load(std::sync::atomic::Ordering::SeqCst)
            <= 1
    );
}

#[tokio::test]
async fn expired_mcp_approval_accepts_one_decision_and_settles_execution_normally() {
    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("storage.sqlite");
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    save_test_pending_provider(
        &storage,
        "test-model",
        "https://example.test/v1/chat/completions",
        "test-token",
        "disabled",
        "",
    );
    let invoker = Arc::new(InvalidatingMcpInvoker::default());
    let invoker_for_service: Arc<dyn McpToolInvoker> = invoker.clone();
    let now = mycopilot_core::storage::now_ms();
    let service = AgentService::new_authorized_for_test(Arc::clone(&storage))
        .with_mcp_tool_invoker(invoker_for_service)
        .with_mcp_approval_clock(move || now + 60_000);
    let run_id = "mcp-expired-approval-run";
    let action_id = uuid::Uuid::new_v4().to_string();
    let invocation_id = uuid::Uuid::new_v4().to_string();
    let storage_id = pending_action_storage_id(run_id, &action_id);
    let mut input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "test-token",
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "messages": []
    }))
    .unwrap();
    input.resume_checkpoint = Some(test_mcp_resume_checkpoint(&storage, run_id, &action_id));
    freeze_test_pending_provider_configuration(&storage, &mut input);
    let action = test_mcp_pending_action(run_id, &action_id, &invocation_id, now);
    seed_durable_mcp_pending_owner(
        &storage,
        "mcp-expired-conversation",
        "mcp-expired-assistant",
        run_id,
        &action,
        now,
    );
    assert!(service
        .store_pending_action(
            run_id,
            "mcp-expired-conversation",
            "mcp-expired-assistant",
            action,
            input,
        )
        .unwrap());

    let (notifications, _receiver) = tokio::sync::mpsc::unbounded_channel();
    let accepted = service
        .approve_action(run_id, &action_id, notifications.clone())
        .unwrap();
    assert_eq!(accepted.status, "approved");
    assert!(service
        .approve_action(run_id, &action_id, notifications)
        .is_err());

    for _ in 0..100 {
        let status = storage
            .get_pending_agent_action(&storage_id)
            .unwrap()
            .map(|record| record.status);
        if matches!(status.as_deref(), Some("failed" | "completed")) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(
        invoker
            .invalidations
            .load(std::sync::atomic::Ordering::SeqCst)
            >= 1,
        "payload cleanup is idempotent; rows={:?}",
        storage.list_pending_agent_actions().unwrap()
    );
    assert!(service.list_pending_actions().is_empty());

    let connection = rusqlite::Connection::open(database_path).unwrap();
    let terminal: (String, Option<String>) = connection
        .query_row(
            "SELECT pending.status, audit.error
             FROM agent_pending_actions pending
             JOIN agent_action_audit audit ON audit.action_id = pending.action_id
             WHERE pending.action_id = ?1",
            [&storage_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_ne!(terminal.0, "pending");
    assert_ne!(terminal.1.as_deref(), Some("mcp.approval_payload_expired"));
}
