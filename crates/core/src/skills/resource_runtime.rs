//! Run-scoped, revision-bound read access to Skill package resources.
//!
//! The runtime exposes logical `skill://package/...` URIs only. Filesystem
//! locations remain private to their source provider, and every read is
//! authorized against the exact id + revision set captured by this session.

use super::digest::package_file_digest;
use super::model::{
    SkillId, SkillResolveError, SkillResourceDescriptor, SkillResourceIndex, SkillResourceKind,
    SkillRevision, SkillSourceId,
};
use super::package::SkillPackagePath;
use super::workspace::{percent_encode, SKILL_FILE_NAME};
use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};

const SKILL_PACKAGE_URI_PREFIX: &str = "skill://package/";
pub const MAX_SKILL_RESOURCE_URI_BYTES: usize = 64 * 1024;
pub const DEFAULT_SKILL_RESOURCE_LIST_PAGE_SIZE: usize = 100;
pub const MAX_SKILL_RESOURCE_LIST_PAGE_SIZE: usize = 200;
pub const DEFAULT_SKILL_RESOURCE_TEXT_PAGE_BYTES: usize = 64 * 1024;
pub const MAX_SKILL_RESOURCE_TEXT_PAGE_BYTES: usize = 256 * 1024;
const MIN_SKILL_RESOURCE_TEXT_PAGE_BYTES: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SkillResourceErrorCode {
    InvalidUri,
    InvalidRequest,
    SkillNotActivated,
    RevisionNotActivated,
    ResourceNotFound,
    ResourceNotText,
    InvalidOffset,
    IntegrityMismatch,
    SnapshotUnavailable,
    SourceContractViolation,
}

impl SkillResourceErrorCode {
    pub fn stable_name(self) -> &'static str {
        match self {
            Self::InvalidUri => "invalidUri",
            Self::InvalidRequest => "invalidRequest",
            Self::SkillNotActivated => "skillNotActivated",
            Self::RevisionNotActivated => "revisionNotActivated",
            Self::ResourceNotFound => "resourceNotFound",
            Self::ResourceNotText => "resourceNotText",
            Self::InvalidOffset => "invalidOffset",
            Self::IntegrityMismatch => "integrityMismatch",
            Self::SnapshotUnavailable => "snapshotUnavailable",
            Self::SourceContractViolation => "sourceContractViolation",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SkillResourceRecovery {
    ChangeRequest,
    ListResources,
    ReactivateSkill,
    ReinstallSkill,
    Retry,
    ReconfigureSource,
}

impl SkillResourceRecovery {
    pub fn stable_name(self) -> &'static str {
        match self {
            Self::ChangeRequest => "changeRequest",
            Self::ListResources => "listResources",
            Self::ReactivateSkill => "reactivateSkill",
            Self::ReinstallSkill => "reinstallSkill",
            Self::Retry => "retry",
            Self::ReconfigureSource => "reconfigureSource",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillResourceUriError {
    reason: String,
}

impl SkillResourceUriError {
    fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }

    pub fn code(&self) -> SkillResourceErrorCode {
        SkillResourceErrorCode::InvalidUri
    }

    pub fn recovery(&self) -> SkillResourceRecovery {
        SkillResourceRecovery::ChangeRequest
    }

    pub fn reason(&self) -> &str {
        &self.reason
    }
}

impl fmt::Display for SkillResourceUriError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "invalid Skill package URI: {}", self.reason)
    }
}

impl Error for SkillResourceUriError {}

/// A validated, portable path inside a Skill package.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SkillResourcePath(String);

impl SkillResourcePath {
    pub fn parse(value: impl Into<String>) -> Result<Self, SkillResourceUriError> {
        let value = value.into();
        let path = SkillPackagePath::parse(value).map_err(|error| {
            SkillResourceUriError::new(format!("invalid resource path: {}", error.message))
        })?;
        if path.as_str() == SKILL_FILE_NAME {
            return Err(SkillResourceUriError::new(
                "SKILL.md is the package entrypoint, not a sibling resource",
            ));
        }
        Ok(Self(path.as_str().to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn is_same_or_descendant_of(&self, prefix: &Self) -> bool {
        self == prefix
            || self
                .0
                .strip_prefix(prefix.as_str())
                .is_some_and(|suffix| suffix.starts_with('/'))
    }
}

impl fmt::Display for SkillResourcePath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Canonical root URI for one immutable Skill package.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SkillPackageUri {
    skill_id: SkillId,
    revision: SkillRevision,
    serialized: Arc<str>,
}

impl SkillPackageUri {
    pub fn new(skill_id: SkillId, revision: SkillRevision) -> Self {
        let serialized = Arc::<str>::from(format!(
            "{SKILL_PACKAGE_URI_PREFIX}{}/{}/",
            percent_encode(skill_id.as_str().as_bytes()),
            percent_encode(revision.as_str().as_bytes())
        ));
        Self {
            skill_id,
            revision,
            serialized,
        }
    }

    pub fn parse(value: &str) -> Result<Self, SkillResourceUriError> {
        let parsed = parse_package_uri(value, false)?;
        Ok(parsed.package)
    }

    pub fn skill_id(&self) -> &SkillId {
        &self.skill_id
    }

    pub fn revision(&self) -> &SkillRevision {
        &self.revision
    }

    pub fn as_str(&self) -> &str {
        &self.serialized
    }

    pub fn resource(&self, path: SkillResourcePath) -> SkillResourceUri {
        SkillResourceUri::new(self.clone(), path)
    }
}

impl fmt::Display for SkillPackageUri {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.serialized.fmt(formatter)
    }
}

/// Canonical URI for one revision-bound sibling resource.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SkillResourceUri {
    package: SkillPackageUri,
    path: SkillResourcePath,
    serialized: Arc<str>,
}

impl SkillResourceUri {
    fn new(package: SkillPackageUri, path: SkillResourcePath) -> Self {
        let encoded_path = path
            .as_str()
            .split('/')
            .map(|component| percent_encode(component.as_bytes()))
            .collect::<Vec<_>>()
            .join("/");
        let serialized = Arc::<str>::from(format!("{}{encoded_path}", package.as_str()));
        Self {
            package,
            path,
            serialized,
        }
    }

    pub fn parse(value: &str) -> Result<Self, SkillResourceUriError> {
        let parsed = parse_package_uri(value, true)?;
        let path = parsed.path.expect("resource parsing requires a path");
        Ok(Self::new(parsed.package, path))
    }

    pub fn package(&self) -> &SkillPackageUri {
        &self.package
    }

    pub fn path(&self) -> &SkillResourcePath {
        &self.path
    }

    pub fn as_str(&self) -> &str {
        &self.serialized
    }
}

impl fmt::Display for SkillResourceUri {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.serialized.fmt(formatter)
    }
}

struct ParsedPackageUri {
    package: SkillPackageUri,
    path: Option<SkillResourcePath>,
}

fn parse_package_uri(
    value: &str,
    require_resource: bool,
) -> Result<ParsedPackageUri, SkillResourceUriError> {
    if value.len() > MAX_SKILL_RESOURCE_URI_BYTES {
        return Err(SkillResourceUriError::new("URI exceeds the size limit"));
    }
    if value.contains(['?', '#']) {
        return Err(SkillResourceUriError::new(
            "query strings and fragments are not supported",
        ));
    }
    let rest = value
        .strip_prefix(SKILL_PACKAGE_URI_PREFIX)
        .ok_or_else(|| {
            SkillResourceUriError::new("URI must use the exact `skill://package/` prefix")
        })?;
    let segments = rest.split('/').collect::<Vec<_>>();
    let valid_shape = if require_resource {
        segments.len() >= 3 && segments.iter().all(|segment| !segment.is_empty())
    } else {
        segments.len() == 3
            && !segments[0].is_empty()
            && !segments[1].is_empty()
            && segments[2].is_empty()
    };
    if !valid_shape {
        return Err(SkillResourceUriError::new(if require_resource {
            "resource URI must contain an id, revision, and non-empty resource path"
        } else {
            "package URI must contain an id and revision and end with `/`"
        }));
    }

    let skill_id_text = decode_uri_segment(segments[0])?;
    let revision_text = decode_uri_segment(segments[1])?;
    let skill_id = SkillId::parse(skill_id_text)
        .map_err(|_| SkillResourceUriError::new("URI contains an invalid Skill id"))?;
    let revision = SkillRevision::parse(revision_text)
        .map_err(|_| SkillResourceUriError::new("URI contains an invalid Skill revision"))?;
    let package = SkillPackageUri::new(skill_id, revision);
    let path = if require_resource {
        let decoded = segments[2..]
            .iter()
            .map(|segment| decode_uri_segment(segment))
            .collect::<Result<Vec<_>, _>>()?
            .join("/");
        Some(SkillResourcePath::parse(decoded)?)
    } else {
        None
    };
    let canonical = path.as_ref().map_or_else(
        || package.as_str().to_string(),
        |path| package.resource(path.clone()).to_string(),
    );
    if canonical != value {
        return Err(SkillResourceUriError::new(
            "URI is not in canonical percent-encoded form",
        ));
    }
    Ok(ParsedPackageUri { package, path })
}

fn decode_uri_segment(segment: &str) -> Result<String, SkillResourceUriError> {
    let bytes = segment.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index.saturating_add(2) >= bytes.len() {
                return Err(SkillResourceUriError::new(
                    "URI contains an incomplete percent escape",
                ));
            }
            let high = decode_hex(bytes[index + 1]).ok_or_else(|| {
                SkillResourceUriError::new("URI contains an invalid percent escape")
            })?;
            let low = decode_hex(bytes[index + 2]).ok_or_else(|| {
                SkillResourceUriError::new("URI contains an invalid percent escape")
            })?;
            decoded.push((high << 4) | low);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded)
        .map_err(|_| SkillResourceUriError::new("URI segment is not valid UTF-8"))
}

fn decode_hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillResourceListOptions {
    prefix: Option<SkillResourcePath>,
    kind: Option<SkillResourceKind>,
    after: Option<SkillResourcePath>,
    limit: usize,
}

impl SkillResourceListOptions {
    pub fn new(limit: usize) -> Result<Self, SkillResourceError> {
        if !(1..=MAX_SKILL_RESOURCE_LIST_PAGE_SIZE).contains(&limit) {
            return Err(SkillResourceError::InvalidRequest {
                reason: format!(
                    "resource list page size must be between 1 and {MAX_SKILL_RESOURCE_LIST_PAGE_SIZE}"
                ),
            });
        }
        Ok(Self {
            prefix: None,
            kind: None,
            after: None,
            limit,
        })
    }

    pub fn with_prefix(mut self, prefix: SkillResourcePath) -> Self {
        self.prefix = Some(prefix);
        self
    }

    pub fn with_kind(mut self, kind: SkillResourceKind) -> Self {
        self.kind = Some(kind);
        self
    }

    pub fn with_after(mut self, after: SkillResourcePath) -> Self {
        self.after = Some(after);
        self
    }

    pub fn prefix(&self) -> Option<&SkillResourcePath> {
        self.prefix.as_ref()
    }

    pub fn kind(&self) -> Option<SkillResourceKind> {
        self.kind
    }

    pub fn after(&self) -> Option<&SkillResourcePath> {
        self.after.as_ref()
    }

    pub fn limit(&self) -> usize {
        self.limit
    }
}

impl Default for SkillResourceListOptions {
    fn default() -> Self {
        Self::new(DEFAULT_SKILL_RESOURCE_LIST_PAGE_SIZE)
            .expect("default Skill resource page size is valid")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillResourceListEntry {
    uri: SkillResourceUri,
    descriptor: SkillResourceDescriptor,
}

impl SkillResourceListEntry {
    pub fn uri(&self) -> &SkillResourceUri {
        &self.uri
    }

    pub fn descriptor(&self) -> &SkillResourceDescriptor {
        &self.descriptor
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillResourceListPage {
    package: SkillPackageUri,
    entries: Vec<SkillResourceListEntry>,
    next_after: Option<SkillResourcePath>,
}

impl SkillResourceListPage {
    pub fn package(&self) -> &SkillPackageUri {
        &self.package
    }

    pub fn entries(&self) -> &[SkillResourceListEntry] {
        &self.entries
    }

    pub fn next_after(&self) -> Option<&SkillResourcePath> {
        self.next_after.as_ref()
    }

    pub fn has_more(&self) -> bool {
        self.next_after.is_some()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SkillResourceTextReadOptions {
    offset: usize,
    max_bytes: usize,
}

impl SkillResourceTextReadOptions {
    pub fn new(offset: usize, max_bytes: usize) -> Result<Self, SkillResourceError> {
        if !(MIN_SKILL_RESOURCE_TEXT_PAGE_BYTES..=MAX_SKILL_RESOURCE_TEXT_PAGE_BYTES)
            .contains(&max_bytes)
        {
            return Err(SkillResourceError::InvalidRequest {
                reason: format!(
                    "resource text page size must be between {MIN_SKILL_RESOURCE_TEXT_PAGE_BYTES} and {MAX_SKILL_RESOURCE_TEXT_PAGE_BYTES} bytes"
                ),
            });
        }
        Ok(Self { offset, max_bytes })
    }

    pub fn offset(self) -> usize {
        self.offset
    }

    pub fn max_bytes(self) -> usize {
        self.max_bytes
    }
}

impl Default for SkillResourceTextReadOptions {
    fn default() -> Self {
        Self::new(0, DEFAULT_SKILL_RESOURCE_TEXT_PAGE_BYTES)
            .expect("default Skill resource text page size is valid")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillResourceTextPage {
    uri: SkillResourceUri,
    text: String,
    offset: usize,
    end_offset: usize,
    total_bytes: usize,
}

impl SkillResourceTextPage {
    pub fn uri(&self) -> &SkillResourceUri {
        &self.uri
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn offset(&self) -> usize {
        self.offset
    }

    pub fn end_offset(&self) -> usize {
        self.end_offset
    }

    pub fn total_bytes(&self) -> usize {
        self.total_bytes
    }

    pub fn truncated(&self) -> bool {
        self.end_offset < self.total_bytes
    }

    pub fn next_offset(&self) -> Option<usize> {
        self.truncated().then_some(self.end_offset)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SkillResourceError {
    InvalidRequest {
        reason: String,
    },
    SkillNotActivated {
        skill_id: SkillId,
    },
    RevisionNotActivated {
        skill_id: SkillId,
        requested_revision: SkillRevision,
        activated_revision: SkillRevision,
    },
    ResourceNotFound {
        uri: Box<SkillResourceUri>,
    },
    ResourceNotText {
        uri: Box<SkillResourceUri>,
    },
    InvalidOffset {
        uri: Box<SkillResourceUri>,
        offset: usize,
    },
    IntegrityMismatch {
        uri: Box<SkillResourceUri>,
        reason: String,
    },
    SnapshotIntegrityMismatch {
        skill_id: SkillId,
        revision: SkillRevision,
        reason: String,
    },
    SnapshotUnavailable {
        skill_id: SkillId,
        revision: SkillRevision,
        reason: String,
    },
    SourceContractViolation {
        source_id: SkillSourceId,
        reason: String,
    },
}

impl SkillResourceError {
    pub fn code(&self) -> SkillResourceErrorCode {
        match self {
            Self::InvalidRequest { .. } => SkillResourceErrorCode::InvalidRequest,
            Self::SkillNotActivated { .. } => SkillResourceErrorCode::SkillNotActivated,
            Self::RevisionNotActivated { .. } => SkillResourceErrorCode::RevisionNotActivated,
            Self::ResourceNotFound { .. } => SkillResourceErrorCode::ResourceNotFound,
            Self::ResourceNotText { .. } => SkillResourceErrorCode::ResourceNotText,
            Self::InvalidOffset { .. } => SkillResourceErrorCode::InvalidOffset,
            Self::IntegrityMismatch { .. } | Self::SnapshotIntegrityMismatch { .. } => {
                SkillResourceErrorCode::IntegrityMismatch
            }
            Self::SnapshotUnavailable { .. } => SkillResourceErrorCode::SnapshotUnavailable,
            Self::SourceContractViolation { .. } => SkillResourceErrorCode::SourceContractViolation,
        }
    }

    pub fn recovery(&self) -> SkillResourceRecovery {
        match self {
            Self::InvalidRequest { .. } | Self::InvalidOffset { .. } => {
                SkillResourceRecovery::ChangeRequest
            }
            Self::SkillNotActivated { .. } | Self::RevisionNotActivated { .. } => {
                SkillResourceRecovery::ReactivateSkill
            }
            Self::ResourceNotFound { .. } => SkillResourceRecovery::ListResources,
            Self::ResourceNotText { .. } => SkillResourceRecovery::ChangeRequest,
            Self::IntegrityMismatch { .. } | Self::SnapshotIntegrityMismatch { .. } => {
                SkillResourceRecovery::ReinstallSkill
            }
            Self::SnapshotUnavailable { .. } => SkillResourceRecovery::Retry,
            Self::SourceContractViolation { .. } => SkillResourceRecovery::ReconfigureSource,
        }
    }

    pub fn message(&self) -> String {
        self.to_string()
    }

    pub fn skill_id(&self) -> Option<&SkillId> {
        match self {
            Self::SkillNotActivated { skill_id }
            | Self::RevisionNotActivated { skill_id, .. }
            | Self::SnapshotIntegrityMismatch { skill_id, .. }
            | Self::SnapshotUnavailable { skill_id, .. } => Some(skill_id),
            Self::ResourceNotFound { uri }
            | Self::ResourceNotText { uri }
            | Self::InvalidOffset { uri, .. }
            | Self::IntegrityMismatch { uri, .. } => Some(uri.package().skill_id()),
            Self::InvalidRequest { .. } | Self::SourceContractViolation { .. } => None,
        }
    }

    pub fn revision(&self) -> Option<&SkillRevision> {
        match self {
            Self::RevisionNotActivated {
                requested_revision, ..
            } => Some(requested_revision),
            Self::SnapshotIntegrityMismatch { revision, .. }
            | Self::SnapshotUnavailable { revision, .. } => Some(revision),
            Self::ResourceNotFound { uri }
            | Self::ResourceNotText { uri }
            | Self::InvalidOffset { uri, .. }
            | Self::IntegrityMismatch { uri, .. } => Some(uri.package().revision()),
            Self::InvalidRequest { .. }
            | Self::SkillNotActivated { .. }
            | Self::SourceContractViolation { .. } => None,
        }
    }

    pub fn uri(&self) -> Option<&SkillResourceUri> {
        match self {
            Self::ResourceNotFound { uri }
            | Self::ResourceNotText { uri }
            | Self::InvalidOffset { uri, .. }
            | Self::IntegrityMismatch { uri, .. } => Some(uri),
            _ => None,
        }
    }

    pub fn source_id(&self) -> Option<&SkillSourceId> {
        match self {
            Self::SourceContractViolation { source_id, .. } => Some(source_id),
            _ => self.skill_id().map(SkillId::source_id),
        }
    }
}

impl fmt::Display for SkillResourceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRequest { reason } => {
                write!(formatter, "invalid resource request: {reason}")
            }
            Self::SkillNotActivated { skill_id } => {
                write!(
                    formatter,
                    "Skill `{skill_id}` is not activated in this resource session"
                )
            }
            Self::RevisionNotActivated {
                skill_id,
                requested_revision,
                activated_revision,
            } => write!(
                formatter,
                "Skill `{skill_id}` resource URI requests revision `{requested_revision}`, but this session grants `{activated_revision}`"
            ),
            Self::ResourceNotFound { uri } => {
                write!(formatter, "Skill resource `{uri}` was not found")
            }
            Self::ResourceNotText { uri } => {
                write!(formatter, "Skill resource `{uri}` is not valid UTF-8 text")
            }
            Self::InvalidOffset { uri, offset } => write!(
                formatter,
                "byte offset {offset} is outside `{uri}` or does not align to a UTF-8 boundary"
            ),
            Self::IntegrityMismatch { uri, reason } => {
                write!(
                    formatter,
                    "Skill resource `{uri}` failed integrity validation: {reason}"
                )
            }
            Self::SnapshotIntegrityMismatch {
                skill_id,
                revision,
                reason,
            } => write!(
                formatter,
                "Skill `{skill_id}` snapshot `{revision}` failed integrity validation: {reason}"
            ),
            Self::SnapshotUnavailable {
                skill_id,
                revision,
                reason,
            } => write!(
                formatter,
                "Skill `{skill_id}` snapshot `{revision}` is unavailable: {reason}"
            ),
            Self::SourceContractViolation { source_id, reason } => write!(
                formatter,
                "Skill source `{source_id}` violated the resource contract: {reason}"
            ),
        }
    }
}

impl Error for SkillResourceError {}

pub(super) trait SkillResourceReader: Send + Sync {
    fn read(&self, expected: &SkillResourceDescriptor)
        -> Result<Vec<u8>, SkillResourceSourceError>;
}

pub(super) type SkillResourceReaderRef = Arc<dyn SkillResourceReader>;

#[derive(Debug)]
pub(super) enum SkillResourceSourceError {
    Integrity(String),
    Unavailable(String),
}

pub(super) struct SkillResourceSessionBinding {
    pub skill_id: SkillId,
    pub revision: SkillRevision,
    pub source_id: SkillSourceId,
    pub resources: SkillResourceIndex,
    pub reader: Option<SkillResourceReaderRef>,
}

/// Owned bytes verified against the descriptor frozen in a resource session.
/// Kept crate-internal so materialization and script policy can share the same
/// authority path without exposing arbitrary binary reads to protocol callers.
pub(crate) struct VerifiedSkillResourceSnapshot {
    pub descriptor: SkillResourceDescriptor,
    pub bytes: Vec<u8>,
}

pub(super) fn restore_resolve_error(
    skill_id: &SkillId,
    revision: &SkillRevision,
    error: SkillResolveError,
) -> SkillResourceError {
    match error {
        SkillResolveError::InvalidSkill { reason, .. } => {
            SkillResourceError::SnapshotIntegrityMismatch {
                skill_id: skill_id.clone(),
                revision: revision.clone(),
                reason,
            }
        }
        SkillResolveError::SourceContractViolation {
            source_id, reason, ..
        } => SkillResourceError::SourceContractViolation { source_id, reason },
        other => SkillResourceError::SnapshotUnavailable {
            skill_id: skill_id.clone(),
            revision: revision.clone(),
            reason: other.to_string(),
        },
    }
}

#[derive(Clone)]
struct SessionBinding {
    package: SkillPackageUri,
    source_id: SkillSourceId,
    resources: SkillResourceIndex,
    reader: Option<SkillResourceReaderRef>,
}

/// Revision-bound resource grants for one Agent run.
///
/// Construct one session from an [`ActivatedSkillSet`](super::ActivatedSkillSet)
/// or restore it from persisted exact selections through [`SkillsService`](super::SkillsService).
/// Each binding is immutable and never follows a mutable installation receipt. The authorized set
/// may grow atomically when the model activates another Skill during the same run; clones share the
/// same run-scoped authority so runtime tools and the Host executor observe one consistent set.
#[derive(Clone)]
pub struct SkillResourceSession {
    bindings: Arc<RwLock<BTreeMap<SkillId, SessionBinding>>>,
}

impl Default for SkillResourceSession {
    fn default() -> Self {
        Self {
            bindings: Arc::new(RwLock::new(BTreeMap::new())),
        }
    }
}

impl SkillResourceSession {
    pub(super) fn from_bindings(
        bindings: impl IntoIterator<Item = SkillResourceSessionBinding>,
    ) -> Result<Self, SkillResourceError> {
        let mut indexed = BTreeMap::new();
        for binding in bindings {
            if !binding.resources.is_empty() && binding.reader.is_none() {
                return Err(SkillResourceError::SourceContractViolation {
                    source_id: binding.source_id,
                    reason: format!(
                        "Skill `{}` exposes resources without a revision-bound reader",
                        binding.skill_id
                    ),
                });
            }
            let skill_id = binding.skill_id;
            let package = SkillPackageUri::new(skill_id.clone(), binding.revision);
            let value = SessionBinding {
                package,
                source_id: binding.source_id,
                resources: binding.resources,
                reader: binding.reader,
            };
            if indexed.insert(skill_id.clone(), value).is_some() {
                return Err(SkillResourceError::InvalidRequest {
                    reason: format!("Skill `{skill_id}` appears more than once in the session"),
                });
            }
        }
        Ok(Self {
            bindings: Arc::new(RwLock::new(indexed)),
        })
    }

    pub fn empty() -> Self {
        Self::default()
    }

    /// Atomically adds exact bindings prepared by a trusted activation resolver.
    ///
    /// Existing bindings are never replaced. Repeating the same id + revision is idempotent;
    /// attempting to redirect an activated id to another revision fails before any insertion.
    pub(crate) fn extend_from(&self, candidate: &Self) -> Result<(), SkillResourceError> {
        if Arc::ptr_eq(&self.bindings, &candidate.bindings) {
            return Ok(());
        }
        let additions = candidate.read_bindings().clone();
        let mut current = self.write_bindings();
        for (skill_id, addition) in &additions {
            if let Some(existing) = current.get(skill_id) {
                if existing.package != addition.package
                    || existing.source_id != addition.source_id
                    || existing.resources != addition.resources
                {
                    return Err(SkillResourceError::RevisionNotActivated {
                        skill_id: skill_id.clone(),
                        requested_revision: addition.package.revision().clone(),
                        activated_revision: existing.package.revision().clone(),
                    });
                }
            }
        }
        for (skill_id, addition) in additions {
            current.entry(skill_id).or_insert(addition);
        }
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.read_bindings().len()
    }

    pub fn is_empty(&self) -> bool {
        self.read_bindings().is_empty()
    }

    pub fn package_uris(&self) -> Vec<SkillPackageUri> {
        self.read_bindings()
            .values()
            .map(|binding| binding.package.clone())
            .collect()
    }

    /// Freeze the candidate session's sole binding for one expected package.
    ///
    /// An empty candidate is represented by `None`. Any non-empty candidate
    /// must contain exactly the requested Skill id + revision; a resolver may
    /// not smuggle grants for another package into an activation result.
    pub(crate) fn freeze_exact_activation_binding(
        &self,
        expected: &SkillPackageUri,
    ) -> Result<Option<Self>, SkillResourceError> {
        let bindings = self.read_bindings();
        if bindings.is_empty() {
            return Ok(None);
        }
        if bindings.len() != 1 {
            return Err(SkillResourceError::InvalidRequest {
                reason: format!(
                    "activation candidate for `{expected}` contains {} package bindings; expected exactly one",
                    bindings.len()
                ),
            });
        }
        let binding = bindings
            .values()
            .next()
            .expect("non-empty binding map must have one value");
        if binding.package != *expected {
            return Err(SkillResourceError::InvalidRequest {
                reason: format!(
                    "activation candidate grants `{}` instead of `{expected}`",
                    binding.package
                ),
            });
        }

        let detached = Self {
            bindings: Arc::new(RwLock::new(BTreeMap::from([(
                expected.skill_id().clone(),
                binding.clone(),
            )]))),
        };
        Ok(Some(detached))
    }

    /// Return canonical metadata for an exact, detached activation binding.
    pub(crate) fn sole_activation_binding_manifest(
        &self,
    ) -> Result<(SkillPackageUri, u64, Vec<SkillResourceKind>), SkillResourceError> {
        let bindings = self.read_bindings();
        if bindings.len() != 1 {
            return Err(SkillResourceError::InvalidRequest {
                reason: format!(
                    "activation binding snapshot contains {} packages; expected exactly one",
                    bindings.len()
                ),
            });
        }
        let binding = bindings
            .values()
            .next()
            .expect("one-element binding map must have one value");
        let mut kinds = binding
            .resources
            .entries()
            .iter()
            .map(SkillResourceDescriptor::kind)
            .collect::<Vec<_>>();
        kinds.sort_unstable_by_key(|kind| kind.stable_name());
        kinds.dedup();
        kinds.shrink_to_fit();
        Ok((
            binding.package.clone(),
            u64::try_from(binding.resources.len()).unwrap_or(u64::MAX),
            kinds,
        ))
    }

    pub fn list(
        &self,
        package: &SkillPackageUri,
        options: &SkillResourceListOptions,
    ) -> Result<SkillResourceListPage, SkillResourceError> {
        let binding = self.binding(package)?;
        let mut matches = binding.resources.entries().iter().filter(|entry| {
            let path = SkillResourcePath(entry.path().to_string());
            options
                .prefix()
                .is_none_or(|prefix| path.is_same_or_descendant_of(prefix))
                && options.kind().is_none_or(|kind| entry.kind() == kind)
                && options.after().is_none_or(|after| path > *after)
        });
        let entries = matches
            .by_ref()
            .take(options.limit())
            .map(|descriptor| {
                let path = SkillResourcePath(descriptor.path().to_string());
                SkillResourceListEntry {
                    uri: package.resource(path),
                    descriptor: descriptor.clone(),
                }
            })
            .collect::<Vec<_>>();
        let has_more = matches.next().is_some();
        let next_after = has_more
            .then(|| entries.last().map(|entry| entry.uri.path().clone()))
            .flatten();
        Ok(SkillResourceListPage {
            package: package.clone(),
            entries,
            next_after,
        })
    }

    pub fn read_text(
        &self,
        uri: &SkillResourceUri,
        options: SkillResourceTextReadOptions,
    ) -> Result<SkillResourceTextPage, SkillResourceError> {
        let snapshot = self.read_verified_bytes(uri)?;
        let total_bytes = usize::try_from(snapshot.descriptor.byte_length()).unwrap_or(usize::MAX);
        let bytes = snapshot.bytes;
        debug_assert_eq!(total_bytes, bytes.len());
        let text =
            std::str::from_utf8(&bytes).map_err(|_| SkillResourceError::ResourceNotText {
                uri: Box::new(uri.clone()),
            })?;
        let offset = options.offset();
        if offset > text.len() || !text.is_char_boundary(offset) {
            return Err(SkillResourceError::InvalidOffset {
                uri: Box::new(uri.clone()),
                offset,
            });
        }
        let mut end_offset = offset.saturating_add(options.max_bytes()).min(text.len());
        while end_offset > offset && !text.is_char_boundary(end_offset) {
            end_offset -= 1;
        }
        let page = text[offset..end_offset].to_string();
        Ok(SkillResourceTextPage {
            uri: uri.clone(),
            text: page,
            offset,
            end_offset,
            total_bytes,
        })
    }

    pub(crate) fn read_verified_bytes(
        &self,
        uri: &SkillResourceUri,
    ) -> Result<VerifiedSkillResourceSnapshot, SkillResourceError> {
        let binding = self.binding(uri.package())?;
        let descriptor = binding.resources.get(uri.path().as_str()).ok_or_else(|| {
            SkillResourceError::ResourceNotFound {
                uri: Box::new(uri.clone()),
            }
        })?;
        let reader =
            binding
                .reader
                .as_ref()
                .ok_or_else(|| SkillResourceError::SourceContractViolation {
                    source_id: binding.source_id.clone(),
                    reason: format!(
                        "Skill `{}` exposes a resource without a reader",
                        uri.package().skill_id()
                    ),
                })?;
        let bytes = reader.read(descriptor).map_err(|error| match error {
            SkillResourceSourceError::Integrity(reason) => SkillResourceError::IntegrityMismatch {
                uri: Box::new(uri.clone()),
                reason,
            },
            SkillResourceSourceError::Unavailable(reason) => {
                SkillResourceError::SnapshotUnavailable {
                    skill_id: uri.package().skill_id().clone(),
                    revision: uri.package().revision().clone(),
                    reason,
                }
            }
        })?;
        if descriptor.byte_length() != u64::try_from(bytes.len()).unwrap_or(u64::MAX)
            || descriptor.content_digest() != package_file_digest(&bytes)
        {
            return Err(SkillResourceError::IntegrityMismatch {
                uri: Box::new(uri.clone()),
                reason: "returned bytes do not match the activated descriptor".to_string(),
            });
        }
        Ok(VerifiedSkillResourceSnapshot {
            descriptor: descriptor.clone(),
            bytes,
        })
    }

    fn binding(&self, package: &SkillPackageUri) -> Result<SessionBinding, SkillResourceError> {
        let bindings = self.read_bindings();
        let binding = bindings.get(package.skill_id()).ok_or_else(|| {
            SkillResourceError::SkillNotActivated {
                skill_id: package.skill_id().clone(),
            }
        })?;
        if binding.package.revision() != package.revision() {
            return Err(SkillResourceError::RevisionNotActivated {
                skill_id: package.skill_id().clone(),
                requested_revision: package.revision().clone(),
                activated_revision: binding.package.revision().clone(),
            });
        }
        Ok(binding.clone())
    }

    fn read_bindings(&self) -> RwLockReadGuard<'_, BTreeMap<SkillId, SessionBinding>> {
        self.bindings
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn write_bindings(&self) -> RwLockWriteGuard<'_, BTreeMap<SkillId, SessionBinding>> {
        self.bindings
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl fmt::Debug for SkillResourceSession {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SkillResourceSession")
            .field("package_uris", &self.package_uris())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
struct TestMemoryResourceReader {
    resources: BTreeMap<String, Vec<u8>>,
}

#[cfg(test)]
impl SkillResourceReader for TestMemoryResourceReader {
    fn read(
        &self,
        expected: &SkillResourceDescriptor,
    ) -> Result<Vec<u8>, SkillResourceSourceError> {
        self.resources
            .get(expected.path())
            .cloned()
            .ok_or_else(|| SkillResourceSourceError::Unavailable("missing fixture".to_string()))
    }
}

/// Builds an in-memory, integrity-bound resource session for cross-module
/// runtime tests without widening the production reader/source boundary.
#[cfg(test)]
pub(crate) fn memory_resource_session_for_test(
    skill_id: SkillId,
    revision: SkillRevision,
    source_id: SkillSourceId,
    resources: Vec<(String, SkillResourceKind, Vec<u8>)>,
) -> Result<SkillResourceSession, SkillResourceError> {
    let mut descriptors = Vec::with_capacity(resources.len());
    let mut bytes_by_path = BTreeMap::new();
    for (path, kind, bytes) in resources {
        descriptors.push(SkillResourceDescriptor::new(
            path.clone(),
            kind,
            u64::try_from(bytes.len()).unwrap_or(u64::MAX),
            package_file_digest(&bytes),
        ));
        bytes_by_path.insert(path, bytes);
    }
    SkillResourceSession::from_bindings([SkillResourceSessionBinding {
        skill_id,
        revision,
        source_id,
        resources: SkillResourceIndex::new(descriptors),
        reader: Some(Arc::new(TestMemoryResourceReader {
            resources: bytes_by_path,
        })),
    }])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::model::{SkillResourceDescriptor, SkillResourceIndex};
    use std::collections::BTreeMap;

    struct MemoryReader {
        resources: BTreeMap<String, Vec<u8>>,
    }

    impl SkillResourceReader for MemoryReader {
        fn read(
            &self,
            expected: &SkillResourceDescriptor,
        ) -> Result<Vec<u8>, SkillResourceSourceError> {
            self.resources
                .get(expected.path())
                .cloned()
                .ok_or_else(|| SkillResourceSourceError::Unavailable("missing fixture".to_string()))
        }
    }

    fn identity() -> (SkillId, SkillRevision, SkillSourceId) {
        let source_id = SkillSourceId::parse("installed:user").unwrap();
        let skill_id =
            SkillId::parse("installed:user:01234567-89ab-4def-8123-456789abcdef").unwrap();
        let revision =
            SkillRevision::parse(format!("skill-package-sha256-v2:{}", "a".repeat(64))).unwrap();
        (skill_id, revision, source_id)
    }

    fn session() -> SkillResourceSession {
        let (skill_id, revision, source_id) = identity();
        let reference = b"alpha\n\xE4\xB8\xAD\xE6\x96\x87\nomega\n".to_vec();
        let asset = vec![0xff, 0x00];
        let descriptors = vec![
            SkillResourceDescriptor::new(
                "assets/icon.bin".to_string(),
                SkillResourceKind::Asset,
                asset.len() as u64,
                package_file_digest(&asset),
            ),
            SkillResourceDescriptor::new(
                "references/guide.md".to_string(),
                SkillResourceKind::Reference,
                reference.len() as u64,
                package_file_digest(&reference),
            ),
        ];
        let reader = Arc::new(MemoryReader {
            resources: BTreeMap::from([
                ("assets/icon.bin".to_string(), asset),
                ("references/guide.md".to_string(), reference),
            ]),
        });
        SkillResourceSession::from_bindings([SkillResourceSessionBinding {
            skill_id,
            revision,
            source_id,
            resources: SkillResourceIndex::new(descriptors),
            reader: Some(reader),
        }])
        .unwrap()
    }

    fn memory_session_with_identity(
        skill_id: SkillId,
        revision: SkillRevision,
        source_id: SkillSourceId,
    ) -> SkillResourceSession {
        memory_resource_session_for_test(
            skill_id,
            revision,
            source_id,
            vec![
                (
                    "assets/icon.bin".to_string(),
                    SkillResourceKind::Asset,
                    vec![0xff, 0x00],
                ),
                (
                    "references/guide.md".to_string(),
                    SkillResourceKind::Reference,
                    b"alpha\n\xE4\xB8\xAD\xE6\x96\x87\nomega\n".to_vec(),
                ),
            ],
        )
        .unwrap()
    }

    #[test]
    fn canonical_uris_roundtrip_and_reject_aliases() {
        let (skill_id, revision, _) = identity();
        let package = SkillPackageUri::new(skill_id, revision);
        assert!(package
            .as_str()
            .starts_with("skill://package/installed%3Auser%3A"));
        assert_eq!(SkillPackageUri::parse(package.as_str()).unwrap(), package);

        let resource = package.resource(SkillResourcePath::parse("references/指南.md").unwrap());
        assert!(resource
            .as_str()
            .ends_with("references/%E6%8C%87%E5%8D%97.md"));
        assert_eq!(
            SkillResourceUri::parse(resource.as_str()).unwrap(),
            resource
        );

        assert!(SkillResourceUri::parse(&resource.as_str().replace("%3A", "%3a")).is_err());
        assert!(SkillResourceUri::parse(
            &resource.as_str().replace("references/", "references%2F")
        )
        .is_err());
        assert!(SkillPackageUri::parse(package.as_str().trim_end_matches('/')).is_err());
        assert!(SkillResourceUri::parse(&format!("{}SKILL.md", package.as_str())).is_err());
        assert!(SkillResourceUri::parse(&format!("{}../secret", package.as_str())).is_err());
    }

    #[test]
    fn list_is_stable_filtered_and_cursor_paginated() {
        let session = session();
        let package = session.package_uris().remove(0);
        let first = session
            .list(&package, &SkillResourceListOptions::new(1).unwrap())
            .unwrap();
        assert_eq!(first.entries()[0].descriptor().path(), "assets/icon.bin");
        assert!(first.has_more());

        let second = session
            .list(
                &package,
                &SkillResourceListOptions::new(1)
                    .unwrap()
                    .with_after(first.next_after().unwrap().clone()),
            )
            .unwrap();
        assert_eq!(
            second.entries()[0].descriptor().path(),
            "references/guide.md"
        );
        assert!(!second.has_more());

        let references = session
            .list(
                &package,
                &SkillResourceListOptions::default()
                    .with_prefix(SkillResourcePath::parse("references").unwrap())
                    .with_kind(SkillResourceKind::Reference),
            )
            .unwrap();
        assert_eq!(references.entries().len(), 1);
    }

    #[test]
    fn text_reads_are_utf8_aligned_and_binary_resources_fail_explicitly() {
        let session = session();
        let package = session.package_uris().remove(0);
        let uri = package.resource(SkillResourcePath::parse("references/guide.md").unwrap());
        let first = session
            .read_text(&uri, SkillResourceTextReadOptions::new(0, 9).unwrap())
            .unwrap();
        assert_eq!(first.text(), "alpha\n中");
        assert_eq!(first.next_offset(), Some(9));
        let second = session
            .read_text(
                &uri,
                SkillResourceTextReadOptions::new(first.next_offset().unwrap(), 8).unwrap(),
            )
            .unwrap();
        assert!(second.text().starts_with('文'));

        let invalid = session
            .read_text(&uri, SkillResourceTextReadOptions::new(7, 8).unwrap())
            .unwrap_err();
        assert_eq!(invalid.code(), SkillResourceErrorCode::InvalidOffset);

        let binary = package.resource(SkillResourcePath::parse("assets/icon.bin").unwrap());
        let error = session
            .read_text(&binary, SkillResourceTextReadOptions::default())
            .unwrap_err();
        assert_eq!(error.code(), SkillResourceErrorCode::ResourceNotText);
    }

    #[test]
    fn session_rejects_unactivated_ids_and_revisions() {
        let session = session();
        let package = session.package_uris().remove(0);
        let other = SkillPackageUri::new(
            SkillId::parse("bundled:application:other").unwrap(),
            package.revision().clone(),
        );
        assert_eq!(
            session
                .list(&other, &SkillResourceListOptions::default())
                .unwrap_err()
                .code(),
            SkillResourceErrorCode::SkillNotActivated
        );

        let wrong_revision = SkillPackageUri::new(
            package.skill_id().clone(),
            SkillRevision::parse("different-revision").unwrap(),
        );
        assert_eq!(
            session
                .list(&wrong_revision, &SkillResourceListOptions::default())
                .unwrap_err()
                .code(),
            SkillResourceErrorCode::RevisionNotActivated
        );
    }

    #[test]
    fn empty_session_clones_share_subsequent_activations() {
        let empty = SkillResourceSession::empty();
        let clone = empty.clone();
        assert!(empty.is_empty());
        assert!(clone.is_empty());

        empty.extend_from(&session()).unwrap();

        assert_eq!(empty.len(), 1);
        assert_eq!(clone.len(), 1);
        assert_eq!(empty.package_uris(), clone.package_uris());
    }

    #[test]
    fn extending_with_the_same_exact_binding_is_idempotent() {
        let active_session = session();
        let exact_duplicate = session();
        let packages_before = active_session.package_uris();

        active_session.extend_from(&exact_duplicate).unwrap();
        active_session.extend_from(&exact_duplicate).unwrap();

        assert_eq!(active_session.len(), 1);
        assert_eq!(active_session.package_uris(), packages_before);
    }

    #[test]
    fn conflicting_revision_rejects_the_whole_extension_atomically() {
        let session = session();
        let (skill_id, activated_revision, source_id) = identity();
        let requested_revision =
            SkillRevision::parse(format!("skill-package-sha256-v2:{}", "b".repeat(64))).unwrap();
        let candidate = memory_session_with_identity(
            skill_id.clone(),
            requested_revision.clone(),
            source_id.clone(),
        );
        let new_skill_id =
            SkillId::parse("installed:user:00000000-0000-4000-8000-000000000001").unwrap();
        let new_binding =
            memory_session_with_identity(new_skill_id, activated_revision.clone(), source_id);
        let new_package = new_binding.package_uris().remove(0);
        candidate.extend_from(&new_binding).unwrap();
        assert_eq!(candidate.len(), 2);

        let error = session.extend_from(&candidate).unwrap_err();

        assert!(matches!(
            error,
            SkillResourceError::RevisionNotActivated {
                skill_id: rejected_skill_id,
                requested_revision: rejected_revision,
                activated_revision: current_revision,
            } if rejected_skill_id == skill_id
                && rejected_revision == requested_revision
                && current_revision == activated_revision
        ));
        assert_eq!(session.len(), 1);
        assert_eq!(
            session
                .list(&new_package, &SkillResourceListOptions::default())
                .unwrap_err()
                .code(),
            SkillResourceErrorCode::SkillNotActivated
        );
    }

    #[test]
    fn preexisting_clone_reads_resources_immediately_after_activation() {
        let empty = SkillResourceSession::empty();
        let runtime_clone = empty.clone();
        let activated = session();
        let package = activated.package_uris().remove(0);
        let uri = package.resource(SkillResourcePath::parse("references/guide.md").unwrap());

        assert_eq!(
            runtime_clone
                .read_text(&uri, SkillResourceTextReadOptions::default())
                .unwrap_err()
                .code(),
            SkillResourceErrorCode::SkillNotActivated
        );

        empty.extend_from(&activated).unwrap();

        let page = runtime_clone
            .read_text(&uri, SkillResourceTextReadOptions::default())
            .unwrap();
        assert_eq!(page.text(), "alpha\n中文\nomega\n");
    }
}
