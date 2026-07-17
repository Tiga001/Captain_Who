use serde::Serialize;
use std::error::Error;
use std::fmt;
use std::path::PathBuf;

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
            Self::DefaultedName => "defaultedName",
            Self::DuplicateName => "duplicateName",
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
