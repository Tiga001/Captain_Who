use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn request_loop_routes_skills_list_through_the_bounded_dispatcher() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().join("workspace");
    let skill_directory = workspace.join(".agents").join("skills").join("auditor");
    fs::create_dir_all(&skill_directory).unwrap();
    fs::write(
        skill_directory.join("SKILL.md"),
        "---\nname: auditor\ndescription: Audit a repository.\n---\n# Instructions\n",
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
    let agent_service = AgentService::new(Arc::clone(&storage));
    let skill_installation_service =
        Arc::new(SkillInstallationService::new(temp.path().join("skills")).unwrap());
    let (outbound_tx, mut outbound_rx) = mpsc::unbounded_channel();
    let git_dispatcher = GitDispatcher::new(outbound_tx.clone());
    let skills_dispatcher = SkillsDispatcher::new(outbound_tx.clone());
    let dispatchers = RequestDispatchers {
        git: &git_dispatcher,
        skills: &skills_dispatcher,
        skill_acquisition: &skills_dispatcher,
    };
    let input = concat!(
        "{\"jsonrpc\":\"2.0\",\"id\":7,\"method\":\"skills.list\",",
        "\"params\":{\"projectId\":\"project-1\"}}\n"
    );

    let shutdown_id = run_request_loop(
        BufReader::new(input.as_bytes()),
        storage,
        &agent_service,
        SkillServices {
            catalog: Arc::new(SkillsService::new()),
            installations: skill_installation_service,
            workflow: Arc::new(SkillInstallationWorkflow::new(
                SkillInstallationService::new(temp.path().join("skills")).unwrap(),
            )),
            source_resolution: Arc::new(SkillSourceResolutionService::new()),
        },
        Arc::new(GitReviewService::new()),
        &dispatchers,
        &outbound_tx,
    )
    .await
    .unwrap();
    let response = tokio::time::timeout(Duration::from_secs(2), outbound_rx.recv())
        .await
        .expect("skills.list must complete")
        .expect("skills.list must produce a response");

    assert!(shutdown_id.is_none());
    assert_eq!(response["id"], 7);
    assert_eq!(
        response["result"]["skills"][0]["id"],
        "workspace:project-1:auditor"
    );
    git_dispatcher.shutdown().await.unwrap();
    skills_dispatcher.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn changed_enablement_emits_one_invalidation_notification() {
    let temp = tempfile::tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&temp.path().join("storage.sqlite")).unwrap());
    let skills_service = Arc::new(SkillsService::new().with_bundled_source().unwrap());
    let catalog = skills_service.list().unwrap();
    let skill_store = temp.path().join("skills");
    let installation_service = SkillInstallationService::new(&skill_store).unwrap();
    let installation_workflow =
        SkillInstallationWorkflow::new(SkillInstallationService::new(&skill_store).unwrap());
    let management = management_response(
        &storage,
        &catalog,
        &installation_service,
        Some(&installation_workflow),
    )
    .unwrap();
    let item = management.skills.first().unwrap();
    let skill_id = item.id.clone();
    let request_value = json!({
        "jsonrpc": "2.0",
        "id": 42,
        "method": SKILLS_SET_ENABLED_METHOD,
        "params": {
            "skillId": skill_id,
            "expectedStateRevision": item.state_revision,
            "enabled": false
        }
    });
    let input = format!("{request_value}\n");
    let agent_service = AgentService::new(Arc::clone(&storage));
    let installations =
        Arc::new(SkillInstallationService::new(temp.path().join("skills")).unwrap());
    let (outbound_tx, mut outbound_rx) = mpsc::unbounded_channel();
    let git_dispatcher = GitDispatcher::new(outbound_tx.clone());
    let skills_dispatcher = SkillsDispatcher::new(outbound_tx.clone());
    let dispatchers = RequestDispatchers {
        git: &git_dispatcher,
        skills: &skills_dispatcher,
        skill_acquisition: &skills_dispatcher,
    };

    run_request_loop(
        BufReader::new(input.as_bytes()),
        Arc::clone(&storage),
        &agent_service,
        SkillServices {
            catalog: Arc::clone(&skills_service),
            installations,
            workflow: Arc::new(SkillInstallationWorkflow::new(
                SkillInstallationService::new(temp.path().join("skills")).unwrap(),
            )),
            source_resolution: Arc::new(SkillSourceResolutionService::new()),
        },
        Arc::new(GitReviewService::new()),
        &dispatchers,
        &outbound_tx,
    )
    .await
    .unwrap();

    let first = tokio::time::timeout(Duration::from_secs(2), outbound_rx.recv())
        .await
        .unwrap()
        .unwrap();
    let second = tokio::time::timeout(Duration::from_secs(2), outbound_rx.recv())
        .await
        .unwrap()
        .unwrap();
    let messages = [first, second];
    let notification = messages
        .iter()
        .find(|message| message["method"] == SKILLS_CHANGED_NOTIFICATION_METHOD)
        .expect("a changed Skill must emit an invalidation");
    assert_eq!(notification["params"]["reason"], "enablementChanged");
    assert_eq!(notification["params"]["skillId"], skill_id);
    assert!(notification["params"]["managementRevision"].is_string());
    let response = messages
        .iter()
        .find(|message| message["id"] == 42)
        .expect("the mutation response must also be delivered");
    assert_eq!(response["result"]["outcome"], "updated");

    git_dispatcher.shutdown().await.unwrap();
    skills_dispatcher.shutdown().await.unwrap();
}

#[test]
fn indeterminate_direct_mutation_emits_catalog_invalidation_without_claiming_success() {
    const INSTALLATION_ID: &str = "0190b0f2-7c50-7cc0-8b25-3bb80f08b336";
    let temp = tempfile::tempdir().unwrap();
    let storage = StorageService::open(&temp.path().join("storage.sqlite")).unwrap();
    let catalog = SkillsService::new();
    let installations = SkillInstallationService::new(temp.path().join("skills")).unwrap();
    let workflow = SkillInstallationWorkflow::new(
        SkillInstallationService::new(temp.path().join("skills")).unwrap(),
    );
    let (notification_tx, mut notification_rx) = mpsc::unbounded_channel();
    let installation_id = SkillInstallationId::parse(INSTALLATION_ID).unwrap();
    let skill_id = SkillId::parse(format!("installed:user:{INSTALLATION_ID}")).unwrap();
    let intended_revision = SkillInstallationRevision::parse(format!(
        "skill-installation-sha256-v1:{}",
        "a".repeat(64)
    ))
    .unwrap();
    let error = SkillInstallationServiceError::Installer {
        operation: SkillInstallationOperation::Update,
        installation_id: installation_id.clone(),
        skill_id: skill_id.clone(),
        source: Box::new(
            mycopilot_core::skills::ManagedSkillInstallerError::CommitIndeterminate {
                operation: mycopilot_core::skills::ManagedSkillMutation::Update,
                installation_id,
                intended_revision: Some(intended_revision),
                reason: "receipt directory acknowledgement was lost".to_string(),
            },
        ),
    };

    let response = skill_mutation_response_with_invalidation(
        &storage,
        &catalog,
        &installations,
        Some(&workflow),
        Some(&notification_tx),
        JsonRpcId::Number(91),
        Err(error),
    );

    assert_eq!(response["error"]["data"]["code"], "commitIndeterminate");
    assert_eq!(response["error"]["data"]["commitMayHaveSucceeded"], true);
    assert!(response.get("result").is_none());
    let notification = notification_rx
        .try_recv()
        .expect("an uncertain direct mutation must invalidate management inventory");
    assert_eq!(notification["method"], SKILLS_CHANGED_NOTIFICATION_METHOD);
    assert_eq!(notification["params"]["reason"], "catalogChanged");
    assert_eq!(notification["params"]["skillId"], skill_id.as_str());
    assert!(notification["params"]["managementRevision"].is_string());
    assert!(notification_rx.try_recv().is_err());
}

#[test]
fn indeterminate_workflow_commit_emits_catalog_invalidation_without_claiming_success() {
    const INSTALLATION_ID: &str = "11111111-1111-4111-8111-111111111111";
    let temp = tempfile::tempdir().unwrap();
    let storage = StorageService::open(&temp.path().join("storage.sqlite")).unwrap();
    let catalog = SkillsService::new();
    let installations = SkillInstallationService::new(temp.path().join("skills")).unwrap();
    let workflow = SkillInstallationWorkflow::new(
        SkillInstallationService::new(temp.path().join("skills")).unwrap(),
    );
    let (notification_tx, mut notification_rx) = mpsc::unbounded_channel();
    let preparation_id =
        mycopilot_core::skills::SkillPreparationId::parse(INSTALLATION_ID).unwrap();
    let installation_id = SkillInstallationId::parse(INSTALLATION_ID).unwrap();
    let skill_id = SkillId::parse(format!("installed:user:{INSTALLATION_ID}")).unwrap();
    let intended_revision = SkillInstallationRevision::parse(format!(
        "skill-installation-sha256-v1:{}",
        "b".repeat(64)
    ))
    .unwrap();
    let workflow_error = mycopilot_core::skills::SkillInstallationWorkflowError::Installation {
        preparation_id,
        source: Box::new(SkillInstallationServiceError::Installer {
            operation: SkillInstallationOperation::Install,
            installation_id: installation_id.clone(),
            skill_id: skill_id.clone(),
            source: Box::new(
                mycopilot_core::skills::ManagedSkillInstallerError::CommitIndeterminate {
                    operation: mycopilot_core::skills::ManagedSkillMutation::Install,
                    installation_id,
                    intended_revision: Some(intended_revision),
                    reason: "receipt directory acknowledgement was lost".to_string(),
                },
            ),
        }),
    };
    let failure = workflow_failure(
        mycopilot_protocol_rs::SkillInspectionPhaseDto::Commit,
        &workflow_error,
    );

    let response = skill_workflow_commit_error_response_with_invalidation(
        &storage,
        &catalog,
        &installations,
        Some(&workflow),
        Some(&notification_tx),
        JsonRpcId::Number(92),
        failure,
    );

    assert_eq!(response["error"]["data"]["code"], "commitIndeterminate");
    assert_eq!(response["error"]["data"]["commitMayHaveSucceeded"], true);
    assert!(response.get("result").is_none());
    let notification = notification_rx
        .try_recv()
        .expect("an uncertain workflow commit must invalidate management inventory");
    assert_eq!(notification["method"], SKILLS_CHANGED_NOTIFICATION_METHOD);
    assert_eq!(notification["params"]["reason"], "catalogChanged");
    assert_eq!(notification["params"]["skillId"], skill_id.as_str());
    assert!(notification_rx.try_recv().is_err());
}

#[test]
fn ordinary_direct_mutation_failure_does_not_emit_an_invalidation() {
    const INSTALLATION_ID: &str = "0190b0f2-7c50-7cc0-8b25-3bb80f08b337";
    let temp = tempfile::tempdir().unwrap();
    let storage = StorageService::open(&temp.path().join("storage.sqlite")).unwrap();
    let catalog = SkillsService::new();
    let installations = SkillInstallationService::new(temp.path().join("skills")).unwrap();
    let workflow = SkillInstallationWorkflow::new(
        SkillInstallationService::new(temp.path().join("skills")).unwrap(),
    );
    let (notification_tx, mut notification_rx) = mpsc::unbounded_channel();
    let installation_id = SkillInstallationId::parse(INSTALLATION_ID).unwrap();
    let skill_id = SkillId::parse(format!("installed:user:{INSTALLATION_ID}")).unwrap();
    let error = SkillInstallationServiceError::Installer {
        operation: SkillInstallationOperation::Update,
        installation_id,
        skill_id,
        source: Box::new(mycopilot_core::skills::ManagedSkillInstallerError::Io {
            operation: "publish managed receipt".to_string(),
            reason: "injected pre-publication failure".to_string(),
        }),
    };

    let response = skill_mutation_response_with_invalidation(
        &storage,
        &catalog,
        &installations,
        Some(&workflow),
        Some(&notification_tx),
        JsonRpcId::Number(93),
        Err(error),
    );

    assert_eq!(response["error"]["data"]["code"], "io");
    assert_eq!(response["error"]["data"]["commitMayHaveSucceeded"], false);
    assert!(notification_rx.try_recv().is_err());
}

#[tokio::test]
async fn outbound_writer_finishes_after_draining_with_a_lingering_sender() {
    let (writer_stream, mut reader_stream) = io::duplex(1024);
    let (outbound_tx, outbound_rx) = mpsc::unbounded_channel();
    let lingering_sender = outbound_tx.clone();
    let (finish_tx, finish_rx) = oneshot::channel();
    let writer = tokio::spawn(run_outbound_writer(writer_stream, outbound_rx, finish_rx));
    let first = json!({ "id": 1, "result": "before shutdown" });
    let final_response = json!({ "id": 2, "result": "shutdown" });

    enqueue_outbound(&outbound_tx, first.clone()).unwrap();
    enqueue_outbound(&outbound_tx, final_response.clone()).unwrap();
    finish_tx.send(()).unwrap();
    drop(outbound_tx);

    tokio::time::timeout(Duration::from_secs(1), writer)
        .await
        .expect("writer must not wait for a lingering producer")
        .expect("writer task must join")
        .expect("writer must drain successfully");
    assert!(lingering_sender.send(json!({ "late": true })).is_err());

    let mut output = String::new();
    reader_stream.read_to_string(&mut output).await.unwrap();
    let messages = output
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(messages, vec![first, final_response]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn request_loop_remains_responsive_while_filesystem_workers_are_blocked() {
    let temp = tempfile::tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&temp.path().join("storage.sqlite")).unwrap());
    let agent_service = AgentService::new(Arc::clone(&storage));
    let skills_service = Arc::new(SkillsService::new());
    let skill_installation_service =
        Arc::new(SkillInstallationService::new(temp.path().join("skills")).unwrap());
    let git_review_service = Arc::new(GitReviewService::new());
    let (outbound_tx, mut outbound_rx) = mpsc::unbounded_channel();
    let dispatcher = GitDispatcher::new(outbound_tx.clone());
    let skill_dispatcher = SkillsDispatcher::new(outbound_tx.clone());
    let request_dispatchers = RequestDispatchers {
        git: &dispatcher,
        skills: &skill_dispatcher,
        skill_acquisition: &skill_dispatcher,
    };
    let (started_tx, started_rx) = std_mpsc::channel();
    let mut release_senders = Vec::new();

    for request_id in 1..=3 {
        let (release_tx, release_rx) = std_mpsc::channel();
        release_senders.push(release_tx);
        let started_tx = started_tx.clone();
        dispatcher
            .try_submit(
                GitJobPriority::Low,
                JsonRpcId::Number(request_id),
                move || {
                    started_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    serde_json::json!({ "id": request_id })
                },
            )
            .unwrap();
    }
    for _ in 0..3 {
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    }
    let (skill_started_tx, skill_started_rx) = std_mpsc::channel();
    let (skill_release_tx, skill_release_rx) = std_mpsc::channel();
    skill_dispatcher
        .try_submit(JsonRpcId::Number(4), move || {
            skill_started_tx.send(()).unwrap();
            skill_release_rx.recv().unwrap();
            serde_json::json!({ "id": 4 })
        })
        .unwrap();
    skill_started_rx
        .recv_timeout(Duration::from_secs(2))
        .unwrap();

    let input = concat!(
        "{\"jsonrpc\":\"2.0\",\"id\":99,\"method\":\"core.ping\",",
        "\"params\":{\"message\":\"responsive\"}}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":100,\"method\":\"core.shutdown\"}\n"
    );
    let shutdown_id = run_request_loop(
        BufReader::new(input.as_bytes()),
        storage,
        &agent_service,
        SkillServices {
            catalog: skills_service,
            installations: skill_installation_service,
            workflow: Arc::new(SkillInstallationWorkflow::new(
                SkillInstallationService::new(temp.path().join("skills")).unwrap(),
            )),
            source_resolution: Arc::new(SkillSourceResolutionService::new()),
        },
        git_review_service,
        &request_dispatchers,
        &outbound_tx,
    )
    .await
    .unwrap();

    assert!(matches!(shutdown_id, Some(JsonRpcId::Number(100))));
    let ping = outbound_rx.try_recv().unwrap();
    assert_eq!(ping["id"], 99);
    assert_eq!(ping["result"]["echo"], "responsive");

    for release in release_senders {
        release.send(()).unwrap();
    }
    skill_release_tx.send(()).unwrap();
    dispatcher.shutdown().await.unwrap();
    skill_dispatcher.shutdown().await.unwrap();
}
