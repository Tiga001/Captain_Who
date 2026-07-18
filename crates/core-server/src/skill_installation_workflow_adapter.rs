//! Protocol boundary for the two-phase managed Skill installation workflow.

use mycopilot_core::skills::{
    GitHubCommit, GitHubReference, GitHubRepository, GitHubSkillLocation, GitHubSubdirectory,
    GitHubWorkflowAcquisitionAdapter, ManagedSkillInstallerErrorCode,
    SkillAcquisitionAdapterErrorCode, SkillAcquisitionSource, SkillId,
    SkillInstallationCommitRequest, SkillInstallationCommitResult, SkillInstallationMutation,
    SkillInstallationOperation, SkillInstallationOutcome, SkillInstallationPreparationRequest,
    SkillInstallationPreview, SkillInstallationWarningCode, SkillInstallationWorkflowError,
    SkillPreparationCancellation, SkillPreparationId, SkillPreviewRevision, SkillRevision,
};
use mycopilot_protocol_rs::{
    SkillAcquisitionSourceDto, SkillCompatibilityIssueDto, SkillCompatibilityIssueSeverityDto,
    SkillCompatibilityReportDto, SkillCompatibilityStatusDto, SkillGithubReferenceDto,
    SkillInspectionErrorCodeDto, SkillInspectionErrorData, SkillInspectionErrorTypeDto,
    SkillInspectionPhaseDto, SkillInspectionRecoveryDto, SkillInstallationChangeDto,
    SkillInstallationChangesDto, SkillInstallationCommitOutcomeDto,
    SkillInstallationCommitResponse, SkillInstallationIntentDto, SkillInstallationPreviewDto,
    SkillPackagePreviewDto, SkillPreparationCancellationOutcomeDto,
    SkillPreparationCancellationResponse, SkillPreparationOperationDto, SkillPreviewSourceDto,
    SkillsCancelPreparationRequest, SkillsCommitInstallationRequest,
    SkillsInspectInstallationRequest, SKILL_INSTALLATION_WORKFLOW_SCHEMA_VERSION,
};
use serde::Deserialize;
use std::collections::BTreeSet;

const CONTAINS_SCRIPTS_ISSUE_ID: &str = "containsScripts";

#[derive(Debug)]
pub(crate) struct SkillInspectionFailure {
    data: Box<SkillInspectionErrorData>,
}

impl SkillInspectionFailure {
    fn new(
        phase: SkillInspectionPhaseDto,
        code: SkillInspectionErrorCodeDto,
        recovery: SkillInspectionRecoveryDto,
        message: impl Into<String>,
        preparation_id: Option<String>,
        diagnostic_code: Option<String>,
    ) -> Self {
        Self {
            data: Box::new(SkillInspectionErrorData {
                error_type: SkillInspectionErrorTypeDto::SkillInspection,
                phase,
                code,
                recovery,
                message: message.into(),
                preparation_id,
                diagnostic_code,
                retry_after_ms: None,
            }),
        }
    }

    fn invalid_source(preparation_id: Option<String>, message: impl Into<String>) -> Self {
        Self::new(
            SkillInspectionPhaseDto::Inspect,
            SkillInspectionErrorCodeDto::InvalidSource,
            SkillInspectionRecoveryDto::FixSource,
            message,
            preparation_id,
            None,
        )
    }

    fn unsupported_source(preparation_id: Option<String>) -> Self {
        Self::new(
            SkillInspectionPhaseDto::Inspect,
            SkillInspectionErrorCodeDto::UnsupportedSource,
            SkillInspectionRecoveryDto::ChooseDifferentSource,
            "This Skill acquisition source is not supported by the current backend.",
            preparation_id,
            None,
        )
    }

    pub(crate) fn into_data(self) -> Box<SkillInspectionErrorData> {
        self.data
    }
}

impl std::fmt::Display for SkillInspectionFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.data.message)
    }
}

impl std::error::Error for SkillInspectionFailure {}

pub(crate) fn dispatch_failure(
    phase: SkillInspectionPhaseDto,
    preparation_id: String,
    message: impl Into<String>,
) -> SkillInspectionFailure {
    SkillInspectionFailure::new(
        phase,
        SkillInspectionErrorCodeDto::Unavailable,
        match phase {
            SkillInspectionPhaseDto::Inspect => SkillInspectionRecoveryDto::RetryLater,
            SkillInspectionPhaseDto::Commit | SkillInspectionPhaseDto::Cancel => {
                SkillInspectionRecoveryDto::RetrySamePreparation
            }
        },
        message,
        Some(preparation_id),
        None,
    )
}

pub(crate) fn preparation_request(
    input: SkillsInspectInstallationRequest,
) -> Result<SkillInstallationPreparationRequest, SkillInspectionFailure> {
    let preparation_id = SkillPreparationId::parse(input.preparation_id.clone()).map_err(|_| {
        SkillInspectionFailure::invalid_source(
            Some(input.preparation_id.clone()),
            "The preparation ID must be a canonical non-nil UUID.",
        )
    })?;
    let source = acquisition_source(&input.source, preparation_id.as_str())?;

    match input.intent {
        SkillInstallationIntentDto::Install {} => {
            // The client-generated preparation UUID is also a stable,
            // retry-safe installation identity. It is never a filesystem path.
            let installation_id =
                mycopilot_core::skills::SkillInstallationId::parse(preparation_id.as_str())
                    .expect("a canonical preparation UUID must be a valid installation UUID");
            Ok(SkillInstallationPreparationRequest::install(
                preparation_id,
                installation_id,
                source,
            ))
        }
        SkillInstallationIntentDto::Update {
            skill_id,
            expected_installation_revision,
        } => {
            let skill_id = SkillId::parse(skill_id).map_err(|_| {
                SkillInspectionFailure::invalid_source(
                    Some(preparation_id.as_str().to_string()),
                    "The update target is not a valid installed Skill ID.",
                )
            })?;
            let expected_revision =
                SkillRevision::parse(expected_installation_revision).map_err(|_| {
                    SkillInspectionFailure::invalid_source(
                        Some(preparation_id.as_str().to_string()),
                        "The expected installation revision is invalid.",
                    )
                })?;
            Ok(SkillInstallationPreparationRequest::update(
                preparation_id,
                skill_id,
                expected_revision,
                source,
            ))
        }
    }
}

fn acquisition_source(
    source: &SkillAcquisitionSourceDto,
    preparation_id: &str,
) -> Result<SkillAcquisitionSource, SkillInspectionFailure> {
    let preparation_id = Some(preparation_id.to_string());
    match source {
        SkillAcquisitionSourceDto::LocalDirectory { directory } => {
            let directory = std::path::PathBuf::from(directory);
            if !directory.is_absolute() {
                return Err(SkillInspectionFailure::invalid_source(
                    preparation_id,
                    "A local Skill directory must be an absolute path.",
                ));
            }
            Ok(SkillAcquisitionSource::local_directory(directory))
        }
        SkillAcquisitionSourceDto::GithubRepository {
            owner,
            repository,
            reference,
            subdirectory,
        } => {
            let repository =
                GitHubRepository::parse(owner.clone(), repository.clone()).map_err(|_| {
                    SkillInspectionFailure::invalid_source(
                        preparation_id.clone(),
                        "The GitHub repository coordinates are invalid.",
                    )
                })?;
            let reference = match reference {
                None | Some(SkillGithubReferenceDto::DefaultBranch {}) => {
                    GitHubReference::DefaultBranch
                }
                Some(SkillGithubReferenceDto::Named { value }) => {
                    GitHubReference::named(value.clone()).map_err(|_| {
                        SkillInspectionFailure::invalid_source(
                            preparation_id.clone(),
                            "The GitHub branch or tag reference is invalid.",
                        )
                    })?
                }
                Some(SkillGithubReferenceDto::Commit { sha }) => {
                    GitHubReference::commit(sha.clone()).map_err(|_| {
                        SkillInspectionFailure::invalid_source(
                            preparation_id.clone(),
                            "The GitHub commit must be a complete lowercase SHA-1.",
                        )
                    })?
                }
            };
            let subdirectory = GitHubSubdirectory::parse(subdirectory.clone().unwrap_or_default())
                .map_err(|_| {
                    SkillInspectionFailure::invalid_source(
                        preparation_id.clone(),
                        "The GitHub Skill subdirectory is invalid.",
                    )
                })?;
            let location = GitHubSkillLocation::new(repository, reference, subdirectory);
            GitHubWorkflowAcquisitionAdapter::source_for(&location).map_err(|_| {
                SkillInspectionFailure::invalid_source(
                    preparation_id,
                    "The GitHub acquisition request could not be encoded.",
                )
            })
        }
        SkillAcquisitionSourceDto::InstalledSource {} => {
            Err(SkillInspectionFailure::unsupported_source(preparation_id))
        }
    }
}

pub(crate) fn preview_response(
    preview: &SkillInstallationPreview,
) -> Result<SkillInstallationPreviewDto, SkillInspectionFailure> {
    let compatibility = compatibility(preview);
    Ok(SkillInstallationPreviewDto {
        schema_version: SKILL_INSTALLATION_WORKFLOW_SCHEMA_VERSION,
        preparation_id: preview.preparation_id().as_str().to_string(),
        preview_revision: preview.preview_revision().as_str().to_string(),
        operation: preparation_operation(preview.operation())?,
        installation_id: preview.installation_id().as_str().to_string(),
        skill_id: preview.skill_id().as_str().to_string(),
        package: SkillPackagePreviewDto {
            format_version: preview.package().format_version(),
            package_revision: preview.package().revision().as_str().to_string(),
            name: preview.package().name().to_string(),
            description: preview.package().description().to_string(),
            file_count: u64::try_from(preview.package().resources().resource_count())
                .unwrap_or(u64::MAX)
                .saturating_add(1),
            total_bytes: preview.package().package_bytes(),
        },
        source: preview_source(preview)?,
        compatibility,
        changes: installation_changes(preview),
        expires_at_unix_ms: preview.expires_at_unix_ms(),
    })
}

fn compatibility(preview: &SkillInstallationPreview) -> SkillCompatibilityReportDto {
    let issues = preview
        .warnings()
        .iter()
        .map(|warning| SkillCompatibilityIssueDto {
            id: warning.code().stable_name().to_string(),
            code: warning.code().stable_name().to_string(),
            severity: SkillCompatibilityIssueSeverityDto::Warning,
            message: warning.message().to_string(),
            capability: None,
            requires_acknowledgement: warning.acknowledgement_required(),
        })
        .collect::<Vec<_>>();
    SkillCompatibilityReportDto {
        status: if issues.is_empty() {
            SkillCompatibilityStatusDto::Compatible
        } else {
            SkillCompatibilityStatusDto::CompatibleWithWarnings
        },
        issues,
    }
}

fn preview_source(
    preview: &SkillInstallationPreview,
) -> Result<SkillPreviewSourceDto, SkillInspectionFailure> {
    match preview.acquisition().provider() {
        "local-directory" => Ok(SkillPreviewSourceDto::LocalDirectory {
            display_name: preview.package().name().to_string(),
            refreshable: true,
        }),
        "github" => github_preview_source(preview),
        _ => Err(SkillInspectionFailure::unsupported_source(Some(
            preview.preparation_id().as_str().to_string(),
        ))),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GitHubOriginDocument {
    schema_version: u32,
    owner: String,
    repository: String,
    requested_reference: GitHubOriginReference,
    resolved_commit: String,
    #[serde(default)]
    subdirectory: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", deny_unknown_fields)]
enum GitHubOriginReference {
    #[serde(rename = "defaultBranch")]
    DefaultBranch,
    #[serde(rename = "named")]
    Named { value: String },
    #[serde(rename = "commit")]
    Commit { sha: String },
}

fn github_preview_source(
    preview: &SkillInstallationPreview,
) -> Result<SkillPreviewSourceDto, SkillInspectionFailure> {
    let invalid_origin = || {
        SkillInspectionFailure::new(
            SkillInspectionPhaseDto::Inspect,
            SkillInspectionErrorCodeDto::Unavailable,
            SkillInspectionRecoveryDto::RetrySamePreparation,
            "The sanitized GitHub acquisition summary is unavailable.",
            Some(preview.preparation_id().as_str().to_string()),
            None,
        )
    };
    let document: GitHubOriginDocument =
        serde_json::from_str(preview.acquisition().reference()).map_err(|_| invalid_origin())?;
    if document.schema_version != 1
        || GitHubRepository::parse(document.owner.clone(), document.repository.clone()).is_err()
        || GitHubCommit::parse(document.resolved_commit.clone()).is_err()
    {
        return Err(invalid_origin());
    }
    let reference = match document.requested_reference {
        GitHubOriginReference::DefaultBranch => SkillGithubReferenceDto::DefaultBranch {},
        GitHubOriginReference::Named { value } => {
            GitHubReference::named(value.clone()).map_err(|_| invalid_origin())?;
            SkillGithubReferenceDto::Named { value }
        }
        GitHubOriginReference::Commit { sha } => {
            GitHubReference::commit(sha.clone()).map_err(|_| invalid_origin())?;
            SkillGithubReferenceDto::Commit { sha }
        }
    };
    if GitHubSubdirectory::parse(document.subdirectory.clone().unwrap_or_default()).is_err() {
        return Err(invalid_origin());
    }
    Ok(SkillPreviewSourceDto::GithubRepository {
        owner: document.owner,
        repository: document.repository,
        reference,
        resolved_commit: document.resolved_commit,
        subdirectory: document.subdirectory,
        refreshable: true,
    })
}

pub(crate) fn commit_request(
    input: SkillsCommitInstallationRequest,
) -> Result<SkillInstallationCommitRequest, SkillInspectionFailure> {
    let preparation_id = SkillPreparationId::parse(input.preparation_id.clone()).map_err(|_| {
        SkillInspectionFailure::new(
            SkillInspectionPhaseDto::Commit,
            SkillInspectionErrorCodeDto::PreparationNotFound,
            SkillInspectionRecoveryDto::InspectAgain,
            "The preparation ID is invalid.",
            Some(input.preparation_id.clone()),
            None,
        )
    })?;
    let preview_revision = SkillPreviewRevision::parse(input.preview_revision).map_err(|_| {
        SkillInspectionFailure::new(
            SkillInspectionPhaseDto::Commit,
            SkillInspectionErrorCodeDto::PreviewMismatch,
            SkillInspectionRecoveryDto::InspectAgain,
            "The preview revision is invalid.",
            Some(preparation_id.as_str().to_string()),
            None,
        )
    })?;
    let accepted = input
        .accepted_issue_ids
        .into_iter()
        .collect::<BTreeSet<_>>();
    if accepted.len() > 1
        || accepted
            .iter()
            .any(|issue| issue != CONTAINS_SCRIPTS_ISSUE_ID)
    {
        return Err(SkillInspectionFailure::new(
            SkillInspectionPhaseDto::Commit,
            SkillInspectionErrorCodeDto::AcknowledgementRequired,
            SkillInspectionRecoveryDto::AcknowledgeWarnings,
            "The accepted compatibility issue identifiers do not match this backend.",
            Some(preparation_id.as_str().to_string()),
            None,
        ));
    }
    let mut request = SkillInstallationCommitRequest::new(preparation_id, preview_revision);
    if accepted.contains(CONTAINS_SCRIPTS_ISSUE_ID) {
        request = request.acknowledge(SkillInstallationWarningCode::ContainsScripts);
    }
    Ok(request)
}

pub(crate) fn commit_response(
    result: &SkillInstallationCommitResult,
) -> Result<SkillInstallationCommitResponse, SkillInspectionFailure> {
    let mutation = result.mutation();
    let preview = result.preview();
    let revision = mutation.revision().ok_or_else(|| {
        SkillInspectionFailure::new(
            SkillInspectionPhaseDto::Commit,
            SkillInspectionErrorCodeDto::Unavailable,
            SkillInspectionRecoveryDto::RetrySamePreparation,
            "The committed Skill mutation did not return a package revision.",
            Some(preview.preparation_id().as_str().to_string()),
            None,
        )
    })?;
    Ok(SkillInstallationCommitResponse {
        schema_version: SKILL_INSTALLATION_WORKFLOW_SCHEMA_VERSION,
        preparation_id: preview.preparation_id().as_str().to_string(),
        operation: preparation_operation(mutation.operation())?,
        outcome: commit_outcome(mutation)?,
        installation_id: mutation.installation_id().as_str().to_string(),
        skill_id: mutation.skill_id().as_str().to_string(),
        package_revision: revision.as_str().to_string(),
        // Receipt schema v1 uses its package revision as the lifecycle CAS.
        installation_revision: revision.as_str().to_string(),
        changes: installation_changes(preview),
    })
}

fn commit_outcome(
    mutation: &SkillInstallationMutation,
) -> Result<SkillInstallationCommitOutcomeDto, SkillInspectionFailure> {
    match mutation.outcome() {
        SkillInstallationOutcome::Installed => Ok(SkillInstallationCommitOutcomeDto::Installed),
        SkillInstallationOutcome::AlreadyInstalled => {
            Ok(SkillInstallationCommitOutcomeDto::AlreadyInstalled)
        }
        SkillInstallationOutcome::Updated => Ok(SkillInstallationCommitOutcomeDto::Updated),
        SkillInstallationOutcome::AlreadyCurrent => {
            Ok(SkillInstallationCommitOutcomeDto::AlreadyCurrent)
        }
        _ => Err(SkillInspectionFailure::new(
            SkillInspectionPhaseDto::Commit,
            SkillInspectionErrorCodeDto::Unavailable,
            SkillInspectionRecoveryDto::RetrySamePreparation,
            "The workflow returned an incompatible mutation outcome.",
            None,
            None,
        )),
    }
}

fn preparation_operation(
    operation: SkillInstallationOperation,
) -> Result<SkillPreparationOperationDto, SkillInspectionFailure> {
    match operation {
        SkillInstallationOperation::Install => Ok(SkillPreparationOperationDto::Install),
        SkillInstallationOperation::Update => Ok(SkillPreparationOperationDto::Update),
        _ => Err(SkillInspectionFailure::new(
            SkillInspectionPhaseDto::Inspect,
            SkillInspectionErrorCodeDto::Unavailable,
            SkillInspectionRecoveryDto::InspectAgain,
            "The workflow returned an unsupported operation.",
            None,
            None,
        )),
    }
}

fn installation_changes(preview: &SkillInstallationPreview) -> SkillInstallationChangesDto {
    match preview.operation() {
        SkillInstallationOperation::Install => SkillInstallationChangesDto {
            content: SkillInstallationChangeDto::New,
            source: SkillInstallationChangeDto::New,
        },
        SkillInstallationOperation::Update => SkillInstallationChangesDto {
            content: if preview.expected_revision() == Some(preview.package().revision()) {
                SkillInstallationChangeDto::Unchanged
            } else {
                SkillInstallationChangeDto::Changed
            },
            // Receipt v1 cannot compare typed acquisition metadata. An update
            // is conservatively presented as a source refresh.
            source: SkillInstallationChangeDto::Changed,
        },
        _ => SkillInstallationChangesDto {
            content: SkillInstallationChangeDto::Changed,
            source: SkillInstallationChangeDto::Changed,
        },
    }
}

pub(crate) fn cancellation_response(
    preparation_id: &SkillPreparationId,
    outcome: SkillPreparationCancellation,
) -> SkillPreparationCancellationResponse {
    SkillPreparationCancellationResponse {
        schema_version: SKILL_INSTALLATION_WORKFLOW_SCHEMA_VERSION,
        preparation_id: preparation_id.as_str().to_string(),
        outcome: match outcome {
            SkillPreparationCancellation::Cancelled => {
                SkillPreparationCancellationOutcomeDto::Cancelled
            }
            SkillPreparationCancellation::AlreadyCancelled => {
                SkillPreparationCancellationOutcomeDto::AlreadyAbsent
            }
            _ => SkillPreparationCancellationOutcomeDto::AlreadyAbsent,
        },
    }
}

pub(crate) fn cancellation_preparation_id(
    input: SkillsCancelPreparationRequest,
) -> Result<SkillPreparationId, SkillInspectionFailure> {
    SkillPreparationId::parse(input.preparation_id.clone()).map_err(|_| {
        SkillInspectionFailure::new(
            SkillInspectionPhaseDto::Cancel,
            SkillInspectionErrorCodeDto::PreparationNotFound,
            SkillInspectionRecoveryDto::InspectAgain,
            "The preparation ID is invalid.",
            Some(input.preparation_id),
            None,
        )
    })
}

pub(crate) fn absent_cancellation_response(
    preparation_id: &SkillPreparationId,
) -> SkillPreparationCancellationResponse {
    SkillPreparationCancellationResponse {
        schema_version: SKILL_INSTALLATION_WORKFLOW_SCHEMA_VERSION,
        preparation_id: preparation_id.as_str().to_string(),
        outcome: SkillPreparationCancellationOutcomeDto::AlreadyAbsent,
    }
}

pub(crate) fn is_missing_preparation(error: &SkillInstallationWorkflowError) -> bool {
    matches!(
        error,
        SkillInstallationWorkflowError::PreparationNotFoundOrExpired { .. }
    )
}

pub(crate) fn workflow_failure(
    phase: SkillInspectionPhaseDto,
    error: &SkillInstallationWorkflowError,
) -> SkillInspectionFailure {
    let preparation_id = error.preparation_id().map(|id| id.as_str().to_string());
    let (code, recovery, message, diagnostic_code) = match error {
        SkillInstallationWorkflowError::UnknownAcquisitionProvider { .. } => (
            SkillInspectionErrorCodeDto::UnsupportedSource,
            SkillInspectionRecoveryDto::ChooseDifferentSource,
            "This Skill acquisition source is not supported.",
            None,
        ),
        SkillInstallationWorkflowError::InvalidUpdateTarget { .. } => (
            SkillInspectionErrorCodeDto::InvalidSource,
            SkillInspectionRecoveryDto::FixSource,
            "The selected Skill cannot be updated through the managed installer.",
            None,
        ),
        SkillInstallationWorkflowError::PreparationConflict { .. } => (
            SkillInspectionErrorCodeDto::IdempotencyConflict,
            SkillInspectionRecoveryDto::InspectAgain,
            "The preparation ID is already bound to a different request.",
            None,
        ),
        SkillInstallationWorkflowError::PreparationNotFoundOrExpired { .. } => (
            SkillInspectionErrorCodeDto::PreparationNotFound,
            SkillInspectionRecoveryDto::InspectAgain,
            "The Skill preparation was not found or has expired.",
            None,
        ),
        SkillInstallationWorkflowError::PreparationCancelled { .. } => (
            SkillInspectionErrorCodeDto::Cancelled,
            SkillInspectionRecoveryDto::InspectAgain,
            "The Skill preparation was cancelled.",
            None,
        ),
        SkillInstallationWorkflowError::PreparationAlreadyCommitted { .. } => (
            SkillInspectionErrorCodeDto::Cancelled,
            SkillInspectionRecoveryDto::InspectAgain,
            "A committed Skill preparation cannot be cancelled.",
            None,
        ),
        SkillInstallationWorkflowError::PreparationBusy { .. } => (
            SkillInspectionErrorCodeDto::Unavailable,
            SkillInspectionRecoveryDto::RetrySamePreparation,
            "The Skill preparation is currently busy.",
            None,
        ),
        SkillInstallationWorkflowError::PreviewMismatch { .. } => (
            SkillInspectionErrorCodeDto::PreviewMismatch,
            SkillInspectionRecoveryDto::InspectAgain,
            "The submitted preview revision does not match the inspected Skill.",
            None,
        ),
        SkillInstallationWorkflowError::PreparationCapacityExceeded { .. }
        | SkillInstallationWorkflowError::PreparationMemoryCapacityExceeded { .. } => (
            SkillInspectionErrorCodeDto::Unavailable,
            SkillInspectionRecoveryDto::RetryLater,
            "The Skill preparation service is at capacity.",
            None,
        ),
        SkillInstallationWorkflowError::WarningAcknowledgementRequired { missing, .. } => (
            SkillInspectionErrorCodeDto::AcknowledgementRequired,
            SkillInspectionRecoveryDto::AcknowledgeWarnings,
            "Required Skill compatibility warnings were not acknowledged.",
            missing
                .first()
                .map(|warning| warning.stable_name().to_string()),
        ),
        SkillInstallationWorkflowError::Acquisition { source, .. } => {
            let diagnostic = source
                .preparation_error()
                .map(|error| error.code().stable_name().to_string());
            match source.code() {
                SkillAcquisitionAdapterErrorCode::InvalidRequest => (
                    SkillInspectionErrorCodeDto::InvalidSource,
                    SkillInspectionRecoveryDto::FixSource,
                    "The Skill acquisition request is invalid.",
                    diagnostic,
                ),
                SkillAcquisitionAdapterErrorCode::NotFound => (
                    SkillInspectionErrorCodeDto::ReferenceNotFound,
                    SkillInspectionRecoveryDto::FixSource,
                    "The public GitHub repository, reference, or Skill directory was not found.",
                    diagnostic,
                ),
                SkillAcquisitionAdapterErrorCode::RateLimited => (
                    SkillInspectionErrorCodeDto::RateLimited,
                    SkillInspectionRecoveryDto::RetryLater,
                    "GitHub temporarily rate-limited this Skill acquisition.",
                    diagnostic,
                ),
                SkillAcquisitionAdapterErrorCode::TooLarge => (
                    SkillInspectionErrorCodeDto::RepositoryTooLarge,
                    SkillInspectionRecoveryDto::ChooseDifferentSource,
                    "The GitHub repository or Skill package exceeds the acquisition limits.",
                    diagnostic,
                ),
                SkillAcquisitionAdapterErrorCode::UnsafePackage => (
                    SkillInspectionErrorCodeDto::UnsafePackage,
                    SkillInspectionRecoveryDto::ChooseDifferentSource,
                    "The acquired archive is not a safe, portable Skill package.",
                    diagnostic,
                ),
                SkillAcquisitionAdapterErrorCode::PreparationFailed => (
                    SkillInspectionErrorCodeDto::InvalidPackage,
                    SkillInspectionRecoveryDto::FixSource,
                    "The acquired Skill package is invalid.",
                    diagnostic,
                ),
                SkillAcquisitionAdapterErrorCode::Unavailable => (
                    SkillInspectionErrorCodeDto::NetworkUnavailable,
                    SkillInspectionRecoveryDto::RetryLater,
                    "The Skill acquisition source is temporarily unavailable.",
                    diagnostic,
                ),
                _ => (
                    SkillInspectionErrorCodeDto::Unavailable,
                    SkillInspectionRecoveryDto::RetryLater,
                    "The Skill acquisition failed.",
                    diagnostic,
                ),
            }
        }
        SkillInstallationWorkflowError::Installation { source, .. } => {
            let installer = source.installer_error();
            match installer.map(|error| error.code()) {
                Some(
                    ManagedSkillInstallerErrorCode::InstallationExists
                    | ManagedSkillInstallerErrorCode::InstallationNotFound
                    | ManagedSkillInstallerErrorCode::RevisionConflict,
                ) => (
                    SkillInspectionErrorCodeDto::SourceChangedDuringRead,
                    SkillInspectionRecoveryDto::InspectAgain,
                    "The installed Skill changed after it was inspected.",
                    None,
                ),
                _ => (
                    SkillInspectionErrorCodeDto::Unavailable,
                    SkillInspectionRecoveryDto::RetrySamePreparation,
                    "The prepared Skill could not be committed. Retry the same preparation.",
                    None,
                ),
            }
        }
        SkillInstallationWorkflowError::Internal { .. } => (
            SkillInspectionErrorCodeDto::Unavailable,
            SkillInspectionRecoveryDto::RetrySamePreparation,
            "The Skill installation workflow is temporarily unavailable.",
            None,
        ),
        _ => (
            SkillInspectionErrorCodeDto::Unavailable,
            SkillInspectionRecoveryDto::RetryLater,
            "The Skill installation workflow failed.",
            None,
        ),
    };
    SkillInspectionFailure::new(
        phase,
        code,
        recovery,
        message,
        preparation_id,
        diagnostic_code,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use mycopilot_core::skills::{
        PreparedSkillPackage, SkillAcquisitionAdapter, SkillAcquisitionAdapterError,
        SkillAcquisitionProvider, SkillInstallationService, SkillInstallationWorkflow,
        SkillPackageOrigin,
    };
    use std::sync::Arc;

    struct FakeGitHubAdapter;

    impl SkillAcquisitionAdapter for FakeGitHubAdapter {
        fn provider(&self) -> SkillAcquisitionProvider {
            SkillAcquisitionProvider::parse("github").unwrap()
        }

        fn acquire(
            &self,
            _source: &SkillAcquisitionSource,
        ) -> Result<PreparedSkillPackage, SkillAcquisitionAdapterError> {
            let origin = SkillPackageOrigin::new(
                "github",
                serde_json::json!({
                    "schemaVersion": 1,
                    "owner": "example",
                    "repository": "shared-skills",
                    "requestedReference": { "kind": "named", "value": "main" },
                    "resolvedCommit": "0123456789abcdef0123456789abcdef01234567",
                    "subdirectory": "skills/auditor"
                })
                .to_string(),
            )
            .unwrap();
            PreparedSkillPackage::from_bytes(
                b"---\nname: github-auditor\ndescription: Audit from GitHub.\n---\n# Instructions\nVERIFY\n"
                    .to_vec(),
                origin,
            )
            .map_err(Into::into)
        }
    }

    #[test]
    fn github_protocol_request_maps_back_to_a_sanitized_resolved_preview() {
        let fixture = tempfile::tempdir().unwrap();
        let mut workflow = SkillInstallationWorkflow::new(
            SkillInstallationService::new(fixture.path().join("skills")).unwrap(),
        );
        workflow
            .register_adapter(Arc::new(FakeGitHubAdapter))
            .unwrap();
        let request = preparation_request(SkillsInspectInstallationRequest {
            preparation_id: "11111111-1111-4111-8111-111111111111".to_string(),
            intent: SkillInstallationIntentDto::Install {},
            source: SkillAcquisitionSourceDto::GithubRepository {
                owner: "example".to_string(),
                repository: "shared-skills".to_string(),
                reference: Some(SkillGithubReferenceDto::Named {
                    value: "main".to_string(),
                }),
                subdirectory: Some("skills/auditor".to_string()),
            },
        })
        .unwrap();

        let preview = preview_response(&workflow.inspect(&request).unwrap()).unwrap();

        assert_eq!(preview.package.name, "github-auditor");
        assert!(matches!(
            preview.source,
            SkillPreviewSourceDto::GithubRepository {
                owner,
                repository,
                reference: SkillGithubReferenceDto::Named { value },
                resolved_commit,
                subdirectory: Some(subdirectory),
                refreshable: true,
            } if owner == "example"
                && repository == "shared-skills"
                && value == "main"
                && resolved_commit == "0123456789abcdef0123456789abcdef01234567"
                && subdirectory == "skills/auditor"
        ));
    }

    #[test]
    fn installed_source_is_an_explicit_future_adapter_not_a_fallback() {
        let error = preparation_request(SkillsInspectInstallationRequest {
            preparation_id: "11111111-1111-4111-8111-111111111111".to_string(),
            intent: SkillInstallationIntentDto::Install {},
            source: SkillAcquisitionSourceDto::InstalledSource {},
        })
        .unwrap_err();

        assert_eq!(
            error.data.code,
            SkillInspectionErrorCodeDto::UnsupportedSource
        );
        assert_eq!(
            error.data.recovery,
            SkillInspectionRecoveryDto::ChooseDifferentSource
        );
    }
}
