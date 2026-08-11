use crate::storage::models::{ModelConfigRecord, ModelSettingsRecord, ModelSettingsSnapshot};
use crate::storage::now_ms;
use crate::{ProviderProfileConfig, ProviderProtocolDialect};
use rusqlite::types::Type;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use std::collections::BTreeMap;
use uuid::Uuid;

const MODEL_SETTINGS_REVISION_PREFIX: &str = "model-settings-v1:";
const PROVIDER_CONNECTION_REVISION_PREFIX: &str = "provider-connection-v1:";
const PROVIDER_PROTOCOL_REVISION_PREFIX: &str = "provider-protocol-v1:";
const SEARCH_CONNECTION_REVISION_PREFIX: &str = "search-connection-v1:";

pub fn load_model_settings(
    connection: &mut Connection,
) -> rusqlite::Result<Option<ModelSettingsRecord>> {
    Ok(load_model_settings_snapshot(connection)?.map(|snapshot| snapshot.settings))
}

pub fn load_model_settings_snapshot(
    connection: &mut Connection,
) -> rusqlite::Result<Option<ModelSettingsSnapshot>> {
    let transaction = connection.transaction()?;
    let settings = transaction
        .query_row(
            "
            SELECT
                api_url,
                api_token,
                search_mode,
                tavily_api_key,
                configuration_revision,
                search_connection_revision
            FROM model_provider_settings
            WHERE id = 'default'
            ",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                ))
            },
        )
        .optional()?;

    let Some((
        api_url,
        api_token,
        search_mode,
        tavily_api_key,
        configuration_revision,
        search_connection_revision,
    )) = settings
    else {
        transaction.commit()?;
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
    } = load_models(&transaction)?;
    transaction.commit()?;
    Ok(Some(ModelSettingsSnapshot {
        settings: ModelSettingsRecord {
            api_url,
            api_token,
            search_mode,
            tavily_api_key,
            models,
        },
        configuration_revision,
        provider_connection_revisions,
        provider_protocol_revisions,
        search_connection_revision,
    }))
}

pub fn save_model_settings(
    connection: &mut Connection,
    settings: ModelSettingsRecord,
) -> rusqlite::Result<()> {
    let previous = load_model_settings_snapshot(connection)?;
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
                let previous_connection = snapshot
                    .settings
                    .effective_connection_for(previous_model)
                    .ok()?;
                let current_connection = settings.effective_connection_for(model).ok()?;
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
            api_token,
            search_mode,
            tavily_api_key,
            configuration_revision,
            search_connection_revision,
            updated_at
        )
        VALUES ('default', ?1, ?2, ?3, ?4, ?5, ?6, ?7)
        ON CONFLICT(id) DO UPDATE SET
            api_url = excluded.api_url,
            api_token = excluded.api_token,
            search_mode = excluded.search_mode,
            tavily_api_key = excluded.tavily_api_key,
            configuration_revision = excluded.configuration_revision,
            search_connection_revision = excluded.search_connection_revision,
            updated_at = excluded.updated_at
        ",
        params![
            &settings.api_url,
            &settings.api_token,
            &settings.search_mode,
            &settings.tavily_api_key,
            configuration_revision,
            search_connection_revision,
            timestamp
        ],
    )?;

    transaction.execute("DELETE FROM models", [])?;

    for (index, model) in settings.models.iter().enumerate() {
        transaction.execute(
            "
            INSERT INTO models (
                id,
                display_name,
                api_url_override,
                api_token_override,
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
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?15)
            ",
            params![
                &model.id,
                &model.display_name,
                &model.api_url_override,
                &model.api_token_override,
                model.supports_image,
                model.context_window_tokens,
                encode_provider_profile_config(model.provider_profile_config.as_ref())?,
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

    transaction.commit()
}

struct LoadedModels {
    models: Vec<ModelConfigRecord>,
    provider_connection_revisions: BTreeMap<String, String>,
    provider_protocol_revisions: BTreeMap<String, String>,
}

fn load_models(connection: &Transaction<'_>) -> rusqlite::Result<LoadedModels> {
    let mut statement = connection.prepare(
        "
        SELECT
            id,
            display_name,
            api_url_override,
            api_token_override,
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
            let model = ModelConfigRecord {
                id: row.get(0)?,
                display_name: row.get(1)?,
                api_url_override: row.get(2)?,
                api_token_override: row.get(3)?,
                supports_image: row.get(4)?,
                context_window_tokens: row.get(5)?,
                provider_profile_config: decode_provider_profile_config(row.get(6)?, 6)?,
                input_price: row.get(9)?,
                cached_input_price: row.get(10)?,
                output_price: row.get(11)?,
                enabled: row.get(12)?,
            };
            let connection_revision = row.get::<_, String>(7)?;
            if !is_provider_connection_revision(&connection_revision) {
                return Err(rusqlite::Error::InvalidQuery);
            }
            let protocol_revision = row.get::<_, String>(8)?;
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

fn encode_provider_profile_config(
    config: Option<&ProviderProfileConfig>,
) -> rusqlite::Result<Option<String>> {
    config
        .map(|config| {
            serde_json::to_string(config)
                .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))
        })
        .transpose()
}

fn decode_provider_profile_config(
    encoded: Option<String>,
    column: usize,
) -> rusqlite::Result<Option<ProviderProfileConfig>> {
    encoded
        .map(|encoded| {
            let config =
                serde_json::from_str::<ProviderProfileConfig>(&encoded).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(column, Type::Text, Box::new(error))
                })?;
            Ok(config)
        })
        .transpose()
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
    previous: &ModelSettingsRecord,
    current: &ModelSettingsRecord,
) -> bool {
    canonical_search_mode(&previous.search_mode) == canonical_search_mode(&current.search_mode)
        && previous.tavily_api_key.trim() == current.tavily_api_key.trim()
}

/// Compares the complete provider-owned wire identity for one configured model.
///
/// A revision is reusable only when the exact endpoint/credential pair, wire model id, detected
/// dialect, Profile/version and reasoning policy are all equivalent. Renderer-only metadata,
/// pricing, capacity and search settings do not participate.
fn same_effective_provider_protocol(
    previous_settings: &ModelSettingsRecord,
    previous_model: &ModelConfigRecord,
    current_settings: &ModelSettingsRecord,
    current_model: &ModelConfigRecord,
) -> bool {
    if previous_model.id != current_model.id {
        return false;
    }
    let Ok(previous_connection) = previous_settings.effective_connection_for(previous_model) else {
        return false;
    };
    let Ok(current_connection) = current_settings.effective_connection_for(current_model) else {
        return false;
    };
    if previous_connection != current_connection {
        return false;
    }

    let previous_dialect =
        ProviderProtocolDialect::detect_from_api_url(&previous_connection.api_url);
    let current_dialect = ProviderProtocolDialect::detect_from_api_url(&current_connection.api_url);
    if previous_dialect != current_dialect {
        return false;
    }

    match (
        previous_model.resolved_provider_profile_config(previous_dialect),
        current_model.resolved_provider_profile_config(current_dialect),
    ) {
        (Ok(previous), Ok(current)) => previous == current,
        // Unsupported persisted profiles remain visible and replaceable without becoming a
        // runtime capability. If the exact opaque configuration is preserved, unrelated edits do
        // not manufacture a wire change; any Run still fails at exact Registration resolution.
        _ => previous_model.provider_profile_config == current_model.provider_profile_config,
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
