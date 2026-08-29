mod agent_run_projection;

pub(crate) use agent_run_projection::{
    canonical_agent_run_lifecycle_projection, current_agent_run_projection_is_safe,
    current_agent_run_projection_is_safe_for_trace_rebuild,
};
#[cfg(test)]
use agent_run_projection::{
    current_persisted_approval_is_safe, current_uuid_is_safe, MAX_JS_SAFE_INTEGER,
};

use crate::storage::models::{
    ChatConversationMetaRecord, ChatConversationRecord, ChatMessageRecord, ChatMessageStateRecord,
};
use crate::storage::{
    context_compaction_repository, now_ms, provider_continuation_repository, world_state_repository,
};
use crate::{AgentMcpToolApproval, AgentMcpToolInvocationEvent};
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

/// Atomically projects an approved Skill script back into its exact live Assistant run.
///
/// This is intentionally strict: an approval decision may resume only the pending Assistant
/// message that still owns the same run and the same frozen Skill ToolCall. The existing Timeline
/// and every unrelated activity are preserved byte-for-byte at the JSON value level; only the
/// run lifecycle and the matching Skill approval fields advance.
pub(crate) fn update_message_run_running_state(
    connection: &Connection,
    conversation_id: &str,
    message_id: &str,
    run_id: &str,
    tool_call_id: &str,
    updated_at: i64,
) -> rusqlite::Result<()> {
    let Some((existing_agent_run_json, started_at, role, message_status)) = connection
        .query_row(
            "SELECT agent_run_json, created_at, role, status
             FROM messages
             WHERE conversation_id = ?1 AND id = ?2",
            params![conversation_id, message_id],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            },
        )
        .optional()?
    else {
        return Err(rusqlite::Error::InvalidQuery);
    };
    if role != "assistant" || message_status.as_deref() != Some("pending") {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let existing_agent_run_json = existing_agent_run_json.ok_or(rusqlite::Error::InvalidQuery)?;
    let mut value = serde_json::from_str::<serde_json::Value>(&existing_agent_run_json)
        .map_err(|_| rusqlite::Error::InvalidQuery)?;
    let run = value.as_object_mut().ok_or(rusqlite::Error::InvalidQuery)?;
    if !current_agent_run_projection_is_safe(run, run_id)
        || run.get("status").and_then(serde_json::Value::as_str) != Some("waiting_for_approval")
        || run
            .get("state")
            .and_then(serde_json::Value::as_object)
            .and_then(|state| state.get("status"))
            .and_then(serde_json::Value::as_str)
            != Some("waiting_for_approval")
        || run
            .get("state")
            .and_then(serde_json::Value::as_object)
            .and_then(|state| state.get("activeRunId"))
            .and_then(serde_json::Value::as_str)
            != Some(run_id)
    {
        return Err(rusqlite::Error::InvalidQuery);
    }

    let mut matching_tool_calls = 0usize;
    for call in run
        .get_mut("toolCalls")
        .and_then(serde_json::Value::as_array_mut)
        .ok_or(rusqlite::Error::InvalidQuery)?
    {
        if call.get("id").and_then(serde_json::Value::as_str) != Some(tool_call_id) {
            continue;
        }
        if call.get("tool").and_then(serde_json::Value::as_str) != Some("skills_run_script")
            || call
                .get("approvalStatus")
                .and_then(serde_json::Value::as_str)
                != Some("required")
        {
            return Err(rusqlite::Error::InvalidQuery);
        }
        call["approvalStatus"] = serde_json::Value::String("approved".to_string());
        matching_tool_calls += 1;
    }

    let mut matching_skill_approvals = 0usize;
    for approval in run
        .get_mut("approvals")
        .and_then(serde_json::Value::as_array_mut)
        .ok_or(rusqlite::Error::InvalidQuery)?
    {
        if approval.get("type").and_then(serde_json::Value::as_str) != Some("skill_script")
            || approval
                .get("script")
                .and_then(|script| script.get("id"))
                .and_then(serde_json::Value::as_str)
                != Some(tool_call_id)
        {
            continue;
        }
        if approval
            .get("script")
            .and_then(|script| script.get("approvalStatus"))
            .and_then(serde_json::Value::as_str)
            != Some("required")
        {
            return Err(rusqlite::Error::InvalidQuery);
        }
        approval["script"]["approvalStatus"] = serde_json::Value::String("approved".to_string());
        matching_skill_approvals += 1;
    }
    if matching_tool_calls != 1 || matching_skill_approvals != 1 {
        return Err(rusqlite::Error::InvalidQuery);
    }

    let approved_waiting_json = serde_json::to_string(&value)
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
    let next_agent_run_json = canonical_agent_run_lifecycle_projection(
        Some(&approved_waiting_json),
        run_id,
        "running",
        started_at,
        updated_at,
        None,
    )?;
    let changed = connection.execute(
        "UPDATE messages
         SET agent_run_json = ?1
         WHERE conversation_id = ?2
           AND id = ?3
           AND role = 'assistant'
           AND status = 'pending'
           AND agent_run_json = ?4",
        params![
            next_agent_run_json,
            conversation_id,
            message_id,
            existing_agent_run_json,
        ],
    )?;
    if changed != 1 {
        return Err(rusqlite::Error::InvalidQuery);
    }
    Ok(())
}

pub fn update_message_state(
    connection: &Connection,
    conversation_id: &str,
    message: &ChatMessageStateRecord,
) -> rusqlite::Result<()> {
    let existing = connection
        .query_row(
            "SELECT status, content, agent_run_json, created_at
             FROM messages
             WHERE conversation_id = ?1 AND id = ?2",
            params![conversation_id, &message.id],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            },
        )
        .optional()?;
    let Some((_existing_status, existing_content, existing_run_json, created_at)) = existing else {
        return Ok(());
    };
    let durable_terminal = durable_terminal_run(
        connection,
        conversation_id,
        &message.id,
        existing_run_json.as_deref(),
    )?;
    let authoritative_usage =
        get_authoritative_message_usage(connection, conversation_id, &message.id)?;

    let (content, status, agent_run_json) = if let Some(terminal) = durable_terminal {
        let incoming_run_status = run_status_from_json(message.agent_run_json.as_deref());
        let incoming_reopens_run = incoming_run_status
            .as_deref()
            .is_some_and(is_live_run_status);
        if incoming_reopens_run || message.status.as_deref() == Some("pending") {
            let next_status = Some(message_status_for_terminal_run(&terminal.status).to_string());
            let next_content = content_for_terminal_write(
                &existing_content,
                &message.content,
                incoming_reopens_run,
            );
            let base_json = if incoming_reopens_run {
                existing_run_json
                    .as_deref()
                    .or(message.agent_run_json.as_deref())
            } else {
                message
                    .agent_run_json
                    .as_deref()
                    .or(existing_run_json.as_deref())
            };
            let stamped = stamp_terminal_agent_run_json(base_json, &terminal, created_at)?;
            (
                next_content,
                next_status,
                overlay_authoritative_usage(stamped, authoritative_usage.as_ref()),
            )
        } else {
            (
                message.content.clone(),
                message.status.clone(),
                overlay_authoritative_usage(
                    message.agent_run_json.clone(),
                    authoritative_usage.as_ref(),
                ),
            )
        }
    } else {
        (
            message.content.clone(),
            message.status.clone(),
            overlay_authoritative_usage(
                message.agent_run_json.clone(),
                authoritative_usage.as_ref(),
            ),
        )
    };

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
            &content,
            &status,
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
            usage.billable_request_count,
            trace.terminal_status,
            trace.completed_at,
            trace.run_id
        FROM messages AS message
        LEFT JOIN agent_usage_records AS usage
          ON usage.conversation_id = message.conversation_id
         AND usage.message_id = message.id
        LEFT JOIN conversation_turn_traces AS trace
          ON trace.conversation_id = message.conversation_id
         AND trace.assistant_message_id = message.id
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
            let created_at = row.get::<_, i64>(3)?;
            let content = row.get::<_, String>(2)?;
            let status = row.get::<_, Option<String>>(4)?;
            let agent_run_json =
                overlay_authoritative_usage(row.get(5)?, authoritative_usage.as_ref());
            let (content, status, agent_run_json) = overlay_loaded_message_with_terminal_trace(
                content,
                status,
                agent_run_json,
                created_at,
                row.get::<_, Option<String>>(17)?,
                row.get::<_, Option<String>>(15)?,
                row.get::<_, Option<i64>>(16)?,
            );
            Ok(ChatMessageRecord {
                id: row.get(0)?,
                role: row.get(1)?,
                content,
                created_at,
                status,
                attachments: Vec::new(),
                agent_run_json,
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

struct DurableTerminalRun {
    run_id: String,
    status: String,
    completed_at: i64,
}

fn is_terminal_run_status(status: &str) -> bool {
    matches!(status, "completed" | "failed" | "cancelled")
}

fn is_live_run_status(status: &str) -> bool {
    matches!(
        status,
        "starting" | "queued" | "running" | "waiting_for_approval" | "idle"
    )
}

fn message_status_for_terminal_run(status: &str) -> &'static str {
    if status == "failed" {
        "error"
    } else {
        "sent"
    }
}

fn run_status_from_json(raw: Option<&str>) -> Option<String> {
    raw.and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
        .and_then(|value| value.get("status")?.as_str().map(str::to_string))
}

fn content_for_terminal_write(
    existing_content: &str,
    incoming_content: &str,
    incoming_reopens_run: bool,
) -> String {
    if incoming_reopens_run {
        return existing_content.to_string();
    }
    incoming_content.to_string()
}

fn stamp_terminal_agent_run_json(
    base_json: Option<&str>,
    terminal: &DurableTerminalRun,
    started_at: i64,
) -> rusqlite::Result<Option<String>> {
    canonical_agent_run_lifecycle_projection(
        base_json,
        &terminal.run_id,
        &terminal.status,
        started_at,
        terminal.completed_at,
        Some(terminal.completed_at),
    )
    .map(Some)
}

fn durable_terminal_run(
    connection: &Connection,
    conversation_id: &str,
    message_id: &str,
    existing_agent_run_json: Option<&str>,
) -> rusqlite::Result<Option<DurableTerminalRun>> {
    let traced = connection
        .query_row(
            "SELECT run_id, terminal_status, completed_at
             FROM conversation_turn_traces
             WHERE conversation_id = ?1 AND assistant_message_id = ?2",
            params![conversation_id, message_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                ))
            },
        )
        .optional()?;
    if let Some((run_id, terminal_status, completed_at)) = traced {
        if is_terminal_run_status(&terminal_status) {
            return Ok(Some(DurableTerminalRun {
                run_id,
                status: terminal_status,
                completed_at: completed_at.unwrap_or_else(now_ms),
            }));
        }
    }

    let Some(raw) = existing_agent_run_json else {
        return Ok(None);
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
        return Ok(None);
    };
    let Some(status) = value.get("status").and_then(serde_json::Value::as_str) else {
        return Ok(None);
    };
    if !is_terminal_run_status(status) {
        return Ok(None);
    }
    let Some(run_id) = value.get("runId").and_then(serde_json::Value::as_str) else {
        return Ok(None);
    };
    if run_id.is_empty() {
        return Ok(None);
    }
    Ok(Some(DurableTerminalRun {
        run_id: run_id.to_string(),
        status: status.to_string(),
        completed_at: value
            .get("completedAt")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or_else(now_ms),
    }))
}

fn overlay_loaded_message_with_terminal_trace(
    content: String,
    status: Option<String>,
    agent_run_json: Option<String>,
    created_at: i64,
    trace_run_id: Option<String>,
    trace_terminal_status: Option<String>,
    trace_completed_at: Option<i64>,
) -> (String, Option<String>, Option<String>) {
    let Some(trace_status) = trace_terminal_status.filter(|status| is_terminal_run_status(status))
    else {
        return (content, status, agent_run_json);
    };
    let Some(run_id) = trace_run_id.filter(|run_id| !run_id.is_empty()) else {
        return (content, status, agent_run_json);
    };
    let run_json_status = run_status_from_json(agent_run_json.as_deref());
    let needs_repair = status.as_deref() == Some("pending")
        || run_json_status.as_deref().is_some_and(is_live_run_status);
    if !needs_repair {
        return (content, status, agent_run_json);
    }
    let terminal = DurableTerminalRun {
        run_id,
        status: trace_status,
        completed_at: trace_completed_at.unwrap_or(created_at),
    };
    let next_status = Some(message_status_for_terminal_run(&terminal.status).to_string());
    let next_json = stamp_terminal_agent_run_json(agent_run_json.as_deref(), &terminal, created_at)
        .ok()
        .flatten()
        .or(agent_run_json);
    (content, next_status, next_json)
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
mod tests;
