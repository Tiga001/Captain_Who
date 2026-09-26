use crate::{
    AgentCollaborationActivitySemantic, AgentCollaborationActivitySnapshot,
    AgentCollaborationEventError, AgentCollaborationEventKind, AgentCollaborationEventRecord,
    AGENT_COLLABORATION_ACTIVITY_SCHEMA_VERSION, AGENT_COLLABORATION_EVENT_SCHEMA_VERSION,
    MAX_AGENT_COLLABORATION_EVENTS_PAGE,
};
use rusqlite::{params, Connection};

#[cfg(test)]
mod task_activity_tests;
mod transmission;

const EVENT_SELECT: &str = "
    SELECT global_sequence, schema_version, event_id, root_sequence, workspace_id, project_id,
           root_agent_id, root_conversation_id, agent_id, conversation_id,
           turn_id, run_id, message_id, kind, resource_revision,
           created_at
    FROM agent_collaboration_events";

pub fn list_root_events(
    connection: &Connection,
    root_agent_id: &str,
    after_root_sequence: u64,
    maximum: usize,
) -> Result<Vec<AgentCollaborationEventRecord>, AgentCollaborationEventError> {
    validate_id(root_agent_id)?;
    let maximum = validate_maximum(maximum)?;
    let after = i64::try_from(after_root_sequence)
        .map_err(|_| AgentCollaborationEventError::InvalidInput)?;
    query_events(
        connection,
        &format!(
            "{EVENT_SELECT}
             WHERE root_agent_id = ?1 AND root_sequence > ?2
             ORDER BY root_sequence
             LIMIT ?3"
        ),
        params![root_agent_id, after, maximum],
    )
}

pub fn list_global_events(
    connection: &Connection,
    after_global_sequence: u64,
    maximum: usize,
) -> Result<Vec<AgentCollaborationEventRecord>, AgentCollaborationEventError> {
    let maximum = validate_maximum(maximum)?;
    let after = i64::try_from(after_global_sequence)
        .map_err(|_| AgentCollaborationEventError::InvalidInput)?;
    query_events(
        connection,
        &format!(
            "{EVENT_SELECT}
             WHERE global_sequence > ?1
             ORDER BY global_sequence
             LIMIT ?2"
        ),
        params![after, maximum],
    )
}

/// Returns the bounded, renderer-safe activity facts owned by one Assistant message.
///
/// This is an internal terminal-snapshot query, not a public event-log page. It deliberately
/// selects only events carrying the exact durable message/trace anchor that the chat Timeline can
/// render. Operational events without an activity projection never leave storage through this
/// path.
pub(crate) fn list_message_activities(
    connection: &Connection,
    assistant_message_id: &str,
) -> Result<Vec<AgentCollaborationEventRecord>, AgentCollaborationEventError> {
    validate_id(assistant_message_id)?;
    const MAX_TERMINAL_MESSAGE_ACTIVITIES: i64 = 2_048;
    let mut events = query_events(
        connection,
        &format!(
            "{EVENT_SELECT}
             WHERE EXISTS (
                 SELECT 1 FROM agent_collaboration_event_activities AS activity
                 WHERE activity.event_id = agent_collaboration_events.event_id
                   AND activity.anchor_message_id = ?1
                   AND activity.trace_boundary_sequence IS NOT NULL
             )
             ORDER BY root_sequence DESC
             LIMIT ?2"
        ),
        params![assistant_message_id, MAX_TERMINAL_MESSAGE_ACTIVITIES],
    )?;
    events.reverse();
    for event in &mut events {
        event.activities.retain(|activity| {
            activity.anchor_message_id.as_deref() == Some(assistant_message_id)
                && activity.trace_boundary_sequence.is_some()
        });
    }
    retain_latest_activities(&mut events, MAX_TERMINAL_MESSAGE_ACTIVITIES as usize);
    Ok(events)
}

fn retain_latest_activities(events: &mut Vec<AgentCollaborationEventRecord>, maximum: usize) {
    let mut remove = events
        .iter()
        .map(|event| event.activities.len())
        .sum::<usize>()
        .saturating_sub(maximum);
    for event in events.iter_mut() {
        let count = remove.min(event.activities.len());
        event.activities.drain(..count);
        remove -= count;
    }
    events.retain(|event| !event.activities.is_empty());
}

pub fn latest_root_sequence(
    connection: &Connection,
    root_agent_id: &str,
) -> Result<u64, AgentCollaborationEventError> {
    validate_id(root_agent_id)?;
    let value = connection
        .query_row(
            "SELECT COALESCE(MAX(root_sequence), 0)
             FROM agent_collaboration_events WHERE root_agent_id = ?1",
            [root_agent_id],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|_| AgentCollaborationEventError::StorageUnavailable)?;
    u64::try_from(value).map_err(|_| AgentCollaborationEventError::CorruptRecord)
}

pub fn latest_global_sequence(
    connection: &Connection,
) -> Result<u64, AgentCollaborationEventError> {
    let value = connection
        .query_row(
            "SELECT COALESCE(MAX(global_sequence), 0) FROM agent_collaboration_events",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|_| AgentCollaborationEventError::StorageUnavailable)?;
    u64::try_from(value).map_err(|_| AgentCollaborationEventError::CorruptRecord)
}

pub fn latest_agent_activity_at(
    connection: &Connection,
    root_agent_id: &str,
    agent_id: &str,
) -> Result<Option<i64>, AgentCollaborationEventError> {
    validate_id(root_agent_id)?;
    validate_id(agent_id)?;
    connection
        .query_row(
            "SELECT MAX(created_at) FROM agent_collaboration_events
             WHERE root_agent_id = ?1 AND agent_id = ?2",
            params![root_agent_id, agent_id],
            |row| row.get::<_, Option<i64>>(0),
        )
        .map_err(|_| AgentCollaborationEventError::StorageUnavailable)?
        .map(nonnegative)
        .transpose()
}

fn query_events<P: rusqlite::Params>(
    connection: &Connection,
    sql: &str,
    params: P,
) -> Result<Vec<AgentCollaborationEventRecord>, AgentCollaborationEventError> {
    let mut statement = connection
        .prepare(sql)
        .map_err(|_| AgentCollaborationEventError::StorageUnavailable)?;
    let rows = statement
        .query_map(params, |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, String>(8)?,
                row.get::<_, String>(9)?,
                row.get::<_, Option<String>>(10)?,
                row.get::<_, Option<String>>(11)?,
                row.get::<_, Option<String>>(12)?,
                row.get::<_, String>(13)?,
                row.get::<_, i64>(14)?,
                row.get::<_, i64>(15)?,
            ))
        })
        .map_err(|_| AgentCollaborationEventError::StorageUnavailable)?;
    let mut events = Vec::new();
    for row in rows {
        let (
            global_sequence,
            stored_schema_version,
            event_id,
            root_sequence,
            workspace_id,
            project_id,
            root_agent_id,
            root_conversation_id,
            agent_id,
            conversation_id,
            turn_id,
            run_id,
            message_id,
            kind,
            resource_revision,
            created_at,
        ) = row.map_err(|_| AgentCollaborationEventError::StorageUnavailable)?;
        // The immutable operational log retains its v2 storage shape. The v3 transport projects
        // the separate request-owned facts; pre-v53 rows deliberately have an empty collection.
        if stored_schema_version != 2 {
            return Err(AgentCollaborationEventError::CorruptRecord);
        }
        let activities = read_activities(connection, &event_id)?;
        let mut event = AgentCollaborationEventRecord {
            global_sequence: positive(global_sequence)?,
            schema_version: AGENT_COLLABORATION_EVENT_SCHEMA_VERSION,
            event_id,
            root_sequence: positive(root_sequence)?,
            workspace_id,
            project_id,
            root_agent_id,
            root_conversation_id,
            agent_id,
            conversation_id,
            turn_id,
            run_id,
            message_id,
            kind: AgentCollaborationEventKind::parse(&kind)?,
            resource_revision: positive(resource_revision)?,
            activities,
            transmission: None,
            created_at: nonnegative(created_at)?,
        };
        event.transmission = transmission::project(connection, &event)?;
        events.push(event);
    }
    Ok(events)
}

fn read_activities(
    connection: &Connection,
    event_id: &str,
) -> Result<Vec<AgentCollaborationActivitySnapshot>, AgentCollaborationEventError> {
    let mut statement = connection
        .prepare(
            "SELECT schema_version, activity_id, semantic, agent_id, task_name_snapshot,
                owner_agent_id, owner_conversation_id, task_message_id, anchor_message_id,
                trace_boundary_sequence
         FROM agent_collaboration_event_activities WHERE event_id = ?1 ORDER BY activity_id",
        )
        .map_err(|_| AgentCollaborationEventError::StorageUnavailable)?;
    let rows = statement
        .query_map([event_id], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, Option<String>>(7)?,
                row.get::<_, Option<String>>(8)?,
                row.get::<_, Option<i64>>(9)?,
            ))
        })
        .map_err(|_| AgentCollaborationEventError::StorageUnavailable)?;
    rows.map(|row| {
        let (
            schema_version,
            activity_id,
            semantic,
            agent_id,
            task_name_snapshot,
            owner_agent_id,
            owner_conversation_id,
            task_message_id,
            anchor_message_id,
            trace_boundary_sequence,
        ) = row.map_err(|_| AgentCollaborationEventError::StorageUnavailable)?;
        let schema_version = u32::try_from(schema_version)
            .map_err(|_| AgentCollaborationEventError::CorruptRecord)?;
        if schema_version != AGENT_COLLABORATION_ACTIVITY_SCHEMA_VERSION {
            return Err(AgentCollaborationEventError::CorruptRecord);
        }
        for value in [
            &activity_id,
            &agent_id,
            &task_name_snapshot,
            &owner_agent_id,
            &owner_conversation_id,
        ] {
            validate_persisted_text(value, 256)?;
        }
        if let Some(task) = task_message_id.as_deref() {
            validate_persisted_text(task, 2_048)?;
        }
        if let Some(anchor) = anchor_message_id.as_deref() {
            validate_persisted_text(anchor, 2_048)?;
        }
        let semantic = AgentCollaborationActivitySemantic::parse(&semantic)?;
        if (semantic == AgentCollaborationActivitySemantic::Updated) != task_message_id.is_none()
            || agent_id == owner_agent_id
            || (anchor_message_id.is_none() && trace_boundary_sequence.is_some())
        {
            return Err(AgentCollaborationEventError::CorruptRecord);
        }
        Ok(AgentCollaborationActivitySnapshot {
            schema_version,
            activity_id,
            semantic,
            agent_id,
            task_name_snapshot,
            owner_agent_id,
            owner_conversation_id,
            task_message_id,
            anchor_message_id,
            trace_boundary_sequence: trace_boundary_sequence
                .map(|value| {
                    u64::try_from(value).map_err(|_| AgentCollaborationEventError::CorruptRecord)
                })
                .transpose()?,
        })
    })
    .collect()
}

fn validate_persisted_text(
    value: &str,
    maximum: usize,
) -> Result<(), AgentCollaborationEventError> {
    if value.trim() != value || value.is_empty() || value.len() > maximum || value.contains('\0') {
        return Err(AgentCollaborationEventError::CorruptRecord);
    }
    Ok(())
}

fn validate_id(value: &str) -> Result<(), AgentCollaborationEventError> {
    if value.trim() != value || value.is_empty() || value.len() > 256 {
        return Err(AgentCollaborationEventError::InvalidInput);
    }
    Ok(())
}

fn validate_maximum(maximum: usize) -> Result<i64, AgentCollaborationEventError> {
    if maximum == 0 || maximum > MAX_AGENT_COLLABORATION_EVENTS_PAGE {
        return Err(AgentCollaborationEventError::InvalidInput);
    }
    i64::try_from(maximum).map_err(|_| AgentCollaborationEventError::InvalidInput)
}

fn positive(value: i64) -> Result<u64, AgentCollaborationEventError> {
    if value <= 0 {
        return Err(AgentCollaborationEventError::CorruptRecord);
    }
    u64::try_from(value).map_err(|_| AgentCollaborationEventError::CorruptRecord)
}

fn nonnegative(value: i64) -> Result<i64, AgentCollaborationEventError> {
    if value < 0 {
        return Err(AgentCollaborationEventError::CorruptRecord);
    }
    Ok(value)
}

#[cfg(test)]
mod tests;
