use super::*;

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
    pub(crate) fn trusted(value: String) -> Self {
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
    pub(crate) fn new(
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
