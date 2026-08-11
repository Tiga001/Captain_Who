//! Canonical SQLite schema installation and validation for the MCP Registry.

use super::*;

/// The production database receives these objects from Core's canonical schema.
/// A completely empty database may install the same isolated sub-schema so the
/// Registry can still be exercised independently in focused tests and tools.
const CANONICAL_MCP_REGISTRY_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS mcp_registry_metadata (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    schema_version INTEGER NOT NULL CHECK (schema_version = 1),
    revision_watermark INTEGER NOT NULL CHECK (
        revision_watermark >= 0
    ),
    updated_at INTEGER NOT NULL CHECK (updated_at >= 0)
);

CREATE TABLE IF NOT EXISTS mcp_registry_servers (
    schema_version INTEGER NOT NULL CHECK (schema_version = 1),
    server_id TEXT PRIMARY KEY,
    display_name TEXT NOT NULL,
    scope_kind TEXT NOT NULL CHECK (scope_kind = 'user'),
    source_kind TEXT NOT NULL CHECK (source_kind = 'user_manual'),
    transport_kind TEXT NOT NULL CHECK (transport_kind = 'stdio'),
    executable TEXT NOT NULL,
    arguments_json TEXT NOT NULL CHECK (
        json_valid(arguments_json)
        AND json_type(arguments_json) = 'array'
    ),
    cwd TEXT NOT NULL,
    enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    trust TEXT NOT NULL CHECK (
        trust IN ('untrusted', 'user_approved')
    ),
    approval_mode TEXT NOT NULL CHECK (
        approval_mode IN ('prompt', 'auto', 'deny')
    ),
    connect_timeout_ms INTEGER NOT NULL CHECK (
        connect_timeout_ms BETWEEN 1 AND 10000
    ),
    request_timeout_ms INTEGER NOT NULL CHECK (
        request_timeout_ms BETWEEN 1 AND 300000
    ),
    shutdown_timeout_ms INTEGER NOT NULL CHECK (
        shutdown_timeout_ms BETWEEN 1 AND 2000
    ),
    config_digest TEXT NOT NULL,
    config_epoch TEXT NOT NULL,
    registry_revision INTEGER NOT NULL CHECK (
        registry_revision > 0
    ),
    launch_spec_digest TEXT NOT NULL,
    authorized_launch_spec_digest TEXT,
    authorized_config_epoch TEXT,
    authorized_config_digest TEXT,
    authorization_format_version INTEGER,
    authorization_policy_version INTEGER,
    authorized_at INTEGER,
    record_state TEXT NOT NULL DEFAULT 'active' CHECK (
        record_state IN ('active', 'invalid')
    ),
    safe_error_code TEXT,
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
    updated_at INTEGER NOT NULL CHECK (updated_at >= created_at),
    CHECK (
        (
            authorized_launch_spec_digest IS NULL
            AND authorized_config_epoch IS NULL
            AND authorized_config_digest IS NULL
            AND authorization_format_version IS NULL
            AND authorization_policy_version IS NULL
            AND authorized_at IS NULL
        )
        OR
        (
            authorized_launch_spec_digest IS NOT NULL
            AND authorized_config_epoch IS NOT NULL
            AND authorized_config_digest IS NOT NULL
            AND authorization_format_version > 0
            AND authorization_policy_version > 0
            AND authorized_at >= 0
        )
    )
);

CREATE INDEX IF NOT EXISTS mcp_registry_servers_revision
ON mcp_registry_servers(registry_revision, server_id);

CREATE TABLE IF NOT EXISTS mcp_registry_model_namespaces (
    schema_version INTEGER NOT NULL CHECK (schema_version = 1),
    server_id TEXT PRIMARY KEY,
    model_namespace TEXT NOT NULL UNIQUE,
    created_at INTEGER NOT NULL CHECK (created_at >= 0)
);
"#;

const REGISTRY_CATALOG_PREFIX: &str = "mcp_registry_";

#[derive(Debug, PartialEq, Eq)]
struct CatalogObject {
    object_type: String,
    name: String,
    table_name: String,
    sql: String,
}

pub(super) fn prepare_schema(
    connection: &mut Connection,
) -> Result<(), McpRegistryPersistenceError> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;

    if application_schema_object_count(&transaction)? == 0 {
        transaction
            .execute_batch(CANONICAL_MCP_REGISTRY_SCHEMA)
            .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
    }

    validate_registry_catalog(&transaction)?;
    seed_and_validate_metadata(&transaction)?;

    transaction
        .commit()
        .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)
}

fn application_schema_object_count(
    connection: &Connection,
) -> Result<i64, McpRegistryPersistenceError> {
    connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_schema
             WHERE sql IS NOT NULL AND name NOT LIKE 'sqlite_%'",
            [],
            |row| row.get(0),
        )
        .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)
}

fn validate_registry_catalog(connection: &Connection) -> Result<(), McpRegistryPersistenceError> {
    let expected = expected_registry_catalog()?;
    let actual = registry_catalog(connection)?;
    if actual != expected {
        return Err(McpRegistryPersistenceError::DevelopmentStorageSchemaResetRequired);
    }
    Ok(())
}

fn expected_registry_catalog() -> Result<Vec<CatalogObject>, McpRegistryPersistenceError> {
    let connection = Connection::open_in_memory()
        .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
    connection
        .execute_batch(CANONICAL_MCP_REGISTRY_SCHEMA)
        .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
    registry_catalog(&connection)
}

fn registry_catalog(
    connection: &Connection,
) -> Result<Vec<CatalogObject>, McpRegistryPersistenceError> {
    let mut statement = connection
        .prepare(
            "SELECT type, name, tbl_name, sql
             FROM sqlite_schema
             WHERE sql IS NOT NULL
               AND (name GLOB ?1 OR tbl_name GLOB ?1)
             ORDER BY type, name, tbl_name",
        )
        .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
    let objects = statement
        .query_map([format!("{REGISTRY_CATALOG_PREFIX}*")], |row| {
            Ok(CatalogObject {
                object_type: row.get(0)?,
                name: row.get(1)?,
                table_name: row.get(2)?,
                sql: normalize_schema_sql(&row.get::<_, String>(3)?),
            })
        })
        .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
    Ok(objects)
}

fn normalize_schema_sql(sql: &str) -> String {
    // The Core baseline and the isolated Registry fixture intentionally share
    // the same SQL tokens, but not their indentation. None of these four
    // objects contains a string literal with whitespace, so removing layout
    // characters produces a stable, formatting-independent catalog identity.
    sql.chars()
        .filter(|character| !character.is_whitespace())
        .collect()
}

fn seed_and_validate_metadata(
    transaction: &Transaction<'_>,
) -> Result<(), McpRegistryPersistenceError> {
    let now = now_ms()?;
    transaction
        .execute(
            "INSERT OR IGNORE INTO mcp_registry_metadata (
                 singleton, schema_version, revision_watermark, updated_at
             ) VALUES (?1, ?2, 0, ?3)",
            params![REGISTRY_METADATA_SINGLETON, REGISTRY_SCHEMA_VERSION, now],
        )
        .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;

    let metadata = transaction
        .query_row(
            "SELECT singleton, schema_version
             FROM mcp_registry_metadata",
            [],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()
        .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
    let row_count = transaction
        .query_row("SELECT COUNT(*) FROM mcp_registry_metadata", [], |row| {
            row.get::<_, i64>(0)
        })
        .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
    if row_count != 1 || metadata != Some((REGISTRY_METADATA_SINGLETON, REGISTRY_SCHEMA_VERSION)) {
        return Err(McpRegistryPersistenceError::DevelopmentStorageSchemaResetRequired);
    }
    Ok(())
}
