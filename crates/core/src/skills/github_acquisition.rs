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

mod archive;
mod transport;

#[cfg(test)]
use archive::extract_selected_skill;
pub(super) use archive::{extract_selected_skill_archive, validate_archive_entry_path};
use transport::*;

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
mod tests;
