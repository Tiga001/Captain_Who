use super::fixtures::{
    test_pending_resume_checkpoint_for_call, valid_resume_collaboration_identity,
};
use super::mcp_fixtures::{
    seed_durable_mcp_pending_owner, test_mcp_pending_action, RecoverableApprovalRaceInvoker,
};
use super::*;
use sha2::{Digest, Sha256};

fn freeze_provider_protocol(
    input: &mut AgentChatInput,
    provider_configuration_revision: String,
    config: mycopilot_core::ProviderProfileConfig,
) {
    let dialect = input
        .api_style
        .map(mycopilot_core::ProviderProtocolDialect::from)
        .unwrap_or_else(|| {
            mycopilot_core::ProviderProtocolDialect::detect_from_api_url(&input.api_url)
        });
    let key = mycopilot_core::ProviderProtocolKey::new(
        dialect,
        &config,
        input.model.clone(),
        Some(provider_configuration_revision.clone()),
    )
    .unwrap();
    input.provider_configuration_revision = Some(provider_configuration_revision);
    input.provider_connection_revision =
        Some(format!("provider-connection-v1:{}", uuid::Uuid::new_v4()));
    input.search_connection_revision =
        Some(format!("search-connection-v1:{}", uuid::Uuid::new_v4()));
    input.provider_profile_config = Some(config);
    input.provider_protocol_key = Some(key);
    input.model_config_id = Some(input.model.clone());
}

fn freeze_deepseek_tool_checkpoint(
    input: &mut AgentChatInput,
    provider_continuation_refs: Vec<mycopilot_core::ProviderContinuationRef>,
) {
    let profile = deepseek_flash_profile(
        mycopilot_core::ReasoningMode::Disabled,
        mycopilot_core::ProviderReasoningEffort::ProviderDefault,
    );
    input.model = "deepseek-flash".to_string();
    input.model_capabilities.image_input = true;
    let key = mycopilot_core::ProviderProtocolKey::new(
        mycopilot_core::ProviderProtocolDialect::OpenAiChatCompletions,
        &profile,
        input.model.clone(),
        input.provider_configuration_revision.clone(),
    )
    .unwrap();
    input.provider_profile_config = Some(profile.clone());
    input.provider_protocol_key = Some(key.clone());
    let checkpoint = input.resume_checkpoint.as_mut().unwrap();
    checkpoint.model_capabilities = input.model_capabilities;
    checkpoint.provider_profile_config = profile;
    checkpoint.provider_protocol_key = key;
    checkpoint.provider_continuation_refs = provider_continuation_refs;
}

fn seed_tampered_provider_continuation(
    database_path: &std::path::Path,
    continuation_ref: &mycopilot_core::ProviderContinuationRef,
    protocol: &mycopilot_core::ProviderProtocolKey,
    conversation_id: &str,
    assistant_message_id: &str,
    run_id: &str,
    runtime_call_id: &str,
) {
    let connection = rusqlite::Connection::open(database_path).unwrap();
    connection
        .execute(
            "INSERT INTO conversations (
                id, project_id, model_id, title, created_at, updated_at,
                pinned_at, archived_at, unread_at
             ) VALUES (?1, NULL, ?2, 'tampered continuation', 1, 1, NULL, NULL, NULL)",
            rusqlite::params![conversation_id, &protocol.model_id],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO messages (
                id, conversation_id, role, content, status, agent_run_json,
                created_at, position
             ) VALUES (?1, ?2, 'assistant', 'visible', 'pending', NULL, 1, 0)",
            rusqlite::params![assistant_message_id, conversation_id],
        )
        .unwrap();
    let protocol_digest = {
        let digest = Sha256::digest(serde_json::to_vec(protocol).unwrap());
        format!("sha256:{digest:x}")
    };
    connection
        .execute(
            "INSERT INTO provider_continuations (
                continuation_id, schema_version, envelope_version,
                conversation_id, assistant_message_id, run_id, request_index,
                assistant_turn_id, assistant_turn_digest, provider_protocol_digest,
                state, superseded_by, compression, encryption, payload_digest,
                nonce, ciphertext, decoded_bytes, compressed_bytes,
                created_at, updated_at, released_at
             ) VALUES (
                ?1, 1, 1, ?2, ?3, ?4, 0,
                ?5, ?6, ?7, 'active', NULL, 'zstd_binary_v1',
                'chacha20_poly1305_v1', ?8, ?9, ?10, 1, 1, 2, 2, NULL
             )",
            rusqlite::params![
                &continuation_ref.id,
                conversation_id,
                assistant_message_id,
                run_id,
                format!("at1_{}", "a".repeat(64)),
                format!("sha256:{}", "b".repeat(64)),
                protocol_digest,
                format!("sha256:{}", "c".repeat(64)),
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
            rusqlite::params![&continuation_ref.id, runtime_call_id],
        )
        .unwrap();
}

#[tokio::test]
async fn provider_continuation_preflight_accepts_decision_but_blocks_mcp_dispatch() {
    for scenario in ["empty", "missing", "tampered"] {
        let fixture = tempdir().unwrap();
        let database_path = fixture
            .path()
            .join(format!("provider-preflight-{scenario}.sqlite"));
        let credentials =
            Arc::new(mycopilot_core::image_generation::InMemoryCredentialStore::default());
        let storage = Arc::new(
            StorageService::open_with_model_credentials(&database_path, credentials.clone())
                .unwrap(),
        );
        save_test_pending_provider(
            &storage,
            "test-model",
            "https://example.test/v1/chat/completions",
            "test-token",
            "disabled",
            "",
        );
        let mut provider_settings = storage.load_model_settings().unwrap().unwrap();
        provider_settings.models[0].provider_model_id = "deepseek-flash".to_string();
        provider_settings.models[0].supports_image = true;
        provider_settings.models[0].provider_profile_config = deepseek_flash_profile(
            mycopilot_core::ReasoningMode::Disabled,
            mycopilot_core::ProviderReasoningEffort::ProviderDefault,
        );
        storage.save_model_settings(provider_settings).unwrap();
        let vault = Arc::new(
            mycopilot_core::ProviderContinuationVaultFactory::open_or_provision(
                Arc::clone(&storage),
                credentials.clone(),
            )
            .unwrap(),
        );
        let invoker = Arc::new(RecoverableApprovalRaceInvoker::default());
        let service =
            AgentService::try_new_deferred_startup_reconciliation_with_provider_continuation_vault(
                Arc::clone(&storage),
                Some(vault),
            )
            .unwrap();
        let run_id = format!("provider-preflight-{scenario}-run");
        let conversation_id = format!("provider-preflight-{scenario}-conversation");
        let assistant_message_id = format!("provider-preflight-{scenario}-assistant");
        let action_id = uuid::Uuid::new_v4().to_string();
        let invocation_id = uuid::Uuid::new_v4().to_string();
        let action = test_mcp_pending_action(
            &run_id,
            &action_id,
            &invocation_id,
            mycopilot_core::storage::now_ms(),
        );
        let AgentProposedAction::McpToolCall { approval } = &action else {
            unreachable!("fixture always creates an MCP approval");
        };
        let mut input = serde_json::from_value::<AgentChatInput>(json!({
            "apiUrl": "https://example.test/v1/chat/completions",
            "apiToken": "test-token",
            "model": "deepseek-flash",
            "modelCapabilities": { "imageInput": true },
            "messages": []
        }))
        .unwrap();
        let run_context = mycopilot_core::AgentRunContext {
            conversation_id: Some(conversation_id.clone()),
            project_id: None,
            workspace: None,
            attachment_library: None,
            permissions: mycopilot_core::AgentPermissions::default(),
            collaboration_identity: None,
        };
        input.context = Some(run_context.clone());
        let mut checkpoint = test_pending_resume_checkpoint_for_call(
            &storage,
            &run_id,
            Some(&action_id),
            &approval.call,
            AgentToolIdentity::Mcp {
                provenance: approval.identity.provenance.clone(),
            },
        );
        checkpoint.run_context = Some(run_context);
        input.resume_checkpoint = Some(checkpoint);
        freeze_test_pending_provider_configuration(&storage, &mut input);
        let refs = if scenario == "empty" {
            Vec::new()
        } else {
            vec![mycopilot_core::ProviderContinuationRef::new()]
        };
        freeze_deepseek_tool_checkpoint(&mut input, refs.clone());
        if scenario == "tampered" {
            let checkpoint = input.resume_checkpoint.as_ref().unwrap();
            seed_tampered_provider_continuation(
                &database_path,
                &refs[0],
                &checkpoint.provider_protocol_key,
                &conversation_id,
                &assistant_message_id,
                &run_id,
                &checkpoint.pending_tool_call_id,
            );
        }
        seed_durable_mcp_pending_owner(
            &storage,
            &conversation_id,
            &assistant_message_id,
            &run_id,
            &action,
            mycopilot_core::storage::now_ms(),
        );
        assert!(service
            .store_pending_action(
                &run_id,
                &conversation_id,
                &assistant_message_id,
                action,
                input,
            )
            .unwrap());
        drop(service);
        drop(storage);

        let reopened_storage = Arc::new(
            StorageService::open_with_model_credentials(&database_path, credentials.clone())
                .unwrap(),
        );
        let reopened_vault = Arc::new(
            mycopilot_core::ProviderContinuationVaultFactory::open_or_provision(
                Arc::clone(&reopened_storage),
                credentials,
            )
            .unwrap(),
        );
        let restarted =
            AgentService::try_new_deferred_startup_reconciliation_with_provider_continuation_vault(
                reopened_storage,
                Some(reopened_vault),
            )
            .unwrap()
            .with_mcp_tool_invoker(invoker.clone() as Arc<dyn McpToolInvoker>);

        let (notifications, _receiver) = tokio::sync::mpsc::unbounded_channel();
        let output = restarted
            .approve_action(&run_id, &action_id, notifications)
            .unwrap();
        assert_eq!(output.status, "failed", "unexpected result for {scenario}");
        assert_eq!(
            output.agent_output.status,
            AgentRunStatus::Running,
            "the accepted ToolResult continues through the normal settlement chain"
        );
        assert_eq!(
            invoker
                .invocations
                .load(std::sync::atomic::Ordering::SeqCst),
            0,
            "{scenario} continuation state must block dispatch before the executor"
        );
        assert!(restarted.list_pending_actions().is_empty());
    }
}

#[test]
fn pending_resume_sqlite_row_contains_only_versioned_secret_free_projection() {
    const API_TOKEN_CANARY: &str = "SQLITE_PENDING_API_TOKEN_CANARY_DO_NOT_PERSIST";
    const URL_CANARY: &str = "SQLITE_PENDING_URL_CANARY_DO_NOT_PERSIST";
    const SEARCH_CANARY: &str = "SQLITE_PENDING_SEARCH_CANARY_DO_NOT_PERSIST";
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let sensitive_api_url = format!("https://example.test/{URL_CANARY}/v1");
    save_test_pending_provider(
        &storage,
        "test-model",
        &sensitive_api_url,
        API_TOKEN_CANARY,
        "tavily",
        SEARCH_CANARY,
    );
    let service = AgentService::new_authorized_for_test(Arc::clone(&storage));
    let mut agent_input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": sensitive_api_url.clone(),
        "apiToken": API_TOKEN_CANARY,
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "searchConfig": {
            "mode": "tavily",
            "tavilyApiKey": SEARCH_CANARY
        },
        "messages": []
    }))
    .unwrap();
    freeze_test_pending_provider_configuration(&storage, &mut agent_input);
    let frozen_profile = agent_input.provider_profile_config.clone().unwrap();
    let frozen_key = agent_input.provider_protocol_key.clone().unwrap();
    let call = AgentToolCall {
        id: "secret-free-resume-call".to_string(),
        tool: "approval_tool".to_string(),
        args: json!({}),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };
    agent_input.resume_checkpoint = Some(test_pending_resume_checkpoint_for_call(
        &storage,
        "secret-free-resume-run",
        None,
        &call,
        AgentToolIdentity::Unregistered {
            tool_name: call.tool.clone(),
        },
    ));
    let action = AgentProposedAction::ToolCall { call };
    service
        .store_pending_action(
            "secret-free-resume-run",
            "secret-free-resume-conversation",
            "secret-free-resume-assistant",
            action,
            agent_input,
        )
        .unwrap();
    drop(service);

    let row = storage.list_pending_agent_actions().unwrap().remove(0);
    assert!(
        PersistedAgentResumeInput::decode(&row.agent_input_json).is_ok(),
        "the durable projection must use the current resume-input schema"
    );
    for forbidden_key in ["\"apiUrl\"", "\"apiToken\"", "\"tavilyApiKey\""] {
        assert!(!row.agent_input_json.contains(forbidden_key));
    }
    for canary in [API_TOKEN_CANARY, URL_CANARY, SEARCH_CANARY] {
        assert!(!row.agent_input_json.contains(canary));
    }

    // A broad settings save during the approval pause changes the global revision but not this
    // model's effective wire protocol or search connection. Restart must retain the run's
    // original profile/key instead of recomputing provenance from the broad save revision.
    let mut edited = storage.load_model_settings().unwrap().unwrap();
    edited.models[0].context_window_tokens = Some(256_000);
    edited.models[0].input_price = "1.5".to_string();
    edited.models.push(ModelConfigRecord {
        id: "unrelated-restart-model".to_string(),
        provider_model_id: "unrelated-restart-model".to_string(),
        display_name: "Unrelated Restart Model".to_string(),
        api_url_override: Some("https://unrelated-restart.example/v1".to_string()),
        api_token_override: Some("unrelated-restart-token".to_string()),
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
    storage.save_model_settings(edited).unwrap();

    let reloaded = AgentService::new_authorized_for_test(storage);
    let pending = reloaded
        .pending_actions
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let restored = &pending.values().next().unwrap().agent_input;
    assert_eq!(
        restored.provider_profile_config.as_ref(),
        Some(&frozen_profile)
    );
    assert_eq!(restored.provider_protocol_key.as_ref(), Some(&frozen_key));
    assert_eq!(
        persisted_endpoint_digest(&restored.api_url),
        persisted_endpoint_digest(&sensitive_api_url)
    );
    assert!(!restored.api_token.is_empty());
    assert!(restored
        .search_config
        .as_ref()
        .unwrap()
        .tavily_api_key
        .is_none());
}

fn frozen_provider_resume_input(
    storage: &Arc<StorageService>,
    api_url: &str,
    api_token: &str,
    search_mode: &str,
    search_key: Option<&str>,
) -> DecodedPersistedAgentResumeInput {
    let dialect = mycopilot_core::ProviderProtocolDialect::detect_from_api_url(api_url);
    frozen_provider_resume_input_with_profile(
        storage,
        api_url,
        api_token,
        search_mode,
        search_key,
        mycopilot_core::ProviderProfileConfig::generic_for_dialect(dialect),
    )
}

fn frozen_provider_resume_input_with_profile(
    storage: &Arc<StorageService>,
    api_url: &str,
    api_token: &str,
    search_mode: &str,
    search_key: Option<&str>,
    profile: mycopilot_core::ProviderProfileConfig,
) -> DecodedPersistedAgentResumeInput {
    let snapshot = storage.load_model_settings_snapshot().unwrap().unwrap();
    let protocol_revision = snapshot.provider_protocol_revisions["test-model"].clone();
    let model = snapshot
        .settings
        .models
        .iter()
        .find(|model| model.id == "test-model")
        .unwrap();
    let provider_model_id = model.provider_model_id.clone();
    let supports_image = model.supports_image;
    let mut input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": api_url,
        "apiToken": api_token,
        "model": provider_model_id,
        "modelCapabilities": { "imageInput": supports_image },
        "contextWindowTokens": 128000,
        "searchConfig": {
            "mode": search_mode,
            "tavilyApiKey": search_key
        },
        "messages": []
    }))
    .unwrap();
    freeze_provider_protocol(&mut input, protocol_revision, profile);
    input.model_config_id = Some(model.id.clone());
    input.provider_connection_revision =
        Some(snapshot.provider_connection_revisions["test-model"].clone());
    input.search_connection_revision = Some(snapshot.search_connection_revision);
    let call = AgentToolCall {
        id: "provider-resume-call".to_string(),
        tool: "read_file".to_string(),
        args: json!({ "path": "README.md" }),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };
    let mut checkpoint = test_pending_resume_checkpoint_for_call(
        storage,
        "provider-resume-run",
        None,
        &call,
        AgentToolIdentity::Builtin {
            tool_name: call.tool.clone(),
        },
    );
    checkpoint.provider_profile_config = input.provider_profile_config.clone().unwrap();
    checkpoint.provider_protocol_key = input.provider_protocol_key.clone().unwrap();
    checkpoint.model_capabilities = input.model_capabilities;
    input.resume_checkpoint = Some(checkpoint);
    PersistedAgentResumeInput::decode(
        &PersistedAgentResumeInput::from_agent_input(&input)
            .unwrap()
            .encode(),
    )
    .unwrap()
}

fn collaboration_resume_input(storage: &StorageService) -> AgentChatInput {
    let snapshot = storage.load_model_settings_snapshot().unwrap().unwrap();
    let mut input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://resume-identity.example/v1/chat/completions",
        "apiToken": "resume-token",
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "contextWindowTokens": 128000,
        "messages": []
    }))
    .unwrap();
    freeze_provider_protocol(
        &mut input,
        snapshot.provider_protocol_revisions["test-model"].clone(),
        mycopilot_core::ProviderProfileConfig::generic_for_dialect(
            mycopilot_core::ProviderProtocolDialect::OpenAiChatCompletions,
        ),
    );
    input.provider_connection_revision =
        Some(snapshot.provider_connection_revisions["test-model"].clone());
    input.search_connection_revision = Some(snapshot.search_connection_revision);
    let call = AgentToolCall {
        id: "resume-collaboration-call".to_string(),
        tool: "read_file".to_string(),
        args: json!({ "path": "README.md" }),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };
    let context = AgentRunContext {
        conversation_id: Some("conversation-child-resume".to_string()),
        project_id: None,
        workspace: None,
        attachment_library: None,
        permissions: AgentPermissions::default(),
        collaboration_identity: Some(valid_resume_collaboration_identity()),
    };
    let mut checkpoint = test_pending_resume_checkpoint_for_call(
        storage,
        "run-collaboration-resume",
        None,
        &call,
        AgentToolIdentity::Builtin {
            tool_name: call.tool.clone(),
        },
    );
    checkpoint.run_context = Some(context.clone());
    input.context = Some(context);
    input.resume_checkpoint = Some(checkpoint);
    input
}

#[test]
fn persisted_resume_requires_exact_collaboration_identity_on_both_context_copies() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    save_test_pending_provider(
        &storage,
        "test-model",
        "https://resume-identity.example/v1/chat/completions",
        "resume-token",
        "disabled",
        "",
    );
    let input = collaboration_resume_input(&storage);
    let encoded = PersistedAgentResumeInput::from_agent_input(&input)
        .unwrap()
        .encode();
    assert!(PersistedAgentResumeInput::decode(&encoded).is_ok());

    let mut missing: Value = serde_json::from_str(&encoded).unwrap();
    missing["context"]
        .as_object_mut()
        .unwrap()
        .remove("collaborationIdentity");
    assert_eq!(
        PersistedAgentResumeInput::decode(&missing.to_string()).unwrap_err(),
        PersistedAgentResumeInputError::InvalidShape
    );

    let mut forged: Value = serde_json::from_str(&encoded).unwrap();
    forged["context"]["collaborationIdentity"]["entrustedTask"] =
        Value::String("A forged task".to_string());
    assert_eq!(
        PersistedAgentResumeInput::decode(&forged.to_string()).unwrap_err(),
        PersistedAgentResumeInputError::InvalidShape
    );

    let mut old: Value = serde_json::from_str(&encoded).unwrap();
    old["resumeInputSchemaVersion"] = json!(6);
    assert_eq!(
        PersistedAgentResumeInput::decode(&old.to_string()).unwrap_err(),
        PersistedAgentResumeInputError::UnsupportedOrMalformed
    );

    let mut mismatched = input;
    mismatched
        .resume_checkpoint
        .as_mut()
        .unwrap()
        .run_context
        .as_mut()
        .unwrap()
        .collaboration_identity
        .as_mut()
        .unwrap()
        .entrusted_task = "Different but individually valid task".to_string();
    let error = match PersistedAgentResumeInput::from_agent_input(&mismatched) {
        Ok(_) => panic!("mismatched Collaboration identity must be rejected"),
        Err(error) => error,
    };
    assert!(error.contains("disagrees"), "{error}");
}

#[test]
fn pending_resume_rejects_provider_token_replacement_at_the_same_endpoint() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let endpoint = "https://provider-identity.example/v1";
    save_test_pending_provider(
        &storage,
        "test-model",
        endpoint,
        "first-fixed-token",
        "disabled",
        "",
    );
    let frozen =
        frozen_provider_resume_input(&storage, endpoint, "first-fixed-token", "disabled", None);

    save_test_pending_provider(
        &storage,
        "test-model",
        endpoint,
        "replacement-fixed-token",
        "disabled",
        "",
    );

    let error = restore_agent_input_secrets(&storage, frozen).unwrap_err();
    assert!(error.contains("provider connection no longer matches"));
}

#[test]
fn pending_resume_selects_local_config_when_provider_model_id_is_shared() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let selected_endpoint = "https://selected-provider.example/v1";
    let selected_token = "selected-provider-token";
    save_test_pending_provider(
        &storage,
        "test-model",
        "https://global-provider.example/v1",
        "global-provider-token",
        "disabled",
        "",
    );

    let mut settings = storage.load_model_settings().unwrap().unwrap();
    settings.models[0].api_url_override = Some(selected_endpoint.to_string());
    settings.models[0].api_token_override = Some(selected_token.to_string());
    let decoy = ModelConfigRecord {
        id: "decoy-model-config".to_string(),
        provider_model_id: "test-model".to_string(),
        display_name: "Decoy model config".to_string(),
        api_url_override: Some("https://decoy-provider.example/v1".to_string()),
        api_token_override: Some("decoy-provider-token".to_string()),
        supports_image: false,
        context_window_tokens: Some(64_000),
        provider_profile_config: mycopilot_core::ProviderProfileConfig::generic_for_dialect(
            mycopilot_core::ProviderProtocolDialect::OpenAiChatCompletions,
        ),
        input_price: "0".to_string(),
        cached_input_price: String::new(),
        output_price: "0".to_string(),
        enabled: true,
    };
    settings.models.insert(0, decoy);
    storage.save_model_settings(settings).unwrap();

    let frozen = frozen_provider_resume_input(
        &storage,
        selected_endpoint,
        selected_token,
        "disabled",
        None,
    );
    assert_eq!(
        frozen.agent_input.model_config_id.as_deref(),
        Some("test-model")
    );
    assert_eq!(frozen.agent_input.model, "test-model");

    let restored = restore_agent_input_secrets(&storage, frozen).unwrap();
    assert_eq!(restored.api_url, selected_endpoint);
    assert_eq!(restored.api_token, selected_token);
    assert_ne!(restored.api_url, "https://decoy-provider.example/v1");
}

#[test]
fn pending_resume_rejects_tokenless_to_token_presence_drift() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let endpoint = "https://provider-presence.example/v1";
    save_test_pending_provider(
        &storage,
        "test-model",
        endpoint,
        "new-fixed-token",
        "disabled",
        "",
    );
    // Simulate a tokenless frozen run bound to this otherwise-current non-secret revision. The
    // bidirectional presence check is independent of the revision CAS and must still fail closed.
    let frozen = frozen_provider_resume_input(&storage, endpoint, "", "disabled", None);

    let error = restore_agent_input_secrets(&storage, frozen).unwrap_err();
    assert!(error.contains("credential presence no longer matches"));
}

#[test]
fn pending_resume_preserves_frozen_protocol_across_unrelated_model_settings_edits() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let endpoint = "https://provider-model.example/v1";
    save_test_pending_provider(
        &storage,
        "test-model",
        endpoint,
        "fixed-model-token",
        "disabled",
        "",
    );
    let frozen =
        frozen_provider_resume_input(&storage, endpoint, "fixed-model-token", "disabled", None);
    let frozen_profile = frozen.agent_input.provider_profile_config.clone().unwrap();
    let frozen_key = frozen.agent_input.provider_protocol_key.clone().unwrap();
    let mut settings = storage.load_model_settings().unwrap().unwrap();
    settings.models[0].context_window_tokens = Some(256_000);
    settings.models[0].input_price = "1.5".to_string();
    settings.models.push(ModelConfigRecord {
        id: "unrelated-model".to_string(),
        provider_model_id: "unrelated-model".to_string(),
        display_name: "Unrelated Model".to_string(),
        api_url_override: Some("https://unrelated-provider.example/v1".to_string()),
        api_token_override: Some("unrelated-token".to_string()),
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
    storage.save_model_settings(settings).unwrap();

    let restored = restore_agent_input_secrets(&storage, frozen).unwrap();
    assert_eq!(
        restored.provider_profile_config.as_ref(),
        Some(&frozen_profile)
    );
    assert_eq!(restored.provider_protocol_key.as_ref(), Some(&frozen_key));
    assert_eq!(restored.context_window_tokens, Some(128_000));
    assert_eq!(restored.api_token, "fixed-model-token");
}

#[test]
fn pending_resume_preserves_frozen_generic_profile_when_current_model_selects_deepseek() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let endpoint = "https://provider-profile-change.example/v1";
    save_test_pending_provider(
        &storage,
        "test-model",
        endpoint,
        "fixed-model-token",
        "disabled",
        "",
    );
    let frozen =
        frozen_provider_resume_input(&storage, endpoint, "fixed-model-token", "disabled", None);

    let mut settings = storage.load_model_settings().unwrap().unwrap();
    settings.models[0].provider_profile_config =
        mycopilot_core::ProviderProfileConfig::deepseek_flash_default();
    settings.models[0].provider_model_id = "deepseek-flash".to_string();
    settings.models[0].supports_image = true;
    storage.save_model_settings(settings).unwrap();

    let frozen_profile = frozen.agent_input.provider_profile_config.clone().unwrap();
    let frozen_key = frozen.agent_input.provider_protocol_key.clone().unwrap();
    let restored = restore_agent_input_secrets(&storage, frozen).unwrap();
    assert_eq!(restored.provider_profile_config, Some(frozen_profile));
    assert_eq!(restored.provider_protocol_key, Some(frozen_key));
}

#[test]
fn pending_resume_preserves_frozen_deepseek_profile_after_host_selects_generic() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let endpoint = "https://provider-profile-freeze.example/v1/chat/completions";
    let token = "fixed-model-token";
    save_test_pending_provider(&storage, "test-model", endpoint, token, "disabled", "");

    let mut settings = storage.load_model_settings().unwrap().unwrap();
    settings.models[0].provider_profile_config =
        mycopilot_core::ProviderProfileConfig::deepseek_flash_default();
    settings.models[0].provider_model_id = "deepseek-flash".to_string();
    settings.models[0].supports_image = true;
    storage.save_model_settings(settings).unwrap();
    let deepseek = mycopilot_core::ProviderProfileConfig::deepseek_flash_default();
    let frozen = frozen_provider_resume_input_with_profile(
        &storage,
        endpoint,
        token,
        "disabled",
        None,
        deepseek.clone(),
    );
    let frozen_key = frozen.agent_input.provider_protocol_key.clone().unwrap();

    let current = storage.load_model_settings().unwrap().unwrap();
    storage
        .save_model_settings_request(renderer_model_settings_update(
            &storage,
            current,
            mycopilot_core::storage::models::ProviderProfileUpdate::SelectGeneric,
        ))
        .unwrap();

    let current = storage.load_model_settings().unwrap().unwrap();
    assert_eq!(
        current.models[0].provider_profile_config.profile().id,
        mycopilot_core::ProviderProfileId::GenericOpenAiChat
    );
    let restored = restore_agent_input_secrets(&storage, frozen).unwrap();
    assert_eq!(restored.provider_profile_config, Some(deepseek));
    assert_eq!(restored.provider_protocol_key, Some(frozen_key));
}

#[test]
fn pending_resume_rejects_provider_endpoint_replacement() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let endpoint = "https://provider-endpoint.example/v1";
    save_test_pending_provider(
        &storage,
        "test-model",
        endpoint,
        "fixed-model-token",
        "disabled",
        "",
    );
    let frozen =
        frozen_provider_resume_input(&storage, endpoint, "fixed-model-token", "disabled", None);

    save_test_pending_provider(
        &storage,
        "test-model",
        "https://replacement-provider.example/v1",
        "fixed-model-token",
        "disabled",
        "",
    );

    let error = restore_agent_input_secrets(&storage, frozen).unwrap_err();
    assert!(error.contains("provider connection no longer matches"));
}

#[test]
fn pending_resume_accepts_search_credential_replacement_without_rehydration() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let endpoint = "https://provider-search.example/v1";
    save_test_pending_provider(
        &storage,
        "test-model",
        endpoint,
        "fixed-provider-token",
        "tavily",
        "first-fixed-search-key",
    );
    let frozen = frozen_provider_resume_input(
        &storage,
        endpoint,
        "fixed-provider-token",
        "tavily",
        Some("first-fixed-search-key"),
    );
    save_test_pending_provider(
        &storage,
        "test-model",
        endpoint,
        "fixed-provider-token",
        "tavily",
        "replacement-fixed-search-key",
    );

    let restored = restore_agent_input_secrets(&storage, frozen).unwrap();
    assert!(restored.search_config.unwrap().tavily_api_key.is_none());
    assert!(storage
        .load_web_search_policy_snapshot()
        .unwrap()
        .available());
    assert!(storage.authorize_web_search_execution().is_ok());
}
