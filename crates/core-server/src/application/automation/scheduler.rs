use super::permissions::{ensure_automation_permission_mode_enabled, permissions_from_projection};
use super::schedule::normalize_schedule;
use super::service::{config_snapshot, parse_permission_mode, AutomationConfigSnapshot};
use crate::application::agent::{
    AgentService, AutomationExecutionContext, AutomationHumanRootDestination,
    AutomationHumanRootStartError, AutomationHumanRootTurnStart, AutomationTurnObservation,
    CoreServerNotificationSender,
};
use mycopilot_core::storage::automation_repository::{
    AutomationCompareAndSetOutcome, AutomationRunMutationOutcome, AutomationRunRecord,
    AutomationRunSettlementInput, NewScheduledAutomationRunRecord, StoredAutomationRunStatus,
};
use mycopilot_core::storage::now_ms;
use mycopilot_core::storage::service::StorageService;
use mycopilot_core::ConversationTurnTraceTerminalStatus;
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{watch, Notify, OwnedSemaphorePermit, Semaphore};
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;

pub(crate) const AUTOMATION_SCHEDULER_INTERVAL: Duration = Duration::from_secs(30);
pub(crate) const AUTOMATION_SCHEDULER_BATCH_LIMIT: usize = 3;
pub(crate) const AUTOMATION_CONCURRENCY_LIMIT: usize = 2;
const AUTOMATION_ADMISSION_LEASE_MS: i64 = 60_000;
const AUTOMATION_RETRY_BACKOFF_MS: i64 = 5_000;
const AUTOMATION_OBSERVATION_POLL: Duration = Duration::from_secs(2);
const AUTOMATION_START_FAILURE_SETTLEMENT_RETRY_MIN: Duration = Duration::from_millis(100);
const AUTOMATION_START_FAILURE_SETTLEMENT_RETRY_MAX: Duration = Duration::from_secs(2);

#[cfg(test)]
static AUTOMATION_FATAL_RUN_INSPECTION_FAILURES: Mutex<Vec<String>> = Mutex::new(Vec::new());

#[cfg(test)]
fn inject_automation_fatal_run_inspection_failures(run_id: &str, attempts: usize) {
    AUTOMATION_FATAL_RUN_INSPECTION_FAILURES
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .extend(std::iter::repeat_n(run_id.to_string(), attempts));
}

#[cfg(test)]
fn take_automation_fatal_run_inspection_failure(run_id: &str) -> bool {
    let mut failures = AUTOMATION_FATAL_RUN_INSPECTION_FAILURES
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let Some(index) = failures.iter().position(|candidate| candidate == run_id) else {
        return false;
    };
    failures.swap_remove(index);
    true
}

/// Process-local accelerator only. SQLite remains authoritative and every wake coalesces safely.
#[derive(Clone, Default)]
pub(crate) struct AutomationSchedulerWake {
    notify: Arc<Notify>,
}

impl AutomationSchedulerWake {
    pub(crate) fn wake(&self) {
        self.notify.notify_one();
    }

    pub(crate) async fn notified(&self) {
        self.notify.notified().await;
    }
}

/// Core-owned lifecycle for scheduled HumanRoot turns.
///
/// No SQLite transaction remains open while this scheduler resolves a schedule, waits for Agent
/// capacity, invokes a provider or MCP tool, or observes an approval. Repository calls own every
/// short `BEGIN IMMEDIATE` claim and SQLite is the sole recovery authority.
pub(crate) struct AutomationScheduler {
    state: Arc<AutomationSchedulerState>,
    stop_tx: watch::Sender<bool>,
    loop_handle: Option<JoinHandle<()>>,
}

struct AutomationSchedulerState {
    storage: Arc<StorageService>,
    agent_service: AgentService,
    notifications: CoreServerNotificationSender,
    wake: AutomationSchedulerWake,
    /// Fixed process-start boundary used for every bounded due-task batch. A boolean "first
    /// cycle" marker is insufficient because more than one batch can have been missed offline.
    startup_recovery_cutoff: i64,
    automation_capacity: Arc<Semaphore>,
    accepting: AtomicBool,
    observed_runs: Mutex<HashSet<String>>,
    workers: Mutex<Vec<JoinHandle<()>>>,
}

impl AutomationScheduler {
    pub(crate) async fn start(
        storage: Arc<StorageService>,
        agent_service: AgentService,
        notifications: CoreServerNotificationSender,
        wake: AutomationSchedulerWake,
    ) -> Result<Self, String> {
        let recovered_at = now_ms();
        blocking_storage(Arc::clone(&storage), move |storage| {
            storage.recover_automation_admission_leases_on_startup(recovered_at)
        })
        .await?;

        let state = Arc::new(AutomationSchedulerState {
            storage,
            agent_service,
            notifications,
            wake,
            startup_recovery_cutoff: recovered_at,
            automation_capacity: Arc::new(Semaphore::new(AUTOMATION_CONCURRENCY_LIMIT)),
            accepting: AtomicBool::new(true),
            observed_runs: Mutex::new(HashSet::new()),
            workers: Mutex::new(Vec::new()),
        });
        state.restore_bound_run_observers().await?;

        let (stop_tx, stop_rx) = watch::channel(false);
        let loop_state = Arc::clone(&state);
        let loop_handle = tokio::spawn(async move {
            if let Err(error) = loop_state.run_loop(stop_rx).await {
                eprintln!("automation scheduler stopped after an internal error: {error}");
            }
        });
        state.wake.wake();
        Ok(Self {
            state,
            stop_tx,
            loop_handle: Some(loop_handle),
        })
    }

    /// Stops new claims before Agent shutdown. Existing observers remain alive so terminal traces
    /// can still be projected while AgentService performs its normal bounded shutdown.
    pub(crate) async fn stop_admissions(&mut self) {
        self.state.accepting.store(false, Ordering::Release);
        let _ = self.stop_tx.send(true);
        self.state.wake.wake();
        if let Some(handle) = self.loop_handle.take() {
            let _ = handle.await;
        }
    }

    /// Performs one final durable compensation pass after Agent shutdown, then retires local
    /// observers. A genuinely non-terminal trace remains recoverable at the next process start.
    pub(crate) async fn finish_shutdown(&self) {
        if let Err(error) = self.state.reconcile_bound_runs_once().await {
            eprintln!("automation shutdown reconciliation failed: {error}");
        }
        let handles = {
            let mut workers = self
                .state
                .workers
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            std::mem::take(&mut *workers)
        };
        for handle in handles {
            handle.abort();
            let _ = handle.await;
        }
    }
}

impl AutomationSchedulerState {
    async fn run_loop(self: Arc<Self>, mut stop_rx: watch::Receiver<bool>) -> Result<(), String> {
        let mut interval = tokio::time::interval(AUTOMATION_SCHEDULER_INTERVAL);
        interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = interval.tick() => {}
                _ = self.wake.notified() => {}
                changed = stop_rx.changed() => {
                    if changed.is_err() || *stop_rx.borrow() {
                        break;
                    }
                    continue;
                }
            }
            if !self.accepting.load(Ordering::Acquire) {
                break;
            }
            match self.run_cycle().await {
                Ok(more_work) => {
                    if more_work {
                        self.wake.wake();
                    }
                }
                Err(error) => {
                    eprintln!("automation scheduler cycle failed safely: {error}");
                }
            }
            self.reap_finished_workers();
        }
        Ok(())
    }

    async fn run_cycle(self: &Arc<Self>) -> Result<bool, String> {
        self.cancel_requested_runs().await?;
        // This is also the in-process repair path for an observer that exited after a transient
        // storage failure. Registration is idempotent, and SQLite identifies bound work.
        self.restore_bound_run_observers().await?;
        let now = now_ms();
        let due = blocking_storage(Arc::clone(&self.storage), move |storage| {
            storage.list_due_automations(now, AUTOMATION_SCHEDULER_BATCH_LIMIT)
        })
        .await?;
        let due_was_full = due.len() == AUTOMATION_SCHEDULER_BATCH_LIMIT;
        for task in due {
            let Some(scheduled_for) = task.config.next_run_at else {
                continue;
            };
            let schedule = match serde_json::from_str(&task.config.schedule_json) {
                Ok(schedule) => schedule,
                Err(_) => {
                    self.block_due_task(
                        &task,
                        "schedule_invalid",
                        "The stored schedule is invalid and must be edited.",
                    )
                    .await?;
                    continue;
                }
            };
            let normalized = match normalize_schedule(&schedule) {
                Ok(normalized) => normalized,
                Err(error) => {
                    self.block_due_task(&task, "schedule_invalid", &error.to_string())
                        .await?;
                    continue;
                }
            };
            let next_run_at = match normalized.next_run_at_ms(now) {
                Ok(next_run_at) => next_run_at,
                Err(error) => {
                    self.block_due_task(&task, "schedule_invalid", &error.to_string())
                        .await?;
                    continue;
                }
            };
            let snapshot = match config_snapshot(&task) {
                Ok(snapshot) => snapshot,
                Err(error) => {
                    self.block_due_task(&task, "configuration_invalid", &error.to_string())
                        .await?;
                    continue;
                }
            };
            let input = NewScheduledAutomationRunRecord {
                id: crate::application::agent_support::create_id("automation-run"),
                automation_id: task.id,
                trigger_kind: if scheduled_for < self.startup_recovery_cutoff {
                    "recovery".to_string()
                } else {
                    "scheduled".to_string()
                },
                scheduled_for,
                config_revision: task.revision,
                config_snapshot_json: snapshot,
                expected_revision: task.revision,
                next_run_at,
                claimed_at: now,
            };
            let _ = blocking_storage(Arc::clone(&self.storage), move |storage| {
                storage.enqueue_scheduled_automation_run(&input)
            })
            .await?;
        }

        let available = self
            .automation_capacity
            .available_permits()
            .min(AUTOMATION_SCHEDULER_BATCH_LIMIT);
        if available == 0 {
            // Active workers wake the scheduler when they finish. Returning `true` here would
            // spin the scheduler while the automation-specific capacity is exhausted.
            return Ok(false);
        }
        let claimed = blocking_storage(Arc::clone(&self.storage), move |storage| {
            storage.claim_ready_automation_runs(now_ms(), AUTOMATION_ADMISSION_LEASE_MS, available)
        })
        .await?;
        let claimed_was_full = claimed.len() == available;
        for run in claimed {
            let Ok(permit) = Arc::clone(&self.automation_capacity).try_acquire_owned() else {
                self.defer_claim(&run).await?;
                continue;
            };
            self.spawn_claimed_worker(run, permit);
        }
        Ok(due_was_full || claimed_was_full)
    }

    async fn restore_bound_run_observers(self: &Arc<Self>) -> Result<(), String> {
        let recoverable = blocking_storage(Arc::clone(&self.storage), |storage| {
            storage.list_recoverable_automation_runs()
        })
        .await?;
        for recovery in recoverable {
            if matches!(
                recovery.run.status,
                StoredAutomationRunStatus::Running | StoredAutomationRunStatus::WaitingForApproval
            ) {
                // Reserve the currently available automation slots synchronously so the scheduler
                // cannot claim fresh work before recovered active turns are accounted for. Extra
                // recovered observers wait without blocking bootstrap.
                let permit = Arc::clone(&self.automation_capacity)
                    .try_acquire_owned()
                    .ok();
                self.spawn_recovered_observer(recovery.run, permit);
            }
        }
        Ok(())
    }

    fn spawn_recovered_observer(
        self: &Arc<Self>,
        run: AutomationRunRecord,
        permit: Option<OwnedSemaphorePermit>,
    ) {
        if !self.register_observer(&run.id) {
            return;
        }
        let state = Arc::clone(self);
        let run_id = run.id.clone();
        let handle = tokio::spawn(async move {
            let permit = match permit {
                Some(permit) => Some(permit),
                None => Arc::clone(&state.automation_capacity)
                    .acquire_owned()
                    .await
                    .ok(),
            };
            if let Some(permit) = permit {
                Arc::clone(&state).observe_bound_run(run, permit).await;
            }
            state.unregister_observer(&run_id);
            state.wake.wake();
        });
        self.workers
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(handle);
    }

    fn spawn_claimed_worker(
        self: &Arc<Self>,
        run: AutomationRunRecord,
        permit: OwnedSemaphorePermit,
    ) {
        if !self.register_observer(&run.id) {
            drop(permit);
            return;
        }
        let state = Arc::clone(self);
        let run_id = run.id.clone();
        let handle = tokio::spawn(async move {
            if let Err(error) = Arc::clone(&state).execute_claimed_run(run, permit).await {
                eprintln!("automation run {run_id} worker failed safely: {error}");
            }
            state.unregister_observer(&run_id);
            state.wake.wake();
        });
        self.workers
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(handle);
    }

    async fn execute_claimed_run(
        self: Arc<Self>,
        run: AutomationRunRecord,
        permit: OwnedSemaphorePermit,
    ) -> Result<(), String> {
        let admission_token = run
            .admission_token
            .clone()
            .ok_or_else(|| "claimed automation run is missing its admission token".to_string())?;
        let snapshot = match AutomationConfigSnapshot::parse(&run.config_snapshot_json) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                self.terminate_unadmitted(
                    &run,
                    &admission_token,
                    StoredAutomationRunStatus::Failed,
                    "config_snapshot_invalid",
                    &error.to_string(),
                )
                .await?;
                return Ok(());
            }
        };
        let preferences = blocking_storage(Arc::clone(&self.storage), |storage| {
            storage.load_ui_preferences()
        })
        .await?;
        let permission_mode = match parse_permission_mode(&snapshot.permission_mode) {
            Ok(mode) => mode,
            Err(error) => {
                self.block_or_terminate(
                    &run,
                    &admission_token,
                    "permission_invalid",
                    &error.to_string(),
                )
                .await?;
                return Ok(());
            }
        };
        if let Err(error) = ensure_automation_permission_mode_enabled(permission_mode, &preferences)
        {
            self.block_or_terminate(
                &run,
                &admission_token,
                "permission_disabled",
                &error.to_string(),
            )
            .await?;
            return Ok(());
        }

        let destination = match snapshot.destination_kind.as_str() {
            "new_chat" => match snapshot.model_id.clone() {
                Some(model_id) => AutomationHumanRootDestination::NewChat {
                    project_id: snapshot.project_id.clone(),
                    model_id,
                },
                None => {
                    self.block_or_terminate(
                        &run,
                        &admission_token,
                        "model_missing",
                        "The scheduled task has no model.",
                    )
                    .await?;
                    return Ok(());
                }
            },
            "existing_chat" => match snapshot.target_conversation_id.clone() {
                Some(conversation_id) => {
                    AutomationHumanRootDestination::ExistingChat { conversation_id }
                }
                None => {
                    self.block_or_terminate(
                        &run,
                        &admission_token,
                        "target_missing",
                        "The target conversation no longer exists.",
                    )
                    .await?;
                    return Ok(());
                }
            },
            _ => {
                self.block_or_terminate(
                    &run,
                    &admission_token,
                    "target_invalid",
                    "The stored automation destination is invalid.",
                )
                .await?;
                return Ok(());
            }
        };
        let task = blocking_storage(Arc::clone(&self.storage), {
            let automation_id = run.automation_id.clone();
            move |storage| storage.get_automation(&automation_id)
        })
        .await?;
        let start = AutomationHumanRootTurnStart {
            context: AutomationExecutionContext {
                automation_id: run.automation_id.clone(),
                automation_run_id: run.id.clone(),
                scheduled_for: run.scheduled_for,
                last_run_at: task.as_ref().and_then(|task| task.last_run_at),
                trigger_kind: run.trigger_kind.clone(),
            },
            admission_token: admission_token.clone(),
            config_revision: run.config_revision,
            title: snapshot.title,
            prompt: snapshot.prompt,
            destination,
            permission_mode: snapshot.permission_mode,
            permissions: permissions_from_projection(snapshot.permissions),
        };

        match self
            .agent_service
            .start_automation_human_root_turn(start, self.notifications.clone())
        {
            Ok(output) => {
                let current = blocking_storage(Arc::clone(&self.storage), {
                    let run_id = run.id.clone();
                    move |storage| storage.get_automation_run(&run_id)
                })
                .await?
                .ok_or_else(|| "admitted automation run disappeared".to_string())?;
                if current.agent_run_id.as_deref() != Some(output.run_id.as_str())
                    || current.assistant_message_id.as_deref()
                        != Some(output.assistant_message_id.as_str())
                {
                    eprintln!(
                        "automation run {} returned an identity that differs from its durable admission; observing the durable identity",
                        run.id
                    );
                }
                self.observe_bound_run(current, permit).await;
            }
            Err(AutomationHumanRootStartError::RetryableCapacity)
            | Err(AutomationHumanRootStartError::RetryableConversationBusy) => {
                self.defer_claim(&run).await?;
            }
            Err(AutomationHumanRootStartError::TargetInvalid { code, message }) => {
                self.block_or_terminate(&run, &admission_token, code, &message)
                    .await?;
            }
            Err(AutomationHumanRootStartError::Fatal(message)) => {
                let Some(current) = self
                    .load_automation_run_after_fatal_until_readable(&run.id)
                    .await
                else {
                    return Ok(());
                };
                if let Some(current) = current.filter(|current| current.agent_run_id.is_some()) {
                    if current.status.is_terminal() {
                        return Ok(());
                    }
                    if !self
                        .settle_bound_start_failure_until_durable(&current, &message)
                        .await
                    {
                        return Ok(());
                    }
                    let Some(current) = self
                        .load_automation_run_after_fatal_until_readable(&current.id)
                        .await
                    else {
                        return Ok(());
                    };
                    if let Some(current) = current.filter(|current| !current.status.is_terminal()) {
                        self.observe_bound_run(current, permit).await;
                    }
                } else {
                    self.terminate_unadmitted(
                        &run,
                        &admission_token,
                        StoredAutomationRunStatus::Failed,
                        "agent_start_failed",
                        &safe_error_message(&message),
                    )
                    .await?;
                }
            }
        }
        Ok(())
    }

    /// A failed authoritative read after `start_automation_human_root_turn` cannot be classified
    /// as unadmitted. Retry until SQLite explicitly says whether an immutable admission binding
    /// exists; otherwise a transient read fault could abandon a bound in-progress Trace forever.
    /// The outer `Option` is `None` only when scheduler shutdown asks this worker to retire.
    async fn load_automation_run_after_fatal_until_readable(
        &self,
        run_id: &str,
    ) -> Option<Option<AutomationRunRecord>> {
        let mut retry_delay = AUTOMATION_START_FAILURE_SETTLEMENT_RETRY_MIN;
        loop {
            #[cfg(test)]
            let inspected = if take_automation_fatal_run_inspection_failure(run_id) {
                Err("injected post-admission Automation run inspection failure".to_string())
            } else {
                blocking_storage(Arc::clone(&self.storage), {
                    let run_id = run_id.to_string();
                    move |storage| storage.get_automation_run(&run_id)
                })
                .await
            };
            #[cfg(not(test))]
            let inspected = blocking_storage(Arc::clone(&self.storage), {
                let run_id = run_id.to_string();
                move |storage| storage.get_automation_run(&run_id)
            })
            .await;
            match inspected {
                Ok(run) => return Some(run),
                Err(error) => {
                    eprintln!(
                        "automation run {run_id} post-start inspection failed safely and will retry: {error}"
                    );
                }
            }
            if !self.accepting.load(Ordering::Acquire) {
                return None;
            }
            tokio::time::sleep(retry_delay).await;
            retry_delay = retry_delay
                .saturating_mul(2)
                .min(AUTOMATION_START_FAILURE_SETTLEMENT_RETRY_MAX);
        }
    }

    /// A fatal return after atomic Automation admission is not an unadmitted start failure: its
    /// one HumanRoot message pair and in-progress Trace already exist. Keep the claimed worker and
    /// its `observed_runs` registration until the exact Trace is durably failed; handing it to the
    /// ordinary observer first would leave an empty in-progress Trace permanently `running`.
    async fn settle_bound_start_failure_until_durable(
        &self,
        run: &AutomationRunRecord,
        cause: &str,
    ) -> bool {
        let mut retry_delay = AUTOMATION_START_FAILURE_SETTLEMENT_RETRY_MIN;
        let cause = safe_error_message(cause);
        loop {
            let service = self.agent_service.clone();
            let automation_run_id = run.id.clone();
            let cause = cause.clone();
            let settlement = tokio::task::spawn_blocking(move || {
                service.settle_automation_start_failure(&automation_run_id, &cause)
            })
            .await;
            match settlement {
                Ok(Ok(())) => return true,
                Ok(Err(error)) => {
                    eprintln!(
                        "automation run {} start-failure settlement failed safely and will retry: {error}",
                        run.id
                    );
                }
                Err(error) => {
                    eprintln!(
                        "automation run {} start-failure settlement worker stopped and will retry: {error}",
                        run.id
                    );
                }
            }
            if !self.accepting.load(Ordering::Acquire) {
                return false;
            }
            tokio::time::sleep(retry_delay).await;
            retry_delay = retry_delay
                .saturating_mul(2)
                .min(AUTOMATION_START_FAILURE_SETTLEMENT_RETRY_MAX);
        }
    }

    async fn observe_bound_run(
        self: Arc<Self>,
        mut run: AutomationRunRecord,
        _permit: OwnedSemaphorePermit,
    ) {
        let (Some(agent_run_id), Some(assistant_message_id)) =
            (run.agent_run_id.clone(), run.assistant_message_id.clone())
        else {
            return;
        };
        let notification = self
            .agent_service
            .durable_turn_notification(&assistant_message_id);
        loop {
            if run.cancellation_requested_at.is_some() {
                if let Err(error) = self
                    .agent_service
                    .cancel_automation_agent_run(&agent_run_id)
                {
                    eprintln!(
                        "automation cancellation for {} failed safely: {error}",
                        run.id
                    );
                }
            }
            match self
                .agent_service
                .automation_turn_state(&agent_run_id, &assistant_message_id)
            {
                Ok(AutomationTurnObservation::Running) => {
                    if run.status == StoredAutomationRunStatus::WaitingForApproval {
                        self.update_waiting(&run, &agent_run_id, false).await;
                    }
                }
                Ok(AutomationTurnObservation::WaitingForApproval) => {
                    if run.status != StoredAutomationRunStatus::WaitingForApproval {
                        self.update_waiting(&run, &agent_run_id, true).await;
                    }
                }
                Ok(AutomationTurnObservation::Terminal {
                    status,
                    result_preview,
                    error_code,
                    error_message,
                }) => {
                    let terminal_status = trace_terminal_status(status);
                    let input = AutomationRunSettlementInput {
                        automation_run_id: run.id.clone(),
                        agent_run_id: agent_run_id.clone(),
                        terminal_status,
                        report_kind: run.report_kind.clone(),
                        result_preview,
                        error_code,
                        error_message,
                        settled_at: now_ms(),
                    };
                    match blocking_storage(Arc::clone(&self.storage), move |storage| {
                        storage.settle_automation_run_from_trace(&input)
                    })
                    .await
                    {
                        Ok(AutomationRunMutationOutcome::Updated(_)) => return,
                        Ok(AutomationRunMutationOutcome::Stale(Some(current)))
                            if current.status.is_terminal() =>
                        {
                            return;
                        }
                        Ok(AutomationRunMutationOutcome::Stale(_)) => {
                            eprintln!(
                                "automation terminal settlement was temporarily stale; retrying"
                            );
                        }
                        Err(error) => {
                            eprintln!(
                                "automation terminal settlement failed safely and will retry: {error}"
                            );
                        }
                    }
                }
                Err(error) => {
                    eprintln!("automation durable observation failed safely: {error}");
                }
            }
            tokio::select! {
                _ = notification.notified() => {}
                _ = tokio::time::sleep(AUTOMATION_OBSERVATION_POLL) => {}
            }
            match blocking_storage(Arc::clone(&self.storage), {
                let run_id = run.id.clone();
                move |storage| storage.get_automation_run(&run_id)
            })
            .await
            {
                Ok(Some(current)) if !current.status.is_terminal() => run = current,
                _ => return,
            }
        }
    }

    async fn update_waiting(&self, run: &AutomationRunRecord, agent_run_id: &str, waiting: bool) {
        let result = blocking_storage(Arc::clone(&self.storage), {
            let run_id = run.id.clone();
            let agent_run_id = agent_run_id.to_string();
            move |storage| {
                storage.set_automation_run_waiting_for_approval(
                    &run_id,
                    &agent_run_id,
                    waiting,
                    now_ms(),
                )
            }
        })
        .await;
        if let Err(error) = result {
            eprintln!("automation approval-state projection failed safely: {error}");
        }
    }

    async fn defer_claim(&self, run: &AutomationRunRecord) -> Result<(), String> {
        let Some(token) = run.admission_token.clone() else {
            return Ok(());
        };
        let deferred_at = now_ms();
        let retry_at = deferred_at.saturating_add(AUTOMATION_RETRY_BACKOFF_MS);
        let run_id = run.id.clone();
        blocking_storage(Arc::clone(&self.storage), move |storage| {
            storage.defer_automation_run(&run_id, &token, retry_at, deferred_at)
        })
        .await?;
        Ok(())
    }

    async fn block_due_task(
        &self,
        task: &mycopilot_core::storage::automation_repository::AutomationRecord,
        code: &str,
        message: &str,
    ) -> Result<(), String> {
        let automation_id = task.id.clone();
        let revision = task.revision;
        let code = canonical_blocked_code(code).to_string();
        let message = safe_error_message(message);
        blocking_storage(Arc::clone(&self.storage), move |storage| {
            storage.block_automation(&automation_id, revision, &code, &message)
        })
        .await?;
        Ok(())
    }

    async fn block_or_terminate(
        &self,
        run: &AutomationRunRecord,
        admission_token: &str,
        code: &str,
        message: &str,
    ) -> Result<(), String> {
        let task = blocking_storage(Arc::clone(&self.storage), {
            let automation_id = run.automation_id.clone();
            move |storage| storage.get_automation(&automation_id)
        })
        .await?;
        if let Some(task) = task.filter(|task| task.revision == run.config_revision) {
            let automation_id = task.id;
            let code = canonical_blocked_code(code).to_string();
            let message = safe_error_message(message);
            let outcome = blocking_storage(Arc::clone(&self.storage), move |storage| {
                storage.block_automation(&automation_id, task.revision, &code, &message)
            })
            .await?;
            if matches!(outcome, AutomationCompareAndSetOutcome::Updated(_)) {
                return Ok(());
            }
        }
        self.terminate_unadmitted(
            run,
            admission_token,
            StoredAutomationRunStatus::Failed,
            code,
            message,
        )
        .await
    }

    async fn terminate_unadmitted(
        &self,
        run: &AutomationRunRecord,
        admission_token: &str,
        status: StoredAutomationRunStatus,
        code: &str,
        message: &str,
    ) -> Result<(), String> {
        let run_id = run.id.clone();
        let admission_token = admission_token.to_string();
        let code = safe_error_code(code);
        let message = safe_error_message(message);
        blocking_storage(Arc::clone(&self.storage), move |storage| {
            storage.terminate_unadmitted_automation_run(
                &run_id,
                &admission_token,
                status,
                &code,
                &message,
                now_ms(),
            )
        })
        .await?;
        Ok(())
    }

    async fn cancel_requested_runs(&self) -> Result<(), String> {
        let runs = blocking_storage(Arc::clone(&self.storage), |storage| {
            storage.list_cancellation_requested_automation_runs()
        })
        .await?;
        for run in runs {
            if let Some(agent_run_id) = run.agent_run_id.as_deref() {
                if let Err(error) = self.agent_service.cancel_automation_agent_run(agent_run_id) {
                    eprintln!("automation deletion cancellation failed safely: {error}");
                }
            }
        }
        Ok(())
    }

    async fn reconcile_bound_runs_once(self: &Arc<Self>) -> Result<(), String> {
        let recoverable = blocking_storage(Arc::clone(&self.storage), |storage| {
            storage.list_recoverable_automation_runs()
        })
        .await?;
        for recovery in recoverable {
            let run = recovery.run;
            let (Some(agent_run_id), Some(assistant_message_id)) =
                (run.agent_run_id.clone(), run.assistant_message_id.clone())
            else {
                continue;
            };
            if let AutomationTurnObservation::Terminal {
                status,
                result_preview,
                error_code,
                error_message,
            } = self
                .agent_service
                .automation_turn_state(&agent_run_id, &assistant_message_id)?
            {
                let input = AutomationRunSettlementInput {
                    automation_run_id: run.id,
                    agent_run_id,
                    terminal_status: trace_terminal_status(status),
                    report_kind: run.report_kind,
                    result_preview,
                    error_code,
                    error_message,
                    settled_at: now_ms(),
                };
                blocking_storage(Arc::clone(&self.storage), move |storage| {
                    storage.settle_automation_run_from_trace(&input)
                })
                .await?;
            }
        }
        Ok(())
    }

    fn register_observer(&self, run_id: &str) -> bool {
        self.observed_runs
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(run_id.to_string())
    }

    fn unregister_observer(&self, run_id: &str) {
        self.observed_runs
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(run_id);
    }

    fn reap_finished_workers(&self) {
        self.workers
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .retain(|handle| !handle.is_finished());
    }
}

async fn blocking_storage<T, F>(storage: Arc<StorageService>, operation: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce(&StorageService) -> Result<T, String> + Send + 'static,
{
    tokio::task::spawn_blocking(move || operation(&storage))
        .await
        .map_err(|_| "automation storage worker stopped unexpectedly".to_string())?
}

fn trace_terminal_status(status: ConversationTurnTraceTerminalStatus) -> StoredAutomationRunStatus {
    match status {
        ConversationTurnTraceTerminalStatus::Completed => StoredAutomationRunStatus::Completed,
        ConversationTurnTraceTerminalStatus::Failed => StoredAutomationRunStatus::Failed,
        ConversationTurnTraceTerminalStatus::Cancelled => StoredAutomationRunStatus::Cancelled,
        ConversationTurnTraceTerminalStatus::InProgress => StoredAutomationRunStatus::Failed,
    }
}

fn safe_error_code(value: &str) -> String {
    let sanitized = value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
        .take(128)
        .collect::<String>();
    if sanitized.is_empty() {
        "automation_failed".to_string()
    } else {
        sanitized
    }
}

/// Health codes cross the public Automation protocol. Never persist an internal/adaptor-specific
/// spelling that would make a subsequently valid list/get response fail strict projection.
fn canonical_blocked_code(value: &str) -> &'static str {
    match value {
        "target_missing" => "target_missing",
        "target_archived" => "target_archived",
        "target_invalid" | "target_not_writable" | "target_not_root" => "target_invalid",
        "project_missing" => "project_missing",
        "project_path_missing" => "project_path_missing",
        "model_missing" => "model_missing",
        "model_disabled" => "model_disabled",
        "permission_disabled" => "permission_disabled",
        "schedule_invalid" => "schedule_invalid",
        "configuration_invalid" | "permission_invalid" => "configuration_invalid",
        _ => "configuration_invalid",
    }
}

fn safe_error_message(value: &str) -> String {
    let sanitized = value
        .chars()
        .map(|character| {
            if character.is_control() && !matches!(character, '\n' | '\r' | '\t') {
                ' '
            } else {
                character
            }
        })
        .collect::<String>();
    let sanitized = sanitized.trim();
    if sanitized.is_empty() {
        return "The automated Agent turn could not be started.".to_string();
    }
    let mut boundary = sanitized.len().min(4_096);
    while boundary > 0 && !sanitized.is_char_boundary(boundary) {
        boundary -= 1;
    }
    sanitized[..boundary].to_string()
}

#[cfg(test)]
mod tests;
