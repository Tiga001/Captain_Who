//! Backend vertical tests for application-managed Skill installation.

use super::*;
use std::fs;

const INSTALLATION_ID: &str = "0190b0f2-7c50-7cc0-8b25-3bb80f08b334";

fn write_local_skill(directory: &std::path::Path, marker: &str) {
    fs::create_dir_all(directory).unwrap();
    fs::write(
        directory.join("SKILL.md"),
        format!(
            concat!(
                "---\n",
                "name: local-lifecycle-auditor\n",
                "description: Exercise the installed Skill lifecycle.\n",
                "---\n",
                "# Instructions\n",
                "{}\n"
            ),
            marker
        ),
    )
    .unwrap();
}

fn skill_rpc_request(id: i64, method: &str, params: Value) -> JsonRpcRequest {
    serde_json::from_value(json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
    }))
    .unwrap()
}

fn call_skill_rpc(
    storage: &StorageService,
    catalog: &SkillsService,
    installations: &SkillInstallationService,
    id: i64,
    method: &str,
    params: Value,
) -> Value {
    handle_skills_request(
        storage,
        catalog,
        installations,
        skill_rpc_request(id, method, params),
    )
}

#[test]
fn local_skill_installation_service_runs_the_complete_backend_lifecycle() {
    let fixture = tempfile::tempdir().unwrap();
    let workspace = fixture.path().join("workspace");
    let local_skill = fixture.path().join("local-skill");
    let store_root = fixture.path().join("skills");
    fs::create_dir(&workspace).unwrap();
    write_local_skill(&local_skill, "LIFECYCLE_VERSION_ONE");

    let storage = StorageService::open(&fixture.path().join("storage.sqlite")).unwrap();
    storage
        .save_project(ProjectRecord {
            id: "project-1".to_string(),
            name: "Workspace".to_string(),
            path: Some(workspace.to_string_lossy().into_owned()),
            created_at: 1,
            pinned_at: None,
        })
        .unwrap();
    let catalog = SkillsService::new()
        .with_installed_source(&store_root)
        .unwrap();
    let installations = SkillInstallationService::new(&store_root).unwrap();
    let install_params = json!({
        "installationId": INSTALLATION_ID,
        "directory": local_skill.to_string_lossy(),
    });

    let installed = call_skill_rpc(
        &storage,
        &catalog,
        &installations,
        1,
        SKILLS_INSTALL_LOCAL_METHOD,
        install_params.clone(),
    );
    assert_eq!(installed["result"]["schemaVersion"], 1);
    assert_eq!(installed["result"]["outcome"], "installed");
    assert_eq!(installed["result"]["installationId"], INSTALLATION_ID);
    let skill_id = installed["result"]["skillId"].as_str().unwrap().to_string();
    let first_revision = installed["result"]["revision"]
        .as_str()
        .unwrap()
        .to_string();
    let retried_install = call_skill_rpc(
        &storage,
        &catalog,
        &installations,
        2,
        SKILLS_INSTALL_LOCAL_METHOD,
        install_params,
    );
    assert_eq!(retried_install["result"]["outcome"], "alreadyInstalled");

    let listed = call_skill_rpc(
        &storage,
        &catalog,
        &installations,
        3,
        SKILLS_LIST_METHOD,
        json!({ "projectId": "project-1" }),
    );
    let listed_skill = listed["result"]["skills"]
        .as_array()
        .unwrap()
        .iter()
        .find(|skill| skill["id"] == skill_id)
        .unwrap();
    assert_eq!(listed_skill["revision"], first_revision);
    let first_activation = skills_adapter::activate_workspace(
        &catalog,
        "project-1",
        &workspace,
        &[mycopilot_protocol_rs::SkillSelectionDto {
            id: skill_id.clone(),
            revision: first_revision.clone(),
        }],
    )
    .unwrap();
    assert!(first_activation.runtime.unwrap().skills[0]
        .instructions
        .contains("LIFECYCLE_VERSION_ONE"));

    write_local_skill(&local_skill, "LIFECYCLE_VERSION_TWO");
    let conflict = call_skill_rpc(
        &storage,
        &catalog,
        &installations,
        4,
        SKILLS_UPDATE_LOCAL_METHOD,
        json!({
            "skillId": skill_id,
            "expectedRevision": "not-the-current-revision",
            "directory": local_skill.to_string_lossy(),
        }),
    );
    assert_eq!(conflict["error"]["code"], SKILL_INSTALLATION_ERROR_CODE);
    assert_eq!(conflict["error"]["data"]["code"], "revisionConflict");
    assert_eq!(conflict["error"]["data"]["actualRevision"], first_revision);
    assert_eq!(conflict["error"]["data"]["commitMayHaveSucceeded"], false);

    let update_params = json!({
        "skillId": skill_id,
        "expectedRevision": first_revision,
        "directory": local_skill.to_string_lossy(),
    });
    let updated = call_skill_rpc(
        &storage,
        &catalog,
        &installations,
        5,
        SKILLS_UPDATE_LOCAL_METHOD,
        update_params.clone(),
    );
    assert_eq!(updated["result"]["outcome"], "updated");
    let second_revision = updated["result"]["revision"].as_str().unwrap().to_string();
    assert_ne!(second_revision, first_revision);
    let retried_update = call_skill_rpc(
        &storage,
        &catalog,
        &installations,
        6,
        SKILLS_UPDATE_LOCAL_METHOD,
        update_params,
    );
    assert_eq!(retried_update["result"]["outcome"], "alreadyCurrent");

    let stale = skills_adapter::activate_workspace(
        &catalog,
        "project-1",
        &workspace,
        &[mycopilot_protocol_rs::SkillSelectionDto {
            id: skill_id.clone(),
            revision: first_revision,
        }],
    )
    .unwrap_err()
    .into_data();
    assert_eq!(stale.code, "stale");
    let relisted = call_skill_rpc(
        &storage,
        &catalog,
        &installations,
        7,
        SKILLS_LIST_METHOD,
        json!({ "projectId": "project-1" }),
    );
    let relisted_skill = relisted["result"]["skills"]
        .as_array()
        .unwrap()
        .iter()
        .find(|skill| skill["id"] == skill_id)
        .unwrap();
    assert_eq!(relisted_skill["revision"], second_revision);
    let second_activation = skills_adapter::activate_workspace(
        &catalog,
        "project-1",
        &workspace,
        &[mycopilot_protocol_rs::SkillSelectionDto {
            id: skill_id.clone(),
            revision: second_revision.clone(),
        }],
    )
    .unwrap();
    assert!(second_activation.runtime.unwrap().skills[0]
        .instructions
        .contains("LIFECYCLE_VERSION_TWO"));

    let uninstall_params = json!({
        "skillId": skill_id,
        "expectedRevision": second_revision,
    });
    let uninstalled = call_skill_rpc(
        &storage,
        &catalog,
        &installations,
        8,
        SKILLS_UNINSTALL_METHOD,
        uninstall_params.clone(),
    );
    assert_eq!(uninstalled["result"]["outcome"], "uninstalled");
    let retried_uninstall = call_skill_rpc(
        &storage,
        &catalog,
        &installations,
        9,
        SKILLS_UNINSTALL_METHOD,
        uninstall_params,
    );
    assert_eq!(retried_uninstall["result"]["outcome"], "alreadyAbsent");
    let after_uninstall = call_skill_rpc(
        &storage,
        &catalog,
        &installations,
        10,
        SKILLS_LIST_METHOD,
        json!({ "projectId": "project-1" }),
    );
    assert!(after_uninstall["result"]["skills"]
        .as_array()
        .unwrap()
        .iter()
        .all(|skill| skill["id"] != skill_id));
    let unavailable = skills_adapter::activate_workspace(
        &catalog,
        "project-1",
        &workspace,
        &[mycopilot_protocol_rs::SkillSelectionDto {
            id: skill_id,
            revision: second_revision,
        }],
    )
    .unwrap_err()
    .into_data();
    assert_eq!(unavailable.code, "notFound");
}

#[test]
fn local_skill_installation_rejects_bad_params_and_preserves_safe_diagnostics() {
    let fixture = tempfile::tempdir().unwrap();
    let storage = StorageService::open(&fixture.path().join("storage.sqlite")).unwrap();
    let catalog = SkillsService::new();
    let installations = SkillInstallationService::new(fixture.path().join("skills")).unwrap();

    let relative = call_skill_rpc(
        &storage,
        &catalog,
        &installations,
        1,
        SKILLS_INSTALL_LOCAL_METHOD,
        json!({
            "installationId": INSTALLATION_ID,
            "directory": "relative/skill",
        }),
    );
    assert_eq!(relative["error"]["code"], -32602);

    let unknown_field = call_skill_rpc(
        &storage,
        &catalog,
        &installations,
        2,
        SKILLS_INSTALL_LOCAL_METHOD,
        json!({
            "installationId": INSTALLATION_ID,
            "directory": fixture.path().join("missing").to_string_lossy(),
            "managedRoot": fixture.path().join("attacker-controlled"),
        }),
    );
    assert_eq!(unknown_field["error"]["code"], -32602);

    let missing_directory = fixture.path().join("missing");
    let preparation = call_skill_rpc(
        &storage,
        &catalog,
        &installations,
        3,
        SKILLS_INSTALL_LOCAL_METHOD,
        json!({
            "installationId": INSTALLATION_ID,
            "directory": missing_directory.to_string_lossy(),
        }),
    );
    assert_eq!(preparation["error"]["code"], SKILL_INSTALLATION_ERROR_CODE);
    assert_eq!(preparation["error"]["data"]["type"], "skillInstallation");
    assert_eq!(preparation["error"]["data"]["code"], "preparationFailed");
    assert_eq!(
        preparation["error"]["data"]["diagnosticCode"],
        "invalidRoot"
    );
    assert_eq!(preparation["error"]["data"]["recovery"], "fixLocalSource");
    assert!(!preparation
        .to_string()
        .contains(&missing_directory.to_string_lossy().to_string()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn request_loop_serializes_install_before_the_following_catalog_read() {
    let fixture = tempfile::tempdir().unwrap();
    let workspace = fixture.path().join("workspace");
    let local_skill = fixture.path().join("local-skill");
    let store_root = fixture.path().join("skills");
    fs::create_dir(&workspace).unwrap();
    write_local_skill(&local_skill, "DISPATCHED_INSTALL_MARKER");
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    storage
        .save_project(ProjectRecord {
            id: "project-1".to_string(),
            name: "Workspace".to_string(),
            path: Some(workspace.to_string_lossy().into_owned()),
            created_at: 1,
            pinned_at: None,
        })
        .unwrap();
    let catalog = Arc::new(
        SkillsService::new()
            .with_installed_source(&store_root)
            .unwrap(),
    );
    let installations = Arc::new(SkillInstallationService::new(&store_root).unwrap());
    let agent_service = AgentService::new(Arc::clone(&storage));
    let (outbound_tx, mut outbound_rx) = mpsc::unbounded_channel();
    let git_dispatcher = GitDispatcher::new(outbound_tx.clone());
    let skills_dispatcher = SkillsDispatcher::new(outbound_tx.clone());
    let dispatchers = RequestDispatchers {
        git: &git_dispatcher,
        skills: &skills_dispatcher,
    };
    let input = format!(
        "{}\n{}\n",
        json!({
            "jsonrpc": "2.0",
            "id": 11,
            "method": SKILLS_INSTALL_LOCAL_METHOD,
            "params": {
                "installationId": INSTALLATION_ID,
                "directory": local_skill.to_string_lossy(),
            },
        }),
        json!({
            "jsonrpc": "2.0",
            "id": 12,
            "method": SKILLS_LIST_METHOD,
            "params": { "projectId": "project-1" },
        }),
    );

    let shutdown_id = run_request_loop(
        BufReader::new(input.as_bytes()),
        storage,
        &agent_service,
        SkillServices {
            catalog,
            installations,
        },
        Arc::new(GitReviewService::new()),
        &dispatchers,
        &outbound_tx,
    )
    .await
    .unwrap();
    let install = tokio::time::timeout(Duration::from_secs(2), outbound_rx.recv())
        .await
        .expect("install must complete")
        .expect("install must produce a response");
    let list = tokio::time::timeout(Duration::from_secs(2), outbound_rx.recv())
        .await
        .expect("list must complete")
        .expect("list must produce a response");

    assert!(shutdown_id.is_none());
    assert_eq!(install["id"], 11);
    assert_eq!(install["result"]["outcome"], "installed");
    assert_eq!(list["id"], 12);
    assert!(list["result"]["skills"]
        .as_array()
        .unwrap()
        .iter()
        .any(|skill| skill["id"] == install["result"]["skillId"]));
    git_dispatcher.shutdown().await.unwrap();
    skills_dispatcher.shutdown().await.unwrap();
}
