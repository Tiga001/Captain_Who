use super::*;

// Keep the existing recovery latency for configuration changes which do not yet publish wakes.
// Durable input eligibility and FIFO rules remain in workflow_execution_pending_inputs.
const WORKFLOW_RECOVERY_INTERVAL: Duration = Duration::from_secs(1);

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
            return Err("Workflow delivery scheduler is already running.".into());
        }
        if self.workflow_dispatch_stopped.load(Ordering::Acquire) {
            return Err("Workflow delivery scheduler is shutting down.".into());
        }
        let service = self.clone();
        let task = spawn_worker(
            self.workflow_scheduler_wake.clone(),
            self.workflow_dispatch_stopped.clone(),
            recovery_interval,
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
            .map_err(|error| format!("Workflow scheduler shutdown fence failed: {error}"))?;
        self.task
            .take()
            .expect("scheduler owns its worker")
            .await
            .map_err(|error| format!("Workflow scheduler shutdown failed: {error}"))
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
    scan: Arc<dyn Fn() + Send + Sync>,
) -> tokio::task::JoinHandle<()> {
    wake.wake(); // Startup recovers durable inputs without depending on a renderer request.
    tokio::spawn(async move {
        let mut recovery = tokio::time::interval_at(
            tokio::time::Instant::now() + recovery_interval,
            recovery_interval,
        );
        recovery.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                biased;
                _ = wake.notify.notified() => {},
                _ = recovery.tick() => {},
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
                eprintln!("workflow scheduler scan failed; recovery will retry: {error}");
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
        let (notifications, _) = mpsc::unbounded_channel();
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
        let definition = json!({"schemaVersion":1,"id":"template","name":"Template","description":"","background":"","nodes":[],"flows":[],"viewport":{"x":0,"y":0,"zoom":1},"boundaryPositions":{"input":{"x":0,"y":0}}});
        for _ in 0..10 {
            for params in [
                json!({"operation":"list"}),
                json!({"operation":"listInstances"}),
                json!({"operation":"validate","definition":definition}),
                json!({"operation":"runtimeSnapshot","instanceId":"missing"}),
                json!({"operation":"nodeMessages","instanceId":"missing","nodeId":"missing"}),
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
        let task = spawn_worker(wake.clone(), stopped.clone(), Duration::from_secs(60), scan);
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
        let (notifications, _) = mpsc::unbounded_channel();
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
            "executionVersion":"version", "content":"queued", "messages":[], "busyPolicy":"queue",
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
            .start_workflow_delivery_scheduler(mpsc::unbounded_channel().0)
            .is_err());
    }
}
