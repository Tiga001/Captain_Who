use super::*;

#[test]
fn prepared_turn_uses_backend_model_capabilities() {
    let fixture = tempdir().unwrap();
    let storage = StorageService::open(&fixture.path().join("storage.sqlite")).unwrap();
    let mut settings = test_model_settings();
    settings.models[0].supports_image = true;
    storage.save_model_settings(settings).unwrap();

    let prepared = prepare_conversation_turn(
        &storage,
        &SkillsService::new(),
        AgentConversationTurnInput {
            conversation_id: Some("conversation-model-capabilities".to_string()),
            project_id: None,
            model_id: "model-1".to_string(),
            context_window_indicator_enabled: true,
            content: "Inspect an image later".to_string(),
            attachments: Vec::new(),
            skills: Vec::new(),
            title: None,
            user_message_id: Some("user-model-capabilities".to_string()),
            assistant_message_id: Some("assistant-model-capabilities".to_string()),
            max_tokens: None,
            temperature: None,
            prompt_preferences: None,
            permissions: AgentPermissions::default(),
        },
        "run-model-capabilities",
    )
    .unwrap();

    assert!(prepared.agent_input.model_capabilities.image_input);
}

#[test]
fn ordinary_turn_preserves_a_blocked_goal_without_implicit_resume() {
    let fixture = tempdir().unwrap();
    let storage = StorageService::open(&fixture.path().join("storage.sqlite")).unwrap();
    storage.save_model_settings(test_model_settings()).unwrap();

    let first = prepare_conversation_turn(
        &storage,
        &SkillsService::new(),
        AgentConversationTurnInput {
            conversation_id: Some("conversation-explicit-goal".to_string()),
            project_id: None,
            model_id: "model-1".to_string(),
            context_window_indicator_enabled: true,
            content: "An ordinary request".to_string(),
            attachments: Vec::new(),
            skills: Vec::new(),
            title: None,
            user_message_id: Some("user-explicit-goal-1".to_string()),
            assistant_message_id: Some("assistant-explicit-goal-1".to_string()),
            max_tokens: None,
            temperature: None,
            prompt_preferences: None,
            permissions: AgentPermissions::default(),
        },
        "run-explicit-goal-1",
    )
    .unwrap();

    assert!(first.agent_input.goal.is_none());
    storage
        .create_conversation_goal(
            mycopilot_core::ConversationGoalMutationActor::User,
            "conversation-explicit-goal",
            "Track this objective across user turns.",
            2,
        )
        .unwrap();
    storage
        .update_conversation_goal_status(
            mycopilot_core::ConversationGoalMutationActor::Model,
            "conversation-explicit-goal",
            mycopilot_core::ConversationGoalStatus::Blocked,
            Some("Waiting for the next user turn."),
            3,
        )
        .unwrap();
    storage
        .update_conversation_goal_objective(
            mycopilot_core::ConversationGoalMutationActor::User,
            "conversation-explicit-goal",
            "Track the revised objective across user turns.",
            4,
        )
        .unwrap();

    let second = prepare_conversation_turn(
        &storage,
        &SkillsService::new(),
        AgentConversationTurnInput {
            conversation_id: Some("conversation-explicit-goal".to_string()),
            project_id: None,
            model_id: "model-1".to_string(),
            context_window_indicator_enabled: true,
            content: "Continue now".to_string(),
            attachments: Vec::new(),
            skills: Vec::new(),
            title: None,
            user_message_id: Some("user-explicit-goal-2".to_string()),
            assistant_message_id: Some("assistant-explicit-goal-2".to_string()),
            max_tokens: None,
            temperature: None,
            prompt_preferences: None,
            permissions: AgentPermissions::default(),
        },
        "run-explicit-goal-2",
    )
    .unwrap();

    let goal = second.agent_input.goal.unwrap();
    assert_eq!(
        goal.objective,
        "Track the revised objective across user turns."
    );
    assert_eq!(goal.status, mycopilot_core::ConversationGoalStatus::Blocked);
    assert_eq!(
        goal.stopped_reason.as_deref(),
        Some("Waiting for the next user turn.")
    );
}

#[test]
fn conversation_world_state_persists_exact_full_and_anchored_diff_across_turns() {
    let fixture = tempdir().unwrap();
    let storage = StorageService::open(&fixture.path().join("storage.sqlite")).unwrap();
    storage.save_model_settings(test_model_settings()).unwrap();

    let first = prepare_conversation_turn(
        &storage,
        &SkillsService::new(),
        AgentConversationTurnInput {
            conversation_id: Some("conversation-world-state".to_string()),
            project_id: None,
            model_id: "model-1".to_string(),
            context_window_indicator_enabled: true,
            content: "First turn".to_string(),
            attachments: Vec::new(),
            skills: Vec::new(),
            title: None,
            user_message_id: Some("user-world-state-1".to_string()),
            assistant_message_id: Some("assistant-world-state-1".to_string()),
            max_tokens: None,
            temperature: None,
            prompt_preferences: None,
            permissions: AgentPermissions::default(),
        },
        "run-world-state-1",
    )
    .unwrap();
    assert_eq!(first.agent_input.world_state_records.len(), 1);
    let mycopilot_core::WorldStateRecord::Full(first_snapshot) =
        &first.agent_input.world_state_records[0].record
    else {
        panic!("first World State record must be full");
    };
    assert_eq!(first_snapshot.sequence, 0);
    assert!(first.agent_input.world_state_records[0]
        .effective_before_message_id
        .is_none());
    let first_projection = first_snapshot
        .model_projection(mycopilot_core::WorldStateLifetime::Conversation)
        .unwrap()
        .render_sanitized_text();
    assert!(first_projection.contains("\"id\":\"environment\""));
    assert!(first_projection.contains("\"os\""));
    assert!(first_projection.contains("\"network\""));
    assert!(!first_projection.contains("\"managedOffice\""));
    assert!(!first_projection.contains("\"shellCommand\""));

    let second = prepare_conversation_turn(
        &storage,
        &SkillsService::new(),
        AgentConversationTurnInput {
            conversation_id: Some("conversation-world-state".to_string()),
            project_id: None,
            model_id: "model-1".to_string(),
            context_window_indicator_enabled: true,
            content: "Second turn".to_string(),
            attachments: Vec::new(),
            skills: Vec::new(),
            title: None,
            user_message_id: Some("user-world-state-2".to_string()),
            assistant_message_id: Some("assistant-world-state-2".to_string()),
            max_tokens: None,
            temperature: None,
            prompt_preferences: Some(mycopilot_core::AgentPromptPreferences {
                work_mode: Some(mycopilot_core::AgentPromptWorkMode::General),
                tone: Some(mycopilot_core::AgentPromptTone::Friendly),
                detail_level: Some(mycopilot_core::AgentPromptDetailLevel::High),
                custom_instructions: None,
                updated_at: Some(42),
            }),
            permissions: AgentPermissions {
                write: AgentWritePermission::All,
                ..AgentPermissions::default()
            },
        },
        "run-world-state-2",
    )
    .unwrap();

    assert_eq!(second.agent_input.world_state_records.len(), 2);
    assert_eq!(
        second.agent_input.world_state_records[1]
            .effective_before_message_id
            .as_deref(),
        Some("user-world-state-2")
    );
    let mycopilot_core::WorldStateRecord::Diff(diff) =
        &second.agent_input.world_state_records[1].record
    else {
        panic!("changed World State must append a diff");
    };
    let rendered = diff
        .model_projection_against(
            first_snapshot,
            mycopilot_core::WorldStateLifetime::Conversation,
        )
        .unwrap()
        .unwrap()
        .render_sanitized_text();
    assert!(rendered.contains("\"write\":\"all\""));
    assert!(rendered.contains("\"workMode\":\"general\""));
    assert!(rendered.contains("\"tone\":\"friendly\""));
    assert!(!rendered.contains("imageInput"));
    assert!(!rendered.contains("updatedAt"));

    let stored = load_conversation_world_state(&storage, "conversation-world-state").unwrap();
    assert_eq!(stored, second.agent_input.world_state_records);
}

#[test]
fn compaction_accepts_newly_closed_exchange_but_rejects_unsafe_trace_boundaries() {
    let trace = ConversationTurnTrace {
        schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
        run_id: "run-visible-boundary".to_string(),
        conversation_id: "conversation-visible-boundary".to_string(),
        assistant_message_id: "assistant-visible-boundary".to_string(),
        terminal_status: ConversationTurnTraceTerminalStatus::InProgress,
        terminal_error: None,
        truncated: false,
        items: vec![
            ConversationTurnTraceItem::AssistantNarration {
                sequence: 0,
                content: "I will read the file.".to_string(),
                truncated: false,
            },
            ConversationTurnTraceItem::ToolCall {
                sequence: 1,
                call_id: "read-visible-boundary".to_string(),
                tool: "read_file".to_string(),
                operation: json!({ "path": "README.md" }),
                approval_status: AgentApprovalStatus::NotRequired,
                truncated: false,
            },
            ConversationTurnTraceItem::ToolResult {
                sequence: 2,
                call_id: "read-visible-boundary".to_string(),
                tool: "read_file".to_string(),
                status: ConversationTraceToolResultStatus::Succeeded,
                success: true,
                observation: json!({ "content": "contents" }),
                approval_status: AgentApprovalStatus::NotRequired,
                error: None,
                truncated: false,
                archive: Default::default(),
            },
        ],
    };
    let result_cursor = ContextJournalCursor::trace_item("assistant-visible-boundary", 2);

    validate_compaction_trace_boundary(
        &result_cursor,
        Some(&trace),
        "run-visible-boundary",
        "conversation-visible-boundary",
        "assistant-visible-boundary",
    )
    .unwrap();

    let split_exchange = validate_compaction_trace_boundary(
        &ContextJournalCursor::trace_item("assistant-visible-boundary", 1),
        Some(&trace),
        "run-visible-boundary",
        "conversation-visible-boundary",
        "assistant-visible-boundary",
    )
    .unwrap_err();
    assert_eq!(
        split_exchange.code(),
        Some("context_compaction_trace_boundary_missing")
    );
}

#[test]
fn production_compaction_services_install_the_current_model_generator() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let service = AgentService::new(storage);
    let agent_input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "secret",
        "model": "model-1",
        "apiStyle": "open_ai_compatible",
        "contextWindowTokens": 128000,
        "maxTokens": 4000,
        "messages": []
    }))
    .unwrap();
    let (notifications, _receiver) = tokio::sync::mpsc::unbounded_channel();

    let _services = service.context_compaction_services(
        "run-production-generator",
        "conversation-production-generator",
        "assistant-production-generator",
        agent_input,
        RunContextToolProjection::pending(),
        notifications,
    );
}

#[test]
fn next_turn_loads_backend_trace_and_never_parses_agent_run_json() {
    let fixture = tempdir().unwrap();
    let storage = StorageService::open(&fixture.path().join("storage.sqlite")).unwrap();
    storage.save_model_settings(test_model_settings()).unwrap();
    storage
        .save_conversation(ChatConversationRecord {
            id: "conversation-history".to_string(),
            project_id: None,
            model_id: Some("model-1".to_string()),
            title: "Trace history".to_string(),
            messages: vec![
                ChatMessageRecord {
                    id: "user-history".to_string(),
                    role: "user".to_string(),
                    content: "Create the file".to_string(),
                    created_at: 1,
                    status: Some("sent".to_string()),
                    attachments: Vec::new(),
                    agent_run_json: None,
                    ui_state_json: None,
                },
                ChatMessageRecord {
                    id: "assistant-history".to_string(),
                    role: "assistant".to_string(),
                    content: "Created src/history.rs.".to_string(),
                    created_at: 2,
                    status: Some("sent".to_string()),
                    attachments: Vec::new(),
                    agent_run_json: Some(
                        json!({
                            "timeline": [{ "content": "FRONTEND_TIMELINE_MUST_NOT_ENTER_CONTEXT" }],
                            "toolCalls": [{ "args": { "filePath": "frontend/fake.rs" } }]
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
    let trace = completed_trace("conversation-history", "assistant-history");
    storage
        .replace_conversation_turn_trace(&trace, 1, 2)
        .unwrap();

    let prepared = prepare_conversation_turn(
        &storage,
        &SkillsService::new(),
        AgentConversationTurnInput {
            conversation_id: Some("conversation-history".to_string()),
            project_id: None,
            model_id: "model-1".to_string(),
            context_window_indicator_enabled: true,
            content: "What changed?".to_string(),
            attachments: Vec::new(),
            skills: Vec::new(),
            title: None,
            user_message_id: Some("user-next".to_string()),
            assistant_message_id: Some("assistant-next".to_string()),
            max_tokens: None,
            temperature: None,
            prompt_preferences: None,
            permissions: AgentPermissions::default(),
        },
        "run-next",
    )
    .unwrap();

    assert_eq!(prepared.agent_input.messages.len(), 3);
    assert_eq!(prepared.agent_input.messages[0].content, "Create the file");
    assert_eq!(prepared.agent_input.messages[0].created_at, Some(1));
    assert_eq!(
        prepared.agent_input.messages[1]
            .conversation_turn_trace
            .as_ref(),
        Some(&trace)
    );
    assert_eq!(
        prepared.agent_input.messages[1].content,
        "Created src/history.rs."
    );
    assert_eq!(prepared.agent_input.messages[1].created_at, Some(2));
    assert_eq!(prepared.agent_input.messages[2].content, "What changed?");
    assert!(prepared.agent_input.messages[2].created_at.is_some());
    let serialized = serde_json::to_string(&prepared.agent_input.messages).unwrap();
    assert!(serialized.contains("src/history.rs"));
    assert!(!serialized.contains("FRONTEND_TIMELINE_MUST_NOT_ENTER_CONTEXT"));
    assert!(!serialized.contains("frontend/fake.rs"));
}

#[test]
fn next_turn_loads_active_summary_and_only_the_uncovered_tail() {
    let fixture = tempdir().unwrap();
    let storage = StorageService::open(&fixture.path().join("storage.sqlite")).unwrap();
    storage.save_model_settings(test_model_settings()).unwrap();
    storage
        .save_conversation(ChatConversationRecord {
            id: "conversation-summary".to_string(),
            project_id: None,
            model_id: Some("model-1".to_string()),
            title: "Summary history".to_string(),
            messages: vec![
                ChatMessageRecord {
                    id: "user-old".to_string(),
                    role: "user".to_string(),
                    content: "Old request".to_string(),
                    created_at: 1,
                    status: Some("sent".to_string()),
                    attachments: Vec::new(),
                    agent_run_json: None,
                    ui_state_json: None,
                },
                ChatMessageRecord {
                    id: "assistant-old".to_string(),
                    role: "assistant".to_string(),
                    content: "Old answer".to_string(),
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
    let prefix = storage
        .prepare_context_compaction_prefix(
            "conversation-summary",
            &ContextJournalCursor::message("assistant-old"),
        )
        .unwrap();
    storage
        .commit_context_compaction_prefix(
            &prefix,
            test_compaction_draft(
                &prefix,
                "summary-active",
                "The old request was completed.",
                100,
                3,
            ),
            "assistant-old",
        )
        .unwrap();

    let prepared = prepare_conversation_turn(
        &storage,
        &SkillsService::new(),
        AgentConversationTurnInput {
            conversation_id: Some("conversation-summary".to_string()),
            project_id: None,
            model_id: "model-1".to_string(),
            context_window_indicator_enabled: true,
            content: "Continue".to_string(),
            attachments: Vec::new(),
            skills: Vec::new(),
            title: None,
            user_message_id: Some("user-next".to_string()),
            assistant_message_id: Some("assistant-next".to_string()),
            max_tokens: None,
            temperature: None,
            prompt_preferences: None,
            permissions: AgentPermissions::default(),
        },
        "run-next",
    )
    .unwrap();

    assert_eq!(prepared.agent_input.messages.len(), 1);
    assert_eq!(prepared.agent_input.messages[0].content, "Continue");
    assert_eq!(
        prepared
            .agent_input
            .context_compaction_summary
            .as_ref()
            .map(|summary| summary.id.as_str()),
        Some("summary-active")
    );
}

#[test]
fn legacy_messages_without_trace_keep_final_text_and_legacy_errors_stay_excluded() {
    let conversation = ChatConversationRecord {
        id: "conversation-legacy".to_string(),
        project_id: None,
        model_id: None,
        title: "Legacy".to_string(),
        messages: vec![
            ChatMessageRecord {
                id: "assistant-legacy".to_string(),
                role: "assistant".to_string(),
                content: "Legacy final answer".to_string(),
                created_at: 1,
                status: Some("sent".to_string()),
                attachments: Vec::new(),
                agent_run_json: Some("not valid json".to_string()),
                ui_state_json: None,
            },
            ChatMessageRecord {
                id: "assistant-error".to_string(),
                role: "assistant".to_string(),
                content: "Legacy error".to_string(),
                created_at: 2,
                status: Some("error".to_string()),
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
    };

    let history = conversation_history_messages(&conversation, &[], &[]);

    assert_eq!(history.len(), 1);
    assert_eq!(history[0].content, "Legacy final answer");
    assert!(history[0].conversation_turn_trace.is_none());

    let mut failed_trace = completed_trace("conversation-legacy", "assistant-error");
    failed_trace.terminal_status = ConversationTurnTraceTerminalStatus::Failed;
    failed_trace.terminal_error = Some("permission denied".to_string());
    let history_with_trace =
        conversation_history_messages(&conversation, &[failed_trace.clone()], &[]);
    assert_eq!(history_with_trace.len(), 2);
    assert_eq!(
        history_with_trace[1].conversation_turn_trace.as_ref(),
        Some(&failed_trace)
    );
}

#[test]
fn next_turn_keeps_committed_prefix_from_an_interrupted_pending_run() {
    let mut trace = completed_trace("conversation-interrupted", "assistant-interrupted");
    trace.terminal_status = ConversationTurnTraceTerminalStatus::InProgress;
    let conversation = ChatConversationRecord {
        id: "conversation-interrupted".to_string(),
        project_id: None,
        model_id: None,
        title: "Interrupted".to_string(),
        messages: vec![ChatMessageRecord {
            id: "assistant-interrupted".to_string(),
            role: "assistant".to_string(),
            content: "duplicated pending narration".to_string(),
            created_at: 1,
            status: Some("pending".to_string()),
            attachments: Vec::new(),
            agent_run_json: None,
            ui_state_json: None,
        }],
        created_at: 1,
        updated_at: 1,
        pinned_at: None,
        archived_at: None,
        unread_at: None,
    };

    let history = conversation_history_messages(&conversation, &[trace.clone()], &[]);

    assert_eq!(history.len(), 1);
    assert!(history[0].content.is_empty());
    assert_eq!(history[0].conversation_turn_trace.as_ref(), Some(&trace));
}

#[test]
fn next_turn_carries_the_uncompressed_model_projection_beside_the_durable_trace() {
    let trace = completed_trace("conversation-exact-history", "assistant-exact-history");
    let model_items = vec![
        mycopilot_core::ConversationModelContextItem {
            sequence: 0,
            ordinal: 0,
            role: "assistant".to_string(),
            content: "I am creating the requested file.".to_string(),
            tool_call_id: None,
            tool_calls: Vec::new(),
            is_error: false,
        },
        mycopilot_core::ConversationModelContextItem {
            sequence: 1,
            ordinal: 0,
            role: "assistant".to_string(),
            content: String::new(),
            tool_call_id: None,
            tool_calls: vec![mycopilot_core::AgentContextCheckpointToolCall {
                id: "write-history".to_string(),
                name: "write_file".to_string(),
                args: json!({
                    "filePath": "src/history.rs",
                    "mode": "create",
                    "content": "EXACT_WRITE_CONTENT"
                }),
            }],
            is_error: false,
        },
        mycopilot_core::ConversationModelContextItem {
            sequence: 2,
            ordinal: 0,
            role: "tool".to_string(),
            content: r#"{"ok":true,"result":{"exact":"EXACT_TOOL_RESULT"}}"#.to_string(),
            tool_call_id: Some("write-history".to_string()),
            tool_calls: Vec::new(),
            is_error: false,
        },
    ];
    let conversation = ChatConversationRecord {
        id: "conversation-exact-history".to_string(),
        project_id: None,
        model_id: None,
        title: "Exact history".to_string(),
        messages: vec![ChatMessageRecord {
            id: "assistant-exact-history".to_string(),
            role: "assistant".to_string(),
            content: "Final answer".to_string(),
            created_at: 1,
            status: Some("sent".to_string()),
            attachments: Vec::new(),
            agent_run_json: None,
            ui_state_json: None,
        }],
        created_at: 1,
        updated_at: 1,
        pinned_at: None,
        archived_at: None,
        unread_at: None,
    };
    let logs = vec![mycopilot_core::ConversationModelContextLog {
        assistant_message_id: "assistant-exact-history".to_string(),
        items: model_items.clone(),
    }];

    let history =
        conversation_history_messages_with_model_context(&conversation, &[trace], &logs, None, &[]);

    assert_eq!(history.len(), 1);
    assert_eq!(history[0].conversation_model_context_items, model_items);
    assert!(history[0].conversation_turn_trace.is_some());
}

#[test]
fn compaction_projection_hides_covered_prefix_but_keeps_raw_conversation_intact() {
    let conversation = ChatConversationRecord {
        id: "conversation-compacted".to_string(),
        project_id: None,
        model_id: None,
        title: "Compacted".to_string(),
        messages: vec![
            ChatMessageRecord {
                id: "user-old".to_string(),
                role: "user".to_string(),
                content: "old request".to_string(),
                created_at: 1,
                status: Some("sent".to_string()),
                attachments: Vec::new(),
                agent_run_json: None,
                ui_state_json: None,
            },
            ChatMessageRecord {
                id: "assistant-old".to_string(),
                role: "assistant".to_string(),
                content: "old answer".to_string(),
                created_at: 2,
                status: Some("sent".to_string()),
                attachments: Vec::new(),
                agent_run_json: None,
                ui_state_json: None,
            },
            ChatMessageRecord {
                id: "user-tail".to_string(),
                role: "user".to_string(),
                content: "new request".to_string(),
                created_at: 3,
                status: Some("sent".to_string()),
                attachments: Vec::new(),
                agent_run_json: None,
                ui_state_json: None,
            },
        ],
        created_at: 1,
        updated_at: 3,
        pinned_at: None,
        archived_at: None,
        unread_at: None,
    };
    let summary_prefix = ContextCompactionPrefix {
        conversation_id: conversation.id.clone(),
        source_revision: "revision-1".to_string(),
        covered_through: ContextJournalCursor::message("assistant-old"),
        previous_summary: None,
        source_items: vec![
            ContextCompactionSourceItem::Message {
                cursor: ContextJournalCursor::message("user-old"),
                role: "user".to_string(),
                content: "old request".to_string(),
                created_at: 1,
                status: Some("sent".to_string()),
                terminal_status: None,
                terminal_error: None,
            },
            ContextCompactionSourceItem::Message {
                cursor: ContextJournalCursor::message("assistant-old"),
                role: "assistant".to_string(),
                content: "old answer".to_string(),
                created_at: 2,
                status: Some("sent".to_string()),
                terminal_status: None,
                terminal_error: None,
            },
        ],
    };
    let summary = ContextCompactionSummary {
        schema_version: CONTEXT_COMPACTION_SUMMARY_SCHEMA_VERSION,
        id: "summary-1".to_string(),
        conversation_id: conversation.id.clone(),
        source_revision: summary_prefix.source_revision.clone(),
        previous_summary_id: None,
        covered_through: summary_prefix.covered_through.clone(),
        content: "old turn summary".to_string(),
        continuity: mycopilot_core::ContextContinuitySnapshot::from_prefix(&summary_prefix)
            .unwrap(),
        generation: ContextCompactionGeneration::test(),
        source_input_tokens: 100,
        summary_input_tokens: 10,
        continuity_input_tokens: 10,
        uncovered_tail_input_tokens: 0,
        replacement_input_tokens: 20,
        created_at: 4,
    };

    let projected =
        conversation_history_messages_with_compaction(&conversation, &[], Some(&summary), &[]);

    assert_eq!(conversation.messages.len(), 3);
    assert_eq!(projected.len(), 1);
    assert_eq!(projected[0].content, "new request");
}

#[test]
fn mid_run_projection_keeps_latest_user_exact_and_only_the_uncovered_trace_tail() {
    let conversation = ChatConversationRecord {
        id: "conversation-mid-run".to_string(),
        project_id: None,
        model_id: None,
        title: "Mid run".to_string(),
        messages: vec![
            ChatMessageRecord {
                id: "user-current".to_string(),
                role: "user".to_string(),
                content: "LATEST_USER_MARKER".to_string(),
                created_at: 1,
                status: Some("sent".to_string()),
                attachments: Vec::new(),
                agent_run_json: None,
                ui_state_json: None,
            },
            ChatMessageRecord {
                id: "assistant-current".to_string(),
                role: "assistant".to_string(),
                content: THINKING_PLACEHOLDER.to_string(),
                created_at: 2,
                status: Some("pending".to_string()),
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
    };
    let trace = ConversationTurnTrace {
        schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
        run_id: "run-mid-run".to_string(),
        conversation_id: conversation.id.clone(),
        assistant_message_id: "assistant-current".to_string(),
        terminal_status: ConversationTurnTraceTerminalStatus::InProgress,
        terminal_error: None,
        truncated: false,
        items: vec![
            ConversationTurnTraceItem::ToolCall {
                sequence: 1,
                call_id: "call-1".to_string(),
                tool: "read_file".to_string(),
                operation: json!({ "path": "README.md" }),
                approval_status: AgentApprovalStatus::NotRequired,
                truncated: false,
            },
            ConversationTurnTraceItem::ToolResult {
                sequence: 2,
                call_id: "call-1".to_string(),
                tool: "read_file".to_string(),
                status: ConversationTraceToolResultStatus::Succeeded,
                success: true,
                observation: json!({ "content": "COVERED_RESULT" }),
                approval_status: AgentApprovalStatus::NotRequired,
                error: None,
                truncated: false,
                archive: Default::default(),
            },
            ConversationTurnTraceItem::AssistantNarration {
                sequence: 3,
                content: "UNCOVERED_TAIL_MARKER".to_string(),
                truncated: false,
            },
        ],
    };
    let seed_prefix = ContextCompactionPrefix {
        conversation_id: conversation.id.clone(),
        source_revision: "revision-mid-run".to_string(),
        covered_through: ContextJournalCursor::message("user-current"),
        previous_summary: None,
        source_items: vec![ContextCompactionSourceItem::Message {
            cursor: ContextJournalCursor::message("user-current"),
            role: "user".to_string(),
            content: "LATEST_USER_MARKER".to_string(),
            created_at: 1,
            status: Some("sent".to_string()),
            terminal_status: None,
            terminal_error: None,
        }],
    };
    let summary = ContextCompactionSummary {
        schema_version: CONTEXT_COMPACTION_SUMMARY_SCHEMA_VERSION,
        id: "summary-mid-run".to_string(),
        conversation_id: conversation.id.clone(),
        source_revision: seed_prefix.source_revision.clone(),
        previous_summary_id: None,
        covered_through: ContextJournalCursor::trace_item("assistant-current", 2),
        content: "The file was read.".to_string(),
        continuity: mycopilot_core::ContextContinuitySnapshot::from_prefix(&seed_prefix).unwrap(),
        generation: ContextCompactionGeneration::test(),
        source_input_tokens: 100,
        summary_input_tokens: 10,
        continuity_input_tokens: 10,
        uncovered_tail_input_tokens: 10,
        replacement_input_tokens: 20,
        created_at: 3,
    };

    let projected = conversation_history_messages_with_model_context(
        &conversation,
        &[trace],
        &[],
        Some(&summary),
        &[],
    );

    assert_eq!(projected.len(), 2);
    assert_eq!(projected[0].content, "LATEST_USER_MARKER");
    let tail = projected[1].conversation_turn_trace.as_ref().unwrap();
    assert_eq!(tail.items.len(), 1);
    assert_eq!(tail.items[0].sequence(), 3);
}

#[test]
fn context_window_snapshot_is_zero_until_first_user_message_then_counts_complete_request() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    storage
        .save_model_settings(ModelSettingsRecord {
            api_url: "https://example.test/v1/chat/completions".to_string(),
            api_token: "token".to_string(),
            search_mode: "disabled".to_string(),
            tavily_api_key: String::new(),
            models: vec![ModelConfigRecord {
                id: "model-1".to_string(),
                display_name: "Model 1".to_string(),
                api_url_override: None,
                api_token_override: None,
                supports_image: false,
                context_window_tokens: Some(128_000),
                input_price: "0.01".to_string(),
                output_price: "0.02".to_string(),
                enabled: true,
            }],
        })
        .unwrap();
    let service = AgentService::new(storage.clone());
    let enabled = service
        .get_context_window_snapshot(AgentContextWindowSnapshotInput {
            conversation_id: None,
            project_id: None,
            model_id: "model-1".to_string(),
            max_tokens: Some(30_000),
            prompt_preferences: None,
            permissions: AgentPermissions::default(),
            skills: Vec::new(),
        })
        .unwrap()
        .snapshot
        .unwrap();

    assert_eq!(enabled.model, "model-1");
    assert_eq!(enabled.context_window_tokens, Some(128_000));
    assert!(enabled.input_capacity_tokens.is_some_and(|value| value > 0));
    assert_eq!(
        enabled.input_tokens, 0,
        "an unstarted composer must publish zero even though its model contract can be previewed"
    );
    assert_eq!(
        enabled.cost_breakdown.total_input_tokens, 0,
        "an unstarted conversation has no actual model request to present"
    );
    assert_eq!(enabled.cost_breakdown.system_tokens, 0);
    assert_eq!(enabled.cost_breakdown.tool_schema_tokens, 0);

    storage
        .save_conversation(ChatConversationRecord {
            id: "conversation-started-window".to_string(),
            project_id: None,
            model_id: Some("model-1".to_string()),
            title: "Started window".to_string(),
            messages: vec![ChatMessageRecord {
                id: "user-started-window".to_string(),
                role: "user".to_string(),
                content: "你好".to_string(),
                created_at: 1,
                status: Some("sent".to_string()),
                attachments: Vec::new(),
                agent_run_json: None,
                ui_state_json: None,
            }],
            created_at: 1,
            updated_at: 1,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
    let started = service
        .get_context_window_snapshot(AgentContextWindowSnapshotInput {
            conversation_id: Some("conversation-started-window".to_string()),
            project_id: None,
            model_id: "model-1".to_string(),
            max_tokens: Some(30_000),
            prompt_preferences: None,
            permissions: AgentPermissions::default(),
            skills: Vec::new(),
        })
        .unwrap()
        .snapshot
        .unwrap();

    assert!(started.input_tokens > 0);
    assert_eq!(
        started.input_tokens, started.cost_breakdown.total_input_tokens,
        "the first user message starts full-request accounting, including fixed contracts"
    );
    assert!(
        started.cost_breakdown.system_tokens > 0 && started.cost_breakdown.tool_schema_tokens > 0
    );
}

#[test]
fn cached_context_preview_measures_skill_without_polluting_durable_revision() {
    let fixture = tempdir().unwrap();
    let workspace = fixture.path().join("workspace");
    write_test_skill(&workspace, "SKILL_PREVIEW_MARKER: verify the repository.");
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    storage.save_model_settings(test_model_settings()).unwrap();
    storage
        .save_project(ProjectRecord {
            id: "project-preview-skill".to_string(),
            name: "Preview workspace".to_string(),
            path: Some(workspace.to_string_lossy().into_owned()),
            created_at: 1,
            pinned_at: None,
        })
        .unwrap();
    storage
        .save_conversation(ChatConversationRecord {
            id: "conversation-preview-skill".to_string(),
            project_id: Some("project-preview-skill".to_string()),
            model_id: Some("model-1".to_string()),
            title: "Preview".to_string(),
            messages: vec![ChatMessageRecord {
                id: "user-preview-skill".to_string(),
                role: "user".to_string(),
                content: "Review the repository.".to_string(),
                created_at: 1,
                status: Some("sent".to_string()),
                attachments: Vec::new(),
                agent_run_json: None,
                ui_state_json: None,
            }],
            created_at: 1,
            updated_at: 1,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
    let skills = Arc::new(SkillsService::new());
    let catalog = skills
        .list_workspace("project-preview-skill", &workspace)
        .unwrap();
    let descriptor = catalog.skills().first().unwrap();
    let service = AgentService::new(Arc::clone(&storage)).with_skills_service(Arc::clone(&skills));
    let base_input = AgentContextWindowSnapshotInput {
        conversation_id: Some("conversation-preview-skill".to_string()),
        project_id: Some("project-preview-skill".to_string()),
        model_id: "model-1".to_string(),
        max_tokens: Some(30_000),
        prompt_preferences: None,
        permissions: AgentPermissions::default(),
        skills: Vec::new(),
    };
    let plain = service
        .get_context_window_snapshot(base_input.clone())
        .unwrap()
        .snapshot
        .unwrap();
    let selected = service
        .get_context_window_snapshot(AgentContextWindowSnapshotInput {
            skills: vec![mycopilot_protocol_rs::SkillSelectionDto {
                id: descriptor.id().as_str().to_string(),
                revision: descriptor.revision().as_str().to_string(),
            }],
            ..base_input
        })
        .unwrap()
        .snapshot
        .unwrap();

    assert!(selected.input_tokens > plain.input_tokens);
}

#[test]
fn committed_test_summary_rebuilds_the_shared_durable_snapshot() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    storage.save_model_settings(test_model_settings()).unwrap();
    storage
        .save_conversation(ChatConversationRecord {
            id: "conversation-capacity-summary".to_string(),
            project_id: None,
            model_id: Some("model-1".to_string()),
            title: "Capacity summary".to_string(),
            messages: vec![
                ChatMessageRecord {
                    id: "user-long".to_string(),
                    role: "user".to_string(),
                    content: "u".repeat(12_000),
                    created_at: 1,
                    status: Some("sent".to_string()),
                    attachments: Vec::new(),
                    agent_run_json: None,
                    ui_state_json: None,
                },
                ChatMessageRecord {
                    id: "assistant-long".to_string(),
                    role: "assistant".to_string(),
                    content: "a".repeat(12_000),
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
    let service = AgentService::new(storage.clone());
    let snapshot_input = AgentContextWindowSnapshotInput {
        conversation_id: Some("conversation-capacity-summary".to_string()),
        project_id: None,
        model_id: "model-1".to_string(),
        max_tokens: Some(30_000),
        prompt_preferences: None,
        permissions: AgentPermissions::default(),
        skills: Vec::new(),
    };
    let before = service
        .get_context_window_snapshot(snapshot_input.clone())
        .unwrap()
        .snapshot
        .unwrap();

    let prefix = storage
        .prepare_context_compaction_prefix(
            "conversation-capacity-summary",
            &ContextJournalCursor::message("assistant-long"),
        )
        .unwrap();
    storage
        .commit_context_compaction_prefix(
            &prefix,
            test_compaction_draft(
                &prefix,
                "summary-capacity",
                "The prior request was completed.",
                before.input_tokens,
                3,
            ),
            "assistant-long",
        )
        .unwrap();
    service.invalidate_conversation_context_state("conversation-capacity-summary");

    let after = service
        .get_context_window_snapshot(snapshot_input)
        .unwrap()
        .snapshot
        .unwrap();
    assert!(after.input_tokens < before.input_tokens);
    assert_eq!(
        storage
            .load_conversation("conversation-capacity-summary")
            .unwrap()
            .unwrap()
            .messages
            .len(),
        2
    );
}
