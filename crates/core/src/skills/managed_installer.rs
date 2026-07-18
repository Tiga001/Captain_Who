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

use super::managed_fs::{atomic_rename_noreplace, atomic_replace, sync_directory};
use super::managed_store::{
    encode_receipt, InstalledPackageRef, InstalledSkillReceipt, ManagedSkillStore,
    ManagedStoreLoadError, INSTALLATIONS_DIRECTORY, MAX_INSTALLATION_DIRECTORY_ENTRIES,
    MAX_LIVE_INSTALLATIONS, MAX_MANAGED_DIRECTORY_ENTRIES, PACKAGES_DIRECTORY,
    PACKAGE_VERSION_DIRECTORY,
};
use super::model::{SkillInstallationId, SkillRevision};
use super::prepared::PreparedSkillPackage;
use super::workspace::{
    is_symlink_or_reparse, metadata_if_present, verify_opened_file_identity, SKILL_FILE_NAME,
};
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
    ) -> Result<ManagedSkillInstallOutcome, ManagedSkillInstallerError> {
        let transaction = self.begin_transaction()?;
        match load_receipt(&transaction.store, request.installation_id())? {
            Some(receipt) if receipt.package.revision == *request.package().revision() => {
                verify_prepared_package(&transaction.store, request.package())?;
                transaction.sync_existing_commit(
                    ManagedSkillMutation::Install,
                    request.installation_id(),
                    Some(request.package().revision()),
                )?;
                Ok(ManagedSkillInstallOutcome::AlreadyInstalled)
            }
            Some(receipt) => Err(ManagedSkillInstallerError::InstallationExists {
                installation_id: request.installation_id().clone(),
                existing_revision: receipt.package.revision,
                requested_revision: request.package().revision().clone(),
            }),
            None => {
                transaction.ensure_new_installation_capacity()?;
                transaction.publish_package(request.package())?;
                transaction.commit_receipt(
                    ManagedSkillMutation::Install,
                    request.installation_id(),
                    request.package(),
                    request.created_at_unix_ms,
                    ReceiptCommitMode::Create,
                )?;
                Ok(ManagedSkillInstallOutcome::Installed)
            }
        }
    }

    /// Replaces an installation using revision-based compare-and-swap.
    ///
    /// A retry whose target revision is already visible succeeds even if its
    /// expected revision is now stale, which lets callers recover from an
    /// indeterminate commit acknowledgement.
    pub fn update(
        &self,
        request: &ManagedSkillUpdateRequest,
    ) -> Result<ManagedSkillUpdateOutcome, ManagedSkillInstallerError> {
        let transaction = self.begin_transaction()?;
        let receipt =
            load_receipt(&transaction.store, request.installation_id())?.ok_or_else(|| {
                ManagedSkillInstallerError::InstallationNotFound {
                    installation_id: request.installation_id().clone(),
                }
            })?;

        // Check the intended target before the expected revision. This makes a
        // retry converge after a lost or indeterminate commit acknowledgement.
        if receipt.package.revision == *request.package().revision() {
            verify_prepared_package(&transaction.store, request.package())?;
            transaction.sync_existing_commit(
                ManagedSkillMutation::Update,
                request.installation_id(),
                Some(request.package().revision()),
            )?;
            return Ok(ManagedSkillUpdateOutcome::AlreadyCurrent);
        }
        if receipt.package.revision != *request.expected_revision() {
            return Err(ManagedSkillInstallerError::RevisionConflict {
                installation_id: request.installation_id().clone(),
                expected_revision: request.expected_revision().clone(),
                actual_revision: receipt.package.revision,
            });
        }

        transaction.ensure_receipt_staging_capacity()?;
        transaction.publish_package(request.package())?;
        transaction.commit_receipt(
            ManagedSkillMutation::Update,
            request.installation_id(),
            request.package(),
            receipt.installed_at_unix_ms,
            ReceiptCommitMode::Replace,
        )?;
        Ok(ManagedSkillUpdateOutcome::Updated)
    }

    /// Removes an installation using revision-based compare-and-swap.
    ///
    /// Immutable package objects are retained; the receipt is the only live
    /// installation reference and therefore the uninstall commit point.
    pub fn uninstall(
        &self,
        request: &ManagedSkillUninstallRequest,
    ) -> Result<ManagedSkillUninstallOutcome, ManagedSkillInstallerError> {
        let transaction = self.begin_transaction()?;
        let Some(receipt) = load_receipt(&transaction.store, request.installation_id())? else {
            transaction.sync_existing_commit(
                ManagedSkillMutation::Uninstall,
                request.installation_id(),
                None,
            )?;
            return Ok(ManagedSkillUninstallOutcome::AlreadyAbsent);
        };
        if receipt.package.revision != *request.expected_revision() {
            return Err(ManagedSkillInstallerError::RevisionConflict {
                installation_id: request.installation_id().clone(),
                expected_revision: request.expected_revision().clone(),
                actual_revision: receipt.package.revision,
            });
        }

        let receipt_path = transaction.layout.receipt_path(request.installation_id());
        let tombstone = transaction
            .layout
            .unique_uninstall_tombstone(request.installation_id())?;
        atomic_rename_noreplace(&receipt_path, &tombstone.path).map_err(|error| {
            transaction.io_error("atomically remove managed Skill receipt", error)
        })?;
        if transaction.should_fail(ManagedSkillInstallerFailpoint::UninstallRenamed) {
            return Err(transaction.commit_indeterminate(
                ManagedSkillMutation::Uninstall,
                request.installation_id(),
                None,
                "injected failure after uninstall receipt rename",
            ));
        }
        sync_directory(&transaction.layout.installations).map_err(|error| {
            transaction.commit_indeterminate(
                ManagedSkillMutation::Uninstall,
                request.installation_id(),
                None,
                format!("cannot sync installations directory after uninstall: {error}"),
            )
        })?;
        if transaction.should_fail(ManagedSkillInstallerFailpoint::UninstallParentSynced) {
            return Err(transaction.commit_indeterminate(
                ManagedSkillMutation::Uninstall,
                request.installation_id(),
                None,
                "injected failure after uninstall directory sync",
            ));
        }

        // Tombstone cleanup is deliberately best effort. The receipt rename and
        // first parent sync above have already durably committed absence.
        if fs::remove_file(&tombstone.path).is_ok() {
            let _ = sync_directory(&transaction.layout.installations);
        }
        Ok(ManagedSkillUninstallOutcome::Uninstalled)
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
        let packages = ensure_exact_directory(&root, PACKAGES_DIRECTORY)?;
        let version = ensure_exact_directory(&packages, PACKAGE_VERSION_DIRECTORY)?;
        let layout = ManagedStoreLayout {
            root,
            installations,
            package_version: version,
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

#[derive(Debug, Clone)]
pub struct ManagedSkillInstallRequest {
    installation_id: SkillInstallationId,
    package: PreparedSkillPackage,
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
        Self {
            installation_id,
            package,
            created_at_unix_ms: unix_time_ms(),
        }
    }

    pub fn installation_id(&self) -> &SkillInstallationId {
        &self.installation_id
    }

    pub fn package(&self) -> &PreparedSkillPackage {
        &self.package
    }

    #[cfg(test)]
    fn set_created_at_unix_ms(&mut self, value: u64) {
        self.created_at_unix_ms = value;
    }
}

#[derive(Debug, Clone)]
pub struct ManagedSkillUpdateRequest {
    installation_id: SkillInstallationId,
    expected_revision: SkillRevision,
    package: PreparedSkillPackage,
}

impl ManagedSkillUpdateRequest {
    pub fn new(
        installation_id: SkillInstallationId,
        expected_revision: SkillRevision,
        package: PreparedSkillPackage,
    ) -> Self {
        Self {
            installation_id,
            expected_revision,
            package,
        }
    }

    pub fn installation_id(&self) -> &SkillInstallationId {
        &self.installation_id
    }

    pub fn expected_revision(&self) -> &SkillRevision {
        &self.expected_revision
    }

    pub fn package(&self) -> &PreparedSkillPackage {
        &self.package
    }
}

#[derive(Debug, Clone)]
pub struct ManagedSkillUninstallRequest {
    installation_id: SkillInstallationId,
    expected_revision: SkillRevision,
}

impl ManagedSkillUninstallRequest {
    pub fn new(installation_id: SkillInstallationId, expected_revision: SkillRevision) -> Self {
        Self {
            installation_id,
            expected_revision,
        }
    }

    pub fn installation_id(&self) -> &SkillInstallationId {
        &self.installation_id
    }

    pub fn expected_revision(&self) -> &SkillRevision {
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
    Packages,
}

impl ManagedSkillStoreCapacity {
    pub fn stable_name(self) -> &'static str {
        match self {
            Self::Installations => "installations",
            Self::InstallationDirectory => "installationDirectory",
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
    InstallationNotFound {
        installation_id: SkillInstallationId,
    },
    RevisionConflict {
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
        intended_revision: Option<SkillRevision>,
        reason: String,
    },
}

impl ManagedSkillInstallerError {
    pub fn code(&self) -> ManagedSkillInstallerErrorCode {
        match self {
            Self::InvalidStore { .. } => ManagedSkillInstallerErrorCode::InvalidStore,
            Self::CapacityExceeded { .. } => ManagedSkillInstallerErrorCode::CapacityExceeded,
            Self::InstallationExists { .. } => ManagedSkillInstallerErrorCode::InstallationExists,
            Self::InstallationNotFound { .. } => {
                ManagedSkillInstallerErrorCode::InstallationNotFound
            }
            Self::RevisionConflict { .. } => ManagedSkillInstallerErrorCode::RevisionConflict,
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
    fn ensure_new_installation_capacity(&self) -> Result<(), ManagedSkillInstallerError> {
        let (directory_entries, live_entries) =
            count_installation_entries(&self.layout.installations)?;
        if live_entries >= MAX_LIVE_INSTALLATIONS {
            return Err(ManagedSkillInstallerError::CapacityExceeded {
                capacity: ManagedSkillStoreCapacity::Installations,
                limit: MAX_LIVE_INSTALLATIONS,
            });
        }
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
        let package_ref = InstalledPackageRef::from_revision(package.revision().clone())
            .map_err(|reason| ManagedSkillInstallerError::StoreCorrupt { reason })?;
        let target = self.layout.package_version.join(&package_ref.digest_hex);
        if metadata_if_present(&target)
            .map_err(|error| self.io_error("inspect managed Skill package", error))?
            .is_some()
        {
            return self.ensure_existing_package_durable(package);
        }
        ensure_directory_has_room(
            &self.layout.package_version,
            MAX_MANAGED_DIRECTORY_ENTRIES,
            ManagedSkillStoreCapacity::Packages,
        )?;

        let mut staging = self
            .layout
            .unique_package_staging(&package_ref.digest_hex)?;
        let skill_path = staging.path.join(SKILL_FILE_NAME);
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&skill_path)
            .map_err(|error| self.io_error("create managed Skill package staging file", error))?;
        file.write_all(package.source_bytes())
            .map_err(|error| self.io_error("write managed Skill package staging file", error))?;
        file.flush()
            .map_err(|error| self.io_error("flush managed Skill package staging file", error))?;
        file.sync_all()
            .map_err(|error| self.io_error("sync managed Skill package staging file", error))?;
        drop(file);
        if self.should_fail(ManagedSkillInstallerFailpoint::PackageFileSynced) {
            return Err(self.injected_io("publish managed Skill package after file sync"));
        }
        sync_directory(&staging.path).map_err(|error| {
            self.io_error("sync managed Skill package staging directory", error)
        })?;

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
        sync_directory(&self.layout.package_version).map_err(|error| {
            self.io_error("sync managed Skill package version directory", error)
        })?;
        if self.should_fail(ManagedSkillInstallerFailpoint::PackageParentSynced) {
            return Err(self.injected_io("publish managed Skill package after parent sync"));
        }
        verify_prepared_package(&self.store, package)
    }

    fn ensure_existing_package_durable(
        &self,
        package: &PreparedSkillPackage,
    ) -> Result<(), ManagedSkillInstallerError> {
        verify_prepared_package(&self.store, package)?;
        // The package may be an orphan left by a crash immediately after its
        // rename. Re-sync the parent before any receipt may reference it.
        sync_directory(&self.layout.package_version).map_err(|error| {
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
        installation_id: &SkillInstallationId,
        package: &PreparedSkillPackage,
        installed_at_unix_ms: u64,
        mode: ReceiptCommitMode,
    ) -> Result<(), ManagedSkillInstallerError> {
        // Recheck immediately before staging. Besides defending the invariant,
        // this keeps the method safe if another internal call site is added.
        self.ensure_receipt_staging_capacity()?;
        let receipt_bytes = encode_receipt(
            installation_id,
            package.revision(),
            package.origin(),
            installed_at_unix_ms,
        )
        .map_err(|reason| ManagedSkillInstallerError::StoreCorrupt { reason })?;
        let mut staging = self.layout.unique_receipt_staging(installation_id)?;
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

        let target = self.layout.receipt_path(installation_id);
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
                installation_id,
                Some(package.revision()),
                "injected failure after receipt publish",
            ));
        }
        sync_directory(&self.layout.installations).map_err(|error| {
            self.commit_indeterminate(
                mutation,
                installation_id,
                Some(package.revision()),
                format!("cannot sync installations directory after receipt commit: {error}"),
            )
        })?;
        if self.should_fail(ManagedSkillInstallerFailpoint::ReceiptParentSynced) {
            return Err(self.commit_indeterminate(
                mutation,
                installation_id,
                Some(package.revision()),
                "injected failure after receipt directory sync",
            ));
        }
        Ok(())
    }

    fn sync_existing_commit(
        &self,
        mutation: ManagedSkillMutation,
        installation_id: &SkillInstallationId,
        intended_revision: Option<&SkillRevision>,
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
        intended_revision: Option<&SkillRevision>,
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
    UninstallRenamed,
    UninstallParentSynced,
}

#[derive(Debug)]
struct ManagedStoreLayout {
    root: PathBuf,
    installations: PathBuf,
    package_version: PathBuf,
}

impl ManagedStoreLayout {
    fn receipt_path(&self, installation_id: &SkillInstallationId) -> PathBuf {
        self.installations.join(format!("{installation_id}.json"))
    }

    fn unique_package_staging(
        &self,
        digest: &str,
    ) -> Result<StagingPath, ManagedSkillInstallerError> {
        create_unique_staging(
            &self.package_version,
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

    fn unique_uninstall_tombstone(
        &self,
        installation_id: &SkillInstallationId,
    ) -> Result<StagingPath, ManagedSkillInstallerError> {
        reserve_unique_staging_path(
            &self.installations,
            |nonce| format!(".uninstall-{installation_id}-{nonce}.tombstone"),
            StagingKind::Tombstone,
        )
    }
}

#[derive(Debug, Clone, Copy)]
enum StagingKind {
    File,
    Directory,
    Tombstone,
}

#[derive(Debug)]
struct StagingPath {
    path: PathBuf,
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
        if !self.armed {
            return;
        }
        match self.kind {
            StagingKind::File | StagingKind::Tombstone => {
                let _ = fs::remove_file(&self.path);
            }
            StagingKind::Directory => {
                // Removing a child through a re-resolved directory path could
                // escape the store if an attacker replaced the staging
                // directory. Only empty directories are safe to clean here;
                // non-empty crash remnants belong to a future handle-relative
                // garbage collector.
                let _ = fs::remove_dir(&self.path);
            }
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
                    kind,
                    // create_dir succeeded, so this transaction owns the
                    // directory. Drop can safely remove it while it is empty;
                    // once SKILL.md exists, remove_dir deliberately leaves the
                    // non-empty remnant for handle-relative garbage collection.
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
    let package_ref = InstalledPackageRef::from_revision(package.revision().clone())
        .map_err(|reason| ManagedSkillInstallerError::StoreCorrupt { reason })?;
    match store.load_package(&package_ref) {
        Ok(snapshot) if snapshot.bytes == package.source_bytes() => Ok(()),
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
        if count >= limit {
            return Err(ManagedSkillInstallerError::CapacityExceeded { capacity, limit });
        }
    }
    Ok(())
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

fn cleanup_stale_transaction_entries(
    layout: &ManagedStoreLayout,
) -> Result<(), ManagedSkillInstallerError> {
    cleanup_stale_transaction_entries_with_limit(layout, MAX_STAGING_CLEANUP_ENTRIES)
}

fn cleanup_stale_transaction_entries_with_limit(
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
        if !is_owned_receipt_staging_name(&name) && !is_owned_tombstone_name(&name) {
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
        fs::remove_file(entry.path()).map_err(|error| ManagedSkillInstallerError::Io {
            operation: "remove stale managed Skill receipt staging entry".to_string(),
            reason: error.to_string(),
        })?;
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

fn is_owned_receipt_staging_name(name: &str) -> bool {
    parse_two_uuid_name(name, ".receipt-", ".tmp")
}

fn is_owned_tombstone_name(name: &str) -> bool {
    parse_two_uuid_name(name, ".uninstall-", ".tombstone")
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
    use crate::skills::digest::PACKAGE_REVISION_PREFIX;
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

    fn install_request(id: &str, package: PreparedSkillPackage) -> ManagedSkillInstallRequest {
        let mut request =
            ManagedSkillInstallRequest::with_installation_id(installation_id(id), package);
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

    fn package_path(root: &Path, revision: &SkillRevision) -> PathBuf {
        root.join(PACKAGES_DIRECTORY)
            .join(PACKAGE_VERSION_DIRECTORY)
            .join(
                revision
                    .as_str()
                    .strip_prefix(PACKAGE_REVISION_PREFIX)
                    .unwrap(),
            )
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
            original_revision.clone(),
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
        assert_eq!(
            receipt.installed_at_unix_ms, 1_784_347_513_399,
            "updates preserve the original installation timestamp"
        );

        let conflict = ManagedSkillUpdateRequest::new(
            request.installation_id().clone(),
            original_revision.clone(),
            package("auditor", "CONFLICT"),
        );
        assert_eq!(
            installer.update(&conflict).unwrap_err().code(),
            ManagedSkillInstallerErrorCode::RevisionConflict
        );
        let wrong_uninstall =
            ManagedSkillUninstallRequest::new(request.installation_id().clone(), original_revision);
        assert_eq!(
            installer.uninstall(&wrong_uninstall).unwrap_err().code(),
            ManagedSkillInstallerErrorCode::RevisionConflict
        );

        let uninstall = ManagedSkillUninstallRequest::new(
            request.installation_id().clone(),
            updated_revision.clone(),
        );
        assert_eq!(
            installer.uninstall(&uninstall).unwrap(),
            ManagedSkillUninstallOutcome::Uninstalled
        );
        assert_eq!(
            installer.uninstall(&uninstall).unwrap(),
            ManagedSkillUninstallOutcome::AlreadyAbsent
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
            let update = ManagedSkillUpdateRequest::new(
                install.installation_id().clone(),
                old_revision.clone(),
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
                retry,
                ManagedSkillUpdateOutcome::Updated | ManagedSkillUpdateOutcome::AlreadyCurrent
            ));
            assert_eq!(
                installed_catalog(&root).skills()[0].revision(),
                &target_revision
            );
        }
    }

    #[test]
    fn uninstall_failpoints_commit_absence_and_retry_converges() {
        for failpoint in [
            ManagedSkillInstallerFailpoint::UninstallRenamed,
            ManagedSkillInstallerFailpoint::UninstallParentSynced,
        ] {
            let fixture = tempdir().unwrap();
            let root = fixture.path().join("store");
            let installer = ManagedSkillInstaller::new(&root).unwrap();
            let install = install_request(INSTALLATION_ID, package("auditor", "DELETE"));
            installer.install(&install).unwrap();
            let uninstall = ManagedSkillUninstallRequest::new(
                install.installation_id().clone(),
                install.package().revision().clone(),
            );
            let failing = ManagedSkillInstaller::with_failpoint(&root, failpoint);

            let error = failing.uninstall(&uninstall).unwrap_err();
            assert_eq!(
                error.code(),
                ManagedSkillInstallerErrorCode::CommitIndeterminate
            );
            assert!(installed_catalog(&root).skills().is_empty());
            assert_eq!(
                fs::read_dir(root.join(INSTALLATIONS_DIRECTORY))
                    .unwrap()
                    .filter_map(Result::ok)
                    .filter(|entry| entry
                        .file_name()
                        .to_str()
                        .is_some_and(is_owned_tombstone_name))
                    .count(),
                1,
                "an indeterminate uninstall retains its owned tombstone for recovery"
            );
            assert_eq!(
                ManagedSkillInstaller::new(&root)
                    .unwrap()
                    .uninstall(&uninstall)
                    .unwrap(),
                ManagedSkillUninstallOutcome::AlreadyAbsent
            );
            assert_eq!(
                fs::read_dir(root.join(INSTALLATIONS_DIRECTORY))
                    .unwrap()
                    .filter_map(Result::ok)
                    .filter(|entry| entry
                        .file_name()
                        .to_str()
                        .is_some_and(is_owned_tombstone_name))
                    .count(),
                0
            );
        }
    }

    #[test]
    fn live_installation_capacity_rejects_growth_but_allows_full_capacity_update() {
        let fixture = tempdir().unwrap();
        let root = fixture.path().join("store");
        let installer = ManagedSkillInstaller::new(&root).unwrap();
        let original = install_request(INSTALLATION_ID, package("auditor", "CAPACITY_OLD"));
        installer.install(&original).unwrap();
        let installations = root.join(INSTALLATIONS_DIRECTORY);
        let receipt_origin = original.package().origin();
        for value in 1..MAX_LIVE_INSTALLATIONS {
            let id =
                SkillInstallationId::parse(Uuid::from_u128(value as u128).to_string()).unwrap();
            let bytes = encode_receipt(
                &id,
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
            original.package().revision().clone(),
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
        outcomes.sort_by_key(|outcome| match outcome {
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
        let base_revision = install.package().revision().clone();
        installer.install(&install).unwrap();
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
                .filter(|result| matches!(result, Ok(ManagedSkillUpdateOutcome::Updated)))
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
    fn stale_receipt_staging_is_cleaned_without_path_based_package_deletion() {
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
            .join(PACKAGE_VERSION_DIRECTORY)
            .join(format!(".package-{digest}-{}.tmp", Uuid::new_v4()));
        fs::create_dir(&package_stage).unwrap();
        let unknown = root
            .join(INSTALLATIONS_DIRECTORY)
            .join(".third-party-state");
        fs::write(&unknown, b"preserve").unwrap();

        let second = install_request(SECOND_INSTALLATION_ID, package("second", "SECOND"));
        installer.install(&second).unwrap();

        assert!(!receipt_stage.exists());
        assert!(
            package_stage.exists(),
            "package staging cleanup requires a future handle-relative GC"
        );
        assert!(unknown.exists());
    }

    #[test]
    fn staging_guards_only_delete_objects_owned_by_the_transaction() {
        let fixture = tempdir().unwrap();
        let foreign_file = fixture.path().join("reserved-but-not-created");
        fs::write(&foreign_file, b"foreign").unwrap();
        drop(StagingPath {
            path: foreign_file.clone(),
            kind: StagingKind::File,
            armed: false,
        });
        assert_eq!(fs::read(&foreign_file).unwrap(), b"foreign");

        let owned_file = fixture.path().join("created-by-transaction");
        fs::write(&owned_file, b"owned").unwrap();
        drop(StagingPath {
            path: owned_file.clone(),
            kind: StagingKind::File,
            armed: true,
        });
        assert!(!owned_file.exists());

        let owned_empty_directory = fixture.path().join("empty-stage");
        fs::create_dir(&owned_empty_directory).unwrap();
        drop(StagingPath {
            path: owned_empty_directory.clone(),
            kind: StagingKind::Directory,
            armed: true,
        });
        assert!(!owned_empty_directory.exists());
    }

    #[test]
    fn staging_cleanup_budget_is_inclusive_and_fails_closed() {
        let fixture = tempdir().unwrap();
        let root = fixture.path().join("store");
        let installer = ManagedSkillInstaller::new(&root).unwrap();
        let request = install_request(INSTALLATION_ID, package("auditor", "BUDGET"));
        installer.install(&request).unwrap();
        let installations = root.join(INSTALLATIONS_DIRECTORY);
        let package_version = root
            .join(PACKAGES_DIRECTORY)
            .join(PACKAGE_VERSION_DIRECTORY);
        let layout = ManagedStoreLayout {
            root,
            installations: installations.clone(),
            package_version,
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
