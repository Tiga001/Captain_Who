// Rust core storage.
use crate::storage::models::{
    ChatConversationMetaRecord, ChatConversationRecord, ChatMessageRecord, ChatMessageStateRecord,
};
use rusqlite::{params, Connection, OptionalExtension};

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
        .query_map([], |row| {
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
        })?
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
    let conversation = connection
        .query_row(
            "
            SELECT id, project_id, model_id, title, created_at, updated_at, pinned_at, archived_at, unread_at
            FROM conversations
            WHERE id = ?1
            ",
            params![conversation_id],
            |row| {
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
            },
        )
        .optional()?;

    let Some(mut conversation) = conversation else {
        return Ok(None);
    };

    conversation.messages = list_messages(connection, conversation_id)?;
    Ok(Some(conversation))
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

    transaction.execute(
        "DELETE FROM messages WHERE conversation_id = ?1",
        params![&conversation.id],
    )?;

    for (index, message) in conversation.messages.iter().enumerate() {
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
    }

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

    transaction.commit()
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

pub fn update_message_status_content_and_agent_run(
    connection: &Connection,
    conversation_id: &str,
    message_id: &str,
    content: &str,
    status: Option<&str>,
    agent_run_json: Option<&str>,
    updated_at: i64,
) -> rusqlite::Result<()> {
    connection.execute(
        "
        UPDATE messages
        SET content = ?1, status = ?2, agent_run_json = ?3
        WHERE conversation_id = ?4 AND id = ?5
        ",
        params![content, status, agent_run_json, conversation_id, message_id],
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
