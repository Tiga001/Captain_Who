//! Backend vertical tests for application-managed Skill installation.

use crate::adapters::skills_adapter;
use crate::*;
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

async fn receive_rpc_response(
    receiver: &mut mpsc::UnboundedReceiver<Value>,
    expected_id: i64,
    notifications: &mut Vec<Value>,
) -> Value {
    loop {
        let message = tokio::time::timeout(Duration::from_secs(5), receiver.recv())
            .await
            .expect("Skill RPC response timed out")
            .expect("Skill RPC outbound channel closed");
        if message["id"] == expected_id {
            return message;
        }
        notifications.push(message);
    }
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
        .save_project(ProjectRecord::with_primary_folder(
            "project-1".to_string(),
            "Workspace".to_string(),
            workspace.to_string_lossy().into_owned(),
            1,
        ))
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
    storage
        .set_skill_enablement_override(&skill_id, false)
        .unwrap();
    let disabled_before_replay = storage
        .load_skill_enablement_states(std::slice::from_ref(&skill_id))
        .unwrap()[&skill_id];
    let retried_install = call_skill_rpc(
        &storage,
        &catalog,
        &installations,
        2,
        SKILLS_INSTALL_LOCAL_METHOD,
        install_params,
    );
    assert_eq!(retried_install["result"]["outcome"], "alreadyInstalled");
    assert_eq!(
        storage
            .load_skill_enablement_states(std::slice::from_ref(&skill_id))
            .unwrap()[&skill_id],
        disabled_before_replay,
        "a delayed install replay must not overwrite a newer user preference"
    );
    storage
        .set_skill_enablement_override(&skill_id, true)
        .unwrap();

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
    assert_eq!(
        stale.code,
        mycopilot_protocol_rs::SkillActivationErrorCodeDto::Stale
    );
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
    storage
        .set_skill_enablement_override(&skill_id, false)
        .unwrap();
    let management = call_skill_rpc(
        &storage,
        &catalog,
        &installations,
        8,
        SKILLS_LIST_MANAGEMENT_METHOD,
        json!({}),
    );
    let second_installation_revision = management["result"]["skills"]
        .as_array()
        .unwrap()
        .iter()
        .find(|skill| skill["id"] == skill_id)
        .and_then(|skill| skill["installationRevision"].as_str())
        .unwrap()
        .to_string();

    let package_revision_uninstall = call_skill_rpc(
        &storage,
        &catalog,
        &installations,
        9,
        SKILLS_UNINSTALL_METHOD,
        json!({
            "skillId": skill_id,
            "expectedRevision": second_revision,
        }),
    );
    assert_eq!(package_revision_uninstall["error"]["code"], -32602);

    let uninstall_params = json!({
        "skillId": skill_id,
        "expectedRevision": second_installation_revision,
    });
    let uninstalled = call_skill_rpc(
        &storage,
        &catalog,
        &installations,
        10,
        SKILLS_UNINSTALL_METHOD,
        uninstall_params.clone(),
    );
    assert_eq!(uninstalled["result"]["outcome"], "uninstalled");
    let state_after_uninstall = storage
        .load_skill_enablement_states(std::slice::from_ref(&skill_id))
        .unwrap()[&skill_id];
    assert!(state_after_uninstall.enabled);
    assert_eq!(state_after_uninstall.generation, 0);
    let retried_uninstall = call_skill_rpc(
        &storage,
        &catalog,
        &installations,
        11,
        SKILLS_UNINSTALL_METHOD,
        uninstall_params,
    );
    assert_eq!(retried_uninstall["result"]["outcome"], "alreadyAbsent");
    assert_eq!(
        storage
            .load_skill_enablement_states(std::slice::from_ref(&skill_id))
            .unwrap()[&skill_id],
        state_after_uninstall,
        "an idempotent uninstall retry must not recreate derived state"
    );
    let after_uninstall = call_skill_rpc(
        &storage,
        &catalog,
        &installations,
        12,
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
    assert_eq!(
        unavailable.code,
        mycopilot_protocol_rs::SkillActivationErrorCodeDto::NotFound
    );
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
        .save_project(ProjectRecord::with_primary_folder(
            "project-1".to_string(),
            "Workspace".to_string(),
            workspace.to_string_lossy().into_owned(),
            1,
        ))
        .unwrap();
    let catalog = Arc::new(
        SkillsService::new()
            .with_installed_source(&store_root)
            .unwrap(),
    );
    let installations = Arc::new(SkillInstallationService::new(&store_root).unwrap());
    let agent_service = AgentService::new(Arc::clone(&storage));
    let (outbound_tx, mut outbound_rx) = mpsc::unbounded_channel();
    let (image_artifact_outbound_tx, _image_artifact_outbound_rx) =
        mpsc::channel(DEFAULT_MAX_CONCURRENT_IMAGE_ARTIFACT_READS);
    let git_dispatcher = GitDispatcher::new(outbound_tx.clone());
    let skills_dispatcher = SkillsDispatcher::new(outbound_tx.clone());
    let image_generation_dispatcher =
        ImageGenerationConfigurationDispatcher::new(outbound_tx.clone());
    let dispatchers = RequestDispatchers {
        git: &git_dispatcher,
        skills: &skills_dispatcher,
        skill_acquisition: &skills_dispatcher,
        image_generation_configuration: &image_generation_dispatcher,
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
        test_core_request_services(storage),
        &agent_service,
        SkillServices {
            catalog,
            installations,
            workflow: Arc::new(SkillInstallationWorkflow::new(
                SkillInstallationService::new(&store_root).unwrap(),
            )),
            source_resolution: Arc::new(SkillSourceResolutionService::new()),
        },
        Arc::new(GitReviewService::new()),
        &dispatchers,
        RequestOutbounds {
            normal: &outbound_tx,
            image_artifact: &image_artifact_outbound_tx,
        },
    )
    .await
    .unwrap();
    let mut notifications = Vec::new();
    let install = receive_rpc_response(&mut outbound_rx, 11, &mut notifications).await;
    let list = receive_rpc_response(&mut outbound_rx, 12, &mut notifications).await;

    assert!(shutdown_id.is_none());
    assert_eq!(install["id"], 11);
    assert_eq!(install["result"]["outcome"], "installed");
    assert_eq!(list["id"], 12);
    assert!(list["result"]["skills"]
        .as_array()
        .unwrap()
        .iter()
        .any(|skill| skill["id"] == install["result"]["skillId"]));
    assert_eq!(notifications.len(), 1);
    assert_eq!(
        notifications[0]["method"],
        SKILLS_CHANGED_NOTIFICATION_METHOD
    );
    assert_eq!(notifications[0]["params"]["reason"], "installed");
    git_dispatcher.shutdown().await.unwrap();
    skills_dispatcher.shutdown().await.unwrap();
    image_generation_dispatcher.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_phase_rpc_runs_install_update_activation_and_uninstall_end_to_end() {
    const INSTALL_PREPARATION_ID: &str = "11111111-1111-4111-8111-111111111111";
    const UPDATE_PREPARATION_ID: &str = "22222222-2222-4222-8222-222222222222";

    let fixture = tempfile::tempdir().unwrap();
    let local_skill = fixture.path().join("local-skill");
    let store_root = fixture.path().join("skills");
    write_local_skill(&local_skill, "TWO_PHASE_VERSION_ONE");
    fs::create_dir_all(local_skill.join("references")).unwrap();
    fs::write(
        local_skill.join("references").join("evidence.md"),
        "immutable evidence",
    )
    .unwrap();
    fs::create_dir_all(local_skill.join("scripts")).unwrap();
    fs::write(
        local_skill.join("scripts").join("check.sh"),
        "#!/bin/sh\nexit 0\n",
    )
    .unwrap();

    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let catalog = Arc::new(
        SkillsService::new()
            .with_installed_source(&store_root)
            .unwrap(),
    );
    let installations = Arc::new(SkillInstallationService::new(&store_root).unwrap());
    let workflow = Arc::new(SkillInstallationWorkflow::new(
        SkillInstallationService::new(&store_root).unwrap(),
    ));
    let agent_service = AgentService::new(Arc::clone(&storage));
    let (outbound_tx, mut outbound_rx) = mpsc::unbounded_channel();
    let (image_artifact_outbound_tx, _image_artifact_outbound_rx) =
        mpsc::channel(DEFAULT_MAX_CONCURRENT_IMAGE_ARTIFACT_READS);
    let git_dispatcher = GitDispatcher::new(outbound_tx.clone());
    let skills_dispatcher = SkillsDispatcher::new(outbound_tx.clone());
    let acquisition_dispatcher = SkillsDispatcher::new(outbound_tx.clone());
    let image_generation_dispatcher =
        ImageGenerationConfigurationDispatcher::new(outbound_tx.clone());
    let dispatchers = RequestDispatchers {
        git: &git_dispatcher,
        skills: &skills_dispatcher,
        skill_acquisition: &acquisition_dispatcher,
        image_generation_configuration: &image_generation_dispatcher,
    };
    let (mut input_writer, input_reader) = io::duplex(64 * 1024);
    let server = run_request_loop(
        BufReader::new(input_reader),
        test_core_request_services(Arc::clone(&storage)),
        &agent_service,
        SkillServices {
            catalog: Arc::clone(&catalog),
            installations,
            workflow,
            source_resolution: Arc::new(SkillSourceResolutionService::new()),
        },
        Arc::new(GitReviewService::new()),
        &dispatchers,
        RequestOutbounds {
            normal: &outbound_tx,
            image_artifact: &image_artifact_outbound_tx,
        },
    );
    let local_directory = local_skill.to_string_lossy().into_owned();
    let client = async {
        let mut notifications = Vec::new();
        let inspect_install = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": SKILLS_INSPECT_INSTALLATION_METHOD,
            "params": {
                "preparationId": INSTALL_PREPARATION_ID,
                "intent": { "operation": "install" },
                "source": { "kind": "localDirectory", "directory": local_directory }
            }
        });
        input_writer
            .write_all(format!("{inspect_install}\n").as_bytes())
            .await
            .unwrap();
        let install_preview = receive_rpc_response(&mut outbound_rx, 1, &mut notifications).await;
        assert_eq!(install_preview["result"]["operation"], "install");
        assert_eq!(install_preview["result"]["package"]["formatVersion"], 2);
        assert_eq!(install_preview["result"]["package"]["fileCount"], 3);
        assert_eq!(
            install_preview["result"]["source"]["kind"],
            "localDirectory"
        );
        assert_eq!(
            install_preview["result"]["compatibility"]["status"],
            "compatibleWithWarnings"
        );
        assert_eq!(
            install_preview["result"]["compatibility"]["issues"][0]["id"],
            "containsScripts"
        );

        let commit_install = json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": SKILLS_COMMIT_INSTALLATION_METHOD,
            "params": {
                "preparationId": INSTALL_PREPARATION_ID,
                "previewRevision": install_preview["result"]["previewRevision"],
                "acceptedIssueIds": []
            }
        });
        input_writer
            .write_all(format!("{commit_install}\n").as_bytes())
            .await
            .unwrap();
        let acknowledgement_required =
            receive_rpc_response(&mut outbound_rx, 2, &mut notifications).await;
        assert_eq!(acknowledgement_required["error"]["code"], -32011);
        assert_eq!(
            acknowledgement_required["error"]["data"]["code"],
            "acknowledgementRequired"
        );

        let accepted_commit = json!({
            "jsonrpc": "2.0",
            "id": 8,
            "method": SKILLS_COMMIT_INSTALLATION_METHOD,
            "params": {
                "preparationId": INSTALL_PREPARATION_ID,
                "previewRevision": install_preview["result"]["previewRevision"],
                "acceptedIssueIds": ["containsScripts"]
            }
        });
        input_writer
            .write_all(format!("{accepted_commit}\n").as_bytes())
            .await
            .unwrap();
        let install = receive_rpc_response(&mut outbound_rx, 8, &mut notifications).await;
        assert_eq!(install["result"]["outcome"], "installed");
        assert_ne!(
            install["result"]["installationRevision"],
            install["result"]["packageRevision"]
        );
        assert!(install["result"]["installationRevision"]
            .as_str()
            .unwrap()
            .starts_with("skill-installation-sha256-v1:"));

        write_local_skill(&local_skill, "TWO_PHASE_VERSION_TWO");
        let inspect_update = json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": SKILLS_INSPECT_INSTALLATION_METHOD,
            "params": {
                "preparationId": UPDATE_PREPARATION_ID,
                "intent": {
                    "operation": "update",
                    "skillId": install["result"]["skillId"],
                    "expectedInstallationRevision": install["result"]["installationRevision"]
                },
                "source": { "kind": "localDirectory", "directory": local_directory }
            }
        });
        input_writer
            .write_all(format!("{inspect_update}\n").as_bytes())
            .await
            .unwrap();
        let update_preview = receive_rpc_response(&mut outbound_rx, 3, &mut notifications).await;
        assert_eq!(update_preview["result"]["changes"]["content"], "changed");

        let commit_update = json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": SKILLS_COMMIT_INSTALLATION_METHOD,
            "params": {
                "preparationId": UPDATE_PREPARATION_ID,
                "previewRevision": update_preview["result"]["previewRevision"],
                "acceptedIssueIds": ["containsScripts"]
            }
        });
        input_writer
            .write_all(format!("{commit_update}\n").as_bytes())
            .await
            .unwrap();
        let update = receive_rpc_response(&mut outbound_rx, 4, &mut notifications).await;
        assert_eq!(update["result"]["outcome"], "updated");

        let list = json!({
            "jsonrpc": "2.0",
            "id": 5,
            "method": SKILLS_LIST_METHOD,
            "params": {}
        });
        input_writer
            .write_all(format!("{list}\n").as_bytes())
            .await
            .unwrap();
        let listed = receive_rpc_response(&mut outbound_rx, 5, &mut notifications).await;
        assert_eq!(listed["result"]["skills"].as_array().unwrap().len(), 1);
        assert_eq!(
            listed["result"]["skills"][0]["revision"],
            update["result"]["packageRevision"]
        );
        let activation = skills_adapter::activate_selected_skills(
            &storage,
            &catalog,
            None,
            &[mycopilot_protocol_rs::SkillSelectionDto {
                id: listed["result"]["skills"][0]["id"]
                    .as_str()
                    .unwrap()
                    .to_string(),
                revision: listed["result"]["skills"][0]["revision"]
                    .as_str()
                    .unwrap()
                    .to_string(),
            }],
        )
        .unwrap();
        assert!(activation.runtime.unwrap().skills[0]
            .instructions
            .contains("TWO_PHASE_VERSION_TWO"));

        let list_management = json!({
            "jsonrpc": "2.0",
            "id": 6,
            "method": SKILLS_LIST_MANAGEMENT_METHOD,
            "params": {}
        });
        input_writer
            .write_all(format!("{list_management}\n").as_bytes())
            .await
            .unwrap();
        let management = receive_rpc_response(&mut outbound_rx, 6, &mut notifications).await;
        assert_eq!(management["result"]["skills"].as_array().unwrap().len(), 1);
        assert_eq!(
            management["result"]["skills"][0]["actions"]["canUpdate"],
            false
        );
        assert_eq!(
            management["result"]["skills"][0]["installationRevision"],
            update["result"]["installationRevision"]
        );

        let stale_uninstall = json!({
            "jsonrpc": "2.0",
            "id": 9,
            "method": SKILLS_UNINSTALL_METHOD,
            "params": {
                "skillId": update["result"]["skillId"],
                "expectedRevision": install["result"]["installationRevision"]
            }
        });
        input_writer
            .write_all(format!("{stale_uninstall}\n").as_bytes())
            .await
            .unwrap();
        let stale = receive_rpc_response(&mut outbound_rx, 9, &mut notifications).await;
        assert_eq!(stale["error"]["code"], SKILL_INSTALLATION_ERROR_CODE);
        assert_eq!(stale["error"]["data"]["code"], "revisionConflict");
        assert_eq!(stale["error"]["data"]["recovery"], "refreshCatalog");
        assert_eq!(
            stale["error"]["data"]["expectedRevision"],
            install["result"]["installationRevision"]
        );
        assert_eq!(
            stale["error"]["data"]["actualRevision"],
            update["result"]["installationRevision"]
        );
        assert_eq!(stale["error"]["data"]["commitMayHaveSucceeded"], false);

        let uninstall = json!({
            "jsonrpc": "2.0",
            "id": 7,
            "method": SKILLS_UNINSTALL_METHOD,
            "params": {
                "skillId": update["result"]["skillId"],
                "expectedRevision": update["result"]["installationRevision"]
            }
        });
        input_writer
            .write_all(format!("{uninstall}\n").as_bytes())
            .await
            .unwrap();
        let removed = receive_rpc_response(&mut outbound_rx, 7, &mut notifications).await;
        assert_eq!(removed["result"]["outcome"], "uninstalled");
        drop(input_writer);
        (listed, notifications)
    };

    let (server_result, (listed, notifications)) = tokio::join!(server, client);
    assert!(server_result.unwrap().is_none());
    assert_eq!(
        notifications
            .iter()
            .filter_map(|message| message["params"]["reason"].as_str())
            .collect::<Vec<_>>(),
        vec!["installed", "updated", "uninstalled"]
    );

    let descriptor = catalog.list().unwrap();
    assert!(descriptor.skills().is_empty());
    assert_eq!(listed["result"]["skills"][0]["source"]["kind"], "installed");

    git_dispatcher.shutdown().await.unwrap();
    skills_dispatcher.shutdown().await.unwrap();
    acquisition_dispatcher.shutdown().await.unwrap();
    image_generation_dispatcher.shutdown().await.unwrap();
}
