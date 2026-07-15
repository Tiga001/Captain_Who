use crate::storage::models::{
    ChatConversationMetaRecord, ChatConversationRecord, ChatMessageRecord, ChatMessageStateRecord,
};
use crate::storage::{context_compaction_repository, now_ms};
use rusqlite::{params, Connection, OptionalExtension, Row};
use std::collections::HashSet;

pub fn conversation_exists(
    connection: &Connection,
    conversation_id: &str,
) -> rusqlite::Result<bool> {
    connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM conversations WHERE id = ?1)",
        params![conversation_id],
        |row| row.get(0),
    )
}

pub fn list_conversations(
    connection: &Connection,
) -> rusqlite::Result<Vec<ChatConversationRecord>> {
    let mut conversation_statement = connection.prepare(
        "
        SELECT id, project_id, model_id, title, created_at, updated_at, pinned_at, archived_at, unread_at
        FROM conversations
        ORDER BY
            CASE WHEN pinned_at IS NULL THEN 1 ELSE 0 END ASC,
            pinned_at DESC,
            updated_at DESC
        ",
    )?;

    let mut conversations = conversation_statement
        .query_map([], conversation_from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    for conversation in &mut conversations {
        conversation.messages = list_messages(connection, &conversation.id)?;
    }

    Ok(conversations)
}

pub fn get_conversation(
    connection: &Connection,
    conversation_id: &str,
) -> rusqlite::Result<Option<ChatConversationRecord>> {
    let mut conversation = connection
        .query_row(
            "
            SELECT id, project_id, model_id, title, created_at, updated_at, pinned_at, archived_at, unread_at
            FROM conversations
            WHERE id = ?1
            ",
            params![conversation_id],
            conversation_from_row,
        )
        .optional()?;
    if let Some(conversation) = &mut conversation {
        conversation.messages = list_messages(connection, &conversation.id)?;
    }
    Ok(conversation)
}

pub fn get_assistant_message_created_at(
    connection: &Connection,
    conversation_id: &str,
    message_id: &str,
) -> rusqlite::Result<Option<i64>> {
    connection
        .query_row(
            "
            SELECT created_at
            FROM messages
            WHERE conversation_id = ?1 AND id = ?2 AND role = 'assistant'
            ",
            params![conversation_id, message_id],
            |row| row.get(0),
        )
        .optional()
}

fn conversation_from_row(row: &Row<'_>) -> rusqlite::Result<ChatConversationRecord> {
    Ok(ChatConversationRecord {
        id: row.get(0)?,
        project_id: row.get(1)?,
        model_id: row.get(2)?,
        title: row.get(3)?,
        messages: Vec::new(),
        created_at: row.get(4)?,
        updated_at: row.get(5)?,
        pinned_at: row.get(6)?,
        archived_at: row.get(7)?,
        unread_at: row.get(8)?,
    })
}

pub fn save_conversation(
    connection: &mut Connection,
    conversation: ChatConversationRecord,
) -> rusqlite::Result<()> {
    let transaction = connection.transaction()?;

    transaction.execute(
        "
        INSERT INTO conversations (
            id,
            project_id,
            model_id,
            title,
            created_at,
            updated_at,
            pinned_at,
            archived_at,
            unread_at
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
        ON CONFLICT(id) DO UPDATE SET
            project_id = excluded.project_id,
            model_id = excluded.model_id,
            title = excluded.title,
            updated_at = excluded.updated_at,
            pinned_at = excluded.pinned_at,
            archived_at = excluded.archived_at,
            unread_at = excluded.unread_at
        ",
        params![
            &conversation.id,
            &conversation.project_id,
            &conversation.model_id,
            &conversation.title,
            conversation.created_at,
            conversation.updated_at,
            conversation.pinned_at,
            conversation.archived_at,
            conversation.unread_at
        ],
    )?;

    for (index, message) in conversation.messages.iter().enumerate() {
        let existing_conversation_id = transaction
            .query_row(
                "SELECT conversation_id FROM messages WHERE id = ?1",
                params![&message.id],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        if existing_conversation_id
            .as_deref()
            .is_some_and(|existing| existing != conversation.id.as_str())
        {
            return Err(rusqlite::Error::InvalidQuery);
        }
        let affected = transaction.execute(
            "
            INSERT INTO messages (
                id,
                conversation_id,
                role,
                content,
                status,
                agent_run_json,
                ui_state_json,
                created_at,
                position
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
            ON CONFLICT(id) DO UPDATE SET
                role = excluded.role,
                content = excluded.content,
                status = excluded.status,
                agent_run_json = excluded.agent_run_json,
                ui_state_json = excluded.ui_state_json,
                created_at = excluded.created_at,
                position = excluded.position
            WHERE messages.conversation_id = excluded.conversation_id
            ",
            params![
                &message.id,
                &conversation.id,
                &message.role,
                &message.content,
                &message.status,
                &message.agent_run_json,
                &message.ui_state_json,
                message.created_at,
                index as i64
            ],
        )?;
        if affected != 1 {
            return Err(rusqlite::Error::InvalidQuery);
        }
        if message.role != "assistant" {
            transaction.execute(
                "DELETE FROM conversation_turn_traces WHERE assistant_message_id = ?1",
                params![&message.id],
            )?;
        }
    }

    let retained_message_ids = conversation
        .messages
        .iter()
        .map(|message| message.id.as_str())
        .collect::<HashSet<_>>();
    let existing_message_ids = {
        let mut statement =
            transaction.prepare("SELECT id FROM messages WHERE conversation_id = ?1")?;
        let message_ids = statement
            .query_map(params![&conversation.id], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        message_ids
    };
    let removed_message_ids = existing_message_ids
        .into_iter()
        .filter(|message_id| !retained_message_ids.contains(message_id.as_str()))
        .collect::<Vec<_>>();
    let compaction_rewind =
        context_compaction_repository::prepare_message_deletion_compaction_rewind(
            &transaction,
            &conversation.id,
            &removed_message_ids,
        )
        .map_err(context_compaction_error_to_sqlite)?;
    for message_id in &removed_message_ids {
        transaction.execute(
            "DELETE FROM messages WHERE conversation_id = ?1 AND id = ?2",
            params![&conversation.id, message_id],
        )?;
    }
    context_compaction_repository::finish_message_deletion_compaction_rewind(
        &transaction,
        compaction_rewind,
        now_ms(),
    )
    .map_err(context_compaction_error_to_sqlite)?;

    transaction.commit()
}

pub fn save_conversation_meta(
    connection: &Connection,
    conversation: &ChatConversationMetaRecord,
) -> rusqlite::Result<()> {
    connection.execute(
        "
        INSERT INTO conversations (
            id,
            project_id,
            model_id,
            title,
            created_at,
            updated_at,
            pinned_at,
            archived_at,
            unread_at
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
        ON CONFLICT(id) DO UPDATE SET
            project_id = excluded.project_id,
            model_id = excluded.model_id,
            title = excluded.title,
            updated_at = excluded.updated_at,
            pinned_at = excluded.pinned_at,
            archived_at = excluded.archived_at,
            unread_at = excluded.unread_at
        ",
        params![
            &conversation.id,
            &conversation.project_id,
            &conversation.model_id,
            &conversation.title,
            conversation.created_at,
            conversation.updated_at,
            conversation.pinned_at,
            conversation.archived_at,
            conversation.unread_at
        ],
    )?;
    Ok(())
}

pub fn upsert_messages(
    connection: &mut Connection,
    conversation_id: &str,
    messages: &[ChatMessageRecord],
    position_offset: i64,
) -> rusqlite::Result<()> {
    let transaction = connection.transaction()?;

    for (index, message) in messages.iter().enumerate() {
        transaction.execute(
            "
            INSERT INTO messages (
                id,
                conversation_id,
                role,
                content,
                status,
                agent_run_json,
                ui_state_json,
                created_at,
                position
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
            ON CONFLICT(id) DO UPDATE SET
                role = excluded.role,
                content = excluded.content,
                status = excluded.status,
                agent_run_json = excluded.agent_run_json,
                ui_state_json = excluded.ui_state_json,
                created_at = excluded.created_at,
                position = excluded.position
            ",
            params![
                &message.id,
                conversation_id,
                &message.role,
                &message.content,
                &message.status,
                &message.agent_run_json,
                &message.ui_state_json,
                message.created_at,
                position_offset + index as i64
            ],
        )?;
    }

    transaction.commit()
}

pub fn delete_messages(
    connection: &mut Connection,
    conversation_id: &str,
    message_ids: &[String],
) -> rusqlite::Result<()> {
    let transaction = connection.transaction()?;
    let compaction_rewind =
        context_compaction_repository::prepare_message_deletion_compaction_rewind(
            &transaction,
            conversation_id,
            message_ids,
        )
        .map_err(context_compaction_error_to_sqlite)?;

    for message_id in message_ids {
        transaction.execute(
            "DELETE FROM messages WHERE conversation_id = ?1 AND id = ?2",
            params![conversation_id, message_id],
        )?;
    }

    let ordered_message_ids = {
        let mut statement = transaction.prepare(
            "
            SELECT id
            FROM messages
            WHERE conversation_id = ?1
            ORDER BY position ASC, created_at ASC
            ",
        )?;

        let ordered_ids = statement
            .query_map(params![conversation_id], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        ordered_ids
    };

    for (position, message_id) in ordered_message_ids.iter().enumerate() {
        transaction.execute(
            "UPDATE messages SET position = ?1 WHERE conversation_id = ?2 AND id = ?3",
            params![position as i64, conversation_id, message_id],
        )?;
    }

    context_compaction_repository::finish_message_deletion_compaction_rewind(
        &transaction,
        compaction_rewind,
        now_ms(),
    )
    .map_err(context_compaction_error_to_sqlite)?;

    transaction.commit()
}

fn context_compaction_error_to_sqlite(
    error: context_compaction_repository::ContextCompactionRepositoryError,
) -> rusqlite::Error {
    match error {
        context_compaction_repository::ContextCompactionRepositoryError::Database(error) => error,
        error => rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            error.to_string(),
        ))),
    }
}

pub fn delete_conversation(connection: &Connection, conversation_id: &str) -> rusqlite::Result<()> {
    connection.execute(
        "DELETE FROM conversations WHERE id = ?1",
        params![conversation_id],
    )?;
    Ok(())
}

pub fn update_message_status_and_content(
    connection: &Connection,
    conversation_id: &str,
    message_id: &str,
    content: &str,
    status: Option<&str>,
    updated_at: i64,
) -> rusqlite::Result<()> {
    connection.execute(
        "
        UPDATE messages
        SET content = ?1, status = ?2
        WHERE conversation_id = ?3 AND id = ?4
        ",
        params![content, status, conversation_id, message_id],
    )?;
    connection.execute(
        "
        UPDATE conversations
        SET updated_at = ?1
        WHERE id = ?2
        ",
        params![updated_at, conversation_id],
    )?;
    Ok(())
}

pub fn update_message_run_terminal_state(
    connection: &Connection,
    conversation_id: &str,
    message_id: &str,
    message_status: Option<&str>,
    run_status: &str,
    completed_at: i64,
) -> rusqlite::Result<()> {
    let existing_agent_run_json = connection
        .query_row(
            "SELECT agent_run_json FROM messages WHERE conversation_id = ?1 AND id = ?2",
            params![conversation_id, message_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten();
    let next_agent_run_json = existing_agent_run_json.map(|raw| {
        let Ok(mut value) = serde_json::from_str::<serde_json::Value>(&raw) else {
            return raw;
        };
        let Some(run) = value.as_object_mut() else {
            return raw;
        };

        run.insert("status".to_string(), run_status.into());
        run.insert("completedAt".to_string(), completed_at.into());
        run.insert(
            "messageStreamCheckpoints".to_string(),
            serde_json::json!({}),
        );
        if let Some(state) = run
            .get_mut("state")
            .and_then(serde_json::Value::as_object_mut)
        {
            state.insert("status".to_string(), run_status.into());
            state.insert("activeRunId".to_string(), serde_json::Value::Null);
            state.insert("updatedAt".to_string(), completed_at.into());
        }

        serde_json::to_string(&value).unwrap_or(raw)
    });

    connection.execute(
        "
        UPDATE messages
        SET status = ?1, agent_run_json = ?2
        WHERE conversation_id = ?3 AND id = ?4
        ",
        params![
            message_status,
            next_agent_run_json,
            conversation_id,
            message_id
        ],
    )?;
    connection.execute(
        "UPDATE conversations SET updated_at = ?1 WHERE id = ?2",
        params![completed_at, conversation_id],
    )?;
    Ok(())
}

pub fn update_message_state(
    connection: &Connection,
    conversation_id: &str,
    message: &ChatMessageStateRecord,
) -> rusqlite::Result<()> {
    connection.execute(
        "
        UPDATE messages
        SET
            content = ?1,
            status = ?2,
            agent_run_json = ?3,
            ui_state_json = ?4
        WHERE conversation_id = ?5 AND id = ?6
        ",
        params![
            &message.content,
            &message.status,
            &message.agent_run_json,
            &message.ui_state_json,
            conversation_id,
            &message.id
        ],
    )?;
    Ok(())
}

fn list_messages(
    connection: &Connection,
    conversation_id: &str,
) -> rusqlite::Result<Vec<ChatMessageRecord>> {
    let mut statement = connection.prepare(
        "
        SELECT id, role, content, created_at, status, agent_run_json, ui_state_json
        FROM messages
        WHERE conversation_id = ?1
        ORDER BY position ASC, created_at ASC
        ",
    )?;

    let messages = statement
        .query_map(params![conversation_id], |row| {
            Ok(ChatMessageRecord {
                id: row.get(0)?,
                role: row.get(1)?,
                content: row.get(2)?,
                created_at: row.get(3)?,
                status: row.get(4)?,
                attachments: Vec::new(),
                agent_run_json: row.get(5)?,
                ui_state_json: row.get(6)?,
            })
        })?
        .collect();

    messages
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{conversation_trace_repository, migrations};
    use crate::{
        ConversationTurnTrace, ConversationTurnTraceItem, ConversationTurnTraceTerminalStatus,
        CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
    };

    #[test]
    fn full_conversation_save_preserves_retained_trace_and_deletes_missing_trace() {
        let mut connection = Connection::open_in_memory().unwrap();
        migrations::run_migrations(&connection).unwrap();
        let mut conversation = conversation();
        save_conversation(&mut connection, conversation.clone()).unwrap();

        let stored = get_conversation(&connection, "conversation-1")
            .unwrap()
            .unwrap();
        assert_eq!(stored.messages.len(), 2);
        assert!(get_conversation(&connection, "missing").unwrap().is_none());

        let trace = ConversationTurnTrace {
            schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
            run_id: "run-1".to_string(),
            conversation_id: conversation.id.clone(),
            assistant_message_id: "assistant-1".to_string(),
            terminal_status: ConversationTurnTraceTerminalStatus::Completed,
            terminal_error: None,
            truncated: false,
            items: vec![ConversationTurnTraceItem::AssistantNarration {
                sequence: 0,
                content: "Inspecting the workspace.".to_string(),
                truncated: false,
            }],
        };
        conversation_trace_repository::replace_trace(&mut connection, &trace, 10, 20).unwrap();

        conversation.title = "Updated title".to_string();
        conversation.messages[1].content = "Updated final answer".to_string();
        conversation
            .messages
            .push(message("user-2", "user", "follow-up", 3, Some("sent")));
        save_conversation(&mut connection, conversation.clone()).unwrap();

        assert_eq!(
            conversation_trace_repository::get_trace_for_message(&connection, "assistant-1")
                .unwrap(),
            Some(trace.clone())
        );
        let stored = list_conversations(&connection).unwrap();
        assert_eq!(stored[0].messages.len(), 3);
        assert_eq!(stored[0].messages[1].content, "Updated final answer");

        conversation.messages[1].role = "user".to_string();
        save_conversation(&mut connection, conversation.clone()).unwrap();
        assert!(
            conversation_trace_repository::get_trace_for_message(&connection, "assistant-1")
                .unwrap()
                .is_none()
        );
        conversation.messages[1].role = "assistant".to_string();
        save_conversation(&mut connection, conversation.clone()).unwrap();
        conversation_trace_repository::replace_trace(&mut connection, &trace, 10, 20).unwrap();

        conversation
            .messages
            .retain(|message| message.id != "assistant-1");
        save_conversation(&mut connection, conversation).unwrap();
        assert!(
            conversation_trace_repository::get_trace_for_message(&connection, "assistant-1")
                .unwrap()
                .is_none()
        );
    }

    fn conversation() -> ChatConversationRecord {
        ChatConversationRecord {
            id: "conversation-1".to_string(),
            project_id: None,
            model_id: Some("model-1".to_string()),
            title: "Conversation".to_string(),
            messages: vec![
                message("user-1", "user", "request", 1, Some("sent")),
                message("assistant-1", "assistant", "final answer", 2, Some("sent")),
            ],
            created_at: 1,
            updated_at: 2,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        }
    }

    fn message(
        id: &str,
        role: &str,
        content: &str,
        created_at: i64,
        status: Option<&str>,
    ) -> ChatMessageRecord {
        ChatMessageRecord {
            id: id.to_string(),
            role: role.to_string(),
            content: content.to_string(),
            created_at,
            status: status.map(ToString::to_string),
            attachments: Vec::new(),
            agent_run_json: None,
            ui_state_json: None,
        }
    }
}
