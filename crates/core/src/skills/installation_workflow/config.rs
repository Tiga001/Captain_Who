use super::*;

pub(super) const MAX_REGISTRY_ENTRIES: usize = 4_096;

pub(super) const MAX_REGISTRY_SNAPSHOT_BYTES: usize = 2 * 1024 * 1024 * 1024;

pub(super) const MAX_PREPARATION_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// Default number of live preparation identities retained by a workflow.
pub const DEFAULT_MAX_SKILL_PREPARATIONS: usize = 64;

/// Default aggregate memory budget for exact prepared package snapshots.
pub const DEFAULT_MAX_SKILL_PREPARATION_BYTES: usize = 256 * 1024 * 1024;

/// Default time for which an uncommitted preview remains usable.
pub const DEFAULT_SKILL_PREPARATION_TTL: Duration = Duration::from_secs(15 * 60);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillInstallationWorkflowConfig {
    pub(super) max_preparations: usize,
    pub(super) max_snapshot_bytes: usize,
    pub(super) preparation_ttl: Duration,
}

impl SkillInstallationWorkflowConfig {
    pub fn new(
        max_preparations: usize,
        max_snapshot_bytes: usize,
        preparation_ttl: Duration,
    ) -> Result<Self, SkillInstallationWorkflowConfigurationError> {
        if !(1..=MAX_REGISTRY_ENTRIES).contains(&max_preparations) {
            return Err(SkillInstallationWorkflowConfigurationError::new(format!(
                "max preparations must be between 1 and {MAX_REGISTRY_ENTRIES}",
            )));
        }
        if !(MAX_SKILL_PACKAGE_BYTES..=MAX_REGISTRY_SNAPSHOT_BYTES).contains(&max_snapshot_bytes) {
            return Err(SkillInstallationWorkflowConfigurationError::new(format!(
                "snapshot budget must be between {MAX_SKILL_PACKAGE_BYTES} and {MAX_REGISTRY_SNAPSHOT_BYTES} bytes",
            )));
        }
        if preparation_ttl < Duration::from_millis(1) || preparation_ttl > MAX_PREPARATION_TTL {
            return Err(SkillInstallationWorkflowConfigurationError::new(
                "preparation TTL must be at least one millisecond and no more than 24 hours",
            ));
        }
        Ok(Self {
            max_preparations,
            max_snapshot_bytes,
            preparation_ttl,
        })
    }

    pub fn max_preparations(&self) -> usize {
        self.max_preparations
    }

    pub fn max_snapshot_bytes(&self) -> usize {
        self.max_snapshot_bytes
    }

    pub fn preparation_ttl(&self) -> Duration {
        self.preparation_ttl
    }
}

impl Default for SkillInstallationWorkflowConfig {
    fn default() -> Self {
        Self {
            max_preparations: DEFAULT_MAX_SKILL_PREPARATIONS,
            max_snapshot_bytes: DEFAULT_MAX_SKILL_PREPARATION_BYTES,
            preparation_ttl: DEFAULT_SKILL_PREPARATION_TTL,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillInstallationWorkflowConfigurationError {
    pub(super) reason: String,
}

impl SkillInstallationWorkflowConfigurationError {
    pub(super) fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }

    pub fn reason(&self) -> &str {
        &self.reason
    }
}

impl fmt::Display for SkillInstallationWorkflowConfigurationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.reason.fmt(formatter)
    }
}

impl Error for SkillInstallationWorkflowConfigurationError {}
