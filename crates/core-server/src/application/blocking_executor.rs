//! Temporary blocking bridge; domain transactions and lock order stay with their callers.
//! No worker may synchronously wait for a job in this or another executor while holding a
//! domain lock needed by that job. In particular, preparation and settlement never share workers.
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Instant;
use tokio::sync::{oneshot, Notify};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BlockingError {
    Closed,
    Overloaded,
    TooLarge,
    Cancelled,
    Panicked,
    WorkerUnavailable(String),
}
impl std::fmt::Display for BlockingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Closed => f.write_str("blocking executor is closed"),
            Self::Overloaded => f.write_str("blocking executor capacity exhausted"),
            Self::TooLarge => f.write_str("blocking job exceeds estimated byte budget"),
            Self::Cancelled => f.write_str("blocking job cancelled before execution"),
            Self::Panicked => f.write_str("blocking job panicked; mutation outcome may be unknown"),
            Self::WorkerUnavailable(message) => write!(f, "blocking worker unavailable: {message}"),
        }
    }
}
impl std::error::Error for BlockingError {}

type Job = Box<dyn FnOnce() + Send + 'static>;
struct State {
    queue: VecDeque<Job>,
    accepting: bool,
    allow_reserved: bool,
    submitted: usize,
    jobs: usize,
    bytes: usize,
    workers: usize,
}
struct Shared {
    label: &'static str,
    max_jobs: usize,
    max_bytes: usize,
    state: Mutex<State>,
    ready: Condvar,
    changed: Notify,
}
struct Owner(Arc<Shared>);
impl Drop for Owner {
    fn drop(&mut self) {
        close(&self.0);
    }
}
#[derive(Clone)]
pub(crate) struct BlockingExecutor(Arc<Owner>);
impl BlockingExecutor {
    pub(crate) fn new(
        label: &'static str,
        workers: usize,
        max_jobs: usize,
        max_bytes: usize,
    ) -> Result<Self, BlockingError> {
        if workers == 0 || max_jobs < workers || max_bytes == 0 {
            return Err(BlockingError::WorkerUnavailable(
                "invalid executor limits".into(),
            ));
        }
        let shared = Arc::new(Shared {
            label,
            max_jobs,
            max_bytes,
            state: Mutex::new(State {
                queue: VecDeque::new(),
                accepting: true,
                allow_reserved: true,
                submitted: 0,
                jobs: 0,
                bytes: 0,
                workers: 0,
            }),
            ready: Condvar::new(),
            changed: Notify::new(),
        });
        for index in 0..workers {
            shared
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .workers += 1;
            let worker = Arc::clone(&shared);
            if let Err(error) = std::thread::Builder::new()
                .name(format!("blocking-{label}-{index}"))
                .spawn(move || worker_loop(worker))
            {
                shared
                    .state
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .workers -= 1;
                close(&shared);
                return Err(BlockingError::WorkerUnavailable(error.to_string()));
            }
        }
        Ok(Self(Arc::new(Owner(shared))))
    }
    pub(crate) fn try_reserve(&self, bytes: usize) -> Result<BlockingReservation, BlockingError> {
        let shared = &self.0 .0;
        let mut state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
        if !state.accepting {
            return Err(BlockingError::Closed);
        }
        if bytes > shared.max_bytes {
            return Err(BlockingError::TooLarge);
        }
        if state.jobs >= shared.max_jobs || bytes > shared.max_bytes - state.bytes {
            return Err(BlockingError::Overloaded);
        }
        state.jobs += 1;
        state.bytes += bytes;
        drop(state);
        Ok(BlockingReservation {
            owner: Arc::clone(&self.0),
            bytes,
            submitted: false,
            admitted: Instant::now(),
            _probe: mycopilot_core::performance::track_queue(
                "blocking.estimated",
                shared.label,
                bytes,
            ),
        })
    }
    /// Only use behind an existing bounded logical owner (Turn permit / RPC lane). This does
    /// not enqueue a closure or copy its payload while capacity is unavailable.
    pub(crate) async fn reserve(&self, bytes: usize) -> Result<BlockingReservation, BlockingError> {
        loop {
            let changed = self.0 .0.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            match self.try_reserve(bytes) {
                Err(BlockingError::Overloaded) => changed.await,
                result => return result,
            }
        }
    }

    /// Atomically reserves settlement plus its owned delivery tail. Never retain the first half
    /// while waiting for the second: that would deadlock when every Turn fills half the budget.
    pub(crate) async fn reserve_pair(
        &self,
        bytes: usize,
    ) -> Result<(BlockingReservation, BlockingReservation), BlockingError> {
        loop {
            let changed = self.0 .0.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            match self.try_reserve_pair(bytes) {
                Err(BlockingError::Overloaded) => changed.await,
                result => return result,
            }
        }
    }

    fn try_reserve_pair(
        &self,
        bytes: usize,
    ) -> Result<(BlockingReservation, BlockingReservation), BlockingError> {
        let shared = &self.0 .0;
        let mut state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
        if !state.accepting {
            return Err(BlockingError::Closed);
        }
        let total = bytes.checked_mul(2).ok_or(BlockingError::TooLarge)?;
        if shared.max_jobs < 2 || total > shared.max_bytes {
            return Err(BlockingError::TooLarge);
        }
        if state.jobs > shared.max_jobs - 2 || total > shared.max_bytes - state.bytes {
            return Err(BlockingError::Overloaded);
        }
        state.jobs += 2;
        state.bytes += total;
        drop(state);
        let reserve = || BlockingReservation {
            owner: Arc::clone(&self.0),
            bytes,
            submitted: false,
            admitted: Instant::now(),
            _probe: mycopilot_core::performance::track_queue(
                "blocking.estimated",
                shared.label,
                bytes,
            ),
        };
        Ok((reserve(), reserve()))
    }
    pub(crate) fn submit<T, F>(
        &self,
        bytes: usize,
        work: F,
    ) -> Result<BlockingTask<T>, BlockingError>
    where
        T: Send + 'static,
        F: FnOnce() -> T + Send + 'static,
    {
        Ok(self.try_reserve(bytes)?.submit(work))
    }
    pub(crate) async fn run<T, F>(&self, bytes: usize, work: F) -> Result<T, BlockingError>
    where
        T: Send + 'static,
        F: FnOnce() -> T + Send + 'static,
    {
        self.submit(bytes, work)?.wait().await
    }
    pub(crate) fn close(&self) {
        close(&self.0 .0);
    }
    /// Reservations already granted may still submit; shutdown drains them too. Close after
    /// producers stop, with completion kept open until all active Turn owners retire.
    /// Final process boundary: previously submitted jobs drain, while unused reservations
    /// (e.g. a model future past shutdown grace) cannot publish a late mutation. The durable
    /// Turn/claim remains the recovery authority; no unstarted work is reported successful.
    pub(crate) async fn shutdown_submitted(&self) {
        {
            let mut state = self.0 .0.state.lock().unwrap_or_else(|e| e.into_inner());
            state.allow_reserved = false;
            state.accepting = false;
        }
        self.close();
        self.wait_drained(false).await;
    }
    pub(crate) async fn shutdown(&self) {
        self.wait_drained(true).await;
    }
    async fn wait_drained(&self, include_reservations: bool) {
        self.close();
        loop {
            let changed = self.0 .0.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            let finished = {
                let state = self.0 .0.state.lock().unwrap_or_else(|e| e.into_inner());
                (if include_reservations {
                    state.jobs
                } else {
                    state.submitted
                }) == 0
                    && state.workers == 0
            };
            if finished {
                return;
            }
            changed.await;
        }
    }
}
fn close(shared: &Shared) {
    shared
        .state
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .accepting = false;
    shared.ready.notify_all();
    shared.changed.notify_waiters();
}
fn worker_loop(shared: Arc<Shared>) {
    loop {
        let job = {
            let mut state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                if let Some(job) = state.queue.pop_front() {
                    break job;
                }
                if !state.accepting && (state.jobs == 0 || !state.allow_reserved) {
                    state.workers -= 1;
                    drop(state);
                    shared.changed.notify_waiters();
                    return;
                }
                state = shared.ready.wait(state).unwrap_or_else(|e| e.into_inner());
            }
        };
        job();
    }
}
pub(crate) struct BlockingReservation {
    owner: Arc<Owner>,
    bytes: usize,
    submitted: bool,
    admitted: Instant,
    _probe: Option<mycopilot_core::performance::QueueEntry>,
}
impl BlockingReservation {
    pub(crate) fn submit<T, F>(mut self, work: F) -> BlockingTask<T>
    where
        T: Send + 'static,
        F: FnOnce() -> T + Send + 'static,
    {
        let (sender, receiver) = oneshot::channel();
        let phase = Arc::new(AtomicU8::new(0));
        let worker_phase = Arc::clone(&phase);
        let runtime = tokio::runtime::Handle::try_current().ok();
        let shared = Arc::clone(&self.owner.0);
        let mut state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
        if !state.allow_reserved {
            drop(state);
            let _ = sender.send(Err(BlockingError::Closed));
            return BlockingTask { receiver, phase };
        }
        state.submitted += 1;
        self.submitted = true;
        let job = Box::new(move || {
            let _budget = self;
            if worker_phase
                .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
            {
                drop(_budget);
                let _ = sender.send(Err(BlockingError::Cancelled));
                return;
            }
            mycopilot_core::performance::record(
                "blocking.queue_wait",
                _budget.owner.0.label,
                _budget.admitted.elapsed(),
                _budget.bytes,
            );
            let started = Instant::now();
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _entered = runtime.as_ref().map(|runtime| runtime.enter());
                work()
            }))
            .map_err(|_| BlockingError::Panicked);
            mycopilot_core::performance::record(
                "blocking.execution",
                _budget.owner.0.label,
                started.elapsed(),
                _budget.bytes,
            );
            worker_phase.store(3, Ordering::Release);
            drop(_budget);
            let _ = sender.send(result);
        });
        state.queue.push_back(job);
        drop(state);
        shared.ready.notify_one();
        BlockingTask { receiver, phase }
    }
}
impl Drop for BlockingReservation {
    fn drop(&mut self) {
        let shared = &self.owner.0;
        let mut state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
        state.jobs -= 1;
        if self.submitted {
            state.submitted -= 1;
        }
        state.bytes -= self.bytes;
        drop(state);
        shared.changed.notify_waiters();
        shared.ready.notify_all();
    }
}
pub(crate) struct BlockingTask<T> {
    receiver: oneshot::Receiver<Result<T, BlockingError>>,
    // Current claim owners drain accepted jobs; keep explicit queued cancellation for
    // callers that can prove no durable ownership has been transferred.
    #[cfg_attr(not(test), expect(dead_code))]
    phase: Arc<AtomicU8>,
}
impl<T> BlockingTask<T> {
    /// Cancellation is explicit and linearizes against worker start. Dropping a waiter never
    /// claims to roll back a synchronous mutation that may already have started.
    #[cfg_attr(not(test), expect(dead_code))]
    pub(crate) fn cancel_before_start(&self) -> bool {
        self.phase
            .compare_exchange(0, 2, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }
    pub(crate) async fn wait(self) -> Result<T, BlockingError> {
        self.receiver.await.unwrap_or(Err(BlockingError::Panicked))
    }
}

/// Preparation, lease maintenance, cancellation and terminal settlement have distinct physical
/// workers. These budgets do not modify logical Turn quotas or transport admission capacities.
#[derive(Clone)]
pub(crate) struct BlockingWork {
    pub(crate) preparation: BlockingExecutor,
    pub(crate) maintenance: BlockingExecutor,
    pub(crate) lease: BlockingExecutor,
    pub(crate) control: BlockingExecutor,
    pub(crate) completion: BlockingExecutor,
    pub(crate) recovery: BlockingExecutor,
}
impl BlockingWork {
    pub(crate) fn new() -> Result<Self, BlockingError> {
        Self::with_turn_limit(super::agent_dispatcher::DEFAULT_AGENT_GLOBAL_CONCURRENCY)
    }
    pub(crate) fn with_turn_limit(limit: usize) -> Result<Self, BlockingError> {
        const MIB: usize = 1024 * 1024;
        Ok(Self {
            preparation: BlockingExecutor::new("preparation", 2, 8, 128 * MIB)?,
            maintenance: BlockingExecutor::new("maintenance", 1, 64, 16 * MIB)?,
            lease: BlockingExecutor::new("lease", 1, 64, 4 * MIB)?,
            control: BlockingExecutor::new("control", 2, 32, 16 * MIB)?,
            completion: BlockingExecutor::new(
                "completion",
                2,
                limit.saturating_mul(2).saturating_add(8),
                limit.max(8).saturating_mul(8 * MIB),
            )?,
            // A completed Turn may need the global admission fence for receipt recovery.
            // Its old cancellation owner is retired before using this lane, so deletion can
            // drain real completion jobs without waiting on its own admission lock here.
            recovery: BlockingExecutor::new(
                "recovery",
                1,
                limit.saturating_mul(2).saturating_add(8),
                limit.max(8).saturating_mul(8 * MIB),
            )?,
        })
    }
}
#[cfg(test)]
mod tests;
