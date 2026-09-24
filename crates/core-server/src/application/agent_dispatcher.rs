//! Persistent child-Agent Wake scheduling.
//!
//! SQLite owns Wake ordering, leases, execution identity, and terminal settlement. This module
//! contributes only a process-local concurrency permit and low-latency notifications. It never
//! interprets a Graph workflow and never invokes a second Agent loop: the production execution
//! port below is backed by the shared [`AgentService`] Turn executor.

use crate::application::agent::AgentRunCancellationOutcome;
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
/// `claim_next_dispatchable` must select globally by Wake sequence and atomically skip Agents
/// which already own a claimed/running/waiting Wake or an in-progress Conversation trace. Thus
/// acquiring the process permit before this method establishes the fixed order
/// `global permit -> durable per-Agent execution right`.
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
        now_ms: i64,
    ) -> Result<Option<AgentWakeRequestRecord>, AgentDispatchClaimError> {
        let wake = self
            .storage
            .claim_next_dispatchable_agent_wake_at(claim_token, now_ms)
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
        now_ms: i64,
    ) -> Result<AgentWakeApprovalWaitOutcome, String> {
        let outcome = self
            .storage
            .mark_agent_wake_waiting_for_pending_approval_at(
                wake_id,
                run_id,
                conversation_id,
                assistant_message_id,
                claim_token,
                now_ms,
            )?;
        if let AgentWakeApprovalWaitOutcome::Waiting(wake) = &outcome {
            self.wait_notifications
                .notify_caller(&wake.requester_agent_id);
        }
        Ok(outcome)
    }

    fn renew_lease(&self, wake_id: &str, claim_token: &str, now_ms: i64) -> Result<(), String> {
        self.storage
            .renew_agent_wake_lease_at(wake_id, claim_token, now_ms)
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    fn settle(&self, settlement: &AgentWakeSettlement, now_ms: i64) -> Result<(), String> {
        let result = self
            .storage
            .finish_agent_turn_with_result_at(
                &FinishAgentTurnResultInput {
                    wake_id: settlement.wake_id.clone(),
                    expected_status: settlement.expected_status,
                    claim_token: settlement.claim_token.clone(),
                    terminal_status: settlement.terminal_status,
                    run_id: settlement.run_id.clone(),
                    assistant_message_id: settlement.assistant_message_id.clone(),
                    terminal_error: settlement.terminal_error.clone(),
                },
                now_ms,
            )
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

    fn inspect_durable(
        &self,
        handle: &AgentWakeExecutionHandle,
        waiting_already_reported: bool,
    ) -> Result<Option<AgentWakeExecutionObservation>, String> {
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
                if let Some(observation) = self.inspect_durable(handle, waiting_already_reported)? {
                    return Ok(observation);
                }
                let notification = self
                    .service
                    .durable_turn_notification(&handle.assistant_message_id);
                let notified = notification.notified();
                tokio::pin!(notified);
                // The second check closes the check/subscribe race. A bounded tick also makes a
                // fresh process observe commits published by a previous Host.
                if let Some(observation) = self.inspect_durable(handle, waiting_already_reported)? {
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
        let agent = self
            .storage
            .get_agent_node(&wake.agent_id)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("recovered Wake {} Agent is missing", wake.wake_id))?;
        Ok(AgentWakeExecutionHandle {
            wake_id: wake.wake_id.clone(),
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
    clock: Arc<dyn AgentDispatcherClock>,
    turn_concurrency: AgentTurnConcurrencyGate,
    work_available: Notify,
    shutdown_requested: AtomicBool,
    running: Mutex<HashMap<String, RunningDispatch>>,
    quiescence: Mutex<AgentDispatcherQuiescence>,
    quiescence_changed: Notify,
    config: AgentDispatcherConfig,
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
        let shared = Arc::new(AgentDispatcherShared {
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
                if !dispatch.waiting_for_approval
                    && !interrupted_runs.contains(run_id)
                    && self
                        .shared
                        .executor
                        .interrupt(run_id)
                        .map(AgentRunCancellationOutcome::any_effect)
                        .unwrap_or(false)
                {
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
            if now >= cancellation_deadline {
                break;
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
    let mut settled = 0usize;
    let recovery_generation = uuid::Uuid::new_v4().to_string();
    loop {
        let scan_generation = begin_dispatcher_scan(&shared);
        // This scan is intentionally repeated. A replacement Host may start before the old
        // process lease expires; the bounded fallback tick later reaches the deadline and
        // recovers it without requiring another restart or an in-memory notification.
        if !shared.shutdown_requested.load(Ordering::Acquire) {
            recover_undispatched_interrupts(&shared)?;
            let recovery_token = format!(
                "agent-recovery:{recovery_generation}:{}",
                shared.clock.now_ms()
            );
            let recovery = shared
                .store
                .recover_orphaned_wakes(&recovery_token, shared.clock.now_ms())
                .map_err(AgentDispatcherError::Storage)?;
            for action in recovery.actions {
                let worker_shared = Arc::clone(&shared);
                workers.spawn(async move { run_recovered_wake(worker_shared, action).await });
            }
        }

        let mut durable_queue_empty = false;
        while !shared.shutdown_requested.load(Ordering::Acquire) {
            let permit = match shared.turn_concurrency.try_reserve_for_dispatch() {
                Ok(permit) => permit,
                Err(_) => break,
            };
            let claim_token = format!("agent-wake-claim:{}", uuid::Uuid::new_v4());
            let wake = match shared
                .store
                .claim_next_dispatchable(&claim_token, shared.clock.now_ms())
            {
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
            let worker_shared = Arc::clone(&shared);
            workers.spawn(async move {
                run_claimed_wake(worker_shared, wake, claim_token, permit).await
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

fn recover_undispatched_interrupts(
    shared: &AgentDispatcherShared,
) -> Result<(), AgentDispatcherError> {
    let pending = shared
        .store
        .list_undispatched_interrupts()
        .map_err(AgentDispatcherError::Storage)?;
    for interrupt in pending {
        match shared.executor.interrupt(&interrupt.run_id) {
            Ok(outcome) if outcome.turn_termination_confirmed() => shared
                .store
                .mark_interrupt_dispatched(
                    &interrupt.caller_agent_id,
                    &interrupt.request_id,
                    shared.clock.now_ms(),
                )
                .map_err(AgentDispatcherError::Storage)?,
            Ok(_) => {
                let wake = shared
                    .store
                    .get_wake(&interrupt.wake_id)
                    .map_err(AgentDispatcherError::Storage)?
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
                // No process-local owner exists yet. Keep the receipt undispatched; pending
                // approval restoration or lease recovery may make the next bounded scan
                // actionable, while a terminal Wake needs no fabricated acknowledgement.
            }
            Err(error) => {
                eprintln!(
                    "failed to redeliver durable Agent interrupt {} for Run {}: {error}",
                    interrupt.request_id, interrupt.run_id
                );
            }
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
            let handle = shared
                .executor
                .recovered_handle(&wake)
                .map_err(AgentDispatcherError::Manager)?;
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
                        wake_id: wake.wake_id.clone(),
                        run_id: Some(handle.run_id.clone()),
                        waiting_for_approval: wake.status == AgentWakeStatus::WaitingForApproval,
                    },
                );
            let _guard = RunningDispatchGuard {
                agent_id: wake.agent_id.clone(),
                shared: Arc::clone(&shared),
            };
            observe_and_settle(
                shared,
                wake.clone(),
                handle,
                wake.status == AgentWakeStatus::WaitingForApproval,
                retained_permit,
            )
            .await
        }
        AgentWakeRecoveryAction::FailBeforeRuntime(wake) => settle_recovery_without_replay(
            &shared,
            wake,
            AgentWakeStatus::Failed,
            "Host 在原子 Turn admission 后、Runtime 采样前退出。",
        ),
        AgentWakeRecoveryAction::OutcomeUnknown(wake) => {
            let handle = shared
                .executor
                .recovered_handle(&wake)
                .map_err(AgentDispatcherError::Manager)?;
            let retained_permit = shared.executor.retain_recovered_permit(&handle.run_id);
            let settled = settle_recovery_without_replay(
                &shared,
                wake,
                AgentWakeStatus::OutcomeUnknown,
                "Host 在可能产生外部副作用后退出，系统拒绝盲目重放。",
            );
            if settled.is_ok() {
                shared
                    .executor
                    .retire_recovered_turn_after_settlement(&handle);
            }
            drop(retained_permit);
            settled
        }
        AgentWakeRecoveryAction::TreeStopped(wake) => {
            let handle = shared
                .executor
                .recovered_handle(&wake)
                .map_err(AgentDispatcherError::Manager)?;
            let retained_permit = shared.executor.retain_recovered_permit(&handle.run_id);
            // On a restarted Host there is normally no live token, but this also retires a
            // durable pending approval if one was restored before Wake recovery.
            let interrupt_error = shared.executor.interrupt(&handle.run_id).err();
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
            let stopped = shared
                .store
                .settle_tree_stopped_wake(&frozen, shared.clock.now_ms())
                .map_err(|error| {
                    AgentDispatcherError::Storage(match interrupt_error.as_deref() {
                        Some(interrupt_error) => {
                            format!("{error}; runtime cancellation also failed: {interrupt_error}")
                        }
                        None => error,
                    })
                })?;
            match stopped {
                TreeStoppedWakeSettlement::Interrupted => {
                    shared
                        .executor
                        .retire_recovered_turn_after_settlement(&handle);
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
                                wake_id: wake.wake_id.clone(),
                                run_id: Some(handle.run_id.clone()),
                                waiting_for_approval: wake.status
                                    == AgentWakeStatus::WaitingForApproval,
                            },
                        );
                    let _guard = RunningDispatchGuard {
                        agent_id: wake.agent_id.clone(),
                        shared: Arc::clone(&shared),
                    };
                    observe_and_settle(
                        shared,
                        wake.clone(),
                        handle,
                        wake.status == AgentWakeStatus::WaitingForApproval,
                        retained_permit,
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
                    );
                    if settled.is_ok() {
                        shared
                            .executor
                            .retire_recovered_turn_after_settlement(&handle);
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

fn settle_recovery_without_replay(
    shared: &AgentDispatcherShared,
    wake: AgentWakeRequestRecord,
    terminal_status: AgentWakeStatus,
    terminal_error: &str,
) -> Result<(), AgentDispatcherError> {
    let claim_token = wake
        .claim_token
        .clone()
        .ok_or_else(|| AgentDispatcherError::Storage("recovered Wake has no claim".to_string()))?;
    shared
        .store
        .settle(
            &AgentWakeSettlement {
                wake_id: wake.wake_id,
                expected_status: wake.status,
                claim_token,
                terminal_status,
                run_id: wake.run_id,
                assistant_message_id: wake.assistant_message_id,
                artifact_refs: Vec::new(),
                terminal_error: Some(terminal_error.to_string()),
            },
            shared.clock.now_ms(),
        )
        .map_err(AgentDispatcherError::Storage)
}

async fn run_claimed_wake(
    shared: Arc<AgentDispatcherShared>,
    wake: AgentWakeRequestRecord,
    claim_token: String,
    global_permit: AgentTurnConcurrencyPermit,
) -> Result<(), AgentDispatcherError> {
    shared
        .running
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .insert(
            wake.agent_id.clone(),
            RunningDispatch {
                wake_id: wake.wake_id.clone(),
                run_id: None,
                waiting_for_approval: false,
            },
        );
    let _running_guard = RunningDispatchGuard {
        agent_id: wake.agent_id.clone(),
        shared: Arc::clone(&shared),
    };

    let handle = match shared.executor.start(&wake, global_permit.clone()) {
        Ok(handle) => handle,
        Err(error) => {
            let persisted = shared
                .store
                .get_wake(&wake.wake_id)
                .map_err(AgentDispatcherError::Storage)?
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
            shared
                .store
                .settle(
                    &AgentWakeSettlement {
                        wake_id: wake.wake_id.clone(),
                        expected_status: persisted.status,
                        claim_token: claim_token.clone(),
                        terminal_status: AgentWakeStatus::Failed,
                        run_id: persisted.run_id,
                        assistant_message_id: persisted.assistant_message_id,
                        artifact_refs: Vec::new(),
                        terminal_error: Some(error),
                    },
                    shared.clock.now_ms(),
                )
                .map_err(AgentDispatcherError::Storage)?;
            notify_dispatcher_work_available(&shared);
            return Ok(());
        }
    };
    if let Some(running) = shared
        .running
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get_mut(&wake.agent_id)
    {
        running.run_id = Some(handle.run_id.clone());
    }

    observe_and_settle(shared, wake, handle, false, Some(global_permit)).await
}

async fn observe_and_settle(
    shared: Arc<AgentDispatcherShared>,
    wake: AgentWakeRequestRecord,
    handle: AgentWakeExecutionHandle,
    mut waiting_already_reported: bool,
    _settlement_permit: Option<AgentTurnConcurrencyPermit>,
) -> Result<(), AgentDispatcherError> {
    let claim_token = wake
        .claim_token
        .clone()
        .ok_or_else(|| AgentDispatcherError::Storage("active Wake has no claim token".into()))?;
    let mut lease_tick = tokio::time::interval(shared.config.wake_lease_renew_interval);
    lease_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    lease_tick.tick().await;
    loop {
        let observation = shared.executor.observe(&handle, waiting_already_reported);
        tokio::pin!(observation);
        let observed = loop {
            tokio::select! {
                observation = &mut observation => break observation,
                _ = lease_tick.tick() => {
                    if let Err(error) = shared.store.renew_lease(
                        &wake.wake_id,
                        &claim_token,
                        shared.clock.now_ms(),
                    ) {
                        let _ = shared.executor.interrupt(&handle.run_id);
                        return Err(AgentDispatcherError::Storage(format!(
                            "Wake {} lost its durable lease while Turn {} was active: {error}",
                            wake.wake_id, handle.run_id
                        )));
                    }
                }
            }
        }
        .map_err(AgentDispatcherError::Manager)?;
        match observed {
            AgentWakeExecutionObservation::WaitingForApproval => {
                if !waiting_already_reported {
                    let outcome = shared
                        .store
                        .mark_waiting_for_approval(
                            &wake.wake_id,
                            &handle.run_id,
                            &handle.conversation_id,
                            &handle.assistant_message_id,
                            &claim_token,
                            shared.clock.now_ms(),
                        )
                        .map_err(AgentDispatcherError::Storage)?;
                    if let AgentWakeApprovalWaitOutcome::TreeStopped(stopped) = outcome {
                        let interrupt_error = shared.executor.interrupt(&handle.run_id).err();
                        let frozen = mycopilot_core::ActiveAgentTreeWake {
                            agent_id: stopped.agent_id,
                            conversation_id: handle.conversation_id.clone(),
                            wake_id: stopped.wake_id,
                            run_id: handle.run_id.clone(),
                            assistant_message_id: handle.assistant_message_id.clone(),
                            claim_token: claim_token.clone(),
                            status: stopped.status,
                        };
                        let settlement = shared
                            .store
                            .settle_tree_stopped_wake(&frozen, shared.clock.now_ms())
                            .map_err(|error| {
                                AgentDispatcherError::Storage(match interrupt_error.as_deref() {
                                    Some(interrupt_error) => format!(
                                        "{error}; runtime cancellation also failed: {interrupt_error}"
                                    ),
                                    None => error,
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
                let persisted = shared
                    .store
                    .get_wake(&wake.wake_id)
                    .map_err(AgentDispatcherError::Storage)?
                    .ok_or_else(|| {
                        AgentDispatcherError::Storage("active Wake disappeared".into())
                    })?;
                shared
                    .store
                    .settle(
                        &AgentWakeSettlement {
                            wake_id: wake.wake_id.clone(),
                            expected_status: persisted.status,
                            claim_token,
                            terminal_status: status,
                            run_id: Some(handle.run_id.clone()),
                            assistant_message_id: Some(handle.assistant_message_id.clone()),
                            artifact_refs,
                            terminal_error,
                        },
                        shared.clock.now_ms(),
                    )
                    .map_err(AgentDispatcherError::Storage)?;
                notify_dispatcher_work_available(&shared);
                return Ok(());
            }
        }
    }
}

struct RunningDispatchGuard {
    agent_id: String,
    shared: Arc<AgentDispatcherShared>,
}

impl Drop for RunningDispatchGuard {
    fn drop(&mut self) {
        self.shared
            .running
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&self.agent_id);
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
mod tests {
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

        fn renew_lease(
            &self,
            _wake_id: &str,
            _claim_token: &str,
            _now_ms: i64,
        ) -> Result<(), String> {
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
                    interrupt_result: Mutex::new(
                        AgentRunCancellationOutcome::TurnTerminationConfirmed,
                    ),
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
        let storage = Arc::new(
            mycopilot_core::storage::service::StorageService::open(&database_path).unwrap(),
        );
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
}
