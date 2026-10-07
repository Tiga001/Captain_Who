use super::*;
use mycopilot_core::workflow::WorkflowPermissionMode;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::mpsc;

fn preference(
    revision: u64,
    model: Option<&str>,
    permission: Option<WorkflowPermissionMode>,
) -> PreferenceUpdate {
    PreferenceUpdate {
        node_id: "member".into(),
        conversation_id: "chat".into(),
        organization_revision: revision,
        model_id: model.map(str::to_owned),
        permission_mode: permission,
    }
}
fn snapshot(instance: &str) -> RuntimeSnapshot {
    RuntimeSnapshot {
        summary: None,
        instance_id: instance.into(),
        sequence: 9,
        inputs: vec![],
        events: vec![],
        paused_conversation_ids: vec![],
        input_runs: vec![],
        preference_updates: vec![],
    }
}
fn take(queue: &WorkflowRuntimePublications, now: Instant) -> (String, Pending) {
    match queue.next(now) {
        Next::Publish(instance, pending) => (instance, pending),
        _ => panic!("expected pending publication"),
    }
}
async fn receive<T>(rx: &mut mpsc::UnboundedReceiver<T>) -> T {
    tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .unwrap()
        .unwrap()
}

#[test]
fn synchronous_service_does_not_require_or_start_a_runtime() {
    let queue = WorkflowRuntimePublications::default();
    assert!(queue
        .enqueue("org", vec![preference(2, Some("a"), None)])
        .is_some());
    assert!(queue.state.lock().unwrap().pending.is_empty());
    queue.close();
    assert!(queue.enqueue("org", vec![]).is_none());
    assert!(queue.start().is_err());
}

#[test]
fn fixed_window_merges_bursts_without_postponing_continuous_activity() {
    let queue = WorkflowRuntimePublications::default();
    queue.start().unwrap();
    let start = Instant::now();
    for ms in 0..100 {
        queue.enqueue_at("org", vec![], start + Duration::from_millis(ms));
    }
    assert!(matches!(
        queue.next(start + Duration::from_millis(99)),
        Next::Wait(Some(_))
    ));
    let (instance, pending) = take(&queue, start + REFRESH_WINDOW);
    assert_eq!(instance, "org");
    assert_eq!(pending.due, start + REFRESH_WINDOW);
    assert!(matches!(
        queue.next(start + REFRESH_WINDOW),
        Next::Wait(None)
    ));
    // A mutation during the first projection schedules another independent fixed window.
    queue.enqueue_at("org", vec![], start + REFRESH_WINDOW);
    assert_eq!(
        take(&queue, start + REFRESH_WINDOW * 2).1.due,
        start + REFRESH_WINDOW * 2
    );
}

#[test]
fn organizations_keep_independent_deadlines_and_earliest_gets_served_first() {
    let queue = WorkflowRuntimePublications::default();
    queue.start().unwrap();
    let now = Instant::now();
    queue.enqueue_at("z-first", vec![], now);
    queue.enqueue_at("a-second", vec![], now + Duration::from_millis(50));
    for _ in 0..100 {
        queue.enqueue_at("z-first", vec![], now + Duration::from_millis(99));
    }
    assert_eq!(take(&queue, now + Duration::from_millis(200)).0, "z-first");
    assert_eq!(take(&queue, now + Duration::from_millis(200)).0, "a-second");
}

#[test]
fn fields_keep_original_revisions_and_late_intermediate_edits_win_per_field() {
    let mut pending = Pending::new(Instant::now());
    pending.merge(vec![preference(2, Some("a"), None)]);
    pending.merge(vec![preference(
        4,
        None,
        Some(WorkflowPermissionMode::Full),
    )]);
    pending.merge(vec![preference(3, Some("b"), None)]);
    pending.merge(vec![preference(2, Some("stale"), None)]);
    let batches = pending.preference_batches();
    assert_eq!(batches.len(), 1);
    assert_eq!(batches[0].len(), 2);
    assert_eq!(batches[0][0].organization_revision, 3);
    assert_eq!(batches[0][0].model_id.as_deref(), Some("b"));
    assert!(batches[0][0].permission_mode.is_none());
    assert_eq!(batches[0][1].organization_revision, 4);
    assert!(batches[0][1].model_id.is_none());
    pending.merge(vec![preference(4, Some("c"), None)]);
    let batches = pending.preference_batches();
    assert_eq!(batches[0].len(), 1);
    assert_eq!(batches[0][0].model_id.as_deref(), Some("c"));
    assert_eq!(
        batches[0][0].permission_mode,
        Some(WorkflowPermissionMode::Full)
    );
}

#[test]
fn rebinding_never_discards_or_retargets_an_earlier_conversation_edit() {
    let mut pending = Pending::new(Instant::now());
    pending.merge(vec![preference(2, Some("old-chat-model"), None)]);
    let mut rebound = preference(4, None, Some(WorkflowPermissionMode::Full));
    rebound.conversation_id = "new-chat".into();
    pending.merge(vec![rebound]);
    let batches = pending.preference_batches();
    assert_eq!(batches.len(), 2);
    assert_eq!(batches[0][0].conversation_id, "chat");
    assert_eq!(batches[0][0].organization_revision, 2);
    assert_eq!(batches[1][0].conversation_id, "new-chat");
    assert_eq!(batches[1][0].organization_revision, 4);
}

#[test]
fn historical_edits_to_more_than_128_nodes_stay_in_bounded_protocol_batches() {
    let mut pending = Pending::new(Instant::now());
    for index in 0..129 {
        let mut update = preference(index + 1, Some("a"), Some(WorkflowPermissionMode::Full));
        update.node_id = format!("member-{index}");
        pending.merge(vec![update]);
    }
    let batches = pending.preference_batches();
    assert_eq!(
        batches.iter().map(Vec::len).collect::<Vec<_>>(),
        vec![128, 1]
    );
}

#[test]
fn failed_projection_merges_newer_changes_without_bypassing_retry_cooldown() {
    let queue = WorkflowRuntimePublications::default();
    queue.start().unwrap();
    let now = Instant::now();
    queue.enqueue_at("org", vec![preference(2, Some("a"), None)], now);
    let (instance, pending) = take(&queue, now + REFRESH_WINDOW);
    queue.enqueue_at(
        "org",
        vec![preference(3, Some("b"), None)],
        now + REFRESH_WINDOW,
    );
    queue.retry(instance, pending, now + REFRESH_WINDOW);
    assert!(matches!(
        queue.next(now + REFRESH_WINDOW * 2),
        Next::Wait(Some(_))
    ));
    let (_, pending) = take(&queue, now + REFRESH_WINDOW * 3);
    assert_eq!(pending.failures, 1);
    assert_eq!(
        pending.preference_batches()[0][0].model_id.as_deref(),
        Some("b")
    );
}

#[test]
fn retries_keep_preferences_with_capped_delay_and_shutdown_does_not_retry() {
    let queue = WorkflowRuntimePublications::default();
    queue.start().unwrap();
    let now = Instant::now();
    let mut failed = Pending::new(now);
    failed.failures = 20;
    failed.merge(vec![preference(2, Some("must-not-lose"), None)]);
    queue.retry("org".into(), failed, now);
    let (instance, pending) = take(&queue, now + MAX_RETRY_DELAY);
    assert_eq!(pending.due, now + MAX_RETRY_DELAY);
    assert_eq!(
        pending.preference_batches()[0][0].model_id.as_deref(),
        Some("must-not-lose")
    );
    queue.retry(instance, pending, now);
    queue.draining();
    queue.enqueue_at("late", vec![], now);
    let (instance, pending) = take(&queue, now);
    queue.retry(instance, pending, now);
    assert!(matches!(queue.next(now), Next::Finished));
}

#[tokio::test]
async fn worker_coalesces_before_projection_and_drains_final_committed_change() {
    let queue = WorkflowRuntimePublications::default();
    queue.start().unwrap();
    let (sender, mut receiver) = crate::transport::outbound_channel();
    let calls = Arc::new(AtomicUsize::new(0));
    let project_calls = calls.clone();
    let publisher = spawn_worker(
        queue.clone(),
        sender,
        Arc::new(move |instance| {
            project_calls.fetch_add(1, Ordering::SeqCst);
            Ok(snapshot(instance))
        }),
    );
    for _ in 0..1000 {
        queue.enqueue("org", vec![]);
    }
    queue.enqueue("org", vec![preference(2, Some("a"), None)]);
    queue.enqueue(
        "org",
        vec![preference(4, None, Some(WorkflowPermissionMode::Full))],
    );
    publisher.shutdown(Duration::from_secs(2)).await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let notification = receiver.recv().await.unwrap();
    assert_eq!(
        notification["params"]["preferenceUpdates"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert!(receiver.recv().await.is_none());
}

#[tokio::test]
async fn dirty_during_projection_gets_a_second_snapshot_without_overlapping_reads() {
    let queue = WorkflowRuntimePublications::default();
    queue.start().unwrap();
    let (sender, mut receiver) = crate::transport::outbound_channel();
    let (started_tx, mut started_rx) = mpsc::unbounded_channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let release_rx = Mutex::new(release_rx);
    let calls = Arc::new(AtomicUsize::new(0));
    let project_calls = calls.clone();
    let publisher = spawn_worker(
        queue.clone(),
        sender,
        Arc::new(move |instance| {
            let call = project_calls.fetch_add(1, Ordering::SeqCst);
            started_tx.send(call).unwrap();
            if call == 0 {
                release_rx
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(5))
                    .unwrap();
            }
            Ok(snapshot(instance))
        }),
    );
    queue.enqueue_at("org", vec![], Instant::now() - REFRESH_WINDOW);
    assert_eq!(receive(&mut started_rx).await, 0);
    queue.enqueue("org", vec![preference(3, Some("new"), None)]);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    release_tx.send(()).unwrap();
    publisher.shutdown(Duration::from_secs(2)).await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert!(receiver.recv().await.unwrap()["params"]
        .get("preferenceUpdates")
        .is_none());
    assert_eq!(
        receiver.recv().await.unwrap()["params"]["preferenceUpdates"][0]["modelId"],
        "new"
    );
    assert!(receiver.recv().await.is_none());
}

#[tokio::test]
async fn consecutive_failures_keep_preferences_until_recovery_and_transport_close_stops_work() {
    let queue = WorkflowRuntimePublications::default();
    queue.start().unwrap();
    let (sender, mut receiver) = crate::transport::outbound_channel();
    let calls = Arc::new(AtomicUsize::new(0));
    let project_calls = calls.clone();
    let publisher = spawn_worker(
        queue.clone(),
        sender,
        Arc::new(move |instance| {
            if project_calls.fetch_add(1, Ordering::SeqCst) < 4 {
                Err(ProjectionFailure::Temporary(
                    "temporary read failure".into(),
                ))
            } else {
                Ok(snapshot(instance))
            }
        }),
    );
    queue.enqueue_at(
        "org",
        vec![preference(3, Some("new"), None)],
        Instant::now() - REFRESH_WINDOW,
    );
    let notification = tokio::time::timeout(Duration::from_secs(5), receiver.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        notification["params"]["preferenceUpdates"][0]["modelId"],
        "new"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 5);
    drop(receiver);
    queue.enqueue("org", vec![]);
    publisher.shutdown(Duration::from_secs(2)).await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 5);
}

#[tokio::test]
async fn shutdown_timeout_fences_slow_read_completion_and_late_mutations() {
    let queue = WorkflowRuntimePublications::default();
    queue.start().unwrap();
    let (sender, mut receiver) = crate::transport::outbound_channel();
    let (started_tx, mut started_rx) = mpsc::unbounded_channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let release_rx = Mutex::new(release_rx);
    let publisher = spawn_worker(
        queue.clone(),
        sender,
        Arc::new(move |instance| {
            started_tx.send(()).unwrap();
            release_rx
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(5))
                .unwrap();
            Ok(snapshot(instance))
        }),
    );
    queue.enqueue_at("org", vec![], Instant::now() - REFRESH_WINDOW);
    receive(&mut started_rx).await;
    assert!(publisher.shutdown(Duration::from_millis(10)).await.is_err());
    queue.enqueue("late", vec![]);
    release_tx.send(()).unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(5), receiver.recv())
            .await
            .unwrap()
            .is_none()
    );
    assert!(matches!(queue.next(Instant::now()), Next::Finished));
}

#[test]
fn only_authoritative_instance_missing_terminates_projection_retries() {
    assert!(matches!(
        classify_storage_projection_error("Organization instance no longer exists".into()),
        ProjectionFailure::InstanceMissing
    ));
    for error in [
        "database is locked",
        "database connection failed",
        "invalid JSON",
        "unrelated missing row",
    ] {
        assert!(matches!(
            classify_storage_projection_error(error.into()),
            ProjectionFailure::Temporary(_)
        ));
    }
}

#[tokio::test]
async fn deleted_instance_does_not_retry_after_pending_or_inflight_changes() {
    let queue = WorkflowRuntimePublications::default();
    queue.start().unwrap();
    let (sender, mut receiver) = crate::transport::outbound_channel();
    let (started_tx, mut started_rx) = mpsc::unbounded_channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let release_rx = Mutex::new(release_rx);
    let calls = Arc::new(AtomicUsize::new(0));
    let project_calls = calls.clone();
    let publisher = spawn_worker(
        queue.clone(),
        sender,
        Arc::new(move |_| {
            let call = project_calls.fetch_add(1, Ordering::SeqCst);
            started_tx.send(call).unwrap();
            if call == 0 {
                release_rx
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(5))
                    .unwrap();
            }
            Err(ProjectionFailure::InstanceMissing)
        }),
    );
    queue.enqueue_at(
        "org",
        vec![preference(2, Some("old"), None)],
        Instant::now() - REFRESH_WINDOW,
    );
    assert_eq!(receive(&mut started_rx).await, 0);
    queue.enqueue_at(
        "org",
        vec![preference(3, Some("new"), None)],
        Instant::now() - REFRESH_WINDOW,
    );
    release_tx.send(()).unwrap();
    assert_eq!(receive(&mut started_rx).await, 1);
    // Longer than the first retry delay: an ordinary transient error would attempt again.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert!(queue.state.lock().unwrap().pending.is_empty());
    publisher.shutdown(Duration::from_secs(2)).await.unwrap();
    assert!(receiver.recv().await.is_none());
}

#[tokio::test]
async fn dropping_owner_wakes_idle_worker_and_fences_future_publication() {
    let queue = WorkflowRuntimePublications::default();
    queue.start().unwrap();
    let (sender, mut receiver) = crate::transport::outbound_channel();
    let publisher = spawn_worker(
        queue.clone(),
        sender,
        Arc::new(|instance| Ok(snapshot(instance))),
    );
    tokio::task::yield_now().await;
    drop(publisher);
    assert!(
        tokio::time::timeout(Duration::from_secs(5), receiver.recv())
            .await
            .unwrap()
            .is_none()
    );
    assert!(queue.enqueue("late", vec![]).is_none());
    assert!(matches!(queue.next(Instant::now()), Next::Finished));
}
