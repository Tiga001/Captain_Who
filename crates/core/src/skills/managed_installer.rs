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

mod api;
mod layout;
#[cfg(test)]
mod tests;
mod transaction;

pub use api::*;
use layout::*;
use transaction::*;

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
        let installation_id = request.installation_id();
        let transaction = self.begin_transaction()?;
        let Some(receipt) = load_receipt(&transaction.store, installation_id)? else {
            // An uninstall also retires an already-absent identity. This
            // closes the crash window in which a previously deleted receipt
            // name was not durable and an old install retry could otherwise
            // resurrect it after restart.
            transaction.retire_installation(installation_id, false)?;
            return Ok(ManagedSkillMutationResult::absent(
                ManagedSkillUninstallOutcome::AlreadyAbsent,
            ));
        };
        if receipt.installation_revision != *request.expected_revision() {
            return Err(ManagedSkillInstallerError::RevisionConflict {
                installation_id: installation_id.clone(),
                expected_revision: request.expected_revision().clone(),
                actual_revision: receipt.installation_revision,
            });
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
