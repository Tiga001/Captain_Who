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
            completed_at,
            effective_permissions_json,
            path_scope,
            command_cwd_scope,
            blocked_reason,
            decision_source
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21)
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
            completed_at = excluded.completed_at,
            effective_permissions_json = excluded.effective_permissions_json,
            path_scope = excluded.path_scope,
            command_cwd_scope = excluded.command_cwd_scope,
            blocked_reason = excluded.blocked_reason,
            decision_source = excluded.decision_source
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
            &record.effective_permissions_json,
            &record.path_scope,
            &record.command_cwd_scope,
            &record.blocked_reason,
            &record.decision_source,
        ],
    )?;
    Ok(())
}

pub fn delete_action_audit_for_conversation(
    connection: &Connection,
    conversation_id: &str,
) -> rusqlite::Result<()> {
    connection.execute(
        "DELETE FROM agent_action_audit WHERE conversation_id = ?1",
        params![conversation_id],
    )?;
    Ok(())
}

pub fn delete_action_audit_for_project(
    connection: &Connection,
    project_id: &str,
) -> rusqlite::Result<()> {
    connection.execute(
        "
        DELETE FROM agent_action_audit
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
            effective_permissions_json: Some(r#"{"write":"workspace_only"}"#.to_string()),
            path_scope: Some("workspace".to_string()),
            command_cwd_scope: Some("workspace".to_string()),
            blocked_reason: None,
            decision_source: Some("manual".to_string()),
        };
        upsert_action_audit_record(&connection, &record).unwrap();

        record.status = "completed".to_string();
        record.completed_at = Some(3);
        upsert_action_audit_record(&connection, &record).unwrap();

        let (status, decision_source): (String, Option<String>) = connection
            .query_row(
                "SELECT status, decision_source FROM agent_action_audit WHERE action_id = 'action-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, "completed");
        assert_eq!(decision_source.as_deref(), Some("manual"));
    }
}
