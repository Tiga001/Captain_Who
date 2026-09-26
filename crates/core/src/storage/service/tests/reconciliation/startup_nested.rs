use super::fixtures::*;
use super::*;

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

    let nested_trace = ConversationTurnTrace {
        schema_version: crate::CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
        run_id: "run-1".to_string(),
        conversation_id: "conversation-nested".to_string(),
        assistant_message_id: "assistant-nested".to_string(),
        terminal_status: crate::ConversationTurnTraceTerminalStatus::InProgress,
        terminal_error: None,
        truncated: false,
        items: vec![
            ConversationTurnTraceItem::ToolCall {
                sequence: 0,
                call_id: "parent-action".to_string(),
                tool: "run_command".to_string(),
                provenance: crate::AgentToolIdentity::Builtin {
                    tool_name: "run_command".to_string(),
                },
                operation: serde_json::json!({}),
                approval_status: crate::AgentApprovalStatus::Approved,
                truncated: false,
            },
            ConversationTurnTraceItem::ToolResult {
                sequence: 1,
                call_id: "parent-action".to_string(),
                tool: "run_command".to_string(),
                status: crate::ConversationTraceToolResultStatus::Succeeded,
                success: true,
                observation: serde_json::json!({ "status": "completed" }),
                approval_status: crate::AgentApprovalStatus::Approved,
                error: None,
                truncated: false,
                archive: Default::default(),
            },
            ConversationTurnTraceItem::ToolCall {
                sequence: 2,
                call_id: "child-call".to_string(),
                tool: "approval_tool".to_string(),
                provenance: crate::AgentToolIdentity::Builtin {
                    tool_name: "approval_tool".to_string(),
                },
                operation: serde_json::json!({}),
                approval_status: crate::AgentApprovalStatus::Required,
                truncated: false,
            },
        ],
    };
    service
        .append_in_progress_conversation_turn_trace_and_apply_guidances(
            &nested_trace,
            &current_model_context_for_trace(&nested_trace),
            1,
            1,
        )
        .unwrap();

    let parent_storage_id = current_pending_storage_id("run-1", "parent-action");
    let mut parent = current_pending_action("run-1", "parent-action", "conversation-nested");
    parent.assistant_message_id = Some("assistant-nested".to_string());
    parent.status = "executing".to_string();
    parent.target_status = Some("completed".to_string());
    service.store_pending_agent_action(parent).unwrap();

    let child_storage_id = current_pending_storage_id("run-1", "child-call");
    let mut child = current_pending_action("run-1", "child-call", "conversation-nested");
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
            "approvalStatus": "required",
            "reason": null
        }
    })
    .to_string();
    attach_current_manual_file_effect_checkpoint(&mut child, &nested_trace);
    service.store_pending_agent_action(child).unwrap();

    assert!(service
        .pending_agent_action_has_unsettled_predecessor(
            &child_storage_id,
            &["parent-action".to_string()],
        )
        .unwrap());

    service
        .reconcile_interrupted_pending_agent_actions(42)
        .unwrap();

    assert!(!service
        .pending_agent_action_has_unsettled_predecessor(
            &child_storage_id,
            &["parent-action".to_string()],
        )
        .unwrap());

    let pending = service.list_pending_agent_actions().unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].action_id, child_storage_id);
    let connection = service.state.connection().unwrap();
    let parent_status: String = connection
        .query_row(
            "SELECT status FROM agent_pending_actions WHERE action_id = ?1",
            [&parent_storage_id],
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
fn pending_sibling_without_a_durable_result_is_not_a_predecessor() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let conversation_id = "conversation-pending-siblings";
    let assistant_message_id = "assistant-pending-siblings";
    let mut owner = conversation(conversation_id, Some("project-1"), assistant_message_id);
    owner.messages[0].role = "assistant".to_string();
    owner.messages[0].status = Some("pending".to_string());
    service.save_conversation(owner).unwrap();

    let sibling_trace = ConversationTurnTrace {
        schema_version: crate::CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
        run_id: "run-1".to_string(),
        conversation_id: conversation_id.to_string(),
        assistant_message_id: assistant_message_id.to_string(),
        terminal_status: crate::ConversationTurnTraceTerminalStatus::InProgress,
        terminal_error: None,
        truncated: false,
        items: vec![ConversationTurnTraceItem::ToolCall {
            sequence: 0,
            call_id: "successor-call".to_string(),
            tool: "approval_tool".to_string(),
            provenance: crate::AgentToolIdentity::Builtin {
                tool_name: "approval_tool".to_string(),
            },
            operation: serde_json::json!({}),
            approval_status: crate::AgentApprovalStatus::Required,
            truncated: false,
        }],
    };
    service
        .append_in_progress_conversation_turn_trace_and_apply_guidances(
            &sibling_trace,
            &current_model_context_for_trace(&sibling_trace),
            1,
            1,
        )
        .unwrap();

    let mut sibling = current_pending_action("run-1", "sibling-call", conversation_id);
    sibling.assistant_message_id = Some(assistant_message_id.to_string());
    sibling.action_type = "tool_call".to_string();
    sibling.tool_name = "approval_tool".to_string();
    sibling.tool_call_id = Some("sibling-call".to_string());
    sibling.action_json = serde_json::json!({
        "type": "tool_call",
        "call": {
            "id": "sibling-call",
            "tool": "approval_tool",
            "args": {},
            "approvalStatus": "required",
            "reason": null
        }
    })
    .to_string();
    service.store_pending_agent_action(sibling).unwrap();

    let successor_storage_id = current_pending_storage_id("run-1", "successor-call");
    let mut successor = current_pending_action("run-1", "successor-call", conversation_id);
    successor.assistant_message_id = Some(assistant_message_id.to_string());
    successor.action_type = "tool_call".to_string();
    successor.tool_name = "approval_tool".to_string();
    successor.tool_call_id = Some("successor-call".to_string());
    successor.action_json = serde_json::json!({
        "type": "tool_call",
        "call": {
            "id": "successor-call",
            "tool": "approval_tool",
            "args": {},
            "approvalStatus": "required",
            "reason": null
        }
    })
    .to_string();
    service.store_pending_agent_action(successor).unwrap();

    assert!(!service
        .pending_agent_action_has_unsettled_predecessor(
            &successor_storage_id,
            &["sibling-call".to_string()],
        )
        .unwrap());
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

    let parent_call_id = "invalid-parent";
    let parent_storage_id = current_pending_storage_id("run-1", parent_call_id);
    let mut parent = current_pending_action("run-1", parent_call_id, "conversation-invalid-nested");
    parent.assistant_message_id = Some("assistant-invalid-nested".to_string());
    parent.status = "executing".to_string();
    let parent_trace = seed_current_in_progress_tool_trace(
        &service,
        "run-1",
        "conversation-invalid-nested",
        "assistant-invalid-nested",
        parent_call_id,
        "run_command",
    );
    attach_current_manual_file_effect_checkpoint(&mut parent, &parent_trace);
    service.store_pending_agent_action(parent).unwrap();

    let child_call_id = "invalid-child-call";
    let child_storage_id = current_pending_storage_id("run-1", child_call_id);
    let mut child = current_pending_action("run-1", child_call_id, "conversation-invalid-nested");
    child.assistant_message_id = Some("assistant-invalid-nested".to_string());
    child.action_type = "tool_call".to_string();
    child.tool_name = "approval_tool".to_string();
    child.action_json = serde_json::json!({
        "type": "tool_call",
        "call": {
            "id": child_call_id,
            "tool": "approval_tool",
            "args": {},
            "approvalStatus": "required",
            "reason": null
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
             WHERE action_id IN (?1, ?2)
             ORDER BY action_id",
        )
        .unwrap()
        .query_map(
            rusqlite::params![&parent_storage_id, &child_storage_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            },
        )
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(
        statuses,
        vec![
            (
                child_storage_id,
                "cancelled".to_string(),
                Some("cancelled".to_string())
            ),
            (
                parent_storage_id,
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
