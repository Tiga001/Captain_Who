//! Per-server connection lifecycle and explicit registry removal operations.

use super::*;

impl McpConnectionManager {
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

    pub(super) async fn start_reserved_until_shutdown(
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
        if self.is_draining() {
            return Err(McpError::shutdown(
                "MCP connection manager is stopping all servers",
            ));
        }
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
                    let old_lifecycle_cancel = state.lifecycle_cancel.take();
                    let peers = take_cleanup_peers(&mut state);
                    let lifecycle_cancel = CancellationToken::new();
                    state.lifecycle_cancel = Some(lifecycle_cancel.clone());
                    let old_cancel = state.watcher_cancel.take();
                    let old_watcher = state.watcher_task.take();
                    let active_calls = state.active_calls.values().cloned().collect::<Vec<_>>();
                    transition_locked(
                        &self.inner.events,
                        &entry,
                        &mut state,
                        McpServerState::Starting,
                    );
                    Some((
                        epoch,
                        lifecycle_cancel,
                        old_lifecycle_cancel,
                        peers,
                        old_cancel,
                        old_watcher,
                        active_calls,
                    ))
                }
            };
            let Some((
                epoch,
                lifecycle_cancel,
                old_lifecycle_cancel,
                peers,
                old_cancel,
                old_watcher,
                active_calls,
            )) = reservation
            else {
                notified.await;
                continue;
            };
            let inflight_guard = EntryInflightGuard {
                entry: Arc::clone(&entry),
                kind: EntryInflightKind::Connect,
            };

            if let Some(cancel) = old_lifecycle_cancel {
                cancel.cancel();
            }
            if let Some(cancel) = old_cancel {
                cancel.cancel();
            }
            let cleanup_deadline =
                tokio::time::Instant::now() + self.inner.policy.lifecycle_cleanup_timeout;
            if let Err(error) = self
                .cleanup_connection_parts(
                    &entry,
                    peers,
                    old_watcher,
                    active_calls,
                    McpOutcomeUnknownReason::ServerRestarted,
                    cleanup_deadline,
                )
                .await
            {
                lifecycle_cancel.cancel();
                let mut state = lock_entry(&entry)?;
                if state.epoch == epoch {
                    state.lifecycle_cancel.take();
                    set_error_locked(&self.inner.events, &entry, &mut state, &error);
                }
                return Err(error);
            }

            let current_registry = match self.inner.registry.get(server_id) {
                Ok(current) => current,
                Err(error) => {
                    self.fail_start_if_owned(&entry, epoch, &error)?;
                    return Err(error);
                }
            };
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
                let Some(current) = current_registry else {
                    self.remove_managed_entry_if_same(server_id, &entry);
                    return Err(McpError::config("MCP server is no longer registered"));
                };
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

            let connect_timeout = registry.config.connect_timeout();
            let connected = tokio::select! {
                biased;
                _ = lifecycle_cancel.cancelled() => {
                    Err(McpError::cancelled("MCP server start"))
                }
                connected = tokio::time::timeout(
                    connect_timeout,
                    self.inner.connector.connect(&registry.config),
                ) => {
                    connected.unwrap_or_else(|_| {
                        Err(McpError::timeout(
                            "MCP server connection",
                            u64::try_from(connect_timeout.as_millis()).unwrap_or(u64::MAX),
                        ))
                    })
                }
            };
            let peer = match connected {
                Ok(peer) => peer,
                Err(error) => {
                    let current_registry = match self.inner.registry.get(server_id) {
                        Ok(current) => current,
                        Err(registry_error) => {
                            self.fail_start_if_owned(&entry, epoch, &registry_error)?;
                            return Err(registry_error);
                        }
                    };
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
                        let Some(current) = current_registry else {
                            self.remove_managed_entry_if_same(server_id, &entry);
                            return Err(McpError::config("MCP server is no longer registered"));
                        };
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
                        state.lifecycle_cancel.take();
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
                let error = self
                    .close_uncommitted_peer(&entry, Arc::clone(&peer))
                    .await
                    .err()
                    .unwrap_or(error);
                let mut state = lock_entry(&entry)?;
                state.connect_inflight = false;
                if state.epoch == epoch && state.status.state == McpServerState::Starting {
                    state.lifecycle_cancel.take();
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
                let error = self
                    .close_uncommitted_peer(&entry, Arc::clone(&peer))
                    .await
                    .err()
                    .unwrap_or(error);
                let mut state = lock_entry(&entry)?;
                state.connect_inflight = false;
                if state.epoch == epoch && state.status.state == McpServerState::Starting {
                    state.lifecycle_cancel.take();
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
                let close_result = self.close_uncommitted_peer(&entry, Arc::clone(&peer)).await;
                if let Err(error) = close_result {
                    self.fail_start_if_owned(&entry, epoch, &error)?;
                    return Err(error);
                }
                let mut state = lock_entry(&entry)?;
                state.connect_inflight = false;
                let status = state.status.clone();
                entry.publish_status(&status);
                return Err(McpError::cancelled("MCP server start"));
            }

            let current_registry = match self.inner.registry.get(server_id) {
                Ok(current) => current,
                Err(error) => {
                    let close_error = self
                        .close_uncommitted_peer(&entry, Arc::clone(&peer))
                        .await
                        .err();
                    let error = close_error.unwrap_or(error);
                    self.fail_start_if_owned(&entry, epoch, &error)?;
                    return Err(error);
                }
            };
            if current_registry.as_ref().is_none_or(|current| {
                current.revision != registry.revision
                    || current.config_digest != registry.config_digest
                    || !current.config.enabled
            }) {
                if let Err(error) = self.close_uncommitted_peer(&entry, Arc::clone(&peer)).await {
                    self.fail_start_if_owned(&entry, epoch, &error)?;
                    return Err(error);
                }
                let owns_reservation = self.abandon_start_for_registry_change(
                    &entry,
                    epoch,
                    current_registry.as_ref(),
                )?;
                let Some(current) = current_registry else {
                    self.remove_managed_entry_if_same(server_id, &entry);
                    return Err(McpError::config("MCP server is no longer registered"));
                };
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

            let signal_subscription = peer.subscribe_signals().map(|signals| {
                let snapshot = signals.snapshot();
                (signals, snapshot)
            });
            let notification_state = signal_subscription
                .as_ref()
                .map(|(_, snapshot)| snapshot.notification_state)
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
                let close_result = self.close_uncommitted_peer(&entry, Arc::clone(&peer)).await;
                if let Err(error) = close_result {
                    self.fail_start_if_owned(&entry, epoch, &error)?;
                    return Err(error);
                }
                let mut state = lock_entry(&entry)?;
                state.connect_inflight = false;
                let status = state.status.clone();
                entry.publish_status(&status);
                return Err(McpError::cancelled("MCP server start"));
            }

            if let Some((signals, initial_signal_snapshot)) = signal_subscription {
                let watcher_manager = self.clone();
                let watcher_entry = Arc::clone(&entry);
                let watcher = tokio::spawn(async move {
                    watcher_manager
                        .watch_peer_signals(
                            server_id,
                            watcher_entry,
                            epoch,
                            signals,
                            initial_signal_snapshot,
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

            drop(inflight_guard);
            let _refresh_guard = EntryInflightGuard {
                entry: Arc::clone(&entry),
                kind: EntryInflightKind::Refresh,
            };
            let _ = tokio::select! {
                biased;
                _ = lifecycle_cancel.cancelled() => {
                    Err(McpError::cancelled("MCP catalog refresh"))
                }
                result = self.finish_refresh(server_id, Arc::clone(&entry), epoch, peer) => result,
            }?;
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

    pub(super) async fn stop_owned(
        &self,
        server_id: McpServerId,
    ) -> Result<McpServerStatus, McpError> {
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
                result = manager.restart_owned(server_id) => result,
            }
        }))
        .await
    }

    async fn restart_owned(&self, server_id: McpServerId) -> Result<McpServerStatus, McpError> {
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
        loop {
            let notified = entry.settled.notified();
            let owns_restart = {
                let mut state = lock_entry(&entry)?;
                if state.removed {
                    return Err(McpError::config("MCP server is being removed"));
                }
                if state.restart_inflight {
                    false
                } else {
                    state.restart_inflight = true;
                    true
                }
            };
            if owns_restart {
                break;
            }
            notified.await;
            let state = lock_entry(&entry)?;
            if state.restart_inflight {
                continue;
            }
            return if state.status.state == McpServerState::Error {
                Err(McpError::shutdown(
                    "The concurrent MCP server restart did not complete successfully",
                ))
            } else {
                Ok(state.status.clone())
            };
        }
        let _restart_guard = EntryRestartGuard {
            entry: Arc::clone(&entry),
        };
        self.stop_entry(server_id, entry, false).await?;
        let _permit = self.reserve_start()?;
        self.start_reserved_until_shutdown(server_id).await
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

    pub(super) async fn stop_entry(
        &self,
        _server_id: McpServerId,
        entry: Arc<ManagedEntry>,
        allow_removed: bool,
    ) -> Result<McpServerStatus, McpError> {
        let deadline = tokio::time::Instant::now() + self.inner.policy.lifecycle_cleanup_timeout;
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
                    && state.lifecycle_cancel.is_none()
                    && state.active_calls.is_empty()
                {
                    return Ok(state.status.clone());
                }
                if state.status.state == McpServerState::Stopping {
                    None
                } else {
                    state.epoch = next_epoch(state.epoch)?;
                    let epoch = state.epoch;
                    let lifecycle_cancel = state.lifecycle_cancel.take();
                    let peers = take_cleanup_peers(&mut state);
                    let watcher_cancel = state.watcher_cancel.take();
                    let watcher = state.watcher_task.take();
                    let active_calls = state.active_calls.values().cloned().collect::<Vec<_>>();
                    transition_locked(
                        &self.inner.events,
                        &entry,
                        &mut state,
                        McpServerState::Stopping,
                    );
                    Some((
                        epoch,
                        lifecycle_cancel,
                        peers,
                        watcher_cancel,
                        watcher,
                        active_calls,
                    ))
                }
            };
            let Some((epoch, lifecycle_cancel, peers, watcher_cancel, watcher, active_calls)) =
                reservation
            else {
                tokio::time::timeout_at(deadline, notified)
                    .await
                    .map_err(|_| {
                        McpError::shutdown(
                            "MCP concurrent stop did not settle within the Host lifecycle deadline",
                        )
                    })?;
                continue;
            };
            if let Some(cancel) = lifecycle_cancel {
                cancel.cancel();
            }
            if let Some(cancel) = watcher_cancel {
                cancel.cancel();
            }
            let cleanup_result = self
                .cleanup_connection_parts(
                    &entry,
                    peers,
                    watcher,
                    active_calls,
                    McpOutcomeUnknownReason::ServerStopped,
                    deadline,
                )
                .await;
            loop {
                let settled = entry.settled.notified();
                let inflight = {
                    let state = lock_entry(&entry)?;
                    state.connect_inflight || state.refresh_inflight
                };
                if !inflight {
                    break;
                }
                if tokio::time::timeout_at(deadline, settled).await.is_err() {
                    let error = McpError::shutdown(
                        "MCP in-flight lifecycle operation did not cancel before the stop deadline",
                    );
                    let mut state = lock_entry(&entry)?;
                    if state.epoch == epoch {
                        set_error_locked(&self.inner.events, &entry, &mut state, &error);
                    }
                    return Err(error);
                }
            }
            let mut state = lock_entry(&entry)?;
            if state.epoch != epoch {
                return Ok(state.status.clone());
            }
            if let Err(error) = cleanup_result {
                set_error_locked(&self.inner.events, &entry, &mut state, &error);
                return Err(error);
            }
            if state.closing_peer.is_some() || !state.active_calls.is_empty() {
                let error = McpError::shutdown(
                    "MCP cleanup remained incomplete after the Host lifecycle deadline",
                );
                set_error_locked(&self.inner.events, &entry, &mut state, &error);
                return Err(error);
            }
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
    }
}
