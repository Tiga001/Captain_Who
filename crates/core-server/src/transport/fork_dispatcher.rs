use super::*;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::OwnedSemaphorePermit;
use tokio::task::JoinHandle;

const MAX_IN_FLIGHT_FORKS: usize = 4;

struct ForkJob {
    request_id: JsonRpcId,
    task: Box<dyn FnOnce() -> Value + Send>,
    _permit: OwnedSemaphorePermit,
}

/// Forks keep their existing transactional/idempotent storage contract, but never run on the
/// stdio control plane. One worker bounds database/attachment pressure; admission counts both
/// queued and running work. Shutdown cancels only jobs that have not started and joins the
/// running transaction before the outbound writer closes (a timeout cannot mean rollback).
pub(crate) struct ForkRequestDispatcher {
    admission: Arc<Semaphore>,
    sender: mpsc::Sender<ForkJob>,
    stopping: Arc<AtomicBool>,
    manager: JoinHandle<()>,
}

impl ForkRequestDispatcher {
    pub(crate) fn new(outbound: mpsc::UnboundedSender<Value>) -> Self {
        let admission = Arc::new(Semaphore::new(MAX_IN_FLIGHT_FORKS));
        let stopping = Arc::new(AtomicBool::new(false));
        let worker_stopping = Arc::clone(&stopping);
        let (sender, mut receiver) = mpsc::channel::<ForkJob>(MAX_IN_FLIGHT_FORKS);
        let manager = tokio::spawn(async move {
            while let Some(job) = receiver.recv().await {
                let request_id = job.request_id.clone();
                let response = if worker_stopping.load(Ordering::Acquire) {
                    fork_unavailable_response(request_id, false)
                } else {
                    match tokio::task::spawn_blocking(job.task).await {
                        Ok(response) => response,
                        Err(_) => response_error(
                            Some(request_id),
                            -32603,
                            "Conversation fork worker failed. Retry the same request.",
                        ),
                    }
                };
                let _ = enqueue_outbound(&outbound, response);
            }
        });
        Self {
            admission,
            sender,
            stopping,
            manager,
        }
    }

    pub(crate) fn try_submit<F>(&self, request_id: JsonRpcId, task: F) -> Result<(), Value>
    where
        F: FnOnce() -> Value + Send + 'static,
    {
        if self.stopping.load(Ordering::Acquire) {
            return Err(fork_unavailable_response(request_id, false));
        }
        let permit = Arc::clone(&self.admission)
            .try_acquire_owned()
            .map_err(|_| fork_unavailable_response(request_id.clone(), true))?;
        self.sender
            .try_send(ForkJob {
                request_id: request_id.clone(),
                task: Box::new(task),
                _permit: permit,
            })
            .map_err(|error| {
                fork_unavailable_response(
                    request_id,
                    matches!(error, mpsc::error::TrySendError::Full(_)),
                )
            })
    }

    /// Close admission and queued-work execution at the shutdown request boundary, before other
    /// services perform asynchronous shutdown handshakes. The already-running fork keeps ownership.
    pub(crate) fn begin_shutdown(&self) {
        self.stopping.store(true, Ordering::Release);
    }

    pub(crate) async fn shutdown(self) -> io::Result<()> {
        self.begin_shutdown();
        drop(self.sender);
        self.manager
            .await
            .map_err(|error| io::Error::other(format!("fork dispatcher stopped: {error}")))
    }
}

fn fork_unavailable_response(request_id: JsonRpcId, busy: bool) -> Value {
    response_error(
        Some(request_id),
        if busy { -32001 } else { -32603 },
        if busy {
            "The conversation fork queue is full. Retry the same request shortly."
        } else {
            "Conversation fork was not started because the core server is shutting down."
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc as std_mpsc;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn admission_is_bounded_and_shutdown_finishes_only_the_running_fork() {
        let (outbound, mut responses) = mpsc::unbounded_channel();
        let dispatcher = ForkRequestDispatcher::new(outbound);
        let (started_tx, started_rx) = oneshot::channel();
        let (release_tx, release_rx) = std_mpsc::channel();
        dispatcher
            .try_submit(JsonRpcId::Number(1), move || {
                let _ = started_tx.send(());
                release_rx.recv().unwrap();
                response_success(JsonRpcId::Number(1), json!({"committed": true}))
            })
            .unwrap();
        started_rx.await.unwrap();
        for id in 2..=MAX_IN_FLIGHT_FORKS {
            dispatcher
                .try_submit(JsonRpcId::Number(id as i64), || {
                    panic!("shutdown must not start a queued fork")
                })
                .unwrap();
        }
        let rejected = dispatcher
            .try_submit(JsonRpcId::Number(99), || unreachable!())
            .unwrap_err();
        assert_eq!(rejected["error"]["code"], -32001);
        let stopping = Arc::clone(&dispatcher.stopping);
        let shutdown = tokio::spawn(dispatcher.shutdown());
        while !stopping.load(Ordering::Acquire) {
            tokio::task::yield_now().await;
        }
        assert!(
            !shutdown.is_finished(),
            "running fork must retain its owner"
        );
        release_tx.send(()).unwrap();
        shutdown.await.unwrap().unwrap();
        let completed = responses.recv().await.unwrap();
        assert_eq!(completed["result"]["committed"], true);
        for id in 2..=MAX_IN_FLIGHT_FORKS {
            let cancelled = responses.recv().await.unwrap();
            assert_eq!(cancelled["id"], id);
            assert_eq!(cancelled["error"]["code"], -32603);
        }
    }
}
