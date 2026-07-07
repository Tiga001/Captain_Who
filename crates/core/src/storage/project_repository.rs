// Rust core storage.
use crate::storage::models::ProjectRecord;
use crate::storage::now_ms;
use rusqlite::{params, Connection, OptionalExtension};

pub fn list_projects(connection: &Connection) -> rusqlite::Result<Vec<ProjectRecord>> {
    let mut statement = connection.prepare(
        "
        SELECT id, name, path, created_at, pinned_at
        FROM projects
        ORDER BY COALESCE(pinned_at, 0) DESC, created_at ASC
        ",
    )?;

    let projects = statement
        .query_map([], |row| {
            Ok(ProjectRecord {
                id: row.get(0)?,
                name: row.get(1)?,
                path: row.get(2)?,
                created_at: row.get(3)?,
                pinned_at: row.get(4)?,
            })
        })?
        .collect();

    projects
}

pub fn get_project(
    connection: &Connection,
    project_id: &str,
) -> rusqlite::Result<Option<ProjectRecord>> {
    connection
        .query_row(
            "
            SELECT id, name, path, created_at, pinned_at
            FROM projects
            WHERE id = ?1
            ",
            params![project_id],
            |row| {
                Ok(ProjectRecord {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    path: row.get(2)?,
                    created_at: row.get(3)?,
                    pinned_at: row.get(4)?,
                })
            },
        )
        .optional()
}

pub fn get_project_by_path(
    connection: &Connection,
    path: &str,
) -> rusqlite::Result<Option<ProjectRecord>> {
    connection
        .query_row(
            "
            SELECT id, name, path, created_at, pinned_at
            FROM projects
            WHERE path = ?1
            ",
            params![path],
            |row| {
                Ok(ProjectRecord {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    path: row.get(2)?,
                    created_at: row.get(3)?,
                    pinned_at: row.get(4)?,
                })
            },
        )
        .optional()
}

pub fn save_project(connection: &Connection, project: ProjectRecord) -> rusqlite::Result<()> {
    let timestamp = now_ms();

    connection.execute(
        "
        INSERT INTO projects (id, name, path, created_at, pinned_at, updated_at)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6)
        ON CONFLICT(id) DO UPDATE SET
            name = excluded.name,
            path = excluded.path,
            pinned_at = excluded.pinned_at,
            updated_at = excluded.updated_at
        ",
        params![
            &project.id,
            &project.name,
            &project.path,
            project.created_at,
            project.pinned_at,
            timestamp
        ],
    )?;

    Ok(())
}

pub fn delete_project(connection: &Connection, project_id: &str) -> rusqlite::Result<()> {
    connection.execute(
        "
        DELETE FROM messages
        WHERE conversation_id IN (
            SELECT id
            FROM conversations
            WHERE project_id = ?1
        )
        ",
        params![project_id],
    )?;
    connection.execute(
        "DELETE FROM conversations WHERE project_id = ?1",
        params![project_id],
    )?;
    connection.execute("DELETE FROM projects WHERE id = ?1", params![project_id])?;
    Ok(())
}
