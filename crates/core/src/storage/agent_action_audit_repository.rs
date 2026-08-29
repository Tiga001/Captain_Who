use crate::storage::models::{AgentActionAuditRecord, AgentUnsettledFileEffect};
use rusqlite::{params, Connection, OptionalExtension};

/// Result of atomically claiming an action's one allowed execution attempt.
///
/// The action audit primary key is also the durable idempotency key. A caller may execute the
/// side effect only after receiving [`AgentActionAuditExecutionClaimOutcome::Claimed`]. An
/// existing claim is deliberately not a lease: after a process crash the action remains claimed
/// and requires state inspection instead of an unsafe automatic replay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentActionAuditExecutionClaimOutcome {
    Claimed,
    AlreadyClaimed {
        status: String,
        tool_result_json: Option<String>,
    },
    IdentityConflict {
        status: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentActionAuditFinalizationOutcome {
    Finalized,
    ClaimMissingOrChanged,
}

/// Outcome of synchronizing an optional manual preterminal audit with its committed pending
/// Direct FileChange credential.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ManualActionAuditJsonCommitOutcome {
    Updated,
    AlreadyCommitted,
    Absent,
    ExpectedActionMismatch,
    NotPreterminal { status: String },
    IdentityConflict { status: String },
}

/// Result of advancing a manually approved command audit to its terminal receipt.
///
/// Manual actions may have an older `pending`, `approved`, or `cancellation_requested` audit from
/// the approval lifecycle. The frozen identity is immutable; only lifecycle/result fields may be
/// advanced. A repeated byte-identical terminal receipt is safe and explicitly idempotent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManualTerminalActionAuditOutcome {
    Inserted,
    Advanced,
    Idempotent,
    Conflict {
        existing_status: Option<String>,
        reason: &'static str,
    },
}

/// Inserts the pre-execution audit receipt if and only if this action has never been claimed.
///
/// SQLite enforces the claim through the `action_id` primary key, so this remains atomic across
/// threads, `StorageService` instances, and processes sharing the database.
pub fn claim_action_audit_execution(
    connection: &Connection,
    record: &AgentActionAuditRecord,
) -> rusqlite::Result<AgentActionAuditExecutionClaimOutcome> {
    let inserted = insert_action_audit_record_if_absent(connection, record)?;
    if inserted {
        return Ok(AgentActionAuditExecutionClaimOutcome::Claimed);
    }

    let existing = load_action_audit_record(connection, &record.action_id)?
        .ok_or_else(|| rusqlite::Error::QueryReturnedNoRows)?;
    let status = existing.status.clone();
    Ok(if has_same_execution_identity(&existing, record) {
        AgentActionAuditExecutionClaimOutcome::AlreadyClaimed {
            status,
            tool_result_json: existing.tool_result_json,
        }
    } else {
        AgentActionAuditExecutionClaimOutcome::IdentityConflict { status }
    })
}

/// Looks up a durable claim without creating one.
///
/// Hosts use this before policy evaluation so a retry can replay an already-final result instead
/// of manufacturing a second, contradictory result when permissions have since changed.
pub fn inspect_action_audit_execution(
    connection: &Connection,
    record: &AgentActionAuditRecord,
) -> rusqlite::Result<Option<AgentActionAuditExecutionClaimOutcome>> {
    let Some(existing) = load_action_audit_record(connection, &record.action_id)? else {
        return Ok(None);
    };
    let status = existing.status.clone();
    Ok(Some(if has_same_execution_identity(&existing, record) {
        AgentActionAuditExecutionClaimOutcome::AlreadyClaimed {
            status,
            tool_result_json: existing.tool_result_json,
        }
    } else {
        AgentActionAuditExecutionClaimOutcome::IdentityConflict { status }
    }))
}

/// Inserts an immutable audit receipt without replacing any record that already owns the id.
pub fn insert_action_audit_record_if_absent(
    connection: &Connection,
    record: &AgentActionAuditRecord,
) -> rusqlite::Result<bool> {
    let changed = connection.execute(
        "
        INSERT INTO agent_action_audit (
            action_id,
            run_id,
            conversation_id,
            assistant_message_id,
            action_type,
            tool_name,
            decision,
            status,
            action_json,
            file_change_result_json,
            command_result_json,
            tool_result_json,
            error,
            created_at,
            decided_at,
            completed_at,
            effective_permissions_json,
            path_scope,
            command_cwd_scope,
            blocked_reason,
            decision_source
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21)
        ON CONFLICT(action_id) DO NOTHING
        ",
        params![
            &record.action_id,
            &record.run_id,
            &record.conversation_id,
            &record.assistant_message_id,
            &record.action_type,
            &record.tool_name,
            &record.decision,
            &record.status,
            &record.action_json,
            &record.file_change_result_json,
            &record.command_result_json,
            &record.tool_result_json,
            &record.error,
            record.created_at,
            record.decided_at,
            record.completed_at,
            &record.effective_permissions_json,
            &record.path_scope,
            &record.command_cwd_scope,
            &record.blocked_reason,
            &record.decision_source,
        ],
    )?;
    Ok(changed == 1)
}

/// Updates the exact preterminal FileChange audit in the caller's pending-action transaction.
///
/// Manual initial audit publication is best effort, so absence is a valid outcome. Once a row
/// exists, however, its complete pending-owner identity and manual preterminal lifecycle are
/// mandatory; a mismatch must abort the outer transaction rather than leave pending and audit
/// with different frozen action JSON.
pub(crate) fn commit_pending_file_change_action_json_if_present(
    connection: &Connection,
    pending_identity: &crate::storage::models::AgentPendingActionRecord,
    expected_action_json: &str,
    committed_action_json: &str,
) -> rusqlite::Result<ManualActionAuditJsonCommitOutcome> {
    let changed = connection.execute(
        "
        UPDATE agent_action_audit
        SET action_json = ?9
        WHERE action_id = ?1
          AND run_id = ?2
          AND conversation_id IS ?3
          AND assistant_message_id IS ?4
          AND action_type = ?5
          AND tool_name = ?6
          AND created_at = ?7
          AND action_json = ?8
          AND (
                (status = 'pending' AND decision IS NULL AND decision_source = 'manual_pending')
             OR (status IN ('approved', 'cancellation_requested') AND decision_source = 'manual')
             OR (status = 'executing' AND decision = 'approved'
                    AND decision_source IN ('auto', 'run_grant'))
          )
        ",
        params![
            &pending_identity.action_id,
            &pending_identity.run_id,
            &pending_identity.conversation_id,
            &pending_identity.assistant_message_id,
            &pending_identity.action_type,
            &pending_identity.tool_name,
            pending_identity.created_at,
            expected_action_json,
            committed_action_json,
        ],
    )?;
    if changed == 1 {
        return Ok(ManualActionAuditJsonCommitOutcome::Updated);
    }
    let Some(existing) = load_action_audit_record(connection, &pending_identity.action_id)? else {
        return Ok(ManualActionAuditJsonCommitOutcome::Absent);
    };
    if existing.action_id != pending_identity.action_id
        || existing.run_id != pending_identity.run_id
        || existing.conversation_id != pending_identity.conversation_id
        || existing.assistant_message_id != pending_identity.assistant_message_id
        || existing.action_type != pending_identity.action_type
        || existing.tool_name != pending_identity.tool_name
        || existing.created_at != pending_identity.created_at
    {
        return Ok(ManualActionAuditJsonCommitOutcome::IdentityConflict {
            status: existing.status,
        });
    }
    let is_file_change_preterminal = matches!(
        (
            existing.status.as_str(),
            existing.decision_source.as_deref()
        ),
        ("pending", Some("manual_pending"))
            | ("approved" | "cancellation_requested", Some("manual"))
            | ("executing", Some("auto" | "run_grant"))
    );
    if !is_file_change_preterminal {
        return Ok(ManualActionAuditJsonCommitOutcome::NotPreterminal {
            status: existing.status,
        });
    }
    Ok(if existing.action_json == committed_action_json {
        ManualActionAuditJsonCommitOutcome::AlreadyCommitted
    } else {
        ManualActionAuditJsonCommitOutcome::ExpectedActionMismatch
    })
}

/// Commits a final result only for the exact pre-execution receipt that is still `executing`.
///
/// This is intentionally an UPDATE rather than an upsert: a missing receipt, a terminal receipt,
/// or a receipt with different frozen identity must never be created or overwritten here.
pub fn finalize_claimed_action_audit_execution(
    connection: &Connection,
    record: &AgentActionAuditRecord,
) -> rusqlite::Result<AgentActionAuditFinalizationOutcome> {
    let changed = connection.execute(
        "
        UPDATE agent_action_audit
        SET status = ?2,
            file_change_result_json = ?3,
            command_result_json = ?4,
            tool_result_json = ?5,
            error = ?6,
            completed_at = ?7,
            blocked_reason = ?8
        WHERE action_id = ?1
          AND status = 'executing'
          AND run_id = ?9
          AND conversation_id IS ?10
          AND assistant_message_id IS ?11
          AND action_type = ?12
          AND tool_name = ?13
          AND decision IS ?14
          AND action_json = ?15
          AND created_at = ?16
          AND decided_at IS ?17
          AND effective_permissions_json IS ?18
          AND path_scope IS ?19
          AND command_cwd_scope IS ?20
          AND decision_source IS ?21
        ",
        params![
            &record.action_id,
            &record.status,
            &record.file_change_result_json,
            &record.command_result_json,
            &record.tool_result_json,
            &record.error,
            record.completed_at,
            &record.blocked_reason,
            &record.run_id,
            &record.conversation_id,
            &record.assistant_message_id,
            &record.action_type,
            &record.tool_name,
            &record.decision,
            &record.action_json,
            record.created_at,
            record.decided_at,
            &record.effective_permissions_json,
            &record.path_scope,
            &record.command_cwd_scope,
            &record.decision_source,
        ],
    )?;
    Ok(if changed == 1 {
        AgentActionAuditFinalizationOutcome::Finalized
    } else {
        AgentActionAuditFinalizationOutcome::ClaimMissingOrChanged
    })
}

/// Inserts or advances one manually approved command audit inside the caller's transaction.
///
/// Unlike [`upsert_action_audit_record`], this function never overwrites frozen identity and never
/// clears the original approval timestamp. It is intended to share a transaction with the
/// pending-action target and paired conversation trace.
pub fn settle_manual_terminal_action_audit(
    connection: &Connection,
    terminal: &AgentActionAuditRecord,
    fallback_decided_at: i64,
) -> rusqlite::Result<ManualTerminalActionAuditOutcome> {
    let existing = load_action_audit_record(connection, &terminal.action_id)?;
    let Some(existing) = existing else {
        let mut inserted = terminal.clone();
        inserted.decided_at = inserted.decided_at.or(Some(fallback_decided_at));
        inserted.decision_source = inserted
            .decision_source
            .or_else(|| Some("manual".to_string()));
        let inserted_record = insert_action_audit_record_if_absent(connection, &inserted)?;
        return Ok(if inserted_record {
            ManualTerminalActionAuditOutcome::Inserted
        } else {
            ManualTerminalActionAuditOutcome::Conflict {
                existing_status: None,
                reason: "audit appeared while terminal receipt was being inserted",
            }
        });
    };

    if !has_same_manual_frozen_identity(&existing, terminal) {
        return Ok(ManualTerminalActionAuditOutcome::Conflict {
            existing_status: Some(existing.status),
            reason: "manual command audit frozen identity differs",
        });
    }

    if matches!(
        existing.status.as_str(),
        "completed" | "failed" | "cancelled" | "rejected"
    ) {
        return Ok(if has_same_manual_terminal_result(&existing, terminal) {
            ManualTerminalActionAuditOutcome::Idempotent
        } else {
            ManualTerminalActionAuditOutcome::Conflict {
                existing_status: Some(existing.status),
                reason: "manual command audit already has a different terminal result",
            }
        });
    }

    let valid_preterminal = match (
        existing.status.as_str(),
        existing.decision_source.as_deref(),
    ) {
        (
            "pending" | "approved" | "cancellation_requested",
            Some("manual") | Some("manual_pending"),
        ) => true,
        ("executing", Some("auto" | "run_grant")) => {
            existing.action_type == "file_change" && existing.tool_name == "apply_patch"
        }
        _ => false,
    };
    if !valid_preterminal {
        return Ok(ManualTerminalActionAuditOutcome::Conflict {
            existing_status: Some(existing.status),
            reason: "file-effect audit has an invalid pre-terminal status",
        });
    }

    let terminal_decided_at = terminal.decided_at.unwrap_or(fallback_decided_at);
    let changed = connection.execute(
        "
        UPDATE agent_action_audit
        SET decision = CASE
                WHEN ?3 = 'rejected' THEN ?2
                ELSE COALESCE(decision, ?2)
            END,
            status = ?3,
            file_change_result_json = ?4,
            command_result_json = ?5,
            tool_result_json = ?6,
            error = ?7,
            decided_at = CASE
                WHEN ?3 = 'rejected' THEN ?8
                ELSE COALESCE(decided_at, ?8)
            END,
            completed_at = ?9,
            blocked_reason = ?10,
            decision_source = ?11
        WHERE action_id = ?1
          AND (
                (
                    status IN ('pending', 'approved', 'cancellation_requested')
                    AND decision_source IN ('manual', 'manual_pending')
                    AND ?11 = 'manual'
                )
                OR (
                    status = 'executing'
                    AND decision_source IN ('auto', 'run_grant')
                    AND action_type = 'file_change'
                    AND tool_name = 'apply_patch'
                    AND ?11 = decision_source
                )
          )
        ",
        params![
            &terminal.action_id,
            &terminal.decision,
            &terminal.status,
            &terminal.file_change_result_json,
            &terminal.command_result_json,
            &terminal.tool_result_json,
            &terminal.error,
            terminal_decided_at,
            terminal.completed_at,
            &terminal.blocked_reason,
            &terminal.decision_source,
        ],
    )?;
    Ok(if changed == 1 {
        ManualTerminalActionAuditOutcome::Advanced
    } else {
        ManualTerminalActionAuditOutcome::Conflict {
            existing_status: Some(existing.status),
            reason: "manual command audit changed before terminal CAS",
        }
    })
}

/// Loads one raw audit receipt for storage-level reconciliation.
///
/// Callers must still validate the receipt against the frozen pending action before treating a
/// terminal status or ToolResult as authoritative.
pub(crate) fn load_action_audit_record(
    connection: &Connection,
    action_id: &str,
) -> rusqlite::Result<Option<AgentActionAuditRecord>> {
    connection
        .query_row(
            "
            SELECT action_id, run_id, conversation_id, assistant_message_id, action_type,
                   tool_name, decision, status, action_json, file_change_result_json,
                   command_result_json, tool_result_json, error, created_at, decided_at,
                   completed_at, effective_permissions_json, path_scope, command_cwd_scope,
                   blocked_reason, decision_source
            FROM agent_action_audit
            WHERE action_id = ?1
            ",
            params![action_id],
            action_audit_record_from_row,
        )
        .optional()
}

/// Lists the exact durable claims that require FileChange startup reconciliation.
///
/// This is deliberately a storage-only selection over the current canonical columns. It neither
/// decodes `action_json` nor accepts, upgrades, or reinterprets any retired payload shape.
pub fn list_executing_file_change_action_audits(
    connection: &Connection,
) -> rusqlite::Result<Vec<AgentActionAuditRecord>> {
    let mut statement = connection.prepare(
        "
        SELECT action_id, run_id, conversation_id, assistant_message_id, action_type,
               tool_name, decision, status, action_json, file_change_result_json,
               command_result_json, tool_result_json, error, created_at, decided_at,
               completed_at, effective_permissions_json, path_scope, command_cwd_scope,
               blocked_reason, decision_source
        FROM agent_action_audit
        WHERE status = 'executing'
          AND action_type = 'file_change'
          AND tool_name = 'apply_patch'
        ORDER BY created_at ASC, action_id ASC
        ",
    )?;
    let records = statement
        .query_map([], action_audit_record_from_row)?
        .collect();
    records
}

fn action_audit_record_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<AgentActionAuditRecord> {
    Ok(AgentActionAuditRecord {
        action_id: row.get(0)?,
        run_id: row.get(1)?,
        conversation_id: row.get(2)?,
        assistant_message_id: row.get(3)?,
        action_type: row.get(4)?,
        tool_name: row.get(5)?,
        decision: row.get(6)?,
        status: row.get(7)?,
        action_json: row.get(8)?,
        file_change_result_json: row.get(9)?,
        command_result_json: row.get(10)?,
        tool_result_json: row.get(11)?,
        error: row.get(12)?,
        created_at: row.get(13)?,
        decided_at: row.get(14)?,
        completed_at: row.get(15)?,
        effective_permissions_json: row.get(16)?,
        path_scope: row.get(17)?,
        command_cwd_scope: row.get(18)?,
        blocked_reason: row.get(19)?,
        decision_source: row.get(20)?,
    })
}

fn has_same_execution_identity(
    existing: &AgentActionAuditRecord,
    candidate: &AgentActionAuditRecord,
) -> bool {
    existing.action_id == candidate.action_id
        && existing.run_id == candidate.run_id
        && existing.conversation_id == candidate.conversation_id
        && existing.assistant_message_id == candidate.assistant_message_id
        && existing.action_type == candidate.action_type
        && existing.tool_name == candidate.tool_name
        && existing.decision == candidate.decision
        && existing.action_json == candidate.action_json
        && existing.effective_permissions_json == candidate.effective_permissions_json
        && existing.path_scope == candidate.path_scope
        && existing.command_cwd_scope == candidate.command_cwd_scope
        && existing.decision_source == candidate.decision_source
}

fn has_same_manual_frozen_identity(
    existing: &AgentActionAuditRecord,
    candidate: &AgentActionAuditRecord,
) -> bool {
    existing.action_id == candidate.action_id
        && existing.run_id == candidate.run_id
        && existing.conversation_id == candidate.conversation_id
        && existing.assistant_message_id == candidate.assistant_message_id
        && existing.action_type == candidate.action_type
        && existing.tool_name == candidate.tool_name
        && existing.action_json == candidate.action_json
        && existing.created_at == candidate.created_at
        && existing.effective_permissions_json == candidate.effective_permissions_json
        && existing.path_scope == candidate.path_scope
        && existing.command_cwd_scope == candidate.command_cwd_scope
}

fn has_same_manual_terminal_result(
    existing: &AgentActionAuditRecord,
    candidate: &AgentActionAuditRecord,
) -> bool {
    existing.status == candidate.status
        && existing.decision == candidate.decision
        && existing.decision_source == candidate.decision_source
        && (candidate.status != "rejected" || existing.decided_at == candidate.decided_at)
        && existing.file_change_result_json == candidate.file_change_result_json
        && existing.command_result_json == candidate.command_result_json
        && existing.tool_result_json == candidate.tool_result_json
        && existing.error == candidate.error
        && existing.completed_at == candidate.completed_at
        && existing.blocked_reason == candidate.blocked_reason
}

pub(crate) fn matches_manual_terminal_action_audit(
    existing: &AgentActionAuditRecord,
    candidate: &AgentActionAuditRecord,
) -> bool {
    has_same_manual_frozen_identity(existing, candidate)
        && has_same_manual_terminal_result(existing, candidate)
}

pub(crate) fn matches_manual_preterminal_action_audit(
    existing: &AgentActionAuditRecord,
    candidate: &AgentActionAuditRecord,
) -> bool {
    has_same_manual_frozen_identity(existing, candidate)
        && matches!(
            existing.status.as_str(),
            "pending" | "approved" | "cancellation_requested"
        )
        && matches!(
            existing.decision_source.as_deref(),
            Some("manual") | Some("manual_pending")
        )
}

pub fn upsert_action_audit_record(
    connection: &Connection,
    record: &AgentActionAuditRecord,
) -> rusqlite::Result<()> {
    connection.execute(
        "
        INSERT INTO agent_action_audit (
            action_id,
            run_id,
            conversation_id,
            assistant_message_id,
            action_type,
            tool_name,
            decision,
            status,
            action_json,
            file_change_result_json,
            command_result_json,
            tool_result_json,
            error,
            created_at,
            decided_at,
            completed_at,
            effective_permissions_json,
            path_scope,
            command_cwd_scope,
            blocked_reason,
            decision_source
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21)
        ON CONFLICT(action_id) DO UPDATE SET
            run_id = excluded.run_id,
            conversation_id = excluded.conversation_id,
            assistant_message_id = excluded.assistant_message_id,
            action_type = excluded.action_type,
            tool_name = excluded.tool_name,
            decision = excluded.decision,
            status = excluded.status,
            action_json = excluded.action_json,
            file_change_result_json = excluded.file_change_result_json,
            command_result_json = excluded.command_result_json,
            tool_result_json = excluded.tool_result_json,
            error = excluded.error,
            created_at = excluded.created_at,
            decided_at = excluded.decided_at,
            completed_at = excluded.completed_at,
            effective_permissions_json = excluded.effective_permissions_json,
            path_scope = excluded.path_scope,
            command_cwd_scope = excluded.command_cwd_scope,
            blocked_reason = excluded.blocked_reason,
            decision_source = excluded.decision_source
        ",
        params![
            &record.action_id,
            &record.run_id,
            &record.conversation_id,
            &record.assistant_message_id,
            &record.action_type,
            &record.tool_name,
            &record.decision,
            &record.status,
            &record.action_json,
            &record.file_change_result_json,
            &record.command_result_json,
            &record.tool_result_json,
            &record.error,
            record.created_at,
            record.decided_at,
            record.completed_at,
            &record.effective_permissions_json,
            &record.path_scope,
            &record.command_cwd_scope,
            &record.blocked_reason,
            &record.decision_source,
        ],
    )?;
    Ok(())
}

pub fn list_tool_result_json_for_run(
    connection: &Connection,
    run_id: &str,
    tool_name: &str,
) -> rusqlite::Result<Vec<String>> {
    let mut statement = connection.prepare(
        "
        SELECT tool_result_json
        FROM agent_action_audit
        WHERE run_id = ?1
          AND tool_name = ?2
          AND tool_result_json IS NOT NULL
        ORDER BY COALESCE(completed_at, decided_at, created_at) ASC, action_id ASC
        ",
    )?;
    let results = statement
        .query_map(params![run_id, tool_name], |row| row.get(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(results)
}

pub fn list_command_result_json_for_run(
    connection: &Connection,
    run_id: &str,
) -> rusqlite::Result<Vec<String>> {
    let mut statement = connection.prepare(
        "
        SELECT command_result_json
        FROM agent_action_audit
        WHERE run_id = ?1
          AND tool_name = 'run_command'
          AND command_result_json IS NOT NULL
        ORDER BY COALESCE(completed_at, decided_at, created_at) ASC, action_id ASC
        ",
    )?;
    let results = statement
        .query_map(params![run_id], |row| row.get(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(results)
}

pub fn list_unsettled_file_effects(
    connection: &Connection,
) -> rusqlite::Result<Vec<AgentUnsettledFileEffect>> {
    let mut statement = connection.prepare(
        "
        SELECT project_id, conversation_id, run_id, action_id
        FROM (
            SELECT
                conversations.project_id AS project_id,
                conversations.id AS conversation_id,
                audit.run_id AS run_id,
                audit.action_id AS action_id,
                audit.created_at AS created_at
            FROM agent_action_audit audit
            JOIN conversations ON conversations.id = audit.conversation_id
            WHERE audit.tool_name IN (
                'run_command',
                'office_document',
                'office_spreadsheet',
                'office_presentation',
                'skills_run_script',
                'skills_materialize_resource'
              )
              AND audit.status = 'executing'
              AND audit.decision_source = 'auto'

            UNION

            SELECT
                conversations.project_id AS project_id,
                conversations.id AS conversation_id,
                pending.run_id AS run_id,
                pending.action_id AS action_id,
                pending.created_at AS created_at
            FROM agent_pending_actions pending
            JOIN conversations ON conversations.id = pending.conversation_id
            WHERE pending.tool_name IN (
                'run_command',
                'office_document',
                'office_spreadsheet',
                'office_presentation',
                'skills_run_script',
                'skills_materialize_resource'
              )
              -- Every write-ahead terminal target is a candidate until StorageService proves a
              -- matching manual terminal audit and paired ToolResult trace. Limiting this branch
              -- to `failed` loses the legacy crash boundary where `completed`/`cancelled` was
              -- durable but the receipt was not.
              AND pending.target_status IN ('completed', 'failed', 'cancelled')
        ) unsettled
        ORDER BY created_at ASC, action_id ASC
        ",
    )?;
    let records = statement
        .query_map([], |row| {
            Ok(AgentUnsettledFileEffect {
                project_id: row.get(0)?,
                conversation_id: row.get(1)?,
                run_id: row.get(2)?,
                action_id: row.get(3)?,
            })
        })?
        .collect();
    records
}

pub fn delete_action_audit_for_conversation(
    connection: &Connection,
    conversation_id: &str,
) -> rusqlite::Result<()> {
    connection.execute(
        "DELETE FROM agent_action_audit WHERE conversation_id = ?1",
        params![conversation_id],
    )?;
    Ok(())
}

/// Retires action receipts owned by specific assistant messages after the service-level deletion
/// barrier has proved that no effect is still executing or durably unsettled.
///
/// Keep this in the caller's message-deletion transaction. The audit table deliberately outlives
/// ordinary message foreign-key cascades for crash recovery, so deleting the message first would
/// otherwise manufacture an orphan receipt that looks unsettled on the next startup.
pub(crate) fn delete_action_audit_for_messages(
    connection: &Connection,
    conversation_id: &str,
    assistant_message_ids: &[String],
) -> rusqlite::Result<()> {
    for assistant_message_id in assistant_message_ids {
        connection.execute(
            "
            DELETE FROM agent_action_audit
            WHERE conversation_id = ?1 AND assistant_message_id = ?2
            ",
            params![conversation_id, assistant_message_id],
        )?;
    }
    Ok(())
}

pub fn delete_action_audit_for_project(
    connection: &Connection,
    project_id: &str,
) -> rusqlite::Result<()> {
    connection.execute(
        "
        DELETE FROM agent_action_audit
        WHERE conversation_id IN (
            SELECT id
            FROM conversations
            WHERE project_id = ?1
        )
        ",
        params![project_id],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::migrations;

    fn executing_auto_file_change(action_id: &str, action_json: &str) -> AgentActionAuditRecord {
        AgentActionAuditRecord {
            action_id: action_id.to_string(),
            run_id: "run-1".to_string(),
            conversation_id: Some("conversation-1".to_string()),
            assistant_message_id: Some("message-1".to_string()),
            action_type: "file_change".to_string(),
            tool_name: "apply_patch".to_string(),
            decision: Some("approved".to_string()),
            status: "executing".to_string(),
            action_json: action_json.to_string(),
            file_change_result_json: None,
            command_result_json: None,
            tool_result_json: None,
            error: None,
            created_at: 10,
            decided_at: Some(10),
            completed_at: None,
            effective_permissions_json: Some(r#"{"write":"workspace_only"}"#.to_string()),
            path_scope: Some("workspace".to_string()),
            command_cwd_scope: None,
            blocked_reason: None,
            decision_source: Some("auto".to_string()),
        }
    }

    #[test]
    fn lists_only_current_executing_file_changes_as_complete_records_in_stable_order() {
        let connection = Connection::open_in_memory().unwrap();
        migrations::run_migrations(&connection).unwrap();

        let mut same_time_later_id =
            executing_auto_file_change("z-same-time", r#"{"proposal":"z"}"#);
        same_time_later_id.created_at = 20;
        same_time_later_id.decided_at = Some(21);
        let mut earliest = executing_auto_file_change("middle-earliest", r#"{"proposal":"first"}"#);
        earliest.run_id = "run-earliest".to_string();
        earliest.conversation_id = Some("conversation-earliest".to_string());
        earliest.assistant_message_id = Some("message-earliest".to_string());
        earliest.created_at = 5;
        earliest.decided_at = Some(6);
        earliest.effective_permissions_json =
            Some(r#"{"read":"workspace_only","write":"workspace_only"}"#.to_string());
        earliest.path_scope = Some("workspace:/project".to_string());
        earliest.decision_source = Some("auto".to_string());
        let mut same_time_earlier_id =
            executing_auto_file_change("a-same-time", r#"{"proposal":"a"}"#);
        same_time_earlier_id.created_at = 20;
        same_time_earlier_id.decided_at = Some(22);
        let mut staged = executing_auto_file_change("staged-current", r#"{"proposal":"staged"}"#);
        staged.created_at = 15;

        // Insert deliberately out of result order so the contract depends on SQL ordering.
        for record in [
            &same_time_later_id,
            &staged,
            &earliest,
            &same_time_earlier_id,
        ] {
            upsert_action_audit_record(&connection, record).unwrap();
        }

        let mut other_executing_action_type =
            executing_auto_file_change("executing-command", r#"{"proposal":"command"}"#);
        other_executing_action_type.action_type = "command".to_string();
        upsert_action_audit_record(&connection, &other_executing_action_type).unwrap();

        let mut other_executing_tool =
            executing_auto_file_change("executing-other-tool", r#"{"proposal":"tool"}"#);
        other_executing_tool.tool_name = "run_command".to_string();
        upsert_action_audit_record(&connection, &other_executing_tool).unwrap();

        let mut invalid_staged_tool =
            executing_auto_file_change("invalid-staged-tool", r#"{"proposal":"invalid"}"#);
        invalid_staged_tool.tool_name = "run_command".to_string();
        upsert_action_audit_record(&connection, &invalid_staged_tool).unwrap();

        let mut terminal_file_change =
            executing_auto_file_change("completed-file-change", r#"{"proposal":"terminal"}"#);
        terminal_file_change.status = "completed".to_string();
        terminal_file_change.completed_at = Some(30);
        upsert_action_audit_record(&connection, &terminal_file_change).unwrap();

        let records = list_executing_file_change_action_audits(&connection).unwrap();
        assert_eq!(
            records
                .iter()
                .map(|record| record.action_id.as_str())
                .collect::<Vec<_>>(),
            vec![
                "middle-earliest",
                "staged-current",
                "a-same-time",
                "z-same-time"
            ]
        );

        let loaded = &records[0];
        assert_eq!(loaded.run_id, earliest.run_id);
        assert_eq!(loaded.conversation_id, earliest.conversation_id);
        assert_eq!(loaded.assistant_message_id, earliest.assistant_message_id);
        assert_eq!(loaded.action_type, "file_change");
        assert_eq!(loaded.tool_name, "apply_patch");
        assert_eq!(loaded.decision, Some("approved".to_string()));
        assert_eq!(loaded.status, "executing");
        assert_eq!(loaded.action_json, earliest.action_json);
        assert_eq!(
            loaded.effective_permissions_json,
            earliest.effective_permissions_json
        );
        assert_eq!(loaded.path_scope, earliest.path_scope);
        assert_eq!(loaded.decision_source, Some("auto".to_string()));
        assert_eq!(loaded.created_at, 5);
        assert_eq!(loaded.decided_at, Some(6));
        assert_eq!(loaded.completed_at, None);
    }

    #[test]
    fn upserts_action_audit_record() {
        let connection = Connection::open_in_memory().unwrap();
        migrations::run_migrations(&connection).unwrap();

        let mut record = AgentActionAuditRecord {
            action_id: "action-1".to_string(),
            run_id: "run-1".to_string(),
            conversation_id: Some("conversation-1".to_string()),
            assistant_message_id: Some("message-1".to_string()),
            action_type: "command".to_string(),
            tool_name: "run_command".to_string(),
            decision: Some("approved".to_string()),
            status: "approved".to_string(),
            action_json: "{}".to_string(),
            file_change_result_json: None,
            command_result_json: None,
            tool_result_json: None,
            error: None,
            created_at: 1,
            decided_at: Some(2),
            completed_at: None,
            effective_permissions_json: Some(r#"{"write":"workspace_only"}"#.to_string()),
            path_scope: Some("workspace".to_string()),
            command_cwd_scope: Some("workspace".to_string()),
            blocked_reason: None,
            decision_source: Some("manual".to_string()),
        };
        upsert_action_audit_record(&connection, &record).unwrap();

        record.status = "completed".to_string();
        record.completed_at = Some(3);
        upsert_action_audit_record(&connection, &record).unwrap();

        let (status, decision_source): (String, Option<String>) = connection
            .query_row(
                "SELECT status, decision_source FROM agent_action_audit WHERE action_id = 'action-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, "completed");
        assert_eq!(decision_source.as_deref(), Some("manual"));
    }

    #[test]
    fn execution_claim_is_atomic_and_never_overwrites_the_first_receipt() {
        let connection = Connection::open_in_memory().unwrap();
        migrations::run_migrations(&connection).unwrap();

        let mut first = AgentActionAuditRecord {
            action_id: "run-1:action-1".to_string(),
            run_id: "run-1".to_string(),
            conversation_id: Some("conversation-1".to_string()),
            assistant_message_id: Some("message-1".to_string()),
            action_type: "office_operation".to_string(),
            tool_name: "office_spreadsheet".to_string(),
            decision: Some("approved".to_string()),
            status: "executing".to_string(),
            action_json: r#"{"operation":"set"}"#.to_string(),
            file_change_result_json: None,
            command_result_json: None,
            tool_result_json: None,
            error: None,
            created_at: 1,
            decided_at: Some(1),
            completed_at: None,
            effective_permissions_json: None,
            path_scope: None,
            command_cwd_scope: None,
            blocked_reason: None,
            decision_source: Some("auto".to_string()),
        };
        assert_eq!(
            claim_action_audit_execution(&connection, &first).unwrap(),
            AgentActionAuditExecutionClaimOutcome::Claimed
        );

        first.action_json = r#"{"operation":"remove"}"#.to_string();
        first.created_at = 2;
        assert_eq!(
            claim_action_audit_execution(&connection, &first).unwrap(),
            AgentActionAuditExecutionClaimOutcome::IdentityConflict {
                status: "executing".to_string()
            }
        );

        let (action_json, created_at): (String, i64) = connection
            .query_row(
                "SELECT action_json, created_at FROM agent_action_audit WHERE action_id = ?1",
                params![first.action_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(action_json, r#"{"operation":"set"}"#);
        assert_eq!(created_at, 1);
    }

    #[test]
    fn matching_execution_claim_is_idempotent_and_finalization_is_cas_guarded() {
        let connection = Connection::open_in_memory().unwrap();
        migrations::run_migrations(&connection).unwrap();
        let mut record = AgentActionAuditRecord {
            action_id: "run-1:action-1".to_string(),
            run_id: "run-1".to_string(),
            conversation_id: Some("conversation-1".to_string()),
            assistant_message_id: Some("message-1".to_string()),
            action_type: "office_operation".to_string(),
            tool_name: "office_spreadsheet".to_string(),
            decision: Some("approved".to_string()),
            status: "executing".to_string(),
            action_json: r#"{"operation":"set"}"#.to_string(),
            file_change_result_json: None,
            command_result_json: None,
            tool_result_json: None,
            error: None,
            created_at: 1,
            decided_at: Some(1),
            completed_at: None,
            effective_permissions_json: Some(r#"{"write":"all"}"#.to_string()),
            path_scope: Some("external".to_string()),
            command_cwd_scope: None,
            blocked_reason: None,
            decision_source: Some("auto".to_string()),
        };
        assert_eq!(
            claim_action_audit_execution(&connection, &record).unwrap(),
            AgentActionAuditExecutionClaimOutcome::Claimed
        );

        let mut retry = record.clone();
        retry.created_at = 99;
        retry.decided_at = Some(99);
        assert_eq!(
            claim_action_audit_execution(&connection, &retry).unwrap(),
            AgentActionAuditExecutionClaimOutcome::AlreadyClaimed {
                status: "executing".to_string(),
                tool_result_json: None,
            }
        );

        record.status = "completed".to_string();
        record.tool_result_json = Some(r#"{"ok":true}"#.to_string());
        record.completed_at = Some(2);
        assert_eq!(
            finalize_claimed_action_audit_execution(&connection, &record).unwrap(),
            AgentActionAuditFinalizationOutcome::Finalized
        );
        assert_eq!(
            finalize_claimed_action_audit_execution(&connection, &record).unwrap(),
            AgentActionAuditFinalizationOutcome::ClaimMissingOrChanged
        );

        let persisted = load_action_audit_record(&connection, &record.action_id)
            .unwrap()
            .unwrap();
        assert_eq!(persisted.status, "completed");
        assert_eq!(persisted.tool_result_json, record.tool_result_json);
        assert_eq!(persisted.created_at, 1);

        retry.status = "executing".to_string();
        assert_eq!(
            claim_action_audit_execution(&connection, &retry).unwrap(),
            AgentActionAuditExecutionClaimOutcome::AlreadyClaimed {
                status: "completed".to_string(),
                tool_result_json: Some(r#"{"ok":true}"#.to_string()),
            }
        );
    }

    #[test]
    fn recovered_approved_action_can_record_a_later_explicit_rejection() {
        let connection = Connection::open_in_memory().unwrap();
        migrations::run_migrations(&connection).unwrap();
        let approved = AgentActionAuditRecord {
            action_id: "mcp-action-1".to_string(),
            run_id: "run-1".to_string(),
            conversation_id: Some("conversation-1".to_string()),
            assistant_message_id: Some("message-1".to_string()),
            action_type: "mcp_tool_call".to_string(),
            tool_name: "mcp__fixture__echo".to_string(),
            decision: Some("approved".to_string()),
            status: "approved".to_string(),
            action_json: r#"{"type":"mcp_tool_call"}"#.to_string(),
            file_change_result_json: None,
            command_result_json: None,
            tool_result_json: None,
            error: None,
            created_at: 1,
            decided_at: Some(10),
            completed_at: None,
            effective_permissions_json: Some(r#"{"write":"workspace_only"}"#.to_string()),
            path_scope: None,
            command_cwd_scope: None,
            blocked_reason: None,
            decision_source: Some("manual".to_string()),
        };
        upsert_action_audit_record(&connection, &approved).unwrap();

        let mut rejected = approved.clone();
        rejected.decision = Some("rejected".to_string());
        rejected.status = "rejected".to_string();
        rejected.tool_result_json = Some(r#"{"status":"rejected"}"#.to_string());
        rejected.decided_at = Some(20);
        rejected.completed_at = Some(20);
        assert_eq!(
            settle_manual_terminal_action_audit(&connection, &rejected, 20).unwrap(),
            ManualTerminalActionAuditOutcome::Advanced
        );

        let persisted = load_action_audit_record(&connection, &rejected.action_id)
            .unwrap()
            .unwrap();
        assert_eq!(persisted.status, "rejected");
        assert_eq!(persisted.decision.as_deref(), Some("rejected"));
        assert_eq!(persisted.decided_at, Some(20));
        assert_eq!(
            settle_manual_terminal_action_audit(&connection, &rejected, 20).unwrap(),
            ManualTerminalActionAuditOutcome::Idempotent
        );

        let mut contradictory = rejected.clone();
        contradictory.decision = Some("approved".to_string());
        assert!(matches!(
            settle_manual_terminal_action_audit(&connection, &contradictory, 20).unwrap(),
            ManualTerminalActionAuditOutcome::Conflict { .. }
        ));
    }

    #[test]
    fn lists_only_matching_persisted_tool_results_in_completion_order() {
        let connection = Connection::open_in_memory().unwrap();
        migrations::run_migrations(&connection).unwrap();
        let base = AgentActionAuditRecord {
            action_id: "action-2".to_string(),
            run_id: "run-1".to_string(),
            conversation_id: Some("conversation-1".to_string()),
            assistant_message_id: Some("message-1".to_string()),
            action_type: "file_change".to_string(),
            tool_name: "apply_patch".to_string(),
            decision: Some("approved".to_string()),
            status: "completed".to_string(),
            action_json: "{}".to_string(),
            file_change_result_json: None,
            command_result_json: None,
            tool_result_json: Some(r#"{"callId":"action-2"}"#.to_string()),
            error: None,
            created_at: 2,
            decided_at: Some(3),
            completed_at: Some(4),
            effective_permissions_json: None,
            path_scope: None,
            command_cwd_scope: None,
            blocked_reason: None,
            decision_source: Some("manual".to_string()),
        };
        upsert_action_audit_record(&connection, &base).unwrap();
        let mut first = base.clone();
        first.action_id = "action-1".to_string();
        first.tool_result_json = Some(r#"{"callId":"action-1"}"#.to_string());
        first.created_at = 1;
        first.decided_at = Some(1);
        first.completed_at = Some(1);
        upsert_action_audit_record(&connection, &first).unwrap();
        let mut other_tool = base.clone();
        other_tool.action_id = "action-3".to_string();
        other_tool.tool_name = "run_command".to_string();
        other_tool.tool_result_json = Some(r#"{"callId":"action-3"}"#.to_string());
        upsert_action_audit_record(&connection, &other_tool).unwrap();

        assert_eq!(
            list_tool_result_json_for_run(&connection, "run-1", "apply_patch").unwrap(),
            vec![
                r#"{"callId":"action-1"}"#.to_string(),
                r#"{"callId":"action-2"}"#.to_string()
            ]
        );
    }
}
