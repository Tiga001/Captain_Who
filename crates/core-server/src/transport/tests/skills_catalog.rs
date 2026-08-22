use super::*;

#[test]
fn skills_list_resolves_the_project_and_returns_camel_case_catalog() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().join("workspace");
    let skill_directory = workspace
        .join(".agents")
        .join("skills")
        .join("workspace-helper");
    fs::create_dir_all(&skill_directory).unwrap();
    fs::write(
        skill_directory.join("SKILL.md"),
        concat!(
            "---\n",
            "name: workspace-helper\n",
            "description: Exercise project-scoped Skill discovery.\n",
            "---\n",
            "# Instructions\n"
        ),
    )
    .unwrap();
    let broken_skill_directory = workspace.join(".agents").join("skills").join("broken");
    fs::create_dir(&broken_skill_directory).unwrap();
    fs::write(
        broken_skill_directory.join("SKILL.md"),
        "missing frontmatter",
    )
    .unwrap();
    let storage = Arc::new(StorageService::open(&temp.path().join("storage.sqlite")).unwrap());
    storage
        .save_project(ProjectRecord {
            id: "project-1".to_string(),
            name: "Workspace".to_string(),
            path: Some(workspace.to_string_lossy().into_owned()),
            created_at: 1,
            pinned_at: None,
        })
        .unwrap();
    let installed_store_root = temp.path().join("skills");
    let skills_service = SkillsService::new()
        .with_bundled_source()
        .and_then(|service| service.with_installed_source(&installed_store_root))
        .unwrap();
    let skill_installation_service = SkillInstallationService::new(&installed_store_root).unwrap();
    assert!(
        !installed_store_root.exists(),
        "registering the read-only source must not create its store"
    );
    let empty_installed_catalog = skills_service.list().unwrap();
    assert_eq!(empty_installed_catalog.skills().len(), 7);
    assert!(empty_installed_catalog.diagnostics().is_empty());
    assert!(!installed_store_root.exists());
    const INSTALLATION_ID: &str = "0190b0f2-7c50-7cc0-8b25-3bb80f08b334";
    write_installed_skill(
        &installed_store_root,
        INSTALLATION_ID,
        concat!(
            "---\n",
            "name: installed-helper\n",
            "description: Exercise installed Skill discovery.\n",
            "---\n",
            "# Instructions\n",
            "Run the installed helper workflow.\n"
        ),
    );
    let request = serde_json::from_value::<JsonRpcRequest>(json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": SKILLS_LIST_METHOD,
        "params": { "projectId": "project-1" }
    }))
    .unwrap();

    let response = handle_skills_request(
        &storage,
        &skills_service,
        &skill_installation_service,
        request,
    );

    assert_eq!(response["id"], 1);
    let skills = response["result"]["skills"].as_array().unwrap();
    assert_eq!(skills.len(), 9);
    assert_eq!(response["result"]["schemaVersion"], 4);
    let workspace_skill = skills
        .iter()
        .find(|skill| skill["id"] == "workspace:project-1:workspace-helper")
        .unwrap();
    let bundled_skill = skills
        .iter()
        .find(|skill| skill["id"] == "bundled:application:documents")
        .unwrap();
    let installed_skill = skills
        .iter()
        .find(|skill| skill["id"] == format!("installed:user:{INSTALLATION_ID}"))
        .unwrap();
    assert_eq!(workspace_skill["source"]["kind"], "workspace");
    assert_eq!(workspace_skill["source"]["id"], "workspace:project-1");
    assert_eq!(workspace_skill["trust"], "untrusted");
    assert_eq!(workspace_skill["activationScope"], "run");
    assert_eq!(
        workspace_skill["description"],
        "Exercise project-scoped Skill discovery."
    );
    assert_eq!(
        workspace_skill["location"],
        ".agents/skills/workspace-helper/SKILL.md"
    );
    assert!(workspace_skill.get("path").is_none());
    assert!(workspace_skill["revision"].is_string());
    assert_eq!(bundled_skill["source"]["kind"], "bundled");
    assert_eq!(bundled_skill["source"]["id"], "bundled:application");
    assert_eq!(bundled_skill["trust"], "application");
    assert_eq!(bundled_skill["activationScope"], "run");
    assert_eq!(bundled_skill["location"], "documents/SKILL.md");
    assert!(bundled_skill["revision"].is_string());
    assert_eq!(installed_skill["source"]["kind"], "installed");
    assert_eq!(installed_skill["source"]["id"], "installed:user");
    assert_eq!(installed_skill["trust"], "untrusted");
    assert_eq!(installed_skill["activationScope"], "run");
    assert!(installed_skill["location"]
        .as_str()
        .is_some_and(
            |location| location.starts_with("packages/v1/") && location.ends_with("/SKILL.md")
        ));
    assert!(installed_skill["revision"].is_string());
    assert!(response["result"]["catalogRevision"].is_string());
    assert_eq!(response["result"]["truncated"], false);
    assert_eq!(
        response["result"]["diagnostics"][0]["code"],
        "missingFrontmatter"
    );
    assert_eq!(response["result"]["diagnostics"][0]["severity"], "error");
    assert!(response["result"]["diagnostics"][0]["message"].is_string());
    assert!(response["result"]["diagnostics"][0]["location"].is_string());
}

#[test]
fn skills_list_without_a_project_returns_only_global_sources() {
    let temp = tempfile::tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&temp.path().join("storage.sqlite")).unwrap());
    let skills_service = SkillsService::new().with_bundled_source().unwrap();
    let skill_installation_service =
        SkillInstallationService::new(temp.path().join("skills")).unwrap();
    let request = serde_json::from_value::<JsonRpcRequest>(json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": SKILLS_LIST_METHOD,
        "params": {}
    }))
    .unwrap();

    let response = handle_skills_request(
        &storage,
        &skills_service,
        &skill_installation_service,
        request,
    );

    assert_eq!(response["result"]["schemaVersion"], 4);
    let skills = response["result"]["skills"].as_array().unwrap();
    assert_eq!(skills.len(), 7);
    assert!(skills
        .iter()
        .all(|skill| skill["source"]["kind"] == "bundled"));
}

#[test]
fn management_enablement_is_cas_protected_and_filters_only_the_picker() {
    let temp = tempfile::tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&temp.path().join("storage.sqlite")).unwrap());
    let skills_service = SkillsService::new().with_bundled_source().unwrap();
    let skill_installation_service =
        SkillInstallationService::new(temp.path().join("skills")).unwrap();
    let list_management = || {
        handle_skills_request(
            &storage,
            &skills_service,
            &skill_installation_service,
            serde_json::from_value::<JsonRpcRequest>(json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": SKILLS_LIST_MANAGEMENT_METHOD,
                "params": {}
            }))
            .unwrap(),
        )
    };

    let first = list_management();
    let first_skills = first["result"]["skills"].as_array().unwrap();
    assert_eq!(first_skills.len(), 7);
    let skill_id = first_skills[0]["id"].as_str().unwrap().to_string();
    let first_state = first_skills[0]["stateRevision"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(first_skills[0]["enabled"], true);
    assert_eq!(first_skills[0]["actions"]["canUninstall"], false);

    let disable = handle_skills_request(
        &storage,
        &skills_service,
        &skill_installation_service,
        serde_json::from_value::<JsonRpcRequest>(json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": SKILLS_SET_ENABLED_METHOD,
            "params": {
                "skillId": skill_id,
                "expectedStateRevision": first_state.clone(),
                "enabled": false
            }
        }))
        .unwrap(),
    );
    assert_eq!(disable["result"]["outcome"], "updated");
    assert_eq!(disable["result"]["enabled"], false);
    let disabled_state = disable["result"]["stateRevision"]
        .as_str()
        .unwrap()
        .to_string();

    let lost_response_retry = handle_skills_request(
        &storage,
        &skills_service,
        &skill_installation_service,
        serde_json::from_value::<JsonRpcRequest>(json!({
            "jsonrpc": "2.0",
            "id": 20,
            "method": SKILLS_SET_ENABLED_METHOD,
            "params": {
                "skillId": skill_id,
                "expectedStateRevision": first_state,
                "enabled": false
            }
        }))
        .unwrap(),
    );
    assert_eq!(lost_response_retry["result"]["outcome"], "alreadyCurrent");
    assert_eq!(
        lost_response_retry["result"]["stateRevision"],
        disabled_state
    );

    let picker = handle_skills_request(
        &storage,
        &skills_service,
        &skill_installation_service,
        serde_json::from_value::<JsonRpcRequest>(json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": SKILLS_LIST_METHOD,
            "params": {}
        }))
        .unwrap(),
    );
    let picker_skills = picker["result"]["skills"].as_array().unwrap();
    assert_eq!(picker_skills.len(), 6);
    assert!(picker_skills.iter().all(|skill| skill["id"] != skill_id));
    let management = list_management();
    let managed_skill = management["result"]["skills"]
        .as_array()
        .unwrap()
        .iter()
        .find(|skill| skill["id"] == skill_id)
        .unwrap();
    assert_eq!(managed_skill["enabled"], false);

    let idempotent = handle_skills_request(
        &storage,
        &skills_service,
        &skill_installation_service,
        serde_json::from_value::<JsonRpcRequest>(json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": SKILLS_SET_ENABLED_METHOD,
            "params": {
                "skillId": skill_id,
                "expectedStateRevision": disabled_state,
                "enabled": false
            }
        }))
        .unwrap(),
    );
    assert_eq!(idempotent["result"]["outcome"], "alreadyCurrent");

    let stale = handle_skills_request(
        &storage,
        &skills_service,
        &skill_installation_service,
        serde_json::from_value::<JsonRpcRequest>(json!({
            "jsonrpc": "2.0",
            "id": 5,
            "method": SKILLS_SET_ENABLED_METHOD,
            "params": {
                "skillId": skill_id,
                "expectedStateRevision": "stale",
                "enabled": true
            }
        }))
        .unwrap(),
    );
    assert_eq!(stale["error"]["code"], -32012);
    assert_eq!(stale["error"]["data"]["type"], "skillManagement");
    assert_eq!(stale["error"]["data"]["operation"], "setEnabled");
    assert_eq!(stale["error"]["data"]["code"], "stateConflict");
    assert_eq!(stale["error"]["data"]["recovery"], "refreshManagement");

    let reenabled = handle_skills_request(
        &storage,
        &skills_service,
        &skill_installation_service,
        serde_json::from_value::<JsonRpcRequest>(json!({
            "jsonrpc": "2.0",
            "id": 6,
            "method": SKILLS_SET_ENABLED_METHOD,
            "params": {
                "skillId": skill_id,
                "expectedStateRevision": disabled_state,
                "enabled": true
            }
        }))
        .unwrap(),
    );
    assert_eq!(reenabled["result"]["outcome"], "updated");
    assert_ne!(
        reenabled["result"]["stateRevision"], first_state,
        "monotonic enablement generation must prevent boolean-state ABA"
    );
}

#[test]
fn set_enabled_distinguishes_missing_and_project_scoped_skills() {
    let temp = tempfile::tempdir().unwrap();
    let storage = StorageService::open(&temp.path().join("storage.sqlite")).unwrap();
    let skills_service = SkillsService::new().with_bundled_source().unwrap();
    let skill_installation_service =
        SkillInstallationService::new(temp.path().join("skills")).unwrap();
    let invoke = |skill_id: &str| {
        handle_skills_request(
            &storage,
            &skills_service,
            &skill_installation_service,
            serde_json::from_value::<JsonRpcRequest>(json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": SKILLS_SET_ENABLED_METHOD,
                "params": {
                    "skillId": skill_id,
                    "expectedStateRevision": "unknown",
                    "enabled": false
                }
            }))
            .unwrap(),
        )
    };

    let missing = invoke("installed:user:01234567-89ab-4def-8123-456789abcdef");
    assert_eq!(missing["error"]["code"], SKILL_MANAGEMENT_ERROR_CODE);
    assert_eq!(missing["error"]["data"]["code"], "notFound");
    assert_eq!(missing["error"]["data"]["operation"], "setEnabled");

    let workspace = invoke("workspace:project-1:auditor");
    assert_eq!(workspace["error"]["code"], SKILL_MANAGEMENT_ERROR_CODE);
    assert_eq!(workspace["error"]["data"]["code"], "notManageable");
    assert_eq!(workspace["error"]["data"]["recovery"], "refreshManagement");
}

#[test]
fn skills_list_rejects_an_unknown_project_with_a_server_error() {
    let temp = tempfile::tempdir().unwrap();
    let storage = StorageService::open(&temp.path().join("storage.sqlite")).unwrap();
    let skill_installation_service =
        SkillInstallationService::new(temp.path().join("skills")).unwrap();
    let request = serde_json::from_value::<JsonRpcRequest>(json!({
        "jsonrpc": "2.0",
        "id": "skills-request",
        "method": SKILLS_LIST_METHOD,
        "params": { "projectId": "missing" }
    }))
    .unwrap();

    let response = handle_skills_request(
        &storage,
        &SkillsService::new(),
        &skill_installation_service,
        request,
    );

    assert_eq!(response["id"], "skills-request");
    assert_eq!(response["error"]["code"], -32000);
    assert_eq!(
        response["error"]["message"],
        "The selected project no longer exists."
    );
}
