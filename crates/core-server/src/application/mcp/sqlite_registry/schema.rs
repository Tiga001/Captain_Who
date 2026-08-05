//! SQLite schema installation, compatibility checks, and namespace repair.

use super::*;

pub(super) fn run_migrations(
    connection: &mut Connection,
) -> Result<(), McpRegistryPersistenceError> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
    quarantine_incompatible_registry_schema(&transaction)?;
    let migrate_approval_mode_constraint =
        prepare_approval_mode_constraint_migration(&transaction)?;
    transaction
        .execute_batch(
            "
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
            ",
        )
        .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
    if migrate_approval_mode_constraint {
        finish_approval_mode_constraint_migration(&transaction)?;
    }

    let now = now_ms()?;
    transaction
        .execute(
            "INSERT OR IGNORE INTO mcp_registry_metadata (
                 singleton, schema_version, revision_watermark, updated_at
             ) VALUES (?1, ?2, 0, ?3)",
            params![REGISTRY_METADATA_SINGLETON, REGISTRY_SCHEMA_VERSION, now],
        )
        .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
    let schema_version = transaction
        .query_row(
            "SELECT schema_version FROM mcp_registry_metadata WHERE singleton = ?1",
            [REGISTRY_METADATA_SINGLETON],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
    if schema_version != REGISTRY_SCHEMA_VERSION {
        return Err(McpRegistryPersistenceError::CorruptRecord);
    }
    transaction
        .commit()
        .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)
}

const APPROVAL_MODE_MIGRATION_BACKUP: &str = "mcp_registry_servers_approval_mode_prompt_deny_v1";

/// SQLite cannot alter a CHECK constraint in place. Preserve every v1 row and
/// identity field in a transaction, recreate the table with the additive
/// `auto` value, then copy the rows back verbatim.
fn prepare_approval_mode_constraint_migration(
    transaction: &Transaction<'_>,
) -> Result<bool, McpRegistryPersistenceError> {
    let Some(columns) = registry_table_columns(transaction, "mcp_registry_servers")? else {
        return Ok(false);
    };
    if !column_names_match(&columns, REGISTRY_SERVER_COLUMNS) {
        return Ok(false);
    }
    let create_sql = transaction
        .query_row(
            "SELECT sql FROM sqlite_schema
             WHERE type = 'table' AND name = 'mcp_registry_servers'",
            [],
            |row| row.get::<_, String>(0),
        )
        .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
    if create_sql.contains("'auto'") {
        return Ok(false);
    }
    let invalid_mode_count = transaction
        .query_row(
            "SELECT COUNT(*) FROM mcp_registry_servers
             WHERE approval_mode NOT IN ('prompt', 'deny')",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
    if invalid_mode_count != 0
        || registry_table_columns(transaction, APPROVAL_MODE_MIGRATION_BACKUP)?.is_some()
    {
        return Err(McpRegistryPersistenceError::CorruptRecord);
    }
    transaction
        .execute_batch(
            "ALTER TABLE mcp_registry_servers
                 RENAME TO mcp_registry_servers_approval_mode_prompt_deny_v1;
             DROP INDEX IF EXISTS mcp_registry_servers_revision;",
        )
        .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
    Ok(true)
}

fn finish_approval_mode_constraint_migration(
    transaction: &Transaction<'_>,
) -> Result<(), McpRegistryPersistenceError> {
    let columns = REGISTRY_SERVER_COLUMNS.join(", ");
    let copy = format!(
        "INSERT INTO mcp_registry_servers ({columns})
         SELECT {columns}
         FROM {APPROVAL_MODE_MIGRATION_BACKUP};
         DROP TABLE {APPROVAL_MODE_MIGRATION_BACKUP};"
    );
    transaction
        .execute_batch(&copy)
        .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)
}

/// Repairs the additive Host-owned model namespace index without changing any
/// Registry configuration identity, revision, or launch authorization.
///
/// Existing valid mappings are immutable. Missing or malformed side-table
/// rows are rebuilt deterministically from the current persisted display name;
/// stale rows for removed/quarantined servers are deleted. The caller runs
/// this once before startup record decoding and once after reconciliation.
pub(super) fn reconcile_model_namespaces(
    connection: &mut Connection,
) -> Result<(), McpRegistryPersistenceError> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;

    let candidates = {
        let mut statement = transaction
            .prepare(
                "SELECT server_id, display_name, created_at
                 FROM mcp_registry_servers
                 WHERE record_state = 'active'
                 ORDER BY created_at, server_id",
            )
            .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
        let rows = statement
            .query_map([], |row| {
                let server_id = match row.get_ref(0)? {
                    ValueRef::Text(value) => std::str::from_utf8(value).ok().map(str::to_string),
                    _ => None,
                };
                let display_name = match row.get_ref(1)? {
                    ValueRef::Text(value) => std::str::from_utf8(value).ok().map(str::to_string),
                    _ => None,
                };
                let created_at = match row.get_ref(2)? {
                    ValueRef::Integer(value) => Some(value),
                    _ => None,
                };
                Ok(server_id.zip(display_name).zip(created_at).map(
                    |((server_id, display_name), created_at)| (server_id, display_name, created_at),
                ))
            })
            .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
        rows.into_iter().flatten().collect::<Vec<_>>()
    };
    let candidates = candidates
        .into_iter()
        .filter_map(|(stored_id, display_name, created_at)| {
            McpServerId::from_str(&stored_id)
                .ok()
                .map(|server_id| (stored_id, server_id, display_name, created_at))
        })
        .collect::<Vec<_>>();
    let active_ids = candidates
        .iter()
        .map(|(stored_id, _, _, _)| stored_id.clone())
        .collect::<std::collections::BTreeSet<_>>();

    let persisted = {
        let mut statement = transaction
            .prepare(
                "SELECT rowid, schema_version, server_id, model_namespace
                 FROM mcp_registry_model_namespaces
                 ORDER BY rowid",
            )
            .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    match row.get_ref(1)? {
                        ValueRef::Integer(value) => Some(value),
                        _ => None,
                    },
                    match row.get_ref(2)? {
                        ValueRef::Text(value) => {
                            std::str::from_utf8(value).ok().map(str::to_string)
                        }
                        _ => None,
                    },
                    match row.get_ref(3)? {
                        ValueRef::Text(value) => {
                            std::str::from_utf8(value).ok().map(str::to_string)
                        }
                        _ => None,
                    },
                ))
            })
            .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
        rows
    };

    let mut mapped_ids = std::collections::BTreeSet::new();
    let mut occupied = std::collections::BTreeSet::<McpModelNamespace>::new();
    for (rowid, schema_version, server_id, raw_namespace) in persisted {
        let valid_mapping = schema_version
            .filter(|version| *version == MODEL_NAMESPACE_SCHEMA_VERSION)
            .zip(server_id)
            .zip(raw_namespace)
            .and_then(|((_, server_id), raw_namespace)| {
                McpModelNamespace::from_str(&raw_namespace)
                    .ok()
                    .map(|namespace| (server_id, namespace))
            })
            .filter(|(server_id, _)| active_ids.contains(server_id))
            .filter(|(server_id, namespace)| {
                !mapped_ids.contains(server_id) && !occupied.contains(namespace)
            });
        if let Some((server_id, namespace)) = valid_mapping {
            mapped_ids.insert(server_id);
            occupied.insert(namespace);
        } else {
            transaction
                .execute(
                    "DELETE FROM mcp_registry_model_namespaces WHERE rowid = ?1",
                    [rowid],
                )
                .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
        }
    }

    for (stored_id, server_id, display_name, created_at) in candidates {
        if mapped_ids.contains(&stored_id) {
            continue;
        }
        let namespace = allocate_model_namespace(&display_name, server_id, occupied.iter())
            .map_err(|_| McpRegistryPersistenceError::CorruptRecord)?;
        transaction
            .execute(
                "INSERT INTO mcp_registry_model_namespaces (
                     schema_version, server_id, model_namespace, created_at
                 ) VALUES (?1, ?2, ?3, ?4)",
                params![
                    MODEL_NAMESPACE_SCHEMA_VERSION,
                    stored_id,
                    namespace.as_str(),
                    created_at.max(0),
                ],
            )
            .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
        occupied.insert(namespace);
    }

    transaction
        .commit()
        .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)
}

fn quarantine_incompatible_registry_schema(
    transaction: &Transaction<'_>,
) -> Result<(), McpRegistryPersistenceError> {
    let namespace_columns = registry_table_columns(transaction, "mcp_registry_model_namespaces")?;
    let namespace_shape_is_incompatible = match namespace_columns.as_deref() {
        Some(columns) => {
            !column_names_match(columns, MODEL_NAMESPACE_COLUMNS)
                || !model_namespace_table_constraints_are_compatible(transaction)?
        }
        None => false,
    };
    if namespace_shape_is_incompatible {
        quarantine_registry_table(transaction, "mcp_registry_model_namespaces")?;
    }

    let metadata_columns = registry_table_columns(transaction, "mcp_registry_metadata")?;
    let metadata_shape_is_incompatible = metadata_columns
        .as_deref()
        .is_some_and(|columns| !column_names_match(columns, REGISTRY_METADATA_COLUMNS));

    if metadata_shape_is_incompatible {
        quarantine_registry_table(transaction, "mcp_registry_metadata")?;
        quarantine_registry_table(transaction, "mcp_registry_servers")?;
        return Ok(());
    }

    if metadata_columns.is_some() && !registry_metadata_version_is_compatible(transaction)? {
        quarantine_registry_table(transaction, "mcp_registry_metadata")?;
        quarantine_registry_table(transaction, "mcp_registry_servers")?;
        return Ok(());
    }

    let server_columns = registry_table_columns(transaction, "mcp_registry_servers")?;
    if server_columns
        .as_deref()
        .is_some_and(|columns| !column_names_match(columns, REGISTRY_SERVER_COLUMNS))
    {
        quarantine_registry_table(transaction, "mcp_registry_servers")?;
    }
    Ok(())
}

fn model_namespace_table_constraints_are_compatible(
    transaction: &Transaction<'_>,
) -> Result<bool, McpRegistryPersistenceError> {
    if transaction
        .prepare("SELECT rowid FROM mcp_registry_model_namespaces LIMIT 0")
        .is_err()
    {
        return Ok(false);
    }

    let table_info_sql = format!(
        "PRAGMA table_info({})",
        quote_identifier("mcp_registry_model_namespaces")
    );
    let primary_key_columns = {
        let mut statement = transaction
            .prepare(&table_info_sql)
            .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
        let mut rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(1)?, row.get::<_, i64>(5)?))
            })
            .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
        rows.retain(|(_, primary_key_position)| *primary_key_position > 0);
        rows.sort_by_key(|(_, primary_key_position)| *primary_key_position);
        rows.into_iter().map(|(name, _)| name).collect::<Vec<_>>()
    };
    if primary_key_columns != ["server_id"] {
        return Ok(false);
    }

    let index_list_sql = format!(
        "PRAGMA index_list({})",
        quote_identifier("mcp_registry_model_namespaces")
    );
    let unique_indexes = {
        let mut statement = transaction
            .prepare(&index_list_sql)
            .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)? != 0,
                    row.get::<_, i64>(4)? != 0,
                ))
            })
            .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
        rows
    };
    for (index_name, unique, partial) in unique_indexes {
        if !unique || partial {
            continue;
        }
        let index_info_sql = format!("PRAGMA index_info({})", quote_identifier(&index_name));
        let columns = {
            let mut statement = transaction
                .prepare(&index_info_sql)
                .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
            let rows = statement
                .query_map([], |row| row.get::<_, Option<String>>(2))
                .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
            rows
        };
        if columns == [Some("model_namespace".to_string())] {
            return Ok(true);
        }
    }
    Ok(false)
}

fn registry_metadata_version_is_compatible(
    transaction: &Transaction<'_>,
) -> Result<bool, McpRegistryPersistenceError> {
    let row_count = transaction
        .query_row("SELECT COUNT(*) FROM mcp_registry_metadata", [], |row| {
            row.get::<_, i64>(0)
        })
        .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
    if row_count == 0 {
        return Ok(true);
    }
    if row_count != 1 {
        return Ok(false);
    }
    let identity = transaction.query_row(
        "SELECT singleton, schema_version FROM mcp_registry_metadata LIMIT 1",
        [],
        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
    );
    Ok(matches!(
        identity,
        Ok((REGISTRY_METADATA_SINGLETON, REGISTRY_SCHEMA_VERSION))
    ))
}

fn registry_table_columns(
    transaction: &Transaction<'_>,
    table_name: &str,
) -> Result<Option<Vec<String>>, McpRegistryPersistenceError> {
    let object_type = transaction
        .query_row(
            "SELECT type FROM sqlite_schema WHERE name = ?1",
            [table_name],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
    let Some(object_type) = object_type else {
        return Ok(None);
    };
    if object_type != "table" {
        return Err(McpRegistryPersistenceError::CorruptRecord);
    }
    let sql = format!("PRAGMA table_info({})", quote_identifier(table_name));
    let mut statement = transaction
        .prepare(&sql)
        .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
    Ok(Some(columns))
}

pub(super) fn column_names_match(actual: &[String], expected: &[&str]) -> bool {
    actual.len() == expected.len()
        && actual
            .iter()
            .map(String::as_str)
            .eq(expected.iter().copied())
}

fn quarantine_registry_table(
    transaction: &Transaction<'_>,
    table_name: &str,
) -> Result<(), McpRegistryPersistenceError> {
    if registry_table_columns(transaction, table_name)?.is_none() {
        return Ok(());
    }
    let quarantine_name = next_quarantine_table_name(transaction, table_name)?;
    let rename = format!(
        "ALTER TABLE {} RENAME TO {}",
        quote_identifier(table_name),
        quote_identifier(&quarantine_name)
    );
    transaction
        .execute_batch(&rename)
        .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;

    if table_name == "mcp_registry_servers" {
        let revision_index_owner = transaction
            .query_row(
                "SELECT tbl_name FROM sqlite_schema
                 WHERE type = 'index' AND name = 'mcp_registry_servers_revision'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
        match revision_index_owner.as_deref() {
            None => {}
            Some(owner) if owner == quarantine_name => transaction
                .execute_batch("DROP INDEX mcp_registry_servers_revision")
                .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?,
            Some(_) => return Err(McpRegistryPersistenceError::CorruptRecord),
        }
    }
    Ok(())
}

fn next_quarantine_table_name(
    transaction: &Transaction<'_>,
    table_name: &str,
) -> Result<String, McpRegistryPersistenceError> {
    for sequence in 1_u32..=10_000 {
        let candidate = format!("{table_name}_incompatible_v{REGISTRY_SCHEMA_VERSION}_{sequence}");
        let exists = transaction
            .query_row(
                "SELECT 1 FROM sqlite_schema WHERE name = ?1",
                [&candidate],
                |_| Ok(()),
            )
            .optional()
            .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?
            .is_some();
        if !exists {
            return Ok(candidate);
        }
    }
    Err(McpRegistryPersistenceError::StorageUnavailable)
}

fn quote_identifier(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}
