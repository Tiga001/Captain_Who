use super::*;
use crate::ConversationTraceRecorder;

fn fixture() -> (
    tempfile::TempDir,
    Box<StorageService>,
    Connection,
    ConversationTraceRecorder,
    ConversationTraceCommitCursor,
) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("trace-external.sqlite");
    // Keep the StorageService at one address so moving this fixture into the test (or the
    // terminal-race thread) does not itself invalidate its process-local database identity.
    let service = Box::new(StorageService::open(&path).unwrap());
    let external = Connection::open(&path).unwrap();
    external.pragma_update(None, "foreign_keys", true).unwrap();
    external.execute_batch(
        "INSERT INTO conversations(id,title,created_at,updated_at) VALUES ('chat','Trace test',1,1);
         INSERT INTO messages(id,conversation_id,role,content,status,created_at,position)
         VALUES ('assistant','chat','assistant','','pending',1,0);",
    ).unwrap();
    let mut recorder = ConversationTraceRecorder::default();
    let mut cursor = ConversationTraceCommitCursor::default();
    recorder.record_narration("first").unwrap();
    append(&service, &mut cursor, &recorder).unwrap();
    (directory, service, external, recorder, cursor)
}

fn append(
    service: &StorageService,
    cursor: &mut ConversationTraceCommitCursor,
    recorder: &ConversationTraceRecorder,
) -> Result<ConversationTraceAppendOutcome, String> {
    service.append_trusted_conversation_trace_publication(
        cursor,
        &recorder.publication(),
        "chat",
        "assistant",
        "run",
        1,
        2,
    )
}

#[test]
fn external_header_change_revokes_cursor_and_rebuilds_the_durable_prefix() {
    let (_directory, service, external, mut recorder, mut cursor) = fixture();
    external.execute(
        "UPDATE conversation_turn_traces SET updated_at=3 WHERE assistant_message_id='assistant'",
        [],
    ).unwrap();
    recorder.record_narration("second").unwrap();
    crate::storage::trace_performance_metrics::start();
    let outcome = append(&service, &mut cursor, &recorder).unwrap();
    let metrics = crate::storage::trace_performance_metrics::finish();
    assert!(outcome.previous_publication.is_none());
    assert_eq!((metrics.trace_loads, metrics.context_loads), (1, 1));

    recorder.record_narration("third").unwrap();
    let outcome = append(&service, &mut cursor, &recorder).unwrap();
    assert!(outcome.previous_publication.is_some());
}

#[test]
fn rolled_back_external_changes_preserve_the_committed_revision() {
    let (_directory, service, external, mut recorder, mut cursor) = fixture();
    external
        .execute_batch(
            "BEGIN IMMEDIATE;
         UPDATE conversation_turn_trace_items
         SET item_json=json_set(item_json,'$.content','uncommitted') WHERE sequence=0;
         ROLLBACK;",
        )
        .unwrap();
    recorder.record_narration("second").unwrap();
    crate::storage::trace_performance_metrics::start();
    let outcome = append(&service, &mut cursor, &recorder).unwrap();
    let metrics = crate::storage::trace_performance_metrics::finish();
    assert!(outcome.previous_publication.is_some());
    assert_eq!((metrics.trace_loads, metrics.context_loads), (0, 0));
}

#[test]
fn external_trace_and_model_corruption_are_not_hidden_by_a_hot_cursor() {
    for mutation in [
        "UPDATE conversation_turn_trace_items SET item_json=json_set(item_json,'$.content','tampered') WHERE sequence=0",
        "UPDATE conversation_model_context_items SET payload=x'00' WHERE sequence=0",
    ] {
        let (_directory, service, external, mut recorder, mut cursor) = fixture();
        external.execute(mutation, []).unwrap();
        recorder.record_narration("second").unwrap();
        assert!(append(&service, &mut cursor, &recorder).is_err());
        assert!(cursor.committed.is_none());
        let count: i64 = external.query_row(
            "SELECT COUNT(*) FROM conversation_turn_trace_items", [], |row| row.get(0),
        ).unwrap();
        assert_eq!(count, 1, "a rejected publication must not leave a suffix");
    }
}

#[test]
fn external_terminal_commit_fences_a_waiting_publication() {
    let (_directory, service, external, mut recorder, mut cursor) = fixture();
    // Use an independent SQLite writer, not the StorageService mutex. The publication must
    // recheck terminal state after acquiring its write transaction behind this settlement.
    external
        .execute_batch(
            "BEGIN IMMEDIATE;
         UPDATE conversation_turn_traces
         SET terminal_status='completed', completed_at=3, updated_at=3
         WHERE assistant_message_id='assistant';",
        )
        .unwrap();
    recorder.record_narration("late").unwrap();
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (finished_tx, finished_rx) = std::sync::mpsc::channel();
    let writer = std::thread::spawn(move || {
        started_tx.send(()).unwrap();
        let result = append(&service, &mut cursor, &recorder);
        finished_tx
            .send((result, cursor.committed.is_none()))
            .unwrap();
    });
    started_rx.recv().unwrap();
    assert!(matches!(
        finished_rx.recv_timeout(std::time::Duration::from_millis(50)),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout)
    ));
    external.execute_batch("COMMIT").unwrap();
    let (result, revoked) = finished_rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    writer.join().unwrap();
    assert!(result.unwrap_err().contains("terminal"));
    assert!(revoked);
    let (status, count): (String, i64) = external
        .query_row(
            "SELECT terminal_status, (SELECT COUNT(*) FROM conversation_turn_trace_items)
         FROM conversation_turn_traces WHERE assistant_message_id='assistant'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(status, "completed");
    assert_eq!(count, 1);
}
