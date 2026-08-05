use std::sync::Arc;
use std::time::Duration;

use mycopilot_mcp_client::McpEnvBinding;
use tempfile::tempdir;

use super::*;

const FORBIDDEN_CANARY: &str = "MCP_REGISTRY_SECRET_CANARY";

fn config(id: McpServerId, display_name: &str) -> McpServerConfig {
    McpServerConfig {
        id,
        display_name: display_name.to_string(),
        scope: McpServerScope::User,
        trust: McpTrustLevel::Untrusted,
        approval_mode: McpApprovalMode::Prompt,
        enabled: false,
        transport: McpTransportConfig::Stdio(McpStdioConfig {
            program: std::env::current_exe().expect("resolve repository-owned test executable"),
            arguments: vec!["--mode".to_string(), "stdio".to_string(), String::new()],
            cwd: std::env::current_dir().expect("resolve repository-owned test cwd"),
            environment: Vec::new(),
        }),
        connect_timeout_ms: 1_000,
        request_timeout_ms: 60_000,
        shutdown_timeout_ms: 2_000,
    }
}

#[test]
fn migration_preserves_existing_database_and_uses_explicit_columns() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("registry.sqlite");
    let legacy = Connection::open(&path).unwrap();
    legacy
        .execute_batch(
            "PRAGMA user_version = 73;
             CREATE TABLE existing_application_data (
                 id INTEGER PRIMARY KEY,
                 value TEXT NOT NULL
             );
             INSERT INTO existing_application_data(value) VALUES ('preserve-me');",
        )
        .unwrap();
    drop(legacy);

    let registry = SqliteMcpRegistry::open(&path).unwrap();
    assert_eq!(registry.current_revision().unwrap(), 0);
    drop(registry);

    let inspection = Connection::open(&path).unwrap();
    assert_eq!(
        inspection
            .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        73
    );
    assert_eq!(
        inspection
            .query_row("SELECT value FROM existing_application_data", [], |row| row
                .get::<_, String>(0))
            .unwrap(),
        "preserve-me"
    );
    let columns = inspection
        .prepare("PRAGMA table_info(mcp_registry_servers)")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(1))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert!(!columns.iter().any(|column| {
        matches!(
            column.as_str(),
            "config_json"
                | "environment"
                | "headers"
                | "bearer_token"
                | "oauth"
                | "stderr"
                | "tool_result"
                | "tool_arguments"
        )
    }));
}

#[test]
fn migration_adds_auto_mode_without_changing_v1_identity_or_authorization() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("registry.sqlite");
    let prompt_id = McpServerId::new();
    let deny_id = McpServerId::new();
    let registry = SqliteMcpRegistry::open(&path).unwrap();

    let prompt = registry
        .add_persisted(config(prompt_id, "prompt fixture"))
        .unwrap();
    let prompt_record = registry.get_persisted(prompt_id).unwrap().unwrap();
    let file_identity = compute_launch_file_identity_digest(
        &prompt_record.entry.config,
        &prompt_record.launch_spec_digest,
    )
    .unwrap();
    registry
        .authorize_launch(
            &McpRegistryMutationPrecondition::from_entry(&prompt),
            &prompt_record.launch_spec_digest,
            &file_identity,
            MCP_LAUNCH_AUTHORIZATION_POLICY_VERSION,
            1_785_384_000_000,
        )
        .unwrap();
    let authorized = registry.get_persisted(prompt_id).unwrap().unwrap();
    let enabled = registry
        .set_enabled(
            &McpRegistryMutationPrecondition::from_entry(&authorized.entry),
            true,
        )
        .unwrap();
    let before = registry.get_persisted(prompt_id).unwrap().unwrap();

    let mut deny_config = config(deny_id, "deny fixture");
    deny_config.approval_mode = McpApprovalMode::Deny;
    let deny = registry.add_persisted(deny_config).unwrap();
    let revision_before_restart = registry.current_revision().unwrap();
    drop(registry);

    let connection = Connection::open(&path).unwrap();
    let current_sql = connection
        .query_row(
            "SELECT sql FROM sqlite_schema
             WHERE type = 'table' AND name = 'mcp_registry_servers'",
            [],
            |row| row.get::<_, String>(0),
        )
        .unwrap();
    let legacy_sql = current_sql.replace(
        "approval_mode IN ('prompt', 'auto', 'deny')",
        "approval_mode IN ('prompt', 'deny')",
    );
    assert_ne!(legacy_sql, current_sql);
    let columns = REGISTRY_SERVER_COLUMNS.join(", ");
    connection
        .execute_batch(
            "ALTER TABLE mcp_registry_servers
                 RENAME TO mcp_registry_servers_auto_source;
             DROP INDEX mcp_registry_servers_revision;",
        )
        .unwrap();
    connection.execute_batch(&legacy_sql).unwrap();
    connection
        .execute_batch(&format!(
            "INSERT INTO mcp_registry_servers ({columns})
             SELECT {columns} FROM mcp_registry_servers_auto_source;
             DROP TABLE mcp_registry_servers_auto_source;
             CREATE INDEX mcp_registry_servers_revision
             ON mcp_registry_servers(registry_revision, server_id);"
        ))
        .unwrap();
    drop(connection);

    let reopened = SqliteMcpRegistry::open(&path).unwrap();
    assert_eq!(
        reopened.current_revision().unwrap(),
        revision_before_restart
    );
    let restored_prompt = reopened.get_persisted(prompt_id).unwrap().unwrap();
    assert_eq!(restored_prompt.entry.config.id, prompt_id);
    assert_eq!(
        restored_prompt.entry.config_epoch,
        before.entry.config_epoch
    );
    assert_eq!(
        restored_prompt.entry.config_digest,
        before.entry.config_digest
    );
    assert_eq!(restored_prompt.entry.revision, enabled.revision);
    assert!(restored_prompt.entry.config.enabled);
    assert_eq!(
        restored_prompt.entry.config.approval_mode,
        McpApprovalMode::Prompt
    );
    assert_eq!(
        restored_prompt.launch_authorization,
        before.launch_authorization
    );
    let restored_deny = reopened.get_persisted(deny_id).unwrap().unwrap();
    assert_eq!(restored_deny.entry.config_epoch, deny.config_epoch);
    assert_eq!(restored_deny.entry.config_digest, deny.config_digest);
    assert_eq!(restored_deny.entry.revision, deny.revision);
    assert_eq!(
        restored_deny.entry.config.approval_mode,
        McpApprovalMode::Deny
    );

    let mut auto_config = restored_prompt.entry.config.clone();
    auto_config.approval_mode = McpApprovalMode::Auto;
    let updated = match reopened
        .update_with_precondition(
            &McpRegistryMutationPrecondition::from_entry(&restored_prompt.entry),
            auto_config,
        )
        .unwrap()
    {
        McpRegistryMutation::Updated(entry) => entry,
        other => panic!("approval-mode change must update the record: {other:?}"),
    };
    assert_eq!(updated.config.id, prompt_id);
    assert_eq!(updated.config.approval_mode, McpApprovalMode::Auto);
    assert_ne!(updated.config_epoch, before.entry.config_epoch);
    assert_ne!(updated.config_digest, before.entry.config_digest);
    assert!(updated.revision > revision_before_restart);
    let persisted_auto = reopened.get_persisted(prompt_id).unwrap().unwrap();
    assert_eq!(
        persisted_auto.entry.config.approval_mode,
        McpApprovalMode::Auto
    );
    assert!(persisted_auto.launch_authorization.is_some());
}

#[test]
fn migration_quarantines_an_incompatible_same_name_table_without_losing_its_data() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("registry.sqlite");
    let registry = SqliteMcpRegistry::open(&path).unwrap();
    drop(registry);

    let legacy = Connection::open(&path).unwrap();
    legacy
        .execute_batch(
            "DROP TABLE mcp_registry_servers;
             CREATE TABLE mcp_registry_servers (
                 server_id TEXT PRIMARY KEY,
                 legacy_value TEXT NOT NULL
             );
             INSERT INTO mcp_registry_servers(server_id, legacy_value)
             VALUES ('legacy-server', 'LEGACY_MCP_ROW_CANARY');",
        )
        .unwrap();
    drop(legacy);

    let reopened = SqliteMcpRegistry::open(&path).unwrap();
    let (revision, records) = reopened.snapshot().unwrap();
    assert_eq!(revision, 0);
    assert!(records.is_empty());
    drop(reopened);

    let inspection = Connection::open(&path).unwrap();
    assert_eq!(
        inspection
            .query_row(
                "SELECT legacy_value
                 FROM mcp_registry_servers_incompatible_v1_1
                 WHERE server_id = 'legacy-server'",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
        "LEGACY_MCP_ROW_CANARY"
    );
    let current_columns = inspection
        .prepare("PRAGMA table_info(mcp_registry_servers)")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(1))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert!(column_names_match(
        &current_columns,
        REGISTRY_SERVER_COLUMNS
    ));
}

#[test]
fn registry_restores_identity_and_advances_revision_across_restart() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("registry.sqlite");
    let id = McpServerId::new();
    let registry = SqliteMcpRegistry::open(&path).unwrap();
    let first = registry.add(config(id, "first")).unwrap();
    let first_epoch = first.config_epoch;
    let first_digest = first.config_digest.clone();
    let first_namespace = first.model_namespace.clone();
    assert_eq!(first.revision, 1);
    drop(registry);

    let reopened = SqliteMcpRegistry::open(&path).unwrap();
    let restored = reopened.get(id).unwrap().unwrap();
    assert_eq!(restored.config_epoch, first_epoch);
    assert_eq!(restored.config_digest, first_digest);
    assert_eq!(restored.model_namespace, first_namespace);
    assert_eq!(restored.revision, 1);
    assert_eq!(reopened.current_revision().unwrap(), 1);

    let mut updated = restored.config.clone();
    updated.display_name = "updated".to_string();
    let changed = reopened
        .update_with_precondition(
            &McpRegistryMutationPrecondition::from_entry(&restored),
            updated,
        )
        .unwrap();
    let McpRegistryMutation::Updated(changed) = changed else {
        panic!("expected updated record");
    };
    assert_eq!(changed.revision, 2);
    assert_ne!(changed.config_epoch, first_epoch);
    assert_ne!(changed.config_digest, first_digest);
    assert_eq!(changed.model_namespace, first_namespace);
    drop(reopened);

    let reopened = SqliteMcpRegistry::open(&path).unwrap();
    assert_eq!(reopened.current_revision().unwrap(), 2);
    assert_eq!(reopened.get(id).unwrap().unwrap(), changed);
}

#[test]
fn legacy_database_backfills_model_namespace_without_changing_authorization_identity() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("registry.sqlite");
    let id = McpServerId::new();
    let registry = SqliteMcpRegistry::open(&path).unwrap();
    let added = registry.add(config(id, "Filesystem Test")).unwrap();
    let persisted = registry.get_persisted(id).unwrap().unwrap();
    let file_identity_digest =
        compute_launch_file_identity_digest(&persisted.entry.config, &persisted.launch_spec_digest)
            .unwrap();
    registry
        .authorize_launch(
            &McpRegistryMutationPrecondition::from_entry(&added),
            &persisted.launch_spec_digest,
            &file_identity_digest,
            MCP_LAUNCH_AUTHORIZATION_POLICY_VERSION,
            1,
        )
        .unwrap();
    let authorized = registry.get_persisted(id).unwrap().unwrap();
    let enabled = registry
        .set_enabled(
            &McpRegistryMutationPrecondition::from_entry(&authorized.entry),
            true,
        )
        .unwrap();
    let before = registry.get_persisted(id).unwrap().unwrap();
    assert_eq!(enabled.model_namespace.as_str(), "filesystem_test");
    drop(registry);

    let legacy = Connection::open(&path).unwrap();
    legacy
        .execute_batch("DROP TABLE mcp_registry_model_namespaces;")
        .unwrap();
    drop(legacy);

    let reopened = SqliteMcpRegistry::open(&path).unwrap();
    let after = reopened.get_persisted(id).unwrap().unwrap();
    assert_eq!(after.entry.model_namespace.as_str(), "filesystem_test");
    assert_eq!(after.entry.config_digest, before.entry.config_digest);
    assert_eq!(after.entry.config_epoch, before.entry.config_epoch);
    assert_eq!(after.entry.revision, before.entry.revision);
    assert_eq!(after.launch_authorization, before.launch_authorization);
    assert!(after.entry.config.enabled);
    assert_eq!(reopened.current_revision().unwrap(), before.entry.revision);
}

#[test]
fn malformed_namespace_storage_class_is_repaired_without_hiding_healthy_servers() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("registry.sqlite");
    let damaged_id = McpServerId::new();
    let healthy_id = McpServerId::new();
    let registry = SqliteMcpRegistry::open(&path).unwrap();
    registry.add(config(damaged_id, "Filesystem Test")).unwrap();
    let healthy = registry.add(config(healthy_id, "Memory Test")).unwrap();
    drop(registry);

    let connection = Connection::open(&path).unwrap();
    connection
        .execute(
            "UPDATE mcp_registry_model_namespaces
             SET model_namespace = x'00ff'
             WHERE server_id = ?1",
            [damaged_id.to_string()],
        )
        .unwrap();
    drop(connection);

    let reopened = SqliteMcpRegistry::open(&path).unwrap();
    let records = reopened.list().unwrap();
    assert_eq!(records.len(), 2);
    assert_eq!(
        reopened.get(damaged_id).unwrap().unwrap().model_namespace,
        McpModelNamespace::from_str("filesystem_test").unwrap()
    );
    assert_eq!(
        reopened.get(healthy_id).unwrap().unwrap().model_namespace,
        healthy.model_namespace
    );
}

#[test]
fn noncanonical_namespace_table_is_rebuilt_with_primary_and_unique_constraints() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("registry.sqlite");
    let first_id = McpServerId::new();
    let second_id = McpServerId::new();
    let registry = SqliteMcpRegistry::open(&path).unwrap();
    registry.add(config(first_id, "Filesystem Test")).unwrap();
    registry.add(config(second_id, "Filesystem/Test")).unwrap();
    drop(registry);

    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch(
            "DROP TABLE mcp_registry_model_namespaces;
             CREATE TABLE mcp_registry_model_namespaces (
                 schema_version INTEGER,
                 server_id TEXT,
                 model_namespace TEXT,
                 created_at INTEGER
             );",
        )
        .unwrap();
    for server_id in [first_id, second_id] {
        connection
            .execute(
                "INSERT INTO mcp_registry_model_namespaces (
                     schema_version, server_id, model_namespace, created_at
                 ) VALUES (1, ?1, 'filesystem_test', 1)",
                [server_id.to_string()],
            )
            .unwrap();
    }
    drop(connection);

    let reopened = SqliteMcpRegistry::open(&path).unwrap();
    let first = reopened.get(first_id).unwrap().unwrap();
    let second = reopened.get(second_id).unwrap().unwrap();
    assert_ne!(first.model_namespace, second.model_namespace);
    assert!([
        first.model_namespace.as_str(),
        second.model_namespace.as_str()
    ]
    .contains(&"filesystem_test"));
    drop(reopened);

    let connection = Connection::open(&path).unwrap();
    assert!(connection
        .execute(
            "INSERT INTO mcp_registry_model_namespaces (
                 schema_version, server_id, model_namespace, created_at
             ) VALUES (1, 'duplicate-server', 'filesystem_test', 1)",
            [],
        )
        .is_err());
    assert!(connection
        .execute(
            "INSERT INTO mcp_registry_model_namespaces (
                 schema_version, server_id, model_namespace, created_at
             ) VALUES (1, ?1, 'another_namespace', 1)",
            [first_id.to_string()],
        )
        .is_err());
}

#[test]
fn structurally_misleading_namespace_tables_are_quarantined_and_rebuilt() {
    let cases = [
        (
            "without-rowid",
            "CREATE TABLE mcp_registry_model_namespaces (
                 schema_version INTEGER NOT NULL,
                 server_id TEXT PRIMARY KEY,
                 model_namespace TEXT NOT NULL UNIQUE,
                 created_at INTEGER NOT NULL
             ) WITHOUT\nROWID;",
        ),
        (
            "compound-primary-key",
            "CREATE TABLE mcp_registry_model_namespaces (
                 schema_version INTEGER NOT NULL,
                 server_id TEXT NOT NULL,
                 model_namespace TEXT NOT NULL UNIQUE,
                 created_at INTEGER NOT NULL,
                 PRIMARY KEY (server_id, created_at)
             );",
        ),
        (
            "partial-unique-index",
            "CREATE TABLE mcp_registry_model_namespaces (
                 schema_version INTEGER NOT NULL,
                 server_id TEXT PRIMARY KEY,
                 model_namespace TEXT NOT NULL,
                 created_at INTEGER NOT NULL
             );
             CREATE UNIQUE INDEX misleading_namespace_unique
             ON mcp_registry_model_namespaces(model_namespace)
             WHERE created_at < 0;",
        ),
        (
            "expression-unique-index",
            "CREATE TABLE mcp_registry_model_namespaces (
                 schema_version INTEGER NOT NULL,
                 server_id TEXT PRIMARY KEY,
                 model_namespace TEXT NOT NULL,
                 created_at INTEGER NOT NULL
             );
             CREATE UNIQUE INDEX misleading_namespace_expression_unique
             ON mcp_registry_model_namespaces(lower(model_namespace));",
        ),
    ];

    for (case, replacement_schema) in cases {
        let directory = tempdir().unwrap();
        let path = directory.path().join(format!("{case}.sqlite"));
        let id = McpServerId::new();
        let registry = SqliteMcpRegistry::open(&path).unwrap();
        registry.add(config(id, "Filesystem Test")).unwrap();
        drop(registry);

        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(&format!(
                "DROP TABLE mcp_registry_model_namespaces; {replacement_schema}"
            ))
            .unwrap();
        connection
            .execute(
                "INSERT INTO mcp_registry_model_namespaces (
                     schema_version, server_id, model_namespace, created_at
                 ) VALUES (1, ?1, 'filesystem_test', 1)",
                [id.to_string()],
            )
            .unwrap();
        drop(connection);

        let reopened = SqliteMcpRegistry::open(&path)
            .unwrap_or_else(|error| panic!("{case} namespace table should be rebuilt: {error:?}"));
        let restored = reopened.get(id).unwrap().unwrap();
        assert_eq!(restored.model_namespace.as_str(), "filesystem_test");
        drop(reopened);

        let connection = Connection::open(&path).unwrap();
        assert!(connection
            .execute(
                "INSERT INTO mcp_registry_model_namespaces (
                     schema_version, server_id, model_namespace, created_at
                 ) VALUES (1, 'another-server', 'filesystem_test', 1)",
                [],
            )
            .is_err());
    }
}

#[test]
fn snapshot_pairs_records_with_the_same_revision_watermark_under_writes() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("registry.sqlite");
    let registry = Arc::new(SqliteMcpRegistry::open(&path).unwrap());
    let writer_registry = Arc::clone(&registry);
    let writer = std::thread::spawn(move || {
        for index in 0..64 {
            writer_registry
                .add_persisted(config(McpServerId::new(), &format!("snapshot-{index}")))
                .unwrap();
        }
    });

    for _ in 0..10_000 {
        let (revision, records) = registry.snapshot().unwrap();
        assert_eq!(revision, records.len() as u64);
        if revision == 64 {
            break;
        }
        std::thread::yield_now();
    }
    writer.join().unwrap();
    let (revision, records) = registry.snapshot().unwrap();
    assert_eq!(revision, 64);
    assert_eq!(records.len(), 64);
}

#[test]
fn registry_revision_never_exceeds_the_javascript_safe_integer_boundary() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("registry.sqlite");
    let registry = SqliteMcpRegistry::open(&path).unwrap();
    {
        let connection = registry.lock_connection().unwrap();
        connection
            .execute(
                "UPDATE mcp_registry_metadata
                 SET revision_watermark = ?1
                 WHERE singleton = ?2",
                params![
                    i64::try_from(MAX_WIRE_SAFE_INTEGER).unwrap(),
                    REGISTRY_METADATA_SINGLETON
                ],
            )
            .unwrap();
    }
    let mut changes = registry.subscribe();
    assert_eq!(
        registry
            .add_persisted(config(McpServerId::new(), "revision-overflow"))
            .unwrap_err(),
        McpRegistryPersistenceError::RevisionExhausted
    );
    assert_eq!(registry.current_revision().unwrap(), MAX_WIRE_SAFE_INTEGER);
    assert!(registry.list_persisted().unwrap().is_empty());
    let no_event = tokio_test_block_on(async {
        tokio::time::timeout(Duration::from_millis(20), changes.recv()).await
    });
    assert!(no_event.is_err());
}

#[test]
fn remove_and_readd_never_reuse_identity_or_revision() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("registry.sqlite");
    let id = McpServerId::new();
    let registry = SqliteMcpRegistry::open(&path).unwrap();
    let first = registry.add(config(id, "same")).unwrap();
    let removed = registry.remove(id).unwrap().unwrap();
    assert_eq!(removed.revision, 2);
    let second = registry.add(config(id, "same")).unwrap();
    assert_eq!(second.revision, 3);
    assert_ne!(second.config_epoch, first.config_epoch);
    assert_eq!(second.config_digest, first.config_digest);
}

#[test]
fn environment_is_rejected_and_never_persisted() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("registry.sqlite");
    let registry = SqliteMcpRegistry::open(&path).unwrap();
    let mut invalid = config(McpServerId::new(), "invalid");
    let McpTransportConfig::Stdio(stdio) = &mut invalid.transport else {
        panic!("test config must use stdio");
    };
    stdio.environment.push(McpEnvBinding::Plain {
        name: "NEUTRAL".to_string(),
        value: FORBIDDEN_CANARY.to_string(),
    });
    assert!(registry.add(invalid).is_err());
    drop(registry);
    let bytes = std::fs::read(&path).unwrap();
    assert!(!String::from_utf8_lossy(&bytes).contains(FORBIDDEN_CANARY));
}

#[test]
fn exact_launch_authorization_preserves_empty_and_ordered_arguments() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("registry.sqlite");
    let id = McpServerId::new();
    let registry = SqliteMcpRegistry::open(&path).unwrap();
    let added = registry.add(config(id, "authorized")).unwrap();
    assert_eq!(
        registry
            .set_enabled(&McpRegistryMutationPrecondition::from_entry(&added), true)
            .unwrap_err(),
        McpRegistryPersistenceError::AuthorizationRequired
    );
    let before = registry.get_persisted(id).unwrap().unwrap();
    let file_identity_digest =
        compute_launch_file_identity_digest(&before.entry.config, &before.launch_spec_digest)
            .unwrap();
    let authorization = registry
        .authorize_launch(
            &McpRegistryMutationPrecondition::from_entry(&before.entry),
            &before.launch_spec_digest,
            &file_identity_digest,
            MCP_LAUNCH_AUTHORIZATION_POLICY_VERSION,
            123_456,
        )
        .unwrap();
    assert_eq!(authorization.server_id, id);
    assert_eq!(
        authorization.authorization_format_version,
        MCP_LAUNCH_AUTHORIZATION_FORMAT_VERSION
    );
    assert_eq!(
        authorization.authorization_policy_version,
        MCP_LAUNCH_AUTHORIZATION_POLICY_VERSION
    );
    let inspection = Connection::open(&path).unwrap();
    assert_eq!(
        inspection
            .query_row(
                "SELECT authorization_format_version
                 FROM mcp_registry_servers WHERE server_id = ?1",
                [id.to_string()],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        i64::from(MCP_LAUNCH_AUTHORIZATION_FORMAT_VERSION)
    );
    drop(inspection);
    let authorized_entry = registry.get(id).unwrap().unwrap();
    assert_eq!(authorized_entry.config.trust, McpTrustLevel::UserApproved);
    let enabled = registry
        .set_enabled(
            &McpRegistryMutationPrecondition::from_entry(&authorized_entry),
            true,
        )
        .unwrap();
    assert!(enabled.config.enabled);
    assert_eq!(
        registry
            .get_persisted(id)
            .unwrap()
            .unwrap()
            .launch_authorization
            .unwrap()
            .launch_spec_digest,
        authorization.launch_spec_digest
    );

    let mut launch_drift = enabled.config.clone();
    let McpTransportConfig::Stdio(stdio) = &mut launch_drift.transport else {
        panic!("test config must use stdio");
    };
    stdio.arguments.swap(0, 1);
    let changed = registry
        .update_with_precondition(
            &McpRegistryMutationPrecondition::from_entry(&enabled),
            launch_drift,
        )
        .unwrap();
    let McpRegistryMutation::Updated(changed) = changed else {
        panic!("expected launch mutation");
    };
    assert!(!changed.config.enabled);
    assert_eq!(changed.config.trust, McpTrustLevel::Untrusted);
    assert!(registry
        .get_persisted(id)
        .unwrap()
        .unwrap()
        .launch_authorization
        .is_none());
}

#[test]
fn launch_identity_tracks_code_not_mutable_data_and_never_blocks_disable() {
    let directory = tempdir().unwrap();
    let executable = directory.path().join("owned-fixture-executable");
    let script = directory.path().join("owned-server.js");
    let mutable_data = directory.path().join("owned-fixture-data");
    std::fs::write(&executable, b"owned executable version one").unwrap();
    std::fs::write(&script, b"owned script version one").unwrap();
    std::fs::write(&mutable_data, b"mutable data version one").unwrap();
    let id = McpServerId::new();
    let mut server = config(id, "selective identity");
    let McpTransportConfig::Stdio(stdio) = &mut server.transport else {
        panic!("test config must use stdio");
    };
    stdio.program = executable.clone();
    stdio.arguments = vec![
        script.to_string_lossy().into_owned(),
        mutable_data.to_string_lossy().into_owned(),
        "--config=missing.js".to_string(),
        "https://owned.invalid/remote-server.js".to_string(),
    ];
    stdio.cwd = directory.path().to_path_buf();
    let registry = SqliteMcpRegistry::open(directory.path().join("registry.sqlite")).unwrap();
    let added = registry.add(server).unwrap();
    let persisted = registry.get_persisted(id).unwrap().unwrap();
    let file_identity_digest =
        compute_launch_file_identity_digest(&persisted.entry.config, &persisted.launch_spec_digest)
            .unwrap();
    registry
        .authorize_launch(
            &McpRegistryMutationPrecondition::from_entry(&added),
            &persisted.launch_spec_digest,
            &file_identity_digest,
            MCP_LAUNCH_AUTHORIZATION_POLICY_VERSION,
            1,
        )
        .unwrap();

    // Extensionless runtime data and unknown option values are not
    // misclassified as executable code authority.
    std::fs::write(&mutable_data, b"mutable data version two with a new size").unwrap();
    let authorized = registry.get(id).unwrap().unwrap();
    let enabled = registry
        .set_enabled(
            &McpRegistryMutationPrecondition::from_entry(&authorized),
            true,
        )
        .unwrap();

    // Executable replacement invalidates future starts, but stale launch
    // identity must never prevent the fail-closed disable path.
    std::fs::write(&executable, b"owned executable version two with a new size").unwrap();
    let disabled = registry
        .set_enabled(
            &McpRegistryMutationPrecondition::from_entry(&enabled),
            false,
        )
        .unwrap();
    assert!(!disabled.config.enabled);
    assert_eq!(
        registry
            .set_enabled(
                &McpRegistryMutationPrecondition::from_entry(&disabled),
                true,
            )
            .unwrap_err(),
        McpRegistryPersistenceError::AuthorizationRequired
    );
}

#[cfg(unix)]
#[test]
fn launch_identity_ignores_metadata_only_ctime_drift() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let directory = tempdir().unwrap();
    let executable = directory.path().join("owned-fixture-executable");
    std::fs::write(&executable, b"stable owned executable bytes").unwrap();
    let mut server = config(McpServerId::new(), "stable physical identity");
    let McpTransportConfig::Stdio(stdio) = &mut server.transport else {
        panic!("test config must use stdio");
    };
    stdio.program = executable.clone();
    stdio.cwd = directory.path().to_path_buf();

    let launch_digest = compute_launch_spec_digest(&server).unwrap();
    let first = compute_launch_file_identity_digest(&server, &launch_digest).unwrap();
    let before = std::fs::metadata(&executable).unwrap();
    let original_mode = before.permissions().mode();
    let mut changed_permissions = before.permissions();
    changed_permissions.set_mode(original_mode ^ 0o100);
    std::fs::set_permissions(&executable, changed_permissions).unwrap();
    let mut restored_permissions = std::fs::metadata(&executable).unwrap().permissions();
    restored_permissions.set_mode(original_mode);
    std::fs::set_permissions(&executable, restored_permissions).unwrap();
    let after = std::fs::metadata(&executable).unwrap();
    assert_ne!(
        (before.ctime(), before.ctime_nsec()),
        (after.ctime(), after.ctime_nsec()),
        "the fixture must exercise a ctime-only identity change"
    );

    let second = compute_launch_file_identity_digest(&server, &launch_digest).unwrap();
    assert_eq!(
        first, second,
        "metadata bookkeeping must not revoke unchanged launch bytes"
    );
}

#[cfg(unix)]
#[test]
fn authorized_server_survives_metadata_only_ctime_drift_across_registry_restart() {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempdir().unwrap();
    let database = directory.path().join("registry.sqlite");
    let executable = directory.path().join("owned-fixture-executable");
    std::fs::write(&executable, b"stable owned executable bytes").unwrap();
    let id = McpServerId::new();
    {
        let registry = SqliteMcpRegistry::open(&database).unwrap();
        let mut server = config(id, "restart-stable physical identity");
        let McpTransportConfig::Stdio(stdio) = &mut server.transport else {
            panic!("test config must use stdio");
        };
        stdio.program = executable.clone();
        stdio.cwd = directory.path().to_path_buf();
        let added = registry.add(server).unwrap();
        let persisted = registry.get_persisted(id).unwrap().unwrap();
        let file_identity_digest = compute_launch_file_identity_digest(
            &persisted.entry.config,
            &persisted.launch_spec_digest,
        )
        .unwrap();
        registry
            .authorize_launch(
                &McpRegistryMutationPrecondition::from_entry(&added),
                &persisted.launch_spec_digest,
                &file_identity_digest,
                MCP_LAUNCH_AUTHORIZATION_POLICY_VERSION,
                1,
            )
            .unwrap();
        let authorized = registry.get(id).unwrap().unwrap();
        registry
            .set_enabled(
                &McpRegistryMutationPrecondition::from_entry(&authorized),
                true,
            )
            .unwrap();
    }

    let before = std::fs::metadata(&executable).unwrap();
    let original_mode = before.permissions().mode();
    let mut changed_permissions = before.permissions();
    changed_permissions.set_mode(original_mode ^ 0o100);
    std::fs::set_permissions(&executable, changed_permissions).unwrap();
    let mut restored_permissions = std::fs::metadata(&executable).unwrap().permissions();
    restored_permissions.set_mode(original_mode);
    std::fs::set_permissions(&executable, restored_permissions).unwrap();

    let reopened = SqliteMcpRegistry::open(&database).unwrap();
    let recovered = reopened.get_persisted(id).unwrap().unwrap();
    assert!(recovered.entry.config.enabled);
    assert_eq!(recovered.entry.config.trust, McpTrustLevel::UserApproved);
    assert!(recovered.launch_authorization.is_some());
    assert!(launch_authorization_is_valid(&recovered));
}

#[test]
fn launch_identity_hashes_same_length_content_replacement() {
    let directory = tempdir().unwrap();
    let executable = directory.path().join("owned-fixture-executable");
    std::fs::write(&executable, b"owned executable version 1").unwrap();
    let mut server = config(McpServerId::new(), "content-bound identity");
    let McpTransportConfig::Stdio(stdio) = &mut server.transport else {
        panic!("test config must use stdio");
    };
    stdio.program = executable.clone();
    stdio.cwd = directory.path().to_path_buf();

    let launch_digest = compute_launch_spec_digest(&server).unwrap();
    let first = compute_launch_file_identity_digest(&server, &launch_digest).unwrap();
    std::fs::write(&executable, b"owned executable version 2").unwrap();
    let second = compute_launch_file_identity_digest(&server, &launch_digest).unwrap();

    assert_ne!(first, second, "launch authority must remain content-bound");
}

#[test]
fn launch_identity_bounds_content_hashing_for_large_runtime_binaries() {
    let directory = tempdir().unwrap();
    let executable = directory.path().join("oversized-owned-fixture-executable");
    let file = std::fs::File::create(&executable).unwrap();
    file.set_len(MAX_LAUNCH_CONTENT_HASH_FILE_BYTES + 1)
        .unwrap();
    let mut server = config(McpServerId::new(), "bounded physical identity");
    let McpTransportConfig::Stdio(stdio) = &mut server.transport else {
        panic!("test config must use stdio");
    };
    stdio.program = executable;
    stdio.cwd = directory.path().to_path_buf();

    let launch_digest = compute_launch_spec_digest(&server).unwrap();
    compute_launch_file_identity_digest(&server, &launch_digest)
        .expect("large binaries use bounded metadata identity without reading sparse contents");
}

#[test]
fn explicit_inline_code_paths_are_identity_bound() {
    let directory = tempdir().unwrap();
    let executable = directory.path().join("owned-fixture-executable");
    let imported = directory.path().join("owned-import.js");
    std::fs::write(&executable, b"owned executable").unwrap();
    std::fs::write(&imported, b"owned import version one").unwrap();
    let mut server = config(McpServerId::new(), "inline code identity");
    let McpTransportConfig::Stdio(stdio) = &mut server.transport else {
        panic!("test config must use stdio");
    };
    stdio.program = executable;
    stdio.cwd = directory.path().to_path_buf();
    stdio.arguments = vec![
        format!("--import={}", imported.to_string_lossy()),
        "--import=file:///owned/remote-style.js".to_string(),
        "--loader=owned-package.js".to_string(),
    ];
    let launch_digest = compute_launch_spec_digest(&server).unwrap();
    let first = compute_launch_file_identity_digest(&server, &launch_digest).unwrap();
    std::fs::write(&imported, b"owned import version two with a new size").unwrap();
    let second = compute_launch_file_identity_digest(&server, &launch_digest).unwrap();
    assert_ne!(first, second);
}

#[test]
fn stale_mutation_precondition_fails_without_an_event() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("registry.sqlite");
    let id = McpServerId::new();
    let registry = SqliteMcpRegistry::open(&path).unwrap();
    let mut changes = registry.subscribe();
    let added = registry.add(config(id, "first")).unwrap();
    let first_change = tokio_test_block_on(changes.recv()).unwrap();
    assert_eq!(first_change.kind, McpRegistryChangeKind::Added);
    let stale = McpRegistryMutationPrecondition::from_entry(&added);
    let mut update = added.config.clone();
    update.display_name = "second".to_string();
    registry
        .update_with_precondition(&stale, update.clone())
        .unwrap();
    let _ = tokio_test_block_on(changes.recv()).unwrap();
    update.display_name = "third".to_string();
    assert_eq!(
        registry
            .update_with_precondition(&stale, update)
            .unwrap_err(),
        McpRegistryPersistenceError::Conflict
    );
    let no_event = tokio_test_block_on(async {
        tokio::time::timeout(Duration::from_millis(20), changes.recv()).await
    });
    assert!(no_event.is_err());
}

#[test]
fn failed_database_mutation_rolls_back_revision_and_publishes_no_event() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("registry.sqlite");
    let id = McpServerId::new();
    let registry = SqliteMcpRegistry::open(&path).unwrap();
    let added = registry.add(config(id, "first")).unwrap();
    let mut changes = registry.subscribe();
    {
        let connection = registry.lock_connection().unwrap();
        connection
            .execute_batch(
                "CREATE TRIGGER reject_mcp_registry_update
                 BEFORE UPDATE ON mcp_registry_servers
                 BEGIN
                     SELECT RAISE(ABORT, 'owned test rejection');
                 END;",
            )
            .unwrap();
    }
    let mut update = added.config.clone();
    update.display_name = "second".to_string();
    assert_eq!(
        registry
            .update_with_precondition(&McpRegistryMutationPrecondition::from_entry(&added), update,)
            .unwrap_err(),
        McpRegistryPersistenceError::StorageUnavailable
    );
    assert_eq!(registry.current_revision().unwrap(), added.revision);
    assert_eq!(registry.get(id).unwrap().unwrap(), added);
    let no_event = tokio_test_block_on(async {
        tokio::time::timeout(Duration::from_millis(20), changes.recv()).await
    });
    assert!(no_event.is_err());
}

#[test]
fn non_launch_update_preserves_authorization_but_launch_change_revokes_it() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("registry.sqlite");
    let id = McpServerId::new();
    let registry = SqliteMcpRegistry::open(&path).unwrap();
    let added = registry.add(config(id, "first")).unwrap();
    let persisted = registry.get_persisted(id).unwrap().unwrap();
    let file_identity_digest =
        compute_launch_file_identity_digest(&persisted.entry.config, &persisted.launch_spec_digest)
            .unwrap();
    let authorization = registry
        .authorize_launch(
            &McpRegistryMutationPrecondition::from_entry(&added),
            &persisted.launch_spec_digest,
            &file_identity_digest,
            MCP_LAUNCH_AUTHORIZATION_POLICY_VERSION,
            1,
        )
        .unwrap();
    let authorized = registry.get(id).unwrap().unwrap();

    let mut rename = authorized.config.clone();
    rename.display_name = "renamed".to_string();
    let renamed = registry
        .update_with_precondition(
            &McpRegistryMutationPrecondition::from_entry(&authorized),
            rename,
        )
        .unwrap();
    let McpRegistryMutation::Updated(renamed) = renamed else {
        panic!("expected update");
    };
    let after_rename = registry.get_persisted(id).unwrap().unwrap();
    assert_eq!(renamed.model_namespace, authorized.model_namespace);
    assert_eq!(
        after_rename
            .launch_authorization
            .as_ref()
            .unwrap()
            .launch_spec_digest,
        authorization.launch_spec_digest
    );
    assert_eq!(renamed.config.trust, McpTrustLevel::UserApproved);

    let mut launch_change = renamed.config.clone();
    let McpTransportConfig::Stdio(stdio) = &mut launch_change.transport else {
        panic!("test config must use stdio");
    };
    stdio.arguments.pop();
    let changed = registry
        .update_with_precondition(
            &McpRegistryMutationPrecondition::from_entry(&renamed),
            launch_change,
        )
        .unwrap();
    let McpRegistryMutation::Updated(changed) = changed else {
        panic!("expected update");
    };
    assert!(!changed.config.enabled);
    assert_eq!(changed.config.trust, McpTrustLevel::Untrusted);
    assert!(registry
        .get_persisted(id)
        .unwrap()
        .unwrap()
        .launch_authorization
        .is_none());
}

#[test]
fn startup_invalidates_legacy_launch_authorization_without_losing_configuration() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("registry.sqlite");
    let id = McpServerId::new();
    {
        let registry = SqliteMcpRegistry::open(&path).unwrap();
        let added = registry.add(config(id, "legacy-authorization")).unwrap();
        let persisted = registry.get_persisted(id).unwrap().unwrap();
        let file_identity_digest = compute_launch_file_identity_digest(
            &persisted.entry.config,
            &persisted.launch_spec_digest,
        )
        .unwrap();
        registry
            .authorize_launch(
                &McpRegistryMutationPrecondition::from_entry(&added),
                &persisted.launch_spec_digest,
                &file_identity_digest,
                MCP_LAUNCH_AUTHORIZATION_POLICY_VERSION,
                1,
            )
            .unwrap();
        let authorized = registry.get(id).unwrap().unwrap();
        registry
            .set_enabled(
                &McpRegistryMutationPrecondition::from_entry(&authorized),
                true,
            )
            .unwrap();
    }
    let connection = Connection::open(&path).unwrap();
    connection
        .execute(
            "UPDATE mcp_registry_servers
             SET authorization_format_version = 1,
                 authorization_policy_version = 1,
                 authorized_launch_spec_digest = launch_spec_digest
             WHERE server_id = ?1",
            [id.to_string()],
        )
        .unwrap();
    drop(connection);

    let reopened = SqliteMcpRegistry::open(&path).unwrap();
    let recovered = reopened.get_persisted(id).unwrap().unwrap();
    assert_eq!(recovered.entry.config.display_name, "legacy-authorization");
    assert!(!recovered.entry.config.enabled);
    assert_eq!(recovered.entry.config.trust, McpTrustLevel::Untrusted);
    assert!(recovered.launch_authorization.is_none());
    assert_eq!(
        recovered.safe_error_code.as_deref(),
        Some(SAFE_ERROR_RECONCILED)
    );
}

#[test]
fn startup_requires_one_time_reauthorization_for_previous_physical_identity_policy() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("registry.sqlite");
    let id = McpServerId::new();
    {
        let registry = SqliteMcpRegistry::open(&path).unwrap();
        let added = registry
            .add(config(id, "previous-policy-authorization"))
            .unwrap();
        let persisted = registry.get_persisted(id).unwrap().unwrap();
        let file_identity_digest = compute_launch_file_identity_digest(
            &persisted.entry.config,
            &persisted.launch_spec_digest,
        )
        .unwrap();
        registry
            .authorize_launch(
                &McpRegistryMutationPrecondition::from_entry(&added),
                &persisted.launch_spec_digest,
                &file_identity_digest,
                MCP_LAUNCH_AUTHORIZATION_POLICY_VERSION,
                1,
            )
            .unwrap();
        let authorized = registry.get(id).unwrap().unwrap();
        registry
            .set_enabled(
                &McpRegistryMutationPrecondition::from_entry(&authorized),
                true,
            )
            .unwrap();
    }
    let connection = Connection::open(&path).unwrap();
    connection
        .execute(
            "UPDATE mcp_registry_servers
             SET authorization_policy_version = 2
             WHERE server_id = ?1",
            [id.to_string()],
        )
        .unwrap();
    drop(connection);

    let reopened = SqliteMcpRegistry::open(&path).unwrap();
    let recovered = reopened.get_persisted(id).unwrap().unwrap();
    assert_eq!(
        recovered.entry.config.display_name,
        "previous-policy-authorization"
    );
    assert!(!recovered.entry.config.enabled);
    assert_eq!(recovered.entry.config.trust, McpTrustLevel::Untrusted);
    assert!(recovered.launch_authorization.is_none());
    assert_eq!(
        recovered.safe_error_code.as_deref(),
        Some(SAFE_ERROR_RECONCILED)
    );
}

#[test]
fn startup_disables_v2_authorization_after_executable_replacement() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("registry.sqlite");
    let executable = directory.path().join("owned-startup-executable");
    std::fs::write(&executable, b"owned executable version one").unwrap();
    let id = McpServerId::new();
    {
        let registry = SqliteMcpRegistry::open(&path).unwrap();
        let mut server = config(id, "startup physical drift");
        let McpTransportConfig::Stdio(stdio) = &mut server.transport else {
            panic!("test config must use stdio");
        };
        stdio.program = executable.clone();
        stdio.cwd = directory.path().to_path_buf();
        let added = registry.add(server).unwrap();
        let persisted = registry.get_persisted(id).unwrap().unwrap();
        let file_identity_digest = compute_launch_file_identity_digest(
            &persisted.entry.config,
            &persisted.launch_spec_digest,
        )
        .unwrap();
        registry
            .authorize_launch(
                &McpRegistryMutationPrecondition::from_entry(&added),
                &persisted.launch_spec_digest,
                &file_identity_digest,
                MCP_LAUNCH_AUTHORIZATION_POLICY_VERSION,
                1,
            )
            .unwrap();
        let authorized = registry.get(id).unwrap().unwrap();
        registry
            .set_enabled(
                &McpRegistryMutationPrecondition::from_entry(&authorized),
                true,
            )
            .unwrap();
    }

    std::fs::write(
        &executable,
        b"owned executable version two with a different size",
    )
    .unwrap();
    let reopened = SqliteMcpRegistry::open(&path).unwrap();
    let recovered = reopened.get_persisted(id).unwrap().unwrap();
    assert!(!recovered.entry.config.enabled);
    assert_eq!(recovered.entry.config.trust, McpTrustLevel::Untrusted);
    assert!(recovered.launch_authorization.is_none());
    assert_eq!(
        recovered.safe_error_code.as_deref(),
        Some(SAFE_ERROR_RECONCILED)
    );
}

#[test]
fn exact_launch_digest_binds_server_and_argument_boundaries() {
    let first_id = McpServerId::new();
    let first = config(first_id, "first");
    let mut different_server = first.clone();
    different_server.id = McpServerId::new();
    assert_ne!(
        compute_launch_spec_digest(&first).unwrap(),
        compute_launch_spec_digest(&different_server).unwrap()
    );

    let mut joined_argument = first.clone();
    let McpTransportConfig::Stdio(stdio) = &mut joined_argument.transport else {
        panic!("test config must use stdio");
    };
    stdio.arguments = vec!["--mode stdio".to_string(), String::new()];
    assert_ne!(
        compute_launch_spec_digest(&first).unwrap(),
        compute_launch_spec_digest(&joined_argument).unwrap()
    );

    let mut without_empty = first.clone();
    let McpTransportConfig::Stdio(stdio) = &mut without_empty.transport else {
        panic!("test config must use stdio");
    };
    stdio.arguments.pop();
    assert_ne!(
        compute_launch_spec_digest(&first).unwrap(),
        compute_launch_spec_digest(&without_empty).unwrap()
    );
}

#[test]
fn startup_reconciles_digest_corruption_to_disabled_untrusted_state() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("registry.sqlite");
    let id = McpServerId::new();
    let registry = SqliteMcpRegistry::open(&path).unwrap();
    let added = registry.add(config(id, "recover")).unwrap();
    drop(registry);

    let inspection = Connection::open(&path).unwrap();
    inspection
        .execute(
            "UPDATE mcp_registry_servers
             SET config_digest = ?2, enabled = 1, trust = 'user_approved'
             WHERE server_id = ?1",
            params![id.to_string(), "f".repeat(64)],
        )
        .unwrap();
    drop(inspection);

    let reopened = SqliteMcpRegistry::open(&path).unwrap();
    let recovered = reopened.get_persisted(id).unwrap().unwrap();
    assert!(!recovered.entry.config.enabled);
    assert_eq!(recovered.entry.config.trust, McpTrustLevel::Untrusted);
    assert!(recovered.entry.revision > added.revision);
    assert_ne!(recovered.entry.config_epoch, added.config_epoch);
    assert_eq!(
        recovered.safe_error_code.as_deref(),
        Some(SAFE_ERROR_RECONCILED)
    );
    assert!(recovered.launch_authorization.is_none());
}

#[test]
fn startup_quarantines_one_bad_storage_class_without_hiding_valid_servers() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("registry.sqlite");
    let bad_id = McpServerId::new();
    let good_id = McpServerId::new();
    let registry = SqliteMcpRegistry::open(&path).unwrap();
    registry.add(config(bad_id, "bad storage class")).unwrap();
    let expected_good = registry.add(config(good_id, "valid neighbor")).unwrap();
    drop(registry);

    let blob_canary = vec![0xff, 0x00, 0x80, 0x41];
    let corruption = Connection::open(&path).unwrap();
    corruption
        .execute(
            "UPDATE mcp_registry_servers
             SET display_name = ?2
             WHERE server_id = ?1",
            params![bad_id.to_string(), &blob_canary],
        )
        .unwrap();
    drop(corruption);

    let reopened = SqliteMcpRegistry::open(&path).unwrap();
    assert_eq!(reopened.get(good_id).unwrap(), Some(expected_good.clone()));
    assert!(reopened.get(bad_id).unwrap().is_none());
    assert_eq!(reopened.list().unwrap(), vec![expected_good.clone()]);
    assert_eq!(reopened.current_revision().unwrap(), 3);
    drop(reopened);

    let inspection = Connection::open(&path).unwrap();
    let quarantined = inspection
        .query_row(
            "SELECT display_name, record_state, enabled, trust,
                    safe_error_code, registry_revision
             FROM mcp_registry_servers
             WHERE server_id = ?1",
            [bad_id.to_string()],
            |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(
        quarantined,
        (
            blob_canary,
            RECORD_STATE_INVALID.to_string(),
            0,
            "untrusted".to_string(),
            SAFE_ERROR_INVALID.to_string(),
            3,
        )
    );
    drop(inspection);

    let reopened_again = SqliteMcpRegistry::open(&path).unwrap();
    assert_eq!(reopened_again.current_revision().unwrap(), 3);
    assert_eq!(reopened_again.get(good_id).unwrap(), Some(expected_good));
    assert!(reopened_again.get(bad_id).unwrap().is_none());
}

#[test]
fn unknown_row_schema_is_quarantined_and_never_listed() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("registry.sqlite");
    let id = McpServerId::new();
    let registry = SqliteMcpRegistry::open(&path).unwrap();
    registry.add(config(id, "quarantine")).unwrap();
    drop(registry);

    let inspection = Connection::open(&path).unwrap();
    inspection
        .execute_batch("PRAGMA ignore_check_constraints = ON;")
        .unwrap();
    inspection
        .execute(
            "UPDATE mcp_registry_servers SET schema_version = 99 WHERE server_id = ?1",
            [id.to_string()],
        )
        .unwrap();
    drop(inspection);

    let reopened = SqliteMcpRegistry::open(&path).unwrap();
    assert!(reopened.get(id).unwrap().is_none());
    let inspection = Connection::open(&path).unwrap();
    let state = inspection
        .query_row(
            "SELECT record_state, enabled, trust, safe_error_code
             FROM mcp_registry_servers WHERE server_id = ?1",
            [id.to_string()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(
        state,
        (
            RECORD_STATE_INVALID.to_string(),
            0,
            "untrusted".to_string(),
            SAFE_ERROR_INVALID.to_string()
        )
    );
}

fn tokio_test_block_on<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap()
        .block_on(future)
}
