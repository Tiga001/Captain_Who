use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Arc;
use std::time::Duration;

use mycopilot_core::image_generation::ImageGenerationConfigurationError;
use mycopilot_protocol_rs::{
    error_with_data, ImageGenerationConfigurationErrorCodeDto,
    ImageGenerationConfigurationErrorData, ImageGenerationConfigurationErrorTypeDto,
    ImageGenerationConfigurationOperationDto, ImageGenerationConfigurationRecoveryDto, JsonRpcId,
    IMAGE_GENERATION_CONFIGURATION_ERROR_CODE,
};
use serde_json::Value;
use tokio::sync::{mpsc, oneshot, OwnedSemaphorePermit, Semaphore};
use tokio::task::JoinHandle;

const DEFAULT_MAX_IN_FLIGHT: usize = 16;
const DEFAULT_SHUTDOWN_GRACE: Duration = Duration::from_secs(2);

type ConfigurationJobTask = Box<dyn FnOnce() -> Value + Send + 'static>;
type ConfigurationWorkerTask = Box<dyn FnOnce() + Send + 'static>;

pub(crate) fn image_generation_configuration_error_response(
    id: JsonRpcId,
    operation: ImageGenerationConfigurationOperationDto,
    error: ImageGenerationConfigurationError,
) -> Value {
    let (code, recovery) = match &error {
        ImageGenerationConfigurationError::InvalidRevision
        | ImageGenerationConfigurationError::TextToImageRequired
        | ImageGenerationConfigurationError::ConfigurationIncomplete(_)
        | ImageGenerationConfigurationError::InvalidConfiguration => (
            ImageGenerationConfigurationErrorCodeDto::InvalidRequest,
            ImageGenerationConfigurationRecoveryDto::FixConfiguration,
        ),
        ImageGenerationConfigurationError::RevisionConflict { .. } => (
            ImageGenerationConfigurationErrorCodeDto::RevisionConflict,
            ImageGenerationConfigurationRecoveryDto::RefreshConfiguration,
        ),
        ImageGenerationConfigurationError::UnsupportedAdapter => (
            ImageGenerationConfigurationErrorCodeDto::UnsupportedAdapter,
            ImageGenerationConfigurationRecoveryDto::FixConfiguration,
        ),
        ImageGenerationConfigurationError::MissingEndpoint => (
            ImageGenerationConfigurationErrorCodeDto::MissingEndpoint,
            ImageGenerationConfigurationRecoveryDto::FixConfiguration,
        ),
        ImageGenerationConfigurationError::InvalidEndpoint => (
            ImageGenerationConfigurationErrorCodeDto::InvalidEndpoint,
            ImageGenerationConfigurationRecoveryDto::FixConfiguration,
        ),
        ImageGenerationConfigurationError::InsecureEndpoint => (
            ImageGenerationConfigurationErrorCodeDto::InsecureEndpoint,
            ImageGenerationConfigurationRecoveryDto::FixConfiguration,
        ),
        ImageGenerationConfigurationError::MissingModel => (
            ImageGenerationConfigurationErrorCodeDto::MissingModel,
            ImageGenerationConfigurationRecoveryDto::FixConfiguration,
        ),
        ImageGenerationConfigurationError::InvalidModelId => (
            ImageGenerationConfigurationErrorCodeDto::InvalidModelId,
            ImageGenerationConfigurationRecoveryDto::FixConfiguration,
        ),
        ImageGenerationConfigurationError::MissingCredential => (
            ImageGenerationConfigurationErrorCodeDto::MissingCredential,
            ImageGenerationConfigurationRecoveryDto::ReenterCredential,
        ),
        ImageGenerationConfigurationError::InvalidCredential => (
            ImageGenerationConfigurationErrorCodeDto::InvalidCredential,
            ImageGenerationConfigurationRecoveryDto::ReenterCredential,
        ),
        ImageGenerationConfigurationError::CredentialReplacementRequired => (
            ImageGenerationConfigurationErrorCodeDto::CredentialReplacementRequired,
            ImageGenerationConfigurationRecoveryDto::ReenterCredential,
        ),
        ImageGenerationConfigurationError::StorageUnavailable => (
            ImageGenerationConfigurationErrorCodeDto::StorageUnavailable,
            ImageGenerationConfigurationRecoveryDto::Retry,
        ),
        ImageGenerationConfigurationError::CredentialStoreUnavailable => (
            ImageGenerationConfigurationErrorCodeDto::CredentialStoreUnavailable,
            ImageGenerationConfigurationRecoveryDto::Retry,
        ),
        ImageGenerationConfigurationError::CommitIndeterminate => (
            ImageGenerationConfigurationErrorCodeDto::CommitIndeterminate,
            ImageGenerationConfigurationRecoveryDto::RefreshConfiguration,
        ),
        ImageGenerationConfigurationError::Unavailable => (
            ImageGenerationConfigurationErrorCodeDto::Unavailable,
            ImageGenerationConfigurationRecoveryDto::Retry,
        ),
    };
    let data = ImageGenerationConfigurationErrorData {
        error_type: ImageGenerationConfigurationErrorTypeDto::ImageGenerationConfiguration,
        operation,
        code,
        recovery,
        message: error.to_string(),
        current_revision: match &error {
            ImageGenerationConfigurationError::RevisionConflict { current_revision } => {
                Some(current_revision.clone())
            }
            _ => None,
        },
        retry_after_ms: None,
        configuration_may_have_changed: matches!(
            error,
            ImageGenerationConfigurationError::CommitIndeterminate
        )
        .then_some(true),
    };
    serde_json::to_value(error_with_data(
        Some(id),
        IMAGE_GENERATION_CONFIGURATION_ERROR_CODE,
        data.message.clone(),
        serde_json::to_value(data).expect("image-generation error data must serialize"),
    ))
    .expect("JSON-RPC error response must serialize")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ImageGenerationConfigurationJobKind {
    Read(ImageGenerationConfigurationOperationDto),
    Mutation(ImageGenerationConfigurationOperationDto),
}

impl ImageGenerationConfigurationJobKind {
    pub(crate) fn for_operation(operation: ImageGenerationConfigurationOperationDto) -> Self {
        match operation {
            ImageGenerationConfigurationOperationDto::GetConfiguration
            | ImageGenerationConfigurationOperationDto::GetStatus => Self::Read(operation),
            ImageGenerationConfigurationOperationDto::UpdateConfiguration
            | ImageGenerationConfigurationOperationDto::SetEnabled => Self::Mutation(operation),
        }
    }

    fn operation(self) -> ImageGenerationConfigurationOperationDto {
        match self {
            Self::Read(operation) | Self::Mutation(operation) => operation,
        }
    }

    fn is_mutation(self) -> bool {
        matches!(self, Self::Mutation(_))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ImageGenerationConfigurationDispatchError {
    Full,
    Closed,
}

#[derive(Debug)]
pub(crate) struct ImageGenerationConfigurationDispatcherShutdownError {
    message: String,
}

impl std::fmt::Display for ImageGenerationConfigurationDispatcherShutdownError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ImageGenerationConfigurationDispatcherShutdownError {}

/// Bounded, serial execution lane for SQLite + native credential-store operations.
///
/// Native credential APIs may block outside Tokio's control. Jobs therefore run on detachable OS
/// threads, while this dispatcher owns admission, mutation ambiguity, and bounded shutdown. A
/// timed-out mutation reports `commitIndeterminate`; it is never silently abandoned as a normal
/// failure or allowed to hold the Tokio runtime open indefinitely.
pub(crate) struct ImageGenerationConfigurationDispatcher {
    admission: Arc<Semaphore>,
    manager: JoinHandle<()>,
    sender: Option<mpsc::Sender<QueuedConfigurationJob>>,
    shutdown: Option<oneshot::Sender<()>>,
}

impl ImageGenerationConfigurationDispatcher {
    pub(crate) fn new(outbound: mpsc::UnboundedSender<Value>) -> Self {
        Self::with_limit_and_shutdown_grace(outbound, DEFAULT_MAX_IN_FLIGHT, DEFAULT_SHUTDOWN_GRACE)
    }

    #[cfg(test)]
    fn with_limit_and_grace(
        outbound: mpsc::UnboundedSender<Value>,
        max_in_flight: usize,
        shutdown_grace: Duration,
    ) -> Self {
        Self::with_limit_and_shutdown_grace(outbound, max_in_flight, shutdown_grace)
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
        kind: ImageGenerationConfigurationJobKind,
        task: F,
    ) -> Result<(), ImageGenerationConfigurationDispatchError>
    where
        F: FnOnce() -> Value + Send + 'static,
    {
        let Some(sender) = self.sender.as_ref() else {
            return Err(ImageGenerationConfigurationDispatchError::Closed);
        };
        let permit = Arc::clone(&self.admission)
            .try_acquire_owned()
            .map_err(|_| ImageGenerationConfigurationDispatchError::Full)?;
        sender
            .try_send(QueuedConfigurationJob {
                _permit: permit,
                request_id,
                kind,
                task: Box::new(task),
            })
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => {
                    ImageGenerationConfigurationDispatchError::Full
                }
                mpsc::error::TrySendError::Closed(_) => {
                    ImageGenerationConfigurationDispatchError::Closed
                }
            })
    }

    pub(crate) async fn shutdown(
        mut self,
    ) -> Result<(), ImageGenerationConfigurationDispatcherShutdownError> {
        self.sender.take();
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        self.manager.await.map_err(
            |error| ImageGenerationConfigurationDispatcherShutdownError {
                message: format!(
                    "image-generation configuration dispatcher stopped unexpectedly: {error}"
                ),
            },
        )
    }
}

struct QueuedConfigurationJob {
    _permit: OwnedSemaphorePermit,
    request_id: JsonRpcId,
    kind: ImageGenerationConfigurationJobKind,
    task: ConfigurationJobTask,
}

struct StartedConfigurationJob {
    _permit: OwnedSemaphorePermit,
    request_id: JsonRpcId,
    kind: ImageGenerationConfigurationJobKind,
    completion: oneshot::Receiver<std::thread::Result<Value>>,
    worker: std::thread::JoinHandle<()>,
}

async fn run_dispatcher(
    mut receiver: mpsc::Receiver<QueuedConfigurationJob>,
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

        let request_id = job.request_id.clone();
        let kind = job.kind;
        let started = match start_job(job) {
            Ok(started) => started,
            Err(response) => {
                let _ = outbound.send(response);
                continue;
            }
        };
        let mut running = Box::pin(finish_started_job(started));
        let (response, shutdown_requested) = tokio::select! {
            biased;
            _ = &mut shutdown => {
                cancel_queued_jobs(&mut receiver, &outbound);
                let response = match tokio::time::timeout(shutdown_grace, &mut running).await {
                    Ok(response) => response,
                    Err(_) => interrupted_response(request_id, kind),
                };
                (response, true)
            }
            response = &mut running => (response, false),
        };
        let _ = outbound.send(response);
        if shutdown_requested {
            cancel_queued_jobs(&mut receiver, &outbound);
            return;
        }
    }
}

fn cancel_queued_jobs(
    receiver: &mut mpsc::Receiver<QueuedConfigurationJob>,
    outbound: &mpsc::UnboundedSender<Value>,
) {
    receiver.close();
    while let Ok(job) = receiver.try_recv() {
        let _ = outbound.send(image_generation_configuration_error_response(
            job.request_id,
            job.kind.operation(),
            ImageGenerationConfigurationError::Unavailable,
        ));
    }
}

fn start_job(job: QueuedConfigurationJob) -> Result<StartedConfigurationJob, Value> {
    let QueuedConfigurationJob {
        _permit: permit,
        request_id,
        kind,
        task,
    } = job;
    let (completion_tx, completion_rx) = oneshot::channel();
    let worker_task: ConfigurationWorkerTask = Box::new(move || {
        let _ = completion_tx.send(catch_unwind(AssertUnwindSafe(task)));
    });
    let worker = std::thread::Builder::new()
        .name("image-configuration".to_string())
        .spawn(worker_task)
        .map_err(|_| {
            image_generation_configuration_error_response(
                request_id.clone(),
                kind.operation(),
                ImageGenerationConfigurationError::Unavailable,
            )
        })?;
    Ok(StartedConfigurationJob {
        _permit: permit,
        request_id,
        kind,
        completion: completion_rx,
        worker,
    })
}

async fn finish_started_job(job: StartedConfigurationJob) -> Value {
    let StartedConfigurationJob {
        _permit,
        request_id,
        kind,
        completion,
        worker,
    } = job;
    let worker_result = completion.await;
    let _ = worker.join();
    match worker_result {
        Ok(Ok(response)) => response,
        Ok(Err(_)) | Err(_) => interrupted_response(request_id, kind),
    }
}

fn interrupted_response(request_id: JsonRpcId, kind: ImageGenerationConfigurationJobKind) -> Value {
    let error = if kind.is_mutation() {
        ImageGenerationConfigurationError::CommitIndeterminate
    } else {
        ImageGenerationConfigurationError::Unavailable
    };
    image_generation_configuration_error_response(request_id, kind.operation(), error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc as std_mpsc;

    fn id(value: i64) -> JsonRpcId {
        JsonRpcId::Number(value)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn completed_jobs_preserve_the_original_response_identity() {
        let (outbound_tx, mut outbound_rx) = mpsc::unbounded_channel();
        let dispatcher = ImageGenerationConfigurationDispatcher::with_limit_and_grace(
            outbound_tx,
            2,
            Duration::from_millis(50),
        );
        dispatcher
            .try_submit(
                id(1),
                ImageGenerationConfigurationJobKind::Read(
                    ImageGenerationConfigurationOperationDto::GetStatus,
                ),
                || serde_json::json!({ "id": 1, "result": { "ready": true } }),
            )
            .unwrap();

        assert_eq!(outbound_rx.recv().await.unwrap()["id"], 1);
        dispatcher.shutdown().await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn shutdown_cancels_queued_reads_and_marks_a_running_mutation_indeterminate() {
        let (outbound_tx, mut outbound_rx) = mpsc::unbounded_channel();
        let dispatcher = ImageGenerationConfigurationDispatcher::with_limit_and_grace(
            outbound_tx,
            2,
            Duration::from_millis(10),
        );
        let (started_tx, started_rx) = std_mpsc::channel();
        let (release_tx, release_rx) = std_mpsc::channel();
        dispatcher
            .try_submit(
                id(1),
                ImageGenerationConfigurationJobKind::Mutation(
                    ImageGenerationConfigurationOperationDto::UpdateConfiguration,
                ),
                move || {
                    started_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    serde_json::json!({ "id": 1, "result": {} })
                },
            )
            .unwrap();
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        dispatcher
            .try_submit(
                id(2),
                ImageGenerationConfigurationJobKind::Read(
                    ImageGenerationConfigurationOperationDto::GetConfiguration,
                ),
                || serde_json::json!({ "id": 2, "result": {} }),
            )
            .unwrap();

        dispatcher.shutdown().await.unwrap();
        let first = outbound_rx.recv().await.unwrap();
        let second = outbound_rx.recv().await.unwrap();
        let responses = [first, second];
        let queued = responses.iter().find(|value| value["id"] == 2).unwrap();
        let running = responses.iter().find(|value| value["id"] == 1).unwrap();
        assert_eq!(queued["error"]["data"]["code"], "unavailable");
        assert_eq!(running["error"]["data"]["code"], "commitIndeterminate");
        assert_eq!(
            running["error"]["data"]["configurationMayHaveChanged"],
            true
        );
        release_tx.send(()).unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn worker_panics_never_become_false_successes() {
        let (outbound_tx, mut outbound_rx) = mpsc::unbounded_channel();
        let dispatcher = ImageGenerationConfigurationDispatcher::with_limit_and_grace(
            outbound_tx,
            2,
            Duration::from_millis(50),
        );
        dispatcher
            .try_submit(
                id(1),
                ImageGenerationConfigurationJobKind::Read(
                    ImageGenerationConfigurationOperationDto::GetConfiguration,
                ),
                || panic!("injected read panic"),
            )
            .unwrap();
        dispatcher
            .try_submit(
                id(2),
                ImageGenerationConfigurationJobKind::Mutation(
                    ImageGenerationConfigurationOperationDto::SetEnabled,
                ),
                || panic!("injected mutation panic"),
            )
            .unwrap();

        let read = outbound_rx.recv().await.unwrap();
        let mutation = outbound_rx.recv().await.unwrap();
        assert_eq!(read["id"], 1);
        assert_eq!(read["error"]["data"]["code"], "unavailable");
        assert_eq!(mutation["id"], 2);
        assert_eq!(mutation["error"]["data"]["code"], "commitIndeterminate");
        assert_eq!(
            mutation["error"]["data"]["configurationMayHaveChanged"],
            true
        );
        dispatcher.shutdown().await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn admission_is_bounded_across_running_and_queued_work() {
        let (outbound_tx, mut outbound_rx) = mpsc::unbounded_channel();
        let dispatcher = ImageGenerationConfigurationDispatcher::with_limit_and_grace(
            outbound_tx,
            1,
            Duration::from_millis(50),
        );
        let (started_tx, started_rx) = std_mpsc::channel();
        let (release_tx, release_rx) = std_mpsc::channel();
        dispatcher
            .try_submit(
                id(1),
                ImageGenerationConfigurationJobKind::Read(
                    ImageGenerationConfigurationOperationDto::GetStatus,
                ),
                move || {
                    started_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    serde_json::json!({ "id": 1, "result": {} })
                },
            )
            .unwrap();
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();

        assert_eq!(
            dispatcher.try_submit(
                id(2),
                ImageGenerationConfigurationJobKind::Read(
                    ImageGenerationConfigurationOperationDto::GetStatus,
                ),
                || serde_json::json!({ "id": 2, "result": {} }),
            ),
            Err(ImageGenerationConfigurationDispatchError::Full)
        );

        release_tx.send(()).unwrap();
        assert_eq!(outbound_rx.recv().await.unwrap()["id"], 1);
        dispatcher.shutdown().await.unwrap();
    }
}
