use serde::Serialize;
use std::error::Error;
use std::fmt;
use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillCatalog {
    pub catalog_revision: String,
    pub skills: Vec<SkillDescriptor>,
    pub diagnostics: Vec<SkillDiagnostic>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillDescriptor {
    pub id: String,
    pub name: String,
    pub description: String,
    pub scope: SkillScope,
    pub path: String,
    pub relative_path: String,
    pub revision: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub enum SkillScope {
    Workspace,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillDiagnostic {
    pub code: SkillDiagnosticCode,
    pub severity: SkillDiagnosticSeverity,
    pub message: String,
    pub path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SkillDiagnosticSeverity {
    Warning,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
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
}

impl SkillDiagnosticCode {
    pub(super) fn stable_name(self) -> &'static str {
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
        }
    }
}

/// Identifies the exact catalog entry that may be resolved for one agent turn.
///
/// `skill_id` is an opaque identifier returned by discovery. `expected_revision`
/// is mandatory so resolving a selection never silently switches to newer bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillResolveRequest {
    pub skill_id: String,
    pub expected_revision: String,
}

/// Stable source identity for a resolved Skill snapshot.
///
/// This is an enum so future user, bundled, or plugin authorities do not need
/// to overload workspace-specific fields.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SkillProvenance {
    Workspace {
        workspace_id: String,
        relative_path: String,
    },
}

/// An immutable snapshot of one validated `SKILL.md` file.
///
/// The complete source is retained because later prompt policy may need the
/// frontmatter as well as the Markdown body. `instructions()` is a zero-copy
/// view over the exact body bytes; neither view is trimmed or normalized.
#[derive(Clone, PartialEq, Eq)]
pub struct ResolvedSkill {
    id: String,
    name: String,
    description: String,
    scope: SkillScope,
    revision: String,
    provenance: SkillProvenance,
    source_text: Arc<str>,
    instructions_range: Range<usize>,
}

pub(super) struct ResolvedSkillMetadata {
    pub id: String,
    pub name: String,
    pub description: String,
    pub scope: SkillScope,
    pub revision: String,
    pub provenance: SkillProvenance,
}

impl ResolvedSkill {
    pub(super) fn new(
        metadata: ResolvedSkillMetadata,
        source_text: Arc<str>,
        instructions_range: Range<usize>,
    ) -> Self {
        debug_assert!(instructions_range.start <= instructions_range.end);
        debug_assert!(instructions_range.end <= source_text.len());
        debug_assert!(source_text.is_char_boundary(instructions_range.start));
        debug_assert!(source_text.is_char_boundary(instructions_range.end));

        Self {
            id: metadata.id,
            name: metadata.name,
            description: metadata.description,
            scope: metadata.scope,
            revision: metadata.revision,
            provenance: metadata.provenance,
            source_text,
            instructions_range,
        }
    }

    pub fn source_text(&self) -> &str {
        &self.source_text
    }

    pub fn instructions(&self) -> &str {
        &self.source_text[self.instructions_range.clone()]
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn description(&self) -> &str {
        &self.description
    }

    pub fn scope(&self) -> SkillScope {
        self.scope
    }

    pub fn revision(&self) -> &str {
        &self.revision
    }

    pub fn provenance(&self) -> &SkillProvenance {
        &self.provenance
    }
}

impl fmt::Debug for ResolvedSkill {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ResolvedSkill")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("description", &self.description)
            .field("scope", &self.scope)
            .field("revision", &self.revision)
            .field("provenance", &self.provenance)
            .field("source_bytes", &self.source_text.len())
            .field("instructions_range", &self.instructions_range)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillResolveError {
    InvalidReference {
        reason: String,
    },
    Workspace(SkillDiscoveryError),
    NotFound {
        skill_id: String,
    },
    Stale {
        skill_id: String,
        expected_revision: String,
        actual_revision: String,
    },
    InvalidSkill {
        skill_id: String,
        code: SkillDiagnosticCode,
        reason: String,
    },
    Unavailable {
        skill_id: Option<String>,
        reason: String,
    },
}

impl SkillResolveError {
    pub fn is_stale(&self) -> bool {
        matches!(self, Self::Stale { .. })
    }

    pub fn is_not_found(&self) -> bool {
        matches!(self, Self::NotFound { .. })
    }

    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::Unavailable { .. }
                | Self::Workspace(SkillDiscoveryError::WorkspaceUnavailable { .. })
        )
    }
}

impl fmt::Display for SkillResolveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidReference { reason } => {
                write!(formatter, "Invalid Skill reference: {reason}")
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
pub enum SkillDiscoveryError {
    WorkspaceUnavailable { path: PathBuf, reason: String },
    WorkspaceNotDirectory { path: PathBuf },
}

impl fmt::Display for SkillDiscoveryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
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
