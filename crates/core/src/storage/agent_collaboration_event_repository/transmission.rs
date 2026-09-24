//! Ephemeral presentation routes derived from committed facts; no scheduling or history writes.

use crate::{
    AgentCollaborationEventError, AgentCollaborationEventKind, AgentCollaborationEventRecord,
    AgentCollaborationTransmission, AgentCollaborationTransmissionKind,
};
use rusqlite::{params, Connection, OptionalExtension};

pub(super) fn project(
    connection: &Connection,
    event: &AgentCollaborationEventRecord,
) -> Result<Option<AgentCollaborationTransmission>, AgentCollaborationEventError> {
    match event.kind {
        AgentCollaborationEventKind::MailboxEnqueued => project_mailbox(connection, event),
        AgentCollaborationEventKind::TurnStarted | AgentCollaborationEventKind::TurnUpdated
            if event.agent_id == event.root_agent_id
                && event.conversation_id == event.root_conversation_id =>
        {
            project_root_turn(connection, event)
        }
        _ => Ok(None),
    }
}

fn project_mailbox(
    connection: &Connection,
    event: &AgentCollaborationEventRecord,
) -> Result<Option<AgentCollaborationTransmission>, AgentCollaborationEventError> {
    let Some(message_id) = event.message_id.as_deref() else {
        return Ok(None);
    };
    let routing: Option<(String, String, String)> = connection
        .query_row(
            "SELECT kind, sender_agent_id, recipient_agent_id
         FROM agent_mailbox_messages
         WHERE message_id = ?1 AND root_agent_id = ?2 AND recipient_agent_id = ?3",
            params![message_id, &event.root_agent_id, &event.agent_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|_| AgentCollaborationEventError::StorageUnavailable)?;
    let Some((kind, sender, recipient)) = routing else {
        return Ok(None);
    };
    let kind = match kind.as_str() {
        "message" => AgentCollaborationTransmissionKind::Message,
        "task" | "followup" => AgentCollaborationTransmissionKind::Task,
        // Automatic terminal receipts are state notifications, not explicit transmissions.
        _ => return Ok(None),
    };
    Ok(Some(AgentCollaborationTransmission {
        id: format!("mailbox:{message_id}"),
        kind,
        source_agent_id: Some(sender),
        target_agent_id: Some(recipient),
    }))
}

fn project_root_turn(
    connection: &Connection,
    event: &AgentCollaborationEventRecord,
) -> Result<Option<AgentCollaborationTransmission>, AgentCollaborationEventError> {
    let (Some(run_id), Some(turn_id)) = (event.run_id.as_deref(), event.turn_id.as_deref()) else {
        return Ok(None);
    };
    let trace: Option<(String, i64, i64)> = connection
        .query_row(
            "SELECT terminal_status, created_at, updated_at FROM conversation_turn_traces
         WHERE run_id = ?1 AND assistant_message_id = ?2 AND conversation_id = ?3
           AND NOT EXISTS (SELECT 1 FROM automation_runs WHERE agent_run_id = ?1)",
            params![run_id, turn_id, &event.conversation_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|_| AgentCollaborationEventError::StorageUnavailable)?;
    let Some((status, started_at, updated_at)) = trace else {
        return Ok(None);
    };

    if event.kind == AgentCollaborationEventKind::TurnUpdated {
        // Match the exact persisted update cut: old progress events must not become completion
        // pulses merely because the current Turn later finished. A root resumed by a child's
        // message can finish a user-facing reply without impersonating human input.
        return Ok((status == "completed"
            && event.created_at == updated_at
            && u64::try_from(updated_at).ok() == Some(event.resource_revision))
        .then(|| AgentCollaborationTransmission {
            id: format!("completion:{run_id}"),
            kind: AgentCollaborationTransmissionKind::Completion,
            source_agent_id: Some(event.root_agent_id.clone()),
            target_agent_id: None,
        }));
    }
    if event.created_at != started_at {
        return Ok(None);
    }

    // Bind to the input which preceded admission, not a later guidance/Agent mailbox projection
    // inserted before the active assistant. Origin is checked after selecting the nearest input
    // so an Agent-authored input cannot fall back to an older human message.
    let input: Option<(String, Option<String>)> = connection
        .query_row(
            "SELECT input.id, input.input_origin_kind
         FROM messages AS input JOIN messages AS assistant
           ON assistant.id = ?1 AND assistant.conversation_id = input.conversation_id
         WHERE input.conversation_id = ?2 AND input.role = 'user'
           AND input.position < assistant.position AND input.created_at <= ?3
           AND NOT EXISTS (
               SELECT 1 FROM agent_run_guidances AS guidance
               WHERE guidance.conversation_id = input.conversation_id
                 AND guidance.client_message_id = input.id
           )
         ORDER BY input.position DESC LIMIT 1",
            params![turn_id, &event.conversation_id, started_at],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|_| AgentCollaborationEventError::StorageUnavailable)?;
    let Some((user_message_id, origin)) = input else {
        return Ok(None);
    };
    if !matches!(origin.as_deref(), None | Some("human")) {
        return Ok(None);
    }
    Ok(Some(AgentCollaborationTransmission {
        id: format!("user-message:{user_message_id}"),
        kind: AgentCollaborationTransmissionKind::UserMessage,
        source_agent_id: None,
        target_agent_id: Some(event.root_agent_id.clone()),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Exercise projection independently of scheduling. Existing event-repository integration
    // tests also run this query against the complete migrated schema and real event triggers.
    fn connection() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(
            "CREATE TABLE agent_mailbox_messages (
                 message_id TEXT, root_agent_id TEXT, sender_agent_id TEXT,
                 recipient_agent_id TEXT, kind TEXT
             );
             CREATE TABLE conversation_turn_traces (
                 assistant_message_id TEXT, conversation_id TEXT, run_id TEXT,
                 terminal_status TEXT, created_at INTEGER, updated_at INTEGER
             );
             CREATE TABLE messages (
                 id TEXT, conversation_id TEXT, role TEXT, position INTEGER,
                 created_at INTEGER, input_origin_kind TEXT
             );
             CREATE TABLE agent_run_guidances (conversation_id TEXT, client_message_id TEXT);
             CREATE TABLE automation_runs (agent_run_id TEXT);
             INSERT INTO messages VALUES ('human-input', 'conversation-root', 'user', 0, 9, 'human');
             INSERT INTO messages VALUES ('assistant-root', 'conversation-root', 'assistant', 10, 10, NULL);
             INSERT INTO conversation_turn_traces VALUES (
                 'assistant-root', 'conversation-root', 'run-root', 'in_progress', 10, 10
             );"
        ).unwrap();
        connection
    }

    fn event(kind: AgentCollaborationEventKind, at: i64) -> AgentCollaborationEventRecord {
        AgentCollaborationEventRecord {
            global_sequence: 1,
            schema_version: 2,
            event_id: "event-id".into(),
            root_sequence: 1,
            workspace_id: None,
            project_id: None,
            root_agent_id: "agent-root".into(),
            root_conversation_id: "conversation-root".into(),
            agent_id: "agent-root".into(),
            conversation_id: "conversation-root".into(),
            turn_id: Some("assistant-root".into()),
            run_id: Some("run-root".into()),
            message_id: Some("mailbox-message".into()),
            kind,
            resource_revision: at as u64,
            activities: vec![],
            transmission: None,
            created_at: at,
        }
    }

    #[test]
    fn mailbox_transmissions_use_actual_sender_and_exclude_automatic_results() {
        let connection = connection();
        for (kind, sender, recipient, expected) in [
            (
                "message",
                "agent-grandchild",
                "agent-child",
                Some(AgentCollaborationTransmissionKind::Message),
            ),
            (
                "task",
                "agent-root",
                "agent-child",
                Some(AgentCollaborationTransmissionKind::Task),
            ),
            (
                "followup",
                "agent-root",
                "agent-grandchild",
                Some(AgentCollaborationTransmissionKind::Task),
            ),
            ("result", "agent-child", "agent-root", None),
        ] {
            connection
                .execute("DELETE FROM agent_mailbox_messages", [])
                .unwrap();
            connection.execute(
                "INSERT INTO agent_mailbox_messages VALUES ('mailbox-message', 'agent-root', ?1, ?2, ?3)",
                params![sender, recipient, kind],
            ).unwrap();
            let mut notification = event(AgentCollaborationEventKind::MailboxEnqueued, 10);
            notification.agent_id = recipient.into();
            let projected = project(&connection, &notification).unwrap();
            assert_eq!(projected.as_ref().map(|value| value.kind), expected);
            if let Some(projected) = projected {
                assert_eq!(projected.id, "mailbox:mailbox-message");
                assert_eq!(projected.source_agent_id.as_deref(), Some(sender));
                assert_eq!(projected.target_agent_id.as_deref(), Some(recipient));
            }
            notification.kind = AgentCollaborationEventKind::MailboxUpdated;
            assert!(project(&connection, &notification).unwrap().is_none());
            notification.kind = AgentCollaborationEventKind::MailboxEnqueued;
            notification.agent_id = "unrelated-agent".into();
            assert!(project(&connection, &notification).unwrap().is_none());
        }
    }

    #[test]
    fn human_admission_identity_survives_guidance_and_approval_resume_updates() {
        let connection = connection();
        let admission = event(AgentCollaborationEventKind::TurnStarted, 10);
        let expected = AgentCollaborationTransmission {
            id: "user-message:human-input".into(),
            kind: AgentCollaborationTransmissionKind::UserMessage,
            source_agent_id: None,
            target_agent_id: Some("agent-root".into()),
        };
        assert_eq!(
            project(&connection, &admission).unwrap(),
            Some(expected.clone())
        );
        connection.execute_batch(
            "INSERT INTO messages VALUES ('same-time-guidance', 'conversation-root', 'user', 1, 10, 'human');
             INSERT INTO agent_run_guidances VALUES ('conversation-root', 'same-time-guidance');
             INSERT INTO messages VALUES ('later-guidance', 'conversation-root', 'user', 2, 15, 'human');
             UPDATE conversation_turn_traces SET updated_at = 20;"
        ).unwrap();
        assert_eq!(project(&connection, &admission).unwrap(), Some(expected));
        assert!(project(
            &connection,
            &event(AgentCollaborationEventKind::TurnUpdated, 20)
        )
        .unwrap()
        .is_none());
        assert!(project(
            &connection,
            &event(AgentCollaborationEventKind::TurnStarted, 20)
        )
        .unwrap()
        .is_none());
    }

    #[test]
    fn completion_is_root_terminal_fact_not_progress_tool_success_or_failure() {
        let connection = connection();
        connection
            .execute("UPDATE conversation_turn_traces SET updated_at = 20", [])
            .unwrap();
        let progress = event(AgentCollaborationEventKind::TurnUpdated, 20);
        assert!(project(&connection, &progress).unwrap().is_none());
        connection.execute("UPDATE conversation_turn_traces SET terminal_status = 'completed', updated_at = 30", []).unwrap();
        // Reading old events after a later completion must not backfill completion everywhere.
        assert!(project(&connection, &progress).unwrap().is_none());
        let mut completed = event(AgentCollaborationEventKind::TurnUpdated, 30);
        assert_eq!(
            project(&connection, &completed).unwrap(),
            Some(AgentCollaborationTransmission {
                id: "completion:run-root".into(),
                kind: AgentCollaborationTransmissionKind::Completion,
                source_agent_id: Some("agent-root".into()),
                target_agent_id: None,
            })
        );
        completed.resource_revision = 29;
        assert!(project(&connection, &completed).unwrap().is_none());
        completed.resource_revision = 30;
        completed.agent_id = "agent-child".into();
        assert!(project(&connection, &completed).unwrap().is_none());
        completed.agent_id = "agent-root".into();
        for status in ["failed", "cancelled", "in_progress"] {
            connection
                .execute(
                    "UPDATE conversation_turn_traces SET terminal_status = ?1",
                    [status],
                )
                .unwrap();
            assert!(project(&connection, &completed).unwrap().is_none());
        }
    }

    #[test]
    fn automation_and_agent_authored_inputs_never_impersonate_human_transmissions() {
        let connection = connection();
        let admission = event(AgentCollaborationEventKind::TurnStarted, 10);
        connection
            .execute("INSERT INTO automation_runs VALUES ('run-root')", [])
            .unwrap();
        assert!(project(&connection, &admission).unwrap().is_none());
        connection.execute("UPDATE conversation_turn_traces SET terminal_status = 'completed', updated_at = 30", []).unwrap();
        assert!(project(
            &connection,
            &event(AgentCollaborationEventKind::TurnUpdated, 30)
        )
        .unwrap()
        .is_none());
        connection
            .execute("DELETE FROM automation_runs", [])
            .unwrap();
        for origin in ["agent", "snapshot"] {
            connection
                .execute("DELETE FROM messages WHERE id = 'nonhuman-input'", [])
                .unwrap();
            connection.execute(
                "INSERT INTO messages VALUES ('nonhuman-input', 'conversation-root', 'user', 1, 10, ?1)",
                [origin],
            ).unwrap();
            assert!(project(&connection, &admission).unwrap().is_none());
            connection.execute("UPDATE conversation_turn_traces SET terminal_status = 'completed', updated_at = 30", []).unwrap();
            assert_eq!(
                project(
                    &connection,
                    &event(AgentCollaborationEventKind::TurnUpdated, 30)
                )
                .unwrap()
                .unwrap()
                .kind,
                AgentCollaborationTransmissionKind::Completion
            );
        }
    }
}
