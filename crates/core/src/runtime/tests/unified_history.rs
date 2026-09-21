use super::*;

/// Exercises real Runtime recording, SQLite restart, private Provider rehydration and final
/// native and generic wire payloads. No cache percentage is assumed: the old request must be an exact
/// message prefix of the next request when the settings and tool catalog are unchanged.
#[tokio::test]
async fn unified_history_keeps_complete_deepseek_request_prefix_across_runs_and_restart() {
    assert_unified_history_wire(crate::AgentApiStyle::OpenAiCompatible, true).await;
}

#[tokio::test]
async fn unified_history_keeps_complete_generic_openai_request_prefix_across_runs_and_restart() {
    assert_unified_history_wire(crate::AgentApiStyle::OpenAiCompatible, false).await;
}

#[tokio::test]
async fn unified_history_keeps_complete_generic_anthropic_request_prefix_across_runs_and_restart() {
    assert_unified_history_wire(crate::AgentApiStyle::AnthropicCompatible, false).await;
}

async fn assert_unified_history_wire(style: crate::AgentApiStyle, native_deepseek: bool) {
    use crate::image_generation::{CredentialStore, InMemoryCredentialStore};
    use crate::protocol::{AgentPermissions, AgentReadPermission};
    use crate::storage::models::{ChatConversationRecord, ChatMessageRecord};
    use crate::storage::service::StorageService;
    use crate::ProviderContinuationVaultFactory;
    use tempfile::tempdir;
    use tokio::net::TcpListener;
    use tokio::time::{timeout, Duration};

    const CONVERSATION: &str = "unified-history-conversation";
    const NARRATION: &str = "先读取文件，然后继续。";
    const PRIVATE_REASONING: &str = "private-history-reasoning";
    let model = if native_deepseek {
        "deepseek-flash"
    } else {
        "test-model"
    };
    let fixture = tempdir().unwrap();
    let workspace = fixture.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    std::fs::write(workspace.join("a.txt"), "A history result").unwrap();
    std::fs::write(workspace.join("b.txt"), "B history result").unwrap();
    let database = fixture.path().join("runtime.sqlite");
    let mut storage = Arc::new(StorageService::open(&database).unwrap());
    storage
        .save_conversation(ChatConversationRecord {
            id: CONVERSATION.to_string(),
            project_id: None,
            model_id: Some(model.to_string()),
            title: "Unified history".to_string(),
            messages: (0..3)
                .map(|index| ChatMessageRecord {
                    id: format!("unified-assistant-{index}"),
                    role: "assistant".to_string(),
                    content: String::new(),
                    created_at: 10 + index,
                    status: Some("pending".to_string()),
                    attachments: Vec::new(),
                    folder_references_json: None,
                    agent_run_json: None,
                    ui_state_json: None,
                    human_interaction_response: None,
                })
                .collect(),
            created_at: 1,
            updated_at: 1,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
    let credentials = Arc::new(InMemoryCredentialStore::default()) as Arc<dyn CredentialStore>;
    let mut vault = Arc::new(
        ProviderContinuationVaultFactory::open_or_provision(
            Arc::clone(&storage),
            Arc::clone(&credentials),
        )
        .unwrap(),
    );
    let dialect = ProviderProtocolDialect::from(style);
    let profile = if native_deepseek {
        ProviderProfileConfig::deepseek_flash_default()
    } else {
        ProviderProfileConfig::generic_for_dialect(dialect)
    };
    let protocol = ProviderProtocolKey::new(dialect, &profile, model, None).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!(
        "http://{}/v1/{}",
        listener.local_addr().unwrap(),
        match style {
            crate::AgentApiStyle::OpenAiCompatible => "chat/completions",
            crate::AgentApiStyle::AnthropicCompatible => "messages",
        }
    );
    let server = tokio::spawn(async move {
        let mut requests = Vec::new();
        for index in 0..4 {
            let (mut stream, _) = timeout(Duration::from_secs(20), listener.accept())
                .await
                .unwrap()
                .unwrap();
            requests.push(read_runtime_test_json_request(&mut stream).await);
            let response = if style == crate::AgentApiStyle::AnthropicCompatible {
                if index == 0 {
                    json!({"content":[
                        {"type":"text","text":NARRATION},
                        {"type":"tool_use","id":"provider-history-a","name":"read_file","input":{"path":"a.txt"}},
                        {"type":"tool_use","id":"provider-history-b","name":"read_file","input":{"path":"b.txt"}}
                    ],"stop_reason":"tool_use"})
                } else {
                    json!({"content":[{"type":"text","text":format!("完成 {index}")}],"stop_reason":"end_turn"})
                }
            } else if index == 0 {
                json!({"choices":[{"message":{
                    "role":"assistant", "content":NARRATION, "reasoning_content":PRIVATE_REASONING,
                    "tool_calls": [
                        {"id":"provider-history-a","type":"function","function":{"name":"read_file","arguments":"{\"path\":\"a.txt\"}"}},
                        {"id":"provider-history-b","type":"function","function":{"name":"read_file","arguments":"{\"path\":\"b.txt\"}"}}
                    ]},"finish_reason":"tool_calls"}]})
            } else {
                json!({"choices":[{"message":{"role":"assistant","content":format!("完成 {index}"),"reasoning_content":format!("private-final-{index}")},"finish_reason":"stop"}]})
            };
            write_runtime_test_json_response(&mut stream, response).await;
        }
        requests
    });
    let mut history = Vec::new();
    for index in 0..3 {
        let assistant_id = format!("unified-assistant-{index}");
        let run_id = format!("unified-run-{index}");
        history.push(message("user", &format!("用户输入 {index}")));
        let mut input = conversation_context_input(history.clone());
        input.api_url = endpoint.clone();
        input.api_token = "test-token".to_string();
        input.model = model.to_string();
        input.api_style = Some(style);
        input.provider_profile_config = Some(profile.clone());
        input.provider_protocol_key = Some(protocol.clone());
        input.stream = Some(false);
        input.assistant_message_id = Some(assistant_id.clone());
        input.skill_activation = Some(activated_skill(
            "KEEP_THIS_SKILL_EXACT\n完整说明保持原始位置。",
        ));
        input.context = Some(AgentRunContext {
            collaboration_identity: None,
            conversation_id: Some(CONVERSATION.to_string()),
            project_id: None,
            workspace: Some(AgentWorkspaceContext {
                folders: Vec::new(),
                project_id: None,
                display_name: Some("History workspace".to_string()),
                root_path: Some(workspace.to_string_lossy().into_owned()),
            }),
            attachment_library: None,
            permissions: AgentPermissions {
                read: AgentReadPermission::All,
                ..Default::default()
            },
        });
        if index == 0 {
            let (attachment, library) = managed_runtime_attachment(
                &workspace,
                "history-notes",
                "notes.txt",
                "text/plain",
                AgentInputAttachmentKind::File,
                b"KEEP_THIS_ATTACHMENT",
            );
            input.attachments = vec![attachment];
            set_runtime_attachment_library(&mut input, library);
        }
        let recorded = Arc::new(Mutex::new(None::<ConversationTraceSnapshot>));
        let recorded_for_observer = Arc::clone(&recorded);
        let storage_for_observer = Arc::clone(&storage);
        let observed_run = run_id.clone();
        let observed_assistant = assistant_id.clone();
        let observer: AgentConversationTraceObserver = Arc::new(move |snapshot| {
            storage_for_observer
                .append_in_progress_conversation_turn_trace_and_apply_guidances(
                    &snapshot.in_progress_audit_trace(
                        &observed_run,
                        CONVERSATION,
                        &observed_assistant,
                    ),
                    &snapshot.model_context_items,
                    10 + index,
                    100 + index,
                )
                .map_err(AgentError::new)?;
            *recorded_for_observer.lock().unwrap() = Some(snapshot);
            Ok(None)
        });
        let output = timeout(
            Duration::from_secs(20),
            AgentRuntime::default().send_chat_with_events_and_cancellation(
                input,
                Some(run_id),
                None,
                AgentCancellationToken::new(),
                Some(
                    AgentRuntimeHostServices::new()
                        .with_storage(Arc::clone(&storage))
                        .with_provider_continuation_vault(Arc::clone(&vault))
                        .with_trace_observer(observer)
                        .with_skill_resources(activated_skill_authority()),
                ),
            ),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(output.status, AgentRunStatus::Completed);
        let trace = output.conversation_turn_trace.as_ref().unwrap();
        let snapshot = recorded.lock().unwrap().clone().unwrap();
        assert!(trace.items.iter().any(|item| matches!(
            item,
            ConversationTurnTraceItem::ContextMaterial {
                material_kind: crate::ConversationContextMaterialKind::RunWorldState,
                ..
            }
        )));
        let public = serde_json::to_string(&snapshot.model_context_items).unwrap();
        assert!(!public.contains(PRIVATE_REASONING));
        storage
            .finalize_chat_message_with_conversation_trace_model_context_and_usage(
                CONVERSATION,
                &assistant_id,
                &output.content,
                Some("sent"),
                "completed",
                trace,
                Some(&snapshot.model_context_items),
                10 + index,
                200 + index,
                None,
                None,
            )
            .unwrap();
        if index == 0 {
            drop(vault);
            drop(storage);
            storage = Arc::new(StorageService::open(&database).unwrap());
            vault = Arc::new(
                ProviderContinuationVaultFactory::open_or_provision(
                    Arc::clone(&storage),
                    Arc::clone(&credentials),
                )
                .unwrap(),
            );
        }
        history.push(AgentChatMessage {
            conversation_completion_covered: false,
            message_id: Some(assistant_id.clone()),
            role: "assistant".to_string(),
            content: output.content,
            created_at: Some(10 + index),
            conversation_turn_trace: storage.get_conversation_turn_trace(&assistant_id).unwrap(),
            conversation_model_context_items: storage
                .get_conversation_model_context_log(&assistant_id)
                .unwrap()
                .unwrap()
                .items,
        });
    }
    let requests = timeout(Duration::from_secs(20), server)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(requests.len(), 4);
    for (previous, next) in [(1, 2), (2, 3)] {
        assert_eq!(requests[previous]["tools"], requests[next]["tools"]);
        assert_eq!(requests[previous]["system"], requests[next]["system"]);
        let before = requests[previous]["messages"].as_array().unwrap();
        let after = requests[next]["messages"].as_array().unwrap();
        let matched = before
            .iter()
            .zip(after)
            .take_while(|(left, right)| left == right)
            .count();
        assert_eq!(matched, before.len(), "first changed message at index {matched}, previous request {previous}, next request {next}: before={} after={}",
            before.get(matched).map(|value| value.to_string().chars().take(600).collect::<String>()).unwrap_or_default(),
            after.get(matched).map(|value| value.to_string().chars().take(600).collect::<String>()).unwrap_or_default());
        let serialized = serde_json::to_string(&requests[next]).unwrap();
        assert_eq!(
            serialized.matches(NARRATION).count(),
            1,
            "tool-turn narration must not duplicate"
        );
        assert_eq!(
            serialized.matches(PRIVATE_REASONING).count(),
            usize::from(native_deepseek)
        );
        assert_eq!(serialized.matches("KEEP_THIS_ATTACHMENT").count(), 1);
        assert_eq!(serialized.matches("A history result").count(), 1);
        assert_eq!(serialized.matches("B history result").count(), 1);
    }
}

#[test]
fn historical_image_references_restore_exact_order_without_persisting_bytes() {
    use crate::context::ConversationTraceRenderer;
    use sha2::{Digest, Sha256};
    let attachments = [
        AgentInputAttachment {
            id: "image-a".into(),
            kind: AgentInputAttachmentKind::Image,
            name: "a.png".into(),
            mime_type: Some("image/png".into()),
            size_bytes: 3,
            encoding: AgentInputAttachmentEncoding::Base64,
            data: "YWJj".into(),
            content_sha256: None,
            truncated: None,
        },
        AgentInputAttachment {
            id: "image-b".into(),
            kind: AgentInputAttachmentKind::Image,
            name: "b.png".into(),
            mime_type: Some("image/png".into()),
            size_bytes: 3,
            encoding: AgentInputAttachmentEncoding::Base64,
            data: "ZGVm".into(),
            content_sha256: None,
            truncated: None,
        },
    ];
    let images = vec![
        crate::ConversationContextImageRef {
            attachment_id: "image-b".into(),
            mime_type: "image/png".into(),
            sha256: format!("sha256:{:x}", Sha256::digest(b"def")),
        },
        crate::ConversationContextImageRef {
            attachment_id: "image-a".into(),
            mime_type: "image/png".into(),
            sha256: format!("sha256:{:x}", Sha256::digest(b"abc")),
        },
    ];
    let mut recorder = ConversationTraceRecorder::default();
    recorder
        .record_context_material(
            "images",
            crate::ConversationContextMaterialKind::InputAttachment,
            "B then A",
            &images,
            1,
        )
        .unwrap();
    let snapshot = recorder.snapshot();
    let serialized = serde_json::to_string(&snapshot.model_context_items).unwrap();
    assert!(!serialized.contains("YWJj"));
    assert!(!serialized.contains("ZGVm"));
    let trace = snapshot.in_progress_audit_trace("run", "conversation", "assistant");
    let mut restored = ContextFrame::new(
        ConversationTraceRenderer::render_with_model_context(&trace, &snapshot.model_context_items)
            .unwrap()
            .activity_items,
    );
    restored.hydrate_context_images(&attachments).unwrap();
    let messages = restored.to_messages();
    assert_eq!(messages[0].content(), "B then A");
    assert_eq!(
        messages[0]
            .images()
            .iter()
            .map(|image| image.data_base64.as_str())
            .collect::<Vec<_>>(),
        ["ZGVm", "YWJj"]
    );
    let before = messages;
    restored.hydrate_context_images(&attachments).unwrap();
    assert_eq!(restored.to_messages(), before);
    let mut changed = attachments.clone();
    changed[0].data = "eHl6".into();
    assert!(restored.hydrate_context_images(&changed).is_err());
    assert!(restored.hydrate_context_images(&attachments[..1]).is_err());
}
