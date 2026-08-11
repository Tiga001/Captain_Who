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
fn standalone_empty_database_installs_the_current_registry_schema_once() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("registry.sqlite");
    let registry = SqliteMcpRegistry::open(&path).unwrap();
    assert_eq!(registry.current_revision().unwrap(), 0);
    drop(registry);

    let inspection = Connection::open(&path).unwrap();
    let objects = inspection
        .prepare(
            "SELECT name FROM sqlite_schema
             WHERE sql IS NOT NULL AND name GLOB 'mcp_registry_*'
             ORDER BY name",
        )
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(
        objects,
        [
            "mcp_registry_metadata",
            "mcp_registry_model_namespaces",
            "mcp_registry_servers",
            "mcp_registry_servers_revision",
        ]
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

    let before = inspection
        .prepare(
            "SELECT type, name, tbl_name, sql FROM sqlite_schema
             WHERE sql IS NOT NULL AND name GLOB 'mcp_registry_*'
             ORDER BY type, name, tbl_name",
        )
        .unwrap()
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    drop(inspection);

    drop(SqliteMcpRegistry::open(&path).unwrap());
    let reopened = Connection::open(&path).unwrap();
    let after = reopened
        .prepare(
            "SELECT type, name, tbl_name, sql FROM sqlite_schema
             WHERE sql IS NOT NULL AND name GLOB 'mcp_registry_*'
             ORDER BY type, name, tbl_name",
        )
        .unwrap()
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(after, before);
}

#[test]
fn core_canonical_schema_matches_registry_catalog_without_catalog_mutation() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("storage.sqlite");
    let _storage = mycopilot_core::storage::service::StorageService::open(&path).unwrap();
    let before = {
        let connection = Connection::open(&path).unwrap();
        let catalog = connection
            .prepare(
                "SELECT type, name, tbl_name, sql FROM sqlite_schema
                 WHERE sql IS NOT NULL AND name GLOB 'mcp_registry_*'
                 ORDER BY type, name, tbl_name",
            )
            .unwrap()
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        catalog
    };

    let registry = SqliteMcpRegistry::open(&path).unwrap();
    assert_eq!(registry.current_revision().unwrap(), 0);
    drop(registry);

    let after = {
        let connection = Connection::open(&path).unwrap();
        let catalog = connection
            .prepare(
                "SELECT type, name, tbl_name, sql FROM sqlite_schema
                 WHERE sql IS NOT NULL AND name GLOB 'mcp_registry_*'
                 ORDER BY type, name, tbl_name",
            )
            .unwrap()
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        catalog
    };
    assert_eq!(after, before);
}

#[test]
fn nonempty_database_without_the_registry_schema_requires_a_development_reset() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("registry.sqlite");
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE existing_application_data (
                 id INTEGER PRIMARY KEY,
                 value TEXT NOT NULL
             );
             INSERT INTO existing_application_data(value) VALUES ('preserve-me');",
        )
        .unwrap();
    drop(connection);

    assert!(matches!(
        SqliteMcpRegistry::open(&path),
        Err(McpRegistryPersistenceError::DevelopmentStorageSchemaResetRequired)
    ));
    let inspection = Connection::open(&path).unwrap();
    assert_eq!(
        inspection
            .query_row("SELECT value FROM existing_application_data", [], |row| {
                row.get::<_, String>(0)
            })
            .unwrap(),
        "preserve-me"
    );
    assert_eq!(
        inspection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_schema
                 WHERE name GLOB 'mcp_registry_*'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        0
    );
}

#[test]
fn incompatible_registry_schema_requires_reset_without_quarantine_or_repair() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("registry.sqlite");
    let registry = SqliteMcpRegistry::open(&path).unwrap();
    drop(registry);

    let incompatible = Connection::open(&path).unwrap();
    incompatible
        .execute_batch(
            "DROP TABLE mcp_registry_servers;
             CREATE TABLE mcp_registry_servers (
                 server_id TEXT PRIMARY KEY,
                 legacy_value TEXT NOT NULL
             );
             INSERT INTO mcp_registry_servers(server_id, legacy_value)
             VALUES ('old-server', 'OLD_MCP_ROW_CANARY');",
        )
        .unwrap();
    drop(incompatible);

    assert!(matches!(
        SqliteMcpRegistry::open(&path),
        Err(McpRegistryPersistenceError::DevelopmentStorageSchemaResetRequired)
    ));

    let inspection = Connection::open(&path).unwrap();
    assert_eq!(
        inspection
            .query_row(
                "SELECT legacy_value FROM mcp_registry_servers
                 WHERE server_id = 'old-server'",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
        "OLD_MCP_ROW_CANARY"
    );
    assert_eq!(
        inspection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_schema
                 WHERE name GLOB 'mcp_registry_servers_incompatible_*'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        0
    );
}

#[test]
fn previous_approval_constraint_requires_reset_without_table_rewrite() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("registry.sqlite");
    drop(SqliteMcpRegistry::open(&path).unwrap());

    let connection = Connection::open(&path).unwrap();
    let current_sql = connection
        .query_row(
            "SELECT sql FROM sqlite_schema
             WHERE type = 'table' AND name = 'mcp_registry_servers'",
            [],
            |row| row.get::<_, String>(0),
        )
        .unwrap();
    let previous_sql = current_sql.replace(
        "approval_mode IN ('prompt', 'auto', 'deny')",
        "approval_mode IN ('prompt', 'deny')",
    );
    assert_ne!(previous_sql, current_sql);
    connection
        .execute_batch(
            "DROP INDEX mcp_registry_servers_revision;
             ALTER TABLE mcp_registry_servers RENAME TO previous_mcp_registry_servers;",
        )
        .unwrap();
    connection.execute_batch(&previous_sql).unwrap();
    connection
        .execute_batch(
            "DROP TABLE previous_mcp_registry_servers;
             CREATE INDEX mcp_registry_servers_revision
             ON mcp_registry_servers(registry_revision, server_id);",
        )
        .unwrap();
    drop(connection);

    assert!(matches!(
        SqliteMcpRegistry::open(&path),
        Err(McpRegistryPersistenceError::DevelopmentStorageSchemaResetRequired)
    ));
    let inspection = Connection::open(&path).unwrap();
    let unchanged_sql = inspection
        .query_row(
            "SELECT sql FROM sqlite_schema
             WHERE type = 'table' AND name = 'mcp_registry_servers'",
            [],
            |row| row.get::<_, String>(0),
        )
        .unwrap();
    assert!(unchanged_sql.contains("approval_mode IN ('prompt', 'deny')"));
    assert!(!unchanged_sql.contains("'auto'"));
}

#[test]
fn unknown_registry_metadata_version_requires_reset_without_rewrite() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("registry.sqlite");
    drop(SqliteMcpRegistry::open(&path).unwrap());

    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch("PRAGMA ignore_check_constraints = ON;")
        .unwrap();
    connection
        .execute(
            "UPDATE mcp_registry_metadata SET schema_version = 99
             WHERE singleton = 1",
            [],
        )
        .unwrap();
    drop(connection);

    assert!(matches!(
        SqliteMcpRegistry::open(&path),
        Err(McpRegistryPersistenceError::DevelopmentStorageSchemaResetRequired)
    ));
    let inspection = Connection::open(&path).unwrap();
    assert_eq!(
        inspection
            .query_row(
                "SELECT schema_version FROM mcp_registry_metadata WHERE singleton = 1",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        99
    );
}

#[test]
fn extra_registry_catalog_object_requires_reset_without_removal() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("registry.sqlite");
    drop(SqliteMcpRegistry::open(&path).unwrap());

    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch(
            "CREATE INDEX unexpected_registry_display_name
             ON mcp_registry_servers(display_name);",
        )
        .unwrap();
    drop(connection);

    assert!(matches!(
        SqliteMcpRegistry::open(&path),
        Err(McpRegistryPersistenceError::DevelopmentStorageSchemaResetRequired)
    ));
    let inspection = Connection::open(&path).unwrap();
    assert!(inspection
        .query_row(
            "SELECT 1 FROM sqlite_schema
             WHERE type = 'index' AND name = 'unexpected_registry_display_name'",
            [],
            |_| Ok(()),
        )
        .optional()
        .unwrap()
        .is_some());
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
fn missing_namespace_table_requires_reset_without_backfill() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("registry.sqlite");
    let id = McpServerId::new();
    let registry = SqliteMcpRegistry::open(&path).unwrap();
    registry.add(config(id, "Filesystem Test")).unwrap();
    drop(registry);

    let damaged = Connection::open(&path).unwrap();
    damaged
        .execute_batch("DROP TABLE mcp_registry_model_namespaces;")
        .unwrap();
    drop(damaged);

    assert!(matches!(
        SqliteMcpRegistry::open(&path),
        Err(McpRegistryPersistenceError::DevelopmentStorageSchemaResetRequired)
    ));
    let inspection = Connection::open(&path).unwrap();
    assert_eq!(
        inspection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_schema
                 WHERE name = 'mcp_registry_model_namespaces'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        0
    );
}

#[test]
fn malformed_namespace_record_is_quarantined_without_reconstructing_history() {
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
    assert_eq!(reopened.list().unwrap(), vec![healthy]);
    assert!(reopened.get(damaged_id).unwrap().is_none());
}

#[test]
fn noncanonical_namespace_table_requires_reset_without_rebuild() {
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
    drop(connection);

    assert!(matches!(
        SqliteMcpRegistry::open(&path),
        Err(McpRegistryPersistenceError::DevelopmentStorageSchemaResetRequired)
    ));
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
                .add_persisted(
                    config(McpServerId::new(), &format!("snapshot-{index}")),
                    None,
                )
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
            .add_persisted(config(McpServerId::new(), "revision-overflow"), None)
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
