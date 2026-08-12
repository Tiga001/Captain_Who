use super::*;

#[test]
fn source_resolution_cancellation_is_routed_and_idempotent() {
    let temp = tempfile::tempdir().unwrap();
    let storage = StorageService::open(&temp.path().join("storage.sqlite")).unwrap();
    let catalog = SkillsService::new();
    let installations = SkillInstallationService::new(temp.path().join("skills")).unwrap();
    let workflow = SkillInstallationWorkflow::new(
        SkillInstallationService::new(temp.path().join("skills")).unwrap(),
    );
    let mut source_resolution =
        SkillSourceResolutionService::with_session_store(workflow.session_store());
    source_resolution
        .register_resolver(Arc::new(StaticGitHubSourceResolver))
        .unwrap();
    let resolution_id = "11111111-1111-4111-8111-111111111111";

    let resolve = parse_skills_request(
        serde_json::from_value::<JsonRpcRequest>(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": SKILLS_RESOLVE_INSTALLATION_SOURCE_METHOD,
            "params": {
                "resolutionId": resolution_id,
                "locator": {
                    "kind": "url",
                    "url": "https://github.com/example/skills"
                }
            }
        }))
        .unwrap(),
    )
    .unwrap();
    let resolved = handle_parsed_skills_request(
        &storage,
        &catalog,
        &installations,
        Some(&workflow),
        Some(&source_resolution),
        None,
        resolve,
    );
    assert_eq!(resolved["result"]["resolutionId"], resolution_id);

    let cancel = |id, resolution_id: &str| {
        let request = parse_skills_request(
            serde_json::from_value::<JsonRpcRequest>(json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": SKILLS_CANCEL_SOURCE_RESOLUTION_METHOD,
                "params": { "resolutionId": resolution_id }
            }))
            .unwrap(),
        )
        .unwrap();
        handle_parsed_skills_request(
            &storage,
            &catalog,
            &installations,
            Some(&workflow),
            Some(&source_resolution),
            None,
            request,
        )
    };

    let first = cancel(2, resolution_id);
    assert_eq!(first["result"]["outcome"], "cancelled");
    assert_eq!(first["result"]["resolutionId"], resolution_id);
    assert_eq!(
        cancel(3, resolution_id)["result"]["outcome"],
        "alreadyCancelled"
    );
    assert_eq!(
        cancel(4, "99999999-9999-4999-8999-999999999999")["result"]["outcome"],
        "alreadyAbsent"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn queued_source_cancellation_cannot_overtake_the_resolution_it_fences() {
    let temp = tempfile::tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&temp.path().join("storage.sqlite")).unwrap());
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
    let workflow = SkillInstallationWorkflow::new(
        SkillInstallationService::new(temp.path().join("skills")).unwrap(),
    );
    let (started_tx, started_rx) = std_mpsc::channel();
    let (release_tx, release_rx) = std_mpsc::channel();
    let mut source_resolution =
        SkillSourceResolutionService::with_session_store(workflow.session_store());
    source_resolution
        .register_resolver(Arc::new(BlockingGitHubSourceResolver {
            started: started_tx,
            release: Mutex::new(release_rx),
        }))
        .unwrap();
    let resolution_id = "11111111-1111-4111-8111-111111111111";
    let input = format!(
        concat!(
            "{{\"jsonrpc\":\"2.0\",\"id\":81,",
            "\"method\":\"skills.resolveInstallationSource\",",
            "\"params\":{{\"resolutionId\":\"{resolution_id}\",",
            "\"locator\":{{\"kind\":\"url\",",
            "\"url\":\"https://github.com/example/skills\"}}}}}}\n",
            "{{\"jsonrpc\":\"2.0\",\"id\":82,",
            "\"method\":\"skills.cancelSourceResolution\",",
            "\"params\":{{\"resolutionId\":\"{resolution_id}\"}}}}\n"
        ),
        resolution_id = resolution_id,
    );

    let shutdown_id = run_request_loop(
        BufReader::new(input.as_bytes()),
        test_core_request_services(Arc::clone(&storage)),
        &agent_service,
        SkillServices {
            catalog: Arc::new(SkillsService::new()),
            installations: Arc::new(
                SkillInstallationService::new(temp.path().join("skills")).unwrap(),
            ),
            workflow: Arc::new(workflow),
            source_resolution: Arc::new(source_resolution),
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
    assert!(shutdown_id.is_none());
    started_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("resolution must enter the acquisition lane");

    assert!(
        tokio::time::timeout(Duration::from_millis(100), outbound_rx.recv())
            .await
            .is_err(),
        "cancellation must wait behind the earlier accepted resolution"
    );
    release_tx.send(()).unwrap();
    let resolved = tokio::time::timeout(Duration::from_secs(2), outbound_rx.recv())
        .await
        .unwrap()
        .unwrap();
    let cancelled = tokio::time::timeout(Duration::from_secs(2), outbound_rx.recv())
        .await
        .unwrap()
        .unwrap();

    assert_eq!(resolved["id"], 81);
    assert_eq!(resolved["result"]["outcome"], "resolved");
    assert_eq!(cancelled["id"], 82);
    assert_eq!(cancelled["result"]["outcome"], "cancelled");

    git_dispatcher.shutdown().await.unwrap();
    skills_dispatcher.shutdown().await.unwrap();
    acquisition_dispatcher.shutdown().await.unwrap();
    image_generation_dispatcher.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn request_loop_resolves_and_hands_off_a_candidate_on_one_acquisition_lane() {
    let temp = tempfile::tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&temp.path().join("storage.sqlite")).unwrap());
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
    let workflow = SkillInstallationWorkflow::new(
        SkillInstallationService::new(temp.path().join("skills")).unwrap(),
    );
    let mut source_resolution =
        SkillSourceResolutionService::with_session_store(workflow.session_store());
    source_resolution
        .register_resolver(Arc::new(StaticGitHubSourceResolver))
        .unwrap();
    let input = concat!(
        "{\"jsonrpc\":\"2.0\",\"id\":71,",
        "\"method\":\"skills.resolveInstallationSource\",",
        "\"params\":{\"resolutionId\":",
        "\"11111111-1111-4111-8111-111111111111\",",
        "\"locator\":{\"kind\":\"url\",",
        "\"url\":\"https://github.com/example/skills\"}}}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":72,",
        "\"method\":\"skills.inspectInstallation\",",
        "\"params\":{\"preparationId\":",
        "\"33333333-3333-4333-8333-333333333333\",",
        "\"intent\":{\"operation\":\"install\"},",
        "\"source\":{\"kind\":\"resolvedCandidate\",",
        "\"resolutionId\":\"11111111-1111-4111-8111-111111111111\",",
        "\"candidateId\":\"candidate-auditor\"}}}\n"
    );

    let shutdown_id = run_request_loop(
        BufReader::new(input.as_bytes()),
        test_core_request_services(Arc::clone(&storage)),
        &agent_service,
        SkillServices {
            catalog: Arc::new(SkillsService::new()),
            installations: Arc::new(
                SkillInstallationService::new(temp.path().join("skills")).unwrap(),
            ),
            workflow: Arc::new(workflow),
            source_resolution: Arc::new(source_resolution),
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
    let first = tokio::time::timeout(Duration::from_secs(2), outbound_rx.recv())
        .await
        .expect("source resolution must complete")
        .expect("source resolution must produce a response");
    let second = tokio::time::timeout(Duration::from_secs(2), outbound_rx.recv())
        .await
        .expect("candidate inspection must complete")
        .expect("candidate inspection must produce a response");
    let responses = [first, second];
    let response = responses
        .iter()
        .find(|response| response["id"] == 71)
        .expect("source resolution response");
    let preview = responses
        .iter()
        .find(|response| response["id"] == 72)
        .expect("candidate inspection response");

    assert!(shutdown_id.is_none());
    assert_eq!(response["id"], 71);
    assert_eq!(
        response["result"]["resolutionId"],
        "11111111-1111-4111-8111-111111111111"
    );
    assert!(response["result"]["expiresAtUnixMs"].as_u64().unwrap() > 0);
    assert_eq!(response["result"]["outcome"], "resolved");
    assert_eq!(
        response["result"]["candidates"][0]["source"]["reference"]["kind"],
        "named"
    );
    assert_eq!(
        response["result"]["candidates"][0]["source"]["reference"]["value"],
        "main"
    );
    assert_eq!(
        response["result"]["candidates"][0]["source"]["resolvedCommit"],
        "0123456789abcdef0123456789abcdef01234567"
    );
    assert_eq!(
        response["result"]["candidates"][0]["acquisition"],
        json!({
            "kind": "resolvedCandidate",
            "resolutionId": "11111111-1111-4111-8111-111111111111",
            "candidateId": "candidate-auditor"
        })
    );
    assert!(
        preview.get("error").is_none(),
        "candidate inspection failed: {preview}"
    );
    assert_eq!(
        preview["result"]["preparationId"],
        "33333333-3333-4333-8333-333333333333"
    );
    assert_eq!(preview["result"]["package"]["name"], "auditor");

    git_dispatcher.shutdown().await.unwrap();
    skills_dispatcher.shutdown().await.unwrap();
    acquisition_dispatcher.shutdown().await.unwrap();
    image_generation_dispatcher.shutdown().await.unwrap();
}

#[test]
fn source_resolution_rejects_invalid_resolution_ids_as_invalid_params() {
    let request = serde_json::from_value::<JsonRpcRequest>(json!({
        "jsonrpc": "2.0",
        "id": 72,
        "method": SKILLS_RESOLVE_INSTALLATION_SOURCE_METHOD,
        "params": {
            "resolutionId": "00000000-0000-0000-0000-000000000000",
            "locator": {
                "kind": "url",
                "url": "https://github.com/example/skills"
            }
        }
    }))
    .unwrap();

    let response = match parse_skills_request(request) {
        Ok(_) => panic!("a nil resolution ID must not cross the protocol boundary"),
        Err(response) => response,
    };

    assert_eq!(response["id"], 72);
    assert_eq!(response["error"]["code"], -32602);
    assert!(response["error"]["message"]
        .as_str()
        .unwrap()
        .contains("canonical non-nil lower-case UUID"));
}

#[test]
fn uninstall_rejects_a_malformed_installation_revision() {
    let request = serde_json::from_value::<JsonRpcRequest>(json!({
        "jsonrpc": "2.0",
        "id": 73,
        "method": SKILLS_UNINSTALL_METHOD,
        "params": {
            "skillId": "installed:user:0190b0f2-7c50-7cc0-8b25-3bb80f08b336",
            "expectedRevision": format!(
                "{}malformed",
                mycopilot_core::skills::SKILL_INSTALLATION_REVISION_PREFIX
            )
        }
    }))
    .unwrap();

    let response = match parse_skills_request(request) {
        Ok(_) => panic!("malformed installation revisions must fail closed"),
        Err(response) => response,
    };

    assert_eq!(response["id"], 73);
    assert_eq!(response["error"]["code"], -32602);
    assert!(response["error"]["message"]
        .as_str()
        .unwrap()
        .contains("installation revision"));
}

#[test]
fn uninstall_rejects_package_revision_and_noncanonical_params() {
    for (id, params) in [
        (
            74,
            json!({
                "skillId": "installed:user:0190b0f2-7c50-7cc0-8b25-3bb80f08b336",
                "expectedRevision": format!("skill-package-sha256-v1:{}", "a".repeat(64))
            }),
        ),
        (
            75,
            json!({
                "skillId": "installed:user:0190b0f2-7c50-7cc0-8b25-3bb80f08b336"
            }),
        ),
        (
            76,
            json!({
                "skillId": "installed:user:0190b0f2-7c50-7cc0-8b25-3bb80f08b336",
                "expectedRevision": format!(
                    "{}{}",
                    mycopilot_core::skills::SKILL_INSTALLATION_REVISION_PREFIX,
                    "a".repeat(64)
                ),
                "packageRevision": format!("skill-package-sha256-v1:{}", "a".repeat(64))
            }),
        ),
    ] {
        let request = serde_json::from_value::<JsonRpcRequest>(json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": SKILLS_UNINSTALL_METHOD,
            "params": params
        }))
        .unwrap();

        let response = match parse_skills_request(request) {
            Ok(_) => panic!("noncanonical uninstall input must fail closed"),
            Err(response) => response,
        };
        assert_eq!(response["id"], id);
        assert_eq!(response["error"]["code"], -32602);
    }
}

#[test]
fn source_resolution_rejects_unknown_hosts_with_structured_recovery_data() {
    let temp = tempfile::tempdir().unwrap();
    let storage = StorageService::open(&temp.path().join("storage.sqlite")).unwrap();
    let installations = SkillInstallationService::new(temp.path().join("skills")).unwrap();
    let service = SkillSourceResolutionService::new();
    let request = serde_json::from_value::<JsonRpcRequest>(json!({
        "jsonrpc": "2.0",
        "id": 72,
        "method": SKILLS_RESOLVE_INSTALLATION_SOURCE_METHOD,
        "params": {
            "resolutionId": "22222222-2222-4222-8222-222222222222",
            "locator": {
                "kind": "url",
                "url": "https://untrusted.example/private?token=secret"
            }
        }
    }))
    .unwrap();
    let parsed = parse_skills_request(request).unwrap();

    let response = handle_parsed_skills_request(
        &storage,
        &SkillsService::new(),
        &installations,
        None,
        Some(&service),
        None,
        parsed,
    );

    assert_eq!(
        response["error"]["code"],
        SKILL_SOURCE_RESOLUTION_ERROR_CODE
    );
    assert_eq!(response["error"]["data"]["type"], "skillSourceResolution");
    assert_eq!(response["error"]["data"]["phase"], "parse");
    assert_eq!(response["error"]["data"]["code"], "unsupportedHost");
    assert_eq!(
        response["error"]["data"]["recovery"],
        "chooseDifferentSource"
    );
    assert!(!response.to_string().contains("token=secret"));
}
