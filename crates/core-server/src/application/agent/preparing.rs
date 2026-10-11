//! Process-owned preparation, before the initial durable Turn Trace exists.
//!
//! Entries are owned by already bounded RPC/executor jobs, not a second logical Turn quota.
//! No database, filesystem or model operation runs under `state`. A commit validates its token
//! inside the storage writer; cancellation/deletion/account fences must be ordered at that same
//! writer boundary, rather than treating this momentary validation as a commit-wide lock.
//!
//! Restart discards this process's tokens. Durable trigger claims and initial Trace receipts
//! distinguish unadmitted preparation from an accepted Turn; only startup may scan orphan claims.
use mycopilot_core::AgentCancellationToken;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use tokio::sync::Notify;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) enum PreparingScope {
    Conversation(String),
    Project(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PreparingToken {
    pub(super) boot_id: String,
    pub(super) generation: u64,
    pub(super) conversation_id: String,
    pub(super) run_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PreparingPhase {
    Preparing,
    Committing,
}

#[derive(Debug, Clone)]
pub(super) struct PreparingSnapshot {
    pub(super) token: PreparingToken,
    pub(super) project_id: Option<String>,
    pub(super) assistant_message_id: String,
    pub(super) source_revision: Option<i64>,
    pub(super) phase: PreparingPhase,
    pub(super) cancellation: AgentCancellationToken,
}

impl PreparingSnapshot {
    fn matches(&self, scope: &PreparingScope) -> bool {
        match scope {
            PreparingScope::Conversation(id) => &self.token.conversation_id == id,
            PreparingScope::Project(id) => self.project_id.as_ref() == Some(id),
        }
    }
}

#[derive(Debug, Default)]
struct PreparingState {
    entries: HashMap<String, PreparingSnapshot>,
    generation: u64,
    stopping: bool,
    deleted: HashSet<PreparingScope>,
}

impl PreparingState {
    fn scope_deleted(&self, conversation_id: &str, project_id: Option<&str>) -> bool {
        self.deleted
            .contains(&PreparingScope::Conversation(conversation_id.into()))
            || project_id
                .is_some_and(|id| self.deleted.contains(&PreparingScope::Project(id.into())))
    }

    fn validate(&self, token: &PreparingToken) -> Result<&PreparingSnapshot, String> {
        let entry = self
            .entries
            .get(&token.conversation_id)
            .filter(|entry| entry.token == *token)
            .ok_or_else(|| "Turn preparation no longer owns this Conversation.".to_string())?;
        if self.stopping
            || entry.cancellation.is_cancelled()
            || self.scope_deleted(&token.conversation_id, entry.project_id.as_deref())
        {
            return Err("Turn preparation was cancelled before admission.".into());
        }
        Ok(entry)
    }
}

#[derive(Debug)]
pub(super) struct PreparingRegistry {
    boot_id: String,
    state: Mutex<PreparingState>,
    idle: Condvar,
    changed: Notify,
}

impl Default for PreparingRegistry {
    fn default() -> Self {
        Self {
            boot_id: uuid::Uuid::new_v4().to_string(),
            state: Mutex::new(PreparingState::default()),
            idle: Condvar::new(),
            changed: Notify::new(),
        }
    }
}

impl PreparingRegistry {
    fn state(&self) -> MutexGuard<'_, PreparingState> {
        self.state.lock().unwrap_or_else(|error| error.into_inner())
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn register(
        self: &Arc<Self>,
        conversation_id: &str,
        project_id: Option<&str>,
        run_id: &str,
        assistant_message_id: &str,
        cancellation: AgentCancellationToken,
        source_revision: Option<i64>,
    ) -> Result<PreparingGuard, String> {
        if conversation_id.is_empty() || run_id.is_empty() || assistant_message_id.is_empty() {
            return Err(
                "Turn preparation requires exact Conversation, Run and assistant identities."
                    .into(),
            );
        }
        let mut state = self.state();
        if state.stopping {
            return Err("Agent runtime is shutting down.".into());
        }
        if cancellation.is_cancelled() || state.scope_deleted(conversation_id, project_id) {
            return Err("Turn preparation was cancelled before admission.".into());
        }
        if state.entries.contains_key(conversation_id) {
            return Err("Conversation already has a preparing Turn.".into());
        }
        if state
            .entries
            .values()
            .any(|entry| entry.token.run_id == run_id)
        {
            return Err("Run already owns another Turn preparation.".into());
        }
        state.generation = state
            .generation
            .checked_add(1)
            .ok_or_else(|| "Turn preparation generation is exhausted.".to_string())?;
        let token = PreparingToken {
            boot_id: self.boot_id.clone(),
            generation: state.generation,
            conversation_id: conversation_id.into(),
            run_id: run_id.into(),
        };
        state.entries.insert(
            conversation_id.into(),
            PreparingSnapshot {
                token: token.clone(),
                project_id: project_id.map(str::to_string),
                assistant_message_id: assistant_message_id.into(),
                source_revision,
                phase: PreparingPhase::Preparing,
                cancellation,
            },
        );
        Ok(PreparingGuard {
            registry: Arc::clone(self),
            token,
        })
    }

    pub(super) fn validate(&self, token: &PreparingToken) -> Result<(), String> {
        self.state().validate(token).map(|_| ())
    }

    pub(super) fn snapshot_run(&self, run_id: &str) -> Option<PreparingSnapshot> {
        self.state()
            .entries
            .values()
            .find(|entry| entry.token.run_id == run_id)
            .cloned()
    }

    pub(super) fn snapshot(&self) -> Vec<PreparingSnapshot> {
        self.state().entries.values().cloned().collect()
    }

    pub(super) fn cancel_run(&self, run_id: &str) -> bool {
        let state = self.state();
        let Some(entry) = state
            .entries
            .values()
            .find(|entry| entry.token.run_id == run_id)
        else {
            return false;
        };
        entry.cancellation.cancel();
        true
    }

    /// Cancellation retains ownership until the synchronous worker actually exits. It neither
    /// releases a physical worker nor allows a second Turn to replace this preparation.
    #[cfg(test)]
    pub(super) fn cancel_scope(&self, scope: &PreparingScope) -> Vec<String> {
        cancel_matching(&self.state(), scope)
    }

    /// Fence registration and cancellation under one short lock; the caller drains afterward.
    /// Failed deletion may remove its tombstone, but cancelled generations never become valid.
    pub(super) fn begin_delete_scope(&self, scope: &PreparingScope) -> Result<Vec<String>, String> {
        let mut state = self.state();
        if !state.deleted.insert(scope.clone()) {
            return Err("Turn preparation scope already has a deletion in progress.".into());
        }
        Ok(cancel_matching(&state, scope))
    }

    pub(super) fn clear_delete_scope(&self, scope: &PreparingScope) {
        self.state().deleted.remove(scope);
    }

    pub(super) fn begin_shutdown(&self) -> Vec<String> {
        let mut state = self.state();
        state.stopping = true;
        state
            .entries
            .values()
            .map(|entry| {
                entry.cancellation.cancel();
                entry.token.run_id.clone()
            })
            .collect()
    }

    /// Blocking callers must use the bounded control/recovery executor, never a Tokio worker.
    /// `wait_timeout` releases `state`, so retirement and unrelated registrations keep moving.
    pub(super) fn wait_scope_idle(&self, scope: &PreparingScope, timeout: Duration) -> bool {
        let started = Instant::now();
        let mut state = self.state();
        while state.entries.values().any(|entry| entry.matches(scope)) {
            let Some(remaining) = timeout.checked_sub(started.elapsed()) else {
                return false;
            };
            let (next, _) = self
                .idle
                .wait_timeout(state, remaining)
                .unwrap_or_else(|error| error.into_inner());
            state = next;
        }
        true
    }

    #[cfg(test)]
    pub(super) async fn wait_idle(&self, timeout: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self.state().entries.is_empty() {
                return true;
            }
            if tokio::time::timeout_at(deadline, changed).await.is_err() {
                return self.state().entries.is_empty();
            }
        }
    }
}

fn cancel_matching(state: &PreparingState, scope: &PreparingScope) -> Vec<String> {
    state
        .entries
        .values()
        .filter(|entry| entry.matches(scope))
        .map(|entry| {
            entry.cancellation.cancel();
            entry.token.run_id.clone()
        })
        .collect()
}

#[derive(Debug)]
pub(super) struct PreparingGuard {
    registry: Arc<PreparingRegistry>,
    token: PreparingToken,
}

impl PreparingGuard {
    #[cfg(test)]
    pub(super) fn token(&self) -> &PreparingToken {
        &self.token
    }

    pub(super) fn validate(&self) -> Result<(), String> {
        self.registry.validate(&self.token)
    }

    pub(super) fn set_revision(&self, revision: Option<i64>) -> Result<(), String> {
        let mut state = self.registry.state();
        state.validate(&self.token)?;
        let entry = state
            .entries
            .get_mut(&self.token.conversation_id)
            .expect("validated owner");
        if entry.phase != PreparingPhase::Preparing {
            return Err("Committing Turn preparation cannot change its source revision.".into());
        }
        entry.source_revision = revision;
        Ok(())
    }

    /// Called within the ordered final admission boundary. This is a phase marker, not permission
    /// to ignore cancellation: validation remains required immediately before durable commit.
    pub(super) fn mark_committing(&self) -> Result<(), String> {
        let mut state = self.registry.state();
        state.validate(&self.token)?;
        state
            .entries
            .get_mut(&self.token.conversation_id)
            .expect("validated owner")
            .phase = PreparingPhase::Committing;
        Ok(())
    }
}

impl Drop for PreparingGuard {
    fn drop(&mut self) {
        let mut state = self.registry.state();
        if state
            .entries
            .get(&self.token.conversation_id)
            .is_some_and(|entry| entry.token == self.token)
        {
            state.entries.remove(&self.token.conversation_id);
            drop(state);
            self.registry.idle.notify_all();
            self.registry.changed.notify_waiters();
        }
    }
}

#[cfg(test)]
mod tests;
