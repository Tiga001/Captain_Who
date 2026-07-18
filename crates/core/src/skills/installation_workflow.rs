//! Two-phase orchestration for acquiring and committing managed Skills.
//!
//! Inspection captures one exact, validated [`PreparedSkillAcquisition`] and
//! returns a safe preview. Commit consumes that snapshot; it never re-reads
//! the acquisition source. A bounded, expiring registry makes retries
//! idempotent without turning preparation IDs into permanent server state.
//!
//! Acquisition is an adapter boundary. Local directories are built in, while
//! future Git, archive, or registry adapters can register another provider and
//! still enter the same preview, acknowledgement, and installation transaction.

use super::acquisition_provenance::{
    SkillInstallationAuthority, SkillInstallationProvenance, SkillInstallationProvenanceView,
    SkillInstallationRefresh, SkillInstallationRefreshView,
};
use super::installation_service::{
    InstalledSkillRecord, SkillInstallationMutation, SkillInstallationOperation,
    SkillInstallationService, SkillInstallationServiceError,
};
#[cfg(test)]
use super::installation_session::SessionClock;
use super::installation_session::{
    duration_millis, InstallationSessionState, PreparationSlot, ResolutionSlot,
    SkillInstallationSessionConfig, SkillInstallationSessionStore,
};
use super::installed::USER_INSTALLED_SKILL_SOURCE_ID;
use super::model::{
    SkillId, SkillInstallationId, SkillInstallationRevision, SkillResourceKind, SkillRevision,
    SkillSourceId,
};
use super::package::MAX_SKILL_PACKAGE_BYTES;
use super::prepared::{PreparedSkillPackage, SkillPackagePreparationError};
use super::prepared_acquisition::PreparedSkillAcquisition;
use super::source_resolution::{SkillSourceCandidateId, SkillSourceResolutionId};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, MutexGuard};
use std::time::Duration;
use uuid::Uuid;

const LOCAL_DIRECTORY_PROVIDER: &str = "local-directory";
const RESOLVED_CANDIDATE_PROVIDER: &str = "resolved-candidate";
const INSTALLED_SOURCE_PROVIDER: &str = "installed-source";
const LOCAL_DIRECTORY_ORIGIN_REFERENCE: &str = "user-selected-directory";
const LOCAL_DIRECTORY_PROVENANCE_AUTHORITY: &str = "user-selected-snapshot";
const MAX_PROVIDER_BYTES: usize = 64;
const MAX_ADAPTER_REQUEST_BYTES: usize = 64 * 1024;
const MAX_REGISTRY_ENTRIES: usize = 4_096;
const MAX_REGISTRY_SNAPSHOT_BYTES: usize = 2 * 1024 * 1024 * 1024;
const MAX_PREPARATION_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// Default number of live preparation identities retained by a workflow.
pub const DEFAULT_MAX_SKILL_PREPARATIONS: usize = 64;
/// Default aggregate memory budget for exact prepared package snapshots.
pub const DEFAULT_MAX_SKILL_PREPARATION_BYTES: usize = 256 * 1024 * 1024;
/// Default time for which an uncommitted preview remains usable.
pub const DEFAULT_SKILL_PREPARATION_TTL: Duration = Duration::from_secs(15 * 60);

const PREVIEW_REVISION_PREFIX: &str = "skill-install-preview-sha256-v1:";

/// Revision of the exact preview contract accepted by a commit.
///
/// This is distinct from [`SkillRevision`]: a package revision identifies the
/// package bytes used by the installer's CAS, while a preview revision also
/// binds the operation target, acquisition request, warnings, and expiration.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SkillPreviewRevision(String);

impl SkillPreviewRevision {
    pub fn parse(value: impl Into<String>) -> Result<Self, SkillPreviewRevisionError> {
        let value = value.into();
        let digest = value
            .strip_prefix(PREVIEW_REVISION_PREFIX)
            .ok_or_else(|| SkillPreviewRevisionError::new("invalid preview revision prefix"))?;
        if digest.len() != 64
            || !digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        {
            return Err(SkillPreviewRevisionError::new(
                "preview revision digest must contain 64 lowercase hexadecimal characters",
            ));
        }
        Ok(Self(value))
    }

    fn from_digest(digest: [u8; 32]) -> Self {
        let mut value = String::with_capacity(PREVIEW_REVISION_PREFIX.len() + 64);
        value.push_str(PREVIEW_REVISION_PREFIX);
        for byte in digest {
            use std::fmt::Write as _;
            write!(&mut value, "{byte:02x}").expect("writing to a String cannot fail");
        }
        Self(value)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SkillPreviewRevision {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("SkillPreviewRevision")
            .field(&self.0)
            .finish()
    }
}

impl fmt::Display for SkillPreviewRevision {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillPreviewRevisionError {
    reason: String,
}

impl SkillPreviewRevisionError {
    fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }

    pub fn reason(&self) -> &str {
        &self.reason
    }
}

impl fmt::Display for SkillPreviewRevisionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.reason.fmt(formatter)
    }
}

impl Error for SkillPreviewRevisionError {}

/// Client-generated idempotency identity for one prepare/commit conversation.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SkillPreparationId(String);

impl SkillPreparationId {
    pub fn new() -> Self {
        Self(Uuid::new_v4().hyphenated().to_string())
    }

    pub fn parse(value: impl Into<String>) -> Result<Self, SkillPreparationIdError> {
        let value = value.into();
        let uuid = Uuid::parse_str(&value)
            .map_err(|_| SkillPreparationIdError::new("preparation id must be a UUID"))?;
        if uuid.is_nil() {
            return Err(SkillPreparationIdError::new(
                "preparation id must not be the nil UUID",
            ));
        }
        if uuid.hyphenated().to_string() != value {
            return Err(SkillPreparationIdError::new(
                "preparation id must use canonical lowercase hyphenated UUID form",
            ));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for SkillPreparationId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for SkillPreparationId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("SkillPreparationId")
            .field(&self.0)
            .finish()
    }
}

impl fmt::Display for SkillPreparationId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillPreparationIdError {
    reason: String,
}

impl SkillPreparationIdError {
    fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }

    pub fn reason(&self) -> &str {
        &self.reason
    }
}

impl fmt::Display for SkillPreparationIdError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.reason.fmt(formatter)
    }
}

impl Error for SkillPreparationIdError {}

/// Stable dispatch key for an acquisition adapter.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SkillAcquisitionProvider(String);

impl SkillAcquisitionProvider {
    pub fn parse(value: impl Into<String>) -> Result<Self, SkillAcquisitionProviderError> {
        let value = value.into();
        let valid = !value.is_empty()
            && value.len() <= MAX_PROVIDER_BYTES
            && value.as_bytes()[0].is_ascii_lowercase()
            && value
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
        if !valid {
            return Err(SkillAcquisitionProviderError::new(format!(
                "acquisition provider must contain 1 to {MAX_PROVIDER_BYTES} lowercase ASCII letters, digits, or hyphens and start with a letter",
            )));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn local_directory() -> Self {
        Self::parse(LOCAL_DIRECTORY_PROVIDER)
            .expect("the built-in local-directory provider id must remain valid")
    }
}

impl fmt::Display for SkillAcquisitionProvider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillAcquisitionProviderError {
    reason: String,
}

impl SkillAcquisitionProviderError {
    fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }

    pub fn reason(&self) -> &str {
        &self.reason
    }
}

impl fmt::Display for SkillAcquisitionProviderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.reason.fmt(formatter)
    }
}

impl Error for SkillAcquisitionProviderError {}

/// Provider-specific acquisition input.
///
/// The custom `Debug` implementation deliberately never prints a local path
/// or opaque request bytes. Adapter request bytes are a bounded transport
/// envelope, not trusted package contents; the adapter must validate them and
/// construct a [`PreparedSkillPackage`] before commit becomes possible.
#[derive(Clone, PartialEq, Eq)]
pub enum SkillAcquisitionSource {
    LocalDirectory {
        directory: PathBuf,
    },
    Adapter {
        provider: SkillAcquisitionProvider,
        request: Arc<[u8]>,
    },
    ResolvedCandidate {
        resolution_id: SkillSourceResolutionId,
        candidate_id: SkillSourceCandidateId,
    },
    InstalledSource,
}

impl SkillAcquisitionSource {
    pub fn local_directory(directory: impl Into<PathBuf>) -> Self {
        Self::LocalDirectory {
            directory: directory.into(),
        }
    }

    pub fn adapter(
        provider: SkillAcquisitionProvider,
        request: impl Into<Vec<u8>>,
    ) -> Result<Self, SkillAcquisitionSourceError> {
        let request = request.into();
        if request.is_empty() || request.len() > MAX_ADAPTER_REQUEST_BYTES {
            return Err(SkillAcquisitionSourceError::new(format!(
                "adapter request must contain 1 to {MAX_ADAPTER_REQUEST_BYTES} bytes",
            )));
        }
        Ok(Self::Adapter {
            provider,
            request: request.into(),
        })
    }

    pub fn resolved_candidate(
        resolution_id: SkillSourceResolutionId,
        candidate_id: SkillSourceCandidateId,
    ) -> Self {
        Self::ResolvedCandidate {
            resolution_id,
            candidate_id,
        }
    }

    /// Reacquires the update target from the typed refresh metadata stored in
    /// its installation receipt. The source contains no provider payload;
    /// workflow inspection reads and validates the authoritative receipt.
    pub fn installed_source() -> Self {
        Self::InstalledSource
    }

    pub fn provider(&self) -> SkillAcquisitionProvider {
        match self {
            Self::LocalDirectory { .. } => SkillAcquisitionProvider::local_directory(),
            Self::Adapter { provider, .. } => provider.clone(),
            Self::ResolvedCandidate { .. } => {
                SkillAcquisitionProvider::parse(RESOLVED_CANDIDATE_PROVIDER)
                    .expect("the built-in resolved-candidate provider id must remain valid")
            }
            Self::InstalledSource => SkillAcquisitionProvider::parse(INSTALLED_SOURCE_PROVIDER)
                .expect("the built-in installed-source provider id must remain valid"),
        }
    }

    pub fn local_directory_path(&self) -> Option<&Path> {
        match self {
            Self::LocalDirectory { directory } => Some(directory),
            Self::Adapter { .. } => None,
            Self::ResolvedCandidate { .. } => None,
            Self::InstalledSource => None,
        }
    }

    pub fn adapter_request(&self) -> Option<&[u8]> {
        match self {
            Self::LocalDirectory { .. } => None,
            Self::Adapter { request, .. } => Some(request),
            Self::ResolvedCandidate { .. } => None,
            Self::InstalledSource => None,
        }
    }
}

impl fmt::Debug for SkillAcquisitionSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LocalDirectory { .. } => formatter
                .debug_struct("LocalDirectory")
                .field("directory", &"[redacted]")
                .finish(),
            Self::Adapter { provider, request } => formatter
                .debug_struct("Adapter")
                .field("provider", provider)
                .field("request_bytes", &request.len())
                .finish(),
            Self::ResolvedCandidate { .. } => formatter
                .debug_struct("ResolvedCandidate")
                .field("resolution_id", &"[redacted]")
                .field("candidate_id", &"[redacted]")
                .finish(),
            Self::InstalledSource => formatter.debug_struct("InstalledSource").finish(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillAcquisitionSourceError {
    reason: String,
}

impl SkillAcquisitionSourceError {
    fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }

    pub fn reason(&self) -> &str {
        &self.reason
    }
}

impl fmt::Display for SkillAcquisitionSourceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.reason.fmt(formatter)
    }
}

impl Error for SkillAcquisitionSourceError {}

/// One acquisition implementation registered with the workflow.
pub trait SkillAcquisitionAdapter: Send + Sync {
    fn provider(&self) -> SkillAcquisitionProvider;

    /// Converts provider input into owned, fully validated bytes and typed
    /// receipt provenance.
    /// Implementations must return sanitized errors: paths, credentials, and
    /// opaque request contents must not be included in diagnostic messages.
    fn acquire(
        &self,
        source: &SkillAcquisitionSource,
    ) -> Result<PreparedSkillAcquisition, SkillAcquisitionAdapterError>;

    /// Exact refresh payload schema versions accepted by [`Self::reacquire`].
    /// An empty slice makes the adapter explicitly non-refreshable.
    fn refresh_schema_versions(&self) -> &'static [u32] {
        &[]
    }

    /// Reacquires from credential-free receipt metadata. Implementations must
    /// decode and revalidate the payload on every call. The borrowed view is a
    /// callback-scoped capability; it cannot be constructed from general
    /// receipt APIs or retained beyond the receipt borrow.
    fn reacquire(
        &self,
        _refresh: SkillInstallationRefreshView<'_>,
    ) -> Result<PreparedSkillAcquisition, SkillAcquisitionAdapterError> {
        Err(SkillAcquisitionAdapterError::invalid_request(
            "this acquisition provider does not support installed-source refresh",
        ))
    }

    /// Projects validated receipt metadata into a payload-free management
    /// view. Providers opt in explicitly; malformed or mismatched metadata
    /// returns `None` and is presented as unknown.
    fn installed_source_presentation(
        &self,
        _provenance: SkillInstallationProvenanceView<'_>,
        _refresh_capable: bool,
    ) -> Option<InstalledSkillSourcePresentation> {
        None
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum SkillAcquisitionAdapterErrorCode {
    InvalidRequest,
    NotFound,
    RateLimited,
    TooLarge,
    UnsafePackage,
    PreparationFailed,
    Unavailable,
}

/// Sanitized failure from an acquisition adapter.
pub enum SkillAcquisitionAdapterError {
    InvalidRequest {
        reason: String,
    },
    NotFound {
        reason: String,
    },
    RateLimited {
        reason: String,
    },
    TooLarge {
        reason: String,
    },
    UnsafePackage {
        reason: String,
    },
    Preparation {
        source: Box<SkillPackagePreparationError>,
    },
    Unavailable {
        reason: String,
    },
}

impl SkillAcquisitionAdapterError {
    pub fn invalid_request(reason: impl Into<String>) -> Self {
        Self::InvalidRequest {
            reason: reason.into(),
        }
    }

    pub fn unavailable(reason: impl Into<String>) -> Self {
        Self::Unavailable {
            reason: reason.into(),
        }
    }

    pub fn not_found(reason: impl Into<String>) -> Self {
        Self::NotFound {
            reason: reason.into(),
        }
    }

    pub fn rate_limited(reason: impl Into<String>) -> Self {
        Self::RateLimited {
            reason: reason.into(),
        }
    }

    pub fn too_large(reason: impl Into<String>) -> Self {
        Self::TooLarge {
            reason: reason.into(),
        }
    }

    pub fn unsafe_package(reason: impl Into<String>) -> Self {
        Self::UnsafePackage {
            reason: reason.into(),
        }
    }

    pub fn code(&self) -> SkillAcquisitionAdapterErrorCode {
        match self {
            Self::InvalidRequest { .. } => SkillAcquisitionAdapterErrorCode::InvalidRequest,
            Self::NotFound { .. } => SkillAcquisitionAdapterErrorCode::NotFound,
            Self::RateLimited { .. } => SkillAcquisitionAdapterErrorCode::RateLimited,
            Self::TooLarge { .. } => SkillAcquisitionAdapterErrorCode::TooLarge,
            Self::UnsafePackage { .. } => SkillAcquisitionAdapterErrorCode::UnsafePackage,
            Self::Preparation { .. } => SkillAcquisitionAdapterErrorCode::PreparationFailed,
            Self::Unavailable { .. } => SkillAcquisitionAdapterErrorCode::Unavailable,
        }
    }

    pub fn reason(&self) -> &str {
        match self {
            Self::InvalidRequest { reason }
            | Self::NotFound { reason }
            | Self::RateLimited { reason }
            | Self::TooLarge { reason }
            | Self::UnsafePackage { reason }
            | Self::Unavailable { reason } => reason,
            Self::Preparation { source } => source.reason(),
        }
    }

    pub fn preparation_error(&self) -> Option<&SkillPackagePreparationError> {
        match self {
            Self::Preparation { source } => Some(source),
            _ => None,
        }
    }
}

impl From<SkillPackagePreparationError> for SkillAcquisitionAdapterError {
    fn from(source: SkillPackagePreparationError) -> Self {
        Self::Preparation {
            source: Box::new(source),
        }
    }
}

impl fmt::Debug for SkillAcquisitionAdapterError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SkillAcquisitionAdapterError")
            .field("code", &self.code())
            .field("reason", &"[redacted]")
            .finish()
    }
}

impl fmt::Display for SkillAcquisitionAdapterError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.reason().fmt(formatter)
    }
}

impl Error for SkillAcquisitionAdapterError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Preparation { source } => Some(source.as_ref()),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillInstallationPreparationIntent {
    Install {
        installation_id: SkillInstallationId,
    },
    Update {
        skill_id: SkillId,
        expected_revision: SkillInstallationRevision,
    },
}

impl SkillInstallationPreparationIntent {
    pub fn operation(&self) -> SkillInstallationOperation {
        match self {
            Self::Install { .. } => SkillInstallationOperation::Install,
            Self::Update { .. } => SkillInstallationOperation::Update,
        }
    }
}

/// Immutable identity-bound inspection request.
#[derive(Clone, PartialEq, Eq)]
pub struct SkillInstallationPreparationRequest {
    preparation_id: SkillPreparationId,
    intent: SkillInstallationPreparationIntent,
    source: SkillAcquisitionSource,
}

impl SkillInstallationPreparationRequest {
    pub fn install(
        preparation_id: SkillPreparationId,
        installation_id: SkillInstallationId,
        source: SkillAcquisitionSource,
    ) -> Self {
        Self {
            preparation_id,
            intent: SkillInstallationPreparationIntent::Install { installation_id },
            source,
        }
    }

    pub fn update(
        preparation_id: SkillPreparationId,
        skill_id: SkillId,
        expected_revision: SkillInstallationRevision,
        source: SkillAcquisitionSource,
    ) -> Self {
        Self {
            preparation_id,
            intent: SkillInstallationPreparationIntent::Update {
                skill_id,
                expected_revision,
            },
            source,
        }
    }

    pub fn preparation_id(&self) -> &SkillPreparationId {
        &self.preparation_id
    }

    pub fn intent(&self) -> &SkillInstallationPreparationIntent {
        &self.intent
    }

    pub fn source(&self) -> &SkillAcquisitionSource {
        &self.source
    }
}

impl fmt::Debug for SkillInstallationPreparationRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SkillInstallationPreparationRequest")
            .field("preparation_id", &self.preparation_id)
            .field("intent", &self.intent)
            .field("source", &self.source)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum SkillInstallationWarningCode {
    ContainsScripts,
    ResourcesNotExposed,
}

impl SkillInstallationWarningCode {
    pub fn stable_name(self) -> &'static str {
        match self {
            Self::ContainsScripts => "containsScripts",
            Self::ResourcesNotExposed => "resourcesNotExposed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillInstallationWarning {
    code: SkillInstallationWarningCode,
    message: String,
    acknowledgement_required: bool,
}

impl SkillInstallationWarning {
    pub fn code(&self) -> SkillInstallationWarningCode {
        self.code
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn acknowledgement_required(&self) -> bool {
        self.acknowledgement_required
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SkillPackageResourceSummary {
    resource_count: usize,
    resource_bytes: u64,
    reference_count: usize,
    asset_count: usize,
    script_count: usize,
}

impl SkillPackageResourceSummary {
    pub fn resource_count(&self) -> usize {
        self.resource_count
    }

    pub fn resource_bytes(&self) -> u64 {
        self.resource_bytes
    }

    pub fn reference_count(&self) -> usize {
        self.reference_count
    }

    pub fn asset_count(&self) -> usize {
        self.asset_count
    }

    pub fn script_count(&self) -> usize {
        self.script_count
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct SkillInstallationPackagePreview {
    name: String,
    description: String,
    revision: SkillRevision,
    format_version: u32,
    entrypoint_bytes: u64,
    resources: SkillPackageResourceSummary,
}

impl SkillInstallationPackagePreview {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn description(&self) -> &str {
        &self.description
    }

    pub fn revision(&self) -> &SkillRevision {
        &self.revision
    }

    pub fn format_version(&self) -> u32 {
        self.format_version
    }

    pub fn entrypoint_bytes(&self) -> u64 {
        self.entrypoint_bytes
    }

    pub fn resources(&self) -> &SkillPackageResourceSummary {
        &self.resources
    }

    pub fn package_bytes(&self) -> u64 {
        self.entrypoint_bytes
            .saturating_add(self.resources.resource_bytes)
    }
}

impl fmt::Debug for SkillInstallationPackagePreview {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SkillInstallationPackagePreview")
            .field("name", &self.name)
            .field("revision", &self.revision)
            .field("format_version", &self.format_version)
            .field("entrypoint_bytes", &self.entrypoint_bytes)
            .field("resources", &self.resources)
            .finish()
    }
}

/// Sanitized acquisition metadata persisted with the installed receipt.
///
/// Local-directory acquisition uses a fixed reference and therefore never
/// exposes the selected path. Network adapters must similarly exclude
/// credentials and may encode stable details such as a resolved commit.
#[derive(Clone, PartialEq, Eq)]
pub struct SkillAcquisitionPresentation {
    provider: String,
    reference: String,
}

const MAX_INSTALLED_SOURCE_DISPLAY_NAME_BYTES: usize = 256;

/// Credential-free, provider-validated receipt projection for management UI.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum InstalledSkillSourcePresentation {
    LocalDirectory,
    GitHub {
        owner: String,
        repository: String,
        tracking_reference: InstalledGitHubTrackingReference,
        resolved_commit: String,
        subdirectory: Option<String>,
        refreshable: bool,
    },
    Provider {
        provider: String,
        display_name: String,
        refreshable: bool,
    },
    Unknown {
        provider: String,
        schema_version: u32,
    },
}

impl InstalledSkillSourcePresentation {
    pub fn refreshable(&self) -> bool {
        matches!(
            self,
            Self::GitHub {
                refreshable: true,
                ..
            } | Self::Provider {
                refreshable: true,
                ..
            }
        )
    }

    pub fn provider(&self) -> &str {
        match self {
            Self::LocalDirectory => LOCAL_DIRECTORY_PROVIDER,
            Self::GitHub { .. } => "github",
            Self::Provider { provider, .. } => provider,
            Self::Unknown { provider, .. } => provider,
        }
    }

    fn has_valid_boundary_fields(&self) -> bool {
        match self {
            Self::Provider { display_name, .. } => {
                !display_name.trim().is_empty()
                    && display_name.len() <= MAX_INSTALLED_SOURCE_DISPLAY_NAME_BYTES
                    && !display_name.chars().any(char::is_control)
            }
            _ => true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum InstalledGitHubTrackingReference {
    DefaultBranch,
    Named(String),
    Commit,
}

impl SkillAcquisitionPresentation {
    pub fn provider(&self) -> &str {
        &self.provider
    }

    pub fn reference(&self) -> &str {
        &self.reference
    }
}

impl fmt::Debug for SkillAcquisitionPresentation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SkillAcquisitionPresentation")
            .field("provider", &self.provider)
            .field("reference", &"[redacted]")
            .field("reference_bytes", &self.reference.len())
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct SkillInstallationPreview {
    preparation_id: SkillPreparationId,
    preview_revision: SkillPreviewRevision,
    operation: SkillInstallationOperation,
    installation_id: SkillInstallationId,
    skill_id: SkillId,
    expected_revision: Option<SkillInstallationRevision>,
    content_changed: bool,
    source_changed: bool,
    acquisition: SkillAcquisitionPresentation,
    package: SkillInstallationPackagePreview,
    warnings: Arc<[SkillInstallationWarning]>,
    expires_at_unix_ms: u64,
}

impl SkillInstallationPreview {
    pub fn preparation_id(&self) -> &SkillPreparationId {
        &self.preparation_id
    }

    pub fn preview_revision(&self) -> &SkillPreviewRevision {
        &self.preview_revision
    }

    pub fn operation(&self) -> SkillInstallationOperation {
        self.operation
    }

    pub fn installation_id(&self) -> &SkillInstallationId {
        &self.installation_id
    }

    pub fn skill_id(&self) -> &SkillId {
        &self.skill_id
    }

    pub fn expected_revision(&self) -> Option<&SkillInstallationRevision> {
        self.expected_revision.as_ref()
    }

    pub fn content_changed(&self) -> bool {
        self.content_changed
    }

    pub fn source_changed(&self) -> bool {
        self.source_changed
    }

    pub fn acquisition(&self) -> &SkillAcquisitionPresentation {
        &self.acquisition
    }

    pub fn package(&self) -> &SkillInstallationPackagePreview {
        &self.package
    }

    pub fn warnings(&self) -> &[SkillInstallationWarning] {
        &self.warnings
    }

    pub fn expires_at_unix_ms(&self) -> u64 {
        self.expires_at_unix_ms
    }
}

impl fmt::Debug for SkillInstallationPreview {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SkillInstallationPreview")
            .field("preparation_id", &self.preparation_id)
            .field("preview_revision", &self.preview_revision)
            .field("operation", &self.operation)
            .field("installation_id", &self.installation_id)
            .field("skill_id", &self.skill_id)
            .field("expected_revision", &self.expected_revision)
            .field("content_changed", &self.content_changed)
            .field("source_changed", &self.source_changed)
            .field("acquisition", &self.acquisition)
            .field("package", &self.package)
            .field("warnings", &self.warnings)
            .field("expires_at_unix_ms", &self.expires_at_unix_ms)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillInstallationCommitRequest {
    preparation_id: SkillPreparationId,
    expected_preview_revision: SkillPreviewRevision,
    acknowledged_warnings: BTreeSet<SkillInstallationWarningCode>,
}

impl SkillInstallationCommitRequest {
    pub fn new(
        preparation_id: SkillPreparationId,
        expected_preview_revision: SkillPreviewRevision,
    ) -> Self {
        Self {
            preparation_id,
            expected_preview_revision,
            acknowledged_warnings: BTreeSet::new(),
        }
    }

    pub fn acknowledge(mut self, code: SkillInstallationWarningCode) -> Self {
        self.acknowledged_warnings.insert(code);
        self
    }

    pub fn preparation_id(&self) -> &SkillPreparationId {
        &self.preparation_id
    }

    pub fn expected_preview_revision(&self) -> &SkillPreviewRevision {
        &self.expected_preview_revision
    }

    pub fn acknowledged_warnings(&self) -> &BTreeSet<SkillInstallationWarningCode> {
        &self.acknowledged_warnings
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillInstallationCommitResult {
    preview: SkillInstallationPreview,
    mutation: SkillInstallationMutation,
    replayed: bool,
}

impl SkillInstallationCommitResult {
    pub fn preview(&self) -> &SkillInstallationPreview {
        &self.preview
    }

    pub fn mutation(&self) -> &SkillInstallationMutation {
        &self.mutation
    }

    pub fn replayed(&self) -> bool {
        self.replayed
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SkillPreparationCancellation {
    Cancelled,
    AlreadyCancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillInstallationWorkflowConfig {
    max_preparations: usize,
    max_snapshot_bytes: usize,
    preparation_ttl: Duration,
}

impl SkillInstallationWorkflowConfig {
    pub fn new(
        max_preparations: usize,
        max_snapshot_bytes: usize,
        preparation_ttl: Duration,
    ) -> Result<Self, SkillInstallationWorkflowConfigurationError> {
        if !(1..=MAX_REGISTRY_ENTRIES).contains(&max_preparations) {
            return Err(SkillInstallationWorkflowConfigurationError::new(format!(
                "max preparations must be between 1 and {MAX_REGISTRY_ENTRIES}",
            )));
        }
        if !(MAX_SKILL_PACKAGE_BYTES..=MAX_REGISTRY_SNAPSHOT_BYTES).contains(&max_snapshot_bytes) {
            return Err(SkillInstallationWorkflowConfigurationError::new(format!(
                "snapshot budget must be between {MAX_SKILL_PACKAGE_BYTES} and {MAX_REGISTRY_SNAPSHOT_BYTES} bytes",
            )));
        }
        if preparation_ttl < Duration::from_millis(1) || preparation_ttl > MAX_PREPARATION_TTL {
            return Err(SkillInstallationWorkflowConfigurationError::new(
                "preparation TTL must be at least one millisecond and no more than 24 hours",
            ));
        }
        Ok(Self {
            max_preparations,
            max_snapshot_bytes,
            preparation_ttl,
        })
    }

    pub fn max_preparations(&self) -> usize {
        self.max_preparations
    }

    pub fn max_snapshot_bytes(&self) -> usize {
        self.max_snapshot_bytes
    }

    pub fn preparation_ttl(&self) -> Duration {
        self.preparation_ttl
    }
}

impl Default for SkillInstallationWorkflowConfig {
    fn default() -> Self {
        Self {
            max_preparations: DEFAULT_MAX_SKILL_PREPARATIONS,
            max_snapshot_bytes: DEFAULT_MAX_SKILL_PREPARATION_BYTES,
            preparation_ttl: DEFAULT_SKILL_PREPARATION_TTL,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillInstallationWorkflowConfigurationError {
    reason: String,
}

impl SkillInstallationWorkflowConfigurationError {
    fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }

    pub fn reason(&self) -> &str {
        &self.reason
    }
}

impl fmt::Display for SkillInstallationWorkflowConfigurationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.reason.fmt(formatter)
    }
}

impl Error for SkillInstallationWorkflowConfigurationError {}

/// Stateful two-phase installation boundary intended for backend RPC use.
pub struct SkillInstallationWorkflow {
    installation_service: SkillInstallationService,
    adapters: BTreeMap<SkillAcquisitionProvider, Arc<dyn SkillAcquisitionAdapter>>,
    sessions: SkillInstallationSessionStore,
    config: SkillInstallationWorkflowConfig,
}

impl fmt::Debug for SkillInstallationWorkflow {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SkillInstallationWorkflow")
            .field(
                "adapter_providers",
                &self.adapters.keys().collect::<Vec<_>>(),
            )
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

/// Panic recovery for adapter callbacks. The dispatcher catches unwinds, so
/// the workflow must release the reservation before that unwind leaves the
/// worker or the idempotency key would remain permanently busy. The attempt
/// token makes a delayed guard harmless after the same key starts new work.
struct PreparingSlotRecovery {
    sessions: SkillInstallationSessionStore,
    preparation_id: SkillPreparationId,
    attempt_id: u64,
    armed: bool,
}

impl PreparingSlotRecovery {
    fn new(
        sessions: &SkillInstallationSessionStore,
        preparation_id: &SkillPreparationId,
        attempt_id: u64,
    ) -> Self {
        Self {
            sessions: sessions.clone(),
            preparation_id: preparation_id.clone(),
            attempt_id,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for PreparingSlotRecovery {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let recovered = self.sessions.lock().is_ok_and(|mut state| {
            if matches!(
                state.preparations.get(&self.preparation_id),
                Some(PreparationSlot::Preparing { attempt_id, .. })
                    if *attempt_id == self.attempt_id
            ) {
                state.preparations.remove(&self.preparation_id);
                true
            } else {
                false
            }
        });
        if recovered {
            self.sessions.notify_all();
        }
    }
}

/// Restores the exact frozen preview if a store call unwinds while commit
/// state is unknown. Normal completion explicitly disarms the guard before
/// notifying waiters; the attempt token is a second ownership check that
/// prevents a delayed guard from rolling a later commit attempt back.
struct CommittingSlotRecovery {
    sessions: SkillInstallationSessionStore,
    preparation_id: SkillPreparationId,
    attempt_id: u64,
    armed: bool,
    request: Option<SkillInstallationPreparationRequest>,
    preview: Option<SkillInstallationPreview>,
    acquisition: Option<PreparedSkillAcquisition>,
    snapshot_bytes: usize,
    expires_at: Duration,
}

impl CommittingSlotRecovery {
    fn new(
        sessions: &SkillInstallationSessionStore,
        attempt_id: u64,
        request: &SkillInstallationPreparationRequest,
        preview: &SkillInstallationPreview,
        acquisition: &PreparedSkillAcquisition,
        snapshot_bytes: usize,
        expires_at: Duration,
    ) -> Self {
        Self {
            sessions: sessions.clone(),
            preparation_id: request.preparation_id().clone(),
            attempt_id,
            armed: true,
            request: Some(request.clone()),
            preview: Some(preview.clone()),
            acquisition: Some(acquisition.clone()),
            snapshot_bytes,
            expires_at,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for CommittingSlotRecovery {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let recovered = self.sessions.lock().is_ok_and(|mut state| {
            let still_owns_slot = matches!(
                state.preparations.get(&self.preparation_id),
                Some(PreparationSlot::Committing { attempt_id, .. })
                    if *attempt_id == self.attempt_id
            );
            if !still_owns_slot {
                return false;
            }
            let (Some(request), Some(preview), Some(acquisition)) = (
                self.request.take(),
                self.preview.take(),
                self.acquisition.take(),
            ) else {
                return false;
            };
            state.preparations.insert(
                self.preparation_id.clone(),
                PreparationSlot::Ready {
                    request,
                    preview,
                    acquisition,
                    snapshot_bytes: self.snapshot_bytes,
                    expires_at: self.expires_at,
                },
            );
            true
        });
        if recovered {
            self.sessions.notify_all();
        }
    }
}

impl SkillInstallationWorkflow {
    pub fn new(installation_service: SkillInstallationService) -> Self {
        Self::with_config(
            installation_service,
            SkillInstallationWorkflowConfig::default(),
        )
    }

    pub fn with_config(
        installation_service: SkillInstallationService,
        config: SkillInstallationWorkflowConfig,
    ) -> Self {
        let sessions = SkillInstallationSessionStore::new(
            SkillInstallationSessionConfig::new(
                super::installation_session::DEFAULT_MAX_SKILL_SOURCE_RESOLUTIONS,
                super::installation_session::DEFAULT_MAX_SKILL_SOURCE_RESOLUTION_CANDIDATES,
                config.max_snapshot_bytes,
                super::installation_session::DEFAULT_SKILL_SOURCE_RESOLUTION_TTL,
            )
            .expect("workflow configuration must form a valid session configuration"),
        );
        Self::with_session_store(installation_service, config, sessions)
    }

    #[cfg(test)]
    fn with_clock(
        installation_service: SkillInstallationService,
        config: SkillInstallationWorkflowConfig,
        clock: Arc<dyn SessionClock>,
    ) -> Self {
        let session_config = SkillInstallationSessionConfig::new(
            super::installation_session::DEFAULT_MAX_SKILL_SOURCE_RESOLUTIONS,
            super::installation_session::DEFAULT_MAX_SKILL_SOURCE_RESOLUTION_CANDIDATES,
            config.max_snapshot_bytes,
            super::installation_session::DEFAULT_SKILL_SOURCE_RESOLUTION_TTL,
        )
        .expect("workflow configuration must form a valid session configuration");
        let sessions = SkillInstallationSessionStore::with_clock(session_config, clock);
        Self::with_session_store(installation_service, config, sessions)
    }

    pub fn with_session_store(
        installation_service: SkillInstallationService,
        config: SkillInstallationWorkflowConfig,
        sessions: SkillInstallationSessionStore,
    ) -> Self {
        let mut adapters: BTreeMap<SkillAcquisitionProvider, Arc<dyn SkillAcquisitionAdapter>> =
            BTreeMap::new();
        let local: Arc<dyn SkillAcquisitionAdapter> = Arc::new(LocalDirectoryAcquisitionAdapter);
        adapters.insert(local.provider(), local);
        Self {
            installation_service,
            adapters,
            sessions,
            config,
        }
    }

    pub fn session_store(&self) -> SkillInstallationSessionStore {
        self.sessions.clone()
    }

    /// Returns whether this workflow has an adapter that can safely decode the
    /// receipt's exact refresh provider and schema. Payload bytes remain
    /// private to the selected adapter.
    pub fn can_refresh(&self, provenance: &SkillInstallationProvenance) -> bool {
        self.installed_source_presentation(provenance).refreshable()
    }

    /// Returns a provider-validated, payload-free source projection. Unknown
    /// providers or semantically invalid provenance fail closed.
    pub fn installed_source_presentation(
        &self,
        provenance: &SkillInstallationProvenance,
    ) -> InstalledSkillSourcePresentation {
        let authority_provider = provenance.authority().provider();
        if provenance
            .refresh()
            .is_some_and(|refresh| refresh.provider() != authority_provider)
        {
            return InstalledSkillSourcePresentation::Unknown {
                provider: authority_provider.to_string(),
                schema_version: provenance.authority().schema_version(),
            };
        }
        let refresh_capable = provenance.refresh().is_some_and(|refresh| {
            self.adapter_for_refresh(refresh).is_ok_and(|(_, adapter)| {
                adapter
                    .refresh_schema_versions()
                    .contains(&refresh.schema_version())
            })
        });
        let presentation = self
            .adapters
            .iter()
            .find_map(|(provider, adapter)| {
                (provider.as_str() == authority_provider).then(|| {
                    adapter
                        .installed_source_presentation(provenance.adapter_view(), refresh_capable)
                })
            })
            .flatten();
        match presentation {
            Some(presentation)
                if presentation.provider() == authority_provider
                    && (!presentation.refreshable() || refresh_capable)
                    && presentation.has_valid_boundary_fields() =>
            {
                presentation
            }
            _ => InstalledSkillSourcePresentation::Unknown {
                provider: authority_provider.to_string(),
                schema_version: provenance.authority().schema_version(),
            },
        }
    }

    /// Registers one additional acquisition provider before the workflow is
    /// shared. Provider replacement is rejected so dispatch cannot silently
    /// change after requests have been prepared.
    pub fn register_adapter(
        &mut self,
        adapter: Arc<dyn SkillAcquisitionAdapter>,
    ) -> Result<(), SkillInstallationWorkflowConfigurationError> {
        let provider = adapter.provider();
        if self.adapters.contains_key(&provider) {
            return Err(SkillInstallationWorkflowConfigurationError::new(format!(
                "acquisition provider `{provider}` is already registered",
            )));
        }
        self.adapters.insert(provider, adapter);
        Ok(())
    }

    pub fn inspect_local_directory_install(
        &self,
        preparation_id: SkillPreparationId,
        installation_id: SkillInstallationId,
        directory: impl Into<PathBuf>,
    ) -> Result<SkillInstallationPreview, SkillInstallationWorkflowError> {
        self.inspect(&SkillInstallationPreparationRequest::install(
            preparation_id,
            installation_id,
            SkillAcquisitionSource::local_directory(directory),
        ))
    }

    pub fn inspect_local_directory_update(
        &self,
        preparation_id: SkillPreparationId,
        skill_id: SkillId,
        expected_revision: SkillInstallationRevision,
        directory: impl Into<PathBuf>,
    ) -> Result<SkillInstallationPreview, SkillInstallationWorkflowError> {
        self.inspect(&SkillInstallationPreparationRequest::update(
            preparation_id,
            skill_id,
            expected_revision,
            SkillAcquisitionSource::local_directory(directory),
        ))
    }

    /// Acquires and validates an exact snapshot, returning the same preview
    /// for a retry with the same preparation ID and byte-for-byte request.
    pub fn inspect(
        &self,
        request: &SkillInstallationPreparationRequest,
    ) -> Result<SkillInstallationPreview, SkillInstallationWorkflowError> {
        let action = PreparedAction::from_intent(&request.intent)?;
        if let Some(preview) = self.existing_preparation(request)? {
            return Ok(preview);
        }
        let current = self.preflight_action(&action)?;
        if let SkillAcquisitionSource::ResolvedCandidate {
            resolution_id,
            candidate_id,
        } = &request.source
        {
            return self.inspect_resolved_candidate(
                request,
                &action,
                current.as_ref(),
                resolution_id,
                candidate_id,
            );
        }
        if matches!(request.source, SkillAcquisitionSource::InstalledSource) {
            return self.inspect_installed_source(request, &action, current.as_ref());
        }
        let provider = request.source.provider();
        let adapter = self.adapters.get(&provider).ok_or_else(|| {
            SkillInstallationWorkflowError::UnknownAcquisitionProvider {
                provider: provider.clone(),
            }
        })?;

        let preparation_attempt_id = loop {
            let now = self.sessions.now();
            let mut registry = self.lock_registry("inspect Skill preparation")?;
            registry.prune_expired(now.monotonic);
            match registry.preparations.get(request.preparation_id()) {
                Some(slot) if slot.request() != request => {
                    return Err(SkillInstallationWorkflowError::PreparationConflict {
                        preparation_id: request.preparation_id.clone(),
                    });
                }
                Some(PreparationSlot::Preparing { .. })
                | Some(PreparationSlot::Committing { .. }) => {
                    drop(self.wait_for_change(registry, "wait for Skill preparation")?);
                    continue;
                }
                Some(PreparationSlot::Ready { preview, .. })
                | Some(PreparationSlot::Committed { preview, .. }) => {
                    return Ok(preview.clone());
                }
                Some(PreparationSlot::Cancelled { .. }) => {
                    return Err(SkillInstallationWorkflowError::PreparationCancelled {
                        preparation_id: request.preparation_id.clone(),
                    });
                }
                None => {
                    break self.reserve_preparation(
                        &mut registry,
                        request.clone(),
                        now.monotonic,
                    )?;
                }
            }
        };

        let mut preparing_recovery = PreparingSlotRecovery::new(
            &self.sessions,
            request.preparation_id(),
            preparation_attempt_id,
        );
        let acquired = adapter.acquire(&request.source).and_then(|acquisition| {
            if acquisition.is_owned_by(provider.as_str()) {
                Ok(acquisition)
            } else {
                Err(invalid_acquisition_provider_output())
            }
        });
        let now = self.sessions.now();
        let mut registry = self.lock_registry("finish Skill preparation")?;
        let result = match acquired {
            Ok(acquisition) => {
                let snapshot_bytes = acquisition_snapshot_bytes(&acquisition);
                if let Err(error) =
                    self.ensure_finished_preparation_capacity(&registry, snapshot_bytes)
                {
                    registry.preparations.remove(request.preparation_id());
                    preparing_recovery.disarm();
                    self.sessions.notify_all();
                    return Err(error);
                }
                let preview = build_preview(
                    request,
                    &action,
                    &acquisition,
                    current.as_ref(),
                    now.unix_ms
                        .saturating_add(duration_millis(self.config.preparation_ttl)),
                );
                registry.preparations.insert(
                    request.preparation_id.clone(),
                    PreparationSlot::Ready {
                        request: request.clone(),
                        preview: preview.clone(),
                        snapshot_bytes,
                        acquisition,
                        expires_at: now.monotonic.saturating_add(self.config.preparation_ttl),
                    },
                );
                Ok(preview)
            }
            Err(source) => {
                registry.preparations.remove(request.preparation_id());
                Err(SkillInstallationWorkflowError::Acquisition {
                    provider,
                    source: Box::new(source),
                })
            }
        };
        preparing_recovery.disarm();
        self.sessions.notify_all();
        result
    }

    fn inspect_resolved_candidate(
        &self,
        request: &SkillInstallationPreparationRequest,
        action: &PreparedAction,
        current: Option<&InstalledSkillRecord>,
        resolution_id: &SkillSourceResolutionId,
        candidate_id: &SkillSourceCandidateId,
    ) -> Result<SkillInstallationPreview, SkillInstallationWorkflowError> {
        loop {
            let now = self.sessions.now();
            let mut state = self.lock_registry("inspect resolved Skill candidate")?;
            state.prune_expired(now.monotonic);
            match state.preparations.get(request.preparation_id()) {
                Some(slot) if slot.request() != request => {
                    return Err(SkillInstallationWorkflowError::PreparationConflict {
                        preparation_id: request.preparation_id.clone(),
                    });
                }
                Some(PreparationSlot::Preparing { .. })
                | Some(PreparationSlot::Committing { .. }) => {
                    drop(self.wait_for_change(state, "wait for resolved Skill preparation")?);
                    continue;
                }
                Some(PreparationSlot::Ready { preview, .. })
                | Some(PreparationSlot::Committed { preview, .. }) => {
                    return Ok(preview.clone());
                }
                Some(PreparationSlot::Cancelled { .. }) => {
                    return Err(SkillInstallationWorkflowError::PreparationCancelled {
                        preparation_id: request.preparation_id.clone(),
                    });
                }
                None => {}
            }

            if state.preparations.len() >= self.config.max_preparations {
                return Err(
                    SkillInstallationWorkflowError::PreparationCapacityExceeded {
                        max_preparations: self.config.max_preparations,
                    },
                );
            }

            let Some(slot) = state.resolutions.remove(resolution_id) else {
                return Err(
                    SkillInstallationWorkflowError::SourceResolutionNotFoundOrExpired {
                        resolution_id: resolution_id.clone(),
                    },
                );
            };
            match slot {
                ResolutionSlot::Ready {
                    locator,
                    resolution,
                    mut candidates,
                    snapshot_bytes,
                    expires_at: resolution_expires_at,
                } => {
                    let Some(acquisition) = candidates.remove(candidate_id) else {
                        state.resolutions.insert(
                            resolution_id.clone(),
                            ResolutionSlot::Ready {
                                locator,
                                resolution,
                                candidates,
                                snapshot_bytes,
                                expires_at: resolution_expires_at,
                            },
                        );
                        return Err(SkillInstallationWorkflowError::SourceCandidateNotFound {
                            resolution_id: resolution_id.clone(),
                            candidate_id: candidate_id.clone(),
                        });
                    };
                    let expires_at_unix_ms = now
                        .unix_ms
                        .saturating_add(duration_millis(self.config.preparation_ttl));
                    let preview =
                        build_preview(request, action, &acquisition, current, expires_at_unix_ms);
                    let snapshot_bytes = acquisition_snapshot_bytes(&acquisition);
                    state.preparations.insert(
                        request.preparation_id.clone(),
                        PreparationSlot::Ready {
                            request: request.clone(),
                            preview: preview.clone(),
                            acquisition,
                            snapshot_bytes,
                            expires_at: now.monotonic.saturating_add(self.config.preparation_ttl),
                        },
                    );
                    state.resolutions.insert(
                        resolution_id.clone(),
                        ResolutionSlot::Consumed {
                            locator,
                            candidate_id: candidate_id.clone(),
                            preparation_id: request.preparation_id.clone(),
                            expires_at: resolution_expires_at,
                        },
                    );
                    self.sessions.notify_all();
                    return Ok(preview);
                }
                ResolutionSlot::Resolving {
                    locator,
                    attempt_id,
                    reserved_bytes,
                    expires_at,
                } => {
                    state.resolutions.insert(
                        resolution_id.clone(),
                        ResolutionSlot::Resolving {
                            locator,
                            attempt_id,
                            reserved_bytes,
                            expires_at,
                        },
                    );
                    return Err(SkillInstallationWorkflowError::SourceResolutionBusy {
                        resolution_id: resolution_id.clone(),
                    });
                }
                ResolutionSlot::Consumed {
                    locator,
                    candidate_id: consumed_candidate_id,
                    preparation_id,
                    expires_at,
                } => {
                    state.resolutions.insert(
                        resolution_id.clone(),
                        ResolutionSlot::Consumed {
                            locator,
                            candidate_id: consumed_candidate_id,
                            preparation_id,
                            expires_at,
                        },
                    );
                    return Err(SkillInstallationWorkflowError::SourceResolutionConsumed {
                        resolution_id: resolution_id.clone(),
                    });
                }
                ResolutionSlot::Cancelled {
                    locator,
                    expires_at,
                } => {
                    state.resolutions.insert(
                        resolution_id.clone(),
                        ResolutionSlot::Cancelled {
                            locator,
                            expires_at,
                        },
                    );
                    return Err(SkillInstallationWorkflowError::SourceResolutionCancelled {
                        resolution_id: resolution_id.clone(),
                    });
                }
            }
        }
    }

    fn inspect_installed_source(
        &self,
        request: &SkillInstallationPreparationRequest,
        action: &PreparedAction,
        current: Option<&InstalledSkillRecord>,
    ) -> Result<SkillInstallationPreview, SkillInstallationWorkflowError> {
        let installation_id = match action {
            PreparedAction::Update {
                installation_id, ..
            } => installation_id,
            PreparedAction::Install { .. } => {
                return Err(SkillInstallationWorkflowError::InstalledSourceRequiresUpdate)
            }
        };

        let preparation_attempt_id = loop {
            let now = self.sessions.now();
            let mut state = self.lock_registry("inspect installed Skill source")?;
            state.prune_expired(now.monotonic);
            match state.preparations.get(request.preparation_id()) {
                Some(slot) if slot.request() != request => {
                    return Err(SkillInstallationWorkflowError::PreparationConflict {
                        preparation_id: request.preparation_id.clone(),
                    });
                }
                Some(PreparationSlot::Preparing { .. })
                | Some(PreparationSlot::Committing { .. }) => {
                    drop(self.wait_for_change(state, "wait for installed Skill refresh")?);
                    continue;
                }
                Some(PreparationSlot::Ready { preview, .. })
                | Some(PreparationSlot::Committed { preview, .. }) => {
                    return Ok(preview.clone());
                }
                Some(PreparationSlot::Cancelled { .. }) => {
                    return Err(SkillInstallationWorkflowError::PreparationCancelled {
                        preparation_id: request.preparation_id.clone(),
                    });
                }
                None => {
                    break self.reserve_preparation(&mut state, request.clone(), now.monotonic)?;
                }
            }
        };

        let mut preparing_recovery = PreparingSlotRecovery::new(
            &self.sessions,
            request.preparation_id(),
            preparation_attempt_id,
        );
        // Receipt read and lifecycle CAS validation intentionally precede all
        // adapter/network work. Commit repeats the same CAS in the installer.
        let acquired = (|| {
            let record = current.expect("update preflight must return an installed record");
            if record.is_legacy() {
                return Err(SkillInstallationWorkflowError::InstalledSourceLegacy {
                    installation_id: installation_id.clone(),
                });
            }
            let refresh = record.provenance().refresh().ok_or_else(|| {
                SkillInstallationWorkflowError::InstalledSourceNotRefreshable {
                    installation_id: installation_id.clone(),
                }
            })?;
            let authority_provider = record.provenance().authority().provider();
            if refresh.provider() != authority_provider {
                return Err(
                    SkillInstallationWorkflowError::InvalidInstalledSourceProvenance {
                        provider: authority_provider.to_string(),
                    },
                );
            }
            let (provider, adapter) = self.adapter_for_refresh(refresh)?;
            if !adapter
                .refresh_schema_versions()
                .contains(&refresh.schema_version())
            {
                return Err(SkillInstallationWorkflowError::UnsupportedRefreshSchema {
                    provider: refresh.provider().to_string(),
                    schema_version: refresh.schema_version(),
                });
            }
            if matches!(
                self.installed_source_presentation(record.provenance()),
                InstalledSkillSourcePresentation::Unknown { .. }
            ) {
                return Err(
                    SkillInstallationWorkflowError::InvalidInstalledSourceProvenance {
                        provider: record.provenance().authority().provider().to_string(),
                    },
                );
            }
            adapter
                .reacquire(refresh.adapter_view())
                .and_then(|acquisition| {
                    if acquisition.is_owned_by(provider.as_str()) {
                        Ok(acquisition)
                    } else {
                        Err(invalid_acquisition_provider_output())
                    }
                })
                .map_err(|source| SkillInstallationWorkflowError::Acquisition {
                    provider: provider.clone(),
                    source: Box::new(source),
                })
        })();

        let now = self.sessions.now();
        let mut state = self.lock_registry("finish installed Skill refresh")?;
        let result = match acquired {
            Ok(acquisition) => {
                let snapshot_bytes = acquisition_snapshot_bytes(&acquisition);
                if let Err(error) =
                    self.ensure_finished_preparation_capacity(&state, snapshot_bytes)
                {
                    state.preparations.remove(request.preparation_id());
                    preparing_recovery.disarm();
                    self.sessions.notify_all();
                    return Err(error);
                }
                let preview = build_preview(
                    request,
                    action,
                    &acquisition,
                    current,
                    now.unix_ms
                        .saturating_add(duration_millis(self.config.preparation_ttl)),
                );
                state.preparations.insert(
                    request.preparation_id.clone(),
                    PreparationSlot::Ready {
                        request: request.clone(),
                        preview: preview.clone(),
                        snapshot_bytes,
                        acquisition,
                        expires_at: now.monotonic.saturating_add(self.config.preparation_ttl),
                    },
                );
                Ok(preview)
            }
            Err(error) => {
                state.preparations.remove(request.preparation_id());
                Err(error)
            }
        };
        preparing_recovery.disarm();
        self.sessions.notify_all();
        result
    }

    fn adapter_for_refresh(
        &self,
        refresh: &SkillInstallationRefresh,
    ) -> Result<
        (&SkillAcquisitionProvider, &Arc<dyn SkillAcquisitionAdapter>),
        SkillInstallationWorkflowError,
    > {
        self.adapters
            .iter()
            .find_map(|(provider, adapter)| {
                (provider.as_str() == refresh.provider()).then_some((provider, adapter))
            })
            .ok_or_else(|| SkillInstallationWorkflowError::UnknownRefreshProvider {
                provider: refresh.provider().to_string(),
            })
    }

    fn existing_preparation(
        &self,
        request: &SkillInstallationPreparationRequest,
    ) -> Result<Option<SkillInstallationPreview>, SkillInstallationWorkflowError> {
        loop {
            let now = self.sessions.now();
            let mut state = self.lock_registry("check existing Skill preparation")?;
            state.prune_expired(now.monotonic);
            match state.preparations.get(request.preparation_id()) {
                Some(slot) if slot.request() != request => {
                    return Err(SkillInstallationWorkflowError::PreparationConflict {
                        preparation_id: request.preparation_id.clone(),
                    });
                }
                Some(PreparationSlot::Preparing { .. })
                | Some(PreparationSlot::Committing { .. }) => {
                    drop(self.wait_for_change(state, "wait for existing Skill preparation")?);
                }
                Some(PreparationSlot::Ready { preview, .. })
                | Some(PreparationSlot::Committed { preview, .. }) => {
                    return Ok(Some(preview.clone()));
                }
                Some(PreparationSlot::Cancelled { .. }) => {
                    return Err(SkillInstallationWorkflowError::PreparationCancelled {
                        preparation_id: request.preparation_id.clone(),
                    });
                }
                None => return Ok(None),
            }
        }
    }

    fn preflight_action(
        &self,
        action: &PreparedAction,
    ) -> Result<Option<InstalledSkillRecord>, SkillInstallationWorkflowError> {
        let PreparedAction::Update {
            installation_id,
            expected_revision,
            ..
        } = action
        else {
            return Ok(None);
        };
        let record = self
            .installation_service
            .read_installed_skill(installation_id)
            .map_err(|error| SkillInstallationWorkflowError::InstalledSkillRead {
                installation_id: installation_id.clone(),
                reason: error.to_string(),
            })?
            .ok_or_else(|| SkillInstallationWorkflowError::InstalledSkillNotFound {
                installation_id: installation_id.clone(),
            })?;
        if record.installation_revision() != expected_revision {
            return Err(
                SkillInstallationWorkflowError::InstalledSourceRevisionConflict {
                    installation_id: installation_id.clone(),
                    expected_revision: expected_revision.clone(),
                    actual_revision: record.installation_revision().clone(),
                },
            );
        }
        Ok(Some(record))
    }

    fn reserve_preparation(
        &self,
        state: &mut InstallationSessionState,
        request: SkillInstallationPreparationRequest,
        now: Duration,
    ) -> Result<u64, SkillInstallationWorkflowError> {
        if state.preparations.len() >= self.config.max_preparations {
            return Err(
                SkillInstallationWorkflowError::PreparationCapacityExceeded {
                    max_preparations: self.config.max_preparations,
                },
            );
        }
        let max_snapshot_bytes = self
            .config
            .max_snapshot_bytes
            .min(self.sessions.config().max_snapshot_bytes());
        if state
            .reserved_snapshot_bytes()
            .saturating_add(MAX_SKILL_PACKAGE_BYTES)
            > max_snapshot_bytes
        {
            return Err(
                SkillInstallationWorkflowError::PreparationMemoryCapacityExceeded {
                    max_snapshot_bytes,
                },
            );
        }
        let attempt_id = state.next_preparation_attempt();
        state.preparations.insert(
            request.preparation_id.clone(),
            PreparationSlot::Preparing {
                request,
                attempt_id,
                _started_at: now,
            },
        );
        Ok(attempt_id)
    }

    fn ensure_finished_preparation_capacity(
        &self,
        state: &InstallationSessionState,
        snapshot_bytes: usize,
    ) -> Result<(), SkillInstallationWorkflowError> {
        let max_snapshot_bytes = self
            .config
            .max_snapshot_bytes
            .min(self.sessions.config().max_snapshot_bytes());
        let resident_without_reservation = state
            .reserved_snapshot_bytes()
            .saturating_sub(MAX_SKILL_PACKAGE_BYTES);
        if resident_without_reservation.saturating_add(snapshot_bytes) > max_snapshot_bytes {
            Err(
                SkillInstallationWorkflowError::PreparationMemoryCapacityExceeded {
                    max_snapshot_bytes,
                },
            )
        } else {
            Ok(())
        }
    }

    /// Commits the exact package captured by `inspect`. Required warnings
    /// must be acknowledged before any store mutation starts.
    pub fn commit(
        &self,
        request: &SkillInstallationCommitRequest,
    ) -> Result<SkillInstallationCommitResult, SkillInstallationWorkflowError> {
        let (
            preparation_request,
            preview,
            acquisition,
            action,
            snapshot_bytes,
            original_expires_at,
            commit_attempt_id,
        ) = loop {
            let now = self.sessions.now();
            let mut registry = self.lock_registry("commit Skill preparation")?;
            registry.prune_expired(now.monotonic);
            let Some(slot) = registry.preparations.get(request.preparation_id()) else {
                return Err(
                    SkillInstallationWorkflowError::PreparationNotFoundOrExpired {
                        preparation_id: request.preparation_id.clone(),
                    },
                );
            };
            match slot {
                PreparationSlot::Preparing { .. } | PreparationSlot::Committing { .. } => {
                    drop(self.wait_for_change(registry, "wait to commit Skill preparation")?);
                    continue;
                }
                PreparationSlot::Cancelled { .. } => {
                    return Err(SkillInstallationWorkflowError::PreparationCancelled {
                        preparation_id: request.preparation_id.clone(),
                    });
                }
                PreparationSlot::Committed {
                    mutation, preview, ..
                } => {
                    ensure_preview_matches(preview, request)?;
                    ensure_warnings_acknowledged(preview, request)?;
                    return Ok(SkillInstallationCommitResult {
                        preview: preview.clone(),
                        mutation: mutation.clone(),
                        replayed: true,
                    });
                }
                PreparationSlot::Ready {
                    request: preparation_request,
                    preview,
                    acquisition,
                    snapshot_bytes,
                    expires_at,
                } => {
                    ensure_preview_matches(preview, request)?;
                    ensure_warnings_acknowledged(preview, request)?;
                    let frozen = (
                        preparation_request.clone(),
                        preview.clone(),
                        acquisition.clone(),
                        PreparedAction::from_intent(&preparation_request.intent)?,
                        *snapshot_bytes,
                        *expires_at,
                    );
                    let commit_attempt_id = registry.next_preparation_attempt();
                    registry.preparations.insert(
                        request.preparation_id.clone(),
                        PreparationSlot::Committing {
                            request: frozen.0.clone(),
                            attempt_id: commit_attempt_id,
                            snapshot_bytes: frozen.4,
                        },
                    );
                    break (
                        frozen.0,
                        frozen.1,
                        frozen.2,
                        frozen.3,
                        frozen.4,
                        frozen.5,
                        commit_attempt_id,
                    );
                }
            }
        };

        let mut committing_recovery = CommittingSlotRecovery::new(
            &self.sessions,
            commit_attempt_id,
            &preparation_request,
            &preview,
            &acquisition,
            snapshot_bytes,
            original_expires_at,
        );
        let (package, provenance) = acquisition.clone().into_parts();
        let committed = match action {
            PreparedAction::Install {
                installation_id, ..
            } => self.installation_service.install_prepared_with_provenance(
                installation_id,
                package,
                provenance,
            ),
            PreparedAction::Update {
                installation_id,
                expected_revision,
                ..
            } => self.installation_service.update_prepared_exact(
                installation_id,
                expected_revision,
                package,
                provenance,
            ),
        };

        let now = self.sessions.now();
        let mut registry = match self.lock_registry("finish Skill commit") {
            Ok(registry) => registry,
            Err(_) => {
                // The store mutation is authoritative. Losing the in-memory
                // replay cache must never turn a known commit result into an
                // unrelated Internal error or hide commit-indeterminate
                // semantics from the caller.
                committing_recovery.disarm();
                self.sessions.notify_all();
                return match committed {
                    Ok(mutation) => Ok(SkillInstallationCommitResult {
                        preview,
                        mutation,
                        replayed: false,
                    }),
                    Err(source) => Err(SkillInstallationWorkflowError::Installation {
                        preparation_id: request.preparation_id.clone(),
                        source: Box::new(source),
                    }),
                };
            }
        };
        let result = match committed {
            Ok(mutation) => {
                let result_preview = preview.clone();
                registry.preparations.insert(
                    request.preparation_id.clone(),
                    PreparationSlot::Committed {
                        request: preparation_request,
                        preview,
                        mutation: mutation.clone(),
                        expires_at: now.monotonic.saturating_add(self.config.preparation_ttl),
                    },
                );
                Ok(SkillInstallationCommitResult {
                    preview: result_preview,
                    mutation,
                    replayed: false,
                })
            }
            Err(source) => {
                registry.preparations.insert(
                    request.preparation_id.clone(),
                    PreparationSlot::Ready {
                        request: preparation_request,
                        preview,
                        snapshot_bytes,
                        acquisition,
                        expires_at: original_expires_at,
                    },
                );
                Err(SkillInstallationWorkflowError::Installation {
                    preparation_id: request.preparation_id.clone(),
                    source: Box::new(source),
                })
            }
        };
        committing_recovery.disarm();
        self.sessions.notify_all();
        result
    }

    pub fn cancel(
        &self,
        preparation_id: &SkillPreparationId,
    ) -> Result<SkillPreparationCancellation, SkillInstallationWorkflowError> {
        let now = self.sessions.now();
        let mut registry = self.lock_registry("cancel Skill preparation")?;
        registry.prune_expired(now.monotonic);
        let Some(slot) = registry.preparations.get(preparation_id) else {
            return Err(
                SkillInstallationWorkflowError::PreparationNotFoundOrExpired {
                    preparation_id: preparation_id.clone(),
                },
            );
        };
        match slot {
            PreparationSlot::Preparing { .. } | PreparationSlot::Committing { .. } => {
                Err(SkillInstallationWorkflowError::PreparationBusy {
                    preparation_id: preparation_id.clone(),
                })
            }
            PreparationSlot::Committed { .. } => Err(
                SkillInstallationWorkflowError::PreparationAlreadyCommitted {
                    preparation_id: preparation_id.clone(),
                },
            ),
            PreparationSlot::Cancelled { .. } => Ok(SkillPreparationCancellation::AlreadyCancelled),
            PreparationSlot::Ready { request, .. } => {
                let request = request.clone();
                registry.preparations.insert(
                    preparation_id.clone(),
                    PreparationSlot::Cancelled {
                        request,
                        expires_at: now.monotonic.saturating_add(self.config.preparation_ttl),
                    },
                );
                self.sessions.notify_all();
                Ok(SkillPreparationCancellation::Cancelled)
            }
        }
    }

    fn lock_registry(
        &self,
        operation: &'static str,
    ) -> Result<MutexGuard<'_, InstallationSessionState>, SkillInstallationWorkflowError> {
        self.sessions
            .lock()
            .map_err(|_| SkillInstallationWorkflowError::Internal {
                operation,
                reason: "installation session registry lock is poisoned".to_string(),
            })
    }

    fn wait_for_change<'a>(
        &self,
        registry: MutexGuard<'a, InstallationSessionState>,
        operation: &'static str,
    ) -> Result<MutexGuard<'a, InstallationSessionState>, SkillInstallationWorkflowError> {
        self.sessions
            .wait(registry)
            .map_err(|_| SkillInstallationWorkflowError::Internal {
                operation,
                reason: "preparation registry lock is poisoned while waiting".to_string(),
            })
    }
}

struct LocalDirectoryAcquisitionAdapter;

impl SkillAcquisitionAdapter for LocalDirectoryAcquisitionAdapter {
    fn provider(&self) -> SkillAcquisitionProvider {
        SkillAcquisitionProvider::local_directory()
    }

    fn acquire(
        &self,
        source: &SkillAcquisitionSource,
    ) -> Result<PreparedSkillAcquisition, SkillAcquisitionAdapterError> {
        let directory = source.local_directory_path().ok_or_else(|| {
            SkillAcquisitionAdapterError::invalid_request(
                "local-directory provider received a different source envelope",
            )
        })?;
        let package =
            PreparedSkillPackage::from_local_directory(directory, LOCAL_DIRECTORY_ORIGIN_REFERENCE)
                .map_err(SkillAcquisitionAdapterError::from)?;
        let authority = SkillInstallationAuthority::new(
            LOCAL_DIRECTORY_PROVIDER,
            1,
            LOCAL_DIRECTORY_PROVENANCE_AUTHORITY,
        )
        .map_err(|_| {
            SkillAcquisitionAdapterError::unavailable(
                "the built-in local-directory provenance contract is invalid",
            )
        })?;
        Ok(PreparedSkillAcquisition::new(
            package,
            SkillInstallationProvenance::new(authority, None),
        ))
    }

    fn installed_source_presentation(
        &self,
        provenance: SkillInstallationProvenanceView<'_>,
        _refresh_capable: bool,
    ) -> Option<InstalledSkillSourcePresentation> {
        let authority = provenance.authority();
        (authority.provider() == LOCAL_DIRECTORY_PROVIDER
            && authority.schema_version() == 1
            && authority.payload() == LOCAL_DIRECTORY_PROVENANCE_AUTHORITY
            && provenance.refresh().is_none())
        .then_some(InstalledSkillSourcePresentation::LocalDirectory)
    }
}

#[derive(Clone)]
enum PreparedAction {
    Install {
        installation_id: SkillInstallationId,
        skill_id: SkillId,
    },
    Update {
        installation_id: SkillInstallationId,
        skill_id: SkillId,
        expected_revision: SkillInstallationRevision,
    },
}

impl PreparedAction {
    fn from_intent(
        intent: &SkillInstallationPreparationIntent,
    ) -> Result<Self, SkillInstallationWorkflowError> {
        let installed_source = SkillSourceId::parse(USER_INSTALLED_SKILL_SOURCE_ID)
            .expect("the built-in installed Skill source id must remain valid");
        match intent {
            SkillInstallationPreparationIntent::Install { installation_id } => {
                let skill_id = SkillId::from_parts(installed_source, installation_id.as_str())
                    .expect("a canonical installation id must form a valid Skill id");
                Ok(Self::Install {
                    installation_id: installation_id.clone(),
                    skill_id,
                })
            }
            SkillInstallationPreparationIntent::Update {
                skill_id,
                expected_revision,
            } => {
                if skill_id.source_id() != &installed_source {
                    return Err(SkillInstallationWorkflowError::InvalidUpdateTarget {
                        skill_id: skill_id.clone(),
                        reason: format!(
                            "Skill does not belong to the user-installed source `{installed_source}`",
                        ),
                    });
                }
                let installation_id =
                    SkillInstallationId::parse(skill_id.local_id()).map_err(|error| {
                        SkillInstallationWorkflowError::InvalidUpdateTarget {
                            skill_id: skill_id.clone(),
                            reason: format!("invalid installed Skill identity: {error}"),
                        }
                    })?;
                Ok(Self::Update {
                    installation_id,
                    skill_id: skill_id.clone(),
                    expected_revision: expected_revision.clone(),
                })
            }
        }
    }

    fn operation(&self) -> SkillInstallationOperation {
        match self {
            Self::Install { .. } => SkillInstallationOperation::Install,
            Self::Update { .. } => SkillInstallationOperation::Update,
        }
    }

    fn installation_id(&self) -> &SkillInstallationId {
        match self {
            Self::Install {
                installation_id, ..
            }
            | Self::Update {
                installation_id, ..
            } => installation_id,
        }
    }

    fn skill_id(&self) -> &SkillId {
        match self {
            Self::Install { skill_id, .. } | Self::Update { skill_id, .. } => skill_id,
        }
    }

    fn expected_revision(&self) -> Option<&SkillInstallationRevision> {
        match self {
            Self::Install { .. } => None,
            Self::Update {
                expected_revision, ..
            } => Some(expected_revision),
        }
    }
}

fn build_preview(
    request: &SkillInstallationPreparationRequest,
    action: &PreparedAction,
    acquisition: &PreparedSkillAcquisition,
    current: Option<&InstalledSkillRecord>,
    expires_at_unix_ms: u64,
) -> SkillInstallationPreview {
    let package = acquisition.package();
    let index = package.resource_index();
    let mut resources = SkillPackageResourceSummary {
        resource_count: index.len(),
        ..SkillPackageResourceSummary::default()
    };
    for resource in index.entries() {
        resources.resource_bytes = resources
            .resource_bytes
            .saturating_add(resource.byte_length());
        match resource.kind() {
            SkillResourceKind::Reference => resources.reference_count += 1,
            SkillResourceKind::Asset => resources.asset_count += 1,
            SkillResourceKind::Script => resources.script_count += 1,
            SkillResourceKind::Other => {
                // Generic resources remain visible in the total count and byte
                // size; unlike scripts they do not introduce a special warning.
            }
        }
    }
    let mut warnings = Vec::new();
    if resources.resource_count > 0 {
        warnings.push(SkillInstallationWarning {
            code: SkillInstallationWarningCode::ResourcesNotExposed,
            message: "This version preserves sibling Skill files but does not yet expose them to the agent runtime. Instructions that depend on those files may not work."
                .to_string(),
            acknowledgement_required: false,
        });
    }
    if resources.script_count > 0 {
        warnings.push(SkillInstallationWarning {
            code: SkillInstallationWarningCode::ContainsScripts,
            message: "This Skill contains script files. They are installed as inert resources and are never executed automatically."
                .to_string(),
            acknowledgement_required: true,
        });
    }
    let package_preview = SkillInstallationPackagePreview {
        name: package.name().to_string(),
        description: package.description().to_string(),
        revision: package.revision().clone(),
        format_version: package.format_version(),
        entrypoint_bytes: u64::try_from(package.source_bytes().len()).unwrap_or(u64::MAX),
        resources,
    };
    let presentation = SkillAcquisitionPresentation {
        provider: package.origin().provider().to_string(),
        reference: package.origin().reference().to_string(),
    };
    let changes = current.map_or(
        PreviewChanges {
            content_changed: true,
            source_changed: true,
        },
        |record| PreviewChanges {
            content_changed: record.package_revision() != package.revision(),
            source_changed: record.provenance() != acquisition.provenance(),
        },
    );
    let preview_revision = preview_revision(
        request,
        action,
        &presentation,
        &package_preview,
        &warnings,
        changes,
        expires_at_unix_ms,
    );
    SkillInstallationPreview {
        preparation_id: request.preparation_id.clone(),
        preview_revision,
        operation: action.operation(),
        installation_id: action.installation_id().clone(),
        skill_id: action.skill_id().clone(),
        expected_revision: action.expected_revision().cloned(),
        content_changed: changes.content_changed,
        source_changed: changes.source_changed,
        acquisition: presentation,
        package: package_preview,
        warnings: warnings.into(),
        expires_at_unix_ms,
    }
}

#[derive(Clone, Copy)]
struct PreviewChanges {
    content_changed: bool,
    source_changed: bool,
}

fn preview_revision(
    request: &SkillInstallationPreparationRequest,
    action: &PreparedAction,
    acquisition: &SkillAcquisitionPresentation,
    package: &SkillInstallationPackagePreview,
    warnings: &[SkillInstallationWarning],
    changes: PreviewChanges,
    expires_at_unix_ms: u64,
) -> SkillPreviewRevision {
    let mut hasher = Sha256::new();
    update_hash_field(&mut hasher, b"mycopilot-skill-install-preview-v1");
    update_hash_field(&mut hasher, request.preparation_id.as_str().as_bytes());
    update_hash_field(&mut hasher, action.operation().stable_name().as_bytes());
    update_hash_field(&mut hasher, action.installation_id().as_str().as_bytes());
    update_hash_field(&mut hasher, action.skill_id().as_str().as_bytes());
    match action.expected_revision() {
        Some(revision) => {
            update_hash_field(&mut hasher, b"expected-revision-present");
            update_hash_field(&mut hasher, revision.as_str().as_bytes());
        }
        None => update_hash_field(&mut hasher, b"expected-revision-absent"),
    }
    update_hash_field(&mut hasher, request.source.provider().as_str().as_bytes());
    match &request.source {
        SkillAcquisitionSource::LocalDirectory { directory } => {
            update_hash_field(&mut hasher, b"local-directory");
            update_hash_field(&mut hasher, os_path_bytes(directory).as_ref());
        }
        SkillAcquisitionSource::Adapter { request, .. } => {
            update_hash_field(&mut hasher, b"adapter-request");
            update_hash_field(&mut hasher, request);
        }
        SkillAcquisitionSource::ResolvedCandidate {
            resolution_id,
            candidate_id,
        } => {
            update_hash_field(&mut hasher, b"resolved-candidate");
            update_hash_field(&mut hasher, resolution_id.as_str().as_bytes());
            update_hash_field(&mut hasher, candidate_id.as_str().as_bytes());
        }
        SkillAcquisitionSource::InstalledSource => {
            update_hash_field(&mut hasher, b"installed-source");
        }
    }
    update_hash_field(&mut hasher, acquisition.provider.as_bytes());
    update_hash_field(&mut hasher, acquisition.reference.as_bytes());
    update_hash_field(&mut hasher, package.revision.as_str().as_bytes());
    update_hash_field(&mut hasher, &package.format_version.to_be_bytes());
    update_hash_field(&mut hasher, package.name.as_bytes());
    update_hash_field(&mut hasher, package.description.as_bytes());
    update_hash_field(&mut hasher, &package.entrypoint_bytes.to_be_bytes());
    update_hash_field(
        &mut hasher,
        &u64::try_from(package.resources.resource_count)
            .unwrap_or(u64::MAX)
            .to_be_bytes(),
    );
    update_hash_field(&mut hasher, &package.resources.resource_bytes.to_be_bytes());
    update_hash_field(
        &mut hasher,
        &u64::try_from(package.resources.reference_count)
            .unwrap_or(u64::MAX)
            .to_be_bytes(),
    );
    update_hash_field(
        &mut hasher,
        &u64::try_from(package.resources.asset_count)
            .unwrap_or(u64::MAX)
            .to_be_bytes(),
    );
    update_hash_field(
        &mut hasher,
        &u64::try_from(package.resources.script_count)
            .unwrap_or(u64::MAX)
            .to_be_bytes(),
    );
    for warning in warnings {
        update_hash_field(&mut hasher, warning.code.stable_name().as_bytes());
        update_hash_field(&mut hasher, warning.message.as_bytes());
        update_hash_field(
            &mut hasher,
            if warning.acknowledgement_required {
                b"ack-required"
            } else {
                b"ack-not-required"
            },
        );
    }
    update_hash_field(
        &mut hasher,
        if changes.content_changed {
            b"content-changed"
        } else {
            b"content-unchanged"
        },
    );
    update_hash_field(
        &mut hasher,
        if changes.source_changed {
            b"source-changed"
        } else {
            b"source-unchanged"
        },
    );
    update_hash_field(&mut hasher, &expires_at_unix_ms.to_be_bytes());
    SkillPreviewRevision::from_digest(hasher.finalize().into())
}

fn update_hash_field(hasher: &mut Sha256, bytes: &[u8]) {
    hasher.update(u64::try_from(bytes.len()).unwrap_or(u64::MAX).to_be_bytes());
    hasher.update(bytes);
}

#[cfg(unix)]
fn os_path_bytes(path: &Path) -> std::borrow::Cow<'_, [u8]> {
    use std::os::unix::ffi::OsStrExt;
    std::borrow::Cow::Borrowed(path.as_os_str().as_bytes())
}

#[cfg(windows)]
fn os_path_bytes(path: &Path) -> std::borrow::Cow<'_, [u8]> {
    use std::os::windows::ffi::OsStrExt;
    let bytes = path
        .as_os_str()
        .encode_wide()
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<_>>();
    std::borrow::Cow::Owned(bytes)
}

#[cfg(not(any(unix, windows)))]
fn os_path_bytes(path: &Path) -> std::borrow::Cow<'_, [u8]> {
    std::borrow::Cow::Owned(path.to_string_lossy().into_owned().into_bytes())
}

fn acquisition_snapshot_bytes(acquisition: &PreparedSkillAcquisition) -> usize {
    acquisition.retained_payload_bytes()
}

fn invalid_acquisition_provider_output() -> SkillAcquisitionAdapterError {
    SkillAcquisitionAdapterError::unavailable(
        "the acquisition provider returned package or provenance metadata owned by a different provider",
    )
}

fn ensure_preview_matches(
    preview: &SkillInstallationPreview,
    request: &SkillInstallationCommitRequest,
) -> Result<(), SkillInstallationWorkflowError> {
    if preview.preview_revision == request.expected_preview_revision {
        Ok(())
    } else {
        Err(SkillInstallationWorkflowError::PreviewMismatch {
            preparation_id: request.preparation_id.clone(),
            expected: request.expected_preview_revision.clone(),
            actual: preview.preview_revision.clone(),
        })
    }
}

fn ensure_warnings_acknowledged(
    preview: &SkillInstallationPreview,
    request: &SkillInstallationCommitRequest,
) -> Result<(), SkillInstallationWorkflowError> {
    let missing = preview
        .warnings()
        .iter()
        .filter(|warning| {
            warning.acknowledgement_required()
                && !request.acknowledged_warnings.contains(&warning.code())
        })
        .map(SkillInstallationWarning::code)
        .collect::<Vec<_>>();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(
            SkillInstallationWorkflowError::WarningAcknowledgementRequired {
                preparation_id: request.preparation_id.clone(),
                missing,
            },
        )
    }
}

#[non_exhaustive]
pub enum SkillInstallationWorkflowError {
    UnknownAcquisitionProvider {
        provider: SkillAcquisitionProvider,
    },
    InvalidUpdateTarget {
        skill_id: SkillId,
        reason: String,
    },
    InstalledSourceRequiresUpdate,
    InstalledSkillNotFound {
        installation_id: SkillInstallationId,
    },
    InstalledSkillRead {
        installation_id: SkillInstallationId,
        reason: String,
    },
    InstalledSourceRevisionConflict {
        installation_id: SkillInstallationId,
        expected_revision: SkillInstallationRevision,
        actual_revision: SkillInstallationRevision,
    },
    InstalledSourceLegacy {
        installation_id: SkillInstallationId,
    },
    InstalledSourceNotRefreshable {
        installation_id: SkillInstallationId,
    },
    UnknownRefreshProvider {
        provider: String,
    },
    UnsupportedRefreshSchema {
        provider: String,
        schema_version: u32,
    },
    InvalidInstalledSourceProvenance {
        provider: String,
    },
    PreparationConflict {
        preparation_id: SkillPreparationId,
    },
    PreparationNotFoundOrExpired {
        preparation_id: SkillPreparationId,
    },
    PreparationCancelled {
        preparation_id: SkillPreparationId,
    },
    PreparationAlreadyCommitted {
        preparation_id: SkillPreparationId,
    },
    PreparationBusy {
        preparation_id: SkillPreparationId,
    },
    PreviewMismatch {
        preparation_id: SkillPreparationId,
        expected: SkillPreviewRevision,
        actual: SkillPreviewRevision,
    },
    PreparationCapacityExceeded {
        max_preparations: usize,
    },
    PreparationMemoryCapacityExceeded {
        max_snapshot_bytes: usize,
    },
    SourceResolutionNotFoundOrExpired {
        resolution_id: SkillSourceResolutionId,
    },
    SourceResolutionBusy {
        resolution_id: SkillSourceResolutionId,
    },
    SourceResolutionConsumed {
        resolution_id: SkillSourceResolutionId,
    },
    SourceResolutionCancelled {
        resolution_id: SkillSourceResolutionId,
    },
    SourceCandidateNotFound {
        resolution_id: SkillSourceResolutionId,
        candidate_id: SkillSourceCandidateId,
    },
    WarningAcknowledgementRequired {
        preparation_id: SkillPreparationId,
        missing: Vec<SkillInstallationWarningCode>,
    },
    Acquisition {
        provider: SkillAcquisitionProvider,
        source: Box<SkillAcquisitionAdapterError>,
    },
    Installation {
        preparation_id: SkillPreparationId,
        source: Box<SkillInstallationServiceError>,
    },
    Internal {
        operation: &'static str,
        reason: String,
    },
}

impl SkillInstallationWorkflowError {
    pub fn preparation_id(&self) -> Option<&SkillPreparationId> {
        match self {
            Self::PreparationConflict { preparation_id }
            | Self::PreparationNotFoundOrExpired { preparation_id }
            | Self::PreparationCancelled { preparation_id }
            | Self::PreparationAlreadyCommitted { preparation_id }
            | Self::PreparationBusy { preparation_id }
            | Self::PreviewMismatch { preparation_id, .. }
            | Self::WarningAcknowledgementRequired { preparation_id, .. }
            | Self::Installation { preparation_id, .. } => Some(preparation_id),
            _ => None,
        }
    }

    pub fn acquisition_error(&self) -> Option<&SkillAcquisitionAdapterError> {
        match self {
            Self::Acquisition { source, .. } => Some(source),
            _ => None,
        }
    }

    pub fn installation_error(&self) -> Option<&SkillInstallationServiceError> {
        match self {
            Self::Installation { source, .. } => Some(source),
            _ => None,
        }
    }

    pub fn missing_warning_acknowledgements(&self) -> Option<&[SkillInstallationWarningCode]> {
        match self {
            Self::WarningAcknowledgementRequired { missing, .. } => Some(missing),
            _ => None,
        }
    }

    pub fn internal_reason(&self) -> Option<&str> {
        match self {
            Self::Internal { reason, .. } => Some(reason),
            _ => None,
        }
    }
}

impl fmt::Debug for SkillInstallationWorkflowError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut debug = formatter.debug_struct("SkillInstallationWorkflowError");
        match self {
            Self::UnknownAcquisitionProvider { provider } => {
                debug.field("kind", &"unknownAcquisitionProvider");
                debug.field("provider", provider);
            }
            Self::InvalidUpdateTarget { skill_id, .. } => {
                debug.field("kind", &"invalidUpdateTarget");
                debug.field("skill_id", skill_id);
            }
            Self::InstalledSourceRequiresUpdate => {
                debug.field("kind", &"installedSourceRequiresUpdate");
            }
            Self::InstalledSkillNotFound { installation_id } => {
                debug.field("kind", &"installedSkillNotFound");
                debug.field("installation_id", installation_id);
            }
            Self::InstalledSkillRead {
                installation_id, ..
            } => {
                debug.field("kind", &"installedSkillRead");
                debug.field("installation_id", installation_id);
            }
            Self::InstalledSourceRevisionConflict {
                installation_id,
                expected_revision,
                actual_revision,
            } => {
                debug.field("kind", &"installedSourceRevisionConflict");
                debug.field("installation_id", installation_id);
                debug.field("expected_revision", expected_revision);
                debug.field("actual_revision", actual_revision);
            }
            Self::InstalledSourceLegacy { installation_id } => {
                debug.field("kind", &"installedSourceLegacy");
                debug.field("installation_id", installation_id);
            }
            Self::InstalledSourceNotRefreshable { installation_id } => {
                debug.field("kind", &"installedSourceNotRefreshable");
                debug.field("installation_id", installation_id);
            }
            Self::UnknownRefreshProvider { provider } => {
                debug.field("kind", &"unknownRefreshProvider");
                debug.field("provider", provider);
            }
            Self::UnsupportedRefreshSchema {
                provider,
                schema_version,
            } => {
                debug.field("kind", &"unsupportedRefreshSchema");
                debug.field("provider", provider);
                debug.field("schema_version", schema_version);
            }
            Self::InvalidInstalledSourceProvenance { provider } => {
                debug.field("kind", &"invalidInstalledSourceProvenance");
                debug.field("provider", provider);
            }
            Self::PreparationConflict { preparation_id } => {
                debug.field("kind", &"preparationConflict");
                debug.field("preparation_id", preparation_id);
            }
            Self::PreparationNotFoundOrExpired { preparation_id } => {
                debug.field("kind", &"preparationNotFoundOrExpired");
                debug.field("preparation_id", preparation_id);
            }
            Self::PreparationCancelled { preparation_id } => {
                debug.field("kind", &"preparationCancelled");
                debug.field("preparation_id", preparation_id);
            }
            Self::PreparationAlreadyCommitted { preparation_id } => {
                debug.field("kind", &"preparationAlreadyCommitted");
                debug.field("preparation_id", preparation_id);
            }
            Self::PreparationBusy { preparation_id } => {
                debug.field("kind", &"preparationBusy");
                debug.field("preparation_id", preparation_id);
            }
            Self::PreviewMismatch {
                preparation_id,
                expected,
                actual,
            } => {
                debug.field("kind", &"previewMismatch");
                debug.field("preparation_id", preparation_id);
                debug.field("expected", expected);
                debug.field("actual", actual);
            }
            Self::PreparationCapacityExceeded { max_preparations } => {
                debug.field("kind", &"preparationCapacityExceeded");
                debug.field("max_preparations", max_preparations);
            }
            Self::PreparationMemoryCapacityExceeded { max_snapshot_bytes } => {
                debug.field("kind", &"preparationMemoryCapacityExceeded");
                debug.field("max_snapshot_bytes", max_snapshot_bytes);
            }
            Self::SourceResolutionNotFoundOrExpired { resolution_id } => {
                debug.field("kind", &"sourceResolutionNotFoundOrExpired");
                debug.field("resolution_id", resolution_id);
            }
            Self::SourceResolutionBusy { resolution_id } => {
                debug.field("kind", &"sourceResolutionBusy");
                debug.field("resolution_id", resolution_id);
            }
            Self::SourceResolutionConsumed { resolution_id } => {
                debug.field("kind", &"sourceResolutionConsumed");
                debug.field("resolution_id", resolution_id);
            }
            Self::SourceResolutionCancelled { resolution_id } => {
                debug.field("kind", &"sourceResolutionCancelled");
                debug.field("resolution_id", resolution_id);
            }
            Self::SourceCandidateNotFound {
                resolution_id,
                candidate_id,
            } => {
                debug.field("kind", &"sourceCandidateNotFound");
                debug.field("resolution_id", resolution_id);
                debug.field("candidate_id", candidate_id);
            }
            Self::WarningAcknowledgementRequired {
                preparation_id,
                missing,
            } => {
                debug.field("kind", &"warningAcknowledgementRequired");
                debug.field("preparation_id", preparation_id);
                debug.field("missing", missing);
            }
            Self::Acquisition {
                provider, source, ..
            } => {
                debug.field("kind", &"acquisition");
                debug.field("provider", provider);
                debug.field("source", source);
            }
            Self::Installation { preparation_id, .. } => {
                debug.field("kind", &"installation");
                debug.field("preparation_id", preparation_id);
            }
            Self::Internal { operation, .. } => {
                debug.field("kind", &"internal");
                debug.field("operation", operation);
            }
        }
        debug.finish()
    }
}

impl fmt::Display for SkillInstallationWorkflowError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownAcquisitionProvider { provider } => {
                write!(
                    formatter,
                    "No Skill acquisition adapter is registered for `{provider}`"
                )
            }
            Self::InvalidUpdateTarget { reason, .. } => reason.fmt(formatter),
            Self::InstalledSourceRequiresUpdate => {
                formatter.write_str("Installed-source acquisition is valid only for updates")
            }
            Self::InstalledSkillNotFound { .. } => {
                formatter.write_str("The installed Skill receipt was not found")
            }
            Self::InstalledSkillRead { .. } => {
                formatter.write_str("The installed Skill receipt could not be read")
            }
            Self::InstalledSourceRevisionConflict { .. } => {
                formatter.write_str("The installed Skill changed before source refresh could begin")
            }
            Self::InstalledSourceLegacy { .. } => {
                formatter.write_str("Legacy Skill installations do not contain refresh provenance")
            }
            Self::InstalledSourceNotRefreshable { .. } => {
                formatter.write_str("The installed Skill source is immutable or not refreshable")
            }
            Self::UnknownRefreshProvider { provider } => write!(
                formatter,
                "No Skill refresh adapter is registered for `{provider}`"
            ),
            Self::UnsupportedRefreshSchema {
                provider,
                schema_version,
            } => write!(
                formatter,
                "Skill refresh provider `{provider}` does not support schema {schema_version}"
            ),
            Self::InvalidInstalledSourceProvenance { provider } => write!(
                formatter,
                "Installed Skill provenance for `{provider}` is invalid or inconsistent"
            ),
            Self::PreparationConflict { .. } => {
                formatter.write_str("Preparation ID is already bound to a different request")
            }
            Self::PreparationNotFoundOrExpired { .. } => {
                formatter.write_str("Skill preparation was not found or has expired")
            }
            Self::PreparationCancelled { .. } => {
                formatter.write_str("Skill preparation was cancelled")
            }
            Self::PreparationAlreadyCommitted { .. } => {
                formatter.write_str("A committed Skill preparation cannot be cancelled")
            }
            Self::PreparationBusy { .. } => {
                formatter.write_str("Skill preparation is currently busy")
            }
            Self::PreviewMismatch { .. } => {
                formatter.write_str("Skill preview revision does not match the prepared preview")
            }
            Self::PreparationCapacityExceeded { max_preparations } => write!(
                formatter,
                "Skill preparation registry reached its {max_preparations}-entry limit",
            ),
            Self::PreparationMemoryCapacityExceeded { max_snapshot_bytes } => write!(
                formatter,
                "Skill preparation registry reached its {max_snapshot_bytes}-byte snapshot limit",
            ),
            Self::SourceResolutionNotFoundOrExpired { .. } => {
                formatter.write_str("Skill source resolution was not found or has expired")
            }
            Self::SourceResolutionBusy { .. } => {
                formatter.write_str("Skill source resolution is still in progress")
            }
            Self::SourceResolutionConsumed { .. } => {
                formatter.write_str("Skill source resolution was already consumed")
            }
            Self::SourceResolutionCancelled { .. } => {
                formatter.write_str("Skill source resolution was cancelled")
            }
            Self::SourceCandidateNotFound { .. } => {
                formatter.write_str("Resolved Skill candidate was not found")
            }
            Self::WarningAcknowledgementRequired { .. } => {
                formatter.write_str("Required Skill installation warnings were not acknowledged")
            }
            Self::Acquisition { provider, .. } => {
                write!(formatter, "Skill acquisition through `{provider}` failed")
            }
            Self::Installation { .. } => formatter.write_str("Skill installation commit failed"),
            Self::Internal { operation, .. } => {
                write!(
                    formatter,
                    "Internal failure while attempting to {operation}"
                )
            }
        }
    }
}

impl Error for SkillInstallationWorkflowError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Acquisition { source, .. } => Some(source.as_ref()),
            Self::Installation { source, .. } => Some(source.as_ref()),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::origin::SkillPackageOrigin;
    use super::super::{
        GitHubAcquisitionSummary, GitHubCommit, GitHubReference, GitHubRepository,
        GitHubSubdirectory, PreparedSkillSourceResolution, PreparedSkillSourceResolutionCandidate,
        ResolvedSkillSource, SkillInstallationOutcome, SkillInstallationSourceLocator,
        SkillInstallationSourceResolver, SkillSourceResolution, SkillSourceResolutionError,
        SkillSourceResolutionService, SkillSourceResolverId, SkillsService,
    };
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
    use tempfile::tempdir;

    const INSTALLATION_ID: &str = "01234567-89ab-4def-8123-456789abcdef";

    fn installation_id() -> SkillInstallationId {
        SkillInstallationId::parse(INSTALLATION_ID).unwrap()
    }

    fn write_skill(directory: &Path, marker: &str, with_resources: bool) {
        fs::create_dir_all(directory).unwrap();
        fs::write(
            directory.join("SKILL.md"),
            format!(
                "---\nname: workflow-fixture\ndescription: Workflow fixture.\n---\n# Instructions\n{marker}\n"
            ),
        )
        .unwrap();
        if with_resources {
            fs::create_dir_all(directory.join("references")).unwrap();
            fs::create_dir_all(directory.join("assets")).unwrap();
            fs::create_dir_all(directory.join("scripts")).unwrap();
            fs::write(directory.join("references/guide.md"), "guide").unwrap();
            fs::write(directory.join("assets/icon.bin"), [0_u8, 1, 2]).unwrap();
            fs::write(directory.join("scripts/check.sh"), "echo check\n").unwrap();
        }
    }

    fn workflow(store: &Path) -> SkillInstallationWorkflow {
        SkillInstallationWorkflow::new(SkillInstallationService::new(store).unwrap())
    }

    const REFRESH_FIXTURE_PROVIDER: &str = "fixture-refresh";

    fn provider_acquisition(
        marker: &str,
        provider: &str,
        authority_payload: &str,
        refresh: Option<(&str, u32, &str)>,
    ) -> PreparedSkillAcquisition {
        let package = PreparedSkillPackage::from_bytes(
            format!(
                "---\nname: refresh-fixture\ndescription: Refresh fixture.\n---\n# Instructions\n{marker}\n"
            )
            .into_bytes(),
            SkillPackageOrigin::new(provider, authority_payload).unwrap(),
        )
        .unwrap();
        let authority = SkillInstallationAuthority::new(provider, 1, authority_payload).unwrap();
        let refresh = refresh.map(|(provider, schema_version, payload)| {
            SkillInstallationRefresh::new(provider, schema_version, payload).unwrap()
        });
        PreparedSkillAcquisition::new(
            package,
            SkillInstallationProvenance::new(authority, refresh),
        )
    }

    fn fixture_acquisition(
        marker: &str,
        authority_payload: &str,
        refresh: Option<(&str, u32, &str)>,
    ) -> PreparedSkillAcquisition {
        provider_acquisition(marker, REFRESH_FIXTURE_PROVIDER, authority_payload, refresh)
    }

    fn acquisition_with_provider_parts(
        marker: &str,
        origin_provider: &str,
        authority_provider: &str,
        refresh_provider: Option<&str>,
    ) -> PreparedSkillAcquisition {
        let package = PreparedSkillPackage::from_bytes(
            format!(
                "---\nname: provider-binding-fixture\ndescription: Provider binding fixture.\n---\n# Instructions\n{marker}\n"
            )
            .into_bytes(),
            SkillPackageOrigin::new(origin_provider, "origin").unwrap(),
        )
        .unwrap();
        let authority =
            SkillInstallationAuthority::new(authority_provider, 1, "authority").unwrap();
        let refresh = refresh_provider
            .map(|provider| SkillInstallationRefresh::new(provider, 1, "tracking").unwrap());
        PreparedSkillAcquisition::new(
            package,
            SkillInstallationProvenance::new(authority, refresh),
        )
    }

    struct RefreshFixtureAdapter {
        calls: Arc<AtomicUsize>,
        next: PreparedSkillAcquisition,
    }

    impl SkillAcquisitionAdapter for RefreshFixtureAdapter {
        fn provider(&self) -> SkillAcquisitionProvider {
            SkillAcquisitionProvider::parse(REFRESH_FIXTURE_PROVIDER).unwrap()
        }

        fn acquire(
            &self,
            _source: &SkillAcquisitionSource,
        ) -> Result<PreparedSkillAcquisition, SkillAcquisitionAdapterError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.next.clone())
        }

        fn refresh_schema_versions(&self) -> &'static [u32] {
            &[1]
        }

        fn reacquire(
            &self,
            refresh: SkillInstallationRefreshView<'_>,
        ) -> Result<PreparedSkillAcquisition, SkillAcquisitionAdapterError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if refresh.provider() != REFRESH_FIXTURE_PROVIDER
                || refresh.schema_version() != 1
                || refresh.payload() != "tracking"
            {
                return Err(SkillAcquisitionAdapterError::invalid_request(
                    "invalid fixture refresh metadata",
                ));
            }
            Ok(self.next.clone())
        }

        fn installed_source_presentation(
            &self,
            provenance: SkillInstallationProvenanceView<'_>,
            refresh_capable: bool,
        ) -> Option<InstalledSkillSourcePresentation> {
            let authority = provenance.authority();
            let refresh = provenance.refresh()?;
            (authority.provider() == REFRESH_FIXTURE_PROVIDER
                && authority.schema_version() == 1
                && refresh.provider() == REFRESH_FIXTURE_PROVIDER
                && refresh.schema_version() == 1
                && refresh.payload() == "tracking")
                .then(|| InstalledSkillSourcePresentation::Provider {
                    provider: REFRESH_FIXTURE_PROVIDER.to_string(),
                    display_name: "Fixture refresh".to_string(),
                    refreshable: refresh_capable,
                })
        }
    }

    const PRESENTATION_FIXTURE_PROVIDER: &str = "fixture-presentation";

    struct PresentationFixtureAdapter {
        presentation: InstalledSkillSourcePresentation,
    }

    impl SkillAcquisitionAdapter for PresentationFixtureAdapter {
        fn provider(&self) -> SkillAcquisitionProvider {
            SkillAcquisitionProvider::parse(PRESENTATION_FIXTURE_PROVIDER).unwrap()
        }

        fn acquire(
            &self,
            _source: &SkillAcquisitionSource,
        ) -> Result<PreparedSkillAcquisition, SkillAcquisitionAdapterError> {
            Err(SkillAcquisitionAdapterError::invalid_request(
                "the presentation fixture does not acquire packages",
            ))
        }

        fn refresh_schema_versions(&self) -> &'static [u32] {
            &[1]
        }

        fn installed_source_presentation(
            &self,
            _provenance: SkillInstallationProvenanceView<'_>,
            _refresh_capable: bool,
        ) -> Option<InstalledSkillSourcePresentation> {
            Some(self.presentation.clone())
        }
    }

    fn refresh_workflow(
        store: &Path,
        initial: PreparedSkillAcquisition,
        next: PreparedSkillAcquisition,
    ) -> (
        SkillInstallationWorkflow,
        InstalledSkillRecord,
        Arc<AtomicUsize>,
    ) {
        let service = SkillInstallationService::new(store).unwrap();
        let (package, provenance) = initial.into_parts();
        service
            .install_prepared_with_provenance(installation_id(), package, provenance)
            .unwrap();
        let record = service
            .read_installed_skill(&installation_id())
            .unwrap()
            .unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let mut workflow = SkillInstallationWorkflow::new(service);
        workflow
            .register_adapter(Arc::new(RefreshFixtureAdapter {
                calls: Arc::clone(&calls),
                next,
            }))
            .unwrap();
        (workflow, record, calls)
    }

    const HANDOFF_COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";

    struct HandoffResolver {
        calls: Arc<AtomicUsize>,
    }

    impl HandoffResolver {
        fn prepared(
            &self,
            locator: &SkillInstallationSourceLocator,
        ) -> PreparedSkillSourceResolution {
            let repository = GitHubRepository::parse("example", "skills").unwrap();
            let commit = GitHubCommit::parse(HANDOFF_COMMIT).unwrap();
            let subdirectory = GitHubSubdirectory::parse("skills/handoff").unwrap();
            let summary = GitHubAcquisitionSummary::for_resolved_pin(
                &repository,
                &GitHubReference::DefaultBranch,
                &commit,
                &subdirectory,
            );
            let origin = summary.origin().unwrap();
            let package = PreparedSkillPackage::from_bytes(
                b"---\nname: handoff-fixture\ndescription: Exact handoff fixture.\n---\n# Instructions\nRESOLVED_ONCE_EXACT_BYTES\n"
                    .to_vec(),
                origin,
            )
            .unwrap();
            PreparedSkillSourceResolution::new(
                locator.as_url(),
                SkillSourceResolverId::parse("github").unwrap(),
                HANDOFF_COMMIT,
                vec![PreparedSkillSourceResolutionCandidate::new(
                    SkillSourceCandidateId::parse("handoff").unwrap(),
                    ResolvedSkillSource::GitHub {
                        owner: "example".to_string(),
                        repository: "skills".to_string(),
                        tracking_reference: GitHubReference::DefaultBranch,
                        resolved_commit: HANDOFF_COMMIT.to_string(),
                        subdirectory: Some("skills/handoff".to_string()),
                    },
                    PreparedSkillAcquisition::new(package, summary.provenance().unwrap()),
                )],
            )
            .unwrap()
        }
    }

    impl SkillInstallationSourceResolver for HandoffResolver {
        fn id(&self) -> SkillSourceResolverId {
            SkillSourceResolverId::parse("github").unwrap()
        }

        fn supported_hosts(&self) -> Vec<String> {
            vec!["github.com".to_string()]
        }

        fn resolve(
            &self,
            locator: &SkillInstallationSourceLocator,
        ) -> Result<SkillSourceResolution, SkillSourceResolutionError> {
            self.prepared(locator).public_resolution()
        }

        fn resolve_prepared(
            &self,
            locator: &SkillInstallationSourceLocator,
        ) -> Result<PreparedSkillSourceResolution, SkillSourceResolutionError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.prepared(locator))
        }
    }

    fn resolved_handoff(
        store: &Path,
    ) -> (
        SkillInstallationWorkflow,
        SkillSourceResolutionId,
        SkillSourceCandidateId,
        Arc<AtomicUsize>,
    ) {
        let sessions = SkillInstallationSessionStore::default();
        let calls = Arc::new(AtomicUsize::new(0));
        let mut resolutions = SkillSourceResolutionService::with_session_store(sessions.clone());
        resolutions
            .register_resolver(Arc::new(HandoffResolver {
                calls: Arc::clone(&calls),
            }))
            .unwrap();
        let resolution_id = SkillSourceResolutionId::new();
        let locator =
            SkillInstallationSourceLocator::url("https://github.com/example/skills").unwrap();
        let registered = resolutions
            .resolve_registered(resolution_id.clone(), &locator)
            .unwrap();
        let candidate_id =
            SkillSourceCandidateId::parse(registered.resolution().candidates()[0].candidate_id())
                .unwrap();
        let workflow = SkillInstallationWorkflow::with_session_store(
            SkillInstallationService::new(store).unwrap(),
            SkillInstallationWorkflowConfig::default(),
            sessions,
        );
        (workflow, resolution_id, candidate_id, calls)
    }

    #[test]
    fn local_preview_is_idempotent_and_commit_uses_the_captured_snapshot() {
        let fixture = tempdir().unwrap();
        let store = fixture.path().join("store");
        let source = fixture.path().join("source");
        write_skill(&source, "ORIGINAL", false);
        let workflow = workflow(&store);
        let preparation_id = SkillPreparationId::new();
        let request = SkillInstallationPreparationRequest::install(
            preparation_id.clone(),
            installation_id(),
            SkillAcquisitionSource::local_directory(&source),
        );

        let first = workflow.inspect(&request).unwrap();
        assert!(first.content_changed());
        assert!(first.source_changed());
        assert_eq!(first.acquisition().provider(), LOCAL_DIRECTORY_PROVIDER);
        assert_eq!(
            first.acquisition().reference(),
            LOCAL_DIRECTORY_ORIGIN_REFERENCE
        );
        assert!(!format!("{first:?}").contains(source.to_str().unwrap()));
        write_skill(&source, "MUTATED", false);
        let retry = workflow.inspect(&request).unwrap();
        assert_eq!(first, retry);

        let committed = workflow
            .commit(&SkillInstallationCommitRequest::new(
                preparation_id.clone(),
                first.preview_revision().clone(),
            ))
            .unwrap();
        assert_eq!(
            committed.mutation().outcome(),
            SkillInstallationOutcome::Installed
        );
        assert_eq!(committed.preview(), &first);
        assert!(!committed.replayed());

        let replay = workflow
            .commit(&SkillInstallationCommitRequest::new(
                preparation_id,
                first.preview_revision().clone(),
            ))
            .unwrap();
        assert!(replay.replayed());
        assert_eq!(replay.preview(), &first);
        assert_eq!(replay.mutation(), committed.mutation());

        let reader = SkillsService::new().with_installed_source(&store).unwrap();
        let catalog = reader.list().unwrap();
        let activated = reader.activate(&[catalog.skills()[0].selection()]).unwrap();
        assert!(activated.skills()[0].instructions().contains("ORIGINAL"));
        assert!(!activated.skills()[0].instructions().contains("MUTATED"));
    }

    #[test]
    fn resolved_candidate_moves_exact_bytes_once_into_the_installation_transaction() {
        let fixture = tempdir().unwrap();
        let store = fixture.path().join("store");
        let (workflow, resolution_id, candidate_id, calls) = resolved_handoff(&store);
        let preparation_id = SkillPreparationId::new();
        let request = SkillInstallationPreparationRequest::install(
            preparation_id.clone(),
            installation_id(),
            SkillAcquisitionSource::resolved_candidate(resolution_id.clone(), candidate_id.clone()),
        );

        let preview = workflow.inspect(&request).unwrap();
        assert_eq!(workflow.inspect(&request).unwrap(), preview);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(preview.acquisition().provider(), "github");
        let origin: serde_json::Value =
            serde_json::from_str(preview.acquisition().reference()).unwrap();
        assert_eq!(origin["resolvedCommit"], HANDOFF_COMMIT);

        let consumed = SkillInstallationPreparationRequest::install(
            SkillPreparationId::new(),
            SkillInstallationId::new(),
            SkillAcquisitionSource::resolved_candidate(resolution_id, candidate_id),
        );
        assert!(matches!(
            workflow.inspect(&consumed).unwrap_err(),
            SkillInstallationWorkflowError::SourceResolutionConsumed { .. }
        ));

        workflow
            .commit(&SkillInstallationCommitRequest::new(
                preparation_id,
                preview.preview_revision().clone(),
            ))
            .unwrap();
        let record = SkillInstallationService::new(&store)
            .unwrap()
            .read_installed_skill(&installation_id())
            .unwrap()
            .unwrap();
        assert_eq!(record.provenance().authority().provider(), "github");
        assert!(record.provenance().refresh().is_some());
        let reader = SkillsService::new().with_installed_source(&store).unwrap();
        let catalog = reader.list().unwrap();
        let activated = reader.activate(&[catalog.skills()[0].selection()]).unwrap();
        assert!(activated.skills()[0]
            .instructions()
            .contains("RESOLVED_ONCE_EXACT_BYTES"));
    }

    #[test]
    fn consuming_a_resolved_candidate_immediately_releases_active_resolution_capacity() {
        let fixture = tempdir().unwrap();
        let config = SkillInstallationSessionConfig::new(
            1,
            2,
            super::super::installation_session::DEFAULT_SKILL_SESSION_SNAPSHOT_BYTES,
            Duration::from_secs(60),
        )
        .unwrap()
        .with_max_resolution_tombstones(4)
        .unwrap();
        let sessions = SkillInstallationSessionStore::new(config);
        let calls = Arc::new(AtomicUsize::new(0));
        let mut resolutions = SkillSourceResolutionService::with_session_store(sessions.clone());
        resolutions
            .register_resolver(Arc::new(HandoffResolver {
                calls: Arc::clone(&calls),
            }))
            .unwrap();
        let locator =
            SkillInstallationSourceLocator::url("https://github.com/example/skills").unwrap();
        let first_id = SkillSourceResolutionId::new();
        let first = resolutions
            .resolve_registered(first_id.clone(), &locator)
            .unwrap();
        let candidate_id =
            SkillSourceCandidateId::parse(first.resolution().candidates()[0].candidate_id())
                .unwrap();
        let workflow = SkillInstallationWorkflow::with_session_store(
            SkillInstallationService::new(fixture.path().join("store")).unwrap(),
            SkillInstallationWorkflowConfig::default(),
            sessions,
        );
        workflow
            .inspect(&SkillInstallationPreparationRequest::install(
                SkillPreparationId::new(),
                installation_id(),
                SkillAcquisitionSource::resolved_candidate(first_id, candidate_id),
            ))
            .unwrap();

        resolutions
            .resolve_registered(SkillSourceResolutionId::new(), &locator)
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn missing_candidate_does_not_consume_the_resolution() {
        let fixture = tempdir().unwrap();
        let (workflow, resolution_id, candidate_id, calls) =
            resolved_handoff(&fixture.path().join("store"));
        let missing = SkillInstallationPreparationRequest::install(
            SkillPreparationId::new(),
            installation_id(),
            SkillAcquisitionSource::resolved_candidate(
                resolution_id.clone(),
                SkillSourceCandidateId::parse("missing").unwrap(),
            ),
        );
        assert!(matches!(
            workflow.inspect(&missing).unwrap_err(),
            SkillInstallationWorkflowError::SourceCandidateNotFound { .. }
        ));

        let selected = SkillInstallationPreparationRequest::install(
            SkillPreparationId::new(),
            installation_id(),
            SkillAcquisitionSource::resolved_candidate(resolution_id, candidate_id),
        );
        workflow.inspect(&selected).unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn installed_source_presentation_cannot_change_provider_or_escalate_refreshability() {
        let fixture = tempdir().unwrap();
        let cross_provider_provenance = provider_acquisition(
            "CURRENT",
            PRESENTATION_FIXTURE_PROVIDER,
            "authority",
            Some((PRESENTATION_FIXTURE_PROVIDER, 1, "tracking")),
        )
        .provenance()
        .clone();
        let mut cross_provider_workflow = workflow(&fixture.path().join("cross-provider"));
        cross_provider_workflow
            .register_adapter(Arc::new(PresentationFixtureAdapter {
                presentation: InstalledSkillSourcePresentation::Provider {
                    provider: "different-provider".to_string(),
                    display_name: "Forged provider".to_string(),
                    refreshable: false,
                },
            }))
            .unwrap();
        assert!(matches!(
            cross_provider_workflow
                .installed_source_presentation(&cross_provider_provenance),
            InstalledSkillSourcePresentation::Unknown { ref provider, .. }
                if provider == PRESENTATION_FIXTURE_PROVIDER
        ));

        let non_refreshable_provenance =
            provider_acquisition("CURRENT", PRESENTATION_FIXTURE_PROVIDER, "authority", None)
                .provenance()
                .clone();
        let mut escalating_workflow = workflow(&fixture.path().join("refresh-escalation"));
        escalating_workflow
            .register_adapter(Arc::new(PresentationFixtureAdapter {
                presentation: InstalledSkillSourcePresentation::Provider {
                    provider: PRESENTATION_FIXTURE_PROVIDER.to_string(),
                    display_name: "Forged refresh capability".to_string(),
                    refreshable: true,
                },
            }))
            .unwrap();
        assert!(matches!(
            escalating_workflow.installed_source_presentation(&non_refreshable_provenance),
            InstalledSkillSourcePresentation::Unknown { ref provider, .. }
                if provider == PRESENTATION_FIXTURE_PROVIDER
        ));
        assert!(!escalating_workflow.can_refresh(&non_refreshable_provenance));
    }

    #[test]
    fn installed_source_presentation_rejects_unsafe_provider_display_names() {
        let fixture = tempdir().unwrap();
        let provenance =
            provider_acquisition("CURRENT", PRESENTATION_FIXTURE_PROVIDER, "authority", None)
                .provenance()
                .clone();
        let invalid_display_names = [
            String::new(),
            "   ".to_string(),
            "Fixture\nsource".to_string(),
            "x".repeat(MAX_INSTALLED_SOURCE_DISPLAY_NAME_BYTES + 1),
        ];

        for (index, display_name) in invalid_display_names.into_iter().enumerate() {
            let mut workflow = workflow(&fixture.path().join(format!("invalid-{index}")));
            workflow
                .register_adapter(Arc::new(PresentationFixtureAdapter {
                    presentation: InstalledSkillSourcePresentation::Provider {
                        provider: PRESENTATION_FIXTURE_PROVIDER.to_string(),
                        display_name,
                        refreshable: false,
                    },
                }))
                .unwrap();

            assert!(matches!(
                workflow.installed_source_presentation(&provenance),
                InstalledSkillSourcePresentation::Unknown { ref provider, .. }
                    if provider == PRESENTATION_FIXTURE_PROVIDER
            ));
        }
    }

    #[test]
    fn installed_source_refresh_updates_bytes_and_persists_new_provenance() {
        let fixture = tempdir().unwrap();
        let store = fixture.path().join("store");
        let initial = fixture_acquisition(
            "VERSION_ONE",
            "authority-one",
            Some((REFRESH_FIXTURE_PROVIDER, 1, "tracking")),
        );
        let next = fixture_acquisition(
            "VERSION_TWO",
            "authority-two",
            Some((REFRESH_FIXTURE_PROVIDER, 1, "tracking")),
        );
        let expected_next = next.clone();
        let (workflow, record, calls) = refresh_workflow(&store, initial, next);
        assert!(workflow.can_refresh(record.provenance()));
        assert!(matches!(
            workflow.installed_source_presentation(record.provenance()),
            InstalledSkillSourcePresentation::Provider {
                refreshable: true,
                ..
            }
        ));
        let request = SkillInstallationPreparationRequest::update(
            SkillPreparationId::new(),
            record.skill_id().clone(),
            record.installation_revision().clone(),
            SkillAcquisitionSource::installed_source(),
        );

        let preview = workflow.inspect(&request).unwrap();
        assert!(preview.content_changed());
        assert!(preview.source_changed());
        assert_eq!(workflow.inspect(&request).unwrap(), preview);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        workflow
            .commit(&SkillInstallationCommitRequest::new(
                request.preparation_id().clone(),
                preview.preview_revision().clone(),
            ))
            .unwrap();

        let updated = SkillInstallationService::new(&store)
            .unwrap()
            .read_installed_skill(&installation_id())
            .unwrap()
            .unwrap();
        assert_eq!(
            updated.package_revision(),
            expected_next.package().revision()
        );
        assert_eq!(updated.provenance(), expected_next.provenance());
        assert_ne!(
            updated.installation_revision(),
            record.installation_revision()
        );
    }

    #[test]
    fn installed_source_can_commit_a_provenance_only_update() {
        let fixture = tempdir().unwrap();
        let store = fixture.path().join("store");
        let initial = fixture_acquisition(
            "SAME_BYTES",
            "authority-one",
            Some((REFRESH_FIXTURE_PROVIDER, 1, "tracking")),
        );
        let next = fixture_acquisition(
            "SAME_BYTES",
            "authority-two",
            Some((REFRESH_FIXTURE_PROVIDER, 1, "tracking")),
        );
        let (workflow, record, _) = refresh_workflow(&store, initial, next);
        let request = SkillInstallationPreparationRequest::update(
            SkillPreparationId::new(),
            record.skill_id().clone(),
            record.installation_revision().clone(),
            SkillAcquisitionSource::installed_source(),
        );

        let preview = workflow.inspect(&request).unwrap();
        assert!(!preview.content_changed());
        assert!(preview.source_changed());
        let committed = workflow
            .commit(&SkillInstallationCommitRequest::new(
                request.preparation_id().clone(),
                preview.preview_revision().clone(),
            ))
            .unwrap();
        assert_eq!(
            committed.mutation().outcome(),
            SkillInstallationOutcome::Updated
        );
    }

    #[test]
    fn installed_source_refresh_rejects_cross_provider_adapter_output() {
        let fixture = tempdir().unwrap();
        let store = fixture.path().join("store");
        let initial = fixture_acquisition(
            "VERSION_ONE",
            "authority-one",
            Some((REFRESH_FIXTURE_PROVIDER, 1, "tracking")),
        );
        let next = acquisition_with_provider_parts(
            "VERSION_TWO",
            REFRESH_FIXTURE_PROVIDER,
            "other-provider",
            Some("other-provider"),
        );
        let (workflow, record, calls) = refresh_workflow(&store, initial, next);
        let request = SkillInstallationPreparationRequest::update(
            SkillPreparationId::new(),
            record.skill_id().clone(),
            record.installation_revision().clone(),
            SkillAcquisitionSource::installed_source(),
        );

        let error = workflow.inspect(&request).unwrap_err();
        match error {
            SkillInstallationWorkflowError::Acquisition { provider, source } => {
                assert_eq!(provider.as_str(), REFRESH_FIXTURE_PROVIDER);
                assert_eq!(source.code(), SkillAcquisitionAdapterErrorCode::Unavailable);
                assert!(!source.reason().contains("other-provider"));
            }
            other => panic!("unexpected provider-binding error: {other:?}"),
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(SkillInstallationService::new(&store)
            .unwrap()
            .read_installed_skill(&installation_id())
            .unwrap()
            .is_some_and(
                |installed| installed.installation_revision() == record.installation_revision()
            ));
    }

    #[test]
    fn stale_installed_source_preflight_never_calls_the_adapter() {
        let fixture = tempdir().unwrap();
        let initial = fixture_acquisition(
            "VERSION_ONE",
            "authority-one",
            Some((REFRESH_FIXTURE_PROVIDER, 1, "tracking")),
        );
        let next = fixture_acquisition(
            "VERSION_TWO",
            "authority-two",
            Some((REFRESH_FIXTURE_PROVIDER, 1, "tracking")),
        );
        let (workflow, record, calls) =
            refresh_workflow(&fixture.path().join("store"), initial, next);
        let stale = SkillInstallationRevision::parse(format!(
            "skill-installation-sha256-v1:{}",
            "0".repeat(64)
        ))
        .unwrap();
        assert_ne!(&stale, record.installation_revision());
        let request = SkillInstallationPreparationRequest::update(
            SkillPreparationId::new(),
            record.skill_id().clone(),
            stale,
            SkillAcquisitionSource::installed_source(),
        );

        assert!(matches!(
            workflow.inspect(&request).unwrap_err(),
            SkillInstallationWorkflowError::InstalledSourceRevisionConflict { .. }
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn installed_source_rejects_missing_provider_unsupported_schema_and_install_intent() {
        let fixture = tempdir().unwrap();
        let next = fixture_acquisition(
            "NEXT",
            "authority-two",
            Some((REFRESH_FIXTURE_PROVIDER, 1, "tracking")),
        );

        let unknown_initial = provider_acquisition(
            "CURRENT",
            "missing-provider",
            "authority-one",
            Some(("missing-provider", 1, "tracking")),
        );
        let (unknown_workflow, unknown_record, unknown_calls) = refresh_workflow(
            &fixture.path().join("unknown"),
            unknown_initial,
            next.clone(),
        );
        let unknown_request = SkillInstallationPreparationRequest::update(
            SkillPreparationId::new(),
            unknown_record.skill_id().clone(),
            unknown_record.installation_revision().clone(),
            SkillAcquisitionSource::installed_source(),
        );
        assert!(matches!(
            unknown_workflow.inspect(&unknown_request).unwrap_err(),
            SkillInstallationWorkflowError::UnknownRefreshProvider { .. }
        ));
        assert_eq!(unknown_calls.load(Ordering::SeqCst), 0);

        let delegated_initial = fixture_acquisition(
            "CURRENT",
            "authority-one",
            Some(("missing-provider", 1, "tracking")),
        );
        let (delegated_workflow, delegated_record, delegated_calls) = refresh_workflow(
            &fixture.path().join("delegated"),
            delegated_initial,
            next.clone(),
        );
        let delegated_request = SkillInstallationPreparationRequest::update(
            SkillPreparationId::new(),
            delegated_record.skill_id().clone(),
            delegated_record.installation_revision().clone(),
            SkillAcquisitionSource::installed_source(),
        );
        assert!(!delegated_workflow.can_refresh(delegated_record.provenance()));
        assert!(matches!(
            delegated_workflow.installed_source_presentation(delegated_record.provenance()),
            InstalledSkillSourcePresentation::Unknown { .. }
        ));
        assert!(matches!(
            delegated_workflow.inspect(&delegated_request).unwrap_err(),
            SkillInstallationWorkflowError::InvalidInstalledSourceProvenance { .. }
        ));
        assert_eq!(delegated_calls.load(Ordering::SeqCst), 0);

        let schema_initial = fixture_acquisition(
            "CURRENT",
            "authority-one",
            Some((REFRESH_FIXTURE_PROVIDER, 2, "tracking")),
        );
        let (schema_workflow, schema_record, schema_calls) =
            refresh_workflow(&fixture.path().join("schema"), schema_initial, next);
        let schema_request = SkillInstallationPreparationRequest::update(
            SkillPreparationId::new(),
            schema_record.skill_id().clone(),
            schema_record.installation_revision().clone(),
            SkillAcquisitionSource::installed_source(),
        );
        assert!(matches!(
            schema_workflow.inspect(&schema_request).unwrap_err(),
            SkillInstallationWorkflowError::UnsupportedRefreshSchema { .. }
        ));
        assert_eq!(schema_calls.load(Ordering::SeqCst), 0);

        let install_request = SkillInstallationPreparationRequest::install(
            SkillPreparationId::new(),
            SkillInstallationId::new(),
            SkillAcquisitionSource::installed_source(),
        );
        assert!(matches!(
            schema_workflow.inspect(&install_request).unwrap_err(),
            SkillInstallationWorkflowError::InstalledSourceRequiresUpdate
        ));
    }

    #[test]
    fn local_installation_is_presented_as_nonrefreshable_without_paths() {
        let fixture = tempdir().unwrap();
        let source = fixture.path().join("source");
        let store = fixture.path().join("store");
        write_skill(&source, "LOCAL", false);
        let workflow = workflow(&store);
        let preview = workflow
            .inspect_local_directory_install(SkillPreparationId::new(), installation_id(), &source)
            .unwrap();
        workflow
            .commit(&SkillInstallationCommitRequest::new(
                preview.preparation_id().clone(),
                preview.preview_revision().clone(),
            ))
            .unwrap();
        let record = SkillInstallationService::new(&store)
            .unwrap()
            .read_installed_skill(&installation_id())
            .unwrap()
            .unwrap();
        assert!(!workflow.can_refresh(record.provenance()));
        assert_eq!(
            workflow.installed_source_presentation(record.provenance()),
            InstalledSkillSourcePresentation::LocalDirectory
        );
        assert!(!record
            .provenance()
            .authority()
            .payload()
            .contains(source.to_str().unwrap()));

        let request = SkillInstallationPreparationRequest::update(
            SkillPreparationId::new(),
            record.skill_id().clone(),
            record.installation_revision().clone(),
            SkillAcquisitionSource::installed_source(),
        );
        assert!(matches!(
            workflow.inspect(&request).unwrap_err(),
            SkillInstallationWorkflowError::InstalledSourceNotRefreshable { .. }
        ));
    }

    #[test]
    fn preview_revision_is_required_for_first_commit_and_committed_replay() {
        let fixture = tempdir().unwrap();
        let store = fixture.path().join("store");
        let source = fixture.path().join("source");
        write_skill(&source, "PREVIEW_BOUND", false);
        let workflow = workflow(&store);
        let preparation_id = SkillPreparationId::new();
        let preview = workflow
            .inspect_local_directory_install(preparation_id.clone(), installation_id(), &source)
            .unwrap();
        let wrong =
            SkillPreviewRevision::parse(format!("{PREVIEW_REVISION_PREFIX}{}", "0".repeat(64)))
                .unwrap();

        assert!(matches!(
            workflow
                .commit(&SkillInstallationCommitRequest::new(
                    preparation_id.clone(),
                    wrong.clone(),
                ))
                .unwrap_err(),
            SkillInstallationWorkflowError::PreviewMismatch { .. }
        ));
        let committed = workflow
            .commit(&SkillInstallationCommitRequest::new(
                preparation_id.clone(),
                preview.preview_revision().clone(),
            ))
            .unwrap();
        assert!(!committed.replayed());
        assert!(matches!(
            workflow
                .commit(&SkillInstallationCommitRequest::new(preparation_id, wrong))
                .unwrap_err(),
            SkillInstallationWorkflowError::PreviewMismatch { .. }
        ));
    }

    #[test]
    fn package_and_installation_revisions_keep_distinct_update_contracts() {
        let fixture = tempdir().unwrap();
        let store = fixture.path().join("store");
        let source = fixture.path().join("source");
        write_skill(&source, "VERSION_ONE", false);
        let workflow = workflow(&store);

        let install_preview = workflow
            .inspect_local_directory_install(SkillPreparationId::new(), installation_id(), &source)
            .unwrap();
        let install = workflow
            .commit(&SkillInstallationCommitRequest::new(
                install_preview.preparation_id().clone(),
                install_preview.preview_revision().clone(),
            ))
            .unwrap();
        let installed_package_revision = install.mutation().package_revision().unwrap().clone();
        let installed_revision = install.mutation().installation_revision().unwrap().clone();
        assert_eq!(
            installed_package_revision,
            *install_preview.package().revision()
        );

        write_skill(&source, "VERSION_TWO", false);
        let update_preview = workflow
            .inspect_local_directory_update(
                SkillPreparationId::new(),
                install.mutation().skill_id().clone(),
                installed_revision.clone(),
                &source,
            )
            .unwrap();
        assert!(update_preview.content_changed());
        assert!(!update_preview.source_changed());
        assert_eq!(
            update_preview.expected_revision(),
            Some(&installed_revision)
        );
        assert_ne!(
            update_preview.package().revision(),
            &installed_package_revision
        );
        let update = workflow
            .commit(&SkillInstallationCommitRequest::new(
                update_preview.preparation_id().clone(),
                update_preview.preview_revision().clone(),
            ))
            .unwrap();
        assert_eq!(update.preview(), &update_preview);
        assert_eq!(
            update.preview().operation(),
            SkillInstallationOperation::Update
        );
        let updated_package_revision = update.mutation().package_revision().unwrap().clone();
        let updated_revision = update.mutation().installation_revision().unwrap().clone();
        assert_eq!(
            updated_package_revision,
            *update_preview.package().revision()
        );

        // The lifecycle revision returned by commit is accepted directly by
        // the following exact update CAS.
        write_skill(&source, "VERSION_THREE", false);
        let next_preview = workflow
            .inspect_local_directory_update(
                SkillPreparationId::new(),
                update.mutation().skill_id().clone(),
                updated_revision,
                &source,
            )
            .unwrap();
        workflow
            .commit(&SkillInstallationCommitRequest::new(
                next_preview.preparation_id().clone(),
                next_preview.preview_revision().clone(),
            ))
            .unwrap();
    }

    #[test]
    fn preparation_identity_cannot_be_rebound_to_a_different_request() {
        let fixture = tempdir().unwrap();
        let source = fixture.path().join("source");
        write_skill(&source, "ONE", false);
        let workflow = workflow(&fixture.path().join("store"));
        let preparation_id = SkillPreparationId::new();
        let first = SkillInstallationPreparationRequest::install(
            preparation_id.clone(),
            installation_id(),
            SkillAcquisitionSource::local_directory(&source),
        );
        workflow.inspect(&first).unwrap();

        let different = SkillInstallationPreparationRequest::install(
            preparation_id,
            SkillInstallationId::new(),
            SkillAcquisitionSource::local_directory(&source),
        );
        assert!(matches!(
            workflow.inspect(&different).unwrap_err(),
            SkillInstallationWorkflowError::PreparationConflict { .. }
        ));
    }

    #[test]
    fn resource_summary_and_script_acknowledgement_gate_the_commit() {
        let fixture = tempdir().unwrap();
        let source = fixture.path().join("source");
        let store = fixture.path().join("store");
        write_skill(&source, "RESOURCEFUL", true);
        let workflow = workflow(&store);
        let preparation_id = SkillPreparationId::new();
        let preview = workflow
            .inspect_local_directory_install(preparation_id.clone(), installation_id(), &source)
            .unwrap();

        assert_eq!(preview.package().resources().resource_count(), 3);
        assert_eq!(preview.package().resources().reference_count(), 1);
        assert_eq!(preview.package().resources().asset_count(), 1);
        assert_eq!(preview.package().resources().script_count(), 1);
        assert_eq!(preview.warnings().len(), 2);
        assert_eq!(
            preview.warnings()[0].code(),
            SkillInstallationWarningCode::ResourcesNotExposed
        );
        assert_eq!(
            preview.warnings()[1].code(),
            SkillInstallationWarningCode::ContainsScripts
        );

        let error = workflow
            .commit(&SkillInstallationCommitRequest::new(
                preparation_id.clone(),
                preview.preview_revision().clone(),
            ))
            .unwrap_err();
        assert_eq!(
            error.missing_warning_acknowledgements(),
            Some(&[SkillInstallationWarningCode::ContainsScripts][..])
        );
        assert!(SkillsService::new()
            .with_installed_source(&store)
            .unwrap()
            .list()
            .unwrap()
            .skills()
            .is_empty());

        workflow
            .commit(
                &SkillInstallationCommitRequest::new(
                    preparation_id,
                    preview.preview_revision().clone(),
                )
                .acknowledge(SkillInstallationWarningCode::ContainsScripts),
            )
            .unwrap();
    }

    #[test]
    fn registered_second_adapter_enters_the_same_transaction() {
        struct FixtureAdapter {
            provider: SkillAcquisitionProvider,
            calls: Arc<AtomicUsize>,
        }

        impl SkillAcquisitionAdapter for FixtureAdapter {
            fn provider(&self) -> SkillAcquisitionProvider {
                self.provider.clone()
            }

            fn acquire(
                &self,
                source: &SkillAcquisitionSource,
            ) -> Result<PreparedSkillAcquisition, SkillAcquisitionAdapterError> {
                self.calls.fetch_add(1, Ordering::SeqCst);
                let marker = source.adapter_request().ok_or_else(|| {
                    SkillAcquisitionAdapterError::invalid_request("missing fixture request")
                })?;
                let origin = SkillPackageOrigin::new("fixture-adapter", "fixture")
                    .map_err(|_| SkillAcquisitionAdapterError::unavailable("invalid origin"))?;
                let package = PreparedSkillPackage::from_bytes(
                    format!(
                        "---\nname: adapter-fixture\ndescription: Adapter fixture.\n---\n# Instructions\n{}\n",
                        String::from_utf8_lossy(marker)
                    )
                    .into_bytes(),
                    origin,
                )
                .map_err(SkillAcquisitionAdapterError::from)?;
                let authority =
                    SkillInstallationAuthority::new("fixture-adapter", 1, "fixture-authority")
                        .map_err(|_| {
                            SkillAcquisitionAdapterError::unavailable("invalid provenance")
                        })?;
                Ok(PreparedSkillAcquisition::new(
                    package,
                    SkillInstallationProvenance::new(authority, None),
                ))
            }
        }

        let fixture = tempdir().unwrap();
        let provider = SkillAcquisitionProvider::parse("fixture-adapter").unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let mut workflow = workflow(&fixture.path().join("store"));
        workflow
            .register_adapter(Arc::new(FixtureAdapter {
                provider: provider.clone(),
                calls: Arc::clone(&calls),
            }))
            .unwrap();
        let preparation_id = SkillPreparationId::new();
        let request = SkillInstallationPreparationRequest::install(
            preparation_id.clone(),
            installation_id(),
            SkillAcquisitionSource::adapter(provider, b"FROM_ADAPTER".to_vec()).unwrap(),
        );

        let first = workflow.inspect(&request).unwrap();
        let second = workflow.inspect(&request).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.acquisition().provider(), "fixture-adapter");
        assert_eq!(first.acquisition().reference(), "fixture");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        workflow
            .commit(&SkillInstallationCommitRequest::new(
                preparation_id,
                first.preview_revision().clone(),
            ))
            .unwrap();
    }

    #[test]
    fn adapter_panic_releases_the_preparation_for_an_idempotent_retry() {
        const PROVIDER: &str = "panic-once-adapter";

        struct PanicOnceAdapter {
            calls: AtomicUsize,
            next: PreparedSkillAcquisition,
        }

        impl SkillAcquisitionAdapter for PanicOnceAdapter {
            fn provider(&self) -> SkillAcquisitionProvider {
                SkillAcquisitionProvider::parse(PROVIDER).unwrap()
            }

            fn acquire(
                &self,
                _source: &SkillAcquisitionSource,
            ) -> Result<PreparedSkillAcquisition, SkillAcquisitionAdapterError> {
                if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                    panic!("injected adapter panic");
                }
                Ok(self.next.clone())
            }
        }

        let fixture = tempdir().unwrap();
        let provider = SkillAcquisitionProvider::parse(PROVIDER).unwrap();
        let mut workflow = workflow(&fixture.path().join("store"));
        workflow
            .register_adapter(Arc::new(PanicOnceAdapter {
                calls: AtomicUsize::new(0),
                next: provider_acquisition("RECOVERED", PROVIDER, "authority", None),
            }))
            .unwrap();
        let request = SkillInstallationPreparationRequest::install(
            SkillPreparationId::new(),
            installation_id(),
            SkillAcquisitionSource::adapter(provider, b"fixture".to_vec()).unwrap(),
        );

        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = workflow.inspect(&request);
        }));
        assert!(panic.is_err());
        let preview = workflow
            .inspect(&request)
            .expect("the same preparation must be retryable after an adapter panic");
        assert_eq!(preview.package().name(), "refresh-fixture");
    }

    #[test]
    fn stale_preparing_recovery_cannot_remove_a_later_attempt() {
        let fixture = tempdir().unwrap();
        let workflow = workflow(&fixture.path().join("store"));
        let preparation_id = SkillPreparationId::new();
        let request = SkillInstallationPreparationRequest::install(
            preparation_id.clone(),
            installation_id(),
            SkillAcquisitionSource::local_directory(fixture.path().join("skill")),
        );

        let first_attempt = {
            let now = workflow.sessions.now();
            let mut state = workflow.sessions.lock().unwrap();
            workflow
                .reserve_preparation(&mut state, request.clone(), now.monotonic)
                .unwrap()
        };
        let stale_recovery =
            PreparingSlotRecovery::new(&workflow.sessions, &preparation_id, first_attempt);

        let second_attempt = {
            let now = workflow.sessions.now();
            let mut state = workflow.sessions.lock().unwrap();
            state.preparations.remove(&preparation_id);
            workflow
                .reserve_preparation(&mut state, request, now.monotonic)
                .unwrap()
        };
        assert_ne!(first_attempt, second_attempt);

        drop(stale_recovery);

        let state = workflow.sessions.lock().unwrap();
        assert!(matches!(
            state.preparations.get(&preparation_id),
            Some(PreparationSlot::Preparing { attempt_id, .. })
                if *attempt_id == second_attempt
        ));
    }

    #[test]
    fn commit_panic_recovery_restores_the_exact_ready_snapshot() {
        let fixture = tempdir().unwrap();
        let directory = fixture.path().join("skill");
        write_skill(&directory, "RECOVER_COMMIT", false);
        let workflow = workflow(&fixture.path().join("store"));
        let preparation_id = SkillPreparationId::new();
        let preparation = SkillInstallationPreparationRequest::install(
            preparation_id.clone(),
            installation_id(),
            SkillAcquisitionSource::local_directory(directory),
        );
        let preview = workflow.inspect(&preparation).unwrap();

        let (stored_request, acquisition, snapshot_bytes, expires_at, attempt_id) = {
            let mut state = workflow.sessions.lock().unwrap();
            let slot = state.preparations.remove(&preparation_id).unwrap();
            let PreparationSlot::Ready {
                request,
                preview: stored_preview,
                acquisition,
                snapshot_bytes,
                expires_at,
            } = slot
            else {
                panic!("inspection must leave a ready preparation");
            };
            assert_eq!(stored_preview, preview);
            let attempt_id = state.next_preparation_attempt();
            state.preparations.insert(
                preparation_id.clone(),
                PreparationSlot::Committing {
                    request: request.clone(),
                    attempt_id,
                    snapshot_bytes,
                },
            );
            (request, acquisition, snapshot_bytes, expires_at, attempt_id)
        };

        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _recovery = CommittingSlotRecovery::new(
                &workflow.sessions,
                attempt_id,
                &stored_request,
                &preview,
                &acquisition,
                snapshot_bytes,
                expires_at,
            );
            panic!("injected commit panic");
        }));
        assert!(panic.is_err());

        let committed = workflow
            .commit(&SkillInstallationCommitRequest::new(
                preparation_id,
                preview.preview_revision().clone(),
            ))
            .expect("commit retry must consume the restored exact snapshot");
        assert_eq!(committed.preview(), &preview);
    }

    #[test]
    fn stale_committing_recovery_cannot_overwrite_a_later_attempt() {
        let fixture = tempdir().unwrap();
        let directory = fixture.path().join("skill");
        write_skill(&directory, "STALE_COMMIT_GUARD", false);
        let workflow = workflow(&fixture.path().join("store"));
        let preparation_id = SkillPreparationId::new();
        let preparation = SkillInstallationPreparationRequest::install(
            preparation_id.clone(),
            installation_id(),
            SkillAcquisitionSource::local_directory(directory),
        );
        let preview = workflow.inspect(&preparation).unwrap();

        let (stored_request, acquisition, snapshot_bytes, expires_at, first_attempt) = {
            let mut state = workflow.sessions.lock().unwrap();
            let slot = state.preparations.remove(&preparation_id).unwrap();
            let PreparationSlot::Ready {
                request,
                acquisition,
                snapshot_bytes,
                expires_at,
                ..
            } = slot
            else {
                panic!("inspection must leave a ready preparation");
            };
            let attempt_id = state.next_preparation_attempt();
            state.preparations.insert(
                preparation_id.clone(),
                PreparationSlot::Committing {
                    request: request.clone(),
                    attempt_id,
                    snapshot_bytes,
                },
            );
            (request, acquisition, snapshot_bytes, expires_at, attempt_id)
        };
        let stale_recovery = CommittingSlotRecovery::new(
            &workflow.sessions,
            first_attempt,
            &stored_request,
            &preview,
            &acquisition,
            snapshot_bytes,
            expires_at,
        );

        let second_attempt = {
            let mut state = workflow.sessions.lock().unwrap();
            let attempt_id = state.next_preparation_attempt();
            state.preparations.insert(
                preparation_id.clone(),
                PreparationSlot::Committing {
                    request: stored_request,
                    attempt_id,
                    snapshot_bytes,
                },
            );
            attempt_id
        };
        assert_ne!(first_attempt, second_attempt);

        drop(stale_recovery);

        let state = workflow.sessions.lock().unwrap();
        assert!(matches!(
            state.preparations.get(&preparation_id),
            Some(PreparationSlot::Committing { attempt_id, .. })
                if *attempt_id == second_attempt
        ));
    }

    #[test]
    fn registered_adapter_cannot_cross_its_provider_ownership_boundary() {
        let mismatched = [
            acquisition_with_provider_parts(
                "ORIGIN_MISMATCH",
                "other-provider",
                REFRESH_FIXTURE_PROVIDER,
                None,
            ),
            acquisition_with_provider_parts(
                "AUTHORITY_MISMATCH",
                REFRESH_FIXTURE_PROVIDER,
                "other-provider",
                None,
            ),
            acquisition_with_provider_parts(
                "REFRESH_MISMATCH",
                REFRESH_FIXTURE_PROVIDER,
                REFRESH_FIXTURE_PROVIDER,
                Some("other-provider"),
            ),
        ];

        for acquisition in mismatched {
            let fixture = tempdir().unwrap();
            let calls = Arc::new(AtomicUsize::new(0));
            let mut workflow = workflow(&fixture.path().join("store"));
            workflow
                .register_adapter(Arc::new(RefreshFixtureAdapter {
                    calls: Arc::clone(&calls),
                    next: acquisition,
                }))
                .unwrap();
            let provider = SkillAcquisitionProvider::parse(REFRESH_FIXTURE_PROVIDER).unwrap();
            let request = SkillInstallationPreparationRequest::install(
                SkillPreparationId::new(),
                installation_id(),
                SkillAcquisitionSource::adapter(provider.clone(), b"fixture".to_vec()).unwrap(),
            );

            let error = workflow.inspect(&request).unwrap_err();
            match error {
                SkillInstallationWorkflowError::Acquisition {
                    provider: actual,
                    source,
                } => {
                    assert_eq!(actual, provider);
                    assert_eq!(source.code(), SkillAcquisitionAdapterErrorCode::Unavailable);
                    assert!(!source.reason().contains("other-provider"));
                }
                other => panic!("unexpected provider-binding error: {other:?}"),
            }
            assert_eq!(calls.load(Ordering::SeqCst), 1);
        }
    }

    struct ManualClock {
        elapsed_ms: AtomicU64,
        started_unix_ms: u64,
    }

    impl Default for ManualClock {
        fn default() -> Self {
            Self {
                elapsed_ms: AtomicU64::new(0),
                started_unix_ms: 1_700_000_000_000,
            }
        }
    }

    impl ManualClock {
        fn advance(&self, duration: Duration) {
            self.elapsed_ms.fetch_add(
                u64::try_from(duration.as_millis()).unwrap(),
                Ordering::SeqCst,
            );
        }
    }

    impl SessionClock for ManualClock {
        fn now(&self) -> super::super::installation_session::SessionTime {
            let elapsed_ms = self.elapsed_ms.load(Ordering::SeqCst);
            super::super::installation_session::SessionTime {
                monotonic: Duration::from_millis(elapsed_ms),
                unix_ms: self.started_unix_ms.saturating_add(elapsed_ms),
            }
        }
    }

    #[test]
    fn retry_returns_the_same_absolute_expiration_and_preview_revision() {
        let fixture = tempdir().unwrap();
        let source = fixture.path().join("source");
        write_skill(&source, "STABLE_PREVIEW", false);
        let config = SkillInstallationWorkflowConfig::new(
            2,
            MAX_SKILL_PACKAGE_BYTES,
            Duration::from_secs(10),
        )
        .unwrap();
        let clock = Arc::new(ManualClock::default());
        let workflow = SkillInstallationWorkflow::with_clock(
            SkillInstallationService::new(fixture.path().join("store")).unwrap(),
            config,
            clock.clone(),
        );
        let request = SkillInstallationPreparationRequest::install(
            SkillPreparationId::new(),
            installation_id(),
            SkillAcquisitionSource::local_directory(&source),
        );

        let first = workflow.inspect(&request).unwrap();
        assert_eq!(first.expires_at_unix_ms(), 1_700_000_010_000);
        clock.advance(Duration::from_secs(5));
        let retry = workflow.inspect(&request).unwrap();
        assert_eq!(retry.expires_at_unix_ms(), first.expires_at_unix_ms());
        assert_eq!(retry.preview_revision(), first.preview_revision());
    }

    #[test]
    fn failed_commit_does_not_extend_the_original_preview_deadline() {
        let fixture = tempdir().unwrap();
        let store = fixture.path().join("store");
        let source = fixture.path().join("source");
        let conflicting = fixture.path().join("conflicting");
        write_skill(&source, "PREPARED_TARGET", false);
        write_skill(&conflicting, "OTHER_PACKAGE", false);
        let config = SkillInstallationWorkflowConfig::new(
            2,
            MAX_SKILL_PACKAGE_BYTES,
            Duration::from_secs(10),
        )
        .unwrap();
        let clock = Arc::new(ManualClock::default());
        let workflow = SkillInstallationWorkflow::with_clock(
            SkillInstallationService::new(&store).unwrap(),
            config,
            clock.clone(),
        );
        let preview = workflow
            .inspect_local_directory_install(SkillPreparationId::new(), installation_id(), &source)
            .unwrap();

        let competing_package =
            PreparedSkillPackage::from_local_directory(&conflicting, "competing-test").unwrap();
        SkillInstallationService::new(&store)
            .unwrap()
            .install_prepared(installation_id(), competing_package)
            .unwrap();
        clock.advance(Duration::from_secs(9));
        let commit = SkillInstallationCommitRequest::new(
            preview.preparation_id().clone(),
            preview.preview_revision().clone(),
        );
        assert!(matches!(
            workflow.commit(&commit).unwrap_err(),
            SkillInstallationWorkflowError::Installation { .. }
        ));

        clock.advance(Duration::from_secs(2));
        assert!(matches!(
            workflow.commit(&commit).unwrap_err(),
            SkillInstallationWorkflowError::PreparationNotFoundOrExpired { .. }
        ));
    }

    #[test]
    fn expiration_and_cancellation_release_snapshot_capacity() {
        let fixture = tempdir().unwrap();
        let first_source = fixture.path().join("first");
        let second_source = fixture.path().join("second");
        write_skill(&first_source, "FIRST", false);
        write_skill(&second_source, "SECOND", false);
        let config = SkillInstallationWorkflowConfig::new(
            1,
            MAX_SKILL_PACKAGE_BYTES,
            Duration::from_secs(10),
        )
        .unwrap();
        let clock = Arc::new(ManualClock::default());
        let workflow = SkillInstallationWorkflow::with_clock(
            SkillInstallationService::new(fixture.path().join("store")).unwrap(),
            config,
            clock.clone(),
        );
        let first_id = SkillPreparationId::new();
        let first_preview = workflow
            .inspect_local_directory_install(first_id.clone(), installation_id(), &first_source)
            .unwrap();
        assert!(matches!(
            workflow
                .inspect_local_directory_install(
                    SkillPreparationId::new(),
                    SkillInstallationId::new(),
                    &second_source,
                )
                .unwrap_err(),
            SkillInstallationWorkflowError::PreparationCapacityExceeded { .. }
        ));
        assert_eq!(
            workflow.cancel(&first_id).unwrap(),
            SkillPreparationCancellation::Cancelled
        );
        assert_eq!(
            workflow.cancel(&first_id).unwrap(),
            SkillPreparationCancellation::AlreadyCancelled
        );

        clock.advance(Duration::from_secs(11));
        workflow
            .inspect_local_directory_install(
                SkillPreparationId::new(),
                SkillInstallationId::new(),
                &second_source,
            )
            .unwrap();
        assert!(matches!(
            workflow
                .commit(&SkillInstallationCommitRequest::new(
                    first_id,
                    first_preview.preview_revision().clone(),
                ))
                .unwrap_err(),
            SkillInstallationWorkflowError::PreparationNotFoundOrExpired { .. }
        ));
    }

    #[test]
    fn debug_representations_do_not_disclose_acquisition_paths_or_payloads() {
        let secret_path = PathBuf::from("/private/secret/skill");
        let source = SkillAcquisitionSource::local_directory(&secret_path);
        let request = SkillInstallationPreparationRequest::install(
            SkillPreparationId::new(),
            installation_id(),
            source,
        );
        let debug = format!("{request:?}");
        assert!(!debug.contains(secret_path.to_str().unwrap()));
        assert!(debug.contains("[redacted]"));

        let provider = SkillAcquisitionProvider::parse("fixture").unwrap();
        let secret = b"https://token@example.invalid/private".to_vec();
        let source = SkillAcquisitionSource::adapter(provider, secret.clone()).unwrap();
        let debug = format!("{source:?}");
        assert!(!debug.contains(&String::from_utf8(secret).unwrap()));
    }

    #[test]
    fn canonical_uuid_and_configuration_contracts_are_strict() {
        assert!(SkillPreparationId::parse("01234567-89ab-4def-8123-456789abcdef").is_ok());
        assert!(SkillPreparationId::parse("01234567-89AB-4DEF-8123-456789ABCDEF").is_err());
        assert!(SkillPreparationId::parse(Uuid::nil().hyphenated().to_string()).is_err());
        assert!(SkillInstallationWorkflowConfig::new(
            0,
            MAX_SKILL_PACKAGE_BYTES,
            Duration::from_secs(1),
        )
        .is_err());
        assert!(
            SkillInstallationWorkflowConfig::new(1, MAX_SKILL_PACKAGE_BYTES, Duration::ZERO,)
                .is_err()
        );
    }
}
