use super::fixtures::*;
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
                provenance: crate::AgentToolIdentity::Builtin {
                    tool_name: "office_spreadsheet".to_string(),
                },
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
                archive: Default::default(),
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

fn pre_runtime_failure_fixture(
    service: &StorageService,
) -> (
    AgentPendingActionRecord,
    ConversationTurnTrace,
    Vec<crate::ConversationModelContextItem>,
) {
    let conversation_id = "conversation-pre-runtime-failure";
    let assistant_message_id = "assistant-pre-runtime-failure";
    let source_call_id = "action-pre-runtime-failure";
    let mut stored = conversation(conversation_id, Some("project-1"), assistant_message_id);
    let mut assistant = stored.messages.remove(0);
    assistant.role = "assistant".to_string();
    assistant.content = "still pending".to_string();
    assistant.status = Some("pending".to_string());
    assistant.agent_run_json = Some(
        serde_json::json!({
            "runId": "run-1",
            "status": "waiting_for_approval",
            "startedAt": 1,
            "state": {
                "status": "waiting_for_approval",
                "activeRunId": "run-1",
                "lastError": null,
                "updatedAt": 1
            }
        })
        .to_string(),
    );
    stored.messages = vec![
        ChatMessageRecord {
            human_interaction_response: None,
            id: "user-pre-runtime-failure".to_string(),
            role: "user".to_string(),
            content: "do work".to_string(),
            created_at: 0,
            status: Some("sent".to_string()),
            attachments: Vec::new(),
            folder_references_json: None,
            agent_run_json: None,
            ui_state_json: None,
        },
        assistant,
    ];
    service.save_conversation(stored).unwrap();
    let mut pending = current_pending_action("run-1", source_call_id, conversation_id);
    pending.assistant_message_id = Some(assistant_message_id.to_string());
    pending.status = "approved".to_string();
    pending.target_status = Some("completed".to_string());
    service.store_pending_agent_action(pending.clone()).unwrap();
    let in_progress = seed_current_in_progress_tool_trace(
        service,
        "run-1",
        conversation_id,
        assistant_message_id,
        source_call_id,
        "run_command",
    );
    let in_progress_model_context = current_model_context_for_trace(&in_progress);
    let terminal = crate::terminal_conversation_trace_from_snapshot(
        crate::ConversationTraceSnapshot {
            items: in_progress.items,
            model_context_items: in_progress_model_context,
            next_sequence: 1,
            truncated: false,
        },
        "run-1",
        conversation_id,
        assistant_message_id,
        crate::ConversationTurnTraceTerminalStatus::Failed,
        "pre-Runtime continuation failed",
    )
    .unwrap();
    (pending, terminal.trace, terminal.model_context_items)
}

#[test]
fn pre_runtime_continuation_failure_commits_all_terminal_facts_atomically() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let (pending, trace, model_context) = pre_runtime_failure_fixture(&service);
    let mut usage = agent_usage_record(
        pending.conversation_id.as_deref().unwrap(),
        pending.assistant_message_id.as_deref().unwrap(),
    );
    usage.status = Some("failed".to_string());
    usage.error = Some("pre-Runtime continuation failed".to_string());
    service
        .enqueue_notification_event(
            &crate::storage::notification_repository::NewNotificationEventRecord {
                notification_kind: "approval_required".to_string(),
                source_kind: "human_root".to_string(),
                source_id: "run-1".to_string(),
                run_id: Some("run-1".to_string()),
                automation_id: None,
                conversation_id: pending.conversation_id.clone(),
                user_message_id: Some("user-pre-runtime-failure".to_string()),
                assistant_message_id: pending.assistant_message_id.clone(),
                approval_action_id: pending.tool_call_id.clone(),
                subject_kind: "prompt_excerpt".to_string(),
                subject_text: "do work".to_string(),
                dedupe_key: "approval-pre-runtime-failure".to_string(),
                supersession_key: "human-root:run-1".to_string(),
                resource_revision: Some(1),
                occurred_at: 1,
                expires_at: 10_000,
            },
        )
        .unwrap();

    service
        .fail_claimed_agent_action_continuation(
            &pending.action_id,
            "approved",
            "completed",
            pending.conversation_id.as_deref().unwrap(),
            pending.assistant_message_id.as_deref().unwrap(),
            "pre-Runtime continuation failed",
            &trace,
            &model_context,
            10,
            Some(&usage),
        )
        .unwrap();

    let connection = service.state.connection().unwrap();
    let (status, target_status, action_json, agent_input_json): (
        String,
        Option<String>,
        String,
        String,
    ) = connection
        .query_row(
            "SELECT status, target_status, action_json, agent_input_json
             FROM agent_pending_actions WHERE action_id = ?1",
            [&pending.action_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(status, "failed");
    assert_eq!(target_status.as_deref(), Some("failed"));
    assert_eq!(action_json, "{}");
    assert_eq!(agent_input_json, "{}");
    drop(connection);
    let conversation = service
        .load_conversation(pending.conversation_id.as_deref().unwrap())
        .unwrap()
        .unwrap();
    let assistant = conversation
        .messages
        .iter()
        .find(|message| message.id == "assistant-pre-runtime-failure")
        .unwrap();
    assert_eq!(assistant.status.as_deref(), Some("error"));
    assert_eq!(assistant.content, "pre-Runtime continuation failed");
    assert_eq!(
        service
            .get_conversation_turn_trace(pending.assistant_message_id.as_deref().unwrap())
            .unwrap()
            .unwrap()
            .terminal_status,
        crate::ConversationTurnTraceTerminalStatus::Failed
    );
    let usage = service
        .load_agent_usage_for_owner(
            "run-1",
            pending.conversation_id.as_deref().unwrap(),
            pending.assistant_message_id.as_deref().unwrap(),
        )
        .unwrap()
        .unwrap();
    assert_eq!(usage.status.as_deref(), Some("failed"));
    let notifications = service.list_notifications(None, 10, false, None).unwrap();
    assert_eq!(notifications.items.len(), 1);
    assert_eq!(notifications.items[0].notification_kind, "task_failed");
    assert_eq!(notifications.items[0].subject_text, "do work");
    let approval_resolved: (Option<i64>, Option<i64>) = service
        .state
        .connection()
        .unwrap()
        .query_row(
            "SELECT resolved_at, superseded_at FROM notification_events
             WHERE dedupe_key = 'approval-pre-runtime-failure'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert!(approval_resolved.0.is_some() && approval_resolved.1.is_some());
}

#[test]
fn pre_runtime_continuation_failure_cas_conflict_changes_nothing() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let (pending, trace, model_context) = pre_runtime_failure_fixture(&service);

    let error = service
        .fail_claimed_agent_action_continuation(
            &pending.action_id,
            "approved",
            "rejected",
            pending.conversation_id.as_deref().unwrap(),
            pending.assistant_message_id.as_deref().unwrap(),
            "pre-Runtime continuation failed",
            &trace,
            &model_context,
            10,
            None,
        )
        .unwrap_err();
    assert!(error.contains("lost its pending-action CAS"));
    let connection = service.state.connection().unwrap();
    let (status, target_status): (String, Option<String>) = connection
        .query_row(
            "SELECT status, target_status FROM agent_pending_actions WHERE action_id = ?1",
            [&pending.action_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(status, "approved");
    assert_eq!(target_status.as_deref(), Some("completed"));
    drop(connection);
    let conversation = service
        .load_conversation(pending.conversation_id.as_deref().unwrap())
        .unwrap()
        .unwrap();
    let assistant = conversation
        .messages
        .iter()
        .find(|message| message.id == "assistant-pre-runtime-failure")
        .unwrap();
    assert_eq!(assistant.status.as_deref(), Some("pending"));
    assert_eq!(assistant.content, "still pending");
    assert_eq!(
        service
            .get_conversation_turn_trace(pending.assistant_message_id.as_deref().unwrap())
            .unwrap()
            .unwrap()
            .terminal_status,
        crate::ConversationTurnTraceTerminalStatus::InProgress
    );
}

#[test]
fn pre_runtime_continuation_failure_rolls_back_when_usage_write_fails() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let (pending, trace, model_context) = pre_runtime_failure_fixture(&service);
    let mut usage = agent_usage_record(
        pending.conversation_id.as_deref().unwrap(),
        pending.assistant_message_id.as_deref().unwrap(),
    );
    usage.status = Some("failed".to_string());
    usage.error = Some("pre-Runtime continuation failed".to_string());
    let mut conflicting_conversation = conversation(
        "conversation-pre-runtime-usage-conflict",
        Some("project-1"),
        "assistant-pre-runtime-usage-conflict",
    );
    conflicting_conversation.messages[0].role = "assistant".to_string();
    service.save_conversation(conflicting_conversation).unwrap();
    let mut conflicting_usage = agent_usage_record(
        "conversation-pre-runtime-usage-conflict",
        "assistant-pre-runtime-usage-conflict",
    );
    // The owner-specific upsert cannot consume a primary key already owned by another message.
    // This fails after pending/message/trace writes have run in the transaction and therefore
    // proves those earlier statements are rolled back too.
    conflicting_usage.id = usage.id.clone();
    service.upsert_agent_usage(conflicting_usage).unwrap();

    let error = service
        .fail_claimed_agent_action_continuation(
            &pending.action_id,
            "approved",
            "completed",
            pending.conversation_id.as_deref().unwrap(),
            pending.assistant_message_id.as_deref().unwrap(),
            "pre-Runtime continuation failed",
            &trace,
            &model_context,
            10,
            Some(&usage),
        )
        .unwrap_err();
    assert!(error.contains("UNIQUE constraint failed: agent_usage_records.id"));
    let connection = service.state.connection().unwrap();
    let (status, target_status): (String, Option<String>) = connection
        .query_row(
            "SELECT status, target_status FROM agent_pending_actions WHERE action_id = ?1",
            [&pending.action_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(status, "approved");
    assert_eq!(target_status.as_deref(), Some("completed"));
    drop(connection);
    let conversation = service
        .load_conversation(pending.conversation_id.as_deref().unwrap())
        .unwrap()
        .unwrap();
    let assistant = conversation
        .messages
        .iter()
        .find(|message| message.id == "assistant-pre-runtime-failure")
        .unwrap();
    assert_eq!(assistant.status.as_deref(), Some("pending"));
    assert_eq!(
        service
            .get_conversation_turn_trace(pending.assistant_message_id.as_deref().unwrap())
            .unwrap()
            .unwrap()
            .terminal_status,
        crate::ConversationTurnTraceTerminalStatus::InProgress
    );
}
