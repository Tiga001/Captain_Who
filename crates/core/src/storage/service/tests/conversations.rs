use super::*;

#[test]
fn deleting_conversation_and_project_removes_composer_drafts() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    service
        .save_conversation(conversation(
            "conversation-1",
            Some("project-1"),
            "message-1",
        ))
        .unwrap();
    service
        .save_conversation(conversation(
            "conversation-2",
            Some("project-1"),
            "message-2",
        ))
        .unwrap();
    service
        .save_conversation(conversation(
            "conversation-3",
            Some("project-2"),
            "message-3",
        ))
        .unwrap();

    service
        .save_composer_draft(composer_draft(
            "conversation-1",
            Some("project-1"),
            "draft 1",
        ))
        .unwrap();
    service
        .save_composer_draft(composer_draft(
            "conversation-2",
            Some("project-1"),
            "draft 2",
        ))
        .unwrap();
    service
        .save_composer_draft(composer_draft(
            "new-conversation-project-1",
            Some("project-1"),
            "new draft",
        ))
        .unwrap();
    service
        .save_composer_draft(composer_draft(
            "conversation-3",
            Some("project-2"),
            "draft 3",
        ))
        .unwrap();

    service.delete_conversation("conversation-1").unwrap();

    let mut scopes = service
        .load_composer_drafts()
        .unwrap()
        .into_iter()
        .map(|draft| draft.scope_id)
        .collect::<Vec<_>>();
    scopes.sort();
    assert_eq!(
        scopes,
        vec![
            "conversation-2".to_string(),
            "conversation-3".to_string(),
            "new-conversation-project-1".to_string()
        ]
    );

    service.delete_project("project-1").unwrap();

    let mut scopes = service
        .load_composer_drafts()
        .unwrap()
        .into_iter()
        .map(|draft| draft.scope_id)
        .collect::<Vec<_>>();
    scopes.sort();
    assert_eq!(scopes, vec!["conversation-3".to_string()]);
}

#[test]
fn conversation_fork_clones_exact_history_archives_and_rewrites_trace_refs() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    service
        .save_conversation(ChatConversationRecord {
            id: "conversation-archive-source".to_string(),
            project_id: None,
            model_id: Some("model-1".to_string()),
            title: "archive source".to_string(),
            messages: vec![
                ChatMessageRecord {
                    id: "user-archive-source".to_string(),
                    role: "user".to_string(),
                    content: "read it".to_string(),
                    created_at: 1,
                    status: Some("sent".to_string()),
                    attachments: Vec::new(),
                    agent_run_json: None,
                    ui_state_json: None,
                },
                ChatMessageRecord {
                    id: "assistant-archive-source".to_string(),
                    role: "assistant".to_string(),
                    content: "done".to_string(),
                    created_at: 2,
                    status: Some("sent".to_string()),
                    attachments: Vec::new(),
                    agent_run_json: None,
                    ui_state_json: None,
                },
            ],
            created_at: 1,
            updated_at: 2,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
    let exact = "{\"content\":\"fork exact history\"}".repeat(20_000);
    let archive = service
        .archive_conversation_tool_result(
            crate::storage::conversation_history_archive_repository::ConversationHistoryArchiveInput {
                conversation_id: "conversation-archive-source".to_string(),
                assistant_message_id: "assistant-archive-source".to_string(),
                sequence: 1,
                call_id: "call-archive-source".to_string(),
                tool: "read_file".to_string(),
                content_type: "application/json".to_string(),
                content: exact.clone(),
                truncated_at_source: false,
                model_projection_truncated: false,
                archive_projection_truncated: false,
                created_at: 3,
            },
        )
        .unwrap();
    let trace = ConversationTurnTrace {
        schema_version: crate::CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
        run_id: "run-archive-source".to_string(),
        conversation_id: "conversation-archive-source".to_string(),
        assistant_message_id: "assistant-archive-source".to_string(),
        terminal_status: crate::ConversationTurnTraceTerminalStatus::Completed,
        terminal_error: None,
        truncated: true,
        items: vec![
            ConversationTurnTraceItem::ToolCall {
                sequence: 0,
                call_id: "call-archive-source".to_string(),
                tool: "read_file".to_string(),
                operation: serde_json::json!({ "path": "large.txt" }),
                approval_status: crate::AgentApprovalStatus::NotRequired,
                truncated: false,
            },
            ConversationTurnTraceItem::ToolResult {
                sequence: 1,
                call_id: "call-archive-source".to_string(),
                tool: "read_file".to_string(),
                status: crate::ConversationTraceToolResultStatus::Succeeded,
                success: true,
                observation: serde_json::json!({ "path": "large.txt", "summary": "bounded" }),
                approval_status: crate::AgentApprovalStatus::NotRequired,
                error: None,
                truncated: true,
                archive: crate::conversation_trace::ConversationHistoryArchiveTraceMetadata {
                    archive_ref: Some(archive.archive_ref.clone()),
                    content_hash: Some(archive.content_hash.clone()),
                    archived_bytes: Some(archive.total_bytes),
                    archived_completely: Some(true),
                    history_projection_truncated: true,
                    ..Default::default()
                },
            },
        ],
    };
    service
        .replace_conversation_turn_trace(&trace, 2, 3)
        .unwrap();

    let forked = service
        .fork_conversation(ForkConversationInput {
            request_id: "fork-archive-request".to_string(),
            source_conversation_id: "conversation-archive-source".to_string(),
            through_assistant_message_id: "assistant-archive-source".to_string(),
        })
        .unwrap();
    let forked_assistant = forked.messages.last().unwrap();
    let forked_trace = service
        .get_conversation_turn_trace(&forked_assistant.id)
        .unwrap()
        .unwrap();
    let ConversationTurnTraceItem::ToolResult {
        archive: forked_archive,
        ..
    } = &forked_trace.items[1]
    else {
        panic!("forked trace must retain the result");
    };
    assert_ne!(forked_archive.archive_ref, Some(archive.archive_ref));
    assert_eq!(
        forked_archive.content_hash.as_deref(),
        Some(archive.content_hash.as_str())
    );
    let page = service
        .read_conversation_history_archive_page(
            &forked.id,
            forked_archive.archive_ref.as_deref().unwrap(),
            crate::storage::conversation_history_archive_repository::ConversationHistoryArchivePageUnit::Char,
            0,
            u64::MAX,
        )
        .unwrap()
        .unwrap();
    assert_eq!(page.content, exact);
    let hits = service
        .search_conversation_history(
            &forked.id,
            "fork exact history",
            &crate::storage::conversation_history_repository::ConversationHistorySearchFilter {
                include_archives: true,
                tool: Some("read_file".to_string()),
                ..Default::default()
            },
            10,
        )
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert!(matches!(
        &hits[0].reference,
        crate::storage::conversation_history_repository::ConversationHistoryRecordRef::Archive {
            archive_ref
        } if Some(archive_ref.as_str()) == forked_archive.archive_ref.as_deref()
    ));
}

#[test]
fn conversation_fork_clones_all_visible_turn_diffs_and_supports_recursive_forks() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let source_conversation_id = "conversation-turn-diff-source";
    let messages = [
        ("user-turn-1", "user", 1, None),
        ("assistant-turn-1", "assistant", 2, Some("run-turn-1")),
        ("user-turn-2", "user", 3, None),
        ("assistant-turn-2", "assistant", 4, Some("run-turn-2")),
        ("user-turn-3", "user", 5, None),
        ("assistant-turn-3", "assistant", 6, Some("run-turn-3")),
    ]
    .into_iter()
    .map(|(id, role, created_at, run_id)| ChatMessageRecord {
        id: id.to_string(),
        role: role.to_string(),
        content: format!("content {id}"),
        created_at,
        status: Some("sent".to_string()),
        attachments: Vec::new(),
        agent_run_json: run_id.map(|run_id| {
            serde_json::json!({
                "runId": run_id,
                "status": "completed"
            })
            .to_string()
        }),
        ui_state_json: None,
    })
    .collect::<Vec<_>>();
    service
        .save_conversation(ChatConversationRecord {
            id: source_conversation_id.to_string(),
            project_id: Some("project-1".to_string()),
            model_id: Some("model-1".to_string()),
            title: "turn diff source".to_string(),
            messages,
            created_at: 1,
            updated_at: 6,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();

    let workspace_root = fixture.root.join("project-1").to_string_lossy().to_string();
    let source_turns = [
        (
            "run-turn-1",
            "assistant-turn-1",
            "action-turn-1",
            AgentTurnFileChange {
                path: "src/first.rs".to_string(),
                before: crate::AgentTurnFileContent::Missing,
                after: crate::AgentTurnFileContent::Text("first\n".to_string()),
            },
        ),
        (
            "run-turn-2",
            "assistant-turn-2",
            "action-turn-2",
            AgentTurnFileChange {
                path: "src/second.rs".to_string(),
                before: crate::AgentTurnFileContent::Text("before\n".to_string()),
                after: crate::AgentTurnFileContent::Text("after\n".to_string()),
            },
        ),
        (
            "run-turn-3",
            "assistant-turn-3",
            "action-turn-3",
            AgentTurnFileChange {
                path: "src/after-cutoff.rs".to_string(),
                before: crate::AgentTurnFileContent::Missing,
                after: crate::AgentTurnFileContent::Text("excluded\n".to_string()),
            },
        ),
    ];
    for (run_id, assistant_message_id, action_id, change) in &source_turns {
        service
            .replace_conversation_turn_trace(
                &ConversationTurnTrace {
                    schema_version: crate::CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
                    run_id: (*run_id).to_string(),
                    conversation_id: source_conversation_id.to_string(),
                    assistant_message_id: (*assistant_message_id).to_string(),
                    terminal_status: crate::ConversationTurnTraceTerminalStatus::Completed,
                    terminal_error: None,
                    truncated: false,
                    items: Vec::new(),
                },
                1,
                2,
            )
            .unwrap();
        let identity = AgentTurnDiffIdentity {
            run_id: (*run_id).to_string(),
            conversation_id: source_conversation_id.to_string(),
            assistant_message_id: (*assistant_message_id).to_string(),
            project_id: "project-1".to_string(),
            workspace_root: workspace_root.clone(),
        };
        service.initialize_agent_turn_diff(&identity).unwrap();
        assert!(service
            .record_agent_turn_file_change(&identity, action_id, change)
            .unwrap());
    }

    let first_fork = service
        .fork_conversation(ForkConversationInput {
            request_id: "fork-turn-diffs-through-second".to_string(),
            source_conversation_id: source_conversation_id.to_string(),
            through_assistant_message_id: "assistant-turn-2".to_string(),
        })
        .unwrap();
    assert_eq!(first_fork.messages.len(), 4);
    let forked_assistants = first_fork
        .messages
        .iter()
        .filter(|message| message.role == "assistant")
        .collect::<Vec<_>>();
    let forked_run_id = |message: &ChatMessageRecord| {
        serde_json::from_str::<serde_json::Value>(
            message.agent_run_json.as_deref().expect("agent run"),
        )
        .unwrap()["runId"]
            .as_str()
            .unwrap()
            .to_string()
    };

    let latest = service
        .load_latest_agent_turn_diff(&first_fork.id, "project-1")
        .unwrap()
        .unwrap();
    assert_eq!(
        latest.identity.assistant_message_id,
        forked_assistants[1].id
    );
    assert_eq!(
        latest.files,
        vec![AgentTurnFileChange {
            path: "src/second.rs".to_string(),
            before: crate::AgentTurnFileContent::Text("before\n".to_string()),
            after: crate::AgentTurnFileContent::Text("after\n".to_string()),
        }]
    );
    let forked_turns = service
        .load_agent_turn_diffs_for_messages(
            &first_fork.id,
            "project-1",
            &forked_assistants
                .iter()
                .map(|message| message.id.clone())
                .collect::<Vec<_>>(),
        )
        .unwrap();
    assert_eq!(forked_turns.len(), 2);
    assert_eq!(
        forked_turns[0].identity.assistant_message_id,
        forked_assistants[0].id
    );
    assert_eq!(forked_turns[0].files[0].path, "src/first.rs");
    assert_eq!(
        forked_turns[1].identity.assistant_message_id,
        forked_assistants[1].id
    );
    assert_eq!(forked_turns[1].files[0].path, "src/second.rs");

    for (message, action_id, change) in [
        (forked_assistants[0], "action-turn-1", &source_turns[0].3),
        (forked_assistants[1], "action-turn-2", &source_turns[1].3),
    ] {
        let identity = AgentTurnDiffIdentity {
            run_id: forked_run_id(message),
            conversation_id: first_fork.id.clone(),
            assistant_message_id: message.id.clone(),
            project_id: "project-1".to_string(),
            workspace_root: workspace_root.clone(),
        };
        assert!(
            !service
                .record_agent_turn_file_change(&identity, action_id, change)
                .unwrap(),
            "forked action ids must retain their idempotency evidence"
        );
    }

    let recursive_fork = service
        .fork_conversation(ForkConversationInput {
            request_id: "fork-turn-diffs-recursively".to_string(),
            source_conversation_id: first_fork.id.clone(),
            through_assistant_message_id: forked_assistants[0].id.clone(),
        })
        .unwrap();
    assert_eq!(recursive_fork.messages.len(), 2);
    let recursive_latest = service
        .load_latest_agent_turn_diff(&recursive_fork.id, "project-1")
        .unwrap()
        .unwrap();
    assert_eq!(
        recursive_latest.files,
        vec![AgentTurnFileChange {
            path: "src/first.rs".to_string(),
            before: crate::AgentTurnFileContent::Missing,
            after: crate::AgentTurnFileContent::Text("first\n".to_string()),
        }]
    );
}

#[test]
fn composer_drafts_only_preserve_full_for_current_permission_semantics() {
    let fixture = StorageFixture::new();
    let service = fixture.service();

    let mut legacy = composer_draft("legacy", None, "legacy full");
    legacy.permission_mode = "full".to_string();
    legacy.permission_mode_version = 0;
    let legacy = service.save_composer_draft(legacy).unwrap();
    assert_eq!(legacy.permission_mode, "default");

    let mut current = composer_draft("current", None, "current full");
    current.permission_mode = "full".to_string();
    current.permission_mode_version =
        crate::storage::models::CURRENT_COMPOSER_PERMISSION_MODE_VERSION;
    current.queued_messages_json =
        r#"[{"id":"queued-1","clientMessageId":"client-1","content":"guide"}]"#.to_string();
    let current = service.save_composer_draft(current).unwrap();
    assert_eq!(current.permission_mode, "full");
    assert_eq!(
        current.queued_messages_json,
        r#"[{"id":"queued-1","clientMessageId":"client-1","content":"guide"}]"#
    );

    let stored = service.load_composer_drafts().unwrap();
    assert_eq!(
        stored
            .iter()
            .find(|draft| draft.scope_id == "legacy")
            .unwrap()
            .permission_mode,
        "default"
    );
    assert_eq!(
        stored
            .iter()
            .find(|draft| draft.scope_id == "current")
            .unwrap()
            .permission_mode,
        "full"
    );
    assert_eq!(
        stored
            .iter()
            .find(|draft| draft.scope_id == "current")
            .unwrap()
            .queued_messages_json,
        r#"[{"id":"queued-1","clientMessageId":"client-1","content":"guide"}]"#
    );
}

#[test]
fn stale_saves_cannot_recreate_deleted_project_data() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let stale_conversation = conversation("conversation-stale", Some("project-1"), "message-stale");
    service
        .save_conversation(stale_conversation.clone())
        .unwrap();
    service.delete_project("project-1").unwrap();

    assert!(service
        .save_conversation(stale_conversation.clone())
        .is_err());
    assert!(service
        .save_conversation_meta(ChatConversationMetaRecord {
            id: stale_conversation.id.clone(),
            project_id: stale_conversation.project_id.clone(),
            model_id: stale_conversation.model_id.clone(),
            title: stale_conversation.title.clone(),
            created_at: stale_conversation.created_at,
            updated_at: stale_conversation.updated_at,
            pinned_at: stale_conversation.pinned_at,
            archived_at: stale_conversation.archived_at,
            unread_at: stale_conversation.unread_at,
        })
        .is_err());
    assert!(service
        .upsert_chat_messages(
            &stale_conversation.id,
            stale_conversation.messages.clone(),
            0,
        )
        .is_err());
    assert!(service
        .save_composer_draft(composer_draft(
            &stale_conversation.id,
            Some("project-1"),
            "stale draft",
        ))
        .is_err());
    assert!(service.load_conversations().unwrap().is_empty());
    assert!(service.load_composer_drafts().unwrap().is_empty());
}

#[test]
fn deleting_conversation_removes_agent_rows_and_keeps_usage_rollup() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    service
        .save_conversation(conversation(
            "conversation-1",
            Some("project-1"),
            "message-1",
        ))
        .unwrap();
    service
        .upsert_agent_usage(agent_usage_record("conversation-1", "message-1"))
        .unwrap();
    service
        .store_pending_agent_action(pending_action("action-1", "conversation-1"))
        .unwrap();
    service
        .upsert_agent_action_audit(action_audit("action-1", "conversation-1"))
        .unwrap();

    service.delete_conversation("conversation-1").unwrap();

    let summary = service
        .get_usage_summary(
            &crate::AgentUsageSummaryInput {
                range: crate::AgentUsageSummaryRange::All,
                from: None,
                to: None,
            },
            200_000,
        )
        .unwrap();
    assert_eq!(summary.request_count, 1);
    assert_eq!(summary.message_count, 1);
    assert_eq!(summary.input_tokens, Some(12));
    assert_eq!(summary.output_tokens, Some(8));
    assert_eq!(summary.total_tokens, Some(20));

    let connection = service.state.connection().unwrap();
    let raw_usage_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM agent_usage_records", [], |row| {
            row.get(0)
        })
        .unwrap();
    let rollup_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM agent_deleted_usage_daily_rollups",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let pending_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM agent_pending_actions", [], |row| {
            row.get(0)
        })
        .unwrap();
    let audit_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM agent_action_audit", [], |row| {
            row.get(0)
        })
        .unwrap();

    assert_eq!(raw_usage_count, 0);
    assert_eq!(rollup_count, 1);
    assert_eq!(pending_count, 0);
    assert_eq!(audit_count, 0);
}

#[test]
fn deleting_messages_keeps_usage_totals_via_rollup() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let mut record = conversation("conversation-usage-delete", Some("project-1"), "message-1");
    record.messages.push(ChatMessageRecord {
        id: "message-2".to_string(),
        role: "assistant".to_string(),
        content: "done".to_string(),
        created_at: 2,
        status: Some("sent".to_string()),
        attachments: Vec::new(),
        agent_run_json: None,
        ui_state_json: None,
    });
    record.updated_at = 2;
    service.save_conversation(record).unwrap();
    let mut first_usage = agent_usage_record("conversation-usage-delete", "message-1");
    first_usage.estimated_cost = Some(0.25);
    service.upsert_agent_usage(first_usage).unwrap();
    let mut second_usage = agent_usage_record("conversation-usage-delete", "message-2");
    second_usage.estimated_cost = Some(0.25);
    service.upsert_agent_usage(second_usage).unwrap();

    service
        .delete_chat_messages("conversation-usage-delete", &["message-1".to_string()])
        .unwrap();

    let summary = service
        .get_usage_summary(
            &crate::AgentUsageSummaryInput {
                range: crate::AgentUsageSummaryRange::All,
                from: None,
                to: None,
            },
            200_000,
        )
        .unwrap();
    assert_eq!(summary.request_count, 2);
    assert_eq!(summary.message_count, 2);
    assert_eq!(summary.input_tokens, Some(24));
    assert_eq!(summary.output_tokens, Some(16));
    assert_eq!(summary.total_tokens, Some(40));
    assert_eq!(summary.estimated_cost, Some(0.5));

    let connection = service.state.connection().unwrap();
    assert_eq!(
        connection
            .query_row("SELECT COUNT(*) FROM agent_usage_records", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
        1
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM agent_deleted_usage_daily_rollups",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        1
    );
}
