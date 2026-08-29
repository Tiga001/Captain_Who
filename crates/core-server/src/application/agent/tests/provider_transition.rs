use super::*;

fn conversation_with_completed_history(id: &str, model_id: Option<&str>) -> ChatConversationRecord {
    ChatConversationRecord {
        id: id.to_string(),
        project_id: None,
        model_id: model_id.map(str::to_string),
        title: "Provider transition".to_string(),
        messages: vec![
            ChatMessageRecord {
                id: format!("{id}-user"),
                role: "user".to_string(),
                content: "查找资料".to_string(),
                created_at: 1,
                status: Some("sent".to_string()),
                attachments: Vec::new(),
                agent_run_json: None,
                ui_state_json: None,
            },
            ChatMessageRecord {
                id: format!("{id}-assistant"),
                role: "assistant".to_string(),
                content: "已完成".to_string(),
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
    }
}

fn completed_history_model_context() -> Vec<ConversationModelContextItem> {
    let call_id = history_call_id();
    vec![
        ConversationModelContextItem {
            sequence: 0,
            ordinal: 0,
            role: "assistant".to_string(),
            content: "I am creating the requested file.".to_string(),
            tool_call_id: None,
            tool_calls: Vec::new(),
            is_error: false,
        },
        ConversationModelContextItem {
            sequence: 1,
            ordinal: 0,
            role: "assistant".to_string(),
            content: String::new(),
            tool_call_id: None,
            tool_calls: vec![AgentContextCheckpointToolCall {
                id: call_id.clone(),
                name: "apply_patch".to_string(),
                args: json!({
                    "request": {
                        "action": "apply",
                        "operation": "create",
                        "filePath": "src/history.rs",
                        "observationId": "fobs_provider_transition",
                        "content": "provider transition fixture"
                    }
                }),
                provider_identity: AgentProviderToolCallIdentity {
                    provider_tool_index: 0,
                    provider_call_id: "write-history".to_string(),
                    runtime_call_id: call_id.clone(),
                },
            }],
            is_error: false,
        },
        ConversationModelContextItem {
            sequence: 2,
            ordinal: 0,
            role: "tool".to_string(),
            content: r#"{"ok":true,"result":{"status":"applied"}}"#.to_string(),
            tool_call_id: Some(call_id),
            tool_calls: Vec::new(),
            is_error: false,
        },
    ]
}

fn persist_completed_history(
    storage: &StorageService,
    conversation_id: &str,
    assistant_message_id: &str,
) {
    let trace = completed_trace(conversation_id, assistant_message_id, 27);
    let model_context = completed_history_model_context();
    storage
        .finalize_chat_message_with_conversation_trace_model_context_and_usage(
            conversation_id,
            assistant_message_id,
            "已完成",
            Some("sent"),
            "completed",
            &trace,
            Some(&model_context),
            2,
            3,
            None,
        )
        .unwrap();
}

#[test]
fn durable_turn_occupancy_blocks_provider_transition_after_runtime_is_gone() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    storage
        .save_model_settings(two_model_settings(None))
        .unwrap();
    let conversation_id = "conversation-transition-durable-turn";
    let mut conversation = conversation_with_completed_history(conversation_id, Some("model-1"));
    let completed_assistant = conversation.messages[1].id.clone();
    conversation.messages.push(ChatMessageRecord {
        id: "assistant-durable-active".to_string(),
        role: "assistant".to_string(),
        content: String::new(),
        created_at: 3,
        status: Some("pending".to_string()),
        attachments: Vec::new(),
        agent_run_json: None,
        ui_state_json: None,
    });
    conversation.updated_at = 3;
    storage.save_conversation(conversation).unwrap();
    persist_completed_history(&storage, conversation_id, &completed_assistant);
    let active = ConversationTraceSnapshot::default().in_progress_trace(
        "run-durable-active",
        conversation_id,
        "assistant-durable-active",
    );
    storage
        .append_in_progress_conversation_turn_trace(&active, 3, 3)
        .unwrap();

    // A fresh Host has no resident Runtime/steering entry. The durable trace lease remains the
    // authority and must still block model mutation.
    let service = AgentService::try_new_deferred_startup_reconciliation(storage.clone()).unwrap();
    let before = storage.load_conversation(conversation_id).unwrap().unwrap();
    let preflight = service
        .preflight_provider_transition(AgentProviderTransitionPreflightInput {
            conversation_id: conversation_id.to_string(),
            target_model_id: "model-2".to_string(),
        })
        .unwrap();
    assert_eq!(preflight.decision, AgentProviderTransitionDecision::Blocked);
    assert_eq!(preflight.reason, AgentProviderTransitionReason::ActiveRun);
    assert_eq!(
        serde_json::to_value(storage.load_conversation(conversation_id).unwrap().unwrap()).unwrap(),
        serde_json::to_value(before).unwrap()
    );
}

#[test]
fn provider_transition_rejects_inactive_roots_and_child_observer_conversations_without_writes() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    storage
        .save_model_settings(two_model_settings(None))
        .unwrap();

    for (conversation_id, agent_id) in [
        (
            "conversation-transition-inactive-root",
            "agent-transition-inactive-root",
        ),
        ("conversation-transition-parent", "agent-transition-parent"),
    ] {
        storage
            .save_conversation(ChatConversationRecord {
                id: conversation_id.to_string(),
                project_id: None,
                model_id: Some("model-1".to_string()),
                title: conversation_id.to_string(),
                messages: Vec::new(),
                created_at: 1,
                updated_at: 1,
                pinned_at: None,
                archived_at: None,
                unread_at: None,
            })
            .unwrap();
        storage
            .ensure_root_agent(&mycopilot_core::EnsureRootAgentInput {
                agent_id: agent_id.to_string(),
                conversation_id: conversation_id.to_string(),
                creation_request_id: format!("ensure-{agent_id}"),
                task_name: "Root".to_string(),
            })
            .unwrap();
    }
    let inactive_root = storage
        .get_agent_node("agent-transition-inactive-root")
        .unwrap()
        .unwrap();
    storage
        .transition_agent_lifecycle(
            &inactive_root.agent_id,
            inactive_root.revision,
            mycopilot_core::AgentLifecycle::Active,
            mycopilot_core::AgentLifecycle::Disabled,
        )
        .unwrap();
    let child = storage
        .create_child_agent(&mycopilot_core::CreateChildAgentInput {
            parent_agent_id: "agent-transition-parent".to_string(),
            creation_request_id: "spawn-transition-observer-child".to_string(),
            task_name: "transition_observer".to_string(),
            task: "Inspect only.".to_string(),
            template_machine_key: None,
            explicit_model_id: None,
            reasoning_effort: None,
            fork_turns: mycopilot_core::AgentForkTurns::None,
        })
        .unwrap();

    let service = AgentService::new(Arc::clone(&storage));
    let inactive_conversation_id = "conversation-transition-inactive-root";
    let inactive_before = storage
        .load_conversation(inactive_conversation_id)
        .unwrap()
        .unwrap();
    let output = service
        .preflight_provider_transition(AgentProviderTransitionPreflightInput {
            conversation_id: inactive_conversation_id.to_string(),
            target_model_id: "model-2".to_string(),
        })
        .unwrap();
    assert_eq!(output.decision, AgentProviderTransitionDecision::Blocked);
    assert_eq!(
        output.reason,
        AgentProviderTransitionReason::UnsupportedTarget
    );
    assert_eq!(
        output.message.as_deref(),
        Some("根 Agent 当前不可用，不能切换模型。")
    );
    assert_eq!(
        serde_json::to_value(
            storage
                .load_conversation(inactive_conversation_id)
                .unwrap()
                .unwrap()
        )
        .unwrap(),
        serde_json::to_value(inactive_before).unwrap()
    );

    let child_conversation_id = child.agent.conversation_id.as_str();
    let child_before = storage
        .load_conversation(child_conversation_id)
        .unwrap()
        .unwrap();
    let error = service
        .preflight_provider_transition(AgentProviderTransitionPreflightInput {
            conversation_id: child_conversation_id.to_string(),
            target_model_id: "model-2".to_string(),
        })
        .unwrap_err();
    assert!(error
        .message()
        .contains("子 Agent Conversation 是只读观察视图"));
    assert_eq!(
        serde_json::to_value(
            storage
                .load_conversation(child_conversation_id)
                .unwrap()
                .unwrap()
        )
        .unwrap(),
        serde_json::to_value(child_before).unwrap()
    );
}

fn two_model_settings(
    target_profile: Option<mycopilot_core::ProviderProfileConfig>,
) -> ModelSettingsRecord {
    let mut settings = test_model_settings();
    let mut target = settings.models[0].clone();
    target.id = "model-2".to_string();
    target.display_name = "Model 2".to_string();
    target.provider_profile_config = target_profile.unwrap_or_else(|| {
        mycopilot_core::ProviderProfileConfig::generic_for_dialect(
            mycopilot_core::ProviderProtocolDialect::OpenAiChatCompletions,
        )
    });
    settings.models.push(target);
    settings
}

fn provider_transition_generator(model: &'static str) -> ContextCompactionSummaryGenerator {
    Arc::new(move |request, cancellation| {
        Box::pin(async move {
            cancellation.check()?;
            let observation = mycopilot_core::ModelRequestObservation {
                schema_version: mycopilot_core::MODEL_REQUEST_OBSERVATION_SCHEMA_VERSION,
                id: format!("model-request-{}", request.operation_id),
                run_id: request.run_id.clone(),
                conversation_id: Some(request.conversation_id.clone()),
                assistant_message_id: Some(request.assistant_message_id.clone()),
                operation_id: Some(request.operation_id.clone()),
                request_index: request.request_index,
                purpose: mycopilot_core::ModelRequestPurpose::ContextCompaction,
                model: model.to_string(),
                api_style: mycopilot_core::AgentApiStyle::OpenAiCompatible,
                status: mycopilot_core::ModelRequestObservationStatus::Completed,
                estimate: None,
                actual_usage: None,
                finish_reason: Some("stop".to_string()),
                error_code: None,
                error_message: None,
                started_at: 10,
                completed_at: 11,
                tool_set: None,
            };
            Ok(AgentContextCompactionGenerationOutput {
                draft: ContextCompactionSummaryDraft {
                    id: format!("transition-summary-{}", request.operation_id),
                    source_revision: request.prefix.source_revision.clone(),
                    content: "已将旧 API 厂商的工具历史压缩为安全摘要。".to_string(),
                    continuity: request.continuity,
                    generation: ContextCompactionGeneration::test(),
                    source_input_tokens: request.source_input_tokens,
                    summary_input_tokens: 8,
                    continuity_input_tokens: 8,
                    uncovered_tail_input_tokens: request.uncovered_tail_input_tokens,
                    replacement_input_tokens: 16,
                    created_at: now_ms(),
                },
                observation,
            })
        })
    })
}

fn seed_replayable_provider_turn(
    database_path: &std::path::Path,
    conversation_id: &str,
    assistant_message_id: &str,
    runtime_call_id: &str,
) -> String {
    let continuation_id = format!("provider-continuation-v1:{}", "a".repeat(36));
    let connection = rusqlite::Connection::open(database_path).unwrap();
    connection
        .execute(
            "INSERT INTO provider_continuations (
                continuation_id, schema_version, envelope_version,
                conversation_id, assistant_message_id, run_id, request_index,
                assistant_turn_id, assistant_turn_digest, provider_protocol_digest,
                state, superseded_by, compression, encryption, payload_digest,
                nonce, ciphertext, decoded_bytes, compressed_bytes,
                created_at, updated_at, released_at, activated_at
             ) VALUES (
                ?1, 1, 1, ?2, ?3, 'run-history', 0,
                ?4, ?5, ?6, 'active', NULL, 'zstd_binary_v1',
                'chacha20_poly1305_v1', ?7, ?8, ?9, 1, 1, 2, 2, NULL, 2
             )",
            rusqlite::params![
                &continuation_id,
                conversation_id,
                assistant_message_id,
                format!("at1_{}", "b".repeat(64)),
                format!("sha256:{}", "c".repeat(64)),
                format!("sha256:{}", "d".repeat(64)),
                format!("sha256:{}", "e".repeat(64)),
                vec![7_u8; 12],
                vec![9_u8; 17],
            ],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO provider_continuation_tool_calls (
                continuation_id, provider_tool_index, runtime_call_id
             ) VALUES (?1, 0, ?2)",
            rusqlite::params![&continuation_id, runtime_call_id],
        )
        .unwrap();
    continuation_id
}

async fn wait_for_provider_transition_completed(
    receiver: &mut tokio::sync::mpsc::UnboundedReceiver<Value>,
) -> Value {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let notification = receiver.recv().await.expect("transition notification");
            let params = notification.get("params").expect("notification params");
            if params.get("status").and_then(Value::as_str) == Some("completed") {
                break params.clone();
            }
        }
    })
    .await
    .expect("transition completes")
}

#[test]
fn conversation_history_without_a_frozen_source_model_fails_closed() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    storage
        .save_model_settings(two_model_settings(None))
        .unwrap();
    storage
        .save_conversation(conversation_with_completed_history(
            "conversation-provider-transition-compatible",
            None,
        ))
        .unwrap();
    let service = AgentService::new(storage.clone());

    let preflight = service
        .preflight_provider_transition(AgentProviderTransitionPreflightInput {
            conversation_id: "conversation-provider-transition-compatible".to_string(),
            target_model_id: "model-2".to_string(),
        })
        .unwrap();
    assert_eq!(preflight.decision, AgentProviderTransitionDecision::Blocked);
    assert_eq!(
        preflight.reason,
        AgentProviderTransitionReason::UnsupportedTarget
    );
    assert!(preflight.operation_id.is_none());
    assert!(preflight.transition_token.is_none());
    assert_eq!(
        storage
            .load_conversation("conversation-provider-transition-compatible")
            .unwrap()
            .unwrap()
            .model_id
            .as_deref(),
        None
    );
}

#[test]
fn incompatible_send_guard_rejects_before_persisting_the_new_turn() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    storage
        .save_model_settings(two_model_settings(Some(
            mycopilot_core::ProviderProfileConfig::deepseek_v4_default(),
        )))
        .unwrap();
    let conversation_id = "conversation-provider-transition-guard";
    let conversation = conversation_with_completed_history(conversation_id, Some("model-1"));
    let assistant_message_id = conversation.messages[1].id.clone();
    storage.save_conversation(conversation).unwrap();
    persist_completed_history(&storage, conversation_id, &assistant_message_id);
    let service = AgentService::new(storage.clone());
    let preflight = service
        .preflight_provider_transition(AgentProviderTransitionPreflightInput {
            conversation_id: conversation_id.to_string(),
            target_model_id: "model-2".to_string(),
        })
        .unwrap();
    assert_eq!(
        preflight.decision,
        AgentProviderTransitionDecision::RequiresCompaction,
        "unexpected preflight: {preflight:?}"
    );
    let before = storage
        .load_conversation(conversation_id)
        .unwrap()
        .unwrap()
        .messages;
    let (notifications, _receiver) = tokio::sync::mpsc::unbounded_channel();
    let error = service
        .start_conversation_turn(
            AgentConversationTurnInput {
                conversation_id: Some(conversation_id.to_string()),
                project_id: None,
                model_id: "model-2".to_string(),
                context_window_indicator_enabled: true,
                content: "继续".to_string(),
                attachments: Vec::new(),
                skills: Vec::new(),
                title: None,
                user_message_id: Some("guard-user-new".to_string()),
                assistant_message_id: Some("guard-assistant-new".to_string()),
                max_tokens: None,
                temperature: None,
                prompt_preferences: None,
                permissions: AgentPermissions::default(),
            },
            notifications,
        )
        .unwrap_err();
    assert_eq!(
        error
            .data()
            .and_then(|data| data.get("code"))
            .and_then(Value::as_str),
        Some("provider_transition_required")
    );
    let after = storage
        .load_conversation(conversation_id)
        .unwrap()
        .unwrap()
        .messages;
    assert_eq!(
        serde_json::to_value(&after).unwrap(),
        serde_json::to_value(&before).unwrap()
    );
    assert!(after
        .iter()
        .all(|message| message.id != "guard-user-new" && message.id != "guard-assistant-new"));
}

#[tokio::test]
async fn confirmed_incompatible_transition_compacts_and_opens_a_sendable_target_epoch() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    storage
        .save_model_settings(two_model_settings(Some(
            mycopilot_core::ProviderProfileConfig::deepseek_v4_default(),
        )))
        .unwrap();
    let conversation_id = "conversation-provider-transition-commit";
    let conversation = conversation_with_completed_history(conversation_id, Some("model-1"));
    let assistant_message_id = conversation.messages[1].id.clone();
    storage.save_conversation(conversation).unwrap();
    persist_completed_history(&storage, conversation_id, &assistant_message_id);
    let original_messages = serde_json::to_value(
        storage
            .load_conversation(conversation_id)
            .unwrap()
            .unwrap()
            .messages,
    )
    .unwrap();

    let service = AgentService::new(storage.clone())
        .with_context_compaction_summary_generator(provider_transition_generator("model-2"));
    let preflight = service
        .preflight_provider_transition(AgentProviderTransitionPreflightInput {
            conversation_id: conversation_id.to_string(),
            target_model_id: "model-2".to_string(),
        })
        .unwrap();
    assert_eq!(
        preflight.decision,
        AgentProviderTransitionDecision::RequiresCompaction
    );
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let running = service
        .start_provider_transition(
            AgentProviderTransitionStartInput {
                conversation_id: conversation_id.to_string(),
                target_model_id: "model-2".to_string(),
                transition_token: preflight.transition_token.unwrap(),
            },
            notifications,
        )
        .unwrap();
    assert_eq!(
        running.status,
        AgentProviderTransitionOperationStatus::Running
    );
    assert_eq!(
        running.source_model_display_name.as_deref(),
        Some("Model 1")
    );
    assert_eq!(
        running.target_model_display_name.as_deref(),
        Some("Model 2")
    );
    let operation_id = running.operation_id.clone();

    let completed = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let notification = receiver.recv().await.expect("transition notification");
            let params = notification.get("params").expect("notification params");
            if params.get("status").and_then(Value::as_str) == Some("completed") {
                break params.clone();
            }
        }
    })
    .await
    .expect("transition completes");
    assert_eq!(
        completed.get("modelId").and_then(Value::as_str),
        Some("model-2")
    );
    assert!(completed.get("summaryId").and_then(Value::as_str).is_some());
    assert_eq!(completed["sourceModelDisplayName"], "Model 1");
    assert_eq!(completed["targetModelDisplayName"], "Model 2");

    let stored = storage.load_conversation(conversation_id).unwrap().unwrap();
    assert_eq!(stored.model_id.as_deref(), Some("model-2"));
    assert_eq!(
        serde_json::to_value(&stored.messages).unwrap(),
        original_messages,
        "visible history is retained"
    );
    assert!(storage
        .get_active_context_compaction_summary(conversation_id)
        .unwrap()
        .is_some());
    let after = service
        .preflight_provider_transition(AgentProviderTransitionPreflightInput {
            conversation_id: conversation_id.to_string(),
            target_model_id: "model-2".to_string(),
        })
        .unwrap();
    assert_eq!(after.decision, AgentProviderTransitionDecision::Compatible);

    drop(service);
    let restarted = AgentService::new(storage.clone());
    let recovered = restarted
        .get_provider_transition_status(AgentProviderTransitionGetStatusInput {
            conversation_id: conversation_id.to_string(),
            operation_id: Some(operation_id),
        })
        .unwrap();
    assert_eq!(recovered.operations.len(), 1);
    assert_eq!(
        recovered.operations[0].source_model_display_name.as_deref(),
        Some("Model 1")
    );
    assert_eq!(
        recovered.operations[0].target_model_display_name.as_deref(),
        Some("Model 2")
    );
}

#[tokio::test]
async fn fork_adaptation_marker_forces_compaction_in_both_profile_directions_and_resolves_atomically(
) {
    for (suffix, source_is_deepseek, target_is_deepseek) in [
        ("deepseek-to-generic", true, false),
        ("generic-to-deepseek", false, true),
    ] {
        let fixture = tempdir().unwrap();
        let database_path = fixture.path().join("storage.sqlite");
        let storage = Arc::new(StorageService::open(&database_path).unwrap());
        let mut settings = two_model_settings(
            target_is_deepseek.then(mycopilot_core::ProviderProfileConfig::deepseek_v4_default),
        );
        settings.models[0].provider_profile_config = if source_is_deepseek {
            mycopilot_core::ProviderProfileConfig::deepseek_v4_default()
        } else {
            mycopilot_core::ProviderProfileConfig::generic_for_dialect(
                mycopilot_core::ProviderProtocolDialect::OpenAiChatCompletions,
            )
        };
        storage.save_model_settings(settings).unwrap();
        let conversation_id = format!("conversation-fork-adaptation-{suffix}");
        let conversation = conversation_with_completed_history(&conversation_id, Some("model-1"));
        let assistant_message_id = conversation.messages[1].id.clone();
        storage.save_conversation(conversation).unwrap();
        persist_completed_history(&storage, &conversation_id, &assistant_message_id);
        let connection = rusqlite::Connection::open(&database_path).unwrap();
        connection
            .execute(
                "INSERT INTO conversation_context_adaptation_requirements (
                    conversation_id, schema_version, reason, source_conversation_id,
                    source_message_id, created_at
                 ) VALUES (?1, 1, 'fork_released_provider_state_requires_compaction',
                           'source-conversation', 'source-assistant', 3)",
                [&conversation_id],
            )
            .unwrap();
        drop(connection);

        let service = AgentService::new(storage.clone())
            .with_context_compaction_summary_generator(provider_transition_generator("model-2"));
        let preflight = service
            .preflight_provider_transition(AgentProviderTransitionPreflightInput {
                conversation_id: conversation_id.clone(),
                target_model_id: "model-2".to_string(),
            })
            .unwrap();
        assert_eq!(
            preflight.decision,
            AgentProviderTransitionDecision::RequiresCompaction,
            "marker must win for {suffix}"
        );
        let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        service
            .start_provider_transition(
                AgentProviderTransitionStartInput {
                    conversation_id: conversation_id.clone(),
                    target_model_id: "model-2".to_string(),
                    transition_token: preflight.transition_token.unwrap(),
                },
                notifications,
            )
            .unwrap();
        let completed = wait_for_provider_transition_completed(&mut receiver).await;
        assert_eq!(completed["modelId"], "model-2");
        assert!(completed.get("summaryId").and_then(Value::as_str).is_some());

        let connection = rusqlite::Connection::open(&database_path).unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM conversation_context_adaptation_requirements
                     WHERE conversation_id = ?1 AND resolved_summary_id IS NOT NULL",
                    [&conversation_id],
                    |row| row.get::<_, u64>(0),
                )
                .unwrap(),
            1,
            "marker must resolve to the successful summary for {suffix}"
        );
        assert!(!storage
            .conversation_requires_context_adaptation(&conversation_id)
            .unwrap());
    }
}

#[tokio::test]
async fn deepseek_to_generic_transition_releases_private_state_and_opens_a_generic_epoch() {
    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("deepseek-to-generic-transition.sqlite");
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    let mut settings = two_model_settings(None);
    settings.models[0].provider_profile_config =
        mycopilot_core::ProviderProfileConfig::deepseek_v4_default();
    storage.save_model_settings(settings).unwrap();
    let conversation_id = "conversation-deepseek-to-generic-transition";
    let conversation = conversation_with_completed_history(conversation_id, Some("model-1"));
    let assistant_message_id = conversation.messages[1].id.clone();
    storage.save_conversation(conversation).unwrap();
    persist_completed_history(&storage, conversation_id, &assistant_message_id);
    let continuation_id = seed_replayable_provider_turn(
        &database_path,
        conversation_id,
        &assistant_message_id,
        &history_call_id(),
    );

    let service = AgentService::new(storage.clone())
        .with_context_compaction_summary_generator(provider_transition_generator("model-2"));
    let preflight = service
        .preflight_provider_transition(AgentProviderTransitionPreflightInput {
            conversation_id: conversation_id.to_string(),
            target_model_id: "model-2".to_string(),
        })
        .unwrap();
    assert_eq!(
        preflight.decision,
        AgentProviderTransitionDecision::RequiresCompaction
    );
    assert_eq!(
        preflight.reason,
        AgentProviderTransitionReason::ApiProviderChanged
    );
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    service
        .start_provider_transition(
            AgentProviderTransitionStartInput {
                conversation_id: conversation_id.to_string(),
                target_model_id: "model-2".to_string(),
                transition_token: preflight.transition_token.unwrap(),
            },
            notifications,
        )
        .unwrap();
    let completed = wait_for_provider_transition_completed(&mut receiver).await;
    assert_eq!(completed["modelId"], "model-2");

    let connection = rusqlite::Connection::open(&database_path).unwrap();
    let released = connection
        .query_row(
            "SELECT state, ciphertext IS NULL, released_at IS NOT NULL
             FROM provider_continuations WHERE continuation_id = ?1",
            [&continuation_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, bool>(1)?,
                    row.get::<_, bool>(2)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(released, ("released".to_string(), true, true));
    assert_eq!(
        storage
            .load_conversation(conversation_id)
            .unwrap()
            .unwrap()
            .model_id
            .as_deref(),
        Some("model-2")
    );
    let after = service
        .preflight_provider_transition(AgentProviderTransitionPreflightInput {
            conversation_id: conversation_id.to_string(),
            target_model_id: "model-2".to_string(),
        })
        .unwrap();
    assert_eq!(after.decision, AgentProviderTransitionDecision::Compatible);
}

#[tokio::test]
async fn same_model_protocol_revision_change_compacts_before_reusing_the_model_id() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let initial_settings = test_model_settings();
    storage
        .save_model_settings(initial_settings.clone())
        .unwrap();
    let conversation_id = "conversation-same-model-protocol-transition";
    let conversation = conversation_with_completed_history(conversation_id, Some("model-1"));
    let assistant_message_id = conversation.messages[1].id.clone();
    storage.save_conversation(conversation).unwrap();
    persist_completed_history(&storage, conversation_id, &assistant_message_id);

    let before_revision = storage
        .load_model_settings_snapshot()
        .unwrap()
        .unwrap()
        .provider_protocol_revisions["model-1"]
        .clone();
    let mut changed_settings = initial_settings;
    changed_settings.models[0].provider_profile_config =
        mycopilot_core::ProviderProfileConfig::deepseek_v4_default();
    storage.save_model_settings(changed_settings).unwrap();
    let after_revision = storage
        .load_model_settings_snapshot()
        .unwrap()
        .unwrap()
        .provider_protocol_revisions["model-1"]
        .clone();
    assert_ne!(after_revision, before_revision);

    let service = AgentService::new(storage.clone())
        .with_context_compaction_summary_generator(provider_transition_generator("model-1"));
    let preflight = service
        .preflight_provider_transition(AgentProviderTransitionPreflightInput {
            conversation_id: conversation_id.to_string(),
            target_model_id: "model-1".to_string(),
        })
        .unwrap();
    assert_eq!(
        preflight.decision,
        AgentProviderTransitionDecision::RequiresCompaction
    );
    assert_eq!(
        preflight.reason,
        AgentProviderTransitionReason::ProviderProtocolChanged
    );
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    service
        .start_provider_transition(
            AgentProviderTransitionStartInput {
                conversation_id: conversation_id.to_string(),
                target_model_id: "model-1".to_string(),
                transition_token: preflight.transition_token.unwrap(),
            },
            notifications,
        )
        .unwrap();
    let completed = wait_for_provider_transition_completed(&mut receiver).await;
    assert_eq!(completed["modelId"], "model-1");
    assert!(storage
        .get_active_context_compaction_summary(conversation_id)
        .unwrap()
        .is_some());
    let after = service
        .preflight_provider_transition(AgentProviderTransitionPreflightInput {
            conversation_id: conversation_id.to_string(),
            target_model_id: "model-1".to_string(),
        })
        .unwrap();
    assert_eq!(after.decision, AgentProviderTransitionDecision::Compatible);
}

#[tokio::test]
async fn failed_transition_gets_a_new_retry_token_and_can_succeed() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    storage
        .save_model_settings(two_model_settings(Some(
            mycopilot_core::ProviderProfileConfig::deepseek_v4_default(),
        )))
        .unwrap();
    let conversation_id = "conversation-provider-transition-retry";
    let conversation = conversation_with_completed_history(conversation_id, Some("model-1"));
    let assistant_message_id = conversation.messages[1].id.clone();
    storage.save_conversation(conversation).unwrap();
    persist_completed_history(&storage, conversation_id, &assistant_message_id);

    let failing_generator: ContextCompactionSummaryGenerator = Arc::new(|_, _| {
        Box::pin(async { Err(AgentError::new("expected transition generation failure")) })
    });
    let failing = AgentService::new(storage.clone())
        .with_context_compaction_summary_generator(failing_generator);
    let first_preflight = failing
        .preflight_provider_transition(AgentProviderTransitionPreflightInput {
            conversation_id: conversation_id.to_string(),
            target_model_id: "model-2".to_string(),
        })
        .unwrap();
    let first_token = first_preflight.transition_token.unwrap();
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    failing
        .start_provider_transition(
            AgentProviderTransitionStartInput {
                conversation_id: conversation_id.to_string(),
                target_model_id: "model-2".to_string(),
                transition_token: first_token.clone(),
            },
            notifications,
        )
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let notification = receiver.recv().await.expect("transition notification");
            if notification
                .get("params")
                .and_then(|params| params.get("status"))
                .and_then(Value::as_str)
                == Some("failed")
            {
                break;
            }
        }
    })
    .await
    .expect("failed transition becomes terminal");

    let second_preflight = failing
        .preflight_provider_transition(AgentProviderTransitionPreflightInput {
            conversation_id: conversation_id.to_string(),
            target_model_id: "model-2".to_string(),
        })
        .unwrap();
    let second_token = second_preflight.transition_token.unwrap();
    assert_ne!(second_token, first_token);
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    failing
        .start_provider_transition(
            AgentProviderTransitionStartInput {
                conversation_id: conversation_id.to_string(),
                target_model_id: "model-2".to_string(),
                transition_token: second_token.clone(),
            },
            notifications,
        )
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let notification = receiver.recv().await.expect("transition notification");
            if notification
                .get("params")
                .and_then(|params| params.get("status"))
                .and_then(Value::as_str)
                == Some("failed")
            {
                break;
            }
        }
    })
    .await
    .expect("second failed transition becomes terminal");
    drop(failing);

    let retrying = AgentService::new(storage.clone())
        .with_context_compaction_summary_generator(provider_transition_generator("model-2"));
    let retry_preflight = retrying
        .preflight_provider_transition(AgentProviderTransitionPreflightInput {
            conversation_id: conversation_id.to_string(),
            target_model_id: "model-2".to_string(),
        })
        .unwrap();
    let retry_token = retry_preflight.transition_token.unwrap();
    assert_ne!(retry_token, first_token);
    assert_ne!(retry_token, second_token);
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    retrying
        .start_provider_transition(
            AgentProviderTransitionStartInput {
                conversation_id: conversation_id.to_string(),
                target_model_id: "model-2".to_string(),
                transition_token: retry_token,
            },
            notifications,
        )
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let notification = receiver.recv().await.expect("transition notification");
            if notification
                .get("params")
                .and_then(|params| params.get("status"))
                .and_then(Value::as_str)
                == Some("completed")
            {
                break;
            }
        }
    })
    .await
    .expect("retry completes");
    assert_eq!(
        storage
            .load_conversation(conversation_id)
            .unwrap()
            .unwrap()
            .model_id
            .as_deref(),
        Some("model-2")
    );
}
