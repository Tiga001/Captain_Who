use super::provider_profiles::{
    read_provider_request, save_provider_profile_fixture, write_provider_stream,
};
use super::*;
use std::time::Duration;
use tokio::net::TcpListener;

/// Exercise both Host persistence entrances with an actual provider-owned grouped turn. The
/// first open call deliberately loses its derived cache, just like an eviction/rebuild boundary.
/// Neither entrance may hydrate the staged native turn before all its calls are visible.
async fn open_provider_turn_at_cold_boundary(native: bool, narration: bool, publication: bool) {
    const CONVERSATION: &str = "conversation-native-partial";
    const ASSISTANT: &str = "assistant-native-partial";
    const RUN: &str = "run-native-partial";
    let fixture = tempdir().unwrap();
    let sources = [
        fixture.path().join("first.txt"),
        fixture.path().join("second.txt"),
    ];
    for source in &sources {
        fs::write(source, "native partial context fixture").unwrap();
    }
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let model_server = tokio::spawn(async move {
        let (mut first, _) = listener.accept().await.unwrap();
        read_provider_request(&mut first).await;
        write_provider_stream(&mut first, json!({
            "role": "assistant",
            "reasoning_content": native.then_some("Inspect the requested files before answering."),
            "content": if narration { "I will inspect both files." } else { "" },
            "tool_calls": (0..2).map(|index| json!({
                "index": index, "id": format!("provider-read-{index}"), "type": "function",
                "function": { "name": "read_file", "arguments": json!({ "path": sources[index] }).to_string() }
            })).collect::<Vec<_>>()
        }), "tool_calls").await;
        drop(first);
        let (mut second, _) = listener.accept().await.unwrap();
        let request = read_provider_request(&mut second).await;
        write_provider_stream(
            &mut second,
            json!({ "role": "assistant", "content": "Both files inspected." }),
            "stop",
        )
        .await;
        request
    });
    let credentials =
        Arc::new(mycopilot_core::image_generation::InMemoryCredentialStore::default());
    let storage = Arc::new(
        StorageService::open_with_model_credentials(
            &fixture.path().join("storage.sqlite"),
            credentials.clone(),
        )
        .unwrap(),
    );
    let api_url = format!("http://{address}/v1/chat/completions");
    save_provider_profile_fixture(
        &storage,
        &api_url,
        native.then(mycopilot_core::ProviderProfileConfig::deepseek_flash_default),
    );
    storage
        .save_conversation(ChatConversationRecord {
            id: CONVERSATION.into(),
            project_id: None,
            model_id: Some("model-1".into()),
            title: "Native partial context".into(),
            messages: vec![
                ChatMessageRecord {
                    human_interaction_response: None,
                    id: "user-native-partial".into(),
                    role: "user".into(),
                    content: "Inspect both files".into(),
                    created_at: 1,
                    status: Some("sent".into()),
                    attachments: vec![],
                    folder_references_json: None,
                    agent_run_json: None,
                    ui_state_json: None,
                },
                ChatMessageRecord {
                    human_interaction_response: None,
                    id: ASSISTANT.into(),
                    role: "assistant".into(),
                    content: String::new(),
                    created_at: 2,
                    status: Some("pending".into()),
                    attachments: vec![],
                    folder_references_json: None,
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
    let vault = Arc::new(
        mycopilot_core::ProviderContinuationVaultFactory::open_or_provision(
            storage.clone(),
            credentials,
        )
        .unwrap(),
    );
    let service =
        AgentService::try_new_deferred_startup_reconciliation_with_provider_continuation_vault(
            storage.clone(),
            Some(vault),
        )
        .unwrap();
    let mut input: AgentChatInput = serde_json::from_value(json!({
        "apiUrl": api_url, "apiToken": "provider-profile-token", "model": if native { "deepseek-flash" } else { "model-1" },
        "modelCapabilities": { "imageInput": native }, "contextWindowTokens": 128000,
        "contextWindowIndicatorEnabled": true, "stream": true, "maxTokens": 30000, "assistantMessageId": ASSISTANT,
        "context": { "conversationId": CONVERSATION, "projectId": null, "workspace": null, "permissions": { "read": "all", "write": "denied", "command": "require_approval", "commandSafety": "guarded", "patch": "require_approval", "builtinExecution": "require_approval" } },
        "messages": [{ "messageId": "user-native-partial", "role": "user", "content": "Inspect both files", "createdAt": 1 }]
    })).unwrap();
    freeze_test_pending_provider_configuration(&storage, &mut input);
    admit_test_conversation_run(
        &storage,
        &ConversationTraceSnapshot::default().in_progress_trace(RUN, CONVERSATION, ASSISTANT),
        permissions_from_input(&input),
        2,
    );
    let (notifications, _receiver) = crate::transport::outbound_channel();
    let host_observer = if publication {
        service.trace_observer(
            RUN,
            CONVERSATION,
            ASSISTANT,
            2,
            input.clone(),
            RunContextToolProjection::pending(),
            notifications,
        )
    } else {
        let service = service.clone();
        let input = input.clone();
        let revision = conversation_context_configuration_revision(&input).unwrap();
        Arc::new(move |value: mycopilot_core::ConversationTracePublication| {
            service
                .persist_in_progress_trace_snapshot(
                    RUN,
                    CONVERSATION,
                    ASSISTANT,
                    2,
                    &input,
                    &notifications,
                    &value.into_snapshot(),
                    &revision,
                    None,
                )
                .map_err(|error| AgentError::new(format!("{error:?}")))
        }) as AgentConversationTraceObserver
    };
    let snapshots = Arc::new(Mutex::new(None));
    let observed = Arc::new(Mutex::new(Vec::new()));
    let observer = {
        let service = service.clone();
        let snapshots = snapshots.clone();
        let observed = observed.clone();
        Arc::new(move |value: mycopilot_core::ConversationTracePublication| {
            let call_count = value
                .trace_items()
                .iter()
                .filter(|item| matches!(item.as_ref(), ConversationTurnTraceItem::ToolCall { .. }))
                .count();
            if call_count > 0 {
                service.invalidate_conversation_context_state(CONVERSATION);
            }
            let baseline = host_observer(value.clone())?;
            if call_count > 0 {
                observed
                    .lock()
                    .unwrap()
                    .push((call_count, baseline.is_some()));
            }
            *snapshots.lock().unwrap() = Some(value.into_snapshot());
            Ok(baseline)
        }) as AgentConversationTraceObserver
    };
    let output = tokio::time::timeout(
        Duration::from_secs(10),
        send_chat_with_host_services(
            input.clone(),
            RUN.into(),
            Arc::new(|_| {}),
            AgentCancellationToken::new(),
            service
                .context_window_provider_host_services()
                .with_trace_observer(observer),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        output.status,
        AgentRunStatus::Completed,
        "native={native}, narration={narration}, publication={publication}: {output:?}"
    );
    let request = tokio::time::timeout(Duration::from_secs(5), model_server)
        .await
        .unwrap()
        .unwrap();
    let messages = request["messages"].as_array().unwrap();
    let tool_results = messages
        .iter()
        .filter(|message| message["role"] == "tool")
        .collect::<Vec<_>>();
    assert_eq!(tool_results.len(), 2);
    assert!(
        tool_results.iter().all(|message| message["content"]
            .as_str()
            .is_some_and(|content| content.contains("native partial context fixture"))),
        "{tool_results:?}"
    );
    let observed = observed.lock().unwrap();
    assert!(observed.iter().any(|(count, _)| *count == 1));
    assert!(observed.iter().any(|(count, _)| *count == 2));
    assert!(
        observed
            .iter()
            .all(|(_, replacement)| *replacement != native),
        "{observed:?}"
    );
    drop(observed);
    let snapshot = snapshots.lock().unwrap().clone().unwrap();
    storage
        .finalize_chat_message_with_conversation_trace_model_context_and_usage(
            CONVERSATION,
            ASSISTANT,
            &output.content,
            Some("sent"),
            "completed",
            output.conversation_turn_trace.as_ref().unwrap(),
            Some(&snapshot.model_context_items),
            2,
            3,
            None,
            None,
        )
        .unwrap();
    let window = service
        .finalize_conversation_context_state(&input, RUN, CONVERSATION, ASSISTANT, &output.content)
        .unwrap()
        .unwrap();
    assert!(window.input_tokens > 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_open_tool_publication_defers_cold_rebuild_until_terminal() {
    for narration in [false, true] {
        open_provider_turn_at_cold_boundary(true, narration, true).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_open_tool_snapshot_defers_cold_rebuild_until_terminal() {
    for narration in [false, true] {
        open_provider_turn_at_cold_boundary(true, narration, false).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn generic_tool_turns_keep_incremental_baselines_at_cold_boundaries() {
    for publication in [false, true] {
        open_provider_turn_at_cold_boundary(false, true, publication).await;
    }
}
