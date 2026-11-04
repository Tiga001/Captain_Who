use super::*;

#[test]
fn conversation_fork_blocks_source_wide_active_command_without_mutating_it() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    save_forkable_command_conversation(&service, "conversation-active-fork");

    let active_session_id = "cmd_000000000000000000000000000000b1";
    create_fork_test_command_session(
        &service,
        active_session_id,
        "conversation-active-fork",
        "assistant-active-fork-2",
        10,
    );
    let running_session_id = "cmd_000000000000000000000000000000b2";
    create_fork_test_command_session(
        &service,
        running_session_id,
        "conversation-active-fork",
        "assistant-active-fork-2",
        10,
    );
    service
        .mark_agent_command_session_running("conversation-active-fork", running_session_id, 11)
        .unwrap();
    let starting_before = service
        .load_agent_command_session("conversation-active-fork", active_session_id)
        .unwrap()
        .unwrap();
    let running_before = service
        .load_agent_command_session("conversation-active-fork", running_session_id)
        .unwrap()
        .unwrap();

    let error = service
        .fork_conversation_request_view(assistant_reply_fork_request(
            "fork-active-command-request",
            "conversation-active-fork",
            // The active command belongs to a later turn. Fork admission is intentionally scoped
            // to the complete source conversation, not only the copied prefix.
            "assistant-active-fork-1",
        ))
        .unwrap_err();
    assert_eq!(
        error,
        crate::storage::conversation_fork_repository::ConversationForkError::ActiveCommandSession {
            conversation_id: "conversation-active-fork".to_string(),
            active_session_count: 2,
        }
    );
    let starting_after = service
        .load_agent_command_session("conversation-active-fork", active_session_id)
        .unwrap()
        .unwrap();
    let running_after = service
        .load_agent_command_session("conversation-active-fork", running_session_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        starting_after.snapshot.status,
        crate::AgentCommandSessionStatus::Starting
    );
    assert_eq!(starting_after.updated_at, starting_before.updated_at);
    assert_eq!(starting_after.settled_at, starting_before.settled_at);
    assert_eq!(
        running_after.snapshot.status,
        crate::AgentCommandSessionStatus::Running
    );
    assert_eq!(running_after.updated_at, running_before.updated_at);
    assert_eq!(running_after.settled_at, running_before.settled_at);

    settle_fork_test_command_session(
        &service,
        active_session_id,
        "conversation-active-fork",
        crate::AgentCommandSessionStatus::Exited,
        12,
    );
    settle_fork_test_command_session(
        &service,
        running_session_id,
        "conversation-active-fork",
        crate::AgentCommandSessionStatus::Interrupted,
        12,
    );
    for (suffix, status) in [
        ('3', crate::AgentCommandSessionStatus::TimedOut),
        ('4', crate::AgentCommandSessionStatus::Failed),
    ] {
        let session_id = format!("cmd_000000000000000000000000000000b{suffix}");
        create_fork_test_command_session(
            &service,
            &session_id,
            "conversation-active-fork",
            "assistant-active-fork-2",
            20,
        );
        settle_fork_test_command_session(
            &service,
            &session_id,
            "conversation-active-fork",
            status,
            21,
        );
    }
    let outcome_unknown_session_id = "cmd_000000000000000000000000000000b5";
    create_fork_test_command_session(
        &service,
        outcome_unknown_session_id,
        "conversation-active-fork",
        "assistant-active-fork-2",
        30,
    );
    {
        let connection = service.state.connection().unwrap();
        connection
            .execute(
                "UPDATE agent_command_sessions
                 SET status = 'outcome_unknown', ended_at = 31,
                     terminal_reason = 'restart', updated_at = 31, settled_at = 31
                 WHERE session_id = ?1",
                [outcome_unknown_session_id],
            )
            .unwrap();
    }

    let forked = service
        .fork_conversation_request_view(assistant_reply_fork_request(
            "fork-after-command-settlement",
            "conversation-active-fork",
            "assistant-active-fork-1",
        ))
        .unwrap()
        .conversation;
    assert_eq!(forked.messages.len(), 2);

    create_fork_test_command_session(
        &service,
        "cmd_000000000000000000000000000000b6",
        "conversation-active-fork",
        "assistant-active-fork-2",
        50,
    );
    let idempotent_retry = service
        .fork_conversation_request_view(assistant_reply_fork_request(
            "fork-after-command-settlement",
            "conversation-active-fork",
            "assistant-active-fork-1",
        ))
        .unwrap()
        .conversation;
    assert_eq!(idempotent_retry.id, forked.id);
}

#[test]
fn agent_tree_fork_rejects_an_active_member_command_without_partial_target_state() {
    const SOURCE_CONVERSATION_ID: &str = "conversation-active-member-tree";
    const SOURCE_ROOT_AGENT_ID: &str = "agent-active-member-tree-root";
    const CHILD_CONVERSATION_ID: &str = "conversation-active-member-tree-child";
    const CHILD_AGENT_ID: &str = "agent-active-member-tree-child";
    const CHILD_ASSISTANT_MESSAGE_ID: &str = "assistant-active-member-tree-child";
    const ACTIVE_SESSION_ID: &str = "cmd_000000000000000000000000000000d1";
    const FORK_REQUEST_ID: &str = "fork-active-member-tree-request";

    let fixture = StorageFixture::new();
    let service = fixture.service();
    let created_at = now_ms();
    let fork_boundary_at = created_at.saturating_add(60_000);
    service
        .save_conversation(ChatConversationRecord {
            id: SOURCE_CONVERSATION_ID.to_string(),
            project_id: None,
            model_id: Some("model-1".to_string()),
            title: "active member tree".to_string(),
            messages: vec![
                ChatMessageRecord {
                    human_interaction_response: None,
                    id: "user-active-member-tree-root".to_string(),
                    role: "user".to_string(),
                    content: "start the root task".to_string(),
                    created_at,
                    status: Some("sent".to_string()),
                    attachments: Vec::new(),
                    folder_references_json: None,
                    agent_run_json: None,
                    ui_state_json: None,
                },
                ChatMessageRecord {
                    human_interaction_response: None,
                    id: "assistant-active-member-tree-root".to_string(),
                    role: "assistant".to_string(),
                    content: "fork boundary".to_string(),
                    created_at: fork_boundary_at,
                    status: Some("sent".to_string()),
                    attachments: Vec::new(),
                    folder_references_json: None,
                    agent_run_json: Some(
                        serde_json::json!({
                            "runId": "run-active-member-tree-root",
                            "status": "completed"
                        })
                        .to_string(),
                    ),
                    ui_state_json: None,
                },
            ],
            created_at,
            updated_at: fork_boundary_at,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
    service
        .ensure_root_agent(&EnsureRootAgentInput {
            agent_id: SOURCE_ROOT_AGENT_ID.to_string(),
            conversation_id: SOURCE_CONVERSATION_ID.to_string(),
            creation_request_id: "ensure-active-member-tree-root".to_string(),
            task_name: "Active member tree".to_string(),
        })
        .unwrap();
    service
        .save_conversation(ChatConversationRecord {
            id: CHILD_CONVERSATION_ID.to_string(),
            project_id: None,
            model_id: Some("model-1".to_string()),
            title: "active child".to_string(),
            messages: Vec::new(),
            created_at: created_at.saturating_add(1),
            updated_at: created_at.saturating_add(1),
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
    {
        let connection = service.state.connection().unwrap();
        connection
            .execute(
                "INSERT INTO agent_nodes (
                     agent_id, schema_version, root_agent_id, root_conversation_id,
                     parent_agent_id, conversation_id, project_id, creation_request_id,
                     task_name, task_path,
                     model_config_id_snapshot, model_display_name_snapshot,
                     model_supports_image_snapshot, model_context_window_tokens_snapshot,
                     model_settings_revision_snapshot, provider_connection_revision_snapshot,
                     provider_protocol_revision_snapshot, model_selection_source_snapshot,
                     lifecycle, revision, created_at, updated_at
                 ) VALUES (
                     ?1, 1, ?2, ?3, ?2, ?4, NULL, ?5,
                     'active_child', '/root/active_child',
                     'model-1', 'Model 1', 0, 4096,
                     'settings-v1', 'connection-v1', 'protocol-v1', 'explicit',
                     'active', 1, ?6, ?6
                 )",
                rusqlite::params![
                    CHILD_AGENT_ID,
                    SOURCE_ROOT_AGENT_ID,
                    SOURCE_CONVERSATION_ID,
                    CHILD_CONVERSATION_ID,
                    "create-active-member-tree-child",
                    created_at.saturating_add(1),
                ],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO messages (
                     id, conversation_id, role, content, status, agent_run_json,
                     created_at, position
                 ) VALUES (
                     ?1, ?2, 'assistant', '', 'sent',
                     json_object('runId', 'run-active-member-tree-child',
                                 'status', 'completed'),
                     ?3, 0
                 )",
                rusqlite::params![
                    CHILD_ASSISTANT_MESSAGE_ID,
                    CHILD_CONVERSATION_ID,
                    created_at.saturating_add(2),
                ],
            )
            .unwrap();
    }
    create_fork_test_command_session(
        &service,
        ACTIVE_SESSION_ID,
        CHILD_CONVERSATION_ID,
        CHILD_ASSISTANT_MESSAGE_ID,
        10,
    );

    let durable_fork_state = || {
        let connection = service.state.connection().unwrap();
        let read_ids = |sql: &str| {
            connection
                .prepare(sql)
                .unwrap()
                .query_map([], |row| row.get::<_, String>(0))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap()
        };
        (
            read_ids("SELECT id FROM conversations ORDER BY id"),
            read_ids("SELECT agent_id FROM agent_nodes ORDER BY agent_id"),
            connection
                .query_row(
                    "SELECT COUNT(*) FROM conversation_forks WHERE request_id = ?1",
                    [FORK_REQUEST_ID],
                    |row| row.get::<_, u64>(0),
                )
                .unwrap(),
            connection
                .query_row(
                    "SELECT COUNT(*) FROM agent_member_conversation_forks
                     WHERE root_fork_request_id = ?1",
                    [FORK_REQUEST_ID],
                    |row| row.get::<_, u64>(0),
                )
                .unwrap(),
        )
    };
    let before = durable_fork_state();
    assert_eq!(before.2, 0);
    assert_eq!(before.3, 0);
    assert_eq!(
        service.list_agent_tree(SOURCE_ROOT_AGENT_ID).unwrap().len(),
        2
    );

    let error = service
        .fork_conversation_request_view(assistant_reply_fork_request(
            FORK_REQUEST_ID,
            SOURCE_CONVERSATION_ID,
            "assistant-active-member-tree-root",
        ))
        .unwrap_err();
    assert_eq!(
        error,
        crate::storage::conversation_fork_repository::ConversationForkError::ActiveCommandSession {
            conversation_id: CHILD_CONVERSATION_ID.to_string(),
            active_session_count: 1,
        }
    );

    let after = durable_fork_state();
    assert_eq!(
        after, before,
        "a rejected tree fork must leave no target state"
    );
    assert_eq!(
        service
            .load_agent_command_session(CHILD_CONVERSATION_ID, ACTIVE_SESSION_ID)
            .unwrap()
            .unwrap()
            .snapshot
            .status,
        crate::AgentCommandSessionStatus::Starting
    );
    let connection = service.state.connection().unwrap();
    let violations = connection
        .prepare("PRAGMA foreign_key_check")
        .unwrap()
        .query_map([], |_| Ok(()))
        .unwrap()
        .count();
    assert_eq!(violations, 0);
}

#[test]
fn fork_commit_rechecks_active_commands_before_writing_any_target_state() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    save_forkable_command_conversation(&service, "conversation-fork-race");
    let archive = service
        .archive_conversation_tool_result(
            crate::storage::conversation_history_archive_repository::ConversationHistoryArchiveInput {
                conversation_id: "conversation-fork-race".to_string(),
                assistant_message_id: "assistant-fork-race-1".to_string(),
                sequence: 1,
                call_id: "call-fork-race".to_string(),
                tool: "read_file".to_string(),
                content_type: "text/plain".to_string(),
                content: "fork race exact content".to_string(),
                truncated_at_source: false,
                model_projection_truncated: false,
                archive_projection_truncated: false,
                created_at: 3,
            },
        )
        .unwrap();
    service
        .replace_conversation_turn_trace(
            &ConversationTurnTrace {
                schema_version: crate::CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
                run_id: "run-conversation-fork-race-1".to_string(),
                conversation_id: "conversation-fork-race".to_string(),
                assistant_message_id: "assistant-fork-race-1".to_string(),
                terminal_status: crate::ConversationTurnTraceTerminalStatus::Completed,
                terminal_error: None,
                truncated: false,
                items: vec![
                    ConversationTurnTraceItem::ToolCall {
                        sequence: 0,
                        call_id: "call-fork-race".to_string(),
                        tool: "read_file".to_string(),
                        provenance: crate::AgentToolIdentity::Builtin {
                            tool_name: "read_file".to_string(),
                        },
                        operation: serde_json::json!({ "path": "large.txt" }),
                        approval_status: crate::AgentApprovalStatus::NotRequired,
                        truncated: false,
                    },
                    ConversationTurnTraceItem::ToolResult {
                        sequence: 1,
                        call_id: "call-fork-race".to_string(),
                        tool: "read_file".to_string(),
                        status: crate::ConversationTraceToolResultStatus::Succeeded,
                        success: true,
                        observation: serde_json::json!({ "content": "bounded" }),
                        approval_status: crate::AgentApprovalStatus::NotRequired,
                        error: None,
                        truncated: true,
                        archive:
                            crate::conversation_trace::ConversationHistoryArchiveTraceMetadata {
                                archive_ref: Some(archive.archive_ref.clone()),
                                content_hash: Some(archive.content_hash.clone()),
                                archived_bytes: Some(archive.total_bytes),
                                archived_completely: Some(true),
                                history_projection_truncated: true,
                                ..Default::default()
                            },
                    },
                ],
            },
            2,
            3,
        )
        .unwrap();

    let mut connection = service.state.connection().unwrap();
    let plan = crate::storage::conversation_fork_repository::build_fork_plan_at_point(
        &connection,
        "fork-race-request",
        "conversation-fork-race",
        &ConversationForkPoint::AssistantReply {
            assistant_message_id: "assistant-fork-race-1".to_string(),
        },
        40,
    )
    .unwrap();
    crate::storage::agent_command_session_repository::create_session(
        &mut connection,
        &fork_test_command_session_create(
            "cmd_000000000000000000000000000000c1",
            "conversation-fork-race",
            "assistant-fork-race-2",
            41,
        ),
    )
    .unwrap();
    let error =
        crate::storage::conversation_fork_repository::commit_fork_plan(&mut connection, &plan)
            .unwrap_err();
    assert!(matches!(
        error,
        crate::storage::conversation_fork_repository::ConversationForkError::ActiveCommandSession {
            active_session_count: 1,
            ..
        }
    ));

    crate::storage::agent_command_session_repository::commit_terminal(
        &mut connection,
        &crate::storage::agent_command_session_repository::AgentCommandSessionTerminalUpdate {
            conversation_id: "conversation-fork-race",
            session_id: "cmd_000000000000000000000000000000c1",
            status: crate::AgentCommandSessionStatus::Failed,
            ended_at: 42,
            exit_code: None,
            latest_sequence: 0,
            transcript_truncated: false,
            output_capture_truncated: false,
            archive_ref: None,
            terminal_reason: Some("test settlement"),
            published_outputs: &[],
            artifact_observation: None,
            committed_at: 42,
        },
    )
    .unwrap();
    connection
        .execute_batch(
            "CREATE TEMP TRIGGER force_fork_commit_failure
             BEFORE INSERT ON conversation_forks
             BEGIN
                 SELECT RAISE(ABORT, 'forced fork commit failure');
             END;",
        )
        .unwrap();
    let forced_error =
        crate::storage::conversation_fork_repository::commit_fork_plan(&mut connection, &plan)
            .unwrap_err();
    assert!(matches!(
        forced_error,
        crate::storage::conversation_fork_repository::ConversationForkError::Other(_)
    ));

    for (table, predicate) in [
        ("conversations", "id = ?1"),
        ("conversation_history_blobs", "conversation_id = ?1"),
        ("conversation_history_blob_chunks", "archive_ref IN (SELECT archive_ref FROM conversation_history_blobs WHERE conversation_id = ?1)"),
        ("conversation_history_fts", "conversation_id = ?1"),
        ("conversation_history_timeline", "conversation_id = ?1"),
        ("conversation_forks", "target_conversation_id = ?1"),
    ] {
        let count = connection
            .query_row(
                &format!("SELECT COUNT(*) FROM {table} WHERE {predicate}"),
                [&plan.target.id],
                |row| row.get::<_, u64>(0),
            )
            .unwrap();
        assert_eq!(count, 0, "{table} retained partial target state");
    }
    for table in [
        "messages",
        "conversation_turn_traces",
        "conversation_turn_trace_items",
        "conversation_model_context_items",
    ] {
        let column = if matches!(table, "messages" | "conversation_turn_traces") {
            "conversation_id"
        } else {
            "assistant_message_id"
        };
        let value = if column == "assistant_message_id" {
            plan.target.messages[1].id.as_str()
        } else {
            plan.target.id.as_str()
        };
        let count = connection
            .query_row(
                &format!("SELECT COUNT(*) FROM {table} WHERE {column} = ?1"),
                [value],
                |row| row.get::<_, u64>(0),
            )
            .unwrap();
        assert_eq!(count, 0, "{table} retained partial target state");
    }
}
