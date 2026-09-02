//! Public GitHub acquisition for immutable Skill package snapshots.
//!
//! This adapter intentionally accepts structured GitHub coordinates instead
//! of arbitrary URLs. Immutable SHAs bypass ref lookup; moving refs use a
//! bounded `git ls-remote` lookup with GitHub's public API only as fallback.
//! The resulting full SHA is downloaded from codeload into a private bounded
//! spool. Only a fully validated [`PreparedSkillPackage`] crosses the
//! installation boundary.

use super::acquisition_provenance::{
    SkillInstallationAuthority, SkillInstallationProvenance, SkillInstallationProvenanceView,
    SkillInstallationRefresh, SkillInstallationRefreshView,
};
use super::installation_workflow::{
    InstalledGitHubTrackingReference, InstalledSkillSourcePresentation, SkillAcquisitionAdapter,
    SkillAcquisitionAdapterError, SkillAcquisitionProvider, SkillAcquisitionSource,
};
use super::origin::SkillPackageOrigin;
use super::package::{
    MAX_SKILL_PACKAGE_BYTES, MAX_SKILL_PACKAGE_FILES, MAX_SKILL_RESOURCE_FILE_BYTES,
};
use super::prepared::{PreparedSkillPackage, SkillPackagePreparationError};
use super::prepared_acquisition::PreparedSkillAcquisition;
use super::workspace::{MAX_SKILL_FILE_BYTES, SKILL_FILE_NAME};
use reqwest::blocking::{Client, Response};
use reqwest::header::{HeaderMap, ACCEPT, CONTENT_LENGTH, RETRY_AFTER};
use reqwest::{StatusCode, Url};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::fs::File;
#[cfg(test)]
use std::io::Cursor;
use std::io::{Read, Seek, SeekFrom, Write};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tempfile::NamedTempFile;
use zip::{CompressionMethod, ZipArchive};

pub const GITHUB_SKILL_ORIGIN_PROVIDER: &str = "github";
const GITHUB_PROVENANCE_SCHEMA_VERSION: u32 = 1;

const GITHUB_USER_AGENT: &str = "CaptainWho-Skill-Acquisition/1";
const GITHUB_API_VERSION: &str = "2022-11-28";
const GITHUB_API_BODY_BYTES: usize = 256 * 1024;
const GITHUB_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const GITHUB_REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
const GITHUB_LS_REMOTE_TIMEOUT: Duration = Duration::from_secs(20);
const GITHUB_RATE_LIMIT_FALLBACK: Duration = Duration::from_secs(60);
const GITHUB_HTTP_ATTEMPTS: usize = 2;
const GITHUB_RETRY_BACKOFF: Duration = Duration::from_millis(150);
pub(super) const MAX_GITHUB_ZIP_BYTES: usize = 32 * 1024 * 1024;
pub(super) const MAX_GITHUB_ARCHIVE_ENTRIES: usize = 20_000;
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

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GitHubResolveRequest {
    repository: GitHubRepository,
    reference: GitHubReference,
}

impl GitHubResolveRequest {
    /// Creates a transport request from validated repository and reference values.
    pub fn new(repository: GitHubRepository, reference: GitHubReference) -> Self {
        Self {
            repository,
            reference,
        }
    }

    pub fn repository(&self) -> &GitHubRepository {
        &self.repository
    }

    pub fn reference(&self) -> &GitHubReference {
        &self.reference
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GitHubArchiveRequest {
    repository: GitHubRepository,
    commit: GitHubCommit,
}

impl GitHubArchiveRequest {
    /// Creates a transport request pinned to a validated immutable commit.
    pub fn new(repository: GitHubRepository, commit: GitHubCommit) -> Self {
        Self { repository, commit }
    }

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
    ) -> Result<GitHubArchive, GitHubTransportError>;
}

/// An immutable private spool containing one bounded GitHub repository ZIP.
///
/// The temporary path never crosses the Core boundary. Clones share the same
/// read-only file and deletion happens automatically after the final owner is
/// dropped. Cache hits verify the captured length and digest before reuse; ZIP
/// readers only reopen this private immutable spool, avoiding one full hash
/// pass for every candidate discovered in a multi-Skill repository.
#[derive(Clone)]
pub struct GitHubArchive {
    inner: Arc<GitHubArchiveInner>,
}

struct GitHubArchiveInner {
    file: NamedTempFile,
    len: usize,
    digest: [u8; 32],
}

impl fmt::Debug for GitHubArchive {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GitHubArchive")
            .field("bytes", &self.inner.len)
            .finish_non_exhaustive()
    }
}

impl GitHubArchive {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, GitHubTransportError> {
        if bytes.len() > MAX_GITHUB_ZIP_BYTES {
            return Err(GitHubTransportError::ResponseTooLarge);
        }
        let mut file = NamedTempFile::new().map_err(|_| GitHubTransportError::Unavailable)?;
        file.write_all(bytes)
            .and_then(|_| file.as_file_mut().flush())
            .map_err(|_| GitHubTransportError::Unavailable)?;
        Ok(Self::from_spool(
            file,
            bytes.len(),
            Sha256::digest(bytes).into(),
        ))
    }

    fn from_spool(file: NamedTempFile, len: usize, digest: [u8; 32]) -> Self {
        Self {
            inner: Arc::new(GitHubArchiveInner { file, len, digest }),
        }
    }

    pub fn len(&self) -> usize {
        self.inner.len
    }

    pub fn is_empty(&self) -> bool {
        self.inner.len == 0
    }

    pub(crate) fn reader(&self) -> Result<File, GitHubTransportError> {
        let mut reader = self
            .inner
            .file
            .reopen()
            .map_err(|_| GitHubTransportError::InvalidResponse)?;
        let metadata = reader
            .metadata()
            .map_err(|_| GitHubTransportError::InvalidResponse)?;
        if metadata.len() != self.inner.len as u64 {
            return Err(GitHubTransportError::InvalidResponse);
        }
        reader
            .seek(SeekFrom::Start(0))
            .map_err(|_| GitHubTransportError::InvalidResponse)?;
        Ok(reader)
    }

    pub(crate) fn verify_integrity(&self) -> Result<(), GitHubTransportError> {
        let mut reader = self.reader()?;
        let mut hasher = Sha256::new();
        let mut buffer = [0_u8; 64 * 1024];
        let mut actual = 0usize;
        loop {
            let read = reader
                .read(&mut buffer)
                .map_err(|_| GitHubTransportError::InvalidResponse)?;
            if read == 0 {
                break;
            }
            actual = actual
                .checked_add(read)
                .ok_or(GitHubTransportError::InvalidResponse)?;
            hasher.update(&buffer[..read]);
        }
        let digest: [u8; 32] = hasher.finalize().into();
        if actual != self.inner.len || digest != self.inner.digest {
            return Err(GitHubTransportError::InvalidResponse);
        }
        Ok(())
    }
}

/// Network transport restricted to public GitHub HTTPS endpoints and a fixed
/// non-interactive `git ls-remote` invocation. Redirects, credentials and
/// model-controlled commands are deliberately unsupported.
pub struct ReqwestGitHubTransport {
    client: Client,
    api_rate_limit_until: Mutex<Option<Instant>>,
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
            .connect_timeout(GITHUB_CONNECT_TIMEOUT)
            .timeout(GITHUB_REQUEST_TIMEOUT)
            .user_agent(GITHUB_USER_AGENT)
            .build()
            .map_err(|_| GitHubTransportError::Unavailable)?;
        Ok(Self {
            client,
            api_rate_limit_until: Mutex::new(None),
        })
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
        self.check_api_rate_limit()?;
        for attempt in 0..GITHUB_HTTP_ATTEMPTS {
            let result = self
                .client
                .get(url.clone())
                .header(ACCEPT, "application/vnd.github+json")
                .header("X-GitHub-Api-Version", GITHUB_API_VERSION)
                .send()
                .map_err(map_reqwest_error)
                .and_then(|response| read_success_response(response, GITHUB_API_BODY_BYTES, true));
            if let Err(GitHubTransportError::RateLimited { retry_after, .. }) = &result {
                self.remember_api_rate_limit(*retry_after);
            }
            if attempt + 1 < GITHUB_HTTP_ATTEMPTS
                && result.as_ref().is_err_and(retryable_transport_error)
            {
                thread::sleep(GITHUB_RETRY_BACKOFF);
                continue;
            }
            return result;
        }
        Err(GitHubTransportError::Unavailable)
    }

    fn check_api_rate_limit(&self) -> Result<(), GitHubTransportError> {
        let mut state = self
            .api_rate_limit_until
            .lock()
            .map_err(|_| GitHubTransportError::Unavailable)?;
        let Some(until) = *state else {
            return Ok(());
        };
        let now = Instant::now();
        if until <= now {
            *state = None;
            return Ok(());
        }
        Err(GitHubTransportError::RateLimited {
            retry_after: Some(until.duration_since(now)),
            secondary: false,
        })
    }

    fn remember_api_rate_limit(&self, retry_after: Option<Duration>) {
        let duration = retry_after.unwrap_or(GITHUB_RATE_LIMIT_FALLBACK);
        if let Ok(mut state) = self.api_rate_limit_until.lock() {
            *state = Instant::now().checked_add(duration);
        }
    }

    fn resolve_with_fallback(
        &self,
        request: &GitHubResolveRequest,
    ) -> Result<GitHubCommit, GitHubTransportError> {
        if let GitHubReference::Commit(commit) = request.reference() {
            return Ok(commit.clone());
        }
        match resolve_with_git_ls_remote(request) {
            Ok(commit) => Ok(commit),
            Err(_) => self.resolve_with_api(request),
        }
    }

    fn resolve_with_api(
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
            GitHubReference::Commit(commit) => Ok(commit.clone()),
        }
    }
}

impl GitHubAcquisitionTransport for ReqwestGitHubTransport {
    fn resolve_commit(
        &self,
        request: &GitHubResolveRequest,
    ) -> Result<GitHubCommit, GitHubTransportError> {
        self.resolve_with_fallback(request)
    }

    fn download_archive(
        &self,
        request: &GitHubArchiveRequest,
    ) -> Result<GitHubArchive, GitHubTransportError> {
        let url = github_codeload_url(request.repository(), request.commit())?;
        for attempt in 0..GITHUB_HTTP_ATTEMPTS {
            let result = self
                .client
                .get(url.clone())
                .timeout(GITHUB_REQUEST_TIMEOUT)
                .header(ACCEPT, "application/zip")
                .send()
                .map_err(map_reqwest_error)
                .and_then(spool_archive_response);
            if attempt + 1 < GITHUB_HTTP_ATTEMPTS
                && result.as_ref().is_err_and(retryable_transport_error)
            {
                thread::sleep(GITHUB_RETRY_BACKOFF);
                continue;
            }
            return result;
        }
        Err(GitHubTransportError::Unavailable)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum GitHubTransportError {
    NotFound,
    RateLimited {
        retry_after: Option<Duration>,
        secondary: bool,
    },
    Rejected,
    ResponseTooLarge,
    InvalidResponse,
    Timeout,
    NetworkUnavailable,
    Unavailable,
}

impl GitHubTransportError {
    pub fn retry_after(&self) -> Option<Duration> {
        match self {
            Self::RateLimited { retry_after, .. } => *retry_after,
            _ => None,
        }
    }
}

impl fmt::Display for GitHubTransportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let reason = match self {
            Self::NotFound => "The public GitHub repository or reference was not found.",
            Self::RateLimited { .. } => "GitHub temporarily rate-limited this public request.",
            Self::Rejected => "GitHub rejected this public request.",
            Self::ResponseTooLarge => {
                "GitHub returned a response larger than the acquisition limit."
            }
            Self::InvalidResponse => "GitHub returned an invalid acquisition response.",
            Self::Timeout => "The GitHub request timed out.",
            Self::NetworkUnavailable => "GitHub could not be reached over the network.",
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
    pub fn with_transport(transport: Arc<dyn GitHubAcquisitionTransport>) -> Self {
        Self { transport }
    }

    pub fn acquire(
        &self,
        location: GitHubSkillLocation,
    ) -> Result<AcquiredGitHubSkill, GitHubAcquisitionError> {
        let commit = match &location.reference {
            GitHubReference::Commit(commit) => commit.clone(),
            reference => self
                .transport
                .resolve_commit(&GitHubResolveRequest::new(
                    location.repository.clone(),
                    reference.clone(),
                ))
                .map_err(map_transport_error)?,
        };
        let archive_request =
            GitHubArchiveRequest::new(location.repository.clone(), commit.clone());
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
        let files = extract_selected_skill_archive(&archive, location.subdirectory())?;
        let summary = GitHubAcquisitionSummary::from_location(&location, commit.clone());
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
    ) -> Result<PreparedSkillAcquisition, SkillAcquisitionAdapterError> {
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
            .and_then(AcquiredGitHubSkill::into_acquisition)
            .map_err(map_acquisition_to_workflow_error)
    }

    fn refresh_schema_versions(&self) -> &'static [u32] {
        &[GITHUB_PROVENANCE_SCHEMA_VERSION]
    }

    fn reacquire(
        &self,
        refresh: SkillInstallationRefreshView<'_>,
    ) -> Result<PreparedSkillAcquisition, SkillAcquisitionAdapterError> {
        if refresh.provider() != GITHUB_SKILL_ORIGIN_PROVIDER
            || refresh.schema_version() != GITHUB_PROVENANCE_SCHEMA_VERSION
        {
            return Err(SkillAcquisitionAdapterError::invalid_request(
                "GitHub refresh metadata has an unsupported provider or schema",
            ));
        }
        let request: GitHubRefreshPayload<'_> =
            serde_json::from_str(refresh.payload()).map_err(|_| {
                SkillAcquisitionAdapterError::invalid_request(
                    "GitHub refresh metadata does not match the strict tracking schema",
                )
            })?;
        let location = request
            .into_location()
            .map_err(map_acquisition_to_workflow_error)?;
        self.acquirer
            .acquire(location)
            .and_then(AcquiredGitHubSkill::into_acquisition)
            .map_err(map_acquisition_to_workflow_error)
    }

    fn installed_source_presentation(
        &self,
        provenance: SkillInstallationProvenanceView<'_>,
        refresh_capable: bool,
    ) -> Option<InstalledSkillSourcePresentation> {
        github_source_presentation(provenance, refresh_capable)
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

    pub(crate) fn into_acquisition(
        self,
    ) -> Result<PreparedSkillAcquisition, GitHubAcquisitionError> {
        let provenance = self.summary.provenance()?;
        Ok(PreparedSkillAcquisition::new(self.package, provenance))
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
    fn from_location(location: &GitHubSkillLocation, resolved_commit: GitHubCommit) -> Self {
        Self {
            owner: location.repository.owner.clone(),
            repository: location.repository.name.clone(),
            requested_reference_kind: location.reference.stable_kind(),
            requested_reference: location.reference.value().map(str::to_string),
            resolved_commit,
            subdirectory: location.subdirectory.as_str().to_string(),
        }
    }

    /// Builds the canonical provenance for a candidate that source resolution
    /// has already pinned to an immutable commit.
    pub(super) fn for_resolved_pin(
        repository: &GitHubRepository,
        tracking_reference: &GitHubReference,
        resolved_commit: &GitHubCommit,
        subdirectory: &GitHubSubdirectory,
    ) -> Self {
        Self {
            owner: repository.owner.clone(),
            repository: repository.name.clone(),
            requested_reference_kind: tracking_reference.stable_kind(),
            requested_reference: tracking_reference.value().map(str::to_string),
            resolved_commit: resolved_commit.clone(),
            subdirectory: subdirectory.as_str().to_string(),
        }
    }

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

    pub(super) fn provenance(&self) -> Result<SkillInstallationProvenance, GitHubAcquisitionError> {
        let authority_payload = serde_json::to_string(&GitHubAuthorityPayload {
            owner: &self.owner,
            repository: &self.repository,
            resolved_commit: self.resolved_commit.as_str(),
            subdirectory: (!self.subdirectory.is_empty()).then_some(self.subdirectory.as_str()),
        })
        .map_err(|_| invalid_origin())?;
        let authority = SkillInstallationAuthority::new(
            GITHUB_SKILL_ORIGIN_PROVIDER,
            GITHUB_PROVENANCE_SCHEMA_VERSION,
            authority_payload,
        )
        .map_err(|_| invalid_origin())?;
        let tracking_reference = match (
            self.requested_reference_kind,
            self.requested_reference.as_deref(),
        ) {
            ("defaultBranch", None) => Some(GitHubTrackingReference::DefaultBranch),
            ("named", Some(value)) => Some(GitHubTrackingReference::Named { value }),
            ("commit", Some(_)) => None,
            _ => return Err(invalid_origin()),
        };
        let refresh = tracking_reference
            .map(|reference| {
                serde_json::to_string(&GitHubRefreshPayload {
                    owner: &self.owner,
                    repository: &self.repository,
                    reference,
                    subdirectory: (!self.subdirectory.is_empty())
                        .then_some(self.subdirectory.as_str()),
                })
                .map_err(|_| invalid_origin())
                .and_then(|payload| {
                    SkillInstallationRefresh::new(
                        GITHUB_SKILL_ORIGIN_PROVIDER,
                        GITHUB_PROVENANCE_SCHEMA_VERSION,
                        payload,
                    )
                    .map_err(|_| invalid_origin())
                })
            })
            .transpose()?;
        Ok(SkillInstallationProvenance::new(authority, refresh))
    }

    pub(super) fn origin(&self) -> Result<SkillPackageOrigin, GitHubAcquisitionError> {
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

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GitHubAuthorityPayload<'a> {
    #[serde(borrow)]
    owner: &'a str,
    #[serde(borrow)]
    repository: &'a str,
    #[serde(borrow)]
    resolved_commit: &'a str,
    #[serde(default, borrow, skip_serializing_if = "Option::is_none")]
    subdirectory: Option<&'a str>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GitHubRefreshPayload<'a> {
    #[serde(borrow)]
    owner: &'a str,
    #[serde(borrow)]
    repository: &'a str,
    #[serde(borrow)]
    reference: GitHubTrackingReference<'a>,
    #[serde(default, borrow)]
    subdirectory: Option<&'a str>,
}

impl GitHubRefreshPayload<'_> {
    fn into_location(self) -> Result<GitHubSkillLocation, GitHubAcquisitionError> {
        let reference = match self.reference {
            GitHubTrackingReference::DefaultBranch => GitHubReference::DefaultBranch,
            GitHubTrackingReference::Named { value } => GitHubReference::named(value)?,
        };
        Ok(GitHubSkillLocation::new(
            GitHubRepository::parse(self.owner, self.repository)?,
            reference,
            GitHubSubdirectory::parse(self.subdirectory.unwrap_or_default())?,
        ))
    }
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
enum GitHubTrackingReference<'a> {
    DefaultBranch,
    Named {
        #[serde(borrow)]
        value: &'a str,
    },
}

fn github_source_presentation(
    provenance: SkillInstallationProvenanceView<'_>,
    refresh_capable: bool,
) -> Option<InstalledSkillSourcePresentation> {
    let authority = provenance.authority();
    if authority.provider() != GITHUB_SKILL_ORIGIN_PROVIDER
        || authority.schema_version() != GITHUB_PROVENANCE_SCHEMA_VERSION
    {
        return None;
    }
    let authority: GitHubAuthorityPayload<'_> = serde_json::from_str(authority.payload()).ok()?;
    let repository = GitHubRepository::parse(authority.owner, authority.repository).ok()?;
    let commit = GitHubCommit::parse(authority.resolved_commit).ok()?;
    let subdirectory_value = authority.subdirectory.unwrap_or_default();
    if authority.subdirectory == Some("") {
        return None;
    }
    let subdirectory = GitHubSubdirectory::parse(subdirectory_value).ok()?;

    let (tracking_reference, refreshable) = match provenance.refresh() {
        None => (InstalledGitHubTrackingReference::Commit, false),
        Some(refresh)
            if refresh.provider() == GITHUB_SKILL_ORIGIN_PROVIDER
                && refresh.schema_version() == GITHUB_PROVENANCE_SCHEMA_VERSION =>
        {
            let refresh_payload: GitHubRefreshPayload<'_> =
                serde_json::from_str(refresh.payload()).ok()?;
            if refresh_payload.owner != repository.owner()
                || refresh_payload.repository != repository.name()
                || refresh_payload.subdirectory.unwrap_or_default() != subdirectory.as_str()
                || refresh_payload.subdirectory == Some("")
            {
                return None;
            }
            let tracking = match refresh_payload.reference {
                GitHubTrackingReference::DefaultBranch => {
                    InstalledGitHubTrackingReference::DefaultBranch
                }
                GitHubTrackingReference::Named { value } => {
                    let reference = GitHubNamedReference::parse(value).ok()?;
                    InstalledGitHubTrackingReference::Named(reference.as_str().to_string())
                }
            };
            (tracking, refresh_capable)
        }
        Some(_) => return None,
    };

    Some(InstalledSkillSourcePresentation::GitHub {
        owner: repository.owner().to_string(),
        repository: repository.name().to_string(),
        tracking_reference,
        resolved_commit: commit.as_str().to_string(),
        subdirectory: (!subdirectory.as_str().is_empty())
            .then(|| subdirectory.as_str().to_string()),
        refreshable,
    })
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
    if !response.status().is_success() {
        return Err(map_response_status(&response));
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
        .map_err(map_io_transport_error)?;
    if body.len() > max_bytes {
        return Err(GitHubTransportError::ResponseTooLarge);
    }
    Ok(body)
}

fn spool_archive_response(mut response: Response) -> Result<GitHubArchive, GitHubTransportError> {
    if !response.status().is_success() {
        return Err(map_response_status(&response));
    }
    let expected_length = response
        .headers()
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok());
    if expected_length.is_some_and(|length| length > MAX_GITHUB_ZIP_BYTES as u64) {
        return Err(GitHubTransportError::ResponseTooLarge);
    }

    spool_archive_reader(&mut response, expected_length)
}

fn spool_archive_reader(
    reader: &mut dyn Read,
    expected_length: Option<u64>,
) -> Result<GitHubArchive, GitHubTransportError> {
    if expected_length.is_some_and(|length| length > MAX_GITHUB_ZIP_BYTES as u64) {
        return Err(GitHubTransportError::ResponseTooLarge);
    }
    let mut file = NamedTempFile::new().map_err(|_| GitHubTransportError::Unavailable)?;
    let mut hasher = Sha256::new();
    let mut total = 0usize;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = reader.read(&mut buffer).map_err(map_io_transport_error)?;
        if read == 0 {
            break;
        }
        total = total
            .checked_add(read)
            .ok_or(GitHubTransportError::ResponseTooLarge)?;
        if total > MAX_GITHUB_ZIP_BYTES {
            return Err(GitHubTransportError::ResponseTooLarge);
        }
        file.write_all(&buffer[..read])
            .map_err(|_| GitHubTransportError::Unavailable)?;
        hasher.update(&buffer[..read]);
    }
    if expected_length.is_some_and(|length| length != total as u64) {
        return Err(GitHubTransportError::InvalidResponse);
    }
    file.as_file_mut()
        .flush()
        .map_err(|_| GitHubTransportError::Unavailable)?;
    let digest: [u8; 32] = hasher.finalize().into();
    Ok(GitHubArchive::from_spool(file, total, digest))
}

fn map_response_status(response: &Response) -> GitHubTransportError {
    map_status_and_headers(response.status(), response.headers())
}

fn map_status_and_headers(status: StatusCode, headers: &HeaderMap) -> GitHubTransportError {
    match status {
        StatusCode::NOT_FOUND => GitHubTransportError::NotFound,
        StatusCode::TOO_MANY_REQUESTS => GitHubTransportError::RateLimited {
            retry_after: retry_after_from_headers(headers).or(Some(GITHUB_RATE_LIMIT_FALLBACK)),
            secondary: headers.contains_key(RETRY_AFTER),
        },
        StatusCode::FORBIDDEN if is_rate_limit_response(headers) => {
            GitHubTransportError::RateLimited {
                retry_after: retry_after_from_headers(headers).or(Some(GITHUB_RATE_LIMIT_FALLBACK)),
                secondary: headers.contains_key(RETRY_AFTER),
            }
        }
        status if status.is_server_error() => GitHubTransportError::Unavailable,
        _ => GitHubTransportError::Rejected,
    }
}

fn is_rate_limit_response(headers: &HeaderMap) -> bool {
    headers.contains_key(RETRY_AFTER)
        || headers
            .get("x-ratelimit-remaining")
            .and_then(|value| value.to_str().ok())
            == Some("0")
}

fn retry_after_from_headers(headers: &HeaderMap) -> Option<Duration> {
    if let Some(seconds) = headers
        .get(RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<u64>().ok())
    {
        return Some(Duration::from_secs(seconds.clamp(1, 24 * 60 * 60)));
    }
    let reset = headers
        .get("x-ratelimit-reset")?
        .to_str()
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
    Some(Duration::from_secs(
        reset.saturating_sub(now).clamp(1, 24 * 60 * 60),
    ))
}

fn map_reqwest_error(error: reqwest::Error) -> GitHubTransportError {
    if error.is_timeout() {
        GitHubTransportError::Timeout
    } else if error.is_connect() {
        GitHubTransportError::NetworkUnavailable
    } else {
        GitHubTransportError::Unavailable
    }
}

fn map_io_transport_error(error: std::io::Error) -> GitHubTransportError {
    if matches!(
        error.kind(),
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
    ) {
        GitHubTransportError::Timeout
    } else {
        GitHubTransportError::NetworkUnavailable
    }
}

fn retryable_transport_error(error: &GitHubTransportError) -> bool {
    matches!(
        error,
        GitHubTransportError::Timeout
            | GitHubTransportError::NetworkUnavailable
            | GitHubTransportError::Unavailable
    )
}

fn resolve_with_git_ls_remote(
    request: &GitHubResolveRequest,
) -> Result<GitHubCommit, GitHubTransportError> {
    if let GitHubReference::Commit(commit) = request.reference() {
        return Ok(commit.clone());
    }
    let repository_url = format!(
        "https://github.com/{}/{}.git",
        request.repository().owner(),
        request.repository().name()
    );
    let mut command = Command::new("git");
    command
        .arg("-c")
        .arg("credential.helper=")
        .arg("-c")
        .arg("core.askPass=")
        .arg("ls-remote")
        .arg("--symref")
        .arg("--exit-code")
        .arg(&repository_url);
    match request.reference() {
        GitHubReference::DefaultBranch => {
            command.arg("HEAD");
        }
        GitHubReference::Named(reference) => {
            command
                .arg(format!("refs/heads/{}", reference.as_str()))
                .arg(format!("refs/tags/{}", reference.as_str()))
                .arg(format!("refs/tags/{}^{{}}", reference.as_str()));
        }
        GitHubReference::Commit(_) => unreachable!("commit references return before spawning git"),
    }
    command
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "Never")
        .env(
            "GIT_CONFIG_GLOBAL",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        )
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = command
        .spawn()
        .map_err(|_| GitHubTransportError::Unavailable)?;
    let deadline = Instant::now()
        .checked_add(GITHUB_LS_REMOTE_TIMEOUT)
        .ok_or(GitHubTransportError::Unavailable)?;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(GitHubTransportError::Timeout);
            }
            Err(_) => return Err(GitHubTransportError::Unavailable),
        }
    };
    if !status.success() {
        return Err(GitHubTransportError::NotFound);
    }
    let mut stdout = child
        .stdout
        .take()
        .ok_or(GitHubTransportError::InvalidResponse)?;
    let mut bytes = Vec::new();
    stdout
        .by_ref()
        .take(GITHUB_API_BODY_BYTES.saturating_add(1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| GitHubTransportError::InvalidResponse)?;
    if bytes.len() > GITHUB_API_BODY_BYTES {
        return Err(GitHubTransportError::InvalidResponse);
    }
    parse_ls_remote_output(request.reference(), &bytes)
}

fn parse_ls_remote_output(
    reference: &GitHubReference,
    output: &[u8],
) -> Result<GitHubCommit, GitHubTransportError> {
    let output = std::str::from_utf8(output).map_err(|_| GitHubTransportError::InvalidResponse)?;
    let mut refs = BTreeMap::<&str, &str>::new();
    for line in output.lines() {
        if line.starts_with("ref: ") {
            continue;
        }
        let Some((sha, name)) = line.split_once('\t') else {
            return Err(GitHubTransportError::InvalidResponse);
        };
        refs.insert(name, sha);
    }
    let sha = match reference {
        GitHubReference::DefaultBranch => refs.get("HEAD").copied(),
        GitHubReference::Named(reference) => {
            let branch = format!("refs/heads/{}", reference.as_str());
            let peeled_tag = format!("refs/tags/{}^{{}}", reference.as_str());
            let tag = format!("refs/tags/{}", reference.as_str());
            refs.get(branch.as_str())
                .or_else(|| refs.get(peeled_tag.as_str()))
                .or_else(|| refs.get(tag.as_str()))
                .copied()
        }
        GitHubReference::Commit(commit) => return Ok(commit.clone()),
    }
    .ok_or(GitHubTransportError::NotFound)?;
    GitHubCommit::parse(sha).map_err(|_| GitHubTransportError::InvalidResponse)
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArchiveEntryScope {
    Unrelated,
    SelectedAncestor,
    SelectedRoot,
    SelectedDescendant,
}

impl ArchiveEntryScope {
    fn requires_strict_audit(self) -> bool {
        !matches!(self, Self::Unrelated)
    }
}

#[cfg(test)]
pub(super) fn extract_selected_skill(
    archive_bytes: &[u8],
    subdirectory: &GitHubSubdirectory,
) -> Result<Vec<(String, Vec<u8>)>, GitHubAcquisitionError> {
    extract_selected_skill_reader(Cursor::new(archive_bytes), subdirectory)
}

pub(super) fn extract_selected_skill_archive(
    archive: &GitHubArchive,
    subdirectory: &GitHubSubdirectory,
) -> Result<Vec<(String, Vec<u8>)>, GitHubAcquisitionError> {
    let reader = archive.reader().map_err(|_| invalid_archive())?;
    extract_selected_skill_reader(reader, subdirectory)
}

fn extract_selected_skill_reader<R: Read + Seek>(
    reader: R,
    subdirectory: &GitHubSubdirectory,
) -> Result<Vec<(String, Vec<u8>)>, GitHubAcquisitionError> {
    let mut archive = ZipArchive::new(reader).map_err(|_| invalid_archive())?;
    if archive.is_empty() || archive.len() > MAX_GITHUB_ARCHIVE_ENTRIES {
        return Err(acquisition_error(
            GitHubAcquisitionErrorCode::ArchiveBudgetExceeded,
            format!("GitHub archive must contain 1 to {MAX_GITHUB_ARCHIVE_ENTRIES} entries."),
        ));
    }

    let mut top_root: Option<String> = None;
    let mut selected_portable_entries = BTreeMap::<String, String>::new();
    let mut selected_portable_files = BTreeMap::<String, String>::new();
    let mut selected_portable_directories = BTreeMap::<String, String>::new();
    let mut selected_archive_uncompressed_bytes = 0_u64;
    let mut selected_bytes = 0usize;
    let mut selected = Vec::new();

    for index in 0..archive.len() {
        let entry = archive.by_index(index).map_err(|_| invalid_archive())?;
        let path = validate_archive_entry_path(&entry)?;
        let root = path.wrapper_root();
        match &top_root {
            None => top_root = Some(root.to_string()),
            Some(expected) if expected == root => {}
            Some(_) => {
                return Err(acquisition_error(
                    GitHubAcquisitionErrorCode::UnsafeArchiveEntry,
                    "GitHub archive contains more than one top-level root.",
                ));
            }
        }

        // The repository is untrusted, but only the selected subtree can cross the package
        // boundary. Unrelated entries still need canonical paths so membership and the codeload
        // wrapper are unambiguous; their file modes and compressed contents are never consumed.
        let scope = path.classify(subdirectory);
        if !scope.requires_strict_audit() {
            continue;
        }

        validate_strict_archive_entry(&entry)?;
        let components = path.strict_utf8_components()?;
        let repository_relative = &components[1..];
        let canonical_path = components.join("/");
        let collision_key = canonical_path.to_lowercase();
        if let Some(previous) =
            selected_portable_entries.insert(collision_key, canonical_path.clone())
        {
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
            &mut selected_portable_files,
            &mut selected_portable_directories,
        )?;

        let size = entry.size();
        if size > MAX_GITHUB_ARCHIVE_ENTRY_BYTES {
            return Err(acquisition_error(
                GitHubAcquisitionErrorCode::ArchiveBudgetExceeded,
                "A selected GitHub archive entry exceeds the uncompressed entry limit.",
            ));
        }
        selected_archive_uncompressed_bytes = selected_archive_uncompressed_bytes
            .checked_add(size)
            .ok_or_else(|| {
                acquisition_error(
                    GitHubAcquisitionErrorCode::ArchiveBudgetExceeded,
                    "Selected GitHub archive size accounting overflowed.",
                )
            })?;
        if selected_archive_uncompressed_bytes > MAX_GITHUB_ARCHIVE_UNCOMPRESSED_BYTES {
            return Err(acquisition_error(
                GitHubAcquisitionErrorCode::ArchiveBudgetExceeded,
                "Selected GitHub archive entries exceed the total uncompressed acquisition limit.",
            ));
        }
        if suspicious_expansion(size, entry.compressed_size()) {
            return Err(acquisition_error(
                GitHubAcquisitionErrorCode::ArchiveBudgetExceeded,
                "Selected GitHub archive entries contain an unsafe compression expansion ratio.",
            ));
        }

        match scope {
            ArchiveEntryScope::SelectedAncestor if !entry.is_dir() => {
                return Err(acquisition_error(
                    GitHubAcquisitionErrorCode::UnsafeArchiveEntry,
                    "An ancestor of the selected Skill directory resolves to a file.",
                ));
            }
            ArchiveEntryScope::SelectedRoot if !entry.is_dir() => {
                return Err(acquisition_error(
                    GitHubAcquisitionErrorCode::UnsafeArchiveEntry,
                    "The selected Skill directory resolves to a file.",
                ));
            }
            ArchiveEntryScope::SelectedAncestor | ArchiveEntryScope::SelectedRoot => continue,
            ArchiveEntryScope::SelectedDescendant => {}
            ArchiveEntryScope::Unrelated => {
                unreachable!("unrelated entries do not enter strict archive auditing")
            }
        }

        let logical_components = repository_relative
            .strip_prefix(subdirectory.components())
            .expect("selected descendants have the selected directory prefix");
        debug_assert!(!logical_components.is_empty());
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

#[derive(Debug)]
pub(super) struct ValidatedArchiveEntryPath {
    wrapper_root: String,
    repository_relative: Vec<Vec<u8>>,
    repository_relative_utf8: Option<Vec<String>>,
}

impl ValidatedArchiveEntryPath {
    pub(super) fn wrapper_root(&self) -> &str {
        &self.wrapper_root
    }

    pub(super) fn repository_relative_utf8(&self) -> Option<&[String]> {
        self.repository_relative_utf8.as_deref()
    }

    pub(super) fn is_within(&self, subdirectory: &GitHubSubdirectory) -> bool {
        matches!(
            self.classify(subdirectory),
            ArchiveEntryScope::SelectedRoot | ArchiveEntryScope::SelectedDescendant
        )
    }

    fn classify(&self, subdirectory: &GitHubSubdirectory) -> ArchiveEntryScope {
        let selected = subdirectory
            .components()
            .iter()
            .map(|component| component.as_bytes())
            .collect::<Vec<_>>();
        let candidate = self
            .repository_relative
            .iter()
            .map(Vec::as_slice)
            .collect::<Vec<_>>();
        if candidate == selected {
            ArchiveEntryScope::SelectedRoot
        } else if selected.starts_with(&candidate) {
            ArchiveEntryScope::SelectedAncestor
        } else if candidate.starts_with(&selected) {
            ArchiveEntryScope::SelectedDescendant
        } else {
            ArchiveEntryScope::Unrelated
        }
    }

    fn strict_utf8_components(&self) -> Result<Vec<String>, GitHubAcquisitionError> {
        let relative = self.repository_relative_utf8.as_ref().ok_or_else(|| {
            acquisition_error(
                GitHubAcquisitionErrorCode::UnsafeArchiveEntry,
                "The selected GitHub Skill contains a non-UTF-8 path.",
            )
        })?;
        if relative
            .iter()
            .any(|component| component.contains(':') || component.chars().any(char::is_control))
        {
            return Err(unsafe_archive_path());
        }
        let mut components = Vec::with_capacity(relative.len().saturating_add(1));
        components.push(self.wrapper_root.clone());
        components.extend(relative.iter().cloned());
        Ok(components)
    }
}

pub(super) fn validate_archive_entry_path(
    entry: &zip::read::ZipFile<'_>,
) -> Result<ValidatedArchiveEntryPath, GitHubAcquisitionError> {
    let raw_name = entry.name_raw();
    if raw_name.is_empty()
        || raw_name.len() > MAX_GITHUB_ARCHIVE_PATH_BYTES
        || raw_name.starts_with(b"/")
        || raw_name.contains(&b'\\')
        || raw_name.contains(&b'\0')
    {
        return Err(unsafe_archive_path());
    }
    let directory = entry.is_dir();
    let path = if directory {
        raw_name
            .strip_suffix(b"/")
            .ok_or_else(unsafe_archive_path)?
    } else {
        if raw_name.ends_with(b"/") {
            return Err(unsafe_archive_path());
        }
        raw_name
    };
    let components = path
        .split(|byte| *byte == b'/')
        .map(<[u8]>::to_vec)
        .collect::<Vec<_>>();
    if components.is_empty()
        || components.len() > MAX_GITHUB_ARCHIVE_PATH_COMPONENTS
        || components.iter().any(|component| {
            component.is_empty()
                || matches!(component.as_slice(), b"." | b"..")
                || component.len() > MAX_GITHUB_ARCHIVE_PATH_COMPONENT_BYTES
        })
    {
        return Err(unsafe_archive_path());
    }
    let wrapper_root = std::str::from_utf8(&components[0])
        .ok()
        .filter(|root| {
            !root.is_empty() && !root.contains(':') && !root.chars().any(char::is_control)
        })
        .ok_or_else(unsafe_archive_path)?
        .to_string();
    let repository_relative = components[1..].to_vec();
    let repository_relative_utf8 = repository_relative
        .iter()
        .map(|component| std::str::from_utf8(component).map(str::to_string))
        .collect::<Result<Vec<_>, _>>()
        .ok();
    Ok(ValidatedArchiveEntryPath {
        wrapper_root,
        repository_relative,
        repository_relative_utf8,
    })
}

fn validate_strict_archive_entry(
    entry: &zip::read::ZipFile<'_>,
) -> Result<(), GitHubAcquisitionError> {
    if entry.encrypted() || !is_supported_compression(entry.compression()) {
        return Err(acquisition_error(
            GitHubAcquisitionErrorCode::UnsafeArchiveEntry,
            "GitHub archive contains an encrypted or unsupported entry.",
        ));
    }
    let directory = entry.is_dir();
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
    Ok(())
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
        GitHubTransportError::RateLimited { .. } => (
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
        GitHubTransportError::Timeout
        | GitHubTransportError::NetworkUnavailable
        | GitHubTransportError::Unavailable => (
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

fn invalid_origin() -> GitHubAcquisitionError {
    acquisition_error(
        GitHubAcquisitionErrorCode::InvalidOrigin,
        "Cannot construct bounded, credential-free GitHub installation provenance.",
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
    use std::collections::VecDeque;
    use std::io::Write;
    use std::sync::Mutex;
    use tempfile::tempdir;
    use zip::write::SimpleFileOptions;
    use zip::ZipWriter;

    const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";
    const OTHER_COMMIT: &str = "89abcdef0123456789abcdef0123456789abcdef";
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
        ) -> Result<GitHubArchive, GitHubTransportError> {
            self.state
                .lock()
                .unwrap()
                .archive_requests
                .push(request.clone());
            GitHubArchive::from_bytes(&self.archive)
        }
    }

    struct SequencedTransport {
        state: Mutex<SequencedState>,
    }

    struct SequencedState {
        responses: VecDeque<(GitHubCommit, Vec<u8>)>,
        pending_archive: Option<Vec<u8>>,
        resolve_count: usize,
        archive_count: usize,
    }

    impl SequencedTransport {
        fn new(responses: Vec<(GitHubCommit, Vec<u8>)>) -> Self {
            Self {
                state: Mutex::new(SequencedState {
                    responses: responses.into(),
                    pending_archive: None,
                    resolve_count: 0,
                    archive_count: 0,
                }),
            }
        }
    }

    impl GitHubAcquisitionTransport for SequencedTransport {
        fn resolve_commit(
            &self,
            _request: &GitHubResolveRequest,
        ) -> Result<GitHubCommit, GitHubTransportError> {
            let mut state = self.state.lock().unwrap();
            let (commit, archive) = state
                .responses
                .pop_front()
                .ok_or(GitHubTransportError::Unavailable)?;
            state.resolve_count += 1;
            state.pending_archive = Some(archive);
            Ok(commit)
        }

        fn download_archive(
            &self,
            _request: &GitHubArchiveRequest,
        ) -> Result<GitHubArchive, GitHubTransportError> {
            let mut state = self.state.lock().unwrap();
            state.archive_count += 1;
            let bytes = state
                .pending_archive
                .take()
                .ok_or(GitHubTransportError::Unavailable)?;
            GitHubArchive::from_bytes(&bytes)
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
    fn tracking_refresh_reacquires_new_bytes_and_projects_typed_source_metadata() {
        let first_archive = write_zip(
            &[("repo-root/skills/a/SKILL.md", SKILL)],
            CompressionMethod::Stored,
        );
        let second_skill = b"---\nname: github-skill\ndescription: Acquired from GitHub.\n---\n\nUPDATED THROUGH TRACKING REF.\n";
        let second_archive = write_zip(
            &[("repo-root/skills/a/SKILL.md", second_skill)],
            CompressionMethod::Stored,
        );
        let transport = Arc::new(SequencedTransport::new(vec![
            (GitHubCommit::parse(COMMIT).unwrap(), first_archive),
            (GitHubCommit::parse(OTHER_COMMIT).unwrap(), second_archive),
        ]));
        let adapter = GitHubWorkflowAcquisitionAdapter::new(Arc::new(
            GitHubSkillAcquirer::with_transport(transport.clone()),
        ));
        let source = GitHubWorkflowAcquisitionAdapter::source_for(&GitHubSkillLocation::new(
            GitHubRepository::parse("openai", "skills").unwrap(),
            GitHubReference::named("main").unwrap(),
            GitHubSubdirectory::parse("skills/a").unwrap(),
        ))
        .unwrap();

        let first = adapter.acquire(&source).unwrap();
        let refresh = first.provenance().refresh().unwrap().clone();
        let second = adapter.reacquire(refresh.adapter_view()).unwrap();

        assert_ne!(first.package().revision(), second.package().revision());
        assert!(second
            .package()
            .instructions()
            .contains("UPDATED THROUGH TRACKING REF"));
        assert!(matches!(
            adapter.installed_source_presentation(second.provenance().adapter_view(), true),
            Some(InstalledSkillSourcePresentation::GitHub {
                tracking_reference: InstalledGitHubTrackingReference::Named(reference),
                resolved_commit,
                refreshable: true,
                ..
            }) if reference == "main" && resolved_commit == OTHER_COMMIT
        ));
        let state = transport.state.lock().unwrap();
        assert_eq!(state.resolve_count, 2);
        assert_eq!(state.archive_count, 2);
    }

    #[test]
    fn immutable_commit_provenance_has_no_refresh_capability() {
        let transport = Arc::new(FakeTransport::new(write_zip(
            &[("repo-root/SKILL.md", SKILL)],
            CompressionMethod::Stored,
        )));
        let adapter = GitHubWorkflowAcquisitionAdapter::new(Arc::new(
            GitHubSkillAcquirer::with_transport(transport),
        ));
        let source = GitHubWorkflowAcquisitionAdapter::source_for(&GitHubSkillLocation::new(
            GitHubRepository::parse("openai", "skills").unwrap(),
            GitHubReference::commit(COMMIT).unwrap(),
            GitHubSubdirectory::root(),
        ))
        .unwrap();

        let acquisition = adapter.acquire(&source).unwrap();

        assert!(acquisition.provenance().refresh().is_none());
        assert!(matches!(
            adapter.installed_source_presentation(acquisition.provenance().adapter_view(), false),
            Some(InstalledSkillSourcePresentation::GitHub {
                tracking_reference: InstalledGitHubTrackingReference::Commit,
                refreshable: false,
                ..
            })
        ));
    }

    #[test]
    fn authority_projection_rejects_unknown_payload_fields() {
        let authority = SkillInstallationAuthority::new(
            GITHUB_SKILL_ORIGIN_PROVIDER,
            GITHUB_PROVENANCE_SCHEMA_VERSION,
            serde_json::json!({
                "owner": "openai",
                "repository": "skills",
                "resolvedCommit": COMMIT,
                "unexpected": "must fail closed"
            })
            .to_string(),
        )
        .unwrap();
        let provenance = SkillInstallationProvenance::new(authority, None);

        assert!(github_source_presentation(provenance.adapter_view(), false).is_none());
    }

    #[test]
    fn immutable_commit_requests_skip_ref_resolution_and_download_the_exact_sha() {
        let transport = Arc::new(FakeTransport::new(write_zip(
            &[("repo-root/SKILL.md", SKILL)],
            CompressionMethod::Stored,
        )));
        let acquirer = GitHubSkillAcquirer::with_transport(transport.clone());
        let location = GitHubSkillLocation::new(
            GitHubRepository::parse("openai", "skills").unwrap(),
            GitHubReference::commit(OTHER_COMMIT).unwrap(),
            GitHubSubdirectory::root(),
        );

        let acquired = acquirer.acquire(location).unwrap();

        assert_eq!(acquired.summary().resolved_commit().as_str(), OTHER_COMMIT);
        let state = transport.state.lock().unwrap();
        assert!(state.resolve_requests.is_empty());
        assert_eq!(state.archive_requests.len(), 1);
        assert_eq!(state.archive_requests[0].commit().as_str(), OTHER_COMMIT);
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
    fn ls_remote_parser_supports_default_branch_branch_and_annotated_tag() {
        let default = parse_ls_remote_output(
            &GitHubReference::DefaultBranch,
            format!("ref: refs/heads/main\tHEAD\n{COMMIT}\tHEAD\n").as_bytes(),
        )
        .unwrap();
        assert_eq!(default.as_str(), COMMIT);

        let branch = parse_ls_remote_output(
            &GitHubReference::named("feature/a").unwrap(),
            format!("{COMMIT}\trefs/heads/feature/a\n").as_bytes(),
        )
        .unwrap();
        assert_eq!(branch.as_str(), COMMIT);

        let tag = parse_ls_remote_output(
            &GitHubReference::named("v1").unwrap(),
            format!("{OTHER_COMMIT}\trefs/tags/v1\n{COMMIT}\trefs/tags/v1^{{}}\n").as_bytes(),
        )
        .unwrap();
        assert_eq!(tag.as_str(), COMMIT);
    }

    #[test]
    fn archive_spool_detects_interrupted_and_length_mismatched_downloads() {
        struct InterruptedReader {
            delivered: bool,
        }

        impl Read for InterruptedReader {
            fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                if self.delivered {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::ConnectionReset,
                        "fixture interruption",
                    ));
                }
                self.delivered = true;
                buffer[..4].copy_from_slice(b"data");
                Ok(4)
            }
        }

        assert_eq!(
            spool_archive_reader(&mut InterruptedReader { delivered: false }, None).unwrap_err(),
            GitHubTransportError::NetworkUnavailable
        );
        assert_eq!(
            spool_archive_reader(&mut Cursor::new(b"short"), Some(99)).unwrap_err(),
            GitHubTransportError::InvalidResponse
        );
        let archive = spool_archive_reader(&mut Cursor::new(b"complete"), Some(8)).unwrap();
        assert_eq!(archive.len(), 8);
        assert!(archive.verify_integrity().is_ok());
    }

    #[test]
    fn github_rate_limit_statuses_preserve_bounded_retry_hints() {
        let mut primary_headers = HeaderMap::new();
        primary_headers.insert("x-ratelimit-remaining", "0".parse().unwrap());
        let primary = map_status_and_headers(StatusCode::FORBIDDEN, &primary_headers);
        assert!(matches!(
            primary,
            GitHubTransportError::RateLimited {
                retry_after: Some(duration),
                secondary: false,
            } if duration == GITHUB_RATE_LIMIT_FALLBACK
        ));

        let mut secondary_headers = HeaderMap::new();
        secondary_headers.insert(RETRY_AFTER, "7".parse().unwrap());
        let secondary = map_status_and_headers(StatusCode::TOO_MANY_REQUESTS, &secondary_headers);
        assert!(!retryable_transport_error(&secondary));
        assert!(matches!(
            secondary,
            GitHubTransportError::RateLimited {
                retry_after: Some(duration),
                secondary: true,
            } if duration == Duration::from_secs(7)
        ));

        assert_eq!(
            map_status_and_headers(StatusCode::FORBIDDEN, &HeaderMap::new()),
            GitHubTransportError::Rejected
        );
        assert!(retryable_transport_error(&GitHubTransportError::Timeout));
        assert!(!retryable_transport_error(
            &GitHubTransportError::InvalidResponse
        ));
    }

    #[test]
    fn transport_requests_are_constructed_from_validated_coordinates() {
        let repository = GitHubRepository::parse("owner", "repo").unwrap();
        let reference = GitHubReference::named("release/v1").unwrap();
        let resolve = GitHubResolveRequest::new(repository.clone(), reference.clone());
        assert_eq!(resolve.repository(), &repository);
        assert_eq!(resolve.reference(), &reference);

        let commit = GitHubCommit::parse(COMMIT).unwrap();
        let archive = GitHubArchiveRequest::new(repository.clone(), commit.clone());
        assert_eq!(archive.repository(), &repository);
        assert_eq!(archive.commit(), &commit);
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
    fn unrelated_repository_symlink_does_not_block_a_selected_skill() {
        let archive = write_zip_with_symlink(
            &[("root/skills/a/SKILL.md", SKILL)],
            "root/CLAUDE.md",
            "AGENTS.md",
        );

        let files =
            extract_selected_skill(&archive, &GitHubSubdirectory::parse("skills/a").unwrap())
                .unwrap();

        assert_eq!(files, vec![(SKILL_FILE_NAME.to_string(), SKILL.to_vec())]);
    }

    #[test]
    fn symlinks_at_or_below_the_selected_boundary_are_rejected() {
        for link in ["root/skills", "root/skills/a", "root/skills/a/assets/link"] {
            let archive =
                write_zip_with_symlink(&[("root/skills/a/SKILL.md", SKILL)], link, "../../outside");

            assert_eq!(
                extract_selected_skill(&archive, &GitHubSubdirectory::parse("skills/a").unwrap(),)
                    .unwrap_err()
                    .code(),
                GitHubAcquisitionErrorCode::UnsafeArchiveEntry,
                "selected boundary symlink: {link}",
            );
        }
    }

    #[test]
    fn unrelated_collisions_and_declared_sizes_do_not_affect_selected_skill() {
        let mut archive = write_zip(
            &[
                ("root/skills/a/SKILL.md", SKILL),
                ("root/unrelated/Guide.md", b"first"),
                ("root/unrelated/guide.md", b"second"),
                ("root/unrelated/large.bin", b"small"),
            ],
            CompressionMethod::Stored,
        );
        set_central_uncompressed_size(
            &mut archive,
            "root/unrelated/large.bin",
            u32::try_from(MAX_GITHUB_ARCHIVE_ENTRY_BYTES + 1).unwrap(),
        );

        let files =
            extract_selected_skill(&archive, &GitHubSubdirectory::parse("skills/a").unwrap())
                .unwrap();

        assert_eq!(files, vec![(SKILL_FILE_NAME.to_string(), SKILL.to_vec())]);
    }

    #[test]
    fn selected_collisions_and_declared_sizes_remain_rejected() {
        let collision = write_zip(
            &[
                ("root/skills/a/SKILL.md", SKILL),
                ("root/skills/a/assets/Guide.md", b"first"),
                ("root/skills/a/assets/guide.md", b"second"),
            ],
            CompressionMethod::Stored,
        );
        assert_eq!(
            extract_selected_skill(&collision, &GitHubSubdirectory::parse("skills/a").unwrap(),)
                .unwrap_err()
                .code(),
            GitHubAcquisitionErrorCode::UnsafeArchiveEntry,
        );

        let mut oversized = write_zip(
            &[
                ("root/skills/a/SKILL.md", SKILL),
                ("root/skills/a/assets/large.bin", b"small"),
            ],
            CompressionMethod::Stored,
        );
        set_central_uncompressed_size(
            &mut oversized,
            "root/skills/a/assets/large.bin",
            u32::try_from(MAX_GITHUB_ARCHIVE_ENTRY_BYTES + 1).unwrap(),
        );
        assert_eq!(
            extract_selected_skill(&oversized, &GitHubSubdirectory::parse("skills/a").unwrap(),)
                .unwrap_err()
                .code(),
            GitHubAcquisitionErrorCode::ArchiveBudgetExceeded,
        );
    }

    #[test]
    fn compression_expansion_is_scoped_to_selected_entries() {
        let highly_compressible = vec![0_u8; 4 * 1024 * 1024];
        let archive = write_zip(
            &[
                ("root/skills/a/SKILL.md", SKILL),
                ("root/unrelated/bomb.bin", &highly_compressible),
            ],
            CompressionMethod::Deflated,
        );

        assert!(
            extract_selected_skill(&archive, &GitHubSubdirectory::parse("skills/a").unwrap(),)
                .is_ok()
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
    fn unrelated_nonportable_names_do_not_block_a_selected_skill() {
        let mut archive = write_zip(
            &[
                ("root/skills/a/SKILL.md", SKILL),
                ("root/unrelated/unique-name.bin", b"outside"),
                ("root/unrelated/legacy:name.txt", b"outside"),
            ],
            CompressionMethod::Stored,
        );
        replace_all(&mut archive, b"unique-name.bin", b"unique-nam\xff.bin");

        let files =
            extract_selected_skill(&archive, &GitHubSubdirectory::parse("skills/a").unwrap())
                .unwrap();

        assert_eq!(files, vec![(SKILL_FILE_NAME.to_string(), SKILL.to_vec())]);
    }

    #[test]
    fn selected_nonportable_names_remain_rejected() {
        let archive = write_zip(
            &[
                ("root/skills/a/SKILL.md", SKILL),
                ("root/skills/a/assets/legacy:name.txt", b"selected"),
            ],
            CompressionMethod::Stored,
        );

        assert_eq!(
            extract_selected_skill(&archive, &GitHubSubdirectory::parse("skills/a").unwrap())
                .unwrap_err()
                .code(),
            GitHubAcquisitionErrorCode::UnsafeArchiveEntry
        );
    }

    #[test]
    fn selected_directory_preserves_safe_generic_sibling_files() {
        let archive = write_zip(
            &[
                ("root/SKILL.md", SKILL),
                ("root/README.md", b"not a package resource"),
            ],
            CompressionMethod::Stored,
        );
        assert_eq!(
            extract_selected_skill(&archive, &GitHubSubdirectory::root()).unwrap(),
            vec![
                (SKILL_FILE_NAME.to_string(), SKILL.to_vec()),
                ("README.md".to_string(), b"not a package resource".to_vec()),
            ]
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

    fn write_zip_with_symlink(
        entries: &[(&str, &[u8])],
        symlink_name: &str,
        target: &str,
    ) -> Vec<u8> {
        let cursor = Cursor::new(Vec::new());
        let mut writer = ZipWriter::new(cursor);
        let options = SimpleFileOptions::default().unix_permissions(0o644);
        for (name, bytes) in entries {
            writer.start_file(*name, options).unwrap();
            writer.write_all(bytes).unwrap();
        }
        writer.add_symlink(symlink_name, target, options).unwrap();
        writer.finish().unwrap().into_inner()
    }

    fn write_symlink_zip() -> Vec<u8> {
        write_zip_with_symlink(
            &[("root/SKILL.md", SKILL)],
            "root/assets/link",
            "../../outside",
        )
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

    fn set_central_uncompressed_size(bytes: &mut [u8], entry_name: &str, size: u32) {
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
                bytes[start + 24..start + 28].copy_from_slice(&size.to_le_bytes());
                return;
            }
            offset = name_end;
        }
        panic!("central entry not found");
    }
}
