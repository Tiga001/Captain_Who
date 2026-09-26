use super::fixtures::{seed_durable_pending_owner, test_pending_resume_checkpoint_for_call};
use super::mcp_fixtures::{
    seed_durable_mcp_pending_owner, test_mcp_envelope, test_mcp_pending_action,
    test_mcp_resume_checkpoint, InvalidatingMcpInvoker,
};
use super::*;

#[derive(Clone)]
struct TestMcpActionSource<'a> {
    server_id: &'a str,
    scope: mycopilot_core::AgentMcpServerScope,
    config_digest: &'a str,
    config_epoch: Option<&'a str>,
    registry_revision: u64,
    catalog_generation: u64,
}

fn store_test_mcp_action_for_source(
    service: &AgentService,
    run_id: &str,
    status: PendingActionStatus,
    source: &TestMcpActionSource<'_>,
    created_at: i64,
) -> (String, String) {
    let action_id = uuid::Uuid::new_v4().to_string();
    let invocation_id = uuid::Uuid::new_v4().to_string();
    let mut action = test_mcp_pending_action(run_id, &action_id, &invocation_id, created_at);
    let AgentProposedAction::McpToolCall { approval } = &mut action else {
        unreachable!("test helper always creates an MCP action");
    };
    approval.identity.provenance.server_id = source.server_id.to_string();
    approval.identity.provenance.scope = source.scope.clone();
    approval.identity.provenance.config_digest = source.config_digest.to_string();
    if let Some(config_epoch) = source.config_epoch {
        approval.identity.provenance.config_epoch = config_epoch.to_string();
    }
    approval.identity.provenance.registry_revision = source.registry_revision;
    approval.identity.provenance.catalog_generation = source.catalog_generation;
    approval.summary.server_id = source.server_id.to_string();
    approval.summary.scope = source.scope.clone();

    let mut input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "test-token",
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "messages": []
    }))
    .unwrap();
    input.resume_checkpoint = Some(test_mcp_resume_checkpoint(
        &service.storage,
        run_id,
        &action_id,
    ));
    freeze_test_pending_provider_configuration(&service.storage, &mut input);
    let conversation_id = format!("conversation-{run_id}");
    let assistant_message_id = format!("assistant-{run_id}");
    seed_durable_mcp_pending_owner(
        &service.storage,
        &conversation_id,
        &assistant_message_id,
        run_id,
        &action,
        created_at,
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

    let storage_id = pending_action_storage_id(run_id, &action_id);
    if status != PendingActionStatus::Pending {
        let mut pending_actions = service
            .pending_actions
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let record = pending_actions.get_mut(&storage_id).unwrap();
        service
            .persist_pending_status(
                record,
                PendingActionStatus::Pending,
                PendingActionStatus::Approved,
            )
            .unwrap();
        record.snapshot.status = PendingActionStatus::Approved;
        if status == PendingActionStatus::Executing {
            service
                .persist_pending_status(
                    record,
                    PendingActionStatus::Approved,
                    PendingActionStatus::Executing,
                )
                .unwrap();
            record.snapshot.status = PendingActionStatus::Executing;
        }
    }
    (storage_id, invocation_id)
}

#[test]
fn server_source_invalidation_atomically_scrubs_predispatch_and_marks_executing_unknown() {
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
    let invoker = Arc::new(InvalidatingMcpInvoker {
        storage: Some(Arc::clone(&storage)),
        ..InvalidatingMcpInvoker::default()
    });
    let service = AgentService::new_authorized_for_test(Arc::clone(&storage))
        .with_mcp_tool_invoker(invoker.clone() as Arc<dyn McpToolInvoker>);
    let server_uuid = uuid::Uuid::new_v4();
    let server_id = server_uuid.to_string();
    let other_server_id = uuid::Uuid::new_v4().to_string();
    let selected_digest = "1".repeat(64);
    let other_digest = "9".repeat(64);
    let now = mycopilot_core::storage::now_ms();
    let selected_source = TestMcpActionSource {
        server_id: &server_id,
        scope: mycopilot_core::AgentMcpServerScope::User,
        config_digest: &selected_digest,
        config_epoch: None,
        registry_revision: 11,
        catalog_generation: 1,
    };
    let filtered_source = TestMcpActionSource {
        server_id: &server_id,
        scope: mycopilot_core::AgentMcpServerScope::User,
        config_digest: &other_digest,
        config_epoch: None,
        registry_revision: 11,
        catalog_generation: 1,
    };
    let unrelated_source = TestMcpActionSource {
        server_id: &other_server_id,
        scope: mycopilot_core::AgentMcpServerScope::User,
        config_digest: &selected_digest,
        config_epoch: None,
        registry_revision: 11,
        catalog_generation: 1,
    };
    let newer_same_source_identity = TestMcpActionSource {
        server_id: &server_id,
        scope: mycopilot_core::AgentMcpServerScope::User,
        config_digest: &selected_digest,
        config_epoch: None,
        registry_revision: 13,
        catalog_generation: 1,
    };

    let mut selected = Vec::new();
    for (suffix, status) in [
        ("pending", PendingActionStatus::Pending),
        ("approved", PendingActionStatus::Approved),
        ("executing", PendingActionStatus::Executing),
    ] {
        selected.push((
            status,
            store_test_mcp_action_for_source(
                &service,
                &format!("server-removal-{suffix}-run"),
                status,
                &selected_source,
                now,
            ),
        ));
    }
    let filtered = store_test_mcp_action_for_source(
        &service,
        "server-removal-filtered-run",
        PendingActionStatus::Pending,
        &filtered_source,
        now,
    );
    let unrelated_mcp = store_test_mcp_action_for_source(
        &service,
        "server-removal-unrelated-mcp-run",
        PendingActionStatus::Pending,
        &unrelated_source,
        now,
    );
    let newer_same_source = store_test_mcp_action_for_source(
        &service,
        "server-removal-newer-same-source-run",
        PendingActionStatus::Pending,
        &newer_same_source_identity,
        now,
    );
    let non_mcp_run_id = "server-removal-non-mcp-run";
    let non_mcp_action_id = "server-removal-non-mcp-action";
    let non_mcp_storage_id = pending_action_storage_id(non_mcp_run_id, non_mcp_action_id);
    let mut non_mcp_input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "test-token",
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "messages": []
    }))
    .unwrap();
    freeze_test_pending_provider_configuration(&storage, &mut non_mcp_input);
    let non_mcp_call = AgentToolCall {
        id: non_mcp_action_id.to_string(),
        tool: "approval_tool".to_string(),
        args: json!({}),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };
    non_mcp_input.resume_checkpoint = Some(test_pending_resume_checkpoint_for_call(
        &storage,
        non_mcp_run_id,
        None,
        &non_mcp_call,
        AgentToolIdentity::Builtin {
            tool_name: non_mcp_call.tool.clone(),
        },
    ));
    seed_durable_pending_owner(
        &storage,
        "server-removal-non-mcp-conversation",
        "server-removal-non-mcp-assistant",
        non_mcp_run_id,
        &non_mcp_call,
        AgentToolIdentity::Builtin {
            tool_name: non_mcp_call.tool.clone(),
        },
        now,
    );
    assert!(service
        .store_pending_action(
            non_mcp_run_id,
            "server-removal-non-mcp-conversation",
            "server-removal-non-mcp-assistant",
            AgentProposedAction::ToolCall { call: non_mcp_call },
            non_mcp_input,
        )
        .unwrap());

    for (storage_id, invocation_id) in selected
        .iter()
        .map(|(_, identity)| identity)
        .chain(std::iter::once(&filtered))
        .chain(std::iter::once(&unrelated_mcp))
        .chain(std::iter::once(&newer_same_source))
    {
        storage
            .store_mcp_approval_envelope(test_mcp_envelope(
                invocation_id,
                storage_id,
                now,
                now + 60_000,
            ))
            .unwrap();
    }

    let target = McpActionInvalidationTarget::server(McpServerId::from_uuid(server_uuid))
        .with_scope(mycopilot_core::AgentMcpServerScope::User)
        .with_source_config_digest(selected_digest.parse::<McpConfigDigest>().unwrap())
        .prior_to_registry_revision(12);
    let summary = service
        .invalidate_mcp_actions_for_server(&target, McpStartupActionTerminalOutcome::PolicyDenied)
        .unwrap();
    assert_eq!(
        summary,
        McpActionInvalidationSummary {
            terminalized_before_dispatch: 1,
            terminalized_outcome_unknown: 1,
            payload_invalidation_attempts: 3,
            payload_invalidation_failures: 0,
        }
    );
    assert_eq!(
        invoker
            .invalidations
            .load(std::sync::atomic::Ordering::SeqCst),
        3
    );

    let connection = rusqlite::Connection::open(&database_path).unwrap();
    for (status, (storage_id, invocation_id)) in &selected {
        let terminal: (String, String, String, String, Option<String>) = connection
            .query_row(
                "SELECT pending.status, pending.action_json, pending.agent_input_json,
                        audit.action_json, audit.error
                 FROM agent_pending_actions pending
                 JOIN agent_action_audit audit ON audit.action_id = pending.action_id
                 WHERE pending.action_id = ?1",
                [storage_id],
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
        if *status == PendingActionStatus::Pending {
            assert_eq!(terminal.0, "pending");
            assert_ne!(terminal.1, "{}");
            assert_ne!(terminal.2, "{}");
            assert_ne!(terminal.3, "{}");
        } else {
            assert_eq!(terminal.0, "failed");
            assert_eq!(terminal.1, "{}");
            assert_eq!(terminal.2, "{}");
            assert_eq!(terminal.3, "{}");
            assert_eq!(
                terminal.4.as_deref(),
                Some(if *status == PendingActionStatus::Executing {
                    "mcp.tool_outcome_unknown"
                } else {
                    "mcp.approval_policy_denied"
                })
            );
        }
        assert!(storage
            .load_mcp_approval_envelope(invocation_id)
            .unwrap()
            .is_none());
    }
    assert!(storage
        .load_mcp_approval_envelope(&filtered.1)
        .unwrap()
        .is_some());
    assert!(storage
        .load_mcp_approval_envelope(&unrelated_mcp.1)
        .unwrap()
        .is_some());
    assert!(storage
        .load_mcp_approval_envelope(&newer_same_source.1)
        .unwrap()
        .is_some());

    let filtered_summary = service
        .invalidate_mcp_actions_for_server(
            &McpActionInvalidationTarget::server(McpServerId::from_uuid(server_uuid))
                .with_source_config_digest(other_digest.parse::<McpConfigDigest>().unwrap()),
            McpStartupActionTerminalOutcome::PayloadUnavailable,
        )
        .unwrap();
    assert_eq!(filtered_summary.terminalized_before_dispatch, 0);
    assert_eq!(filtered_summary.terminalized_outcome_unknown, 0);
    assert_eq!(filtered_summary.payload_invalidation_attempts, 1);
    assert_eq!(filtered_summary.payload_invalidation_failures, 0);
    let filtered_status: String = connection
        .query_row(
            "SELECT status FROM agent_pending_actions WHERE action_id = ?1",
            [&filtered.0],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(filtered_status, "pending");
    assert!(storage
        .load_mcp_approval_envelope(&filtered.1)
        .unwrap()
        .is_none());

    let in_memory = service
        .pending_actions
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    assert_eq!(in_memory.len(), 5);
    assert!(in_memory.contains_key(&selected[0].1 .0));
    assert!(in_memory.contains_key(&filtered.0));
    assert!(in_memory.contains_key(&unrelated_mcp.0));
    assert!(in_memory.contains_key(&newer_same_source.0));
    assert!(in_memory.contains_key(&non_mcp_storage_id));
    assert_eq!(
        in_memory[&non_mcp_storage_id].snapshot.status,
        PendingActionStatus::Pending
    );
    drop(in_memory);
    let non_mcp_status: String = connection
        .query_row(
            "SELECT status FROM agent_pending_actions WHERE action_id = ?1",
            [&non_mcp_storage_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(non_mcp_status, "pending");
    assert_eq!(
        invoker
            .invalidations
            .load(std::sync::atomic::Ordering::SeqCst),
        4
    );
}

#[test]
fn catalog_generation_invalidation_targets_only_prior_generation_of_same_config_epoch() {
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
    let invoker = Arc::new(InvalidatingMcpInvoker {
        storage: Some(Arc::clone(&storage)),
        ..InvalidatingMcpInvoker::default()
    });
    let service = AgentService::new_authorized_for_test(Arc::clone(&storage))
        .with_mcp_tool_invoker(invoker.clone() as Arc<dyn McpToolInvoker>);
    let server_uuid = uuid::Uuid::new_v4();
    let server_id = server_uuid.to_string();
    let config_digest = "7".repeat(64);
    let config_epoch = mycopilot_mcp_client::McpConfigEpoch::new();
    let config_epoch_string = config_epoch.to_string();
    let other_epoch_string = mycopilot_mcp_client::McpConfigEpoch::new().to_string();
    let now = mycopilot_core::storage::now_ms();
    let prior_source = TestMcpActionSource {
        server_id: &server_id,
        scope: mycopilot_core::AgentMcpServerScope::User,
        config_digest: &config_digest,
        config_epoch: Some(&config_epoch_string),
        registry_revision: 5,
        catalog_generation: 2,
    };
    let current_source = TestMcpActionSource {
        catalog_generation: 3,
        ..prior_source.clone()
    };
    let other_epoch_source = TestMcpActionSource {
        config_epoch: Some(&other_epoch_string),
        catalog_generation: 2,
        ..prior_source.clone()
    };

    let mut prior = Vec::new();
    for (suffix, status) in [
        ("pending", PendingActionStatus::Pending),
        ("approved", PendingActionStatus::Approved),
        ("executing", PendingActionStatus::Executing),
    ] {
        prior.push((
            status,
            store_test_mcp_action_for_source(
                &service,
                &format!("catalog-prior-{suffix}"),
                status,
                &prior_source,
                now,
            ),
        ));
    }
    let current = store_test_mcp_action_for_source(
        &service,
        "catalog-current-pending",
        PendingActionStatus::Pending,
        &current_source,
        now,
    );
    let other_epoch = store_test_mcp_action_for_source(
        &service,
        "catalog-other-epoch-pending",
        PendingActionStatus::Pending,
        &other_epoch_source,
        now,
    );
    for (storage_id, invocation_id) in prior
        .iter()
        .map(|(_, identity)| identity)
        .chain(std::iter::once(&current))
        .chain(std::iter::once(&other_epoch))
    {
        storage
            .store_mcp_approval_envelope(test_mcp_envelope(
                invocation_id,
                storage_id,
                now,
                now + 60_000,
            ))
            .unwrap();
    }

    let summary = service
        .invalidate_mcp_actions_for_server(
            &McpActionInvalidationTarget::server(McpServerId::from_uuid(server_uuid))
                .with_source_config_digest(config_digest.parse::<McpConfigDigest>().unwrap())
                .with_source_config_epoch(config_epoch)
                .prior_to_catalog_generation(3),
            McpStartupActionTerminalOutcome::PolicyDenied,
        )
        .unwrap();
    assert_eq!(summary.terminalized_before_dispatch, 1);
    assert_eq!(summary.terminalized_outcome_unknown, 1);
    assert_eq!(summary.payload_invalidation_attempts, 3);
    assert_eq!(summary.payload_invalidation_failures, 0);

    let connection = rusqlite::Connection::open(database_path).unwrap();
    for (status, (storage_id, invocation_id)) in &prior {
        let terminal: (String, Option<String>) = connection
            .query_row(
                "SELECT pending.status, audit.error
                 FROM agent_pending_actions pending
                 JOIN agent_action_audit audit ON audit.action_id = pending.action_id
                 WHERE pending.action_id = ?1",
                [storage_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        if *status == PendingActionStatus::Pending {
            assert_eq!(terminal.0, "pending");
        } else {
            assert_eq!(terminal.0, "failed");
            assert_eq!(
                terminal.1.as_deref(),
                Some(if *status == PendingActionStatus::Executing {
                    "mcp.tool_outcome_unknown"
                } else {
                    "mcp.approval_policy_denied"
                })
            );
        }
        assert!(storage
            .load_mcp_approval_envelope(invocation_id)
            .unwrap()
            .is_none());
    }
    for (storage_id, invocation_id) in [&current, &other_epoch] {
        let status: String = connection
            .query_row(
                "SELECT status FROM agent_pending_actions WHERE action_id = ?1",
                [storage_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(status, "pending");
        assert!(storage
            .load_mcp_approval_envelope(invocation_id)
            .unwrap()
            .is_some());
    }
}
