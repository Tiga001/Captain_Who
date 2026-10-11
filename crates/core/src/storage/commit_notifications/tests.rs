use super::*;
use rusqlite::OpenFlags;
use std::sync::Barrier;
use std::time::Duration;

const SCHEMA: &str = "
    PRAGMA journal_mode = WAL;
    PRAGMA synchronous = FULL;
    PRAGMA foreign_keys = ON;
    CREATE TABLE parents(id INTEGER PRIMARY KEY);
    CREATE TABLE agent_collaboration_events(
        global_sequence INTEGER PRIMARY KEY AUTOINCREMENT, payload TEXT NOT NULL);
    CREATE TABLE automation_events(
        sequence INTEGER PRIMARY KEY AUTOINCREMENT, payload TEXT NOT NULL);
    CREATE TABLE notification_change_events(
        sequence INTEGER PRIMARY KEY AUTOINCREMENT, payload TEXT NOT NULL,
        parent_id INTEGER REFERENCES parents(id) DEFERRABLE INITIALLY DEFERRED);
";

struct Fixture {
    writer: Connection,
    reader: Connection,
    notifications: StorageEventNotifications,
    commits: CommitNotifications,
    directory: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("notifications.sqlite");
        let writer = Connection::open(&path).unwrap();
        writer.execute_batch(SCHEMA).unwrap();
        let reader = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        reader.pragma_update(None, "query_only", true).unwrap();
        let notifications = StorageEventNotifications::default();
        let commits = CommitNotifications::install(&writer, notifications.clone()).unwrap();
        Self {
            writer,
            reader,
            notifications,
            commits,
            directory,
        }
    }

    fn collaboration_count(&self) -> i64 {
        self.reader
            .query_row(
                "SELECT COUNT(*) FROM agent_collaboration_events",
                [],
                |row| row.get(0),
            )
            .unwrap()
    }

    fn publish(&self) {
        self.commits.publish_committed(&self.writer).unwrap();
    }
}

#[test]
fn explicit_commit_wakes_only_after_independent_connection_can_read() {
    let mut fixture = Fixture::new();
    let mut changed = fixture
        .notifications
        .subscribe(StorageEventStream::AgentCollaboration);
    let transaction = fixture.writer.transaction().unwrap();
    transaction
        .execute(
            "INSERT INTO agent_collaboration_events(payload) VALUES ('committed')",
            [],
        )
        .unwrap();
    fixture.commits.publish_committed(&transaction).unwrap();
    assert!(!changed.has_changed().unwrap());
    assert_eq!(
        fixture
            .reader
            .query_row(
                "SELECT COUNT(*) FROM agent_collaboration_events",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    transaction.commit().unwrap();
    assert!(
        !changed.has_changed().unwrap(),
        "no SQLite hook publishes directly"
    );
    fixture.publish();
    assert!(changed.has_changed().unwrap());
    assert_eq!(*changed.borrow_and_update(), 1);
    assert_eq!(fixture.collaboration_count(), 1);
    fixture.publish();
    assert!(
        !changed.has_changed().unwrap(),
        "one wake for one completed boundary"
    );
}

#[test]
fn autocommit_and_multiple_commits_coalesce_without_losing_committed_events() {
    let fixture = Fixture::new();
    let mut changed = fixture
        .notifications
        .subscribe(StorageEventStream::Automation);
    fixture
        .writer
        .execute_batch(
            "INSERT INTO automation_events(payload) VALUES ('one');
         INSERT INTO automation_events(payload) VALUES ('two');
         BEGIN;
         INSERT INTO automation_events(payload) VALUES ('rollback');
         ROLLBACK;",
        )
        .unwrap();
    assert!(!changed.has_changed().unwrap());
    fixture.publish();
    assert_eq!(*changed.borrow_and_update(), 1);
    assert_eq!(
        fixture
            .reader
            .query_row("SELECT COUNT(*) FROM automation_events", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        2
    );
    fixture
        .writer
        .execute(
            "INSERT INTO automation_events(payload) VALUES ('three')",
            [],
        )
        .unwrap();
    fixture.publish();
    assert_eq!(*changed.borrow_and_update(), 2);
}

#[test]
fn rollback_and_transaction_drop_do_not_publish() {
    let mut fixture = Fixture::new();
    let changed = fixture
        .notifications
        .subscribe(StorageEventStream::AgentCollaboration);
    for explicitly_rollback in [false, true] {
        let transaction = fixture.writer.transaction().unwrap();
        transaction
            .execute(
                "INSERT INTO agent_collaboration_events(payload) VALUES ('discard')",
                [],
            )
            .unwrap();
        if explicitly_rollback {
            transaction.rollback().unwrap();
        } else {
            drop(transaction);
        }
        fixture.publish();
        assert!(!changed.has_changed().unwrap());
        assert_eq!(fixture.collaboration_count(), 0);
    }
}

#[test]
fn rolled_back_savepoint_cannot_notify_a_stream_committed_in_another_savepoint() {
    let fixture = Fixture::new();
    let collaboration = fixture
        .notifications
        .subscribe(StorageEventStream::AgentCollaboration);
    let automation = fixture
        .notifications
        .subscribe(StorageEventStream::Automation);
    fixture
        .writer
        .execute_batch(
            "BEGIN;
         SAVEPOINT discarded;
         INSERT INTO agent_collaboration_events(payload) VALUES ('discarded');
         ROLLBACK TO discarded;
         RELEASE discarded;
         SAVEPOINT kept;
         INSERT INTO automation_events(payload) VALUES ('kept');
         RELEASE kept;",
        )
        .unwrap();
    fixture.publish();
    assert!(!collaboration.has_changed().unwrap());
    assert!(!automation.has_changed().unwrap());
    fixture.writer.execute_batch("COMMIT").unwrap();
    fixture.publish();
    assert!(!collaboration.has_changed().unwrap());
    assert!(automation.has_changed().unwrap());
    assert_eq!(fixture.collaboration_count(), 0);
}

#[test]
fn outermost_savepoint_release_is_a_successful_commit_boundary() {
    let fixture = Fixture::new();
    let changed = fixture
        .notifications
        .subscribe(StorageEventStream::AgentCollaboration);
    fixture
        .writer
        .execute_batch(
            "SAVEPOINT outermost;
         INSERT INTO agent_collaboration_events(payload) VALUES ('savepoint');",
        )
        .unwrap();
    fixture.publish();
    assert!(!changed.has_changed().unwrap());
    fixture.writer.execute_batch("RELEASE outermost").unwrap();
    fixture.publish();
    assert!(changed.has_changed().unwrap());
    assert_eq!(fixture.collaboration_count(), 1);
}

#[test]
fn deferred_constraint_commit_failure_remains_unpublished_until_rollback() {
    let fixture = Fixture::new();
    let changed = fixture
        .notifications
        .subscribe(StorageEventStream::Notification);
    fixture
        .writer
        .execute_batch(
            "BEGIN;
         INSERT INTO notification_change_events(payload, parent_id) VALUES ('invalid', 42);",
        )
        .unwrap();
    assert!(fixture.writer.execute_batch("COMMIT").is_err());
    assert!(
        !fixture.writer.is_autocommit(),
        "failed COMMIT can leave an open transaction"
    );
    fixture.publish();
    assert!(!changed.has_changed().unwrap());
    fixture.writer.execute_batch("ROLLBACK").unwrap();
    fixture.publish();
    assert!(!changed.has_changed().unwrap());
    assert_eq!(
        fixture
            .reader
            .query_row(
                "SELECT COUNT(*) FROM notification_change_events",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    fixture
        .writer
        .execute(
            "INSERT INTO notification_change_events(payload) VALUES ('valid')",
            [],
        )
        .unwrap();
    fixture.publish();
    assert!(
        changed.has_changed().unwrap(),
        "rolled-back rowid reuse must still notify"
    );
}

#[test]
fn deferred_constraint_commit_failure_can_retry_without_premature_notification() {
    let fixture = Fixture::new();
    let changed = fixture
        .notifications
        .subscribe(StorageEventStream::Notification);
    fixture
        .writer
        .execute_batch(
            "BEGIN;
         INSERT INTO notification_change_events(payload, parent_id) VALUES ('pending', 42);",
        )
        .unwrap();
    assert!(fixture.writer.execute_batch("COMMIT").is_err());
    fixture.publish();
    assert!(!changed.has_changed().unwrap());
    fixture
        .writer
        .execute_batch("INSERT INTO parents(id) VALUES (42); COMMIT;")
        .unwrap();
    fixture.publish();
    assert!(changed.has_changed().unwrap());
    assert_eq!(
        fixture
            .reader
            .query_row(
                "SELECT COUNT(*) FROM notification_change_events",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
}

#[test]
fn accepting_commit_hook_is_not_the_notification_boundary() {
    let fixture = Fixture::new();
    let changed = fixture
        .notifications
        .subscribe(StorageEventStream::AgentCollaboration);
    let hook_observed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let observed = Arc::clone(&hook_observed);
    let during_commit = changed.clone();
    fixture
        .writer
        .commit_hook(Some(move || {
            assert!(!during_commit.has_changed().unwrap());
            observed.store(true, Ordering::Relaxed);
            false
        }))
        .unwrap();
    fixture
        .writer
        .execute(
            "INSERT INTO agent_collaboration_events(payload) VALUES ('accepted')",
            [],
        )
        .unwrap();
    assert!(hook_observed.load(Ordering::Relaxed));
    assert!(!changed.has_changed().unwrap());
    fixture.publish();
    assert!(changed.has_changed().unwrap());
}

#[test]
fn commit_hook_veto_does_not_publish_rolled_back_insert() {
    let fixture = Fixture::new();
    let changed = fixture
        .notifications
        .subscribe(StorageEventStream::AgentCollaboration);
    fixture.writer.commit_hook(Some(|| true)).unwrap();
    assert!(fixture
        .writer
        .execute(
            "INSERT INTO agent_collaboration_events(payload) VALUES ('veto')",
            []
        )
        .is_err());
    assert!(fixture.writer.is_autocommit());
    fixture.publish();
    assert!(!changed.has_changed().unwrap());
    assert_eq!(fixture.collaboration_count(), 0);
}

#[test]
fn installation_uses_existing_committed_head_without_republishing_history() {
    let fixture = Fixture::new();
    fixture
        .writer
        .execute(
            "INSERT INTO automation_events(payload) VALUES ('existing')",
            [],
        )
        .unwrap();
    let notifications = StorageEventNotifications::default();
    let changed = notifications.subscribe(StorageEventStream::Automation);
    let commits = CommitNotifications::install(&fixture.writer, notifications).unwrap();
    commits.publish_committed(&fixture.writer).unwrap();
    assert!(!changed.has_changed().unwrap());
    fixture
        .writer
        .execute_batch(
            "BEGIN; INSERT INTO automation_events(payload) VALUES ('discarded'); ROLLBACK;",
        )
        .unwrap();
    commits.publish_committed(&fixture.writer).unwrap();
    assert!(!changed.has_changed().unwrap());
    fixture
        .writer
        .execute("INSERT INTO automation_events(payload) VALUES ('new')", [])
        .unwrap();
    commits.publish_committed(&fixture.writer).unwrap();
    assert!(changed.has_changed().unwrap());
}

#[test]
fn failed_journal_verification_retains_the_wakeup_for_a_later_attempt() {
    let fixture = Fixture::new();
    let changed = fixture
        .notifications
        .subscribe(StorageEventStream::Automation);
    fixture
        .writer
        .execute(
            "INSERT INTO automation_events(payload) VALUES ('committed')",
            [],
        )
        .unwrap();
    fixture
        .writer
        .execute_batch("ALTER TABLE automation_events RENAME TO unavailable_events")
        .unwrap();
    assert!(fixture.commits.publish_committed(&fixture.writer).is_err());
    assert!(!changed.has_changed().unwrap());
    fixture
        .writer
        .execute_batch("ALTER TABLE unavailable_events RENAME TO automation_events")
        .unwrap();
    fixture.publish();
    assert!(changed.has_changed().unwrap());
}

#[test]
fn old_read_snapshot_must_end_before_observing_a_new_committed_notification() {
    let fixture = Fixture::new();
    let changed = fixture
        .notifications
        .subscribe(StorageEventStream::AgentCollaboration);
    fixture.reader.execute_batch("BEGIN").unwrap();
    assert_eq!(fixture.collaboration_count(), 0);
    fixture
        .writer
        .execute(
            "INSERT INTO agent_collaboration_events(payload) VALUES ('new')",
            [],
        )
        .unwrap();
    fixture.publish();
    assert!(changed.has_changed().unwrap());
    assert_eq!(
        fixture.collaboration_count(),
        0,
        "WAL read transactions keep their original snapshot"
    );
    fixture.reader.execute_batch("COMMIT").unwrap();
    assert_eq!(
        fixture.collaboration_count(),
        1,
        "fresh snapshots read committed state"
    );
}

#[tokio::test]
async fn commit_racing_with_subscriber_catchup_does_not_lose_durable_events() {
    let fixture = Fixture::new();
    let Fixture {
        writer,
        reader,
        notifications,
        commits,
        directory,
    } = fixture;
    let mut changed = notifications.subscribe(StorageEventStream::AgentCollaboration);
    let start = Arc::new(Barrier::new(2));
    let writer_start = Arc::clone(&start);
    let producer = std::thread::spawn(move || {
        writer_start.wait();
        for index in 0..128 {
            writer
                .execute(
                    "INSERT INTO agent_collaboration_events(payload) VALUES (?1)",
                    [format!("event-{index}")],
                )
                .unwrap();
            commits.publish_committed(&writer).unwrap();
        }
    });
    start.wait();
    let mut cursor = 0;
    let mut delivered = Vec::new();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            // Acknowledge the hint before catch-up. Commits that race with this query leave the
            // receiver changed, so the next iteration cannot sleep past the new durable cursor.
            changed.borrow_and_update();
            let events = reader.prepare("SELECT global_sequence FROM agent_collaboration_events WHERE global_sequence > ?1 ORDER BY global_sequence LIMIT 17").unwrap()
                .query_map([cursor], |row| row.get::<_, i64>(0)).unwrap()
                .collect::<rusqlite::Result<Vec<_>>>().unwrap();
            let full_page = events.len() == 17;
            if let Some(last) = events.last() { cursor = *last; }
            delivered.extend(events);
            if cursor == 128 { break; }
            // Continue immediately while a bounded page may have more durable entries. Watch
            // values are wake hints, never substituted for the durable event cursor.
            if full_page { continue; }
            changed.changed().await.unwrap();
        }
    }).await.unwrap();
    producer.join().unwrap();
    assert_eq!(delivered, (1..=128).collect::<Vec<_>>());
    drop(reader);
    drop(directory);
}
