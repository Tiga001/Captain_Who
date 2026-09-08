//! Immutable context-mode bindings for admitted runs and their delegated wakes.
//! Global preferences affect a new root run; an existing task tree retains its originating mode.

use crate::AgentContextProfile;
use rusqlite::{params, Connection, OptionalExtension};

fn read_profile(row: &rusqlite::Row<'_>) -> rusqlite::Result<AgentContextProfile> {
    match row.get::<_, String>(0)?.as_str() {
        "full" => Ok(AgentContextProfile::Full),
        "minimal" => Ok(AgentContextProfile::Minimal),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}

fn profile_name(profile: AgentContextProfile) -> &'static str {
    match profile {
        AgentContextProfile::Full => "full",
        AgentContextProfile::Minimal => "minimal",
    }
}

pub(crate) fn load_run(
    connection: &Connection,
    run_id: &str,
) -> rusqlite::Result<Option<AgentContextProfile>> {
    connection
        .query_row(
            "SELECT context_profile FROM agent_context_profile_run_policies WHERE run_id=?1",
            [run_id],
            read_profile,
        )
        .optional()
}

fn load_wake(
    connection: &Connection,
    wake_id: &str,
) -> rusqlite::Result<Option<AgentContextProfile>> {
    connection
        .query_row(
            "SELECT context_profile FROM agent_context_profile_wake_policies WHERE wake_id=?1",
            [wake_id],
            read_profile,
        )
        .optional()
}

/// Must run in the same transaction as trace admission, after the trace's foreign key exists.
pub(crate) fn freeze_run(
    connection: &Connection,
    run_id: &str,
    trusted_wake_id: Option<&str>,
) -> rusqlite::Result<AgentContextProfile> {
    if let Some(profile) = load_run(connection, run_id)? {
        return Ok(profile);
    }
    let inherited = trusted_wake_id
        .map(|id| load_wake(connection, id))
        .transpose()?
        .flatten();
    let profile = match inherited {
        Some(profile) => profile,
        None => connection
            .query_row(
                "SELECT context_profile FROM agent_prompt_preferences WHERE id='default'",
                [],
                read_profile,
            )
            .optional()?
            .unwrap_or_default(),
    };
    connection.execute(
        "INSERT INTO agent_context_profile_run_policies(run_id,context_profile) VALUES (?1,?2)",
        params![run_id, profile_name(profile)],
    )?;
    Ok(profile)
}

pub(crate) fn inherit_run_for_wake(
    connection: &Connection,
    run_id: &str,
    wake_id: &str,
) -> rusqlite::Result<()> {
    if let Some(profile) = load_run(connection, run_id)? {
        persist_wake(connection, wake_id, profile)?;
    }
    Ok(())
}

pub(crate) fn inherit_wake_for_wake(
    connection: &Connection,
    source_wake_id: &str,
    wake_id: &str,
) -> rusqlite::Result<()> {
    if let Some(profile) = load_wake(connection, source_wake_id)? {
        persist_wake(connection, wake_id, profile)?;
    }
    Ok(())
}

fn persist_wake(
    connection: &Connection,
    wake_id: &str,
    profile: AgentContextProfile,
) -> rusqlite::Result<()> {
    connection.execute(
        "INSERT INTO agent_context_profile_wake_policies(wake_id,context_profile) VALUES (?1,?2)
         ON CONFLICT(wake_id) DO NOTHING",
        params![wake_id, profile_name(profile)],
    )?;
    if load_wake(connection, wake_id)? != Some(profile) {
        return Err(rusqlite::Error::InvalidParameterName(
            "Agent wake context profile does not match its originating run".into(),
        ));
    }
    Ok(())
}
