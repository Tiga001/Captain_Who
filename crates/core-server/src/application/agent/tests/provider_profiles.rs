use super::*;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

async fn read_provider_request(stream: &mut TcpStream) -> Value {
    let mut request = Vec::new();
    let mut buffer = [0_u8; 4_096];
    let mut body_start = None;
    let mut expected_length = None;
    loop {
        let read = stream.read(&mut buffer).await.unwrap();
        assert!(read > 0, "provider fixture closed before request completed");
        request.extend_from_slice(&buffer[..read]);
        if body_start.is_none() {
            if let Some(index) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                let start = index + 4;
                let headers = String::from_utf8_lossy(&request[..index]);
                let content_length = headers
                    .lines()
                    .find_map(|line| {
                        line.split_once(':').and_then(|(name, value)| {
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().ok())
                                .flatten()
                        })
                    })
                    .unwrap();
                body_start = Some(start);
                expected_length = Some(start + content_length);
            }
        }
        if expected_length.is_some_and(|length| request.len() >= length) {
            break;
        }
    }
    serde_json::from_slice(&request[body_start.unwrap()..expected_length.unwrap()]).unwrap()
}

async fn write_provider_stream(stream: &mut TcpStream, delta: Value, finish_reason: &str) {
    stream
        .write_all(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
        )
        .await
        .unwrap();
    let frame = json!({
        "choices": [{ "delta": delta, "finish_reason": null }]
    });
    let finish = json!({
        "choices": [{ "delta": {}, "finish_reason": finish_reason }]
    });
    stream
        .write_all(format!("data: {frame}\n\ndata: {finish}\n\ndata: [DONE]\n\n").as_bytes())
        .await
        .unwrap();
}

async fn collect_until_done(
    receiver: &mut tokio::sync::mpsc::UnboundedReceiver<Value>,
) -> Vec<Value> {
    tokio::time::timeout(Duration::from_secs(5), async {
        let mut events = Vec::new();
        loop {
            let event = receiver.recv().await.expect("Agent event channel closed");
            let done = event["params"]["type"] == "done";
            events.push(event);
            if done {
                return events;
            }
        }
    })
    .await
    .expect("timed out waiting for Agent terminal event")
}

fn provider_message_text(message: &Value) -> String {
    fn collect(value: &Value, output: &mut String) {
        match value {
            Value::String(value) => output.push_str(value),
            Value::Array(values) => values.iter().for_each(|value| collect(value, output)),
            Value::Object(object) => {
                if let Some(text) = object.get("text") {
                    collect(text, output);
                } else if let Some(content) = object.get("content") {
                    collect(content, output);
                }
            }
            Value::Null | Value::Bool(_) | Value::Number(_) => {}
        }
    }
    let mut output = String::new();
    collect(&message["content"], &mut output);
    output
}

fn save_provider_profile_fixture(
    storage: &StorageService,
    api_url: &str,
    profile: Option<mycopilot_core::ProviderProfileConfig>,
) {
    let mut settings = test_model_settings();
    settings.api_url = api_url.to_string();
    settings.api_token = "provider-profile-token".to_string();
    settings.models[0].provider_profile_config = profile.unwrap_or_else(|| {
        mycopilot_core::ProviderProfileConfig::generic_for_dialect(
            mycopilot_core::ProviderProtocolDialect::OpenAiChatCompletions,
        )
    });
    storage.save_model_settings(settings).unwrap();
}

fn turn_input(model_id: &str) -> AgentConversationTurnInput {
    AgentConversationTurnInput {
        conversation_id: None,
        project_id: None,
        model_id: model_id.to_string(),
        context_window_indicator_enabled: true,
        content: "Verify the frozen provider profile".to_string(),
        attachments: Vec::new(),
        skills: Vec::new(),
        title: None,
        user_message_id: Some("user-provider-profile".to_string()),
        assistant_message_id: Some("assistant-provider-profile".to_string()),
        max_tokens: None,
        temperature: None,
        prompt_preferences: None,
        permissions: AgentPermissions::default(),
    }
}

#[test]
fn renderer_turn_input_rejects_host_only_collaboration_fields() {
    let error = serde_json::from_value::<AgentConversationTurnInput>(json!({
        "conversationId": null,
        "projectId": null,
        "modelId": "model-1",
        "content": "attempt to forge child identity",
        "attachments": [],
        "skills": [],
        "title": null,
        "userMessageId": null,
        "assistantMessageId": null,
        "maxTokens": null,
        "temperature": null,
        "promptPreferences": null,
        "permissions": AgentPermissions::default(),
        "collaborationIdentity": { "agentId": "forged" }
    }))
    .unwrap_err();
    assert!(error.to_string().contains("unknown field"), "{error}");
}

#[test]
fn current_model_freezes_the_generic_profile_for_the_resolved_dialect() {
    let fixture = tempdir().unwrap();
    let storage = StorageService::open(&fixture.path().join("storage.sqlite")).unwrap();
    storage.save_model_settings(test_model_settings()).unwrap();
    let revision = storage
        .load_model_settings_snapshot()
        .unwrap()
        .unwrap()
        .provider_protocol_revisions["model-1"]
        .clone();

    let prepared = prepare_conversation_turn(
        &storage,
        &SkillsService::new(),
        turn_input("model-1"),
        "run-provider-profile-generic",
    )
    .unwrap();

    assert!(mycopilot_core::storage::config_repository::is_provider_protocol_revision(&revision));
    let config = prepared.agent_input.provider_profile_config.unwrap();
    let key = prepared.agent_input.provider_protocol_key.unwrap();
    assert_eq!(
        config,
        mycopilot_core::ProviderProfileConfig::generic_for_dialect(
            mycopilot_core::ProviderProtocolDialect::OpenAiChatCompletions,
        )
    );
    assert_eq!(
        key.dialect,
        mycopilot_core::ProviderProtocolDialect::OpenAiChatCompletions
    );
    assert_eq!(key.profile, config.profile);
    assert_eq!(key.model_id, "model-1");
    assert_eq!(
        key.provider_configuration_revision.as_deref(),
        Some(revision.as_str())
    );
}

#[test]
fn renderer_profile_selection_round_trips_into_the_frozen_registration() {
    let fixture = tempdir().unwrap();
    let storage = StorageService::open(&fixture.path().join("storage.sqlite")).unwrap();
    let request =
        serde_json::from_value::<mycopilot_core::storage::models::ModelSettingsSaveRequest>(
            json!({
                "apiUrl": "https://api.deepseek.com/v1/chat/completions",
                "apiToken": "provider-profile-token",
                "searchMode": "disabled",
                "tavilyApiKey": "",
                "models": [{
                    "id": "model-1",
                    "previousModelId": null,
                    "displayName": "DeepSeek V4 Chat",
                    "apiUrlOverride": null,
                    "apiTokenOverride": null,
                    "supportsImage": false,
                    "contextWindowTokens": 128000,
                    "providerProfileUpdate": {
                        "kind": "select_registered_profile",
                        "profileId": "deepseek_v4_chat",
                        "settings": {
                            "kind": "deepseek_v4_chat",
                            "reasoning": {"mode": "enabled", "effort": "high"}
                        }
                    },
                    "inputPrice": "0",
                    "cachedInputPrice": "",
                    "outputPrice": "0",
                    "enabled": true
                }]
            }),
        )
        .unwrap();
    let authoritative = storage.save_model_settings_request(request).unwrap();
    let stored = &authoritative.models[0].provider_profile_config;
    assert_eq!(
        stored.profile,
        mycopilot_core::ProviderProfileRef::deepseek_v4_chat()
    );
    assert_eq!(
        stored.reasoning.mode,
        mycopilot_core::ReasoningMode::Enabled
    );
    assert_eq!(
        stored.reasoning.effort,
        mycopilot_core::ReasoningEffort::High
    );

    let prepared = prepare_conversation_turn(
        &storage,
        &SkillsService::new(),
        turn_input("model-1"),
        "run-provider-profile-renderer-selection",
    )
    .unwrap();
    let frozen_config = prepared.agent_input.provider_profile_config.unwrap();
    let frozen_key = prepared.agent_input.provider_protocol_key.unwrap();
    assert_eq!(frozen_config, stored.clone());
    assert_eq!(frozen_key.profile, stored.profile);
    let registration = mycopilot_core::resolve_provider_registration_for_key(&frozen_key).unwrap();
    assert_eq!(registration.profile(), stored.profile);
    assert_eq!(
        registration.runtime_capabilities().tool_exchange(),
        mycopilot_core::ProviderToolExchangeSemantics::ExactProviderGrouped
    );
}

#[test]
fn renderer_generic_selection_freezes_the_anthropic_generic_registration() {
    let fixture = tempdir().unwrap();
    let storage = StorageService::open(&fixture.path().join("storage.sqlite")).unwrap();
    let request =
        serde_json::from_value::<mycopilot_core::storage::models::ModelSettingsSaveRequest>(
            json!({
                "apiUrl": "https://api.anthropic.com/v1/messages",
                "apiToken": "provider-profile-token",
                "searchMode": "disabled",
                "tavilyApiKey": "",
                "models": [{
                    "id": "model-1",
                    "previousModelId": null,
                    "displayName": "Anthropic-compatible",
                    "apiUrlOverride": null,
                    "apiTokenOverride": null,
                    "supportsImage": false,
                    "contextWindowTokens": 128000,
                    "providerProfileUpdate": {"kind": "select_generic"},
                    "inputPrice": "0",
                    "cachedInputPrice": "",
                    "outputPrice": "0",
                    "enabled": true
                }]
            }),
        )
        .unwrap();
    storage.save_model_settings_request(request).unwrap();

    let prepared = prepare_conversation_turn(
        &storage,
        &SkillsService::new(),
        turn_input("model-1"),
        "run-provider-profile-anthropic-selection",
    )
    .unwrap();
    let frozen_config = prepared.agent_input.provider_profile_config.unwrap();
    let frozen_key = prepared.agent_input.provider_protocol_key.unwrap();
    assert_eq!(
        frozen_config.profile,
        mycopilot_core::ProviderProfileRef::generic_for_dialect(
            mycopilot_core::ProviderProtocolDialect::AnthropicMessages
        )
    );
    assert_eq!(
        mycopilot_core::resolve_provider_registration_for_key(&frozen_key)
            .unwrap()
            .profile(),
        frozen_config.profile
    );
}

#[test]
fn prepared_runs_bind_to_only_the_selected_models_effective_protocol_revision() {
    let fixture = tempdir().unwrap();
    let storage = StorageService::open(&fixture.path().join("storage.sqlite")).unwrap();
    storage.save_model_settings(test_model_settings()).unwrap();

    let prepare = |suffix: &str| {
        let mut input = turn_input("model-1");
        input.user_message_id = Some(format!("user-provider-revision-{suffix}"));
        input.assistant_message_id = Some(format!("assistant-provider-revision-{suffix}"));
        prepare_conversation_turn(
            &storage,
            &SkillsService::new(),
            input,
            &format!("run-provider-revision-{suffix}"),
        )
        .unwrap()
        .agent_input
    };

    let initial_snapshot = storage.load_model_settings_snapshot().unwrap().unwrap();
    let initial_global_revision = initial_snapshot.configuration_revision;
    let initial = prepare("initial");
    let initial_revision = initial.provider_configuration_revision.unwrap();
    assert_ne!(
        initial.provider_connection_revision.as_deref(),
        Some(initial_revision.as_str()),
        "protocol and endpoint/token revisions have independent lifecycle semantics"
    );

    let mut metadata_only = storage.load_model_settings().unwrap().unwrap();
    metadata_only.models[0].display_name = "Renamed model".to_string();
    metadata_only.models[0].context_window_tokens = Some(256_000);
    metadata_only.models[0].input_price = "1.5".to_string();
    metadata_only.models[0].output_price = "2.5".to_string();
    metadata_only.search_mode = "tavily".to_string();
    metadata_only.tavily_api_key = "search-only-token".to_string();
    metadata_only
        .models
        .push(mycopilot_core::storage::models::ModelConfigRecord {
            id: "model-2".to_string(),
            display_name: "Other model".to_string(),
            api_url_override: Some("https://other.example/v1".to_string()),
            api_token_override: Some("other-token".to_string()),
            supports_image: false,
            context_window_tokens: Some(64_000),
            provider_profile_config: mycopilot_core::ProviderProfileConfig::generic_for_dialect(
                mycopilot_core::ProviderProtocolDialect::OpenAiChatCompletions,
            ),
            input_price: "0".to_string(),
            cached_input_price: String::new(),
            output_price: "0".to_string(),
            enabled: true,
        });
    storage.save_model_settings(metadata_only).unwrap();
    let metadata_snapshot = storage.load_model_settings_snapshot().unwrap().unwrap();
    assert_ne!(
        metadata_snapshot.configuration_revision,
        initial_global_revision
    );
    let after_metadata = prepare("metadata");
    assert_eq!(
        after_metadata.provider_configuration_revision.as_deref(),
        Some(initial_revision.as_str()),
        "unrelated settings saves must not invalidate the selected model's protocol state"
    );

    let mut wire_change = storage.load_model_settings().unwrap().unwrap();
    wire_change.models[0].provider_profile_config =
        mycopilot_core::ProviderProfileConfig::deepseek_v4_default();
    storage.save_model_settings(wire_change).unwrap();
    let after_wire_change = prepare("wire-change");
    assert_ne!(
        after_wire_change.provider_configuration_revision.as_deref(),
        Some(initial_revision.as_str()),
        "a Provider Profile change must rotate the selected model's protocol state"
    );
    assert_eq!(
        after_wire_change.provider_connection_revision, initial.provider_connection_revision,
        "a Profile-only edit must not rotate the endpoint/token identity"
    );
}

#[test]
fn explicit_profile_incompatible_with_the_endpoint_dialect_fails_closed() {
    let fixture = tempdir().unwrap();
    let storage = StorageService::open(&fixture.path().join("storage.sqlite")).unwrap();
    let mut settings = test_model_settings();
    settings.api_url = "https://api.anthropic.com/v1/messages".to_string();
    settings.models[0].provider_profile_config =
        mycopilot_core::ProviderProfileConfig::deepseek_v4_default();
    storage.save_model_settings(settings).unwrap();

    let error = match prepare_conversation_turn(
        &storage,
        &SkillsService::new(),
        turn_input("model-1"),
        "run-provider-profile-incompatible",
    ) {
        Ok(_) => panic!("an incompatible explicit provider profile must fail closed"),
        Err(error) => error,
    };

    let rendered = error.to_string();
    assert!(rendered.contains("Provider Profile"));
    assert!(rendered.contains("incompatible"));
}

#[test]
fn generic_agent_service_startup_does_not_require_provider_continuation_credentials() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());

    let service =
        AgentService::try_new_with_startup_reconciliation(Arc::clone(&storage), false, None)
            .expect("an unavailable optional Provider vault must not block Generic startup");

    assert!(service.provider_continuation_vault.is_none());
    assert!(service.list_pending_actions().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ordinary_root_turn_is_durable_before_its_terminal_event() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let model_server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let request = read_provider_request(&mut stream).await;
        write_provider_stream(
            &mut stream,
            json!({ "role": "assistant", "content": "Persisted root answer." }),
            "stop",
        )
        .await;
        request
    });

    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    save_provider_profile_fixture(
        &storage,
        &format!("http://{address}/v1/chat/completions"),
        None,
    );
    let service =
        AgentService::try_new_with_startup_reconciliation(Arc::clone(&storage), false, None)
            .unwrap();
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let mut input = turn_input("model-1");
    input.conversation_id = Some("conversation-root-characterization".to_string());
    input.user_message_id = Some("user-root-characterization".to_string());
    input.assistant_message_id = Some("assistant-root-characterization".to_string());
    input.title = Some("Root turn characterization".to_string());

    let turn = service
        .start_conversation_turn(input, notifications)
        .unwrap();
    assert_eq!(turn.event_name, AGENT_EVENT_NAME);
    assert_eq!(turn.conversation_id, "conversation-root-characterization");
    assert_eq!(turn.user_message_id, "user-root-characterization");
    assert_eq!(turn.assistant_message_id, "assistant-root-characterization");

    let events = collect_until_done(&mut receiver).await;
    let done = events.last().unwrap();
    assert_eq!(done["params"]["runId"], turn.run_id);
    assert_eq!(done["params"]["status"], "completed", "{events:?}");

    // `turn.rs` gates terminal publication on the atomic message/trace commit. A successful read
    // immediately after Done therefore characterizes the public durability boundary relied on by
    // future root and child Turn orchestration.
    let conversation = storage
        .load_conversation(&turn.conversation_id)
        .unwrap()
        .unwrap();
    assert_eq!(conversation.model_id.as_deref(), Some("model-1"));
    assert_eq!(conversation.title, "Root turn characterization");
    assert_eq!(conversation.messages.len(), 2);
    let user = &conversation.messages[0];
    assert_eq!(user.id, turn.user_message_id);
    assert_eq!(user.role, "user");
    assert_eq!(user.content, "Verify the frozen provider profile");
    assert_eq!(user.status.as_deref(), Some("sent"));
    let assistant = &conversation.messages[1];
    assert_eq!(assistant.id, turn.assistant_message_id);
    assert_eq!(assistant.role, "assistant");
    assert_eq!(assistant.content, "Persisted root answer.");
    assert_eq!(assistant.status.as_deref(), Some("sent"));

    let trace = storage
        .get_conversation_turn_trace(&turn.assistant_message_id)
        .unwrap()
        .expect("completed root Turn has a durable terminal trace");
    assert_eq!(trace.run_id, turn.run_id);
    assert_eq!(trace.conversation_id, turn.conversation_id);
    assert_eq!(trace.assistant_message_id, turn.assistant_message_id);
    assert_eq!(
        trace.terminal_status,
        ConversationTurnTraceTerminalStatus::Completed
    );

    let provider_request = model_server.await.unwrap();
    let provider_messages = provider_request["messages"].as_array().unwrap();
    assert!(provider_messages.iter().any(|message| {
        message["role"] == "user"
            && message["content"]
                .as_str()
                .is_some_and(|content| content.contains("Verify the frozen provider profile"))
    }));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn trusted_child_wake_uses_the_root_loop_without_duplicating_the_parent_task() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let model_server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let request = read_provider_request(&mut stream).await;
        write_provider_stream(
            &mut stream,
            json!({ "role": "assistant", "content": "Child evidence." }),
            "stop",
        )
        .await;
        request
    });

    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    save_provider_profile_fixture(
        &storage,
        &format!("http://{address}/v1/chat/completions"),
        Some(mycopilot_core::ProviderProfileConfig {
            schema_version: mycopilot_core::PROVIDER_PROFILE_CONFIG_SCHEMA_VERSION,
            profile: mycopilot_core::ProviderProfileRef::deepseek_v4_chat(),
            reasoning: mycopilot_core::ReasoningPolicy {
                mode: mycopilot_core::ReasoningMode::Enabled,
                effort: mycopilot_core::ReasoningEffort::High,
            },
        }),
    );
    storage
        .save_conversation(ChatConversationRecord {
            id: "conversation-root-for-child".to_string(),
            project_id: None,
            model_id: Some("model-1".to_string()),
            title: "Root for child".to_string(),
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
            agent_id: "agent-root-for-child".to_string(),
            conversation_id: "conversation-root-for-child".to_string(),
            creation_request_id: "ensure-root-for-child".to_string(),
            task_name: "Root".to_string(),
        })
        .unwrap();
    let spawn = storage
        .create_child_agent(&mycopilot_core::CreateChildAgentInput {
            parent_agent_id: "agent-root-for-child".to_string(),
            creation_request_id: "spawn-child-controlled-turn".to_string(),
            task_name: "security_review".to_string(),
            task: "Inspect authentication and report evidence.".to_string(),
            template_machine_key: None,
            explicit_model_id: None,
            reasoning_effort: Some(mycopilot_core::ReasoningEffort::High),
            fork_turns: mycopilot_core::AgentForkTurns::None,
        })
        .unwrap();
    // The node snapshot freezes selection/audit facts, not the entire global Settings revision.
    // An unrelated model edit must not strand an existing child forever.
    let mut settings = storage.load_model_settings().unwrap().unwrap();
    let mut unrelated = settings.models[0].clone();
    unrelated.id = "unrelated-model".to_string();
    unrelated.display_name = "Unrelated model".to_string();
    settings.models.push(unrelated);
    storage.save_model_settings(settings).unwrap();
    let claim_token = "controlled-child-claim";
    let claimed = storage
        .claim_next_agent_wake(&spawn.agent.agent_id, claim_token)
        .unwrap()
        .unwrap();
    assert_eq!(claimed.wake_id, spawn.initial_wake.wake_id);
    storage
        .transition_agent_wake(
            &claimed.wake_id,
            mycopilot_core::AgentWakeStatus::Claimed,
            mycopilot_core::AgentWakeStatus::Running,
            Some(claim_token),
        )
        .unwrap();

    let service =
        AgentService::try_new_with_startup_reconciliation(Arc::clone(&storage), false, None)
            .unwrap();
    let trusted = TrustedAgentWakeTurnStart::new(
        spawn.initial_wake.wake_id.clone(),
        spawn.agent.agent_id.clone(),
        spawn.agent.conversation_id.clone(),
        spawn.task_message.message_id.clone(),
        claim_token.to_string(),
        spawn.collaboration_identity.clone(),
    )
    .unwrap();
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let turn = service
        .execute_turn(AgentTurnStart::AgentWake(trusted), notifications)
        .unwrap();
    assert_eq!(turn.conversation_id, spawn.agent.conversation_id);
    assert_eq!(
        turn.user_message_id,
        spawn.task_message.projection_message_id
    );

    let events = collect_until_done(&mut receiver).await;
    assert_eq!(events.last().unwrap()["params"]["status"], "completed");
    let conversation = storage
        .load_conversation(&spawn.agent.conversation_id)
        .unwrap()
        .unwrap();
    assert_eq!(conversation.messages.len(), 2);
    assert_eq!(
        conversation.messages[0].id,
        spawn.task_message.projection_message_id
    );
    assert_eq!(conversation.messages[0].role, "user");
    assert_eq!(conversation.messages[1].id, turn.assistant_message_id);
    assert_eq!(conversation.messages[1].content, "Child evidence.");
    let trace = storage
        .get_conversation_turn_trace(&turn.assistant_message_id)
        .unwrap()
        .unwrap();
    assert_eq!(trace.run_id, turn.run_id);
    assert_eq!(trace.conversation_id, spawn.agent.conversation_id);
    assert_eq!(
        trace.terminal_status,
        ConversationTurnTraceTerminalStatus::Completed
    );
    let usage = storage
        .load_agent_usage_for_owner(
            &turn.run_id,
            &spawn.agent.conversation_id,
            &turn.assistant_message_id,
        )
        .unwrap()
        .expect("child Turn Usage is owned by its independent Conversation");
    assert_eq!(usage.conversation_id, spawn.agent.conversation_id);
    assert!(storage
        .load_agent_usage_for_owner(
            &turn.run_id,
            "conversation-root-for-child",
            &turn.assistant_message_id,
        )
        .unwrap()
        .is_none());
    assert_eq!(
        storage
            .conversation_message_origin(
                &spawn.agent.conversation_id,
                &spawn.task_message.projection_message_id,
            )
            .unwrap(),
        mycopilot_core::ConversationMessageOrigin::Agent {
            sender_agent_id: "agent-root-for-child".to_string(),
            source_agent_message_id: spawn.task_message.message_id.clone(),
        }
    );

    let provider_request = model_server.await.unwrap();
    assert_eq!(provider_request["reasoning_effort"], "high");
    let provider_messages = provider_request["messages"].as_array().unwrap();
    assert_eq!(
        provider_messages
            .iter()
            .filter(|message| {
                message["role"] == "user"
                    && provider_message_text(message)
                        .contains("Inspect authentication and report evidence.")
            })
            .count(),
        1
    );
    assert!(provider_messages.iter().any(|message| {
        message["role"] == "system"
            && provider_message_text(message).contains("security_review")
            && provider_message_text(message).contains("agent-root-for-child")
    }));
}

#[test]
fn trusted_child_wake_fails_closed_when_its_selected_model_is_disabled() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    storage.save_model_settings(test_model_settings()).unwrap();
    storage
        .save_conversation(ChatConversationRecord {
            id: "conversation-root-disabled-child-model".to_string(),
            project_id: None,
            model_id: Some("model-1".to_string()),
            title: "Root".to_string(),
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
            agent_id: "agent-root-disabled-child-model".to_string(),
            conversation_id: "conversation-root-disabled-child-model".to_string(),
            creation_request_id: "ensure-root-disabled-child-model".to_string(),
            task_name: "Root".to_string(),
        })
        .unwrap();
    let spawn = storage
        .create_child_agent(&mycopilot_core::CreateChildAgentInput {
            parent_agent_id: "agent-root-disabled-child-model".to_string(),
            creation_request_id: "spawn-disabled-child-model".to_string(),
            task_name: "disabled_model".to_string(),
            task: "Must not silently switch models.".to_string(),
            template_machine_key: None,
            explicit_model_id: None,
            reasoning_effort: None,
            fork_turns: mycopilot_core::AgentForkTurns::None,
        })
        .unwrap();
    let mut settings = storage.load_model_settings().unwrap().unwrap();
    settings.models[0].enabled = false;
    storage.save_model_settings(settings).unwrap();
    let claim_token = "disabled-child-model-claim";
    storage
        .claim_next_agent_wake(&spawn.agent.agent_id, claim_token)
        .unwrap()
        .unwrap();
    storage
        .transition_agent_wake(
            &spawn.initial_wake.wake_id,
            mycopilot_core::AgentWakeStatus::Claimed,
            mycopilot_core::AgentWakeStatus::Running,
            Some(claim_token),
        )
        .unwrap();
    let trusted = TrustedAgentWakeTurnStart::new(
        spawn.initial_wake.wake_id.clone(),
        spawn.agent.agent_id.clone(),
        spawn.agent.conversation_id.clone(),
        spawn.task_message.message_id.clone(),
        claim_token.to_string(),
        spawn.collaboration_identity,
    )
    .unwrap();
    let service = AgentService::new(Arc::clone(&storage));
    let (notifications, _receiver) = tokio::sync::mpsc::unbounded_channel();
    let error = service
        .execute_turn(AgentTurnStart::AgentWake(trusted), notifications)
        .unwrap_err();
    assert!(error.to_string().contains("模型未启用"), "{error}");
    let conversation = storage
        .load_conversation(&spawn.agent.conversation_id)
        .unwrap()
        .unwrap();
    assert_eq!(conversation.messages.len(), 1);
    assert_eq!(
        conversation.messages[0].id,
        spawn.task_message.projection_message_id
    );
    assert!(storage
        .list_conversation_turn_traces(&spawn.agent.conversation_id)
        .unwrap()
        .is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn independent_hosts_admit_only_one_turn_without_loser_message_side_effects() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let model_server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let request = read_provider_request(&mut stream).await;
        write_provider_stream(
            &mut stream,
            json!({ "role": "assistant", "content": "Winning answer." }),
            "stop",
        )
        .await;
        request
    });
    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("storage.sqlite");
    let storage_a = Arc::new(StorageService::open(&database_path).unwrap());
    save_provider_profile_fixture(
        &storage_a,
        &format!("http://{address}/v1/chat/completions"),
        None,
    );
    storage_a
        .save_conversation(ChatConversationRecord {
            id: "conversation-cross-host-admission".to_string(),
            project_id: None,
            model_id: Some("model-1".to_string()),
            title: "Cross Host admission".to_string(),
            messages: Vec::new(),
            created_at: 1,
            updated_at: 1,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
    let storage_b = Arc::new(StorageService::open(&database_path).unwrap());
    let service_a = AgentService::new(Arc::clone(&storage_a));
    let service_b = AgentService::new(Arc::clone(&storage_b));
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let (notifications_a, mut receiver_a) = tokio::sync::mpsc::unbounded_channel();
    let (notifications_b, mut receiver_b) = tokio::sync::mpsc::unbounded_channel();
    let input = |suffix: &str| {
        let mut input = turn_input("model-1");
        input.conversation_id = Some("conversation-cross-host-admission".to_string());
        input.user_message_id = Some(format!("user-cross-host-{suffix}"));
        input.assistant_message_id = Some(format!("assistant-cross-host-{suffix}"));
        input.content = format!("cross Host request {suffix}");
        input
    };
    let barrier_a = Arc::clone(&barrier);
    let start_a = tokio::task::spawn_blocking(move || {
        barrier_a.wait();
        service_a.start_conversation_turn(input("a"), notifications_a)
    });
    let barrier_b = Arc::clone(&barrier);
    let start_b = tokio::task::spawn_blocking(move || {
        barrier_b.wait();
        service_b.start_conversation_turn(input("b"), notifications_b)
    });
    let (result_a, result_b) = tokio::join!(start_a, start_b);
    let result_a = result_a.unwrap();
    let result_b = result_b.unwrap();
    assert_ne!(
        result_a.is_ok(),
        result_b.is_ok(),
        "a={result_a:?} b={result_b:?}"
    );
    let (winner, loser_suffix, winner_events) = match (result_a, result_b) {
        (Ok(winner), Err(_)) => (winner, "b", &mut receiver_a),
        (Err(_), Ok(winner)) => (winner, "a", &mut receiver_b),
        _ => unreachable!(),
    };

    let during = storage_a
        .load_conversation("conversation-cross-host-admission")
        .unwrap()
        .unwrap();
    assert_eq!(during.messages.len(), 2);
    assert!(during
        .messages
        .iter()
        .all(|message| !message.id.ends_with(loser_suffix)));
    let traces = storage_a
        .list_conversation_turn_traces("conversation-cross-host-admission")
        .unwrap();
    assert_eq!(traces.len(), 1);
    assert_eq!(traces[0].run_id, winner.run_id);
    assert_eq!(
        traces[0].terminal_status,
        ConversationTurnTraceTerminalStatus::InProgress
    );

    let events = collect_until_done(winner_events).await;
    assert_eq!(events.last().unwrap()["params"]["status"], "completed");
    model_server.await.unwrap();
}

#[test]
fn public_human_turn_rejects_a_child_conversation_before_any_turn_write() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    storage.save_model_settings(test_model_settings()).unwrap();
    storage
        .save_conversation(ChatConversationRecord {
            id: "conversation-root-readonly-child".to_string(),
            project_id: None,
            model_id: Some("model-1".to_string()),
            title: "Root".to_string(),
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
            agent_id: "agent-root-readonly-child".to_string(),
            conversation_id: "conversation-root-readonly-child".to_string(),
            creation_request_id: "ensure-root-readonly-child".to_string(),
            task_name: "Root".to_string(),
        })
        .unwrap();
    let spawn = storage
        .create_child_agent(&mycopilot_core::CreateChildAgentInput {
            parent_agent_id: "agent-root-readonly-child".to_string(),
            creation_request_id: "spawn-readonly-child".to_string(),
            task_name: "readonly".to_string(),
            task: "Inspect only.".to_string(),
            template_machine_key: None,
            explicit_model_id: None,
            reasoning_effort: None,
            fork_turns: mycopilot_core::AgentForkTurns::None,
        })
        .unwrap();
    let before = storage
        .load_conversation(&spawn.agent.conversation_id)
        .unwrap()
        .unwrap();
    let service = AgentService::new(Arc::clone(&storage));
    let (notifications, _receiver) = tokio::sync::mpsc::unbounded_channel();
    let mut input = turn_input("model-1");
    input.conversation_id = Some(spawn.agent.conversation_id.clone());
    input.user_message_id = Some("forged-human-child-user".to_string());
    input.assistant_message_id = Some("forged-human-child-assistant".to_string());
    let error = service
        .start_conversation_turn(input, notifications)
        .unwrap_err();
    assert!(error.to_string().contains("子 Agent Conversation"));
    let after = storage
        .load_conversation(&spawn.agent.conversation_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::to_value(after).unwrap(),
        serde_json::to_value(before).unwrap()
    );
    assert!(storage
        .list_conversation_turn_traces(&spawn.agent.conversation_id)
        .unwrap()
        .is_empty());
    assert!(storage
        .load_agent_usage_for_owner(
            "unused-run",
            &spawn.agent.conversation_id,
            "forged-human-child-assistant",
        )
        .unwrap()
        .is_none());
}

#[test]
fn public_human_turn_rejects_an_inactive_root_before_any_turn_write() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    storage.save_model_settings(test_model_settings()).unwrap();
    storage
        .save_conversation(ChatConversationRecord {
            id: "conversation-inactive-root".to_string(),
            project_id: None,
            model_id: Some("model-1".to_string()),
            title: "Inactive root".to_string(),
            messages: Vec::new(),
            created_at: 1,
            updated_at: 1,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
    let root = storage
        .ensure_root_agent(&mycopilot_core::EnsureRootAgentInput {
            agent_id: "agent-inactive-root".to_string(),
            conversation_id: "conversation-inactive-root".to_string(),
            creation_request_id: "ensure-inactive-root".to_string(),
            task_name: "Root".to_string(),
        })
        .unwrap()
        .record()
        .clone();
    storage
        .transition_agent_lifecycle(
            &root.agent_id,
            root.revision,
            mycopilot_core::AgentLifecycle::Active,
            mycopilot_core::AgentLifecycle::Disabled,
        )
        .unwrap();

    let before = storage
        .load_conversation("conversation-inactive-root")
        .unwrap()
        .unwrap();
    let service = AgentService::new(Arc::clone(&storage));
    let (notifications, _receiver) = tokio::sync::mpsc::unbounded_channel();
    let mut input = turn_input("model-1");
    input.conversation_id = Some("conversation-inactive-root".to_string());
    input.user_message_id = Some("forged-inactive-root-user".to_string());
    input.assistant_message_id = Some("forged-inactive-root-assistant".to_string());
    let error = service
        .start_conversation_turn(input, notifications)
        .unwrap_err();
    assert!(error.to_string().contains("根 Agent 当前不可用"));
    assert_eq!(
        serde_json::to_value(
            storage
                .load_conversation("conversation-inactive-root")
                .unwrap()
                .unwrap()
        )
        .unwrap(),
        serde_json::to_value(before).unwrap()
    );
    assert!(storage
        .list_conversation_turn_traces("conversation-inactive-root")
        .unwrap()
        .is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unavailable_provider_vault_keeps_generic_and_deepseek_text_only_runs_available() {
    for (scenario, profile) in [
        ("generic", None),
        (
            "deepseek",
            Some({
                let mut profile = mycopilot_core::ProviderProfileConfig::deepseek_v4_default();
                profile.reasoning.mode = mycopilot_core::ReasoningMode::Disabled;
                profile
            }),
        ),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let model_server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = read_provider_request(&mut stream).await;
            write_provider_stream(
                &mut stream,
                json!({ "role": "assistant", "content": "Text-only response." }),
                "stop",
            )
            .await;
            request
        });
        let fixture = tempdir().unwrap();
        let storage = Arc::new(
            StorageService::open(&fixture.path().join(format!("{scenario}.sqlite"))).unwrap(),
        );
        save_provider_profile_fixture(
            &storage,
            &format!("http://{address}/v1/chat/completions"),
            profile,
        );
        let service =
            AgentService::try_new_with_startup_reconciliation(Arc::clone(&storage), false, None)
                .unwrap();
        let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        service
            .start_conversation_turn(turn_input("model-1"), notifications)
            .unwrap();
        let events = collect_until_done(&mut receiver).await;
        let done = events.last().unwrap();
        assert_eq!(done["params"]["status"], "completed", "{events:?}");
        assert!(events
            .iter()
            .all(|event| event["params"]["type"] != "error"));
        assert!(service.list_pending_actions().is_empty());
        assert_eq!(model_server.await.unwrap()["model"], "model-1");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unavailable_provider_vault_blocks_deepseek_tool_turn_before_tool_or_approval_side_effects()
{
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let model_server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let request = read_provider_request(&mut stream).await;
        write_provider_stream(
            &mut stream,
            json!({
                "role": "assistant",
                "content": "",
                "tool_calls": [{
                    "index": 0,
                    "id": "provider-vault-unavailable-call",
                    "type": "function",
                    "function": {
                        "name": "write_file",
                        "arguments": serde_json::to_string(&json!({
                            "phase": "begin",
                            "filePath": "must-not-create.md",
                            "mode": "create"
                        })).unwrap()
                    }
                }]
            }),
            "tool_calls",
        )
        .await;
        request
    });
    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("deepseek-no-provider-vault.sqlite");
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    let mut profile = mycopilot_core::ProviderProfileConfig::deepseek_v4_default();
    profile.reasoning.mode = mycopilot_core::ReasoningMode::Disabled;
    save_provider_profile_fixture(
        &storage,
        &format!("http://{address}/v1/chat/completions"),
        Some(profile),
    );
    let service =
        AgentService::try_new_with_startup_reconciliation(Arc::clone(&storage), false, None)
            .unwrap();
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let turn = service
        .start_conversation_turn(turn_input("model-1"), notifications)
        .unwrap();
    let events = collect_until_done(&mut receiver).await;
    assert!(events.iter().any(|event| {
        event["params"]["type"] == "error"
            && event["params"]["code"] == "provider_continuation_unavailable"
    }));
    assert!(events
        .iter()
        .all(|event| event["params"]["type"] != "approval_required"));
    assert_eq!(events.last().unwrap()["params"]["status"], "failed");
    assert!(service.list_pending_actions().is_empty());
    assert!(storage
        .list_agent_file_drafts_for_run(&turn.run_id)
        .unwrap()
        .is_empty());
    let provider_rows: i64 = rusqlite::Connection::open(&database_path)
        .unwrap()
        .query_row("SELECT COUNT(*) FROM provider_continuations", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(provider_rows, 0);
    assert_eq!(model_server.await.unwrap()["model"], "model-1");
}
