//! Explicit development-only reset for the Electron-owned SQLite database.
//!
//! This binary is not part of the packaged application. It defaults to a non-destructive preflight
//! and requires `--confirm-reset` before it snapshots and atomically replaces the exact database.

#[allow(dead_code, unused_imports)] // The shared MCP Registry module exposes more than reset uses.
#[path = "../application/mcp/sqlite_registry.rs"]
mod sqlite_registry;

use mycopilot_core::durable_fs::{atomic_replace, sync_directory};
use mycopilot_core::storage::agent_prompt_preferences_repository;
use mycopilot_core::storage::config_repository::is_provider_protocol_revision;
use mycopilot_core::storage::image_generation_repository::{
    self, DEFAULT_IMAGE_GENERATION_PROFILE_ID,
};
use mycopilot_core::storage::models::{
    AgentPromptPreferencesRecord, ImageGenerationProfileRecord, ModelConfigRecord,
    ModelSettingsRecord, UiPreferencesRecord,
};
use mycopilot_core::storage::preferences_repository;
use mycopilot_core::storage::service::StorageService;
use mycopilot_core::storage::{acquire_database_instance_lock, create_verified_sqlite_snapshot};
use mycopilot_core::{ProviderProfileConfig, ProviderProtocolDialect};
use mycopilot_mcp_client::{McpRegistry, McpTrustLevel};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use sha2::{Digest, Sha256};
use sqlite_registry::{
    launch_authorization_is_valid, McpPersistedRegistryRecord, McpRegistryMutationPrecondition,
    SqliteMcpRegistry,
};
use std::ffi::{OsStr, OsString};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const DATABASE_FILE_NAME: &str = "storage.sqlite";
const BACKUP_DIRECTORY_NAME: &str = "storage-backups";
const CONFIRM_RESET_FLAG: &str = "--confirm-reset";
const APP_DATA_ROOT_FLAG: &str = "--app-data-root";
const SQLITE_TRANSIENT_SUFFIXES: [&str; 3] = ["-journal", "-wal", "-shm"];
const PRESERVED_CONFIGURATION_TABLES: &[&str] = &[
    "model_provider_settings",
    "models",
    "ui_preferences",
    "agent_prompt_preferences",
    "skill_enablement_overrides",
    "image_generation_profiles",
    "mcp_registry_metadata",
    "mcp_registry_servers",
    "mcp_registry_model_namespaces",
];

#[derive(Debug, Clone, PartialEq, Eq)]
struct ResetOptions {
    app_data_root: PathBuf,
    confirm_reset: bool,
}

#[derive(Debug)]
struct PreservedConfiguration {
    model_settings: Option<ModelSettingsRecord>,
    ui_preferences: UiPreferencesRecord,
    agent_prompt_preferences: AgentPromptPreferencesRecord,
    skill_enablement_overrides: Vec<(String, bool)>,
    image_generation_profile: Option<ImageGenerationProfileRecord>,
    mcp_records: Vec<McpPersistedRegistryRecord>,
    mcp_server_count: usize,
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
        format!(
            "Development storage reset {mode}\n\
             database: {}\n\
             source database: {source}\n\
             backup: {backup}\n\
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
                "close MyCopilot before resetting `{}`: {error}",
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
    let model_settings = load_model_settings_for_development_reset(&connection)?;
    let model_settings = model_settings.map(validate_model_profiles).transpose()?;
    let ui_preferences =
        preferences_repository::load_ui_preferences(&connection).map_err(redacted_storage_error)?;
    let agent_prompt_preferences =
        agent_prompt_preferences_repository::load_agent_prompt_preferences(&connection)
            .map_err(redacted_storage_error)?;
    let skill_enablement_overrides = load_skill_enablement_overrides(&connection)?;
    let image_generation_profile = image_generation_repository::load_image_generation_profile(
        &connection,
        DEFAULT_IMAGE_GENERATION_PROFILE_ID,
    )
    .map_err(redacted_storage_error)?;
    let image_profile_count = count_rows_if_table_exists(&connection, "image_generation_profiles")?;
    if image_profile_count > u64::from(image_generation_profile.is_some()) {
        return Err(invalid_data(
            "unsupported non-default image-generation profiles exist; remove them explicitly before resetting",
        ));
    }
    ensure_no_image_credential_reconciliation(&connection)?;
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
        model_settings,
        ui_preferences,
        agent_prompt_preferences,
        skill_enablement_overrides,
        image_generation_profile,
        mcp_records,
        mcp_server_count: mcp_count,
    };
    Ok((Some(configuration), discarded_conversation_rows))
}

/// Reads only the configuration fields that the explicit development reset preserves.
///
/// The source database must already use the current per-model Provider Protocol identity. The
/// reset command rebuilds storage; it is not an importer for retired development formats.
fn load_model_settings_for_development_reset(
    connection: &Connection,
) -> io::Result<Option<ModelSettingsRecord>> {
    let provider_settings = connection
        .query_row(
            "SELECT api_url, api_token, search_mode, tavily_api_key
             FROM model_provider_settings
             WHERE id = 'default'",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .optional()
        .map_err(redacted_storage_error)?;
    let Some((api_url, api_token, search_mode, tavily_api_key)) = provider_settings else {
        if count_rows_if_table_exists(connection, "models")? != 0 {
            return Err(invalid_data(
                "models exist without the default provider settings record",
            ));
        }
        return Ok(None);
    };

    let mut statement = connection
        .prepare(
            "SELECT
                 id,
                 display_name,
                 api_url_override,
                 api_token_override,
                 supports_image,
                 context_window_tokens,
                 provider_profile_config_json,
                 input_price,
                 cached_input_price,
                 output_price,
                 enabled,
                 provider_protocol_revision
             FROM models
             ORDER BY position ASC, created_at ASC",
        )
        .map_err(redacted_storage_error)?;
    let raw_models = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, bool>(4)?,
                row.get::<_, Option<u32>>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, String>(8)?,
                row.get::<_, String>(9)?,
                row.get::<_, bool>(10)?,
                row.get::<_, String>(11)?,
            ))
        })
        .map_err(redacted_storage_error)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(redacted_storage_error)?;

    let mut models = Vec::with_capacity(raw_models.len());
    for (
        id,
        display_name,
        api_url_override,
        api_token_override,
        supports_image,
        context_window_tokens,
        profile_json,
        input_price,
        cached_input_price,
        output_price,
        enabled,
        provider_protocol_revision,
    ) in raw_models
    {
        if !is_provider_protocol_revision(&provider_protocol_revision) {
            return Err(invalid_data(format!(
                "model `{id}` does not use the current Provider Protocol revision"
            )));
        }
        let provider_profile_config = serde_json::from_str::<ProviderProfileConfig>(&profile_json)
            .map_err(|_| {
                invalid_data(format!(
                    "model `{id}` has an unreadable Provider Profile configuration"
                ))
            })?;
        models.push(ModelConfigRecord {
            id,
            display_name,
            api_url_override,
            api_token_override,
            supports_image,
            context_window_tokens,
            provider_profile_config,
            input_price,
            cached_input_price,
            output_price,
            enabled,
        });
    }

    Ok(Some(ModelSettingsRecord {
        api_url,
        api_token,
        search_mode,
        tavily_api_key,
        models,
    }))
}

fn validate_model_profiles(settings: ModelSettingsRecord) -> io::Result<ModelSettingsRecord> {
    for model in &settings.models {
        let api_url = model
            .api_url_override
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(&settings.api_url);
        let dialect = ProviderProtocolDialect::detect_from_api_url(api_url);
        model.provider_profile_config.validate().map_err(|_| {
            invalid_data(format!(
                "model `{}` has an invalid Provider Profile; select a supported profile before resetting",
                model.id
            ))
        })?;
        model
            .provider_profile_config
            .validate_for_dialect(dialect)
            .map_err(|_| {
                invalid_data(format!(
                    "model `{}` has a Provider Profile incompatible with its API dialect",
                    model.id
                ))
            })?;
    }
    Ok(settings)
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
            "image-generation credential reconciliation is pending; start and cleanly close MyCopilot before resetting",
        ));
    }
    Ok(())
}

fn load_mcp_records(database_path: &Path) -> io::Result<Vec<McpPersistedRegistryRecord>> {
    let registry = SqliteMcpRegistry::open(database_path).map_err(redacted_mcp_error)?;
    let (_, records) = registry.snapshot().map_err(redacted_mcp_error)?;
    Ok(records)
}

fn build_fresh_database(
    database_path: &Path,
    configuration: Option<&PreservedConfiguration>,
) -> io::Result<()> {
    create_private_empty_file(database_path)?;
    let storage = StorageService::open_for_development_reset(database_path)
        .map_err(|_| io::Error::other("failed to create the canonical storage schema"))?;
    if let Some(configuration) = configuration {
        if let Some(settings) = &configuration.model_settings {
            storage
                .save_model_settings(settings.clone())
                .map_err(|_| io::Error::other("failed to restore normalized model settings"))?;
        }
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
        if let Some(profile) = &configuration.image_generation_profile {
            match storage
                .compare_and_set_image_generation_profile(
                    DEFAULT_IMAGE_GENERATION_PROFILE_ID,
                    0,
                    profile,
                )
                .map_err(|_| {
                    io::Error::other("failed to restore the image-generation profile")
                })? {
                image_generation_repository::ImageGenerationProfileCompareAndSetOutcome::Updated(
                    _,
                ) => {}
                image_generation_repository::ImageGenerationProfileCompareAndSetOutcome::Conflict(
                    _,
                ) => {
                    return Err(io::Error::other(
                        "fresh image-generation profile unexpectedly conflicted",
                    ));
                }
            }
        }
    }
    drop(storage);

    if let Some(configuration) = configuration {
        restore_mcp_records(database_path, &configuration.mcp_records)?;
    }
    Ok(())
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
    let storage = StorageService::open_for_development_reset(database_path).map_err(|_| {
        io::Error::other("fresh database was rejected by the canonical storage boundary")
    })?;
    if let Some(configuration) = configuration {
        let restored = storage
            .load_model_settings_snapshot()
            .map_err(|_| io::Error::other("restored model settings could not be verified"))?;
        match (&configuration.model_settings, restored) {
            (None, None) => {}
            (Some(expected), Some(restored))
                if expected.models.len() == restored.settings.models.len() =>
            {
                if restored
                    .provider_protocol_revisions
                    .values()
                    .any(|revision| !revision.starts_with("provider-protocol-v1:"))
                {
                    return Err(invalid_data(
                        "restored models do not have explicit Profiles and current protocol revisions",
                    ));
                }
            }
            _ => return Err(invalid_data("restored model settings count mismatch")),
        }
    }
    drop(storage);
    let connection = open_read_only(database_path)?;
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
        let expected_models = configuration
            .model_settings
            .as_ref()
            .map_or(0, |settings| settings.models.len());
        if model_count != expected_models {
            return Err(invalid_data("fresh database model count mismatch"));
        }
        let expected_settings_rows = usize::from(configuration.model_settings.is_some());
        verify_table_count(
            &connection,
            "model_provider_settings",
            expected_settings_rows,
        )?;
        verify_table_count(&connection, "ui_preferences", 1)?;
        verify_table_count(&connection, "agent_prompt_preferences", 1)?;
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
            usize::from(configuration.image_generation_profile.is_some()),
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
        model_count: configuration
            .and_then(|configuration| configuration.model_settings.as_ref())
            .map_or(0, |settings| settings.models.len()),
        skill_override_count: configuration.map_or(0, |configuration| {
            configuration.skill_enablement_overrides.len()
        }),
        mcp_server_count: configuration.map_or(0, |configuration| configuration.mcp_server_count),
        image_generation_profile_count: configuration
            .and_then(|configuration| configuration.image_generation_profile.as_ref())
            .map_or(0, |_| 1),
        discarded_conversation_rows,
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
    use mycopilot_core::storage::image_generation_repository::IMAGE_GENERATION_PROFILE_SCHEMA_VERSION;
    use mycopilot_core::storage::models::{ModelConfigRecord, ProjectRecord};
    use mycopilot_mcp_client::{
        McpApprovalMode, McpServerConfig, McpServerId, McpServerScope, McpStdioConfig,
        McpTransportConfig,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};

    static TEST_SEQUENCE: AtomicUsize = AtomicUsize::new(0);

    fn options(root: &Path, confirm_reset: bool) -> ResetOptions {
        ResetOptions {
            app_data_root: root.to_path_buf(),
            confirm_reset,
        }
    }

    fn model_settings_with_secret(secret: &str) -> ModelSettingsRecord {
        ModelSettingsRecord {
            api_url: "https://api.example.test/v1/chat/completions".to_string(),
            api_token: secret.to_string(),
            search_mode: "tavily".to_string(),
            tavily_api_key: format!("search-{secret}"),
            models: vec![ModelConfigRecord {
                id: "model-a".to_string(),
                display_name: "Model A".to_string(),
                api_url_override: None,
                api_token_override: None,
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
        let storage = StorageService::open(&root.join(DATABASE_FILE_NAME)).unwrap();
        storage
            .save_model_settings(model_settings_with_secret(secret))
            .unwrap();
        storage
            .set_skill_enablement_override("skill-a", false)
            .unwrap();
        let mut ui_preferences = storage.load_ui_preferences().unwrap();
        ui_preferences.profile_display_name = "Reset Test".to_string();
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
        drop(storage);

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
    fn confirmed_reset_initializes_an_absent_database_without_a_backup() {
        let fixture = tempfile::tempdir().unwrap();

        let report = execute(options(fixture.path(), true)).unwrap();

        assert!(report.confirmed);
        assert!(!report.source_existed);
        assert_eq!(report.backup_path, None);
        assert!(report.database_path.is_file());
        assert!(!fixture.path().join(BACKUP_DIRECTORY_NAME).exists());
        StorageService::open(&report.database_path).unwrap();
        let connection = open_read_only(&report.database_path).unwrap();
        assert!(pragma_rows(&connection, "PRAGMA foreign_key_check")
            .unwrap()
            .is_empty());
        ensure_only_configuration_tables_have_rows(&connection).unwrap();
    }

    #[test]
    fn reset_rejects_non_default_image_profiles_instead_of_dropping_them() {
        let fixture = tempfile::tempdir().unwrap();
        let storage = StorageService::open(&fixture.path().join(DATABASE_FILE_NAME)).unwrap();
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
        drop(storage);

        let error = execute(options(fixture.path(), false)).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error
            .to_string()
            .contains("unsupported non-default image-generation profiles"));
        assert!(!fixture.path().join(BACKUP_DIRECTORY_NAME).exists());
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
    fn confirmed_reset_failure_preserves_current_v8_source_and_recovery_backup() {
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
        // A canonical v8 database uses WAL mode. Exercise the actual recovery shape by restoring
        // the immutable snapshot to a writable database identity instead of mutating the only
        // backup merely to inspect it.
        let recovered_database = fixture.path().join("recovered-v8.sqlite");
        fs::copy(&backups[0], &recovered_database).unwrap();
        let backup = Connection::open(&recovered_database).unwrap();
        mycopilot_core::storage::migrations::run_migrations(&backup).unwrap();
        assert_eq!(pragma_rows(&backup, "PRAGMA quick_check").unwrap(), ["ok"]);
        assert_eq!(
            backup
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            8
        );
        assert_eq!(
            backup
                .query_row(
                    "SELECT api_token FROM model_provider_settings WHERE id = 'default'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            secret
        );
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
        let storage = StorageService::open(&fixture.path().join(DATABASE_FILE_NAME)).unwrap();
        let snapshot = storage.load_model_settings_snapshot().unwrap().unwrap();
        assert_eq!(snapshot.settings.api_token, secret);
        assert_eq!(
            snapshot.settings.models[0]
                .provider_profile_config
                .profile
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
            count_rows_if_table_exists(&connection, "projects").unwrap(),
            0
        );
        assert!(pragma_rows(&connection, "PRAGMA foreign_key_check")
            .unwrap()
            .is_empty());
        assert!(!report.render().contains(secret));
    }

    #[test]
    fn reset_preserves_the_exact_mcp_model_namespace_after_a_display_name_change() {
        let fixture = tempfile::tempdir().unwrap();
        let database = fixture.path().join(DATABASE_FILE_NAME);
        StorageService::open(&database).unwrap();
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
