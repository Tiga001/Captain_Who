//! SQLite persistence and CAS transitions for durable Task Continuation State.

use crate::{
    ContextHistoryRef, TaskContinuationCheckpoint, TaskControlState, TaskControlStatus,
    TaskInterruptionState, TaskStatePatchOperation, TaskStateSnapshot, TaskWorkItem,
    TaskWorkItemStatus, TASK_CONTROL_STATE_SCHEMA_VERSION,
};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use std::collections::BTreeSet;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskStateRevisionRecord {
    pub revision: u64,
    pub snapshot: TaskStateSnapshot,
    pub mutation_kind: String,
    pub mutation_id: Option<String>,
    pub result_task_id: String,
    pub created_at: i64,
}

pub fn load_task(
    connection: &Connection,
    conversation_id: &str,
    task_id: &str,
) -> Result<Option<TaskStateSnapshot>, String> {
    connection
        .query_row(
            "SELECT schema_version, task_id, conversation_id, objective, source_message_id,
                    status, revision, current_run_id, stopped_reason, checkpoint_json,
                    created_at, updated_at
             FROM task_control_states
             WHERE conversation_id = ?1 AND task_id = ?2",
            params![conversation_id, task_id],
            decode_snapshot_row,
        )
        .optional()
        .map_err(database_error)
}

pub fn load_latest_task(
    connection: &Connection,
    conversation_id: &str,
) -> Result<Option<TaskStateSnapshot>, String> {
    connection
        .query_row(
            "SELECT schema_version, task_id, conversation_id, objective, source_message_id,
                    status, revision, current_run_id, stopped_reason, checkpoint_json,
                    created_at, updated_at
             FROM task_control_states
             WHERE conversation_id = ?1
             ORDER BY
                CASE WHEN status IN ('active', 'waiting_user', 'blocked') THEN 0 ELSE 1 END,
                updated_at DESC, task_id DESC
             LIMIT 1",
            [conversation_id],
            decode_snapshot_row,
        )
        .optional()
        .map_err(database_error)
}

pub fn list_revisions(
    connection: &Connection,
    conversation_id: &str,
    task_id: &str,
) -> Result<Vec<TaskStateRevisionRecord>, String> {
    let mut statement = connection
        .prepare(
            "SELECT revision, snapshot_json, mutation_kind, mutation_id, result_task_id, created_at
             FROM task_control_state_revisions
             WHERE conversation_id = ?1 AND task_id = ?2
             ORDER BY revision",
        )
        .map_err(database_error)?;
    let records = statement
        .query_map(params![conversation_id, task_id], |row| {
            let snapshot_json = row.get::<_, String>(1)?;
            let snapshot = serde_json::from_str::<TaskStateSnapshot>(&snapshot_json)
                .map_err(json_decode_error)?;
            Ok(TaskStateRevisionRecord {
                revision: row.get(0)?,
                snapshot,
                mutation_kind: row.get(2)?,
                mutation_id: row.get(3)?,
                result_task_id: row.get(4)?,
                created_at: row.get(5)?,
            })
        })
        .map_err(database_error)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(database_error)?;
    Ok(records)
}

pub fn begin_or_resume_for_turn(
    connection: &mut Connection,
    conversation_id: &str,
    source_message_id: &str,
    objective: &str,
    run_id: &str,
    created_at: i64,
) -> Result<TaskStateSnapshot, String> {
    let transaction = connection.transaction().map_err(database_error)?;
    let current = load_latest_task(&transaction, conversation_id)?;
    let snapshot = match current {
        Some(mut snapshot) if !snapshot.control.status.is_terminal() => {
            let mutation_id = format!("host:begin:{run_id}");
            if let Some(existing) =
                revision_for_mutation(&transaction, &snapshot.control.task_id, &mutation_id)?
            {
                load_task(&transaction, conversation_id, &existing.result_task_id)?
                    .ok_or_else(|| "Task State begin journal 指向不存在的任务。".to_string())?
            } else {
                snapshot.control.revision = snapshot.control.revision.saturating_add(1);
                snapshot.control.current_run_id = Some(run_id.to_string());
                snapshot.control.stopped_reason = None;
                if matches!(
                    snapshot.control.status,
                    TaskControlStatus::WaitingUser | TaskControlStatus::Blocked
                ) {
                    snapshot.control.status = TaskControlStatus::Active;
                }
                snapshot.checkpoint.interruption = None;
                snapshot.control.updated_at = created_at.max(snapshot.control.updated_at);
                validate_snapshot_refs(&transaction, &snapshot)?;
                update_snapshot_cas(
                    &transaction,
                    &snapshot,
                    snapshot.control.revision - 1,
                    "run_started",
                    Some(&mutation_id),
                    &snapshot.control.task_id,
                )?;
                snapshot
            }
        }
        _ => create_task_in_transaction(
            &transaction,
            conversation_id,
            source_message_id,
            objective,
            Some(run_id),
            created_at,
            "created",
            Some(&format!("host:begin:{run_id}")),
        )?,
    };
    transaction.commit().map_err(database_error)?;
    Ok(snapshot)
}

#[allow(clippy::too_many_arguments)]
pub fn patch_task(
    connection: &mut Connection,
    conversation_id: &str,
    task_id: &str,
    expected_revision: u64,
    operations: &[TaskStatePatchOperation],
    run_id: Option<&str>,
    mutation_id: &str,
    updated_at: i64,
) -> Result<TaskStateSnapshot, String> {
    let transaction = connection.transaction().map_err(database_error)?;
    if let Some(existing) = revision_for_mutation(&transaction, task_id, mutation_id)? {
        let snapshot = load_task(&transaction, conversation_id, &existing.result_task_id)?
            .ok_or_else(|| "Task State 幂等 journal 指向不存在的结果任务。".to_string())?;
        transaction.commit().map_err(database_error)?;
        return Ok(snapshot);
    }
    let mut snapshot = load_task(&transaction, conversation_id, task_id)?
        .ok_or_else(|| "Task State 不存在或不属于当前 conversation。".to_string())?;
    if snapshot.control.revision != expected_revision {
        return Err(format!(
            "Task State revision 冲突：expectedRevision={expected_revision}，currentRevision={}。",
            snapshot.control.revision
        ));
    }
    if let Some(run_id) = run_id {
        if snapshot.control.current_run_id.as_deref() != Some(run_id) {
            return Err("Task State 不属于当前活动 run，拒绝写入。".to_string());
        }
    }
    if snapshot.control.status.is_terminal() {
        return Err("终态 Task State 不能继续修改；请创建或接续新任务。".to_string());
    }

    let superseding_objective = snapshot
        .apply_operations(operations)
        .map_err(|error| error.to_string())?;
    if let Some(objective) = superseding_objective {
        let source_message_id = latest_user_message_id(&transaction, conversation_id)?
            .ok_or_else(|| "无法定位 supersede 的最新用户消息。".to_string())?;
        let previous_revision = snapshot.control.revision;
        snapshot.control.revision = snapshot.control.revision.saturating_add(1);
        snapshot.control.status = TaskControlStatus::Superseded;
        snapshot.control.current_run_id = None;
        snapshot.control.stopped_reason = Some("superseded_by_user_correction".to_string());
        snapshot.control.updated_at = updated_at.max(snapshot.control.updated_at);
        snapshot.validate().map_err(|error| error.to_string())?;
        let new_task_id = format!("task-{}", Uuid::new_v4());
        update_snapshot_cas(
            &transaction,
            &snapshot,
            previous_revision,
            "superseded",
            Some(mutation_id),
            &new_task_id,
        )?;
        let replacement = create_task_with_id_in_transaction(
            &transaction,
            &new_task_id,
            conversation_id,
            &source_message_id,
            &objective,
            run_id,
            updated_at,
            "superseding_task_created",
            None,
        )?;
        transaction.commit().map_err(database_error)?;
        return Ok(replacement);
    }

    let previous_revision = snapshot.control.revision;
    snapshot.control.revision = snapshot.control.revision.saturating_add(1);
    snapshot.control.updated_at = updated_at.max(snapshot.control.updated_at);
    validate_snapshot_refs(&transaction, &snapshot)?;
    update_snapshot_cas(
        &transaction,
        &snapshot,
        previous_revision,
        "model_patch",
        Some(mutation_id),
        &snapshot.control.task_id,
    )?;
    transaction.commit().map_err(database_error)?;
    Ok(snapshot)
}

#[allow(clippy::too_many_arguments)]
pub fn rollback_task(
    connection: &mut Connection,
    conversation_id: &str,
    task_id: &str,
    expected_revision: u64,
    target_revision: u64,
    mutation_id: &str,
    updated_at: i64,
) -> Result<TaskStateSnapshot, String> {
    let transaction = connection.transaction().map_err(database_error)?;
    if let Some(existing) = revision_for_mutation(&transaction, task_id, mutation_id)? {
        let snapshot = load_task(&transaction, conversation_id, &existing.result_task_id)?
            .ok_or_else(|| "Task State 回滚 journal 指向不存在的结果任务。".to_string())?;
        transaction.commit().map_err(database_error)?;
        return Ok(snapshot);
    }
    let current = load_task(&transaction, conversation_id, task_id)?
        .ok_or_else(|| "Task State 不存在或不属于当前 conversation。".to_string())?;
    if current.control.revision != expected_revision {
        return Err(format!(
            "Task State revision 冲突：expectedRevision={expected_revision}，currentRevision={}。",
            current.control.revision
        ));
    }
    if target_revision == 0 || target_revision >= expected_revision {
        return Err("Task State 回滚目标必须是早于当前 revision 的既有版本。".to_string());
    }
    let latest = load_latest_task(&transaction, conversation_id)?
        .ok_or_else(|| "当前 conversation 没有 Task State。".to_string())?;
    if latest.control.task_id != task_id {
        return Err("只能回滚当前 conversation 的最新 Task State。".to_string());
    }
    let target = load_revision_snapshot(&transaction, conversation_id, task_id, target_revision)?
        .ok_or_else(|| format!("Task State revision {target_revision} 不存在。"))?;

    let mut restored = target;
    restored.control.schema_version = TASK_CONTROL_STATE_SCHEMA_VERSION;
    restored.control.task_id = current.control.task_id.clone();
    restored.control.conversation_id = current.control.conversation_id.clone();
    restored.control.revision = current.control.revision.saturating_add(1);
    restored.control.current_run_id = if restored.control.status.is_terminal() {
        None
    } else {
        current.control.current_run_id.clone()
    };
    restored.control.created_at = current.control.created_at;
    restored.control.updated_at = updated_at.max(current.control.updated_at);
    validate_snapshot_refs(&transaction, &restored)?;
    update_snapshot_cas(
        &transaction,
        &restored,
        expected_revision,
        "rollback",
        Some(mutation_id),
        task_id,
    )?;
    transaction.commit().map_err(database_error)?;
    Ok(restored)
}

#[allow(clippy::too_many_arguments)]
pub fn replace_work_items(
    connection: &mut Connection,
    conversation_id: &str,
    task_id: &str,
    expected_revision: u64,
    work_items: Vec<TaskWorkItem>,
    run_id: &str,
    mutation_id: &str,
    updated_at: i64,
) -> Result<TaskStateSnapshot, String> {
    let current = load_task(connection, conversation_id, task_id)?
        .ok_or_else(|| "Task State 不存在或不属于当前 conversation。".to_string())?;
    let all_completed = !work_items.is_empty()
        && work_items
            .iter()
            .all(|item| item.status == TaskWorkItemStatus::Completed);
    let mut operations = vec![TaskStatePatchOperation::ReplaceWorkItems {
        items: work_items.clone(),
    }];
    if all_completed {
        let mut refs = current.checkpoint.history_evidence_refs.clone();
        for reference in work_items.iter().flat_map(|item| item.evidence_refs.iter()) {
            if !refs.contains(reference) {
                refs.push(reference.clone());
            }
        }
        refs.truncate(40);
        operations.push(TaskStatePatchOperation::SetHistoryEvidenceRefs { refs });
        operations.push(TaskStatePatchOperation::SetStatus {
            status: TaskControlStatus::Completed,
            stopped_reason: None,
        });
    }
    patch_task(
        connection,
        conversation_id,
        task_id,
        expected_revision,
        &operations,
        Some(run_id),
        mutation_id,
        updated_at,
    )
}

pub fn repair_after_history_mutation(
    transaction: &Transaction<'_>,
    conversation_id: &str,
    updated_at: i64,
) -> Result<Option<TaskStateSnapshot>, String> {
    let Some(mut snapshot) = load_latest_task(transaction, conversation_id)? else {
        return Ok(None);
    };
    let Some(latest_user_message_id) = latest_user_message_id(transaction, conversation_id)? else {
        transaction
            .execute(
                "DELETE FROM task_control_states WHERE conversation_id = ?1",
                [conversation_id],
            )
            .map_err(database_error)?;
        return Ok(None);
    };

    let mut changed = false;
    if !message_exists(
        transaction,
        conversation_id,
        &snapshot.control.source_message_id,
    )? {
        snapshot.control.source_message_id = latest_user_message_id;
        changed = true;
    }
    changed |= retain_existing_refs(
        transaction,
        conversation_id,
        &mut snapshot.checkpoint.history_evidence_refs,
    )?;
    for item in &mut snapshot.checkpoint.work_items {
        changed |= retain_existing_refs(transaction, conversation_id, &mut item.evidence_refs)?;
        if item.status == TaskWorkItemStatus::Completed && item.evidence_refs.is_empty() {
            item.status = TaskWorkItemStatus::InProgress;
            item.note = Some(
                "Completion evidence was deleted; this work item must be revalidated.".to_string(),
            );
            changed = true;
        }
    }
    for decision in &mut snapshot.checkpoint.confirmed_decisions {
        changed |= retain_existing_refs(transaction, conversation_id, &mut decision.evidence_refs)?;
    }
    let mut retained_artifacts = Vec::with_capacity(snapshot.checkpoint.artifact_refs.len());
    for artifact in std::mem::take(&mut snapshot.checkpoint.artifact_refs) {
        if history_ref_exists(transaction, conversation_id, &artifact.reference)? {
            retained_artifacts.push(artifact);
        } else {
            changed = true;
        }
    }
    snapshot.checkpoint.artifact_refs = retained_artifacts;
    if let Some(interruption) = &mut snapshot.checkpoint.interruption {
        let valid_work_item_ids = snapshot
            .checkpoint
            .work_items
            .iter()
            .map(|item| item.id.as_str())
            .collect::<BTreeSet<_>>();
        let previous_len = interruption.revalidate_work_item_ids.len();
        interruption
            .revalidate_work_item_ids
            .retain(|id| valid_work_item_ids.contains(id.as_str()));
        changed |= previous_len != interruption.revalidate_work_item_ids.len();
    }
    if snapshot.control.status == TaskControlStatus::Completed
        && (snapshot.checkpoint.history_evidence_refs.is_empty()
            || snapshot.checkpoint.work_items.iter().any(|item| {
                !matches!(
                    item.status,
                    TaskWorkItemStatus::Completed | TaskWorkItemStatus::Cancelled
                )
            }))
    {
        snapshot.control.status = TaskControlStatus::Active;
        snapshot.control.stopped_reason = Some("completion_evidence_deleted".to_string());
        changed = true;
    }
    if !changed {
        return Ok(Some(snapshot));
    }

    let previous_revision = snapshot.control.revision;
    snapshot.control.revision = snapshot.control.revision.saturating_add(1);
    snapshot.control.updated_at = updated_at.max(snapshot.control.updated_at);
    validate_snapshot_refs(transaction, &snapshot)?;
    update_snapshot_cas(
        transaction,
        &snapshot,
        previous_revision,
        "history_repaired",
        Some(&format!(
            "host:history_repaired:{}:{}",
            snapshot.control.task_id, previous_revision
        )),
        &snapshot.control.task_id,
    )?;
    Ok(Some(snapshot))
}

pub fn settle_run(
    connection: &mut Connection,
    conversation_id: &str,
    task_id: &str,
    run_id: &str,
    waiting_for_approval: bool,
    stopped_reason: Option<&str>,
    updated_at: i64,
) -> Result<Option<TaskStateSnapshot>, String> {
    let transaction = connection.transaction().map_err(database_error)?;
    let Some(mut snapshot) = load_task(&transaction, conversation_id, task_id)? else {
        transaction.commit().map_err(database_error)?;
        return Ok(None);
    };
    let mutation_id = format!(
        "host:settle:{run_id}:{}",
        if waiting_for_approval {
            "approval"
        } else {
            "terminal"
        }
    );
    if revision_for_mutation(&transaction, task_id, &mutation_id)?.is_some() {
        transaction.commit().map_err(database_error)?;
        return Ok(Some(snapshot));
    }
    if snapshot.control.current_run_id.as_deref() != Some(run_id) {
        transaction.commit().map_err(database_error)?;
        return Ok(Some(snapshot));
    }
    let previous_revision = snapshot.control.revision;
    snapshot.control.revision = snapshot.control.revision.saturating_add(1);
    snapshot.control.updated_at = updated_at.max(snapshot.control.updated_at);
    snapshot.control.stopped_reason = stopped_reason.map(str::to_string);
    if waiting_for_approval {
        if !snapshot.control.status.is_terminal() {
            snapshot.control.status = TaskControlStatus::WaitingUser;
        }
    } else {
        snapshot.control.current_run_id = None;
    }
    snapshot.validate().map_err(|error| error.to_string())?;
    update_snapshot_cas(
        &transaction,
        &snapshot,
        previous_revision,
        if waiting_for_approval {
            "waiting_for_approval"
        } else {
            "run_settled"
        },
        Some(&mutation_id),
        &snapshot.control.task_id,
    )?;
    transaction.commit().map_err(database_error)?;
    Ok(Some(snapshot))
}

pub fn resume_after_approval(
    connection: &mut Connection,
    conversation_id: &str,
    task_id: &str,
    run_id: &str,
    updated_at: i64,
) -> Result<Option<TaskStateSnapshot>, String> {
    let transaction = connection.transaction().map_err(database_error)?;
    let Some(mut snapshot) = load_task(&transaction, conversation_id, task_id)? else {
        transaction.commit().map_err(database_error)?;
        return Ok(None);
    };
    let mutation_id = format!("host:approval_resumed:{run_id}");
    if revision_for_mutation(&transaction, task_id, &mutation_id)?.is_some() {
        transaction.commit().map_err(database_error)?;
        return Ok(Some(snapshot));
    }
    if snapshot.control.current_run_id.as_deref() != Some(run_id) {
        return Err("审批恢复的 Task State 与 run 身份不一致。".to_string());
    }
    let previous_revision = snapshot.control.revision;
    snapshot.control.revision = snapshot.control.revision.saturating_add(1);
    if snapshot.control.status == TaskControlStatus::WaitingUser {
        snapshot.control.status = TaskControlStatus::Active;
    }
    snapshot.control.stopped_reason = None;
    snapshot.control.updated_at = updated_at.max(snapshot.control.updated_at);
    snapshot.validate().map_err(|error| error.to_string())?;
    update_snapshot_cas(
        &transaction,
        &snapshot,
        previous_revision,
        "approval_resumed",
        Some(&mutation_id),
        &snapshot.control.task_id,
    )?;
    transaction.commit().map_err(database_error)?;
    Ok(Some(snapshot))
}

pub fn mark_interrupted_tasks(
    connection: &mut Connection,
    interrupted_at: i64,
) -> Result<usize, String> {
    let transaction = connection.transaction().map_err(database_error)?;
    let task_ids = {
        let mut statement = transaction
            .prepare(
                "SELECT task_id, conversation_id
                 FROM task_control_states AS task
                 WHERE current_run_id IS NOT NULL
                   AND status IN ('active', 'waiting_user', 'blocked')
                   AND NOT EXISTS (
                       SELECT 1
                       FROM agent_pending_actions AS action
                       WHERE action.run_id = task.current_run_id
                         AND action.status IN ('pending', 'approved', 'executing')
                   )",
            )
            .map_err(database_error)?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(database_error)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(database_error)?;
        rows
    };
    for (task_id, conversation_id) in &task_ids {
        let mut snapshot = load_task(&transaction, conversation_id, task_id)?
            .ok_or_else(|| "待中断 Task State 已消失。".to_string())?;
        let Some(previous_run_id) = snapshot.control.current_run_id.take() else {
            continue;
        };
        let previous_revision = snapshot.control.revision;
        snapshot.control.revision = snapshot.control.revision.saturating_add(1);
        if snapshot.control.status == TaskControlStatus::WaitingUser {
            snapshot.control.status = TaskControlStatus::Active;
        }
        snapshot.control.stopped_reason = Some("run_interrupted".to_string());
        snapshot.control.updated_at = interrupted_at.max(snapshot.control.updated_at);
        snapshot.checkpoint.interruption = Some(TaskInterruptionState {
            interrupted: true,
            previous_run_id: Some(previous_run_id.clone()),
            reason: Some(
                "The previous process ended before the run reached a durable terminal state."
                    .to_string(),
            ),
            revalidate_work_item_ids: snapshot
                .checkpoint
                .work_items
                .iter()
                .filter(|item| item.status == TaskWorkItemStatus::InProgress)
                .map(|item| item.id.clone())
                .collect(),
            updated_at: interrupted_at,
        });
        snapshot.validate().map_err(|error| error.to_string())?;
        update_snapshot_cas(
            &transaction,
            &snapshot,
            previous_revision,
            "run_interrupted",
            Some(&format!("host:interrupted:{previous_run_id}")),
            &snapshot.control.task_id,
        )?;
    }
    transaction.commit().map_err(database_error)?;
    Ok(task_ids.len())
}

pub fn clone_latest_for_fork(
    connection: &Transaction<'_>,
    source_conversation_id: &str,
    target_conversation_id: &str,
    message_id_map: &std::collections::HashMap<String, String>,
    replacements: &std::collections::HashMap<String, String>,
    created_at: i64,
) -> Result<Option<TaskStateSnapshot>, String> {
    let Some(source) = load_latest_task(connection, source_conversation_id)? else {
        return Ok(None);
    };
    let Some(source_message_id) = message_id_map
        .get(&source.control.source_message_id)
        .cloned()
    else {
        // The task began after the fork cutoff and is not causally visible in the fork.
        return Ok(None);
    };
    let remap_ref = |reference: &ContextHistoryRef| -> Option<ContextHistoryRef> {
        match reference {
            ContextHistoryRef::Message { message_id } => message_id_map
                .get(message_id)
                .cloned()
                .map(ContextHistoryRef::message),
            ContextHistoryRef::TraceItem {
                assistant_message_id,
                sequence,
            } => message_id_map
                .get(assistant_message_id)
                .cloned()
                .map(|id| ContextHistoryRef::trace_item(id, *sequence)),
            ContextHistoryRef::Archive { archive_ref } => replacements
                .get(archive_ref)
                .cloned()
                .map(ContextHistoryRef::archive),
        }
    };

    let mut checkpoint = source.checkpoint.clone();
    checkpoint.history_evidence_refs = checkpoint
        .history_evidence_refs
        .iter()
        .filter_map(&remap_ref)
        .collect();
    for item in &mut checkpoint.work_items {
        item.evidence_refs = item.evidence_refs.iter().filter_map(&remap_ref).collect();
        if item.status == TaskWorkItemStatus::Completed && item.evidence_refs.is_empty() {
            item.status = TaskWorkItemStatus::InProgress;
            item.note = Some(
                "Completion evidence was outside the fork boundary; revalidation is required."
                    .to_string(),
            );
        }
    }
    for decision in &mut checkpoint.confirmed_decisions {
        decision.evidence_refs = decision
            .evidence_refs
            .iter()
            .filter_map(&remap_ref)
            .collect();
    }
    checkpoint.artifact_refs = checkpoint
        .artifact_refs
        .into_iter()
        .filter_map(|mut artifact| {
            artifact.reference = remap_ref(&artifact.reference)?;
            Some(artifact)
        })
        .collect();
    checkpoint.interruption = None;

    let status = if source.control.status == TaskControlStatus::Completed
        && (checkpoint.history_evidence_refs.is_empty()
            || checkpoint.work_items.iter().any(|item| {
                !matches!(
                    item.status,
                    TaskWorkItemStatus::Completed | TaskWorkItemStatus::Cancelled
                )
            })) {
        TaskControlStatus::Active
    } else {
        source.control.status
    };
    let task_id = format!("task-{}", Uuid::new_v4());
    let snapshot = TaskStateSnapshot {
        control: TaskControlState {
            schema_version: TASK_CONTROL_STATE_SCHEMA_VERSION,
            task_id,
            conversation_id: target_conversation_id.to_string(),
            objective: source.control.objective,
            source_message_id,
            status,
            revision: 1,
            current_run_id: None,
            stopped_reason: Some("forked_snapshot_requires_revalidation".to_string()),
            created_at,
            updated_at: created_at,
        },
        checkpoint,
    };
    validate_snapshot_refs(connection, &snapshot)?;
    let checkpoint_json =
        serde_json::to_string(&snapshot.checkpoint).map_err(|error| error.to_string())?;
    connection
        .execute(
            "INSERT INTO task_control_states (
                task_id, conversation_id, schema_version, objective, source_message_id,
                status, revision, current_run_id, stopped_reason, checkpoint_json,
                created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, NULL, ?7, ?8, ?9, ?9)",
            params![
                &snapshot.control.task_id,
                &snapshot.control.conversation_id,
                snapshot.control.schema_version,
                &snapshot.control.objective,
                &snapshot.control.source_message_id,
                snapshot.control.status.as_str(),
                &snapshot.control.stopped_reason,
                checkpoint_json,
                created_at,
            ],
        )
        .map_err(database_error)?;
    insert_revision(
        connection,
        &snapshot,
        "fork_cloned",
        None,
        &snapshot.control.task_id,
    )?;
    Ok(Some(snapshot))
}

#[allow(clippy::too_many_arguments)]
fn create_task_in_transaction(
    transaction: &Transaction<'_>,
    conversation_id: &str,
    source_message_id: &str,
    objective: &str,
    run_id: Option<&str>,
    created_at: i64,
    mutation_kind: &str,
    mutation_id: Option<&str>,
) -> Result<TaskStateSnapshot, String> {
    create_task_with_id_in_transaction(
        transaction,
        &format!("task-{}", Uuid::new_v4()),
        conversation_id,
        source_message_id,
        objective,
        run_id,
        created_at,
        mutation_kind,
        mutation_id,
    )
}

#[allow(clippy::too_many_arguments)]
fn create_task_with_id_in_transaction(
    transaction: &Transaction<'_>,
    task_id: &str,
    conversation_id: &str,
    source_message_id: &str,
    objective: &str,
    run_id: Option<&str>,
    created_at: i64,
    mutation_kind: &str,
    mutation_id: Option<&str>,
) -> Result<TaskStateSnapshot, String> {
    let snapshot = TaskStateSnapshot {
        control: TaskControlState {
            schema_version: TASK_CONTROL_STATE_SCHEMA_VERSION,
            task_id: task_id.to_string(),
            conversation_id: conversation_id.to_string(),
            objective: objective.trim().to_string(),
            source_message_id: source_message_id.to_string(),
            status: TaskControlStatus::Active,
            revision: 1,
            current_run_id: run_id.map(str::to_string),
            stopped_reason: None,
            created_at,
            updated_at: created_at,
        },
        checkpoint: TaskContinuationCheckpoint::default(),
    };
    snapshot.validate().map_err(|error| error.to_string())?;
    let checkpoint_json =
        serde_json::to_string(&snapshot.checkpoint).map_err(|error| error.to_string())?;
    transaction
        .execute(
            "INSERT INTO task_control_states (
                task_id, conversation_id, schema_version, objective, source_message_id,
                status, revision, current_run_id, stopped_reason, checkpoint_json,
                created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                &snapshot.control.task_id,
                &snapshot.control.conversation_id,
                snapshot.control.schema_version,
                &snapshot.control.objective,
                &snapshot.control.source_message_id,
                snapshot.control.status.as_str(),
                snapshot.control.revision,
                &snapshot.control.current_run_id,
                &snapshot.control.stopped_reason,
                checkpoint_json,
                snapshot.control.created_at,
                snapshot.control.updated_at,
            ],
        )
        .map_err(database_error)?;
    insert_revision(
        transaction,
        &snapshot,
        mutation_kind,
        mutation_id,
        &snapshot.control.task_id,
    )?;
    Ok(snapshot)
}

fn update_snapshot_cas(
    transaction: &Transaction<'_>,
    snapshot: &TaskStateSnapshot,
    expected_revision: u64,
    mutation_kind: &str,
    mutation_id: Option<&str>,
    result_task_id: &str,
) -> Result<(), String> {
    let checkpoint_json =
        serde_json::to_string(&snapshot.checkpoint).map_err(|error| error.to_string())?;
    let changed = transaction
        .execute(
            "UPDATE task_control_states
             SET objective = ?1, source_message_id = ?2, status = ?3, revision = ?4,
                 current_run_id = ?5, stopped_reason = ?6, checkpoint_json = ?7, updated_at = ?8
             WHERE task_id = ?9 AND conversation_id = ?10 AND revision = ?11",
            params![
                &snapshot.control.objective,
                &snapshot.control.source_message_id,
                snapshot.control.status.as_str(),
                snapshot.control.revision,
                &snapshot.control.current_run_id,
                &snapshot.control.stopped_reason,
                checkpoint_json,
                snapshot.control.updated_at,
                &snapshot.control.task_id,
                &snapshot.control.conversation_id,
                expected_revision,
            ],
        )
        .map_err(database_error)?;
    if changed != 1 {
        return Err(format!(
            "Task State CAS 冲突：taskId={} expectedRevision={expected_revision}。",
            snapshot.control.task_id
        ));
    }
    insert_revision(
        transaction,
        snapshot,
        mutation_kind,
        mutation_id,
        result_task_id,
    )
}

fn insert_revision(
    transaction: &Transaction<'_>,
    snapshot: &TaskStateSnapshot,
    mutation_kind: &str,
    mutation_id: Option<&str>,
    result_task_id: &str,
) -> Result<(), String> {
    let snapshot_json = serde_json::to_string(snapshot).map_err(|error| error.to_string())?;
    transaction
        .execute(
            "INSERT INTO task_control_state_revisions (
                task_id, revision, conversation_id, snapshot_json, mutation_kind,
                mutation_id, result_task_id, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                &snapshot.control.task_id,
                snapshot.control.revision,
                &snapshot.control.conversation_id,
                snapshot_json,
                mutation_kind,
                mutation_id,
                result_task_id,
                snapshot.control.updated_at,
            ],
        )
        .map_err(database_error)?;
    Ok(())
}

fn revision_for_mutation(
    connection: &Connection,
    task_id: &str,
    mutation_id: &str,
) -> Result<Option<TaskStateRevisionRecord>, String> {
    connection
        .query_row(
            "SELECT revision, snapshot_json, mutation_kind, mutation_id, result_task_id, created_at
             FROM task_control_state_revisions
             WHERE task_id = ?1 AND mutation_id = ?2",
            params![task_id, mutation_id],
            |row| {
                let snapshot_json = row.get::<_, String>(1)?;
                Ok(TaskStateRevisionRecord {
                    revision: row.get(0)?,
                    snapshot: serde_json::from_str(&snapshot_json).map_err(json_decode_error)?,
                    mutation_kind: row.get(2)?,
                    mutation_id: row.get(3)?,
                    result_task_id: row.get(4)?,
                    created_at: row.get(5)?,
                })
            },
        )
        .optional()
        .map_err(database_error)
}

fn load_revision_snapshot(
    connection: &Connection,
    conversation_id: &str,
    task_id: &str,
    revision: u64,
) -> Result<Option<TaskStateSnapshot>, String> {
    connection
        .query_row(
            "SELECT snapshot_json
             FROM task_control_state_revisions
             WHERE conversation_id = ?1 AND task_id = ?2 AND revision = ?3",
            params![conversation_id, task_id, revision],
            |row| {
                serde_json::from_str::<TaskStateSnapshot>(&row.get::<_, String>(0)?)
                    .map_err(json_decode_error)
            },
        )
        .optional()
        .map_err(database_error)
}

fn validate_snapshot_refs(
    connection: &Connection,
    snapshot: &TaskStateSnapshot,
) -> Result<(), String> {
    snapshot.validate().map_err(|error| error.to_string())?;
    if !message_exists(
        connection,
        &snapshot.control.conversation_id,
        &snapshot.control.source_message_id,
    )? {
        return Err("Task State sourceMessageId 不存在或跨 conversation。".to_string());
    }
    let mut seen = BTreeSet::new();
    for reference in snapshot.all_history_refs() {
        let key = serde_json::to_string(reference).map_err(|error| error.to_string())?;
        if seen.insert(key)
            && !history_ref_exists(connection, &snapshot.control.conversation_id, reference)?
        {
            return Err(format!(
                "Task State 历史证据引用不存在或跨 conversation：{reference:?}"
            ));
        }
    }
    let mut completion_refs = Vec::new();
    if snapshot.control.status == TaskControlStatus::Completed {
        completion_refs.extend(snapshot.checkpoint.history_evidence_refs.iter());
    }
    completion_refs.extend(
        snapshot
            .checkpoint
            .work_items
            .iter()
            .filter(|item| item.status == TaskWorkItemStatus::Completed)
            .flat_map(|item| item.evidence_refs.iter()),
    );
    for reference in completion_refs {
        if !is_valid_completion_evidence(connection, &snapshot.control.conversation_id, reference)?
        {
            return Err(format!(
                "失败、取消、拒绝、冲突或未完成记录不能作为完成证据：{reference:?}"
            ));
        }
    }
    Ok(())
}

fn retain_existing_refs(
    connection: &Connection,
    conversation_id: &str,
    refs: &mut Vec<ContextHistoryRef>,
) -> Result<bool, String> {
    let original_len = refs.len();
    let mut retained = Vec::with_capacity(original_len);
    for reference in std::mem::take(refs) {
        if history_ref_exists(connection, conversation_id, &reference)? {
            retained.push(reference);
        }
    }
    let changed = retained.len() != original_len;
    *refs = retained;
    Ok(changed)
}

fn message_exists(
    connection: &Connection,
    conversation_id: &str,
    message_id: &str,
) -> Result<bool, String> {
    connection
        .query_row(
            "SELECT 1 FROM messages WHERE conversation_id = ?1 AND id = ?2",
            params![conversation_id, message_id],
            |_| Ok(()),
        )
        .optional()
        .map(|value| value.is_some())
        .map_err(database_error)
}

fn history_ref_exists(
    connection: &Connection,
    conversation_id: &str,
    reference: &ContextHistoryRef,
) -> Result<bool, String> {
    let exists = match reference {
        ContextHistoryRef::Message { message_id } => connection
            .query_row(
                "SELECT 1 FROM messages WHERE conversation_id = ?1 AND id = ?2",
                params![conversation_id, message_id],
                |_| Ok(()),
            )
            .optional()
            .map_err(database_error)?
            .is_some(),
        ContextHistoryRef::TraceItem {
            assistant_message_id,
            sequence,
        } => connection
            .query_row(
                "SELECT 1
                 FROM conversation_turn_trace_items AS item
                 JOIN conversation_turn_traces AS trace
                   ON trace.assistant_message_id = item.assistant_message_id
                 WHERE trace.conversation_id = ?1
                   AND item.assistant_message_id = ?2
                   AND item.sequence = ?3",
                params![conversation_id, assistant_message_id, sequence],
                |_| Ok(()),
            )
            .optional()
            .map_err(database_error)?
            .is_some(),
        ContextHistoryRef::Archive { archive_ref } => connection
            .query_row(
                "SELECT 1 FROM conversation_history_blobs
                 WHERE conversation_id = ?1 AND archive_ref = ?2",
                params![conversation_id, archive_ref],
                |_| Ok(()),
            )
            .optional()
            .map_err(database_error)?
            .is_some(),
    };
    Ok(exists)
}

fn is_valid_completion_evidence(
    connection: &Connection,
    conversation_id: &str,
    reference: &ContextHistoryRef,
) -> Result<bool, String> {
    match reference {
        ContextHistoryRef::Message { message_id } => connection
            .query_row(
                "SELECT role, status FROM messages WHERE conversation_id = ?1 AND id = ?2",
                params![conversation_id, message_id],
                |row| {
                    let role = row.get::<_, String>(0)?;
                    let status = row.get::<_, Option<String>>(1)?;
                    Ok(matches!(role.as_str(), "user" | "assistant")
                        && !matches!(
                            status.as_deref(),
                            Some("pending" | "error" | "cancelled" | "rejected")
                        ))
                },
            )
            .optional()
            .map(|value| value.unwrap_or(false))
            .map_err(database_error),
        ContextHistoryRef::TraceItem {
            assistant_message_id,
            sequence,
        } => completion_trace_item_succeeded(
            connection,
            conversation_id,
            assistant_message_id,
            *sequence,
        ),
        ContextHistoryRef::Archive { archive_ref } => {
            let identity = connection
                .query_row(
                    "SELECT assistant_message_id, sequence
                     FROM conversation_history_blobs
                     WHERE conversation_id = ?1 AND archive_ref = ?2",
                    params![conversation_id, archive_ref],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, u64>(1)?)),
                )
                .optional()
                .map_err(database_error)?;
            let Some((assistant_message_id, sequence)) = identity else {
                return Ok(false);
            };
            completion_trace_item_succeeded(
                connection,
                conversation_id,
                &assistant_message_id,
                sequence,
            )
        }
    }
}

fn completion_trace_item_succeeded(
    connection: &Connection,
    conversation_id: &str,
    assistant_message_id: &str,
    sequence: u64,
) -> Result<bool, String> {
    connection
        .query_row(
            "SELECT item.item_kind,
                    json_extract(item.item_json, '$.status'),
                    json_extract(item.item_json, '$.success')
             FROM conversation_turn_trace_items AS item
             JOIN conversation_turn_traces AS trace
               ON trace.assistant_message_id = item.assistant_message_id
             WHERE trace.conversation_id = ?1
               AND item.assistant_message_id = ?2
               AND item.sequence = ?3",
            params![conversation_id, assistant_message_id, sequence],
            |row| {
                Ok(row.get::<_, String>(0)? == "tool_result"
                    && row.get::<_, Option<String>>(1)?.as_deref() == Some("succeeded")
                    && row.get::<_, Option<bool>>(2)? == Some(true))
            },
        )
        .optional()
        .map(|value| value.unwrap_or(false))
        .map_err(database_error)
}

fn latest_user_message_id(
    connection: &Connection,
    conversation_id: &str,
) -> Result<Option<String>, String> {
    connection
        .query_row(
            "SELECT id FROM messages
             WHERE conversation_id = ?1 AND role = 'user'
             ORDER BY position DESC LIMIT 1",
            [conversation_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(database_error)
}

fn decode_snapshot_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<TaskStateSnapshot> {
    let status = match row.get::<_, String>(5)?.as_str() {
        "active" => TaskControlStatus::Active,
        "waiting_user" => TaskControlStatus::WaitingUser,
        "blocked" => TaskControlStatus::Blocked,
        "completed" => TaskControlStatus::Completed,
        "superseded" => TaskControlStatus::Superseded,
        "cancelled" => TaskControlStatus::Cancelled,
        other => {
            return Err(rusqlite::Error::FromSqlConversionFailure(
                5,
                rusqlite::types::Type::Text,
                format!("unknown Task State status `{other}`").into(),
            ))
        }
    };
    let checkpoint_json = row.get::<_, String>(9)?;
    let checkpoint = serde_json::from_str::<TaskContinuationCheckpoint>(&checkpoint_json)
        .map_err(json_decode_error)?;
    let snapshot = TaskStateSnapshot {
        control: TaskControlState {
            schema_version: row.get(0)?,
            task_id: row.get(1)?,
            conversation_id: row.get(2)?,
            objective: row.get(3)?,
            source_message_id: row.get(4)?,
            status,
            revision: row.get(6)?,
            current_run_id: row.get(7)?,
            stopped_reason: row.get(8)?,
            created_at: row.get(10)?,
            updated_at: row.get(11)?,
        },
        checkpoint,
    };
    snapshot.validate().map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            9,
            rusqlite::types::Type::Text,
            error.to_string().into(),
        )
    })?;
    Ok(snapshot)
}

fn json_decode_error(error: serde_json::Error) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error))
}

fn database_error(error: rusqlite::Error) -> String {
    format!("Task State 数据库操作失败：{error}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::migrations;

    fn connection() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch("PRAGMA foreign_keys = ON;")
            .unwrap();
        migrations::run_migrations(&connection).unwrap();
        connection
            .execute(
                "INSERT INTO conversations (
                    id, project_id, model_id, title, created_at, updated_at,
                    pinned_at, archived_at, unread_at
                 ) VALUES ('conversation-1', NULL, NULL, 'Task', 1, 1, NULL, NULL, NULL)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO messages (
                    id, conversation_id, role, content, status, agent_run_json,
                    ui_state_json, created_at, position
                 ) VALUES ('message-1', 'conversation-1', 'user', 'Build task state',
                           'sent', NULL, NULL, 1, 0)",
                [],
            )
            .unwrap();
        connection
    }

    #[test]
    fn snapshot_and_append_only_revisions_use_cas() {
        let mut connection = connection();
        let initial = begin_or_resume_for_turn(
            &mut connection,
            "conversation-1",
            "message-1",
            "Build task state",
            "run-1",
            2,
        )
        .unwrap();
        let patched = patch_task(
            &mut connection,
            "conversation-1",
            &initial.control.task_id,
            1,
            &[TaskStatePatchOperation::SetCurrentPhase {
                value: Some("Persistence".to_string()),
            }],
            Some("run-1"),
            "call-1",
            3,
        )
        .unwrap();
        assert_eq!(patched.control.revision, 2);
        assert_eq!(
            list_revisions(&connection, "conversation-1", &patched.control.task_id)
                .unwrap()
                .len(),
            2
        );
        let conflict = patch_task(
            &mut connection,
            "conversation-1",
            &patched.control.task_id,
            1,
            &[TaskStatePatchOperation::SetCurrentPhase { value: None }],
            Some("run-1"),
            "call-2",
            4,
        )
        .unwrap_err();
        assert!(conflict.contains("revision 冲突"));
    }

    #[test]
    fn failed_tool_result_cannot_be_completion_evidence() {
        let mut connection = connection();
        connection
            .execute(
                "INSERT INTO messages (
                    id, conversation_id, role, content, status, agent_run_json,
                    ui_state_json, created_at, position
                 ) VALUES ('assistant-1', 'conversation-1', 'assistant', '', 'error',
                           NULL, NULL, 2, 1)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO conversation_turn_traces (
                    assistant_message_id, schema_version, run_id, conversation_id,
                    terminal_status, terminal_error, truncated, created_at, updated_at,
                    completed_at
                 ) VALUES ('assistant-1', 3, 'run-1', 'conversation-1',
                           'failed', 'failed', 0, 2, 2, 2)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO conversation_turn_trace_items (
                    assistant_message_id, sequence, item_kind, item_json
                 ) VALUES (
                    'assistant-1', 0, 'tool_result',
                    '{\"type\":\"tool_result\",\"sequence\":0,\"callId\":\"c\",\"tool\":\"read_file\",\"status\":\"failed\",\"success\":false,\"observation\":{},\"approvalStatus\":\"not_required\",\"error\":\"failed\",\"truncated\":false}'
                 )",
                [],
            )
            .unwrap();
        let initial = begin_or_resume_for_turn(
            &mut connection,
            "conversation-1",
            "message-1",
            "Build task state",
            "run-1",
            2,
        )
        .unwrap();
        let evidence = ContextHistoryRef::trace_item("assistant-1", 0);
        let error = patch_task(
            &mut connection,
            "conversation-1",
            &initial.control.task_id,
            initial.control.revision,
            &[
                TaskStatePatchOperation::SetHistoryEvidenceRefs {
                    refs: vec![evidence],
                },
                TaskStatePatchOperation::SetStatus {
                    status: TaskControlStatus::Completed,
                    stopped_reason: None,
                },
            ],
            Some("run-1"),
            "call-complete",
            3,
        )
        .unwrap_err();
        assert!(error.contains("不能作为完成证据"));
    }

    #[test]
    fn evidence_refs_are_strictly_scoped_to_the_task_conversation() {
        let mut connection = connection();
        connection
            .execute(
                "INSERT INTO conversations (
                    id, project_id, model_id, title, created_at, updated_at,
                    pinned_at, archived_at, unread_at
                 ) VALUES ('conversation-2', NULL, NULL, 'Other', 1, 1, NULL, NULL, NULL)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO messages (
                    id, conversation_id, role, content, status, agent_run_json,
                    ui_state_json, created_at, position
                 ) VALUES ('other-message', 'conversation-2', 'user', 'Other task',
                           'sent', NULL, NULL, 1, 0)",
                [],
            )
            .unwrap();
        let initial = begin_or_resume_for_turn(
            &mut connection,
            "conversation-1",
            "message-1",
            "Build task state",
            "run-1",
            2,
        )
        .unwrap();
        let error = patch_task(
            &mut connection,
            "conversation-1",
            &initial.control.task_id,
            initial.control.revision,
            &[TaskStatePatchOperation::SetHistoryEvidenceRefs {
                refs: vec![ContextHistoryRef::message("other-message")],
            }],
            Some("run-1"),
            "cross-conversation-ref",
            3,
        )
        .unwrap_err();

        assert!(error.contains("跨 conversation"));
    }

    #[test]
    fn todo_completion_updates_authoritative_task_status_with_evidence() {
        let mut connection = connection();
        let initial = begin_or_resume_for_turn(
            &mut connection,
            "conversation-1",
            "message-1",
            "Build task state",
            "run-1",
            2,
        )
        .unwrap();
        let completed = replace_work_items(
            &mut connection,
            "conversation-1",
            &initial.control.task_id,
            initial.control.revision,
            vec![TaskWorkItem {
                id: "work-1".to_string(),
                title: "Persist snapshot".to_string(),
                status: TaskWorkItemStatus::Completed,
                note: None,
                evidence_refs: vec![ContextHistoryRef::message("message-1")],
            }],
            "run-1",
            "todo-call-1",
            3,
        )
        .unwrap();

        assert_eq!(completed.control.status, TaskControlStatus::Completed);
        assert_eq!(
            completed.checkpoint.history_evidence_refs,
            vec![ContextHistoryRef::message("message-1")]
        );
    }

    #[test]
    fn rollback_creates_a_new_revision_instead_of_rewriting_history() {
        let mut connection = connection();
        let initial = begin_or_resume_for_turn(
            &mut connection,
            "conversation-1",
            "message-1",
            "Build task state",
            "run-1",
            2,
        )
        .unwrap();
        let patched = patch_task(
            &mut connection,
            "conversation-1",
            &initial.control.task_id,
            initial.control.revision,
            &[TaskStatePatchOperation::SetCurrentPhase {
                value: Some("Persistence".to_string()),
            }],
            Some("run-1"),
            "patch-before-rollback",
            3,
        )
        .unwrap();
        let rolled_back = rollback_task(
            &mut connection,
            "conversation-1",
            &initial.control.task_id,
            patched.control.revision,
            initial.control.revision,
            "host:rollback:1",
            4,
        )
        .unwrap();

        assert_eq!(rolled_back.control.revision, 3);
        assert_eq!(rolled_back.checkpoint.current_phase, None);
        let revisions =
            list_revisions(&connection, "conversation-1", &initial.control.task_id).unwrap();
        assert_eq!(revisions.len(), 3);
        assert_eq!(revisions[2].mutation_kind, "rollback");
    }

    #[test]
    fn deleting_history_repairs_refs_and_downgrades_completion() {
        let mut connection = connection();
        connection
            .execute(
                "INSERT INTO messages (
                    id, conversation_id, role, content, status, agent_run_json,
                    ui_state_json, created_at, position
                 ) VALUES ('message-2', 'conversation-1', 'user', 'Continue safely',
                           'sent', NULL, NULL, 2, 1)",
                [],
            )
            .unwrap();
        let initial = begin_or_resume_for_turn(
            &mut connection,
            "conversation-1",
            "message-1",
            "Build task state",
            "run-1",
            3,
        )
        .unwrap();
        let completed = replace_work_items(
            &mut connection,
            "conversation-1",
            &initial.control.task_id,
            initial.control.revision,
            vec![TaskWorkItem {
                id: "work-1".to_string(),
                title: "Persist snapshot".to_string(),
                status: TaskWorkItemStatus::Completed,
                note: None,
                evidence_refs: vec![ContextHistoryRef::message("message-1")],
            }],
            "run-1",
            "todo-completed",
            4,
        )
        .unwrap();
        assert_eq!(completed.control.status, TaskControlStatus::Completed);

        let transaction = connection.transaction().unwrap();
        transaction
            .execute("DELETE FROM messages WHERE id = 'message-1'", [])
            .unwrap();
        let repaired = repair_after_history_mutation(&transaction, "conversation-1", 5)
            .unwrap()
            .unwrap();
        transaction.commit().unwrap();

        assert_eq!(repaired.control.source_message_id, "message-2");
        assert_eq!(repaired.control.status, TaskControlStatus::Active);
        assert!(repaired.checkpoint.history_evidence_refs.is_empty());
        assert_eq!(
            repaired.checkpoint.work_items[0].status,
            TaskWorkItemStatus::InProgress
        );
        assert_eq!(
            list_revisions(&connection, "conversation-1", &initial.control.task_id)
                .unwrap()
                .last()
                .unwrap()
                .mutation_kind,
            "history_repaired"
        );
    }

    #[test]
    fn supersede_is_atomic_and_idempotently_returns_the_replacement() {
        let mut connection = connection();
        let initial = begin_or_resume_for_turn(
            &mut connection,
            "conversation-1",
            "message-1",
            "Old objective",
            "run-1",
            2,
        )
        .unwrap();
        let operations = [TaskStatePatchOperation::Supersede {
            objective: "New objective".to_string(),
        }];
        let replacement = patch_task(
            &mut connection,
            "conversation-1",
            &initial.control.task_id,
            initial.control.revision,
            &operations,
            Some("run-1"),
            "supersede-call",
            3,
        )
        .unwrap();
        let retried = patch_task(
            &mut connection,
            "conversation-1",
            &initial.control.task_id,
            initial.control.revision,
            &operations,
            Some("run-1"),
            "supersede-call",
            3,
        )
        .unwrap();

        assert_ne!(replacement.control.task_id, initial.control.task_id);
        assert_eq!(replacement.control.objective, "New objective");
        assert_eq!(retried.control.task_id, replacement.control.task_id);
        let old = load_task(&connection, "conversation-1", &initial.control.task_id)
            .unwrap()
            .unwrap();
        assert_eq!(old.control.status, TaskControlStatus::Superseded);
    }

    #[test]
    fn fork_clones_snapshot_and_rebinds_history_refs() {
        let mut connection = connection();
        connection
            .execute(
                "INSERT INTO conversations (
                    id, project_id, model_id, title, created_at, updated_at,
                    pinned_at, archived_at, unread_at
                 ) VALUES ('conversation-fork', NULL, NULL, 'Fork', 1, 1, NULL, NULL, NULL)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO messages (
                    id, conversation_id, role, content, status, agent_run_json,
                    ui_state_json, created_at, position
                 ) VALUES ('fork-message-1', 'conversation-fork', 'user', 'Build task state',
                           'sent', NULL, NULL, 1, 0)",
                [],
            )
            .unwrap();
        let initial = begin_or_resume_for_turn(
            &mut connection,
            "conversation-1",
            "message-1",
            "Build task state",
            "run-1",
            2,
        )
        .unwrap();
        let patched = patch_task(
            &mut connection,
            "conversation-1",
            &initial.control.task_id,
            initial.control.revision,
            &[TaskStatePatchOperation::SetHistoryEvidenceRefs {
                refs: vec![ContextHistoryRef::message("message-1")],
            }],
            Some("run-1"),
            "add-evidence",
            3,
        )
        .unwrap();

        let transaction = connection.transaction().unwrap();
        let cloned = clone_latest_for_fork(
            &transaction,
            "conversation-1",
            "conversation-fork",
            &std::collections::HashMap::from([(
                "message-1".to_string(),
                "fork-message-1".to_string(),
            )]),
            &std::collections::HashMap::new(),
            4,
        )
        .unwrap()
        .unwrap();
        transaction.commit().unwrap();

        assert_ne!(cloned.control.task_id, patched.control.task_id);
        assert_eq!(cloned.control.conversation_id, "conversation-fork");
        assert_eq!(cloned.control.source_message_id, "fork-message-1");
        assert_eq!(
            cloned.checkpoint.history_evidence_refs,
            vec![ContextHistoryRef::message("fork-message-1")]
        );
        assert!(cloned.control.current_run_id.is_none());
    }

    #[test]
    fn startup_marks_unapproved_active_run_interrupted() {
        let mut connection = connection();
        let initial = begin_or_resume_for_turn(
            &mut connection,
            "conversation-1",
            "message-1",
            "Build task state",
            "run-1",
            2,
        )
        .unwrap();
        assert_eq!(mark_interrupted_tasks(&mut connection, 4).unwrap(), 1);
        let interrupted = load_task(&connection, "conversation-1", &initial.control.task_id)
            .unwrap()
            .unwrap();
        assert!(interrupted.control.current_run_id.is_none());
        assert_eq!(
            interrupted.control.stopped_reason.as_deref(),
            Some("run_interrupted")
        );
        assert!(interrupted
            .checkpoint
            .interruption
            .as_ref()
            .is_some_and(|state| state.interrupted));
    }

    #[test]
    fn startup_retires_orphaned_waiting_approval_task_state() {
        let mut connection = connection();
        let initial = begin_or_resume_for_turn(
            &mut connection,
            "conversation-1",
            "message-1",
            "Build task state",
            "run-1",
            2,
        )
        .unwrap();
        let waiting = settle_run(
            &mut connection,
            "conversation-1",
            &initial.control.task_id,
            "run-1",
            true,
            Some("waiting_for_approval"),
            3,
        )
        .unwrap()
        .unwrap();
        assert_eq!(waiting.control.status, TaskControlStatus::WaitingUser);

        assert_eq!(mark_interrupted_tasks(&mut connection, 4).unwrap(), 1);
        let recovered = load_task(&connection, "conversation-1", &initial.control.task_id)
            .unwrap()
            .unwrap();
        assert_eq!(recovered.control.status, TaskControlStatus::Active);
        assert!(recovered.control.current_run_id.is_none());
        assert!(recovered
            .checkpoint
            .interruption
            .is_some_and(|state| state.interrupted));
    }
}
