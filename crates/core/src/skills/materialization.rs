//! Safe, create-only materialization of revision-bound Skill resources.
//!
//! This module is deliberately downstream of [`SkillResourceSession`]. It
//! never accepts a managed-store path and never resolves mutable installation
//! state. It supports one exact logical URI as an atomic file publication and
//! a complete `templates/**` subtree as an atomic directory publication.

use super::digest::package_file_digest;
use super::model::{SkillResourceDescriptor, SkillResourceKind};
use super::package::{
    SkillPackagePath, MAX_SKILL_PACKAGE_BYTES, MAX_SKILL_PACKAGE_DIRECTORIES,
    MAX_SKILL_PACKAGE_FILES, MAX_SKILL_RESOURCE_FILE_BYTES,
};
use super::resource_runtime::{
    SkillPackageUri, SkillResourceError, SkillResourceListOptions, SkillResourcePath,
    SkillResourceSession, SkillResourceUri, MAX_SKILL_RESOURCE_LIST_PAGE_SIZE,
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::path::{Path, PathBuf};

const STAGING_NAME_PREFIX: &str = ".mycopilot-skill-stage-";
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

    fn components(&self) -> impl DoubleEndedIterator<Item = &str> {
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
            destination,
        })
    }

    pub fn source(&self) -> &SkillResourceUri {
        &self.source
    }

    pub fn destination(&self) -> &SkillMaterializationDestination {
        &self.destination
    }

    fn workspace_root(&self) -> &Path {
        &self.workspace_root
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

    fn workspace_root(&self) -> &Path {
        &self.workspace_root
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
    status: SkillMaterializationStatus,
    source: SkillResourceUri,
    destination: SkillMaterializationDestination,
    descriptor: SkillResourceDescriptor,
    bytes_written: u64,
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
    source: SkillResourceUri,
    relative_path: String,
    descriptor: SkillResourceDescriptor,
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
    status: SkillMaterializationStatus,
    source: SkillPackageUri,
    source_prefix: SkillResourcePath,
    destination: SkillMaterializationDestination,
    entries: Vec<SkillMaterializedTreeEntry>,
    byte_length: u64,
    bytes_written: u64,
    plan_digest: String,
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
struct TreeFileFingerprint {
    byte_length: u64,
    content_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TreeFingerprint {
    directories: BTreeSet<String>,
    files: BTreeMap<String, TreeFileFingerprint>,
}

impl TreeFingerprint {
    fn digest(&self) -> String {
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

#[derive(Debug)]
struct PreparedTemplateFile {
    source: SkillResourceUri,
    relative_path: String,
    descriptor: SkillResourceDescriptor,
    bytes: Vec<u8>,
}

#[derive(Debug)]
struct PreparedTemplateTree {
    files: Vec<PreparedTemplateFile>,
    fingerprint: TreeFingerprint,
    byte_length: u64,
    plan_digest: String,
}

fn prepare_template_tree(
    session: &SkillResourceSession,
    request: &SkillTemplateTreeMaterializationRequest,
) -> Result<PreparedTemplateTree, SkillMaterializationError> {
    let prefix = request.source_prefix().as_str();
    let descendant_prefix = format!("{prefix}/");
    let mut after = None;
    let mut listed = BTreeMap::<String, (SkillResourceUri, SkillResourceDescriptor)>::new();

    loop {
        let mut options = SkillResourceListOptions::new(MAX_SKILL_RESOURCE_LIST_PAGE_SIZE)?
            .with_prefix(request.source_prefix().clone());
        if let Some(cursor) = after.take() {
            options = options.with_after(cursor);
        }
        let page = session.list(request.source(), &options)?;
        for entry in page.entries() {
            let Some(relative_path) = entry.descriptor().path().strip_prefix(&descendant_prefix)
            else {
                // `list` intentionally includes an exact prefix match. A tree
                // publication has no file location for that match, so only
                // strict descendants participate.
                continue;
            };
            let relative = SkillPackagePath::parse(relative_path.to_string()).map_err(|error| {
                SkillMaterializationError::Resource {
                    source: Box::new(SkillResourceError::IntegrityMismatch {
                        uri: Box::new(entry.uri().clone()),
                        reason: format!(
                            "template subtree contains an invalid relative path: {}",
                            error.message
                        ),
                    }),
                }
            })?;
            if listed.len() >= MAX_SKILL_MATERIALIZATION_TREE_FILES {
                return Err(SkillMaterializationError::ResourceBudgetExceeded {
                    reason: format!(
                        "template subtree contains more than {MAX_SKILL_MATERIALIZATION_TREE_FILES} files"
                    ),
                });
            }
            if listed
                .insert(
                    relative.as_str().to_string(),
                    (entry.uri().clone(), entry.descriptor().clone()),
                )
                .is_some()
            {
                return Err(SkillMaterializationError::Resource {
                    source: Box::new(SkillResourceError::IntegrityMismatch {
                        uri: Box::new(entry.uri().clone()),
                        reason: "template subtree contains a duplicate relative path".to_string(),
                    }),
                });
            }
        }
        after = page.next_after().cloned();
        if after.is_none() {
            break;
        }
    }

    if listed.is_empty() {
        return Err(SkillMaterializationError::EmptySource {
            package: request.source().clone(),
            prefix: request.source_prefix().clone(),
        });
    }

    let mut files = Vec::with_capacity(listed.len());
    let mut directories = BTreeSet::new();
    let mut fingerprints = BTreeMap::new();
    let mut byte_length = 0u64;
    for (relative_path, (uri, listed_descriptor)) in listed {
        for (index, _) in relative_path.match_indices('/') {
            directories.insert(relative_path[..index].to_string());
        }
        if directories.len() > MAX_SKILL_MATERIALIZATION_TREE_DIRECTORIES {
            return Err(SkillMaterializationError::ResourceBudgetExceeded {
                reason: format!(
                    "template subtree contains more than {MAX_SKILL_MATERIALIZATION_TREE_DIRECTORIES} directories"
                ),
            });
        }

        byte_length = byte_length
            .checked_add(listed_descriptor.byte_length())
            .ok_or_else(|| SkillMaterializationError::ResourceBudgetExceeded {
                reason: "template subtree byte length overflowed".to_string(),
            })?;
        if byte_length > u64::try_from(MAX_SKILL_MATERIALIZATION_TREE_BYTES).unwrap_or(u64::MAX) {
            return Err(SkillMaterializationError::ResourceBudgetExceeded {
                reason: format!(
                    "template subtree exceeds {MAX_SKILL_MATERIALIZATION_TREE_BYTES} bytes"
                ),
            });
        }

        let snapshot = session.read_verified_bytes(&uri)?;
        if snapshot.descriptor != listed_descriptor {
            return Err(SkillMaterializationError::Resource {
                source: Box::new(SkillResourceError::IntegrityMismatch {
                    uri: Box::new(uri),
                    reason:
                        "resource descriptor changed while the materialization plan was prepared"
                            .to_string(),
                }),
            });
        }
        fingerprints.insert(
            relative_path.clone(),
            TreeFileFingerprint {
                byte_length: snapshot.descriptor.byte_length(),
                content_digest: snapshot.descriptor.content_digest().to_string(),
            },
        );
        files.push(PreparedTemplateFile {
            source: uri,
            relative_path,
            descriptor: snapshot.descriptor,
            bytes: snapshot.bytes,
        });
    }

    let fingerprint = TreeFingerprint {
        directories,
        files: fingerprints,
    };
    let plan_digest = fingerprint.digest();
    Ok(PreparedTemplateTree {
        files,
        fingerprint,
        byte_length,
        plan_digest,
    })
}

/// Stateless executor for revision-bound Skill resource materialization.
#[derive(Debug, Default, Clone, Copy)]
pub struct SkillResourceMaterializer;

impl SkillResourceMaterializer {
    pub const fn new() -> Self {
        Self
    }

    pub fn materialize(
        &self,
        session: &SkillResourceSession,
        request: &SkillMaterializationRequest,
    ) -> Result<SkillMaterializationOutcome, SkillMaterializationError> {
        let snapshot = session.read_verified_bytes(request.source())?;
        if snapshot.bytes.len() > MAX_SKILL_MATERIALIZATION_FILE_BYTES {
            return Err(SkillMaterializationError::Resource {
                source: Box::new(SkillResourceError::IntegrityMismatch {
                    uri: Box::new(request.source().clone()),
                    reason: format!(
                        "resource exceeds the {MAX_SKILL_MATERIALIZATION_FILE_BYTES}-byte materialization limit"
                    ),
                }),
            });
        }
        let descriptor = snapshot.descriptor;
        if descriptor.kind() != SkillResourceKind::Asset
            && !descriptor.path().starts_with("templates/")
        {
            return Err(SkillMaterializationError::SourceKindDenied {
                uri: Box::new(request.source().clone()),
                kind: descriptor.kind(),
            });
        }

        let status = materialize_file(
            request.workspace_root(),
            request.destination(),
            &descriptor,
            &snapshot.bytes,
        )?;
        Ok(SkillMaterializationOutcome {
            status,
            source: request.source().clone(),
            destination: request.destination().clone(),
            descriptor,
            bytes_written: if status == SkillMaterializationStatus::Created {
                u64::try_from(snapshot.bytes.len()).unwrap_or(u64::MAX)
            } else {
                0
            },
        })
    }

    /// Publishes a complete `templates/**` subtree as one create-only
    /// directory transaction. The destination directory is never merged or
    /// overwritten. An already-present byte-for-byte identical tree is an
    /// idempotent success.
    pub fn materialize_template_tree(
        &self,
        session: &SkillResourceSession,
        request: &SkillTemplateTreeMaterializationRequest,
    ) -> Result<SkillTemplateTreeMaterializationOutcome, SkillMaterializationError> {
        // Complete source verification deliberately precedes all workspace
        // filesystem activity. A tampered or unavailable package can never
        // leave a partial destination or staging tree.
        let prepared = prepare_template_tree(session, request)?;
        let status = materialize_tree(request.workspace_root(), request.destination(), &prepared)?;
        let bytes_written = if status == SkillMaterializationStatus::Created {
            prepared.byte_length
        } else {
            0
        };
        Ok(SkillTemplateTreeMaterializationOutcome {
            status,
            source: request.source().clone(),
            source_prefix: request.source_prefix().clone(),
            destination: request.destination().clone(),
            entries: prepared
                .files
                .into_iter()
                .map(|file| SkillMaterializedTreeEntry {
                    source: file.source,
                    relative_path: file.relative_path,
                    descriptor: file.descriptor,
                })
                .collect(),
            byte_length: prepared.byte_length,
            bytes_written,
            plan_digest: prepared.plan_digest,
        })
    }
}

#[cfg(any(target_vendor = "apple", target_os = "linux", target_os = "android"))]
fn materialize_file(
    workspace_root: &Path,
    destination: &SkillMaterializationDestination,
    descriptor: &SkillResourceDescriptor,
    bytes: &[u8],
) -> Result<SkillMaterializationStatus, SkillMaterializationError> {
    unix::materialize_file(workspace_root, destination, descriptor, bytes)
}

#[cfg(not(any(target_vendor = "apple", target_os = "linux", target_os = "android")))]
fn materialize_file(
    _workspace_root: &Path,
    _destination: &SkillMaterializationDestination,
    _descriptor: &SkillResourceDescriptor,
    _bytes: &[u8],
) -> Result<SkillMaterializationStatus, SkillMaterializationError> {
    Err(SkillMaterializationError::UnsupportedPlatform)
}

#[cfg(any(target_vendor = "apple", target_os = "linux", target_os = "android"))]
fn materialize_tree(
    workspace_root: &Path,
    destination: &SkillMaterializationDestination,
    prepared: &PreparedTemplateTree,
) -> Result<SkillMaterializationStatus, SkillMaterializationError> {
    unix::materialize_tree(workspace_root, destination, prepared)
}

#[cfg(not(any(target_vendor = "apple", target_os = "linux", target_os = "android")))]
fn materialize_tree(
    _workspace_root: &Path,
    _destination: &SkillMaterializationDestination,
    _prepared: &PreparedTemplateTree,
) -> Result<SkillMaterializationStatus, SkillMaterializationError> {
    Err(SkillMaterializationError::UnsupportedPlatform)
}

#[cfg(any(target_vendor = "apple", target_os = "linux", target_os = "android"))]
mod unix {
    use super::*;
    use std::ffi::{CStr, CString};
    use std::fs::File;
    use std::io::{self, Read, Write};
    use std::mem::MaybeUninit;
    use std::os::fd::{AsRawFd, FromRawFd, RawFd};
    use std::os::unix::ffi::OsStrExt;
    use uuid::Uuid;

    const MAX_STAGING_NAME_ATTEMPTS: usize = 16;

    #[cfg(test)]
    #[derive(Clone, Copy, PartialEq, Eq)]
    pub(super) enum MaterializationTestHookPoint {
        FileBeforePublish,
        TreeBeforePublish,
    }

    #[cfg(test)]
    type MaterializationTestHook = dyn FnMut(MaterializationTestHookPoint, RawFd, &CStr);

    #[cfg(test)]
    thread_local! {
        static MATERIALIZATION_TEST_HOOK: std::cell::RefCell<Option<Box<MaterializationTestHook>>> =
            std::cell::RefCell::new(None);
    }

    #[cfg(test)]
    pub(super) struct MaterializationTestHookGuard;

    #[cfg(test)]
    impl Drop for MaterializationTestHookGuard {
        fn drop(&mut self) {
            MATERIALIZATION_TEST_HOOK.with(|slot| {
                slot.borrow_mut().take();
            });
        }
    }

    #[cfg(test)]
    pub(super) fn install_materialization_test_hook(
        hook: impl FnMut(MaterializationTestHookPoint, RawFd, &CStr) + 'static,
    ) -> MaterializationTestHookGuard {
        MATERIALIZATION_TEST_HOOK.with(|slot| {
            assert!(slot.borrow_mut().replace(Box::new(hook)).is_none());
        });
        MaterializationTestHookGuard
    }

    #[cfg(test)]
    fn invoke_materialization_test_hook(
        point: MaterializationTestHookPoint,
        parent_fd: RawFd,
        staging_name: &CStr,
    ) {
        MATERIALIZATION_TEST_HOOK.with(|slot| {
            if let Some(hook) = slot.borrow_mut().as_mut() {
                hook(point, parent_fd, staging_name);
            }
        });
    }

    pub(super) fn materialize_file(
        workspace_root: &Path,
        destination: &SkillMaterializationDestination,
        descriptor: &SkillResourceDescriptor,
        bytes: &[u8],
    ) -> Result<SkillMaterializationStatus, SkillMaterializationError> {
        let root = open_workspace_root(workspace_root, destination)?;
        let components = destination.components().collect::<Vec<_>>();
        let (file_name, parents) = components
            .split_last()
            .expect("validated destination has at least one component");
        let target_name = CString::new(*file_name).expect("validated component contains no NUL");
        let opened = open_parent_chain(root, parents, destination)?;
        opened.verify(destination)?;

        match inspect_target(opened.parent(), &target_name, descriptor, destination)? {
            TargetState::Absent => {}
            TargetState::Identical => return Ok(SkillMaterializationStatus::AlreadyPresent),
            TargetState::Different(existing_digest) => {
                return Err(conflict(destination, descriptor, existing_digest))
            }
        }

        let mut staging = create_staging(opened.parent(), destination)?;
        staging
            .file_mut()
            .write_all(bytes)
            .map_err(|error| io_error("write", destination, error))?;
        staging
            .file()
            .sync_all()
            .map_err(|error| io_error("synchronize", destination, error))?;

        // Revalidate every parent directory after staging and immediately
        // before publication. The rename itself remains relative to the opened
        // parent handle and therefore never follows a replaced symlink.
        #[cfg(test)]
        invoke_materialization_test_hook(
            MaterializationTestHookPoint::FileBeforePublish,
            opened.parent().as_raw_fd(),
            staging.name(),
        );
        opened.verify(destination)?;
        staging.verify_link(destination)?;
        match rename_noreplace(opened.parent().as_raw_fd(), staging.name(), &target_name) {
            Ok(()) => {
                if let Err(reason) = staging.verify_published(opened.parent(), &target_name) {
                    return Err(SkillMaterializationError::CommitIndeterminate {
                        destination: destination.clone(),
                        expected_digest: descriptor.content_digest().to_string(),
                        reason,
                    });
                }
                staging.disarm();
            }
            Err(error) if is_already_exists(&error) => {
                return match inspect_target(opened.parent(), &target_name, descriptor, destination)?
                {
                    TargetState::Identical => Ok(SkillMaterializationStatus::AlreadyPresent),
                    TargetState::Different(existing_digest) => {
                        Err(conflict(destination, descriptor, existing_digest))
                    }
                    TargetState::Absent => Err(SkillMaterializationError::Conflict {
                        destination: destination.clone(),
                        expected_digest: descriptor.content_digest().to_string(),
                        existing_digest: None,
                    }),
                };
            }
            Err(error) if is_unsupported(&error) => {
                return Err(SkillMaterializationError::UnsupportedPlatform)
            }
            Err(error) => return Err(io_error("publish", destination, error)),
        }

        if let Err(error) = opened.parent().sync_all() {
            return Err(SkillMaterializationError::CommitIndeterminate {
                destination: destination.clone(),
                expected_digest: descriptor.content_digest().to_string(),
                reason: error.to_string(),
            });
        }
        if let Err(error) = opened.verify(destination) {
            return Err(SkillMaterializationError::CommitIndeterminate {
                destination: destination.clone(),
                expected_digest: descriptor.content_digest().to_string(),
                reason: error.to_string(),
            });
        }
        Ok(SkillMaterializationStatus::Created)
    }

    pub(super) fn materialize_tree(
        workspace_root: &Path,
        destination: &SkillMaterializationDestination,
        prepared: &PreparedTemplateTree,
    ) -> Result<SkillMaterializationStatus, SkillMaterializationError> {
        let root = open_workspace_root(workspace_root, destination)?;
        let components = destination.components().collect::<Vec<_>>();
        let (directory_name, parents) = components
            .split_last()
            .expect("validated destination has at least one component");
        let target_name =
            CString::new(*directory_name).expect("validated component contains no NUL");
        let opened = open_parent_chain(root, parents, destination)?;
        opened.verify(destination)?;

        match inspect_tree_target(
            opened.parent(),
            &target_name,
            &prepared.fingerprint,
            destination,
        )? {
            TreeTargetState::Absent => {}
            TreeTargetState::Identical => return Ok(SkillMaterializationStatus::AlreadyPresent),
            TreeTargetState::Different(existing_digest) => {
                return Err(tree_conflict(
                    destination,
                    &prepared.plan_digest,
                    existing_digest,
                ))
            }
        }

        let mut staging = create_staging_directory(opened.parent(), destination)?;
        populate_staging_tree(staging.directory(), prepared, destination)?;
        if !tree_matches(staging.directory(), &prepared.fingerprint, destination)? {
            return Err(SkillMaterializationError::UnsafeDestination {
                destination: destination.clone(),
                reason: "staging tree changed before publication".to_string(),
            });
        }

        // Publication remains relative to the already-opened destination
        // parent. Revalidating the logical chain prevents a renamed/replaced
        // ancestor from silently changing the user's visible destination.
        #[cfg(test)]
        invoke_materialization_test_hook(
            MaterializationTestHookPoint::TreeBeforePublish,
            opened.parent().as_raw_fd(),
            staging.name(),
        );
        opened.verify(destination)?;
        staging.verify_link(destination)?;
        match rename_noreplace(opened.parent().as_raw_fd(), staging.name(), &target_name) {
            Ok(()) => {
                if let Err(reason) = staging.verify_published(opened.parent(), &target_name) {
                    return Err(SkillMaterializationError::CommitIndeterminate {
                        destination: destination.clone(),
                        expected_digest: prepared.plan_digest.clone(),
                        reason,
                    });
                }
                staging.disarm();
            }
            Err(error) if is_already_exists(&error) => {
                return match inspect_tree_target(
                    opened.parent(),
                    &target_name,
                    &prepared.fingerprint,
                    destination,
                )? {
                    TreeTargetState::Identical => Ok(SkillMaterializationStatus::AlreadyPresent),
                    TreeTargetState::Different(existing_digest) => Err(tree_conflict(
                        destination,
                        &prepared.plan_digest,
                        existing_digest,
                    )),
                    TreeTargetState::Absent => {
                        Err(tree_conflict(destination, &prepared.plan_digest, None))
                    }
                };
            }
            Err(error) if is_unsupported(&error) => {
                return Err(SkillMaterializationError::UnsupportedPlatform)
            }
            Err(error) => return Err(io_error("publish", destination, error)),
        }

        if let Err(error) = opened.parent().sync_all() {
            return Err(SkillMaterializationError::CommitIndeterminate {
                destination: destination.clone(),
                expected_digest: prepared.plan_digest.clone(),
                reason: error.to_string(),
            });
        }
        if let Err(error) = opened.verify(destination) {
            return Err(SkillMaterializationError::CommitIndeterminate {
                destination: destination.clone(),
                expected_digest: prepared.plan_digest.clone(),
                reason: error.to_string(),
            });
        }
        Ok(SkillMaterializationStatus::Created)
    }

    enum TreeTargetState {
        Absent,
        Identical,
        Different(Option<String>),
    }

    fn inspect_tree_target(
        parent: &File,
        name: &CStr,
        expected: &TreeFingerprint,
        destination: &SkillMaterializationDestination,
    ) -> Result<TreeTargetState, SkillMaterializationError> {
        let linked = match stat_at(parent.as_raw_fd(), name, libc::AT_SYMLINK_NOFOLLOW) {
            Ok(linked) => linked,
            Err(error) if error.raw_os_error() == Some(libc::ENOENT) => {
                return Ok(TreeTargetState::Absent)
            }
            Err(error) => return Err(io_error("inspect", destination, error)),
        };
        if is_symlink(&linked) {
            return Err(SkillMaterializationError::UnsafeDestination {
                destination: destination.clone(),
                reason: "destination is a symlink".to_string(),
            });
        }
        if !is_directory(&linked) {
            return Ok(TreeTargetState::Different(None));
        }

        let directory = open_directory_at(parent.as_raw_fd(), name, &linked, destination)?;
        // Two complete passes make an idempotent hit depend on a stable full
        // tree observation, rather than a single possibly-racing walk.
        let first = tree_matches(&directory, expected, destination)?;
        let second = first && tree_matches(&directory, expected, destination)?;
        let after =
            file_stat(&directory).map_err(|error| io_error("reinspect", destination, error))?;
        let relinked =
            stat_at(parent.as_raw_fd(), name, libc::AT_SYMLINK_NOFOLLOW).map_err(|error| {
                SkillMaterializationError::UnsafeDestination {
                    destination: destination.clone(),
                    reason: format!("destination changed while it was inspected: {error}"),
                }
            })?;
        if !same_inode(&linked, &after)
            || !same_inode(&linked, &relinked)
            || !is_directory(&after)
            || !is_directory(&relinked)
        {
            return Err(SkillMaterializationError::UnsafeDestination {
                destination: destination.clone(),
                reason: "destination changed while it was inspected".to_string(),
            });
        }
        if first && second {
            Ok(TreeTargetState::Identical)
        } else {
            Ok(TreeTargetState::Different(None))
        }
    }

    fn tree_matches(
        root: &File,
        expected: &TreeFingerprint,
        destination: &SkillMaterializationDestination,
    ) -> Result<bool, SkillMaterializationError> {
        let mut visited_directories = BTreeSet::new();
        let mut visited_files = BTreeSet::new();
        let mut remaining_entries = expected
            .directories
            .len()
            .saturating_add(expected.files.len())
            .saturating_add(1);
        if !directory_matches(
            root,
            "",
            expected,
            &mut visited_directories,
            &mut visited_files,
            &mut remaining_entries,
            destination,
        )? {
            return Ok(false);
        }
        Ok(visited_directories == expected.directories
            && visited_files.len() == expected.files.len()
            && visited_files
                .iter()
                .all(|path| expected.files.contains_key(path)))
    }

    fn directory_matches(
        directory: &File,
        relative_directory: &str,
        expected: &TreeFingerprint,
        visited_directories: &mut BTreeSet<String>,
        visited_files: &mut BTreeSet<String>,
        remaining_entries: &mut usize,
        destination: &SkillMaterializationDestination,
    ) -> Result<bool, SkillMaterializationError> {
        let Some(names) = read_directory_names(directory, *remaining_entries)
            .map_err(|error| io_error("enumerate existing", destination, error))?
        else {
            return Ok(false);
        };
        for name in names {
            if *remaining_entries == 0 {
                return Ok(false);
            }
            *remaining_entries -= 1;
            let name_text = match std::str::from_utf8(name.to_bytes()) {
                Ok(value) => value,
                Err(_) => return Ok(false),
            };
            let path = if relative_directory.is_empty() {
                name_text.to_string()
            } else {
                format!("{relative_directory}/{name_text}")
            };
            let linked = stat_at(directory.as_raw_fd(), &name, libc::AT_SYMLINK_NOFOLLOW).map_err(
                |error| SkillMaterializationError::UnsafeDestination {
                    destination: destination.clone(),
                    reason: format!("destination changed while it was inspected: {error}"),
                },
            )?;
            if is_symlink(&linked) {
                return Err(SkillMaterializationError::UnsafeDestination {
                    destination: destination.clone(),
                    reason: format!("destination contains a symlink at `{path}`"),
                });
            }
            if is_directory(&linked) {
                if !expected.directories.contains(&path)
                    || !visited_directories.insert(path.clone())
                {
                    return Ok(false);
                }
                let child = open_directory_at(directory.as_raw_fd(), &name, &linked, destination)?;
                if !directory_matches(
                    &child,
                    &path,
                    expected,
                    visited_directories,
                    visited_files,
                    remaining_entries,
                    destination,
                )? {
                    return Ok(false);
                }
            } else if is_regular_file(&linked) {
                let Some(expected_file) = expected.files.get(&path) else {
                    return Ok(false);
                };
                if !visited_files.insert(path.clone())
                    || !file_matches_at(
                        directory,
                        &name,
                        &linked,
                        expected_file,
                        &path,
                        destination,
                    )?
                {
                    return Ok(false);
                }
            } else {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn file_matches_at(
        parent: &File,
        name: &CStr,
        linked: &libc::stat,
        expected: &TreeFileFingerprint,
        relative_path: &str,
        destination: &SkillMaterializationDestination,
    ) -> Result<bool, SkillMaterializationError> {
        // SAFETY: the parent fd and name are live. `O_NONBLOCK` prevents a
        // raced special file from blocking the execution thread.
        let fd = unsafe {
            libc::openat(
                parent.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            let error = io::Error::last_os_error();
            return Err(SkillMaterializationError::UnsafeDestination {
                destination: destination.clone(),
                reason: format!(
                    "cannot safely inspect `{relative_path}` because the destination changed: {error}"
                ),
            });
        }
        // SAFETY: `fd` is newly owned by this function.
        let mut file = unsafe { File::from_raw_fd(fd) };
        let before = file_stat(&file).map_err(|error| io_error("inspect", destination, error))?;
        if !same_inode(linked, &before)
            || !is_regular_file(&before)
            || before.st_size < 0
            || u64::try_from(before.st_size).unwrap_or(u64::MAX) != expected.byte_length
        {
            return Ok(false);
        }
        let mut bytes = Vec::with_capacity(usize::try_from(before.st_size).unwrap_or(0));
        (&mut file)
            .take(expected.byte_length.saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(|error| io_error("read existing", destination, error))?;
        let after = file_stat(&file).map_err(|error| io_error("reinspect", destination, error))?;
        let relinked =
            stat_at(parent.as_raw_fd(), name, libc::AT_SYMLINK_NOFOLLOW).map_err(|error| {
                SkillMaterializationError::UnsafeDestination {
                    destination: destination.clone(),
                    reason: format!(
                        "destination changed while `{relative_path}` was read: {error}"
                    ),
                }
            })?;
        if !same_inode(&before, &after)
            || !same_inode(&before, &relinked)
            || before.st_size != after.st_size
            || !is_regular_file(&relinked)
        {
            return Err(SkillMaterializationError::UnsafeDestination {
                destination: destination.clone(),
                reason: format!("destination changed while `{relative_path}` was read"),
            });
        }
        Ok(
            u64::try_from(bytes.len()).unwrap_or(u64::MAX) == expected.byte_length
                && package_file_digest(&bytes) == expected.content_digest,
        )
    }

    fn open_directory_at(
        parent_fd: RawFd,
        name: &CStr,
        linked: &libc::stat,
        destination: &SkillMaterializationDestination,
    ) -> Result<File, SkillMaterializationError> {
        // SAFETY: the parent fd and C string are live. The successful fd is
        // immediately transferred to `File`.
        let fd = unsafe {
            libc::openat(
                parent_fd,
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            let error = io::Error::last_os_error();
            return Err(SkillMaterializationError::UnsafeDestination {
                destination: destination.clone(),
                reason: format!("cannot safely open destination directory: {error}"),
            });
        }
        // SAFETY: `fd` is newly owned by this function.
        let directory = unsafe { File::from_raw_fd(fd) };
        let opened =
            file_stat(&directory).map_err(|error| io_error("inspect", destination, error))?;
        if !is_directory(&opened) || !same_inode(linked, &opened) {
            return Err(SkillMaterializationError::UnsafeDestination {
                destination: destination.clone(),
                reason: "destination directory changed while it was opened".to_string(),
            });
        }
        Ok(directory)
    }

    /// Reads at most `limit` names. `None` means the directory contains more
    /// entries than the caller is willing to inspect; this prevents an
    /// attacker-controlled existing destination from forcing unbounded memory.
    fn read_directory_names(directory: &File, limit: usize) -> io::Result<Option<Vec<CString>>> {
        // `dup` would share a directory offset with the caller's open file
        // description. Open `.` relative to the handle instead so every walk
        // receives an independent cursor and repeated verification is stable.
        const DOT: &[u8] = b".\0";
        // SAFETY: the source fd and static NUL-terminated name are live.
        let stream_fd = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                DOT.as_ptr().cast(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if stream_fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `stream_fd` is newly owned and is consumed by fdopendir.
        let stream = unsafe { libc::fdopendir(stream_fd) };
        if stream.is_null() {
            let error = io::Error::last_os_error();
            // SAFETY: fdopendir failed and therefore did not consume the fd.
            unsafe { libc::close(stream_fd) };
            return Err(error);
        }

        let mut names = Vec::new();
        loop {
            set_errno(0);
            // SAFETY: the stream remains live until closed below.
            let entry = unsafe { libc::readdir(stream) };
            if entry.is_null() {
                let error = get_errno();
                // SAFETY: stream is live and closed exactly once.
                unsafe { libc::closedir(stream) };
                if error == 0 {
                    break;
                }
                return Err(io::Error::from_raw_os_error(error));
            }
            // SAFETY: readdir returned a live dirent with a NUL-terminated
            // d_name valid until the next call; copy it immediately.
            let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) };
            if name.to_bytes() != b"." && name.to_bytes() != b".." {
                if names.len() >= limit {
                    // SAFETY: stream is live and closed exactly once.
                    unsafe { libc::closedir(stream) };
                    return Ok(None);
                }
                names.push(name.to_owned());
            }
        }
        names.sort_by(|left, right| left.to_bytes().cmp(right.to_bytes()));
        Ok(Some(names))
    }

    #[cfg(target_vendor = "apple")]
    fn errno_location() -> *mut libc::c_int {
        // SAFETY: libc returns the current thread's errno pointer.
        unsafe { libc::__error() }
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    fn errno_location() -> *mut libc::c_int {
        // SAFETY: libc returns the current thread's errno pointer.
        unsafe { libc::__errno_location() }
    }

    fn set_errno(value: libc::c_int) {
        // SAFETY: errno_location returns the current thread's writable errno.
        unsafe { *errno_location() = value }
    }

    fn get_errno() -> libc::c_int {
        // SAFETY: errno_location returns the current thread's readable errno.
        unsafe { *errno_location() }
    }

    struct TreeStagingGuard {
        parent_fd: RawFd,
        name: CString,
        directory: File,
        device: libc::dev_t,
        inode: libc::ino_t,
        armed: bool,
    }

    impl TreeStagingGuard {
        fn name(&self) -> &CStr {
            &self.name
        }

        fn directory(&self) -> &File {
            &self.directory
        }

        fn verify_link(
            &self,
            destination: &SkillMaterializationDestination,
        ) -> Result<(), SkillMaterializationError> {
            let opened = file_stat(&self.directory)
                .map_err(|error| io_error("inspect staging for", destination, error))?;
            let linked = stat_at(self.parent_fd, &self.name, libc::AT_SYMLINK_NOFOLLOW).map_err(
                |error| SkillMaterializationError::UnsafeDestination {
                    destination: destination.clone(),
                    reason: format!("staging directory changed before publication: {error}"),
                },
            )?;
            if !self.matches(&opened) || !self.matches(&linked) {
                return Err(SkillMaterializationError::UnsafeDestination {
                    destination: destination.clone(),
                    reason: "staging directory was replaced before publication".to_string(),
                });
            }
            Ok(())
        }

        fn verify_published(&self, parent: &File, target: &CStr) -> Result<(), String> {
            let opened = file_stat(&self.directory)
                .map_err(|error| format!("cannot inspect published staging directory: {error}"))?;
            let linked = stat_at(parent.as_raw_fd(), target, libc::AT_SYMLINK_NOFOLLOW)
                .map_err(|error| format!("cannot inspect published destination: {error}"))?;
            if self.matches(&opened) && self.matches(&linked) {
                Ok(())
            } else {
                Err(
                    "published destination does not match the verified staging directory"
                        .to_string(),
                )
            }
        }

        fn matches(&self, stat: &libc::stat) -> bool {
            is_directory(stat) && stat.st_dev == self.device && stat.st_ino == self.inode
        }

        fn disarm(&mut self) {
            self.armed = false;
        }
    }

    impl Drop for TreeStagingGuard {
        fn drop(&mut self) {
            if self.armed {
                remove_owned_staging_tree(
                    self.parent_fd,
                    &self.name,
                    &self.directory,
                    self.device,
                    self.inode,
                );
            }
        }
    }

    fn create_staging_directory(
        parent: &File,
        destination: &SkillMaterializationDestination,
    ) -> Result<TreeStagingGuard, SkillMaterializationError> {
        for _ in 0..MAX_STAGING_NAME_ATTEMPTS {
            let name = CString::new(format!(
                "{STAGING_NAME_PREFIX}{}",
                Uuid::new_v4().hyphenated()
            ))
            .expect("generated staging name contains no NUL");
            // SAFETY: parent and name are live.
            if unsafe { libc::mkdirat(parent.as_raw_fd(), name.as_ptr(), 0o700) } != 0 {
                let error = io::Error::last_os_error();
                if error.raw_os_error() == Some(libc::EEXIST) {
                    continue;
                }
                return Err(io_error("create staging directory for", destination, error));
            }

            // SAFETY: parent and name are live; fd is owned below on success.
            let fd = unsafe {
                libc::openat(
                    parent.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                )
            };
            if fd < 0 {
                let error = io::Error::last_os_error();
                // We do not yet own an inode identity that can make path-based
                // cleanup safe. A hostile same-UID actor may have rebound the
                // random name, so prefer a harmless orphan over deleting an
                // object we cannot prove we created.
                return Err(io_error("open staging directory for", destination, error));
            }
            // SAFETY: fd is newly owned.
            let directory = unsafe { File::from_raw_fd(fd) };
            let opened = file_stat(&directory)
                .map_err(|error| io_error("inspect staging for", destination, error))?;
            if !is_directory(&opened) {
                return Err(SkillMaterializationError::UnsafeDestination {
                    destination: destination.clone(),
                    reason: "staging directory is not a directory".to_string(),
                });
            }
            let guard = TreeStagingGuard {
                parent_fd: parent.as_raw_fd(),
                name,
                directory,
                device: opened.st_dev,
                inode: opened.st_ino,
                armed: true,
            };
            if unsafe { libc::fchmod(guard.directory.as_raw_fd(), 0o700) } != 0 {
                let error = io::Error::last_os_error();
                return Err(io_error("set staging permissions for", destination, error));
            }
            guard.verify_link(destination)?;
            return Ok(guard);
        }
        Err(SkillMaterializationError::Io {
            operation: "allocate staging name for",
            destination: destination.clone(),
            reason: "staging namespace is unexpectedly saturated".to_string(),
        })
    }

    fn populate_staging_tree(
        root: &File,
        prepared: &PreparedTemplateTree,
        destination: &SkillMaterializationDestination,
    ) -> Result<(), SkillMaterializationError> {
        let mut directories = BTreeMap::<String, File>::new();
        for relative_path in &prepared.fingerprint.directories {
            let (parent_path, name_text) = split_parent(relative_path);
            let parent = relative_directory(root, &directories, parent_path);
            let name = CString::new(name_text).expect("validated component contains no NUL");
            // SAFETY: parent and name are live.
            if unsafe { libc::mkdirat(parent.as_raw_fd(), name.as_ptr(), 0o700) } != 0 {
                return Err(io_error(
                    "create staging subdirectory for",
                    destination,
                    io::Error::last_os_error(),
                ));
            }
            let linked = stat_at(parent.as_raw_fd(), &name, libc::AT_SYMLINK_NOFOLLOW)
                .map_err(|error| io_error("inspect staging for", destination, error))?;
            let child = open_directory_at(parent.as_raw_fd(), &name, &linked, destination)?;
            // SAFETY: child fd is live.
            if unsafe { libc::fchmod(child.as_raw_fd(), 0o700) } != 0 {
                return Err(io_error(
                    "set staging directory permissions for",
                    destination,
                    io::Error::last_os_error(),
                ));
            }
            directories.insert(relative_path.clone(), child);
        }

        for prepared_file in &prepared.files {
            let (parent_path, name_text) = split_parent(&prepared_file.relative_path);
            let parent = relative_directory(root, &directories, parent_path);
            let name = CString::new(name_text).expect("validated component contains no NUL");
            // SAFETY: parent and name are live. The successful fd is owned by
            // File immediately.
            let fd = unsafe {
                libc::openat(
                    parent.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_WRONLY
                        | libc::O_CREAT
                        | libc::O_EXCL
                        | libc::O_NOFOLLOW
                        | libc::O_CLOEXEC,
                    0o600,
                )
            };
            if fd < 0 {
                return Err(io_error(
                    "create staging file for",
                    destination,
                    io::Error::last_os_error(),
                ));
            }
            // SAFETY: fd is newly owned.
            let mut file = unsafe { File::from_raw_fd(fd) };
            // SAFETY: file fd is live.
            if unsafe { libc::fchmod(file.as_raw_fd(), 0o600) } != 0 {
                return Err(io_error(
                    "set staging file permissions for",
                    destination,
                    io::Error::last_os_error(),
                ));
            }
            file.write_all(&prepared_file.bytes)
                .map_err(|error| io_error("write staging file for", destination, error))?;
            file.sync_all()
                .map_err(|error| io_error("synchronize staging file for", destination, error))?;
            let opened = file_stat(&file)
                .map_err(|error| io_error("inspect staging file for", destination, error))?;
            let linked = stat_at(parent.as_raw_fd(), &name, libc::AT_SYMLINK_NOFOLLOW)
                .map_err(|error| io_error("inspect staging file for", destination, error))?;
            if !is_regular_file(&opened)
                || !is_regular_file(&linked)
                || !same_inode(&opened, &linked)
                || opened.st_size < 0
                || u64::try_from(opened.st_size).unwrap_or(u64::MAX)
                    != prepared_file.descriptor.byte_length()
            {
                return Err(SkillMaterializationError::UnsafeDestination {
                    destination: destination.clone(),
                    reason: format!(
                        "staging file `{}` changed while it was written",
                        prepared_file.relative_path
                    ),
                });
            }
        }

        for directory in directories.values().rev() {
            directory.sync_all().map_err(|error| {
                io_error("synchronize staging directory for", destination, error)
            })?;
        }
        root.sync_all()
            .map_err(|error| io_error("synchronize staging tree for", destination, error))
    }

    fn split_parent(path: &str) -> (&str, &str) {
        path.rsplit_once('/').unwrap_or(("", path))
    }

    fn relative_directory<'a>(
        root: &'a File,
        directories: &'a BTreeMap<String, File>,
        path: &str,
    ) -> &'a File {
        if path.is_empty() {
            root
        } else {
            directories
                .get(path)
                .expect("prepared directory set contains every parent")
        }
    }

    fn remove_owned_staging_tree(
        parent_fd: RawFd,
        name: &CStr,
        directory: &File,
        device: libc::dev_t,
        inode: libc::ino_t,
    ) {
        let expected_matches =
            |stat: &libc::stat| is_directory(stat) && stat.st_dev == device && stat.st_ino == inode;
        let Ok(opened) = file_stat(directory) else {
            return;
        };
        let Ok(linked) = stat_at(parent_fd, name, libc::AT_SYMLINK_NOFOLLOW) else {
            return;
        };
        if !expected_matches(&opened) || !expected_matches(&linked) {
            return;
        }
        let mut budget = MAX_SKILL_MATERIALIZATION_TREE_FILES
            .saturating_add(MAX_SKILL_MATERIALIZATION_TREE_DIRECTORIES)
            .saturating_add(1);
        if clear_owned_directory(directory, &mut budget).is_err() {
            return;
        }
        let Ok(relinked) = stat_at(parent_fd, name, libc::AT_SYMLINK_NOFOLLOW) else {
            return;
        };
        if expected_matches(&relinked) {
            // SAFETY: the name still resolves to the exact staging inode we
            // created and just emptied. No broader path deletion is possible.
            unsafe { libc::unlinkat(parent_fd, name.as_ptr(), libc::AT_REMOVEDIR) };
        }
    }

    fn clear_owned_directory(directory: &File, budget: &mut usize) -> io::Result<()> {
        let names = read_directory_names(directory, *budget)?
            .ok_or_else(|| io::Error::other("staging cleanup budget exhausted"))?;
        for name in names {
            if *budget == 0 {
                return Err(io::Error::other("staging cleanup budget exhausted"));
            }
            *budget -= 1;
            let linked = stat_at(directory.as_raw_fd(), &name, libc::AT_SYMLINK_NOFOLLOW)?;
            if is_directory(&linked) {
                let child = open_directory_at_io(directory.as_raw_fd(), &name, &linked)?;
                clear_owned_directory(&child, budget)?;
                let relinked = stat_at(directory.as_raw_fd(), &name, libc::AT_SYMLINK_NOFOLLOW)?;
                if !same_inode(&linked, &relinked) || !is_directory(&relinked) {
                    return Err(io::Error::other("staging directory changed during cleanup"));
                }
                // SAFETY: name still points to the exact child inode opened
                // and recursively emptied above.
                if unsafe {
                    libc::unlinkat(directory.as_raw_fd(), name.as_ptr(), libc::AT_REMOVEDIR)
                } != 0
                {
                    return Err(io::Error::last_os_error());
                }
            } else if unsafe { libc::unlinkat(directory.as_raw_fd(), name.as_ptr(), 0) } != 0 {
                // Non-directories, including symlinks, are unlinked as entries
                // and are never followed.
                return Err(io::Error::last_os_error());
            }
        }
        Ok(())
    }

    fn open_directory_at_io(
        parent_fd: RawFd,
        name: &CStr,
        linked: &libc::stat,
    ) -> io::Result<File> {
        // SAFETY: parent/name are live; fd is transferred to File on success.
        let fd = unsafe {
            libc::openat(
                parent_fd,
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: fd is newly owned.
        let directory = unsafe { File::from_raw_fd(fd) };
        let opened = file_stat(&directory)?;
        if !is_directory(&opened) || !same_inode(linked, &opened) {
            return Err(io::Error::other(
                "staging directory changed while it was opened",
            ));
        }
        Ok(directory)
    }

    struct OpenedWorkspaceRoot {
        file: File,
        lexical_path: PathBuf,
        device: libc::dev_t,
        inode: libc::ino_t,
    }

    fn open_workspace_root(
        workspace_root: &Path,
        destination: &SkillMaterializationDestination,
    ) -> Result<OpenedWorkspaceRoot, SkillMaterializationError> {
        let root = CString::new(workspace_root.as_os_str().as_bytes()).map_err(|_| {
            SkillMaterializationError::InvalidWorkspace {
                reason: "workspace root contains a NUL byte".to_string(),
            }
        })?;
        // SAFETY: `root` is a live NUL-terminated C string. The returned fd is
        // owned immediately by `File` on success.
        let fd = unsafe {
            libc::open(
                root.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            let error = io::Error::last_os_error();
            return Err(if matches_errno(&error, &[libc::ELOOP, libc::ENOTDIR]) {
                SkillMaterializationError::InvalidWorkspace {
                    reason: "workspace root is not a plain directory".to_string(),
                }
            } else {
                io_error("open workspace for", destination, error)
            });
        }
        // SAFETY: `fd` is newly owned by this function.
        let file = unsafe { File::from_raw_fd(fd) };
        let identity =
            file_stat(&file).map_err(|error| SkillMaterializationError::InvalidWorkspace {
                reason: format!("workspace root cannot be inspected: {error}"),
            })?;
        if !is_directory(&identity) {
            return Err(SkillMaterializationError::InvalidWorkspace {
                reason: "workspace root is not a plain directory".to_string(),
            });
        }
        Ok(OpenedWorkspaceRoot {
            file,
            lexical_path: workspace_root.to_path_buf(),
            device: identity.st_dev,
            inode: identity.st_ino,
        })
    }

    struct OpenedParentChain {
        directories: Vec<File>,
        names: Vec<CString>,
        root_path: PathBuf,
        root_device: libc::dev_t,
        root_inode: libc::ino_t,
    }

    impl OpenedParentChain {
        fn parent(&self) -> &File {
            self.directories
                .last()
                .expect("opened chain contains the workspace root")
        }

        fn verify(
            &self,
            destination: &SkillMaterializationDestination,
        ) -> Result<(), SkillMaterializationError> {
            let root = self
                .directories
                .first()
                .expect("opened chain contains the workspace root");
            let opened_root = file_stat(root).map_err(|error| {
                unsafe_parent(
                    destination,
                    format!("cannot inspect opened workspace root: {error}"),
                )
            })?;
            let root_path = CString::new(self.root_path.as_os_str().as_bytes()).map_err(|_| {
                unsafe_parent(
                    destination,
                    "workspace root contains a NUL byte".to_string(),
                )
            })?;
            // SAFETY: the path is a live C string and the fd is immediately
            // transferred to File on success.
            let rebound_fd = unsafe {
                libc::open(
                    root_path.as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                )
            };
            if rebound_fd < 0 {
                return Err(unsafe_parent(
                    destination,
                    format!(
                        "workspace root binding changed: {}",
                        io::Error::last_os_error()
                    ),
                ));
            }
            // SAFETY: `rebound_fd` is newly owned.
            let rebound = unsafe { File::from_raw_fd(rebound_fd) };
            let rebound_root = file_stat(&rebound).map_err(|error| {
                unsafe_parent(
                    destination,
                    format!("cannot inspect rebound workspace root: {error}"),
                )
            })?;
            let expected_root = |stat: &libc::stat| {
                is_directory(stat)
                    && stat.st_dev == self.root_device
                    && stat.st_ino == self.root_inode
            };
            if !expected_root(&opened_root) || !expected_root(&rebound_root) {
                return Err(unsafe_parent(
                    destination,
                    "workspace root path was renamed or rebound during materialization".to_string(),
                ));
            }
            for (index, name) in self.names.iter().enumerate() {
                let expected = file_stat(&self.directories[index]).map_err(|error| {
                    unsafe_parent(
                        destination,
                        format!("cannot inspect opened parent: {error}"),
                    )
                })?;
                let child = file_stat(&self.directories[index + 1]).map_err(|error| {
                    unsafe_parent(destination, format!("cannot inspect opened child: {error}"))
                })?;
                let linked = stat_at(
                    self.directories[index].as_raw_fd(),
                    name,
                    libc::AT_SYMLINK_NOFOLLOW,
                )
                .map_err(|error| {
                    unsafe_parent(destination, format!("parent chain changed: {error}"))
                })?;
                if !is_directory(&expected)
                    || !is_directory(&child)
                    || !is_directory(&linked)
                    || child.st_dev != linked.st_dev
                    || child.st_ino != linked.st_ino
                {
                    return Err(unsafe_parent(
                        destination,
                        "parent chain changed or contains a link".to_string(),
                    ));
                }
            }
            Ok(())
        }
    }

    fn open_parent_chain(
        root: OpenedWorkspaceRoot,
        parents: &[&str],
        destination: &SkillMaterializationDestination,
    ) -> Result<OpenedParentChain, SkillMaterializationError> {
        let OpenedWorkspaceRoot {
            file,
            lexical_path,
            device,
            inode,
        } = root;
        let mut directories = vec![file];
        let mut names = Vec::with_capacity(parents.len());
        for component in parents {
            let name = CString::new(*component).expect("validated component contains no NUL");
            let parent = directories.last().unwrap();
            // SAFETY: the parent fd and C string are live. A successful fd is
            // immediately transferred into `File`.
            let fd = unsafe {
                libc::openat(
                    parent.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                )
            };
            if fd < 0 {
                let error = io::Error::last_os_error();
                return Err(if error.raw_os_error() == Some(libc::ENOENT) {
                    SkillMaterializationError::DestinationParentNotFound {
                        destination: destination.clone(),
                    }
                } else if matches_errno(&error, &[libc::ELOOP, libc::ENOTDIR]) {
                    unsafe_parent(
                        destination,
                        "parent is a symlink or is not a directory".to_string(),
                    )
                } else {
                    io_error("open parent for", destination, error)
                });
            }
            names.push(name);
            // SAFETY: `fd` is newly owned by this function.
            directories.push(unsafe { File::from_raw_fd(fd) });
        }
        Ok(OpenedParentChain {
            directories,
            names,
            root_path: lexical_path,
            root_device: device,
            root_inode: inode,
        })
    }

    enum TargetState {
        Absent,
        Identical,
        Different(Option<String>),
    }

    fn inspect_target(
        parent: &File,
        name: &CStr,
        descriptor: &SkillResourceDescriptor,
        destination: &SkillMaterializationDestination,
    ) -> Result<TargetState, SkillMaterializationError> {
        // `O_NONBLOCK` prevents an attacker-controlled FIFO from blocking the
        // execution thread. `O_NOFOLLOW` ensures a final symlink is never read.
        // SAFETY: the parent fd and name are live for the call.
        let fd = unsafe {
            libc::openat(
                parent.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            let error = io::Error::last_os_error();
            return if error.raw_os_error() == Some(libc::ENOENT) {
                Ok(TargetState::Absent)
            } else if matches_errno(&error, &[libc::ELOOP, libc::ENOTDIR]) {
                Err(SkillMaterializationError::UnsafeDestination {
                    destination: destination.clone(),
                    reason: "destination is a symlink or traverses a non-directory".to_string(),
                })
            } else {
                Err(io_error("inspect", destination, error))
            };
        }
        // SAFETY: `fd` is newly owned by this function.
        let mut file = unsafe { File::from_raw_fd(fd) };
        let before = file_stat(&file).map_err(|error| io_error("inspect", destination, error))?;
        if !is_regular_file(&before) {
            return Ok(TargetState::Different(None));
        }
        if before.st_size < 0
            || u64::try_from(before.st_size).unwrap_or(u64::MAX) != descriptor.byte_length()
        {
            return Ok(TargetState::Different(None));
        }

        let mut existing = Vec::with_capacity(usize::try_from(before.st_size).unwrap_or(0));
        (&mut file)
            .take(u64::try_from(MAX_SKILL_MATERIALIZATION_FILE_BYTES).unwrap_or(u64::MAX) + 1)
            .read_to_end(&mut existing)
            .map_err(|error| io_error("read existing", destination, error))?;
        let after = file_stat(&file).map_err(|error| io_error("reinspect", destination, error))?;
        let linked =
            stat_at(parent.as_raw_fd(), name, libc::AT_SYMLINK_NOFOLLOW).map_err(|error| {
                SkillMaterializationError::UnsafeDestination {
                    destination: destination.clone(),
                    reason: format!("destination changed while it was inspected: {error}"),
                }
            })?;
        if before.st_dev != after.st_dev
            || before.st_ino != after.st_ino
            || before.st_size != after.st_size
            || before.st_dev != linked.st_dev
            || before.st_ino != linked.st_ino
            || !is_regular_file(&linked)
        {
            return Err(SkillMaterializationError::UnsafeDestination {
                destination: destination.clone(),
                reason: "destination changed while it was inspected".to_string(),
            });
        }
        let digest = package_file_digest(&existing);
        if digest == descriptor.content_digest() {
            Ok(TargetState::Identical)
        } else {
            Ok(TargetState::Different(Some(digest)))
        }
    }

    struct StagingGuard {
        parent_fd: RawFd,
        name: CString,
        file: File,
        device: libc::dev_t,
        inode: libc::ino_t,
        armed: bool,
    }

    impl StagingGuard {
        fn name(&self) -> &CStr {
            &self.name
        }

        fn file(&self) -> &File {
            &self.file
        }

        fn file_mut(&mut self) -> &mut File {
            &mut self.file
        }

        fn matches(&self, stat: &libc::stat) -> bool {
            is_regular_file(stat) && stat.st_dev == self.device && stat.st_ino == self.inode
        }

        fn verify_link(
            &self,
            destination: &SkillMaterializationDestination,
        ) -> Result<(), SkillMaterializationError> {
            let opened = file_stat(&self.file)
                .map_err(|error| io_error("inspect staging for", destination, error))?;
            let linked = stat_at(self.parent_fd, &self.name, libc::AT_SYMLINK_NOFOLLOW).map_err(
                |error| SkillMaterializationError::UnsafeDestination {
                    destination: destination.clone(),
                    reason: format!("staging file changed before publication: {error}"),
                },
            )?;
            if !self.matches(&opened) || !self.matches(&linked) {
                return Err(SkillMaterializationError::UnsafeDestination {
                    destination: destination.clone(),
                    reason: "staging file was replaced before publication".to_string(),
                });
            }
            Ok(())
        }

        fn verify_published(&self, parent: &File, target: &CStr) -> Result<(), String> {
            let opened = file_stat(&self.file)
                .map_err(|error| format!("cannot inspect published staging file: {error}"))?;
            let linked = stat_at(parent.as_raw_fd(), target, libc::AT_SYMLINK_NOFOLLOW)
                .map_err(|error| format!("cannot inspect published destination: {error}"))?;
            if self.matches(&opened) && self.matches(&linked) {
                Ok(())
            } else {
                Err("published destination does not match the verified staging file".to_string())
            }
        }

        fn disarm(&mut self) {
            self.armed = false;
        }
    }

    impl Drop for StagingGuard {
        fn drop(&mut self) {
            if self.armed {
                let opened = file_stat(&self.file);
                let linked = stat_at(self.parent_fd, &self.name, libc::AT_SYMLINK_NOFOLLOW);
                if opened.as_ref().is_ok_and(|stat| self.matches(stat))
                    && linked.as_ref().is_ok_and(|stat| self.matches(stat))
                {
                    // SAFETY: the name still resolves to the exact regular file
                    // inode held by this guard. Cleanup never follows a link or
                    // deletes a concurrent replacement.
                    unsafe {
                        libc::unlinkat(self.parent_fd, self.name.as_ptr(), 0);
                    }
                }
            }
        }
    }

    fn create_staging(
        parent: &File,
        destination: &SkillMaterializationDestination,
    ) -> Result<StagingGuard, SkillMaterializationError> {
        for _ in 0..MAX_STAGING_NAME_ATTEMPTS {
            let name = CString::new(format!(
                "{STAGING_NAME_PREFIX}{}",
                Uuid::new_v4().hyphenated()
            ))
            .expect("generated staging name contains no NUL");
            // SAFETY: the parent fd and C string are live. A successful fd is
            // immediately transferred into `File`.
            let fd = unsafe {
                libc::openat(
                    parent.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_WRONLY
                        | libc::O_CREAT
                        | libc::O_EXCL
                        | libc::O_NOFOLLOW
                        | libc::O_CLOEXEC,
                    0o600,
                )
            };
            if fd >= 0 {
                // SAFETY: `fd` is newly owned by this function.
                let file = unsafe { File::from_raw_fd(fd) };
                let opened = file_stat(&file)
                    .map_err(|error| io_error("inspect staging for", destination, error))?;
                if !is_regular_file(&opened) {
                    return Err(SkillMaterializationError::UnsafeDestination {
                        destination: destination.clone(),
                        reason: "staging file is not a regular file".to_string(),
                    });
                }
                let guard = StagingGuard {
                    parent_fd: parent.as_raw_fd(),
                    name,
                    file,
                    device: opened.st_dev,
                    inode: opened.st_ino,
                    armed: true,
                };
                // Make the privacy contract independent of the process umask.
                // SAFETY: the guard owns a live file descriptor.
                if unsafe { libc::fchmod(guard.file.as_raw_fd(), 0o600) } != 0 {
                    let error = io::Error::last_os_error();
                    return Err(io_error("set staging permissions for", destination, error));
                }
                guard.verify_link(destination)?;
                return Ok(guard);
            }
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::EEXIST) {
                return Err(io_error("create staging file for", destination, error));
            }
        }
        Err(SkillMaterializationError::Io {
            operation: "allocate staging name for",
            destination: destination.clone(),
            reason: "staging namespace is unexpectedly saturated".to_string(),
        })
    }

    fn file_stat(file: &File) -> io::Result<libc::stat> {
        let mut stat = MaybeUninit::<libc::stat>::uninit();
        // SAFETY: `stat` points to writable memory and the file fd is live.
        if unsafe { libc::fstat(file.as_raw_fd(), stat.as_mut_ptr()) } == 0 {
            // SAFETY: successful fstat initialized the structure.
            Ok(unsafe { stat.assume_init() })
        } else {
            Err(io::Error::last_os_error())
        }
    }

    fn stat_at(parent_fd: RawFd, name: &CStr, flags: i32) -> io::Result<libc::stat> {
        let mut stat = MaybeUninit::<libc::stat>::uninit();
        // SAFETY: all pointers and fds are live for the call.
        if unsafe { libc::fstatat(parent_fd, name.as_ptr(), stat.as_mut_ptr(), flags) } == 0 {
            // SAFETY: successful fstatat initialized the structure.
            Ok(unsafe { stat.assume_init() })
        } else {
            Err(io::Error::last_os_error())
        }
    }

    fn is_directory(stat: &libc::stat) -> bool {
        stat.st_mode & libc::S_IFMT == libc::S_IFDIR
    }

    fn is_regular_file(stat: &libc::stat) -> bool {
        stat.st_mode & libc::S_IFMT == libc::S_IFREG
    }

    fn is_symlink(stat: &libc::stat) -> bool {
        stat.st_mode & libc::S_IFMT == libc::S_IFLNK
    }

    fn same_inode(left: &libc::stat, right: &libc::stat) -> bool {
        left.st_dev == right.st_dev && left.st_ino == right.st_ino
    }

    fn conflict(
        destination: &SkillMaterializationDestination,
        descriptor: &SkillResourceDescriptor,
        existing_digest: Option<String>,
    ) -> SkillMaterializationError {
        SkillMaterializationError::Conflict {
            destination: destination.clone(),
            expected_digest: descriptor.content_digest().to_string(),
            existing_digest,
        }
    }

    fn tree_conflict(
        destination: &SkillMaterializationDestination,
        expected_digest: &str,
        existing_digest: Option<String>,
    ) -> SkillMaterializationError {
        SkillMaterializationError::Conflict {
            destination: destination.clone(),
            expected_digest: expected_digest.to_string(),
            existing_digest,
        }
    }

    fn unsafe_parent(
        destination: &SkillMaterializationDestination,
        reason: String,
    ) -> SkillMaterializationError {
        SkillMaterializationError::UnsafeParent {
            destination: destination.clone(),
            reason,
        }
    }

    fn io_error(
        operation: &'static str,
        destination: &SkillMaterializationDestination,
        error: io::Error,
    ) -> SkillMaterializationError {
        SkillMaterializationError::Io {
            operation,
            destination: destination.clone(),
            reason: error.to_string(),
        }
    }

    fn matches_errno(error: &io::Error, values: &[i32]) -> bool {
        error
            .raw_os_error()
            .is_some_and(|actual| values.contains(&actual))
    }

    fn is_already_exists(error: &io::Error) -> bool {
        matches_errno(error, &[libc::EEXIST, libc::ENOTEMPTY])
    }

    fn is_unsupported(error: &io::Error) -> bool {
        matches_errno(error, &[libc::ENOSYS, libc::EINVAL, libc::ENOTSUP])
    }

    #[cfg(target_vendor = "apple")]
    fn rename_noreplace(parent_fd: RawFd, source: &CStr, target: &CStr) -> io::Result<()> {
        // SAFETY: both names and the parent fd are live for the call.
        let result = unsafe {
            libc::renameatx_np(
                parent_fd,
                source.as_ptr(),
                parent_fd,
                target.as_ptr(),
                libc::RENAME_EXCL,
            )
        };
        if result == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    fn rename_noreplace(parent_fd: RawFd, source: &CStr, target: &CStr) -> io::Result<()> {
        // SAFETY: both names and the parent fd are live for the syscall.
        let result = unsafe {
            libc::syscall(
                libc::SYS_renameat2,
                parent_fd,
                source.as_ptr(),
                parent_fd,
                target.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        };
        if result == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }
}

#[cfg(all(
    test,
    any(target_vendor = "apple", target_os = "linux", target_os = "android")
))]
mod tests {
    use super::*;
    use crate::skills::digest::package_file_digest;
    use crate::skills::model::{SkillId, SkillResourceIndex, SkillRevision, SkillSourceId};
    use crate::skills::resource_runtime::{
        SkillResourceReader, SkillResourceReaderRef, SkillResourceSessionBinding,
        SkillResourceSourceError,
    };
    use std::collections::BTreeMap;
    #[cfg(unix)]
    use std::ffi::CString;
    use std::fs;
    #[cfg(unix)]
    use std::io::Write as _;
    #[cfg(unix)]
    use std::os::fd::FromRawFd;
    use std::sync::{Arc, Barrier, Mutex};
    use tempfile::tempdir;

    struct MemoryReader {
        bytes: BTreeMap<String, Vec<u8>>,
    }

    impl SkillResourceReader for MemoryReader {
        fn read(
            &self,
            expected: &SkillResourceDescriptor,
        ) -> Result<Vec<u8>, SkillResourceSourceError> {
            self.bytes
                .get(expected.path())
                .cloned()
                .ok_or_else(|| SkillResourceSourceError::Unavailable("fixture missing".to_string()))
        }
    }

    fn test_session(
        path: &str,
        kind: SkillResourceKind,
        descriptor_bytes: &[u8],
        reader_bytes: &[u8],
    ) -> (SkillResourceSession, SkillResourceUri) {
        let source_id = SkillSourceId::parse("installed:user").unwrap();
        let skill_id =
            SkillId::parse("installed:user:01234567-89ab-4def-8123-456789abcdef").unwrap();
        let revision =
            SkillRevision::parse(format!("skill-package-sha256-v3:{}", "a".repeat(64))).unwrap();
        let descriptor = SkillResourceDescriptor::new(
            path.to_string(),
            kind,
            descriptor_bytes.len() as u64,
            package_file_digest(descriptor_bytes),
        );
        let reader: SkillResourceReaderRef = Arc::new(MemoryReader {
            bytes: BTreeMap::from([(path.to_string(), reader_bytes.to_vec())]),
        });
        let resource_session = SkillResourceSession::from_bindings([SkillResourceSessionBinding {
            skill_id: skill_id.clone(),
            revision: revision.clone(),
            source_id,
            resources: SkillResourceIndex::new(vec![descriptor]),
            reader: Some(reader),
        }])
        .unwrap();
        let uri = resource_session.package_uris()[0]
            .resource(super::super::resource_runtime::SkillResourcePath::parse(path).unwrap());
        (resource_session, uri)
    }

    fn test_tree_session(
        entries: &[(&str, SkillResourceKind, &[u8], &[u8])],
    ) -> (SkillResourceSession, SkillPackageUri) {
        let source_id = SkillSourceId::parse("installed:user").unwrap();
        let skill_id =
            SkillId::parse("installed:user:11234567-89ab-4def-8123-456789abcdef").unwrap();
        let revision =
            SkillRevision::parse(format!("skill-package-sha256-v3:{}", "b".repeat(64))).unwrap();
        let descriptors = entries
            .iter()
            .map(|(path, kind, descriptor_bytes, _)| {
                SkillResourceDescriptor::new(
                    (*path).to_string(),
                    *kind,
                    descriptor_bytes.len() as u64,
                    package_file_digest(descriptor_bytes),
                )
            })
            .collect::<Vec<_>>();
        let reader: SkillResourceReaderRef = Arc::new(MemoryReader {
            bytes: entries
                .iter()
                .map(|(path, _, _, reader_bytes)| ((*path).to_string(), (*reader_bytes).to_vec()))
                .collect(),
        });
        let session = SkillResourceSession::from_bindings([SkillResourceSessionBinding {
            skill_id,
            revision,
            source_id,
            resources: SkillResourceIndex::new(descriptors),
            reader: Some(reader),
        }])
        .unwrap();
        let package = session.package_uris()[0].clone();
        (session, package)
    }

    fn test_request(
        uri: SkillResourceUri,
        root: &Path,
        destination: &str,
    ) -> SkillMaterializationRequest {
        SkillMaterializationRequest::new(
            uri,
            root,
            SkillMaterializationDestination::parse(destination).unwrap(),
        )
        .unwrap()
    }

    fn test_tree_request(
        package: SkillPackageUri,
        prefix: &str,
        root: &Path,
        destination: &str,
    ) -> SkillTemplateTreeMaterializationRequest {
        SkillTemplateTreeMaterializationRequest::new(
            package,
            SkillResourcePath::parse(prefix).unwrap(),
            root,
            SkillMaterializationDestination::parse(destination).unwrap(),
        )
        .unwrap()
    }

    fn staging_entries(root: &Path) -> Vec<String> {
        fs::read_dir(root)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with(STAGING_NAME_PREFIX))
            .collect()
    }

    #[test]
    fn creates_one_asset_atomically_and_same_content_is_idempotent() {
        let workspace = tempdir().unwrap();
        let bytes = b"asset bytes\n";
        let (session, uri) =
            test_session("assets/report.txt", SkillResourceKind::Asset, bytes, bytes);
        let request = test_request(uri, workspace.path(), "outputs/report.txt");
        fs::create_dir(workspace.path().join("outputs")).unwrap();
        let materializer = SkillResourceMaterializer::new();

        let created = materializer.materialize(&session, &request).unwrap();
        assert_eq!(created.status(), SkillMaterializationStatus::Created);
        assert_eq!(created.destination().as_str(), "outputs/report.txt");
        assert_eq!(created.byte_length(), bytes.len() as u64);
        assert_eq!(created.bytes_written(), bytes.len() as u64);
        assert_eq!(
            fs::read(workspace.path().join("outputs/report.txt")).unwrap(),
            bytes
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(workspace.path().join("outputs/report.txt"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        assert!(staging_entries(&workspace.path().join("outputs")).is_empty());

        let repeated = materializer.materialize(&session, &request).unwrap();
        assert_eq!(
            repeated.status(),
            SkillMaterializationStatus::AlreadyPresent
        );
        assert_eq!(repeated.bytes_written(), 0);
        assert_eq!(
            fs::read(workspace.path().join("outputs/report.txt")).unwrap(),
            bytes
        );
    }

    #[test]
    fn templates_other_is_allowed_but_reference_and_script_are_denied() {
        let workspace = tempdir().unwrap();
        let (template_session, template_uri) = test_session(
            "templates/budget.xlsx",
            SkillResourceKind::Other,
            b"template",
            b"template",
        );
        let template_request = test_request(template_uri, workspace.path(), "budget.xlsx");
        assert_eq!(
            SkillResourceMaterializer::new()
                .materialize(&template_session, &template_request)
                .unwrap()
                .status(),
            SkillMaterializationStatus::Created
        );

        for (path, kind) in [
            ("references/guide.md", SkillResourceKind::Reference),
            ("scripts/run.sh", SkillResourceKind::Script),
        ] {
            let (denied_session, denied_uri) = test_session(path, kind, b"denied", b"denied");
            let denied_request = test_request(denied_uri, workspace.path(), "denied.txt");
            let error = SkillResourceMaterializer::new()
                .materialize(&denied_session, &denied_request)
                .unwrap_err();
            assert_eq!(
                error.code(),
                SkillMaterializationErrorCode::SourceKindDenied
            );
            assert!(!workspace.path().join("denied.txt").exists());
        }
    }

    #[test]
    fn existing_different_content_and_non_file_targets_are_typed_conflicts() {
        let workspace = tempdir().unwrap();
        let (session, uri) = test_session(
            "assets/report.txt",
            SkillResourceKind::Asset,
            b"new",
            b"new",
        );
        fs::write(workspace.path().join("report.txt"), b"old").unwrap();
        let request = test_request(uri.clone(), workspace.path(), "report.txt");
        let error = SkillResourceMaterializer::new()
            .materialize(&session, &request)
            .unwrap_err();
        assert_eq!(error.code(), SkillMaterializationErrorCode::Conflict);
        assert_eq!(
            fs::read(workspace.path().join("report.txt")).unwrap(),
            b"old"
        );

        fs::create_dir(workspace.path().join("directory")).unwrap();
        let directory_request = test_request(uri, workspace.path(), "directory");
        assert_eq!(
            SkillResourceMaterializer::new()
                .materialize(&session, &directory_request)
                .unwrap_err()
                .code(),
            SkillMaterializationErrorCode::Conflict
        );
    }

    #[test]
    fn destination_validation_rejects_traversal_absolute_and_reserved_trees() {
        for destination in [
            "../outside.txt",
            "/tmp/outside.txt",
            ".git/hooks/post-commit",
            "output/.git/config",
            ".HG/store/file",
            ".agents/skills/evil/SKILL.md",
            ".mycopilot-skill-stage-forged",
        ] {
            let error = SkillMaterializationDestination::parse(destination).unwrap_err();
            assert!(matches!(
                error.code(),
                SkillMaterializationErrorCode::InvalidDestination
                    | SkillMaterializationErrorCode::ReservedDestination
            ));
        }
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_parent_and_destination_never_touch_outside_files() {
        use std::os::unix::fs::symlink;

        let workspace = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let (session, uri) = test_session(
            "assets/report.txt",
            SkillResourceKind::Asset,
            b"safe",
            b"safe",
        );
        let linked_root = workspace.path().join("linked-root");
        symlink(outside.path(), &linked_root).unwrap();
        let root_request = test_request(uri.clone(), &linked_root, "root-out.txt");
        let root_error = SkillResourceMaterializer::new()
            .materialize(&session, &root_request)
            .unwrap_err();
        assert_eq!(
            root_error.code(),
            SkillMaterializationErrorCode::InvalidWorkspace
        );
        assert!(!outside.path().join("root-out.txt").exists());

        symlink(outside.path(), workspace.path().join("linked")).unwrap();
        let parent_request = test_request(uri.clone(), workspace.path(), "linked/out.txt");
        let parent_error = SkillResourceMaterializer::new()
            .materialize(&session, &parent_request)
            .unwrap_err();
        assert_eq!(
            parent_error.code(),
            SkillMaterializationErrorCode::UnsafeParent
        );
        assert!(!outside.path().join("out.txt").exists());

        let canary = outside.path().join("canary.txt");
        fs::write(&canary, b"CANARY").unwrap();
        symlink(&canary, workspace.path().join("target.txt")).unwrap();
        let target_request = test_request(uri, workspace.path(), "target.txt");
        let target_error = SkillResourceMaterializer::new()
            .materialize(&session, &target_request)
            .unwrap_err();
        assert_eq!(
            target_error.code(),
            SkillMaterializationErrorCode::UnsafeDestination
        );
        assert_eq!(fs::read(canary).unwrap(), b"CANARY");
    }

    #[cfg(unix)]
    #[test]
    fn replaced_file_staging_inode_is_rejected_and_never_published_or_deleted() {
        use super::unix::{install_materialization_test_hook, MaterializationTestHookPoint};

        let workspace = tempdir().unwrap();
        let bytes = b"verified bytes";
        let (session, uri) =
            test_session("assets/report.txt", SkillResourceKind::Asset, bytes, bytes);
        let request = test_request(uri, workspace.path(), "report.txt");
        let replacement_name = Arc::new(Mutex::new(None::<String>));
        let captured_name = Arc::clone(&replacement_name);
        let _hook = install_materialization_test_hook(move |point, parent_fd, staging_name| {
            if point != MaterializationTestHookPoint::FileBeforePublish {
                return;
            }
            *captured_name.lock().unwrap() = Some(staging_name.to_string_lossy().into_owned());
            let moved = CString::new(".test-original-staging").unwrap();
            // SAFETY: all fds and C strings are live for these test-only calls.
            assert_eq!(
                unsafe {
                    libc::renameat(parent_fd, staging_name.as_ptr(), parent_fd, moved.as_ptr())
                },
                0
            );
            let fd = unsafe {
                libc::openat(
                    parent_fd,
                    staging_name.as_ptr(),
                    libc::O_WRONLY
                        | libc::O_CREAT
                        | libc::O_EXCL
                        | libc::O_NOFOLLOW
                        | libc::O_CLOEXEC,
                    0o600,
                )
            };
            assert!(fd >= 0);
            // SAFETY: fd is newly owned by this test.
            let mut replacement = unsafe { fs::File::from_raw_fd(fd) };
            replacement.write_all(b"attacker replacement").unwrap();
            replacement.sync_all().unwrap();
        });

        let error = SkillResourceMaterializer::new()
            .materialize(&session, &request)
            .unwrap_err();
        assert_eq!(
            error.code(),
            SkillMaterializationErrorCode::UnsafeDestination
        );
        assert!(!workspace.path().join("report.txt").exists());
        let replacement_name = replacement_name.lock().unwrap().clone().unwrap();
        assert_eq!(
            fs::read(workspace.path().join(replacement_name)).unwrap(),
            b"attacker replacement"
        );
    }

    #[cfg(unix)]
    #[test]
    fn workspace_root_rebind_is_detected_before_publication() {
        use super::unix::{install_materialization_test_hook, MaterializationTestHookPoint};

        let container = tempdir().unwrap();
        let workspace = container.path().join("workspace");
        let moved_workspace = container.path().join("workspace-moved");
        fs::create_dir(&workspace).unwrap();
        let bytes = b"verified bytes";
        let (session, uri) =
            test_session("assets/report.txt", SkillResourceKind::Asset, bytes, bytes);
        let request = test_request(uri, &workspace, "report.txt");
        let hook_workspace = workspace.clone();
        let hook_moved = moved_workspace.clone();
        let _hook = install_materialization_test_hook(move |point, _, _| {
            if point == MaterializationTestHookPoint::FileBeforePublish {
                fs::rename(&hook_workspace, &hook_moved).unwrap();
                fs::create_dir(&hook_workspace).unwrap();
            }
        });

        let error = SkillResourceMaterializer::new()
            .materialize(&session, &request)
            .unwrap_err();
        assert_eq!(error.code(), SkillMaterializationErrorCode::UnsafeParent);
        assert!(!workspace.join("report.txt").exists());
        assert!(!moved_workspace.join("report.txt").exists());
        assert!(staging_entries(&moved_workspace).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn replaced_tree_staging_inode_is_rejected_and_never_published_or_deleted() {
        use super::unix::{install_materialization_test_hook, MaterializationTestHookPoint};

        let workspace = tempdir().unwrap();
        let bytes = b"verified template";
        let (session, package) = test_tree_session(&[(
            "templates/kit/template.txt",
            SkillResourceKind::Other,
            bytes,
            bytes,
        )]);
        let request = test_tree_request(package, "templates/kit", workspace.path(), "kit");
        let replacement_name = Arc::new(Mutex::new(None::<String>));
        let captured_name = Arc::clone(&replacement_name);
        let _hook = install_materialization_test_hook(move |point, parent_fd, staging_name| {
            if point != MaterializationTestHookPoint::TreeBeforePublish {
                return;
            }
            *captured_name.lock().unwrap() = Some(staging_name.to_string_lossy().into_owned());
            let moved = CString::new(".test-original-staging-tree").unwrap();
            // SAFETY: all fds and C strings are live for these test-only calls.
            assert_eq!(
                unsafe {
                    libc::renameat(parent_fd, staging_name.as_ptr(), parent_fd, moved.as_ptr())
                },
                0
            );
            assert_eq!(
                unsafe { libc::mkdirat(parent_fd, staging_name.as_ptr(), 0o700) },
                0
            );
        });

        let error = SkillResourceMaterializer::new()
            .materialize_template_tree(&session, &request)
            .unwrap_err();
        assert_eq!(
            error.code(),
            SkillMaterializationErrorCode::UnsafeDestination
        );
        assert!(!workspace.path().join("kit").exists());
        let replacement_name = replacement_name.lock().unwrap().clone().unwrap();
        assert!(workspace.path().join(replacement_name).is_dir());
        assert_eq!(
            fs::read(
                workspace
                    .path()
                    .join(".test-original-staging-tree/template.txt")
            )
            .unwrap(),
            bytes
        );
    }

    #[test]
    fn missing_parent_and_tampered_source_fail_without_partial_files() {
        let workspace = tempdir().unwrap();
        let (session, uri) = test_session(
            "assets/report.txt",
            SkillResourceKind::Asset,
            b"safe",
            b"safe",
        );
        let missing_request = test_request(uri, workspace.path(), "missing/out.txt");
        let missing = SkillResourceMaterializer::new()
            .materialize(&session, &missing_request)
            .unwrap_err();
        assert_eq!(
            missing.code(),
            SkillMaterializationErrorCode::DestinationParentNotFound
        );

        let (tampered_session, tampered_uri) = test_session(
            "assets/tampered.txt",
            SkillResourceKind::Asset,
            b"expected",
            b"tampered",
        );
        let tampered_request = test_request(tampered_uri, workspace.path(), "tampered.txt");
        let tampered = SkillResourceMaterializer::new()
            .materialize(&tampered_session, &tampered_request)
            .unwrap_err();
        assert_eq!(
            tampered.code(),
            SkillMaterializationErrorCode::ResourceError
        );
        assert_eq!(
            tampered.resource_error().unwrap().code().stable_name(),
            "integrityMismatch"
        );
        assert!(!workspace.path().join("tampered.txt").exists());
        assert!(staging_entries(workspace.path()).is_empty());
    }

    #[test]
    fn template_tree_is_published_atomically_and_repeated_as_an_idempotent_hit() {
        let workspace = tempdir().unwrap();
        fs::create_dir(workspace.path().join("exports")).unwrap();
        let budget = b"xlsx-template";
        let notes = b"template notes\n";
        let (session, package) = test_tree_session(&[
            (
                "templates/office/budget.xlsx",
                SkillResourceKind::Other,
                budget,
                budget,
            ),
            (
                "templates/office/docs/notes.txt",
                SkillResourceKind::Other,
                notes,
                notes,
            ),
            (
                "templates/unrelated.txt",
                SkillResourceKind::Other,
                b"not selected",
                b"not selected",
            ),
        ]);
        let request = test_tree_request(
            package,
            "templates/office",
            workspace.path(),
            "exports/office-kit",
        );
        let materializer = SkillResourceMaterializer::new();

        let created = materializer
            .materialize_template_tree(&session, &request)
            .unwrap();
        assert_eq!(created.status(), SkillMaterializationStatus::Created);
        assert_eq!(created.file_count(), 2);
        assert_eq!(created.byte_length(), (budget.len() + notes.len()) as u64);
        assert_eq!(created.bytes_written(), created.byte_length());
        assert!(created
            .plan_digest()
            .starts_with(SKILL_MATERIALIZATION_TREE_DIGEST_PREFIX));
        assert_eq!(
            created
                .entries()
                .iter()
                .map(SkillMaterializedTreeEntry::relative_path)
                .collect::<Vec<_>>(),
            vec!["budget.xlsx", "docs/notes.txt"]
        );
        let destination = workspace.path().join("exports/office-kit");
        assert_eq!(fs::read(destination.join("budget.xlsx")).unwrap(), budget);
        assert_eq!(fs::read(destination.join("docs/notes.txt")).unwrap(), notes);
        assert!(!destination.join("unrelated.txt").exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&destination).unwrap().permissions().mode() & 0o777,
                0o700
            );
            assert_eq!(
                fs::metadata(destination.join("docs"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
            assert_eq!(
                fs::metadata(destination.join("budget.xlsx"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        assert!(staging_entries(&workspace.path().join("exports")).is_empty());

        let repeated = materializer
            .materialize_template_tree(&session, &request)
            .unwrap();
        assert_eq!(
            repeated.status(),
            SkillMaterializationStatus::AlreadyPresent
        );
        assert_eq!(repeated.bytes_written(), 0);
        assert_eq!(repeated.plan_digest(), created.plan_digest());
        assert!(staging_entries(&workspace.path().join("exports")).is_empty());
    }

    #[test]
    fn template_tree_never_merges_with_or_overwrites_an_existing_destination() {
        let workspace = tempdir().unwrap();
        let bytes = b"new-template";
        let (session, package) = test_tree_session(&[(
            "templates/kit/template.txt",
            SkillResourceKind::Other,
            bytes,
            bytes,
        )]);
        let request = test_tree_request(package, "templates/kit", workspace.path(), "kit");
        fs::create_dir(workspace.path().join("kit")).unwrap();
        fs::write(workspace.path().join("kit/canary.txt"), b"CANARY").unwrap();

        let error = SkillResourceMaterializer::new()
            .materialize_template_tree(&session, &request)
            .unwrap_err();
        assert_eq!(error.code(), SkillMaterializationErrorCode::Conflict);
        assert_eq!(
            fs::read(workspace.path().join("kit/canary.txt")).unwrap(),
            b"CANARY"
        );
        assert!(!workspace.path().join("kit/template.txt").exists());
        assert!(staging_entries(workspace.path()).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn template_tree_rejects_nested_symlinks_without_touching_their_targets() {
        use std::os::unix::fs::symlink;

        let workspace = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let bytes = b"template";
        let (session, package) = test_tree_session(&[(
            "templates/kit/docs/template.txt",
            SkillResourceKind::Other,
            bytes,
            bytes,
        )]);
        let request = test_tree_request(package, "templates/kit", workspace.path(), "kit");
        fs::create_dir(workspace.path().join("kit")).unwrap();
        fs::write(outside.path().join("canary.txt"), b"CANARY").unwrap();
        symlink(outside.path(), workspace.path().join("kit/docs")).unwrap();

        let error = SkillResourceMaterializer::new()
            .materialize_template_tree(&session, &request)
            .unwrap_err();
        assert_eq!(
            error.code(),
            SkillMaterializationErrorCode::UnsafeDestination
        );
        assert_eq!(
            fs::read(outside.path().join("canary.txt")).unwrap(),
            b"CANARY"
        );
        assert!(!outside.path().join("template.txt").exists());
        assert!(staging_entries(workspace.path()).is_empty());
    }

    #[test]
    fn template_tree_validates_prefix_empty_source_and_all_bytes_before_writing() {
        let workspace = tempdir().unwrap();
        let denied = SkillTemplateTreeMaterializationRequest::new(
            SkillPackageUri::new(
                SkillId::parse("installed:user:21234567-89ab-4def-8123-456789abcdef").unwrap(),
                SkillRevision::parse(format!("skill-package-sha256-v3:{}", "c".repeat(64)))
                    .unwrap(),
            ),
            SkillResourcePath::parse("assets").unwrap(),
            workspace.path(),
            SkillMaterializationDestination::parse("denied").unwrap(),
        )
        .unwrap_err();
        assert_eq!(
            denied.code(),
            SkillMaterializationErrorCode::SourcePrefixDenied
        );

        let (empty_session, empty_package) = test_tree_session(&[(
            "templates/other/file.txt",
            SkillResourceKind::Other,
            b"other",
            b"other",
        )]);
        let empty_request = test_tree_request(
            empty_package,
            "templates/missing",
            workspace.path(),
            "empty",
        );
        let empty = SkillResourceMaterializer::new()
            .materialize_template_tree(&empty_session, &empty_request)
            .unwrap_err();
        assert_eq!(empty.code(), SkillMaterializationErrorCode::EmptySource);
        assert!(!workspace.path().join("empty").exists());

        let (tampered_session, tampered_package) = test_tree_session(&[
            (
                "templates/kit/a.txt",
                SkillResourceKind::Other,
                b"safe",
                b"safe",
            ),
            (
                "templates/kit/b.txt",
                SkillResourceKind::Other,
                b"expected",
                b"tampered",
            ),
        ]);
        let tampered_request = test_tree_request(
            tampered_package,
            "templates/kit",
            workspace.path(),
            "tampered",
        );
        let tampered = SkillResourceMaterializer::new()
            .materialize_template_tree(&tampered_session, &tampered_request)
            .unwrap_err();
        assert_eq!(
            tampered.code(),
            SkillMaterializationErrorCode::ResourceError
        );
        assert_eq!(
            tampered.resource_error().unwrap().code().stable_name(),
            "integrityMismatch"
        );
        assert!(!workspace.path().join("tampered").exists());
        assert!(staging_entries(workspace.path()).is_empty());
    }

    #[test]
    fn concurrent_identical_template_trees_converge_without_merging() {
        let workspace = tempdir().unwrap();
        let bytes = b"shared-template";
        let (session, package) = test_tree_session(&[(
            "templates/kit/nested/template.txt",
            SkillResourceKind::Other,
            bytes,
            bytes,
        )]);
        let request = test_tree_request(package, "templates/kit", workspace.path(), "kit");
        let session = Arc::new(session);
        let request = Arc::new(request);
        let barrier = Arc::new(Barrier::new(2));
        let mut handles = Vec::new();
        for _ in 0..2 {
            let session = session.clone();
            let request = request.clone();
            let barrier = barrier.clone();
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                SkillResourceMaterializer::new()
                    .materialize_template_tree(&session, &request)
                    .unwrap()
                    .status()
            }));
        }
        let mut statuses = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>();
        statuses.sort_by_key(|status| status.stable_name());
        assert_eq!(
            statuses,
            vec![
                SkillMaterializationStatus::AlreadyPresent,
                SkillMaterializationStatus::Created
            ]
        );
        assert_eq!(
            fs::read(workspace.path().join("kit/nested/template.txt")).unwrap(),
            bytes
        );
        assert!(staging_entries(workspace.path()).is_empty());
    }

    #[test]
    fn concurrent_identical_requests_converge_to_one_file() {
        let workspace = tempdir().unwrap();
        let bytes = b"concurrent bytes";
        let (session, uri) =
            test_session("assets/shared.txt", SkillResourceKind::Asset, bytes, bytes);
        let request = test_request(uri, workspace.path(), "shared.txt");
        let session = Arc::new(session);
        let request = Arc::new(request);
        let barrier = Arc::new(Barrier::new(2));
        let mut handles = Vec::new();
        for _ in 0..2 {
            let session = session.clone();
            let request = request.clone();
            let barrier = barrier.clone();
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                SkillResourceMaterializer::new()
                    .materialize(&session, &request)
                    .unwrap()
                    .status()
            }));
        }
        let mut statuses = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>();
        statuses.sort_by_key(|status| status.stable_name());
        assert_eq!(
            statuses,
            vec![
                SkillMaterializationStatus::AlreadyPresent,
                SkillMaterializationStatus::Created
            ]
        );
        assert_eq!(
            fs::read(workspace.path().join("shared.txt")).unwrap(),
            bytes
        );
        assert!(staging_entries(workspace.path()).is_empty());
    }
}
