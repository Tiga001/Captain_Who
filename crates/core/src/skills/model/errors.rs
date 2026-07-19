use super::*;

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
