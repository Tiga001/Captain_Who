//! Two-phase orchestration for acquiring and committing managed Skills.
//!
//! Inspection captures one exact, validated [`PreparedSkillPackage`] and
//! returns a safe preview. Commit consumes that snapshot; it never re-reads
//! the acquisition source. A bounded, expiring registry makes retries
//! idempotent without turning preparation IDs into permanent server state.
//!
//! Acquisition is an adapter boundary. Local directories are built in, while
//! future Git, archive, or registry adapters can register another provider and
//! still enter the same preview, acknowledgement, and installation transaction.

use super::installation_service::{
    SkillInstallationMutation, SkillInstallationOperation, SkillInstallationService,
    SkillInstallationServiceError,
};
use super::installed::USER_INSTALLED_SKILL_SOURCE_ID;
use super::model::{SkillId, SkillInstallationId, SkillResourceKind, SkillRevision, SkillSourceId};
use super::package::MAX_SKILL_PACKAGE_BYTES;
use super::prepared::{PreparedSkillPackage, SkillPackagePreparationError};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use uuid::Uuid;

const LOCAL_DIRECTORY_PROVIDER: &str = "local-directory";
const LOCAL_DIRECTORY_ORIGIN_REFERENCE: &str = "user-selected-directory";
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

    pub fn provider(&self) -> SkillAcquisitionProvider {
        match self {
            Self::LocalDirectory { .. } => SkillAcquisitionProvider::local_directory(),
            Self::Adapter { provider, .. } => provider.clone(),
        }
    }

    pub fn local_directory_path(&self) -> Option<&Path> {
        match self {
            Self::LocalDirectory { directory } => Some(directory),
            Self::Adapter { .. } => None,
        }
    }

    pub fn adapter_request(&self) -> Option<&[u8]> {
        match self {
            Self::LocalDirectory { .. } => None,
            Self::Adapter { request, .. } => Some(request),
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

    /// Converts provider input into an owned, fully validated package.
    /// Implementations must return sanitized errors: paths, credentials, and
    /// opaque request contents must not be included in diagnostic messages.
    fn acquire(
        &self,
        source: &SkillAcquisitionSource,
    ) -> Result<PreparedSkillPackage, SkillAcquisitionAdapterError>;
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
        expected_revision: SkillRevision,
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
        expected_revision: SkillRevision,
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
    expected_revision: Option<SkillRevision>,
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

    pub fn expected_revision(&self) -> Option<&SkillRevision> {
        self.expected_revision.as_ref()
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
    registry: Mutex<PreparationRegistry>,
    changed: Condvar,
    config: SkillInstallationWorkflowConfig,
    clock: Arc<dyn WorkflowClock>,
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
        Self::with_clock(
            installation_service,
            config,
            Arc::new(SystemWorkflowClock::new()),
        )
    }

    fn with_clock(
        installation_service: SkillInstallationService,
        config: SkillInstallationWorkflowConfig,
        clock: Arc<dyn WorkflowClock>,
    ) -> Self {
        let mut adapters: BTreeMap<SkillAcquisitionProvider, Arc<dyn SkillAcquisitionAdapter>> =
            BTreeMap::new();
        let local: Arc<dyn SkillAcquisitionAdapter> = Arc::new(LocalDirectoryAcquisitionAdapter);
        adapters.insert(local.provider(), local);
        Self {
            installation_service,
            adapters,
            registry: Mutex::new(PreparationRegistry::default()),
            changed: Condvar::new(),
            config,
            clock,
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
        expected_revision: SkillRevision,
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
        let provider = request.source.provider();
        let adapter = self.adapters.get(&provider).ok_or_else(|| {
            SkillInstallationWorkflowError::UnknownAcquisitionProvider {
                provider: provider.clone(),
            }
        })?;

        loop {
            let now = self.clock.now();
            let mut registry = self.lock_registry("inspect Skill preparation")?;
            registry.prune_expired(now.monotonic);
            match registry.entries.get(request.preparation_id()) {
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
                    registry.reserve(request.clone(), now.monotonic, &self.config)?;
                    break;
                }
            }
        }

        let acquired = adapter.acquire(&request.source);
        let now = self.clock.now();
        let mut registry = self.lock_registry("finish Skill preparation")?;
        let result = match acquired {
            Ok(package) => {
                let preview = build_preview(
                    request,
                    &action,
                    &package,
                    now.unix_ms
                        .saturating_add(duration_millis(self.config.preparation_ttl)),
                );
                registry.entries.insert(
                    request.preparation_id.clone(),
                    PreparationSlot::Ready {
                        request: request.clone(),
                        preview: preview.clone(),
                        snapshot_bytes: package_snapshot_bytes(&package),
                        package,
                        expires_at: now.monotonic.saturating_add(self.config.preparation_ttl),
                    },
                );
                Ok(preview)
            }
            Err(source) => {
                registry.entries.remove(request.preparation_id());
                Err(SkillInstallationWorkflowError::Acquisition {
                    provider,
                    source: Box::new(source),
                })
            }
        };
        self.changed.notify_all();
        result
    }

    /// Commits the exact package captured by `inspect`. Required warnings
    /// must be acknowledged before any store mutation starts.
    pub fn commit(
        &self,
        request: &SkillInstallationCommitRequest,
    ) -> Result<SkillInstallationCommitResult, SkillInstallationWorkflowError> {
        let (preparation_request, preview, package, action, snapshot_bytes, original_expires_at) = loop {
            let now = self.clock.now();
            let mut registry = self.lock_registry("commit Skill preparation")?;
            registry.prune_expired(now.monotonic);
            let Some(slot) = registry.entries.get(request.preparation_id()) else {
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
                    package,
                    snapshot_bytes,
                    expires_at,
                } => {
                    ensure_preview_matches(preview, request)?;
                    ensure_warnings_acknowledged(preview, request)?;
                    let values = (
                        preparation_request.clone(),
                        preview.clone(),
                        package.clone(),
                        PreparedAction::from_intent(&preparation_request.intent)?,
                        *snapshot_bytes,
                        *expires_at,
                    );
                    registry.entries.insert(
                        request.preparation_id.clone(),
                        PreparationSlot::Committing {
                            request: values.0.clone(),
                            snapshot_bytes: values.4,
                        },
                    );
                    break values;
                }
            }
        };

        let committed = match action {
            PreparedAction::Install {
                installation_id, ..
            } => self
                .installation_service
                .install_prepared(installation_id, package.clone()),
            PreparedAction::Update {
                installation_id,
                expected_revision,
                ..
            } => self.installation_service.update_prepared(
                installation_id,
                expected_revision,
                package.clone(),
            ),
        };

        let now = self.clock.now();
        let mut registry = self.lock_registry("finish Skill commit")?;
        let result = match committed {
            Ok(mutation) => {
                let result_preview = preview.clone();
                registry.entries.insert(
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
                registry.entries.insert(
                    request.preparation_id.clone(),
                    PreparationSlot::Ready {
                        request: preparation_request,
                        preview,
                        snapshot_bytes,
                        package,
                        expires_at: original_expires_at,
                    },
                );
                Err(SkillInstallationWorkflowError::Installation {
                    preparation_id: request.preparation_id.clone(),
                    source: Box::new(source),
                })
            }
        };
        self.changed.notify_all();
        result
    }

    pub fn cancel(
        &self,
        preparation_id: &SkillPreparationId,
    ) -> Result<SkillPreparationCancellation, SkillInstallationWorkflowError> {
        let now = self.clock.now();
        let mut registry = self.lock_registry("cancel Skill preparation")?;
        registry.prune_expired(now.monotonic);
        let Some(slot) = registry.entries.get(preparation_id) else {
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
                registry.entries.insert(
                    preparation_id.clone(),
                    PreparationSlot::Cancelled {
                        request,
                        expires_at: now.monotonic.saturating_add(self.config.preparation_ttl),
                    },
                );
                self.changed.notify_all();
                Ok(SkillPreparationCancellation::Cancelled)
            }
        }
    }

    fn lock_registry(
        &self,
        operation: &'static str,
    ) -> Result<MutexGuard<'_, PreparationRegistry>, SkillInstallationWorkflowError> {
        self.registry
            .lock()
            .map_err(|_| SkillInstallationWorkflowError::Internal {
                operation,
                reason: "preparation registry lock is poisoned".to_string(),
            })
    }

    fn wait_for_change<'a>(
        &self,
        registry: MutexGuard<'a, PreparationRegistry>,
        operation: &'static str,
    ) -> Result<MutexGuard<'a, PreparationRegistry>, SkillInstallationWorkflowError> {
        self.changed
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
    ) -> Result<PreparedSkillPackage, SkillAcquisitionAdapterError> {
        let directory = source.local_directory_path().ok_or_else(|| {
            SkillAcquisitionAdapterError::invalid_request(
                "local-directory provider received a different source envelope",
            )
        })?;
        PreparedSkillPackage::from_local_directory(directory, LOCAL_DIRECTORY_ORIGIN_REFERENCE)
            .map_err(Into::into)
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
        expected_revision: SkillRevision,
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

    fn expected_revision(&self) -> Option<&SkillRevision> {
        match self {
            Self::Install { .. } => None,
            Self::Update {
                expected_revision, ..
            } => Some(expected_revision),
        }
    }
}

#[derive(Default)]
struct PreparationRegistry {
    entries: BTreeMap<SkillPreparationId, PreparationSlot>,
}

impl PreparationRegistry {
    fn prune_expired(&mut self, now: Duration) {
        self.entries.retain(|_, slot| !slot.is_expired(now));
    }

    fn reserve(
        &mut self,
        request: SkillInstallationPreparationRequest,
        now: Duration,
        config: &SkillInstallationWorkflowConfig,
    ) -> Result<(), SkillInstallationWorkflowError> {
        if self.entries.len() >= config.max_preparations {
            return Err(
                SkillInstallationWorkflowError::PreparationCapacityExceeded {
                    max_preparations: config.max_preparations,
                },
            );
        }
        let resident = self
            .entries
            .values()
            .map(PreparationSlot::reserved_snapshot_bytes)
            .sum::<usize>();
        if resident.saturating_add(MAX_SKILL_PACKAGE_BYTES) > config.max_snapshot_bytes {
            return Err(
                SkillInstallationWorkflowError::PreparationMemoryCapacityExceeded {
                    max_snapshot_bytes: config.max_snapshot_bytes,
                },
            );
        }
        self.entries.insert(
            request.preparation_id.clone(),
            PreparationSlot::Preparing {
                request,
                _started_at: now,
            },
        );
        Ok(())
    }
}

enum PreparationSlot {
    Preparing {
        request: SkillInstallationPreparationRequest,
        _started_at: Duration,
    },
    Ready {
        request: SkillInstallationPreparationRequest,
        preview: SkillInstallationPreview,
        package: PreparedSkillPackage,
        snapshot_bytes: usize,
        expires_at: Duration,
    },
    Committing {
        request: SkillInstallationPreparationRequest,
        snapshot_bytes: usize,
    },
    Committed {
        request: SkillInstallationPreparationRequest,
        preview: SkillInstallationPreview,
        mutation: SkillInstallationMutation,
        expires_at: Duration,
    },
    Cancelled {
        request: SkillInstallationPreparationRequest,
        expires_at: Duration,
    },
}

impl PreparationSlot {
    fn request(&self) -> &SkillInstallationPreparationRequest {
        match self {
            Self::Preparing { request, .. }
            | Self::Ready { request, .. }
            | Self::Committing { request, .. }
            | Self::Committed { request, .. }
            | Self::Cancelled { request, .. } => request,
        }
    }

    fn is_expired(&self, now: Duration) -> bool {
        match self {
            Self::Ready { expires_at, .. }
            | Self::Committed { expires_at, .. }
            | Self::Cancelled { expires_at, .. } => now >= *expires_at,
            Self::Preparing { .. } | Self::Committing { .. } => false,
        }
    }

    fn reserved_snapshot_bytes(&self) -> usize {
        match self {
            Self::Preparing { .. } => MAX_SKILL_PACKAGE_BYTES,
            Self::Ready { snapshot_bytes, .. } | Self::Committing { snapshot_bytes, .. } => {
                *snapshot_bytes
            }
            Self::Committed { .. } | Self::Cancelled { .. } => 0,
        }
    }
}

fn build_preview(
    request: &SkillInstallationPreparationRequest,
    action: &PreparedAction,
    package: &PreparedSkillPackage,
    expires_at_unix_ms: u64,
) -> SkillInstallationPreview {
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
    let acquisition = SkillAcquisitionPresentation {
        provider: package.origin().provider().to_string(),
        reference: package.origin().reference().to_string(),
    };
    let preview_revision = preview_revision(
        request,
        action,
        &acquisition,
        &package_preview,
        &warnings,
        expires_at_unix_ms,
    );
    SkillInstallationPreview {
        preparation_id: request.preparation_id.clone(),
        preview_revision,
        operation: action.operation(),
        installation_id: action.installation_id().clone(),
        skill_id: action.skill_id().clone(),
        expected_revision: action.expected_revision().cloned(),
        acquisition,
        package: package_preview,
        warnings: warnings.into(),
        expires_at_unix_ms,
    }
}

fn preview_revision(
    request: &SkillInstallationPreparationRequest,
    action: &PreparedAction,
    acquisition: &SkillAcquisitionPresentation,
    package: &SkillInstallationPackagePreview,
    warnings: &[SkillInstallationWarning],
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

fn package_snapshot_bytes(package: &PreparedSkillPackage) -> usize {
    package.resource_index().entries().iter().fold(
        package.source_bytes().len(),
        |total, resource| {
            total.saturating_add(usize::try_from(resource.byte_length()).unwrap_or(usize::MAX))
        },
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

fn duration_millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[derive(Clone, Copy)]
struct WorkflowTime {
    monotonic: Duration,
    unix_ms: u64,
}

trait WorkflowClock: Send + Sync {
    fn now(&self) -> WorkflowTime;
}

struct SystemWorkflowClock {
    started: Instant,
    started_unix_ms: u64,
}

impl SystemWorkflowClock {
    fn new() -> Self {
        Self {
            started: Instant::now(),
            started_unix_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(duration_millis)
                .unwrap_or(0),
        }
    }
}

impl WorkflowClock for SystemWorkflowClock {
    fn now(&self) -> WorkflowTime {
        let monotonic = self.started.elapsed();
        WorkflowTime {
            monotonic,
            unix_ms: self
                .started_unix_ms
                .saturating_add(duration_millis(monotonic)),
        }
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
    use super::super::{SkillInstallationOutcome, SkillsService};
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
    fn package_revision_is_the_installation_cas_revision_across_updates() {
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
        let installed_revision = install.mutation().revision().unwrap().clone();
        assert_eq!(installed_revision, *install_preview.package().revision());

        write_skill(&source, "VERSION_TWO", false);
        let update_preview = workflow
            .inspect_local_directory_update(
                SkillPreparationId::new(),
                install.mutation().skill_id().clone(),
                installed_revision.clone(),
                &source,
            )
            .unwrap();
        assert_eq!(
            update_preview.expected_revision(),
            Some(&installed_revision)
        );
        assert_ne!(update_preview.package().revision(), &installed_revision);
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
        let updated_revision = update.mutation().revision().unwrap().clone();
        assert_eq!(updated_revision, *update_preview.package().revision());

        // The returned revision is not synthetic: it is accepted directly by
        // the installer's package-revision CAS on the following update.
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
            ) -> Result<PreparedSkillPackage, SkillAcquisitionAdapterError> {
                self.calls.fetch_add(1, Ordering::SeqCst);
                let marker = source.adapter_request().ok_or_else(|| {
                    SkillAcquisitionAdapterError::invalid_request("missing fixture request")
                })?;
                let origin = SkillPackageOrigin::new("fixture-adapter", "fixture")
                    .map_err(|_| SkillAcquisitionAdapterError::unavailable("invalid origin"))?;
                PreparedSkillPackage::from_bytes(
                    format!(
                        "---\nname: adapter-fixture\ndescription: Adapter fixture.\n---\n# Instructions\n{}\n",
                        String::from_utf8_lossy(marker)
                    )
                    .into_bytes(),
                    origin,
                )
                .map_err(Into::into)
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

    impl WorkflowClock for ManualClock {
        fn now(&self) -> WorkflowTime {
            let elapsed_ms = self.elapsed_ms.load(Ordering::SeqCst);
            WorkflowTime {
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
