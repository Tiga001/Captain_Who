use crate::content_revision;
use crate::context::{
    ContextCompactionGeneration, ContextCompactionGenerationKind, ContextCompactionPrefix,
    ContextCompactionSourceItem, ContextCompactionSummary, ContextCompactionSummaryDraft,
    ContextJournalCursor,
};
use crate::storage::{
    context_compaction_receipt_repository, conversation_trace_repository,
    provider_continuation_repository, world_state_repository,
};
use crate::{ContextCompactionReceipt, ContextCompactionReceiptStatus, ModelRequestObservation};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::fmt::{Display, Formatter};

#[derive(Debug)]
pub enum ContextCompactionRepositoryError {
    Database(rusqlite::Error),
    Invalid(String),
    Stale(String),
}

impl Display for ContextCompactionRepositoryError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Database(error) => write!(formatter, "本地数据库操作失败：{error}"),
            Self::Invalid(message) => write!(formatter, "上下文压缩数据无效：{message}"),
            Self::Stale(message) => write!(formatter, "stale_context_compaction_prefix: {message}"),
        }
    }
}

impl Error for ContextCompactionRepositoryError {}

impl ContextCompactionRepositoryError {
    pub fn is_stale(&self) -> bool {
        matches!(self, Self::Stale(_))
    }
}

impl From<rusqlite::Error> for ContextCompactionRepositoryError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Database(error)
    }
}

#[derive(Debug)]
pub(crate) struct MessageDeletionCompactionRewind {
    conversation_id: String,
    generated_summary_ids: Vec<String>,
    restore_summary_id: Option<String>,
    restore_active_head: bool,
    previous_head_revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ContextCompactionSummaryLineage {
    pub introduced_by_assistant_message_id: String,
    pub source_conversation_id: Option<String>,
    pub source_summary_id: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct ContextCompactionSummaryVersion {
    pub summary: ContextCompactionSummary,
    pub lineage: ContextCompactionSummaryLineage,
}

/// Captures the compaction state that must be restored when one or more assistant messages are
/// removed. A summary is a branch-local derived state even when its covered raw prefix predates
/// the deleted turn, so source-revision validation alone is not enough for edit-and-resend.
pub(crate) fn prepare_message_deletion_compaction_rewind(
    connection: &Connection,
    conversation_id: &str,
    message_ids: &[String],
) -> Result<MessageDeletionCompactionRewind, ContextCompactionRepositoryError> {
    let active_head = connection
        .query_row(
            "SELECT summary_id, revision
             FROM conversation_context_compaction_heads
             WHERE conversation_id = ?1",
            [conversation_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, u64>(1)?)),
        )
        .optional()?;

    let mut previous_by_summary = HashMap::new();
    let mut visited_messages = HashSet::new();
    for message_id in message_ids {
        if !visited_messages.insert(message_id.as_str()) {
            continue;
        }
        let rows = {
            let mut statement = connection.prepare(
                "SELECT summary.id, summary.previous_summary_id
                 FROM context_compaction_summaries AS summary
                 INNER JOIN context_compaction_summary_lineage AS lineage
                    ON lineage.summary_id = summary.id
                 WHERE summary.conversation_id = ?1
                   AND lineage.conversation_id = ?1
                   AND lineage.introduced_by_assistant_message_id = ?2",
            )?;
            let rows = statement
                .query_map(params![conversation_id, message_id], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows
        };
        for (summary_id, previous_summary_id) in rows {
            if previous_by_summary
                .insert(summary_id.clone(), previous_summary_id)
                .is_some()
            {
                return Err(ContextCompactionRepositoryError::Invalid(format!(
                    "同一个摘要存在重复的因果归属：{summary_id}"
                )));
            }
        }
    }

    let generated_summary_ids = previous_by_summary.keys().cloned().collect::<Vec<_>>();
    let (restore_summary_id, restore_active_head, previous_head_revision) =
        match active_head.as_ref() {
            Some((active_summary_id, revision))
                if previous_by_summary.contains_key(active_summary_id) =>
            {
                let mut current = active_summary_id.clone();
                let mut visited = HashSet::new();
                let restore_summary_id = loop {
                    if !visited.insert(current.clone()) {
                        return Err(ContextCompactionRepositoryError::Invalid(
                            "待删除消息生成的压缩摘要链存在循环。".to_string(),
                        ));
                    }
                    let previous = previous_by_summary.get(&current).ok_or_else(|| {
                        ContextCompactionRepositoryError::Invalid(
                            "active summary 缺少对应的因果归属。".to_string(),
                        )
                    })?;
                    match previous {
                        Some(previous) if previous_by_summary.contains_key(previous) => {
                            current = previous.clone();
                        }
                        previous => break previous.clone(),
                    }
                };
                (restore_summary_id, true, *revision)
            }
            Some((_, revision)) => (None, false, *revision),
            None => (None, false, 0),
        };

    if restore_active_head {
        let active_summary_id = active_head
            .as_ref()
            .map(|(summary_id, _)| summary_id.as_str())
            .ok_or_else(|| {
                ContextCompactionRepositoryError::Stale(
                    "待回退的 active summary 已不存在。".to_string(),
                )
            })?;
        let active_summary = load_summary(connection, active_summary_id)?;
        let restored_summary = restore_summary_id
            .as_deref()
            .map(|summary_id| load_summary(connection, summary_id))
            .transpose()?;
        ensure_provider_replay_survives_summary_rollback(
            connection,
            &active_summary,
            restored_summary.as_ref(),
        )?;
    }

    Ok(MessageDeletionCompactionRewind {
        conversation_id: conversation_id.to_string(),
        generated_summary_ids,
        restore_summary_id,
        restore_active_head,
        previous_head_revision,
    })
}

/// Completes a branch rewind after the selected messages have been deleted in the same
/// transaction. Generated summaries are removed and the active cursor is restored only when the
/// pre-run summary still describes the remaining authoritative raw prefix.
pub(crate) fn finish_message_deletion_compaction_rewind(
    connection: &Connection,
    rewind: MessageDeletionCompactionRewind,
    updated_at: i64,
) -> Result<(), ContextCompactionRepositoryError> {
    for summary_id in &rewind.generated_summary_ids {
        connection.execute(
            "DELETE FROM context_compaction_summaries
             WHERE id = ?1 AND conversation_id = ?2",
            params![summary_id, &rewind.conversation_id],
        )?;
    }

    if rewind.restore_active_head {
        connection.execute(
            "DELETE FROM conversation_context_compaction_heads WHERE conversation_id = ?1",
            [&rewind.conversation_id],
        )?;
        if let Some(summary_id) = rewind.restore_summary_id {
            let summary = connection
                .query_row(
                    "SELECT id FROM context_compaction_summaries
                     WHERE id = ?1 AND conversation_id = ?2",
                    params![&summary_id, &rewind.conversation_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()?
                .map(|_| load_summary(connection, &summary_id))
                .transpose()?;
            if let Some(summary) = summary {
                if summary_matches_current_raw_prefix(connection, &summary)? {
                    connection.execute(
                        "INSERT INTO conversation_context_compaction_heads (
                            conversation_id, summary_id, revision, updated_at
                         ) VALUES (?1, ?2, ?3, ?4)",
                        params![
                            &rewind.conversation_id,
                            &summary.id,
                            rewind.previous_head_revision.saturating_add(1),
                            updated_at,
                        ],
                    )?;
                }
            }
        }
    } else {
        // Message deletion may invalidate a summary produced by another turn. Reuse the normal
        // content-addressed guard for that case instead of restoring an unrelated branch head.
        let _ = get_active_summary(connection, &rewind.conversation_id)?;
    }
    Ok(())
}

pub fn get_active_summary(
    connection: &Connection,
    conversation_id: &str,
) -> Result<Option<ContextCompactionSummary>, ContextCompactionRepositoryError> {
    let summary_id = connection
        .query_row(
            "SELECT summary_id
             FROM conversation_context_compaction_heads
             WHERE conversation_id = ?1",
            [conversation_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    let Some(summary_id) = summary_id else {
        return Ok(None);
    };
    let summary = load_summary(connection, &summary_id)?;
    if summary.conversation_id != conversation_id {
        return Err(ContextCompactionRepositoryError::Invalid(
            "active head 指向了其他会话的摘要。".to_string(),
        ));
    }
    if !summary_matches_current_raw_prefix(connection, &summary)?
        || !continuity_refs_exist(connection, &summary)?
    {
        if provider_continuation_repository::has_released_for_conversation(
            connection,
            conversation_id,
        )? {
            // Invalidating the summary would make its raw prefix model-visible again. A released
            // tombstone proves at least one provider-native Tool turn in this conversation can no
            // longer be reconstructed, so falling back to the split durable log is unsafe.
            return Err(ContextCompactionRepositoryError::Stale(
                "provider_context_boundary_required: active summary 已失效，但其 Provider replay 已安全释放，拒绝暴露残缺 Tool Exchange。"
                    .to_string(),
            ));
        }
        // Raw history remains authoritative. An edit, delete or rollback invalidates only the
        // derived summaries; later requests immediately fall back to the remaining raw log.
        connection.execute(
            "DELETE FROM context_compaction_summaries WHERE conversation_id = ?1",
            [conversation_id],
        )?;
        return Ok(None);
    }
    Ok(Some(summary))
}

pub fn prepare_prefix(
    connection: &Connection,
    conversation_id: &str,
    covered_through: &ContextJournalCursor,
) -> Result<ContextCompactionPrefix, ContextCompactionRepositoryError> {
    covered_through
        .validate()
        .map_err(|error| ContextCompactionRepositoryError::Invalid(error.to_string()))?;
    let entries = list_journal_entries(connection, conversation_id)?;
    let boundary_index = cursor_index(&entries, covered_through).ok_or_else(|| {
        ContextCompactionRepositoryError::Invalid(format!(
            "覆盖边界不属于当前会话日志：{covered_through:?}"
        ))
    })?;
    if !entries[boundary_index].is_safe_boundary() {
        return Err(ContextCompactionRepositoryError::Invalid(
            "上下文压缩不能停在未闭合的工具调用上。".to_string(),
        ));
    }

    let previous_summary = get_active_summary(connection, conversation_id)?;
    let source_start = match &previous_summary {
        Some(previous) => {
            let previous_index =
                cursor_index(&entries, &previous.covered_through).ok_or_else(|| {
                    ContextCompactionRepositoryError::Stale(
                        "active summary 的日志游标已不存在。".to_string(),
                    )
                })?;
            if previous_index >= boundary_index {
                return Err(ContextCompactionRepositoryError::Invalid(
                    "新的上下文压缩游标必须向日志尾部推进。".to_string(),
                ));
            }
            previous_index + 1
        }
        None => 0,
    };
    let source_items = entries[source_start..=boundary_index].to_vec();
    let source_revision = source_revision(
        conversation_id,
        covered_through,
        &entries[..=boundary_index],
    )?;
    let prefix = ContextCompactionPrefix {
        conversation_id: conversation_id.to_string(),
        source_revision,
        covered_through: covered_through.clone(),
        previous_summary,
        source_items,
    };
    prefix
        .validate()
        .map_err(|error| ContextCompactionRepositoryError::Invalid(error.to_string()))?;
    Ok(prefix)
}

/// Commits an immutable summary and advances the active cursor in one transaction.
///
/// Summary generation runs outside the write transaction. The exact raw prefix is reconstructed
/// again under the transaction before the head can move.
pub fn commit_prefix_replacement(
    connection: &mut Connection,
    expected_prefix: &ContextCompactionPrefix,
    draft: ContextCompactionSummaryDraft,
    introduced_by_assistant_message_id: &str,
) -> Result<ContextCompactionSummary, ContextCompactionRepositoryError> {
    validate_commit_inputs(expected_prefix, &draft)?;
    let transaction = connection.transaction()?;
    let summary = commit_prefix_replacement_in_transaction(
        &transaction,
        expected_prefix,
        draft,
        introduced_by_assistant_message_id,
    )?;
    transaction.commit()?;
    Ok(summary)
}

/// Atomically applies a generated summary together with its provider observation and audit
/// receipt. A caller may safely treat an `Ok` result as proof that all four durable facts became
/// visible together: request observation, immutable summary, active head and applied receipt.
pub fn commit_prefix_replacement_with_receipt(
    connection: &mut Connection,
    expected_prefix: &ContextCompactionPrefix,
    draft: ContextCompactionSummaryDraft,
    receipt: &ContextCompactionReceipt,
    observation: &ModelRequestObservation,
) -> Result<ContextCompactionSummary, ContextCompactionRepositoryError> {
    validate_commit_inputs(expected_prefix, &draft)?;
    receipt
        .validate()
        .map_err(|error| ContextCompactionRepositoryError::Invalid(error.to_string()))?;
    if receipt.status != ContextCompactionReceiptStatus::Applied
        || receipt.conversation_id != expected_prefix.conversation_id
        || receipt.source_revision.as_deref() != Some(expected_prefix.source_revision.as_str())
        || receipt.summary_id.as_deref() != Some(draft.id.as_str())
    {
        return Err(ContextCompactionRepositoryError::Invalid(
            "applied receipt 与待提交的压缩前缀或摘要草稿不一致。".to_string(),
        ));
    }

    let transaction = connection.transaction()?;
    let summary = commit_prefix_replacement_in_transaction(
        &transaction,
        expected_prefix,
        draft,
        &receipt.assistant_message_id,
    )?;
    context_compaction_receipt_repository::record_receipt_in_connection(
        &transaction,
        receipt,
        Some(observation),
    )
    .map_err(map_receipt_error)?;
    transaction.commit()?;
    Ok(summary)
}

/// Atomically installs a Provider-neutral summary and switches the conversation to the exact
/// target model whose wire revision was preflighted.
///
/// The source journal, active summary head, current conversation model, conversation update
/// revision, and target model protocol revision are all checked again inside the same SQLite
/// transaction. The composer draft is updated in that transaction as well, preventing a crash
/// between backend commit and Renderer persistence from resurrecting the old model selection.
#[allow(clippy::too_many_arguments)]
pub fn commit_provider_transition_with_receipt(
    connection: &mut Connection,
    expected_prefix: &ContextCompactionPrefix,
    draft: ContextCompactionSummaryDraft,
    receipt: &ContextCompactionReceipt,
    observation: &ModelRequestObservation,
    expected_current_model_id: Option<&str>,
    expected_conversation_updated_at: i64,
    target_model_id: &str,
    expected_target_provider_protocol_revision: &str,
) -> Result<(ContextCompactionSummary, i64), ContextCompactionRepositoryError> {
    validate_commit_inputs(expected_prefix, &draft)?;
    receipt
        .validate()
        .map_err(|error| ContextCompactionRepositoryError::Invalid(error.to_string()))?;
    if receipt.status != ContextCompactionReceiptStatus::Applied
        || receipt.conversation_id != expected_prefix.conversation_id
        || receipt.source_revision.as_deref() != Some(expected_prefix.source_revision.as_str())
        || receipt.summary_id.as_deref() != Some(draft.id.as_str())
        || receipt.model != target_model_id
    {
        return Err(ContextCompactionRepositoryError::Invalid(
            "Provider transition receipt 与待提交的前缀、摘要或目标模型不一致。".to_string(),
        ));
    }

    let transaction = connection.transaction()?;
    validate_provider_transition_compare_and_set(
        &transaction,
        &expected_prefix.conversation_id,
        expected_current_model_id,
        expected_conversation_updated_at,
        target_model_id,
        expected_target_provider_protocol_revision,
    )?;
    // Recover any staged row whose durable ToolCall handoff committed before a crash so the
    // release below evaluates the complete Provider-state set inside this transaction.
    provider_continuation_repository::has_replayable_for_conversation(
        &transaction,
        &expected_prefix.conversation_id,
    )?;
    let summary = commit_prefix_replacement_in_transaction(
        &transaction,
        expected_prefix,
        draft,
        &receipt.assistant_message_id,
    )?;
    if provider_continuation_repository::has_replayable_for_conversation(
        &transaction,
        &expected_prefix.conversation_id,
    )? {
        return Err(ContextCompactionRepositoryError::Stale(
            "provider_context_boundary_required: 历史压缩未完整覆盖已有 Provider replay 状态。"
                .to_string(),
        ));
    }
    let transition_updated_at = summary
        .created_at
        .max(expected_conversation_updated_at.saturating_add(1));
    let transition_updated_at = provider_transition_updated_at(
        &transaction,
        &expected_prefix.conversation_id,
        transition_updated_at,
    )?;
    let mut committed_receipt = receipt.clone();
    committed_receipt.updated_at = committed_receipt.updated_at.max(transition_updated_at);
    context_compaction_receipt_repository::record_receipt_in_connection(
        &transaction,
        &committed_receipt,
        Some(observation),
    )
    .map_err(map_receipt_error)?;
    apply_provider_transition_model_selection(
        &transaction,
        &expected_prefix.conversation_id,
        target_model_id,
        transition_updated_at,
    )?;
    transaction.commit()?;
    Ok((summary, transition_updated_at))
}

fn provider_transition_updated_at(
    transaction: &Transaction<'_>,
    conversation_id: &str,
    minimum_updated_at: i64,
) -> Result<i64, ContextCompactionRepositoryError> {
    let draft_updated_at = transaction
        .query_row(
            "SELECT updated_at FROM composer_drafts WHERE scope_id = ?1",
            [conversation_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()?;
    Ok(draft_updated_at
        .map(|updated_at| minimum_updated_at.max(updated_at.saturating_add(1)))
        .unwrap_or(minimum_updated_at))
}

fn validate_provider_transition_compare_and_set(
    transaction: &Transaction<'_>,
    conversation_id: &str,
    expected_current_model_id: Option<&str>,
    expected_conversation_updated_at: i64,
    target_model_id: &str,
    expected_target_provider_protocol_revision: &str,
) -> Result<(), ContextCompactionRepositoryError> {
    let current = transaction
        .query_row(
            "SELECT model_id, updated_at FROM conversations WHERE id = ?1",
            [conversation_id],
            |row| Ok((row.get::<_, Option<String>>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()?;
    let Some((current_model_id, current_updated_at)) = current else {
        return Err(ContextCompactionRepositoryError::Stale(
            "Provider transition 的目标会话已不存在。".to_string(),
        ));
    };
    if current_model_id.as_deref() != expected_current_model_id
        || current_updated_at != expected_conversation_updated_at
    {
        return Err(ContextCompactionRepositoryError::Stale(
            "Provider transition 期间会话 head 或当前模型已变化。".to_string(),
        ));
    }
    let target_revision = transaction
        .query_row(
            "SELECT provider_protocol_revision FROM models WHERE id = ?1 AND enabled = 1",
            [target_model_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    if target_revision.as_deref() != Some(expected_target_provider_protocol_revision) {
        return Err(ContextCompactionRepositoryError::Stale(
            "Provider transition 期间目标模型的协议配置已变化。".to_string(),
        ));
    }
    Ok(())
}

fn apply_provider_transition_model_selection(
    transaction: &Transaction<'_>,
    conversation_id: &str,
    target_model_id: &str,
    updated_at: i64,
) -> Result<(), ContextCompactionRepositoryError> {
    let affected = transaction.execute(
        "UPDATE conversations SET model_id = ?1, updated_at = ?2 WHERE id = ?3",
        params![target_model_id, updated_at, conversation_id],
    )?;
    if affected != 1 {
        return Err(ContextCompactionRepositoryError::Stale(
            "Provider transition 提交前目标会话已不存在。".to_string(),
        ));
    }
    transaction.execute(
        "UPDATE composer_drafts
         SET model_id = ?1,
             updated_at = ?2
         WHERE scope_id = ?3",
        params![target_model_id, updated_at, conversation_id],
    )?;
    Ok(())
}

fn validate_commit_inputs(
    expected_prefix: &ContextCompactionPrefix,
    draft: &ContextCompactionSummaryDraft,
) -> Result<(), ContextCompactionRepositoryError> {
    expected_prefix
        .validate()
        .map_err(|error| ContextCompactionRepositoryError::Invalid(error.to_string()))?;
    draft
        .validate()
        .map_err(|error| ContextCompactionRepositoryError::Invalid(error.to_string()))?;
    if draft.source_revision != expected_prefix.source_revision {
        return Err(ContextCompactionRepositoryError::Stale(
            "摘要草稿不是基于待替换日志前缀生成的。".to_string(),
        ));
    }
    Ok(())
}

fn commit_prefix_replacement_in_transaction(
    transaction: &Transaction<'_>,
    expected_prefix: &ContextCompactionPrefix,
    draft: ContextCompactionSummaryDraft,
    introduced_by_assistant_message_id: &str,
) -> Result<ContextCompactionSummary, ContextCompactionRepositoryError> {
    let active_summary_id = transaction
        .query_row(
            "SELECT summary_id
             FROM conversation_context_compaction_heads
             WHERE conversation_id = ?1",
            [&expected_prefix.conversation_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    let expected_summary_id = expected_prefix
        .previous_summary
        .as_ref()
        .map(|summary| summary.id.as_str());
    if active_summary_id.as_deref() != expected_summary_id {
        return Err(ContextCompactionRepositoryError::Stale(
            "摘要生成期间 active head 已发生变化。".to_string(),
        ));
    }
    let current_prefix = prepare_prefix(
        transaction,
        &expected_prefix.conversation_id,
        &expected_prefix.covered_through,
    )?;
    if current_prefix.source_revision != expected_prefix.source_revision
        || current_prefix.covered_through != expected_prefix.covered_through
        || current_prefix
            .previous_summary
            .as_ref()
            .map(|summary| summary.id.as_str())
            != expected_summary_id
    {
        return Err(ContextCompactionRepositoryError::Stale(
            "摘要生成期间原始上下文日志或 active head 已发生变化。".to_string(),
        ));
    }

    let summary = draft
        .finish(&current_prefix)
        .map_err(|error| ContextCompactionRepositoryError::Invalid(error.to_string()))?;
    insert_summary(transaction, &summary)?;
    insert_summary_lineage(
        transaction,
        &summary,
        &ContextCompactionSummaryLineage {
            introduced_by_assistant_message_id: introduced_by_assistant_message_id.to_string(),
            source_conversation_id: None,
            source_summary_id: None,
        },
    )?;
    rebase_active_world_state_for_summary(transaction, &summary)?;
    let current_head_revision = transaction
        .query_row(
            "SELECT revision
             FROM conversation_context_compaction_heads
             WHERE conversation_id = ?1",
            [&summary.conversation_id],
            |row| row.get::<_, u64>(0),
        )
        .optional()?
        .unwrap_or(0);
    transaction.execute(
        "INSERT INTO conversation_context_compaction_heads (
            conversation_id, summary_id, revision, updated_at
         ) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(conversation_id) DO UPDATE SET
            summary_id = excluded.summary_id,
            revision = excluded.revision,
            updated_at = excluded.updated_at",
        params![
            &summary.conversation_id,
            &summary.id,
            current_head_revision.saturating_add(1),
            summary.created_at,
        ],
    )?;
    // Release only an exact Provider turn whose complete runtime Tool-call set appears as closed
    // ToolResult items in this newly replaced prefix. Message identity is too coarse: one
    // assistant message/run may contain several Provider requestIndex turns, and a trace cursor
    // can cover the first exchange while a later exchange remains live. Any later transaction
    // failure restores both summary state and ciphertext.
    let covered_runtime_tool_calls = expected_prefix
        .source_items
        .iter()
        .filter_map(|item| match item {
            ContextCompactionSourceItem::TraceItem { run_id, item, .. } => match item.as_ref() {
                crate::ConversationTurnTraceItem::ToolResult { call_id, .. } => {
                    Some((run_id.clone(), call_id.clone()))
                }
                _ => None,
            },
            ContextCompactionSourceItem::Message { .. } => None,
        })
        .collect::<Vec<_>>();
    provider_continuation_repository::release_for_covered_runtime_tool_calls(
        transaction,
        &summary.conversation_id,
        &covered_runtime_tool_calls,
        summary.created_at,
    )?;
    Ok(summary)
}

fn rebase_active_world_state_for_summary(
    transaction: &Connection,
    summary: &ContextCompactionSummary,
) -> Result<(), ContextCompactionRepositoryError> {
    let Some(active_snapshot) =
        world_state_repository::fold_active_snapshot(transaction, &summary.conversation_id)
            .map_err(map_world_state_error)?
    else {
        // Conversations created before the World State journal remain valid and acquire their
        // first full snapshot lazily on the next normal turn.
        return Ok(());
    };
    let new_epoch_id = world_state_epoch_id_for_summary(&summary.id);
    world_state_repository::rebase_active_epoch_in_connection(
        transaction,
        &world_state_repository::ConversationWorldStateRebaseRequest {
            conversation_id: &summary.conversation_id,
            expected_source_epoch_id: &active_snapshot.epoch_id,
            expected_source_revision: &active_snapshot.revision,
            covered_through_message_id: summary.covered_through.message_id(),
            new_epoch_id: &new_epoch_id,
            base_summary_id: &summary.id,
            created_at: summary.created_at,
        },
    )
    .map_err(map_world_state_error)?;
    Ok(())
}

fn world_state_epoch_id_for_summary(summary_id: &str) -> String {
    let digest = Sha256::digest(summary_id.as_bytes());
    let mut encoded = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        write!(&mut encoded, "{byte:02x}").expect("writing to a String cannot fail");
    }
    format!("world-state-summary-sha256-v1:{encoded}")
}

fn map_world_state_error(
    error: world_state_repository::ConversationWorldStateRepositoryError,
) -> ContextCompactionRepositoryError {
    match error {
        world_state_repository::ConversationWorldStateRepositoryError::Database(error) => {
            ContextCompactionRepositoryError::Database(error)
        }
        world_state_repository::ConversationWorldStateRepositoryError::Conflict(message) => {
            ContextCompactionRepositoryError::Stale(message)
        }
        world_state_repository::ConversationWorldStateRepositoryError::Invalid(message)
        | world_state_repository::ConversationWorldStateRepositoryError::Corrupt(message) => {
            ContextCompactionRepositoryError::Invalid(message)
        }
    }
}

fn map_receipt_error(
    error: context_compaction_receipt_repository::ContextCompactionReceiptRepositoryError,
) -> ContextCompactionRepositoryError {
    match error {
        context_compaction_receipt_repository::ContextCompactionReceiptRepositoryError::Database(
            error,
        ) => ContextCompactionRepositoryError::Database(error),
        context_compaction_receipt_repository::ContextCompactionReceiptRepositoryError::Invalid(
            message,
        ) => ContextCompactionRepositoryError::Invalid(message),
    }
}

pub fn rollback_active_summary(
    connection: &mut Connection,
    conversation_id: &str,
    expected_summary_id: &str,
    updated_at: i64,
) -> Result<Option<ContextCompactionSummary>, ContextCompactionRepositoryError> {
    let transaction = connection.transaction()?;
    let active = get_active_summary(&transaction, conversation_id)?.ok_or_else(|| {
        ContextCompactionRepositoryError::Stale("会话当前没有 active summary。".to_string())
    })?;
    if active.id != expected_summary_id {
        return Err(ContextCompactionRepositoryError::Stale(
            "active summary 已发生变化，拒绝回滚旧版本。".to_string(),
        ));
    }
    let restored = active
        .previous_summary_id
        .as_deref()
        .map(|summary_id| load_summary(&transaction, summary_id))
        .transpose()?;
    ensure_provider_replay_survives_summary_rollback(&transaction, &active, restored.as_ref())?;
    rollback_world_state_epoch_for_summary(
        &transaction,
        conversation_id,
        expected_summary_id,
        restored.as_ref().map(|summary| summary.id.as_str()),
    )?;
    match &restored {
        Some(summary) => {
            let current_revision = transaction.query_row(
                "SELECT revision FROM conversation_context_compaction_heads WHERE conversation_id = ?1",
                [conversation_id],
                |row| row.get::<_, u64>(0),
            )?;
            transaction.execute(
                "UPDATE conversation_context_compaction_heads
                 SET summary_id = ?1, revision = ?2, updated_at = ?3
                 WHERE conversation_id = ?4",
                params![
                    &summary.id,
                    current_revision.saturating_add(1),
                    updated_at,
                    conversation_id,
                ],
            )?;
        }
        None => {
            transaction.execute(
                "DELETE FROM conversation_context_compaction_heads WHERE conversation_id = ?1",
                [conversation_id],
            )?;
        }
    }
    transaction.commit()?;
    Ok(restored)
}

fn ensure_provider_replay_survives_summary_rollback(
    connection: &Connection,
    active: &ContextCompactionSummary,
    restored: Option<&ContextCompactionSummary>,
) -> Result<(), ContextCompactionRepositoryError> {
    if restored.is_some_and(|summary| summary.conversation_id != active.conversation_id) {
        return Err(ContextCompactionRepositoryError::Invalid(
            "待恢复摘要属于其他会话。".to_string(),
        ));
    }
    let entries = list_journal_entries(connection, &active.conversation_id)?;
    let active_index = cursor_index(&entries, &active.covered_through).ok_or_else(|| {
        ContextCompactionRepositoryError::Stale(
            "active summary 的日志游标已不存在，无法安全验证 Provider replay。".to_string(),
        )
    })?;
    let source_start = match restored {
        Some(summary) => {
            let restored_index =
                cursor_index(&entries, &summary.covered_through).ok_or_else(|| {
                    ContextCompactionRepositoryError::Stale(
                        "待恢复摘要的日志游标已不存在，无法安全验证 Provider replay。".to_string(),
                    )
                })?;
            if restored_index >= active_index {
                return Err(ContextCompactionRepositoryError::Invalid(
                    "摘要回滚边界没有严格后退。".to_string(),
                ));
            }
            restored_index + 1
        }
        None => 0,
    };
    let covered_runtime_tool_calls = entries[source_start..=active_index]
        .iter()
        .filter_map(|item| match item {
            ContextCompactionSourceItem::TraceItem { run_id, item, .. } => match item.as_ref() {
                crate::ConversationTurnTraceItem::ToolResult { call_id, .. } => {
                    Some((run_id.clone(), call_id.clone()))
                }
                _ => None,
            },
            ContextCompactionSourceItem::Message { .. } => None,
        })
        .collect::<Vec<_>>();
    if provider_continuation_repository::has_released_for_covered_runtime_tool_calls(
        connection,
        &active.conversation_id,
        &covered_runtime_tool_calls,
    )? {
        return Err(ContextCompactionRepositoryError::Stale(
            "provider_context_boundary_required: 摘要覆盖范围内的 Provider replay 已安全释放，拒绝恢复残缺 Tool Exchange。"
                .to_string(),
        ));
    }
    Ok(())
}

fn rollback_world_state_epoch_for_summary(
    transaction: &Connection,
    conversation_id: &str,
    expected_summary_id: &str,
    restored_summary_id: Option<&str>,
) -> Result<(), ContextCompactionRepositoryError> {
    let epochs = {
        let mut statement = transaction.prepare(
            "SELECT epoch_id, base_summary_id
             FROM conversation_world_state_epochs
             WHERE conversation_id = ?1
             ORDER BY generation DESC
             LIMIT 2",
        )?;
        let rows = statement
            .query_map([conversation_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    let Some((active_epoch_id, active_base_summary_id)) = epochs.first() else {
        // Legacy conversations without a World State journal keep their existing rollback
        // behavior and establish a correctly based initial epoch on the next normal turn.
        return Ok(());
    };
    if active_base_summary_id.as_deref() != Some(expected_summary_id) {
        return Err(ContextCompactionRepositoryError::Stale(
            "active World State epoch 与待回滚摘要不一致。".to_string(),
        ));
    }
    let Some((_, previous_base_summary_id)) = epochs.get(1) else {
        return Err(ContextCompactionRepositoryError::Stale(
            "待回滚摘要没有可恢复的上一 World State epoch。".to_string(),
        ));
    };
    if previous_base_summary_id.as_deref() != restored_summary_id {
        return Err(ContextCompactionRepositoryError::Invalid(
            "上一 World State epoch 的 summary 边界与摘要链不一致。".to_string(),
        ));
    }

    let current = world_state_repository::fold_active_snapshot(transaction, conversation_id)
        .map_err(map_world_state_error)?
        .ok_or_else(|| {
            ContextCompactionRepositoryError::Invalid(
                "active World State epoch 无法折叠。".to_string(),
            )
        })?;
    if !world_state_repository::delete_epoch(transaction, conversation_id, active_epoch_id)
        .map_err(map_world_state_error)?
    {
        return Err(ContextCompactionRepositoryError::Stale(
            "active World State epoch 在回滚期间已变化。".to_string(),
        ));
    }
    let restored = world_state_repository::fold_active_snapshot(transaction, conversation_id)
        .map_err(map_world_state_error)?
        .ok_or_else(|| {
            ContextCompactionRepositoryError::Invalid(
                "上一 World State epoch 无法折叠。".to_string(),
            )
        })?;
    if restored.revision != current.revision {
        // Returning before commit rolls the epoch deletion back with the summary head. Losing
        // state appended after compaction is never an acceptable way to roll a summary back.
        return Err(ContextCompactionRepositoryError::Stale(
            "摘要生成后 World State 已继续变化，无法无损回滚。".to_string(),
        ));
    }
    Ok(())
}

fn list_journal_entries(
    connection: &Connection,
    conversation_id: &str,
) -> Result<Vec<ContextCompactionSourceItem>, ContextCompactionRepositoryError> {
    let rows = {
        let mut statement = connection.prepare(
            "SELECT id, role, content, created_at, status
             FROM messages
             WHERE conversation_id = ?1
             ORDER BY position ASC, created_at ASC, id ASC",
        )?;
        let rows = statement
            .query_map([conversation_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, Option<String>>(4)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };

    let mut entries = Vec::new();
    for (message_id, role, mut content, created_at, status) in rows {
        match role.as_str() {
            "user" => entries.push(ContextCompactionSourceItem::Message {
                cursor: ContextJournalCursor::message(&message_id),
                role,
                content,
                created_at,
                status,
                terminal_status: None,
                terminal_error: None,
            }),
            "assistant" => {
                let trace =
                    conversation_trace_repository::get_trace_for_message(connection, &message_id)?;
                if let Some(trace) = &trace {
                    for item in trace.items.iter().filter(|item| item.is_model_visible()) {
                        entries.push(ContextCompactionSourceItem::TraceItem {
                            cursor: ContextJournalCursor::trace_item(&message_id, item.sequence()),
                            run_id: trace.run_id.clone(),
                            created_at: match item {
                                crate::ConversationTurnTraceItem::UserGuidance {
                                    created_at,
                                    ..
                                } => *created_at,
                                _ => created_at,
                            },
                            item: Box::new(item.clone()),
                        });
                    }
                }

                let terminal_status = trace.as_ref().map(|trace| trace.terminal_status);
                let has_complete_message = trace
                    .as_ref()
                    .is_some_and(|trace| trace.terminal_status.is_terminal())
                    || (trace.is_none() && status.as_deref() != Some("pending"));
                if !has_complete_message || (trace.is_none() && status.as_deref() == Some("error"))
                {
                    continue;
                }
                if status.as_deref() == Some("pending") && content.trim() == "正在思考..." {
                    content.clear();
                }
                entries.push(ContextCompactionSourceItem::Message {
                    cursor: ContextJournalCursor::message(&message_id),
                    role,
                    content,
                    created_at,
                    status,
                    terminal_status,
                    terminal_error: trace.and_then(|trace| trace.terminal_error),
                });
            }
            _ => {
                return Err(ContextCompactionRepositoryError::Invalid(format!(
                    "会话包含未知消息角色：{role}"
                )))
            }
        }
    }
    for entry in &entries {
        entry
            .validate()
            .map_err(|error| ContextCompactionRepositoryError::Invalid(error.to_string()))?;
    }
    Ok(entries)
}

fn cursor_index(
    entries: &[ContextCompactionSourceItem],
    cursor: &ContextJournalCursor,
) -> Option<usize> {
    entries.iter().position(|entry| entry.cursor() == cursor)
}

/// Returns the exact closed runtime Tool calls hidden by a durable summary boundary.
///
/// Fork planning uses this to distinguish released Provider turns that remain visible in the
/// target's raw tail from turns whose complete Tool Exchange is represented by the cloned active
/// summary. Only the latter may be omitted without restoring destroyed Provider state.
pub(crate) fn covered_runtime_tool_calls_through_cursor(
    connection: &Connection,
    conversation_id: &str,
    covered_through: &ContextJournalCursor,
) -> Result<Vec<(String, String)>, ContextCompactionRepositoryError> {
    let entries = list_journal_entries(connection, conversation_id)?;
    let boundary_index = cursor_index(&entries, covered_through).ok_or_else(|| {
        ContextCompactionRepositoryError::Invalid("摘要覆盖游标不属于目标会话日志。".to_string())
    })?;
    Ok(entries[..=boundary_index]
        .iter()
        .filter_map(|item| match item {
            ContextCompactionSourceItem::TraceItem { run_id, item, .. } => match item.as_ref() {
                crate::ConversationTurnTraceItem::ToolResult { call_id, .. } => {
                    Some((run_id.clone(), call_id.clone()))
                }
                _ => None,
            },
            ContextCompactionSourceItem::Message { .. } => None,
        })
        .collect())
}

pub(crate) fn source_revision_for_cursor(
    connection: &Connection,
    conversation_id: &str,
    covered_through: &ContextJournalCursor,
) -> Result<String, ContextCompactionRepositoryError> {
    let entries = list_journal_entries(connection, conversation_id)?;
    let boundary_index = cursor_index(&entries, covered_through).ok_or_else(|| {
        ContextCompactionRepositoryError::Invalid("摘要覆盖游标不属于目标会话日志。".to_string())
    })?;
    source_revision(
        conversation_id,
        covered_through,
        &entries[..=boundary_index],
    )
}

fn source_revision(
    conversation_id: &str,
    covered_through: &ContextJournalCursor,
    complete_raw_prefix: &[ContextCompactionSourceItem],
) -> Result<String, ContextCompactionRepositoryError> {
    let material = serde_json::to_vec(&json!({
        "schemaVersion": 1,
        "conversationId": conversation_id,
        "coveredThrough": covered_through,
        "rawPrefix": complete_raw_prefix,
    }))
    .map_err(|error| {
        ContextCompactionRepositoryError::Invalid(format!("无法序列化上下文日志前缀：{error}"))
    })?;
    Ok(content_revision(&material))
}

pub(crate) fn insert_summary(
    transaction: &Connection,
    summary: &ContextCompactionSummary,
) -> Result<(), ContextCompactionRepositoryError> {
    summary
        .validate()
        .map_err(|error| ContextCompactionRepositoryError::Invalid(error.to_string()))?;
    if !continuity_refs_exist(transaction, summary)? {
        return Err(ContextCompactionRepositoryError::Invalid(
            "Continuity V2 包含已失效或跨会话的历史引用。".to_string(),
        ));
    }
    let (cursor_kind, message_id, trace_sequence) = cursor_columns(&summary.covered_through);
    let continuity_json = serde_json::to_string(&summary.continuity).map_err(|error| {
        ContextCompactionRepositoryError::Invalid(format!("无法序列化上下文连续性骨架：{error}"))
    })?;
    transaction.execute(
        "INSERT INTO context_compaction_summaries (
            id, conversation_id, schema_version, source_revision, previous_summary_id,
            covered_through_kind, covered_through_message_id,
            covered_through_trace_sequence, content, continuity_schema_version,
            continuity_json, generation_kind, generation_model, source_input_tokens,
            summary_input_tokens, continuity_input_tokens, uncovered_tail_input_tokens,
            replacement_input_tokens, created_at
         ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15,
            ?16, ?17, ?18, ?19
         )",
        params![
            &summary.id,
            &summary.conversation_id,
            summary.schema_version,
            &summary.source_revision,
            &summary.previous_summary_id,
            cursor_kind,
            message_id,
            trace_sequence,
            &summary.content,
            summary.continuity.schema_version,
            continuity_json,
            summary.generation.kind.as_str(),
            &summary.generation.model,
            summary.source_input_tokens,
            summary.summary_input_tokens,
            summary.continuity_input_tokens,
            summary.uncovered_tail_input_tokens,
            summary.replacement_input_tokens,
            summary.created_at,
        ],
    )?;
    Ok(())
}

fn continuity_refs_exist(
    connection: &Connection,
    summary: &ContextCompactionSummary,
) -> Result<bool, ContextCompactionRepositoryError> {
    if !summary.continuity.is_v2() {
        return Ok(true);
    }
    for reference in summary.continuity.all_refs() {
        let exists = match reference {
            crate::ContextHistoryRef::Message { message_id } => connection
                .query_row(
                    "SELECT 1 FROM messages WHERE conversation_id = ?1 AND id = ?2",
                    params![&summary.conversation_id, message_id],
                    |_| Ok(()),
                )
                .optional()?
                .is_some(),
            crate::ContextHistoryRef::TraceItem {
                assistant_message_id,
                sequence,
            } => connection
                .query_row(
                    "SELECT 1
                     FROM conversation_turn_trace_items AS item
                     INNER JOIN conversation_turn_traces AS trace
                        ON trace.assistant_message_id = item.assistant_message_id
                     WHERE trace.conversation_id = ?1
                       AND item.assistant_message_id = ?2
                       AND item.sequence = ?3",
                    params![&summary.conversation_id, assistant_message_id, sequence],
                    |_| Ok(()),
                )
                .optional()?
                .is_some(),
            crate::ContextHistoryRef::Archive { archive_ref } => connection
                .query_row(
                    "SELECT 1
                     FROM conversation_history_blobs
                     WHERE conversation_id = ?1 AND archive_ref = ?2",
                    params![&summary.conversation_id, archive_ref],
                    |_| Ok(()),
                )
                .optional()?
                .is_some(),
        };
        if !exists {
            return Ok(false);
        }
    }
    Ok(true)
}

pub(crate) fn insert_summary_lineage(
    connection: &Connection,
    summary: &ContextCompactionSummary,
    lineage: &ContextCompactionSummaryLineage,
) -> Result<(), ContextCompactionRepositoryError> {
    let role = connection
        .query_row(
            "SELECT role FROM messages WHERE id = ?1 AND conversation_id = ?2",
            params![
                &lineage.introduced_by_assistant_message_id,
                &summary.conversation_id
            ],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    if role.as_deref() != Some("assistant") {
        return Err(ContextCompactionRepositoryError::Invalid(
            "摘要的因果归属必须指向同一会话中的 assistant 消息。".to_string(),
        ));
    }
    connection.execute(
        "INSERT INTO context_compaction_summary_lineage (
            summary_id, conversation_id, introduced_by_assistant_message_id,
            source_conversation_id, source_summary_id, created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            &summary.id,
            &summary.conversation_id,
            &lineage.introduced_by_assistant_message_id,
            &lineage.source_conversation_id,
            &lineage.source_summary_id,
            summary.created_at,
        ],
    )?;
    Ok(())
}

pub(crate) fn list_active_summary_chain(
    connection: &Connection,
    conversation_id: &str,
) -> Result<Vec<ContextCompactionSummaryVersion>, ContextCompactionRepositoryError> {
    let Some(mut current) = get_active_summary(connection, conversation_id)? else {
        return Ok(Vec::new());
    };
    let mut newest_first = Vec::new();
    let mut visited = HashSet::new();
    loop {
        if !visited.insert(current.id.clone()) {
            return Err(ContextCompactionRepositoryError::Invalid(
                "上下文压缩摘要链存在循环。".to_string(),
            ));
        }
        let lineage = load_summary_lineage(connection, &current.id)?;
        let previous_id = current.previous_summary_id.clone();
        newest_first.push(ContextCompactionSummaryVersion {
            summary: current,
            lineage,
        });
        let Some(previous_id) = previous_id else {
            break;
        };
        current = load_summary(connection, &previous_id)?;
    }
    newest_first.reverse();
    Ok(newest_first)
}

pub(crate) fn set_active_summary_head(
    connection: &Connection,
    conversation_id: &str,
    summary_id: &str,
    revision: u64,
    updated_at: i64,
) -> Result<(), ContextCompactionRepositoryError> {
    connection.execute(
        "INSERT INTO conversation_context_compaction_heads (
            conversation_id, summary_id, revision, updated_at
         ) VALUES (?1, ?2, ?3, ?4)",
        params![conversation_id, summary_id, revision.max(1), updated_at],
    )?;
    Ok(())
}

fn load_summary_lineage(
    connection: &Connection,
    summary_id: &str,
) -> Result<ContextCompactionSummaryLineage, ContextCompactionRepositoryError> {
    connection
        .query_row(
            "SELECT introduced_by_assistant_message_id, source_conversation_id, source_summary_id
             FROM context_compaction_summary_lineage
             WHERE summary_id = ?1",
            [summary_id],
            |row| {
                Ok(ContextCompactionSummaryLineage {
                    introduced_by_assistant_message_id: row.get(0)?,
                    source_conversation_id: row.get(1)?,
                    source_summary_id: row.get(2)?,
                })
            },
        )
        .optional()?
        .ok_or_else(|| {
            ContextCompactionRepositoryError::Invalid(format!("摘要缺少因果归属记录：{summary_id}"))
        })
}

pub(crate) fn load_summary(
    connection: &Connection,
    summary_id: &str,
) -> Result<ContextCompactionSummary, ContextCompactionRepositoryError> {
    let row = connection
        .query_row(
            "SELECT
                id, conversation_id, schema_version, source_revision, previous_summary_id,
                covered_through_kind, covered_through_message_id,
                covered_through_trace_sequence, content, continuity_schema_version,
                continuity_json, generation_kind, generation_model, source_input_tokens,
                summary_input_tokens, continuity_input_tokens, uncovered_tail_input_tokens,
                replacement_input_tokens, created_at
             FROM context_compaction_summaries
             WHERE id = ?1",
            [summary_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, u32>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, Option<u64>>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, u32>(9)?,
                    row.get::<_, String>(10)?,
                    row.get::<_, String>(11)?,
                    row.get::<_, Option<String>>(12)?,
                    row.get::<_, u64>(13)?,
                    row.get::<_, u64>(14)?,
                    row.get::<_, u64>(15)?,
                    row.get::<_, u64>(16)?,
                    row.get::<_, u64>(17)?,
                    row.get::<_, i64>(18)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| {
            ContextCompactionRepositoryError::Invalid(format!("找不到摘要版本：{summary_id}"))
        })?;
    let generation_kind = ContextCompactionGenerationKind::from_str(&row.11).ok_or_else(|| {
        ContextCompactionRepositoryError::Invalid(format!("摘要包含未知生成方式：{}", row.11))
    })?;
    let continuity: crate::context::ContextContinuitySnapshot = serde_json::from_str(&row.10)
        .map_err(|error| {
            ContextCompactionRepositoryError::Invalid(format!("无法解析上下文连续性骨架：{error}"))
        })?;
    if continuity.schema_version != row.9 {
        return Err(ContextCompactionRepositoryError::Invalid(
            "上下文连续性骨架 schema 列与 JSON 不一致。".to_string(),
        ));
    }
    let summary = ContextCompactionSummary {
        schema_version: row.2,
        id: row.0,
        conversation_id: row.1,
        source_revision: row.3,
        previous_summary_id: row.4,
        covered_through: cursor_from_columns(&row.5, row.6, row.7)?,
        content: row.8,
        continuity,
        generation: ContextCompactionGeneration {
            kind: generation_kind,
            model: row.12,
        },
        source_input_tokens: row.13,
        summary_input_tokens: row.14,
        continuity_input_tokens: row.15,
        uncovered_tail_input_tokens: row.16,
        replacement_input_tokens: row.17,
        created_at: row.18,
    };
    summary
        .validate()
        .map_err(|error| ContextCompactionRepositoryError::Invalid(error.to_string()))?;
    Ok(summary)
}

fn summary_matches_current_raw_prefix(
    connection: &Connection,
    summary: &ContextCompactionSummary,
) -> Result<bool, ContextCompactionRepositoryError> {
    let entries = list_journal_entries(connection, &summary.conversation_id)?;
    let Some(index) = cursor_index(&entries, &summary.covered_through) else {
        return Ok(false);
    };
    Ok(source_revision(
        &summary.conversation_id,
        &summary.covered_through,
        &entries[..=index],
    )? == summary.source_revision)
}

fn cursor_columns(cursor: &ContextJournalCursor) -> (&'static str, &str, Option<u64>) {
    match cursor {
        ContextJournalCursor::Message { message_id } => ("message", message_id, None),
        ContextJournalCursor::TraceItem {
            assistant_message_id,
            sequence,
        } => ("trace_item", assistant_message_id, Some(*sequence)),
    }
}

fn cursor_from_columns(
    kind: &str,
    message_id: String,
    trace_sequence: Option<u64>,
) -> Result<ContextJournalCursor, ContextCompactionRepositoryError> {
    match (kind, trace_sequence) {
        ("message", None) => Ok(ContextJournalCursor::message(message_id)),
        ("trace_item", Some(sequence)) => {
            Ok(ContextJournalCursor::trace_item(message_id, sequence))
        }
        _ => Err(ContextCompactionRepositoryError::Invalid(
            "摘要日志游标列不一致。".to_string(),
        )),
    }
}

#[cfg(test)]
mod tests;
