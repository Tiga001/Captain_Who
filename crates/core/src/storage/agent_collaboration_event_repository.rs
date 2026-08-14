use crate::{
    AgentCollaborationActivitySemantic, AgentCollaborationActivitySnapshot,
    AgentCollaborationEventError, AgentCollaborationEventKind, AgentCollaborationEventRecord,
    AGENT_COLLABORATION_ACTIVITY_SCHEMA_VERSION, AGENT_COLLABORATION_EVENT_SCHEMA_VERSION,
    MAX_AGENT_COLLABORATION_EVENTS_PAGE,
};
use rusqlite::{params, Connection};

const EVENT_SELECT: &str = "
    SELECT global_sequence, schema_version, event_id, root_sequence, workspace_id, project_id,
           root_agent_id, root_conversation_id, agent_id, conversation_id,
           turn_id, run_id, message_id, kind, resource_revision,
           activity_schema_version, activity_semantic, activity_agent_id,
           activity_task_name_snapshot, activity_root_anchor_message_id, created_at
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
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, String>(8)?,
                row.get::<_, String>(9)?,
                row.get::<_, Option<String>>(10)?,
                row.get::<_, Option<String>>(11)?,
                row.get::<_, Option<String>>(12)?,
                row.get::<_, String>(13)?,
                row.get::<_, i64>(14)?,
                row.get::<_, Option<i64>>(15)?,
                row.get::<_, Option<String>>(16)?,
                row.get::<_, Option<String>>(17)?,
                row.get::<_, Option<String>>(18)?,
                row.get::<_, Option<String>>(19)?,
                row.get::<_, i64>(20)?,
            ))
        })
        .map_err(|_| AgentCollaborationEventError::StorageUnavailable)?;
    let mut events = Vec::new();
    for row in rows {
        let (
            global_sequence,
            schema_version,
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
            activity_schema_version,
            activity_semantic,
            activity_agent_id,
            activity_task_name_snapshot,
            activity_root_anchor_message_id,
            created_at,
        ) = row.map_err(|_| AgentCollaborationEventError::StorageUnavailable)?;
        let schema_version = u32::try_from(schema_version)
            .map_err(|_| AgentCollaborationEventError::CorruptRecord)?;
        if schema_version != AGENT_COLLABORATION_EVENT_SCHEMA_VERSION {
            return Err(AgentCollaborationEventError::CorruptRecord);
        }
        events.push(AgentCollaborationEventRecord {
            global_sequence: positive(global_sequence)?,
            schema_version,
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
            activity: parse_activity(
                activity_schema_version,
                activity_semantic,
                activity_agent_id,
                activity_task_name_snapshot,
                activity_root_anchor_message_id,
            )?,
            created_at: nonnegative(created_at)?,
        });
    }
    Ok(events)
}

fn parse_activity(
    schema_version: Option<i64>,
    semantic: Option<String>,
    agent_id: Option<String>,
    task_name_snapshot: Option<String>,
    root_anchor_message_id: Option<String>,
) -> Result<Option<AgentCollaborationActivitySnapshot>, AgentCollaborationEventError> {
    let (schema_version, semantic, agent_id, task_name_snapshot, root_anchor_message_id) = match (
        schema_version,
        semantic,
        agent_id,
        task_name_snapshot,
        root_anchor_message_id,
    ) {
        (None, None, None, None, None) => return Ok(None),
        (
            Some(schema_version),
            Some(semantic),
            Some(agent_id),
            Some(task_name_snapshot),
            root_anchor_message_id,
        ) => (
            schema_version,
            semantic,
            agent_id,
            task_name_snapshot,
            root_anchor_message_id,
        ),
        _ => return Err(AgentCollaborationEventError::CorruptRecord),
    };
    let schema_version =
        u32::try_from(schema_version).map_err(|_| AgentCollaborationEventError::CorruptRecord)?;
    if schema_version != AGENT_COLLABORATION_ACTIVITY_SCHEMA_VERSION {
        return Err(AgentCollaborationEventError::CorruptRecord);
    }
    validate_persisted_text(&agent_id, 256)?;
    validate_persisted_text(&task_name_snapshot, 256)?;
    if let Some(anchor) = root_anchor_message_id.as_deref() {
        validate_persisted_text(anchor, 2_048)?;
    }
    Ok(Some(AgentCollaborationActivitySnapshot {
        schema_version,
        semantic: AgentCollaborationActivitySemantic::parse(&semantic)?,
        agent_id,
        task_name_snapshot,
        root_anchor_message_id,
    }))
}

fn validate_persisted_text(
    value: &str,
    maximum: usize,
) -> Result<(), AgentCollaborationEventError> {
    if value.trim() != value || value.is_empty() || value.len() > maximum {
        return Err(AgentCollaborationEventError::CorruptRecord);
    }
    Ok(())
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
        SendAgentMessageRequest,
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

    fn insert_activity_event(
        connection: &mut Connection,
        event_id: &str,
        outer_agent_id: &str,
        outer_conversation_id: &str,
        activity_agent_id: &str,
        task_name_snapshot: &str,
        root_anchor_message_id: Option<&str>,
    ) -> rusqlite::Result<()> {
        let transaction = connection.transaction()?;
        transaction.execute(
            "UPDATE agent_collaboration_event_sequences
             SET next_sequence = next_sequence + 1 WHERE root_agent_id = 'agent-root'",
            [],
        )?;
        transaction.execute(
            "INSERT INTO agent_collaboration_events (
                 event_id, schema_version, root_agent_id, root_sequence,
                 workspace_id, project_id, root_conversation_id,
                 agent_id, conversation_id, kind, resource_revision,
                 activity_schema_version, activity_semantic, activity_agent_id,
                 activity_task_name_snapshot, activity_root_anchor_message_id, created_at
             ) VALUES (
                 ?1, 2, 'agent-root',
                 (SELECT next_sequence - 1 FROM agent_collaboration_event_sequences
                  WHERE root_agent_id = 'agent-root'),
                 'project-a', 'project-a', 'conversation-root', ?2, ?3,
                 'wake_created', 1, 1, 'started', ?4, ?5, ?6, 20
             )",
            params![
                event_id,
                outer_agent_id,
                outer_conversation_id,
                activity_agent_id,
                task_name_snapshot,
                root_anchor_message_id,
            ],
        )?;
        transaction.commit()
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
                'forged', 2, 'agent-a', 3, NULL, NULL, 'conversation-a',
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
                'gap', 2, 'agent-a', 999, NULL, NULL, 'conversation-a',
                'agent-a', 'conversation-a', 'agent_updated', 1, 3
             )",
            [],
        );
        assert!(wrong_sequence.is_err());
    }

    #[test]
    fn activity_identity_rejects_root_or_spoofed_subjects_and_untrusted_anchors() {
        let mut connection = tree();
        project_conversation(&connection, "conversation-other", "project-a");
        root(&mut connection, "agent-other", "conversation-other");
        connection
            .execute_batch(
                "INSERT INTO messages (
                     id, conversation_id, role, content, status, created_at, position
                 ) VALUES
                    ('root-assistant', 'conversation-root', 'assistant', 'root', 'sent', 10, 0),
                    ('root-user', 'conversation-root', 'user', 'user', 'sent', 11, 1),
                    ('child-assistant', 'conversation-child', 'assistant', 'child', 'sent', 12, 0),
                    ('other-assistant', 'conversation-other', 'assistant', 'other', 'sent', 13, 0);",
            )
            .unwrap();

        for (event_id, anchor) in [
            ("activity-user-anchor", Some("root-user")),
            ("activity-child-anchor", Some("child-assistant")),
            ("activity-other-root-anchor", Some("other-assistant")),
        ] {
            assert!(insert_activity_event(
                &mut connection,
                event_id,
                "agent-child",
                "conversation-child",
                "agent-child",
                "review",
                anchor,
            )
            .is_err());
        }
        assert!(insert_activity_event(
            &mut connection,
            "activity-root-subject",
            "agent-root",
            "conversation-root",
            "agent-root",
            "root",
            None,
        )
        .is_err());
        assert!(insert_activity_event(
            &mut connection,
            "activity-spoofed-task",
            "agent-child",
            "conversation-child",
            "agent-child",
            "not-review",
            None,
        )
        .is_err());

        insert_activity_event(
            &mut connection,
            "activity-valid-anchor",
            "agent-child",
            "conversation-child",
            "agent-child",
            "review",
            Some("root-assistant"),
        )
        .unwrap();
        let event = list_root_events(&connection, "agent-root", 0, 32)
            .unwrap()
            .into_iter()
            .find(|event| event.event_id == "activity-valid-anchor")
            .unwrap();
        assert_eq!(
            event
                .activity
                .as_ref()
                .and_then(|activity| activity.root_anchor_message_id.as_deref()),
            Some("root-assistant")
        );
    }

    #[test]
    fn activity_projection_failure_rolls_back_the_source_wake_and_root_sequence() {
        let mut connection = tree();
        let message = agent_graph_repository::enqueue_agent_message(
            &mut connection,
            &EnqueueAgentMessageInput {
                message_id: "mailbox-activity-fault".to_string(),
                root_agent_id: "agent-root".to_string(),
                sender_agent_id: "agent-root".to_string(),
                recipient_agent_id: "agent-child".to_string(),
                request_id: "message-activity-fault".to_string(),
                kind: AgentMailboxKind::Task,
                content: "review".to_string(),
                projection_message_id: "projection-activity-fault".to_string(),
            },
            20,
        )
        .unwrap()
        .record()
        .clone();
        let before_sequence = latest_root_sequence(&connection, "agent-root").unwrap();
        connection
            .execute_batch(
                "CREATE TEMP TRIGGER fail_collaboration_activity_projection
                 BEFORE INSERT ON agent_collaboration_events
                 WHEN NEW.activity_semantic IS NOT NULL
                 BEGIN SELECT RAISE(ABORT, 'activity projection fault'); END;",
            )
            .unwrap();

        assert!(agent_graph_repository::enqueue_agent_wake(
            &mut connection,
            &EnqueueAgentWakeInput {
                wake_id: "wake-activity-fault".to_string(),
                root_agent_id: "agent-root".to_string(),
                agent_id: "agent-child".to_string(),
                requester_agent_id: "agent-root".to_string(),
                request_id: "wake-request-activity-fault".to_string(),
                source_agent_message_id: Some(message.message_id),
            },
            21,
        )
        .is_err());
        assert_eq!(
            latest_root_sequence(&connection, "agent-root").unwrap(),
            before_sequence
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM agent_wake_requests
                     WHERE wake_id = 'wake-activity-fault'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0
        );
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
        agent_graph_repository::send_agent_message(
            &mut connection,
            &SendAgentMessageRequest {
                sender_agent_id: "agent-child".to_string(),
                recipient_agent_id: "agent-root".to_string(),
                request_id: "child-update-1".to_string(),
                content: "still working".to_string(),
            },
            11,
        )
        .unwrap();
        agent_graph_repository::follow_up_agent(
            &mut connection,
            &SendAgentMessageRequest {
                sender_agent_id: "agent-root".to_string(),
                recipient_agent_id: "agent-child".to_string(),
                request_id: "followup-1".to_string(),
                content: "continue".to_string(),
            },
            12,
        )
        .unwrap();
        connection
            .execute(
                "UPDATE conversations SET model_id = 'model-b', updated_at = 13
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
                AgentCollaborationEventKind::MailboxEnqueued,
                AgentCollaborationEventKind::MailboxEnqueued,
                AgentCollaborationEventKind::WakeCreated,
                AgentCollaborationEventKind::AgentUpdated,
            ]
        );
        assert!(events
            .iter()
            .all(|event| { event.schema_version == AGENT_COLLABORATION_EVENT_SCHEMA_VERSION }));
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
        assert_eq!(events[9].agent_id, "agent-child");
        assert_eq!(events[10].agent_id, "agent-child");
        assert_eq!(events[11].agent_id, "agent-root");
        assert_eq!(
            events[1].activity,
            Some(AgentCollaborationActivitySnapshot {
                schema_version: AGENT_COLLABORATION_ACTIVITY_SCHEMA_VERSION,
                semantic: AgentCollaborationActivitySemantic::Started,
                agent_id: "agent-child".to_string(),
                task_name_snapshot: "review".to_string(),
                root_anchor_message_id: None,
            })
        );
        assert_eq!(
            events[3].activity,
            Some(AgentCollaborationActivitySnapshot {
                schema_version: AGENT_COLLABORATION_ACTIVITY_SCHEMA_VERSION,
                semantic: AgentCollaborationActivitySemantic::Interrupted,
                agent_id: "agent-child".to_string(),
                task_name_snapshot: "review".to_string(),
                root_anchor_message_id: None,
            })
        );
        assert_eq!(
            events[6]
                .activity
                .as_ref()
                .map(|activity| activity.semantic),
            Some(AgentCollaborationActivitySemantic::WaitingApproval)
        );
        assert_eq!(
            events[8].activity,
            Some(AgentCollaborationActivitySnapshot {
                schema_version: AGENT_COLLABORATION_ACTIVITY_SCHEMA_VERSION,
                semantic: AgentCollaborationActivitySemantic::Updated,
                agent_id: "agent-child".to_string(),
                task_name_snapshot: "review".to_string(),
                root_anchor_message_id: None,
            })
        );
        assert_eq!(
            events[10].activity,
            Some(AgentCollaborationActivitySnapshot {
                schema_version: AGENT_COLLABORATION_ACTIVITY_SCHEMA_VERSION,
                semantic: AgentCollaborationActivitySemantic::Started,
                agent_id: "agent-child".to_string(),
                task_name_snapshot: "review".to_string(),
                root_anchor_message_id: None,
            })
        );
        assert!(events
            .iter()
            .enumerate()
            .filter(|(index, _)| !matches!(index, 1 | 3 | 6 | 8 | 10))
            .all(|(_, event)| event.activity.is_none()));

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

    #[test]
    fn activity_history_pages_without_duplicates_across_the_maximum_boundary() {
        let mut connection = tree();
        let initial = latest_root_sequence(&connection, "agent-root").unwrap();
        for sequence in 1..=520 {
            insert_activity_event(
                &mut connection,
                &format!("activity-page-{sequence}"),
                "agent-child",
                "conversation-child",
                "agent-child",
                "review",
                None,
            )
            .unwrap();
        }

        let first = list_root_events(
            &connection,
            "agent-root",
            initial,
            MAX_AGENT_COLLABORATION_EVENTS_PAGE,
        )
        .unwrap();
        let second = list_root_events(
            &connection,
            "agent-root",
            first.last().unwrap().root_sequence,
            MAX_AGENT_COLLABORATION_EVENTS_PAGE,
        )
        .unwrap();
        assert_eq!(first.len(), 512);
        assert_eq!(second.len(), 8);
        assert_eq!(first.first().unwrap().root_sequence, initial + 1);
        assert_eq!(second.last().unwrap().root_sequence, initial + 520);
        let all = first.iter().chain(&second).collect::<Vec<_>>();
        assert!(all.windows(2).all(|pair| {
            pair[1].root_sequence == pair[0].root_sequence + 1
                && pair[1].event_id != pair[0].event_id
        }));
        assert!(all.iter().all(|event| {
            event.activity.as_ref().is_some_and(|activity| {
                activity.semantic == AgentCollaborationActivitySemantic::Started
            })
        }));

        assert!(insert_activity_event(
            &mut connection,
            "activity-page-520",
            "agent-child",
            "conversation-child",
            "agent-child",
            "review",
            None,
        )
        .is_err());
        assert_eq!(
            latest_root_sequence(&connection, "agent-root").unwrap(),
            initial + 520
        );
    }
}
