use crate::storage::models::{
    ChatConversationMetaRecord, ChatConversationRecord, ChatMessageRecord, ChatMessageStateRecord,
};
use crate::storage::{
    context_compaction_repository, now_ms, provider_continuation_repository, world_state_repository,
};
use crate::AgentMcpServerScope;
use rusqlite::{params, Connection, OptionalExtension, Row, Transaction};
use std::collections::HashSet;

pub(crate) struct McpInvocationTerminalProjection<'a> {
    pub action_id: &'a str,
    pub invocation_id: &'a str,
    pub call_id: &'a str,
    pub server_id: &'a str,
    pub server_display_name: &'a str,
    pub scope: &'a AgentMcpServerScope,
    pub raw_tool_name: &'a str,
    pub model_tool_name: &'a str,
    pub state: &'static str,
    pub dispatch_certainty: &'static str,
    pub outcome: &'static str,
    pub is_error: Option<bool>,
    pub error_code: &'static str,
}

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
    let conversation_metas = list_conversation_metas(connection)?;
    let mut conversations = conversation_metas
        .into_iter()
        .map(conversation_from_meta)
        .collect::<Vec<_>>();

    for conversation in &mut conversations {
        conversation.messages = list_messages(connection, &conversation.id)?;
    }

    Ok(conversations)
}

pub fn list_conversation_metas(
    connection: &Connection,
) -> rusqlite::Result<Vec<ChatConversationMetaRecord>> {
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

    let conversations = conversation_statement
        .query_map([], conversation_meta_from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
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
    Ok(conversation_from_meta(conversation_meta_from_row(row)?))
}

fn conversation_meta_from_row(row: &Row<'_>) -> rusqlite::Result<ChatConversationMetaRecord> {
    Ok(ChatConversationMetaRecord {
        id: row.get(0)?,
        project_id: row.get(1)?,
        model_id: row.get(2)?,
        title: row.get(3)?,
        created_at: row.get(4)?,
        updated_at: row.get(5)?,
        pinned_at: row.get(6)?,
        archived_at: row.get(7)?,
        unread_at: row.get(8)?,
    })
}

fn conversation_from_meta(meta: ChatConversationMetaRecord) -> ChatConversationRecord {
    ChatConversationRecord {
        id: meta.id,
        project_id: meta.project_id,
        model_id: meta.model_id,
        title: meta.title,
        messages: Vec::new(),
        created_at: meta.created_at,
        updated_at: meta.updated_at,
        pinned_at: meta.pinned_at,
        archived_at: meta.archived_at,
        unread_at: meta.unread_at,
    }
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
    world_state_repository::rewind_for_message_deletion(
        &transaction,
        &conversation.id,
        &removed_message_ids,
    )
    .map_err(world_state_error_to_sqlite)?;
    provider_continuation_repository::release_for_messages(
        &transaction,
        &conversation.id,
        &removed_message_ids,
        now_ms(),
    )?;
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
        WHERE excluded.updated_at > conversations.updated_at
           OR (
                excluded.updated_at = conversations.updated_at
                AND excluded.model_id IS conversations.model_id
           )
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
    delete_messages_in_transaction(&transaction, conversation_id, message_ids)?;
    transaction.commit()
}

/// Deletes messages and rewinds any derived compaction state using the caller's transaction.
///
/// Message-owned records that live outside the `messages` foreign-key graph must be retired by
/// the caller in the same transaction before invoking this helper. Keep [`delete_messages`] as the
/// standalone convenience API for repository callers that do not own such records.
pub(crate) fn delete_messages_in_transaction(
    connection: &Transaction<'_>,
    conversation_id: &str,
    message_ids: &[String],
) -> rusqlite::Result<()> {
    let compaction_rewind =
        context_compaction_repository::prepare_message_deletion_compaction_rewind(
            connection,
            conversation_id,
            message_ids,
        )
        .map_err(context_compaction_error_to_sqlite)?;
    world_state_repository::rewind_for_message_deletion(connection, conversation_id, message_ids)
        .map_err(world_state_error_to_sqlite)?;
    provider_continuation_repository::release_for_messages(
        connection,
        conversation_id,
        message_ids,
        now_ms(),
    )?;

    for message_id in message_ids {
        connection.execute(
            "DELETE FROM messages WHERE conversation_id = ?1 AND id = ?2",
            params![conversation_id, message_id],
        )?;
    }

    let ordered_message_ids = {
        let mut statement = connection.prepare(
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
        connection.execute(
            "UPDATE messages SET position = ?1 WHERE conversation_id = ?2 AND id = ?3",
            params![position as i64, conversation_id, message_id],
        )?;
    }

    context_compaction_repository::finish_message_deletion_compaction_rewind(
        connection,
        compaction_rewind,
        now_ms(),
    )
    .map_err(context_compaction_error_to_sqlite)?;
    Ok(())
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

fn world_state_error_to_sqlite(
    error: world_state_repository::ConversationWorldStateRepositoryError,
) -> rusqlite::Error {
    match error {
        world_state_repository::ConversationWorldStateRepositoryError::Database(error) => error,
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

/// Settles the Renderer-safe projection of one typed MCP invocation.
///
/// The projection is presentation state, but leaving it non-terminal after the owning run was
/// durably terminalized produces a permanent spinner after restart. Locate the Host-generated
/// invocation identity rather than a model-visible name, replace any drifted presentation copy,
/// and remove duplicates. If the Renderer never persisted the approval projection, append the
/// same bounded, secret-free identity available in the durable pending action so recovery still
/// explains why the external result is unknown.
pub(crate) fn update_message_mcp_invocation_terminal_state(
    connection: &Connection,
    conversation_id: &str,
    message_id: &str,
    projection: &McpInvocationTerminalProjection<'_>,
) -> rusqlite::Result<()> {
    let existing_agent_run_json = connection
        .query_row(
            "SELECT agent_run_json FROM messages WHERE conversation_id = ?1 AND id = ?2",
            params![conversation_id, message_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten();
    let Some(raw) = existing_agent_run_json else {
        return Ok(());
    };
    let Ok(mut value) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return Ok(());
    };
    let Some(run) = value.as_object_mut() else {
        return Ok(());
    };
    let scope = serde_json::to_value(projection.scope)
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
    let mut terminal = serde_json::json!({
        "actionId": projection.action_id,
        "invocationId": projection.invocation_id,
        "callId": projection.call_id,
        "serverId": projection.server_id,
        "serverDisplayName": projection.server_display_name,
        "scope": scope,
        "rawToolName": projection.raw_tool_name,
        "modelToolName": projection.model_tool_name,
        "external": true,
        "state": projection.state,
        "dispatchCertainty": projection.dispatch_certainty,
        "outcome": projection.outcome,
        "errorCode": projection.error_code,
        "outputTruncated": false,
    });
    if let Some(is_error) = projection.is_error {
        terminal["isError"] = is_error.into();
    }
    let invocations = run
        .entry("mcpInvocations".to_string())
        .or_insert_with(|| serde_json::json!([]));
    if !invocations.is_array() {
        *invocations = serde_json::json!([]);
    }
    let Some(invocations) = invocations.as_array_mut() else {
        return Err(rusqlite::Error::InvalidQuery);
    };

    // `invocationId` is the Host-generated one-time identity. Old projections may be missing a
    // display field or contain drifted presentation data, so requiring every field to match would
    // append a second record and leave the original `running` entry spinning forever. Replace the
    // first occurrence with the authoritative typed projection and remove every duplicate.
    let first_index = invocations.iter().position(|candidate| {
        candidate
            .get("invocationId")
            .and_then(serde_json::Value::as_str)
            == Some(projection.invocation_id)
    });
    invocations.retain(|candidate| {
        candidate
            .get("invocationId")
            .and_then(serde_json::Value::as_str)
            != Some(projection.invocation_id)
    });
    if let Some(index) = first_index {
        invocations.insert(index.min(invocations.len()), terminal);
    } else {
        invocations.push(terminal);
    }

    let next_agent_run_json = serde_json::to_string(&value)
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
    connection.execute(
        "UPDATE messages
         SET agent_run_json = ?1
         WHERE conversation_id = ?2 AND id = ?3",
        params![next_agent_run_json, conversation_id, message_id],
    )?;
    Ok(())
}

/// Repairs an assistant lifecycle after process restart using the backend-owned trace identity.
///
/// Unlike the normal terminal update, startup reconciliation must also canonicalize `runId`:
/// older renderer-owned shutdown paths could persist a terminal-looking message independently
/// from its still-running trace. A valid JSON object is preserved, while missing or malformed
/// presentation state is replaced with the smallest backend lifecycle record needed by storage
/// consumers. Timeline fields remain presentation-only and are never consulted by reconciliation.
pub(crate) fn reconcile_message_run_terminal_state(
    connection: &Connection,
    conversation_id: &str,
    message_id: &str,
    run_id: &str,
    message_status: &str,
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
    let mut run = existing_agent_run_json
        .as_deref()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();

    run.insert("runId".to_string(), run_id.into());
    run.insert("assistantMessageId".to_string(), message_id.into());
    run.insert("status".to_string(), run_status.into());
    run.insert("completedAt".to_string(), completed_at.into());
    run.insert(
        "messageStreamCheckpoints".to_string(),
        serde_json::json!({}),
    );
    let state = run
        .entry("state".to_string())
        .or_insert_with(|| serde_json::json!({}));
    if !state.is_object() {
        *state = serde_json::json!({});
    }
    let state = state
        .as_object_mut()
        .expect("startup reconciliation installs an object state");
    state.insert("status".to_string(), run_status.into());
    state.insert("activeRunId".to_string(), serde_json::Value::Null);
    state.insert("updatedAt".to_string(), completed_at.into());
    let run_json = serde_json::to_string(&serde_json::Value::Object(run))
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;

    connection.execute(
        "UPDATE messages
         SET status = ?1, agent_run_json = ?2
         WHERE conversation_id = ?3 AND id = ?4",
        params![message_status, run_json, conversation_id, message_id],
    )?;
    // Startup maintenance is not new user activity. Do not move an old repaired conversation to
    // the top of the sidebar by rewriting `conversations.updated_at`.
    Ok(())
}

/// Repairs a non-terminal run whose next approval checkpoint is already durable.
///
/// The pending-action row remains the authority for the approval payload. This function only
/// restores the message/run lifecycle fields and deliberately preserves every existing timeline,
/// tool, and approval field in `agent_run_json`.
pub fn update_message_run_waiting_state(
    connection: &Connection,
    conversation_id: &str,
    message_id: &str,
    run_id: &str,
    updated_at: i64,
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

        run.insert("runId".to_string(), run_id.into());
        run.insert("status".to_string(), "waiting_for_approval".into());
        run.remove("completedAt");
        if let Some(state) = run
            .get_mut("state")
            .and_then(serde_json::Value::as_object_mut)
        {
            state.insert("status".to_string(), "waiting_for_approval".into());
            state.insert("activeRunId".to_string(), run_id.into());
            state.insert("updatedAt".to_string(), updated_at.into());
        }

        serde_json::to_string(&value).unwrap_or(raw)
    });

    connection.execute(
        "UPDATE messages
         SET status = 'pending', agent_run_json = ?1
         WHERE conversation_id = ?2 AND id = ?3",
        params![next_agent_run_json, conversation_id, message_id],
    )?;
    connection.execute(
        "UPDATE conversations SET updated_at = ?1 WHERE id = ?2",
        params![updated_at, conversation_id],
    )?;
    Ok(())
}

pub fn update_message_state(
    connection: &Connection,
    conversation_id: &str,
    message: &ChatMessageStateRecord,
) -> rusqlite::Result<()> {
    let authoritative_usage =
        get_authoritative_message_usage(connection, conversation_id, &message.id)?;
    let agent_run_json =
        overlay_authoritative_usage(message.agent_run_json.clone(), authoritative_usage.as_ref());
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
            &agent_run_json,
            &message.ui_state_json,
            conversation_id,
            &message.id
        ],
    )?;
    Ok(())
}

pub fn update_message_ui_state(
    connection: &Connection,
    conversation_id: &str,
    message_id: &str,
    ui_state_json: Option<&str>,
) -> rusqlite::Result<()> {
    connection.execute(
        "
        UPDATE messages
        SET ui_state_json = ?1
        WHERE conversation_id = ?2 AND id = ?3
        ",
        params![ui_state_json, conversation_id, message_id],
    )?;
    Ok(())
}

fn list_messages(
    connection: &Connection,
    conversation_id: &str,
) -> rusqlite::Result<Vec<ChatMessageRecord>> {
    let mut statement = connection.prepare(
        "
        SELECT
            message.id,
            message.role,
            message.content,
            message.created_at,
            message.status,
            message.agent_run_json,
            message.ui_state_json,
            usage.run_id,
            usage.input_tokens,
            usage.output_tokens,
            usage.output_thinking_tokens,
            usage.total_tokens,
            usage.cached_input_tokens,
            usage.cache_creation_input_tokens,
            usage.billable_request_count
        FROM messages AS message
        LEFT JOIN agent_usage_records AS usage
          ON usage.conversation_id = message.conversation_id
         AND usage.message_id = message.id
        WHERE message.conversation_id = ?1
        ORDER BY message.position ASC, message.created_at ASC
        ",
    )?;

    let messages = statement
        .query_map(params![conversation_id], |row| {
            let authoritative_usage = match row.get::<_, Option<String>>(7)? {
                Some(run_id) => Some(AuthoritativeMessageUsage {
                    run_id,
                    input_tokens: row.get(8)?,
                    output_tokens: row.get(9)?,
                    output_thinking_tokens: row.get(10)?,
                    total_tokens: row.get(11)?,
                    cached_input_tokens: row.get(12)?,
                    cache_creation_input_tokens: row.get(13)?,
                    billable_request_count: row.get::<_, Option<i64>>(14)?.unwrap_or_default(),
                }),
                None => None,
            };
            Ok(ChatMessageRecord {
                id: row.get(0)?,
                role: row.get(1)?,
                content: row.get(2)?,
                created_at: row.get(3)?,
                status: row.get(4)?,
                attachments: Vec::new(),
                agent_run_json: overlay_authoritative_usage(
                    row.get(5)?,
                    authoritative_usage.as_ref(),
                ),
                ui_state_json: row.get(6)?,
            })
        })?
        .collect();

    messages
}

#[derive(Debug, Clone)]
struct AuthoritativeMessageUsage {
    run_id: String,
    input_tokens: Option<i64>,
    output_tokens: Option<i64>,
    output_thinking_tokens: Option<i64>,
    total_tokens: Option<i64>,
    cached_input_tokens: Option<i64>,
    cache_creation_input_tokens: Option<i64>,
    billable_request_count: i64,
}

fn get_authoritative_message_usage(
    connection: &Connection,
    conversation_id: &str,
    message_id: &str,
) -> rusqlite::Result<Option<AuthoritativeMessageUsage>> {
    connection
        .query_row(
            "SELECT
                run_id,
                input_tokens,
                output_tokens,
                output_thinking_tokens,
                total_tokens,
                cached_input_tokens,
                cache_creation_input_tokens,
                billable_request_count
             FROM agent_usage_records
             WHERE conversation_id = ?1 AND message_id = ?2",
            params![conversation_id, message_id],
            |row| {
                Ok(AuthoritativeMessageUsage {
                    run_id: row.get(0)?,
                    input_tokens: row.get(1)?,
                    output_tokens: row.get(2)?,
                    output_thinking_tokens: row.get(3)?,
                    total_tokens: row.get(4)?,
                    cached_input_tokens: row.get(5)?,
                    cache_creation_input_tokens: row.get(6)?,
                    billable_request_count: row.get(7)?,
                })
            },
        )
        .optional()
}

fn overlay_authoritative_usage(
    agent_run_json: Option<String>,
    usage: Option<&AuthoritativeMessageUsage>,
) -> Option<String> {
    let raw = agent_run_json?;
    let Some(usage) = usage else {
        return Some(raw);
    };
    let Ok(mut value) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return Some(raw);
    };
    let Some(run) = value.as_object_mut() else {
        return Some(raw);
    };
    if run
        .get("runId")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|run_id| run_id != usage.run_id)
    {
        return Some(raw);
    }

    let mut projected = serde_json::Map::new();
    insert_usage_token(&mut projected, "inputTokens", usage.input_tokens);
    insert_usage_token(&mut projected, "outputTokens", usage.output_tokens);
    insert_usage_token(
        &mut projected,
        "outputThinkingTokens",
        usage.output_thinking_tokens,
    );
    insert_usage_token(&mut projected, "totalTokens", usage.total_tokens);
    insert_usage_token(
        &mut projected,
        "cachedInputTokens",
        usage.cached_input_tokens,
    );
    insert_usage_token(
        &mut projected,
        "cacheCreationInputTokens",
        usage.cache_creation_input_tokens,
    );
    projected.insert(
        "billableRequestCount".to_string(),
        usage.billable_request_count.max(0).into(),
    );
    run.insert("usage".to_string(), projected.into());

    serde_json::to_string(&value).ok().or(Some(raw))
}

fn insert_usage_token(
    usage: &mut serde_json::Map<String, serde_json::Value>,
    key: &str,
    value: Option<i64>,
) {
    if let Some(value) = value {
        usage.insert(key.to_string(), value.max(0).into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::models::AgentUsageRecordInsert;
    use crate::storage::{conversation_trace_repository, migrations, usage_repository};
    use crate::{
        ConversationTurnTrace, ConversationTurnTraceItem, ConversationTurnTraceTerminalStatus,
        CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
    };

    #[test]
    fn conversation_metadata_listing_does_not_require_message_hydration() {
        let mut connection = Connection::open_in_memory().unwrap();
        migrations::run_migrations(&connection).unwrap();
        let conversation = conversation();
        save_conversation(&mut connection, conversation.clone()).unwrap();

        let metas = list_conversation_metas(&connection).unwrap();

        assert_eq!(metas.len(), 1);
        assert_eq!(metas[0].id, conversation.id);
        assert_eq!(metas[0].title, conversation.title);
    }

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

    #[test]
    fn caller_owned_message_deletion_can_be_rolled_back_as_one_transaction() {
        let mut connection = Connection::open_in_memory().unwrap();
        migrations::run_migrations(&connection).unwrap();
        save_conversation(&mut connection, conversation()).unwrap();

        let transaction = connection.transaction().unwrap();
        delete_messages_in_transaction(
            &transaction,
            "conversation-1",
            &["assistant-1".to_string()],
        )
        .unwrap();
        assert_eq!(
            get_conversation(&transaction, "conversation-1")
                .unwrap()
                .unwrap()
                .messages
                .len(),
            1
        );
        transaction.rollback().unwrap();

        let stored = get_conversation(&connection, "conversation-1")
            .unwrap()
            .unwrap();
        assert_eq!(stored.messages.len(), 2);
        assert_eq!(stored.messages[1].id, "assistant-1");
        assert_eq!(stored.messages[1].role, "assistant");
    }

    #[test]
    fn authoritative_usage_repairs_loaded_json_and_rejects_stale_renderer_usage() {
        let mut connection = Connection::open_in_memory().unwrap();
        migrations::run_migrations(&connection).unwrap();
        let mut stored = conversation();
        stored.messages[1].agent_run_json = Some(
            serde_json::json!({
                "runId": "run-1",
                "status": "completed",
                "usage": {
                    "inputTokens": 92_510,
                    "outputTokens": 391,
                    "totalTokens": 92_901,
                    "billableRequestCount": 1
                },
                "timeline": [{ "id": "presentation-only" }]
            })
            .to_string(),
        );
        save_conversation(&mut connection, stored).unwrap();
        usage_repository::upsert_usage_record(
            &connection,
            &AgentUsageRecordInsert {
                id: "usage-run-1".to_string(),
                conversation_id: "conversation-1".to_string(),
                message_id: "assistant-1".to_string(),
                run_id: "run-1".to_string(),
                project_id: None,
                model_id: "model-1".to_string(),
                model_name: "Model 1".to_string(),
                started_at: Some(1),
                completed_at: Some(2),
                status: Some("completed".to_string()),
                error: None,
                created_at: 2,
                input_tokens: Some(2_942_988),
                output_tokens: Some(38_253),
                output_thinking_tokens: Some(32_402),
                total_tokens: Some(2_981_241),
                cached_input_tokens: None,
                cache_creation_input_tokens: None,
                billable_request_count: 42,
                input_price: None,
                output_price: None,
                estimated_cost: None,
            },
        )
        .unwrap();

        let loaded = get_conversation(&connection, "conversation-1")
            .unwrap()
            .unwrap();
        let run: serde_json::Value =
            serde_json::from_str(loaded.messages[1].agent_run_json.as_deref().unwrap()).unwrap();
        assert_eq!(run["usage"]["inputTokens"], 2_942_988);
        assert_eq!(run["usage"]["outputTokens"], 38_253);
        assert_eq!(run["usage"]["outputThinkingTokens"], 32_402);
        assert_eq!(run["usage"]["totalTokens"], 2_981_241);
        assert_eq!(run["usage"]["billableRequestCount"], 42);
        assert_eq!(run["timeline"][0]["id"], "presentation-only");

        update_message_state(
            &connection,
            "conversation-1",
            &ChatMessageStateRecord {
                id: "assistant-1".to_string(),
                content: "final answer".to_string(),
                status: Some("sent".to_string()),
                agent_run_json: Some(
                    serde_json::json!({
                        "runId": "run-1",
                        "status": "completed",
                        "usage": {
                            "inputTokens": 92_510,
                            "outputTokens": 391,
                            "totalTokens": 92_901,
                            "billableRequestCount": 1
                        },
                        "timeline": [{ "id": "still-preserved" }]
                    })
                    .to_string(),
                ),
                ui_state_json: None,
            },
        )
        .unwrap();
        let raw: String = connection
            .query_row(
                "SELECT agent_run_json FROM messages WHERE id = 'assistant-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let run: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(run["usage"]["inputTokens"], 2_942_988);
        assert_eq!(run["usage"]["outputTokens"], 38_253);
        assert_eq!(run["usage"]["billableRequestCount"], 42);
        assert_eq!(run["timeline"][0]["id"], "still-preserved");
    }

    #[test]
    fn ui_state_update_never_rewrites_canonical_message_or_agent_run() {
        let mut connection = Connection::open_in_memory().unwrap();
        migrations::run_migrations(&connection).unwrap();
        let mut stored = conversation();
        stored.messages[1].content = "canonical final answer".to_string();
        stored.messages[1].status = Some("sent".to_string());
        stored.messages[1].agent_run_json = Some(
            serde_json::json!({
                "runId": "run-1",
                "status": "completed",
                "timeline": [{ "id": "terminal-answer", "type": "message" }]
            })
            .to_string(),
        );
        save_conversation(&mut connection, stored).unwrap();

        update_message_ui_state(
            &connection,
            "conversation-1",
            "assistant-1",
            Some(r#"{"timelineCollapsed":false}"#),
        )
        .unwrap();

        let (content, status, agent_run_json, ui_state_json): (
            String,
            Option<String>,
            Option<String>,
            Option<String>,
        ) = connection
            .query_row(
                "SELECT content, status, agent_run_json, ui_state_json
                 FROM messages WHERE id = 'assistant-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(content, "canonical final answer");
        assert_eq!(status.as_deref(), Some("sent"));
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(agent_run_json.as_deref().unwrap()).unwrap()
                ["timeline"][0]["id"],
            "terminal-answer"
        );
        assert_eq!(
            ui_state_json.as_deref(),
            Some(r#"{"timelineCollapsed":false}"#)
        );
    }

    #[test]
    fn normal_terminal_update_refreshes_conversation_but_startup_repair_does_not() {
        let mut connection = Connection::open_in_memory().unwrap();
        migrations::run_migrations(&connection).unwrap();
        let mut stored = conversation();
        stored.messages[1].agent_run_json = Some(
            serde_json::json!({
                "runId": "run-1",
                "status": "running",
                "state": { "status": "running", "activeRunId": "run-1" }
            })
            .to_string(),
        );
        save_conversation(&mut connection, stored).unwrap();

        update_message_run_terminal_state(
            &connection,
            "conversation-1",
            "assistant-1",
            Some("sent"),
            "completed",
            50,
        )
        .unwrap();
        let updated_at: i64 = connection
            .query_row(
                "SELECT updated_at FROM conversations WHERE id = 'conversation-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(updated_at, 50);

        reconcile_message_run_terminal_state(
            &connection,
            "conversation-1",
            "assistant-1",
            "run-1",
            "error",
            "failed",
            100,
        )
        .unwrap();
        let updated_at: i64 = connection
            .query_row(
                "SELECT updated_at FROM conversations WHERE id = 'conversation-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(updated_at, 50);
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
