use super::builtin_fixtures::store_builtin_sensitive_test_pending;
use super::fixtures::{seed_durable_pending_owner, test_pending_resume_checkpoint_for_call};
use super::*;

#[test]
fn builtin_sensitive_startup_terminalization_is_typed_atomic_and_secret_free() {
    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("builtin-sensitive-startup.sqlite");
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
    let now_ms = mycopilot_core::storage::now_ms();
    let now_seconds = u64::try_from(now_ms).unwrap() / 1_000;
    let secret = "BUILTIN_STARTUP_RAW_ARGS_MUST_NEVER_PERSIST";
    let cases = [
        (
            "pending",
            McpStartupActionTerminalOutcome::PayloadUnavailable,
            "payload_unavailable",
            "definitely_not_dispatched",
        ),
        (
            "approved",
            McpStartupActionTerminalOutcome::PayloadUnavailable,
            "payload_unavailable",
            "definitely_not_dispatched",
        ),
        (
            "executing",
            McpStartupActionTerminalOutcome::OutcomeUnknown,
            "outcome_unknown",
            "possibly_dispatched",
        ),
        (
            "pending",
            McpStartupActionTerminalOutcome::Expired,
            "expired",
            "definitely_not_dispatched",
        ),
    ];

    for (index, (expected_status, outcome, result_status, certainty)) in
        cases.into_iter().enumerate()
    {
        let run_id = format!("builtin-sensitive-startup-run-{index}");
        let conversation_id = format!("builtin-sensitive-startup-conversation-{index}");
        let assistant_message_id = format!("builtin-sensitive-startup-assistant-{index}");
        let action_id = uuid::Uuid::new_v4().to_string();
        let call = AgentToolCall {
            id: format!("builtin-sensitive-startup-call-{index}"),
            tool: "browser_evaluate".to_string(),
            // The raw evaluate function/password/cookie/file handle is process-sealed. The
            // durable pending ToolCall is the exact private projection and contains no args.
            args: json!({}),
            approval_status: AgentApprovalStatus::Required,
            reason: Some("Run the reviewed page script.".to_string()),
        };
        let approval = mycopilot_core::AgentBuiltinMcpToolApproval {
            schema_version: mycopilot_core::BUILTIN_MCP_TOOL_APPROVAL_SCHEMA_VERSION,
            identity: mycopilot_core::BuiltinMcpToolApprovalIdentity {
                action_id: action_id.clone(),
                approval_id: uuid::Uuid::new_v4().to_string(),
                run_id: run_id.clone(),
                call_id: call.id.clone(),
                capability_id: "browser_automation".to_string(),
                capability_activation_id: uuid::Uuid::new_v4().to_string(),
                managed_mcp_id: "builtin.browser_automation.mcp".to_string(),
                package_name: "@playwright/mcp".to_string(),
                package_version: "0.0.79".to_string(),
                upstream_catalog_digest: format!("sha256:{}", "1".repeat(64)),
                manifest_digest: format!("sha256:{}", "2".repeat(64)),
                policy_digest: format!("sha256:{}", "3".repeat(64)),
                policy_revision: 1,
                tool_id: "browser_evaluate".to_string(),
                raw_name: "browser_evaluate".to_string(),
                model_name: "browser_evaluate".to_string(),
                upstream_schema_digest: format!("sha256:{}", "4".repeat(64)),
                host_overlay_digest: format!("sha256:{}", "5".repeat(64)),
                host_input_schema_digest: format!("sha256:{}", "6".repeat(64)),
                arguments_digest: format!("sha256:{}", "7".repeat(64)),
                resource_scope_digest: format!("sha256:{}", "8".repeat(64)),
                origin: Some("https://mail.example.test".to_string()),
            },
            capability_display_name: "Browser automation".to_string(),
            tool_display_name: "Evaluate page script".to_string(),
            call_reason: "Run the reviewed page script.".to_string(),
            operation_category: "page_script_execution".to_string(),
            resource_summary: mycopilot_core::BuiltinMcpToolResourceSummary {
                scope: "page_script_execution".to_string(),
                display_name: "Current managed page".to_string(),
                file_basenames: Vec::new(),
                origin: Some("https://mail.example.test".to_string()),
            },
            risk_kinds: vec![mycopilot_core::BuiltinMcpToolRiskKind::PageScriptExecution],
            created_at: now_seconds,
            expires_at: now_seconds.saturating_add(900),
            approval_status: AgentApprovalStatus::Required,
        };
        let action = AgentProposedAction::BuiltinMcpToolApproval {
            approval: Box::new(approval),
        };
        let provenance = AgentToolIdentity::BuiltinCapability {
            capability_id: "browser_automation".into(),
            managed_mcp_id: "builtin.browser_automation.mcp".into(),
            package_name: "@playwright/mcp".into(),
            package_version: "0.0.79".into(),
            upstream_catalog_digest: format!("sha256:{}", "1".repeat(64)).into(),
            policy_digest: format!("sha256:{}", "3".repeat(64)).into(),
            manifest_digest: format!("sha256:{}", "2".repeat(64)).into(),
            tool_id: "browser_evaluate".into(),
            raw_name: "browser_evaluate".into(),
            model_name: "browser_evaluate".into(),
            upstream_schema_digest: format!("sha256:{}", "4".repeat(64)).into(),
            host_overlay_digest: format!("sha256:{}", "5".repeat(64)).into(),
            host_input_schema_digest: format!("sha256:{}", "6".repeat(64)).into(),
        };
        let mut input: AgentChatInput = serde_json::from_value(json!({
            "apiUrl": "https://example.test/v1/chat/completions",
            "apiToken": "test-token",
            "model": "test-model",
            "modelCapabilities": { "imageInput": false },
            "messages": []
        }))
        .unwrap();
        freeze_test_pending_provider_configuration(&storage, &mut input);
        input.resume_checkpoint = Some(test_pending_resume_checkpoint_for_call(
            &storage,
            &run_id,
            Some(&action_id),
            &call,
            provenance.clone(),
        ));
        seed_durable_pending_owner(
            &storage,
            &conversation_id,
            &assistant_message_id,
            &run_id,
            &call,
            provenance,
            now_ms.saturating_add(i64::try_from(index).unwrap()),
        );
        assert!(service
            .store_pending_action(
                &run_id,
                &conversation_id,
                &assistant_message_id,
                action,
                input,
            )
            .unwrap());
        let storage_id = pending_action_storage_id(&run_id, &action_id);
        if expected_status != "pending" {
            let connection = rusqlite::Connection::open(&database_path).unwrap();
            connection
                .execute(
                    "UPDATE agent_pending_actions SET status = ?2 WHERE action_id = ?1",
                    rusqlite::params![storage_id, expected_status],
                )
                .unwrap();
            connection
                .execute(
                    "UPDATE agent_action_audit SET status = ?2 WHERE action_id = ?1",
                    rusqlite::params![storage_id, expected_status],
                )
                .unwrap();
        }
        let prefix = storage
            .get_conversation_turn_trace(&assistant_message_id)
            .unwrap()
            .unwrap();
        let durable_row = storage
            .get_pending_agent_action(&storage_id)
            .unwrap()
            .unwrap();
        let terminal_at = durable_row
            .created_at
            .max(durable_row.updated_at)
            .saturating_add(1_000);
        assert!(storage
            .terminalize_builtin_mcp_tool_agent_action_on_startup(
                &storage_id,
                expected_status,
                outcome,
                terminal_at,
            )
            .unwrap());
        assert!(!storage
            .terminalize_builtin_mcp_tool_agent_action_on_startup(
                &storage_id,
                expected_status,
                outcome,
                terminal_at.saturating_add(1),
            )
            .unwrap());
        let trace = storage
            .get_conversation_turn_trace(&assistant_message_id)
            .unwrap()
            .unwrap();
        assert_eq!(&trace.items[..prefix.items.len()], prefix.items.as_slice());
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
        assert!(rendered.contains(&format!("\"status\":\"{result_status}\"")));
        assert!(rendered.contains(&format!("\"dispatchCertainty\":\"{certainty}\"")));
        assert!(!rendered.contains(secret));
        let retired = storage
            .get_pending_agent_action(&storage_id)
            .unwrap()
            .unwrap();
        assert_eq!(retired.status, "failed");
        assert_eq!(retired.action_json, "{}");
        assert_eq!(retired.agent_input_json, "{}");
    }
}

#[test]
fn builtin_sensitive_rejection_crash_window_recovers_the_exact_rejected_receipt() {
    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("builtin-sensitive-reject-crash.sqlite");
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    save_test_pending_provider(
        &storage,
        "test-model",
        "http://127.0.0.1:9/v1/chat/completions",
        "test-token",
        "disabled",
        "",
    );
    let service = AgentService::new_authorized_for_test(Arc::clone(&storage));
    let run_id = "builtin-sensitive-reject-crash-run";
    let conversation_id = "builtin-sensitive-reject-crash-conversation";
    let assistant_message_id = "builtin-sensitive-reject-crash-assistant";
    let (action_id, call) = store_builtin_sensitive_test_pending(
        &service,
        &storage,
        run_id,
        conversation_id,
        assistant_message_id,
        "builtin-sensitive-reject-crash-call",
    );
    let storage_id = pending_action_storage_id(run_id, &action_id);
    let record = service
        .pending_actions
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(&storage_id)
        .unwrap()
        .clone();
    let AgentProposedAction::BuiltinMcpToolApproval { approval } = &record.snapshot.action else {
        panic!("expected the typed built-in MCP approval");
    };
    let feedback_secret =
        "使用密码 UNLABELLED_PASSWORD_CANARY_7Yp9；Use UNLABELLED_REJECTION_SECRET";
    let live_result =
        mycopilot_core::builtin_mcp_tool_rejected_result(approval, Some(feedback_secret));
    let mut persisted_input = record.agent_input.clone();
    persisted_input.approval_decision = Some(AgentApprovalDecision {
        action_id: action_id.clone(),
        status: AgentApprovalDecisionStatus::Rejected,
        message: None,
    });
    persisted_input.tool_continuation = Some(AgentToolContinuation {
        call: call.clone(),
        result: mycopilot_core::builtin_capability_tool_result_persistence_projection(&live_result),
    });
    let completed_at = mycopilot_core::storage::now_ms();
    let (notifications, _receiver) = tokio::sync::mpsc::unbounded_channel();
    service
        .commit_rejected_mcp_receipt(&record, &persisted_input, completed_at, &notifications)
        .unwrap();

    let crash_window = storage
        .get_pending_agent_action(&storage_id)
        .unwrap()
        .unwrap();
    assert_eq!(crash_window.status, "pending");
    assert_eq!(crash_window.target_status.as_deref(), Some("rejected"));
    let committed_prefix = storage
        .get_conversation_turn_trace(assistant_message_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        committed_prefix
            .items
            .iter()
            .filter(|item| matches!(
                item,
                ConversationTurnTraceItem::ToolResult { call_id, .. } if call_id == &call.id
            ))
            .count(),
        1
    );
    let prefix_json = serde_json::to_string(&committed_prefix).unwrap();
    assert!(prefix_json.contains("\"status\":\"rejected\""));
    assert!(prefix_json.contains("\"dispatchCertainty\":\"definitely_not_dispatched\""));
    assert!(!prefix_json.contains("payload_unavailable"));
    assert!(!prefix_json.contains("outcome_unknown"));
    assert!(!prefix_json.contains("UNLABELLED_PASSWORD_CANARY_7Yp9"));
    assert!(!prefix_json.contains("UNLABELLED_REJECTION_SECRET"));

    // Simulate a process crash after the atomic receipt but before the in-memory continuation
    // claim. Startup must adopt that exact receipt, never reinterpret refusal as dispatch.
    drop(service);
    let restarted = AgentService::new_authorized_for_test(Arc::clone(&storage));
    assert!(!restarted
        .pending_actions
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .contains_key(&storage_id));
    let retired = storage
        .get_pending_agent_action(&storage_id)
        .unwrap()
        .unwrap();
    assert_eq!(retired.status, "rejected");
    let trace = storage
        .get_conversation_turn_trace(assistant_message_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        &trace.items[..committed_prefix.items.len()],
        committed_prefix.items.as_slice()
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
    let trace_json = serde_json::to_string(&trace).unwrap();
    assert!(!trace_json.contains("payload_unavailable"));
    assert!(!trace_json.contains("outcome_unknown"));
    assert!(!trace_json.contains("UNLABELLED_PASSWORD_CANARY_7Yp9"));
    assert!(!trace_json.contains("UNLABELLED_REJECTION_SECRET"));
}
