use crate::storage::models::ComposerDraftRecord;
use rusqlite::{params, Connection};

pub fn list_composer_drafts(connection: &Connection) -> rusqlite::Result<Vec<ComposerDraftRecord>> {
    let mut statement = connection.prepare(
        "
        SELECT scope_id, message, permission_mode, permission_mode_version, model_id, project_id,
               attachments_json, folder_references_json, skills_json, queued_messages_json, updated_at
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
                permission_mode_version: row.get(3)?,
                model_id: row.get(4)?,
                project_id: row.get(5)?,
                attachments_json: row.get(6)?,
                folder_references_json: row.get(7)?,
                skills_json: row.get(8)?,
                queued_messages_json: row.get(9)?,
                updated_at: row.get(10)?,
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
            permission_mode_version,
            model_id,
            project_id,
            attachments_json,
            folder_references_json,
            skills_json,
            queued_messages_json,
            updated_at
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
        ON CONFLICT(scope_id) DO UPDATE SET
            message = excluded.message,
            permission_mode = excluded.permission_mode,
            permission_mode_version = excluded.permission_mode_version,
            model_id = excluded.model_id,
            project_id = excluded.project_id,
            attachments_json = excluded.attachments_json,
            folder_references_json = excluded.folder_references_json,
            skills_json = excluded.skills_json,
            queued_messages_json = excluded.queued_messages_json,
            updated_at = excluded.updated_at
        WHERE excluded.updated_at > composer_drafts.updated_at
           OR (
                excluded.updated_at = composer_drafts.updated_at
                AND excluded.model_id IS composer_drafts.model_id
           )
        ",
        params![
            &draft.scope_id,
            &draft.message,
            &draft.permission_mode,
            draft.permission_mode_version,
            &draft.model_id,
            &draft.project_id,
            &draft.attachments_json,
            &draft.folder_references_json,
            &draft.skills_json,
            &draft.queued_messages_json,
            draft.updated_at
        ],
    )?;
    Ok(())
}

/// Updates only the rapidly changing text field while preserving heavyweight draft payloads.
/// Returns `false` only when the scope has no durable draft yet, allowing the caller to seed it
/// once through the full save path. A stale update still returns `true` without overwriting data.
pub fn save_composer_draft_message(
    connection: &Connection,
    scope_id: &str,
    message: &str,
    updated_at: i64,
) -> rusqlite::Result<bool> {
    let changed = connection.execute(
        "UPDATE composer_drafts
         SET message = ?1, updated_at = ?2
         WHERE scope_id = ?3 AND updated_at <= ?2",
        params![message, updated_at, scope_id],
    )?;
    if changed > 0 {
        return Ok(true);
    }

    connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM composer_drafts WHERE scope_id = ?1)",
        params![scope_id],
        |row| row.get(0),
    )
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
