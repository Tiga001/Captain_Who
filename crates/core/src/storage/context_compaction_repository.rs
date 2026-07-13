use crate::content_revision;
use crate::context::{
    ContextCompactionGeneration, ContextCompactionGenerationKind, ContextCompactionPrefix,
    ContextCompactionSourceMessage, ContextCompactionSummary, ContextCompactionSummaryDraft,
};
use crate::conversation_trace::ConversationTurnTraceTerminalStatus;
use crate::storage::conversation_trace_repository;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde_json::json;
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
    validate_summary_against_current_prefix(connection, &summary)?;
    Ok(Some(summary))
}

pub fn prepare_prefix(
    connection: &Connection,
    conversation_id: &str,
    covered_through_message_id: &str,
) -> Result<ContextCompactionPrefix, ContextCompactionRepositoryError> {
    let messages = list_source_messages(connection, conversation_id)?;
    if messages.is_empty() {
        return Err(ContextCompactionRepositoryError::Invalid(
            "会话没有可压缩的 durable 消息。".to_string(),
        ));
    }
    let boundary_index = messages
        .iter()
        .position(|message| message.message_id == covered_through_message_id)
        .ok_or_else(|| {
            ContextCompactionRepositoryError::Invalid(format!(
                "覆盖边界消息不属于会话：{covered_through_message_id}"
            ))
        })?;
    let selected = &messages[..=boundary_index];
    validate_compactable_prefix(selected)?;

    let previous_summary = get_active_summary(connection, conversation_id)?;
    let previous_count = previous_summary
        .as_ref()
        .map_or(0, |summary| summary.covered_message_ids.len());
    let covered_message_ids = selected
        .iter()
        .map(|message| message.message_id.clone())
        .collect::<Vec<_>>();
    if previous_count >= covered_message_ids.len() {
        return Err(ContextCompactionRepositoryError::Invalid(
            "新的 durable 前缀必须扩展当前摘要的覆盖范围。".to_string(),
        ));
    }
    if let Some(previous) = &previous_summary {
        if covered_message_ids[..previous_count] != previous.covered_message_ids {
            return Err(ContextCompactionRepositoryError::Stale(
                "当前消息前缀已不再匹配 active summary。".to_string(),
            ));
        }
    }
    let source_messages = selected[previous_count..].to_vec();
    let source_revision = source_revision(
        conversation_id,
        &covered_message_ids,
        previous_summary.as_ref(),
        &source_messages,
    )?;
    let prefix = ContextCompactionPrefix {
        conversation_id: conversation_id.to_string(),
        source_revision,
        covered_through_message_id: covered_through_message_id.to_string(),
        covered_message_ids,
        previous_summary,
        source_messages,
    };
    prefix
        .validate()
        .map_err(|error| ContextCompactionRepositoryError::Invalid(error.to_string()))?;
    Ok(prefix)
}

/// Commits an immutable summary and switches the active projection in one transaction.
///
/// The source snapshot is rebuilt inside the transaction. A caller can therefore perform summary
/// generation outside the write lock without risking replacement of a newer history revision.
pub fn commit_prefix_replacement(
    connection: &mut Connection,
    expected_prefix: &ContextCompactionPrefix,
    draft: ContextCompactionSummaryDraft,
) -> Result<ContextCompactionSummary, ContextCompactionRepositoryError> {
    expected_prefix
        .validate()
        .map_err(|error| ContextCompactionRepositoryError::Invalid(error.to_string()))?;
    draft
        .validate()
        .map_err(|error| ContextCompactionRepositoryError::Invalid(error.to_string()))?;
    if draft.source_revision != expected_prefix.source_revision {
        return Err(ContextCompactionRepositoryError::Stale(
            "摘要草稿不是基于待替换 durable 前缀生成的。".to_string(),
        ));
    }

    let transaction = connection.transaction()?;
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
        &transaction,
        &expected_prefix.conversation_id,
        &expected_prefix.covered_through_message_id,
    )?;
    if current_prefix.source_revision != expected_prefix.source_revision
        || current_prefix.covered_message_ids != expected_prefix.covered_message_ids
        || current_prefix
            .previous_summary
            .as_ref()
            .map(|summary| summary.id.as_str())
            != expected_prefix
                .previous_summary
                .as_ref()
                .map(|summary| summary.id.as_str())
    {
        return Err(ContextCompactionRepositoryError::Stale(
            "摘要生成期间 durable 历史或 active head 已发生变化。".to_string(),
        ));
    }

    let summary = draft
        .finish(&current_prefix)
        .map_err(|error| ContextCompactionRepositoryError::Invalid(error.to_string()))?;
    insert_summary(&transaction, &summary)?;
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
            summary.created_at
        ],
    )?;
    transaction.commit()?;
    Ok(summary)
}

/// Atomically restores the previous immutable summary version, or the raw history if this was the
/// first compaction. The expected ID prevents rolling back a head that changed concurrently.
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
                    summary.id,
                    current_revision.saturating_add(1),
                    updated_at,
                    conversation_id
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

fn list_source_messages(
    connection: &Connection,
    conversation_id: &str,
) -> Result<Vec<ContextCompactionSourceMessage>, ContextCompactionRepositoryError> {
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
    rows.into_iter()
        .map(|(message_id, role, content, created_at, status)| {
            let conversation_turn_trace = if role == "assistant" {
                conversation_trace_repository::get_trace_for_message(connection, &message_id)?
            } else {
                None
            };
            Ok(ContextCompactionSourceMessage {
                message_id,
                role,
                content,
                created_at,
                status,
                conversation_turn_trace,
            })
        })
        .collect()
}

fn validate_compactable_prefix(
    messages: &[ContextCompactionSourceMessage],
) -> Result<(), ContextCompactionRepositoryError> {
    if messages
        .first()
        .is_none_or(|message| message.role != "user")
        || messages
            .last()
            .is_none_or(|message| message.role != "assistant")
    {
        return Err(ContextCompactionRepositoryError::Invalid(
            "durable 前缀必须从 user 消息开始并在完整 assistant turn 后结束。".to_string(),
        ));
    }
    for (index, message) in messages.iter().enumerate() {
        if !matches!(message.role.as_str(), "user" | "assistant") {
            return Err(ContextCompactionRepositoryError::Invalid(format!(
                "durable 前缀包含不支持的消息角色：{}",
                message.role
            )));
        }
        let expected_role = if index % 2 == 0 { "user" } else { "assistant" };
        if message.role != expected_role {
            return Err(ContextCompactionRepositoryError::Invalid(format!(
                "durable 前缀必须由完整的 user/assistant turn 顺序组成，消息 {} 的角色应为 {expected_role}。",
                message.message_id
            )));
        }
        if message.status.as_deref() == Some("pending") {
            return Err(ContextCompactionRepositoryError::Invalid(format!(
                "durable 前缀不能覆盖运行中的消息：{}",
                message.message_id
            )));
        }
        if message
            .conversation_turn_trace
            .as_ref()
            .is_some_and(|trace| {
                trace.terminal_status == ConversationTurnTraceTerminalStatus::InProgress
            })
        {
            return Err(ContextCompactionRepositoryError::Invalid(format!(
                "durable 前缀不能覆盖运行中的 trace：{}",
                message.message_id
            )));
        }
    }
    Ok(())
}

fn source_revision(
    conversation_id: &str,
    covered_message_ids: &[String],
    previous_summary: Option<&ContextCompactionSummary>,
    source_messages: &[ContextCompactionSourceMessage],
) -> Result<String, ContextCompactionRepositoryError> {
    let material = serde_json::to_vec(&json!({
        "schemaVersion": 2,
        "conversationId": conversation_id,
        "coveredMessageIds": covered_message_ids,
        "previousSummary": previous_summary,
        "sourceMessages": source_messages,
    }))
    .map_err(|error| {
        ContextCompactionRepositoryError::Invalid(format!("无法序列化 durable 前缀：{error}"))
    })?;
    Ok(content_revision(&material))
}

fn insert_summary(
    transaction: &Transaction<'_>,
    summary: &ContextCompactionSummary,
) -> Result<(), ContextCompactionRepositoryError> {
    summary
        .validate()
        .map_err(|error| ContextCompactionRepositoryError::Invalid(error.to_string()))?;
    transaction.execute(
        "INSERT INTO context_compaction_summaries (
            id, conversation_id, schema_version, source_revision, previous_summary_id,
            covered_through_message_id, content, generation_kind, generation_model,
            source_input_tokens, summary_input_tokens, created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            &summary.id,
            &summary.conversation_id,
            summary.schema_version,
            &summary.source_revision,
            &summary.previous_summary_id,
            &summary.covered_through_message_id,
            &summary.content,
            summary.generation.kind.as_str(),
            &summary.generation.model,
            summary.source_input_tokens,
            summary.summary_input_tokens,
            summary.created_at,
        ],
    )?;
    for (ordinal, message_id) in summary.covered_message_ids.iter().enumerate() {
        transaction.execute(
            "INSERT INTO context_compaction_summary_sources (summary_id, ordinal, message_id)
             VALUES (?1, ?2, ?3)",
            params![&summary.id, ordinal as u64, message_id],
        )?;
    }
    Ok(())
}

fn load_summary(
    connection: &Connection,
    summary_id: &str,
) -> Result<ContextCompactionSummary, ContextCompactionRepositoryError> {
    let row = connection
        .query_row(
            "SELECT
                id, conversation_id, schema_version, source_revision, previous_summary_id,
                covered_through_message_id, content, generation_kind, generation_model,
                source_input_tokens, summary_input_tokens, created_at
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
                    row.get::<_, String>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, u64>(9)?,
                    row.get::<_, u64>(10)?,
                    row.get::<_, i64>(11)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| {
            ContextCompactionRepositoryError::Invalid(format!("找不到摘要版本：{summary_id}"))
        })?;
    let covered_message_ids = {
        let mut statement = connection.prepare(
            "SELECT message_id
             FROM context_compaction_summary_sources
             WHERE summary_id = ?1
             ORDER BY ordinal ASC",
        )?;
        let message_ids = statement
            .query_map([summary_id], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        message_ids
    };
    let generation_kind = ContextCompactionGenerationKind::from_str(&row.7).ok_or_else(|| {
        ContextCompactionRepositoryError::Invalid(format!("摘要包含未知生成方式：{}", row.7))
    })?;
    let summary = ContextCompactionSummary {
        schema_version: row.2,
        id: row.0,
        conversation_id: row.1,
        source_revision: row.3,
        previous_summary_id: row.4,
        covered_through_message_id: row.5,
        covered_message_ids,
        content: row.6,
        generation: ContextCompactionGeneration {
            kind: generation_kind,
            model: row.8,
        },
        source_input_tokens: row.9,
        summary_input_tokens: row.10,
        created_at: row.11,
    };
    summary
        .validate()
        .map_err(|error| ContextCompactionRepositoryError::Invalid(error.to_string()))?;
    Ok(summary)
}

fn validate_summary_against_current_prefix(
    connection: &Connection,
    summary: &ContextCompactionSummary,
) -> Result<(), ContextCompactionRepositoryError> {
    let current_ids = {
        let mut statement = connection.prepare(
            "SELECT id
             FROM messages
             WHERE conversation_id = ?1
             ORDER BY position ASC, created_at ASC, id ASC
             LIMIT ?2",
        )?;
        let message_ids = statement
            .query_map(
                params![
                    &summary.conversation_id,
                    u64::try_from(summary.covered_message_ids.len()).unwrap_or(u64::MAX)
                ],
                |row| row.get::<_, String>(0),
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        message_ids
    };
    if current_ids != summary.covered_message_ids {
        return Err(ContextCompactionRepositoryError::Stale(
            "active summary 不再覆盖当前会话的连续消息前缀。".to_string(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::migrations;

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
        for (position, id, role, content) in [
            (0, "user-1", "user", "first request"),
            (1, "assistant-1", "assistant", "first answer"),
            (2, "user-2", "user", "second request"),
            (3, "assistant-2", "assistant", "second answer"),
            (4, "user-3", "user", "current request"),
        ] {
            connection
                .execute(
                    "INSERT INTO messages (
                        id, conversation_id, role, content, status, agent_run_json,
                        ui_state_json, created_at, position
                     ) VALUES (?1, 'conversation-1', ?2, ?3, 'sent', NULL, NULL, ?4, ?4)",
                    params![id, role, content, position],
                )
                .unwrap();
        }
        connection
    }

    fn draft(
        prefix: &ContextCompactionPrefix,
        id: &str,
        content: &str,
        created_at: i64,
    ) -> ContextCompactionSummaryDraft {
        ContextCompactionSummaryDraft {
            id: id.to_string(),
            source_revision: prefix.source_revision.clone(),
            content: content.to_string(),
            generation: ContextCompactionGeneration::test(),
            source_input_tokens: 100,
            summary_input_tokens: 10,
            created_at,
        }
    }

    #[test]
    fn commits_recursive_prefix_versions_without_deleting_raw_history() {
        let mut connection = setup();
        let first_prefix = prepare_prefix(&connection, "conversation-1", "assistant-1").unwrap();
        assert_eq!(first_prefix.source_messages[0].created_at, 0);
        assert_eq!(first_prefix.source_messages[1].created_at, 1);
        let first = commit_prefix_replacement(
            &mut connection,
            &first_prefix,
            draft(&first_prefix, "summary-1", "first turn summary", 10),
        )
        .unwrap();
        assert_eq!(
            get_active_summary(&connection, "conversation-1").unwrap(),
            Some(first)
        );

        let second_prefix = prepare_prefix(&connection, "conversation-1", "assistant-2").unwrap();
        assert_eq!(second_prefix.source_messages.len(), 2);
        assert_eq!(second_prefix.source_messages[0].message_id, "user-2");
        let second = commit_prefix_replacement(
            &mut connection,
            &second_prefix,
            draft(&second_prefix, "summary-2", "first two turns summary", 20),
        )
        .unwrap();
        assert_eq!(second.previous_summary_id.as_deref(), Some("summary-1"));
        assert_eq!(second.covered_message_ids.len(), 4);
        let raw_message_count = connection
            .query_row(
                "SELECT COUNT(*) FROM messages WHERE conversation_id = 'conversation-1'",
                [],
                |row| row.get::<_, u64>(0),
            )
            .unwrap();
        assert_eq!(raw_message_count, 5);

        let restored = rollback_active_summary(&mut connection, "conversation-1", "summary-2", 30)
            .unwrap()
            .unwrap();
        assert_eq!(restored.id, "summary-1");
        assert_eq!(
            get_active_summary(&connection, "conversation-1")
                .unwrap()
                .unwrap()
                .id,
            "summary-1"
        );
    }

    #[test]
    fn stale_snapshot_cannot_replace_a_newer_head() {
        let mut connection = setup();
        let stale = prepare_prefix(&connection, "conversation-1", "assistant-1").unwrap();
        let fresh = prepare_prefix(&connection, "conversation-1", "assistant-1").unwrap();
        commit_prefix_replacement(
            &mut connection,
            &fresh,
            draft(&fresh, "summary-1", "fresh summary", 10),
        )
        .unwrap();

        let error = commit_prefix_replacement(
            &mut connection,
            &stale,
            draft(&stale, "summary-stale", "stale summary", 11),
        )
        .unwrap_err();
        assert!(error
            .to_string()
            .contains("stale_context_compaction_prefix"));
        assert_eq!(
            get_active_summary(&connection, "conversation-1")
                .unwrap()
                .unwrap()
                .id,
            "summary-1"
        );
    }

    #[test]
    fn draft_cannot_be_committed_against_a_different_source_revision() {
        let mut connection = setup();
        let prefix = prepare_prefix(&connection, "conversation-1", "assistant-1").unwrap();
        let mut mismatched = draft(&prefix, "summary-wrong-source", "summary", 10);
        mismatched.source_revision = "another-source-revision".to_string();

        let error = commit_prefix_replacement(&mut connection, &prefix, mismatched).unwrap_err();

        assert!(error
            .to_string()
            .contains("stale_context_compaction_prefix"));
        assert!(get_active_summary(&connection, "conversation-1")
            .unwrap()
            .is_none());
    }

    #[test]
    fn covered_source_mutation_invalidates_all_summary_versions() {
        let mut connection = setup();
        let prefix = prepare_prefix(&connection, "conversation-1", "assistant-1").unwrap();
        commit_prefix_replacement(
            &mut connection,
            &prefix,
            draft(&prefix, "summary-1", "summary", 10),
        )
        .unwrap();

        connection
            .execute(
                "UPDATE messages SET content = 'edited request' WHERE id = 'user-1'",
                [],
            )
            .unwrap();

        assert!(get_active_summary(&connection, "conversation-1")
            .unwrap()
            .is_none());
        let summary_count = connection
            .query_row(
                "SELECT COUNT(*) FROM context_compaction_summaries",
                [],
                |row| row.get::<_, u64>(0),
            )
            .unwrap();
        assert_eq!(summary_count, 0);
    }

    #[test]
    fn covered_source_timestamp_mutation_invalidates_all_summary_versions() {
        let mut connection = setup();
        let prefix = prepare_prefix(&connection, "conversation-1", "assistant-1").unwrap();
        commit_prefix_replacement(
            &mut connection,
            &prefix,
            draft(&prefix, "summary-1", "summary", 10),
        )
        .unwrap();

        connection
            .execute(
                "UPDATE messages SET created_at = 100 WHERE id = 'user-1'",
                [],
            )
            .unwrap();

        assert!(get_active_summary(&connection, "conversation-1")
            .unwrap()
            .is_none());
        let summary_count = connection
            .query_row(
                "SELECT COUNT(*) FROM context_compaction_summaries",
                [],
                |row| row.get::<_, u64>(0),
            )
            .unwrap();
        assert_eq!(summary_count, 0);
    }

    #[test]
    fn failed_insert_keeps_previous_head_active() {
        let mut connection = setup();
        let first_prefix = prepare_prefix(&connection, "conversation-1", "assistant-1").unwrap();
        commit_prefix_replacement(
            &mut connection,
            &first_prefix,
            draft(&first_prefix, "summary-1", "first summary", 10),
        )
        .unwrap();
        let second_prefix = prepare_prefix(&connection, "conversation-1", "assistant-2").unwrap();
        connection
            .execute_batch(
                "CREATE TRIGGER reject_test_summary
                 BEFORE INSERT ON context_compaction_summaries
                 WHEN NEW.id = 'summary-2'
                 BEGIN
                    SELECT RAISE(ABORT, 'injected failure');
                 END;",
            )
            .unwrap();

        assert!(commit_prefix_replacement(
            &mut connection,
            &second_prefix,
            draft(&second_prefix, "summary-2", "second summary", 20),
        )
        .is_err());
        assert_eq!(
            get_active_summary(&connection, "conversation-1")
                .unwrap()
                .unwrap()
                .id,
            "summary-1"
        );
    }
}
