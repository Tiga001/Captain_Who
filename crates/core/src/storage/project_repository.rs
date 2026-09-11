use crate::storage::models::{ProjectFolderRecord, ProjectFolderRole, ProjectRecord};
use crate::storage::now_ms;
use rusqlite::{params, Connection};
use std::collections::HashMap;

pub fn project_exists(connection: &Connection, project_id: &str) -> rusqlite::Result<bool> {
    connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM projects WHERE id = ?1)",
        params![project_id],
        |row| row.get(0),
    )
}

pub fn list_projects(connection: &Connection) -> rusqlite::Result<Vec<ProjectRecord>> {
    let mut folders_by_project = list_folders_by_project(connection)?;
    let mut statement = connection.prepare(
        "
        SELECT id, name, created_at, pinned_at
        FROM projects
        ORDER BY COALESCE(pinned_at, 0) DESC, created_at ASC
        ",
    )?;

    let projects = statement
        .query_map([], |row| {
            let id: String = row.get(0)?;
            let folders = folders_by_project.remove(&id).unwrap_or_default();
            Ok(ProjectRecord {
                id,
                name: row.get(1)?,
                folders,
                created_at: row.get(2)?,
                pinned_at: row.get(3)?,
            })
        })?
        .collect();

    projects
}

pub fn load_project(
    connection: &Connection,
    project_id: &str,
) -> rusqlite::Result<Option<ProjectRecord>> {
    let mut statement = connection.prepare(
        "
        SELECT id, name, created_at, pinned_at
        FROM projects
        WHERE id = ?1
        ",
    )?;
    let mut rows = statement.query(params![project_id])?;
    let Some(row) = rows.next()? else {
        return Ok(None);
    };
    let id: String = row.get(0)?;
    let name: String = row.get(1)?;
    let created_at: i64 = row.get(2)?;
    let pinned_at: Option<i64> = row.get(3)?;
    let folders = list_project_folders(connection, project_id)?;
    Ok(Some(ProjectRecord {
        id,
        name,
        folders,
        created_at,
        pinned_at,
    }))
}

/// Returns the primary folder path of a project without loading the full record.
pub fn primary_project_path(
    connection: &Connection,
    project_id: &str,
) -> rusqlite::Result<Option<String>> {
    let mut statement = connection.prepare(
        "
        SELECT path
        FROM project_folders
        WHERE project_id = ?1 AND role = 'primary'
        LIMIT 1
        ",
    )?;
    let mut rows = statement.query(params![project_id])?;
    match rows.next()? {
        Some(row) => Ok(Some(row.get(0)?)),
        None => Ok(None),
    }
}

pub fn list_project_folders(
    connection: &Connection,
    project_id: &str,
) -> rusqlite::Result<Vec<ProjectFolderRecord>> {
    let mut statement = connection.prepare(
        "
        SELECT id, path, alias, role, sort_order, created_at
        FROM project_folders
        WHERE project_id = ?1
        ORDER BY sort_order ASC, created_at ASC, id ASC
        ",
    )?;
    let folders = statement
        .query_map(params![project_id], read_folder_row)?
        .collect();
    folders
}

fn list_folders_by_project(
    connection: &Connection,
) -> rusqlite::Result<HashMap<String, Vec<ProjectFolderRecord>>> {
    let mut statement = connection.prepare(
        "
        SELECT project_id, id, path, alias, role, sort_order, created_at
        FROM project_folders
        ORDER BY project_id ASC, sort_order ASC, created_at ASC, id ASC
        ",
    )?;
    let mut rows = statement.query([])?;
    let mut folders_by_project: HashMap<String, Vec<ProjectFolderRecord>> = HashMap::new();
    while let Some(row) = rows.next()? {
        let project_id: String = row.get(0)?;
        let role_text: String = row.get(4)?;
        let folder = ProjectFolderRecord {
            id: row.get(1)?,
            path: row.get(2)?,
            alias: row.get(3)?,
            role: parse_role(&role_text, 4)?,
            sort_order: row.get(5)?,
            created_at: row.get(6)?,
        };
        folders_by_project
            .entry(project_id)
            .or_default()
            .push(folder);
    }
    Ok(folders_by_project)
}

fn read_folder_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ProjectFolderRecord> {
    let role_text: String = row.get(3)?;
    Ok(ProjectFolderRecord {
        id: row.get(0)?,
        path: row.get(1)?,
        alias: row.get(2)?,
        role: parse_role(&role_text, 3)?,
        sort_order: row.get(4)?,
        created_at: row.get(5)?,
    })
}

fn parse_role(value: &str, column: usize) -> rusqlite::Result<ProjectFolderRole> {
    ProjectFolderRole::parse(value).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            column,
            rusqlite::types::Type::Text,
            format!("unknown project folder role {value}").into(),
        )
    })
}

/// Structural validation error raised before any project write happens.
pub fn invalid_project_error(detail: String) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(detail.into())
}

/// Upserts the project row and replaces its folder set in a single transaction. The whole
/// write is rejected before touching the database when the folder set is structurally invalid.
pub fn save_project(connection: &Connection, project: ProjectRecord) -> rusqlite::Result<()> {
    project.validate_folders().map_err(invalid_project_error)?;
    let timestamp = now_ms();
    let transaction = connection.unchecked_transaction()?;

    transaction.execute(
        "
        INSERT INTO projects (id, name, created_at, pinned_at, updated_at)
        VALUES (?1, ?2, ?3, ?4, ?5)
        ON CONFLICT(id) DO UPDATE SET
            name = excluded.name,
            pinned_at = excluded.pinned_at,
            updated_at = excluded.updated_at
        ",
        params![
            &project.id,
            &project.name,
            project.created_at,
            project.pinned_at,
            timestamp
        ],
    )?;

    // The caller owns folder identity (ids and created_at are carried over on edits), so the
    // folder set is rewritten wholesale inside the transaction. This keeps the unique
    // (project_id, path) / (project_id, alias) constraints satisfied even when folders swap
    // paths, aliases or the primary role among themselves.
    transaction.execute(
        "DELETE FROM project_folders WHERE project_id = ?1",
        params![&project.id],
    )?;
    let mut insert = transaction.prepare(
        "
        INSERT INTO project_folders (id, project_id, path, alias, role, sort_order, created_at)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
        ",
    )?;
    for folder in &project.folders {
        insert.execute(params![
            &folder.id,
            &project.id,
            &folder.path,
            &folder.alias,
            folder.role.as_str(),
            folder.sort_order,
            folder.created_at,
        ])?;
    }
    drop(insert);

    transaction.commit()
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
    connection.execute(
        "DELETE FROM project_folders WHERE project_id = ?1",
        params![project_id],
    )?;
    connection.execute("DELETE FROM projects WHERE id = ?1", params![project_id])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::migrations::run_migrations;

    fn connection() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        run_migrations(&connection).unwrap();
        connection
    }

    fn folder(
        id: &str,
        path: &str,
        alias: &str,
        role: ProjectFolderRole,
        sort_order: i64,
    ) -> ProjectFolderRecord {
        ProjectFolderRecord {
            id: id.to_string(),
            path: path.to_string(),
            alias: alias.to_string(),
            role,
            sort_order,
            created_at: 5,
        }
    }

    fn multi_folder_project() -> ProjectRecord {
        ProjectRecord {
            id: "project-multi".to_string(),
            name: "Multi".to_string(),
            folders: vec![
                folder(
                    "f-main",
                    "/repos/main",
                    "main",
                    ProjectFolderRole::Primary,
                    0,
                ),
                folder(
                    "f-docs",
                    "/repos/docs",
                    "docs",
                    ProjectFolderRole::Auxiliary,
                    1,
                ),
            ],
            created_at: 3,
            pinned_at: None,
        }
    }

    #[test]
    fn saves_and_lists_projects_with_ordered_folders() {
        let connection = connection();
        save_project(&connection, multi_folder_project()).unwrap();
        save_project(
            &connection,
            ProjectRecord::without_folders("project-empty", "Empty", 9),
        )
        .unwrap();
        save_project(
            &connection,
            ProjectRecord::with_primary_folder("project-single", "Single", "/repos/single", 1),
        )
        .unwrap();

        let projects = list_projects(&connection).unwrap();
        let ids: Vec<_> = projects.iter().map(|project| project.id.as_str()).collect();
        assert_eq!(ids, ["project-single", "project-multi", "project-empty"]);
        let multi = &projects[1];
        assert_eq!(multi.primary_path(), Some("/repos/main"));
        assert_eq!(
            multi
                .auxiliary_folders()
                .map(|folder| folder.alias.as_str())
                .collect::<Vec<_>>(),
            ["docs"]
        );
        assert!(projects[2].folders.is_empty());
        assert_eq!(projects[2].primary_path(), None);
        assert_eq!(
            projects[0].folders,
            vec![ProjectFolderRecord {
                id: "project-single-primary".to_string(),
                path: "/repos/single".to_string(),
                alias: "single".to_string(),
                role: ProjectFolderRole::Primary,
                sort_order: 0,
                created_at: 1,
            }]
        );
        assert_eq!(
            primary_project_path(&connection, "project-multi").unwrap(),
            Some("/repos/main".to_string())
        );
        assert_eq!(
            primary_project_path(&connection, "project-empty").unwrap(),
            None
        );
        assert_eq!(
            load_project(&connection, "project-multi")
                .unwrap()
                .unwrap()
                .folders
                .len(),
            2
        );
        assert!(load_project(&connection, "missing").unwrap().is_none());
    }

    #[test]
    fn resaving_replaces_folders_and_allows_swapping_the_primary_role() {
        let connection = connection();
        let mut project = multi_folder_project();
        save_project(&connection, project.clone()).unwrap();

        project.folders[0].role = ProjectFolderRole::Auxiliary;
        project.folders[1].role = ProjectFolderRole::Primary;
        project.folders[1].sort_order = 0;
        project.folders[0].sort_order = 1;
        project.folders.push(folder(
            "f-tools",
            "/repos/tools",
            "tools",
            ProjectFolderRole::Auxiliary,
            2,
        ));
        project.name = "Renamed".to_string();
        save_project(&connection, project).unwrap();

        let stored = load_project(&connection, "project-multi").unwrap().unwrap();
        assert_eq!(stored.name, "Renamed");
        assert_eq!(stored.primary_path(), Some("/repos/docs"));
        assert_eq!(
            stored
                .folders
                .iter()
                .map(|folder| folder.id.as_str())
                .collect::<Vec<_>>(),
            ["f-docs", "f-main", "f-tools"]
        );

        let trimmed = ProjectRecord {
            folders: vec![folder(
                "f-tools",
                "/repos/tools",
                "tools",
                ProjectFolderRole::Primary,
                0,
            )],
            ..stored
        };
        save_project(&connection, trimmed).unwrap();
        let stored = load_project(&connection, "project-multi").unwrap().unwrap();
        assert_eq!(stored.folders.len(), 1);
        assert_eq!(stored.primary_path(), Some("/repos/tools"));
    }

    #[test]
    fn two_folders_may_exchange_paths_and_aliases_in_one_save() {
        let connection = connection();
        let mut project = multi_folder_project();
        save_project(&connection, project.clone()).unwrap();
        project.folders[0].path = "/repos/docs".to_string();
        project.folders[0].alias = "docs".to_string();
        project.folders[1].path = "/repos/main".to_string();
        project.folders[1].alias = "main".to_string();
        save_project(&connection, project).unwrap();
        let stored = load_project(&connection, "project-multi").unwrap().unwrap();
        assert_eq!(stored.folders[0].id, "f-main");
        assert_eq!(stored.folders[0].path, "/repos/docs");
        assert_eq!(stored.folders[1].id, "f-docs");
        assert_eq!(stored.folders[1].alias, "main");
    }

    #[test]
    fn structurally_invalid_folder_sets_are_rejected_without_writing() {
        let connection = connection();
        save_project(&connection, multi_folder_project()).unwrap();

        let mut no_primary = multi_folder_project();
        no_primary.folders[0].role = ProjectFolderRole::Auxiliary;
        let mut two_primaries = multi_folder_project();
        two_primaries.folders[1].role = ProjectFolderRole::Primary;
        let mut duplicate_path = multi_folder_project();
        duplicate_path.folders[1].path = "/repos/main".to_string();
        let mut duplicate_alias = multi_folder_project();
        duplicate_alias.folders[1].alias = "main".to_string();
        let mut blank_alias = multi_folder_project();
        blank_alias.folders[1].alias = "  ".to_string();
        let mut duplicate_id = multi_folder_project();
        duplicate_id.folders[1].id = "f-main".to_string();

        for (label, invalid) in [
            ("no primary", no_primary),
            ("two primaries", two_primaries),
            ("duplicate path", duplicate_path),
            ("duplicate alias", duplicate_alias),
            ("blank alias", blank_alias),
            ("duplicate id", duplicate_id),
        ] {
            let mut invalid = invalid;
            invalid.name = format!("changed by {label}");
            assert!(
                save_project(&connection, invalid).is_err(),
                "{label} must be rejected"
            );
            let stored = load_project(&connection, "project-multi").unwrap().unwrap();
            assert_eq!(stored.name, "Multi", "{label} must not rename the project");
            assert_eq!(stored.folders, multi_folder_project().folders);
        }
    }

    #[test]
    fn deleting_a_project_removes_its_folders() {
        let connection = connection();
        save_project(&connection, multi_folder_project()).unwrap();
        delete_project(&connection, "project-multi").unwrap();
        assert!(list_projects(&connection).unwrap().is_empty());
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM project_folders", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
}
