use super::fixtures::*;
use super::mcp_fixture::mcp_rejection_settlement;
use super::*;

fn install_terminal_mcp_outcome_unknown_projection(
    fixture: &StorageFixture,
    conversation_id: &str,
    assistant_message_id: &str,
    action: &AgentProposedAction,
) {
    let AgentProposedAction::McpToolCall { approval } = action else {
        panic!("test helper requires an MCP action");
    };
    let connection = rusqlite::Connection::open(fixture.root.join("storage.sqlite")).unwrap();
    let raw: String = connection
        .query_row(
            "SELECT agent_run_json FROM messages
             WHERE conversation_id = ?1 AND id = ?2",
            rusqlite::params![conversation_id, assistant_message_id],
            |row| row.get(0),
        )
        .unwrap();
    let mut run: serde_json::Value = serde_json::from_str(&raw).unwrap();
    run["mcpInvocations"] = serde_json::json!([{
        "actionId": approval.identity.action_id,
        "invocationId": approval.identity.invocation_id,
        "callId": approval.identity.call_id,
        "serverId": approval.identity.provenance.server_id,
        "serverDisplayName": approval.summary.server_display_name,
        "scope": approval.identity.provenance.scope,
        "rawToolName": approval.identity.provenance.raw_tool_name,
        "modelToolName": approval.identity.provenance.model_tool_name,
        "external": true,
        "state": "outcome_unknown",
        "dispatchCertainty": "possibly_dispatched",
        "outcome": "outcome_unknown",
        "errorCode": "mcp.tool_outcome_unknown",
        "durationMs": 1,
        "outputTruncated": false
    }]);
    run["timeline"] = serde_json::json!([{
        "id": format!("mcp-invocation-{}", approval.identity.invocation_id),
        "type": "mcp_tool_call",
        "invocationId": approval.identity.invocation_id
    }]);
    connection
        .execute(
            "UPDATE messages SET agent_run_json = ?3
             WHERE conversation_id = ?1 AND id = ?2",
            rusqlite::params![conversation_id, assistant_message_id, run.to_string()],
        )
        .unwrap();
}

#[test]
fn startup_reconciliation_finishes_committed_mcp_rejection_without_replay() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let run_id = "run-mcp-rejection-crash";
    let conversation_id = "conversation-mcp-rejection-crash";
    let assistant_message_id = "assistant-mcp-rejection-crash";
    let (pending, pending_audit, terminal_audit, expected_trace, _) =
        mcp_rejection_settlement(run_id, conversation_id, assistant_message_id);
    let AgentProposedAction::McpToolCall {
        approval: expected_approval,
    } = serde_json::from_str::<AgentProposedAction>(&terminal_audit.action_json).unwrap()
    else {
        panic!("fixture must retain its typed MCP approval identity");
    };
    let action_id = pending.action_id.clone();
    save_assistant_conversation(&service, conversation_id, assistant_message_id);
    service.store_pending_agent_action(pending).unwrap();
    service.upsert_agent_action_audit(pending_audit).unwrap();

    assert_eq!(
        service
            .commit_current_manual_settlement(
                &terminal_audit,
                "pending",
                "rejected",
                &expected_trace,
                12,
            )
            .unwrap(),
        AgentPendingActionResultCommitOutcome::Committed {
            trace_changed: true,
        }
    );

    // Reproduce the exact crash boundary: the rejection receipt is fully committed, but the
    // following pending-status CAS never ran before the process exited.
    let connection = service.state.connection().unwrap();
    let committed_state: (
        String,
        Option<String>,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
    ) = connection
        .query_row(
            "
            SELECT pending.status,
                   pending.target_status,
                   audit.status,
                   audit.decision,
                   audit.error,
                   audit.blocked_reason
            FROM agent_pending_actions pending
            JOIN agent_action_audit audit ON audit.action_id = pending.action_id
            WHERE pending.action_id = ?1
            ",
            [&action_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(committed_state.0, "pending");
    assert_eq!(committed_state.1.as_deref(), Some("rejected"));
    assert_eq!(committed_state.2, "rejected");
    assert_eq!(committed_state.3.as_deref(), Some("rejected"));
    assert_eq!(committed_state.4, None);
    assert_eq!(committed_state.5, None);
    drop(connection);
    assert_eq!(
        service
            .get_conversation_turn_trace(assistant_message_id)
            .unwrap()
            .as_ref(),
        Some(&expected_trace)
    );

    let reconciled = service
        .reconcile_interrupted_pending_agent_actions(42)
        .unwrap();
    assert_eq!(reconciled.len(), 1);
    assert_eq!(reconciled[0].action_id, action_id);
    assert_eq!(reconciled[0].status, "pending");
    assert_eq!(reconciled[0].target_status.as_deref(), Some("rejected"));

    let connection = service.state.connection().unwrap();
    let recovered_state: (
        String,
        Option<String>,
        String,
        Option<String>,
        Option<String>,
        String,
    ) = connection
        .query_row(
            "
            SELECT pending.status,
                   pending.target_status,
                   audit.status,
                   audit.decision,
                   audit.error,
                   audit.tool_result_json
            FROM agent_pending_actions pending
            JOIN agent_action_audit audit ON audit.action_id = pending.action_id
            WHERE pending.action_id = ?1
            ",
            [&action_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(recovered_state.0, "rejected");
    assert_eq!(recovered_state.1.as_deref(), Some("rejected"));
    assert_eq!(recovered_state.2, "rejected");
    assert_eq!(recovered_state.3.as_deref(), Some("rejected"));
    assert_eq!(recovered_state.4, None);
    assert!(!recovered_state.5.contains("outcome_unknown"));
    assert!(!recovered_state.5.contains("mcp.tool_outcome_unknown"));
    drop(connection);

    let recovered_trace = service
        .get_conversation_turn_trace(assistant_message_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        recovered_trace.terminal_status,
        crate::ConversationTurnTraceTerminalStatus::Failed
    );
    assert_eq!(recovered_trace.items, expected_trace.items);
    assert_eq!(
        recovered_trace
            .items
            .iter()
            .filter(|item| matches!(item, ConversationTurnTraceItem::ToolResult { .. }))
            .count(),
        1,
        "startup reconciliation must not append or replay the rejected MCP call"
    );
    let recovered_model_context = service
        .get_conversation_model_context_log(assistant_message_id)
        .unwrap()
        .unwrap();
    recovered_trace
        .validate_complete_model_context(&recovered_model_context.items)
        .unwrap();

    let conversation = service.load_conversation(conversation_id).unwrap().unwrap();
    let run: serde_json::Value = serde_json::from_str(
        conversation.messages[0]
            .agent_run_json
            .as_deref()
            .expect("backend-owned rejected MCP trace must produce a typed observer run"),
    )
    .unwrap();
    assert_eq!(
        run["mcpInvocations"][0]["actionId"],
        expected_approval.identity.action_id
    );
    assert_eq!(
        run["mcpInvocations"][0]["invocationId"],
        expected_approval.identity.invocation_id
    );
    assert_eq!(run["mcpInvocations"][0]["state"], "rejected");
    assert_eq!(run["mcpInvocations"][0]["outcome"], "rejected");
    assert_eq!(run["timeline"][0]["type"], "mcp_tool_call");
    assert_eq!(run["timeline"][0]["traceSequence"], 0);

    assert!(service
        .reconcile_interrupted_pending_agent_actions(43)
        .unwrap()
        .is_empty());
    assert_eq!(
        service
            .get_conversation_turn_trace(assistant_message_id)
            .unwrap(),
        Some(recovered_trace)
    );
}

#[test]
fn terminal_mcp_outcome_unknown_is_adopted_without_rewriting_the_completed_turn() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let run_id = "run-terminal-mcp-adoption";
    let conversation_id = "conversation-terminal-mcp-adoption";
    let assistant_message_id = "assistant-terminal-mcp-adoption";
    let (mut pending, mut audit, _, mut trace, mut call) =
        mcp_rejection_settlement(run_id, conversation_id, assistant_message_id);
    pending.status = "executing".to_string();
    audit.status = "approved".to_string();
    audit.decision = Some("approved".to_string());
    audit.decided_at = Some(pending.created_at);
    audit.decision_source = Some("manual".to_string());
    call.approval_status = crate::AgentApprovalStatus::Approved;

    let mut action: AgentProposedAction = serde_json::from_str(&pending.action_json).unwrap();
    let AgentProposedAction::McpToolCall { approval } = &mut action else {
        panic!("fixture must contain an MCP action");
    };
    approval.call.approval_status = crate::AgentApprovalStatus::Approved;
    approval.approval_mode = crate::AgentMcpApprovalMode::Auto;
    // The durable call stores only the privacy-safe `{}` operation. The opaque digest continues
    // to bind the original process-sealed arguments and therefore must not be recomputed here.
    approval.identity.arguments_digest = "e".repeat(64);
    pending.action_json = serde_json::to_string(&action).unwrap();
    audit.action_json = pending.action_json.clone();

    let ConversationTurnTraceItem::ToolCall {
        approval_status, ..
    } = &mut trace.items[0]
    else {
        panic!("fixture must begin with an MCP ToolCall");
    };
    *approval_status = crate::AgentApprovalStatus::Approved;
    let durable_result = crate::mcp_tool_result_persistence_projection(&AgentToolResult {
        exact_archive_file: None,
        call_id: call.id.clone(),
        tool: call.tool.clone(),
        ok: false,
        result: Some(serde_json::json!({
            "schemaVersion": 1,
            "type": "mcp_tool",
            "external": true,
            "status": "outcome_unknown",
            "outcome": "outcome_unknown",
            "dispatchCertainty": "possibly_dispatched",
            "isError": true,
            "code": "mcp.tool_outcome_unknown",
            "retryable": false
        })),
        error: Some("The external MCP tool reported an error.".to_string()),
    });
    trace.items[1] =
        crate::conversation_trace::projected_tool_result_trace_item(1, &call, &durable_result);
    trace.terminal_status = crate::ConversationTurnTraceTerminalStatus::Completed;
    trace.terminal_error = None;

    let mut model_context = current_model_context_for_trace(&trace);
    model_context[1].content = crate::conversation_trace::render_tool_observation(&durable_result);
    trace
        .validate_complete_model_context(&model_context)
        .unwrap();
    save_assistant_conversation(&service, conversation_id, assistant_message_id);
    service.store_pending_agent_action(pending.clone()).unwrap();
    service.upsert_agent_action_audit(audit).unwrap();
    let mut usage = agent_usage_record(conversation_id, assistant_message_id);
    usage.run_id = run_id.to_string();
    service
        .finalize_chat_message_with_conversation_trace_model_context_and_usage(
            conversation_id,
            assistant_message_id,
            "",
            Some("sent"),
            "completed",
            &trace,
            Some(&model_context),
            10,
            20,
            Some(&usage),
            None,
        )
        .unwrap();
    install_terminal_mcp_outcome_unknown_projection(
        &fixture,
        conversation_id,
        assistant_message_id,
        &action,
    );

    let mut tampered_action = action.clone();
    let AgentProposedAction::McpToolCall { approval } = &mut tampered_action else {
        panic!("fixture must contain an MCP action");
    };
    approval.call.args = serde_json::json!({ "unexpected": true });
    let tampered_action_json = serde_json::to_string(&tampered_action).unwrap();
    let connection = rusqlite::Connection::open(fixture.root.join("storage.sqlite")).unwrap();
    connection
        .execute(
            "UPDATE agent_pending_actions SET action_json = ?2 WHERE action_id = ?1",
            rusqlite::params![pending.action_id, tampered_action_json],
        )
        .unwrap();
    connection
        .execute(
            "UPDATE agent_action_audit SET action_json = ?2 WHERE action_id = ?1",
            rusqlite::params![pending.action_id, tampered_action_json],
        )
        .unwrap();
    assert!(service
        .adopt_terminal_mcp_agent_action_on_startup(&pending.action_id, "executing", 40)
        .is_err());
    connection
        .execute(
            "UPDATE agent_pending_actions SET action_json = ?2 WHERE action_id = ?1",
            rusqlite::params![pending.action_id, pending.action_json],
        )
        .unwrap();
    connection
        .execute(
            "UPDATE agent_action_audit SET action_json = ?2 WHERE action_id = ?1",
            rusqlite::params![pending.action_id, pending.action_json],
        )
        .unwrap();
    connection
        .execute(
            "UPDATE messages SET status = 'pending'
             WHERE conversation_id = ?1 AND id = ?2",
            rusqlite::params![conversation_id, assistant_message_id],
        )
        .unwrap();
    assert!(service
        .adopt_terminal_mcp_agent_action_on_startup(&pending.action_id, "executing", 41)
        .is_err());
    connection
        .execute(
            "UPDATE messages SET status = 'sent'
             WHERE conversation_id = ?1 AND id = ?2",
            rusqlite::params![conversation_id, assistant_message_id],
        )
        .unwrap();
    drop(connection);

    let trace_before = service
        .get_conversation_turn_trace(assistant_message_id)
        .unwrap()
        .unwrap();
    let context_before = service
        .get_conversation_model_context_log(assistant_message_id)
        .unwrap()
        .unwrap();
    assert!(service
        .adopt_terminal_mcp_agent_action_on_startup(&pending.action_id, "executing", 42)
        .unwrap());
    assert!(!service
        .adopt_terminal_mcp_agent_action_on_startup(&pending.action_id, "executing", 43)
        .unwrap());

    let adopted = service
        .get_pending_agent_action(&pending.action_id)
        .unwrap()
        .unwrap();
    assert_eq!(adopted.status, "failed");
    assert_eq!(adopted.target_status.as_deref(), Some("failed"));
    assert_eq!(adopted.action_json, "{}");
    assert_eq!(adopted.agent_input_json, "{}");
    let audit = service
        .get_agent_action_audit(&pending.action_id)
        .unwrap()
        .unwrap();
    assert_eq!(audit.status, "failed");
    assert_eq!(audit.error.as_deref(), Some("mcp.tool_outcome_unknown"));
    assert_eq!(audit.decision_source.as_deref(), Some("auto"));
    assert_eq!(audit.tool_result_json, None);
    assert_eq!(
        service
            .get_conversation_turn_trace(assistant_message_id)
            .unwrap(),
        Some(trace_before)
    );
    assert_eq!(
        service
            .get_conversation_model_context_log(assistant_message_id)
            .unwrap(),
        Some(context_before)
    );
    let conversation = service.load_conversation(conversation_id).unwrap().unwrap();
    let run: serde_json::Value = serde_json::from_str(
        conversation.messages[0]
            .agent_run_json
            .as_deref()
            .expect("terminal Assistant run must remain present"),
    )
    .unwrap();
    assert_eq!(run["status"], "completed");
}

#[test]
fn terminal_mcp_adoption_rejects_a_noncanonical_outcome_unknown_result() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let run_id = "run-invalid-terminal-mcp-adoption";
    let conversation_id = "conversation-invalid-terminal-mcp-adoption";
    let assistant_message_id = "assistant-invalid-terminal-mcp-adoption";
    let (mut pending, mut audit, _, mut trace, mut call) =
        mcp_rejection_settlement(run_id, conversation_id, assistant_message_id);
    pending.status = "executing".to_string();
    audit.status = "approved".to_string();
    audit.decision = Some("approved".to_string());
    audit.decided_at = Some(pending.created_at);
    audit.decision_source = Some("manual".to_string());
    call.approval_status = crate::AgentApprovalStatus::Approved;
    let mut action: AgentProposedAction = serde_json::from_str(&pending.action_json).unwrap();
    let AgentProposedAction::McpToolCall { approval } = &mut action else {
        panic!("fixture must contain an MCP action");
    };
    approval.call.approval_status = crate::AgentApprovalStatus::Approved;
    approval.approval_mode = crate::AgentMcpApprovalMode::Auto;
    approval.identity.arguments_digest = "e".repeat(64);
    pending.action_json = serde_json::to_string(&action).unwrap();
    audit.action_json = pending.action_json.clone();
    let ConversationTurnTraceItem::ToolCall {
        approval_status, ..
    } = &mut trace.items[0]
    else {
        panic!("fixture must begin with an MCP ToolCall");
    };
    *approval_status = crate::AgentApprovalStatus::Approved;
    let durable_result = crate::mcp_tool_result_persistence_projection(&AgentToolResult {
        exact_archive_file: None,
        call_id: call.id.clone(),
        tool: call.tool.clone(),
        ok: false,
        result: Some(serde_json::json!({
            "schemaVersion": 1,
            "type": "mcp_tool",
            "external": true,
            "status": "outcome_unknown",
            "outcome": "outcome_unknown",
            "dispatchCertainty": "definitely_not_dispatched",
            "isError": true,
            "code": "mcp.tool_outcome_unknown",
            "retryable": false
        })),
        error: Some("The external MCP tool reported an error.".to_string()),
    });
    trace.items[1] =
        crate::conversation_trace::projected_tool_result_trace_item(1, &call, &durable_result);
    trace.terminal_status = crate::ConversationTurnTraceTerminalStatus::Completed;
    trace.terminal_error = None;
    let mut model_context = current_model_context_for_trace(&trace);
    model_context[1].content = crate::conversation_trace::render_tool_observation(&durable_result);
    save_assistant_conversation(&service, conversation_id, assistant_message_id);
    service.store_pending_agent_action(pending.clone()).unwrap();
    service.upsert_agent_action_audit(audit).unwrap();
    let mut usage = agent_usage_record(conversation_id, assistant_message_id);
    usage.run_id = run_id.to_string();
    service
        .finalize_chat_message_with_conversation_trace_model_context_and_usage(
            conversation_id,
            assistant_message_id,
            "",
            Some("sent"),
            "completed",
            &trace,
            Some(&model_context),
            10,
            20,
            Some(&usage),
            None,
        )
        .unwrap();
    install_terminal_mcp_outcome_unknown_projection(
        &fixture,
        conversation_id,
        assistant_message_id,
        &action,
    );

    assert!(service
        .adopt_terminal_mcp_agent_action_on_startup(&pending.action_id, "executing", 42)
        .is_err());
    let unchanged = service
        .get_pending_agent_action(&pending.action_id)
        .unwrap()
        .unwrap();
    assert_eq!(unchanged.status, "executing");
    assert!(unchanged.target_status.is_none());
    assert_eq!(unchanged.action_json, pending.action_json);
}
