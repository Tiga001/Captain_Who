use super::*;

pub(super) fn mcp_rejection_settlement(
    run_id: &str,
    conversation_id: &str,
    assistant_message_id: &str,
) -> (
    AgentPendingActionRecord,
    AgentActionAuditRecord,
    AgentActionAuditRecord,
    ConversationTurnTrace,
    AgentToolCall,
) {
    let action_id = "31522e9e-0f12-4d7c-9a7d-2c6e91c1f0d1";
    let call_id =
        crate::llm::model_response_tool_call_id(run_id, 0, 0, "fixture-mcp-rejection-call");
    let model_tool_name = "mcp__fixture__list_directory";
    let provenance = crate::AgentMcpToolProvenance {
        server_id: "7f4a2d91-24ab-4d24-9eed-63daf26a6c15".to_string(),
        scope: crate::AgentMcpServerScope::User,
        raw_tool_name: "list_directory".to_string(),
        model_tool_name: model_tool_name.to_string(),
        config_epoch: "66dcbb6b-92a3-4d4e-9591-f0707e4ca3e3".to_string(),
        registry_revision: 3,
        config_digest: "a".repeat(64),
        catalog_generation: 4,
        catalog_digest: "b".repeat(64),
        catalog_schema_digest: "c".repeat(64),
        schema_digest: "d".repeat(64),
        schema_normalizer_version: crate::MCP_INPUT_SCHEMA_NORMALIZER_VERSION,
    };
    let call = AgentToolCall {
        id: call_id.to_string(),
        tool: model_tool_name.to_string(),
        args: serde_json::json!({}),
        approval_status: crate::AgentApprovalStatus::Required,
        reason: None,
    };
    let approval = crate::AgentMcpToolApproval {
        identity: crate::AgentMcpToolInvocationIdentity {
            action_id: action_id.to_string(),
            invocation_id: "8e8272e7-a27b-4b82-a7bf-c90b98b2d76c".to_string(),
            run_id: run_id.to_string(),
            call_id: call_id.to_string(),
            provenance: provenance.clone(),
            arguments_digest: crate::mcp_tool_arguments_digest(&serde_json::json!({})).unwrap(),
        },
        call: call.clone(),
        summary: crate::AgentMcpToolApprovalSummary {
            server_id: provenance.server_id.clone(),
            server_display_name: "Fixture MCP".to_string(),
            scope: provenance.scope.clone(),
            raw_tool_name: provenance.raw_tool_name.clone(),
            model_tool_name: provenance.model_tool_name.clone(),
            display_reason: Some("List the allowed fixture directory.".to_string()),
            arguments: crate::AgentMcpArgumentSummary {
                encoded_bytes: 2,
                top_level_property_count: 0,
                string_value_count: 0,
                number_value_count: 0,
                boolean_value_count: 0,
                null_value_count: 0,
                object_value_count: 1,
                array_value_count: 0,
                max_depth: 0,
                truncated: false,
            },
            risk: crate::AgentMcpToolRisk::ReadOnlyClaimed,
            external: true,
        },
        approval_mode: crate::AgentMcpApprovalMode::Prompt,
        payload_persistence: crate::AgentMcpApprovalPayloadPersistence::ProcessOnly,
        created_at: 10,
        expires_at: 1_000,
    };
    let action = AgentProposedAction::McpToolCall {
        approval: Box::new(approval.clone()),
    };
    let action_json = serde_json::to_string(&action).unwrap();
    let storage_id = format!("v2:{}:{run_id}:{action_id}", run_id.len());
    let pending = AgentPendingActionRecord {
        action_id: storage_id.clone(),
        run_id: run_id.to_string(),
        conversation_id: Some(conversation_id.to_string()),
        assistant_message_id: Some(assistant_message_id.to_string()),
        action_type: "mcp_tool_call".to_string(),
        tool_name: model_tool_name.to_string(),
        tool_call_id: Some(call_id.to_string()),
        status: "pending".to_string(),
        target_status: None,
        action_json: action_json.clone(),
        agent_input_json: "{}".to_string(),
        created_at: 10,
        updated_at: 11,
    };
    let pending_audit = AgentActionAuditRecord {
        action_id: storage_id,
        run_id: run_id.to_string(),
        conversation_id: Some(conversation_id.to_string()),
        assistant_message_id: Some(assistant_message_id.to_string()),
        action_type: "mcp_tool_call".to_string(),
        tool_name: model_tool_name.to_string(),
        decision: None,
        status: "pending".to_string(),
        action_json,
        file_change_result_json: None,
        command_result_json: None,
        tool_result_json: None,
        error: None,
        created_at: 10,
        decided_at: None,
        completed_at: None,
        effective_permissions_json: Some("{}".to_string()),
        path_scope: None,
        command_cwd_scope: None,
        blocked_reason: None,
        decision_source: Some("manual_pending".to_string()),
    };
    let rejected_result = crate::mcp_tool_result_persistence_projection(
        &crate::mcp_tool_result_from_rejected_approval(&approval, None).unwrap(),
    );
    let mut terminal_audit = pending_audit.clone();
    terminal_audit.decision = Some("rejected".to_string());
    terminal_audit.status = "rejected".to_string();
    terminal_audit.tool_result_json = Some(serde_json::to_string(&rejected_result).unwrap());
    terminal_audit.decided_at = Some(12);
    terminal_audit.completed_at = Some(12);
    terminal_audit.decision_source = Some("manual".to_string());
    let trace = ConversationTurnTrace {
        schema_version: crate::CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
        run_id: run_id.to_string(),
        conversation_id: conversation_id.to_string(),
        assistant_message_id: assistant_message_id.to_string(),
        terminal_status: crate::ConversationTurnTraceTerminalStatus::InProgress,
        terminal_error: None,
        truncated: false,
        items: vec![
            ConversationTurnTraceItem::ToolCall {
                sequence: 0,
                call_id: call_id.to_string(),
                tool: model_tool_name.to_string(),
                provenance: crate::AgentToolIdentity::Mcp { provenance },
                operation: serde_json::json!({}),
                approval_status: crate::AgentApprovalStatus::Required,
                truncated: false,
            },
            crate::conversation_trace::projected_tool_result_trace_item(1, &call, &rejected_result),
        ],
    };
    (pending, pending_audit, terminal_audit, trace, call)
}
