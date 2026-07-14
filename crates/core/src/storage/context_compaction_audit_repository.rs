//! Read-only assembly of compaction acceptance reports.

use crate::storage::{
    context_compaction_receipt_repository, model_request_observation_repository, now_ms,
};
use crate::{
    ContextCompactionAuditBundle, ContextCompactionSummaryEvidence,
    ContextCompactionSummaryRelation,
};
use rusqlite::{Connection, OptionalExtension};
use std::collections::HashSet;
use std::error::Error;
use std::fmt::{Display, Formatter};

#[derive(Debug)]
pub enum ContextCompactionAuditRepositoryError {
    Database(rusqlite::Error),
    Invalid(String),
}

impl Display for ContextCompactionAuditRepositoryError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Database(error) => write!(formatter, "本地数据库操作失败：{error}"),
            Self::Invalid(message) => write!(formatter, "上下文压缩验收数据无效：{message}"),
        }
    }
}

impl Error for ContextCompactionAuditRepositoryError {}

impl From<rusqlite::Error> for ContextCompactionAuditRepositoryError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Database(error)
    }
}

pub fn get_audit_bundle(
    connection: &Connection,
    conversation_id: &str,
    operation_id: Option<&str>,
    limit: usize,
) -> Result<ContextCompactionAuditBundle, ContextCompactionAuditRepositoryError> {
    let conversation_exists = connection
        .query_row(
            "SELECT 1 FROM conversations WHERE id = ?1",
            [conversation_id],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if !conversation_exists {
        return Err(ContextCompactionAuditRepositoryError::Invalid(
            "找不到指定会话。".to_string(),
        ));
    }

    let receipts = context_compaction_receipt_repository::list_receipts(
        connection,
        conversation_id,
        operation_id,
        limit,
    )
    .map_err(|error| ContextCompactionAuditRepositoryError::Invalid(error.to_string()))?;
    let mut reports = Vec::with_capacity(receipts.len());
    for receipt in receipts {
        let observation = receipt
            .generation_observation_id
            .as_deref()
            .map(|observation_id| {
                model_request_observation_repository::get_observation(connection, observation_id)
            })
            .transpose()
            .map_err(|error| ContextCompactionAuditRepositoryError::Invalid(error.to_string()))?
            .flatten();
        let summary = receipt
            .summary_id
            .as_deref()
            .map(|summary_id| load_summary_evidence(connection, conversation_id, summary_id))
            .transpose()?;
        reports.push(
            crate::context_compaction_audit::build_compaction_audit_report(
                receipt,
                observation,
                summary,
            ),
        );
    }

    let observations = model_request_observation_repository::list_observations_for_conversation(
        connection,
        conversation_id,
    )
    .map_err(|error| ContextCompactionAuditRepositoryError::Invalid(error.to_string()))?;
    Ok(ContextCompactionAuditBundle {
        conversation_id: conversation_id.to_string(),
        generated_at: now_ms(),
        reports,
        estimation_error_groups: crate::context_compaction_audit::group_estimation_errors(
            &observations,
        ),
    })
}

fn load_summary_evidence(
    connection: &Connection,
    conversation_id: &str,
    summary_id: &str,
) -> Result<ContextCompactionSummaryEvidence, ContextCompactionAuditRepositoryError> {
    let row = connection
        .query_row(
            "SELECT source_revision, source_input_tokens, summary_input_tokens,
                    continuity_input_tokens, replacement_input_tokens
             FROM context_compaction_summaries
             WHERE id = ?1 AND conversation_id = ?2",
            [summary_id, conversation_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, u64>(1)?,
                    row.get::<_, u64>(2)?,
                    row.get::<_, u64>(3)?,
                    row.get::<_, u64>(4)?,
                ))
            },
        )
        .optional()?;
    let Some((source_revision, source, summary, continuity, replacement)) = row else {
        return Ok(ContextCompactionSummaryEvidence {
            summary_id: summary_id.to_string(),
            relation: ContextCompactionSummaryRelation::Missing,
            source_revision: None,
            source_input_tokens: None,
            summary_input_tokens: None,
            continuity_input_tokens: None,
            replacement_input_tokens: None,
        });
    };
    Ok(ContextCompactionSummaryEvidence {
        summary_id: summary_id.to_string(),
        relation: summary_relation(connection, conversation_id, summary_id)?,
        source_revision: Some(source_revision),
        source_input_tokens: Some(source),
        summary_input_tokens: Some(summary),
        continuity_input_tokens: Some(continuity),
        replacement_input_tokens: Some(replacement),
    })
}

fn summary_relation(
    connection: &Connection,
    conversation_id: &str,
    summary_id: &str,
) -> Result<ContextCompactionSummaryRelation, ContextCompactionAuditRepositoryError> {
    let active_id = connection
        .query_row(
            "SELECT summary_id
             FROM conversation_context_compaction_heads
             WHERE conversation_id = ?1",
            [conversation_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    let Some(mut current_id) = active_id else {
        return Ok(ContextCompactionSummaryRelation::Detached);
    };
    if current_id == summary_id {
        return Ok(ContextCompactionSummaryRelation::Active);
    }

    let mut visited = HashSet::new();
    while visited.insert(current_id.clone()) {
        let previous = connection
            .query_row(
                "SELECT previous_summary_id
                 FROM context_compaction_summaries
                 WHERE id = ?1 AND conversation_id = ?2",
                [&current_id, conversation_id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten();
        let Some(previous) = previous else {
            break;
        };
        if previous == summary_id {
            return Ok(ContextCompactionSummaryRelation::Superseded);
        }
        current_id = previous;
    }
    Ok(ContextCompactionSummaryRelation::Detached)
}
