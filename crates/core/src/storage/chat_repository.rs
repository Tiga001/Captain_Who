use crate::storage::models::{
    ChatConversationMetaRecord, ChatConversationRecord, ChatMessageRecord, ChatMessageStateRecord,
};
use crate::storage::{
    context_compaction_repository, now_ms, provider_continuation_repository, world_state_repository,
};
use crate::{AgentMcpServerScope, AgentMcpToolApproval, AgentMcpToolInvocationEvent};
use rusqlite::{params, Connection, OptionalExtension, Row, Transaction};
use std::collections::HashSet;

/// Persists one Renderer-safe MCP lifecycle card from the Host-authenticated frozen approval.
///
/// Raw arguments, result content and diagnostics never enter this projection. The helper may
/// create the canonical AgentRun presentation skeleton, but it cannot change the durable
/// conversation trace or invent an invocation identity.
pub(crate) fn upsert_message_mcp_invocation_event(
    connection: &Connection,
    conversation_id: &str,
    message_id: &str,
    approval: &AgentMcpToolApproval,
    invocation: &AgentMcpToolInvocationEvent,
    updated_at: i64,
) -> rusqlite::Result<()> {
    let identity = &approval.identity;
    if invocation.action_id != identity.action_id
        || invocation.invocation_id != identity.invocation_id
        || invocation.call_id != identity.call_id
        || invocation.server_id != identity.provenance.server_id
        || invocation.server_display_name != approval.summary.server_display_name
        || invocation.raw_tool_name != identity.provenance.raw_tool_name
        || invocation.model_tool_name != identity.provenance.model_tool_name
        || !invocation.external
    {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let Some((existing_agent_run_json, started_at)) = connection
        .query_row(
            "SELECT agent_run_json, created_at
             FROM messages WHERE conversation_id = ?1 AND id = ?2",
            params![conversation_id, message_id],
            |row| Ok((row.get::<_, Option<String>>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()?
    else {
        return Err(rusqlite::Error::InvalidQuery);
    };
    let preserved_terminal = existing_agent_run_json
        .as_deref()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
        .and_then(|value| value.as_object().cloned())
        .filter(|run| current_agent_run_projection_is_safe(run, &identity.run_id))
        .and_then(|run| {
            let status = run.get("status")?.as_str()?;
            matches!(status, "completed" | "failed" | "cancelled").then(|| {
                (
                    status.to_string(),
                    run.get("completedAt")
                        .and_then(serde_json::Value::as_i64)
                        .unwrap_or(updated_at),
                )
            })
        });
    let (run_status, completed_at) = preserved_terminal
        .as_ref()
        .map(|(status, completed_at)| (status.as_str(), Some(*completed_at)))
        .unwrap_or(("running", None));
    let canonical = canonical_agent_run_lifecycle_projection(
        existing_agent_run_json.as_deref(),
        &identity.run_id,
        run_status,
        started_at,
        updated_at,
        completed_at,
    )?;
    let mut value = serde_json::from_str::<serde_json::Value>(&canonical)
        .map_err(|_| rusqlite::Error::InvalidQuery)?;
    let run = value.as_object_mut().ok_or(rusqlite::Error::InvalidQuery)?;
    let mut projected = serde_json::json!({
        "actionId": invocation.action_id,
        "invocationId": invocation.invocation_id,
        "callId": invocation.call_id,
        "serverId": invocation.server_id,
        "serverDisplayName": invocation.server_display_name,
        "scope": approval.summary.scope,
        "rawToolName": invocation.raw_tool_name,
        "modelToolName": invocation.model_tool_name,
        "external": true,
        "state": invocation.state,
        "dispatchCertainty": invocation.dispatch_certainty,
        "outputTruncated": invocation.output_truncated,
    });
    let projected = projected
        .as_object_mut()
        .expect("MCP invocation projection is an object");
    for (field, value) in [
        (
            "displayReason",
            invocation
                .display_reason
                .as_ref()
                .map(|value| serde_json::Value::String(value.clone())),
        ),
        (
            "outcome",
            invocation
                .outcome
                .map(|value| serde_json::to_value(value).expect("serialize MCP outcome")),
        ),
        ("isError", invocation.is_error.map(serde_json::Value::Bool)),
        (
            "errorCode",
            invocation
                .error_code
                .as_ref()
                .map(|value| serde_json::Value::String(value.clone())),
        ),
        (
            "durationMs",
            invocation.duration_ms.map(serde_json::Value::from),
        ),
    ] {
        if let Some(value) = value {
            projected.insert(field.to_string(), value);
        }
    }
    let projected = serde_json::Value::Object(projected.clone());

    let invocations = run
        .get_mut("mcpInvocations")
        .and_then(serde_json::Value::as_array_mut)
        .ok_or(rusqlite::Error::InvalidQuery)?;
    let matching = invocations
        .iter()
        .enumerate()
        .filter(|(_, candidate)| {
            candidate
                .get("invocationId")
                .and_then(serde_json::Value::as_str)
                == Some(identity.invocation_id.as_str())
        })
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    if matching.len() > 1
        || invocations.iter().any(|candidate| {
            candidate
                .get("invocationId")
                .and_then(serde_json::Value::as_str)
                != Some(identity.invocation_id.as_str())
                && (candidate
                    .get("actionId")
                    .and_then(serde_json::Value::as_str)
                    == Some(identity.action_id.as_str())
                    || candidate.get("callId").and_then(serde_json::Value::as_str)
                        == Some(identity.call_id.as_str()))
        })
    {
        return Err(rusqlite::Error::InvalidQuery);
    }
    if let Some(index) = matching.first().copied() {
        let current = invocations[index]
            .as_object()
            .ok_or(rusqlite::Error::InvalidQuery)?;
        for field in [
            "actionId",
            "invocationId",
            "callId",
            "serverId",
            "serverDisplayName",
            "scope",
            "rawToolName",
            "modelToolName",
            "external",
        ] {
            if current.get(field) != projected.get(field) {
                return Err(rusqlite::Error::InvalidQuery);
            }
        }
        invocations[index] = projected;
    } else {
        invocations.push(projected);
    }

    if let Some(tool_calls) = run
        .get_mut("toolCalls")
        .and_then(serde_json::Value::as_array_mut)
    {
        tool_calls.retain(|call| {
            call.get("id").and_then(serde_json::Value::as_str) != Some(identity.call_id.as_str())
        });
    }
    if let Some(tool_results) = run
        .get_mut("toolResults")
        .and_then(serde_json::Value::as_array_mut)
    {
        tool_results.retain(|result| {
            result.get("callId").and_then(serde_json::Value::as_str)
                != Some(identity.call_id.as_str())
        });
    }
    let timeline = run
        .get_mut("timeline")
        .and_then(serde_json::Value::as_array_mut)
        .ok_or(rusqlite::Error::InvalidQuery)?;
    timeline.retain(|item| {
        !((item.get("type").and_then(serde_json::Value::as_str) == Some("tool_call")
            && item.get("callId").and_then(serde_json::Value::as_str)
                == Some(identity.call_id.as_str()))
            || (item.get("type").and_then(serde_json::Value::as_str) == Some("mcp_tool_call")
                && item.get("invocationId").and_then(serde_json::Value::as_str)
                    == Some(identity.invocation_id.as_str())))
    });
    timeline.push(serde_json::json!({
        "id": format!("mcp-invocation-{}", identity.invocation_id),
        "type": "mcp_tool_call",
        "invocationId": identity.invocation_id,
    }));
    if !current_agent_run_projection_is_safe(run, &identity.run_id) {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let next_agent_run_json = serde_json::to_string(&value)
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
    connection.execute(
        "UPDATE messages SET agent_run_json = ?1 WHERE conversation_id = ?2 AND id = ?3",
        params![next_agent_run_json, conversation_id, message_id],
    )?;
    Ok(())
}

struct StoredImmutableMessage {
    role: String,
    content: String,
    status: Option<String>,
    agent_run_json: Option<String>,
    ui_state_json: Option<String>,
    created_at: i64,
}

fn immutable_graph_message(
    connection: &Connection,
    conversation_id: &str,
    message_id: &str,
) -> rusqlite::Result<Option<StoredImmutableMessage>> {
    connection
        .query_row(
            "SELECT role, content, status, agent_run_json, ui_state_json, created_at
             FROM messages
             WHERE conversation_id = ?1 AND id = ?2
               AND input_origin_kind IN ('agent', 'snapshot')",
            params![conversation_id, message_id],
            |row| {
                Ok(StoredImmutableMessage {
                    role: row.get(0)?,
                    content: row.get(1)?,
                    status: row.get(2)?,
                    agent_run_json: row.get(3)?,
                    ui_state_json: row.get(4)?,
                    created_at: row.get(5)?,
                })
            },
        )
        .optional()
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

/// Renderer/model-facing Conversation list. Immutable source Turns retained by an edit receipt
/// remain queryable through the raw repository paths but are absent from this active projection.
pub fn list_active_conversations(
    connection: &Connection,
) -> rusqlite::Result<Vec<ChatConversationRecord>> {
    let mut conversations = list_conversations(connection)?;
    for conversation in &mut conversations {
        retain_active_messages(connection, conversation)?;
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
    get_conversation_with_messages(connection, conversation_id, list_messages)
}

pub fn get_active_conversation(
    connection: &Connection,
    conversation_id: &str,
) -> rusqlite::Result<Option<ChatConversationRecord>> {
    let mut conversation = get_conversation(connection, conversation_id)?;
    if let Some(conversation) = &mut conversation {
        retain_active_messages(connection, conversation)?;
    }
    Ok(conversation)
}

/// Reads exactly the Conversation/message fields stored in SQLite, without renderer-owned
/// overlays such as authoritative Usage. Write-side CAS/admission callers must use this form so
/// a derived presentation cannot be mistaken for an attempted rewrite of immutable history.
pub(crate) fn get_persisted_conversation(
    connection: &Connection,
    conversation_id: &str,
) -> rusqlite::Result<Option<ChatConversationRecord>> {
    get_conversation_with_messages(connection, conversation_id, list_persisted_messages)
}

pub(crate) fn get_active_persisted_conversation(
    connection: &Connection,
    conversation_id: &str,
) -> rusqlite::Result<Option<ChatConversationRecord>> {
    let mut conversation = get_persisted_conversation(connection, conversation_id)?;
    if let Some(conversation) = &mut conversation {
        retain_active_messages(connection, conversation)?;
    }
    Ok(conversation)
}

fn retain_active_messages(
    connection: &Connection,
    conversation: &mut ChatConversationRecord,
) -> rusqlite::Result<()> {
    let superseded = crate::storage::conversation_turn_rewrite_repository::superseded_message_ids(
        connection,
        &conversation.id,
    )?;
    if !superseded.is_empty() {
        conversation
            .messages
            .retain(|message| !superseded.contains(&message.id));
    }
    Ok(())
}

fn get_conversation_with_messages(
    connection: &Connection,
    conversation_id: &str,
    load_messages: fn(&Connection, &str) -> rusqlite::Result<Vec<ChatMessageRecord>>,
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
        conversation.messages = load_messages(connection, &conversation.id)?;
    }
    Ok(conversation)
}

/// Removes transport-only Agent input projections from the user-facing root chat surface.
///
/// The rows remain durable Conversation/Context facts and child observer snapshots keep their
/// authenticated origins. Only the interactive root presentation hides them, so a child result
/// can never reappear after reload as a forged human `role=user` bubble.
pub fn retain_user_facing_root_messages(
    connection: &Connection,
    conversation: &mut ChatConversationRecord,
) -> rusqlite::Result<()> {
    let mut statement = connection.prepare(
        "SELECT id FROM messages
         WHERE conversation_id = ?1
           AND (
               input_origin_kind = 'agent'
               OR (
                   input_origin_kind = 'snapshot'
                   AND snapshot_original_origin_kind = 'agent'
               )
           )",
    )?;
    let internal_ids = statement
        .query_map([conversation.id.as_str()], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<HashSet<_>>>()?;
    conversation
        .messages
        .retain(|message| !internal_ids.contains(&message.id));
    Ok(())
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
    save_conversation_in_connection(&transaction, &conversation)?;
    transaction.commit()
}

/// Applies the complete Conversation/message projection inside a caller-owned transaction.
/// Turn admission uses this together with its empty trace lease so cross-Host races cannot expose
/// one half of startup.
pub(crate) fn save_conversation_in_connection(
    connection: &Connection,
    conversation: &ChatConversationRecord,
) -> rusqlite::Result<()> {
    let agent_bound = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM agent_nodes WHERE conversation_id = ?1)",
        [&conversation.id],
        |row| row.get::<_, bool>(0),
    )?;
    let mut next_agent_position = connection.query_row(
        "SELECT COALESCE(MAX(position), -1) + 1 FROM messages WHERE conversation_id = ?1",
        [&conversation.id],
        |row| row.get::<_, i64>(0),
    )?;

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

    for (index, message) in conversation.messages.iter().enumerate() {
        let existing_identity = connection
            .query_row(
                "SELECT conversation_id, position FROM messages WHERE id = ?1",
                params![&message.id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
            )
            .optional()?;
        if existing_identity
            .as_ref()
            .is_some_and(|(existing, _)| existing != &conversation.id)
        {
            return Err(rusqlite::Error::InvalidQuery);
        }
        if let Some(existing) = immutable_graph_message(connection, &conversation.id, &message.id)?
        {
            if existing.role != message.role
                || existing.content != message.content
                || existing.status != message.status
                || existing.created_at != message.created_at
                || existing.agent_run_json != message.agent_run_json
                || existing.ui_state_json != message.ui_state_json
            {
                return Err(rusqlite::Error::InvalidQuery);
            }
            continue;
        }
        let position = if agent_bound {
            existing_identity
                .as_ref()
                .map(|(_, position)| *position)
                .unwrap_or_else(|| {
                    let position = next_agent_position;
                    next_agent_position = next_agent_position.saturating_add(1);
                    position
                })
        } else {
            index as i64
        };
        let affected = connection.execute(
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
                position
            ],
        )?;
        if affected != 1 {
            return Err(rusqlite::Error::InvalidQuery);
        }
        if message.role != "assistant" {
            connection.execute(
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
            connection.prepare("SELECT id FROM messages WHERE conversation_id = ?1")?;
        let message_ids = statement
            .query_map(params![&conversation.id], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        message_ids
    };
    let mut removed_message_ids = existing_message_ids
        .into_iter()
        .filter(|message_id| !retained_message_ids.contains(message_id.as_str()))
        .collect::<Vec<_>>();
    if agent_bound && !removed_message_ids.is_empty() {
        let mut retained_graph_projections = HashSet::new();
        for message_id in &removed_message_ids {
            let is_projection = connection.query_row(
                "SELECT COALESCE(input_origin_kind IN ('agent', 'snapshot'), 0) FROM messages
                 WHERE conversation_id = ?1 AND id = ?2",
                params![&conversation.id, message_id],
                |row| row.get::<_, bool>(0),
            )?;
            if is_projection {
                retained_graph_projections.insert(message_id.clone());
            }
        }
        removed_message_ids.retain(|id| !retained_graph_projections.contains(id));
    }
    if !removed_message_ids.is_empty() {
        let superseded =
            crate::storage::conversation_turn_rewrite_repository::superseded_message_ids(
                connection,
                &conversation.id,
            )?;
        removed_message_ids.retain(|id| !superseded.contains(id));
    }
    let compaction_rewind =
        context_compaction_repository::prepare_message_deletion_compaction_rewind(
            connection,
            &conversation.id,
            &removed_message_ids,
        )
        .map_err(context_compaction_error_to_sqlite)?;
    world_state_repository::rewind_for_message_deletion(
        connection,
        &conversation.id,
        &removed_message_ids,
    )
    .map_err(world_state_error_to_sqlite)?;
    provider_continuation_repository::release_for_messages(
        connection,
        &conversation.id,
        &removed_message_ids,
        now_ms(),
    )?;
    for message_id in &removed_message_ids {
        connection.execute(
            "DELETE FROM messages WHERE conversation_id = ?1 AND id = ?2",
            params![&conversation.id, message_id],
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
    let agent_bound = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM agent_nodes WHERE conversation_id = ?1)",
        [conversation_id],
        |row| row.get::<_, bool>(0),
    )?;
    let mut next_agent_position = transaction.query_row(
        "SELECT COALESCE(MAX(position), -1) + 1 FROM messages WHERE conversation_id = ?1",
        [conversation_id],
        |row| row.get::<_, i64>(0),
    )?;

    for (index, message) in messages.iter().enumerate() {
        if let Some(existing) = immutable_graph_message(&transaction, conversation_id, &message.id)?
        {
            if existing.role != message.role
                || existing.content != message.content
                || existing.status != message.status
                || existing.created_at != message.created_at
                || existing.agent_run_json != message.agent_run_json
                || existing.ui_state_json != message.ui_state_json
            {
                return Err(rusqlite::Error::InvalidQuery);
            }
            continue;
        }
        let existing_position = transaction
            .query_row(
                "SELECT position FROM messages WHERE conversation_id = ?1 AND id = ?2",
                params![conversation_id, &message.id],
                |row| row.get::<_, i64>(0),
            )
            .optional()?;
        let position = if agent_bound {
            existing_position.unwrap_or_else(|| {
                let position = next_agent_position;
                next_agent_position = next_agent_position.saturating_add(1);
                position
            })
        } else {
            position_offset + index as i64
        };
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
                position
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
    for message_id in message_ids {
        let is_agent_projection = connection.query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM messages
                 WHERE conversation_id = ?1
                   AND id = ?2
                   AND input_origin_kind IN ('agent', 'snapshot')
             )",
            params![conversation_id, message_id],
            |row| row.get::<_, bool>(0),
        )?;
        if is_agent_projection {
            return Err(rusqlite::Error::InvalidQuery);
        }
    }
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

    let agent_bound = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM agent_nodes WHERE conversation_id = ?1)",
        [conversation_id],
        |row| row.get::<_, bool>(0),
    )?;
    if !agent_bound {
        for (position, message_id) in ordered_message_ids.iter().enumerate() {
            connection.execute(
                "UPDATE messages SET position = ?1 WHERE conversation_id = ?2 AND id = ?3",
                params![position as i64, conversation_id, message_id],
            )?;
        }
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
    run_id: &str,
    message_status: Option<&str>,
    run_status: &str,
    completed_at: i64,
) -> rusqlite::Result<()> {
    let Some((existing_agent_run_json, started_at)) = connection
        .query_row(
            "SELECT agent_run_json, created_at FROM messages WHERE conversation_id = ?1 AND id = ?2",
            params![conversation_id, message_id],
            |row| Ok((row.get::<_, Option<String>>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()?
    else {
        return Ok(());
    };
    let next_agent_run_json = canonical_agent_run_lifecycle_projection(
        existing_agent_run_json.as_deref(),
        run_id,
        run_status,
        started_at,
        completed_at,
        Some(completed_at),
    )?;

    connection.execute(
        "
        UPDATE messages
        SET status = ?1, agent_run_json = ?2
        WHERE conversation_id = ?3 AND id = ?4
        ",
        params![
            message_status,
            Some(next_agent_run_json),
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

const CURRENT_AGENT_RUN_KEYS: &[&str] = &[
    "runId",
    "status",
    "startedAt",
    "firstResponseAt",
    "lastResponseAt",
    "completedAt",
    "toolDefinitions",
    "toolSetRevision",
    "todo",
    "toolCalls",
    "toolResults",
    "webSearchActivities",
    "readActivities",
    "approvals",
    "skillInstallations",
    "diffs",
    "fileDrafts",
    "commandSessions",
    "mcpInvocations",
    "messageStreamCheckpoints",
    "timeline",
    "state",
    "error",
    "usage",
    "finishReason",
    "activatedSkills",
    "skillActivationRevision",
    "explicitSkillSelections",
];

fn exact_keys(object: &serde_json::Map<String, serde_json::Value>, allowed: &[&str]) -> bool {
    object.len() == allowed.len() && object.keys().all(|key| allowed.contains(&key.as_str()))
}

fn only_allowed_keys(
    object: &serde_json::Map<String, serde_json::Value>,
    allowed: &[&str],
) -> bool {
    object.keys().all(|key| allowed.contains(&key.as_str()))
}

const MAX_CURRENT_RUN_ITEMS: usize = 10_000;
const MAX_JS_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

fn exact_required_optional_keys(
    object: &serde_json::Map<String, serde_json::Value>,
    required: &[&str],
    optional: &[&str],
) -> bool {
    required.iter().all(|key| object.contains_key(*key))
        && object
            .keys()
            .all(|key| required.contains(&key.as_str()) || optional.contains(&key.as_str()))
}

fn safe_integer(value: &serde_json::Value) -> bool {
    value
        .as_u64()
        .is_some_and(|value| value <= MAX_JS_SAFE_INTEGER)
}

fn bounded_string(value: &serde_json::Value, maximum: usize, allow_empty: bool) -> bool {
    value
        .as_str()
        .is_some_and(|value| (allow_empty || !value.is_empty()) && value.chars().count() <= maximum)
}

fn optional_bounded_string(
    object: &serde_json::Map<String, serde_json::Value>,
    key: &str,
    maximum: usize,
    allow_empty: bool,
) -> bool {
    object
        .get(key)
        .is_none_or(|value| bounded_string(value, maximum, allow_empty))
}

fn nullable_bounded_string(value: &serde_json::Value, maximum: usize, allow_empty: bool) -> bool {
    value.is_null() || bounded_string(value, maximum, allow_empty)
}

fn optional_safe_integer(object: &serde_json::Map<String, serde_json::Value>, key: &str) -> bool {
    object.get(key).is_none_or(safe_integer)
}

fn current_string_array_is_safe(value: &serde_json::Value, maximum: usize) -> bool {
    value.as_array().is_some_and(|values| {
        values.len() <= MAX_CURRENT_RUN_ITEMS
            && values
                .iter()
                .all(|value| bounded_string(value, maximum, true))
    })
}

fn record_array_is_safe(
    value: &serde_json::Value,
    validate: impl Fn(&serde_json::Map<String, serde_json::Value>) -> bool,
) -> bool {
    value.as_array().is_some_and(|values| {
        values.len() <= MAX_CURRENT_RUN_ITEMS
            && values
                .iter()
                .all(|value| value.as_object().is_some_and(&validate))
    })
}

fn current_tool_definition_is_safe(
    definition: &serde_json::Map<String, serde_json::Value>,
) -> bool {
    exact_required_optional_keys(
        definition,
        &[
            "name",
            "description",
            "inputSchema",
            "safety",
            "requiresWorkspace",
            "requiresApproval",
            "approvalMode",
        ],
        &[],
    ) && bounded_string(&definition["name"], 1_024, false)
        && bounded_string(&definition["description"], 128 * 1_024, true)
        && matches!(
            definition["safety"].as_str(),
            Some("read_only" | "requires_approval" | "destructive")
        )
        && definition["requiresWorkspace"].is_boolean()
        && definition["requiresApproval"].is_boolean()
        && matches!(
            definition["approvalMode"].as_str(),
            Some("never" | "always" | "dynamic")
        )
}

fn current_uuid_is_safe(value: &str) -> bool {
    let Ok(parsed) = uuid::Uuid::parse_str(value) else {
        return false;
    };
    parsed.hyphenated().to_string().eq_ignore_ascii_case(value)
        && matches!(parsed.get_version_num(), 1..=5)
        && parsed.get_variant() == uuid::Variant::RFC4122
}

fn current_tool_call_is_safe(call: &serde_json::Map<String, serde_json::Value>) -> bool {
    exact_keys(call, &["id", "tool", "args", "approvalStatus", "reason"])
        && bounded_string(&call["id"], 1_024, false)
        && bounded_string(&call["tool"], 1_024, false)
        && matches!(
            call["approvalStatus"].as_str(),
            Some("not_required" | "required" | "approved" | "rejected")
        )
        && (call["reason"].is_null() || bounded_string(&call["reason"], 4_096, true))
        && serde_json::from_value::<crate::AgentToolCall>(serde_json::Value::Object(call.clone()))
            .is_ok()
}

fn current_tool_result_is_safe(result: &serde_json::Map<String, serde_json::Value>) -> bool {
    exact_required_optional_keys(result, &["callId", "tool", "ok"], &["result", "error"])
        && bounded_string(&result["callId"], 1_024, false)
        && bounded_string(&result["tool"], 1_024, false)
        && result["ok"].is_boolean()
        && optional_bounded_string(result, "error", 128 * 1_024, true)
}

fn current_approval_status_is_safe(value: &serde_json::Value) -> bool {
    matches!(
        value.as_str(),
        Some("not_required" | "required" | "approved" | "rejected")
    )
}

fn current_command_observation_is_safe(value: &serde_json::Value) -> bool {
    if value.is_null() {
        return true;
    }
    let Some(observation) = value.as_object() else {
        return false;
    };
    exact_required_optional_keys(
        observation,
        &["kinds"],
        &["expectedOutputs", "additionalRoots"],
    ) && observation["kinds"].as_array().is_some_and(|kinds| {
        kinds.len() <= MAX_CURRENT_RUN_ITEMS && kinds.iter().all(|kind| kind == "office")
    }) && ["expectedOutputs", "additionalRoots"].iter().all(|field| {
        observation.get(*field).is_none_or(|values| {
            values.as_array().is_some_and(|values| {
                values.len() <= MAX_CURRENT_RUN_ITEMS
                    && values
                        .iter()
                        .all(|value| bounded_string(value, 16 * 1_024, true))
            })
        })
    })
}

fn current_command_approval_is_safe(value: &serde_json::Value) -> bool {
    let Some(command) = value.as_object() else {
        return false;
    };
    exact_keys(
        command,
        &[
            "id",
            "command",
            "cwd",
            "timeoutMs",
            "approvalStatus",
            "riskLevel",
            "reason",
            "observe",
        ],
    ) && bounded_string(&command["id"], 1_024, false)
        && bounded_string(&command["command"], 4 * 1_024 * 1_024, true)
        && (command["cwd"].is_null() || bounded_string(&command["cwd"], 16 * 1_024, true))
        && (command["timeoutMs"].is_null() || safe_integer(&command["timeoutMs"]))
        && current_approval_status_is_safe(&command["approvalStatus"])
        && (command["riskLevel"].is_null()
            || matches!(
                command["riskLevel"].as_str(),
                Some("read_only" | "writes_workspace" | "network" | "destructive" | "unknown")
            ))
        && (command["reason"].is_null() || bounded_string(&command["reason"], 16 * 1_024, true))
        && current_command_observation_is_safe(&command["observe"])
}

fn current_persisted_approval_is_safe(
    approval: &serde_json::Map<String, serde_json::Value>,
) -> bool {
    match approval.get("type").and_then(serde_json::Value::as_str) {
        Some("tool_call") => {
            exact_keys(approval, &["type", "call"])
                && approval["call"]
                    .as_object()
                    .is_some_and(current_tool_call_is_safe)
        }
        Some("diff") => {
            exact_keys(approval, &["type", "diff"])
                && approval["diff"]
                    .as_object()
                    .is_some_and(current_diff_is_safe)
        }
        Some("file_write") => {
            exact_keys(approval, &["type", "fileWrite"])
                && approval["fileWrite"]
                    .as_object()
                    .is_some_and(current_file_write_approval_is_safe)
        }
        Some("command") => {
            exact_keys(approval, &["type", "command"])
                && current_command_approval_is_safe(&approval["command"])
        }
        Some("skill_materialization") => {
            exact_keys(approval, &["type", "materialization"])
                && approval["materialization"]
                    .as_object()
                    .is_some_and(current_skill_materialization_is_safe)
        }
        Some("skill_script") => {
            exact_keys(approval, &["type", "script"])
                && approval["script"]
                    .as_object()
                    .is_some_and(current_skill_script_is_safe)
        }
        Some("mcp_tool_call") => false,
        Some("office_operation") => {
            exact_keys(approval, &["type", "officeOperation"])
                && approval["officeOperation"]
                    .as_object()
                    .is_some_and(current_office_operation_is_safe)
        }
        Some("skill_installation") => {
            if !exact_keys(approval, &["type", "installation"]) {
                return false;
            }
            let synthetic = serde_json::Map::from_iter([
                ("action".to_string(), approval["installation"].clone()),
                (
                    "status".to_string(),
                    serde_json::Value::String("waiting_for_approval".to_string()),
                ),
            ]);
            current_skill_installation_is_safe(&synthetic)
        }
        Some(_) | None => false,
    }
}

fn current_file_write_approval_is_safe(
    proposal: &serde_json::Map<String, serde_json::Value>,
) -> bool {
    exact_keys(
        proposal,
        &[
            "id",
            "draftId",
            "mode",
            "filePath",
            "baseRevision",
            "summary",
            "additions",
            "deletions",
            "lineCount",
            "byteCount",
            "approvalStatus",
        ],
    ) && bounded_string(&proposal["id"], 1_024, false)
        && bounded_string(&proposal["draftId"], 1_024, false)
        && matches!(
            proposal["mode"].as_str(),
            Some("create" | "rewrite" | "modify" | "append" | "upsert")
        )
        && bounded_string(&proposal["filePath"], 16 * 1_024, false)
        && nullable_bounded_string(&proposal["baseRevision"], 1_024, false)
        && nullable_bounded_string(&proposal["summary"], 16 * 1_024, true)
        && ["additions", "deletions", "lineCount", "byteCount"]
            .iter()
            .all(|field| safe_integer(&proposal[*field]))
        && current_approval_status_is_safe(&proposal["approvalStatus"])
        && serde_json::from_value::<crate::AgentFileWriteProposal>(serde_json::Value::Object(
            proposal.clone(),
        ))
        .is_ok()
}

fn current_skill_materialization_is_safe(
    materialization: &serde_json::Map<String, serde_json::Value>,
) -> bool {
    exact_keys(
        materialization,
        &[
            "id",
            "sourceUri",
            "sourcePrefix",
            "destination",
            "approvalStatus",
            "reason",
        ],
    ) && bounded_string(&materialization["id"], 1_024, false)
        && bounded_string(&materialization["sourceUri"], 16 * 1_024, false)
        && nullable_bounded_string(&materialization["sourcePrefix"], 16 * 1_024, true)
        && bounded_string(&materialization["destination"], 16 * 1_024, false)
        && current_approval_status_is_safe(&materialization["approvalStatus"])
        && nullable_bounded_string(&materialization["reason"], 16 * 1_024, true)
        && serde_json::from_value::<crate::AgentSkillMaterializationRequest>(
            serde_json::Value::Object(materialization.clone()),
        )
        .is_ok()
}

fn current_skill_script_requirements_are_safe(value: &serde_json::Value) -> bool {
    let Some(requirements) = value.as_object() else {
        return false;
    };
    exact_required_optional_keys(requirements, &[], &["pythonDistributions", "commands"])
        && requirements
            .get("pythonDistributions")
            .is_none_or(|value| current_string_array_is_safe(value, 1_024))
        && requirements
            .get("commands")
            .is_none_or(|value| current_string_array_is_safe(value, 1_024))
}

fn current_skill_script_preflight_is_safe(value: &serde_json::Value) -> bool {
    let Some(preflight) = value.as_object() else {
        return false;
    };
    exact_required_optional_keys(
        preflight,
        &["status", "interpreter", "runtimeFingerprint"],
        &["interpreterVersion", "dependencies", "errorCode", "message"],
    ) && matches!(
        preflight["status"].as_str(),
        Some("ready" | "missing_dependencies" | "unsupported" | "conflict")
    ) && preflight["interpreter"] == "python3"
        && bounded_string(&preflight["runtimeFingerprint"], 4_096, false)
        && optional_bounded_string(preflight, "interpreterVersion", 1_024, false)
        && preflight.get("dependencies").is_none_or(|dependencies| {
            record_array_is_safe(dependencies, |dependency| {
                exact_required_optional_keys(dependency, &["kind", "name", "status"], &["version"])
                    && matches!(
                        dependency["kind"].as_str(),
                        Some("python_distribution" | "command")
                    )
                    && bounded_string(&dependency["name"], 1_024, false)
                    && matches!(dependency["status"].as_str(), Some("available" | "missing"))
                    && optional_bounded_string(dependency, "version", 1_024, false)
            })
        })
        && optional_bounded_string(preflight, "errorCode", 1_024, false)
        && optional_bounded_string(preflight, "message", 128 * 1_024, true)
}

fn current_skill_script_is_safe(script: &serde_json::Map<String, serde_json::Value>) -> bool {
    exact_keys(
        script,
        &[
            "id",
            "scriptUri",
            "skillId",
            "skillRevision",
            "resourcePath",
            "resourceDigest",
            "interpreter",
            "args",
            "requirements",
            "preflight",
            "timeoutMs",
            "approvalStatus",
            "reason",
        ],
    ) && bounded_string(&script["id"], 1_024, false)
        && bounded_string(&script["scriptUri"], 16 * 1_024, false)
        && bounded_string(&script["skillId"], 1_024, false)
        && bounded_string(&script["skillRevision"], 1_024, false)
        && bounded_string(&script["resourcePath"], 16 * 1_024, false)
        && bounded_string(&script["resourceDigest"], 1_024, false)
        && script["interpreter"] == "python3"
        && current_string_array_is_safe(&script["args"], 64 * 1_024)
        && current_skill_script_requirements_are_safe(&script["requirements"])
        && current_skill_script_preflight_is_safe(&script["preflight"])
        && (script["timeoutMs"].is_null() || safe_integer(&script["timeoutMs"]))
        && current_approval_status_is_safe(&script["approvalStatus"])
        && nullable_bounded_string(&script["reason"], 16 * 1_024, true)
        && serde_json::from_value::<crate::AgentSkillScriptRequest>(serde_json::Value::Object(
            script.clone(),
        ))
        .is_ok()
}

fn current_file_input_ref_is_safe(value: &serde_json::Value) -> bool {
    let Some(reference) = value.as_object() else {
        return false;
    };
    match reference.get("type").and_then(serde_json::Value::as_str) {
        Some("attachment") => {
            exact_keys(reference, &["type", "readPath"])
                && bounded_string(&reference["readPath"], 16 * 1_024, false)
        }
        Some("workspace" | "external") => {
            exact_keys(reference, &["type", "path"])
                && bounded_string(&reference["path"], 16 * 1_024, false)
        }
        Some("generated_artifact") => {
            exact_keys(reference, &["type", "uri", "path"])
                && bounded_string(&reference["uri"], 16 * 1_024, false)
                && bounded_string(&reference["path"], 16 * 1_024, false)
        }
        Some("skill_resource") => {
            exact_keys(reference, &["type", "uri"])
                && bounded_string(&reference["uri"], 16 * 1_024, false)
        }
        Some(_) | None => false,
    }
}

fn current_file_input_spec_is_safe(value: &serde_json::Value) -> bool {
    let Some(input) = value.as_object() else {
        return false;
    };
    exact_keys(input, &["mountPath", "source"])
        && bounded_string(&input["mountPath"], 16 * 1_024, false)
        && current_file_input_ref_is_safe(&input["source"])
}

fn current_file_input_binding_is_safe(value: &serde_json::Value) -> bool {
    let Some(binding) = value.as_object() else {
        return false;
    };
    exact_keys(
        binding,
        &[
            "schemaVersion",
            "mountPath",
            "source",
            "sizeBytes",
            "sha256",
        ],
    ) && binding["schemaVersion"] == crate::AGENT_FILE_INPUT_BINDING_SCHEMA_VERSION
        && bounded_string(&binding["mountPath"], 16 * 1_024, false)
        && current_file_input_ref_is_safe(&binding["source"])
        && safe_integer(&binding["sizeBytes"])
        && binding["sha256"].as_str().is_some_and(|digest| {
            digest.len() == 64
                && digest
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
}

fn current_office_property_map_is_safe(value: &serde_json::Value) -> bool {
    value.as_object().is_some_and(|properties| {
        properties.values().all(|property| {
            property.is_string()
                || property.is_number()
                || property.is_boolean()
                || property.as_object().is_some_and(|resource| {
                    exact_keys(resource, &["resourcePath"])
                        && bounded_string(&resource["resourcePath"], 16 * 1_024, false)
                })
        })
    })
}

fn current_office_position_is_safe(value: &serde_json::Value) -> bool {
    let Some(position) = value.as_object() else {
        return false;
    };
    match position.get("type").and_then(serde_json::Value::as_str) {
        Some("index") => {
            exact_keys(position, &["type", "index"]) && safe_integer(&position["index"])
        }
        Some("after" | "before") => {
            exact_keys(position, &["type", "target"])
                && bounded_string(&position["target"], 16 * 1_024, false)
        }
        Some(_) | None => false,
    }
}

fn current_office_parameters_are_safe(value: &serde_json::Value) -> bool {
    let Some(parameters) = value.as_object() else {
        return false;
    };
    match parameters.get("type").and_then(serde_json::Value::as_str) {
        Some("help") => {
            exact_required_optional_keys(parameters, &["type"], &["verb", "element"])
                && parameters.get("verb").is_none_or(|value| {
                    matches!(
                        value.as_str(),
                        Some(
                            "status"
                                | "help"
                                | "create"
                                | "view"
                                | "get"
                                | "query"
                                | "validate"
                                | "set"
                                | "add"
                                | "remove"
                                | "move"
                                | "swap"
                        )
                    )
                })
                && optional_bounded_string(parameters, "element", 1_024, false)
        }
        Some("create") => {
            exact_required_optional_keys(parameters, &["type"], &["locale", "minimal", "overwrite"])
                && optional_bounded_string(parameters, "locale", 1_024, false)
                && parameters
                    .get("minimal")
                    .is_none_or(serde_json::Value::is_boolean)
                && parameters
                    .get("overwrite")
                    .is_none_or(serde_json::Value::is_boolean)
        }
        Some("view") => {
            exact_required_optional_keys(
                parameters,
                &["type", "mode"],
                &[
                    "start",
                    "end",
                    "maxLines",
                    "issueType",
                    "limit",
                    "columns",
                    "pages",
                    "range",
                    "viewport",
                    "grid",
                    "renderMode",
                    "pageCount",
                ],
            ) && matches!(
                parameters["mode"].as_str(),
                Some(
                    "text"
                        | "annotated"
                        | "outline"
                        | "stats"
                        | "issues"
                        | "html"
                        | "svg"
                        | "screenshot"
                        | "forms"
                )
            ) && ["start", "end", "maxLines", "limit"]
                .iter()
                .all(|field| optional_safe_integer(parameters, field))
                && optional_bounded_string(parameters, "issueType", 1_024, false)
                && parameters
                    .get("columns")
                    .is_none_or(|value| current_string_array_is_safe(value, 1_024))
                && parameters.get("pages").is_none_or(|pages| {
                    record_array_is_safe(pages, |page| {
                        exact_required_optional_keys(page, &["start"], &["end"])
                            && safe_integer(&page["start"])
                            && optional_safe_integer(page, "end")
                    })
                })
                && optional_bounded_string(parameters, "range", 16 * 1_024, false)
                && parameters.get("viewport").is_none_or(|viewport| {
                    viewport.as_object().is_some_and(|viewport| {
                        exact_keys(viewport, &["width", "height"])
                            && safe_integer(&viewport["width"])
                            && viewport["width"].as_u64() != Some(0)
                            && safe_integer(&viewport["height"])
                            && viewport["height"].as_u64() != Some(0)
                    })
                })
                && parameters.get("grid").is_none_or(|grid| {
                    grid.as_object().is_some_and(|grid| {
                        matches!(
                            grid.get("mode").and_then(serde_json::Value::as_str),
                            Some("auto")
                        ) && exact_keys(grid, &["mode"])
                            || matches!(
                                grid.get("mode").and_then(serde_json::Value::as_str),
                                Some("columns")
                            ) && exact_keys(grid, &["mode", "columns"])
                                && safe_integer(&grid["columns"])
                                && grid["columns"].as_u64() != Some(0)
                    })
                })
                && parameters
                    .get("renderMode")
                    .is_none_or(|value| matches!(value.as_str(), Some("auto" | "html")))
                && parameters
                    .get("pageCount")
                    .is_none_or(serde_json::Value::is_boolean)
        }
        Some("get") => {
            exact_required_optional_keys(parameters, &["type"], &["target", "depth"])
                && optional_bounded_string(parameters, "target", 16 * 1_024, false)
                && optional_safe_integer(parameters, "depth")
        }
        Some("query") => {
            exact_required_optional_keys(
                parameters,
                &["type", "selector"],
                &["contains", "compact", "fields"],
            ) && bounded_string(&parameters["selector"], 16 * 1_024, false)
                && optional_bounded_string(parameters, "contains", 16 * 1_024, true)
                && parameters
                    .get("compact")
                    .is_none_or(serde_json::Value::is_boolean)
                && parameters
                    .get("fields")
                    .is_none_or(|value| current_string_array_is_safe(value, 1_024))
        }
        Some("validate") => exact_keys(parameters, &["type"]),
        Some("set") => {
            exact_required_optional_keys(
                parameters,
                &["type", "target"],
                &["properties", "replacement", "force"],
            ) && bounded_string(&parameters["target"], 16 * 1_024, false)
                && parameters
                    .get("properties")
                    .is_none_or(current_office_property_map_is_safe)
                && parameters.get("replacement").is_none_or(|replacement| {
                    replacement.as_object().is_some_and(|replacement| {
                        exact_keys(replacement, &["find", "replace"])
                            && bounded_string(&replacement["find"], 128 * 1_024, true)
                            && bounded_string(&replacement["replace"], 128 * 1_024, true)
                    })
                })
                && parameters
                    .get("force")
                    .is_none_or(serde_json::Value::is_boolean)
        }
        Some("add") => {
            exact_required_optional_keys(
                parameters,
                &["type", "parent", "elementType"],
                &["copyFrom", "position", "properties", "force"],
            ) && bounded_string(&parameters["parent"], 16 * 1_024, false)
                && bounded_string(&parameters["elementType"], 1_024, false)
                && optional_bounded_string(parameters, "copyFrom", 16 * 1_024, false)
                && parameters
                    .get("position")
                    .is_none_or(current_office_position_is_safe)
                && parameters
                    .get("properties")
                    .is_none_or(current_office_property_map_is_safe)
                && parameters
                    .get("force")
                    .is_none_or(serde_json::Value::is_boolean)
        }
        Some("remove") => {
            exact_required_optional_keys(parameters, &["type", "target"], &["shift", "properties"])
                && bounded_string(&parameters["target"], 16 * 1_024, false)
                && parameters
                    .get("shift")
                    .is_none_or(|value| matches!(value.as_str(), Some("left" | "up")))
                && parameters
                    .get("properties")
                    .is_none_or(current_office_property_map_is_safe)
        }
        Some("move") => {
            exact_required_optional_keys(
                parameters,
                &["type", "target"],
                &["newParent", "position", "properties"],
            ) && bounded_string(&parameters["target"], 16 * 1_024, false)
                && optional_bounded_string(parameters, "newParent", 16 * 1_024, false)
                && parameters
                    .get("position")
                    .is_none_or(current_office_position_is_safe)
                && parameters
                    .get("properties")
                    .is_none_or(current_office_property_map_is_safe)
        }
        Some("swap") => {
            exact_keys(parameters, &["type", "firstTarget", "secondTarget"])
                && bounded_string(&parameters["firstTarget"], 16 * 1_024, false)
                && bounded_string(&parameters["secondTarget"], 16 * 1_024, false)
        }
        Some(_) | None => false,
    }
}

fn current_office_request_is_safe(value: &serde_json::Value) -> bool {
    let Some(request) = value.as_object() else {
        return false;
    };
    exact_keys(
        request,
        &[
            "documentKind",
            "operation",
            "documentPath",
            "outputPath",
            "destinationPath",
            "inputs",
            "timeoutMs",
            "parameters",
        ],
    ) && matches!(
        request["documentKind"].as_str(),
        Some("document" | "spreadsheet" | "presentation")
    ) && matches!(
        request["operation"].as_str(),
        Some(
            "help"
                | "create"
                | "view"
                | "get"
                | "query"
                | "validate"
                | "set"
                | "add"
                | "remove"
                | "move"
                | "swap"
        )
    ) && nullable_bounded_string(&request["documentPath"], 16 * 1_024, false)
        && nullable_bounded_string(&request["outputPath"], 16 * 1_024, false)
        && nullable_bounded_string(&request["destinationPath"], 16 * 1_024, false)
        && request["inputs"].as_array().is_some_and(|inputs| {
            inputs.len() <= MAX_CURRENT_RUN_ITEMS
                && inputs.iter().all(current_file_input_spec_is_safe)
        })
        && (request["timeoutMs"].is_null() || safe_integer(&request["timeoutMs"]))
        && current_office_parameters_are_safe(&request["parameters"])
        && request["parameters"]
            .get("type")
            .and_then(serde_json::Value::as_str)
            == request["operation"].as_str()
}

fn current_office_path_slot_is_safe(value: &serde_json::Value) -> bool {
    let Some(slot) = value.as_object() else {
        return false;
    };
    match slot.get("type").and_then(serde_json::Value::as_str) {
        Some("document" | "output" | "destination") => exact_keys(slot, &["type"]),
        Some("resource") => exact_keys(slot, &["type", "index"]) && safe_integer(&slot["index"]),
        Some(_) | None => false,
    }
}

fn current_office_path_identity_is_safe(value: &serde_json::Value) -> bool {
    let Some(identity) = value.as_object() else {
        return false;
    };
    exact_keys(identity, &["revision", "device", "inode"])
        && bounded_string(&identity["revision"], 1_024, false)
        && (identity["device"].is_null() || safe_integer(&identity["device"]))
        && (identity["inode"].is_null() || safe_integer(&identity["inode"]))
}

fn current_office_frozen_path_is_safe(value: &serde_json::Value) -> bool {
    let Some(path) = value.as_object() else {
        return false;
    };
    exact_keys(
        path,
        &[
            "slot",
            "logicalPath",
            "purpose",
            "scope",
            "normalizedPath",
            "state",
            "objectIdentity",
            "parentIdentity",
            "contentRevision",
            "size",
            "writeDisposition",
        ],
    ) && current_office_path_slot_is_safe(&path["slot"])
        && bounded_string(&path["logicalPath"], 16 * 1_024, false)
        && matches!(
            path["purpose"].as_str(),
            Some("readSource" | "writeTarget" | "inPlaceTarget")
        )
        && matches!(
            path["scope"].as_str(),
            Some("workspace" | "external" | "attachment")
        )
        && bounded_string(&path["normalizedPath"], 16 * 1_024, false)
        && matches!(path["state"].as_str(), Some("missing" | "present"))
        && (path["objectIdentity"].is_null()
            || current_office_path_identity_is_safe(&path["objectIdentity"]))
        && current_office_path_identity_is_safe(&path["parentIdentity"])
        && nullable_bounded_string(&path["contentRevision"], 1_024, false)
        && (path["size"].is_null() || safe_integer(&path["size"]))
        && (path["writeDisposition"].is_null()
            || matches!(
                path["writeDisposition"].as_str(),
                Some("createNew" | "replaceExisting")
            ))
}

fn current_office_prepared_is_safe(value: &serde_json::Value) -> bool {
    let Some(prepared) = value.as_object() else {
        return false;
    };
    exact_keys(
        prepared,
        &[
            "schemaVersion",
            "providerId",
            "engineRevision",
            "workspaceRevision",
            "access",
            "request",
            "argv",
            "paths",
            "inputBindings",
        ],
    ) && prepared["schemaVersion"] == crate::office::OFFICE_PREPARED_EXECUTION_SCHEMA_VERSION
        && bounded_string(&prepared["providerId"], 1_024, false)
        && bounded_string(&prepared["engineRevision"], 4_096, false)
        && nullable_bounded_string(&prepared["workspaceRevision"], 4_096, false)
        && matches!(prepared["access"].as_str(), Some("readOnly" | "fileWrite"))
        && current_office_request_is_safe(&prepared["request"])
        && current_string_array_is_safe(&prepared["argv"], 64 * 1_024)
        && prepared["paths"].as_array().is_some_and(|paths| {
            paths.len() <= MAX_CURRENT_RUN_ITEMS
                && paths.iter().all(current_office_frozen_path_is_safe)
        })
        && prepared["inputBindings"]
            .as_array()
            .is_some_and(|bindings| {
                bindings.len() <= MAX_CURRENT_RUN_ITEMS
                    && bindings.iter().all(current_file_input_binding_is_safe)
            })
}

fn current_office_operation_is_safe(
    operation: &serde_json::Map<String, serde_json::Value>,
) -> bool {
    exact_keys(
        operation,
        &[
            "schemaVersion",
            "id",
            "semanticArgs",
            "prepared",
            "approvalStatus",
            "reason",
        ],
    ) && operation["schemaVersion"] == crate::AGENT_OFFICE_OPERATION_SCHEMA_VERSION
        && bounded_string(&operation["id"], 1_024, false)
        && operation["semanticArgs"].is_object()
        && current_office_prepared_is_safe(&operation["prepared"])
        && current_approval_status_is_safe(&operation["approvalStatus"])
        && bounded_string(&operation["reason"], 16 * 1_024, true)
}

fn current_diff_is_safe(diff: &serde_json::Map<String, serde_json::Value>) -> bool {
    exact_keys(
        diff,
        &[
            "id",
            "operation",
            "filePath",
            "patch",
            "baseRevision",
            "summary",
            "approvalStatus",
        ],
    ) && bounded_string(&diff["id"], 1_024, false)
        && bounded_string(&diff["filePath"], 16 * 1_024, false)
        && bounded_string(&diff["patch"], 4 * 1_024 * 1_024, true)
        && (diff["baseRevision"].is_null() || bounded_string(&diff["baseRevision"], 1_024, false))
        && (diff["summary"].is_null() || bounded_string(&diff["summary"], 16 * 1_024, true))
        && serde_json::from_value::<crate::AgentDiffProposal>(serde_json::Value::Object(
            diff.clone(),
        ))
        .is_ok()
}

fn current_todo_is_safe(value: &serde_json::Value) -> bool {
    let Some(todo) = value.as_object() else {
        return false;
    };
    exact_keys(todo, &["revision", "items", "updatedAt"])
        && safe_integer(&todo["revision"])
        && safe_integer(&todo["updatedAt"])
        && record_array_is_safe(&todo["items"], |item| {
            exact_required_optional_keys(
                item,
                &["id", "title", "status", "createdAt", "updatedAt"],
                &["note"],
            ) && bounded_string(&item["id"], 1_024, false)
                && bounded_string(&item["title"], 64 * 1_024, true)
                && matches!(
                    item["status"].as_str(),
                    Some("pending" | "in_progress" | "completed" | "blocked")
                )
                && safe_integer(&item["createdAt"])
                && safe_integer(&item["updatedAt"])
                && optional_bounded_string(item, "note", 64 * 1_024, true)
        })
}

fn current_skill_installation_is_safe(
    installation: &serde_json::Map<String, serde_json::Value>,
) -> bool {
    if !exact_keys(installation, &["action", "status"])
        || !matches!(
            installation["status"].as_str(),
            Some(
                "waiting_for_approval"
                    | "installing"
                    | "installed"
                    | "already_installed"
                    | "rejected"
                    | "failed"
                    | "uncertain"
            )
        )
    {
        return false;
    }
    let Some(action) = installation["action"].as_object() else {
        return false;
    };
    let Some(preview) = action.get("preview").and_then(serde_json::Value::as_object) else {
        return false;
    };
    let Some(resource_summary) = preview
        .get("resourceSummary")
        .and_then(serde_json::Value::as_object)
    else {
        return false;
    };
    exact_keys(
        action,
        &[
            "schemaVersion",
            "id",
            "installRef",
            "preview",
            "approvalStatus",
            "expiresAt",
        ],
    ) && action["schemaVersion"] == crate::AGENT_SKILL_INSTALLATION_SCHEMA_VERSION
        && bounded_string(&action["id"], 1_024, false)
        && bounded_string(&action["installRef"], 4_096, false)
        && safe_integer(&action["expiresAt"])
        && exact_keys(
            preview,
            &[
                "name",
                "description",
                "sourceSummary",
                "resolvedRevision",
                "fileCount",
                "totalBytes",
                "resourceSummary",
                "containsScripts",
                "warnings",
                "compatibility",
                "operation",
                "impact",
            ],
        )
        && bounded_string(&preview["name"], 1_024, false)
        && bounded_string(&preview["description"], 128 * 1_024, true)
        && bounded_string(&preview["resolvedRevision"], 4_096, false)
        && safe_integer(&preview["fileCount"])
        && safe_integer(&preview["totalBytes"])
        && exact_keys(
            resource_summary,
            &["total", "references", "assets", "scripts", "bytes"],
        )
        && resource_summary.values().all(safe_integer)
        && preview["containsScripts"].is_boolean()
        && record_array_is_safe(&preview["warnings"], |warning| {
            exact_keys(warning, &["code", "message", "requiresAcknowledgement"])
                && bounded_string(&warning["code"], 1_024, false)
                && bounded_string(&warning["message"], 128 * 1_024, true)
                && warning["requiresAcknowledgement"].is_boolean()
        })
        && bounded_string(&preview["compatibility"], 1_024, false)
        && bounded_string(&preview["operation"], 1_024, false)
        && bounded_string(&preview["impact"], 1_024, false)
        && serde_json::from_value::<crate::AgentSkillInstallationRequest>(
            serde_json::Value::Object(action.clone()),
        )
        .is_ok()
}

fn current_activated_skill_is_safe(skill: &serde_json::Map<String, serde_json::Value>) -> bool {
    let Some(source) = skill.get("source").and_then(serde_json::Value::as_object) else {
        return false;
    };
    exact_keys(skill, &["id", "name", "revision", "source"])
        && bounded_string(&skill["id"], 1_024, false)
        && bounded_string(&skill["name"], 1_024, false)
        && bounded_string(&skill["revision"], 1_024, false)
        && exact_keys(source, &["kind", "id"])
        && matches!(
            source["kind"].as_str(),
            Some("workspace" | "bundled" | "installed")
        )
        && bounded_string(&source["id"], 1_024, false)
}

fn current_skill_selection_is_safe(selection: &serde_json::Map<String, serde_json::Value>) -> bool {
    exact_keys(selection, &["id", "revision"])
        && bounded_string(&selection["id"], 1_024, false)
        && bounded_string(&selection["revision"], 1_024, false)
}

fn current_command_output_is_safe(output: &serde_json::Map<String, serde_json::Value>) -> bool {
    if !exact_required_optional_keys(
        output,
        &[
            "name",
            "kind",
            "readPath",
            "mimeType",
            "sizeBytes",
            "sha256",
        ],
        &["width", "height"],
    ) || !bounded_string(&output["name"], 1_024, false)
        || !output["name"].as_str().is_some_and(|name| {
            name == name.trim()
                && !name
                    .chars()
                    .any(|character| character <= '\u{001f}' || character == '\u{007f}')
        })
        || !safe_integer(&output["sizeBytes"])
        || output["sizeBytes"].as_u64() == Some(0)
        || !output["sha256"].as_str().is_some_and(|digest| {
            digest.len() == 64
                && digest
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
    {
        return false;
    }
    let digest = output["sha256"].as_str().expect("validated digest");
    match output["kind"].as_str() {
        Some("document") => {
            output["readPath"] == format!("artifact://sha256/{digest}")
                && output["mimeType"] == "application/pdf"
                && !output.contains_key("width")
                && !output.contains_key("height")
        }
        Some("image") => {
            output["readPath"] == format!("image-artifact://sha256/{digest}")
                && matches!(
                    output["mimeType"].as_str(),
                    Some("image/png" | "image/jpeg" | "image/webp")
                )
                && output.get("width").is_some_and(safe_integer)
                && output.get("width").and_then(serde_json::Value::as_u64) != Some(0)
                && output.get("height").is_some_and(safe_integer)
                && output.get("height").and_then(serde_json::Value::as_u64) != Some(0)
        }
        _ => false,
    }
}

fn current_command_sessions_are_safe(
    value: &serde_json::Value,
    run_command_call_ids: &HashSet<&str>,
) -> bool {
    let Some(sessions) = value.as_object() else {
        return false;
    };
    sessions.len() <= MAX_CURRENT_RUN_ITEMS
        && sessions.iter().all(|(call_id, value)| {
            let Some(session) = value.as_object() else {
                return false;
            };
            run_command_call_ids.contains(call_id.as_str())
                && exact_required_optional_keys(
                    session,
                    &["callId", "status", "latestSequence", "outputTruncated"],
                    &["startedAt", "endedAt", "exitCode", "outputs"],
                )
                && session["callId"] == *call_id
                && matches!(
                    session["status"].as_str(),
                    Some("exited" | "interrupted" | "timed_out" | "failed" | "outcome_unknown")
                )
                && safe_integer(&session["latestSequence"])
                && session["outputTruncated"].is_boolean()
                && session.get("startedAt").is_none_or(safe_integer)
                && session.get("endedAt").is_none_or(safe_integer)
                && session.get("exitCode").is_none_or(|value| {
                    value.as_i64().is_some_and(|value| {
                        (-(MAX_JS_SAFE_INTEGER as i64)..=MAX_JS_SAFE_INTEGER as i64)
                            .contains(&value)
                    })
                })
                && session.get("outputs").is_none_or(|outputs| {
                    outputs.as_array().is_some_and(|outputs| {
                        outputs.len() <= 32
                            && outputs.iter().all(|output| {
                                output
                                    .as_object()
                                    .is_some_and(current_command_output_is_safe)
                            })
                    })
                })
        })
}

fn current_file_draft_is_safe(draft: &serde_json::Map<String, serde_json::Value>) -> bool {
    let required = [
        "draftId",
        "conversationId",
        "filePath",
        "mode",
        "status",
        "additions",
        "deletions",
        "lineCount",
        "byteCount",
        "chunkCount",
        "nextChunkIndex",
        "statsFinal",
        "createdAt",
        "updatedAt",
    ];
    exact_required_optional_keys(draft, &required, &["projectId", "baseRevision", "summary"])
        && bounded_string(&draft["draftId"], 1_024, false)
        && bounded_string(&draft["conversationId"], 1_024, true)
        && bounded_string(&draft["filePath"], 16 * 1_024, false)
        && matches!(
            draft["mode"].as_str(),
            Some("create" | "rewrite" | "modify" | "append" | "upsert")
        )
        && matches!(
            draft["status"].as_str(),
            Some(
                "drafting"
                    | "ready"
                    | "waiting_approval"
                    | "applying"
                    | "applied"
                    | "rejected"
                    | "conflict"
                    | "failed"
                    | "aborted"
                    | "expired"
            )
        )
        && [
            "additions",
            "deletions",
            "lineCount",
            "byteCount",
            "chunkCount",
            "nextChunkIndex",
            "createdAt",
            "updatedAt",
        ]
        .iter()
        .all(|field| safe_integer(&draft[*field]))
        && draft["statsFinal"].is_boolean()
        && optional_bounded_string(draft, "projectId", 1_024, false)
        && optional_bounded_string(draft, "baseRevision", 1_024, false)
        && optional_bounded_string(draft, "summary", 16 * 1_024, true)
}

fn current_web_source_is_safe(source: &serde_json::Map<String, serde_json::Value>) -> bool {
    exact_required_optional_keys(
        source,
        &["id", "title", "url", "displayUrl", "domain"],
        &["faviconUrl", "snippet", "score", "publishedDate"],
    ) && bounded_string(&source["id"], 1_024, false)
        && bounded_string(&source["title"], 128 * 1_024, true)
        && bounded_string(&source["url"], 16 * 1_024, false)
        && bounded_string(&source["displayUrl"], 16 * 1_024, true)
        && bounded_string(&source["domain"], 4_096, false)
        && optional_bounded_string(source, "faviconUrl", 16 * 1_024, true)
        && optional_bounded_string(source, "snippet", 128 * 1_024, true)
        && source.get("score").is_none_or(serde_json::Value::is_number)
        && optional_bounded_string(source, "publishedDate", 4_096, true)
}

fn current_web_activity_is_safe(activity: &serde_json::Map<String, serde_json::Value>) -> bool {
    exact_required_optional_keys(
        activity,
        &[
            "callId",
            "query",
            "provider",
            "status",
            "sources",
            "updatedAt",
        ],
        &[
            "kind",
            "answer",
            "summaryQuality",
            "error",
            "responseTime",
            "truncated",
        ],
    ) && bounded_string(&activity["callId"], 1_024, false)
        && bounded_string(&activity["query"], 128 * 1_024, true)
        && bounded_string(&activity["provider"], 1_024, false)
        && matches!(
            activity["status"].as_str(),
            Some("running" | "completed" | "failed" | "cancelled")
        )
        && record_array_is_safe(&activity["sources"], current_web_source_is_safe)
        && safe_integer(&activity["updatedAt"])
        && activity
            .get("kind")
            .is_none_or(|value| matches!(value.as_str(), Some("search" | "fetch")))
        && optional_bounded_string(activity, "answer", 4 * 1_024 * 1_024, true)
        && activity
            .get("summaryQuality")
            .is_none_or(|value| matches!(value.as_str(), Some("good" | "low")))
        && optional_bounded_string(activity, "error", 128 * 1_024, true)
        && activity
            .get("responseTime")
            .is_none_or(|value| value.is_null() || value.is_number() || value.is_string())
        && activity
            .get("truncated")
            .is_none_or(serde_json::Value::is_boolean)
}

fn current_read_activity_is_safe(activity: &serde_json::Map<String, serde_json::Value>) -> bool {
    exact_required_optional_keys(
        activity,
        &[
            "callId",
            "tool",
            "kind",
            "status",
            "path",
            "fileName",
            "updatedAt",
        ],
        &[
            "extension",
            "mimeType",
            "thumbnailDataUrl",
            "fullDataUrl",
            "error",
        ],
    ) && bounded_string(&activity["callId"], 1_024, false)
        && bounded_string(&activity["tool"], 1_024, false)
        && matches!(
            activity["kind"].as_str(),
            Some("file" | "image" | "word" | "presentation" | "spreadsheet")
        )
        && matches!(
            activity["status"].as_str(),
            Some("running" | "completed" | "failed" | "cancelled")
        )
        && bounded_string(&activity["path"], 16 * 1_024, false)
        && bounded_string(&activity["fileName"], 4_096, false)
        && safe_integer(&activity["updatedAt"])
        && optional_bounded_string(activity, "extension", 1_024, true)
        && optional_bounded_string(activity, "mimeType", 1_024, true)
        && optional_bounded_string(activity, "thumbnailDataUrl", 32 * 1_024 * 1_024, true)
        && optional_bounded_string(activity, "fullDataUrl", 32 * 1_024 * 1_024, true)
        && optional_bounded_string(activity, "error", 128 * 1_024, true)
}

fn current_timeline_attachment_is_safe(
    attachment: &serde_json::Map<String, serde_json::Value>,
) -> bool {
    exact_required_optional_keys(
        attachment,
        &["id", "kind", "name", "sizeBytes"],
        &[
            "mimeType",
            "encoding",
            "data",
            "previewData",
            "previewMimeType",
            "createdAt",
        ],
    ) && bounded_string(&attachment["id"], 1_024, false)
        && matches!(attachment["kind"].as_str(), Some("file" | "image"))
        && bounded_string(&attachment["name"], 4_096, false)
        && safe_integer(&attachment["sizeBytes"])
        && attachment
            .get("mimeType")
            .is_none_or(|value| value.is_null() || bounded_string(value, 1_024, true))
        && attachment
            .get("encoding")
            .is_none_or(|value| matches!(value.as_str(), Some("utf8" | "base64")))
        && optional_bounded_string(attachment, "data", 32 * 1_024 * 1_024, true)
        && attachment
            .get("previewData")
            .is_none_or(|value| value.is_null() || bounded_string(value, 32 * 1_024 * 1_024, true))
        && attachment
            .get("previewMimeType")
            .is_none_or(|value| value.is_null() || bounded_string(value, 1_024, true))
        && attachment.get("createdAt").is_none_or(safe_integer)
}

fn current_timeline_item_is_safe(item: &serde_json::Map<String, serde_json::Value>) -> bool {
    if !bounded_string(
        item.get("id").unwrap_or(&serde_json::Value::Null),
        1_024,
        false,
    ) {
        return false;
    }
    match item.get("type").and_then(serde_json::Value::as_str) {
        Some("message") => {
            exact_required_optional_keys(item, &["id", "type", "content"], &["streamId"])
                && bounded_string(&item["content"], 4 * 1_024 * 1_024, true)
                && optional_bounded_string(item, "streamId", 1_024, false)
        }
        Some("tool_call") => {
            exact_keys(item, &["id", "type", "callId"])
                && bounded_string(&item["callId"], 1_024, false)
        }
        Some("mcp_tool_call") => {
            exact_keys(item, &["id", "type", "invocationId"])
                && bounded_string(&item["invocationId"], 1_024, false)
        }
        Some("context_compaction") => {
            exact_keys(item, &["id", "type", "operationId", "status"])
                && bounded_string(&item["operationId"], 1_024, false)
                && matches!(
                    item["status"].as_str(),
                    Some("running" | "applied" | "skipped" | "failed" | "cancelled")
                )
        }
        Some("error") => {
            exact_keys(item, &["id", "type", "message"])
                && bounded_string(&item["message"], 128 * 1_024, true)
        }
        Some("user_guidance") => {
            exact_required_optional_keys(
                item,
                &[
                    "id",
                    "type",
                    "clientMessageId",
                    "content",
                    "attachments",
                    "status",
                    "createdAt",
                ],
                &[
                    "guidanceId",
                    "rejectionCode",
                    "error",
                    "recoverable",
                    "sequence",
                ],
            ) && bounded_string(&item["clientMessageId"], 1_024, false)
                && bounded_string(&item["content"], 4 * 1_024 * 1_024, true)
                && record_array_is_safe(&item["attachments"], current_timeline_attachment_is_safe)
                && matches!(
                    item["status"].as_str(),
                    Some("submitting" | "queued" | "applied" | "rejected")
                )
                && safe_integer(&item["createdAt"])
                && optional_bounded_string(item, "guidanceId", 1_024, false)
                && optional_bounded_string(item, "rejectionCode", 1_024, false)
                && optional_bounded_string(item, "error", 128 * 1_024, true)
                && item
                    .get("recoverable")
                    .is_none_or(serde_json::Value::is_boolean)
                && item.get("sequence").is_none_or(safe_integer)
        }
        _ => false,
    }
}

fn current_message_stream_checkpoints_are_safe(value: &serde_json::Value) -> bool {
    value.as_object().is_some_and(|checkpoints| {
        checkpoints.len() <= MAX_CURRENT_RUN_ITEMS
            && checkpoints.values().all(|checkpoint| {
                checkpoint.as_object().is_some_and(|checkpoint| {
                    exact_keys(checkpoint, &["baseContentLength", "baseWasThinking"])
                        && safe_integer(&checkpoint["baseContentLength"])
                        && checkpoint["baseWasThinking"].is_boolean()
                })
            })
    })
}

fn current_mcp_invocation_is_safe(value: &serde_json::Value) -> bool {
    const REQUIRED: &[&str] = &[
        "actionId",
        "invocationId",
        "callId",
        "serverId",
        "serverDisplayName",
        "rawToolName",
        "modelToolName",
        "external",
        "state",
        "dispatchCertainty",
        "outputTruncated",
    ];
    const OPTIONAL: &[&str] = &[
        "scope",
        "displayReason",
        "outcome",
        "isError",
        "errorCode",
        "rejectionReason",
        "durationMs",
    ];
    let Some(invocation) = value.as_object() else {
        return false;
    };
    if !REQUIRED.iter().all(|field| invocation.contains_key(*field))
        || !invocation
            .keys()
            .all(|key| REQUIRED.contains(&key.as_str()) || OPTIONAL.contains(&key.as_str()))
    {
        return false;
    }
    for field in [
        "actionId",
        "invocationId",
        "callId",
        "serverId",
        "serverDisplayName",
        "rawToolName",
        "modelToolName",
        "state",
        "dispatchCertainty",
    ] {
        if invocation
            .get(field)
            .and_then(serde_json::Value::as_str)
            .is_none_or(|value| value.is_empty())
        {
            return false;
        }
    }
    if invocation.get("external") != Some(&serde_json::Value::Bool(true))
        || !invocation
            .get("outputTruncated")
            .is_some_and(serde_json::Value::is_boolean)
    {
        return false;
    }
    if invocation.get("scope").is_some_and(|scope| {
        let Some(scope) = scope.as_object() else {
            return true;
        };
        serde_json::from_value::<AgentMcpServerScope>(serde_json::Value::Object(scope.clone()))
            .is_err()
            || !match scope["type"].as_str() {
                Some("builtin" | "user" | "managed") => exact_keys(scope, &["type"]),
                Some("project") => {
                    exact_keys(scope, &["type", "projectId"])
                        && bounded_string(&scope["projectId"], 1_024, false)
                }
                Some("plugin") => {
                    exact_keys(scope, &["type", "pluginId"])
                        && bounded_string(&scope["pluginId"], 1_024, false)
                }
                _ => false,
            }
    }) {
        return false;
    }
    for field in ["displayReason", "outcome", "errorCode", "rejectionReason"] {
        if invocation
            .get(field)
            .is_some_and(|value| !value.is_string())
        {
            return false;
        }
    }
    if invocation
        .get("isError")
        .is_some_and(|value| !value.is_boolean())
        || invocation
            .get("durationMs")
            .is_some_and(|value| !safe_integer(value))
        || !bounded_string(&invocation["callId"], 1_024, false)
        || !bounded_string(&invocation["serverDisplayName"], 1_024, false)
        || !bounded_string(&invocation["rawToolName"], 1_024, false)
        || !bounded_string(&invocation["modelToolName"], 1_024, false)
        || !optional_bounded_string(invocation, "displayReason", 512, false)
        || !optional_bounded_string(invocation, "errorCode", 1_024, false)
        || !optional_bounded_string(invocation, "rejectionReason", 512, false)
    {
        return false;
    }
    let Some(action_id) = invocation["actionId"].as_str() else {
        return false;
    };
    let Some(invocation_id) = invocation["invocationId"].as_str() else {
        return false;
    };
    let Some(server_id) = invocation["serverId"].as_str() else {
        return false;
    };
    if !current_uuid_is_safe(action_id)
        || !current_uuid_is_safe(invocation_id)
        || !current_uuid_is_safe(server_id)
        || action_id == invocation_id
    {
        return false;
    }
    let state = invocation["state"].as_str();
    let dispatch = invocation["dispatchCertainty"].as_str();
    let outcome = invocation
        .get("outcome")
        .and_then(serde_json::Value::as_str);
    let is_error = invocation
        .get("isError")
        .and_then(serde_json::Value::as_bool);
    let has_error_code = invocation.contains_key("errorCode");
    let has_duration = invocation.contains_key("durationMs");
    let truncated = invocation["outputTruncated"].as_bool().unwrap_or(false);
    match state {
        Some("pending_approval" | "approved") => {
            dispatch == Some("definitely_not_dispatched")
                && outcome.is_none()
                && is_error.is_none()
                && !has_error_code
                && !has_duration
                && !truncated
        }
        Some("dispatching" | "running") => {
            dispatch == Some("possibly_dispatched")
                && outcome.is_none()
                && is_error.is_none()
                && !has_error_code
                && !has_duration
                && !truncated
        }
        Some("completed") => {
            dispatch == Some("response_received")
                && has_duration
                && ((outcome == Some("succeeded") && is_error == Some(false) && !has_error_code)
                    || (outcome == Some("tool_error") && is_error == Some(true) && has_error_code))
        }
        Some("failed") => {
            has_duration
                && has_error_code
                && is_error == Some(true)
                && matches!(
                    (outcome, dispatch),
                    (Some("output_too_large"), Some("response_received"))
                        | (Some("timed_out"), Some("definitely_not_dispatched"))
                        | (Some("transport_error"), Some("definitely_not_dispatched"))
                        | (Some("transport_error"), Some("response_received"))
                )
                && (dispatch != Some("definitely_not_dispatched") || !truncated)
        }
        Some("cancelled") => {
            dispatch == Some("definitely_not_dispatched")
                && outcome == Some("cancelled")
                && is_error.is_none()
                && has_error_code
                && !truncated
        }
        Some("rejected") => {
            dispatch == Some("definitely_not_dispatched")
                && outcome == Some("rejected")
                && is_error.is_none()
                && has_error_code
                && !truncated
        }
        Some("expired") => {
            dispatch == Some("definitely_not_dispatched")
                && outcome == Some("expired")
                && is_error.is_none()
                && has_error_code
                && !truncated
        }
        Some("payload_unavailable") => {
            dispatch == Some("definitely_not_dispatched")
                && outcome == Some("payload_unavailable")
                && is_error == Some(true)
                && has_error_code
                && !truncated
        }
        Some("policy_denied") => {
            dispatch == Some("definitely_not_dispatched")
                && outcome == Some("policy_denied")
                && is_error.is_none()
                && has_error_code
                && !truncated
        }
        Some("outcome_unknown") => {
            dispatch == Some("possibly_dispatched")
                && outcome == Some("outcome_unknown")
                && is_error.is_none()
                && has_error_code
                && !truncated
        }
        _ => false,
    }
}

fn current_agent_run_projection_is_safe(
    run: &serde_json::Map<String, serde_json::Value>,
    expected_run_id: &str,
) -> bool {
    const REQUIRED: &[&str] = &[
        "runId",
        "status",
        "startedAt",
        "toolDefinitions",
        "toolCalls",
        "toolResults",
        "webSearchActivities",
        "readActivities",
        "approvals",
        "diffs",
        "fileDrafts",
        "mcpInvocations",
        "messageStreamCheckpoints",
        "timeline",
    ];
    if !only_allowed_keys(run, CURRENT_AGENT_RUN_KEYS)
        || !REQUIRED.iter().all(|field| run.contains_key(*field))
        || !run.get("runId").is_some_and(|value| {
            value.is_null()
                || value
                    .as_str()
                    .is_some_and(|run_id| run_id == expected_run_id)
        })
        || !matches!(
            run["status"].as_str(),
            Some(
                "starting"
                    | "idle"
                    | "queued"
                    | "running"
                    | "waiting_for_approval"
                    | "completed"
                    | "failed"
                    | "cancelled"
            )
        )
        || !safe_integer(&run["startedAt"])
        || !run.get("firstResponseAt").is_none_or(safe_integer)
        || !run.get("lastResponseAt").is_none_or(safe_integer)
        || !run.get("completedAt").is_none_or(safe_integer)
    {
        return false;
    }
    let terminal = matches!(
        run["status"].as_str(),
        Some("completed" | "failed" | "cancelled")
    );
    if terminal != run.contains_key("completedAt")
        || !record_array_is_safe(&run["toolDefinitions"], current_tool_definition_is_safe)
        || !record_array_is_safe(&run["toolCalls"], current_tool_call_is_safe)
        || !record_array_is_safe(&run["toolResults"], current_tool_result_is_safe)
        || !record_array_is_safe(&run["webSearchActivities"], current_web_activity_is_safe)
        || !record_array_is_safe(&run["readActivities"], current_read_activity_is_safe)
        || !record_array_is_safe(&run["approvals"], current_persisted_approval_is_safe)
        || !record_array_is_safe(&run["diffs"], current_diff_is_safe)
        || !record_array_is_safe(&run["fileDrafts"], current_file_draft_is_safe)
        || !run["mcpInvocations"].as_array().is_some_and(|invocations| {
            invocations.len() <= MAX_CURRENT_RUN_ITEMS
                && invocations.iter().all(current_mcp_invocation_is_safe)
        })
        || !record_array_is_safe(&run["timeline"], current_timeline_item_is_safe)
        || !current_message_stream_checkpoints_are_safe(&run["messageStreamCheckpoints"])
    {
        return false;
    }

    if run
        .get("todo")
        .is_some_and(|todo| !current_todo_is_safe(todo))
        || run.get("skillInstallations").is_some_and(|installations| {
            !record_array_is_safe(installations, current_skill_installation_is_safe)
        })
        || run
            .get("activatedSkills")
            .is_some_and(|skills| !record_array_is_safe(skills, current_activated_skill_is_safe))
        || run
            .get("explicitSkillSelections")
            .is_some_and(|selections| {
                !record_array_is_safe(selections, current_skill_selection_is_safe)
            })
    {
        return false;
    }
    if let Some(revision) = run.get("toolSetRevision") {
        let Some(revision) = revision.as_object() else {
            return false;
        };
        if !exact_keys(revision, &["stable", "dynamic", "effective"])
            || !["stable", "dynamic", "effective"]
                .iter()
                .all(|field| bounded_string(&revision[*field], 1_024, false))
        {
            return false;
        }
    }
    if let Some(state) = run.get("state") {
        let Some(state) = state.as_object() else {
            return false;
        };
        if !exact_keys(state, &["status", "activeRunId", "lastError", "updatedAt"])
            || !matches!(
                state["status"].as_str(),
                Some(
                    "idle"
                        | "queued"
                        | "running"
                        | "waiting_for_approval"
                        | "completed"
                        | "failed"
                        | "cancelled"
                )
            )
            || !safe_integer(&state["updatedAt"])
            || !state
                .get("activeRunId")
                .is_some_and(|value| value.is_null() || bounded_string(value, 1_024, false))
            || !state
                .get("lastError")
                .is_some_and(|value| value.is_null() || bounded_string(value, 128 * 1_024, true))
        {
            return false;
        }
    }
    if let Some(usage) = run.get("usage") {
        let Some(usage) = usage.as_object() else {
            return false;
        };
        if !only_allowed_keys(
            usage,
            &[
                "inputTokens",
                "outputTokens",
                "outputThinkingTokens",
                "totalTokens",
                "cachedInputTokens",
                "cacheCreationInputTokens",
                "billableRequestCount",
            ],
        ) || !usage.values().all(safe_integer)
        {
            return false;
        }
    }
    if !optional_bounded_string(run, "error", 128 * 1_024, true)
        || !optional_bounded_string(run, "finishReason", 1_024, true)
        || !optional_bounded_string(run, "skillActivationRevision", 1_024, false)
    {
        return false;
    }

    let tool_call_ids = run["toolCalls"]
        .as_array()
        .expect("validated ToolCall array")
        .iter()
        .map(|call| call["id"].as_str().expect("validated ToolCall id"))
        .collect::<Vec<_>>();
    let tool_result_ids = run["toolResults"]
        .as_array()
        .expect("validated ToolResult array")
        .iter()
        .map(|result| result["callId"].as_str().expect("validated ToolResult id"))
        .collect::<Vec<_>>();
    let run_command_call_ids = run["toolCalls"]
        .as_array()
        .expect("validated ToolCall array")
        .iter()
        .filter(|call| call["tool"] == "run_command")
        .map(|call| call["id"].as_str().expect("validated ToolCall id"))
        .collect::<HashSet<_>>();
    if run
        .get("commandSessions")
        .is_some_and(|sessions| !current_command_sessions_are_safe(sessions, &run_command_call_ids))
    {
        return false;
    }
    let invocations = run["mcpInvocations"]
        .as_array()
        .expect("validated MCP invocation array");
    let mcp_action_ids = invocations
        .iter()
        .map(|invocation| {
            invocation["actionId"]
                .as_str()
                .expect("validated MCP action id")
        })
        .collect::<Vec<_>>();
    let mcp_invocation_ids = invocations
        .iter()
        .map(|invocation| {
            invocation["invocationId"]
                .as_str()
                .expect("validated MCP invocation id")
        })
        .collect::<Vec<_>>();
    let mcp_call_ids = invocations
        .iter()
        .map(|invocation| {
            invocation["callId"]
                .as_str()
                .expect("validated MCP call id")
        })
        .collect::<Vec<_>>();
    let timeline = run["timeline"]
        .as_array()
        .expect("validated Timeline array");
    let timeline_ids = timeline
        .iter()
        .map(|item| item["id"].as_str().expect("validated Timeline id"))
        .collect::<Vec<_>>();
    let timeline_mcp_ids = timeline
        .iter()
        .filter(|item| item["type"] == "mcp_tool_call")
        .map(|item| {
            item["invocationId"]
                .as_str()
                .expect("validated Timeline MCP invocation id")
        })
        .collect::<Vec<_>>();
    let unique =
        |values: &[&str]| values.iter().copied().collect::<HashSet<_>>().len() == values.len();
    if !unique(&tool_call_ids)
        || !unique(&tool_result_ids)
        || !unique(&mcp_action_ids)
        || !unique(&mcp_invocation_ids)
        || !unique(&mcp_call_ids)
        || !unique(&timeline_ids)
        || !unique(&timeline_mcp_ids)
        || tool_call_ids.iter().any(|id| mcp_call_ids.contains(id))
        || tool_result_ids.iter().any(|id| mcp_call_ids.contains(id))
        || timeline.iter().any(|item| {
            item["type"] == "tool_call"
                && item["callId"]
                    .as_str()
                    .is_some_and(|id| mcp_call_ids.contains(&id))
        })
        || timeline_mcp_ids
            .iter()
            .any(|id| !mcp_invocation_ids.contains(id))
        || mcp_invocation_ids
            .iter()
            .any(|id| !timeline_mcp_ids.contains(id))
    {
        return false;
    }
    true
}

pub(crate) fn canonical_agent_run_lifecycle_projection(
    existing_agent_run_json: Option<&str>,
    run_id: &str,
    run_status: &str,
    started_at: i64,
    updated_at: i64,
    completed_at: Option<i64>,
) -> rusqlite::Result<String> {
    let mut run = existing_agent_run_json
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
        .and_then(|value| value.as_object().cloned())
        .filter(|run| current_agent_run_projection_is_safe(run, run_id))
        .unwrap_or_default();

    run.insert("runId".to_string(), run_id.into());
    run.insert("status".to_string(), run_status.into());
    run.entry("startedAt".to_string())
        .or_insert_with(|| started_at.into());
    if let Some(completed_at) = completed_at {
        run.insert("completedAt".to_string(), completed_at.into());
    } else {
        run.remove("completedAt");
    }

    for field in [
        "toolDefinitions",
        "toolCalls",
        "toolResults",
        "approvals",
        "diffs",
        "fileDrafts",
        "webSearchActivities",
        "readActivities",
        "mcpInvocations",
        "timeline",
    ] {
        if !run.get(field).is_some_and(serde_json::Value::is_array) {
            run.insert(field.to_string(), serde_json::json!([]));
        }
    }
    if !run
        .get("messageStreamCheckpoints")
        .is_some_and(serde_json::Value::is_object)
    {
        run.insert(
            "messageStreamCheckpoints".to_string(),
            serde_json::json!({}),
        );
    }

    let state = run
        .entry("state".to_string())
        .or_insert_with(|| serde_json::json!({}));
    if !state.is_object() {
        *state = serde_json::json!({});
    }
    let state = state
        .as_object_mut()
        .expect("canonical AgentRun lifecycle installs an object state");
    state.insert("status".to_string(), run_status.into());
    let active_run_id = if completed_at.is_none() {
        serde_json::Value::String(run_id.to_string())
    } else {
        serde_json::Value::Null
    };
    state.insert("activeRunId".to_string(), active_run_id);
    if !state
        .get("lastError")
        .is_some_and(|value| value.is_string() || value.is_null())
    {
        state.insert("lastError".to_string(), serde_json::Value::Null);
    }
    state.insert("updatedAt".to_string(), updated_at.into());

    serde_json::to_string(&serde_json::Value::Object(run))
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))
}

/// Repairs an assistant lifecycle after process restart using the backend-owned trace identity.
///
/// Startup reconciliation always emits the complete current Renderer-safe AgentRun shape. Missing
/// or malformed presentation JSON is not interpreted or patched as an older shape; durable trace
/// identity supplies the lifecycle owner, while presentation collections restart empty.
pub(crate) fn reconcile_message_run_terminal_state(
    connection: &Connection,
    conversation_id: &str,
    message_id: &str,
    run_id: &str,
    message_status: &str,
    run_status: &str,
    completed_at: i64,
) -> rusqlite::Result<()> {
    let Some((existing_agent_run_json, started_at)) = connection
        .query_row(
            "SELECT agent_run_json, created_at
             FROM messages WHERE conversation_id = ?1 AND id = ?2",
            params![conversation_id, message_id],
            |row| Ok((row.get::<_, Option<String>>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()?
    else {
        return Ok(());
    };
    let run_json = canonical_agent_run_lifecycle_projection(
        existing_agent_run_json.as_deref(),
        run_id,
        run_status,
        started_at,
        completed_at,
        Some(completed_at),
    )?;

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
/// restores a complete current AgentRun projection and preserves every valid existing timeline,
/// tool, and approval field in `agent_run_json`.
pub fn update_message_run_waiting_state(
    connection: &Connection,
    conversation_id: &str,
    message_id: &str,
    run_id: &str,
    updated_at: i64,
) -> rusqlite::Result<()> {
    let Some((existing_agent_run_json, started_at)) = connection
        .query_row(
            "SELECT agent_run_json, created_at
             FROM messages WHERE conversation_id = ?1 AND id = ?2",
            params![conversation_id, message_id],
            |row| Ok((row.get::<_, Option<String>>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()?
    else {
        return Ok(());
    };
    let next_agent_run_json = canonical_agent_run_lifecycle_projection(
        existing_agent_run_json.as_deref(),
        run_id,
        "waiting_for_approval",
        started_at,
        updated_at,
        None,
    )?;

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

fn list_persisted_messages(
    connection: &Connection,
    conversation_id: &str,
) -> rusqlite::Result<Vec<ChatMessageRecord>> {
    let mut statement = connection.prepare(
        "SELECT id, role, content, created_at, status, agent_run_json, ui_state_json
         FROM messages
         WHERE conversation_id = ?1
         ORDER BY position ASC, created_at ASC",
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

    fn approval_is_safe(value: &serde_json::Value) -> bool {
        value
            .as_object()
            .is_some_and(current_persisted_approval_is_safe)
    }

    fn current_office_approval() -> serde_json::Value {
        serde_json::json!({
            "type": "office_operation",
            "officeOperation": {
                "schemaVersion": 5,
                "id": "office-call",
                "semanticArgs": {},
                "prepared": {
                    "schemaVersion": 5,
                    "providerId": "officecli",
                    "engineRevision": "officecli-v1",
                    "workspaceRevision": null,
                    "access": "fileWrite",
                    "request": {
                        "documentKind": "document",
                        "operation": "create",
                        "documentPath": null,
                        "outputPath": null,
                        "destinationPath": null,
                        "inputs": [],
                        "timeoutMs": null,
                        "parameters": { "type": "create" }
                    },
                    "argv": [],
                    "paths": [],
                    "inputBindings": []
                },
                "approvalStatus": "required",
                "reason": "Create a document"
            }
        })
    }

    fn current_skill_installation_approval() -> serde_json::Value {
        serde_json::json!({
            "type": "skill_installation",
            "installation": {
                "schemaVersion": 1,
                "id": "install-call",
                "installRef": "opaque-install-ref",
                "preview": {
                    "name": "Example",
                    "description": "Example Skill",
                    "sourceSummary": {},
                    "resolvedRevision": "revision-1",
                    "fileCount": 1,
                    "totalBytes": 10,
                    "resourceSummary": {
                        "total": 1,
                        "references": 0,
                        "assets": 0,
                        "scripts": 0,
                        "bytes": 10
                    },
                    "containsScripts": false,
                    "warnings": [],
                    "compatibility": "compatible",
                    "operation": "install",
                    "impact": "Adds one Skill"
                },
                "approvalStatus": "required",
                "expiresAt": 100
            }
        })
    }

    #[test]
    fn current_persisted_approval_variants_match_the_renderer_projection() {
        let approvals = [
            serde_json::json!({
                "type": "tool_call",
                "call": {
                    "id": "tool-call",
                    "tool": "read_file",
                    "args": {"path": "README.md"},
                    "approvalStatus": "required",
                    "reason": null
                }
            }),
            serde_json::json!({
                "type": "diff",
                "diff": {
                    "id": "diff-call",
                    "operation": "update",
                    "filePath": "README.md",
                    "patch": "@@ -1 +1 @@",
                    "baseRevision": null,
                    "summary": "Update README",
                    "approvalStatus": "required"
                }
            }),
            serde_json::json!({
                "type": "file_write",
                "fileWrite": {
                    "id": "write-call",
                    "draftId": "draft-1",
                    "mode": "rewrite",
                    "filePath": "README.md",
                    "baseRevision": null,
                    "summary": "Rewrite README",
                    "additions": 1,
                    "deletions": 1,
                    "lineCount": 1,
                    "byteCount": 16,
                    "approvalStatus": "required"
                }
            }),
            serde_json::json!({
                "type": "command",
                "command": {
                    "id": "command-call",
                    "command": "cargo check",
                    "cwd": null,
                    "timeoutMs": 30_000,
                    "approvalStatus": "required",
                    "riskLevel": "read_only",
                    "reason": "Check the workspace",
                    "observe": {
                        "kinds": ["office"],
                        "expectedOutputs": [""],
                        "additionalRoots": []
                    }
                }
            }),
            serde_json::json!({
                "type": "skill_materialization",
                "materialization": {
                    "id": "materialize-call",
                    "sourceUri": "skill://example/reference.md",
                    "sourcePrefix": null,
                    "destination": "reference.md",
                    "approvalStatus": "required",
                    "reason": null
                }
            }),
            serde_json::json!({
                "type": "skill_script",
                "script": {
                    "id": "script-call",
                    "scriptUri": "skill://example/scripts/check.py",
                    "skillId": "workspace:example",
                    "skillRevision": "revision-1",
                    "resourcePath": "scripts/check.py",
                    "resourceDigest": "digest-1",
                    "interpreter": "python3",
                    "args": ["--check"],
                    "requirements": {
                        "pythonDistributions": ["openpyxl"],
                        "commands": []
                    },
                    "preflight": {
                        "status": "ready",
                        "interpreter": "python3",
                        "interpreterVersion": "3.12",
                        "dependencies": [{
                            "kind": "python_distribution",
                            "name": "openpyxl",
                            "status": "available",
                            "version": "3.1"
                        }],
                        "runtimeFingerprint": "python3:3.12"
                    },
                    "timeoutMs": null,
                    "approvalStatus": "required",
                    "reason": "Run the checked script"
                }
            }),
            current_office_approval(),
            current_skill_installation_approval(),
        ];

        for approval in approvals {
            assert!(approval_is_safe(&approval), "current approval: {approval}");
        }
        assert!(!approval_is_safe(&serde_json::json!({
            "type": "mcp_tool_call",
            "approval": {}
        })));
    }

    #[test]
    fn persisted_approval_rejects_wrong_office_versions_and_malformed_nested_data() {
        let mut wrong_outer = current_office_approval();
        wrong_outer["officeOperation"]["schemaVersion"] = 4.into();
        assert!(!approval_is_safe(&wrong_outer));

        let mut wrong_inner = current_office_approval();
        wrong_inner["officeOperation"]["prepared"]["schemaVersion"] = 4.into();
        assert!(!approval_is_safe(&wrong_inner));

        let mut unsafe_integer = current_office_approval();
        unsafe_integer["officeOperation"]["prepared"]["request"]["timeoutMs"] =
            (MAX_JS_SAFE_INTEGER + 1).into();
        assert!(!approval_is_safe(&unsafe_integer));

        let mut extra_nested_field = current_office_approval();
        extra_nested_field["officeOperation"]["prepared"]["request"]["parameters"]
            ["unrecognized"] = true.into();
        assert!(!approval_is_safe(&extra_nested_field));

        let mut non_object_semantic_args = current_office_approval();
        non_object_semantic_args["officeOperation"]["semanticArgs"] = "create".into();
        assert!(!approval_is_safe(&non_object_semantic_args));

        let mut bad_script_nested_field = serde_json::json!({
            "type": "skill_script",
            "script": {
                "id": "script-call",
                "scriptUri": "skill://example/scripts/check.py",
                "skillId": "workspace:example",
                "skillRevision": "revision-1",
                "resourcePath": "scripts/check.py",
                "resourceDigest": "digest-1",
                "interpreter": "python3",
                "args": [],
                "requirements": {},
                "preflight": {
                    "status": "ready",
                    "interpreter": "python3",
                    "runtimeFingerprint": "python3:3.12"
                },
                "timeoutMs": null,
                "approvalStatus": "required",
                "reason": null
            }
        });
        bad_script_nested_field["script"]["preflight"]["unrecognized"] = true.into();
        assert!(!approval_is_safe(&bad_script_nested_field));

        let mut oversized_installation = current_skill_installation_approval();
        oversized_installation["installation"]["preview"]["name"] = "x".repeat(1_025).into();
        assert!(!approval_is_safe(&oversized_installation));
    }

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
                cached_input_price: None,
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
            "run-1",
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

    fn assert_complete_current_agent_run_lifecycle(
        run: &serde_json::Value,
        expected_status: &str,
        expected_active_run_id: Option<&str>,
        expected_completed_at: Option<i64>,
    ) {
        assert_eq!(run["runId"], "run-1");
        assert_eq!(run["status"], expected_status);
        assert_eq!(run["startedAt"], 2);
        match expected_completed_at {
            Some(completed_at) => assert_eq!(run["completedAt"], completed_at),
            None => assert!(run.get("completedAt").is_none()),
        }
        for field in [
            "toolDefinitions",
            "toolCalls",
            "toolResults",
            "approvals",
            "diffs",
            "fileDrafts",
            "webSearchActivities",
            "readActivities",
            "mcpInvocations",
            "timeline",
        ] {
            assert_eq!(run[field], serde_json::json!([]), "current {field}");
        }
        assert_eq!(run["messageStreamCheckpoints"], serde_json::json!({}));
        assert_eq!(run["state"]["status"], expected_status);
        assert_eq!(
            run["state"]["activeRunId"],
            expected_active_run_id
                .map(serde_json::Value::from)
                .unwrap_or(serde_json::Value::Null)
        );
        assert!(run["state"]["lastError"].is_null());
        assert!(run["state"]["updatedAt"].is_i64());
        assert!(run.get("assistantMessageId").is_none());
        assert!(run.get("fileWritePreviews").is_none());
    }

    #[test]
    fn startup_recovery_writes_complete_current_agent_run_from_missing_or_malformed_json() {
        let mut connection = Connection::open_in_memory().unwrap();
        migrations::run_migrations(&connection).unwrap();
        save_conversation(&mut connection, conversation()).unwrap();

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
        let terminal_raw: String = connection
            .query_row(
                "SELECT agent_run_json FROM messages WHERE id = 'assistant-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let terminal: serde_json::Value = serde_json::from_str(&terminal_raw).unwrap();
        assert_complete_current_agent_run_lifecycle(&terminal, "failed", None, Some(100));

        connection
            .execute(
                "UPDATE messages SET agent_run_json = '\"malformed-shape\"' WHERE id = 'assistant-1'",
                [],
            )
            .unwrap();
        update_message_run_waiting_state(
            &connection,
            "conversation-1",
            "assistant-1",
            "run-1",
            200,
        )
        .unwrap();
        let waiting_raw: String = connection
            .query_row(
                "SELECT agent_run_json FROM messages WHERE id = 'assistant-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let waiting: serde_json::Value = serde_json::from_str(&waiting_raw).unwrap();
        assert_complete_current_agent_run_lifecycle(
            &waiting,
            "waiting_for_approval",
            Some("run-1"),
            None,
        );
        assert_eq!(waiting["state"]["updatedAt"], 200);
    }

    #[test]
    fn startup_recovery_does_not_preserve_unknown_or_malformed_current_projection_fields() {
        let current_base = serde_json::json!({
            "runId": "run-1",
            "status": "running",
            "startedAt": 2,
            "toolDefinitions": [],
            "toolCalls": [],
            "toolResults": [],
            "approvals": [{
                "type": "command",
                "command": {
                    "id": "command-call",
                    "command": "true",
                    "cwd": null,
                    "timeoutMs": null,
                    "approvalStatus": "approved",
                    "riskLevel": "read_only",
                    "reason": null,
                    "observe": null
                }
            }],
            "diffs": [],
            "fileDrafts": [],
            "webSearchActivities": [{
                "callId": "web-call",
                "query": "current query",
                "provider": "test",
                "status": "completed",
                "sources": [{
                    "id": "source-1",
                    "title": "Current source",
                    "url": "https://example.test/source",
                    "displayUrl": "example.test/source",
                    "domain": "example.test"
                }],
                "updatedAt": 3
            }],
            "readActivities": [],
            "mcpInvocations": [],
            "messageStreamCheckpoints": {},
            "timeline": [],
            "state": {
                "status": "running",
                "activeRunId": "run-1",
                "lastError": null,
                "updatedAt": 2
            }
        });

        let mut unknown_top = current_base.clone();
        unknown_top["assistantMessageId"] = "must-not-survive".into();
        let recovered = canonical_agent_run_lifecycle_projection(
            Some(&unknown_top.to_string()),
            "run-1",
            "failed",
            2,
            10,
            Some(10),
        )
        .unwrap();
        let recovered: serde_json::Value = serde_json::from_str(&recovered).unwrap();
        assert!(recovered.get("assistantMessageId").is_none());
        assert_complete_current_agent_run_lifecycle(&recovered, "failed", None, Some(10));

        let mut malformed_nested = current_base;
        malformed_nested["state"]["privateRecoveryField"] = true.into();
        malformed_nested["approvals"] = serde_json::json!([{
            "type": "command",
            "command": { "legacyArguments": ["must-not-survive"] }
        }]);
        let recovered = canonical_agent_run_lifecycle_projection(
            Some(&malformed_nested.to_string()),
            "run-1",
            "waiting_for_approval",
            2,
            20,
            None,
        )
        .unwrap();
        let recovered: serde_json::Value = serde_json::from_str(&recovered).unwrap();
        assert_eq!(recovered["approvals"], serde_json::json!([]));
        assert!(recovered["state"].get("privateRecoveryField").is_none());
        assert_complete_current_agent_run_lifecycle(
            &recovered,
            "waiting_for_approval",
            Some("run-1"),
            None,
        );

        let current_with_presentation = serde_json::json!({
            "runId": "run-1",
            "status": "running",
            "startedAt": 2,
            "toolDefinitions": [{
                "name": "run_command",
                "description": "Run a managed command",
                "inputSchema": {"type": "object"},
                "safety": "requires_approval",
                "requiresWorkspace": true,
                "requiresApproval": true,
                "approvalMode": "always"
            }],
            "toolCalls": [{
                "id": "command-call",
                "tool": "run_command",
                "args": {"command": "true"},
                "approvalStatus": "approved",
                "reason": null
            }],
            "toolResults": [],
            "approvals": [{
                "type": "command",
                "command": {
                    "id": "command-call",
                    "command": "true",
                    "cwd": null,
                    "timeoutMs": null,
                    "approvalStatus": "approved",
                    "riskLevel": "read_only",
                    "reason": null,
                    "observe": null
                }
            }],
            "diffs": [],
            "fileDrafts": [],
            "webSearchActivities": [{
                "callId": "web-call",
                "query": "current query",
                "provider": "test",
                "status": "completed",
                "sources": [{
                    "id": "source-1",
                    "title": "Current source",
                    "url": "https://example.test/source",
                    "displayUrl": "example.test/source",
                    "domain": "example.test"
                }],
                "updatedAt": 3
            }],
            "readActivities": [],
            "mcpInvocations": [],
            "messageStreamCheckpoints": {
                "stream-1": {"baseContentLength": 3, "baseWasThinking": false}
            },
            "timeline": [{"id": "message-1", "type": "message", "content": "kept"}],
            "todo": {
                "revision": 1,
                "items": [{
                    "id": "todo-1",
                    "title": "finish",
                    "status": "completed",
                    "createdAt": 2,
                    "updatedAt": 3
                }],
                "updatedAt": 3
            },
            "skillInstallations": [{
                "action": {
                    "schemaVersion": 1,
                    "id": "install-1",
                    "installRef": "opaque-install-ref",
                    "preview": {
                        "name": "Example",
                        "description": "Example Skill",
                        "sourceSummary": {},
                        "resolvedRevision": "revision-1",
                        "fileCount": 1,
                        "totalBytes": 10,
                        "resourceSummary": {
                            "total": 1,
                            "references": 0,
                            "assets": 0,
                            "scripts": 0,
                            "bytes": 10
                        },
                        "containsScripts": false,
                        "warnings": [],
                        "compatibility": "compatible",
                        "operation": "install",
                        "impact": "Adds one Skill"
                    },
                    "approvalStatus": "approved",
                    "expiresAt": 100
                },
                "status": "installed"
            }],
            "commandSessions": {
                "command-call": {
                    "callId": "command-call",
                    "status": "exited",
                    "startedAt": 2,
                    "endedAt": 3,
                    "exitCode": 0,
                    "latestSequence": 1,
                    "outputTruncated": false
                }
            },
            "activatedSkills": [{
                "id": "workspace:example",
                "name": "Example",
                "revision": "revision-1",
                "source": {"kind": "workspace", "id": "workspace:example"}
            }],
            "explicitSkillSelections": [{
                "id": "workspace:example",
                "revision": "revision-1"
            }],
            "state": {
                "status": "running",
                "activeRunId": "run-1",
                "lastError": null,
                "updatedAt": 2
            }
        });
        let valid_presentation = current_with_presentation.clone();
        let recovered = canonical_agent_run_lifecycle_projection(
            Some(&current_with_presentation.to_string()),
            "run-1",
            "failed",
            2,
            30,
            Some(30),
        )
        .unwrap();
        let recovered: serde_json::Value = serde_json::from_str(&recovered).unwrap();
        assert_eq!(recovered["timeline"][0]["content"], "kept");
        assert_eq!(
            recovered["webSearchActivities"][0]["sources"][0]["id"],
            "source-1"
        );
        assert_eq!(recovered["todo"]["revision"], 1);
        assert_eq!(recovered["skillInstallations"][0]["status"], "installed");
        assert_eq!(recovered["approvals"][0]["type"], "command");
        assert_eq!(
            recovered["commandSessions"]["command-call"]["status"],
            "exited"
        );
        assert_eq!(recovered["activatedSkills"][0]["name"], "Example");
        assert_eq!(
            recovered["explicitSkillSelections"][0]["revision"],
            "revision-1"
        );

        let invalid_projections = [
            {
                let mut invalid = valid_presentation.clone();
                invalid["toolDefinitions"][0]
                    .as_object_mut()
                    .unwrap()
                    .remove("inputSchema");
                invalid
            },
            {
                let mut invalid = valid_presentation.clone();
                invalid["timeline"][0]["privateField"] = true.into();
                invalid
            },
            {
                let mut invalid = valid_presentation.clone();
                invalid["messageStreamCheckpoints"]["stream-1"]["privateField"] = true.into();
                invalid
            },
            {
                let mut invalid = valid_presentation.clone();
                invalid["toolCalls"][0]
                    .as_object_mut()
                    .unwrap()
                    .remove("reason");
                invalid
            },
            {
                let mut invalid = valid_presentation;
                invalid["webSearchActivities"][0]["sources"][0]["privateField"] = true.into();
                invalid
            },
            {
                let mut invalid = current_with_presentation;
                invalid["approvals"][0]["command"]["runtimeBinding"] = serde_json::json!({});
                invalid["approvals"][0]["command"]["inputs"] = serde_json::json!([]);
                invalid
            },
        ];
        for invalid in invalid_projections {
            let recovered = canonical_agent_run_lifecycle_projection(
                Some(&invalid.to_string()),
                "run-1",
                "failed",
                2,
                40,
                Some(40),
            )
            .unwrap();
            let recovered: serde_json::Value = serde_json::from_str(&recovered).unwrap();
            assert_eq!(recovered["toolDefinitions"], serde_json::json!([]));
            assert_eq!(recovered["timeline"], serde_json::json!([]));
            assert_eq!(recovered["webSearchActivities"], serde_json::json!([]));
            assert_eq!(recovered["messageStreamCheckpoints"], serde_json::json!({}));
        }

        assert!(!current_uuid_is_safe(
            "00000000-0000-0000-0000-000000000000"
        ));
        assert!(!current_uuid_is_safe(
            "ffffffff-ffff-4fff-0fff-ffffffffffff"
        ));
        assert!(current_uuid_is_safe("550e8400-e29b-41d4-a716-446655440000"));
    }

    #[test]
    fn canonical_recovery_preserves_the_current_pre_start_projection() {
        let pre_start = serde_json::json!({
            "runId": null,
            "status": "starting",
            "startedAt": 2,
            "toolDefinitions": [],
            "toolCalls": [],
            "toolResults": [],
            "approvals": [],
            "diffs": [],
            "fileDrafts": [],
            "webSearchActivities": [],
            "readActivities": [],
            "mcpInvocations": [],
            "messageStreamCheckpoints": {},
            "timeline": [{"id": "message-1", "type": "message", "content": "kept"}]
        });

        let recovered = canonical_agent_run_lifecycle_projection(
            Some(&pre_start.to_string()),
            "run-1",
            "running",
            2,
            3,
            None,
        )
        .unwrap();
        let recovered: serde_json::Value = serde_json::from_str(&recovered).unwrap();
        assert_eq!(recovered["runId"], "run-1");
        assert_eq!(recovered["status"], "running");
        assert_eq!(recovered["timeline"][0]["content"], "kept");
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
