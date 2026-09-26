use super::*;
use mycopilot_core::storage::models::{ChatConversationRecord, ChatMessageRecord};
use mycopilot_core::{EnsureRootAgentInput, SendAgentMessageRequest};
use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicI64, AtomicUsize};
use tokio::sync::oneshot;

#[derive(Default)]
struct TestClock(AtomicI64);

impl AgentDispatcherClock for TestClock {
    fn now_ms(&self) -> i64 {
        self.0.fetch_add(1, Ordering::SeqCst)
    }
}

struct FakeWake {
    record: AgentWakeRequestRecord,
    terminal: bool,
}

#[derive(Default)]
struct FakeStoreState {
    wakes: VecDeque<FakeWake>,
    active_agents: HashSet<String>,
    settled: Vec<String>,
    claimed: Vec<String>,
    max_active: usize,
    recoveries: usize,
    recovery_not_before_ms: Option<i64>,
    recover_tree_stopped: bool,
    mark_wait_tree_stopped: bool,
    tree_stopped_settlement: Option<TreeStoppedWakeSettlement>,
    temporary_claim_failures: usize,
    interrupt_disposition: Option<AgentInterruptDisposition>,
    interrupt_dispatched: bool,
    interrupt_dispatch_marks: Vec<(String, String, i64)>,
    undispatched_interrupts: Vec<mycopilot_core::UndispatchedAgentInterrupt>,
}

#[derive(Default)]
struct FakeStore {
    state: Mutex<FakeStoreState>,
    settled_notify: Notify,
}

impl FakeStore {
    fn push(&self, sequence: u64, agent_id: impl Into<String>) {
        let agent_id = agent_id.into();
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .wakes
            .push_back(FakeWake {
                record: wake(sequence, &agent_id),
                terminal: false,
            });
    }

    fn settled(&self) -> Vec<String> {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .settled
            .clone()
    }

    fn set_interrupt_disposition(&self, disposition: AgentInterruptDisposition) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        state.interrupt_disposition = Some(disposition);
        state.interrupt_dispatched = false;
    }
}

impl AgentDispatcherStore for FakeStore {
    fn list_undispatched_interrupts(
        &self,
    ) -> Result<Vec<mycopilot_core::UndispatchedAgentInterrupt>, String> {
        Ok(self
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .undispatched_interrupts
            .clone())
    }

    fn recover_orphaned_wakes(
        &self,
        recovery_token: &str,
        now_ms: i64,
    ) -> Result<AgentWakeRecoveryBatch, String> {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        state.recoveries += 1;
        if state
            .recovery_not_before_ms
            .is_some_and(|deadline| now_ms >= deadline)
        {
            state.recovery_not_before_ms = None;
            let recover_tree_stopped = state.recover_tree_stopped;
            if let Some(candidate) = state.wakes.iter_mut().find(|candidate| {
                !candidate.terminal
                    && matches!(
                        candidate.record.status,
                        AgentWakeStatus::Running | AgentWakeStatus::WaitingForApproval
                    )
            }) {
                candidate.record.claim_token = Some(recovery_token.to_string());
                candidate.record.lease_expires_at = Some(now_ms + 60_000);
                let recovered = candidate.record.clone();
                return Ok(AgentWakeRecoveryBatch {
                    requeued_before_dispatch: 0,
                    actions: vec![if recover_tree_stopped {
                        AgentWakeRecoveryAction::TreeStopped(recovered)
                    } else {
                        AgentWakeRecoveryAction::OutcomeUnknown(recovered)
                    }],
                });
            }
        }
        Ok(AgentWakeRecoveryBatch::default())
    }

    fn claim_next_dispatchable(
        &self,
        claim_token: &str,
        now_ms: i64,
    ) -> Result<Option<AgentWakeRequestRecord>, AgentDispatchClaimError> {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if state.temporary_claim_failures > 0 {
            state.temporary_claim_failures -= 1;
            return Err(AgentDispatchClaimError::NotReady(
                "temporary Mailbox projection contention".to_string(),
            ));
        }
        let Some(index) = state.wakes.iter().position(|candidate| {
            !candidate.terminal
                && candidate.record.status == AgentWakeStatus::Queued
                && !state.active_agents.contains(&candidate.record.agent_id)
        }) else {
            return Ok(None);
        };
        let agent_id = state.wakes[index].record.agent_id.clone();
        state.claimed.push(agent_id.clone());
        state.active_agents.insert(agent_id);
        state.max_active = state.max_active.max(state.active_agents.len());
        let record = &mut state.wakes[index].record;
        record.status = AgentWakeStatus::Claimed;
        record.claim_token = Some(claim_token.to_string());
        record.claimed_at = Some(now_ms);
        record.lease_expires_at = Some(now_ms + 60_000);
        Ok(Some(record.clone()))
    }

    fn get_wake(&self, wake_id: &str) -> Result<Option<AgentWakeRequestRecord>, String> {
        Ok(self
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .wakes
            .iter()
            .find(|candidate| candidate.record.wake_id == wake_id)
            .map(|candidate| candidate.record.clone()))
    }

    fn mark_waiting_for_approval(
        &self,
        wake_id: &str,
        _run_id: &str,
        _conversation_id: &str,
        _assistant_message_id: &str,
        _claim_token: &str,
        _now_ms: i64,
    ) -> Result<AgentWakeApprovalWaitOutcome, String> {
        let tree_stopped = self
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .mark_wait_tree_stopped;
        if tree_stopped {
            return self
                .get_wake(wake_id)?
                .ok_or_else(|| "missing Wake".to_string())
                .map(AgentWakeApprovalWaitOutcome::TreeStopped);
        }
        mutate_wake(&self.state, wake_id, |wake| {
            wake.status = AgentWakeStatus::WaitingForApproval;
            Ok(())
        })
        .map(AgentWakeApprovalWaitOutcome::Waiting)
    }

    fn renew_lease(&self, _wake_id: &str, _claim_token: &str, _now_ms: i64) -> Result<(), String> {
        Ok(())
    }

    fn settle(&self, settlement: &AgentWakeSettlement, _now_ms: i64) -> Result<(), String> {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        let index = state
            .wakes
            .iter()
            .position(|candidate| candidate.record.wake_id == settlement.wake_id)
            .ok_or_else(|| "missing Wake".to_string())?;
        let agent_id = state.wakes[index].record.agent_id.clone();
        state.wakes[index].record.status = settlement.terminal_status;
        state.wakes[index].terminal = true;
        state.active_agents.remove(&agent_id);
        state.settled.push(settlement.wake_id.clone());
        self.settled_notify.notify_waiters();
        Ok(())
    }

    fn settle_tree_stopped_wake(
        &self,
        wake: &mycopilot_core::ActiveAgentTreeWake,
        now_ms: i64,
    ) -> Result<TreeStoppedWakeSettlement, String> {
        if let Some(outcome) = self
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .tree_stopped_settlement
        {
            if outcome != TreeStoppedWakeSettlement::Interrupted {
                return Ok(outcome);
            }
        }
        self.settle(
            &AgentWakeSettlement {
                wake_id: wake.wake_id.clone(),
                expected_status: wake.status,
                claim_token: wake.claim_token.clone(),
                terminal_status: AgentWakeStatus::Interrupted,
                run_id: Some(wake.run_id.clone()),
                assistant_message_id: Some(wake.assistant_message_id.clone()),
                artifact_refs: Vec::new(),
                terminal_error: Some("stopped tree".to_string()),
            },
            now_ms,
        )?;
        Ok(TreeStoppedWakeSettlement::Interrupted)
    }

    fn interrupt_descendant(
        &self,
        _caller_agent_id: &str,
        _target_agent_id: &str,
        _request_id: &str,
        _now_ms: i64,
    ) -> Result<AgentInterruptRequest, String> {
        let state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        let disposition = state
            .interrupt_disposition
            .clone()
            .unwrap_or(AgentInterruptDisposition::NoPendingExecution);
        let requires_runtime_dispatch = !state.interrupt_dispatched
            && matches!(&disposition, AgentInterruptDisposition::ActiveTurn { .. });
        Ok(AgentInterruptRequest {
            disposition,
            requires_runtime_dispatch,
        })
    }

    fn mark_interrupt_dispatched(
        &self,
        caller_agent_id: &str,
        request_id: &str,
        now_ms: i64,
    ) -> Result<(), String> {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        state.interrupt_dispatched = true;
        state.interrupt_dispatch_marks.push((
            caller_agent_id.to_string(),
            request_id.to_string(),
            now_ms,
        ));
        state.undispatched_interrupts.retain(|interrupt| {
            interrupt.caller_agent_id != caller_agent_id || interrupt.request_id != request_id
        });
        Ok(())
    }
}

fn find_wake(
    state: &Mutex<FakeStoreState>,
    wake_id: &str,
) -> Result<AgentWakeRequestRecord, String> {
    state
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .wakes
        .iter()
        .find(|candidate| candidate.record.wake_id == wake_id)
        .map(|candidate| candidate.record.clone())
        .ok_or_else(|| "missing Wake".to_string())
}

fn mutate_wake(
    state: &Mutex<FakeStoreState>,
    wake_id: &str,
    mutate: impl FnOnce(&mut AgentWakeRequestRecord) -> Result<(), String>,
) -> Result<AgentWakeRequestRecord, String> {
    let mut state = state.lock().unwrap_or_else(|error| error.into_inner());
    let record = state
        .wakes
        .iter_mut()
        .find(|candidate| candidate.record.wake_id == wake_id)
        .ok_or_else(|| "missing Wake".to_string())?;
    mutate(&mut record.record)?;
    Ok(record.record.clone())
}

struct FakeExecutor {
    active: AtomicUsize,
    max_active: AtomicUsize,
    starts: Mutex<Vec<String>>,
    gates: Mutex<HashMap<String, oneshot::Receiver<()>>>,
    permits: Mutex<HashMap<String, AgentTurnConcurrencyPermit>>,
    interrupts: Mutex<Vec<String>>,
    interrupt_result: Mutex<AgentRunCancellationOutcome>,
    start_notify: Notify,
}

impl FakeExecutor {
    fn immediate() -> Self {
        Self {
            active: AtomicUsize::new(0),
            max_active: AtomicUsize::new(0),
            starts: Mutex::new(Vec::new()),
            gates: Mutex::new(HashMap::new()),
            permits: Mutex::new(HashMap::new()),
            interrupts: Mutex::new(Vec::new()),
            interrupt_result: Mutex::new(AgentRunCancellationOutcome::TurnTerminationConfirmed),
            start_notify: Notify::new(),
        }
    }

    fn set_interrupt_result(&self, outcome: AgentRunCancellationOutcome) {
        *self
            .interrupt_result
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = outcome;
    }

    fn gated() -> (Self, HashMap<String, oneshot::Sender<()>>) {
        Self::gated_agents(["agent-a", "agent-b", "agent-c"])
    }

    fn gated_agents(
        agents: impl IntoIterator<Item = impl Into<String>>,
    ) -> (Self, HashMap<String, oneshot::Sender<()>>) {
        let mut receivers = HashMap::new();
        let mut senders = HashMap::new();
        for agent in agents {
            let agent = agent.into();
            let (sender, receiver) = oneshot::channel();
            senders.insert(agent.clone(), sender);
            receivers.insert(agent, receiver);
        }
        (
            Self {
                active: AtomicUsize::new(0),
                max_active: AtomicUsize::new(0),
                starts: Mutex::new(Vec::new()),
                gates: Mutex::new(receivers),
                permits: Mutex::new(HashMap::new()),
                interrupts: Mutex::new(Vec::new()),
                interrupt_result: Mutex::new(AgentRunCancellationOutcome::TurnTerminationConfirmed),
                start_notify: Notify::new(),
            },
            senders,
        )
    }
}

impl AgentWakeTurnExecutionPort for FakeExecutor {
    fn start(
        &self,
        wake: &AgentWakeRequestRecord,
        permit: AgentTurnConcurrencyPermit,
    ) -> Result<AgentWakeExecutionHandle, String> {
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_active.fetch_max(active, Ordering::SeqCst);
        self.permits
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(format!("run:{}", wake.wake_id), permit);
        // Publish the deterministic test barrier only after every fact asserted by a waiter
        // is visible. Otherwise `wait_for_starts(2)` can observe the second vector entry
        // between that write and the corresponding active-count update.
        self.starts
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(wake.agent_id.clone());
        self.start_notify.notify_waiters();
        Ok(AgentWakeExecutionHandle {
            wake_id: wake.wake_id.clone(),
            run_id: format!("run:{}", wake.wake_id),
            conversation_id: format!("conversation:{}", wake.agent_id),
            assistant_message_id: format!("assistant:{}", wake.wake_id),
        })
    }

    fn observe<'a>(
        &'a self,
        handle: &'a AgentWakeExecutionHandle,
        _waiting_already_reported: bool,
    ) -> DispatchFuture<'a, Result<AgentWakeExecutionObservation, String>> {
        Box::pin(async move {
            let agent_id = handle
                .conversation_id
                .strip_prefix("conversation:")
                .unwrap_or(&handle.conversation_id)
                .to_string();
            let gate = self
                .gates
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .remove(&agent_id);
            if let Some(gate) = gate {
                let _ = gate.await;
            }
            self.active.fetch_sub(1, Ordering::SeqCst);
            self.permits
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .remove(&handle.run_id);
            Ok(AgentWakeExecutionObservation::Terminal {
                status: AgentWakeStatus::Completed,
                artifact_refs: Vec::new(),
                terminal_error: None,
            })
        })
    }

    fn recovered_handle(
        &self,
        wake: &AgentWakeRequestRecord,
    ) -> Result<AgentWakeExecutionHandle, String> {
        Ok(AgentWakeExecutionHandle {
            wake_id: wake.wake_id.clone(),
            run_id: wake
                .run_id
                .clone()
                .ok_or_else(|| "missing recovered run".to_string())?,
            conversation_id: format!("conversation:{}", wake.agent_id),
            assistant_message_id: wake
                .assistant_message_id
                .clone()
                .ok_or_else(|| "missing recovered assistant".to_string())?,
        })
    }

    fn retain_recovered_permit(&self, run_id: &str) -> Option<AgentTurnConcurrencyPermit> {
        self.permits
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(run_id)
            .cloned()
    }

    fn retire_recovered_turn_after_settlement(&self, handle: &AgentWakeExecutionHandle) {
        self.permits
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&handle.run_id);
    }

    fn interrupt(&self, run_id: &str) -> Result<AgentRunCancellationOutcome, String> {
        self.interrupts
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(run_id.to_string());
        Ok(*self
            .interrupt_result
            .lock()
            .unwrap_or_else(|error| error.into_inner()))
    }
}

fn wake(sequence: u64, agent_id: &str) -> AgentWakeRequestRecord {
    AgentWakeRequestRecord {
        sequence,
        wake_id: format!("wake-{sequence}"),
        root_agent_id: "root".to_string(),
        agent_id: agent_id.to_string(),
        requester_agent_id: "root".to_string(),
        request_id: format!("request-{sequence}"),
        source_agent_message_id: Some(format!("message-{sequence}")),
        status: AgentWakeStatus::Queued,
        status_revision: 1,
        claim_token: None,
        lease_expires_at: None,
        result_message_id: None,
        terminal_error: None,
        run_id: None,
        assistant_message_id: None,
        created_at: sequence as i64,
        claimed_at: None,
        started_at: None,
        completed_at: None,
    }
}

async fn wait_for_settled(store: &FakeStore, count: usize) {
    loop {
        if store.settled().len() >= count {
            return;
        }
        let notified = store.settled_notify.notified();
        tokio::pin!(notified);
        if store.settled().len() >= count {
            return;
        }
        notified.await;
    }
}

async fn wait_for_starts(executor: &FakeExecutor, count: usize) {
    loop {
        if executor.starts.lock().unwrap().len() >= count {
            return;
        }
        let notified = executor.start_notify.notified();
        tokio::pin!(notified);
        if executor.starts.lock().unwrap().len() >= count {
            return;
        }
        notified.await;
    }
}

fn active_interrupt_store(status: AgentWakeStatus) -> Arc<FakeStore> {
    let store = Arc::new(FakeStore::default());
    store.push(1, "agent-a");
    mutate_wake(&store.state, "wake-1", |wake| {
        wake.status = status;
        wake.run_id = Some("run:wake-1".to_string());
        Ok(())
    })
    .unwrap();
    store.set_interrupt_disposition(AgentInterruptDisposition::ActiveTurn {
        wake_id: "wake-1".to_string(),
        run_id: "run:wake-1".to_string(),
    });
    store
}

fn start_interrupt_test_dispatcher(
    store: Arc<FakeStore>,
    executor: Arc<FakeExecutor>,
) -> AgentDispatcher {
    AgentDispatcher::start_with_clock(
        store,
        executor,
        Arc::new(TestClock::default()),
        AgentTurnConcurrencyGate::new(1).unwrap(),
        AgentDispatcherConfig {
            global_concurrency_limit: 1,
            idle_poll_interval: Duration::from_secs(60),
            ..AgentDispatcherConfig::default()
        },
    )
    .unwrap()
}

#[tokio::test]
async fn interrupt_agent_reports_active_turn_only_after_runtime_delivery() {
    let store = active_interrupt_store(AgentWakeStatus::Running);
    let executor = Arc::new(FakeExecutor::immediate());
    let dispatcher = start_interrupt_test_dispatcher(Arc::clone(&store), Arc::clone(&executor));

    let disposition = dispatcher
        .interrupt_agent("root", "agent-a", "interrupt-1")
        .unwrap();

    assert_eq!(
        disposition,
        AgentInterruptDisposition::ActiveTurn {
            wake_id: "wake-1".to_string(),
            run_id: "run:wake-1".to_string(),
        }
    );
    assert_eq!(
        executor.interrupts.lock().unwrap().as_slice(),
        ["run:wake-1"]
    );
    {
        let state = store.state.lock().unwrap();
        assert!(state.interrupt_dispatched);
        assert_eq!(state.interrupt_dispatch_marks.len(), 1);
        assert_eq!(state.interrupt_dispatch_marks[0].0, "root");
        assert_eq!(state.interrupt_dispatch_marks[0].1, "interrupt-1");
    }

    let retry = dispatcher
        .interrupt_agent("root", "agent-a", "interrupt-1")
        .unwrap();
    assert_eq!(retry, disposition);
    assert_eq!(
        executor.interrupts.lock().unwrap().as_slice(),
        ["run:wake-1"],
        "a durably dispatched request must not hit the executor twice"
    );
    assert_eq!(
        store.state.lock().unwrap().interrupt_dispatch_marks.len(),
        1
    );
    dispatcher.shutdown().await.unwrap();
}

#[tokio::test]
async fn interrupt_agent_runtime_miss_is_an_error_while_wake_remains_active() {
    let store = active_interrupt_store(AgentWakeStatus::Running);
    let executor = Arc::new(FakeExecutor::immediate());
    executor.set_interrupt_result(AgentRunCancellationOutcome::NoEffect);
    let dispatcher = start_interrupt_test_dispatcher(Arc::clone(&store), Arc::clone(&executor));

    let error = dispatcher
        .interrupt_agent("root", "agent-a", "interrupt-1")
        .unwrap_err();

    assert!(matches!(error, AgentDispatcherError::Manager(message) if
        message.contains("interrupt did not reach active Turn run:wake-1")
            && message.contains("durable Wake status remains running")));
    assert_eq!(
        executor.interrupts.lock().unwrap().as_slice(),
        ["run:wake-1"]
    );
    assert!(!store.state.lock().unwrap().interrupt_dispatched);

    executor.set_interrupt_result(AgentRunCancellationOutcome::TurnTerminationConfirmed);
    let retry = dispatcher
        .interrupt_agent("root", "agent-a", "interrupt-1")
        .unwrap();
    assert_eq!(
        retry,
        AgentInterruptDisposition::ActiveTurn {
            wake_id: "wake-1".to_string(),
            run_id: "run:wake-1".to_string(),
        }
    );
    assert_eq!(
        executor.interrupts.lock().unwrap().as_slice(),
        ["run:wake-1", "run:wake-1"],
        "an undispatched durable receipt must remain retryable"
    );
    assert!(store.state.lock().unwrap().interrupt_dispatched);
    dispatcher.shutdown().await.unwrap();
}

#[tokio::test]
async fn interrupt_agent_does_not_acknowledge_resource_cleanup_without_turn_termination() {
    let store = active_interrupt_store(AgentWakeStatus::Running);
    let executor = Arc::new(FakeExecutor::immediate());
    executor.set_interrupt_result(AgentRunCancellationOutcome::ResourcesOnly);
    let dispatcher = start_interrupt_test_dispatcher(Arc::clone(&store), Arc::clone(&executor));

    let error = dispatcher
        .interrupt_agent("root", "agent-a", "interrupt-resources-only")
        .unwrap_err();

    assert!(matches!(error, AgentDispatcherError::Manager(message) if
        message.contains("interrupt did not reach active Turn run:wake-1")
            && message.contains("durable Wake status remains running")));
    assert_eq!(
        executor.interrupts.lock().unwrap().as_slice(),
        ["run:wake-1"]
    );
    assert!(
        !store.state.lock().unwrap().interrupt_dispatched,
        "resource cleanup alone must leave the durable interrupt receipt retryable"
    );
    dispatcher.shutdown().await.unwrap();
}

#[tokio::test]
async fn interrupt_agent_runtime_miss_reports_no_active_turn_after_durable_settlement() {
    let store = active_interrupt_store(AgentWakeStatus::Completed);
    let executor = Arc::new(FakeExecutor::immediate());
    executor.set_interrupt_result(AgentRunCancellationOutcome::NoEffect);
    let dispatcher = start_interrupt_test_dispatcher(Arc::clone(&store), Arc::clone(&executor));

    let disposition = dispatcher
        .interrupt_agent("root", "agent-a", "interrupt-1")
        .unwrap();

    assert_eq!(disposition, AgentInterruptDisposition::NoPendingExecution);
    assert_eq!(
        executor.interrupts.lock().unwrap().as_slice(),
        ["run:wake-1"]
    );
    assert!(!store.state.lock().unwrap().interrupt_dispatched);
    dispatcher.shutdown().await.unwrap();
}

#[tokio::test]
async fn startup_redelivers_an_unacknowledged_durable_interrupt() {
    let store = Arc::new(FakeStore::default());
    store
        .state
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .undispatched_interrupts
        .push(mycopilot_core::UndispatchedAgentInterrupt {
            caller_agent_id: "root".to_string(),
            target_agent_id: "agent-a".to_string(),
            request_id: "interrupt-before-crash".to_string(),
            wake_id: "wake-before-crash".to_string(),
            run_id: "run-before-crash".to_string(),
        });
    let executor = Arc::new(FakeExecutor::immediate());
    let dispatcher = AgentDispatcher::start_with_clock(
        store.clone(),
        executor.clone(),
        Arc::new(TestClock::default()),
        AgentTurnConcurrencyGate::new(1).unwrap(),
        AgentDispatcherConfig {
            global_concurrency_limit: 1,
            idle_poll_interval: Duration::from_millis(1),
            ..AgentDispatcherConfig::default()
        },
    )
    .unwrap();

    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if store
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .interrupt_dispatch_marks
                .len()
                == 1
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("startup must redeliver the durable interrupt receipt");
    assert_eq!(
        executor
            .interrupts
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_slice(),
        ["run-before-crash"]
    );
    assert!(store
        .state
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .undispatched_interrupts
        .is_empty());
    dispatcher.shutdown().await.unwrap();
}

struct ApprovalResumeExecutor {
    observation: Mutex<VecDeque<AgentWakeExecutionObservation>>,
    changed: Notify,
    interrupts: AtomicUsize,
}

impl ApprovalResumeExecutor {
    fn new() -> Self {
        Self {
            observation: Mutex::new(VecDeque::from([
                AgentWakeExecutionObservation::WaitingForApproval,
            ])),
            changed: Notify::new(),
            interrupts: AtomicUsize::new(0),
        }
    }

    fn resume(&self) {
        self.observation
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push_back(AgentWakeExecutionObservation::RunningAfterApproval);
        self.changed.notify_waiters();
    }
}

impl AgentWakeTurnExecutionPort for ApprovalResumeExecutor {
    fn start(
        &self,
        wake: &AgentWakeRequestRecord,
        _permit: AgentTurnConcurrencyPermit,
    ) -> Result<AgentWakeExecutionHandle, String> {
        Ok(AgentWakeExecutionHandle {
            wake_id: wake.wake_id.clone(),
            run_id: format!("run:{}", wake.wake_id),
            conversation_id: format!("conversation:{}", wake.agent_id),
            assistant_message_id: format!("assistant:{}", wake.wake_id),
        })
    }

    fn observe<'a>(
        &'a self,
        _handle: &'a AgentWakeExecutionHandle,
        _waiting_already_reported: bool,
    ) -> DispatchFuture<'a, Result<AgentWakeExecutionObservation, String>> {
        Box::pin(async move {
            loop {
                if let Some(next) = self
                    .observation
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .pop_front()
                {
                    return Ok(next);
                }
                let changed = self.changed.notified();
                tokio::pin!(changed);
                if let Some(next) = self
                    .observation
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .pop_front()
                {
                    return Ok(next);
                }
                changed.await;
            }
        })
    }

    fn recovered_handle(
        &self,
        _wake: &AgentWakeRequestRecord,
    ) -> Result<AgentWakeExecutionHandle, String> {
        Err("not a recovery fixture".to_string())
    }

    fn retain_recovered_permit(&self, _run_id: &str) -> Option<AgentTurnConcurrencyPermit> {
        None
    }

    fn retire_recovered_turn_after_settlement(&self, _handle: &AgentWakeExecutionHandle) {}

    fn interrupt(&self, _run_id: &str) -> Result<AgentRunCancellationOutcome, String> {
        self.interrupts.fetch_add(1, Ordering::SeqCst);
        Ok(AgentRunCancellationOutcome::TurnTerminationConfirmed)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn approval_resume_clears_waiting_before_shutdown_cancels_the_live_continuation() {
    let store = Arc::new(FakeStore::default());
    store.push(1, "agent-a");
    let executor = Arc::new(ApprovalResumeExecutor::new());
    let dispatcher = AgentDispatcher::start_with_clock(
        store.clone(),
        executor.clone(),
        Arc::new(TestClock::default()),
        AgentTurnConcurrencyGate::new(1).unwrap(),
        AgentDispatcherConfig {
            global_concurrency_limit: 1,
            wake_lease_renew_interval: Duration::from_secs(60),
            idle_poll_interval: Duration::from_secs(60),
            shutdown_grace: Duration::from_millis(5),
        },
    )
    .unwrap();
    dispatcher.notify_work_available();
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if store
                .get_wake("wake-1")
                .unwrap()
                .is_some_and(|wake| wake.status == AgentWakeStatus::WaitingForApproval)
            {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    mutate_wake(&store.state, "wake-1", |wake| {
        wake.status = AgentWakeStatus::Running;
        Ok(())
    })
    .unwrap();
    executor.resume();
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let resumed = dispatcher
                .shared
                .running
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .get("agent-a")
                .is_some_and(|running| !running.waiting_for_approval);
            if resumed {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();

    let report = dispatcher.shutdown().await.unwrap();
    assert_eq!(report.cancellation_requested, 1);
    assert_eq!(executor.interrupts.load(Ordering::SeqCst), 1);
    assert_eq!(
        store.get_wake("wake-1").unwrap().unwrap().status,
        AgentWakeStatus::Running,
        "shutdown must preserve the durable active Wake for restart recovery"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stop_winning_approval_wait_cas_settles_immediately_without_lease_recovery() {
    let store = Arc::new(FakeStore::default());
    store.push(1, "agent-a");
    store
        .state
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .mark_wait_tree_stopped = true;
    let executor = Arc::new(ApprovalResumeExecutor::new());
    let dispatcher = AgentDispatcher::start_with_clock(
        store.clone(),
        executor.clone(),
        Arc::new(TestClock::default()),
        AgentTurnConcurrencyGate::new(1).unwrap(),
        AgentDispatcherConfig {
            global_concurrency_limit: 1,
            wake_lease_renew_interval: Duration::from_secs(60),
            idle_poll_interval: Duration::from_secs(60),
            shutdown_grace: Duration::from_millis(5),
        },
    )
    .unwrap();
    dispatcher.notify_work_available();

    tokio::time::timeout(Duration::from_secs(1), wait_for_settled(&store, 1))
        .await
        .expect("a stop-first approval race must settle without waiting for lease recovery");
    assert_eq!(
        store.get_wake("wake-1").unwrap().unwrap().status,
        AgentWakeStatus::Interrupted
    );
    assert_eq!(executor.interrupts.load(Ordering::SeqCst), 1);
    dispatcher.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_rescans_an_approval_that_resumes_after_its_first_snapshot() {
    let store = Arc::new(FakeStore::default());
    store.push(1, "agent-a");
    let executor = Arc::new(ApprovalResumeExecutor::new());
    let dispatcher = AgentDispatcher::start_with_clock(
        store.clone(),
        executor.clone(),
        Arc::new(TestClock::default()),
        AgentTurnConcurrencyGate::new(1).unwrap(),
        AgentDispatcherConfig {
            global_concurrency_limit: 1,
            wake_lease_renew_interval: Duration::from_secs(60),
            idle_poll_interval: Duration::from_millis(1),
            shutdown_grace: Duration::from_millis(20),
        },
    )
    .unwrap();
    dispatcher.notify_work_available();
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if dispatcher.observed_waiting_for_approval("agent-a") == Some(true) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();

    let shutdown_shared = Arc::clone(&dispatcher.shared);
    let shutdown = tokio::spawn(dispatcher.shutdown());
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if shutdown_shared.shutdown_requested.load(Ordering::Acquire) {
                // Wait beyond the first grace so its initial cancellation snapshot has
                // definitely preserved this still-waiting approval.
                tokio::time::sleep(Duration::from_millis(25)).await;
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    mutate_wake(&store.state, "wake-1", |wake| {
        wake.status = AgentWakeStatus::Running;
        Ok(())
    })
    .unwrap();
    executor.resume();

    let report = shutdown.await.unwrap().unwrap();
    assert_eq!(report.cancellation_requested, 1);
    assert_eq!(executor.interrupts.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_hundred_wakes_for_one_agent_never_overlap_and_all_settle() {
    let store = Arc::new(FakeStore::default());
    for sequence in 1..=100 {
        store.push(sequence, "agent-a");
    }
    let executor = Arc::new(FakeExecutor::immediate());
    let dispatcher = AgentDispatcher::start_with_clock(
        store.clone(),
        executor.clone(),
        Arc::new(TestClock::default()),
        AgentTurnConcurrencyGate::new(8).unwrap(),
        AgentDispatcherConfig {
            global_concurrency_limit: 8,
            idle_poll_interval: Duration::from_secs(60),
            ..AgentDispatcherConfig::default()
        },
    )
    .unwrap();
    dispatcher.notify_work_available();
    wait_for_settled(&store, 100).await;
    assert_eq!(executor.max_active.load(Ordering::SeqCst), 1);
    assert_eq!(
        store
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .max_active,
        1
    );
    dispatcher.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn global_limit_and_fifo_make_progress_across_agents() {
    let store = Arc::new(FakeStore::default());
    store.push(1, "agent-a");
    store.push(2, "agent-b");
    store.push(3, "agent-c");
    let (executor, mut gates) = FakeExecutor::gated();
    let executor = Arc::new(executor);
    let dispatcher = AgentDispatcher::start_with_clock(
        store.clone(),
        executor.clone(),
        Arc::new(TestClock::default()),
        AgentTurnConcurrencyGate::new(2).unwrap(),
        AgentDispatcherConfig {
            global_concurrency_limit: 2,
            idle_poll_interval: Duration::from_secs(60),
            ..AgentDispatcherConfig::default()
        },
    )
    .unwrap();
    dispatcher.notify_work_available();
    wait_for_starts(&executor, 2).await;
    assert_eq!(
        store.state.lock().unwrap().claimed.as_slice(),
        ["agent-a", "agent-b"],
        "durable admission is FIFO even if spawned workers begin in either order"
    );
    assert_eq!(executor.max_active.load(Ordering::SeqCst), 2);

    gates.remove("agent-a").unwrap().send(()).unwrap();
    wait_for_starts(&executor, 3).await;
    assert_eq!(executor.starts.lock().unwrap()[2], "agent-c");
    gates.remove("agent-b").unwrap().send(()).unwrap();
    gates.remove("agent-c").unwrap().send(()).unwrap();
    wait_for_settled(&store, 3).await;
    dispatcher.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_hundred_agents_respect_limit_fifty_fifo_and_leave_no_ghost_activity() {
    const AGENT_COUNT: usize = 100;
    const GLOBAL_LIMIT: usize = DEFAULT_AGENT_GLOBAL_CONCURRENCY;

    let store = Arc::new(FakeStore::default());
    let agents = (0..AGENT_COUNT)
        .map(|index| format!("agent-{index:02}"))
        .collect::<Vec<_>>();
    for (index, agent_id) in agents.iter().enumerate() {
        store.push(u64::try_from(index + 1).unwrap(), agent_id);
    }
    let (executor, mut gates) = FakeExecutor::gated_agents(agents.iter().cloned());
    let executor = Arc::new(executor);
    let dispatcher = AgentDispatcher::start_with_clock(
        store.clone(),
        executor.clone(),
        Arc::new(TestClock::default()),
        AgentTurnConcurrencyGate::new(GLOBAL_LIMIT).unwrap(),
        AgentDispatcherConfig {
            global_concurrency_limit: GLOBAL_LIMIT,
            idle_poll_interval: Duration::from_secs(60),
            ..AgentDispatcherConfig::default()
        },
    )
    .unwrap();
    dispatcher.notify_work_available();

    for completed_before_wave in (0..AGENT_COUNT).step_by(GLOBAL_LIMIT) {
        let admitted = completed_before_wave + GLOBAL_LIMIT;
        wait_for_starts(&executor, admitted).await;
        let state = store
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        assert_eq!(
            state.claimed.as_slice(),
            &agents[..admitted],
            "durable claims must remain FIFO at every concurrency wave"
        );
        assert_eq!(state.active_agents.len(), GLOBAL_LIMIT);
        drop(state);
        for agent_id in &agents[completed_before_wave..admitted] {
            gates.remove(agent_id).unwrap().send(()).unwrap();
        }
    }

    wait_for_settled(&store, AGENT_COUNT).await;
    assert_eq!(executor.max_active.load(Ordering::SeqCst), GLOBAL_LIMIT);
    assert_eq!(executor.active.load(Ordering::SeqCst), 0);
    {
        let state = store
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        assert_eq!(state.settled.len(), AGENT_COUNT);
        assert!(state.active_agents.is_empty());
    }
    let turn_concurrency = dispatcher.shared.turn_concurrency.clone();
    dispatcher.shutdown().await.unwrap();
    assert_eq!(turn_concurrency.active(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn temporary_candidate_projection_conflict_does_not_stop_the_manager() {
    let store = Arc::new(FakeStore::default());
    store.push(1, "agent-a");
    store.push(2, "agent-b");
    store
        .state
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .temporary_claim_failures = 1;
    let executor = Arc::new(FakeExecutor::immediate());
    let dispatcher = AgentDispatcher::start_with_clock(
        store.clone(),
        executor.clone(),
        Arc::new(TestClock::default()),
        AgentTurnConcurrencyGate::new(2).unwrap(),
        AgentDispatcherConfig {
            global_concurrency_limit: 2,
            idle_poll_interval: Duration::from_millis(1),
            ..AgentDispatcherConfig::default()
        },
    )
    .unwrap();
    dispatcher.notify_work_available();
    tokio::time::timeout(Duration::from_secs(1), wait_for_settled(&store, 2))
        .await
        .expect("a candidate-level conflict must not terminate the Dispatcher");
    assert_eq!(executor.starts.lock().unwrap().len(), 2);
    dispatcher.shutdown().await.unwrap();
}

#[test]
fn cloned_permit_holds_slot_until_result_settlement_owner_drops() {
    let gate = AgentTurnConcurrencyGate::new(1).unwrap();
    let service_owner = gate.try_acquire().unwrap();
    let settlement_owner = service_owner.clone();
    drop(service_owner);
    assert!(
        gate.try_acquire().is_err(),
        "terminal trace alone cannot free slot"
    );
    drop(settlement_owner);
    assert!(
        gate.try_acquire().is_ok(),
        "result commit releases final owner"
    );

    let recovered = gate.adopt_recovered();
    assert_eq!(
        gate.active(),
        1,
        "recovered occupancy counts against the same gate"
    );
    assert!(gate.try_acquire().is_err());
    drop(recovered);
}

#[test]
fn empty_queue_probe_reserves_capacity_without_reporting_a_logical_turn() {
    let gate = AgentTurnConcurrencyGate::new(1).unwrap();
    let reservation = gate.try_reserve_for_dispatch().unwrap();
    assert_eq!(gate.active(), 0, "a claim probe is not an Agent Turn");
    assert!(
        gate.try_acquire().is_err(),
        "the reservation must still fence a concurrent root admission"
    );

    reservation.activate_dispatch_reservation();
    assert_eq!(gate.active(), 1, "a claimed Wake becomes one active Turn");
    drop(reservation);
    assert_eq!(gate.active(), 0);
}

#[test]
fn stale_empty_scan_cannot_overwrite_a_new_work_generation() {
    let store: Arc<dyn AgentDispatcherStore> = Arc::new(FakeStore::default());
    let executor: Arc<dyn AgentWakeTurnExecutionPort> = Arc::new(FakeExecutor::immediate());
    let shared = Arc::new(AgentDispatcherShared {
        store,
        executor,
        clock: Arc::new(TestClock::default()),
        turn_concurrency: AgentTurnConcurrencyGate::new(1).unwrap(),
        work_available: Notify::new(),
        shutdown_requested: AtomicBool::new(false),
        running: Mutex::new(HashMap::new()),
        quiescence: Mutex::new(AgentDispatcherQuiescence::default()),
        quiescence_changed: Notify::new(),
        config: AgentDispatcherConfig {
            global_concurrency_limit: 1,
            ..AgentDispatcherConfig::default()
        },
    });

    let empty_scan_generation = begin_dispatcher_scan(&shared);
    let scan_has_read_empty = std::sync::Barrier::new(2);
    let notification_has_committed = std::sync::Barrier::new(2);
    std::thread::scope(|scope| {
        let notification_shared = Arc::clone(&shared);
        let scan_has_read_empty = &scan_has_read_empty;
        let notification_has_committed = &notification_has_committed;
        scope.spawn(move || {
            scan_has_read_empty.wait();
            notify_dispatcher_work_available(&notification_shared);
            notification_has_committed.wait();
        });

        // This is the exact empty-claim / publish gap: the scan has already read None, but
        // cannot publish idle until the concurrent committed-work notification completes.
        scan_has_read_empty.wait();
        notification_has_committed.wait();
        finish_dispatcher_scan(&shared, empty_scan_generation, true);
    });

    let state = shared
        .quiescence
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    assert!(!state.scan_in_progress);
    assert!(!state.durable_queue_idle);
    assert_eq!(state.work_generation, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn manager_repeats_recovery_after_a_fresh_restart_lease_expires() {
    let store = Arc::new(FakeStore::default());
    store.push(1, "agent-a");
    {
        let mut state = store.state.lock().unwrap();
        let wake = &mut state.wakes[0].record;
        wake.status = AgentWakeStatus::Running;
        wake.claim_token = Some("old-host".to_string());
        wake.claimed_at = Some(1);
        wake.started_at = Some(2);
        wake.lease_expires_at = Some(50);
        wake.run_id = Some("run:wake-1".to_string());
        wake.assistant_message_id = Some("assistant:wake-1".to_string());
        state.active_agents.insert("agent-a".to_string());
        state.recovery_not_before_ms = Some(50);
    }
    let executor = Arc::new(FakeExecutor::immediate());
    let dispatcher = AgentDispatcher::start_with_clock(
        store.clone(),
        executor,
        Arc::new(TestClock::default()),
        AgentTurnConcurrencyGate::new(1).unwrap(),
        AgentDispatcherConfig {
            global_concurrency_limit: 1,
            idle_poll_interval: Duration::from_millis(1),
            ..AgentDispatcherConfig::default()
        },
    )
    .unwrap();
    tokio::time::timeout(Duration::from_secs(1), wait_for_settled(&store, 1))
        .await
        .expect("periodic recovery must reach the later lease deadline");
    assert!(store.state.lock().unwrap().recoveries > 1);
    assert_eq!(
        store.state.lock().unwrap().wakes[0].record.status,
        AgentWakeStatus::OutcomeUnknown
    );
    dispatcher.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stopped_tree_recovery_interrupts_local_approval_state_and_never_replays() {
    let store = Arc::new(FakeStore::default());
    store.push(1, "agent-a");
    {
        let mut state = store.state.lock().unwrap();
        let wake = &mut state.wakes[0].record;
        wake.status = AgentWakeStatus::WaitingForApproval;
        wake.claim_token = Some("old-host".to_string());
        wake.claimed_at = Some(1);
        wake.started_at = Some(2);
        wake.lease_expires_at = Some(3);
        wake.run_id = Some("run:wake-1".to_string());
        wake.assistant_message_id = Some("assistant:wake-1".to_string());
        state.active_agents.insert("agent-a".to_string());
        state.recovery_not_before_ms = Some(3);
        state.recover_tree_stopped = true;
    }
    let executor = Arc::new(FakeExecutor::immediate());
    let dispatcher = AgentDispatcher::start_with_clock(
        store.clone(),
        executor.clone(),
        Arc::new(TestClock::default()),
        AgentTurnConcurrencyGate::new(1).unwrap(),
        AgentDispatcherConfig {
            global_concurrency_limit: 1,
            idle_poll_interval: Duration::from_millis(1),
            ..AgentDispatcherConfig::default()
        },
    )
    .unwrap();

    tokio::time::timeout(Duration::from_secs(1), wait_for_settled(&store, 1))
        .await
        .expect("a durably stopped Wake must settle without being resumed");
    assert_eq!(
        store.state.lock().unwrap().wakes[0].record.status,
        AgentWakeStatus::Interrupted
    );
    assert_eq!(
        executor.interrupts.lock().unwrap().as_slice(),
        ["run:wake-1"]
    );
    assert!(executor.starts.lock().unwrap().is_empty());
    dispatcher.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stopped_tree_recovery_with_uncertain_external_action_becomes_outcome_unknown() {
    let store = Arc::new(FakeStore::default());
    store.push(1, "agent-a");
    {
        let mut state = store.state.lock().unwrap();
        let wake = &mut state.wakes[0].record;
        wake.status = AgentWakeStatus::WaitingForApproval;
        wake.claim_token = Some("old-host".to_string());
        wake.claimed_at = Some(1);
        wake.started_at = Some(2);
        wake.lease_expires_at = Some(3);
        wake.run_id = Some("run:wake-1".to_string());
        wake.assistant_message_id = Some("assistant:wake-1".to_string());
        state.active_agents.insert("agent-a".to_string());
        state.recovery_not_before_ms = Some(3);
        state.recover_tree_stopped = true;
        state.tree_stopped_settlement =
            Some(TreeStoppedWakeSettlement::UnsafeActiveAction { count: 1 });
    }
    let executor = Arc::new(FakeExecutor::immediate());
    let dispatcher = AgentDispatcher::start_with_clock(
        store.clone(),
        executor.clone(),
        Arc::new(TestClock::default()),
        AgentTurnConcurrencyGate::new(1).unwrap(),
        AgentDispatcherConfig {
            global_concurrency_limit: 1,
            idle_poll_interval: Duration::from_millis(1),
            ..AgentDispatcherConfig::default()
        },
    )
    .unwrap();

    tokio::time::timeout(Duration::from_secs(1), wait_for_settled(&store, 1))
        .await
        .expect("an uncertain external action must still converge to a safe terminal state");
    assert_eq!(
        store.state.lock().unwrap().wakes[0].record.status,
        AgentWakeStatus::OutcomeUnknown
    );
    assert_eq!(
        executor.interrupts.lock().unwrap().as_slice(),
        ["run:wake-1"]
    );
    assert!(executor.starts.lock().unwrap().is_empty());
    dispatcher.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_stops_claiming_and_leaves_unfinished_wake_durable() {
    let store = Arc::new(FakeStore::default());
    store.push(1, "agent-a");
    let (executor, _gates) = FakeExecutor::gated();
    let executor = Arc::new(executor);
    let dispatcher = AgentDispatcher::start_with_clock(
        store,
        executor.clone(),
        Arc::new(TestClock::default()),
        AgentTurnConcurrencyGate::new(1).unwrap(),
        AgentDispatcherConfig {
            global_concurrency_limit: 1,
            idle_poll_interval: Duration::from_secs(60),
            shutdown_grace: Duration::from_millis(2),
            ..AgentDispatcherConfig::default()
        },
    )
    .unwrap();
    wait_for_starts(&executor, 1).await;
    let report = dispatcher.shutdown().await.unwrap();
    assert_eq!(report.cancellation_requested, 1);
    assert_eq!(report.unfinished_persisted, 1);
}

fn empty_conversation(id: &str) -> ChatConversationRecord {
    ChatConversationRecord {
        id: id.to_string(),
        project_id: None,
        model_id: Some("model-a".to_string()),
        title: id.to_string(),
        messages: Vec::new(),
        created_at: 1,
        updated_at: 1,
        pinned_at: None,
        archived_at: None,
        unread_at: None,
    }
}

#[tokio::test]
async fn durable_terminal_observation_uses_trace_diagnostics_not_the_final_reply() {
    let fixture = tempfile::tempdir().unwrap();
    let storage = Arc::new(
        mycopilot_core::storage::service::StorageService::open(
            &fixture.path().join("terminal-observation.sqlite"),
        )
        .unwrap(),
    );
    let conversation_id = "terminal-observation-child";
    storage
        .save_conversation(empty_conversation(conversation_id))
        .unwrap();
    let service = crate::application::agent::AgentService::try_new_deferred_startup_reconciliation_with_agent_limit(
        Arc::clone(&storage), None, 2,
    ).unwrap();
    let (notifications, _) = tokio::sync::mpsc::unbounded_channel();
    let port = SharedAgentTurnExecutionPort::new(service, Arc::clone(&storage), notifications);
    for (index, (status, expected_status, error)) in [
        (
            ConversationTurnTraceTerminalStatus::Completed,
            AgentWakeStatus::Completed,
            None,
        ),
        (
            ConversationTurnTraceTerminalStatus::Failed,
            AgentWakeStatus::Failed,
            Some("  Provider request failed.  "),
        ),
        (
            ConversationTurnTraceTerminalStatus::Cancelled,
            AgentWakeStatus::Interrupted,
            Some("The user interrupted this run."),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let run_id = format!("terminal-observation-run-{index}");
        let assistant_message_id = format!("terminal-observation-message-{index}");
        storage
            .upsert_chat_messages(
                conversation_id,
                vec![ChatMessageRecord {
                    human_interaction_response: None,
                    id: assistant_message_id.clone(),
                    role: "assistant".to_string(),
                    content: "MODEL_FINAL_MUST_STAY_LOCAL".to_string(),
                    created_at: 2 + index as i64,
                    status: Some("sent".to_string()),
                    attachments: Vec::new(),
                    folder_references_json: None,
                    agent_run_json: None,
                    ui_state_json: None,
                }],
                index as i64,
            )
            .unwrap();
        let mut trace = mycopilot_core::completed_conversation_trace_without_items(
            &run_id,
            conversation_id,
            &assistant_message_id,
        );
        trace.terminal_status = status;
        trace.terminal_error = error.map(str::to_string);
        storage
            .replace_conversation_turn_trace(&trace, 2 + index as i64, 2 + index as i64)
            .unwrap();
        let observation = port
            .inspect_durable(
                &AgentWakeExecutionHandle {
                    wake_id: format!("terminal-observation-wake-{index}"),
                    run_id,
                    conversation_id: conversation_id.to_string(),
                    assistant_message_id,
                },
                false,
            )
            .unwrap()
            .unwrap();
        assert_eq!(
            observation,
            AgentWakeExecutionObservation::Terminal {
                status: expected_status,
                artifact_refs: Vec::new(),
                terminal_error: error.map(|error| error.trim().to_string()),
            }
        );
    }
}

#[test]
fn sqlite_adapter_atomically_projects_claims_and_settles_direct_parent_result() {
    let fixture = tempfile::tempdir().unwrap();
    let storage = Arc::new(
        mycopilot_core::storage::service::StorageService::open(
            &fixture.path().join("dispatcher.sqlite"),
        )
        .unwrap(),
    );
    storage
        .save_conversation(empty_conversation("conversation-root"))
        .unwrap();
    storage
        .save_conversation(empty_conversation("conversation-child"))
        .unwrap();
    storage
        .ensure_root_agent(&EnsureRootAgentInput {
            agent_id: "agent-root".to_string(),
            conversation_id: "conversation-root".to_string(),
            creation_request_id: "ensure-root".to_string(),
            task_name: "Root".to_string(),
        })
        .unwrap();
    let raw = rusqlite::Connection::open(fixture.path().join("dispatcher.sqlite")).unwrap();
    raw.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
    raw.execute(
        "INSERT INTO agent_nodes (
             agent_id, schema_version, root_agent_id, root_conversation_id,
             parent_agent_id, conversation_id, project_id, creation_request_id,
             task_name, task_path, model_config_id_snapshot,
             model_display_name_snapshot, model_supports_image_snapshot,
             model_context_window_tokens_snapshot, model_settings_revision_snapshot,
             provider_connection_revision_snapshot, provider_protocol_revision_snapshot,
             model_selection_source_snapshot, lifecycle, revision, created_at, updated_at
         ) VALUES (
             'agent-child', 1, 'agent-root', 'conversation-root',
             'agent-root', 'conversation-child', NULL, 'spawn-child',
             'review', '/root/review', 'model-a', 'Model A', 0, 32000,
             'settings-v1', 'connection-v1', 'protocol-v1', 'explicit',
             'active', 1, 2, 2
         )",
        [],
    )
    .unwrap();
    drop(raw);
    let dispatch = storage
        .follow_up_agent(&SendAgentMessageRequest {
            sender_agent_id: "agent-root".to_string(),
            recipient_agent_id: "agent-child".to_string(),
            request_id: "followup-one".to_string(),
            content: "review auth".to_string(),
        })
        .unwrap();
    let wake_id = dispatch.deferred_wake.unwrap().wake_id;
    let adapter = SqliteAgentDispatcherStore::new(Arc::clone(&storage));
    let claimed = adapter
        .claim_next_dispatchable("dispatcher-claim", mycopilot_core::storage::now_ms())
        .unwrap()
        .unwrap();
    assert_eq!(claimed.wake_id, wake_id);
    assert_eq!(claimed.status, AgentWakeStatus::Claimed);
    let source = storage
        .get_agent_message(claimed.source_agent_message_id.as_deref().unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(
        source.delivery_status,
        mycopilot_core::AgentMailboxDeliveryStatus::Acknowledged
    );
    assert_eq!(
        storage
            .conversation_message_origin("conversation-child", &source.projection_message_id,)
            .unwrap(),
        mycopilot_core::ConversationMessageOrigin::Agent {
            sender_agent_id: "agent-root".to_string(),
            source_agent_message_id: source.message_id.clone(),
        }
    );

    let settlement = AgentWakeSettlement {
        wake_id: wake_id.clone(),
        expected_status: AgentWakeStatus::Claimed,
        claim_token: "dispatcher-claim".to_string(),
        terminal_status: AgentWakeStatus::Failed,
        run_id: None,
        assistant_message_id: None,
        artifact_refs: Vec::new(),
        terminal_error: Some("model disabled".to_string()),
    };
    adapter
        .settle(&settlement, mycopilot_core::storage::now_ms())
        .unwrap();
    adapter
        .settle(&settlement, mycopilot_core::storage::now_ms())
        .unwrap();
    let terminal = storage.get_agent_wake(&wake_id).unwrap().unwrap();
    assert_eq!(terminal.status, AgentWakeStatus::Failed);
    let result = storage
        .get_agent_message(terminal.result_message_id.as_deref().unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(result.sender_agent_id, "agent-child");
    assert_eq!(result.recipient_agent_id, "agent-root");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sqlite_dispatcher_redelivers_interrupt_receipt_left_before_runtime_dispatch() {
    let fixture = tempfile::tempdir().unwrap();
    let database_path = fixture.path().join("interrupt-recovery.sqlite");
    let storage =
        Arc::new(mycopilot_core::storage::service::StorageService::open(&database_path).unwrap());
    storage
        .save_conversation(empty_conversation("conversation-root-interrupt"))
        .unwrap();
    storage
        .save_conversation(empty_conversation("conversation-child-interrupt"))
        .unwrap();
    storage
        .ensure_root_agent(&EnsureRootAgentInput {
            agent_id: "agent-root-interrupt".to_string(),
            conversation_id: "conversation-root-interrupt".to_string(),
            creation_request_id: "ensure-root-interrupt".to_string(),
            task_name: "Root".to_string(),
        })
        .unwrap();
    let raw = rusqlite::Connection::open(&database_path).unwrap();
    raw.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
    raw.execute(
        "INSERT INTO agent_nodes (
             agent_id, schema_version, root_agent_id, root_conversation_id,
             parent_agent_id, conversation_id, project_id, creation_request_id,
             task_name, task_path, model_config_id_snapshot,
             model_display_name_snapshot, model_supports_image_snapshot,
             model_context_window_tokens_snapshot, model_settings_revision_snapshot,
             provider_connection_revision_snapshot, provider_protocol_revision_snapshot,
             model_selection_source_snapshot, lifecycle, revision, created_at, updated_at
         ) VALUES (
             'agent-child-interrupt', 1, 'agent-root-interrupt',
             'conversation-root-interrupt', 'agent-root-interrupt',
             'conversation-child-interrupt', NULL, 'spawn-child-interrupt',
             'review', '/root/review', 'model-a', 'Model A', 0, 32000,
             'settings-v1', 'connection-v1', 'protocol-v1', 'explicit',
             'active', 1, 2, 2
         )",
        [],
    )
    .unwrap();
    drop(raw);
    let dispatch = storage
        .follow_up_agent(&SendAgentMessageRequest {
            sender_agent_id: "agent-root-interrupt".to_string(),
            recipient_agent_id: "agent-child-interrupt".to_string(),
            request_id: "followup-interrupt-recovery".to_string(),
            content: "wait for interrupt recovery".to_string(),
        })
        .unwrap();
    let wake_id = dispatch.deferred_wake.unwrap().wake_id;
    let claimed_at = mycopilot_core::storage::now_ms();
    let claimed = storage
        .claim_next_dispatchable_agent_wake_at("claim-before-crash", claimed_at)
        .unwrap()
        .unwrap();
    assert_eq!(claimed.wake_id, wake_id);
    storage
        .upsert_chat_messages(
            "conversation-child-interrupt",
            vec![ChatMessageRecord {
                human_interaction_response: None,
                id: "assistant-before-crash".to_string(),
                role: "assistant".to_string(),
                content: String::new(),
                created_at: claimed_at + 1,
                status: Some("streaming".to_string()),
                attachments: Vec::new(),
                folder_references_json: None,
                agent_run_json: None,
                ui_state_json: None,
            }],
            1,
        )
        .unwrap();
    let trace = mycopilot_core::ConversationTraceSnapshot::default().in_progress_trace(
        "run-before-crash",
        "conversation-child-interrupt",
        "assistant-before-crash",
    );
    storage
        .append_in_progress_conversation_turn_trace(&trace, claimed_at + 1, claimed_at + 1)
        .unwrap();
    let raw = rusqlite::Connection::open(&database_path).unwrap();
    raw.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
    raw.execute(
        "UPDATE agent_wake_requests
         SET status = 'running', status_revision = status_revision + 1,
             run_id = 'run-before-crash', assistant_message_id = 'assistant-before-crash',
             started_at = ?1
         WHERE wake_id = ?2 AND status = 'claimed' AND claim_token = 'claim-before-crash'",
        rusqlite::params![claimed_at + 1, &wake_id],
    )
    .unwrap();
    drop(raw);
    let receipt = storage
        .interrupt_agent_execution_with_receipt_at(
            "agent-root-interrupt",
            "agent-child-interrupt",
            "interrupt-before-crash",
            claimed_at + 2,
        )
        .unwrap();
    assert_eq!(receipt.dispatched_at, None);

    let store = Arc::new(SqliteAgentDispatcherStore::new(Arc::clone(&storage)));
    let executor = Arc::new(FakeExecutor::immediate());
    let dispatcher = AgentDispatcher::start_with_clock(
        store,
        executor.clone(),
        Arc::new(TestClock(AtomicI64::new(claimed_at + 3))),
        AgentTurnConcurrencyGate::new(1).unwrap(),
        AgentDispatcherConfig {
            global_concurrency_limit: 1,
            idle_poll_interval: Duration::from_millis(1),
            shutdown_grace: Duration::from_millis(2),
            ..AgentDispatcherConfig::default()
        },
    )
    .unwrap();
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if storage
                .list_undispatched_agent_interrupts()
                .unwrap()
                .is_empty()
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the restarted Dispatcher must acknowledge the redelivered interrupt");
    assert_eq!(
        executor
            .interrupts
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_slice(),
        ["run-before-crash"]
    );
    dispatcher.shutdown().await.unwrap();
}

#[test]
fn zero_global_limit_is_rejected() {
    let config = AgentDispatcherConfig {
        global_concurrency_limit: 0,
        ..AgentDispatcherConfig::default()
    };
    assert!(matches!(
        config.validate(),
        Err(AgentDispatcherError::Configuration(_))
    ));
}
