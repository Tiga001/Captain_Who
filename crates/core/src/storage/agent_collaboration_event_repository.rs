use crate::{
    AgentCollaborationEventError, AgentCollaborationEventKind, AgentCollaborationEventRecord,
    MAX_AGENT_COLLABORATION_EVENTS_PAGE,
};
use rusqlite::{params, Connection};

const EVENT_SELECT: &str = "
    SELECT global_sequence, event_id, root_sequence, workspace_id, project_id,
           root_agent_id, root_conversation_id, agent_id, conversation_id,
           turn_id, run_id, message_id, kind, resource_revision, created_at
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
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, String>(8)?,
                row.get::<_, Option<String>>(9)?,
                row.get::<_, Option<String>>(10)?,
                row.get::<_, Option<String>>(11)?,
                row.get::<_, String>(12)?,
                row.get::<_, i64>(13)?,
                row.get::<_, i64>(14)?,
            ))
        })
        .map_err(|_| AgentCollaborationEventError::StorageUnavailable)?;
    let mut events = Vec::new();
    for row in rows {
        let (
            global_sequence,
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
        events.push(AgentCollaborationEventRecord {
            global_sequence: positive(global_sequence)?,
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
            created_at: nonnegative(created_at)?,
        });
    }
    Ok(events)
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
mod tests {
    use super::*;
    use crate::storage::{agent_graph_repository, migrations};
    use crate::{
        AgentMailboxKind, AgentModelSelectionSnapshot, CreateAgentNodeInput,
        EnqueueAgentMessageInput, EnqueueAgentWakeInput, EnsureRootAgentInput,
    };

    fn conversation(connection: &Connection, id: &str) {
        connection
            .execute(
                "INSERT INTO conversations (
                    id, project_id, model_id, title, created_at, updated_at,
                    pinned_at, archived_at, unread_at, revision
                 ) VALUES (?1, NULL, NULL, ?1, 1, 1, NULL, NULL, NULL, 0)",
                [id],
            )
            .unwrap();
    }

    fn root(connection: &mut Connection, id: &str, conversation_id: &str) {
        agent_graph_repository::ensure_root_agent(
            connection,
            &EnsureRootAgentInput {
                agent_id: id.to_string(),
                conversation_id: conversation_id.to_string(),
                creation_request_id: format!("create:{id}"),
                task_name: "root".to_string(),
            },
            1,
        )
        .unwrap();
    }

    fn project(connection: &Connection, id: &str) {
        connection
            .execute(
                "INSERT INTO projects (id, name, path, created_at, pinned_at, updated_at)
                 VALUES (?1, ?1, NULL, 1, NULL, 1)",
                [id],
            )
            .unwrap();
    }

    fn project_conversation(connection: &Connection, id: &str, project_id: &str) {
        connection
            .execute(
                "INSERT INTO conversations (
                    id, project_id, model_id, title, created_at, updated_at,
                    pinned_at, archived_at, unread_at, revision
                 ) VALUES (?1, ?2, 'model-a', ?1, 1, 1, NULL, NULL, NULL, 0)",
                params![id, project_id],
            )
            .unwrap();
    }

    fn tree() -> Connection {
        let mut connection = Connection::open_in_memory().unwrap();
        migrations::run_migrations(&connection).unwrap();
        project(&connection, "project-a");
        project_conversation(&connection, "conversation-root", "project-a");
        project_conversation(&connection, "conversation-child", "project-a");
        root(&mut connection, "agent-root", "conversation-root");
        agent_graph_repository::create_agent_node(
            &mut connection,
            &CreateAgentNodeInput {
                agent_id: "agent-child".to_string(),
                root_agent_id: "agent-root".to_string(),
                parent_agent_id: "agent-root".to_string(),
                conversation_id: "conversation-child".to_string(),
                creation_request_id: "spawn-child".to_string(),
                task_name: "review".to_string(),
                task_path: "/root/review".to_string(),
                template_snapshot: None,
                model_snapshot: AgentModelSelectionSnapshot {
                    model_config_id: "model-a".to_string(),
                    display_name: "Model A".to_string(),
                    supports_image: false,
                    effective_context_window_tokens: 64_000,
                    model_settings_configuration_revision: "model-settings-v1:test".to_string(),
                    provider_connection_revision: "provider-connection-v1:test".to_string(),
                    provider_protocol_revision: "provider-protocol-v1:test".to_string(),
                },
            },
            2,
        )
        .unwrap();
        connection
    }

    #[test]
    fn root_sequences_are_monotonic_isolated_and_cross_tree_events_fail_closed() {
        let mut connection = Connection::open_in_memory().unwrap();
        migrations::run_migrations(&connection).unwrap();
        conversation(&connection, "conversation-a");
        conversation(&connection, "conversation-b");
        root(&mut connection, "agent-a", "conversation-a");
        root(&mut connection, "agent-b", "conversation-b");
        connection
            .execute(
                "UPDATE agent_nodes
                 SET lifecycle = 'archived', revision = 2, updated_at = 2
                 WHERE agent_id = 'agent-a'",
                [],
            )
            .unwrap();

        let events = list_root_events(&connection, "agent-a", 0, 16).unwrap();
        assert_eq!(
            events
                .iter()
                .map(|event| event.root_sequence)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert!(events.iter().all(|event| {
            event.root_agent_id == "agent-a"
                && event.root_conversation_id == "conversation-a"
                && event.agent_id == "agent-a"
                && event.conversation_id == "conversation-a"
        }));
        let global = list_global_events(&connection, 0, 16).unwrap();
        assert_eq!(global.len(), 3);
        assert!(global
            .windows(2)
            .all(|pair| pair[0].global_sequence < pair[1].global_sequence));

        let transaction = connection.transaction().unwrap();
        transaction
            .execute(
                "UPDATE agent_collaboration_event_sequences
                 SET next_sequence = next_sequence + 1 WHERE root_agent_id = 'agent-a'",
                [],
            )
            .unwrap();
        let cross_tree = transaction.execute(
            "INSERT INTO agent_collaboration_events (
                event_id, schema_version, root_agent_id, root_sequence,
                workspace_id, project_id, root_conversation_id,
                agent_id, conversation_id, kind, resource_revision, created_at
             ) VALUES (
                'forged', 1, 'agent-a', 3, NULL, NULL, 'conversation-a',
                'agent-b', 'conversation-b', 'agent_updated', 1, 3
             )",
            [],
        );
        assert!(cross_tree.is_err());
        transaction.rollback().unwrap();
        assert_eq!(latest_root_sequence(&connection, "agent-a").unwrap(), 2);

        let wrong_sequence = connection.execute(
            "INSERT INTO agent_collaboration_events (
                event_id, schema_version, root_agent_id, root_sequence,
                workspace_id, project_id, root_conversation_id,
                agent_id, conversation_id, kind, resource_revision, created_at
             ) VALUES (
                'gap', 1, 'agent-a', 999, NULL, NULL, 'conversation-a',
                'agent-a', 'conversation-a', 'agent_updated', 1, 3
             )",
            [],
        );
        assert!(wrong_sequence.is_err());
    }

    #[test]
    fn every_collaboration_domain_trigger_emits_exact_identity_and_rolls_back_atomically() {
        let mut connection = tree();
        let initial = latest_root_sequence(&connection, "agent-root").unwrap();

        let mailbox = agent_graph_repository::enqueue_agent_message(
            &mut connection,
            &EnqueueAgentMessageInput {
                message_id: "mailbox-1".to_string(),
                root_agent_id: "agent-root".to_string(),
                sender_agent_id: "agent-root".to_string(),
                recipient_agent_id: "agent-child".to_string(),
                request_id: "mailbox-request-1".to_string(),
                kind: AgentMailboxKind::Task,
                content: "review".to_string(),
                projection_message_id: "projection-1".to_string(),
            },
            3,
        )
        .unwrap()
        .record()
        .clone();
        agent_graph_repository::enqueue_agent_wake(
            &mut connection,
            &EnqueueAgentWakeInput {
                wake_id: "wake-1".to_string(),
                root_agent_id: "agent-root".to_string(),
                agent_id: "agent-child".to_string(),
                requester_agent_id: "agent-root".to_string(),
                request_id: "wake-request-1".to_string(),
                source_agent_message_id: Some(mailbox.message_id.clone()),
            },
            4,
        )
        .unwrap();
        connection
            .execute(
                "UPDATE agent_mailbox_messages
                 SET delivery_status = 'claimed', claim_token = 'claim-mailbox',
                     lease_expires_at = 100, claimed_at = 5
                 WHERE message_id = 'mailbox-1'",
                [],
            )
            .unwrap();
        connection
            .execute(
                "UPDATE agent_wake_requests
                 SET status = 'cancelled', status_revision = 2, completed_at = 6
                 WHERE wake_id = 'wake-1'",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO messages (
                    id, conversation_id, role, content, status, agent_run_json, ui_state_json,
                    created_at, position
                 ) VALUES ('assistant-1', 'conversation-child', 'assistant', '', 'pending',
                           NULL, NULL, 7, 0)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO conversation_turn_traces (
                    assistant_message_id, conversation_id, run_id, schema_version,
                    terminal_status, terminal_error, truncated, created_at, updated_at, completed_at
                 ) VALUES ('assistant-1', 'conversation-child', 'run-1', 1,
                           'in_progress', NULL, 0, 7, 7, NULL)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "UPDATE conversation_turn_traces
                 SET terminal_status = 'completed', updated_at = 8, completed_at = 8
                 WHERE run_id = 'run-1'",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO agent_pending_actions (
                    action_id, run_id, conversation_id, assistant_message_id, action_type,
                    tool_name, tool_call_id, status, target_status, action_json,
                    agent_input_json, created_at, updated_at
                 ) VALUES ('approval-1', 'run-1', 'conversation-child', 'assistant-1',
                           'tool_call', 'read_file', 'call-1', 'pending', NULL,
                           '{}', '{}', 9, 9)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "UPDATE agent_pending_actions
                 SET status = 'approved', target_status = 'completed', updated_at = 10
                 WHERE action_id = 'approval-1'",
                [],
            )
            .unwrap();
        connection
            .execute(
                "UPDATE conversations SET model_id = 'model-b', updated_at = 11
                 WHERE id = 'conversation-root'",
                [],
            )
            .unwrap();

        let events = list_root_events(&connection, "agent-root", initial, 32).unwrap();
        assert_eq!(
            events.iter().map(|event| event.kind).collect::<Vec<_>>(),
            vec![
                AgentCollaborationEventKind::MailboxEnqueued,
                AgentCollaborationEventKind::WakeCreated,
                AgentCollaborationEventKind::MailboxUpdated,
                AgentCollaborationEventKind::WakeUpdated,
                AgentCollaborationEventKind::TurnStarted,
                AgentCollaborationEventKind::TurnUpdated,
                AgentCollaborationEventKind::ApprovalProjected,
                AgentCollaborationEventKind::ApprovalUpdated,
                AgentCollaborationEventKind::AgentUpdated,
            ]
        );
        assert!(events
            .windows(2)
            .all(|pair| pair[1].root_sequence == pair[0].root_sequence + 1));
        assert!(events.iter().all(|event| {
            event.root_agent_id == "agent-root"
                && event.root_conversation_id == "conversation-root"
                && event.project_id.as_deref() == Some("project-a")
                && event.workspace_id == event.project_id
        }));
        assert_eq!(events[0].agent_id, "agent-child");
        assert_eq!(events[0].message_id.as_deref(), Some("mailbox-1"));
        assert_eq!(events[1].agent_id, "agent-child");
        assert_eq!(events[1].message_id.as_deref(), Some("mailbox-1"));
        assert_eq!(events[4].turn_id.as_deref(), Some("assistant-1"));
        assert_eq!(events[4].run_id.as_deref(), Some("run-1"));
        assert_eq!(events[6].agent_id, "agent-child");
        assert_eq!(events[8].agent_id, "agent-root");

        let before_rollback = latest_root_sequence(&connection, "agent-root").unwrap();
        let transaction = connection.transaction().unwrap();
        transaction
            .execute(
                "INSERT INTO agent_mailbox_messages (
                    message_id, schema_version, root_agent_id, sender_agent_id,
                    recipient_agent_id, request_id, kind, content, projection_message_id,
                    delivery_status, created_at
                 ) VALUES ('mailbox-rollback', 1, 'agent-root', 'agent-root', 'agent-child',
                           'mailbox-request-rollback', 'task', 'rollback',
                           'projection-rollback', 'queued', 12)",
                [],
            )
            .unwrap();
        assert_eq!(
            latest_root_sequence(&transaction, "agent-root").unwrap(),
            before_rollback + 1
        );
        transaction.rollback().unwrap();
        assert_eq!(
            latest_root_sequence(&connection, "agent-root").unwrap(),
            before_rollback
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM agent_mailbox_messages
                     WHERE message_id = 'mailbox-rollback'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0
        );
    }

    #[test]
    fn maximum_page_preserves_a_contiguous_cursor_and_exposes_the_remainder() {
        let mut connection = Connection::open_in_memory().unwrap();
        migrations::run_migrations(&connection).unwrap();
        conversation(&connection, "conversation-a");
        root(&mut connection, "agent-a", "conversation-a");
        for revision in 2..=520_i64 {
            connection
                .execute(
                    "UPDATE agent_nodes SET revision = ?1, updated_at = ?1
                     WHERE agent_id = 'agent-a'",
                    [revision],
                )
                .unwrap();
        }
        let first = list_root_events(
            &connection,
            "agent-a",
            0,
            MAX_AGENT_COLLABORATION_EVENTS_PAGE,
        )
        .unwrap();
        assert_eq!(first.len(), MAX_AGENT_COLLABORATION_EVENTS_PAGE);
        assert_eq!(first.first().unwrap().root_sequence, 1);
        assert_eq!(first.last().unwrap().root_sequence, 512);
        assert!(latest_root_sequence(&connection, "agent-a").unwrap() > 512);
        let second = list_root_events(&connection, "agent-a", 512, 512).unwrap();
        assert_eq!(second.first().unwrap().root_sequence, 513);
        assert_eq!(second.last().unwrap().root_sequence, 520);
    }
}
