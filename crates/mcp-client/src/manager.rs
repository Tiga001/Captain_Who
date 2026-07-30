//! Connection-manager facade and shared coordination state.
//!
//! Lifecycle, catalog refresh, invocation, registry reconciliation, and shutdown behavior live in
//! focused child modules while this file retains the stable public types and common invariants.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex, Weak};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, watch, Notify};
use tokio::task::{JoinHandle, JoinSet};
use tokio_util::sync::CancellationToken;

use crate::catalog::discover_catalog_with_limits;
use crate::limits::{validate_tool_arguments, validate_tool_result};
use crate::{
    BoxMcpFuture, McpActiveCallId, McpActiveCallProvenance, McpActiveCallSnapshot, McpApprovalMode,
    McpCancellationToken, McpCatalogCompleteness, McpCatalogIssue, McpCatalogPolicy,
    McpCatalogSnapshot, McpCatalogToolCall, McpConfigDigest, McpConfigEpoch, McpConnector,
    McpDispatchCertainty, McpDispatchTracker, McpError, McpErrorKind, McpEvent, McpEventSink,
    McpInvocationId, McpInvocationState, McpModelCallId, McpOutcomeUnknownReason, McpPeer,
    McpPeerNotificationState, McpProtocolSnapshot, McpRegistry, McpRegistryChange,
    McpRegistryChangeKind, McpRegistryEntry, McpRegistrySubscriptionError, McpSafeError,
    McpSecurityLimits, McpServerId, McpServerScope, McpServerState, McpToolCall, McpToolId,
    McpToolResult, McpTrustLevel, NoopMcpEventSink,
};

const FORCE_SHUTDOWN_GRACE_MAX: Duration = Duration::from_millis(250);

mod catalog_refresh;
mod connection_lifecycle;
mod invocation;
mod registry_sync;
mod shutdown;

#[cfg(test)]
mod tests;

use invocation::{
    cancel_active_calls, is_terminal_invocation_state, settle_active_calls,
    wait_for_active_call_removal,
};

#[cfg(test)]
use registry_sync::{reconcile_registry_record, stop_and_forget_removed_change};

#[derive(Clone, Debug)]
pub struct McpManagerPolicy {
    pub catalog: McpCatalogPolicy,
    pub notification_debounce: Duration,
    pub active_call_settle_timeout: Duration,
    pub max_active_calls_per_server: usize,
    pub max_active_calls_total: usize,
    pub security_limits: McpSecurityLimits,
}

impl Default for McpManagerPolicy {
    fn default() -> Self {
        Self {
            catalog: McpCatalogPolicy::default(),
            notification_debounce: Duration::from_millis(100),
            active_call_settle_timeout: Duration::from_millis(250),
            max_active_calls_per_server: 32,
            max_active_calls_total: 256,
            security_limits: McpSecurityLimits::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerStatus {
    pub server_id: McpServerId,
    /// Bounded, control-character-normalized text for display only.
    pub display_name: String,
    pub state: McpServerState,
    pub enabled: bool,
    pub scope: McpServerScope,
    pub trust: McpTrustLevel,
    pub approval_mode: McpApprovalMode,
    pub config_epoch: McpConfigEpoch,
    pub registry_revision: u64,
    pub config_digest: McpConfigDigest,
    pub protocol: Option<McpProtocolSnapshot>,
    pub notification_state: McpPeerNotificationState,
    pub catalog_generation: u64,
    pub catalog_completeness: McpCatalogCompleteness,
    pub tool_count: usize,
    pub active_call_count: usize,
    pub last_error: Option<McpSafeError>,
}

impl McpServerStatus {
    fn from_registry(entry: &McpRegistryEntry) -> Self {
        Self {
            server_id: entry.config.id,
            display_name: safe_display_name(&entry.config.display_name),
            state: McpServerState::Disabled,
            enabled: entry.config.enabled,
            scope: entry.config.scope.clone(),
            trust: entry.config.trust,
            approval_mode: entry.config.approval_mode,
            config_epoch: entry.config_epoch,
            registry_revision: entry.revision,
            config_digest: entry.config_digest.clone(),
            protocol: None,
            notification_state: McpPeerNotificationState::Unknown,
            catalog_generation: 0,
            catalog_completeness: McpCatalogCompleteness::Failed(McpCatalogIssue::RequestFailed),
            tool_count: 0,
            active_call_count: 0,
            last_error: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpBatchOperationResult {
    /// None denotes a manager-wide failure that cannot be attributed to a
    /// registered server. A synthetic/random server identity is never used.
    pub server_id: Option<McpServerId>,
    pub status: Option<McpServerStatus>,
    pub error: Option<McpSafeError>,
}

struct ManagedEntryState {
    epoch: u64,
    event_sequence: u64,
    status: McpServerStatus,
    catalog: McpCatalogSnapshot,
    peer: Option<Arc<dyn McpPeer>>,
    closing_peer: Option<Arc<dyn McpPeer>>,
    connect_inflight: bool,
    refresh_inflight: bool,
    watcher_cancel: Option<CancellationToken>,
    watcher_task: Option<JoinHandle<()>>,
    active_calls: BTreeMap<McpActiveCallId, Arc<ActiveCallControl>>,
    removed: bool,
}

struct ManagedEntry {
    state: StdMutex<ManagedEntryState>,
    status: watch::Sender<McpServerStatus>,
    settled: Notify,
}

struct ActiveCallControl {
    id: McpActiveCallId,
    provenance: McpActiveCallProvenance,
    cancellation: McpCancellationToken,
    dispatch: McpDispatchTracker,
    state: StdMutex<McpInvocationState>,
    cancellation_reason: StdMutex<Option<McpOutcomeUnknownReason>>,
    started_at: tokio::time::Instant,
    deadline: tokio::time::Instant,
    timeout_ms: u64,
    removed: AtomicBool,
    settled: Notify,
}

impl ActiveCallControl {
    fn new(id: McpActiveCallId, provenance: McpActiveCallProvenance, timeout_ms: u64) -> Self {
        let started_at = tokio::time::Instant::now();
        let deadline = started_at + Duration::from_millis(timeout_ms);
        Self {
            id,
            provenance,
            cancellation: McpCancellationToken::new(),
            dispatch: McpDispatchTracker::new(),
            state: StdMutex::new(McpInvocationState::Dispatching),
            cancellation_reason: StdMutex::new(None),
            started_at,
            deadline,
            timeout_ms,
            removed: AtomicBool::new(false),
            settled: Notify::new(),
        }
    }

    fn snapshot(&self) -> McpActiveCallSnapshot {
        McpActiveCallSnapshot {
            id: self.id.clone(),
            provenance: self.provenance.clone(),
            state: self
                .state
                .lock()
                .map(|state| *state)
                .unwrap_or(McpInvocationState::OutcomeUnknown),
            dispatch_phase: self.dispatch.phase(),
            dispatch_certainty: self.dispatch.certainty(),
            timeout_ms: self.timeout_ms,
            elapsed_ms: u64::try_from(self.started_at.elapsed().as_millis()).unwrap_or(u64::MAX),
            deadline_remaining_ms: u64::try_from(
                self.deadline
                    .saturating_duration_since(tokio::time::Instant::now())
                    .as_millis(),
            )
            .unwrap_or(u64::MAX),
        }
    }

    fn set_state(&self, next: McpInvocationState) {
        if let Ok(mut state) = self.state.lock() {
            *state = next;
        }
        if is_terminal_invocation_state(next) {
            self.settled.notify_waiters();
        }
    }

    fn cancel(&self, reason: McpOutcomeUnknownReason) {
        if let Ok(mut stored) = self.cancellation_reason.lock() {
            if stored.is_none() {
                *stored = Some(reason);
            }
        }
        self.cancellation.cancel();
    }

    fn cancellation_reason(&self) -> McpOutcomeUnknownReason {
        self.cancellation_reason
            .lock()
            .ok()
            .and_then(|reason| *reason)
            .unwrap_or(McpOutcomeUnknownReason::Cancelled)
    }

    async fn wait_terminal(&self) {
        loop {
            let notified = self.settled.notified();
            let terminal = self
                .state
                .lock()
                .map(|state| is_terminal_invocation_state(*state))
                .unwrap_or(true);
            if terminal {
                return;
            }
            notified.await;
        }
    }

    async fn wait_removed(&self) {
        loop {
            let notified = self.settled.notified();
            if self.removed.load(Ordering::Acquire) {
                return;
            }
            notified.await;
        }
    }
}

struct ActiveCallGuard {
    entry: Arc<ManagedEntry>,
    control: Arc<ActiveCallControl>,
    global_permit: Option<GlobalActiveCallPermit>,
}

struct GlobalActiveCallPermit {
    inner: Arc<ManagerInner>,
}

impl Drop for ActiveCallGuard {
    fn drop(&mut self) {
        let terminal = self
            .control
            .state
            .lock()
            .map(|state| is_terminal_invocation_state(*state))
            .unwrap_or(true);
        if !terminal {
            let next = if self.control.dispatch.certainty()
                == McpDispatchCertainty::DefinitelyNotDispatched
            {
                McpInvocationState::Cancelled
            } else {
                McpInvocationState::OutcomeUnknown
            };
            self.control.set_state(next);
        }
        let global_permit = self.global_permit.take();
        if let Ok(mut state) = self.entry.state.lock() {
            if state
                .active_calls
                .get(&self.control.id)
                .is_some_and(|active| Arc::ptr_eq(active, &self.control))
            {
                state.active_calls.remove(&self.control.id);
            }
            state.status.active_call_count = state.active_calls.len();
            // Release global admission before making the new per-Server count
            // observable. Publish while the same state lock is held so a new
            // call cannot publish count=1 and then be overwritten by this
            // older guard's count=0 snapshot.
            drop(global_permit);
            self.entry.publish_status(&state.status);
        } else {
            drop(global_permit);
            self.entry.settled.notify_waiters();
        }
        self.control.removed.store(true, Ordering::Release);
        self.control.settled.notify_waiters();
    }
}

impl Drop for GlobalActiveCallPermit {
    fn drop(&mut self) {
        let previous = self.inner.active_call_count.fetch_sub(1, Ordering::AcqRel);
        debug_assert!(previous > 0, "global MCP active-call count underflow");
    }
}

impl ManagedEntry {
    fn new(registry: &McpRegistryEntry) -> Self {
        let status = McpServerStatus::from_registry(registry);
        let mut catalog = McpCatalogSnapshot::empty(registry.config.id);
        catalog.source_config_epoch = Some(registry.config_epoch);
        catalog.source_registry_revision = Some(registry.revision);
        catalog.source_config_digest = Some(registry.config_digest.clone());
        let (status_sender, _) = watch::channel(status.clone());
        Self {
            state: StdMutex::new(ManagedEntryState {
                epoch: 0,
                event_sequence: 0,
                catalog,
                status,
                peer: None,
                closing_peer: None,
                connect_inflight: false,
                refresh_inflight: false,
                watcher_cancel: None,
                watcher_task: None,
                active_calls: BTreeMap::new(),
                removed: false,
            }),
            status: status_sender,
            settled: Notify::new(),
        }
    }

    fn publish_status(&self, status: &McpServerStatus) {
        self.status.send_replace(status.clone());
        self.settled.notify_waiters();
    }
}

struct ManagerInner {
    registry: Arc<dyn McpRegistry>,
    connector: Arc<dyn McpConnector>,
    policy: McpManagerPolicy,
    entries: StdMutex<BTreeMap<McpServerId, Arc<ManagedEntry>>>,
    lifecycle: StdMutex<ManagerLifecycle>,
    lifecycle_settled: Notify,
    shutdown_started: AtomicBool,
    shutdown_cancel: CancellationToken,
    registry_watcher_started: AtomicBool,
    active_call_count: AtomicUsize,
    events: mpsc::UnboundedSender<McpEvent>,
}

#[derive(Debug, Default)]
struct ManagerLifecycle {
    draining: bool,
    active_starts: usize,
}

struct StartPermit {
    inner: Arc<ManagerInner>,
}

struct DrainPermit {
    inner: Arc<ManagerInner>,
}

struct EntryInflightGuard {
    entry: Arc<ManagedEntry>,
}

impl Drop for StartPermit {
    fn drop(&mut self) {
        if let Ok(mut lifecycle) = self.inner.lifecycle.lock() {
            lifecycle.active_starts = lifecycle.active_starts.saturating_sub(1);
        }
        self.inner.lifecycle_settled.notify_waiters();
    }
}

impl Drop for DrainPermit {
    fn drop(&mut self) {
        if let Ok(mut lifecycle) = self.inner.lifecycle.lock() {
            lifecycle.draining = false;
        }
        self.inner.lifecycle_settled.notify_waiters();
    }
}

impl Drop for EntryInflightGuard {
    fn drop(&mut self) {
        let status = {
            let Ok(mut state) = self.entry.state.lock() else {
                return;
            };
            if !state.connect_inflight && !state.refresh_inflight {
                return;
            }
            state.connect_inflight = false;
            state.refresh_inflight = false;
            state.status.clone()
        };
        self.entry.publish_status(&status);
    }
}

#[derive(Clone, Debug)]
pub struct McpShutdownReport {
    pub results: Vec<McpBatchOperationResult>,
    /// True when the graceful deadline expired and the Manager invalidated every active entry,
    /// cancelled watchers and force-polled peer close before returning.
    pub forced: bool,
    /// True only when every peer close future settled before the force grace expired.
    pub cleanup_complete: bool,
}

#[derive(Clone)]
pub struct McpConnectionManager {
    inner: Arc<ManagerInner>,
}

impl fmt::Debug for McpConnectionManager {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let entry_count = self
            .inner
            .entries
            .lock()
            .map(|entries| entries.len())
            .unwrap_or_default();
        formatter
            .debug_struct("McpConnectionManager")
            .field("entry_count", &entry_count)
            .field("draining", &self.is_draining())
            .finish_non_exhaustive()
    }
}

impl McpConnectionManager {
    pub fn new(
        registry: Arc<dyn McpRegistry>,
        connector: Arc<dyn McpConnector>,
        event_sink: Arc<dyn McpEventSink>,
        policy: McpManagerPolicy,
    ) -> Result<Self, McpError> {
        policy.catalog.limits.validate()?;
        policy.security_limits.validate()?;
        let catalog_limits = &policy.catalog.limits;
        let security_limits = &policy.security_limits;
        if catalog_limits.max_pages > security_limits.max_catalog_pages
            || catalog_limits.max_tools > security_limits.max_tools
            || catalog_limits.max_schema_bytes_per_page > security_limits.max_schema_bytes_per_page
            || catalog_limits.max_total_schema_bytes > security_limits.max_total_schema_bytes
            || catalog_limits.max_descriptor_bytes_per_page
                > security_limits.max_descriptor_bytes_per_page
            || catalog_limits.max_total_descriptor_bytes
                > security_limits.max_total_descriptor_bytes
            || catalog_limits.max_cursor_bytes > security_limits.max_cursor_bytes
            || catalog_limits.max_model_name_bytes > security_limits.max_model_name_bytes
        {
            return Err(McpError::config(
                "MCP catalog limits exceed the centralized security policy",
            ));
        }
        if policy.notification_debounce > Duration::from_secs(60) {
            return Err(McpError::config(
                "MCP notification debounce must not exceed 60 seconds",
            ));
        }
        if policy.active_call_settle_timeout.is_zero()
            || policy.active_call_settle_timeout > Duration::from_secs(30)
        {
            return Err(McpError::config(
                "MCP active-call settlement timeout must be between 1 ms and 30 seconds",
            ));
        }
        if policy.max_active_calls_per_server == 0
            || policy.max_active_calls_total == 0
            || policy.max_active_calls_per_server > policy.max_active_calls_total
            || policy.max_active_calls_total > 4096
        {
            return Err(McpError::config(
                "MCP active-call limits must be non-zero, ordered, and at most 4096",
            ));
        }
        let runtime = tokio::runtime::Handle::try_current().map_err(|_| {
            McpError::config("MCP connection manager requires an active Tokio runtime")
        })?;
        let (events, mut event_receiver) = mpsc::unbounded_channel();
        runtime.spawn(async move {
            while let Some(event) = event_receiver.recv().await {
                event_sink.emit(event);
            }
        });
        let manager = Self {
            inner: Arc::new(ManagerInner {
                registry,
                connector,
                policy,
                entries: StdMutex::new(BTreeMap::new()),
                lifecycle: StdMutex::new(ManagerLifecycle::default()),
                lifecycle_settled: Notify::new(),
                shutdown_started: AtomicBool::new(false),
                shutdown_cancel: CancellationToken::new(),
                registry_watcher_started: AtomicBool::new(false),
                active_call_count: AtomicUsize::new(0),
                events,
            }),
        };
        manager.ensure_registry_watcher();
        Ok(manager)
    }

    pub fn without_events(
        registry: Arc<dyn McpRegistry>,
        connector: Arc<dyn McpConnector>,
        policy: McpManagerPolicy,
    ) -> Result<Self, McpError> {
        Self::new(registry, connector, Arc::new(NoopMcpEventSink), policy)
    }

    pub fn get_status(&self, server_id: McpServerId) -> Result<Option<McpServerStatus>, McpError> {
        if let Some(entry) = self.get_entry(server_id)? {
            return Ok(Some(lock_entry(&entry)?.status.clone()));
        }
        Ok(self
            .inner
            .registry
            .get(server_id)?
            .as_ref()
            .map(McpServerStatus::from_registry))
    }

    pub fn list_statuses(&self) -> Result<Vec<McpServerStatus>, McpError> {
        let registry = self.inner.registry.list()?;
        let entries = self
            .inner
            .entries
            .lock()
            .map_err(|_| McpError::protocol("MCP manager entries lock is unavailable"))?;
        let mut statuses = Vec::with_capacity(registry.len());
        for record in registry {
            if let Some(entry) = entries.get(&record.config.id) {
                statuses.push(lock_entry(entry)?.status.clone());
            } else {
                statuses.push(McpServerStatus::from_registry(&record));
            }
        }
        statuses.sort_by_key(|status| status.server_id);
        Ok(statuses)
    }

    pub fn catalog(&self, server_id: McpServerId) -> Result<Option<McpCatalogSnapshot>, McpError> {
        let Some(entry) = self.get_entry(server_id)? else {
            return Ok(None);
        };
        let catalog = lock_entry(&entry)?.catalog.clone();
        Ok(Some(catalog))
    }

    fn entry_for(&self, registry: &McpRegistryEntry) -> Result<Arc<ManagedEntry>, McpError> {
        let mut entries = self
            .inner
            .entries
            .lock()
            .map_err(|_| McpError::protocol("MCP manager entries lock is unavailable"))?;
        Ok(entries
            .entry(registry.config.id)
            .or_insert_with(|| Arc::new(ManagedEntry::new(registry)))
            .clone())
    }

    fn get_entry(&self, server_id: McpServerId) -> Result<Option<Arc<ManagedEntry>>, McpError> {
        let entries = self
            .inner
            .entries
            .lock()
            .map_err(|_| McpError::protocol("MCP manager entries lock is unavailable"))?;
        Ok(entries.get(&server_id).cloned())
    }

    fn reserve_start(&self) -> Result<StartPermit, McpError> {
        if self.inner.shutdown_started.load(Ordering::Acquire) {
            return Err(McpError::shutdown(
                "MCP connection manager has begun permanent shutdown",
            ));
        }
        let mut lifecycle = self
            .inner
            .lifecycle
            .lock()
            .map_err(|_| McpError::shutdown("MCP manager lifecycle lock is unavailable"))?;
        if lifecycle.draining || self.inner.shutdown_started.load(Ordering::Acquire) {
            return Err(McpError::shutdown(
                "MCP connection manager is stopping all servers",
            ));
        }
        lifecycle.active_starts = lifecycle
            .active_starts
            .checked_add(1)
            .ok_or_else(|| McpError::protocol("MCP active start count overflowed"))?;
        drop(lifecycle);
        Ok(StartPermit {
            inner: Arc::clone(&self.inner),
        })
    }

    fn is_draining(&self) -> bool {
        self.inner
            .lifecycle
            .lock()
            .map(|lifecycle| lifecycle.draining)
            .unwrap_or(true)
    }

    fn abandon_start_for_registry_change(
        &self,
        entry: &Arc<ManagedEntry>,
        epoch: u64,
        registry: Option<&McpRegistryEntry>,
    ) -> Result<bool, McpError> {
        let mut state = lock_entry(entry)?;
        state.connect_inflight = false;
        if let Some(registry) = registry {
            apply_registry_locked(&self.inner.events, entry, &mut state, registry);
        } else {
            state.removed = true;
        }
        let owns_reservation = state.epoch == epoch;
        if owns_reservation {
            state.status.protocol = None;
            state.status.notification_state = McpPeerNotificationState::Unknown;
            transition_locked(
                &self.inner.events,
                entry,
                &mut state,
                McpServerState::Disabled,
            );
        } else {
            let status = state.status.clone();
            entry.publish_status(&status);
        }
        Ok(owns_reservation)
    }

    fn remove_managed_entry_if_same(&self, server_id: McpServerId, expected: &Arc<ManagedEntry>) {
        let Ok(mut entries) = self.inner.entries.lock() else {
            return;
        };
        if entries
            .get(&server_id)
            .is_some_and(|entry| Arc::ptr_eq(entry, expected))
        {
            entries.remove(&server_id);
        }
    }
}

fn lock_entry(
    entry: &ManagedEntry,
) -> Result<std::sync::MutexGuard<'_, ManagedEntryState>, McpError> {
    entry
        .state
        .lock()
        .map_err(|_| McpError::protocol("MCP manager entry lock is unavailable"))
}

fn clear_closing_peer(entry: &Arc<ManagedEntry>, expected: &Arc<dyn McpPeer>) {
    let status = {
        let Ok(mut state) = entry.state.lock() else {
            return;
        };
        if state
            .closing_peer
            .as_ref()
            .is_none_or(|peer| !Arc::ptr_eq(peer, expected))
        {
            return;
        }
        state.closing_peer.take();
        state.status.clone()
    };
    entry.publish_status(&status);
}

fn apply_registry_locked(
    events: &mpsc::UnboundedSender<McpEvent>,
    _entry: &ManagedEntry,
    state: &mut ManagedEntryState,
    registry: &McpRegistryEntry,
) {
    let source_changed = state.catalog.source_config_epoch != Some(registry.config_epoch)
        || state.catalog.source_registry_revision != Some(registry.revision)
        || state.catalog.source_config_digest.as_ref() != Some(&registry.config_digest);
    state.status.enabled = registry.config.enabled;
    state.status.display_name = safe_display_name(&registry.config.display_name);
    state.status.scope = registry.config.scope.clone();
    state.status.trust = registry.config.trust;
    state.status.approval_mode = registry.config.approval_mode;
    state.status.config_epoch = registry.config_epoch;
    state.status.registry_revision = registry.revision;
    state.status.config_digest = registry.config_digest.clone();
    if source_changed {
        let mut invalidated = McpCatalogSnapshot::empty(registry.config.id);
        invalidated.generation = state.catalog.generation;
        invalidated.source_config_epoch = Some(registry.config_epoch);
        invalidated.source_registry_revision = Some(registry.revision);
        invalidated.source_config_digest = Some(registry.config_digest.clone());
        let catalog_changed = invalidated != state.catalog;
        state.catalog = invalidated;
        sync_catalog_status_from_state(state);
        if catalog_changed {
            emit_catalog_locked(events, state);
        }
    }
}

fn safe_display_name(value: &str) -> String {
    const MAX_BYTES: usize = 256;
    let mut output = String::new();
    for character in value.chars() {
        let character = if character.is_control() {
            '\u{fffd}'
        } else {
            character
        };
        if output.len() + character.len_utf8() > MAX_BYTES {
            break;
        }
        output.push(character);
    }
    output
}

fn next_epoch(epoch: u64) -> Result<u64, McpError> {
    epoch
        .checked_add(1)
        .ok_or_else(|| McpError::protocol("MCP connection epoch exhausted"))
}

fn transition_locked(
    events: &mpsc::UnboundedSender<McpEvent>,
    entry: &ManagedEntry,
    state: &mut ManagedEntryState,
    next: McpServerState,
) {
    let previous = state.status.state;
    if previous == next {
        let status = state.status.clone();
        entry.publish_status(&status);
        return;
    }
    state.status.state = next;
    state.event_sequence = state.event_sequence.saturating_add(1);
    let _ = events.send(McpEvent::ServerStateChanged {
        server_id: state.status.server_id,
        sequence: state.event_sequence,
        previous,
        current: next,
    });
    let status = state.status.clone();
    entry.publish_status(&status);
}

fn emit_catalog_locked(events: &mpsc::UnboundedSender<McpEvent>, state: &mut ManagedEntryState) {
    state.event_sequence = state.event_sequence.saturating_add(1);
    let _ = events.send(McpEvent::CatalogChanged {
        server_id: state.status.server_id,
        sequence: state.event_sequence,
        generation: state.catalog.generation,
        config_epoch: state.status.config_epoch,
        registry_revision: state.status.registry_revision,
        config_digest: state.status.config_digest.clone(),
        completeness: state.catalog.completeness.clone(),
        tool_count: state.catalog.tools.len(),
    });
}

fn set_error_locked(
    events: &mpsc::UnboundedSender<McpEvent>,
    entry: &ManagedEntry,
    state: &mut ManagedEntryState,
    error: &McpError,
) {
    let safe_error = McpSafeError::from(error);
    state.status.last_error = Some(safe_error.clone());
    transition_locked(events, entry, state, McpServerState::Error);
    state.event_sequence = state.event_sequence.saturating_add(1);
    let _ = events.send(McpEvent::ServerError {
        server_id: state.status.server_id,
        sequence: state.event_sequence,
        error: safe_error,
    });
    let status = state.status.clone();
    entry.publish_status(&status);
}

fn sync_catalog_status(status: &mut McpServerStatus, catalog: &McpCatalogSnapshot) {
    status.catalog_generation = catalog.generation;
    status.catalog_completeness = catalog.completeness.clone();
    status.tool_count = catalog.tools.len();
}

fn sync_catalog_status_from_state(state: &mut ManagedEntryState) {
    let generation = state.catalog.generation;
    let completeness = state.catalog.completeness.clone();
    let tool_count = state.catalog.tools.len();
    state.status.catalog_generation = generation;
    state.status.catalog_completeness = completeness;
    state.status.tool_count = tool_count;
}

fn catalog_safe_error() -> McpSafeError {
    McpSafeError {
        kind: crate::McpErrorKind::Protocol,
        code: "mcp_catalog_incomplete".to_string(),
        message: "The MCP tool catalog is incomplete or stale.".to_string(),
        exit_code: None,
    }
}

fn notification_safe_error() -> McpSafeError {
    McpSafeError {
        kind: crate::McpErrorKind::Protocol,
        code: "mcp_notifications_unavailable".to_string(),
        message: "MCP dynamic tool notifications are unavailable.".to_string(),
        exit_code: None,
    }
}

fn operation_result(
    server_id: McpServerId,
    result: Result<McpServerStatus, McpError>,
) -> McpBatchOperationResult {
    match result {
        Ok(status) => McpBatchOperationResult {
            server_id: Some(server_id),
            status: Some(status),
            error: None,
        },
        Err(error) => McpBatchOperationResult {
            server_id: Some(server_id),
            status: None,
            error: Some(McpSafeError::from(&error)),
        },
    }
}

async fn await_manager_operation<T>(task: JoinHandle<Result<T, McpError>>) -> Result<T, McpError> {
    task.await
        .map_err(|_| McpError::shutdown("MCP manager operation task failed"))?
}

fn manager_task_batch_failure() -> McpBatchOperationResult {
    let error = McpError::shutdown("MCP manager operation task failed");
    McpBatchOperationResult {
        server_id: None,
        status: None,
        error: Some(McpSafeError::from(&error)),
    }
}
