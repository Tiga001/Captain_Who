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
        .find(|skill| skill.id().as_str() == "bundled:application:repository-evidence-auditor")
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
    fs::create_dir_all(local_skill.join("templates")).unwrap();
    fs::write(
        local_skill.join("templates/runtime.md"),
        "revision-bound resource marker",
    )
    .unwrap();
    fs::create_dir_all(local_skill.join("scripts")).unwrap();
    fs::write(
        local_skill.join("scripts/fail.py"),
        concat!(
            "import sys\n",
            "print('skill stdout marker')\n",
            "print('skill stderr marker', file=sys.stderr)\n",
            "raise SystemExit(17)\n",
        ),
    )
    .unwrap();
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

    let skills = Arc::new(
        SkillsService::new()
            .with_installed_source(&store_root)
            .unwrap(),
    );
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
    let resources = activation.skills[0]
        .resources
        .as_ref()
        .expect("installed sibling resource metadata must cross the runtime boundary");
    assert_eq!(resources.resource_count, 2);
    assert_eq!(resources.kinds, vec!["other", "script"]);
    let session = prepared
        .skill_resources
        .as_ref()
        .expect("prepared turn must retain the host-only resource authority");
    let package = mycopilot_core::skills::SkillPackageUri::parse(&resources.root_uri).unwrap();
    let page = session
        .list(
            &package,
            &mycopilot_core::skills::SkillResourceListOptions::new(10).unwrap(),
        )
        .unwrap();
    assert_eq!(page.entries().len(), 2);
    let template_entry = page
        .entries()
        .iter()
        .find(|entry| entry.descriptor().path() == "templates/runtime.md")
        .expect("installed template must be indexed");
    let script_entry = page
        .entries()
        .iter()
        .find(|entry| entry.descriptor().path() == "scripts/fail.py")
        .expect("installed script must be indexed");
    let text = session
        .read_text(
            template_entry.uri(),
            mycopilot_core::skills::SkillResourceTextReadOptions::new(0, 1024).unwrap(),
        )
        .unwrap();
    assert_eq!(text.text(), "revision-bound resource marker");

    let mut materialization_input = prepared.agent_input.clone();
    materialization_input
        .context
        .as_mut()
        .unwrap()
        .permissions
        .write = AgentWritePermission::WorkspaceOnly;
    let service = AgentService::new(Arc::clone(&storage)).with_skills_service(Arc::clone(&skills));
    let request = AgentSkillMaterializationRequest {
        id: "materialize-runtime-template".to_string(),
        source_uri: template_entry.uri().to_string(),
        source_prefix: None,
        destination: "runtime.md".to_string(),
        approval_status: AgentApprovalStatus::Approved,
        reason: Some("exercise the exact revision materializer".to_string()),
    };
    let mut automatic_materialization_input = materialization_input.clone();
    automatic_materialization_input
        .context
        .as_mut()
        .unwrap()
        .permissions
        .patch = mycopilot_core::AgentPatchPermission::AutoApprove;
    let automatic_request = AgentSkillMaterializationRequest {
        id: "auto-materialize-runtime-template".to_string(),
        source_uri: template_entry.uri().to_string(),
        source_prefix: None,
        destination: "runtime-auto.md".to_string(),
        approval_status: AgentApprovalStatus::Approved,
        reason: Some("verify exact-once automatic materialization".to_string()),
    };
    inject_auto_action_audit_post_commit_failure(
        "run-auto-materialize-runtime-template",
        &automatic_request.id,
        "completed",
    );
    let automatic = service
        .execute_auto_approved_action(
            AutoApprovedActionContext::new(
                automatic_materialization_input.clone(),
                "run-auto-materialize-runtime-template".to_string(),
                Some("conversation-installed-skill".to_string()),
                Some("assistant-auto-materialize-runtime-template".to_string()),
                Some(Arc::clone(session)),
            ),
            AgentProposedAction::SkillMaterialization {
                materialization: automatic_request.clone(),
            },
            AgentCancellationToken::new(),
        )
        .unwrap();
    assert!(
        automatic.ok,
        "automatic materialization failed: {automatic:?}"
    );
    assert_eq!(
        fs::read_to_string(workspace.join("runtime-auto.md")).unwrap(),
        "revision-bound resource marker"
    );
    let automatic_run_id = "run-auto-materialize-runtime-template";
    let automatic_effect_id = pending_action_storage_id(automatic_run_id, &automatic_request.id);
    service.file_effects.restore_unsettled(
        None,
        Some("conversation-installed-skill"),
        automatic_run_id,
        &automatic_effect_id,
    );
    assert_eq!(
        service.unsettled_file_effect_ids_for_conversation("conversation-installed-skill"),
        vec![format!("{automatic_run_id}/{automatic_effect_id}")]
    );
    let replay = service
        .execute_auto_approved_action(
            AutoApprovedActionContext::new(
                automatic_materialization_input,
                automatic_run_id.to_string(),
                Some("conversation-installed-skill".to_string()),
                Some("assistant-auto-materialize-runtime-template".to_string()),
                Some(Arc::clone(session)),
            ),
            AgentProposedAction::SkillMaterialization {
                materialization: automatic_request,
            },
            AgentCancellationToken::new(),
        )
        .unwrap();
    assert_eq!(replay.call_id, automatic.call_id);
    assert_eq!(replay.ok, automatic.ok);
    assert_eq!(replay.result, automatic.result);
    assert!(service
        .unsettled_file_effect_ids_for_conversation("conversation-installed-skill")
        .is_empty());
    assert_eq!(
        storage
            .list_agent_tool_results_for_run(
                "run-auto-materialize-runtime-template",
                "skills_materialize_resource",
            )
            .unwrap()
            .len(),
        1
    );
    let first = service.execute_skill_materialization(
        &materialization_input,
        &request,
        Some(session.as_ref()),
    );
    assert!(first.ok, "materialization failed: {:?}", first.error);
    assert_eq!(
        fs::read_to_string(workspace.join("runtime.md")).unwrap(),
        "revision-bound resource marker"
    );
    let second = service.execute_skill_materialization(
        &materialization_input,
        &request,
        Some(session.as_ref()),
    );
    assert!(second.ok);
    assert_eq!(
        second
            .result
            .as_ref()
            .and_then(|result| result.get("status"))
            .and_then(Value::as_str),
        Some("already_applied")
    );

    let script_uri = script_entry.uri().clone();
    let preflight = mycopilot_core::skills::preflight_skill_python_script(
        session,
        &workspace,
        &script_uri,
        mycopilot_core::AgentSkillScriptInterpreter::Python3,
        &mycopilot_core::AgentSkillScriptRequirements::default(),
    )
    .unwrap();
    if let mycopilot_core::skills::SkillScriptPreflightOutcome::Ready { report, plan } = preflight {
        let mut script_input = materialization_input.clone();
        let permissions = &mut script_input.context.as_mut().unwrap().permissions;
        permissions.command = mycopilot_core::AgentCommandPermission::AutoApprove;
        permissions.command_safety = mycopilot_core::AgentCommandSafetyPolicy::Guarded;
        let script_request = mycopilot_core::AgentSkillScriptRequest {
            id: "run-installed-failing-script".to_string(),
            script_uri: script_uri.to_string(),
            skill_id: script_uri.package().skill_id().as_str().to_string(),
            skill_revision: script_uri.package().revision().as_str().to_string(),
            resource_path: script_uri.path().as_str().to_string(),
            resource_digest: plan.resource_digest().to_string(),
            interpreter: mycopilot_core::AgentSkillScriptInterpreter::Python3,
            args: Vec::new(),
            requirements: mycopilot_core::AgentSkillScriptRequirements::default(),
            preflight: report,
            timeout_ms: Some(2_000),
            approval_status: AgentApprovalStatus::Approved,
            reason: Some("verify failure result fidelity".to_string()),
        };

        let automatic = service.execute_skill_script(
            &script_input,
            &script_request,
            Some(session),
            CommandAuthorizationSource::Automatic,
            AgentCancellationToken::new(),
            None,
        );
        assert!(!automatic.ok);
        assert_eq!(
            automatic
                .result
                .as_ref()
                .and_then(|result| result.get("code"))
                .and_then(Value::as_str),
            Some("authorizationDenied")
        );

        script_input
            .context
            .as_mut()
            .unwrap()
            .permissions
            .command_safety = mycopilot_core::AgentCommandSafetyPolicy::FullAccess;
        script_input.context.as_mut().unwrap().permissions.read =
            mycopilot_core::AgentReadPermission::All;
        script_input.context.as_mut().unwrap().permissions.write = AgentWritePermission::All;
        let full_access_automatic = service.execute_skill_script(
            &script_input,
            &script_request,
            Some(session),
            CommandAuthorizationSource::Automatic,
            AgentCancellationToken::new(),
            None,
        );
        assert!(!full_access_automatic.ok);
        assert_eq!(
            full_access_automatic
                .result
                .as_ref()
                .and_then(|result| result.get("code"))
                .and_then(Value::as_str),
            Some("authorizationDenied")
        );
        let explicitly_approved = service.execute_skill_script(
            &script_input,
            &script_request,
            Some(session),
            CommandAuthorizationSource::ExplicitUser,
            AgentCancellationToken::new(),
            None,
        );
        assert!(!explicitly_approved.ok);
        let result = explicitly_approved
            .result
            .as_ref()
            .expect("non-zero scripts must retain their structured execution result");
        assert_eq!(result["exitCode"], 17);
        assert!(result["stdout"]
            .as_str()
            .unwrap()
            .contains("skill stdout marker"));
        assert!(result["stderr"]
            .as_str()
            .unwrap()
            .contains("skill stderr marker"));
        assert_eq!(result["errorCode"], "skill_script.nonzero_exit");
    }

    // Persisted approval continuations carry only id + exact revision. Prove
    // the Host restoration boundary reopens that immutable package after the
    // mutable receipt has moved to a newer revision and after it is removed.
    fs::write(
        local_skill.join("templates/runtime.md"),
        "new receipt resource marker",
    )
    .unwrap();
    let updated = installations
        .update_local_directory(&LocalSkillUpdateRequest::new(
            descriptor.id().clone(),
            descriptor.revision().clone(),
            &local_skill,
        ))
        .unwrap();
    assert_eq!(updated.outcome(), SkillInstallationOutcome::Updated);

    let restore_and_materialize_old = |destination: &str| {
        let restarted_skills = Arc::new(
            SkillsService::new()
                .with_installed_source(&store_root)
                .unwrap(),
        );
        let restarted = AgentService::new(Arc::clone(&storage))
            .with_skills_service(Arc::clone(&restarted_skills));
        let restored = restarted
            .restore_skill_resource_session(&prepared.agent_input)
            .unwrap()
            .expect("old exact resource grant must restore after restart");
        let request = AgentSkillMaterializationRequest {
            id: format!("materialize-{destination}"),
            source_uri: template_entry.uri().to_string(),
            source_prefix: None,
            destination: destination.to_string(),
            approval_status: AgentApprovalStatus::Approved,
            reason: Some("verify exact revision restoration".to_string()),
        };
        let result = restarted.execute_skill_materialization(
            &materialization_input,
            &request,
            Some(restored.as_ref()),
        );
        assert!(
            result.ok,
            "restored materialization failed: {:?}",
            result.error
        );
        assert_eq!(
            fs::read_to_string(workspace.join(destination)).unwrap(),
            "revision-bound resource marker"
        );
    };
    restore_and_materialize_old("runtime-after-update.md");

    let uninstalled = installations
        .uninstall(&SkillUninstallRequest::new(
            descriptor.id().clone(),
            updated.package_revision().unwrap().clone(),
        ))
        .unwrap();
    assert_eq!(uninstalled.outcome(), SkillInstallationOutcome::Uninstalled);
    restore_and_materialize_old("runtime-after-uninstall.md");

    assert_eq!(prepared.output.activated_skills.len(), 1);
    assert_eq!(
        prepared.output.activated_skills[0].source.kind,
        mycopilot_protocol_rs::SkillSourceKindDto::Installed
    );

    let public_output = serde_json::to_string(&prepared.output).unwrap();
    assert!(!public_output.contains(INSTRUCTION_MARKER));
    assert!(!public_output.contains(DESCRIPTION_MARKER));
    assert!(!public_output.contains(&source_text));
    assert!(!public_output.contains("revision-bound resource marker"));
    let persisted = storage
        .load_conversation("conversation-installed-skill")
        .unwrap()
        .unwrap();
    let persisted_json = serde_json::to_string(&persisted).unwrap();
    assert!(!persisted_json.contains(INSTRUCTION_MARKER));
    assert!(!persisted_json.contains(DESCRIPTION_MARKER));
    assert!(!persisted_json.contains(&source_text));
    assert!(!persisted_json.contains("revision-bound resource marker"));
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
