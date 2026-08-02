//! Catalog discovery, refresh coordination, and peer notification handling.

use super::*;

impl McpConnectionManager {
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
                    let lifecycle_cancel = state
                        .lifecycle_cancel
                        .as_ref()
                        .cloned()
                        .ok_or_else(|| McpError::cancelled("MCP catalog refresh"))?;
                    state.refresh_inflight = true;
                    let epoch = state.epoch;
                    transition_locked(
                        &self.inner.events,
                        &entry,
                        &mut state,
                        McpServerState::Discovering,
                    );
                    Some((epoch, peer, lifecycle_cancel))
                }
            };
            let Some((epoch, peer, lifecycle_cancel)) = reserved else {
                notified.await;
                continue;
            };
            let _inflight_guard = EntryInflightGuard {
                entry: Arc::clone(&entry),
                kind: EntryInflightKind::Refresh,
            };
            return tokio::select! {
                biased;
                _ = lifecycle_cancel.cancelled() => {
                    Err(McpError::cancelled("MCP catalog refresh"))
                }
                result = self.finish_refresh(server_id, Arc::clone(&entry), epoch, peer) => result,
            };
        }
    }

    pub(super) async fn finish_refresh(
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
        let catalog_refresh_timeout = self.inner.policy.catalog_refresh_timeout;
        let discovered = tokio::time::timeout(
            catalog_refresh_timeout,
            discover_catalog_with_limits(
                peer.as_ref(),
                server_id,
                Some(&previous),
                &self.inner.policy.catalog,
                &self.inner.policy.security_limits,
            ),
        )
        .await
        .unwrap_or_else(|_| {
            Err(McpError::timeout(
                "MCP Catalog refresh",
                u64::try_from(catalog_refresh_timeout.as_millis()).unwrap_or(u64::MAX),
            ))
        });
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

    pub(super) async fn watch_peer_signals(
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
            let reserved = {
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
                    let Some(lifecycle_cancel) = state.lifecycle_cancel.as_ref().cloned() else {
                        return Ok(());
                    };
                    state.refresh_inflight = true;
                    transition_locked(
                        &self.inner.events,
                        &entry,
                        &mut state,
                        McpServerState::Discovering,
                    );
                    Some((peer, lifecycle_cancel))
                }
            };
            let Some((peer, lifecycle_cancel)) = reserved else {
                notified.await;
                continue;
            };
            let _inflight_guard = EntryInflightGuard {
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
        let (lifecycle_cancel, watcher_cancel, peers, active_calls) = {
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
            let lifecycle_cancel = state.lifecycle_cancel.take();
            let watcher_cancel = state.watcher_cancel.take();
            // This method executes inside the watcher task. Dropping its stored JoinHandle lets
            // the current task return naturally without attempting to await or abort itself.
            state.watcher_task.take();
            let peers = take_cleanup_peers(&mut state);
            let active_calls = state.active_calls.values().cloned().collect::<Vec<_>>();
            (lifecycle_cancel, watcher_cancel, peers, active_calls)
        };
        if let Some(cancel) = lifecycle_cancel {
            cancel.cancel();
        }
        if let Some(cancel) = watcher_cancel {
            cancel.cancel();
        }
        let deadline = tokio::time::Instant::now() + self.inner.policy.lifecycle_cleanup_timeout;
        if let Err(error) = self
            .cleanup_connection_parts(
                &entry,
                peers,
                None,
                active_calls,
                McpOutcomeUnknownReason::ServerExited,
                deadline,
            )
            .await
        {
            if let Ok(mut state) = entry.state.lock() {
                if state.epoch == epoch {
                    set_error_locked(&self.inner.events, &entry, &mut state, &error);
                }
            }
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
}
