use crate::protocol::AgentGuidanceStatus;
use crate::storage::models::AgentRunGuidanceRecord;
use rusqlite::types::Type;
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::BTreeSet;
use std::io::{Error as IoError, ErrorKind};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentRunGuidanceStoreOutcome {
    Inserted,
    Idempotent,
    Conflict {
        existing_guidance_id: String,
        existing_status: AgentGuidanceStatus,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentRunGuidanceTransitionOutcome {
    Updated,
    Idempotent,
    NotFound,
    Conflict { current_status: AgentGuidanceStatus },
}

pub fn store_guidance(
    connection: &mut Connection,
    record: &AgentRunGuidanceRecord,
) -> rusqlite::Result<AgentRunGuidanceStoreOutcome> {
    let transaction = connection.transaction()?;
    let outcome = store_guidance_in_connection(&transaction, record)?;
    transaction.commit()?;
    Ok(outcome)
}

pub(crate) fn store_guidance_in_connection(
    connection: &Connection,
    record: &AgentRunGuidanceRecord,
) -> rusqlite::Result<AgentRunGuidanceStoreOutcome> {
    validate_new_record(record)?;
    let role = connection
        .query_row(
            "SELECT role FROM messages
             WHERE id = ?1 AND conversation_id = ?2",
            params![&record.assistant_message_id, &record.conversation_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    if role.as_deref() != Some("assistant") {
        return Err(invalid_input(
            "agent run guidance must belong to an assistant message in the same conversation",
        ));
    }

    let affected = connection.execute(
        "INSERT INTO agent_run_guidances (
            guidance_id, client_message_id, run_id, conversation_id, assistant_message_id,
            content, status, applied_trace_sequence, terminal_reason, created_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, NULL, ?8, ?9)
         ON CONFLICT DO NOTHING",
        params![
            &record.guidance_id,
            &record.client_message_id,
            &record.run_id,
            &record.conversation_id,
            &record.assistant_message_id,
            &record.content,
            record.status.as_str(),
            record.created_at,
            record.updated_at,
        ],
    )?;
    if affected == 0 {
        let existing = load_guidance_by_id_in_connection(connection, &record.guidance_id)?
            .or(load_guidance_by_client_message_in_connection(
                connection,
                &record.run_id,
                &record.client_message_id,
            )?)
            .ok_or(rusqlite::Error::QueryReturnedNoRows)?;
        let outcome = if same_frozen_identity(&existing, record) {
            AgentRunGuidanceStoreOutcome::Idempotent
        } else {
            AgentRunGuidanceStoreOutcome::Conflict {
                existing_guidance_id: existing.guidance_id,
                existing_status: existing.status,
            }
        };
        return Ok(outcome);
    }

    for (position, attachment_id) in record.attachment_ids.iter().enumerate() {
        connection.execute(
            "INSERT INTO agent_run_guidance_attachments (
                guidance_id, attachment_id, position
             ) VALUES (?1, ?2, ?3)",
            params![
                &record.guidance_id,
                attachment_id,
                i64::try_from(position).map_err(|_| {
                    invalid_input("agent run guidance has too many attachments")
                })?,
            ],
        )?;
    }
    Ok(AgentRunGuidanceStoreOutcome::Inserted)
}

pub fn sum_attachment_bytes_for_run(
    connection: &Connection,
    run_id: &str,
) -> rusqlite::Result<u64> {
    let total = connection.query_row(
        "SELECT COALESCE(SUM(attachment.size_bytes), 0)
         FROM agent_run_guidances AS guidance
         JOIN agent_run_guidance_attachments AS ownership
           ON ownership.guidance_id = guidance.guidance_id
         JOIN attachments AS attachment ON attachment.id = ownership.attachment_id
         WHERE guidance.run_id = ?1",
        [run_id],
        |row| row.get::<_, i64>(0),
    )?;
    u64::try_from(total).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(0, Type::Integer, Box::new(error))
    })
}

pub fn load_guidance(
    connection: &Connection,
    guidance_id: &str,
) -> rusqlite::Result<Option<AgentRunGuidanceRecord>> {
    load_guidance_by_id_in_connection(connection, guidance_id)
}

pub fn load_guidance_by_client_message(
    connection: &Connection,
    run_id: &str,
    client_message_id: &str,
) -> rusqlite::Result<Option<AgentRunGuidanceRecord>> {
    load_guidance_by_client_message_in_connection(connection, run_id, client_message_id)
}

pub fn list_queued_guidances(
    connection: &Connection,
) -> rusqlite::Result<Vec<AgentRunGuidanceRecord>> {
    let mut statement = connection.prepare(
        "SELECT guidance_id
         FROM agent_run_guidances
         WHERE status = 'queued'
         ORDER BY created_at ASC, guidance_id ASC",
    )?;
    let ids = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    ids.into_iter()
        .map(|guidance_id| {
            load_guidance_by_id_in_connection(connection, &guidance_id)?
                .ok_or(rusqlite::Error::QueryReturnedNoRows)
        })
        .collect()
}

pub fn mark_guidance_applied(
    connection: &Connection,
    guidance_id: &str,
    trace_sequence: u64,
    updated_at: i64,
) -> rusqlite::Result<AgentRunGuidanceTransitionOutcome> {
    let trace_sequence = i64::try_from(trace_sequence)
        .map_err(|_| invalid_input("guidance trace sequence exceeds SQLite INTEGER"))?;
    let affected = connection.execute(
        "UPDATE agent_run_guidances
         SET status = 'applied',
             applied_trace_sequence = ?1,
             updated_at = ?2
         WHERE guidance_id = ?3
           AND status = 'queued'
           AND updated_at <= ?2",
        params![trace_sequence, updated_at, guidance_id],
    )?;
    if affected == 1 {
        return Ok(AgentRunGuidanceTransitionOutcome::Updated);
    }
    let Some(existing) = load_guidance_by_id_in_connection(connection, guidance_id)? else {
        return Ok(AgentRunGuidanceTransitionOutcome::NotFound);
    };
    if existing.status == AgentGuidanceStatus::Applied
        && existing.applied_trace_sequence == Some(trace_sequence as u64)
    {
        Ok(AgentRunGuidanceTransitionOutcome::Idempotent)
    } else {
        Ok(AgentRunGuidanceTransitionOutcome::Conflict {
            current_status: existing.status,
        })
    }
}

pub fn mark_guidance_terminal(
    connection: &Connection,
    guidance_id: &str,
    status: AgentGuidanceStatus,
    reason: &str,
    updated_at: i64,
) -> rusqlite::Result<AgentRunGuidanceTransitionOutcome> {
    if !matches!(
        status,
        AgentGuidanceStatus::Rejected | AgentGuidanceStatus::Abandoned
    ) || reason.trim().is_empty()
    {
        return Err(invalid_input(
            "terminal guidance transition requires rejected/abandoned status and a reason",
        ));
    }
    let affected = connection.execute(
        "UPDATE agent_run_guidances
         SET status = ?1,
             terminal_reason = ?2,
             updated_at = ?3
         WHERE guidance_id = ?4
           AND status = 'queued'
           AND updated_at <= ?3",
        params![status.as_str(), reason.trim(), updated_at, guidance_id],
    )?;
    if affected == 1 {
        return Ok(AgentRunGuidanceTransitionOutcome::Updated);
    }
    let Some(existing) = load_guidance_by_id_in_connection(connection, guidance_id)? else {
        return Ok(AgentRunGuidanceTransitionOutcome::NotFound);
    };
    if existing.status == status && existing.terminal_reason.as_deref() == Some(reason.trim()) {
        Ok(AgentRunGuidanceTransitionOutcome::Idempotent)
    } else {
        Ok(AgentRunGuidanceTransitionOutcome::Conflict {
            current_status: existing.status,
        })
    }
}

pub fn abandon_queued_guidances_for_run(
    connection: &Connection,
    run_id: &str,
    reason: &str,
    updated_at: i64,
) -> rusqlite::Result<usize> {
    if run_id.trim().is_empty() || reason.trim().is_empty() {
        return Err(invalid_input(
            "abandoning queued guidance requires a run id and reason",
        ));
    }
    connection.execute(
        "UPDATE agent_run_guidances
         SET status = 'abandoned',
             terminal_reason = ?1,
             updated_at = ?2
         WHERE run_id = ?3
           AND status = 'queued'
           AND updated_at <= ?2",
        params![reason.trim(), updated_at, run_id],
    )
}

fn load_guidance_by_id_in_connection(
    connection: &Connection,
    guidance_id: &str,
) -> rusqlite::Result<Option<AgentRunGuidanceRecord>> {
    load_guidance_with_predicate(
        connection,
        "guidance.guidance_id = ?1",
        params![guidance_id],
    )
}

fn load_guidance_by_client_message_in_connection(
    connection: &Connection,
    run_id: &str,
    client_message_id: &str,
) -> rusqlite::Result<Option<AgentRunGuidanceRecord>> {
    load_guidance_with_predicate(
        connection,
        "guidance.run_id = ?1 AND guidance.client_message_id = ?2",
        params![run_id, client_message_id],
    )
}

fn load_guidance_with_predicate(
    connection: &Connection,
    predicate: &str,
    parameters: impl rusqlite::Params,
) -> rusqlite::Result<Option<AgentRunGuidanceRecord>> {
    let sql = format!(
        "SELECT
            guidance.guidance_id,
            guidance.client_message_id,
            guidance.run_id,
            guidance.conversation_id,
            guidance.assistant_message_id,
            guidance.content,
            guidance.status,
            guidance.applied_trace_sequence,
            guidance.terminal_reason,
            guidance.created_at,
            guidance.updated_at
         FROM agent_run_guidances AS guidance
         WHERE {predicate}"
    );
    let mut record = connection
        .query_row(&sql, parameters, guidance_from_row)
        .optional()?;
    if let Some(record) = &mut record {
        record.attachment_ids = load_attachment_ids(connection, &record.guidance_id)?;
    }
    Ok(record)
}

fn guidance_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<AgentRunGuidanceRecord> {
    let status = row.get::<_, String>(6)?;
    let status = AgentGuidanceStatus::from_str(&status).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            6,
            Type::Text,
            Box::new(IoError::new(
                ErrorKind::InvalidData,
                "invalid agent guidance status",
            )),
        )
    })?;
    let trace_sequence = row.get::<_, Option<i64>>(7)?;
    let applied_trace_sequence =
        trace_sequence
            .map(u64::try_from)
            .transpose()
            .map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(7, Type::Integer, Box::new(error))
            })?;
    Ok(AgentRunGuidanceRecord {
        guidance_id: row.get(0)?,
        client_message_id: row.get(1)?,
        run_id: row.get(2)?,
        conversation_id: row.get(3)?,
        assistant_message_id: row.get(4)?,
        content: row.get(5)?,
        status,
        attachment_ids: Vec::new(),
        applied_trace_sequence,
        terminal_reason: row.get(8)?,
        created_at: row.get(9)?,
        updated_at: row.get(10)?,
    })
}

fn load_attachment_ids(
    connection: &Connection,
    guidance_id: &str,
) -> rusqlite::Result<Vec<String>> {
    let mut statement = connection.prepare(
        "SELECT attachment_id
         FROM agent_run_guidance_attachments
         WHERE guidance_id = ?1
         ORDER BY position ASC",
    )?;
    let attachment_ids = statement
        .query_map([guidance_id], |row| row.get::<_, String>(0))?
        .collect();
    attachment_ids
}

fn same_frozen_identity(
    existing: &AgentRunGuidanceRecord,
    candidate: &AgentRunGuidanceRecord,
) -> bool {
    existing.guidance_id == candidate.guidance_id
        && existing.client_message_id == candidate.client_message_id
        && existing.run_id == candidate.run_id
        && existing.conversation_id == candidate.conversation_id
        && existing.assistant_message_id == candidate.assistant_message_id
        && existing.content == candidate.content
        && existing.attachment_ids == candidate.attachment_ids
}

fn validate_new_record(record: &AgentRunGuidanceRecord) -> rusqlite::Result<()> {
    if record.guidance_id.trim().is_empty()
        || record.client_message_id.trim().is_empty()
        || record.run_id.trim().is_empty()
        || record.conversation_id.trim().is_empty()
        || record.assistant_message_id.trim().is_empty()
        || record.content.trim().is_empty()
        || record.status != AgentGuidanceStatus::Queued
        || record.applied_trace_sequence.is_some()
        || record.terminal_reason.is_some()
        || record.created_at < 0
        || record.updated_at < record.created_at
    {
        return Err(invalid_input("invalid queued agent run guidance record"));
    }
    let mut attachment_ids = BTreeSet::new();
    if record
        .attachment_ids
        .iter()
        .any(|id| id.trim().is_empty() || !attachment_ids.insert(id.as_str()))
    {
        return Err(invalid_input(
            "agent run guidance contains invalid or duplicate attachment ids",
        ));
    }
    Ok(())
}

fn invalid_input(message: &str) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(Box::new(IoError::new(
        ErrorKind::InvalidInput,
        message,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::migrations;

    fn setup() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        migrations::run_migrations(&connection).unwrap();
        connection
            .execute_batch(
                "INSERT INTO conversations (
                    id, project_id, model_id, title, created_at, updated_at,
                    pinned_at, archived_at, unread_at
                 ) VALUES ('conversation-1', NULL, NULL, 'title', 1, 1, NULL, NULL, NULL);
                 INSERT INTO messages (
                    id, conversation_id, role, content, status, agent_run_json,
                    ui_state_json, created_at, position
                 ) VALUES (
                    'assistant-1', 'conversation-1', 'assistant', '', 'pending',
                    NULL, NULL, 2, 0
                 );
                 INSERT INTO attachments (
                    id, conversation_id, message_id, project_id, kind, original_name,
                    mime_type, size_bytes, storage_rel_path, created_at
                 ) VALUES (
                    'attachment-1', 'conversation-1', 'assistant-1', NULL, 'file',
                    'notes.txt', 'text/plain', 5, 'conversation-1/notes.txt', 3
                 );",
            )
            .unwrap();
        connection
    }

    fn record() -> AgentRunGuidanceRecord {
        AgentRunGuidanceRecord {
            guidance_id: "guidance-1".to_string(),
            client_message_id: "client-1".to_string(),
            run_id: "run-1".to_string(),
            conversation_id: "conversation-1".to_string(),
            assistant_message_id: "assistant-1".to_string(),
            content: "Please inspect the notes.".to_string(),
            status: AgentGuidanceStatus::Queued,
            attachment_ids: vec!["attachment-1".to_string()],
            applied_trace_sequence: None,
            terminal_reason: None,
            created_at: 10,
            updated_at: 10,
        }
    }

    #[test]
    fn guidance_store_is_idempotent_and_preserves_attachment_order() {
        let mut connection = setup();
        let record = record();
        assert_eq!(
            store_guidance(&mut connection, &record).unwrap(),
            AgentRunGuidanceStoreOutcome::Inserted
        );
        assert_eq!(
            store_guidance(&mut connection, &record).unwrap(),
            AgentRunGuidanceStoreOutcome::Idempotent
        );
        assert_eq!(
            load_guidance(&connection, "guidance-1").unwrap(),
            Some(record.clone())
        );
        assert_eq!(
            load_guidance_by_client_message(&connection, "run-1", "client-1").unwrap(),
            Some(record)
        );
    }

    #[test]
    fn guidance_store_rejects_client_and_guidance_identity_reuse() {
        let mut connection = setup();
        store_guidance(&mut connection, &record()).unwrap();

        let mut changed = record();
        changed.guidance_id = "guidance-2".to_string();
        changed.content = "Different content.".to_string();
        assert!(matches!(
            store_guidance(&mut connection, &changed).unwrap(),
            AgentRunGuidanceStoreOutcome::Conflict {
                existing_guidance_id,
                ..
            } if existing_guidance_id == "guidance-1"
        ));

        let mut changed = record();
        changed.client_message_id = "client-2".to_string();
        changed.content = "Different content.".to_string();
        assert!(matches!(
            store_guidance(&mut connection, &changed).unwrap(),
            AgentRunGuidanceStoreOutcome::Conflict {
                existing_guidance_id,
                ..
            } if existing_guidance_id == "guidance-1"
        ));
    }

    #[test]
    fn applied_and_terminal_transitions_are_compare_and_set_idempotent() {
        let mut connection = setup();
        store_guidance(&mut connection, &record()).unwrap();
        assert_eq!(
            mark_guidance_applied(&connection, "guidance-1", 4, 11).unwrap(),
            AgentRunGuidanceTransitionOutcome::Updated
        );
        assert_eq!(
            mark_guidance_applied(&connection, "guidance-1", 4, 11).unwrap(),
            AgentRunGuidanceTransitionOutcome::Idempotent
        );
        assert!(matches!(
            mark_guidance_terminal(
                &connection,
                "guidance-1",
                AgentGuidanceStatus::Rejected,
                "too late",
                12,
            )
            .unwrap(),
            AgentRunGuidanceTransitionOutcome::Conflict {
                current_status: AgentGuidanceStatus::Applied
            }
        ));
        let applied = load_guidance(&connection, "guidance-1").unwrap().unwrap();
        assert_eq!(applied.status, AgentGuidanceStatus::Applied);
        assert_eq!(applied.applied_trace_sequence, Some(4));
    }

    #[test]
    fn queued_guidance_can_be_abandoned_during_startup_reconciliation() {
        let mut connection = setup();
        store_guidance(&mut connection, &record()).unwrap();
        assert_eq!(list_queued_guidances(&connection).unwrap().len(), 1);
        assert_eq!(
            abandon_queued_guidances_for_run(&connection, "run-1", "host process restarted", 20,)
                .unwrap(),
            1
        );
        let abandoned = load_guidance(&connection, "guidance-1").unwrap().unwrap();
        assert_eq!(abandoned.status, AgentGuidanceStatus::Abandoned);
        assert_eq!(
            abandoned.terminal_reason.as_deref(),
            Some("host process restarted")
        );
        assert!(list_queued_guidances(&connection).unwrap().is_empty());
    }
}
