use super::fixtures::*;
use super::*;

pub(super) fn manual_command_settlement(
    call_id: &str,
    conversation_id: &str,
    assistant_message_id: &str,
) -> (
    AgentPendingActionRecord,
    AgentActionAuditRecord,
    AgentActionAuditRecord,
    ConversationTurnTrace,
) {
    let storage_id = current_pending_storage_id("run-1", call_id);
    let action = AgentProposedAction::Command {
        command: crate::AgentCommandRequest {
            id: call_id.to_string(),
            command: "node script.mjs".to_string(),
            cwd: None,
            timeout_ms: None,
            approval_status: crate::AgentApprovalStatus::Required,
            risk_level: None,
            reason: Some("test atomic settlement".to_string()),
            observe: None,
            inputs: Vec::new(),
            runtime_binding: None,
            managed_office_script: None,
        },
    };
    let action_json = serde_json::to_string(&action).unwrap();
    let pending = AgentPendingActionRecord {
        action_id: storage_id.clone(),
        run_id: "run-1".to_string(),
        conversation_id: Some(conversation_id.to_string()),
        assistant_message_id: Some(assistant_message_id.to_string()),
        action_type: "command".to_string(),
        tool_name: "run_command".to_string(),
        tool_call_id: Some(call_id.to_string()),
        status: "approved".to_string(),
        target_status: None,
        action_json: action_json.clone(),
        agent_input_json: "{}".to_string(),
        created_at: 10,
        updated_at: 11,
    };
    let approved_audit = AgentActionAuditRecord {
        action_id: storage_id,
        run_id: "run-1".to_string(),
        conversation_id: Some(conversation_id.to_string()),
        assistant_message_id: Some(assistant_message_id.to_string()),
        action_type: "command".to_string(),
        tool_name: "run_command".to_string(),
        decision: Some("approved".to_string()),
        status: "approved".to_string(),
        action_json,
        file_change_result_json: None,
        command_result_json: None,
        tool_result_json: None,
        error: None,
        created_at: 10,
        decided_at: Some(11),
        completed_at: None,
        effective_permissions_json: Some("{}".to_string()),
        path_scope: Some("workspace".to_string()),
        command_cwd_scope: Some("workspace".to_string()),
        blocked_reason: None,
        decision_source: Some("manual".to_string()),
    };
    let command_result = AgentCommandExecutionResult {
        outputs: Vec::new(),
        command: "node script.mjs".to_string(),
        cwd: "/workspace".to_string(),
        exit_code: Some(0),
        stdout: "created workbook".to_string(),
        stderr: String::new(),
        timed_out: false,
        cancelled: false,
        duration_ms: 12,
        stdout_truncated: false,
        stderr_truncated: false,
        output_capture: Default::default(),
        stdout_spool: Default::default(),
        stderr_spool: Default::default(),
        error: None,
        policy_evaluation: None,
        artifact_observation: None,
        input_files: Vec::new(),
        runtime: None,
        managed_outputs: None,
        authoritative_archive_ref: None,
        history_open: None,
    };
    let tool_result = crate::command::command_tool_result(call_id, &command_result);
    let mut terminal_audit = approved_audit.clone();
    terminal_audit.status = "completed".to_string();
    terminal_audit.command_result_json = Some(serde_json::to_string(&command_result).unwrap());
    terminal_audit.tool_result_json = Some(serde_json::to_string(&tool_result).unwrap());
    terminal_audit.completed_at = Some(12);
    let trace_operation = serde_json::json!({
        "command": "node script.mjs",
        "reason": "test atomic settlement",
    });
    let trace_call = AgentToolCall {
        id: call_id.to_string(),
        tool: "run_command".to_string(),
        args: trace_operation.clone(),
        approval_status: crate::AgentApprovalStatus::Approved,
        reason: Some("test atomic settlement".to_string()),
    };
    let trace = ConversationTurnTrace {
        schema_version: crate::CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
        run_id: "run-1".to_string(),
        conversation_id: conversation_id.to_string(),
        assistant_message_id: assistant_message_id.to_string(),
        terminal_status: crate::ConversationTurnTraceTerminalStatus::InProgress,
        terminal_error: None,
        truncated: false,
        items: vec![
            ConversationTurnTraceItem::ToolCall {
                sequence: 0,
                call_id: call_id.to_string(),
                tool: "run_command".to_string(),
                provenance: crate::AgentToolIdentity::Builtin {
                    tool_name: "run_command".to_string(),
                },
                operation: trace_operation,
                approval_status: crate::AgentApprovalStatus::Approved,
                truncated: false,
            },
            crate::conversation_trace::projected_tool_result_trace_item(
                1,
                &trace_call,
                &tool_result,
            ),
        ],
    };
    (pending, approved_audit, terminal_audit, trace)
}
