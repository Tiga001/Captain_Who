use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Arc;
use std::time::Duration;

use mycopilot_core::skills::{SkillId, SkillInstallationId, SkillInstallationOperation};
use mycopilot_protocol_rs::{
    error, error_with_data, JsonRpcId, SkillInspectionErrorCodeDto, SkillInspectionErrorData,
    SkillInspectionErrorTypeDto, SkillInspectionPhaseDto, SkillInspectionRecoveryDto,
    SkillInstallationErrorCodeDto, SkillInstallationErrorData, SkillInstallationErrorTypeDto,
    SkillInstallationOperationDto, SkillInstallationRecoveryDto, SKILL_INSPECTION_ERROR_CODE,
    SKILL_INSTALLATION_ERROR_CODE,
};
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
    "The Skill filesystem queue is full. Please retry shortly.";
const SKILL_DISPATCH_CLOSED_MESSAGE: &str = "The Skill filesystem service is shutting down.";
const SKILL_DISPATCH_CANCELLED_MESSAGE: &str =
    "Skill filesystem request was cancelled because the core server is shutting down.";
const SKILL_DISPATCH_PANICKED_MESSAGE: &str = "Skill filesystem request worker panicked.";
const SKILL_DISPATCH_TIMED_OUT_MESSAGE: &str =
    "Skill filesystem request did not finish before the core server shut down.";
const SKILL_DISPATCH_WORKER_UNAVAILABLE_MESSAGE: &str =
    "Skill filesystem request worker could not start.";
const SKILL_MUTATION_CANCELLED_MESSAGE: &str =
    "Skill mutation was cancelled before it started because the core server is shutting down.";
const SKILL_MUTATION_UNAVAILABLE_MESSAGE: &str =
    "Skill mutation could not start. Retry the same request.";
const SKILL_MUTATION_INDETERMINATE_MESSAGE: &str =
    "Skill mutation did not finish before shutdown; its commit state is indeterminate.";
const SKILL_WORKFLOW_COMMIT_CANCELLED_MESSAGE: &str =
    "Skill commit was cancelled before it started because the core server is shutting down.";
const SKILL_WORKFLOW_COMMIT_UNAVAILABLE_MESSAGE: &str =
    "Skill commit could not start. Retry the same preparation.";
const SKILL_WORKFLOW_COMMIT_INDETERMINATE_MESSAGE: &str =
    "Skill commit did not finish; refresh the Skill list because it may have completed.";

type SkillJobTask = Box<dyn FnOnce() -> Value + Send + 'static>;
type SkillWorkerTask = Box<dyn FnOnce() + Send + 'static>;

#[derive(Debug, Clone, PartialEq, Eq)]
enum SkillJobKind {
    Read,
    WorkflowCommit { preparation_id: String },
    Mutation(SkillMutationJob),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SkillMutationJob {
    operation: SkillInstallationOperation,
    target: SkillMutationTarget,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SkillMutationTarget {
    InstallationId(SkillInstallationId),
    SkillId(SkillId),
}

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

pub(crate) fn mutation_admission_error_response(
    request_id: JsonRpcId,
    operation: SkillInstallationOperation,
    target: SkillMutationTarget,
    error: SkillsDispatchError,
) -> Value {
    mutation_error_response(
        request_id,
        &SkillMutationJob { operation, target },
        false,
        SkillInstallationErrorCodeDto::Unavailable,
        error.message(),
    )
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

/// Runs bounded Skill filesystem work away from the async control plane.
///
/// A single serial lane intentionally limits filesystem scans and mutations. Each accepted job
/// runs on a detachable OS thread so an unresponsive filesystem cannot hold Tokio runtime
/// shutdown. The semaphore limits running and queued work together; the Tokio channel alone would
/// only bound work that has not yet been received by the dispatcher task.
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
        self.try_submit_job(request_id, SkillJobKind::Read, task)
    }

    /// Submits a mutating filesystem job with enough non-sensitive metadata to
    /// report an indeterminate commit if shutdown outlives its grace period.
    pub(crate) fn try_submit_mutation<F>(
        &self,
        request_id: JsonRpcId,
        operation: SkillInstallationOperation,
        target: SkillMutationTarget,
        task: F,
    ) -> Result<(), SkillsDispatchError>
    where
        F: FnOnce() -> Value + Send + 'static,
    {
        self.try_submit_job(
            request_id,
            SkillJobKind::Mutation(SkillMutationJob { operation, target }),
            task,
        )
    }

    /// Submits a two-phase workflow commit. The preparation identity is enough
    /// to retry safely, while the concrete install/update target remains
    /// intentionally encapsulated in the workflow session.
    pub(crate) fn try_submit_workflow_commit<F>(
        &self,
        request_id: JsonRpcId,
        preparation_id: String,
        task: F,
    ) -> Result<(), SkillsDispatchError>
    where
        F: FnOnce() -> Value + Send + 'static,
    {
        self.try_submit_job(
            request_id,
            SkillJobKind::WorkflowCommit { preparation_id },
            task,
        )
    }

    fn try_submit_job<F>(
        &self,
        request_id: JsonRpcId,
        kind: SkillJobKind,
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
            kind,
            task: Box::new(task),
        };
        sender.try_send(job).map_err(|error| match error {
            mpsc::error::TrySendError::Full(_) => SkillsDispatchError::Full,
            mpsc::error::TrySendError::Closed(_) => SkillsDispatchError::Closed,
        })
    }

    /// Stops admission, cancels queued requests, and gives the running filesystem job a bounded
    /// grace period.
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
    kind: SkillJobKind,
    task: SkillJobTask,
}

struct StartedSkillJob {
    _permit: OwnedSemaphorePermit,
    request_id: JsonRpcId,
    kind: SkillJobKind,
    completion: oneshot::Receiver<std::thread::Result<Value>>,
    worker: std::thread::JoinHandle<()>,
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
        let running_kind = job.kind.clone();
        // Starting is deliberately synchronous: after a job leaves the queue it is either
        // running or has a start-failure response. No shutdown await-point may exist between
        // dequeue and worker admission, otherwise shutdown could start a mutation merely by
        // polling its cleanup path.
        let started = match start_job(job) {
            Ok(started) => started,
            Err(response) => {
                let _ = outbound.send(response);
                continue;
            }
        };
        let mut running = Box::pin(finish_started_job(started));
        let response = tokio::select! {
            biased;
            _ = &mut shutdown => {
                cancel_queued_jobs(&mut receiver, &outbound);
                let response = match tokio::time::timeout(shutdown_grace, &mut running).await {
                    Ok(response) => response,
                    Err(_) => running_timeout_response(running_request_id, &running_kind),
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
        let response = match &job.kind {
            SkillJobKind::Read => {
                worker_failure_response(job.request_id.clone(), SKILL_DISPATCH_CANCELLED_MESSAGE)
            }
            SkillJobKind::WorkflowCommit { preparation_id } => workflow_commit_error_response(
                job.request_id.clone(),
                preparation_id,
                false,
                SkillInspectionErrorCodeDto::Cancelled,
                SkillInspectionRecoveryDto::RetrySamePreparation,
                SKILL_WORKFLOW_COMMIT_CANCELLED_MESSAGE,
            ),
            SkillJobKind::Mutation(mutation) => mutation_error_response(
                job.request_id.clone(),
                mutation,
                false,
                SkillInstallationErrorCodeDto::Cancelled,
                SKILL_MUTATION_CANCELLED_MESSAGE,
            ),
        };
        let _ = outbound.send(response);
        // Dropping the job releases its admission permit and task closure.
    }
}

fn start_job(job: QueuedSkillJob) -> Result<StartedSkillJob, Value> {
    start_job_with_spawner(job, |worker| {
        std::thread::Builder::new()
            .name("skill-filesystem".to_string())
            .spawn(worker)
    })
}

fn start_job_with_spawner<S>(job: QueuedSkillJob, spawn: S) -> Result<StartedSkillJob, Value>
where
    S: FnOnce(SkillWorkerTask) -> std::io::Result<std::thread::JoinHandle<()>>,
{
    let QueuedSkillJob {
        permit,
        request_id,
        kind,
        task,
    } = job;
    let (completion_tx, completion_rx) = oneshot::channel();
    let worker_task: SkillWorkerTask = Box::new(move || {
        let _ = completion_tx.send(catch_unwind(AssertUnwindSafe(task)));
    });
    let worker =
        spawn(worker_task).map_err(|_| worker_start_failure_response(request_id.clone(), &kind))?;
    Ok(StartedSkillJob {
        _permit: permit,
        request_id,
        kind,
        completion: completion_rx,
        worker,
    })
}

async fn finish_started_job(job: StartedSkillJob) -> Value {
    let StartedSkillJob {
        _permit,
        request_id,
        kind,
        completion,
        worker,
    } = job;
    let worker_result = completion.await;
    drop(worker);
    match worker_result {
        Ok(Ok(response)) => response,
        Ok(Err(_)) => {
            worker_execution_failure_response(request_id, &kind, SKILL_DISPATCH_PANICKED_MESSAGE)
        }
        Err(error) => worker_execution_failure_response(
            request_id,
            &kind,
            &format!("Skill filesystem request worker stopped without a response: {error}"),
        ),
    }
}

fn worker_start_failure_response(request_id: JsonRpcId, kind: &SkillJobKind) -> Value {
    match kind {
        SkillJobKind::Read => {
            worker_failure_response(request_id, SKILL_DISPATCH_WORKER_UNAVAILABLE_MESSAGE)
        }
        SkillJobKind::WorkflowCommit { preparation_id } => workflow_commit_error_response(
            request_id,
            preparation_id,
            false,
            SkillInspectionErrorCodeDto::Unavailable,
            SkillInspectionRecoveryDto::RetrySamePreparation,
            SKILL_WORKFLOW_COMMIT_UNAVAILABLE_MESSAGE,
        ),
        SkillJobKind::Mutation(mutation) => mutation_error_response(
            request_id,
            mutation,
            false,
            SkillInstallationErrorCodeDto::Unavailable,
            SKILL_MUTATION_UNAVAILABLE_MESSAGE,
        ),
    }
}

fn running_timeout_response(request_id: JsonRpcId, kind: &SkillJobKind) -> Value {
    match kind {
        SkillJobKind::Read => worker_failure_response(request_id, SKILL_DISPATCH_TIMED_OUT_MESSAGE),
        SkillJobKind::WorkflowCommit { preparation_id } => workflow_commit_error_response(
            request_id,
            preparation_id,
            true,
            SkillInspectionErrorCodeDto::CommitIndeterminate,
            SkillInspectionRecoveryDto::RefreshManagement,
            SKILL_WORKFLOW_COMMIT_INDETERMINATE_MESSAGE,
        ),
        SkillJobKind::Mutation(mutation) => mutation_error_response(
            request_id,
            mutation,
            true,
            SkillInstallationErrorCodeDto::CommitIndeterminate,
            SKILL_MUTATION_INDETERMINATE_MESSAGE,
        ),
    }
}

fn worker_execution_failure_response(
    request_id: JsonRpcId,
    kind: &SkillJobKind,
    message: &str,
) -> Value {
    match kind {
        SkillJobKind::Read => worker_failure_response(request_id, message),
        SkillJobKind::WorkflowCommit { preparation_id } => workflow_commit_error_response(
            request_id,
            preparation_id,
            true,
            SkillInspectionErrorCodeDto::CommitIndeterminate,
            SkillInspectionRecoveryDto::RefreshManagement,
            SKILL_WORKFLOW_COMMIT_INDETERMINATE_MESSAGE,
        ),
        // A mutation closure can panic or lose its completion signal after its
        // receipt commit point. Conservatively preserve retry-safe semantics.
        SkillJobKind::Mutation(mutation) => mutation_error_response(
            request_id,
            mutation,
            true,
            SkillInstallationErrorCodeDto::CommitIndeterminate,
            message,
        ),
    }
}

fn workflow_commit_error_response(
    request_id: JsonRpcId,
    preparation_id: &str,
    commit_may_have_succeeded: bool,
    code: SkillInspectionErrorCodeDto,
    recovery: SkillInspectionRecoveryDto,
    message: &str,
) -> Value {
    let fallback_request_id = request_id.clone();
    let data = SkillInspectionErrorData {
        error_type: SkillInspectionErrorTypeDto::SkillInspection,
        phase: SkillInspectionPhaseDto::Commit,
        code,
        recovery,
        message: message.to_string(),
        preparation_id: Some(preparation_id.to_string()),
        diagnostic_code: None,
        retry_after_ms: None,
        commit_may_have_succeeded,
        skill_id: None,
        intended_installation_revision: None,
    };
    serde_json::to_value(error_with_data(
        Some(request_id),
        SKILL_INSPECTION_ERROR_CODE,
        message,
        serde_json::to_value(data).expect("Skill inspection error data must serialize"),
    ))
    .unwrap_or_else(|_| worker_failure_response(fallback_request_id, message))
}

fn mutation_error_response(
    request_id: JsonRpcId,
    mutation: &SkillMutationJob,
    commit_may_have_succeeded: bool,
    code: SkillInstallationErrorCodeDto,
    message: &str,
) -> Value {
    let fallback_request_id = request_id.clone();
    let operation = match mutation.operation {
        SkillInstallationOperation::Install => SkillInstallationOperationDto::Install,
        SkillInstallationOperation::Update => SkillInstallationOperationDto::Update,
        SkillInstallationOperation::Uninstall => SkillInstallationOperationDto::Uninstall,
        _ => return worker_failure_response(fallback_request_id, message),
    };
    let (installation_id, skill_id) = match &mutation.target {
        SkillMutationTarget::InstallationId(installation_id) => {
            (Some(installation_id.as_str().to_string()), None)
        }
        SkillMutationTarget::SkillId(skill_id) => (None, Some(skill_id.as_str().to_string())),
    };
    let data = SkillInstallationErrorData {
        error_type: SkillInstallationErrorTypeDto::SkillInstallation,
        operation,
        code,
        recovery: SkillInstallationRecoveryDto::RetrySameRequest,
        message: message.to_string(),
        commit_may_have_succeeded,
        installation_id,
        skill_id,
        diagnostic_code: None,
        intended_revision: None,
        expected_revision: None,
        actual_revision: None,
        capacity: None,
        limit: None,
    };
    serde_json::to_value(error_with_data(
        Some(request_id),
        SKILL_INSTALLATION_ERROR_CODE,
        message,
        serde_json::to_value(data).expect("Skill installation error data must serialize"),
    ))
    .unwrap_or_else(|_| worker_failure_response(fallback_request_id, message))
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
            Value::String("Skill filesystem request worker failed.".to_string()),
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

    fn installation_id() -> SkillInstallationId {
        SkillInstallationId::parse("0190b0f2-7c50-7cc0-8b25-3bb80f08b334").unwrap()
    }

    fn installed_skill_id() -> SkillId {
        SkillId::parse(format!("installed:user:{}", installation_id())).unwrap()
    }

    fn workflow_preparation_id() -> &'static str {
        "0190b0f2-7c50-7cc0-8b25-3bb80f08b335"
    }

    fn inspection_error_data(response: &Value) -> SkillInspectionErrorData {
        serde_json::from_value(response["error"]["data"].clone())
            .expect("response must contain valid Skill inspection error data")
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
        dispatcher
            .try_submit_mutation(
                id(2),
                SkillInstallationOperation::Install,
                SkillMutationTarget::InstallationId(installation_id()),
                || label("queued"),
            )
            .unwrap();
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
    async fn mutation_worker_panic_is_conservatively_commit_indeterminate() {
        let (outbound_tx, mut outbound_rx) = mpsc::unbounded_channel();
        let dispatcher = SkillsDispatcher::with_limit(outbound_tx, 2);

        dispatcher
            .try_submit_mutation(
                JsonRpcId::String("mutation-panic".to_string()),
                SkillInstallationOperation::Install,
                SkillMutationTarget::InstallationId(installation_id()),
                || panic!("test mutation panic"),
            )
            .unwrap();
        let response = outbound_rx.recv().await.unwrap();
        dispatcher.shutdown().await.unwrap();

        assert_eq!(response["id"], "mutation-panic");
        assert_eq!(response["error"]["code"], SKILL_INSTALLATION_ERROR_CODE);
        assert_eq!(response["error"]["data"]["code"], "commitIndeterminate");
        assert_eq!(response["error"]["data"]["operation"], "install");
        assert_eq!(
            response["error"]["data"]["installationId"],
            installation_id().as_str()
        );
        assert_eq!(response["error"]["data"]["commitMayHaveSucceeded"], true);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn workflow_commit_worker_panic_is_conservatively_commit_indeterminate() {
        let (outbound_tx, mut outbound_rx) = mpsc::unbounded_channel();
        let dispatcher = SkillsDispatcher::with_limit(outbound_tx, 2);

        dispatcher
            .try_submit_workflow_commit(
                JsonRpcId::String("workflow-panic".to_string()),
                workflow_preparation_id().to_string(),
                || panic!("test workflow commit panic"),
            )
            .unwrap();
        let response = outbound_rx.recv().await.unwrap();
        dispatcher.shutdown().await.unwrap();

        let data = inspection_error_data(&response);
        assert_eq!(response["id"], "workflow-panic");
        assert_eq!(response["error"]["code"], SKILL_INSPECTION_ERROR_CODE);
        assert_eq!(
            data.error_type,
            SkillInspectionErrorTypeDto::SkillInspection
        );
        assert_eq!(data.phase, SkillInspectionPhaseDto::Commit);
        assert_eq!(data.code, SkillInspectionErrorCodeDto::CommitIndeterminate);
        assert_eq!(data.recovery, SkillInspectionRecoveryDto::RefreshManagement);
        assert_eq!(
            data.preparation_id.as_deref(),
            Some(workflow_preparation_id())
        );
        assert!(data.commit_may_have_succeeded);
        assert_eq!(
            response["error"]["message"],
            SKILL_WORKFLOW_COMMIT_INDETERMINATE_MESSAGE
        );
        assert!(!response.to_string().contains("test workflow commit panic"));
    }

    #[test]
    fn mutation_worker_start_failure_is_retryable_and_definitely_not_committed() {
        let admission = Arc::new(Semaphore::new(1));
        let permit = Arc::clone(&admission).try_acquire_owned().unwrap();
        let task_ran = Arc::new(AtomicBool::new(false));
        let task_ran_from_worker = Arc::clone(&task_ran);
        let job = QueuedSkillJob {
            permit,
            request_id: JsonRpcId::String("mutation-start-failure".to_string()),
            kind: SkillJobKind::Mutation(SkillMutationJob {
                operation: SkillInstallationOperation::Update,
                target: SkillMutationTarget::SkillId(installed_skill_id()),
            }),
            task: Box::new(move || {
                task_ran_from_worker.store(true, Ordering::Release);
                label("must-not-run")
            }),
        };

        let response = match start_job_with_spawner(job, |_worker| {
            Err(std::io::Error::other("forced worker start failure"))
        }) {
            Ok(_) => panic!("forced worker start failure unexpectedly succeeded"),
            Err(response) => response,
        };

        assert!(!task_ran.load(Ordering::Acquire));
        assert_eq!(admission.available_permits(), 1);
        assert_eq!(response["id"], "mutation-start-failure");
        assert_eq!(response["error"]["code"], SKILL_INSTALLATION_ERROR_CODE);
        assert_eq!(
            response["error"]["message"],
            SKILL_MUTATION_UNAVAILABLE_MESSAGE
        );
        assert_eq!(response["error"]["data"]["code"], "unavailable");
        assert_eq!(response["error"]["data"]["recovery"], "retrySameRequest");
        assert_eq!(response["error"]["data"]["operation"], "update");
        assert_eq!(
            response["error"]["data"]["skillId"],
            installed_skill_id().as_str()
        );
        assert_eq!(response["error"]["data"]["commitMayHaveSucceeded"], false);
        assert!(!response.to_string().contains("forced worker start failure"));
    }

    #[test]
    fn workflow_commit_worker_start_failure_is_retryable_and_definitely_not_committed() {
        let admission = Arc::new(Semaphore::new(1));
        let permit = Arc::clone(&admission).try_acquire_owned().unwrap();
        let task_ran = Arc::new(AtomicBool::new(false));
        let task_ran_from_worker = Arc::clone(&task_ran);
        let job = QueuedSkillJob {
            permit,
            request_id: JsonRpcId::String("workflow-start-failure".to_string()),
            kind: SkillJobKind::WorkflowCommit {
                preparation_id: workflow_preparation_id().to_string(),
            },
            task: Box::new(move || {
                task_ran_from_worker.store(true, Ordering::Release);
                label("must-not-run")
            }),
        };

        let response = match start_job_with_spawner(job, |_worker| {
            Err(std::io::Error::other("forced worker start failure"))
        }) {
            Ok(_) => panic!("forced worker start failure unexpectedly succeeded"),
            Err(response) => response,
        };

        let data = inspection_error_data(&response);
        assert!(!task_ran.load(Ordering::Acquire));
        assert_eq!(admission.available_permits(), 1);
        assert_eq!(response["id"], "workflow-start-failure");
        assert_eq!(response["error"]["code"], SKILL_INSPECTION_ERROR_CODE);
        assert_eq!(
            response["error"]["message"],
            SKILL_WORKFLOW_COMMIT_UNAVAILABLE_MESSAGE
        );
        assert_eq!(
            data.error_type,
            SkillInspectionErrorTypeDto::SkillInspection
        );
        assert_eq!(data.phase, SkillInspectionPhaseDto::Commit);
        assert_eq!(data.code, SkillInspectionErrorCodeDto::Unavailable);
        assert_eq!(
            data.recovery,
            SkillInspectionRecoveryDto::RetrySamePreparation
        );
        assert_eq!(
            data.preparation_id.as_deref(),
            Some(workflow_preparation_id())
        );
        assert!(!data.commit_may_have_succeeded);
        assert!(response["error"]["data"]
            .get("commitMayHaveSucceeded")
            .is_none());
        assert!(!response.to_string().contains("forced worker start failure"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn workflow_commit_completion_loss_is_conservatively_commit_indeterminate() {
        let admission = Arc::new(Semaphore::new(1));
        let permit = Arc::clone(&admission).try_acquire_owned().unwrap();
        let task_ran = Arc::new(AtomicBool::new(false));
        let task_ran_from_worker = Arc::clone(&task_ran);
        let job = QueuedSkillJob {
            permit,
            request_id: JsonRpcId::String("workflow-completion-loss".to_string()),
            kind: SkillJobKind::WorkflowCommit {
                preparation_id: workflow_preparation_id().to_string(),
            },
            task: Box::new(move || {
                task_ran_from_worker.store(true, Ordering::Release);
                label("must-not-run")
            }),
        };

        let started = start_job_with_spawner(job, |_worker| {
            std::thread::Builder::new()
                .name("skill-completion-loss-test".to_string())
                .spawn(|| {})
        })
        .expect("test worker must start");
        let response = finish_started_job(started).await;

        let data = inspection_error_data(&response);
        assert!(!task_ran.load(Ordering::Acquire));
        assert_eq!(admission.available_permits(), 1);
        assert_eq!(response["id"], "workflow-completion-loss");
        assert_eq!(response["error"]["code"], SKILL_INSPECTION_ERROR_CODE);
        assert_eq!(data.phase, SkillInspectionPhaseDto::Commit);
        assert_eq!(data.code, SkillInspectionErrorCodeDto::CommitIndeterminate);
        assert_eq!(data.recovery, SkillInspectionRecoveryDto::RefreshManagement);
        assert_eq!(
            data.preparation_id.as_deref(),
            Some(workflow_preparation_id())
        );
        assert!(data.commit_may_have_succeeded);
        assert_eq!(
            response["error"]["message"],
            SKILL_WORKFLOW_COMMIT_INDETERMINATE_MESSAGE
        );
    }

    #[test]
    fn mutation_admission_failure_preserves_retry_identity() {
        let response = mutation_admission_error_response(
            id(27),
            SkillInstallationOperation::Install,
            SkillMutationTarget::InstallationId(installation_id()),
            SkillsDispatchError::Full,
        );

        assert_eq!(response["id"], 27);
        assert_eq!(response["error"]["code"], SKILL_INSTALLATION_ERROR_CODE);
        assert_eq!(
            response["error"]["message"],
            SKILL_DISPATCH_QUEUE_FULL_MESSAGE
        );
        assert_eq!(response["error"]["data"]["code"], "unavailable");
        assert_eq!(response["error"]["data"]["operation"], "install");
        assert_eq!(
            response["error"]["data"]["installationId"],
            installation_id().as_str()
        );
        assert_eq!(response["error"]["data"]["commitMayHaveSucceeded"], false);
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
    async fn shutdown_marks_a_queued_mutation_as_not_started() {
        let (outbound_tx, mut outbound_rx) = mpsc::unbounded_channel();
        let dispatcher = SkillsDispatcher::with_limit(outbound_tx, 3);
        let queued_ran = Arc::new(AtomicBool::new(false));
        let (started_tx, started_rx) = std_mpsc::channel();
        let (release_tx, release_rx) = std_mpsc::channel();

        dispatcher
            .try_submit(id(1), move || {
                started_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                label("running-read")
            })
            .unwrap();
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let queued_ran_from_task = Arc::clone(&queued_ran);
        dispatcher
            .try_submit_mutation(
                id(2),
                SkillInstallationOperation::Update,
                SkillMutationTarget::SkillId(installed_skill_id()),
                move || {
                    queued_ran_from_task.store(true, Ordering::Release);
                    label("must-not-run")
                },
            )
            .unwrap();

        let shutdown = tokio::spawn(dispatcher.shutdown());
        let cancelled = outbound_rx.recv().await.unwrap();

        assert_eq!(cancelled["id"], 2);
        assert_eq!(cancelled["error"]["code"], SKILL_INSTALLATION_ERROR_CODE);
        assert_eq!(
            cancelled["error"]["message"],
            SKILL_MUTATION_CANCELLED_MESSAGE
        );
        assert_eq!(cancelled["error"]["data"]["type"], "skillInstallation");
        assert_eq!(cancelled["error"]["data"]["code"], "cancelled");
        assert_eq!(cancelled["error"]["data"]["recovery"], "retrySameRequest");
        assert_eq!(cancelled["error"]["data"]["operation"], "update");
        assert_eq!(
            cancelled["error"]["data"]["skillId"],
            installed_skill_id().as_str()
        );
        assert!(cancelled["error"]["data"].get("installationId").is_none());
        assert_eq!(cancelled["error"]["data"]["commitMayHaveSucceeded"], false);
        assert!(cancelled["error"]["data"].get("commitState").is_none());
        assert!(!queued_ran.load(Ordering::Acquire));
        assert!(!shutdown.is_finished());

        release_tx.send(()).unwrap();
        assert_eq!(outbound_rx.recv().await.unwrap()["label"], "running-read");
        shutdown.await.unwrap().unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn shutdown_marks_a_queued_workflow_commit_as_not_started() {
        let (outbound_tx, mut outbound_rx) = mpsc::unbounded_channel();
        let dispatcher = SkillsDispatcher::with_limit(outbound_tx, 3);
        let queued_ran = Arc::new(AtomicBool::new(false));
        let (started_tx, started_rx) = std_mpsc::channel();
        let (release_tx, release_rx) = std_mpsc::channel();

        dispatcher
            .try_submit(id(1), move || {
                started_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                label("running-read")
            })
            .unwrap();
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let queued_ran_from_task = Arc::clone(&queued_ran);
        dispatcher
            .try_submit_workflow_commit(
                JsonRpcId::String("queued-workflow".to_string()),
                workflow_preparation_id().to_string(),
                move || {
                    queued_ran_from_task.store(true, Ordering::Release);
                    label("must-not-run")
                },
            )
            .unwrap();

        let shutdown = tokio::spawn(dispatcher.shutdown());
        let cancelled = outbound_rx.recv().await.unwrap();

        let data = inspection_error_data(&cancelled);
        assert_eq!(cancelled["id"], "queued-workflow");
        assert_eq!(cancelled["error"]["code"], SKILL_INSPECTION_ERROR_CODE);
        assert_eq!(
            cancelled["error"]["message"],
            SKILL_WORKFLOW_COMMIT_CANCELLED_MESSAGE
        );
        assert_eq!(
            data.error_type,
            SkillInspectionErrorTypeDto::SkillInspection
        );
        assert_eq!(data.phase, SkillInspectionPhaseDto::Commit);
        assert_eq!(data.code, SkillInspectionErrorCodeDto::Cancelled);
        assert_eq!(
            data.recovery,
            SkillInspectionRecoveryDto::RetrySamePreparation
        );
        assert_eq!(
            data.preparation_id.as_deref(),
            Some(workflow_preparation_id())
        );
        assert!(!data.commit_may_have_succeeded);
        assert!(cancelled["error"]["data"]
            .get("commitMayHaveSucceeded")
            .is_none());
        assert!(!queued_ran.load(Ordering::Acquire));
        assert!(!shutdown.is_finished());

        release_tx.send(()).unwrap();
        assert_eq!(outbound_rx.recv().await.unwrap()["label"], "running-read");
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
        assert!(response["error"].get("data").is_none());
        release_tx.send(()).unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn running_mutation_shutdown_timeout_reports_an_indeterminate_commit() {
        let (outbound_tx, mut outbound_rx) = mpsc::unbounded_channel();
        let dispatcher = SkillsDispatcher::with_limit_and_shutdown_grace(
            outbound_tx,
            2,
            Duration::from_millis(25),
        );
        let (started_tx, started_rx) = std_mpsc::channel();
        let (release_tx, release_rx) = std_mpsc::channel();
        dispatcher
            .try_submit_mutation(
                JsonRpcId::String("mutation-9".to_string()),
                SkillInstallationOperation::Uninstall,
                SkillMutationTarget::SkillId(installed_skill_id()),
                move || {
                    started_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    label("late-mutation")
                },
            )
            .unwrap();
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();

        tokio::time::timeout(Duration::from_secs(1), dispatcher.shutdown())
            .await
            .expect("dispatcher shutdown must respect its deadline")
            .unwrap();
        let response = outbound_rx.recv().await.unwrap();

        assert_eq!(response["id"], "mutation-9");
        assert_eq!(response["error"]["code"], SKILL_INSTALLATION_ERROR_CODE);
        assert_eq!(
            response["error"]["message"],
            SKILL_MUTATION_INDETERMINATE_MESSAGE
        );
        assert_eq!(response["error"]["data"]["type"], "skillInstallation");
        assert_eq!(response["error"]["data"]["code"], "commitIndeterminate");
        assert_eq!(response["error"]["data"]["recovery"], "retrySameRequest");
        assert_eq!(response["error"]["data"]["operation"], "uninstall");
        assert_eq!(
            response["error"]["data"]["skillId"],
            installed_skill_id().as_str()
        );
        assert!(response["error"]["data"].get("installationId").is_none());
        assert_eq!(response["error"]["data"]["commitMayHaveSucceeded"], true);
        assert_eq!(
            response["error"]["data"]["message"],
            SKILL_MUTATION_INDETERMINATE_MESSAGE
        );
        assert!(response["error"]["data"].get("commitState").is_none());
        release_tx.send(()).unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn running_workflow_commit_shutdown_timeout_reports_an_indeterminate_commit() {
        let (outbound_tx, mut outbound_rx) = mpsc::unbounded_channel();
        let dispatcher = SkillsDispatcher::with_limit_and_shutdown_grace(
            outbound_tx,
            2,
            Duration::from_millis(25),
        );
        let (started_tx, started_rx) = std_mpsc::channel();
        let (release_tx, release_rx) = std_mpsc::channel();
        dispatcher
            .try_submit_workflow_commit(
                JsonRpcId::String("workflow-timeout".to_string()),
                workflow_preparation_id().to_string(),
                move || {
                    started_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    label("late-workflow")
                },
            )
            .unwrap();
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();

        tokio::time::timeout(Duration::from_secs(1), dispatcher.shutdown())
            .await
            .expect("dispatcher shutdown must respect its deadline")
            .unwrap();
        let response = outbound_rx.recv().await.unwrap();

        let data = inspection_error_data(&response);
        assert_eq!(response["id"], "workflow-timeout");
        assert_eq!(response["error"]["code"], SKILL_INSPECTION_ERROR_CODE);
        assert_eq!(
            response["error"]["message"],
            SKILL_WORKFLOW_COMMIT_INDETERMINATE_MESSAGE
        );
        assert_eq!(
            data.error_type,
            SkillInspectionErrorTypeDto::SkillInspection
        );
        assert_eq!(data.phase, SkillInspectionPhaseDto::Commit);
        assert_eq!(data.code, SkillInspectionErrorCodeDto::CommitIndeterminate);
        assert_eq!(data.recovery, SkillInspectionRecoveryDto::RefreshManagement);
        assert_eq!(
            data.preparation_id.as_deref(),
            Some(workflow_preparation_id())
        );
        assert!(data.commit_may_have_succeeded);
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
