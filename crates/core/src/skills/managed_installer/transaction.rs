use super::*;

pub(super) struct ManagedStoreTransaction<'a> {
    pub(super) installer: &'a ManagedSkillInstaller,
    pub(super) _process_guard: MutexGuard<'a, ()>,
    pub(super) _writer_lock: File,
    pub(super) layout: ManagedStoreLayout,
    pub(super) store: ManagedSkillStore,
}

impl ManagedStoreTransaction<'_> {
    pub(super) fn retired_identity_exists(
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

    pub(super) fn ensure_installation_id_available(
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

    pub(super) fn ensure_live_identity_not_retired(
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

    pub(super) fn ensure_new_installation_capacity(
        &self,
    ) -> Result<(), ManagedSkillInstallerError> {
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

    pub(super) fn ensure_receipt_staging_capacity(&self) -> Result<(), ManagedSkillInstallerError> {
        let (directory_entries, _) = count_installation_entries(&self.layout.installations)?;
        ensure_count_has_room(
            directory_entries,
            MAX_INSTALLATION_DIRECTORY_ENTRIES,
            ManagedSkillStoreCapacity::InstallationDirectory,
        )
    }

    pub(super) fn publish_package(
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

    pub(super) fn write_staged_package(
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

    pub(super) fn write_staged_file(
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

    pub(super) fn ensure_existing_package_durable(
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

    pub(super) fn commit_receipt(
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

    pub(super) fn sync_existing_commit(
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
    pub(super) fn retire_installation(
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

    pub(super) fn publish_retired_identity(
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

    pub(super) fn io_error(&self, operation: &str, error: io::Error) -> ManagedSkillInstallerError {
        ManagedSkillInstallerError::Io {
            operation: operation.to_string(),
            reason: error.to_string(),
        }
    }

    pub(super) fn injected_io(&self, operation: &str) -> ManagedSkillInstallerError {
        ManagedSkillInstallerError::Io {
            operation: operation.to_string(),
            reason: "injected transaction failure".to_string(),
        }
    }

    pub(super) fn commit_indeterminate(
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

    pub(super) fn should_fail(&self, point: ManagedSkillInstallerFailpoint) -> bool {
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
pub(super) enum ReceiptCommitMode {
    Create,
    Replace,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ManagedSkillInstallerFailpoint {
    PackageFileSynced,
    PackagePublished,
    PackageParentSynced,
    ReceiptFileSynced,
    ReceiptPublished,
    ReceiptParentSynced,
    UninstallRetirementPublished,
    UninstallParentSynced,
}
