use super::fixtures::{append_durable_pending_trace, test_pending_resume_checkpoint_for_call};
use super::*;

#[test]
fn startup_reconciliation_failure_prevents_agent_service_startup() {
    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("storage.sqlite");
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    save_test_pending_provider(
        &storage,
        "test-model",
        "https://example.test/v1/chat/completions",
        "test-token",
        "disabled",
        "",
    );
    storage
        .save_conversation(ChatConversationRecord {
            id: "conversation-bad-reconciliation".to_string(),
            project_id: None,
            model_id: Some("model-1".to_string()),
            title: "bad reconciliation".to_string(),
            messages: vec![ChatMessageRecord {
                human_interaction_response: None,
                id: "assistant-bad-reconciliation".to_string(),
                role: "assistant".to_string(),
                content: String::new(),
                created_at: 1,
                status: Some("pending".to_string()),
                attachments: Vec::new(),
                folder_references_json: None,
                agent_run_json: Some("{}".to_string()),
                ui_state_json: None,
            }],
            created_at: 1,
            updated_at: 1,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();

    let call = AgentToolCall {
        id: "bad-reconciliation-call".to_string(),
        tool: "approval_tool".to_string(),
        args: json!({}),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };
    let action = AgentProposedAction::ToolCall { call: call.clone() };
    let mut agent_input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "test-token",
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "messages": []
    }))
    .unwrap();
    freeze_test_pending_provider_configuration(&storage, &mut agent_input);
    agent_input.resume_checkpoint = Some(test_pending_resume_checkpoint_for_call(
        &storage,
        "bad-reconciliation-run",
        Some("bad-reconciliation-call"),
        &call,
        AgentToolIdentity::Builtin {
            tool_name: call.tool.clone(),
        },
    ));
    append_durable_pending_trace(
        &storage,
        "conversation-bad-reconciliation",
        "assistant-bad-reconciliation",
        "bad-reconciliation-run",
        &call,
        AgentToolIdentity::Builtin {
            tool_name: call.tool.clone(),
        },
        1,
    );
    storage
        .store_pending_agent_action(AgentPendingActionRecord {
            action_id: pending_action_storage_id(
                "bad-reconciliation-run",
                "bad-reconciliation-call",
            ),
            run_id: "bad-reconciliation-run".to_string(),
            conversation_id: Some("conversation-bad-reconciliation".to_string()),
            assistant_message_id: Some("assistant-bad-reconciliation".to_string()),
            action_type: "tool_call".to_string(),
            tool_name: "approval_tool".to_string(),
            tool_call_id: Some("bad-reconciliation-call".to_string()),
            status: "approved".to_string(),
            target_status: None,
            action_json: serialize_json(&action),
            // Keep this row on the supported Host projection so the test exercises the generic
            // interrupted-action reconciliation failure rather than legacy-format retirement.
            agent_input_json: PersistedAgentResumeInput::from_agent_input(&agent_input)
                .unwrap()
                .encode(),
            created_at: 1,
            updated_at: 1,
        })
        .unwrap();

    // Corrupt the persisted row explicitly so the test continues to cover startup fail-closed
    // behavior while ordinary repository writes remain protected by the canonical CHECK.
    let corruption = rusqlite::Connection::open(&database_path).unwrap();
    corruption
        .pragma_update(None, "ignore_check_constraints", 1)
        .unwrap();
    corruption
        .execute(
            "UPDATE messages SET agent_run_json = ?1 WHERE id = ?2",
            ["not-json", "assistant-bad-reconciliation"],
        )
        .unwrap();
    corruption
        .pragma_update(None, "ignore_check_constraints", 0)
        .unwrap();
    drop(corruption);

    let error = match AgentService::try_new(storage) {
        Ok(_) => panic!("reconciliation failure must prevent startup"),
        Err(error) => error,
    };
    assert!(error.contains("failed to reconcile interrupted pending actions"));
    assert!(error.contains("无法解析中断操作"));
}

#[test]
fn agent_service_startup_retires_an_orphaned_cancelled_conversation_trace() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    storage
        .save_conversation(ChatConversationRecord {
            id: "conversation-orphaned-trace".to_string(),
            project_id: None,
            model_id: Some("model-1".to_string()),
            title: "orphaned trace".to_string(),
            messages: vec![ChatMessageRecord {
                human_interaction_response: None,
                id: "assistant-orphaned-trace".to_string(),
                role: "assistant".to_string(),
                content: "partial response".to_string(),
                created_at: 1,
                status: Some("sent".to_string()),
                attachments: Vec::new(),
                folder_references_json: None,
                agent_run_json: Some(
                    json!({
                        "runId": "run-orphaned-trace",
                        "status": "cancelled",
                        "completedAt": 20,
                        "state": {
                            "status": "running",
                            "activeRunId": null,
                            "updatedAt": 10
                        }
                    })
                    .to_string(),
                ),
                ui_state_json: None,
            }],
            created_at: 1,
            updated_at: 2,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
    let trace = ConversationTurnTrace {
        schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
        run_id: "run-orphaned-trace".to_string(),
        conversation_id: "conversation-orphaned-trace".to_string(),
        assistant_message_id: "assistant-orphaned-trace".to_string(),
        terminal_status: ConversationTurnTraceTerminalStatus::InProgress,
        terminal_error: None,
        truncated: false,
        items: vec![ConversationTurnTraceItem::AssistantNarration {
            first_tool_call_id: None,
            provider_turn_id: None,
            sequence: 0,
            content: "Reading the image.".to_string(),
            truncated: false,
        }],
    };
    storage
        .append_in_progress_conversation_turn_trace_and_apply_guidances(
            &trace,
            &[ConversationModelContextItem {
                images: Vec::new(),
                sequence: 0,
                ordinal: 0,
                role: "assistant".to_string(),
                content: "Reading the image.".to_string(),
                tool_call_id: None,
                tool_calls: Vec::new(),
                is_error: false,
            }],
            10,
            15,
        )
        .unwrap();

    let _service = AgentService::new_authorized_for_test(storage.clone());

    let repaired = storage
        .get_conversation_turn_trace("assistant-orphaned-trace")
        .unwrap()
        .unwrap();
    assert_eq!(
        repaired.terminal_status,
        ConversationTurnTraceTerminalStatus::Cancelled
    );
    assert_eq!(repaired.items, trace.items);
    let conversation = storage
        .load_conversation("conversation-orphaned-trace")
        .unwrap()
        .unwrap();
    assert_eq!(conversation.updated_at, 2);
    let run: Value =
        serde_json::from_str(conversation.messages[0].agent_run_json.as_deref().unwrap()).unwrap();
    assert_eq!(run["status"], "cancelled");
    assert_eq!(run["state"]["status"], "cancelled");
}
