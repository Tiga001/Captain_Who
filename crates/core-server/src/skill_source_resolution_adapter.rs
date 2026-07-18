//! Protocol boundary for read-only Skill installation source resolution.

use mycopilot_core::skills::{
    GitHubReference, ResolvedSkillSource, SkillInstallationSourceLocator,
    SkillRegisteredSourceResolution, SkillSourceResolutionCancellation, SkillSourceResolutionError,
    SkillSourceResolutionErrorCode, SkillSourceResolutionId, SkillSourceResolutionOutcome,
    SkillSourceResolutionPhase, SkillSourceResolutionRecovery,
};
use mycopilot_protocol_rs::{
    SkillGithubReferenceDto, SkillInstallationSourceLocatorDto, SkillPackagePreviewDto,
    SkillResolutionIdDto, SkillResolvedAcquisitionSourceDto, SkillResolvedCandidateAcquisitionDto,
    SkillResolvedGithubCommitShaDto, SkillSourceResolutionCancellationOutcomeDto,
    SkillSourceResolutionCandidateDto, SkillSourceResolutionErrorCodeDto,
    SkillSourceResolutionErrorData, SkillSourceResolutionErrorTypeDto,
    SkillSourceResolutionOutcomeDto, SkillSourceResolutionPhaseDto,
    SkillSourceResolutionProviderDto, SkillSourceResolutionRecoveryDto,
    SkillsCancelSourceResolutionRequest, SkillsCancelSourceResolutionResponse,
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

pub(crate) fn resolution_request(
    input: SkillsResolveInstallationSourceRequest,
) -> Result<(SkillSourceResolutionId, SkillInstallationSourceLocator), SkillSourceResolutionFailure>
{
    let resolution_id = SkillSourceResolutionId::parse(input.resolution_id.into_inner())
        .map_err(|_| invalid_resolution_id_failure())?;
    let locator = match input.locator {
        SkillInstallationSourceLocatorDto::Url { url } => {
            SkillInstallationSourceLocator::url(url).map_err(resolution_failure)
        }
    }?;
    Ok((resolution_id, locator))
}

pub(crate) fn cancellation_resolution_id(
    input: SkillsCancelSourceResolutionRequest,
) -> Result<SkillSourceResolutionId, SkillSourceResolutionFailure> {
    SkillSourceResolutionId::parse(input.resolution_id.into_inner())
        .map_err(|_| invalid_resolution_id_failure())
}

pub(crate) fn source_resolution_cancellation_response(
    resolution_id: &SkillSourceResolutionId,
    outcome: SkillSourceResolutionCancellation,
) -> Result<SkillsCancelSourceResolutionResponse, SkillSourceResolutionFailure> {
    let resolution_id = SkillResolutionIdDto::parse(resolution_id.as_str())
        .map_err(|_| internal_mapping_failure())?;
    let outcome = match outcome {
        SkillSourceResolutionCancellation::Cancelled => {
            SkillSourceResolutionCancellationOutcomeDto::Cancelled
        }
        SkillSourceResolutionCancellation::AlreadyCancelled => {
            SkillSourceResolutionCancellationOutcomeDto::AlreadyCancelled
        }
        SkillSourceResolutionCancellation::AlreadyConsumed => {
            SkillSourceResolutionCancellationOutcomeDto::AlreadyConsumed
        }
        SkillSourceResolutionCancellation::AlreadyAbsent => {
            SkillSourceResolutionCancellationOutcomeDto::AlreadyAbsent
        }
        _ => return Err(internal_mapping_failure()),
    };
    Ok(SkillsCancelSourceResolutionResponse {
        schema_version: SKILL_SOURCE_RESOLUTION_SCHEMA_VERSION,
        resolution_id,
        outcome,
    })
}

pub(crate) fn resolution_response(
    registered: &SkillRegisteredSourceResolution,
) -> Result<SkillsResolveInstallationSourceResponse, SkillSourceResolutionFailure> {
    let resolution = registered.resolution();
    let resolution_id = SkillResolutionIdDto::parse(registered.resolution_id().as_str())
        .map_err(|_| internal_mapping_failure())?;
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
                tracking_reference,
                resolved_commit,
                subdirectory,
            } => {
                let sha = SkillResolvedGithubCommitShaDto::parse(resolved_commit.clone())
                    .map_err(|_| internal_mapping_failure())?;
                let reference = match tracking_reference {
                    GitHubReference::DefaultBranch => SkillGithubReferenceDto::DefaultBranch {},
                    GitHubReference::Named(reference) => SkillGithubReferenceDto::Named {
                        value: reference.as_str().to_string(),
                    },
                    GitHubReference::Commit(commit) => SkillGithubReferenceDto::Commit {
                        sha: commit.as_str().to_string(),
                    },
                    _ => return Err(internal_mapping_failure()),
                };
                SkillResolvedAcquisitionSourceDto::GithubRepository {
                    owner: owner.clone(),
                    repository: repository.clone(),
                    reference,
                    resolved_commit: sha,
                    subdirectory: subdirectory.clone(),
                }
            }
            _ => return Err(internal_mapping_failure()),
        };
        let package = candidate.package();
        let candidate_id = candidate.candidate_id().to_string();
        candidates.push(SkillSourceResolutionCandidateDto {
            candidate_id: candidate_id.clone(),
            acquisition: SkillResolvedCandidateAcquisitionDto::ResolvedCandidate {
                resolution_id: resolution_id.clone(),
                candidate_id,
            },
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
        resolution_id,
        canonical_url: resolution.canonical_url().to_string(),
        provider,
        resolved_commit,
        expires_at_unix_ms: registered.expires_at_unix_ms(),
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
    let (code, message) = match error.code() {
        SkillSourceResolutionErrorCode::InvalidLocator => (
            SkillSourceResolutionErrorCodeDto::InvalidLocator,
            "The Skill source locator is invalid.",
        ),
        SkillSourceResolutionErrorCode::UnsupportedLocator => (
            SkillSourceResolutionErrorCodeDto::UnsupportedLocator,
            "This Skill source locator type is not supported.",
        ),
        SkillSourceResolutionErrorCode::UnsupportedHost => (
            SkillSourceResolutionErrorCodeDto::UnsupportedHost,
            "This URL host is not supported.",
        ),
        SkillSourceResolutionErrorCode::UnsupportedUrlShape => (
            SkillSourceResolutionErrorCodeDto::UnsupportedUrlShape,
            "This URL does not identify a supported Skill source.",
        ),
        SkillSourceResolutionErrorCode::RepositoryNotFound => (
            SkillSourceResolutionErrorCodeDto::RepositoryNotFound,
            "The requested repository was not found or is not accessible.",
        ),
        SkillSourceResolutionErrorCode::ReferenceNotFound => (
            SkillSourceResolutionErrorCodeDto::ReferenceNotFound,
            "The requested repository reference was not found.",
        ),
        SkillSourceResolutionErrorCode::PathNotFound => (
            SkillSourceResolutionErrorCodeDto::PathNotFound,
            "The requested path was not found at the resolved source.",
        ),
        SkillSourceResolutionErrorCode::AmbiguousReference => (
            SkillSourceResolutionErrorCodeDto::AmbiguousReference,
            "The source reference is ambiguous; use a more specific URL.",
        ),
        SkillSourceResolutionErrorCode::NoSkillsFound => (
            SkillSourceResolutionErrorCodeDto::NoSkillsFound,
            "No valid Skill package was found at the resolved location.",
        ),
        SkillSourceResolutionErrorCode::TooManySkills => (
            SkillSourceResolutionErrorCodeDto::TooManySkills,
            "The source contains too many Skill packages to inspect safely.",
        ),
        SkillSourceResolutionErrorCode::NetworkUnavailable => (
            SkillSourceResolutionErrorCodeDto::NetworkUnavailable,
            "The Skill source provider is temporarily unreachable.",
        ),
        SkillSourceResolutionErrorCode::RateLimited => (
            SkillSourceResolutionErrorCodeDto::RateLimited,
            "The Skill source provider temporarily refused the resolution request.",
        ),
        SkillSourceResolutionErrorCode::RepositoryTooLarge => (
            SkillSourceResolutionErrorCodeDto::RepositoryTooLarge,
            "The repository is too large to inspect safely.",
        ),
        SkillSourceResolutionErrorCode::UnsafePackage => (
            SkillSourceResolutionErrorCodeDto::UnsafePackage,
            "The resolved Skill package was rejected by the safety policy.",
        ),
        SkillSourceResolutionErrorCode::InvalidPackage => (
            SkillSourceResolutionErrorCodeDto::InvalidPackage,
            "The resolved Skill package is invalid.",
        ),
        SkillSourceResolutionErrorCode::IdempotencyConflict => (
            SkillSourceResolutionErrorCodeDto::ResolutionIdConflict,
            "The resolution ID is already bound to a different source locator.",
        ),
        SkillSourceResolutionErrorCode::ResolutionNotFoundOrExpired => (
            SkillSourceResolutionErrorCodeDto::ResolutionNotFoundOrExpired,
            "The Skill source resolution was not found or has expired.",
        ),
        SkillSourceResolutionErrorCode::ResolutionConsumed => (
            SkillSourceResolutionErrorCodeDto::ResolutionConsumed,
            "The Skill source resolution has already been consumed.",
        ),
        SkillSourceResolutionErrorCode::CandidateNotFound => (
            SkillSourceResolutionErrorCodeDto::CandidateNotFound,
            "The selected Skill source candidate does not exist in this resolution.",
        ),
        SkillSourceResolutionErrorCode::CapacityExceeded => (
            SkillSourceResolutionErrorCodeDto::CapacityExceeded,
            "The Skill source resolution registry is temporarily at capacity.",
        ),
        SkillSourceResolutionErrorCode::Cancelled => (
            SkillSourceResolutionErrorCodeDto::Cancelled,
            "The Skill source resolution was cancelled.",
        ),
        SkillSourceResolutionErrorCode::Unavailable => (
            SkillSourceResolutionErrorCodeDto::Unavailable,
            "Skill source resolution is temporarily unavailable.",
        ),
        _ => (
            SkillSourceResolutionErrorCodeDto::Unavailable,
            "Skill source resolution is temporarily unavailable.",
        ),
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
        SkillSourceResolutionRecovery::StartNewResolution => {
            SkillSourceResolutionRecoveryDto::StartNewResolution
        }
        SkillSourceResolutionRecovery::RetrySameResolution => {
            SkillSourceResolutionRecoveryDto::RetrySameResolution
        }
        _ => SkillSourceResolutionRecoveryDto::RetryLater,
    };
    // The domain service binds the selected resolver id after host dispatch. Never infer a
    // provider from an error phase: doing so would mislabel future GitLab/registry adapters.
    let provider = match error.provider().map(|provider| provider.as_str()) {
        Some("github") => Some(SkillSourceResolutionProviderDto::Github),
        _ => None,
    };
    // Resolver errors are untrusted provider output. Preserve only typed fields at the Host
    // boundary and derive the user-facing message from our stable error taxonomy. A future
    // resolver may otherwise leak credentials, private URLs, paths, or upstream response bodies.
    SkillSourceResolutionFailure::new(phase, code, recovery, message, provider)
}

pub(crate) fn resolution_dispatch_failure(
    _message: impl Into<String>,
) -> SkillSourceResolutionFailure {
    SkillSourceResolutionFailure::new(
        SkillSourceResolutionPhaseDto::Resolve,
        SkillSourceResolutionErrorCodeDto::Unavailable,
        SkillSourceResolutionRecoveryDto::RetryLater,
        "Skill source resolution is temporarily unavailable.",
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

fn invalid_resolution_id_failure() -> SkillSourceResolutionFailure {
    SkillSourceResolutionFailure::new(
        SkillSourceResolutionPhaseDto::Parse,
        SkillSourceResolutionErrorCodeDto::InvalidLocator,
        SkillSourceResolutionRecoveryDto::StartNewResolution,
        "The Skill source resolution ID is invalid.",
        None,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use mycopilot_core::skills::{
        PreparedSkillAcquisition, PreparedSkillPackage, PreparedSkillSourceResolution,
        PreparedSkillSourceResolutionCandidate, SkillInstallationAuthority,
        SkillInstallationProvenance, SkillInstallationRefresh, SkillInstallationSourceResolver,
        SkillPackageOrigin, SkillSourceCandidateId, SkillSourceResolution,
        SkillSourceResolutionService, SkillSourceResolverId, GITHUB_SKILL_ORIGIN_PROVIDER,
    };
    use std::sync::Arc;

    struct StaticPreparedResolver;

    impl SkillInstallationSourceResolver for StaticPreparedResolver {
        fn id(&self) -> SkillSourceResolverId {
            SkillSourceResolverId::parse("github").unwrap()
        }

        fn supported_hosts(&self) -> Vec<String> {
            vec!["github.com".to_string()]
        }

        fn resolve(
            &self,
            _locator: &SkillInstallationSourceLocator,
        ) -> Result<SkillSourceResolution, SkillSourceResolutionError> {
            unreachable!("registered source resolution uses resolve_prepared")
        }

        fn resolve_prepared(
            &self,
            _locator: &SkillInstallationSourceLocator,
        ) -> Result<PreparedSkillSourceResolution, SkillSourceResolutionError> {
            let commit = "0123456789abcdef0123456789abcdef01234567";
            let origin_reference = serde_json::json!({
                "schemaVersion": 1,
                "owner": "example",
                "repository": "skills",
                "requestedReference": { "kind": "named", "value": "main" },
                "resolvedCommit": commit,
                "subdirectory": "skills/alpha"
            })
            .to_string();
            let origin =
                SkillPackageOrigin::new(GITHUB_SKILL_ORIGIN_PROVIDER, origin_reference).unwrap();
            let package = PreparedSkillPackage::from_bytes(
                concat!(
                    "---\n",
                    "name: alpha\n",
                    "description: Alpha fixture.\n",
                    "---\n",
                    "# Instructions\n",
                    "Audit the repository.\n"
                )
                .as_bytes()
                .to_vec(),
                origin,
            )
            .unwrap();
            let provenance = SkillInstallationProvenance::new(
                SkillInstallationAuthority::new(
                    GITHUB_SKILL_ORIGIN_PROVIDER,
                    1,
                    serde_json::json!({
                        "owner": "example",
                        "repository": "skills",
                        "resolvedCommit": commit,
                        "subdirectory": "skills/alpha"
                    })
                    .to_string(),
                )
                .unwrap(),
                Some(
                    SkillInstallationRefresh::new(
                        GITHUB_SKILL_ORIGIN_PROVIDER,
                        1,
                        serde_json::json!({
                            "owner": "example",
                            "repository": "skills",
                            "reference": { "kind": "named", "value": "main" },
                            "subdirectory": "skills/alpha"
                        })
                        .to_string(),
                    )
                    .unwrap(),
                ),
            );
            PreparedSkillSourceResolution::new(
                "https://github.com/example/skills/tree/main/skills/alpha",
                self.id(),
                commit,
                vec![PreparedSkillSourceResolutionCandidate::new(
                    SkillSourceCandidateId::parse("candidate").unwrap(),
                    ResolvedSkillSource::GitHub {
                        owner: "example".to_string(),
                        repository: "skills".to_string(),
                        tracking_reference: GitHubReference::named("main").unwrap(),
                        resolved_commit: commit.to_string(),
                        subdirectory: Some("skills/alpha".to_string()),
                    },
                    PreparedSkillAcquisition::new(package, provenance),
                )],
            )
        }
    }

    #[test]
    fn response_mapping_preserves_tracking_metadata_and_one_time_handoff_authority() {
        let resolution_id =
            SkillSourceResolutionId::parse("11111111-1111-4111-8111-111111111111").unwrap();
        let locator = SkillInstallationSourceLocator::url(
            "https://github.com/example/skills/tree/main/skills/alpha",
        )
        .unwrap();
        let mut service = SkillSourceResolutionService::new();
        service
            .register_resolver(Arc::new(StaticPreparedResolver))
            .unwrap();
        let registered = service.resolve_registered(resolution_id, &locator).unwrap();

        let response = resolution_response(&registered).unwrap();

        assert_eq!(response.outcome, SkillSourceResolutionOutcomeDto::Resolved);
        assert_eq!(
            response.resolution_id.as_str(),
            "11111111-1111-4111-8111-111111111111"
        );
        assert!(response.expires_at_unix_ms > 0);
        let candidate = &response.candidates[0];
        let SkillResolvedCandidateAcquisitionDto::ResolvedCandidate {
            resolution_id,
            candidate_id,
        } = &candidate.acquisition;
        assert_eq!(resolution_id, &response.resolution_id);
        assert_eq!(candidate_id, &candidate.candidate_id);
        let SkillResolvedAcquisitionSourceDto::GithubRepository {
            reference,
            resolved_commit,
            ..
        } = &candidate.source;
        assert!(matches!(
            reference,
            SkillGithubReferenceDto::Named { value } if value == "main"
        ));
        assert_eq!(resolved_commit.as_str(), response.resolved_commit.as_str());
    }

    #[test]
    fn cancellation_mapping_preserves_idempotent_outcomes_and_identity() {
        let resolution_id =
            SkillSourceResolutionId::parse("11111111-1111-4111-8111-111111111111").unwrap();
        for (domain, protocol) in [
            (
                SkillSourceResolutionCancellation::Cancelled,
                SkillSourceResolutionCancellationOutcomeDto::Cancelled,
            ),
            (
                SkillSourceResolutionCancellation::AlreadyCancelled,
                SkillSourceResolutionCancellationOutcomeDto::AlreadyCancelled,
            ),
            (
                SkillSourceResolutionCancellation::AlreadyConsumed,
                SkillSourceResolutionCancellationOutcomeDto::AlreadyConsumed,
            ),
            (
                SkillSourceResolutionCancellation::AlreadyAbsent,
                SkillSourceResolutionCancellationOutcomeDto::AlreadyAbsent,
            ),
        ] {
            let response = source_resolution_cancellation_response(&resolution_id, domain).unwrap();
            assert_eq!(
                response.schema_version,
                SKILL_SOURCE_RESOLUTION_SCHEMA_VERSION
            );
            assert_eq!(response.resolution_id.as_str(), resolution_id.as_str());
            assert_eq!(response.outcome, protocol);
        }
    }

    #[test]
    fn source_resolution_registry_errors_keep_actionable_protocol_recovery() {
        for (error, expected_code, expected_recovery) in [
            (
                SkillSourceResolutionError::resolve(
                    SkillSourceResolutionErrorCode::IdempotencyConflict,
                    SkillSourceResolutionRecovery::StartNewResolution,
                    "conflict",
                ),
                SkillSourceResolutionErrorCodeDto::ResolutionIdConflict,
                SkillSourceResolutionRecoveryDto::StartNewResolution,
            ),
            (
                SkillSourceResolutionError::resolve(
                    SkillSourceResolutionErrorCode::CapacityExceeded,
                    SkillSourceResolutionRecovery::RetryLater,
                    "capacity",
                ),
                SkillSourceResolutionErrorCodeDto::CapacityExceeded,
                SkillSourceResolutionRecoveryDto::RetryLater,
            ),
            (
                SkillSourceResolutionError::resolve(
                    SkillSourceResolutionErrorCode::ResolutionNotFoundOrExpired,
                    SkillSourceResolutionRecovery::StartNewResolution,
                    "not found or expired",
                ),
                SkillSourceResolutionErrorCodeDto::ResolutionNotFoundOrExpired,
                SkillSourceResolutionRecoveryDto::StartNewResolution,
            ),
            (
                SkillSourceResolutionError::resolve(
                    SkillSourceResolutionErrorCode::ResolutionConsumed,
                    SkillSourceResolutionRecovery::StartNewResolution,
                    "consumed",
                ),
                SkillSourceResolutionErrorCodeDto::ResolutionConsumed,
                SkillSourceResolutionRecoveryDto::StartNewResolution,
            ),
            (
                SkillSourceResolutionError::resolve(
                    SkillSourceResolutionErrorCode::CandidateNotFound,
                    SkillSourceResolutionRecovery::RetrySameResolution,
                    "candidate not found",
                ),
                SkillSourceResolutionErrorCodeDto::CandidateNotFound,
                SkillSourceResolutionRecoveryDto::RetrySameResolution,
            ),
            (
                SkillSourceResolutionError::resolve(
                    SkillSourceResolutionErrorCode::Cancelled,
                    SkillSourceResolutionRecovery::StartNewResolution,
                    "cancelled",
                ),
                SkillSourceResolutionErrorCodeDto::Cancelled,
                SkillSourceResolutionRecoveryDto::StartNewResolution,
            ),
        ] {
            let data = resolution_failure(error).into_data();
            assert_eq!(data.code, expected_code);
            assert_eq!(data.recovery, expected_recovery);
        }
    }

    #[test]
    fn resolver_messages_never_cross_the_host_boundary() {
        let secret = "https://provider.example/private?token=do-not-expose";
        let data = resolution_failure(SkillSourceResolutionError::resolve(
            SkillSourceResolutionErrorCode::RateLimited,
            SkillSourceResolutionRecovery::RetryLater,
            format!("upstream rejected {secret}"),
        ))
        .into_data();

        assert_eq!(data.code, SkillSourceResolutionErrorCodeDto::RateLimited);
        assert_eq!(
            data.message,
            "The Skill source provider temporarily refused the resolution request."
        );
        assert!(!data.message.contains(secret));
        assert!(!format!("{data:?}").contains(secret));

        let dispatch_data = resolution_dispatch_failure(secret).into_data();
        assert_eq!(
            dispatch_data.message,
            "Skill source resolution is temporarily unavailable."
        );
        assert!(!dispatch_data.message.contains(secret));
    }
}
