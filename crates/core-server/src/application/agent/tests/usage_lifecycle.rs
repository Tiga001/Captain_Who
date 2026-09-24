use super::file_change_permissions::{
    bind_direct_execution_to_input, seed_durable_direct_file_change_owner,
};
use super::*;

fn seed_terminal_retry_pending_action(
    storage: &StorageService,
    run_id: &str,
    conversation_id: &str,
    assistant_message_id: &str,
    action_id: &str,
) -> String {
    let storage_id = pending_action_storage_id(run_id, action_id);
    storage
        .store_pending_agent_action(AgentPendingActionRecord {
            action_id: storage_id.clone(),
            run_id: run_id.to_string(),
            conversation_id: Some(conversation_id.to_string()),
            assistant_message_id: Some(assistant_message_id.to_string()),
            action_type: "tool_call".to_string(),
            tool_name: "approval_retry_fixture".to_string(),
            tool_call_id: Some(action_id.to_string()),
            status: "approved".to_string(),
            target_status: Some("completed".to_string()),
            action_json: "{}".to_string(),
            agent_input_json: "{}".to_string(),
            created_at: 1,
            updated_at: 1,
        })
        .unwrap();
    storage_id
}

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
                human_interaction_response: None,
                id: "assistant-1".to_string(),
                role: "assistant".to_string(),
                content: String::new(),
                created_at: 1,
                status: Some("pending".to_string()),
                attachments: Vec::new(),
                folder_references_json: None,
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
    let service = AgentService::new_authorized_for_test(storage);
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
fn non_error_usage_transition_clears_a_stale_error() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let service = AgentService::new_authorized_for_test(storage);
    service.register_usage_context(
        "run-clear-stale-error",
        AgentRunUsageContext {
            conversation_id: "conversation-clear-stale-error".to_string(),
            assistant_message_id: "assistant-clear-stale-error".to_string(),
            run_id: "run-clear-stale-error".to_string(),
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

    let stale = service
        .prepare_run_usage_record(
            "run-clear-stale-error",
            AgentRunStatus::WaitingForApproval,
            None,
            Some("tool_calls".to_string()),
        )
        .unwrap();
    assert_eq!(stale.error.as_deref(), Some("tool_calls"));

    let completed = service
        .prepare_run_usage_record(
            "run-clear-stale-error",
            AgentRunStatus::Completed,
            None,
            None,
        )
        .unwrap();
    assert_eq!(completed.error, None);
}

#[test]
fn moonshot_completion_usage_is_priced_persisted_and_summarized_as_output() {
    const CONVERSATION_ID: &str = "conversation-moonshot-usage";
    const ASSISTANT_MESSAGE_ID: &str = "assistant-moonshot-usage";
    const RUN_ID: &str = "run-moonshot-usage";

    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    storage
        .save_conversation(ChatConversationRecord {
            id: CONVERSATION_ID.to_string(),
            project_id: None,
            model_id: Some("kimi-k3".to_string()),
            title: "Moonshot usage".to_string(),
            messages: vec![ChatMessageRecord {
                human_interaction_response: None,
                id: ASSISTANT_MESSAGE_ID.to_string(),
                role: "assistant".to_string(),
                content: "done".to_string(),
                created_at: 1,
                status: Some("sent".to_string()),
                attachments: Vec::new(),
                folder_references_json: None,
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

    let moonshot_usage_semantics = mycopilot_core::resolve_provider_vendor_registration(
        mycopilot_core::ProviderVendorId::Moonshot,
        "kimi-k3",
        ProviderProtocolDialect::OpenAiChatCompletions,
    )
    .unwrap()
    .runtime_capabilities()
    .usage();
    assert_eq!(
        moonshot_usage_semantics,
        ProviderUsageSemantics::StandardAdditive
    );

    let service = AgentService::new_authorized_for_test(Arc::clone(&storage));
    service.register_usage_context(
        RUN_ID,
        AgentRunUsageContext {
            conversation_id: CONVERSATION_ID.to_string(),
            assistant_message_id: ASSISTANT_MESSAGE_ID.to_string(),
            run_id: RUN_ID.to_string(),
            project_id: None,
            model_id: "kimi-k3".to_string(),
            model_name: "Kimi K3".to_string(),
            provider_usage_semantics: moonshot_usage_semantics,
            input_price: Some("0.01".to_string()),
            cached_input_price: Some("0.002".to_string()),
            output_price: Some("0.02".to_string()),
            started_at: 1,
        },
    );
    service
        .persist_run_usage(
            RUN_ID,
            AgentRunStatus::Completed,
            Some(AgentUsage {
                input_tokens: Some(1_000),
                output_tokens: Some(250),
                output_thinking_tokens: None,
                total_tokens: Some(1_250),
                cached_input_tokens: Some(400),
                cache_creation_input_tokens: None,
                billable_request_count: Some(1),
            }),
            None,
        )
        .unwrap();

    let persisted = storage
        .load_agent_usage_for_owner(RUN_ID, CONVERSATION_ID, ASSISTANT_MESSAGE_ID)
        .unwrap()
        .unwrap();
    assert_eq!(persisted.output_tokens, Some(250));
    assert_eq!(persisted.output_thinking_tokens, None);
    assert_eq!(persisted.cached_input_tokens, Some(400));
    assert_eq!(persisted.total_tokens, Some(1_250));
    assert!(
        (persisted.estimated_cost.unwrap() - 0.0118).abs() < f64::EPSILON,
        "Moonshot completion output must contribute its 0.005 output-cost share"
    );

    let summary = service
        .get_usage_summary(&AgentUsageSummaryInput {
            range: AgentUsageSummaryRange::All,
            from: None,
            to: None,
        })
        .unwrap();
    assert_eq!(summary.output_tokens, Some(250));
    assert_eq!(summary.output_thinking_tokens, None);
    assert_eq!(summary.cached_input_tokens, Some(400));
    assert_eq!(summary.total_tokens, Some(1_250));
    assert!((summary.estimated_cost.unwrap() - 0.0118).abs() < f64::EPSILON);
    assert_eq!(summary.models.len(), 1);
    assert_eq!(summary.models[0].model_id, "kimi-k3");
    assert_eq!(summary.models[0].output_tokens, Some(250));
    assert!((summary.models[0].estimated_cost.unwrap() - 0.0118).abs() < f64::EPSILON);
}

#[test]
fn model_request_interruption_settles_visible_message_without_losing_failed_audit() {
    for (reason, partial_response) in [
        (
            AgentModelRequestInterruptionReason::ServiceConnectionFailed,
            "",
        ),
        (
            AgentModelRequestInterruptionReason::OutputLimitReached,
            "已生成但未完成的正文",
        ),
        (AgentModelRequestInterruptionReason::OutputLimitReached, ""),
        (AgentModelRequestInterruptionReason::EmptyResponse, ""),
        (
            AgentModelRequestInterruptionReason::StreamInterrupted,
            "断流前的正文",
        ),
    ] {
        const CONVERSATION_ID: &str = "conversation-model-interruption";
        const ASSISTANT_MESSAGE_ID: &str = "assistant-model-interruption";
        const RUN_ID: &str = "run-model-interruption";
        const DIAGNOSTIC: &str = "provider transport diagnostic";

        let fixture = tempdir().unwrap();
        let storage =
            Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
        storage
            .save_conversation(ChatConversationRecord {
                id: CONVERSATION_ID.to_string(),
                project_id: None,
                model_id: Some("model-1".to_string()),
                title: "Model interruption".to_string(),
                messages: vec![ChatMessageRecord {
                    human_interaction_response: None,
                    id: ASSISTANT_MESSAGE_ID.to_string(),
                    role: "assistant".to_string(),
                    content: "provisional failed sampling".to_string(),
                    created_at: 1,
                    status: Some("pending".to_string()),
                    attachments: Vec::new(),
                    folder_references_json: None,
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
        storage
            .append_in_progress_conversation_turn_trace(
                &mycopilot_core::ConversationTraceSnapshot::default().in_progress_trace(
                    RUN_ID,
                    CONVERSATION_ID,
                    ASSISTANT_MESSAGE_ID,
                ),
                1,
                1,
            )
            .unwrap();

        let service = AgentService::new_authorized_for_test(Arc::clone(&storage));
        service.register_usage_context(
            RUN_ID,
            AgentRunUsageContext {
                conversation_id: CONVERSATION_ID.to_string(),
                assistant_message_id: ASSISTANT_MESSAGE_ID.to_string(),
                run_id: RUN_ID.to_string(),
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
        let usage = AgentUsage {
            input_tokens: Some(11),
            output_tokens: Some(2),
            output_thinking_tokens: None,
            total_tokens: Some(13),
            cached_input_tokens: None,
            cache_creation_input_tokens: None,
            billable_request_count: Some(1),
        };
        let trace = failed_conversation_trace_without_items(
            RUN_ID,
            CONVERSATION_ID,
            ASSISTANT_MESSAGE_ID,
            DIAGNOSTIC,
        );

        service
            .persist_assistant_model_request_interruption(
                CONVERSATION_ID,
                ASSISTANT_MESSAGE_ID,
                DIAGNOSTIC,
                Some(usage),
                &trace,
                reason,
                partial_response,
            )
            .unwrap();

        let conversation = storage.load_conversation(CONVERSATION_ID).unwrap().unwrap();
        let message = &conversation.messages[0];
        assert_eq!(message.content, partial_response);
        assert_eq!(message.status.as_deref(), Some("sent"));
        let projection: serde_json::Value =
            serde_json::from_str(message.agent_run_json.as_deref().unwrap()).unwrap();
        assert_eq!(projection["interruption"]["reason"], reason.as_str());
        assert!(projection.get("error").is_none());
        assert!(!projection["timeline"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["type"] == "error"));

        // Reopening the database must reconstruct the reason without a live renderer event.
        let reopened = StorageService::open(&fixture.path().join("storage.sqlite")).unwrap();
        let reloaded = reopened
            .load_conversation(CONVERSATION_ID)
            .unwrap()
            .unwrap();
        let reloaded_projection: serde_json::Value =
            serde_json::from_str(reloaded.messages[0].agent_run_json.as_deref().unwrap()).unwrap();
        assert_eq!(
            reloaded_projection["interruption"]["reason"],
            reason.as_str()
        );
        assert_eq!(reloaded.messages[0].content, partial_response);

        let persisted_trace = storage
            .get_conversation_turn_trace(ASSISTANT_MESSAGE_ID)
            .unwrap()
            .unwrap();
        assert_eq!(
            persisted_trace.terminal_status,
            ConversationTurnTraceTerminalStatus::Failed
        );
        assert_eq!(persisted_trace.terminal_error.as_deref(), Some(DIAGNOSTIC));

        let persisted_usage = storage
            .load_agent_usage_for_owner(RUN_ID, CONVERSATION_ID, ASSISTANT_MESSAGE_ID)
            .unwrap()
            .unwrap();
        assert_eq!(persisted_usage.status.as_deref(), Some("failed"));
        assert_eq!(persisted_usage.error.as_deref(), Some(DIAGNOSTIC));
    }
}

#[test]
fn failed_terminal_settlement_closes_a_durable_open_tool_call_with_paired_context() {
    const CONVERSATION_ID: &str = "conversation-failed-open-tool";
    const ASSISTANT_MESSAGE_ID: &str = "assistant-failed-open-tool";
    const RUN_ID: &str = "run-failed-open-tool";
    const CALL_ID: &str = "call-failed-open-tool";
    const FAILURE: &str = "tool worker stopped before returning a result";

    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    storage
        .save_conversation(ChatConversationRecord {
            id: CONVERSATION_ID.to_string(),
            project_id: None,
            model_id: Some("model-1".to_string()),
            title: "Failed open tool".to_string(),
            messages: vec![ChatMessageRecord {
                human_interaction_response: None,
                id: ASSISTANT_MESSAGE_ID.to_string(),
                role: "assistant".to_string(),
                content: String::new(),
                created_at: 1,
                status: Some("pending".to_string()),
                attachments: Vec::new(),
                folder_references_json: None,
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
        id: CALL_ID.to_string(),
        tool: "attachments_list_project".to_string(),
        args: json!({}),
        approval_status: AgentApprovalStatus::NotRequired,
        reason: None,
    };
    let snapshot = ConversationTraceSnapshot {
        items: vec![ConversationTurnTraceItem::ToolCall {
            sequence: 0,
            call_id: call.id.clone(),
            tool: call.tool.clone(),
            provenance: AgentToolIdentity::Builtin {
                tool_name: call.tool.clone(),
            },
            operation: call.args.clone(),
            approval_status: call.approval_status,
            truncated: false,
        }],
        model_context_items: vec![ConversationModelContextItem {
            images: Vec::new(),
            sequence: 0,
            ordinal: 0,
            role: "assistant".to_string(),
            content: String::new(),
            tool_call_id: None,
            tool_calls: vec![AgentContextCheckpointToolCall {
                id: call.id.clone(),
                name: call.tool.clone(),
                args: call.args.clone(),
                provider_identity: AgentProviderToolCallIdentity {
                    provider_tool_index: 0,
                    provider_call_id: call.id.clone(),
                    runtime_call_id: call.id.clone(),
                },
            }],
            is_error: false,
        }],
        next_sequence: 1,
        truncated: false,
    };
    let service = AgentService::new_authorized_for_test(Arc::clone(&storage));
    storage
        .append_in_progress_conversation_turn_trace_and_apply_guidances(
            &snapshot.in_progress_audit_trace(RUN_ID, CONVERSATION_ID, ASSISTANT_MESSAGE_ID),
            &snapshot.model_context_items,
            1,
            2,
        )
        .unwrap();

    let durable_trace = storage
        .get_conversation_turn_trace(ASSISTANT_MESSAGE_ID)
        .unwrap()
        .unwrap();
    let durable_model_context = storage
        .get_conversation_model_context_log(ASSISTANT_MESSAGE_ID)
        .unwrap()
        .unwrap()
        .items;
    let snapshot = ConversationTraceSnapshot {
        items: durable_trace.items,
        model_context_items: durable_model_context,
        next_sequence: 1,
        truncated: durable_trace.truncated,
    };
    service
        .trace_snapshots
        .lock()
        .unwrap()
        .insert(RUN_ID.to_string(), snapshot.clone());
    let runtime_terminal_trace = terminal_conversation_trace_from_snapshot(
        snapshot,
        RUN_ID,
        CONVERSATION_ID,
        ASSISTANT_MESSAGE_ID,
        ConversationTurnTraceTerminalStatus::Failed,
        FAILURE,
    )
    .unwrap()
    .trace;
    service
        .persist_assistant_error(
            CONVERSATION_ID,
            ASSISTANT_MESSAGE_ID,
            FAILURE,
            None,
            &runtime_terminal_trace,
        )
        .unwrap();

    let trace = storage
        .get_conversation_turn_trace(ASSISTANT_MESSAGE_ID)
        .unwrap()
        .unwrap();
    let model_context = storage
        .get_conversation_model_context_log(ASSISTANT_MESSAGE_ID)
        .unwrap()
        .unwrap()
        .items;
    trace
        .validate_complete_model_context(&model_context)
        .unwrap();
    assert_eq!(
        trace.terminal_status,
        ConversationTurnTraceTerminalStatus::Failed
    );
    assert!(matches!(
        trace.items.as_slice(),
        [
            ConversationTurnTraceItem::ToolCall { call_id, .. },
            ConversationTurnTraceItem::ToolResult {
                call_id: result_call_id,
                status: ConversationTraceToolResultStatus::Failed,
                success: false,
                ..
            }
        ] if call_id == CALL_ID && result_call_id == CALL_ID
    ));
    assert_eq!(model_context.len(), 2);
}

#[test]
fn precommitted_wait_result_survives_stale_or_conflicting_terminal_snapshots() {
    for cancelled in [false, true] {
        for conflicting in [false, true] {
            let fixture = tempdir().unwrap();
            let storage =
                Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
            let conversation_id = "conversation-precommitted-cleanup";
            let assistant_message_id = "assistant-precommitted-cleanup";
            let run_id = "run-precommitted-cleanup";
            storage
                .save_conversation(ChatConversationRecord {
                    id: conversation_id.into(),
                    project_id: None,
                    model_id: Some("model-1".into()),
                    title: "Precommitted cleanup".into(),
                    messages: vec![ChatMessageRecord {
                        human_interaction_response: None,
                        id: assistant_message_id.into(),
                        role: "assistant".into(),
                        content: String::new(),
                        created_at: 1,
                        status: Some("streaming".into()),
                        attachments: Vec::new(),
                        folder_references_json: None,
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
            let service = AgentService::new_authorized_for_test(Arc::clone(&storage));
            let call = AgentToolCall {
                id: "wait-call".into(),
                tool: "wait_agent".into(),
                args: json!({"targets": ["research-child"]}),
                approval_status: AgentApprovalStatus::NotRequired,
                reason: None,
            };
            let open = ConversationTraceSnapshot {
                items: vec![ConversationTurnTraceItem::ToolCall {
                    sequence: 0,
                    call_id: call.id.clone(),
                    tool: call.tool.clone(),
                    provenance: AgentToolIdentity::Builtin {
                        tool_name: call.tool.clone(),
                    },
                    operation: call.args.clone(),
                    approval_status: call.approval_status,
                    truncated: false,
                }],
                model_context_items: vec![ConversationModelContextItem {
                    images: Vec::new(),
                    sequence: 0,
                    ordinal: 0,
                    role: "assistant".into(),
                    content: String::new(),
                    tool_call_id: None,
                    is_error: false,
                    tool_calls: vec![AgentContextCheckpointToolCall {
                        id: call.id.clone(),
                        name: call.tool.clone(),
                        args: call.args.clone(),
                        provider_identity: AgentProviderToolCallIdentity {
                            provider_tool_index: 0,
                            provider_call_id: call.id.clone(),
                            runtime_call_id: call.id.clone(),
                        },
                    }],
                }],
                next_sequence: 1,
                truncated: false,
            };
            let open_trace =
                open.in_progress_audit_trace(run_id, conversation_id, assistant_message_id);
            let result = AgentToolResult {
                exact_archive_file: None,
                call_id: call.id.clone(),
                tool: call.tool.clone(),
                ok: true,
                error: None,
                result: Some(
                    json!({"targets": [{"taskName": "research-child", "messages": [{
                        "content": "学科建设与科研实力：完整研究证据与参考资料。".repeat(2_000)
                    }]}]}),
                ),
            };
            let committed = conversation_trace_snapshot_with_recovered_tool_result(
                &open_trace,
                open.model_context_items.clone(),
                &result,
            )
            .unwrap();
            storage
                .append_in_progress_conversation_turn_trace_and_apply_guidances(
                    &committed.in_progress_audit_trace(
                        run_id,
                        conversation_id,
                        assistant_message_id,
                    ),
                    &committed.model_context_items,
                    1,
                    2,
                )
                .unwrap();

            // Before the fix a failed postprocessor could leave either the old open-call cache,
            // or its synthetic failed result, while the Host had already committed success.
            let mut unpublished = if conflicting { committed.clone() } else { open };
            if conflicting {
                if let ConversationTurnTraceItem::ToolResult {
                    success,
                    status,
                    observation,
                    error,
                    ..
                } = &mut unpublished.items[1]
                {
                    *success = false;
                    *status = ConversationTraceToolResultStatus::Failed;
                    *observation = json!({"failed": true});
                    *error = Some("postprocessing failed".into());
                }
                unpublished.model_context_items[1].content = "postprocessing failed".into();
                unpublished.model_context_items[1].is_error = true;
            }
            service
                .trace_snapshots
                .lock()
                .unwrap()
                .insert(run_id.into(), unpublished);
            if cancelled {
                let mut output = AgentChatOutput {
                    content: String::new(),
                    status: AgentRunStatus::Cancelled,
                    run_id: run_id.into(),
                    events: Vec::new(),
                    tool_definitions: Vec::new(),
                    todo: None,
                    usage: None,
                    finish_reason: None,
                    proposed_actions: Vec::new(),
                    conversation_turn_trace: None,
                };
                service
                    .persist_final_assistant_output(
                        conversation_id,
                        assistant_message_id,
                        &mut output,
                        None,
                    )
                    .unwrap();
            } else {
                service
                    .persist_assistant_error(
                        conversation_id,
                        assistant_message_id,
                        "postprocessing failed",
                        None,
                        &failed_conversation_trace_without_items(
                            run_id,
                            conversation_id,
                            assistant_message_id,
                            "postprocessing failed",
                        ),
                    )
                    .unwrap();
            }
            let settled = storage
                .get_conversation_turn_trace(assistant_message_id)
                .unwrap()
                .unwrap();
            let context = storage
                .get_conversation_model_context_log(assistant_message_id)
                .unwrap()
                .unwrap()
                .items;
            assert_eq!(
                settled.items, committed.items,
                "the successful receipt is immutable"
            );
            assert_eq!(context, committed.model_context_items);
            settled.validate_complete_model_context(&context).unwrap();
            assert_eq!(
                settled.terminal_status,
                if cancelled {
                    ConversationTurnTraceTerminalStatus::Cancelled
                } else {
                    ConversationTurnTraceTerminalStatus::Failed
                }
            );
        }
    }
}

#[tokio::test]
async fn terminal_transaction_retry_reloads_sqlite_and_counts_usage_once() {
    const CONVERSATION_ID: &str = "conversation-terminal-retry";
    const ASSISTANT_MESSAGE_ID: &str = "assistant-terminal-retry";
    const RUN_ID: &str = "run-terminal-retry";

    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("storage.sqlite");
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    storage
        .save_conversation(ChatConversationRecord {
            id: CONVERSATION_ID.to_string(),
            project_id: None,
            model_id: Some("model-1".to_string()),
            title: "Terminal retry".to_string(),
            messages: vec![ChatMessageRecord {
                human_interaction_response: None,
                id: ASSISTANT_MESSAGE_ID.to_string(),
                role: "assistant".to_string(),
                content: String::new(),
                created_at: 1,
                status: Some("pending".to_string()),
                attachments: Vec::new(),
                folder_references_json: None,
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
    let service = AgentService::new_authorized_for_test(Arc::clone(&storage));
    let pending_action_id = seed_terminal_retry_pending_action(
        &storage,
        RUN_ID,
        CONVERSATION_ID,
        ASSISTANT_MESSAGE_ID,
        "approval-terminal-retry",
    );
    storage
        .append_in_progress_conversation_turn_trace(
            &mycopilot_core::ConversationTraceSnapshot::default().in_progress_trace(
                RUN_ID,
                CONVERSATION_ID,
                ASSISTANT_MESSAGE_ID,
            ),
            1,
            1,
        )
        .unwrap();
    service.register_usage_context(
        RUN_ID,
        AgentRunUsageContext {
            conversation_id: CONVERSATION_ID.to_string(),
            assistant_message_id: ASSISTANT_MESSAGE_ID.to_string(),
            run_id: RUN_ID.to_string(),
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
    let previous_usage_state = service
        .usage_contexts
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(RUN_ID)
        .cloned();

    rusqlite::Connection::open(&database_path)
        .unwrap()
        .execute_batch(
            "CREATE TRIGGER inject_terminal_usage_failure
             BEFORE INSERT ON agent_usage_records
             BEGIN
                 SELECT RAISE(ABORT, 'injected terminal usage failure');
             END;
             CREATE TRIGGER inject_pending_status_failure
             BEFORE UPDATE OF status ON agent_pending_actions
             WHEN OLD.status = 'approved'
             BEGIN
                 SELECT RAISE(ABORT, 'injected pending status failure');
             END;",
        )
        .unwrap();

    let usage = AgentUsage {
        input_tokens: Some(20),
        output_tokens: Some(8),
        output_thinking_tokens: None,
        total_tokens: Some(28),
        cached_input_tokens: None,
        cache_creation_input_tokens: None,
        billable_request_count: Some(1),
    };
    let mut output = AgentChatOutput {
        content: "terminal retry completed".to_string(),
        status: AgentRunStatus::Completed,
        run_id: RUN_ID.to_string(),
        events: Vec::new(),
        tool_definitions: Vec::new(),
        todo: None,
        usage: Some(usage.clone()),
        finish_reason: Some("stop".to_string()),
        proposed_actions: Vec::new(),
        conversation_turn_trace: None,
    };
    let rollback_count = Arc::new(Mutex::new(0_usize));
    let rollback_count_for_retry = Arc::clone(&rollback_count);

    super::super::turn_executor::persist_terminal_with_bounded_retry(
        || {
            service.persist_final_assistant_output(
                CONVERSATION_ID,
                ASSISTANT_MESSAGE_ID,
                &mut output,
                None,
            )
        },
        || {
            let mut contexts = service
                .usage_contexts
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            match &previous_usage_state {
                Some(previous) => {
                    contexts.insert(RUN_ID.to_string(), previous.clone());
                }
                None => {
                    contexts.remove(RUN_ID);
                }
            }
            drop(contexts);
            let mut count = rollback_count_for_retry
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            *count += 1;
            if *count == 1 {
                rusqlite::Connection::open(&database_path)
                    .unwrap()
                    .execute_batch("DROP TRIGGER inject_terminal_usage_failure;")
                    .unwrap();
            }
        },
    )
    .await
    .unwrap();

    let pending_rollback_count = Arc::new(Mutex::new(0_usize));
    let pending_rollback_count_for_retry = Arc::clone(&pending_rollback_count);
    super::super::turn_executor::persist_terminal_with_bounded_retry(
        || {
            storage.transition_pending_agent_action(
                &pending_action_id,
                "approved",
                "completed",
                "{}",
                mycopilot_core::storage::now_ms(),
            )
        },
        || {
            let trace = storage
                .get_conversation_turn_trace(ASSISTANT_MESSAGE_ID)
                .unwrap()
                .unwrap();
            assert_eq!(
                trace.terminal_status,
                ConversationTurnTraceTerminalStatus::Completed,
                "assistant/trace/Usage commits before the independent pending CAS retry"
            );
            assert!(storage
                .load_agent_usage_for_owner(RUN_ID, CONVERSATION_ID, ASSISTANT_MESSAGE_ID)
                .unwrap()
                .is_some());
            let mut count = pending_rollback_count_for_retry
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            *count += 1;
            if *count == 1 {
                rusqlite::Connection::open(&database_path)
                    .unwrap()
                    .execute_batch("DROP TRIGGER inject_pending_status_failure;")
                    .unwrap();
            }
        },
    )
    .await
    .unwrap();

    assert_eq!(
        *rollback_count
            .lock()
            .unwrap_or_else(|error| error.into_inner()),
        1
    );
    assert_eq!(
        *pending_rollback_count
            .lock()
            .unwrap_or_else(|error| error.into_inner()),
        1
    );
    let pending_status = rusqlite::Connection::open(&database_path)
        .unwrap()
        .query_row(
            "SELECT status FROM agent_pending_actions WHERE action_id = ?1",
            [&pending_action_id],
            |row| row.get::<_, String>(0),
        )
        .unwrap();
    assert_eq!(pending_status, "completed");
    assert_eq!(output.usage, Some(usage));
    let persisted_usage = storage
        .load_agent_usage_for_owner(RUN_ID, CONVERSATION_ID, ASSISTANT_MESSAGE_ID)
        .unwrap()
        .unwrap();
    assert_eq!(persisted_usage.total_tokens, Some(28));
    assert_eq!(persisted_usage.billable_request_count, 1);
    assert_eq!(
        storage
            .get_conversation_turn_trace(ASSISTANT_MESSAGE_ID)
            .unwrap()
            .unwrap()
            .terminal_status,
        ConversationTurnTraceTerminalStatus::Completed
    );
    assert!(storage
        .list_in_progress_conversation_turn_traces()
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn terminal_error_transaction_retry_reloads_sqlite_and_counts_usage_once() {
    const CONVERSATION_ID: &str = "conversation-terminal-error-retry";
    const ASSISTANT_MESSAGE_ID: &str = "assistant-terminal-error-retry";
    const RUN_ID: &str = "run-terminal-error-retry";
    const FAILURE: &str = "deterministic provider stream failure";

    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("storage.sqlite");
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    storage
        .save_conversation(ChatConversationRecord {
            id: CONVERSATION_ID.to_string(),
            project_id: None,
            model_id: Some("model-1".to_string()),
            title: "Terminal error retry".to_string(),
            messages: vec![ChatMessageRecord {
                human_interaction_response: None,
                id: ASSISTANT_MESSAGE_ID.to_string(),
                role: "assistant".to_string(),
                content: String::new(),
                created_at: 1,
                status: Some("pending".to_string()),
                attachments: Vec::new(),
                folder_references_json: None,
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
    let service = AgentService::new_authorized_for_test(Arc::clone(&storage));
    let pending_action_id = seed_terminal_retry_pending_action(
        &storage,
        RUN_ID,
        CONVERSATION_ID,
        ASSISTANT_MESSAGE_ID,
        "approval-terminal-error-retry",
    );
    storage
        .append_in_progress_conversation_turn_trace(
            &mycopilot_core::ConversationTraceSnapshot::default().in_progress_trace(
                RUN_ID,
                CONVERSATION_ID,
                ASSISTANT_MESSAGE_ID,
            ),
            1,
            1,
        )
        .unwrap();
    service.register_usage_context(
        RUN_ID,
        AgentRunUsageContext {
            conversation_id: CONVERSATION_ID.to_string(),
            assistant_message_id: ASSISTANT_MESSAGE_ID.to_string(),
            run_id: RUN_ID.to_string(),
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
    let previous_usage_state = service
        .usage_contexts
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(RUN_ID)
        .cloned();

    rusqlite::Connection::open(&database_path)
        .unwrap()
        .execute_batch(
            "CREATE TRIGGER inject_terminal_error_usage_failure
             BEFORE INSERT ON agent_usage_records
             BEGIN
                 SELECT RAISE(ABORT, 'injected terminal error usage failure');
             END;
             CREATE TRIGGER inject_pending_error_status_failure
             BEFORE UPDATE OF status ON agent_pending_actions
             WHEN OLD.status = 'approved'
             BEGIN
                 SELECT RAISE(ABORT, 'injected pending error status failure');
             END;",
        )
        .unwrap();

    let usage = AgentUsage {
        input_tokens: Some(20),
        output_tokens: Some(8),
        output_thinking_tokens: None,
        total_tokens: Some(28),
        cached_input_tokens: None,
        cache_creation_input_tokens: None,
        billable_request_count: Some(1),
    };
    let terminal_trace = failed_conversation_trace_without_items(
        RUN_ID,
        CONVERSATION_ID,
        ASSISTANT_MESSAGE_ID,
        FAILURE,
    );
    let rollback_count = Arc::new(Mutex::new(0_usize));
    let rollback_count_for_retry = Arc::clone(&rollback_count);

    let cumulative_usage = super::super::turn_executor::persist_terminal_with_bounded_retry(
        || {
            service.persist_assistant_error(
                CONVERSATION_ID,
                ASSISTANT_MESSAGE_ID,
                FAILURE,
                Some(usage.clone()),
                &terminal_trace,
            )
        },
        || {
            super::super::turn_executor::restore_run_usage_state(
                &service,
                RUN_ID,
                &previous_usage_state,
            );
            let mut count = rollback_count_for_retry
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            *count += 1;
            if *count == 1 {
                rusqlite::Connection::open(&database_path)
                    .unwrap()
                    .execute_batch("DROP TRIGGER inject_terminal_error_usage_failure;")
                    .unwrap();
            }
        },
    )
    .await
    .unwrap();

    let pending_rollback_count = Arc::new(Mutex::new(0_usize));
    let pending_rollback_count_for_retry = Arc::clone(&pending_rollback_count);
    super::super::turn_executor::persist_terminal_with_bounded_retry(
        || {
            storage.transition_pending_agent_action(
                &pending_action_id,
                "approved",
                "completed",
                "{}",
                mycopilot_core::storage::now_ms(),
            )
        },
        || {
            let trace = storage
                .get_conversation_turn_trace(ASSISTANT_MESSAGE_ID)
                .unwrap()
                .unwrap();
            assert_eq!(
                trace.terminal_status,
                ConversationTurnTraceTerminalStatus::Failed,
                "a failed model continuation does not rewrite the approved action outcome"
            );
            let mut count = pending_rollback_count_for_retry
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            *count += 1;
            if *count == 1 {
                rusqlite::Connection::open(&database_path)
                    .unwrap()
                    .execute_batch("DROP TRIGGER inject_pending_error_status_failure;")
                    .unwrap();
            }
        },
    )
    .await
    .unwrap();

    assert_eq!(
        *rollback_count
            .lock()
            .unwrap_or_else(|error| error.into_inner()),
        1
    );
    assert_eq!(
        *pending_rollback_count
            .lock()
            .unwrap_or_else(|error| error.into_inner()),
        1
    );
    let (pending_status, pending_target_status) = rusqlite::Connection::open(&database_path)
        .unwrap()
        .query_row(
            "SELECT status, target_status FROM agent_pending_actions WHERE action_id = ?1",
            [&pending_action_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .unwrap();
    assert_eq!(pending_status, "completed");
    assert_eq!(pending_target_status, "completed");
    assert_eq!(cumulative_usage, Some(usage));
    let persisted_usage = storage
        .load_agent_usage_for_owner(RUN_ID, CONVERSATION_ID, ASSISTANT_MESSAGE_ID)
        .unwrap()
        .unwrap();
    assert_eq!(persisted_usage.total_tokens, Some(28));
    assert_eq!(persisted_usage.billable_request_count, 1);
    assert_eq!(persisted_usage.status.as_deref(), Some("failed"));
    assert_eq!(persisted_usage.error.as_deref(), Some(FAILURE));
    let persisted_trace = storage
        .get_conversation_turn_trace(ASSISTANT_MESSAGE_ID)
        .unwrap()
        .unwrap();
    assert_eq!(
        persisted_trace.terminal_status,
        ConversationTurnTraceTerminalStatus::Failed
    );
    assert_eq!(persisted_trace.terminal_error.as_deref(), Some(FAILURE));
    assert!(storage
        .list_in_progress_conversation_turn_traces()
        .unwrap()
        .is_empty());
}

#[test]
fn sibling_conversation_usage_owners_remain_independent() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    storage
        .save_project(ProjectRecord::without_folders(
            "project-shared".to_string(),
            "Shared project".to_string(),
            1,
        ))
        .unwrap();
    for (conversation_id, assistant_message_id, model_id) in [
        ("conversation-child-a", "assistant-child-a", "model-a"),
        ("conversation-child-b", "assistant-child-b", "model-b"),
    ] {
        storage
            .save_conversation(ChatConversationRecord {
                id: conversation_id.to_string(),
                project_id: Some("project-shared".to_string()),
                model_id: Some(model_id.to_string()),
                title: conversation_id.to_string(),
                messages: vec![ChatMessageRecord {
                    human_interaction_response: None,
                    id: assistant_message_id.to_string(),
                    role: "assistant".to_string(),
                    content: String::new(),
                    created_at: 1,
                    status: Some("pending".to_string()),
                    attachments: Vec::new(),
                    folder_references_json: None,
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
    }
    let service = AgentService::new_authorized_for_test(Arc::clone(&storage));
    for (run_id, conversation_id, assistant_message_id, model_id, model_name, total_tokens) in [
        (
            "run-child-a",
            "conversation-child-a",
            "assistant-child-a",
            "model-a",
            "Model A",
            11,
        ),
        (
            "run-child-b",
            "conversation-child-b",
            "assistant-child-b",
            "model-b",
            "Model B",
            29,
        ),
    ] {
        service.register_usage_context(
            run_id,
            AgentRunUsageContext {
                conversation_id: conversation_id.to_string(),
                assistant_message_id: assistant_message_id.to_string(),
                run_id: run_id.to_string(),
                project_id: Some("project-shared".to_string()),
                model_id: model_id.to_string(),
                model_name: model_name.to_string(),
                provider_usage_semantics: ProviderUsageSemantics::StandardAdditive,
                input_price: None,
                cached_input_price: None,
                output_price: None,
                started_at: 1,
            },
        );
        service
            .persist_run_usage(
                run_id,
                AgentRunStatus::Completed,
                Some(AgentUsage {
                    input_tokens: Some(total_tokens - 1),
                    output_tokens: Some(1),
                    output_thinking_tokens: None,
                    total_tokens: Some(total_tokens),
                    cached_input_tokens: None,
                    cache_creation_input_tokens: None,
                    billable_request_count: Some(1),
                }),
                None,
            )
            .unwrap();
    }

    let child_a = storage
        .load_agent_usage_for_owner("run-child-a", "conversation-child-a", "assistant-child-a")
        .unwrap()
        .unwrap();
    let child_b = storage
        .load_agent_usage_for_owner("run-child-b", "conversation-child-b", "assistant-child-b")
        .unwrap()
        .unwrap();
    assert_eq!(
        (child_a.model_id.as_str(), child_a.total_tokens),
        ("model-a", Some(11))
    );
    assert_eq!(
        (child_b.model_id.as_str(), child_b.total_tokens),
        ("model-b", Some(29))
    );
    assert!(storage
        .load_agent_usage_for_owner("run-child-a", "conversation-child-b", "assistant-child-b",)
        .unwrap()
        .is_none());
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
                human_interaction_response: None,
                id: "assistant-cumulative".to_string(),
                role: "assistant".to_string(),
                content: String::new(),
                created_at: 1,
                status: Some("pending".to_string()),
                attachments: Vec::new(),
                folder_references_json: None,
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
                        "fileChangeProposals": [],
                        "fileChanges": [],
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
    let service = AgentService::new_authorized_for_test(storage.clone());
    let in_progress_trace = ConversationTraceSnapshot::default().in_progress_trace(
        "run-cumulative",
        "conversation-cumulative",
        "assistant-cumulative",
    );
    assert!(storage
        .append_in_progress_conversation_turn_trace(&in_progress_trace, 1, 1)
        .unwrap());
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

    let pending_call = AgentToolCall {
        id: "call-cumulative".to_string(),
        tool: "approval_tool".to_string(),
        args: json!({ "path": "safe.txt" }),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };
    let pending_action = AgentProposedAction::ToolCall {
        call: pending_call.clone(),
    };
    storage
        .store_pending_agent_action(AgentPendingActionRecord {
            action_id: pending_action_storage_id("run-cumulative", &pending_call.id),
            run_id: "run-cumulative".to_string(),
            conversation_id: Some("conversation-cumulative".to_string()),
            assistant_message_id: Some("assistant-cumulative".to_string()),
            action_type: "tool_call".to_string(),
            tool_name: pending_call.tool.clone(),
            tool_call_id: Some(pending_call.id.clone()),
            status: "pending".to_string(),
            target_status: None,
            action_json: serde_json::to_string(&pending_action).unwrap(),
            agent_input_json: "{}".to_string(),
            created_at: 1,
            updated_at: 1,
        })
        .unwrap();

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
    waiting.proposed_actions = vec![pending_action];
    service
        .stage_waiting_segment_usage("run-cumulative", waiting.usage.clone())
        .unwrap();
    service
        .persist_final_assistant_output(
            "conversation-cumulative",
            "assistant-cumulative",
            &mut waiting,
            None,
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
    assert!(
        matches!(
            &projected,
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
        ),
        "unexpected cumulative usage projection: {projected:#?}"
    );

    let mut completed = usage_output(AgentRunStatus::Completed, continuation_usage);
    service
        .persist_final_assistant_output(
            "conversation-cumulative",
            "assistant-cumulative",
            &mut completed,
            None,
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

    let persisted_usage = storage
        .load_agent_usage_for_owner(
            "run-cumulative",
            "conversation-cumulative",
            "assistant-cumulative",
        )
        .unwrap()
        .unwrap();
    assert_eq!(
        persisted_usage.error, None,
        "normal Provider finish reasons must not be stored as Agent errors"
    );
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
    let service = AgentService::new_authorized_for_test(storage);
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
    let service = AgentService::new_authorized_for_test(storage);
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
    let service = AgentService::new_authorized_for_test(storage.clone());
    let mut base_input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "secret",
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "contextWindowTokens": 128000,
        "messages": []
    }))
    .unwrap();
    let run_context = AgentRunContext {
        conversation_id: Some("conversation-checkpoint".to_string()),
        project_id: None,
        workspace: None,
        attachment_library: None,
        permissions: AgentPermissions::default(),
        collaboration_identity: None,
    };
    base_input.context = Some(run_context.clone());
    let canonical_target = fixture.path().join("report.txt");
    let (mut action, pending_call) = direct_file_change_fixture(
        "run-checkpoint",
        "conversation-checkpoint",
        "call-checkpoint",
        ("report.txt", canonical_target.to_str().unwrap()),
        None,
        Some("draft report"),
        AgentApprovalStatus::Required,
    );
    let run_checkpoint = AgentRunCheckpoint {
        pause_reason: mycopilot_core::AgentRunCheckpointPauseReason::Approval,
        version: AGENT_RUN_CHECKPOINT_SCHEMA_VERSION,
        run_id: "run-checkpoint".to_string(),
        pending_action_id: Some(pending_action_storage_id(
            "run-checkpoint",
            "call-checkpoint",
        )),
        file_change_run_grant_ref: None,
        pending_file_observation: None,
        context_items: vec![
            mycopilot_core::AgentContextCheckpointItem {
                context_image_refs: Vec::new(),
                role: "system".to_string(),
                content: "rules".to_string(),
                images: Vec::new(),
                tool_call_id: None,
                tool_calls: Vec::new(),
                is_error: false,
                sources: vec!["backend_system_prompt".to_string()],
                scope: "run".to_string(),
                retention: "retained".to_string(),
                request_order: None,
                group: None,
                origin: None,
            },
            mycopilot_core::AgentContextCheckpointItem {
                context_image_refs: Vec::new(),
                role: "assistant".to_string(),
                content: String::new(),
                images: Vec::new(),
                tool_call_id: None,
                tool_calls: vec![mycopilot_core::AgentContextCheckpointToolCall {
                    id: "call-checkpoint".to_string(),
                    name: "apply_patch".to_string(),
                    args: pending_call.args.clone(),
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
                request_order: None,
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
        run_context: Some(run_context),
        collaboration_run_snapshot: None,
        model_capabilities: ModelCapabilities::default(),
        provider_profile_config: crate::test_provider_profile_config(),
        provider_protocol_key: crate::test_provider_protocol_key("test-model"),
        assistant_turn_identity: crate::test_assistant_turn_identity(&["call-checkpoint"]),
        provider_continuation_refs: Vec::new(),
        conversation_world_state_records: Vec::new(),
        run_world_state: crate::test_run_world_state(),
        pending_tool_call_id: "call-checkpoint".to_string(),
        conversation_model_context_items: Vec::new(),
        conversation_trace_items: Vec::new(),
        next_conversation_trace_sequence: 0,
        conversation_trace_truncated: false,
    };
    let mut checkpoint = agent_input_with_run_checkpoint(&base_input, &run_checkpoint);
    save_test_pending_provider_for_input(&storage, &mut checkpoint);
    bind_direct_execution_to_input(&mut action, &checkpoint);
    seed_durable_direct_file_change_owner(
        &storage,
        &mut checkpoint,
        "conversation-checkpoint",
        "assistant-checkpoint",
        "run-checkpoint",
        &pending_call,
    );
    let run_checkpoint = checkpoint
        .resume_checkpoint
        .clone()
        .expect("test run checkpoint remains attached");

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

    let reloaded = AgentService::new_authorized_for_test(storage);
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

#[test]
fn pending_usage_restore_does_not_expose_missing_model_config_id() {
    let fixture = tempdir().unwrap();
    let storage = StorageService::open(&fixture.path().join("storage.sqlite")).unwrap();
    let local_model_config_id = "019d2e91-3ec8-7a36-a3b8-private-model-config";
    let provider_model_id = "provider-wire-model";
    let mut agent_input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "secret",
        "model": provider_model_id,
        "modelCapabilities": { "imageInput": false },
        "messages": []
    }))
    .unwrap();
    agent_input.model_config_id = Some(local_model_config_id.to_string());
    agent_input.provider_profile_config = Some(crate::test_provider_profile_config());
    agent_input.provider_protocol_key = Some(crate::test_provider_protocol_key(provider_model_id));

    let action_id = "call-missing-model-config";
    let run_id = "run-missing-model-config";
    let pending_action = PendingActionRecord {
        storage_id: pending_action_storage_id(run_id, action_id),
        snapshot: PendingAgentActionSnapshot {
            action_id: action_id.to_string(),
            action_type: "tool_call".to_string(),
            tool_name: "test_tool".to_string(),
            tool_call_id: Some(action_id.to_string()),
            run_id: run_id.to_string(),
            conversation_id: Some("conversation-missing-model-config".to_string()),
            assistant_message_id: Some("assistant-missing-model-config".to_string()),
            action: AgentProposedAction::ToolCall {
                call: AgentToolCall {
                    id: action_id.to_string(),
                    tool: "test_tool".to_string(),
                    args: json!({}),
                    approval_status: AgentApprovalStatus::Required,
                    reason: None,
                },
            },
            created_at: 1,
            status: PendingActionStatus::Pending,
        },
        agent_input,
    };

    let restored = super::super::usage::restore_pending_usage_contexts(
        &storage,
        &HashMap::from([(pending_action.storage_id.clone(), pending_action)]),
    )
    .unwrap();
    let usage = restored.get(run_id).unwrap();

    assert_eq!(usage.context.model_id, local_model_config_id);
    assert_eq!(usage.context.model_name, provider_model_id);
    assert!(!usage.context.model_name.contains(local_model_config_id));
}
