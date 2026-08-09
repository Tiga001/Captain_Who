use super::*;

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
fn legacy_model_freezes_the_generic_profile_for_the_resolved_dialect() {
    let fixture = tempdir().unwrap();
    let storage = StorageService::open(&fixture.path().join("storage.sqlite")).unwrap();
    storage.save_model_settings(test_model_settings()).unwrap();
    let revision = storage
        .load_model_settings_snapshot()
        .unwrap()
        .unwrap()
        .configuration_revision;

    let prepared = prepare_conversation_turn(
        &storage,
        &SkillsService::new(),
        turn_input("model-1"),
        "run-provider-profile-generic",
    )
    .unwrap();

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
fn explicit_profile_incompatible_with_the_endpoint_dialect_fails_closed() {
    let fixture = tempdir().unwrap();
    let storage = StorageService::open(&fixture.path().join("storage.sqlite")).unwrap();
    let mut settings = test_model_settings();
    settings.api_url = "https://api.anthropic.com/v1/messages".to_string();
    settings.models[0].provider_profile_config =
        Some(mycopilot_core::ProviderProfileConfig::deepseek_v4_default());
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
