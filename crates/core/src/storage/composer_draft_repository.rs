// Rust core storage.
use crate::storage::models::ComposerDraftRecord;
use rusqlite::{params, Connection};

pub fn list_composer_drafts(connection: &Connection) -> rusqlite::Result<Vec<ComposerDraftRecord>> {
    let mut statement = connection.prepare(
        "
        SELECT scope_id, message, permission_mode, model_id, project_id, attachments_json, updated_at
        FROM composer_drafts
        ORDER BY updated_at DESC
        ",
    )?;

    let drafts = statement
        .query_map([], |row| {
            Ok(ComposerDraftRecord {
                scope_id: row.get(0)?,
                message: row.get(1)?,
                permission_mode: row.get(2)?,
                model_id: row.get(3)?,
                project_id: row.get(4)?,
                attachments_json: row.get(5)?,
                updated_at: row.get(6)?,
            })
        })?
        .collect();

    drafts
}

pub fn save_composer_draft(
    connection: &Connection,
    draft: ComposerDraftRecord,
) -> rusqlite::Result<()> {
    connection.execute(
        "
        INSERT INTO composer_drafts (
            scope_id,
            message,
            permission_mode,
            model_id,
            project_id,
            attachments_json,
            updated_at
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
        ON CONFLICT(scope_id) DO UPDATE SET
            message = excluded.message,
            permission_mode = excluded.permission_mode,
            model_id = excluded.model_id,
            project_id = excluded.project_id,
            attachments_json = excluded.attachments_json,
            updated_at = excluded.updated_at
        ",
        params![
            &draft.scope_id,
            &draft.message,
            &draft.permission_mode,
            &draft.model_id,
            &draft.project_id,
            &draft.attachments_json,
            draft.updated_at
        ],
    )?;
    Ok(())
}

pub fn delete_composer_draft(connection: &Connection, scope_id: &str) -> rusqlite::Result<()> {
    connection.execute(
        "DELETE FROM composer_drafts WHERE scope_id = ?1",
        params![scope_id],
    )?;
    Ok(())
}

pub fn delete_project_composer_drafts(
    connection: &Connection,
    project_id: &str,
) -> rusqlite::Result<()> {
    connection.execute(
        "
        DELETE FROM composer_drafts
        WHERE project_id = ?1
           OR scope_id IN (
                SELECT id
                FROM conversations
                WHERE project_id = ?1
           )
        ",
        params![project_id],
    )?;
    Ok(())
}
