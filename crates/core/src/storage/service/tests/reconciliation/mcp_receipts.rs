use super::fixtures::*;
use super::mcp_fixture::mcp_rejection_settlement;
use super::*;

#[test]
fn malformed_mcp_action_is_scrubbed_from_its_durable_mcp_identity() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let run_id = "run-malformed-mcp";
    let conversation_id = "conversation-malformed-mcp";
    let assistant_message_id = "assistant-malformed-mcp";
    let (mut pending, mut audit, _, mut trace, _) =
        mcp_rejection_settlement(run_id, conversation_id, assistant_message_id);
    trace.items.truncate(1);
    pending.action_json = serde_json::json!({ "unknownMcpAction": true }).to_string();
    pending.agent_input_json = serde_json::json!({ "unknownMcpResume": true }).to_string();
    audit.action_json =
        serde_json::json!({ "privateMcpActionCanary": "PRIVATE_MCP_ACTION_CANARY" }).to_string();
    save_assistant_conversation(&service, conversation_id, assistant_message_id);
    service
        .append_in_progress_conversation_turn_trace_and_apply_guidances(
            &trace,
            &current_model_context_for_trace(&trace),
            10,
            10,
        )
        .unwrap();
    service.store_pending_agent_action(pending.clone()).unwrap();
    service.upsert_agent_action_audit(audit).unwrap();

    assert!(service
        .terminalize_mcp_agent_action_on_startup(
            &pending.action_id,
            "pending",
            McpStartupActionTerminalOutcome::PayloadUnavailable,
            42,
        )
        .unwrap());

    let connection = service.state.connection().unwrap();
    let scrubbed: (String, String, String, String) = connection
        .query_row(
            "SELECT pending.status, pending.action_json, pending.agent_input_json,
                    audit.action_json
             FROM agent_pending_actions AS pending
             JOIN agent_action_audit AS audit ON audit.action_id = pending.action_id
             WHERE pending.action_id = ?1",
            [&pending.action_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(
        scrubbed,
        (
            "failed".to_string(),
            "{}".to_string(),
            "{}".to_string(),
            "{}".to_string(),
        )
    );
    drop(connection);
    let terminal_trace = service
        .get_conversation_turn_trace(assistant_message_id)
        .unwrap()
        .unwrap();
    let terminal_context = service
        .get_conversation_model_context_log(assistant_message_id)
        .unwrap()
        .unwrap();
    terminal_trace
        .validate_complete_model_context(&terminal_context.items)
        .unwrap();
}

#[test]
fn mcp_server_tool_error_is_a_completed_authoritative_receipt() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let run_id = "run-mcp-tool-error";
    let conversation_id = "conversation-mcp-tool-error";
    let assistant_message_id = "assistant-mcp-tool-error";
    let (mut pending, mut preterminal_audit, _, mut trace, call) =
        mcp_rejection_settlement(run_id, conversation_id, assistant_message_id);
    pending.status = "executing".to_string();
    preterminal_audit.status = "approved".to_string();
    preterminal_audit.decision = Some("approved".to_string());
    preterminal_audit.decided_at = Some(11);
    preterminal_audit.decision_source = Some("manual".to_string());

    let live_result = AgentToolResult {
        exact_archive_file: None,
        call_id: call.id.clone(),
        tool: call.tool.clone(),
        ok: false,
        result: Some(serde_json::json!({
            "schemaVersion": 1,
            "type": "mcp_tool",
            "external": true,
            "status": "completed",
            "outcome": "tool_error",
            "dispatchCertainty": "response_received",
            "isError": true,
            "content": [{ "type": "text", "text": "fixture error omitted durably" }]
        })),
        error: Some("fixture server returned isError".to_string()),
    };
    let durable_result = crate::mcp_tool_result_persistence_projection(&live_result);
    let mut terminal_audit = preterminal_audit.clone();
    terminal_audit.status = "completed".to_string();
    terminal_audit.tool_result_json = Some(serde_json::to_string(&durable_result).unwrap());
    terminal_audit.error = durable_result.error.clone();
    terminal_audit.blocked_reason = durable_result.error.clone();
    terminal_audit.completed_at = Some(12);
    trace.items[1] =
        crate::conversation_trace::projected_tool_result_trace_item(1, &call, &durable_result);

    save_assistant_conversation(&service, conversation_id, assistant_message_id);
    service.store_pending_agent_action(pending).unwrap();
    service
        .upsert_agent_action_audit(preterminal_audit)
        .unwrap();
    assert_eq!(
        service
            .commit_current_manual_settlement(
                &terminal_audit,
                "executing",
                "completed",
                &trace,
                12,
            )
            .unwrap(),
        AgentPendingActionResultCommitOutcome::Committed {
            trace_changed: true,
        }
    );

    let connection = service.state.connection().unwrap();
    let (pending_status, target_status, audit_status, persisted_result): (
        String,
        Option<String>,
        String,
        String,
    ) = connection
        .query_row(
            "
            SELECT pending.status, pending.target_status, audit.status, audit.tool_result_json
            FROM agent_pending_actions pending
            JOIN agent_action_audit audit ON audit.action_id = pending.action_id
            WHERE pending.run_id = ?1
            ",
            [run_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(pending_status, "executing");
    assert_eq!(target_status.as_deref(), Some("completed"));
    assert_eq!(audit_status, "completed");
    assert_eq!(
        serde_json::to_value(serde_json::from_str::<AgentToolResult>(&persisted_result).unwrap())
            .unwrap(),
        serde_json::to_value(durable_result).unwrap()
    );
}

#[test]
fn mcp_durable_receipt_rejects_unknown_canary_fields_without_partial_commit() {
    const CANARY: &str = "MCP_NEUTRAL_FIELD_CANARY_MUST_NOT_PERSIST";
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let run_id = "run-mcp-rejection-canary";
    let conversation_id = "conversation-mcp-rejection-canary";
    let assistant_message_id = "assistant-mcp-rejection-canary";
    let (pending, pending_audit, mut terminal_audit, mut trace, call) =
        mcp_rejection_settlement(run_id, conversation_id, assistant_message_id);
    let mut tampered_result = serde_json::from_str::<AgentToolResult>(
        terminal_audit.tool_result_json.as_deref().unwrap(),
    )
    .unwrap();
    tampered_result
        .result
        .as_mut()
        .and_then(serde_json::Value::as_object_mut)
        .unwrap()
        .insert(
            "neutralData".to_string(),
            serde_json::Value::String(CANARY.to_string()),
        );
    terminal_audit.tool_result_json = Some(serde_json::to_string(&tampered_result).unwrap());
    trace.items[1] =
        crate::conversation_trace::projected_tool_result_trace_item(1, &call, &tampered_result);

    let storage_id = pending.action_id.clone();
    save_assistant_conversation(&service, conversation_id, assistant_message_id);
    service.store_pending_agent_action(pending).unwrap();
    service.upsert_agent_action_audit(pending_audit).unwrap();
    let error = service
        .commit_current_manual_settlement(&terminal_audit, "pending", "rejected", &trace, 12)
        .unwrap_err();
    assert!(error.contains("unknown field"), "{error}");

    let connection = service.state.connection().unwrap();
    let (target_status, audit_status, tool_result_json): (Option<String>, String, Option<String>) =
        connection
            .query_row(
                "
            SELECT pending.target_status, audit.status, audit.tool_result_json
            FROM agent_pending_actions pending
            JOIN agent_action_audit audit ON audit.action_id = pending.action_id
            WHERE pending.action_id = ?1
            ",
                [&storage_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
    assert_eq!(target_status, None);
    assert_eq!(audit_status, "pending");
    assert_eq!(tool_result_json, None);
    assert_eq!(
        connection
            .query_row(
                "SELECT instr(COALESCE(tool_result_json, ''), ?2)
                 FROM agent_action_audit
                 WHERE action_id = ?1",
                rusqlite::params![storage_id, CANARY],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        0
    );
    drop(connection);
    assert!(service
        .get_conversation_turn_trace(assistant_message_id)
        .unwrap()
        .is_none());
}
