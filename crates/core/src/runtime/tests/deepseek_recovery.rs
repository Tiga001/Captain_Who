use super::*;

#[tokio::test]
async fn deepseek_cancellation_during_result_publication_closes_grouped_suffix() {
    use crate::image_generation::{CredentialStore, InMemoryCredentialStore};
    use crate::protocol::{
        AgentCommandPermission, AgentCommandSafetyPolicy, AgentPermissions, AgentReadPermission,
        AgentWritePermission,
    };
    use crate::storage::models::{ChatConversationRecord, ChatMessageRecord};
    use crate::storage::service::StorageService;
    use crate::{
        ProviderContinuationVaultFactory, ProviderProfileConfig, ProviderProtocolDialect,
        ProviderProtocolKey, ReasoningEffort, ReasoningMode, ReasoningPolicy,
    };
    use tempfile::tempdir;
    use tokio::net::TcpListener;
    use tokio::time::{timeout, Duration};

    const CONVERSATION_ID: &str = "conversation-deepseek-result-publish-cancel";
    const ASSISTANT_ID: &str = "assistant-deepseek-result-publish-cancel";
    const RUN_ID: &str = "run-deepseek-result-publish-cancel";
    const MODEL_ID: &str = "deepseek-result-publish-cancel-model";

    fn provider_read_call(id: &str, path: &str) -> Value {
        json!({
            "id": id,
            "type": "function",
            "function": {
                "name": "read_file",
                "arguments": serde_json::to_string(&json!({ "path": path })).unwrap(),
            }
        })
    }

    let fixture = tempdir().unwrap();
    let workspace = fixture.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    std::fs::write(workspace.join("first.txt"), "first result").unwrap();
    std::fs::write(workspace.join("second.txt"), "second must not execute").unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("runtime.sqlite")).unwrap());
    storage
        .save_conversation(ChatConversationRecord {
            id: CONVERSATION_ID.to_string(),
            project_id: None,
            model_id: Some(MODEL_ID.to_string()),
            title: "DeepSeek result publication cancel".to_string(),
            messages: vec![ChatMessageRecord {
                id: ASSISTANT_ID.to_string(),
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
    let credentials = Arc::new(InMemoryCredentialStore::default()) as Arc<dyn CredentialStore>;
    let vault = Arc::new(
        ProviderContinuationVaultFactory::open_or_provision(Arc::clone(&storage), credentials)
            .unwrap(),
    );
    let mut profile = ProviderProfileConfig::deepseek_v4_default();
    let ProviderProfileConfig::V1(config) = &mut profile else {
        unreachable!("legacy DeepSeek constructor must produce schema v1")
    };
    config.reasoning = ReasoningPolicy {
        mode: ReasoningMode::Enabled,
        effort: ReasoningEffort::High,
    };
    let protocol = ProviderProtocolKey::new(
        ProviderProtocolDialect::OpenAiChatCompletions,
        &profile,
        MODEL_ID,
        None,
    )
    .unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let _request = read_runtime_test_json_request(&mut stream).await;
        write_runtime_test_json_response(
            &mut stream,
            json!({
                "choices": [{
                    "message": {
                        "role": "assistant",
                        "content": "",
                        "reasoning_content": "Cancel only after the first result is recorded.",
                        "tool_calls": [
                            provider_read_call("provider-read-first", "first.txt"),
                            provider_read_call("provider-read-second", "second.txt"),
                        ],
                    },
                    "finish_reason": "tool_calls",
                }]
            }),
        )
        .await;
        timeout(Duration::from_millis(300), listener.accept())
            .await
            .is_err()
    });

    let cancellation = AgentCancellationToken::new();
    let cancellation_for_observer = cancellation.clone();
    let trace_snapshots = Arc::new(Mutex::new(Vec::new()));
    let trace_snapshots_for_observer = Arc::clone(&trace_snapshots);
    let observer: AgentConversationTraceObserver = Arc::new(move |snapshot| {
        let result_count = snapshot
            .items
            .iter()
            .filter(|item| matches!(item, ConversationTurnTraceItem::ToolResult { .. }))
            .count();
        if result_count == 1 {
            cancellation_for_observer.cancel();
        }
        trace_snapshots_for_observer.lock().unwrap().push(snapshot);
        Ok(None)
    });
    let mut input = conversation_context_input(vec![message(
        "user",
        "Read two files but stop during first-result publication.",
    )]);
    input.api_url = format!("http://{address}/v1/chat/completions");
    input.api_token = "test-token".to_string();
    input.provider_profile_config = Some(profile);
    input.provider_protocol_key = Some(protocol.clone());
    input.model = MODEL_ID.to_string();
    input.stream = Some(false);
    input.assistant_message_id = Some(ASSISTANT_ID.to_string());
    input.context = Some(AgentRunContext {
        collaboration_identity: None,
        conversation_id: Some(CONVERSATION_ID.to_string()),
        project_id: None,
        workspace: Some(AgentWorkspaceContext {
            project_id: None,
            display_name: Some("DeepSeek cancellation workspace".to_string()),
            root_path: Some(workspace.to_string_lossy().into_owned()),
        }),
        attachment_library: None,
        permissions: AgentPermissions {
            read: AgentReadPermission::All,
            write: AgentWritePermission::All,
            command: AgentCommandPermission::RequireApproval,
            command_safety: AgentCommandSafetyPolicy::FullAccess,
            patch: Default::default(),
            builtin_execution: Default::default(),
        },
    });
    let output = AgentRuntime::default()
        .send_chat_with_events_and_cancellation(
            input,
            Some(RUN_ID.to_string()),
            None,
            cancellation,
            Some(
                AgentRuntimeHostServices::new()
                    .with_storage(storage)
                    .with_provider_continuation_vault(Arc::clone(&vault))
                    .with_trace_observer(observer),
            ),
        )
        .await
        .unwrap();
    let no_second_request = server.await.unwrap();
    assert!(no_second_request);
    assert_eq!(output.status, AgentRunStatus::Cancelled);
    let trace = output
        .conversation_turn_trace
        .as_ref()
        .expect("cancelled grouped turn must retain a complete trace");
    trace.validate().unwrap();
    let results = trace
        .items
        .iter()
        .filter_map(|item| match item {
            ConversationTurnTraceItem::ToolResult {
                call_id,
                observation,
                ..
            } => Some((call_id, observation)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(results.len(), 2);
    assert!(results[0].1.to_string().contains("first result"));
    assert!(results[1].1.to_string().contains("runCancelled"));
    let terminal_snapshot = trace_snapshots
        .lock()
        .unwrap()
        .iter()
        .rev()
        .find(|snapshot| {
            snapshot
                .items
                .iter()
                .filter(|item| matches!(item, ConversationTurnTraceItem::ToolResult { .. }))
                .count()
                == 2
        })
        .cloned()
        .expect("the grouped cancellation must publish one complete model-context snapshot");
    let result_context = terminal_snapshot
        .model_context_items
        .iter()
        .filter_map(|item| {
            item.tool_call_id
                .as_deref()
                .map(|call_id| (call_id, item.is_error))
        })
        .collect::<Vec<_>>();
    assert_eq!(
        result_context,
        vec![(results[0].0.as_str(), false), (results[1].0.as_str(), true)],
        "the authoritative successful result must not be rewritten as an error while the synthetic cancelled suffix must remain an error"
    );
    let persisted = vault
        .list_replayable_for_conversation(CONVERSATION_ID, &protocol)
        .unwrap();
    assert_eq!(persisted.len(), 1);
    assert_eq!(persisted[0].assistant_turn.provider_tool_calls().len(), 2);
}

#[tokio::test]
async fn deepseek_commit_unknown_trace_publish_recovers_staged_turn_without_tool_dispatch() {
    use crate::image_generation::{CredentialStore, InMemoryCredentialStore};
    use crate::protocol::{
        AgentCommandPermission, AgentCommandSafetyPolicy, AgentPermissions, AgentReadPermission,
        AgentWritePermission,
    };
    use crate::storage::models::{ChatConversationRecord, ChatMessageRecord};
    use crate::storage::service::StorageService;
    use crate::{
        ProviderContinuationVaultFactory, ProviderProfileConfig, ProviderProtocolDialect,
        ProviderProtocolKey, ReasoningEffort, ReasoningMode, ReasoningPolicy,
        CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
    };
    use tempfile::tempdir;
    use tokio::net::TcpListener;
    use tokio::time::{timeout, Duration};

    const CONVERSATION_ID: &str = "conversation-deepseek-commit-unknown";
    const ASSISTANT_ID: &str = "assistant-deepseek-commit-unknown";
    const RUN_ID: &str = "run-deepseek-commit-unknown";
    const MODEL_ID: &str = "deepseek-commit-unknown-model";
    const OBSERVER_ERROR: &str = "trace observer committed before returning an unknown outcome";

    let fixture = tempdir().unwrap();
    let workspace = fixture.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    std::fs::write(workspace.join("never-read.txt"), "must not be dispatched").unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("runtime.sqlite")).unwrap());
    storage
        .save_conversation(ChatConversationRecord {
            id: CONVERSATION_ID.to_string(),
            project_id: None,
            model_id: Some(MODEL_ID.to_string()),
            title: "DeepSeek commit-unknown handoff".to_string(),
            messages: vec![ChatMessageRecord {
                id: ASSISTANT_ID.to_string(),
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
    let credentials = Arc::new(InMemoryCredentialStore::default()) as Arc<dyn CredentialStore>;
    let vault = Arc::new(
        ProviderContinuationVaultFactory::open_or_provision(Arc::clone(&storage), credentials)
            .unwrap(),
    );
    let mut profile = ProviderProfileConfig::deepseek_v4_default();
    let ProviderProfileConfig::V1(config) = &mut profile else {
        unreachable!("legacy DeepSeek constructor must produce schema v1")
    };
    config.reasoning = ReasoningPolicy {
        mode: ReasoningMode::Enabled,
        effort: ReasoningEffort::High,
    };
    let protocol = ProviderProtocolKey::new(
        ProviderProtocolDialect::OpenAiChatCompletions,
        &profile,
        MODEL_ID,
        None,
    )
    .unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let _request = read_runtime_test_json_request(&mut stream).await;
        write_runtime_test_json_response(
            &mut stream,
            json!({
                "choices": [{
                    "message": {
                        "role": "assistant",
                        "content": "",
                        "reasoning_content": "Seal before publishing the handoff.",
                        "tool_calls": [{
                            "id": "provider-commit-unknown-read",
                            "type": "function",
                            "function": {
                                "name": "read_file",
                                "arguments": serde_json::to_string(
                                    &json!({ "path": "never-read.txt" })
                                )
                                .unwrap(),
                            }
                        }],
                    },
                    "finish_reason": "tool_calls",
                }]
            }),
        )
        .await;
        timeout(Duration::from_millis(300), listener.accept())
            .await
            .is_err()
    });

    let storage_for_observer = Arc::clone(&storage);
    let observer: AgentConversationTraceObserver = Arc::new(move |snapshot| {
        if !snapshot
            .items
            .iter()
            .any(|item| matches!(item, ConversationTurnTraceItem::ToolCall { .. }))
        {
            return Ok(None);
        }
        let committed = ConversationTurnTrace {
            schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
            run_id: RUN_ID.to_string(),
            conversation_id: CONVERSATION_ID.to_string(),
            assistant_message_id: ASSISTANT_ID.to_string(),
            terminal_status: ConversationTurnTraceTerminalStatus::InProgress,
            terminal_error: None,
            truncated: snapshot.truncated,
            items: snapshot.items,
        };
        storage_for_observer
            .append_in_progress_conversation_turn_trace(&committed, 1, 2)
            .map_err(AgentError::new)?;
        Err(AgentError::new(OBSERVER_ERROR))
    });

    let mut input = conversation_context_input(vec![message(
        "user",
        "Read one file after the durable provider handoff.",
    )]);
    input.api_url = format!("http://{address}/v1/chat/completions");
    input.api_token = "test-token".to_string();
    input.provider_profile_config = Some(profile);
    input.provider_protocol_key = Some(protocol.clone());
    input.model = MODEL_ID.to_string();
    input.stream = Some(false);
    input.assistant_message_id = Some(ASSISTANT_ID.to_string());
    input.context = Some(AgentRunContext {
        collaboration_identity: None,
        conversation_id: Some(CONVERSATION_ID.to_string()),
        project_id: None,
        workspace: Some(AgentWorkspaceContext {
            project_id: None,
            display_name: Some("DeepSeek commit-unknown workspace".to_string()),
            root_path: Some(workspace.to_string_lossy().into_owned()),
        }),
        attachment_library: None,
        permissions: AgentPermissions {
            read: AgentReadPermission::All,
            write: AgentWritePermission::All,
            command: AgentCommandPermission::RequireApproval,
            command_safety: AgentCommandSafetyPolicy::FullAccess,
            patch: Default::default(),
            builtin_execution: Default::default(),
        },
    });
    let error = AgentRuntime::default()
        .send_chat_with_events_and_cancellation(
            input,
            Some(RUN_ID.to_string()),
            None,
            AgentCancellationToken::new(),
            Some(
                AgentRuntimeHostServices::new()
                    .with_storage(Arc::clone(&storage))
                    .with_provider_continuation_vault(Arc::clone(&vault))
                    .with_trace_observer(observer),
            ),
        )
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), OBSERVER_ERROR);
    let terminal_trace = error
        .conversation_turn_trace()
        .expect("commit-unknown failure must still close the in-memory grouped turn");
    terminal_trace.validate().unwrap();
    assert_eq!(
        terminal_trace
            .items
            .iter()
            .filter(|item| matches!(item, ConversationTurnTraceItem::ToolResult { .. }))
            .count(),
        1
    );
    assert!(
        server.await.unwrap(),
        "the failed handoff must not dispatch a Tool or send another Provider request"
    );

    let recovered = vault
        .list_replayable_for_conversation(CONVERSATION_ID, &protocol)
        .unwrap();
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].assistant_turn.provider_tool_calls().len(), 1);
    assert_eq!(
        recovered[0].assistant_turn.provider_tool_calls()[0].id,
        "provider-commit-unknown-read"
    );
}

#[tokio::test]
async fn deepseek_checkpoint_abort_closes_unknown_suffix_and_replays_next_run() {
    use crate::image_generation::{CredentialStore, InMemoryCredentialStore};
    use crate::protocol::{
        AgentCommandPermission, AgentCommandSafetyPolicy, AgentPermissions, AgentReadPermission,
        AgentWritePermission,
    };
    use crate::storage::models::{ChatConversationRecord, ChatMessageRecord};
    use crate::storage::service::StorageService;
    use crate::{
        ProviderContinuationVaultFactory, ProviderProfileConfig, ProviderProtocolDialect,
        ProviderProtocolKey, ReasoningEffort, ReasoningMode, ReasoningPolicy,
    };
    use tempfile::tempdir;
    use tokio::net::TcpListener;

    const CONVERSATION_ID: &str = "conversation-deepseek-checkpoint-abort";
    const FIRST_ASSISTANT_ID: &str = "assistant-deepseek-checkpoint-abort";
    const SECOND_ASSISTANT_ID: &str = "assistant-deepseek-checkpoint-recovery";
    const FIRST_RUN_ID: &str = "run-deepseek-checkpoint-abort";
    const SECOND_RUN_ID: &str = "run-deepseek-checkpoint-recovery";
    const MODEL_ID: &str = "deepseek-checkpoint-abort-model";
    const PROVIDER_REVISION: &str = "provider-protocol-v1:deepseek-checkpoint-abort";
    const REASONING: &str = "Preserve this reasoning across the aborted grouped turn.";

    fn provider_tool_call(id: &str, name: &str, args: Value) -> Value {
        json!({
            "id": id,
            "type": "function",
            "function": {
                "name": name,
                "arguments": serde_json::to_string(&args).unwrap(),
            }
        })
    }

    let fixture = tempdir().unwrap();
    let workspace = fixture.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("runtime.sqlite")).unwrap());
    storage
        .save_conversation(ChatConversationRecord {
            id: CONVERSATION_ID.to_string(),
            project_id: None,
            model_id: Some(MODEL_ID.to_string()),
            title: "DeepSeek checkpoint abort".to_string(),
            messages: vec![
                ChatMessageRecord {
                    id: FIRST_ASSISTANT_ID.to_string(),
                    role: "assistant".to_string(),
                    content: String::new(),
                    created_at: 1,
                    status: Some("failed".to_string()),
                    attachments: Vec::new(),
                    agent_run_json: None,
                    ui_state_json: None,
                },
                ChatMessageRecord {
                    id: SECOND_ASSISTANT_ID.to_string(),
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
    let credentials = Arc::new(InMemoryCredentialStore::default()) as Arc<dyn CredentialStore>;
    let vault = Arc::new(
        ProviderContinuationVaultFactory::open_or_provision(Arc::clone(&storage), credentials)
            .unwrap(),
    );
    let mut profile = ProviderProfileConfig::deepseek_v4_default();
    let ProviderProfileConfig::V1(config) = &mut profile else {
        unreachable!("legacy DeepSeek constructor must produce schema v1")
    };
    config.reasoning = ReasoningPolicy {
        mode: ReasoningMode::Enabled,
        effort: ReasoningEffort::High,
    };
    let protocol = ProviderProtocolKey::new(
        ProviderProtocolDialect::OpenAiChatCompletions,
        &profile,
        MODEL_ID,
        Some(PROVIDER_REVISION.to_string()),
    )
    .unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let replayed_request = Arc::new(Mutex::new(None::<Value>));
    let replayed_request_for_server = Arc::clone(&replayed_request);
    let server = tokio::spawn(async move {
        let (mut first_stream, _) = listener.accept().await.unwrap();
        let first_request = read_runtime_test_json_request(&mut first_stream).await;
        assert!(first_request["messages"]
            .as_array()
            .unwrap()
            .iter()
            .all(|message| message.get("reasoning_content").is_none()));
        write_runtime_test_json_response(
            &mut first_stream,
            json!({
                "choices": [{
                    "message": {
                        "role": "assistant",
                        "content": "",
                        "reasoning_content": REASONING,
                        "tool_calls": [
                            provider_tool_call(
                                "provider-approval-call",
                                "run_command",
                                json!({ "command": "echo should-require-approval" }),
                            ),
                            provider_tool_call(
                                "provider-unknown-call",
                                "unknown_private_tool",
                                json!({ "secret": "must-stay-private" }),
                            ),
                        ],
                    },
                    "finish_reason": "tool_calls",
                }]
            }),
        )
        .await;

        let (mut second_stream, _) = listener.accept().await.unwrap();
        let second_request = read_runtime_test_json_request(&mut second_stream).await;
        *replayed_request_for_server.lock().unwrap() = Some(second_request);
        write_runtime_test_json_response(
            &mut second_stream,
            json!({
                "choices": [{
                    "message": {
                        "role": "assistant",
                        "content": "recovered from a protocol-complete aborted turn",
                        "reasoning_content": "The previous grouped turn is closed.",
                    },
                    "finish_reason": "stop",
                }]
            }),
        )
        .await;
    });

    let mut first_input = conversation_context_input(vec![message(
        "user",
        "Request an approval call followed by an unknown call.",
    )]);
    first_input.api_url = format!("http://{address}/v1/chat/completions");
    first_input.api_token = "test-token".to_string();
    first_input.provider_profile_config = Some(profile.clone());
    first_input.provider_protocol_key = Some(protocol.clone());
    first_input.provider_configuration_revision = Some(PROVIDER_REVISION.to_string());
    first_input.model = MODEL_ID.to_string();
    first_input.stream = Some(false);
    first_input.assistant_message_id = Some(FIRST_ASSISTANT_ID.to_string());
    first_input.context = Some(AgentRunContext {
        collaboration_identity: None,
        conversation_id: Some(CONVERSATION_ID.to_string()),
        project_id: None,
        workspace: Some(AgentWorkspaceContext {
            project_id: None,
            display_name: Some("DeepSeek abort workspace".to_string()),
            root_path: Some(workspace.to_string_lossy().into_owned()),
        }),
        attachment_library: None,
        permissions: AgentPermissions {
            read: AgentReadPermission::All,
            write: AgentWritePermission::All,
            command: AgentCommandPermission::RequireApproval,
            command_safety: AgentCommandSafetyPolicy::FullAccess,
            patch: Default::default(),
            builtin_execution: Default::default(),
        },
    });
    let snapshots = Arc::new(Mutex::new(Vec::<ConversationTraceSnapshot>::new()));
    let snapshots_for_observer = Arc::clone(&snapshots);
    let observer: AgentConversationTraceObserver = Arc::new(move |snapshot| {
        snapshots_for_observer.lock().unwrap().push(snapshot);
        Ok(None)
    });
    let first_error = AgentRuntime::default()
        .send_chat_with_events_and_cancellation(
            first_input,
            Some(FIRST_RUN_ID.to_string()),
            None,
            AgentCancellationToken::new(),
            Some(
                AgentRuntimeHostServices::new()
                    .with_storage(Arc::clone(&storage))
                    .with_provider_continuation_vault(Arc::clone(&vault))
                    .with_trace_observer(observer),
            ),
        )
        .await
        .unwrap_err();
    assert_eq!(
        first_error.code(),
        Some("agent.checkpoint_private_tool_arguments")
    );
    let failed_trace = first_error
        .conversation_turn_trace()
        .expect("terminal abort must retain a complete trace")
        .clone();
    failed_trace.validate().unwrap();
    let trace_pairs = failed_trace
        .items
        .iter()
        .filter_map(|item| match item {
            ConversationTurnTraceItem::ToolCall { call_id, .. } => Some(("call", call_id.clone())),
            ConversationTurnTraceItem::ToolResult { call_id, .. } => {
                Some(("result", call_id.clone()))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(trace_pairs.len(), 4);
    assert_eq!(trace_pairs[0].0, "call");
    assert_eq!(trace_pairs[1], ("result", trace_pairs[0].1.clone()));
    assert_eq!(trace_pairs[2].0, "call");
    assert_eq!(trace_pairs[3], ("result", trace_pairs[2].1.clone()));
    let persisted = vault
        .list_replayable_for_conversation(CONVERSATION_ID, &protocol)
        .unwrap();
    assert_eq!(persisted.len(), 1);
    assert_eq!(persisted[0].assistant_turn.provider_tool_calls().len(), 2);

    let final_snapshot = snapshots
        .lock()
        .unwrap()
        .last()
        .cloned()
        .expect("terminal abort must publish the protocol-complete model context");
    let aborted_history = AgentChatMessage {
        message_id: Some(FIRST_ASSISTANT_ID.to_string()),
        role: "assistant".to_string(),
        content: String::new(),
        created_at: Some(2),
        conversation_turn_trace: Some(failed_trace),
        conversation_model_context_items: final_snapshot.model_context_items,
    };
    let mut recovery_input = conversation_context_input(vec![
        message(
            "user",
            "Request an approval call followed by an unknown call.",
        ),
        aborted_history,
        message("user", "Continue without repeating the skipped tools."),
    ]);
    recovery_input.api_url = format!("http://{address}/v1/chat/completions");
    recovery_input.api_token = "test-token".to_string();
    recovery_input.provider_profile_config = Some(profile);
    recovery_input.provider_protocol_key = Some(protocol);
    recovery_input.provider_configuration_revision = Some(PROVIDER_REVISION.to_string());
    recovery_input.model = MODEL_ID.to_string();
    recovery_input.stream = Some(false);
    recovery_input.assistant_message_id = Some(SECOND_ASSISTANT_ID.to_string());
    recovery_input.context = Some(AgentRunContext {
        collaboration_identity: None,
        conversation_id: Some(CONVERSATION_ID.to_string()),
        project_id: None,
        workspace: Some(AgentWorkspaceContext {
            project_id: None,
            display_name: Some("DeepSeek recovery workspace".to_string()),
            root_path: Some(workspace.to_string_lossy().into_owned()),
        }),
        attachment_library: None,
        permissions: AgentPermissions {
            read: AgentReadPermission::All,
            write: AgentWritePermission::All,
            command: AgentCommandPermission::RequireApproval,
            command_safety: AgentCommandSafetyPolicy::FullAccess,
            patch: Default::default(),
            builtin_execution: Default::default(),
        },
    });
    let recovered = AgentRuntime::default()
        .send_chat_with_events_and_cancellation(
            recovery_input,
            Some(SECOND_RUN_ID.to_string()),
            None,
            AgentCancellationToken::new(),
            Some(
                AgentRuntimeHostServices::new()
                    .with_storage(storage)
                    .with_provider_continuation_vault(vault),
            ),
        )
        .await
        .unwrap();
    server.await.unwrap();
    assert_eq!(recovered.status, AgentRunStatus::Completed);
    assert_eq!(
        recovered.content,
        "recovered from a protocol-complete aborted turn"
    );
    let replayed = replayed_request.lock().unwrap().clone().unwrap();
    let messages = replayed["messages"].as_array().unwrap();
    let replayed_turn = messages
        .iter()
        .find(|message| message["reasoning_content"].as_str() == Some(REASONING))
        .expect("the exact aborted Provider turn must replay");
    let provider_ids = replayed_turn["tool_calls"]
        .as_array()
        .unwrap()
        .iter()
        .map(|call| call["id"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        provider_ids,
        ["provider-approval-call", "provider-unknown-call"]
    );
    let result_ids = messages
        .iter()
        .filter(|message| message["role"] == "tool")
        .map(|message| message["tool_call_id"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        result_ids,
        ["provider-approval-call", "provider-unknown-call"]
    );
    assert!(messages
        .iter()
        .filter(|message| message["role"] == "tool")
        .all(|message| message["content"]
            .as_str()
            .is_some_and(|content| content.contains("groupedTurnAborted"))));
}

#[tokio::test]
async fn deepseek_runtime_persists_grouped_turns_before_tool_side_effects() {
    use crate::image_generation::{CredentialStore, InMemoryCredentialStore};
    use crate::protocol::{
        AgentCommandPermission, AgentCommandSafetyPolicy, AgentPermissions, AgentReadPermission,
        AgentWritePermission,
    };
    use crate::storage::models::{ChatConversationRecord, ChatMessageRecord};
    use crate::storage::service::StorageService;
    use crate::{
        ProviderContinuationVaultFactory, ProviderProfileConfig, ProviderProtocolDialect,
        ProviderProtocolKey, ReasoningEffort, ReasoningMode, ReasoningPolicy,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tempfile::tempdir;
    use tokio::io::AsyncWriteExt;
    use tokio::net::{TcpListener, TcpStream};
    use tokio::time::{timeout, Duration};

    const CONVERSATION_ID: &str = "conversation-deepseek-runtime-e2e";
    const ASSISTANT_MESSAGE_ID: &str = "assistant-deepseek-runtime-e2e";
    const RUN_ID: &str = "run-deepseek-runtime-e2e";
    const MODEL_ID: &str = "deepseek-v4-runtime-e2e";
    const FIRST_REASONING: &str = "第一轮 reasoning：\n逐字保留  alpha  ";
    const SECOND_REASONING: &str = "第二轮 reasoning：工具 1/2 都已完成。\n";

    fn provider_tool_call(id: &str, command: &str) -> Value {
        json!({
            "id": id,
            "type": "function",
            "function": {
                "name": "run_command",
                "arguments": serde_json::to_string(&json!({ "command": command })).unwrap()
            }
        })
    }

    fn matching_reasoning_turns<'a>(request: &'a Value, reasoning: &str) -> Vec<&'a Value> {
        request["messages"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|message| {
                message["role"] == "assistant"
                    && message["reasoning_content"].as_str() == Some(reasoning)
            })
            .collect()
    }

    fn validate_replayed_reasoning(request_index: usize, request: &Value) -> Result<(), String> {
        let messages = request["messages"]
            .as_array()
            .ok_or_else(|| "messages must be an array".to_string())?;
        let all_reasoning = messages
            .iter()
            .filter_map(|message| message.get("reasoning_content"))
            .collect::<Vec<_>>();
        let first_turns = matching_reasoning_turns(request, FIRST_REASONING);
        let second_turns = matching_reasoning_turns(request, SECOND_REASONING);
        let tool_result_ids = messages
            .iter()
            .filter(|message| message["role"] == "tool")
            .filter_map(|message| message["tool_call_id"].as_str())
            .collect::<Vec<_>>();

        match request_index {
            0 if all_reasoning.is_empty() => Ok(()),
            1 if first_turns.len() == 1 && second_turns.is_empty() => {
                let provider_call_ids = first_turns[0]["tool_calls"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|call| call["id"].as_str())
                    .collect::<Vec<_>>();
                if all_reasoning.len() == 1
                    && provider_call_ids == ["deepseek-provider-1a", "deepseek-provider-1b"]
                    && tool_result_ids == ["deepseek-provider-1a", "deepseek-provider-1b"]
                {
                    Ok(())
                } else {
                    Err("request 2 did not replay one grouped first turn".to_string())
                }
            }
            2 if first_turns.len() == 1 && second_turns.len() == 1 => {
                let first_provider_call_ids = first_turns[0]["tool_calls"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|call| call["id"].as_str())
                    .collect::<Vec<_>>();
                let second_provider_call_ids = second_turns[0]["tool_calls"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|call| call["id"].as_str())
                    .collect::<Vec<_>>();
                if all_reasoning.len() == 2
                    && first_provider_call_ids == ["deepseek-provider-1a", "deepseek-provider-1b"]
                    && second_provider_call_ids == ["deepseek-provider-2"]
                    && tool_result_ids
                        == [
                            "deepseek-provider-1a",
                            "deepseek-provider-1b",
                            "deepseek-provider-2",
                        ]
                {
                    Ok(())
                } else {
                    Err("request 3 did not replay both exact grouped turns".to_string())
                }
            }
            _ => Err(format!(
                "request {} omitted or duplicated byte-exact reasoning_content",
                request_index + 1
            )),
        }
    }

    async fn write_status_response(stream: &mut TcpStream, status: &str, body: Value) {
        let body = serde_json::to_vec(&body).unwrap();
        let headers = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(headers.as_bytes()).await.unwrap();
        stream.write_all(&body).await.unwrap();
    }

    let fixture = tempdir().unwrap();
    let workspace = fixture.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("runtime.sqlite")).unwrap());
    storage
        .save_conversation(ChatConversationRecord {
            id: CONVERSATION_ID.to_string(),
            project_id: None,
            model_id: Some(MODEL_ID.to_string()),
            title: "DeepSeek Runtime E2E".to_string(),
            messages: vec![ChatMessageRecord {
                id: ASSISTANT_MESSAGE_ID.to_string(),
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

    let credentials = Arc::new(InMemoryCredentialStore::default()) as Arc<dyn CredentialStore>;
    let vault = Arc::new(
        ProviderContinuationVaultFactory::open_or_provision(Arc::clone(&storage), credentials)
            .unwrap(),
    );
    let mut provider_profile = ProviderProfileConfig::deepseek_v4_default();
    let ProviderProfileConfig::V1(config) = &mut provider_profile else {
        unreachable!("legacy DeepSeek constructor must produce schema v1")
    };
    config.reasoning = ReasoningPolicy {
        mode: ReasoningMode::Enabled,
        effort: ReasoningEffort::Max,
    };
    let provider_protocol = ProviderProtocolKey::new(
        ProviderProtocolDialect::OpenAiChatCompletions,
        &provider_profile,
        MODEL_ID,
        None,
    )
    .unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let mut requests = Vec::new();
        for request_index in 0..3 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = read_runtime_test_json_request(&mut stream).await;
            requests.push(request.clone());
            if let Err(reason) = validate_replayed_reasoning(request_index, &request) {
                write_status_response(
                    &mut stream,
                    "400 Bad Request",
                    json!({ "error": { "message": reason } }),
                )
                .await;
                return (requests, Some(reason), true);
            }

            let response = match request_index {
                0 => json!({
                    "choices": [{
                        "message": {
                            "role": "assistant",
                            "content": "",
                            "reasoning_content": FIRST_REASONING,
                            "tool_calls": [
                                provider_tool_call("deepseek-provider-1a", "touch first-effect"),
                                provider_tool_call("deepseek-provider-1b", "touch second-effect")
                            ]
                        },
                        "finish_reason": "tool_calls"
                    }]
                }),
                1 => json!({
                    "choices": [{
                        "message": {
                            "role": "assistant",
                            "content": "",
                            "reasoning_content": SECOND_REASONING,
                            "tool_calls": [provider_tool_call(
                                "deepseek-provider-2",
                                "touch third-effect"
                            )]
                        },
                        "finish_reason": "tool_calls"
                    }]
                }),
                _ => json!({
                    "choices": [{
                        "message": {
                            "role": "assistant",
                            "content": "three persisted tool effects completed",
                            "reasoning_content": "最终 reasoning 不应触发第四次请求。"
                        },
                        "finish_reason": "stop"
                    }]
                }),
            };
            write_status_response(&mut stream, "200 OK", response).await;
        }

        let no_extra_request = match timeout(Duration::from_millis(300), listener.accept()).await {
            Ok(Ok((mut stream, _))) => {
                requests.push(read_runtime_test_json_request(&mut stream).await);
                write_status_response(
                    &mut stream,
                    "500 Internal Server Error",
                    json!({ "error": { "message": "unexpected fourth request" } }),
                )
                .await;
                false
            }
            Ok(Err(error)) => panic!("fake provider accept failed: {error}"),
            Err(_) => true,
        };
        (requests, None, no_extra_request)
    });

    let side_effects = Arc::new(AtomicUsize::new(0));
    let persisted_before_effect = Arc::new(Mutex::new(Vec::<(String, usize)>::new()));
    let executor_vault = Arc::clone(&vault);
    let executor_protocol = provider_protocol.clone();
    let executor_side_effects = Arc::clone(&side_effects);
    let executor_checks = Arc::clone(&persisted_before_effect);
    let host_executor: AgentHostActionExecutor = Arc::new(move |action, _, cancellation| {
        if cancellation.is_cancelled() {
            return Err(AgentError::new("unexpected cancellation"));
        }
        let AgentProposedAction::Command { command } = action else {
            return Err(AgentError::new("expected a command side effect"));
        };
        let expected_persisted_turns = if command.command == "touch third-effect" {
            2
        } else {
            1
        };
        let persisted_turns = executor_vault
            .list_replayable_for_conversation(CONVERSATION_ID, &executor_protocol)
            .map_err(|error| AgentError::new(error.to_string()))?;
        executor_checks
            .lock()
            .unwrap()
            .push((command.command.clone(), persisted_turns.len()));
        if persisted_turns.len() != expected_persisted_turns {
            return Err(AgentError::new(
                "tool side effect reached the Host before its provider turn was persisted",
            ));
        }
        executor_side_effects.fetch_add(1, Ordering::SeqCst);
        Ok(AgentToolResult {
            exact_archive_file: None,
            call_id: command.id,
            tool: "run_command".to_string(),
            ok: true,
            result: Some(json!({
                "exitCode": 0,
                "stdout": "fake side effect committed",
                "stderr": ""
            })),
            error: None,
        })
    });

    let input = AgentChatInput {
        api_url: format!("http://{address}/v1/chat/completions"),
        api_token: "deepseek-runtime-token".to_string(),
        provider_configuration_revision: None,
        provider_connection_revision: None,
        search_connection_revision: None,
        provider_profile_config: Some(provider_profile),
        provider_protocol_key: Some(provider_protocol.clone()),
        model: MODEL_ID.to_string(),
        model_capabilities: crate::ModelCapabilities::default(),
        api_style: Some(crate::protocol::AgentApiStyle::OpenAiCompatible),
        context_window_tokens: Some(128_000),
        context_window_indicator_enabled: true,
        max_tokens: Some(1_024),
        temperature: Some(0.3),
        stream: Some(false),
        context: Some(AgentRunContext {
            collaboration_identity: None,
            conversation_id: Some(CONVERSATION_ID.to_string()),
            project_id: None,
            workspace: Some(AgentWorkspaceContext {
                project_id: None,
                display_name: Some("DeepSeek E2E workspace".to_string()),
                root_path: Some(workspace.to_string_lossy().into_owned()),
            }),
            attachment_library: None,
            permissions: AgentPermissions {
                read: AgentReadPermission::All,
                write: AgentWritePermission::All,
                command: AgentCommandPermission::AutoApprove,
                command_safety: AgentCommandSafetyPolicy::FullAccess,
                patch: Default::default(),
                builtin_execution: Default::default(),
            },
        }),
        search_config: None,
        prompt_preferences: None,
        approval_decision: None,
        tool_continuation: None,
        attachments: Vec::new(),
        resume_checkpoint: None,
        assistant_message_id: Some(ASSISTANT_MESSAGE_ID.to_string()),
        context_compaction_summary: None,
        world_state_records: Vec::new(),
        skill_activation: None,
        skill_discovery: None,
        messages: vec![message(
            "user",
            "Run three fake side effects across two DeepSeek tool turns, then finish.",
        )],
    };
    let mut missing_reasoning_input = input.clone();
    let runtime_result = AgentRuntime::default()
        .send_chat_with_events_and_cancellation(
            input,
            Some(RUN_ID.to_string()),
            None,
            AgentCancellationToken::new(),
            Some(
                AgentRuntimeHostServices::new()
                    .with_host_actions(host_executor, Arc::clone(&storage))
                    .with_provider_continuation_vault(Arc::clone(&vault)),
            ),
        )
        .await;
    let output = runtime_result.unwrap();
    assert_eq!(output.status, AgentRunStatus::Completed);
    assert_eq!(output.content, "three persisted tool effects completed");
    let (requests, provider_rejection, no_extra_request) = server.await.unwrap();

    assert!(provider_rejection.is_none(), "{provider_rejection:?}");
    assert_eq!(side_effects.load(Ordering::SeqCst), 3);
    assert_eq!(
        persisted_before_effect.lock().unwrap().as_slice(),
        [
            ("touch first-effect".to_string(), 1),
            ("touch second-effect".to_string(), 1),
            ("touch third-effect".to_string(), 2),
        ]
    );
    assert_eq!(requests.len(), 3);
    assert!(
        no_extra_request,
        "runtime issued an unexpected fourth request"
    );

    let persisted_turns = vault
        .list_replayable_for_conversation(CONVERSATION_ID, &provider_protocol)
        .unwrap();
    assert_eq!(persisted_turns.len(), 2);
    assert_eq!(
        persisted_turns[0]
            .assistant_turn
            .provider_tool_calls()
            .len(),
        2,
        "one multi-call provider turn must have one continuation envelope"
    );
    assert_eq!(
        persisted_turns[1]
            .assistant_turn
            .provider_tool_calls()
            .len(),
        1
    );

    const MISSING_CONVERSATION_ID: &str = "conversation-deepseek-missing-reasoning";
    const MISSING_ASSISTANT_MESSAGE_ID: &str = "assistant-deepseek-missing-reasoning";
    storage
        .save_conversation(ChatConversationRecord {
            id: MISSING_CONVERSATION_ID.to_string(),
            project_id: None,
            model_id: Some(MODEL_ID.to_string()),
            title: "DeepSeek missing reasoning".to_string(),
            messages: vec![ChatMessageRecord {
                id: MISSING_ASSISTANT_MESSAGE_ID.to_string(),
                role: "assistant".to_string(),
                content: String::new(),
                created_at: 2,
                status: Some("pending".to_string()),
                attachments: Vec::new(),
                agent_run_json: None,
                ui_state_json: None,
            }],
            created_at: 2,
            updated_at: 2,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
    let missing_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let missing_address = missing_listener.local_addr().unwrap();
    let missing_server = tokio::spawn(async move {
        let (mut stream, _) = missing_listener.accept().await.unwrap();
        let request = read_runtime_test_json_request(&mut stream).await;
        write_status_response(
            &mut stream,
            "200 OK",
            json!({
                "choices": [{
                    "message": {
                        "role": "assistant",
                        "content": "",
                        "tool_calls": [provider_tool_call(
                            "deepseek-provider-missing-reasoning",
                            "touch must-not-run"
                        )]
                    },
                    "finish_reason": "tool_calls"
                }]
            }),
        )
        .await;
        let no_retry = timeout(Duration::from_millis(300), missing_listener.accept())
            .await
            .is_err();
        (request, no_retry)
    });
    missing_reasoning_input.api_url = format!("http://{missing_address}/v1/chat/completions");
    missing_reasoning_input
        .context
        .as_mut()
        .unwrap()
        .conversation_id = Some(MISSING_CONVERSATION_ID.to_string());
    missing_reasoning_input.assistant_message_id = Some(MISSING_ASSISTANT_MESSAGE_ID.to_string());
    missing_reasoning_input.messages = vec![message(
        "user",
        "Call one tool, but reject the response if its reasoning is missing.",
    )];
    let forbidden_side_effects = Arc::new(AtomicUsize::new(0));
    let executor_forbidden_side_effects = Arc::clone(&forbidden_side_effects);
    let forbidden_executor: AgentHostActionExecutor = Arc::new(move |action, _, _| {
        executor_forbidden_side_effects.fetch_add(1, Ordering::SeqCst);
        let AgentProposedAction::Command { command } = action else {
            return Err(AgentError::new("expected command"));
        };
        Ok(AgentToolResult {
            exact_archive_file: None,
            call_id: command.id,
            tool: "run_command".to_string(),
            ok: true,
            result: Some(json!({ "exitCode": 0 })),
            error: None,
        })
    });
    let missing_error = AgentRuntime::default()
        .send_chat_with_events_and_cancellation(
            missing_reasoning_input,
            Some("run-deepseek-missing-reasoning".to_string()),
            None,
            AgentCancellationToken::new(),
            Some(
                AgentRuntimeHostServices::new()
                    .with_host_actions(forbidden_executor, Arc::clone(&storage))
                    .with_provider_continuation_vault(Arc::clone(&vault)),
            ),
        )
        .await
        .unwrap_err();
    let (_, missing_no_retry) = missing_server.await.unwrap();
    assert_eq!(missing_error.code(), Some("provider_reasoning_required"));
    assert_eq!(forbidden_side_effects.load(Ordering::SeqCst), 0);
    assert!(
        missing_no_retry,
        "missing reasoning must not trigger a retry"
    );
    assert!(vault
        .list_replayable_for_conversation(MISSING_CONVERSATION_ID, &provider_protocol)
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn deepseek_ordinary_reasoning_survives_restart_for_a_future_tools_request() {
    use crate::image_generation::{CredentialStore, InMemoryCredentialStore};
    use crate::protocol::{
        AgentCommandPermission, AgentCommandSafetyPolicy, AgentPermissions, AgentReadPermission,
        AgentWritePermission,
    };
    use crate::provider_profile::{
        ProviderFamilyReasoningPolicy, ProviderFamilySettings, ProviderProfileRef,
        ProviderReasoningEffort, ProviderVendorId,
    };
    use crate::storage::models::{
        AgentRunGuidanceRecord, ChatConversationRecord, ChatMessageRecord,
    };
    use crate::storage::service::StorageService;
    use crate::{
        ProviderContinuationVaultFactory, ProviderProfileConfig, ProviderProtocolDialect,
        ProviderProtocolKey, ReasoningMode,
    };
    use std::sync::atomic::{AtomicI64, Ordering};
    use tempfile::tempdir;
    use tokio::net::TcpListener;
    use tokio::sync::oneshot;

    const MODEL_ID: &str = "deepseek-v4-flash";
    const CONVERSATION_ID: &str = "conversation-deepseek-ordinary-restart";
    const FIRST_ASSISTANT_ID: &str = "assistant-deepseek-ordinary-first";
    const NEXT_ASSISTANT_ID: &str = "assistant-deepseek-ordinary-next";
    const FIRST_RUN_ID: &str = "run-deepseek-ordinary-first";
    const NEXT_RUN_ID: &str = "run-deepseek-ordinary-next";
    const GUIDANCE_ID: &str = "guidance-deepseek-ordinary";
    const CLIENT_MESSAGE_ID: &str = "client-deepseek-ordinary";
    const STEER_REASONING: &str = "deepseek-private-ordinary-steer-reasoning-canary";
    const FINAL_REASONING: &str = "deepseek-private-ordinary-final-reasoning-canary";
    const STEER_VISIBLE: &str = "Visible DeepSeek answer before steering.";
    const FINAL_VISIBLE: &str = "Visible DeepSeek final answer.";

    fn pending_assistant(id: &str, created_at: i64) -> ChatMessageRecord {
        ChatMessageRecord {
            id: id.to_string(),
            role: "assistant".to_string(),
            content: String::new(),
            created_at,
            status: Some("pending".to_string()),
            attachments: Vec::new(),
            agent_run_json: None,
            ui_state_json: None,
        }
    }

    fn runtime_input(
        api_url: &str,
        profile: ProviderProfileConfig,
        protocol: ProviderProtocolKey,
        assistant_message_id: &str,
        messages: Vec<AgentChatMessage>,
    ) -> AgentChatInput {
        let mut input = conversation_context_input(messages);
        input.api_url = api_url.to_string();
        input.api_token = "deepseek-runtime-test-token".to_string();
        input.provider_profile_config = Some(profile);
        input.provider_protocol_key = Some(protocol);
        input.model = MODEL_ID.to_string();
        input.stream = Some(false);
        input.assistant_message_id = Some(assistant_message_id.to_string());
        input.context = Some(AgentRunContext {
            collaboration_identity: None,
            conversation_id: Some(CONVERSATION_ID.to_string()),
            project_id: None,
            workspace: None,
            attachment_library: None,
            permissions: Default::default(),
        });
        input
    }

    fn durable_assistant_history(
        storage: &StorageService,
        assistant_message_id: &str,
    ) -> AgentChatMessage {
        let conversation = storage
            .load_conversation(CONVERSATION_ID)
            .unwrap()
            .expect("restart must reload the durable DeepSeek conversation");
        let assistant = conversation
            .messages
            .iter()
            .find(|message| message.id == assistant_message_id)
            .expect("restart must reload the durable DeepSeek assistant message");
        AgentChatMessage {
            message_id: Some(assistant_message_id.to_string()),
            role: "assistant".to_string(),
            content: assistant.content.clone(),
            created_at: Some(assistant.created_at),
            conversation_turn_trace: Some(
                storage
                    .get_conversation_turn_trace(assistant_message_id)
                    .unwrap()
                    .expect("restart must reload the durable DeepSeek trace"),
            ),
            conversation_model_context_items: storage
                .get_conversation_model_context_log(assistant_message_id)
                .unwrap()
                .expect("restart must reload the exact DeepSeek model context")
                .items,
        }
    }

    fn count_request_occurrences(request: &Value, needle: &str) -> usize {
        serde_json::to_string(request)
            .unwrap()
            .matches(needle)
            .count()
    }

    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("runtime.sqlite");
    let workspace = fixture.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    storage
        .save_conversation(ChatConversationRecord {
            id: CONVERSATION_ID.to_string(),
            project_id: None,
            model_id: Some(MODEL_ID.to_string()),
            title: "DeepSeek ordinary restart".to_string(),
            messages: vec![
                pending_assistant(FIRST_ASSISTANT_ID, 1),
                pending_assistant(NEXT_ASSISTANT_ID, 2),
            ],
            created_at: 1,
            updated_at: 2,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
    storage
        .store_agent_run_guidance(AgentRunGuidanceRecord {
            guidance_id: GUIDANCE_ID.to_string(),
            client_message_id: CLIENT_MESSAGE_ID.to_string(),
            run_id: FIRST_RUN_ID.to_string(),
            conversation_id: CONVERSATION_ID.to_string(),
            assistant_message_id: FIRST_ASSISTANT_ID.to_string(),
            content: "Continue after this ordinary response.".to_string(),
            status: crate::AgentGuidanceStatus::Queued,
            attachment_ids: Vec::new(),
            applied_trace_sequence: None,
            terminal_reason: None,
            created_at: 3,
            updated_at: 3,
        })
        .unwrap();

    let credentials = Arc::new(InMemoryCredentialStore::default()) as Arc<dyn CredentialStore>;
    let vault = Arc::new(
        ProviderContinuationVaultFactory::open_or_provision(
            Arc::clone(&storage),
            Arc::clone(&credentials),
        )
        .unwrap(),
    );
    let profile = ProviderProfileConfig::from_family_settings(
        ProviderProfileRef::deepseek_v4_chat(),
        ProviderVendorId::DeepSeek,
        ProviderFamilySettings::DeepseekV4Chat {
            reasoning: ProviderFamilyReasoningPolicy {
                mode: ReasoningMode::Enabled,
                effort: ProviderReasoningEffort::Low,
            },
        },
    );
    let protocol = ProviderProtocolKey::new(
        ProviderProtocolDialect::OpenAiChatCompletions,
        &profile,
        MODEL_ID,
        None,
    )
    .unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let api_url = format!("http://{address}/v1/chat/completions");
    let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let requests_for_server = Arc::clone(&requests);
    let (first_request_seen_tx, first_request_seen_rx) = oneshot::channel();
    let (release_first_response_tx, release_first_response_rx) = oneshot::channel();
    let server = tokio::spawn(async move {
        let mut first_request_seen_tx = Some(first_request_seen_tx);
        let mut release_first_response_rx = Some(release_first_response_rx);
        for request_index in 0..3 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = read_runtime_test_json_request(&mut stream).await;
            requests_for_server.lock().unwrap().push(request);
            if request_index == 0 {
                first_request_seen_tx.take().unwrap().send(()).unwrap();
                release_first_response_rx.take().unwrap().await.unwrap();
            }
            let response = match request_index {
                0 => json!({
                    "choices": [{
                        "message": {
                            "role": "assistant",
                            "content": STEER_VISIBLE,
                            "reasoning_content": STEER_REASONING
                        },
                        "finish_reason": "stop"
                    }]
                }),
                1 => json!({
                    "choices": [{
                        "message": {
                            "role": "assistant",
                            "content": FINAL_VISIBLE,
                            "reasoning_content": FINAL_REASONING
                        },
                        "finish_reason": "stop"
                    }]
                }),
                _ => json!({
                    "choices": [{
                        "message": {
                            "role": "assistant",
                            "content": "DeepSeek future-tools replay verified.",
                            "reasoning_content": "private follow-up reasoning"
                        },
                        "finish_reason": "stop"
                    }]
                }),
            };
            write_runtime_test_json_response(&mut stream, response).await;
        }
    });

    let snapshots = Arc::new(Mutex::new(Vec::<ConversationTraceSnapshot>::new()));
    let snapshots_for_observer = Arc::clone(&snapshots);
    let storage_for_observer = Arc::clone(&storage);
    let observer_timestamp = Arc::new(AtomicI64::new(10));
    let observer_timestamp_for_callback = Arc::clone(&observer_timestamp);
    let trace_observer: AgentConversationTraceObserver = Arc::new(move |snapshot| {
        let updated_at = observer_timestamp_for_callback.fetch_add(1, Ordering::SeqCst);
        storage_for_observer
            .append_in_progress_conversation_turn_trace_and_apply_guidances(
                &snapshot.in_progress_audit_trace(
                    FIRST_RUN_ID,
                    CONVERSATION_ID,
                    FIRST_ASSISTANT_ID,
                ),
                &snapshot.model_context_items,
                1,
                updated_at,
            )
            .map_err(AgentError::new)?;
        snapshots_for_observer.lock().unwrap().push(snapshot);
        Ok(None)
    });

    let queue = AgentSteerInputQueue::new();
    let first_input = runtime_input(
        &api_url,
        profile.clone(),
        protocol.clone(),
        FIRST_ASSISTANT_ID,
        vec![message(
            "user",
            "Begin the DeepSeek ordinary reasoning turn.",
        )],
    );
    let runtime_queue = queue.clone();
    let runtime_vault = Arc::clone(&vault);
    let runtime_storage = Arc::clone(&storage);
    let first_runtime = tokio::spawn(async move {
        AgentRuntime::default()
            .send_chat_with_events_and_cancellation(
                first_input,
                Some(FIRST_RUN_ID.to_string()),
                None,
                AgentCancellationToken::new(),
                Some(
                    AgentRuntimeHostServices::new()
                        .with_storage(runtime_storage)
                        .with_provider_continuation_vault(runtime_vault)
                        .with_trace_observer(trace_observer)
                        .with_steer_input(runtime_queue),
                ),
            )
            .await
            .unwrap()
    });
    first_request_seen_rx.await.unwrap();
    assert_eq!(
        queue
            .enqueue(runtime_steer_input(
                GUIDANCE_ID,
                CLIENT_MESSAGE_ID,
                "Continue after this ordinary response.",
            ))
            .unwrap(),
        AgentSteerEnqueueOutcome::Queued
    );
    release_first_response_tx.send(()).unwrap();
    let first_output = first_runtime.await.unwrap();
    assert_eq!(first_output.status, AgentRunStatus::Completed);
    assert_eq!(first_output.content, FINAL_VISIBLE);
    let terminal_trace = first_output
        .conversation_turn_trace
        .clone()
        .expect("DeepSeek runtime must return its terminal trace");
    let last_snapshot = snapshots
        .lock()
        .unwrap()
        .last()
        .cloned()
        .expect("DeepSeek steer must publish a durable snapshot");
    storage
        .finalize_chat_message_with_conversation_trace_model_context_and_usage(
            CONVERSATION_ID,
            FIRST_ASSISTANT_ID,
            &first_output.content,
            Some("sent"),
            "completed",
            &terminal_trace,
            Some(&last_snapshot.model_context_items),
            1,
            100,
            None,
            None,
        )
        .unwrap();
    assert_eq!(
        vault
            .list_replayable_for_conversation(CONVERSATION_ID, &protocol)
            .unwrap()
            .len(),
        2
    );
    for public_projection in [
        serde_json::to_string(&first_output).unwrap(),
        serde_json::to_string(&terminal_trace).unwrap(),
        serde_json::to_string(&last_snapshot.model_context_items).unwrap(),
    ] {
        assert!(!public_projection.contains(STEER_REASONING));
        assert!(!public_projection.contains(FINAL_REASONING));
        assert!(!public_projection.contains("providerContinuation"));
    }

    drop(vault);
    drop(storage);
    let restarted_storage = Arc::new(StorageService::open(&database_path).unwrap());
    let restarted_vault = Arc::new(
        ProviderContinuationVaultFactory::open_or_provision(
            Arc::clone(&restarted_storage),
            Arc::clone(&credentials),
        )
        .unwrap(),
    );
    let durable_history = durable_assistant_history(&restarted_storage, FIRST_ASSISTANT_ID);
    let mut restarted_input = runtime_input(
        &api_url,
        profile,
        protocol,
        NEXT_ASSISTANT_ID,
        vec![
            message("user", "Begin the DeepSeek ordinary reasoning turn."),
            durable_history,
            message("user", "Use the available tools if needed."),
        ],
    );
    restarted_input.context.as_mut().unwrap().workspace = Some(AgentWorkspaceContext {
        project_id: None,
        display_name: Some("DeepSeek future-tools workspace".to_string()),
        root_path: Some(workspace.to_string_lossy().into_owned()),
    });
    restarted_input.context.as_mut().unwrap().permissions = AgentPermissions {
        read: AgentReadPermission::All,
        write: AgentWritePermission::All,
        command: AgentCommandPermission::RequireApproval,
        command_safety: AgentCommandSafetyPolicy::FullAccess,
        patch: Default::default(),
        builtin_execution: Default::default(),
    };
    let forbidden_executor: AgentHostActionExecutor = Arc::new(|_, _, _| {
        Err(AgentError::new(
            "future-tools replay test must not execute a tool",
        ))
    });
    let restarted_output = AgentRuntime::default()
        .send_chat_with_events_and_cancellation(
            restarted_input,
            Some(NEXT_RUN_ID.to_string()),
            None,
            AgentCancellationToken::new(),
            Some(
                AgentRuntimeHostServices::new()
                    .with_host_actions(forbidden_executor, Arc::clone(&restarted_storage))
                    .with_provider_continuation_vault(restarted_vault),
            ),
        )
        .await
        .unwrap();
    assert_eq!(restarted_output.status, AgentRunStatus::Completed);

    server.await.unwrap();
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 3);
    assert_eq!(count_request_occurrences(&requests[0], STEER_REASONING), 0);
    assert_eq!(count_request_occurrences(&requests[0], FINAL_REASONING), 0);
    assert_eq!(count_request_occurrences(&requests[2], STEER_REASONING), 1);
    assert_eq!(count_request_occurrences(&requests[2], FINAL_REASONING), 1);
    assert!(requests[2]
        .get("tools")
        .and_then(Value::as_array)
        .is_some_and(|tools| !tools.is_empty()));
}
