//! Public GitHub acquisition for immutable Skill package snapshots.
//!
//! This adapter intentionally accepts structured GitHub coordinates instead
//! of arbitrary URLs. It resolves every requested ref through GitHub's public
//! API, downloads a ZIP for the resulting full commit SHA from codeload, and
//! keeps extraction entirely in memory. The only output that crosses the
//! installation boundary is a fully validated [`PreparedSkillPackage`].

use super::installation_workflow::{
    SkillAcquisitionAdapter, SkillAcquisitionAdapterError, SkillAcquisitionProvider,
    SkillAcquisitionSource,
};
use super::origin::SkillPackageOrigin;
use super::package::{
    MAX_SKILL_PACKAGE_BYTES, MAX_SKILL_PACKAGE_FILES, MAX_SKILL_RESOURCE_FILE_BYTES,
};
use super::prepared::{PreparedSkillPackage, SkillPackagePreparationError};
use super::workspace::{MAX_SKILL_FILE_BYTES, SKILL_FILE_NAME};
use reqwest::blocking::{Client, Response};
use reqwest::header::{ACCEPT, CONTENT_LENGTH};
use reqwest::{StatusCode, Url};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::io::{Cursor, Read};
use std::sync::Arc;
use std::time::Duration;
use zip::{CompressionMethod, ZipArchive};

pub const GITHUB_SKILL_ORIGIN_PROVIDER: &str = "github";

const GITHUB_USER_AGENT: &str = "MyCopilot-Skill-Acquisition/1";
const GITHUB_API_VERSION: &str = "2022-11-28";
const GITHUB_API_BODY_BYTES: usize = 256 * 1024;
const MAX_GITHUB_ZIP_BYTES: usize = 32 * 1024 * 1024;
const MAX_GITHUB_ARCHIVE_ENTRIES: usize = 20_000;
const MAX_GITHUB_ARCHIVE_PATH_BYTES: usize = 4 * 1024;
const MAX_GITHUB_ARCHIVE_PATH_COMPONENTS: usize = 256;
const MAX_GITHUB_ARCHIVE_PATH_COMPONENT_BYTES: usize = 255;
const MAX_GITHUB_ARCHIVE_ENTRY_BYTES: u64 = 128 * 1024 * 1024;
const MAX_GITHUB_ARCHIVE_UNCOMPRESSED_BYTES: u64 = 512 * 1024 * 1024;
const MAX_GITHUB_EXPANSION_RATIO: u64 = 200;
const EXPANSION_RATIO_GRACE_BYTES: u64 = 1024 * 1024;

/// A validated public GitHub repository coordinate.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GitHubRepository {
    owner: String,
    name: String,
}

impl GitHubRepository {
    pub fn parse(
        owner: impl Into<String>,
        name: impl Into<String>,
    ) -> Result<Self, GitHubAcquisitionError> {
        let owner = owner.into();
        let name = name.into();
        if !is_valid_owner(&owner) || !is_valid_repository_name(&name) {
            return Err(acquisition_error(
                GitHubAcquisitionErrorCode::InvalidRepository,
                "GitHub owner and repository must be canonical public repository names.",
            ));
        }
        Ok(Self { owner, name })
    }

    pub fn owner(&self) -> &str {
        &self.owner
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn display_name(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }
}

/// A validated moving Git ref (branch or tag).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GitHubNamedReference(String);

impl GitHubNamedReference {
    pub fn parse(value: impl Into<String>) -> Result<Self, GitHubAcquisitionError> {
        let value = value.into();
        if !is_valid_named_reference(&value) {
            return Err(acquisition_error(
                GitHubAcquisitionErrorCode::InvalidReference,
                "GitHub branch or tag reference is not a valid bounded ref name.",
            ));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// An immutable, full SHA-1 commit object identifier returned by GitHub.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GitHubCommit(String);

impl GitHubCommit {
    pub fn parse(value: impl Into<String>) -> Result<Self, GitHubAcquisitionError> {
        let value = value.into();
        if value.len() != 40 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(acquisition_error(
                GitHubAcquisitionErrorCode::InvalidCommit,
                "GitHub did not return a full 40-character commit identifier.",
            ));
        }
        Ok(Self(value.to_ascii_lowercase()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum GitHubReference {
    DefaultBranch,
    Named(GitHubNamedReference),
    Commit(GitHubCommit),
}

impl GitHubReference {
    pub fn named(value: impl Into<String>) -> Result<Self, GitHubAcquisitionError> {
        Ok(Self::Named(GitHubNamedReference::parse(value)?))
    }

    pub fn commit(value: impl Into<String>) -> Result<Self, GitHubAcquisitionError> {
        Ok(Self::Commit(GitHubCommit::parse(value)?))
    }

    fn stable_kind(&self) -> &'static str {
        match self {
            Self::DefaultBranch => "defaultBranch",
            Self::Named(_) => "named",
            Self::Commit(_) => "commit",
        }
    }

    fn value(&self) -> Option<&str> {
        match self {
            Self::DefaultBranch => None,
            Self::Named(reference) => Some(reference.as_str()),
            Self::Commit(commit) => Some(commit.as_str()),
        }
    }
}

/// A validated directory relative to the repository root. Empty means root.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct GitHubSubdirectory {
    value: String,
    components: Vec<String>,
}

impl GitHubSubdirectory {
    pub fn root() -> Self {
        Self::default()
    }

    pub fn parse(value: impl Into<String>) -> Result<Self, GitHubAcquisitionError> {
        let value = value.into();
        if value.is_empty() {
            return Ok(Self::root());
        }
        if value.len() > 1024
            || value.starts_with('/')
            || value.ends_with('/')
            || value.contains(['\\', '\0'])
        {
            return Err(invalid_subdirectory());
        }
        let components = value.split('/').map(str::to_string).collect::<Vec<_>>();
        if components.len() > 32
            || components.iter().any(|component| {
                component.is_empty()
                    || matches!(component.as_str(), "." | "..")
                    || component.len() > 255
                    || component.chars().any(char::is_control)
                    || component.contains(':')
                    || component.ends_with(['.', ' '])
            })
        {
            return Err(invalid_subdirectory());
        }
        Ok(Self { value, components })
    }

    pub fn as_str(&self) -> &str {
        &self.value
    }

    fn components(&self) -> &[String] {
        &self.components
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitHubSkillLocation {
    repository: GitHubRepository,
    reference: GitHubReference,
    subdirectory: GitHubSubdirectory,
}

impl GitHubSkillLocation {
    pub fn new(
        repository: GitHubRepository,
        reference: GitHubReference,
        subdirectory: GitHubSubdirectory,
    ) -> Self {
        Self {
            repository,
            reference,
            subdirectory,
        }
    }

    pub fn repository(&self) -> &GitHubRepository {
        &self.repository
    }

    pub fn reference(&self) -> &GitHubReference {
        &self.reference
    }

    pub fn subdirectory(&self) -> &GitHubSubdirectory {
        &self.subdirectory
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitHubResolveRequest {
    repository: GitHubRepository,
    reference: GitHubReference,
}

impl GitHubResolveRequest {
    pub fn repository(&self) -> &GitHubRepository {
        &self.repository
    }

    pub fn reference(&self) -> &GitHubReference {
        &self.reference
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitHubArchiveRequest {
    repository: GitHubRepository,
    commit: GitHubCommit,
}

impl GitHubArchiveRequest {
    pub fn repository(&self) -> &GitHubRepository {
        &self.repository
    }

    pub fn commit(&self) -> &GitHubCommit {
        &self.commit
    }
}

/// The narrow network seam used by acquisition. Implementations return no
/// response bodies or arbitrary error strings, preventing accidental secret
/// propagation through higher-level diagnostics.
pub trait GitHubAcquisitionTransport: Send + Sync {
    fn resolve_commit(
        &self,
        request: &GitHubResolveRequest,
    ) -> Result<GitHubCommit, GitHubTransportError>;

    fn download_archive(
        &self,
        request: &GitHubArchiveRequest,
    ) -> Result<Vec<u8>, GitHubTransportError>;
}

/// Network transport restricted to GitHub's fixed public API and codeload
/// hosts. Redirects and credentials are deliberately unsupported.
pub struct ReqwestGitHubTransport {
    client: Client,
}

impl fmt::Debug for ReqwestGitHubTransport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReqwestGitHubTransport")
            .finish_non_exhaustive()
    }
}

impl ReqwestGitHubTransport {
    pub fn new() -> Result<Self, GitHubTransportError> {
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(60))
            .user_agent(GITHUB_USER_AGENT)
            .build()
            .map_err(|_| GitHubTransportError::Unavailable)?;
        Ok(Self { client })
    }

    fn resolve_ref(
        &self,
        repository: &GitHubRepository,
        reference: &str,
    ) -> Result<GitHubCommit, GitHubTransportError> {
        let url = github_api_commit_url(repository, reference)?;
        let body = self.get_bounded_json(url)?;
        let response: CommitApiResponse =
            serde_json::from_slice(&body).map_err(|_| GitHubTransportError::InvalidResponse)?;
        GitHubCommit::parse(response.sha).map_err(|_| GitHubTransportError::InvalidResponse)
    }

    fn get_bounded_json(&self, url: Url) -> Result<Vec<u8>, GitHubTransportError> {
        let response = self
            .client
            .get(url)
            .header(ACCEPT, "application/vnd.github+json")
            .header("X-GitHub-Api-Version", GITHUB_API_VERSION)
            .send()
            .map_err(|_| GitHubTransportError::Unavailable)?;
        read_success_response(response, GITHUB_API_BODY_BYTES, true)
    }
}

impl GitHubAcquisitionTransport for ReqwestGitHubTransport {
    fn resolve_commit(
        &self,
        request: &GitHubResolveRequest,
    ) -> Result<GitHubCommit, GitHubTransportError> {
        match request.reference() {
            GitHubReference::DefaultBranch => {
                let repository_url = github_api_repository_url(request.repository())?;
                let body = self.get_bounded_json(repository_url)?;
                let repository: RepositoryApiResponse = serde_json::from_slice(&body)
                    .map_err(|_| GitHubTransportError::InvalidResponse)?;
                let default_branch = GitHubNamedReference::parse(repository.default_branch)
                    .map_err(|_| GitHubTransportError::InvalidResponse)?;
                self.resolve_ref(request.repository(), default_branch.as_str())
            }
            GitHubReference::Named(reference) => {
                self.resolve_ref(request.repository(), reference.as_str())
            }
            GitHubReference::Commit(commit) => {
                self.resolve_ref(request.repository(), commit.as_str())
            }
        }
    }

    fn download_archive(
        &self,
        request: &GitHubArchiveRequest,
    ) -> Result<Vec<u8>, GitHubTransportError> {
        let url = github_codeload_url(request.repository(), request.commit())?;
        let response = self
            .client
            .get(url)
            .header(ACCEPT, "application/zip")
            .send()
            .map_err(|_| GitHubTransportError::Unavailable)?;
        read_success_response(response, MAX_GITHUB_ZIP_BYTES, false)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum GitHubTransportError {
    NotFound,
    RateLimited,
    Rejected,
    ResponseTooLarge,
    InvalidResponse,
    Unavailable,
}

impl fmt::Display for GitHubTransportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let reason = match self {
            Self::NotFound => "The public GitHub repository or reference was not found.",
            Self::RateLimited => "GitHub temporarily rate-limited this public request.",
            Self::Rejected => "GitHub rejected this public request.",
            Self::ResponseTooLarge => {
                "GitHub returned a response larger than the acquisition limit."
            }
            Self::InvalidResponse => "GitHub returned an invalid acquisition response.",
            Self::Unavailable => "GitHub acquisition is temporarily unavailable.",
        };
        reason.fmt(formatter)
    }
}

impl Error for GitHubTransportError {}

pub struct GitHubSkillAcquirer {
    transport: Arc<dyn GitHubAcquisitionTransport>,
}

impl fmt::Debug for GitHubSkillAcquirer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GitHubSkillAcquirer")
            .finish_non_exhaustive()
    }
}

impl GitHubSkillAcquirer {
    pub fn new() -> Result<Self, GitHubAcquisitionError> {
        let transport = ReqwestGitHubTransport::new().map_err(map_transport_error)?;
        Ok(Self::with_transport(Arc::new(transport)))
    }

    pub fn with_transport(transport: Arc<dyn GitHubAcquisitionTransport>) -> Self {
        Self { transport }
    }

    pub fn acquire(
        &self,
        location: GitHubSkillLocation,
    ) -> Result<AcquiredGitHubSkill, GitHubAcquisitionError> {
        let resolve_request = GitHubResolveRequest {
            repository: location.repository.clone(),
            reference: location.reference.clone(),
        };
        let commit = self
            .transport
            .resolve_commit(&resolve_request)
            .map_err(map_transport_error)?;
        let archive_request = GitHubArchiveRequest {
            repository: location.repository.clone(),
            commit: commit.clone(),
        };
        let archive = self
            .transport
            .download_archive(&archive_request)
            .map_err(map_transport_error)?;
        if archive.len() > MAX_GITHUB_ZIP_BYTES {
            return Err(acquisition_error(
                GitHubAcquisitionErrorCode::ArchiveTooLarge,
                "The GitHub archive exceeds the compressed acquisition limit.",
            ));
        }
        let files = extract_selected_skill(&archive, location.subdirectory())?;
        let summary = GitHubAcquisitionSummary {
            owner: location.repository.owner.clone(),
            repository: location.repository.name.clone(),
            requested_reference_kind: location.reference.stable_kind(),
            requested_reference: location.reference.value().map(str::to_string),
            resolved_commit: commit.clone(),
            subdirectory: location.subdirectory.as_str().to_string(),
        };
        let origin = summary.origin()?;
        let package = PreparedSkillPackage::from_files(files, origin).map_err(map_package_error)?;
        Ok(AcquiredGitHubSkill { package, summary })
    }
}

/// Workflow adapter for the protocol's exact `githubRepository` source shape.
///
/// Callers should use [`Self::source_for`] instead of assembling opaque
/// adapter bytes. That keeps the strict JSON wire owned by this adapter and
/// prevents server and client glue from drifting apart.
pub struct GitHubWorkflowAcquisitionAdapter {
    acquirer: Arc<GitHubSkillAcquirer>,
}

impl fmt::Debug for GitHubWorkflowAcquisitionAdapter {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GitHubWorkflowAcquisitionAdapter")
            .finish_non_exhaustive()
    }
}

impl GitHubWorkflowAcquisitionAdapter {
    pub fn new(acquirer: Arc<GitHubSkillAcquirer>) -> Self {
        Self { acquirer }
    }

    pub fn public_github() -> Result<Self, GitHubAcquisitionError> {
        Ok(Self::new(Arc::new(GitHubSkillAcquirer::new()?)))
    }

    /// Encodes a validated location into the workflow's generic provider
    /// envelope using the same strict JSON schema consumed by [`Self::acquire`].
    pub fn source_for(
        location: &GitHubSkillLocation,
    ) -> Result<SkillAcquisitionSource, GitHubAcquisitionError> {
        let request = GitHubWorkflowRequest::from_location(location);
        let bytes = serde_json::to_vec(&request).map_err(|_| invalid_workflow_request())?;
        SkillAcquisitionSource::adapter(github_provider(), bytes)
            .map_err(|_| invalid_workflow_request())
    }
}

impl SkillAcquisitionAdapter for GitHubWorkflowAcquisitionAdapter {
    fn provider(&self) -> SkillAcquisitionProvider {
        github_provider()
    }

    fn acquire(
        &self,
        source: &SkillAcquisitionSource,
    ) -> Result<PreparedSkillPackage, SkillAcquisitionAdapterError> {
        let request_bytes = source.adapter_request().ok_or_else(|| {
            SkillAcquisitionAdapterError::invalid_request(
                "github provider received a different acquisition source envelope",
            )
        })?;
        let request: GitHubWorkflowRequest =
            serde_json::from_slice(request_bytes).map_err(|_| {
                SkillAcquisitionAdapterError::invalid_request(
                "githubRepository acquisition request does not match the strict protocol schema",
            )
            })?;
        let location = request
            .into_location()
            .map_err(map_acquisition_to_workflow_error)?;
        self.acquirer
            .acquire(location)
            .map(AcquiredGitHubSkill::into_package)
            .map_err(map_acquisition_to_workflow_error)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", deny_unknown_fields)]
enum GitHubWorkflowRequest {
    #[serde(rename = "githubRepository", rename_all = "camelCase")]
    Repository {
        owner: String,
        repository: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reference: Option<GitHubWorkflowReference>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subdirectory: Option<String>,
    },
}

impl GitHubWorkflowRequest {
    fn from_location(location: &GitHubSkillLocation) -> Self {
        Self::Repository {
            owner: location.repository.owner.clone(),
            repository: location.repository.name.clone(),
            reference: Some(GitHubWorkflowReference::from_reference(&location.reference)),
            subdirectory: (!location.subdirectory.value.is_empty())
                .then(|| location.subdirectory.value.clone()),
        }
    }

    fn into_location(self) -> Result<GitHubSkillLocation, GitHubAcquisitionError> {
        match self {
            Self::Repository {
                owner,
                repository,
                reference,
                subdirectory,
            } => {
                if subdirectory.as_deref() == Some("") {
                    return Err(invalid_subdirectory());
                }
                Ok(GitHubSkillLocation::new(
                    GitHubRepository::parse(owner, repository)?,
                    reference
                        .unwrap_or(GitHubWorkflowReference::DefaultBranch)
                        .into_reference()?,
                    GitHubSubdirectory::parse(subdirectory.unwrap_or_default())?,
                ))
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", deny_unknown_fields)]
enum GitHubWorkflowReference {
    #[serde(rename = "defaultBranch")]
    DefaultBranch,
    #[serde(rename = "named")]
    Named { value: String },
    #[serde(rename = "commit")]
    Commit { sha: String },
}

impl GitHubWorkflowReference {
    fn from_reference(reference: &GitHubReference) -> Self {
        match reference {
            GitHubReference::DefaultBranch => Self::DefaultBranch,
            GitHubReference::Named(reference) => Self::Named {
                value: reference.as_str().to_string(),
            },
            GitHubReference::Commit(commit) => Self::Commit {
                sha: commit.as_str().to_string(),
            },
        }
    }

    fn into_reference(self) -> Result<GitHubReference, GitHubAcquisitionError> {
        match self {
            Self::DefaultBranch => Ok(GitHubReference::DefaultBranch),
            Self::Named { value } => GitHubReference::named(value),
            Self::Commit { sha } => GitHubReference::commit(sha),
        }
    }
}

pub struct AcquiredGitHubSkill {
    package: PreparedSkillPackage,
    summary: GitHubAcquisitionSummary,
}

impl fmt::Debug for AcquiredGitHubSkill {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AcquiredGitHubSkill")
            .field("summary", &self.summary)
            .field("package_revision", &self.package.revision())
            .finish()
    }
}

impl AcquiredGitHubSkill {
    pub fn package(&self) -> &PreparedSkillPackage {
        &self.package
    }

    pub fn summary(&self) -> &GitHubAcquisitionSummary {
        &self.summary
    }

    pub fn into_package(self) -> PreparedSkillPackage {
        self.package
    }
}

/// Sanitized, replayable acquisition metadata. It contains no credential,
/// arbitrary URL, local filesystem path, temporary path, or response body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitHubAcquisitionSummary {
    owner: String,
    repository: String,
    requested_reference_kind: &'static str,
    requested_reference: Option<String>,
    resolved_commit: GitHubCommit,
    subdirectory: String,
}

impl GitHubAcquisitionSummary {
    pub fn owner(&self) -> &str {
        &self.owner
    }

    pub fn repository(&self) -> &str {
        &self.repository
    }

    pub fn requested_reference_kind(&self) -> &'static str {
        self.requested_reference_kind
    }

    pub fn requested_reference(&self) -> Option<&str> {
        self.requested_reference.as_deref()
    }

    pub fn resolved_commit(&self) -> &GitHubCommit {
        &self.resolved_commit
    }

    pub fn subdirectory(&self) -> &str {
        &self.subdirectory
    }

    fn origin(&self) -> Result<SkillPackageOrigin, GitHubAcquisitionError> {
        let requested_reference = match (
            self.requested_reference_kind,
            self.requested_reference.as_deref(),
        ) {
            ("defaultBranch", None) => GitHubOriginRequestedReference::DefaultBranch,
            ("named", Some(value)) => GitHubOriginRequestedReference::Named { value },
            ("commit", Some(sha)) => GitHubOriginRequestedReference::Commit { sha },
            _ => {
                return Err(acquisition_error(
                    GitHubAcquisitionErrorCode::InvalidOrigin,
                    "Sanitized GitHub acquisition metadata is internally inconsistent.",
                ));
            }
        };
        let reference = serde_json::to_string(&GitHubOriginReference {
            schema_version: 1,
            owner: &self.owner,
            repository: &self.repository,
            requested_reference,
            resolved_commit: self.resolved_commit.as_str(),
            subdirectory: (!self.subdirectory.is_empty()).then_some(self.subdirectory.as_str()),
        })
        .map_err(|_| {
            acquisition_error(
                GitHubAcquisitionErrorCode::InvalidOrigin,
                "Cannot encode sanitized GitHub acquisition metadata.",
            )
        })?;
        SkillPackageOrigin::new(GITHUB_SKILL_ORIGIN_PROVIDER, reference).map_err(|_| {
            acquisition_error(
                GitHubAcquisitionErrorCode::InvalidOrigin,
                "Sanitized GitHub acquisition metadata exceeds the origin contract.",
            )
        })
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GitHubOriginReference<'a> {
    schema_version: u32,
    owner: &'a str,
    repository: &'a str,
    requested_reference: GitHubOriginRequestedReference<'a>,
    resolved_commit: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    subdirectory: Option<&'a str>,
}

#[derive(Serialize)]
#[serde(tag = "kind")]
enum GitHubOriginRequestedReference<'a> {
    #[serde(rename = "defaultBranch")]
    DefaultBranch,
    #[serde(rename = "named")]
    Named { value: &'a str },
    #[serde(rename = "commit")]
    Commit { sha: &'a str },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum GitHubAcquisitionErrorCode {
    InvalidRepository,
    InvalidReference,
    InvalidCommit,
    InvalidSubdirectory,
    InvalidWorkflowRequest,
    NotFound,
    RateLimited,
    TransportRejected,
    Unavailable,
    ArchiveTooLarge,
    InvalidArchive,
    UnsafeArchiveEntry,
    ArchiveBudgetExceeded,
    SkillNotFound,
    PackageRejected,
    InvalidOrigin,
}

#[derive(Clone, PartialEq, Eq)]
pub struct GitHubAcquisitionError {
    code: GitHubAcquisitionErrorCode,
    reason: String,
    preparation: Option<Box<SkillPackagePreparationError>>,
}

impl GitHubAcquisitionError {
    pub fn code(&self) -> GitHubAcquisitionErrorCode {
        self.code
    }

    pub fn reason(&self) -> &str {
        &self.reason
    }

    pub fn into_preparation_error(self) -> Option<SkillPackagePreparationError> {
        self.preparation.map(|error| *error)
    }
}

impl fmt::Debug for GitHubAcquisitionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GitHubAcquisitionError")
            .field("code", &self.code)
            .field("reason", &"[redacted]")
            .field("has_preparation_error", &self.preparation.is_some())
            .finish()
    }
}

impl fmt::Display for GitHubAcquisitionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.reason.fmt(formatter)
    }
}

impl Error for GitHubAcquisitionError {}

#[derive(Deserialize)]
struct RepositoryApiResponse {
    default_branch: String,
}

#[derive(Deserialize)]
struct CommitApiResponse {
    sha: String,
}

fn read_success_response(
    mut response: Response,
    max_bytes: usize,
    json_response: bool,
) -> Result<Vec<u8>, GitHubTransportError> {
    let status = response.status();
    if !status.is_success() {
        return Err(map_status(status));
    }
    if let Some(length) = response
        .headers()
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
    {
        if length > max_bytes as u64 {
            return Err(GitHubTransportError::ResponseTooLarge);
        }
    }
    if json_response {
        let content_type_is_json = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| {
                value.split(';').next().is_some_and(|mime| {
                    mime.trim().ends_with("/json") || mime.trim().ends_with("+json")
                })
            });
        if !content_type_is_json {
            return Err(GitHubTransportError::InvalidResponse);
        }
    }
    let mut body = Vec::new();
    response
        .by_ref()
        .take(max_bytes.saturating_add(1) as u64)
        .read_to_end(&mut body)
        .map_err(|_| GitHubTransportError::Unavailable)?;
    if body.len() > max_bytes {
        return Err(GitHubTransportError::ResponseTooLarge);
    }
    Ok(body)
}

fn map_status(status: StatusCode) -> GitHubTransportError {
    match status {
        StatusCode::NOT_FOUND => GitHubTransportError::NotFound,
        StatusCode::TOO_MANY_REQUESTS => GitHubTransportError::RateLimited,
        StatusCode::FORBIDDEN => GitHubTransportError::RateLimited,
        status if status.is_server_error() => GitHubTransportError::Unavailable,
        _ => GitHubTransportError::Rejected,
    }
}

fn github_api_repository_url(repository: &GitHubRepository) -> Result<Url, GitHubTransportError> {
    github_url(
        "https://api.github.com/",
        &["repos", repository.owner(), repository.name()],
    )
}

fn github_api_commit_url(
    repository: &GitHubRepository,
    reference: &str,
) -> Result<Url, GitHubTransportError> {
    github_url(
        "https://api.github.com/",
        &[
            "repos",
            repository.owner(),
            repository.name(),
            "commits",
            reference,
        ],
    )
}

fn github_codeload_url(
    repository: &GitHubRepository,
    commit: &GitHubCommit,
) -> Result<Url, GitHubTransportError> {
    github_url(
        "https://codeload.github.com/",
        &[
            repository.owner(),
            repository.name(),
            "zip",
            commit.as_str(),
        ],
    )
}

fn github_url(base: &str, segments: &[&str]) -> Result<Url, GitHubTransportError> {
    let mut url = Url::parse(base).map_err(|_| GitHubTransportError::InvalidResponse)?;
    let mut path = url
        .path_segments_mut()
        .map_err(|_| GitHubTransportError::InvalidResponse)?;
    path.pop_if_empty();
    for segment in segments {
        path.push(segment);
    }
    drop(path);
    Ok(url)
}

#[derive(Debug)]
struct SelectedArchiveEntry {
    index: usize,
    logical_path: String,
    size: usize,
}

fn extract_selected_skill(
    archive_bytes: &[u8],
    subdirectory: &GitHubSubdirectory,
) -> Result<Vec<(String, Vec<u8>)>, GitHubAcquisitionError> {
    let reader = Cursor::new(archive_bytes);
    let mut archive = ZipArchive::new(reader).map_err(|_| invalid_archive())?;
    if archive.is_empty() || archive.len() > MAX_GITHUB_ARCHIVE_ENTRIES {
        return Err(acquisition_error(
            GitHubAcquisitionErrorCode::ArchiveBudgetExceeded,
            format!("GitHub archive must contain 1 to {MAX_GITHUB_ARCHIVE_ENTRIES} entries."),
        ));
    }

    let mut top_root: Option<String> = None;
    let mut portable_entries = BTreeMap::<String, String>::new();
    let mut portable_files = BTreeMap::<String, String>::new();
    let mut portable_directories = BTreeMap::<String, String>::new();
    let mut archive_uncompressed_bytes = 0_u64;
    let mut selected_bytes = 0usize;
    let mut selected = Vec::new();

    for index in 0..archive.len() {
        let entry = archive.by_index(index).map_err(|_| invalid_archive())?;
        let components = validate_archive_entry(&entry)?;
        let root = components.first().expect("validated archive path");
        match &top_root {
            None => top_root = Some(root.clone()),
            Some(expected) if expected == root => {}
            Some(_) => {
                return Err(acquisition_error(
                    GitHubAcquisitionErrorCode::UnsafeArchiveEntry,
                    "GitHub archive contains more than one top-level root.",
                ));
            }
        }
        let canonical_path = components.join("/");
        let collision_key = canonical_path.to_lowercase();
        if let Some(previous) = portable_entries.insert(collision_key, canonical_path.clone()) {
            let collision = if previous == canonical_path {
                "duplicate paths"
            } else {
                "paths that collide on a case-insensitive filesystem"
            };
            return Err(acquisition_error(
                GitHubAcquisitionErrorCode::UnsafeArchiveEntry,
                format!("GitHub archive contains {collision}."),
            ));
        }
        validate_portable_archive_tree(
            &components,
            entry.is_dir(),
            &mut portable_files,
            &mut portable_directories,
        )?;

        let size = entry.size();
        if size > MAX_GITHUB_ARCHIVE_ENTRY_BYTES {
            return Err(acquisition_error(
                GitHubAcquisitionErrorCode::ArchiveBudgetExceeded,
                "A GitHub archive entry exceeds the uncompressed entry limit.",
            ));
        }
        archive_uncompressed_bytes =
            archive_uncompressed_bytes
                .checked_add(size)
                .ok_or_else(|| {
                    acquisition_error(
                        GitHubAcquisitionErrorCode::ArchiveBudgetExceeded,
                        "GitHub archive size accounting overflowed.",
                    )
                })?;
        if archive_uncompressed_bytes > MAX_GITHUB_ARCHIVE_UNCOMPRESSED_BYTES {
            return Err(acquisition_error(
                GitHubAcquisitionErrorCode::ArchiveBudgetExceeded,
                "GitHub archive exceeds the total uncompressed acquisition limit.",
            ));
        }
        if suspicious_expansion(size, entry.compressed_size()) {
            return Err(acquisition_error(
                GitHubAcquisitionErrorCode::ArchiveBudgetExceeded,
                "GitHub archive contains an entry with an unsafe compression expansion ratio.",
            ));
        }

        let repository_relative = &components[1..];
        let Some(logical_components) = repository_relative.strip_prefix(subdirectory.components())
        else {
            continue;
        };
        if logical_components.is_empty() {
            if entry.is_dir() {
                continue;
            }
            return Err(acquisition_error(
                GitHubAcquisitionErrorCode::UnsafeArchiveEntry,
                "The selected Skill directory resolves to a file.",
            ));
        }
        validate_selected_package_path(logical_components, entry.is_dir())?;
        if entry.is_dir() {
            continue;
        }
        if selected.len() >= MAX_SKILL_PACKAGE_FILES {
            return Err(acquisition_error(
                GitHubAcquisitionErrorCode::ArchiveBudgetExceeded,
                format!("Selected Skill contains more than {MAX_SKILL_PACKAGE_FILES} files."),
            ));
        }
        let logical_path = logical_components.join("/");
        let file_limit = if logical_path == SKILL_FILE_NAME {
            MAX_SKILL_FILE_BYTES
        } else {
            MAX_SKILL_RESOURCE_FILE_BYTES
        };
        let size = usize::try_from(size).map_err(|_| {
            acquisition_error(
                GitHubAcquisitionErrorCode::ArchiveBudgetExceeded,
                "Selected Skill file size does not fit this platform.",
            )
        })?;
        if size > file_limit {
            return Err(acquisition_error(
                GitHubAcquisitionErrorCode::ArchiveBudgetExceeded,
                "A selected Skill file exceeds its package limit.",
            ));
        }
        selected_bytes = selected_bytes.checked_add(size).ok_or_else(|| {
            acquisition_error(
                GitHubAcquisitionErrorCode::ArchiveBudgetExceeded,
                "Selected Skill size accounting overflowed.",
            )
        })?;
        if selected_bytes > MAX_SKILL_PACKAGE_BYTES {
            return Err(acquisition_error(
                GitHubAcquisitionErrorCode::ArchiveBudgetExceeded,
                format!("Selected Skill exceeds {MAX_SKILL_PACKAGE_BYTES} bytes."),
            ));
        }
        selected.push(SelectedArchiveEntry {
            index,
            logical_path,
            size,
        });
    }

    if !selected
        .iter()
        .any(|entry| entry.logical_path == SKILL_FILE_NAME)
    {
        return Err(acquisition_error(
            GitHubAcquisitionErrorCode::SkillNotFound,
            "The selected GitHub directory does not contain an exact-case SKILL.md file.",
        ));
    }

    let mut files = Vec::with_capacity(selected.len());
    let mut actual_total = 0usize;
    for selected_entry in selected {
        let mut entry = archive
            .by_index(selected_entry.index)
            .map_err(|_| invalid_archive())?;
        let mut bytes = Vec::with_capacity(selected_entry.size.min(1024 * 1024));
        entry
            .by_ref()
            .take(selected_entry.size.saturating_add(1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| invalid_archive())?;
        if bytes.len() != selected_entry.size {
            return Err(acquisition_error(
                GitHubAcquisitionErrorCode::InvalidArchive,
                "A GitHub archive entry did not match its declared uncompressed size.",
            ));
        }
        actual_total = actual_total.checked_add(bytes.len()).ok_or_else(|| {
            acquisition_error(
                GitHubAcquisitionErrorCode::ArchiveBudgetExceeded,
                "Selected Skill size accounting overflowed.",
            )
        })?;
        if actual_total > MAX_SKILL_PACKAGE_BYTES {
            return Err(acquisition_error(
                GitHubAcquisitionErrorCode::ArchiveBudgetExceeded,
                "Selected Skill exceeds the package byte limit while reading.",
            ));
        }
        files.push((selected_entry.logical_path, bytes));
    }
    Ok(files)
}

fn validate_archive_entry(
    entry: &zip::read::ZipFile<'_>,
) -> Result<Vec<String>, GitHubAcquisitionError> {
    let raw_name = std::str::from_utf8(entry.name_raw()).map_err(|_| {
        acquisition_error(
            GitHubAcquisitionErrorCode::UnsafeArchiveEntry,
            "GitHub archive contains a non-UTF-8 entry name.",
        )
    })?;
    if raw_name != entry.name()
        || raw_name.is_empty()
        || raw_name.len() > MAX_GITHUB_ARCHIVE_PATH_BYTES
        || raw_name.starts_with('/')
        || raw_name.contains(['\\', '\0'])
    {
        return Err(unsafe_archive_path());
    }
    let directory = entry.is_dir();
    let path = if directory {
        raw_name.strip_suffix('/').ok_or_else(unsafe_archive_path)?
    } else {
        if raw_name.ends_with('/') {
            return Err(unsafe_archive_path());
        }
        raw_name
    };
    let components = path.split('/').map(str::to_string).collect::<Vec<_>>();
    if components.is_empty()
        || components.len() > MAX_GITHUB_ARCHIVE_PATH_COMPONENTS
        || components.iter().any(|component| {
            component.is_empty()
                || matches!(component.as_str(), "." | "..")
                || component.len() > MAX_GITHUB_ARCHIVE_PATH_COMPONENT_BYTES
                || component.contains(':')
                || component.chars().any(char::is_control)
        })
    {
        return Err(unsafe_archive_path());
    }
    if entry.encrypted() || !is_supported_compression(entry.compression()) {
        return Err(acquisition_error(
            GitHubAcquisitionErrorCode::UnsafeArchiveEntry,
            "GitHub archive contains an encrypted or unsupported entry.",
        ));
    }
    let mode_kind = entry.unix_mode().map(|mode| mode & 0o170000).unwrap_or(0);
    let valid_kind = if directory {
        matches!(mode_kind, 0 | 0o040000)
    } else {
        matches!(mode_kind, 0 | 0o100000)
    };
    if entry.is_symlink() || !valid_kind {
        return Err(acquisition_error(
            GitHubAcquisitionErrorCode::UnsafeArchiveEntry,
            "GitHub archive contains a symlink or special filesystem entry.",
        ));
    }
    Ok(components)
}

fn validate_portable_archive_tree(
    components: &[String],
    directory: bool,
    files: &mut BTreeMap<String, String>,
    directories: &mut BTreeMap<String, String>,
) -> Result<(), GitHubAcquisitionError> {
    let path = components.join("/");
    let path_key = path.to_lowercase();
    if directory {
        if files.contains_key(&path_key) {
            return Err(archive_tree_collision());
        }
        match directories.get(&path_key) {
            Some(existing) if existing != &path => return Err(archive_tree_collision()),
            Some(_) => {}
            None => {
                directories.insert(path_key, path.clone());
            }
        }
    } else {
        if directories.contains_key(&path_key) {
            return Err(archive_tree_collision());
        }
        match files.get(&path_key) {
            Some(existing) if existing != &path => return Err(archive_tree_collision()),
            Some(_) => {}
            None => {
                files.insert(path_key, path.clone());
            }
        }
    }

    let mut parent = String::new();
    for component in components.iter().take(components.len().saturating_sub(1)) {
        parent = if parent.is_empty() {
            component.clone()
        } else {
            format!("{parent}/{component}")
        };
        let parent_key = parent.to_lowercase();
        if files.contains_key(&parent_key) {
            return Err(archive_tree_collision());
        }
        match directories.get(&parent_key) {
            Some(existing) if existing != &parent => return Err(archive_tree_collision()),
            Some(_) => {}
            None => {
                directories.insert(parent_key, parent.clone());
            }
        }
    }
    Ok(())
}

fn archive_tree_collision() -> GitHubAcquisitionError {
    acquisition_error(
        GitHubAcquisitionErrorCode::UnsafeArchiveEntry,
        "GitHub archive contains colliding file and directory paths.",
    )
}

fn is_supported_compression(method: CompressionMethod) -> bool {
    matches!(
        method,
        CompressionMethod::Stored | CompressionMethod::Deflated
    )
}

fn validate_selected_package_path(
    components: &[String],
    directory: bool,
) -> Result<(), GitHubAcquisitionError> {
    let first = components.first().map(String::as_str);
    let supported_resource_root = matches!(first, Some("references" | "assets" | "scripts"));
    let accepted = if directory {
        supported_resource_root
    } else {
        (components.len() == 1 && first == Some(SKILL_FILE_NAME))
            || (components.len() >= 2 && supported_resource_root)
    };
    if !accepted {
        return Err(acquisition_error(
            GitHubAcquisitionErrorCode::UnsafeArchiveEntry,
            "The selected Skill directory contains an unsupported package entry.",
        ));
    }
    Ok(())
}

fn suspicious_expansion(uncompressed: u64, compressed: u64) -> bool {
    if uncompressed <= EXPANSION_RATIO_GRACE_BYTES {
        return false;
    }
    let permitted = compressed
        .saturating_mul(MAX_GITHUB_EXPANSION_RATIO)
        .saturating_add(EXPANSION_RATIO_GRACE_BYTES);
    uncompressed > permitted
}

fn is_valid_owner(value: &str) -> bool {
    (1..=39).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        && value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && value
            .as_bytes()
            .last()
            .is_some_and(u8::is_ascii_alphanumeric)
        && !value.contains("--")
}

fn is_valid_repository_name(value: &str) -> bool {
    (1..=100).contains(&value.len())
        && !matches!(value, "." | "..")
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn is_valid_named_reference(value: &str) -> bool {
    if value.is_empty()
        || value.len() > 1024
        || value == "@"
        || value.starts_with('/')
        || value.ends_with('/')
        || value.ends_with('.')
        || value.contains("..")
        || value.contains("@{")
        || value.contains("//")
        || value
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
        || value.contains(['~', '^', ':', '?', '*', '[', '\\'])
    {
        return false;
    }
    value.split('/').all(|component| {
        !component.is_empty() && !component.starts_with('.') && !component.ends_with(".lock")
    })
}

fn map_transport_error(error: GitHubTransportError) -> GitHubAcquisitionError {
    let (code, reason) = match error {
        GitHubTransportError::NotFound => (
            GitHubAcquisitionErrorCode::NotFound,
            "The public GitHub repository, reference, or archive was not found.",
        ),
        GitHubTransportError::RateLimited => (
            GitHubAcquisitionErrorCode::RateLimited,
            "GitHub temporarily rate-limited this public acquisition.",
        ),
        GitHubTransportError::Rejected => (
            GitHubAcquisitionErrorCode::TransportRejected,
            "GitHub rejected this public acquisition request.",
        ),
        GitHubTransportError::ResponseTooLarge => (
            GitHubAcquisitionErrorCode::ArchiveTooLarge,
            "GitHub returned a response larger than the acquisition limit.",
        ),
        GitHubTransportError::InvalidResponse => (
            GitHubAcquisitionErrorCode::Unavailable,
            "GitHub returned an invalid acquisition response.",
        ),
        GitHubTransportError::Unavailable => (
            GitHubAcquisitionErrorCode::Unavailable,
            "GitHub acquisition is temporarily unavailable.",
        ),
    };
    acquisition_error(code, reason)
}

fn map_package_error(error: SkillPackagePreparationError) -> GitHubAcquisitionError {
    GitHubAcquisitionError {
        code: GitHubAcquisitionErrorCode::PackageRejected,
        reason: format!("GitHub Skill package validation failed: {}", error.reason()),
        preparation: Some(Box::new(error)),
    }
}

fn map_acquisition_to_workflow_error(
    error: GitHubAcquisitionError,
) -> SkillAcquisitionAdapterError {
    let GitHubAcquisitionError {
        code,
        reason,
        preparation,
    } = error;
    if let Some(preparation) = preparation {
        return (*preparation).into();
    }
    match code {
        GitHubAcquisitionErrorCode::NotFound | GitHubAcquisitionErrorCode::SkillNotFound => {
            SkillAcquisitionAdapterError::not_found(reason)
        }
        GitHubAcquisitionErrorCode::RateLimited => {
            SkillAcquisitionAdapterError::rate_limited(reason)
        }
        GitHubAcquisitionErrorCode::ArchiveTooLarge
        | GitHubAcquisitionErrorCode::ArchiveBudgetExceeded => {
            SkillAcquisitionAdapterError::too_large(reason)
        }
        GitHubAcquisitionErrorCode::InvalidArchive
        | GitHubAcquisitionErrorCode::UnsafeArchiveEntry => {
            SkillAcquisitionAdapterError::unsafe_package(reason)
        }
        GitHubAcquisitionErrorCode::TransportRejected
        | GitHubAcquisitionErrorCode::Unavailable
        | GitHubAcquisitionErrorCode::InvalidOrigin => {
            SkillAcquisitionAdapterError::unavailable(reason)
        }
        _ => SkillAcquisitionAdapterError::invalid_request(reason),
    }
}

fn github_provider() -> SkillAcquisitionProvider {
    SkillAcquisitionProvider::parse(GITHUB_SKILL_ORIGIN_PROVIDER)
        .expect("the built-in github provider id must remain valid")
}

fn invalid_workflow_request() -> GitHubAcquisitionError {
    acquisition_error(
        GitHubAcquisitionErrorCode::InvalidWorkflowRequest,
        "Cannot encode the validated GitHub location for the acquisition workflow.",
    )
}

fn invalid_subdirectory() -> GitHubAcquisitionError {
    acquisition_error(
        GitHubAcquisitionErrorCode::InvalidSubdirectory,
        "GitHub Skill subdirectory must be a bounded, portable repository-relative path.",
    )
}

fn invalid_archive() -> GitHubAcquisitionError {
    acquisition_error(
        GitHubAcquisitionErrorCode::InvalidArchive,
        "GitHub returned an invalid ZIP archive.",
    )
}

fn unsafe_archive_path() -> GitHubAcquisitionError {
    acquisition_error(
        GitHubAcquisitionErrorCode::UnsafeArchiveEntry,
        "GitHub archive contains an unsafe or ambiguous entry path.",
    )
}

fn acquisition_error(
    code: GitHubAcquisitionErrorCode,
    reason: impl Into<String>,
) -> GitHubAcquisitionError {
    GitHubAcquisitionError {
        code,
        reason: reason.into(),
        preparation: None,
    }
}

#[cfg(test)]
mod tests {
    use super::super::installation_service::SkillInstallationService;
    use super::super::installation_workflow::{
        SkillAcquisitionAdapterErrorCode, SkillInstallationPreparationRequest,
        SkillInstallationWorkflow, SkillPreparationId,
    };
    use super::super::model::SkillInstallationId;
    use super::*;
    use std::io::Write;
    use std::sync::Mutex;
    use tempfile::tempdir;
    use zip::write::SimpleFileOptions;
    use zip::ZipWriter;

    const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";
    const SKILL: &[u8] = b"---\nname: github-skill\ndescription: Acquired from GitHub.\n---\n\nFollow the GitHub workflow.\n";

    #[derive(Default)]
    struct FakeState {
        resolve_requests: Vec<GitHubResolveRequest>,
        archive_requests: Vec<GitHubArchiveRequest>,
    }

    struct FakeTransport {
        commit: GitHubCommit,
        archive: Vec<u8>,
        state: Mutex<FakeState>,
    }

    impl FakeTransport {
        fn new(archive: Vec<u8>) -> Self {
            Self {
                commit: GitHubCommit::parse(COMMIT).unwrap(),
                archive,
                state: Mutex::new(FakeState::default()),
            }
        }
    }

    impl GitHubAcquisitionTransport for FakeTransport {
        fn resolve_commit(
            &self,
            request: &GitHubResolveRequest,
        ) -> Result<GitHubCommit, GitHubTransportError> {
            self.state
                .lock()
                .unwrap()
                .resolve_requests
                .push(request.clone());
            Ok(self.commit.clone())
        }

        fn download_archive(
            &self,
            request: &GitHubArchiveRequest,
        ) -> Result<Vec<u8>, GitHubTransportError> {
            self.state
                .lock()
                .unwrap()
                .archive_requests
                .push(request.clone());
            Ok(self.archive.clone())
        }
    }

    #[test]
    fn acquisition_pins_commit_extracts_only_selected_package_and_sanitizes_origin() {
        let archive = write_zip(
            &[
                ("repo-root/README.md", b"outside"),
                ("repo-root/skills/a/SKILL.md", SKILL),
                ("repo-root/skills/a/references/guide.md", b"guide"),
                ("repo-root/skills/b/SKILL.md", SKILL),
            ],
            CompressionMethod::Stored,
        );
        let transport = Arc::new(FakeTransport::new(archive));
        let acquirer = GitHubSkillAcquirer::with_transport(transport.clone());
        let location = GitHubSkillLocation::new(
            GitHubRepository::parse("openai", "skills").unwrap(),
            GitHubReference::named("release/v1").unwrap(),
            GitHubSubdirectory::parse("skills/a").unwrap(),
        );

        let acquired = acquirer.acquire(location).unwrap();

        assert_eq!(acquired.package().name(), "github-skill");
        assert_eq!(acquired.package().resource_index().len(), 1);
        assert_eq!(acquired.summary().owner(), "openai");
        assert_eq!(acquired.summary().repository(), "skills");
        assert_eq!(acquired.summary().requested_reference(), Some("release/v1"));
        assert_eq!(acquired.summary().resolved_commit().as_str(), COMMIT);
        assert_eq!(acquired.package().origin().provider(), "github");
        let origin: serde_json::Value =
            serde_json::from_str(acquired.package().origin().reference()).unwrap();
        assert_eq!(origin["owner"], "openai");
        assert_eq!(origin["repository"], "skills");
        assert_eq!(origin["resolvedCommit"], COMMIT);
        assert_eq!(origin["subdirectory"], "skills/a");
        let state = transport.state.lock().unwrap();
        assert_eq!(state.resolve_requests.len(), 1);
        assert_eq!(state.archive_requests.len(), 1);
        assert_eq!(state.archive_requests[0].commit().as_str(), COMMIT);
    }

    #[test]
    fn workflow_adapter_owns_strict_protocol_wire_and_inspects_a_fake_transport_package() {
        let archive = write_zip(
            &[("repo-root/skills/a/SKILL.md", SKILL)],
            CompressionMethod::Stored,
        );
        let transport = Arc::new(FakeTransport::new(archive));
        let acquirer = Arc::new(GitHubSkillAcquirer::with_transport(transport));
        let adapter = Arc::new(GitHubWorkflowAcquisitionAdapter::new(acquirer));
        let location = GitHubSkillLocation::new(
            GitHubRepository::parse("openai", "skills").unwrap(),
            GitHubReference::named("main").unwrap(),
            GitHubSubdirectory::parse("skills/a").unwrap(),
        );
        let source = GitHubWorkflowAcquisitionAdapter::source_for(&location).unwrap();
        let wire: serde_json::Value =
            serde_json::from_slice(source.adapter_request().unwrap()).unwrap();
        assert_eq!(wire["kind"], "githubRepository");
        assert_eq!(wire["owner"], "openai");
        assert_eq!(wire["repository"], "skills");
        assert_eq!(wire["reference"]["kind"], "named");
        assert_eq!(wire["reference"]["value"], "main");
        assert_eq!(wire["subdirectory"], "skills/a");

        let fixture = tempdir().unwrap();
        let installation_service =
            SkillInstallationService::new(fixture.path().join("store")).unwrap();
        let mut workflow = SkillInstallationWorkflow::new(installation_service);
        workflow.register_adapter(adapter).unwrap();
        let request = SkillInstallationPreparationRequest::install(
            SkillPreparationId::new(),
            SkillInstallationId::parse("01234567-89ab-4def-8123-456789abcdef").unwrap(),
            source,
        );

        let preview = workflow.inspect(&request).unwrap();
        assert_eq!(preview.package().name(), "github-skill");
        assert_eq!(preview.package().format_version(), 1);
    }

    #[test]
    fn workflow_adapter_rejects_unknown_wire_fields_before_transport() {
        let transport = Arc::new(FakeTransport::new(Vec::new()));
        let adapter = GitHubWorkflowAcquisitionAdapter::new(Arc::new(
            GitHubSkillAcquirer::with_transport(transport.clone()),
        ));
        let source = SkillAcquisitionSource::adapter(
            github_provider(),
            br#"{
                "kind":"githubRepository",
                "owner":"openai",
                "repository":"skills",
                "unexpected":"must-fail"
            }"#
            .to_vec(),
        )
        .unwrap();

        let error = adapter.acquire(&source).unwrap_err();
        assert_eq!(
            error.code(),
            SkillAcquisitionAdapterErrorCode::InvalidRequest
        );
        assert!(transport.state.lock().unwrap().resolve_requests.is_empty());
    }

    #[test]
    fn fixed_endpoint_builders_percent_encode_refs_and_pin_codeload_to_commit() {
        let repository = GitHubRepository::parse("owner", "repo").unwrap();
        let api = github_api_commit_url(&repository, "feature/a").unwrap();
        assert_eq!(
            api.as_str(),
            "https://api.github.com/repos/owner/repo/commits/feature%2Fa"
        );
        let commit = GitHubCommit::parse(COMMIT).unwrap();
        let codeload = github_codeload_url(&repository, &commit).unwrap();
        assert_eq!(
            codeload.as_str(),
            format!("https://codeload.github.com/owner/repo/zip/{COMMIT}")
        );
    }

    #[test]
    fn structured_coordinates_reject_urls_partial_commits_and_unsafe_subdirectories() {
        assert!(GitHubRepository::parse("https://github.com/openai", "skills").is_err());
        assert!(GitHubRepository::parse("openai", "skills.git/other").is_err());
        assert!(GitHubReference::commit("01234567").is_err());
        assert!(GitHubReference::named("refs/../main").is_err());
        assert!(GitHubSubdirectory::parse("../skills/a").is_err());
        assert!(GitHubSubdirectory::parse("skills\\a").is_err());
        assert!(GitHubSubdirectory::parse("/skills/a").is_err());
    }

    #[test]
    fn archive_security_corpus_rejects_absolute_parent_backslash_and_multiple_roots() {
        for unsafe_name in [
            "/repo-root/SKILL.md",
            "repo-root/../SKILL.md",
            "repo-root\\SKILL.md",
        ] {
            let archive = write_zip(&[(unsafe_name, SKILL)], CompressionMethod::Stored);
            assert_eq!(
                extract_selected_skill(&archive, &GitHubSubdirectory::root())
                    .unwrap_err()
                    .code(),
                GitHubAcquisitionErrorCode::UnsafeArchiveEntry,
                "unsafe name: {unsafe_name}"
            );
        }

        let multiple_roots = write_zip(
            &[("root-a/SKILL.md", SKILL), ("root-b/README.md", b"other")],
            CompressionMethod::Stored,
        );
        assert_eq!(
            extract_selected_skill(&multiple_roots, &GitHubSubdirectory::root())
                .unwrap_err()
                .code(),
            GitHubAcquisitionErrorCode::UnsafeArchiveEntry
        );
    }

    #[test]
    fn archive_security_corpus_rejects_case_collisions_symlinks_and_special_modes() {
        let collision = write_zip(
            &[("root/SKILL.md", SKILL), ("root/skill.md", b"collision")],
            CompressionMethod::Stored,
        );
        assert_eq!(
            extract_selected_skill(&collision, &GitHubSubdirectory::root())
                .unwrap_err()
                .code(),
            GitHubAcquisitionErrorCode::UnsafeArchiveEntry
        );

        let symlink = write_symlink_zip();
        assert_eq!(
            extract_selected_skill(&symlink, &GitHubSubdirectory::root())
                .unwrap_err()
                .code(),
            GitHubAcquisitionErrorCode::UnsafeArchiveEntry
        );

        let mut special = write_zip(
            &[
                ("root/SKILL.md", SKILL),
                ("root/assets/pipe", b"not-a-regular-file"),
            ],
            CompressionMethod::Stored,
        );
        set_central_unix_mode(&mut special, "root/assets/pipe", 0o010644);
        assert_eq!(
            extract_selected_skill(&special, &GitHubSubdirectory::root())
                .unwrap_err()
                .code(),
            GitHubAcquisitionErrorCode::UnsafeArchiveEntry
        );
    }

    #[test]
    fn archive_security_corpus_rejects_invalid_utf8_and_zip_bombs_before_extraction() {
        let mut invalid_utf8 = write_zip(
            &[("root/SKILL.md", SKILL), ("root/assets/bad.bin", b"bad")],
            CompressionMethod::Stored,
        );
        replace_all(&mut invalid_utf8, b"bad.bin", b"\xffad.bin");
        assert_eq!(
            extract_selected_skill(&invalid_utf8, &GitHubSubdirectory::root())
                .unwrap_err()
                .code(),
            GitHubAcquisitionErrorCode::UnsafeArchiveEntry
        );

        let bomb = vec![0_u8; 4 * 1024 * 1024];
        let archive = write_zip(
            &[("root/SKILL.md", SKILL), ("root/assets/bomb.bin", &bomb)],
            CompressionMethod::Deflated,
        );
        assert_eq!(
            extract_selected_skill(&archive, &GitHubSubdirectory::root())
                .unwrap_err()
                .code(),
            GitHubAcquisitionErrorCode::ArchiveBudgetExceeded
        );
    }

    #[test]
    fn selected_directory_rejects_unexpected_sibling_files() {
        let archive = write_zip(
            &[
                ("root/SKILL.md", SKILL),
                ("root/README.md", b"not a package resource"),
            ],
            CompressionMethod::Stored,
        );
        assert_eq!(
            extract_selected_skill(&archive, &GitHubSubdirectory::root())
                .unwrap_err()
                .code(),
            GitHubAcquisitionErrorCode::UnsafeArchiveEntry
        );
    }

    fn write_zip(entries: &[(&str, &[u8])], compression: CompressionMethod) -> Vec<u8> {
        let cursor = Cursor::new(Vec::new());
        let mut writer = ZipWriter::new(cursor);
        let options = SimpleFileOptions::default()
            .compression_method(compression)
            .unix_permissions(0o644);
        for (name, bytes) in entries {
            writer.start_file(*name, options).unwrap();
            writer.write_all(bytes).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    fn write_symlink_zip() -> Vec<u8> {
        let cursor = Cursor::new(Vec::new());
        let mut writer = ZipWriter::new(cursor);
        let options = SimpleFileOptions::default().unix_permissions(0o644);
        writer.start_file("root/SKILL.md", options).unwrap();
        writer.write_all(SKILL).unwrap();
        writer
            .add_symlink("root/assets/link", "../../outside", options)
            .unwrap();
        writer.finish().unwrap().into_inner()
    }

    fn replace_all(bytes: &mut [u8], needle: &[u8], replacement: &[u8]) {
        assert_eq!(needle.len(), replacement.len());
        let mut offset = 0;
        let mut replacements = 0;
        while let Some(relative) = bytes[offset..]
            .windows(needle.len())
            .position(|window| window == needle)
        {
            let start = offset + relative;
            bytes[start..start + needle.len()].copy_from_slice(replacement);
            offset = start + needle.len();
            replacements += 1;
        }
        assert!(
            replacements >= 2,
            "local and central names must be replaced"
        );
    }

    fn set_central_unix_mode(bytes: &mut [u8], entry_name: &str, mode: u32) {
        const CENTRAL_SIGNATURE: &[u8] = b"PK\x01\x02";
        let mut offset = 0;
        while let Some(relative) = bytes[offset..]
            .windows(CENTRAL_SIGNATURE.len())
            .position(|window| window == CENTRAL_SIGNATURE)
        {
            let start = offset + relative;
            let name_length = u16::from_le_bytes([bytes[start + 28], bytes[start + 29]]) as usize;
            let name_start = start + 46;
            let name_end = name_start + name_length;
            if bytes.get(name_start..name_end) == Some(entry_name.as_bytes()) {
                bytes[start + 5] = 3;
                bytes[start + 38..start + 42].copy_from_slice(&(mode << 16).to_le_bytes());
                return;
            }
            offset = name_end;
        }
        panic!("central entry not found");
    }
}
