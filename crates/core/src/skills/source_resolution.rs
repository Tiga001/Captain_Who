//! Human-facing installation locators resolved into immutable acquisition candidates.
//!
//! Resolution and acquisition are deliberately separate extension axes. A resolver accepts a
//! convenient, non-authoritative locator (currently an HTTPS URL), discovers zero or more Skill
//! packages, and returns immutable provider coordinates. The existing acquisition workflow still
//! owns package preparation, preview, and commit.

use super::github_acquisition::{GitHubCommit, GitHubRepository, GitHubSubdirectory};
use super::model::{
    SkillRevision, SKILL_PACKAGE_FORMAT_VERSION, SKILL_PACKAGE_FORMAT_VERSION_V2,
    SKILL_PACKAGE_FORMAT_VERSION_V3,
};
use reqwest::Url;
use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::sync::Arc;

const MAX_INSTALLATION_LOCATOR_BYTES: usize = 4 * 1024;
const MAX_RESOLVER_ID_BYTES: usize = 64;
const MAX_RESOLUTION_CANDIDATES: usize = 64;
const MAX_CANDIDATE_ID_BYTES: usize = 256;
const MAX_RESOLVED_REVISION_BYTES: usize = 256;

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

pub trait SkillInstallationSourceResolver: Send + Sync {
    fn id(&self) -> SkillSourceResolverId;

    /// Exact lower-case DNS hosts owned by this resolver. Registration rejects overlap. Returning
    /// owned values keeps enterprise and user-configured providers extensible.
    fn supported_hosts(&self) -> Vec<String>;

    fn resolve(
        &self,
        locator: &SkillInstallationSourceLocator,
    ) -> Result<SkillSourceResolution, SkillSourceResolutionError>;
}

pub struct SkillSourceResolutionService {
    resolvers: BTreeMap<SkillSourceResolverId, Arc<dyn SkillInstallationSourceResolver>>,
    hosts: BTreeMap<String, SkillSourceResolverId>,
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
        Self {
            resolvers: BTreeMap::new(),
            hosts: BTreeMap::new(),
        }
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
    if id.is_empty()
        || id.len() > MAX_CANDIDATE_ID_BYTES
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
    {
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

fn invalid_resolver_output() -> SkillSourceResolutionError {
    SkillSourceResolutionError::resolve(
        SkillSourceResolutionErrorCode::Unavailable,
        SkillSourceResolutionRecovery::RetryLater,
        "The Skill source resolver returned an invalid result.",
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
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SkillSourceResolutionRecovery {
    FixLocator,
    RetryLater,
    NarrowLocator,
    ChooseDifferentSource,
}

#[derive(Clone, PartialEq, Eq)]
pub struct SkillSourceResolutionError {
    phase: SkillSourceResolutionPhase,
    code: SkillSourceResolutionErrorCode,
    recovery: SkillSourceResolutionRecovery,
    message: String,
    provider: Option<SkillSourceResolverId>,
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
    use super::*;

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
    fn locator_debug_never_exposes_the_url() {
        let locator = SkillInstallationSourceLocator::url(
            "https://github.com/private-owner/private-repository",
        )
        .expect("valid bounded locator");
        let debug = format!("{locator:?}");
        assert!(!debug.contains("private-owner"));
        assert!(debug.contains("[redacted]"));
    }
}
