use crate::storage::models::{
    BrowserDownloadListInput, BrowserDownloadLocationMode, BrowserDownloadRecord,
    BrowserDownloadRegistration, BrowserDownloadSettingsRecord, BrowserDownloadSettingsUpdate,
    BrowserDownloadSource, BROWSER_DOWNLOAD_SCHEMA_VERSION,
};
use rusqlite::{params, Connection, OptionalExtension};

pub fn load_settings(connection: &Connection) -> rusqlite::Result<BrowserDownloadSettingsRecord> {
    connection.query_row(
        "SELECT schema_version, location_mode, custom_directory, ask_where_to_save,
                revision, updated_at
         FROM browser_download_settings WHERE id = 'default'",
        [],
        |row| {
            Ok(BrowserDownloadSettingsRecord {
                schema_version: row.get(0)?,
                location_mode: parse_location_mode(row.get::<_, String>(1)?)?,
                custom_directory: row.get(2)?,
                ask_where_to_save: row.get(3)?,
                revision: row.get(4)?,
                updated_at: row.get(5)?,
            })
        },
    )
}

pub fn save_settings(
    connection: &mut Connection,
    update: &BrowserDownloadSettingsUpdate,
) -> rusqlite::Result<Option<BrowserDownloadSettingsRecord>> {
    let transaction = connection.transaction()?;
    let changed = transaction.execute(
        "UPDATE browser_download_settings
         SET location_mode = ?1,
             custom_directory = ?2,
             ask_where_to_save = ?3,
             revision = revision + 1,
             updated_at = ?4
         WHERE id = 'default' AND revision = ?5",
        params![
            location_mode_text(update.location_mode),
            update.custom_directory,
            update.ask_where_to_save,
            update.updated_at,
            update.expected_revision
        ],
    )?;
    if changed != 1 {
        transaction.rollback()?;
        return Ok(None);
    }
    let saved = load_settings(&transaction)?;
    transaction.commit()?;
    Ok(Some(saved))
}

pub fn register(
    connection: &mut Connection,
    input: &BrowserDownloadRegistration,
) -> rusqlite::Result<BrowserDownloadRecord> {
    let transaction = connection.transaction()?;
    let project_id = match input.conversation_id.as_deref() {
        Some(conversation_id) => transaction
            .query_row(
                "SELECT project_id FROM conversations WHERE id = ?1",
                [conversation_id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()?
            .ok_or_else(|| rusqlite::Error::QueryReturnedNoRows)?,
        None => None,
    };
    transaction.execute(
        "INSERT INTO browser_downloads (
             download_id, schema_version, source_kind, display_name, mime_type,
             size_bytes, sha256, absolute_path, source_origin, conversation_id,
             project_id, run_id, call_id, created_at
         ) VALUES (
             ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14
         )",
        params![
            input.download_id,
            BROWSER_DOWNLOAD_SCHEMA_VERSION,
            source_text(input.source),
            input.display_name,
            input.mime_type,
            input.size_bytes,
            input.sha256,
            input.absolute_path,
            input.source_origin,
            input.conversation_id,
            project_id,
            input.run_id,
            input.call_id,
            input.created_at
        ],
    )?;
    let record = load_by_id(&transaction, &input.download_id)?
        .ok_or_else(|| rusqlite::Error::QueryReturnedNoRows)?;
    transaction.commit()?;
    Ok(record)
}

pub fn load_by_id(
    connection: &Connection,
    download_id: &str,
) -> rusqlite::Result<Option<BrowserDownloadRecord>> {
    connection
        .query_row(
            "SELECT schema_version, download_id, source_kind, display_name, mime_type,
                    size_bytes, sha256, absolute_path, source_origin, conversation_id,
                    project_id, run_id, call_id, created_at
             FROM browser_downloads WHERE download_id = ?1",
            [download_id],
            map_record,
        )
        .optional()
}

pub fn list(
    connection: &Connection,
    input: &BrowserDownloadListInput,
) -> rusqlite::Result<Vec<BrowserDownloadRecord>> {
    let query = input.query.trim().to_lowercase();
    let pattern = format!("%{}%", escape_like(&query));
    let mut statement = connection.prepare(
        "SELECT schema_version, download_id, source_kind, display_name, mime_type,
                size_bytes, sha256, absolute_path, source_origin, conversation_id,
                project_id, run_id, call_id, created_at
         FROM browser_downloads
         WHERE (?1 = '' OR lower(display_name) LIKE ?2 ESCAPE '\\')
         ORDER BY created_at DESC, download_id DESC
         LIMIT ?3",
    )?;
    let rows = statement.query_map(
        params![query, pattern, i64::from(input.limit) + 1],
        map_record,
    )?;
    rows.collect()
}

pub fn clear(connection: &Connection) -> rusqlite::Result<usize> {
    connection.execute("DELETE FROM browser_downloads", [])
}

pub fn conversation_project_id(
    connection: &Connection,
    conversation_id: &str,
) -> rusqlite::Result<Option<Option<String>>> {
    connection
        .query_row(
            "SELECT project_id FROM conversations WHERE id = ?1",
            [conversation_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
}

fn map_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<BrowserDownloadRecord> {
    Ok(BrowserDownloadRecord {
        schema_version: row.get(0)?,
        download_id: row.get(1)?,
        source: parse_source(row.get::<_, String>(2)?)?,
        display_name: row.get(3)?,
        mime_type: row.get(4)?,
        size_bytes: row.get(5)?,
        sha256: row.get(6)?,
        absolute_path: row.get(7)?,
        source_origin: row.get(8)?,
        conversation_id: row.get(9)?,
        project_id: row.get(10)?,
        run_id: row.get(11)?,
        call_id: row.get(12)?,
        created_at: row.get(13)?,
    })
}

fn parse_location_mode(value: String) -> rusqlite::Result<BrowserDownloadLocationMode> {
    match value.as_str() {
        "system" => Ok(BrowserDownloadLocationMode::System),
        "custom" => Ok(BrowserDownloadLocationMode::Custom),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}

fn location_mode_text(value: BrowserDownloadLocationMode) -> &'static str {
    match value {
        BrowserDownloadLocationMode::System => "system",
        BrowserDownloadLocationMode::Custom => "custom",
    }
}

fn parse_source(value: String) -> rusqlite::Result<BrowserDownloadSource> {
    match value.as_str() {
        "manual" => Ok(BrowserDownloadSource::Manual),
        "agent" => Ok(BrowserDownloadSource::Agent),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}

fn source_text(value: BrowserDownloadSource) -> &'static str {
    match value {
        BrowserDownloadSource::Manual => "manual",
        BrowserDownloadSource::Agent => "agent",
    }
}

fn escape_like(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}
