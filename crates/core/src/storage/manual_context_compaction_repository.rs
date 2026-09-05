//! Durable manual maintenance operations. No operation creates or owns a chat message.
use crate::storage::models::{
    ManualContextCompactionOperation, ManualContextCompactionUsageRecord,
};
use crate::storage::{context_compaction_receipt_repository, context_compaction_repository};
use crate::{
    ContextCompactionPrefix, ContextCompactionReceipt, ContextCompactionReceiptStatus,
    ContextCompactionSummary, ContextCompactionSummaryDraft, ModelRequestObservation,
};
use rusqlite::{params, Connection, OptionalExtension};

fn json_error(error: impl std::fmt::Display) -> String {
    format!("手动压缩记录无效：{error}")
}

fn validate(operation: &ManualContextCompactionOperation) -> Result<(), String> {
    for value in [
        &operation.operation_id,
        &operation.request_id,
        &operation.conversation_id,
    ] {
        if value.trim() != value || value.is_empty() || value.len() > 1024 {
            return Err("手动压缩操作身份无效。".to_string());
        }
    }
    if !matches!(
        operation.status.as_str(),
        "running" | "completed" | "noop" | "cancelled" | "failed" | "interrupted"
    ) || !matches!(
        operation.phase.as_str(),
        "preparing" | "generating" | "committing"
    ) || operation.started_at < 0
        || operation.updated_at < operation.started_at
        || (operation.status == "running") != operation.completed_at.is_none()
        || operation
            .completed_at
            .is_some_and(|at| at < operation.started_at)
        || (operation.status == "completed" && operation.summary_id.is_none())
    {
        return Err("手动压缩操作状态无效。".to_string());
    }
    Ok(())
}

pub fn get(
    connection: &Connection,
    conversation_id: &str,
    operation_id: Option<&str>,
) -> Result<Option<ManualContextCompactionOperation>, String> {
    let json = connection
        .query_row(
            "SELECT operation_json FROM manual_context_compaction_operations
         WHERE conversation_id = ?1 AND (?2 IS NULL OR operation_id = ?2)
         ORDER BY started_at DESC, operation_id DESC LIMIT 1",
            params![conversation_id, operation_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(json_error)?;
    json.map(|json| serde_json::from_str(&json).map_err(json_error))
        .transpose()
}

pub fn list(
    connection: &Connection,
    conversation_id: &str,
) -> Result<Vec<ManualContextCompactionOperation>, String> {
    let mut statement = connection.prepare("SELECT operation_json FROM manual_context_compaction_operations WHERE conversation_id = ?1 ORDER BY started_at, operation_id").map_err(json_error)?;
    let rows = statement
        .query_map([conversation_id], |row| row.get::<_, String>(0))
        .map_err(json_error)?;
    rows.map(|row| serde_json::from_str(&row.map_err(json_error)?).map_err(json_error))
        .collect()
}

pub fn claim(
    connection: &mut Connection,
    operation: &ManualContextCompactionOperation,
) -> Result<ManualContextCompactionOperation, String> {
    validate(operation)?;
    let transaction = connection.transaction().map_err(json_error)?;
    let existing = transaction.query_row("SELECT operation_json FROM manual_context_compaction_operations WHERE request_id = ?1 OR operation_id = ?2", params![operation.request_id, operation.operation_id], |row| row.get::<_, String>(0)).optional().map_err(json_error)?;
    if let Some(existing) = existing {
        let existing: ManualContextCompactionOperation =
            serde_json::from_str(&existing).map_err(json_error)?;
        if existing.request_id != operation.request_id
            || existing.conversation_id != operation.conversation_id
            || existing.operation_id != operation.operation_id
        {
            return Err("manual_context_compaction_request_conflict".to_string());
        }
        return Ok(existing);
    }
    if operation.status != "running" || operation.phase != "preparing" {
        return Err("新手动压缩必须从 preparing 状态开始。".to_string());
    }
    let available = transaction.query_row("SELECT EXISTS(SELECT 1 FROM conversations WHERE id = ?1 AND archived_at IS NULL) AND NOT EXISTS(SELECT 1 FROM agent_nodes WHERE conversation_id = ?1 AND (parent_agent_id IS NOT NULL OR lifecycle != 'active'))", [&operation.conversation_id], |row| row.get::<_, bool>(0)).map_err(json_error)?;
    let busy = transaction.query_row("WITH scope(conversation_id) AS (SELECT ?1 UNION SELECT conversation_id FROM agent_nodes WHERE root_conversation_id=?1) SELECT EXISTS(
        SELECT 1 FROM conversation_turn_traces WHERE conversation_id IN (SELECT conversation_id FROM scope) AND terminal_status = 'in_progress'
        UNION ALL SELECT 1 FROM agent_command_sessions WHERE conversation_id IN (SELECT conversation_id FROM scope) AND status IN ('starting','running')
        UNION ALL SELECT 1 FROM agent_pending_actions WHERE conversation_id IN (SELECT conversation_id FROM scope) AND status IN ('pending','approved','executing')
        UNION ALL SELECT 1 FROM context_compaction_receipts WHERE conversation_id IN (SELECT conversation_id FROM scope) AND status = 'in_progress'
        UNION ALL SELECT 1 FROM manual_context_compaction_operations WHERE conversation_id IN (SELECT conversation_id FROM scope) AND status = 'running'
        UNION ALL SELECT 1 FROM agent_wake_requests WHERE agent_id IN (SELECT agent_id FROM agent_nodes WHERE conversation_id IN (SELECT conversation_id FROM scope)) AND status IN ('queued','claimed','running','waiting_for_approval')
    )", [&operation.conversation_id], |row| row.get::<_, bool>(0)).map_err(json_error)?;
    if !available || busy {
        return Err("manual_context_compaction_conversation_busy_or_unavailable".to_string());
    }
    transaction.execute("INSERT INTO manual_context_compaction_operations (operation_id,request_id,conversation_id,status,phase,summary_id,operation_json,started_at,updated_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)", params![operation.operation_id,operation.request_id,operation.conversation_id,operation.status,operation.phase,operation.summary_id,serde_json::to_string(operation).map_err(json_error)?,operation.started_at,operation.updated_at]).map_err(json_error)?;
    transaction.commit().map_err(json_error)?;
    Ok(operation.clone())
}

/// Terminal states are immutable. Cancellation and commit serialize under the same connection.
pub fn update(
    connection: &Connection,
    operation: &ManualContextCompactionOperation,
) -> Result<ManualContextCompactionOperation, String> {
    validate(operation)?;
    let existing = get(
        connection,
        &operation.conversation_id,
        Some(&operation.operation_id),
    )?
    .ok_or_else(|| "找不到手动压缩操作。".to_string())?;
    if existing.request_id != operation.request_id || existing.started_at != operation.started_at {
        return Err("手动压缩操作身份不一致。".to_string());
    }
    if existing.status != "running" {
        return Ok(existing);
    }
    if operation.updated_at < existing.updated_at {
        return Ok(existing);
    }
    connection.execute("UPDATE manual_context_compaction_operations SET status=?2,phase=?3,summary_id=?4,operation_json=?5,updated_at=?6 WHERE operation_id=?1 AND status='running'", params![operation.operation_id,operation.status,operation.phase,operation.summary_id,serde_json::to_string(operation).map_err(json_error)?,operation.updated_at]).map_err(json_error)?;
    Ok(operation.clone())
}

pub fn interrupt_running(connection: &mut Connection, now: i64) -> Result<(), String> {
    let transaction = connection.transaction().map_err(json_error)?;
    let rows = {
        let mut statement = transaction.prepare("SELECT operation_json FROM manual_context_compaction_operations WHERE status='running'").map_err(json_error)?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(json_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(json_error)?;
        rows
    };
    for json in rows {
        let mut operation: ManualContextCompactionOperation =
            serde_json::from_str(&json).map_err(json_error)?;
        operation.status = "interrupted".to_string();
        operation.updated_at = now.max(operation.updated_at);
        operation.completed_at = Some(operation.updated_at);
        operation.error = Some("应用重启，压缩未完成；原上下文继续有效。".to_string());
        update(&transaction, &operation)?;
    }
    transaction.commit().map_err(json_error)
}

/// Persist actual provider usage before attempting the summary commit. Never replace an earlier
/// record or attach this request to the last assistant message's usage row.
pub fn record_usage(
    connection: &Connection,
    record: &ManualContextCompactionUsageRecord,
) -> Result<(), String> {
    if get(
        connection,
        &record.conversation_id,
        Some(&record.operation_id),
    )?
    .is_none()
    {
        return Err("手动压缩用量缺少操作归属。".to_string());
    }
    let json = serde_json::to_string(record).map_err(json_error)?;
    let existing = connection
        .query_row(
            "SELECT record_json FROM manual_context_compaction_usage_records WHERE operation_id=?1",
            [&record.operation_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(json_error)?;
    if let Some(existing) = existing {
        return if existing == json {
            Ok(())
        } else {
            Err("manual_context_compaction_usage_conflict".to_string())
        };
    }
    connection.execute("INSERT INTO manual_context_compaction_usage_records (operation_id,conversation_id,model_id,model_name,created_at,input_tokens,output_tokens,output_thinking_tokens,total_tokens,cached_input_tokens,cache_creation_input_tokens,billable_request_count,estimated_cost,record_json) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)", params![record.operation_id,record.conversation_id,record.model_id,record.model_name,record.created_at,record.input_tokens,record.output_tokens,record.output_thinking_tokens,record.total_tokens,record.cached_input_tokens,record.cache_creation_input_tokens,record.billable_request_count,record.estimated_cost,json]).map_err(json_error)?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn commit_if_current(
    connection: &mut Connection,
    prefix: &ContextCompactionPrefix,
    draft: ContextCompactionSummaryDraft,
    receipt: &ContextCompactionReceipt,
    observation: &ModelRequestObservation,
    operation: &ManualContextCompactionOperation,
    expected_model_id: &str,
    expected_provider_protocol_revision: &str,
) -> Result<Option<ContextCompactionSummary>, String> {
    validate(operation)?;
    receipt.validate().map_err(json_error)?;
    context_compaction_repository::validate_commit_inputs(prefix, &draft).map_err(json_error)?;
    if operation.status != "completed"
        || operation.conversation_id != prefix.conversation_id
        || operation.summary_id.as_deref() != Some(draft.id.as_str())
        || receipt.operation_id != operation.operation_id
        || receipt.status != ContextCompactionReceiptStatus::Applied
        || receipt.source_revision.as_deref() != Some(prefix.source_revision.as_str())
        || receipt.summary_id != operation.summary_id
        || receipt.conversation_id != prefix.conversation_id
    {
        return Err("手动压缩提交身份不一致。".to_string());
    }
    let transaction = connection.transaction().map_err(json_error)?;
    let existing = get(
        &transaction,
        &operation.conversation_id,
        Some(&operation.operation_id),
    )?
    .ok_or_else(|| "找不到手动压缩操作。".to_string())?;
    if existing.status != "running" {
        return Ok(None);
    }
    let model_current = transaction.query_row("SELECT EXISTS(SELECT 1 FROM conversations c JOIN models m ON m.id=?2 WHERE c.id=?1 AND c.archived_at IS NULL AND c.model_id=?2 AND m.provider_protocol_revision=?3)", params![operation.conversation_id,expected_model_id,expected_provider_protocol_revision], |row| row.get::<_,bool>(0)).map_err(json_error)?;
    if !model_current {
        return Ok(None);
    }
    let latest_message = transaction
        .query_row(
            "SELECT id FROM messages WHERE conversation_id=?1 ORDER BY position DESC LIMIT 1",
            [&operation.conversation_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(json_error)?;
    if latest_message.as_deref() != Some(prefix.covered_through.message_id()) {
        return Ok(None);
    }
    let summary = match context_compaction_repository::commit_prefix_replacement_in_transaction(
        &transaction,
        prefix,
        draft,
        &receipt.assistant_message_id,
    ) {
        Ok(summary) => summary,
        Err(error) if error.is_stale() => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    context_compaction_receipt_repository::record_receipt_in_connection(
        &transaction,
        receipt,
        Some(observation),
    )
    .map_err(json_error)?;
    let result = update(&transaction, operation)?;
    if result != *operation {
        return Err("手动压缩提交状态已变化。".to_string());
    }
    transaction.commit().map_err(json_error)?;
    Ok(Some(summary))
}

pub(crate) fn insert_cloned_completed(
    connection: &Connection,
    operation: &ManualContextCompactionOperation,
) -> Result<(), String> {
    validate(operation)?;
    if operation.status != "completed" {
        return Err("仅复制已完成的手动压缩边界。".to_string());
    }
    connection.execute("INSERT INTO manual_context_compaction_operations (operation_id,request_id,conversation_id,status,phase,summary_id,operation_json,started_at,updated_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)", params![operation.operation_id,operation.request_id,operation.conversation_id,operation.status,operation.phase,operation.summary_id,serde_json::to_string(operation).map_err(json_error)?,operation.started_at,operation.updated_at]).map_err(json_error)?;
    Ok(())
}

#[cfg(test)]
mod tests;
