use serde::{Deserialize, Serialize};

pub const SKILL_CATALOG_SCHEMA_VERSION: u32 = 4;
pub const SKILL_MUTATION_SCHEMA_VERSION: u32 = 1;
pub const SKILL_INSTALLATION_WORKFLOW_SCHEMA_VERSION: u32 = 1;
pub const SKILL_MANAGEMENT_SCHEMA_VERSION: u32 = 1;
pub const SKILL_SOURCE_RESOLUTION_SCHEMA_VERSION: u32 = 2;
pub const SKILL_INSTALLATION_ERROR_CODE: i64 = -32010;
pub const SKILL_INSPECTION_ERROR_CODE: i64 = -32011;
pub const SKILL_MANAGEMENT_ERROR_CODE: i64 = -32012;
pub const SKILL_SOURCE_RESOLUTION_ERROR_CODE: i64 = -32013;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillsInstallLocalRequest {
    pub installation_id: String,
    pub directory: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillsUpdateLocalRequest {
    pub skill_id: String,
    pub expected_revision: String,
    pub directory: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillsUninstallRequest {
    pub skill_id: String,
    /// Exact installation lifecycle revision returned by `skills.listManagement`.
    /// The server still accepts a package revision for legacy callers.
    pub expected_revision: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillsInspectInstallationRequest {
    pub preparation_id: String,
    pub intent: SkillInstallationIntentDto,
    pub source: SkillAcquisitionSourceDto,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "operation", deny_unknown_fields)]
pub enum SkillInstallationIntentDto {
    #[serde(rename = "install")]
    Install {},
    #[serde(rename = "update", rename_all = "camelCase")]
    Update {
        skill_id: String,
        expected_installation_revision: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillAcquisitionSourceDto {
    LocalDirectory {
        directory: String,
    },
    GithubRepository {
        owner: String,
        repository: String,
        reference: Option<SkillGithubReferenceDto>,
        subdirectory: Option<String>,
    },
    ResolvedCandidate {
        resolution_id: SkillResolutionIdDto,
        candidate_id: String,
    },
    InstalledSource {},
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", deny_unknown_fields)]
enum SkillAcquisitionSourceDtoWire {
    #[serde(rename = "localDirectory", rename_all = "camelCase")]
    LocalDirectory { directory: String },
    #[serde(rename = "githubRepository", rename_all = "camelCase")]
    GithubRepository {
        owner: String,
        repository: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reference: Option<SkillGithubReferenceDto>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subdirectory: Option<String>,
    },
    #[serde(rename = "resolvedCandidate", rename_all = "camelCase")]
    ResolvedCandidate {
        resolution_id: SkillResolutionIdDto,
        candidate_id: String,
    },
    #[serde(rename = "installedSource")]
    InstalledSource {},
}

impl SkillAcquisitionSourceDto {
    fn validate(&self) -> Result<(), String> {
        match self {
            Self::LocalDirectory { directory } if directory.trim().is_empty() => {
                return Err("local directory must be non-empty".to_string());
            }
            Self::GithubRepository {
                owner,
                repository,
                reference,
                subdirectory,
            } => {
                if owner.trim().is_empty() || repository.trim().is_empty() {
                    return Err("GitHub owner and repository must be non-empty".to_string());
                }
                if subdirectory
                    .as_deref()
                    .is_some_and(|value| value.trim().is_empty())
                {
                    return Err("GitHub subdirectory must be non-empty when present".to_string());
                }
                match reference {
                    Some(SkillGithubReferenceDto::Named { value }) if value.trim().is_empty() => {
                        return Err("GitHub named reference must be non-empty".to_string());
                    }
                    Some(SkillGithubReferenceDto::Commit { sha }) => {
                        SkillResolvedGithubCommitShaDto::parse(sha.clone())
                            .map_err(|reason| reason.to_string())?;
                    }
                    _ => {}
                }
            }
            Self::ResolvedCandidate { candidate_id, .. } if candidate_id.trim().is_empty() => {
                return Err("resolved candidate id must be non-empty".to_string());
            }
            _ => {}
        }
        Ok(())
    }
}

impl From<SkillAcquisitionSourceDtoWire> for SkillAcquisitionSourceDto {
    fn from(value: SkillAcquisitionSourceDtoWire) -> Self {
        match value {
            SkillAcquisitionSourceDtoWire::LocalDirectory { directory } => {
                Self::LocalDirectory { directory }
            }
            SkillAcquisitionSourceDtoWire::GithubRepository {
                owner,
                repository,
                reference,
                subdirectory,
            } => Self::GithubRepository {
                owner,
                repository,
                reference,
                subdirectory,
            },
            SkillAcquisitionSourceDtoWire::ResolvedCandidate {
                resolution_id,
                candidate_id,
            } => Self::ResolvedCandidate {
                resolution_id,
                candidate_id,
            },
            SkillAcquisitionSourceDtoWire::InstalledSource {} => Self::InstalledSource {},
        }
    }
}

impl From<&SkillAcquisitionSourceDto> for SkillAcquisitionSourceDtoWire {
    fn from(value: &SkillAcquisitionSourceDto) -> Self {
        match value {
            SkillAcquisitionSourceDto::LocalDirectory { directory } => Self::LocalDirectory {
                directory: directory.clone(),
            },
            SkillAcquisitionSourceDto::GithubRepository {
                owner,
                repository,
                reference,
                subdirectory,
            } => Self::GithubRepository {
                owner: owner.clone(),
                repository: repository.clone(),
                reference: reference.clone(),
                subdirectory: subdirectory.clone(),
            },
            SkillAcquisitionSourceDto::ResolvedCandidate {
                resolution_id,
                candidate_id,
            } => Self::ResolvedCandidate {
                resolution_id: resolution_id.clone(),
                candidate_id: candidate_id.clone(),
            },
            SkillAcquisitionSourceDto::InstalledSource {} => Self::InstalledSource {},
        }
    }
}

impl Serialize for SkillAcquisitionSourceDto {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.validate().map_err(serde::ser::Error::custom)?;
        SkillAcquisitionSourceDtoWire::from(self).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for SkillAcquisitionSourceDto {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = Self::from(SkillAcquisitionSourceDtoWire::deserialize(deserializer)?);
        value.validate().map_err(serde::de::Error::custom)?;
        Ok(value)
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", deny_unknown_fields)]
pub enum SkillGithubReferenceDto {
    #[serde(rename = "defaultBranch")]
    DefaultBranch {},
    #[serde(rename = "named")]
    Named { value: String },
    #[serde(rename = "commit")]
    Commit { sha: String },
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SkillPreparationOperationDto {
    Install,
    Update,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillInstallationPreviewDto {
    pub schema_version: u32,
    pub preparation_id: String,
    pub preview_revision: String,
    pub operation: SkillPreparationOperationDto,
    pub installation_id: String,
    pub skill_id: String,
    pub package: SkillPackagePreviewDto,
    pub source: SkillPreviewSourceDto,
    pub compatibility: SkillCompatibilityReportDto,
    pub changes: SkillInstallationChangesDto,
    pub expires_at_unix_ms: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillPackagePreviewDto {
    pub format_version: u32,
    pub package_revision: String,
    pub name: String,
    pub description: String,
    pub file_count: u64,
    pub total_bytes: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillsResolveInstallationSourceRequest {
    pub resolution_id: SkillResolutionIdDto,
    pub locator: SkillInstallationSourceLocatorDto,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillsCancelSourceResolutionRequest {
    pub resolution_id: SkillResolutionIdDto,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SkillSourceResolutionCancellationOutcomeDto {
    Cancelled,
    AlreadyCancelled,
    AlreadyConsumed,
    AlreadyAbsent,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillsCancelSourceResolutionResponse {
    pub schema_version: u32,
    pub resolution_id: SkillResolutionIdDto,
    pub outcome: SkillSourceResolutionCancellationOutcomeDto,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", deny_unknown_fields)]
pub enum SkillInstallationSourceLocatorDto {
    #[serde(rename = "url")]
    Url { url: String },
}

/// A canonical non-nil lower-case UUID generated by the client for one source resolution.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SkillResolutionIdDto(String);

impl SkillResolutionIdDto {
    pub fn parse(value: impl Into<String>) -> Result<Self, &'static str> {
        let value = value.into();
        let bytes = value.as_bytes();
        let valid = bytes.len() == 36
            && bytes.iter().enumerate().all(|(index, byte)| match index {
                8 | 13 | 18 | 23 => *byte == b'-',
                _ => byte.is_ascii_digit() || (b'a'..=b'f').contains(byte),
            })
            && bytes
                .iter()
                .any(|byte| matches!(*byte, b'1'..=b'9' | b'a'..=b'f'));
        if !valid {
            return Err("expected a canonical non-nil lower-case UUID");
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_inner(self) -> String {
        self.0
    }
}

impl Serialize for SkillResolutionIdDto {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for SkillResolutionIdDto {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(serde::de::Error::custom)
    }
}

/// A canonical, lower-case, complete Git commit SHA.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SkillResolvedGithubCommitShaDto(String);

impl SkillResolvedGithubCommitShaDto {
    pub fn parse(value: impl Into<String>) -> Result<Self, &'static str> {
        let value = value.into();
        let is_full_lowercase_sha = value.len() == 40
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
        if !is_full_lowercase_sha {
            return Err("expected a lower-case 40-character hexadecimal SHA");
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_inner(self) -> String {
        self.0
    }
}

impl Serialize for SkillResolvedGithubCommitShaDto {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for SkillResolvedGithubCommitShaDto {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SkillSourceResolutionProviderDto {
    Github,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SkillSourceResolutionOutcomeDto {
    Resolved,
    SelectionRequired,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", deny_unknown_fields)]
pub enum SkillResolvedAcquisitionSourceDto {
    #[serde(rename = "githubRepository", rename_all = "camelCase")]
    GithubRepository {
        owner: String,
        repository: String,
        reference: SkillGithubReferenceDto,
        resolved_commit: SkillResolvedGithubCommitShaDto,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subdirectory: Option<String>,
    },
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", deny_unknown_fields)]
pub enum SkillResolvedCandidateAcquisitionDto {
    #[serde(rename = "resolvedCandidate", rename_all = "camelCase")]
    ResolvedCandidate {
        resolution_id: SkillResolutionIdDto,
        candidate_id: String,
    },
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillSourceResolutionCandidateDto {
    pub candidate_id: String,
    pub acquisition: SkillResolvedCandidateAcquisitionDto,
    pub source: SkillResolvedAcquisitionSourceDto,
    pub package: SkillPackagePreviewDto,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillsResolveInstallationSourceResponse {
    pub schema_version: u32,
    pub resolution_id: SkillResolutionIdDto,
    pub canonical_url: String,
    pub provider: SkillSourceResolutionProviderDto,
    pub resolved_commit: SkillResolvedGithubCommitShaDto,
    pub expires_at_unix_ms: u64,
    pub outcome: SkillSourceResolutionOutcomeDto,
    pub candidates: Vec<SkillSourceResolutionCandidateDto>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SkillsResolveInstallationSourceResponseWire {
    schema_version: u32,
    resolution_id: SkillResolutionIdDto,
    canonical_url: String,
    provider: SkillSourceResolutionProviderDto,
    resolved_commit: SkillResolvedGithubCommitShaDto,
    expires_at_unix_ms: u64,
    outcome: SkillSourceResolutionOutcomeDto,
    candidates: Vec<SkillSourceResolutionCandidateDto>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SkillsResolveInstallationSourceResponseWireRef<'a> {
    schema_version: u32,
    resolution_id: &'a SkillResolutionIdDto,
    canonical_url: &'a str,
    provider: SkillSourceResolutionProviderDto,
    resolved_commit: &'a SkillResolvedGithubCommitShaDto,
    expires_at_unix_ms: u64,
    outcome: SkillSourceResolutionOutcomeDto,
    candidates: &'a [SkillSourceResolutionCandidateDto],
}

impl Serialize for SkillsResolveInstallationSourceResponse {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.validate().map_err(serde::ser::Error::custom)?;
        SkillsResolveInstallationSourceResponseWireRef {
            schema_version: self.schema_version,
            resolution_id: &self.resolution_id,
            canonical_url: &self.canonical_url,
            provider: self.provider,
            resolved_commit: &self.resolved_commit,
            expires_at_unix_ms: self.expires_at_unix_ms,
            outcome: self.outcome,
            candidates: &self.candidates,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for SkillsResolveInstallationSourceResponse {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = SkillsResolveInstallationSourceResponseWire::deserialize(deserializer)?;
        let response = Self {
            schema_version: wire.schema_version,
            resolution_id: wire.resolution_id,
            canonical_url: wire.canonical_url,
            provider: wire.provider,
            resolved_commit: wire.resolved_commit,
            expires_at_unix_ms: wire.expires_at_unix_ms,
            outcome: wire.outcome,
            candidates: wire.candidates,
        };
        response.validate().map_err(serde::de::Error::custom)?;
        Ok(response)
    }
}

impl SkillsResolveInstallationSourceResponse {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != SKILL_SOURCE_RESOLUTION_SCHEMA_VERSION {
            return Err(format!(
                "unsupported Skill source resolution schema version {}",
                self.schema_version
            ));
        }
        if self.canonical_url.trim().is_empty() {
            return Err("canonicalUrl must be non-empty".to_string());
        }
        if self.expires_at_unix_ms == 0 || self.expires_at_unix_ms > 9_007_199_254_740_991 {
            return Err("expiresAtUnixMs must be a positive JavaScript-safe integer".to_string());
        }
        match self.outcome {
            SkillSourceResolutionOutcomeDto::Resolved if self.candidates.len() != 1 => {
                return Err("resolved outcome requires exactly one candidate".to_string());
            }
            SkillSourceResolutionOutcomeDto::SelectionRequired if self.candidates.len() < 2 => {
                return Err(
                    "selectionRequired outcome requires at least two candidates".to_string()
                );
            }
            _ => {}
        }

        let mut candidate_ids = std::collections::HashSet::new();
        for candidate in &self.candidates {
            if candidate.candidate_id.trim().is_empty() {
                return Err("candidateId must be non-empty".to_string());
            }
            if !candidate_ids.insert(candidate.candidate_id.as_str()) {
                return Err("candidateId values must be unique".to_string());
            }
            let SkillResolvedCandidateAcquisitionDto::ResolvedCandidate {
                resolution_id,
                candidate_id,
            } = &candidate.acquisition;
            if resolution_id != &self.resolution_id {
                return Err(
                    "candidate acquisition resolutionId must match response.resolutionId"
                        .to_string(),
                );
            }
            if candidate_id != &candidate.candidate_id {
                return Err(
                    "candidate acquisition candidateId must match candidate.candidateId"
                        .to_string(),
                );
            }
            if candidate.package.format_version == 0
                || candidate.package.package_revision.trim().is_empty()
                || candidate.package.name.trim().is_empty()
                || candidate.package.description.trim().is_empty()
                || candidate.package.file_count == 0
                || candidate.package.total_bytes == 0
            {
                return Err("candidate package preview is invalid".to_string());
            }
            match &candidate.source {
                SkillResolvedAcquisitionSourceDto::GithubRepository {
                    owner,
                    repository,
                    reference,
                    resolved_commit,
                    subdirectory,
                } => {
                    if owner.trim().is_empty() || repository.trim().is_empty() {
                        return Err("candidate GitHub repository identity is invalid".to_string());
                    }
                    if matches!(subdirectory, Some(value) if value.trim().is_empty()) {
                        return Err(
                            "candidate subdirectory must be non-empty when present".to_string()
                        );
                    }
                    if resolved_commit != &self.resolved_commit {
                        return Err(
                            "candidate resolvedCommit must match response.resolvedCommit"
                                .to_string(),
                        );
                    }
                    match reference {
                        SkillGithubReferenceDto::DefaultBranch {} => {}
                        SkillGithubReferenceDto::Named { value } if value.trim().is_empty() => {
                            return Err("candidate named reference must be non-empty".to_string());
                        }
                        SkillGithubReferenceDto::Named { .. } => {}
                        SkillGithubReferenceDto::Commit { sha } => {
                            let reference_commit =
                                SkillResolvedGithubCommitShaDto::parse(sha.clone())
                                    .map_err(|reason| reason.to_string())?;
                            if &reference_commit != resolved_commit {
                                return Err("candidate commit reference sha must match candidate resolvedCommit".to_string());
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SkillSourceResolutionPhaseDto {
    Parse,
    Resolve,
    Discover,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SkillSourceResolutionErrorCodeDto {
    InvalidLocator,
    UnsupportedLocator,
    UnsupportedHost,
    UnsupportedUrlShape,
    RepositoryNotFound,
    ReferenceNotFound,
    PathNotFound,
    AmbiguousReference,
    NoSkillsFound,
    TooManySkills,
    NetworkUnavailable,
    RateLimited,
    RepositoryTooLarge,
    UnsafePackage,
    InvalidPackage,
    ResolutionIdConflict,
    ResolutionNotFoundOrExpired,
    ResolutionConsumed,
    CandidateNotFound,
    CapacityExceeded,
    Cancelled,
    Unavailable,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SkillSourceResolutionRecoveryDto {
    FixLocator,
    RetryLater,
    NarrowLocator,
    ChooseDifferentSource,
    RetrySameResolution,
    StartNewResolution,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillSourceResolutionErrorData {
    #[serde(rename = "type")]
    pub error_type: SkillSourceResolutionErrorTypeDto,
    pub phase: SkillSourceResolutionPhaseDto,
    pub code: SkillSourceResolutionErrorCodeDto,
    pub recovery: SkillSourceResolutionRecoveryDto,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<SkillSourceResolutionProviderDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SkillSourceResolutionErrorTypeDto {
    SkillSourceResolution,
}

/// A presentation-safe source summary. Authority-bearing local paths and
/// credentials must never be placed in this DTO.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", deny_unknown_fields)]
pub enum SkillPreviewSourceDto {
    #[serde(rename = "localDirectory", rename_all = "camelCase")]
    LocalDirectory {
        display_name: String,
        refreshable: bool,
    },
    #[serde(rename = "githubRepository", rename_all = "camelCase")]
    GithubRepository {
        owner: String,
        repository: String,
        reference: SkillGithubReferenceDto,
        resolved_commit: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subdirectory: Option<String>,
        refreshable: bool,
    },
    #[serde(rename = "installedSource", rename_all = "camelCase")]
    InstalledSource {
        display_name: String,
        refreshable: bool,
    },
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SkillCompatibilityStatusDto {
    Compatible,
    CompatibleWithWarnings,
    Unknown,
    Incompatible,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SkillCompatibilityIssueSeverityDto {
    Warning,
    Error,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillCompatibilityIssueDto {
    pub id: String,
    pub code: String,
    pub severity: SkillCompatibilityIssueSeverityDto,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability: Option<String>,
    pub requires_acknowledgement: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillCompatibilityReportDto {
    pub status: SkillCompatibilityStatusDto,
    pub issues: Vec<SkillCompatibilityIssueDto>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SkillInstallationChangeDto {
    New,
    Changed,
    Unchanged,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillInstallationChangesDto {
    pub content: SkillInstallationChangeDto,
    pub source: SkillInstallationChangeDto,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillsCommitInstallationRequest {
    pub preparation_id: String,
    pub preview_revision: String,
    pub accepted_issue_ids: Vec<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SkillInstallationCommitOutcomeDto {
    Installed,
    AlreadyInstalled,
    Updated,
    AlreadyCurrent,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillInstallationCommitResponse {
    pub schema_version: u32,
    pub preparation_id: String,
    pub operation: SkillPreparationOperationDto,
    pub outcome: SkillInstallationCommitOutcomeDto,
    pub installation_id: String,
    pub skill_id: String,
    pub package_revision: String,
    pub installation_revision: String,
    pub changes: SkillInstallationChangesDto,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillsCancelPreparationRequest {
    pub preparation_id: String,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SkillPreparationCancellationOutcomeDto {
    Cancelled,
    AlreadyAbsent,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillPreparationCancellationResponse {
    pub schema_version: u32,
    pub preparation_id: String,
    pub outcome: SkillPreparationCancellationOutcomeDto,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SkillInspectionPhaseDto {
    Inspect,
    Commit,
    Cancel,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SkillInspectionErrorCodeDto {
    InvalidSource,
    UnsupportedSource,
    SourceNotAccessible,
    ReferenceNotFound,
    SubdirectoryNotFound,
    SourceChangedDuringRead,
    ResolutionNotFound,
    ResolutionExpired,
    ResolutionConsumed,
    CandidateNotFound,
    CapacityExceeded,
    InstallationRetired,
    InstallationNotFound,
    InstallationRevisionConflict,
    SourceNotRefreshable,
    PersistedSourceInvalid,
    NetworkUnavailable,
    RateLimited,
    RepositoryTooLarge,
    UnsafePackage,
    InvalidPackage,
    Incompatible,
    PreparationNotFound,
    PreparationExpired,
    IdempotencyConflict,
    PreviewMismatch,
    AcknowledgementRequired,
    CommitIndeterminate,
    Cancelled,
    Unavailable,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SkillInspectionRecoveryDto {
    FixSource,
    RetrySamePreparation,
    RetryLater,
    InspectAgain,
    NewInstallationIdentity,
    FreeCapacity,
    ContactSupport,
    AcknowledgeWarnings,
    ChooseDifferentSource,
    ResolveAgain,
    RefreshManagement,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillInspectionErrorData {
    #[serde(rename = "type")]
    pub error_type: SkillInspectionErrorTypeDto,
    pub phase: SkillInspectionPhaseDto,
    pub code: SkillInspectionErrorCodeDto,
    pub recovery: SkillInspectionRecoveryDto,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preparation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostic_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "bool_is_false")]
    pub commit_may_have_succeeded: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intended_installation_revision: Option<String>,
}

fn bool_is_false(value: &bool) -> bool {
    !*value
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SkillInspectionErrorTypeDto {
    SkillInspection,
}
