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

/// Exercise the same bound Host port as sampling, independently of the model transport.
fn commit_prepared_world_state(
    service: &AgentService,
    prepared: &mut crate::application::agent_support::PreparedConversationTurn,
) {
    let output = &prepared.output;
    let before = load_conversation_world_state(&service.storage, &output.conversation_id).unwrap();
    assert_eq!(
        prepared.agent_input.world_state_records, before,
        "admission only loads history"
    );
    let mut trace = mycopilot_core::ConversationTraceSnapshot::default().in_progress_trace(
        &output.run_id,
        &output.conversation_id,
        &output.assistant_message_id,
    );
    service
        .storage
        .append_in_progress_conversation_turn_trace(&trace, 1, 1)
        .unwrap();
    let boundary = mycopilot_core::WorldStateRequestBoundary {
        run_id: output.run_id.clone(),
        assistant_message_id: output.assistant_message_id.clone(),
        request_index: 1,
        after_trace_sequence: None,
    };
    let host = service.conversation_world_state_host(
        &prepared.agent_input,
        &output.run_id,
        &output.conversation_id,
        &output.assistant_message_id,
        &AgentCancellationToken::new(),
    );
    host.prepare_request(mycopilot_core::AgentConversationWorldStateRequest {
        conversation_id: output.conversation_id.clone(),
        boundary: boundary.clone(),
        sections: Vec::new(),
    })
    .unwrap();
    host.mark_request_observed(&boundary).unwrap();
    prepared.agent_input.world_state_records =
        load_conversation_world_state(&service.storage, &output.conversation_id).unwrap();
    trace.terminal_status = ConversationTurnTraceTerminalStatus::Completed;
    service
        .storage
        .replace_conversation_turn_trace(&trace, 1, 2)
        .unwrap();
}

#[test]
fn conversation_world_state_persists_exact_full_and_anchored_diff_across_turns() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let service = AgentService::new(storage.clone());
    let mut settings = test_model_settings();
    settings.models[0].provider_model_id = "provider-model-1".to_string();
    storage.save_model_settings(settings).unwrap();

    let mut first = prepare_conversation_turn(
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
    assert!(first.agent_input.world_state_records.is_empty());
    commit_prepared_world_state(&service, &mut first);
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
    assert!(first_projection.contains("\"id\":\"model.selection\""));
    assert!(first_projection.contains("\"configuredModelId\":\"provider-model-1\""));
    assert!(!first_projection.contains("\"configuredModelId\":\"model-1\""));
    assert!(first_projection.contains("\"imageInput\":false"));
    assert!(first_projection.contains("\"os\""));
    assert!(first_projection.contains("\"network\""));
    assert!(!first_projection.contains("\"managedOffice\""));
    assert!(!first_projection.contains("\"shellCommand\""));

    let mut second = prepare_conversation_turn(
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
                context_profile: mycopilot_core::AgentContextProfile::Full,
                work_mode: Some(mycopilot_core::AgentPromptWorkMode::General),
                tone: Some(mycopilot_core::AgentPromptTone::Friendly),
                detail_level: Some(mycopilot_core::AgentPromptDetailLevel::High),
                custom_instructions: None,
                updated_at: Some(42),
                automation_execution_context: None,
            }),
            permissions: AgentPermissions {
                write: AgentWritePermission::All,
                ..AgentPermissions::default()
            },
        },
        "run-world-state-2",
    )
    .unwrap();

    assert_eq!(
        second.agent_input.world_state_records,
        first.agent_input.world_state_records
    );
    commit_prepared_world_state(&service, &mut second);
    assert_eq!(second.agent_input.world_state_records.len(), 2);
    assert_eq!(
        second.agent_input.world_state_records[1]
            .request_boundary
            .as_ref(),
        Some(&mycopilot_core::WorldStateRequestBoundary {
            run_id: "run-world-state-2".into(),
            assistant_message_id: "assistant-world-state-2".into(),
            request_index: 1,
            after_trace_sequence: None,
        })
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
fn model_switch_appends_visible_selection_diffs_even_when_modalities_match() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let service = AgentService::new(storage.clone());
    let mut settings = test_model_settings();
    settings.models[0].provider_model_id = "provider-model-1".to_string();
    let mut alternate = settings.models[0].clone();
    alternate.id = "model-2".to_string();
    alternate.provider_model_id = "provider-model-2".to_string();
    alternate.display_name = "Model 2".to_string();
    let mut vision = settings.models[0].clone();
    vision.id = "model-vision".to_string();
    vision.provider_model_id = "provider-model-vision".to_string();
    vision.display_name = "Vision model".to_string();
    vision.supports_image = true;
    settings.models.extend([alternate, vision]);
    storage.save_model_settings(settings).unwrap();

    let prepare = |model_id: &str, user_id: &str, assistant_id: &str, run_id: &str| {
        let mut prepared = prepare_conversation_turn(
            &storage,
            &SkillsService::new(),
            AgentConversationTurnInput {
                conversation_id: Some("conversation-model-switch".to_string()),
                project_id: None,
                model_id: model_id.to_string(),
                context_window_indicator_enabled: true,
                content: format!("Use {model_id}"),
                attachments: Vec::new(),
                skills: Vec::new(),
                title: None,
                user_message_id: Some(user_id.to_string()),
                assistant_message_id: Some(assistant_id.to_string()),
                max_tokens: None,
                temperature: None,
                prompt_preferences: None,
                permissions: AgentPermissions::default(),
            },
            run_id,
        )
        .unwrap();
        commit_prepared_world_state(&service, &mut prepared);
        prepared
    };

    let first = prepare(
        "model-1",
        "user-model-1",
        "assistant-model-1",
        "run-model-1",
    );
    let mycopilot_core::WorldStateRecord::Full(first_snapshot) =
        &first.agent_input.world_state_records[0].record
    else {
        panic!("first model selection must establish a full snapshot");
    };

    let second = prepare(
        "model-2",
        "user-model-2",
        "assistant-model-2",
        "run-model-2",
    );
    let mycopilot_core::WorldStateRecord::Diff(second_diff) =
        &second.agent_input.world_state_records[1].record
    else {
        panic!("switching model identity must append a diff");
    };
    assert_eq!(
        second.agent_input.world_state_records[1]
            .request_boundary
            .as_ref(),
        Some(&mycopilot_core::WorldStateRequestBoundary {
            run_id: "run-model-2".into(),
            assistant_message_id: "assistant-model-2".into(),
            request_index: 1,
            after_trace_sequence: None,
        })
    );
    let second_projection = second_diff
        .model_projection_against(
            first_snapshot,
            mycopilot_core::WorldStateLifetime::Conversation,
        )
        .unwrap()
        .unwrap()
        .render_sanitized_text();
    assert!(second_projection.contains("\"configuredModelId\":\"provider-model-2\""));
    assert!(second_projection.contains("\"imageInput\":false"));

    let second_snapshot = mycopilot_core::WorldStateReducer::fold(
        first_snapshot.clone(),
        std::slice::from_ref(second_diff),
    )
    .unwrap();
    let third = prepare(
        "model-vision",
        "user-model-vision",
        "assistant-model-vision",
        "run-model-vision",
    );
    let mycopilot_core::WorldStateRecord::Diff(third_diff) =
        &third.agent_input.world_state_records[2].record
    else {
        panic!("switching image capability must append a diff");
    };
    assert_eq!(
        third.agent_input.world_state_records[2]
            .request_boundary
            .as_ref(),
        Some(&mycopilot_core::WorldStateRequestBoundary {
            run_id: "run-model-vision".into(),
            assistant_message_id: "assistant-model-vision".into(),
            request_index: 1,
            after_trace_sequence: None,
        })
    );
    let third_projection = third_diff
        .model_projection_against(
            &second_snapshot,
            mycopilot_core::WorldStateLifetime::Conversation,
        )
        .unwrap()
        .unwrap()
        .render_sanitized_text();
    assert!(third_projection.contains("\"configuredModelId\":\"provider-model-vision\""));
    assert!(third_projection.contains("\"imageInput\":true"));
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
                first_tool_call_id: None,
                provider_turn_id: None,
                sequence: 0,
                content: "I will read the file.".to_string(),
                truncated: false,
            },
            ConversationTurnTraceItem::ToolCall {
                sequence: 1,
                call_id: "read-visible-boundary".to_string(),
                tool: "read_file".to_string(),
                operation: json!({ "path": "README.md" }),
                provenance: AgentToolIdentity::Builtin {
                    tool_name: "read_file".to_string(),
                },
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
        "modelCapabilities": { "imageInput": false },
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
                    human_interaction_response: None,
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
                    human_interaction_response: None,
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
    let mut trace = completed_trace("conversation-history", "assistant-history", 18);
    trace.items.clear();
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
                    human_interaction_response: None,
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
                    human_interaction_response: None,
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
fn terminal_assistant_without_trace_is_rejected_and_current_failed_trace_is_accepted() {
    let conversation = ChatConversationRecord {
        id: "conversation-legacy".to_string(),
        project_id: None,
        model_id: None,
        title: "Legacy".to_string(),
        messages: vec![
            ChatMessageRecord {
                human_interaction_response: None,
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
                human_interaction_response: None,
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

    let error = conversation_history_messages(&conversation, &[], &[]).unwrap_err();
    assert!(error.contains("conversation_history_corrupt"));
    assert!(!error.contains("Legacy final answer"));

    let mut failed_trace = completed_trace("conversation-legacy", "assistant-error", 18);
    failed_trace.items.clear();
    failed_trace.terminal_status = ConversationTurnTraceTerminalStatus::Failed;
    failed_trace.terminal_error = Some("permission denied".to_string());
    let mut current_only = conversation.clone();
    current_only.messages.remove(0);
    let history_with_trace =
        conversation_history_messages(&current_only, &[failed_trace.clone()], &[]).unwrap();
    assert_eq!(history_with_trace.len(), 1);
    assert_eq!(
        history_with_trace[0].conversation_turn_trace.as_ref(),
        Some(&failed_trace)
    );
}

#[test]
fn next_turn_keeps_committed_prefix_from_an_interrupted_pending_run() {
    let mut trace = completed_trace("conversation-interrupted", "assistant-interrupted", 18);
    trace.items.clear();
    trace.terminal_status = ConversationTurnTraceTerminalStatus::InProgress;
    let conversation = ChatConversationRecord {
        id: "conversation-interrupted".to_string(),
        project_id: None,
        model_id: None,
        title: "Interrupted".to_string(),
        messages: vec![ChatMessageRecord {
            human_interaction_response: None,
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

    let history = conversation_history_messages(&conversation, &[trace.clone()], &[]).unwrap();

    assert_eq!(history.len(), 1);
    assert!(history[0].content.is_empty());
    assert_eq!(history[0].conversation_turn_trace.as_ref(), Some(&trace));
}

#[test]
fn next_turn_carries_the_uncompressed_model_projection_beside_the_durable_trace() {
    let trace = completed_trace("conversation-exact-history", "assistant-exact-history", 19);
    let runtime_call_id = history_call_id();
    let model_items = vec![
        mycopilot_core::ConversationModelContextItem {
            images: Vec::new(),
            sequence: 0,
            ordinal: 0,
            role: "assistant".to_string(),
            content: "I am creating the requested file.".to_string(),
            tool_call_id: None,
            tool_calls: Vec::new(),
            is_error: false,
        },
        mycopilot_core::ConversationModelContextItem {
            images: Vec::new(),
            sequence: 1,
            ordinal: 0,
            role: "assistant".to_string(),
            content: String::new(),
            tool_call_id: None,
            tool_calls: vec![mycopilot_core::AgentContextCheckpointToolCall {
                id: runtime_call_id.clone(),
                name: "apply_patch".to_string(),
                args: json!({
                    "request": {
                        "action": "apply",
                        "operation": "create",
                        "filePath": "src/history.rs",
                        "content": "EXACT_WRITE_CONTENT"
                    }
                }),
                provider_identity: mycopilot_core::AgentProviderToolCallIdentity {
                    provider_tool_index: 0,
                    provider_call_id: "write-history".to_string(),
                    runtime_call_id: runtime_call_id.clone(),
                },
            }],
            is_error: false,
        },
        mycopilot_core::ConversationModelContextItem {
            images: Vec::new(),
            sequence: 2,
            ordinal: 0,
            role: "tool".to_string(),
            content: r#"{"ok":true,"result":{"exact":"EXACT_TOOL_RESULT"}}"#.to_string(),
            tool_call_id: Some(runtime_call_id),
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
            human_interaction_response: None,
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

    let history = conversation_history_messages_with_model_context(
        &conversation,
        std::slice::from_ref(&trace),
        &logs,
        None,
        &[],
    )
    .unwrap();

    assert_eq!(history.len(), 1);
    assert_eq!(history[0].conversation_model_context_items, model_items);
    assert!(history[0].conversation_turn_trace.is_some());

    let incomplete_logs = [mycopilot_core::ConversationModelContextLog {
        assistant_message_id: "assistant-exact-history".to_string(),
        items: logs[0].items[..1].to_vec(),
    }];
    let error = conversation_history_messages_with_model_context(
        &conversation,
        &[trace],
        &incomplete_logs,
        None,
        &[],
    )
    .unwrap_err();
    assert!(error.contains("incomplete model context"));
    assert!(!error.contains("EXACT_WRITE_CONTENT"));
    assert!(!error.contains("EXACT_TOOL_RESULT"));
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
                human_interaction_response: None,
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
                human_interaction_response: None,
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
                human_interaction_response: None,
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
        conversation_history_messages_with_compaction(&conversation, &[], Some(&summary), &[])
            .unwrap();

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
                human_interaction_response: None,
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
                human_interaction_response: None,
                id: "assistant-current".to_string(),
                role: "assistant".to_string(),
                content: String::new(),
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
                provenance: AgentToolIdentity::Builtin {
                    tool_name: "read_file".to_string(),
                },
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
                first_tool_call_id: None,
                provider_turn_id: None,
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

    let tail_logs = [mycopilot_core::ConversationModelContextLog {
        assistant_message_id: "assistant-current".to_string(),
        items: vec![mycopilot_core::ConversationModelContextItem {
            images: Vec::new(),
            sequence: 3,
            ordinal: 0,
            role: "assistant".to_string(),
            content: "UNCOVERED_TAIL_MARKER".to_string(),
            tool_call_id: None,
            tool_calls: Vec::new(),
            is_error: false,
        }],
    }];
    let projected = conversation_history_messages_with_model_context(
        &conversation,
        &[trace],
        &tail_logs,
        Some(&summary),
        &[],
    )
    .unwrap();

    assert_eq!(projected.len(), 2);
    assert_eq!(projected[0].content, "LATEST_USER_MARKER");
    let tail = projected[1].conversation_turn_trace.as_ref().unwrap();
    assert_eq!(tail.items.len(), 1);
    assert_eq!(tail.items[0].sequence(), 3);
}

#[test]
fn compaction_projection_drops_command_session_audit_that_references_the_covered_prefix() {
    let conversation = ChatConversationRecord {
        id: "conversation-command-audit-tail".to_string(),
        project_id: None,
        model_id: None,
        title: "Command audit tail".to_string(),
        messages: vec![
            ChatMessageRecord {
                human_interaction_response: None,
                id: "user-command-audit-tail".to_string(),
                role: "user".to_string(),
                content: "Run the command.".to_string(),
                created_at: 1,
                status: Some("sent".to_string()),
                attachments: Vec::new(),
                agent_run_json: None,
                ui_state_json: None,
            },
            ChatMessageRecord {
                human_interaction_response: None,
                id: "assistant-command-audit-tail".to_string(),
                role: "assistant".to_string(),
                content: "The command completed.".to_string(),
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
    };
    let call_id = history_call_id();
    let session_id = "cmd_0123456789abcdef0123456789abcdef";
    let trace = ConversationTurnTrace {
        schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
        run_id: "run-command-audit-tail".to_string(),
        conversation_id: conversation.id.clone(),
        assistant_message_id: "assistant-command-audit-tail".to_string(),
        terminal_status: ConversationTurnTraceTerminalStatus::Completed,
        terminal_error: None,
        truncated: false,
        items: vec![
            ConversationTurnTraceItem::ToolCall {
                sequence: 0,
                call_id: call_id.clone(),
                tool: "run_command".to_string(),
                operation: json!({ "command": "sleep 30" }),
                provenance: AgentToolIdentity::Builtin {
                    tool_name: "run_command".to_string(),
                },
                approval_status: AgentApprovalStatus::Approved,
                truncated: false,
            },
            ConversationTurnTraceItem::ToolResult {
                sequence: 1,
                call_id: call_id.clone(),
                tool: "run_command".to_string(),
                status: ConversationTraceToolResultStatus::Succeeded,
                success: true,
                observation: json!({ "status": "running", "sessionId": session_id }),
                approval_status: AgentApprovalStatus::Approved,
                error: None,
                truncated: false,
                archive: Default::default(),
            },
            ConversationTurnTraceItem::AssistantNarration {
                first_tool_call_id: None,
                provider_turn_id: None,
                sequence: 2,
                content: "The remaining model-visible tail.".to_string(),
                truncated: false,
            },
            ConversationTurnTraceItem::CommandSessionLifecycle {
                sequence: 3,
                phase: mycopilot_core::ConversationCommandSessionLifecyclePhase::Started,
                session_id: session_id.to_string(),
                call_id: call_id.clone(),
                status: AgentCommandSessionStatus::Running,
                exit_code: None,
                latest_sequence: 0,
                output_truncated: false,
                archive: Default::default(),
                created_at: 2,
            },
            ConversationTurnTraceItem::CommandSessionLifecycle {
                sequence: 4,
                phase: mycopilot_core::ConversationCommandSessionLifecyclePhase::Terminal,
                session_id: session_id.to_string(),
                call_id: call_id.clone(),
                status: AgentCommandSessionStatus::Exited,
                exit_code: Some(0),
                latest_sequence: 1,
                output_truncated: false,
                archive: Default::default(),
                created_at: 3,
            },
        ],
    };
    trace.validate().unwrap();

    let prefix = ContextCompactionPrefix {
        conversation_id: conversation.id.clone(),
        source_revision: "revision-command-audit-tail".to_string(),
        covered_through: ContextJournalCursor::trace_item("assistant-command-audit-tail", 1),
        previous_summary: None,
        source_items: vec![
            ContextCompactionSourceItem::Message {
                cursor: ContextJournalCursor::message("user-command-audit-tail"),
                role: "user".to_string(),
                content: "Run the command.".to_string(),
                created_at: 1,
                status: Some("sent".to_string()),
                terminal_status: None,
                terminal_error: None,
            },
            ContextCompactionSourceItem::TraceItem {
                cursor: ContextJournalCursor::trace_item("assistant-command-audit-tail", 0),
                run_id: trace.run_id.clone(),
                created_at: 2,
                item: Box::new(trace.items[0].clone()),
            },
            ContextCompactionSourceItem::TraceItem {
                cursor: ContextJournalCursor::trace_item("assistant-command-audit-tail", 1),
                run_id: trace.run_id.clone(),
                created_at: 2,
                item: Box::new(trace.items[1].clone()),
            },
        ],
    };
    prefix.validate().unwrap();
    let summary = ContextCompactionSummary {
        schema_version: CONTEXT_COMPACTION_SUMMARY_SCHEMA_VERSION,
        id: "summary-command-audit-tail".to_string(),
        conversation_id: conversation.id.clone(),
        source_revision: prefix.source_revision.clone(),
        previous_summary_id: None,
        covered_through: prefix.covered_through.clone(),
        content: "The command was started successfully.".to_string(),
        continuity: mycopilot_core::ContextContinuitySnapshot::from_prefix(&prefix).unwrap(),
        generation: ContextCompactionGeneration::test(),
        source_input_tokens: 100,
        summary_input_tokens: 10,
        continuity_input_tokens: 10,
        uncovered_tail_input_tokens: 10,
        replacement_input_tokens: 20,
        created_at: 4,
    };
    summary.validate().unwrap();
    let model_context_items = vec![
        ConversationModelContextItem {
            images: Vec::new(),
            sequence: 0,
            ordinal: 0,
            role: "assistant".to_string(),
            content: String::new(),
            tool_call_id: None,
            tool_calls: vec![AgentContextCheckpointToolCall {
                id: call_id.clone(),
                name: "run_command".to_string(),
                args: json!({ "command": "sleep 30" }),
                provider_identity: AgentProviderToolCallIdentity {
                    provider_tool_index: 0,
                    provider_call_id: "provider-command-audit-tail".to_string(),
                    runtime_call_id: call_id.clone(),
                },
            }],
            is_error: false,
        },
        ConversationModelContextItem {
            images: Vec::new(),
            sequence: 1,
            ordinal: 0,
            role: "tool".to_string(),
            content: r#"{"status":"running"}"#.to_string(),
            tool_call_id: Some(call_id),
            tool_calls: Vec::new(),
            is_error: false,
        },
        ConversationModelContextItem {
            images: Vec::new(),
            sequence: 2,
            ordinal: 0,
            role: "assistant".to_string(),
            content: "The remaining model-visible tail.".to_string(),
            tool_call_id: None,
            tool_calls: Vec::new(),
            is_error: false,
        },
    ];
    trace
        .validate_complete_model_context(&model_context_items)
        .unwrap();

    let projected = conversation_history_messages_with_model_context(
        &conversation,
        std::slice::from_ref(&trace),
        &[mycopilot_core::ConversationModelContextLog {
            assistant_message_id: trace.assistant_message_id.clone(),
            items: model_context_items.clone(),
        }],
        Some(&summary),
        &[],
    )
    .unwrap();

    assert_eq!(projected.len(), 1);
    let projected_trace = projected[0].conversation_turn_trace.as_ref().unwrap();
    assert_eq!(projected_trace.items.len(), 1);
    assert!(matches!(
        &projected_trace.items[0],
        ConversationTurnTraceItem::AssistantNarration { sequence: 2, .. }
    ));
    assert_eq!(trace.items.len(), 5, "the durable audit trace stays intact");

    let mut invalid_raw_trace = trace.clone();
    let ConversationTurnTraceItem::CommandSessionLifecycle { call_id, .. } =
        &mut invalid_raw_trace.items[3]
    else {
        panic!("expected command session lifecycle fixture");
    };
    *call_id = format!("tc1_{}", "B".repeat(43));
    let error = conversation_history_messages_with_model_context(
        &conversation,
        &[invalid_raw_trace],
        &[mycopilot_core::ConversationModelContextLog {
            assistant_message_id: trace.assistant_message_id.clone(),
            items: model_context_items,
        }],
        Some(&summary),
        &[],
    )
    .unwrap_err();
    assert!(error.contains("has an invalid trace"));
    assert!(!error.contains("invalid projected trace"));
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
                provider_model_id: "model-1".to_string(),
                display_name: "Model 1".to_string(),
                api_url_override: None,
                api_token_override: None,
                supports_image: false,
                context_window_tokens: Some(128_000),
                provider_profile_config: mycopilot_core::ProviderProfileConfig::generic_for_dialect(
                    mycopilot_core::ProviderProtocolDialect::OpenAiChatCompletions,
                ),
                input_price: "0.01".to_string(),
                cached_input_price: String::new(),
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
                human_interaction_response: None,
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
                human_interaction_response: None,
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
                    human_interaction_response: None,
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
                    human_interaction_response: None,
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
    storage
        .replace_conversation_turn_trace(
            &mycopilot_core::completed_conversation_trace_without_items(
                "run-capacity-summary",
                "conversation-capacity-summary",
                "assistant-long",
            ),
            2,
            2,
        )
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

#[test]
#[ignore = "release profile: 500 durable Turns through real ContextAssembler and compaction"]
fn five_hundred_turn_context_compaction_release_profile() {
    const LOGICAL_TURNS: usize = 500;
    const MESSAGE_BODY_BYTES: usize = 320;

    let total_started = std::time::Instant::now();
    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("context-500.sqlite");
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    storage.save_model_settings(test_model_settings()).unwrap();

    let seed_started = std::time::Instant::now();
    let mut messages = Vec::with_capacity(LOGICAL_TURNS * 2);
    for turn in 0..LOGICAL_TURNS {
        let user_created_at = i64::try_from(turn * 2 + 1).unwrap();
        let assistant_created_at = user_created_at + 1;
        messages.push(ChatMessageRecord {
            human_interaction_response: None,
            id: format!("user-profile-{turn:03}"),
            role: "user".to_string(),
            content: format!("turn {turn:03} request {}", "u".repeat(MESSAGE_BODY_BYTES)),
            created_at: user_created_at,
            status: Some("sent".to_string()),
            attachments: Vec::new(),
            agent_run_json: None,
            ui_state_json: None,
        });
        messages.push(ChatMessageRecord {
            human_interaction_response: None,
            id: format!("assistant-profile-{turn:03}"),
            role: "assistant".to_string(),
            content: format!("turn {turn:03} response {}", "a".repeat(MESSAGE_BODY_BYTES)),
            created_at: assistant_created_at,
            status: Some("sent".to_string()),
            attachments: Vec::new(),
            agent_run_json: None,
            ui_state_json: None,
        });
    }
    storage
        .save_conversation(ChatConversationRecord {
            id: "conversation-500-profile".to_string(),
            project_id: None,
            model_id: Some("model-1".to_string()),
            title: "500 Turn context profile".to_string(),
            messages,
            created_at: 1,
            updated_at: i64::try_from(LOGICAL_TURNS * 2).unwrap(),
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
    for turn in 0..LOGICAL_TURNS {
        let assistant_message_id = format!("assistant-profile-{turn:03}");
        let created_at = i64::try_from(turn * 2 + 2).unwrap();
        storage
            .replace_conversation_turn_trace(
                &mycopilot_core::completed_conversation_trace_without_items(
                    &format!("run-profile-{turn:03}"),
                    "conversation-500-profile",
                    &assistant_message_id,
                ),
                created_at,
                created_at,
            )
            .unwrap();
    }
    let seed_elapsed = seed_started.elapsed();

    let service = AgentService::new(storage.clone());
    let snapshot_input = AgentContextWindowSnapshotInput {
        conversation_id: Some("conversation-500-profile".to_string()),
        project_id: None,
        model_id: "model-1".to_string(),
        max_tokens: Some(30_000),
        prompt_preferences: None,
        permissions: AgentPermissions::default(),
        skills: Vec::new(),
    };
    let pre_assembly_started = std::time::Instant::now();
    let before = service
        .get_context_window_snapshot(snapshot_input.clone())
        .unwrap()
        .snapshot
        .unwrap();
    let pre_assembly_elapsed = pre_assembly_started.elapsed();
    assert!(
        before
            .input_capacity_tokens
            .is_some_and(|capacity| before.input_tokens > capacity),
        "the profile must reach the real over-capacity compaction condition"
    );

    let compaction_started = std::time::Instant::now();
    let last_assistant = format!("assistant-profile-{:03}", LOGICAL_TURNS - 1);
    let prefix = storage
        .prepare_context_compaction_prefix(
            "conversation-500-profile",
            &ContextJournalCursor::message(&last_assistant),
        )
        .unwrap();
    let compacted_source_items = prefix.source_items.len();
    storage
        .commit_context_compaction_prefix(
            &prefix,
            test_compaction_draft(
                &prefix,
                "summary-500-profile",
                "The preceding 500 logical turns are durably summarized for this profile.",
                before.input_tokens,
                i64::try_from(LOGICAL_TURNS * 2 + 1).unwrap(),
            ),
            &last_assistant,
        )
        .unwrap();
    service.invalidate_conversation_context_state("conversation-500-profile");
    let compaction_elapsed = compaction_started.elapsed();

    let post_assembly_started = std::time::Instant::now();
    let after = service
        .get_context_window_snapshot(snapshot_input)
        .unwrap()
        .snapshot
        .unwrap();
    let post_assembly_elapsed = post_assembly_started.elapsed();
    assert!(after.input_tokens < before.input_tokens);
    assert_eq!(
        storage
            .load_conversation("conversation-500-profile")
            .unwrap()
            .unwrap()
            .messages
            .len(),
        LOGICAL_TURNS * 2,
        "compaction must not delete the durable Conversation history"
    );
    assert_eq!(
        storage
            .get_active_context_compaction_summary("conversation-500-profile")
            .unwrap()
            .as_ref()
            .map(|summary| summary.id.as_str()),
        Some("summary-500-profile")
    );
    let sqlite_bytes = std::fs::metadata(&database_path)
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    eprintln!(
        "context_compaction_profile logical_turns={LOGICAL_TURNS} messages={} source_items={compacted_source_items} pre_input_tokens={} post_input_tokens={} sqlite_bytes={sqlite_bytes} seed_ms={} pre_assemble_ms={} compact_ms={} post_assemble_ms={} total_ms={}",
        LOGICAL_TURNS * 2,
        before.input_tokens,
        after.input_tokens,
        seed_elapsed.as_millis(),
        pre_assembly_elapsed.as_millis(),
        compaction_elapsed.as_millis(),
        post_assembly_elapsed.as_millis(),
        total_started.elapsed().as_millis(),
    );
}

#[test]
fn compacted_history_retains_only_immutable_images_without_reviving_completion() {
    let image = mycopilot_core::ConversationContextImageRef {
        attachment_id: "historical-image".into(),
        mime_type: "image/png".into(),
        sha256: format!("sha256:{}", "a".repeat(64)),
    };
    let messages = [
        ("user-visual", "user", "inspect image"),
        ("assistant-visual", "assistant", "old visual answer"),
        ("user-later", "user", "continue"),
        ("assistant-later", "assistant", "later answer"),
    ]
    .into_iter()
    .enumerate()
    .map(|(index, (id, role, content))| ChatMessageRecord {
        human_interaction_response: None,
        id: id.into(),
        role: role.into(),
        content: content.into(),
        created_at: index as i64 + 1,
        status: Some("sent".into()),
        attachments: Vec::new(),
        agent_run_json: None,
        ui_state_json: None,
    })
    .collect();
    let conversation = ChatConversationRecord {
        id: "visual-history".into(),
        project_id: None,
        model_id: None,
        title: "images".into(),
        messages,
        created_at: 1,
        updated_at: 4,
        pinned_at: None,
        archived_at: None,
        unread_at: None,
    };
    let trace = ConversationTurnTrace {
        schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
        run_id: "visual-run".into(),
        conversation_id: conversation.id.clone(),
        assistant_message_id: "assistant-visual".into(),
        terminal_status: ConversationTurnTraceTerminalStatus::Completed,
        terminal_error: None,
        truncated: false,
        items: vec![
            ConversationTurnTraceItem::ContextMaterial {
                sequence: 0,
                event_id: "visual-input".into(),
                material_kind: mycopilot_core::ConversationContextMaterialKind::InputAttachment,
                content: "image description".into(),
                images: vec![image.clone()],
                created_at: 2,
            },
            ConversationTurnTraceItem::ContextMaterial {
                sequence: 1,
                event_id: "visual-world".into(),
                material_kind: mycopilot_core::ConversationContextMaterialKind::RunWorldState,
                content: "old run state".into(),
                images: Vec::new(),
                created_at: 2,
            },
        ],
    };
    let later_trace = ConversationTurnTrace {
        run_id: "later-run".into(),
        assistant_message_id: "assistant-later".into(),
        items: Vec::new(),
        ..trace.clone()
    };
    let log = mycopilot_core::ConversationModelContextLog {
        assistant_message_id: trace.assistant_message_id.clone(),
        items: vec![
            ConversationModelContextItem {
                sequence: 0,
                ordinal: 0,
                role: "user".into(),
                content: "image description".into(),
                images: vec![image.clone()],
                tool_call_id: None,
                tool_calls: Vec::new(),
                is_error: false,
            },
            ConversationModelContextItem {
                sequence: 1,
                ordinal: 0,
                role: "user".into(),
                content: "old run state".into(),
                images: Vec::new(),
                tool_call_id: None,
                tool_calls: Vec::new(),
                is_error: false,
            },
        ],
    };
    // Cover the image's own message, then cover a later whole message. Both paths must preserve
    // image bytes but must not resurrect the already summarized assistant prose or pure text.
    for cursor in [
        ContextJournalCursor::message("assistant-visual"),
        ContextJournalCursor::message("assistant-later"),
    ] {
        let summary = ContextCompactionSummary {
            schema_version: CONTEXT_COMPACTION_SUMMARY_SCHEMA_VERSION,
            id: "visual-summary".into(),
            conversation_id: conversation.id.clone(),
            source_revision: "visual-revision".into(),
            previous_summary_id: None,
            covered_through: cursor.clone(),
            content: "summary".into(),
            continuity: mycopilot_core::ContextContinuitySnapshot {
                schema_version: mycopilot_core::CONTEXT_CONTINUITY_SCHEMA_VERSION,
                covered_through: cursor,
                task_evidence_refs: Vec::new(),
                unresolved_failure_refs: Vec::new(),
                approval_refs: Vec::new(),
                important_decision_refs: Vec::new(),
                recent_refs: Vec::new(),
                archived_counts: Default::default(),
            },
            generation: ContextCompactionGeneration::test(),
            source_input_tokens: 100,
            summary_input_tokens: 10,
            continuity_input_tokens: 10,
            uncovered_tail_input_tokens: 0,
            replacement_input_tokens: 20,
            created_at: 5,
        };
        let projected = conversation_history_messages_with_model_context(
            &conversation,
            &[trace.clone(), later_trace.clone()],
            std::slice::from_ref(&log),
            Some(&summary),
            &[],
        )
        .unwrap();
        let visual = projected
            .iter()
            .find(|message| message.message_id.as_deref() == Some("assistant-visual"))
            .unwrap();
        assert!(visual.content.is_empty());
        assert!(visual.conversation_completion_covered);
        assert_eq!(
            visual.conversation_turn_trace.as_ref().unwrap().items.len(),
            1
        );
        assert_eq!(visual.conversation_model_context_items.len(), 1);
        assert_eq!(
            visual.conversation_model_context_items[0].images,
            vec![image.clone()]
        );
        assert!(!projected
            .iter()
            .any(|message| message.content == "old visual answer"));
    }
}
