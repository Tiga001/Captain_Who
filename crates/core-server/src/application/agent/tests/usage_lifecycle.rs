use super::*;

#[test]
fn persists_usage_for_failed_runs() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    storage
        .save_conversation(ChatConversationRecord {
            id: "conversation-1".to_string(),
            project_id: None,
            model_id: Some("model-1".to_string()),
            title: "Usage test".to_string(),
            messages: vec![ChatMessageRecord {
                id: "assistant-1".to_string(),
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
    let service = AgentService::new(storage);
    service.register_usage_context(
        "run-1",
        AgentRunUsageContext {
            conversation_id: "conversation-1".to_string(),
            assistant_message_id: "assistant-1".to_string(),
            run_id: "run-1".to_string(),
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

    service
        .persist_run_usage(
            "run-1",
            AgentRunStatus::Failed,
            Some(AgentUsage {
                input_tokens: Some(20),
                output_tokens: Some(8),
                output_thinking_tokens: None,
                total_tokens: Some(28),
                cached_input_tokens: None,
                cache_creation_input_tokens: None,
                billable_request_count: Some(3),
            }),
            Some("invalid tool arguments".to_string()),
        )
        .unwrap();

    let summary = service
        .get_usage_summary(&AgentUsageSummaryInput {
            range: AgentUsageSummaryRange::All,
            from: None,
            to: None,
        })
        .unwrap();
    assert_eq!(summary.request_count, 3);
    assert_eq!(summary.input_tokens, Some(20));
    assert_eq!(summary.output_tokens, Some(8));
    assert_eq!(summary.total_tokens, Some(28));
}

#[test]
fn approval_segments_project_one_cumulative_usage_snapshot_to_chat_history() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    storage
        .save_conversation(ChatConversationRecord {
            id: "conversation-cumulative".to_string(),
            project_id: None,
            model_id: Some("model-1".to_string()),
            title: "Cumulative usage".to_string(),
            messages: vec![ChatMessageRecord {
                id: "assistant-cumulative".to_string(),
                role: "assistant".to_string(),
                content: String::new(),
                created_at: 1,
                status: Some("pending".to_string()),
                attachments: Vec::new(),
                agent_run_json: Some(
                    json!({
                        "runId": "run-cumulative",
                        "status": "running",
                        "usage": {
                            "inputTokens": 0,
                            "outputTokens": 0,
                            "totalTokens": 0,
                            "billableRequestCount": 0
                        },
                        "startedAt": 1,
                        "toolDefinitions": [],
                        "toolCalls": [],
                        "toolResults": [],
                        "webSearchActivities": [],
                        "readActivities": [],
                        "approvals": [],
                        "diffs": [],
                        "fileDrafts": [],
                        "mcpInvocations": [],
                        "messageStreamCheckpoints": {},
                        "timeline": [{
                            "id": "keep-presentation",
                            "type": "message",
                            "content": "Presentation remains intact."
                        }],
                        "state": {
                            "status": "running",
                            "activeRunId": "run-cumulative",
                            "lastError": null,
                            "updatedAt": 1
                        }
                    })
                    .to_string(),
                ),
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
        "run-cumulative",
        AgentRunUsageContext {
            conversation_id: "conversation-cumulative".to_string(),
            assistant_message_id: "assistant-cumulative".to_string(),
            run_id: "run-cumulative".to_string(),
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

    let mut waiting = usage_output(
        AgentRunStatus::WaitingForApproval,
        AgentUsage {
            input_tokens: Some(100),
            output_tokens: Some(20),
            output_thinking_tokens: Some(8),
            total_tokens: Some(120),
            cached_input_tokens: Some(5),
            cache_creation_input_tokens: None,
            billable_request_count: Some(2),
        },
    );
    service
        .persist_final_assistant_output(
            "conversation-cumulative",
            "assistant-cumulative",
            &mut waiting,
        )
        .unwrap();

    let continuation_usage = AgentUsage {
        input_tokens: Some(60),
        output_tokens: Some(10),
        output_thinking_tokens: Some(4),
        total_tokens: Some(70),
        cached_input_tokens: Some(3),
        cache_creation_input_tokens: Some(2),
        billable_request_count: Some(1),
    };
    let projected = service.project_cumulative_usage_onto_event(AgentEvent::Done {
        run_id: "run-cumulative".to_string(),
        success: true,
        status: Some(AgentRunStatus::Completed),
        content: Some("done".to_string()),
        usage: Some(continuation_usage.clone()),
        finish_reason: Some("stop".to_string()),
        proposed_actions: Vec::new(),
    });
    assert!(matches!(
        projected,
        AgentEvent::Done {
            usage: Some(AgentUsage {
                input_tokens: Some(160),
                output_tokens: Some(30),
                total_tokens: Some(190),
                billable_request_count: Some(3),
                ..
            }),
            ..
        }
    ));

    let mut completed = usage_output(AgentRunStatus::Completed, continuation_usage);
    service
        .persist_final_assistant_output(
            "conversation-cumulative",
            "assistant-cumulative",
            &mut completed,
        )
        .unwrap();

    assert_eq!(
        completed.usage,
        Some(AgentUsage {
            input_tokens: Some(160),
            output_tokens: Some(30),
            output_thinking_tokens: Some(12),
            total_tokens: Some(190),
            cached_input_tokens: Some(8),
            cache_creation_input_tokens: Some(2),
            billable_request_count: Some(3),
        })
    );
    let summary = service
        .get_usage_summary(&AgentUsageSummaryInput {
            range: AgentUsageSummaryRange::All,
            from: None,
            to: None,
        })
        .unwrap();
    assert_eq!(summary.request_count, 3);
    assert_eq!(summary.input_tokens, Some(160));
    assert_eq!(summary.output_tokens, Some(30));
    assert_eq!(summary.total_tokens, Some(190));

    let conversation = storage
        .load_conversation("conversation-cumulative")
        .unwrap()
        .unwrap();
    let run: Value =
        serde_json::from_str(conversation.messages[0].agent_run_json.as_deref().unwrap()).unwrap();
    assert_eq!(run["usage"]["inputTokens"], 160);
    assert_eq!(run["usage"]["outputTokens"], 30);
    assert_eq!(run["usage"]["outputThinkingTokens"], 12);
    assert_eq!(run["usage"]["totalTokens"], 190);
    assert_eq!(run["usage"]["cachedInputTokens"], 8);
    assert_eq!(run["usage"]["cacheCreationInputTokens"], 2);
    assert_eq!(run["usage"]["billableRequestCount"], 3);
    assert_eq!(run["timeline"][0]["id"], "keep-presentation");
}

fn usage_output(status: AgentRunStatus, usage: AgentUsage) -> AgentChatOutput {
    AgentChatOutput {
        content: "done".to_string(),
        status,
        run_id: "run-cumulative".to_string(),
        events: Vec::new(),
        tool_definitions: Vec::new(),
        todo: None,
        usage: Some(usage),
        finish_reason: Some("stop".to_string()),
        proposed_actions: Vec::new(),
        conversation_turn_trace: None,
    }
}

#[test]
fn deleting_project_cancels_runs_and_discards_usage_contexts() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let service = AgentService::new(storage);
    let cancellation = AgentCancellationToken::new();
    service.register_cancellation("run-1", cancellation.clone());
    service.register_usage_context(
        "run-1",
        AgentRunUsageContext {
            conversation_id: "conversation-1".to_string(),
            assistant_message_id: "assistant-1".to_string(),
            run_id: "run-1".to_string(),
            project_id: Some("project-1".to_string()),
            model_id: "model-1".to_string(),
            model_name: "Model 1".to_string(),
            provider_usage_semantics: ProviderUsageSemantics::StandardAdditive,
            input_price: None,
            cached_input_price: None,
            output_price: None,
            started_at: 1,
        },
    );

    service.delete_project("project-1").unwrap();

    assert!(cancellation.is_cancelled());
    assert!(service.is_project_deleting(Some("project-1")));
    assert!(service
        .usage_contexts
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .is_empty());
}

#[test]
fn failed_project_deletion_releases_the_command_finalization_barrier() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let service = AgentService::new(storage);
    inject_project_deletion_failure("project-delete-failure");

    let error = service
        .delete_project("project-delete-failure")
        .unwrap_err();

    assert!(error.contains("injected project deletion failure"));
    assert!(!service.is_project_deleting(Some("project-delete-failure")));
}

#[test]
fn pending_approval_persists_full_run_checkpoint() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    save_test_pending_provider(
        &storage,
        "test-model",
        "https://example.test/v1/chat/completions",
        "secret",
        "disabled",
        "",
    );
    let service = AgentService::new(storage.clone());
    let base_input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "secret",
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "contextWindowTokens": 128000,
        "messages": []
    }))
    .unwrap();
    let run_checkpoint = AgentRunCheckpoint {
        version: AGENT_RUN_CHECKPOINT_SCHEMA_VERSION,
        run_id: "run-checkpoint".to_string(),
        pending_action_id: None,
        context_items: vec![
            mycopilot_core::AgentContextCheckpointItem {
                role: "system".to_string(),
                content: "rules".to_string(),
                images: Vec::new(),
                tool_call_id: None,
                tool_calls: Vec::new(),
                is_error: false,
                sources: vec!["backend_system_prompt".to_string()],
                scope: "run".to_string(),
                retention: "retained".to_string(),
                group: None,
                origin: None,
            },
            mycopilot_core::AgentContextCheckpointItem {
                role: "assistant".to_string(),
                content: String::new(),
                images: Vec::new(),
                tool_call_id: None,
                tool_calls: vec![mycopilot_core::AgentContextCheckpointToolCall {
                    id: "call-checkpoint".to_string(),
                    name: "apply_patch".to_string(),
                    args: json!({ "operation": "create", "filePath": "report.txt" }),
                    provider_identity: mycopilot_core::AgentProviderToolCallIdentity {
                        provider_tool_index: 0,
                        provider_call_id: "call-checkpoint".to_string(),
                        runtime_call_id: "call-checkpoint".to_string(),
                    },
                }],
                is_error: false,
                sources: vec!["model_response".to_string()],
                scope: "run".to_string(),
                retention: "retained".to_string(),
                group: Some(mycopilot_core::AgentContextCheckpointGroup {
                    id: "exchange-checkpoint".to_string(),
                    kind: "tool_exchange".to_string(),
                }),
                origin: None,
            },
        ],
        next_model_request_index: 1,
        queued_tool_calls: Vec::new(),
        deferred_external_tool_call_count: 0,
        suppressed_narration: false,
        extension_snapshots: Vec::new(),
        tool_set: crate::test_tool_set_checkpoint(),
        run_context: None,
        model_capabilities: ModelCapabilities::default(),
        provider_profile_config: crate::test_provider_profile_config(),
        provider_protocol_key: crate::test_provider_protocol_key("test-model"),
        assistant_turn_identity: crate::test_assistant_turn_identity(&["call-checkpoint"]),
        provider_continuation_refs: Vec::new(),
        run_world_state: crate::test_run_world_state(),
        pending_tool_call_id: "call-checkpoint".to_string(),
        conversation_model_context_items: Vec::new(),
        conversation_trace_items: Vec::new(),
        next_conversation_trace_sequence: 0,
        conversation_trace_truncated: false,
    };
    let mut checkpoint = agent_input_with_run_checkpoint(&base_input, &run_checkpoint);
    save_test_pending_provider_for_input(&storage, &mut checkpoint);
    let run_checkpoint = checkpoint
        .resume_checkpoint
        .clone()
        .expect("test run checkpoint remains attached");
    let action = AgentProposedAction::ToolCall {
        call: AgentToolCall {
            id: "call-checkpoint".to_string(),
            tool: "apply_patch".to_string(),
            args: json!({ "operation": "create", "filePath": "report.txt" }),
            approval_status: AgentApprovalStatus::Required,
            reason: None,
        },
    };

    service
        .store_pending_action(
            "run-checkpoint",
            "conversation-checkpoint",
            "assistant-checkpoint",
            action,
            checkpoint,
        )
        .unwrap();
    assert!(base_input.resume_checkpoint.is_none());

    let reloaded = AgentService::new(storage);
    {
        let usage_contexts = reloaded
            .usage_contexts
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let restored = usage_contexts.get("run-checkpoint").unwrap();
        assert_eq!(
            restored.context.provider_usage_semantics,
            ProviderUsageSemantics::StandardAdditive
        );
        assert!(restored.usage.is_none());
        assert!(restored.context.input_price.is_none());
        assert!(restored.context.output_price.is_none());
    }
    let pending = reloaded
        .pending_actions
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let record = pending
        .get(&pending_action_storage_id(
            "run-checkpoint",
            "call-checkpoint",
        ))
        .unwrap();
    assert_eq!(
        record.agent_input.resume_checkpoint.as_ref(),
        Some(&run_checkpoint)
    );
    assert!(record.agent_input.messages.is_empty());
    assert!(record.agent_input.attachments.is_empty());
    assert!(!record.agent_input.api_token.is_empty());
    assert_eq!(record.agent_input.context_window_tokens, Some(128_000));
}
