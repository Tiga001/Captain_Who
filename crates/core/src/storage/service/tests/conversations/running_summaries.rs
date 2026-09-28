use super::*;
use rusqlite::hooks::{AuthAction, AuthContext, Authorization};

fn seed_summary_conversation(service: &StorageService, id: &str, status: &str) {
    let mut value = conversation(id, None, &format!("{id}-user"));
    value.messages.push(ChatMessageRecord {
        human_interaction_response: None,
        id: format!("{id}-assistant"),
        role: "assistant".into(),
        content: "history payload".repeat(1_000),
        created_at: 2,
        status: Some(status.into()),
        attachments: Vec::new(),
        folder_references_json: None,
        agent_run_json: None,
        ui_state_json: None,
    });
    service.save_conversation(value).unwrap();
}

fn commit_summary_trace(
    service: &StorageService,
    id: &str,
    status: crate::ConversationTurnTraceTerminalStatus,
) {
    let mut trace = empty_projection_trace(id, &format!("{id}-assistant"), &format!("{id}-run"));
    trace.terminal_status = status;
    let mut connection = service.state.connection().unwrap();
    conversation_trace_repository::replace_trace(&mut connection, &trace, 2, 3).unwrap();
}

#[test]
fn running_summaries_keep_durable_waits_and_pending_admission_but_exclude_terminal_and_archived() {
    use crate::ConversationTurnTraceTerminalStatus as Status;
    let fixture = StorageFixture::new();
    let service = fixture.service();
    for (id, message_status) in [
        ("summary-queued", "pending"),
        ("summary-running", "pending"),
        ("summary-waiting", "sent"),
        ("summary-completed", "pending"),
        ("summary-failed", "pending"),
        ("summary-cancelled", "pending"),
        ("summary-idle", "sent"),
        ("summary-archived", "pending"),
    ] {
        seed_summary_conversation(&service, id, message_status);
    }
    for (id, status) in [
        ("summary-running", Status::InProgress),
        ("summary-waiting", Status::InProgress),
        ("summary-completed", Status::Completed),
        ("summary-failed", Status::Failed),
        ("summary-cancelled", Status::Cancelled),
    ] {
        commit_summary_trace(&service, id, status);
    }
    service
        .state
        .connection()
        .unwrap()
        .execute(
            "UPDATE conversations SET archived_at = 4 WHERE id = 'summary-archived'",
            [],
        )
        .unwrap();
    let (rows, selects) = trace_storage_selects(&service, || {
        service.load_running_conversation_summaries().unwrap()
    });
    assert_eq!(
        rows.iter().map(|row| row.id.as_str()).collect::<Vec<_>>(),
        ["summary-queued", "summary-running", "summary-waiting"]
    );
    assert_eq!(selects.len(), 1);
    let json = serde_json::to_value(&rows).unwrap();
    assert!(json
        .as_array()
        .unwrap()
        .iter()
        .all(|row| row.as_object().unwrap().len() == 3));
    commit_summary_trace(&service, "summary-running", Status::Completed);
    assert!(!service
        .load_running_conversation_summaries()
        .unwrap()
        .iter()
        .any(|row| row.id == "summary-running"));
}

#[test]
fn running_summaries_filter_child_agents_before_loading_any_details() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    seed_summary_conversation(&service, "summary-root", "pending");
    let mut child = conversation("summary-child", None, "child-user");
    child.messages.clear();
    service.save_conversation(child).unwrap();
    bind_agent_root(&service, "summary-root-agent", "summary-root");
    bind_agent_child(
        &service,
        "summary-child-agent",
        "summary-child",
        "summary-root-agent",
        "summary-root",
        "child",
    );
    service.state.connection().unwrap().execute(
        "INSERT INTO messages (id, conversation_id, role, content, status, created_at, position)
         VALUES ('child-pending', 'summary-child', 'assistant', '', 'pending', 2, 0)", [],
    ).unwrap();
    let rows = service.load_running_conversation_summaries().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, "summary-root");
}

#[test]
fn running_summaries_do_not_read_history_payloads_or_generate_previews() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    seed_summary_conversation(&service, "summary-payload", "pending");
    commit_summary_trace(
        &service,
        "summary-payload",
        crate::ConversationTurnTraceTerminalStatus::InProgress,
    );
    service
        .state
        .connection()
        .unwrap()
        .authorizer(Some(|context: AuthContext<'_>| match context.action {
            AuthAction::Read {
                table_name: "messages",
                column_name: "content" | "agent_run_json" | "folder_references_json",
            } => Authorization::Deny,
            AuthAction::Read {
                table_name:
                    "conversation_turn_trace_items"
                    | "conversation_model_context_items"
                    | "attachments"
                    | "agent_run_guidance",
                ..
            } => Authorization::Deny,
            _ => Authorization::Allow,
        }));
    let rows = service.load_running_conversation_summaries().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, "summary-payload");
}

#[test]
fn running_summaries_scan_only_pending_admission_metadata() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    seed_summary_conversation(&service, "summary-pending-index", "pending");
    let (rows, selects) = trace_storage_selects(&service, || {
        service.load_running_conversation_summaries().unwrap()
    });
    assert_eq!(rows.len(), 1);
    assert_eq!(selects.len(), 1);

    // Check the production SQL instead of a duplicated query. One SELECT can still scan every
    // settled message; this covering partial index bounds the fallback to pending assistants.
    let connection = service.state.connection().unwrap();
    let mut statement = connection
        .prepare(&format!("EXPLAIN QUERY PLAN {}", selects[0]))
        .unwrap();
    let plan = statement
        .query_map([], |row| row.get::<_, String>(3))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert!(
        plan.iter().any(|detail| {
            detail.contains("COVERING INDEX idx_messages_pending_assistant_summary")
        }),
        "pending admission must use the covering partial index: {plan:?}"
    );
    assert!(
        !plan.iter().any(|detail| detail == "SCAN message"),
        "running summaries must not scan settled message history: {plan:?}"
    );
}

#[test]
fn running_summaries_keep_query_count_and_payload_independent_of_settled_history() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    seed_summary_conversation(&service, "summary-active", "pending");
    let expected = service.load_running_conversation_summaries().unwrap();
    for target in [100, 1_000] {
        {
            let mut connection = service.state.connection().unwrap();
            let transaction = connection.transaction().unwrap();
            for index in (if target == 100 { 0 } else { 100 })..target {
                let id = format!("summary-history-{index}");
                transaction.execute(
                    "INSERT INTO conversations (id, title, created_at, updated_at) VALUES (?1, ?1, 1, 1)",
                    [&id],
                ).unwrap();
                transaction.execute(
                    "INSERT INTO messages (id, conversation_id, role, content, status, created_at, position)
                     VALUES (?1, ?1, 'assistant', ?2, 'sent', 1, 0)",
                    rusqlite::params![id, "completed history ".repeat(100)],
                ).unwrap();
            }
            transaction.commit().unwrap();
        }
        let (rows, selects) = trace_storage_selects(&service, || {
            service.load_running_conversation_summaries().unwrap()
        });
        assert_eq!(rows, expected);
        assert_eq!(selects.len(), 1);
        let started = std::time::Instant::now();
        let full = service.load_conversations().unwrap();
        let full_time = started.elapsed();
        let started = std::time::Instant::now();
        let summary = service.load_running_conversation_summaries().unwrap();
        let summary_time = started.elapsed();
        let full_bytes = serde_json::to_vec(&full).unwrap().len();
        let summary_bytes = serde_json::to_vec(&summary).unwrap().len();
        assert!(full_bytes > summary_bytes * 100);
        eprintln!("running summaries: conversations={} full_us={} summary_us={} full_bytes={} summary_bytes={}",
            target + 1, full_time.as_micros(), summary_time.as_micros(), full_bytes, summary_bytes);
    }
}
