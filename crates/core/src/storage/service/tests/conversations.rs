use super::*;

#[test]
fn conversation_load_rebuilds_mcp_timeline_items_at_their_trace_positions() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let conversation_id = "conversation-mcp-trace-order";
    let assistant_message_id = "assistant-mcp-trace-order";
    let run_id = "run-mcp-trace-order";
    service
        .save_conversation(ChatConversationRecord {
            id: conversation_id.to_string(),
            project_id: None,
            model_id: Some("model-1".to_string()),
            title: "MCP trace order".to_string(),
            messages: vec![
                ChatMessageRecord {
                    id: "user-mcp-trace-order".to_string(),
                    role: "user".to_string(),
                    content: "Use two MCP tools.".to_string(),
                    created_at: 1,
                    status: Some("sent".to_string()),
                    attachments: Vec::new(),
                    agent_run_json: None,
                    ui_state_json: None,
                },
                ChatMessageRecord {
                    id: assistant_message_id.to_string(),
                    role: "assistant".to_string(),
                    content: "Done.".to_string(),
                    created_at: 2,
                    status: Some("sent".to_string()),
                    attachments: Vec::new(),
                    agent_run_json: Some(
                        serde_json::json!({
                            "runId": run_id,
                            "status": "completed",
                            "timeline": [
                                {
                                    "id": "stale-mcp-two",
                                    "type": "mcp_tool_call",
                                    "invocationId": "invocation-two"
                                },
                                {
                                    "id": "legacy-without-trace-anchor",
                                    "type": "mcp_tool_call",
                                    "invocationId": "legacy-invocation"
                                },
                                {
                                    "id": "stale-message",
                                    "type": "message",
                                    "content": "stale narration"
                                },
                                {
                                    "id": "stale-mcp-one",
                                    "type": "mcp_tool_call",
                                    "invocationId": "invocation-one"
                                },
                                {
                                    "id": "duplicate-mcp-one",
                                    "type": "mcp_tool_call",
                                    "invocationId": "invocation-one"
                                }
                            ],
                            "mcpInvocations": [
                                {
                                    "callId": "call-mcp-one",
                                    "invocationId": "invocation-one"
                                },
                                {
                                    "callId": "call-mcp-one",
                                    "invocationId": "invocation-one"
                                },
                                {
                                    "callId": "call-mcp-two",
                                    "invocationId": "invocation-two"
                                },
                                {
                                    "callId": "call-without-trace",
                                    "invocationId": "legacy-invocation"
                                }
                            ]
                        })
                        .to_string(),
                    ),
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

    let successful_result =
        |sequence: u64, call_id: &str, tool: &str| ConversationTurnTraceItem::ToolResult {
            sequence,
            call_id: call_id.to_string(),
            tool: tool.to_string(),
            status: crate::ConversationTraceToolResultStatus::Succeeded,
            success: true,
            observation: serde_json::json!({ "ok": true }),
            approval_status: crate::AgentApprovalStatus::NotRequired,
            error: None,
            truncated: false,
            archive: Default::default(),
        };
    let trace = ConversationTurnTrace {
        schema_version: crate::CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
        run_id: run_id.to_string(),
        conversation_id: conversation_id.to_string(),
        assistant_message_id: assistant_message_id.to_string(),
        terminal_status: crate::ConversationTurnTraceTerminalStatus::Completed,
        terminal_error: None,
        truncated: false,
        items: vec![
            ConversationTurnTraceItem::AssistantNarration {
                sequence: 0,
                content: "Before first MCP.".to_string(),
                truncated: false,
            },
            ConversationTurnTraceItem::ToolCall {
                sequence: 1,
                call_id: "call-mcp-one".to_string(),
                tool: "mcp_tool_one".to_string(),
                provenance: None,
                operation: serde_json::json!({}),
                approval_status: crate::AgentApprovalStatus::NotRequired,
                truncated: false,
            },
            successful_result(2, "call-mcp-one", "mcp_tool_one"),
            ConversationTurnTraceItem::AssistantNarration {
                sequence: 3,
                content: "Between MCP calls.".to_string(),
                truncated: false,
            },
            ConversationTurnTraceItem::ToolCall {
                sequence: 4,
                call_id: "call-mcp-two".to_string(),
                tool: "mcp_tool_two".to_string(),
                provenance: None,
                operation: serde_json::json!({}),
                approval_status: crate::AgentApprovalStatus::NotRequired,
                truncated: false,
            },
            successful_result(5, "call-mcp-two", "mcp_tool_two"),
            ConversationTurnTraceItem::AssistantNarration {
                sequence: 6,
                content: "Before ordinary tool.".to_string(),
                truncated: false,
            },
            ConversationTurnTraceItem::ToolCall {
                sequence: 7,
                call_id: "call-ordinary".to_string(),
                tool: "read_file".to_string(),
                provenance: None,
                operation: serde_json::json!({ "path": "notes.txt" }),
                approval_status: crate::AgentApprovalStatus::NotRequired,
                truncated: false,
            },
            successful_result(8, "call-ordinary", "read_file"),
            ConversationTurnTraceItem::AssistantNarration {
                sequence: 9,
                content: "After ordinary tool.".to_string(),
                truncated: false,
            },
        ],
    };
    service
        .replace_conversation_turn_trace(&trace, 2, 3)
        .unwrap();

    let loaded = service.load_conversation(conversation_id).unwrap().unwrap();
    let assistant = loaded
        .messages
        .iter()
        .find(|message| message.id == assistant_message_id)
        .unwrap();
    let run = serde_json::from_str::<serde_json::Value>(
        assistant.agent_run_json.as_deref().expect("agent run"),
    )
    .unwrap();
    let timeline = run["timeline"].as_array().unwrap();
    let restored = timeline
        .iter()
        .map(|item| match item["type"].as_str().unwrap() {
            "message" => format!("message:{}", item["content"].as_str().unwrap()),
            "mcp_tool_call" => {
                format!("mcp:{}", item["invocationId"].as_str().unwrap())
            }
            "tool_call" => format!("tool:{}", item["callId"].as_str().unwrap()),
            other => panic!("unexpected timeline item type: {other}"),
        })
        .collect::<Vec<_>>();

    assert_eq!(
        restored,
        vec![
            "message:Before first MCP.",
            "mcp:invocation-one",
            "message:Between MCP calls.",
            "mcp:invocation-two",
            "message:Before ordinary tool.",
            "tool:call-ordinary",
            "message:After ordinary tool.",
        ]
    );
    assert_eq!(
        timeline
            .iter()
            .filter(|item| item["type"] == "mcp_tool_call")
            .count(),
        2,
        "typed MCP timeline markers must be emitted once from durable trace anchors"
    );
    assert!(
        timeline
            .iter()
            .all(|item| item["invocationId"] != "legacy-invocation"),
        "an MCP presentation item without a durable trace anchor must fail closed"
    );
}

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
    let managed_session_id = "cmd_000000000000000000000000000000a1";
    let managed_running_exact =
        format!(r#"{{"status":"running","sessionId":"{managed_session_id}"}}"#);
    let managed_running_archive = service
        .archive_conversation_tool_result(
            crate::storage::conversation_history_archive_repository::ConversationHistoryArchiveInput {
                conversation_id: "conversation-archive-source".to_string(),
                assistant_message_id: "assistant-archive-source".to_string(),
                sequence: 5,
                call_id: "call-managed-source".to_string(),
                tool: "run_command".to_string(),
                content_type: "application/json".to_string(),
                content: managed_running_exact.clone(),
                truncated_at_source: false,
                model_projection_truncated: false,
                archive_projection_truncated: false,
                created_at: 4,
            },
        )
        .unwrap();
    let managed_terminal_exact =
        r#"{"status":"exited","exitCode":0,"stdout":"managed exact output"}"#.to_string();
    let managed_terminal_archive = service
        .archive_conversation_tool_result(
            crate::storage::conversation_history_archive_repository::ConversationHistoryArchiveInput {
                conversation_id: "conversation-archive-source".to_string(),
                assistant_message_id: "assistant-archive-source".to_string(),
                sequence: 6,
                call_id: format!("command-session:{managed_session_id}"),
                tool: "run_command".to_string(),
                content_type: "application/json".to_string(),
                content: managed_terminal_exact.clone(),
                truncated_at_source: false,
                model_projection_truncated: true,
                archive_projection_truncated: false,
                created_at: 5,
            },
        )
        .unwrap();
    let source_archive_open =
        crate::storage::conversation_history_open::encode_archive_history_open(
            archive.archive_ref.clone(),
            37,
        )
        .unwrap();
    let source_tool_exchange_open =
        crate::storage::conversation_history_open::encode_history_open(
            &crate::storage::conversation_history_open::HistoryOpenRoute::ToolExchange {
                reference: crate::storage::conversation_history_repository::ConversationHistoryRecordRef::TraceItem {
                    assistant_message_id: "assistant-archive-source".to_string(),
                    sequence: 1,
                },
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
                provenance: None,
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
            ConversationTurnTraceItem::ToolCall {
                sequence: 2,
                call_id: "call-history-source".to_string(),
                tool: "conversation_history".to_string(),
                provenance: None,
                operation: serde_json::json!({ "open": source_archive_open }),
                approval_status: crate::AgentApprovalStatus::NotRequired,
                truncated: false,
            },
            ConversationTurnTraceItem::ToolResult {
                sequence: 3,
                call_id: "call-history-source".to_string(),
                tool: "conversation_history".to_string(),
                status: crate::ConversationTraceToolResultStatus::Succeeded,
                success: true,
                observation: serde_json::json!({
                    "view": "exact_tool_result",
                    "navigation": {
                        "next": source_archive_open,
                        "toolExchange": source_tool_exchange_open
                    }
                }),
                approval_status: crate::AgentApprovalStatus::NotRequired,
                error: None,
                truncated: false,
                archive: Default::default(),
            },
            ConversationTurnTraceItem::ToolCall {
                sequence: 4,
                call_id: "call-managed-source".to_string(),
                tool: "run_command".to_string(),
                provenance: None,
                operation: serde_json::json!({ "command": "python3 app.py" }),
                approval_status: crate::AgentApprovalStatus::Approved,
                truncated: false,
            },
            ConversationTurnTraceItem::ToolResult {
                sequence: 5,
                call_id: "call-managed-source".to_string(),
                tool: "run_command".to_string(),
                status: crate::ConversationTraceToolResultStatus::Succeeded,
                success: true,
                observation: serde_json::json!({
                    "status": "running",
                    "sessionId": managed_session_id
                }),
                approval_status: crate::AgentApprovalStatus::Approved,
                error: None,
                truncated: false,
                archive: crate::conversation_trace::ConversationHistoryArchiveTraceMetadata {
                    archive_ref: Some(managed_running_archive.archive_ref.clone()),
                    content_hash: Some(managed_running_archive.content_hash.clone()),
                    archived_bytes: Some(managed_running_archive.total_bytes),
                    archived_completely: Some(true),
                    ..Default::default()
                },
            },
            ConversationTurnTraceItem::CommandSessionLifecycle {
                sequence: 6,
                phase: crate::ConversationCommandSessionLifecyclePhase::Terminal,
                session_id: managed_session_id.to_string(),
                call_id: "call-managed-source".to_string(),
                status: crate::AgentCommandSessionStatus::Exited,
                exit_code: Some(0),
                latest_sequence: 1,
                output_truncated: false,
                archive: crate::conversation_trace::ConversationHistoryArchiveTraceMetadata {
                    archive_ref: Some(managed_terminal_archive.archive_ref.clone()),
                    content_hash: Some(managed_terminal_archive.content_hash.clone()),
                    archived_bytes: Some(managed_terminal_archive.total_bytes),
                    archived_completely: Some(true),
                    model_projection_truncated: true,
                    ..Default::default()
                },
                created_at: 5,
            },
        ],
    };
    let exact_model_items = vec![
        crate::ConversationModelContextItem {
            sequence: 0,
            ordinal: 0,
            role: "assistant".to_string(),
            content: String::new(),
            tool_call_id: None,
            tool_calls: vec![crate::AgentContextCheckpointToolCall {
                id: "call-archive-source".to_string(),
                name: "read_file".to_string(),
                args: serde_json::json!({ "path": "large.txt" }),
                provider_identity: None,
            }],
            is_error: false,
        },
        crate::ConversationModelContextItem {
            sequence: 1,
            ordinal: 0,
            role: "tool".to_string(),
            content: r#"{"ok":true,"result":{"content":"EXACT_FORK_MODEL_MARKER"}}"#.to_string(),
            tool_call_id: Some("call-archive-source".to_string()),
            tool_calls: Vec::new(),
            is_error: false,
        },
        crate::ConversationModelContextItem {
            sequence: 2,
            ordinal: 0,
            role: "assistant".to_string(),
            content: String::new(),
            tool_call_id: None,
            tool_calls: vec![crate::AgentContextCheckpointToolCall {
                id: "call-history-source".to_string(),
                name: "conversation_history".to_string(),
                args: serde_json::json!({ "open": source_archive_open }),
                provider_identity: None,
            }],
            is_error: false,
        },
        crate::ConversationModelContextItem {
            sequence: 3,
            ordinal: 0,
            role: "tool".to_string(),
            content: serde_json::json!({
                "view": "exact_tool_result",
                "navigation": {
                    "next": source_archive_open,
                    "toolExchange": source_tool_exchange_open
                }
            })
            .to_string(),
            tool_call_id: Some("call-history-source".to_string()),
            tool_calls: Vec::new(),
            is_error: false,
        },
        crate::ConversationModelContextItem {
            sequence: 4,
            ordinal: 0,
            role: "assistant".to_string(),
            content: String::new(),
            tool_call_id: None,
            tool_calls: vec![crate::AgentContextCheckpointToolCall {
                id: "call-managed-source".to_string(),
                name: "run_command".to_string(),
                args: serde_json::json!({ "command": "python3 app.py" }),
                provider_identity: None,
            }],
            is_error: false,
        },
        crate::ConversationModelContextItem {
            sequence: 5,
            ordinal: 0,
            role: "tool".to_string(),
            content: managed_running_exact.clone(),
            tool_call_id: Some("call-managed-source".to_string()),
            tool_calls: Vec::new(),
            is_error: false,
        },
    ];
    let mut in_progress_trace = trace.clone();
    in_progress_trace.terminal_status = crate::ConversationTurnTraceTerminalStatus::InProgress;
    service
        .append_in_progress_conversation_turn_trace_and_apply_guidances(
            &in_progress_trace,
            &exact_model_items,
            2,
            3,
        )
        .unwrap();
    service
        .replace_conversation_turn_trace(&trace, 2, 4)
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
    let forked_model_context = service
        .get_conversation_model_context_log(&forked_assistant.id)
        .unwrap()
        .unwrap();
    assert_eq!(forked_model_context.items.len(), exact_model_items.len());
    assert!(forked_model_context.items[1]
        .content
        .contains("EXACT_FORK_MODEL_MARKER"));
    let ConversationTurnTraceItem::ToolResult {
        archive: forked_archive,
        ..
    } = &forked_trace.items[1]
    else {
        panic!("forked trace must retain the result");
    };
    assert_ne!(
        forked_archive.archive_ref.as_deref(),
        Some(archive.archive_ref.as_str())
    );
    assert_eq!(
        forked_archive.content_hash.as_deref(),
        Some(archive.content_hash.as_str())
    );
    let forked_archive_ref = forked_archive.archive_ref.as_deref().unwrap();
    let ConversationTurnTraceItem::ToolResult {
        observation: forked_history_observation,
        ..
    } = &forked_trace.items[3]
    else {
        panic!("forked trace must retain the conversation_history result");
    };
    let forked_trace_next = forked_history_observation["navigation"]["next"]
        .as_str()
        .unwrap();
    assert_eq!(
        crate::storage::conversation_history_open::decode_history_open(forked_trace_next).unwrap(),
        crate::storage::conversation_history_open::HistoryOpenRoute::Archive {
            archive_ref: forked_archive_ref.to_string(),
            start_char: 37,
        }
    );
    let forked_trace_source = forked_history_observation["navigation"]["toolExchange"]
        .as_str()
        .unwrap();
    assert_eq!(
        crate::storage::conversation_history_open::decode_history_open(forked_trace_source)
            .unwrap(),
        crate::storage::conversation_history_open::HistoryOpenRoute::ToolExchange {
            reference: crate::storage::conversation_history_repository::ConversationHistoryRecordRef::TraceItem {
                assistant_message_id: forked_assistant.id.clone(),
                sequence: 1,
            },
        }
    );
    let forked_model_history =
        serde_json::from_str::<serde_json::Value>(&forked_model_context.items[3].content).unwrap();
    assert_eq!(
        crate::storage::conversation_history_open::decode_history_open(
            forked_model_history["navigation"]["next"].as_str().unwrap()
        )
        .unwrap(),
        crate::storage::conversation_history_open::HistoryOpenRoute::Archive {
            archive_ref: forked_archive_ref.to_string(),
            start_char: 37,
        }
    );
    assert!(
        !forked_model_context.items[3]
            .content
            .contains(&archive.archive_ref),
        "forked model history must not retain the source archive identity inside an opaque open"
    );
    let page = service
        .read_conversation_history_archive_page(
            &forked.id,
            forked_archive_ref,
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

    let ConversationTurnTraceItem::ToolResult {
        archive: forked_managed_running,
        ..
    } = &forked_trace.items[5]
    else {
        panic!("forked trace must retain the managed running result");
    };
    let ConversationTurnTraceItem::CommandSessionLifecycle {
        archive: forked_managed_terminal,
        ..
    } = &forked_trace.items[6]
    else {
        panic!("forked trace must retain the managed terminal lifecycle");
    };
    let forked_managed_running_ref = forked_managed_running.archive_ref.as_deref().unwrap();
    let forked_managed_terminal_ref = forked_managed_terminal.archive_ref.as_deref().unwrap();
    assert_ne!(forked_managed_running_ref, forked_managed_terminal_ref);
    assert_eq!(
        forked_managed_running.content_hash.as_deref(),
        Some(managed_running_archive.content_hash.as_str())
    );
    assert_eq!(
        forked_managed_terminal.content_hash.as_deref(),
        Some(managed_terminal_archive.content_hash.as_str())
    );
    for (archive_ref, expected_call_id, expected_content) in [
        (
            forked_managed_running_ref,
            "call-managed-source",
            managed_running_exact.as_str(),
        ),
        (
            forked_managed_terminal_ref,
            "command-session:cmd_000000000000000000000000000000a1",
            managed_terminal_exact.as_str(),
        ),
    ] {
        let descriptor = {
            let connection = service.state.connection().unwrap();
            crate::storage::conversation_history_archive_repository::find_archive_by_ref(
                &connection,
                &forked.id,
                archive_ref,
            )
            .unwrap()
            .unwrap()
        };
        assert_eq!(descriptor.call_id, expected_call_id);
        let page = service
            .read_conversation_history_archive_page(
                &forked.id,
                archive_ref,
                crate::storage::conversation_history_archive_repository::ConversationHistoryArchivePageUnit::Char,
                0,
                u64::MAX,
            )
            .unwrap()
            .unwrap();
        assert_eq!(page.content, expected_content);
    }

    let recursively_forked = service
        .fork_conversation(ForkConversationInput {
            request_id: "fork-archive-recursive-request".to_string(),
            source_conversation_id: forked.id.clone(),
            through_assistant_message_id: forked_assistant.id.clone(),
        })
        .unwrap();
    let recursive_assistant = recursively_forked.messages.last().unwrap();
    let recursive_trace = service
        .get_conversation_turn_trace(&recursive_assistant.id)
        .unwrap()
        .unwrap();
    let recursive_refs = [&recursive_trace.items[5], &recursive_trace.items[6]]
        .into_iter()
        .map(|item| match item {
            ConversationTurnTraceItem::ToolResult { archive, .. }
            | ConversationTurnTraceItem::CommandSessionLifecycle { archive, .. } => {
                archive.archive_ref.as_deref().unwrap()
            }
            _ => panic!("recursive managed archive item has the wrong type"),
        })
        .collect::<Vec<_>>();
    assert_ne!(recursive_refs[0], recursive_refs[1]);
    let connection = service.state.connection().unwrap();
    let recursive_call_ids = recursive_refs
        .iter()
        .map(|archive_ref| {
            crate::storage::conversation_history_archive_repository::find_archive_by_ref(
                &connection,
                &recursively_forked.id,
                archive_ref,
            )
            .unwrap()
            .unwrap()
            .call_id
        })
        .collect::<Vec<_>>();
    assert_eq!(
        recursive_call_ids,
        vec![
            "call-managed-source".to_string(),
            "command-session:cmd_000000000000000000000000000000a1".to_string(),
        ]
    );
}

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
        .fork_conversation_view(ForkConversationInput {
            request_id: "fork-active-command-request".to_string(),
            source_conversation_id: "conversation-active-fork".to_string(),
            // The active command belongs to a later turn. Fork admission is intentionally scoped
            // to the complete source conversation, not only the copied prefix.
            through_assistant_message_id: "assistant-active-fork-1".to_string(),
        })
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
        .fork_conversation(ForkConversationInput {
            request_id: "fork-after-command-settlement".to_string(),
            source_conversation_id: "conversation-active-fork".to_string(),
            through_assistant_message_id: "assistant-active-fork-1".to_string(),
        })
        .unwrap();
    assert_eq!(forked.messages.len(), 2);

    create_fork_test_command_session(
        &service,
        "cmd_000000000000000000000000000000b6",
        "conversation-active-fork",
        "assistant-active-fork-2",
        50,
    );
    let idempotent_retry = service
        .fork_conversation(ForkConversationInput {
            request_id: "fork-after-command-settlement".to_string(),
            source_conversation_id: "conversation-active-fork".to_string(),
            through_assistant_message_id: "assistant-active-fork-1".to_string(),
        })
        .unwrap();
    assert_eq!(idempotent_retry.id, forked.id);
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
                        provenance: None,
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
    let plan = crate::storage::conversation_fork_repository::build_fork_plan(
        &connection,
        &ForkConversationInput {
            request_id: "fork-race-request".to_string(),
            source_conversation_id: "conversation-fork-race".to_string(),
            through_assistant_message_id: "assistant-fork-race-1".to_string(),
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

fn save_forkable_command_conversation(service: &StorageService, conversation_id: &str) {
    service
        .save_conversation(ChatConversationRecord {
            id: conversation_id.to_string(),
            project_id: None,
            model_id: Some("model-1".to_string()),
            title: "fork command source".to_string(),
            messages: vec![
                ("user", "1", 1),
                ("assistant", "1", 2),
                ("user", "2", 3),
                ("assistant", "2", 4),
            ]
            .into_iter()
            .map(|(role, suffix, created_at)| ChatMessageRecord {
                id: format!(
                    "{role}-{conversation_id_suffix}-{suffix}",
                    conversation_id_suffix = conversation_id.trim_start_matches("conversation-")
                ),
                role: role.to_string(),
                content: format!("{role} {suffix}"),
                created_at,
                status: Some("sent".to_string()),
                attachments: Vec::new(),
                agent_run_json: (role == "assistant").then(|| {
                    serde_json::json!({
                        "runId": format!("run-{conversation_id}-{suffix}"),
                        "status": "completed"
                    })
                    .to_string()
                }),
                ui_state_json: None,
            })
            .collect(),
            created_at: 1,
            updated_at: 4,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
}

fn fork_test_command_session_create(
    session_id: &str,
    conversation_id: &str,
    assistant_message_id: &str,
    started_at: u64,
) -> crate::storage::agent_command_session_repository::AgentCommandSessionCreate {
    crate::storage::agent_command_session_repository::AgentCommandSessionCreate {
        snapshot: crate::AgentCommandSessionSnapshot {
            schema_version: crate::storage::agent_command_session_repository::AGENT_COMMAND_SESSION_SCHEMA_VERSION,
            session_id: session_id.to_string(),
            conversation_id: conversation_id.to_string(),
            assistant_message_id: assistant_message_id.to_string(),
            origin_run_id: format!("run-{session_id}"),
            call_id: format!("call-{session_id}"),
            project_id: None,
            command: "python3 app.py".to_string(),
            cwd: "/tmp".to_string(),
            command_digest:
                "sha256:0000000000000000000000000000000000000000000000000000000000000000"
                    .to_string(),
            status: crate::AgentCommandSessionStatus::Starting,
            started_at,
            ended_at: None,
            exit_code: None,
            latest_sequence: 0,
            output_truncated: false,
            outputs: Vec::new(),
            archive_ref: None,
        },
        authorization_source: crate::command::CommandAuthorizationSource::ExplicitUser,
        approval_provenance: serde_json::json!({ "decision": "approved" }),
        permission_provenance: serde_json::json!({ "mode": "default" }),
        created_at: i64::try_from(started_at).unwrap(),
    }
}

fn create_fork_test_command_session(
    service: &StorageService,
    session_id: &str,
    conversation_id: &str,
    assistant_message_id: &str,
    started_at: u64,
) {
    service
        .create_agent_command_session(&fork_test_command_session_create(
            session_id,
            conversation_id,
            assistant_message_id,
            started_at,
        ))
        .unwrap();
}

fn settle_fork_test_command_session(
    service: &StorageService,
    session_id: &str,
    conversation_id: &str,
    status: crate::AgentCommandSessionStatus,
    timestamp: u64,
) {
    service
        .settle_agent_command_session(
            &crate::storage::agent_command_session_repository::AgentCommandSessionTerminalUpdate {
                conversation_id,
                session_id,
                status,
                ended_at: timestamp,
                exit_code: (status == crate::AgentCommandSessionStatus::Exited).then_some(0),
                latest_sequence: 0,
                transcript_truncated: false,
                output_capture_truncated: false,
                archive_ref: None,
                terminal_reason: None,
                published_outputs: &[],
                committed_at: i64::try_from(timestamp).unwrap(),
            },
        )
        .unwrap();
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
