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

async fn wait_for_terminal_agent_wake(
    storage: &StorageService,
    wake_id: &str,
) -> mycopilot_core::AgentWakeRequestRecord {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let wake = storage
                .get_agent_wake(wake_id)
                .unwrap()
                .expect("Wake must remain durable while Dispatcher observes it");
            if matches!(
                wake.status,
                mycopilot_core::AgentWakeStatus::Completed
                    | mycopilot_core::AgentWakeStatus::Failed
                    | mycopilot_core::AgentWakeStatus::Interrupted
                    | mycopilot_core::AgentWakeStatus::OutcomeUnknown
                    | mycopilot_core::AgentWakeStatus::Cancelled
            ) {
                return wake;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("timed out waiting for Dispatcher Wake settlement")
}

struct AdvancingDispatcherClock(std::sync::atomic::AtomicI64);

impl AdvancingDispatcherClock {
    fn new(initial: i64) -> Self {
        Self(std::sync::atomic::AtomicI64::new(initial))
    }

    fn reset(&self, value: i64) {
        self.0.store(value, std::sync::atomic::Ordering::SeqCst);
    }
}

impl crate::application::agent_dispatcher::AgentDispatcherClock for AdvancingDispatcherClock {
    fn now_ms(&self) -> i64 {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    }
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

fn provider_request_message_text(request: &Value) -> String {
    request["messages"]
        .as_array()
        .into_iter()
        .flatten()
        .map(provider_message_text)
        .collect::<Vec<_>>()
        .join("\n")
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

fn fork_transition_summary_generator() -> ContextCompactionSummaryGenerator {
    Arc::new(|request, cancellation| {
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
                model: "model-2".to_string(),
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
                    content: "Durable provider transition summary.".to_string(),
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
    assert_eq!(key.profile, config.profile());
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
        stored.profile(),
        mycopilot_core::ProviderProfileRef::deepseek_v4_chat()
    );
    assert_eq!(
        stored.reasoning_mode(),
        mycopilot_core::ReasoningMode::Enabled
    );
    assert_eq!(
        stored.provider_reasoning_effort(),
        mycopilot_core::ProviderReasoningEffort::High
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
    assert_eq!(frozen_key.profile, stored.profile());
    let registration = mycopilot_core::resolve_provider_registration_for_key(&frozen_key).unwrap();
    assert_eq!(registration.profile(), stored.profile());
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
        frozen_config.profile(),
        mycopilot_core::ProviderProfileRef::generic_for_dialect(
            mycopilot_core::ProviderProtocolDialect::AnthropicMessages
        )
    );
    assert_eq!(
        mycopilot_core::resolve_provider_registration_for_key(&frozen_key)
            .unwrap()
            .profile(),
        frozen_config.profile()
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
async fn rewrite_turn_is_atomic_replayable_and_runs_with_only_the_active_context() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let model_server = tokio::spawn(async move {
        let mut requests = Vec::new();
        for answer in ["Original answer.", "Replacement answer."] {
            let (mut stream, _) = listener.accept().await.unwrap();
            requests.push(read_provider_request(&mut stream).await);
            write_provider_stream(
                &mut stream,
                json!({ "role": "assistant", "content": answer }),
                "stop",
            )
            .await;
        }
        requests
    });

    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    storage
        .save_project(ProjectRecord {
            id: "rewrite-project".to_string(),
            name: "Rewrite project".to_string(),
            path: Some(fixture.path().to_string_lossy().into_owned()),
            created_at: 1,
            pinned_at: None,
        })
        .unwrap();
    save_provider_profile_fixture(
        &storage,
        &format!("http://{address}/v1/chat/completions"),
        None,
    );
    let service =
        AgentService::try_new_with_startup_reconciliation(Arc::clone(&storage), false, None)
            .unwrap();
    let source_attachment = mycopilot_core::AgentInputAttachment {
        id: "rewrite-source-attachment".to_string(),
        kind: mycopilot_core::AgentInputAttachmentKind::File,
        name: "evidence.txt".to_string(),
        mime_type: Some("text/plain".to_string()),
        size_bytes: 16,
        encoding: mycopilot_core::AgentInputAttachmentEncoding::Utf8,
        data: "durable evidence".to_string(),
        truncated: None,
    };
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let mut initial = turn_input("model-1");
    initial.conversation_id = Some("conversation-rewrite-provider".to_string());
    initial.project_id = Some("rewrite-project".to_string());
    initial.content = "Original prompt must disappear".to_string();
    initial.user_message_id = Some("rewrite-source-user".to_string());
    initial.assistant_message_id = Some("rewrite-source-assistant".to_string());
    initial.attachments = vec![source_attachment.clone()];
    initial.permissions.write = AgentWritePermission::WorkspaceOnly;
    let source = service
        .start_conversation_turn(initial, notifications)
        .unwrap();
    assert_eq!(
        collect_until_done(&mut receiver).await.last().unwrap()["params"]["status"],
        "completed"
    );

    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let replacement_content = "Replacement prompt is authoritative";
    let mut replacement = turn_input("model-1");
    replacement.conversation_id = Some(source.conversation_id.clone());
    replacement.project_id = Some("rewrite-project".to_string());
    replacement.content = replacement_content.to_string();
    replacement.user_message_id = Some("rewrite-replacement-user".to_string());
    replacement.assistant_message_id = Some("rewrite-replacement-assistant".to_string());
    replacement.attachments = vec![source_attachment.clone()];
    replacement.title = Some(create_conversation_title(replacement_content));
    replacement.permissions = AgentPermissions::default();
    let rewrite = AgentConversationTurnRewriteInput {
        request_id: "rewrite-provider-request".to_string(),
        source_user_message_id: source.user_message_id.clone(),
        source_assistant_message_id: source.assistant_message_id.clone(),
        turn: replacement,
    };
    let replacement = service
        .rewrite_conversation_turn(rewrite.clone(), notifications)
        .unwrap();
    assert_eq!(
        collect_until_done(&mut receiver).await.last().unwrap()["params"]["status"],
        "completed"
    );

    let active = storage
        .load_conversation(&source.conversation_id)
        .unwrap()
        .unwrap();
    assert_eq!(active.title, create_conversation_title(replacement_content));
    assert_eq!(
        active
            .messages
            .iter()
            .map(|message| message.id.as_str())
            .collect::<Vec<_>>(),
        vec!["rewrite-replacement-user", "rewrite-replacement-assistant"]
    );
    assert_eq!(active.messages[1].content, "Replacement answer.");
    assert!(storage
        .search_chats(&mycopilot_core::storage::models::ChatSearchInput {
            query: "Original prompt must disappear".to_string(),
            limit: Some(10),
        })
        .unwrap()
        .is_empty());
    assert_eq!(
        storage
            .search_chats(&mycopilot_core::storage::models::ChatSearchInput {
                query: "Replacement prompt".to_string(),
                limit: Some(10),
            })
            .unwrap()
            .len(),
        1
    );
    let active_world_state = storage
        .list_active_conversation_world_state_records(&source.conversation_id)
        .unwrap();
    assert!(active_world_state.iter().any(|entry| {
        entry.effective_before_message_id.as_deref() == Some("rewrite-replacement-user")
    }));

    let replacement_attachment_id = replacement.user_message.attachments[0].id.clone();
    assert_ne!(replacement_attachment_id, source_attachment.id);
    let loaded = storage
        .load_input_attachments(&[source_attachment.id.clone(), replacement_attachment_id])
        .unwrap();
    assert_eq!(loaded.len(), 2);
    assert!(loaded
        .iter()
        .all(|attachment| attachment.data == "ZHVyYWJsZSBldmlkZW5jZQ=="));
    let active_library = storage
        .build_attachment_library_context(&source.conversation_id, Some("rewrite-project"))
        .unwrap();
    assert_eq!(
        active_library
            .conversation_attachments
            .iter()
            .map(|attachment| attachment.id.as_str())
            .collect::<Vec<_>>(),
        vec![replacement.user_message.attachments[0].id.as_str()]
    );
    storage
        .save_conversation(ChatConversationRecord {
            id: "conversation-rewrite-project-peer".to_string(),
            project_id: Some("rewrite-project".to_string()),
            model_id: Some("model-1".to_string()),
            title: "Peer".to_string(),
            messages: vec![ChatMessageRecord {
                id: "rewrite-peer-user".to_string(),
                role: "user".to_string(),
                content: "peer".to_string(),
                created_at: 20,
                status: Some("sent".to_string()),
                attachments: Vec::new(),
                agent_run_json: None,
                ui_state_json: None,
            }],
            created_at: 20,
            updated_at: 20,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
    let peer_library = storage
        .build_attachment_library_context(
            "conversation-rewrite-project-peer",
            Some("rewrite-project"),
        )
        .unwrap();
    assert_eq!(
        peer_library
            .project_attachments
            .iter()
            .map(|attachment| attachment.id.as_str())
            .collect::<Vec<_>>(),
        vec![replacement.user_message.attachments[0].id.as_str()]
    );

    let replay = service
        .rewrite_conversation_turn(rewrite, tokio::sync::mpsc::unbounded_channel().0)
        .unwrap();
    assert_eq!(replay.run_id, replacement.run_id);
    assert_eq!(replay.assistant_message.status.as_deref(), Some("sent"));
    assert_eq!(replay.assistant_message.content, "Replacement answer.");

    let requests = model_server.await.unwrap();
    let replacement_request = provider_request_message_text(&requests[1]);
    assert!(replacement_request.contains(replacement_content));
    assert!(!replacement_request.contains("Original prompt must disappear"));
    assert!(!replacement_request.contains("Original answer."));
}

#[test]
fn rewrite_pre_runtime_failure_is_fail_closed_then_replays_the_failed_terminal() {
    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("storage.sqlite");
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    storage
        .save_project(ProjectRecord {
            id: "rewrite-failure-project".to_string(),
            name: "Rewrite failure project".to_string(),
            path: Some(fixture.path().to_string_lossy().into_owned()),
            created_at: 1,
            pinned_at: None,
        })
        .unwrap();
    storage
        .save_conversation(ChatConversationRecord {
            id: "conversation-rewrite-failure".to_string(),
            project_id: Some("rewrite-failure-project".to_string()),
            model_id: Some("model-1".to_string()),
            title: "source".to_string(),
            messages: vec![ChatMessageRecord {
                id: "rewrite-failure-source-user".to_string(),
                role: "user".to_string(),
                content: "source".to_string(),
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
    storage
        .ensure_root_agent(&mycopilot_core::EnsureRootAgentInput {
            agent_id: "rewrite-failure-root".to_string(),
            conversation_id: "conversation-rewrite-failure".to_string(),
            creation_request_id: "rewrite-failure-root-request".to_string(),
            task_name: "Rewrite failure".to_string(),
        })
        .unwrap();
    let (mut source, revision) = storage
        .load_conversation_for_turn("conversation-rewrite-failure")
        .unwrap();
    let mut source = source.take().unwrap();
    source.messages.push(ChatMessageRecord {
        id: "rewrite-failure-source-assistant".to_string(),
        role: "assistant".to_string(),
        content: "Thinking...".to_string(),
        created_at: 2,
        status: Some("pending".to_string()),
        attachments: Vec::new(),
        agent_run_json: None,
        ui_state_json: None,
    });
    let source_trace = mycopilot_core::ConversationTraceSnapshot::default().in_progress_trace(
        "rewrite-failure-source-run",
        "conversation-rewrite-failure",
        "rewrite-failure-source-assistant",
    );
    storage
        .save_conversation_and_begin_turn(
            source,
            revision,
            None,
            mycopilot_core::AgentTurnPermissionSource::HostAuthenticatedRoot(
                AgentPermissions::default(),
            ),
            &source_trace,
            2,
            2,
        )
        .unwrap();
    let source_terminal = mycopilot_core::completed_conversation_trace_without_items(
        "rewrite-failure-source-run",
        "conversation-rewrite-failure",
        "rewrite-failure-source-assistant",
    );
    storage
        .finalize_chat_message_with_conversation_trace(
            "conversation-rewrite-failure",
            "rewrite-failure-source-assistant",
            "source answer",
            Some("sent"),
            "completed",
            &source_terminal,
            2,
            3,
        )
        .unwrap();
    let service =
        AgentService::try_new_with_startup_reconciliation(Arc::clone(&storage), false, None)
            .unwrap();

    let (mut replacement, revision) = storage
        .load_conversation_for_turn("conversation-rewrite-failure")
        .unwrap();
    let mut replacement = replacement.take().unwrap();
    let replacement_user = ChatMessageRecord {
        id: "rewrite-failure-user".to_string(),
        role: "user".to_string(),
        content: "replacement".to_string(),
        created_at: 4,
        status: Some("sent".to_string()),
        attachments: Vec::new(),
        agent_run_json: None,
        ui_state_json: None,
    };
    let replacement_assistant = ChatMessageRecord {
        id: "rewrite-failure-assistant".to_string(),
        role: "assistant".to_string(),
        content: "Thinking...".to_string(),
        created_at: 5,
        status: Some("pending".to_string()),
        attachments: Vec::new(),
        agent_run_json: None,
        ui_state_json: None,
    };
    replacement
        .messages
        .extend([replacement_user.clone(), replacement_assistant.clone()]);
    replacement.updated_at = 5;
    let response = AgentConversationTurnOutput {
        run_id: "rewrite-failure-run".to_string(),
        event_name: AGENT_EVENT_NAME.to_string(),
        conversation_id: "conversation-rewrite-failure".to_string(),
        user_message_id: replacement_user.id.clone(),
        assistant_message_id: replacement_assistant.id.clone(),
        user_message: replacement_user,
        assistant_message: replacement_assistant,
        activated_skills: Vec::new(),
        skill_activation_revision: None,
    };
    let admission = mycopilot_core::storage::conversation_turn_rewrite_repository::ConversationTurnRewriteAdmission {
        request_id: "rewrite-failure-request".to_string(),
        request_fingerprint: format!("sha256:{}", "d".repeat(64)),
        conversation_id: "conversation-rewrite-failure".to_string(),
        source_user_message_id: "rewrite-failure-source-user".to_string(),
        source_assistant_message_id: "rewrite-failure-source-assistant".to_string(),
        replacement_user_message_id: "rewrite-failure-user".to_string(),
        replacement_assistant_message_id: "rewrite-failure-assistant".to_string(),
        run_id: "rewrite-failure-run".to_string(),
        response_json: serde_json::to_string(&response).unwrap(),
        created_at: 5,
    };
    let replacement_trace = mycopilot_core::ConversationTraceSnapshot::default().in_progress_trace(
        "rewrite-failure-run",
        "conversation-rewrite-failure",
        "rewrite-failure-assistant",
    );
    let prepared_attachments = storage
        .prepare_conversation_turn_rewrite_attachments(
            "conversation-rewrite-failure",
            "rewrite-failure-user",
            Some("rewrite-failure-project"),
            &[],
            4,
        )
        .unwrap();
    storage
        .rewrite_conversation_turn_and_begin_turn(
            replacement,
            revision,
            mycopilot_core::AgentTurnPermissionSource::HostAuthenticatedRoot(
                AgentPermissions::default(),
            ),
            &[],
            &replacement_trace,
            5,
            5,
            &admission,
            &prepared_attachments,
        )
        .unwrap();

    let fault = rusqlite::Connection::open(&database_path).unwrap();
    fault
        .execute_batch(
            "CREATE TRIGGER fail_rewrite_terminalization
             BEFORE UPDATE ON messages
             WHEN NEW.id = 'rewrite-failure-assistant'
             BEGIN
                 SELECT RAISE(ABORT, 'injected rewrite settlement failure');
             END;",
        )
        .unwrap();
    let error = service
        .settle_prepared_rewrite_failure(
            "conversation-rewrite-failure",
            "rewrite-failure-assistant",
            Some("rewrite-failure-run"),
            "rewrite-failure-request",
            "injected pre-runtime failure",
        )
        .unwrap_err();
    assert!(error.contains("injected rewrite settlement failure"));
    assert_eq!(
        storage
            .get_conversation_turn_trace("rewrite-failure-assistant")
            .unwrap()
            .unwrap()
            .terminal_status,
        ConversationTurnTraceTerminalStatus::InProgress
    );
    fault
        .execute_batch("DROP TRIGGER fail_rewrite_terminalization;")
        .unwrap();
    let settled = service
        .settle_prepared_rewrite_failure(
            "conversation-rewrite-failure",
            "rewrite-failure-assistant",
            Some("rewrite-failure-run"),
            "rewrite-failure-request",
            "injected pre-runtime failure",
        )
        .unwrap()
        .unwrap();
    assert_eq!(settled.assistant_message.status.as_deref(), Some("error"));
    assert_eq!(
        storage
            .get_conversation_turn_trace("rewrite-failure-assistant")
            .unwrap()
            .unwrap()
            .terminal_status,
        ConversationTurnTraceTerminalStatus::Failed
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reopened_assistant_and_provider_transition_forks_complete_human_turns() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let model_server = tokio::spawn(async move {
        let mut requests = Vec::new();
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = read_provider_request(&mut stream).await;
            write_provider_stream(
                &mut stream,
                json!({ "role": "assistant", "content": "Continued fork answer." }),
                "stop",
            )
            .await;
            requests.push(request);
        }
        requests
    });

    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("storage.sqlite");
    let source_conversation_id = "conversation-root-fork-source";
    let source_assistant_message_id = "assistant-root-fork-source";
    let (assistant_fork_id, divider_fork_id) = {
        let storage = Arc::new(StorageService::open(&database_path).unwrap());
        let mut settings = test_model_settings();
        settings.api_url = format!("http://{address}/v1/chat/completions");
        settings.api_token = "fork-transition-token".to_string();
        let mut target_model = settings.models[0].clone();
        target_model.id = "model-2".to_string();
        target_model.display_name = "Model 2".to_string();
        target_model.provider_profile_config =
            mycopilot_core::ProviderProfileConfig::deepseek_v4_default();
        settings.models.push(target_model);
        storage.save_model_settings(settings).unwrap();
        storage
            .save_conversation(ChatConversationRecord {
                id: source_conversation_id.to_string(),
                project_id: None,
                model_id: Some("model-1".to_string()),
                title: "Fork continuation source".to_string(),
                messages: vec![
                    ChatMessageRecord {
                        id: "user-root-fork-source".to_string(),
                        role: "user".to_string(),
                        content: "Inspect the source file".to_string(),
                        created_at: 1,
                        status: Some("sent".to_string()),
                        attachments: Vec::new(),
                        agent_run_json: None,
                        ui_state_json: None,
                    },
                    ChatMessageRecord {
                        id: source_assistant_message_id.to_string(),
                        role: "assistant".to_string(),
                        content: "Source inspection complete.".to_string(),
                        created_at: 2,
                        status: Some("sent".to_string()),
                        attachments: Vec::new(),
                        // This raw row intentionally has no renderer Timeline. The durable Trace
                        // below makes the Fork view richer than its immutable persisted snapshot.
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
            .ensure_root_agent(&mycopilot_core::EnsureRootAgentInput {
                agent_id: "agent-root-fork-source".to_string(),
                conversation_id: source_conversation_id.to_string(),
                creation_request_id: "ensure-root-fork-source".to_string(),
                task_name: "Fork continuation source".to_string(),
            })
            .unwrap();

        let mut source_trace =
            completed_trace(source_conversation_id, source_assistant_message_id, 18);
        for item in &mut source_trace.items {
            match item {
                ConversationTurnTraceItem::ToolCall {
                    tool,
                    provenance,
                    operation,
                    ..
                } => {
                    *tool = "apply_patch".to_string();
                    *provenance = AgentToolIdentity::Builtin {
                        tool_name: "apply_patch".to_string(),
                    };
                    *operation = json!({
                        "request": {
                            "action": "apply",
                            "operation": "create",
                            "filePath": "src/history.rs",
                            "contentBytes": 22,
                            "contentDigest": mycopilot_core::file_change::content_digest(
                                b"durable source content",
                            ),
                        }
                    });
                }
                ConversationTurnTraceItem::ToolResult { tool, .. } => {
                    *tool = "apply_patch".to_string();
                }
                _ => {}
            }
        }
        let runtime_call_id = history_call_id();
        let model_context_items = vec![
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
                    id: runtime_call_id.clone(),
                    name: "apply_patch".to_string(),
                    args: json!({
                        "request": {
                            "action": "apply",
                            "operation": "create",
                            "filePath": "src/history.rs",
                            "content": "durable source content"
                        }
                    }),
                    provider_identity: AgentProviderToolCallIdentity {
                        provider_tool_index: 0,
                        provider_call_id: "write-history".to_string(),
                        runtime_call_id: runtime_call_id.clone(),
                    },
                }],
                is_error: false,
            },
            ConversationModelContextItem {
                sequence: 2,
                ordinal: 0,
                role: "tool".to_string(),
                content: r#"{"ok":true,"result":{"status":"applied"}}"#.to_string(),
                tool_call_id: Some(runtime_call_id),
                tool_calls: Vec::new(),
                is_error: false,
            },
        ];
        let mut in_progress = source_trace.clone();
        in_progress.terminal_status = ConversationTurnTraceTerminalStatus::InProgress;
        storage
            .append_in_progress_conversation_turn_trace_and_apply_guidances(
                &in_progress,
                &model_context_items,
                2,
                2,
            )
            .unwrap();
        storage
            .replace_conversation_turn_trace(&source_trace, 2, 3)
            .unwrap();

        let assistant_fork = storage
            .fork_conversation_request_view(
                mycopilot_core::storage::models::ForkConversationRequest {
                    request_id: "fork-assistant-reply-and-continue".to_string(),
                    source_conversation_id: source_conversation_id.to_string(),
                    fork_point:
                        mycopilot_core::storage::models::ConversationForkPoint::AssistantReply {
                            assistant_message_id: source_assistant_message_id.to_string(),
                        },
                },
            )
            .unwrap();
        let assistant_fork_run: Value = serde_json::from_str(
            assistant_fork.conversation.messages[1]
                .agent_run_json
                .as_deref()
                .expect("assistant Fork view must reconstruct its durable tool Timeline"),
        )
        .unwrap();
        assert_eq!(assistant_fork_run["timeline"][0]["type"], "message");
        assert_eq!(assistant_fork_run["timeline"][1]["type"], "tool_call");
        let assistant_fork_id = assistant_fork.conversation.id;

        let transition_service = AgentService::new(Arc::clone(&storage))
            .with_context_compaction_summary_generator(fork_transition_summary_generator());
        let preflight = transition_service
            .preflight_provider_transition(AgentProviderTransitionPreflightInput {
                conversation_id: source_conversation_id.to_string(),
                target_model_id: "model-2".to_string(),
            })
            .unwrap();
        assert_eq!(
            preflight.decision,
            AgentProviderTransitionDecision::RequiresCompaction
        );
        let (transition_notifications, mut transition_receiver) =
            tokio::sync::mpsc::unbounded_channel();
        let transition = transition_service
            .start_provider_transition(
                AgentProviderTransitionStartInput {
                    conversation_id: source_conversation_id.to_string(),
                    target_model_id: "model-2".to_string(),
                    transition_token: preflight.transition_token.unwrap(),
                },
                transition_notifications,
            )
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let event = transition_receiver
                    .recv()
                    .await
                    .expect("provider transition event channel closed");
                if event["params"]["status"] == "completed" {
                    break;
                }
            }
        })
        .await
        .expect("provider transition must complete deterministically");
        assert_eq!(
            storage
                .load_conversation(source_conversation_id)
                .unwrap()
                .unwrap()
                .model_id
                .as_deref(),
            Some("model-2")
        );

        let divider_fork = storage
            .fork_conversation_request_view(
                mycopilot_core::storage::models::ForkConversationRequest {
                    request_id: "fork-provider-divider-and-continue".to_string(),
                    source_conversation_id: source_conversation_id.to_string(),
                    fork_point: mycopilot_core::storage::models::ConversationForkPoint::ProviderTransitionBoundary {
                        operation_id: transition.operation_id,
                    },
                },
            )
            .unwrap();
        assert_eq!(
            divider_fork.conversation.model_id.as_deref(),
            Some("model-2")
        );
        assert_eq!(
            storage
                .get_active_context_compaction_summary(&divider_fork.conversation.id)
                .unwrap()
                .unwrap()
                .content,
            "Durable provider transition summary."
        );
        let forked_run: Value = serde_json::from_str(
            divider_fork.conversation.messages[1]
                .agent_run_json
                .as_deref()
                .expect("divider Fork view must reconstruct its durable tool Timeline"),
        )
        .unwrap();
        assert_eq!(forked_run["timeline"][0]["type"], "message");
        assert_eq!(forked_run["timeline"][1]["type"], "tool_call");
        (assistant_fork_id, divider_fork.conversation.id)
    };

    // A new Host process uses the raw admission snapshot and must not feed the reconstructed
    // renderer Timeline back through the immutable snapshot guard.
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    let service =
        AgentService::try_new_with_startup_reconciliation(Arc::clone(&storage), false, None)
            .unwrap();
    for (suffix, conversation_id, model_id) in [
        ("assistant", &assistant_fork_id, "model-1"),
        ("divider", &divider_fork_id, "model-2"),
    ] {
        let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let mut input = turn_input(model_id);
        input.conversation_id = Some(conversation_id.clone());
        input.content = format!("Continue from the {suffix} forked snapshot");
        input.user_message_id = Some(format!("user-root-{suffix}-fork-next"));
        input.assistant_message_id = Some(format!("assistant-root-{suffix}-fork-next"));

        let turn = service
            .start_conversation_turn(input, notifications)
            .expect("reopened collaboration root Fork must admit a human Turn");
        let events = collect_until_done(&mut receiver).await;
        let persisted_after_turn = storage.load_conversation(conversation_id).unwrap().unwrap();
        let persisted_error = rusqlite::Connection::open(&database_path)
            .unwrap()
            .query_row(
                "SELECT error FROM agent_usage_records WHERE run_id = ?1",
                [&turn.run_id],
                |row| row.get::<_, Option<String>>(0),
            )
            .unwrap();
        assert_eq!(
            events.last().unwrap()["params"]["status"],
            "completed",
            "reopened {suffix} Fork failed: {persisted_error:?}"
        );

        let conversation = persisted_after_turn;
        assert_eq!(conversation.messages.len(), 4);
        assert_eq!(conversation.messages[3].id, turn.assistant_message_id);
        assert_eq!(conversation.messages[3].content, "Continued fork answer.");
        let old_run: Value =
            serde_json::from_str(conversation.messages[1].agent_run_json.as_deref().unwrap())
                .unwrap();
        assert_eq!(old_run["timeline"][0]["type"], "message");
        assert_eq!(old_run["timeline"][1]["type"], "tool_call");
    }

    let requests = model_server.await.unwrap();
    let request_texts = requests
        .iter()
        .map(provider_request_message_text)
        .collect::<Vec<_>>();
    assert!(request_texts
        .iter()
        .any(|text| text.contains("Continue from the assistant forked snapshot")));
    assert!(request_texts.iter().any(|text| {
        text.contains("Durable provider transition summary.")
            && text.contains("Continue from the divider forked snapshot")
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
        Some(mycopilot_core::ProviderProfileConfig::V1(
            mycopilot_core::ProviderProfileConfigV1 {
                schema_version: mycopilot_core::PROVIDER_PROFILE_CONFIG_SCHEMA_VERSION,
                profile: mycopilot_core::ProviderProfileRef::deepseek_v4_chat(),
                reasoning: mycopilot_core::ReasoningPolicy {
                    mode: mycopilot_core::ReasoningMode::Enabled,
                    effort: mycopilot_core::ReasoningEffort::High,
                },
            },
        )),
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
    let inherited_permissions = AgentPermissions {
        read: mycopilot_core::AgentReadPermission::All,
        write: mycopilot_core::AgentWritePermission::WorkspaceOnly,
        command: mycopilot_core::AgentCommandPermission::AutoApprove,
        command_safety: mycopilot_core::AgentCommandSafetyPolicy::Guarded,
        patch: mycopilot_core::AgentPatchPermission::AutoApprove,
        builtin_execution: mycopilot_core::AgentBuiltinExecutionPermission::AutoApprove,
    };
    seed_root_effective_permissions(
        &storage,
        "agent-root-for-child",
        "conversation-root-for-child",
        "root-for-child",
        inherited_permissions,
    );
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
    .unwrap()
    .with_global_permit(service.turn_concurrency_gate().try_acquire().unwrap());
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
    assert_eq!(
        storage
            .get_agent_effective_permission_snapshot(&spawn.agent.agent_id)
            .unwrap()
            .unwrap()
            .permissions,
        inherited_permissions
    );
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
    let run_world_state = provider_messages
        .iter()
        .map(provider_message_text)
        .find(|content| {
            content.contains("<backend_world_state_record>")
                && content.contains("permissions.effective")
        })
        .expect("the real child Runtime projects its effective permissions to the provider");
    let run_world_state = run_world_state
        .lines()
        .find_map(|line| serde_json::from_str::<Value>(line.trim()).ok())
        .unwrap_or_else(|| {
            panic!("the child Runtime emits one canonical World State JSON record: {run_world_state:?}")
        });
    let effective_permissions = run_world_state["sections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|section| section["id"] == "permissions.effective")
        .expect("the child Runtime World State contains effective permissions");
    assert_eq!(
        effective_permissions["value"],
        json!({
            "read": "all",
            "write": "workspace_only",
            "command": "auto_approve",
            "commandSafety": "guarded",
            "patch": "auto_approve",
            "builtinExecution": "auto_approve"
        })
    );
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dispatcher_runs_two_persisted_children_and_an_idle_followup_through_the_shared_loop() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let model_server = tokio::spawn(async move {
        let mut requests = Vec::new();
        for answer in [
            "First child result.",
            "Second child result.",
            "First child follow-up result.",
        ] {
            let (mut stream, _) = listener.accept().await.unwrap();
            requests.push(read_provider_request(&mut stream).await);
            write_provider_stream(
                &mut stream,
                json!({ "role": "assistant", "content": answer }),
                "stop",
            )
            .await;
        }
        requests
    });

    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    save_provider_profile_fixture(
        &storage,
        &format!("http://{address}/v1/chat/completions"),
        None,
    );
    let root_conversation_id = "conversation-dispatcher-closed-loop";
    let root_agent_id = "agent-dispatcher-closed-loop";
    storage
        .save_conversation(ChatConversationRecord {
            id: root_conversation_id.to_string(),
            project_id: None,
            model_id: Some("model-1".to_string()),
            title: "Dispatcher closed loop".to_string(),
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
            agent_id: root_agent_id.to_string(),
            conversation_id: root_conversation_id.to_string(),
            creation_request_id: "ensure-dispatcher-root".to_string(),
            task_name: "Root".to_string(),
        })
        .unwrap();
    seed_root_effective_permissions(
        &storage,
        root_agent_id,
        root_conversation_id,
        "dispatcher-root",
        AgentPermissions::default(),
    );

    let factory =
        crate::application::agent_collaboration::ChildAgentFactory::new(Arc::clone(&storage));
    let first = factory
        .create_child(&mycopilot_core::CreateChildAgentInput {
            parent_agent_id: root_agent_id.to_string(),
            creation_request_id: "spawn-dispatcher-first".to_string(),
            task_name: "first_review".to_string(),
            task: "Inspect the first subsystem.".to_string(),
            template_machine_key: None,
            explicit_model_id: None,
            reasoning_effort: None,
            fork_turns: mycopilot_core::AgentForkTurns::None,
        })
        .unwrap();
    let second = factory
        .create_child(&mycopilot_core::CreateChildAgentInput {
            parent_agent_id: root_agent_id.to_string(),
            creation_request_id: "spawn-dispatcher-second".to_string(),
            task_name: "second_review".to_string(),
            task: "Inspect the second subsystem.".to_string(),
            template_machine_key: None,
            explicit_model_id: None,
            reasoning_effort: None,
            fork_turns: mycopilot_core::AgentForkTurns::None,
        })
        .unwrap();

    let service = AgentService::try_new_deferred_startup_reconciliation_with_agent_limit(
        Arc::clone(&storage),
        None,
        1,
    )
    .unwrap();
    let gate = service.turn_concurrency_gate();
    let (notifications, _receiver) = tokio::sync::mpsc::unbounded_channel();
    let store: Arc<dyn crate::application::agent_dispatcher::AgentDispatcherStore> = Arc::new(
        crate::application::agent_dispatcher::SqliteAgentDispatcherStore::new(Arc::clone(&storage)),
    );
    let executor: Arc<dyn crate::application::agent_dispatcher::AgentWakeTurnExecutionPort> =
        Arc::new(
            crate::application::agent_dispatcher::SharedAgentTurnExecutionPort::new(
                service.clone(),
                Arc::clone(&storage),
                notifications,
            )
            .with_fallback_poll_interval(Duration::from_millis(5)),
        );
    let dispatcher = crate::application::agent_dispatcher::AgentDispatcher::start(
        store,
        executor,
        gate.clone(),
        crate::application::agent_dispatcher::AgentDispatcherConfig {
            global_concurrency_limit: 1,
            wake_lease_renew_interval: Duration::from_millis(50),
            idle_poll_interval: Duration::from_millis(5),
            shutdown_grace: Duration::from_secs(2),
        },
    )
    .unwrap();
    dispatcher.notify_work_available();

    let first_terminal = wait_for_terminal_agent_wake(&storage, &first.initial_wake.wake_id).await;
    let second_terminal =
        wait_for_terminal_agent_wake(&storage, &second.initial_wake.wake_id).await;
    tokio::time::timeout(
        Duration::from_secs(1),
        dispatcher.wait_until_turns_settled(),
    )
    .await
    .expect("Dispatcher must release its result-settlement permit");
    assert_eq!(
        first_terminal.status,
        mycopilot_core::AgentWakeStatus::Completed
    );
    assert_eq!(
        second_terminal.status,
        mycopilot_core::AgentWakeStatus::Completed
    );
    assert_eq!(
        gate.active(),
        0,
        "settled children must return the shared slot"
    );

    for (terminal, child_agent_id) in [
        (&first_terminal, first.agent.agent_id.as_str()),
        (&second_terminal, second.agent.agent_id.as_str()),
    ] {
        let result = storage
            .get_agent_message(
                terminal
                    .result_message_id
                    .as_deref()
                    .expect("completed delegated Wake has a result Outbox"),
            )
            .unwrap()
            .unwrap();
        assert_eq!(result.kind, mycopilot_core::AgentMailboxKind::Result);
        assert_eq!(result.sender_agent_id, child_agent_id);
        assert_eq!(result.recipient_agent_id, root_agent_id);
        assert_eq!(
            result.delivery_status,
            mycopilot_core::AgentMailboxDeliveryStatus::Queued
        );
    }
    let root_traces = storage
        .list_conversation_turn_traces(root_conversation_id)
        .unwrap();
    assert_eq!(root_traces.len(), 1);
    assert_eq!(
        root_traces[0].assistant_message_id,
        "assistant-permission-seed-dispatcher-root"
    );
    assert_eq!(
        root_traces[0].terminal_status,
        ConversationTurnTraceTerminalStatus::Completed
    );
    let root_messages = storage
        .load_conversation(root_conversation_id)
        .unwrap()
        .unwrap()
        .messages;
    assert_eq!(root_messages.len(), 1);
    assert_eq!(
        root_messages[0].id,
        "assistant-permission-seed-dispatcher-root"
    );
    assert!(storage
        .claim_next_agent_wake(root_agent_id, "root-must-not-auto-wake")
        .unwrap()
        .is_none());

    let followup =
        crate::application::agent_collaboration::AgentMessagingService::new(Arc::clone(&storage))
            .follow_up(&mycopilot_core::SendAgentMessageRequest {
                sender_agent_id: root_agent_id.to_string(),
                recipient_agent_id: first.agent.agent_id.clone(),
                request_id: "follow-up-first-child".to_string(),
                content: "Now verify the follow-up condition.".to_string(),
            })
            .unwrap();
    let followup_wake = followup
        .deferred_wake
        .expect("follow-up must leave a durable execution opportunity");
    dispatcher.notify_work_available();
    let followup_terminal = wait_for_terminal_agent_wake(&storage, &followup_wake.wake_id).await;
    tokio::time::timeout(
        Duration::from_secs(1),
        dispatcher.wait_until_turns_settled(),
    )
    .await
    .expect("follow-up settlement must release its shared permit");
    assert_eq!(
        followup_terminal.status,
        mycopilot_core::AgentWakeStatus::Completed
    );
    assert_eq!(gate.active(), 0);
    assert_eq!(
        storage
            .list_conversation_turn_traces(&first.agent.conversation_id)
            .unwrap()
            .len(),
        2,
        "an idle child must reuse its persistent Conversation for follow-up"
    );
    let followup_result = storage
        .get_agent_message(
            followup_terminal
                .result_message_id
                .as_deref()
                .expect("follow-up has a result Outbox"),
        )
        .unwrap()
        .unwrap();
    assert_eq!(followup_result.sender_agent_id, first.agent.agent_id);
    assert_eq!(followup_result.recipient_agent_id, root_agent_id);
    assert!(storage
        .claim_next_agent_wake(root_agent_id, "root-still-must-not-auto-wake")
        .unwrap()
        .is_none());

    dispatcher.shutdown().await.unwrap();
    let requests = model_server.await.unwrap();
    assert_eq!(requests.len(), 3);
    let request_messages = requests
        .iter()
        .map(provider_request_message_text)
        .collect::<Vec<_>>();
    assert!(
        request_messages[..2]
            .iter()
            .any(|content| content.contains("first subsystem")),
        "messages={request_messages:?} requests={requests:#?}"
    );
    assert!(
        request_messages[..2]
            .iter()
            .any(|content| content.contains("second subsystem")),
        "messages={request_messages:?} requests={requests:#?}"
    );
    assert!(
        request_messages[2].contains("Now verify the follow-up condition."),
        "{request_messages:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn recovered_unknown_child_releases_startup_permit_and_accepts_a_later_followup() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let model_server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let request = read_provider_request(&mut stream).await;
        write_provider_stream(
            &mut stream,
            json!({ "role": "assistant", "content": "Recovered child follow-up result." }),
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
    storage
        .save_conversation(ChatConversationRecord {
            id: "conversation-recovery-root".to_string(),
            project_id: None,
            model_id: Some("model-1".to_string()),
            title: "Recovery root".to_string(),
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
            agent_id: "agent-recovery-root".to_string(),
            conversation_id: "conversation-recovery-root".to_string(),
            creation_request_id: "ensure-recovery-root".to_string(),
            task_name: "Root".to_string(),
        })
        .unwrap();
    seed_root_effective_permissions(
        &storage,
        "agent-recovery-root",
        "conversation-recovery-root",
        "recovery-root",
        AgentPermissions::default(),
    );
    let child =
        crate::application::agent_collaboration::ChildAgentFactory::new(Arc::clone(&storage))
            .create_child(&mycopilot_core::CreateChildAgentInput {
                parent_agent_id: "agent-recovery-root".to_string(),
                creation_request_id: "spawn-recovery-child".to_string(),
                task_name: "recovery_review".to_string(),
                task: "Start a potentially side-effecting review.".to_string(),
                template_machine_key: None,
                explicit_model_id: None,
                reasoning_effort: None,
                fork_turns: mycopilot_core::AgentForkTurns::None,
            })
            .unwrap();

    let admitted_at = mycopilot_core::storage::now_ms();
    let claimed = storage
        .claim_next_dispatchable_agent_wake_at("crashed-host", admitted_at)
        .unwrap()
        .unwrap();
    assert_eq!(claimed.wake_id, child.initial_wake.wake_id);
    let (conversation, revision) = storage
        .load_conversation_for_turn(&child.agent.conversation_id)
        .unwrap();
    let mut conversation = conversation.unwrap();
    let revision = revision.unwrap();
    let run_id = "run-crashed-after-runtime-admission";
    let assistant_message_id = "assistant-crashed-after-runtime-admission";
    conversation.messages.push(ChatMessageRecord {
        id: assistant_message_id.to_string(),
        role: "assistant".to_string(),
        content: "Work may have crossed an external side-effect boundary.".to_string(),
        created_at: admitted_at + 1,
        status: Some("pending".to_string()),
        attachments: Vec::new(),
        agent_run_json: None,
        ui_state_json: None,
    });
    conversation.updated_at = admitted_at + 1;
    let trace = mycopilot_core::ConversationTraceSnapshot::default().in_progress_trace(
        run_id,
        &child.agent.conversation_id,
        assistant_message_id,
    );
    storage
        .save_conversation_and_begin_turn_with_preloaded_agent_messages(
            conversation,
            Some(revision),
            Some(&mycopilot_core::TrustedAgentWakeTurnAdmission {
                agent_id: child.agent.agent_id.clone(),
                wake_id: claimed.wake_id.clone(),
                claim_token: claimed.claim_token.clone().unwrap(),
                source_agent_message_id: claimed.source_agent_message_id.clone().unwrap(),
            }),
            mycopilot_core::AgentTurnPermissionSource::InheritTrustedAncestors,
            &[claimed.source_agent_message_id.clone().unwrap()],
            &trace,
            admitted_at + 1,
            admitted_at + 1,
        )
        .unwrap();
    let running = storage.get_agent_wake(&claimed.wake_id).unwrap().unwrap();
    assert_eq!(running.status, mycopilot_core::AgentWakeStatus::Running);

    // A fresh AgentService reconstructs both Conversation occupancy and the process-global slot
    // from the durable in-progress trace. Dispatcher recovery must retire both only after the
    // outcome_unknown result transaction commits.
    let service = AgentService::try_new_deferred_startup_reconciliation_with_agent_limit(
        Arc::clone(&storage),
        None,
        1,
    )
    .unwrap();
    let gate = service.turn_concurrency_gate();
    assert_eq!(gate.active(), 1);
    let (notifications, _receiver) = tokio::sync::mpsc::unbounded_channel();
    let store: Arc<dyn crate::application::agent_dispatcher::AgentDispatcherStore> = Arc::new(
        crate::application::agent_dispatcher::SqliteAgentDispatcherStore::new(Arc::clone(&storage)),
    );
    let executor: Arc<dyn crate::application::agent_dispatcher::AgentWakeTurnExecutionPort> =
        Arc::new(
            crate::application::agent_dispatcher::SharedAgentTurnExecutionPort::new(
                service.clone(),
                Arc::clone(&storage),
                notifications,
            )
            .with_fallback_poll_interval(Duration::from_millis(5)),
        );
    let recovery_at = running.lease_expires_at.unwrap();
    let clock = Arc::new(AdvancingDispatcherClock::new(recovery_at));
    let dispatcher = crate::application::agent_dispatcher::AgentDispatcher::start_with_clock(
        store,
        executor,
        clock.clone(),
        gate.clone(),
        crate::application::agent_dispatcher::AgentDispatcherConfig {
            global_concurrency_limit: 1,
            wake_lease_renew_interval: Duration::from_millis(50),
            idle_poll_interval: Duration::from_millis(5),
            shutdown_grace: Duration::from_secs(2),
        },
    )
    .unwrap();
    let recovered = wait_for_terminal_agent_wake(&storage, &claimed.wake_id).await;
    assert_eq!(
        recovered.status,
        mycopilot_core::AgentWakeStatus::OutcomeUnknown
    );
    tokio::time::timeout(
        Duration::from_secs(1),
        dispatcher.wait_until_turns_settled(),
    )
    .await
    .expect("recovery settlement must retire startup occupancy and its counted permit");
    assert_eq!(gate.active(), 0);
    clock.reset(mycopilot_core::storage::now_ms());

    let followup =
        crate::application::agent_collaboration::AgentMessagingService::new(Arc::clone(&storage))
            .follow_up(&mycopilot_core::SendAgentMessageRequest {
                sender_agent_id: "agent-recovery-root".to_string(),
                recipient_agent_id: child.agent.agent_id.clone(),
                request_id: "follow-up-after-unknown".to_string(),
                content: "Continue only with a safe read-only check.".to_string(),
            })
            .unwrap();
    let followup_wake = followup.deferred_wake.unwrap();
    dispatcher.notify_work_available();
    let completed = wait_for_terminal_agent_wake(&storage, &followup_wake.wake_id).await;
    assert_eq!(completed.status, mycopilot_core::AgentWakeStatus::Completed);
    tokio::time::timeout(
        Duration::from_secs(1),
        dispatcher.wait_until_turns_settled(),
    )
    .await
    .expect("a later child Turn must return the same shared permit");
    assert_eq!(gate.active(), 0);
    assert_eq!(
        storage
            .list_conversation_turn_traces(&child.agent.conversation_id)
            .unwrap()
            .len(),
        2
    );
    dispatcher.shutdown().await.unwrap();
    let request = model_server.await.unwrap();
    assert!(provider_request_message_text(&request).contains("safe read-only check"));
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
    let service = AgentService::new(Arc::clone(&storage));
    let trusted = TrustedAgentWakeTurnStart::new(
        spawn.initial_wake.wake_id.clone(),
        spawn.agent.agent_id.clone(),
        spawn.agent.conversation_id.clone(),
        spawn.task_message.message_id.clone(),
        claim_token.to_string(),
        spawn.collaboration_identity,
    )
    .unwrap()
    .with_global_permit(service.turn_concurrency_gate().try_acquire().unwrap());
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
                let mycopilot_core::ProviderProfileConfig::V1(config) = &mut profile else {
                    unreachable!("legacy DeepSeek constructor must produce schema v1")
                };
                config.reasoning.mode = mycopilot_core::ReasoningMode::Disabled;
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
                        "name": "apply_patch",
                        "arguments": serde_json::to_string(&json!({
                            "request": {
                                "action": "begin",
                                "operation": "create",
                                "filePath": "must-not-create.md"
                            }
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
    let mycopilot_core::ProviderProfileConfig::V1(config) = &mut profile else {
        unreachable!("legacy DeepSeek constructor must produce schema v1")
    };
    config.reasoning.mode = mycopilot_core::ReasoningMode::Disabled;
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
        .list_agent_file_changes_for_run(&turn.run_id)
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
