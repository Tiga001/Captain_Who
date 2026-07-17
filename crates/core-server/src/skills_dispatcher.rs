use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Arc;
use std::time::Duration;

use mycopilot_protocol_rs::{error, JsonRpcId};
use serde_json::Value;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::task::JoinHandle;

pub(crate) const DEFAULT_SKILL_MAX_IN_FLIGHT: usize = 16;
const DEFAULT_SKILL_SHUTDOWN_GRACE: Duration = Duration::from_secs(2);

const SKILL_DISPATCH_QUEUE_FULL_CODE: i64 = -32002;
const SKILL_DISPATCH_UNAVAILABLE_CODE: i64 = -32603;

const SKILL_DISPATCH_QUEUE_FULL_MESSAGE: &str =
    "The skill discovery queue is full. Please retry shortly.";
const SKILL_DISPATCH_CLOSED_MESSAGE: &str = "The skill discovery service is shutting down.";
const SKILL_DISPATCH_CANCELLED_MESSAGE: &str =
    "Skill discovery request was cancelled because the core server is shutting down.";
const SKILL_DISPATCH_PANICKED_MESSAGE: &str = "Skill discovery request worker panicked.";
const SKILL_DISPATCH_TIMED_OUT_MESSAGE: &str =
    "Skill discovery request did not finish before the core server shut down.";

type SkillJobTask = Box<dyn FnOnce() -> Value + Send + 'static>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SkillsDispatchError {
    Full,
    Closed,
}

impl SkillsDispatchError {
    pub(crate) fn code(self) -> i64 {
        match self {
            Self::Full => SKILL_DISPATCH_QUEUE_FULL_CODE,
            Self::Closed => SKILL_DISPATCH_UNAVAILABLE_CODE,
        }
    }

    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::Full => SKILL_DISPATCH_QUEUE_FULL_MESSAGE,
            Self::Closed => SKILL_DISPATCH_CLOSED_MESSAGE,
        }
    }
}

#[derive(Debug)]
pub(crate) struct SkillsDispatcherShutdownError {
    message: String,
}

impl std::fmt::Display for SkillsDispatcherShutdownError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for SkillsDispatcherShutdownError {}

/// Runs bounded, read-only skill discovery work away from the async control plane.
///
/// A single serial lane intentionally limits filesystem scans. Each accepted scan runs on a
/// detachable OS thread so an unresponsive filesystem cannot hold Tokio runtime shutdown. The semaphore limits
/// running and queued work together; the Tokio channel alone would only bound work
/// that has not yet been received by the dispatcher task.
pub(crate) struct SkillsDispatcher {
    admission: Arc<Semaphore>,
    manager: JoinHandle<()>,
    sender: Option<mpsc::Sender<QueuedSkillJob>>,
    shutdown: Option<oneshot::Sender<()>>,
}

impl SkillsDispatcher {
    pub(crate) fn new(outbound: mpsc::UnboundedSender<Value>) -> Self {
        Self::with_limit_and_shutdown_grace(
            outbound,
            DEFAULT_SKILL_MAX_IN_FLIGHT,
            DEFAULT_SKILL_SHUTDOWN_GRACE,
        )
    }

    #[cfg(test)]
    fn with_limit(outbound: mpsc::UnboundedSender<Value>, max_in_flight: usize) -> Self {
        Self::with_limit_and_shutdown_grace(outbound, max_in_flight, DEFAULT_SKILL_SHUTDOWN_GRACE)
    }

    fn with_limit_and_shutdown_grace(
        outbound: mpsc::UnboundedSender<Value>,
        max_in_flight: usize,
        shutdown_grace: Duration,
    ) -> Self {
        let max_in_flight = max_in_flight.max(1);
        let admission = Arc::new(Semaphore::new(max_in_flight));
        let (sender, receiver) = mpsc::channel(max_in_flight);
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let manager = tokio::spawn(run_dispatcher(
            receiver,
            outbound,
            shutdown_rx,
            shutdown_grace,
        ));
        Self {
            admission,
            manager,
            sender: Some(sender),
            shutdown: Some(shutdown_tx),
        }
    }

    pub(crate) fn try_submit<F>(
        &self,
        request_id: JsonRpcId,
        task: F,
    ) -> Result<(), SkillsDispatchError>
    where
        F: FnOnce() -> Value + Send + 'static,
    {
        let Some(sender) = self.sender.as_ref() else {
            return Err(SkillsDispatchError::Closed);
        };
        let permit = Arc::clone(&self.admission)
            .try_acquire_owned()
            .map_err(|_| SkillsDispatchError::Full)?;
        let job = QueuedSkillJob {
            permit,
            request_id,
            task: Box::new(task),
        };
        sender.try_send(job).map_err(|error| match error {
            mpsc::error::TrySendError::Full(_) => SkillsDispatchError::Full,
            mpsc::error::TrySendError::Closed(_) => SkillsDispatchError::Closed,
        })
    }

    /// Stops admission, cancels queued requests, and gives the running scan a bounded grace period.
    pub(crate) async fn shutdown(mut self) -> Result<(), SkillsDispatcherShutdownError> {
        self.sender.take();
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        self.manager
            .await
            .map_err(|error| SkillsDispatcherShutdownError {
                message: format!("Skill dispatcher stopped unexpectedly: {error}"),
            })
    }
}

struct QueuedSkillJob {
    permit: OwnedSemaphorePermit,
    request_id: JsonRpcId,
    task: SkillJobTask,
}

async fn run_dispatcher(
    mut receiver: mpsc::Receiver<QueuedSkillJob>,
    outbound: mpsc::UnboundedSender<Value>,
    mut shutdown: oneshot::Receiver<()>,
    shutdown_grace: Duration,
) {
    loop {
        let job = tokio::select! {
            biased;
            _ = &mut shutdown => {
                cancel_queued_jobs(&mut receiver, &outbound);
                return;
            }
            job = receiver.recv() => {
                let Some(job) = job else {
                    return;
                };
                job
            }
        };

        let running_request_id = job.request_id.clone();
        let mut running = Box::pin(execute_job(job));
        let response = tokio::select! {
            biased;
            _ = &mut shutdown => {
                cancel_queued_jobs(&mut receiver, &outbound);
                let response = match tokio::time::timeout(shutdown_grace, &mut running).await {
                    Ok(response) => response,
                    Err(_) => worker_failure_response(
                        running_request_id,
                        SKILL_DISPATCH_TIMED_OUT_MESSAGE,
                    ),
                };
                let _ = outbound.send(response);
                return;
            }
            response = &mut running => response,
        };
        let _ = outbound.send(response);
    }
}

fn cancel_queued_jobs(
    receiver: &mut mpsc::Receiver<QueuedSkillJob>,
    outbound: &mpsc::UnboundedSender<Value>,
) {
    receiver.close();
    while let Ok(job) = receiver.try_recv() {
        let _ = outbound.send(worker_failure_response(
            job.request_id,
            SKILL_DISPATCH_CANCELLED_MESSAGE,
        ));
        // Dropping the job releases its admission permit and task closure.
    }
}

async fn execute_job(job: QueuedSkillJob) -> Value {
    let QueuedSkillJob {
        permit,
        request_id,
        task,
    } = job;
    let _permit = permit;
    let (completion_tx, completion_rx) = oneshot::channel();
    let worker = std::thread::Builder::new()
        .name("skill-discovery".to_string())
        .spawn(move || {
            let _ = completion_tx.send(catch_unwind(AssertUnwindSafe(task)));
        });
    let worker = match worker {
        Ok(worker) => worker,
        Err(error) => {
            return worker_failure_response(
                request_id,
                &format!("Skill discovery request worker could not start: {error}"),
            );
        }
    };
    let worker_result = completion_rx.await;
    drop(worker);
    match worker_result {
        Ok(Ok(response)) => response,
        Ok(Err(_)) => worker_failure_response(request_id, SKILL_DISPATCH_PANICKED_MESSAGE),
        Err(error) => worker_failure_response(
            request_id,
            &format!("Skill discovery request worker stopped without a response: {error}"),
        ),
    }
}

fn worker_failure_response(request_id: JsonRpcId, message: &str) -> Value {
    let fallback_id = match &request_id {
        JsonRpcId::String(value) => Value::String(value.clone()),
        JsonRpcId::Number(value) => Value::Number((*value).into()),
    };
    serde_json::to_value(error(
        Some(request_id),
        SKILL_DISPATCH_UNAVAILABLE_CODE,
        message,
    ))
    .unwrap_or_else(|_| {
        let mut error = serde_json::Map::new();
        error.insert(
            "code".to_string(),
            Value::Number(SKILL_DISPATCH_UNAVAILABLE_CODE.into()),
        );
        error.insert(
            "message".to_string(),
            Value::String("Skill discovery request worker failed.".to_string()),
        );

        let mut response = serde_json::Map::new();
        response.insert("jsonrpc".to_string(), Value::String("2.0".to_string()));
        response.insert("id".to_string(), fallback_id);
        response.insert("error".to_string(), Value::Object(error));
        Value::Object(response)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc as std_mpsc;
    use std::time::Duration;

    fn id(value: i64) -> JsonRpcId {
        JsonRpcId::Number(value)
    }

    fn label(value: &'static str) -> Value {
        serde_json::json!({ "label": value })
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn bounds_running_and_queued_work_together() {
        let (outbound_tx, mut outbound_rx) = mpsc::unbounded_channel();
        let dispatcher = SkillsDispatcher::with_limit(outbound_tx, 2);
        let (started_tx, started_rx) = std_mpsc::channel();
        let (release_tx, release_rx) = std_mpsc::channel();

        dispatcher
            .try_submit(id(1), move || {
                started_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                label("running")
            })
            .unwrap();
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        dispatcher.try_submit(id(2), || label("queued")).unwrap();
        assert_eq!(
            dispatcher.try_submit(id(3), || label("rejected")),
            Err(SkillsDispatchError::Full)
        );
        assert_eq!(
            SkillsDispatchError::Full.code(),
            SKILL_DISPATCH_QUEUE_FULL_CODE
        );
        assert_eq!(
            SkillsDispatchError::Full.message(),
            SKILL_DISPATCH_QUEUE_FULL_MESSAGE
        );
        assert_eq!(
            SkillsDispatchError::Closed.code(),
            SKILL_DISPATCH_UNAVAILABLE_CODE
        );
        assert_eq!(
            SkillsDispatchError::Closed.message(),
            SKILL_DISPATCH_CLOSED_MESSAGE
        );

        release_tx.send(()).unwrap();
        assert_eq!(outbound_rx.recv().await.unwrap()["label"], "running");
        assert_eq!(outbound_rx.recv().await.unwrap()["label"], "queued");
        dispatcher.shutdown().await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn converts_worker_panics_into_an_error_with_the_original_id() {
        let (outbound_tx, mut outbound_rx) = mpsc::unbounded_channel();
        let dispatcher = SkillsDispatcher::with_limit(outbound_tx, 2);

        dispatcher
            .try_submit(JsonRpcId::String("skill-41".to_string()), || {
                panic!("test panic")
            })
            .unwrap();
        let response = outbound_rx.recv().await.unwrap();
        dispatcher.shutdown().await.unwrap();

        assert_eq!(response["id"], "skill-41");
        assert_eq!(response["error"]["code"], SKILL_DISPATCH_UNAVAILABLE_CODE);
        assert_eq!(
            response["error"]["message"],
            SKILL_DISPATCH_PANICKED_MESSAGE
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn shutdown_cancels_queued_jobs_and_waits_for_running_work() {
        let (outbound_tx, mut outbound_rx) = mpsc::unbounded_channel();
        let dispatcher = SkillsDispatcher::with_limit(outbound_tx, 3);
        let queued_ran = Arc::new(AtomicBool::new(false));
        let (started_tx, started_rx) = std_mpsc::channel();
        let (release_tx, release_rx) = std_mpsc::channel();

        dispatcher
            .try_submit(id(1), move || {
                started_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                serde_json::json!({ "id": 1, "result": "running" })
            })
            .unwrap();
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let queued_ran_from_task = Arc::clone(&queued_ran);
        dispatcher
            .try_submit(id(2), move || {
                queued_ran_from_task.store(true, Ordering::Release);
                serde_json::json!({ "id": 2, "result": "must-not-run" })
            })
            .unwrap();

        let shutdown = tokio::spawn(dispatcher.shutdown());
        let cancelled = outbound_rx.recv().await.unwrap();
        assert_eq!(cancelled["id"], 2);
        assert_eq!(cancelled["error"]["code"], SKILL_DISPATCH_UNAVAILABLE_CODE);
        assert_eq!(
            cancelled["error"]["message"],
            SKILL_DISPATCH_CANCELLED_MESSAGE
        );
        assert!(!queued_ran.load(Ordering::Acquire));
        assert!(!shutdown.is_finished());

        release_tx.send(()).unwrap();
        let running = outbound_rx.recv().await.unwrap();
        assert_eq!(running["id"], 1);
        shutdown.await.unwrap().unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn shutdown_has_a_deadline_for_an_unresponsive_filesystem_worker() {
        let (outbound_tx, mut outbound_rx) = mpsc::unbounded_channel();
        let dispatcher = SkillsDispatcher::with_limit_and_shutdown_grace(
            outbound_tx,
            2,
            Duration::from_millis(25),
        );
        let (started_tx, started_rx) = std_mpsc::channel();
        let (release_tx, release_rx) = std_mpsc::channel();
        dispatcher
            .try_submit(id(9), move || {
                started_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                label("late")
            })
            .unwrap();
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();

        tokio::time::timeout(Duration::from_secs(1), dispatcher.shutdown())
            .await
            .expect("dispatcher shutdown must respect its deadline")
            .unwrap();
        let response = outbound_rx.recv().await.unwrap();

        assert_eq!(response["id"], 9);
        assert_eq!(response["error"]["code"], SKILL_DISPATCH_UNAVAILABLE_CODE);
        assert_eq!(
            response["error"]["message"],
            SKILL_DISPATCH_TIMED_OUT_MESSAGE
        );
        release_tx.send(()).unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn blocking_skill_work_does_not_block_outbound_messages() {
        let (outbound_tx, mut outbound_rx) = mpsc::unbounded_channel();
        let dispatcher = SkillsDispatcher::with_limit(outbound_tx.clone(), 2);
        let (started_tx, started_rx) = std_mpsc::channel();
        let (release_tx, release_rx) = std_mpsc::channel();

        dispatcher
            .try_submit(id(1), move || {
                started_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                label("skill")
            })
            .unwrap();
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();

        outbound_tx.send(label("notification")).unwrap();
        assert_eq!(outbound_rx.try_recv().unwrap()["label"], "notification");

        release_tx.send(()).unwrap();
        dispatcher.shutdown().await.unwrap();
        assert_eq!(outbound_rx.recv().await.unwrap()["label"], "skill");
    }
}
