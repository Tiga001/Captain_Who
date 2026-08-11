use crate::{
    ConversationGoal, ConversationGoalMutationActor, ConversationGoalRevision,
    ConversationGoalRevisionEvent, ConversationGoalStatus,
    CONVERSATION_GOAL_REVISION_SCHEMA_VERSION,
};
use rusqlite::types::Type;
use rusqlite::{params, Connection, OptionalExtension, Row, Transaction};
use uuid::Uuid;

pub(crate) fn get_goal(
    connection: &Connection,
    conversation_id: &str,
) -> rusqlite::Result<Option<ConversationGoal>> {
    connection
        .query_row(
            "SELECT goal_id, conversation_id, objective, source_message_id, status,
                    stopped_reason, created_at, updated_at
             FROM conversation_goals
             WHERE conversation_id = ?1",
            [conversation_id],
            goal_from_row,
        )
        .optional()
}

pub(crate) fn get_visible_goal(
    connection: &Connection,
    conversation_id: &str,
) -> rusqlite::Result<Option<ConversationGoal>> {
    connection
        .query_row(
            "SELECT goal_id, conversation_id, objective, source_message_id, status,
                    stopped_reason, created_at, updated_at
             FROM conversation_goals
             WHERE conversation_id = ?1
               AND status IN ('active', 'blocked')",
            [conversation_id],
            goal_from_row,
        )
        .optional()
}

pub(crate) fn create_goal(
    connection: &Connection,
    actor: ConversationGoalMutationActor,
    conversation_id: &str,
    objective: &str,
    now: i64,
) -> rusqlite::Result<Option<ConversationGoal>> {
    let transaction = connection.unchecked_transaction()?;
    let goal_id = format!("goal-{}", Uuid::new_v4());
    let goal = transaction
        .query_row(
            "INSERT INTO conversation_goals (
                conversation_id, goal_id, objective, source_message_id, status,
                stopped_reason, created_at, updated_at
             )
             SELECT ?1, ?2, ?3, message.id, 'active', NULL, ?4, ?4
             FROM messages AS message
             WHERE message.conversation_id = ?1
               AND message.role = 'user'
             ORDER BY message.position DESC
             LIMIT 1
             ON CONFLICT(conversation_id) DO UPDATE SET
                goal_id = excluded.goal_id,
                objective = excluded.objective,
                source_message_id = excluded.source_message_id,
                status = 'active',
                stopped_reason = NULL,
                created_at = excluded.created_at,
                updated_at = excluded.updated_at
             WHERE conversation_goals.status IN ('completed', 'cancelled')
             RETURNING goal_id, conversation_id, objective, source_message_id, status,
                       stopped_reason, created_at, updated_at",
            params![conversation_id, goal_id, objective, now],
            goal_from_row,
        )
        .optional()?;
    if let Some(goal) = goal.as_ref() {
        append_revision(
            &transaction,
            goal,
            Some(actor),
            &ConversationGoalRevisionEvent::Initial { goal: goal.clone() },
            now,
        )?;
    }
    transaction.commit()?;
    Ok(goal)
}

pub(crate) fn update_goal_status(
    connection: &Connection,
    actor: ConversationGoalMutationActor,
    conversation_id: &str,
    status: ConversationGoalStatus,
    stopped_reason: Option<&str>,
    now: i64,
) -> rusqlite::Result<Option<ConversationGoal>> {
    let transaction = connection.unchecked_transaction()?;
    let Some(previous) = get_goal(&transaction, conversation_id)? else {
        transaction.commit()?;
        return Ok(None);
    };
    require_initial_revision(&transaction, &previous)?;
    let goal = transaction
        .query_row(
            "UPDATE conversation_goals
             SET status = ?2, stopped_reason = ?3, updated_at = ?4
             WHERE conversation_id = ?1
               AND ?4 >= updated_at
               AND (
                    (status = 'active' AND ?2 IN ('blocked', 'completed', 'cancelled'))
                    OR (status = 'blocked' AND ?2 IN ('active', 'cancelled'))
               )
             RETURNING goal_id, conversation_id, objective, source_message_id, status,
                       stopped_reason, created_at, updated_at",
            params![conversation_id, status.as_str(), stopped_reason, now],
            goal_from_row,
        )
        .optional()?;
    if let Some(goal) = goal.as_ref() {
        append_revision(
            &transaction,
            goal,
            Some(actor),
            &ConversationGoalRevisionEvent::StatusChanged {
                previous_status: previous.status,
                status: goal.status,
                previous_stopped_reason: previous.stopped_reason,
                stopped_reason: goal.stopped_reason.clone(),
                updated_at: goal.updated_at,
            },
            now,
        )?;
    }
    transaction.commit()?;
    Ok(goal)
}

/// Reserved host boundary for an explicit model or user objective edit.
///
/// The current model tool intentionally does not expose this yet. A future user UI can use it
/// without inventing a second persistence path.
pub(crate) fn update_goal_objective(
    connection: &Connection,
    actor: ConversationGoalMutationActor,
    conversation_id: &str,
    objective: &str,
    now: i64,
) -> rusqlite::Result<Option<ConversationGoal>> {
    let transaction = connection.unchecked_transaction()?;
    let Some(previous) = get_goal(&transaction, conversation_id)? else {
        transaction.commit()?;
        return Ok(None);
    };
    require_initial_revision(&transaction, &previous)?;
    let goal = transaction
        .query_row(
            "UPDATE conversation_goals
             SET objective = ?2, updated_at = ?3
             WHERE conversation_id = ?1
               AND status IN ('active', 'blocked')
               AND objective != ?2
               AND ?3 >= updated_at
             RETURNING goal_id, conversation_id, objective, source_message_id, status,
                       stopped_reason, created_at, updated_at",
            params![conversation_id, objective, now],
            goal_from_row,
        )
        .optional()?;
    if let Some(goal) = goal.as_ref() {
        append_revision(
            &transaction,
            goal,
            Some(actor),
            &ConversationGoalRevisionEvent::ObjectiveChanged {
                previous_objective: previous.objective,
                objective: goal.objective.clone(),
                updated_at: goal.updated_at,
            },
            now,
        )?;
    }
    transaction.commit()?;
    Ok(goal)
}

pub(crate) fn list_goal_revisions(
    connection: &Connection,
    conversation_id: &str,
    goal_id: &str,
) -> rusqlite::Result<Vec<ConversationGoalRevision>> {
    let mut statement = connection.prepare(
        "SELECT conversation_id, goal_id, sequence, schema_version, actor,
                event_kind, event_json, created_at
         FROM conversation_goal_revisions
         WHERE conversation_id = ?1 AND goal_id = ?2
         ORDER BY sequence",
    )?;
    let revisions = statement
        .query_map(params![conversation_id, goal_id], revision_from_row)?
        .collect();
    revisions
}

fn require_initial_revision(
    connection: &Transaction<'_>,
    goal: &ConversationGoal,
) -> rusqlite::Result<()> {
    let exists = connection.query_row(
        "SELECT EXISTS (
            SELECT 1 FROM conversation_goal_revisions
            WHERE goal_id = ?1 AND sequence = 1
         )",
        [&goal.goal_id],
        |row| row.get::<_, bool>(0),
    )?;
    if exists {
        Ok(())
    } else {
        Err(rusqlite::Error::InvalidQuery)
    }
}

fn append_revision(
    connection: &Transaction<'_>,
    goal: &ConversationGoal,
    actor: Option<ConversationGoalMutationActor>,
    event: &ConversationGoalRevisionEvent,
    created_at: i64,
) -> rusqlite::Result<()> {
    let sequence = connection.query_row(
        "SELECT COALESCE(MAX(sequence), 0) + 1
         FROM conversation_goal_revisions
         WHERE goal_id = ?1",
        [&goal.goal_id],
        |row| row.get::<_, i64>(0),
    )?;
    let event_json = serde_json::to_string(event)
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
    connection.execute(
        "INSERT INTO conversation_goal_revisions (
            conversation_id, goal_id, sequence, schema_version, actor,
            event_kind, event_json, created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            goal.conversation_id,
            goal.goal_id,
            sequence,
            i64::from(CONVERSATION_GOAL_REVISION_SCHEMA_VERSION),
            actor.map(ConversationGoalMutationActor::as_str),
            event.kind(),
            event_json,
            created_at,
        ],
    )?;
    Ok(())
}

fn revision_from_row(row: &Row<'_>) -> rusqlite::Result<ConversationGoalRevision> {
    let actor = match row.get::<_, Option<String>>(4)?.as_deref() {
        Some("model") => Some(ConversationGoalMutationActor::Model),
        Some("user") => Some(ConversationGoalMutationActor::User),
        None => None,
        Some(value) => {
            return Err(rusqlite::Error::FromSqlConversionFailure(
                4,
                Type::Text,
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("unknown goal revision actor `{value}`"),
                )
                .into(),
            ));
        }
    };
    let event_kind = row.get::<_, String>(5)?;
    let event_json = row.get::<_, String>(6)?;
    let event =
        serde_json::from_str::<ConversationGoalRevisionEvent>(&event_json).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(6, Type::Text, Box::new(error))
        })?;
    if event.kind() != event_kind {
        return Err(rusqlite::Error::FromSqlConversionFailure(
            5,
            Type::Text,
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "goal revision event kind does not match event JSON",
            )
            .into(),
        ));
    }
    let schema_version = row.get::<_, i64>(3)?;
    let sequence = row.get::<_, i64>(2)?;
    Ok(ConversationGoalRevision {
        schema_version: u32::try_from(schema_version).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(3, Type::Integer, Box::new(error))
        })?,
        conversation_id: row.get(0)?,
        goal_id: row.get(1)?,
        sequence: u64::try_from(sequence).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(2, Type::Integer, Box::new(error))
        })?,
        actor,
        event,
        created_at: row.get(7)?,
    })
}

fn goal_from_row(row: &Row<'_>) -> rusqlite::Result<ConversationGoal> {
    let status = match row.get::<_, String>(4)?.as_str() {
        "active" => ConversationGoalStatus::Active,
        "blocked" => ConversationGoalStatus::Blocked,
        "completed" => ConversationGoalStatus::Completed,
        "cancelled" => ConversationGoalStatus::Cancelled,
        value => {
            return Err(rusqlite::Error::FromSqlConversionFailure(
                4,
                Type::Text,
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("unknown conversation goal status `{value}`"),
                )
                .into(),
            ));
        }
    };
    Ok(ConversationGoal {
        goal_id: row.get(0)?,
        conversation_id: row.get(1)?,
        objective: row.get(2)?,
        source_message_id: row.get(3)?,
        status,
        stopped_reason: row.get(5)?,
        created_at: row.get(6)?,
        updated_at: row.get(7)?,
    })
}
