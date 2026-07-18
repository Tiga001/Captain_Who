//! Protocol boundary for read-only Skill installation source resolution.

use mycopilot_core::skills::{
    ResolvedSkillSource, SkillInstallationSourceLocator, SkillSourceResolution,
    SkillSourceResolutionError, SkillSourceResolutionErrorCode, SkillSourceResolutionOutcome,
    SkillSourceResolutionPhase, SkillSourceResolutionRecovery,
};
use mycopilot_protocol_rs::{
    SkillInstallationSourceLocatorDto, SkillPackagePreviewDto, SkillResolvedAcquisitionSourceDto,
    SkillResolvedGithubCommitShaDto, SkillResolvedGithubReferenceDto,
    SkillSourceResolutionCandidateDto, SkillSourceResolutionErrorCodeDto,
    SkillSourceResolutionErrorData, SkillSourceResolutionErrorTypeDto,
    SkillSourceResolutionOutcomeDto, SkillSourceResolutionPhaseDto,
    SkillSourceResolutionProviderDto, SkillSourceResolutionRecoveryDto,
    SkillsResolveInstallationSourceRequest, SkillsResolveInstallationSourceResponse,
    SKILL_SOURCE_RESOLUTION_SCHEMA_VERSION,
};

#[derive(Debug)]
pub(crate) struct SkillSourceResolutionFailure {
    data: Box<SkillSourceResolutionErrorData>,
}

impl SkillSourceResolutionFailure {
    fn new(
        phase: SkillSourceResolutionPhaseDto,
        code: SkillSourceResolutionErrorCodeDto,
        recovery: SkillSourceResolutionRecoveryDto,
        message: impl Into<String>,
        provider: Option<SkillSourceResolutionProviderDto>,
    ) -> Self {
        Self {
            data: Box::new(SkillSourceResolutionErrorData {
                error_type: SkillSourceResolutionErrorTypeDto::SkillSourceResolution,
                phase,
                code,
                recovery,
                message: message.into(),
                provider,
                retry_after_ms: None,
            }),
        }
    }

    pub(crate) fn into_data(self) -> Box<SkillSourceResolutionErrorData> {
        self.data
    }
}

impl std::fmt::Display for SkillSourceResolutionFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.data.message)
    }
}

impl std::error::Error for SkillSourceResolutionFailure {}

pub(crate) fn source_locator(
    input: SkillsResolveInstallationSourceRequest,
) -> Result<SkillInstallationSourceLocator, SkillSourceResolutionFailure> {
    match input.locator {
        SkillInstallationSourceLocatorDto::Url { url } => {
            SkillInstallationSourceLocator::url(url).map_err(resolution_failure)
        }
    }
}

pub(crate) fn resolution_response(
    resolution: &SkillSourceResolution,
) -> Result<SkillsResolveInstallationSourceResponse, SkillSourceResolutionFailure> {
    let provider = match resolution.provider().as_str() {
        "github" => SkillSourceResolutionProviderDto::Github,
        _ => return Err(internal_mapping_failure()),
    };
    let resolved_commit = SkillResolvedGithubCommitShaDto::parse(resolution.resolved_revision())
        .map_err(|_| internal_mapping_failure())?;
    let outcome = match resolution.outcome() {
        SkillSourceResolutionOutcome::Resolved => SkillSourceResolutionOutcomeDto::Resolved,
        SkillSourceResolutionOutcome::SelectionRequired => {
            SkillSourceResolutionOutcomeDto::SelectionRequired
        }
    };
    let mut candidates = Vec::with_capacity(resolution.candidates().len());
    for candidate in resolution.candidates() {
        let source = match candidate.source() {
            ResolvedSkillSource::GitHub {
                owner,
                repository,
                resolved_commit,
                subdirectory,
            } => {
                let sha = SkillResolvedGithubCommitShaDto::parse(resolved_commit.clone())
                    .map_err(|_| internal_mapping_failure())?;
                SkillResolvedAcquisitionSourceDto::GithubRepository {
                    owner: owner.clone(),
                    repository: repository.clone(),
                    reference: SkillResolvedGithubReferenceDto::Commit { sha },
                    subdirectory: subdirectory.clone(),
                }
            }
            _ => return Err(internal_mapping_failure()),
        };
        let package = candidate.package();
        candidates.push(SkillSourceResolutionCandidateDto {
            candidate_id: candidate.candidate_id().to_string(),
            source,
            package: SkillPackagePreviewDto {
                format_version: package.format_version(),
                package_revision: package.revision().as_str().to_string(),
                name: package.name().to_string(),
                description: package.description().to_string(),
                file_count: package.file_count(),
                total_bytes: package.total_bytes(),
            },
        });
    }
    let response = SkillsResolveInstallationSourceResponse {
        schema_version: SKILL_SOURCE_RESOLUTION_SCHEMA_VERSION,
        canonical_url: resolution.canonical_url().to_string(),
        provider,
        resolved_commit,
        outcome,
        candidates,
    };
    response
        .validate()
        .map_err(|_| internal_mapping_failure())?;
    Ok(response)
}

pub(crate) fn resolution_failure(
    error: SkillSourceResolutionError,
) -> SkillSourceResolutionFailure {
    let phase = match error.phase() {
        SkillSourceResolutionPhase::Parse => SkillSourceResolutionPhaseDto::Parse,
        SkillSourceResolutionPhase::Resolve => SkillSourceResolutionPhaseDto::Resolve,
        SkillSourceResolutionPhase::Discover => SkillSourceResolutionPhaseDto::Discover,
        _ => SkillSourceResolutionPhaseDto::Resolve,
    };
    let code = match error.code() {
        SkillSourceResolutionErrorCode::InvalidLocator => {
            SkillSourceResolutionErrorCodeDto::InvalidLocator
        }
        SkillSourceResolutionErrorCode::UnsupportedLocator => {
            SkillSourceResolutionErrorCodeDto::UnsupportedLocator
        }
        SkillSourceResolutionErrorCode::UnsupportedHost => {
            SkillSourceResolutionErrorCodeDto::UnsupportedHost
        }
        SkillSourceResolutionErrorCode::UnsupportedUrlShape => {
            SkillSourceResolutionErrorCodeDto::UnsupportedUrlShape
        }
        SkillSourceResolutionErrorCode::RepositoryNotFound => {
            SkillSourceResolutionErrorCodeDto::RepositoryNotFound
        }
        SkillSourceResolutionErrorCode::ReferenceNotFound => {
            SkillSourceResolutionErrorCodeDto::ReferenceNotFound
        }
        SkillSourceResolutionErrorCode::PathNotFound => {
            SkillSourceResolutionErrorCodeDto::PathNotFound
        }
        SkillSourceResolutionErrorCode::AmbiguousReference => {
            SkillSourceResolutionErrorCodeDto::AmbiguousReference
        }
        SkillSourceResolutionErrorCode::NoSkillsFound => {
            SkillSourceResolutionErrorCodeDto::NoSkillsFound
        }
        SkillSourceResolutionErrorCode::TooManySkills => {
            SkillSourceResolutionErrorCodeDto::TooManySkills
        }
        SkillSourceResolutionErrorCode::NetworkUnavailable => {
            SkillSourceResolutionErrorCodeDto::NetworkUnavailable
        }
        SkillSourceResolutionErrorCode::RateLimited => {
            SkillSourceResolutionErrorCodeDto::RateLimited
        }
        SkillSourceResolutionErrorCode::RepositoryTooLarge => {
            SkillSourceResolutionErrorCodeDto::RepositoryTooLarge
        }
        SkillSourceResolutionErrorCode::UnsafePackage => {
            SkillSourceResolutionErrorCodeDto::UnsafePackage
        }
        SkillSourceResolutionErrorCode::InvalidPackage => {
            SkillSourceResolutionErrorCodeDto::InvalidPackage
        }
        SkillSourceResolutionErrorCode::Unavailable => {
            SkillSourceResolutionErrorCodeDto::Unavailable
        }
        _ => SkillSourceResolutionErrorCodeDto::Unavailable,
    };
    let recovery = match error.recovery() {
        SkillSourceResolutionRecovery::FixLocator => SkillSourceResolutionRecoveryDto::FixLocator,
        SkillSourceResolutionRecovery::RetryLater => SkillSourceResolutionRecoveryDto::RetryLater,
        SkillSourceResolutionRecovery::NarrowLocator => {
            SkillSourceResolutionRecoveryDto::NarrowLocator
        }
        SkillSourceResolutionRecovery::ChooseDifferentSource => {
            SkillSourceResolutionRecoveryDto::ChooseDifferentSource
        }
        _ => SkillSourceResolutionRecoveryDto::RetryLater,
    };
    // The domain service binds the selected resolver id after host dispatch. Never infer a
    // provider from an error phase: doing so would mislabel future GitLab/registry adapters.
    let provider = match error.provider().map(|provider| provider.as_str()) {
        Some("github") => Some(SkillSourceResolutionProviderDto::Github),
        _ => None,
    };
    SkillSourceResolutionFailure::new(phase, code, recovery, error.message(), provider)
}

pub(crate) fn resolution_dispatch_failure(
    message: impl Into<String>,
) -> SkillSourceResolutionFailure {
    SkillSourceResolutionFailure::new(
        SkillSourceResolutionPhaseDto::Resolve,
        SkillSourceResolutionErrorCodeDto::Unavailable,
        SkillSourceResolutionRecoveryDto::RetryLater,
        message,
        None,
    )
}

fn internal_mapping_failure() -> SkillSourceResolutionFailure {
    SkillSourceResolutionFailure::new(
        SkillSourceResolutionPhaseDto::Resolve,
        SkillSourceResolutionErrorCodeDto::Unavailable,
        SkillSourceResolutionRecoveryDto::RetryLater,
        "The resolved Skill source could not be encoded safely.",
        None,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use mycopilot_core::skills::{
        ResolvedSkillPackagePreview, SkillRevision, SkillSourceResolutionCandidate,
        SkillSourceResolverId,
    };

    #[test]
    fn response_mapping_preserves_only_immutable_github_coordinates() {
        let revision = SkillRevision::parse(format!("sha256:{}", "a".repeat(64))).unwrap();
        let resolution = SkillSourceResolution::new(
            "https://github.com/example/skills/tree/main/skills/alpha",
            SkillSourceResolverId::parse("github").unwrap(),
            "0123456789abcdef0123456789abcdef01234567",
            vec![SkillSourceResolutionCandidate::new(
                "candidate",
                ResolvedSkillSource::GitHub {
                    owner: "example".to_string(),
                    repository: "skills".to_string(),
                    resolved_commit: "0123456789abcdef0123456789abcdef01234567".to_string(),
                    subdirectory: Some("skills/alpha".to_string()),
                },
                ResolvedSkillPackagePreview::new(1, revision, "alpha", "Alpha fixture.", 1, 64),
            )],
        )
        .unwrap();

        let response = resolution_response(&resolution).unwrap();

        assert_eq!(response.outcome, SkillSourceResolutionOutcomeDto::Resolved);
        let SkillResolvedAcquisitionSourceDto::GithubRepository { reference, .. } =
            &response.candidates[0].source;
        assert!(matches!(
            reference,
            SkillResolvedGithubReferenceDto::Commit { .. }
        ));
    }
}
