use super::super::common::WAKE_LEASE_DURATION_MS;
use super::*;
use std::cell::Cell;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}
fn disk_fixture(
    remaining_lease_ms: i64,
) -> (tempfile::TempDir, Connection, FinishAgentTurnResultInput) {
    let directory = tempfile::tempdir().unwrap();
    let mut connection = Connection::open(directory.path().join("fixture.sqlite")).unwrap();
    migrations::run_migrations(&connection).unwrap();
    insert_project(&connection, "project-a");
    for conversation in ["conversation-root", "conversation-child"] {
        insert_conversation(&connection, conversation, Some("project-a"));
    }
    ensure_root(&mut connection, "agent-root", "conversation-root");
    create_agent_node(
        &mut connection,
        &child_input(
            "agent-child",
            "agent-root",
            "agent-root",
            "conversation-child",
            "review",
            "/root/review",
        ),
        11,
    )
    .unwrap();
    let task = message_input("fence", "agent-root", "agent-child", AgentMailboxKind::Task);
    enqueue_agent_message(&mut connection, &task, 30).unwrap();
    let mut wake = wake_input("fence");
    wake.source_agent_message_id = Some(task.message_id);
    enqueue_agent_wake(&mut connection, &wake, 30).unwrap();
    let transaction = connection.transaction().unwrap();
    project_agent_wake_source_in_transaction(&transaction, &wake.wake_id, "projection-fence", 30)
        .unwrap();
    transaction.commit().unwrap();
    claim_next_agent_wake(
        &mut connection,
        "agent-child",
        "claim-fence",
        now() - WAKE_LEASE_DURATION_MS + remaining_lease_ms,
    )
    .unwrap()
    .unwrap();
    let input = FinishAgentTurnResultInput {
        wake_id: wake.wake_id,
        expected_status: AgentWakeStatus::Claimed,
        claim_token: "claim-fence".into(),
        terminal_status: AgentWakeStatus::Failed,
        run_id: None,
        assistant_message_id: None,
        terminal_error: Some("synthetic unstarted turn".into()),
    };
    (directory, connection, input)
}

#[test]
fn settlement_clock_expiry_before_commit_rolls_back_result_outbox() {
    let (_directory, mut connection, input) = disk_fixture(WAKE_LEASE_DURATION_MS);
    let lease = get_agent_wake(&connection, &input.wake_id)
        .unwrap()
        .unwrap()
        .lease_expires_at
        .unwrap();
    let ticks = Cell::new(0);
    let result = super::super::settlement::finish_agent_turn_with_result_with_clock(
        &mut connection,
        &input,
        || {
            let tick = ticks.get();
            ticks.set(tick + 1);
            if tick == 0 {
                lease - 1
            } else {
                lease
            }
        },
    );
    assert!(result.is_err());
    let stored = get_agent_wake(&connection, &input.wake_id)
        .unwrap()
        .unwrap();
    assert_eq!(stored.status, AgentWakeStatus::Claimed);
    assert!(stored.result_message_id.is_none());
    let results: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM agent_mailbox_messages WHERE kind = 'result'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(results, 0);
    finish_agent_turn_with_result(&mut connection, &input, lease - 1).unwrap();
    // An immutable committed receipt remains replayable after its execution lease expires.
    super::super::settlement::finish_agent_turn_with_result_with_clock(
        &mut connection,
        &input,
        || lease + 1,
    )
    .unwrap();
}

#[test]
fn settlement_clock_checks_after_external_writer_wait() {
    let (directory, mut connection, input) = disk_fixture(100);
    connection.busy_timeout(Duration::from_secs(2)).unwrap();
    let writer = Connection::open(directory.path().join("fixture.sqlite")).unwrap();
    writer.execute_batch("BEGIN IMMEDIATE").unwrap();
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let waiter = std::thread::spawn(move || {
        started_tx.send(()).unwrap();
        super::super::settlement::finish_agent_turn_with_result_with_clock(
            &mut connection,
            &input,
            now,
        )
    });
    started_rx.recv().unwrap();
    std::thread::sleep(Duration::from_millis(150));
    writer.execute_batch("ROLLBACK").unwrap();
    assert!(waiter.join().unwrap().is_err());
    let results: i64 = writer
        .query_row(
            "SELECT COUNT(*) FROM agent_mailbox_messages WHERE kind = 'result'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(results, 0);
}
