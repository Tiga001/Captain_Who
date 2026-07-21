use super::*;

fn assistant_run_message(
    id: &str,
    run_id: &str,
    run_status: &str,
    message_status: &str,
    position_time: i64,
) -> ChatMessageRecord {
    let terminal = matches!(run_status, "completed" | "failed" | "cancelled");
    let presentation_status = if run_status == "cancelled" {
        // This is the real legacy failure shape: the top-level lifecycle was saved, but the
        // renderer-owned nested state remained stale.
        "running"
    } else {
        run_status
    };
    let mut run = serde_json::json!({
        "runId": run_id,
        "assistantMessageId": id,
        "status": run_status,
        "timeline": [{ "kind": "presentation-only" }],
        "state": {
            "status": presentation_status,
            "activeRunId": if terminal {
                serde_json::Value::Null
            } else {
                serde_json::Value::String(run_id.to_string())
            },
            "updatedAt": position_time
        }
    });
    if terminal {
        run["completedAt"] = serde_json::json!(22);
    }
    ChatMessageRecord {
        id: id.to_string(),
        role: "assistant".to_string(),
        content: "partial response".to_string(),
        created_at: position_time,
        status: Some(message_status.to_string()),
        attachments: Vec::new(),
        agent_run_json: Some(run.to_string()),
        ui_state_json: None,
    }
}

fn save_run_conversation(
    service: &StorageService,
    conversation_id: &str,
    assistant_message_id: &str,
    run_id: &str,
    run_status: &str,
    message_status: &str,
) {
    service
        .save_conversation(ChatConversationRecord {
            id: conversation_id.to_string(),
            project_id: None,
            model_id: Some("model-1".to_string()),
            title: "orphan trace".to_string(),
            messages: vec![
                ChatMessageRecord {
                    id: format!("user-{assistant_message_id}"),
                    role: "user".to_string(),
                    content: "do work".to_string(),
                    created_at: 1,
                    status: Some("sent".to_string()),
                    attachments: Vec::new(),
                    agent_run_json: None,
                    ui_state_json: None,
                },
                assistant_run_message(assistant_message_id, run_id, run_status, message_status, 2),
            ],
            created_at: 1,
            updated_at: 2,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
}

fn in_progress_trace(
    conversation_id: &str,
    assistant_message_id: &str,
    run_id: &str,
) -> ConversationTurnTrace {
    ConversationTurnTrace {
        schema_version: crate::CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
        run_id: run_id.to_string(),
        conversation_id: conversation_id.to_string(),
        assistant_message_id: assistant_message_id.to_string(),
        terminal_status: crate::ConversationTurnTraceTerminalStatus::InProgress,
        terminal_error: None,
        truncated: false,
        items: vec![
            ConversationTurnTraceItem::AssistantNarration {
                sequence: 0,
                content: "I will inspect the file.".to_string(),
                truncated: false,
            },
            ConversationTurnTraceItem::ToolCall {
                sequence: 1,
                call_id: format!("call-{run_id}"),
                tool: "read_file".to_string(),
                operation: serde_json::json!({ "path": "notes.txt" }),
                approval_status: crate::AgentApprovalStatus::NotRequired,
                truncated: false,
            },
            ConversationTurnTraceItem::ToolResult {
                sequence: 2,
                call_id: format!("call-{run_id}"),
                tool: "read_file".to_string(),
                status: crate::ConversationTraceToolResultStatus::Succeeded,
                success: true,
                observation: serde_json::json!({ "path": "notes.txt", "lines": 3 }),
                approval_status: crate::AgentApprovalStatus::NotRequired,
                error: None,
                truncated: false,
            },
        ],
    }
}

fn store_in_progress_trace(
    service: &StorageService,
    conversation_id: &str,
    assistant_message_id: &str,
    run_id: &str,
) -> ConversationTurnTrace {
    let trace = in_progress_trace(conversation_id, assistant_message_id, run_id);
    assert!(service
        .append_in_progress_conversation_turn_trace(&trace, 10, 20)
        .unwrap());
    trace
}

#[test]
fn startup_trace_reconciliation_retires_cancelled_orphan_and_unblocks_fork() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let conversation_id = "conversation-cancelled-orphan";
    let assistant_message_id = "assistant-cancelled-orphan";
    let run_id = "run-cancelled-orphan";
    save_run_conversation(
        &service,
        conversation_id,
        assistant_message_id,
        run_id,
        "cancelled",
        "sent",
    );
    let original_trace =
        store_in_progress_trace(&service, conversation_id, assistant_message_id, run_id);
    let mut usage = agent_usage_record(conversation_id, assistant_message_id);
    usage.run_id = run_id.to_string();
    usage.status = Some("completed".to_string());
    usage.error = None;
    usage.completed_at = Some(25);
    service.upsert_agent_usage(usage).unwrap();

    let blocked = service.fork_conversation(ForkConversationInput {
        request_id: "fork-before-reconciliation".to_string(),
        source_conversation_id: conversation_id.to_string(),
        through_assistant_message_id: assistant_message_id.to_string(),
    });
    assert!(blocked.unwrap_err().contains("这条回复仍在生成"));

    assert_eq!(
        service
            .reconcile_orphaned_in_progress_conversation_turn_traces(&HashSet::new(), 100)
            .unwrap(),
        1
    );
    let trace = service
        .get_conversation_turn_trace(assistant_message_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        trace.terminal_status,
        crate::ConversationTurnTraceTerminalStatus::Cancelled
    );
    assert_eq!(trace.items, original_trace.items);
    assert!(trace
        .terminal_error
        .as_deref()
        .is_some_and(|error| error.contains("cancelled before")));

    let stored = service.load_conversation(conversation_id).unwrap().unwrap();
    let message = &stored.messages[1];
    assert_eq!(message.status.as_deref(), Some("sent"));
    let run: serde_json::Value =
        serde_json::from_str(message.agent_run_json.as_deref().unwrap()).unwrap();
    assert_eq!(run["runId"], run_id);
    assert_eq!(run["status"], "cancelled");
    assert_eq!(run["state"]["status"], "cancelled");
    assert!(run["state"]["activeRunId"].is_null());
    assert_eq!(run["timeline"][0]["kind"], "presentation-only");

    let usage_state: (String, Option<String>, Option<i64>) = service
        .state
        .connection()
        .unwrap()
        .query_row(
            "SELECT status, error, completed_at FROM agent_usage_records WHERE run_id = ?1",
            [run_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(usage_state.0, "cancelled");
    assert!(usage_state.1.unwrap().contains("cancelled before"));
    assert_eq!(usage_state.2, Some(22));
    let lifecycle_times: (i64, Option<i64>) = service
        .state
        .connection()
        .unwrap()
        .query_row(
            "SELECT conversation.updated_at, trace.completed_at
             FROM conversations AS conversation
             INNER JOIN conversation_turn_traces AS trace
                 ON trace.conversation_id = conversation.id
             WHERE conversation.id = ?1",
            [conversation_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(lifecycle_times, (2, Some(22)));

    let forked = service
        .fork_conversation(ForkConversationInput {
            request_id: "fork-after-reconciliation".to_string(),
            source_conversation_id: conversation_id.to_string(),
            through_assistant_message_id: assistant_message_id.to_string(),
        })
        .unwrap();
    let cloned_assistant = &forked.messages[1];
    let cloned_trace = service
        .get_conversation_turn_trace(&cloned_assistant.id)
        .unwrap()
        .unwrap();
    assert_eq!(
        cloned_trace.terminal_status,
        crate::ConversationTurnTraceTerminalStatus::Cancelled
    );
    assert_eq!(cloned_trace.items, original_trace.items);

    assert_eq!(
        service
            .reconcile_orphaned_in_progress_conversation_turn_traces(&HashSet::new(), 200)
            .unwrap(),
        0,
        "startup reconciliation must be idempotent"
    );
}

#[test]
fn completed_looking_renderer_state_is_conservatively_reconciled_as_interrupted_failure() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let conversation_id = "conversation-completed-looking";
    let assistant_message_id = "assistant-completed-looking";
    let run_id = "run-completed-looking";
    save_run_conversation(
        &service,
        conversation_id,
        assistant_message_id,
        run_id,
        "completed",
        "sent",
    );
    store_in_progress_trace(&service, conversation_id, assistant_message_id, run_id);

    assert_eq!(
        service
            .reconcile_orphaned_in_progress_conversation_turn_traces(&HashSet::new(), 100)
            .unwrap(),
        1
    );
    let trace = service
        .get_conversation_turn_trace(assistant_message_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        trace.terminal_status,
        crate::ConversationTurnTraceTerminalStatus::Failed
    );
    assert!(trace
        .terminal_error
        .as_deref()
        .is_some_and(|error| error.contains("application exited")));
    let stored = service.load_conversation(conversation_id).unwrap().unwrap();
    let message = &stored.messages[1];
    assert_eq!(message.status.as_deref(), Some("error"));
    let run: serde_json::Value =
        serde_json::from_str(message.agent_run_json.as_deref().unwrap()).unwrap();
    assert_eq!(run["status"], "failed");
    assert_eq!(run["state"]["status"], "failed");
}

#[test]
fn startup_trace_reconciliation_preserves_pending_approval_and_current_active_run() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    for (conversation_id, assistant_message_id, run_id) in [
        ("conversation-pending", "assistant-pending", "run-pending"),
        ("conversation-active", "assistant-active", "run-active"),
    ] {
        save_run_conversation(
            &service,
            conversation_id,
            assistant_message_id,
            run_id,
            "running",
            "pending",
        );
        store_in_progress_trace(&service, conversation_id, assistant_message_id, run_id);
    }
    service
        .store_pending_agent_action(AgentPendingActionRecord {
            action_id: "action-pending-trace".to_string(),
            run_id: "run-pending".to_string(),
            conversation_id: Some("conversation-pending".to_string()),
            assistant_message_id: Some("assistant-pending".to_string()),
            action_type: "tool_call".to_string(),
            tool_name: "approval_tool".to_string(),
            tool_call_id: Some("pending-call".to_string()),
            status: "pending".to_string(),
            target_status: None,
            action_json: "{}".to_string(),
            agent_input_json: "{}".to_string(),
            created_at: 5,
            updated_at: 5,
        })
        .unwrap();
    let active = HashSet::from(["run-active".to_string()]);

    assert_eq!(
        service
            .reconcile_orphaned_in_progress_conversation_turn_traces(&active, 100)
            .unwrap(),
        0
    );
    for assistant_message_id in ["assistant-pending", "assistant-active"] {
        assert_eq!(
            service
                .get_conversation_turn_trace(assistant_message_id)
                .unwrap()
                .unwrap()
                .terminal_status,
            crate::ConversationTurnTraceTerminalStatus::InProgress
        );
    }
}

#[test]
fn startup_trace_reconciliation_rolls_back_every_candidate_on_commit_failure() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    for suffix in ["a", "b"] {
        let conversation_id = format!("conversation-atomic-{suffix}");
        let assistant_message_id = format!("assistant-atomic-{suffix}");
        let run_id = format!("run-atomic-{suffix}");
        save_run_conversation(
            &service,
            &conversation_id,
            &assistant_message_id,
            &run_id,
            "running",
            "pending",
        );
        store_in_progress_trace(&service, &conversation_id, &assistant_message_id, &run_id);
    }
    service
        .state
        .connection()
        .unwrap()
        .execute_batch(
            "CREATE TRIGGER reject_second_orphan_reconciliation
             BEFORE UPDATE OF status ON messages
             WHEN NEW.id = 'assistant-atomic-b'
             BEGIN
                 SELECT RAISE(ABORT, 'injected reconciliation failure');
             END;",
        )
        .unwrap();

    let error = service
        .reconcile_orphaned_in_progress_conversation_turn_traces(&HashSet::new(), 100)
        .unwrap_err();
    assert!(error.contains("injected reconciliation failure"));
    for assistant_message_id in ["assistant-atomic-a", "assistant-atomic-b"] {
        assert_eq!(
            service
                .get_conversation_turn_trace(assistant_message_id)
                .unwrap()
                .unwrap()
                .terminal_status,
            crate::ConversationTurnTraceTerminalStatus::InProgress
        );
    }
}
