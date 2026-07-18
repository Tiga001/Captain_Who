//! Crash-consistent mutations for the application-managed Skill store.
//!
//! The installer consumes only immutable [`PreparedSkillPackage`] snapshots.
//! It publishes a content-addressed package before atomically committing the
//! receipt, which is the sole installation/update linearization point.
//!
//! The store is an application-owned local-filesystem capability. The writer
//! lock coordinates cooperating installer instances; it is not a sandbox
//! against a hostile same-user process that can swap managed parent
//! directories while a transaction is running. A future handle-relative
//! filesystem backend can strengthen that boundary without changing the
//! prepared-package or transaction APIs.

use super::acquisition_provenance::SkillInstallationProvenance;
use super::managed_fs::{atomic_rename_noreplace, atomic_replace, sync_directory};
#[cfg(test)]
use super::managed_store::encode_receipt;
use super::managed_store::{
    encode_receipt_v2, InstalledPackageRef, InstalledSkillReceipt, ManagedSkillStore,
    ManagedStoreLoadError, INSTALLATIONS_DIRECTORY, MAX_INSTALLATION_DIRECTORY_ENTRIES,
    MAX_LIVE_INSTALLATIONS, MAX_MANAGED_DIRECTORY_ENTRIES, MAX_RETIRED_INSTALLATION_ENTRIES,
    PACKAGES_DIRECTORY, PACKAGE_V1_DIRECTORY, PACKAGE_V2_DIRECTORY, PACKAGE_V3_DIRECTORY,
    RETIRED_INSTALLATIONS_DIRECTORY,
};
use super::model::{
    SkillInstallationId, SkillInstallationRevision, SkillRevision, SKILL_PACKAGE_FORMAT_VERSION,
    SKILL_PACKAGE_FORMAT_VERSION_V2, SKILL_PACKAGE_FORMAT_VERSION_V3,
};
use super::package::{
    MAX_SKILL_PACKAGE_DEPTH, MAX_SKILL_PACKAGE_DIRECTORIES, MAX_SKILL_PACKAGE_DIRECTORY_ENTRIES,
    MAX_SKILL_PACKAGE_FILES, PACKAGE_MANIFEST_FILE,
};
use super::prepared::PreparedSkillPackage;
use super::workspace::{
    is_symlink_or_reparse, metadata_if_present, verify_opened_file_identity, SKILL_FILE_NAME,
};
use std::collections::BTreeSet;
use std::error::Error;
use std::ffi::OsStr;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
#[cfg(windows)]
use std::os::windows::fs::OpenOptionsExt;
#[cfg(windows)]
use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT;

const WRITER_LOCK_FILE: &str = ".writer.lock";
const MAX_LAYOUT_ENTRIES: usize = 100_000;
const MAX_STAGING_CLEANUP_ENTRIES: usize = MAX_INSTALLATION_DIRECTORY_ENTRIES;
const MAX_PACKAGE_STAGING_TREE_ENTRIES: usize =
    MAX_SKILL_PACKAGE_FILES + MAX_SKILL_PACKAGE_DIRECTORIES + 1;
const STAGING_ATTEMPTS: usize = 8;

#[derive(Debug)]
pub struct ManagedSkillInstaller {
    root: PathBuf,
    process_lock: Mutex<()>,
    #[cfg(test)]
    failpoint: Option<ManagedSkillInstallerFailpoint>,
}

impl ManagedSkillInstaller {
    /// Configures an installer without touching the filesystem.
    pub fn new(root: impl Into<PathBuf>) -> Result<Self, ManagedSkillInstallerError> {
        let root = root.into();
        if !root.is_absolute() {
            return Err(ManagedSkillInstallerError::InvalidStore {
                reason: "managed Skill store root must be an absolute path".to_string(),
            });
        }
        Ok(Self {
            root,
            process_lock: Mutex::new(()),
            #[cfg(test)]
            failpoint: None,
        })
    }

    /// Installs a prepared package under a stable installation identity.
    ///
    /// Repeating the same request is idempotent. Reusing the installation ID
    /// for different package bytes fails instead of silently overwriting it.
    pub fn install(
        &self,
        request: &ManagedSkillInstallRequest,
    ) -> Result<ManagedSkillMutationResult<ManagedSkillInstallOutcome>, ManagedSkillInstallerError>
    {
        let transaction = self.begin_transaction()?;
        let intended_receipt = InstalledSkillReceipt::new_v2(
            request.installation_id().clone(),
            1,
            request.package().format_version(),
            request.package().revision().clone(),
            request.provenance().clone(),
            request.created_at_unix_ms,
            request.created_at_unix_ms,
        )
        .map_err(|reason| ManagedSkillInstallerError::StoreCorrupt { reason })?;
        let existing = load_receipt(&transaction.store, request.installation_id())?;
        if existing.is_some() {
            transaction.ensure_live_identity_not_retired(request.installation_id())?;
        }
        match existing {
            Some(receipt)
                if receipt.package.format_version == request.package().format_version()
                    && receipt.package.revision == *request.package().revision()
                    && receipt.provenance == *request.provenance() =>
            {
                verify_prepared_package(&transaction.store, request.package())?;
                transaction.sync_existing_commit(
                    ManagedSkillMutation::Install,
                    request.installation_id(),
                    Some(&receipt.installation_revision),
                )?;
                Ok(ManagedSkillMutationResult::committed(
                    ManagedSkillInstallOutcome::AlreadyInstalled,
                    &receipt,
                ))
            }
            Some(receipt) => Err(ManagedSkillInstallerError::InstallationExists {
                installation_id: request.installation_id().clone(),
                existing_revision: receipt.package.revision,
                requested_revision: request.package().revision().clone(),
            }),
            None => {
                transaction.ensure_installation_id_available(request.installation_id())?;
                transaction.ensure_new_installation_capacity()?;
                transaction.publish_package(request.package())?;
                transaction.commit_receipt(
                    ManagedSkillMutation::Install,
                    &intended_receipt,
                    ReceiptCommitMode::Create,
                )?;
                Ok(ManagedSkillMutationResult::committed(
                    ManagedSkillInstallOutcome::Installed,
                    &intended_receipt,
                ))
            }
        }
    }

    /// Replaces an installation using exact lifecycle compare-and-swap.
    ///
    /// A retry whose target revision is already visible succeeds even if its
    /// expected revision is now stale, which lets callers recover from an
    /// indeterminate commit acknowledgement.
    pub fn update(
        &self,
        request: &ManagedSkillUpdateRequest,
    ) -> Result<ManagedSkillMutationResult<ManagedSkillUpdateOutcome>, ManagedSkillInstallerError>
    {
        let transaction = self.begin_transaction()?;
        let receipt =
            load_receipt(&transaction.store, request.installation_id())?.ok_or_else(|| {
                ManagedSkillInstallerError::InstallationNotFound {
                    installation_id: request.installation_id().clone(),
                }
            })?;
        transaction.ensure_live_identity_not_retired(request.installation_id())?;

        // Check the intended target before the expected revision. This makes a
        // retry converge after a lost or indeterminate commit acknowledgement.
        if !receipt.is_legacy_v1()
            && receipt.package.format_version == request.package().format_version()
            && receipt.package.revision == *request.package().revision()
            && receipt.provenance == *request.provenance()
        {
            verify_prepared_package(&transaction.store, request.package())?;
            transaction.sync_existing_commit(
                ManagedSkillMutation::Update,
                request.installation_id(),
                Some(&receipt.installation_revision),
            )?;
            return Ok(ManagedSkillMutationResult::committed(
                ManagedSkillUpdateOutcome::AlreadyCurrent,
                &receipt,
            ));
        }
        if receipt.installation_revision != *request.expected_revision() {
            return Err(ManagedSkillInstallerError::RevisionConflict {
                installation_id: request.installation_id().clone(),
                expected_revision: request.expected_revision().clone(),
                actual_revision: receipt.installation_revision,
            });
        }

        let generation = receipt.generation.checked_add(1).ok_or_else(|| {
            ManagedSkillInstallerError::StoreCorrupt {
                reason: format!(
                    "managed Skill installation `{}` exhausted its receipt generation",
                    request.installation_id()
                ),
            }
        })?;
        let updated_at_unix_ms = unix_time_ms()
            .max(receipt.installed_at_unix_ms)
            .max(receipt.updated_at_unix_ms);
        let intended_receipt = InstalledSkillReceipt::new_v2(
            request.installation_id().clone(),
            generation,
            request.package().format_version(),
            request.package().revision().clone(),
            request.provenance().clone(),
            receipt.installed_at_unix_ms,
            updated_at_unix_ms,
        )
        .map_err(|reason| ManagedSkillInstallerError::StoreCorrupt { reason })?;

        transaction.ensure_receipt_staging_capacity()?;
        transaction.publish_package(request.package())?;
        transaction.commit_receipt(
            ManagedSkillMutation::Update,
            &intended_receipt,
            ReceiptCommitMode::Replace,
        )?;
        Ok(ManagedSkillMutationResult::committed(
            ManagedSkillUpdateOutcome::Updated,
            &intended_receipt,
        ))
    }

    /// Removes an installation using exact lifecycle compare-and-swap.
    ///
    /// Immutable package objects are retained. The receipt is only the live
    /// activation reference; the durable retired marker is the monotonic,
    /// authoritative uninstall linearization point.
    pub fn uninstall(
        &self,
        request: &ManagedSkillUninstallRequest,
    ) -> Result<ManagedSkillMutationResult<ManagedSkillUninstallOutcome>, ManagedSkillInstallerError>
    {
        self.uninstall_with_expectation(
            request.installation_id(),
            UninstallExpectation::InstallationRevision(request.expected_revision()),
        )
    }

    /// Compatibility-only package-revision CAS uninstall.
    ///
    /// Unlike a service-layer receipt lookup followed by [`Self::uninstall`],
    /// this method evaluates the legacy package revision and durably retires
    /// the installation identity while holding the same writer transaction.
    /// New callers should always prefer the exact lifecycle revision API.
    pub fn uninstall_legacy(
        &self,
        request: &ManagedSkillLegacyUninstallRequest,
    ) -> Result<ManagedSkillMutationResult<ManagedSkillUninstallOutcome>, ManagedSkillInstallerError>
    {
        self.uninstall_with_expectation(
            request.installation_id(),
            UninstallExpectation::PackageRevision(request.expected_package_revision()),
        )
    }

    fn uninstall_with_expectation(
        &self,
        installation_id: &SkillInstallationId,
        expectation: UninstallExpectation<'_>,
    ) -> Result<ManagedSkillMutationResult<ManagedSkillUninstallOutcome>, ManagedSkillInstallerError>
    {
        let transaction = self.begin_transaction()?;
        let Some(receipt) = load_receipt(&transaction.store, installation_id)? else {
            // Every uninstall mode also retires an already-absent identity. This
            // closes the crash window in which a previously deleted receipt
            // name was not durable and an old install retry could otherwise
            // resurrect it after restart.
            transaction.retire_installation(installation_id, false)?;
            return Ok(ManagedSkillMutationResult::absent(
                ManagedSkillUninstallOutcome::AlreadyAbsent,
            ));
        };
        match expectation {
            UninstallExpectation::InstallationRevision(expected_revision)
                if receipt.installation_revision != *expected_revision =>
            {
                return Err(ManagedSkillInstallerError::RevisionConflict {
                    installation_id: installation_id.clone(),
                    expected_revision: expected_revision.clone(),
                    actual_revision: receipt.installation_revision,
                });
            }
            UninstallExpectation::PackageRevision(expected_revision)
                if receipt.package.revision != *expected_revision =>
            {
                return Err(ManagedSkillInstallerError::LegacyPackageRevisionConflict {
                    installation_id: installation_id.clone(),
                    expected_revision: expected_revision.clone(),
                    actual_revision: receipt.package.revision,
                });
            }
            _ => {}
        }

        transaction.retire_installation(installation_id, true)?;

        // The retired receipt is a durable, single-use installation-ID ledger.
        // Keeping it prevents stale install retries from resurrecting an
        // explicitly uninstalled Skill and closes uninstall/reinstall CAS ABA.
        Ok(ManagedSkillMutationResult::absent(
            ManagedSkillUninstallOutcome::Uninstalled,
        ))
    }

    fn begin_transaction(&self) -> Result<ManagedStoreTransaction<'_>, ManagedSkillInstallerError> {
        let process_guard =
            self.process_lock
                .lock()
                .map_err(|_| ManagedSkillInstallerError::Io {
                    operation: "acquire managed Skill installer process lock".to_string(),
                    reason: "installer process lock is poisoned".to_string(),
                })?;
        let root = ensure_store_root(&self.root)?;
        let writer_lock = acquire_writer_lock(&root)?;
        let installations = ensure_exact_directory(&root, INSTALLATIONS_DIRECTORY)?;
        let retired_installations = ensure_exact_directory(&root, RETIRED_INSTALLATIONS_DIRECTORY)?;
        let packages = ensure_exact_directory(&root, PACKAGES_DIRECTORY)?;
        let package_v1 = ensure_exact_directory(&packages, PACKAGE_V1_DIRECTORY)?;
        let package_v2 = ensure_exact_directory(&packages, PACKAGE_V2_DIRECTORY)?;
        let package_v3 = ensure_exact_directory(&packages, PACKAGE_V3_DIRECTORY)?;
        let layout = ManagedStoreLayout {
            root,
            installations,
            retired_installations,
            package_v1,
            package_v2,
            package_v3,
        };
        cleanup_stale_transaction_entries(&layout)?;
        let store = ManagedSkillStore::new(layout.root.clone())
            .map_err(|reason| ManagedSkillInstallerError::InvalidStore { reason })?;
        Ok(ManagedStoreTransaction {
            installer: self,
            _process_guard: process_guard,
            _writer_lock: writer_lock,
            layout,
            store,
        })
    }

    #[cfg(test)]
    fn with_failpoint(root: impl Into<PathBuf>, failpoint: ManagedSkillInstallerFailpoint) -> Self {
        let mut installer = Self::new(root).unwrap();
        installer.failpoint = Some(failpoint);
        installer
    }
}

#[derive(Debug, Clone, Copy)]
enum UninstallExpectation<'a> {
    InstallationRevision(&'a SkillInstallationRevision),
    PackageRevision(&'a SkillRevision),
}

#[derive(Debug, Clone)]
pub struct ManagedSkillInstallRequest {
    installation_id: SkillInstallationId,
    package: PreparedSkillPackage,
    provenance: SkillInstallationProvenance,
    created_at_unix_ms: u64,
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
    fn set_created_at_unix_ms(&mut self, value: u64) {
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
    fn committed(outcome: T, receipt: &InstalledSkillReceipt) -> Self {
        Self {
            outcome,
            committed_state: Some(ManagedSkillCommittedState::from_receipt(receipt)),
        }
    }

    fn absent(outcome: T) -> Self {
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

struct ManagedStoreTransaction<'a> {
    installer: &'a ManagedSkillInstaller,
    _process_guard: MutexGuard<'a, ()>,
    _writer_lock: File,
    layout: ManagedStoreLayout,
    store: ManagedSkillStore,
}

impl ManagedStoreTransaction<'_> {
    fn retired_identity_exists(
        &self,
        installation_id: &SkillInstallationId,
    ) -> Result<bool, ManagedSkillInstallerError> {
        let retired_path = self.layout.retired_path(installation_id);
        let Some(metadata) = metadata_if_present(&retired_path).map_err(|error| {
            self.io_error("inspect retired managed Skill installation identity", error)
        })?
        else {
            return Ok(false);
        };
        if is_symlink_or_reparse(&metadata) || !metadata.is_file() {
            return Err(ManagedSkillInstallerError::StoreCorrupt {
                reason: format!(
                    "retired managed Skill installation identity `{installation_id}` is not a plain file"
                ),
            });
        }
        Ok(true)
    }

    fn ensure_installation_id_available(
        &self,
        installation_id: &SkillInstallationId,
    ) -> Result<(), ManagedSkillInstallerError> {
        if !self.retired_identity_exists(installation_id)? {
            return Ok(());
        }
        Err(ManagedSkillInstallerError::InstallationRetired {
            installation_id: installation_id.clone(),
        })
    }

    fn ensure_live_identity_not_retired(
        &self,
        installation_id: &SkillInstallationId,
    ) -> Result<(), ManagedSkillInstallerError> {
        if self.retired_identity_exists(installation_id)? {
            Err(ManagedSkillInstallerError::StoreCorrupt {
                reason: format!(
                    "managed Skill installation `{installation_id}` is simultaneously live and retired"
                ),
            })
        } else {
            Ok(())
        }
    }

    fn ensure_new_installation_capacity(&self) -> Result<(), ManagedSkillInstallerError> {
        let (directory_entries, live_entries) =
            count_installation_entries(&self.layout.installations)?;
        if live_entries >= MAX_LIVE_INSTALLATIONS {
            return Err(ManagedSkillInstallerError::CapacityExceeded {
                capacity: ManagedSkillStoreCapacity::Installations,
                limit: MAX_LIVE_INSTALLATIONS,
            });
        }
        let retired_entries = count_directory_entries(
            &self.layout.retired_installations,
            MAX_RETIRED_INSTALLATION_ENTRIES,
            ManagedSkillStoreCapacity::RetiredInstallationIds,
        )?;
        // Every live identity owns one future retirement slot. Without this
        // reservation the append-only ledger could fill while Skills remain
        // installed, making those installations impossible to uninstall.
        ensure_count_has_room(
            retired_entries.saturating_add(live_entries),
            MAX_RETIRED_INSTALLATION_ENTRIES,
            ManagedSkillStoreCapacity::RetiredInstallationIds,
        )?;
        ensure_count_has_room(
            directory_entries,
            MAX_INSTALLATION_DIRECTORY_ENTRIES,
            ManagedSkillStoreCapacity::InstallationDirectory,
        )
    }

    fn ensure_receipt_staging_capacity(&self) -> Result<(), ManagedSkillInstallerError> {
        let (directory_entries, _) = count_installation_entries(&self.layout.installations)?;
        ensure_count_has_room(
            directory_entries,
            MAX_INSTALLATION_DIRECTORY_ENTRIES,
            ManagedSkillStoreCapacity::InstallationDirectory,
        )
    }

    fn publish_package(
        &self,
        package: &PreparedSkillPackage,
    ) -> Result<(), ManagedSkillInstallerError> {
        let package_ref = InstalledPackageRef::from_format_and_revision(
            package.format_version(),
            package.revision().clone(),
        )
        .map_err(|reason| ManagedSkillInstallerError::StoreCorrupt { reason })?;
        let package_version = self.layout.package_version(package.format_version());
        let target = package_version.join(&package_ref.digest_hex);
        if metadata_if_present(&target)
            .map_err(|error| self.io_error("inspect managed Skill package", error))?
            .is_some()
        {
            return self.ensure_existing_package_durable(package);
        }
        ensure_directory_has_room(
            package_version,
            MAX_MANAGED_DIRECTORY_ENTRIES,
            ManagedSkillStoreCapacity::Packages,
        )?;

        let mut staging = self
            .layout
            .unique_package_staging(package.format_version(), &package_ref.digest_hex)?;
        self.write_staged_package(&staging.path, package)?;
        if self.should_fail(ManagedSkillInstallerFailpoint::PackageFileSynced) {
            return Err(self.injected_io("publish managed Skill package after file sync"));
        }
        match atomic_rename_noreplace(&staging.path, &target) {
            Ok(()) => staging.disarm(),
            Err(error)
                if error.kind() == io::ErrorKind::AlreadyExists
                    || metadata_if_present(&target).ok().flatten().is_some() =>
            {
                return self.ensure_existing_package_durable(package);
            }
            Err(error) => {
                return Err(self.io_error("atomically publish managed Skill package", error))
            }
        }
        if self.should_fail(ManagedSkillInstallerFailpoint::PackagePublished) {
            return Err(self.injected_io("publish managed Skill package before parent sync"));
        }
        sync_directory(package_version).map_err(|error| {
            self.io_error("sync managed Skill package version directory", error)
        })?;
        if self.should_fail(ManagedSkillInstallerFailpoint::PackageParentSynced) {
            return Err(self.injected_io("publish managed Skill package after parent sync"));
        }
        verify_prepared_package(&self.store, package)
    }

    fn write_staged_package(
        &self,
        staging_root: &Path,
        package: &PreparedSkillPackage,
    ) -> Result<(), ManagedSkillInstallerError> {
        self.write_staged_file(&staging_root.join(SKILL_FILE_NAME), package.source_bytes())?;
        let mut directories = BTreeSet::new();
        if matches!(
            package.format_version(),
            SKILL_PACKAGE_FORMAT_VERSION_V2 | SKILL_PACKAGE_FORMAT_VERSION_V3
        ) {
            for resource in package.resources() {
                let relative = Path::new(resource.descriptor().path());
                let parent =
                    relative
                        .parent()
                        .ok_or_else(|| ManagedSkillInstallerError::StoreCorrupt {
                            reason: "prepared package resource has no parent directory".to_string(),
                        })?;
                let mut current = PathBuf::new();
                for component in parent.components() {
                    current.push(component.as_os_str());
                    if directories.insert(current.clone()) {
                        fs::create_dir(staging_root.join(&current)).map_err(|error| {
                            self.io_error("create managed Skill resource staging directory", error)
                        })?;
                    }
                }
                self.write_staged_file(&staging_root.join(relative), resource.bytes())?;
            }
            let manifest = package.manifest_bytes().ok_or_else(|| {
                ManagedSkillInstallerError::StoreCorrupt {
                    reason: "prepared package has no manifest bytes".to_string(),
                }
            })?;
            self.write_staged_file(&staging_root.join(PACKAGE_MANIFEST_FILE), manifest)?;
        }

        let mut directories = directories.into_iter().collect::<Vec<_>>();
        directories.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
        for directory in directories {
            sync_directory(&staging_root.join(directory)).map_err(|error| {
                self.io_error("sync managed Skill resource staging directory", error)
            })?;
        }
        sync_directory(staging_root)
            .map_err(|error| self.io_error("sync managed Skill package staging directory", error))
    }

    fn write_staged_file(
        &self,
        path: &Path,
        bytes: &[u8],
    ) -> Result<(), ManagedSkillInstallerError> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options
            .open(path)
            .map_err(|error| self.io_error("create managed Skill package staging file", error))?;
        file.write_all(bytes)
            .map_err(|error| self.io_error("write managed Skill package staging file", error))?;
        file.flush()
            .map_err(|error| self.io_error("flush managed Skill package staging file", error))?;
        file.sync_all()
            .map_err(|error| self.io_error("sync managed Skill package staging file", error))
    }

    fn ensure_existing_package_durable(
        &self,
        package: &PreparedSkillPackage,
    ) -> Result<(), ManagedSkillInstallerError> {
        verify_prepared_package(&self.store, package)?;
        // The package may be an orphan left by a crash immediately after its
        // rename. Re-sync the parent before any receipt may reference it.
        sync_directory(self.layout.package_version(package.format_version())).map_err(|error| {
            self.io_error("sync reused managed Skill package version directory", error)
        })?;
        if self.should_fail(ManagedSkillInstallerFailpoint::PackageParentSynced) {
            return Err(self.injected_io("reuse managed Skill package after parent sync"));
        }
        Ok(())
    }

    fn commit_receipt(
        &self,
        mutation: ManagedSkillMutation,
        receipt: &InstalledSkillReceipt,
        mode: ReceiptCommitMode,
    ) -> Result<(), ManagedSkillInstallerError> {
        // Recheck immediately before staging. Besides defending the invariant,
        // this keeps the method safe if another internal call site is added.
        self.ensure_receipt_staging_capacity()?;
        let receipt_bytes = encode_receipt_v2(receipt)
            .map_err(|reason| ManagedSkillInstallerError::StoreCorrupt { reason })?;
        let mut staging = self
            .layout
            .unique_receipt_staging(&receipt.installation_id)?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staging.path)
            .map_err(|error| self.io_error("create managed Skill receipt staging file", error))?;
        staging.arm();
        file.write_all(&receipt_bytes)
            .map_err(|error| self.io_error("write managed Skill receipt staging file", error))?;
        file.flush()
            .map_err(|error| self.io_error("flush managed Skill receipt staging file", error))?;
        file.sync_all()
            .map_err(|error| self.io_error("sync managed Skill receipt staging file", error))?;
        drop(file);
        if self.should_fail(ManagedSkillInstallerFailpoint::ReceiptFileSynced) {
            return Err(self.injected_io("commit managed Skill receipt after file sync"));
        }

        let target = self.layout.receipt_path(&receipt.installation_id);
        let publish_result = match mode {
            ReceiptCommitMode::Create => atomic_rename_noreplace(&staging.path, &target),
            ReceiptCommitMode::Replace => atomic_replace(&staging.path, &target),
        };
        publish_result
            .map_err(|error| self.io_error("atomically commit managed Skill receipt", error))?;
        staging.disarm();
        if self.should_fail(ManagedSkillInstallerFailpoint::ReceiptPublished) {
            return Err(self.commit_indeterminate(
                mutation,
                &receipt.installation_id,
                Some(&receipt.installation_revision),
                "injected failure after receipt publish",
            ));
        }
        sync_directory(&self.layout.installations).map_err(|error| {
            self.commit_indeterminate(
                mutation,
                &receipt.installation_id,
                Some(&receipt.installation_revision),
                format!("cannot sync installations directory after receipt commit: {error}"),
            )
        })?;
        if self.should_fail(ManagedSkillInstallerFailpoint::ReceiptParentSynced) {
            return Err(self.commit_indeterminate(
                mutation,
                &receipt.installation_id,
                Some(&receipt.installation_revision),
                "injected failure after receipt directory sync",
            ));
        }
        Ok(())
    }

    fn sync_existing_commit(
        &self,
        mutation: ManagedSkillMutation,
        installation_id: &SkillInstallationId,
        intended_revision: Option<&SkillInstallationRevision>,
    ) -> Result<(), ManagedSkillInstallerError> {
        sync_directory(&self.layout.installations).map_err(|error| {
            self.commit_indeterminate(
                mutation,
                installation_id,
                intended_revision,
                format!("cannot sync an already-visible receipt state: {error}"),
            )
        })
    }

    /// Durably retires an installation identity before removing its live
    /// receipt. The order is intentionally monotonic across directories:
    ///
    /// 1. publish and fsync the retired marker;
    /// 2. remove the live receipt;
    /// 3. fsync the installations directory.
    ///
    /// Readers treat a retired marker as authoritative if a crash exposes both
    /// names, and writer recovery removes the stale live side. There is never a
    /// durable state in which deletion is committed without the identity also
    /// being retired.
    fn retire_installation(
        &self,
        installation_id: &SkillInstallationId,
        had_live_receipt: bool,
    ) -> Result<(), ManagedSkillInstallerError> {
        self.publish_retired_identity(installation_id, had_live_receipt)?;
        if self.should_fail(ManagedSkillInstallerFailpoint::UninstallRetirementPublished) {
            return Err(self.commit_indeterminate(
                ManagedSkillMutation::Uninstall,
                installation_id,
                None,
                "injected failure after durable retirement marker publish",
            ));
        }

        let receipt_path = self.layout.receipt_path(installation_id);
        if let Some(metadata) = metadata_if_present(&receipt_path).map_err(|error| {
            self.commit_indeterminate(
                ManagedSkillMutation::Uninstall,
                installation_id,
                None,
                format!("cannot inspect retired managed Skill receipt: {error}"),
            )
        })? {
            if is_symlink_or_reparse(&metadata) || !metadata.is_file() {
                return Err(self.commit_indeterminate(
                    ManagedSkillMutation::Uninstall,
                    installation_id,
                    None,
                    format!(
                        "live managed Skill receipt for retired installation `{installation_id}` is not a plain file"
                    ),
                ));
            }
            fs::remove_file(&receipt_path).map_err(|error| {
                self.commit_indeterminate(
                    ManagedSkillMutation::Uninstall,
                    installation_id,
                    None,
                    format!("cannot remove retired managed Skill receipt: {error}"),
                )
            })?;
        }
        sync_directory(&self.layout.installations).map_err(|error| {
            self.commit_indeterminate(
                ManagedSkillMutation::Uninstall,
                installation_id,
                None,
                format!("cannot sync installations directory after retirement: {error}"),
            )
        })?;
        if self.should_fail(ManagedSkillInstallerFailpoint::UninstallParentSynced) {
            return Err(self.commit_indeterminate(
                ManagedSkillMutation::Uninstall,
                installation_id,
                None,
                "injected failure after uninstall directory sync",
            ));
        }
        Ok(())
    }

    fn publish_retired_identity(
        &self,
        installation_id: &SkillInstallationId,
        had_live_receipt: bool,
    ) -> Result<(), ManagedSkillInstallerError> {
        let target = self.layout.retired_path(installation_id);
        if self.retired_identity_exists(installation_id)? {
            return sync_directory(&self.layout.retired_installations).map_err(|error| {
                self.commit_indeterminate(
                    ManagedSkillMutation::Uninstall,
                    installation_id,
                    None,
                    format!("cannot sync an already-visible retired identity: {error}"),
                )
            });
        }
        let retired_entries = count_directory_entries(
            &self.layout.retired_installations,
            MAX_RETIRED_INSTALLATION_ENTRIES,
            ManagedSkillStoreCapacity::RetiredInstallationIds,
        )?;
        let (_, live_entries) = count_installation_entries(&self.layout.installations)?;
        ensure_retirement_marker_capacity(retired_entries, live_entries, had_live_receipt)?;

        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut marker = options.open(&target).map_err(|error| {
            self.io_error("create retired managed Skill installation identity", error)
        })?;
        let bytes = format!(
            "{{\"schemaVersion\":1,\"installationId\":\"{installation_id}\",\"state\":\"retired\"}}\n"
        );
        marker.write_all(bytes.as_bytes()).map_err(|error| {
            self.commit_indeterminate(
                ManagedSkillMutation::Uninstall,
                installation_id,
                None,
                format!("cannot write retired installation identity: {error}"),
            )
        })?;
        marker.flush().map_err(|error| {
            self.commit_indeterminate(
                ManagedSkillMutation::Uninstall,
                installation_id,
                None,
                format!("cannot flush retired installation identity: {error}"),
            )
        })?;
        marker.sync_all().map_err(|error| {
            self.commit_indeterminate(
                ManagedSkillMutation::Uninstall,
                installation_id,
                None,
                format!("cannot sync retired installation identity: {error}"),
            )
        })?;
        drop(marker);
        sync_directory(&self.layout.retired_installations).map_err(|error| {
            self.commit_indeterminate(
                ManagedSkillMutation::Uninstall,
                installation_id,
                None,
                format!("cannot sync retired installation identities directory: {error}"),
            )
        })
    }

    fn io_error(&self, operation: &str, error: io::Error) -> ManagedSkillInstallerError {
        ManagedSkillInstallerError::Io {
            operation: operation.to_string(),
            reason: error.to_string(),
        }
    }

    fn injected_io(&self, operation: &str) -> ManagedSkillInstallerError {
        ManagedSkillInstallerError::Io {
            operation: operation.to_string(),
            reason: "injected transaction failure".to_string(),
        }
    }

    fn commit_indeterminate(
        &self,
        operation: ManagedSkillMutation,
        installation_id: &SkillInstallationId,
        intended_revision: Option<&SkillInstallationRevision>,
        reason: impl Into<String>,
    ) -> ManagedSkillInstallerError {
        ManagedSkillInstallerError::CommitIndeterminate {
            operation,
            installation_id: installation_id.clone(),
            intended_revision: intended_revision.cloned(),
            reason: reason.into(),
        }
    }

    fn should_fail(&self, point: ManagedSkillInstallerFailpoint) -> bool {
        #[cfg(test)]
        {
            self.installer.failpoint == Some(point)
        }
        #[cfg(not(test))]
        {
            let _ = self.installer;
            let _ = point;
            false
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum ReceiptCommitMode {
    Create,
    Replace,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ManagedSkillInstallerFailpoint {
    PackageFileSynced,
    PackagePublished,
    PackageParentSynced,
    ReceiptFileSynced,
    ReceiptPublished,
    ReceiptParentSynced,
    UninstallRetirementPublished,
    UninstallParentSynced,
}

#[derive(Debug)]
struct ManagedStoreLayout {
    root: PathBuf,
    installations: PathBuf,
    retired_installations: PathBuf,
    package_v1: PathBuf,
    package_v2: PathBuf,
    package_v3: PathBuf,
}

impl ManagedStoreLayout {
    fn package_version(&self, format_version: u32) -> &Path {
        match format_version {
            SKILL_PACKAGE_FORMAT_VERSION => &self.package_v1,
            SKILL_PACKAGE_FORMAT_VERSION_V2 => &self.package_v2,
            SKILL_PACKAGE_FORMAT_VERSION_V3 => &self.package_v3,
            _ => unreachable!("PreparedSkillPackage validates format version"),
        }
    }

    fn receipt_path(&self, installation_id: &SkillInstallationId) -> PathBuf {
        self.installations.join(format!("{installation_id}.json"))
    }

    fn retired_path(&self, installation_id: &SkillInstallationId) -> PathBuf {
        self.retired_installations
            .join(format!("{installation_id}.json"))
    }

    fn unique_package_staging(
        &self,
        format_version: u32,
        digest: &str,
    ) -> Result<StagingPath, ManagedSkillInstallerError> {
        create_unique_staging(
            self.package_version(format_version),
            |nonce| format!(".package-{digest}-{nonce}.tmp"),
            StagingKind::Directory,
        )
    }

    fn unique_receipt_staging(
        &self,
        installation_id: &SkillInstallationId,
    ) -> Result<StagingPath, ManagedSkillInstallerError> {
        reserve_unique_staging_path(
            &self.installations,
            |nonce| format!(".receipt-{installation_id}-{nonce}.tmp"),
            StagingKind::File,
        )
    }
}

#[derive(Debug, Clone, Copy)]
enum StagingKind {
    File,
    Directory,
}

#[derive(Debug)]
struct StagingPath {
    path: PathBuf,
    parent: PathBuf,
    kind: StagingKind,
    armed: bool,
}

impl StagingPath {
    fn arm(&mut self) {
        self.armed = true;
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for StagingPath {
    fn drop(&mut self) {
        if !self.armed
            || self.path.parent() != Some(self.parent.as_path())
            || !self.has_owned_name()
        {
            return;
        }
        match self.kind {
            StagingKind::File => {
                if fs::symlink_metadata(&self.path)
                    .is_ok_and(|metadata| !is_symlink_or_reparse(&metadata) && metadata.is_file())
                {
                    let _ = fs::remove_file(&self.path);
                }
            }
            StagingKind::Directory => {
                // Package staging names are transaction-owned capabilities.
                // Validate the complete bounded tree before recursive removal,
                // and never follow links or reparse points. This remains a
                // path-based best effort under the store's documented
                // same-user threat model; a future handle-relative backend can
                // strengthen races without changing transaction semantics.
                let _ = remove_owned_package_staging_directory(&self.parent, &self.path);
            }
        }
    }
}

impl StagingPath {
    fn has_owned_name(&self) -> bool {
        let Some(name) = self.path.file_name().and_then(OsStr::to_str) else {
            return false;
        };
        match self.kind {
            StagingKind::File => is_owned_receipt_staging_name(name),
            StagingKind::Directory => is_owned_package_staging_name(name),
        }
    }
}

fn create_unique_staging(
    parent: &Path,
    name: impl Fn(Uuid) -> String,
    kind: StagingKind,
) -> Result<StagingPath, ManagedSkillInstallerError> {
    for _ in 0..STAGING_ATTEMPTS {
        let path = parent.join(name(Uuid::new_v4()));
        match fs::create_dir(&path) {
            Ok(()) => {
                return Ok(StagingPath {
                    path,
                    parent: parent.to_path_buf(),
                    kind,
                    // create_dir succeeded, so this transaction owns the
                    // exact staging name. Drop may remove its bounded, plain
                    // tree if a later package write or publish step fails.
                    armed: true,
                });
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(ManagedSkillInstallerError::Io {
                    operation: "create managed Skill package staging directory".to_string(),
                    reason: error.to_string(),
                })
            }
        }
    }
    Err(ManagedSkillInstallerError::Io {
        operation: "create unique managed Skill package staging directory".to_string(),
        reason: "exhausted random staging names".to_string(),
    })
}

fn reserve_unique_staging_path(
    parent: &Path,
    name: impl Fn(Uuid) -> String,
    kind: StagingKind,
) -> Result<StagingPath, ManagedSkillInstallerError> {
    for _ in 0..STAGING_ATTEMPTS {
        let path = parent.join(name(Uuid::new_v4()));
        match metadata_if_present(&path) {
            Ok(None) => {
                return Ok(StagingPath {
                    path,
                    parent: parent.to_path_buf(),
                    kind,
                    // A lexical absence check is not ownership. The caller
                    // arms the guard only after create_new (receipt) succeeds.
                    // Tombstones intentionally remain unarmed so a failed
                    // rename can never delete an independently-created target.
                    armed: false,
                });
            }
            Ok(Some(_)) => continue,
            Err(error) => {
                return Err(ManagedSkillInstallerError::Io {
                    operation: "inspect managed Skill staging path".to_string(),
                    reason: error.to_string(),
                })
            }
        }
    }
    Err(ManagedSkillInstallerError::Io {
        operation: "reserve unique managed Skill staging path".to_string(),
        reason: "exhausted random staging names".to_string(),
    })
}

fn load_receipt(
    store: &ManagedSkillStore,
    installation_id: &SkillInstallationId,
) -> Result<Option<InstalledSkillReceipt>, ManagedSkillInstallerError> {
    match store.load_receipt(installation_id) {
        Ok(receipt) => Ok(Some(receipt)),
        Err(ManagedStoreLoadError::NotFound) => Ok(None),
        Err(ManagedStoreLoadError::Invalid(issue)) => {
            Err(ManagedSkillInstallerError::StoreCorrupt {
                reason: issue.message,
            })
        }
        Err(ManagedStoreLoadError::Unavailable(issue)) => Err(ManagedSkillInstallerError::Io {
            operation: "read managed Skill installation receipt".to_string(),
            reason: issue.message,
        }),
    }
}

fn verify_prepared_package(
    store: &ManagedSkillStore,
    package: &PreparedSkillPackage,
) -> Result<(), ManagedSkillInstallerError> {
    let package_ref = InstalledPackageRef::from_format_and_revision(
        package.format_version(),
        package.revision().clone(),
    )
    .map_err(|reason| ManagedSkillInstallerError::StoreCorrupt { reason })?;
    match store.load_complete_package(&package_ref) {
        Ok(snapshot)
            if snapshot.package.bytes == package.source_bytes()
                && snapshot.package.format_version == package.format_version()
                && snapshot.package.resources == package.resource_index()
                && snapshot.resource_bytes.len() == package.resources().len()
                && snapshot.resource_bytes.iter().zip(package.resources()).all(
                    |((path, bytes), prepared)| {
                        path == prepared.descriptor().path() && bytes == prepared.bytes()
                    },
                ) =>
        {
            Ok(())
        }
        Ok(_) => Err(ManagedSkillInstallerError::StoreCorrupt {
            reason: format!(
                "managed Skill package `{}` does not match its prepared byte snapshot",
                package.revision()
            ),
        }),
        Err(ManagedStoreLoadError::NotFound) => Err(ManagedSkillInstallerError::StoreCorrupt {
            reason: format!("managed Skill package `{}` is missing", package.revision()),
        }),
        Err(ManagedStoreLoadError::Invalid(issue)) => {
            Err(ManagedSkillInstallerError::StoreCorrupt {
                reason: issue.message,
            })
        }
        Err(ManagedStoreLoadError::Unavailable(issue)) => Err(ManagedSkillInstallerError::Io {
            operation: "verify managed Skill package".to_string(),
            reason: issue.message,
        }),
    }
}

fn ensure_store_root(root: &Path) -> Result<PathBuf, ManagedSkillInstallerError> {
    let parent = root
        .parent()
        .ok_or_else(|| ManagedSkillInstallerError::InvalidStore {
            reason: "managed Skill store root has no parent directory".to_string(),
        })?;
    match metadata_if_present(root) {
        Ok(Some(_)) => {}
        Ok(None) => match fs::create_dir(root) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(ManagedSkillInstallerError::Io {
                    operation: "create managed Skill store root".to_string(),
                    reason: error.to_string(),
                })
            }
        },
        Err(error) => {
            return Err(ManagedSkillInstallerError::Io {
                operation: "inspect managed Skill store root".to_string(),
                reason: error.to_string(),
            })
        }
    }
    validate_plain_directory(root, "managed Skill store root")?;
    // Repeat the parent barrier even for an existing root so a retry repairs a
    // prior create-success/parent-sync-failure window.
    sync_directory(parent).map_err(|error| ManagedSkillInstallerError::Io {
        operation: "sync managed Skill store parent directory".to_string(),
        reason: error.to_string(),
    })?;
    Ok(root.to_path_buf())
}

fn ensure_exact_directory(
    parent: &Path,
    name: &str,
) -> Result<PathBuf, ManagedSkillInstallerError> {
    let mut exact = directory_has_exact_entry(parent, OsStr::new(name), MAX_LAYOUT_ENTRIES)?;
    let path = parent.join(name);
    if !exact {
        if metadata_if_present(&path)
            .map_err(|error| ManagedSkillInstallerError::Io {
                operation: format!("inspect managed Skill directory `{name}`"),
                reason: error.to_string(),
            })?
            .is_some()
        {
            // Another cooperating initializer may have created the exact
            // entry between our scan and metadata lookup. Rescan before
            // classifying it as a wrong-case alias.
            exact = directory_has_exact_entry(parent, OsStr::new(name), MAX_LAYOUT_ENTRIES)?;
            if !exact {
                return Err(ManagedSkillInstallerError::InvalidStore {
                    reason: format!(
                        "managed Skill directory `{name}` does not use its required exact-case name"
                    ),
                });
            }
        }
        if !exact {
            match fs::create_dir(&path) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => {
                    return Err(ManagedSkillInstallerError::Io {
                        operation: format!("create managed Skill directory `{name}`"),
                        reason: error.to_string(),
                    })
                }
            }
        }
    }
    if !directory_has_exact_entry(parent, OsStr::new(name), MAX_LAYOUT_ENTRIES)? {
        return Err(ManagedSkillInstallerError::InvalidStore {
            reason: format!(
                "managed Skill directory `{name}` does not use its required exact-case name"
            ),
        });
    }
    validate_plain_directory(&path, &format!("managed Skill directory `{name}`"))?;
    let canonical_parent =
        parent
            .canonicalize()
            .map_err(|error| ManagedSkillInstallerError::Io {
                operation: "resolve managed Skill parent directory".to_string(),
                reason: error.to_string(),
            })?;
    let canonical_child = path
        .canonicalize()
        .map_err(|error| ManagedSkillInstallerError::Io {
            operation: format!("resolve managed Skill directory `{name}`"),
            reason: error.to_string(),
        })?;
    if !canonical_child.starts_with(&canonical_parent) {
        return Err(ManagedSkillInstallerError::InvalidStore {
            reason: format!("managed Skill directory `{name}` escapes its parent"),
        });
    }
    // An earlier attempt may have created this directory but failed its parent
    // barrier. Always repeat it before the layout can be used for a commit.
    sync_directory(parent).map_err(|error| ManagedSkillInstallerError::Io {
        operation: format!("sync parent of managed Skill directory `{name}`"),
        reason: error.to_string(),
    })?;
    Ok(path)
}

fn validate_plain_directory(path: &Path, label: &str) -> Result<(), ManagedSkillInstallerError> {
    let metadata = metadata_if_present(path)
        .map_err(|error| ManagedSkillInstallerError::Io {
            operation: format!("inspect {label}"),
            reason: error.to_string(),
        })?
        .ok_or_else(|| ManagedSkillInstallerError::InvalidStore {
            reason: format!("{label} is missing"),
        })?;
    if is_symlink_or_reparse(&metadata) {
        return Err(ManagedSkillInstallerError::InvalidStore {
            reason: format!("{label} cannot be a symlink or reparse point"),
        });
    }
    if !metadata.is_dir() {
        return Err(ManagedSkillInstallerError::InvalidStore {
            reason: format!("{label} is not a directory"),
        });
    }
    Ok(())
}

fn acquire_writer_lock(root: &Path) -> Result<File, ManagedSkillInstallerError> {
    let path = root.join(WRITER_LOCK_FILE);
    let mut exact_name_exists =
        directory_has_exact_entry(root, OsStr::new(WRITER_LOCK_FILE), MAX_LAYOUT_ENTRIES)?;
    if !exact_name_exists
        && metadata_if_present(&path)
            .map_err(|error| ManagedSkillInstallerError::Io {
                operation: "inspect managed Skill writer lock".to_string(),
                reason: error.to_string(),
            })?
            .is_some()
    {
        exact_name_exists =
            directory_has_exact_entry(root, OsStr::new(WRITER_LOCK_FILE), MAX_LAYOUT_ENTRIES)?;
        if !exact_name_exists {
            return Err(ManagedSkillInstallerError::InvalidStore {
                reason: "managed Skill writer lock does not use its required exact-case name"
                    .to_string(),
            });
        }
    }
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
    #[cfg(windows)]
    options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    let file = options
        .open(&path)
        .map_err(|error| ManagedSkillInstallerError::Io {
            operation: "open managed Skill writer lock".to_string(),
            reason: error.to_string(),
        })?;
    if !directory_has_exact_entry(root, OsStr::new(WRITER_LOCK_FILE), MAX_LAYOUT_ENTRIES)? {
        return Err(ManagedSkillInstallerError::InvalidStore {
            reason: "managed Skill writer lock does not use its required exact-case name"
                .to_string(),
        });
    }
    verify_writer_lock_binding(&file, &path, root)?;
    if !directory_has_exact_entry(root, OsStr::new(WRITER_LOCK_FILE), MAX_LAYOUT_ENTRIES)? {
        return Err(ManagedSkillInstallerError::InvalidStore {
            reason: "managed Skill writer lock changed its exact-case name while locking"
                .to_string(),
        });
    }
    file.lock()
        .map_err(|error| ManagedSkillInstallerError::Io {
            operation: "acquire managed Skill writer lock".to_string(),
            reason: error.to_string(),
        })?;
    verify_writer_lock_binding(&file, &path, root)?;
    // Repeating these barriers makes a retry finish a directory or lock-file
    // creation whose prior parent sync failed.
    file.sync_all()
        .map_err(|error| ManagedSkillInstallerError::Io {
            operation: "sync managed Skill writer lock".to_string(),
            reason: error.to_string(),
        })?;
    sync_directory(root).map_err(|error| ManagedSkillInstallerError::Io {
        operation: "sync managed Skill store writer lock entry".to_string(),
        reason: error.to_string(),
    })?;
    Ok(file)
}

fn directory_has_exact_entry(
    parent: &Path,
    name: &OsStr,
    max_entries: usize,
) -> Result<bool, ManagedSkillInstallerError> {
    let entries = fs::read_dir(parent).map_err(|error| ManagedSkillInstallerError::Io {
        operation: format!("read managed Skill directory `{}`", parent.display()),
        reason: error.to_string(),
    })?;
    let mut exact = false;
    for (index, entry) in entries.enumerate() {
        if index >= max_entries {
            return Err(ManagedSkillInstallerError::InvalidStore {
                reason: format!(
                    "managed Skill directory `{}` exceeds {max_entries} entries",
                    parent.display()
                ),
            });
        }
        let entry = entry.map_err(|error| ManagedSkillInstallerError::Io {
            operation: "inspect managed Skill directory entry".to_string(),
            reason: error.to_string(),
        })?;
        if entry.file_name() == name {
            exact = true;
        }
    }
    Ok(exact)
}

fn verify_writer_lock_binding(
    file: &File,
    path: &Path,
    root: &Path,
) -> Result<(), ManagedSkillInstallerError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| ManagedSkillInstallerError::Io {
        operation: "verify managed Skill writer lock path".to_string(),
        reason: error.to_string(),
    })?;
    if is_symlink_or_reparse(&metadata) || !metadata.is_file() {
        return Err(ManagedSkillInstallerError::InvalidStore {
            reason: "managed Skill writer lock must be a plain file".to_string(),
        });
    }
    let opened_metadata = file
        .metadata()
        .map_err(|error| ManagedSkillInstallerError::Io {
            operation: "inspect opened managed Skill writer lock".to_string(),
            reason: error.to_string(),
        })?;
    if !opened_metadata.is_file() {
        return Err(ManagedSkillInstallerError::InvalidStore {
            reason: "opened managed Skill writer lock is not a file".to_string(),
        });
    }
    let canonical_path = path
        .canonicalize()
        .map_err(|error| ManagedSkillInstallerError::Io {
            operation: "resolve managed Skill writer lock".to_string(),
            reason: error.to_string(),
        })?;
    let canonical_root = root
        .canonicalize()
        .map_err(|error| ManagedSkillInstallerError::Io {
            operation: "resolve managed Skill store root".to_string(),
            reason: error.to_string(),
        })?;
    if !canonical_path.starts_with(canonical_root) {
        return Err(ManagedSkillInstallerError::InvalidStore {
            reason: "managed Skill writer lock escapes its store root".to_string(),
        });
    }
    verify_opened_file_identity(file, &metadata, &opened_metadata, &canonical_path).map_err(
        |reason| ManagedSkillInstallerError::InvalidStore {
            reason: format!("managed Skill writer lock changed while opened: {reason}"),
        },
    )
}

fn count_installation_entries(
    installations: &Path,
) -> Result<(usize, usize), ManagedSkillInstallerError> {
    let entries = fs::read_dir(installations).map_err(|error| ManagedSkillInstallerError::Io {
        operation: "count managed Skill installation entries".to_string(),
        reason: error.to_string(),
    })?;
    let mut directory_entries = 0usize;
    let mut live_entries = 0usize;
    for entry in entries {
        let entry = entry.map_err(|error| ManagedSkillInstallerError::Io {
            operation: "inspect managed Skill installation entry while counting capacity"
                .to_string(),
            reason: error.to_string(),
        })?;
        directory_entries = directory_entries.saturating_add(1);
        if directory_entries > MAX_INSTALLATION_DIRECTORY_ENTRIES {
            return Err(ManagedSkillInstallerError::CapacityExceeded {
                capacity: ManagedSkillStoreCapacity::InstallationDirectory,
                limit: MAX_INSTALLATION_DIRECTORY_ENTRIES,
            });
        }
        if !entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.starts_with('.'))
        {
            live_entries = live_entries.saturating_add(1);
        }
    }
    Ok((directory_entries, live_entries))
}

fn ensure_directory_has_room(
    directory: &Path,
    limit: usize,
    capacity: ManagedSkillStoreCapacity,
) -> Result<(), ManagedSkillInstallerError> {
    let count = count_directory_entries(directory, limit, capacity)?;
    ensure_count_has_room(count, limit, capacity)
}

fn count_directory_entries(
    directory: &Path,
    limit: usize,
    capacity: ManagedSkillStoreCapacity,
) -> Result<usize, ManagedSkillInstallerError> {
    let entries = fs::read_dir(directory).map_err(|error| ManagedSkillInstallerError::Io {
        operation: format!("count managed Skill {} entries", capacity.stable_name()),
        reason: error.to_string(),
    })?;
    let mut count = 0usize;
    for entry in entries {
        entry.map_err(|error| ManagedSkillInstallerError::Io {
            operation: format!("inspect managed Skill {} entry", capacity.stable_name()),
            reason: error.to_string(),
        })?;
        count = count.saturating_add(1);
        if count > limit {
            return Err(ManagedSkillInstallerError::CapacityExceeded { capacity, limit });
        }
    }
    Ok(count)
}

fn ensure_count_has_room(
    count: usize,
    limit: usize,
    capacity: ManagedSkillStoreCapacity,
) -> Result<(), ManagedSkillInstallerError> {
    if count >= limit {
        Err(ManagedSkillInstallerError::CapacityExceeded { capacity, limit })
    } else {
        Ok(())
    }
}

fn ensure_retirement_marker_capacity(
    retired_entries: usize,
    live_entries: usize,
    consumes_live_identity: bool,
) -> Result<(), ManagedSkillInstallerError> {
    // Retiring a live identity converts one live slot into one retired slot,
    // while retiring an already-absent identity grows the permanent ledger and
    // must preserve a future slot for every remaining live installation.
    let reserved_entries = retired_entries.saturating_add(live_entries);
    let exhausted = if consumes_live_identity {
        // live -> retired preserves the combined identity count.
        reserved_entries > MAX_RETIRED_INSTALLATION_ENTRIES
    } else {
        // absent -> retired adds one permanent identity.
        reserved_entries >= MAX_RETIRED_INSTALLATION_ENTRIES
    };
    if exhausted {
        Err(ManagedSkillInstallerError::CapacityExceeded {
            capacity: ManagedSkillStoreCapacity::RetiredInstallationIds,
            limit: MAX_RETIRED_INSTALLATION_ENTRIES,
        })
    } else {
        Ok(())
    }
}

fn cleanup_stale_transaction_entries(
    layout: &ManagedStoreLayout,
) -> Result<(), ManagedSkillInstallerError> {
    cleanup_stale_transaction_entries_with_limit(layout, MAX_STAGING_CLEANUP_ENTRIES)
}

fn cleanup_stale_transaction_entries_with_limit(
    layout: &ManagedStoreLayout,
    max_entries: usize,
) -> Result<(), ManagedSkillInstallerError> {
    cleanup_stale_installation_entries_with_limit(layout, max_entries)?;
    for package_version in [&layout.package_v1, &layout.package_v2, &layout.package_v3] {
        cleanup_stale_package_entries_with_limit(package_version, max_entries)?;
    }
    Ok(())
}

fn cleanup_stale_installation_entries_with_limit(
    layout: &ManagedStoreLayout,
    max_entries: usize,
) -> Result<(), ManagedSkillInstallerError> {
    let mut installations_changed = false;
    for (index, entry) in fs::read_dir(&layout.installations)
        .map_err(|error| ManagedSkillInstallerError::Io {
            operation: "scan managed Skill installation staging entries".to_string(),
            reason: error.to_string(),
        })?
        .enumerate()
    {
        if index >= max_entries {
            return Err(ManagedSkillInstallerError::InvalidStore {
                reason: format!(
                    "managed Skill installations directory exceeds the {max_entries}-entry staging cleanup budget"
                ),
            });
        }
        let entry = entry.map_err(|error| ManagedSkillInstallerError::Io {
            operation: "inspect managed Skill installation staging entry".to_string(),
            reason: error.to_string(),
        })?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if let Some(installation_id) = canonical_receipt_installation_id(&name) {
            let retired_path = layout.retired_path(&installation_id);
            if let Some(retired_metadata) = metadata_if_present(&retired_path).map_err(|error| {
                ManagedSkillInstallerError::Io {
                    operation: "inspect retired identity during writer recovery".to_string(),
                    reason: error.to_string(),
                }
            })? {
                if is_symlink_or_reparse(&retired_metadata) || !retired_metadata.is_file() {
                    return Err(ManagedSkillInstallerError::StoreCorrupt {
                        reason: format!(
                            "retired managed Skill installation identity `{installation_id}` is not a plain file"
                        ),
                    });
                }
                let live_metadata = match fs::symlink_metadata(entry.path()) {
                    Ok(metadata) => metadata,
                    // A legacy tombstone for the same identity may have been
                    // processed earlier from the same buffered read_dir
                    // snapshot and already removed this live entry.
                    Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                    Err(error) => {
                        return Err(ManagedSkillInstallerError::Io {
                            operation: "inspect live receipt during retirement recovery"
                                .to_string(),
                            reason: error.to_string(),
                        })
                    }
                };
                if is_symlink_or_reparse(&live_metadata) || !live_metadata.is_file() {
                    return Err(ManagedSkillInstallerError::StoreCorrupt {
                        reason: format!(
                            "live managed Skill receipt for retired installation `{installation_id}` is not a plain file"
                        ),
                    });
                }
                // Make the already-visible retirement marker durable before
                // deleting a live name that may have reappeared after a crash.
                sync_directory(&layout.retired_installations).map_err(|error| {
                    ManagedSkillInstallerError::Io {
                        operation: "sync retired identity during writer recovery".to_string(),
                        reason: error.to_string(),
                    }
                })?;
                fs::remove_file(entry.path()).map_err(|error| ManagedSkillInstallerError::Io {
                    operation: "remove live receipt for retired installation".to_string(),
                    reason: error.to_string(),
                })?;
                installations_changed = true;
            }
            continue;
        }
        let retired_installation_id = owned_tombstone_installation_id(&name);
        if !is_owned_receipt_staging_name(&name) && retired_installation_id.is_none() {
            continue;
        }
        let metadata =
            fs::symlink_metadata(entry.path()).map_err(|error| ManagedSkillInstallerError::Io {
                operation: "inspect stale managed Skill receipt staging entry".to_string(),
                reason: error.to_string(),
            })?;
        if is_symlink_or_reparse(&metadata) || !metadata.is_file() {
            return Err(ManagedSkillInstallerError::StoreCorrupt {
                reason: format!("owned receipt staging entry `{name}` is not a plain file"),
            });
        }
        if let Some(installation_id) = retired_installation_id {
            let retired_path = layout.retired_path(&installation_id);
            let live_path = layout.receipt_path(&installation_id);
            let live_receipt_present = match metadata_if_present(&live_path).map_err(|error| {
                ManagedSkillInstallerError::Io {
                    operation: "inspect live receipt before tombstone migration".to_string(),
                    reason: error.to_string(),
                }
            })? {
                Some(live_metadata)
                    if is_symlink_or_reparse(&live_metadata) || !live_metadata.is_file() =>
                {
                    return Err(ManagedSkillInstallerError::StoreCorrupt {
                        reason: format!(
                            "live managed Skill receipt for retired installation `{installation_id}` is not a plain file"
                        ),
                    });
                }
                Some(_) => true,
                None => false,
            };
            let tombstone_moved = match metadata_if_present(&retired_path) {
                Ok(Some(retired_metadata))
                    if is_symlink_or_reparse(&retired_metadata) || !retired_metadata.is_file() =>
                {
                    return Err(ManagedSkillInstallerError::StoreCorrupt {
                        reason: format!(
                            "retired managed Skill installation identity `{installation_id}` is not a plain file"
                        ),
                    });
                }
                Ok(Some(_)) => false,
                Ok(None) => {
                    let retired_entries = count_directory_entries(
                        &layout.retired_installations,
                        MAX_RETIRED_INSTALLATION_ENTRIES,
                        ManagedSkillStoreCapacity::RetiredInstallationIds,
                    )?;
                    let (_, live_entries) = count_installation_entries(&layout.installations)?;
                    ensure_retirement_marker_capacity(
                        retired_entries,
                        live_entries,
                        live_receipt_present,
                    )?;
                    atomic_rename_noreplace(&entry.path(), &retired_path).map_err(|error| {
                        ManagedSkillInstallerError::Io {
                            operation: "migrate uninstall tombstone into the retired-ID ledger"
                                .to_string(),
                            reason: error.to_string(),
                        }
                    })?;
                    true
                }
                Err(error) => {
                    return Err(ManagedSkillInstallerError::Io {
                        operation: "inspect retired managed Skill identity during migration"
                            .to_string(),
                        reason: error.to_string(),
                    })
                }
            };
            // A legacy tombstone is proof of an uninstall intent. Durably
            // publish/migrate it before removing any live receipt left by an
            // interrupted cross-directory transaction.
            sync_directory(&layout.retired_installations).map_err(|error| {
                ManagedSkillInstallerError::Io {
                    operation: "sync retired identity after tombstone migration".to_string(),
                    reason: error.to_string(),
                }
            })?;
            if let Some(live_metadata) =
                metadata_if_present(&live_path).map_err(|error| ManagedSkillInstallerError::Io {
                    operation: "inspect live receipt during tombstone migration".to_string(),
                    reason: error.to_string(),
                })?
            {
                if is_symlink_or_reparse(&live_metadata) || !live_metadata.is_file() {
                    return Err(ManagedSkillInstallerError::StoreCorrupt {
                        reason: format!(
                            "live managed Skill receipt for retired installation `{installation_id}` is not a plain file"
                        ),
                    });
                }
                fs::remove_file(&live_path).map_err(|error| ManagedSkillInstallerError::Io {
                    operation: "remove live receipt during tombstone migration".to_string(),
                    reason: error.to_string(),
                })?;
            }
            if !tombstone_moved {
                fs::remove_file(entry.path()).map_err(|error| ManagedSkillInstallerError::Io {
                    operation: "remove superseded uninstall tombstone".to_string(),
                    reason: error.to_string(),
                })?;
            }
        } else {
            fs::remove_file(entry.path()).map_err(|error| ManagedSkillInstallerError::Io {
                operation: "remove stale managed Skill receipt staging entry".to_string(),
                reason: error.to_string(),
            })?;
        }
        installations_changed = true;
    }
    if installations_changed {
        sync_directory(&layout.installations).map_err(|error| ManagedSkillInstallerError::Io {
            operation: "sync installations after staging cleanup".to_string(),
            reason: error.to_string(),
        })?;
    }

    Ok(())
}

fn cleanup_stale_package_entries_with_limit(
    package_version: &Path,
    max_entries: usize,
) -> Result<(), ManagedSkillInstallerError> {
    let mut directory_changed = false;
    for (index, entry) in fs::read_dir(package_version)
        .map_err(|error| ManagedSkillInstallerError::Io {
            operation: "scan managed Skill package staging entries".to_string(),
            reason: error.to_string(),
        })?
        .enumerate()
    {
        if index >= max_entries {
            return Err(ManagedSkillInstallerError::InvalidStore {
                reason: format!(
                    "managed Skill package version directory exceeds the {max_entries}-entry staging cleanup budget"
                ),
            });
        }
        let entry = entry.map_err(|error| ManagedSkillInstallerError::Io {
            operation: "inspect managed Skill package staging entry".to_string(),
            reason: error.to_string(),
        })?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if !is_owned_package_staging_name(&name) {
            continue;
        }
        match remove_owned_package_staging_directory(package_version, &entry.path()) {
            Ok(true) => directory_changed = true,
            Ok(false) => {
                return Err(ManagedSkillInstallerError::StoreCorrupt {
                    reason: format!(
                        "owned package staging entry `{name}` escaped its package version directory"
                    ),
                })
            }
            Err(error) if error.kind() == io::ErrorKind::InvalidData => {
                return Err(ManagedSkillInstallerError::StoreCorrupt {
                    reason: format!("owned package staging entry `{name}` is invalid: {error}"),
                })
            }
            Err(error) => {
                return Err(ManagedSkillInstallerError::Io {
                    operation: "remove stale managed Skill package staging entry".to_string(),
                    reason: error.to_string(),
                })
            }
        }
    }
    if directory_changed {
        sync_directory(package_version).map_err(|error| ManagedSkillInstallerError::Io {
            operation: "sync package version directory after staging cleanup".to_string(),
            reason: error.to_string(),
        })?;
    }
    Ok(())
}

fn remove_owned_package_staging_directory(parent: &Path, path: &Path) -> io::Result<bool> {
    if path.parent() != Some(parent)
        || !path
            .file_name()
            .and_then(OsStr::to_str)
            .is_some_and(is_owned_package_staging_name)
    {
        return Ok(false);
    }
    let metadata = fs::symlink_metadata(path)?;
    if is_symlink_or_reparse(&metadata) || !metadata.is_dir() {
        return Err(invalid_staging_tree(
            "the staging root is not a plain directory",
        ));
    }
    validate_package_staging_tree(path)?;
    // std::fs::remove_dir_all does not follow directory symlinks. The
    // validation above additionally rejects every link/reparse point and
    // bounds the tree before any recursive deletion starts.
    fs::remove_dir_all(path)?;
    Ok(true)
}

fn validate_package_staging_tree(root: &Path) -> io::Result<()> {
    let mut pending = vec![(root.to_path_buf(), 0_usize)];
    let mut total_entries = 0_usize;
    while let Some((directory, depth)) = pending.pop() {
        let mut directory_entries = 0_usize;
        for entry in fs::read_dir(&directory)? {
            directory_entries = directory_entries.saturating_add(1);
            if directory_entries > MAX_SKILL_PACKAGE_DIRECTORY_ENTRIES + 1 {
                return Err(invalid_staging_tree(
                    "a directory exceeds the package staging entry limit",
                ));
            }
            total_entries = total_entries.saturating_add(1);
            if total_entries > MAX_PACKAGE_STAGING_TREE_ENTRIES {
                return Err(invalid_staging_tree(
                    "the package staging tree exceeds its entry limit",
                ));
            }
            let entry = entry?;
            let metadata = fs::symlink_metadata(entry.path())?;
            if is_symlink_or_reparse(&metadata) {
                return Err(invalid_staging_tree(
                    "the package staging tree contains a link or reparse point",
                ));
            }
            if metadata.is_dir() {
                if depth >= MAX_SKILL_PACKAGE_DEPTH {
                    return Err(invalid_staging_tree(
                        "the package staging tree exceeds its depth limit",
                    ));
                }
                pending.push((entry.path(), depth + 1));
            } else if !metadata.is_file() {
                return Err(invalid_staging_tree(
                    "the package staging tree contains a non-file entry",
                ));
            }
        }
    }
    Ok(())
}

fn invalid_staging_tree(reason: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, reason)
}

fn is_owned_receipt_staging_name(name: &str) -> bool {
    parse_two_uuid_name(name, ".receipt-", ".tmp")
}

fn canonical_receipt_installation_id(name: &str) -> Option<SkillInstallationId> {
    SkillInstallationId::parse(name.strip_suffix(".json")?).ok()
}

fn is_owned_package_staging_name(name: &str) -> bool {
    let Some(body) = name
        .strip_prefix(".package-")
        .and_then(|value| value.strip_suffix(".tmp"))
    else {
        return false;
    };
    if body.len() != 64 + 1 + 36 || body.as_bytes().get(64) != Some(&b'-') {
        return false;
    }
    body[..64]
        .bytes()
        .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        && Uuid::parse_str(&body[65..])
            .is_ok_and(|uuid| uuid.hyphenated().to_string() == body[65..])
}

fn owned_tombstone_installation_id(name: &str) -> Option<SkillInstallationId> {
    let body = name
        .strip_prefix(".uninstall-")?
        .strip_suffix(".tombstone")?;
    if body.len() != 36 + 1 + 36 || body.as_bytes().get(36) != Some(&b'-') {
        return None;
    }
    let installation_id = SkillInstallationId::parse(&body[..36]).ok()?;
    Uuid::parse_str(&body[37..])
        .is_ok_and(|uuid| uuid.hyphenated().to_string() == body[37..])
        .then_some(installation_id)
}

fn parse_two_uuid_name(name: &str, prefix: &str, suffix: &str) -> bool {
    let Some(body) = name
        .strip_prefix(prefix)
        .and_then(|value| value.strip_suffix(suffix))
    else {
        return false;
    };
    if body.len() != 36 + 1 + 36 || body.as_bytes().get(36) != Some(&b'-') {
        return false;
    }
    SkillInstallationId::parse(&body[..36]).is_ok()
        && Uuid::parse_str(&body[37..])
            .is_ok_and(|uuid| uuid.hyphenated().to_string() == body[37..])
}

fn unix_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::digest::{
        PACKAGE_REVISION_PREFIX, PACKAGE_REVISION_V2_PREFIX, PACKAGE_REVISION_V3_PREFIX,
    };
    use crate::skills::{
        SkillPackageOrigin, SkillSourceKind, SkillsService, USER_INSTALLED_SKILL_SOURCE_ID,
    };
    use std::sync::{Arc, Barrier};
    use tempfile::tempdir;

    const INSTALLATION_ID: &str = "01234567-89ab-4def-8123-456789abcdef";
    const SECOND_INSTALLATION_ID: &str = "11111111-2222-4333-8444-555555555555";

    fn installation_id(value: &str) -> SkillInstallationId {
        SkillInstallationId::parse(value).unwrap()
    }

    fn package(name: &str, marker: &str) -> PreparedSkillPackage {
        PreparedSkillPackage::from_bytes(
            format!(
                "---\nname: {name}\ndescription: Managed fixture {name}.\n---\n# Instructions\n{marker}\n"
            )
            .into_bytes(),
            SkillPackageOrigin::new("local-directory", format!("fixture:{name}")).unwrap(),
        )
        .unwrap()
    }

    fn package_v2(name: &str, marker: &str, resource: &[u8]) -> PreparedSkillPackage {
        PreparedSkillPackage::from_files(
            vec![
                (
                    SKILL_FILE_NAME.to_string(),
                    format!(
                        "---\nname: {name}\ndescription: Managed fixture {name}.\n---\n# Instructions\n{marker}\n"
                    )
                    .into_bytes(),
                ),
                ("references/guide.md".to_string(), resource.to_vec()),
                ("assets/data.bin".to_string(), vec![0, 1, 2, 255]),
            ],
            SkillPackageOrigin::new("local-directory", format!("fixture:{name}")).unwrap(),
        )
        .unwrap()
    }

    fn package_v3(name: &str, marker: &str) -> PreparedSkillPackage {
        PreparedSkillPackage::from_files(
            vec![
                (
                    SKILL_FILE_NAME.to_string(),
                    format!(
                        "---\nname: {name}\ndescription: Managed fixture {name}.\n---\n# Instructions\n{marker}\n"
                    )
                    .into_bytes(),
                ),
                ("README.md".to_string(), b"ROOT_RESOURCE".to_vec()),
                (
                    "agents/openai.yaml".to_string(),
                    b"interface: chat".to_vec(),
                ),
                ("references/guide.md".to_string(), b"GUIDE".to_vec()),
            ],
            SkillPackageOrigin::new("local-directory", format!("fixture:{name}")).unwrap(),
        )
        .unwrap()
    }

    fn provenance(authority: &str, refresh: Option<&str>) -> SkillInstallationProvenance {
        use crate::skills::{SkillInstallationAuthority, SkillInstallationRefresh};

        SkillInstallationProvenance::new(
            SkillInstallationAuthority::new("fixture", 1, authority).unwrap(),
            refresh.map(|payload| SkillInstallationRefresh::new("fixture", 1, payload).unwrap()),
        )
    }

    fn install_request(id: &str, package: PreparedSkillPackage) -> ManagedSkillInstallRequest {
        let mut request =
            ManagedSkillInstallRequest::with_installation_id(installation_id(id), package);
        request.set_created_at_unix_ms(1_784_347_513_399);
        request
    }

    fn install_request_with_provenance(
        id: &str,
        package: PreparedSkillPackage,
        provenance: SkillInstallationProvenance,
    ) -> ManagedSkillInstallRequest {
        let mut request = ManagedSkillInstallRequest::with_installation_id_and_provenance(
            installation_id(id),
            package,
            provenance,
        );
        request.set_created_at_unix_ms(1_784_347_513_399);
        request
    }

    fn installed_catalog(root: &Path) -> crate::skills::SkillCatalog {
        SkillsService::new()
            .with_installed_source(root)
            .unwrap()
            .list()
            .unwrap()
    }

    fn installed_receipt(root: &Path, id: &SkillInstallationId) -> InstalledSkillReceipt {
        ManagedSkillStore::new(root)
            .unwrap()
            .load_receipt(id)
            .unwrap()
    }

    fn installation_revision(root: &Path, id: &SkillInstallationId) -> SkillInstallationRevision {
        installed_receipt(root, id).installation_revision
    }

    fn package_path(root: &Path, revision: &SkillRevision) -> PathBuf {
        root.join(PACKAGES_DIRECTORY)
            .join(PACKAGE_V1_DIRECTORY)
            .join(
                revision
                    .as_str()
                    .strip_prefix(PACKAGE_REVISION_PREFIX)
                    .unwrap(),
            )
    }

    fn package_staging_entries(root: &Path) -> Vec<PathBuf> {
        let packages = root.join(PACKAGES_DIRECTORY);
        [
            PACKAGE_V1_DIRECTORY,
            PACKAGE_V2_DIRECTORY,
            PACKAGE_V3_DIRECTORY,
        ]
        .into_iter()
        .flat_map(|version| {
            fs::read_dir(packages.join(version))
                .into_iter()
                .flatten()
                .filter_map(Result::ok)
                .filter(|entry| {
                    entry
                        .file_name()
                        .to_str()
                        .is_some_and(is_owned_package_staging_name)
                })
                .map(|entry| entry.path())
                .collect::<Vec<_>>()
        })
        .collect()
    }

    #[test]
    fn install_update_and_uninstall_have_stable_idempotent_semantics() {
        let fixture = tempdir().unwrap();
        let root = fixture.path().join("store");
        let installer = ManagedSkillInstaller::new(&root).unwrap();
        let original = package("auditor", "ORIGINAL");
        let original_revision = original.revision().clone();
        let request = install_request(INSTALLATION_ID, original.clone());

        assert_eq!(
            installer.install(&request).unwrap(),
            ManagedSkillInstallOutcome::Installed
        );
        assert_eq!(
            installer.install(&request).unwrap(),
            ManagedSkillInstallOutcome::AlreadyInstalled
        );
        let receipt = ManagedSkillStore::new(&root)
            .unwrap()
            .load_receipt(request.installation_id())
            .unwrap();
        assert_eq!(receipt.installed_at_unix_ms, 1_784_347_513_399);
        assert_eq!(receipt.package.revision, original_revision);
        let original_installation_revision = receipt.installation_revision.clone();

        let second = install_request(SECOND_INSTALLATION_ID, original);
        assert_eq!(
            installer.install(&second).unwrap(),
            ManagedSkillInstallOutcome::Installed
        );
        assert_eq!(installed_catalog(&root).skills().len(), 2);

        let updated = package("auditor", "UPDATED");
        let updated_revision = updated.revision().clone();
        let update = ManagedSkillUpdateRequest::new(
            request.installation_id().clone(),
            original_installation_revision.clone(),
            updated,
        );
        assert_eq!(
            installer.update(&update).unwrap(),
            ManagedSkillUpdateOutcome::Updated
        );
        assert_eq!(
            installer.update(&update).unwrap(),
            ManagedSkillUpdateOutcome::AlreadyCurrent
        );
        let receipt = ManagedSkillStore::new(&root)
            .unwrap()
            .load_receipt(request.installation_id())
            .unwrap();
        assert_eq!(receipt.package.revision, updated_revision);
        let updated_installation_revision = receipt.installation_revision.clone();
        assert_eq!(
            receipt.installed_at_unix_ms, 1_784_347_513_399,
            "updates preserve the original installation timestamp"
        );

        let conflict = ManagedSkillUpdateRequest::new(
            request.installation_id().clone(),
            original_installation_revision.clone(),
            package("auditor", "CONFLICT"),
        );
        assert_eq!(
            installer.update(&conflict).unwrap_err().code(),
            ManagedSkillInstallerErrorCode::RevisionConflict
        );
        let wrong_uninstall = ManagedSkillUninstallRequest::new(
            request.installation_id().clone(),
            original_installation_revision,
        );
        assert_eq!(
            installer.uninstall(&wrong_uninstall).unwrap_err().code(),
            ManagedSkillInstallerErrorCode::RevisionConflict
        );

        let uninstall = ManagedSkillUninstallRequest::new(
            request.installation_id().clone(),
            updated_installation_revision,
        );
        assert_eq!(
            installer.uninstall(&uninstall).unwrap(),
            ManagedSkillUninstallOutcome::Uninstalled
        );
        assert_eq!(
            installer.uninstall(&uninstall).unwrap(),
            ManagedSkillUninstallOutcome::AlreadyAbsent
        );
        let retired = root
            .join(RETIRED_INSTALLATIONS_DIRECTORY)
            .join(format!("{}.json", request.installation_id()));
        assert!(retired.is_file());
        assert_eq!(
            ManagedSkillInstaller::new(&root)
                .unwrap()
                .install(&request)
                .unwrap_err()
                .code(),
            ManagedSkillInstallerErrorCode::InstallationRetired,
            "an old install retry must not resurrect an explicitly uninstalled Skill"
        );
        assert!(package_path(&root, &updated_revision).is_dir());
        let catalog = installed_catalog(&root);
        assert_eq!(catalog.skills().len(), 1);
        assert_eq!(
            catalog.skills()[0].source_kind(),
            SkillSourceKind::Installed
        );
        assert!(catalog.skills()[0]
            .id()
            .as_str()
            .starts_with(USER_INSTALLED_SKILL_SOURCE_ID));
    }

    #[test]
    fn legacy_uninstall_atomically_checks_package_revision_and_retires_absent_ids() {
        let fixture = tempdir().unwrap();
        let root = fixture.path().join("store");
        let installer = ManagedSkillInstaller::new(&root).unwrap();
        let current_package = package("auditor", "CURRENT");
        let current_revision = current_package.revision().clone();
        let install = install_request(INSTALLATION_ID, current_package);
        installer.install(&install).unwrap();

        let wrong_revision = package("auditor", "WRONG").revision().clone();
        let conflict_request = ManagedSkillLegacyUninstallRequest::new(
            install.installation_id().clone(),
            wrong_revision.clone(),
        );
        let conflict = installer.uninstall_legacy(&conflict_request).unwrap_err();
        assert!(matches!(
            conflict,
            ManagedSkillInstallerError::LegacyPackageRevisionConflict {
                ref installation_id,
                ref expected_revision,
                ref actual_revision,
            } if installation_id == install.installation_id()
                && expected_revision == &wrong_revision
                && actual_revision == &current_revision
        ));
        assert_eq!(
            installed_receipt(&root, install.installation_id())
                .package
                .revision,
            current_revision
        );
        assert!(!root
            .join(RETIRED_INSTALLATIONS_DIRECTORY)
            .join(format!("{}.json", install.installation_id()))
            .exists());

        let uninstall = ManagedSkillLegacyUninstallRequest::new(
            install.installation_id().clone(),
            current_revision,
        );
        assert_eq!(
            installer.uninstall_legacy(&uninstall).unwrap(),
            ManagedSkillUninstallOutcome::Uninstalled
        );
        assert_eq!(
            installer.uninstall_legacy(&uninstall).unwrap(),
            ManagedSkillUninstallOutcome::AlreadyAbsent
        );

        let absent_id = installation_id(SECOND_INSTALLATION_ID);
        let absent = ManagedSkillLegacyUninstallRequest::new(
            absent_id.clone(),
            package("absent", "EXPECTED").revision().clone(),
        );
        assert_eq!(
            installer.uninstall_legacy(&absent).unwrap(),
            ManagedSkillUninstallOutcome::AlreadyAbsent
        );
        assert_eq!(
            installer.uninstall_legacy(&absent).unwrap(),
            ManagedSkillUninstallOutcome::AlreadyAbsent
        );
        assert!(root
            .join(RETIRED_INSTALLATIONS_DIRECTORY)
            .join(format!("{absent_id}.json"))
            .is_file());
        assert_eq!(
            installer
                .install(&install_request(
                    SECOND_INSTALLATION_ID,
                    package("absent", "EXPECTED"),
                ))
                .unwrap_err()
                .code(),
            ManagedSkillInstallerErrorCode::InstallationRetired
        );
    }

    #[test]
    fn provenance_only_update_advances_generation_and_identical_retry_is_inert() {
        let fixture = tempdir().unwrap();
        let root = fixture.path().join("store");
        let installer = ManagedSkillInstaller::new(&root).unwrap();
        let package = package("auditor", "SAME_BYTES");
        let install = install_request_with_provenance(
            INSTALLATION_ID,
            package.clone(),
            provenance("commit-a", Some("main")),
        );
        let installed = installer.install(&install).unwrap();
        assert_eq!(installed, ManagedSkillInstallOutcome::Installed);
        let before = installed_receipt(&root, install.installation_id());
        let conflicting_install = install_request_with_provenance(
            INSTALLATION_ID,
            package.clone(),
            provenance("commit-b", Some("main")),
        );
        assert_eq!(
            installer.install(&conflicting_install).unwrap_err().code(),
            ManagedSkillInstallerErrorCode::InstallationExists
        );

        let update = ManagedSkillUpdateRequest::with_provenance(
            install.installation_id().clone(),
            before.installation_revision.clone(),
            package,
            provenance("commit-b", Some("main")),
        );
        let updated = installer.update(&update).unwrap();
        assert_eq!(updated, ManagedSkillUpdateOutcome::Updated);
        let after = installed_receipt(&root, install.installation_id());
        assert_eq!(after.package.revision, before.package.revision);
        assert_eq!(after.generation, 2);
        assert_ne!(after.installation_revision, before.installation_revision);
        assert_eq!(after.installed_at_unix_ms, before.installed_at_unix_ms);
        assert!(after.updated_at_unix_ms >= before.installed_at_unix_ms);
        assert_eq!(after.provenance, *update.provenance());

        let retry = installer.update(&update).unwrap();
        assert_eq!(retry, ManagedSkillUpdateOutcome::AlreadyCurrent);
        let retried = installed_receipt(&root, install.installation_id());
        assert_eq!(retried.generation, after.generation);
        assert_eq!(retried.installation_revision, after.installation_revision);
        assert_eq!(retried.updated_at_unix_ms, after.updated_at_unix_ms);
    }

    #[test]
    fn installation_generation_prevents_aba_from_authorizing_a_different_target() {
        let fixture = tempdir().unwrap();
        let root = fixture.path().join("store");
        let installer = ManagedSkillInstaller::new(&root).unwrap();
        let package = package("auditor", "SAME_BYTES");
        let install = install_request_with_provenance(
            INSTALLATION_ID,
            package.clone(),
            provenance("authority-a", None),
        );
        installer.install(&install).unwrap();
        let generation_one = installed_receipt(&root, install.installation_id());

        let to_b = ManagedSkillUpdateRequest::with_provenance(
            install.installation_id().clone(),
            generation_one.installation_revision.clone(),
            package.clone(),
            provenance("authority-b", None),
        );
        installer.update(&to_b).unwrap();
        let generation_two = installed_receipt(&root, install.installation_id());
        let back_to_a = ManagedSkillUpdateRequest::with_provenance(
            install.installation_id().clone(),
            generation_two.installation_revision,
            package.clone(),
            provenance("authority-a", None),
        );
        installer.update(&back_to_a).unwrap();
        let generation_three = installed_receipt(&root, install.installation_id());
        assert_eq!(generation_three.generation, 3);
        assert_ne!(
            generation_three.installation_revision,
            generation_one.installation_revision
        );

        let stale_to_c = ManagedSkillUpdateRequest::with_provenance(
            install.installation_id().clone(),
            generation_one.installation_revision,
            package,
            provenance("authority-c", None),
        );
        let error = installer.update(&stale_to_c).unwrap_err();
        assert_eq!(
            error.code(),
            ManagedSkillInstallerErrorCode::RevisionConflict
        );
        assert_eq!(
            installed_receipt(&root, install.installation_id()).installation_revision,
            generation_three.installation_revision
        );
    }

    #[test]
    fn updating_a_v1_receipt_naturally_migrates_it_to_v2() {
        let fixture = tempdir().unwrap();
        let root = fixture.path().join("store");
        let installer = ManagedSkillInstaller::new(&root).unwrap();
        let package = package("auditor", "LEGACY_BYTES");
        let install = install_request(INSTALLATION_ID, package.clone());
        installer.install(&install).unwrap();
        let legacy = serde_json::json!({
            "schemaVersion": 1,
            "installationId": INSTALLATION_ID,
            "package": {
                "formatVersion": package.format_version(),
                "revision": package.revision().as_str(),
                "entrypoint": "SKILL.md"
            },
            "origin": {
                "provider": package.origin().provider(),
                "reference": package.origin().reference()
            },
            "installedAtUnixMs": 1_784_347_513_399_u64
        });
        fs::write(
            root.join(INSTALLATIONS_DIRECTORY)
                .join(format!("{INSTALLATION_ID}.json")),
            serde_json::to_vec(&legacy).unwrap(),
        )
        .unwrap();
        let before = installed_receipt(&root, install.installation_id());
        assert!(before.is_legacy_v1());
        assert_eq!(before.generation, 1);

        let update = ManagedSkillUpdateRequest::new(
            install.installation_id().clone(),
            before.installation_revision.clone(),
            package,
        );
        assert_eq!(
            installer.update(&update).unwrap(),
            ManagedSkillUpdateOutcome::Updated
        );
        let after = installed_receipt(&root, install.installation_id());
        assert!(!after.is_legacy_v1());
        assert_eq!(after.receipt_schema_version, 2);
        assert_eq!(after.generation, 2);
        assert_ne!(after.installation_revision, before.installation_revision);
        assert_eq!(after.installed_at_unix_ms, before.installed_at_unix_ms);
    }

    #[test]
    fn exhausted_generation_fails_closed_without_replacing_the_receipt() {
        let fixture = tempdir().unwrap();
        let root = fixture.path().join("store");
        let installer = ManagedSkillInstaller::new(&root).unwrap();
        let package = package("auditor", "MAX_GENERATION");
        let install = install_request_with_provenance(
            INSTALLATION_ID,
            package.clone(),
            provenance("authority-a", None),
        );
        installer.install(&install).unwrap();
        let max_generation = InstalledSkillReceipt::new_v2(
            install.installation_id().clone(),
            u64::MAX,
            package.format_version(),
            package.revision().clone(),
            install.provenance().clone(),
            1_784_347_513_399,
            1_784_347_513_399,
        )
        .unwrap();
        fs::write(
            root.join(INSTALLATIONS_DIRECTORY)
                .join(format!("{INSTALLATION_ID}.json")),
            encode_receipt_v2(&max_generation).unwrap(),
        )
        .unwrap();

        let update = ManagedSkillUpdateRequest::with_provenance(
            install.installation_id().clone(),
            max_generation.installation_revision.clone(),
            package,
            provenance("authority-b", None),
        );
        assert_eq!(
            installer.update(&update).unwrap_err().code(),
            ManagedSkillInstallerErrorCode::StoreCorrupt
        );
        let still_current = installed_receipt(&root, install.installation_id());
        assert_eq!(still_current.generation, u64::MAX);
        assert_eq!(
            still_current.installation_revision,
            max_generation.installation_revision
        );
    }

    #[test]
    fn v2_install_update_and_v1_downgrade_share_the_existing_transaction() {
        let fixture = tempdir().unwrap();
        let root = fixture.path().join("store");
        let installer = ManagedSkillInstaller::new(&root).unwrap();
        let original = package("resourceful", "V1_INSTRUCTIONS");
        let install = install_request(INSTALLATION_ID, original);
        installer.install(&install).unwrap();
        let original_installation_revision =
            installation_revision(&root, install.installation_id());

        let resourceful = package_v2("resourceful", "V2_INSTRUCTIONS", b"GUIDE_V2");
        assert_eq!(
            resourceful.format_version(),
            SKILL_PACKAGE_FORMAT_VERSION_V2
        );
        assert!(resourceful
            .revision()
            .as_str()
            .starts_with(PACKAGE_REVISION_V2_PREFIX));
        let v2_revision = resourceful.revision().clone();
        let update = ManagedSkillUpdateRequest::new(
            install.installation_id().clone(),
            original_installation_revision,
            resourceful.clone(),
        );
        assert_eq!(
            installer.update(&update).unwrap(),
            ManagedSkillUpdateOutcome::Updated
        );
        assert_eq!(
            installer.update(&update).unwrap(),
            ManagedSkillUpdateOutcome::AlreadyCurrent
        );
        let v2_installation_revision = installation_revision(&root, install.installation_id());

        let digest = v2_revision
            .as_str()
            .strip_prefix(PACKAGE_REVISION_V2_PREFIX)
            .unwrap();
        let v2_root = root
            .join(PACKAGES_DIRECTORY)
            .join(PACKAGE_V2_DIRECTORY)
            .join(digest);
        assert_eq!(
            fs::read(v2_root.join("references/guide.md")).unwrap(),
            b"GUIDE_V2"
        );
        assert!(v2_root.join(PACKAGE_MANIFEST_FILE).is_file());

        let service = SkillsService::new().with_installed_source(&root).unwrap();
        let catalog = service.list().unwrap();
        let resolved = service.resolve(&catalog.skills()[0].selection()).unwrap();
        assert_eq!(resolved.format_version(), SKILL_PACKAGE_FORMAT_VERSION_V2);
        assert_eq!(resolved.resources().len(), 2);
        assert_eq!(
            resolved
                .resources()
                .get("references/guide.md")
                .unwrap()
                .byte_length(),
            8
        );

        let downgraded = package("resourceful", "V1_AGAIN");
        let downgrade_revision = downgraded.revision().clone();
        let downgrade = ManagedSkillUpdateRequest::new(
            install.installation_id().clone(),
            v2_installation_revision,
            downgraded,
        );
        assert_eq!(
            installer.update(&downgrade).unwrap(),
            ManagedSkillUpdateOutcome::Updated
        );
        let catalog = service.list().unwrap();
        assert_eq!(catalog.skills()[0].revision(), &downgrade_revision);
        let resolved = service.resolve(&catalog.skills()[0].selection()).unwrap();
        assert_eq!(resolved.format_version(), SKILL_PACKAGE_FORMAT_VERSION);
        assert!(resolved.resources().is_empty());
    }

    #[test]
    fn v3_install_roundtrips_generic_resources_through_the_managed_store() {
        let fixture = tempdir().unwrap();
        let root = fixture.path().join("store");
        let installer = ManagedSkillInstaller::new(&root).unwrap();
        let package = package_v3("portable", "USE_ALL_FILES");
        assert_eq!(package.format_version(), SKILL_PACKAGE_FORMAT_VERSION_V3);
        assert!(package
            .revision()
            .as_str()
            .starts_with(PACKAGE_REVISION_V3_PREFIX));
        let request = install_request(INSTALLATION_ID, package.clone());

        assert_eq!(
            installer.install(&request).unwrap(),
            ManagedSkillInstallOutcome::Installed
        );
        assert_eq!(
            installer.install(&request).unwrap(),
            ManagedSkillInstallOutcome::AlreadyInstalled
        );

        let digest = package
            .revision()
            .as_str()
            .strip_prefix(PACKAGE_REVISION_V3_PREFIX)
            .unwrap();
        let package_root = root
            .join(PACKAGES_DIRECTORY)
            .join(PACKAGE_V3_DIRECTORY)
            .join(digest);
        assert_eq!(
            fs::read(package_root.join("README.md")).unwrap(),
            b"ROOT_RESOURCE"
        );
        assert_eq!(
            fs::read(package_root.join("agents/openai.yaml")).unwrap(),
            b"interface: chat"
        );
        assert!(package_root.join(PACKAGE_MANIFEST_FILE).is_file());

        let service = SkillsService::new().with_installed_source(&root).unwrap();
        let catalog = service.list().unwrap();
        let resolved = service.resolve(&catalog.skills()[0].selection()).unwrap();
        assert_eq!(resolved.format_version(), SKILL_PACKAGE_FORMAT_VERSION_V3);
        assert_eq!(resolved.resources(), &package.resource_index());
        assert_eq!(
            resolved.resources().get("README.md").unwrap().kind(),
            crate::skills::SkillResourceKind::Other
        );

        let package_ref = InstalledPackageRef::from_format_and_revision(
            package.format_version(),
            package.revision().clone(),
        )
        .unwrap();
        let snapshot = ManagedSkillStore::new(root)
            .unwrap()
            .load_complete_package(&package_ref)
            .unwrap();
        assert_eq!(
            snapshot.resource_bytes,
            vec![
                ("README.md".to_string(), b"ROOT_RESOURCE".to_vec()),
                (
                    "agents/openai.yaml".to_string(),
                    b"interface: chat".to_vec()
                ),
                ("references/guide.md".to_string(), b"GUIDE".to_vec()),
            ]
        );
    }

    #[test]
    fn updates_can_cross_v2_and_v3_format_boundaries_without_changing_identity() {
        let fixture = tempdir().unwrap();
        let root = fixture.path().join("store");
        let installer = ManagedSkillInstaller::new(&root).unwrap();
        let v2 = package_v2("portable", "V2", b"V2_GUIDE");
        let install = install_request(INSTALLATION_ID, v2);
        installer.install(&install).unwrap();
        let v2_installation_revision = installation_revision(&root, install.installation_id());

        let v3 = package_v3("portable", "V3");
        let v3_revision = v3.revision().clone();
        let upgrade = ManagedSkillUpdateRequest::new(
            install.installation_id().clone(),
            v2_installation_revision,
            v3,
        );
        assert_eq!(
            installer.update(&upgrade).unwrap(),
            ManagedSkillUpdateOutcome::Updated
        );
        let service = SkillsService::new().with_installed_source(&root).unwrap();
        let catalog = service.list().unwrap();
        let skill_id = catalog.skills()[0].id().clone();
        assert_eq!(catalog.skills()[0].revision(), &v3_revision);
        assert_eq!(
            service
                .resolve(&catalog.skills()[0].selection())
                .unwrap()
                .format_version(),
            SKILL_PACKAGE_FORMAT_VERSION_V3
        );
        let v3_installation_revision = installation_revision(&root, install.installation_id());

        let v2_again = package_v2("portable", "V2_AGAIN", b"UPDATED_GUIDE");
        let v2_again_revision = v2_again.revision().clone();
        let downgrade = ManagedSkillUpdateRequest::new(
            install.installation_id().clone(),
            v3_installation_revision,
            v2_again,
        );
        assert_eq!(
            installer.update(&downgrade).unwrap(),
            ManagedSkillUpdateOutcome::Updated
        );
        let catalog = service.list().unwrap();
        assert_eq!(catalog.skills()[0].id(), &skill_id);
        assert_eq!(catalog.skills()[0].revision(), &v2_again_revision);
        assert_eq!(
            service
                .resolve(&catalog.skills()[0].selection())
                .unwrap()
                .format_version(),
            SKILL_PACKAGE_FORMAT_VERSION_V2
        );
    }

    #[test]
    fn v2_retry_deeply_verifies_an_existing_content_addressed_tree() {
        let fixture = tempdir().unwrap();
        let root = fixture.path().join("store");
        let installer = ManagedSkillInstaller::new(&root).unwrap();
        let package = package_v2("resourceful", "READ_RESOURCE", b"ORIGINAL");
        let request = install_request(INSTALLATION_ID, package.clone());
        installer.install(&request).unwrap();

        let digest = package
            .revision()
            .as_str()
            .strip_prefix(PACKAGE_REVISION_V2_PREFIX)
            .unwrap();
        fs::write(
            root.join(PACKAGES_DIRECTORY)
                .join(PACKAGE_V2_DIRECTORY)
                .join(digest)
                .join("references/guide.md"),
            b"TAMPERED",
        )
        .unwrap();

        let error = installer.install(&request).unwrap_err();
        assert_eq!(error.code(), ManagedSkillInstallerErrorCode::StoreCorrupt);
    }

    #[test]
    fn v2_resolve_deeply_verifies_every_revision_bound_resource() {
        let fixture = tempdir().unwrap();
        let root = fixture.path().join("store");
        let installer = ManagedSkillInstaller::new(&root).unwrap();
        let package = package_v2("resourceful", "READ_RESOURCE", b"ORIGINAL");
        let request = install_request(INSTALLATION_ID, package.clone());
        installer.install(&request).unwrap();

        let service = SkillsService::new().with_installed_source(&root).unwrap();
        let selection = service.list().unwrap().skills()[0].selection();
        let digest = package
            .revision()
            .as_str()
            .strip_prefix(PACKAGE_REVISION_V2_PREFIX)
            .unwrap();
        fs::remove_file(
            root.join(PACKAGES_DIRECTORY)
                .join(PACKAGE_V2_DIRECTORY)
                .join(digest)
                .join("references/guide.md"),
        )
        .unwrap();

        // Catalog discovery is intentionally manifest-only, but activation
        // must not expose a resource index for an incomplete package.
        assert_eq!(service.list().unwrap().skills().len(), 1);
        let error = service.resolve(&selection).unwrap_err();
        assert_eq!(error.code(), crate::skills::SkillErrorCode::InvalidSkill);
        assert_eq!(
            error.diagnostic_code(),
            Some(crate::skills::SkillDiagnosticCode::MissingSkillFile)
        );
    }

    #[test]
    fn mixed_v1_v2_receipts_list_and_resolve_together() {
        let fixture = tempdir().unwrap();
        let root = fixture.path().join("store");
        let installer = ManagedSkillInstaller::new(&root).unwrap();
        let v1 = install_request(INSTALLATION_ID, package("plain", "PLAIN"));
        let v2 = install_request(
            SECOND_INSTALLATION_ID,
            package_v2("resourceful", "RESOURCEFUL", b"GUIDE"),
        );
        installer.install(&v1).unwrap();
        installer.install(&v2).unwrap();

        let v1_receipt = ManagedSkillStore::new(&root)
            .unwrap()
            .load_receipt(v1.installation_id())
            .unwrap();
        let v2_receipt = ManagedSkillStore::new(&root)
            .unwrap()
            .load_receipt(v2.installation_id())
            .unwrap();
        assert_eq!(
            v1_receipt.package.format_version,
            SKILL_PACKAGE_FORMAT_VERSION
        );
        assert_eq!(
            v2_receipt.package.format_version,
            SKILL_PACKAGE_FORMAT_VERSION_V2
        );

        let service = SkillsService::new().with_installed_source(&root).unwrap();
        let catalog = service.list().unwrap();
        assert_eq!(catalog.skills().len(), 2);
        let resolved = catalog
            .skills()
            .iter()
            .map(|descriptor| service.resolve(&descriptor.selection()).unwrap())
            .collect::<Vec<_>>();
        assert!(resolved
            .iter()
            .any(|package| package.format_version() == SKILL_PACKAGE_FORMAT_VERSION));
        assert!(resolved.iter().any(|package| {
            package.format_version() == SKILL_PACKAGE_FORMAT_VERSION_V2
                && package.resources().len() == 2
        }));
    }

    #[test]
    fn corrupt_v2_manifest_is_isolated_by_the_read_only_source() {
        let fixture = tempdir().unwrap();
        let root = fixture.path().join("store");
        let installer = ManagedSkillInstaller::new(&root).unwrap();
        let package = package_v2("resourceful", "READ_RESOURCE", b"GUIDE");
        let request = install_request(INSTALLATION_ID, package.clone());
        installer.install(&request).unwrap();
        let digest = package
            .revision()
            .as_str()
            .strip_prefix(PACKAGE_REVISION_V2_PREFIX)
            .unwrap();
        let manifest_path = root
            .join(PACKAGES_DIRECTORY)
            .join(PACKAGE_V2_DIRECTORY)
            .join(digest)
            .join(PACKAGE_MANIFEST_FILE);
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
        manifest["unexpected"] = serde_json::json!(true);
        fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();

        let catalog = installed_catalog(&root);
        assert!(catalog.skills().is_empty());
        assert_eq!(
            catalog.diagnostics()[0].code(),
            crate::skills::SkillDiagnosticCode::InvalidPackageManifest
        );
    }

    #[test]
    fn v2_failpoints_preserve_the_same_receipt_linearization_boundary() {
        let fixture = tempdir().unwrap();
        let root = fixture.path().join("store");
        let request = install_request(
            INSTALLATION_ID,
            package_v2("resourceful", "READ_RESOURCE", b"GUIDE"),
        );
        let precommit = ManagedSkillInstaller::with_failpoint(
            &root,
            ManagedSkillInstallerFailpoint::PackagePublished,
        );
        assert_eq!(
            precommit.install(&request).unwrap_err().code(),
            ManagedSkillInstallerErrorCode::Io
        );
        assert!(installed_catalog(&root).skills().is_empty());

        let indeterminate = ManagedSkillInstaller::with_failpoint(
            &root,
            ManagedSkillInstallerFailpoint::ReceiptPublished,
        );
        assert_eq!(
            indeterminate.install(&request).unwrap_err().code(),
            ManagedSkillInstallerErrorCode::CommitIndeterminate
        );
        let catalog = installed_catalog(&root);
        assert_eq!(catalog.skills().len(), 1);
        assert_eq!(
            SkillsService::new()
                .with_installed_source(&root)
                .unwrap()
                .resolve(&catalog.skills()[0].selection())
                .unwrap()
                .resources()
                .len(),
            2
        );
    }

    #[test]
    fn local_directory_preparation_installs_and_resolves_exact_instructions() {
        let fixture = tempdir().unwrap();
        let acquisition = fixture.path().join("acquired-skill");
        fs::create_dir(&acquisition).unwrap();
        fs::write(
            acquisition.join(SKILL_FILE_NAME),
            "---\nname: local-auditor\ndescription: Local vertical slice.\n---\n# Instructions\nLOCAL_EXACT_INSTRUCTIONS\n",
        )
        .unwrap();
        let prepared =
            PreparedSkillPackage::from_local_directory(&acquisition, "user-selected-directory")
                .unwrap();
        let root = fixture.path().join("store");
        let installer = ManagedSkillInstaller::new(&root).unwrap();
        let request = install_request(INSTALLATION_ID, prepared);

        installer.install(&request).unwrap();
        let service = SkillsService::new().with_installed_source(&root).unwrap();
        let catalog = service.list().unwrap();
        assert_eq!(catalog.skills().len(), 1);
        let resolved = service.resolve(&catalog.skills()[0].selection()).unwrap();
        assert!(resolved.instructions().contains("LOCAL_EXACT_INSTRUCTIONS"));
        assert_eq!(
            resolved.descriptor().id().local_id(),
            request.installation_id().as_str()
        );
        let receipt = ManagedSkillStore::new(&root)
            .unwrap()
            .load_receipt(request.installation_id())
            .unwrap();
        assert_eq!(
            receipt._origin.provider(),
            crate::skills::LOCAL_DIRECTORY_SKILL_ORIGIN_PROVIDER
        );
        assert_eq!(receipt._origin.reference(), "user-selected-directory");
    }

    #[test]
    fn install_rejects_id_reuse_and_never_repairs_corrupt_state() {
        let fixture = tempdir().unwrap();
        let root = fixture.path().join("store");
        let installer = ManagedSkillInstaller::new(&root).unwrap();
        let first = install_request(INSTALLATION_ID, package("first", "FIRST"));
        installer.install(&first).unwrap();

        let different = install_request(INSTALLATION_ID, package("second", "SECOND"));
        assert_eq!(
            installer.install(&different).unwrap_err().code(),
            ManagedSkillInstallerErrorCode::InstallationExists
        );

        fs::write(
            root.join(INSTALLATIONS_DIRECTORY)
                .join(format!("{INSTALLATION_ID}.json")),
            b"{not-json",
        )
        .unwrap();
        assert_eq!(
            installer.install(&first).unwrap_err().code(),
            ManagedSkillInstallerErrorCode::StoreCorrupt
        );
        assert_eq!(
            fs::read(
                root.join(INSTALLATIONS_DIRECTORY)
                    .join(format!("{INSTALLATION_ID}.json"))
            )
            .unwrap(),
            b"{not-json"
        );
    }

    #[test]
    fn install_failpoints_preserve_absent_or_fully_committed_state_and_retry() {
        let precommit = [
            ManagedSkillInstallerFailpoint::PackageFileSynced,
            ManagedSkillInstallerFailpoint::PackagePublished,
            ManagedSkillInstallerFailpoint::PackageParentSynced,
            ManagedSkillInstallerFailpoint::ReceiptFileSynced,
        ];
        for failpoint in precommit {
            let fixture = tempdir().unwrap();
            let root = fixture.path().join("store");
            let request = install_request(INSTALLATION_ID, package("auditor", "INSTALL"));
            let failing = ManagedSkillInstaller::with_failpoint(&root, failpoint);

            let error = failing.install(&request).unwrap_err();
            assert_eq!(error.code(), ManagedSkillInstallerErrorCode::Io);
            assert!(installed_catalog(&root).skills().is_empty());
            assert!(
                package_staging_entries(&root).is_empty(),
                "{failpoint:?} must not leak package staging directories"
            );
            assert_eq!(
                ManagedSkillInstaller::new(&root)
                    .unwrap()
                    .install(&request)
                    .unwrap(),
                ManagedSkillInstallOutcome::Installed
            );
            assert_eq!(installed_catalog(&root).skills().len(), 1);
        }

        for failpoint in [
            ManagedSkillInstallerFailpoint::ReceiptPublished,
            ManagedSkillInstallerFailpoint::ReceiptParentSynced,
        ] {
            let fixture = tempdir().unwrap();
            let root = fixture.path().join("store");
            let request = install_request(INSTALLATION_ID, package("auditor", "INSTALL"));
            let failing = ManagedSkillInstaller::with_failpoint(&root, failpoint);

            let error = failing.install(&request).unwrap_err();
            assert_eq!(
                error.code(),
                ManagedSkillInstallerErrorCode::CommitIndeterminate
            );
            assert!(error.commit_may_have_succeeded());
            assert_eq!(installed_catalog(&root).skills().len(), 1);
            assert_eq!(
                ManagedSkillInstaller::new(&root)
                    .unwrap()
                    .install(&request)
                    .unwrap(),
                ManagedSkillInstallOutcome::AlreadyInstalled
            );
        }
    }

    #[test]
    fn retry_resyncs_an_orphan_package_before_committing_its_receipt() {
        let fixture = tempdir().unwrap();
        let root = fixture.path().join("store");
        let request = install_request(INSTALLATION_ID, package("auditor", "ORPHAN"));

        let publish_failure = ManagedSkillInstaller::with_failpoint(
            &root,
            ManagedSkillInstallerFailpoint::PackagePublished,
        );
        assert_eq!(
            publish_failure.install(&request).unwrap_err().code(),
            ManagedSkillInstallerErrorCode::Io
        );
        assert!(package_path(&root, request.package().revision()).is_dir());
        assert!(installed_catalog(&root).skills().is_empty());

        // This failpoint is reached only after the existing-package retry path
        // has verified the package and re-synced packages/v1.
        let resync_failure = ManagedSkillInstaller::with_failpoint(
            &root,
            ManagedSkillInstallerFailpoint::PackageParentSynced,
        );
        assert_eq!(
            resync_failure.install(&request).unwrap_err().code(),
            ManagedSkillInstallerErrorCode::Io
        );
        assert!(installed_catalog(&root).skills().is_empty());

        assert_eq!(
            ManagedSkillInstaller::new(&root)
                .unwrap()
                .install(&request)
                .unwrap(),
            ManagedSkillInstallOutcome::Installed
        );
    }

    #[test]
    fn update_failpoints_expose_only_old_or_complete_new_packages() {
        let failpoints = [
            ManagedSkillInstallerFailpoint::PackageFileSynced,
            ManagedSkillInstallerFailpoint::PackagePublished,
            ManagedSkillInstallerFailpoint::PackageParentSynced,
            ManagedSkillInstallerFailpoint::ReceiptFileSynced,
            ManagedSkillInstallerFailpoint::ReceiptPublished,
            ManagedSkillInstallerFailpoint::ReceiptParentSynced,
        ];
        for failpoint in failpoints {
            let fixture = tempdir().unwrap();
            let root = fixture.path().join("store");
            let installer = ManagedSkillInstaller::new(&root).unwrap();
            let install = install_request(INSTALLATION_ID, package("auditor", "OLD"));
            let old_revision = install.package().revision().clone();
            installer.install(&install).unwrap();
            let old_installation_revision = installation_revision(&root, install.installation_id());
            let update = ManagedSkillUpdateRequest::new(
                install.installation_id().clone(),
                old_installation_revision,
                package("auditor", "NEW"),
            );
            let target_revision = update.package().revision().clone();
            let failing = ManagedSkillInstaller::with_failpoint(&root, failpoint);

            let error = failing.update(&update).unwrap_err();
            let visible = installed_catalog(&root);
            assert_eq!(visible.skills().len(), 1);
            let visible_revision = visible.skills()[0].revision();
            if matches!(
                failpoint,
                ManagedSkillInstallerFailpoint::ReceiptPublished
                    | ManagedSkillInstallerFailpoint::ReceiptParentSynced
            ) {
                assert_eq!(
                    error.code(),
                    ManagedSkillInstallerErrorCode::CommitIndeterminate
                );
                assert_eq!(visible_revision, &target_revision);
            } else {
                assert_eq!(error.code(), ManagedSkillInstallerErrorCode::Io);
                assert_eq!(visible_revision, &old_revision);
            }

            let retry = ManagedSkillInstaller::new(&root)
                .unwrap()
                .update(&update)
                .unwrap();
            assert!(matches!(
                retry.outcome(),
                ManagedSkillUpdateOutcome::Updated | ManagedSkillUpdateOutcome::AlreadyCurrent
            ));
            assert_eq!(
                installed_catalog(&root).skills()[0].revision(),
                &target_revision
            );
        }
    }

    #[test]
    fn provenance_only_update_failpoints_expose_one_complete_receipt_generation() {
        for failpoint in [
            ManagedSkillInstallerFailpoint::PackageParentSynced,
            ManagedSkillInstallerFailpoint::ReceiptFileSynced,
            ManagedSkillInstallerFailpoint::ReceiptPublished,
            ManagedSkillInstallerFailpoint::ReceiptParentSynced,
        ] {
            let fixture = tempdir().unwrap();
            let root = fixture.path().join("store");
            let installer = ManagedSkillInstaller::new(&root).unwrap();
            let package = package("auditor", "UNCHANGED_PACKAGE");
            let old_provenance = provenance("authority-a", Some("refresh-a"));
            let install = install_request_with_provenance(
                INSTALLATION_ID,
                package.clone(),
                old_provenance.clone(),
            );
            installer.install(&install).unwrap();
            let before = installed_receipt(&root, install.installation_id());
            let new_provenance = provenance("authority-b", Some("refresh-b"));
            let update = ManagedSkillUpdateRequest::with_provenance(
                install.installation_id().clone(),
                before.installation_revision.clone(),
                package,
                new_provenance.clone(),
            );
            let failing = ManagedSkillInstaller::with_failpoint(&root, failpoint);

            let error = failing.update(&update).unwrap_err();
            let visible = installed_receipt(&root, install.installation_id());
            assert_eq!(visible.package.revision, before.package.revision);
            if matches!(
                failpoint,
                ManagedSkillInstallerFailpoint::ReceiptPublished
                    | ManagedSkillInstallerFailpoint::ReceiptParentSynced
            ) {
                assert_eq!(
                    error.code(),
                    ManagedSkillInstallerErrorCode::CommitIndeterminate
                );
                assert_eq!(visible.generation, 2);
                assert_eq!(visible.provenance, new_provenance);
                assert_ne!(visible.installation_revision, before.installation_revision);
            } else {
                assert_eq!(error.code(), ManagedSkillInstallerErrorCode::Io);
                assert_eq!(visible.generation, 1);
                assert_eq!(visible.provenance, old_provenance);
                assert_eq!(visible.installation_revision, before.installation_revision);
            }

            let retry = ManagedSkillInstaller::new(&root)
                .unwrap()
                .update(&update)
                .unwrap();
            assert!(matches!(
                retry.outcome(),
                ManagedSkillUpdateOutcome::Updated | ManagedSkillUpdateOutcome::AlreadyCurrent
            ));
            let converged = installed_receipt(&root, install.installation_id());
            assert_eq!(converged.generation, 2);
            assert_eq!(converged.provenance, new_provenance);
        }
    }

    #[test]
    fn uninstall_failpoints_commit_absence_and_retry_converges() {
        for failpoint in [
            ManagedSkillInstallerFailpoint::UninstallRetirementPublished,
            ManagedSkillInstallerFailpoint::UninstallParentSynced,
        ] {
            let fixture = tempdir().unwrap();
            let root = fixture.path().join("store");
            let installer = ManagedSkillInstaller::new(&root).unwrap();
            let install = install_request(INSTALLATION_ID, package("auditor", "DELETE"));
            installer.install(&install).unwrap();
            let uninstall = ManagedSkillUninstallRequest::new(
                install.installation_id().clone(),
                installation_revision(&root, install.installation_id()),
            );
            let failing = ManagedSkillInstaller::with_failpoint(&root, failpoint);

            let error = failing.uninstall(&uninstall).unwrap_err();
            assert_eq!(
                error.code(),
                ManagedSkillInstallerErrorCode::CommitIndeterminate
            );
            assert!(installed_catalog(&root).skills().is_empty());
            assert_eq!(
                fs::read_dir(root.join(RETIRED_INSTALLATIONS_DIRECTORY))
                    .unwrap()
                    .filter_map(Result::ok)
                    .count(),
                1,
                "an indeterminate uninstall durably retires its installation identity"
            );
            assert_eq!(
                ManagedSkillInstaller::new(&root)
                    .unwrap()
                    .uninstall(&uninstall)
                    .unwrap(),
                ManagedSkillUninstallOutcome::AlreadyAbsent
            );
            assert_eq!(
                fs::read_dir(root.join(RETIRED_INSTALLATIONS_DIRECTORY))
                    .unwrap()
                    .filter_map(Result::ok)
                    .count(),
                1,
                "idempotent uninstall retries never delete the retired-ID ledger"
            );
        }
    }

    #[test]
    fn every_error_after_retirement_publication_is_commit_indeterminate() {
        let fixture = tempdir().unwrap();
        let root = fixture.path().join("store");
        let installer = ManagedSkillInstaller::new(&root).unwrap();
        let install = install_request(INSTALLATION_ID, package("auditor", "DELETE"));
        installer.install(&install).unwrap();

        let transaction = installer.begin_transaction().unwrap();
        transaction
            .publish_retired_identity(install.installation_id(), true)
            .unwrap();
        let receipt_path = transaction.layout.receipt_path(install.installation_id());
        fs::remove_file(&receipt_path).unwrap();
        fs::create_dir(&receipt_path).unwrap();

        let error = transaction
            .retire_installation(install.installation_id(), true)
            .unwrap_err();
        assert_eq!(
            error.code(),
            ManagedSkillInstallerErrorCode::CommitIndeterminate
        );
        assert!(error.commit_may_have_succeeded());
        assert!(installed_catalog(&root).skills().is_empty());
    }

    #[test]
    fn live_installation_capacity_rejects_growth_but_allows_full_capacity_update() {
        let fixture = tempdir().unwrap();
        let root = fixture.path().join("store");
        let installer = ManagedSkillInstaller::new(&root).unwrap();
        let original = install_request(INSTALLATION_ID, package("auditor", "CAPACITY_OLD"));
        installer.install(&original).unwrap();
        let original_installation_revision =
            installation_revision(&root, original.installation_id());
        let installations = root.join(INSTALLATIONS_DIRECTORY);
        let receipt_origin = original.package().origin();
        for value in 1..MAX_LIVE_INSTALLATIONS {
            let id =
                SkillInstallationId::parse(Uuid::from_u128(value as u128).to_string()).unwrap();
            let bytes = encode_receipt(
                &id,
                original.package().format_version(),
                original.package().revision(),
                receipt_origin,
                1_784_347_513_399,
            )
            .unwrap();
            fs::write(installations.join(format!("{id}.json")), bytes).unwrap();
        }
        assert_eq!(
            fs::read_dir(&installations).unwrap().count(),
            MAX_LIVE_INSTALLATIONS
        );

        let overflow = install_request(
            SECOND_INSTALLATION_ID,
            package("overflow", "MUST_NOT_INSTALL"),
        );
        let error = installer.install(&overflow).unwrap_err();
        assert_eq!(
            error.code(),
            ManagedSkillInstallerErrorCode::CapacityExceeded
        );
        assert!(matches!(
            error,
            ManagedSkillInstallerError::CapacityExceeded {
                capacity: ManagedSkillStoreCapacity::Installations,
                limit: MAX_LIVE_INSTALLATIONS,
            }
        ));
        assert_eq!(
            fs::read_dir(&installations).unwrap().count(),
            MAX_LIVE_INSTALLATIONS
        );

        let updated = package("auditor", "CAPACITY_NEW");
        let updated_revision = updated.revision().clone();
        let update = ManagedSkillUpdateRequest::new(
            original.installation_id().clone(),
            original_installation_revision,
            updated,
        );
        assert_eq!(
            installer.update(&update).unwrap(),
            ManagedSkillUpdateOutcome::Updated
        );
        assert_eq!(
            ManagedSkillStore::new(&root)
                .unwrap()
                .load_receipt(original.installation_id())
                .unwrap()
                .package
                .revision,
            updated_revision
        );
        assert_eq!(
            fs::read_dir(&installations).unwrap().count(),
            MAX_LIVE_INSTALLATIONS
        );
    }

    #[test]
    fn absent_retirement_preserves_ledger_slots_for_every_live_installation() {
        assert!(
            ensure_retirement_marker_capacity(MAX_RETIRED_INSTALLATION_ENTRIES - 1, 1, true,)
                .is_ok()
        );

        let error =
            ensure_retirement_marker_capacity(MAX_RETIRED_INSTALLATION_ENTRIES - 1, 1, false)
                .unwrap_err();
        assert_eq!(
            error.code(),
            ManagedSkillInstallerErrorCode::CapacityExceeded
        );
        assert!(matches!(
            error,
            ManagedSkillInstallerError::CapacityExceeded {
                capacity: ManagedSkillStoreCapacity::RetiredInstallationIds,
                limit: MAX_RETIRED_INSTALLATION_ENTRIES,
            }
        ));

        assert!(
            ensure_retirement_marker_capacity(MAX_RETIRED_INSTALLATION_ENTRIES - 1, 2, true,)
                .is_err()
        );
    }

    #[test]
    fn concurrent_mutations_are_serialized_by_the_installer() {
        let fixture = tempdir().unwrap();
        let root = fixture.path().join("store");
        let installers = [
            Arc::new(ManagedSkillInstaller::new(&root).unwrap()),
            Arc::new(ManagedSkillInstaller::new(&root).unwrap()),
        ];
        let install = Arc::new(install_request(
            INSTALLATION_ID,
            package("auditor", "CONCURRENT"),
        ));
        let barrier = Arc::new(Barrier::new(3));
        let mut handles = Vec::new();
        for installer in installers {
            let install = Arc::clone(&install);
            let barrier = Arc::clone(&barrier);
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                installer.install(&install).unwrap()
            }));
        }
        barrier.wait();
        let mut outcomes = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>();
        outcomes.sort_by_key(|outcome| match outcome.outcome() {
            ManagedSkillInstallOutcome::Installed => 0,
            ManagedSkillInstallOutcome::AlreadyInstalled => 1,
        });
        assert_eq!(
            outcomes,
            vec![
                ManagedSkillInstallOutcome::Installed,
                ManagedSkillInstallOutcome::AlreadyInstalled
            ]
        );
        assert_eq!(installed_catalog(&root).skills().len(), 1);
    }

    #[test]
    fn concurrent_updates_from_one_revision_commit_exactly_one_target() {
        let fixture = tempdir().unwrap();
        let root = fixture.path().join("store");
        let installer = ManagedSkillInstaller::new(&root).unwrap();
        let install = install_request(INSTALLATION_ID, package("auditor", "BASE"));
        installer.install(&install).unwrap();
        let base_revision = installation_revision(&root, install.installation_id());
        let first = Arc::new(ManagedSkillUpdateRequest::new(
            install.installation_id().clone(),
            base_revision.clone(),
            package("auditor", "TARGET_ONE"),
        ));
        let second = Arc::new(ManagedSkillUpdateRequest::new(
            install.installation_id().clone(),
            base_revision,
            package("auditor", "TARGET_TWO"),
        ));
        let target_revisions = [
            first.package().revision().clone(),
            second.package().revision().clone(),
        ];
        let barrier = Arc::new(Barrier::new(3));
        let mut handles = Vec::new();
        for (installer, request) in [
            (ManagedSkillInstaller::new(&root).unwrap(), first),
            (ManagedSkillInstaller::new(&root).unwrap(), second),
        ] {
            let barrier = Arc::clone(&barrier);
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                installer.update(&request)
            }));
        }
        barrier.wait();
        let results = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>();

        assert_eq!(
            results
                .iter()
                .filter(|result| {
                    matches!(
                        result,
                        Ok(result)
                            if result.outcome() == &ManagedSkillUpdateOutcome::Updated
                    )
                })
                .count(),
            1
        );
        assert_eq!(
            results
                .iter()
                .filter(|result| matches!(
                    result,
                    Err(error)
                        if error.code() == ManagedSkillInstallerErrorCode::RevisionConflict
                ))
                .count(),
            1
        );
        let visible = installed_catalog(&root);
        assert_eq!(visible.skills().len(), 1);
        assert!(target_revisions.contains(visible.skills()[0].revision()));
    }

    #[test]
    fn legacy_uninstall_and_update_share_one_writer_linearization_order() {
        let fixture = tempdir().unwrap();
        let root = fixture.path().join("store");
        let installer = ManagedSkillInstaller::new(&root).unwrap();
        let base_package = package("auditor", "BASE");
        let base_package_revision = base_package.revision().clone();
        let install = install_request(INSTALLATION_ID, base_package);
        installer.install(&install).unwrap();
        let base_installation_revision = installation_revision(&root, install.installation_id());
        let target_package = package("auditor", "TARGET");
        let target_package_revision = target_package.revision().clone();

        let update = Arc::new(ManagedSkillUpdateRequest::new(
            install.installation_id().clone(),
            base_installation_revision,
            target_package,
        ));
        let uninstall = Arc::new(ManagedSkillLegacyUninstallRequest::new(
            install.installation_id().clone(),
            base_package_revision,
        ));
        let barrier = Arc::new(Barrier::new(3));
        let update_handle = {
            let root = root.clone();
            let update = Arc::clone(&update);
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                let installer = ManagedSkillInstaller::new(root).unwrap();
                barrier.wait();
                installer.update(&update)
            })
        };
        let uninstall_handle = {
            let root = root.clone();
            let uninstall = Arc::clone(&uninstall);
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                let installer = ManagedSkillInstaller::new(root).unwrap();
                barrier.wait();
                installer.uninstall_legacy(&uninstall)
            })
        };
        barrier.wait();
        let update_result = update_handle.join().unwrap();
        let uninstall_result = uninstall_handle.join().unwrap();

        match (update_result, uninstall_result) {
            (Ok(updated), Err(conflict)) => {
                assert_eq!(updated.outcome(), &ManagedSkillUpdateOutcome::Updated);
                assert!(matches!(
                    conflict,
                    ManagedSkillInstallerError::LegacyPackageRevisionConflict {
                        actual_revision,
                        ..
                    } if actual_revision == target_package_revision
                ));
                let receipt = installed_receipt(&root, install.installation_id());
                assert_eq!(receipt.package.revision, target_package_revision);
            }
            (Err(update_error), Ok(uninstalled)) => {
                assert_eq!(
                    update_error.code(),
                    ManagedSkillInstallerErrorCode::InstallationNotFound
                );
                assert_eq!(
                    uninstalled.outcome(),
                    &ManagedSkillUninstallOutcome::Uninstalled
                );
                assert!(ManagedSkillStore::new(&root)
                    .unwrap()
                    .load_receipt(install.installation_id())
                    .is_err());
                assert_eq!(
                    installer.install(&install).unwrap_err().code(),
                    ManagedSkillInstallerErrorCode::InstallationRetired
                );
            }
            (update_result, uninstall_result) => panic!(
                "unexpected serialized mutation outcomes: update={update_result:?}, uninstall={uninstall_result:?}"
            ),
        }
    }

    #[test]
    fn stale_transaction_staging_is_cleaned_without_touching_foreign_entries() {
        let fixture = tempdir().unwrap();
        let root = fixture.path().join("store");
        let installer = ManagedSkillInstaller::new(&root).unwrap();
        let install = install_request(INSTALLATION_ID, package("auditor", "FIRST"));
        installer.install(&install).unwrap();
        let nonce = Uuid::new_v4();
        let receipt_stage = root.join(INSTALLATIONS_DIRECTORY).join(format!(
            ".receipt-{}-{nonce}.tmp",
            install.installation_id()
        ));
        fs::write(&receipt_stage, b"partial").unwrap();
        let digest = install
            .package()
            .revision()
            .as_str()
            .strip_prefix(PACKAGE_REVISION_PREFIX)
            .unwrap();
        let package_stage = root
            .join(PACKAGES_DIRECTORY)
            .join(PACKAGE_V1_DIRECTORY)
            .join(format!(".package-{digest}-{}.tmp", Uuid::new_v4()));
        fs::create_dir(&package_stage).unwrap();
        fs::create_dir(package_stage.join("references")).unwrap();
        fs::write(package_stage.join(SKILL_FILE_NAME), b"partial Skill").unwrap();
        fs::write(
            package_stage.join("references").join("guide.md"),
            b"partial",
        )
        .unwrap();
        let foreign_package_stage = package_stage
            .parent()
            .unwrap()
            .join(format!(".package-{digest}-not-a-uuid.tmp"));
        fs::create_dir(&foreign_package_stage).unwrap();
        fs::write(foreign_package_stage.join("keep"), b"foreign").unwrap();
        let unknown = root
            .join(INSTALLATIONS_DIRECTORY)
            .join(".third-party-state");
        fs::write(&unknown, b"preserve").unwrap();

        let second = install_request(SECOND_INSTALLATION_ID, package("second", "SECOND"));
        installer.install(&second).unwrap();

        assert!(!receipt_stage.exists());
        assert!(!package_stage.exists());
        assert_eq!(
            fs::read(foreign_package_stage.join("keep")).unwrap(),
            b"foreign"
        );
        assert!(unknown.exists());
    }

    #[test]
    fn legacy_uninstall_tombstones_migrate_to_the_single_use_identity_ledger() {
        let fixture = tempdir().unwrap();
        let root = fixture.path().join("store");
        let installer = ManagedSkillInstaller::new(&root).unwrap();
        let first = install_request(INSTALLATION_ID, package("auditor", "FIRST"));
        installer.install(&first).unwrap();
        let receipt = root
            .join(INSTALLATIONS_DIRECTORY)
            .join(format!("{}.json", first.installation_id()));
        let legacy_tombstone = root.join(INSTALLATIONS_DIRECTORY).join(format!(
            ".uninstall-{}-{}.tombstone",
            first.installation_id(),
            Uuid::new_v4()
        ));
        fs::rename(&receipt, &legacy_tombstone).unwrap();

        let second = install_request(SECOND_INSTALLATION_ID, package("second", "SECOND"));
        installer.install(&second).unwrap();

        assert!(!legacy_tombstone.exists());
        assert!(root
            .join(RETIRED_INSTALLATIONS_DIRECTORY)
            .join(format!("{}.json", first.installation_id()))
            .is_file());
        assert_eq!(
            installer.install(&first).unwrap_err().code(),
            ManagedSkillInstallerErrorCode::InstallationRetired
        );
    }

    #[test]
    fn tombstone_migration_retires_and_removes_a_simultaneously_live_identity() {
        let fixture = tempdir().unwrap();
        let root = fixture.path().join("store");
        let installer = ManagedSkillInstaller::new(&root).unwrap();
        let first = install_request(INSTALLATION_ID, package("auditor", "FIRST"));
        installer.install(&first).unwrap();
        let receipt = root
            .join(INSTALLATIONS_DIRECTORY)
            .join(format!("{}.json", first.installation_id()));
        let legacy_tombstone = root.join(INSTALLATIONS_DIRECTORY).join(format!(
            ".uninstall-{}-{}.tombstone",
            first.installation_id(),
            Uuid::new_v4()
        ));
        fs::copy(&receipt, &legacy_tombstone).unwrap();

        let second = install_request(SECOND_INSTALLATION_ID, package("second", "SECOND"));
        assert_eq!(
            installer.install(&second).unwrap(),
            ManagedSkillInstallOutcome::Installed
        );

        assert!(!receipt.exists());
        assert!(!legacy_tombstone.exists());
        assert!(root
            .join(RETIRED_INSTALLATIONS_DIRECTORY)
            .join(format!("{}.json", first.installation_id()))
            .is_file());
        assert_eq!(
            installer.install(&first).unwrap_err().code(),
            ManagedSkillInstallerErrorCode::InstallationRetired
        );
        assert_eq!(installed_catalog(&root).skills().len(), 1);
    }

    #[test]
    fn durable_retirement_wins_if_a_live_receipt_reappears_after_crash() {
        let fixture = tempdir().unwrap();
        let root = fixture.path().join("store");
        let installer = ManagedSkillInstaller::new(&root).unwrap();
        let first = install_request(INSTALLATION_ID, package("auditor", "FIRST"));
        installer.install(&first).unwrap();
        let receipt = root
            .join(INSTALLATIONS_DIRECTORY)
            .join(format!("{}.json", first.installation_id()));
        let retired = root
            .join(RETIRED_INSTALLATIONS_DIRECTORY)
            .join(format!("{}.json", first.installation_id()));
        fs::copy(&receipt, &retired).unwrap();

        let index = ManagedSkillStore::new(&root)
            .unwrap()
            .scan_receipts()
            .unwrap();
        assert!(index.receipts.is_empty());
        assert_eq!(index.issues.len(), 1);
        assert!(receipt.is_file(), "read-only scans never repair the store");

        let second = install_request(SECOND_INSTALLATION_ID, package("second", "SECOND"));
        assert_eq!(
            installer.install(&second).unwrap(),
            ManagedSkillInstallOutcome::Installed
        );
        assert!(!receipt.exists());
        assert!(retired.is_file());
        assert_eq!(
            installer.install(&first).unwrap_err().code(),
            ManagedSkillInstallerErrorCode::InstallationRetired
        );
    }

    #[test]
    fn staging_guards_only_delete_objects_owned_by_the_transaction() {
        let fixture = tempdir().unwrap();
        let foreign_file = fixture.path().join("reserved-but-not-created");
        fs::write(&foreign_file, b"foreign").unwrap();
        drop(StagingPath {
            path: foreign_file.clone(),
            parent: fixture.path().to_path_buf(),
            kind: StagingKind::File,
            armed: false,
        });
        assert_eq!(fs::read(&foreign_file).unwrap(), b"foreign");

        let owned_file = fixture.path().join(format!(
            ".receipt-{}-{}.tmp",
            installation_id(INSTALLATION_ID),
            Uuid::new_v4()
        ));
        fs::write(&owned_file, b"owned").unwrap();
        drop(StagingPath {
            path: owned_file.clone(),
            parent: fixture.path().to_path_buf(),
            kind: StagingKind::File,
            armed: true,
        });
        assert!(!owned_file.exists());

        let digest = "a".repeat(64);
        let owned_directory = fixture
            .path()
            .join(format!(".package-{digest}-{}.tmp", Uuid::new_v4()));
        fs::create_dir(&owned_directory).unwrap();
        fs::create_dir(owned_directory.join("references")).unwrap();
        fs::write(owned_directory.join(SKILL_FILE_NAME), b"owned").unwrap();
        fs::write(
            owned_directory.join("references").join("guide.md"),
            b"owned",
        )
        .unwrap();
        drop(StagingPath {
            path: owned_directory.clone(),
            parent: fixture.path().to_path_buf(),
            kind: StagingKind::Directory,
            armed: true,
        });
        assert!(!owned_directory.exists());

        let outside_parent = fixture.path().join("outside");
        fs::create_dir(&outside_parent).unwrap();
        let escaped = outside_parent.join(format!(".package-{digest}-{}.tmp", Uuid::new_v4()));
        fs::create_dir(&escaped).unwrap();
        fs::write(escaped.join(SKILL_FILE_NAME), b"foreign").unwrap();
        drop(StagingPath {
            path: escaped.clone(),
            parent: fixture.path().to_path_buf(),
            kind: StagingKind::Directory,
            armed: true,
        });
        assert_eq!(fs::read(escaped.join(SKILL_FILE_NAME)).unwrap(), b"foreign");
    }

    #[test]
    fn startup_cleanup_rejects_owned_package_names_that_are_not_plain_directories() {
        let fixture = tempdir().unwrap();
        let root = fixture.path().join("store");
        let installer = ManagedSkillInstaller::new(&root).unwrap();
        let first = install_request(INSTALLATION_ID, package("auditor", "FIRST"));
        installer.install(&first).unwrap();
        let invalid_stage = root
            .join(PACKAGES_DIRECTORY)
            .join(PACKAGE_V1_DIRECTORY)
            .join(format!(
                ".package-{}-{}.tmp",
                "b".repeat(64),
                Uuid::new_v4()
            ));
        fs::write(&invalid_stage, b"do not delete").unwrap();

        let second = install_request(SECOND_INSTALLATION_ID, package("second", "SECOND"));
        let error = installer.install(&second).unwrap_err();

        assert_eq!(error.code(), ManagedSkillInstallerErrorCode::StoreCorrupt);
        assert_eq!(fs::read(&invalid_stage).unwrap(), b"do not delete");
        assert_eq!(installed_catalog(&root).skills().len(), 1);
    }

    #[test]
    fn staging_cleanup_budget_is_inclusive_and_fails_closed() {
        let fixture = tempdir().unwrap();
        let root = fixture.path().join("store");
        let installer = ManagedSkillInstaller::new(&root).unwrap();
        let request = install_request(INSTALLATION_ID, package("auditor", "BUDGET"));
        installer.install(&request).unwrap();
        let installations = root.join(INSTALLATIONS_DIRECTORY);
        let package_version = root.join(PACKAGES_DIRECTORY).join(PACKAGE_V1_DIRECTORY);
        let package_v2 = root.join(PACKAGES_DIRECTORY).join(PACKAGE_V2_DIRECTORY);
        let package_v3 = root.join(PACKAGES_DIRECTORY).join(PACKAGE_V3_DIRECTORY);
        let layout = ManagedStoreLayout {
            root,
            installations: installations.clone(),
            retired_installations: fixture.path().join("unused-retired-installations"),
            package_v1: package_version,
            package_v2,
            package_v3,
        };
        let stale = installations.join(format!(
            ".receipt-{}-{}.tmp",
            request.installation_id(),
            Uuid::new_v4()
        ));
        fs::write(&stale, b"stale").unwrap();

        assert_eq!(
            cleanup_stale_transaction_entries_with_limit(&layout, 1)
                .unwrap_err()
                .code(),
            ManagedSkillInstallerErrorCode::InvalidStore
        );
        if !stale.exists() {
            fs::write(&stale, b"stale").unwrap();
        }
        cleanup_stale_transaction_entries_with_limit(&layout, 2).unwrap();
        assert!(!stale.exists());
    }

    #[test]
    fn package_capacity_reserves_space_for_the_next_atomic_stage() {
        let fixture = tempdir().unwrap();
        let packages = fixture.path().join("packages");
        fs::create_dir(&packages).unwrap();

        ensure_directory_has_room(&packages, 2, ManagedSkillStoreCapacity::Packages).unwrap();
        fs::create_dir(packages.join("first")).unwrap();
        ensure_directory_has_room(&packages, 2, ManagedSkillStoreCapacity::Packages).unwrap();
        fs::create_dir(packages.join("second")).unwrap();
        let error = ensure_directory_has_room(&packages, 2, ManagedSkillStoreCapacity::Packages)
            .unwrap_err();
        assert!(matches!(
            error,
            ManagedSkillInstallerError::CapacityExceeded {
                capacity: ManagedSkillStoreCapacity::Packages,
                limit: 2,
            }
        ));
    }
}
