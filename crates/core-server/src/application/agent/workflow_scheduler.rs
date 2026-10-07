use super::*;

// Deadline-driven retries and committed readiness changes do normal work. This sparse scan
// recovers missed external changes; hints never replace durable eligibility/claim checks.
const WORKFLOW_RECOVERY_INTERVAL: Duration = Duration::from_secs(30);

#[derive(Clone, Default)]
pub(super) struct WorkflowSchedulerWake {
    notify: Arc<Notify>,
    running: Arc<AtomicBool>,
    #[cfg(test)]
    pub(super) scans: Arc<AtomicU64>,
}

impl WorkflowSchedulerWake {
    fn wake(&self) {
        // notify_one retains one permit even when the worker is scanning. The worker consumes it
        // BEFORE scanning; it never clears a pending flag after scanning. A concurrent commit
        // therefore always causes a subsequent scan, including at the completion boundary.
        self.notify.notify_one();
    }
}

/// Owns the only blocking workflow scan. Shutdown joins it rather than aborting an async wrapper
/// and detaching an already-started spawn_blocking job.
pub(crate) struct WorkflowDeliveryScheduler {
    service: AgentService,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl AgentService {
    pub(crate) fn wake_workflow_deliveries(&self) {
        if !self.workflow_dispatch_stopped.load(Ordering::Acquire) {
            self.workflow_scheduler_wake.wake();
        }
    }

    /// Invalidate only hints; the next attempt reloads authoritative configuration and claims.
    pub(crate) fn workflow_readiness_changed(&self, conversation_id: Option<&str>) {
        self.workflow_retry
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .changed(conversation_id);
        self.wake_workflow_deliveries();
    }

    pub(super) fn workflow_capacity_changed(&self) {
        self.workflow_retry
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .capacity_changed();
        self.wake_workflow_deliveries();
    }

    #[cfg(test)]
    pub(crate) fn workflow_readiness_generation_for_test(&self) -> u64 {
        self.workflow_retry
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .generation
    }

    pub(crate) fn start_workflow_delivery_scheduler(
        &self,
        notifications: CoreServerNotificationSender,
    ) -> Result<WorkflowDeliveryScheduler, String> {
        self.start_workflow_delivery_scheduler_with_interval(
            notifications,
            WORKFLOW_RECOVERY_INTERVAL,
        )
    }

    pub(super) fn start_workflow_delivery_scheduler_with_interval(
        &self,
        notifications: CoreServerNotificationSender,
        recovery_interval: Duration,
    ) -> Result<WorkflowDeliveryScheduler, String> {
        if self
            .workflow_scheduler_wake
            .running
            .swap(true, Ordering::AcqRel)
        {
            return Err("Organization delivery scheduler is already running.".into());
        }
        if self.workflow_dispatch_stopped.load(Ordering::Acquire) {
            return Err("Organization delivery scheduler is shutting down.".into());
        }
        let service = self.clone();
        let task = spawn_worker(
            self.workflow_scheduler_wake.clone(),
            self.workflow_dispatch_stopped.clone(),
            recovery_interval,
            Some(self.workflow_retry.clone()),
            Arc::new(move || service.dispatch_workflow_deliveries(notifications.clone())),
        );
        Ok(WorkflowDeliveryScheduler {
            service: self.clone(),
            task: Some(task),
        })
    }

    /// This fence shares the root/transition admission lock. Once it returns, neither a queued
    /// scan nor an in-flight preparation can admit a new workflow turn or provider transition.
    pub(crate) fn stop_workflow_delivery_admissions(&self) {
        let _admission = self
            .conversation_admission
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        self.workflow_dispatch_stopped
            .store(true, Ordering::Release);
        self.workflow_scheduler_wake.wake();
    }
}

impl WorkflowDeliveryScheduler {
    pub(crate) async fn shutdown(mut self) -> Result<(), String> {
        let service = self.service.clone();
        tokio::task::spawn_blocking(move || service.stop_workflow_delivery_admissions())
            .await
            .map_err(|error| format!("Organization scheduler shutdown fence failed: {error}"))?;
        self.task
            .take()
            .expect("scheduler owns its worker")
            .await
            .map_err(|error| format!("Organization scheduler shutdown failed: {error}"))
    }
}

impl Drop for WorkflowDeliveryScheduler {
    fn drop(&mut self) {
        // Normal shutdown must call shutdown() to drain. On an early bootstrap error, prevent
        // further work and let the owned worker finish its current scan instead of aborting it.
        self.service
            .workflow_dispatch_stopped
            .store(true, Ordering::Release);
        self.service.workflow_scheduler_wake.wake();
    }
}

fn spawn_worker(
    wake: WorkflowSchedulerWake,
    stopped: Arc<AtomicBool>,
    recovery_interval: Duration,
    retries: Option<Arc<Mutex<super::workflow_retry::WorkflowRetryState>>>,
    scan: Arc<dyn Fn() + Send + Sync>,
) -> tokio::task::JoinHandle<()> {
    wake.wake(); // Startup recovers durable inputs without depending on a renderer request.
    tokio::spawn(async move {
        let mut recovery = tokio::time::Instant::now() + recovery_interval;
        loop {
            let retry = retries.as_ref().and_then(|state| {
                state
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .next_delay(Instant::now())
            });
            let deadline = retry.map_or(recovery, |delay| {
                recovery.min(tokio::time::Instant::now() + delay)
            });
            tokio::select! {
                biased;
                _ = wake.notify.notified() => {},
                _ = tokio::time::sleep_until(deadline) => {},
            }
            if tokio::time::Instant::now() >= recovery {
                recovery = tokio::time::Instant::now() + recovery_interval;
            }
            if stopped.load(Ordering::Acquire) {
                break;
            }
            let scan = Arc::clone(&scan);
            let stopped_for_scan = Arc::clone(&stopped);
            // Exactly one scan can be queued/running. Never select cancellation against this
            // JoinHandle: blocking work cannot be aborted safely and must drain on shutdown.
            if let Err(error) = tokio::task::spawn_blocking(move || {
                if !stopped_for_scan.load(Ordering::Acquire) {
                    scan();
                }
            })
            .await
            {
                eprintln!("organization scheduler scan failed; recovery will retry: {error}");
                if let Some(retries) = &retries {
                    retries
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .scan_failed(Instant::now());
                }
            }
            if stopped.load(Ordering::Acquire) {
                break;
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::AtomicUsize;
    use tokio::sync::{mpsc, oneshot};

    fn take_wake(wake: &WorkflowSchedulerWake) -> bool {
        use std::task::{Context, Poll, Waker};
        let mut notified = std::pin::pin!(wake.notify.notified());
        matches!(
            notified
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop())),
            Poll::Ready(())
        )
    }

    async fn receive<T>(rx: &mut mpsc::UnboundedReceiver<T>) -> T {
        tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .unwrap()
            .unwrap()
    }

    #[test]
    fn workflow_reads_and_draft_writes_do_not_scan_or_wake_but_readiness_writes_do() {
        let directory = tempfile::tempdir().unwrap();
        let storage =
            Arc::new(StorageService::open(&directory.path().join("readiness.sqlite")).unwrap());
        let service = AgentService::new_authorized_for_test(storage.clone());
        let (notifications, _) = crate::transport::outbound_channel();
        assert!(take_wake(&service.workflow_scheduler_wake)); // Initial execution-access grant.
        let call = |params| {
            crate::transport::handle_request(
                &storage,
                &service,
                notifications.clone(),
                mycopilot_protocol_rs::JsonRpcRequest {
                    jsonrpc: "2.0".into(),
                    id: mycopilot_protocol_rs::JsonRpcId::Number(1),
                    method: "agent.workflows.request".into(),
                    params: Some(params),
                },
            )
        };
        let definition = json!({"schemaVersion":1,"id":"template","name":"Template","description":"","background":"","nodes":[],"viewport":{"x":0,"y":0,"zoom":1}});
        for _ in 0..10 {
            for params in [
                json!({"operation":"list"}),
                json!({"operation":"listInstances"}),
                json!({"operation":"validate","definition":definition}),
                json!({"operation":"runtimeSnapshot","instanceId":"missing"}),
            ] {
                let _ = call(params);
                assert!(!take_wake(&service.workflow_scheduler_wake));
            }
        }
        assert_eq!(
            service
                .workflow_scheduler_wake
                .scans
                .load(Ordering::Acquire),
            0
        );
        assert!(
            call(json!({"operation":"save","definition":definition,"expectedRevision":0}))
                .get("result")
                .is_some()
        );
        assert!(take_wake(&service.workflow_scheduler_wake));
        assert_eq!(
            service
                .workflow_scheduler_wake
                .scans
                .load(Ordering::Acquire),
            0,
            "readiness writes enqueue a hint and never scan in the RPC handler"
        );
        assert!(call(json!({"operation":"saveDraft","definition":definition,"expectedRevision":1,"expectedDraftRevision":0})).get("result").is_some());
        assert!(!take_wake(&service.workflow_scheduler_wake));
        assert!(
            call(json!({"operation":"save","definition":definition,"expectedRevision":0}))
                .get("error")
                .is_some()
        );
        assert!(
            !take_wake(&service.workflow_scheduler_wake),
            "conflicting writes do not wake"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn workflow_scheduler_coalesces_concurrent_busy_wakes_and_keeps_completion_wake() {
        let wake = WorkflowSchedulerWake::default();
        let stopped = Arc::new(AtomicBool::new(false));
        let scans = Arc::new(AtomicUsize::new(0));
        let (started_tx, mut started_rx) = mpsc::unbounded_channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let release_rx = Mutex::new(release_rx);
        let scan_count = scans.clone();
        let scan = Arc::new(move || {
            let number = scan_count.fetch_add(1, Ordering::AcqRel) + 1;
            started_tx.send(number).unwrap();
            release_rx
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(5))
                .unwrap();
        });
        let task = spawn_worker(
            wake.clone(),
            stopped.clone(),
            Duration::from_secs(60),
            None,
            scan,
        );
        assert_eq!(receive(&mut started_rx).await, 1);
        std::thread::scope(|scope| {
            for _ in 0..8 {
                let wake = wake.clone();
                scope.spawn(move || {
                    for _ in 0..1000 {
                        wake.wake();
                    }
                });
            }
        });
        assert_eq!(
            scans.load(Ordering::Acquire),
            1,
            "only one blocking scan may run"
        );
        release_tx.send(()).unwrap();
        assert_eq!(receive(&mut started_rx).await, 2);
        release_tx.send(()).unwrap();
        // A wake at the end of a scan remains a permit; there is no post-scan clear to erase it.
        wake.wake();
        assert_eq!(receive(&mut started_rx).await, 3);
        stopped.store(true, Ordering::Release);
        wake.wake();
        release_tx.send(()).unwrap();
        task.await.unwrap();
        assert_eq!(scans.load(Ordering::Acquire), 3);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn workflow_scheduler_recovery_retries_without_a_wake_and_survives_scan_failure() {
        let wake = WorkflowSchedulerWake::default();
        let stopped = Arc::new(AtomicBool::new(false));
        let attempts = Arc::new(AtomicUsize::new(0));
        let (tx, mut rx) = mpsc::unbounded_channel();
        let scan_attempts = attempts.clone();
        let task = spawn_worker(
            wake.clone(),
            stopped.clone(),
            Duration::from_millis(20),
            None,
            Arc::new(move || {
                let attempt = scan_attempts.fetch_add(1, Ordering::AcqRel) + 1;
                tx.send(attempt).unwrap();
                if attempt == 1 {
                    panic!("injected scan failure");
                }
                // Attempt two represents durable work still blocked by busy/configuration state.
            }),
        );
        assert_eq!(receive(&mut rx).await, 1);
        assert_eq!(receive(&mut rx).await, 2);
        assert_eq!(receive(&mut rx).await, 3);
        stopped.store(true, Ordering::Release);
        wake.wake();
        task.await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn workflow_scheduler_shutdown_drains_blocking_scan_and_rejects_pending_wakes() {
        let wake = WorkflowSchedulerWake::default();
        let stopped = Arc::new(AtomicBool::new(false));
        let scans = Arc::new(AtomicUsize::new(0));
        let (started_tx, mut started_rx) = mpsc::unbounded_channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let release_rx = Mutex::new(release_rx);
        let scan_count = scans.clone();
        let task = spawn_worker(
            wake.clone(),
            stopped.clone(),
            Duration::from_secs(60),
            None,
            Arc::new(move || {
                scan_count.fetch_add(1, Ordering::AcqRel);
                started_tx.send(()).unwrap();
                release_rx
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(5))
                    .unwrap();
            }),
        );
        receive(&mut started_rx).await;
        wake.wake();
        stopped.store(true, Ordering::Release);
        wake.wake();
        let (drained_tx, mut drained_rx) = oneshot::channel();
        let shutdown = tokio::spawn(async move {
            task.await.unwrap();
            drained_tx.send(()).unwrap();
        });
        assert!(
            tokio::time::timeout(Duration::from_millis(30), &mut drained_rx)
                .await
                .is_err(),
            "shutdown must not detach an already-started blocking scan"
        );
        release_tx.send(()).unwrap();
        drained_rx.await.unwrap();
        shutdown.await.unwrap();
        assert_eq!(scans.load(Ordering::Acquire), 1);
    }

    #[tokio::test]
    async fn workflow_scheduler_stop_fences_root_and_provider_transition_admission() {
        let directory = tempfile::tempdir().unwrap();
        let storage =
            Arc::new(StorageService::open(&directory.path().join("shutdown.sqlite")).unwrap());
        let service = AgentService::new_authorized_for_test(storage);
        let (notifications, _) = crate::transport::outbound_channel();
        let scheduler = service
            .start_workflow_delivery_scheduler(notifications.clone())
            .unwrap();
        assert!(service
            .start_workflow_delivery_scheduler(notifications.clone())
            .is_err());
        scheduler.shutdown().await.unwrap();
        service.wake_workflow_deliveries();
        assert!(service.workflow_dispatch_stopped.load(Ordering::Acquire));
        let input = AgentConversationTurnInput {
            conversation_id: Some("recipient".into()),
            project_id: None,
            model_id: "model".into(),
            context_window_indicator_enabled: false,
            content: "queued".into(),
            attachments: vec![],
            folder_references: vec![],
            skills: vec![],
            title: None,
            user_message_id: None,
            assistant_message_id: None,
            max_tokens: None,
            temperature: None,
            prompt_preferences: None,
            permissions: AgentPermissions::default(),
        };
        let workflow = serde_json::from_value(json!({
            "id":"input", "instanceId":"instance", "nodeId":"node", "conversationId":"recipient",
            "executionVersion":"version", "content":"queued", "messages":[], "mailStatus":"pending",
            "status":"pending", "runId":null, "deliveryId":null, "createdAt":1, "error":null
        }))
        .unwrap();
        let error = service
            .start_root_turn_with_workflow(
                input,
                None,
                None,
                None,
                Some(workflow),
                notifications.clone(),
            )
            .unwrap_err();
        assert!(error.to_string().contains("shutting down"));
        assert!(service
            .storage
            .load_conversation("recipient")
            .unwrap()
            .is_none());
        let error = service
            .start_workflow_provider_transition(
                AgentProviderTransitionStartInput {
                    conversation_id: "missing".into(),
                    target_model_id: "model".into(),
                    transition_token: "token".into(),
                },
                notifications,
            )
            .unwrap_err();
        assert!(error.to_string().contains("shutting down"));
        assert!(service
            .start_workflow_delivery_scheduler(crate::transport::outbound_channel().0)
            .is_err());
    }
}

#[cfg(test)]
mod deadline_tests {
    use super::*;
    use mycopilot_core::workflow_execution::PendingInputCandidate;
    use tokio::sync::mpsc;
    fn candidate() -> PendingInputCandidate {
        PendingInputCandidate {
            id: "wait".into(),
            sequence: 1,
            instance_id: "instance".into(),
            execution_version: "v1".into(),
            conversation_id: Some("target".into()),
        }
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn workflow_retry_deadline_precedes_sparse_recovery_and_new_wake_precedes_deadline() {
        let wake = WorkflowSchedulerWake::default();
        let stopped = Arc::new(AtomicBool::new(false));
        let retries = Arc::new(Mutex::new(
            super::super::workflow_retry::WorkflowRetryState::default(),
        ));
        retries.lock().unwrap().defer(
            &candidate(),
            super::super::workflow_retry::RetryReason::Busy,
            0,
            Instant::now() - Duration::from_millis(900),
        );
        let (tx, mut rx) = mpsc::unbounded_channel();
        let count = Arc::new(AtomicU64::new(0));
        let state = retries.clone();
        let sequence = count.clone();
        let task = spawn_worker(
            wake.clone(),
            stopped.clone(),
            Duration::from_secs(60),
            Some(retries),
            Arc::new(move || {
                let n = sequence.fetch_add(1, Ordering::AcqRel) + 1;
                if n > 1 {
                    state.lock().unwrap().defer(
                        &candidate(),
                        super::super::workflow_retry::RetryReason::Busy,
                        0,
                        Instant::now(),
                    );
                }
                tx.send(n).unwrap();
            }),
        );
        assert_eq!(rx.recv().await, Some(1));
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(1), rx.recv())
                .await
                .unwrap(),
            Some(2)
        );
        wake.wake(); // A newly committed input must not wait for the existing retry's deadline.
        assert_eq!(
            tokio::time::timeout(Duration::from_millis(500), rx.recv())
                .await
                .unwrap(),
            Some(3)
        );
        stopped.store(true, Ordering::Release);
        wake.wake();
        task.await.unwrap();
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn workflow_page_sql_failure_backs_off_expired_hints_and_condition_change_recovers() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("scan-fault.sqlite");
        let storage = Arc::new(StorageService::open(&path).unwrap());
        let service = AgentService::new_authorized_for_test(storage);
        let db = rusqlite::Connection::open(&path).unwrap();
        db.execute("DROP INDEX workflow_mail_input_pending_sequence", [])
            .unwrap();
        let generation = service.workflow_retry.lock().unwrap().generation;
        service.workflow_retry.lock().unwrap().defer(
            &candidate(),
            super::super::workflow_retry::RetryReason::Busy,
            generation,
            Instant::now() - Duration::from_secs(5),
        );
        let (notifications, _) = crate::transport::outbound_channel();
        let scheduler = service
            .start_workflow_delivery_scheduler_with_interval(notifications, Duration::from_secs(60))
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if service
                    .workflow_scheduler_wake
                    .scans
                    .load(Ordering::Acquire)
                    > 0
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert_eq!(
            service
                .workflow_scheduler_wake
                .scans
                .load(Ordering::Acquire),
            1,
            "expired entry must not retry failing SQL every 10ms"
        );
        db.execute("CREATE INDEX workflow_mail_input_pending_sequence ON workflow_mail_inputs(sequence) WHERE status='pending'",[]).unwrap();
        service.workflow_readiness_changed(None);
        tokio::time::timeout(Duration::from_millis(500), async {
            loop {
                if service
                    .workflow_scheduler_wake
                    .scans
                    .load(Ordering::Acquire)
                    > 1
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        scheduler.shutdown().await.unwrap();
    }
}
