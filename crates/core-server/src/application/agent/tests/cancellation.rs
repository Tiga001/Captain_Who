use super::*;

fn pending_checkpoint_context_item(
    call: &AgentToolCall,
) -> mycopilot_core::AgentContextCheckpointItem {
    mycopilot_core::AgentContextCheckpointItem {
        role: "assistant".to_string(),
        content: String::new(),
        images: Vec::new(),
        tool_call_id: None,
        tool_calls: vec![pending_checkpoint_tool_call(call)],
        is_error: false,
        sources: vec!["model_response".to_string()],
        scope: "run".to_string(),
        retention: "retained".to_string(),
        group: None,
        origin: None,
    }
}

fn pending_checkpoint_tool_call(call: &AgentToolCall) -> AgentContextCheckpointToolCall {
    AgentContextCheckpointToolCall {
        id: call.id.clone(),
        name: call.tool.clone(),
        args: call.args.clone(),
        provider_identity: AgentProviderToolCallIdentity {
            provider_tool_index: 0,
            provider_call_id: call.id.clone(),
            runtime_call_id: call.id.clone(),
        },
    }
}

fn pending_model_context_item(call: &AgentToolCall) -> ConversationModelContextItem {
    ConversationModelContextItem {
        sequence: 0,
        ordinal: 0,
        role: "assistant".to_string(),
        content: String::new(),
        tool_call_id: None,
        tool_calls: vec![pending_checkpoint_tool_call(call)],
        is_error: false,
    }
}

#[test]
fn cancelling_pending_approval_commits_one_paired_cancelled_trace() {
    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("storage.sqlite");
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    storage
        .save_conversation(ChatConversationRecord {
            id: "conversation-cancel".to_string(),
            project_id: None,
            model_id: Some("model-1".to_string()),
            title: "Cancel trace".to_string(),
            messages: vec![
                ChatMessageRecord {
                    id: "user-cancel".to_string(),
                    role: "user".to_string(),
                    content: "Keep tracking this objective.".to_string(),
                    created_at: 1,
                    status: Some("sent".to_string()),
                    attachments: Vec::new(),
                    agent_run_json: None,
                    ui_state_json: None,
                },
                ChatMessageRecord {
                    id: "assistant-cancel".to_string(),
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
        })
        .unwrap();
    let service = AgentService::new(storage.clone());
    service.register_usage_context(
        "run-cancel",
        AgentRunUsageContext {
            conversation_id: "conversation-cancel".to_string(),
            assistant_message_id: "assistant-cancel".to_string(),
            run_id: "run-cancel".to_string(),
            project_id: None,
            model_id: "model-1".to_string(),
            model_name: "Model 1".to_string(),
            provider_usage_semantics: ProviderUsageSemantics::StandardAdditive,
            input_price: None,
            cached_input_price: None,
            output_price: None,
            started_at: 1,
        },
    );
    let call = AgentToolCall {
        id: "call-cancel".to_string(),
        tool: "approval_tool".to_string(),
        args: json!({ "path": "safe.txt" }),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };
    let mut agent_input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "secret",
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "messages": []
    }))
    .unwrap();
    agent_input.resume_checkpoint = Some(AgentRunCheckpoint {
        version: AGENT_RUN_CHECKPOINT_SCHEMA_VERSION,
        run_id: "run-cancel".to_string(),
        pending_action_id: None,
        context_items: vec![pending_checkpoint_context_item(&call)],
        next_model_request_index: 1,
        queued_tool_calls: Vec::new(),
        deferred_external_tool_call_count: 0,
        suppressed_narration: false,
        extension_snapshots: Vec::new(),
        tool_set: crate::test_tool_set_checkpoint(),
        run_context: None,
        collaboration_run_snapshot: None,
        model_capabilities: ModelCapabilities::default(),
        provider_profile_config: crate::test_provider_profile_config(),
        provider_protocol_key: crate::test_provider_protocol_key("test-model"),
        assistant_turn_identity: crate::test_assistant_turn_identity(&[call.id.as_str()]),
        provider_continuation_refs: Vec::new(),
        run_world_state: crate::test_run_world_state(),
        pending_tool_call_id: call.id.clone(),
        conversation_model_context_items: vec![pending_model_context_item(&call)],
        conversation_trace_items: vec![ConversationTurnTraceItem::ToolCall {
            sequence: 0,
            call_id: call.id.clone(),
            tool: call.tool.clone(),
            operation: json!({ "path": "safe.txt" }),
            provenance: AgentToolIdentity::Builtin {
                tool_name: call.tool.clone(),
            },
            approval_status: AgentApprovalStatus::Required,
            truncated: false,
        }],
        next_conversation_trace_sequence: 1,
        conversation_trace_truncated: false,
    });
    save_test_pending_provider_for_input(&storage, &mut agent_input);
    service
        .store_pending_action(
            "run-cancel",
            "conversation-cancel",
            "assistant-cancel",
            AgentProposedAction::ToolCall { call: call.clone() },
            agent_input,
        )
        .unwrap();

    let approval_events = storage.list_notifications(None, 100, false, None).unwrap();
    let approval_event = approval_events
        .items
        .iter()
        .find(|event| event.notification_kind == "approval_required")
        .expect("ordinary HumanRoot approval must publish a durable notification");
    assert_eq!(
        approval_event.approval_action_id.as_deref(),
        Some(call.id.as_str())
    );
    assert_eq!(approval_event.subject_text, "Keep tracking this objective.");
    assert!(approval_event.resolved_at.is_none());

    assert!(service.cancel_action("run-cancel", &call.id).unwrap());

    let trace = storage
        .get_conversation_turn_trace("assistant-cancel")
        .unwrap()
        .unwrap();
    trace.validate().unwrap();
    assert_eq!(
        trace.terminal_status,
        ConversationTurnTraceTerminalStatus::Cancelled
    );
    assert_eq!(trace.items.len(), 2);
    assert!(matches!(
        &trace.items[1],
        ConversationTurnTraceItem::ToolResult {
            call_id,
            approval_status: AgentApprovalStatus::Rejected,
            ..
        } if call_id == "call-cancel"
    ));
    let conversation = storage
        .load_conversations()
        .unwrap()
        .into_iter()
        .find(|conversation| conversation.id == "conversation-cancel")
        .unwrap();
    assert_eq!(conversation.messages[1].content, "");
    assert_eq!(conversation.messages[1].status.as_deref(), Some("sent"));

    let terminal_events = storage.list_notifications(None, 100, false, None).unwrap();
    assert!(terminal_events
        .items
        .iter()
        .all(|event| event.notification_kind != "approval_required"));
    let cancelled = terminal_events
        .items
        .iter()
        .find(|event| event.notification_kind == "task_cancelled")
        .expect("terminal cancellation must publish its replacement notification");
    assert_eq!(cancelled.subject_text, "Keep tracking this objective.");
    assert!(cancelled.resolved_at.is_some());

    let notification_connection = rusqlite::Connection::open(database_path).unwrap();
    let (resolved_at, superseded_at): (Option<i64>, Option<i64>) = notification_connection
        .query_row(
            "SELECT resolved_at, superseded_at FROM notification_events WHERE id = ?1",
            [&approval_event.id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert!(resolved_at.is_some());
    assert!(superseded_at.is_some());
}

#[test]
fn forced_cancellation_uses_backend_runtime_snapshot_instead_of_empty_trace() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    storage
        .save_conversation(ChatConversationRecord {
            id: "conversation-forced".to_string(),
            project_id: None,
            model_id: Some("model-1".to_string()),
            title: "Forced cancellation".to_string(),
            messages: vec![ChatMessageRecord {
                id: "assistant-forced".to_string(),
                role: "assistant".to_string(),
                content: String::new(),
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
        })
        .unwrap();
    let service = AgentService::new(storage.clone());
    service.register_usage_context(
        "run-forced",
        AgentRunUsageContext {
            conversation_id: "conversation-forced".to_string(),
            assistant_message_id: "assistant-forced".to_string(),
            run_id: "run-forced".to_string(),
            project_id: None,
            model_id: "model-1".to_string(),
            model_name: "Model 1".to_string(),
            provider_usage_semantics: ProviderUsageSemantics::StandardAdditive,
            input_price: None,
            cached_input_price: None,
            output_price: None,
            started_at: 1,
        },
    );
    let checkpoint = AgentRunCheckpoint {
        version: AGENT_RUN_CHECKPOINT_SCHEMA_VERSION,
        run_id: "run-forced".to_string(),
        pending_action_id: None,
        context_items: Vec::new(),
        next_model_request_index: 1,
        queued_tool_calls: Vec::new(),
        deferred_external_tool_call_count: 0,
        suppressed_narration: false,
        extension_snapshots: Vec::new(),
        tool_set: crate::test_tool_set_checkpoint(),
        run_context: None,
        collaboration_run_snapshot: None,
        model_capabilities: ModelCapabilities::default(),
        provider_profile_config: crate::test_provider_profile_config(),
        provider_protocol_key: crate::test_provider_protocol_key("model-1"),
        assistant_turn_identity: crate::test_assistant_turn_identity(&["pending-command"]),
        provider_continuation_refs: Vec::new(),
        run_world_state: crate::test_run_world_state(),
        pending_tool_call_id: "pending-command".to_string(),
        conversation_model_context_items: vec![
            ConversationModelContextItem {
                sequence: 0,
                ordinal: 0,
                role: "assistant".to_string(),
                content: String::new(),
                tool_call_id: None,
                tool_calls: vec![AgentContextCheckpointToolCall {
                    id: "write-forced".to_string(),
                    name: "apply_patch".to_string(),
                    args: json!({
                        "action": "apply",
                        "operation": "create",
                        "filePath": "created.txt",
                        "observationId": "fobs_forced_cancellation",
                        "content": "forced cancellation fixture"
                    }),
                    provider_identity: AgentProviderToolCallIdentity {
                        provider_tool_index: 0,
                        provider_call_id: "write-forced".to_string(),
                        runtime_call_id: "write-forced".to_string(),
                    },
                }],
                is_error: false,
            },
            ConversationModelContextItem {
                sequence: 1,
                ordinal: 0,
                role: "tool".to_string(),
                content: json!({
                    "filePath": "created.txt",
                    "status": "applied",
                    "additions": 3,
                    "deletions": 0
                })
                .to_string(),
                tool_call_id: Some("write-forced".to_string()),
                tool_calls: Vec::new(),
                is_error: false,
            },
        ],
        conversation_trace_items: vec![
            ConversationTurnTraceItem::ToolCall {
                sequence: 0,
                call_id: "write-forced".to_string(),
                tool: "apply_patch".to_string(),
                operation: json!({
                    "action": "apply",
                    "operation": "create",
                    "filePath": "created.txt"
                }),
                provenance: AgentToolIdentity::Builtin {
                    tool_name: "apply_patch".to_string(),
                },
                approval_status: AgentApprovalStatus::Approved,
                truncated: false,
            },
            ConversationTurnTraceItem::ToolResult {
                sequence: 1,
                call_id: "write-forced".to_string(),
                tool: "apply_patch".to_string(),
                status: ConversationTraceToolResultStatus::Succeeded,
                success: true,
                observation: json!({
                    "filePath": "created.txt",
                    "status": "applied",
                    "additions": 3,
                    "deletions": 0
                }),
                approval_status: AgentApprovalStatus::Approved,
                error: None,
                truncated: false,
                archive: Default::default(),
            },
        ],
        next_conversation_trace_sequence: 2,
        conversation_trace_truncated: false,
    };
    service.seed_trace_snapshot_from_checkpoint("run-forced", Some(&checkpoint));

    service.persist_forced_cancelled_runs(&["run-forced".to_string()]);

    let trace = storage
        .get_conversation_turn_trace("assistant-forced")
        .unwrap()
        .unwrap();
    trace.validate().unwrap();
    assert_eq!(
        trace.terminal_status,
        ConversationTurnTraceTerminalStatus::Cancelled
    );
    assert_eq!(trace.items.len(), 2);
    assert!(serde_json::to_string(&trace)
        .unwrap()
        .contains("created.txt"));
}

#[test]
fn failed_forced_cancellation_projection_is_retired_by_current_startup_reconciliation() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    storage
        .save_conversation(ChatConversationRecord {
            id: "conversation-forced-recovery".to_string(),
            project_id: None,
            model_id: Some("model-1".to_string()),
            title: "Forced cancellation recovery".to_string(),
            messages: vec![ChatMessageRecord {
                id: "assistant-forced-recovery".to_string(),
                role: "assistant".to_string(),
                content: String::new(),
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
        })
        .unwrap();
    let durable_trace = ConversationTurnTrace {
        schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
        run_id: "run-forced-recovery".to_string(),
        conversation_id: "conversation-forced-recovery".to_string(),
        assistant_message_id: "assistant-forced-recovery".to_string(),
        terminal_status: ConversationTurnTraceTerminalStatus::InProgress,
        terminal_error: None,
        truncated: false,
        items: vec![ConversationTurnTraceItem::AssistantNarration {
            sequence: 0,
            content: "A durable partial response.".to_string(),
            truncated: false,
        }],
    };
    let durable_model_context = vec![ConversationModelContextItem {
        sequence: 0,
        ordinal: 0,
        role: "assistant".to_string(),
        content: "A durable partial response.".to_string(),
        tool_call_id: None,
        tool_calls: Vec::new(),
        is_error: false,
    }];
    storage
        .append_in_progress_conversation_turn_trace_and_apply_guidances(
            &durable_trace,
            &durable_model_context,
            1,
            1,
        )
        .unwrap();

    let service = AgentService::new(storage.clone());
    service.register_usage_context(
        "run-forced-recovery",
        AgentRunUsageContext {
            conversation_id: "conversation-forced-recovery".to_string(),
            assistant_message_id: "assistant-forced-recovery".to_string(),
            run_id: "run-forced-recovery".to_string(),
            project_id: None,
            model_id: "model-1".to_string(),
            model_name: "Model 1".to_string(),
            provider_usage_semantics: ProviderUsageSemantics::StandardAdditive,
            input_price: None,
            cached_input_price: None,
            output_price: None,
            started_at: 1,
        },
    );
    service.trace_snapshots.lock().unwrap().insert(
        "run-forced-recovery".to_string(),
        ConversationTraceSnapshot {
            items: vec![ConversationTurnTraceItem::ToolCall {
                sequence: 0,
                call_id: "corrupt-call".to_string(),
                tool: "apply_patch".to_string(),
                provenance: AgentToolIdentity::Builtin {
                    tool_name: "apply_patch".to_string(),
                },
                operation: json!({
                    "action": "apply",
                    "operation": "create",
                    "filePath": "private.txt"
                }),
                approval_status: AgentApprovalStatus::Approved,
                truncated: false,
            }],
            model_context_items: Vec::new(),
            next_sequence: 1,
            truncated: false,
        },
    );

    service.persist_forced_cancelled_runs(&["run-forced-recovery".to_string()]);
    assert_eq!(
        storage
            .get_conversation_turn_trace("assistant-forced-recovery")
            .unwrap()
            .unwrap()
            .terminal_status,
        ConversationTurnTraceTerminalStatus::Failed,
        "startup must retire the orphaned durable trace before later in-memory cancellation cleanup"
    );

    assert_eq!(
        storage
            .reconcile_orphaned_in_progress_conversation_turn_traces(&HashSet::new(), 100)
            .unwrap(),
        0
    );
    let recovered = storage
        .get_conversation_turn_trace("assistant-forced-recovery")
        .unwrap()
        .unwrap();
    assert_eq!(
        recovered.terminal_status,
        ConversationTurnTraceTerminalStatus::Failed
    );
    let model_context = storage
        .get_conversation_model_context_log("assistant-forced-recovery")
        .unwrap()
        .unwrap();
    recovered
        .validate_complete_model_context(&model_context.items)
        .unwrap();
}

#[test]
fn terminal_message_and_trace_roll_back_together_when_trace_is_invalid() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    storage
        .save_conversation(ChatConversationRecord {
            id: "conversation-atomic".to_string(),
            project_id: None,
            model_id: None,
            title: "Atomic trace".to_string(),
            messages: vec![ChatMessageRecord {
                id: "assistant-atomic".to_string(),
                role: "assistant".to_string(),
                content: String::new(),
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
        })
        .unwrap();
    let invalid_trace = ConversationTurnTrace {
        schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
        run_id: "run-atomic".to_string(),
        conversation_id: "conversation-atomic".to_string(),
        assistant_message_id: "assistant-atomic".to_string(),
        terminal_status: ConversationTurnTraceTerminalStatus::Completed,
        terminal_error: None,
        truncated: false,
        items: vec![ConversationTurnTraceItem::ToolCall {
            sequence: 0,
            call_id: "unresolved".to_string(),
            tool: "read_file".to_string(),
            operation: json!({ "path": "file.txt" }),
            provenance: AgentToolIdentity::Builtin {
                tool_name: "read_file".to_string(),
            },
            approval_status: AgentApprovalStatus::NotRequired,
            truncated: false,
        }],
    };

    assert!(storage
        .finalize_chat_message_with_conversation_trace(
            "conversation-atomic",
            "assistant-atomic",
            "final answer",
            Some("sent"),
            "completed",
            &invalid_trace,
            1,
            2,
        )
        .is_err());

    let conversation = storage.load_conversations().unwrap().remove(0);
    assert!(conversation.messages[0].content.is_empty());
    assert_eq!(conversation.messages[0].status.as_deref(), Some("pending"));
    assert!(storage
        .get_conversation_turn_trace("assistant-atomic")
        .unwrap()
        .is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn cancelling_immediately_after_approval_prevents_command_side_effects() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    storage
        .save_conversation(ChatConversationRecord {
            id: "conversation-cancel-before-spawn".to_string(),
            project_id: None,
            model_id: Some("model-1".to_string()),
            title: "Cancel before spawn".to_string(),
            messages: vec![ChatMessageRecord {
                id: "assistant-cancel-before-spawn".to_string(),
                role: "assistant".to_string(),
                content: String::new(),
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
        })
        .unwrap();
    let service = AgentService::new(Arc::clone(&storage));
    let command = AgentCommandRequest {
        id: "command-cancel-before-spawn".to_string(),
        command: "mkdir cancelled-before-spawn".to_string(),
        cwd: None,
        timeout_ms: Some(10_000),
        approval_status: AgentApprovalStatus::Required,
        risk_level: Some(AgentCommandRiskLevel::WritesWorkspace),
        reason: Some("verify approval cancellation race".to_string()),
        observe: None,
        inputs: Vec::new(),
        runtime_binding: None,
        managed_office_script: None,
    };
    let call = checkpoint_call_for_command(&command);
    let mut checkpoint = AgentRunCheckpoint {
        version: AGENT_RUN_CHECKPOINT_SCHEMA_VERSION,
        run_id: "run-cancel-before-spawn".to_string(),
        pending_action_id: None,
        context_items: vec![pending_checkpoint_context_item(&call)],
        next_model_request_index: 1,
        queued_tool_calls: Vec::new(),
        deferred_external_tool_call_count: 0,
        suppressed_narration: false,
        extension_snapshots: Vec::new(),
        tool_set: crate::test_tool_set_checkpoint(),
        run_context: None,
        collaboration_run_snapshot: None,
        model_capabilities: ModelCapabilities::default(),
        provider_profile_config: crate::test_provider_profile_config(),
        provider_protocol_key: crate::test_provider_protocol_key("test-model"),
        assistant_turn_identity: crate::test_assistant_turn_identity(&[call.id.as_str()]),
        provider_continuation_refs: Vec::new(),
        run_world_state: crate::test_run_world_state(),
        pending_tool_call_id: call.id.clone(),
        conversation_model_context_items: vec![pending_model_context_item(&call)],
        conversation_trace_items: vec![ConversationTurnTraceItem::ToolCall {
            sequence: 0,
            call_id: call.id.clone(),
            tool: call.tool.clone(),
            operation: call.args.clone(),
            provenance: AgentToolIdentity::Builtin {
                tool_name: call.tool.clone(),
            },
            approval_status: AgentApprovalStatus::Required,
            truncated: false,
        }],
        next_conversation_trace_sequence: 1,
        conversation_trace_truncated: false,
    };
    let mut agent_input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://must-not-be-called.test/v1/chat/completions",
        "apiToken": "secret",
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "messages": []
    }))
    .unwrap();
    agent_input.context = Some(AgentRunContext {
        collaboration_identity: None,
        conversation_id: Some("conversation-cancel-before-spawn".to_string()),
        project_id: None,
        workspace: Some(AgentWorkspaceContext {
            project_id: None,
            display_name: Some("test".to_string()),
            root_path: Some(fixture.path().to_string_lossy().into_owned()),
        }),
        attachment_library: None,
        permissions: AgentPermissions::default(),
    });
    checkpoint.run_context = agent_input.context.clone();
    agent_input.resume_checkpoint = Some(checkpoint);
    save_test_pending_provider_for_input(&storage, &mut agent_input);
    service
        .store_pending_action(
            "run-cancel-before-spawn",
            "conversation-cancel-before-spawn",
            "assistant-cancel-before-spawn",
            AgentProposedAction::Command {
                command: command.clone(),
            },
            agent_input,
        )
        .unwrap();

    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let approved = service
        .approve_action("run-cancel-before-spawn", &command.id, notifications)
        .unwrap();
    assert_eq!(approved.status, "approved");
    assert_eq!(
        approved.agent_output.status,
        AgentRunStatus::Running,
        "approval must end waiting_for_approval before the command process finishes"
    );
    assert!(service
        .cancel_action("run-cancel-before-spawn", &command.id)
        .unwrap());
    assert!(!fixture.path().join("cancelled-before-spawn").exists());

    let mut saw_cancelled_done = false;
    let mut saw_cancelled_tool_result = false;
    for _ in 0..100 {
        while let Ok(notification) = receiver.try_recv() {
            if notification["params"]["type"] == "tool_result" {
                assert_eq!(notification["params"]["result"]["ok"], false);
                assert_eq!(
                    notification["params"]["result"]["result"]["cancelled"],
                    true
                );
                saw_cancelled_tool_result = true;
            }
            if notification["params"]["type"] == "done"
                && notification["params"]["status"] == "cancelled"
            {
                saw_cancelled_done = true;
            }
        }
        if saw_cancelled_done {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(saw_cancelled_done);
    assert!(saw_cancelled_tool_result);
    assert!(!fixture.path().join("cancelled-before-spawn").exists());
}

#[tokio::test(flavor = "current_thread")]
async fn message_deletion_cancels_a_rejected_actions_pre_spawn_continuation() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let conversation_id = "conversation-delete-inline-continuation";
    let assistant_message_id = "assistant-delete-inline-continuation";
    let run_id = "run-delete-inline-continuation";
    let action_id = "action-delete-inline-continuation";
    storage
        .save_conversation(ChatConversationRecord {
            id: conversation_id.to_string(),
            project_id: None,
            model_id: Some("model-1".to_string()),
            title: "Delete inline continuation".to_string(),
            messages: vec![ChatMessageRecord {
                id: assistant_message_id.to_string(),
                role: "assistant".to_string(),
                content: String::new(),
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
        })
        .unwrap();

    let call = AgentToolCall {
        id: action_id.to_string(),
        tool: "approval_tool".to_string(),
        args: json!({ "operation": "no-op" }),
        approval_status: AgentApprovalStatus::Required,
        reason: Some("exercise the pre-spawn continuation lease".to_string()),
    };
    let mut checkpoint = AgentRunCheckpoint {
        version: AGENT_RUN_CHECKPOINT_SCHEMA_VERSION,
        run_id: run_id.to_string(),
        pending_action_id: None,
        context_items: vec![pending_checkpoint_context_item(&call)],
        next_model_request_index: 1,
        queued_tool_calls: Vec::new(),
        deferred_external_tool_call_count: 0,
        suppressed_narration: false,
        extension_snapshots: Vec::new(),
        tool_set: crate::test_tool_set_checkpoint(),
        run_context: None,
        collaboration_run_snapshot: None,
        model_capabilities: ModelCapabilities::default(),
        provider_profile_config: crate::test_provider_profile_config(),
        provider_protocol_key: crate::test_provider_protocol_key("test-model"),
        assistant_turn_identity: crate::test_assistant_turn_identity(&[call.id.as_str()]),
        provider_continuation_refs: Vec::new(),
        run_world_state: crate::test_run_world_state(),
        pending_tool_call_id: call.id.clone(),
        conversation_model_context_items: vec![pending_model_context_item(&call)],
        conversation_trace_items: vec![ConversationTurnTraceItem::ToolCall {
            sequence: 0,
            call_id: call.id.clone(),
            tool: call.tool.clone(),
            operation: call.args.clone(),
            provenance: AgentToolIdentity::Builtin {
                tool_name: call.tool.clone(),
            },
            approval_status: AgentApprovalStatus::Required,
            truncated: false,
        }],
        next_conversation_trace_sequence: 1,
        conversation_trace_truncated: false,
    };
    let mut agent_input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://must-not-be-called.test/v1/chat/completions",
        "apiToken": "secret",
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "messages": []
    }))
    .unwrap();
    agent_input.context = Some(AgentRunContext {
        collaboration_identity: None,
        conversation_id: Some(conversation_id.to_string()),
        project_id: None,
        workspace: Some(AgentWorkspaceContext {
            project_id: None,
            display_name: Some("test".to_string()),
            root_path: Some(fixture.path().to_string_lossy().into_owned()),
        }),
        attachment_library: None,
        permissions: AgentPermissions::default(),
    });
    checkpoint.run_context = agent_input.context.clone();
    agent_input.resume_checkpoint = Some(checkpoint);

    save_test_pending_provider_for_input(&storage, &mut agent_input);
    let service = AgentService::new(Arc::clone(&storage));
    service
        .store_pending_action(
            run_id,
            conversation_id,
            assistant_message_id,
            AgentProposedAction::ToolCall { call: call.clone() },
            agent_input,
        )
        .unwrap();
    let stale_turn_cancellation = AgentCancellationToken::new();
    service.register_cancellation(run_id, stale_turn_cancellation.clone());
    let (notifications, _receiver) = tokio::sync::mpsc::unbounded_channel();
    service
        .reject_action(
            run_id,
            action_id,
            Some("not approved".to_string()),
            notifications,
        )
        .unwrap();

    // The original model worker may finish after the approval decision has already registered
    // the continuation token. Its cleanup must not remove that newer registration by run ID.
    service.unregister_cancellation_if_current(run_id, &stale_turn_cancellation);

    // A current-thread runtime cannot poll the spawned continuation until this function yields.
    // The lease therefore proves it was registered synchronously, before queue_action_continuation
    // returned and before deletion could acquire its lifecycle marker.
    assert!(service.process_runs.has_active_run(run_id));
    assert!(service
        .cancellations
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .contains_key(run_id));
    let deletion_error = service
        .delete_chat_messages(conversation_id, &[assistant_message_id.to_string()])
        .unwrap_err();
    assert!(deletion_error.contains("cancelled agent runs did not reach a safe terminal boundary"));
    assert_eq!(
        storage
            .load_conversation(conversation_id)
            .unwrap()
            .unwrap()
            .messages
            .len(),
        1
    );

    tokio::task::yield_now().await;
    for _ in 0..100 {
        let cancellation_active = service
            .cancellations
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .contains_key(run_id);
        if !cancellation_active && !service.process_runs.has_active_run(run_id) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(!service.process_runs.has_active_run(run_id));
    assert!(!service
        .cancellations
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .contains_key(run_id));

    service
        .delete_chat_messages(conversation_id, &[assistant_message_id.to_string()])
        .unwrap();
    assert!(storage
        .load_conversation(conversation_id)
        .unwrap()
        .unwrap()
        .messages
        .is_empty());
    assert!(storage
        .get_conversation_turn_trace(assistant_message_id)
        .unwrap()
        .is_none());
    assert!(service.list_pending_actions().is_empty());
    assert!(service
        .unsettled_file_effect_ids_for_conversation(conversation_id)
        .is_empty());
}

#[tokio::test]
async fn cancelling_run_during_approved_command_finishes_cancelled_without_resuming_model() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    storage
        .save_conversation(ChatConversationRecord {
            id: "conversation-command-cancel".to_string(),
            project_id: None,
            model_id: Some("model-1".to_string()),
            title: "Command cancellation".to_string(),
            messages: vec![ChatMessageRecord {
                id: "assistant-command-cancel".to_string(),
                role: "assistant".to_string(),
                content: String::new(),
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
        })
        .unwrap();
    let service = AgentService::new(storage.clone());
    service.register_usage_context(
        "run-command-cancel",
        AgentRunUsageContext {
            conversation_id: "conversation-command-cancel".to_string(),
            assistant_message_id: "assistant-command-cancel".to_string(),
            run_id: "run-command-cancel".to_string(),
            project_id: None,
            model_id: "model-1".to_string(),
            model_name: "Model 1".to_string(),
            provider_usage_semantics: ProviderUsageSemantics::StandardAdditive,
            input_price: None,
            cached_input_price: None,
            output_price: None,
            started_at: 1,
        },
    );
    let command = AgentCommandRequest {
        id: "command-cancel".to_string(),
        command: "sleep 5".to_string(),
        cwd: None,
        timeout_ms: Some(10_000),
        approval_status: AgentApprovalStatus::Approved,
        risk_level: Some(AgentCommandRiskLevel::ReadOnly),
        reason: Some("exercise cancellation".to_string()),
        observe: None,
        inputs: Vec::new(),
        runtime_binding: None,
        managed_office_script: None,
    };
    let call = checkpoint_call_for_command(&command);
    let mut checkpoint = AgentRunCheckpoint {
        version: AGENT_RUN_CHECKPOINT_SCHEMA_VERSION,
        run_id: "run-command-cancel".to_string(),
        pending_action_id: None,
        context_items: vec![pending_checkpoint_context_item(&call)],
        next_model_request_index: 1,
        queued_tool_calls: Vec::new(),
        deferred_external_tool_call_count: 0,
        suppressed_narration: false,
        extension_snapshots: Vec::new(),
        tool_set: crate::test_tool_set_checkpoint(),
        run_context: None,
        collaboration_run_snapshot: None,
        model_capabilities: ModelCapabilities::default(),
        provider_profile_config: crate::test_provider_profile_config(),
        provider_protocol_key: crate::test_provider_protocol_key("test-model"),
        assistant_turn_identity: crate::test_assistant_turn_identity(&[call.id.as_str()]),
        provider_continuation_refs: Vec::new(),
        run_world_state: crate::test_run_world_state(),
        pending_tool_call_id: call.id.clone(),
        conversation_model_context_items: vec![pending_model_context_item(&call)],
        conversation_trace_items: vec![ConversationTurnTraceItem::ToolCall {
            sequence: 0,
            call_id: call.id.clone(),
            tool: call.tool.clone(),
            operation: call.args.clone(),
            provenance: AgentToolIdentity::Builtin {
                tool_name: call.tool.clone(),
            },
            approval_status: AgentApprovalStatus::Required,
            truncated: false,
        }],
        next_conversation_trace_sequence: 1,
        conversation_trace_truncated: false,
    };
    let mut agent_input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://should-not-be-called.test/v1/chat/completions",
        "apiToken": "secret",
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "messages": []
    }))
    .unwrap();
    agent_input.context = Some(AgentRunContext {
        collaboration_identity: None,
        conversation_id: Some("conversation-command-cancel".to_string()),
        project_id: None,
        workspace: Some(AgentWorkspaceContext {
            project_id: None,
            display_name: Some("test".to_string()),
            root_path: Some(fixture.path().to_string_lossy().into_owned()),
        }),
        attachment_library: None,
        permissions: AgentPermissions::default(),
    });
    checkpoint.run_context = agent_input.context.clone();
    agent_input.resume_checkpoint = Some(checkpoint);
    save_test_pending_provider_for_input(&storage, &mut agent_input);
    let record = PendingActionRecord {
        storage_id: pending_action_storage_id("run-command-cancel", &command.id),
        snapshot: PendingAgentActionSnapshot {
            action_id: command.id.clone(),
            action_type: "command".to_string(),
            tool_name: "run_command".to_string(),
            tool_call_id: Some(command.id.clone()),
            run_id: "run-command-cancel".to_string(),
            conversation_id: Some("conversation-command-cancel".to_string()),
            assistant_message_id: Some("assistant-command-cancel".to_string()),
            action: AgentProposedAction::Command {
                command: command.clone(),
            },
            created_at: 1,
            status: PendingActionStatus::Approved,
        },
        agent_input,
    };
    assert_eq!(
        service.persist_pending_action(&record).unwrap(),
        PendingActionStoreOutcome::Inserted
    );
    service
        .pending_actions
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .insert(record.storage_id.clone(), record.clone());
    let (notifications, _receiver) = tokio::sync::mpsc::unbounded_channel();
    let guard = service
        .process_runs
        .register(&record.storage_id, &record.snapshot.run_id);
    let execution_service = service.clone();
    let task = tokio::spawn(async move {
        execution_service
            .run_command_execution(record, call, guard, notifications)
            .await;
    });

    let mut cancelled = false;
    for _ in 0..100 {
        if service.cancel_run("run-command-cancel") {
            cancelled = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(cancelled);
    task.await.unwrap();

    let trace = storage
        .get_conversation_turn_trace("assistant-command-cancel")
        .unwrap()
        .unwrap();
    trace.validate().unwrap();
    assert_eq!(
        trace.terminal_status,
        ConversationTurnTraceTerminalStatus::Cancelled
    );
    assert!(matches!(
        trace.items.last(),
        Some(ConversationTurnTraceItem::ToolResult {
            status: ConversationTraceToolResultStatus::Cancelled,
            archive,
            ..
        }) if archive.archived_completely == Some(true)
            && archive.archive_ref.is_some()
    ));
    let model_log = storage
        .get_conversation_model_context_log("assistant-command-cancel")
        .unwrap()
        .expect("cancelled approved command model log");
    assert!(model_log
        .items
        .iter()
        .any(|item| item.tool_call_id.as_deref() == Some("command-cancel")));
}
