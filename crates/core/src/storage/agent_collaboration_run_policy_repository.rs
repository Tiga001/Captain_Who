//! Host-only collaboration policy bindings for a logical run and its delegated wakes.
//!
//! Settings updates never rewrite these records. Admission and wake creation copy the policy
//! under the same transaction as their execution identity, so queued descendants and approval
//! recovery retain the capability with which their tree started.

use crate::AgentCollaborationSettings;
use rusqlite::{params, Connection, OptionalExtension};

pub(crate) fn load_run(
    connection: &Connection,
    run_id: &str,
) -> rusqlite::Result<Option<AgentCollaborationSettings>> {
    connection
        .query_row(
            "SELECT enabled, revision, updated_at FROM agent_collaboration_run_policies WHERE run_id = ?1",
            [run_id],
            read_policy,
        )
        .optional()
}

fn read_policy(row: &rusqlite::Row<'_>) -> rusqlite::Result<AgentCollaborationSettings> {
    Ok(AgentCollaborationSettings {
        enabled: row.get(0)?,
        revision: row.get(1)?,
        updated_at: row.get(2)?,
    })
}

/// Called after the new trace exists, inside the atomic Turn-admission transaction.
pub(crate) fn freeze_run(
    connection: &Connection,
    run_id: &str,
    trusted_wake_id: Option<&str>,
) -> rusqlite::Result<AgentCollaborationSettings> {
    if let Some(policy) = load_run(connection, run_id)? {
        return Ok(policy);
    }
    let inherited = trusted_wake_id
        .map(|wake_id| {
            connection
                .query_row(
                    "SELECT enabled, revision, updated_at FROM agent_collaboration_wake_policies WHERE wake_id = ?1",
                    [wake_id],
                    read_policy,
                )
                .optional()
        })
        .transpose()?
        .flatten();
    // Direct Host-created wakes have no source runtime. They begin a new policy scope just as
    // human/automation turns do; model-created wakes carry the originating run's exact policy.
    let policy = match inherited {
        Some(policy) => policy,
        None => connection.query_row(
            "SELECT enabled, revision, updated_at FROM agent_collaboration_settings WHERE singleton = 1",
            [],
            read_policy,
        )?,
    };
    connection.execute(
        "INSERT INTO agent_collaboration_run_policies(run_id, enabled, revision, updated_at) VALUES (?1, ?2, ?3, ?4)",
        params![run_id, policy.enabled, policy.revision, policy.updated_at],
    )?;
    Ok(policy)
}

/// Copies a runtime policy before the wake transaction commits. Repository-only Host callers
/// may have no runtime policy; actual model execution requires one at the harness boundary.
pub(crate) fn inherit_run_for_wake(
    connection: &Connection,
    run_id: &str,
    wake_id: &str,
) -> rusqlite::Result<()> {
    let Some(policy) = load_run(connection, run_id)? else {
        return Ok(());
    };
    persist_wake_policy(connection, wake_id, &policy)
}

/// Pre-admission failures can report back before a child run exists. Preserve the policy carried
/// by that queued wake when its result schedules a parent continuation.
pub(crate) fn inherit_wake_for_wake(
    connection: &Connection,
    source_wake_id: &str,
    wake_id: &str,
) -> rusqlite::Result<()> {
    let policy = connection.query_row(
        "SELECT enabled, revision, updated_at FROM agent_collaboration_wake_policies WHERE wake_id = ?1",
        [source_wake_id], read_policy,
    ).optional()?;
    if let Some(policy) = policy {
        persist_wake_policy(connection, wake_id, &policy)?;
    }
    Ok(())
}

fn persist_wake_policy(
    connection: &Connection,
    wake_id: &str,
    policy: &AgentCollaborationSettings,
) -> rusqlite::Result<()> {
    connection.execute(
        "INSERT INTO agent_collaboration_wake_policies(wake_id, enabled, revision, updated_at)
         VALUES (?1, ?2, ?3, ?4) ON CONFLICT(wake_id) DO NOTHING",
        params![wake_id, policy.enabled, policy.revision, policy.updated_at],
    )?;
    let persisted = connection.query_row(
        "SELECT enabled, revision, updated_at FROM agent_collaboration_wake_policies WHERE wake_id = ?1",
        [wake_id],
        read_policy,
    )?;
    if &persisted != policy {
        return Err(rusqlite::Error::InvalidParameterName(
            "Agent wake collaboration policy does not match its originating run".into(),
        ));
    }
    Ok(())
}
