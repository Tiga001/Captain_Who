use super::*;

#[test]
fn collaboration_root_fork_reopens_with_raw_snapshot_and_accepts_a_new_turn() {
    let fixture = tempfile::tempdir().unwrap();
    let database_path = fixture.path().join("storage.sqlite");
    let service = StorageService::open_with_model_credentials(
        &database_path,
        Arc::new(crate::image_generation::InMemoryCredentialStore::default()),
    )
    .unwrap();
    let source_conversation_id = "conversation-fork-continue-source";
    let source_assistant_message_id = "assistant-fork-continue-source";
    let mut source = conversation(source_conversation_id, None, "user-fork-continue-source");
    source.messages.push(ChatMessageRecord {
        human_interaction_response: None,
        id: source_assistant_message_id.to_string(),
        role: "assistant".to_string(),
        content: "Durable source answer".to_string(),
        created_at: 2,
        status: Some("sent".to_string()),
        attachments: Vec::new(),
        // The renderer Timeline and authoritative Usage are reconstructed around this raw
        // lifecycle projection. Neither derived value belongs in the persisted Fork snapshot.
        folder_references_json: None,
        agent_run_json: Some(
            crate::storage::chat_repository::canonical_agent_run_lifecycle_projection(
                None,
                "run-fork-continue-source",
                "completed",
                1,
                2,
                Some(2),
            )
            .unwrap(),
        ),
        ui_state_json: None,
    });
    source.updated_at = 2;
    service.save_conversation(source).unwrap();
    service
        .ensure_root_agent(&EnsureRootAgentInput {
            agent_id: "agent-fork-continue-source".to_string(),
            conversation_id: source_conversation_id.to_string(),
            creation_request_id: "ensure-fork-continue-source".to_string(),
            task_name: "Fork continuation source".to_string(),
        })
        .unwrap();

    let source_trace = ConversationTurnTrace {
        schema_version: crate::CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
        run_id: "run-fork-continue-source".to_string(),
        conversation_id: source_conversation_id.to_string(),
        assistant_message_id: source_assistant_message_id.to_string(),
        terminal_status: crate::ConversationTurnTraceTerminalStatus::Completed,
        terminal_error: None,
        truncated: false,
        items: vec![
            ConversationTurnTraceItem::AssistantNarration {
                provider_turn_id: None,
                first_tool_call_id: None,
                sequence: 0,
                content: "Inspecting durable history".to_string(),
                truncated: false,
            },
            ConversationTurnTraceItem::ToolCall {
                sequence: 1,
                call_id: "call-fork-continue".to_string(),
                tool: "read_file".to_string(),
                provenance: crate::AgentToolIdentity::Builtin {
                    tool_name: "read_file".to_string(),
                },
                operation: serde_json::json!({ "path": "README.md" }),
                approval_status: crate::AgentApprovalStatus::NotRequired,
                truncated: false,
            },
            ConversationTurnTraceItem::ToolResult {
                sequence: 2,
                call_id: "call-fork-continue".to_string(),
                tool: "read_file".to_string(),
                status: crate::ConversationTraceToolResultStatus::Succeeded,
                success: true,
                observation: serde_json::json!({ "content": "bounded" }),
                approval_status: crate::AgentApprovalStatus::NotRequired,
                error: None,
                truncated: false,
                archive: Default::default(),
            },
        ],
    };
    let mut in_progress = source_trace.clone();
    in_progress.terminal_status = crate::ConversationTurnTraceTerminalStatus::InProgress;
    service
        .append_in_progress_conversation_turn_trace(&in_progress, 2, 2)
        .unwrap();
    service
        .replace_conversation_turn_trace(&source_trace, 2, 3)
        .unwrap();
    let mut source_usage = agent_usage_record(source_conversation_id, source_assistant_message_id);
    source_usage.id = "usage-fork-continue-source".to_string();
    source_usage.run_id = "run-fork-continue-source".to_string();
    service.upsert_agent_usage(source_usage).unwrap();
    let source_view = service
        .load_conversation(source_conversation_id)
        .unwrap()
        .unwrap();
    let source_view_run: serde_json::Value =
        serde_json::from_str(source_view.messages[1].agent_run_json.as_deref().unwrap()).unwrap();
    assert_eq!(source_view_run["usage"]["totalTokens"], 20);
    assert_eq!(source_view_run["timeline"][0]["type"], "message");
    let (source_admission, _) = service
        .load_conversation_for_turn(source_conversation_id)
        .unwrap();
    let source_admission_run: serde_json::Value = serde_json::from_str(
        source_admission.unwrap().messages[1]
            .agent_run_json
            .as_deref()
            .unwrap(),
    )
    .unwrap();
    assert!(source_admission_run.get("usage").is_none());
    assert!(source_admission_run["timeline"]
        .as_array()
        .unwrap()
        .is_empty());

    let forked = service
        .fork_conversation_request_view(assistant_reply_fork_request(
            "fork-continue-request",
            source_conversation_id,
            source_assistant_message_id,
        ))
        .unwrap()
        .conversation;
    let forked_conversation_id = forked.id.clone();
    let forked_assistant_message_id = forked.messages[1].id.clone();
    let projected_run: serde_json::Value = serde_json::from_str(
        forked.messages[1]
            .agent_run_json
            .as_deref()
            .expect("fork view must project the cloned durable Trace"),
    )
    .unwrap();
    assert_eq!(projected_run["timeline"][0]["type"], "message");
    assert_eq!(projected_run["timeline"][1]["type"], "tool_call");
    assert_eq!(projected_run["usage"]["totalTokens"], 0);

    let persisted_fork_run = {
        let connection = service.state.connection().unwrap();
        connection
            .query_row(
                "SELECT agent_run_json FROM messages
                 WHERE conversation_id = ?1 AND id = ?2",
                rusqlite::params![&forked_conversation_id, &forked_assistant_message_id],
                |row| row.get::<_, Option<String>>(0),
            )
            .unwrap()
    };
    assert_ne!(
        forked.messages[1].agent_run_json, persisted_fork_run,
        "the Fork return value is a renderer projection, not write-back input"
    );

    // Reopen to cover the real application lifecycle: Turn admission must receive the raw
    // immutable snapshot, while a renderer read continues to receive the reconstructed Timeline.
    drop(service);
    let reopened = StorageService::open_with_model_credentials(
        &database_path,
        Arc::new(crate::image_generation::InMemoryCredentialStore::default()),
    )
    .unwrap();
    let (candidate, revision) = reopened
        .load_conversation_for_turn(&forked_conversation_id)
        .unwrap();
    let mut candidate = candidate.unwrap();
    assert_eq!(candidate.messages[1].agent_run_json, persisted_fork_run);
    let next_created_at = candidate.updated_at.saturating_add(1);
    candidate.updated_at = next_created_at.saturating_add(1);
    candidate.messages.extend([
        ChatMessageRecord {
            human_interaction_response: None,
            id: "user-fork-continue-next".to_string(),
            role: "user".to_string(),
            content: "Continue after the fork".to_string(),
            created_at: next_created_at,
            status: Some("sent".to_string()),
            attachments: Vec::new(),
            folder_references_json: None,
            agent_run_json: None,
            ui_state_json: None,
        },
        ChatMessageRecord {
            human_interaction_response: None,
            id: "assistant-fork-continue-next".to_string(),
            role: "assistant".to_string(),
            content: "Thinking...".to_string(),
            created_at: next_created_at.saturating_add(1),
            status: Some("pending".to_string()),
            attachments: Vec::new(),
            folder_references_json: None,
            agent_run_json: None,
            ui_state_json: None,
        },
    ]);
    let next_trace = crate::ConversationTraceSnapshot::default().in_progress_trace(
        "run-fork-continue-next",
        &forked_conversation_id,
        "assistant-fork-continue-next",
    );
    reopened
        .save_conversation_and_begin_turn(
            candidate,
            revision,
            None,
            crate::AgentTurnPermissionSource::HostAuthenticatedRoot(
                crate::AgentPermissions::default(),
            ),
            &next_trace,
            next_created_at.saturating_add(1),
            next_created_at.saturating_add(1),
        )
        .unwrap();
    let completed_trace = crate::completed_conversation_trace_without_items(
        "run-fork-continue-next",
        &forked_conversation_id,
        "assistant-fork-continue-next",
    );
    reopened
        .finalize_chat_message_with_conversation_trace(
            &forked_conversation_id,
            "assistant-fork-continue-next",
            "Continued answer",
            Some("sent"),
            "completed",
            &completed_trace,
            next_created_at.saturating_add(1),
            next_created_at.saturating_add(2),
        )
        .unwrap();

    let view = reopened
        .load_conversation(&forked_conversation_id)
        .unwrap()
        .unwrap();
    assert_eq!(view.messages.len(), 4);
    assert_eq!(view.messages[3].content, "Continued answer");
    let old_projected_run: serde_json::Value =
        serde_json::from_str(view.messages[1].agent_run_json.as_deref().unwrap()).unwrap();
    assert_eq!(old_projected_run["timeline"][0]["type"], "message");

    let connection = reopened.state.connection().unwrap();
    let (raw_run_json, origin_kind) = connection
        .query_row(
            "SELECT agent_run_json, input_origin_kind FROM messages
             WHERE conversation_id = ?1 AND id = ?2",
            rusqlite::params![&forked_conversation_id, &forked_assistant_message_id],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(raw_run_json, persisted_fork_run);
    assert_eq!(origin_kind.as_deref(), Some("snapshot"));
}
