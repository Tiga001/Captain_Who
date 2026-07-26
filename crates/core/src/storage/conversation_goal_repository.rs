use crate::{ConversationGoal, ConversationGoalStatus};
use rusqlite::{params, Connection, OptionalExtension, Row};
use std::collections::HashMap;
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
    conversation_id: &str,
    objective: &str,
    now: i64,
) -> rusqlite::Result<Option<ConversationGoal>> {
    let goal_id = format!("goal-{}", Uuid::new_v4());
    connection
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
        .optional()
}

pub(crate) fn update_goal_status(
    connection: &Connection,
    conversation_id: &str,
    status: ConversationGoalStatus,
    stopped_reason: Option<&str>,
    now: i64,
) -> rusqlite::Result<Option<ConversationGoal>> {
    connection
        .query_row(
            "UPDATE conversation_goals
             SET status = ?2, stopped_reason = ?3, updated_at = ?4
             WHERE conversation_id = ?1
               AND status = 'active'
             RETURNING goal_id, conversation_id, objective, source_message_id, status,
                       stopped_reason, created_at, updated_at",
            params![conversation_id, status.as_str(), stopped_reason, now],
            goal_from_row,
        )
        .optional()
}

pub(crate) fn cancel_goal(
    connection: &Connection,
    conversation_id: &str,
    now: i64,
) -> rusqlite::Result<Option<ConversationGoal>> {
    connection
        .query_row(
            "UPDATE conversation_goals
             SET status = 'cancelled', stopped_reason = NULL, updated_at = ?2
             WHERE conversation_id = ?1
               AND status IN ('active', 'blocked')
             RETURNING goal_id, conversation_id, objective, source_message_id, status,
                       stopped_reason, created_at, updated_at",
            params![conversation_id, now],
            goal_from_row,
        )
        .optional()
}

/// A new user turn is an explicit opportunity to continue a previously blocked goal. This only
/// reactivates the persistent objective; it never starts a model run on its own.
pub(crate) fn resume_blocked_goal_for_user_turn(
    connection: &Connection,
    conversation_id: &str,
    now: i64,
) -> rusqlite::Result<Option<ConversationGoal>> {
    connection.execute(
        "UPDATE conversation_goals
         SET status = 'active', stopped_reason = NULL, updated_at = ?2
         WHERE conversation_id = ?1
           AND status = 'blocked'",
        params![conversation_id, now],
    )?;
    get_visible_goal(connection, conversation_id)
}

pub(crate) fn clone_visible_goal_for_fork(
    connection: &Connection,
    source_conversation_id: &str,
    target_conversation_id: &str,
    message_id_map: &HashMap<String, String>,
    created_at: i64,
) -> Result<(), String> {
    let Some(source) = get_goal(connection, source_conversation_id).map_err(database_error)? else {
        return Ok(());
    };
    if source.status != ConversationGoalStatus::Active {
        return Ok(());
    }
    let Some(target_source_message_id) = message_id_map.get(&source.source_message_id) else {
        return Ok(());
    };
    connection
        .execute(
            "INSERT INTO conversation_goals (
                conversation_id, goal_id, objective, source_message_id, status,
                stopped_reason, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, NULL, ?6, ?6)",
            params![
                target_conversation_id,
                format!("goal-{}", Uuid::new_v4()),
                source.objective,
                target_source_message_id,
                ConversationGoalStatus::Active.as_str(),
                created_at,
            ],
        )
        .map_err(database_error)?;
    Ok(())
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
                rusqlite::types::Type::Text,
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("unknown conversation goal status `{value}`"),
                )
                .into(),
            ))
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

fn database_error(error: rusqlite::Error) -> String {
    format!("本地数据库操作失败：{error}")
}
