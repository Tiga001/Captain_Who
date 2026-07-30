use crate::storage::models::{ModelConfigRecord, ModelSettingsRecord, ModelSettingsSnapshot};
use crate::storage::now_ms;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use uuid::Uuid;

const MODEL_SETTINGS_REVISION_PREFIX: &str = "model-settings-v1:";

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
                configuration_revision
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
                ))
            },
        )
        .optional()?;

    let Some((api_url, api_token, search_mode, tavily_api_key, configuration_revision)) = settings
    else {
        transaction.commit()?;
        return Ok(None);
    };
    if !is_model_settings_revision(&configuration_revision) {
        return Err(rusqlite::Error::InvalidQuery);
    }

    let models = load_models(&transaction)?;
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
    }))
}

pub fn save_model_settings(
    connection: &mut Connection,
    settings: ModelSettingsRecord,
) -> rusqlite::Result<()> {
    let timestamp = now_ms();
    let configuration_revision = new_model_settings_revision();
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
            updated_at
        )
        VALUES ('default', ?1, ?2, ?3, ?4, ?5, ?6)
        ON CONFLICT(id) DO UPDATE SET
            api_url = excluded.api_url,
            api_token = excluded.api_token,
            search_mode = excluded.search_mode,
            tavily_api_key = excluded.tavily_api_key,
            configuration_revision = excluded.configuration_revision,
            updated_at = excluded.updated_at
        ",
        params![
            &settings.api_url,
            &settings.api_token,
            &settings.search_mode,
            &settings.tavily_api_key,
            configuration_revision,
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
                input_price,
                output_price,
                enabled,
                position,
                created_at,
                updated_at
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?11)
            ",
            params![
                &model.id,
                &model.display_name,
                &model.api_url_override,
                &model.api_token_override,
                model.supports_image,
                model.context_window_tokens,
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

fn load_models(connection: &Transaction<'_>) -> rusqlite::Result<Vec<ModelConfigRecord>> {
    let mut statement = connection.prepare(
        "
        SELECT
            id,
            display_name,
            api_url_override,
            api_token_override,
            supports_image,
            context_window_tokens,
            input_price,
            output_price,
            enabled
        FROM models
        ORDER BY position ASC, created_at ASC
        ",
    )?;

    let models = statement
        .query_map([], |row| {
            Ok(ModelConfigRecord {
                id: row.get(0)?,
                display_name: row.get(1)?,
                api_url_override: row.get(2)?,
                api_token_override: row.get(3)?,
                supports_image: row.get(4)?,
                context_window_tokens: row.get(5)?,
                input_price: row.get(6)?,
                output_price: row.get(7)?,
                enabled: row.get(8)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    Ok(models)
}

fn new_model_settings_revision() -> String {
    format!("{MODEL_SETTINGS_REVISION_PREFIX}{}", Uuid::new_v4())
}

pub fn is_model_settings_revision(value: &str) -> bool {
    let Some(raw_uuid) = value.strip_prefix(MODEL_SETTINGS_REVISION_PREFIX) else {
        return false;
    };
    Uuid::parse_str(raw_uuid)
        .is_ok_and(|uuid| uuid.get_version_num() == 4 && uuid.to_string() == raw_uuid)
}
