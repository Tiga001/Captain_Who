use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex, Weak};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, watch, Notify};
use tokio::task::{JoinHandle, JoinSet};
use tokio_util::sync::CancellationToken;

use crate::catalog::discover_catalog;
use crate::{
    McpCancellationToken, McpCatalogCompleteness, McpCatalogIssue, McpCatalogPolicy,
    McpCatalogSnapshot, McpCatalogToolCall, McpConfigDigest, McpConnector, McpError, McpEvent,
    McpEventSink, McpPeer, McpPeerNotificationState, McpProtocolSnapshot, McpRegistry,
    McpRegistryChange, McpRegistryChangeKind, McpRegistryEntry, McpRegistrySubscriptionError,
    McpSafeError, McpServerId, McpServerScope, McpServerState, McpToolCall, McpToolId,
    McpToolResult, McpTrustLevel, NoopMcpEventSink,
};

const FORCE_SHUTDOWN_GRACE_MAX: Duration = Duration::from_millis(250);

#[derive(Clone, Debug)]
pub struct McpManagerPolicy {
    pub catalog: McpCatalogPolicy,
    pub notification_debounce: Duration,
}

impl Default for McpManagerPolicy {
    fn default() -> Self {
        Self {
            catalog: McpCatalogPolicy::default(),
            notification_debounce: Duration::from_millis(100),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerStatus {
    pub server_id: McpServerId,
    pub state: McpServerState,
    pub enabled: bool,
    pub scope: McpServerScope,
    pub trust: McpTrustLevel,
    pub config_digest: McpConfigDigest,
    pub protocol: Option<McpProtocolSnapshot>,
    pub notification_state: McpPeerNotificationState,
    pub catalog_generation: u64,
    pub catalog_completeness: McpCatalogCompleteness,
    pub tool_count: usize,
    pub last_error: Option<McpSafeError>,
}

impl McpServerStatus {
    fn from_registry(entry: &McpRegistryEntry) -> Self {
        Self {
            server_id: entry.config.id,
            state: McpServerState::Disabled,
            enabled: entry.config.enabled,
            scope: entry.config.scope.clone(),
            trust: entry.config.trust,
            config_digest: entry.config_digest.clone(),
            protocol: None,
            notification_state: McpPeerNotificationState::Unknown,
            catalog_generation: 0,
            catalog_completeness: McpCatalogCompleteness::Failed(McpCatalogIssue::RequestFailed),
            tool_count: 0,
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
    removed: bool,
}

struct ManagedEntry {
    state: StdMutex<ManagedEntryState>,
    status: watch::Sender<McpServerStatus>,
    settled: Notify,
}

impl ManagedEntry {
    fn new(registry: &McpRegistryEntry) -> Self {
        let status = McpServerStatus::from_registry(registry);
        let mut catalog = McpCatalogSnapshot::empty(registry.config.id);
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
        if policy.notification_debounce > Duration::from_secs(60) {
            return Err(McpError::config(
                "MCP notification debounce must not exceed 60 seconds",
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
                let same_config = state.status.config_digest == registry.config_digest;
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
                    transition_locked(
                        &self.inner.events,
                        &entry,
                        &mut state,
                        McpServerState::Starting,
                    );
                    Some((epoch, old_peer, old_cancel, old_watcher))
                }
            };
            let Some((epoch, old_peer, old_cancel, old_watcher)) = reservation else {
                notified.await;
                continue;
            };
            let _inflight_guard = EntryInflightGuard {
                entry: Arc::clone(&entry),
            };

            if let Some(cancel) = old_cancel {
                cancel.cancel();
            }
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
                return McpShutdownReport {
                    results,
                    forced: false,
                    cleanup_complete: true,
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

    async fn force_shutdown_entries(
        &self,
        force_grace: Duration,
    ) -> (Vec<McpBatchOperationResult>, bool) {
        let entries = self
            .inner
            .entries
            .lock()
            .map(|entries| {
                entries
                    .iter()
                    .map(|(server_id, entry)| (*server_id, Arc::clone(entry)))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let mut tasks = JoinSet::new();
        let mut results = Vec::with_capacity(entries.len());
        for (server_id, entry) in entries {
            let (peers, cancel, watcher, status) = {
                let Ok(mut state) = entry.state.lock() else {
                    continue;
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
                let generation = state.catalog.generation;
                state.catalog = McpCatalogSnapshot::empty(server_id);
                state.catalog.generation = generation;
                sync_catalog_status_from_state(&mut state);
                state.status.protocol = None;
                state.status.notification_state = McpPeerNotificationState::Unknown;
                state.status.last_error = None;
                transition_locked(
                    &self.inner.events,
                    &entry,
                    &mut state,
                    McpServerState::Disabled,
                );
                (peers, cancel, watcher, state.status.clone())
            };
            if let Some(cancel) = cancel {
                cancel.cancel();
            }
            if let Some(watcher) = watcher {
                watcher.abort();
                tasks.spawn(async move {
                    let _ = watcher.await;
                });
            }
            for peer in peers {
                let _ = peer.force_close();
                tasks.spawn(async move {
                    let _ = peer.close().await;
                });
            }
            results.push(McpBatchOperationResult {
                server_id: Some(server_id),
                status: Some(status),
                error: None,
            });
        }

        let settled = tokio::time::timeout(force_grace, async {
            while tasks.join_next().await.is_some() {}
        })
        .await
        .is_ok();
        if !settled {
            tasks.abort_all();
            while tasks.join_next().await.is_some() {}
        }
        results.sort_by_key(|result| result.server_id);
        (results, settled)
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
        let server_id = request.tool_id.server_id;
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
        let entry = self
            .get_entry(server_id)?
            .ok_or_else(|| McpError::config("MCP catalog invocation server is not ready"))?;

        let (peer, call, timeout_ms) = {
            let state = lock_entry(&entry)?;
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
            if state.status.config_digest != registry.config_digest
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
            {
                return Err(McpError::config(
                    "MCP catalog invocation tool route is stale",
                ));
            }
            let timeout_ms = request
                .timeout_ms
                .filter(|timeout_ms| *timeout_ms > 0)
                .map(|timeout_ms| timeout_ms.min(registry.config.request_timeout_ms))
                .unwrap_or(registry.config.request_timeout_ms);
            let call = McpToolCall {
                name: request.tool_id.raw_name.clone(),
                arguments: request.arguments,
                timeout_ms: Some(timeout_ms),
            };
            (peer, call, timeout_ms)
        };

        let settle_cancellation = cancellation.clone();
        let protocol_call = peer.call_tool(call, cancellation);
        tokio::pin!(protocol_call);
        let deadline = tokio::time::sleep(Duration::from_millis(timeout_ms));
        tokio::pin!(deadline);
        tokio::select! {
            biased;
            _ = self.inner.shutdown_cancel.cancelled() => {
                settle_cancellation.cancel();
                let _ = tokio::time::timeout(Duration::from_millis(250), &mut protocol_call).await;
                Err(McpError::shutdown("MCP connection manager is shutting down"))
            }
            _ = &mut deadline => {
                settle_cancellation.cancel();
                let _ = tokio::time::timeout(Duration::from_millis(250), &mut protocol_call).await;
                Err(McpError::timeout("MCP tools/call", timeout_ms))
            }
            result = &mut protocol_call => result,
        }
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
                    transition_locked(
                        &self.inner.events,
                        &entry,
                        &mut state,
                        McpServerState::Stopping,
                    );
                    Some((epoch, peer, cancel, watcher))
                }
            };
            let Some((epoch, peer, cancel, watcher)) = reservation else {
                notified.await;
                continue;
            };
            if let Some(cancel) = cancel {
                cancel.cancel();
            }
            let close_result = if let Some(peer) = peer.as_ref() {
                let result = peer.close().await;
                clear_closing_peer(&entry, peer);
                result
            } else {
                Ok(())
            };
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
            if state.catalog.source_config_digest.as_ref() == Some(&state.status.config_digest) {
                state.catalog.clone()
            } else {
                let mut empty = McpCatalogSnapshot::empty(server_id);
                empty.generation = state.catalog.generation;
                empty
            }
        };
        let discovered = discover_catalog(
            peer.as_ref(),
            server_id,
            Some(&previous),
            &self.inner.policy.catalog,
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
        let peer = {
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
            peer
        };
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
                    Ok(change) => reconcile_registry_change(&weak, change).await,
                    Err(McpRegistrySubscriptionError::Lagged { .. }) => {
                        reconcile_registry_snapshot(&weak).await
                    }
                    Err(McpRegistrySubscriptionError::Closed) => return,
                }
            }
        });
    }
}

async fn reconcile_registry_change(inner: &Weak<ManagerInner>, change: McpRegistryChange) {
    let Some(inner) = inner.upgrade() else {
        return;
    };
    let manager = McpConnectionManager { inner };
    match manager.inner.registry.get(change.server_id) {
        Ok(Some(record)) if record.revision == change.revision => {
            reconcile_registry_record(&manager, record, change.kind).await;
        }
        Ok(Some(_)) => {
            // A newer committed revision already supersedes this notification.
        }
        Ok(None) if change.kind == McpRegistryChangeKind::Removed => {
            stop_and_forget_removed(&manager, change.server_id).await;
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
            stop_and_forget_removed(&manager, server_id).await;
        }
    }
    for record in records {
        reconcile_registry_record(&manager, record, McpRegistryChangeKind::Updated).await;
    }
}

async fn reconcile_registry_record(
    manager: &McpConnectionManager,
    record: McpRegistryEntry,
    change_kind: McpRegistryChangeKind,
) {
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

    let config_changed = previous.config_digest != record.config_digest;
    let active_or_failed = matches!(
        previous.state,
        McpServerState::Starting
            | McpServerState::Discovering
            | McpServerState::Ready
            | McpServerState::Degraded
            | McpServerState::Error
    );
    if (change_kind == McpRegistryChangeKind::Added || config_changed) && active_or_failed {
        let _ = manager.restart(record.config.id).await;
    }
}

async fn stop_and_forget_removed(manager: &McpConnectionManager, server_id: McpServerId) {
    if let Ok(Some(entry)) = manager.get_entry(server_id) {
        if let Ok(mut state) = entry.state.lock() {
            state.removed = true;
        }
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
    if state.catalog.source_config_digest.as_ref() != Some(&registry.config_digest) {
        let mut invalidated = McpCatalogSnapshot::empty(registry.config.id);
        invalidated.generation = state.catalog.generation;
        invalidated.source_config_digest = Some(registry.config_digest.clone());
        let catalog_changed = invalidated != state.catalog;
        state.catalog = invalidated;
        sync_catalog_status_from_state(state);
        if catalog_changed {
            emit_catalog_locked(events, state);
        }
    }
    state.status.enabled = registry.config.enabled;
    state.status.scope = registry.config.scope.clone();
    state.status.trust = registry.config.trust;
    state.status.config_digest = registry.config_digest.clone();
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
