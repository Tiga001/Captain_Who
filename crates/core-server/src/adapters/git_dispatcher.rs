use std::collections::VecDeque;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use mycopilot_protocol_rs::{error, JsonRpcId};
use serde_json::Value;
use tokio::sync::mpsc;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::task::JoinHandle;

pub(crate) const DEFAULT_GIT_WORKER_COUNT: usize = 3;
pub(crate) const DEFAULT_GIT_MAX_IN_FLIGHT: usize = 128;
const DEFAULT_GIT_HIGH_PRIORITY_RESERVE: usize = 8;
const DEFAULT_GIT_MEDIUM_PRIORITY_RESERVE: usize = 8;

const GIT_DISPATCH_QUEUE_FULL_CODE: i64 = -32001;
const GIT_DISPATCH_UNAVAILABLE_CODE: i64 = -32603;
const READ_PRIORITY_SCHEDULE: [GitJobPriority; 3] = [
    GitJobPriority::Medium,
    GitJobPriority::Medium,
    GitJobPriority::Low,
];

type GitJobTask = Box<dyn FnOnce() -> Value + Send + 'static>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GitJobPriority {
    High,
    Medium,
    Low,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GitDispatchError {
    Full,
    Closed,
}

impl GitDispatchError {
    pub(crate) fn code(self) -> i64 {
        match self {
            Self::Full => GIT_DISPATCH_QUEUE_FULL_CODE,
            Self::Closed => GIT_DISPATCH_UNAVAILABLE_CODE,
        }
    }

    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::Full => "The Git review queue is full. Please retry shortly.",
            Self::Closed => "The Git review service is shutting down.",
        }
    }
}

#[derive(Debug)]
pub(crate) struct GitDispatcherShutdownError {
    message: String,
}

impl std::fmt::Display for GitDispatcherShutdownError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for GitDispatcherShutdownError {}

pub(crate) struct GitDispatcher {
    admission: AdmissionCapacity,
    manager: JoinHandle<()>,
    sender: Option<mpsc::Sender<QueuedGitJob>>,
    shutdown_requested: Arc<AtomicBool>,
}

impl GitDispatcher {
    pub(crate) fn new(outbound: mpsc::UnboundedSender<Value>) -> Self {
        Self::with_capacity_reservations(
            outbound,
            DEFAULT_GIT_WORKER_COUNT,
            DEFAULT_GIT_MAX_IN_FLIGHT,
            DEFAULT_GIT_HIGH_PRIORITY_RESERVE,
            DEFAULT_GIT_MEDIUM_PRIORITY_RESERVE,
        )
    }

    #[cfg(test)]
    fn with_limits(
        outbound: mpsc::UnboundedSender<Value>,
        worker_count: usize,
        max_in_flight: usize,
    ) -> Self {
        Self::with_capacity_reservations(outbound, worker_count, max_in_flight, 0, 0)
    }

    fn with_capacity_reservations(
        outbound: mpsc::UnboundedSender<Value>,
        worker_count: usize,
        max_in_flight: usize,
        high_priority_reserve: usize,
        medium_priority_reserve: usize,
    ) -> Self {
        let worker_count = worker_count.max(1);
        let max_in_flight = max_in_flight.max(1);
        let high_priority_reserve = high_priority_reserve.min(max_in_flight);
        let medium_priority_reserve =
            medium_priority_reserve.min(max_in_flight - high_priority_reserve);
        let admission = AdmissionCapacity {
            low: Arc::new(Semaphore::new(
                max_in_flight - high_priority_reserve - medium_priority_reserve,
            )),
            non_high: Arc::new(Semaphore::new(max_in_flight - high_priority_reserve)),
            total: Arc::new(Semaphore::new(max_in_flight)),
        };
        let shutdown_requested = Arc::new(AtomicBool::new(false));
        let (sender, receiver) = mpsc::channel(max_in_flight);
        let manager = tokio::spawn(run_dispatcher(
            receiver,
            outbound,
            worker_count,
            Arc::clone(&shutdown_requested),
        ));
        Self {
            admission,
            manager,
            sender: Some(sender),
            shutdown_requested,
        }
    }

    pub(crate) fn try_submit<F>(
        &self,
        priority: GitJobPriority,
        request_id: JsonRpcId,
        task: F,
    ) -> Result<(), GitDispatchError>
    where
        F: FnOnce() -> Value + Send + 'static,
    {
        let Some(sender) = self.sender.as_ref() else {
            return Err(GitDispatchError::Closed);
        };
        let permits = self.admission.try_acquire(priority)?;
        let job = QueuedGitJob {
            permits,
            priority,
            request_id,
            task: Box::new(task),
        };
        sender.try_send(job).map_err(|error| match error {
            mpsc::error::TrySendError::Full(_) => GitDispatchError::Full,
            mpsc::error::TrySendError::Closed(_) => GitDispatchError::Closed,
        })
    }

    /// Stops admission, returns an error for queued requests, and waits for running workers.
    pub(crate) async fn shutdown(mut self) -> Result<(), GitDispatcherShutdownError> {
        self.shutdown_requested.store(true, Ordering::Release);
        self.sender.take();
        self.manager
            .await
            .map_err(|error| GitDispatcherShutdownError {
                message: format!("Git dispatcher stopped unexpectedly: {error}"),
            })
    }
}

struct AdmissionCapacity {
    low: Arc<Semaphore>,
    non_high: Arc<Semaphore>,
    total: Arc<Semaphore>,
}

impl AdmissionCapacity {
    fn try_acquire(
        &self,
        priority: GitJobPriority,
    ) -> Result<Vec<OwnedSemaphorePermit>, GitDispatchError> {
        let mut permits = Vec::with_capacity(3);
        if priority == GitJobPriority::Low {
            permits.push(
                Arc::clone(&self.low)
                    .try_acquire_owned()
                    .map_err(|_| GitDispatchError::Full)?,
            );
        }
        if priority != GitJobPriority::High {
            permits.push(
                Arc::clone(&self.non_high)
                    .try_acquire_owned()
                    .map_err(|_| GitDispatchError::Full)?,
            );
        }
        permits.push(
            Arc::clone(&self.total)
                .try_acquire_owned()
                .map_err(|_| GitDispatchError::Full)?,
        );
        Ok(permits)
    }
}

struct QueuedGitJob {
    permits: Vec<OwnedSemaphorePermit>,
    priority: GitJobPriority,
    request_id: JsonRpcId,
    task: GitJobTask,
}

#[derive(Default)]
struct PriorityQueues {
    high: VecDeque<QueuedGitJob>,
    low: VecDeque<QueuedGitJob>,
    medium: VecDeque<QueuedGitJob>,
    schedule_cursor: usize,
}

impl PriorityQueues {
    fn push(&mut self, job: QueuedGitJob) {
        match job.priority {
            GitJobPriority::High => self.high.push_back(job),
            GitJobPriority::Medium => self.medium.push_back(job),
            GitJobPriority::Low => self.low.push_back(job),
        }
    }

    fn pop_high(&mut self) -> Option<QueuedGitJob> {
        self.high.pop_front()
    }

    fn pop_read(&mut self) -> Option<QueuedGitJob> {
        for _ in 0..READ_PRIORITY_SCHEDULE.len() {
            let priority = READ_PRIORITY_SCHEDULE[self.schedule_cursor];
            self.schedule_cursor = (self.schedule_cursor + 1) % READ_PRIORITY_SCHEDULE.len();
            let next = match priority {
                GitJobPriority::Medium => self.medium.pop_front(),
                GitJobPriority::Low => self.low.pop_front(),
                GitJobPriority::High => unreachable!("high jobs are scheduled exclusively"),
            };
            if next.is_some() {
                return next;
            }
        }
        None
    }

    fn pop_any(&mut self) -> Option<QueuedGitJob> {
        self.pop_high().or_else(|| self.pop_read())
    }

    fn has_high(&self) -> bool {
        !self.high.is_empty()
    }

    fn is_empty(&self) -> bool {
        self.high.is_empty() && self.medium.is_empty() && self.low.is_empty()
    }
}

async fn run_dispatcher(
    mut receiver: mpsc::Receiver<QueuedGitJob>,
    outbound: mpsc::UnboundedSender<Value>,
    worker_count: usize,
    shutdown_requested: Arc<AtomicBool>,
) {
    let (completion_tx, mut completion_rx) = mpsc::unbounded_channel::<bool>();
    let mut accepting = true;
    let mut exclusive_running = false;
    let mut queues = PriorityQueues::default();
    let mut running = 0usize;

    loop {
        if shutdown_requested.load(Ordering::Acquire) {
            receiver.close();
            accepting = false;
        }
        drain_admission_channel(&mut receiver, &mut queues, &mut accepting);

        if shutdown_requested.load(Ordering::Acquire) {
            while let Some(job) = queues.pop_any() {
                let _ = outbound.send(worker_failure_response(
                    job.request_id,
                    "Git request was cancelled because the core server is shutting down.",
                ));
                // Dropping the job releases its admission permit and task closure.
            }
        }

        while running < worker_count && !exclusive_running {
            // Mutations are globally exclusive. They cannot be pre-empted, so once one is queued,
            // stop admitting new reads and let already-running reads reach their snapshot checks.
            if queues.has_high() {
                if running > 0 {
                    break;
                }
                let job = queues
                    .pop_high()
                    .expect("a high-priority job was observed above");
                running += 1;
                exclusive_running = true;
                launch_job(job, true, outbound.clone(), completion_tx.clone());
                break;
            }

            let Some(job) = queues.pop_read() else {
                break;
            };
            running += 1;
            launch_job(job, false, outbound.clone(), completion_tx.clone());
        }

        if !accepting && queues.is_empty() && running == 0 {
            break;
        }

        tokio::select! {
            biased;
            completion = completion_rx.recv(), if running > 0 => {
                if let Some(was_exclusive) = completion {
                    running -= 1;
                    exclusive_running &= !was_exclusive;
                } else {
                    break;
                }
            }
            job = receiver.recv(), if accepting => {
                match job {
                    Some(job) => queues.push(job),
                    None => accepting = false,
                }
            }
        }
    }
}

fn drain_admission_channel(
    receiver: &mut mpsc::Receiver<QueuedGitJob>,
    queues: &mut PriorityQueues,
    accepting: &mut bool,
) {
    loop {
        match receiver.try_recv() {
            Ok(job) => queues.push(job),
            Err(mpsc::error::TryRecvError::Empty) => break,
            Err(mpsc::error::TryRecvError::Disconnected) => {
                *accepting = false;
                break;
            }
        }
    }
}

fn launch_job(
    job: QueuedGitJob,
    exclusive: bool,
    outbound: mpsc::UnboundedSender<Value>,
    completion: mpsc::UnboundedSender<bool>,
) {
    tokio::spawn(async move {
        let QueuedGitJob {
            permits,
            request_id,
            task,
            ..
        } = job;
        let _guard = RunningJobGuard {
            completion,
            exclusive,
            permits,
        };
        let worker_result =
            tokio::task::spawn_blocking(move || catch_unwind(AssertUnwindSafe(task))).await;
        let response = match worker_result {
            Ok(Ok(response)) => response,
            Ok(Err(_)) => worker_failure_response(request_id, "Git request worker panicked."),
            Err(error) => worker_failure_response(
                request_id,
                &format!("Git request worker could not complete: {error}"),
            ),
        };
        let _ = outbound.send(response);
    });
}

struct RunningJobGuard {
    completion: mpsc::UnboundedSender<bool>,
    exclusive: bool,
    permits: Vec<OwnedSemaphorePermit>,
}

impl Drop for RunningJobGuard {
    fn drop(&mut self) {
        let _ = self.completion.send(self.exclusive);
        let _ = &self.permits;
    }
}

fn worker_failure_response(request_id: JsonRpcId, message: &str) -> Value {
    serde_json::to_value(error(
        Some(request_id),
        GIT_DISPATCH_UNAVAILABLE_CODE,
        message,
    ))
    .unwrap_or_else(|_| {
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": null,
            "error": {
                "code": GIT_DISPATCH_UNAVAILABLE_CODE,
                "message": "Git request worker failed."
            }
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc as std_mpsc;
    use std::time::Duration;

    fn id(value: i64) -> JsonRpcId {
        JsonRpcId::Number(value)
    }

    fn label(value: &'static str) -> Value {
        serde_json::json!({ "label": value })
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn prioritizes_queued_work_without_starving_lower_priorities() {
        let (outbound_tx, mut outbound_rx) = mpsc::unbounded_channel();
        let dispatcher = GitDispatcher::with_limits(outbound_tx, 1, 8);
        let (started_tx, started_rx) = std_mpsc::channel();
        let (release_tx, release_rx) = std_mpsc::channel();

        dispatcher
            .try_submit(GitJobPriority::Low, id(0), move || {
                started_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                label("blocker")
            })
            .unwrap();
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();

        dispatcher
            .try_submit(GitJobPriority::Low, id(1), || label("low"))
            .unwrap();
        dispatcher
            .try_submit(GitJobPriority::Medium, id(2), || label("medium"))
            .unwrap();
        dispatcher
            .try_submit(GitJobPriority::High, id(3), || label("high"))
            .unwrap();
        release_tx.send(()).unwrap();

        let mut labels = Vec::new();
        for _ in 0..4 {
            labels.push(
                outbound_rx
                    .recv()
                    .await
                    .unwrap()
                    .get("label")
                    .and_then(Value::as_str)
                    .unwrap()
                    .to_string(),
            );
        }
        dispatcher.shutdown().await.unwrap();

        assert_eq!(labels, ["blocker", "high", "medium", "low"]);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn bounds_running_and_queued_work() {
        let (outbound_tx, mut outbound_rx) = mpsc::unbounded_channel();
        let dispatcher = GitDispatcher::with_limits(outbound_tx, 1, 2);
        let (started_tx, started_rx) = std_mpsc::channel();
        let (release_tx, release_rx) = std_mpsc::channel();

        dispatcher
            .try_submit(GitJobPriority::Low, id(1), move || {
                started_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                label("running")
            })
            .unwrap();
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        dispatcher
            .try_submit(GitJobPriority::Low, id(2), || label("queued"))
            .unwrap();
        assert_eq!(
            dispatcher.try_submit(GitJobPriority::High, id(3), || label("rejected")),
            Err(GitDispatchError::Full)
        );

        release_tx.send(()).unwrap();
        assert_eq!(outbound_rx.recv().await.unwrap()["label"], "running");
        assert_eq!(outbound_rx.recv().await.unwrap()["label"], "queued");
        dispatcher.shutdown().await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn reserves_admission_capacity_for_medium_and_high_priority_work() {
        let (outbound_tx, mut outbound_rx) = mpsc::unbounded_channel();
        let dispatcher = GitDispatcher::with_capacity_reservations(outbound_tx, 1, 4, 1, 1);
        let (started_tx, started_rx) = std_mpsc::channel();
        let (release_tx, release_rx) = std_mpsc::channel();

        dispatcher
            .try_submit(GitJobPriority::Low, id(1), move || {
                started_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                label("running-low")
            })
            .unwrap();
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        dispatcher
            .try_submit(GitJobPriority::Low, id(2), || label("queued-low"))
            .unwrap();
        assert_eq!(
            dispatcher.try_submit(GitJobPriority::Low, id(3), || label("rejected-low")),
            Err(GitDispatchError::Full)
        );
        dispatcher
            .try_submit(GitJobPriority::Medium, id(4), || label("medium"))
            .unwrap();
        dispatcher
            .try_submit(GitJobPriority::High, id(5), || label("high"))
            .unwrap();

        release_tx.send(()).unwrap();
        let mut labels = Vec::new();
        for _ in 0..4 {
            labels.push(
                outbound_rx.recv().await.unwrap()["label"]
                    .as_str()
                    .unwrap()
                    .to_string(),
            );
        }
        assert_eq!(labels, ["running-low", "high", "medium", "queued-low"]);
        dispatcher.shutdown().await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn permits_out_of_order_completion_while_preserving_response_ids() {
        let (outbound_tx, mut outbound_rx) = mpsc::unbounded_channel();
        let dispatcher = GitDispatcher::with_limits(outbound_tx, 2, 4);
        let (started_tx, started_rx) = std_mpsc::channel();
        let (release_tx, release_rx) = std_mpsc::channel();

        dispatcher
            .try_submit(GitJobPriority::Low, id(1), move || {
                started_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                serde_json::json!({ "id": 1 })
            })
            .unwrap();
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        dispatcher
            .try_submit(
                GitJobPriority::Low,
                id(2),
                || serde_json::json!({ "id": 2 }),
            )
            .unwrap();

        assert_eq!(outbound_rx.recv().await.unwrap()["id"], 2);
        release_tx.send(()).unwrap();
        assert_eq!(outbound_rx.recv().await.unwrap()["id"], 1);
        dispatcher.shutdown().await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn outbound_notifications_are_not_starved_by_blocking_work() {
        let (outbound_tx, mut outbound_rx) = mpsc::unbounded_channel();
        let dispatcher = GitDispatcher::with_limits(outbound_tx.clone(), 1, 2);
        let (started_tx, started_rx) = std_mpsc::channel();
        let (release_tx, release_rx) = std_mpsc::channel();

        dispatcher
            .try_submit(GitJobPriority::Low, id(1), move || {
                started_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                label("git")
            })
            .unwrap();
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();

        outbound_tx.send(label("notification")).unwrap();
        assert_eq!(outbound_rx.try_recv().unwrap()["label"], "notification");

        release_tx.send(()).unwrap();
        dispatcher.shutdown().await.unwrap();
        assert_eq!(outbound_rx.recv().await.unwrap()["label"], "git");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn converts_worker_panics_into_an_error_with_the_original_id() {
        let (outbound_tx, mut outbound_rx) = mpsc::unbounded_channel();
        let dispatcher = GitDispatcher::with_limits(outbound_tx, 1, 2);

        dispatcher
            .try_submit(GitJobPriority::High, id(41), || panic!("test panic"))
            .unwrap();
        let response = outbound_rx.recv().await.unwrap();
        dispatcher.shutdown().await.unwrap();

        assert_eq!(response["id"], 41);
        assert_eq!(response["error"]["code"], GIT_DISPATCH_UNAVAILABLE_CODE);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn shutdown_cancels_queued_jobs_and_waits_for_running_work() {
        let (outbound_tx, mut outbound_rx) = mpsc::unbounded_channel();
        let dispatcher = GitDispatcher::with_limits(outbound_tx, 1, 3);
        let (started_tx, started_rx) = std_mpsc::channel();
        let (release_tx, release_rx) = std_mpsc::channel();

        dispatcher
            .try_submit(GitJobPriority::High, id(1), move || {
                started_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                serde_json::json!({ "id": 1, "result": "running" })
            })
            .unwrap();
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        dispatcher
            .try_submit(
                GitJobPriority::Low,
                id(2),
                || serde_json::json!({ "id": 2, "result": "must-not-run" }),
            )
            .unwrap();

        let shutdown = tokio::spawn(dispatcher.shutdown());
        let cancelled = outbound_rx.recv().await.unwrap();
        assert_eq!(cancelled["id"], 2);
        assert_eq!(cancelled["error"]["code"], GIT_DISPATCH_UNAVAILABLE_CODE);
        assert!(!shutdown.is_finished());

        release_tx.send(()).unwrap();
        let running = outbound_rx.recv().await.unwrap();
        assert_eq!(running["id"], 1);
        shutdown.await.unwrap().unwrap();
    }
}
