use super::common::{
    conflict, corrupt, immediate, positive_u64, read_error, validate_id, validate_time, write_error,
};
use super::nodes::ensure_active_agent;
use crate::{
    AgentBuiltinExecutionPermission, AgentCommandPermission, AgentCommandSafetyPolicy,
    AgentEffectivePermissionSnapshot, AgentGraphError, AgentPatchPermission, AgentPermissions,
    AgentReadPermission, AgentWritePermission, AGENT_EFFECTIVE_PERMISSION_SNAPSHOT_SCHEMA_VERSION,
};
use rusqlite::{params, Connection, OptionalExtension};

const EFFECTIVE_PERMISSION_SELECT: &str = "
    SELECT snapshot.agent_id, snapshot.schema_version, snapshot.root_agent_id,
           snapshot.conversation_id, snapshot.source_run_id,
           snapshot.source_assistant_message_id, snapshot.read_permission,
           snapshot.write_permission, snapshot.command_permission,
           snapshot.command_safety_policy, snapshot.patch_permission,
           snapshot.builtin_execution_permission, snapshot.revision, snapshot.created_at,
           snapshot.updated_at
    FROM agent_effective_permission_snapshots AS snapshot";

pub fn get_agent_effective_permission_snapshot(
    connection: &Connection,
    agent_id: &str,
) -> Result<Option<AgentEffectivePermissionSnapshot>, AgentGraphError> {
    validate_id("agent_id", agent_id)?;
    query_effective_permission_snapshot(connection, agent_id)
}

/// Persists the Host-authenticated permissions of an exact active Turn. This covers the lazy-root
/// case where the root node is materialized only while constructing collaboration Host services.
#[allow(clippy::too_many_arguments)]
pub fn record_agent_effective_permissions_for_active_turn(
    connection: &mut Connection,
    agent_id: &str,
    conversation_id: &str,
    run_id: &str,
    assistant_message_id: &str,
    permissions: AgentPermissions,
    updated_at: i64,
) -> Result<AgentEffectivePermissionSnapshot, AgentGraphError> {
    let transaction = immediate(connection)?;
    let snapshot = record_agent_effective_permissions_in_transaction(
        &transaction,
        agent_id,
        conversation_id,
        run_id,
        assistant_message_id,
        permissions,
        updated_at,
    )?;
    transaction.commit().map_err(write_error)?;
    Ok(snapshot)
}

/// Resolves a child permission set from the direct parent and every durable ancestor. The direct
/// parent is the inheritance baseline; older ancestor snapshots are dynamic ceilings which stop a
/// stale intermediate Agent from retaining authority after the root has tightened it.
pub(crate) fn inherit_agent_permissions_in_transaction(
    transaction: &Connection,
    child_agent_id: &str,
) -> Result<AgentPermissions, AgentGraphError> {
    validate_id("child_agent_id", child_agent_id)?;
    let child = ensure_active_agent(transaction, child_agent_id)?;
    let direct_parent_id = child
        .parent_agent_id
        .as_deref()
        .ok_or_else(|| conflict("root Agent cannot inherit child Wake permissions"))?;
    let mut statement = transaction
        .prepare(&format!(
            "WITH RECURSIVE ancestors(agent_id, parent_agent_id, depth) AS (
                 SELECT agent_id, parent_agent_id, 0
                 FROM agent_nodes WHERE agent_id = ?1
                 UNION ALL
                 SELECT parent.agent_id, parent.parent_agent_id, child.depth + 1
                 FROM agent_nodes AS parent
                 JOIN ancestors AS child ON parent.agent_id = child.parent_agent_id
             )
             {EFFECTIVE_PERMISSION_SELECT}
             JOIN ancestors ON ancestors.agent_id = snapshot.agent_id
             WHERE ancestors.depth > 0
             ORDER BY ancestors.depth"
        ))
        .map_err(read_error)?;
    let snapshots = statement
        .query_map([child_agent_id], read_effective_permission_row)
        .map_err(read_error)?
        .map(|row| {
            row.map_err(read_error)
                .and_then(decode_effective_permission_snapshot)
        })
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);

    let ancestor_count = transaction
        .query_row(
            "WITH RECURSIVE ancestors(agent_id, parent_agent_id) AS (
                 SELECT agent_id, parent_agent_id
                 FROM agent_nodes WHERE agent_id = ?1
                 UNION ALL
                 SELECT parent.agent_id, parent.parent_agent_id
                 FROM agent_nodes AS parent
                 JOIN ancestors AS child ON parent.agent_id = child.parent_agent_id
             )
             SELECT COUNT(*) - 1 FROM ancestors",
            [child_agent_id],
            |row| row.get::<_, i64>(0),
        )
        .map_err(read_error)?;
    if ancestor_count <= 0
        || snapshots.len() != usize::try_from(ancestor_count).unwrap_or(usize::MAX)
    {
        return Err(conflict(
            "trusted child permission inheritance is missing a durable ancestor snapshot",
        ));
    }
    let direct_parent = snapshots
        .first()
        .filter(|snapshot| snapshot.agent_id == direct_parent_id)
        .ok_or_else(|| corrupt("direct parent permission snapshot is missing or misordered"))?;
    if snapshots.iter().any(|snapshot| {
        snapshot.root_agent_id != child.root_agent_id || snapshot.conversation_id.is_empty()
    }) {
        return Err(corrupt(
            "ancestor permission snapshot crosses the child Agent root tree",
        ));
    }
    Ok(snapshots
        .iter()
        .skip(1)
        .fold(direct_parent.permissions, |effective, ancestor| {
            effective.meet(ancestor.permissions)
        }))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn record_agent_effective_permissions_in_transaction(
    transaction: &Connection,
    agent_id: &str,
    conversation_id: &str,
    run_id: &str,
    assistant_message_id: &str,
    permissions: AgentPermissions,
    updated_at: i64,
) -> Result<AgentEffectivePermissionSnapshot, AgentGraphError> {
    for (field, value) in [
        ("agent_id", agent_id),
        ("conversation_id", conversation_id),
        ("run_id", run_id),
        ("assistant_message_id", assistant_message_id),
    ] {
        validate_id(field, value)?;
    }
    validate_time(updated_at)?;
    let node = ensure_active_agent(transaction, agent_id)?;
    if node.conversation_id != conversation_id {
        return Err(conflict(
            "Agent effective permissions do not match the bound Conversation",
        ));
    }
    let exact_active_turn = transaction
        .query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM conversation_turn_traces
                 WHERE conversation_id = ?1 AND run_id = ?2
                   AND assistant_message_id = ?3 AND terminal_status = 'in_progress'
             )",
            params![conversation_id, run_id, assistant_message_id],
            |row| row.get::<_, bool>(0),
        )
        .map_err(read_error)?;
    if !exact_active_turn {
        return Err(conflict(
            "Agent effective permissions require the exact active Turn identity",
        ));
    }
    let existing = query_effective_permission_snapshot(transaction, agent_id)?;
    let changed = existing.as_ref().is_none_or(|snapshot| {
        snapshot.source_run_id != run_id
            || snapshot.source_assistant_message_id != assistant_message_id
            || snapshot.permissions != permissions
    });
    if changed {
        let created_at = existing
            .as_ref()
            .map_or(updated_at, |snapshot| snapshot.created_at);
        let next_updated_at = existing.as_ref().map_or(updated_at, |snapshot| {
            updated_at.max(snapshot.updated_at.saturating_add(1))
        });
        transaction
            .execute(
                "INSERT INTO agent_effective_permission_snapshots (
                     agent_id, schema_version, root_agent_id, conversation_id, source_run_id,
                     source_assistant_message_id, read_permission, write_permission,
                     command_permission, command_safety_policy, patch_permission,
                     builtin_execution_permission, revision, created_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 1, ?13, ?14)
                 ON CONFLICT(agent_id) DO UPDATE SET
                     source_run_id = excluded.source_run_id,
                     source_assistant_message_id = excluded.source_assistant_message_id,
                     read_permission = excluded.read_permission,
                     write_permission = excluded.write_permission,
                     command_permission = excluded.command_permission,
                     command_safety_policy = excluded.command_safety_policy,
                     patch_permission = excluded.patch_permission,
                     builtin_execution_permission = excluded.builtin_execution_permission,
                     revision = agent_effective_permission_snapshots.revision + 1,
                     updated_at = excluded.updated_at",
                params![
                    agent_id,
                    i64::from(AGENT_EFFECTIVE_PERMISSION_SNAPSHOT_SCHEMA_VERSION),
                    &node.root_agent_id,
                    conversation_id,
                    run_id,
                    assistant_message_id,
                    read_permission_as_str(permissions.read),
                    write_permission_as_str(permissions.write),
                    command_permission_as_str(permissions.command),
                    command_safety_as_str(permissions.command_safety),
                    patch_permission_as_str(permissions.patch),
                    builtin_execution_permission_as_str(permissions.builtin_execution),
                    created_at,
                    next_updated_at,
                ],
            )
            .map_err(write_error)?;
    }
    query_effective_permission_snapshot(transaction, agent_id)?
        .ok_or_else(|| corrupt("Agent effective permission snapshot disappeared after upsert"))
}

pub(super) fn query_effective_permission_snapshot(
    connection: &Connection,
    agent_id: &str,
) -> Result<Option<AgentEffectivePermissionSnapshot>, AgentGraphError> {
    connection
        .query_row(
            &format!("{EFFECTIVE_PERMISSION_SELECT} WHERE snapshot.agent_id = ?1"),
            [agent_id],
            read_effective_permission_row,
        )
        .optional()
        .map_err(read_error)?
        .map(decode_effective_permission_snapshot)
        .transpose()
}

pub(super) fn read_effective_permission_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<EffectivePermissionRow> {
    Ok(EffectivePermissionRow {
        agent_id: row.get(0)?,
        schema_version: row.get(1)?,
        root_agent_id: row.get(2)?,
        conversation_id: row.get(3)?,
        source_run_id: row.get(4)?,
        source_assistant_message_id: row.get(5)?,
        read_permission: row.get(6)?,
        write_permission: row.get(7)?,
        command_permission: row.get(8)?,
        command_safety_policy: row.get(9)?,
        patch_permission: row.get(10)?,
        builtin_execution_permission: row.get(11)?,
        revision: row.get(12)?,
        created_at: row.get(13)?,
        updated_at: row.get(14)?,
    })
}

pub(super) struct EffectivePermissionRow {
    agent_id: String,
    schema_version: i64,
    root_agent_id: String,
    conversation_id: String,
    source_run_id: String,
    source_assistant_message_id: String,
    read_permission: String,
    write_permission: String,
    command_permission: String,
    command_safety_policy: String,
    patch_permission: String,
    builtin_execution_permission: String,
    revision: i64,
    created_at: i64,
    updated_at: i64,
}

pub(super) fn decode_effective_permission_snapshot(
    row: EffectivePermissionRow,
) -> Result<AgentEffectivePermissionSnapshot, AgentGraphError> {
    if row.schema_version != i64::from(AGENT_EFFECTIVE_PERMISSION_SNAPSHOT_SCHEMA_VERSION) {
        return Err(corrupt(
            "unsupported Agent effective permission snapshot schema version",
        ));
    }
    Ok(AgentEffectivePermissionSnapshot {
        agent_id: row.agent_id,
        root_agent_id: row.root_agent_id,
        conversation_id: row.conversation_id,
        source_run_id: row.source_run_id,
        source_assistant_message_id: row.source_assistant_message_id,
        permissions: AgentPermissions {
            read: parse_read_permission(&row.read_permission)?,
            write: parse_write_permission(&row.write_permission)?,
            command: parse_command_permission(&row.command_permission)?,
            command_safety: parse_command_safety(&row.command_safety_policy)?,
            patch: parse_patch_permission(&row.patch_permission)?,
            builtin_execution: parse_builtin_execution_permission(
                &row.builtin_execution_permission,
            )?,
        },
        revision: positive_u64(row.revision, "Agent effective permission revision")?,
        created_at: row.created_at,
        updated_at: row.updated_at,
    })
}

pub(super) fn read_permission_as_str(value: AgentReadPermission) -> &'static str {
    match value {
        AgentReadPermission::WorkspaceOnly => "workspace_only",
        AgentReadPermission::All => "all",
    }
}

pub(super) fn parse_read_permission(value: &str) -> Result<AgentReadPermission, AgentGraphError> {
    match value {
        "workspace_only" => Ok(AgentReadPermission::WorkspaceOnly),
        "all" => Ok(AgentReadPermission::All),
        _ => Err(corrupt("unknown effective read permission")),
    }
}

pub(super) fn write_permission_as_str(value: AgentWritePermission) -> &'static str {
    match value {
        AgentWritePermission::Denied => "denied",
        AgentWritePermission::WorkspaceOnly => "workspace_only",
        AgentWritePermission::All => "all",
    }
}

pub(super) fn parse_write_permission(value: &str) -> Result<AgentWritePermission, AgentGraphError> {
    match value {
        "denied" => Ok(AgentWritePermission::Denied),
        "workspace_only" => Ok(AgentWritePermission::WorkspaceOnly),
        "all" => Ok(AgentWritePermission::All),
        _ => Err(corrupt("unknown effective write permission")),
    }
}

pub(super) fn command_permission_as_str(value: AgentCommandPermission) -> &'static str {
    match value {
        AgentCommandPermission::RequireApproval => "require_approval",
        AgentCommandPermission::AutoApprove => "auto_approve",
    }
}

pub(super) fn parse_command_permission(
    value: &str,
) -> Result<AgentCommandPermission, AgentGraphError> {
    match value {
        "require_approval" => Ok(AgentCommandPermission::RequireApproval),
        "auto_approve" => Ok(AgentCommandPermission::AutoApprove),
        _ => Err(corrupt("unknown effective command permission")),
    }
}

pub(super) fn command_safety_as_str(value: AgentCommandSafetyPolicy) -> &'static str {
    match value {
        AgentCommandSafetyPolicy::Guarded => "guarded",
        AgentCommandSafetyPolicy::FullAccess => "full_access",
    }
}

pub(super) fn parse_command_safety(
    value: &str,
) -> Result<AgentCommandSafetyPolicy, AgentGraphError> {
    match value {
        "guarded" => Ok(AgentCommandSafetyPolicy::Guarded),
        "full_access" => Ok(AgentCommandSafetyPolicy::FullAccess),
        _ => Err(corrupt("unknown effective command safety policy")),
    }
}

pub(super) fn patch_permission_as_str(value: AgentPatchPermission) -> &'static str {
    match value {
        AgentPatchPermission::RequireApproval => "require_approval",
        AgentPatchPermission::AutoApprove => "auto_approve",
    }
}

pub(super) fn parse_patch_permission(value: &str) -> Result<AgentPatchPermission, AgentGraphError> {
    match value {
        "require_approval" => Ok(AgentPatchPermission::RequireApproval),
        "auto_approve" => Ok(AgentPatchPermission::AutoApprove),
        _ => Err(corrupt("unknown effective patch permission")),
    }
}

pub(super) fn builtin_execution_permission_as_str(
    value: AgentBuiltinExecutionPermission,
) -> &'static str {
    match value {
        AgentBuiltinExecutionPermission::RequireApproval => "require_approval",
        AgentBuiltinExecutionPermission::AutoApprove => "auto_approve",
    }
}

pub(super) fn parse_builtin_execution_permission(
    value: &str,
) -> Result<AgentBuiltinExecutionPermission, AgentGraphError> {
    match value {
        "require_approval" => Ok(AgentBuiltinExecutionPermission::RequireApproval),
        "auto_approve" => Ok(AgentBuiltinExecutionPermission::AutoApprove),
        _ => Err(corrupt("unknown effective built-in execution permission")),
    }
}
