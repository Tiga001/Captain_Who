use crate::storage::models::{ModelConfigRecord, ModelSettingsRecord, ModelSettingsSnapshot};
use crate::storage::now_ms;
use crate::ProviderProfileConfig;
use rusqlite::types::Type;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use std::collections::BTreeMap;
use uuid::Uuid;

const MODEL_SETTINGS_REVISION_PREFIX: &str = "model-settings-v1:";
const PROVIDER_CONNECTION_REVISION_PREFIX: &str = "provider-connection-v1:";
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

    let (models, provider_connection_revisions) = load_models(&transaction)?;
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
                input_price,
                output_price,
                enabled,
                position,
                created_at,
                updated_at
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?13)
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
                &model.input_price,
                &model.output_price,
                model.enabled,
                index as i64,
                timestamp,
            ],
        )?;
    }

    transaction.commit()
}

fn load_models(
    connection: &Transaction<'_>,
) -> rusqlite::Result<(Vec<ModelConfigRecord>, BTreeMap<String, String>)> {
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
            input_price,
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
                input_price: row.get(8)?,
                output_price: row.get(9)?,
                enabled: row.get(10)?,
            };
            let connection_revision = row.get::<_, String>(7)?;
            if !is_provider_connection_revision(&connection_revision) {
                return Err(rusqlite::Error::InvalidQuery);
            }
            Ok((model, connection_revision))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let mut models = Vec::with_capacity(stored.len());
    let mut provider_connection_revisions = BTreeMap::new();
    for (model, revision) in stored {
        if provider_connection_revisions
            .insert(model.id.clone(), revision)
            .is_some()
        {
            return Err(rusqlite::Error::InvalidQuery);
        }
        models.push(model);
    }
    Ok((models, provider_connection_revisions))
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
            config.validate().map_err(|error| {
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
