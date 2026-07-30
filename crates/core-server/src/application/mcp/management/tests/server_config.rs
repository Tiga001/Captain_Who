use super::*;

#[tokio::test]
async fn add_is_disabled_untrusted_prompt_and_projects_only_safe_fields() {
    let harness = TestHarness::new();
    let output = harness
        .service
        .add_server(create_input("safe fixture"))
        .expect("add server");

    assert!(!output.server.summary.enabled);
    assert_eq!(output.server.summary.trust, McpTrustLevelDto::Untrusted);
    assert_eq!(
        output.server.summary.approval_mode,
        McpApprovalModeDto::Prompt
    );
    assert_eq!(
        output.server.summary.launch_authorization_state,
        McpLaunchAuthorizationStateDto::Required
    );
    assert_eq!(output.server.summary.state, McpConnectionStateDto::Disabled);

    let persisted = harness
        .registry
        .get_persisted(server_id(&output.server))
        .expect("read persisted server")
        .expect("server exists");
    let McpTransportConfig::Stdio(stdio) = &persisted.entry.config.transport else {
        panic!("Round 5A Registry only accepts stdio");
    };
    assert!(stdio.environment.is_empty());
    assert!(persisted.launch_authorization.is_none());

    let serialized = serde_json::to_value(&output).expect("serialize safe details DTO");
    assert_renderer_safe_json(&serialized);
    let serialized_text = serde_json::to_string(&serialized).expect("serialize safe JSON");
    for forbidden in [
        "MCP_TEST_TOKEN_CANARY",
        "MCP_TEST_HEADER_CANARY",
        "MCP_TEST_CIPHERTEXT_CANARY",
    ] {
        assert!(!serialized_text.contains(forbidden));
    }

    harness.shutdown().await;
}

#[tokio::test]
async fn add_maps_registry_capacity_to_policy_denied_without_publishing_an_event() {
    const MAX_SERVERS: usize = crate::application::mcp::sqlite_registry::MCP_REGISTRY_MAX_SERVERS;

    let harness = TestHarness::new();
    for index in 0..MAX_SERVERS {
        harness
            .service
            .add_server(create_input(&format!("capacity fixture {index}")))
            .expect("add within the Registry capacity");
    }
    let mut changes = harness.registry.subscribe();
    let failure = harness
        .service
        .add_server(create_input("over capacity"))
        .expect_err("the Host capacity policy must reject another Server")
        .into_data();
    assert_eq!(failure.code, McpManagementErrorCodeDto::PolicyDenied);
    assert_eq!(failure.recovery, McpManagementRecoveryDto::DoNotRetry);
    assert_eq!(failure.current_registry_revision, Some(MAX_SERVERS as u64));
    let (revision, records) = harness.registry.snapshot().unwrap();
    assert_eq!(revision, MAX_SERVERS as u64);
    assert_eq!(records.len(), MAX_SERVERS);
    let no_event = tokio::time::timeout(Duration::from_millis(20), changes.recv()).await;
    assert!(no_event.is_err());

    harness.shutdown().await;
}

#[tokio::test]
async fn non_launch_update_preserves_authorization_but_launch_update_revokes_it() {
    let harness = TestHarness::new();
    let added = harness
        .service
        .add_server(create_input("update fixture"))
        .expect("add server");
    let authorized = authorize(&harness.service, &added.server);
    let enabled = harness
        .service
        .enable_server(mutation_input(&authorized.server))
        .expect("enable server without creating a Manager connection");
    let id = server_id(&enabled.server);
    let before_rename = harness
        .registry
        .get_persisted(id)
        .expect("read enabled server")
        .expect("enabled server exists");
    let original_authorization = before_rename
        .launch_authorization
        .clone()
        .expect("authorization exists");

    let renamed = harness
        .service
        .update_server(update_input(
            &enabled.server,
            "renamed update fixture",
            enabled.server.arguments.clone(),
        ))
        .await
        .expect("non-launch update");
    let after_rename = harness
        .registry
        .get_persisted(id)
        .expect("read renamed server")
        .expect("renamed server exists");
    assert!(renamed.server.summary.enabled);
    assert_eq!(
        renamed.server.summary.launch_authorization_state,
        McpLaunchAuthorizationStateDto::Authorized
    );
    assert_ne!(
        after_rename.entry.config_epoch,
        before_rename.entry.config_epoch
    );
    assert_ne!(
        after_rename.entry.config_digest,
        before_rename.entry.config_digest
    );
    assert_eq!(
        after_rename.launch_spec_digest,
        before_rename.launch_spec_digest
    );
    let rebound_authorization = after_rename
        .launch_authorization
        .as_ref()
        .expect("non-launch update retains authorization");
    assert_eq!(
        rebound_authorization.authored_config_epoch,
        after_rename.entry.config_epoch
    );
    assert_eq!(
        rebound_authorization.authored_config_digest,
        after_rename.entry.config_digest
    );
    assert_ne!(
        rebound_authorization.authored_config_epoch,
        original_authorization.authored_config_epoch
    );
    assert_eq!(
        rebound_authorization.launch_spec_digest,
        original_authorization.launch_spec_digest
    );

    let launch_changed = harness
        .service
        .update_server(update_input(
            &renamed.server,
            "renamed update fixture",
            vec![
                "--different-fixture".to_string(),
                String::new(),
                "--tail".to_string(),
            ],
        ))
        .await
        .expect("launch update must stop cleanly even without a prior Manager entry");
    let after_launch_change = harness
        .registry
        .get_persisted(id)
        .expect("read launch-updated server")
        .expect("launch-updated server exists");
    assert!(!launch_changed.server.summary.enabled);
    assert_eq!(
        launch_changed.server.summary.trust,
        McpTrustLevelDto::Untrusted
    );
    assert_eq!(
        launch_changed.server.summary.launch_authorization_state,
        McpLaunchAuthorizationStateDto::Required
    );
    assert!(after_launch_change.launch_authorization.is_none());
    assert_ne!(
        after_launch_change.launch_spec_digest,
        before_rename.launch_spec_digest
    );

    harness.shutdown().await;
}

#[tokio::test]
async fn disable_and_delete_succeed_without_a_preexisting_manager_entry() {
    let harness = TestHarness::new();

    let disable_added = harness
        .service
        .add_server(create_input("disable fixture"))
        .expect("add disable fixture");
    let disable_authorized = authorize(&harness.service, &disable_added.server);
    let disable_enabled = harness
        .service
        .enable_server(mutation_input(&disable_authorized.server))
        .expect("enable without starting");
    let disabled = harness
        .service
        .disable_server(mutation_input(&disable_enabled.server))
        .await
        .expect("disable must construct and stop an inert Manager entry");
    assert!(!disabled.server.summary.enabled);
    assert_eq!(
        disabled.server.summary.state,
        McpConnectionStateDto::Disabled
    );

    let delete_added = harness
        .service
        .add_server(create_input("delete fixture"))
        .expect("add delete fixture");
    let delete_authorized = authorize(&harness.service, &delete_added.server);
    let delete_enabled = harness
        .service
        .enable_server(mutation_input(&delete_authorized.server))
        .expect("enable without starting");
    let delete_id = server_id(&delete_enabled.server);
    let deleted = harness
        .service
        .delete_server(mutation_input(&delete_enabled.server))
        .await
        .expect("delete must stop an inert Manager entry before removing Registry state");
    assert!(!deleted.server.summary.enabled);
    assert_eq!(
        deleted.server.summary.state,
        McpConnectionStateDto::Disabled
    );
    assert!(harness
        .registry
        .get_persisted(delete_id)
        .expect("read deleted identity")
        .is_none());

    harness.shutdown().await;
}
