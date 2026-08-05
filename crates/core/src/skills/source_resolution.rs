//! Human-facing installation locators resolved into immutable acquisition candidates.
//!
//! Resolution and acquisition are deliberately separate extension axes. A resolver accepts a
//! convenient, non-authoritative locator (currently an HTTPS URL), discovers zero or more Skill
//! packages, and returns immutable provider coordinates. The existing acquisition workflow still
//! owns package preparation, preview, and commit.

use super::github_acquisition::{
    GitHubCommit, GitHubReference, GitHubRepository, GitHubSubdirectory,
};
use super::installation_session::{
    duration_millis, ResolutionSlot, SessionRegistryError, SkillInstallationSessionStore,
};
use super::model::{
    SkillRevision, SKILL_PACKAGE_FORMAT_VERSION, SKILL_PACKAGE_FORMAT_VERSION_V2,
    SKILL_PACKAGE_FORMAT_VERSION_V3,
};
use super::package::MAX_SKILL_PACKAGE_BYTES;
use super::prepared::PreparedSkillPackage;
use super::prepared_acquisition::PreparedSkillAcquisition;
use reqwest::Url;
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::sync::Arc;
use uuid::Uuid;

const MAX_INSTALLATION_LOCATOR_BYTES: usize = 4 * 1024;
const MAX_RESOLVER_ID_BYTES: usize = 64;
const MAX_RESOLUTION_CANDIDATES: usize = 64;
const MAX_CANDIDATE_ID_BYTES: usize = 256;
const MAX_RESOLVED_REVISION_BYTES: usize = 256;

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SkillSourceResolutionId(String);

impl SkillSourceResolutionId {
    pub fn new() -> Self {
        Self(Uuid::new_v4().hyphenated().to_string())
    }

    pub fn parse(value: impl Into<String>) -> Result<Self, SkillSourceResolutionIdError> {
        let value = value.into();
        let uuid = Uuid::parse_str(&value)
            .map_err(|_| SkillSourceResolutionIdError::new("resolution id must be a UUID"))?;
        if uuid.is_nil() || uuid.hyphenated().to_string() != value {
            return Err(SkillSourceResolutionIdError::new(
                "resolution id must be a canonical lower-case non-nil UUID",
            ));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for SkillSourceResolutionId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for SkillSourceResolutionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("SkillSourceResolutionId")
            .field(&self.0)
            .finish()
    }
}

impl fmt::Display for SkillSourceResolutionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillSourceResolutionIdError {
    reason: String,
}

impl SkillSourceResolutionIdError {
    fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }

    pub fn reason(&self) -> &str {
        &self.reason
    }
}

impl fmt::Display for SkillSourceResolutionIdError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.reason.fmt(formatter)
    }
}

impl Error for SkillSourceResolutionIdError {}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SkillSourceCandidateId(String);

impl SkillSourceCandidateId {
    pub fn parse(value: impl Into<String>) -> Result<Self, SkillSourceCandidateIdError> {
        let value = value.into();
        if !valid_candidate_id(&value) {
            return Err(SkillSourceCandidateIdError::new(format!(
                "candidate id must contain 1 to {MAX_CANDIDATE_ID_BYTES} portable ASCII characters",
            )));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SkillSourceCandidateId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillSourceCandidateIdError {
    reason: String,
}

impl SkillSourceCandidateIdError {
    fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }

    pub fn reason(&self) -> &str {
        &self.reason
    }
}

impl fmt::Display for SkillSourceCandidateIdError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.reason.fmt(formatter)
    }
}

impl Error for SkillSourceCandidateIdError {}

#[derive(Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SkillInstallationSourceLocator {
    Url(String),
}

impl SkillInstallationSourceLocator {
    pub fn url(value: impl Into<String>) -> Result<Self, SkillSourceResolutionError> {
        let value = value.into();
        if value.is_empty() || value.len() > MAX_INSTALLATION_LOCATOR_BYTES {
            return Err(SkillSourceResolutionError::parse(
                SkillSourceResolutionErrorCode::InvalidLocator,
                SkillSourceResolutionRecovery::FixLocator,
                "The Skill installation URL must be non-empty and within the supported length.",
            ));
        }
        Ok(Self::Url(value))
    }

    pub fn as_url(&self) -> &str {
        match self {
            Self::Url(value) => value,
        }
    }
}

impl fmt::Debug for SkillInstallationSourceLocator {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SkillInstallationSourceLocator")
            .field("kind", &"url")
            .field("value", &"[redacted]")
            .field("value_bytes", &self.as_url().len())
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SkillSourceResolverId(String);

impl SkillSourceResolverId {
    pub fn parse(value: impl Into<String>) -> Result<Self, SkillSourceResolverConfigurationError> {
        let value = value.into();
        let valid = !value.is_empty()
            && value.len() <= MAX_RESOLVER_ID_BYTES
            && value.as_bytes()[0].is_ascii_lowercase()
            && value
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
        if !valid {
            return Err(SkillSourceResolverConfigurationError::new(format!(
                "resolver id must contain 1 to {MAX_RESOLVER_ID_BYTES} lowercase ASCII letters, digits, or hyphens and start with a letter",
            )));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SkillSourceResolverId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ResolvedSkillSource {
    GitHub {
        owner: String,
        repository: String,
        tracking_reference: GitHubReference,
        resolved_commit: String,
        subdirectory: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedSkillPackagePreview {
    format_version: u32,
    revision: SkillRevision,
    name: String,
    description: String,
    file_count: u64,
    total_bytes: u64,
}

impl ResolvedSkillPackagePreview {
    /// Builds the immutable package metadata exposed by a source resolver.
    ///
    /// Resolver implementations are expected to obtain these values from a fully validated
    /// [`PreparedSkillPackage`](super::PreparedSkillPackage), never from untrusted manifest text
    /// alone.
    pub fn new(
        format_version: u32,
        revision: SkillRevision,
        name: impl Into<String>,
        description: impl Into<String>,
        file_count: u64,
        total_bytes: u64,
    ) -> Self {
        Self {
            format_version,
            revision,
            name: name.into(),
            description: description.into(),
            file_count,
            total_bytes,
        }
    }

    pub fn format_version(&self) -> u32 {
        self.format_version
    }

    pub fn revision(&self) -> &SkillRevision {
        &self.revision
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn description(&self) -> &str {
        &self.description
    }

    pub fn file_count(&self) -> u64 {
        self.file_count
    }

    pub fn total_bytes(&self) -> u64 {
        self.total_bytes
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillSourceResolutionCandidate {
    candidate_id: String,
    source: ResolvedSkillSource,
    package: ResolvedSkillPackagePreview,
}

impl SkillSourceResolutionCandidate {
    /// Builds one immutable acquisition candidate discovered by a resolver.
    pub fn new(
        candidate_id: impl Into<String>,
        source: ResolvedSkillSource,
        package: ResolvedSkillPackagePreview,
    ) -> Self {
        Self {
            candidate_id: candidate_id.into(),
            source,
            package,
        }
    }

    pub fn candidate_id(&self) -> &str {
        &self.candidate_id
    }

    pub fn source(&self) -> &ResolvedSkillSource {
        &self.source
    }

    pub fn package(&self) -> &ResolvedSkillPackagePreview {
        &self.package
    }
}

/// Resolver-owned candidate whose metadata and installable bytes come from one
/// fully validated package snapshot.
pub struct PreparedSkillSourceResolutionCandidate {
    candidate_id: SkillSourceCandidateId,
    source: ResolvedSkillSource,
    acquisition: PreparedSkillAcquisition,
}

impl fmt::Debug for PreparedSkillSourceResolutionCandidate {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreparedSkillSourceResolutionCandidate")
            .field("candidate_id", &self.candidate_id)
            .field("source", &self.source)
            .field("acquisition", &self.acquisition)
            .finish()
    }
}

impl PreparedSkillSourceResolutionCandidate {
    pub fn new(
        candidate_id: SkillSourceCandidateId,
        source: ResolvedSkillSource,
        acquisition: PreparedSkillAcquisition,
    ) -> Self {
        Self {
            candidate_id,
            source,
            acquisition,
        }
    }

    pub fn candidate_id(&self) -> &SkillSourceCandidateId {
        &self.candidate_id
    }

    pub fn source(&self) -> &ResolvedSkillSource {
        &self.source
    }

    pub fn package(&self) -> &PreparedSkillPackage {
        self.acquisition.package()
    }

    pub fn acquisition(&self) -> &PreparedSkillAcquisition {
        &self.acquisition
    }

    fn into_parts(
        self,
    ) -> (
        SkillSourceCandidateId,
        SkillSourceResolutionCandidate,
        PreparedSkillAcquisition,
    ) {
        let preview = package_preview(self.acquisition.package());
        let public =
            SkillSourceResolutionCandidate::new(self.candidate_id.as_str(), self.source, preview);
        (self.candidate_id, public, self.acquisition)
    }
}

/// Completed resolver output before it is admitted to the bounded session
/// registry. It intentionally owns the exact packages shown by its candidates.
pub struct PreparedSkillSourceResolution {
    canonical_url: String,
    provider: SkillSourceResolverId,
    resolved_revision: String,
    candidates: Vec<PreparedSkillSourceResolutionCandidate>,
}

impl fmt::Debug for PreparedSkillSourceResolution {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreparedSkillSourceResolution")
            .field("canonical_url", &"[redacted]")
            .field("provider", &self.provider)
            .field("resolved_revision", &self.resolved_revision)
            .field("candidate_count", &self.candidates.len())
            .finish()
    }
}

impl PreparedSkillSourceResolution {
    pub fn new(
        canonical_url: impl Into<String>,
        provider: SkillSourceResolverId,
        resolved_revision: impl Into<String>,
        candidates: Vec<PreparedSkillSourceResolutionCandidate>,
    ) -> Result<Self, SkillSourceResolutionError> {
        let prepared = Self {
            canonical_url: canonical_url.into(),
            provider,
            resolved_revision: resolved_revision.into(),
            candidates,
        };
        prepared.validate()?;
        Ok(prepared)
    }

    pub fn canonical_url(&self) -> &str {
        &self.canonical_url
    }

    pub fn provider(&self) -> &SkillSourceResolverId {
        &self.provider
    }

    pub fn resolved_revision(&self) -> &str {
        &self.resolved_revision
    }

    pub fn candidates(&self) -> &[PreparedSkillSourceResolutionCandidate] {
        &self.candidates
    }

    pub(super) fn public_resolution(
        &self,
    ) -> Result<SkillSourceResolution, SkillSourceResolutionError> {
        SkillSourceResolution::new(
            self.canonical_url.clone(),
            self.provider.clone(),
            self.resolved_revision.clone(),
            self.candidates
                .iter()
                .map(|candidate| {
                    SkillSourceResolutionCandidate::new(
                        candidate.candidate_id().as_str(),
                        candidate.source().clone(),
                        package_preview(candidate.package()),
                    )
                })
                .collect(),
        )
    }

    fn validate(&self) -> Result<(), SkillSourceResolutionError> {
        if self.candidates.is_empty() {
            return Err(SkillSourceResolutionError::discover(
                SkillSourceResolutionErrorCode::NoSkillsFound,
                SkillSourceResolutionRecovery::ChooseDifferentSource,
                "No installable Skill was found at this URL.",
            ));
        }
        if self.candidates.len() > MAX_RESOLUTION_CANDIDATES {
            return Err(SkillSourceResolutionError::discover(
                SkillSourceResolutionErrorCode::TooManySkills,
                SkillSourceResolutionRecovery::NarrowLocator,
                "The source contains too many installable Skills. Select a narrower URL.",
            ));
        }
        validate_canonical_url(&self.canonical_url)?;
        if self.resolved_revision.is_empty()
            || self.resolved_revision.len() > MAX_RESOLVED_REVISION_BYTES
            || self.resolved_revision.chars().any(char::is_control)
        {
            return Err(invalid_resolver_output());
        }
        let mut ids = BTreeSet::new();
        for candidate in &self.candidates {
            if !ids.insert(candidate.candidate_id()) {
                return Err(invalid_resolver_output());
            }
            if !candidate.acquisition().is_owned_by(self.provider.as_str()) {
                return Err(invalid_resolver_output());
            }
            let public = SkillSourceResolutionCandidate::new(
                candidate.candidate_id().as_str(),
                candidate.source().clone(),
                package_preview(candidate.package()),
            );
            validate_candidate(&public, &self.provider, &self.resolved_revision)?;
        }
        Ok(())
    }

    fn into_parts(
        self,
    ) -> Result<
        (
            SkillSourceResolution,
            BTreeMap<SkillSourceCandidateId, PreparedSkillAcquisition>,
            usize,
        ),
        SkillSourceResolutionError,
    > {
        let mut public = Vec::with_capacity(self.candidates.len());
        let mut packages = BTreeMap::new();
        let mut snapshot_bytes = 0_usize;
        for candidate in self.candidates {
            let (candidate_id, descriptor, acquisition) = candidate.into_parts();
            snapshot_bytes = snapshot_bytes.saturating_add(acquisition.retained_payload_bytes());
            public.push(descriptor);
            packages.insert(candidate_id, acquisition);
        }
        let resolution = SkillSourceResolution::new(
            self.canonical_url,
            self.provider,
            self.resolved_revision,
            public,
        )?;
        Ok((resolution, packages, snapshot_bytes))
    }
}

fn package_preview(package: &PreparedSkillPackage) -> ResolvedSkillPackagePreview {
    let resource_index = package.resource_index();
    let resource_bytes = resource_index
        .entries()
        .iter()
        .fold(0_u64, |total, resource| {
            total.saturating_add(resource.byte_length())
        });
    ResolvedSkillPackagePreview::new(
        package.format_version(),
        package.revision().clone(),
        package.name(),
        package.description(),
        u64::try_from(resource_index.len())
            .unwrap_or(u64::MAX)
            .saturating_add(1),
        u64::try_from(package.source_bytes().len())
            .unwrap_or(u64::MAX)
            .saturating_add(resource_bytes),
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillSourceResolutionOutcome {
    Resolved,
    SelectionRequired,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillSourceResolution {
    canonical_url: String,
    provider: SkillSourceResolverId,
    resolved_revision: String,
    candidates: Arc<[SkillSourceResolutionCandidate]>,
}

impl SkillSourceResolution {
    /// Builds a completed resolution after every candidate has been fully validated.
    pub fn new(
        canonical_url: impl Into<String>,
        provider: SkillSourceResolverId,
        resolved_revision: impl Into<String>,
        candidates: Vec<SkillSourceResolutionCandidate>,
    ) -> Result<Self, SkillSourceResolutionError> {
        if candidates.is_empty() {
            return Err(SkillSourceResolutionError::discover(
                SkillSourceResolutionErrorCode::NoSkillsFound,
                SkillSourceResolutionRecovery::ChooseDifferentSource,
                "No installable Skill was found at this URL.",
            ));
        }
        if candidates.len() > MAX_RESOLUTION_CANDIDATES {
            return Err(SkillSourceResolutionError::discover(
                SkillSourceResolutionErrorCode::TooManySkills,
                SkillSourceResolutionRecovery::NarrowLocator,
                "The source contains too many installable Skills. Select a narrower URL.",
            ));
        }
        let canonical_url = canonical_url.into();
        validate_canonical_url(&canonical_url)?;
        let resolved_revision = resolved_revision.into();
        if resolved_revision.is_empty()
            || resolved_revision.len() > MAX_RESOLVED_REVISION_BYTES
            || resolved_revision.chars().any(char::is_control)
        {
            return Err(invalid_resolver_output());
        }
        let mut candidate_ids = std::collections::BTreeSet::new();
        for candidate in &candidates {
            validate_candidate(candidate, &provider, &resolved_revision)?;
            if !candidate_ids.insert(candidate.candidate_id()) {
                return Err(invalid_resolver_output());
            }
        }
        Ok(Self {
            canonical_url,
            provider,
            resolved_revision,
            candidates: candidates.into(),
        })
    }

    pub fn canonical_url(&self) -> &str {
        &self.canonical_url
    }

    pub fn provider(&self) -> &SkillSourceResolverId {
        &self.provider
    }

    pub fn resolved_revision(&self) -> &str {
        &self.resolved_revision
    }

    pub fn outcome(&self) -> SkillSourceResolutionOutcome {
        if self.candidates.len() == 1 {
            SkillSourceResolutionOutcome::Resolved
        } else {
            SkillSourceResolutionOutcome::SelectionRequired
        }
    }

    pub fn candidates(&self) -> &[SkillSourceResolutionCandidate] {
        &self.candidates
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillRegisteredSourceResolution {
    resolution_id: SkillSourceResolutionId,
    resolution: SkillSourceResolution,
    expires_at_unix_ms: u64,
}

impl SkillRegisteredSourceResolution {
    fn new(
        resolution_id: SkillSourceResolutionId,
        resolution: SkillSourceResolution,
        expires_at_unix_ms: u64,
    ) -> Self {
        Self {
            resolution_id,
            resolution,
            expires_at_unix_ms,
        }
    }

    pub fn resolution_id(&self) -> &SkillSourceResolutionId {
        &self.resolution_id
    }

    pub fn resolution(&self) -> &SkillSourceResolution {
        &self.resolution
    }

    pub fn expires_at_unix_ms(&self) -> u64 {
        self.expires_at_unix_ms
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SkillSourceResolutionCancellation {
    Cancelled,
    AlreadyCancelled,
    AlreadyConsumed,
    /// No authority existed, and a bounded fence now prevents a queued resolve from publishing it.
    AlreadyAbsent,
}

pub trait SkillInstallationSourceResolver: Send + Sync {
    fn id(&self) -> SkillSourceResolverId;

    /// Exact lower-case DNS hosts owned by this resolver. Registration rejects overlap. Returning
    /// owned values keeps enterprise and user-configured providers extensible.
    fn supported_hosts(&self) -> Vec<String>;

    fn resolve(
        &self,
        locator: &SkillInstallationSourceLocator,
    ) -> Result<SkillSourceResolution, SkillSourceResolutionError>;

    /// Resolves and retains the exact validated package snapshots represented
    /// by the public candidate metadata. Providers that support installation
    /// handoff override this method; the legacy metadata-only method remains so
    /// existing read-only resolver implementations keep compiling.
    fn resolve_prepared(
        &self,
        _locator: &SkillInstallationSourceLocator,
    ) -> Result<PreparedSkillSourceResolution, SkillSourceResolutionError> {
        Err(SkillSourceResolutionError::resolve(
            SkillSourceResolutionErrorCode::Unavailable,
            SkillSourceResolutionRecovery::RetryLater,
            "This Skill source resolver does not support prepared candidate handoff.",
        ))
    }
}

pub struct SkillSourceResolutionService {
    resolvers: BTreeMap<SkillSourceResolverId, Arc<dyn SkillInstallationSourceResolver>>,
    hosts: BTreeMap<String, SkillSourceResolverId>,
    sessions: SkillInstallationSessionStore,
}

/// Panic recovery for resolver callbacks. A resolver attempt runs outside the
/// session lock, so an unwind must release only the reservation owned by that
/// exact attempt. Matching both the attempt identity and locator prevents a
/// stale guard from removing a cancellation, a published result, or a later
/// retry that reused the same resolution identity.
struct ResolvingSlotRecovery {
    sessions: SkillInstallationSessionStore,
    resolution_id: SkillSourceResolutionId,
    locator: SkillInstallationSourceLocator,
    attempt_id: u64,
}

impl ResolvingSlotRecovery {
    fn new(
        sessions: &SkillInstallationSessionStore,
        resolution_id: &SkillSourceResolutionId,
        locator: &SkillInstallationSourceLocator,
        attempt_id: u64,
    ) -> Self {
        Self {
            sessions: sessions.clone(),
            resolution_id: resolution_id.clone(),
            locator: locator.clone(),
            attempt_id,
        }
    }
}

impl Drop for ResolvingSlotRecovery {
    fn drop(&mut self) {
        let recovered = self.sessions.lock().is_ok_and(|mut state| {
            let still_owns_slot = matches!(
                state.resolutions.get(&self.resolution_id),
                Some(ResolutionSlot::Resolving {
                    locator,
                    attempt_id,
                    ..
                }) if locator == &self.locator && *attempt_id == self.attempt_id
            );
            if still_owns_slot {
                state.resolutions.remove(&self.resolution_id);
            }
            still_owns_slot
        });
        if recovered {
            self.sessions.notify_all();
        }
    }
}

impl Default for SkillSourceResolutionService {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for SkillSourceResolutionService {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SkillSourceResolutionService")
            .field("resolver_ids", &self.resolvers.keys().collect::<Vec<_>>())
            .field("hosts", &self.hosts.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl SkillSourceResolutionService {
    pub fn new() -> Self {
        Self::with_session_store(SkillInstallationSessionStore::default())
    }

    pub fn with_session_store(sessions: SkillInstallationSessionStore) -> Self {
        Self {
            resolvers: BTreeMap::new(),
            hosts: BTreeMap::new(),
            sessions,
        }
    }

    pub fn session_store(&self) -> SkillInstallationSessionStore {
        self.sessions.clone()
    }

    pub fn register_resolver(
        &mut self,
        resolver: Arc<dyn SkillInstallationSourceResolver>,
    ) -> Result<(), SkillSourceResolverConfigurationError> {
        let id = resolver.id();
        if self.resolvers.contains_key(&id) {
            return Err(SkillSourceResolverConfigurationError::new(format!(
                "Skill source resolver `{id}` is already registered",
            )));
        }
        let mut normalized_hosts = Vec::new();
        for host in resolver.supported_hosts() {
            let normalized = normalize_registered_host(&host)?;
            if self.hosts.contains_key(&normalized)
                || normalized_hosts
                    .iter()
                    .any(|existing| existing == &normalized)
            {
                return Err(SkillSourceResolverConfigurationError::new(format!(
                    "Skill source resolver host `{normalized}` is already registered",
                )));
            }
            normalized_hosts.push(normalized);
        }
        if normalized_hosts.is_empty() {
            return Err(SkillSourceResolverConfigurationError::new(
                "Skill source resolver must register at least one exact host",
            ));
        }

        for host in normalized_hosts {
            self.hosts.insert(host, id.clone());
        }
        self.resolvers.insert(id, resolver);
        Ok(())
    }

    pub fn resolve(
        &self,
        locator: &SkillInstallationSourceLocator,
    ) -> Result<SkillSourceResolution, SkillSourceResolutionError> {
        let url = parse_dispatch_url(locator.as_url())?;
        let host = url.host_str().ok_or_else(|| {
            SkillSourceResolutionError::parse(
                SkillSourceResolutionErrorCode::InvalidLocator,
                SkillSourceResolutionRecovery::FixLocator,
                "The Skill installation URL does not contain a valid host.",
            )
        })?;
        let Some(id) = self.hosts.get(host) else {
            return Err(SkillSourceResolutionError::parse(
                SkillSourceResolutionErrorCode::UnsupportedHost,
                SkillSourceResolutionRecovery::ChooseDifferentSource,
                "This Skill installation URL host is not supported.",
            ));
        };
        let resolver = self
            .resolvers
            .get(id)
            .expect("registered host must reference a registered resolver");
        let resolution = resolver
            .resolve(locator)
            .map_err(|error| error.bind_provider(id.clone()))?;
        if resolution.provider() != id {
            return Err(invalid_resolver_output().bind_provider(id.clone()));
        }
        let canonical_host = parse_dispatch_url(resolution.canonical_url())
            .ok()
            .and_then(|url| url.host_str().map(str::to_string));
        if canonical_host
            .as_deref()
            .and_then(|host| self.hosts.get(host))
            != Some(id)
        {
            return Err(invalid_resolver_output().bind_provider(id.clone()));
        }
        Ok(resolution)
    }

    /// Idempotently resolves and retains fully prepared candidates under the
    /// client-provided resolution identity. A retry with the same identity and
    /// locator returns the same registered result without invoking the
    /// resolver or transport again.
    pub fn resolve_registered(
        &self,
        resolution_id: SkillSourceResolutionId,
        locator: &SkillInstallationSourceLocator,
    ) -> Result<SkillRegisteredSourceResolution, SkillSourceResolutionError> {
        let (resolver_id, resolver) = self.resolver_for(locator)?;
        let attempt_id = loop {
            let now = self.sessions.now();
            let mut state = self.lock_sessions()?;
            state.prune_expired(now.monotonic);
            match state.resolutions.get(&resolution_id) {
                Some(slot) if slot.locator().is_some_and(|bound| bound != locator) => {
                    return Err(resolution_id_conflict());
                }
                Some(ResolutionSlot::Resolving { expires_at, .. }) => {
                    let deadline = *expires_at;
                    drop(self.wait_for_resolution(state, deadline)?);
                    continue;
                }
                Some(ResolutionSlot::Ready { resolution, .. }) => {
                    return Ok(resolution.clone());
                }
                Some(ResolutionSlot::Consumed { .. }) => {
                    return Err(resolution_consumed());
                }
                Some(ResolutionSlot::Cancelled { .. }) => {
                    return Err(resolution_cancelled());
                }
                None => {
                    let config = self.sessions.config();
                    if state.active_resolution_count() >= config.max_resolutions() {
                        return Err(resolution_capacity_exceeded());
                    }
                    // Every admitted active slot reserves its future consumed/cancelled
                    // tombstone. This keeps idempotent replay bounded without letting old
                    // tombstones consume the much smaller active-resolution allowance.
                    if state.retained_resolution_record_count()
                        >= config.max_resolution_tombstones()
                    {
                        return Err(resolution_tombstone_capacity_exceeded());
                    }
                    if state.retained_resolution_candidates() >= config.max_resolution_candidates()
                    {
                        return Err(resolution_candidate_capacity_exceeded());
                    }
                    if state
                        .reserved_snapshot_bytes()
                        .saturating_add(MAX_SKILL_PACKAGE_BYTES)
                        > config.max_snapshot_bytes()
                    {
                        return Err(resolution_memory_capacity_exceeded());
                    }
                    let attempt_id = state.next_resolution_attempt();
                    state.resolutions.insert(
                        resolution_id.clone(),
                        ResolutionSlot::Resolving {
                            locator: locator.clone(),
                            attempt_id,
                            reserved_bytes: MAX_SKILL_PACKAGE_BYTES,
                            expires_at: now.monotonic.saturating_add(config.resolving_ttl()),
                        },
                    );
                    break attempt_id;
                }
            }
        };

        let _resolving_recovery =
            ResolvingSlotRecovery::new(&self.sessions, &resolution_id, locator, attempt_id);
        let prepared = resolver
            .resolve_prepared(locator)
            .map_err(|error| error.bind_provider(resolver_id.clone()))
            .and_then(|prepared| {
                self.validate_prepared_resolution(&resolver_id, &prepared)?;
                Ok(prepared)
            });

        let now = self.sessions.now();
        let mut state = self.lock_sessions()?;
        state.prune_expired(now.monotonic);
        let still_current = matches!(
            state.resolutions.get(&resolution_id),
            Some(ResolutionSlot::Resolving {
                attempt_id: current,
                ..
            }) if *current == attempt_id
        );
        if !still_current {
            self.sessions.notify_all();
            return Err(resolution_not_found_or_expired());
        }

        let prepared = match prepared {
            Ok(prepared) => prepared,
            Err(error) => {
                state.resolutions.remove(&resolution_id);
                self.sessions.notify_all();
                return Err(error);
            }
        };
        let (resolution, packages, snapshot_bytes) = prepared.into_parts()?;
        let config = self.sessions.config();
        let resident_without_reservation = state
            .reserved_snapshot_bytes()
            .saturating_sub(MAX_SKILL_PACKAGE_BYTES);
        let retained_candidates = state.retained_resolution_candidates();
        if resident_without_reservation.saturating_add(snapshot_bytes) > config.max_snapshot_bytes()
        {
            state.resolutions.remove(&resolution_id);
            self.sessions.notify_all();
            return Err(resolution_memory_capacity_exceeded());
        }
        if retained_candidates.saturating_add(packages.len()) > config.max_resolution_candidates() {
            state.resolutions.remove(&resolution_id);
            self.sessions.notify_all();
            return Err(resolution_candidate_capacity_exceeded());
        }

        let expires_at = now.monotonic.saturating_add(config.resolution_ttl());
        let registered = SkillRegisteredSourceResolution::new(
            resolution_id.clone(),
            resolution,
            now.unix_ms
                .saturating_add(duration_millis(config.resolution_ttl())),
        );
        state.resolutions.insert(
            resolution_id,
            ResolutionSlot::Ready {
                locator: locator.clone(),
                resolution: registered.clone(),
                candidates: packages,
                snapshot_bytes,
                expires_at,
            },
        );
        self.sessions.notify_all();
        Ok(registered)
    }

    pub fn cancel_registered_resolution(
        &self,
        resolution_id: &SkillSourceResolutionId,
    ) -> Result<SkillSourceResolutionCancellation, SkillSourceResolutionError> {
        let now = self.sessions.now();
        let mut state = self.lock_sessions()?;
        state.prune_expired(now.monotonic);
        let Some(slot) = state.resolutions.get(resolution_id) else {
            let config = self.sessions.config();
            if state.retained_resolution_record_count() >= config.max_resolution_tombstones() {
                return Err(resolution_tombstone_capacity_exceeded());
            }
            state.resolutions.insert(
                resolution_id.clone(),
                ResolutionSlot::Cancelled {
                    locator: None,
                    expires_at: now.monotonic.saturating_add(config.resolution_ttl()),
                },
            );
            self.sessions.notify_all();
            return Ok(SkillSourceResolutionCancellation::AlreadyAbsent);
        };
        match slot {
            ResolutionSlot::Consumed { .. } => {
                Ok(SkillSourceResolutionCancellation::AlreadyConsumed)
            }
            ResolutionSlot::Cancelled { .. } => {
                Ok(SkillSourceResolutionCancellation::AlreadyCancelled)
            }
            ResolutionSlot::Resolving { locator, .. } | ResolutionSlot::Ready { locator, .. } => {
                let locator = locator.clone();
                let expires_at = now
                    .monotonic
                    .saturating_add(self.sessions.config().resolution_ttl());
                state.resolutions.insert(
                    resolution_id.clone(),
                    ResolutionSlot::Cancelled {
                        locator: Some(locator),
                        expires_at,
                    },
                );
                self.sessions.notify_all();
                Ok(SkillSourceResolutionCancellation::Cancelled)
            }
        }
    }

    fn resolver_for(
        &self,
        locator: &SkillInstallationSourceLocator,
    ) -> Result<
        (
            SkillSourceResolverId,
            Arc<dyn SkillInstallationSourceResolver>,
        ),
        SkillSourceResolutionError,
    > {
        let url = parse_dispatch_url(locator.as_url())?;
        let host = url.host_str().ok_or_else(|| {
            SkillSourceResolutionError::parse(
                SkillSourceResolutionErrorCode::InvalidLocator,
                SkillSourceResolutionRecovery::FixLocator,
                "The Skill installation URL does not contain a valid host.",
            )
        })?;
        let Some(id) = self.hosts.get(host) else {
            return Err(SkillSourceResolutionError::parse(
                SkillSourceResolutionErrorCode::UnsupportedHost,
                SkillSourceResolutionRecovery::ChooseDifferentSource,
                "This Skill installation URL host is not supported.",
            ));
        };
        Ok((
            id.clone(),
            Arc::clone(
                self.resolvers
                    .get(id)
                    .expect("registered host must reference a registered resolver"),
            ),
        ))
    }

    fn validate_prepared_resolution(
        &self,
        id: &SkillSourceResolverId,
        resolution: &PreparedSkillSourceResolution,
    ) -> Result<(), SkillSourceResolutionError> {
        if resolution.provider() != id {
            return Err(invalid_resolver_output().bind_provider(id.clone()));
        }
        let canonical_host = parse_dispatch_url(resolution.canonical_url())
            .ok()
            .and_then(|url| url.host_str().map(str::to_string));
        if canonical_host
            .as_deref()
            .and_then(|host| self.hosts.get(host))
            != Some(id)
        {
            return Err(invalid_resolver_output().bind_provider(id.clone()));
        }
        if resolution
            .candidates()
            .iter()
            .any(|candidate| !candidate.acquisition().is_owned_by(id.as_str()))
        {
            return Err(invalid_resolver_output().bind_provider(id.clone()));
        }
        Ok(())
    }

    fn lock_sessions(
        &self,
    ) -> Result<
        std::sync::MutexGuard<'_, super::installation_session::InstallationSessionState>,
        SkillSourceResolutionError,
    > {
        self.sessions.lock().map_err(map_session_error)
    }

    fn wait_for_resolution<'a>(
        &self,
        state: std::sync::MutexGuard<'a, super::installation_session::InstallationSessionState>,
        deadline: std::time::Duration,
    ) -> Result<
        std::sync::MutexGuard<'a, super::installation_session::InstallationSessionState>,
        SkillSourceResolutionError,
    > {
        self.sessions
            .wait_until(state, deadline)
            .map_err(map_session_error)
    }
}

fn validate_canonical_url(value: &str) -> Result<(), SkillSourceResolutionError> {
    if value.is_empty() || value.len() > MAX_INSTALLATION_LOCATOR_BYTES {
        return Err(invalid_resolver_output());
    }
    let url = parse_dispatch_url(value).map_err(|_| invalid_resolver_output())?;
    if url.query().is_some() || url.fragment().is_some() {
        return Err(invalid_resolver_output());
    }
    Ok(())
}

fn validate_candidate(
    candidate: &SkillSourceResolutionCandidate,
    provider: &SkillSourceResolverId,
    resolved_revision: &str,
) -> Result<(), SkillSourceResolutionError> {
    let id = candidate.candidate_id();
    if !valid_candidate_id(id) {
        return Err(invalid_resolver_output());
    }
    let package = candidate.package();
    if !matches!(
        package.format_version(),
        SKILL_PACKAGE_FORMAT_VERSION
            | SKILL_PACKAGE_FORMAT_VERSION_V2
            | SKILL_PACKAGE_FORMAT_VERSION_V3
    ) || package.name().trim().is_empty()
        || package.description().trim().is_empty()
        || package.file_count() == 0
        || package.total_bytes() == 0
    {
        return Err(invalid_resolver_output());
    }
    match candidate.source() {
        ResolvedSkillSource::GitHub {
            owner,
            repository,
            tracking_reference,
            resolved_commit,
            subdirectory,
        } => {
            let canonical_commit = GitHubCommit::parse(resolved_commit.clone())
                .ok()
                .is_some_and(|commit| commit.as_str() == resolved_commit);
            if provider.as_str() != "github"
                || resolved_commit != resolved_revision
                || !canonical_commit
                || GitHubRepository::parse(owner.clone(), repository.clone()).is_err()
                || matches!(
                    tracking_reference,
                    GitHubReference::Commit(commit) if commit.as_str() != resolved_commit
                )
                || subdirectory.as_deref().is_some_and(|value| {
                    value.is_empty() || GitHubSubdirectory::parse(value.to_string()).is_err()
                })
            {
                return Err(invalid_resolver_output());
            }
        }
    }
    Ok(())
}

fn valid_candidate_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_CANDIDATE_ID_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
}

fn invalid_resolver_output() -> SkillSourceResolutionError {
    SkillSourceResolutionError::resolve(
        SkillSourceResolutionErrorCode::Unavailable,
        SkillSourceResolutionRecovery::RetryLater,
        "The Skill source resolver returned an invalid result.",
    )
}

fn resolution_id_conflict() -> SkillSourceResolutionError {
    SkillSourceResolutionError::resolve(
        SkillSourceResolutionErrorCode::IdempotencyConflict,
        SkillSourceResolutionRecovery::StartNewResolution,
        "The resolution ID is already bound to a different source locator.",
    )
}

fn resolution_not_found_or_expired() -> SkillSourceResolutionError {
    SkillSourceResolutionError::resolve(
        SkillSourceResolutionErrorCode::ResolutionNotFoundOrExpired,
        SkillSourceResolutionRecovery::StartNewResolution,
        "The Skill source resolution was not found or has expired.",
    )
}

fn resolution_consumed() -> SkillSourceResolutionError {
    SkillSourceResolutionError::resolve(
        SkillSourceResolutionErrorCode::ResolutionConsumed,
        SkillSourceResolutionRecovery::StartNewResolution,
        "The Skill source resolution has already been consumed.",
    )
}

fn resolution_cancelled() -> SkillSourceResolutionError {
    SkillSourceResolutionError::resolve(
        SkillSourceResolutionErrorCode::Cancelled,
        SkillSourceResolutionRecovery::StartNewResolution,
        "The Skill source resolution was cancelled.",
    )
}

fn resolution_capacity_exceeded() -> SkillSourceResolutionError {
    SkillSourceResolutionError::resolve(
        SkillSourceResolutionErrorCode::CapacityExceeded,
        SkillSourceResolutionRecovery::RetryLater,
        "The Skill source resolution registry is at capacity.",
    )
}

fn resolution_tombstone_capacity_exceeded() -> SkillSourceResolutionError {
    SkillSourceResolutionError::resolve(
        SkillSourceResolutionErrorCode::CapacityExceeded,
        SkillSourceResolutionRecovery::RetryLater,
        "The Skill source resolution replay registry is at capacity.",
    )
}

fn resolution_candidate_capacity_exceeded() -> SkillSourceResolutionError {
    SkillSourceResolutionError::resolve(
        SkillSourceResolutionErrorCode::CapacityExceeded,
        SkillSourceResolutionRecovery::RetryLater,
        "The Skill source resolution candidate registry is at capacity.",
    )
}

fn resolution_memory_capacity_exceeded() -> SkillSourceResolutionError {
    SkillSourceResolutionError::resolve(
        SkillSourceResolutionErrorCode::CapacityExceeded,
        SkillSourceResolutionRecovery::RetryLater,
        "The Skill installation session snapshot budget is exhausted.",
    )
}

fn map_session_error(_error: SessionRegistryError) -> SkillSourceResolutionError {
    SkillSourceResolutionError::resolve(
        SkillSourceResolutionErrorCode::Unavailable,
        SkillSourceResolutionRecovery::RetryLater,
        "The Skill installation session registry is unavailable.",
    )
}

fn parse_dispatch_url(value: &str) -> Result<Url, SkillSourceResolutionError> {
    let url = Url::parse(value).map_err(|_| {
        SkillSourceResolutionError::parse(
            SkillSourceResolutionErrorCode::InvalidLocator,
            SkillSourceResolutionRecovery::FixLocator,
            "The Skill installation URL is invalid.",
        )
    })?;
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.cannot_be_a_base()
        || url.host_str().is_none()
    {
        return Err(SkillSourceResolutionError::parse(
            SkillSourceResolutionErrorCode::InvalidLocator,
            SkillSourceResolutionRecovery::FixLocator,
            "Skill installation URLs must use HTTPS and cannot contain credentials.",
        ));
    }
    if url.port().is_some_and(|port| port != 443) {
        return Err(SkillSourceResolutionError::parse(
            SkillSourceResolutionErrorCode::InvalidLocator,
            SkillSourceResolutionRecovery::FixLocator,
            "Skill installation URLs cannot use a custom port.",
        ));
    }
    Ok(url)
}

fn normalize_registered_host(host: &str) -> Result<String, SkillSourceResolverConfigurationError> {
    let normalized = host.to_ascii_lowercase();
    let valid = !normalized.is_empty()
        && normalized.len() <= 253
        && !normalized.starts_with('.')
        && !normalized.ends_with('.')
        && normalized.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'-')
        });
    if !valid || normalized != host {
        return Err(SkillSourceResolverConfigurationError::new(
            "Skill source resolver hosts must be canonical lower-case DNS names",
        ));
    }
    Ok(normalized)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SkillSourceResolutionPhase {
    Parse,
    Resolve,
    Discover,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SkillSourceResolutionErrorCode {
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
    IdempotencyConflict,
    ResolutionNotFoundOrExpired,
    ResolutionConsumed,
    CandidateNotFound,
    CapacityExceeded,
    Cancelled,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SkillSourceResolutionRecovery {
    FixLocator,
    RetryLater,
    NarrowLocator,
    ChooseDifferentSource,
    RetrySameResolution,
    StartNewResolution,
}

#[derive(Clone, PartialEq, Eq)]
pub struct SkillSourceResolutionError {
    phase: SkillSourceResolutionPhase,
    code: SkillSourceResolutionErrorCode,
    recovery: SkillSourceResolutionRecovery,
    message: String,
    provider: Option<SkillSourceResolverId>,
    retry_after_ms: Option<u64>,
}

impl SkillSourceResolutionError {
    pub fn new(
        phase: SkillSourceResolutionPhase,
        code: SkillSourceResolutionErrorCode,
        recovery: SkillSourceResolutionRecovery,
        message: impl Into<String>,
    ) -> Self {
        Self {
            phase,
            code,
            recovery,
            message: message.into(),
            provider: None,
            retry_after_ms: None,
        }
    }

    pub fn parse(
        code: SkillSourceResolutionErrorCode,
        recovery: SkillSourceResolutionRecovery,
        message: impl Into<String>,
    ) -> Self {
        Self::new(SkillSourceResolutionPhase::Parse, code, recovery, message)
    }

    pub fn resolve(
        code: SkillSourceResolutionErrorCode,
        recovery: SkillSourceResolutionRecovery,
        message: impl Into<String>,
    ) -> Self {
        Self::new(SkillSourceResolutionPhase::Resolve, code, recovery, message)
    }

    pub fn discover(
        code: SkillSourceResolutionErrorCode,
        recovery: SkillSourceResolutionRecovery,
        message: impl Into<String>,
    ) -> Self {
        Self::new(
            SkillSourceResolutionPhase::Discover,
            code,
            recovery,
            message,
        )
    }

    pub fn phase(&self) -> SkillSourceResolutionPhase {
        self.phase
    }

    pub fn code(&self) -> SkillSourceResolutionErrorCode {
        self.code
    }

    pub fn recovery(&self) -> SkillSourceResolutionRecovery {
        self.recovery
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn provider(&self) -> Option<&SkillSourceResolverId> {
        self.provider.as_ref()
    }

    pub fn retry_after_ms(&self) -> Option<u64> {
        self.retry_after_ms
    }

    pub fn with_retry_after(mut self, retry_after: std::time::Duration) -> Self {
        self.retry_after_ms = u64::try_from(retry_after.as_millis()).ok();
        self
    }

    fn bind_provider(mut self, provider: SkillSourceResolverId) -> Self {
        self.provider = Some(provider);
        self
    }
}

impl fmt::Debug for SkillSourceResolutionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SkillSourceResolutionError")
            .field("phase", &self.phase)
            .field("code", &self.code)
            .field("recovery", &self.recovery)
            .field("provider", &self.provider)
            .field("retry_after_ms", &self.retry_after_ms)
            .field("message", &"[redacted]")
            .finish()
    }
}

impl fmt::Display for SkillSourceResolutionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.message.fmt(formatter)
    }
}

impl Error for SkillSourceResolutionError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillSourceResolverConfigurationError {
    reason: String,
}

impl SkillSourceResolverConfigurationError {
    fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }

    pub fn reason(&self) -> &str {
        &self.reason
    }
}

impl fmt::Display for SkillSourceResolverConfigurationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.reason.fmt(formatter)
    }
}

impl Error for SkillSourceResolverConfigurationError {}

#[cfg(test)]
mod tests {
    use super::super::acquisition_provenance::SkillInstallationProvenance;
    use super::super::installation_session::{
        SessionClock, SessionTime, SkillInstallationSessionConfig,
        DEFAULT_SKILL_SESSION_SNAPSHOT_BYTES,
    };
    use super::super::origin::SkillPackageOrigin;
    use super::*;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
    use std::thread;

    const FIXTURE_COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";

    #[derive(Debug)]
    struct StubResolver {
        id: &'static str,
        hosts: &'static [&'static str],
    }

    impl SkillInstallationSourceResolver for StubResolver {
        fn id(&self) -> SkillSourceResolverId {
            SkillSourceResolverId::parse(self.id).expect("valid fixture resolver id")
        }

        fn supported_hosts(&self) -> Vec<String> {
            self.hosts.iter().map(|host| (*host).to_string()).collect()
        }

        fn resolve(
            &self,
            _locator: &SkillInstallationSourceLocator,
        ) -> Result<SkillSourceResolution, SkillSourceResolutionError> {
            Err(SkillSourceResolutionError::discover(
                SkillSourceResolutionErrorCode::NoSkillsFound,
                SkillSourceResolutionRecovery::ChooseDifferentSource,
                "fixture",
            ))
        }
    }

    struct PreparedStubResolver {
        calls: Arc<AtomicUsize>,
        candidate_count: usize,
    }

    impl PreparedStubResolver {
        fn resolution(
            &self,
            locator: &SkillInstallationSourceLocator,
        ) -> PreparedSkillSourceResolution {
            let candidates = (0..self.candidate_count)
                .map(|index| {
                    let package = PreparedSkillPackage::from_bytes(
                        format!(
                            "---\nname: fixture-{index}\ndescription: Prepared resolver fixture.\n---\n# Instructions\nEXACT_BYTES_{index}\n"
                        )
                        .into_bytes(),
                        SkillPackageOrigin::new("github", format!("fixture-{index}"))
                            .expect("valid fixture origin"),
                    )
                    .expect("valid fixture package");
                    let provenance =
                        SkillInstallationProvenance::from_legacy_origin(package.origin());
                    PreparedSkillSourceResolutionCandidate::new(
                        SkillSourceCandidateId::parse(format!("fixture-{index}"))
                            .expect("valid fixture candidate id"),
                        ResolvedSkillSource::GitHub {
                            owner: "example".to_string(),
                            repository: "skills".to_string(),
                            tracking_reference: GitHubReference::DefaultBranch,
                            resolved_commit: FIXTURE_COMMIT.to_string(),
                            subdirectory: Some(format!("skills/fixture-{index}")),
                        },
                        PreparedSkillAcquisition::new(package, provenance),
                    )
                })
                .collect();
            PreparedSkillSourceResolution::new(
                locator.as_url(),
                SkillSourceResolverId::parse("github").unwrap(),
                FIXTURE_COMMIT,
                candidates,
            )
            .expect("valid prepared fixture resolution")
        }
    }

    impl SkillInstallationSourceResolver for PreparedStubResolver {
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
            self.resolution(locator).public_resolution()
        }

        fn resolve_prepared(
            &self,
            locator: &SkillInstallationSourceLocator,
        ) -> Result<PreparedSkillSourceResolution, SkillSourceResolutionError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.resolution(locator))
        }
    }

    struct PanicOncePreparedResolver {
        calls: Arc<AtomicUsize>,
    }

    impl SkillInstallationSourceResolver for PanicOncePreparedResolver {
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
            unreachable!("registered resolution uses the prepared resolver contract")
        }

        fn resolve_prepared(
            &self,
            locator: &SkillInstallationSourceLocator,
        ) -> Result<PreparedSkillSourceResolution, SkillSourceResolutionError> {
            if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                panic!("fixture resolver panic");
            }
            Ok(PreparedStubResolver {
                calls: Arc::new(AtomicUsize::new(0)),
                candidate_count: 1,
            }
            .resolution(locator))
        }
    }

    struct CrossProviderPreparedResolver;

    impl SkillInstallationSourceResolver for CrossProviderPreparedResolver {
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
            unreachable!("registered resolution uses the prepared resolver contract")
        }

        fn resolve_prepared(
            &self,
            locator: &SkillInstallationSourceLocator,
        ) -> Result<PreparedSkillSourceResolution, SkillSourceResolutionError> {
            let package = PreparedSkillPackage::from_bytes(
                b"---\nname: cross-provider\ndescription: Cross-provider fixture.\n---\n# Instructions\nMUST_BE_REJECTED\n"
                    .to_vec(),
                SkillPackageOrigin::new("other-provider", "malicious-output").unwrap(),
            )
            .unwrap();
            let provenance = SkillInstallationProvenance::from_legacy_origin(package.origin());
            let candidate = PreparedSkillSourceResolutionCandidate::new(
                SkillSourceCandidateId::parse("cross-provider").unwrap(),
                ResolvedSkillSource::GitHub {
                    owner: "example".to_string(),
                    repository: "skills".to_string(),
                    tracking_reference: GitHubReference::DefaultBranch,
                    resolved_commit: FIXTURE_COMMIT.to_string(),
                    subdirectory: Some("skills/cross-provider".to_string()),
                },
                PreparedSkillAcquisition::new(package, provenance),
            );

            // Construct the malformed value directly to exercise the service
            // boundary even though the public constructor also rejects it.
            Ok(PreparedSkillSourceResolution {
                canonical_url: locator.as_url().to_string(),
                provider: SkillSourceResolverId::parse("github").unwrap(),
                resolved_revision: FIXTURE_COMMIT.to_string(),
                candidates: vec![candidate],
            })
        }
    }

    fn prepared_service(
        sessions: SkillInstallationSessionStore,
        calls: Arc<AtomicUsize>,
        candidate_count: usize,
    ) -> SkillSourceResolutionService {
        let mut service = SkillSourceResolutionService::with_session_store(sessions);
        service
            .register_resolver(Arc::new(PreparedStubResolver {
                calls,
                candidate_count,
            }))
            .unwrap();
        service
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
        fn advance(&self, duration: std::time::Duration) {
            self.elapsed_ms.fetch_add(
                u64::try_from(duration.as_millis()).unwrap(),
                Ordering::SeqCst,
            );
        }
    }

    impl SessionClock for ManualClock {
        fn now(&self) -> SessionTime {
            let elapsed_ms = self.elapsed_ms.load(Ordering::SeqCst);
            SessionTime {
                monotonic: std::time::Duration::from_millis(elapsed_ms),
                unix_ms: self.started_unix_ms.saturating_add(elapsed_ms),
            }
        }
    }

    #[test]
    fn registry_rejects_duplicate_ids_and_hosts_without_partial_registration() {
        let mut service = SkillSourceResolutionService::new();
        service
            .register_resolver(Arc::new(StubResolver {
                id: "github",
                hosts: &["github.com"],
            }))
            .expect("first resolver registers");

        assert!(service
            .register_resolver(Arc::new(StubResolver {
                id: "github",
                hosts: &["example.com"],
            }))
            .is_err());
        assert!(service
            .register_resolver(Arc::new(StubResolver {
                id: "example",
                hosts: &["github.com", "unused.example"],
            }))
            .is_err());
        assert!(!service.hosts.contains_key("unused.example"));
    }

    #[test]
    fn dispatch_rejects_credentials_custom_ports_and_unknown_hosts() {
        let service = SkillSourceResolutionService::new();
        for value in [
            "http://github.com/owner/repo",
            "https://user:secret@github.com/owner/repo",
            "https://github.com:8443/owner/repo",
            "https://example.com/owner/repo",
        ] {
            let locator = SkillInstallationSourceLocator::url(value).expect("bounded URL");
            assert!(service.resolve(&locator).is_err());
        }
    }

    #[test]
    fn dispatch_binds_the_selected_provider_to_resolver_errors() {
        let mut service = SkillSourceResolutionService::new();
        service
            .register_resolver(Arc::new(StubResolver {
                id: "github",
                hosts: &["github.com"],
            }))
            .unwrap();
        let locator =
            SkillInstallationSourceLocator::url("https://github.com/example/skills").unwrap();

        let error = service.resolve(&locator).unwrap_err();

        assert_eq!(
            error.provider().map(SkillSourceResolverId::as_str),
            Some("github")
        );
        assert_eq!(error.phase(), SkillSourceResolutionPhase::Discover);
    }

    #[test]
    fn prepared_resolution_constructor_rejects_cross_provider_acquisition() {
        let resolver = CrossProviderPreparedResolver;
        let locator =
            SkillInstallationSourceLocator::url("https://github.com/example/skills").unwrap();
        let malformed = resolver.resolve_prepared(&locator).unwrap();

        let error = PreparedSkillSourceResolution::new(
            malformed.canonical_url,
            malformed.provider,
            malformed.resolved_revision,
            malformed.candidates,
        )
        .unwrap_err();

        assert_eq!(error.code(), SkillSourceResolutionErrorCode::Unavailable);
        assert_eq!(error.recovery(), SkillSourceResolutionRecovery::RetryLater);
    }

    #[test]
    fn registered_resolver_cannot_cross_its_provider_ownership_boundary() {
        let mut service = SkillSourceResolutionService::new();
        service
            .register_resolver(Arc::new(CrossProviderPreparedResolver))
            .unwrap();
        let locator =
            SkillInstallationSourceLocator::url("https://github.com/example/skills").unwrap();

        let error = service
            .resolve_registered(SkillSourceResolutionId::new(), &locator)
            .unwrap_err();

        assert_eq!(error.code(), SkillSourceResolutionErrorCode::Unavailable);
        assert_eq!(error.recovery(), SkillSourceResolutionRecovery::RetryLater);
        assert_eq!(
            error.provider().map(SkillSourceResolverId::as_str),
            Some("github")
        );
    }

    #[test]
    fn locator_debug_never_exposes_the_url() {
        let locator = SkillInstallationSourceLocator::url(
            "https://github.com/private-owner/private-repository",
        )
        .expect("valid bounded locator");
        let debug = format!("{locator:?}");
        assert!(!debug.contains("private-owner"));
        assert!(debug.contains("[redacted]"));
    }

    #[test]
    fn registered_resolution_retries_replay_without_resolving_again() {
        let calls = Arc::new(AtomicUsize::new(0));
        let service = prepared_service(
            SkillInstallationSessionStore::default(),
            Arc::clone(&calls),
            1,
        );
        let resolution_id = SkillSourceResolutionId::new();
        let locator =
            SkillInstallationSourceLocator::url("https://github.com/example/skills").unwrap();

        let first = service
            .resolve_registered(resolution_id.clone(), &locator)
            .unwrap();
        let retry = service
            .resolve_registered(resolution_id.clone(), &locator)
            .unwrap();

        assert_eq!(first, retry);
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        let different =
            SkillInstallationSourceLocator::url("https://github.com/example/other").unwrap();
        let error = service
            .resolve_registered(resolution_id, &different)
            .unwrap_err();
        assert_eq!(
            error.code(),
            SkillSourceResolutionErrorCode::IdempotencyConflict
        );
        assert_eq!(
            error.recovery(),
            SkillSourceResolutionRecovery::StartNewResolution
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn concurrent_registered_retries_share_one_resolver_attempt() {
        let calls = Arc::new(AtomicUsize::new(0));
        let service = Arc::new(prepared_service(
            SkillInstallationSessionStore::default(),
            Arc::clone(&calls),
            1,
        ));
        let resolution_id = SkillSourceResolutionId::new();
        let locator =
            SkillInstallationSourceLocator::url("https://github.com/example/skills").unwrap();
        let threads = (0..8)
            .map(|_| {
                let service = Arc::clone(&service);
                let resolution_id = resolution_id.clone();
                let locator = locator.clone();
                thread::spawn(move || {
                    service
                        .resolve_registered(resolution_id, &locator)
                        .expect("shared resolution succeeds")
                })
            })
            .collect::<Vec<_>>();
        let results = threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect::<Vec<_>>();

        assert!(results.windows(2).all(|pair| pair[0] == pair[1]));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn resolver_panic_releases_attempt_for_immediate_idempotent_retry() {
        let calls = Arc::new(AtomicUsize::new(0));
        let mut service = SkillSourceResolutionService::new();
        service
            .register_resolver(Arc::new(PanicOncePreparedResolver {
                calls: Arc::clone(&calls),
            }))
            .unwrap();
        let resolution_id = SkillSourceResolutionId::new();
        let locator =
            SkillInstallationSourceLocator::url("https://github.com/example/skills").unwrap();

        let first = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            service.resolve_registered(resolution_id.clone(), &locator)
        }));
        assert!(first.is_err());
        assert!(!service
            .session_store()
            .lock()
            .unwrap()
            .resolutions
            .contains_key(&resolution_id));

        let retry = service
            .resolve_registered(resolution_id.clone(), &locator)
            .expect("the same identity can retry immediately after the panic");
        let replay = service
            .resolve_registered(resolution_id, &locator)
            .expect("the completed retry remains idempotent");

        assert_eq!(retry, replay);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn resolution_ttl_releases_resolution_capacity() {
        let clock = Arc::new(ManualClock::default());
        let config = SkillInstallationSessionConfig::new(
            1,
            1,
            DEFAULT_SKILL_SESSION_SNAPSHOT_BYTES,
            std::time::Duration::from_millis(10),
        )
        .unwrap()
        .with_resolving_ttl(std::time::Duration::from_secs(1));
        let sessions = SkillInstallationSessionStore::with_clock(config, clock.clone());
        let calls = Arc::new(AtomicUsize::new(0));
        let service = prepared_service(sessions, Arc::clone(&calls), 1);
        let locator =
            SkillInstallationSourceLocator::url("https://github.com/example/skills").unwrap();
        service
            .resolve_registered(SkillSourceResolutionId::new(), &locator)
            .unwrap();

        let second_id = SkillSourceResolutionId::new();
        let full = service
            .resolve_registered(second_id.clone(), &locator)
            .unwrap_err();
        assert_eq!(
            full.code(),
            SkillSourceResolutionErrorCode::CapacityExceeded
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        clock.advance(std::time::Duration::from_millis(11));
        service.resolve_registered(second_id, &locator).unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn cancellation_is_idempotent_and_immediately_releases_active_capacity() {
        let config = SkillInstallationSessionConfig::new(
            1,
            2,
            DEFAULT_SKILL_SESSION_SNAPSHOT_BYTES,
            std::time::Duration::from_secs(60),
        )
        .unwrap()
        .with_max_resolution_tombstones(4)
        .unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let service = prepared_service(
            SkillInstallationSessionStore::new(config),
            Arc::clone(&calls),
            1,
        );
        let locator =
            SkillInstallationSourceLocator::url("https://github.com/example/skills").unwrap();
        let first_id = SkillSourceResolutionId::new();
        service
            .resolve_registered(first_id.clone(), &locator)
            .unwrap();

        assert_eq!(
            service.cancel_registered_resolution(&first_id).unwrap(),
            SkillSourceResolutionCancellation::Cancelled
        );
        assert_eq!(
            service.cancel_registered_resolution(&first_id).unwrap(),
            SkillSourceResolutionCancellation::AlreadyCancelled
        );
        let not_yet_started_id = SkillSourceResolutionId::new();
        assert_eq!(
            service
                .cancel_registered_resolution(&not_yet_started_id)
                .unwrap(),
            SkillSourceResolutionCancellation::AlreadyAbsent
        );
        let cancelled_before_start = service
            .resolve_registered(not_yet_started_id, &locator)
            .unwrap_err();
        assert_eq!(
            cancelled_before_start.code(),
            SkillSourceResolutionErrorCode::Cancelled
        );

        service
            .resolve_registered(SkillSourceResolutionId::new(), &locator)
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn tombstones_are_bounded_independently_and_expiry_reclaims_them() {
        let clock = Arc::new(ManualClock::default());
        let config = SkillInstallationSessionConfig::new(
            1,
            1,
            DEFAULT_SKILL_SESSION_SNAPSHOT_BYTES,
            std::time::Duration::from_millis(10),
        )
        .unwrap()
        .with_max_resolution_tombstones(2)
        .unwrap();
        let sessions = SkillInstallationSessionStore::with_clock(config, clock.clone());
        let calls = Arc::new(AtomicUsize::new(0));
        let service = prepared_service(sessions, Arc::clone(&calls), 1);
        let locator =
            SkillInstallationSourceLocator::url("https://github.com/example/skills").unwrap();

        for _ in 0..2 {
            let resolution_id = SkillSourceResolutionId::new();
            service
                .resolve_registered(resolution_id.clone(), &locator)
                .unwrap();
            service
                .cancel_registered_resolution(&resolution_id)
                .unwrap();
        }
        let third_id = SkillSourceResolutionId::new();
        let full = service
            .resolve_registered(third_id.clone(), &locator)
            .unwrap_err();
        assert_eq!(
            full.code(),
            SkillSourceResolutionErrorCode::CapacityExceeded
        );
        assert_eq!(calls.load(Ordering::SeqCst), 2);

        clock.advance(std::time::Duration::from_millis(11));
        service.resolve_registered(third_id, &locator).unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn candidate_capacity_counts_exact_retained_candidates() {
        let config = SkillInstallationSessionConfig::new(
            2,
            1,
            DEFAULT_SKILL_SESSION_SNAPSHOT_BYTES,
            std::time::Duration::from_secs(60),
        )
        .unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let service = prepared_service(
            SkillInstallationSessionStore::new(config),
            Arc::clone(&calls),
            1,
        );
        let locator =
            SkillInstallationSourceLocator::url("https://github.com/example/skills").unwrap();

        service
            .resolve_registered(SkillSourceResolutionId::new(), &locator)
            .unwrap();
        let error = service
            .resolve_registered(SkillSourceResolutionId::new(), &locator)
            .unwrap_err();

        assert_eq!(
            error.code(),
            SkillSourceResolutionErrorCode::CapacityExceeded
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}
