use crate::storage::model_request_observation_repository;
use crate::{
    ContextCompactionReceipt, ContextCompactionReceiptStage, ContextCompactionReceiptStatus,
    ModelRequestObservation,
};
use rusqlite::{params, Connection, OptionalExtension};
use std::error::Error;
use std::fmt::{Display, Formatter};

#[derive(Debug)]
pub enum ContextCompactionReceiptRepositoryError {
    Database(rusqlite::Error),
    Invalid(String),
}

impl Display for ContextCompactionReceiptRepositoryError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Database(error) => write!(formatter, "本地数据库操作失败：{error}"),
            Self::Invalid(message) => write!(formatter, "上下文压缩 receipt 无效：{message}"),
        }
    }
}

impl Error for ContextCompactionReceiptRepositoryError {}

impl From<rusqlite::Error> for ContextCompactionReceiptRepositoryError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Database(error)
    }
}

pub fn record_receipt(
    connection: &mut Connection,
    receipt: &ContextCompactionReceipt,
    observation: Option<&ModelRequestObservation>,
) -> Result<(), ContextCompactionReceiptRepositoryError> {
    let transaction = connection.transaction()?;
    record_receipt_in_connection(&transaction, receipt, observation)?;
    transaction.commit()?;
    Ok(())
}

/// Writes one receipt transition inside an existing caller-owned transaction.
///
/// This is used by the successful compaction commit so the provider observation, immutable
/// summary, active head and terminal receipt become visible together or not at all.
pub(crate) fn record_receipt_in_connection(
    connection: &Connection,
    receipt: &ContextCompactionReceipt,
    observation: Option<&ModelRequestObservation>,
) -> Result<(), ContextCompactionReceiptRepositoryError> {
    receipt
        .validate()
        .map_err(|error| ContextCompactionReceiptRepositoryError::Invalid(error.to_string()))?;
    if let Some(observation) = observation {
        model_request_observation_repository::insert_observation(connection, observation)
            .map_err(|error| ContextCompactionReceiptRepositoryError::Invalid(error.to_string()))?;
    }
    validate_observation_reference(receipt, observation)?;

    let existing = get_receipt(connection, &receipt.operation_id)?;
    match existing {
        None => {
            if receipt.status != ContextCompactionReceiptStatus::InProgress
                || receipt.stage != ContextCompactionReceiptStage::Planned
            {
                return Err(ContextCompactionReceiptRepositoryError::Invalid(
                    "压缩 receipt 必须先以 planned/in_progress 状态创建。".to_string(),
                ));
            }
            insert_receipt(connection, receipt)?;
        }
        Some(existing) if existing == *receipt => {}
        Some(existing) => {
            validate_transition(&existing, receipt)?;
            update_receipt(connection, receipt)?;
        }
    }
    Ok(())
}

pub fn get_receipt(
    connection: &Connection,
    operation_id: &str,
) -> Result<Option<ContextCompactionReceipt>, ContextCompactionReceiptRepositoryError> {
    let row = connection
        .query_row(
            "SELECT status, stage, receipt_json
             FROM context_compaction_receipts
             WHERE operation_id = ?1",
            [operation_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()?;
    row.map(decode_receipt).transpose()
}

pub fn list_provider_transition_receipts(
    connection: &Connection,
    conversation_id: &str,
    operation_id: Option<&str>,
    limit: usize,
) -> Result<Vec<ContextCompactionReceipt>, ContextCompactionReceiptRepositoryError> {
    let bounded_limit = i64::try_from(limit.clamp(1, 100)).unwrap_or(100);
    let rows = if let Some(operation_id) = operation_id {
        let mut statement = connection.prepare(
            "SELECT status, stage, receipt_json
             FROM context_compaction_receipts
             WHERE conversation_id = ?1 AND operation_id = ?2
             ORDER BY started_at DESC, operation_id DESC
             LIMIT ?3",
        )?;
        let rows = statement
            .query_map(
                params![conversation_id, operation_id, bounded_limit],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    } else {
        let mut statement = connection.prepare(
            "SELECT status, stage, receipt_json
             FROM context_compaction_receipts
             WHERE conversation_id = ?1
               AND operation_id LIKE 'provider-transition-%'
             ORDER BY started_at DESC, operation_id DESC
             LIMIT ?2",
        )?;
        let rows = statement
            .query_map(params![conversation_id, bounded_limit], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    rows.into_iter().map(decode_receipt).collect()
}

pub fn provider_transition_failed_attempt_count(
    connection: &Connection,
    conversation_id: &str,
    target_model_id: &str,
) -> Result<u64, ContextCompactionReceiptRepositoryError> {
    connection
        .query_row(
            "SELECT COUNT(*)
             FROM context_compaction_receipts
             WHERE conversation_id = ?1
               AND model = ?2
               AND operation_id LIKE 'provider-transition-%'
               AND status IN ('failed', 'cancelled', 'interrupted')",
            params![conversation_id, target_model_id],
            |row| row.get::<_, u64>(0),
        )
        .map_err(Into::into)
}

pub fn mark_in_progress_receipts_interrupted(
    connection: &mut Connection,
    completed_at: i64,
) -> Result<(), ContextCompactionReceiptRepositoryError> {
    let receipts = {
        let mut statement = connection.prepare(
            "SELECT status, stage, receipt_json
             FROM context_compaction_receipts
             WHERE status = 'in_progress'
             ORDER BY started_at ASC, operation_id ASC",
        )?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    if receipts.is_empty() {
        return Ok(());
    }
    let transaction = connection.transaction()?;
    for row in receipts {
        let mut receipt = decode_receipt(row)?;
        receipt
            .mark_interrupted(completed_at)
            .map_err(|error| ContextCompactionReceiptRepositoryError::Invalid(error.to_string()))?;
        update_receipt(&transaction, &receipt)?;
    }
    transaction.commit()?;
    Ok(())
}

fn validate_observation_reference(
    receipt: &ContextCompactionReceipt,
    observation: Option<&ModelRequestObservation>,
) -> Result<(), ContextCompactionReceiptRepositoryError> {
    if let Some(observation) = observation {
        receipt
            .validate_generation_observation(observation)
            .map_err(|error| ContextCompactionReceiptRepositoryError::Invalid(error.to_string()))?;
        if matches!(
            receipt.status,
            ContextCompactionReceiptStatus::Applied | ContextCompactionReceiptStatus::Refreshed
        ) && observation.status != crate::ModelRequestObservationStatus::Completed
        {
            return Err(ContextCompactionReceiptRepositoryError::Invalid(
                "成功或 refreshed 的压缩 receipt 必须引用已完成的模型请求观测。".to_string(),
            ));
        }
    }
    match (&receipt.generation_observation_id, observation) {
        (None, None) => Ok(()),
        (Some(expected), Some(observation)) if expected == &observation.id => Ok(()),
        (Some(expected), None) => Err(ContextCompactionReceiptRepositoryError::Invalid(format!(
            "receipt 引用了未随本次写入提供的模型请求观测：{expected}"
        ))),
        (None, Some(_)) | (Some(_), Some(_)) => {
            Err(ContextCompactionReceiptRepositoryError::Invalid(
                "receipt 与模型请求观测引用不一致。".to_string(),
            ))
        }
    }
}

fn validate_transition(
    existing: &ContextCompactionReceipt,
    next: &ContextCompactionReceipt,
) -> Result<(), ContextCompactionReceiptRepositoryError> {
    if existing.status.is_terminal() {
        return Err(ContextCompactionReceiptRepositoryError::Invalid(
            "已进入终态的压缩 receipt 不可再次修改。".to_string(),
        ));
    }
    if next.stage < existing.stage || next.updated_at < existing.updated_at {
        return Err(ContextCompactionReceiptRepositoryError::Invalid(
            "压缩 receipt 不能回退阶段或时间。".to_string(),
        ));
    }
    if existing.schema_version != next.schema_version
        || existing.operation_id != next.operation_id
        || existing.run_id != next.run_id
        || existing.conversation_id != next.conversation_id
        || existing.assistant_message_id != next.assistant_message_id
        || existing.request_index != next.request_index
        || existing.attempt_index != next.attempt_index
        || existing.model != next.model
        || existing.api_style != next.api_style
        || existing.plan != next.plan
        || existing.started_at != next.started_at
        || existing
            .source_revision
            .as_ref()
            .is_some_and(|revision| Some(revision) != next.source_revision.as_ref())
    {
        return Err(ContextCompactionReceiptRepositoryError::Invalid(
            "压缩 receipt 的不可变身份或计划发生变化。".to_string(),
        ));
    }
    Ok(())
}

fn insert_receipt(
    connection: &Connection,
    receipt: &ContextCompactionReceipt,
) -> Result<(), ContextCompactionReceiptRepositoryError> {
    let receipt_json = encode_receipt(receipt)?;
    connection.execute(
        "INSERT INTO context_compaction_receipts (
            operation_id, schema_version, run_id, conversation_id, assistant_message_id,
            request_index, attempt_index, model, api_style, status, stage,
            generation_observation_id, summary_id, receipt_json,
            started_at, updated_at, completed_at
         ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17
         )",
        params![
            &receipt.operation_id,
            receipt.schema_version,
            &receipt.run_id,
            &receipt.conversation_id,
            &receipt.assistant_message_id,
            receipt.request_index,
            receipt.attempt_index,
            &receipt.model,
            api_style_name(receipt.api_style),
            receipt.status.as_str(),
            receipt.stage.as_str(),
            &receipt.generation_observation_id,
            &receipt.summary_id,
            receipt_json,
            receipt.started_at,
            receipt.updated_at,
            receipt.completed_at,
        ],
    )?;
    Ok(())
}

fn update_receipt(
    connection: &Connection,
    receipt: &ContextCompactionReceipt,
) -> Result<(), ContextCompactionReceiptRepositoryError> {
    let receipt_json = encode_receipt(receipt)?;
    let changed = connection.execute(
        "UPDATE context_compaction_receipts
         SET status = ?1,
             stage = ?2,
             generation_observation_id = ?3,
             summary_id = ?4,
             receipt_json = ?5,
             updated_at = ?6,
             completed_at = ?7
         WHERE operation_id = ?8",
        params![
            receipt.status.as_str(),
            receipt.stage.as_str(),
            &receipt.generation_observation_id,
            &receipt.summary_id,
            receipt_json,
            receipt.updated_at,
            receipt.completed_at,
            &receipt.operation_id,
        ],
    )?;
    if changed != 1 {
        return Err(ContextCompactionReceiptRepositoryError::Invalid(
            "找不到需要更新的压缩 receipt。".to_string(),
        ));
    }
    Ok(())
}

fn encode_receipt(
    receipt: &ContextCompactionReceipt,
) -> Result<String, ContextCompactionReceiptRepositoryError> {
    serde_json::to_string(receipt).map_err(|error| {
        ContextCompactionReceiptRepositoryError::Invalid(format!("无法序列化压缩 receipt：{error}"))
    })
}

fn decode_receipt(
    row: (String, String, String),
) -> Result<ContextCompactionReceipt, ContextCompactionReceiptRepositoryError> {
    let (status, stage, receipt_json) = row;
    let receipt: ContextCompactionReceipt =
        serde_json::from_str(&receipt_json).map_err(|error| {
            ContextCompactionReceiptRepositoryError::Invalid(format!(
                "无法解析压缩 receipt：{error}"
            ))
        })?;
    receipt
        .validate()
        .map_err(|error| ContextCompactionReceiptRepositoryError::Invalid(error.to_string()))?;
    if Some(receipt.status) != ContextCompactionReceiptStatus::from_str(&status)
        || Some(receipt.stage) != ContextCompactionReceiptStage::from_str(&stage)
    {
        return Err(ContextCompactionReceiptRepositoryError::Invalid(
            "压缩 receipt 索引列与 JSON 内容不一致。".to_string(),
        ));
    }
    Ok(receipt)
}

fn api_style_name(api_style: crate::AgentApiStyle) -> &'static str {
    match api_style {
        crate::AgentApiStyle::OpenAiCompatible => "open_ai_compatible",
        crate::AgentApiStyle::AnthropicCompatible => "anthropic_compatible",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::migrations;
    use crate::{
        ContextCompactionReceiptPlan, ContextJournalCursor,
        CONTEXT_COMPACTION_RECEIPT_SCHEMA_VERSION,
    };

    fn setup() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        migrations::run_migrations(&connection).unwrap();
        connection
            .execute(
                "INSERT INTO conversations (
                    id, project_id, model_id, title, created_at, updated_at,
                    pinned_at, archived_at, unread_at
                 ) VALUES ('conversation-1', NULL, NULL, 'Test', 1, 1, NULL, NULL, NULL)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO messages (
                    id, conversation_id, role, content, status, agent_run_json,
                    ui_state_json, created_at, position
                 ) VALUES (
                    'assistant-1', 'conversation-1', 'assistant', '', 'pending',
                    NULL, NULL, 1, 0
                 )",
                [],
            )
            .unwrap();
        connection
    }

    fn planned_receipt() -> ContextCompactionReceipt {
        ContextCompactionReceipt {
            schema_version: CONTEXT_COMPACTION_RECEIPT_SCHEMA_VERSION,
            operation_id: "operation-1".to_string(),
            run_id: "run-1".to_string(),
            conversation_id: "conversation-1".to_string(),
            assistant_message_id: "assistant-1".to_string(),
            request_index: 1,
            attempt_index: 1,
            model: "test-model".to_string(),
            api_style: crate::AgentApiStyle::OpenAiCompatible,
            status: ContextCompactionReceiptStatus::InProgress,
            stage: ContextCompactionReceiptStage::Planned,
            plan: ContextCompactionReceiptPlan {
                context_revision: "1".to_string(),
                persistent_revision: "1".to_string(),
                request_input_tokens: 120,
                available_input_tokens: Some(120),
                request_trigger_input_tokens: Some(100),
                request_target_input_tokens: Some(30),
                source_input_tokens: 100,
                retained_input_tokens: 0,
                target_replacement_tokens: 30,
                expected_reclaimed_tokens: 70,
                planned_reclaimed_tokens: 70,
                projected_request_input_tokens: 50,
                best_effort: false,
                protected_input_tokens: 0,
                protected_reasons: Default::default(),
                atomic_unit_count: 2,
                previous_summary_id: None,
                covered_through: ContextJournalCursor::message("assistant-old"),
            },
            source_revision: None,
            generation_observation_id: None,
            summary_id: None,
            result: None,
            error: None,
            started_at: 1,
            updated_at: 1,
            completed_at: None,
        }
    }

    fn completed_compaction_observation() -> ModelRequestObservation {
        crate::model_request_observation::ModelRequestObservationBuilder::new(
            "model-request-operation-1",
            "run-1",
            Some("conversation-1".to_string()),
            Some("assistant-1".to_string()),
            Some("operation-1".to_string()),
            1,
            crate::ModelRequestPurpose::ContextCompaction,
            "test-model",
            crate::AgentApiStyle::OpenAiCompatible,
            None,
            1,
        )
        .completed(None, Some("stop".to_string()), 2)
        .unwrap()
    }

    #[test]
    fn startup_marks_unfinished_receipts_interrupted_without_moving_other_state() {
        let mut connection = setup();
        record_receipt(&mut connection, &planned_receipt(), None).unwrap();

        mark_in_progress_receipts_interrupted(&mut connection, 10).unwrap();

        let receipt = get_receipt(&connection, "operation-1").unwrap().unwrap();
        assert_eq!(receipt.status, ContextCompactionReceiptStatus::Interrupted);
        assert_eq!(receipt.completed_at, Some(10));
        assert_eq!(
            receipt
                .error
                .as_ref()
                .and_then(|error| error.code.as_deref()),
            Some("context_compaction_interrupted")
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM context_compaction_summaries",
                    [],
                    |row| row.get::<_, u64>(0),
                )
                .unwrap(),
            0
        );
    }

    #[test]
    fn provider_transition_failed_attempt_count_is_unambiguous_with_equal_timestamps() {
        let mut connection = setup();
        for (operation_id, run_id) in [
            ("provider-transition-ffff", "transition-run-1"),
            ("provider-transition-0000", "transition-run-2"),
        ] {
            let mut receipt = planned_receipt();
            receipt.operation_id = operation_id.to_string();
            receipt.run_id = run_id.to_string();
            receipt.model = "target-model".to_string();
            receipt.started_at = 5;
            receipt.updated_at = 5;
            record_receipt(&mut connection, &receipt, None).unwrap();
            receipt
                .complete_error(&crate::AgentError::new("failed"), None, 5)
                .unwrap();
            record_receipt(&mut connection, &receipt, None).unwrap();
        }

        assert_eq!(
            provider_transition_failed_attempt_count(&connection, "conversation-1", "target-model")
                .unwrap(),
            2
        );
    }

    #[test]
    fn rejects_success_receipt_linked_to_failed_generation_observation() {
        let completed = completed_compaction_observation();
        let mut receipt = planned_receipt();
        receipt.complete_refreshed(Some(&completed), 2).unwrap();

        let mut failed = completed;
        failed.status = crate::ModelRequestObservationStatus::Failed;
        failed.finish_reason = None;
        failed.error_code = Some("provider_error".to_string());
        failed.error_message = Some("provider failed".to_string());
        failed.validate().unwrap();

        assert!(matches!(
            validate_observation_reference(&receipt, Some(&failed)),
            Err(ContextCompactionReceiptRepositoryError::Invalid(_))
        ));
    }
}
