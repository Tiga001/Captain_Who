use super::*;

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
    pub(crate) fn new(name: String, description: String) -> Self {
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

pub(crate) struct SkillDescriptorParts {
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
    pub(crate) fn new(parts: SkillDescriptorParts) -> Self {
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
    #[allow(dead_code)]
    pub(crate) fn new(
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

    pub(crate) fn new(entries: Vec<SkillResourceDescriptor>) -> Self {
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
pub(crate) struct SkillPackageInvariantError {
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
    #[allow(dead_code)]
    pub(crate) fn new(
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

    pub(crate) fn with_resources(
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

    pub(crate) fn instructions_range(&self) -> &Range<usize> {
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
    pub(crate) fn new(
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

    pub(crate) fn into_parts(self) -> (Vec<SkillDescriptor>, Vec<SkillDiagnostic>, bool) {
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
    pub(crate) fn new(
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
    UnsupportedToolReference,
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
            Self::UnsupportedToolReference => "unsupportedToolReference",
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
