use crate::protocol::{AgentError, AgentResult};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct AgentCancellationToken {
    flag: Arc<AtomicBool>,
}

impl Default for AgentCancellationToken {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentCancellationToken {
    pub fn new() -> Self {
        Self {
            flag: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn cancel(&self) {
        self.flag.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }

    /// Returns whether both handles address the same cancellation state.
    ///
    /// This is intentionally stronger than comparing the current cancelled value: lifecycle
    /// owners use it to avoid unregistering a newer token that reused the same stable run ID.
    pub fn shares_state_with(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.flag, &other.flag)
    }

    pub fn check(&self) -> AgentResult<()> {
        if self.is_cancelled() {
            Err(AgentError::cancelled())
        } else {
            Ok(())
        }
    }

    pub async fn cancelled(&self) {
        while !self.is_cancelled() {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}
