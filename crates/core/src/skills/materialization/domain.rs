use super::*;

pub(super) const STAGING_NAME_PREFIX: &str = ".mycopilot-skill-stage-";
const GIT_DIRECTORY: &str = ".git";
const MERCURIAL_DIRECTORY: &str = ".hg";
const SUBVERSION_DIRECTORY: &str = ".svn";
const AGENTS_DIRECTORY: &str = ".agents";

pub const MAX_SKILL_MATERIALIZATION_FILE_BYTES: usize = MAX_SKILL_RESOURCE_FILE_BYTES;
pub const MAX_SKILL_MATERIALIZATION_TREE_FILES: usize = MAX_SKILL_PACKAGE_FILES - 1;
pub const MAX_SKILL_MATERIALIZATION_TREE_DIRECTORIES: usize = MAX_SKILL_PACKAGE_DIRECTORIES;
pub const MAX_SKILL_MATERIALIZATION_TREE_BYTES: usize = MAX_SKILL_PACKAGE_BYTES;
pub const SKILL_MATERIALIZATION_TREE_DIGEST_PREFIX: &str = "skill-materialization-tree-sha256-v1:";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SkillMaterializationStatus {
    Created,
    AlreadyPresent,
}

impl SkillMaterializationStatus {
    pub fn stable_name(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::AlreadyPresent => "alreadyPresent",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SkillMaterializationErrorCode {
    InvalidDestination,
    ReservedDestination,
    InvalidWorkspace,
    ResourceError,
    SourceKindDenied,
    SourcePrefixDenied,
    EmptySource,
    ResourceBudgetExceeded,
    DestinationParentNotFound,
    UnsafeParent,
    UnsafeDestination,
    Conflict,
    Io,
    CommitIndeterminate,
    UnsupportedPlatform,
}

impl SkillMaterializationErrorCode {
    pub fn stable_name(self) -> &'static str {
        match self {
            Self::InvalidDestination => "invalidDestination",
            Self::ReservedDestination => "reservedDestination",
            Self::InvalidWorkspace => "invalidWorkspace",
            Self::ResourceError => "resourceError",
            Self::SourceKindDenied => "sourceKindDenied",
            Self::SourcePrefixDenied => "sourcePrefixDenied",
            Self::EmptySource => "emptySource",
            Self::ResourceBudgetExceeded => "resourceBudgetExceeded",
            Self::DestinationParentNotFound => "destinationParentNotFound",
            Self::UnsafeParent => "unsafeParent",
            Self::UnsafeDestination => "unsafeDestination",
            Self::Conflict => "conflict",
            Self::Io => "io",
            Self::CommitIndeterminate => "commitIndeterminate",
            Self::UnsupportedPlatform => "unsupportedPlatform",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SkillMaterializationRecovery {
    ChangeRequest,
    ReactivateSkill,
    CreateParent,
    ChooseDestination,
    RepairWorkspace,
    Retry,
    InspectDestination,
    UseSupportedPlatform,
}

impl SkillMaterializationRecovery {
    pub fn stable_name(self) -> &'static str {
        match self {
            Self::ChangeRequest => "changeRequest",
            Self::ReactivateSkill => "reactivateSkill",
            Self::CreateParent => "createParent",
            Self::ChooseDestination => "chooseDestination",
            Self::RepairWorkspace => "repairWorkspace",
            Self::Retry => "retry",
            Self::InspectDestination => "inspectDestination",
            Self::UseSupportedPlatform => "useSupportedPlatform",
        }
    }
}

/// A validated, portable path relative to the selected workspace root.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SkillMaterializationDestination(String);

impl SkillMaterializationDestination {
    pub fn parse(value: impl Into<String>) -> Result<Self, SkillMaterializationError> {
        let value = value.into();
        let path = SkillPackagePath::parse(value).map_err(|error| {
            SkillMaterializationError::InvalidDestination {
                reason: error.message,
            }
        })?;
        let components = path.as_str().split('/').collect::<Vec<_>>();
        if components.iter().any(|component| {
            matches_ignore_ascii_case(
                component,
                &[
                    GIT_DIRECTORY,
                    MERCURIAL_DIRECTORY,
                    SUBVERSION_DIRECTORY,
                    AGENTS_DIRECTORY,
                ],
            )
        }) {
            return Err(SkillMaterializationError::ReservedDestination {
                reason: "Skill resources cannot be materialized into repository metadata or the workspace Skill source tree".to_string(),
            });
        }
        if components.iter().any(|component| {
            component
                .to_ascii_lowercase()
                .starts_with(STAGING_NAME_PREFIX)
        }) {
            return Err(SkillMaterializationError::ReservedDestination {
                reason: "destination collides with the materialization transaction namespace"
                    .to_string(),
            });
        }
        Ok(Self(path.as_str().to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub(super) fn components(&self) -> impl DoubleEndedIterator<Item = &str> {
        self.0.split('/')
    }
}

impl fmt::Display for SkillMaterializationDestination {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

fn matches_ignore_ascii_case(value: &str, candidates: &[&str]) -> bool {
    candidates
        .iter()
        .any(|candidate| value.eq_ignore_ascii_case(candidate))
}

#[derive(Clone)]
pub struct SkillMaterializationRequest {
    source: SkillResourceUri,
    workspace_root: PathBuf,
    workspace_identity: Option<crate::file_change::FileChangeDirectoryIdentity>,
    destination: SkillMaterializationDestination,
}

impl SkillMaterializationRequest {
    pub fn new(
        source: SkillResourceUri,
        workspace_root: impl Into<PathBuf>,
        destination: SkillMaterializationDestination,
    ) -> Result<Self, SkillMaterializationError> {
        let workspace_root = workspace_root.into();
        if !workspace_root.is_absolute() {
            return Err(SkillMaterializationError::InvalidWorkspace {
                reason: "workspace root must be absolute".to_string(),
            });
        }
        Ok(Self {
            source,
            workspace_root,
            workspace_identity: None,
            destination,
        })
    }

    pub fn source(&self) -> &SkillResourceUri {
        &self.source
    }

    pub fn destination(&self) -> &SkillMaterializationDestination {
        &self.destination
    }

    pub(super) fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    pub fn with_workspace_identity(
        mut self,
        identity: crate::file_change::FileChangeDirectoryIdentity,
    ) -> Self {
        self.workspace_identity = Some(identity);
        self
    }

    pub(super) fn workspace_identity(
        &self,
    ) -> Option<&crate::file_change::FileChangeDirectoryIdentity> {
        self.workspace_identity.as_ref()
    }
}

impl fmt::Debug for SkillMaterializationRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SkillMaterializationRequest")
            .field("source", &self.source)
            .field("destination", &self.destination)
            .field("workspace_root", &"[private workspace]")
            .finish()
    }
}

#[derive(Clone)]
pub struct SkillTemplateTreeMaterializationRequest {
    source: SkillPackageUri,
    source_prefix: SkillResourcePath,
    workspace_root: PathBuf,
    workspace_identity: Option<crate::file_change::FileChangeDirectoryIdentity>,
    destination: SkillMaterializationDestination,
}

impl SkillTemplateTreeMaterializationRequest {
    pub fn new(
        source: SkillPackageUri,
        source_prefix: SkillResourcePath,
        workspace_root: impl Into<PathBuf>,
        destination: SkillMaterializationDestination,
    ) -> Result<Self, SkillMaterializationError> {
        if source_prefix.as_str() != "templates"
            && !source_prefix.as_str().starts_with("templates/")
        {
            return Err(SkillMaterializationError::SourcePrefixDenied {
                prefix: source_prefix,
            });
        }
        let workspace_root = workspace_root.into();
        if !workspace_root.is_absolute() {
            return Err(SkillMaterializationError::InvalidWorkspace {
                reason: "workspace root must be absolute".to_string(),
            });
        }
        Ok(Self {
            source,
            source_prefix,
            workspace_root,
            workspace_identity: None,
            destination,
        })
    }

    pub fn source(&self) -> &SkillPackageUri {
        &self.source
    }

    pub fn source_prefix(&self) -> &SkillResourcePath {
        &self.source_prefix
    }

    pub fn destination(&self) -> &SkillMaterializationDestination {
        &self.destination
    }

    pub(super) fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    pub fn with_workspace_identity(
        mut self,
        identity: crate::file_change::FileChangeDirectoryIdentity,
    ) -> Self {
        self.workspace_identity = Some(identity);
        self
    }

    pub(super) fn workspace_identity(
        &self,
    ) -> Option<&crate::file_change::FileChangeDirectoryIdentity> {
        self.workspace_identity.as_ref()
    }
}

impl fmt::Debug for SkillTemplateTreeMaterializationRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SkillTemplateTreeMaterializationRequest")
            .field("source", &self.source)
            .field("source_prefix", &self.source_prefix)
            .field("destination", &self.destination)
            .field("workspace_root", &"[private workspace]")
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillMaterializationOutcome {
    pub(super) status: SkillMaterializationStatus,
    pub(super) source: SkillResourceUri,
    pub(super) destination: SkillMaterializationDestination,
    pub(super) descriptor: SkillResourceDescriptor,
    pub(super) bytes_written: u64,
}

impl SkillMaterializationOutcome {
    pub fn status(&self) -> SkillMaterializationStatus {
        self.status
    }

    pub fn source(&self) -> &SkillResourceUri {
        &self.source
    }

    pub fn destination(&self) -> &SkillMaterializationDestination {
        &self.destination
    }

    pub fn descriptor(&self) -> &SkillResourceDescriptor {
        &self.descriptor
    }

    pub fn content_digest(&self) -> &str {
        self.descriptor.content_digest()
    }

    pub fn byte_length(&self) -> u64 {
        self.descriptor.byte_length()
    }

    /// Bytes newly published by this call. This is zero for an idempotent hit.
    pub fn bytes_written(&self) -> u64 {
        self.bytes_written
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillMaterializedTreeEntry {
    pub(super) source: SkillResourceUri,
    pub(super) relative_path: String,
    pub(super) descriptor: SkillResourceDescriptor,
}

impl SkillMaterializedTreeEntry {
    pub fn source(&self) -> &SkillResourceUri {
        &self.source
    }

    pub fn relative_path(&self) -> &str {
        &self.relative_path
    }

    pub fn descriptor(&self) -> &SkillResourceDescriptor {
        &self.descriptor
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillTemplateTreeMaterializationOutcome {
    pub(super) status: SkillMaterializationStatus,
    pub(super) source: SkillPackageUri,
    pub(super) source_prefix: SkillResourcePath,
    pub(super) destination: SkillMaterializationDestination,
    pub(super) entries: Vec<SkillMaterializedTreeEntry>,
    pub(super) byte_length: u64,
    pub(super) bytes_written: u64,
    pub(super) plan_digest: String,
}

impl SkillTemplateTreeMaterializationOutcome {
    pub fn status(&self) -> SkillMaterializationStatus {
        self.status
    }

    pub fn source(&self) -> &SkillPackageUri {
        &self.source
    }

    pub fn source_prefix(&self) -> &SkillResourcePath {
        &self.source_prefix
    }

    pub fn destination(&self) -> &SkillMaterializationDestination {
        &self.destination
    }

    pub fn entries(&self) -> &[SkillMaterializedTreeEntry] {
        &self.entries
    }

    pub fn file_count(&self) -> usize {
        self.entries.len()
    }

    pub fn byte_length(&self) -> u64 {
        self.byte_length
    }

    pub fn bytes_written(&self) -> u64 {
        self.bytes_written
    }

    pub fn plan_digest(&self) -> &str {
        &self.plan_digest
    }
}

#[derive(Debug)]
#[non_exhaustive]
pub enum SkillMaterializationError {
    InvalidDestination {
        reason: String,
    },
    ReservedDestination {
        reason: String,
    },
    InvalidWorkspace {
        reason: String,
    },
    Resource {
        source: Box<SkillResourceError>,
    },
    SourceKindDenied {
        uri: Box<SkillResourceUri>,
        kind: SkillResourceKind,
    },
    SourcePrefixDenied {
        prefix: SkillResourcePath,
    },
    EmptySource {
        package: SkillPackageUri,
        prefix: SkillResourcePath,
    },
    ResourceBudgetExceeded {
        reason: String,
    },
    DestinationParentNotFound {
        destination: SkillMaterializationDestination,
    },
    UnsafeParent {
        destination: SkillMaterializationDestination,
        reason: String,
    },
    UnsafeDestination {
        destination: SkillMaterializationDestination,
        reason: String,
    },
    Conflict {
        destination: SkillMaterializationDestination,
        expected_digest: String,
        existing_digest: Option<String>,
    },
    Io {
        operation: &'static str,
        destination: SkillMaterializationDestination,
        reason: String,
    },
    CommitIndeterminate {
        destination: SkillMaterializationDestination,
        expected_digest: String,
        reason: String,
    },
    UnsupportedPlatform,
}

impl SkillMaterializationError {
    pub fn code(&self) -> SkillMaterializationErrorCode {
        match self {
            Self::InvalidDestination { .. } => SkillMaterializationErrorCode::InvalidDestination,
            Self::ReservedDestination { .. } => SkillMaterializationErrorCode::ReservedDestination,
            Self::InvalidWorkspace { .. } => SkillMaterializationErrorCode::InvalidWorkspace,
            Self::Resource { .. } => SkillMaterializationErrorCode::ResourceError,
            Self::SourceKindDenied { .. } => SkillMaterializationErrorCode::SourceKindDenied,
            Self::SourcePrefixDenied { .. } => SkillMaterializationErrorCode::SourcePrefixDenied,
            Self::EmptySource { .. } => SkillMaterializationErrorCode::EmptySource,
            Self::ResourceBudgetExceeded { .. } => {
                SkillMaterializationErrorCode::ResourceBudgetExceeded
            }
            Self::DestinationParentNotFound { .. } => {
                SkillMaterializationErrorCode::DestinationParentNotFound
            }
            Self::UnsafeParent { .. } => SkillMaterializationErrorCode::UnsafeParent,
            Self::UnsafeDestination { .. } => SkillMaterializationErrorCode::UnsafeDestination,
            Self::Conflict { .. } => SkillMaterializationErrorCode::Conflict,
            Self::Io { .. } => SkillMaterializationErrorCode::Io,
            Self::CommitIndeterminate { .. } => SkillMaterializationErrorCode::CommitIndeterminate,
            Self::UnsupportedPlatform => SkillMaterializationErrorCode::UnsupportedPlatform,
        }
    }

    pub fn recovery(&self) -> SkillMaterializationRecovery {
        match self {
            Self::InvalidDestination { .. }
            | Self::ReservedDestination { .. }
            | Self::SourceKindDenied { .. }
            | Self::SourcePrefixDenied { .. }
            | Self::EmptySource { .. }
            | Self::ResourceBudgetExceeded { .. } => SkillMaterializationRecovery::ChangeRequest,
            Self::InvalidWorkspace { .. } | Self::UnsafeParent { .. } => {
                SkillMaterializationRecovery::RepairWorkspace
            }
            Self::Resource { .. } => SkillMaterializationRecovery::ReactivateSkill,
            Self::DestinationParentNotFound { .. } => SkillMaterializationRecovery::CreateParent,
            Self::UnsafeDestination { .. } | Self::Conflict { .. } => {
                SkillMaterializationRecovery::ChooseDestination
            }
            Self::Io { .. } => SkillMaterializationRecovery::Retry,
            Self::CommitIndeterminate { .. } => SkillMaterializationRecovery::InspectDestination,
            Self::UnsupportedPlatform => SkillMaterializationRecovery::UseSupportedPlatform,
        }
    }

    pub fn message(&self) -> String {
        self.to_string()
    }

    pub fn destination(&self) -> Option<&SkillMaterializationDestination> {
        match self {
            Self::DestinationParentNotFound { destination }
            | Self::UnsafeParent { destination, .. }
            | Self::UnsafeDestination { destination, .. }
            | Self::Conflict { destination, .. }
            | Self::Io { destination, .. }
            | Self::CommitIndeterminate { destination, .. } => Some(destination),
            _ => None,
        }
    }

    pub fn resource_error(&self) -> Option<&SkillResourceError> {
        match self {
            Self::Resource { source } => Some(source),
            _ => None,
        }
    }
}

impl fmt::Display for SkillMaterializationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidDestination { reason } => {
                write!(formatter, "invalid materialization destination: {reason}")
            }
            Self::ReservedDestination { reason } => {
                write!(formatter, "reserved materialization destination: {reason}")
            }
            Self::InvalidWorkspace { reason } => {
                write!(formatter, "invalid materialization workspace: {reason}")
            }
            Self::Resource { source } => source.fmt(formatter),
            Self::SourceKindDenied { uri, kind } => write!(
                formatter,
                "Skill resource `{uri}` has kind `{}` and is not an asset or a templates/ resource",
                kind.stable_name()
            ),
            Self::SourcePrefixDenied { prefix } => write!(
                formatter,
                "Skill resource prefix `{prefix}` is not below templates/"
            ),
            Self::EmptySource { package, prefix } => write!(
                formatter,
                "Skill package `{package}` has no resources below `{prefix}/`"
            ),
            Self::ResourceBudgetExceeded { reason } => {
                write!(formatter, "Skill materialization plan exceeds its budget: {reason}")
            }
            Self::DestinationParentNotFound { destination } => write!(
                formatter,
                "materialization parent for `{destination}` does not exist"
            ),
            Self::UnsafeParent {
                destination,
                reason,
            } => write!(
                formatter,
                "materialization parent for `{destination}` is unsafe: {reason}"
            ),
            Self::UnsafeDestination {
                destination,
                reason,
            } => write!(
                formatter,
                "materialization destination `{destination}` is unsafe: {reason}"
            ),
            Self::Conflict {
                destination,
                expected_digest,
                existing_digest,
            } => match existing_digest {
                Some(existing) => write!(
                    formatter,
                    "materialization destination `{destination}` already contains different content (expected {expected_digest}, found {existing})"
                ),
                None => write!(
                    formatter,
                    "materialization destination `{destination}` already exists and does not exactly match the requested content"
                ),
            },
            Self::Io {
                operation,
                destination,
                reason,
            } => write!(
                formatter,
                "cannot {operation} materialization destination `{destination}`: {reason}"
            ),
            Self::CommitIndeterminate {
                destination,
                expected_digest,
                reason,
            } => write!(
                formatter,
                "materialization of `{destination}` may have committed digest {expected_digest}, but durability could not be confirmed: {reason}"
            ),
            Self::UnsupportedPlatform => write!(
                formatter,
                "this platform does not provide the required handle-relative atomic no-replace filesystem primitives"
            ),
        }
    }
}

impl Error for SkillMaterializationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Resource { source } => Some(source),
            _ => None,
        }
    }
}

impl From<SkillResourceError> for SkillMaterializationError {
    fn from(source: SkillResourceError) -> Self {
        Self::Resource {
            source: Box::new(source),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TreeFileFingerprint {
    pub(super) byte_length: u64,
    pub(super) content_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TreeFingerprint {
    pub(super) directories: BTreeSet<String>,
    pub(super) files: BTreeMap<String, TreeFileFingerprint>,
}

impl TreeFingerprint {
    pub(super) fn digest(&self) -> String {
        let mut hasher = Sha256::new();
        update_digest_field(&mut hasher, b"skill-materialization-tree-v1");
        for directory in &self.directories {
            update_digest_field(&mut hasher, b"directory");
            update_digest_field(&mut hasher, directory.as_bytes());
        }
        for (path, file) in &self.files {
            update_digest_field(&mut hasher, b"file");
            update_digest_field(&mut hasher, path.as_bytes());
            update_digest_field(&mut hasher, &file.byte_length.to_be_bytes());
            update_digest_field(&mut hasher, file.content_digest.as_bytes());
        }
        format!(
            "{SKILL_MATERIALIZATION_TREE_DIGEST_PREFIX}{:x}",
            hasher.finalize()
        )
    }
}

fn update_digest_field(hasher: &mut Sha256, value: &[u8]) {
    hasher.update(u64::try_from(value.len()).unwrap_or(u64::MAX).to_be_bytes());
    hasher.update(value);
}
