use crate::storage::models::{AttachmentRecord, ChatMessageRecord};
use crate::storage::{
    attachment_repository, chat_repository, context_compaction_repository,
    conversation_history_archive_repository, conversation_model_context_repository,
    conversation_trace_repository, managed_artifact_repository, turn_diff_repository,
};
use crate::{
    AgentForkTurns, ConversationMessageOrigin, ConversationModelContextItem, ConversationTurnTrace,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

use super::conversation_fork_repository::{ConversationForkError, ForkAttachmentCopy};

const SNAPSHOT_SCHEMA_VERSION: u32 = 1;

#[derive(Debug)]
struct SnapshotTrace {
    trace: ConversationTurnTrace,
    model_context_items: Vec<ConversationModelContextItem>,
    created_at: i64,
    committed_at: i64,
}

#[derive(Debug)]
struct SnapshotMessage {
    record: ChatMessageRecord,
    source_message_id: String,
    original_origin: Option<ConversationMessageOrigin>,
}

#[derive(Debug, Clone)]
struct SnapshotArtifactGrant {
    artifact_id: String,
    target_run_id: String,
    call_id: String,
    created_at: i64,
}

/// Complete creation-time child history snapshot. Building is read-only; applying uses the
/// caller's existing transaction and never begins or commits one.
#[derive(Debug)]
pub(crate) struct ChildContextSnapshotPlan {
    source_conversation_id: String,
    target_conversation_id: String,
    fork_turns: AgentForkTurns,
    selected_turn_count: usize,
    messages: Vec<SnapshotMessage>,
    traces: Vec<SnapshotTrace>,
    archives: Vec<conversation_history_archive_repository::ConversationHistoryArchiveForkCopy>,
    turn_diffs: Vec<turn_diff_repository::AgentTurnDiffForkCopy>,
    artifact_grants: Vec<SnapshotArtifactGrant>,
    summaries: Vec<context_compaction_repository::ContextCompactionSummaryVersion>,
    message_id_map: HashMap<String, String>,
    id_replacements: HashMap<String, String>,
    pub(crate) attachments: Vec<ForkAttachmentCopy>,
    created_at: i64,
}

/// Builds a collaboration-owned snapshot of complete settled logical turns.
///
/// Candidate logical turns use the same settled assistant boundary as the existing Conversation
/// fork: non-pending, no active Run state, and a terminal trace when a trace exists. Collaboration
/// snapshots are stricter at selection time: every selected assistant must have its durable trace
/// so the child can immediately enter the shared ContextAssembler integrity boundary. Any tail
/// after the last boundary (including an in-progress Turn's user message) is excluded.
pub(crate) fn build_child_context_snapshot_plan(
    connection: &Connection,
    source_conversation_id: &str,
    target_conversation_id: &str,
    fork_turns: &AgentForkTurns,
    created_at: i64,
) -> Result<ChildContextSnapshotPlan, ConversationForkError> {
    fork_turns
        .validate()
        .map_err(|error| ConversationForkError::Other(error.to_string()))?;
    validate_id("source conversation", source_conversation_id)?;
    validate_id("target conversation", target_conversation_id)?;
    if source_conversation_id == target_conversation_id || created_at < 0 {
        return Err(ConversationForkError::Other(
            "child context snapshot has an invalid identity or timestamp".to_string(),
        ));
    }
    let source = chat_repository::get_active_conversation(connection, source_conversation_id)
        .map_err(database_error)?
        .ok_or_else(|| ConversationForkError::Other("source conversation does not exist".into()))?;

    let mut closed_turns: Vec<(Vec<ChatMessageRecord>, Option<String>)> = Vec::new();
    if !matches!(fork_turns, AgentForkTurns::None) {
        let mut open = Vec::new();
        for message in source.messages {
            open.push(message.clone());
            if message.role != "assistant" {
                continue;
            }
            let trace =
                conversation_trace_repository::get_trace_for_message(connection, &message.id)
                    .map_err(database_error)?;
            let trace_is_settled = trace
                .as_ref()
                .is_none_or(|trace| trace.terminal_status.is_terminal());
            let run_terminal =
                agent_run_status(message.agent_run_json.as_deref())?.is_none_or(|status| {
                    matches!(
                        status.as_str(),
                        "idle" | "completed" | "failed" | "cancelled"
                    )
                });
            if trace_is_settled && message.status.as_deref() != Some("pending") && run_terminal {
                let missing_trace = trace.is_none().then(|| message.id.clone());
                closed_turns.push((std::mem::take(&mut open), missing_trace));
            }
        }
    }

    let selected_turns = match *fork_turns {
        AgentForkTurns::None => Vec::new(),
        AgentForkTurns::All => closed_turns,
        AgentForkTurns::Last(count) => {
            let start = closed_turns.len().saturating_sub(count as usize);
            closed_turns.split_off(start)
        }
    };
    let selected_turn_count = selected_turns.len();
    if let Some(message_id) = selected_turns
        .iter()
        .find_map(|(_, missing_trace)| missing_trace.as_deref())
    {
        return Err(ConversationForkError::Other(format!(
            "selected settled assistant `{message_id}` is missing its durable trace and cannot enter a collaboration context snapshot"
        )));
    }
    let source_messages = selected_turns
        .into_iter()
        .flat_map(|(messages, _)| messages)
        .collect::<Vec<_>>();
    let source_message_ids = source_messages
        .iter()
        .map(|message| message.id.clone())
        .collect::<Vec<_>>();
    let message_id_map = source_messages
        .iter()
        .map(|message| (message.id.clone(), new_id("message")))
        .collect::<HashMap<_, _>>();

    let source_attachments = attachment_repository::list_message_attachments_for_fork(
        connection,
        source_conversation_id,
        &source_message_ids,
    )
    .map_err(database_error)?;
    let attachment_id_map = source_attachments
        .iter()
        .map(|attachment| (attachment.id.clone(), new_id("attachment")))
        .collect::<HashMap<_, _>>();

    let mut traces = Vec::new();
    let mut run_id_map = HashMap::new();
    for message in source_messages
        .iter()
        .filter(|message| message.role == "assistant")
    {
        let source_trace =
            conversation_trace_repository::get_trace_for_message(connection, &message.id)
                .map_err(database_error)?
                .ok_or_else(|| {
                    ConversationForkError::Other(format!(
                        "selected settled assistant `{}` is missing its durable trace",
                        message.id
                    ))
                })?;
        if !source_trace.terminal_status.is_terminal() {
            return Err(ConversationForkError::Other(
                "in-progress traces cannot enter a child context snapshot".to_string(),
            ));
        }
        let target_run_id = new_id("run");
        run_id_map.insert(source_trace.run_id.clone(), target_run_id.clone());
        let (trace_created_at, committed_at) = trace_times(connection, &message.id)?;
        let model_context_items =
            conversation_model_context_repository::get_log_for_message(connection, &message.id)
                .map_err(database_error)?
                .map(|log| log.items)
                .unwrap_or_default();
        source_trace
            .validate_complete_model_context(&model_context_items)
            .map_err(|error| {
                ConversationForkError::Other(format!(
                    "selected settled assistant `{}` has incomplete model context: {error}",
                    message.id
                ))
            })?;
        traces.push(SnapshotTrace {
            trace: ConversationTurnTrace {
                schema_version: source_trace.schema_version,
                run_id: target_run_id,
                conversation_id: target_conversation_id.to_string(),
                assistant_message_id: mapped(&message_id_map, &message.id, "assistant message")?,
                terminal_status: source_trace.terminal_status,
                terminal_error: source_trace.terminal_error,
                truncated: source_trace.truncated,
                items: source_trace.items,
            },
            model_context_items,
            created_at: trace_created_at,
            committed_at,
        });
    }

    let mut archive_id_map = HashMap::new();
    let mut archives = Vec::new();
    for trace in &traces {
        for item in &trace.trace.items {
            let archive = match item {
                crate::ConversationTurnTraceItem::ToolResult { archive, .. }
                | crate::ConversationTurnTraceItem::CommandSessionLifecycle { archive, .. } => {
                    archive
                }
                _ => continue,
            };
            let Some(source_archive_ref) = archive.archive_ref.as_deref() else {
                continue;
            };
            if archive_id_map.contains_key(source_archive_ref) {
                continue;
            }
            let copy = conversation_history_archive_repository::load_fork_copy(
                connection,
                source_conversation_id,
                source_archive_ref,
                target_conversation_id,
                &trace.trace.assistant_message_id,
            )
            .map_err(database_error)?;
            archive_id_map.insert(
                source_archive_ref.to_string(),
                copy.target_archive_ref.clone(),
            );
            archives.push(copy);
        }
    }

    let mut replacements = message_id_map.clone();
    replacements.extend(run_id_map.clone());
    replacements.extend(attachment_id_map.clone());
    replacements.extend(archive_id_map);
    replacements.insert(
        source_conversation_id.to_string(),
        target_conversation_id.to_string(),
    );
    for trace in &mut traces {
        rewrite_json_ids(&mut trace.trace.items, &replacements)?;
        rewrite_json_ids(&mut trace.model_context_items, &replacements)?;
        trace.trace.validate().map_err(|error| {
            ConversationForkError::Other(format!("rewritten child trace is invalid: {error}"))
        })?;
    }

    let source_origins =
        load_source_origins(connection, source_conversation_id, &source_message_ids)?;
    let messages = source_messages
        .iter()
        .map(|source_message| {
            Ok(SnapshotMessage {
                record: ChatMessageRecord {
                    human_interaction_response: source_message.human_interaction_response.clone(),
                    id: mapped(&message_id_map, &source_message.id, "message")?,
                    role: source_message.role.clone(),
                    content: source_message.content.clone(),
                    created_at: source_message.created_at,
                    status: source_message.status.clone(),
                    attachments: Vec::new(),
                    // The terminal trace is the durable execution fact. Mutable run projections
                    // and frontend-owned UI state are deliberately not inherited.
                    agent_run_json: None,
                    ui_state_json: None,
                },
                source_message_id: source_message.id.clone(),
                original_origin: source_origins.get(&source_message.id).cloned().flatten(),
            })
        })
        .collect::<Result<Vec<_>, ConversationForkError>>()?;

    let attachments = source_attachments
        .into_iter()
        .map(|source_attachment| {
            Ok(ForkAttachmentCopy {
                target: AttachmentRecord {
                    id: mapped(&attachment_id_map, &source_attachment.id, "attachment")?,
                    conversation_id: target_conversation_id.to_string(),
                    message_id: mapped(
                        &message_id_map,
                        &source_attachment.message_id,
                        "attachment message",
                    )?,
                    project_id: source.project_id.clone(),
                    kind: source_attachment.kind.clone(),
                    original_name: source_attachment.original_name.clone(),
                    mime_type: source_attachment.mime_type.clone(),
                    size_bytes: source_attachment.size_bytes,
                    storage_rel_path: String::new(),
                    created_at: source_attachment.created_at,
                },
                source: source_attachment,
            })
        })
        .collect::<Result<Vec<_>, ConversationForkError>>()?;

    let mut turn_diffs = Vec::new();
    for assistant in source_messages
        .iter()
        .filter(|message| message.role == "assistant")
    {
        for mut copy in turn_diff_repository::list_fork_copies_through_message(
            connection,
            source_conversation_id,
            &assistant.id,
        )
        .map_err(database_error)?
        {
            if !source_message_ids.contains(&copy.record.identity.assistant_message_id)
                || turn_diffs.iter().any(
                    |existing: &turn_diff_repository::AgentTurnDiffForkCopy| {
                        existing.record.identity.assistant_message_id
                            == copy.record.identity.assistant_message_id
                    },
                )
            {
                continue;
            }
            copy.record.identity.conversation_id = target_conversation_id.to_string();
            copy.record.identity.assistant_message_id = mapped(
                &message_id_map,
                &copy.record.identity.assistant_message_id,
                "turn diff message",
            )?;
            copy.record.identity.run_id =
                mapped(&run_id_map, &copy.record.identity.run_id, "turn diff run")?;
            turn_diffs.push(copy);
        }
    }

    let selected_source_run_ids = run_id_map.keys().cloned().collect::<HashSet<_>>();
    let artifact_grants = load_artifact_grants(
        connection,
        source_conversation_id,
        target_conversation_id,
        &selected_source_run_ids,
        &run_id_map,
    )?;
    let summaries = if matches!(fork_turns, AgentForkTurns::All) {
        context_compaction_repository::list_active_summary_chain(connection, source_conversation_id)
            .map_err(|error| ConversationForkError::Other(error.to_string()))?
            .into_iter()
            .take_while(|version| {
                source_message_ids.contains(&version.lineage.introduced_by_assistant_message_id)
            })
            .collect()
    } else {
        Vec::new()
    };

    Ok(ChildContextSnapshotPlan {
        source_conversation_id: source_conversation_id.to_string(),
        target_conversation_id: target_conversation_id.to_string(),
        fork_turns: *fork_turns,
        selected_turn_count,
        messages,
        traces,
        archives,
        turn_diffs,
        artifact_grants,
        summaries,
        message_id_map,
        id_replacements: replacements,
        attachments,
        created_at,
    })
}

pub(crate) fn apply_child_context_snapshot_in_transaction(
    connection: &Connection,
    plan: &ChildContextSnapshotPlan,
) -> Result<(), ConversationForkError> {
    let (fork_kind, fork_turn_count) = encode_fork_turns(plan.fork_turns);
    connection
        .execute(
            "INSERT INTO child_context_snapshots (
                target_conversation_id, schema_version, source_conversation_id, fork_kind,
                fork_turn_count, selected_turn_count, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                &plan.target_conversation_id,
                SNAPSHOT_SCHEMA_VERSION,
                &plan.source_conversation_id,
                fork_kind,
                fork_turn_count,
                plan.selected_turn_count as i64,
                plan.created_at,
            ],
        )
        .map_err(database_error)?;
    for (position, message) in plan.messages.iter().enumerate() {
        let (original_kind, original_agent_id, original_mailbox_id) = match &message.original_origin
        {
            Some(ConversationMessageOrigin::Human) => (Some("human"), None, None),
            Some(ConversationMessageOrigin::Agent {
                sender_agent_id,
                source_agent_message_id,
            }) => (
                Some("agent"),
                Some(sender_agent_id.as_str()),
                Some(source_agent_message_id.as_str()),
            ),
            Some(ConversationMessageOrigin::HistoricalSnapshot { original, .. }) => {
                match original.as_ref() {
                    ConversationMessageOrigin::Human => (Some("human"), None, None),
                    ConversationMessageOrigin::Agent {
                        sender_agent_id,
                        source_agent_message_id,
                    } => (
                        Some("agent"),
                        Some(sender_agent_id.as_str()),
                        Some(source_agent_message_id.as_str()),
                    ),
                    ConversationMessageOrigin::HistoricalSnapshot { .. } => {
                        return Err(ConversationForkError::Other(
                            "nested snapshot origin is corrupt".into(),
                        ));
                    }
                }
            }
            None => (None, None, None),
        };
        connection
            .execute(
                "INSERT INTO messages (
                id, conversation_id, role, content, status, input_origin_kind,
                snapshot_source_conversation_id, snapshot_source_message_id,
                snapshot_original_origin_kind, snapshot_original_agent_id,
                snapshot_original_mailbox_message_id,
                agent_run_json, created_at, position
             ) VALUES (?1, ?2, ?3, ?4, ?5, 'snapshot', ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                params![
                    &message.record.id,
                    &plan.target_conversation_id,
                    &message.record.role,
                    &message.record.content,
                    &message.record.status,
                    &plan.source_conversation_id,
                    &message.source_message_id,
                    original_kind,
                    original_agent_id,
                    original_mailbox_id,
                    &message.record.agent_run_json,
                    message.record.created_at,
                    position as i64,
                ],
            )
            .map_err(database_error)?;
    }
    for message in &plan.messages {
        if let Some(display) = &message.record.human_interaction_response {
            super::human_interaction_repository::store_message_projection(
                connection,
                &message.record.id,
                display,
            )
            .map_err(|error| ConversationForkError::Other(error.to_string()))?;
        }
    }
    for archive in &plan.archives {
        conversation_history_archive_repository::clone_archive_in_connection(connection, archive)
            .map_err(database_error)?;
    }
    for trace in &plan.traces {
        conversation_trace_repository::commit_trace_in_connection(
            connection,
            &trace.trace,
            trace.created_at,
            trace.committed_at,
        )
        .map_err(database_error)?;
        conversation_model_context_repository::commit_items_in_connection(
            connection,
            &trace.trace.conversation_id,
            &trace.trace.assistant_message_id,
            &trace.model_context_items,
        )
        .map_err(database_error)?;
    }
    for copy in &plan.turn_diffs {
        turn_diff_repository::insert_fork_copy(connection, copy).map_err(database_error)?;
    }
    for attachment in &plan.attachments {
        if attachment.target.storage_rel_path.trim().is_empty() {
            return Err(ConversationForkError::Other(
                "snapshot attachment must be staged to an independent target path".into(),
            ));
        }
        attachment_repository::save_attachment(connection, &attachment.target)
            .map_err(database_error)?;
    }
    for grant in &plan.artifact_grants {
        connection
            .execute(
                "INSERT INTO managed_artifact_grants (
                artifact_id, conversation_id, run_id, call_id, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    &grant.artifact_id,
                    &plan.target_conversation_id,
                    &grant.target_run_id,
                    &grant.call_id,
                    grant.created_at,
                ],
            )
            .map_err(database_error)?;
    }
    super::conversation_fork_repository::clone_child_snapshot_summary_chain(
        connection,
        &plan.source_conversation_id,
        &plan.target_conversation_id,
        plan.created_at,
        &plan.summaries,
        &plan.message_id_map,
        &plan.id_replacements,
    )
    .map_err(ConversationForkError::Other)?;
    Ok(())
}

pub(crate) fn child_context_snapshot_fork_turns(
    connection: &Connection,
    target_conversation_id: &str,
) -> Result<Option<AgentForkTurns>, ConversationForkError> {
    connection
        .query_row(
            "SELECT fork_kind, fork_turn_count FROM child_context_snapshots
             WHERE target_conversation_id = ?1",
            [target_conversation_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<u32>>(1)?)),
        )
        .optional()
        .map_err(database_error)?
        .map(|(kind, count)| decode_fork_turns(&kind, count))
        .transpose()
}

fn load_source_origins(
    connection: &Connection,
    conversation_id: &str,
    message_ids: &[String],
) -> Result<HashMap<String, Option<ConversationMessageOrigin>>, ConversationForkError> {
    let mut origins = HashMap::new();
    for message_id in message_ids {
        let role = connection
            .query_row(
                "SELECT role FROM messages WHERE conversation_id = ?1 AND id = ?2",
                params![conversation_id, message_id],
                |row| row.get::<_, String>(0),
            )
            .map_err(database_error)?;
        let origin = if role == "user" {
            Some(
                super::agent_graph_repository::conversation_message_origin(
                    connection,
                    conversation_id,
                    message_id,
                )
                .map_err(|error| ConversationForkError::Other(error.to_string()))?,
            )
        } else {
            None
        };
        origins.insert(message_id.clone(), origin);
    }
    Ok(origins)
}

fn load_artifact_grants(
    connection: &Connection,
    source_conversation_id: &str,
    _target_conversation_id: &str,
    source_run_ids: &HashSet<String>,
    run_id_map: &HashMap<String, String>,
) -> Result<Vec<SnapshotArtifactGrant>, ConversationForkError> {
    if source_run_ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut statement = connection
        .prepare(
            "SELECT artifact_id, run_id, call_id, created_at
         FROM managed_artifact_grants WHERE conversation_id = ?1",
        )
        .map_err(database_error)?;
    let rows = statement
        .query_map([source_conversation_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })
        .map_err(database_error)?;
    let mut grants = Vec::new();
    for row in rows {
        let (artifact_id, source_run_id, call_id, created_at) = row.map_err(database_error)?;
        if !source_run_ids.contains(&source_run_id) {
            continue;
        }
        if managed_artifact_repository::find(connection, &artifact_id)
            .map_err(database_error)?
            .is_none()
        {
            return Err(ConversationForkError::Other(
                "artifact grant references a missing artifact".into(),
            ));
        }
        grants.push(SnapshotArtifactGrant {
            artifact_id,
            target_run_id: mapped(run_id_map, &source_run_id, "artifact run")?,
            call_id,
            created_at,
        });
    }
    Ok(grants)
}

fn rewrite_json_ids<T: serde::Serialize + serde::de::DeserializeOwned>(
    value: &mut T,
    replacements: &HashMap<String, String>,
) -> Result<(), ConversationForkError> {
    let mut json = serde_json::to_value(&*value)
        .map_err(|error| ConversationForkError::Other(error.to_string()))?;
    super::conversation_fork_repository::rewrite_exact_ids(&mut json, replacements);
    super::conversation_fork_repository::rewrite_history_open_tokens(&mut json, replacements)
        .map_err(ConversationForkError::Other)?;
    *value = serde_json::from_value(json)
        .map_err(|error| ConversationForkError::Other(error.to_string()))?;
    Ok(())
}

fn agent_run_status(raw: Option<&str>) -> Result<Option<String>, ConversationForkError> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let value = serde_json::from_str::<Value>(raw).map_err(|error| {
        ConversationForkError::Other(format!("invalid historical run JSON: {error}"))
    })?;
    match value.get("status") {
        Some(Value::String(status)) => Ok(Some(status.clone())),
        Some(Value::Null) | None => Ok(None),
        _ => Err(ConversationForkError::Other(
            "historical run status is invalid".into(),
        )),
    }
}

fn trace_times(
    connection: &Connection,
    message_id: &str,
) -> Result<(i64, i64), ConversationForkError> {
    connection
        .query_row(
            "SELECT created_at, COALESCE(completed_at, updated_at)
         FROM conversation_turn_traces WHERE assistant_message_id = ?1",
            [message_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(database_error)
}

fn encode_fork_turns(value: AgentForkTurns) -> (&'static str, Option<u32>) {
    match value {
        AgentForkTurns::None => ("none", None),
        AgentForkTurns::All => ("all", None),
        AgentForkTurns::Last(count) => ("last", Some(count)),
    }
}

fn decode_fork_turns(
    kind: &str,
    count: Option<u32>,
) -> Result<AgentForkTurns, ConversationForkError> {
    match (kind, count) {
        ("none", None) => Ok(AgentForkTurns::None),
        ("all", None) => Ok(AgentForkTurns::All),
        ("last", Some(count)) if count > 0 => Ok(AgentForkTurns::Last(count)),
        _ => Err(ConversationForkError::Other(
            "stored fork selector is corrupt".into(),
        )),
    }
}

fn mapped(
    mapping: &HashMap<String, String>,
    source: &str,
    label: &str,
) -> Result<String, ConversationForkError> {
    mapping
        .get(source)
        .cloned()
        .ok_or_else(|| ConversationForkError::Other(format!("missing {label} identity mapping")))
}

fn validate_id(label: &str, value: &str) -> Result<(), ConversationForkError> {
    if value.trim().is_empty() || value.trim() != value || value.len() > 256 {
        return Err(ConversationForkError::Other(format!("invalid {label}")));
    }
    Ok(())
}

fn new_id(prefix: &str) -> String {
    format!("{prefix}-{}", Uuid::new_v4())
}

fn database_error(error: rusqlite::Error) -> ConversationForkError {
    ConversationForkError::Other(format!("database operation failed: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::agent_graph_repository;
    use crate::storage::conversation_history_archive_repository::ConversationHistoryArchiveInput;
    use crate::storage::managed_artifact_repository::{
        ManagedArtifactGrant, ManagedArtifactKind, ManagedArtifactRecord,
    };
    use crate::storage::migrations::run_migrations;
    use crate::{
        AgentModelSelectionSnapshot, ContextCompactionGeneration, ContextCompactionSummaryDraft,
        ContextJournalCursor, ConversationTurnTraceTerminalStatus, CreateAgentNodeInput,
        EnsureRootAgentInput, CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
    };

    fn fixture() -> Connection {
        let mut connection = Connection::open_in_memory().unwrap();
        run_migrations(&connection).unwrap();
        for (id, title) in [("source", "Source"), ("target", "Target")] {
            connection
                .execute(
                    "INSERT INTO conversations (
                    id, project_id, model_id, title, created_at, updated_at,
                    pinned_at, archived_at, unread_at
                 ) VALUES (?1, NULL, 'model-a', ?2, 1, 1, NULL, NULL, NULL)",
                    params![id, title],
                )
                .unwrap();
        }
        agent_graph_repository::ensure_root_agent(
            &mut connection,
            &EnsureRootAgentInput {
                agent_id: "root-agent".into(),
                conversation_id: "source".into(),
                creation_request_id: "root-request".into(),
                task_name: "Root".into(),
            },
            1,
        )
        .unwrap();
        agent_graph_repository::create_agent_node(
            &mut connection,
            &CreateAgentNodeInput {
                agent_id: "child-agent".into(),
                root_agent_id: "root-agent".into(),
                parent_agent_id: "root-agent".into(),
                conversation_id: "target".into(),
                creation_request_id: "child-request".into(),
                task_name: "Child".into(),
                task_path: "/root/Child".into(),
                template_snapshot: None,
                model_snapshot: AgentModelSelectionSnapshot {
                    model_config_id: "model-a".into(),
                    display_name: "Model A".into(),
                    supports_image: false,
                    effective_context_window_tokens: 64_000,
                    model_settings_configuration_revision: "model-settings-v1:test".into(),
                    provider_connection_revision: "provider-connection-v1:test".into(),
                    provider_protocol_revision: "provider-protocol-v1:test".into(),
                },
            },
            2,
        )
        .unwrap();
        connection.execute(
            "INSERT INTO messages (
                id, conversation_id, role, content, status, input_origin_kind,
                agent_run_json, created_at, position
             ) VALUES
                ('source-user', 'source', 'user', 'question', 'sent', 'human', NULL, 10, 0),
                ('source-assistant', 'source', 'assistant', 'answer', 'completed', NULL,
                 '{\"runId\":\"source-run\",\"status\":\"completed\",\"usage\":{\"totalTokens\":12}}',
                 11, 1),
                ('source-active', 'source', 'user', 'active tail', 'sent', 'human', NULL, 12, 2)",
            [],
        ).unwrap();
        connection
            .execute(
                "INSERT INTO chat_message_ui_states (message_id, ui_state_json)
             VALUES ('source-assistant', '{\"expanded\":true}')",
                [],
            )
            .unwrap();
        let archive = conversation_history_archive_repository::store_archive(
            &mut connection,
            &ConversationHistoryArchiveInput {
                conversation_id: "source".into(),
                assistant_message_id: "source-assistant".into(),
                sequence: 1,
                call_id: "source-call".into(),
                tool: "read_file".into(),
                content_type: "application/json".into(),
                content: "exact archived evidence".into(),
                truncated_at_source: false,
                model_projection_truncated: true,
                archive_projection_truncated: false,
                created_at: 11,
            },
        )
        .unwrap();
        let digest = "a".repeat(64);
        let artifact = managed_artifact_repository::register(
            &mut connection,
            &ManagedArtifactRecord {
                artifact_id: format!("sha256:{digest}"),
                kind: ManagedArtifactKind::Image,
                storage_relative_path: format!("objects/{digest}.png"),
                format: "png".into(),
                media_type: "image/png".into(),
                size_bytes: 42,
                sha256: digest,
                width: Some(2),
                height: Some(3),
                created_at: 11,
            },
        )
        .unwrap();
        managed_artifact_repository::grant(
            &mut connection,
            &ManagedArtifactGrant {
                artifact_id: artifact.artifact_id.clone(),
                conversation_id: "source".into(),
                run_id: "source-run".into(),
                call_id: "source-call".into(),
                created_at: 11,
            },
        )
        .unwrap();
        let trace = ConversationTurnTrace {
            schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
            run_id: "source-run".into(),
            conversation_id: "source".into(),
            assistant_message_id: "source-assistant".into(),
            terminal_status: ConversationTurnTraceTerminalStatus::Completed,
            terminal_error: None,
            truncated: false,
            items: vec![
                crate::ConversationTurnTraceItem::ToolCall {
                    sequence: 0,
                    call_id: "source-call".into(),
                    tool: "read_file".into(),
                    provenance: crate::AgentToolIdentity::Builtin {
                        tool_name: "read_file".into(),
                    },
                    operation: serde_json::json!({ "path": "evidence.txt" }),
                    approval_status: crate::AgentApprovalStatus::NotRequired,
                    truncated: false,
                },
                crate::ConversationTurnTraceItem::ToolResult {
                    sequence: 1,
                    call_id: "source-call".into(),
                    tool: "read_file".into(),
                    status: crate::ConversationTraceToolResultStatus::Succeeded,
                    success: true,
                    observation: serde_json::json!({
                        "artifact": artifact.artifact_id,
                        "summary": "bounded"
                    }),
                    approval_status: crate::AgentApprovalStatus::NotRequired,
                    error: None,
                    truncated: true,
                    archive: crate::conversation_trace::ConversationHistoryArchiveTraceMetadata {
                        archive_ref: Some(archive.archive_ref.clone()),
                        content_hash: Some(archive.content_hash),
                        archived_bytes: Some(archive.total_bytes),
                        archived_completely: Some(true),
                        history_projection_truncated: true,
                        ..Default::default()
                    },
                },
            ],
        };
        conversation_trace_repository::commit_trace_in_connection(&connection, &trace, 11, 12)
            .unwrap();
        conversation_model_context_repository::commit_items_in_connection(
            &connection,
            "source",
            "source-assistant",
            &[
                ConversationModelContextItem {
                    images: Vec::new(),
                    sequence: 0,
                    ordinal: 0,
                    role: "assistant".into(),
                    content: String::new(),
                    tool_call_id: None,
                    tool_calls: vec![crate::AgentContextCheckpointToolCall {
                        id: "source-call".into(),
                        name: "read_file".into(),
                        args: serde_json::json!({ "path": "evidence.txt" }),
                        provider_identity: crate::AgentProviderToolCallIdentity {
                            provider_tool_index: 0,
                            provider_call_id: "source-call".into(),
                            runtime_call_id: "source-call".into(),
                        },
                    }],
                    is_error: false,
                },
                ConversationModelContextItem {
                    images: Vec::new(),
                    sequence: 1,
                    ordinal: 0,
                    role: "tool".into(),
                    content: "{\"summary\":\"bounded\"}".into(),
                    tool_call_id: Some("source-call".into()),
                    tool_calls: Vec::new(),
                    is_error: false,
                },
            ],
        )
        .unwrap();
        connection
    }

    #[test]
    fn child_snapshot_preserves_material_order_and_rebinds_image_without_runtime_authority() {
        let connection = fixture();
        attachment_repository::insert_attachment(
            &connection,
            &AttachmentRecord {
                id: "source-image".into(),
                conversation_id: "source".into(),
                message_id: "source-user".into(),
                project_id: None,
                kind: "image".into(),
                original_name: "image.png".into(),
                mime_type: Some("image/png".into()),
                size_bytes: 8,
                storage_rel_path: "source/image.png".into(),
                created_at: 10,
            },
        )
        .unwrap();
        let image = crate::ConversationContextImageRef {
            attachment_id: "source-image".into(),
            mime_type: "image/png".into(),
            sha256: format!("sha256:{}", "a".repeat(64)),
        };
        let mut trace =
            conversation_trace_repository::get_trace_for_message(&connection, "source-assistant")
                .unwrap()
                .unwrap();
        let mut model = conversation_model_context_repository::get_log_for_message(
            &connection,
            "source-assistant",
        )
        .unwrap()
        .unwrap()
        .items;
        for (sequence, kind, content, images) in [
            (
                2,
                crate::ConversationContextMaterialKind::InputAttachment,
                "immutable attachment description",
                vec![image.clone()],
            ),
            (
                3,
                crate::ConversationContextMaterialKind::RunWorldState,
                "historical root may ask user",
                Vec::new(),
            ),
        ] {
            trace
                .items
                .push(crate::ConversationTurnTraceItem::ContextMaterial {
                    sequence,
                    event_id: format!("source-material-{sequence}"),
                    material_kind: kind,
                    content: content.into(),
                    images: images.clone(),
                    created_at: 11,
                });
            model.push(ConversationModelContextItem {
                sequence,
                ordinal: 0,
                role: "user".into(),
                content: content.into(),
                images,
                tool_calls: Vec::new(),
                tool_call_id: None,
                is_error: false,
            });
        }
        conversation_trace_repository::commit_trace_in_connection(&connection, &trace, 11, 12)
            .unwrap();
        conversation_model_context_repository::commit_items_in_connection(
            &connection,
            "source",
            "source-assistant",
            &model,
        )
        .unwrap();
        let mut plan = build_child_context_snapshot_plan(
            &connection,
            "source",
            "target",
            &AgentForkTurns::All,
            20,
        )
        .unwrap();
        assert_eq!(plan.attachments.len(), 1);
        plan.attachments[0].target.storage_rel_path = "target/independent-image.png".into();
        let target_image = plan.attachments[0].target.id.clone();
        assert_ne!(target_image, image.attachment_id);
        apply_child_context_snapshot_in_transaction(&connection, &plan).unwrap();
        let log = conversation_model_context_repository::get_log_for_message(
            &connection,
            &plan.message_id_map["source-assistant"],
        )
        .unwrap()
        .unwrap();
        assert_eq!(log.items.len(), 4);
        assert_eq!(log.items[2].images[0].attachment_id, target_image);
        assert_eq!(log.items[2].images[0].sha256, image.sha256);
        assert_eq!(log.items[3].content, model[3].content);
        let child = chat_repository::get_conversation(&connection, "target")
            .unwrap()
            .unwrap();
        assert_eq!(child.messages.len(), 2);
        assert!(child
            .messages
            .iter()
            .all(|message| message.agent_run_json.is_none()));
        for table in [
            "human_interaction_requests",
            "human_interaction_deliveries",
            "human_interaction_suspensions",
            "agent_usage_records",
        ] {
            let count: i64 = connection
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(
                count, 0,
                "history must not grant execution authority: {table}"
            );
        }
    }

    #[test]
    fn child_snapshot_keeps_narration_bound_to_its_snapshot_tool_call() {
        let connection = fixture();
        let mut trace =
            conversation_trace_repository::get_trace_for_message(&connection, "source-assistant")
                .unwrap()
                .unwrap();
        let mut model = conversation_model_context_repository::get_log_for_message(
            &connection,
            "source-assistant",
        )
        .unwrap()
        .unwrap()
        .items;
        let source_call =
            crate::llm::model_response_tool_call_id("source-run", 0, 0, "source-call");
        let provider_turn_id = format!("at1_{}", "a".repeat(64));
        let mut call_trace = trace.items[0].clone();
        let mut result_trace = trace.items[1].clone();
        if let crate::ConversationTurnTraceItem::ToolCall {
            sequence, call_id, ..
        } = &mut call_trace
        {
            *sequence = 3;
            *call_id = source_call.clone();
        }
        if let crate::ConversationTurnTraceItem::ToolResult {
            sequence,
            call_id,
            archive,
            ..
        } = &mut result_trace
        {
            *sequence = 4;
            *call_id = source_call.clone();
            *archive = Default::default();
        }
        trace
            .items
            .push(crate::ConversationTurnTraceItem::AssistantNarration {
                sequence: 2,
                content: "Inspecting evidence.".into(),
                provider_turn_id: Some(provider_turn_id.clone()),
                first_tool_call_id: Some(source_call.clone()),
                truncated: false,
            });
        trace.items.extend([call_trace, result_trace]);
        let mut call_model = model[0].clone();
        call_model.sequence = 3;
        call_model.content = "Inspecting evidence.".into();
        call_model.tool_calls[0].id = source_call.clone();
        call_model.tool_calls[0].provider_identity.runtime_call_id = source_call.clone();
        let mut result_model = model[1].clone();
        result_model.sequence = 4;
        result_model.tool_call_id = Some(source_call.clone());
        model.push(ConversationModelContextItem {
            sequence: 2,
            ordinal: 0,
            role: "assistant".into(),
            content: "Inspecting evidence.".into(),
            images: Vec::new(),
            tool_call_id: None,
            tool_calls: Vec::new(),
            is_error: false,
        });
        model.extend([call_model, result_model]);
        conversation_trace_repository::commit_trace_in_connection(&connection, &trace, 11, 12)
            .unwrap();
        conversation_model_context_repository::commit_items_in_connection(
            &connection,
            "source",
            "source-assistant",
            &model,
        )
        .unwrap();
        let plan = build_child_context_snapshot_plan(
            &connection,
            "source",
            "target",
            &AgentForkTurns::All,
            20,
        )
        .unwrap();
        apply_child_context_snapshot_in_transaction(&connection, &plan).unwrap();
        let target_trace = conversation_trace_repository::get_trace_for_message(
            &connection,
            &plan.message_id_map["source-assistant"],
        )
        .unwrap()
        .unwrap();
        let crate::ConversationTurnTraceItem::AssistantNarration {
            first_tool_call_id: Some(bound_call),
            provider_turn_id: Some(bound_turn),
            ..
        } = &target_trace.items[2]
        else {
            panic!("missing narration identity")
        };
        let crate::ConversationTurnTraceItem::ToolCall { call_id, .. } = &target_trace.items[3]
        else {
            panic!("missing snapshot call")
        };
        assert_ne!(target_trace.run_id, "source-run");
        assert_eq!(bound_call, call_id);
        assert_eq!(bound_turn, &provider_turn_id);
        let log = conversation_model_context_repository::get_log_for_message(
            &connection,
            &plan.message_id_map["source-assistant"],
        )
        .unwrap()
        .unwrap();
        assert_eq!(&log.items[3].tool_calls[0].id, bound_call);
        assert_eq!(log.items[4].tool_call_id.as_ref(), Some(bound_call));
        target_trace
            .validate_complete_model_context(&log.items)
            .unwrap();
    }

    #[test]
    fn child_snapshot_inherits_verified_answer_history_without_question_authority() {
        let connection = fixture();
        let display: crate::human_interaction::HumanInteractionResponseDisplay =
            serde_json::from_value(serde_json::json!({
                "type":"human_interaction_response", "schemaVersion":1,
                "requestId":"source-request", "responseId":"source-response",
                "answers":[{"questionId":"source-question", "question":"source-run",
                    "kind":"text", "answer":"source-call"}]
            }))
            .unwrap();
        let content = serde_json::to_string(&display).unwrap();
        connection
            .execute(
                "UPDATE messages SET content=?1 WHERE id='source-user'",
                [&content],
            )
            .unwrap();
        crate::storage::human_interaction_repository::store_message_projection(
            &connection,
            "source-user",
            &display,
        )
        .unwrap();
        let plan = build_child_context_snapshot_plan(
            &connection,
            "source",
            "target",
            &AgentForkTurns::All,
            20,
        )
        .unwrap();
        apply_child_context_snapshot_in_transaction(&connection, &plan).unwrap();
        let child = chat_repository::get_conversation(&connection, "target")
            .unwrap()
            .unwrap();
        assert_eq!(
            child.messages.len(),
            2,
            "the open parent tail is not inherited"
        );
        assert_eq!(child.messages[0].content, content);
        assert_eq!(
            child.messages[0].human_interaction_response.as_ref(),
            Some(&display)
        );
        assert_ne!(child.messages[0].id, "source-user");
        for table in [
            "human_interaction_requests",
            "human_interaction_responses",
            "human_interaction_deliveries",
            "human_interaction_suspensions",
            "agent_usage_records",
        ] {
            let count: i64 = connection
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(count, 0, "pure inherited history must not create {table}");
        }
        let mut envelope = serde_json::json!({"callId":"source-call", "result":display});
        rewrite_json_ids(
            &mut envelope,
            &HashMap::from([
                ("source-call".into(), "child-call".into()),
                ("source-run".into(), "child-run".into()),
            ]),
        )
        .unwrap();
        assert_eq!(envelope["callId"], "child-call");
        assert_eq!(envelope["result"], serde_json::to_value(display).unwrap());
    }

    #[test]
    fn snapshot_messages_are_exact_noops_for_generic_save_and_reject_mutation() {
        let mut connection = fixture();
        let plan = build_child_context_snapshot_plan(
            &connection,
            "source",
            "target",
            &AgentForkTurns::All,
            20,
        )
        .unwrap();
        apply_child_context_snapshot_in_transaction(&connection, &plan).unwrap();
        let mut target = chat_repository::get_conversation(&connection, "target")
            .unwrap()
            .unwrap();
        assert!(target.messages.iter().all(|message| {
            message.agent_run_json.is_none() && message.ui_state_json.is_none()
        }));
        let snapshot_message_id = target.messages[0].id.clone();
        chat_repository::update_message_ui_state(
            &connection,
            "target",
            &snapshot_message_id,
            Some(r#"{"favorited":true}"#),
        )
        .unwrap();
        chat_repository::save_conversation(&mut connection, target.clone()).unwrap();
        assert_eq!(
            chat_repository::get_conversation(&connection, "target")
                .unwrap()
                .unwrap()
                .messages[0]
                .ui_state_json
                .as_deref(),
            Some(r#"{"favorited":true}"#)
        );
        let target_trace = conversation_trace_repository::get_trace_for_message(
            &connection,
            &target.messages[1].id,
        )
        .unwrap()
        .unwrap();
        assert_ne!(target_trace.run_id, "source-run");
        assert_eq!(target_trace.items.len(), 2);
        assert_eq!(
            conversation_model_context_repository::get_log_for_message(
                &connection,
                &target.messages[1].id,
            )
            .unwrap()
            .unwrap()
            .items
            .len(),
            2
        );
        let copied_archive = conversation_history_archive_repository::find_archive_for_trace_item(
            &connection,
            "target",
            &target.messages[1].id,
            1,
        )
        .unwrap()
        .unwrap();
        assert_ne!(
            copied_archive.archive_ref,
            plan.archives[0].source_archive_ref
        );
        assert!(managed_artifact_repository::find_authorized(
            &connection,
            &format!("sha256:{}", "a".repeat(64)),
            "target",
        )
        .unwrap()
        .is_some());
        target.messages.push(ChatMessageRecord {
            human_interaction_response: None,
            id: "new-assistant".into(),
            role: "assistant".into(),
            content: "new".into(),
            created_at: 21,
            status: Some("completed".into()),
            attachments: Vec::new(),
            agent_run_json: None,
            ui_state_json: None,
        });
        chat_repository::save_conversation(&mut connection, target.clone()).unwrap();
        target.messages[0].content = "tampered".into();
        assert!(chat_repository::save_conversation(&mut connection, target).is_err());
        assert!(connection.execute(
            "DELETE FROM messages WHERE conversation_id = 'target' AND input_origin_kind = 'snapshot'",
            [],
        ).is_err());
        assert!(connection
            .execute(
                "DELETE FROM child_context_snapshots WHERE target_conversation_id = 'target'",
                [],
            )
            .is_err());

        // A child Agent belongs to its root's durable collaboration event log. Raw node deletion
        // is not the future controlled whole-tree deletion path and must not punch through the
        // immutable snapshot provenance while that root still exists.
        assert!(connection
            .execute("DELETE FROM agent_nodes WHERE agent_id = 'child-agent'", [])
            .is_err());
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM child_context_snapshots WHERE target_conversation_id = 'target'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1
        );
    }

    #[test]
    fn snapshot_insert_trigger_rejects_forged_source_content_and_actor() {
        let connection = fixture();
        let plan = build_child_context_snapshot_plan(
            &connection,
            "source",
            "target",
            &AgentForkTurns::None,
            20,
        )
        .unwrap();
        apply_child_context_snapshot_in_transaction(&connection, &plan).unwrap();
        let error = connection
            .execute(
                "INSERT INTO messages (
                id, conversation_id, role, content, status, input_origin_kind,
                snapshot_source_conversation_id, snapshot_source_message_id,
                snapshot_original_origin_kind, agent_run_json,
                created_at, position
             ) VALUES (
                'forged', 'target', 'user', 'forged', 'sent', 'snapshot',
                'source', 'source-active', 'agent', NULL, 12, 0
             )",
                [],
            )
            .unwrap_err();
        assert!(error.to_string().contains("constraint") || error.to_string().contains("snapshot"));
    }

    #[test]
    fn selected_legacy_turn_without_trace_fails_but_last_can_bypass_older_history() {
        let connection = fixture();
        connection
            .execute("DELETE FROM conversation_turn_traces", [])
            .unwrap();
        connection
            .execute(
                "INSERT INTO messages (
                    id, conversation_id, role, content, status, input_origin_kind,
                    agent_run_json, created_at, position
                 ) VALUES (
                    'source-assistant-new', 'source', 'assistant', 'new answer', 'completed',
                    NULL, NULL, 13, 3
                 )",
                [],
            )
            .unwrap();
        conversation_trace_repository::commit_trace_in_connection(
            &connection,
            &ConversationTurnTrace {
                schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
                run_id: "source-run-new".into(),
                conversation_id: "source".into(),
                assistant_message_id: "source-assistant-new".into(),
                terminal_status: ConversationTurnTraceTerminalStatus::Completed,
                terminal_error: None,
                truncated: false,
                items: Vec::new(),
            },
            13,
            14,
        )
        .unwrap();

        for selector in [AgentForkTurns::All, AgentForkTurns::Last(2)] {
            let error =
                build_child_context_snapshot_plan(&connection, "source", "target", &selector, 20)
                    .unwrap_err();
            assert!(error.to_string().contains("missing its durable trace"));
        }

        let last = build_child_context_snapshot_plan(
            &connection,
            "source",
            "target",
            &AgentForkTurns::Last(1),
            20,
        )
        .unwrap();
        assert_eq!(last.selected_turn_count, 1);
        assert_eq!(last.messages.len(), 2);
        assert_eq!(last.messages[0].record.content, "active tail");
        assert_eq!(last.messages[1].record.content, "new answer");
        assert_eq!(last.traces.len(), 1);

        let none = build_child_context_snapshot_plan(
            &connection,
            "source",
            "target",
            &AgentForkTurns::None,
            20,
        )
        .unwrap();
        assert_eq!(none.selected_turn_count, 0);
    }

    #[test]
    fn selected_trace_with_missing_or_incomplete_model_context_fails_closed() {
        for delete_sql in [
            "DELETE FROM conversation_model_context_items WHERE assistant_message_id = 'source-assistant'",
            "DELETE FROM conversation_model_context_items WHERE assistant_message_id = 'source-assistant' AND sequence = 1",
        ] {
            let connection = fixture();
            connection.execute(delete_sql, []).unwrap();

            let error = build_child_context_snapshot_plan(
                &connection,
                "source",
                "target",
                &AgentForkTurns::All,
                20,
            )
            .unwrap_err();

            assert!(error.to_string().contains("incomplete model context"));
            assert_eq!(
                connection
                    .query_row(
                        "SELECT COUNT(*) FROM child_context_snapshots WHERE target_conversation_id = 'target'",
                        [],
                        |row| row.get::<_, i64>(0),
                    )
                    .unwrap(),
                0
            );
        }
    }

    #[test]
    fn all_preserves_active_compaction_while_last_never_imports_older_summary() {
        let mut connection = fixture();
        let prefix = context_compaction_repository::prepare_prefix(
            &connection,
            "source",
            &ContextJournalCursor::message("source-assistant"),
        )
        .unwrap();
        let summary = context_compaction_repository::commit_prefix_replacement(
            &mut connection,
            &prefix,
            ContextCompactionSummaryDraft {
                id: "source-summary".into(),
                source_revision: prefix.source_revision.clone(),
                content: "stable summary".into(),
                continuity: crate::ContextContinuitySnapshot::from_prefix(&prefix).unwrap(),
                generation: ContextCompactionGeneration::test(),
                source_input_tokens: 100,
                summary_input_tokens: 10,
                continuity_input_tokens: 10,
                uncovered_tail_input_tokens: 0,
                replacement_input_tokens: 10,
                created_at: 15,
            },
            "source-assistant",
        )
        .unwrap();

        let all = build_child_context_snapshot_plan(
            &connection,
            "source",
            "target",
            &AgentForkTurns::All,
            20,
        )
        .unwrap();
        assert_eq!(all.summaries.len(), 1);
        let last = build_child_context_snapshot_plan(
            &connection,
            "source",
            "unused-target",
            &AgentForkTurns::Last(1),
            20,
        )
        .unwrap();
        assert!(last.summaries.is_empty());

        apply_child_context_snapshot_in_transaction(&connection, &all).unwrap();
        let copied = context_compaction_repository::get_active_summary(&connection, "target")
            .unwrap()
            .unwrap();
        assert_eq!(copied.content, summary.content);
        assert_ne!(copied.id, summary.id);
        assert_eq!(
            copied.covered_through,
            ContextJournalCursor::message(all.message_id_map["source-assistant"].clone())
        );
    }
}
