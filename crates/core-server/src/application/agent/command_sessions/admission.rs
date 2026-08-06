use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Copy)]
pub(super) struct HostCommandSessionAdmissionLimits {
    pub(super) max_sessions: usize,
    pub(super) max_sessions_per_conversation: usize,
}

impl Default for HostCommandSessionAdmissionLimits {
    fn default() -> Self {
        Self {
            max_sessions: 32,
            max_sessions_per_conversation: 8,
        }
    }
}

impl HostCommandSessionAdmissionLimits {
    pub(super) fn validate(self) -> Result<Self, String> {
        if self.max_sessions == 0
            || self.max_sessions_per_conversation == 0
            || self.max_sessions_per_conversation > self.max_sessions
        {
            return Err(
                "命令 Session Host 配额必须为正数，且会话配额不能超过全局配额。".to_string(),
            );
        }
        Ok(self)
    }
}

pub(super) struct HostCommandSessionAdmission {
    limits: HostCommandSessionAdmissionLimits,
    state: Mutex<HostCommandSessionAdmissionState>,
}

#[derive(Default)]
struct HostCommandSessionAdmissionState {
    total: usize,
    by_conversation: HashMap<String, usize>,
}

pub(super) struct HostCommandSessionAdmissionLease {
    admission: Arc<HostCommandSessionAdmission>,
    conversation_id: String,
    released: bool,
}

impl HostCommandSessionAdmission {
    pub(super) fn new(limits: HostCommandSessionAdmissionLimits) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            limits: limits.validate()?,
            state: Mutex::new(HostCommandSessionAdmissionState::default()),
        }))
    }

    pub(super) fn try_acquire(
        self: &Arc<Self>,
        conversation_id: &str,
    ) -> Result<HostCommandSessionAdmissionLease, String> {
        let mut state = lock(&self.state);
        if state.total >= self.limits.max_sessions {
            return Err(format!(
                "命令 Session Host 已达到全局保留上限（{}）；请等待已有 Session 完成结算。",
                self.limits.max_sessions
            ));
        }
        let conversation_count = state
            .by_conversation
            .get(conversation_id)
            .copied()
            .unwrap_or(0);
        if conversation_count >= self.limits.max_sessions_per_conversation {
            return Err(format!(
                "当前会话已达到命令 Session 保留上限（{}）；请等待已有 Session 完成结算。",
                self.limits.max_sessions_per_conversation
            ));
        }
        state.total = state.total.saturating_add(1);
        state
            .by_conversation
            .insert(conversation_id.to_string(), conversation_count + 1);
        Ok(HostCommandSessionAdmissionLease {
            admission: Arc::clone(self),
            conversation_id: conversation_id.to_string(),
            released: false,
        })
    }

    fn release(&self, conversation_id: &str) {
        let mut state = lock(&self.state);
        state.total = state.total.saturating_sub(1);
        if let Some(count) = state.by_conversation.get_mut(conversation_id) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                state.by_conversation.remove(conversation_id);
            }
        }
    }

    #[cfg(test)]
    pub(super) fn retained(&self) -> usize {
        lock(&self.state).total
    }
}

impl HostCommandSessionAdmissionLease {
    pub(super) fn release(mut self) {
        if !self.released {
            self.admission.release(&self.conversation_id);
            self.released = true;
        }
    }
}

impl Drop for HostCommandSessionAdmissionLease {
    fn drop(&mut self) {
        if !self.released {
            self.admission.release(&self.conversation_id);
            self.released = true;
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lease_counts_until_explicit_release_or_drop() {
        let admission = HostCommandSessionAdmission::new(HostCommandSessionAdmissionLimits {
            max_sessions: 2,
            max_sessions_per_conversation: 1,
        })
        .unwrap();
        let first = admission.try_acquire("conversation-a").unwrap();
        assert!(admission.try_acquire("conversation-a").is_err());
        let second = admission.try_acquire("conversation-b").unwrap();
        assert!(admission.try_acquire("conversation-c").is_err());
        first.release();
        assert_eq!(admission.retained(), 1);
        drop(second);
        assert_eq!(admission.retained(), 0);
    }
}
