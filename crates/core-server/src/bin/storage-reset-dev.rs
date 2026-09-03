//! Explicit development-only reset for the Electron-owned SQLite database.
//!
//! This binary is not part of the packaged application. It defaults to a non-destructive preflight
//! and requires `--confirm-reset` before it snapshots and atomically replaces the exact database.

#[allow(dead_code, unused_imports)] // The shared MCP Registry module exposes more than reset uses.
#[path = "../application/mcp/sqlite_registry.rs"]
mod sqlite_registry;

use mycopilot_core::durable_fs::{atomic_replace, sync_directory};
use mycopilot_core::image_generation::{CredentialStore, DevelopmentFileCredentialStore};
use mycopilot_core::storage::agent_prompt_preferences_repository;
use mycopilot_core::storage::browser_data_repository;
use mycopilot_core::storage::browser_download_repository;
use mycopilot_core::storage::config_repository::is_provider_protocol_revision;
use mycopilot_core::storage::image_generation_repository::{
    self, DEFAULT_IMAGE_GENERATION_PROFILE_ID,
};
use mycopilot_core::storage::models::{
    AgentPromptPreferencesRecord, BrowserDownloadSettingsRecord, BrowserPreferencesRecord,
    BrowserPreferencesUpdate, ImageGenerationProfileRecord, UiPreferencesRecord,
    BROWSER_DATA_SCHEMA_VERSION,
};
use mycopilot_core::storage::notification_repository::{self, NotificationSettingsRecord};
use mycopilot_core::storage::preferences_repository;
use mycopilot_core::storage::service::StorageService;
use mycopilot_core::storage::{acquire_database_instance_lock, create_verified_sqlite_snapshot};
use mycopilot_core::ProviderProfileConfig;
use mycopilot_mcp_client::{McpRegistry, McpTrustLevel};
use rusqlite::types::Value as SqliteValue;
use rusqlite::{params, params_from_iter, Connection, OpenFlags, OptionalExtension};
use sha2::{Digest, Sha256};
use sqlite_registry::{
    launch_authorization_is_valid, McpPersistedRegistryRecord, McpRegistryMutationPrecondition,
    SqliteMcpRegistry,
};
use std::ffi::{OsStr, OsString};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

const DATABASE_FILE_NAME: &str = "storage.sqlite";
const BACKUP_DIRECTORY_NAME: &str = "storage-backups";
const CONFIRM_RESET_FLAG: &str = "--confirm-reset";
const APP_DATA_ROOT_FLAG: &str = "--app-data-root";
const SQLITE_TRANSIENT_SUFFIXES: [&str; 3] = ["-journal", "-wal", "-shm"];
const EXACT_CONFIGURATION_TABLES: &[&str] = &[
    "model_provider_settings",
    "models",
    "model_provider_credential_staging",
    "model_provider_credential_cleanup",
    "mcp_builtin_capability_metadata",
    "mcp_builtin_capability_policies",
    "image_generation_profiles",
    "image_generation_credential_staging",
    "image_generation_credential_cleanup",
];
const PRESERVED_CONFIGURATION_TABLES: &[&str] = &[
    "model_provider_settings",
    "models",
    "ui_preferences",
    "agent_prompt_preferences",
    "skill_enablement_overrides",
    "image_generation_profiles",
    "notification_settings",
    "browser_download_settings",
    "browser_preferences",
    "mcp_registry_metadata",
    "mcp_registry_servers",
    "mcp_registry_model_namespaces",
    "mcp_builtin_capability_metadata",
    "mcp_builtin_capability_policies",
];

#[derive(Debug, Clone, PartialEq, Eq)]
struct ResetOptions {
    app_data_root: PathBuf,
    confirm_reset: bool,
}

struct PreservedConfiguration {
    model_count: usize,
    has_model_provider_settings: bool,
    exact_configuration_tables: Vec<ExactConfigurationTableSnapshot>,
    ui_preferences: UiPreferencesRecord,
    agent_prompt_preferences: AgentPromptPreferencesRecord,
    skill_enablement_overrides: Vec<(String, bool)>,
    image_generation_profile: Option<ImageGenerationProfileRecord>,
    image_generation_profile_count: usize,
    notification_settings: Option<NotificationSettingsRecord>,
    browser_download_settings: Option<BrowserDownloadSettingsRecord>,
    browser_preferences: Option<BrowserPreferencesRecord>,
    mcp_records: Vec<McpPersistedRegistryRecord>,
    mcp_server_count: usize,
}

impl std::fmt::Debug for PreservedConfiguration {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("PreservedConfiguration([REDACTED])")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ConfigurationColumnSignature {
    cid: i64,
    name: String,
    declared_type: String,
    not_null: bool,
    default_value: Option<String>,
    primary_key_position: i64,
}

#[derive(Debug, Clone, PartialEq)]
struct ExactConfigurationTableSnapshot {
    name: String,
    create_sql: String,
    columns: Vec<ConfigurationColumnSignature>,
    rows: Vec<Vec<SqliteValue>>,
}

impl ExactConfigurationTableSnapshot {
    fn has_same_schema(&self, other: &Self) -> bool {
        self.name == other.name
            && self.create_sql == other.create_sql
            && self.columns == other.columns
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ResetReport {
    database_path: PathBuf,
    backup_path: Option<PathBuf>,
    confirmed: bool,
    source_existed: bool,
    model_count: usize,
    skill_override_count: usize,
    mcp_server_count: usize,
    image_generation_profile_count: usize,
    discarded_conversation_rows: u64,
    preserved_configuration: bool,
}

impl ResetReport {
    fn render(&self) -> String {
        let mode = if self.confirmed {
            "completed"
        } else {
            "dry-run"
        };
        let backup = self
            .backup_path
            .as_ref()
            .map_or_else(|| "none".to_string(), |path| path.display().to_string());
        let source = if self.source_existed {
            "present"
        } else {
            "absent"
        };
        let preservation = if !self.source_existed {
            "not applicable"
        } else if self.preserved_configuration {
            "supported"
        } else {
            "skipped (unsupported schema; defaults used)"
        };
        format!(
            "Development storage reset {mode}\n\
             database: {}\n\
             source database: {source}\n\
             backup: {backup}\n\
             configuration preservation: {preservation}\n\
             preserved models: {}\n\
             preserved Skill overrides: {}\n\
             preserved MCP servers: {}\n\
             preserved image-generation profiles: {}\n\
             discarded non-configuration rows: {}",
            self.database_path.display(),
            self.model_count,
            self.skill_override_count,
            self.mcp_server_count,
            self.image_generation_profile_count,
            self.discarded_conversation_rows,
        )
    }
}

fn main() {
    match parse_options(std::env::args_os().skip(1)).and_then(execute) {
        Ok(report) => {
            println!("{}", report.render());
            if !report.confirmed {
                println!(
                    "No database or configuration files were changed. Re-run with {CONFIRM_RESET_FLAG} after closing the app."
                );
            }
        }
        Err(error) => {
            eprintln!("storage_reset_dev_failed: {error}");
            std::process::exit(1);
        }
    }
}

fn parse_options(arguments: impl IntoIterator<Item = OsString>) -> io::Result<ResetOptions> {
    let mut arguments = arguments.into_iter();
    let mut app_data_root = None;
    let mut confirm_reset = false;
    while let Some(argument) = arguments.next() {
        if argument == OsStr::new(APP_DATA_ROOT_FLAG) {
            if app_data_root.is_some() {
                return Err(invalid_input("--app-data-root may be supplied only once"));
            }
            app_data_root = Some(PathBuf::from(arguments.next().ok_or_else(|| {
                invalid_input("--app-data-root requires an absolute directory")
            })?));
        } else if argument == OsStr::new(CONFIRM_RESET_FLAG) {
            if confirm_reset {
                return Err(invalid_input("--confirm-reset may be supplied only once"));
            }
            confirm_reset = true;
        } else {
            return Err(invalid_input("unsupported storage reset argument"));
        }
    }
    let app_data_root = app_data_root
        .ok_or_else(|| invalid_input("--app-data-root is required and must come from Electron"))?;
    Ok(ResetOptions {
        app_data_root,
        confirm_reset,
    })
}

fn execute(options: ResetOptions) -> io::Result<ResetReport> {
    let app_data_root = validate_app_data_root(&options.app_data_root)?;
    let database_path = app_data_root.join(DATABASE_FILE_NAME);
    let _instance_lock = acquire_database_instance_lock(&database_path).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!(
                "close Captain Who before resetting `{}`: {error}",
                database_path.display()
            ),
        )
    })?;
    let source_existed = validate_optional_database(&database_path)?;

    if !options.confirm_reset {
        let (configuration, discarded_conversation_rows) = if source_existed {
            inspect_source(&database_path, None)?
        } else {
            (None, 0)
        };
        return Ok(report_from_configuration(
            database_path,
            None,
            false,
            source_existed,
            configuration.as_ref(),
            discarded_conversation_rows,
        ));
    }

    let backup_path = source_existed
        .then(|| create_database_backup(&app_data_root, &database_path))
        .transpose()?;
    let backup_digest = backup_path.as_deref().map(file_digest).transpose()?;
    let mcp_working_snapshot = backup_path
        .as_deref()
        .map(|backup| TemporaryDatabaseSnapshot::from_source(&app_data_root, backup, "mcp"))
        .transpose()?;
    let (configuration, discarded_conversation_rows) = match backup_path.as_deref() {
        Some(backup_path) => inspect_source(
            backup_path,
            mcp_working_snapshot
                .as_ref()
                .map(TemporaryDatabaseSnapshot::path),
        )?,
        None => (None, 0),
    };
    if let (Some(backup_path), Some(expected_digest)) =
        (backup_path.as_deref(), backup_digest.as_ref())
    {
        if &file_digest(backup_path)? != expected_digest {
            return Err(invalid_data(
                "the recovery backup changed while configuration was inspected",
            ));
        }
    }
    drop(mcp_working_snapshot);

    let mut temporary_files = TemporaryDatabaseFiles::new(&app_data_root)?;
    build_fresh_database(temporary_files.staging(), configuration.as_ref())?;
    create_private_empty_file(temporary_files.publication())?;
    create_verified_sqlite_snapshot(temporary_files.staging(), temporary_files.publication())?;
    verify_fresh_database(temporary_files.publication(), configuration.as_ref())?;

    if source_existed {
        checkpoint_source_and_remove_transients(&database_path)?;
    }
    atomic_replace(temporary_files.publication(), &database_path)?;
    temporary_files.mark_published();
    sync_directory(&app_data_root)?;

    Ok(report_from_configuration(
        database_path,
        backup_path,
        true,
        source_existed,
        configuration.as_ref(),
        discarded_conversation_rows,
    ))
}

fn validate_app_data_root(root: &Path) -> io::Result<PathBuf> {
    if root.as_os_str().is_empty() || !root.is_absolute() {
        return Err(invalid_input(
            "Electron application data root must be an absolute directory",
        ));
    }
    let metadata = fs::symlink_metadata(root)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "Electron application data root must be a real directory: {}",
                root.display()
            ),
        ));
    }
    fs::canonicalize(root)
}

fn validate_optional_database(database_path: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(database_path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "storage database must be a regular non-symlink file: {}",
                    database_path.display()
                ),
            ))
        }
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn inspect_source(
    source_path: &Path,
    mutable_mcp_snapshot: Option<&Path>,
) -> io::Result<(Option<PreservedConfiguration>, u64)> {
    let connection = open_read_only(source_path)?;
    let schema_version = storage_schema_version(&connection)?;
    if !can_preserve_development_configuration(schema_version) {
        return Ok((None, count_all_business_rows(&connection)?));
    }
    validate_source_database_integrity(&connection)?;
    mycopilot_core::storage::migrations::run_migrations(&connection)
        .map_err(|_| invalid_data("current development storage schema is not exactly canonical"))?;
    validate_model_configuration_schema(&connection)?;
    validate_model_profile_rows_without_credentials(&connection)?;
    let exact_configuration_tables = load_exact_configuration_table_snapshots(&connection)?;
    let model_count = count_rows_if_table_exists(&connection, "models")? as usize;
    let has_model_provider_settings =
        count_rows_if_table_exists(&connection, "model_provider_settings")? == 1;
    let ui_preferences = load_ui_preferences_for_development_reset(&connection)?;
    let agent_prompt_preferences =
        agent_prompt_preferences_repository::load_agent_prompt_preferences(&connection)
            .map_err(redacted_storage_error)?;
    let skill_enablement_overrides = load_skill_enablement_overrides(&connection)?;
    let image_generation_profile = image_generation_repository::load_image_generation_profile(
        &connection,
        DEFAULT_IMAGE_GENERATION_PROFILE_ID,
    )
    .map_err(redacted_storage_error)?;
    let image_generation_profile_count =
        count_rows_if_table_exists(&connection, "image_generation_profiles")? as usize;
    ensure_no_image_credential_reconciliation(&connection)?;
    ensure_no_model_provider_credential_reconciliation(&connection)?;
    let notification_settings =
        if count_rows_if_table_exists(&connection, "notification_settings")? == 1 {
            Some(
                notification_repository::load_notification_settings(&connection)
                    .map_err(redacted_storage_error)?,
            )
        } else {
            None
        };
    let browser_download_settings =
        if count_rows_if_table_exists(&connection, "browser_download_settings")? == 1 {
            Some(load_browser_download_settings_for_development_reset(
                &connection,
            )?)
        } else {
            None
        };
    let browser_preferences =
        if count_rows_if_table_exists(&connection, "browser_preferences")? == 1 {
            Some(
                browser_data_repository::load_preferences(&connection)
                    .map_err(redacted_storage_error)?,
            )
        } else {
            None
        };
    let discarded_conversation_rows = count_discarded_conversation_rows(&connection)?;
    let expected_mcp_count = count_rows_if_table_exists(&connection, "mcp_registry_servers")?;
    drop(connection);

    let mcp_records = match mutable_mcp_snapshot {
        Some(snapshot) => load_mcp_records(snapshot)?,
        None if expected_mcp_count == 0 => Vec::new(),
        None => {
            // Dry-run must remain read-only. Exact MCP decoding/reconciliation is performed from
            // the already-created backup only after explicit confirmation.
            Vec::with_capacity(expected_mcp_count as usize)
        }
    };
    let mcp_count = if mutable_mcp_snapshot.is_some() {
        mcp_records.len()
    } else {
        expected_mcp_count as usize
    };
    let configuration = PreservedConfiguration {
        model_count,
        has_model_provider_settings,
        exact_configuration_tables,
        ui_preferences,
        agent_prompt_preferences,
        skill_enablement_overrides,
        image_generation_profile,
        image_generation_profile_count,
        notification_settings,
        browser_download_settings,
        browser_preferences,
        mcp_records,
        mcp_server_count: mcp_count,
    };
    Ok((Some(configuration), discarded_conversation_rows))
}

fn storage_schema_version(connection: &Connection) -> io::Result<i32> {
    connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(redacted_storage_error)
}

fn validate_source_database_integrity(connection: &Connection) -> io::Result<()> {
    let quick_check = pragma_rows(connection, "PRAGMA quick_check")?;
    if quick_check.as_slice() != ["ok"] {
        return Err(invalid_data("source database failed SQLite quick_check"));
    }
    if !pragma_rows(connection, "PRAGMA foreign_key_check")?.is_empty() {
        return Err(invalid_data("source database failed foreign_key_check"));
    }
    Ok(())
}

fn validate_model_profile_rows_without_credentials(connection: &Connection) -> io::Result<()> {
    let mut statement = connection
        .prepare(
            "SELECT display_name, provider_profile_config_json, provider_protocol_revision
             FROM models ORDER BY position ASC, created_at ASC",
        )
        .map_err(redacted_storage_error)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(redacted_storage_error)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(redacted_storage_error)?;
    for (label, profile, revision) in rows {
        decode_reset_provider_profile(&label, &profile, &revision)?;
    }
    Ok(())
}

fn can_preserve_development_configuration(schema_version: i32) -> bool {
    schema_version == mycopilot_core::storage::migrations::STORAGE_SCHEMA_VERSION
}

fn validate_model_configuration_schema(source: &Connection) -> io::Result<()> {
    let canonical = Connection::open_in_memory().map_err(redacted_storage_error)?;
    mycopilot_core::storage::migrations::run_migrations(&canonical)
        .map_err(redacted_storage_error)?;
    let current_provider =
        snapshot_configuration_table_schema(&canonical, "model_provider_settings")?;
    let source_provider = snapshot_configuration_table_schema(source, "model_provider_settings")?;
    if source_provider != current_provider {
        return Err(invalid_data(
            "configuration table schema differs for model provider settings",
        ));
    }

    let current_models = snapshot_configuration_table_schema(&canonical, "models")?;
    let source_models = snapshot_configuration_table_schema(source, "models")?;
    if source_models != current_models {
        return Err(invalid_data(
            "configuration table schema differs for models",
        ));
    }
    Ok(())
}

fn load_exact_configuration_table_snapshots(
    source: &Connection,
) -> io::Result<Vec<ExactConfigurationTableSnapshot>> {
    let source_snapshots =
        snapshot_exact_configuration_tables_named(source, EXACT_CONFIGURATION_TABLES)?;
    let canonical = Connection::open_in_memory().map_err(redacted_storage_error)?;
    mycopilot_core::storage::migrations::run_migrations(&canonical)
        .map_err(redacted_storage_error)?;
    let canonical_snapshots =
        snapshot_exact_configuration_tables_named(&canonical, EXACT_CONFIGURATION_TABLES)?;
    if source_snapshots.len() != canonical_snapshots.len()
        || source_snapshots
            .iter()
            .zip(&canonical_snapshots)
            .any(|(source, canonical)| !source.has_same_schema(canonical))
    {
        return Err(invalid_data(
            "configuration table schema differs from the canonical development schema",
        ));
    }
    Ok(source_snapshots)
}

#[cfg(test)]
fn snapshot_exact_configuration_tables(
    connection: &Connection,
) -> io::Result<Vec<ExactConfigurationTableSnapshot>> {
    snapshot_exact_configuration_tables_named(connection, EXACT_CONFIGURATION_TABLES)
}

fn snapshot_exact_configuration_tables_named(
    connection: &Connection,
    tables: &[&str],
) -> io::Result<Vec<ExactConfigurationTableSnapshot>> {
    tables
        .iter()
        .map(|table| snapshot_exact_configuration_table(connection, table))
        .collect()
}

fn snapshot_exact_configuration_table(
    connection: &Connection,
    table: &str,
) -> io::Result<ExactConfigurationTableSnapshot> {
    let (create_sql, columns) = snapshot_configuration_table_schema(connection, table)?;
    let projection = columns
        .iter()
        .map(|column| quote_sqlite_identifier(&column.name))
        .collect::<Vec<_>>()
        .join(", ");
    let table_identifier = quote_sqlite_identifier(table);
    let mut statement = connection
        .prepare(&format!(
            "SELECT {projection} FROM {table_identifier} ORDER BY rowid"
        ))
        .map_err(redacted_storage_error)?;
    let column_count = columns.len();
    let rows = statement
        .query_map([], |row| {
            (0..column_count)
                .map(|index| row.get::<_, SqliteValue>(index))
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .map_err(redacted_storage_error)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(redacted_storage_error)?;
    Ok(ExactConfigurationTableSnapshot {
        name: table.to_string(),
        create_sql,
        columns,
        rows,
    })
}

fn snapshot_configuration_table_schema(
    connection: &Connection,
    table: &str,
) -> io::Result<(String, Vec<ConfigurationColumnSignature>)> {
    let create_sql = connection
        .query_row(
            "SELECT sql FROM sqlite_schema WHERE type = 'table' AND name = ?1",
            [table],
            |row| row.get::<_, String>(0),
        )
        .map_err(redacted_storage_error)?;
    let columns = {
        let mut statement = connection
            .prepare(
                "SELECT cid, name, type, \"notnull\", dflt_value, pk
                 FROM pragma_table_info(?1)
                 ORDER BY cid",
            )
            .map_err(redacted_storage_error)?;
        let columns = statement
            .query_map([table], |row| {
                Ok(ConfigurationColumnSignature {
                    cid: row.get(0)?,
                    name: row.get(1)?,
                    declared_type: row.get(2)?,
                    not_null: row.get::<_, i64>(3)? != 0,
                    default_value: row.get(4)?,
                    primary_key_position: row.get(5)?,
                })
            })
            .map_err(redacted_storage_error)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(redacted_storage_error)?;
        columns
    };
    if columns.is_empty() {
        return Err(invalid_data(format!(
            "configuration table {table} has no columns"
        )));
    }
    Ok((create_sql, columns))
}

fn quote_sqlite_identifier(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}

fn restore_exact_configuration_tables(
    database_path: &Path,
    expected: &[ExactConfigurationTableSnapshot],
) -> io::Result<()> {
    let mut connection = Connection::open(database_path).map_err(redacted_storage_error)?;
    let transaction = connection.transaction().map_err(redacted_storage_error)?;
    for snapshot in expected {
        let current = snapshot_exact_configuration_table(&transaction, &snapshot.name)?;
        if !current.has_same_schema(snapshot) {
            return Err(invalid_data(format!(
                "fresh configuration table {} has an incompatible schema",
                snapshot.name
            )));
        }
        if !current.rows.is_empty() {
            return Err(invalid_data(format!(
                "fresh configuration table {} was unexpectedly populated",
                snapshot.name
            )));
        }
        if snapshot.rows.is_empty() {
            continue;
        }
        let table_identifier = quote_sqlite_identifier(&snapshot.name);
        let projection = snapshot
            .columns
            .iter()
            .map(|column| quote_sqlite_identifier(&column.name))
            .collect::<Vec<_>>()
            .join(", ");
        let placeholders = (1..=snapshot.columns.len())
            .map(|index| format!("?{index}"))
            .collect::<Vec<_>>()
            .join(", ");
        let mut statement = transaction
            .prepare(&format!(
                "INSERT INTO {table_identifier} ({projection}) VALUES ({placeholders})"
            ))
            .map_err(redacted_storage_error)?;
        for row in &snapshot.rows {
            if row.len() != snapshot.columns.len() {
                return Err(invalid_data(format!(
                    "configuration table {} snapshot row width changed",
                    snapshot.name
                )));
            }
            statement
                .execute(params_from_iter(row.iter()))
                .map_err(redacted_storage_error)?;
        }
    }
    transaction.commit().map_err(redacted_storage_error)
}

/// Preserves UI configuration only from the current development schema.
///
/// Conversation, checkpoint, pending-action, and Agent runtime records are intentionally never
/// decoded or migrated by this reset utility.
fn load_ui_preferences_for_development_reset(
    connection: &Connection,
) -> io::Result<UiPreferencesRecord> {
    let schema_version = storage_schema_version(connection)?;
    if can_preserve_development_configuration(schema_version) {
        return preferences_repository::load_ui_preferences(connection)
            .map_err(redacted_storage_error);
    }

    Err(invalid_data(
        "only the current development UI preference schema can be preserved",
    ))
}

fn load_browser_download_settings_for_development_reset(
    connection: &Connection,
) -> io::Result<BrowserDownloadSettingsRecord> {
    let schema_version = storage_schema_version(connection)?;
    if can_preserve_development_configuration(schema_version) {
        return browser_download_repository::load_settings(connection)
            .map_err(redacted_storage_error);
    }

    Err(invalid_data(
        "only the current development browser download settings schema can be preserved",
    ))
}

fn decode_reset_provider_profile(
    model_label: &str,
    profile_json: &str,
    provider_protocol_revision: &str,
) -> io::Result<ProviderProfileConfig> {
    if !is_provider_protocol_revision(provider_protocol_revision) {
        return Err(invalid_data(format!(
            "model `{model_label}` does not use the current Provider Protocol revision"
        )));
    }
    serde_json::from_str::<ProviderProfileConfig>(profile_json).map_err(|_| {
        invalid_data(format!(
            "model `{model_label}` has an unreadable Provider Profile configuration"
        ))
    })
}

fn load_skill_enablement_overrides(connection: &Connection) -> io::Result<Vec<(String, bool)>> {
    let mut statement = connection
        .prepare("SELECT skill_id, enabled FROM skill_enablement_overrides ORDER BY skill_id ASC")
        .map_err(redacted_storage_error)?;
    let records = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .map_err(redacted_storage_error)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(redacted_storage_error)?;
    Ok(records)
}

fn ensure_no_image_credential_reconciliation(connection: &Connection) -> io::Result<()> {
    let staged = count_rows_if_table_exists(connection, "image_generation_credential_staging")?;
    let cleanup = count_rows_if_table_exists(connection, "image_generation_credential_cleanup")?;
    if staged != 0 || cleanup != 0 {
        return Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "image-generation credential reconciliation is pending; start and cleanly close Captain Who before resetting",
        ));
    }
    Ok(())
}

fn ensure_no_model_provider_credential_reconciliation(connection: &Connection) -> io::Result<()> {
    let staged = count_rows_if_table_exists(connection, "model_provider_credential_staging")?;
    let cleanup = count_rows_if_table_exists(connection, "model_provider_credential_cleanup")?;
    if staged != 0 || cleanup != 0 {
        return Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "model-provider credential reconciliation is pending; start and cleanly close Captain Who before resetting",
        ));
    }
    Ok(())
}

fn load_mcp_records(database_path: &Path) -> io::Result<Vec<McpPersistedRegistryRecord>> {
    let registry = SqliteMcpRegistry::open(database_path).map_err(redacted_mcp_error)?;
    let (_, records) = registry.snapshot().map_err(redacted_mcp_error)?;
    Ok(records)
}

fn development_model_credential_store(
    database_path: &Path,
) -> io::Result<Arc<dyn CredentialStore>> {
    let root = development_model_credential_root(database_path);
    DevelopmentFileCredentialStore::new(root)
        .map(|store| Arc::new(store) as Arc<dyn CredentialStore>)
        .map_err(|_| io::Error::other("failed to initialize development model credential store"))
}

fn development_model_credential_root(database_path: &Path) -> PathBuf {
    database_path
        .parent()
        .map(|parent| parent.join("model-provider-development-credentials-v1"))
        .unwrap_or_else(|| PathBuf::from("model-provider-development-credentials-v1"))
}

fn build_fresh_database(
    database_path: &Path,
    configuration: Option<&PreservedConfiguration>,
) -> io::Result<()> {
    create_private_empty_file(database_path)?;
    let model_credentials = development_model_credential_store(database_path)?;
    let storage = StorageService::open_for_development_reset_with_model_credentials(
        database_path,
        model_credentials,
    )
    .map_err(|_| io::Error::other("failed to create the canonical storage schema"))?;
    if let Some(configuration) = configuration {
        storage
            .save_ui_preferences(configuration.ui_preferences.clone())
            .map_err(|_| io::Error::other("failed to restore UI preferences"))?;
        storage
            .save_agent_prompt_preferences(configuration.agent_prompt_preferences.clone())
            .map_err(|_| io::Error::other("failed to restore agent prompt preferences"))?;
        for (skill_id, enabled) in &configuration.skill_enablement_overrides {
            storage
                .set_skill_enablement_override(skill_id, *enabled)
                .map_err(|_| io::Error::other("failed to restore a Skill enablement override"))?;
        }
        if let Some(preferences) = &configuration.browser_preferences {
            storage
                .save_browser_preferences(BrowserPreferencesUpdate {
                    schema_version: BROWSER_DATA_SCHEMA_VERSION,
                    link_open_target: preferences.link_open_target,
                    expected_revision: 0,
                    updated_at: preferences.updated_at,
                })
                .map_err(|_| io::Error::other("failed to restore browser preferences"))?;
        }
    }
    drop(storage);

    if let Some(configuration) = configuration {
        restore_exact_configuration_tables(
            database_path,
            &configuration.exact_configuration_tables,
        )?;
    }
    if let Some(settings) = configuration.and_then(|value| value.notification_settings.as_ref()) {
        restore_notification_settings(database_path, settings)?;
    }
    if let Some(settings) = configuration.and_then(|value| value.browser_download_settings.as_ref())
    {
        restore_browser_download_settings(database_path, settings)?;
    }

    if let Some(configuration) = configuration {
        restore_mcp_records(database_path, &configuration.mcp_records)?;
    }
    Ok(())
}

fn restore_notification_settings(
    database_path: &Path,
    settings: &NotificationSettingsRecord,
) -> io::Result<()> {
    let connection = Connection::open(database_path).map_err(redacted_storage_error)?;
    let changed = connection
        .execute(
            "UPDATE notification_settings
             SET enabled = ?1, sound_enabled = ?2, show_task_content = ?3,
                 human_completed_enabled = ?4, human_failed_enabled = ?5,
                 human_approval_enabled = ?6, human_cancelled_enabled = ?7,
                 revision = ?8, updated_at = ?9
             WHERE singleton_id = 1",
            params![
                settings.enabled,
                settings.sound_enabled,
                settings.show_task_content,
                settings.human_completed_enabled,
                settings.human_failed_enabled,
                settings.human_approval_enabled,
                settings.human_cancelled_enabled,
                settings.revision,
                settings.updated_at,
            ],
        )
        .map_err(redacted_storage_error)?;
    if changed != 1 {
        return Err(io::Error::other(
            "failed to restore the notification settings singleton",
        ));
    }
    connection
        .close()
        .map_err(|(_, error)| redacted_storage_error(error))
}

fn restore_browser_download_settings(
    database_path: &Path,
    settings: &BrowserDownloadSettingsRecord,
) -> io::Result<()> {
    let connection = Connection::open(database_path).map_err(redacted_storage_error)?;
    let location_mode = match settings.location_mode {
        mycopilot_core::storage::models::BrowserDownloadLocationMode::System => "system",
        mycopilot_core::storage::models::BrowserDownloadLocationMode::Custom => "custom",
    };
    let changed = connection
        .execute(
            "UPDATE browser_download_settings
             SET schema_version = ?1, location_mode = ?2, custom_directory = ?3,
                 ask_where_to_save = ?4, revision = ?5, updated_at = ?6
             WHERE id = 'default'",
            params![
                settings.schema_version,
                location_mode,
                settings.custom_directory,
                settings.ask_where_to_save,
                settings.revision,
                settings.updated_at
            ],
        )
        .map_err(redacted_storage_error)?;
    if changed != 1 {
        return Err(invalid_data(
            "failed to restore browser download settings singleton",
        ));
    }
    connection
        .close()
        .map_err(|(_, error)| redacted_storage_error(error))
}

fn restore_mcp_records(
    database_path: &Path,
    records: &[McpPersistedRegistryRecord],
) -> io::Result<()> {
    let registry = SqliteMcpRegistry::open(database_path).map_err(redacted_mcp_error)?;
    for source in records {
        let original = source.entry.config.clone();
        let was_enabled = original.enabled;
        let was_user_approved = original.trust == McpTrustLevel::UserApproved;
        if was_user_approved && !launch_authorization_is_valid(source) {
            return Err(invalid_data(
                "an MCP launch authorization is no longer valid; review that server before resetting",
            ));
        }
        let mut initial = original;
        initial.enabled = false;
        initial.trust = McpTrustLevel::Untrusted;
        let mut entry = registry
            .add_persisted(initial, Some(source.entry.model_namespace.clone()))
            .map_err(redacted_mcp_error)?;

        if was_user_approved {
            let persisted = registry
                .get_persisted(entry.config.id)
                .map_err(redacted_mcp_error)?
                .ok_or_else(|| io::Error::other("restored MCP server disappeared"))?;
            let source_authorization = source
                .launch_authorization
                .as_ref()
                .ok_or_else(|| invalid_data("approved MCP server has no launch authorization"))?;
            let restored_authorization = registry
                .authorize_launch(
                    &McpRegistryMutationPrecondition::from_entry(&entry),
                    &persisted.launch_spec_digest,
                    &source_authorization.file_identity_digest,
                    source_authorization.authorization_policy_version,
                    source_authorization.authorized_at_ms,
                )
                .map_err(redacted_mcp_error)?;
            if restored_authorization.server_id != source_authorization.server_id
                || restored_authorization.launch_spec_digest
                    != source_authorization.launch_spec_digest
                || restored_authorization.file_identity_digest
                    != source_authorization.file_identity_digest
                || restored_authorization.authorization_format_version
                    != source_authorization.authorization_format_version
                || restored_authorization.authorization_policy_version
                    != source_authorization.authorization_policy_version
                || restored_authorization.authorized_at_ms != source_authorization.authorized_at_ms
            {
                return Err(invalid_data(
                    "restored MCP launch authorization differs from its preserved identity",
                ));
            }
            entry = registry
                .get(entry.config.id)
                .map_err(|_| io::Error::other("failed to reload restored MCP server"))?
                .ok_or_else(|| io::Error::other("restored MCP server disappeared"))?;
        }
        if was_enabled {
            registry
                .set_enabled(&McpRegistryMutationPrecondition::from_entry(&entry), true)
                .map_err(redacted_mcp_error)?;
        }
    }
    let restored = registry
        .list()
        .map_err(|_| io::Error::other("failed to verify restored MCP server configurations"))?;
    if restored.len() != records.len() {
        return Err(io::Error::other(
            "restored MCP server count does not match the preserved snapshot",
        ));
    }
    for source in records {
        let restored_entry = restored
            .iter()
            .find(|entry| entry.config.id == source.entry.config.id)
            .ok_or_else(|| io::Error::other("a preserved MCP server was not restored"))?;
        if restored_entry.config != source.entry.config
            || restored_entry.model_namespace != source.entry.model_namespace
        {
            return Err(invalid_data(
                "a restored MCP server identity differs from its preserved value",
            ));
        }
    }
    Ok(())
}

fn verify_fresh_database(
    database_path: &Path,
    configuration: Option<&PreservedConfiguration>,
) -> io::Result<()> {
    // Reopen through the exact Core production boundary after MCP restoration. This proves the
    // Core canonical catalog and the independently owned MCP Registry schema can coexist across
    // an application restart.
    let storage = StorageService::open_for_development_reset_with_model_credentials(
        database_path,
        development_model_credential_store(database_path)?,
    )
    .map_err(|_| {
        io::Error::other("fresh database was rejected by the canonical storage boundary")
    })?;
    if let Some(configuration) = configuration {
        let restored_editor = storage
            .load_model_settings_for_edit()
            .map_err(|_| io::Error::other("restored model settings could not be verified"))?;
        if restored_editor
            .as_ref()
            .map_or(0, |settings| settings.models.len())
            != configuration.model_count
            || restored_editor.is_some() != configuration.has_model_provider_settings
        {
            return Err(invalid_data("restored model settings count mismatch"));
        }
        if let Some(expected) = &configuration.image_generation_profile {
            let restored = storage
                .load_image_generation_profile(DEFAULT_IMAGE_GENERATION_PROFILE_ID)
                .map_err(|_| {
                    io::Error::other(
                        "restored image-generation configuration could not be verified",
                    )
                })?;
            if restored.as_ref() != Some(expected) {
                return Err(invalid_data(
                    "restored image-generation configuration differs from its preserved value",
                ));
            }
        }
    }
    drop(storage);
    let connection = open_read_only(database_path)?;
    if let Some(configuration) = configuration {
        let table_names = configuration
            .exact_configuration_tables
            .iter()
            .map(|table| table.name.as_str())
            .collect::<Vec<_>>();
        let restored_configuration =
            snapshot_exact_configuration_tables_named(&connection, &table_names)?;
        if restored_configuration != configuration.exact_configuration_tables {
            return Err(invalid_data(
                "restored image-generation configuration differs from its exact snapshot",
            ));
        }
    }
    let quick_check = pragma_rows(&connection, "PRAGMA quick_check")?;
    if quick_check.as_slice() != ["ok"] {
        return Err(invalid_data("fresh database failed SQLite quick_check"));
    }
    let foreign_key_errors = pragma_rows(&connection, "PRAGMA foreign_key_check")?;
    if !foreign_key_errors.is_empty() {
        return Err(invalid_data("fresh database failed foreign_key_check"));
    }
    ensure_only_configuration_tables_have_rows(&connection)?;
    if let Some(configuration) = configuration {
        let model_count = count_rows_if_table_exists(&connection, "models")? as usize;
        let expected_models = configuration.model_count;
        if model_count != expected_models {
            return Err(invalid_data("fresh database model count mismatch"));
        }
        let expected_settings_rows = usize::from(configuration.has_model_provider_settings);
        verify_table_count(
            &connection,
            "model_provider_settings",
            expected_settings_rows,
        )?;
        verify_table_count(&connection, "ui_preferences", 1)?;
        verify_table_count(&connection, "agent_prompt_preferences", 1)?;
        verify_table_count(&connection, "notification_settings", 1)?;
        verify_table_count(&connection, "browser_download_settings", 1)?;
        verify_table_count(&connection, "browser_preferences", 1)?;
        if let Some(expected) = &configuration.notification_settings {
            let restored = notification_repository::load_notification_settings(&connection)
                .map_err(redacted_storage_error)?;
            if &restored != expected {
                return Err(invalid_data(
                    "restored notification settings differ from their preserved value",
                ));
            }
        }
        if let Some(expected) = &configuration.browser_download_settings {
            let restored = browser_download_repository::load_settings(&connection)
                .map_err(redacted_storage_error)?;
            if &restored != expected {
                return Err(invalid_data(
                    "restored browser download settings differ from their preserved value",
                ));
            }
        }
        if let Some(expected) = &configuration.browser_preferences {
            let restored = browser_data_repository::load_preferences(&connection)
                .map_err(redacted_storage_error)?;
            if restored.link_open_target != expected.link_open_target {
                return Err(invalid_data(
                    "restored browser preferences differ from their preserved value",
                ));
            }
        }
        let skill_count =
            count_rows_if_table_exists(&connection, "skill_enablement_overrides")? as usize;
        if skill_count != configuration.skill_enablement_overrides.len() {
            return Err(invalid_data(
                "fresh database Skill enablement override count mismatch",
            ));
        }
        let mcp_count = count_rows_if_table_exists(&connection, "mcp_registry_servers")? as usize;
        if mcp_count != configuration.mcp_records.len() {
            return Err(invalid_data("fresh database MCP server count mismatch"));
        }
        verify_table_count(
            &connection,
            "mcp_registry_model_namespaces",
            configuration.mcp_records.len(),
        )?;
        verify_table_count(&connection, "mcp_registry_metadata", 1)?;
        verify_table_count(
            &connection,
            "image_generation_profiles",
            configuration.image_generation_profile_count,
        )?;
    }
    Ok(())
}

fn verify_table_count(connection: &Connection, table: &str, expected: usize) -> io::Result<()> {
    let actual = count_rows_if_table_exists(connection, table)? as usize;
    if actual != expected {
        return Err(invalid_data(format!(
            "fresh database table `{table}` has {actual} rows; expected {expected}"
        )));
    }
    Ok(())
}

fn create_database_backup(root: &Path, database_path: &Path) -> io::Result<PathBuf> {
    let backup_directory = root.join(BACKUP_DIRECTORY_NAME);
    create_private_directory(&backup_directory)?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| io::Error::other("system clock is before the Unix epoch"))?
        .as_millis();
    let backup_path = backup_directory.join(format!(
        "storage-reset-dev-{timestamp}-{}.sqlite",
        std::process::id()
    ));
    create_private_empty_file(&backup_path)?;
    if let Err(error) = create_verified_sqlite_snapshot(database_path, &backup_path) {
        let _ = fs::remove_file(&backup_path);
        return Err(error);
    }
    sync_directory(&backup_directory)?;
    Ok(backup_path)
}

fn file_digest(path: &Path) -> io::Result<[u8; 32]> {
    let mut file = File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(digest.finalize().into())
}

fn checkpoint_source_and_remove_transients(database_path: &Path) -> io::Result<()> {
    let connection = Connection::open_with_flags(
        database_path,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(redacted_storage_error)?;
    connection
        .busy_timeout(std::time::Duration::from_secs(1))
        .map_err(redacted_storage_error)?;
    let (busy, _, _) = connection
        .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })
        .map_err(redacted_storage_error)?;
    if busy != 0 {
        return Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "SQLite refused the final checkpoint; close every process using this database",
        ));
    }
    connection
        .execute_batch("BEGIN EXCLUSIVE; COMMIT;")
        .map_err(|_| {
            io::Error::new(
                io::ErrorKind::WouldBlock,
                "SQLite database is still in use by another process",
            )
        })?;
    connection
        .close()
        .map_err(|(_, error)| redacted_storage_error(error))?;
    for suffix in SQLITE_TRANSIENT_SUFFIXES {
        let transient = sqlite_transient_path(database_path, suffix);
        match fs::symlink_metadata(&transient) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
                return Err(invalid_data(format!(
                    "SQLite transient is not a regular file: {}",
                    transient.display()
                )));
            }
            Ok(_) => fs::remove_file(&transient)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn open_read_only(path: &Path) -> io::Result<Connection> {
    Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(redacted_storage_error)
}

fn count_discarded_conversation_rows(connection: &Connection) -> io::Result<u64> {
    application_business_tables(connection)?
        .into_iter()
        .filter(|table| !PRESERVED_CONFIGURATION_TABLES.contains(&table.as_str()))
        .try_fold(0_u64, |total, table| {
            total
                .checked_add(count_rows_if_table_exists(connection, &table)?)
                .ok_or_else(|| invalid_data("discarded row count overflow"))
        })
}

fn count_all_business_rows(connection: &Connection) -> io::Result<u64> {
    application_business_tables(connection)?
        .into_iter()
        .try_fold(0_u64, |total, table| {
            total
                .checked_add(count_rows_if_table_exists(connection, &table)?)
                .ok_or_else(|| invalid_data("discarded row count overflow"))
        })
}

fn ensure_only_configuration_tables_have_rows(connection: &Connection) -> io::Result<()> {
    for table in application_business_tables(connection)? {
        if PRESERVED_CONFIGURATION_TABLES.contains(&table.as_str()) {
            continue;
        }
        if count_rows_if_table_exists(connection, &table)? != 0 {
            return Err(invalid_data(format!(
                "fresh database contains rows in non-configuration table `{table}`"
            )));
        }
    }
    Ok(())
}

fn application_business_tables(connection: &Connection) -> io::Result<Vec<String>> {
    let mut statement = connection
        .prepare(
            "SELECT name FROM sqlite_schema
             WHERE type = 'table'
               AND name NOT LIKE 'sqlite_%'
               AND name NOT GLOB 'conversation_history_fts_*'
             ORDER BY name ASC",
        )
        .map_err(redacted_storage_error)?;
    let tables = statement
        .query_map([], |row| row.get(0))
        .map_err(redacted_storage_error)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(redacted_storage_error)?;
    Ok(tables)
}

fn count_rows_if_table_exists(connection: &Connection, table: &str) -> io::Result<u64> {
    let exists = connection
        .query_row(
            "SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = ?1",
            [table],
            |_| Ok(()),
        )
        .optional()
        .map_err(redacted_storage_error)?
        .is_some();
    if !exists {
        return Ok(0);
    }
    if !table
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Err(invalid_input("unsafe static SQLite table identifier"));
    }
    connection
        .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
            row.get::<_, u64>(0)
        })
        .map_err(redacted_storage_error)
}

fn pragma_rows(connection: &Connection, pragma: &str) -> io::Result<Vec<String>> {
    let mut statement = connection.prepare(pragma).map_err(redacted_storage_error)?;
    let rows = statement
        .query_map([], |row| row.get(0))
        .map_err(redacted_storage_error)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(redacted_storage_error)?;
    Ok(rows)
}

fn report_from_configuration(
    database_path: PathBuf,
    backup_path: Option<PathBuf>,
    confirmed: bool,
    source_existed: bool,
    configuration: Option<&PreservedConfiguration>,
    discarded_conversation_rows: u64,
) -> ResetReport {
    ResetReport {
        database_path,
        backup_path,
        confirmed,
        source_existed,
        model_count: configuration.map_or(0, |configuration| configuration.model_count),
        skill_override_count: configuration.map_or(0, |configuration| {
            configuration.skill_enablement_overrides.len()
        }),
        mcp_server_count: configuration.map_or(0, |configuration| configuration.mcp_server_count),
        image_generation_profile_count: configuration.map_or(0, |configuration| {
            configuration.image_generation_profile_count
        }),
        discarded_conversation_rows,
        preserved_configuration: configuration.is_some(),
    }
}

struct TemporaryDatabaseFiles {
    staging: PathBuf,
    publication: PathBuf,
    published: bool,
}

struct TemporaryDatabaseSnapshot {
    path: PathBuf,
}

impl TemporaryDatabaseSnapshot {
    fn from_source(root: &Path, source: &Path, label: &str) -> io::Result<Self> {
        let nonce = format!(
            "{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|_| io::Error::other("system clock is before the Unix epoch"))?
                .as_nanos()
        );
        let path = root.join(format!(".storage.sqlite.reset-dev-{label}-{nonce}.working"));
        create_private_empty_file(&path)?;
        if let Err(error) = create_verified_sqlite_snapshot(source, &path) {
            remove_database_files(&path);
            return Err(error);
        }
        Ok(Self { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TemporaryDatabaseSnapshot {
    fn drop(&mut self) {
        remove_database_files(&self.path);
    }
}

impl TemporaryDatabaseFiles {
    fn new(root: &Path) -> io::Result<Self> {
        let nonce = format!(
            "{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|_| io::Error::other("system clock is before the Unix epoch"))?
                .as_nanos()
        );
        Ok(Self {
            staging: root.join(format!(".storage.sqlite.reset-dev-{nonce}.staging")),
            publication: root.join(format!(".storage.sqlite.reset-dev-{nonce}.publish")),
            published: false,
        })
    }

    fn staging(&self) -> &Path {
        &self.staging
    }

    fn publication(&self) -> &Path {
        &self.publication
    }

    fn mark_published(&mut self) {
        self.published = true;
    }
}

impl Drop for TemporaryDatabaseFiles {
    fn drop(&mut self) {
        remove_database_files(&self.staging);
        if !self.published {
            remove_database_files(&self.publication);
        }
    }
}

fn remove_database_files(database_path: &Path) {
    let _ = fs::remove_file(database_path);
    for suffix in SQLITE_TRANSIENT_SUFFIXES {
        let _ = fs::remove_file(sqlite_transient_path(database_path, suffix));
    }
}

fn sqlite_transient_path(database_path: &Path, suffix: &str) -> PathBuf {
    let mut path = database_path.as_os_str().to_os_string();
    path.push(suffix);
    path.into()
}

fn create_private_directory(path: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    match builder.create(path) {
        Ok(()) => sync_directory(path.parent().unwrap_or_else(|| Path::new("."))),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            let metadata = fs::symlink_metadata(path)?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(invalid_data(format!(
                    "backup path is not a real directory: {}",
                    path.display()
                )));
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if metadata.permissions().mode() & 0o077 != 0 {
                    return Err(invalid_data(format!(
                        "backup directory permissions must be private (0700): {}",
                        path.display()
                    )));
                }
            }
            Ok(())
        }
        Err(error) => Err(error),
    }
}

fn create_private_empty_file(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.create_new(true).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

fn redacted_storage_error(_: rusqlite::Error) -> io::Error {
    io::Error::other("stored configuration could not be read safely")
}

fn redacted_mcp_error(_: sqlite_registry::McpRegistryPersistenceError) -> io::Error {
    io::Error::other("MCP Registry configuration could not be restored safely")
}

fn invalid_input(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}

fn invalid_data(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mycopilot_core::image_generation::{
        CredentialReference, CredentialSecret, CredentialStore, DevelopmentFileCredentialStore,
        ImageGenerationConfigurationService,
    };
    use mycopilot_core::storage::image_generation_repository::IMAGE_GENERATION_PROFILE_SCHEMA_VERSION;
    use mycopilot_core::storage::models::{
        BrowserDownloadLocationMode, BrowserDownloadSettingsUpdate, BrowserLinkOpenTarget,
        ChatConversationRecord, ChatMessageRecord, ModelConfigRecord, ModelSettingsRecord,
        ProjectRecord, BROWSER_DOWNLOAD_SCHEMA_VERSION,
    };
    use mycopilot_core::storage::notification_repository::{
        NotificationSettingsUpdate, NotificationSettingsUpdateOutcome,
    };
    use mycopilot_core::ProviderProtocolDialect;
    use mycopilot_mcp_client::{
        McpApprovalMode, McpServerConfig, McpServerId, McpServerScope, McpStdioConfig,
        McpTransportConfig,
    };
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    static TEST_SEQUENCE: AtomicUsize = AtomicUsize::new(0);

    fn options(root: &Path, confirm_reset: bool) -> ResetOptions {
        ResetOptions {
            app_data_root: root.to_path_buf(),
            confirm_reset,
        }
    }

    fn open_test_storage(database: &Path) -> StorageService {
        StorageService::open_with_model_credentials(
            database,
            development_model_credential_store(database).unwrap(),
        )
        .unwrap()
    }

    fn model_settings_with_secret(secret: &str) -> ModelSettingsRecord {
        ModelSettingsRecord {
            api_url: "https://api.example.test/v1/chat/completions".to_string(),
            api_token: secret.to_string(),
            search_mode: "tavily".to_string(),
            tavily_api_key: format!("search-{secret}"),
            models: vec![ModelConfigRecord {
                id: "model-a".to_string(),
                provider_model_id: "model-a".to_string(),
                display_name: "Model A".to_string(),
                api_url_override: Some(
                    "https://per-model.example.test/v1/chat/completions".to_string(),
                ),
                api_token_override: Some(format!("model-{secret}")),
                supports_image: false,
                context_window_tokens: Some(32_000),
                provider_profile_config: ProviderProfileConfig::generic_for_dialect(
                    ProviderProtocolDialect::OpenAiChatCompletions,
                ),
                input_price: "0".to_string(),
                cached_input_price: "".to_string(),
                output_price: "0".to_string(),
                enabled: true,
            }],
        }
    }

    fn populated_storage(root: &Path, secret: &str) {
        let storage = open_test_storage(&root.join(DATABASE_FILE_NAME));
        storage
            .save_model_settings(model_settings_with_secret(secret))
            .unwrap();
        storage
            .set_skill_enablement_override("skill-a", false)
            .unwrap();
        let mut ui_preferences = storage.load_ui_preferences().unwrap();
        ui_preferences.profile_display_name = "Reset Test".to_string();
        ui_preferences.profile_handle = "RESET_CURRENT".to_string();
        ui_preferences.translucent_sidebar = true;
        ui_preferences.translucent_sidebar_transparency = 73;
        storage.save_ui_preferences(ui_preferences).unwrap();
        let mut prompt_preferences = storage.load_agent_prompt_preferences().unwrap();
        prompt_preferences.custom_instructions = "Preserve this preference".to_string();
        storage
            .save_agent_prompt_preferences(prompt_preferences)
            .unwrap();
        let image_profile = ImageGenerationProfileRecord {
            id: DEFAULT_IMAGE_GENERATION_PROFILE_ID.to_string(),
            schema_version: IMAGE_GENERATION_PROFILE_SCHEMA_VERSION,
            adapter_id: "smartmlSeedream".to_string(),
            endpoint_url: "https://image.example.test/v1".to_string(),
            model_id: "image-model".to_string(),
            credential_ref: Some("opaque-credential-reference".to_string()),
            enabled: true,
            text_to_image: true,
            image_to_image: true,
            default_size_preset: "2K".to_string(),
            default_watermark: false,
            generation: 0,
            created_at: 0,
            updated_at: 0,
        };
        assert!(matches!(
            storage
                .compare_and_set_image_generation_profile(
                    DEFAULT_IMAGE_GENERATION_PROFILE_ID,
                    0,
                    &image_profile,
                )
                .unwrap(),
            image_generation_repository::ImageGenerationProfileCompareAndSetOutcome::Updated(_)
        ));
        storage
            .save_project(ProjectRecord {
                id: "project-a".to_string(),
                name: "Disposable".to_string(),
                path: Some(root.join("workspace").display().to_string()),
                created_at: 1,
                pinned_at: None,
            })
            .unwrap();
        storage
            .save_conversation(ChatConversationRecord {
                id: "disposable-conversation".to_string(),
                project_id: Some("project-a".to_string()),
                model_id: Some("model-a".to_string()),
                title: "Disposable conversation".to_string(),
                messages: vec![
                    ChatMessageRecord {
                        id: "disposable-user-message".to_string(),
                        role: "user".to_string(),
                        content: "discard this conversation".to_string(),
                        created_at: 2,
                        status: Some("sent".to_string()),
                        attachments: Vec::new(),
                        agent_run_json: None,
                        ui_state_json: None,
                    },
                    ChatMessageRecord {
                        id: "disposable-assistant-message".to_string(),
                        role: "assistant".to_string(),
                        content: "discard this runtime".to_string(),
                        created_at: 3,
                        status: Some("pending".to_string()),
                        attachments: Vec::new(),
                        agent_run_json: None,
                        ui_state_json: None,
                    },
                ],
                created_at: 2,
                updated_at: 3,
                pinned_at: None,
                archived_at: None,
                unread_at: None,
            })
            .unwrap();
        let trace = mycopilot_core::ConversationTraceSnapshot::default().in_progress_trace(
            "disposable-run",
            "disposable-conversation",
            "disposable-assistant-message",
        );
        storage
            .append_in_progress_conversation_turn_trace(&trace, 3, 3)
            .unwrap();
        let download_directory = root.join("Downloads");
        fs::create_dir_all(&download_directory).unwrap();
        storage
            .save_browser_download_settings(BrowserDownloadSettingsUpdate {
                schema_version: BROWSER_DOWNLOAD_SCHEMA_VERSION,
                location_mode: BrowserDownloadLocationMode::Custom,
                custom_directory: Some(download_directory.display().to_string()),
                ask_where_to_save: true,
                expected_revision: 0,
                updated_at: 7,
            })
            .unwrap();
        let browser_preferences = storage.load_browser_preferences().unwrap();
        storage
            .save_browser_preferences(BrowserPreferencesUpdate {
                schema_version: BROWSER_DATA_SCHEMA_VERSION,
                link_open_target: BrowserLinkOpenTarget::Builtin,
                expected_revision: browser_preferences.revision,
                updated_at: 8,
            })
            .unwrap();
        let notification_settings = storage.load_notification_settings().unwrap();
        assert!(matches!(
            storage
                .update_notification_settings(&NotificationSettingsUpdate {
                    enabled: false,
                    sound_enabled: false,
                    show_task_content: false,
                    human_completed_enabled: false,
                    human_failed_enabled: true,
                    human_approval_enabled: false,
                    human_cancelled_enabled: true,
                    expected_revision: notification_settings.revision,
                    updated_at: 9,
                })
                .unwrap(),
            NotificationSettingsUpdateOutcome::Updated(_)
        ));
        drop(storage);

        let connection = Connection::open(root.join(DATABASE_FILE_NAME)).unwrap();
        connection
            .execute(
                "INSERT INTO mcp_builtin_capability_metadata (
                     singleton, schema_version, revision_watermark
                 ) VALUES (1, 1, 1)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO mcp_builtin_capability_policies (
                     schema_version, capability_id, user_allowed,
                     policy_version, policy_revision
                 ) VALUES (1, 'browser_automation', 1, 1, 1)",
                [],
            )
            .unwrap();
        drop(connection);

        let registry = SqliteMcpRegistry::open(root.join(DATABASE_FILE_NAME)).unwrap();
        registry
            .add(McpServerConfig {
                id: McpServerId::new(),
                display_name: "Reset Test MCP".to_string(),
                scope: McpServerScope::User,
                trust: McpTrustLevel::Untrusted,
                approval_mode: McpApprovalMode::Prompt,
                enabled: false,
                transport: McpTransportConfig::Stdio(McpStdioConfig {
                    program: std::env::current_exe().unwrap(),
                    arguments: vec!["--reset-test".to_string()],
                    cwd: root.to_path_buf(),
                    environment: Vec::new(),
                }),
                connect_timeout_ms: 1_000,
                request_timeout_ms: 60_000,
                shutdown_timeout_ms: 2_000,
            })
            .unwrap();
    }

    #[test]
    fn parser_requires_an_explicit_absolute_root() {
        let missing = parse_options(Vec::<OsString>::new()).unwrap_err();
        assert_eq!(missing.kind(), io::ErrorKind::InvalidInput);

        let parsed = parse_options([
            OsString::from(APP_DATA_ROOT_FLAG),
            OsString::from("/absolute/root"),
            OsString::from(CONFIRM_RESET_FLAG),
        ])
        .unwrap();
        assert!(parsed.confirm_reset);
        assert!(parsed.app_data_root.is_absolute());
    }

    #[test]
    fn configuration_preservation_is_limited_to_the_current_schema() {
        assert_eq!(
            mycopilot_core::storage::migrations::STORAGE_SCHEMA_VERSION,
            33
        );
        assert!(can_preserve_development_configuration(33));
        assert!(!can_preserve_development_configuration(32));
        assert!(!can_preserve_development_configuration(31));
        assert!(!can_preserve_development_configuration(30));
        assert!(!can_preserve_development_configuration(34));
    }

    #[test]
    fn confirmed_reset_initializes_an_absent_database_without_a_backup() {
        let fixture = tempfile::tempdir().unwrap();

        let report = execute(options(fixture.path(), true)).unwrap();

        assert!(report.confirmed);
        assert!(!report.source_existed);
        assert_eq!(report.backup_path, None);
        assert!(report.database_path.is_file());
        assert!(!fixture.path().join(BACKUP_DIRECTORY_NAME).exists());
        open_test_storage(&report.database_path);
        let connection = open_read_only(&report.database_path).unwrap();
        assert!(pragma_rows(&connection, "PRAGMA foreign_key_check")
            .unwrap()
            .is_empty());
        ensure_only_configuration_tables_have_rows(&connection).unwrap();
    }

    #[test]
    fn reset_preserves_current_non_default_image_profiles_exactly() {
        let fixture = tempfile::tempdir().unwrap();
        let storage = open_test_storage(&fixture.path().join(DATABASE_FILE_NAME));
        let extra_profile = ImageGenerationProfileRecord {
            id: "future-profile".to_string(),
            schema_version: IMAGE_GENERATION_PROFILE_SCHEMA_VERSION,
            adapter_id: "smartmlSeedream".to_string(),
            endpoint_url: "https://image.example.test/v1".to_string(),
            model_id: "image-model".to_string(),
            credential_ref: None,
            enabled: true,
            text_to_image: true,
            image_to_image: false,
            default_size_preset: "2K".to_string(),
            default_watermark: true,
            generation: 0,
            created_at: 0,
            updated_at: 0,
        };
        assert!(matches!(
            storage
                .compare_and_set_image_generation_profile("future-profile", 0, &extra_profile,)
                .unwrap(),
            image_generation_repository::ImageGenerationProfileCompareAndSetOutcome::Updated(_)
        ));
        let expected = storage
            .load_image_generation_profile("future-profile")
            .unwrap()
            .unwrap();
        drop(storage);

        let report = execute(options(fixture.path(), true)).unwrap();

        assert_eq!(report.image_generation_profile_count, 1);
        let storage = open_test_storage(&fixture.path().join(DATABASE_FILE_NAME));
        assert_eq!(
            storage
                .load_image_generation_profile("future-profile")
                .unwrap()
                .as_ref(),
            Some(&expected)
        );
    }

    #[test]
    fn dry_run_is_read_only_and_report_is_redacted() {
        let fixture = tempfile::tempdir().unwrap();
        let secret = format!(
            "never-print-this-{}",
            TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        );
        populated_storage(fixture.path(), &secret);
        let database = fixture.path().join(DATABASE_FILE_NAME);
        let before = fs::read(&database).unwrap();

        let report = execute(options(fixture.path(), false)).unwrap();

        assert!(!report.confirmed);
        assert_eq!(report.model_count, 1);
        assert_eq!(report.mcp_server_count, 1);
        assert_eq!(fs::read(database).unwrap(), before);
        assert!(!fixture.path().join(BACKUP_DIRECTORY_NAME).exists());
        assert!(!report.render().contains(&secret));
    }

    #[test]
    fn current_schema_reset_rejects_an_extra_schema_object() {
        let fixture = tempfile::tempdir().unwrap();
        populated_storage(fixture.path(), "current-schema-tamper-secret");
        let database = fixture.path().join(DATABASE_FILE_NAME);
        let connection = Connection::open(&database).unwrap();
        connection
            .execute_batch("CREATE TABLE injected_reset_table(value TEXT NOT NULL);")
            .unwrap();
        drop(connection);

        let error = execute(options(fixture.path(), false)).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("exactly canonical"));
        assert!(!fixture.path().join(BACKUP_DIRECTORY_NAME).exists());
    }

    #[test]
    fn current_schema_reset_rejects_foreign_key_corruption() {
        let fixture = tempfile::tempdir().unwrap();
        populated_storage(fixture.path(), "current-foreign-key-secret");
        let database = fixture.path().join(DATABASE_FILE_NAME);
        let connection = Connection::open(&database).unwrap();
        connection
            .pragma_update(None, "foreign_keys", false)
            .unwrap();
        connection
            .execute(
                "INSERT INTO attachments (
                     id, conversation_id, message_id, project_id, kind,
                     original_name, mime_type, size_bytes, storage_rel_path, created_at
                 ) VALUES (
                     'orphan-attachment', 'missing-conversation', 'missing-message', NULL,
                     'file', 'orphan.txt', 'text/plain', 1, 'orphan.txt', 0
                 )",
                [],
            )
            .unwrap();
        drop(connection);

        let error = execute(options(fixture.path(), false)).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("foreign_key_check"));
        assert!(!fixture.path().join(BACKUP_DIRECTORY_NAME).exists());
    }

    #[test]
    fn reset_rejects_a_non_current_per_model_protocol_revision() {
        let fixture = tempfile::tempdir().unwrap();
        populated_storage(fixture.path(), "reset-invalid-revision-token");
        let database = fixture.path().join(DATABASE_FILE_NAME);
        let connection = Connection::open(&database).unwrap();
        connection
            .execute(
                "UPDATE models SET provider_protocol_revision = 'provider-protocol-v1:not-a-uuid'",
                [],
            )
            .unwrap();
        drop(connection);

        let error = execute(options(fixture.path(), false)).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error
            .to_string()
            .contains("current Provider Protocol revision"));
        assert!(!fixture.path().join(BACKUP_DIRECTORY_NAME).exists());
    }

    #[test]
    fn reset_fails_closed_when_a_configuration_table_has_unknown_columns() {
        let fixture = tempfile::tempdir().unwrap();
        populated_storage(fixture.path(), "reset-unknown-column-token");
        let database = fixture.path().join(DATABASE_FILE_NAME);
        let connection = Connection::open(&database).unwrap();
        connection
            .execute(
                "ALTER TABLE models ADD COLUMN future_provider_setting TEXT",
                [],
            )
            .unwrap();
        drop(connection);
        let source_digest = file_digest(&database).unwrap();

        let error = execute(options(fixture.path(), false)).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("exactly canonical"));
        assert_eq!(file_digest(&database).unwrap(), source_digest);
        assert!(!fixture.path().join(BACKUP_DIRECTORY_NAME).exists());
    }

    #[test]
    fn confirmed_reset_failure_preserves_current_source_and_recovery_backup() {
        let fixture = tempfile::tempdir().unwrap();
        let secret = "confirmed-reset-failure-secret";
        populated_storage(fixture.path(), secret);
        let database = fixture.path().join(DATABASE_FILE_NAME);
        let connection = Connection::open(&database).unwrap();
        connection
            .execute(
                "UPDATE models SET provider_protocol_revision = 'provider-protocol-v1:not-a-uuid'",
                [],
            )
            .unwrap();
        drop(connection);
        let source_digest = file_digest(&database).unwrap();

        let error = execute(options(fixture.path(), true)).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error
            .to_string()
            .contains("current Provider Protocol revision"));
        assert!(!error.to_string().contains(secret));
        assert_eq!(file_digest(&database).unwrap(), source_digest);

        let backup_directory = fixture.path().join(BACKUP_DIRECTORY_NAME);
        let backups = fs::read_dir(&backup_directory)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.file_name()
                    .and_then(OsStr::to_str)
                    .is_some_and(|name| {
                        name.starts_with("storage-reset-dev-") && name.ends_with(".sqlite")
                    })
            })
            .collect::<Vec<_>>();
        assert_eq!(
            backups.len(),
            1,
            "one immutable recovery snapshot must remain"
        );
        // A canonical database uses WAL mode. Exercise the actual recovery shape by restoring
        // the immutable snapshot to a writable database identity instead of mutating the only
        // backup merely to inspect it.
        let recovered_database = fixture.path().join("recovered-current.sqlite");
        fs::copy(&backups[0], &recovered_database).unwrap();
        let backup = Connection::open(&recovered_database).unwrap();
        mycopilot_core::storage::migrations::run_migrations(&backup).unwrap();
        assert_eq!(pragma_rows(&backup, "PRAGMA quick_check").unwrap(), ["ok"]);
        assert_eq!(
            backup
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            i64::from(mycopilot_core::storage::migrations::STORAGE_SCHEMA_VERSION)
        );
        let credential_ref = backup
            .query_row(
                "SELECT api_token_ref FROM model_provider_settings WHERE id = 'default'",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap();
        assert!(CredentialReference::parse(&credential_ref).is_ok());
        assert!(!fs::read(&recovered_database)
            .unwrap()
            .windows(secret.len())
            .any(|window| window == secret.as_bytes()));
        drop(backup);
        remove_database_files(&recovered_database);

        let temporary_files = fs::read_dir(fixture.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with(".storage.sqlite.reset-dev-"))
            .collect::<Vec<_>>();
        assert!(
            temporary_files.is_empty(),
            "failed reset leaked temporary database files: {temporary_files:?}"
        );
    }

    #[test]
    fn confirmed_reset_backs_up_and_restores_only_configuration() {
        let fixture = tempfile::tempdir().unwrap();
        let secret = "reset-test-api-token";
        populated_storage(fixture.path(), secret);
        let source = Connection::open(fixture.path().join(DATABASE_FILE_NAME)).unwrap();
        let exact_configuration_before = snapshot_exact_configuration_tables(&source).unwrap();
        drop(source);

        let report = execute(options(fixture.path(), true)).unwrap();

        assert!(report.confirmed);
        let backup = report.backup_path.as_ref().unwrap();
        assert!(backup.is_file());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(backup).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        let storage = open_test_storage(&fixture.path().join(DATABASE_FILE_NAME));
        let snapshot = storage.load_model_settings_snapshot().unwrap().unwrap();
        assert_eq!(snapshot.settings.api_token, secret);
        assert_eq!(snapshot.settings.search_mode, "tavily");
        assert_eq!(snapshot.settings.tavily_api_key, format!("search-{secret}"));
        assert_eq!(
            snapshot.settings.models[0].api_url_override.as_deref(),
            Some("https://per-model.example.test/v1/chat/completions")
        );
        assert_eq!(
            snapshot.settings.models[0].api_token_override.as_deref(),
            Some(format!("model-{secret}").as_str())
        );
        assert_eq!(
            snapshot.settings.models[0]
                .provider_profile_config
                .profile()
                .id,
            mycopilot_core::ProviderProfileId::GenericOpenAiChat
        );
        assert!(
            snapshot.provider_protocol_revisions["model-a"].starts_with("provider-protocol-v1:")
        );
        assert!(
            !storage
                .load_skill_enablement(&["skill-a".to_string()])
                .unwrap()["skill-a"]
        );
        assert_eq!(
            storage.load_ui_preferences().unwrap().profile_display_name,
            "Reset Test"
        );
        assert_eq!(
            storage
                .load_agent_prompt_preferences()
                .unwrap()
                .custom_instructions,
            "Preserve this preference"
        );
        let image_profile = storage
            .load_image_generation_profile(DEFAULT_IMAGE_GENERATION_PROFILE_ID)
            .unwrap()
            .unwrap();
        assert_eq!(
            image_profile.credential_ref.as_deref(),
            Some("opaque-credential-reference")
        );
        drop(storage);
        let registry = SqliteMcpRegistry::open(fixture.path().join(DATABASE_FILE_NAME)).unwrap();
        let mcp_servers = registry.list().unwrap();
        assert_eq!(mcp_servers.len(), 1);
        assert_eq!(mcp_servers[0].config.display_name, "Reset Test MCP");
        drop(registry);
        let connection = open_read_only(&report.database_path).unwrap();
        assert_eq!(
            snapshot_exact_configuration_tables(&connection).unwrap(),
            exact_configuration_before
        );
        assert_eq!(
            count_rows_if_table_exists(&connection, "projects").unwrap(),
            0
        );
        assert!(pragma_rows(&connection, "PRAGMA foreign_key_check")
            .unwrap()
            .is_empty());
        assert!(!report.render().contains(secret));
    }

    #[test]
    fn confirmed_reset_preserves_a_current_image_credential_through_reconciliation() {
        let fixture = tempfile::tempdir().unwrap();
        let database = fixture.path().join(DATABASE_FILE_NAME);
        let storage = open_test_storage(&database);
        let credentials = Arc::new(
            DevelopmentFileCredentialStore::new(
                fixture
                    .path()
                    .join("image-generation-development-credentials-v1"),
            )
            .unwrap(),
        );
        let reference = credentials.new_reference();
        credentials
            .replace(
                &reference,
                CredentialSecret::new("temporary-reset-test-secret").unwrap(),
            )
            .unwrap();
        let profile = ImageGenerationProfileRecord {
            id: DEFAULT_IMAGE_GENERATION_PROFILE_ID.to_string(),
            schema_version: IMAGE_GENERATION_PROFILE_SCHEMA_VERSION,
            adapter_id: "smartmlSeedream".to_string(),
            endpoint_url: "https://image.example.test/v1".to_string(),
            model_id: "image-model".to_string(),
            credential_ref: Some(reference.as_str().to_string()),
            enabled: true,
            text_to_image: true,
            image_to_image: false,
            default_size_preset: "2K".to_string(),
            default_watermark: false,
            generation: 0,
            created_at: 0,
            updated_at: 0,
        };
        assert!(matches!(
            storage
                .compare_and_set_image_generation_profile(
                    DEFAULT_IMAGE_GENERATION_PROFILE_ID,
                    0,
                    &profile,
                )
                .unwrap(),
            image_generation_repository::ImageGenerationProfileCompareAndSetOutcome::Updated(_)
        ));
        drop(storage);
        assert!(credentials.get(&reference).unwrap().is_some());

        execute(options(fixture.path(), true)).unwrap();

        let storage = Arc::new(open_test_storage(&database));
        let restored = storage
            .load_image_generation_profile(DEFAULT_IMAGE_GENERATION_PROFILE_ID)
            .unwrap()
            .unwrap();
        assert_eq!(restored.credential_ref.as_deref(), Some(reference.as_str()));
        let configuration = ImageGenerationConfigurationService::new(
            Arc::clone(&storage),
            Arc::clone(&credentials) as Arc<dyn CredentialStore>,
        );
        configuration.reconcile_credentials().unwrap();
        assert!(credentials.get(&reference).unwrap().is_some());
    }

    #[test]
    fn confirmed_reset_rebuilds_v30_with_defaults_instead_of_decoding_configuration() {
        let fixture = tempfile::tempdir().unwrap();
        populated_storage(fixture.path(), "v30-must-not-be-decoded-token");
        let database = fixture.path().join(DATABASE_FILE_NAME);
        let connection = Connection::open(&database).unwrap();
        connection.pragma_update(None, "user_version", 30).unwrap();
        drop(connection);

        let report = execute(options(fixture.path(), true)).unwrap();

        assert!(report.confirmed);
        assert!(report
            .backup_path
            .as_ref()
            .is_some_and(|path| path.is_file()));
        assert!(!report.preserved_configuration);
        assert!(report
            .render()
            .contains("configuration preservation: skipped (unsupported schema; defaults used)"));
        assert_eq!(report.model_count, 0);
        let storage = open_test_storage(&database);
        assert!(storage.load_model_settings_snapshot().unwrap().is_none());
        assert!(storage.load_projects().unwrap().is_empty());
        assert!(storage.load_conversations().unwrap().is_empty());
    }

    #[test]
    fn reset_preserves_the_exact_mcp_model_namespace_after_a_display_name_change() {
        let fixture = tempfile::tempdir().unwrap();
        let database = fixture.path().join(DATABASE_FILE_NAME);
        open_test_storage(&database);
        let registry = SqliteMcpRegistry::open(&database).unwrap();
        let server_id = McpServerId::new();
        let original = registry
            .add_persisted(
                McpServerConfig {
                    id: server_id,
                    display_name: "Original Namespace Label".to_string(),
                    scope: McpServerScope::User,
                    trust: McpTrustLevel::Untrusted,
                    approval_mode: McpApprovalMode::Prompt,
                    enabled: false,
                    transport: McpTransportConfig::Stdio(McpStdioConfig {
                        program: std::env::current_exe().unwrap(),
                        arguments: vec!["--namespace-reset-test".to_string()],
                        cwd: fixture.path().to_path_buf(),
                        environment: Vec::new(),
                    }),
                    connect_timeout_ms: 1_000,
                    request_timeout_ms: 60_000,
                    shutdown_timeout_ms: 2_000,
                },
                None,
            )
            .unwrap();
        let expected_namespace = original.model_namespace.clone();
        let mut renamed = original.config.clone();
        renamed.display_name = "Completely Different Label".to_string();
        registry
            .update_with_precondition(
                &McpRegistryMutationPrecondition::from_entry(&original),
                renamed,
            )
            .unwrap();
        assert_eq!(
            registry.get(server_id).unwrap().unwrap().model_namespace,
            expected_namespace
        );
        drop(registry);

        execute(options(fixture.path(), true)).unwrap();

        let restored = SqliteMcpRegistry::open(&database)
            .unwrap()
            .get(server_id)
            .unwrap()
            .unwrap();
        assert_eq!(restored.model_namespace, expected_namespace);
        assert_eq!(restored.config.display_name, "Completely Different Label");
    }

    #[test]
    fn mcp_inspection_never_mutates_the_only_recovery_backup() {
        let fixture = tempfile::tempdir().unwrap();
        populated_storage(fixture.path(), "backup-secret");
        let database = fixture.path().join(DATABASE_FILE_NAME);
        let backup = create_database_backup(fixture.path(), &database).unwrap();
        let before = file_digest(&backup).unwrap();
        let working =
            TemporaryDatabaseSnapshot::from_source(fixture.path(), &backup, "mcp-test").unwrap();

        let records = load_mcp_records(working.path()).unwrap();

        assert_eq!(records.len(), 1);
        assert_eq!(file_digest(&backup).unwrap(), before);
    }

    #[test]
    fn reset_refuses_a_live_database_owner() {
        let fixture = tempfile::tempdir().unwrap();
        populated_storage(fixture.path(), "secret");
        let database = fixture.path().join(DATABASE_FILE_NAME);
        let _lock = acquire_database_instance_lock(&database).unwrap();

        let error = execute(options(fixture.path(), true)).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert!(!fixture.path().join(BACKUP_DIRECTORY_NAME).exists());
    }

    #[test]
    fn reset_does_not_touch_another_storage_root() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        populated_storage(first.path(), "first-secret");
        populated_storage(second.path(), "second-secret");
        let second_before = fs::read(second.path().join(DATABASE_FILE_NAME)).unwrap();

        execute(options(first.path(), true)).unwrap();

        assert_eq!(
            fs::read(second.path().join(DATABASE_FILE_NAME)).unwrap(),
            second_before
        );
        assert!(!second.path().join(BACKUP_DIRECTORY_NAME).exists());
    }

    #[test]
    fn reset_does_not_run_startup_cleanup_in_sibling_managed_directories() {
        let fixture = tempfile::tempdir().unwrap();
        populated_storage(fixture.path(), "filesystem-boundary-secret");
        let sentinels = [
            fixture.path().join("attachments/orphan/sentinel.bin"),
            fixture
                .path()
                .join("image-generation-artifacts/objects/sentinel.bin"),
            fixture.path().join("managed-command-runs/v1/sentinel.bin"),
            fixture.path().join("skills/sentinel/SKILL.md"),
            fixture.path().join("credentials/sentinel.bin"),
        ];
        for (index, path) in sentinels.iter().enumerate() {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, format!("managed-directory-sentinel-{index}")).unwrap();
        }
        let before = sentinels
            .iter()
            .map(|path| fs::read(path).unwrap())
            .collect::<Vec<_>>();

        execute(options(fixture.path(), true)).unwrap();

        for (path, expected) in sentinels.iter().zip(before) {
            assert_eq!(fs::read(path).unwrap(), expected);
        }
    }
}
