//! Persistent child-Agent Wake scheduling.
//!
//! SQLite owns Wake ordering, leases, execution identity, and terminal settlement. This module
//! contributes only a process-local concurrency permit and low-latency notifications. It never
//! interprets a Graph workflow and never invokes a second Agent loop: the production execution
//! port below is backed by the shared [`AgentService`] Turn executor.

use crate::application::agent::AgentRunCancellationOutcome;
use crate::application::blocking_executor::{BlockingError, BlockingReservation, BlockingWork};
use mycopilot_core::storage::service::AgentWakeApprovalWaitOutcome;
use mycopilot_core::{
    AgentGraphError, AgentResultArtifactReference, AgentWakeRecoveryAction, AgentWakeRecoveryBatch,
    AgentWakeRequestRecord, AgentWakeStatus, ConversationTurnTraceTerminalStatus,
    FinishAgentTurnResultInput, InterruptAgentExecutionOutcome,
};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Notify;
use tokio::task::{JoinHandle, JoinSet};

pub(crate) const DEFAULT_AGENT_GLOBAL_CONCURRENCY: usize = 50;
const DEFAULT_WAKE_LEASE_RENEW_INTERVAL: Duration = Duration::from_secs(20);
const DEFAULT_DISPATCH_IDLE_POLL_INTERVAL: Duration = Duration::from_secs(1);
const DEFAULT_DISPATCH_SHUTDOWN_GRACE: Duration = Duration::from_secs(5);
// Admission input is loaded inside the job. This conservative reservation budgets the closure
// and expected history working set; it is not a bound on all model/runtime memory.
const DISPATCH_PREPARATION_ESTIMATED_BYTES: usize = 2 * 1024 * 1024;
const DISPATCH_MAINTENANCE_ESTIMATED_BYTES: usize = 16 * 1024;

type DispatchFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// One process-wide concurrency gate shared by public root Turns and dispatched child Turns.
/// Dispatcher first reserves capacity for the durable claim, then promotes that reservation to
/// an active permit once a Wake exists. Active permits are released only after the logical Turn
/// becomes terminal. Approval-paused Turns intentionally retain their permit.
#[derive(Clone)]
pub(crate) struct AgentTurnConcurrencyGate {
    state: Arc<Mutex<AgentTurnConcurrencyState>>,
    changed: Arc<Notify>,
    limit: usize,
}

#[derive(Debug, Default)]
struct AgentTurnConcurrencyState {
    active: usize,
    reserved: usize,
}

impl AgentTurnConcurrencyGate {
    pub(crate) fn new(limit: usize) -> Result<Self, AgentDispatcherError> {
        if limit == 0 {
            return Err(AgentDispatcherError::Configuration(
                "global Agent concurrency limit must be greater than zero".to_string(),
            ));
        }
        Ok(Self {
            state: Arc::new(Mutex::new(AgentTurnConcurrencyState::default())),
            changed: Arc::new(Notify::new()),
            limit,
        })
    }

    pub(crate) fn default_process_gate() -> Self {
        Self::new(DEFAULT_AGENT_GLOBAL_CONCURRENCY)
            .expect("the built-in global Agent concurrency limit is valid")
    }

    pub(crate) fn limit(&self) -> usize {
        self.limit
    }

    pub(crate) fn try_acquire(&self) -> Result<AgentTurnConcurrencyPermit, String> {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if state.active.saturating_add(state.reserved) >= self.limit {
            return Err(format!(
                "Agent 全局并发上限为 {}，请等待正在执行的 Turn 完成。",
                self.limit
            ));
        }
        state.active = state.active.saturating_add(1);
        Ok(AgentTurnConcurrencyPermit(Arc::new(
            AgentTurnConcurrencyLease {
                state: Arc::clone(&self.state),
                changed: Arc::clone(&self.changed),
                phase: AtomicU8::new(AgentTurnConcurrencyPermitPhase::Active as u8),
            },
        )))
    }

    /// Reserves one process slot while the Dispatcher performs its durable claim. An empty queue
    /// probe consumes capacity (so a root Turn cannot race past the configured limit) but is not
    /// reported as an active logical Turn. A successful claim promotes this exact reservation
    /// before the worker can start.
    fn try_reserve_for_dispatch(&self) -> Result<AgentTurnConcurrencyPermit, String> {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if state.active.saturating_add(state.reserved) >= self.limit {
            return Err(format!(
                "Agent 全局并发上限为 {}，请等待正在执行的 Turn 完成。",
                self.limit
            ));
        }
        state.reserved = state.reserved.saturating_add(1);
        Ok(AgentTurnConcurrencyPermit(Arc::new(
            AgentTurnConcurrencyLease {
                state: Arc::clone(&self.state),
                changed: Arc::clone(&self.changed),
                phase: AtomicU8::new(AgentTurnConcurrencyPermitPhase::Reserved as u8),
            },
        )))
    }

    /// Rebuilds the process accounting for a durable in-progress Turn after restart. Recovery is
    /// deliberately allowed to exceed a newly lowered configured limit; fresh admissions remain
    /// blocked until the durable active count falls below that limit.
    pub(crate) fn adopt_recovered(&self) -> AgentTurnConcurrencyPermit {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        state.active = state.active.saturating_add(1);
        AgentTurnConcurrencyPermit(Arc::new(AgentTurnConcurrencyLease {
            state: Arc::clone(&self.state),
            changed: Arc::clone(&self.changed),
            phase: AtomicU8::new(AgentTurnConcurrencyPermitPhase::Active as u8),
        }))
    }

    fn active_count(&self) -> usize {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .active
    }

    #[cfg(test)]
    pub(crate) fn active(&self) -> usize {
        self.active_count()
    }
}

struct AgentTurnConcurrencyLease {
    state: Arc<Mutex<AgentTurnConcurrencyState>>,
    changed: Arc<Notify>,
    phase: AtomicU8,
}

#[repr(u8)]
enum AgentTurnConcurrencyPermitPhase {
    Reserved = 0,
    Active = 1,
}

impl Drop for AgentTurnConcurrencyLease {
    fn drop(&mut self) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if self.phase.load(Ordering::Acquire) == AgentTurnConcurrencyPermitPhase::Active as u8 {
            state.active = state.active.saturating_sub(1);
        } else {
            state.reserved = state.reserved.saturating_sub(1);
        }
        drop(state);
        self.changed.notify_waiters();
    }
}

/// Cloneable ownership of one counted slot. AgentService retains one clone across approval
/// continuation while Dispatcher retains another until Wake/result settlement commits. The slot
/// is returned only after the last clone disappears.
#[derive(Clone)]
pub(crate) struct AgentTurnConcurrencyPermit(Arc<AgentTurnConcurrencyLease>);

impl AgentTurnConcurrencyPermit {
    fn activate_dispatch_reservation(&self) {
        if self
            .0
            .phase
            .compare_exchange(
                AgentTurnConcurrencyPermitPhase::Reserved as u8,
                AgentTurnConcurrencyPermitPhase::Active as u8,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_err()
        {
            return;
        }
        let mut state = self
            .0
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        state.reserved = state.reserved.saturating_sub(1);
        state.active = state.active.saturating_add(1);
        drop(state);
        self.0.changed.notify_waiters();
    }
}

impl std::fmt::Debug for AgentTurnConcurrencyPermit {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("AgentTurnConcurrencyPermit(..)")
    }
}

#[derive(Debug, Clone)]
pub(crate) struct AgentDispatcherConfig {
    pub(crate) global_concurrency_limit: usize,
    pub(crate) wake_lease_renew_interval: Duration,
    pub(crate) idle_poll_interval: Duration,
    pub(crate) shutdown_grace: Duration,
}

impl Default for AgentDispatcherConfig {
    fn default() -> Self {
        Self {
            global_concurrency_limit: DEFAULT_AGENT_GLOBAL_CONCURRENCY,
            wake_lease_renew_interval: DEFAULT_WAKE_LEASE_RENEW_INTERVAL,
            idle_poll_interval: DEFAULT_DISPATCH_IDLE_POLL_INTERVAL,
            shutdown_grace: DEFAULT_DISPATCH_SHUTDOWN_GRACE,
        }
    }
}

impl AgentDispatcherConfig {
    fn validate(&self) -> Result<(), AgentDispatcherError> {
        if self.global_concurrency_limit == 0 {
            return Err(AgentDispatcherError::Configuration(
                "global Agent concurrency limit must be greater than zero".to_string(),
            ));
        }
        if self.wake_lease_renew_interval.is_zero() {
            return Err(AgentDispatcherError::Configuration(
                "Wake lease renewal interval must be greater than zero".to_string(),
            ));
        }
        if self.idle_poll_interval.is_zero() {
            return Err(AgentDispatcherError::Configuration(
                "dispatcher fallback poll interval must be greater than zero".to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentWakeExecutionHandle {
    pub(crate) wake_id: String,
    pub(crate) claim_token: String,
    pub(crate) run_id: String,
    pub(crate) conversation_id: String,
    pub(crate) assistant_message_id: String,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum AgentWakeExecutionObservation {
    WaitingForApproval,
    RunningAfterApproval,
    Terminal {
        status: AgentWakeStatus,
        artifact_refs: Vec<AgentResultArtifactReference>,
        terminal_error: Option<String>,
    },
}

#[derive(Debug, Clone)]
pub(crate) struct AgentWakeSettlement {
    pub(crate) wake_id: String,
    pub(crate) expected_status: AgentWakeStatus,
    pub(crate) claim_token: String,
    pub(crate) terminal_status: AgentWakeStatus,
    pub(crate) run_id: Option<String>,
    pub(crate) assistant_message_id: Option<String>,
    pub(crate) artifact_refs: Vec<AgentResultArtifactReference>,
    pub(crate) terminal_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AgentInterruptDisposition {
    NoPendingExecution,
    QueuedWakeCancelled { wake_id: String },
    ActiveTurn { wake_id: String, run_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentInterruptRequest {
    disposition: AgentInterruptDisposition,
    requires_runtime_dispatch: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TreeStoppedWakeSettlement {
    Interrupted,
    AlreadyTerminal,
    LostOwnership,
    UnsafeActiveAction { count: u64 },
}

/// The durable coordination boundary used by the scheduler.
///
/// `claim_next_dispatchable` selects by Wake sequence among eligible roots and atomically skips Agents
/// which already own a claimed/running/waiting Wake or an in-progress Conversation trace. Thus
/// acquiring the process permit before this method establishes the fixed order
/// `global permit -> durable per-Agent execution right`. The production store also skips roots
/// with a claimed preparation so one tree cannot fill the shared preparation worker queue.
pub(crate) trait AgentDispatcherStore: Send + Sync {
    fn list_undispatched_interrupts(
        &self,
    ) -> Result<Vec<mycopilot_core::UndispatchedAgentInterrupt>, String>;

    fn recover_orphaned_wakes(
        &self,
        recovery_token: &str,
        now_ms: i64,
    ) -> Result<AgentWakeRecoveryBatch, String>;

    fn claim_next_dispatchable(
        &self,
        claim_token: &str,
        now_ms: i64,
    ) -> Result<Option<AgentWakeRequestRecord>, AgentDispatchClaimError>;

    fn get_wake(&self, wake_id: &str) -> Result<Option<AgentWakeRequestRecord>, String>;

    fn mark_waiting_for_approval(
        &self,
        wake_id: &str,
        run_id: &str,
        conversation_id: &str,
        assistant_message_id: &str,
        claim_token: &str,
        now_ms: i64,
    ) -> Result<AgentWakeApprovalWaitOutcome, String>;

    fn renew_lease(&self, wake_id: &str, claim_token: &str, now_ms: i64) -> Result<(), String>;

    fn settle(&self, settlement: &AgentWakeSettlement, now_ms: i64) -> Result<(), String>;

    fn settle_tree_stopped_wake(
        &self,
        wake: &mycopilot_core::ActiveAgentTreeWake,
        now_ms: i64,
    ) -> Result<TreeStoppedWakeSettlement, String>;

    fn interrupt_descendant(
        &self,
        caller_agent_id: &str,
        target_agent_id: &str,
        request_id: &str,
        now_ms: i64,
    ) -> Result<AgentInterruptRequest, String>;

    fn mark_interrupt_dispatched(
        &self,
        caller_agent_id: &str,
        request_id: &str,
        now_ms: i64,
    ) -> Result<(), String>;
}

/// Separates a candidate-level projection/lease race from a database failure. A transient
/// conflict must not stop the manager, while corruption or unavailable storage must remain
/// visible instead of being retried forever as if it were normal contention.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AgentDispatchClaimError {
    NotReady(String),
    Fatal(String),
}

impl std::fmt::Display for AgentDispatchClaimError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotReady(message) => write!(formatter, "Wake candidate is not ready: {message}"),
            Self::Fatal(message) => write!(formatter, "Wake claim storage failed: {message}"),
        }
    }
}

/// Production persistence adapter. Every method delegates to the canonical `StorageService`
/// transaction/repository path; the dispatcher never caches a second Wake or Mailbox truth.
pub(crate) struct SqliteAgentDispatcherStore {
    storage: Arc<mycopilot_core::storage::service::StorageService>,
    wait_notifications: crate::application::agent_wait::AgentWaitNotifications,
}

impl SqliteAgentDispatcherStore {
    pub(crate) fn new(storage: Arc<mycopilot_core::storage::service::StorageService>) -> Self {
        Self {
            storage,
            wait_notifications: crate::application::agent_wait::shared_agent_wait_notifications(),
        }
    }
}

impl AgentDispatcherStore for SqliteAgentDispatcherStore {
    fn list_undispatched_interrupts(
        &self,
    ) -> Result<Vec<mycopilot_core::UndispatchedAgentInterrupt>, String> {
        self.storage
            .list_undispatched_agent_interrupts()
            .map_err(|error| error.to_string())
    }

    fn recover_orphaned_wakes(
        &self,
        recovery_token: &str,
        now_ms: i64,
    ) -> Result<AgentWakeRecoveryBatch, String> {
        self.storage
            .recover_agent_wakes_at(recovery_token, now_ms)
            .map_err(|error| error.to_string())
    }

    fn claim_next_dispatchable(
        &self,
        claim_token: &str,
        _now_ms: i64,
    ) -> Result<Option<AgentWakeRequestRecord>, AgentDispatchClaimError> {
        let wake = self
            .storage
            .claim_next_fair_dispatchable_agent_wake(claim_token)
            .map_err(|error| match error {
                AgentGraphError::Conflict(message) => AgentDispatchClaimError::NotReady(message),
                error => AgentDispatchClaimError::Fatal(error.to_string()),
            })?;
        if let Some(wake) = &wake {
            self.wait_notifications
                .notify_caller(&wake.requester_agent_id);
        }
        Ok(wake)
    }

    fn get_wake(&self, wake_id: &str) -> Result<Option<AgentWakeRequestRecord>, String> {
        self.storage
            .get_agent_wake(wake_id)
            .map_err(|error| error.to_string())
    }

    fn mark_waiting_for_approval(
        &self,
        wake_id: &str,
        run_id: &str,
        conversation_id: &str,
        assistant_message_id: &str,
        claim_token: &str,
        _now_ms: i64,
    ) -> Result<AgentWakeApprovalWaitOutcome, String> {
        let outcome = self.storage.mark_agent_wake_waiting_for_pending_approval(
            wake_id,
            run_id,
            conversation_id,
            assistant_message_id,
            claim_token,
        )?;
        if let AgentWakeApprovalWaitOutcome::Waiting(wake) = &outcome {
            self.wait_notifications
                .notify_caller(&wake.requester_agent_id);
        }
        Ok(outcome)
    }

    fn renew_lease(&self, wake_id: &str, claim_token: &str, _now_ms: i64) -> Result<(), String> {
        // Production renewal obtains its current time after acquiring the SQLite connection.
        // The trait clock remains available to deterministic fake stores only.
        self.storage
            .renew_agent_wake_lease(wake_id, claim_token)
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    fn settle(&self, settlement: &AgentWakeSettlement, _now_ms: i64) -> Result<(), String> {
        let result = self
            .storage
            .finish_agent_turn_with_result(&FinishAgentTurnResultInput {
                wake_id: settlement.wake_id.clone(),
                expected_status: settlement.expected_status,
                claim_token: settlement.claim_token.clone(),
                terminal_status: settlement.terminal_status,
                run_id: settlement.run_id.clone(),
                assistant_message_id: settlement.assistant_message_id.clone(),
                terminal_error: settlement.terminal_error.clone(),
            })
            .map_err(|error| error.to_string())?;
        self.wait_notifications
            .notify_caller(&result.result_message.recipient_agent_id);
        Ok(())
    }

    fn settle_tree_stopped_wake(
        &self,
        wake: &mycopilot_core::ActiveAgentTreeWake,
        now_ms: i64,
    ) -> Result<TreeStoppedWakeSettlement, String> {
        match self
            .storage
            .settle_tree_stopped_active_wake_at(wake, now_ms)
            .map_err(|error| error.to_string())?
        {
            mycopilot_core::AgentTreeStoppedWakeSettlementOutcome::Interrupted(settlement) => {
                self.wait_notifications
                    .notify_caller(&settlement.result_message.recipient_agent_id);
                Ok(TreeStoppedWakeSettlement::Interrupted)
            }
            mycopilot_core::AgentTreeStoppedWakeSettlementOutcome::AlreadyTerminal => {
                Ok(TreeStoppedWakeSettlement::AlreadyTerminal)
            }
            mycopilot_core::AgentTreeStoppedWakeSettlementOutcome::LostOwnership => {
                Ok(TreeStoppedWakeSettlement::LostOwnership)
            }
            mycopilot_core::AgentTreeStoppedWakeSettlementOutcome::UnsafeActiveAction { count } => {
                Ok(TreeStoppedWakeSettlement::UnsafeActiveAction { count })
            }
        }
    }

    fn interrupt_descendant(
        &self,
        caller_agent_id: &str,
        target_agent_id: &str,
        request_id: &str,
        now_ms: i64,
    ) -> Result<AgentInterruptRequest, String> {
        let receipt = self
            .storage
            .interrupt_agent_execution_with_receipt_at(
                caller_agent_id,
                target_agent_id,
                request_id,
                now_ms,
            )
            .map_err(|error| error.to_string())?;
        self.wait_notifications.notify_caller(caller_agent_id);
        let requires_runtime_dispatch = receipt.dispatched_at.is_none()
            && matches!(
                &receipt.outcome,
                InterruptAgentExecutionOutcome::ActiveTurn { .. }
            );
        let disposition = match receipt.outcome {
            InterruptAgentExecutionOutcome::NoPendingExecution => {
                AgentInterruptDisposition::NoPendingExecution
            }
            InterruptAgentExecutionOutcome::QueuedWakeCancelled { wake_id } => {
                AgentInterruptDisposition::QueuedWakeCancelled { wake_id }
            }
            InterruptAgentExecutionOutcome::ActiveTurn { wake_id, run_id } => {
                AgentInterruptDisposition::ActiveTurn { wake_id, run_id }
            }
        };
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
        self.storage
            .mark_agent_interrupt_dispatched_at(caller_agent_id, request_id, now_ms)
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}

/// Narrow adapter around the one shared Turn executor.
///
/// A production implementation starts `AgentTurnStart::AgentWake`, then observes only durable
/// Conversation trace/pending-action facts. Renderer events are intentionally absent here.
pub(crate) trait AgentWakeTurnExecutionPort: Send + Sync {
    fn blocking_work(&self) -> Option<BlockingWork> {
        None
    }

    fn reconcile_execution_fences(&self) -> Result<(), String> {
        Ok(())
    }

    fn renew_claim_lease(
        &self,
        store: &dyn AgentDispatcherStore,
        wake_id: &str,
        claim_token: &str,
        now_ms: i64,
    ) -> Result<(), String> {
        store.renew_lease(wake_id, claim_token, now_ms)
    }

    fn interrupt_owned(&self, handle: &AgentWakeExecutionHandle) -> Result<(), String> {
        self.interrupt(&handle.run_id).map(|_| ())
    }

    fn start(
        &self,
        wake: &AgentWakeRequestRecord,
        permit: AgentTurnConcurrencyPermit,
    ) -> Result<AgentWakeExecutionHandle, String>;

    fn observe<'a>(
        &'a self,
        handle: &'a AgentWakeExecutionHandle,
        waiting_already_reported: bool,
    ) -> DispatchFuture<'a, Result<AgentWakeExecutionObservation, String>>;

    fn recovered_handle(
        &self,
        wake: &AgentWakeRequestRecord,
    ) -> Result<AgentWakeExecutionHandle, String>;

    fn retain_recovered_permit(&self, run_id: &str) -> Option<AgentTurnConcurrencyPermit>;

    fn retire_recovered_turn_after_settlement(&self, handle: &AgentWakeExecutionHandle);

    fn interrupt(&self, run_id: &str) -> Result<AgentRunCancellationOutcome, String>;
}

/// Production adapter for the shared `AgentService` executor. Its observer uses the durable
/// Conversation trace and pending-action rows; the private notification hub only avoids polling
/// latency and is never trusted as evidence.
#[derive(Clone)]
pub(crate) struct SharedAgentTurnExecutionPort {
    service: crate::application::agent::AgentService,
    storage: Arc<mycopilot_core::storage::service::StorageService>,
    collaboration: crate::application::agent_collaboration::ChildAgentFactory,
    notifications: crate::application::agent::CoreServerNotificationSender,
    fallback_poll_interval: Duration,
}

impl SharedAgentTurnExecutionPort {
    pub(crate) fn new(
        service: crate::application::agent::AgentService,
        storage: Arc<mycopilot_core::storage::service::StorageService>,
        notifications: crate::application::agent::CoreServerNotificationSender,
    ) -> Self {
        Self {
            collaboration: crate::application::agent_collaboration::ChildAgentFactory::new(
                Arc::clone(&storage),
            ),
            service,
            storage,
            notifications,
            fallback_poll_interval: DEFAULT_DISPATCH_IDLE_POLL_INTERVAL,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_fallback_poll_interval(mut self, interval: Duration) -> Self {
        self.fallback_poll_interval = interval;
        self
    }

    async fn inspect_durable_async(
        &self,
        handle: &AgentWakeExecutionHandle,
        waiting_already_reported: bool,
    ) -> Result<Option<AgentWakeExecutionObservation>, String> {
        let port = self.clone();
        let handle = handle.clone();
        self.service
            .blocking_work
            .maintenance
            .reserve(DISPATCH_MAINTENANCE_ESTIMATED_BYTES)
            .await
            .map_err(|error| error.to_string())?
            .submit(move || port.inspect_durable(&handle, waiting_already_reported))
            .wait()
            .await
            .map_err(|error| error.to_string())?
    }

    fn inspect_durable(
        &self,
        handle: &AgentWakeExecutionHandle,
        waiting_already_reported: bool,
    ) -> Result<Option<AgentWakeExecutionObservation>, String> {
        if self.service.agent_wake_execution_refused(
            &handle.wake_id,
            &handle.claim_token,
            &handle.run_id,
            &handle.assistant_message_id,
        ) {
            return Err(
                "child Runtime refused this execution claim; durable recovery owns its outcome"
                    .into(),
            );
        }
        let trace = self
            .storage
            .get_conversation_turn_trace(&handle.assistant_message_id)?;
        let Some(trace) = trace else {
            return Ok(None);
        };
        if trace.run_id != handle.run_id || trace.conversation_id != handle.conversation_id {
            return Err("durable Turn identity does not match the dispatched Wake".to_string());
        }
        match trace.terminal_status {
            ConversationTurnTraceTerminalStatus::InProgress => {
                let has_pending_approval =
                    self.storage
                        .list_pending_agent_actions()?
                        .iter()
                        .any(|action| {
                            action.run_id == handle.run_id
                                && action.conversation_id.as_deref()
                                    == Some(handle.conversation_id.as_str())
                                && action.assistant_message_id.as_deref()
                                    == Some(handle.assistant_message_id.as_str())
                                && action.status == "pending"
                        });
                if !waiting_already_reported && has_pending_approval {
                    return Ok(Some(AgentWakeExecutionObservation::WaitingForApproval));
                }
                if waiting_already_reported && !has_pending_approval {
                    let wake = self
                        .storage
                        .get_agent_wake(&handle.wake_id)
                        .map_err(|error| error.to_string())?
                        .ok_or_else(|| "approval-resumed Wake is missing".to_string())?;
                    if wake.status == AgentWakeStatus::Running {
                        return Ok(Some(AgentWakeExecutionObservation::RunningAfterApproval));
                    }
                }
                Ok(None)
            }
            status => {
                // The durable trace owns terminal facts. The final assistant reply stays in the
                // child conversation; only explicit send_message calls deliver its work to a parent.
                let wake_status = match status {
                    ConversationTurnTraceTerminalStatus::Completed => AgentWakeStatus::Completed,
                    ConversationTurnTraceTerminalStatus::Failed => AgentWakeStatus::Failed,
                    // `Cancelled` is reserved for a queued/claimed Wake which never crossed the
                    // Turn dispatch boundary. Once a Turn exists, cancellation is an interrupted
                    // delegated execution and must still emit the direct-parent result Outbox.
                    ConversationTurnTraceTerminalStatus::Cancelled => AgentWakeStatus::Interrupted,
                    ConversationTurnTraceTerminalStatus::InProgress => unreachable!(),
                };
                let terminal_error = (wake_status != AgentWakeStatus::Completed)
                    .then_some(trace.terminal_error.as_deref())
                    .flatten()
                    .map(str::trim)
                    .filter(|error| !error.is_empty())
                    .map(|error| {
                        bounded_utf8_with_suffix(
                            error,
                            mycopilot_core::AGENT_RESULT_TERMINAL_ERROR_MAX_BYTES,
                            " [已截断]",
                        )
                    });
                Ok(Some(AgentWakeExecutionObservation::Terminal {
                    status: wake_status,
                    // Managed Artifact references are delivered by the typed collaboration
                    // settlement service; ordinary chat attachments are not guessed as Artifacts.
                    artifact_refs: Vec::new(),
                    terminal_error,
                }))
            }
        }
    }
}

fn bounded_utf8_with_suffix(value: &str, maximum_bytes: usize, suffix: &str) -> String {
    if value.len() <= maximum_bytes {
        return value.to_string();
    }
    let suffix = if suffix.len() < maximum_bytes {
        suffix
    } else {
        ""
    };
    let mut boundary = maximum_bytes.saturating_sub(suffix.len()).min(value.len());
    while boundary > 0 && !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    let mut output = String::with_capacity(boundary + suffix.len());
    output.push_str(&value[..boundary]);
    output.push_str(suffix);
    output
}

impl AgentWakeTurnExecutionPort for SharedAgentTurnExecutionPort {
    fn blocking_work(&self) -> Option<BlockingWork> {
        Some(self.service.blocking_work.clone())
    }

    fn reconcile_execution_fences(&self) -> Result<(), String> {
        self.service.reconcile_agent_wake_execution_fences()
    }

    fn renew_claim_lease(
        &self,
        store: &dyn AgentDispatcherStore,
        wake_id: &str,
        claim_token: &str,
        now_ms: i64,
    ) -> Result<(), String> {
        self.service
            .renew_unfenced_agent_wake_lease(wake_id, claim_token, || {
                store.renew_lease(wake_id, claim_token, now_ms)
            })
    }

    fn interrupt_owned(&self, handle: &AgentWakeExecutionHandle) -> Result<(), String> {
        self.service.cancel_owned_agent_wake_runtime(
            &handle.wake_id,
            &handle.claim_token,
            &handle.run_id,
            &handle.assistant_message_id,
        );
        Ok(())
    }

    fn start(
        &self,
        wake: &AgentWakeRequestRecord,
        permit: AgentTurnConcurrencyPermit,
    ) -> Result<AgentWakeExecutionHandle, String> {
        let source_message_id = wake
            .source_agent_message_id
            .as_deref()
            .ok_or_else(|| "dispatchable child Wake has no source Mailbox message".to_string())?;
        let claim_token = wake
            .claim_token
            .as_deref()
            .ok_or_else(|| "dispatchable child Wake has no claim token".to_string())?;
        let spawn = self
            .collaboration
            .resolve_trusted_claimed_wake(
                &wake.agent_id,
                &wake.wake_id,
                source_message_id,
                claim_token,
            )
            .map_err(|error| error.to_string())?;
        let trusted = crate::application::agent::TrustedAgentWakeTurnStart::new(
            wake.wake_id.clone(),
            wake.agent_id.clone(),
            spawn.agent.conversation_id.clone(),
            source_message_id.to_string(),
            claim_token.to_string(),
            spawn.collaboration_identity,
        )
        .map_err(|error| error.to_string())?
        .with_global_permit(permit);
        let output = self
            .service
            .execute_turn(
                crate::application::agent::AgentTurnStart::AgentWake(trusted),
                self.notifications.clone(),
            )
            .map_err(|error| error.to_string())?;
        Ok(AgentWakeExecutionHandle {
            wake_id: wake.wake_id.clone(),
            claim_token: claim_token.to_string(),
            run_id: output.run_id,
            conversation_id: output.conversation_id,
            assistant_message_id: output.assistant_message_id,
        })
    }

    fn observe<'a>(
        &'a self,
        handle: &'a AgentWakeExecutionHandle,
        waiting_already_reported: bool,
    ) -> DispatchFuture<'a, Result<AgentWakeExecutionObservation, String>> {
        Box::pin(async move {
            loop {
                if let Some(observation) = self
                    .inspect_durable_async(handle, waiting_already_reported)
                    .await?
                {
                    return Ok(observation);
                }
                let notification = self
                    .service
                    .durable_turn_notification(&handle.assistant_message_id);
                let notified = notification.notified();
                tokio::pin!(notified);
                // The second check closes the check/subscribe race. A bounded tick also makes a
                // fresh process observe commits published by a previous Host.
                if let Some(observation) = self
                    .inspect_durable_async(handle, waiting_already_reported)
                    .await?
                {
                    return Ok(observation);
                }
                tokio::select! {
                    _ = &mut notified => {}
                    _ = tokio::time::sleep(self.fallback_poll_interval) => {}
                }
            }
        })
    }

    fn recovered_handle(
        &self,
        wake: &AgentWakeRequestRecord,
    ) -> Result<AgentWakeExecutionHandle, String> {
        let claim_token = wake
            .claim_token
            .clone()
            .ok_or_else(|| "recovered Wake has no claim token".to_string())?;
        if self.service.agent_wake_execution_refused(
            &wake.wake_id,
            &claim_token,
            wake.run_id.as_deref().unwrap_or_default(),
            wake.assistant_message_id.as_deref().unwrap_or_default(),
        ) {
            return Err(
                "refused child Runtime claim cannot be reattached before durable recovery".into(),
            );
        }
        let agent = self
            .storage
            .get_agent_node(&wake.agent_id)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("recovered Wake {} Agent is missing", wake.wake_id))?;
        Ok(AgentWakeExecutionHandle {
            wake_id: wake.wake_id.clone(),
            claim_token,
            run_id: wake
                .run_id
                .clone()
                .ok_or_else(|| format!("recovered Wake {} has no run identity", wake.wake_id))?,
            conversation_id: agent.conversation_id,
            assistant_message_id: wake.assistant_message_id.clone().ok_or_else(|| {
                format!("recovered Wake {} has no assistant identity", wake.wake_id)
            })?,
        })
    }

    fn retain_recovered_permit(&self, run_id: &str) -> Option<AgentTurnConcurrencyPermit> {
        self.service
            .retain_recovered_turn_concurrency_permit(run_id)
    }

    fn retire_recovered_turn_after_settlement(&self, handle: &AgentWakeExecutionHandle) {
        self.service.retire_recovered_turn_after_settlement(
            &handle.conversation_id,
            &handle.run_id,
            &handle.assistant_message_id,
        );
    }

    fn interrupt(&self, run_id: &str) -> Result<AgentRunCancellationOutcome, String> {
        self.service.interrupt_agent_wake_run(run_id)
    }
}

pub(crate) trait AgentDispatcherClock: Send + Sync {
    fn now_ms(&self) -> i64;
}

#[derive(Default)]
struct SystemAgentDispatcherClock;

impl AgentDispatcherClock for SystemAgentDispatcherClock {
    fn now_ms(&self) -> i64 {
        crate::application::agent_support::now_ms()
    }
}

#[derive(Debug)]
pub(crate) enum AgentDispatcherError {
    Configuration(String),
    Storage(String),
    Manager(String),
}

impl std::fmt::Display for AgentDispatcherError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Configuration(message) => {
                write!(formatter, "invalid Agent dispatcher: {message}")
            }
            Self::Storage(message) => {
                write!(formatter, "Agent dispatcher storage failed: {message}")
            }
            Self::Manager(message) => write!(formatter, "Agent dispatcher stopped: {message}"),
        }
    }
}

impl std::error::Error for AgentDispatcherError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct AgentDispatcherShutdownReport {
    pub(crate) settled_during_grace: usize,
    pub(crate) cancellation_requested: usize,
    pub(crate) unfinished_persisted: usize,
}

#[derive(Clone)]
struct RunningDispatch {
    wake_id: String,
    claim_token: Option<String>,
    run_id: Option<String>,
    waiting_for_approval: bool,
}

#[derive(Default)]
struct AgentDispatcherQuiescence {
    scan_in_progress: bool,
    durable_queue_idle: bool,
    work_generation: u64,
}

struct AgentDispatcherShared {
    store: Arc<dyn AgentDispatcherStore>,
    executor: Arc<dyn AgentWakeTurnExecutionPort>,
    blocking_work: BlockingWork,
    preparing: Mutex<std::collections::HashSet<String>>,
    clock: Arc<dyn AgentDispatcherClock>,
    turn_concurrency: AgentTurnConcurrencyGate,
    work_available: Notify,
    shutdown_requested: AtomicBool,
    running: Mutex<HashMap<String, RunningDispatch>>,
    quiescence: Mutex<AgentDispatcherQuiescence>,
    quiescence_changed: Notify,
    config: AgentDispatcherConfig,
}

// Temporary synchronous StorageService bridge. Claim projection/recovery may still scan data;
// renewal therefore owns a separate physical lane. Admission/cancellation may wait on global
// guards and must use their separate executors. Round 3 moves scans to the storage architecture.
async fn dispatcher_store<T: Send + 'static>(
    shared: &Arc<AgentDispatcherShared>,
    operation: impl FnOnce(&dyn AgentDispatcherStore, i64) -> Result<T, String> + Send + 'static,
) -> Result<T, AgentDispatcherError> {
    let worker = Arc::clone(shared);
    shared
        .blocking_work
        .maintenance
        .reserve(DISPATCH_MAINTENANCE_ESTIMATED_BYTES)
        .await
        .map_err(|error| AgentDispatcherError::Manager(error.to_string()))?
        .submit(move || operation(worker.store.as_ref(), worker.clock.now_ms()))
        .wait()
        .await
        .map_err(|error| AgentDispatcherError::Manager(error.to_string()))?
        .map_err(AgentDispatcherError::Storage)
}

async fn dispatcher_renew_lease(
    shared: &Arc<AgentDispatcherShared>,
    wake_id: String,
    claim_token: String,
) -> Result<(), AgentDispatcherError> {
    let worker = Arc::clone(shared);
    shared
        .blocking_work
        .lease
        .reserve(DISPATCH_MAINTENANCE_ESTIMATED_BYTES)
        .await
        .map_err(|error| AgentDispatcherError::Manager(error.to_string()))?
        .submit(move || {
            worker.executor.renew_claim_lease(
                worker.store.as_ref(),
                &wake_id,
                &claim_token,
                worker.clock.now_ms(),
            )
        })
        .wait()
        .await
        .map_err(|error| AgentDispatcherError::Manager(error.to_string()))?
        .map_err(AgentDispatcherError::Storage)
}

async fn dispatcher_control<T: Send + 'static>(
    shared: &Arc<AgentDispatcherShared>,
    operation: impl FnOnce(&dyn AgentWakeTurnExecutionPort) -> Result<T, String> + Send + 'static,
) -> Result<T, AgentDispatcherError> {
    let executor = Arc::clone(&shared.executor);
    shared
        .blocking_work
        .control
        .reserve(DISPATCH_MAINTENANCE_ESTIMATED_BYTES)
        .await
        .map_err(|error| AgentDispatcherError::Manager(error.to_string()))?
        .submit(move || operation(executor.as_ref()))
        .wait()
        .await
        .map_err(|error| AgentDispatcherError::Manager(error.to_string()))?
        .map_err(AgentDispatcherError::Manager)
}

struct PreparingDispatchGuard {
    claim_token: String,
    shared: Arc<AgentDispatcherShared>,
}

impl Drop for PreparingDispatchGuard {
    fn drop(&mut self) {
        self.shared
            .preparing
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&self.claim_token);
        self.shared.quiescence_changed.notify_waiters();
    }
}

/// Starts immediately after claim, independently of preparation capacity and locks. Dropping
/// the guard stops future ticks; an already-started durable renewal is still owned by the pool.
struct DispatchWakeLease {
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    failure: Arc<Mutex<Option<String>>>,
    failure_changed: Arc<Notify>,
}

impl DispatchWakeLease {
    fn start(shared: Arc<AgentDispatcherShared>, wake_id: String, claim_token: String) -> Self {
        let (stop, mut stopped) = tokio::sync::oneshot::channel();
        let failure = Arc::new(Mutex::new(None));
        let worker_failure = Arc::clone(&failure);
        let failure_changed = Arc::new(Notify::new());
        let worker_failure_changed = Arc::clone(&failure_changed);
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(shared.config.wake_lease_renew_interval);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            tick.tick().await;
            loop {
                tokio::select! {
                    biased;
                    _ = &mut stopped => return,
                    _ = tick.tick() => {}
                }
                let wake_id = wake_id.clone();
                let claim_token = claim_token.clone();
                if let Err(error) = dispatcher_renew_lease(&shared, wake_id, claim_token).await {
                    *worker_failure
                        .lock()
                        .unwrap_or_else(|error| error.into_inner()) = Some(error.to_string());
                    worker_failure_changed.notify_waiters();
                    return;
                }
            }
        });
        Self {
            stop: Some(stop),
            failure,
            failure_changed,
        }
    }

    async fn wait_failure(&self) -> String {
        loop {
            let changed = self.failure_changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if let Some(error) = self.failure() {
                return error;
            }
            changed.await;
        }
    }

    fn failure(&self) -> Option<String> {
        self.failure
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }
}

impl Drop for DispatchWakeLease {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }
}

/// Process owner for the persistent scheduler. Dropping or aborting it never deletes a Wake;
/// startup recovery derives the next action from SQLite.
pub(crate) struct AgentDispatcher {
    shared: Arc<AgentDispatcherShared>,
    manager: Option<JoinHandle<Result<usize, AgentDispatcherError>>>,
}

impl AgentDispatcher {
    /// Production construction always receives the one gate owned by `AgentService`; there is no
    /// public constructor which can accidentally create an independent child-only semaphore.
    pub(crate) fn start(
        store: Arc<dyn AgentDispatcherStore>,
        executor: Arc<dyn AgentWakeTurnExecutionPort>,
        gate: AgentTurnConcurrencyGate,
        config: AgentDispatcherConfig,
    ) -> Result<Self, AgentDispatcherError> {
        Self::start_with_clock(
            store,
            executor,
            Arc::new(SystemAgentDispatcherClock),
            gate,
            config,
        )
    }

    pub(crate) fn start_with_clock(
        store: Arc<dyn AgentDispatcherStore>,
        executor: Arc<dyn AgentWakeTurnExecutionPort>,
        clock: Arc<dyn AgentDispatcherClock>,
        gate: AgentTurnConcurrencyGate,
        config: AgentDispatcherConfig,
    ) -> Result<Self, AgentDispatcherError> {
        config.validate()?;
        if gate.limit() != config.global_concurrency_limit {
            return Err(AgentDispatcherError::Configuration(format!(
                "shared Turn gate limit {} does not match dispatcher limit {}",
                gate.limit(),
                config.global_concurrency_limit
            )));
        }
        let blocking_work = match executor.blocking_work() {
            Some(work) => work,
            None => BlockingWork::new()
                .map_err(|error| AgentDispatcherError::Configuration(error.to_string()))?,
        };
        let shared = Arc::new(AgentDispatcherShared {
            blocking_work,
            preparing: Mutex::new(std::collections::HashSet::new()),
            store,
            executor,
            clock,
            turn_concurrency: gate,
            work_available: Notify::new(),
            shutdown_requested: AtomicBool::new(false),
            running: Mutex::new(HashMap::new()),
            quiescence: Mutex::new(AgentDispatcherQuiescence::default()),
            quiescence_changed: Notify::new(),
            config,
        });
        let manager_shared = Arc::clone(&shared);
        let manager = tokio::spawn(async move { run_agent_dispatcher(manager_shared).await });
        Ok(Self {
            shared,
            manager: Some(manager),
        })
    }

    /// Low-latency hint after a committed Wake/message transition. Correctness never depends on
    /// this notification because the manager also scans the durable queue.
    pub(crate) fn notify_work_available(&self) {
        notify_dispatcher_work_available(&self.shared);
    }

    /// Waits until the Dispatcher has observed an empty durable queue after every worker/result
    /// settlement and no root/child logical Turn remains active. The manager's empty-queue claim
    /// reservation is deliberately distinct from an active Turn, closing the transient-zero race
    /// between a worker dropping its permit and the manager's next durable scan.
    pub(crate) async fn wait_until_turns_settled(&self) {
        loop {
            let dispatcher_changed = self.shared.quiescence_changed.notified();
            let gate_changed = self.shared.turn_concurrency.changed.notified();
            tokio::pin!(dispatcher_changed, gate_changed);
            let quiescent = {
                let state = self
                    .shared
                    .quiescence
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                !state.scan_in_progress
                    && state.durable_queue_idle
                    && self.shared.turn_concurrency.active_count() == 0
                    && self
                        .shared
                        .running
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .is_empty()
            };
            if quiescent {
                return;
            }
            tokio::select! {
                _ = &mut dispatcher_changed => {}
                _ = &mut gate_changed => {}
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn observed_waiting_for_approval(&self, agent_id: &str) -> Option<bool> {
        self.shared
            .running
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(agent_id)
            .map(|dispatch| dispatch.waiting_for_approval)
    }

    #[cfg(test)]
    pub(crate) fn shutdown_requested(&self) -> bool {
        self.shared.shutdown_requested.load(Ordering::Acquire)
    }

    pub(crate) fn interrupt_agent(
        &self,
        caller_agent_id: &str,
        target_agent_id: &str,
        request_id: &str,
    ) -> Result<AgentInterruptDisposition, AgentDispatcherError> {
        let interrupt_request = self
            .shared
            .store
            .interrupt_descendant(
                caller_agent_id,
                target_agent_id,
                request_id,
                self.shared.clock.now_ms(),
            )
            .map_err(AgentDispatcherError::Storage)?;
        let mut disposition = interrupt_request.disposition;
        if interrupt_request.requires_runtime_dispatch {
            let AgentInterruptDisposition::ActiveTurn { wake_id, run_id } = &disposition else {
                return Err(AgentDispatcherError::Storage(
                    "Only an active-Turn interrupt may require runtime dispatch".to_string(),
                ));
            };
            let interrupt_outcome = self
                .shared
                .executor
                .interrupt(run_id)
                .map_err(AgentDispatcherError::Manager)?;
            if !interrupt_outcome.turn_termination_confirmed() {
                let wake = self
                    .shared
                    .store
                    .get_wake(wake_id)
                    .map_err(AgentDispatcherError::Storage)?
                    .ok_or_else(|| {
                        AgentDispatcherError::Storage(format!(
                            "Wake {wake_id} disappeared after interrupt missed Turn {run_id}"
                        ))
                    })?;
                if wake.status.is_terminal() {
                    // The Turn may settle between the durable interrupt lookup and the
                    // process-local cancellation delivery. In that race there is no remaining
                    // execution to interrupt, so report the existing no-active-Turn contract.
                    disposition = AgentInterruptDisposition::NoPendingExecution;
                } else {
                    return Err(AgentDispatcherError::Manager(format!(
                        "interrupt did not reach active Turn {run_id} for Wake {wake_id}; durable Wake status remains {}",
                        wake.status.as_str()
                    )));
                }
            } else {
                self.shared
                    .store
                    .mark_interrupt_dispatched(
                        caller_agent_id,
                        request_id,
                        self.shared.clock.now_ms(),
                    )
                    .map_err(AgentDispatcherError::Storage)?;
            }
        }
        notify_dispatcher_work_available(&self.shared);
        Ok(disposition)
    }

    /// Stops new claims, gives active Turns a bounded grace period, then requests cancellation of
    /// live non-approval Runs. Waiting approvals remain durable and are never guessed cancelled.
    pub(crate) async fn shutdown(
        mut self,
    ) -> Result<AgentDispatcherShutdownReport, AgentDispatcherError> {
        self.shared
            .shutdown_requested
            .store(true, Ordering::Release);
        self.shared.work_available.notify_waiters();
        let Some(mut manager) = self.manager.take() else {
            return Ok(AgentDispatcherShutdownReport::default());
        };

        if let Ok(result) =
            tokio::time::timeout(self.shared.config.shutdown_grace, &mut manager).await
        {
            return result
                .map_err(|error| AgentDispatcherError::Manager(error.to_string()))?
                .map(|settled_during_grace| AgentDispatcherShutdownReport {
                    settled_during_grace,
                    ..AgentDispatcherShutdownReport::default()
                });
        }

        let mut cancellation_requested = 0usize;
        let mut interrupted_runs = std::collections::HashSet::<String>::new();
        let cancellation_deadline = tokio::time::Instant::now() + self.shared.config.shutdown_grace;
        loop {
            // Approval can resume after shutdown begins. Re-scan until the bounded deadline so a
            // Waiting->Running CAS which loses the first snapshot race is still interrupted.
            let running = self
                .shared
                .running
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .values()
                .cloned()
                .collect::<Vec<_>>();
            for dispatch in &running {
                let Some(run_id) = dispatch.run_id.as_deref() else {
                    continue;
                };
                if !dispatch.waiting_for_approval && !interrupted_runs.contains(run_id) {
                    let cancel_run = run_id.to_string();
                    let effected = dispatcher_control(&self.shared, move |executor| {
                        executor.interrupt(&cancel_run)
                    })
                    .await
                    .map(AgentRunCancellationOutcome::any_effect)
                    .unwrap_or(false);
                    if !effected {
                        continue;
                    }
                    interrupted_runs.insert(run_id.to_string());
                    cancellation_requested = cancellation_requested.saturating_add(1);
                }
            }
            if manager.is_finished() {
                let settled_during_grace = manager
                    .await
                    .map_err(|error| AgentDispatcherError::Manager(error.to_string()))??;
                return Ok(AgentDispatcherShutdownReport {
                    settled_during_grace,
                    cancellation_requested,
                    unfinished_persisted: 0,
                });
            }
            let now = tokio::time::Instant::now();
            let preparing = !self
                .shared
                .preparing
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .is_empty();
            let scan_in_progress = self
                .shared
                .quiescence
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .scan_in_progress;
            if now >= cancellation_deadline && !preparing && !scan_in_progress {
                break;
            }
            if now >= cancellation_deadline {
                // A started synchronous preparation/claim may still commit. Keep its observer,
                // renewal and exact permit alive until its real result can be handed off.
                tokio::time::sleep(Duration::from_millis(10)).await;
                continue;
            }
            tokio::time::sleep(
                self.shared
                    .config
                    .idle_poll_interval
                    .min(cancellation_deadline.saturating_duration_since(now)),
            )
            .await;
        }

        // Dropping the manager aborts only process observers. Running/waiting Wake and Turn facts
        // remain in SQLite for conservative startup recovery.
        manager.abort();
        let unfinished_persisted = self
            .shared
            .running
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .len();
        Ok(AgentDispatcherShutdownReport {
            settled_during_grace: 0,
            cancellation_requested,
            unfinished_persisted,
        })
    }
}

async fn run_agent_dispatcher(
    shared: Arc<AgentDispatcherShared>,
) -> Result<usize, AgentDispatcherError> {
    let mut workers = JoinSet::<Result<(), AgentDispatcherError>>::new();
    let mut result = run_agent_dispatcher_loop(Arc::clone(&shared), &mut workers).await;
    if result.is_err() {
        // A failed claim/recovery scan is also shutdown. Dropping JoinSet here would detach
        // a non-abortable synchronous start and lose its lease/permit before its real result.
        shared.shutdown_requested.store(true, Ordering::Release);
        shared.work_available.notify_waiters();
        loop {
            let changed = shared.quiescence_changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if shared
                .preparing
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .is_empty()
            {
                break;
            }
            tokio::select! {
                _ = &mut changed => {}
                completion = workers.join_next(), if !workers.is_empty() => {
                    match completion {
                        Some(Err(error)) => eprintln!("Agent dispatcher worker stopped while draining failed manager: {error}"),
                        Some(Ok(Err(error))) => eprintln!("Agent dispatcher could not settle a worker while draining failed manager: {error}"),
                        _ => {}
                    }
                }
            }
        }
        // A scan failure also owns previously started Turns, not only preparing ones. Keep
        // approval waits durable, but deliver cancellation to every known live Run before its
        // observer/renewal owner disappears. Report failures alongside the original scan error.
        let running = shared
            .running
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .values()
            .filter(|dispatch| !dispatch.waiting_for_approval)
            .cloned()
            .collect::<Vec<_>>();
        let mut cancellation_errors = Vec::new();
        for dispatch in running {
            let Some(run_id) = dispatch.run_id else {
                continue;
            };
            let cancel_run = run_id.clone();
            match dispatcher_control(&shared, move |executor| executor.interrupt(&cancel_run)).await
            {
                Ok(outcome) if outcome.turn_termination_confirmed() => {}
                Ok(_) => {
                    let wake_id = dispatch.wake_id.clone();
                    match dispatcher_store(&shared, move |store, _| store.get_wake(&wake_id)).await {
                        Ok(Some(wake)) if wake.status.is_terminal() => {}
                        Ok(Some(wake)) => cancellation_errors.push(format!(
                            "Run {run_id} cancellation was not confirmed; Wake {} remains {}",
                            dispatch.wake_id, wake.status.as_str()
                        )),
                        Ok(None) => cancellation_errors.push(format!("Run {run_id} cancellation was not confirmed; Wake {} is missing", dispatch.wake_id)),
                        Err(error) => cancellation_errors.push(format!("Run {run_id} cancellation was not confirmed; Wake lookup failed: {error}")),
                    }
                }
                Err(error) => cancellation_errors.push(format!("Run {run_id}: {error}")),
            }
        }
        if !cancellation_errors.is_empty() {
            result = Err(AgentDispatcherError::Manager(format!(
                "{}; failed-manager cancellation incomplete (durable recovery required): {}",
                result.as_ref().unwrap_err(),
                cancellation_errors.join("; ")
            )));
        }
        // Every start is now rejected before execution or has published/cancelled its actual
        // Run. Only observers remain abortable; durable facts stay available for recovery.
        workers.shutdown().await;
        shared
            .quiescence
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .scan_in_progress = false;
        shared.quiescence_changed.notify_waiters();
    }
    result
}

async fn run_agent_dispatcher_loop(
    shared: Arc<AgentDispatcherShared>,
    workers: &mut JoinSet<Result<(), AgentDispatcherError>>,
) -> Result<usize, AgentDispatcherError> {
    let mut settled = 0usize;
    let recovery_generation = uuid::Uuid::new_v4().to_string();
    loop {
        let scan_generation = begin_dispatcher_scan(&shared);
        // This scan is intentionally repeated. A replacement Host may start before the old
        // process lease expires; the bounded fallback tick later reaches the deadline and
        // recovers it without requiring another restart or an in-memory notification.
        if !shared.shutdown_requested.load(Ordering::Acquire) {
            let execution = Arc::clone(&shared.executor);
            dispatcher_store(&shared, move |_, _| execution.reconcile_execution_fences()).await?;
            recover_undispatched_interrupts(&shared).await?;
            let recovery_token = format!(
                "agent-recovery:{recovery_generation}:{}",
                shared.clock.now_ms()
            );
            let recovery = dispatcher_store(&shared, move |store, now| {
                store.recover_orphaned_wakes(&recovery_token, now)
            })
            .await?;
            for action in recovery.actions {
                let worker_shared = Arc::clone(&shared);
                workers.spawn(async move { run_recovered_wake(worker_shared, action).await });
            }
        }

        let mut durable_queue_empty = false;
        while !shared.shutdown_requested.load(Ordering::Acquire) {
            // Physical preparation capacity is reserved before acquiring any durable claim.
            // Exhaustion leaves the Wake queued in SQLite instead of creating another queue.
            let preparation = match shared
                .blocking_work
                .preparation
                .try_reserve(DISPATCH_PREPARATION_ESTIMATED_BYTES)
            {
                Ok(reservation) => reservation,
                Err(BlockingError::Overloaded) => break,
                Err(error) => return Err(AgentDispatcherError::Manager(error.to_string())),
            };
            let permit = match shared.turn_concurrency.try_reserve_for_dispatch() {
                Ok(permit) => permit,
                Err(_) => break,
            };
            let claim_token = format!("agent-wake-claim:{}", uuid::Uuid::new_v4());
            let claim_store = Arc::clone(&shared.store);
            let clock = Arc::clone(&shared.clock);
            let worker_claim = claim_token.clone();
            let claim = shared
                .blocking_work
                .maintenance
                .reserve(DISPATCH_MAINTENANCE_ESTIMATED_BYTES)
                .await
                .map_err(|error| AgentDispatcherError::Manager(error.to_string()))?
                .submit(move || claim_store.claim_next_dispatchable(&worker_claim, clock.now_ms()))
                .wait()
                .await
                .map_err(|error| AgentDispatcherError::Manager(error.to_string()))?;
            let wake = match claim {
                Ok(wake) => wake,
                Err(AgentDispatchClaimError::NotReady(error)) => {
                    // Projection contention is a per-candidate condition, not a manager-fatal
                    // event. The durable queue is re-read after the bounded fallback tick; an
                    // unrelated Agent may then win without losing this Wake.
                    eprintln!("Agent dispatcher skipped a temporarily unavailable Wake: {error}");
                    drop(permit);
                    break;
                }
                Err(AgentDispatchClaimError::Fatal(error)) => {
                    drop(permit);
                    return Err(AgentDispatcherError::Storage(error));
                }
            };
            let Some(wake) = wake else {
                drop(permit);
                durable_queue_empty = true;
                break;
            };
            permit.activate_dispatch_reservation();
            shared
                .preparing
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .insert(claim_token.clone());
            let worker_shared = Arc::clone(&shared);
            workers.spawn(async move {
                run_claimed_wake(worker_shared, wake, claim_token, permit, preparation).await
            });
        }

        let no_running_dispatch = shared
            .running
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .is_empty();
        finish_dispatcher_scan(
            &shared,
            scan_generation,
            durable_queue_empty && workers.is_empty() && no_running_dispatch,
        );

        if shared.shutdown_requested.load(Ordering::Acquire) && workers.is_empty() {
            return Ok(settled);
        }

        tokio::select! {
            biased;
            completed = workers.join_next(), if !workers.is_empty() => {
                match completed {
                    Some(Ok(Ok(()))) => settled = settled.saturating_add(1),
                    Some(Ok(Err(error))) => {
                        // The durable Wake remains recoverable. A single failed item must not stop
                        // unrelated Agents from making progress.
                        eprintln!("{error}");
                    }
                    Some(Err(error)) => {
                        eprintln!("Agent dispatcher worker stopped unexpectedly: {error}");
                    }
                    None => {}
                }
            }
            _ = shared.work_available.notified(), if !shared.shutdown_requested.load(Ordering::Acquire) => {}
            _ = shared.turn_concurrency.changed.notified(), if !shared.shutdown_requested.load(Ordering::Acquire) => {}
            _ = tokio::time::sleep(shared.config.idle_poll_interval), if !shared.shutdown_requested.load(Ordering::Acquire) => {}
        }
    }
}

async fn recover_undispatched_interrupts(
    shared: &Arc<AgentDispatcherShared>,
) -> Result<(), AgentDispatcherError> {
    let pending = dispatcher_store(shared, |store, _| store.list_undispatched_interrupts()).await?;
    for interrupt in pending {
        let run_id = interrupt.run_id.clone();
        match dispatcher_control(shared, move |executor| executor.interrupt(&run_id)).await {
            Ok(outcome) if outcome.turn_termination_confirmed() => {
                dispatcher_store(shared, move |store, now| {
                    store.mark_interrupt_dispatched(
                        &interrupt.caller_agent_id,
                        &interrupt.request_id,
                        now,
                    )
                })
                .await?;
            }
            Ok(_) => {
                let wake_id = interrupt.wake_id.clone();
                let wake = dispatcher_store(shared, move |store, _| store.get_wake(&wake_id))
                    .await?
                    .ok_or_else(|| {
                        AgentDispatcherError::Storage(format!(
                            "undispatched interrupt Wake {} disappeared",
                            interrupt.wake_id
                        ))
                    })?;
                if !wake.status.is_terminal()
                    && (wake.agent_id != interrupt.target_agent_id
                        || wake.run_id.as_deref() != Some(interrupt.run_id.as_str()))
                {
                    return Err(AgentDispatcherError::Storage(format!(
                        "undispatched interrupt {} no longer owns Wake {} Run {}",
                        interrupt.request_id, interrupt.wake_id, interrupt.run_id
                    )));
                }
            }
            Err(error) => eprintln!(
                "failed to redeliver durable Agent interrupt {} for Run {}: {error}",
                interrupt.request_id, interrupt.run_id
            ),
        }
    }
    Ok(())
}

fn begin_dispatcher_scan(shared: &AgentDispatcherShared) -> u64 {
    let mut state = shared
        .quiescence
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    state.scan_in_progress = true;
    state.durable_queue_idle = false;
    let generation = state.work_generation;
    drop(state);
    shared.quiescence_changed.notify_waiters();
    generation
}

fn finish_dispatcher_scan(
    shared: &AgentDispatcherShared,
    scan_generation: u64,
    durable_queue_idle: bool,
) {
    let mut state = shared
        .quiescence
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    state.scan_in_progress = false;
    state.durable_queue_idle = durable_queue_idle && state.work_generation == scan_generation;
    drop(state);
    shared.quiescence_changed.notify_waiters();
}

async fn run_recovered_wake(
    shared: Arc<AgentDispatcherShared>,
    recovery: AgentWakeRecoveryAction,
) -> Result<(), AgentDispatcherError> {
    match recovery {
        AgentWakeRecoveryAction::Observe(wake) => {
            let recovered_wake = wake.clone();
            let handle = dispatcher_control(&shared, move |executor| {
                executor.recovered_handle(&recovered_wake)
            })
            .await?;
            // `AgentService` reconstructed this counted lease from the durable in-progress trace.
            // Terminal traces need no slot; approval/in-progress traces must retain one through
            // result settlement.
            let retained_permit = shared.executor.retain_recovered_permit(&handle.run_id);
            shared
                .running
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .insert(
                    wake.agent_id.clone(),
                    RunningDispatch {
                        claim_token: wake.claim_token.clone(),
                        wake_id: wake.wake_id.clone(),
                        run_id: Some(handle.run_id.clone()),
                        waiting_for_approval: wake.status == AgentWakeStatus::WaitingForApproval,
                    },
                );
            let _guard = RunningDispatchGuard {
                claim_token: wake.claim_token.clone(),
                agent_id: wake.agent_id.clone(),
                shared: Arc::clone(&shared),
            };
            observe_and_settle(
                shared,
                wake.clone(),
                handle,
                wake.status == AgentWakeStatus::WaitingForApproval,
                retained_permit,
                None,
            )
            .await
        }
        AgentWakeRecoveryAction::FailBeforeRuntime(wake) => {
            settle_recovery_without_replay(
                &shared,
                wake,
                AgentWakeStatus::Failed,
                "Host 在原子 Turn admission 后、Runtime 采样前退出。",
            )
            .await
        }
        AgentWakeRecoveryAction::OutcomeUnknown(wake) => {
            let recovered_wake = wake.clone();
            let handle = dispatcher_control(&shared, move |executor| {
                executor.recovered_handle(&recovered_wake)
            })
            .await?;
            let retained_permit = shared.executor.retain_recovered_permit(&handle.run_id);
            let settled = settle_recovery_without_replay(
                &shared,
                wake,
                AgentWakeStatus::OutcomeUnknown,
                "Host 在可能产生外部副作用后退出，系统拒绝盲目重放。",
            )
            .await;
            if settled.is_ok() {
                let retired_handle = handle.clone();
                dispatcher_control(&shared, move |executor| {
                    executor.retire_recovered_turn_after_settlement(&retired_handle);
                    Ok(())
                })
                .await?;
            }
            drop(retained_permit);
            settled
        }
        AgentWakeRecoveryAction::TreeStopped(wake) => {
            let recovered_wake = wake.clone();
            let handle = dispatcher_control(&shared, move |executor| {
                executor.recovered_handle(&recovered_wake)
            })
            .await?;
            let retained_permit = shared.executor.retain_recovered_permit(&handle.run_id);
            // On a restarted Host there is normally no live token, but this also retires a
            // durable pending approval if one was restored before Wake recovery.
            let run_id = handle.run_id.clone();
            let interrupt_error =
                dispatcher_control(&shared, move |executor| executor.interrupt(&run_id))
                    .await
                    .err();
            let frozen = mycopilot_core::ActiveAgentTreeWake {
                agent_id: wake.agent_id.clone(),
                conversation_id: handle.conversation_id.clone(),
                wake_id: wake.wake_id.clone(),
                run_id: handle.run_id.clone(),
                assistant_message_id: handle.assistant_message_id.clone(),
                claim_token: wake.claim_token.clone().ok_or_else(|| {
                    AgentDispatcherError::Storage(
                        "recovered stopped Wake has no claim token".to_string(),
                    )
                })?,
                status: wake.status,
            };
            let stopped = dispatcher_store(&shared, move |store, now| {
                store.settle_tree_stopped_wake(&frozen, now)
            })
            .await
            .map_err(|error| {
                AgentDispatcherError::Storage(match interrupt_error.as_ref() {
                    Some(interrupt_error) => {
                        format!("{error}; runtime cancellation also failed: {interrupt_error}")
                    }
                    None => error.to_string(),
                })
            })?;
            match stopped {
                TreeStoppedWakeSettlement::Interrupted => {
                    let retired_handle = handle.clone();
                    dispatcher_control(&shared, move |executor| {
                        executor.retire_recovered_turn_after_settlement(&retired_handle);
                        Ok(())
                    })
                    .await?;
                    drop(retained_permit);
                    Ok(())
                }
                TreeStoppedWakeSettlement::AlreadyTerminal => {
                    shared
                        .running
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .insert(
                            wake.agent_id.clone(),
                            RunningDispatch {
                                claim_token: wake.claim_token.clone(),
                                wake_id: wake.wake_id.clone(),
                                run_id: Some(handle.run_id.clone()),
                                waiting_for_approval: wake.status
                                    == AgentWakeStatus::WaitingForApproval,
                            },
                        );
                    let _guard = RunningDispatchGuard {
                        claim_token: wake.claim_token.clone(),
                        agent_id: wake.agent_id.clone(),
                        shared: Arc::clone(&shared),
                    };
                    observe_and_settle(
                        shared,
                        wake.clone(),
                        handle,
                        wake.status == AgentWakeStatus::WaitingForApproval,
                        retained_permit,
                        None,
                    )
                    .await
                }
                TreeStoppedWakeSettlement::UnsafeActiveAction { count } => {
                    let reason = format!(
                        "Host 重启后发现 {count} 条可能已经产生外部副作用的未确认执行；系统拒绝伪装为安全中断。"
                    );
                    let settled = settle_recovery_without_replay(
                        &shared,
                        wake,
                        AgentWakeStatus::OutcomeUnknown,
                        &reason,
                    )
                    .await;
                    if settled.is_ok() {
                        let retired_handle = handle.clone();
                        dispatcher_control(&shared, move |executor| {
                            executor.retire_recovered_turn_after_settlement(&retired_handle);
                            Ok(())
                        })
                        .await?;
                    }
                    drop(retained_permit);
                    settled
                }
                TreeStoppedWakeSettlement::LostOwnership => {
                    drop(retained_permit);
                    Err(AgentDispatcherError::Storage(
                        "recovered stopped Wake changed ownership before settlement".to_string(),
                    ))
                }
            }
        }
    }
}

async fn settle_recovery_without_replay(
    shared: &Arc<AgentDispatcherShared>,
    wake: AgentWakeRequestRecord,
    terminal_status: AgentWakeStatus,
    terminal_error: &str,
) -> Result<(), AgentDispatcherError> {
    let claim_token = wake
        .claim_token
        .clone()
        .ok_or_else(|| AgentDispatcherError::Storage("recovered Wake has no claim".to_string()))?;
    let settlement = AgentWakeSettlement {
        wake_id: wake.wake_id,
        expected_status: wake.status,
        claim_token,
        terminal_status,
        run_id: wake.run_id,
        assistant_message_id: wake.assistant_message_id,
        artifact_refs: Vec::new(),
        terminal_error: Some(terminal_error.to_string()),
    };
    dispatcher_store(shared, move |store, now| store.settle(&settlement, now)).await
}

async fn run_claimed_wake(
    shared: Arc<AgentDispatcherShared>,
    wake: AgentWakeRequestRecord,
    claim_token: String,
    global_permit: AgentTurnConcurrencyPermit,
    preparation: BlockingReservation,
) -> Result<(), AgentDispatcherError> {
    let preparing = PreparingDispatchGuard {
        claim_token: claim_token.clone(),
        shared: Arc::clone(&shared),
    };
    let lease = DispatchWakeLease::start(
        Arc::clone(&shared),
        wake.wake_id.clone(),
        claim_token.clone(),
    );
    shared
        .running
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .insert(
            wake.agent_id.clone(),
            RunningDispatch {
                claim_token: wake.claim_token.clone(),
                wake_id: wake.wake_id.clone(),
                run_id: None,
                waiting_for_approval: false,
            },
        );
    let _running_guard = RunningDispatchGuard {
        claim_token: wake.claim_token.clone(),
        agent_id: wake.agent_id.clone(),
        shared: Arc::clone(&shared),
    };
    let worker_shared = Arc::clone(&shared);
    let worker_wake = wake.clone();
    let worker_permit = global_permit.clone();
    let lease_failure = Arc::clone(&lease.failure);
    let started = preparation
        .submit(move || {
            if worker_shared.shutdown_requested.load(Ordering::Acquire) {
                // No Turn was accepted. Preserve the claim for authoritative expiry/recovery.
                return Ok(None);
            }
            if let Some(error) = lease_failure
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .clone()
            {
                return Err(error);
            }
            let _preparation_span =
                mycopilot_core::performance::Span::new("turn.child_preparation", "start");
            worker_shared
                .executor
                .start(&worker_wake, worker_permit)
                .map(Some)
        })
        .wait()
        .await
        .map_err(|error| AgentDispatcherError::Manager(error.to_string()))?;

    // A synchronous start cannot be force-aborted. Keep its lease/permit and preparing guard
    // until the actual result is known; shutdown waits for this exact handoff before observers.
    if let Some(error) = lease.failure() {
        if let Ok(Some(handle)) = &started {
            let handle = handle.clone();
            dispatcher_control(&shared, move |executor| executor.interrupt_owned(&handle)).await?;
        }
        return Err(AgentDispatcherError::Storage(format!(
            "Wake {} lost its durable lease during preparation: {error}",
            wake.wake_id
        )));
    }
    let handle = match started {
        Ok(Some(handle)) => handle,
        Ok(None) => {
            return Err(AgentDispatcherError::Manager(format!(
                "Wake {} remains durably claimed because shutdown stopped preparation",
                wake.wake_id
            )))
        }
        Err(error) => {
            let wake_id = wake.wake_id.clone();
            let persisted = dispatcher_store(&shared, move |store, _| store.get_wake(&wake_id))
                .await?
                .ok_or_else(|| {
                    AgentDispatcherError::Storage(format!(
                        "Wake {} disappeared after Turn start failed",
                        wake.wake_id
                    ))
                })?;
            if !matches!(
                persisted.status,
                AgentWakeStatus::Claimed | AgentWakeStatus::Running
            ) || persisted.claim_token.as_deref() != Some(claim_token.as_str())
            {
                return Err(AgentDispatcherError::Storage(format!(
                    "Wake {} changed ownership after Turn start failed",
                    wake.wake_id
                )));
            }
            let settlement = AgentWakeSettlement {
                wake_id: wake.wake_id.clone(),
                expected_status: persisted.status,
                claim_token,
                terminal_status: AgentWakeStatus::Failed,
                run_id: persisted.run_id,
                assistant_message_id: persisted.assistant_message_id,
                artifact_refs: Vec::new(),
                terminal_error: Some(error),
            };
            dispatcher_store(&shared, move |store, now| store.settle(&settlement, now)).await?;
            notify_dispatcher_work_available(&shared);
            return Ok(());
        }
    };
    if let Some(running) = shared
        .running
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get_mut(&wake.agent_id)
        .filter(|current| current.claim_token == wake.claim_token)
    {
        running.run_id = Some(handle.run_id.clone());
    }
    if shared.shutdown_requested.load(Ordering::Acquire) {
        let run_id = handle.run_id.clone();
        dispatcher_control(&shared, move |executor| executor.interrupt(&run_id)).await?;
    }
    drop(preparing);
    observe_and_settle(
        shared,
        wake,
        handle,
        false,
        Some(global_permit),
        Some(lease),
    )
    .await
}

async fn observe_and_settle(
    shared: Arc<AgentDispatcherShared>,
    wake: AgentWakeRequestRecord,
    handle: AgentWakeExecutionHandle,
    mut waiting_already_reported: bool,
    _settlement_permit: Option<AgentTurnConcurrencyPermit>,
    lease: Option<DispatchWakeLease>,
) -> Result<(), AgentDispatcherError> {
    let claim_token = wake
        .claim_token
        .clone()
        .ok_or_else(|| AgentDispatcherError::Storage("active Wake has no claim token".into()))?;
    let lease = lease.unwrap_or_else(|| {
        DispatchWakeLease::start(
            Arc::clone(&shared),
            wake.wake_id.clone(),
            claim_token.clone(),
        )
    });
    loop {
        let observation = shared.executor.observe(&handle, waiting_already_reported);
        tokio::pin!(observation);
        let observed = tokio::select! {
            observation = &mut observation => observation.map_err(AgentDispatcherError::Manager)?,
            error = lease.wait_failure() => {
                let expired_handle = handle.clone();
                let cancellation = dispatcher_control(&shared, move |executor| executor.interrupt_owned(&expired_handle)).await;
                return Err(AgentDispatcherError::Storage(format!(
                    "Wake {} lost its durable lease while Turn {} was active: {error}; cancellation: {cancellation:?}",
                    wake.wake_id, handle.run_id,
                )));
            }
        };
        match observed {
            AgentWakeExecutionObservation::WaitingForApproval => {
                if !waiting_already_reported {
                    let waiting_handle = handle.clone();
                    let waiting_claim = claim_token.clone();
                    let outcome = dispatcher_store(&shared, move |store, now| {
                        store.mark_waiting_for_approval(
                            &waiting_handle.wake_id,
                            &waiting_handle.run_id,
                            &waiting_handle.conversation_id,
                            &waiting_handle.assistant_message_id,
                            &waiting_claim,
                            now,
                        )
                    })
                    .await?;
                    if let AgentWakeApprovalWaitOutcome::TreeStopped(stopped) = outcome {
                        let run_id = handle.run_id.clone();
                        let interrupt_error = dispatcher_control(&shared, move |executor| {
                            executor.interrupt(&run_id)
                        })
                        .await
                        .err();
                        let frozen = mycopilot_core::ActiveAgentTreeWake {
                            agent_id: stopped.agent_id,
                            conversation_id: handle.conversation_id.clone(),
                            wake_id: stopped.wake_id,
                            run_id: handle.run_id.clone(),
                            assistant_message_id: handle.assistant_message_id.clone(),
                            claim_token: claim_token.clone(),
                            status: stopped.status,
                        };
                        let settlement = dispatcher_store(&shared, move |store, now| {
                            store.settle_tree_stopped_wake(&frozen, now)
                        })
                        .await
                        .map_err(|error| {
                            AgentDispatcherError::Storage(match interrupt_error.as_ref() {
                                Some(interrupt_error) => format!(
                                    "{error}; runtime cancellation also failed: {interrupt_error}"
                                ),
                                None => error.to_string(),
                            })
                        })?;
                        match settlement {
                            TreeStoppedWakeSettlement::Interrupted => {}
                            TreeStoppedWakeSettlement::AlreadyTerminal => continue,
                            TreeStoppedWakeSettlement::LostOwnership => {
                                return Err(AgentDispatcherError::Storage(
                                    "stopped approval Wake changed ownership before settlement"
                                        .to_string(),
                                ));
                            }
                            TreeStoppedWakeSettlement::UnsafeActiveAction { count } => {
                                return Err(AgentDispatcherError::Storage(format!(
                                    "stopped approval Turn still has {count} unconfirmed external executions"
                                )));
                            }
                        }
                        notify_dispatcher_work_available(&shared);
                        return Ok(());
                    }
                    waiting_already_reported =
                        matches!(outcome, AgentWakeApprovalWaitOutcome::Waiting(_));
                    if let Some(running) = shared
                        .running
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .get_mut(&wake.agent_id)
                        .filter(|current| current.run_id.as_deref() == Some(handle.run_id.as_str()))
                    {
                        running.waiting_for_approval = waiting_already_reported;
                    }
                }
            }
            AgentWakeExecutionObservation::RunningAfterApproval => {
                if waiting_already_reported {
                    waiting_already_reported = false;
                    if let Some(running) = shared
                        .running
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .get_mut(&wake.agent_id)
                        .filter(|current| current.run_id.as_deref() == Some(handle.run_id.as_str()))
                    {
                        running.waiting_for_approval = false;
                    }
                }
            }
            AgentWakeExecutionObservation::Terminal {
                status,
                artifact_refs,
                terminal_error,
            } => {
                let wake_id = wake.wake_id.clone();
                let persisted = dispatcher_store(&shared, move |store, _| store.get_wake(&wake_id))
                    .await?
                    .ok_or_else(|| {
                        AgentDispatcherError::Storage("active Wake disappeared".into())
                    })?;
                let settlement = AgentWakeSettlement {
                    wake_id: wake.wake_id.clone(),
                    expected_status: persisted.status,
                    claim_token,
                    terminal_status: status,
                    run_id: Some(handle.run_id.clone()),
                    assistant_message_id: Some(handle.assistant_message_id.clone()),
                    artifact_refs,
                    terminal_error,
                };
                dispatcher_store(&shared, move |store, now| store.settle(&settlement, now)).await?;
                notify_dispatcher_work_available(&shared);
                return Ok(());
            }
        }
    }
}

struct RunningDispatchGuard {
    agent_id: String,
    claim_token: Option<String>,
    shared: Arc<AgentDispatcherShared>,
}

impl Drop for RunningDispatchGuard {
    fn drop(&mut self) {
        let mut running = self
            .shared
            .running
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if running
            .get(&self.agent_id)
            .is_some_and(|current| current.claim_token == self.claim_token)
        {
            running.remove(&self.agent_id);
        }
        drop(running);
        notify_dispatcher_work_available(&self.shared);
    }
}

fn notify_dispatcher_work_available(shared: &AgentDispatcherShared) {
    let mut state = shared
        .quiescence
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    state.work_generation = state.work_generation.wrapping_add(1);
    state.durable_queue_idle = false;
    drop(state);
    shared.quiescence_changed.notify_waiters();
    shared.work_available.notify_one();
}

#[cfg(test)]
mod tests;
