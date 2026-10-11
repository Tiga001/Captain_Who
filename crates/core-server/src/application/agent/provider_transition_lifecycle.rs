//! Process ownership for provider-transition compactions. These are not ordinary Agent turns
//! and therefore must be cancelled and joined separately before closing the storage executor.
use super::*;
use std::sync::atomic::AtomicUsize;
use tokio::task::JoinSet;

pub(super) struct ProviderTransitionTasks {
    state: Mutex<TaskState>,
    cancellation: AgentCancellationToken,
    active: Arc<AtomicUsize>,
    first_failure: Mutex<Option<String>>,
    drain: tokio::sync::Mutex<()>,
}
struct TaskState {
    accepting: bool,
    tasks: JoinSet<()>,
}
struct ActiveTask(Arc<AtomicUsize>);
impl Drop for ActiveTask {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}
impl Default for ProviderTransitionTasks {
    fn default() -> Self {
        Self {
            state: Mutex::new(TaskState {
                accepting: true,
                tasks: JoinSet::new(),
            }),
            cancellation: AgentCancellationToken::new(),
            active: Arc::new(AtomicUsize::new(0)),
            first_failure: Mutex::new(None),
            drain: tokio::sync::Mutex::new(()),
        }
    }
}
impl ProviderTransitionTasks {
    pub(super) fn cancellation(&self) -> AgentCancellationToken {
        self.cancellation.clone()
    }

    pub(super) fn spawn(
        &self,
        work: impl Future<Output = ()> + Send + 'static,
    ) -> Result<(), String> {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        while let Some(result) = state.tasks.try_join_next() {
            if let Err(error) = result {
                self.record_failure(format!("provider transition task failed: {error}"));
            }
        }
        if !state.accepting {
            return Err("provider_transition_executor_closed".into());
        }
        self.active.fetch_add(1, Ordering::AcqRel);
        // Construct outside the future so even a task dropped before its first poll retires.
        let active = ActiveTask(Arc::clone(&self.active));
        state.tasks.spawn(async move {
            let _active = active;
            work.await;
        });
        Ok(())
    }

    pub(super) fn begin_shutdown(&self) -> usize {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .accepting = false;
        self.cancellation.cancel();
        self.active()
    }

    pub(super) fn active(&self) -> usize {
        self.active.load(Ordering::Acquire)
    }

    pub(super) fn record_failure(&self, error: String) {
        eprintln!("provider transition durability/lifecycle failure: {error}");
        // Failure reporting remains bounded even if a long-lived process has repeated errors.
        let mut first = self
            .first_failure
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if first.is_none() {
            *first = Some(error);
        }
    }

    pub(super) async fn shutdown(&self) -> Result<(), String> {
        self.begin_shutdown();
        let _drain = self.drain.lock().await;
        // Keep JoinSet in its process owner. Cancelling this shutdown awaiter must not drop
        // the set and abort tasks which still own receipt/usage settlement.
        while let Some(result) = std::future::poll_fn(|context| {
            self.state
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .tasks
                .poll_join_next(context)
        })
        .await
        {
            if let Err(error) = result {
                self.record_failure(format!("provider transition task failed: {error}"));
            }
        }
        match self
            .first_failure
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_ref()
        {
            Some(error) => Err(error.clone()),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cancelled_shutdown_future_does_not_abort_started_settlement() {
        let tasks = ProviderTransitionTasks::default();
        let release = Arc::new(Notify::new());
        let finished = Arc::new(AtomicBool::new(false));
        let task_release = Arc::clone(&release);
        let task_finished = Arc::clone(&finished);
        tasks
            .spawn(async move {
                task_release.notified().await;
                task_finished.store(true, Ordering::Release);
            })
            .unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(20), tasks.shutdown())
                .await
                .is_err()
        );
        assert_eq!(tasks.active(), 1);
        assert!(!finished.load(Ordering::Acquire));
        release.notify_one();
        tokio::time::timeout(Duration::from_secs(2), tasks.shutdown())
            .await
            .unwrap()
            .unwrap();
        assert!(finished.load(Ordering::Acquire));
        assert_eq!(tasks.active(), 0);
    }
}
