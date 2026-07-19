use crate::storage::models::AgentPendingActionRecord;
use rusqlite::{params, Connection};

/// Result of publishing an immutable pending-action snapshot.
///
/// `action_id` is the durable idempotency key. Replaying the exact same frozen
/// identity is harmless, while reusing the id for another run, action, or
/// continuation is a conflict and must never replace the original snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PendingActionStoreOutcome {
    Inserted,
    Idempotent,
    Conflict {
        existing_run_id: String,
        existing_status: String,
    },
}

pub fn store_pending_action(
    connection: &Connection,
    record: &AgentPendingActionRecord,
) -> rusqlite::Result<PendingActionStoreOutcome> {
    let affected = connection.execute(
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
            target_status,
            action_json,
            agent_input_json,
            created_at,
            updated_at
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
        ON CONFLICT(action_id) DO NOTHING
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
            &record.target_status,
            &record.action_json,
            &record.agent_input_json,
            record.created_at,
            record.updated_at,
        ],
    )?;
    if affected == 1 {
        return Ok(PendingActionStoreOutcome::Inserted);
    }

    let existing = connection.query_row(
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
            target_status,
            action_json,
            agent_input_json,
            created_at,
            updated_at
        FROM agent_pending_actions
        WHERE action_id = ?1
        ",
        [&record.action_id],
        |row| {
            Ok(AgentPendingActionRecord {
                action_id: row.get(0)?,
                run_id: row.get(1)?,
                conversation_id: row.get(2)?,
                assistant_message_id: row.get(3)?,
                action_type: row.get(4)?,
                tool_name: row.get(5)?,
                tool_call_id: row.get(6)?,
                status: row.get(7)?,
                target_status: row.get(8)?,
                action_json: row.get(9)?,
                agent_input_json: row.get(10)?,
                created_at: row.get(11)?,
                updated_at: row.get(12)?,
            })
        },
    )?;

    if same_frozen_identity(&existing, record) {
        Ok(PendingActionStoreOutcome::Idempotent)
    } else {
        Ok(PendingActionStoreOutcome::Conflict {
            existing_run_id: existing.run_id,
            existing_status: existing.status,
        })
    }
}

fn same_frozen_identity(
    existing: &AgentPendingActionRecord,
    candidate: &AgentPendingActionRecord,
) -> bool {
    existing.action_id == candidate.action_id
        && existing.run_id == candidate.run_id
        && existing.conversation_id == candidate.conversation_id
        && existing.assistant_message_id == candidate.assistant_message_id
        && existing.action_type == candidate.action_type
        && existing.tool_name == candidate.tool_name
        && existing.tool_call_id == candidate.tool_call_id
        && existing.status == candidate.status
        && existing.action_json == candidate.action_json
        && existing.agent_input_json == candidate.agent_input_json
}

pub fn list_interrupted_actions(
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
            target_status,
            action_json,
            agent_input_json,
            created_at,
            updated_at
        FROM agent_pending_actions
        WHERE status IN ('approved', 'executing')
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
                target_status: row.get(8)?,
                action_json: row.get(9)?,
                agent_input_json: row.get(10)?,
                created_at: row.get(11)?,
                updated_at: row.get(12)?,
            })
        })?
        .collect();
    records
}

/// Marks commands which had crossed the approval boundary before process exit
/// as failed. They are deliberately not replayed because their external side
/// effects cannot be proven absent after an unclean shutdown.
pub fn fail_interrupted_actions(
    connection: &Connection,
    updated_at: i64,
) -> rusqlite::Result<usize> {
    connection.execute(
        "
        UPDATE agent_pending_actions
        SET status = 'failed',
            agent_input_json = '{}',
            updated_at = ?1
        WHERE status IN ('approved', 'executing')
        ",
        [updated_at],
    )
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
            target_status,
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
                target_status: row.get(8)?,
                action_json: row.get(9)?,
                agent_input_json: row.get(10)?,
                created_at: row.get(11)?,
                updated_at: row.get(12)?,
            })
        })?
        .collect();
    records
}

/// Atomically replaces both lifecycle status and its persisted resume payload. Keeping these
/// columns in one statement prevents a terminal row from becoming visible while it still carries
/// the prior run-scoped payload.
pub fn transition_pending_action(
    connection: &Connection,
    action_id: &str,
    expected_status: &str,
    status: &str,
    agent_input_json: &str,
    updated_at: i64,
) -> rusqlite::Result<usize> {
    connection.execute(
        "
        UPDATE agent_pending_actions
        SET status = ?3,
            target_status = CASE WHEN ?3 = 'pending' THEN NULL ELSE target_status END,
            agent_input_json = ?4,
            updated_at = ?5
        WHERE action_id = ?1
          AND status = ?2
          AND (
              ?3 = 'pending'
              OR (?3 IN ('approved', 'executing') AND target_status IS NULL)
              OR (?3 IN ('rejected', 'cancelled', 'completed', 'failed') AND target_status = ?3)
          )
        ",
        params![
            action_id,
            expected_status,
            status,
            agent_input_json,
            updated_at
        ],
    )
}

pub fn set_pending_action_target_status(
    connection: &Connection,
    action_id: &str,
    expected_status: &str,
    target_status: &str,
    updated_at: i64,
) -> rusqlite::Result<usize> {
    connection.execute(
        "
        UPDATE agent_pending_actions
        SET target_status = ?3,
            updated_at = ?4
        WHERE action_id = ?1
          AND status = ?2
          AND ?3 IN ('rejected', 'cancelled', 'completed', 'failed')
          AND (target_status IS NULL OR target_status = ?3)
        ",
        params![action_id, expected_status, target_status, updated_at],
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
    fn stores_lists_and_updates_pending_actions() {
        const INSTRUCTION_MARKER: &str = "PERSISTED_SKILL_BODY_MUST_BE_REDACTED";
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
            target_status: None,
            action_json: r#"{"type":"tool_call"}"#.to_string(),
            agent_input_json: format!(
                r#"{{"skillActivation":{{"skills":[{{"instructions":"{INSTRUCTION_MARKER}"}}]}}}}"#
            ),
            created_at: 1,
            updated_at: 1,
        };
        assert_eq!(
            store_pending_action(&connection, &record).unwrap(),
            PendingActionStoreOutcome::Inserted
        );
        let mut idempotent_replay = record.clone();
        idempotent_replay.updated_at = 2;
        assert_eq!(
            store_pending_action(&connection, &idempotent_replay).unwrap(),
            PendingActionStoreOutcome::Idempotent
        );

        let pending = list_pending_actions(&connection).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].action_id, "action-1");

        let redacted_input = r#"{"skillActivation":{"activationRevision":"activation-1","skills":[{"id":"skill-1","name":"reviewer","revision":"revision-1","source":"bundled:application","instructions":""}]}}"#;
        assert_eq!(
            set_pending_action_target_status(&connection, "action-1", "pending", "completed", 2,)
                .unwrap(),
            1
        );
        assert_eq!(
            transition_pending_action(&connection, "action-1", "pending", "failed", "{}", 2,)
                .unwrap(),
            0,
            "a terminal CAS must not disagree with the write-ahead action outcome"
        );
        transition_pending_action(
            &connection,
            "action-1",
            "pending",
            "completed",
            redacted_input,
            2,
        )
        .unwrap();
        assert_eq!(
            transition_pending_action(&connection, "action-1", "approved", "failed", "{}", 3,)
                .unwrap(),
            0
        );
        assert!(list_pending_actions(&connection).unwrap().is_empty());

        let (status, agent_input_json, updated_at): (String, String, i64) = connection
            .query_row(
                "SELECT status, agent_input_json, updated_at FROM agent_pending_actions WHERE action_id = ?1",
                ["action-1"],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(status, "completed");
        assert_eq!(agent_input_json, redacted_input);
        assert!(!agent_input_json.contains(INSTRUCTION_MARKER));
        assert_eq!(updated_at, 2);
    }

    #[test]
    fn compensating_transition_to_pending_atomically_clears_target_status() {
        let connection = Connection::open_in_memory().unwrap();
        migrations::run_migrations(&connection).unwrap();
        let record = AgentPendingActionRecord {
            action_id: "cancel-retry".to_string(),
            run_id: "run-1".to_string(),
            conversation_id: Some("conversation-1".to_string()),
            assistant_message_id: Some("message-1".to_string()),
            action_type: "tool_call".to_string(),
            tool_name: "approval_tool".to_string(),
            tool_call_id: Some("cancel-retry".to_string()),
            status: "pending".to_string(),
            target_status: None,
            action_json: "{}".to_string(),
            agent_input_json: r#"{"resume":true}"#.to_string(),
            created_at: 1,
            updated_at: 1,
        };
        assert_eq!(
            store_pending_action(&connection, &record).unwrap(),
            PendingActionStoreOutcome::Inserted
        );
        assert_eq!(
            transition_pending_action(
                &connection,
                "cancel-retry",
                "pending",
                "executing",
                &record.agent_input_json,
                2,
            )
            .unwrap(),
            1
        );
        assert_eq!(
            set_pending_action_target_status(
                &connection,
                "cancel-retry",
                "executing",
                "cancelled",
                3,
            )
            .unwrap(),
            1
        );
        assert_eq!(
            transition_pending_action(
                &connection,
                "cancel-retry",
                "executing",
                "pending",
                &record.agent_input_json,
                4,
            )
            .unwrap(),
            1
        );

        let (status, target_status): (String, Option<String>) = connection
            .query_row(
                "SELECT status, target_status FROM agent_pending_actions WHERE action_id = ?1",
                ["cancel-retry"],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, "pending");
        assert_eq!(target_status, None);
        assert_eq!(
            set_pending_action_target_status(
                &connection,
                "cancel-retry",
                "pending",
                "completed",
                5,
            )
            .unwrap(),
            1
        );
    }

    #[test]
    fn action_id_collision_never_replaces_the_frozen_snapshot() {
        let connection = Connection::open_in_memory().unwrap();
        migrations::run_migrations(&connection).unwrap();
        let original = AgentPendingActionRecord {
            action_id: "shared-action-id".to_string(),
            run_id: "run-original".to_string(),
            conversation_id: Some("conversation-original".to_string()),
            assistant_message_id: Some("message-original".to_string()),
            action_type: "command".to_string(),
            tool_name: "run_command".to_string(),
            tool_call_id: Some("shared-action-id".to_string()),
            status: "pending".to_string(),
            target_status: None,
            action_json: r#"{"command":"printf original"}"#.to_string(),
            agent_input_json: r#"{"resume":"original"}"#.to_string(),
            created_at: 10,
            updated_at: 10,
        };
        assert_eq!(
            store_pending_action(&connection, &original).unwrap(),
            PendingActionStoreOutcome::Inserted
        );

        let mut collision = original.clone();
        collision.run_id = "run-attacker".to_string();
        collision.conversation_id = Some("conversation-attacker".to_string());
        collision.action_json = r#"{"command":"printf replaced"}"#.to_string();
        collision.agent_input_json = r#"{"resume":"attacker"}"#.to_string();
        assert_eq!(
            store_pending_action(&connection, &collision).unwrap(),
            PendingActionStoreOutcome::Conflict {
                existing_run_id: "run-original".to_string(),
                existing_status: "pending".to_string(),
            }
        );

        let persisted = list_pending_actions(&connection).unwrap();
        assert_eq!(persisted, vec![original]);
    }

    #[test]
    fn startup_reconciliation_fails_approved_and_executing_without_replay() {
        let connection = Connection::open_in_memory().unwrap();
        migrations::run_migrations(&connection).unwrap();
        for (action_id, status) in [
            ("approved-action", "approved"),
            ("executing-action", "executing"),
        ] {
            connection
                .execute(
                    "INSERT INTO agent_pending_actions (
                        action_id, run_id, action_type, tool_name, status, action_json,
                        agent_input_json, created_at, updated_at
                     ) VALUES (?1, 'run-1', 'command', 'run_command', ?2, '{}', 'sensitive', 1, 1)",
                    params![action_id, status],
                )
                .unwrap();
        }

        assert_eq!(list_interrupted_actions(&connection).unwrap().len(), 2);
        assert_eq!(fail_interrupted_actions(&connection, 9).unwrap(), 2);
        let reconciled: Vec<(String, String, String, i64)> = connection
            .prepare(
                "SELECT action_id, status, agent_input_json, updated_at
                 FROM agent_pending_actions ORDER BY action_id",
            )
            .unwrap()
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(
            reconciled,
            vec![
                (
                    "approved-action".to_string(),
                    "failed".to_string(),
                    "{}".to_string(),
                    9
                ),
                (
                    "executing-action".to_string(),
                    "failed".to_string(),
                    "{}".to_string(),
                    9
                ),
            ]
        );
    }
}
