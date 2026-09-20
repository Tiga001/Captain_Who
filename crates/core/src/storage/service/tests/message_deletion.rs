use super::*;

fn assistant_message(id: &str, created_at: i64) -> ChatMessageRecord {
    ChatMessageRecord {
        human_interaction_response: None,
        id: id.to_string(),
        role: "assistant".to_string(),
        content: format!("answer from {id}"),
        created_at,
        status: Some("sent".to_string()),
        attachments: Vec::new(),
        agent_run_json: None,
        ui_state_json: None,
    }
}

fn save_assistant_messages(
    service: &StorageService,
    conversation_id: &str,
    assistant_message_ids: &[&str],
) {
    let mut stored = conversation(conversation_id, Some("project-1"), assistant_message_ids[0]);
    stored.messages = assistant_message_ids
        .iter()
        .enumerate()
        .map(|(index, message_id)| assistant_message(message_id, index as i64 + 1))
        .collect();
    stored.updated_at = assistant_message_ids.len() as i64;
    service.save_conversation(stored).unwrap();
}

fn persist_settled_manual_command(
    service: &StorageService,
    call_id: &str,
    run_id: &str,
    conversation_id: &str,
    assistant_message_id: &str,
) -> String {
    let storage_id = crate::canonical_pending_action_id(run_id, call_id);
    let action = AgentProposedAction::Command {
        command: crate::AgentCommandRequest {
            id: call_id.to_string(),
            command: "node build.mjs".to_string(),
            cwd: None,
            timeout_ms: None,
            approval_status: crate::AgentApprovalStatus::Required,
            risk_level: None,
            reason: Some("build the reviewed artifact".to_string()),
            observe: None,
            inputs: Vec::new(),
            runtime_binding: None,
            managed_office_script: None,
        },
    };
    let action_json = serde_json::to_string(&action).unwrap();
    let checkpoint_call = ConversationTurnTraceItem::ToolCall {
        sequence: 0,
        call_id: call_id.to_string(),
        tool: "run_command".to_string(),
        provenance: crate::AgentToolIdentity::Builtin {
            tool_name: "run_command".to_string(),
        },
        operation: serde_json::json!({
            "command": "node build.mjs",
        }),
        approval_status: crate::AgentApprovalStatus::Required,
        truncated: false,
    };
    let pending = AgentPendingActionRecord {
        action_id: storage_id.clone(),
        run_id: run_id.to_string(),
        conversation_id: Some(conversation_id.to_string()),
        assistant_message_id: Some(assistant_message_id.to_string()),
        action_type: "command".to_string(),
        tool_name: "run_command".to_string(),
        tool_call_id: Some(call_id.to_string()),
        status: "approved".to_string(),
        target_status: None,
        action_json: action_json.clone(),
        agent_input_json: serde_json::json!({
            "apiUrl": "https://not-used.invalid/v1",
            "apiToken": "redacted",
            "model": "message-deletion-test",
            "messages": [],
            "resumeCheckpoint": {
                "version": 2,
                "runId": run_id,
                "contextItems": [],
                "nextModelRequestIndex": 1,
                "queuedToolCalls": [],
                "suppressedNarration": false,
                "extensionSnapshots": [],
                "pendingToolCallId": call_id,
                "conversationTraceItems": [checkpoint_call],
                "nextConversationTraceSequence": 1,
                "conversationTraceTruncated": false,
                "modelVisibleTraceItemCount": 0
            }
        })
        .to_string(),
        created_at: 10,
        updated_at: 11,
    };
    service.store_pending_agent_action(pending).unwrap();

    let approved_audit = AgentActionAuditRecord {
        action_id: storage_id.clone(),
        run_id: run_id.to_string(),
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
    service
        .upsert_agent_action_audit(approved_audit.clone())
        .unwrap();

    let command_result = AgentCommandExecutionResult {
        outputs: Vec::new(),
        command: "node build.mjs".to_string(),
        cwd: "/workspace".to_string(),
        exit_code: Some(0),
        stdout: "artifact created".to_string(),
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
    let mut terminal_audit = approved_audit;
    terminal_audit.status = "completed".to_string();
    terminal_audit.command_result_json = Some(serde_json::to_string(&command_result).unwrap());
    terminal_audit.tool_result_json = Some(serde_json::to_string(&tool_result).unwrap());
    terminal_audit.completed_at = Some(12);
    let call = AgentToolCall {
        id: call_id.to_string(),
        tool: "run_command".to_string(),
        args: serde_json::json!({
            "command": "node build.mjs",
        }),
        approval_status: crate::AgentApprovalStatus::Approved,
        reason: Some("build the reviewed artifact".to_string()),
    };
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
                call_id: call.id.clone(),
                tool: call.tool.clone(),
                provenance: crate::AgentToolIdentity::Builtin {
                    tool_name: call.tool.clone(),
                },
                operation: call.args.clone(),
                approval_status: call.approval_status,
                truncated: false,
            },
            crate::conversation_trace::projected_tool_result_trace_item(1, &call, &tool_result),
        ],
    };
    let model_context_items = vec![
        crate::ConversationModelContextItem {
            images: Vec::new(),
            sequence: 0,
            ordinal: 0,
            role: "assistant".to_string(),
            content: String::new(),
            tool_call_id: None,
            tool_calls: vec![crate::AgentContextCheckpointToolCall {
                id: call.id.clone(),
                name: call.tool.clone(),
                args: call.args.clone(),
                provider_identity: crate::AgentProviderToolCallIdentity {
                    provider_tool_index: 0,
                    provider_call_id: call.id.clone(),
                    runtime_call_id: call.id.clone(),
                },
            }],
            is_error: false,
        },
        crate::ConversationModelContextItem {
            images: Vec::new(),
            sequence: 1,
            ordinal: 0,
            role: "tool".to_string(),
            content: serde_json::to_string(&tool_result).unwrap(),
            tool_call_id: Some(call.id.clone()),
            tool_calls: Vec::new(),
            is_error: false,
        },
    ];
    service
        .commit_pending_agent_action_audited_result_trace_with_model_context(
            &terminal_audit,
            "approved",
            "completed",
            &trace,
            &model_context_items,
            12,
        )
        .unwrap();
    storage_id
}

#[test]
fn deleting_owner_message_atomically_retires_settled_file_effect_receipts() {
    let fixture = StorageFixture::new();
    let database_path = fixture.root.join("storage.sqlite");
    let service = fixture.service();
    save_assistant_messages(
        &service,
        "conversation-delete-owner",
        &["assistant-delete", "assistant-retain"],
    );
    service
        .save_input_attachments(
            "conversation-delete-owner",
            "assistant-delete",
            Some("project-1"),
            &[input_attachment(
                &service,
                "attachment-delete",
                AgentInputAttachmentKind::File,
                "artifact.txt",
                Some("text/plain"),
                b"artifact",
            )],
            4,
        )
        .unwrap();
    let attachment_path = {
        let library = service
            .build_attachment_library_context("conversation-delete-owner", Some("project-1"))
            .unwrap();
        PathBuf::from(library.root_path.unwrap())
            .join(&library.conversation_attachments[0].storage_rel_path)
    };
    assert!(attachment_path.is_file());

    let deleted_storage_id = persist_settled_manual_command(
        &service,
        "call-delete",
        "run-delete",
        "conversation-delete-owner",
        "assistant-delete",
    );
    // A Conversation has exactly one durable in-progress Turn. Settle the first interrupted
    // continuation before constructing the second historical owner in this deletion fixture.
    assert_eq!(
        service
            .reconcile_interrupted_pending_agent_actions(41)
            .unwrap()
            .len(),
        1
    );
    let retained_storage_id = persist_settled_manual_command(
        &service,
        "call-retain",
        "run-retain",
        "conversation-delete-owner",
        "assistant-retain",
    );
    assert_eq!(
        service
            .reconcile_interrupted_pending_agent_actions(42)
            .unwrap()
            .len(),
        1
    );
    assert!(service.list_unsettled_file_effects().unwrap().is_empty());

    service
        .delete_chat_messages(
            "conversation-delete-owner",
            &["assistant-delete".to_string()],
        )
        .unwrap();

    let connection = service.state.connection().unwrap();
    let deleted_receipts: (i64, i64) = connection
        .query_row(
            "
            SELECT
                (SELECT COUNT(*) FROM agent_pending_actions WHERE action_id = ?1),
                (SELECT COUNT(*) FROM agent_action_audit WHERE action_id = ?1)
            ",
            [&deleted_storage_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    let retained_receipts: (i64, i64) = connection
        .query_row(
            "
            SELECT
                (SELECT COUNT(*) FROM agent_pending_actions WHERE action_id = ?1),
                (SELECT COUNT(*) FROM agent_action_audit WHERE action_id = ?1)
            ",
            [&retained_storage_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    let deleted_message_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM messages WHERE id = 'assistant-delete'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let deleted_attachment_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM attachments WHERE id = 'attachment-delete'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    drop(connection);

    assert_eq!(deleted_receipts, (0, 0));
    assert_eq!(retained_receipts, (1, 1));
    assert_eq!(deleted_message_count, 0);
    assert_eq!(deleted_attachment_count, 0);
    assert!(!attachment_path.exists());
    assert!(service
        .get_conversation_turn_trace("assistant-delete")
        .unwrap()
        .is_none());
    assert!(service
        .get_conversation_turn_trace("assistant-retain")
        .unwrap()
        .is_some());
    drop(service);

    let reopened = StorageService::open(&database_path).unwrap();
    assert!(reopened.list_unsettled_file_effects().unwrap().is_empty());
}

#[test]
fn failed_message_delete_rolls_back_attachments_and_action_receipts() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    save_assistant_messages(
        &service,
        "conversation-delete-rollback",
        &["assistant-rollback"],
    );
    service
        .save_input_attachments(
            "conversation-delete-rollback",
            "assistant-rollback",
            Some("project-1"),
            &[input_attachment(
                &service,
                "attachment-rollback",
                AgentInputAttachmentKind::File,
                "keep.txt",
                Some("text/plain"),
                b"keep",
            )],
            4,
        )
        .unwrap();
    let attachment_path = {
        let library = service
            .build_attachment_library_context("conversation-delete-rollback", Some("project-1"))
            .unwrap();
        PathBuf::from(library.root_path.unwrap())
            .join(&library.conversation_attachments[0].storage_rel_path)
    };

    let mut pending = pending_action("receipt-rollback", "conversation-delete-rollback");
    pending.assistant_message_id = Some("assistant-rollback".to_string());
    service.store_pending_agent_action(pending).unwrap();
    let mut audit = action_audit("receipt-rollback", "conversation-delete-rollback");
    audit.assistant_message_id = Some("assistant-rollback".to_string());
    service.upsert_agent_action_audit(audit).unwrap();
    {
        let connection = service.state.connection().unwrap();
        connection
            .execute_batch(
                "
                CREATE TEMP TRIGGER reject_test_message_delete
                BEFORE DELETE ON messages
                WHEN OLD.id = 'assistant-rollback'
                BEGIN
                    SELECT RAISE(ABORT, 'injected message deletion failure');
                END;
                ",
            )
            .unwrap();
    }

    let error = service
        .delete_chat_messages(
            "conversation-delete-rollback",
            &["assistant-rollback".to_string()],
        )
        .unwrap_err();
    assert!(error.contains("injected message deletion failure"));

    let connection = service.state.connection().unwrap();
    let counts: (i64, i64, i64, i64) = connection
        .query_row(
            "
            SELECT
                (SELECT COUNT(*) FROM messages WHERE id = 'assistant-rollback'),
                (SELECT COUNT(*) FROM attachments WHERE id = 'attachment-rollback'),
                (SELECT COUNT(*) FROM agent_pending_actions WHERE action_id = 'receipt-rollback'),
                (SELECT COUNT(*) FROM agent_action_audit WHERE action_id = 'receipt-rollback')
            ",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(counts, (1, 1, 1, 1));
    assert!(attachment_path.is_file());
}
