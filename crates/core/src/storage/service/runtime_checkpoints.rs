use super::*;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

const MAX_SEQUENCE: u64 = 9_007_199_254_740_991;
const PATCH_KEYS: &[&str] = &[
    "toolDefinitions",
    "toolSetRevision",
    "todo",
    "firstResponseAt",
    "lastResponseAt",
    "explicitSkillSelections",
    "activatedSkills",
    "skillActivationRevision",
    "state",
    "timeline",
    "messageStreamCheckpoints",
    "finishReason",
    "error",
    "fileChanges",
    "skillInstallations",
    "webSearchActivities",
    "readActivities",
    "toolCalls",
    "toolResults",
];

#[derive(Debug, Clone)]
pub struct CoreRuntimeCheckpoint {
    pub conversation_id: String,
    pub assistant_message_id: String,
    pub run_id: String,
    pub generation: String,
    pub expected_sequence: u64,
    pub sequence: u64,
    pub content: String,
    /// A safe presentation patch, never a replacement of Trace/action/usage authority.
    pub projection_json: String,
}

#[derive(Debug, Clone)]
pub struct CoreRuntimeCheckpointState {
    pub generation: String,
    pub sequence: u64,
    pub content: String,
    /// Complete stored projection, used to initialize a new Runtime segment.
    pub projection_json: String,
    /// Stored display calls whose immutable Trace identity proves a built-in read implementation.
    pub presentation_call_ids: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoreRuntimeCheckpointOutcome {
    Persisted,
    Replayed,
    Retired,
}

impl StorageService {
    /// The caller holds the exact Run's publication gate and validates its current runtime token.
    /// A new generation retires the previous segment without replacing its persisted presentation.
    pub fn begin_runtime_checkpoint(
        &self,
        conversation_id: &str,
        assistant_message_id: &str,
        run_id: &str,
    ) -> Result<Option<CoreRuntimeCheckpointState>, String> {
        validate_identity(conversation_id, assistant_message_id, run_id)?;
        self.state.write_sync(
            conversation_id.len() + assistant_message_id.len() + run_id.len() + 4096,
            crate::storage::StorageWriteClass::Control,
            |connection| {
                let transaction = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).map_err(storage_error)?;
                let Some((content, mut projection)) = active_projection(&transaction, conversation_id, assistant_message_id, run_id)? else { return Ok(None) };
                let generation = Uuid::new_v4().to_string();
                projection.insert("coreCheckpoint".into(), serde_json::json!({
                    "generation": generation, "sequence": 0, "fingerprint": format!("{:x}", Sha256::digest([])),
                }));
                let projection_json = serde_json::to_string(&projection).map_err(|error| error.to_string())?;
                let presentation_call_ids = presentation_call_ids(&transaction, assistant_message_id, &projection_json)?;
                transaction.execute("UPDATE messages SET agent_run_json=?1 WHERE conversation_id=?2 AND id=?3",
                    rusqlite::params![projection_json, conversation_id, assistant_message_id]).map_err(storage_error)?;
                transaction.commit().map_err(storage_error)?;
                Ok(Some(CoreRuntimeCheckpointState { generation, sequence: 0, content, projection_json, presentation_call_ids }))
            },
        )?
    }

    /// Commits only the next checkpoint from the current generation. Fingerprints are computed
    /// here from identity, sequence, content and parsed patch, so ACK loss is an exact no-op replay.
    pub fn apply_runtime_checkpoint(
        &self,
        checkpoint: &CoreRuntimeCheckpoint,
    ) -> Result<CoreRuntimeCheckpointOutcome, String> {
        validate_identity(
            &checkpoint.conversation_id,
            &checkpoint.assistant_message_id,
            &checkpoint.run_id,
        )?;
        if checkpoint.sequence == 0
            || checkpoint.sequence > MAX_SEQUENCE
            || checkpoint.expected_sequence.checked_add(1) != Some(checkpoint.sequence)
        {
            return Err("Runtime checkpoint sequence must advance exactly once".into());
        }
        let patch: Map<String, Value> =
            serde_json::from_str(&checkpoint.projection_json).map_err(|error| error.to_string())?;
        if patch.keys().any(|key| !PATCH_KEYS.contains(&key.as_str())) {
            return Err("Runtime checkpoint contains an authoritative or unsupported field".into());
        }
        let fingerprint = format!(
            "{:x}",
            Sha256::digest(
                serde_json::to_vec(&(
                    &checkpoint.conversation_id,
                    &checkpoint.assistant_message_id,
                    &checkpoint.run_id,
                    &checkpoint.generation,
                    checkpoint.expected_sequence,
                    checkpoint.sequence,
                    &checkpoint.content,
                    &patch,
                ))
                .map_err(|error| error.to_string())?
            )
        );
        self.state.write_sync(
            checkpoint.content.len() + checkpoint.projection_json.len() + 4096,
            crate::storage::StorageWriteClass::Normal,
            |connection| {
                let transaction = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).map_err(storage_error)?;
                let Some((existing_content, mut projection)) = active_projection(&transaction, &checkpoint.conversation_id, &checkpoint.assistant_message_id, &checkpoint.run_id)? else {
                    return Ok(CoreRuntimeCheckpointOutcome::Retired);
                };
                let Some(marker) = projection.get("coreCheckpoint").filter(|value| chat_repository::current_core_checkpoint_is_safe(value)) else {
                    return Ok(CoreRuntimeCheckpointOutcome::Retired);
                };
                if marker["generation"].as_str() != Some(checkpoint.generation.as_str()) {
                    return Ok(CoreRuntimeCheckpointOutcome::Retired);
                }
                let current_sequence = marker["sequence"].as_u64().expect("validated checkpoint sequence");
                if current_sequence == checkpoint.sequence && marker["fingerprint"] == fingerprint {
                    return Ok(CoreRuntimeCheckpointOutcome::Replayed);
                }
                if current_sequence != checkpoint.expected_sequence {
                    return Err("Runtime checkpoint sequence conflict".into());
                }
                merge_patch(&mut projection, &patch)?;
                projection.insert("coreCheckpoint".into(), serde_json::json!({
                    "generation": checkpoint.generation, "sequence": checkpoint.sequence, "fingerprint": fingerprint,
                }));
                if !chat_repository::current_agent_run_projection_is_safe_for_trace_rebuild(&projection, &checkpoint.run_id) {
                    return Err("Runtime checkpoint projection is invalid".into());
                }
                let projection_json = serde_json::to_string(&projection).map_err(|error| error.to_string())?;
                if existing_content == checkpoint.content {
                    // Avoid content/status UPDATE triggers and FTS churn for presentation-only ticks.
                    transaction.execute("UPDATE messages SET agent_run_json=?1 WHERE conversation_id=?2 AND id=?3",
                        rusqlite::params![projection_json, checkpoint.conversation_id, checkpoint.assistant_message_id]).map_err(storage_error)?;
                } else {
                    transaction.execute("UPDATE messages SET content=?1, agent_run_json=?2 WHERE conversation_id=?3 AND id=?4",
                        rusqlite::params![checkpoint.content, projection_json, checkpoint.conversation_id, checkpoint.assistant_message_id]).map_err(storage_error)?;
                }
                transaction.commit().map_err(storage_error)?;
                Ok(CoreRuntimeCheckpointOutcome::Persisted)
            },
        )?
    }
}

fn validate_identity(conversation_id: &str, message_id: &str, run_id: &str) -> Result<(), String> {
    if [conversation_id, message_id, run_id]
        .iter()
        .any(|id| id.trim().is_empty())
    {
        return Err("Runtime checkpoint requires exact nonempty Turn identity".into());
    }
    Ok(())
}

fn presentation_call_ids(
    connection: &rusqlite::Connection,
    message_id: &str,
    projection_json: &str,
) -> Result<Vec<String>, String> {
    let mut statement = connection
        .prepare(
            "SELECT json_extract(item_json,'$.callId'), json_extract(item_json,'$.tool'),
                json_extract(item_json,'$.provenance')
         FROM conversation_turn_trace_items
         WHERE assistant_message_id=?1 AND item_kind='tool_call'
           AND EXISTS (SELECT 1 FROM json_each(?2,'$.toolCalls') AS display
               WHERE json_extract(display.value,'$.id')=json_extract(item_json,'$.callId')
                 AND json_extract(display.value,'$.tool')=json_extract(item_json,'$.tool'))
         ORDER BY sequence",
        )
        .map_err(storage_error)?;
    let rows = statement
        .query_map(rusqlite::params![message_id, projection_json], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(storage_error)?;
    let mut identities = Vec::new();
    for row in rows {
        let (call_id, tool, provenance) = row.map_err(storage_error)?;
        if is_display_read_tool(&tool)
            && matches!(serde_json::from_str::<crate::AgentToolIdentity>(&provenance),
            Ok(crate::AgentToolIdentity::Builtin { tool_name }) if tool_name == tool)
        {
            identities.push(call_id);
        }
    }
    Ok(identities)
}

fn is_display_read_tool(tool: &str) -> bool {
    matches!(
        tool,
        "web_search"
            | "web_fetch"
            | "read_file"
            | "read_image"
            | "read_word"
            | "read_spreadsheet"
            | "read_presentation"
    )
}

type ActiveProjection = (String, Map<String, Value>);

fn active_projection(
    connection: &rusqlite::Connection,
    conversation_id: &str,
    message_id: &str,
    run_id: &str,
) -> Result<Option<ActiveProjection>, String> {
    // Superseded/deleted messages are excluded; neither an old owner nor a new generation can
    // reanimate a terminal Trace. This check and publication share the writer transaction.
    let row = connection.query_row(
        "SELECT message.content, message.agent_run_json, message.created_at
         FROM messages AS message JOIN conversation_turn_traces AS trace
           ON trace.assistant_message_id=message.id AND trace.conversation_id=message.conversation_id
         WHERE message.conversation_id=?1 AND message.id=?2 AND message.role='assistant'
           AND trace.run_id=?3 AND trace.terminal_status='in_progress'
           AND NOT EXISTS (SELECT 1 FROM conversation_turn_rewrites AS rewrite
                           WHERE rewrite.conversation_id=message.conversation_id
                             AND rewrite.source_assistant_message_id=message.id)",
        rusqlite::params![conversation_id, message_id, run_id],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?, row.get::<_, i64>(2)?)),
    ).optional().map_err(storage_error)?;
    let Some((content, raw, created_at)) = row else {
        return Ok(None);
    };
    let parsed = raw
        .as_deref()
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok());
    if parsed
        .as_ref()
        .and_then(|value| value.get("runId"))
        .and_then(Value::as_str)
        .is_some_and(|id| id != run_id)
    {
        return Ok(None);
    }
    let status = parsed
        .as_ref()
        .and_then(|value| value.get("status"))
        .and_then(Value::as_str)
        .unwrap_or("running");
    if matches!(status, "completed" | "failed" | "cancelled") {
        return Ok(None);
    }
    if let Some(projection) = parsed.as_ref().and_then(Value::as_object) {
        if chat_repository::current_agent_run_projection_is_safe_for_trace_rebuild(
            projection, run_id,
        ) {
            return Ok(Some((content, projection.clone())));
        }
        if projection.contains_key("coreCheckpoint") {
            return Err("Stored Core runtime projection is invalid".into());
        }
    }
    let canonical = chat_repository::canonical_agent_run_lifecycle_projection(
        raw.as_deref(),
        run_id,
        status,
        created_at,
        now_ms(),
        None,
    )
    .map_err(storage_error)?;
    let projection = serde_json::from_str(&canonical).map_err(|error| error.to_string())?;
    Ok(Some((content, projection)))
}

fn unanchored_stream(item: &Value) -> bool {
    item["type"] == "message"
        && item.get("streamId").and_then(Value::as_str).is_some()
        && item.get("traceSequence").is_none()
}

fn merge_patch(
    projection: &mut Map<String, Value>,
    patch: &Map<String, Value>,
) -> Result<(), String> {
    for (key, value) in patch {
        if key == "timeline" {
            let entries = value
                .as_array()
                .ok_or("Runtime checkpoint timeline must be an array")?;
            if entries.iter().any(|item| !unanchored_stream(item)) {
                return Err("Runtime checkpoint cannot replace a Trace-owned timeline item".into());
            }
            let mut timeline = projection
                .get("timeline")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            timeline.retain(|item| !unanchored_stream(item));
            timeline.extend(entries.iter().cloned());
            projection.insert(key.clone(), Value::Array(timeline));
        } else if key == "toolCalls" || key == "toolResults" {
            let entries = value
                .as_array()
                .ok_or("Runtime checkpoint tool projection must be an array")?;
            if entries
                .iter()
                .any(|entry| !entry["tool"].as_str().is_some_and(is_display_read_tool))
            {
                return Err(
                    "Runtime checkpoint tool projection is outside the read-only display scope"
                        .into(),
                );
            }
            merge_by_identity(
                projection,
                key,
                entries,
                if key == "toolCalls" { "id" } else { "callId" },
            )?;
        } else if matches!(
            key.as_str(),
            "readActivities" | "fileChanges" | "skillInstallations"
        ) {
            let entries = value
                .as_array()
                .ok_or("Runtime checkpoint collection must be an array")?;
            merge_activity_entries(projection, key, entries)?;
        } else if key == "state" {
            let mut state = value
                .as_object()
                .cloned()
                .ok_or("Runtime checkpoint state must be an object")?;
            state.insert("status".into(), projection["status"].clone());
            state.insert("activeRunId".into(), projection["runId"].clone());
            projection.insert(key.clone(), Value::Object(state));
        } else {
            projection.insert(key.clone(), value.clone());
        }
    }
    Ok(())
}

fn activity_identity<'a>(field: &str, value: &'a Value) -> Option<&'a str> {
    match field {
        "readActivities" => value.get("callId"),
        "fileChanges" => value.get("transactionId"),
        "skillInstallations" => value.get("action").and_then(|action| action.get("id")),
        _ => None,
    }
    .and_then(Value::as_str)
}

fn activity_is_settled(field: &str, value: &Value) -> bool {
    match field {
        "readActivities" => matches!(
            value["status"].as_str(),
            Some("completed" | "failed" | "cancelled")
        ),
        "fileChanges" => matches!(
            value["status"].as_str(),
            Some(
                "applied"
                    | "already_applied"
                    | "rejected"
                    | "conflict"
                    | "failed"
                    | "outcome_unknown"
                    | "aborted"
                    | "expired"
            )
        ),
        "skillInstallations" => matches!(
            value["status"].as_str(),
            Some("installed" | "already_installed" | "rejected" | "failed" | "uncertain")
        ),
        _ => false,
    }
}

fn merge_activity_entries(
    projection: &mut Map<String, Value>,
    field: &str,
    incoming: &[Value],
) -> Result<(), String> {
    let mut entries = projection
        .get(field)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut positions = entries
        .iter()
        .enumerate()
        .filter_map(|(index, value)| {
            activity_identity(field, value).map(|id| (id.to_string(), index))
        })
        .collect::<HashMap<_, _>>();
    let mut seen = HashSet::new();
    for value in incoming {
        if !chat_repository::current_checkpoint_collection_entry_is_safe(field, value) {
            return Err("Runtime checkpoint collection entry is invalid".into());
        }
        let identity =
            activity_identity(field, value).ok_or("Runtime checkpoint entry has no identity")?;
        if !seen.insert(identity) {
            return Err("Runtime checkpoint collection repeats an identity".into());
        }
        if let Some(index) = positions.get(identity).copied() {
            let existing = &mut entries[index];
            let (left, right, stable): (&Value, &Value, &[&str]) = match field {
                "readActivities" => (&*existing, value, &["tool", "kind"]),
                "fileChanges" => (
                    &*existing,
                    value,
                    &[
                        "conversationId",
                        "projectId",
                        "filePath",
                        "operation",
                        "createdAt",
                    ],
                ),
                "skillInstallations" => (
                    &existing["action"],
                    &value["action"],
                    &["installRef", "preview", "expiresAt"],
                ),
                _ => unreachable!(),
            };
            if stable.iter().any(|key| left.get(*key) != right.get(*key)) {
                return Err("Runtime checkpoint collection identity changed".into());
            }
            let older = value["updatedAt"]
                .as_u64()
                .zip(existing["updatedAt"].as_u64())
                .is_some_and(|(next, previous)| next < previous);
            if older || (activity_is_settled(field, existing) && !activity_is_settled(field, value))
            {
                continue;
            }
            let mut next = value.clone();
            if field == "skillInstallations"
                && existing["action"]["approvalStatus"] != "required"
                && next["action"]["approvalStatus"] == "required"
            {
                next["action"]["approvalStatus"] = existing["action"]["approvalStatus"].clone();
            }
            *existing = next;
        } else {
            positions.insert(identity.to_string(), entries.len());
            entries.push(value.clone());
        }
    }
    projection.insert(field.to_string(), Value::Array(entries));
    Ok(())
}

fn merge_by_identity(
    projection: &mut Map<String, Value>,
    key: &str,
    incoming: &[Value],
    identity_key: &str,
) -> Result<(), String> {
    let mut entries = projection
        .get(key)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for value in incoming {
        let identity = value
            .get(identity_key)
            .and_then(Value::as_str)
            .ok_or("Runtime checkpoint entry has no identity")?;
        if let Some(existing) = entries
            .iter_mut()
            .find(|item| item.get(identity_key).and_then(Value::as_str) == Some(identity))
        {
            if existing.get("tool") != value.get("tool") {
                return Err("Runtime checkpoint tool identity changed".into());
            }
            *existing = value.clone();
        } else {
            entries.push(value.clone());
        }
    }
    projection.insert(key.to_string(), Value::Array(entries));
    Ok(())
}

#[cfg(test)]
mod tests;
