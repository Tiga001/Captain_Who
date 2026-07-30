//! Registry subscription and managed-entry reconciliation.

use super::*;

impl McpConnectionManager {
    pub(super) fn ensure_registry_watcher(&self) {
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

pub(super) async fn reconcile_registry_record(
    manager: &McpConnectionManager,
    record: McpRegistryEntry,
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

pub(super) async fn stop_and_forget_removed_change(
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
