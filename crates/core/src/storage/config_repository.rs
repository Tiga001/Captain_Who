use crate::storage::models::{ModelConfigRecord, ModelSettingsRecord};
use crate::storage::now_ms;
use rusqlite::{params, Connection, OptionalExtension};

pub fn load_model_settings(
    connection: &Connection,
) -> rusqlite::Result<Option<ModelSettingsRecord>> {
    let settings = connection
        .query_row(
            "
            SELECT api_url, api_token, search_mode, tavily_api_key
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
                ))
            },
        )
        .optional()?;

    let Some((api_url, api_token, search_mode, tavily_api_key)) = settings else {
        return Ok(None);
    };

    Ok(Some(ModelSettingsRecord {
        api_url,
        api_token,
        search_mode,
        tavily_api_key,
        models: load_models(connection)?,
    }))
}

pub fn save_model_settings(
    connection: &mut Connection,
    settings: ModelSettingsRecord,
) -> rusqlite::Result<()> {
    let timestamp = now_ms();
    let transaction = connection.transaction()?;

    transaction.execute(
        "
        INSERT INTO model_provider_settings (
            id,
            api_url,
            api_token,
            search_mode,
            tavily_api_key,
            updated_at
        )
        VALUES ('default', ?1, ?2, ?3, ?4, ?5)
        ON CONFLICT(id) DO UPDATE SET
            api_url = excluded.api_url,
            api_token = excluded.api_token,
            search_mode = excluded.search_mode,
            tavily_api_key = excluded.tavily_api_key,
            updated_at = excluded.updated_at
        ",
        params![
            &settings.api_url,
            &settings.api_token,
            &settings.search_mode,
            &settings.tavily_api_key,
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
                short_name,
                provider_path,
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
                &model.short_name,
                &model.provider_path,
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

fn load_models(connection: &Connection) -> rusqlite::Result<Vec<ModelConfigRecord>> {
    let mut statement = connection.prepare(
        "
        SELECT
            id,
            display_name,
            short_name,
            provider_path,
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
                short_name: row.get(2)?,
                provider_path: row.get(3)?,
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
