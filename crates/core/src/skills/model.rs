use std::error::Error;
use std::fmt;
use std::ops::Range;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use uuid::Uuid;

pub const SKILL_PACKAGE_FORMAT_VERSION: u32 = 1;
pub const SKILL_PACKAGE_FORMAT_VERSION_V2: u32 = 2;
pub const SKILL_PACKAGE_FORMAT_VERSION_V3: u32 = 3;
pub const DEFAULT_MAX_ACTIVATED_SKILLS: usize = 8;
pub const DEFAULT_MAX_ACTIVATED_SKILL_BYTES: usize = 512 * 1024;

const MAX_OPAQUE_ID_BYTES: usize = 16 * 1024;
const MAX_LOCAL_SKILL_ID_BYTES: usize = 4 * 1024;
const MAX_SOURCE_ID_BYTES: usize = MAX_OPAQUE_ID_BYTES - MAX_LOCAL_SKILL_ID_BYTES - 1;
const MAX_REVISION_BYTES: usize = 256;

pub const SKILL_INSTALLATION_REVISION_PREFIX: &str = "skill-installation-sha256-v1:";

/// Stable identity of one configured Skill source.
///
/// The value is opaque to callers. Its final `:`-separated component is part
/// of the source identity, while a [`SkillId`] appends one local component.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SkillSourceId(String);

impl SkillSourceId {
    pub fn parse(value: impl Into<String>) -> Result<Self, SkillReferenceError> {
        let value = value.into();
        validate_opaque_ascii(&value, "source id", MAX_SOURCE_ID_BYTES)?;
        let Some((kind, authority)) = value.split_once(':') else {
            return Err(SkillReferenceError::new(
                "source id must contain a non-empty kind and authority",
            ));
        };
        if kind.is_empty() || authority.is_empty() || authority.split(':').any(str::is_empty) {
            return Err(SkillReferenceError::new(
                "source id must contain non-empty colon-separated kind and authority components",
            ));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SkillSourceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("SkillSourceId")
            .field(&self.0)
            .finish()
    }
}

impl fmt::Display for SkillSourceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for SkillSourceId {
    type Err = SkillReferenceError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

/// Stable application-generated identity of one managed Skill installation.
///
/// The identity is deliberately independent from the package revision so an
/// installation can move to a new immutable package without changing its
/// source-qualified [`SkillId`].
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SkillInstallationId(String);

impl SkillInstallationId {
    /// Generates a stable id for an installation request. Callers should keep
    /// and retry the same request if commit acknowledgement is uncertain.
    pub fn new() -> Self {
        Self(Uuid::new_v4().hyphenated().to_string())
    }

    pub fn parse(value: impl Into<String>) -> Result<Self, SkillReferenceError> {
        let value = value.into();
        let uuid = Uuid::parse_str(&value)
            .map_err(|_| SkillReferenceError::new("installation id must be a UUID"))?;
        if uuid.is_nil() {
            return Err(SkillReferenceError::new(
                "installation id must not be the nil UUID",
            ));
        }
        if uuid.hyphenated().to_string() != value {
            return Err(SkillReferenceError::new(
                "installation id must use canonical lowercase hyphenated UUID form",
            ));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for SkillInstallationId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for SkillInstallationId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("SkillInstallationId")
            .field(&self.0)
            .finish()
    }
}

impl fmt::Display for SkillInstallationId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for SkillInstallationId {
    type Err = SkillReferenceError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

/// Opaque, source-qualified Skill identity.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SkillId {
    value: String,
    source_id: SkillSourceId,
}

impl SkillId {
    pub fn parse(value: impl Into<String>) -> Result<Self, SkillReferenceError> {
        let value = value.into();
        validate_opaque_ascii(&value, "skill id", MAX_OPAQUE_ID_BYTES)?;
        let (source, local_id) = value
            .rsplit_once(':')
            .ok_or_else(|| SkillReferenceError::new("skill id is missing its local component"))?;
        if local_id.is_empty() || local_id.contains(':') {
            return Err(SkillReferenceError::new(
                "skill id must have one non-empty local component",
            ));
        }
        validate_opaque_ascii(local_id, "local skill id", MAX_LOCAL_SKILL_ID_BYTES)?;
        let source_id = SkillSourceId::parse(source)?;
        Ok(Self { value, source_id })
    }

    pub(super) fn from_parts(
        source_id: SkillSourceId,
        local_id: &str,
    ) -> Result<Self, SkillReferenceError> {
        validate_opaque_ascii(local_id, "local skill id", MAX_LOCAL_SKILL_ID_BYTES)?;
        if local_id.contains(':') {
            return Err(SkillReferenceError::new(
                "local skill id cannot contain a colon",
            ));
        }
        Self::parse(format!("{source_id}:{local_id}"))
    }

    pub fn as_str(&self) -> &str {
        &self.value
    }

    pub fn source_id(&self) -> &SkillSourceId {
        &self.source_id
    }

    pub(super) fn local_id(&self) -> &str {
        self.value
            .rsplit_once(':')
            .map_or("", |(_, local_id)| local_id)
    }
}

impl fmt::Debug for SkillId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_tuple("SkillId").field(&self.value).finish()
    }
}

impl fmt::Display for SkillId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.value.fmt(formatter)
    }
}

impl FromStr for SkillId {
    type Err = SkillReferenceError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl PartialEq<str> for SkillId {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl PartialEq<String> for SkillId {
    fn eq(&self, other: &String) -> bool {
        self.as_str() == other
    }
}

/// Revision token binding a selection to one exact Skill package snapshot.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SkillRevision(String);

impl SkillRevision {
    pub fn parse(value: impl Into<String>) -> Result<Self, SkillReferenceError> {
        let value = value.into();
        validate_opaque_ascii(&value, "skill revision", MAX_REVISION_BYTES)?;
        Ok(Self(value))
    }

    pub(super) fn trusted(value: String) -> Self {
        debug_assert!(Self::parse(value.clone()).is_ok());
        Self(value)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SkillRevision {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("SkillRevision")
            .field(&self.0)
            .finish()
    }
}

impl fmt::Display for SkillRevision {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for SkillRevision {
    type Err = SkillReferenceError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl PartialEq<str> for SkillRevision {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl PartialEq<String> for SkillRevision {
    fn eq(&self, other: &String) -> bool {
        self.as_str() == other
    }
}

/// Compare-and-swap identity for one exact managed installation receipt.
///
/// Unlike [`SkillRevision`], this token binds installation lifecycle state,
/// including its monotonically increasing generation and acquisition
/// provenance. It therefore changes even when an update retains identical
/// package bytes but changes where future refreshes are resolved from.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SkillInstallationRevision(String);

impl SkillInstallationRevision {
    pub fn parse(value: impl Into<String>) -> Result<Self, SkillReferenceError> {
        let value = value.into();
        let Some(digest) = value.strip_prefix(SKILL_INSTALLATION_REVISION_PREFIX) else {
            return Err(SkillReferenceError::new(format!(
                "installation revision must start with `{SKILL_INSTALLATION_REVISION_PREFIX}`"
            )));
        };
        if digest.len() != 64
            || !digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        {
            return Err(SkillReferenceError::new(
                "installation revision digest must contain exactly 64 lowercase hexadecimal characters",
            ));
        }
        Ok(Self(value))
    }

    pub(super) fn trusted(value: String) -> Self {
        debug_assert!(Self::parse(value.clone()).is_ok());
        Self(value)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SkillInstallationRevision {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("SkillInstallationRevision")
            .field(&self.0)
            .finish()
    }
}

impl fmt::Display for SkillInstallationRevision {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for SkillInstallationRevision {
    type Err = SkillReferenceError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl PartialEq<str> for SkillInstallationRevision {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl PartialEq<String> for SkillInstallationRevision {
    fn eq(&self, other: &String) -> bool {
        self.as_str() == other
    }
}

/// A user or model selection captured from a catalog.
///
/// Both fields are validated and immutable, so a catalog descriptor can always
/// be converted to a resolver input without a second, weaker string contract.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SkillSelection {
    skill_id: SkillId,
    expected_revision: SkillRevision,
}

impl SkillSelection {
    pub fn new(skill_id: SkillId, expected_revision: SkillRevision) -> Self {
        Self {
            skill_id,
            expected_revision,
        }
    }

    pub fn parse(
        skill_id: impl Into<String>,
        expected_revision: impl Into<String>,
    ) -> Result<Self, SkillReferenceError> {
        Ok(Self::new(
            SkillId::parse(skill_id)?,
            SkillRevision::parse(expected_revision)?,
        ))
    }

    pub fn from_descriptor(descriptor: &SkillDescriptor) -> Self {
        Self::new(descriptor.id().clone(), descriptor.revision().clone())
    }

    pub fn skill_id(&self) -> &SkillId {
        &self.skill_id
    }

    pub fn expected_revision(&self) -> &SkillRevision {
        &self.expected_revision
    }
}

/// Compatibility name for the v1 resolver request.
pub type SkillResolveRequest = SkillSelection;

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct SkillReferenceError {
    reason: String,
}

impl SkillReferenceError {
    pub(super) fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }

    pub fn reason(&self) -> &str {
        &self.reason
    }
}

impl fmt::Display for SkillReferenceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.reason.fmt(formatter)
    }
}

impl Error for SkillReferenceError {}

fn validate_opaque_ascii(
    value: &str,
    label: &str,
    max_bytes: usize,
) -> Result<(), SkillReferenceError> {
    if value.is_empty() {
        return Err(SkillReferenceError::new(format!(
            "{label} must not be empty"
        )));
    }
    if value.len() > max_bytes {
        return Err(SkillReferenceError::new(format!(
            "{label} exceeds {max_bytes} bytes"
        )));
    }
    if !value.bytes().all(|byte| byte.is_ascii_graphic()) {
        return Err(SkillReferenceError::new(format!(
            "{label} must contain only printable ASCII without whitespace"
        )));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SkillSourceKind {
    Workspace,
    Bundled,
    Installed,
}

impl SkillSourceKind {
    pub fn stable_name(self) -> &'static str {
        match self {
            Self::Workspace => "workspace",
            Self::Bundled => "bundled",
            Self::Installed => "installed",
        }
    }
}

/// Trust is an input to policy, not an authorization grant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SkillTrust {
    Untrusted,
    UserApproved,
    Application,
}

impl SkillTrust {
    pub fn stable_name(self) -> &'static str {
        match self {
            Self::Untrusted => "untrusted",
            Self::UserApproved => "userApproved",
            Self::Application => "application",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SkillActivationScope {
    Run,
}

impl SkillActivationScope {
    pub fn stable_name(self) -> &'static str {
        match self {
            Self::Run => "run",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SkillProvenance {
    Workspace {
        workspace_id: String,
        relative_path: String,
    },
    Bundled {
        source_id: SkillSourceId,
        relative_path: String,
    },
    Installed {
        source_id: SkillSourceId,
        installation_id: SkillInstallationId,
        relative_path: String,
    },
    Other {
        source_id: SkillSourceId,
        display_location: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillMetadata {
    name: String,
    description: String,
}

impl SkillMetadata {
    pub(super) fn new(name: String, description: String) -> Self {
        Self { name, description }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn description(&self) -> &str {
        &self.description
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SkillDefinition {
    id: SkillId,
    metadata: SkillMetadata,
    source_kind: SkillSourceKind,
    trust: SkillTrust,
    activation_scope: SkillActivationScope,
    revision: SkillRevision,
    provenance: SkillProvenance,
}

/// Immutable catalog view of a Skill package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillDescriptor(Arc<SkillDefinition>);

pub(super) struct SkillDescriptorParts {
    pub id: SkillId,
    pub name: String,
    pub description: String,
    pub source_kind: SkillSourceKind,
    pub trust: SkillTrust,
    pub activation_scope: SkillActivationScope,
    pub revision: SkillRevision,
    pub provenance: SkillProvenance,
}

impl SkillDescriptor {
    pub(super) fn new(parts: SkillDescriptorParts) -> Self {
        Self(Arc::new(SkillDefinition {
            id: parts.id,
            metadata: SkillMetadata::new(parts.name, parts.description),
            source_kind: parts.source_kind,
            trust: parts.trust,
            activation_scope: parts.activation_scope,
            revision: parts.revision,
            provenance: parts.provenance,
        }))
    }

    pub fn id(&self) -> &SkillId {
        &self.0.id
    }

    pub fn metadata(&self) -> &SkillMetadata {
        &self.0.metadata
    }

    pub fn name(&self) -> &str {
        self.metadata().name()
    }

    pub fn description(&self) -> &str {
        self.metadata().description()
    }

    pub fn source_kind(&self) -> SkillSourceKind {
        self.0.source_kind
    }

    pub fn trust(&self) -> SkillTrust {
        self.0.trust
    }

    pub fn activation_scope(&self) -> SkillActivationScope {
        self.0.activation_scope
    }

    pub fn revision(&self) -> &SkillRevision {
        &self.0.revision
    }

    pub fn provenance(&self) -> &SkillProvenance {
        &self.0.provenance
    }

    pub fn selection(&self) -> SkillSelection {
        SkillSelection::from_descriptor(self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum SkillResourceKind {
    Reference,
    Asset,
    Script,
    /// A revision-bound package file outside the conventional
    /// `references/`, `assets/`, and `scripts/` trees.
    Other,
}

impl SkillResourceKind {
    pub fn stable_name(self) -> &'static str {
        match self {
            Self::Reference => "reference",
            Self::Asset => "asset",
            Self::Script => "script",
            Self::Other => "other",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SkillResourceDescriptor {
    path: String,
    kind: SkillResourceKind,
    byte_length: u64,
    content_digest: String,
}

impl SkillResourceDescriptor {
    pub(super) fn new(
        path: String,
        kind: SkillResourceKind,
        byte_length: u64,
        content_digest: String,
    ) -> Self {
        Self {
            path,
            kind,
            byte_length,
            content_digest,
        }
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn kind(&self) -> SkillResourceKind {
        self.kind
    }

    pub fn byte_length(&self) -> u64 {
        self.byte_length
    }

    pub fn content_digest(&self) -> &str {
        &self.content_digest
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SkillResourceIndex {
    entries: Arc<[SkillResourceDescriptor]>,
}

impl SkillResourceIndex {
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn entries(&self) -> &[SkillResourceDescriptor] {
        &self.entries
    }

    pub fn get(&self, path: &str) -> Option<&SkillResourceDescriptor> {
        self.entries
            .binary_search_by(|entry| entry.path().cmp(path))
            .ok()
            .map(|index| &self.entries[index])
    }

    pub(super) fn new(entries: Vec<SkillResourceDescriptor>) -> Self {
        Self {
            entries: entries.into(),
        }
    }
}

/// Immutable, revision-bound package returned by a Skill source.
#[derive(Clone, PartialEq, Eq)]
pub struct ResolvedSkillPackage {
    descriptor: SkillDescriptor,
    format_version: u32,
    resources: SkillResourceIndex,
    source_text: Arc<str>,
    instructions_range: Range<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SkillPackageInvariantError {
    reason: String,
}

impl SkillPackageInvariantError {
    fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }
}

impl fmt::Display for SkillPackageInvariantError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.reason.fmt(formatter)
    }
}

impl Error for SkillPackageInvariantError {}

impl ResolvedSkillPackage {
    pub(super) fn new(
        descriptor: SkillDescriptor,
        source_text: Arc<str>,
        instructions_range: Range<usize>,
    ) -> Result<Self, SkillPackageInvariantError> {
        Self::with_resources(
            descriptor,
            SKILL_PACKAGE_FORMAT_VERSION,
            SkillResourceIndex::default(),
            source_text,
            instructions_range,
        )
    }

    pub(super) fn with_resources(
        descriptor: SkillDescriptor,
        format_version: u32,
        resources: SkillResourceIndex,
        source_text: Arc<str>,
        instructions_range: Range<usize>,
    ) -> Result<Self, SkillPackageInvariantError> {
        if instructions_range.start > instructions_range.end {
            return Err(SkillPackageInvariantError::new(
                "Skill instruction range starts after it ends",
            ));
        }
        if instructions_range.end > source_text.len() {
            return Err(SkillPackageInvariantError::new(
                "Skill instruction range exceeds the source snapshot",
            ));
        }
        if !source_text.is_char_boundary(instructions_range.start)
            || !source_text.is_char_boundary(instructions_range.end)
        {
            return Err(SkillPackageInvariantError::new(
                "Skill instruction range is not aligned to UTF-8 boundaries",
            ));
        }
        if format_version == SKILL_PACKAGE_FORMAT_VERSION && !resources.is_empty() {
            return Err(SkillPackageInvariantError::new(
                "Skill package format v1 cannot expose sibling resources",
            ));
        }
        if !matches!(
            format_version,
            SKILL_PACKAGE_FORMAT_VERSION
                | SKILL_PACKAGE_FORMAT_VERSION_V2
                | SKILL_PACKAGE_FORMAT_VERSION_V3
        ) {
            return Err(SkillPackageInvariantError::new(format!(
                "unsupported Skill package format version {format_version}"
            )));
        }
        let contains_other = resources
            .entries()
            .iter()
            .any(|resource| resource.kind() == SkillResourceKind::Other);
        if format_version == SKILL_PACKAGE_FORMAT_VERSION_V2
            && (resources.is_empty() || contains_other)
        {
            return Err(SkillPackageInvariantError::new(
                "Skill package format v2 requires only conventional sibling resources",
            ));
        }
        if format_version == SKILL_PACKAGE_FORMAT_VERSION_V3 && !contains_other {
            return Err(SkillPackageInvariantError::new(
                "Skill package format v3 requires at least one generic sibling resource",
            ));
        }
        let mut previous = None;
        for resource in resources.entries() {
            if resource.path().is_empty() {
                return Err(SkillPackageInvariantError::new(
                    "Skill resource path cannot be empty",
                ));
            }
            if previous.is_some_and(|path: &str| path >= resource.path()) {
                return Err(SkillPackageInvariantError::new(
                    "Skill resources must use unique canonical sorted paths",
                ));
            }
            previous = Some(resource.path());
        }

        Ok(Self {
            descriptor,
            format_version,
            resources,
            source_text,
            instructions_range,
        })
    }

    pub fn descriptor(&self) -> &SkillDescriptor {
        &self.descriptor
    }

    pub fn format_version(&self) -> u32 {
        self.format_version
    }

    pub fn resources(&self) -> &SkillResourceIndex {
        &self.resources
    }

    pub fn source_text(&self) -> &str {
        &self.source_text
    }

    pub fn instructions(&self) -> &str {
        &self.source_text[self.instructions_range.clone()]
    }

    pub(super) fn instructions_range(&self) -> &Range<usize> {
        &self.instructions_range
    }

    pub fn id(&self) -> &SkillId {
        self.descriptor.id()
    }

    pub fn name(&self) -> &str {
        self.descriptor.name()
    }

    pub fn description(&self) -> &str {
        self.descriptor.description()
    }

    pub fn source_kind(&self) -> SkillSourceKind {
        self.descriptor.source_kind()
    }

    pub fn trust(&self) -> SkillTrust {
        self.descriptor.trust()
    }

    pub fn activation_scope(&self) -> SkillActivationScope {
        self.descriptor.activation_scope()
    }

    pub fn revision(&self) -> &SkillRevision {
        self.descriptor.revision()
    }

    pub fn provenance(&self) -> &SkillProvenance {
        self.descriptor.provenance()
    }
}

impl fmt::Debug for ResolvedSkillPackage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ResolvedSkillPackage")
            .field("descriptor", &self.descriptor)
            .field("format_version", &self.format_version)
            .field("resources", &self.resources)
            .field("source_bytes", &self.source_text.len())
            .field("instructions_range", &self.instructions_range)
            .finish()
    }
}

pub type ResolvedSkill = ResolvedSkillPackage;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillCatalog {
    catalog_revision: String,
    skills: Vec<SkillDescriptor>,
    diagnostics: Vec<SkillDiagnostic>,
    truncated: bool,
}

impl SkillCatalog {
    pub(super) fn new(
        catalog_revision: String,
        skills: Vec<SkillDescriptor>,
        diagnostics: Vec<SkillDiagnostic>,
        truncated: bool,
    ) -> Self {
        Self {
            catalog_revision,
            skills,
            diagnostics,
            truncated,
        }
    }

    pub fn catalog_revision(&self) -> &str {
        &self.catalog_revision
    }

    pub fn skills(&self) -> &[SkillDescriptor] {
        &self.skills
    }

    pub fn diagnostics(&self) -> &[SkillDiagnostic] {
        &self.diagnostics
    }

    pub fn truncated(&self) -> bool {
        self.truncated
    }

    pub(super) fn into_parts(self) -> (Vec<SkillDescriptor>, Vec<SkillDiagnostic>, bool) {
        (self.skills, self.diagnostics, self.truncated)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillDiagnostic {
    code: SkillDiagnosticCode,
    severity: SkillDiagnosticSeverity,
    message: String,
    path: String,
}

impl SkillDiagnostic {
    pub(super) fn new(
        code: SkillDiagnosticCode,
        severity: SkillDiagnosticSeverity,
        message: String,
        path: String,
    ) -> Self {
        Self {
            code,
            severity,
            message,
            path,
        }
    }

    pub fn code(&self) -> SkillDiagnosticCode {
        self.code
    }

    pub fn severity(&self) -> SkillDiagnosticSeverity {
        self.severity
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn path(&self) -> &str {
        &self.path
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SkillDiagnosticSeverity {
    Warning,
    Error,
}

impl SkillDiagnosticSeverity {
    pub fn stable_name(self) -> &'static str {
        match self {
            Self::Warning => "warning",
            Self::Error => "error",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SkillDiagnosticCode {
    InvalidRoot,
    RootEscapesWorkspace,
    TooManyEntries,
    ScanBudgetExceeded,
    CatalogTooLarge,
    UnreadableEntry,
    UnsupportedPathEncoding,
    SymlinkNotAllowed,
    PathChangedDuringRead,
    MissingSkillFile,
    SkillFileTooLarge,
    InvalidUtf8,
    NulByte,
    MissingFrontmatter,
    InvalidFrontmatter,
    MissingDescription,
    InvalidName,
    InvalidDescription,
    InvalidDirectoryName,
    MissingInstructions,
    DefaultedName,
    DuplicateName,
    SourceUnavailable,
    SourceContractViolation,
    InvalidInstallationReceipt,
    PackageRevisionMismatch,
    UnexpectedPackageEntry,
    InvalidResourcePath,
    ResourceFileTooLarge,
    PackageTooLarge,
    InvalidPackageManifest,
}

impl SkillDiagnosticCode {
    pub fn stable_name(self) -> &'static str {
        match self {
            Self::InvalidRoot => "invalidRoot",
            Self::RootEscapesWorkspace => "rootEscapesWorkspace",
            Self::TooManyEntries => "tooManyEntries",
            Self::ScanBudgetExceeded => "scanBudgetExceeded",
            Self::CatalogTooLarge => "catalogTooLarge",
            Self::UnreadableEntry => "unreadableEntry",
            Self::UnsupportedPathEncoding => "unsupportedPathEncoding",
            Self::SymlinkNotAllowed => "symlinkNotAllowed",
            Self::PathChangedDuringRead => "pathChangedDuringRead",
            Self::MissingSkillFile => "missingSkillFile",
            Self::SkillFileTooLarge => "skillFileTooLarge",
            Self::InvalidUtf8 => "invalidUtf8",
            Self::NulByte => "nulByte",
            Self::MissingFrontmatter => "missingFrontmatter",
            Self::InvalidFrontmatter => "invalidFrontmatter",
            Self::MissingDescription => "missingDescription",
            Self::InvalidName => "invalidName",
            Self::InvalidDescription => "invalidDescription",
            Self::InvalidDirectoryName => "invalidDirectoryName",
            Self::MissingInstructions => "missingInstructions",
            Self::DefaultedName => "defaultedName",
            Self::DuplicateName => "duplicateName",
            Self::SourceUnavailable => "sourceUnavailable",
            Self::SourceContractViolation => "sourceContractViolation",
            Self::InvalidInstallationReceipt => "invalidInstallationReceipt",
            Self::PackageRevisionMismatch => "packageRevisionMismatch",
            Self::UnexpectedPackageEntry => "unexpectedPackageEntry",
            Self::InvalidResourcePath => "invalidResourcePath",
            Self::ResourceFileTooLarge => "resourceFileTooLarge",
            Self::PackageTooLarge => "packageTooLarge",
            Self::InvalidPackageManifest => "invalidPackageManifest",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SkillRecovery {
    ChangeSelection,
    RefreshCatalog,
    Retry,
    RepairSkill,
    ReconfigureSource,
    ReduceSelection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SkillErrorCode {
    InvalidReference,
    InvalidSource,
    DuplicateSource,
    SourceNotRegistered,
    WorkspaceUnavailable,
    WorkspaceNotDirectory,
    NotFound,
    Stale,
    InvalidSkill,
    Unavailable,
    TooManySkills,
    DuplicateSelection,
    ResolveFailed,
    SourceBudgetExceeded,
}

impl SkillErrorCode {
    pub fn stable_name(self) -> &'static str {
        match self {
            Self::InvalidReference => "invalidReference",
            Self::InvalidSource => "invalidSource",
            Self::DuplicateSource => "duplicateSource",
            Self::SourceNotRegistered => "sourceNotRegistered",
            Self::WorkspaceUnavailable => "workspaceUnavailable",
            Self::WorkspaceNotDirectory => "workspaceNotDirectory",
            Self::NotFound => "notFound",
            Self::Stale => "stale",
            Self::InvalidSkill => "invalidSkill",
            Self::Unavailable => "unavailable",
            Self::TooManySkills => "tooManySkills",
            Self::DuplicateSelection => "duplicateSelection",
            Self::ResolveFailed => "resolveFailed",
            Self::SourceBudgetExceeded => "sourceBudgetExceeded",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SkillResolveError {
    InvalidReference {
        reason: String,
    },
    SourceNotRegistered {
        source_id: SkillSourceId,
    },
    Workspace(SkillDiscoveryError),
    NotFound {
        skill_id: SkillId,
    },
    Stale {
        skill_id: SkillId,
        expected_revision: SkillRevision,
        actual_revision: SkillRevision,
    },
    InvalidSkill {
        skill_id: SkillId,
        code: SkillDiagnosticCode,
        reason: String,
    },
    Unavailable {
        skill_id: Option<SkillId>,
        reason: String,
    },
    SourceContractViolation {
        source_id: SkillSourceId,
        skill_id: Option<SkillId>,
        reason: String,
    },
}

impl SkillResolveError {
    pub fn code(&self) -> SkillErrorCode {
        match self {
            Self::InvalidReference { .. } => SkillErrorCode::InvalidReference,
            Self::SourceNotRegistered { .. } => SkillErrorCode::SourceNotRegistered,
            Self::Workspace(error) => error.code(),
            Self::NotFound { .. } => SkillErrorCode::NotFound,
            Self::Stale { .. } => SkillErrorCode::Stale,
            Self::InvalidSkill { .. } => SkillErrorCode::InvalidSkill,
            Self::Unavailable { .. } => SkillErrorCode::Unavailable,
            Self::SourceContractViolation { .. } => SkillErrorCode::ResolveFailed,
        }
    }

    pub fn recovery(&self) -> SkillRecovery {
        match self {
            Self::InvalidReference { .. } => SkillRecovery::ChangeSelection,
            Self::SourceNotRegistered { .. } => SkillRecovery::ReconfigureSource,
            Self::Workspace(error) => error.recovery(),
            Self::Unavailable { .. } => SkillRecovery::Retry,
            Self::NotFound { .. } | Self::Stale { .. } => SkillRecovery::RefreshCatalog,
            Self::InvalidSkill { .. } => SkillRecovery::RepairSkill,
            Self::SourceContractViolation { .. } => SkillRecovery::ReconfigureSource,
        }
    }

    pub fn is_stale(&self) -> bool {
        matches!(self, Self::Stale { .. })
    }

    pub fn is_not_found(&self) -> bool {
        matches!(self, Self::NotFound { .. })
    }

    pub fn is_retryable(&self) -> bool {
        self.recovery() == SkillRecovery::Retry
    }

    pub fn skill_id(&self) -> Option<&SkillId> {
        match self {
            Self::NotFound { skill_id }
            | Self::Stale { skill_id, .. }
            | Self::InvalidSkill { skill_id, .. } => Some(skill_id),
            Self::Unavailable { skill_id, .. } => skill_id.as_ref(),
            Self::SourceContractViolation { skill_id, .. } => skill_id.as_ref(),
            _ => None,
        }
    }

    pub fn source_id(&self) -> Option<&SkillSourceId> {
        match self {
            Self::SourceNotRegistered { source_id } => Some(source_id),
            Self::SourceContractViolation { source_id, .. } => Some(source_id),
            _ => self.skill_id().map(SkillId::source_id),
        }
    }

    pub fn diagnostic_code(&self) -> Option<SkillDiagnosticCode> {
        match self {
            Self::InvalidSkill { code, .. } => Some(*code),
            _ => None,
        }
    }

    pub fn expected_revision(&self) -> Option<&SkillRevision> {
        match self {
            Self::Stale {
                expected_revision, ..
            } => Some(expected_revision),
            _ => None,
        }
    }

    pub fn actual_revision(&self) -> Option<&SkillRevision> {
        match self {
            Self::Stale {
                actual_revision, ..
            } => Some(actual_revision),
            _ => None,
        }
    }

    pub fn message(&self) -> String {
        self.to_string()
    }
}

impl fmt::Display for SkillResolveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidReference { reason } => {
                write!(formatter, "Invalid Skill reference: {reason}")
            }
            Self::SourceNotRegistered { source_id } => {
                write!(formatter, "Skill source `{source_id}` is not registered")
            }
            Self::Workspace(error) => error.fmt(formatter),
            Self::NotFound { skill_id } => {
                write!(formatter, "Skill `{skill_id}` is no longer available")
            }
            Self::Stale {
                skill_id,
                expected_revision,
                actual_revision,
            } => write!(
                formatter,
                "Skill `{skill_id}` changed after selection (expected {expected_revision}, found {actual_revision})"
            ),
            Self::InvalidSkill {
                skill_id,
                code,
                reason,
            } => write!(
                formatter,
                "Skill `{skill_id}` is invalid ({}): {reason}",
                code.stable_name()
            ),
            Self::Unavailable { skill_id, reason } => match skill_id {
                Some(skill_id) => {
                    write!(formatter, "Skill `{skill_id}` is temporarily unavailable: {reason}")
                }
                None => write!(formatter, "Skill source is temporarily unavailable: {reason}"),
            },
            Self::SourceContractViolation {
                source_id, reason, ..
            } => write!(
                formatter,
                "Skill source `{source_id}` violated the resolver contract: {reason}"
            ),
        }
    }
}

impl Error for SkillResolveError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Workspace(error) => Some(error),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SkillDiscoveryError {
    InvalidSource { reason: String },
    WorkspaceUnavailable { path: PathBuf, reason: String },
    WorkspaceNotDirectory { path: PathBuf },
}

impl SkillDiscoveryError {
    pub fn code(&self) -> SkillErrorCode {
        match self {
            Self::InvalidSource { .. } => SkillErrorCode::InvalidSource,
            Self::WorkspaceUnavailable { .. } => SkillErrorCode::WorkspaceUnavailable,
            Self::WorkspaceNotDirectory { .. } => SkillErrorCode::WorkspaceNotDirectory,
        }
    }

    pub fn recovery(&self) -> SkillRecovery {
        match self {
            Self::InvalidSource { .. } => SkillRecovery::ReconfigureSource,
            Self::WorkspaceUnavailable { .. } => SkillRecovery::Retry,
            Self::WorkspaceNotDirectory { .. } => SkillRecovery::ReconfigureSource,
        }
    }

    pub fn message(&self) -> String {
        self.to_string()
    }
}

impl fmt::Display for SkillDiscoveryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSource { reason } => write!(formatter, "Invalid Skill source: {reason}"),
            Self::WorkspaceUnavailable { path, reason } => write!(
                formatter,
                "Cannot access workspace {}: {reason}",
                path.display()
            ),
            Self::WorkspaceNotDirectory { path } => {
                write!(
                    formatter,
                    "Workspace is not a directory: {}",
                    path.display()
                )
            }
        }
    }
}

impl Error for SkillDiscoveryError {}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SkillRegistrationError {
    InvalidSource { reason: String },
    DuplicateSource { source_id: SkillSourceId },
}

impl SkillRegistrationError {
    pub fn code(&self) -> SkillErrorCode {
        match self {
            Self::InvalidSource { .. } => SkillErrorCode::InvalidSource,
            Self::DuplicateSource { .. } => SkillErrorCode::DuplicateSource,
        }
    }

    pub fn recovery(&self) -> SkillRecovery {
        SkillRecovery::ReconfigureSource
    }

    pub fn message(&self) -> String {
        self.to_string()
    }

    pub fn source_id(&self) -> Option<&SkillSourceId> {
        match self {
            Self::DuplicateSource { source_id } => Some(source_id),
            Self::InvalidSource { .. } => None,
        }
    }
}

impl fmt::Display for SkillRegistrationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSource { reason } => write!(formatter, "Invalid Skill source: {reason}"),
            Self::DuplicateSource { source_id } => {
                write!(
                    formatter,
                    "Skill source `{source_id}` is already registered"
                )
            }
        }
    }
}

impl Error for SkillRegistrationError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SkillActivationPolicy {
    max_skills: usize,
    max_total_source_bytes: usize,
}

impl SkillActivationPolicy {
    pub const fn new(max_skills: usize, max_total_source_bytes: usize) -> Self {
        Self {
            max_skills,
            max_total_source_bytes,
        }
    }

    pub const fn max_skills(self) -> usize {
        self.max_skills
    }

    pub const fn max_total_source_bytes(self) -> usize {
        self.max_total_source_bytes
    }
}

impl Default for SkillActivationPolicy {
    fn default() -> Self {
        Self::new(
            DEFAULT_MAX_ACTIVATED_SKILLS,
            DEFAULT_MAX_ACTIVATED_SKILL_BYTES,
        )
    }
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SkillActivationRevision(String);

impl SkillActivationRevision {
    pub(super) fn trusted(value: String) -> Self {
        Self(value)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SkillActivationRevision {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("SkillActivationRevision")
            .field(&self.0)
            .finish()
    }
}

impl fmt::Display for SkillActivationRevision {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivatedSkillSet {
    skills: Arc<[ResolvedSkillPackage]>,
    revision: SkillActivationRevision,
    total_source_bytes: usize,
}

impl ActivatedSkillSet {
    pub(super) fn new(
        skills: Vec<ResolvedSkillPackage>,
        revision: SkillActivationRevision,
        total_source_bytes: usize,
    ) -> Self {
        Self {
            skills: skills.into(),
            revision,
            total_source_bytes,
        }
    }

    pub fn skills(&self) -> &[ResolvedSkillPackage] {
        &self.skills
    }

    pub fn revision(&self) -> &SkillActivationRevision {
        &self.revision
    }

    pub fn total_source_bytes(&self) -> usize {
        self.total_source_bytes
    }

    pub fn is_empty(&self) -> bool {
        self.skills.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SkillActivationError {
    SourceRegistration {
        source: SkillRegistrationError,
    },
    TooManySkills {
        max: usize,
        actual: usize,
    },
    DuplicateSelection {
        skill_id: SkillId,
        first_index: usize,
        duplicate_index: usize,
        first_revision: SkillRevision,
        duplicate_revision: SkillRevision,
    },
    Resolve {
        selection_index: usize,
        source: SkillResolveError,
    },
    SourceBudgetExceeded {
        max_bytes: usize,
        actual_bytes: usize,
    },
}

impl SkillActivationError {
    pub fn code(&self) -> SkillErrorCode {
        match self {
            Self::SourceRegistration { source } => source.code(),
            Self::TooManySkills { .. } => SkillErrorCode::TooManySkills,
            Self::DuplicateSelection { .. } => SkillErrorCode::DuplicateSelection,
            Self::Resolve { source, .. } => source.code(),
            Self::SourceBudgetExceeded { .. } => SkillErrorCode::SourceBudgetExceeded,
        }
    }

    pub fn recovery(&self) -> SkillRecovery {
        match self {
            Self::SourceRegistration { source } => source.recovery(),
            Self::TooManySkills { .. } | Self::SourceBudgetExceeded { .. } => {
                SkillRecovery::ReduceSelection
            }
            Self::DuplicateSelection { .. } => SkillRecovery::ChangeSelection,
            Self::Resolve { source, .. } => source.recovery(),
        }
    }

    pub fn skill_id(&self) -> Option<&SkillId> {
        match self {
            Self::DuplicateSelection { skill_id, .. } => Some(skill_id),
            Self::Resolve { source, .. } => source.skill_id(),
            _ => None,
        }
    }

    pub fn expected_revision(&self) -> Option<&SkillRevision> {
        match self {
            Self::DuplicateSelection { first_revision, .. } => Some(first_revision),
            Self::Resolve { source, .. } => source.expected_revision(),
            _ => None,
        }
    }

    pub fn actual_revision(&self) -> Option<&SkillRevision> {
        match self {
            Self::DuplicateSelection {
                duplicate_revision, ..
            } => Some(duplicate_revision),
            Self::Resolve { source, .. } => source.actual_revision(),
            _ => None,
        }
    }

    pub fn message(&self) -> String {
        self.to_string()
    }

    pub fn selection_index(&self) -> Option<usize> {
        match self {
            Self::DuplicateSelection {
                duplicate_index, ..
            } => Some(*duplicate_index),
            Self::Resolve {
                selection_index, ..
            } => Some(*selection_index),
            _ => None,
        }
    }

    pub fn source_error(&self) -> Option<&SkillResolveError> {
        match self {
            Self::Resolve { source, .. } => Some(source),
            _ => None,
        }
    }

    pub fn registration_error(&self) -> Option<&SkillRegistrationError> {
        match self {
            Self::SourceRegistration { source } => Some(source),
            _ => None,
        }
    }
}

impl fmt::Display for SkillActivationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SourceRegistration { source } => source.fmt(formatter),
            Self::TooManySkills { max, actual } => {
                write!(formatter, "cannot activate {actual} Skills; the limit is {max}")
            }
            Self::DuplicateSelection {
                skill_id,
                first_index,
                duplicate_index,
                first_revision,
                duplicate_revision,
            } => write!(
                formatter,
                "Skill `{skill_id}` is selected more than once (indices {first_index} and {duplicate_index}, revisions {first_revision} and {duplicate_revision})"
            ),
            Self::Resolve {
                selection_index,
                source,
            } => write!(
                formatter,
                "cannot resolve Skill selection at index {selection_index}: {source}"
            ),
            Self::SourceBudgetExceeded {
                max_bytes,
                actual_bytes,
            } => write!(
                formatter,
                "activated Skill sources require {actual_bytes} bytes; the limit is {max_bytes}"
            ),
        }
    }
}

impl Error for SkillActivationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::SourceRegistration { source } => Some(source),
            Self::Resolve { source, .. } => Some(source),
            _ => None,
        }
    }
}
