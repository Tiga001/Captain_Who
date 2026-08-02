//! Manager-wide graceful and forced shutdown coordination.

use super::*;

impl McpConnectionManager {
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

    pub(super) fn graceful_shutdown_cleanup_complete(
        &self,
        results: &[McpBatchOperationResult],
    ) -> bool {
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
                    && state.lifecycle_cancel.is_none()
                    && !state.connect_inflight
                    && !state.refresh_inflight
                    && !state.restart_inflight
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

    pub(super) async fn force_shutdown_entries(
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
            let (peers, lifecycle_cancel, cancel, watcher, active_calls, status) = {
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
                state.restart_inflight = false;
                let lifecycle_cancel = state.lifecycle_cancel.take();
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
                (
                    peers,
                    lifecycle_cancel,
                    cancel,
                    watcher,
                    active_calls,
                    state.status.clone(),
                )
            };
            if let Some(cancel) = lifecycle_cancel {
                cancel.cancel();
            }
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
                && state.lifecycle_cancel.is_none()
                && !state.connect_inflight
                && !state.refresh_inflight
                && !state.restart_inflight
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
        let drain_deadline =
            tokio::time::Instant::now() + self.inner.policy.lifecycle_cleanup_timeout;
        let entries_to_cancel = self
            .inner
            .entries
            .lock()
            .map(|entries| entries.values().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        for entry in entries_to_cancel {
            if let Ok(state) = entry.state.lock() {
                if let Some(cancel) = state.lifecycle_cancel.as_ref() {
                    cancel.cancel();
                }
            }
        }
        let mut admitted_start_timeout = false;
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
            if tokio::time::timeout_at(drain_deadline, settled)
                .await
                .is_err()
            {
                admitted_start_timeout = true;
                break;
            }
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
        if admitted_start_timeout {
            results.push(manager_task_batch_failure());
        }
        results.sort_by_key(|result| result.server_id);
        results
    }
}
