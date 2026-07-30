use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex, Weak};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, watch, Notify};
use tokio::task::{JoinHandle, JoinSet};
use tokio_util::sync::CancellationToken;

use crate::catalog::discover_catalog_with_limits;
use crate::limits::{validate_structured_content, validate_tool_arguments};
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

#[derive(Clone, Debug)]
pub struct McpManagerPolicy {
    pub catalog: McpCatalogPolicy,
    pub notification_debounce: Duration,
    pub active_call_settle_timeout: Duration,
    pub security_limits: McpSecurityLimits,
}

impl Default for McpManagerPolicy {
    fn default() -> Self {
        Self {
            catalog: McpCatalogPolicy::default(),
            notification_debounce: Duration::from_millis(100),
            active_call_settle_timeout: Duration::from_millis(250),
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
        let status = if let Ok(mut state) = self.entry.state.lock() {
            if state
                .active_calls
                .get(&self.control.id)
                .is_some_and(|active| Arc::ptr_eq(active, &self.control))
            {
                state.active_calls.remove(&self.control.id);
            }
            state.status.active_call_count = state.active_calls.len();
            Some(state.status.clone())
        } else {
            None
        };
        if let Some(status) = status {
            self.entry.publish_status(&status);
        } else {
            self.entry.settled.notify_waiters();
        }
        self.control.removed.store(true, Ordering::Release);
        self.control.settled.notify_waiters();
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

    pub async fn start(&self, server_id: McpServerId) -> Result<McpServerStatus, McpError> {
        self.ensure_registry_watcher();
        let permit = self.reserve_start()?;
        let manager = self.clone();
        await_manager_operation(tokio::spawn(async move {
            let _permit = permit;
            manager.start_reserved_until_shutdown(server_id).await
        }))
        .await
    }

    async fn start_reserved_until_shutdown(
        &self,
        server_id: McpServerId,
    ) -> Result<McpServerStatus, McpError> {
        tokio::select! {
            biased;
            _ = self.inner.shutdown_cancel.cancelled() => {
                Err(McpError::shutdown("MCP connection manager is shutting down"))
            }
            result = self.start_reserved(server_id) => result,
        }
    }

    async fn start_reserved(&self, server_id: McpServerId) -> Result<McpServerStatus, McpError> {
        let mut registry = self
            .inner
            .registry
            .get(server_id)?
            .ok_or_else(|| McpError::config("MCP server is not registered"))?;
        let entry = self.entry_for(&registry)?;
        if !registry.config.enabled {
            self.stop_entry(server_id, Arc::clone(&entry), false)
                .await?;
            let mut state = lock_entry(&entry)?;
            apply_registry_locked(&self.inner.events, &entry, &mut state, &registry);
            let status = state.status.clone();
            entry.publish_status(&status);
            return Ok(status);
        }

        loop {
            if registry.config.trust == McpTrustLevel::Untrusted {
                self.stop_entry(server_id, Arc::clone(&entry), false)
                    .await?;
                let error =
                    McpError::config("MCP server is not trusted for connection or invocation");
                let mut state = lock_entry(&entry)?;
                apply_registry_locked(&self.inner.events, &entry, &mut state, &registry);
                set_error_locked(&self.inner.events, &entry, &mut state, &error);
                return Err(error);
            }
            let notified = entry.settled.notified();
            let reservation = {
                let mut state = lock_entry(&entry)?;
                if state.removed {
                    return Err(McpError::config("MCP server is being removed"));
                }
                let same_config = state.status.config_epoch == registry.config_epoch
                    && state.status.registry_revision == registry.revision
                    && state.status.config_digest == registry.config_digest;
                if matches!(
                    state.status.state,
                    McpServerState::Ready | McpServerState::Degraded
                ) && state.peer.is_some()
                    && same_config
                {
                    return Ok(state.status.clone());
                }
                if state.connect_inflight
                    || state.refresh_inflight
                    || state.closing_peer.is_some()
                    || matches!(
                        state.status.state,
                        McpServerState::Starting
                            | McpServerState::Discovering
                            | McpServerState::Stopping
                    )
                {
                    None
                } else {
                    state.epoch = next_epoch(state.epoch)?;
                    let epoch = state.epoch;
                    state.connect_inflight = true;
                    apply_registry_locked(&self.inner.events, &entry, &mut state, &registry);
                    state.status.last_error = None;
                    let old_peer = state.peer.take();
                    state.closing_peer = old_peer.clone();
                    let old_cancel = state.watcher_cancel.take();
                    let old_watcher = state.watcher_task.take();
                    let active_calls = state.active_calls.values().cloned().collect::<Vec<_>>();
                    transition_locked(
                        &self.inner.events,
                        &entry,
                        &mut state,
                        McpServerState::Starting,
                    );
                    Some((epoch, old_peer, old_cancel, old_watcher, active_calls))
                }
            };
            let Some((epoch, old_peer, old_cancel, old_watcher, active_calls)) = reservation else {
                notified.await;
                continue;
            };
            let _inflight_guard = EntryInflightGuard {
                entry: Arc::clone(&entry),
            };

            if let Some(cancel) = old_cancel {
                cancel.cancel();
            }
            cancel_active_calls(&active_calls, McpOutcomeUnknownReason::ServerRestarted);
            let _ =
                settle_active_calls(&active_calls, self.inner.policy.active_call_settle_timeout)
                    .await;
            if let Some(peer) = old_peer {
                let _ = peer.close().await;
                clear_closing_peer(&entry, &peer);
            }
            if let Some(watcher) = old_watcher {
                let _ = watcher.await;
            }

            let current_registry = self.inner.registry.get(server_id)?;
            if current_registry.as_ref().is_none_or(|current| {
                current.revision != registry.revision
                    || current.config_digest != registry.config_digest
                    || !current.config.enabled
            }) {
                let owns_reservation = self.abandon_start_for_registry_change(
                    &entry,
                    epoch,
                    current_registry.as_ref(),
                )?;
                if current_registry.is_none() {
                    self.remove_managed_entry_if_same(server_id, &entry);
                    return Err(McpError::config("MCP server is no longer registered"));
                }
                let current = current_registry.expect("registry entry checked as present");
                if !owns_reservation {
                    return Err(McpError::cancelled("MCP server start"));
                }
                if !current.config.enabled {
                    return self
                        .get_status(server_id)?
                        .ok_or_else(|| McpError::config("MCP server status disappeared"));
                }
                registry = current;
                continue;
            }

            let connected = self.inner.connector.connect(&registry.config).await;
            let peer = match connected {
                Ok(peer) => peer,
                Err(error) => {
                    let current_registry = self.inner.registry.get(server_id)?;
                    if current_registry.as_ref().is_none_or(|current| {
                        current.revision != registry.revision
                            || current.config_digest != registry.config_digest
                            || !current.config.enabled
                    }) {
                        let owns_reservation = self.abandon_start_for_registry_change(
                            &entry,
                            epoch,
                            current_registry.as_ref(),
                        )?;
                        if current_registry.is_none() {
                            self.remove_managed_entry_if_same(server_id, &entry);
                            return Err(McpError::config("MCP server is no longer registered"));
                        }
                        let current = current_registry.expect("registry entry checked as present");
                        if !owns_reservation {
                            return Err(McpError::cancelled("MCP server start"));
                        }
                        if !current.config.enabled {
                            return self
                                .get_status(server_id)?
                                .ok_or_else(|| McpError::config("MCP server status disappeared"));
                        }
                        registry = current;
                        continue;
                    }
                    let mut state = lock_entry(&entry)?;
                    state.connect_inflight = false;
                    if state.epoch == epoch && state.status.state == McpServerState::Starting {
                        set_error_locked(&self.inner.events, &entry, &mut state, &error);
                    } else {
                        let status = state.status.clone();
                        entry.publish_status(&status);
                    }
                    return Err(error);
                }
            };
            if peer.server_id() != server_id {
                let error = McpError::protocol(
                    "MCP connector returned a peer for a different server identity",
                );
                let _ = peer.close().await;
                let mut state = lock_entry(&entry)?;
                state.connect_inflight = false;
                if state.epoch == epoch && state.status.state == McpServerState::Starting {
                    set_error_locked(&self.inner.events, &entry, &mut state, &error);
                } else {
                    let status = state.status.clone();
                    entry.publish_status(&status);
                }
                return Err(error);
            }
            if let Err(error) = self
                .inner
                .policy
                .security_limits
                .validate_protocol_snapshot(peer.protocol_snapshot())
            {
                let _ = peer.close().await;
                let mut state = lock_entry(&entry)?;
                state.connect_inflight = false;
                if state.epoch == epoch && state.status.state == McpServerState::Starting {
                    set_error_locked(&self.inner.events, &entry, &mut state, &error);
                } else {
                    let status = state.status.clone();
                    entry.publish_status(&status);
                }
                return Err(error);
            }

            let stale = {
                let state = lock_entry(&entry)?;
                state.epoch != epoch
                    || state.removed
                    || state.status.state != McpServerState::Starting
            };
            if stale {
                let _ = peer.close().await;
                let mut state = lock_entry(&entry)?;
                state.connect_inflight = false;
                let status = state.status.clone();
                entry.publish_status(&status);
                return Err(McpError::cancelled("MCP server start"));
            }

            let current_registry = self.inner.registry.get(server_id)?;
            if current_registry.as_ref().is_none_or(|current| {
                current.revision != registry.revision
                    || current.config_digest != registry.config_digest
                    || !current.config.enabled
            }) {
                let _ = peer.close().await;
                let owns_reservation = self.abandon_start_for_registry_change(
                    &entry,
                    epoch,
                    current_registry.as_ref(),
                )?;
                if current_registry.is_none() {
                    self.remove_managed_entry_if_same(server_id, &entry);
                    return Err(McpError::config("MCP server is no longer registered"));
                }
                let current = current_registry.expect("registry entry checked as present");
                if !owns_reservation {
                    return Err(McpError::cancelled("MCP server start"));
                }
                if !current.config.enabled {
                    return self
                        .get_status(server_id)?
                        .ok_or_else(|| McpError::config("MCP server status disappeared"));
                }
                registry = current;
                continue;
            }

            let signal_receiver = peer.subscribe_signals();
            let initial_signal_snapshot = signal_receiver
                .as_ref()
                .map(crate::McpPeerSignalReceiver::snapshot);
            let notification_state = initial_signal_snapshot
                .map(|snapshot| snapshot.notification_state)
                .unwrap_or(McpPeerNotificationState::Unsupported);
            let watcher_cancel = CancellationToken::new();
            let commit_stale = {
                let mut state = lock_entry(&entry)?;
                if state.epoch != epoch || state.status.state != McpServerState::Starting {
                    true
                } else {
                    state.connect_inflight = false;
                    state.refresh_inflight = true;
                    state.status.protocol = Some(peer.protocol_snapshot().clone());
                    state.status.notification_state = notification_state;
                    state.peer = Some(Arc::clone(&peer));
                    state.watcher_cancel = Some(watcher_cancel.clone());
                    transition_locked(
                        &self.inner.events,
                        &entry,
                        &mut state,
                        McpServerState::Discovering,
                    );
                    false
                }
            };
            if commit_stale {
                let _ = peer.close().await;
                let mut state = lock_entry(&entry)?;
                state.connect_inflight = false;
                let status = state.status.clone();
                entry.publish_status(&status);
                return Err(McpError::cancelled("MCP server start"));
            }

            if let Some(signals) = signal_receiver {
                let watcher_manager = self.clone();
                let watcher_entry = Arc::clone(&entry);
                let watcher = tokio::spawn(async move {
                    watcher_manager
                        .watch_peer_signals(
                            server_id,
                            watcher_entry,
                            epoch,
                            signals,
                            initial_signal_snapshot
                                .expect("signal snapshot exists when receiver exists"),
                            watcher_cancel,
                        )
                        .await;
                });
                let mut pending_watcher = Some(watcher);
                {
                    let mut state = lock_entry(&entry)?;
                    if state.epoch == epoch && !state.removed {
                        state.watcher_task = pending_watcher.take();
                    }
                }
                if let Some(watcher) = pending_watcher {
                    watcher.abort();
                    let _ = watcher.await;
                }
            }

            let _ = self
                .finish_refresh(server_id, Arc::clone(&entry), epoch, peer)
                .await?;
            return self
                .get_status(server_id)?
                .ok_or_else(|| McpError::config("MCP server status disappeared"));
        }
    }

    pub async fn stop(&self, server_id: McpServerId) -> Result<McpServerStatus, McpError> {
        let manager = self.clone();
        await_manager_operation(tokio::spawn(async move {
            tokio::select! {
                biased;
                _ = manager.inner.shutdown_cancel.cancelled() => {
                    Err(McpError::shutdown("MCP connection manager is shutting down"))
                }
                result = manager.stop_owned(server_id) => result,
            }
        }))
        .await
    }

    async fn stop_owned(&self, server_id: McpServerId) -> Result<McpServerStatus, McpError> {
        self.ensure_registry_watcher();
        let entry = match self.get_entry(server_id)? {
            Some(entry) => entry,
            None => {
                let registry = self
                    .inner
                    .registry
                    .get(server_id)?
                    .ok_or_else(|| McpError::config("MCP server is not registered"))?;
                self.entry_for(&registry)?
            }
        };
        self.stop_entry(server_id, entry, false).await
    }

    pub async fn restart(&self, server_id: McpServerId) -> Result<McpServerStatus, McpError> {
        let manager = self.clone();
        await_manager_operation(tokio::spawn(async move {
            tokio::select! {
                biased;
                _ = manager.inner.shutdown_cancel.cancelled() => {
                    Err(McpError::shutdown("MCP connection manager is shutting down"))
                }
                result = async {
                    manager.stop_owned(server_id).await?;
                    let _permit = manager.reserve_start()?;
                    manager.start_reserved_until_shutdown(server_id).await
                } => result,
            }
        }))
        .await
    }

    pub async fn refresh(&self, server_id: McpServerId) -> Result<McpCatalogSnapshot, McpError> {
        let manager = self.clone();
        await_manager_operation(tokio::spawn(async move {
            tokio::select! {
                biased;
                _ = manager.inner.shutdown_cancel.cancelled() => {
                    Err(McpError::shutdown("MCP connection manager is shutting down"))
                }
                result = manager.refresh_owned(server_id) => result,
            }
        }))
        .await
    }

    async fn refresh_owned(&self, server_id: McpServerId) -> Result<McpCatalogSnapshot, McpError> {
        self.ensure_registry_watcher();
        let entry = self
            .get_entry(server_id)?
            .ok_or_else(|| McpError::config("MCP server is not connected"))?;
        loop {
            let notified = entry.settled.notified();
            let reserved = {
                let mut state = lock_entry(&entry)?;
                if state.removed {
                    return Err(McpError::config("MCP server is being removed"));
                }
                if state.refresh_inflight
                    || matches!(
                        state.status.state,
                        McpServerState::Starting | McpServerState::Stopping
                    )
                {
                    None
                } else {
                    let peer = state
                        .peer
                        .as_ref()
                        .cloned()
                        .ok_or_else(|| McpError::protocol("MCP server is not ready"))?;
                    state.refresh_inflight = true;
                    let epoch = state.epoch;
                    transition_locked(
                        &self.inner.events,
                        &entry,
                        &mut state,
                        McpServerState::Discovering,
                    );
                    Some((epoch, peer))
                }
            };
            let Some((epoch, peer)) = reserved else {
                notified.await;
                continue;
            };
            let _inflight_guard = EntryInflightGuard {
                entry: Arc::clone(&entry),
            };
            return self
                .finish_refresh(server_id, Arc::clone(&entry), epoch, peer)
                .await;
        }
    }

    pub async fn start_enabled(&self) -> Vec<McpBatchOperationResult> {
        self.ensure_registry_watcher();
        let entries = match self.inner.registry.list() {
            Ok(entries) => entries,
            Err(error) => {
                return vec![McpBatchOperationResult {
                    server_id: None,
                    status: None,
                    error: Some(McpSafeError::from(&error)),
                }];
            }
        };
        let mut tasks = Vec::new();
        let mut results = Vec::new();
        for registry in entries.into_iter().filter(|entry| entry.config.enabled) {
            match self.reserve_start() {
                Ok(permit) => {
                    let manager = self.clone();
                    tasks.push(tokio::spawn(async move {
                        let _permit = permit;
                        let result = manager
                            .start_reserved_until_shutdown(registry.config.id)
                            .await;
                        operation_result(registry.config.id, result)
                    }));
                }
                Err(error) => {
                    results.push(operation_result(registry.config.id, Err(error)));
                }
            }
        }
        for task in tasks {
            let joined = task.await;
            if let Ok(result) = joined {
                results.push(result);
            }
        }
        results.sort_by_key(|result| result.server_id);
        results
    }

    pub async fn stop_all(&self) -> Vec<McpBatchOperationResult> {
        let operation_manager = self.clone();
        match tokio::spawn(async move {
            tokio::select! {
                biased;
                _ = operation_manager.inner.shutdown_cancel.cancelled() => {
                    vec![manager_task_batch_failure()]
                }
                results = operation_manager.stop_all_owned() => results,
            }
        })
        .await
        {
            Ok(results) => results,
            Err(_) => vec![manager_task_batch_failure()],
        }
    }

    /// Permanently stop this Manager within a Host-owned deadline.
    ///
    /// Unlike reusable [`Self::stop_all`], this rejects future starts and cancels every admitted
    /// start. A reserved tail of the deadline is used to invalidate entries, abort notification
    /// watchers and poll peer close concurrently if graceful shutdown does not settle.
    pub async fn shutdown(&self, timeout: Duration) -> McpShutdownReport {
        self.inner.shutdown_started.store(true, Ordering::Release);
        self.inner.shutdown_cancel.cancel();

        let force_grace = timeout.min(FORCE_SHUTDOWN_GRACE_MAX);
        let graceful_grace = timeout.saturating_sub(force_grace);
        if !graceful_grace.is_zero() {
            if let Ok(results) = tokio::time::timeout(graceful_grace, self.stop_all_owned()).await {
                let cleanup_complete = self.graceful_shutdown_cleanup_complete(&results);
                return McpShutdownReport {
                    results,
                    forced: false,
                    cleanup_complete,
                };
            }
        }

        let (results, cleanup_complete) = self.force_shutdown_entries(force_grace).await;
        McpShutdownReport {
            results,
            forced: true,
            cleanup_complete,
        }
    }

    fn graceful_shutdown_cleanup_complete(&self, results: &[McpBatchOperationResult]) -> bool {
        if results.iter().any(|result| {
            result.server_id.is_none()
                || result.error.is_some()
                || result.status.as_ref().is_none_or(|status| {
                    status.state != McpServerState::Disabled || status.active_call_count != 0
                })
        }) {
            return false;
        }
        let Ok(entries) = self.inner.entries.lock() else {
            return false;
        };
        if entries.len() != results.len() {
            return false;
        }
        if !entries.values().all(|entry| {
            entry.state.lock().is_ok_and(|state| {
                state.peer.is_none()
                    && state.closing_peer.is_none()
                    && state.watcher_cancel.is_none()
                    && state.watcher_task.is_none()
                    && !state.connect_inflight
                    && !state.refresh_inflight
                    && state.active_calls.is_empty()
                    && state.status.state == McpServerState::Disabled
                    && state.status.active_call_count == 0
            })
        }) {
            return false;
        }
        self.inner
            .lifecycle
            .lock()
            .is_ok_and(|lifecycle| !lifecycle.draining && lifecycle.active_starts == 0)
    }

    async fn force_shutdown_entries(
        &self,
        force_grace: Duration,
    ) -> (Vec<McpBatchOperationResult>, bool) {
        let (entries, mut cleanup_complete) = match self.inner.entries.lock() {
            Ok(entries) => (
                entries
                    .iter()
                    .map(|(server_id, entry)| (*server_id, Arc::clone(entry)))
                    .collect::<Vec<_>>(),
                true,
            ),
            Err(poisoned) => (
                poisoned
                    .into_inner()
                    .iter()
                    .map(|(server_id, entry)| (*server_id, Arc::clone(entry)))
                    .collect::<Vec<_>>(),
                false,
            ),
        };
        let mut tasks = JoinSet::new();
        let mut results = Vec::with_capacity(entries.len());
        let mut active_calls_to_verify = Vec::new();
        for (server_id, entry) in &entries {
            let (peers, cancel, watcher, active_calls, status) = {
                let mut state = match entry.state.lock() {
                    Ok(state) => state,
                    Err(poisoned) => {
                        cleanup_complete = false;
                        poisoned.into_inner()
                    }
                };
                state.removed = true;
                state.epoch = state.epoch.saturating_add(1);
                state.connect_inflight = false;
                state.refresh_inflight = false;
                let mut peers = Vec::new();
                if let Some(peer) = state.peer.take() {
                    peers.push(peer);
                }
                if let Some(peer) = state.closing_peer.take() {
                    if !peers.iter().any(|active| Arc::ptr_eq(active, &peer)) {
                        peers.push(peer);
                    }
                }
                let cancel = state.watcher_cancel.take();
                let watcher = state.watcher_task.take();
                let active_calls = state.active_calls.values().cloned().collect::<Vec<_>>();
                let generation = state.catalog.generation;
                state.catalog = McpCatalogSnapshot::empty(*server_id);
                state.catalog.generation = generation;
                sync_catalog_status_from_state(&mut state);
                state.status.protocol = None;
                state.status.notification_state = McpPeerNotificationState::Unknown;
                state.status.last_error = None;
                transition_locked(
                    &self.inner.events,
                    entry,
                    &mut state,
                    McpServerState::Disabled,
                );
                (peers, cancel, watcher, active_calls, state.status.clone())
            };
            if let Some(cancel) = cancel {
                cancel.cancel();
            }
            if let Some(watcher) = watcher {
                watcher.abort();
                tasks.spawn(async move {
                    match watcher.await {
                        Ok(()) => true,
                        Err(error) => error.is_cancelled(),
                    }
                });
            }
            cancel_active_calls(&active_calls, McpOutcomeUnknownReason::Shutdown);
            if !active_calls.is_empty() {
                active_calls_to_verify.extend(active_calls.iter().cloned());
                tasks.spawn(async move {
                    wait_for_active_call_removal(&active_calls).await;
                    true
                });
            }
            for peer in peers {
                let _ = peer.force_close();
                tasks.spawn(async move { peer.close().await.is_ok() });
            }
            results.push(McpBatchOperationResult {
                server_id: Some(*server_id),
                status: Some(status),
                error: None,
            });
        }

        let tasks_complete: bool = tokio::time::timeout(force_grace, async {
            let mut complete = true;
            while let Some(result) = tasks.join_next().await {
                complete &= result.unwrap_or(false);
            }
            complete
        })
        .await
        .unwrap_or_default();
        cleanup_complete &= tasks_complete;

        if !tasks_complete {
            tasks.abort_all();
            while tasks.join_next().await.is_some() {}
        }
        cleanup_complete &= active_calls_to_verify.iter().all(|active| {
            active.removed.load(Ordering::Acquire)
                && active
                    .state
                    .lock()
                    .map(|state| is_terminal_invocation_state(*state))
                    .unwrap_or(false)
        });
        for (server_id, entry) in &entries {
            let state = match entry.state.lock() {
                Ok(state) => state,
                Err(poisoned) => {
                    cleanup_complete = false;
                    poisoned.into_inner()
                }
            };
            let entry_complete = state.peer.is_none()
                && state.closing_peer.is_none()
                && state.watcher_cancel.is_none()
                && state.watcher_task.is_none()
                && !state.connect_inflight
                && !state.refresh_inflight
                && state.active_calls.is_empty()
                && state.status.active_call_count == 0;
            cleanup_complete &= entry_complete;
            if let Some(result) = results
                .iter_mut()
                .find(|result| result.server_id == Some(*server_id))
            {
                result.status = Some(state.status.clone());
            }
        }
        results.sort_by_key(|result| result.server_id);
        (results, cleanup_complete)
    }

    async fn stop_all_owned(&self) -> Vec<McpBatchOperationResult> {
        self.ensure_registry_watcher();
        loop {
            let settled = self.inner.lifecycle_settled.notified();
            let became_owner = match self.inner.lifecycle.lock() {
                Ok(mut lifecycle) if !lifecycle.draining => {
                    lifecycle.draining = true;
                    true
                }
                Ok(_) => false,
                Err(_) => return Vec::new(),
            };
            if became_owner {
                break;
            }
            settled.await;
            if !self.is_draining() {
                return self
                    .list_statuses()
                    .unwrap_or_default()
                    .into_iter()
                    .map(|status| McpBatchOperationResult {
                        server_id: Some(status.server_id),
                        status: Some(status),
                        error: None,
                    })
                    .collect();
            }
        }
        let _drain_permit = DrainPermit {
            inner: Arc::clone(&self.inner),
        };
        loop {
            let settled = self.inner.lifecycle_settled.notified();
            let active_starts = self
                .inner
                .lifecycle
                .lock()
                .map(|lifecycle| lifecycle.active_starts)
                .unwrap_or_default();
            if active_starts == 0 {
                break;
            }
            settled.await;
        }
        let ids = self
            .inner
            .entries
            .lock()
            .map(|entries| entries.keys().copied().collect::<Vec<_>>())
            .unwrap_or_default();
        let mut tasks = JoinSet::new();
        for server_id in ids {
            let manager = self.clone();
            tasks.spawn(async move {
                let result = manager.stop_owned(server_id).await;
                operation_result(server_id, result)
            });
        }
        let mut results = Vec::new();
        while let Some(joined) = tasks.join_next().await {
            if let Ok(result) = joined {
                results.push(result);
            }
        }
        results.sort_by_key(|result| result.server_id);
        results
    }

    pub async fn remove_server(
        &self,
        server_id: McpServerId,
    ) -> Result<Option<McpRegistryEntry>, McpError> {
        let manager = self.clone();
        await_manager_operation(tokio::spawn(async move {
            tokio::select! {
                biased;
                _ = manager.inner.shutdown_cancel.cancelled() => {
                    Err(McpError::shutdown("MCP connection manager is shutting down"))
                }
                result = manager.remove_server_owned(server_id) => result,
            }
        }))
        .await
    }

    async fn remove_server_owned(
        &self,
        server_id: McpServerId,
    ) -> Result<Option<McpRegistryEntry>, McpError> {
        self.ensure_registry_watcher();
        let entry = match self.get_entry(server_id)? {
            Some(entry) => Some(entry),
            None => self
                .inner
                .registry
                .get(server_id)?
                .map(|registry| self.entry_for(&registry))
                .transpose()?,
        };
        if let Some(entry) = entry {
            {
                let mut state = lock_entry(&entry)?;
                state.removed = true;
            }
            self.stop_entry(server_id, Arc::clone(&entry), true).await?;
        }
        let removed = self.inner.registry.remove(server_id)?;
        if let Ok(mut entries) = self.inner.entries.lock() {
            entries.remove(&server_id);
        }
        Ok(removed)
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

    pub fn active_call(
        &self,
        id: &McpActiveCallId,
    ) -> Result<Option<McpActiveCallSnapshot>, McpError> {
        let Some(entry) = self.get_entry(id.server_id)? else {
            return Ok(None);
        };
        let snapshot = lock_entry(&entry)?
            .active_calls
            .get(id)
            .map(|call| call.snapshot());
        Ok(snapshot)
    }

    pub fn list_active_calls(
        &self,
        server_id: Option<McpServerId>,
    ) -> Result<Vec<McpActiveCallSnapshot>, McpError> {
        let entries = self
            .inner
            .entries
            .lock()
            .map_err(|_| McpError::protocol("MCP manager entries lock is unavailable"))?;
        let mut calls = Vec::new();
        for (entry_server_id, entry) in entries.iter() {
            if server_id.is_some_and(|expected| expected != *entry_server_id) {
                continue;
            }
            calls.extend(
                lock_entry(entry)?
                    .active_calls
                    .values()
                    .map(|call| call.snapshot()),
            );
        }
        calls.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(calls)
    }

    pub fn active_call_count(&self) -> Result<usize, McpError> {
        Ok(self.list_active_calls(None)?.len())
    }

    pub fn resolve_model_name(&self, model_name: &str) -> Result<Option<McpToolId>, McpError> {
        let entries = self
            .inner
            .entries
            .lock()
            .map_err(|_| McpError::protocol("MCP manager entries lock is unavailable"))?;
        let mut resolved = None;
        for entry in entries.values() {
            let state = lock_entry(entry)?;
            if state.removed
                || !state.status.enabled
                || state.status.trust == McpTrustLevel::Untrusted
                || !matches!(
                    state.status.state,
                    McpServerState::Ready | McpServerState::Degraded
                )
                || state.peer.is_none()
                || state.catalog.source_config_epoch != Some(state.status.config_epoch)
                || state.catalog.source_registry_revision != Some(state.status.registry_revision)
                || state.catalog.source_config_digest.as_ref() != Some(&state.status.config_digest)
                || state.catalog.completeness != McpCatalogCompleteness::Complete
            {
                continue;
            }
            if let Some(tool_id) = state.catalog.resolve_model_name(model_name) {
                if resolved.is_some() {
                    return Err(McpError::protocol(
                        "MCP model tool name is ambiguous in the catalog",
                    ));
                }
                resolved = Some(tool_id.clone());
            }
        }
        Ok(resolved)
    }

    /// Invoke a tool only if the caller's catalog and configuration snapshot
    /// still identify the active, complete catalog.
    ///
    /// All routing checks happen while briefly holding the per-server state
    /// lock. The peer and raw call are cloned out before the protocol request
    /// is awaited.
    pub async fn call_catalog_tool(
        &self,
        request: McpCatalogToolCall,
        cancellation: McpCancellationToken,
    ) -> Result<McpToolResult, McpError> {
        let invocation_id = McpInvocationId::new();
        let model_call_id = McpModelCallId::new(format!("legacy-{invocation_id}"))?;
        let id = McpActiveCallId::new(request.tool_id.server_id, invocation_id, model_call_id);
        self.call_catalog_tool_identified(id, request, cancellation)
            .await
    }

    /// Invoke a catalog-bound tool using the Host/Runtime identity that will
    /// also appear in approval records, traces and checkpoints.
    pub async fn call_catalog_tool_identified(
        &self,
        id: McpActiveCallId,
        request: McpCatalogToolCall,
        cancellation: McpCancellationToken,
    ) -> Result<McpToolResult, McpError> {
        let server_id = request.tool_id.server_id;
        if id.server_id != server_id {
            return Err(McpError::config(
                "MCP active-call identity does not match the catalog route",
            ));
        }
        if cancellation.is_cancelled() {
            return Err(McpError::cancelled("MCP tools/call"));
        }
        let registry =
            self.inner.registry.get(server_id)?.ok_or_else(|| {
                McpError::config("MCP catalog invocation server is not registered")
            })?;
        if !registry.config.enabled {
            return Err(McpError::config(
                "MCP catalog invocation server is disabled",
            ));
        }
        if registry.config.trust == McpTrustLevel::Untrusted {
            return Err(McpError::config(
                "MCP catalog invocation server is not trusted",
            ));
        }
        if registry.config.approval_mode == McpApprovalMode::Deny {
            return Err(McpError::config(
                "MCP tool invocation is denied by server approval policy",
            ));
        }
        validate_tool_arguments(&request.arguments, &self.inner.policy.security_limits)?;
        let entry = self
            .get_entry(server_id)?
            .ok_or_else(|| McpError::config("MCP catalog invocation server is not ready"))?;

        let (peer, call, control, active_status) = {
            let mut state = lock_entry(&entry)?;
            if state.removed {
                return Err(McpError::config(
                    "MCP catalog invocation server is not registered",
                ));
            }
            if !state.status.enabled {
                return Err(McpError::config(
                    "MCP catalog invocation server is disabled",
                ));
            }
            if state.status.trust == McpTrustLevel::Untrusted {
                return Err(McpError::config(
                    "MCP catalog invocation server is not trusted",
                ));
            }
            if state.status.approval_mode == McpApprovalMode::Deny {
                return Err(McpError::config(
                    "MCP tool invocation is denied by server approval policy",
                ));
            }
            if state.status.state != McpServerState::Ready {
                return Err(McpError::config(
                    "MCP catalog invocation server is not ready",
                ));
            }
            let peer = state.peer.clone().ok_or_else(|| {
                McpError::protocol("MCP catalog invocation active peer is unavailable")
            })?;
            if peer.server_id() != server_id || state.catalog.server_id != server_id {
                return Err(McpError::protocol(
                    "MCP catalog invocation server identity is inconsistent",
                ));
            }
            if state.catalog.completeness != McpCatalogCompleteness::Complete {
                return Err(McpError::config(
                    "MCP catalog invocation catalog is incomplete",
                ));
            }
            if state.status.config_epoch != registry.config_epoch
                || state.status.registry_revision != registry.revision
                || request.expected_config_epoch != state.status.config_epoch
                || request.expected_registry_revision != state.status.registry_revision
                || state.catalog.source_config_epoch != Some(state.status.config_epoch)
                || state.catalog.source_registry_revision != Some(state.status.registry_revision)
                || state.status.config_digest != registry.config_digest
                || state.catalog.source_config_digest.as_ref() != Some(&state.status.config_digest)
                || request.expected_config_digest != state.status.config_digest
            {
                return Err(McpError::config(
                    "MCP catalog invocation configuration snapshot is stale",
                ));
            }
            if request.expected_catalog_generation != state.catalog.generation {
                return Err(McpError::config(
                    "MCP catalog invocation generation is stale",
                ));
            }
            if state.catalog.content_digest.as_ref() != Some(&request.expected_catalog_digest) {
                return Err(McpError::config(
                    "MCP catalog invocation content digest is stale",
                ));
            }
            let tool = state
                .catalog
                .tools
                .iter()
                .find(|tool| tool.id == request.tool_id)
                .ok_or_else(|| McpError::config("MCP catalog invocation tool is unavailable"))?;
            if !tool.routable {
                return Err(McpError::config(
                    "MCP catalog invocation tool is not routable",
                ));
            }
            if tool.raw_name != request.tool_id.raw_name
                || tool.model_name != request.expected_model_name
                || tool.schema_digest != request.expected_schema_digest
            {
                return Err(McpError::config(
                    "MCP catalog invocation tool route is stale",
                ));
            }
            let timeout_ms = request
                .timeout_ms
                .filter(|timeout_ms| *timeout_ms > 0)
                .map(|timeout_ms| timeout_ms.min(registry.config.request_timeout_ms))
                .unwrap_or(registry.config.request_timeout_ms)
                .min(self.inner.policy.security_limits.max_tool_timeout_ms);
            let provenance = McpActiveCallProvenance {
                tool_id: request.tool_id.clone(),
                model_name: request.expected_model_name.clone(),
                config_epoch: request.expected_config_epoch,
                registry_revision: request.expected_registry_revision,
                config_digest: request.expected_config_digest.clone(),
                catalog_generation: request.expected_catalog_generation,
                catalog_digest: request.expected_catalog_digest.clone(),
                schema_digest: request.expected_schema_digest.clone(),
            };
            let call = McpToolCall {
                name: request.tool_id.raw_name.clone(),
                arguments: request.arguments,
                timeout_ms: Some(timeout_ms),
            };
            if state.active_calls.contains_key(&id) {
                return Err(McpError::config(
                    "MCP active-call identity is already in use",
                ));
            }
            let control = Arc::new(ActiveCallControl::new(id.clone(), provenance, timeout_ms));
            state.active_calls.insert(id, Arc::clone(&control));
            state.status.active_call_count = state.active_calls.len();
            let active_status = state.status.clone();
            (peer, call, control, active_status)
        };

        entry.publish_status(&active_status);
        let _active_guard = ActiveCallGuard {
            entry: Arc::clone(&entry),
            control: Arc::clone(&control),
        };
        // This is the logical dispatch linearization point. A configuration
        // mutation committed before the following Registry read is definitely
        // not dispatched. A mutation committed after this point races a
        // possibly-dispatched call and is conservatively settled by the
        // Registry watcher as outcome-unknown.
        control.dispatch.mark_dispatching();
        let final_registry =
            self.inner.registry.get(server_id)?.ok_or_else(|| {
                McpError::config("MCP catalog invocation server is not registered")
            })?;
        if final_registry.config_epoch != request.expected_config_epoch
            || final_registry.revision != request.expected_registry_revision
            || final_registry.config_digest != request.expected_config_digest
            || !final_registry.config.enabled
            || final_registry.config.trust == McpTrustLevel::Untrusted
            || final_registry.config.approval_mode == McpApprovalMode::Deny
        {
            return Err(McpError::config(
                "MCP catalog invocation configuration epoch is stale",
            ));
        }
        {
            let state = lock_entry(&entry)?;
            if state.removed
                || state.status.state != McpServerState::Ready
                || state.status.config_epoch != request.expected_config_epoch
                || state.status.registry_revision != request.expected_registry_revision
                || state.catalog.source_config_epoch != Some(request.expected_config_epoch)
                || state.catalog.source_registry_revision
                    != Some(request.expected_registry_revision)
                || state
                    .active_calls
                    .get(&control.id)
                    .is_none_or(|active| !Arc::ptr_eq(active, &control))
            {
                return Err(McpError::config(
                    "MCP catalog invocation configuration epoch is stale",
                ));
            }
        }
        if self.inner.shutdown_cancel.is_cancelled()
            || cancellation.is_cancelled()
            || control.cancellation.is_cancelled()
        {
            return Err(McpError::cancelled("MCP tools/call"));
        }
        // An SDK request handle only proves local queuing, not whether bytes
        // reached the server, so cross the uncertainty boundary immediately
        // before entering the peer.
        control.dispatch.mark_request_queued();
        control.set_state(McpInvocationState::Running);
        let mut protocol_call =
            peer.call_tool_tracked(call, control.cancellation.clone(), control.dispatch.clone());
        let deadline = tokio::time::sleep_until(control.deadline);
        tokio::pin!(deadline);
        let result = tokio::select! {
            biased;
            _ = self.inner.shutdown_cancel.cancelled() => {
                settle_interrupted_call(
                    &mut protocol_call,
                    &control,
                    self.inner.policy.active_call_settle_timeout,
                    McpOutcomeUnknownReason::Shutdown,
                ).await
            }
            _ = cancellation.cancelled() => {
                settle_interrupted_call(
                    &mut protocol_call,
                    &control,
                    self.inner.policy.active_call_settle_timeout,
                    McpOutcomeUnknownReason::Cancelled,
                ).await
            }
            _ = control.cancellation.cancelled() => {
                let reason = control.cancellation_reason();
                settle_interrupted_call(
                    &mut protocol_call,
                    &control,
                    self.inner.policy.active_call_settle_timeout,
                    reason,
                ).await
            }
            _ = &mut deadline => {
                settle_interrupted_call(
                    &mut protocol_call,
                    &control,
                    self.inner.policy.active_call_settle_timeout,
                    McpOutcomeUnknownReason::TimedOut,
                ).await
            }
            result = &mut protocol_call => normalize_dispatched_result(&control, result),
        };
        let result = result.and_then(|result| {
            validate_tool_result_limits(&result, &self.inner.policy.security_limits)?;
            Ok(result)
        });
        control.set_state(invocation_state_for_result(&result));
        result
    }

    async fn stop_entry(
        &self,
        _server_id: McpServerId,
        entry: Arc<ManagedEntry>,
        allow_removed: bool,
    ) -> Result<McpServerStatus, McpError> {
        loop {
            let notified = entry.settled.notified();
            let reservation = {
                let mut state = lock_entry(&entry)?;
                if state.removed && !allow_removed {
                    return Err(McpError::config("MCP server is being removed"));
                }
                if state.status.state == McpServerState::Disabled
                    && state.peer.is_none()
                    && state.closing_peer.is_none()
                    && !state.connect_inflight
                    && !state.refresh_inflight
                    && state.active_calls.is_empty()
                {
                    return Ok(state.status.clone());
                }
                if state.status.state == McpServerState::Stopping {
                    None
                } else {
                    state.epoch = next_epoch(state.epoch)?;
                    let epoch = state.epoch;
                    let peer = state.peer.take();
                    state.closing_peer = peer.clone();
                    let cancel = state.watcher_cancel.take();
                    let watcher = state.watcher_task.take();
                    let active_calls = state.active_calls.values().cloned().collect::<Vec<_>>();
                    transition_locked(
                        &self.inner.events,
                        &entry,
                        &mut state,
                        McpServerState::Stopping,
                    );
                    Some((epoch, peer, cancel, watcher, active_calls))
                }
            };
            let Some((epoch, peer, cancel, watcher, active_calls)) = reservation else {
                notified.await;
                continue;
            };
            if let Some(cancel) = cancel {
                cancel.cancel();
            }
            cancel_active_calls(&active_calls, McpOutcomeUnknownReason::ServerStopped);
            let settled_before_close =
                settle_active_calls(&active_calls, self.inner.policy.active_call_settle_timeout)
                    .await;
            if !settled_before_close {
                if let Some(peer) = peer.as_ref() {
                    let _ = peer.force_close();
                }
            }
            let close_result = if let Some(peer) = peer.as_ref() {
                let result = peer.close().await;
                clear_closing_peer(&entry, peer);
                result
            } else {
                Ok(())
            };
            let settled_after_close =
                settle_active_calls(&active_calls, self.inner.policy.active_call_settle_timeout)
                    .await;
            if let Some(watcher) = watcher {
                let _ = watcher.await;
            }
            loop {
                let settled = entry.settled.notified();
                let inflight = {
                    let state = lock_entry(&entry)?;
                    state.connect_inflight || state.refresh_inflight
                };
                if !inflight {
                    break;
                }
                settled.await;
            }
            let mut state = lock_entry(&entry)?;
            if state.epoch != epoch {
                return Ok(state.status.clone());
            }
            if !settled_after_close || !state.active_calls.is_empty() {
                let error = McpError::shutdown(
                    "MCP active-call cleanup did not complete while stopping the server",
                );
                set_error_locked(&self.inner.events, &entry, &mut state, &error);
                return Err(error);
            }
            match close_result {
                Ok(()) => {
                    state.status.protocol = None;
                    state.status.notification_state = McpPeerNotificationState::Unknown;
                    state.status.last_error = None;
                    transition_locked(
                        &self.inner.events,
                        &entry,
                        &mut state,
                        McpServerState::Disabled,
                    );
                    return Ok(state.status.clone());
                }
                Err(error) => {
                    set_error_locked(&self.inner.events, &entry, &mut state, &error);
                    return Err(error);
                }
            }
        }
    }

    async fn finish_refresh(
        &self,
        server_id: McpServerId,
        entry: Arc<ManagedEntry>,
        epoch: u64,
        peer: Arc<dyn McpPeer>,
    ) -> Result<McpCatalogSnapshot, McpError> {
        let previous = {
            let state = lock_entry(&entry)?;
            if state.catalog.source_config_epoch == Some(state.status.config_epoch)
                && state.catalog.source_registry_revision == Some(state.status.registry_revision)
                && state.catalog.source_config_digest.as_ref() == Some(&state.status.config_digest)
            {
                state.catalog.clone()
            } else {
                let mut empty = McpCatalogSnapshot::empty(server_id);
                empty.generation = state.catalog.generation;
                empty
            }
        };
        let discovered = discover_catalog_with_limits(
            peer.as_ref(),
            server_id,
            Some(&previous),
            &self.inner.policy.catalog,
            &self.inner.policy.security_limits,
        )
        .await;
        let mut state = lock_entry(&entry)?;
        state.refresh_inflight = false;
        if state.epoch != epoch
            || state.removed
            || state
                .peer
                .as_ref()
                .is_none_or(|active| !Arc::ptr_eq(active, &peer))
        {
            let status = state.status.clone();
            entry.publish_status(&status);
            return Err(McpError::cancelled("MCP catalog refresh"));
        }
        let mut candidate = match discovered {
            Ok(candidate) => candidate,
            Err(error) => {
                let mut fallback = previous.clone();
                fallback.completeness = if fallback.content_digest.is_some() {
                    McpCatalogCompleteness::Stale(McpCatalogIssue::InvalidSchema)
                } else {
                    McpCatalogCompleteness::Failed(McpCatalogIssue::InvalidSchema)
                };
                state.status.last_error = Some(McpSafeError::from(&error));
                fallback
            }
        };
        candidate.source_config_epoch = Some(state.status.config_epoch);
        candidate.source_registry_revision = Some(state.status.registry_revision);
        candidate.source_config_digest = Some(state.status.config_digest.clone());
        let catalog_changed = candidate != state.catalog;
        state.catalog = candidate.clone();
        sync_catalog_status(&mut state.status, &candidate);
        let notification_unavailable =
            state.status.notification_state == McpPeerNotificationState::Unavailable;
        let next_state = if candidate.completeness == McpCatalogCompleteness::Complete
            && !notification_unavailable
        {
            state.status.last_error = None;
            McpServerState::Ready
        } else {
            if state.status.last_error.is_none() {
                state.status.last_error = Some(
                    if notification_unavailable
                        && candidate.completeness == McpCatalogCompleteness::Complete
                    {
                        notification_safe_error()
                    } else {
                        catalog_safe_error()
                    },
                );
            }
            McpServerState::Degraded
        };
        if catalog_changed {
            emit_catalog_locked(&self.inner.events, &mut state);
        }
        transition_locked(&self.inner.events, &entry, &mut state, next_state);
        let status = state.status.clone();
        entry.publish_status(&status);
        Ok(candidate)
    }

    async fn watch_peer_signals(
        &self,
        server_id: McpServerId,
        entry: Arc<ManagedEntry>,
        epoch: u64,
        mut signals: crate::McpPeerSignalReceiver,
        mut observed: crate::McpPeerSignalSnapshot,
        cancel: CancellationToken,
    ) {
        self.update_notification_state(&entry, epoch, observed.notification_state);
        if observed.transport_closed {
            self.handle_server_exit(server_id, entry, epoch, observed.exit_code)
                .await;
            return;
        }
        loop {
            let current = signals.snapshot();
            let snapshot = if current.sequence != observed.sequence {
                current
            } else {
                let changed = tokio::select! {
                    _ = cancel.cancelled() => return,
                    changed = signals.changed() => changed,
                };
                let Some(snapshot) = changed else {
                    return;
                };
                snapshot
            };
            if snapshot.transport_closed {
                self.handle_server_exit(server_id, Arc::clone(&entry), epoch, snapshot.exit_code)
                    .await;
                return;
            }
            self.update_notification_state(&entry, epoch, snapshot.notification_state);
            if snapshot.tools_revision <= observed.tools_revision {
                observed = snapshot;
                continue;
            }
            tokio::select! {
                _ = cancel.cancelled() => return,
                _ = tokio::time::sleep(self.inner.policy.notification_debounce) => {}
            }
            let latest = signals.snapshot();
            if latest.transport_closed {
                self.handle_server_exit(server_id, Arc::clone(&entry), epoch, latest.exit_code)
                    .await;
                return;
            }
            observed = latest;
            tokio::select! {
                _ = cancel.cancelled() => return,
                _ = self.refresh_for_epoch(server_id, Arc::clone(&entry), epoch) => {}
            }
        }
    }

    async fn refresh_for_epoch(
        &self,
        server_id: McpServerId,
        entry: Arc<ManagedEntry>,
        epoch: u64,
    ) -> Result<(), McpError> {
        loop {
            let notified = entry.settled.notified();
            let peer = {
                let mut state = lock_entry(&entry)?;
                if state.epoch != epoch || state.removed {
                    return Ok(());
                }
                if state.refresh_inflight {
                    None
                } else {
                    let Some(peer) = state.peer.as_ref().cloned() else {
                        return Ok(());
                    };
                    state.refresh_inflight = true;
                    transition_locked(
                        &self.inner.events,
                        &entry,
                        &mut state,
                        McpServerState::Discovering,
                    );
                    Some(peer)
                }
            };
            let Some(peer) = peer else {
                notified.await;
                continue;
            };
            let _inflight_guard = EntryInflightGuard {
                entry: Arc::clone(&entry),
            };
            let _ = self
                .finish_refresh(server_id, Arc::clone(&entry), epoch, peer)
                .await?;
            return Ok(());
        }
    }

    async fn handle_server_exit(
        &self,
        server_id: McpServerId,
        entry: Arc<ManagedEntry>,
        epoch: u64,
        exit_code: Option<i32>,
    ) {
        let (peer, active_calls) = {
            let Ok(mut state) = entry.state.lock() else {
                return;
            };
            if state.epoch != epoch
                || state.removed
                || matches!(
                    state.status.state,
                    McpServerState::Stopping | McpServerState::Disabled
                )
            {
                return;
            }
            state.event_sequence = state.event_sequence.saturating_add(1);
            let _ = self.inner.events.send(McpEvent::ServerExited {
                server_id,
                sequence: state.event_sequence,
                exit_code,
            });
            if state.catalog.content_digest.is_some() {
                state.catalog.completeness =
                    McpCatalogCompleteness::Stale(McpCatalogIssue::RequestFailed);
            } else {
                state.catalog.completeness =
                    McpCatalogCompleteness::Failed(McpCatalogIssue::RequestFailed);
            }
            sync_catalog_status_from_state(&mut state);
            emit_catalog_locked(&self.inner.events, &mut state);
            let error = McpError::server_exited(exit_code);
            set_error_locked(&self.inner.events, &entry, &mut state, &error);
            state.watcher_cancel.take();
            state.watcher_task.take();
            let peer = state.peer.take();
            state.closing_peer = peer.clone();
            let active_calls = state.active_calls.values().cloned().collect::<Vec<_>>();
            (peer, active_calls)
        };
        cancel_active_calls(&active_calls, McpOutcomeUnknownReason::ServerExited);
        let _ =
            settle_active_calls(&active_calls, self.inner.policy.active_call_settle_timeout).await;
        if let Some(peer) = peer {
            let _ = peer.close().await;
            clear_closing_peer(&entry, &peer);
        }
    }

    fn update_notification_state(
        &self,
        entry: &Arc<ManagedEntry>,
        epoch: u64,
        notification_state: McpPeerNotificationState,
    ) {
        let Ok(mut state) = entry.state.lock() else {
            return;
        };
        if state.epoch != epoch || state.removed {
            return;
        }
        state.status.notification_state = notification_state;
        if notification_state == McpPeerNotificationState::Unavailable
            && state.status.state == McpServerState::Ready
        {
            state.status.last_error = Some(notification_safe_error());
            transition_locked(
                &self.inner.events,
                entry,
                &mut state,
                McpServerState::Degraded,
            );
        } else {
            let status = state.status.clone();
            entry.publish_status(&status);
        }
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

    fn ensure_registry_watcher(&self) {
        if self
            .inner
            .registry_watcher_started
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }
        let weak = Arc::downgrade(&self.inner);
        let mut changes = self.inner.registry.subscribe();
        tokio::spawn(async move {
            loop {
                match changes.recv().await {
                    Ok(change) => {
                        emit_registry_change(&weak, &change);
                        reconcile_registry_change(&weak, change).await;
                    }
                    Err(McpRegistrySubscriptionError::Lagged { skipped }) => {
                        emit_registry_reconciliation_required(&weak, skipped);
                        reconcile_registry_snapshot(&weak).await
                    }
                    Err(McpRegistrySubscriptionError::Closed) => return,
                }
            }
        });
    }
}

fn emit_registry_change(inner: &Weak<ManagerInner>, change: &McpRegistryChange) {
    let Some(inner) = inner.upgrade() else {
        return;
    };
    let _ = inner.events.send(McpEvent::RegistryChanged {
        revision: change.revision,
        kind: change.kind,
        server_id: change.server_id,
        scope: change.scope.clone(),
        config_digest: change.config_digest.clone(),
        config_epoch: change.config_epoch,
    });
}

fn emit_registry_reconciliation_required(inner: &Weak<ManagerInner>, skipped_changes: u64) {
    let Some(inner) = inner.upgrade() else {
        return;
    };
    let _ = inner
        .events
        .send(McpEvent::RegistryReconciliationRequired { skipped_changes });
}

async fn reconcile_registry_change(inner: &Weak<ManagerInner>, change: McpRegistryChange) {
    let Some(inner) = inner.upgrade() else {
        return;
    };
    let manager = McpConnectionManager { inner };
    match manager.inner.registry.get(change.server_id) {
        Ok(Some(record))
            if record.revision == change.revision && record.config_epoch == change.config_epoch =>
        {
            reconcile_registry_record(&manager, record).await;
        }
        Ok(Some(_)) => {
            // A newer committed revision already supersedes this notification.
        }
        Ok(None) if change.kind == McpRegistryChangeKind::Removed => {
            stop_and_forget_removed_change(&manager, &change).await;
        }
        Ok(None) | Err(_) => {}
    }
}

async fn reconcile_registry_snapshot(inner: &Weak<ManagerInner>) {
    let Some(inner) = inner.upgrade() else {
        return;
    };
    let manager = McpConnectionManager { inner };
    let records = match manager.inner.registry.list() {
        Ok(records) => records,
        Err(_) => return,
    };
    let registered = records
        .iter()
        .map(|record| record.config.id)
        .collect::<BTreeSet<_>>();
    let managed_ids = manager
        .inner
        .entries
        .lock()
        .map(|entries| entries.keys().copied().collect::<Vec<_>>())
        .unwrap_or_default();
    for server_id in managed_ids {
        if !registered.contains(&server_id) {
            stop_and_forget_unregistered(&manager, server_id).await;
        }
    }
    for record in records {
        reconcile_registry_record(&manager, record).await;
    }
}

async fn reconcile_registry_record(manager: &McpConnectionManager, record: McpRegistryEntry) {
    let Ok(Some(entry)) = manager.get_entry(record.config.id) else {
        // Adding a registry entry alone never launches a process. The host must
        // call start/start_enabled, preserving install vs. connect separation.
        return;
    };
    let previous = {
        let Ok(mut state) = entry.state.lock() else {
            return;
        };
        let previous = state.status.clone();
        state.removed = false;
        apply_registry_locked(&manager.inner.events, &entry, &mut state, &record);
        let status = state.status.clone();
        entry.publish_status(&status);
        previous
    };

    if !record.config.enabled {
        if previous.state != McpServerState::Disabled {
            let _ = manager.stop(record.config.id).await;
        }
        return;
    }

    let config_changed = previous.config_epoch != record.config_epoch
        || previous.registry_revision != record.revision
        || previous.config_digest != record.config_digest;
    let active_or_failed = matches!(
        previous.state,
        McpServerState::Starting
            | McpServerState::Discovering
            | McpServerState::Ready
            | McpServerState::Degraded
            | McpServerState::Error
    );
    // An Added notification can race an explicit start that has already adopted
    // this exact Registry incarnation. Restart only when the managed entry was
    // actually bound to a different configuration identity.
    if config_changed && active_or_failed {
        let _ = manager.restart(record.config.id).await;
    }
}

async fn stop_and_forget_removed_change(
    manager: &McpConnectionManager,
    change: &McpRegistryChange,
) {
    let server_id = change.server_id;
    let Ok(Some(entry)) = manager.get_entry(server_id) else {
        return;
    };
    let should_stop = {
        let Ok(mut state) = entry.state.lock() else {
            return;
        };
        let identity_matches = state.status.server_id == server_id
            && state.status.config_epoch == change.config_epoch
            && state.status.config_digest == change.config_digest
            && state.status.registry_revision < change.revision;
        let should_stop =
            identity_matches && matches!(manager.inner.registry.get(server_id), Ok(None));
        if should_stop {
            state.removed = true;
        }
        should_stop
    };
    if should_stop {
        let _ = manager
            .stop_entry(server_id, Arc::clone(&entry), true)
            .await;
        manager.remove_managed_entry_if_same(server_id, &entry);
    }
}

async fn stop_and_forget_unregistered(manager: &McpConnectionManager, server_id: McpServerId) {
    let Ok(Some(entry)) = manager.get_entry(server_id) else {
        return;
    };
    let expected = {
        let Ok(state) = entry.state.lock() else {
            return;
        };
        (
            state.status.config_epoch,
            state.status.registry_revision,
            state.status.config_digest.clone(),
        )
    };
    let should_stop = {
        let Ok(mut state) = entry.state.lock() else {
            return;
        };
        let identity_matches = state.status.config_epoch == expected.0
            && state.status.registry_revision == expected.1
            && state.status.config_digest == expected.2;
        let should_stop =
            identity_matches && matches!(manager.inner.registry.get(server_id), Ok(None));
        if should_stop {
            state.removed = true;
        }
        should_stop
    };
    if should_stop {
        let _ = manager
            .stop_entry(server_id, Arc::clone(&entry), true)
            .await;
        manager.remove_managed_entry_if_same(server_id, &entry);
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

fn is_terminal_invocation_state(state: McpInvocationState) -> bool {
    matches!(
        state,
        McpInvocationState::Completed
            | McpInvocationState::Failed
            | McpInvocationState::Cancelled
            | McpInvocationState::OutcomeUnknown
    )
}

fn cancel_active_calls(calls: &[Arc<ActiveCallControl>], reason: McpOutcomeUnknownReason) {
    for call in calls {
        call.cancel(reason);
    }
}

async fn settle_active_calls(calls: &[Arc<ActiveCallControl>], timeout: Duration) -> bool {
    tokio::time::timeout(timeout, wait_for_active_call_removal(calls))
        .await
        .is_ok()
}

async fn wait_for_active_call_removal(calls: &[Arc<ActiveCallControl>]) {
    for call in calls {
        call.wait_terminal().await;
        call.wait_removed().await;
    }
}

fn invocation_state_for_result(result: &Result<McpToolResult, McpError>) -> McpInvocationState {
    match result {
        Ok(_) => McpInvocationState::Completed,
        Err(error) if error.kind == McpErrorKind::OutcomeUnknown => {
            McpInvocationState::OutcomeUnknown
        }
        Err(error) if error.kind == McpErrorKind::Cancelled => McpInvocationState::Cancelled,
        Err(_) => McpInvocationState::Failed,
    }
}

fn normalize_dispatched_result(
    control: &ActiveCallControl,
    result: Result<McpToolResult, McpError>,
) -> Result<McpToolResult, McpError> {
    match result {
        Ok(result) => {
            control.dispatch.mark_response_received();
            Ok(result)
        }
        Err(error) if error.kind == McpErrorKind::OutcomeUnknown => Err(error),
        Err(error) if error.dispatch_certainty == Some(McpDispatchCertainty::ResponseReceived) => {
            if control.dispatch.certainty() == McpDispatchCertainty::ResponseReceived {
                Err(error)
            } else {
                Err(McpError::outcome_unknown(
                    "MCP tools/call",
                    McpOutcomeUnknownReason::ProtocolFailure,
                    control.dispatch.certainty(),
                ))
            }
        }
        Err(error)
            if control.dispatch.certainty() != McpDispatchCertainty::DefinitelyNotDispatched =>
        {
            let reason = match error.kind {
                McpErrorKind::Cancelled => control.cancellation_reason(),
                McpErrorKind::Timeout => McpOutcomeUnknownReason::TimedOut,
                McpErrorKind::ServerExited => McpOutcomeUnknownReason::ServerExited,
                McpErrorKind::Shutdown => McpOutcomeUnknownReason::Shutdown,
                McpErrorKind::Protocol => McpOutcomeUnknownReason::ProtocolFailure,
                _ => McpOutcomeUnknownReason::TransportClosed,
            };
            Err(McpError::outcome_unknown(
                "MCP tools/call",
                reason,
                control.dispatch.certainty(),
            ))
        }
        Err(error) => Err(error),
    }
}

async fn settle_interrupted_call(
    call: &mut BoxMcpFuture<'_, McpToolResult>,
    control: &ActiveCallControl,
    grace: Duration,
    reason: McpOutcomeUnknownReason,
) -> Result<McpToolResult, McpError> {
    control.cancel(reason);
    match tokio::time::timeout(grace, call).await {
        Ok(result) => normalize_dispatched_result(control, result),
        Err(_) => Err(McpError::outcome_unknown(
            "MCP tools/call",
            reason,
            control.dispatch.certainty(),
        )),
    }
}

fn validate_tool_result_limits(
    result: &McpToolResult,
    limits: &McpSecurityLimits,
) -> Result<(), McpError> {
    if result.content.len() > limits.max_content_blocks {
        return Err(McpError::output_too_large(
            "MCP tools/call",
            "MCP tool result exceeded the configured content-block limit",
        ));
    }
    if let Some(structured) = &result.structured_content {
        if validate_structured_content(structured, limits).is_err() {
            return Err(McpError::output_too_large(
                "MCP tools/call",
                "MCP structured result exceeded the configured safety budget",
            ));
        }
    }
    let encoded = serde_json::to_vec(result).map_err(|_| {
        McpError::protocol("MCP tool result could not be measured safely")
            .with_dispatch_certainty(McpDispatchCertainty::ResponseReceived)
    })?;
    if encoded.len() > limits.max_raw_tool_result_bytes {
        return Err(McpError::output_too_large(
            "MCP tools/call",
            "MCP tool result exceeded the configured byte limit",
        ));
    }
    let mut total_media = 0_usize;
    for block in &result.content {
        let media_bytes = match block {
            crate::McpContentBlock::Image { data, .. }
            | crate::McpContentBlock::Audio { data, .. } => data.len(),
            crate::McpContentBlock::EmbeddedResource {
                resource: crate::McpEmbeddedResource::Blob { data, .. },
            } => data.len(),
            _ => 0,
        };
        if media_bytes > limits.max_encoded_media_bytes {
            return Err(McpError::output_too_large(
                "MCP tools/call",
                "MCP tool result contained an oversized encoded media block",
            ));
        }
        total_media = total_media.checked_add(media_bytes).ok_or_else(|| {
            McpError::output_too_large(
                "MCP tools/call",
                "MCP tool result media byte count overflowed",
            )
        })?;
    }
    if total_media > limits.max_total_encoded_media_bytes {
        return Err(McpError::output_too_large(
            "MCP tools/call",
            "MCP tool result exceeded the aggregate encoded media limit",
        ));
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::InMemoryMcpRegistry;
    use std::panic::{catch_unwind, AssertUnwindSafe};
    use std::path::PathBuf;

    struct RejectingConnector;

    impl McpConnector for RejectingConnector {
        fn connect<'a>(
            &'a self,
            _: &'a crate::McpServerConfig,
        ) -> BoxMcpFuture<'a, Arc<dyn McpPeer>> {
            Box::pin(async { Err(McpError::spawn("test connector does not start processes")) })
        }
    }

    fn test_manager(registry: Arc<dyn McpRegistry>) -> McpConnectionManager {
        McpConnectionManager::new(
            registry,
            Arc::new(RejectingConnector),
            Arc::new(NoopMcpEventSink),
            McpManagerPolicy::default(),
        )
        .unwrap()
    }

    fn test_config(server_id: McpServerId) -> crate::McpServerConfig {
        crate::McpServerConfig {
            id: server_id,
            display_name: "shutdown-test".to_string(),
            scope: McpServerScope::User,
            trust: McpTrustLevel::UserApproved,
            approval_mode: McpApprovalMode::Prompt,
            enabled: true,
            transport: crate::McpTransportConfig::Stdio(crate::McpStdioConfig {
                program: PathBuf::from("/not-executed"),
                arguments: Vec::new(),
                cwd: PathBuf::from("/"),
                environment: Vec::new(),
            }),
            connect_timeout_ms: 10_000,
            request_timeout_ms: 60_000,
            shutdown_timeout_ms: 2_000,
        }
    }

    #[tokio::test]
    async fn added_reconciliation_does_not_restart_the_same_registry_incarnation() {
        let registry = InMemoryMcpRegistry::shared();
        let server_id = McpServerId::new();
        let registered = registry.add(test_config(server_id)).unwrap();
        let manager = test_manager(registry);
        let entry = Arc::new(ManagedEntry::new(&registered));
        {
            let mut state = entry.state.lock().unwrap();
            state.status.state = McpServerState::Error;
        }
        manager
            .inner
            .entries
            .lock()
            .unwrap()
            .insert(server_id, Arc::clone(&entry));

        reconcile_registry_record(&manager, registered).await;

        let state = entry.state.lock().unwrap();
        assert_eq!(state.epoch, 0);
        assert_eq!(state.status.state, McpServerState::Error);
        assert!(!state.connect_inflight);
        assert!(!state.refresh_inflight);
    }

    #[tokio::test]
    async fn stale_removed_change_cannot_stop_a_readded_registry_incarnation() {
        let registry = InMemoryMcpRegistry::shared();
        let server_id = McpServerId::new();
        let config = test_config(server_id);
        let first = registry.add(config.clone()).unwrap();
        let first_removed = registry.remove(server_id).unwrap().unwrap();
        let readded = registry.add(config).unwrap();
        let readded_removed = registry.remove(server_id).unwrap().unwrap();
        assert_eq!(first_removed.config_epoch, first.config_epoch);
        assert_ne!(readded.config_epoch, first.config_epoch);

        let manager = test_manager(registry);
        let entry = Arc::new(ManagedEntry::new(&readded));
        manager
            .inner
            .entries
            .lock()
            .unwrap()
            .insert(server_id, Arc::clone(&entry));
        let stale_change = McpRegistryChange {
            revision: first_removed.revision,
            kind: McpRegistryChangeKind::Removed,
            server_id,
            scope: first_removed.config.scope,
            enabled: first_removed.config.enabled,
            config_digest: first_removed.config_digest,
            config_epoch: first_removed.config_epoch,
        };

        stop_and_forget_removed_change(&manager, &stale_change).await;

        assert!(manager
            .get_entry(server_id)
            .unwrap()
            .is_some_and(|current| Arc::ptr_eq(&current, &entry)));
        assert!(!entry.state.lock().unwrap().removed);

        let current_change = McpRegistryChange {
            revision: readded_removed.revision,
            kind: McpRegistryChangeKind::Removed,
            server_id,
            scope: readded_removed.config.scope,
            enabled: readded_removed.config.enabled,
            config_digest: readded_removed.config_digest,
            config_epoch: readded_removed.config_epoch,
        };
        stop_and_forget_removed_change(&manager, &current_change).await;
        assert!(manager.get_entry(server_id).unwrap().is_none());
        assert!(entry.state.lock().unwrap().removed);
    }

    #[tokio::test]
    async fn active_call_settlement_reports_timeout_until_terminal_removal() {
        let server_id = McpServerId::new();
        let control = Arc::new(ActiveCallControl::new(
            McpActiveCallId::new(
                server_id,
                McpInvocationId::new(),
                McpModelCallId::new("settlement-boundary").unwrap(),
            ),
            McpActiveCallProvenance {
                tool_id: McpToolId {
                    server_id,
                    raw_name: "slow".to_string(),
                },
                model_name: "mcp__fixture__slow".to_string(),
                config_epoch: McpConfigEpoch::new(),
                registry_revision: 1,
                config_digest: "a".repeat(64).parse().unwrap(),
                catalog_generation: 1,
                catalog_digest: "b".repeat(64).parse().unwrap(),
                schema_digest: "c".repeat(64).parse().unwrap(),
            },
            60_000,
        ));
        assert!(
            !settle_active_calls(std::slice::from_ref(&control), Duration::from_millis(1),).await
        );

        control.set_state(McpInvocationState::OutcomeUnknown);
        assert!(
            !settle_active_calls(std::slice::from_ref(&control), Duration::from_millis(1),).await,
            "a terminal state alone is not cleanup until the active registry guard is removed"
        );
        control.removed.store(true, Ordering::Release);
        control.settled.notify_waiters();
        assert!(
            settle_active_calls(std::slice::from_ref(&control), Duration::from_millis(50),).await
        );
    }

    #[tokio::test]
    async fn forced_shutdown_recovers_poisoned_entry_map_but_reports_incomplete_cleanup() {
        let registry = InMemoryMcpRegistry::shared();
        let server_id = McpServerId::new();
        let registered = registry.add(test_config(server_id)).unwrap();
        let manager = test_manager(registry);
        let entry = Arc::new(ManagedEntry::new(&registered));
        manager
            .inner
            .entries
            .lock()
            .unwrap()
            .insert(server_id, Arc::clone(&entry));

        let inner = Arc::clone(&manager.inner);
        assert!(catch_unwind(AssertUnwindSafe(move || {
            let _entries = inner.entries.lock().unwrap();
            panic!("poison manager entry map for deterministic cleanup test");
        }))
        .is_err());

        let (results, cleanup_complete) = manager
            .force_shutdown_entries(Duration::from_millis(50))
            .await;
        assert!(!cleanup_complete);
        assert_eq!(results.len(), 1);
        let state = entry.state.lock().unwrap();
        assert!(state.removed);
        assert_eq!(state.status.state, McpServerState::Disabled);
        assert!(state.active_calls.is_empty());
    }

    #[tokio::test]
    async fn forced_shutdown_recovers_poisoned_entry_state_but_never_claims_completion() {
        let registry = InMemoryMcpRegistry::shared();
        let server_id = McpServerId::new();
        let registered = registry.add(test_config(server_id)).unwrap();
        let manager = test_manager(registry);
        let entry = Arc::new(ManagedEntry::new(&registered));
        manager
            .inner
            .entries
            .lock()
            .unwrap()
            .insert(server_id, Arc::clone(&entry));

        let poisoned_entry = Arc::clone(&entry);
        assert!(catch_unwind(AssertUnwindSafe(move || {
            let _state = poisoned_entry.state.lock().unwrap();
            panic!("poison managed entry state for deterministic cleanup test");
        }))
        .is_err());

        let (results, cleanup_complete) = manager
            .force_shutdown_entries(Duration::from_millis(50))
            .await;
        assert!(!cleanup_complete);
        assert_eq!(results.len(), 1);
        let state = match entry.state.lock() {
            Ok(_) => panic!("managed entry state should remain poisoned"),
            Err(poisoned) => poisoned.into_inner(),
        };
        assert!(state.removed);
        assert_eq!(state.status.state, McpServerState::Disabled);
        assert!(state.active_calls.is_empty());
    }

    #[tokio::test]
    async fn graceful_shutdown_completion_requires_successful_results_and_clean_entry_state() {
        let registry = InMemoryMcpRegistry::shared();
        let server_id = McpServerId::new();
        let registered = registry.add(test_config(server_id)).unwrap();
        let manager = test_manager(registry);
        let entry = Arc::new(ManagedEntry::new(&registered));
        manager
            .inner
            .entries
            .lock()
            .unwrap()
            .insert(server_id, Arc::clone(&entry));
        let disabled = entry.state.lock().unwrap().status.clone();
        let success = McpBatchOperationResult {
            server_id: Some(server_id),
            status: Some(disabled.clone()),
            error: None,
        };
        assert!(manager.graceful_shutdown_cleanup_complete(std::slice::from_ref(&success)));

        let close_error = McpError::shutdown("fixture peer close failed");
        let failed = McpBatchOperationResult {
            server_id: Some(server_id),
            status: None,
            error: Some(McpSafeError::from(&close_error)),
        };
        assert!(!manager.graceful_shutdown_cleanup_complete(&[failed]));

        entry.state.lock().unwrap().status.state = McpServerState::Error;
        assert!(!manager.graceful_shutdown_cleanup_complete(&[success]));
    }

    #[test]
    fn every_error_after_request_queue_is_outcome_unknown_without_response_evidence() {
        let server_id = McpServerId::new();
        let control = ActiveCallControl::new(
            McpActiveCallId::new(
                server_id,
                McpInvocationId::new(),
                McpModelCallId::new("post-dispatch-errors").unwrap(),
            ),
            McpActiveCallProvenance {
                tool_id: McpToolId {
                    server_id,
                    raw_name: "mutating_tool".to_string(),
                },
                model_name: "mcp__fixture__mutating_tool".to_string(),
                config_epoch: McpConfigEpoch::new(),
                registry_revision: 1,
                config_digest: "a".repeat(64).parse().unwrap(),
                catalog_generation: 1,
                catalog_digest: "b".repeat(64).parse().unwrap(),
                schema_digest: "c".repeat(64).parse().unwrap(),
            },
            60_000,
        );
        control.dispatch.mark_request_queued();

        for error in [
            McpError::config("post-dispatch config failure"),
            McpError::spawn("post-dispatch spawn failure"),
            McpError::negotiation("post-dispatch negotiation failure"),
            McpError::protocol("post-dispatch protocol failure"),
            McpError::cancelled("MCP tools/call"),
            McpError::timeout("MCP tools/call", 1),
            McpError::server_exited(None),
            McpError::shutdown("post-dispatch shutdown"),
        ] {
            let normalized = normalize_dispatched_result(&control, Err(error)).unwrap_err();
            assert_eq!(normalized.kind, McpErrorKind::OutcomeUnknown);
            assert_eq!(
                normalized.dispatch_certainty,
                Some(McpDispatchCertainty::PossiblyDispatched)
            );
        }
    }
}
