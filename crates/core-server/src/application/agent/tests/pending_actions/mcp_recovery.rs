use super::mcp_fixtures::{
    seed_durable_mcp_pending_owner, test_mcp_envelope, test_mcp_pending_action,
    test_mcp_resume_checkpoint, StaticMcpStartupInspector,
};
use super::*;

#[test]
fn startup_preserves_pending_mcp_tickets_while_pruning_expired_and_orphaned_payloads() {
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
    let mut agent_input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "test-token",
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "messages": []
    }))
    .unwrap();
    freeze_test_pending_provider_configuration(&storage, &mut agent_input);
    let now = mycopilot_core::storage::now_ms();
    let live_run_id = "mcp-envelope-live-run";
    let expired_run_id = "mcp-envelope-expired-run";
    let live_action_id = uuid::Uuid::new_v4().to_string();
    let expired_action_id = uuid::Uuid::new_v4().to_string();
    let live_invocation_id = uuid::Uuid::new_v4().to_string();
    let expired_invocation_id = uuid::Uuid::new_v4().to_string();
    let orphan_invocation_id = uuid::Uuid::new_v4().to_string();
    let mut live_agent_input = agent_input.clone();
    live_agent_input.resume_checkpoint = Some(test_mcp_resume_checkpoint(
        &storage,
        live_run_id,
        &live_action_id,
    ));
    let mut expired_agent_input = agent_input;
    expired_agent_input.resume_checkpoint = Some(test_mcp_resume_checkpoint(
        &storage,
        expired_run_id,
        &expired_action_id,
    ));
    let live_action =
        test_mcp_pending_action(live_run_id, &live_action_id, &live_invocation_id, now);
    let expired_action = test_mcp_pending_action(
        expired_run_id,
        &expired_action_id,
        &expired_invocation_id,
        now,
    );
    seed_durable_mcp_pending_owner(
        &storage,
        "mcp-envelope-live-conversation",
        "mcp-envelope-live-assistant",
        live_run_id,
        &live_action,
        now,
    );
    seed_durable_mcp_pending_owner(
        &storage,
        "mcp-envelope-expired-conversation",
        "mcp-envelope-expired-assistant",
        expired_run_id,
        &expired_action,
        now,
    );

    assert!(service
        .store_pending_action(
            live_run_id,
            "mcp-envelope-live-conversation",
            "mcp-envelope-live-assistant",
            live_action,
            live_agent_input,
        )
        .unwrap());
    assert!(service
        .store_pending_action(
            expired_run_id,
            "mcp-envelope-expired-conversation",
            "mcp-envelope-expired-assistant",
            expired_action,
            expired_agent_input,
        )
        .unwrap());

    // Headless child Turns do not have a Renderer maintaining `agent_run_json`. The durable
    // approval ticket must survive independently from its short-lived sealed payload envelope.
    {
        let connection = rusqlite::Connection::open(&database_path).unwrap();
        connection
            .execute(
                "UPDATE messages SET agent_run_json = NULL
                 WHERE conversation_id IN (
                    'mcp-envelope-live-conversation',
                    'mcp-envelope-expired-conversation'
                 )",
                [],
            )
            .unwrap();
    }

    storage
        .store_mcp_approval_envelope(test_mcp_envelope(
            &live_invocation_id,
            &pending_action_storage_id(live_run_id, &live_action_id),
            now,
            now + 60_000,
        ))
        .unwrap();
    storage
        .store_mcp_approval_envelope(test_mcp_envelope(
            &expired_invocation_id,
            &pending_action_storage_id(expired_run_id, &expired_action_id),
            now - 60_000,
            now - 1,
        ))
        .unwrap();
    storage
        .store_mcp_approval_envelope(test_mcp_envelope(
            &orphan_invocation_id,
            "missing-mcp-pending-action",
            now,
            now + 60_000,
        ))
        .unwrap();
    drop(service);

    let restarted = AgentService::try_new(Arc::clone(&storage))
        .expect("MCP envelope reconciliation must allow safe Agent startup");
    let pending = restarted.list_pending_actions();
    assert_eq!(pending.len(), 2);
    assert!(pending.iter().any(|action| action.run_id == live_run_id));
    assert!(pending.iter().any(|action| action.run_id == expired_run_id));
    assert_eq!(
        storage
            .list_recoverable_agent_actions_after_reconciliation()
            .unwrap()
            .len(),
        2
    );
    assert!(storage
        .load_mcp_approval_envelope(&live_invocation_id)
        .unwrap()
        .is_some());
    assert!(storage
        .load_mcp_approval_envelope(&expired_invocation_id)
        .unwrap()
        .is_none());
    assert!(storage
        .load_mcp_approval_envelope(&orphan_invocation_id)
        .unwrap()
        .is_none());
}

#[test]
fn mcp_startup_terminalization_uses_durable_identity_when_private_action_is_corrupt() {
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
    let run_id = "corrupt-mcp-run";
    let action_id = uuid::Uuid::new_v4().to_string();
    let invocation_id = uuid::Uuid::new_v4().to_string();
    let storage_id = pending_action_storage_id(run_id, &action_id);
    let action = test_mcp_pending_action(run_id, &action_id, &invocation_id, now);
    seed_durable_mcp_pending_owner(
        &storage,
        "corrupt-mcp-conversation",
        "corrupt-mcp-assistant",
        run_id,
        &action,
        now,
    );
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
    assert!(service
        .store_pending_action(
            run_id,
            "corrupt-mcp-conversation",
            "corrupt-mcp-assistant",
            action,
            input,
        )
        .unwrap());
    let row = storage
        .list_pending_agent_actions()
        .unwrap()
        .into_iter()
        .find(|row| row.action_id == storage_id)
        .unwrap();
    let approved_at = row.created_at.saturating_add(1);
    let executing_at = row.created_at.saturating_add(2);
    let terminal_at = row.created_at.saturating_add(3);
    storage
        .transition_pending_agent_action(
            &storage_id,
            "pending",
            "approved",
            &row.agent_input_json,
            approved_at,
        )
        .unwrap();
    storage
        .transition_pending_agent_action(
            &storage_id,
            "approved",
            "executing",
            &row.agent_input_json,
            executing_at,
        )
        .unwrap();

    // Simulate on-disk corruption without weakening the canonical write path. Normal writes are
    // rejected by the schema-level JSON validity constraint.
    let corruption = rusqlite::Connection::open(&database_path).unwrap();
    corruption
        .pragma_update(None, "ignore_check_constraints", 1)
        .unwrap();
    corruption
        .execute(
            "UPDATE agent_pending_actions SET action_json = ?1 WHERE action_id = ?2",
            ["{invalid typed MCP action", storage_id.as_str()],
        )
        .unwrap();
    corruption
        .pragma_update(None, "ignore_check_constraints", 0)
        .unwrap();
    drop(corruption);

    assert!(storage
        .terminalize_mcp_agent_action_on_startup(
            &storage_id,
            "executing",
            McpStartupActionTerminalOutcome::OutcomeUnknown,
            terminal_at,
        )
        .unwrap());
    let (status, action_json): (String, String) = rusqlite::Connection::open(database_path)
        .unwrap()
        .query_row(
            "SELECT status, action_json FROM agent_pending_actions WHERE action_id = ?1",
            [&storage_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(status, "failed");
    assert_eq!(action_json, "{}");
    let terminal = storage
        .get_conversation_turn_trace("corrupt-mcp-assistant")
        .unwrap()
        .unwrap();
    assert_eq!(
        terminal.terminal_status,
        ConversationTurnTraceTerminalStatus::Failed
    );
}

#[test]
fn typed_startup_keeps_durable_waiting_and_approved_but_never_replays_executing() {
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
    let mut identities = Vec::new();
    for status in [
        PendingActionStatus::Pending,
        PendingActionStatus::Approved,
        PendingActionStatus::Executing,
    ] {
        let label = pending_status_label(status);
        let run_id = format!("mcp-typed-startup-{label}-run");
        let action_id = uuid::Uuid::new_v4().to_string();
        let invocation_id = uuid::Uuid::new_v4().to_string();
        let storage_id = pending_action_storage_id(&run_id, &action_id);
        let conversation_id = format!("conversation-{label}");
        let assistant_message_id = format!("assistant-{label}");
        let action_created_at = if status == PendingActionStatus::Approved {
            now.saturating_sub(120_000)
        } else {
            now
        };
        let action =
            test_mcp_pending_action(&run_id, &action_id, &invocation_id, action_created_at);
        seed_durable_mcp_pending_owner(
            &storage,
            &conversation_id,
            &assistant_message_id,
            &run_id,
            &action,
            now,
        );
        let mut input = serde_json::from_value::<AgentChatInput>(json!({
            "apiUrl": "https://example.test/v1/chat/completions",
            "apiToken": "test-token",
            "model": "test-model",
            "modelCapabilities": { "imageInput": false },
            "messages": []
        }))
        .unwrap();
        input.resume_checkpoint = Some(test_mcp_resume_checkpoint(&storage, &run_id, &action_id));
        freeze_test_pending_provider_configuration(&storage, &mut input);
        assert!(service
            .store_pending_action(
                &run_id,
                &conversation_id,
                &assistant_message_id,
                action,
                input,
            )
            .unwrap());
        if status != PendingActionStatus::Pending {
            let row = storage
                .list_pending_agent_actions()
                .unwrap()
                .into_iter()
                .find(|row| row.action_id == storage_id)
                .unwrap();
            storage
                .transition_pending_agent_action(
                    &storage_id,
                    "pending",
                    "approved",
                    &row.agent_input_json,
                    now + 1,
                )
                .unwrap();
            if status == PendingActionStatus::Executing {
                storage
                    .transition_pending_agent_action(
                        &storage_id,
                        "approved",
                        "executing",
                        &row.agent_input_json,
                        now + 2,
                    )
                    .unwrap();
            }
        }
        identities.push((
            status,
            storage_id,
            conversation_id,
            assistant_message_id,
            invocation_id,
        ));
    }
    drop(service);

    let restarted = AgentService::try_new_deferred_startup_reconciliation(Arc::clone(&storage))
        .unwrap()
        .with_mcp_startup_inspector(Arc::new(StaticMcpStartupInspector(
            McpApprovalStartupPayloadState::Expired,
        )));
    assert_eq!(restarted.reconcile_startup_mcp_actions().unwrap(), 1);

    let in_memory = restarted
        .pending_actions
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    for (status, storage_id, _, _, _) in &identities {
        match status {
            PendingActionStatus::Pending | PendingActionStatus::Approved => {
                assert_eq!(in_memory[storage_id].snapshot.status, *status);
            }
            PendingActionStatus::Executing => assert!(!in_memory.contains_key(storage_id)),
            _ => unreachable!(),
        }
    }
    drop(in_memory);

    let visible_waiting = restarted.list_pending_actions();
    assert_eq!(visible_waiting.len(), 2);
    assert!(visible_waiting
        .iter()
        .any(|snapshot| snapshot.status == PendingActionStatus::Pending));
    assert!(visible_waiting
        .iter()
        .any(|snapshot| snapshot.status == PendingActionStatus::Approved));
    assert!(!visible_waiting
        .iter()
        .any(|snapshot| snapshot.status == PendingActionStatus::Executing));

    let connection = rusqlite::Connection::open(database_path).unwrap();
    let executing_id = identities
        .iter()
        .find(|(status, _, _, _, _)| *status == PendingActionStatus::Executing)
        .map(|(_, storage_id, _, _, _)| storage_id)
        .unwrap();
    let terminal: (String, String, String) = connection
        .query_row(
            "SELECT pending.status, pending.action_json, audit.error
             FROM agent_pending_actions pending
             JOIN agent_action_audit audit ON audit.action_id = pending.action_id
             WHERE pending.action_id = ?1",
            [executing_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(terminal.0, "failed");
    assert_eq!(terminal.1, "{}");
    assert_eq!(terminal.2, "mcp.tool_outcome_unknown");

    let (_, _, conversation_id, _, invocation_id) = identities
        .iter()
        .find(|(status, _, _, _, _)| *status == PendingActionStatus::Executing)
        .unwrap();
    let recovered = storage.load_conversation(conversation_id).unwrap().unwrap();
    let run: Value =
        serde_json::from_str(recovered.messages[0].agent_run_json.as_deref().unwrap()).unwrap();
    assert_eq!(run["status"], "failed");
    let invocations = run["mcpInvocations"].as_array().unwrap();
    assert_eq!(
        invocations
            .iter()
            .filter(|candidate| candidate["invocationId"] == invocation_id.as_str())
            .count(),
        1
    );
    let invocation = invocations
        .iter()
        .find(|candidate| candidate["invocationId"] == invocation_id.as_str())
        .unwrap();
    assert_eq!(invocation["state"], "outcome_unknown");
    assert_eq!(invocation["outcome"], "outcome_unknown");
    assert_eq!(invocation["dispatchCertainty"], "possibly_dispatched");
    assert_eq!(invocation["errorCode"], "mcp.tool_outcome_unknown");
    for forbidden in [
        "rawArguments",
        "rawResult",
        "stderr",
        "payloadRef",
        "ciphertext",
    ] {
        assert!(
            !invocation.as_object().unwrap().contains_key(forbidden),
            "terminal MCP projection must remove {forbidden}"
        );
    }
}
