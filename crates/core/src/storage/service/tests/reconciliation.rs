use super::*;

fn in_progress_result_trace(
    conversation_id: &str,
    assistant_message_id: &str,
) -> ConversationTurnTrace {
    ConversationTurnTrace {
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
                call_id: "action-atomic-result".to_string(),
                tool: "office_spreadsheet".to_string(),
                operation: serde_json::json!({ "operation": "set" }),
                approval_status: crate::AgentApprovalStatus::Approved,
                truncated: false,
            },
            ConversationTurnTraceItem::ToolResult {
                sequence: 1,
                call_id: "action-atomic-result".to_string(),
                tool: "office_spreadsheet".to_string(),
                status: crate::ConversationTraceToolResultStatus::Succeeded,
                success: true,
                observation: serde_json::json!({ "exitCode": 0 }),
                approval_status: crate::AgentApprovalStatus::Approved,
                error: None,
                truncated: false,
            },
        ],
    }
}

#[test]
fn pending_target_and_paired_trace_commit_atomically() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let mut stored_conversation = conversation(
        "conversation-atomic-result",
        Some("project-1"),
        "assistant-atomic-result",
    );
    stored_conversation.messages[0].role = "assistant".to_string();
    service.save_conversation(stored_conversation).unwrap();
    let mut pending = pending_action("action-atomic-result", "conversation-atomic-result");
    pending.assistant_message_id = Some("assistant-atomic-result".to_string());
    pending.status = "approved".to_string();
    service.store_pending_agent_action(pending).unwrap();

    let changed = service
        .commit_pending_agent_action_result_trace(
            "action-atomic-result",
            "approved",
            "completed",
            &in_progress_result_trace("conversation-atomic-result", "assistant-atomic-result"),
            1,
            2,
        )
        .unwrap();
    assert!(changed);

    let target_status: Option<String> = service
        .state
        .connection()
        .unwrap()
        .query_row(
            "SELECT target_status FROM agent_pending_actions WHERE action_id = ?1",
            ["action-atomic-result"],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(target_status.as_deref(), Some("completed"));
    let trace = service
        .get_conversation_turn_trace("assistant-atomic-result")
        .unwrap()
        .unwrap();
    assert!(matches!(
        trace.items.last(),
        Some(ConversationTurnTraceItem::ToolResult { call_id, .. })
            if call_id == "action-atomic-result"
    ));
}

#[test]
fn invalid_trace_rolls_back_pending_target_status() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let mut stored_conversation = conversation(
        "conversation-atomic-rollback",
        Some("project-1"),
        "assistant-atomic-rollback",
    );
    stored_conversation.messages[0].role = "assistant".to_string();
    service.save_conversation(stored_conversation).unwrap();
    let mut pending = pending_action("action-atomic-result", "conversation-atomic-rollback");
    pending.assistant_message_id = Some("assistant-atomic-rollback".to_string());
    pending.status = "approved".to_string();
    service.store_pending_agent_action(pending).unwrap();

    let error = service
        .commit_pending_agent_action_result_trace(
            "action-atomic-result",
            "approved",
            "completed",
            &in_progress_result_trace("missing-conversation", "assistant-atomic-rollback"),
            1,
            2,
        )
        .unwrap_err();
    assert!(error.contains("conversation trace assistant message does not exist"));

    let target_status: Option<String> = service
        .state
        .connection()
        .unwrap()
        .query_row(
            "SELECT target_status FROM agent_pending_actions WHERE action_id = ?1",
            ["action-atomic-result"],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(target_status, None);
    assert!(service
        .get_conversation_turn_trace("assistant-atomic-rollback")
        .unwrap()
        .is_none());
}

#[test]
fn startup_reconciliation_marks_interrupted_action_message_and_usage_failed() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let mut interrupted_conversation = conversation(
        "conversation-interrupted",
        Some("project-1"),
        "assistant-interrupted",
    );
    let message = &mut interrupted_conversation.messages[0];
    message.role = "assistant".to_string();
    message.status = Some("pending".to_string());
    message.agent_run_json = Some(
        serde_json::json!({
            "status": "waiting_for_approval",
            "state": {
                "status": "waiting_for_approval",
                "activeRunId": "run-1",
                "updatedAt": 1
            }
        })
        .to_string(),
    );
    service.save_conversation(interrupted_conversation).unwrap();
    let mut usage = agent_usage_record("conversation-interrupted", "assistant-interrupted");
    usage.status = Some("running".to_string());
    usage.completed_at = None;
    service.upsert_agent_usage(usage).unwrap();
    let mut pending = pending_action("action-interrupted", "conversation-interrupted");
    pending.assistant_message_id = Some("assistant-interrupted".to_string());
    pending.status = "approved".to_string();
    pending.agent_input_json = "sensitive continuation".to_string();
    service.store_pending_agent_action(pending).unwrap();

    let reconciled = service
        .reconcile_interrupted_pending_agent_actions(42)
        .unwrap();
    assert_eq!(reconciled.len(), 1);
    assert_eq!(reconciled[0].status, "approved");
    assert!(service.list_pending_agent_actions().unwrap().is_empty());

    let conversation = service
        .load_conversation("conversation-interrupted")
        .unwrap()
        .unwrap();
    let message = &conversation.messages[0];
    assert_eq!(message.status.as_deref(), Some("error"));
    let run: serde_json::Value =
        serde_json::from_str(message.agent_run_json.as_deref().unwrap()).unwrap();
    assert_eq!(run["status"], "failed");
    assert_eq!(run["state"]["status"], "failed");
    assert!(run["state"]["activeRunId"].is_null());

    let connection = service.state.connection().unwrap();
    let pending_row: (String, String) = connection
        .query_row(
            "SELECT status, agent_input_json FROM agent_pending_actions WHERE action_id = ?1",
            ["action-interrupted"],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(pending_row, ("failed".to_string(), "{}".to_string()));
    let usage_row: (String, Option<String>, Option<i64>) = connection
        .query_row(
            "SELECT status, error, completed_at FROM agent_usage_records WHERE run_id = ?1",
            ["run-1"],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(usage_row.0, "failed");
    assert!(usage_row.1.unwrap().contains("outcome is unknown"));
    assert_eq!(usage_row.2, Some(42));
}

#[test]
fn startup_reconciliation_preserves_a_durable_terminal_assistant_commit() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let mut conversation = conversation(
        "conversation-terminal",
        Some("project-1"),
        "assistant-terminal",
    );
    let message = &mut conversation.messages[0];
    message.role = "assistant".to_string();
    message.status = Some("sent".to_string());
    message.agent_run_json = Some(
        serde_json::json!({
            "status": "completed",
            "completedAt": 40,
            "state": { "status": "completed", "activeRunId": null, "updatedAt": 40 }
        })
        .to_string(),
    );
    service.save_conversation(conversation).unwrap();
    let mut usage = agent_usage_record("conversation-terminal", "assistant-terminal");
    usage.status = Some("running".to_string());
    usage.completed_at = None;
    service.upsert_agent_usage(usage).unwrap();
    let mut pending = pending_action("action-terminal", "conversation-terminal");
    pending.assistant_message_id = Some("assistant-terminal".to_string());
    pending.status = "approved".to_string();
    pending.target_status = Some("completed".to_string());
    service.store_pending_agent_action(pending).unwrap();

    service
        .reconcile_interrupted_pending_agent_actions(42)
        .unwrap();

    let connection = service.state.connection().unwrap();
    let pending_status: String = connection
        .query_row(
            "SELECT status FROM agent_pending_actions WHERE action_id = 'action-terminal'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(pending_status, "completed");
    let usage_status: (String, Option<i64>) = connection
        .query_row(
            "SELECT status, completed_at FROM agent_usage_records WHERE run_id = 'run-1'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(usage_status, ("completed".to_string(), Some(42)));
    drop(connection);
    let conversation = service
        .load_conversation("conversation-terminal")
        .unwrap()
        .unwrap();
    assert_eq!(conversation.messages[0].status.as_deref(), Some("sent"));
}

#[test]
fn startup_reconciliation_preserves_a_valid_nested_pending_checkpoint() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let mut conversation =
        conversation("conversation-nested", Some("project-1"), "assistant-nested");
    let message = &mut conversation.messages[0];
    message.role = "assistant".to_string();
    message.status = Some("pending".to_string());
    message.agent_run_json = Some(
        serde_json::json!({
            "runId": "run-1",
            "status": "running",
            "state": { "status": "running", "activeRunId": "run-1", "updatedAt": 1 }
        })
        .to_string(),
    );
    service.save_conversation(conversation).unwrap();
    let mut usage = agent_usage_record("conversation-nested", "assistant-nested");
    usage.status = Some("running".to_string());
    usage.completed_at = None;
    service.upsert_agent_usage(usage).unwrap();

    let mut parent = pending_action("parent-action", "conversation-nested");
    parent.assistant_message_id = Some("assistant-nested".to_string());
    parent.status = "executing".to_string();
    parent.target_status = Some("completed".to_string());
    service.store_pending_agent_action(parent).unwrap();

    let mut child = pending_action("child-storage-id", "conversation-nested");
    child.assistant_message_id = Some("assistant-nested".to_string());
    child.action_type = "tool_call".to_string();
    child.tool_name = "approval_tool".to_string();
    child.tool_call_id = Some("child-call".to_string());
    child.action_json = serde_json::json!({
        "type": "tool_call",
        "call": {
            "id": "child-call",
            "tool": "approval_tool",
            "args": {},
            "approvalStatus": "required"
        }
    })
    .to_string();
    child.agent_input_json = serde_json::json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "",
        "model": "test-model",
        "messages": [],
        "resumeCheckpoint": {
            "version": 2,
            "runId": "run-1",
            "contextItems": [],
            "nextModelRequestIndex": 1,
            "queuedToolCalls": [],
            "suppressedNarration": false,
            "extensionSnapshots": [],
            "pendingToolCallId": "child-call",
            "conversationTraceItems": [
                {
                    "type": "tool_result",
                    "sequence": 1,
                    "callId": "parent-action",
                    "tool": "run_command",
                    "status": "succeeded",
                    "success": true,
                    "observation": {},
                    "approvalStatus": "approved",
                    "truncated": false
                },
                {
                    "type": "tool_call",
                    "sequence": 2,
                    "callId": "child-call",
                    "tool": "approval_tool",
                    "operation": {},
                    "approvalStatus": "required",
                    "truncated": false
                }
            ],
            "nextConversationTraceSequence": 3,
            "conversationTraceTruncated": false,
            "modelVisibleTraceItemCount": 0
        }
    })
    .to_string();
    service.store_pending_agent_action(child).unwrap();

    service
        .reconcile_interrupted_pending_agent_actions(42)
        .unwrap();

    let pending = service.list_pending_agent_actions().unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].action_id, "child-storage-id");
    let connection = service.state.connection().unwrap();
    let parent_status: String = connection
        .query_row(
            "SELECT status FROM agent_pending_actions WHERE action_id = 'parent-action'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(parent_status, "completed");
    let usage_state: (String, Option<String>, Option<i64>) = connection
        .query_row(
            "SELECT status, error, completed_at FROM agent_usage_records WHERE run_id = 'run-1'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(
        usage_state,
        ("waiting_for_approval".to_string(), None, None)
    );
    drop(connection);
    let conversation = service
        .load_conversation("conversation-nested")
        .unwrap()
        .unwrap();
    let message = &conversation.messages[0];
    assert_eq!(message.status.as_deref(), Some("pending"));
    let run: serde_json::Value =
        serde_json::from_str(message.agent_run_json.as_deref().unwrap()).unwrap();
    assert_eq!(run["status"], "waiting_for_approval");
    assert_eq!(run["state"]["status"], "waiting_for_approval");
    assert_eq!(run["state"]["activeRunId"], "run-1");
}

#[test]
fn startup_reconciliation_retires_an_invalid_nested_pending_action() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let mut conversation = conversation(
        "conversation-invalid-nested",
        Some("project-1"),
        "assistant-invalid-nested",
    );
    let message = &mut conversation.messages[0];
    message.role = "assistant".to_string();
    message.status = Some("pending".to_string());
    message.agent_run_json = Some(
        serde_json::json!({
            "runId": "run-1",
            "status": "waiting_for_approval",
            "state": {
                "status": "waiting_for_approval",
                "activeRunId": "run-1",
                "updatedAt": 1
            }
        })
        .to_string(),
    );
    service.save_conversation(conversation).unwrap();
    let mut usage = agent_usage_record("conversation-invalid-nested", "assistant-invalid-nested");
    usage.status = Some("waiting_for_approval".to_string());
    usage.completed_at = None;
    service.upsert_agent_usage(usage).unwrap();

    let mut parent = pending_action("invalid-parent", "conversation-invalid-nested");
    parent.assistant_message_id = Some("assistant-invalid-nested".to_string());
    parent.status = "executing".to_string();
    service.store_pending_agent_action(parent).unwrap();

    let mut child = pending_action("invalid-child", "conversation-invalid-nested");
    child.assistant_message_id = Some("assistant-invalid-nested".to_string());
    child.action_type = "tool_call".to_string();
    child.tool_name = "approval_tool".to_string();
    child.tool_call_id = Some("invalid-child-call".to_string());
    child.action_json = serde_json::json!({
        "type": "tool_call",
        "call": {
            "id": "invalid-child-call",
            "tool": "approval_tool",
            "args": {},
            "approvalStatus": "required"
        }
    })
    .to_string();
    // A pending child without a run checkpoint is not a durable handoff and must never remain
    // approvable after its interrupted parent has been failed.
    child.agent_input_json = serde_json::json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "",
        "model": "test-model",
        "messages": []
    })
    .to_string();
    service.store_pending_agent_action(child).unwrap();

    service
        .reconcile_interrupted_pending_agent_actions(42)
        .unwrap();

    assert!(service.list_pending_agent_actions().unwrap().is_empty());
    let connection = service.state.connection().unwrap();
    let statuses = connection
        .prepare(
            "SELECT action_id, status, target_status
             FROM agent_pending_actions
             WHERE action_id IN ('invalid-parent', 'invalid-child')
             ORDER BY action_id",
        )
        .unwrap()
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(
        statuses,
        vec![
            (
                "invalid-child".to_string(),
                "cancelled".to_string(),
                Some("cancelled".to_string())
            ),
            (
                "invalid-parent".to_string(),
                "failed".to_string(),
                Some("failed".to_string())
            )
        ]
    );
    drop(connection);
    let conversation = service
        .load_conversation("conversation-invalid-nested")
        .unwrap()
        .unwrap();
    assert_eq!(conversation.messages[0].status.as_deref(), Some("error"));
}

#[test]
fn startup_reconciliation_keeps_failed_action_outcome_when_assistant_commit_completed() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let mut conversation = conversation(
        "conversation-failed-action",
        Some("project-1"),
        "assistant-failed-action",
    );
    let message = &mut conversation.messages[0];
    message.role = "assistant".to_string();
    message.status = Some("sent".to_string());
    message.agent_run_json = Some(
        serde_json::json!({
            "status": "completed",
            "completedAt": 40,
            "state": { "status": "completed", "activeRunId": null, "updatedAt": 40 }
        })
        .to_string(),
    );
    service.save_conversation(conversation).unwrap();
    let mut usage = agent_usage_record("conversation-failed-action", "assistant-failed-action");
    usage.status = Some("completed".to_string());
    usage.completed_at = Some(40);
    service.upsert_agent_usage(usage).unwrap();
    let mut pending = pending_action("action-failed", "conversation-failed-action");
    pending.assistant_message_id = Some("assistant-failed-action".to_string());
    pending.status = "approved".to_string();
    pending.target_status = Some("failed".to_string());
    service.store_pending_agent_action(pending).unwrap();

    service
        .reconcile_interrupted_pending_agent_actions(42)
        .unwrap();

    let connection = service.state.connection().unwrap();
    let pending_status: String = connection
        .query_row(
            "SELECT status FROM agent_pending_actions WHERE action_id = 'action-failed'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(pending_status, "failed");
    let usage_status: String = connection
        .query_row(
            "SELECT status FROM agent_usage_records WHERE run_id = 'run-1'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(usage_status, "completed");
    drop(connection);
    let conversation = service
        .load_conversation("conversation-failed-action")
        .unwrap()
        .unwrap();
    assert_eq!(conversation.messages[0].status.as_deref(), Some("sent"));
}
