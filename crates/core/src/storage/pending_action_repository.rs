// Rust core storage for persisted agent pending approvals.
use crate::storage::models::AgentPendingActionRecord;
use rusqlite::{params, Connection};

pub fn upsert_pending_action(
    connection: &Connection,
    record: &AgentPendingActionRecord,
) -> rusqlite::Result<()> {
    connection.execute(
        "
        INSERT INTO agent_pending_actions (
            action_id,
            run_id,
            conversation_id,
            assistant_message_id,
            action_type,
            tool_name,
            tool_call_id,
            status,
            action_json,
            agent_input_json,
            created_at,
            updated_at
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
        ON CONFLICT(action_id) DO UPDATE SET
            run_id = excluded.run_id,
            conversation_id = excluded.conversation_id,
            assistant_message_id = excluded.assistant_message_id,
            action_type = excluded.action_type,
            tool_name = excluded.tool_name,
            tool_call_id = excluded.tool_call_id,
            status = excluded.status,
            action_json = excluded.action_json,
            agent_input_json = excluded.agent_input_json,
            created_at = excluded.created_at,
            updated_at = excluded.updated_at
        ",
        params![
            &record.action_id,
            &record.run_id,
            &record.conversation_id,
            &record.assistant_message_id,
            &record.action_type,
            &record.tool_name,
            &record.tool_call_id,
            &record.status,
            &record.action_json,
            &record.agent_input_json,
            record.created_at,
            record.updated_at,
        ],
    )?;
    Ok(())
}

pub fn list_pending_actions(
    connection: &Connection,
) -> rusqlite::Result<Vec<AgentPendingActionRecord>> {
    let mut statement = connection.prepare(
        "
        SELECT
            action_id,
            run_id,
            conversation_id,
            assistant_message_id,
            action_type,
            tool_name,
            tool_call_id,
            status,
            action_json,
            agent_input_json,
            created_at,
            updated_at
        FROM agent_pending_actions
        WHERE status = 'pending'
        ORDER BY created_at ASC
        ",
    )?;

    let records = statement
        .query_map([], |row| {
            Ok(AgentPendingActionRecord {
                action_id: row.get(0)?,
                run_id: row.get(1)?,
                conversation_id: row.get(2)?,
                assistant_message_id: row.get(3)?,
                action_type: row.get(4)?,
                tool_name: row.get(5)?,
                tool_call_id: row.get(6)?,
                status: row.get(7)?,
                action_json: row.get(8)?,
                agent_input_json: row.get(9)?,
                created_at: row.get(10)?,
                updated_at: row.get(11)?,
            })
        })?
        .collect();
    records
}

pub fn update_pending_action_status(
    connection: &Connection,
    action_id: &str,
    status: &str,
    updated_at: i64,
) -> rusqlite::Result<usize> {
    connection.execute(
        "
        UPDATE agent_pending_actions
        SET status = ?2, updated_at = ?3
        WHERE action_id = ?1
        ",
        params![action_id, status, updated_at],
    )
}

pub fn delete_pending_actions_for_conversation(
    connection: &Connection,
    conversation_id: &str,
) -> rusqlite::Result<()> {
    connection.execute(
        "DELETE FROM agent_pending_actions WHERE conversation_id = ?1",
        params![conversation_id],
    )?;
    Ok(())
}

pub fn delete_pending_actions_for_project(
    connection: &Connection,
    project_id: &str,
) -> rusqlite::Result<()> {
    connection.execute(
        "
        DELETE FROM agent_pending_actions
        WHERE conversation_id IN (
            SELECT id
            FROM conversations
            WHERE project_id = ?1
        )
        ",
        params![project_id],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::migrations;

    #[test]
    fn upserts_lists_and_updates_pending_actions() {
        let connection = Connection::open_in_memory().unwrap();
        migrations::run_migrations(&connection).unwrap();

        let record = AgentPendingActionRecord {
            action_id: "action-1".to_string(),
            run_id: "run-1".to_string(),
            conversation_id: Some("conversation-1".to_string()),
            assistant_message_id: Some("message-1".to_string()),
            action_type: "diff".to_string(),
            tool_name: "apply_patch".to_string(),
            tool_call_id: Some("action-1".to_string()),
            status: "pending".to_string(),
            action_json: r#"{"type":"tool_call"}"#.to_string(),
            agent_input_json: "{}".to_string(),
            created_at: 1,
            updated_at: 1,
        };
        upsert_pending_action(&connection, &record).unwrap();

        let pending = list_pending_actions(&connection).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].action_id, "action-1");

        update_pending_action_status(&connection, "action-1", "completed", 2).unwrap();
        assert!(list_pending_actions(&connection).unwrap().is_empty());
    }
}
