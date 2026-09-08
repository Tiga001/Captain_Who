use crate::{
    AgentTurnDiffIdentity, AgentTurnDiffRecord, AgentTurnFileChange, AgentTurnFileContent,
    AGENT_TURN_DIFF_SCHEMA_VERSION,
};
use rusqlite::types::Value;
use rusqlite::{params, params_from_iter, Connection, OptionalExtension};
use std::collections::{HashMap, HashSet};
use std::path::{Component, Path};

const MAX_TURN_DIFF_FILES: i64 = 500;
const MAX_TURN_DIFF_TEXT_BYTES: i64 = 32 * 1024 * 1024;
const LOAD_TURNS_QUERY_CHUNK_SIZE: usize = 200;

#[derive(Debug, Clone)]
pub(crate) struct AgentTurnDiffForkAction {
    pub action_id: String,
    pub created_at: i64,
}

/// Exact durable evidence for one visible turn. Forks preserve it verbatim so a forked
/// conversation can itself be forked without losing review history or action idempotency.
#[derive(Debug, Clone)]
pub(crate) struct AgentTurnDiffForkCopy {
    pub record: AgentTurnDiffRecord,
    pub actions: Vec<AgentTurnDiffForkAction>,
    pub created_at: i64,
    pub updated_at: i64,
}

pub fn initialize_turn(
    connection: &Connection,
    identity: &AgentTurnDiffIdentity,
    created_at: i64,
) -> rusqlite::Result<()> {
    validate_identity(identity)?;
    validate_assistant_message(connection, identity)?;
    insert_turn_if_absent(connection, identity, created_at)?;
    validate_stored_identity(connection, identity)
}

pub fn record_file_change(
    connection: &mut Connection,
    identity: &AgentTurnDiffIdentity,
    action_id: &str,
    change: &AgentTurnFileChange,
    updated_at: i64,
) -> rusqlite::Result<bool> {
    validate_identity(identity)?;
    validate_change(change)?;
    let action_id = action_id.trim();
    if action_id.is_empty() {
        return Err(invalid_input("agent turn diff action id cannot be empty"));
    }

    let transaction = connection.transaction()?;
    validate_assistant_message(&transaction, identity)?;
    insert_turn_if_absent(&transaction, identity, updated_at)?;
    validate_stored_identity(&transaction, identity)?;

    let inserted = transaction.execute(
        "
        INSERT INTO agent_turn_diff_actions (assistant_message_id, action_id, created_at)
        VALUES (?1, ?2, ?3)
        ON CONFLICT(assistant_message_id, action_id) DO NOTHING
        ",
        params![identity.assistant_message_id, action_id, updated_at],
    )?;
    if inserted == 0 {
        transaction.commit()?;
        return Ok(false);
    }

    let existing = load_file_change(&transaction, &identity.assistant_message_id, &change.path)?;
    if existing.is_none() {
        let file_count: i64 = transaction.query_row(
            "SELECT COUNT(*) FROM agent_turn_diff_files WHERE assistant_message_id = ?1",
            [&identity.assistant_message_id],
            |row| row.get(0),
        )?;
        if file_count >= MAX_TURN_DIFF_FILES {
            mark_turn_truncated(&transaction, &identity.assistant_message_id, updated_at)?;
            transaction.commit()?;
            return Ok(true);
        }
    }

    let mut aggregate = AgentTurnFileChange {
        path: change.path.clone(),
        before: existing
            .as_ref()
            .map(|existing| existing.before.clone())
            .unwrap_or_else(|| change.before.clone()),
        after: change.after.clone(),
    };
    if aggregate.is_exact_noop() {
        transaction.execute(
            "
            DELETE FROM agent_turn_diff_files
            WHERE assistant_message_id = ?1 AND path = ?2
            ",
            params![identity.assistant_message_id, aggregate.path],
        )?;
        touch_turn(&transaction, &identity.assistant_message_id, updated_at)?;
        transaction.commit()?;
        return Ok(true);
    }

    let existing_bytes = existing.as_ref().map(change_text_bytes).unwrap_or_default();
    let stored_bytes: i64 = transaction.query_row(
        "
        SELECT COALESCE(SUM(
            COALESCE(length(CAST(before_text AS BLOB)), 0) +
            COALESCE(length(CAST(after_text AS BLOB)), 0)
        ), 0)
        FROM agent_turn_diff_files
        WHERE assistant_message_id = ?1
        ",
        [&identity.assistant_message_id],
        |row| row.get(0),
    )?;
    let aggregate_bytes = change_text_bytes(&aggregate);
    if stored_bytes
        .saturating_sub(existing_bytes)
        .saturating_add(aggregate_bytes)
        > MAX_TURN_DIFF_TEXT_BYTES
    {
        degrade_text_content(&mut aggregate.before);
        degrade_text_content(&mut aggregate.after);
        mark_turn_truncated(&transaction, &identity.assistant_message_id, updated_at)?;
    }

    transaction.execute(
        "
        INSERT INTO agent_turn_diff_files (
            assistant_message_id,
            path,
            before_kind,
            before_text,
            after_kind,
            after_text
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6)
        ON CONFLICT(assistant_message_id, path) DO UPDATE SET
            before_kind = excluded.before_kind,
            before_text = excluded.before_text,
            after_kind = excluded.after_kind,
            after_text = excluded.after_text
        ",
        params![
            identity.assistant_message_id,
            aggregate.path,
            aggregate.before.kind(),
            aggregate.before.text(),
            aggregate.after.kind(),
            aggregate.after.text(),
        ],
    )?;
    touch_turn(&transaction, &identity.assistant_message_id, updated_at)?;
    transaction.commit()?;
    Ok(true)
}

pub fn load_latest_turn(
    connection: &Connection,
    conversation_id: &str,
    project_id: &str,
) -> rusqlite::Result<Option<AgentTurnDiffRecord>> {
    let identity_and_truncated = connection
        .query_row(
            "
            SELECT
                turn.run_id,
                turn.conversation_id,
                turn.assistant_message_id,
                turn.project_id,
                turn.workspace_root,
                turn.truncated
            FROM agent_turn_diffs AS turn
            INNER JOIN messages AS message
                ON message.id = turn.assistant_message_id
            WHERE turn.conversation_id = ?1
              AND turn.project_id = ?2
              AND turn.schema_version = ?3
            ORDER BY
                message.position DESC,
                message.created_at DESC,
                turn.updated_at DESC,
                turn.assistant_message_id DESC
            LIMIT 1
            ",
            params![
                conversation_id,
                project_id,
                i64::from(AGENT_TURN_DIFF_SCHEMA_VERSION)
            ],
            |row| {
                Ok((
                    AgentTurnDiffIdentity {
                        run_id: row.get(0)?,
                        conversation_id: row.get(1)?,
                        assistant_message_id: row.get(2)?,
                        project_id: row.get(3)?,
                        workspace_root: row.get(4)?,
                    },
                    row.get::<_, bool>(5)?,
                ))
            },
        )
        .optional()?;
    let Some((identity, truncated)) = identity_and_truncated else {
        return Ok(None);
    };

    let mut statement = connection.prepare(
        "
        SELECT path, before_kind, before_text, after_kind, after_text
        FROM agent_turn_diff_files
        WHERE assistant_message_id = ?1
        ORDER BY path COLLATE NOCASE ASC, path ASC
        ",
    )?;
    let files = statement
        .query_map([&identity.assistant_message_id], |row| {
            let before_kind = row.get::<_, String>(1)?;
            let before_text = row.get::<_, Option<String>>(2)?;
            let after_kind = row.get::<_, String>(3)?;
            let after_text = row.get::<_, Option<String>>(4)?;
            Ok((
                row.get::<_, String>(0)?,
                before_kind,
                before_text,
                after_kind,
                after_text,
            ))
        })?
        .map(|result| {
            let (path, before_kind, before_text, after_kind, after_text) = result?;
            let before = AgentTurnFileContent::from_storage(&before_kind, before_text)
                .map_err(invalid_input)?;
            let after = AgentTurnFileContent::from_storage(&after_kind, after_text)
                .map_err(invalid_input)?;
            Ok(AgentTurnFileChange {
                path,
                before,
                after,
            })
        })
        .collect::<rusqlite::Result<Vec<_>>>()?;

    Ok(Some(AgentTurnDiffRecord {
        identity,
        files,
        truncated,
    }))
}

pub fn load_turns_for_messages(
    connection: &Connection,
    conversation_id: &str,
    project_id: &str,
    assistant_message_ids: &[String],
) -> rusqlite::Result<Vec<AgentTurnDiffRecord>> {
    let mut seen_ids = HashSet::new();
    let requested_ids = assistant_message_ids
        .iter()
        .filter(|id| !id.trim().is_empty() && seen_ids.insert((*id).clone()))
        .cloned()
        .collect::<Vec<_>>();
    if requested_ids.is_empty() {
        return Ok(Vec::new());
    }

    let mut records = HashMap::<String, AgentTurnDiffRecord>::new();
    for chunk in requested_ids.chunks(LOAD_TURNS_QUERY_CHUNK_SIZE) {
        let placeholders = std::iter::repeat_n("?", chunk.len())
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "
            SELECT
                turn.run_id,
                turn.conversation_id,
                turn.assistant_message_id,
                turn.project_id,
                turn.workspace_root,
                turn.truncated,
                file.path,
                file.before_kind,
                file.before_text,
                file.after_kind,
                file.after_text
            FROM agent_turn_diffs AS turn
            INNER JOIN messages AS message
                ON message.id = turn.assistant_message_id
            LEFT JOIN agent_turn_diff_files AS file
                ON file.assistant_message_id = turn.assistant_message_id
            WHERE turn.conversation_id = ?
              AND turn.project_id = ?
              AND turn.schema_version = ?
              AND message.conversation_id = ?
              AND turn.assistant_message_id IN ({placeholders})
            ORDER BY
                message.position ASC,
                file.path COLLATE NOCASE ASC,
                file.path ASC
            "
        );
        let mut values = vec![
            Value::Text(conversation_id.to_string()),
            Value::Text(project_id.to_string()),
            Value::Integer(i64::from(AGENT_TURN_DIFF_SCHEMA_VERSION)),
            Value::Text(conversation_id.to_string()),
        ];
        values.extend(chunk.iter().cloned().map(Value::Text));

        let mut statement = connection.prepare(&sql)?;
        let mut rows = statement.query(params_from_iter(values.iter()))?;
        while let Some(row) = rows.next()? {
            let identity = AgentTurnDiffIdentity {
                run_id: row.get(0)?,
                conversation_id: row.get(1)?,
                assistant_message_id: row.get(2)?,
                project_id: row.get(3)?,
                workspace_root: row.get(4)?,
            };
            let assistant_message_id = identity.assistant_message_id.clone();
            let truncated = row.get::<_, bool>(5)?;
            let record =
                records
                    .entry(assistant_message_id)
                    .or_insert_with(|| AgentTurnDiffRecord {
                        identity,
                        files: Vec::new(),
                        truncated,
                    });

            let Some(path) = row.get::<_, Option<String>>(6)? else {
                continue;
            };
            let before_kind = row.get::<_, String>(7)?;
            let before_text = row.get::<_, Option<String>>(8)?;
            let after_kind = row.get::<_, String>(9)?;
            let after_text = row.get::<_, Option<String>>(10)?;
            record.files.push(AgentTurnFileChange {
                path,
                before: AgentTurnFileContent::from_storage(&before_kind, before_text)
                    .map_err(invalid_input)?,
                after: AgentTurnFileContent::from_storage(&after_kind, after_text)
                    .map_err(invalid_input)?,
            });
        }
    }

    Ok(requested_ids
        .into_iter()
        .filter_map(|assistant_message_id| records.remove(&assistant_message_id))
        .collect())
}

pub(crate) fn list_fork_copies_for_messages(
    connection: &Connection,
    conversation_id: &str,
    source_message_ids: &[String],
) -> rusqlite::Result<Vec<AgentTurnDiffForkCopy>> {
    // A raw position cutoff also includes immutable records hidden by edit/resend. Evidence
    // must use the same selected message set as the fork's messages, traces, and attachments.
    let source_message_ids_json = serde_json::to_string(source_message_ids)
        .map_err(|error| invalid_input(error.to_string()))?;

    let mut statement = connection.prepare(
        "
        SELECT
            turn.run_id,
            turn.conversation_id,
            turn.assistant_message_id,
            turn.project_id,
            turn.workspace_root,
            turn.truncated,
            turn.created_at,
            turn.updated_at
        FROM agent_turn_diffs AS turn
        INNER JOIN messages AS message
            ON message.id = turn.assistant_message_id
        WHERE turn.conversation_id = ?1
          AND message.conversation_id = ?1
          AND message.id IN (SELECT value FROM json_each(?2))
          AND turn.schema_version = ?3
        ORDER BY
            message.position ASC,
            message.created_at ASC,
            turn.assistant_message_id ASC
        ",
    )?;
    let mut copies = statement
        .query_map(
            params![
                conversation_id,
                source_message_ids_json,
                i64::from(AGENT_TURN_DIFF_SCHEMA_VERSION)
            ],
            |row| {
                Ok(AgentTurnDiffForkCopy {
                    record: AgentTurnDiffRecord {
                        identity: AgentTurnDiffIdentity {
                            run_id: row.get(0)?,
                            conversation_id: row.get(1)?,
                            assistant_message_id: row.get(2)?,
                            project_id: row.get(3)?,
                            workspace_root: row.get(4)?,
                        },
                        files: Vec::new(),
                        truncated: row.get(5)?,
                    },
                    actions: Vec::new(),
                    created_at: row.get(6)?,
                    updated_at: row.get(7)?,
                })
            },
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let copy_indexes = copies
        .iter()
        .enumerate()
        .map(|(index, copy)| (copy.record.identity.assistant_message_id.clone(), index))
        .collect::<HashMap<_, _>>();

    let mut files_statement = connection.prepare(
        "
        SELECT
            file.assistant_message_id,
            file.path,
            file.before_kind,
            file.before_text,
            file.after_kind,
            file.after_text
        FROM agent_turn_diff_files AS file
        INNER JOIN agent_turn_diffs AS turn
            ON turn.assistant_message_id = file.assistant_message_id
        INNER JOIN messages AS message
            ON message.id = turn.assistant_message_id
        WHERE turn.conversation_id = ?1
          AND message.conversation_id = ?1
          AND message.id IN (SELECT value FROM json_each(?2))
          AND turn.schema_version = ?3
        ORDER BY
            message.position ASC,
            file.path COLLATE NOCASE ASC,
            file.path ASC
        ",
    )?;
    let files = files_statement
        .query_map(
            params![
                conversation_id,
                source_message_ids_json,
                i64::from(AGENT_TURN_DIFF_SCHEMA_VERSION)
            ],
            |row| {
                let before_kind = row.get::<_, String>(2)?;
                let before_text = row.get::<_, Option<String>>(3)?;
                let after_kind = row.get::<_, String>(4)?;
                let after_text = row.get::<_, Option<String>>(5)?;
                Ok((
                    row.get::<_, String>(0)?,
                    AgentTurnFileChange {
                        path: row.get(1)?,
                        before: AgentTurnFileContent::from_storage(&before_kind, before_text)
                            .map_err(invalid_input)?,
                        after: AgentTurnFileContent::from_storage(&after_kind, after_text)
                            .map_err(invalid_input)?,
                    },
                ))
            },
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (assistant_message_id, file) in files {
        let index = copy_indexes
            .get(&assistant_message_id)
            .copied()
            .ok_or_else(|| invalid_input("fork turn diff file is missing its parent turn"))?;
        copies[index].record.files.push(file);
    }

    let mut actions_statement = connection.prepare(
        "
        SELECT
            action.assistant_message_id,
            action.action_id,
            action.created_at
        FROM agent_turn_diff_actions AS action
        INNER JOIN agent_turn_diffs AS turn
            ON turn.assistant_message_id = action.assistant_message_id
        INNER JOIN messages AS message
            ON message.id = turn.assistant_message_id
        WHERE turn.conversation_id = ?1
          AND message.conversation_id = ?1
          AND message.id IN (SELECT value FROM json_each(?2))
          AND turn.schema_version = ?3
        ORDER BY
            message.position ASC,
            action.created_at ASC,
            action.action_id ASC
        ",
    )?;
    let actions = actions_statement
        .query_map(
            params![
                conversation_id,
                source_message_ids_json,
                i64::from(AGENT_TURN_DIFF_SCHEMA_VERSION)
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    AgentTurnDiffForkAction {
                        action_id: row.get(1)?,
                        created_at: row.get(2)?,
                    },
                ))
            },
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (assistant_message_id, action) in actions {
        let index = copy_indexes
            .get(&assistant_message_id)
            .copied()
            .ok_or_else(|| invalid_input("fork turn diff action is missing its parent turn"))?;
        copies[index].actions.push(action);
    }

    Ok(copies)
}

pub(crate) fn insert_fork_copy(
    connection: &Connection,
    copy: &AgentTurnDiffForkCopy,
) -> rusqlite::Result<()> {
    validate_identity(&copy.record.identity)?;
    validate_assistant_message(connection, &copy.record.identity)?;
    if copy.created_at < 0 || copy.updated_at < copy.created_at {
        return Err(invalid_input("fork turn diff timestamps are invalid"));
    }

    connection.execute(
        "
        INSERT INTO agent_turn_diffs (
            assistant_message_id,
            conversation_id,
            run_id,
            project_id,
            workspace_root,
            schema_version,
            truncated,
            created_at,
            updated_at
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
        ",
        params![
            copy.record.identity.assistant_message_id,
            copy.record.identity.conversation_id,
            copy.record.identity.run_id,
            copy.record.identity.project_id,
            copy.record.identity.workspace_root,
            i64::from(AGENT_TURN_DIFF_SCHEMA_VERSION),
            copy.record.truncated,
            copy.created_at,
            copy.updated_at,
        ],
    )?;

    for file in &copy.record.files {
        validate_change(file)?;
        connection.execute(
            "
            INSERT INTO agent_turn_diff_files (
                assistant_message_id,
                path,
                before_kind,
                before_text,
                after_kind,
                after_text
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            ",
            params![
                copy.record.identity.assistant_message_id,
                file.path,
                file.before.kind(),
                file.before.text(),
                file.after.kind(),
                file.after.text(),
            ],
        )?;
    }

    for action in &copy.actions {
        if action.action_id.trim().is_empty() || action.created_at < 0 {
            return Err(invalid_input("fork turn diff action is invalid"));
        }
        connection.execute(
            "
            INSERT INTO agent_turn_diff_actions (
                assistant_message_id,
                action_id,
                created_at
            )
            VALUES (?1, ?2, ?3)
            ",
            params![
                copy.record.identity.assistant_message_id,
                action.action_id,
                action.created_at,
            ],
        )?;
    }

    Ok(())
}

fn insert_turn_if_absent(
    connection: &Connection,
    identity: &AgentTurnDiffIdentity,
    timestamp: i64,
) -> rusqlite::Result<()> {
    connection.execute(
        "
        INSERT INTO agent_turn_diffs (
            assistant_message_id,
            conversation_id,
            run_id,
            project_id,
            workspace_root,
            schema_version,
            truncated,
            created_at,
            updated_at
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, ?7, ?7)
        ON CONFLICT(assistant_message_id) DO NOTHING
        ",
        params![
            identity.assistant_message_id,
            identity.conversation_id,
            identity.run_id,
            identity.project_id,
            identity.workspace_root,
            i64::from(AGENT_TURN_DIFF_SCHEMA_VERSION),
            timestamp,
        ],
    )?;
    Ok(())
}

fn validate_stored_identity(
    connection: &Connection,
    identity: &AgentTurnDiffIdentity,
) -> rusqlite::Result<()> {
    let matches: bool = connection.query_row(
        "
        SELECT
            run_id = ?2
            AND conversation_id = ?3
            AND project_id = ?4
            AND workspace_root = ?5
            AND schema_version = ?6
        FROM agent_turn_diffs
        WHERE assistant_message_id = ?1
        ",
        params![
            identity.assistant_message_id,
            identity.run_id,
            identity.conversation_id,
            identity.project_id,
            identity.workspace_root,
            i64::from(AGENT_TURN_DIFF_SCHEMA_VERSION),
        ],
        |row| row.get(0),
    )?;
    if matches {
        Ok(())
    } else {
        Err(invalid_input(
            "agent turn diff identity cannot change after initialization",
        ))
    }
}

fn validate_assistant_message(
    connection: &Connection,
    identity: &AgentTurnDiffIdentity,
) -> rusqlite::Result<()> {
    let role = connection
        .query_row(
            "
            SELECT role
            FROM messages
            WHERE id = ?1 AND conversation_id = ?2
            ",
            params![identity.assistant_message_id, identity.conversation_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    match role.as_deref() {
        Some("assistant") => Ok(()),
        Some(_) => Err(invalid_input(
            "agent turn diff message must have the assistant role",
        )),
        None => Err(invalid_input(
            "agent turn diff assistant message does not exist in its conversation",
        )),
    }
}

fn validate_identity(identity: &AgentTurnDiffIdentity) -> rusqlite::Result<()> {
    if [
        &identity.run_id,
        &identity.conversation_id,
        &identity.assistant_message_id,
        &identity.project_id,
        &identity.workspace_root,
    ]
    .into_iter()
    .any(|value| value.trim().is_empty())
    {
        return Err(invalid_input("agent turn diff identity cannot be empty"));
    }
    Ok(())
}

fn validate_change(change: &AgentTurnFileChange) -> rusqlite::Result<()> {
    let path = Path::new(&change.path);
    if change.path.trim().is_empty()
        || path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(invalid_input(
            "agent turn diff file path must be workspace-relative",
        ));
    }
    Ok(())
}

fn load_file_change(
    connection: &Connection,
    assistant_message_id: &str,
    path: &str,
) -> rusqlite::Result<Option<AgentTurnFileChange>> {
    connection
        .query_row(
            "
            SELECT before_kind, before_text, after_kind, after_text
            FROM agent_turn_diff_files
            WHERE assistant_message_id = ?1 AND path = ?2
            ",
            params![assistant_message_id, path],
            |row| {
                let before_kind = row.get::<_, String>(0)?;
                let before_text = row.get::<_, Option<String>>(1)?;
                let after_kind = row.get::<_, String>(2)?;
                let after_text = row.get::<_, Option<String>>(3)?;
                let before = AgentTurnFileContent::from_storage(&before_kind, before_text)
                    .map_err(invalid_input)?;
                let after = AgentTurnFileContent::from_storage(&after_kind, after_text)
                    .map_err(invalid_input)?;
                Ok(AgentTurnFileChange {
                    path: path.to_string(),
                    before,
                    after,
                })
            },
        )
        .optional()
}

fn change_text_bytes(change: &AgentTurnFileChange) -> i64 {
    [change.before.text(), change.after.text()]
        .into_iter()
        .flatten()
        .map(|content| i64::try_from(content.len()).unwrap_or(i64::MAX))
        .fold(0i64, i64::saturating_add)
}

fn degrade_text_content(content: &mut AgentTurnFileContent) {
    if matches!(content, AgentTurnFileContent::Text(_)) {
        *content = AgentTurnFileContent::TooLarge;
    }
}

fn mark_turn_truncated(
    connection: &Connection,
    assistant_message_id: &str,
    updated_at: i64,
) -> rusqlite::Result<()> {
    connection.execute(
        "
        UPDATE agent_turn_diffs
        SET truncated = 1, updated_at = MAX(updated_at, ?2)
        WHERE assistant_message_id = ?1
        ",
        params![assistant_message_id, updated_at],
    )?;
    Ok(())
}

fn touch_turn(
    connection: &Connection,
    assistant_message_id: &str,
    updated_at: i64,
) -> rusqlite::Result<()> {
    connection.execute(
        "
        UPDATE agent_turn_diffs
        SET updated_at = MAX(updated_at, ?2)
        WHERE assistant_message_id = ?1
        ",
        params![assistant_message_id, updated_at],
    )?;
    Ok(())
}

fn invalid_input(message: impl Into<String>) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        message.into(),
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::migrations;

    #[test]
    fn aggregates_one_turn_idempotently_and_does_not_fall_back_past_an_empty_turn() {
        let mut connection = Connection::open_in_memory().unwrap();
        migrations::run_migrations(&connection).unwrap();
        connection
            .execute(
                "
                INSERT INTO projects (id, name, path, created_at, updated_at)
                VALUES ('project-1', 'Project', '/tmp/project-1', 1, 1)
                ",
                [],
            )
            .unwrap();
        connection
            .execute(
                "
                INSERT INTO conversations (id, project_id, title, created_at, updated_at)
                VALUES ('conversation-1', 'project-1', 'Conversation', 1, 1)
                ",
                [],
            )
            .unwrap();
        for (position, message_id) in [(0, "assistant-1"), (1, "assistant-2")] {
            connection
                .execute(
                    "
                    INSERT INTO messages (
                        id, conversation_id, role, content, created_at, position
                    )
                    VALUES (?1, 'conversation-1', 'assistant', '', ?2, ?3)
                    ",
                    params![message_id, position + 1, position],
                )
                .unwrap();
        }

        let first_identity = identity("run-1", "assistant-1");
        initialize_turn(&connection, &first_identity, 1).unwrap();
        assert!(record_file_change(
            &mut connection,
            &first_identity,
            "action-1",
            &text_change("src/main.rs", "before\n", "middle\n"),
            2,
        )
        .unwrap());
        assert!(record_file_change(
            &mut connection,
            &first_identity,
            "action-2",
            &text_change("src/main.rs", "middle\n", "after\n"),
            3,
        )
        .unwrap());
        assert!(!record_file_change(
            &mut connection,
            &first_identity,
            "action-2",
            &text_change("src/main.rs", "after\n", "ignored\n"),
            4,
        )
        .unwrap());

        let first = load_latest_turn(&connection, "conversation-1", "project-1")
            .unwrap()
            .unwrap();
        assert_eq!(
            first.files,
            vec![text_change("src/main.rs", "before\n", "after\n")]
        );

        assert!(record_file_change(
            &mut connection,
            &first_identity,
            "action-3",
            &text_change("src/main.rs", "after\n", "before\n"),
            5,
        )
        .unwrap());
        assert!(load_latest_turn(&connection, "conversation-1", "project-1")
            .unwrap()
            .unwrap()
            .files
            .is_empty());

        let second_identity = identity("run-2", "assistant-2");
        initialize_turn(&connection, &second_identity, 6).unwrap();
        let latest = load_latest_turn(&connection, "conversation-1", "project-1")
            .unwrap()
            .unwrap();
        assert_eq!(latest.identity.assistant_message_id, "assistant-2");
        assert!(latest.files.is_empty());
    }

    #[test]
    fn loads_turns_in_requested_order_without_duplicate_summaries() {
        let mut connection = Connection::open_in_memory().unwrap();
        migrations::run_migrations(&connection).unwrap();
        connection
            .execute(
                "
                INSERT INTO projects (id, name, path, created_at, updated_at)
                VALUES ('project-1', 'Project', '/tmp/project-1', 1, 1)
                ",
                [],
            )
            .unwrap();
        connection
            .execute(
                "
                INSERT INTO conversations (id, project_id, title, created_at, updated_at)
                VALUES ('conversation-1', 'project-1', 'Conversation', 1, 1)
                ",
                [],
            )
            .unwrap();
        for (position, message_id) in [(0, "assistant-1"), (1, "assistant-2")] {
            connection
                .execute(
                    "
                    INSERT INTO messages (
                        id, conversation_id, role, content, created_at, position
                    )
                    VALUES (?1, 'conversation-1', 'assistant', '', ?2, ?3)
                    ",
                    params![message_id, position + 1, position],
                )
                .unwrap();
        }

        for (run_id, message_id, path) in [
            ("run-1", "assistant-1", "first.txt"),
            ("run-2", "assistant-2", "second.txt"),
        ] {
            let identity = identity(run_id, message_id);
            initialize_turn(&connection, &identity, 1).unwrap();
            record_file_change(
                &mut connection,
                &identity,
                &format!("action-{message_id}"),
                &text_change(path, "before\n", "after\n"),
                2,
            )
            .unwrap();
        }

        let records = load_turns_for_messages(
            &connection,
            "conversation-1",
            "project-1",
            &[
                "assistant-2".to_string(),
                "missing".to_string(),
                "assistant-1".to_string(),
                "assistant-2".to_string(),
            ],
        )
        .unwrap();

        assert_eq!(
            records
                .iter()
                .map(|record| record.identity.assistant_message_id.as_str())
                .collect::<Vec<_>>(),
            vec!["assistant-2", "assistant-1"]
        );
        assert_eq!(records[0].files[0].path, "second.txt");
        assert_eq!(records[1].files[0].path, "first.txt");
        assert!(load_turns_for_messages(
            &connection,
            "conversation-1",
            "missing-project",
            &["assistant-1".to_string()],
        )
        .unwrap()
        .is_empty());
    }

    fn identity(run_id: &str, assistant_message_id: &str) -> AgentTurnDiffIdentity {
        AgentTurnDiffIdentity {
            run_id: run_id.to_string(),
            conversation_id: "conversation-1".to_string(),
            assistant_message_id: assistant_message_id.to_string(),
            project_id: "project-1".to_string(),
            workspace_root: "/tmp/project-1".to_string(),
        }
    }

    fn text_change(path: &str, before: &str, after: &str) -> AgentTurnFileChange {
        AgentTurnFileChange {
            path: path.to_string(),
            before: AgentTurnFileContent::Text(before.to_string()),
            after: AgentTurnFileContent::Text(after.to_string()),
        }
    }
}
