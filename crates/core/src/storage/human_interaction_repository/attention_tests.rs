use super::*;
use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
use std::cell::RefCell;

thread_local! { static STATEMENTS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) }; }

fn approval(connection: &Connection, id: &str, conversation: &str, status: &str) {
    connection.execute("INSERT INTO agent_pending_actions(action_id,run_id,conversation_id,assistant_message_id,action_type,tool_name,status,action_json,agent_input_json,created_at,updated_at) VALUES(?1,'run',?2,'assistant','tool','test',?3,'{}','{}',1,1)", params![id, conversation, status]).unwrap();
}

#[test]
fn attention_is_sparse_metadata_only_and_uses_three_reads_for_a_thousand_chats() {
    let fixture = Fixture::new();
    let request = fixture.request(HumanInteractionMode::Async, "async");
    let mut connection = fixture.connect();
    let tx = connection.transaction().unwrap();
    for index in 0..1000 {
        insert_conversation(&tx, &format!("empty-{index}"));
    }
    tx.commit().unwrap();
    approval(&connection, "pending-1", "chat", "pending");
    approval(&connection, "pending-2", "chat", "pending");
    approval(&connection, "approved", "empty-1", "approved");
    connection.authorizer(Some(|context: AuthContext<'_>| match context.action {
        AuthAction::Read {
            table_name:
                "messages"
                | "conversation_turn_trace_items"
                | "conversation_model_context_items"
                | "human_interaction_responses"
                | "human_interaction_deliveries",
            ..
        } => Authorization::Deny,
        AuthAction::Read {
            column_name: "questions_json" | "action_json" | "agent_input_json",
            ..
        } => Authorization::Deny,
        _ => Authorization::Allow,
    }));
    STATEMENTS.with(|statements| statements.borrow_mut().clear());
    connection.trace(Some(|sql| {
        STATEMENTS.with(|statements| statements.borrow_mut().push(sql.to_owned()))
    }));
    let start = std::time::Instant::now();
    let snapshot = load_attention(&mut connection).unwrap();
    let elapsed = start.elapsed();
    assert_eq!(snapshot.requests.len(), 1);
    assert_eq!(snapshot.requests[0].request_id, request.request_id);
    assert_eq!(snapshot.approval_conversation_ids, ["chat"]);
    let count = STATEMENTS.with(|statements| {
        statements
            .borrow()
            .iter()
            .filter(|sql| sql.starts_with("SELECT"))
            .count()
    });
    assert_eq!(count, 3);
    let bytes = serde_json::to_vec(&snapshot).unwrap().len();
    assert!(bytes < 300);
    eprintln!("attention: 1001 conversations, 1 pending question, 1 pending approval owner: {count} SELECTs, {bytes} response bytes, {elapsed:?}");
}

#[test]
fn attention_reconciles_submit_ignore_cancel_archive_and_deletion_without_losing_watermark() {
    let fixture = Fixture::new();
    let submitted_request = fixture.request(HumanInteractionMode::Async, "submitted");
    let ignored_request = fixture.request(HumanInteractionMode::Async, "ignored");
    let cancelled = fixture.request(HumanInteractionMode::Sync, "cancelled");
    let remains_open = fixture.request(HumanInteractionMode::Async, "remains-open");
    let mut connection = fixture.connect();
    submit(
        &mut connection,
        &submission(&submitted_request, "submission"),
        20,
    )
    .unwrap();
    ignore(&mut connection, &ignored(&ignored_request, "ignored"), 20).unwrap();
    connection.execute("UPDATE human_interaction_requests SET status='cancelled',revision=revision+1 WHERE request_id=?1", [&cancelled.request_id]).unwrap();
    connection.execute("UPDATE conversation_turn_traces SET terminal_status='completed',completed_at=20 WHERE assistant_message_id='assistant'", []).unwrap();
    let snapshot = load_attention(&mut connection).unwrap();
    assert_eq!(snapshot.requests.len(), 1);
    assert_eq!(
        snapshot.requests[0].request_id, remains_open.request_id,
        "async questions survive run completion"
    );
    approval(&connection, "pending", "chat", "pending");
    connection
        .execute(
            "UPDATE conversations SET archived_at=30 WHERE id='chat'",
            [],
        )
        .unwrap();
    let archived = load_attention(&mut connection).unwrap();
    assert!(archived.requests.is_empty() && archived.approval_conversation_ids.is_empty());
    assert_eq!(archived.request_sequence, remains_open.sequence);
    connection
        .execute(
            "UPDATE conversations SET archived_at=NULL WHERE id='chat'",
            [],
        )
        .unwrap();
    assert_eq!(load_attention(&mut connection).unwrap().requests.len(), 1);
    connection
        .execute("DELETE FROM human_interaction_requests", [])
        .unwrap();
    assert_eq!(
        load_attention(&mut connection).unwrap().request_sequence,
        remains_open.sequence
    );
    assert!(load_attention(&mut connection).unwrap().requests.is_empty());
}

#[test]
fn attention_query_plans_use_pending_indexes() {
    let fixture = Fixture::new();
    let connection = fixture.connect();
    for (sql, index) in [
        (
            super::super::attention::OPEN_REQUESTS,
            "idx_human_interaction_requests_attention",
        ),
        (
            super::super::attention::OPEN_APPROVALS,
            "idx_agent_pending_actions_status",
        ),
    ] {
        let plan = connection
            .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
            .unwrap()
            .query_map([], |row| row.get::<_, String>(3))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
            .join("\n");
        assert!(plan.contains(index), "{plan}");
        eprintln!("{index}:\n{plan}");
    }
}

#[test]
fn attention_empty_conversation_benchmark() {
    if std::env::var_os("MYCOPILOT_ATTENTION_BENCH").is_none() {
        return;
    }
    let fixture = Fixture::new();
    let mut connection = fixture.connect();
    let tx = connection.transaction().unwrap();
    for index in 0..1000 {
        insert_conversation(&tx, &format!("empty-{index}"));
    }
    tx.commit().unwrap();
    let mut baseline = Vec::new();
    let mut optimized = Vec::new();
    let mut old_bytes = 0;
    let mut new_bytes = 0;
    for _ in 0..5 {
        old_bytes = 0;
        let started = std::time::Instant::now();
        for index in 0..1000 {
            let page = list_requests(
                &mut connection,
                &HumanInteractionListInput {
                    conversation_id: format!("empty-{index}"),
                    cursor: None,
                    limit: 100,
                },
            )
            .unwrap();
            old_bytes += serde_json::to_vec(&page).unwrap().len();
        }
        baseline.push(started.elapsed());
        let started = std::time::Instant::now();
        let snapshot = load_attention(&mut connection).unwrap();
        new_bytes = serde_json::to_vec(&snapshot).unwrap().len();
        assert!(snapshot.requests.is_empty() && snapshot.approval_conversation_ids.is_empty());
        optimized.push(started.elapsed());
    }
    baseline.sort();
    optimized.sort();
    eprintln!("attention empty-1000 benchmark (5 runs median, repository+JSON only): before 1000 calls, {old_bytes} response bytes, {:?}; after 1 call/3 SELECTs, {new_bytes} response bytes, {:?}", baseline[2], optimized[2]);
}
