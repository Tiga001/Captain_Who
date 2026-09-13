use crate::storage::models::{
    normalize_model_display_name, StoredModelConfigRecord, StoredModelSettingsRecord,
    StoredModelSettingsSnapshot,
};
use crate::storage::now_ms;
use crate::{ProviderProfileConfig, ProviderProtocolDialect};
use rusqlite::types::Type;
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

const MODEL_SETTINGS_REVISION_PREFIX: &str = "model-settings-v1:";
const PROVIDER_CONNECTION_REVISION_PREFIX: &str = "provider-connection-v1:";
const PROVIDER_PROTOCOL_REVISION_PREFIX: &str = "provider-protocol-v1:";
const SEARCH_CONNECTION_REVISION_PREFIX: &str = "search-connection-v1:";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ModelProviderCredentialJournalRecord {
    pub credential_ref: String,
    pub is_active: bool,
}

pub(crate) fn stage_model_provider_credentials(
    connection: &mut Connection,
    credential_refs: &[String],
) -> rusqlite::Result<()> {
    let transaction = connection.transaction()?;
    for credential_ref in credential_refs {
        transaction.execute(
            "INSERT INTO model_provider_credential_staging (credential_ref, created_at)
             VALUES (?1, ?2)",
            params![credential_ref, now_ms()],
        )?;
    }
    transaction.commit()
}

pub(crate) fn remove_model_provider_credential_staging(
    connection: &Connection,
    credential_ref: &str,
) -> rusqlite::Result<bool> {
    connection
        .execute(
            "DELETE FROM model_provider_credential_staging WHERE credential_ref = ?1",
            [credential_ref],
        )
        .map(|changed| changed > 0)
}

pub(crate) fn list_model_provider_credential_staging(
    connection: &Connection,
) -> rusqlite::Result<Vec<ModelProviderCredentialJournalRecord>> {
    list_model_provider_credential_journal(connection, "model_provider_credential_staging")
}

pub(crate) fn list_model_provider_credential_cleanup(
    connection: &Connection,
) -> rusqlite::Result<Vec<ModelProviderCredentialJournalRecord>> {
    list_model_provider_credential_journal(connection, "model_provider_credential_cleanup")
}

fn list_model_provider_credential_journal(
    connection: &Connection,
    table: &str,
) -> rusqlite::Result<Vec<ModelProviderCredentialJournalRecord>> {
    debug_assert!(matches!(
        table,
        "model_provider_credential_staging" | "model_provider_credential_cleanup"
    ));
    let sql = format!(
        "SELECT journal.credential_ref,
                EXISTS (
                    SELECT 1 FROM model_provider_settings AS settings
                    WHERE settings.api_token_ref = journal.credential_ref
                       OR settings.tavily_api_key_ref = journal.credential_ref
                    UNION ALL
                    SELECT 1 FROM models AS model
                    WHERE model.api_token_override_ref = journal.credential_ref
                ) AS is_active
         FROM {table} AS journal
         ORDER BY journal.created_at ASC, journal.credential_ref ASC"
    );
    let mut statement = connection.prepare(&sql)?;
    let records = statement
        .query_map([], |row| {
            Ok(ModelProviderCredentialJournalRecord {
                credential_ref: row.get(0)?,
                is_active: row.get(1)?,
            })
        })?
        .collect();
    records
}

pub(crate) fn remove_model_provider_credential_cleanup(
    connection: &Connection,
    credential_ref: &str,
) -> rusqlite::Result<bool> {
    connection
        .execute(
            "DELETE FROM model_provider_credential_cleanup WHERE credential_ref = ?1",
            [credential_ref],
        )
        .map(|changed| changed > 0)
}

pub(crate) fn load_model_settings(
    connection: &mut Connection,
) -> rusqlite::Result<Option<StoredModelSettingsRecord>> {
    Ok(load_model_settings_snapshot(connection)?.map(|snapshot| snapshot.settings))
}

pub(crate) fn load_model_settings_snapshot(
    connection: &mut Connection,
) -> rusqlite::Result<Option<StoredModelSettingsSnapshot>> {
    let transaction = connection.transaction()?;
    let snapshot = load_model_settings_snapshot_in_connection(&transaction)?;
    transaction.commit()?;
    Ok(snapshot)
}

/// Loads one coherent model catalog through an existing transaction/connection.
///
/// Collaboration spawn uses this entry point while holding its `BEGIN IMMEDIATE` transaction so
/// model/template resolution and the immutable child snapshot cannot observe different catalog
/// revisions. Ordinary callers should continue to use [`load_model_settings_snapshot`].
pub(crate) fn load_model_settings_snapshot_in_connection(
    connection: &Connection,
) -> rusqlite::Result<Option<StoredModelSettingsSnapshot>> {
    let settings = connection
        .query_row(
            "
            SELECT
                api_url,
                api_token_ref,
                search_mode,
                tavily_api_key_ref,
                configuration_revision,
                search_connection_revision
            FROM model_provider_settings
            WHERE id = 'default'
            ",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                ))
            },
        )
        .optional()?;

    let Some((
        api_url,
        api_token_ref,
        search_mode,
        tavily_api_key_ref,
        configuration_revision,
        search_connection_revision,
    )) = settings
    else {
        return Ok(None);
    };
    if !is_model_settings_revision(&configuration_revision) {
        return Err(rusqlite::Error::InvalidQuery);
    }
    if !is_search_connection_revision(&search_connection_revision) {
        return Err(rusqlite::Error::InvalidQuery);
    }

    let LoadedModels {
        models,
        provider_connection_revisions,
        provider_protocol_revisions,
    } = load_models(connection)?;
    Ok(Some(StoredModelSettingsSnapshot {
        settings: StoredModelSettingsRecord {
            api_url,
            api_token_ref,
            search_mode,
            tavily_api_key_ref,
            models,
        },
        configuration_revision,
        provider_connection_revisions,
        provider_protocol_revisions,
        search_connection_revision,
    }))
}

#[cfg(test)]
pub(crate) fn save_model_settings(
    connection: &mut Connection,
    settings: StoredModelSettingsRecord,
) -> rusqlite::Result<()> {
    save_model_settings_with_credential_journal(connection, settings, &[], &[]).map(|_| ())
}

#[cfg(test)]
pub(crate) fn credential_free_fixture(
    settings: crate::storage::models::ModelSettingsRecord,
) -> StoredModelSettingsRecord {
    StoredModelSettingsRecord {
        api_url: settings.api_url,
        api_token_ref: None,
        search_mode: settings.search_mode,
        tavily_api_key_ref: None,
        models: settings
            .models
            .into_iter()
            .map(|model| StoredModelConfigRecord {
                id: model.id,
                provider_model_id: model.provider_model_id,
                display_name: model.display_name,
                api_url_override: model.api_url_override,
                api_token_override_ref: None,
                supports_image: model.supports_image,
                context_window_tokens: model.context_window_tokens,
                provider_profile_config: model.provider_profile_config,
                input_price: model.input_price,
                cached_input_price: model.cached_input_price,
                output_price: model.output_price,
                enabled: model.enabled,
            })
            .collect(),
    }
}

pub(crate) fn save_model_settings_with_credential_journal(
    connection: &mut Connection,
    mut settings: StoredModelSettingsRecord,
    published_staging_refs: &[String],
    retired_credential_refs: &[String],
) -> rusqlite::Result<StoredModelSettingsSnapshot> {
    for model in &mut settings.models {
        model.provider_model_id = model.provider_model_id.trim().to_string();
        model.display_name = model.display_name.trim().to_string();
    }
    let previous = load_model_settings_snapshot(connection)?;
    let incoming_model_ids = settings
        .models
        .iter()
        .map(|model| model.id.as_str())
        .collect::<BTreeSet<_>>();
    if incoming_model_ids.len() != settings.models.len() {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let normalized_display_names = settings
        .models
        .iter()
        .map(|model| normalize_model_display_name(&model.display_name))
        .collect::<BTreeSet<_>>();
    if normalized_display_names.len() != settings.models.len()
        || normalized_display_names.iter().any(String::is_empty)
        || settings
            .models
            .iter()
            .any(|model| model.provider_model_id.is_empty())
    {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let removed_model_ids = previous
        .as_ref()
        .map(|snapshot| {
            snapshot
                .settings
                .models
                .iter()
                .filter(|model| !incoming_model_ids.contains(model.id.as_str()))
                .map(|model| model.id.clone())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let timestamp = now_ms();
    let configuration_revision = new_model_settings_revision();
    let search_connection_revision = previous
        .as_ref()
        .filter(|snapshot| same_effective_search_connection(&snapshot.settings, &settings))
        .map(|snapshot| snapshot.search_connection_revision.clone())
        .unwrap_or_else(new_search_connection_revision);
    let provider_connection_revisions = settings
        .models
        .iter()
        .map(|model| {
            let preserved = previous.as_ref().and_then(|snapshot| {
                let previous_model = snapshot
                    .settings
                    .models
                    .iter()
                    .find(|candidate| candidate.id == model.id)?;
                let previous_connection =
                    effective_connection_identity(&snapshot.settings, previous_model)?;
                let current_connection = effective_connection_identity(&settings, model)?;
                (previous_connection == current_connection)
                    .then(|| {
                        snapshot
                            .provider_connection_revisions
                            .get(&model.id)
                            .cloned()
                    })
                    .flatten()
            });
            (
                model.id.clone(),
                preserved.unwrap_or_else(new_provider_connection_revision),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let provider_protocol_revisions = settings
        .models
        .iter()
        .map(|model| {
            let preserved = previous.as_ref().and_then(|snapshot| {
                let previous_model = snapshot
                    .settings
                    .models
                    .iter()
                    .find(|candidate| candidate.id == model.id)?;
                same_effective_provider_protocol(
                    &snapshot.settings,
                    previous_model,
                    &settings,
                    model,
                )
                .then(|| snapshot.provider_protocol_revisions.get(&model.id).cloned())
                .flatten()
            });
            (
                model.id.clone(),
                preserved.unwrap_or_else(new_provider_protocol_revision),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let transaction = connection.transaction()?;

    transaction.execute(
        "
        INSERT INTO model_provider_settings (
            id,
            api_url,
            api_token_ref,
            search_mode,
            tavily_api_key_ref,
            configuration_revision,
            search_connection_revision,
            updated_at
        )
        VALUES ('default', ?1, ?2, ?3, ?4, ?5, ?6, ?7)
        ON CONFLICT(id) DO UPDATE SET
            api_url = excluded.api_url,
            api_token_ref = excluded.api_token_ref,
            search_mode = excluded.search_mode,
            tavily_api_key_ref = excluded.tavily_api_key_ref,
            configuration_revision = excluded.configuration_revision,
            search_connection_revision = excluded.search_connection_revision,
            updated_at = excluded.updated_at
        ",
        params![
            &settings.api_url,
            &settings.api_token_ref,
            &settings.search_mode,
            &settings.tavily_api_key_ref,
            configuration_revision,
            search_connection_revision,
            timestamp
        ],
    )?;

    for (index, model) in settings.models.iter().enumerate() {
        transaction.execute(
            "
            INSERT INTO models (
                id,
                provider_model_id,
                display_name,
                normalized_display_name,
                api_url_override,
                api_token_override_ref,
                supports_image,
                context_window_tokens,
                provider_profile_config_json,
                provider_connection_revision,
                provider_protocol_revision,
                input_price,
                cached_input_price,
                output_price,
                enabled,
                position,
                created_at,
                updated_at
            )
            VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15,
                ?16, ?17, ?17
            )
            ON CONFLICT(id) DO UPDATE SET
                provider_model_id = excluded.provider_model_id,
                display_name = excluded.display_name,
                normalized_display_name = excluded.normalized_display_name,
                api_url_override = excluded.api_url_override,
                api_token_override_ref = excluded.api_token_override_ref,
                supports_image = excluded.supports_image,
                context_window_tokens = excluded.context_window_tokens,
                provider_profile_config_json = excluded.provider_profile_config_json,
                provider_connection_revision = excluded.provider_connection_revision,
                provider_protocol_revision = excluded.provider_protocol_revision,
                input_price = excluded.input_price,
                cached_input_price = excluded.cached_input_price,
                output_price = excluded.output_price,
                enabled = excluded.enabled,
                position = excluded.position,
                updated_at = excluded.updated_at
            ",
            params![
                &model.id,
                &model.provider_model_id,
                &model.display_name,
                normalize_model_display_name(&model.display_name),
                &model.api_url_override,
                &model.api_token_override_ref,
                model.supports_image,
                model.context_window_tokens,
                encode_provider_profile_config(&model.provider_profile_config)?,
                provider_connection_revisions
                    .get(&model.id)
                    .ok_or(rusqlite::Error::InvalidQuery)?,
                provider_protocol_revisions
                    .get(&model.id)
                    .ok_or(rusqlite::Error::InvalidQuery)?,
                &model.input_price,
                &model.cached_input_price,
                &model.output_price,
                model.enabled,
                index as i64,
                timestamp,
            ],
        )?;
    }

    for model_id in removed_model_ids {
        transaction.execute("DELETE FROM models WHERE id = ?1", params![model_id])?;
    }

    for credential_ref in published_staging_refs {
        transaction.execute(
            "DELETE FROM model_provider_credential_staging WHERE credential_ref = ?1",
            [credential_ref],
        )?;
    }
    for credential_ref in retired_credential_refs {
        transaction.execute(
            "INSERT OR IGNORE INTO model_provider_credential_cleanup (credential_ref, created_at)
             VALUES (?1, ?2)",
            params![credential_ref, timestamp],
        )?;
    }

    transaction.commit()?;
    Ok(StoredModelSettingsSnapshot {
        settings,
        configuration_revision,
        provider_connection_revisions,
        provider_protocol_revisions,
        search_connection_revision,
    })
}

struct LoadedModels {
    models: Vec<StoredModelConfigRecord>,
    provider_connection_revisions: BTreeMap<String, String>,
    provider_protocol_revisions: BTreeMap<String, String>,
}

fn load_models(connection: &Connection) -> rusqlite::Result<LoadedModels> {
    let mut statement = connection.prepare(
        "
        SELECT
            id,
            provider_model_id,
            display_name,
            normalized_display_name,
            api_url_override,
            api_token_override_ref,
            supports_image,
            context_window_tokens,
            provider_profile_config_json,
            provider_connection_revision,
            provider_protocol_revision,
            input_price,
            cached_input_price,
            output_price,
            enabled
        FROM models
        ORDER BY position ASC, created_at ASC
        ",
    )?;

    let stored = statement
        .query_map([], |row| {
            let display_name = row.get::<_, String>(2)?;
            if normalize_model_display_name(&display_name) != row.get::<_, String>(3)? {
                return Err(rusqlite::Error::InvalidQuery);
            }
            let model = StoredModelConfigRecord {
                id: row.get(0)?,
                provider_model_id: row.get(1)?,
                display_name,
                api_url_override: row.get(4)?,
                api_token_override_ref: row.get(5)?,
                supports_image: row.get(6)?,
                context_window_tokens: row.get(7)?,
                provider_profile_config: decode_provider_profile_config(row.get(8)?, 8)?,
                input_price: row.get(11)?,
                cached_input_price: row.get(12)?,
                output_price: row.get(13)?,
                enabled: row.get(14)?,
            };
            let connection_revision = row.get::<_, String>(9)?;
            if !is_provider_connection_revision(&connection_revision) {
                return Err(rusqlite::Error::InvalidQuery);
            }
            let protocol_revision = row.get::<_, String>(10)?;
            if !is_provider_protocol_revision(&protocol_revision) {
                return Err(rusqlite::Error::InvalidQuery);
            }
            Ok((model, connection_revision, protocol_revision))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let mut models = Vec::with_capacity(stored.len());
    let mut provider_connection_revisions = BTreeMap::new();
    let mut provider_protocol_revisions = BTreeMap::new();
    for (model, connection_revision, protocol_revision) in stored {
        if provider_connection_revisions
            .insert(model.id.clone(), connection_revision)
            .is_some()
            || provider_protocol_revisions
                .insert(model.id.clone(), protocol_revision)
                .is_some()
        {
            return Err(rusqlite::Error::InvalidQuery);
        }
        models.push(model);
    }
    Ok(LoadedModels {
        models,
        provider_connection_revisions,
        provider_protocol_revisions,
    })
}

fn encode_provider_profile_config(config: &ProviderProfileConfig) -> rusqlite::Result<String> {
    serde_json::to_string(config)
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))
}

fn decode_provider_profile_config(
    encoded: String,
    column: usize,
) -> rusqlite::Result<ProviderProfileConfig> {
    serde_json::from_str::<ProviderProfileConfig>(&encoded).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(column, Type::Text, Box::new(error))
    })
}

fn new_model_settings_revision() -> String {
    format!("{MODEL_SETTINGS_REVISION_PREFIX}{}", Uuid::new_v4())
}

fn new_provider_connection_revision() -> String {
    format!("{PROVIDER_CONNECTION_REVISION_PREFIX}{}", Uuid::new_v4())
}

fn new_provider_protocol_revision() -> String {
    format!("{PROVIDER_PROTOCOL_REVISION_PREFIX}{}", Uuid::new_v4())
}

fn new_search_connection_revision() -> String {
    format!("{SEARCH_CONNECTION_REVISION_PREFIX}{}", Uuid::new_v4())
}

fn same_effective_search_connection(
    previous: &StoredModelSettingsRecord,
    current: &StoredModelSettingsRecord,
) -> bool {
    canonical_search_mode(&previous.search_mode) == canonical_search_mode(&current.search_mode)
        && previous.tavily_api_key_ref == current.tavily_api_key_ref
}

/// Compares the complete provider-owned wire identity for one configured model.
///
/// A revision is reusable only when the exact endpoint/credential pair, wire model id, detected
/// dialect, Profile/version and reasoning policy are all equivalent. Renderer-only metadata,
/// pricing, capacity and search settings do not participate.
fn same_effective_provider_protocol(
    previous_settings: &StoredModelSettingsRecord,
    previous_model: &StoredModelConfigRecord,
    current_settings: &StoredModelSettingsRecord,
    current_model: &StoredModelConfigRecord,
) -> bool {
    if previous_model.provider_model_id != current_model.provider_model_id {
        return false;
    }
    let Some(previous_connection) =
        effective_connection_identity(previous_settings, previous_model)
    else {
        return false;
    };
    let Some(current_connection) = effective_connection_identity(current_settings, current_model)
    else {
        return false;
    };
    if previous_connection != current_connection {
        return false;
    }

    let previous_dialect = ProviderProtocolDialect::detect_from_api_url(previous_connection.0);
    let current_dialect = ProviderProtocolDialect::detect_from_api_url(current_connection.0);
    if previous_dialect != current_dialect {
        return false;
    }

    match (
        previous_model
            .provider_profile_config
            .validate_for_model(&previous_model.provider_model_id, previous_dialect),
        current_model
            .provider_profile_config
            .validate_for_model(&current_model.provider_model_id, current_dialect),
    ) {
        (Ok(()), Ok(())) => {
            previous_model.provider_profile_config == current_model.provider_profile_config
        }
        // Unsupported persisted profiles remain visible and replaceable without becoming a
        // runtime capability. If the exact opaque configuration is preserved, unrelated edits do
        // not manufacture a wire change; any Run still fails at exact Registration resolution.
        _ => previous_model.provider_profile_config == current_model.provider_profile_config,
    }
}

fn effective_connection_identity<'a>(
    settings: &'a StoredModelSettingsRecord,
    model: &'a StoredModelConfigRecord,
) -> Option<(&'a str, &'a str)> {
    match (
        model
            .api_url_override
            .as_deref()
            .filter(|value| !value.trim().is_empty()),
        model.api_token_override_ref.as_deref(),
    ) {
        (Some(url), Some(reference)) => Some((url.trim(), reference)),
        (None, None) => settings
            .api_token_ref
            .as_deref()
            .filter(|_| !settings.api_url.trim().is_empty())
            .map(|reference| (settings.api_url.trim(), reference)),
        _ => None,
    }
}

fn canonical_search_mode(value: &str) -> &'static str {
    match value {
        "disabled" => "disabled",
        "tavily" => "tavily",
        _ => "auto",
    }
}

pub fn is_model_settings_revision(value: &str) -> bool {
    is_canonical_v4_revision(value, MODEL_SETTINGS_REVISION_PREFIX)
}

pub fn is_provider_connection_revision(value: &str) -> bool {
    is_canonical_v4_revision(value, PROVIDER_CONNECTION_REVISION_PREFIX)
}

/// Validates the per-model opaque identity carried by `ProviderProtocolKey` and pending-run
/// provenance. Global model-settings revisions are deliberately not protocol identities.
pub fn is_provider_protocol_revision(value: &str) -> bool {
    is_canonical_v4_revision(value, PROVIDER_PROTOCOL_REVISION_PREFIX)
}

pub fn is_search_connection_revision(value: &str) -> bool {
    is_canonical_v4_revision(value, SEARCH_CONNECTION_REVISION_PREFIX)
}

fn is_canonical_v4_revision(value: &str, prefix: &str) -> bool {
    let Some(raw_uuid) = value.strip_prefix(prefix) else {
        return false;
    };
    Uuid::parse_str(raw_uuid)
        .is_ok_and(|uuid| uuid.get_version_num() == 4 && uuid.to_string() == raw_uuid)
}
