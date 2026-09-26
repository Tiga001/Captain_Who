use super::fixtures::*;
use super::*;

fn seed_current_terminal_assistant_trace(
    service: &StorageService,
    run_id: &str,
    conversation_id: &str,
    assistant_message_id: &str,
    status: crate::ConversationTurnTraceTerminalStatus,
) {
    let trace = ConversationTurnTrace {
        schema_version: crate::CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
        run_id: run_id.to_string(),
        conversation_id: conversation_id.to_string(),
        assistant_message_id: assistant_message_id.to_string(),
        terminal_status: status,
        terminal_error: (status == crate::ConversationTurnTraceTerminalStatus::Failed)
            .then(|| "The run ended with a safe test failure.".to_string()),
        truncated: false,
        items: vec![ConversationTurnTraceItem::AssistantNarration {
            provider_turn_id: None,
            first_tool_call_id: None,
            sequence: 0,
            content: "current assistant output".to_string(),
            truncated: false,
        }],
    };
    service
        .finalize_chat_message_with_conversation_trace_model_context_and_usage(
            conversation_id,
            assistant_message_id,
            "current assistant output",
            Some(
                if status == crate::ConversationTurnTraceTerminalStatus::Completed {
                    "sent"
                } else {
                    "error"
                },
            ),
            if status == crate::ConversationTurnTraceTerminalStatus::Completed {
                "completed"
            } else {
                "failed"
            },
            &trace,
            Some(&current_model_context_for_trace(&trace)),
            1,
            40,
            None,
            None,
        )
        .unwrap();
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
    let trace = seed_current_in_progress_tool_trace(
        &service,
        "run-1",
        "conversation-interrupted",
        "assistant-interrupted",
        "action-interrupted",
        "run_command",
    );
    let mut usage = agent_usage_record("conversation-interrupted", "assistant-interrupted");
    usage.status = Some("running".to_string());
    usage.completed_at = None;
    service.upsert_agent_usage(usage).unwrap();
    let source_call_id = "action-interrupted";
    let storage_id = current_pending_storage_id("run-1", source_call_id);
    let mut pending = current_pending_action("run-1", source_call_id, "conversation-interrupted");
    pending.assistant_message_id = Some("assistant-interrupted".to_string());
    pending.status = "approved".to_string();
    attach_current_manual_file_effect_checkpoint(&mut pending, &trace);
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
            [&storage_id],
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
    seed_current_terminal_assistant_trace(
        &service,
        "run-1",
        "conversation-terminal",
        "assistant-terminal",
        crate::ConversationTurnTraceTerminalStatus::Completed,
    );
    let mut usage = agent_usage_record("conversation-terminal", "assistant-terminal");
    usage.status = Some("running".to_string());
    usage.completed_at = None;
    service.upsert_agent_usage(usage).unwrap();
    let source_call_id = "action-terminal";
    let storage_id = current_pending_storage_id("run-1", source_call_id);
    let mut pending = current_pending_action("run-1", source_call_id, "conversation-terminal");
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
            "SELECT status FROM agent_pending_actions WHERE action_id = ?1",
            [&storage_id],
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
    seed_current_terminal_assistant_trace(
        &service,
        "run-1",
        "conversation-failed-action",
        "assistant-failed-action",
        crate::ConversationTurnTraceTerminalStatus::Completed,
    );
    let mut usage = agent_usage_record("conversation-failed-action", "assistant-failed-action");
    usage.status = Some("completed".to_string());
    usage.completed_at = Some(40);
    service.upsert_agent_usage(usage).unwrap();
    let source_call_id = "action-failed";
    let storage_id = current_pending_storage_id("run-1", source_call_id);
    let mut pending = current_pending_action("run-1", source_call_id, "conversation-failed-action");
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
            "SELECT status FROM agent_pending_actions WHERE action_id = ?1",
            [&storage_id],
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
