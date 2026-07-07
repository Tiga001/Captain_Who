// Rust core storage agent action audit log.
use crate::storage::models::AgentActionAuditRecord;
use rusqlite::{params, Connection};

pub fn upsert_action_audit_record(
    connection: &Connection,
    record: &AgentActionAuditRecord,
) -> rusqlite::Result<()> {
    connection.execute(
        "
        INSERT INTO agent_action_audit (
            action_id,
            run_id,
            conversation_id,
            assistant_message_id,
            action_type,
            tool_name,
            decision,
            status,
            action_json,
            patch_result_json,
            command_result_json,
            tool_result_json,
            error,
            created_at,
            decided_at,
            completed_at
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
        ON CONFLICT(action_id) DO UPDATE SET
            run_id = excluded.run_id,
            conversation_id = excluded.conversation_id,
            assistant_message_id = excluded.assistant_message_id,
            action_type = excluded.action_type,
            tool_name = excluded.tool_name,
            decision = excluded.decision,
            status = excluded.status,
            action_json = excluded.action_json,
            patch_result_json = excluded.patch_result_json,
            command_result_json = excluded.command_result_json,
            tool_result_json = excluded.tool_result_json,
            error = excluded.error,
            created_at = excluded.created_at,
            decided_at = excluded.decided_at,
            completed_at = excluded.completed_at
        ",
        params![
            &record.action_id,
            &record.run_id,
            &record.conversation_id,
            &record.assistant_message_id,
            &record.action_type,
            &record.tool_name,
            &record.decision,
            &record.status,
            &record.action_json,
            &record.patch_result_json,
            &record.command_result_json,
            &record.tool_result_json,
            &record.error,
            record.created_at,
            record.decided_at,
            record.completed_at,
        ],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::migrations;

    #[test]
    fn upserts_action_audit_record() {
        let connection = Connection::open_in_memory().unwrap();
        migrations::run_migrations(&connection).unwrap();

        let mut record = AgentActionAuditRecord {
            action_id: "action-1".to_string(),
            run_id: "run-1".to_string(),
            conversation_id: Some("conversation-1".to_string()),
            assistant_message_id: Some("message-1".to_string()),
            action_type: "command".to_string(),
            tool_name: "run_command".to_string(),
            decision: Some("approved".to_string()),
            status: "approved".to_string(),
            action_json: "{}".to_string(),
            patch_result_json: None,
            command_result_json: None,
            tool_result_json: None,
            error: None,
            created_at: 1,
            decided_at: Some(2),
            completed_at: None,
        };
        upsert_action_audit_record(&connection, &record).unwrap();

        record.status = "completed".to_string();
        record.completed_at = Some(3);
        upsert_action_audit_record(&connection, &record).unwrap();

        let status: String = connection
            .query_row(
                "SELECT status FROM agent_action_audit WHERE action_id = 'action-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(status, "completed");
    }
}
