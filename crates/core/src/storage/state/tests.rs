use super::*;
use std::sync::mpsc;

struct Fixture {
    state: Arc<StorageState>,
    path: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("captain-storage-round3-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        let state = Arc::new(StorageState::open(&path.join("fixture.sqlite")).unwrap());
        state
            .write_sync(1024, StorageWriteClass::Normal, |c| {
                c.execute_batch(
                    "CREATE TABLE fixture(value INTEGER); INSERT INTO fixture VALUES(1);",
                )
            })
            .unwrap()
            .unwrap();
        Self { state, path }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[test]
fn actual_linked_sqlite_and_disk_connection_contract() {
    let fixture = Fixture::new();
    assert_eq!(rusqlite::version(), "3.53.2");
    assert_eq!(rusqlite::version_number(), 3_053_002);
    let writer = fixture.state.connection().unwrap();
    let mode: String = writer
        .query_row("PRAGMA journal_mode", [], |r| r.get(0))
        .unwrap();
    assert_eq!(mode, "wal");
    assert_eq!(
        writer
            .query_row("PRAGMA synchronous", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        2
    );
    assert_eq!(
        writer
            .query_row("PRAGMA busy_timeout", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        5000
    );
    drop(writer);
    let reader = fixture.state.read_connection().unwrap();
    assert!(reader.execute("INSERT INTO fixture VALUES(2)", []).is_err());
    assert_eq!(
        reader
            .query_row("PRAGMA query_only", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
}

#[test]
fn read_snapshot_does_not_block_writer_and_refresh_sees_committed_data() {
    let fixture = Fixture::new();
    let mut reader = fixture.state.read_connection().unwrap();
    let transaction = reader.transaction().unwrap();
    assert_eq!(
        transaction
            .query_row("SELECT value FROM fixture", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    let state = fixture.state.clone();
    std::thread::spawn(move || {
        state
            .write_sync(1024, StorageWriteClass::Normal, |c| {
                c.execute("UPDATE fixture SET value=2", [])
            })
            .unwrap()
            .unwrap()
    })
    .join()
    .unwrap();
    assert_eq!(
        transaction
            .query_row("SELECT value FROM fixture", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    transaction.commit().unwrap();
    assert_eq!(
        reader
            .query_row("SELECT value FROM fixture", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        2
    );
}

#[test]
fn nested_connection_waits_are_rejected_before_deadlock() {
    let fixture = Fixture::new();
    let read = fixture.state.read_connection().unwrap();
    assert!(fixture
        .state
        .write_sync(1, StorageWriteClass::Control, |_| ())
        .is_err());
    drop(read);
    fixture
        .state
        .write_sync(1, StorageWriteClass::Normal, |_| {
            assert!(fixture.state.connection().is_err());
            assert!(fixture.state.read_connection().is_err());
            assert!(fixture
                .state
                .write_sync(1, StorageWriteClass::Normal, |_| ())
                .is_err());
        })
        .unwrap();
}

#[test]
fn failed_and_unfinished_transactions_cannot_leak_to_next_owner() {
    let fixture = Fixture::new();
    assert!(fixture
        .state
        .write_sync(1, StorageWriteClass::Normal, |c| c
            .execute_batch("BEGIN; UPDATE fixture SET value=9;"))
        .is_err());
    let value = fixture
        .state
        .read_connection()
        .unwrap()
        .query_row("SELECT value FROM fixture", [], |r| r.get::<_, i64>(0))
        .unwrap();
    assert_eq!(value, 1);
    let result = fixture
        .state
        .write_sync(1, StorageWriteClass::Normal, |c| {
            let transaction = c.transaction().unwrap();
            transaction
                .execute("UPDATE fixture SET value=3", [])
                .unwrap();
            transaction.execute("INSERT INTO missing_table VALUES(1)", [])?;
            transaction.commit()
        })
        .unwrap();
    assert!(result.is_err());
    assert_eq!(
        fixture
            .state
            .read_connection()
            .unwrap()
            .query_row("SELECT value FROM fixture", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
}

#[tokio::test]
async fn cancelled_queued_job_never_starts_but_started_job_is_drained() {
    let fixture = Fixture::new();
    let (started_send, started_receive) = tokio::sync::oneshot::channel();
    let (release_send, release_receive) = mpsc::channel();
    let started = tokio::spawn(fixture.state.write_async(
        1024,
        StorageWriteClass::Normal,
        move |c| {
            let transaction = c.transaction().unwrap();
            transaction
                .execute("UPDATE fixture SET value=4", [])
                .unwrap();
            let _ = started_send.send(());
            release_receive.recv().unwrap();
            transaction.commit().unwrap();
        },
    ));
    started_receive.await.unwrap();
    let queued = tokio::spawn(
        fixture
            .state
            .write_async(1024, StorageWriteClass::Normal, |c| {
                c.execute("UPDATE fixture SET value=99", [])
            }),
    );
    tokio::task::yield_now().await;
    queued.abort();
    let _ = queued.await;
    started.abort();
    let _ = started.await;
    let state = fixture.state.clone();
    let closing = tokio::spawn(async move { state.shutdown().await });
    tokio::task::yield_now().await;
    assert!(!closing.is_finished());
    assert!(fixture
        .state
        .write_async(1, StorageWriteClass::Control, |_| ())
        .await
        .is_err());
    release_send.send(()).unwrap();
    closing.await.unwrap().unwrap();
    let reopened = Connection::open(fixture.path.join("fixture.sqlite")).unwrap();
    assert_eq!(
        reopened
            .query_row("SELECT value FROM fixture", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        4
    );
}

#[tokio::test]
async fn pinned_snapshot_checkpoint_and_backup_include_wal_commits() {
    let fixture = Fixture::new();
    // Keep the reader on a separate thread so no test holds a read lease across an async write.
    let (ready_send, ready_receive) = tokio::sync::oneshot::channel();
    let (release_send, release_receive) = mpsc::channel();
    let state = fixture.state.clone();
    let reading = std::thread::spawn(move || {
        let mut reader = state.read_connection().unwrap();
        let snapshot = reader.transaction().unwrap();
        assert_eq!(
            snapshot
                .query_row("SELECT value FROM fixture", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
        ready_send.send(()).unwrap();
        release_receive.recv().unwrap();
        snapshot.commit().unwrap();
    });
    ready_receive.await.unwrap();
    fixture
        .state
        .write_async(1, StorageWriteClass::Normal, |c| {
            c.execute("UPDATE fixture SET value=8", [])
        })
        .await
        .unwrap()
        .unwrap();
    let pinned = fixture.state.checkpoint().await.unwrap();
    assert!(pinned.wal_bytes > 0);
    assert!(pinned.log_frames > pinned.checkpointed_frames);
    let backup = fixture.path.join("backup.sqlite");
    std::fs::File::create(&backup).unwrap();
    super::super::create_verified_sqlite_snapshot(&fixture.path.join("fixture.sqlite"), &backup)
        .unwrap();
    let restored = Connection::open(backup).unwrap();
    assert_eq!(
        restored
            .query_row("SELECT value FROM fixture", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        8
    );
    drop(restored);
    release_send.send(()).unwrap();
    reading.join().unwrap();
    let progress = fixture.state.checkpoint().await.unwrap();
    assert_eq!(progress.log_frames, progress.checkpointed_frames);
    assert_eq!(fixture.state.shutdown().await.unwrap().wal_bytes, 0);
}

#[test]
fn writer_runs_on_its_only_named_thread_and_preserves_fifo() {
    let fixture = Fixture::new();
    fixture
        .state
        .write_sync(1, StorageWriteClass::Normal, |_| {
            assert_eq!(std::thread::current().name(), Some("core-sqlite-writer"))
        })
        .unwrap();
    let first = fixture
        .state
        .writer
        .gate
        .reserve(1, StorageWriteClass::Normal)
        .unwrap();
    let second = fixture
        .state
        .writer
        .gate
        .reserve(1, StorageWriteClass::Control)
        .unwrap();
    let (send, receive) = mpsc::channel();
    let waiter = std::thread::spawn(move || {
        second.wait();
        send.send(()).unwrap();
    });
    assert!(receive.recv_timeout(Duration::from_millis(30)).is_err());
    drop(first);
    receive.recv_timeout(Duration::from_secs(2)).unwrap();
    waiter.join().unwrap();
}

#[test]
fn count_and_bytes_reserve_control_capacity_and_release_cancelled_tickets() {
    let gate = WriteGate::new();
    let normal = gate
        .reserve(56 * 1024 * 1024, StorageWriteClass::Normal)
        .unwrap();
    assert!(gate.reserve(1, StorageWriteClass::Normal).is_err());
    let control = gate
        .reserve(8 * 1024 * 1024, StorageWriteClass::Control)
        .unwrap();
    assert!(gate.reserve(1, StorageWriteClass::Control).is_err());
    drop(normal);
    drop(control);
    let mut permits = Vec::new();
    for _ in 0..56 {
        permits.push(gate.reserve(1, StorageWriteClass::Normal).unwrap());
    }
    assert!(gate.reserve(1, StorageWriteClass::Normal).is_err());
    for _ in 0..8 {
        permits.push(gate.reserve(1, StorageWriteClass::Control).unwrap());
    }
    assert!(gate.reserve(1, StorageWriteClass::Control).is_err());
    permits.clear();
    gate.reserve(1, StorageWriteClass::Normal).unwrap().wait();
}

#[test]
fn read_pool_bounds_waiters_and_drains_accepted_reads_after_close() {
    let fixture = Fixture::new();
    let pool = fixture.state.readers.as_ref().unwrap().clone();
    let (ready_send, ready_receive) = mpsc::channel();
    let mut releases = Vec::new();
    let mut holders = Vec::new();
    for _ in 0..READ_CONNECTIONS {
        let pool = pool.clone();
        let ready = ready_send.clone();
        let (send, receive) = mpsc::channel();
        releases.push(send);
        holders.push(std::thread::spawn(move || {
            let _reader = pool.acquire().unwrap();
            ready.send(()).unwrap();
            receive.recv().unwrap();
        }));
    }
    for _ in 0..READ_CONNECTIONS {
        ready_receive.recv_timeout(Duration::from_secs(2)).unwrap();
    }
    let mut waiters = Vec::new();
    for _ in READ_CONNECTIONS..READ_JOBS {
        let pool = pool.clone();
        waiters.push(std::thread::spawn(move || {
            let _reader = pool.acquire().unwrap();
        }));
    }
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if pool.state.lock().unwrap().jobs == READ_JOBS {
            break;
        }
        assert!(Instant::now() < deadline, "all bounded read jobs admitted");
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(matches!(pool.acquire(), Err(error) if error == "storage_read_pool_overloaded"));
    pool.close();
    assert!(matches!(pool.acquire(), Err(error) if error == "storage_executor_closed"));
    for release in releases {
        release.send(()).unwrap();
    }
    for job in holders.into_iter().chain(waiters) {
        job.join().unwrap();
    }
    let state = pool.state.lock().unwrap();
    assert_eq!(state.jobs, 0);
    assert_eq!(state.connections.len(), READ_CONNECTIONS);
}
