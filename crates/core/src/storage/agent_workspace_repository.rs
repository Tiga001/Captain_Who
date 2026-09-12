//! Host-owned immutable workspace membership for a Run and every Wake it delegates.
use crate::{
    workspace::{capture_project_workspace, WorkspaceResolver},
    AgentWorkspaceContext,
};
use rusqlite::{params, Connection, OptionalExtension};

fn decode(json: String) -> rusqlite::Result<Option<AgentWorkspaceContext>> {
    let workspace: Option<AgentWorkspaceContext> = serde_json::from_str(&json)
        .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
    WorkspaceResolver::from_context(workspace.as_ref())
        .validate_shape()
        .map_err(rusqlite::Error::InvalidParameterName)?;
    Ok(workspace)
}

pub(crate) fn load_run(
    connection: &Connection,
    run_id: &str,
) -> rusqlite::Result<Option<Option<AgentWorkspaceContext>>> {
    connection
        .query_row(
            "SELECT workspace_json FROM agent_workspace_run_bindings WHERE run_id=?1",
            [run_id],
            |r| r.get(0),
        )
        .optional()?
        .map(decode)
        .transpose()
}

pub(crate) fn load_wake(
    connection: &Connection,
    wake_id: &str,
) -> rusqlite::Result<Option<Option<AgentWorkspaceContext>>> {
    connection
        .query_row(
            "SELECT workspace_json FROM agent_workspace_wake_bindings WHERE wake_id=?1",
            [wake_id],
            |r| r.get(0),
        )
        .optional()?
        .map(decode)
        .transpose()
}

pub(crate) fn freeze_run(
    connection: &Connection,
    run_id: &str,
    trusted_wake_id: Option<&str>,
    project_id: Option<&str>,
) -> rusqlite::Result<Option<AgentWorkspaceContext>> {
    if let Some(workspace) = load_run(connection, run_id)? {
        return Ok(workspace);
    }
    let inherited = trusted_wake_id
        .map(|id| {
            load_wake(connection, id)?.ok_or_else(|| {
                rusqlite::Error::InvalidParameterName(
                    "Trusted Agent wake has no frozen workspace".into(),
                )
            })
        })
        .transpose()?;
    let workspace = match inherited {
        Some(workspace) => workspace,
        None => project_id
            .map(|id| super::project_repository::load_project(connection, id))
            .transpose()?
            .flatten()
            .as_ref()
            .map(capture_project_workspace)
            .transpose()
            .map_err(rusqlite::Error::InvalidParameterName)?,
    };
    let json = serde_json::to_string(&workspace)
        .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
    connection.execute(
        "INSERT INTO agent_workspace_run_bindings(run_id,workspace_json) VALUES (?1,?2)",
        params![run_id, json],
    )?;
    Ok(workspace)
}

pub(crate) fn inherit_run_for_wake(
    connection: &Connection,
    run_id: &str,
    wake_id: &str,
) -> rusqlite::Result<()> {
    let workspace = load_run(connection, run_id)?.ok_or_else(|| {
        rusqlite::Error::InvalidParameterName("Originating Run has no frozen workspace".into())
    })?;
    persist_wake(connection, wake_id, workspace)?;
    Ok(())
}

pub(crate) fn inherit_wake_for_wake(
    connection: &Connection,
    source_wake_id: &str,
    wake_id: &str,
) -> rusqlite::Result<()> {
    let workspace = load_wake(connection, source_wake_id)?.ok_or_else(|| {
        rusqlite::Error::InvalidParameterName("Originating Wake has no frozen workspace".into())
    })?;
    persist_wake(connection, wake_id, workspace)?;
    Ok(())
}

fn persist_wake(
    connection: &Connection,
    wake_id: &str,
    workspace: Option<AgentWorkspaceContext>,
) -> rusqlite::Result<()> {
    let json = serde_json::to_string(&workspace)
        .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
    connection.execute("INSERT INTO agent_workspace_wake_bindings(wake_id,workspace_json) VALUES (?1,?2) ON CONFLICT(wake_id) DO NOTHING",params![wake_id,json])?;
    if load_wake(connection, wake_id)? != Some(workspace) {
        return Err(rusqlite::Error::InvalidParameterName(
            "Agent wake workspace does not match its originating run".into(),
        ));
    }
    Ok(())
}

/// Explicit Host-created tasks without a source Run freeze settings at creation, never at resume.
pub(crate) fn capture_host_wake(
    connection: &Connection,
    wake_id: &str,
    project_id: Option<&str>,
) -> rusqlite::Result<()> {
    if load_wake(connection, wake_id)?.is_some() {
        return Ok(());
    }
    let workspace = project_id
        .map(|id| super::project_repository::load_project(connection, id))
        .transpose()?
        .flatten()
        .as_ref()
        .map(capture_project_workspace)
        .transpose()
        .map_err(rusqlite::Error::InvalidParameterName)?;
    persist_wake(connection, wake_id, workspace)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_trusted_workspace_bindings_never_fall_back_to_project_settings() {
        let connection = Connection::open_in_memory().unwrap();
        crate::storage::migrations::run_migrations(&connection).unwrap();
        assert!(
            freeze_run(&connection, "new-run", Some("missing-wake"), None)
                .unwrap_err()
                .to_string()
                .contains("no frozen workspace")
        );
        assert!(inherit_run_for_wake(&connection, "missing-run", "new-wake").is_err());
        assert!(inherit_wake_for_wake(&connection, "missing-wake", "new-wake").is_err());
        let count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM agent_workspace_run_bindings",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
    }
}
