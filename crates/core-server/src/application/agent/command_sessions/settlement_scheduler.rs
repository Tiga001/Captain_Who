use super::{AgentCommandSessionRegistryInner, HandoffState, HostCommandSession};
use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::thread;
use std::time::{Duration, Instant};

const RETRY_INITIAL: Duration = Duration::from_millis(25);
const RETRY_MAX: Duration = Duration::from_secs(1);

pub(super) struct CommandSessionSettlementScheduler {
    state: Mutex<SchedulerState>,
    changed: Condvar,
    worker_exited: Condvar,
    worker_threads_spawned: AtomicUsize,
    settlement_attempts: AtomicUsize,
}

struct SchedulerState {
    ready: VecDeque<ScheduledSettlement>,
    delayed: Vec<ScheduledSettlement>,
    known: HashSet<String>,
    stopping: bool,
    worker_started: bool,
    worker_done: bool,
    next_order: u64,
}

struct ScheduledSettlement {
    session_id: String,
    session: Arc<HostCommandSession>,
    due_at: Instant,
    retry_delay: Duration,
    order: u64,
}

impl CommandSessionSettlementScheduler {
    pub(super) fn start(
        registry: Weak<AgentCommandSessionRegistryInner>,
    ) -> Arc<CommandSessionSettlementScheduler> {
        let scheduler = Arc::new(Self {
            state: Mutex::new(SchedulerState {
                ready: VecDeque::new(),
                delayed: Vec::new(),
                known: HashSet::new(),
                stopping: false,
                worker_started: false,
                worker_done: false,
                next_order: 1,
            }),
            changed: Condvar::new(),
            worker_exited: Condvar::new(),
            worker_threads_spawned: AtomicUsize::new(0),
            settlement_attempts: AtomicUsize::new(0),
        });
        let worker_scheduler = Arc::clone(&scheduler);
        match thread::Builder::new()
            .name("command-session-settlement".to_string())
            .spawn(move || worker_scheduler.run(registry))
        {
            Ok(_worker) => {
                let mut state = lock(&scheduler.state);
                state.worker_started = true;
                scheduler
                    .worker_threads_spawned
                    .fetch_add(1, Ordering::Relaxed);
                scheduler.changed.notify_all();
            }
            Err(error) => {
                let mut state = lock(&scheduler.state);
                state.stopping = true;
                state.worker_done = true;
                // Scheduling will return this generic unavailable condition. There is no safe
                // per-Session fallback: that would recreate the unbounded thread architecture.
                let _ = error;
                scheduler.worker_exited.notify_all();
            }
        }
        scheduler
    }

    pub(super) fn schedule(&self, session: Arc<HostCommandSession>) -> Result<(), String> {
        let session_id = session
            .session_id_string()
            .ok_or_else(|| "命令 Session 尚未建立可调度的进程身份。".to_string())?;
        let mut state = lock(&self.state);
        if state.stopping || !state.worker_started || state.worker_done {
            return Err("命令 Session 终态结算调度器不可用。".to_string());
        }
        if !state.known.insert(session_id.clone()) {
            return Ok(());
        }
        let order = take_next_order(&mut state);
        state.ready.push_back(ScheduledSettlement {
            session_id,
            session,
            due_at: Instant::now(),
            retry_delay: RETRY_INITIAL,
            order,
        });
        self.changed.notify_one();
        Ok(())
    }

    pub(super) fn shutdown(&self, wait: Duration) -> bool {
        self.request_stop();
        let state = lock(&self.state);
        if state.worker_done {
            return true;
        }
        let (state, _) = self
            .worker_exited
            .wait_timeout_while(state, wait, |state| !state.worker_done)
            .unwrap_or_else(|error| error.into_inner());
        state.worker_done
    }

    pub(super) fn request_stop(&self) {
        let mut state = lock(&self.state);
        state.stopping = true;
        // Pending retries intentionally remain only in memory. Their durable Session rows stay
        // active and startup reconciliation will conservatively mark them outcome-unknown.
        state.ready.clear();
        state.delayed.clear();
        state.known.clear();
        self.changed.notify_all();
    }

    fn run(&self, registry: Weak<AgentCommandSessionRegistryInner>) {
        loop {
            let Some(job) = self.next_job() else {
                break;
            };
            let Some(registry) = registry.upgrade() else {
                break;
            };
            self.settlement_attempts.fetch_add(1, Ordering::Relaxed);
            let result = if job.session.is_terminal_settled() {
                Ok(())
            } else if let Some(terminal) = job.session.terminal() {
                match job.session.handoff_state() {
                    HandoffState::Adopted => registry.settle_handed_off(&job.session, &terminal),
                    HandoffState::Synchronous | HandoffState::Aborted => {
                        registry.settle_synchronous(&job.session, &terminal)
                    }
                    HandoffState::Pending => {
                        Err("未完成持久交接的命令 Session 不能进入终态结算队列。".to_string())
                    }
                }
            } else {
                Err("命令 Session 在缺少进程终态时被提交到结算队列。".to_string())
            };
            match result {
                Ok(()) => {
                    job.session.mark_terminal_settled();
                    self.complete(&job.session_id);
                }
                Err(error) => {
                    job.session.record_persistence_error(error);
                    self.retry(job);
                }
            }
        }
        let mut state = lock(&self.state);
        state.worker_done = true;
        state.ready.clear();
        state.delayed.clear();
        state.known.clear();
        self.worker_exited.notify_all();
    }

    fn next_job(&self) -> Option<ScheduledSettlement> {
        let mut state = lock(&self.state);
        loop {
            if state.stopping {
                return None;
            }
            move_due_jobs(&mut state);
            if let Some(job) = state.ready.pop_front() {
                return Some(job);
            }
            let wait = state
                .delayed
                .iter()
                .map(|job| job.due_at)
                .min()
                .map(|due_at| due_at.saturating_duration_since(Instant::now()));
            state = match wait {
                Some(wait) => {
                    self.changed
                        .wait_timeout(state, wait)
                        .unwrap_or_else(|error| error.into_inner())
                        .0
                }
                None => self
                    .changed
                    .wait(state)
                    .unwrap_or_else(|error| error.into_inner()),
            };
        }
    }

    fn complete(&self, session_id: &str) {
        let mut state = lock(&self.state);
        state.known.remove(session_id);
    }

    fn retry(&self, mut job: ScheduledSettlement) {
        let mut state = lock(&self.state);
        if state.stopping {
            state.known.remove(&job.session_id);
            return;
        }
        job.due_at = Instant::now() + job.retry_delay;
        job.retry_delay = job.retry_delay.saturating_mul(2).min(RETRY_MAX);
        job.order = take_next_order(&mut state);
        state.delayed.push(job);
        self.changed.notify_one();
    }

    #[cfg(test)]
    pub(super) fn stats(&self) -> (usize, usize, usize) {
        let state = lock(&self.state);
        (
            self.worker_threads_spawned.load(Ordering::Relaxed),
            state.known.len(),
            self.settlement_attempts.load(Ordering::Relaxed),
        )
    }
}

fn move_due_jobs(state: &mut SchedulerState) {
    let now = Instant::now();
    let mut due = Vec::new();
    let mut index = 0;
    while index < state.delayed.len() {
        if state.delayed[index].due_at <= now {
            due.push(state.delayed.swap_remove(index));
        } else {
            index += 1;
        }
    }
    due.sort_by_key(|job| (job.due_at, job.order));
    state.ready.extend(due);
}

fn take_next_order(state: &mut SchedulerState) -> u64 {
    let order = state.next_order;
    state.next_order = state.next_order.saturating_add(1);
    order
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}
