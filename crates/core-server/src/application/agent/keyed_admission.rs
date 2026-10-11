//! Logical admission ordering by the complete, already-authorized Conversation resource set.
//!
//! The registry mutex only updates waiter/owner bookkeeping. A returned guard reserves resources
//! without retaining that mutex; its holder may perform preparation on a bounded blocking worker.
//! Control intent must use Preparing/stop/delete fences instead of waiting behind this guard.
//! Keys are lookup results from Core state, never authority inferred from an untrusted DTO.
//!
//! Waiters are bounded by their existing RPC/executor/Turn owners. Conflicting requests are FIFO;
//! an unrelated Conversation can pass blocked work, but nobody can pass a waiting global fence.
//! Multi-resource requests declare their entire set before admission. Nested acquisition on this
//! coordinator is rejected, including attempts to upgrade a Conversation reservation to global.
use std::collections::{HashSet, VecDeque};
use std::marker::PhantomData;
use std::rc::Rc;
use std::sync::{Condvar, LockResult, Mutex, MutexGuard};
use std::thread::{self, ThreadId};

#[derive(Debug, Clone)]
enum Scope {
    Global,
    Conversations(Vec<String>),
}

impl Scope {
    fn conflicts(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Global, _) | (_, Self::Global) => true,
            (Self::Conversations(left), Self::Conversations(right)) => {
                left.iter().any(|id| right.binary_search(id).is_ok())
            }
        }
    }

    fn label(&self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::Conversations(_) => "conversation",
        }
    }
}

#[derive(Debug)]
struct Waiter {
    ticket: u64,
    thread: ThreadId,
    scope: Scope,
}

#[derive(Debug, Default)]
struct State {
    generation: u64,
    queue: VecDeque<Waiter>,
    global: bool,
    active_keys: HashSet<String>,
    owners: HashSet<ThreadId>,
}

impl State {
    fn ready(&self, index: usize) -> bool {
        let scope = &self.queue[index].scope;
        if self.global {
            return false;
        }
        let keys_busy = match scope {
            Scope::Global => !self.active_keys.is_empty(),
            Scope::Conversations(keys) => keys.iter().any(|key| self.active_keys.contains(key)),
        };
        !keys_busy
            && self
                .queue
                .iter()
                .take(index)
                .all(|earlier| !scope.conflicts(&earlier.scope))
    }
}

#[derive(Debug, Default)]
pub(super) struct KeyedAdmission {
    state: Mutex<State>,
    changed: Condvar,
}

impl KeyedAdmission {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|error| error.into_inner())
    }

    /// Compatibility for audited startup and conservative global callers. This coordinator does
    /// not expose its bookkeeping mutex, so application panics cannot poison the admission state.
    pub(super) fn lock(&self) -> LockResult<AdmissionGuard<'_>> {
        Ok(self.acquire(Scope::Global))
    }

    pub(super) fn lock_conversation(&self, conversation_id: &str) -> AdmissionGuard<'_> {
        self.lock_conversations(&[conversation_id.to_string()])
    }

    pub(super) fn lock_conversations(&self, conversation_ids: &[String]) -> AdmissionGuard<'_> {
        assert!(
            !conversation_ids.is_empty() && conversation_ids.iter().all(|id| !id.is_empty()),
            "resource admission requires a non-empty, authoritative Conversation resource set"
        );
        let mut keys = conversation_ids.to_vec();
        keys.sort_unstable();
        keys.dedup();
        self.acquire(Scope::Conversations(keys))
    }

    fn acquire(&self, scope: Scope) -> AdmissionGuard<'_> {
        let current_thread = thread::current().id();
        let wait = mycopilot_core::performance::Span::new("resource.admission_wait", scope.label());
        let mut state = self.state();
        if state.owners.contains(&current_thread)
            || state
                .queue
                .iter()
                .any(|waiter| waiter.thread == current_thread)
        {
            // Drop bookkeeping before reporting the programming error; catching a nested-call
            // panic must leave the original owner and all unrelated keys usable.
            drop(state);
            panic!("nested resource admission is forbidden; declare all resources in one request");
        }
        let ticket = state
            .generation
            .checked_add(1)
            .expect("admission ticket exhausted");
        state.generation = ticket;
        state.queue.push_back(Waiter {
            ticket,
            thread: current_thread,
            scope,
        });
        self.changed.notify_all();
        loop {
            let index = state
                .queue
                .iter()
                .position(|waiter| waiter.ticket == ticket)
                .expect("queued admission retains its ticket");
            if state.ready(index) {
                let waiter = state
                    .queue
                    .remove(index)
                    .expect("ready admission is queued");
                match &waiter.scope {
                    Scope::Global => state.global = true,
                    Scope::Conversations(keys) => state.active_keys.extend(keys.iter().cloned()),
                }
                state.owners.insert(current_thread);
                drop(state);
                drop(wait);
                let hold = mycopilot_core::performance::Span::new(
                    "resource.admission_hold",
                    waiter.scope.label(),
                );
                return AdmissionGuard {
                    admission: self,
                    owner: current_thread,
                    scope: waiter.scope,
                    _hold: hold,
                    _not_send: PhantomData,
                };
            }
            state = self
                .changed
                .wait(state)
                .unwrap_or_else(|error| error.into_inner());
        }
    }
}

/// Like `MutexGuard`, a reservation cannot move to another thread. The owner thread is part of
/// nested-acquisition detection, and RAII always releases the exact originally declared set.
pub(super) struct AdmissionGuard<'a> {
    admission: &'a KeyedAdmission,
    owner: ThreadId,
    scope: Scope,
    _hold: mycopilot_core::performance::Span,
    _not_send: PhantomData<Rc<()>>,
}

impl Drop for AdmissionGuard<'_> {
    fn drop(&mut self) {
        let mut state = self.admission.state();
        match &self.scope {
            Scope::Global => state.global = false,
            Scope::Conversations(keys) => {
                for key in keys {
                    state.active_keys.remove(key);
                }
            }
        }
        state.owners.remove(&self.owner);
        drop(state);
        self.admission.changed.notify_all();
    }
}

#[cfg(test)]
mod tests;
