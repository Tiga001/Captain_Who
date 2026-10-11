use super::*;
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;

fn fixture() -> (tempfile::TempDir, Arc<StorageService>) {
    let directory = tempfile::tempdir().unwrap();
    let storage = Arc::new(
        StorageService::open_with_model_credentials(
            &directory.path().join("notification-lease.sqlite"),
            Arc::new(crate::image_generation::InMemoryCredentialStore::default()),
        )
        .unwrap(),
    );
    storage
        .enqueue_notification_event(&NewNotificationEventRecord {
            notification_kind: "task_completed".into(),
            source_kind: "human_root".into(),
            source_id: "synthetic-run".into(),
            run_id: Some("synthetic-run".into()),
            automation_id: None,
            conversation_id: Some("synthetic-conversation".into()),
            user_message_id: Some("synthetic-user".into()),
            assistant_message_id: Some("synthetic-assistant".into()),
            approval_action_id: None,
            subject_kind: "prompt_excerpt".into(),
            subject_text: "Synthetic lease test".into(),
            dedupe_key: "synthetic-completed".into(),
            supersession_key: "synthetic-run".into(),
            resource_revision: Some(1),
            occurred_at: 1_000,
            expires_at: 100_000,
        })
        .unwrap();
    (directory, storage)
}

fn block_writer(storage: &Arc<StorageService>) -> (mpsc::Sender<()>, std::thread::JoinHandle<()>) {
    let storage = storage.clone();
    let (entered, entered_rx) = mpsc::channel();
    let (release, release_rx) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        storage
            .state
            .write_sync(1, StorageWriteClass::Control, move |_| {
                entered.send(()).unwrap();
                release_rx.recv_timeout(Duration::from_secs(10)).unwrap();
            })
            .unwrap();
    });
    entered_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    (release, worker)
}

fn writer_clock(now: Arc<AtomicI64>) -> impl Fn() -> i64 + Send {
    move || {
        // This also deterministically rejects sampling before entry into the writer, even when
        // the calling thread has not reached its FIFO reservation before the barrier releases.
        assert_eq!(std::thread::current().name(), Some("core-sqlite-writer"));
        now.load(Ordering::SeqCst)
    }
}

#[test]
fn notification_claim_lease_starts_after_the_writer_barrier() {
    let (_directory, storage) = fixture();
    let now = Arc::new(AtomicI64::new(1_000));
    let clock = writer_clock(now.clone());
    let (release, blocker) = block_writer(&storage);
    let (started, started_rx) = mpsc::channel();
    let claiming = std::thread::spawn(move || {
        started.send(()).unwrap();
        storage
            .claim_pending_notification_batches_with_clock("claim", 5_000, 10, clock)
            .unwrap()
    });
    started_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    now.store(10_000, Ordering::SeqCst);
    release.send(()).unwrap();
    blocker.join().unwrap();
    let records = claiming.join().unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].claim_expires_at, Some(15_000));
}

#[test]
fn notification_validate_rejects_a_lease_expired_behind_the_writer_barrier() {
    let (_directory, storage) = fixture();
    let record = storage
        .claim_pending_notification_batches_with_clock("claim", 5_000, 10, || 1_000)
        .unwrap()
        .remove(0);
    let now = Arc::new(AtomicI64::new(2_000));
    let clock = writer_clock(now.clone());
    let (release, blocker) = block_writer(&storage);
    let (started, started_rx) = mpsc::channel();
    let validating = std::thread::spawn(move || {
        started.send(()).unwrap();
        storage
            .validate_claimed_notification_batch_with_clock(&record.id, "claim", clock)
            .unwrap()
    });
    started_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    now.store(6_000, Ordering::SeqCst);
    release.send(()).unwrap();
    blocker.join().unwrap();
    assert!(validating.join().unwrap().is_none());
}

fn advancing_clock(first: i64, last: i64) -> impl Fn() -> i64 + Send {
    let calls = AtomicUsize::new(0);
    move || {
        if calls.fetch_add(1, Ordering::SeqCst) == 0 {
            first
        } else {
            last
        }
    }
}

#[test]
fn notification_claim_expiring_in_its_transaction_rolls_back_the_claim() {
    let (_directory, storage) = fixture();
    assert!(storage
        .claim_pending_notification_batches_with_clock(
            "expired",
            5_000,
            10,
            advancing_clock(1_000, 6_000),
        )
        .unwrap()
        .is_empty());
    let record = storage
        .claim_pending_notification_batches_with_clock("successor", 5_000, 10, || 6_000)
        .unwrap()
        .remove(0);
    assert_eq!(record.attempt_count, 1);
    assert_eq!(record.claim_token.as_deref(), Some("successor"));
}

#[test]
fn notification_claim_expiring_during_commit_does_not_return_delivery_authority() {
    let (_directory, storage) = fixture();
    let now = Arc::new(AtomicI64::new(1_000));
    let committing_at = now.clone();
    storage
        .state
        .write_sync(1, StorageWriteClass::Control, move |connection| {
            connection
                .commit_hook(Some(move || {
                    committing_at.store(6_000, Ordering::SeqCst);
                    false
                }))
                .unwrap();
        })
        .unwrap();
    let records = storage
        .claim_pending_notification_batches_with_clock(
            "expired-at-commit",
            5_000,
            10,
            writer_clock(now),
        )
        .unwrap();
    assert!(records.is_empty());
    // The commit is a durable fact; do not manufacture a rollback after it succeeded. A fresh
    // successor can reclaim the expired batch once, without extending the expired token.
    let connection = storage.state.read_connection().unwrap();
    let claim: (String, i64, i64) = connection
        .query_row(
            "SELECT claim_token, claim_expires_at, attempt_count FROM notification_batches",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(claim, ("expired-at-commit".into(), 6_000, 1));
    drop(connection);
    let successor = storage
        .claim_pending_notification_batches_with_clock("successor", 5_000, 10, || 6_000)
        .unwrap()
        .remove(0);
    assert_eq!(successor.claim_token.as_deref(), Some("successor"));
    assert_eq!(successor.claim_expires_at, Some(11_000));
    assert_eq!(successor.attempt_count, 2);
}

#[test]
fn notification_replayed_claim_and_validation_recheck_expiry_before_returning() {
    let (_directory, storage) = fixture();
    let record = storage
        .claim_pending_notification_batches_with_clock("claim", 5_000, 10, || 1_000)
        .unwrap()
        .remove(0);
    assert!(storage
        .claim_pending_notification_batches_with_clock(
            "claim",
            5_000,
            10,
            advancing_clock(2_000, 6_000),
        )
        .unwrap()
        .is_empty());
    assert!(storage
        .validate_claimed_notification_batch_with_clock(
            &record.id,
            "claim",
            advancing_clock(2_000, 6_000),
        )
        .unwrap()
        .is_none());
    // Final checks neither extend the original lease nor consume a new delivery attempt.
    let connection = storage.state.read_connection().unwrap();
    let lease: (i64, i64) = connection
        .query_row(
            "SELECT claim_expires_at, attempt_count FROM notification_batches WHERE id=?1",
            [&record.id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(lease, (6_000, 1));
}

#[test]
fn notification_late_delivery_ack_preserves_the_real_receipt() {
    let (_directory, storage) = fixture();
    let record = storage
        .claim_pending_notification_batches_with_clock("claim", 5_000, 10, || 1_000)
        .unwrap()
        .remove(0);
    assert!(storage
        .validate_claimed_notification_batch_with_clock(&record.id, "claim", || 6_000)
        .unwrap()
        .is_none());
    let delivered = storage
        .acknowledge_notification_batch(
            &record.id,
            "claim",
            "delivered",
            "completed",
            "initial",
            record.revision,
            6_001,
        )
        .unwrap()
        .unwrap();
    assert_eq!(delivered.delivered_revision, Some(record.revision));
    assert_eq!(delivered.disposition.as_deref(), Some("delivered"));
    assert!(delivered.claim_token.is_none());
}

#[test]
fn notification_successor_fences_late_delivery_ack_and_release() {
    let (_directory, storage) = fixture();
    let record = storage
        .claim_pending_notification_batches_with_clock("old", 5_000, 10, || 1_000)
        .unwrap()
        .remove(0);
    let successor = storage
        .claim_pending_notification_batches_with_clock("new", 5_000, 10, || 6_000)
        .unwrap()
        .remove(0);
    assert!(storage
        .acknowledge_notification_batch(
            &record.id,
            "old",
            "delivered",
            "completed",
            "initial",
            record.revision,
            6_001,
        )
        .unwrap()
        .is_none());
    assert!(storage
        .release_notification_batch(&record.id, "old", 7_000, "synthetic_error")
        .unwrap()
        .is_none());
    let current = storage
        .validate_claimed_notification_batch_with_clock(&record.id, "new", || 6_002)
        .unwrap()
        .unwrap();
    assert_eq!(current.claim_expires_at, successor.claim_expires_at);
    assert_eq!(current.attempt_count, 2);
    assert_eq!(current.claim_token.as_deref(), Some("new"));
    assert_eq!(current.delivered_revision, None);
}
