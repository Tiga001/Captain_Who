//! Model-only views of immutable Agent-authored Conversation inputs. Mailbox payloads and their
//! Conversation copies remain byte-identical durable facts; typed Result identities are hidden
//! only at model/history read boundaries, including inherited snapshot messages.

use super::agent_graph_repository;
use crate::{AgentMailboxKind, ConversationMessageOrigin};
use rusqlite::{params, Connection, OptionalExtension};

pub(crate) fn project_history_preview(
    connection: &Connection,
    conversation_id: &str,
    reference: &super::conversation_history_repository::ConversationHistoryRecordRef,
    preview: &mut String,
    truncated: &mut bool,
) -> Result<(), String> {
    let super::conversation_history_repository::ConversationHistoryRecordRef::Message {
        message_id,
    } = reference
    else {
        return Ok(());
    };
    if let Some(content) = project_message(connection, conversation_id, message_id)? {
        const PREVIEW_CHARS: usize = 600;
        *truncated = content.chars().count() > PREVIEW_CHARS;
        *preview = content.chars().take(PREVIEW_CHARS).collect();
    }
    Ok(())
}

pub(crate) fn project_history_record(
    connection: &Connection,
    conversation_id: &str,
    record: &mut super::conversation_history_repository::ConversationHistoryRecord,
) -> Result<(), String> {
    let super::conversation_history_repository::ConversationHistoryRecordRef::Message {
        message_id,
    } = &record.reference
    else {
        return Ok(());
    };
    if let Some(content) = project_message(connection, conversation_id, message_id)? {
        let mut value: serde_json::Value = serde_json::from_str(&record.serialized_json)
            .map_err(|_| "Cannot read the collaboration history record.".to_string())?;
        value["content"] = content.into();
        record.serialized_json = serde_json::to_string(&value)
            .map_err(|_| "Cannot project the collaboration history record.".to_string())?;
    }
    Ok(())
}

pub(crate) fn project_message(
    connection: &Connection,
    conversation_id: &str,
    message_id: &str,
) -> Result<Option<String>, String> {
    let stored: Option<(String, String)> = connection
        .query_row(
            "SELECT role, content FROM messages WHERE conversation_id = ?1 AND id = ?2",
            params![conversation_id, message_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|_| "Cannot read the collaboration message model view.".to_string())?;
    let Some((role, content)) = stored else {
        // Unsaved human input in a context preview has no collaboration origin.
        return Ok(None);
    };
    if role != "user" {
        return Ok(None);
    }
    let origin = agent_graph_repository::conversation_message_origin(
        connection,
        conversation_id,
        message_id,
    )
    .map_err(|_| "Cannot validate the collaboration message origin.".to_string())?;
    let is_snapshot = matches!(
        &origin,
        ConversationMessageOrigin::HistoricalSnapshot { .. }
    );
    let mut original = &origin;
    while let ConversationMessageOrigin::HistoricalSnapshot {
        original: source, ..
    } = original
    {
        original = source;
    }
    let ConversationMessageOrigin::Agent {
        sender_agent_id,
        source_agent_message_id,
    } = original
    else {
        return Ok(None);
    };
    if is_snapshot {
        return project_snapshot_result(sender_agent_id, source_agent_message_id, &content);
    }
    let mailbox = agent_graph_repository::get_agent_message(connection, source_agent_message_id)
        .map_err(|_| "Cannot read the collaboration message source.".to_string())?
        .ok_or_else(|| "The collaboration message source is missing.".to_string())?;
    if mailbox.sender_agent_id != *sender_agent_id || mailbox.content != content {
        return Err("The collaboration message does not match its immutable source.".to_string());
    }
    if mailbox.kind != AgentMailboxKind::Result {
        return Ok(None);
    }
    let sender = agent_graph_repository::get_agent_node(connection, sender_agent_id)
        .map_err(|_| "Cannot read the collaboration result sender.".to_string())?
        .ok_or_else(|| "The collaboration result sender is missing.".to_string())?;
    crate::conversation_trace::project_agent_mailbox_model_envelope(
        &sender.agent_id,
        &sender.task_name,
        &sender.task_path,
        mailbox.kind,
        &mailbox.content,
    )
    .map(|(content, _)| Some(content))
}

/// Snapshot provenance is immutable and survives deletion of the original tree. Production
/// Result receipts have a dedicated, deterministic Host namespace, while ordinary Agent messages
/// use `mailbox-...`; use both that frozen receipt and the typed payload, never JSON-shaped prose.
pub(crate) fn project_snapshot_result(
    sender_agent_id: &str,
    source_agent_message_id: &str,
    content: &str,
) -> Result<Option<String>, String> {
    if !source_agent_message_id.starts_with("mailbox-result-") {
        return Ok(None);
    }
    let result: crate::AgentTurnResultEnvelope = serde_json::from_str(content)
        .map_err(|_| "The frozen collaboration Result is invalid.".to_string())?;
    if result.child_agent_id != sender_agent_id
        || source_agent_message_id
            != agent_graph_repository::stable_fact_id("mailbox-result", &[&result.wake_id])
    {
        return Err("The frozen collaboration Result does not match its Host origin.".to_string());
    }
    crate::conversation_trace::project_agent_mailbox_model_envelope(
        sender_agent_id,
        &result.task_name,
        &result.task_path,
        AgentMailboxKind::Result,
        content,
    )
    .map(|(content, _)| Some(content))
}
