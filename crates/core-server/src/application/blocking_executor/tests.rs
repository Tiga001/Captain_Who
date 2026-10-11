use super::*;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Duration;

fn executor(workers: usize, count: usize, bytes: usize) -> BlockingExecutor {
    BlockingExecutor::new("test", workers, count, bytes).unwrap()
}

#[tokio::test]
async fn resizing_reservations_is_atomic_and_preserves_settlement_after_overload() {
    let pool = executor(1, 2, 12);
    let mut first = pool.try_reserve(4).unwrap();
    let second = pool.try_reserve(6).unwrap();
    assert_eq!(first.try_resize(7), Err(BlockingError::Overloaded));
    assert_eq!(first.try_resize(13), Err(BlockingError::TooLarge));
    assert_eq!(first.bytes, 4);
    assert_eq!(pool.0 .0.state.lock().unwrap().bytes, 10);
    first.try_resize(6).unwrap();
    assert_eq!(pool.0 .0.state.lock().unwrap().bytes, 12);
    assert!(matches!(
        pool.try_reserve(0),
        Err(BlockingError::Overloaded)
    ));
    drop(second);
    pool.close();
    first.try_resize(12).unwrap();
    assert_eq!(first.submit(|| "settled").wait().await, Ok("settled"));
    pool.shutdown().await;
    assert_eq!(pool.0 .0.state.lock().unwrap().bytes, 0);
}

#[tokio::test]
async fn shrinking_wakes_capacity_waiter_and_final_shutdown_rejects_resize() {
    let pool = executor(1, 2, 10);
    let mut first = pool.try_reserve(10).unwrap();
    let waiting = pool.clone();
    let next = tokio::spawn(async move { waiting.reserve(6).await });
    tokio::task::yield_now().await;
    assert!(!next.is_finished());
    first.try_resize(4).unwrap();
    let second = tokio::time::timeout(Duration::from_secs(1), next)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    pool.shutdown_submitted().await;
    assert_eq!(first.try_resize(1), Err(BlockingError::Closed));
    assert_eq!(first.bytes, 4);
    assert_eq!(
        first.submit(|| panic!("late mutation")).wait().await,
        Err(BlockingError::Closed)
    );
    drop(second);
    assert_eq!(pool.0 .0.state.lock().unwrap().bytes, 0);
    assert_eq!(pool.0 .0.state.lock().unwrap().jobs, 0);
}

#[tokio::test]
async fn paired_reservations_are_atomic_and_tail_retains_its_budget() {
    let pool = executor(1, 4, 12);
    let (first, tail) = pool.reserve_pair(4).await.unwrap();
    assert!(matches!(
        pool.try_reserve_pair(4),
        Err(BlockingError::Overloaded)
    ));
    // A failed pair holds neither half; all four remaining bytes are still available.
    let single = pool.try_reserve(4).unwrap();
    assert!(matches!(
        pool.try_reserve_pair(7),
        Err(BlockingError::TooLarge)
    ));
    assert_eq!(first.submit(|| 1).wait().await, Ok(1));
    drop(single);
    let next = pool.reserve_pair(4).await.unwrap();
    assert!(matches!(
        pool.try_reserve(1),
        Err(BlockingError::Overloaded)
    ));
    drop(tail);
    drop(next);
    pool.shutdown().await;
}

#[tokio::test]
async fn count_and_byte_limits_include_reservations_and_release_on_drop() {
    let pool = executor(1, 2, 10);
    let first = pool.try_reserve(6).unwrap();
    assert!(matches!(
        pool.try_reserve(5),
        Err(BlockingError::Overloaded)
    ));
    assert!(matches!(pool.try_reserve(11), Err(BlockingError::TooLarge)));
    let second = pool.try_reserve(4).unwrap();
    assert!(matches!(
        pool.try_reserve(0),
        Err(BlockingError::Overloaded)
    ));
    drop(first);
    assert_eq!(pool.submit(6, || 7).unwrap().wait().await.unwrap(), 7);
    drop(second);
    pool.shutdown().await;
    assert!(matches!(pool.try_reserve(0), Err(BlockingError::Closed)));
}

#[tokio::test(flavor = "current_thread")]
async fn queued_cancel_does_not_run_and_started_drop_is_drained() {
    let pool = executor(1, 2, 10);
    let (started_tx, started_rx) = oneshot::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let committed = Arc::new(AtomicBool::new(false));
    let commit = Arc::clone(&committed);
    let first = pool
        .submit(5, move || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            commit.store(true, Ordering::Release);
        })
        .unwrap();
    started_rx.await.unwrap();
    assert!(!first.cancel_before_start());
    let second = pool
        .submit(5, || panic!("cancelled closure must not execute"))
        .unwrap();
    assert!(second.cancel_before_start());
    drop(first); // Dropping a future is deliberately not a rollback.
    pool.close();
    let draining = pool.clone();
    let shutdown = tokio::spawn(async move { draining.shutdown().await });
    tokio::time::sleep(Duration::from_millis(10)).await;
    assert!(!shutdown.is_finished());
    release_tx.send(()).unwrap();
    assert_eq!(second.wait().await, Err(BlockingError::Cancelled));
    shutdown.await.unwrap();
    assert!(committed.load(Ordering::Acquire));
}

#[tokio::test]
async fn reservation_accepted_before_shutdown_can_finish_after_close() {
    let pool = executor(1, 1, 10);
    let reservation = pool.try_reserve(10).unwrap();
    pool.close();
    assert_eq!(reservation.submit(|| 42).wait().await.unwrap(), 42);
    pool.shutdown().await;
}

#[tokio::test]
async fn panic_returns_error_releases_capacity_and_worker_survives() {
    let pool = executor(1, 1, 10);
    assert_eq!(
        pool.submit(10, || panic!("injected failure"))
            .unwrap()
            .wait()
            .await,
        Err(BlockingError::Panicked)
    );
    let reservation = pool.reserve(10).await.unwrap();
    assert_eq!(reservation.submit(|| 9).wait().await.unwrap(), 9);
    pool.shutdown().await;
}

#[tokio::test(flavor = "current_thread")]
async fn captured_runtime_context_can_launch_async_runtime_work() {
    let pool = executor(1, 1, 10);
    let task = pool.run(0, || tokio::spawn(async { 5 })).await.unwrap();
    assert_eq!(task.await.unwrap(), 5);
    pool.shutdown().await;
}

#[tokio::test(flavor = "current_thread")]
async fn slow_preparation_does_not_block_control_lease_or_runtime_timer() {
    let prep = executor(1, 1, 10);
    let control = executor(1, 1, 10);
    let maintenance = executor(1, 1, 10);
    let (started_tx, started_rx) = oneshot::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let preparation = prep
        .submit(10, move || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        })
        .unwrap();
    started_rx.await.unwrap();
    let progress = Arc::new(AtomicUsize::new(0));
    for _ in 0..10 {
        let progress = Arc::clone(&progress);
        control
            .run(0, move || progress.fetch_add(1, Ordering::Relaxed))
            .await
            .unwrap();
        assert_eq!(
            maintenance.run(0, || "lease CAS").await.unwrap(),
            "lease CAS"
        );
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    assert_eq!(progress.load(Ordering::Relaxed), 10);
    release_tx.send(()).unwrap();
    preparation.wait().await.unwrap();
    prep.shutdown().await;
    control.shutdown().await;
    maintenance.shutdown().await;
}

#[tokio::test(flavor = "current_thread")]
async fn waiting_reservation_wakes_when_capacity_released_or_executor_closed() {
    let pool = executor(1, 1, 10);
    let held = pool.try_reserve(10).unwrap();
    let waiting = pool.clone();
    let task = tokio::spawn(async move { waiting.reserve(10).await });
    tokio::task::yield_now().await;
    drop(held);
    drop(task.await.unwrap().unwrap());
    let held = pool.try_reserve(10).unwrap();
    let waiting = pool.clone();
    let task = tokio::spawn(async move { waiting.reserve(10).await });
    tokio::task::yield_now().await;
    pool.close();
    assert!(matches!(task.await.unwrap(), Err(BlockingError::Closed)));
    drop(held);
    pool.shutdown().await;
}

#[tokio::test(flavor = "current_thread")]
async fn final_shutdown_drains_submitted_work_but_fences_unused_reservations() {
    let pool = executor(1, 2, 10);
    let unused = pool.try_reserve(5).unwrap();
    let (started_tx, started_rx) = oneshot::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let work = pool
        .submit(5, move || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            7
        })
        .unwrap();
    started_rx.await.unwrap();
    let draining = pool.clone();
    let shutdown = tokio::spawn(async move { draining.shutdown_submitted().await });
    tokio::time::sleep(Duration::from_millis(10)).await;
    assert!(!shutdown.is_finished());
    release_tx.send(()).unwrap();
    shutdown.await.unwrap();
    assert_eq!(work.wait().await, Ok(7));
    assert_eq!(
        unused
            .submit(|| panic!("late model must not mutate"))
            .wait()
            .await,
        Err(BlockingError::Closed)
    );
}
