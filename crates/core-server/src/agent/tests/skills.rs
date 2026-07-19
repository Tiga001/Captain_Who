use super::*;

#[test]
fn conversation_turn_resolves_skill_snapshot_before_persisting_the_run() {
    const INSTRUCTIONS: &str = "SKILL_SERVER_MARKER: inspect evidence before editing.";
    let fixture = tempdir().unwrap();
    let workspace = fixture.path().join("workspace");
    write_test_skill(&workspace, INSTRUCTIONS);
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    storage.save_model_settings(test_model_settings()).unwrap();
    storage
        .save_project(ProjectRecord {
            id: "project-skills".to_string(),
            name: "Skill workspace".to_string(),
            path: Some(workspace.to_string_lossy().into_owned()),
            created_at: 1,
            pinned_at: None,
        })
        .unwrap();
    let skills = SkillsService::new();
    let catalog = skills.list_workspace("project-skills", &workspace).unwrap();
    let descriptor = catalog.skills().first().unwrap();
    let selection = mycopilot_protocol_rs::SkillSelectionDto {
        id: descriptor.id().as_str().to_string(),
        revision: descriptor.revision().as_str().to_string(),
    };

    let prepared = prepare_conversation_turn(
        &storage,
        &skills,
        skill_turn_input("project-skills", selection, "conversation-skills"),
        "run-skills",
    )
    .unwrap();

    let activation = prepared.agent_input.skill_activation.as_ref().unwrap();
    assert_eq!(activation.skills.len(), 1);
    assert_eq!(activation.skills[0].instructions.trim(), INSTRUCTIONS);
    assert_eq!(prepared.output.activated_skills.len(), 1);
    assert_eq!(
        prepared.output.skill_activation_revision.as_deref(),
        Some(activation.activation_revision.as_str())
    );
    let public_output = serde_json::to_string(&prepared.output).unwrap();
    assert!(!public_output.contains(INSTRUCTIONS));
    assert!(!public_output.contains("DESCRIPTION_DISCOVERY_ONLY"));

    let mut without_skill = prepared.agent_input.clone();
    without_skill.skill_activation = None;
    assert_eq!(
        conversation_context_configuration_revision(&prepared.agent_input).unwrap(),
        conversation_context_configuration_revision(&without_skill).unwrap()
    );
}

#[test]
fn bundled_skill_crosses_the_production_turn_boundary_without_public_instruction_leakage() {
    const BUNDLED_INSTRUCTION_MARKER: &str = "Treat Application trust as package provenance";
    let fixture = tempdir().unwrap();
    let workspace = fixture.path().join("workspace");
    fs::create_dir_all(&workspace).unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    storage.save_model_settings(test_model_settings()).unwrap();
    storage
        .save_project(ProjectRecord {
            id: "project-bundled-skill".to_string(),
            name: "Bundled Skill workspace".to_string(),
            path: Some(workspace.to_string_lossy().into_owned()),
            created_at: 1,
            pinned_at: None,
        })
        .unwrap();

    let skills = SkillsService::new().with_bundled_source().unwrap();
    let catalog = skills
        .list_with_workspace("project-bundled-skill", &workspace)
        .unwrap();
    let descriptor = catalog
        .skills()
        .iter()
        .find(|skill| skill.source_kind() == mycopilot_core::skills::SkillSourceKind::Bundled)
        .expect("production catalog must expose the bundled auditor");
    let selection = mycopilot_protocol_rs::SkillSelectionDto {
        id: descriptor.id().as_str().to_string(),
        revision: descriptor.revision().as_str().to_string(),
    };

    let prepared = prepare_conversation_turn(
        &storage,
        &skills,
        skill_turn_input(
            "project-bundled-skill",
            selection,
            "conversation-bundled-skill",
        ),
        "run-bundled-skill",
    )
    .unwrap();

    let activation = prepared.agent_input.skill_activation.as_ref().unwrap();
    assert_eq!(activation.skills.len(), 1);
    assert_eq!(
        activation.skills[0].id,
        "bundled:application:repository-evidence-auditor"
    );
    assert!(activation.skills[0]
        .instructions
        .contains(BUNDLED_INSTRUCTION_MARKER));
    assert_eq!(prepared.output.activated_skills.len(), 1);
    assert_eq!(
        prepared.output.activated_skills[0].source.kind,
        mycopilot_protocol_rs::SkillSourceKindDto::Bundled
    );

    let public_output = serde_json::to_string(&prepared.output).unwrap();
    assert!(!public_output.contains(BUNDLED_INSTRUCTION_MARKER));
    let persisted = storage
        .load_conversation("conversation-bundled-skill")
        .unwrap()
        .unwrap();
    assert!(!serde_json::to_string(&persisted)
        .unwrap()
        .contains(BUNDLED_INSTRUCTION_MARKER));
}

#[test]
fn bundled_skill_activates_without_a_project_in_turn_and_preview_paths() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    storage.save_model_settings(test_model_settings()).unwrap();
    let skills = Arc::new(SkillsService::new().with_bundled_source().unwrap());
    let descriptor = skills.list().unwrap().skills()[0].clone();
    let selection = mycopilot_protocol_rs::SkillSelectionDto {
        id: descriptor.id().as_str().to_string(),
        revision: descriptor.revision().as_str().to_string(),
    };

    let prepared = prepare_conversation_turn(
        &storage,
        &skills,
        global_skill_turn_input(selection.clone(), "conversation-global-bundled"),
        "run-global-bundled",
    )
    .unwrap();
    assert_eq!(
        prepared.agent_input.context.as_ref().unwrap().project_id,
        None
    );
    assert_eq!(
        prepared
            .agent_input
            .skill_activation
            .as_ref()
            .unwrap()
            .skills[0]
            .id,
        descriptor.id().as_str()
    );

    let service = AgentService::new(Arc::clone(&storage)).with_skills_service(skills);
    let preview = service
        .get_context_window_snapshot(AgentContextWindowSnapshotInput {
            conversation_id: None,
            project_id: None,
            model_id: "model-1".to_string(),
            max_tokens: Some(30_000),
            prompt_preferences: None,
            permissions: AgentPermissions::default(),
            skills: vec![selection],
        })
        .unwrap();
    assert!(preview.snapshot.is_some());
}

#[test]
fn disabled_global_skill_is_rejected_by_turn_and_preview_paths() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    storage.save_model_settings(test_model_settings()).unwrap();
    let skills = Arc::new(SkillsService::new().with_bundled_source().unwrap());
    let descriptor = skills.list().unwrap().skills()[0].clone();
    let skill_id = descriptor.id().as_str().to_string();
    storage
        .set_skill_enablement_override(&skill_id, false)
        .unwrap();
    let selection = mycopilot_protocol_rs::SkillSelectionDto {
        id: skill_id.clone(),
        revision: descriptor.revision().as_str().to_string(),
    };

    let turn_error = match prepare_conversation_turn(
        &storage,
        &skills,
        global_skill_turn_input(selection.clone(), "conversation-disabled-bundled"),
        "run-disabled-bundled",
    ) {
        Ok(_) => panic!("a disabled Skill must fail before preparing the run"),
        Err(error) => error,
    };
    let turn_data = turn_error.skill_activation().unwrap();
    assert_eq!(
        turn_data.code,
        mycopilot_protocol_rs::SkillActivationErrorCodeDto::Disabled
    );
    assert_eq!(
        turn_data.recovery,
        mycopilot_protocol_rs::SkillActivationRecoveryDto::RejectSelection
    );
    assert_eq!(turn_data.skill_id.as_deref(), Some(skill_id.as_str()));
    assert!(storage
        .load_conversation("conversation-disabled-bundled")
        .unwrap()
        .is_none());

    let service = AgentService::new(Arc::clone(&storage)).with_skills_service(skills);
    let preview_error = service
        .get_context_window_snapshot(AgentContextWindowSnapshotInput {
            conversation_id: None,
            project_id: None,
            model_id: "model-1".to_string(),
            max_tokens: Some(30_000),
            prompt_preferences: None,
            permissions: AgentPermissions::default(),
            skills: vec![selection],
        })
        .unwrap_err();
    assert_eq!(
        preview_error.skill_activation().unwrap().code,
        mycopilot_protocol_rs::SkillActivationErrorCodeDto::Disabled
    );
}

#[test]
fn workspace_skill_without_a_project_is_rejected_explicitly() {
    let fixture = tempdir().unwrap();
    let workspace = fixture.path().join("workspace");
    write_test_skill(&workspace, "WORKSPACE_ONLY_SKILL_MARKER");
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    storage.save_model_settings(test_model_settings()).unwrap();
    let skills = Arc::new(SkillsService::new());
    let descriptor = skills
        .list_workspace("detached-workspace", &workspace)
        .unwrap()
        .skills()[0]
        .clone();
    let selection = mycopilot_protocol_rs::SkillSelectionDto {
        id: descriptor.id().as_str().to_string(),
        revision: descriptor.revision().as_str().to_string(),
    };

    let turn_error = match prepare_conversation_turn(
        &storage,
        &skills,
        global_skill_turn_input(selection.clone(), "conversation-workspace-without-project"),
        "run-workspace-without-project",
    ) {
        Ok(_) => panic!("a workspace Skill must not activate without a project"),
        Err(error) => error,
    };
    let turn_data = turn_error.skill_activation().unwrap();
    assert_eq!(
        turn_data.code,
        mycopilot_protocol_rs::SkillActivationErrorCodeDto::InvalidSelection
    );
    assert!(turn_data.message.contains("requires a project"));

    let service = AgentService::new(Arc::clone(&storage)).with_skills_service(skills);
    let preview_error = service
        .get_context_window_snapshot(AgentContextWindowSnapshotInput {
            conversation_id: None,
            project_id: None,
            model_id: "model-1".to_string(),
            max_tokens: Some(30_000),
            prompt_preferences: None,
            permissions: AgentPermissions::default(),
            skills: vec![selection],
        })
        .unwrap_err();
    assert_eq!(
        preview_error.skill_activation().unwrap().code,
        mycopilot_protocol_rs::SkillActivationErrorCodeDto::InvalidSelection
    );
}

#[test]
fn installed_skill_crosses_the_production_turn_boundary_without_instruction_leakage() {
    const INSTALLATION_ID: &str = "0190b0f2-7c50-7cc0-8b25-3bb80f08b334";
    const INSTRUCTION_MARKER: &str = "INSTALLED_SKILL_AGENT_RUNTIME_MARKER";
    const DESCRIPTION_MARKER: &str = "INSTALLED_SKILL_DESCRIPTION_ONLY_MARKER";
    let fixture = tempdir().unwrap();
    let workspace = fixture.path().join("workspace");
    fs::create_dir_all(&workspace).unwrap();
    let store_root = fixture.path().join("skills");
    let local_skill = fixture.path().join("local-skill");
    fs::create_dir_all(&local_skill).unwrap();
    let source_text = format!(
        concat!(
            "---\n",
            "name: installed-runtime-auditor\n",
            "description: {}\n",
            "---\n",
            "# Instructions\n",
            "{}\n"
        ),
        DESCRIPTION_MARKER, INSTRUCTION_MARKER
    );
    fs::write(local_skill.join("SKILL.md"), &source_text).unwrap();
    let installation_id = SkillInstallationId::parse(INSTALLATION_ID).unwrap();
    let installations = SkillInstallationService::new(&store_root).unwrap();
    let installed = installations
        .install_local_directory(&LocalSkillInstallRequest::new(
            installation_id,
            &local_skill,
        ))
        .unwrap();
    assert_eq!(installed.outcome(), SkillInstallationOutcome::Installed);
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    storage.save_model_settings(test_model_settings()).unwrap();
    storage
        .save_project(ProjectRecord {
            id: "project-installed-skill".to_string(),
            name: "Installed Skill workspace".to_string(),
            path: Some(workspace.to_string_lossy().into_owned()),
            created_at: 1,
            pinned_at: None,
        })
        .unwrap();

    let skills = SkillsService::new()
        .with_installed_source(&store_root)
        .unwrap();
    let catalog = skills
        .list_with_workspace("project-installed-skill", &workspace)
        .unwrap();
    let descriptor = catalog
        .skills()
        .iter()
        .find(|skill| skill.source_kind() == mycopilot_core::skills::SkillSourceKind::Installed)
        .expect("production catalog must expose the installed Skill");
    let selection = mycopilot_protocol_rs::SkillSelectionDto {
        id: descriptor.id().as_str().to_string(),
        revision: descriptor.revision().as_str().to_string(),
    };

    let prepared = prepare_conversation_turn(
        &storage,
        &skills,
        skill_turn_input(
            "project-installed-skill",
            selection,
            "conversation-installed-skill",
        ),
        "run-installed-skill",
    )
    .unwrap();

    let activation = prepared.agent_input.skill_activation.as_ref().unwrap();
    assert_eq!(activation.skills.len(), 1);
    assert_eq!(
        activation.skills[0].id,
        format!("installed:user:{INSTALLATION_ID}")
    );
    assert_eq!(activation.skills[0].source, "installed:user");
    assert!(activation.skills[0]
        .instructions
        .contains(INSTRUCTION_MARKER));
    assert!(!activation.skills[0]
        .instructions
        .contains(DESCRIPTION_MARKER));
    assert_eq!(prepared.output.activated_skills.len(), 1);
    assert_eq!(
        prepared.output.activated_skills[0].source.kind,
        mycopilot_protocol_rs::SkillSourceKindDto::Installed
    );

    let public_output = serde_json::to_string(&prepared.output).unwrap();
    assert!(!public_output.contains(INSTRUCTION_MARKER));
    assert!(!public_output.contains(DESCRIPTION_MARKER));
    assert!(!public_output.contains(&source_text));
    let persisted = storage
        .load_conversation("conversation-installed-skill")
        .unwrap()
        .unwrap();
    let persisted_json = serde_json::to_string(&persisted).unwrap();
    assert!(!persisted_json.contains(INSTRUCTION_MARKER));
    assert!(!persisted_json.contains(DESCRIPTION_MARKER));
    assert!(!persisted_json.contains(&source_text));
}

#[test]
fn stale_skill_selection_fails_before_conversation_mutation() {
    let fixture = tempdir().unwrap();
    let workspace = fixture.path().join("workspace");
    write_test_skill(&workspace, "first revision");
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    storage.save_model_settings(test_model_settings()).unwrap();
    storage
        .save_project(ProjectRecord {
            id: "project-stale-skill".to_string(),
            name: "Skill workspace".to_string(),
            path: Some(workspace.to_string_lossy().into_owned()),
            created_at: 1,
            pinned_at: None,
        })
        .unwrap();
    let skills = SkillsService::new();
    let catalog = skills
        .list_workspace("project-stale-skill", &workspace)
        .unwrap();
    let descriptor = catalog.skills().first().unwrap();
    let selection = mycopilot_protocol_rs::SkillSelectionDto {
        id: descriptor.id().as_str().to_string(),
        revision: descriptor.revision().as_str().to_string(),
    };
    write_test_skill(&workspace, "second revision");

    let error = match prepare_conversation_turn(
        &storage,
        &skills,
        skill_turn_input("project-stale-skill", selection, "conversation-stale-skill"),
        "run-stale-skill",
    ) {
        Ok(_) => panic!("a stale Skill selection must fail before preparing the run"),
        Err(error) => error,
    };

    let data = error.skill_activation().unwrap();
    assert_eq!(
        data.code,
        mycopilot_protocol_rs::SkillActivationErrorCodeDto::Stale
    );
    assert_eq!(
        data.recovery,
        mycopilot_protocol_rs::SkillActivationRecoveryDto::RefreshCatalog
    );
    assert!(data.expected_revision.is_some());
    assert!(data.actual_revision.is_some());
    assert!(storage
        .load_conversation("conversation-stale-skill")
        .unwrap()
        .is_none());
}

#[test]
fn existing_conversation_rejects_cross_project_skill_turn_and_preview() {
    let fixture = tempdir().unwrap();
    let workspace_a = fixture.path().join("workspace-a");
    let workspace_b = fixture.path().join("workspace-b");
    fs::create_dir_all(&workspace_a).unwrap();
    write_test_skill(&workspace_b, "SKILL_PROJECT_B_MARKER");
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    storage.save_model_settings(test_model_settings()).unwrap();
    for (id, name, path) in [
        ("project-a", "Project A", &workspace_a),
        ("project-b", "Project B", &workspace_b),
    ] {
        storage
            .save_project(ProjectRecord {
                id: id.to_string(),
                name: name.to_string(),
                path: Some(path.to_string_lossy().into_owned()),
                created_at: 1,
                pinned_at: None,
            })
            .unwrap();
    }
    storage
        .save_conversation(ChatConversationRecord {
            id: "conversation-project-boundary".to_string(),
            project_id: Some("project-a".to_string()),
            model_id: Some("model-1".to_string()),
            title: "Project-bound conversation".to_string(),
            messages: vec![ChatMessageRecord {
                id: "user-existing-project-a".to_string(),
                role: "user".to_string(),
                content: "History from project A.".to_string(),
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

    let skills = Arc::new(SkillsService::new());
    let descriptor = skills
        .list_workspace("project-b", &workspace_b)
        .unwrap()
        .skills()[0]
        .clone();
    let selection = mycopilot_protocol_rs::SkillSelectionDto {
        id: descriptor.id().as_str().to_string(),
        revision: descriptor.revision().as_str().to_string(),
    };
    let turn_error = match prepare_conversation_turn(
        &storage,
        &skills,
        skill_turn_input(
            "project-b",
            selection.clone(),
            "conversation-project-boundary",
        ),
        "run-project-boundary",
    ) {
        Ok(_) => panic!("an ordinary turn must not migrate an existing conversation"),
        Err(error) => error,
    };
    assert!(turn_error.message().contains("不能迁移会话项目"));
    assert!(turn_error.skill_activation().is_none());
    let unchanged = storage
        .load_conversation("conversation-project-boundary")
        .unwrap()
        .unwrap();
    assert_eq!(unchanged.project_id.as_deref(), Some("project-a"));
    assert_eq!(unchanged.messages.len(), 1);

    let service = AgentService::new(Arc::clone(&storage)).with_skills_service(skills);
    let preview_error = service
        .get_context_window_snapshot(AgentContextWindowSnapshotInput {
            conversation_id: Some("conversation-project-boundary".to_string()),
            project_id: Some("project-b".to_string()),
            model_id: "model-1".to_string(),
            max_tokens: Some(30_000),
            prompt_preferences: None,
            permissions: AgentPermissions::default(),
            skills: vec![selection],
        })
        .unwrap_err();
    assert!(preview_error.message().contains("不能迁移会话项目"));
}

#[test]
fn conversation_turn_and_pending_restore_use_the_model_connection_override() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let mut settings = test_model_settings();
    settings.api_url.clear();
    settings.api_token.clear();
    settings.models[0].api_url_override = Some("https://model.example/v1".to_string());
    settings.models[0].api_token_override = Some("model-token".to_string());
    storage.save_model_settings(settings).unwrap();

    let prepared = prepare_conversation_turn(
        &storage,
        &SkillsService::new(),
        AgentConversationTurnInput {
            conversation_id: Some("conversation-model-override".to_string()),
            project_id: None,
            model_id: "model-1".to_string(),
            context_window_indicator_enabled: true,
            content: "Hello".to_string(),
            attachments: Vec::new(),
            skills: Vec::new(),
            title: None,
            user_message_id: Some("user-model-override".to_string()),
            assistant_message_id: Some("assistant-model-override".to_string()),
            max_tokens: None,
            temperature: None,
            prompt_preferences: None,
            permissions: AgentPermissions::default(),
        },
        "run-model-override",
    )
    .unwrap();

    assert_eq!(prepared.agent_input.api_url, "https://model.example/v1");
    assert_eq!(prepared.agent_input.api_token, "model-token");

    let mut persisted_input = prepared.agent_input;
    persisted_input.api_url = "https://stale.example/v1".to_string();
    persisted_input.api_token.clear();
    let restored = restore_agent_input_secrets(&storage, persisted_input);
    assert_eq!(restored.api_url, "https://model.example/v1");
    assert_eq!(restored.api_token, "model-token");
}
