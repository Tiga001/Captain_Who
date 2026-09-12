use crate::file_change::{
    derive_file_change_run_grant_scope, FileChangeDirectoryIdentity, FileChangeRunGrantRecord,
    FileChangeRunGrantScopeKind, FileChangeRunGrantStatus,
};
use crate::storage::pending_action_repository;
use crate::{AgentRunCheckpoint, AgentRunContext, AgentWritePermission};
use rusqlite::{params, Connection, OptionalExtension};

/// Exact current Host resume envelope required by the Run-grant authority projection.
///
/// This is not a compatibility decoder. A schema bump must update this strict authority check in
/// the same change as `PersistedAgentResumeInput`; an unknown envelope makes the grant inert.
const CURRENT_PERSISTED_AGENT_RESUME_INPUT_SCHEMA_VERSION: u64 = 14;

pub fn insert_pending_run_grant(
    connection: &Connection,
    grant: &FileChangeRunGrantRecord,
) -> rusqlite::Result<bool> {
    validate_record(grant)?;
    if grant.status != FileChangeRunGrantStatus::Pending {
        return Err(invalid("new FileChange run grant is not pending"));
    }
    let changed = connection.execute(
        r#"
        INSERT OR IGNORE INTO agent_file_change_run_grants (
            schema_version, grant_id, revision, status, run_id, conversation_id, project_id,
            scope_kind, workspace_identity, canonical_scope_path, scope_directory_identity_json,
            granting_pending_action_id, base_write_permission, granting_permission_revision,
            granting_tool_set_revision, granting_provider_wire_revision,
            apply_patch_contract_revision, activation_result_digest, created_at,
            activated_at, inactive_at, revoked_at
        ) VALUES (?1, ?2, ?3, 'pending', ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
                  ?14, ?15, ?16, NULL, ?17, NULL, NULL, NULL)
        "#,
        params![
            grant.schema_version,
            grant.grant_id,
            grant.revision,
            grant.run_id,
            grant.conversation_id,
            grant.project_id,
            scope_kind_label(grant.scope_kind),
            grant.workspace_identity,
            grant.canonical_scope_path,
            serialize_directory_identity(&grant.scope_directory_identity)?,
            grant.granting_pending_action_id,
            write_permission_label(grant.base_write_permission),
            grant.granting_permission_revision,
            grant.granting_tool_set_revision,
            grant.granting_provider_wire_revision,
            grant.apply_patch_contract_revision,
            grant.created_at,
        ],
    )?;
    if changed == 1 {
        return Ok(true);
    }
    let existing = get_run_grant_for_pending_action(connection, &grant.granting_pending_action_id)?
        .ok_or_else(|| invalid("FileChange run grant insert was ignored without an exact row"))?;
    if existing.status == FileChangeRunGrantStatus::Pending
        && existing.run_id == grant.run_id
        && existing.conversation_id == grant.conversation_id
        && existing.project_id == grant.project_id
        && existing.scope_kind == grant.scope_kind
        && existing.workspace_identity == grant.workspace_identity
        && existing.canonical_scope_path == grant.canonical_scope_path
        && existing.scope_directory_identity == grant.scope_directory_identity
        && existing.granting_pending_action_id == grant.granting_pending_action_id
        && existing.base_write_permission == grant.base_write_permission
        && existing.granting_permission_revision == grant.granting_permission_revision
        && existing.granting_tool_set_revision == grant.granting_tool_set_revision
        && existing.granting_provider_wire_revision == grant.granting_provider_wire_revision
        && existing.apply_patch_contract_revision == grant.apply_patch_contract_revision
        && existing.activation_result_digest.is_none()
    {
        Ok(false)
    } else {
        Err(invalid(
            "FileChange run grant intent conflicts with durable state",
        ))
    }
}

pub fn get_run_grant_for_pending_action(
    connection: &Connection,
    pending_action_id: &str,
) -> rusqlite::Result<Option<FileChangeRunGrantRecord>> {
    connection
        .query_row(
            &format!("{} WHERE granting_pending_action_id = ?1", select_sql()),
            [pending_action_id],
            map_record,
        )
        .optional()
}

fn get_active_run_grant_record(
    connection: &Connection,
    run_id: &str,
) -> rusqlite::Result<Option<FileChangeRunGrantRecord>> {
    connection
        .query_row(
            &format!("{} WHERE run_id = ?1 AND status = 'active'", select_sql()),
            [run_id],
            map_record,
        )
        .optional()
}

/// Loads an active grant only while its exact granting Pending Action and terminal receipt remain
/// present and mutually consistent. An orphan or tampered row is inert authority.
pub fn get_active_run_grant(
    connection: &Connection,
    run_id: &str,
) -> rusqlite::Result<Option<FileChangeRunGrantRecord>> {
    let Some(grant) = get_active_run_grant_record(connection, run_id)? else {
        return Ok(None);
    };
    if active_grant_has_authoritative_receipt(connection, &grant)? {
        Ok(Some(grant))
    } else {
        Ok(None)
    }
}

pub fn list_active_run_grant_records(
    connection: &Connection,
) -> rusqlite::Result<Vec<FileChangeRunGrantRecord>> {
    let mut statement = connection.prepare(&format!(
        "{} WHERE status = 'active' ORDER BY run_id, grant_id",
        select_sql()
    ))?;
    let records = statement.query_map([], map_record)?.collect();
    records
}

pub fn list_pending_run_grant_records(
    connection: &Connection,
) -> rusqlite::Result<Vec<FileChangeRunGrantRecord>> {
    let mut statement = connection.prepare(&format!(
        "{} WHERE status = 'pending' ORDER BY run_id, grant_id",
        select_sql()
    ))?;
    let records = statement.query_map([], map_record)?.collect();
    records
}

/// Settles an exact pending intent inside the caller's transaction.
///
/// Activating a new grant revokes the former active grant for the same Run first, ensuring that a
/// second explicit approval narrows or relocates authority instead of accumulating path scopes.
pub fn settle_pending_run_grant(
    connection: &Connection,
    pending_action_id: &str,
    activate: bool,
    activation_result_digest: Option<&str>,
    settled_at: i64,
) -> rusqlite::Result<bool> {
    if activate != activation_result_digest.is_some_and(crate::file_change::valid_digest) {
        return Err(invalid(
            "FileChange run grant activation result digest is invalid",
        ));
    }
    let Some(existing) = get_run_grant_for_pending_action(connection, pending_action_id)? else {
        return Ok(false);
    };
    if existing.status != FileChangeRunGrantStatus::Pending {
        let expected = if activate {
            FileChangeRunGrantStatus::Active
        } else {
            FileChangeRunGrantStatus::Inactive
        };
        let same_activation = if activate {
            existing.activation_result_digest.as_deref() == activation_result_digest
        } else {
            existing.activation_result_digest.is_none()
        };
        return if (existing.status == expected && same_activation)
            || existing.status == FileChangeRunGrantStatus::Inactive
        {
            Ok(false)
        } else {
            Err(invalid(
                "FileChange run grant intent already has a different terminal state",
            ))
        };
    }
    if activate {
        connection.execute(
            r#"
            UPDATE agent_file_change_run_grants
            SET status = 'revoked', revision = revision + 1, revoked_at = ?2
            WHERE run_id = ?1 AND status = 'active' AND granting_pending_action_id <> ?3
            "#,
            params![existing.run_id, settled_at, pending_action_id],
        )?;
    }
    let changed = if activate {
        connection.execute(
            r#"
            UPDATE agent_file_change_run_grants
            SET status = 'active', revision = 1, activation_result_digest = ?2,
                activated_at = ?3
            WHERE granting_pending_action_id = ?1 AND status = 'pending' AND revision = 0
            "#,
            params![pending_action_id, activation_result_digest, settled_at],
        )?
    } else {
        connection.execute(
            r#"
            UPDATE agent_file_change_run_grants
            SET status = 'inactive', revision = 1, inactive_at = ?2
            WHERE granting_pending_action_id = ?1 AND status = 'pending' AND revision = 0
            "#,
            params![pending_action_id, settled_at],
        )?
    };
    if changed == 1 {
        Ok(true)
    } else {
        Err(invalid(
            "FileChange run grant intent lost its exact lifecycle CAS",
        ))
    }
}

pub fn revoke_active_run_grant(
    connection: &Connection,
    run_id: &str,
    revoked_at: i64,
) -> rusqlite::Result<usize> {
    connection.execute(
        r#"
        UPDATE agent_file_change_run_grants
        SET status = 'revoked', revision = revision + 1, revoked_at = ?2
        WHERE run_id = ?1 AND status = 'active'
        "#,
        params![run_id, revoked_at],
    )
}

pub fn retire_pending_run_grant_for_action(
    connection: &Connection,
    pending_action_id: &str,
    retired_at: i64,
) -> rusqlite::Result<bool> {
    Ok(connection.execute(
        r#"
        UPDATE agent_file_change_run_grants
        SET status = 'inactive', revision = 1, inactive_at = ?2
        WHERE granting_pending_action_id = ?1 AND status = 'pending' AND revision = 0
        "#,
        params![pending_action_id, retired_at],
    )? == 1)
}

pub fn revoke_nonterminal_run_grants(
    connection: &Connection,
    run_id: &str,
    revoked_at: i64,
) -> rusqlite::Result<usize> {
    connection.execute(
        r#"
        UPDATE agent_file_change_run_grants
        SET status = 'revoked', revision = revision + 1, revoked_at = ?2
        WHERE run_id = ?1 AND status IN ('pending', 'active')
        "#,
        params![run_id, revoked_at],
    )
}

pub fn active_run_grant_matches_ref(
    connection: &Connection,
    run_id: &str,
    expected: &crate::file_change::FileChangeRunGrantRef,
) -> rusqlite::Result<bool> {
    expected.validate().map_err(invalid)?;
    Ok(
        get_active_run_grant(connection, run_id)?.is_some_and(|grant| {
            grant.grant_id == expected.grant_id
                && grant.revision == expected.revision
                && grant.schema_version == expected.schema_version
                && grant.apply_patch_contract_revision == expected.apply_patch_contract_revision
        }),
    )
}

fn select_sql() -> &'static str {
    r#"
    SELECT schema_version, grant_id, revision, status, run_id, conversation_id, project_id,
           scope_kind, workspace_identity, canonical_scope_path, scope_directory_identity_json,
           granting_pending_action_id, base_write_permission, granting_permission_revision,
           granting_tool_set_revision, granting_provider_wire_revision,
           apply_patch_contract_revision, activation_result_digest, created_at,
           activated_at, inactive_at, revoked_at
    FROM agent_file_change_run_grants
    "#
}

fn map_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<FileChangeRunGrantRecord> {
    let record = FileChangeRunGrantRecord {
        schema_version: row.get(0)?,
        grant_id: row.get(1)?,
        revision: i64_to_u64(row.get(2)?)?,
        status: parse_status(row.get::<_, String>(3)?.as_str())?,
        run_id: row.get(4)?,
        conversation_id: row.get(5)?,
        project_id: row.get(6)?,
        scope_kind: parse_scope_kind(row.get::<_, String>(7)?.as_str())?,
        workspace_identity: row.get(8)?,
        canonical_scope_path: row.get(9)?,
        scope_directory_identity: deserialize_directory_identity(row.get::<_, String>(10)?)?,
        granting_pending_action_id: row.get(11)?,
        base_write_permission: parse_write_permission(row.get::<_, String>(12)?.as_str())?,
        granting_permission_revision: row.get(13)?,
        granting_tool_set_revision: row.get(14)?,
        granting_provider_wire_revision: row.get(15)?,
        apply_patch_contract_revision: row.get(16)?,
        activation_result_digest: row.get(17)?,
        created_at: row.get(18)?,
        activated_at: row.get(19)?,
        inactive_at: row.get(20)?,
        revoked_at: row.get(21)?,
    };
    validate_record(&record)?;
    Ok(record)
}

fn validate_record(record: &FileChangeRunGrantRecord) -> rusqlite::Result<()> {
    record.validate().map_err(invalid)
}

/// Extracts the current Host-persisted Run authority needed to re-prove a grant.
///
/// Storage intentionally owns only this narrow authority projection, not a second resume parser.
/// Both copies of the current Run context must be present and exactly equal: the top-level value
/// is what continuation execution consumes, while the checkpoint value is what restart restores.
/// Any unsupported envelope, malformed checkpoint, or divergence makes the grant inert.
fn persisted_granting_run_context(
    agent_input_json: &str,
    grant: &FileChangeRunGrantRecord,
    granting_call_id: &str,
) -> Option<AgentRunContext> {
    let value = serde_json::from_str::<serde_json::Value>(agent_input_json).ok()?;
    let object = value.as_object()?;
    if object
        .get("resumeInputSchemaVersion")
        .and_then(serde_json::Value::as_u64)
        != Some(CURRENT_PERSISTED_AGENT_RESUME_INPUT_SCHEMA_VERSION)
    {
        return None;
    }
    let run_context =
        serde_json::from_value::<AgentRunContext>(object.get("context")?.clone()).ok()?;
    let checkpoint =
        serde_json::from_value::<AgentRunCheckpoint>(object.get("resumeCheckpoint")?.clone())
            .ok()?;
    if checkpoint.version != crate::AGENT_RUN_CHECKPOINT_SCHEMA_VERSION
        || checkpoint.run_id != grant.run_id
        || checkpoint.pending_tool_call_id != granting_call_id
        || checkpoint.pending_action_id.as_deref()
            != Some(grant.granting_pending_action_id.as_str())
        || checkpoint.file_change_run_grant_ref.is_some()
        || checkpoint.run_context.as_ref() != Some(&run_context)
        || checkpoint.tool_set.effective_revision != grant.granting_tool_set_revision
    {
        return None;
    }
    Some(run_context)
}

fn active_grant_has_authoritative_receipt(
    connection: &Connection,
    grant: &FileChangeRunGrantRecord,
) -> rusqlite::Result<bool> {
    let Some(pending) = pending_action_repository::load_pending_action(
        connection,
        &grant.granting_pending_action_id,
    )?
    else {
        return Ok(false);
    };
    if !crate::storage::service::manual_file_effect_has_authoritative_settlement(
        connection, &pending,
    )
    .map_err(invalid)?
    {
        return Ok(false);
    }
    let evidence = connection
        .query_row(
            r#"
            SELECT p.run_id, p.conversation_id, p.assistant_message_id, p.action_type,
                   p.tool_name, p.tool_call_id, p.status, p.target_status, p.action_json,
                   a.run_id, a.conversation_id, a.assistant_message_id, a.action_type,
                   a.tool_name, a.decision, a.status, a.action_json,
                   a.file_change_result_json, a.decision_source, a.completed_at,
                   m.status, t.terminal_status
            FROM agent_pending_actions p
            JOIN agent_action_audit a ON a.action_id = p.action_id
            JOIN messages m
              ON m.id = p.assistant_message_id
             AND m.conversation_id = p.conversation_id
            JOIN conversation_turn_traces t
              ON t.assistant_message_id = p.assistant_message_id
             AND t.conversation_id = p.conversation_id
             AND t.run_id = p.run_id
            WHERE p.action_id = ?1
            "#,
            [&grant.granting_pending_action_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, String>(9)?,
                    row.get::<_, Option<String>>(10)?,
                    row.get::<_, Option<String>>(11)?,
                    row.get::<_, String>(12)?,
                    row.get::<_, String>(13)?,
                    row.get::<_, Option<String>>(14)?,
                    row.get::<_, String>(15)?,
                    row.get::<_, String>(16)?,
                    row.get::<_, Option<String>>(17)?,
                    row.get::<_, Option<String>>(18)?,
                    row.get::<_, Option<i64>>(19)?,
                    row.get::<_, Option<String>>(20)?,
                    row.get::<_, String>(21)?,
                ))
            },
        )
        .optional()?;
    let Some((
        pending_run_id,
        pending_conversation_id,
        pending_assistant_message_id,
        pending_action_type,
        pending_tool_name,
        pending_tool_call_id,
        pending_status,
        pending_target_status,
        pending_action_json,
        audit_run_id,
        audit_conversation_id,
        audit_assistant_message_id,
        audit_action_type,
        audit_tool_name,
        audit_decision,
        audit_status,
        audit_action_json,
        result_json,
        decision_source,
        completed_at,
        message_status,
        trace_terminal_status,
    )) = evidence
    else {
        return Ok(false);
    };
    let Some(result_json) = result_json else {
        return Ok(false);
    };
    let result_is_authoritative =
        serde_json::from_str::<crate::AgentFileChangeResult>(&result_json)
            .ok()
            .zip(serde_json::from_str::<crate::AgentProposedAction>(&audit_action_json).ok())
            .is_some_and(|(result, action)| {
                let crate::AgentProposedAction::FileChange { file_change } = action else {
                    return false;
                };
                let Some(run_context) = persisted_granting_run_context(
                    &pending.agent_input_json,
                    grant,
                    &file_change.id,
                ) else {
                    return false;
                };
                let Ok(derived_scope) =
                    derive_file_change_run_grant_scope(&file_change, &run_context)
                else {
                    return false;
                };
                matches!(
                    result.status,
                    crate::AgentFileChangeResultStatus::Applied
                        | crate::AgentFileChangeResultStatus::AlreadyApplied
                ) && file_change.approval_status == crate::AgentApprovalStatus::Required
                    && derived_scope.matches_record(grant)
                    && file_change.execution.run_id == grant.run_id
                    && file_change.execution.conversation_id == grant.conversation_id
                    && file_change.execution.project_id.as_deref() == grant.project_id.as_deref()
                    && file_change.execution.source_tool_name == "apply_patch"
                    && crate::canonical_pending_action_id(&grant.run_id, &file_change.id)
                        == grant.granting_pending_action_id
                    && file_change.execution.permission_revision
                        == grant.granting_permission_revision
                    && file_change.execution.tool_set_revision == grant.granting_tool_set_revision
                    && file_change.execution.provider_wire_revision
                        == grant.granting_provider_wire_revision
                    && crate::file_change_support::file_change_result_matches_frozen_proposal(
                        &result,
                        &file_change,
                    )
            });
    let authoritative = pending_run_id == grant.run_id
        && pending_conversation_id.as_deref() == Some(grant.conversation_id.as_str())
        && pending_assistant_message_id == audit_assistant_message_id
        && pending_action_type == "file_change"
        && pending_tool_name == "apply_patch"
        && pending_tool_call_id.as_deref().is_some_and(|call_id| {
            crate::canonical_pending_action_id(&grant.run_id, call_id)
                == grant.granting_pending_action_id
        })
        && matches!(pending_status.as_str(), "executing" | "completed")
        && pending_target_status.as_deref() == Some("completed")
        && audit_run_id == grant.run_id
        && audit_conversation_id.as_deref() == Some(grant.conversation_id.as_str())
        && audit_action_type == "file_change"
        && audit_tool_name == "apply_patch"
        && audit_decision.as_deref() == Some("approved")
        && audit_status == "completed"
        && pending_action_json == audit_action_json
        && decision_source.as_deref() == Some("manual")
        && completed_at.is_some()
        && message_status.as_deref() == Some("pending")
        && trace_terminal_status == "in_progress"
        && result_is_authoritative
        && grant.activation_result_digest.as_deref()
            == Some(crate::file_change::content_digest(result_json.as_bytes()).as_str());
    Ok(authoritative)
}

fn serialize_directory_identity(
    identity: &FileChangeDirectoryIdentity,
) -> rusqlite::Result<String> {
    identity.validate().map_err(invalid)?;
    serde_json::to_string(identity)
        .map_err(|_| invalid("FileChange grant directory identity could not be encoded"))
}

fn deserialize_directory_identity(value: String) -> rusqlite::Result<FileChangeDirectoryIdentity> {
    let identity = serde_json::from_str::<FileChangeDirectoryIdentity>(&value)
        .map_err(|_| invalid("FileChange grant directory identity is malformed"))?;
    identity.validate().map_err(invalid)?;
    Ok(identity)
}

fn scope_kind_label(kind: FileChangeRunGrantScopeKind) -> &'static str {
    match kind {
        FileChangeRunGrantScopeKind::Workspace => "workspace",
        FileChangeRunGrantScopeKind::ExternalParent => "external_parent",
    }
}

fn parse_scope_kind(value: &str) -> rusqlite::Result<FileChangeRunGrantScopeKind> {
    match value {
        "workspace" => Ok(FileChangeRunGrantScopeKind::Workspace),
        "external_parent" => Ok(FileChangeRunGrantScopeKind::ExternalParent),
        _ => Err(invalid("invalid FileChange run grant scope kind")),
    }
}

fn parse_status(value: &str) -> rusqlite::Result<FileChangeRunGrantStatus> {
    match value {
        "pending" => Ok(FileChangeRunGrantStatus::Pending),
        "active" => Ok(FileChangeRunGrantStatus::Active),
        "inactive" => Ok(FileChangeRunGrantStatus::Inactive),
        "revoked" => Ok(FileChangeRunGrantStatus::Revoked),
        _ => Err(invalid("invalid FileChange run grant status")),
    }
}

fn write_permission_label(permission: AgentWritePermission) -> &'static str {
    match permission {
        AgentWritePermission::Denied => "denied",
        AgentWritePermission::WorkspaceOnly => "workspace_only",
        AgentWritePermission::All => "all",
    }
}

fn parse_write_permission(value: &str) -> rusqlite::Result<AgentWritePermission> {
    match value {
        "workspace_only" => Ok(AgentWritePermission::WorkspaceOnly),
        "all" => Ok(AgentWritePermission::All),
        _ => Err(invalid("invalid FileChange run grant write ceiling")),
    }
}

fn i64_to_u64(value: i64) -> rusqlite::Result<u64> {
    u64::try_from(value).map_err(|_| invalid("FileChange run grant revision is negative"))
}

fn invalid(message: impl Into<String>) -> rusqlite::Error {
    rusqlite::Error::InvalidParameterName(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_change::APPLY_PATCH_RUN_GRANT_CONTRACT_REVISION;
    use crate::file_change::FILE_CHANGE_RUN_GRANT_SCHEMA_VERSION;
    use crate::storage::StorageState;
    use std::path::Path;

    fn seed_pending_action(connection: &Connection, grant: &FileChangeRunGrantRecord) {
        connection
            .execute(
                r#"
                INSERT OR IGNORE INTO agent_pending_actions (
                    action_id, run_id, conversation_id, assistant_message_id, action_type,
                    tool_name, tool_call_id, status, target_status, action_json, agent_input_json,
                    created_at, updated_at
                ) VALUES (?1, ?2, ?3, 'assistant-1', 'file_change', 'apply_patch', NULL,
                          'approved', NULL, '{}', '{}', 10, 10)
                "#,
                params![
                    grant.granting_pending_action_id,
                    grant.run_id,
                    grant.conversation_id,
                ],
            )
            .unwrap();
    }

    fn insert_intent(connection: &Connection, grant: &FileChangeRunGrantRecord) -> bool {
        seed_pending_action(connection, grant);
        insert_pending_run_grant(connection, grant).unwrap()
    }

    fn activation_digest() -> String {
        crate::file_change::content_digest(b"exact-terminal-result")
    }

    fn grant(call_id: &str, scope: &str) -> FileChangeRunGrantRecord {
        let run_id = "run-1";
        FileChangeRunGrantRecord {
            schema_version: FILE_CHANGE_RUN_GRANT_SCHEMA_VERSION,
            grant_id: format!("grant-{call_id}"),
            revision: 0,
            status: FileChangeRunGrantStatus::Pending,
            run_id: run_id.into(),
            conversation_id: "conversation-1".into(),
            project_id: Some("project-1".into()),
            scope_kind: FileChangeRunGrantScopeKind::Workspace,
            workspace_identity: Some("sha256:workspace".into()),
            canonical_scope_path: scope.into(),
            scope_directory_identity: FileChangeDirectoryIdentity::Unix {
                schema_version: FILE_CHANGE_RUN_GRANT_SCHEMA_VERSION,
                device: 1,
                inode: 2,
            },
            granting_pending_action_id: crate::canonical_pending_action_id(run_id, call_id),
            base_write_permission: AgentWritePermission::WorkspaceOnly,
            granting_permission_revision: "permission-v1".into(),
            granting_tool_set_revision: "tool-set-v1".into(),
            granting_provider_wire_revision: "provider-protocol-v1".into(),
            apply_patch_contract_revision: APPLY_PATCH_RUN_GRANT_CONTRACT_REVISION.into(),
            activation_result_digest: None,
            created_at: 10,
            activated_at: None,
            inactive_at: None,
            revoked_at: None,
        }
    }

    #[test]
    fn intent_activation_is_exact_and_replaces_the_prior_run_grant() {
        let storage = StorageState::open(Path::new(":memory:")).unwrap();
        let connection = storage.connection().unwrap();
        connection
            .execute(
                "INSERT INTO projects (id,name,created_at,updated_at) VALUES ('project-1','p',1,1)",
                [],
            )
            .unwrap();
        connection
            .execute("INSERT INTO conversations (id,project_id,title,created_at,updated_at) VALUES ('conversation-1','project-1','c',1,1)", [])
            .unwrap();
        let first = grant("action-1", "/w");
        assert!(insert_intent(&connection, &first));
        assert!(!insert_pending_run_grant(&connection, &first).unwrap());
        let first_pending_id = crate::canonical_pending_action_id("run-1", "action-1");
        let first_digest = activation_digest();
        assert!(settle_pending_run_grant(
            &connection,
            &first_pending_id,
            true,
            Some(&first_digest),
            20
        )
        .unwrap());
        assert!(!settle_pending_run_grant(
            &connection,
            &first_pending_id,
            true,
            Some(&first_digest),
            21
        )
        .unwrap());
        let active = get_active_run_grant_record(&connection, "run-1")
            .unwrap()
            .unwrap();
        assert_eq!(active.reference().unwrap().revision, 1);

        let second = grant("action-2", "/w/subdir");
        assert!(insert_intent(&connection, &second));
        let second_pending_id = crate::canonical_pending_action_id("run-1", "action-2");
        let second_digest = activation_digest();
        assert!(settle_pending_run_grant(
            &connection,
            &second_pending_id,
            true,
            Some(&second_digest),
            30
        )
        .unwrap());
        let active = get_active_run_grant_record(&connection, "run-1")
            .unwrap()
            .unwrap();
        assert_eq!(active.granting_pending_action_id, second_pending_id);
        assert_eq!(
            get_run_grant_for_pending_action(&connection, &first_pending_id)
                .unwrap()
                .unwrap()
                .status,
            FileChangeRunGrantStatus::Revoked
        );
    }

    #[test]
    fn unsuccessful_intent_is_inactive_and_never_authoritative() {
        let storage = StorageState::open(Path::new(":memory:")).unwrap();
        let connection = storage.connection().unwrap();
        connection
            .execute(
                "INSERT INTO projects (id,name,created_at,updated_at) VALUES ('project-1','p',1,1)",
                [],
            )
            .unwrap();
        connection
            .execute("INSERT INTO conversations (id,project_id,title,created_at,updated_at) VALUES ('conversation-1','project-1','c',1,1)", [])
            .unwrap();
        let pending = grant("action-failed", "/w");
        assert!(insert_intent(&connection, &pending));
        let pending_id = crate::canonical_pending_action_id("run-1", "action-failed");
        assert!(settle_pending_run_grant(&connection, &pending_id, false, None, 20).unwrap());
        assert!(get_active_run_grant(&connection, "run-1")
            .unwrap()
            .is_none());
        assert_eq!(
            get_run_grant_for_pending_action(&connection, &pending_id)
                .unwrap()
                .unwrap()
                .status,
            FileChangeRunGrantStatus::Inactive
        );
        let later_success_digest = activation_digest();
        assert!(!settle_pending_run_grant(
            &connection,
            &pending_id,
            true,
            Some(&later_success_digest),
            30,
        )
        .unwrap());
        assert_eq!(
            get_run_grant_for_pending_action(&connection, &pending_id)
                .unwrap()
                .unwrap()
                .status,
            FileChangeRunGrantStatus::Inactive,
            "singleAction opt-out must not prevent settlement or reactivate remembered authority",
        );
    }

    #[test]
    fn identical_intent_retry_is_idempotent_but_owner_or_scope_change_conflicts() {
        let storage = StorageState::open(Path::new(":memory:")).unwrap();
        let connection = storage.connection().unwrap();
        connection
            .execute("INSERT INTO conversations (id,title,created_at,updated_at) VALUES ('conversation-1','c',1,1)", [])
            .unwrap();
        let first = grant("retry-call", "/w");
        assert!(insert_intent(&connection, &first));
        let mut retry = first.clone();
        retry.grant_id = "new-random-id".into();
        retry.created_at += 1;
        assert!(!insert_pending_run_grant(&connection, &retry).unwrap());
        retry.canonical_scope_path = "/other".into();
        assert!(insert_pending_run_grant(&connection, &retry).is_err());
    }

    #[test]
    fn terminal_retirement_revokes_pending_and_active_authority() {
        let storage = StorageState::open(Path::new(":memory:")).unwrap();
        let connection = storage.connection().unwrap();
        connection
            .execute("INSERT INTO conversations (id,title,created_at,updated_at) VALUES ('conversation-1','c',1,1)", [])
            .unwrap();
        let pending = grant("pending-call", "/w");
        assert!(insert_intent(&connection, &pending));
        let active = grant("active-call", "/w");
        assert!(insert_intent(&connection, &active));
        let active_id = crate::canonical_pending_action_id("run-1", "active-call");
        let digest = activation_digest();
        settle_pending_run_grant(&connection, &active_id, true, Some(&digest), 20).unwrap();
        assert_eq!(
            revoke_nonterminal_run_grants(&connection, "run-1", 30).unwrap(),
            2
        );
        assert!(get_active_run_grant_record(&connection, "run-1")
            .unwrap()
            .is_none());
    }
}
