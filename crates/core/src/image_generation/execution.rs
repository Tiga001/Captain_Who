//! Provider-neutral, replay-safe image-generation execution service.
//!
//! This service is intentionally not an Agent Tool. It is the backend execution boundary that a
//! future Tool can invoke after applying Agent permissions and file-materialization policy.

use super::artifact::{
    ImageArtifactError, ImageArtifactErrorCode, ImageArtifactFormat,
    ImageGenerationArtifactCandidate, ImageGenerationArtifactStore, PublishedImageArtifact,
};
use super::configuration::{
    ImageGenerationConfigurationError, ImageGenerationConfigurationService,
    ImageGenerationExecutionSnapshot, ImageGenerationReadiness,
};
use super::provider::ImageGenerationAdapterRegistry;
use super::types::{
    ImageGenerationError, ImageGenerationErrorCode, ImageGenerationOperation,
    ImageGenerationRequest, ImageGenerationResult, ImageGenerationResultStatus,
    PreparedImageGenerationRequest,
};
use crate::storage::image_generation_execution_repository::{
    ImageGenerationArtifactJournalRecord, ImageGenerationExecutionClaimOutcome,
    ImageGenerationExecutionIdentityRecord, ImageGenerationExecutionJournalRecord,
    ImageGenerationExecutionMutationOutcome, ImageGenerationExecutionTerminalUpdate,
    StoredImageGenerationArtifactState, StoredImageGenerationExecutionStatus,
};
use crate::storage::now_ms;
use crate::storage::service::StorageService;
use crate::AgentCancellationToken;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{Notify, Semaphore};
use tokio::time::Instant;

pub const IMAGE_GENERATION_EXECUTION_RECEIPT_SCHEMA_VERSION: u32 = 1;
const SAFE_EXECUTION_REQUEST_SCHEMA_VERSION: u32 = 1;
pub const DEFAULT_IMAGE_GENERATION_MAX_CONCURRENT: usize = 2;
pub const DEFAULT_IMAGE_GENERATION_MAX_ADMITTED: usize = 8;
pub const DEFAULT_IMAGE_GENERATION_EXECUTION_TIMEOUT: Duration = Duration::from_secs(5 * 60);
pub const DEFAULT_IMAGE_GENERATION_CONFIGURATION_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_EXECUTION_ID_BYTES: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageGenerationExecutionLimits {
    pub max_concurrent: usize,
    pub max_admitted: usize,
    pub execution_timeout: Duration,
    pub configuration_timeout: Duration,
}

impl Default for ImageGenerationExecutionLimits {
    fn default() -> Self {
        Self {
            max_concurrent: DEFAULT_IMAGE_GENERATION_MAX_CONCURRENT,
            max_admitted: DEFAULT_IMAGE_GENERATION_MAX_ADMITTED,
            execution_timeout: DEFAULT_IMAGE_GENERATION_EXECUTION_TIMEOUT,
            configuration_timeout: DEFAULT_IMAGE_GENERATION_CONFIGURATION_TIMEOUT,
        }
    }
}

impl ImageGenerationExecutionLimits {
    fn validate(self) -> Result<Self, ImageGenerationExecutionServiceError> {
        if self.max_concurrent == 0
            || self.max_admitted < self.max_concurrent
            || self.max_admitted > 64
            || self.execution_timeout.is_zero()
            || self.execution_timeout > Duration::from_secs(30 * 60)
            || self.configuration_timeout.is_zero()
            || self.configuration_timeout > self.execution_timeout
        {
            return Err(ImageGenerationExecutionServiceError::new(
                ImageGenerationExecutionServiceErrorCode::InvalidConfiguration,
                "image-generation execution limits are invalid",
                false,
            ));
        }
        Ok(self)
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct ImageGenerationExecutionId(String);

impl ImageGenerationExecutionId {
    pub fn parse(value: impl Into<String>) -> Result<Self, ImageGenerationExecutionServiceError> {
        let value = value.into();
        if value.trim() != value
            || value.is_empty()
            || value.len() > MAX_EXECUTION_ID_BYTES
            || value.chars().any(char::is_control)
        {
            return Err(ImageGenerationExecutionServiceError::new(
                ImageGenerationExecutionServiceErrorCode::InvalidRequest,
                "image-generation execution id is invalid",
                false,
            ));
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ImageGenerationExecutionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("ImageGenerationExecutionId")
            .field(&self.0)
            .finish()
    }
}

impl fmt::Display for ImageGenerationExecutionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct ImageGenerationExecutionRequest {
    pub execution_id: ImageGenerationExecutionId,
    pub request: ImageGenerationRequest,
}

impl fmt::Debug for ImageGenerationExecutionRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ImageGenerationExecutionRequest")
            .field("execution_id", &self.execution_id)
            .field("request", &"[REDACTED]")
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ImageGenerationExecutionStatus {
    Succeeded,
    Failed,
    Cancelled,
    OutcomeIndeterminate,
    CommitIndeterminate,
}

impl ImageGenerationExecutionStatus {
    fn stored(self) -> StoredImageGenerationExecutionStatus {
        match self {
            Self::Succeeded => StoredImageGenerationExecutionStatus::Succeeded,
            Self::Failed => StoredImageGenerationExecutionStatus::Failed,
            Self::Cancelled => StoredImageGenerationExecutionStatus::Cancelled,
            Self::OutcomeIndeterminate => {
                StoredImageGenerationExecutionStatus::OutcomeIndeterminate
            }
            Self::CommitIndeterminate => StoredImageGenerationExecutionStatus::CommitIndeterminate,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ImageGenerationExecutionPhase {
    Provider,
    ArtifactDownload,
    ArtifactPublish,
    Journal,
    Recovery,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ImageGenerationExecutionFailureCode {
    ProviderFailed,
    Cancelled,
    DeadlineExceeded,
    ArtifactFailed,
    CommitIndeterminate,
    ExecutionInterrupted,
    JournalUnavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageGenerationExecutionFailure {
    pub code: ImageGenerationExecutionFailureCode,
    pub phase: ImageGenerationExecutionPhase,
    pub message: String,
    pub recovery: String,
    pub retryable: bool,
    pub generation_may_have_succeeded: bool,
    pub provider_succeeded: bool,
    pub artifact_commit_may_have_succeeded: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_error_code: Option<ImageGenerationErrorCode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact_error_code: Option<ImageArtifactErrorCode>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageGenerationExecutionReceipt {
    pub schema_version: u32,
    pub execution_id: String,
    pub request_fingerprint: String,
    pub status: ImageGenerationExecutionStatus,
    pub provider_profile_id: String,
    pub adapter_id: String,
    pub profile_revision: u64,
    pub model_id: String,
    pub operation: ImageGenerationOperation,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_request_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact: Option<ImageGenerationArtifactCandidate>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ImageGenerationExecutionFailure>,
    pub created_at: i64,
    pub completed_at: i64,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageGenerationExecutionResult {
    pub receipt: ImageGenerationExecutionReceipt,
    /// Present only when the terminal receipt references a locally verified managed Artifact.
    pub managed_artifact: Option<PublishedImageArtifact>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageGenerationExecutionServiceErrorCode {
    InvalidConfiguration,
    InvalidRequest,
    ConfigurationDisabled,
    ConfigurationIncomplete,
    ConfigurationUnavailable,
    Busy,
    Cancelled,
    DeadlineExceeded,
    ShuttingDown,
    IdempotencyConflict,
    AlreadyClaimed,
    JournalUnavailable,
    JournalCorrupt,
    CommitIndeterminate,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageGenerationExecutionServiceError {
    pub code: ImageGenerationExecutionServiceErrorCode,
    pub message: String,
    pub retryable: bool,
}

impl ImageGenerationExecutionServiceError {
    #[must_use]
    pub fn new(
        code: ImageGenerationExecutionServiceErrorCode,
        message: impl Into<String>,
        retryable: bool,
    ) -> Self {
        Self {
            code,
            message: message.into(),
            retryable,
        }
    }
}

impl fmt::Display for ImageGenerationExecutionServiceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ImageGenerationExecutionServiceError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ImageGenerationExecutionRecoveryReport {
    pub recovered_artifacts: usize,
    pub interrupted_before_publish: usize,
    pub remote_outcome_unknown: usize,
    pub commit_indeterminate: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ImageGenerationExecutionShutdownReport {
    pub cancelled_executions: usize,
    pub timed_out: bool,
}

pub struct ImageGenerationExecutionService {
    configuration: Arc<ImageGenerationConfigurationService>,
    adapters: Arc<ImageGenerationAdapterRegistry>,
    artifacts: Arc<dyn ImageGenerationArtifactStore>,
    storage: Arc<StorageService>,
    limits: ImageGenerationExecutionLimits,
    admitted: Arc<Semaphore>,
    workers: Arc<Semaphore>,
    accepting: AtomicBool,
    active: Arc<Mutex<HashMap<String, AgentCancellationToken>>>,
    active_changed: Arc<Notify>,
}

#[derive(Debug, Clone, Copy)]
struct ExecutionTiming {
    started: Instant,
    deadline: Instant,
}

impl fmt::Debug for ImageGenerationExecutionService {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ImageGenerationExecutionService")
            .field("limits", &self.limits)
            .field("accepting", &self.accepting.load(Ordering::SeqCst))
            .finish_non_exhaustive()
    }
}

impl ImageGenerationExecutionService {
    pub fn new(
        configuration: Arc<ImageGenerationConfigurationService>,
        adapters: Arc<ImageGenerationAdapterRegistry>,
        artifacts: Arc<dyn ImageGenerationArtifactStore>,
        storage: Arc<StorageService>,
        limits: ImageGenerationExecutionLimits,
    ) -> Result<Self, ImageGenerationExecutionServiceError> {
        let limits = limits.validate()?;
        if adapters.is_empty() {
            return Err(ImageGenerationExecutionServiceError::new(
                ImageGenerationExecutionServiceErrorCode::InvalidConfiguration,
                "no image-generation provider adapters are registered",
                false,
            ));
        }
        Ok(Self {
            configuration,
            adapters,
            artifacts,
            storage,
            limits,
            admitted: Arc::new(Semaphore::new(limits.max_admitted)),
            workers: Arc::new(Semaphore::new(limits.max_concurrent)),
            accepting: AtomicBool::new(true),
            active: Arc::new(Mutex::new(HashMap::new())),
            active_changed: Arc::new(Notify::new()),
        })
    }

    pub async fn execute(
        &self,
        request: ImageGenerationExecutionRequest,
        cancellation: AgentCancellationToken,
    ) -> Result<ImageGenerationExecutionResult, ImageGenerationExecutionServiceError> {
        if !self.accepting.load(Ordering::SeqCst) {
            return Err(service_error(
                ImageGenerationExecutionServiceErrorCode::ShuttingDown,
                "image-generation execution service is shutting down",
                true,
            ));
        }
        if cancellation.is_cancelled() {
            return Err(service_error(
                ImageGenerationExecutionServiceErrorCode::Cancelled,
                "image-generation execution was cancelled before admission",
                false,
            ));
        }
        let _admission = Arc::clone(&self.admitted)
            .try_acquire_owned()
            .map_err(|_| {
                service_error(
                    ImageGenerationExecutionServiceErrorCode::Busy,
                    "image-generation execution capacity is full",
                    true,
                )
            })?;
        let _active = self.register_active(&request.execution_id, cancellation.clone())?;
        let started = Instant::now();
        let deadline = started + self.limits.execution_timeout;
        let worker = tokio::select! {
            _ = cancellation.cancelled() => return Err(service_error(
                ImageGenerationExecutionServiceErrorCode::Cancelled,
                "image-generation execution was cancelled while waiting for capacity",
                false,
            )),
            result = tokio::time::timeout_at(deadline, Arc::clone(&self.workers).acquire_owned()) => {
                result
                    .map_err(|_| service_error(
                        ImageGenerationExecutionServiceErrorCode::DeadlineExceeded,
                        "image-generation execution timed out while waiting for capacity",
                        true,
                    ))?
                    .map_err(|_| service_error(
                        ImageGenerationExecutionServiceErrorCode::ShuttingDown,
                        "image-generation worker pool is unavailable",
                        true,
                    ))?
            }
        };
        if let Some(existing) = storage_inspect(
            Arc::clone(&self.storage),
            request.execution_id.as_str().to_string(),
        )
        .await?
        {
            if !request_matches_safe_identity(
                &request.request,
                &existing.identity.safe_request_json,
            ) {
                return Err(service_error(
                    ImageGenerationExecutionServiceErrorCode::IdempotencyConflict,
                    "image-generation execution id is already bound to different frozen inputs",
                    false,
                ));
            }
            return self.result_from_existing(existing).await;
        }

        let configuration = Arc::clone(&self.configuration);
        let binding_task = tokio::task::spawn_blocking(move || {
            let snapshot = configuration.resolve_execution_snapshot();
            (snapshot, worker)
        });
        let configuration_deadline =
            (Instant::now() + self.limits.configuration_timeout).min(deadline);
        let (snapshot_result, worker) = tokio::select! {
            _ = cancellation.cancelled() => return Err(service_error(
                ImageGenerationExecutionServiceErrorCode::Cancelled,
                "image-generation execution was cancelled while resolving configuration",
                false,
            )),
            result = tokio::time::timeout_at(configuration_deadline, binding_task) => {
                result
                    .map_err(|_| service_error(
                        ImageGenerationExecutionServiceErrorCode::ConfigurationUnavailable,
                        "image-generation credential resolution timed out",
                        true,
                    ))?
                    .map_err(|_| service_error(
                        ImageGenerationExecutionServiceErrorCode::ConfigurationUnavailable,
                        "image-generation credential resolver stopped unexpectedly",
                        true,
                    ))?
            }
        };
        let _worker = worker;
        let snapshot = snapshot_result.map_err(map_configuration_error)?;
        let provider = self
            .adapters
            .create(snapshot.profile.clone())
            .map_err(map_preparation_error)?;
        let prepared = provider
            .prepare(request.request)
            .map_err(map_preparation_error)?;
        validate_prepared_profile(&snapshot.profile, &prepared).map_err(map_preparation_error)?;
        if prepared.normalized().operation() == ImageGenerationOperation::Status {
            return Err(service_error(
                ImageGenerationExecutionServiceErrorCode::InvalidRequest,
                "status is not an Artifact-producing image-generation execution",
                false,
            ));
        }
        let identity = execution_identity(&request.execution_id, &prepared)?;
        let claim = storage_claim(Arc::clone(&self.storage), identity.clone()).await?;
        match claim {
            ImageGenerationExecutionClaimOutcome::Existing(existing) => {
                drop(snapshot);
                self.result_from_existing(existing).await
            }
            ImageGenerationExecutionClaimOutcome::IdentityConflict(_) => {
                drop(snapshot);
                Err(service_error(
                    ImageGenerationExecutionServiceErrorCode::IdempotencyConflict,
                    "image-generation execution id is already bound to different frozen inputs",
                    false,
                ))
            }
            ImageGenerationExecutionClaimOutcome::Claimed(claimed) => {
                if cancellation.is_cancelled() {
                    drop(snapshot);
                    return self
                        .finalize_failure(
                            &claimed,
                            ImageGenerationExecutionStatus::Cancelled,
                            failure(
                                ImageGenerationExecutionFailureCode::Cancelled,
                                ImageGenerationExecutionPhase::Provider,
                                "image-generation execution was cancelled before the provider request",
                                "Start a new request when ready.",
                                false,
                                false,
                                false,
                                false,
                                None,
                                None,
                            ),
                            None,
                            None,
                            started,
                        )
                        .await;
                }
                self.execute_claimed(
                    claimed,
                    provider,
                    prepared,
                    snapshot,
                    cancellation,
                    ExecutionTiming { started, deadline },
                )
                .await
            }
        }
    }

    async fn execute_claimed(
        &self,
        claimed: ImageGenerationExecutionJournalRecord,
        provider: Arc<dyn super::provider::ImageGenerationProvider>,
        prepared: PreparedImageGenerationRequest,
        snapshot: ImageGenerationExecutionSnapshot,
        cancellation: AgentCancellationToken,
        timing: ExecutionTiming,
    ) -> Result<ImageGenerationExecutionResult, ImageGenerationExecutionServiceError> {
        let ExecutionTiming { started, deadline } = timing;
        let mut provider_future = provider.execute(&prepared, &snapshot.credential);
        let provider_result = tokio::select! {
            _ = cancellation.cancelled() => {
                return self.finalize_failure(
                    &claimed,
                    ImageGenerationExecutionStatus::OutcomeIndeterminate,
                    failure(
                        ImageGenerationExecutionFailureCode::Cancelled,
                        ImageGenerationExecutionPhase::Provider,
                        "image-generation provider request was cancelled; its remote outcome is unknown",
                        "Inspect provider usage before starting a new execution.",
                        false,
                        true,
                        false,
                        false,
                        None,
                        None,
                    ),
                    None,
                    None,
                    started,
                ).await;
            }
            result = tokio::time::timeout_at(deadline, &mut provider_future) => {
                match result {
                    Ok(result) => result,
                    Err(_) => {
                        return self.finalize_failure(
                            &claimed,
                            ImageGenerationExecutionStatus::OutcomeIndeterminate,
                            failure(
                                ImageGenerationExecutionFailureCode::DeadlineExceeded,
                                ImageGenerationExecutionPhase::Provider,
                                "image-generation provider request exceeded the execution deadline; its remote outcome is unknown",
                                "Inspect provider usage before starting a new execution.",
                                false,
                                true,
                                false,
                                false,
                                Some(ImageGenerationErrorCode::RequestTimedOut),
                                None,
                            ),
                            None,
                            None,
                            started,
                        ).await;
                    }
                }
            }
        };
        // Provider credentials are not retained during Artifact download or persistence.
        drop(provider_future);
        drop(snapshot);
        let provider_result = match provider_result {
            Ok(result) => result,
            Err(error) => {
                let remote_unknown = provider_outcome_unknown(&error);
                let provider_request_id =
                    opaque_provider_request_id(error.provider_request_id.as_deref());
                return self
                    .finalize_failure(
                        &claimed,
                        if remote_unknown {
                            ImageGenerationExecutionStatus::OutcomeIndeterminate
                        } else {
                            ImageGenerationExecutionStatus::Failed
                        },
                        failure(
                            ImageGenerationExecutionFailureCode::ProviderFailed,
                            ImageGenerationExecutionPhase::Provider,
                            provider_failure_message(error.code),
                            if remote_unknown {
                                "Inspect provider usage before starting a new execution."
                            } else if error.retryable {
                                "Retry with a new execution id."
                            } else {
                                "Review the provider configuration or request."
                            },
                            error.retryable && !remote_unknown,
                            remote_unknown,
                            false,
                            false,
                            Some(error.code),
                            None,
                        ),
                        provider_request_id,
                        error.http_status,
                        started,
                    )
                    .await;
            }
        };
        let provider_request_id =
            opaque_provider_request_id(provider_result.provider_request_id.as_deref());
        let provider_http_status = provider_result.http_status;
        if let Err(error) = validate_provider_result(&prepared, &provider_result) {
            return self
                .finalize_failure(
                    &claimed,
                    ImageGenerationExecutionStatus::Failed,
                    failure(
                        ImageGenerationExecutionFailureCode::ProviderFailed,
                        ImageGenerationExecutionPhase::Provider,
                        provider_failure_message(error.code),
                        "Retry with a new execution id after reviewing the provider adapter.",
                        false,
                        false,
                        true,
                        false,
                        Some(error.code),
                        None,
                    ),
                    provider_request_id,
                    provider_http_status,
                    started,
                )
                .await;
        }
        let [output] = provider_result.outputs.as_slice() else {
            unreachable!("validated provider result has exactly one output")
        };
        let prepared_artifact = tokio::select! {
            _ = cancellation.cancelled() => {
                return self.finalize_failure(
                    &claimed,
                    ImageGenerationExecutionStatus::Cancelled,
                    failure(
                        ImageGenerationExecutionFailureCode::Cancelled,
                        ImageGenerationExecutionPhase::ArtifactDownload,
                        "image-generation output download was cancelled after the provider succeeded",
                        "The provider may have charged for this generation; start a new execution only if another image is desired.",
                        false,
                        false,
                        true,
                        false,
                        None,
                        None,
                    ),
                    provider_request_id.clone(),
                    provider_http_status,
                    started,
                ).await;
            }
            result = tokio::time::timeout_at(deadline, self.artifacts.stage(output, &cancellation)) => {
                match result {
                    Ok(Ok(artifact)) => artifact,
                    Ok(Err(error)) => {
                        return self.finalize_artifact_failure(
                            &claimed,
                            error,
                            provider_request_id.clone(),
                            provider_http_status,
                            started,
                            ImageGenerationExecutionPhase::ArtifactDownload,
                        ).await;
                    }
                    Err(_) => {
                        return self.finalize_failure(
                            &claimed,
                            ImageGenerationExecutionStatus::Failed,
                            failure(
                                ImageGenerationExecutionFailureCode::DeadlineExceeded,
                                ImageGenerationExecutionPhase::ArtifactDownload,
                                "image-generation Artifact download exceeded the execution deadline",
                                "Start a new execution only if another generated image is desired.",
                                false,
                                false,
                                true,
                                false,
                                None,
                                Some(ImageArtifactErrorCode::DownloadTimedOut),
                            ),
                            provider_request_id.clone(),
                            provider_http_status,
                            started,
                        ).await;
                    }
                }
            }
        };

        let artifact_record = artifact_journal_record(prepared_artifact.candidate());
        let preparation = storage_prepare_artifact(
            Arc::clone(&self.storage),
            claimed.identity.execution_id.clone(),
            artifact_record,
            provider_request_id.clone(),
            provider_http_status,
        )
        .await;
        let publishing = match preparation {
            Ok(ImageGenerationExecutionMutationOutcome::Updated(record))
            | Ok(ImageGenerationExecutionMutationOutcome::AlreadyCurrent(record)) => record,
            Ok(ImageGenerationExecutionMutationOutcome::StateConflict(Some(record)))
                if record.status.is_terminal() =>
            {
                return self.result_from_existing(record).await;
            }
            Ok(ImageGenerationExecutionMutationOutcome::StateConflict(Some(record))) => {
                let artifact_commit_is_indeterminate = record.artifact.is_some();
                return self
                    .finalize_failure(
                        &record,
                        if artifact_commit_is_indeterminate {
                            ImageGenerationExecutionStatus::CommitIndeterminate
                        } else {
                            ImageGenerationExecutionStatus::Failed
                        },
                        failure(
                            ImageGenerationExecutionFailureCode::JournalUnavailable,
                            ImageGenerationExecutionPhase::Journal,
                            "image-generation Artifact candidate conflicted with its durable journal",
                            "Inspect the execution journal before starting another generation.",
                            false,
                            false,
                            true,
                            artifact_commit_is_indeterminate,
                            None,
                            None,
                        ),
                        provider_request_id,
                        provider_http_status,
                        started,
                    )
                    .await;
            }
            Ok(ImageGenerationExecutionMutationOutcome::StateConflict(None)) => {
                return Err(service_error(
                    ImageGenerationExecutionServiceErrorCode::JournalCorrupt,
                    "claimed image-generation execution disappeared from its durable journal",
                    false,
                ));
            }
            Err(_) => {
                match storage_inspect(
                    Arc::clone(&self.storage),
                    claimed.identity.execution_id.clone(),
                )
                .await
                {
                    Ok(Some(record)) if record.status.is_terminal() => {
                        return self.result_from_existing(record).await;
                    }
                    Ok(Some(record))
                        if record.status == StoredImageGenerationExecutionStatus::Publishing
                            && record.artifact.as_ref().is_some_and(|artifact| {
                                candidate_from_journal(artifact).as_ref()
                                    == Ok(prepared_artifact.candidate())
                            }) =>
                    {
                        record
                    }
                    Ok(Some(record)) => {
                        return self
                            .finalize_failure(
                                &record,
                                ImageGenerationExecutionStatus::Failed,
                                failure(
                                    ImageGenerationExecutionFailureCode::JournalUnavailable,
                                    ImageGenerationExecutionPhase::Journal,
                                    "image-generation Artifact candidate could not be durably recorded",
                                    "Retry with a new execution id after storage becomes available.",
                                    false,
                                    false,
                                    true,
                                    false,
                                    None,
                                    None,
                                ),
                                provider_request_id,
                                provider_http_status,
                                started,
                            )
                            .await;
                    }
                    Ok(None) => {
                        return Err(service_error(
                            ImageGenerationExecutionServiceErrorCode::JournalCorrupt,
                            "claimed image-generation execution disappeared from its durable journal",
                            false,
                        ));
                    }
                    Err(_) => {
                        return Err(service_error(
                            ImageGenerationExecutionServiceErrorCode::CommitIndeterminate,
                            "image-generation Artifact journal outcome is indeterminate",
                            false,
                        ));
                    }
                }
            }
        };

        let artifacts = Arc::clone(&self.artifacts);
        let publish_cancellation = cancellation.clone();
        let publish_task = tokio::task::spawn_blocking(move || {
            artifacts.publish(prepared_artifact, &publish_cancellation)
        });
        let published = match publish_task.await {
            Ok(result) => result,
            Err(_) => {
                return self
                    .finalize_failure(
                        &publishing,
                        ImageGenerationExecutionStatus::CommitIndeterminate,
                        failure(
                            ImageGenerationExecutionFailureCode::CommitIndeterminate,
                            ImageGenerationExecutionPhase::ArtifactPublish,
                            "image-generation Artifact publisher stopped at an unknown commit point",
                            "Inspect managed Artifact storage before retrying.",
                            false,
                            false,
                            true,
                            true,
                            None,
                            Some(ImageArtifactErrorCode::CommitIndeterminate),
                        ),
                        provider_request_id,
                        provider_http_status,
                        started,
                    )
                    .await;
            }
        };
        let published = match published {
            Ok(published) => published,
            Err(error) => {
                return self
                    .finalize_artifact_failure(
                        &publishing,
                        error,
                        provider_request_id,
                        provider_http_status,
                        started,
                        ImageGenerationExecutionPhase::ArtifactPublish,
                    )
                    .await;
            }
        };
        let receipt = receipt(
            &publishing,
            ImageGenerationExecutionStatus::Succeeded,
            provider_request_id,
            provider_http_status,
            Some(published.candidate.clone()),
            None,
            started,
        );
        self.finalize_receipt(receipt, Some(published)).await
    }

    async fn finalize_artifact_failure(
        &self,
        claimed: &ImageGenerationExecutionJournalRecord,
        error: ImageArtifactError,
        provider_request_id: Option<String>,
        http_status: Option<u16>,
        started: Instant,
        phase: ImageGenerationExecutionPhase,
    ) -> Result<ImageGenerationExecutionResult, ImageGenerationExecutionServiceError> {
        let status = match error.code {
            ImageArtifactErrorCode::Cancelled => ImageGenerationExecutionStatus::Cancelled,
            ImageArtifactErrorCode::CommitIndeterminate => {
                ImageGenerationExecutionStatus::CommitIndeterminate
            }
            _ => ImageGenerationExecutionStatus::Failed,
        };
        self.finalize_failure(
            claimed,
            status,
            failure(
                if error.code == ImageArtifactErrorCode::CommitIndeterminate {
                    ImageGenerationExecutionFailureCode::CommitIndeterminate
                } else if error.code == ImageArtifactErrorCode::Cancelled {
                    ImageGenerationExecutionFailureCode::Cancelled
                } else {
                    ImageGenerationExecutionFailureCode::ArtifactFailed
                },
                phase,
                artifact_failure_message(error.code),
                if error.commit_may_have_succeeded {
                    "Inspect the managed Artifact state before retrying."
                } else {
                    "Start a new execution only if another generated image is desired."
                },
                false,
                false,
                true,
                error.commit_may_have_succeeded,
                None,
                Some(error.code),
            ),
            provider_request_id,
            http_status,
            started,
        )
        .await
    }

    async fn finalize_failure(
        &self,
        claimed: &ImageGenerationExecutionJournalRecord,
        status: ImageGenerationExecutionStatus,
        error: ImageGenerationExecutionFailure,
        provider_request_id: Option<String>,
        http_status: Option<u16>,
        started: Instant,
    ) -> Result<ImageGenerationExecutionResult, ImageGenerationExecutionServiceError> {
        let receipt = receipt(
            claimed,
            status,
            provider_request_id,
            http_status,
            claimed
                .artifact
                .as_ref()
                .map(candidate_from_journal)
                .transpose()?,
            Some(error),
            started,
        );
        self.finalize_receipt(receipt, None).await
    }

    async fn finalize_receipt(
        &self,
        receipt: ImageGenerationExecutionReceipt,
        managed_artifact: Option<PublishedImageArtifact>,
    ) -> Result<ImageGenerationExecutionResult, ImageGenerationExecutionServiceError> {
        let terminal_json = serde_json::to_string(&receipt).map_err(|_| {
            service_error(
                ImageGenerationExecutionServiceErrorCode::JournalCorrupt,
                "image-generation terminal receipt could not be encoded",
                false,
            )
        })?;
        let error = receipt.error.as_ref();
        let update = ImageGenerationExecutionTerminalUpdate {
            expected_request_fingerprint: receipt.request_fingerprint.clone(),
            expected_artifact_sha256: receipt
                .artifact
                .as_ref()
                .map(|artifact| artifact.sha256.clone()),
            status: receipt.status.stored(),
            remote_outcome_unknown: error.is_some_and(|error| error.generation_may_have_succeeded),
            provider_succeeded: receipt.status == ImageGenerationExecutionStatus::Succeeded
                || error.is_some_and(|error| error.provider_succeeded),
            commit_may_have_succeeded: error
                .is_some_and(|error| error.artifact_commit_may_have_succeeded),
            provider_request_id: receipt.provider_request_id.clone(),
            http_status: receipt.http_status,
            terminal_result_json: terminal_json,
        };
        let execution_id = receipt.execution_id.clone();
        let outcome = storage_finalize(
            Arc::clone(&self.storage),
            execution_id.clone(),
            update.clone(),
        )
        .await;
        match outcome {
            Ok(ImageGenerationExecutionMutationOutcome::Updated(record))
            | Ok(ImageGenerationExecutionMutationOutcome::AlreadyCurrent(record)) => {
                let authoritative = parse_terminal_receipt(&record)?;
                Ok(ImageGenerationExecutionResult {
                    receipt: authoritative,
                    managed_artifact,
                })
            }
            Ok(ImageGenerationExecutionMutationOutcome::StateConflict(Some(record)))
                if record.status.is_terminal() =>
            {
                if record.identity.request_fingerprint != receipt.request_fingerprint {
                    return Err(service_error(
                        ImageGenerationExecutionServiceErrorCode::IdempotencyConflict,
                        "image-generation execution journal identity changed before finalization",
                        false,
                    ));
                }
                let authoritative = parse_terminal_receipt(&record)?;
                Ok(ImageGenerationExecutionResult {
                    receipt: authoritative,
                    managed_artifact,
                })
            }
            Ok(ImageGenerationExecutionMutationOutcome::StateConflict(_)) | Err(_) => {
                // The provider or filesystem side effect may already have happened. Inspect and
                // retry only the exact terminal journal commit; never replay provider execution.
                let inspected =
                    storage_inspect(Arc::clone(&self.storage), execution_id.clone()).await;
                if let Ok(Some(record)) = inspected {
                    if record.status.is_terminal() {
                        if record.identity.request_fingerprint != receipt.request_fingerprint {
                            return Err(service_error(
                                ImageGenerationExecutionServiceErrorCode::IdempotencyConflict,
                                "image-generation execution journal identity changed before finalization",
                                false,
                            ));
                        }
                        return Ok(ImageGenerationExecutionResult {
                            receipt: parse_terminal_receipt(&record)?,
                            managed_artifact,
                        });
                    }
                    if record.identity.request_fingerprint == receipt.request_fingerprint {
                        if let Ok(
                            ImageGenerationExecutionMutationOutcome::Updated(record)
                            | ImageGenerationExecutionMutationOutcome::AlreadyCurrent(record),
                        ) =
                            storage_finalize(Arc::clone(&self.storage), execution_id, update).await
                        {
                            return Ok(ImageGenerationExecutionResult {
                                receipt: parse_terminal_receipt(&record)?,
                                managed_artifact,
                            });
                        }
                    }
                }
                Err(service_error(
                    ImageGenerationExecutionServiceErrorCode::CommitIndeterminate,
                    "image-generation terminal receipt commit outcome is indeterminate",
                    false,
                ))
            }
        }
    }

    async fn result_from_existing(
        &self,
        record: ImageGenerationExecutionJournalRecord,
    ) -> Result<ImageGenerationExecutionResult, ImageGenerationExecutionServiceError> {
        if !record.status.is_terminal() {
            return Err(service_error(
                ImageGenerationExecutionServiceErrorCode::AlreadyClaimed,
                "image-generation execution is already claimed and will not be replayed",
                false,
            ));
        }
        if record.status == StoredImageGenerationExecutionStatus::CommitIndeterminate {
            return self.reconcile_indeterminate_commit(record).await;
        }
        let receipt = parse_terminal_receipt(&record)?;
        let managed_artifact = if receipt.status == ImageGenerationExecutionStatus::Succeeded {
            let candidate = receipt.artifact.as_ref().ok_or_else(|| {
                service_error(
                    ImageGenerationExecutionServiceErrorCode::JournalCorrupt,
                    "successful image-generation receipt has no Artifact",
                    false,
                )
            })?;
            Some(
                artifact_inspect(Arc::clone(&self.artifacts), candidate.clone())
                    .await?
                    .ok_or_else(|| {
                        service_error(
                            ImageGenerationExecutionServiceErrorCode::CommitIndeterminate,
                            "successful image-generation Artifact is missing from managed storage",
                            false,
                        )
                    })?,
            )
        } else {
            None
        };
        Ok(ImageGenerationExecutionResult {
            receipt,
            managed_artifact,
        })
    }

    async fn reconcile_indeterminate_commit(
        &self,
        record: ImageGenerationExecutionJournalRecord,
    ) -> Result<ImageGenerationExecutionResult, ImageGenerationExecutionServiceError> {
        let original = parse_terminal_receipt(&record)?;
        let candidate = record
            .artifact
            .as_ref()
            .map(candidate_from_journal)
            .transpose()?
            .ok_or_else(|| {
                service_error(
                    ImageGenerationExecutionServiceErrorCode::JournalCorrupt,
                    "indeterminate image-generation commit has no frozen Artifact candidate",
                    false,
                )
            })?;
        if original.artifact.as_ref() != Some(&candidate) {
            return Err(service_error(
                ImageGenerationExecutionServiceErrorCode::JournalCorrupt,
                "indeterminate image-generation receipt does not match its Artifact journal",
                false,
            ));
        }
        match artifact_confirm_published(Arc::clone(&self.artifacts), candidate.clone()).await {
            Ok(Some(published)) => {
                let receipt = recovery_success_receipt(&record, candidate)?;
                self.finalize_receipt(receipt, Some(published)).await
            }
            Ok(None) => {
                let receipt = recovery_receipt(
                    &record,
                    ImageGenerationExecutionStatus::Failed,
                    failure(
                        ImageGenerationExecutionFailureCode::ExecutionInterrupted,
                        ImageGenerationExecutionPhase::Recovery,
                        "image-generation Artifact was not present when an indeterminate commit was reconciled",
                        "Start a new execution only if another generated image is desired.",
                        false,
                        false,
                        true,
                        false,
                        None,
                        None,
                    ),
                    Some(candidate),
                )?;
                self.finalize_receipt(receipt, None).await
            }
            Err(_) => Ok(ImageGenerationExecutionResult {
                receipt: original,
                managed_artifact: None,
            }),
        }
    }

    pub async fn reconcile_interrupted(
        &self,
    ) -> Result<ImageGenerationExecutionRecoveryReport, ImageGenerationExecutionServiceError> {
        let records = storage_list_interrupted(Arc::clone(&self.storage)).await?;
        let mut report = ImageGenerationExecutionRecoveryReport::default();
        for record in records {
            match record.status {
                StoredImageGenerationExecutionStatus::Executing => {
                    let receipt = recovery_receipt(
                        &record,
                        ImageGenerationExecutionStatus::OutcomeIndeterminate,
                        failure(
                            ImageGenerationExecutionFailureCode::ExecutionInterrupted,
                            ImageGenerationExecutionPhase::Recovery,
                            "image-generation execution was interrupted before a local Artifact candidate was recorded",
                            "Inspect provider usage before starting a new execution.",
                            false,
                            true,
                            false,
                            false,
                            None,
                            None,
                        ),
                        None,
                    )?;
                    self.finalize_receipt(receipt, None).await?;
                    report.remote_outcome_unknown += 1;
                }
                StoredImageGenerationExecutionStatus::Publishing => {
                    let candidate = record
                        .artifact
                        .as_ref()
                        .map(candidate_from_journal)
                        .transpose()?
                        .ok_or_else(|| {
                            service_error(
                                ImageGenerationExecutionServiceErrorCode::JournalCorrupt,
                                "publishing image-generation execution has no Artifact candidate",
                                false,
                            )
                        })?;
                    match artifact_confirm_published(Arc::clone(&self.artifacts), candidate.clone())
                        .await
                    {
                        Ok(Some(published)) => {
                            let receipt = recovery_success_receipt(&record, candidate)?;
                            self.finalize_receipt(receipt, Some(published)).await?;
                            report.recovered_artifacts += 1;
                        }
                        Ok(None) => {
                            let receipt = recovery_receipt(
                                &record,
                                ImageGenerationExecutionStatus::Failed,
                                failure(
                                    ImageGenerationExecutionFailureCode::ExecutionInterrupted,
                                    ImageGenerationExecutionPhase::Recovery,
                                    "image-generation execution was interrupted before Artifact publication",
                                    "Start a new execution only if another generated image is desired.",
                                    false,
                                    false,
                                    true,
                                    false,
                                    None,
                                    None,
                                ),
                                Some(candidate),
                            )?;
                            self.finalize_receipt(receipt, None).await?;
                            report.interrupted_before_publish += 1;
                        }
                        Err(_) => {
                            let receipt = recovery_receipt(
                                &record,
                                ImageGenerationExecutionStatus::CommitIndeterminate,
                                failure(
                                    ImageGenerationExecutionFailureCode::CommitIndeterminate,
                                    ImageGenerationExecutionPhase::Recovery,
                                    "image-generation Artifact state could not be reconciled safely",
                                    "Inspect managed Artifact storage before retrying.",
                                    false,
                                    false,
                                    true,
                                    true,
                                    None,
                                    Some(ImageArtifactErrorCode::CommitIndeterminate),
                                ),
                                Some(candidate),
                            )?;
                            self.finalize_receipt(receipt, None).await?;
                            report.commit_indeterminate += 1;
                        }
                    }
                }
                StoredImageGenerationExecutionStatus::CommitIndeterminate => {
                    let result = self.reconcile_indeterminate_commit(record).await?;
                    match result.receipt.status {
                        ImageGenerationExecutionStatus::Succeeded => {
                            report.recovered_artifacts += 1;
                        }
                        ImageGenerationExecutionStatus::Failed => {
                            report.interrupted_before_publish += 1;
                        }
                        ImageGenerationExecutionStatus::CommitIndeterminate => {
                            report.commit_indeterminate += 1;
                        }
                        _ => {
                            return Err(service_error(
                                ImageGenerationExecutionServiceErrorCode::JournalCorrupt,
                                "indeterminate image-generation commit resolved to an invalid status",
                                false,
                            ));
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(report)
    }

    pub async fn shutdown(&self, grace: Duration) -> ImageGenerationExecutionShutdownReport {
        self.accepting.store(false, Ordering::SeqCst);
        let tokens = self
            .active
            .lock()
            .map(|active| active.values().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        for token in &tokens {
            token.cancel();
        }
        let wait = async {
            loop {
                if self.active.lock().map_or(true, |active| active.is_empty()) {
                    break;
                }
                self.active_changed.notified().await;
            }
        };
        let timed_out = tokio::time::timeout(grace, wait).await.is_err();
        ImageGenerationExecutionShutdownReport {
            cancelled_executions: tokens.len(),
            timed_out,
        }
    }

    fn register_active(
        &self,
        execution_id: &ImageGenerationExecutionId,
        cancellation: AgentCancellationToken,
    ) -> Result<ActiveExecutionGuard, ImageGenerationExecutionServiceError> {
        let mut active = self.active.lock().map_err(|_| {
            service_error(
                ImageGenerationExecutionServiceErrorCode::ConfigurationUnavailable,
                "image-generation active execution registry is unavailable",
                true,
            )
        })?;
        if active.contains_key(execution_id.as_str()) {
            return Err(service_error(
                ImageGenerationExecutionServiceErrorCode::AlreadyClaimed,
                "image-generation execution is already active",
                false,
            ));
        }
        active.insert(execution_id.as_str().to_string(), cancellation);
        Ok(ActiveExecutionGuard {
            execution_id: execution_id.as_str().to_string(),
            active: Arc::clone(&self.active),
            changed: Arc::clone(&self.active_changed),
        })
    }
}

struct ActiveExecutionGuard {
    execution_id: String,
    active: Arc<Mutex<HashMap<String, AgentCancellationToken>>>,
    changed: Arc<Notify>,
}

impl Drop for ActiveExecutionGuard {
    fn drop(&mut self) {
        if let Ok(mut active) = self.active.lock() {
            active.remove(&self.execution_id);
        }
        self.changed.notify_waiters();
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SafeExecutionRequest {
    schema_version: u32,
    operation: ImageGenerationOperation,
    prompt_sha256: String,
    prompt_bytes: usize,
    inputs: Vec<SafeImageInput>,
    size_preset: String,
    watermark: bool,
    output_count: u8,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SafeImageInput {
    media_type: String,
    size_bytes: usize,
    sha256: String,
}

fn execution_identity(
    execution_id: &ImageGenerationExecutionId,
    prepared: &PreparedImageGenerationRequest,
) -> Result<ImageGenerationExecutionIdentityRecord, ImageGenerationExecutionServiceError> {
    let normalized = prepared.normalized();
    let prompt = normalized.prompt().ok_or_else(|| {
        service_error(
            ImageGenerationExecutionServiceErrorCode::InvalidRequest,
            "image-generation execution requires a prompt",
            false,
        )
    })?;
    let safe = SafeExecutionRequest {
        schema_version: SAFE_EXECUTION_REQUEST_SCHEMA_VERSION,
        operation: normalized.operation(),
        prompt_sha256: prefixed_sha256(prompt.as_bytes()),
        prompt_bytes: prompt.len(),
        inputs: normalized
            .inputs()
            .iter()
            .map(|input| SafeImageInput {
                media_type: input.media_type().as_str().to_string(),
                size_bytes: input.decoded_size_bytes(),
                sha256: input.sha256().to_string(),
            })
            .collect(),
        size_preset: normalized.size_preset().to_string(),
        watermark: normalized.watermark(),
        output_count: normalized.output_count(),
    };
    let safe_request_json = serde_json::to_string(&safe).map_err(|_| {
        service_error(
            ImageGenerationExecutionServiceErrorCode::InvalidRequest,
            "image-generation request identity could not be encoded",
            false,
        )
    })?;
    let mut fingerprint = Sha256::new();
    fingerprint.update(b"mycopilot.image-generation.execution.v1\0");
    for value in [
        prepared.provider_profile_id().as_bytes(),
        prepared.adapter_id().as_str().as_bytes(),
        &prepared.profile_revision().to_be_bytes(),
        prepared.endpoint_url().as_bytes(),
        prepared.model_id().as_bytes(),
        safe_request_json.as_bytes(),
    ] {
        fingerprint.update((value.len() as u64).to_be_bytes());
        fingerprint.update(value);
    }
    Ok(ImageGenerationExecutionIdentityRecord {
        execution_id: execution_id.as_str().to_string(),
        request_fingerprint: format!("sha256:{:x}", fingerprint.finalize()),
        safe_request_json,
        profile_id: prepared.provider_profile_id().to_string(),
        adapter_id: prepared.adapter_id().as_str().to_string(),
        profile_revision: prepared.profile_revision(),
        model_id: prepared.model_id().to_string(),
        operation: operation_name(normalized.operation()).to_string(),
    })
}

fn request_matches_safe_identity(request: &ImageGenerationRequest, encoded: &str) -> bool {
    let Ok(safe) = serde_json::from_str::<SafeExecutionRequest>(encoded) else {
        return false;
    };
    if safe.schema_version != SAFE_EXECUTION_REQUEST_SCHEMA_VERSION
        || safe.output_count != 1
        || safe.inputs.len() > 1
    {
        return false;
    }
    let (operation, prompt, explicit_size, inputs) = match request {
        ImageGenerationRequest::Generate(request) => (
            ImageGenerationOperation::Generate,
            request.prompt.trim(),
            request.size_preset,
            &[][..],
        ),
        ImageGenerationRequest::Edit(request) => (
            ImageGenerationOperation::Edit,
            request.prompt.trim(),
            request.size_preset,
            request.inputs.as_slice(),
        ),
        ImageGenerationRequest::Status => return false,
    };
    if safe.operation != operation
        || safe.prompt_bytes != prompt.len()
        || safe.prompt_sha256 != prefixed_sha256(prompt.as_bytes())
        || explicit_size.is_some_and(|size| safe.size_preset != size.to_string())
        || safe.inputs.len() != inputs.len()
    {
        return false;
    }
    safe.inputs.iter().zip(inputs).all(|(expected, actual)| {
        expected.media_type == actual.media_type().as_str()
            && expected.size_bytes == actual.decoded_size_bytes()
            && expected.sha256 == actual.sha256()
    })
}

fn validate_provider_result(
    prepared: &PreparedImageGenerationRequest,
    result: &ImageGenerationResult,
) -> Result<(), ImageGenerationError> {
    if result.status != ImageGenerationResultStatus::Succeeded
        || result.provider_profile_id != prepared.provider_profile_id()
        || &result.adapter_id != prepared.adapter_id()
        || result.profile_revision != prepared.profile_revision()
        || result.model_id != prepared.model_id()
        || result.operation != prepared.normalized().operation()
        || result.outputs.len() != 1
        || result.capabilities.is_some()
    {
        return Err(ImageGenerationError::new(
            ImageGenerationErrorCode::InvalidResponse,
            "image-generation provider result does not match the frozen execution",
            false,
        ));
    }
    Ok(())
}

fn validate_prepared_profile(
    profile: &super::types::ImageGenerationProviderProfile,
    prepared: &PreparedImageGenerationRequest,
) -> Result<(), ImageGenerationError> {
    if prepared.provider_profile_id() != profile.id
        || prepared.adapter_id() != &profile.adapter_id
        || prepared.profile_revision() != profile.revision
        || prepared.endpoint_url() != profile.endpoint_url
        || prepared.model_id() != profile.model_id
    {
        return Err(ImageGenerationError::invalid_configuration(
            "image-generation prepared request does not match the credential-bound profile",
        ));
    }
    Ok(())
}

fn provider_outcome_unknown(error: &ImageGenerationError) -> bool {
    error.http_status.is_none()
        && (error.code == ImageGenerationErrorCode::RequestTimedOut
            || (error.code == ImageGenerationErrorCode::TransportFailed && !error.retryable))
}

fn artifact_journal_record(
    candidate: &ImageGenerationArtifactCandidate,
) -> ImageGenerationArtifactJournalRecord {
    ImageGenerationArtifactJournalRecord {
        ordinal: 0,
        artifact_id: candidate.artifact_id.clone(),
        state: StoredImageGenerationArtifactState::Candidate,
        storage_relative_path: candidate.storage_relative_path.clone(),
        format: format_name(candidate.format).to_string(),
        media_type: candidate.media_type.clone(),
        width: candidate.width,
        height: candidate.height,
        size_bytes: candidate.size_bytes,
        sha256: candidate.sha256.clone(),
        created_at: 0,
        published_at: None,
    }
}

fn candidate_from_journal(
    record: &ImageGenerationArtifactJournalRecord,
) -> Result<ImageGenerationArtifactCandidate, ImageGenerationExecutionServiceError> {
    let format = match record.format.as_str() {
        "png" => ImageArtifactFormat::Png,
        "jpeg" => ImageArtifactFormat::Jpeg,
        "webp" => ImageArtifactFormat::Webp,
        _ => {
            return Err(service_error(
                ImageGenerationExecutionServiceErrorCode::JournalCorrupt,
                "image-generation Artifact journal contains an unsupported format",
                false,
            ))
        }
    };
    Ok(ImageGenerationArtifactCandidate {
        artifact_id: record.artifact_id.clone(),
        storage_relative_path: record.storage_relative_path.clone(),
        format,
        media_type: record.media_type.clone(),
        width: record.width,
        height: record.height,
        size_bytes: record.size_bytes,
        sha256: record.sha256.clone(),
    })
}

fn receipt(
    claimed: &ImageGenerationExecutionJournalRecord,
    status: ImageGenerationExecutionStatus,
    provider_request_id: Option<String>,
    http_status: Option<u16>,
    artifact: Option<ImageGenerationArtifactCandidate>,
    error: Option<ImageGenerationExecutionFailure>,
    started: Instant,
) -> ImageGenerationExecutionReceipt {
    let completed_at = now_ms().max(claimed.created_at);
    ImageGenerationExecutionReceipt {
        schema_version: IMAGE_GENERATION_EXECUTION_RECEIPT_SCHEMA_VERSION,
        execution_id: claimed.identity.execution_id.clone(),
        request_fingerprint: claimed.identity.request_fingerprint.clone(),
        status,
        provider_profile_id: claimed.identity.profile_id.clone(),
        adapter_id: claimed.identity.adapter_id.clone(),
        profile_revision: claimed.identity.profile_revision,
        model_id: claimed.identity.model_id.clone(),
        operation: parse_operation(&claimed.identity.operation)
            .expect("claimed operation was validated before insertion"),
        provider_request_id,
        http_status,
        artifact,
        error,
        created_at: claimed.created_at,
        completed_at,
        duration_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
    }
}

fn recovery_receipt(
    record: &ImageGenerationExecutionJournalRecord,
    status: ImageGenerationExecutionStatus,
    error: ImageGenerationExecutionFailure,
    artifact: Option<ImageGenerationArtifactCandidate>,
) -> Result<ImageGenerationExecutionReceipt, ImageGenerationExecutionServiceError> {
    Ok(ImageGenerationExecutionReceipt {
        schema_version: IMAGE_GENERATION_EXECUTION_RECEIPT_SCHEMA_VERSION,
        execution_id: record.identity.execution_id.clone(),
        request_fingerprint: record.identity.request_fingerprint.clone(),
        status,
        provider_profile_id: record.identity.profile_id.clone(),
        adapter_id: record.identity.adapter_id.clone(),
        profile_revision: record.identity.profile_revision,
        model_id: record.identity.model_id.clone(),
        operation: parse_operation(&record.identity.operation).ok_or_else(|| {
            service_error(
                ImageGenerationExecutionServiceErrorCode::JournalCorrupt,
                "image-generation execution journal contains an invalid operation",
                false,
            )
        })?,
        provider_request_id: record.provider_request_id.clone(),
        http_status: record.http_status,
        artifact,
        error: Some(error),
        created_at: record.created_at,
        completed_at: now_ms().max(record.created_at),
        duration_ms: u64::try_from(now_ms().saturating_sub(record.created_at)).unwrap_or(0),
    })
}

fn recovery_success_receipt(
    record: &ImageGenerationExecutionJournalRecord,
    artifact: ImageGenerationArtifactCandidate,
) -> Result<ImageGenerationExecutionReceipt, ImageGenerationExecutionServiceError> {
    let mut receipt = recovery_receipt(
        record,
        ImageGenerationExecutionStatus::Succeeded,
        failure(
            ImageGenerationExecutionFailureCode::ExecutionInterrupted,
            ImageGenerationExecutionPhase::Recovery,
            "recovered",
            "No recovery is required.",
            false,
            false,
            true,
            false,
            None,
            None,
        ),
        Some(artifact),
    )?;
    receipt.error = None;
    Ok(receipt)
}

fn parse_terminal_receipt(
    record: &ImageGenerationExecutionJournalRecord,
) -> Result<ImageGenerationExecutionReceipt, ImageGenerationExecutionServiceError> {
    let value = record.terminal_result_json.as_deref().ok_or_else(|| {
        service_error(
            ImageGenerationExecutionServiceErrorCode::JournalCorrupt,
            "terminal image-generation journal record has no receipt",
            false,
        )
    })?;
    let receipt = serde_json::from_str::<ImageGenerationExecutionReceipt>(value).map_err(|_| {
        service_error(
            ImageGenerationExecutionServiceErrorCode::JournalCorrupt,
            "image-generation terminal receipt is invalid",
            false,
        )
    })?;
    if receipt.schema_version != IMAGE_GENERATION_EXECUTION_RECEIPT_SCHEMA_VERSION
        || receipt.execution_id != record.identity.execution_id
        || receipt.request_fingerprint != record.identity.request_fingerprint
        || receipt.status.stored() != record.status
    {
        return Err(service_error(
            ImageGenerationExecutionServiceErrorCode::JournalCorrupt,
            "image-generation terminal receipt identity does not match its journal record",
            false,
        ));
    }
    Ok(receipt)
}

#[allow(clippy::too_many_arguments)]
fn failure(
    code: ImageGenerationExecutionFailureCode,
    phase: ImageGenerationExecutionPhase,
    message: &str,
    recovery: &str,
    retryable: bool,
    generation_may_have_succeeded: bool,
    provider_succeeded: bool,
    artifact_commit_may_have_succeeded: bool,
    provider_error_code: Option<ImageGenerationErrorCode>,
    artifact_error_code: Option<ImageArtifactErrorCode>,
) -> ImageGenerationExecutionFailure {
    ImageGenerationExecutionFailure {
        code,
        phase,
        message: bounded_message(message),
        recovery: bounded_message(recovery),
        retryable,
        generation_may_have_succeeded,
        provider_succeeded,
        artifact_commit_may_have_succeeded,
        provider_error_code,
        artifact_error_code,
    }
}

fn bounded_message(value: &str) -> String {
    value.chars().take(512).collect()
}

fn map_configuration_error(
    error: ImageGenerationConfigurationError,
) -> ImageGenerationExecutionServiceError {
    match error {
        ImageGenerationConfigurationError::ConfigurationIncomplete(
            ImageGenerationReadiness::Disabled,
        ) => service_error(
            ImageGenerationExecutionServiceErrorCode::ConfigurationDisabled,
            "image-generation provider is disabled",
            false,
        ),
        ImageGenerationConfigurationError::ConfigurationIncomplete(_)
        | ImageGenerationConfigurationError::MissingEndpoint
        | ImageGenerationConfigurationError::MissingModel
        | ImageGenerationConfigurationError::MissingCredential => service_error(
            ImageGenerationExecutionServiceErrorCode::ConfigurationIncomplete,
            "image-generation provider configuration is incomplete",
            false,
        ),
        ImageGenerationConfigurationError::StorageUnavailable
        | ImageGenerationConfigurationError::CredentialStoreUnavailable
        | ImageGenerationConfigurationError::Unavailable
        | ImageGenerationConfigurationError::CommitIndeterminate => service_error(
            ImageGenerationExecutionServiceErrorCode::ConfigurationUnavailable,
            "image-generation provider configuration is unavailable",
            true,
        ),
        ImageGenerationConfigurationError::RevisionConflict { .. } => service_error(
            ImageGenerationExecutionServiceErrorCode::ConfigurationUnavailable,
            "image-generation provider changed while its credential was being resolved",
            true,
        ),
        _ => service_error(
            ImageGenerationExecutionServiceErrorCode::InvalidConfiguration,
            "image-generation provider configuration is invalid",
            false,
        ),
    }
}

fn map_preparation_error(error: ImageGenerationError) -> ImageGenerationExecutionServiceError {
    let code = match error.code {
        ImageGenerationErrorCode::InvalidRequest
        | ImageGenerationErrorCode::UnsupportedOperation => {
            ImageGenerationExecutionServiceErrorCode::InvalidRequest
        }
        _ => ImageGenerationExecutionServiceErrorCode::InvalidConfiguration,
    };
    service_error(code, provider_failure_message(error.code), error.retryable)
}

fn provider_failure_message(code: ImageGenerationErrorCode) -> &'static str {
    match code {
        ImageGenerationErrorCode::InvalidConfiguration => {
            "image-generation provider configuration is invalid"
        }
        ImageGenerationErrorCode::InvalidRequest => "image-generation request is invalid",
        ImageGenerationErrorCode::UnsupportedOperation => {
            "image-generation operation is not supported by the active provider"
        }
        ImageGenerationErrorCode::AuthenticationFailed => {
            "image-generation provider rejected the configured credential"
        }
        ImageGenerationErrorCode::RateLimited => {
            "image-generation provider rate limit was exceeded"
        }
        ImageGenerationErrorCode::ProviderRejected => {
            "image-generation provider rejected the request"
        }
        ImageGenerationErrorCode::ProviderUnavailable => {
            "image-generation provider is temporarily unavailable"
        }
        ImageGenerationErrorCode::RequestTimedOut => "image-generation provider request timed out",
        ImageGenerationErrorCode::ResponseTooLarge => {
            "image-generation provider response exceeded the safety limit"
        }
        ImageGenerationErrorCode::InvalidResponse => {
            "image-generation provider returned an invalid response"
        }
        ImageGenerationErrorCode::TransportFailed => "image-generation provider transport failed",
        ImageGenerationErrorCode::ProfileConflict => {
            "image-generation provider changed after the request was prepared"
        }
    }
}

fn artifact_failure_message(code: ImageArtifactErrorCode) -> &'static str {
    match code {
        ImageArtifactErrorCode::InvalidConfiguration => {
            "image Artifact storage or transfer configuration is invalid"
        }
        ImageArtifactErrorCode::UnsafeUrl
        | ImageArtifactErrorCode::DnsRejected
        | ImageArtifactErrorCode::RedirectRejected => {
            "image Artifact URL was rejected by the network safety policy"
        }
        ImageArtifactErrorCode::TransportFailed => "image Artifact download failed",
        ImageArtifactErrorCode::DownloadTimedOut => "image Artifact download timed out",
        ImageArtifactErrorCode::HttpRejected => {
            "image Artifact server returned an unsuccessful status"
        }
        ImageArtifactErrorCode::ResponseTooLarge => {
            "image Artifact exceeded the configured byte limit"
        }
        ImageArtifactErrorCode::UnsupportedMediaType => {
            "image Artifact media type is unsupported or inconsistent"
        }
        ImageArtifactErrorCode::InvalidImage => "image Artifact could not be fully validated",
        ImageArtifactErrorCode::Io => "image Artifact storage operation failed",
        ImageArtifactErrorCode::Conflict => "image Artifact identity or content conflicted",
        ImageArtifactErrorCode::Cancelled => "image Artifact operation was cancelled",
        ImageArtifactErrorCode::CommitIndeterminate => {
            "image Artifact publication outcome is indeterminate"
        }
    }
}

fn opaque_provider_request_id(value: Option<&str>) -> Option<String> {
    let value = value?.trim();
    if value.is_empty() {
        return None;
    }
    if let Some(digest) = value.strip_prefix("sha256:") {
        if matches!(digest.len(), 32 | 64)
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Some(value.to_string());
        }
    }
    Some(format!("sha256:{:x}", Sha256::digest(value.as_bytes())))
}

fn service_error(
    code: ImageGenerationExecutionServiceErrorCode,
    message: &str,
    retryable: bool,
) -> ImageGenerationExecutionServiceError {
    ImageGenerationExecutionServiceError::new(code, bounded_message(message), retryable)
}

fn operation_name(operation: ImageGenerationOperation) -> &'static str {
    match operation {
        ImageGenerationOperation::Generate => "generate",
        ImageGenerationOperation::Edit => "edit",
        ImageGenerationOperation::Status => "status",
    }
}

fn parse_operation(value: &str) -> Option<ImageGenerationOperation> {
    match value {
        "generate" => Some(ImageGenerationOperation::Generate),
        "edit" => Some(ImageGenerationOperation::Edit),
        _ => None,
    }
}

fn format_name(format: ImageArtifactFormat) -> &'static str {
    match format {
        ImageArtifactFormat::Png => "png",
        ImageArtifactFormat::Jpeg => "jpeg",
        ImageArtifactFormat::Webp => "webp",
    }
}

fn prefixed_sha256(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

async fn storage_claim(
    storage: Arc<StorageService>,
    identity: ImageGenerationExecutionIdentityRecord,
) -> Result<ImageGenerationExecutionClaimOutcome, ImageGenerationExecutionServiceError> {
    tokio::task::spawn_blocking(move || storage.claim_image_generation_execution(&identity))
        .await
        .map_err(|_| journal_unavailable())?
        .map_err(|_| journal_unavailable())
}

async fn storage_prepare_artifact(
    storage: Arc<StorageService>,
    execution_id: String,
    artifact: ImageGenerationArtifactJournalRecord,
    provider_request_id: Option<String>,
    http_status: Option<u16>,
) -> Result<ImageGenerationExecutionMutationOutcome, ImageGenerationExecutionServiceError> {
    tokio::task::spawn_blocking(move || {
        storage.prepare_image_generation_artifact(
            &execution_id,
            &artifact,
            provider_request_id.as_deref(),
            http_status,
        )
    })
    .await
    .map_err(|_| journal_unavailable())?
    .map_err(|_| journal_unavailable())
}

async fn storage_finalize(
    storage: Arc<StorageService>,
    execution_id: String,
    update: ImageGenerationExecutionTerminalUpdate,
) -> Result<ImageGenerationExecutionMutationOutcome, ImageGenerationExecutionServiceError> {
    tokio::task::spawn_blocking(move || {
        storage.finalize_image_generation_execution(&execution_id, &update)
    })
    .await
    .map_err(|_| journal_unavailable())?
    .map_err(|_| journal_unavailable())
}

async fn storage_inspect(
    storage: Arc<StorageService>,
    execution_id: String,
) -> Result<Option<ImageGenerationExecutionJournalRecord>, ImageGenerationExecutionServiceError> {
    tokio::task::spawn_blocking(move || storage.inspect_image_generation_execution(&execution_id))
        .await
        .map_err(|_| journal_unavailable())?
        .map_err(|_| journal_unavailable())
}

async fn storage_list_interrupted(
    storage: Arc<StorageService>,
) -> Result<Vec<ImageGenerationExecutionJournalRecord>, ImageGenerationExecutionServiceError> {
    tokio::task::spawn_blocking(move || storage.list_interrupted_image_generation_executions())
        .await
        .map_err(|_| journal_unavailable())?
        .map_err(|_| journal_unavailable())
}

async fn artifact_inspect(
    artifacts: Arc<dyn ImageGenerationArtifactStore>,
    candidate: ImageGenerationArtifactCandidate,
) -> Result<Option<PublishedImageArtifact>, ImageGenerationExecutionServiceError> {
    tokio::task::spawn_blocking(move || artifacts.inspect(&candidate))
        .await
        .map_err(|_| {
            service_error(
                ImageGenerationExecutionServiceErrorCode::CommitIndeterminate,
                "image-generation Artifact verifier stopped unexpectedly",
                false,
            )
        })?
        .map_err(|_| {
            service_error(
                ImageGenerationExecutionServiceErrorCode::CommitIndeterminate,
                "image-generation Artifact could not be verified",
                false,
            )
        })
}

async fn artifact_confirm_published(
    artifacts: Arc<dyn ImageGenerationArtifactStore>,
    candidate: ImageGenerationArtifactCandidate,
) -> Result<Option<PublishedImageArtifact>, ImageGenerationExecutionServiceError> {
    tokio::task::spawn_blocking(move || artifacts.confirm_published(&candidate))
        .await
        .map_err(|_| {
            service_error(
                ImageGenerationExecutionServiceErrorCode::CommitIndeterminate,
                "image-generation Artifact durability verifier stopped unexpectedly",
                false,
            )
        })?
        .map_err(|_| {
            service_error(
                ImageGenerationExecutionServiceErrorCode::CommitIndeterminate,
                "image-generation Artifact durability could not be confirmed",
                false,
            )
        })
}

fn journal_unavailable() -> ImageGenerationExecutionServiceError {
    service_error(
        ImageGenerationExecutionServiceErrorCode::JournalUnavailable,
        "image-generation execution journal is unavailable",
        true,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image_generation::artifact::{
        ImageArtifactPublicationStatus, PreparedImageArtifact,
    };
    use crate::image_generation::credential_store::{CredentialSecret, InMemoryCredentialStore};
    use crate::image_generation::provider::{
        ImageGenerationProvider, ImageGenerationProviderFactory,
    };
    use crate::image_generation::types::{
        ImageGenerationAdapterId, ImageGenerationCapabilities, ImageGenerationDefaults,
        ImageGenerationGenerateRequest, ImageGenerationProviderProfile,
        ImageGenerationResultStatus, ImageGenerationUrlOutput,
    };
    use crate::image_generation::{
        ImageGenerationConfigurationUpdate, ImageGenerationCredentialMutation,
    };
    use futures_util::future::BoxFuture;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
    use tempfile::tempdir;

    struct TestProvider {
        profile: ImageGenerationProviderProfile,
        calls: Arc<AtomicUsize>,
        fail: bool,
        invalid_result: bool,
        invalid_prepared_profile: bool,
    }

    struct TestProviderFactory {
        calls: Arc<AtomicUsize>,
        fail: bool,
        invalid_result: bool,
        invalid_prepared_profile: bool,
    }

    impl ImageGenerationProviderFactory for TestProviderFactory {
        fn adapter_id(&self) -> ImageGenerationAdapterId {
            ImageGenerationAdapterId::SmartMlSeedream
        }

        fn create(
            &self,
            profile: ImageGenerationProviderProfile,
        ) -> Result<Arc<dyn ImageGenerationProvider>, ImageGenerationError> {
            Ok(Arc::new(TestProvider {
                profile,
                calls: Arc::clone(&self.calls),
                fail: self.fail,
                invalid_result: self.invalid_result,
                invalid_prepared_profile: self.invalid_prepared_profile,
            }))
        }
    }

    impl ImageGenerationProvider for TestProvider {
        fn profile(&self) -> &ImageGenerationProviderProfile {
            &self.profile
        }

        fn prepare(
            &self,
            request: ImageGenerationRequest,
        ) -> Result<PreparedImageGenerationRequest, ImageGenerationError> {
            let normalized = request.normalize(&self.profile)?;
            if self.invalid_prepared_profile {
                let mut mismatched = self.profile.clone();
                mismatched.endpoint_url = "https://other.example/images/generations".to_string();
                return Ok(PreparedImageGenerationRequest::new(&mismatched, normalized));
            }
            Ok(PreparedImageGenerationRequest::new(
                &self.profile,
                normalized,
            ))
        }

        fn execute<'a>(
            &'a self,
            request: &'a PreparedImageGenerationRequest,
            _credential: &'a CredentialSecret,
        ) -> BoxFuture<'a, Result<ImageGenerationResult, ImageGenerationError>> {
            Box::pin(async move {
                self.calls.fetch_add(1, AtomicOrdering::SeqCst);
                if self.fail {
                    return Err(ImageGenerationError::new(
                        ImageGenerationErrorCode::ProviderRejected,
                        "provider rejected the test request",
                        false,
                    )
                    .with_http_status(422));
                }
                let mut result = ImageGenerationResult {
                    status: ImageGenerationResultStatus::Succeeded,
                    provider_profile_id: self.profile.id.clone(),
                    adapter_id: self.profile.adapter_id.clone(),
                    profile_revision: self.profile.revision,
                    model_id: self.profile.model_id.clone(),
                    operation: request.normalized().operation(),
                    http_status: Some(200),
                    provider_request_id: Some(format!("sha256:{}", "c".repeat(32))),
                    outputs: vec![ImageGenerationUrlOutput::new(
                        "https://artifact.invalid/signed?secret=hidden".to_string(),
                    )],
                    capabilities: None,
                };
                if self.invalid_result {
                    result.model_id = "unexpected-model".to_string();
                }
                Ok(result)
            })
        }
    }

    struct TestArtifactStore {
        root: PathBuf,
        panic_after_publish: bool,
    }

    impl ImageGenerationArtifactStore for TestArtifactStore {
        fn stage<'a>(
            &'a self,
            _source: &'a ImageGenerationUrlOutput,
            _cancellation: &'a AgentCancellationToken,
        ) -> BoxFuture<'a, Result<PreparedImageArtifact, ImageArtifactError>> {
            Box::pin(async move {
                let digest = "b".repeat(64);
                let staging = self.root.join("staging");
                let target = self.root.join("target.png");
                fs::write(&staging, b"test").unwrap();
                Ok(PreparedImageArtifact::from_test_parts(
                    ImageGenerationArtifactCandidate {
                        artifact_id: format!("sha256:{digest}"),
                        storage_relative_path: format!("objects/{digest}.png"),
                        format: ImageArtifactFormat::Png,
                        media_type: "image/png".to_string(),
                        width: 2,
                        height: 3,
                        size_bytes: 4,
                        sha256: digest,
                    },
                    staging,
                    target,
                ))
            })
        }

        fn publish(
            &self,
            mut prepared: PreparedImageArtifact,
            _cancellation: &AgentCancellationToken,
        ) -> Result<PublishedImageArtifact, ImageArtifactError> {
            let candidate = prepared.candidate().clone();
            let target = self.root.join("target.png");
            let _ = fs::remove_file(self.root.join("staging"));
            fs::write(&target, b"test").unwrap();
            if self.panic_after_publish {
                panic!("injected publisher failure after publication");
            }
            prepared.mark_published_for_test();
            Ok(PublishedImageArtifact {
                candidate,
                absolute_path: target,
                status: ImageArtifactPublicationStatus::Created,
            })
        }

        fn inspect(
            &self,
            candidate: &ImageGenerationArtifactCandidate,
        ) -> Result<Option<PublishedImageArtifact>, ImageArtifactError> {
            let target = self.root.join("target.png");
            Ok(target.exists().then(|| PublishedImageArtifact {
                candidate: candidate.clone(),
                absolute_path: target,
                status: ImageArtifactPublicationStatus::AlreadyPresent,
            }))
        }

        fn confirm_published(
            &self,
            candidate: &ImageGenerationArtifactCandidate,
        ) -> Result<Option<PublishedImageArtifact>, ImageArtifactError> {
            self.inspect(candidate)
        }
    }

    struct Fixture {
        service: ImageGenerationExecutionService,
        calls: Arc<AtomicUsize>,
        directory: tempfile::TempDir,
    }

    fn fixture(provider_fails: bool) -> Fixture {
        fixture_with_behavior(provider_fails, false, false)
    }

    fn fixture_with_behavior(
        provider_fails: bool,
        invalid_result: bool,
        panic_after_publish: bool,
    ) -> Fixture {
        fixture_with_all_behavior(provider_fails, invalid_result, panic_after_publish, false)
    }

    fn fixture_with_all_behavior(
        provider_fails: bool,
        invalid_result: bool,
        panic_after_publish: bool,
        invalid_prepared_profile: bool,
    ) -> Fixture {
        let directory = tempdir().unwrap();
        let storage = Arc::new(StorageService::open(&directory.path().join("app.db")).unwrap());
        let credentials = Arc::new(InMemoryCredentialStore::default());
        let configuration = Arc::new(ImageGenerationConfigurationService::new(
            Arc::clone(&storage),
            credentials,
        ));
        let configured = configuration
            .update_configuration(ImageGenerationConfigurationUpdate {
                expected_revision: "image-generation:v1:0".to_string(),
                adapter_id: ImageGenerationAdapterId::SmartMlSeedream,
                endpoint_url: "https://provider.example/images/generations".to_string(),
                model_id: "seedream".to_string(),
                text_to_image: true,
                image_to_image: false,
                defaults: ImageGenerationDefaults::default(),
                credential_mutation: ImageGenerationCredentialMutation::Replace(
                    CredentialSecret::new("secret").unwrap(),
                ),
            })
            .unwrap()
            .configuration;
        configuration
            .set_enabled(&configured.revision, true)
            .unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let mut adapters = ImageGenerationAdapterRegistry::new();
        adapters
            .register(Arc::new(TestProviderFactory {
                calls: Arc::clone(&calls),
                fail: provider_fails,
                invalid_result,
                invalid_prepared_profile,
            }))
            .unwrap();
        let artifact_root = directory.path().join("artifacts");
        fs::create_dir(&artifact_root).unwrap();
        let artifacts = Arc::new(TestArtifactStore {
            root: artifact_root,
            panic_after_publish,
        });
        let service = ImageGenerationExecutionService::new(
            configuration,
            Arc::new(adapters),
            artifacts,
            storage,
            ImageGenerationExecutionLimits::default(),
        )
        .unwrap();
        Fixture {
            service,
            calls,
            directory,
        }
    }

    fn request(id: &str) -> ImageGenerationExecutionRequest {
        ImageGenerationExecutionRequest {
            execution_id: ImageGenerationExecutionId::parse(id).unwrap(),
            request: ImageGenerationRequest::Generate(ImageGenerationGenerateRequest::new(
                "a safe test image",
            )),
        }
    }

    #[tokio::test]
    async fn successful_execution_is_durable_and_idempotent() {
        let fixture = fixture(false);
        let first = fixture
            .service
            .execute(request("execution-success"), AgentCancellationToken::new())
            .await
            .unwrap();
        assert_eq!(
            first.receipt.status,
            ImageGenerationExecutionStatus::Succeeded
        );
        assert!(first.managed_artifact.is_some());
        assert_eq!(fixture.calls.load(AtomicOrdering::SeqCst), 1);

        let replay = fixture
            .service
            .execute(request("execution-success"), AgentCancellationToken::new())
            .await
            .unwrap();
        assert_eq!(replay.receipt, first.receipt);
        assert_eq!(fixture.calls.load(AtomicOrdering::SeqCst), 1);
        let database = fs::read(fixture.directory.path().join("app.db")).unwrap();
        let database = String::from_utf8_lossy(&database);
        assert!(!database.contains("a safe test image"));
        assert!(!database.contains("artifact.invalid"));
        assert!(!database.contains("signature=private"));
        assert!(!database.contains("secret"));
    }

    #[tokio::test]
    async fn provider_failure_is_persisted_and_not_replayed() {
        let fixture = fixture(true);
        let first = fixture
            .service
            .execute(request("execution-failed"), AgentCancellationToken::new())
            .await
            .unwrap();
        assert_eq!(first.receipt.status, ImageGenerationExecutionStatus::Failed);
        assert_eq!(fixture.calls.load(AtomicOrdering::SeqCst), 1);
        let replay = fixture
            .service
            .execute(request("execution-failed"), AgentCancellationToken::new())
            .await
            .unwrap();
        assert_eq!(replay.receipt, first.receipt);
        assert_eq!(fixture.calls.load(AtomicOrdering::SeqCst), 1);
    }

    #[tokio::test]
    async fn invalid_provider_result_is_persisted_and_not_left_executing() {
        let fixture = fixture_with_behavior(false, true, false);
        let first = fixture
            .service
            .execute(
                request("execution-invalid-result"),
                AgentCancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(first.receipt.status, ImageGenerationExecutionStatus::Failed);
        assert_eq!(
            first.receipt.error.as_ref().unwrap().provider_error_code,
            Some(ImageGenerationErrorCode::InvalidResponse)
        );
        let replay = fixture
            .service
            .execute(
                request("execution-invalid-result"),
                AgentCancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(replay.receipt, first.receipt);
        assert_eq!(fixture.calls.load(AtomicOrdering::SeqCst), 1);
    }

    #[tokio::test]
    async fn prepared_request_must_match_the_credential_bound_profile_before_execution() {
        let fixture = fixture_with_all_behavior(false, false, false, true);

        let error = fixture
            .service
            .execute(
                request("execution-invalid-prepared-profile"),
                AgentCancellationToken::new(),
            )
            .await
            .unwrap_err();

        assert_eq!(
            error.code,
            ImageGenerationExecutionServiceErrorCode::InvalidConfiguration
        );
        assert_eq!(fixture.calls.load(AtomicOrdering::SeqCst), 0);
    }

    #[tokio::test]
    async fn indeterminate_publish_retains_candidate_and_reconciles_without_provider_replay() {
        let fixture = fixture_with_behavior(false, false, true);
        let first = fixture
            .service
            .execute(
                request("execution-publish-panic"),
                AgentCancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(
            first.receipt.status,
            ImageGenerationExecutionStatus::CommitIndeterminate
        );
        assert!(first.receipt.artifact.is_some());

        let replay = fixture
            .service
            .execute(
                request("execution-publish-panic"),
                AgentCancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(
            replay.receipt.status,
            ImageGenerationExecutionStatus::Succeeded
        );
        assert!(replay.managed_artifact.is_some());
        assert_eq!(fixture.calls.load(AtomicOrdering::SeqCst), 1);
    }

    #[tokio::test]
    async fn execution_id_cannot_be_reused_for_different_inputs() {
        let fixture = fixture(false);
        fixture
            .service
            .execute(request("execution-conflict"), AgentCancellationToken::new())
            .await
            .unwrap();
        let mut different = request("execution-conflict");
        different.request = ImageGenerationRequest::Generate(ImageGenerationGenerateRequest::new(
            "different prompt",
        ));
        let error = fixture
            .service
            .execute(different, AgentCancellationToken::new())
            .await
            .unwrap_err();
        assert_eq!(
            error.code,
            ImageGenerationExecutionServiceErrorCode::IdempotencyConflict
        );
        assert_eq!(fixture.calls.load(AtomicOrdering::SeqCst), 1);
    }

    #[test]
    fn debug_and_safe_identity_do_not_contain_prompt_or_signed_url() {
        let profile = ImageGenerationProviderProfile::new(
            "default",
            ImageGenerationAdapterId::SmartMlSeedream,
            "https://provider.example/images/generations",
            "seedream",
            1,
            ImageGenerationCapabilities::smartml_seedream(false),
            ImageGenerationDefaults::default(),
        )
        .unwrap();
        let prepared = PreparedImageGenerationRequest::new(
            &profile,
            ImageGenerationRequest::Generate(ImageGenerationGenerateRequest::new("private prompt"))
                .normalize(&profile)
                .unwrap(),
        );
        let identity = execution_identity(
            &ImageGenerationExecutionId::parse("safe-debug").unwrap(),
            &prepared,
        )
        .unwrap();
        assert!(!identity.safe_request_json.contains("private prompt"));
        assert!(!format!("{prepared:?}").contains("private prompt"));
    }
}
