use crate::storage::models::{
    BrowserHistoryListInput, BrowserHistoryMetadataUpdate, BrowserHistoryRecord,
    BrowserHistoryRegistration, BrowserLinkOpenTarget, BrowserOwnedDataClearInput,
    BrowserOwnedDataClearOutput, BrowserOwnedDataRangeInput, BrowserOwnedDataSummary,
    BrowserPreferencesRecord, BrowserPreferencesUpdate, BROWSER_DATA_SCHEMA_VERSION,
};
use rusqlite::{params, Connection};

pub fn load_preferences(connection: &Connection) -> rusqlite::Result<BrowserPreferencesRecord> {
    connection.query_row(
        "SELECT schema_version, link_open_target, revision, updated_at
         FROM browser_preferences WHERE id = 'default'",
        [],
        |row| {
            Ok(BrowserPreferencesRecord {
                schema_version: row.get(0)?,
                link_open_target: decode_link_target(row.get::<_, String>(1)?)?,
                revision: row.get(2)?,
                updated_at: row.get(3)?,
            })
        },
    )
}

pub fn save_preferences(
    connection: &mut Connection,
    update: &BrowserPreferencesUpdate,
) -> rusqlite::Result<Option<BrowserPreferencesRecord>> {
    let transaction = connection.transaction()?;
    let changed = transaction.execute(
        "UPDATE browser_preferences
         SET link_open_target = ?1, revision = revision + 1, updated_at = ?2
         WHERE id = 'default' AND revision = ?3",
        params![
            encode_link_target(update.link_open_target),
            update.updated_at,
            update.expected_revision
        ],
    )?;
    if changed == 0 {
        transaction.rollback()?;
        return Ok(None);
    }
    let saved = load_preferences(&transaction)?;
    transaction.commit()?;
    Ok(Some(saved))
}

pub fn register_history(
    connection: &Connection,
    input: &BrowserHistoryRegistration,
) -> rusqlite::Result<BrowserHistoryRecord> {
    connection.execute(
        "INSERT INTO browser_history (
            history_id, schema_version, url, title, hostname, favicon_url, visited_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            input.history_id,
            input.schema_version,
            input.url,
            input.title,
            input.hostname,
            input.favicon_url,
            input.visited_at
        ],
    )?;
    Ok(input.clone())
}

pub fn update_history_metadata(
    connection: &Connection,
    input: &BrowserHistoryMetadataUpdate,
) -> rusqlite::Result<bool> {
    Ok(connection.execute(
        "UPDATE browser_history SET title = ?1, favicon_url = ?2 WHERE history_id = ?3",
        params![input.title, input.favicon_url, input.history_id],
    )? > 0)
}

pub fn list_history(
    connection: &Connection,
    input: &BrowserHistoryListInput,
) -> rusqlite::Result<Vec<BrowserHistoryRecord>> {
    let mut statement = connection.prepare(
        "SELECT schema_version, history_id, url, title, hostname, favicon_url, visited_at
         FROM browser_history
         WHERE ?1 = ''
            OR instr(lower(title), lower(?1)) > 0
            OR instr(lower(hostname), lower(?1)) > 0
            OR instr(lower(url), lower(?1)) > 0
         ORDER BY visited_at DESC, history_id DESC
         LIMIT ?2",
    )?;
    let rows = statement.query_map(
        params![input.query.trim(), input.limit.saturating_add(1)],
        |row| {
            Ok(BrowserHistoryRecord {
                schema_version: row.get(0)?,
                history_id: row.get(1)?,
                url: row.get(2)?,
                title: row.get(3)?,
                hostname: row.get(4)?,
                favicon_url: row.get(5)?,
                visited_at: row.get(6)?,
            })
        },
    )?;
    rows.collect()
}

pub fn delete_history(
    connection: &mut Connection,
    history_ids: &[String],
) -> rusqlite::Result<usize> {
    let transaction = connection.transaction()?;
    let mut deleted = 0;
    {
        let mut statement =
            transaction.prepare("DELETE FROM browser_history WHERE history_id = ?1")?;
        for history_id in history_ids {
            deleted += statement.execute([history_id])?;
        }
    }
    transaction.commit()?;
    Ok(deleted)
}

pub fn summarize_owned_data(
    connection: &Connection,
    input: &BrowserOwnedDataRangeInput,
) -> rusqlite::Result<BrowserOwnedDataSummary> {
    let history_count =
        count_with_optional_since(connection, "browser_history", "visited_at", input.since)?;
    let history_site_count = connection.query_row(
        "SELECT COUNT(DISTINCT hostname) FROM browser_history
         WHERE ?1 IS NULL OR visited_at >= ?1",
        [input.since],
        |row| row.get(0),
    )?;
    let download_count =
        count_with_optional_since(connection, "browser_downloads", "created_at", input.since)?;
    Ok(BrowserOwnedDataSummary {
        schema_version: BROWSER_DATA_SCHEMA_VERSION,
        history_count,
        history_site_count,
        download_count,
    })
}

pub fn clear_owned_data(
    connection: &mut Connection,
    input: &BrowserOwnedDataClearInput,
) -> rusqlite::Result<BrowserOwnedDataClearOutput> {
    let transaction = connection.transaction()?;
    let deleted_history_count = if input.clear_history {
        transaction.execute(
            "DELETE FROM browser_history WHERE ?1 IS NULL OR visited_at >= ?1",
            [input.since],
        )? as u64
    } else {
        0
    };
    let deleted_download_count = if input.clear_downloads {
        transaction.execute(
            "DELETE FROM browser_downloads WHERE ?1 IS NULL OR created_at >= ?1",
            [input.since],
        )? as u64
    } else {
        0
    };
    transaction.commit()?;
    Ok(BrowserOwnedDataClearOutput {
        schema_version: BROWSER_DATA_SCHEMA_VERSION,
        deleted_history_count,
        deleted_download_count,
    })
}

fn count_with_optional_since(
    connection: &Connection,
    table: &str,
    timestamp_column: &str,
    since: Option<i64>,
) -> rusqlite::Result<u64> {
    let sql = format!("SELECT COUNT(*) FROM {table} WHERE ?1 IS NULL OR {timestamp_column} >= ?1");
    connection.query_row(&sql, [since], |row| row.get(0))
}

fn encode_link_target(target: BrowserLinkOpenTarget) -> &'static str {
    match target {
        BrowserLinkOpenTarget::System => "system",
        BrowserLinkOpenTarget::Builtin => "builtin",
    }
}

fn decode_link_target(value: String) -> rusqlite::Result<BrowserLinkOpenTarget> {
    match value.as_str() {
        "system" => Ok(BrowserLinkOpenTarget::System),
        "builtin" => Ok(BrowserLinkOpenTarget::Builtin),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}
