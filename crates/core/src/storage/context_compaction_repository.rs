use crate::content_revision;
use crate::context::{
    ContextCompactionGeneration, ContextCompactionGenerationKind, ContextCompactionPrefix,
    ContextCompactionSourceItem, ContextCompactionSummary, ContextCompactionSummaryDraft,
    ContextJournalCursor,
};
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
    if !summary_matches_current_raw_prefix(connection, &summary)? {
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
) -> Result<ContextCompactionSummary, ContextCompactionRepositoryError> {
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
            summary.created_at,
        ],
    )?;
    transaction.commit()?;
    Ok(summary)
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
                    for item in &trace.items {
                        entries.push(ContextCompactionSourceItem::TraceItem {
                            cursor: ContextJournalCursor::trace_item(&message_id, item.sequence()),
                            run_id: trace.run_id.clone(),
                            item: item.clone(),
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

fn insert_summary(
    transaction: &Transaction<'_>,
    summary: &ContextCompactionSummary,
) -> Result<(), ContextCompactionRepositoryError> {
    summary
        .validate()
        .map_err(|error| ContextCompactionRepositoryError::Invalid(error.to_string()))?;
    let (cursor_kind, message_id, trace_sequence) = cursor_columns(&summary.covered_through);
    transaction.execute(
        "INSERT INTO context_compaction_summaries (
            id, conversation_id, schema_version, source_revision, previous_summary_id,
            covered_through_kind, covered_through_message_id,
            covered_through_trace_sequence, content, generation_kind, generation_model,
            source_input_tokens, summary_input_tokens, created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
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
            summary.generation.kind.as_str(),
            &summary.generation.model,
            summary.source_input_tokens,
            summary.summary_input_tokens,
            summary.created_at,
        ],
    )?;
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
                covered_through_kind, covered_through_message_id,
                covered_through_trace_sequence, content, generation_kind, generation_model,
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
                    row.get::<_, Option<u64>>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, String>(9)?,
                    row.get::<_, Option<String>>(10)?,
                    row.get::<_, u64>(11)?,
                    row.get::<_, u64>(12)?,
                    row.get::<_, i64>(13)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| {
            ContextCompactionRepositoryError::Invalid(format!("找不到摘要版本：{summary_id}"))
        })?;
    let generation_kind = ContextCompactionGenerationKind::from_str(&row.9).ok_or_else(|| {
        ContextCompactionRepositoryError::Invalid(format!("摘要包含未知生成方式：{}", row.9))
    })?;
    let summary = ContextCompactionSummary {
        schema_version: row.2,
        id: row.0,
        conversation_id: row.1,
        source_revision: row.3,
        previous_summary_id: row.4,
        covered_through: cursor_from_columns(&row.5, row.6, row.7)?,
        content: row.8,
        generation: ContextCompactionGeneration {
            kind: generation_kind,
            model: row.10,
        },
        source_input_tokens: row.11,
        summary_input_tokens: row.12,
        created_at: row.13,
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
mod tests {
    use super::*;
    use crate::storage::migrations;
    use crate::{
        AgentApprovalStatus, ConversationTraceToolResultStatus, ConversationTurnTrace,
        ConversationTurnTraceItem, ConversationTurnTraceTerminalStatus,
        CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
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
        for (position, id, role, content, status) in [
            (0, "user-1", "user", "first request", "sent"),
            (1, "assistant-1", "assistant", "first answer", "sent"),
            (2, "user-2", "user", "current request", "sent"),
            (3, "assistant-2", "assistant", "正在思考...", "pending"),
        ] {
            connection
                .execute(
                    "INSERT INTO messages (
                        id, conversation_id, role, content, status, agent_run_json,
                        ui_state_json, created_at, position
                     ) VALUES (?1, 'conversation-1', ?2, ?3, ?4, NULL, NULL, ?5, ?5)",
                    params![id, role, content, status, position],
                )
                .unwrap();
        }
        let trace = ConversationTurnTrace {
            schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
            run_id: "run-2".to_string(),
            conversation_id: "conversation-1".to_string(),
            assistant_message_id: "assistant-2".to_string(),
            terminal_status: ConversationTurnTraceTerminalStatus::InProgress,
            terminal_error: None,
            truncated: false,
            items: vec![
                ConversationTurnTraceItem::ToolCall {
                    sequence: 0,
                    call_id: "call-1".to_string(),
                    tool: "web_fetch".to_string(),
                    operation: json!({ "url": "https://example.com" }),
                    approval_status: AgentApprovalStatus::NotRequired,
                    truncated: false,
                },
                ConversationTurnTraceItem::ToolResult {
                    sequence: 1,
                    call_id: "call-1".to_string(),
                    tool: "web_fetch".to_string(),
                    status: ConversationTraceToolResultStatus::Succeeded,
                    success: true,
                    observation: json!({ "content": "page body" }),
                    approval_status: AgentApprovalStatus::NotRequired,
                    error: None,
                    truncated: false,
                },
            ],
        };
        conversation_trace_repository::commit_trace_in_connection(&connection, &trace, 3, 4)
            .unwrap();
        connection
    }

    fn draft(prefix: &ContextCompactionPrefix, id: &str) -> ContextCompactionSummaryDraft {
        ContextCompactionSummaryDraft {
            id: id.to_string(),
            source_revision: prefix.source_revision.clone(),
            content: format!("summary {id}"),
            generation: ContextCompactionGeneration::test(),
            source_input_tokens: 100,
            summary_input_tokens: 10,
            created_at: 10,
        }
    }

    #[test]
    fn compacts_complete_history_then_advances_inside_current_run() {
        let mut connection = setup();
        let first_cursor = ContextJournalCursor::message("assistant-1");
        let first = prepare_prefix(&connection, "conversation-1", &first_cursor).unwrap();
        commit_prefix_replacement(&mut connection, &first, draft(&first, "summary-1")).unwrap();

        let run_cursor = ContextJournalCursor::trace_item("assistant-2", 1);
        let second = prepare_prefix(&connection, "conversation-1", &run_cursor).unwrap();
        assert!(second.source_items.iter().any(|item| {
            matches!(item, ContextCompactionSourceItem::TraceItem { cursor, .. } if cursor == &run_cursor)
        }));
        let summary =
            commit_prefix_replacement(&mut connection, &second, draft(&second, "summary-2"))
                .unwrap();
        assert_eq!(summary.previous_summary_id.as_deref(), Some("summary-1"));
        assert_eq!(summary.covered_through, run_cursor);

        let raw_count = connection
            .query_row(
                "SELECT COUNT(*) FROM messages WHERE conversation_id = 'conversation-1'",
                [],
                |row| row.get::<_, u64>(0),
            )
            .unwrap();
        assert_eq!(raw_count, 4);
    }

    #[test]
    fn appending_after_a_trace_cursor_keeps_the_summary_valid() {
        let mut connection = setup();
        let cursor = ContextJournalCursor::trace_item("assistant-2", 1);
        let prefix = prepare_prefix(&connection, "conversation-1", &cursor).unwrap();
        commit_prefix_replacement(&mut connection, &prefix, draft(&prefix, "summary-1")).unwrap();

        let mut trace =
            conversation_trace_repository::get_trace_for_message(&connection, "assistant-2")
                .unwrap()
                .unwrap();
        trace
            .items
            .push(ConversationTurnTraceItem::AssistantNarration {
                sequence: 2,
                content: "continue".to_string(),
                truncated: false,
            });
        conversation_trace_repository::commit_trace_in_connection(&connection, &trace, 3, 5)
            .unwrap();

        assert_eq!(
            get_active_summary(&connection, "conversation-1")
                .unwrap()
                .unwrap()
                .id,
            "summary-1"
        );
    }

    #[test]
    fn mutation_inside_covered_raw_prefix_drops_the_derived_head() {
        let mut connection = setup();
        let cursor = ContextJournalCursor::message("assistant-1");
        let prefix = prepare_prefix(&connection, "conversation-1", &cursor).unwrap();
        commit_prefix_replacement(&mut connection, &prefix, draft(&prefix, "summary-1")).unwrap();
        connection
            .execute(
                "UPDATE messages SET content = 'edited request' WHERE id = 'user-1'",
                [],
            )
            .unwrap();

        assert!(get_active_summary(&connection, "conversation-1")
            .unwrap()
            .is_none());
    }
}
