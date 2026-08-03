use super::*;

#[tokio::test]
async fn launch_authorization_preview_is_one_shot_and_commit_is_cas_bound() {
    let harness = TestHarness::new();
    let added = harness
        .service
        .add_server(create_input("authorization fixture"))
        .expect("add server");

    let stale_preview = harness
        .service
        .prepare_launch_authorization(mutation_input(&added.server))
        .expect("prepare stale preview");
    let renamed = harness
        .service
        .update_server(update_input(
            &added.server,
            "renamed authorization fixture",
            added.server.arguments.clone(),
        ))
        .await
        .expect("rename server");

    let stale_error = harness
        .service
        .commit_launch_authorization(commit_input(&stale_preview))
        .expect_err("stale Registry identity must fail authorization");
    assert_eq!(
        failure_code(stale_error),
        McpManagementErrorCodeDto::Conflict
    );
    let consumed_error = harness
        .service
        .commit_launch_authorization(commit_input(&stale_preview))
        .expect_err("failed preview is still consumed exactly once");
    assert_eq!(
        failure_code(consumed_error),
        McpManagementErrorCodeDto::AuthorizationStale
    );

    let preview = harness
        .service
        .prepare_launch_authorization(mutation_input(&renamed.server))
        .expect("prepare current preview");
    let committed = harness
        .service
        .commit_launch_authorization(commit_input(&preview))
        .expect("commit current preview");
    assert!(committed.authorized);
    assert_eq!(
        committed.server.summary.launch_authorization_state,
        McpLaunchAuthorizationStateDto::Authorized
    );
    let replay_error = harness
        .service
        .commit_launch_authorization(commit_input(&preview))
        .expect_err("authorization preview cannot be replayed");
    assert_eq!(
        failure_code(replay_error),
        McpManagementErrorCodeDto::AuthorizationStale
    );

    harness.shutdown().await;
}

#[tokio::test]
async fn launch_authorization_commit_rejects_file_identity_drift_and_details_show_stale() {
    let owned_launch = tempfile::tempdir().expect("temporary owned launch directory");
    let executable = owned_launch.path().join("owned-mcp-fixture");
    std::fs::write(&executable, b"owned executable version one")
        .expect("write repository-owned launch fixture");
    let mut input = create_input("physical identity fixture");
    input.executable = executable.to_string_lossy().into_owned();
    input.cwd = owned_launch.path().to_string_lossy().into_owned();

    let harness = TestHarness::new();
    let added = harness.service.add_server(input).expect("add server");
    let stale_preview = harness
        .service
        .prepare_launch_authorization(mutation_input(&added.server))
        .expect("prepare launch authorization");
    std::fs::write(
        &executable,
        b"owned executable version two with different length",
    )
    .expect("replace repository-owned launch fixture");
    let conflict = harness
        .service
        .commit_launch_authorization(commit_input(&stale_preview))
        .expect_err("file drift during native confirmation must fail");
    assert_eq!(failure_code(conflict), McpManagementErrorCodeDto::Conflict);

    let current = harness
        .service
        .get_server(
            McpServerIdInput {
                schema_version: MCP_MANAGEMENT_SCHEMA_VERSION,
                server_id: added.server.summary.server_id.clone(),
            },
            McpManagementOperationDto::Get,
        )
        .expect("read current server details");
    let authorized = authorize(&harness.service, &current.server);
    assert_eq!(
        authorized.server.summary.launch_authorization_state,
        McpLaunchAuthorizationStateDto::Authorized
    );
    std::fs::write(&executable, b"third owned executable replacement")
        .expect("replace authorized repository launch fixture");
    let stale = harness
        .service
        .get_server(
            McpServerIdInput {
                schema_version: MCP_MANAGEMENT_SCHEMA_VERSION,
                server_id: added.server.summary.server_id,
            },
            McpManagementOperationDto::Get,
        )
        .expect("read stale launch authorization details");
    assert_eq!(
        stale.server.summary.launch_authorization_state,
        McpLaunchAuthorizationStateDto::Stale
    );

    harness.shutdown().await;
}

#[cfg(unix)]
#[tokio::test]
async fn launch_authorization_preview_shows_the_actual_process_plan() {
    use std::os::unix::fs::symlink;

    let owned_launch = tempfile::tempdir().expect("temporary owned launch directory");
    let executable_target = owned_launch.path().join("owned-executable-target");
    let executable_link = owned_launch.path().join("owned-executable-link");
    let script_target = owned_launch.path().join("owned-script-target.js");
    let script_link = owned_launch.path().join("owned-script-link.js");
    std::fs::write(&executable_target, b"owned executable target").unwrap();
    std::fs::write(&script_target, b"owned script target").unwrap();
    symlink(&executable_target, &executable_link).unwrap();
    symlink(&script_target, &script_link).unwrap();
    let mut input = create_input("canonical preview fixture");
    input.executable = executable_link.to_string_lossy().into_owned();
    input.arguments = vec![script_link.to_string_lossy().into_owned()];
    input.cwd = owned_launch.path().to_string_lossy().into_owned();

    let harness = TestHarness::new();
    let added = harness.service.add_server(input).expect("add server");
    let preview = harness
        .service
        .prepare_launch_authorization(mutation_input(&added.server))
        .expect("prepare canonical launch preview");
    assert_eq!(preview.executable, executable_link.to_string_lossy());
    assert_eq!(
        preview.arguments,
        vec![std::fs::canonicalize(script_target)
            .unwrap()
            .to_string_lossy()
            .into_owned()]
    );
    assert_eq!(
        preview.cwd,
        std::fs::canonicalize(owned_launch.path())
            .unwrap()
            .to_string_lossy()
    );
    harness.shutdown().await;
}

#[tokio::test]
async fn newest_launch_authorization_preview_atomically_replaces_older_preview() {
    let harness = TestHarness::new();
    let added = harness
        .service
        .add_server(create_input("bounded authorization fixture"))
        .expect("add server");

    let mut previews = Vec::new();
    for _ in 0..8 {
        previews.push(
            harness
                .service
                .prepare_launch_authorization(mutation_input(&added.server))
                .expect("replacement preview"),
        );
    }
    assert_eq!(
        harness
            .service
            .authorization_previews
            .lock()
            .expect("preview lock")
            .len(),
        1
    );
    for stale in &previews[..previews.len() - 1] {
        let failure = harness
            .service
            .commit_launch_authorization(commit_input(stale))
            .expect_err("a superseded preview must fail closed");
        assert_eq!(
            failure_code(failure),
            McpManagementErrorCodeDto::AuthorizationStale
        );
    }
    harness
        .service
        .commit_launch_authorization(commit_input(previews.last().expect("latest preview")))
        .expect("the newest preview remains one-shot usable");

    harness.shutdown().await;
}

#[tokio::test]
async fn launch_authorization_previews_have_global_hard_admission() {
    let harness = TestHarness::new();
    for index in 0..MAX_LAUNCH_AUTHORIZATION_PREVIEWS {
        let added = harness
            .service
            .add_server(create_input(&format!("bounded fixture {index}")))
            .expect("add server within global preview admission");
        harness
            .service
            .prepare_launch_authorization(mutation_input(&added.server))
            .expect("prepare preview within global admission");
    }
    let overflow = harness
        .service
        .add_server(create_input("overflow fixture"))
        .expect("add overflow server without authorizing it");
    let denied = harness
        .service
        .prepare_launch_authorization(mutation_input(&overflow.server))
        .expect_err("global authorization preview storm must be rejected");
    let data = denied.into_data();
    assert_eq!(data.code, McpManagementErrorCodeDto::PolicyDenied);
    assert_eq!(data.recovery, McpManagementRecoveryDto::Retry);
    assert!(!data.message.contains("overflow fixture"));
    assert_eq!(
        harness
            .service
            .authorization_previews
            .lock()
            .expect("preview lock")
            .len(),
        MAX_LAUNCH_AUTHORIZATION_PREVIEWS
    );

    harness.shutdown().await;
}

#[tokio::test]
async fn enable_requires_exact_authorization_and_preserves_authorization_identity() {
    let harness = TestHarness::new();
    let added = harness
        .service
        .add_server(create_input("enable fixture"))
        .expect("add server");

    let denied = harness
        .service
        .enable_server(mutation_input(&added.server))
        .expect_err("untrusted server cannot be enabled");
    assert_eq!(
        failure_code(denied),
        McpManagementErrorCodeDto::AuthorizationRequired
    );

    let authorized = authorize(&harness.service, &added.server);
    let id = server_id(&authorized.server);
    let authorized_record = harness
        .registry
        .get_persisted(id)
        .expect("read authorized server")
        .expect("authorized server exists");
    let authorization = authorized_record
        .launch_authorization
        .clone()
        .expect("launch authorization exists");
    assert_eq!(
        authorization.authored_config_epoch,
        authorized_record.entry.config_epoch
    );
    assert_eq!(
        authorization.authored_config_digest,
        authorized_record.entry.config_digest
    );

    let enabled = harness
        .service
        .enable_server(mutation_input(&authorized.server))
        .expect("enable authorized server");
    assert!(enabled.server.summary.enabled);
    assert_eq!(enabled.server.summary.trust, McpTrustLevelDto::UserApproved);
    assert_eq!(
        enabled.server.summary.launch_authorization_state,
        McpLaunchAuthorizationStateDto::Authorized
    );

    let enabled_record = harness
        .registry
        .get_persisted(id)
        .expect("read enabled server")
        .expect("enabled server exists");
    let enabled_authorization = enabled_record
        .launch_authorization
        .expect("authorization is retained");
    assert_eq!(
        enabled_record.entry.config_epoch, enabled_authorization.authored_config_epoch,
        "Host mutations rebind exact authorization to the committed config identity"
    );
    assert_eq!(
        enabled_record.entry.config_digest,
        enabled_authorization.authored_config_digest
    );
    assert_ne!(
        enabled_authorization.authored_config_epoch,
        authorization.authored_config_epoch
    );
    assert_ne!(
        enabled_authorization.authored_config_digest,
        authorization.authored_config_digest
    );
    assert_eq!(
        enabled_authorization.launch_spec_digest,
        enabled_record.launch_spec_digest
    );

    harness.shutdown().await;
}
