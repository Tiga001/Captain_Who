use super::*;

pub(super) const LOCAL_DIRECTORY_PROVIDER: &str = "local-directory";

pub(super) const RESOLVED_CANDIDATE_PROVIDER: &str = "resolved-candidate";

pub(super) const INSTALLED_SOURCE_PROVIDER: &str = "installed-source";

pub(super) const LOCAL_DIRECTORY_ORIGIN_REFERENCE: &str = "user-selected-directory";

pub(super) const LOCAL_DIRECTORY_PROVENANCE_AUTHORITY: &str = "user-selected-snapshot";

pub(super) const MAX_PROVIDER_BYTES: usize = 64;

pub(super) const MAX_ADAPTER_REQUEST_BYTES: usize = 64 * 1024;

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

    pub(super) fn local_directory() -> Self {
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
    pub(super) reason: String,
}

impl SkillAcquisitionProviderError {
    pub(super) fn new(reason: impl Into<String>) -> Self {
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
    pub(super) reason: String,
}

impl SkillAcquisitionSourceError {
    pub(super) fn new(reason: impl Into<String>) -> Self {
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

/// Sanitized acquisition metadata persisted with the installed receipt.
///
/// Local-directory acquisition uses a fixed reference and therefore never
/// exposes the selected path. Network adapters must similarly exclude
/// credentials and may encode stable details such as a resolved commit.
#[derive(Clone, PartialEq, Eq)]
pub struct SkillAcquisitionPresentation {
    pub(super) provider: String,
    pub(super) reference: String,
}

pub(super) const MAX_INSTALLED_SOURCE_DISPLAY_NAME_BYTES: usize = 256;

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

    pub(super) fn has_valid_boundary_fields(&self) -> bool {
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

pub(super) struct LocalDirectoryAcquisitionAdapter;

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
