use super::*;
use std::future::Future;
use std::pin::Pin;
use std::sync::mpsc;
use std::task::{Context, Poll, Waker};

/// Stop polling the accepted async predecessor completely. Its ability to release a later
/// sync caller must depend on the writer, not on getting another turn on that caller's runtime.
fn assert_async_predecessor_advances_without_repoll(checkpoint_job: bool) {
    let directory = tempfile::tempdir().unwrap();
    let state = Arc::new(StorageState::open(&directory.path().join("admission.sqlite")).unwrap());
    state
        .write_sync(1, StorageWriteClass::Normal, |connection| {
            connection
                .execute_batch("CREATE TABLE admission(value); INSERT INTO admission VALUES(1)")
        })
        .unwrap()
        .unwrap();

    let (held_send, held_receive) = mpsc::channel();
    let (release_send, release_receive) = mpsc::channel();
    let held_state = state.clone();
    let holder = std::thread::spawn(move || {
        let connection = held_state.connection().unwrap();
        held_send.send(()).unwrap();
        release_receive.recv().unwrap();
        drop(connection);
    });
    held_receive.recv_timeout(Duration::from_secs(2)).unwrap();

    let mut predecessor: Pin<Box<dyn Future<Output = Result<(), String>> + Send>> =
        if checkpoint_job {
            let checkpoint = state.checkpoint();
            Box::pin(async move { checkpoint.await.map(|_| ()) })
        } else {
            let write = state.write_async(1, StorageWriteClass::Normal, |connection| {
                connection.execute("UPDATE admission SET value = 12", [])
            });
            Box::pin(async move { write.await?.map(|_| ()).map_err(|error| error.to_string()) })
        };
    assert!(matches!(
        predecessor
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop())),
        Poll::Pending
    ));

    let (finished_send, finished_receive) = mpsc::channel();
    let successor_state = state.clone();
    let successor = std::thread::spawn(move || {
        let result = successor_state.write_sync(1, StorageWriteClass::Normal, |connection| {
            let preceding: i64 = connection
                .query_row("SELECT value FROM admission", [], |row| row.get(0))
                .unwrap();
            connection
                .execute("UPDATE admission SET value = 123", [])
                .unwrap();
            preceding
        });
        let _ = finished_send.send(result);
    });
    release_send.send(()).unwrap();
    holder.join().unwrap();
    // Use an OS watchdog: a timeout future on the blocked runtime cannot diagnose this bug.
    let before_cancellation = finished_receive.recv_timeout(Duration::from_secs(2));
    // On the old implementation, dropping this future cancels its ticket and releases the
    // successor. Always do that before asserting, so the regression fails without hanging.
    drop(predecessor);
    let completed_without_repoll = before_cancellation.is_ok();
    let result = before_cancellation.unwrap_or_else(|_| {
        finished_receive
            .recv_timeout(Duration::from_secs(2))
            .expect("cancelling the queued predecessor must release the synthetic waiter")
    });
    successor.join().unwrap();
    assert!(
        completed_without_repoll,
        "an accepted async predecessor must progress without repoll"
    );
    assert_eq!(result.unwrap(), if checkpoint_job { 1 } else { 12 });
    let reopened = Connection::open(directory.path().join("admission.sqlite")).unwrap();
    assert_eq!(
        reopened
            .query_row("SELECT value FROM admission", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        123
    );
}

#[test]
fn queued_async_writer_releases_sync_successor_without_future_repoll() {
    assert_async_predecessor_advances_without_repoll(false);
}

#[test]
fn queued_checkpoint_releases_sync_successor_without_future_repoll() {
    assert_async_predecessor_advances_without_repoll(true);
}

#[test]
fn async_registration_after_predecessor_release_and_close_still_dispatches() {
    let gate = WriteGate::new();
    let first = gate.reserve(1, StorageWriteClass::Normal).unwrap();
    let second = gate.reserve(1, StorageWriteClass::Control).unwrap();
    drop(first);
    gate.close();
    let (sent, received) = mpsc::channel();
    let queued = second.dispatch(move |permit| {
        drop(permit);
        sent.send(()).unwrap();
    });
    received.recv_timeout(Duration::from_secs(2)).unwrap();
    drop(queued);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(2), gate.drained())
            .await
            .unwrap();
    });
    assert!(gate.reserve(1, StorageWriteClass::Control).is_err());
}

#[test]
fn queued_cancellation_releases_ticket_before_captured_value_reenters_gate() {
    struct ReenterOnDrop {
        gate: Arc<WriteGate>,
        entered: mpsc::Sender<()>,
        completed: mpsc::Sender<()>,
    }
    impl Drop for ReenterOnDrop {
        fn drop(&mut self) {
            self.entered.send(()).unwrap();
            let permit = self.gate.reserve(1, StorageWriteClass::Normal).unwrap();
            permit.wait();
            drop(permit);
            self.completed.send(()).unwrap();
        }
    }
    let gate = WriteGate::new();
    let weak_gate = Arc::downgrade(&gate);
    let first = gate.reserve(1, StorageWriteClass::Normal).unwrap();
    let second = gate.reserve(1, StorageWriteClass::Normal).unwrap();
    let (entered_send, entered_receive) = mpsc::channel();
    let (completed_send, completed_receive) = mpsc::channel();
    let capture = ReenterOnDrop {
        gate: gate.clone(),
        entered: entered_send,
        completed: completed_send,
    };
    let executed = Arc::new(AtomicBool::new(false));
    let job_executed = executed.clone();
    let queued = second.dispatch(move |permit| {
        let _capture = capture;
        job_executed.store(true, Ordering::Release);
        drop(permit);
    });
    let cancelling = std::thread::spawn(move || drop(queued));
    entered_receive
        .recv_timeout(Duration::from_secs(2))
        .unwrap();
    drop(first);
    completed_receive
        .recv_timeout(Duration::from_secs(2))
        .unwrap();
    cancelling.join().unwrap();
    assert!(!executed.load(Ordering::Acquire));
    drop(gate);
    assert!(
        weak_gate.upgrade().is_none(),
        "queued callbacks must not keep the gate alive"
    );
}

#[test]
fn concurrent_async_promotion_and_cancellation_execute_at_most_once_and_restore_capacity() {
    use std::sync::atomic::AtomicUsize;
    use std::sync::Barrier;

    for _ in 0..64 {
        let gate = WriteGate::new();
        let first = gate.reserve(1, StorageWriteClass::Normal).unwrap();
        let second = gate.reserve(1, StorageWriteClass::Normal).unwrap();
        let executed = Arc::new(AtomicUsize::new(0));
        let job_executed = executed.clone();
        let queued = second.dispatch(move |permit| {
            job_executed.fetch_add(1, Ordering::AcqRel);
            drop(permit);
        });
        let barrier = Arc::new(Barrier::new(2));
        let cancelling_barrier = barrier.clone();
        let cancelling = std::thread::spawn(move || {
            cancelling_barrier.wait();
            drop(queued);
        });
        barrier.wait();
        drop(first);
        cancelling.join().unwrap();
        assert!(executed.load(Ordering::Acquire) <= 1);
        let mut all_capacity = Vec::new();
        for _ in 0..56 {
            all_capacity.push(
                gate.reserve(1024 * 1024, StorageWriteClass::Normal)
                    .unwrap(),
            );
        }
        for _ in 0..8 {
            all_capacity.push(
                gate.reserve(1024 * 1024, StorageWriteClass::Control)
                    .unwrap(),
            );
        }
        assert!(gate.reserve(1, StorageWriteClass::Control).is_err());
    }
}

#[test]
fn submitted_writer_survives_future_cancellation_before_worker_execution() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("claimed.sqlite");
    let state = StorageState::open(&path).unwrap();
    let (busy_send, busy_receive) = mpsc::channel();
    let (release_send, release_receive) = mpsc::channel();
    state.worker.spawn(move || {
        busy_send.send(()).unwrap();
        release_receive.recv().unwrap();
    });
    busy_receive.recv_timeout(Duration::from_secs(2)).unwrap();
    let mut submitted = Box::pin(
        state.write_async(1, StorageWriteClass::Normal, |connection| {
            connection.execute_batch("CREATE TABLE claimed(value); INSERT INTO claimed VALUES(7)")
        }),
    );
    assert!(matches!(
        submitted
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop())),
        Poll::Pending
    ));
    drop(submitted);
    release_send.send(()).unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(2), state.shutdown())
            .await
            .unwrap()
            .unwrap();
    });
    let reopened = Connection::open(path).unwrap();
    assert_eq!(
        reopened
            .query_row("SELECT value FROM claimed", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        7
    );
}

#[tokio::test]
async fn panicking_async_writer_releases_admission_and_does_not_pin_shutdown() {
    let directory = tempfile::tempdir().unwrap();
    let state = StorageState::open(&directory.path().join("panicked.sqlite")).unwrap();
    let result = state
        .write_async(1, StorageWriteClass::Normal, |_| {
            panic!("synthetic writer panic")
        })
        .await;
    assert_eq!(result.unwrap_err(), "storage_worker_panicked");
    tokio::time::timeout(Duration::from_secs(2), state.writer.gate.drained())
        .await
        .unwrap();
    // The connection mutex is poisoned, but its permit is gone and shutdown must return an
    // error instead of waiting forever for a transaction that already unwound.
    assert!(
        tokio::time::timeout(Duration::from_secs(2), state.shutdown())
            .await
            .unwrap()
            .is_err()
    );
}
