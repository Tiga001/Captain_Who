use rusqlite::{ffi, Connection, OptionalExtension};
use sha2::{Digest, Sha256};

pub const STORAGE_SCHEMA_VERSION: i32 = 3;
pub const DEVELOPMENT_STORAGE_SCHEMA_RESET_REQUIRED: &str =
    "development_storage_schema_reset_required";

const CANONICAL_SCHEMA: &str = include_str!("canonical_schema.sql");
const CANONICAL_SCHEMA_FINGERPRINT: &str =
    "sha256:a757cbd73bfa720a40d36d6ec2c4827bc71255496b4da90f6868c731fc1ac46e";

/// Opens the single supported development schema.
///
/// A brand-new database is initialized atomically. Once any application-owned schema object
/// exists, its explicit schema version and catalog fingerprint must match the current baseline.
/// Development databases from earlier baselines are intentionally not upgraded in place.
pub fn run_migrations(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute_batch("PRAGMA foreign_keys = ON;")?;

    let schema_version = read_schema_version(connection)?;
    let object_count = application_schema_object_count(connection)?;

    if schema_version == 0 && object_count == 0 {
        return create_canonical_schema(connection);
    }

    if schema_version != STORAGE_SCHEMA_VERSION {
        return Err(reset_required_error(format!(
            "expected schema version {STORAGE_SCHEMA_VERSION}, found {schema_version}"
        )));
    }

    validate_canonical_schema(connection)
}

fn create_canonical_schema(connection: &Connection) -> rusqlite::Result<()> {
    let transaction = connection.unchecked_transaction()?;
    transaction.execute_batch(CANONICAL_SCHEMA)?;
    transaction.pragma_update(None, "user_version", STORAGE_SCHEMA_VERSION)?;

    validate_canonical_schema(&transaction)?;
    ensure_foreign_keys_are_valid(&transaction)?;

    transaction.commit()
}

fn validate_canonical_schema(connection: &Connection) -> rusqlite::Result<()> {
    let actual_fingerprint = schema_fingerprint(connection)?;
    if actual_fingerprint != CANONICAL_SCHEMA_FINGERPRINT {
        return Err(reset_required_error(format!(
            "schema catalog fingerprint mismatch (expected {CANONICAL_SCHEMA_FINGERPRINT}, found {actual_fingerprint})"
        )));
    }
    ensure_foreign_keys_are_valid(connection)
}

fn read_schema_version(connection: &Connection) -> rusqlite::Result<i32> {
    connection.query_row("PRAGMA user_version", [], |row| row.get(0))
}

fn application_schema_object_count(connection: &Connection) -> rusqlite::Result<i64> {
    connection.query_row(
        "SELECT COUNT(*) FROM sqlite_schema
         WHERE sql IS NOT NULL AND name NOT LIKE 'sqlite_%'",
        [],
        |row| row.get(0),
    )
}

fn schema_fingerprint(connection: &Connection) -> rusqlite::Result<String> {
    let mut statement = connection.prepare(
        "SELECT type, name, tbl_name, sql
         FROM sqlite_schema
         WHERE sql IS NOT NULL
           AND name NOT LIKE 'sqlite_%'
         ORDER BY type, name, tbl_name",
    )?;
    let mut rows = statement.query([])?;
    let mut digest = Sha256::new();

    while let Some(row) = rows.next()? {
        for index in 0..4 {
            let value: String = row.get(index)?;
            digest.update((value.len() as u64).to_be_bytes());
            digest.update(value.as_bytes());
        }
    }

    Ok(format!("sha256:{:x}", digest.finalize()))
}

fn ensure_foreign_keys_are_valid(connection: &Connection) -> rusqlite::Result<()> {
    let violation = connection
        .query_row("PRAGMA foreign_key_check", [], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })
        .optional()?;

    if let Some((table, row_id, parent)) = violation {
        return Err(reset_required_error(format!(
            "foreign key violation in table {table}, row {row_id}, parent {}",
            parent.unwrap_or_else(|| "<unknown>".to_string())
        )));
    }

    Ok(())
}

fn reset_required_error(detail: impl std::fmt::Display) -> rusqlite::Error {
    rusqlite::Error::SqliteFailure(
        ffi::Error::new(ffi::SQLITE_SCHEMA),
        Some(format!(
            "{DEVELOPMENT_STORAGE_SCHEMA_RESET_REQUIRED}: {detail}"
        )),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_database_creates_the_canonical_schema() {
        let connection = Connection::open_in_memory().unwrap();

        run_migrations(&connection).unwrap();

        assert_eq!(
            read_schema_version(&connection).unwrap(),
            STORAGE_SCHEMA_VERSION
        );
        assert_eq!(
            schema_fingerprint(&connection).unwrap(),
            CANONICAL_SCHEMA_FINGERPRINT
        );
        ensure_foreign_keys_are_valid(&connection).unwrap();

        for required_object in [
            "models",
            "conversation_turn_traces",
            "provider_continuations",
            "context_compaction_summaries",
            "conversation_forks",
            "agent_command_sessions",
            "mcp_registry_metadata",
            "mcp_registry_servers",
            "mcp_registry_model_namespaces",
            "mcp_registry_servers_revision",
            "conversation_history_fts",
            "conversation_history_fts_message_insert",
            "validate_agent_command_session_model_receipt_payload_insert",
        ] {
            let exists = connection
                .query_row(
                    "SELECT 1 FROM sqlite_schema
                     WHERE name = ?1 AND sql IS NOT NULL",
                    [required_object],
                    |_| Ok(()),
                )
                .optional()
                .unwrap()
                .is_some();
            assert!(exists, "missing canonical schema object {required_object}");
        }

        let maintenance_table_exists = connection
            .query_row(
                "SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = 'maintenance_tasks'",
                [],
                |_| Ok(()),
            )
            .optional()
            .unwrap()
            .is_some();
        assert!(!maintenance_table_exists);
    }

    #[test]
    fn canonical_composer_schema_requires_complete_array_payloads() {
        let connection = Connection::open_in_memory().unwrap();
        run_migrations(&connection).unwrap();

        let columns = [
            "permission_mode_version",
            "attachments_json",
            "skills_json",
            "queued_messages_json",
        ];
        for column in columns {
            let (not_null, default_value): (i64, Option<String>) = connection
                .query_row(
                    "SELECT [notnull], dflt_value FROM pragma_table_info('composer_drafts')
                     WHERE name = ?1",
                    [column],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap();
            assert_eq!(not_null, 1, "{column} must be required");
            assert_eq!(default_value, None, "{column} must not have a default");
        }

        let non_array = connection.execute(
            "INSERT INTO composer_drafts (
                scope_id, message, permission_mode, permission_mode_version, model_id, project_id,
                attachments_json, skills_json, queued_messages_json, updated_at
             ) VALUES ('draft-invalid', '', 'default', 1, NULL, NULL, '{}', '[]', '[]', 1)",
            [],
        );
        assert!(non_array.is_err());

        for statement in [
            "INSERT INTO composer_drafts (
                scope_id, message, permission_mode, model_id, project_id,
                attachments_json, skills_json, queued_messages_json, updated_at
             ) VALUES ('draft-missing-version', '', 'default', NULL, NULL, '[]', '[]', '[]', 1)",
            "INSERT INTO composer_drafts (
                scope_id, message, permission_mode, permission_mode_version, model_id, project_id,
                attachments_json, queued_messages_json, updated_at
             ) VALUES ('draft-missing-skills', '', 'default', 1, NULL, NULL, '[]', '[]', 1)",
            "INSERT INTO composer_drafts (
                scope_id, message, permission_mode, permission_mode_version, model_id, project_id,
                attachments_json, skills_json, updated_at
             ) VALUES ('draft-missing-queue', '', 'default', 1, NULL, NULL, '[]', '[]', 1)",
        ] {
            assert!(connection.execute(statement, []).is_err());
        }
    }

    #[test]
    fn canonical_goal_revision_actor_is_required() {
        let connection = Connection::open_in_memory().unwrap();
        run_migrations(&connection).unwrap();

        let (not_null, default_value): (i64, Option<String>) = connection
            .query_row(
                "SELECT [notnull], dflt_value
                 FROM pragma_table_info('conversation_goal_revisions')
                 WHERE name = 'actor'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(not_null, 1);
        assert_eq!(default_value, None);
    }

    #[test]
    fn reopening_the_current_schema_does_not_mutate_the_catalog() {
        let connection = Connection::open_in_memory().unwrap();
        run_migrations(&connection).unwrap();
        let before = schema_fingerprint(&connection).unwrap();
        let before_changes = connection.total_changes();

        run_migrations(&connection).unwrap();

        assert_eq!(schema_fingerprint(&connection).unwrap(), before);
        assert_eq!(connection.total_changes(), before_changes);
    }

    #[test]
    fn an_unversioned_non_empty_database_requires_a_development_reset() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch("CREATE TABLE historical_development_table (id TEXT PRIMARY KEY);")
            .unwrap();

        let error = run_migrations(&connection).unwrap_err();

        assert!(error
            .to_string()
            .contains(DEVELOPMENT_STORAGE_SCHEMA_RESET_REQUIRED));
        assert!(connection
            .query_row(
                "SELECT 1 FROM sqlite_schema WHERE name = 'historical_development_table'",
                [],
                |_| Ok(()),
            )
            .optional()
            .unwrap()
            .is_some());
    }

    #[test]
    fn an_unknown_schema_version_requires_a_development_reset() {
        let connection = Connection::open_in_memory().unwrap();
        connection.pragma_update(None, "user_version", 999).unwrap();

        let error = run_migrations(&connection).unwrap_err();

        assert!(error
            .to_string()
            .contains(DEVELOPMENT_STORAGE_SCHEMA_RESET_REQUIRED));
        assert_eq!(read_schema_version(&connection).unwrap(), 999);
    }

    #[test]
    fn a_tampered_current_schema_requires_a_development_reset() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch("CREATE TABLE incomplete (id TEXT PRIMARY KEY);")
            .unwrap();
        connection
            .pragma_update(None, "user_version", STORAGE_SCHEMA_VERSION)
            .unwrap();

        let error = run_migrations(&connection).unwrap_err();

        assert!(error
            .to_string()
            .contains(DEVELOPMENT_STORAGE_SCHEMA_RESET_REQUIRED));
    }

    #[test]
    fn a_current_schema_missing_a_trigger_requires_a_development_reset() {
        let connection = Connection::open_in_memory().unwrap();
        run_migrations(&connection).unwrap();
        connection
            .execute_batch("DROP TRIGGER conversation_history_fts_message_insert;")
            .unwrap();

        let error = run_migrations(&connection).unwrap_err();

        assert!(error
            .to_string()
            .contains(DEVELOPMENT_STORAGE_SCHEMA_RESET_REQUIRED));
    }

    #[test]
    fn a_current_schema_missing_an_fts_shadow_table_requires_a_development_reset() {
        let connection = Connection::open_in_memory().unwrap();
        run_migrations(&connection).unwrap();
        connection
            .execute_batch("DROP TABLE conversation_history_fts_data;")
            .unwrap();

        let error = run_migrations(&connection).unwrap_err();

        assert!(error
            .to_string()
            .contains(DEVELOPMENT_STORAGE_SCHEMA_RESET_REQUIRED));
    }
}
