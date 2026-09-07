use super::*;
use crate::storage::{agent_graph_repository, chat_repository, migrations};
use crate::{
    AgentLifecycle, AgentModelSelectionSnapshot, AgentWakeStatus, CreateAgentNodeInput,
    EnqueueAgentMessageInput, EnqueueAgentWakeInput, EnsureRootAgentInput,
    FinishAgentTurnResultInput, SendAgentMessageRequest,
};
use serde_json::json;

const TEST_MAX_MESSAGE_BYTES: usize = 1_048_576;
const TEST_MAX_UNBOUND_MAILBOX_MESSAGES: u64 = 1_024;
const TEST_MAX_UNBOUND_ORDINARY_MAILBOX_MESSAGES: u64 = 960;

fn result_payload(agent_id: &str, task_name: &str, task_path: &str, summary: &str) -> String {
    serde_json::to_string(&crate::AgentTurnResultEnvelope {
        schema_version: crate::AGENT_RESULT_ENVELOPE_SCHEMA_VERSION,
        child_agent_id: agent_id.into(),
        task_name: task_name.into(),
        task_path: task_path.into(),
        wake_id: "private-wake".into(),
        turn_id: Some("private-turn".into()),
        run_id: Some("private-run".into()),
        status: AgentWakeStatus::Completed,
        summary: summary.into(),
        artifact_refs: Vec::new(),
        terminal_error: None,
    })
    .unwrap()
}

fn connection() -> Connection {
    let connection = Connection::open_in_memory().unwrap();
    migrations::run_migrations(&connection).unwrap();
    connection
}

fn model_snapshot() -> AgentModelSelectionSnapshot {
    AgentModelSelectionSnapshot {
        model_config_id: "model-a".into(),
        display_name: "Model A".into(),
        supports_image: false,
        effective_context_window_tokens: 64_000,
        model_settings_configuration_revision: "model-settings-v1:test".into(),
        provider_connection_revision: "provider-connection-v1:test".into(),
        provider_protocol_revision: "provider-protocol-v1:test".into(),
    }
}

fn setup_tree() -> Connection {
    let mut connection = connection();
    connection
        .execute(
            "INSERT INTO projects (id, name, path, created_at, pinned_at, updated_at)
                 VALUES ('project-a', 'Project A', NULL, 1, NULL, 1)",
            [],
        )
        .unwrap();
    for id in [
        "conversation-root",
        "conversation-child",
        "conversation-grand",
    ] {
        connection
            .execute(
                "INSERT INTO conversations (
                         id, project_id, model_id, title, created_at, updated_at,
                         pinned_at, archived_at, unread_at
                     ) VALUES (?1, 'project-a', 'model-a', ?1, 1, 1, NULL, NULL, NULL)",
                [id],
            )
            .unwrap();
    }
    agent_graph_repository::ensure_root_agent(
        &mut connection,
        &EnsureRootAgentInput {
            agent_id: "agent-root".into(),
            conversation_id: "conversation-root".into(),
            creation_request_id: "ensure-root".into(),
            task_name: "Root".into(),
        },
        1,
    )
    .unwrap();
    for (agent_id, parent, conversation, task_name, task_path, request) in [
        (
            "agent-child",
            "agent-root",
            "conversation-child",
            "child",
            "/root/child",
            "spawn-child",
        ),
        (
            "agent-grand",
            "agent-child",
            "conversation-grand",
            "grand",
            "/root/child/grand",
            "spawn-grand",
        ),
    ] {
        agent_graph_repository::create_agent_node(
            &mut connection,
            &CreateAgentNodeInput {
                agent_id: agent_id.into(),
                root_agent_id: "agent-root".into(),
                parent_agent_id: parent.into(),
                conversation_id: conversation.into(),
                creation_request_id: request.into(),
                task_name: task_name.into(),
                task_path: task_path.into(),
                template_snapshot: None,
                model_snapshot: model_snapshot(),
            },
            2,
        )
        .unwrap();
    }
    connection
}

fn begin_empty_turn(
    connection: &Connection,
    conversation_id: &str,
    run_id: &str,
    assistant_message_id: &str,
    created_at: i64,
) {
    connection
            .execute(
                "INSERT INTO messages (
                     id, conversation_id, role, content, status, created_at, position
                 ) VALUES (
                     ?1, ?2, 'assistant', '', 'pending', ?3,
                     (SELECT COALESCE(MAX(position), -1) + 1 FROM messages WHERE conversation_id = ?2)
                 )",
                params![assistant_message_id, conversation_id, created_at],
            )
            .unwrap();
    let trace = ConversationTraceSnapshot::default().in_progress_trace(
        run_id,
        conversation_id,
        assistant_message_id,
    );
    conversation_trace_repository::commit_trace_in_connection(
        connection, &trace, created_at, created_at,
    )
    .unwrap();
}

fn append_wait_tool_call(
    connection: &Connection,
    conversation_id: &str,
    run_id: &str,
    assistant_message_id: &str,
    call_id: &str,
    at: i64,
) {
    let trace =
        conversation_trace_repository::get_trace_for_message(connection, assistant_message_id)
            .unwrap()
            .unwrap();
    let model_items = conversation_model_context_repository::get_log_for_message(
        connection,
        assistant_message_id,
    )
    .unwrap()
    .map(|log| log.items)
    .unwrap_or_default();
    let next_sequence = trace
        .items
        .last()
        .map(crate::ConversationTurnTraceItem::sequence)
        .map_or(0, |sequence| sequence.saturating_add(1));
    let mut recorder = crate::conversation_trace::ConversationTraceRecorder::from_durable_snapshot(
        ConversationTraceSnapshot {
            items: trace.items,
            model_context_items: model_items,
            next_sequence,
            truncated: trace.truncated,
        },
    );
    let call = crate::AgentToolCall {
        id: call_id.to_string(),
        tool: "wait_agent".to_string(),
        args: json!({"targets": ["agent-child"]}),
        approval_status: crate::AgentApprovalStatus::NotRequired,
        reason: None,
    };
    let sequence = recorder.record_tool_call(&call).unwrap();
    recorder
        .record_model_message(
            sequence,
            0,
            &crate::llm::LlmMessage::assistant(
                "",
                vec![crate::llm::LlmToolCall {
                    id: call.id.clone(),
                    name: call.tool.clone(),
                    args: call.args.clone(),
                }],
            ),
        )
        .unwrap();
    let snapshot = recorder.snapshot();
    conversation_trace_repository::commit_trace_in_connection(
        connection,
        &snapshot.in_progress_audit_trace(run_id, conversation_id, assistant_message_id),
        at,
        at,
    )
    .unwrap();
    conversation_model_context_repository::commit_items_in_connection(
        connection,
        conversation_id,
        assistant_message_id,
        &snapshot.model_context_items,
    )
    .unwrap();
}

fn begin_wait_turn(
    connection: &Connection,
    conversation_id: &str,
    run_id: &str,
    assistant_message_id: &str,
    call_id: &str,
    created_at: i64,
) {
    begin_empty_turn(
        connection,
        conversation_id,
        run_id,
        assistant_message_id,
        created_at,
    );
    append_wait_tool_call(
        connection,
        conversation_id,
        run_id,
        assistant_message_id,
        call_id,
        created_at,
    );
}

fn safe_input(
    conversation_id: &str,
    run_id: &str,
    assistant_message_id: &str,
    batch: u64,
    expected_sequence: u64,
) -> BindAgentSafeBoundaryInput {
    BindAgentSafeBoundaryInput {
        conversation_id: conversation_id.into(),
        run_id: run_id.into(),
        assistant_message_id: assistant_message_id.into(),
        model_batch_index: batch,
        expected_next_trace_sequence: expected_sequence,
        maximum: MAX_BATCH_MESSAGES,
    }
}

fn wait_input(
    caller: &str,
    conversation: &str,
    run: &str,
    assistant: &str,
    batch: u64,
    targets: &[&str],
) -> PollAgentWaitInput {
    PollAgentWaitInput {
        caller_agent_id: caller.into(),
        conversation_id: conversation.into(),
        run_id: run.into(),
        assistant_message_id: assistant.into(),
        model_batch_index: batch,
        target_agent_ids: targets.iter().map(|target| (*target).into()).collect(),
        maximum_messages: MAX_SAFE_BOUNDARY_MESSAGES,
    }
}

#[test]
fn safe_boundary_binds_fifo_messages_once_and_preserves_durable_receipts() {
    let mut connection = setup_tree();
    begin_empty_turn(
        &connection,
        "conversation-child",
        "run-child",
        "assistant-child",
        10,
    );
    for suffix in ["one", "two"] {
        agent_graph_repository::follow_up_agent(
            &mut connection,
            &SendAgentMessageRequest {
                sender_agent_id: "agent-root".into(),
                recipient_agent_id: "agent-child".into(),
                request_id: format!("followup-{suffix}"),
                content: format!("payload {suffix}"),
            },
            11,
        )
        .unwrap();
    }
    let input = safe_input("conversation-child", "run-child", "assistant-child", 1, 0);
    let first = bind_safe_boundary(&mut connection, &input, 12)
        .unwrap()
        .unwrap();
    assert_eq!(first.messages.len(), 2);
    assert_eq!(
        first.messages[0].mailbox_sequence + 1,
        first.messages[1].mailbox_sequence
    );
    assert_eq!(first.messages[0].trace_sequence, Some(0));
    assert_eq!(first.messages[1].trace_sequence, Some(1));
    assert_eq!(first.messages[0].content, "payload one");
    let durable_trace =
        conversation_trace_repository::get_trace_for_message(&connection, "assistant-child")
            .unwrap()
            .unwrap();
    let durable_content = match &durable_trace.items[0] {
        crate::ConversationTurnTraceItem::AgentMailboxDelivery { content, .. } => content.clone(),
        item => panic!("unexpected trace item: {item:?}"),
    };
    let durable_model =
        conversation_model_context_repository::get_log_for_message(&connection, "assistant-child")
            .unwrap()
            .unwrap();
    assert_eq!(durable_model.items[0].content, durable_content);
    let mut runtime_replay = crate::conversation_trace::ConversationTraceRecorder::default();
    let runtime_content = runtime_replay
        .record_agent_mailbox_delivery(
            0,
            &first.receipt.receipt_id,
            &first.messages[0].message_id,
            &first.messages[0].sender_agent_id,
            &first.messages[0].sender_task_name,
            &first.messages[0].sender_task_path,
            first.messages[0].kind,
            &first.messages[0].content,
            first.messages[0].created_at,
        )
        .unwrap()
        .unwrap();
    assert_eq!(runtime_content, durable_content);
    let envelope: serde_json::Value = serde_json::from_str(&durable_content).unwrap();
    assert_eq!(envelope["type"], "agent_collaboration_input");
    assert_eq!(envelope["payload"], "payload one");
    assert_eq!(
        bind_safe_boundary(&mut connection, &input, 13).unwrap(),
        Some(first.clone())
    );
    let satisfied: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM agent_wake_requests WHERE status = 'satisfied'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(satisfied, 2);
    let positions = connection
        .prepare(
            "SELECT role FROM messages WHERE conversation_id = 'conversation-child'
                 ORDER BY position ASC",
        )
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(positions, vec!["user", "user", "assistant"]);
    assert!(connection
        .execute(
            "DELETE FROM agent_model_batch_receipts WHERE receipt_id = ?1",
            [&first.receipt.receipt_id],
        )
        .is_err());
    assert!(connection
        .execute(
            "DELETE FROM agent_model_batch_receipt_items WHERE receipt_id = ?1",
            [&first.receipt.receipt_id],
        )
        .is_err());
    assert!(connection
        .execute(
            "DELETE FROM conversation_turn_traces WHERE assistant_message_id = 'assistant-child'",
            [],
        )
        .is_err());
    let late = agent_graph_repository::follow_up_agent(
        &mut connection,
        &SendAgentMessageRequest {
            sender_agent_id: "agent-root".into(),
            recipient_agent_id: "agent-child".into(),
            request_id: "followup-after-closed-batch".into(),
            content: "late payload".into(),
        },
        14,
    )
    .unwrap();
    agent_graph_repository::project_pending_agent_messages_in_transaction(
        &connection,
        "agent-child",
        "late-claim",
        15,
        1,
    )
    .unwrap();
    assert!(connection
        .execute(
            "INSERT INTO agent_model_batch_receipt_items (
                     receipt_id, message_id, ordinal, mailbox_sequence,
                     delivery_path, trace_sequence, bound_at
                 ) VALUES (?1, ?2, 99, ?3, 'turn_start', NULL, 15)",
            params![
                &first.receipt.receipt_id,
                &late.message.message_id,
                i64::try_from(late.message.sequence).unwrap()
            ],
        )
        .is_err());
}

#[test]
fn safe_boundary_enforces_fifo_model_budget_and_leaves_excess_pending() {
    let mut connection = setup_tree();
    begin_empty_turn(
        &connection,
        "conversation-child",
        "run-budget",
        "assistant-budget",
        20,
    );
    for index in 0..6 {
        agent_graph_repository::follow_up_agent(
            &mut connection,
            &SendAgentMessageRequest {
                sender_agent_id: "agent-root".into(),
                recipient_agent_id: "agent-child".into(),
                request_id: format!("budget-{index}"),
                content: format!("{index}:{}", "a".repeat(100_000)),
            },
            21 + index,
        )
        .unwrap();
    }
    let first = bind_safe_boundary(
        &mut connection,
        &safe_input("conversation-child", "run-budget", "assistant-budget", 1, 0),
        30,
    )
    .unwrap()
    .unwrap();
    assert!(!first.messages.is_empty());
    assert!(first.messages.len() < 6);
    let projected = connection
        .prepare(
            "SELECT json_extract(item_json, '$.content')
                 FROM conversation_turn_trace_items
                 WHERE assistant_message_id = 'assistant-budget'
                   AND item_kind = 'agent_mailbox_delivery'
                 ORDER BY sequence",
        )
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert!(projected.iter().map(String::len).sum::<usize>() <= MAX_SAFE_BOUNDARY_MODEL_BYTES);
    assert!(projected.iter().all(|content| {
        serde_json::from_str::<serde_json::Value>(content).unwrap()["payloadTruncated"] == true
    }));
    let second = bind_safe_boundary(
        &mut connection,
        &safe_input(
            "conversation-child",
            "run-budget",
            "assistant-budget",
            2,
            first.messages.len() as u64,
        ),
        31,
    )
    .unwrap()
    .unwrap();
    assert_eq!(first.messages.len() + second.messages.len(), 6);
}

#[test]
fn wait_freezes_status_only_first_ready_snapshot_across_sampling_and_restart() {
    let mut connection = setup_tree();
    begin_wait_turn(
        &connection,
        "conversation-root",
        "run-root",
        "assistant-root",
        "wait-status-freeze",
        40,
    );
    connection
        .execute(
            "INSERT INTO agent_wake_requests (
                     wake_id, schema_version, root_agent_id, agent_id, requester_agent_id,
                     request_id, source_agent_message_id, status, status_revision,
                     created_at, completed_at
                 ) VALUES (
                     'wake-completed', 1, 'agent-root', 'agent-child', 'agent-root',
                     'wake-completed-request', NULL, 'completed', 1, 41, 41
                 )",
            [],
        )
        .unwrap();
    let input = wait_input(
        "agent-root",
        "conversation-root",
        "run-root",
        "assistant-root",
        1,
        &["agent-child"],
    );
    let first = poll_wait_ready(&mut connection, &input, 42)
        .unwrap()
        .unwrap();
    assert!(first.targets[0].messages.is_empty());
    assert_eq!(
        first.targets[0].latest_wake_status,
        Some(AgentWakeStatus::Completed)
    );
    assert_eq!(first.receipt.sampling_bound_at, Some(42));
    connection
        .execute(
            "INSERT INTO agent_wake_requests (
                     wake_id, schema_version, root_agent_id, agent_id, requester_agent_id,
                     request_id, source_agent_message_id, status, status_revision,
                     terminal_error, created_at, completed_at
                 ) VALUES (
                     'wake-later', 1, 'agent-root', 'agent-child', 'agent-root',
                     'wake-later-request', NULL, 'failed', 1, 'later', 44, 44
                 )",
            [],
        )
        .unwrap();
    let replay = poll_wait_ready(&mut connection, &input, 45)
        .unwrap()
        .unwrap();
    assert_eq!(replay.receipt.receipt_id, first.receipt.receipt_id);
    assert_eq!(replay.targets, first.targets);
    assert_eq!(replay.receipt.sampling_bound_at, Some(42));
    assert!(connection
        .execute(
            "DELETE FROM agent_model_batch_receipt_targets WHERE receipt_id = ?1",
            [&first.receipt.receipt_id],
        )
        .is_err());
    assert!(connection
        .execute(
            "INSERT INTO agent_model_batch_receipt_targets (
                     receipt_id, target_agent_id, ordinal, target_status_version,
                     latest_wake_sequence, latest_wake_status_revision, latest_wake_status,
                     display_status, frozen_at
                 ) VALUES (?1, 'agent-grand', 99, 1, NULL, NULL, NULL, 'idle', 45)",
            [&first.receipt.receipt_id],
        )
        .is_err());

    let next_trace_sequence =
        conversation_trace_repository::get_trace_for_message(&connection, "assistant-root")
            .unwrap()
            .unwrap()
            .items
            .last()
            .map(crate::ConversationTurnTraceItem::sequence)
            .map_or(0, |sequence| sequence.saturating_add(1));
    let closed = safe_input(
        "conversation-root",
        "run-root",
        "assistant-root",
        2,
        next_trace_sequence,
    );
    assert!(bind_safe_boundary(&mut connection, &closed, 46)
        .unwrap()
        .is_none());
    let closed_wait = wait_input(
        "agent-root",
        "conversation-root",
        "run-root",
        "assistant-root",
        2,
        &["agent-child"],
    );
    assert!(matches!(
        poll_wait_ready(&mut connection, &closed_wait, 47),
        Err(AgentGraphError::Conflict(_))
    ));
}

#[test]
fn wait_readiness_probe_keeps_idle_cursor_unchanged_and_detects_status_advance() {
    let mut connection = setup_tree();
    begin_wait_turn(
        &connection,
        "conversation-root",
        "run-probe",
        "assistant-probe",
        "wait-probe",
        35,
    );
    let input = wait_input(
        "agent-root",
        "conversation-root",
        "run-probe",
        "assistant-probe",
        1,
        &["agent-child"],
    );
    assert!(poll_wait_ready(&mut connection, &input, 36)
        .unwrap()
        .is_none());
    let cursor_updated_at = connection
        .query_row(
            "SELECT updated_at FROM agent_collaboration_cursors
             WHERE caller_agent_id = 'agent-root'
               AND run_id = 'run-probe'
               AND target_agent_id = 'agent-child'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap();
    let changes_before = connection.total_changes();

    assert!(!probe_wait_ready(&connection, &input, 36).unwrap());
    assert_eq!(connection.total_changes(), changes_before);
    assert_eq!(
        connection
            .query_row(
                "SELECT updated_at FROM agent_collaboration_cursors
                 WHERE caller_agent_id = 'agent-root'
                   AND run_id = 'run-probe'
                   AND target_agent_id = 'agent-child'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        cursor_updated_at
    );

    connection
        .execute(
            "INSERT INTO agent_wake_requests (
                 wake_id, schema_version, root_agent_id, agent_id, requester_agent_id,
                 request_id, source_agent_message_id, status, status_revision,
                 created_at, completed_at
             ) VALUES (
                 'wake-probe', 1, 'agent-root', 'agent-child', 'agent-root',
                 'wake-probe-request', NULL, 'completed', 1, 37, 37
             )",
            [],
        )
        .unwrap();
    assert!(probe_wait_ready(&connection, &input, 38).unwrap());
}

#[test]
fn wait_consumes_only_caller_inbox_and_coalesces_result_wake() {
    let mut connection = setup_tree();
    begin_wait_turn(
        &connection,
        "conversation-child",
        "run-wait",
        "assistant-wait",
        "wait-result",
        50,
    );
    let result = agent_graph_repository::enqueue_agent_message(
        &mut connection,
        &EnqueueAgentMessageInput {
            message_id: "message-result".into(),
            root_agent_id: "agent-root".into(),
            sender_agent_id: "agent-grand".into(),
            recipient_agent_id: "agent-child".into(),
            request_id: "result-message-request".into(),
            kind: AgentMailboxKind::Result,
            content: result_payload(
                "agent-grand",
                "grand",
                "/root/child/grand",
                "grandchild result",
            ),
            projection_message_id: "projection-result".into(),
        },
        51,
    )
    .unwrap()
    .record()
    .clone();
    agent_graph_repository::enqueue_agent_wake(
        &mut connection,
        &EnqueueAgentWakeInput {
            wake_id: "wake-result".into(),
            root_agent_id: "agent-root".into(),
            agent_id: "agent-child".into(),
            requester_agent_id: "agent-grand".into(),
            request_id: "wake-result-request".into(),
            source_agent_message_id: Some(result.message_id.clone()),
        },
        51,
    )
    .unwrap();
    agent_graph_repository::send_agent_message(
        &mut connection,
        &SendAgentMessageRequest {
            sender_agent_id: "agent-child".into(),
            recipient_agent_id: "agent-grand".into(),
            request_id: "outbound-not-in-caller-inbox".into(),
            content: "outbound".into(),
        },
        52,
    )
    .unwrap();
    let input = wait_input(
        "agent-child",
        "conversation-child",
        "run-wait",
        "assistant-wait",
        1,
        &["agent-grand"],
    );
    let ready = poll_wait_ready(&mut connection, &input, 53)
        .unwrap()
        .unwrap();
    assert_eq!(ready.targets[0].messages.len(), 1);
    assert_eq!(ready.targets[0].messages[0].message_id, "message-result");
    let wake_status: String = connection
        .query_row(
            "SELECT status FROM agent_wake_requests WHERE wake_id = 'wake-result'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(wake_status, "satisfied");
    assert!(matches!(
        poll_wait_ready(
            &mut connection,
            &wait_input(
                "agent-child",
                "conversation-child",
                "run-wait",
                "assistant-wait",
                2,
                &["agent-root"],
            ),
            54,
        ),
        Err(AgentGraphError::Conflict(_))
    ));
    let filtered =
        list_trace_bound_projection_message_ids(&connection, "conversation-child").unwrap();
    assert_eq!(filtered, vec!["projection-result"]);
    assert!(bind_safe_boundary(
        &mut connection,
        &safe_input("conversation-child", "run-wait", "assistant-wait", 1, 0,),
        55,
    )
    .unwrap()
    .is_none());
    let filtered =
        list_trace_bound_projection_message_ids(&connection, "conversation-child").unwrap();
    assert_eq!(filtered, vec!["projection-result"]);
}

#[test]
fn wait_binding_rolls_back_before_a_durable_tool_result_can_be_committed() {
    let mut connection = setup_tree();
    begin_empty_turn(
        &connection,
        "conversation-root",
        "run-wait-rollback",
        "assistant-wait-rollback",
        56,
    );
    let message = agent_graph_repository::enqueue_agent_message(
        &mut connection,
        &EnqueueAgentMessageInput {
            message_id: "message-wait-rollback".into(),
            root_agent_id: "agent-root".into(),
            sender_agent_id: "agent-child".into(),
            recipient_agent_id: "agent-root".into(),
            request_id: "message-wait-rollback-request".into(),
            kind: AgentMailboxKind::Result,
            content: result_payload(
                "agent-child",
                "child",
                "/root/child",
                "authenticated child result",
            ),
            projection_message_id: "projection-wait-rollback".into(),
        },
        57,
    )
    .unwrap()
    .record()
    .clone();
    agent_graph_repository::enqueue_agent_wake(
        &mut connection,
        &EnqueueAgentWakeInput {
            wake_id: "wake-wait-rollback".into(),
            root_agent_id: "agent-root".into(),
            agent_id: "agent-root".into(),
            requester_agent_id: "agent-child".into(),
            request_id: "wake-wait-rollback-request".into(),
            source_agent_message_id: Some(message.message_id.clone()),
        },
        57,
    )
    .unwrap();
    let input = wait_input(
        "agent-root",
        "conversation-root",
        "run-wait-rollback",
        "assistant-wait-rollback",
        1,
        &["agent-child"],
    );
    assert!(matches!(
        poll_wait_ready(&mut connection, &input, 58),
        Err(AgentGraphError::Conflict(_))
    ));
    let (delivery_status, wake_status, receipt_count, projection_count): (
        String,
        String,
        i64,
        i64,
    ) = connection
        .query_row(
            "SELECT mailbox.delivery_status, wake.status,
                        (SELECT COUNT(*) FROM agent_model_batch_receipts
                         WHERE run_id = 'run-wait-rollback'),
                        (SELECT COUNT(*) FROM messages
                         WHERE id = 'projection-wait-rollback')
                 FROM agent_mailbox_messages AS mailbox
                 JOIN agent_wake_requests AS wake
                   ON wake.source_agent_message_id = mailbox.message_id
                 WHERE mailbox.message_id = 'message-wait-rollback'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(delivery_status, "queued");
    assert_eq!(wake_status, "queued");
    assert_eq!(receipt_count, 0);
    assert_eq!(projection_count, 0);

    append_wait_tool_call(
        &connection,
        "conversation-root",
        "run-wait-rollback",
        "assistant-wait-rollback",
        "wait-after-rollback",
        59,
    );
    let ready = poll_wait_ready(&mut connection, &input, 60)
        .unwrap()
        .unwrap();
    assert_eq!(ready.receipt.sampling_bound_at, Some(60));
    assert_eq!(ready.targets[0].messages[0].kind, AgentMailboxKind::Result);
    let trace = conversation_trace_repository::get_trace_for_message(
        &connection,
        "assistant-wait-rollback",
    )
    .unwrap()
    .unwrap();
    assert!(matches!(
        trace.items.last(),
        Some(crate::ConversationTurnTraceItem::ToolResult { tool, .. }) if tool == "wait_agent"
    ));
}

#[test]
fn wait_first_ready_returns_all_ready_targets_and_next_wait_only_new_facts() {
    let mut connection = setup_tree();
    begin_wait_turn(
        &connection,
        "conversation-root",
        "run-wait-many",
        "assistant-wait-many",
        "wait-many-first",
        61,
    );
    for (sender, request, content) in [
        ("agent-child", "many-child-1", "child one"),
        ("agent-grand", "many-grand-1", "grand one"),
    ] {
        agent_graph_repository::send_agent_message(
            &mut connection,
            &SendAgentMessageRequest {
                sender_agent_id: sender.into(),
                recipient_agent_id: "agent-root".into(),
                request_id: request.into(),
                content: content.into(),
            },
            62,
        )
        .unwrap();
    }
    let first = poll_wait_ready(
        &mut connection,
        &wait_input(
            "agent-root",
            "conversation-root",
            "run-wait-many",
            "assistant-wait-many",
            1,
            &["agent-child", "agent-grand"],
        ),
        63,
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        first
            .targets
            .iter()
            .map(|target| target.target_agent_id.as_str())
            .collect::<Vec<_>>(),
        vec!["agent-child", "agent-grand"]
    );
    assert!(first
        .targets
        .iter()
        .all(|target| target.messages.len() == 1));

    append_wait_tool_call(
        &connection,
        "conversation-root",
        "run-wait-many",
        "assistant-wait-many",
        "wait-many-second",
        64,
    );
    agent_graph_repository::send_agent_message(
        &mut connection,
        &SendAgentMessageRequest {
            sender_agent_id: "agent-child".into(),
            recipient_agent_id: "agent-root".into(),
            request_id: "many-child-2".into(),
            content: "child two".into(),
        },
        65,
    )
    .unwrap();
    let second = poll_wait_ready(
        &mut connection,
        &wait_input(
            "agent-root",
            "conversation-root",
            "run-wait-many",
            "assistant-wait-many",
            2,
            &["agent-child", "agent-grand"],
        ),
        66,
    )
    .unwrap()
    .unwrap();
    assert_eq!(second.targets.len(), 1);
    assert_eq!(second.targets[0].target_agent_id, "agent-child");
    assert_eq!(second.targets[0].messages.len(), 1);
    let model_input: serde_json::Value =
        serde_json::from_str(&second.targets[0].messages[0].content).unwrap();
    assert_eq!(model_input["payload"], "child two");
}

#[test]
fn wait_enforces_global_fifo_message_and_byte_budget_leaving_excess_pending() {
    let mut connection = setup_tree();
    begin_wait_turn(
        &connection,
        "conversation-root",
        "run-wait-budget",
        "assistant-wait-budget",
        "wait-budget-first",
        67,
    );
    for index in 0..70 {
        let sender = if index % 2 == 0 {
            "agent-child"
        } else {
            "agent-grand"
        };
        agent_graph_repository::send_agent_message(
            &mut connection,
            &SendAgentMessageRequest {
                sender_agent_id: sender.into(),
                recipient_agent_id: "agent-root".into(),
                request_id: format!("wait-budget-{index}"),
                content: format!("{index:03}:{}", "b".repeat(3_000)),
            },
            68 + index,
        )
        .unwrap();
    }
    let first = poll_wait_ready(
        &mut connection,
        &wait_input(
            "agent-root",
            "conversation-root",
            "run-wait-budget",
            "assistant-wait-budget",
            1,
            &["agent-child", "agent-grand"],
        ),
        140,
    )
    .unwrap()
    .unwrap();
    let first_messages = first
        .targets
        .iter()
        .flat_map(|target| target.messages.iter())
        .count();
    assert!(first_messages > 0);
    assert!(
        first_messages < 64,
        "the shared 128KiB budget must bind first"
    );
    let result_bytes = connection
        .query_row(
            "SELECT length(CAST(json_extract(item_json, '$.observation') AS BLOB))
                 FROM conversation_turn_trace_items
                 WHERE assistant_message_id = 'assistant-wait-budget'
                   AND item_kind = 'tool_result'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap();
    assert!(result_bytes <= i64::try_from(MAX_SAFE_BOUNDARY_MODEL_BYTES).unwrap());
    let pending: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM agent_mailbox_messages AS mailbox
                 WHERE recipient_agent_id = 'agent-root'
                   AND NOT EXISTS (
                       SELECT 1 FROM agent_model_batch_receipt_items AS item
                       WHERE item.message_id = mailbox.message_id
                   )",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(pending, 70 - i64::try_from(first_messages).unwrap());
    let bound_sequences = connection
        .prepare(
            "SELECT mailbox_sequence FROM agent_model_batch_receipt_items
                 WHERE receipt_id = ?1 ORDER BY ordinal",
        )
        .unwrap()
        .query_map([&first.receipt.receipt_id], |row| row.get::<_, i64>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    let expected_prefix = connection
        .prepare(
            "SELECT sequence FROM agent_mailbox_messages
                 WHERE recipient_agent_id = 'agent-root' ORDER BY sequence LIMIT ?1",
        )
        .unwrap()
        .query_map([i64::try_from(first_messages).unwrap()], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(bound_sequences, expected_prefix);

    // Recreate the live Runtime's raw checkpoint view of the same Tool exchange and let the
    // normal terminal projector bound it. The resulting trace must extend the ToolResult
    // prefix which `poll_wait_ready` committed above byte-for-byte, including archive
    // truncation metadata. This is the production precommit -> terminal boundary which used
    // to fail only when the wait payload crossed the generic history-projection threshold.
    let call = crate::AgentToolCall {
        id: "wait-budget-first".to_string(),
        tool: "wait_agent".to_string(),
        args: json!({"targets": ["agent-child"]}),
        approval_status: crate::AgentApprovalStatus::NotRequired,
        reason: None,
    };
    let result = AgentToolResult {
        exact_archive_file: None,
        call_id: call.id.clone(),
        tool: call.tool.clone(),
        ok: true,
        result: Some(crate::agent_wait_model_value(&first.targets).unwrap()),
        error: None,
    };
    let mut runtime = crate::conversation_trace::ConversationTraceRecorder::default();
    runtime.record_tool_call(&call).unwrap();
    runtime.record_tool_result(&call, &result);
    let terminal = runtime.finish(
        "run-wait-budget",
        "conversation-root",
        "assistant-wait-budget",
        ConversationTurnTraceTerminalStatus::Completed,
        None,
    );
    conversation_trace_repository::commit_trace_in_connection(&connection, &terminal, 67, 141)
        .unwrap();
    assert!(terminal.truncated);
    assert!(matches!(
        terminal.items.last(),
        Some(crate::ConversationTurnTraceItem::ToolResult {
            archive: crate::ConversationHistoryArchiveTraceMetadata {
                history_projection_truncated: true,
                ..
            },
            ..
        })
    ));
}

#[test]
fn wait_truncates_oversized_fifo_head_and_caps_distinct_targets() {
    let mut connection = setup_tree();
    begin_wait_turn(
        &connection,
        "conversation-root",
        "run-wait-oversized",
        "assistant-wait-oversized",
        "wait-oversized",
        141,
    );
    for (request, sender, content) in [
        (
            "oversized-first",
            "agent-child",
            format!("first:{}", "x".repeat(TEST_MAX_MESSAGE_BYTES - 6)),
        ),
        ("small-second", "agent-grand", "second".to_string()),
    ] {
        agent_graph_repository::send_agent_message(
            &mut connection,
            &SendAgentMessageRequest {
                sender_agent_id: sender.into(),
                recipient_agent_id: "agent-root".into(),
                request_id: request.into(),
                content,
            },
            142,
        )
        .unwrap();
    }
    let ready = poll_wait_ready(
        &mut connection,
        &wait_input(
            "agent-root",
            "conversation-root",
            "run-wait-oversized",
            "assistant-wait-oversized",
            1,
            &["agent-grand", "agent-child"],
        ),
        143,
    )
    .unwrap()
    .unwrap();
    let sequences = connection
        .prepare(
            "SELECT mailbox_sequence FROM agent_model_batch_receipt_items
                 WHERE receipt_id = ?1 ORDER BY ordinal",
        )
        .unwrap()
        .query_map([&ready.receipt.receipt_id], |row| row.get::<_, i64>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(sequences.len(), 2);
    assert!(sequences[0] < sequences[1]);
    let first = ready
        .targets
        .iter()
        .flat_map(|target| target.messages.iter())
        .find(|message| message.mailbox_sequence == u64::try_from(sequences[0]).unwrap())
        .unwrap();
    let envelope: serde_json::Value = serde_json::from_str(&first.content).unwrap();
    assert_eq!(envelope["senderTaskName"], "child");
    assert!(envelope.get("senderAgentId").is_none());
    assert_eq!(envelope["payloadTruncated"], true);
    assert!(
        first.content.len() <= crate::conversation_trace::AGENT_MAILBOX_MODEL_ENVELOPE_MAX_BYTES
    );

    let too_many = PollAgentWaitInput {
        model_batch_index: 2,
        target_agent_ids: (0..=MAX_WAIT_TARGETS)
            .map(|index| format!("target-{index}"))
            .collect(),
        ..wait_input(
            "agent-root",
            "conversation-root",
            "run-wait-oversized",
            "assistant-wait-oversized",
            2,
            &["agent-child"],
        )
    };
    assert!(matches!(
        poll_wait_ready(&mut connection, &too_many, 144),
        Err(AgentGraphError::InvalidInput {
            field: "target_agent_ids",
            ..
        })
    ));
}

#[test]
fn wait_status_version_observes_hidden_active_completion_and_lifecycle_changes() {
    let mut connection = setup_tree();
    begin_wait_turn(
        &connection,
        "conversation-root",
        "run-status",
        "assistant-status",
        "wait-status-active",
        60,
    );
    connection
        .execute(
            "INSERT INTO agent_wake_requests (
                     wake_id, schema_version, root_agent_id, agent_id, requester_agent_id,
                     request_id, source_agent_message_id, status, status_revision,
                     claim_token, lease_expires_at, created_at, claimed_at, started_at
                 ) VALUES (
                     'wake-older-active', 1, 'agent-root', 'agent-child', 'agent-root',
                     'older-active-request', NULL, 'running', 1,
                     'older-active-claim', 1000, 61, 61, 61
                 )",
            [],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO agent_wake_requests (
                     wake_id, schema_version, root_agent_id, agent_id, requester_agent_id,
                     request_id, source_agent_message_id, status, status_revision, created_at
                 ) VALUES (
                     'wake-newer-queued', 1, 'agent-root', 'agent-child', 'agent-root',
                     'newer-queued-request', NULL, 'queued', 1, 62
                 )",
            [],
        )
        .unwrap();
    let baseline = wait_input(
        "agent-root",
        "conversation-root",
        "run-status",
        "assistant-status",
        1,
        &["agent-child"],
    );
    assert!(poll_wait_ready(&mut connection, &baseline, 63)
        .unwrap()
        .is_none());
    connection
        .execute(
            "UPDATE agent_wake_requests
                 SET status = 'failed', status_revision = status_revision + 1,
                     terminal_error = 'failed', completed_at = 64
                 WHERE wake_id = 'wake-older-active'",
            [],
        )
        .unwrap();
    let advanced = poll_wait_ready(
        &mut connection,
        &wait_input(
            "agent-root",
            "conversation-root",
            "run-status",
            "assistant-status",
            2,
            &["agent-child"],
        ),
        65,
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        advanced.targets[0].latest_wake_status,
        Some(AgentWakeStatus::Queued)
    );
    assert_eq!(
        advanced.targets[0].display_status,
        crate::AgentDisplayStatus::Queued
    );

    append_wait_tool_call(
        &connection,
        "conversation-root",
        "run-status",
        "assistant-status",
        "wait-status-lifecycle",
        65,
    );

    let lifecycle_baseline = wait_input(
        "agent-root",
        "conversation-root",
        "run-status",
        "assistant-status",
        3,
        &["agent-grand"],
    );
    assert!(poll_wait_ready(&mut connection, &lifecycle_baseline, 66)
        .unwrap()
        .is_none());
    agent_graph_repository::transition_agent_lifecycle(
        &mut connection,
        "agent-grand",
        1,
        AgentLifecycle::Active,
        AgentLifecycle::Disabled,
        67,
    )
    .unwrap();
    let lifecycle = poll_wait_ready(
        &mut connection,
        &wait_input(
            "agent-root",
            "conversation-root",
            "run-status",
            "assistant-status",
            4,
            &["agent-grand"],
        ),
        68,
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        lifecycle.targets[0].display_status,
        crate::AgentDisplayStatus::Disabled
    );
}

#[test]
fn sampled_wait_status_is_atomic_and_does_not_replay_into_a_new_run() {
    let mut connection = setup_tree();
    begin_wait_turn(
        &connection,
        "conversation-root",
        "run-before-crash",
        "assistant-before-crash",
        "wait-before-crash",
        80,
    );
    connection
        .execute(
            "INSERT INTO agent_wake_requests (
                     wake_id, schema_version, root_agent_id, agent_id, requester_agent_id,
                     request_id, source_agent_message_id, status, status_revision,
                     claim_token, lease_expires_at, created_at, claimed_at, started_at
                 ) VALUES (
                     'wake-waiting-crash', 1, 'agent-root', 'agent-child', 'agent-root',
                     'wake-waiting-crash-request', NULL, 'running', 1,
                     'wake-waiting-crash-claim', 1000, 81, 81, 81
                 )",
            [],
        )
        .unwrap();
    let baseline = wait_input(
        "agent-root",
        "conversation-root",
        "run-before-crash",
        "assistant-before-crash",
        1,
        &["agent-child"],
    );
    assert!(poll_wait_ready(&mut connection, &baseline, 82)
        .unwrap()
        .is_none());
    connection
        .execute(
            "UPDATE agent_wake_requests
                 SET status = 'waiting_for_approval', status_revision = status_revision + 1
                 WHERE wake_id = 'wake-waiting-crash'",
            [],
        )
        .unwrap();
    let ready = poll_wait_ready(
        &mut connection,
        &wait_input(
            "agent-root",
            "conversation-root",
            "run-before-crash",
            "assistant-before-crash",
            2,
            &["agent-child"],
        ),
        83,
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        ready.targets[0].display_status,
        crate::AgentDisplayStatus::WaitingApproval
    );

    let mut crashed =
        conversation_trace_repository::get_trace_for_message(&connection, "assistant-before-crash")
            .unwrap()
            .unwrap();
    crashed.terminal_status = ConversationTurnTraceTerminalStatus::Failed;
    crashed.terminal_error = Some("outcome unknown".into());
    conversation_trace_repository::commit_trace_in_connection(&connection, &crashed, 80, 84)
        .unwrap();
    begin_wait_turn(
        &connection,
        "conversation-root",
        "run-after-crash",
        "assistant-after-crash",
        "wait-after-crash",
        85,
    );
    let rebound = poll_wait_ready(
        &mut connection,
        &wait_input(
            "agent-root",
            "conversation-root",
            "run-after-crash",
            "assistant-after-crash",
            1,
            &["agent-child"],
        ),
        86,
    )
    .unwrap();
    assert!(rebound.is_none());
    assert_eq!(ready.receipt.sampling_bound_at, Some(83));
}

#[test]
fn legacy_open_message_wait_replays_authenticated_snapshot_and_closes_once() {
    let mut connection = setup_tree();
    begin_wait_turn(
        &connection,
        "conversation-root",
        "run-open-wait",
        "assistant-open-wait",
        "wait-open-old",
        90,
    );
    let message = agent_graph_repository::enqueue_agent_message(
        &mut connection,
        &EnqueueAgentMessageInput {
            message_id: "message-open-wait".into(),
            root_agent_id: "agent-root".into(),
            sender_agent_id: "agent-child".into(),
            recipient_agent_id: "agent-root".into(),
            request_id: "message-open-wait-request".into(),
            kind: AgentMailboxKind::Result,
            content: result_payload(
                "agent-child",
                "child",
                "/root/child",
                "frozen child payload",
            ),
            projection_message_id: "projection-open-wait".into(),
        },
        91,
    )
    .unwrap()
    .record()
    .clone();
    agent_graph_repository::enqueue_agent_wake(
        &mut connection,
        &EnqueueAgentWakeInput {
            wake_id: "wake-open-wait".into(),
            root_agent_id: "agent-root".into(),
            agent_id: "agent-root".into(),
            requester_agent_id: "agent-child".into(),
            request_id: "wake-open-wait-request".into(),
            source_agent_message_id: Some(message.message_id.clone()),
        },
        91,
    )
    .unwrap();
    agent_graph_repository::project_pending_agent_messages_in_transaction(
        &connection,
        "agent-root",
        "open-wait-claim",
        92,
        1,
    )
    .unwrap();
    let receipt = ensure_receipt(
        &connection,
        "agent-root",
        "conversation-root",
        "run-open-wait",
        "assistant-open-wait",
        1,
        92,
    )
    .unwrap();
    let candidate = query_acknowledged_candidate(&connection, &message.message_id)
        .unwrap()
        .unwrap();
    insert_receipt_item(
        &connection,
        &receipt.receipt_id,
        &candidate,
        0,
        AgentDeliveryPath::WaitAgent,
        None,
        92,
    )
    .unwrap();
    agent_graph_repository::satisfy_agent_wake_by_source_message_in_transaction(
        &connection,
        &message.message_id,
        92,
    )
    .unwrap();
    let target_version = target_status_version(&connection, "agent-child").unwrap();
    let frozen = AgentWaitTargetSnapshot {
        target_agent_id: "agent-child".into(),
        target_task_name: "child".into(),
        messages: vec![wait_delivered(&candidate).unwrap()],
        target_status_version: target_version,
        latest_wake_sequence: None,
        latest_wake_status_revision: None,
        latest_wake_status: None,
        display_status: crate::AgentDisplayStatus::Idle,
    };
    insert_wait_receipt_target(&connection, &receipt.receipt_id, 0, &frozen, 92).unwrap();
    upsert_cursor(
        &connection,
        "agent-root",
        "run-open-wait",
        "agent-child",
        target_version,
        candidate.mailbox_sequence,
        0,
        0,
        92,
    )
    .unwrap();

    let trace =
        conversation_trace_repository::get_trace_for_message(&connection, "assistant-open-wait")
            .unwrap()
            .unwrap();
    let model_items = conversation_model_context_repository::get_log_for_message(
        &connection,
        "assistant-open-wait",
    )
    .unwrap()
    .unwrap()
    .items;
    let next_sequence = trace
        .items
        .last()
        .map(crate::ConversationTurnTraceItem::sequence)
        .unwrap_or(0)
        .saturating_add(1);
    let terminal = crate::terminal_conversation_trace_from_snapshot(
        ConversationTraceSnapshot {
            items: trace.items,
            model_context_items: model_items,
            next_sequence,
            truncated: trace.truncated,
        },
        "run-open-wait",
        "conversation-root",
        "assistant-open-wait",
        ConversationTurnTraceTerminalStatus::Failed,
        "simulated crash after wait bind",
    )
    .unwrap();
    conversation_trace_repository::commit_trace_in_connection(&connection, &terminal.trace, 90, 93)
        .unwrap();
    conversation_model_context_repository::commit_items_in_connection(
        &connection,
        "conversation-root",
        "assistant-open-wait",
        &terminal.model_context_items,
    )
    .unwrap();

    begin_wait_turn(
        &connection,
        "conversation-root",
        "run-recover-wait",
        "assistant-recover-wait",
        "wait-open-recover",
        94,
    );
    let replay = poll_wait_ready(
        &mut connection,
        &wait_input(
            "agent-root",
            "conversation-root",
            "run-recover-wait",
            "assistant-recover-wait",
            1,
            &["agent-child"],
        ),
        95,
    )
    .unwrap()
    .unwrap();
    assert_eq!(replay.receipt.sampling_bound_at, Some(95));
    assert_eq!(replay.targets, vec![frozen]);
    let old_sampled: i64 = connection
        .query_row(
            "SELECT sampling_bound_at FROM agent_model_batch_receipts WHERE receipt_id = ?1",
            [&receipt.receipt_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(old_sampled, 95);
    let replay_source: String = connection
        .query_row(
            "SELECT source_receipt_id FROM agent_model_batch_receipt_replays
                 WHERE receipt_id = ?1",
            [&replay.receipt.receipt_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(replay_source, receipt.receipt_id);
    let unique_message_effects: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM agent_model_batch_receipt_items
                 WHERE message_id = 'message-open-wait'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(unique_message_effects, 1);
    let recovered_trace =
        conversation_trace_repository::get_trace_for_message(&connection, "assistant-recover-wait")
            .unwrap()
            .unwrap();
    let observation = match recovered_trace.items.last().unwrap() {
        crate::ConversationTurnTraceItem::ToolResult { observation, .. } => observation,
        item => panic!("unexpected recovered trace item: {item:?}"),
    };
    let recovered_message = &observation["targets"][0]["messages"][0];
    assert!(observation.get("sourceReceiptId").is_none());
    assert_eq!(observation["targets"][0]["taskName"], "child");
    assert_eq!(recovered_message["senderTaskName"], "child");
    assert!(recovered_message.get("senderAgentId").is_none());
    assert_eq!(recovered_message["kind"], "result");
    let authenticated: serde_json::Value =
        serde_json::from_str(recovered_message["content"].as_str().unwrap()).unwrap();
    assert_eq!(authenticated["senderTaskName"], "child");
    assert!(authenticated.get("senderAgentId").is_none());
    assert_eq!(authenticated["kind"], "result");
    let payload: serde_json::Value =
        serde_json::from_str(authenticated["payload"].as_str().unwrap()).unwrap();
    assert_eq!(payload["summary"], "frozen child payload");
    assert!(payload.get("childAgentId").is_none());
}

#[test]
fn stale_terminal_full_save_preserves_dynamic_projection_order_and_receipt() {
    let mut connection = setup_tree();
    connection
        .execute(
            "INSERT INTO messages (
                     id, conversation_id, role, content, status, created_at, position
                 ) VALUES (
                     'assistant-previous', 'conversation-child', 'assistant',
                     'previous answer', 'sent', 69, 0
                 )",
            [],
        )
        .unwrap();
    begin_empty_turn(
        &connection,
        "conversation-child",
        "run-merge",
        "assistant-merge",
        70,
    );
    let mut stale = chat_repository::get_conversation(&connection, "conversation-child")
        .unwrap()
        .unwrap();
    agent_graph_repository::follow_up_agent(
        &mut connection,
        &SendAgentMessageRequest {
            sender_agent_id: "agent-root".into(),
            recipient_agent_id: "agent-child".into(),
            request_id: "merge-followup".into(),
            content: "dynamic input".into(),
        },
        71,
    )
    .unwrap();
    let delivery = bind_safe_boundary(
        &mut connection,
        &safe_input("conversation-child", "run-merge", "assistant-merge", 1, 0),
        72,
    )
    .unwrap()
    .unwrap();
    let assistant = stale
        .messages
        .iter_mut()
        .find(|message| message.id == "assistant-merge")
        .unwrap();
    assistant.content = "terminal answer".into();
    assistant.status = Some("sent".into());
    stale.updated_at = 73;
    chat_repository::save_conversation(&mut connection, stale).unwrap();
    let mut terminal =
        conversation_trace_repository::get_trace_for_message(&connection, "assistant-merge")
            .unwrap()
            .unwrap();
    terminal.terminal_status = ConversationTurnTraceTerminalStatus::Completed;
    conversation_trace_repository::commit_trace_in_connection(&connection, &terminal, 70, 74)
        .unwrap();
    let reloaded = chat_repository::get_conversation(&connection, "conversation-child")
        .unwrap()
        .unwrap();
    assert_eq!(
        reloaded
            .messages
            .iter()
            .map(|message| (message.role.as_str(), message.content.as_str()))
            .collect::<Vec<_>>(),
        vec![
            ("assistant", "previous answer"),
            ("user", "dynamic input"),
            ("assistant", "terminal answer")
        ]
    );
    let item_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM agent_model_batch_receipt_items
                 WHERE receipt_id = ?1",
            [&delivery.receipt.receipt_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(item_count, 1);
    let trace_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM conversation_turn_trace_items
                 WHERE assistant_message_id = 'assistant-merge'
                   AND item_kind = 'agent_mailbox_delivery'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(trace_count, 1);
}

#[test]
fn mailbox_unbound_count_quota_is_typed_idempotent_and_delete_protected() {
    let mut connection = setup_tree();
    let first = EnqueueAgentMessageInput {
        message_id: "quota-count-message-0".into(),
        root_agent_id: "agent-root".into(),
        sender_agent_id: "agent-child".into(),
        recipient_agent_id: "agent-root".into(),
        request_id: "quota-count-request-0".into(),
        kind: AgentMailboxKind::Message,
        content: "x".into(),
        projection_message_id: "quota-count-projection-0".into(),
    };
    agent_graph_repository::enqueue_agent_message(&mut connection, &first, 100).unwrap();
    for index in 1..TEST_MAX_UNBOUND_ORDINARY_MAILBOX_MESSAGES {
        connection
            .execute(
                "INSERT INTO agent_mailbox_messages (
                         message_id, schema_version, root_agent_id, sender_agent_id,
                         recipient_agent_id, request_id, kind, content, projection_message_id,
                         delivery_status, created_at
                     ) VALUES (?1, 1, 'agent-root', 'agent-child', 'agent-root', ?2,
                               'message', 'x', ?3, 'queued', 100)",
                params![
                    format!("quota-count-message-{index}"),
                    format!("quota-count-request-{index}"),
                    format!("quota-count-projection-{index}"),
                ],
            )
            .unwrap();
    }
    assert!(matches!(
        agent_graph_repository::enqueue_agent_message(&mut connection, &first, 101).unwrap(),
        crate::IdempotentCreate::Existing(_)
    ));
    let overflow = EnqueueAgentMessageInput {
        message_id: "quota-count-overflow".into(),
        request_id: "quota-count-overflow".into(),
        projection_message_id: "quota-count-overflow-projection".into(),
        ..first.clone()
    };
    assert!(matches!(
        agent_graph_repository::enqueue_agent_message(&mut connection, &overflow, 102),
        Err(AgentGraphError::ResourceLimit {
            resource: "ordinary unbound Mailbox messages per recipient",
            limit: TEST_MAX_UNBOUND_ORDINARY_MAILBOX_MESSAGES,
        })
    ));
    agent_graph_repository::enqueue_agent_wake(
        &mut connection,
        &EnqueueAgentWakeInput {
            wake_id: "quota-result-wake".into(),
            root_agent_id: "agent-root".into(),
            agent_id: "agent-child".into(),
            requester_agent_id: "agent-root".into(),
            request_id: "quota-result-wake".into(),
            source_agent_message_id: None,
        },
        103,
    )
    .unwrap();
    agent_graph_repository::claim_next_agent_wake(
        &mut connection,
        "agent-child",
        "quota-result-claim",
        104,
    )
    .unwrap()
    .unwrap();
    agent_graph_repository::transition_agent_wake(
        &mut connection,
        "quota-result-wake",
        AgentWakeStatus::Claimed,
        AgentWakeStatus::Running,
        Some("quota-result-claim"),
        105,
    )
    .unwrap();
    let reserved_result = agent_graph_repository::finish_agent_turn_with_result(
        &mut connection,
        &FinishAgentTurnResultInput {
            wake_id: "quota-result-wake".into(),
            expected_status: AgentWakeStatus::Running,
            claim_token: "quota-result-claim".into(),
            terminal_status: AgentWakeStatus::Completed,
            run_id: None,
            assistant_message_id: None,
            summary: "terminal result still persists".into(),
            terminal_error: None,
        },
        106,
    )
    .unwrap()
    .result_message;
    assert_eq!(reserved_result.kind, AgentMailboxKind::Result);
    assert_eq!(reserved_result.sender_agent_id, "agent-child");
    assert_eq!(reserved_result.recipient_agent_id, "agent-root");
    for index in
        1..=(TEST_MAX_UNBOUND_MAILBOX_MESSAGES - TEST_MAX_UNBOUND_ORDINARY_MAILBOX_MESSAGES - 1)
    {
        connection
            .execute(
                "INSERT INTO agent_mailbox_messages (
                         message_id, schema_version, root_agent_id, sender_agent_id,
                         recipient_agent_id, request_id, kind, content, projection_message_id,
                         delivery_status, created_at
                     ) VALUES (?1, 1, 'agent-root', 'agent-child', 'agent-root', ?2,
                               'result', 'reserved', ?3, 'queued', 107)",
                params![
                    format!("quota-count-reserved-{index}"),
                    format!("quota-count-reserved-request-{index}"),
                    format!("quota-count-reserved-projection-{index}"),
                ],
            )
            .unwrap();
    }
    let hard_overflow = EnqueueAgentMessageInput {
        message_id: "quota-count-hard-overflow".into(),
        root_agent_id: "agent-root".into(),
        sender_agent_id: "agent-child".into(),
        recipient_agent_id: "agent-root".into(),
        request_id: "quota-count-hard-overflow".into(),
        kind: AgentMailboxKind::Result,
        content: "hard limit".into(),
        projection_message_id: "quota-count-hard-overflow-projection".into(),
    };
    assert!(matches!(
        agent_graph_repository::enqueue_agent_message(&mut connection, &hard_overflow, 108),
        Err(AgentGraphError::ResourceLimit {
            resource: "unbound Mailbox messages per recipient",
            limit: TEST_MAX_UNBOUND_MAILBOX_MESSAGES,
        })
    ));
    assert!(connection
        .execute(
            "DELETE FROM agent_mailbox_messages WHERE message_id = 'quota-count-message-0'",
            [],
        )
        .is_err());

    // A model receipt releases one settled fact from both quotas. Projection+receipt is the
    // only release mechanism; acknowledging alone intentionally remains charged.
    begin_empty_turn(
        &connection,
        "conversation-root",
        "run-quota-release",
        "assistant-quota-release",
        109,
    );
    agent_graph_repository::project_pending_agent_messages_in_transaction(
        &connection,
        "agent-root",
        "quota-release-claim",
        110,
        1,
    )
    .unwrap();
    bind_safe_boundary(
        &mut connection,
        &BindAgentSafeBoundaryInput {
            conversation_id: "conversation-root".into(),
            run_id: "run-quota-release".into(),
            assistant_message_id: "assistant-quota-release".into(),
            model_batch_index: 1,
            expected_next_trace_sequence: 0,
            maximum: 1,
        },
        111,
    )
    .unwrap()
    .unwrap();
    let released_slot = EnqueueAgentMessageInput {
        message_id: "quota-count-released-slot".into(),
        request_id: "quota-count-released-slot".into(),
        projection_message_id: "quota-count-released-slot-projection".into(),
        ..first
    };
    agent_graph_repository::enqueue_agent_message(&mut connection, &released_slot, 112).unwrap();
}

#[test]
fn mailbox_unbound_byte_quota_and_wake_deletion_are_database_guarded() {
    let mut connection = setup_tree();
    let chunk = "z".repeat(TEST_MAX_MESSAGE_BYTES);
    for index in 0..15 {
        connection
            .execute(
                "INSERT INTO agent_mailbox_messages (
                         message_id, schema_version, root_agent_id, sender_agent_id,
                         recipient_agent_id, request_id, kind, content, projection_message_id,
                         delivery_status, created_at
                     ) VALUES (?1, 1, 'agent-root', 'agent-root', 'agent-child', ?2,
                               'message', ?3, ?4, 'queued', 110)",
                params![
                    format!("quota-byte-message-{index}"),
                    format!("quota-byte-request-{index}"),
                    &chunk,
                    format!("quota-byte-projection-{index}"),
                ],
            )
            .unwrap();
    }
    let overflow = EnqueueAgentMessageInput {
        message_id: "quota-byte-overflow".into(),
        root_agent_id: "agent-root".into(),
        sender_agent_id: "agent-root".into(),
        recipient_agent_id: "agent-child".into(),
        request_id: "quota-byte-overflow".into(),
        kind: AgentMailboxKind::Message,
        content: "one more byte".into(),
        projection_message_id: "quota-byte-overflow-projection".into(),
    };
    assert!(matches!(
        agent_graph_repository::enqueue_agent_message(&mut connection, &overflow, 111),
        Err(AgentGraphError::ResourceLimit {
            resource: "ordinary unbound Mailbox bytes per recipient",
            limit: 15_728_640,
        })
    ));
    connection
        .execute(
            "INSERT INTO agent_wake_requests (
                     wake_id, schema_version, root_agent_id, agent_id, requester_agent_id,
                     request_id, status, status_revision, created_at
                 ) VALUES ('quota-delete-wake', 1, 'agent-root', 'agent-grand',
                           'agent-child', 'quota-delete-wake-request', 'queued', 1, 112)",
            [],
        )
        .unwrap();
    assert!(connection
        .execute(
            "DELETE FROM agent_wake_requests WHERE wake_id = 'quota-delete-wake'",
            [],
        )
        .is_err());
}
