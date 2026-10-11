//! Real executor scheduling tests. Gates replace slow preparation without introducing model or
//! network dependencies; storage lease fencing itself is covered by disk admission regressions.
use super::*;
use std::sync::Condvar;

pub(super) struct PreparationBlocker {
    entered: AtomicBool,
    only_agent: Option<String>,
    released: Mutex<bool>,
    changed: Condvar,
}

impl PreparationBlocker {
    fn new() -> Arc<Self> {
        Self::for_agent(None)
    }

    fn for_agent(agent: Option<&str>) -> Arc<Self> {
        Arc::new(Self {
            entered: AtomicBool::new(false),
            only_agent: agent.map(str::to_string),
            released: Mutex::new(false),
            changed: Condvar::new(),
        })
    }

    pub(super) fn wait(&self, agent: &str) {
        if self
            .only_agent
            .as_deref()
            .is_some_and(|target| target != agent)
        {
            return;
        }
        self.entered.store(true, Ordering::Release);
        let released = self.released.lock().unwrap();
        // A finite fixture safety timeout makes a regression fail instead of hanging CI when a
        // synchronous start accidentally returns to the only Tokio scheduler thread.
        let (released, timeout) = self
            .changed
            .wait_timeout_while(released, Duration::from_secs(3), |released| !*released)
            .unwrap();
        assert!(
            *released && !timeout.timed_out(),
            "preparation was not released by async control"
        );
    }

    fn release(&self) {
        *self.released.lock().unwrap() = true;
        self.changed.notify_all();
    }
}

struct ReleasePreparation(Arc<PreparationBlocker>);
impl Drop for ReleasePreparation {
    fn drop(&mut self) {
        self.0.release();
    }
}

async fn wait_until(mut condition: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .expect("dispatcher fixture did not make progress");
}

fn config() -> AgentDispatcherConfig {
    AgentDispatcherConfig {
        global_concurrency_limit: 1,
        wake_lease_renew_interval: Duration::from_millis(5),
        idle_poll_interval: Duration::from_millis(5),
        shutdown_grace: Duration::from_millis(10),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn slow_preparation_and_maintenance_leave_scheduler_and_claim_renewal_available() {
    let store = Arc::new(FakeStore::default());
    store.push(1, "agent-a");
    let blocker = PreparationBlocker::new();
    let _release = ReleasePreparation(Arc::clone(&blocker));
    let mut executor = FakeExecutor::immediate();
    executor.start_blocker = Some(Arc::clone(&blocker));
    let executor = Arc::new(executor);
    let dispatcher = AgentDispatcher::start_with_clock(
        store.clone(),
        executor.clone(),
        Arc::new(TestClock::default()),
        AgentTurnConcurrencyGate::new(1).unwrap(),
        config(),
    )
    .unwrap();
    wait_until(|| blocker.entered.load(Ordering::Acquire)).await;
    let maintenance_blocker = PreparationBlocker::new();
    let _release_maintenance = ReleasePreparation(Arc::clone(&maintenance_blocker));
    let maintenance_worker = Arc::clone(&maintenance_blocker);
    let maintenance = dispatcher
        .shared
        .blocking_work
        .maintenance
        .submit(DISPATCH_MAINTENANCE_ESTIMATED_BYTES, move || {
            maintenance_worker.wait("maintenance")
        })
        .unwrap();
    wait_until(|| maintenance_blocker.entered.load(Ordering::Acquire)).await;
    let renewals = store.state.lock().unwrap().renewals;
    wait_until(|| store.state.lock().unwrap().renewals >= renewals + 2).await;
    assert!(executor.starts.lock().unwrap().is_empty());
    assert_eq!(dispatcher.shared.turn_concurrency.active(), 1);
    assert_eq!(
        store.get_wake("wake-1").unwrap().unwrap().status,
        AgentWakeStatus::Claimed
    );
    maintenance_blocker.release();
    maintenance.wait().await.unwrap();
    blocker.release();
    wait_for_settled(&store, 1).await;
    dispatcher.shutdown().await.unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn exhausted_preparation_capacity_does_not_claim_or_consume_logical_turns() {
    let work = BlockingWork::new().unwrap();
    let mut held = Vec::new();
    while let Ok(reservation) = work
        .preparation
        .try_reserve(DISPATCH_PREPARATION_ESTIMATED_BYTES)
    {
        held.push(reservation);
    }
    assert!(!held.is_empty());
    let store = Arc::new(FakeStore::default());
    store.push(1, "agent-a");
    let mut executor = FakeExecutor::immediate();
    executor.blocking_work = Some(work);
    let executor = Arc::new(executor);
    let gate = AgentTurnConcurrencyGate::new(1).unwrap();
    let dispatcher = AgentDispatcher::start_with_clock(
        store.clone(),
        executor.clone(),
        Arc::new(TestClock::default()),
        gate.clone(),
        config(),
    )
    .unwrap();
    wait_until(|| store.state.lock().unwrap().recoveries >= 2).await;
    assert_eq!(gate.active(), 0);
    assert!(store.state.lock().unwrap().claimed.is_empty());
    assert_eq!(
        store.get_wake("wake-1").unwrap().unwrap().status,
        AgentWakeStatus::Queued
    );
    drop(held);
    dispatcher.notify_work_available();
    wait_for_settled(&store, 1).await;
    dispatcher.shutdown().await.unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn lost_preparation_lease_cancels_late_handle_without_settling_another_owner() {
    let store = Arc::new(FakeStore::default());
    store.push(1, "agent-a");
    store.state.lock().unwrap().reject_renewal = true;
    let blocker = PreparationBlocker::new();
    let _release = ReleasePreparation(Arc::clone(&blocker));
    let mut executor = FakeExecutor::immediate();
    executor.start_blocker = Some(Arc::clone(&blocker));
    let executor = Arc::new(executor);
    let dispatcher = AgentDispatcher::start_with_clock(
        store.clone(),
        executor.clone(),
        Arc::new(TestClock::default()),
        AgentTurnConcurrencyGate::new(1).unwrap(),
        config(),
    )
    .unwrap();
    wait_until(|| blocker.entered.load(Ordering::Acquire)).await;
    wait_until(|| store.state.lock().unwrap().renewals > 0).await;
    // Let the independently scheduled lease owner receive the rejected maintenance result.
    tokio::time::sleep(Duration::from_millis(20)).await;
    mutate_wake(&store.state, "wake-1", |wake| {
        wake.claim_token = Some("replacement-owner".to_string());
        Ok(())
    })
    .unwrap();
    dispatcher.shared.running.lock().unwrap().insert(
        "agent-a".to_string(),
        RunningDispatch {
            wake_id: "wake-1".to_string(),
            claim_token: Some("replacement-owner".to_string()),
            run_id: Some("replacement-run".to_string()),
            waiting_for_approval: true,
        },
    );
    blocker.release();
    wait_until(|| !executor.interrupts.lock().unwrap().is_empty()).await;
    wait_until(|| dispatcher.shared.preparing.lock().unwrap().is_empty()).await;
    assert_eq!(
        dispatcher
            .shared
            .running
            .lock()
            .unwrap()
            .get("agent-a")
            .unwrap()
            .claim_token
            .as_deref(),
        Some("replacement-owner")
    );
    dispatcher.shared.running.lock().unwrap().clear();
    assert!(store.settled().is_empty());
    assert_eq!(
        store
            .get_wake("wake-1")
            .unwrap()
            .unwrap()
            .claim_token
            .as_deref(),
        Some("replacement-owner")
    );
    dispatcher.shutdown().await.unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn shutdown_waits_for_non_abortable_preparation_and_cancels_its_late_run() {
    let store = Arc::new(FakeStore::default());
    store.push(1, "agent-a");
    store.push(2, "agent-b");
    let blocker = PreparationBlocker::new();
    let _release = ReleasePreparation(Arc::clone(&blocker));
    let (mut executor, _gates) = FakeExecutor::gated();
    executor.start_blocker = Some(Arc::clone(&blocker));
    let executor = Arc::new(executor);
    let dispatcher = AgentDispatcher::start_with_clock(
        store.clone(),
        executor.clone(),
        Arc::new(TestClock::default()),
        AgentTurnConcurrencyGate::new(1).unwrap(),
        config(),
    )
    .unwrap();
    wait_until(|| blocker.entered.load(Ordering::Acquire)).await;
    let shared = Arc::clone(&dispatcher.shared);
    let shutdown = tokio::spawn(dispatcher.shutdown());
    wait_until(|| shared.shutdown_requested.load(Ordering::Acquire)).await;
    tokio::time::sleep(Duration::from_millis(40)).await;
    assert!(
        !shutdown.is_finished(),
        "shutdown detached a still-running synchronous admission"
    );
    assert!(store.state.lock().unwrap().renewals > 0);
    blocker.release();
    let report = tokio::time::timeout(Duration::from_secs(2), shutdown)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(!executor.interrupts.lock().unwrap().is_empty());
    assert_eq!(report.unfinished_persisted, 1);
    assert_eq!(store.state.lock().unwrap().claimed, vec!["agent-a"]);
    assert_eq!(
        store.get_wake("wake-2").unwrap().unwrap().status,
        AgentWakeStatus::Queued
    );
}

#[tokio::test(flavor = "current_thread")]
async fn fatal_manager_error_drains_started_preparation_before_returning() {
    let store = Arc::new(FakeStore::default());
    store.push(1, "agent-a");
    let blocker = PreparationBlocker::for_agent(Some("agent-b"));
    let _release = ReleasePreparation(Arc::clone(&blocker));
    let (mut executor, _gates) = FakeExecutor::gated();
    executor.start_blocker = Some(Arc::clone(&blocker));
    let executor = Arc::new(executor);
    let dispatcher = AgentDispatcher::start_with_clock(
        store.clone(),
        executor.clone(),
        Arc::new(TestClock::default()),
        AgentTurnConcurrencyGate::new(3).unwrap(),
        AgentDispatcherConfig {
            global_concurrency_limit: 3,
            ..config()
        },
    )
    .unwrap();
    wait_for_starts(&executor, 1).await;
    store.push(2, "agent-b");
    dispatcher.notify_work_available();
    wait_until(|| blocker.entered.load(Ordering::Acquire)).await;
    store.state.lock().unwrap().fatal_claim_after = Some(2);
    dispatcher.notify_work_available();
    wait_until(|| dispatcher.shared.shutdown_requested.load(Ordering::Acquire)).await;
    let shutdown = tokio::spawn(dispatcher.shutdown());
    tokio::time::sleep(Duration::from_millis(40)).await;
    assert!(
        !shutdown.is_finished(),
        "fatal manager exit detached started preparation"
    );
    assert!(store.state.lock().unwrap().renewals > 0);
    blocker.release();
    let result = tokio::time::timeout(Duration::from_secs(2), shutdown)
        .await
        .unwrap()
        .unwrap();
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("synthetic claim failure"));
    let interrupts = executor.interrupts.lock().unwrap();
    assert!(
        interrupts.contains(&"run:wake-1".to_string()),
        "fatal exit left an earlier active Run alive"
    );
    assert!(
        interrupts.contains(&"run:wake-2".to_string()),
        "fatal exit detached the late prepared Run"
    );
    assert!(store.settled().is_empty());
}

#[tokio::test]
async fn final_execution_refusal_stops_observation_renewal_and_same_claim_reattachment() {
    let fixture = tempfile::tempdir().unwrap();
    let storage = Arc::new(
        mycopilot_core::storage::service::StorageService::open_with_model_credentials(
            &fixture.path().join("execution-refusal.sqlite"),
            Arc::new(mycopilot_core::image_generation::InMemoryCredentialStore::default()),
        )
        .unwrap(),
    );
    let service = crate::application::agent::AgentService::try_new_deferred_startup_reconciliation_with_agent_limit(
        Arc::clone(&storage), None, 2,
    ).unwrap();
    let (notifications, _) = crate::transport::outbound_channel();
    let port =
        SharedAgentTurnExecutionPort::new(service.clone(), Arc::clone(&storage), notifications);
    let store = FakeStore::default();
    store.push(1, "agent-a");
    let mut wake = store
        .claim_next_dispatchable("old-generation", 1)
        .unwrap()
        .unwrap();
    wake.run_id = Some("same-run".into());
    wake.assistant_message_id = Some("same-assistant".into());
    let trusted = mycopilot_core::TrustedAgentWakeTurnAdmission {
        agent_id: wake.agent_id.clone(),
        wake_id: wake.wake_id.clone(),
        claim_token: "old-generation".into(),
        source_agent_message_id: "source".into(),
    };
    let mut handle = AgentWakeExecutionHandle {
        wake_id: wake.wake_id.clone(),
        claim_token: "old-generation".into(),
        run_id: "same-run".into(),
        conversation_id: "conversation:agent-a".into(),
        assistant_message_id: "same-assistant".into(),
    };
    service
        .record_fenced_agent_wake_execution(&trusted, &handle.run_id, &handle.assistant_message_id)
        .unwrap();
    assert!(port
        .inspect_durable(&handle, false)
        .unwrap_err()
        .contains("refused"));
    assert!(port
        .renew_claim_lease(&store, &wake.wake_id, "old-generation", 2)
        .is_err());
    assert_eq!(store.state.lock().unwrap().renewals, 0);
    assert!(port
        .recovered_handle(&wake)
        .unwrap_err()
        .contains("reattached"));
    assert!(
        service
            .register_agent_wake_runtime_owner(
                &trusted,
                &handle.run_id,
                &handle.assistant_message_id,
                &mycopilot_core::AgentCancellationToken::new()
            )
            .is_err(),
        "ordinary registration must not reopen a refused generation"
    );
    service
        .record_fenced_agent_wake_execution(&trusted, &handle.run_id, &handle.assistant_message_id)
        .unwrap();
    handle.claim_token = "new-generation".into();
    assert!(
        port.inspect_durable(&handle, false).unwrap().is_none(),
        "a new generation is not fenced by the prior refusal"
    );
    port.reconcile_execution_fences().unwrap();
    assert!(!service.agent_wake_execution_refused(
        &wake.wake_id,
        "old-generation",
        "same-run",
        "same-assistant"
    ));
    service.shutdown_blocking_work().await;
}
