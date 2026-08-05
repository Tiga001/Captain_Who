use super::*;

#[derive(Debug, Clone)]
pub struct ManagedSkillInstallRequest {
    installation_id: SkillInstallationId,
    package: PreparedSkillPackage,
    provenance: SkillInstallationProvenance,
    pub(super) created_at_unix_ms: u64,
}

impl ManagedSkillInstallRequest {
    pub fn new(package: PreparedSkillPackage) -> Self {
        Self::with_installation_id(SkillInstallationId::new(), package)
    }

    pub fn with_installation_id(
        installation_id: SkillInstallationId,
        package: PreparedSkillPackage,
    ) -> Self {
        let provenance = SkillInstallationProvenance::from_legacy_origin(package.origin());
        Self::with_installation_id_and_provenance(installation_id, package, provenance)
    }

    pub fn with_installation_id_and_provenance(
        installation_id: SkillInstallationId,
        package: PreparedSkillPackage,
        provenance: SkillInstallationProvenance,
    ) -> Self {
        Self {
            installation_id,
            package,
            provenance,
            created_at_unix_ms: unix_time_ms(),
        }
    }

    pub fn installation_id(&self) -> &SkillInstallationId {
        &self.installation_id
    }

    pub fn package(&self) -> &PreparedSkillPackage {
        &self.package
    }

    pub fn provenance(&self) -> &SkillInstallationProvenance {
        &self.provenance
    }

    #[cfg(test)]
    pub(super) fn set_created_at_unix_ms(&mut self, value: u64) {
        self.created_at_unix_ms = value;
    }
}

#[derive(Debug, Clone)]
pub struct ManagedSkillUpdateRequest {
    installation_id: SkillInstallationId,
    expected_revision: SkillInstallationRevision,
    package: PreparedSkillPackage,
    provenance: SkillInstallationProvenance,
}

impl ManagedSkillUpdateRequest {
    pub fn new(
        installation_id: SkillInstallationId,
        expected_revision: SkillInstallationRevision,
        package: PreparedSkillPackage,
    ) -> Self {
        let provenance = SkillInstallationProvenance::from_legacy_origin(package.origin());
        Self::with_provenance(installation_id, expected_revision, package, provenance)
    }

    pub fn with_provenance(
        installation_id: SkillInstallationId,
        expected_revision: SkillInstallationRevision,
        package: PreparedSkillPackage,
        provenance: SkillInstallationProvenance,
    ) -> Self {
        Self {
            installation_id,
            expected_revision,
            package,
            provenance,
        }
    }

    pub fn installation_id(&self) -> &SkillInstallationId {
        &self.installation_id
    }

    pub fn expected_revision(&self) -> &SkillInstallationRevision {
        &self.expected_revision
    }

    pub fn package(&self) -> &PreparedSkillPackage {
        &self.package
    }

    pub fn provenance(&self) -> &SkillInstallationProvenance {
        &self.provenance
    }
}

#[derive(Debug, Clone)]
pub struct ManagedSkillUninstallRequest {
    installation_id: SkillInstallationId,
    expected_revision: SkillInstallationRevision,
}

/// Compatibility request for callers that only possess a package revision.
///
/// Package revisions cannot distinguish an A→B→A lifecycle. This type is
/// intentionally separate from [`ManagedSkillUninstallRequest`] so new code
/// cannot accidentally opt into the weaker comparison model.
#[derive(Debug, Clone)]
pub struct ManagedSkillLegacyUninstallRequest {
    installation_id: SkillInstallationId,
    expected_package_revision: SkillRevision,
}

impl ManagedSkillLegacyUninstallRequest {
    pub fn new(
        installation_id: SkillInstallationId,
        expected_package_revision: SkillRevision,
    ) -> Self {
        Self {
            installation_id,
            expected_package_revision,
        }
    }

    pub fn installation_id(&self) -> &SkillInstallationId {
        &self.installation_id
    }

    pub fn expected_package_revision(&self) -> &SkillRevision {
        &self.expected_package_revision
    }
}

impl ManagedSkillUninstallRequest {
    pub fn new(
        installation_id: SkillInstallationId,
        expected_revision: SkillInstallationRevision,
    ) -> Self {
        Self {
            installation_id,
            expected_revision,
        }
    }

    pub fn installation_id(&self) -> &SkillInstallationId {
        &self.installation_id
    }

    pub fn expected_revision(&self) -> &SkillInstallationRevision {
        &self.expected_revision
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ManagedSkillInstallOutcome {
    Installed,
    AlreadyInstalled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ManagedSkillUpdateOutcome {
    Updated,
    AlreadyCurrent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ManagedSkillUninstallOutcome {
    Uninstalled,
    AlreadyAbsent,
}

/// Revision identities atomically committed by an install or update.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedSkillCommittedState {
    package_revision: SkillRevision,
    installation_revision: SkillInstallationRevision,
}

impl ManagedSkillCommittedState {
    fn from_receipt(receipt: &InstalledSkillReceipt) -> Self {
        Self {
            package_revision: receipt.package.revision.clone(),
            installation_revision: receipt.installation_revision.clone(),
        }
    }

    pub fn package_revision(&self) -> &SkillRevision {
        &self.package_revision
    }

    pub fn installation_revision(&self) -> &SkillInstallationRevision {
        &self.installation_revision
    }
}

/// Outcome plus the receipt state made visible by a managed mutation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedSkillMutationResult<T> {
    outcome: T,
    committed_state: Option<ManagedSkillCommittedState>,
}

impl<T> ManagedSkillMutationResult<T> {
    pub(super) fn committed(outcome: T, receipt: &InstalledSkillReceipt) -> Self {
        Self {
            outcome,
            committed_state: Some(ManagedSkillCommittedState::from_receipt(receipt)),
        }
    }

    pub(super) fn absent(outcome: T) -> Self {
        Self {
            outcome,
            committed_state: None,
        }
    }

    pub fn outcome(&self) -> &T {
        &self.outcome
    }

    pub fn committed_state(&self) -> Option<&ManagedSkillCommittedState> {
        self.committed_state.as_ref()
    }

    pub fn into_outcome(self) -> T {
        self.outcome
    }
}

impl<T: PartialEq> PartialEq<T> for ManagedSkillMutationResult<T> {
    fn eq(&self, other: &T) -> bool {
        self.outcome == *other
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ManagedSkillMutation {
    Install,
    Update,
    Uninstall,
}

impl ManagedSkillMutation {
    pub fn stable_name(self) -> &'static str {
        match self {
            Self::Install => "install",
            Self::Update => "update",
            Self::Uninstall => "uninstall",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ManagedSkillInstallerErrorCode {
    InvalidStore,
    CapacityExceeded,
    InstallationExists,
    InstallationRetired,
    InstallationNotFound,
    RevisionConflict,
    StoreCorrupt,
    Io,
    CommitIndeterminate,
}

impl ManagedSkillInstallerErrorCode {
    pub fn stable_name(self) -> &'static str {
        match self {
            Self::InvalidStore => "invalidStore",
            Self::CapacityExceeded => "capacityExceeded",
            Self::InstallationExists => "installationExists",
            Self::InstallationRetired => "installationRetired",
            Self::InstallationNotFound => "installationNotFound",
            Self::RevisionConflict => "revisionConflict",
            Self::StoreCorrupt => "storeCorrupt",
            Self::Io => "io",
            Self::CommitIndeterminate => "commitIndeterminate",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
/// Bounded store resource reported by a capacity error.
pub enum ManagedSkillStoreCapacity {
    Installations,
    InstallationDirectory,
    RetiredInstallationIds,
    Packages,
}

impl ManagedSkillStoreCapacity {
    pub fn stable_name(self) -> &'static str {
        match self {
            Self::Installations => "installations",
            Self::InstallationDirectory => "installationDirectory",
            Self::RetiredInstallationIds => "retiredInstallationIds",
            Self::Packages => "packages",
        }
    }
}

#[derive(Debug)]
#[non_exhaustive]
pub enum ManagedSkillInstallerError {
    InvalidStore {
        reason: String,
    },
    CapacityExceeded {
        capacity: ManagedSkillStoreCapacity,
        limit: usize,
    },
    InstallationExists {
        installation_id: SkillInstallationId,
        existing_revision: SkillRevision,
        requested_revision: SkillRevision,
    },
    InstallationRetired {
        installation_id: SkillInstallationId,
    },
    InstallationNotFound {
        installation_id: SkillInstallationId,
    },
    RevisionConflict {
        installation_id: SkillInstallationId,
        expected_revision: SkillInstallationRevision,
        actual_revision: SkillInstallationRevision,
    },
    /// Compatibility-only conflict from an atomic package-revision CAS.
    LegacyPackageRevisionConflict {
        installation_id: SkillInstallationId,
        expected_revision: SkillRevision,
        actual_revision: SkillRevision,
    },
    StoreCorrupt {
        reason: String,
    },
    Io {
        operation: String,
        reason: String,
    },
    CommitIndeterminate {
        operation: ManagedSkillMutation,
        installation_id: SkillInstallationId,
        intended_revision: Option<SkillInstallationRevision>,
        reason: String,
    },
}

impl ManagedSkillInstallerError {
    pub fn code(&self) -> ManagedSkillInstallerErrorCode {
        match self {
            Self::InvalidStore { .. } => ManagedSkillInstallerErrorCode::InvalidStore,
            Self::CapacityExceeded { .. } => ManagedSkillInstallerErrorCode::CapacityExceeded,
            Self::InstallationExists { .. } => ManagedSkillInstallerErrorCode::InstallationExists,
            Self::InstallationRetired { .. } => ManagedSkillInstallerErrorCode::InstallationRetired,
            Self::InstallationNotFound { .. } => {
                ManagedSkillInstallerErrorCode::InstallationNotFound
            }
            Self::RevisionConflict { .. } | Self::LegacyPackageRevisionConflict { .. } => {
                ManagedSkillInstallerErrorCode::RevisionConflict
            }
            Self::StoreCorrupt { .. } => ManagedSkillInstallerErrorCode::StoreCorrupt,
            Self::Io { .. } => ManagedSkillInstallerErrorCode::Io,
            Self::CommitIndeterminate { .. } => ManagedSkillInstallerErrorCode::CommitIndeterminate,
        }
    }

    pub fn commit_may_have_succeeded(&self) -> bool {
        matches!(self, Self::CommitIndeterminate { .. })
    }
}

impl fmt::Display for ManagedSkillInstallerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidStore { reason } => write!(formatter, "invalid managed Skill store: {reason}"),
            Self::CapacityExceeded { capacity, limit } => write!(
                formatter,
                "managed Skill {} capacity of {limit} entries is exhausted",
                capacity.stable_name()
            ),
            Self::InstallationExists {
                installation_id,
                existing_revision,
                requested_revision,
            } => write!(
                formatter,
                "managed Skill installation `{installation_id}` already points to `{existing_revision}`, not requested `{requested_revision}`"
            ),
            Self::InstallationRetired { installation_id } => write!(
                formatter,
                "managed Skill installation identity `{installation_id}` has been permanently retired"
            ),
            Self::InstallationNotFound { installation_id } => write!(
                formatter,
                "managed Skill installation `{installation_id}` does not exist"
            ),
            Self::RevisionConflict {
                installation_id,
                expected_revision,
                actual_revision,
            } => write!(
                formatter,
                "managed Skill installation `{installation_id}` revision conflict: expected `{expected_revision}`, actual `{actual_revision}`"
            ),
            Self::LegacyPackageRevisionConflict {
                installation_id,
                expected_revision,
                actual_revision,
            } => write!(
                formatter,
                "managed Skill installation `{installation_id}` package revision conflict: expected `{expected_revision}`, actual `{actual_revision}`"
            ),
            Self::StoreCorrupt { reason } => {
                write!(formatter, "managed Skill store is corrupt: {reason}")
            }
            Self::Io { operation, reason } => write!(formatter, "cannot {operation}: {reason}"),
            Self::CommitIndeterminate {
                operation,
                installation_id,
                reason,
                ..
            } => write!(
                formatter,
                "managed Skill {} for `{installation_id}` may have committed: {reason}",
                operation.stable_name()
            ),
        }
    }
}

impl Error for ManagedSkillInstallerError {}
