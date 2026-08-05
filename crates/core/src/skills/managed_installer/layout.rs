use super::*;

#[derive(Debug)]
pub(super) struct ManagedStoreLayout {
    pub(super) root: PathBuf,
    pub(super) installations: PathBuf,
    pub(super) retired_installations: PathBuf,
    pub(super) package_v1: PathBuf,
    pub(super) package_v2: PathBuf,
    pub(super) package_v3: PathBuf,
}

impl ManagedStoreLayout {
    pub(super) fn package_version(&self, format_version: u32) -> &Path {
        match format_version {
            SKILL_PACKAGE_FORMAT_VERSION => &self.package_v1,
            SKILL_PACKAGE_FORMAT_VERSION_V2 => &self.package_v2,
            SKILL_PACKAGE_FORMAT_VERSION_V3 => &self.package_v3,
            _ => unreachable!("PreparedSkillPackage validates format version"),
        }
    }

    pub(super) fn receipt_path(&self, installation_id: &SkillInstallationId) -> PathBuf {
        self.installations.join(format!("{installation_id}.json"))
    }

    pub(super) fn retired_path(&self, installation_id: &SkillInstallationId) -> PathBuf {
        self.retired_installations
            .join(format!("{installation_id}.json"))
    }

    pub(super) fn unique_package_staging(
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

    pub(super) fn unique_receipt_staging(
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
pub(super) enum StagingKind {
    File,
    Directory,
}

#[derive(Debug)]
pub(super) struct StagingPath {
    pub(super) path: PathBuf,
    pub(super) parent: PathBuf,
    pub(super) kind: StagingKind,
    pub(super) armed: bool,
}

impl StagingPath {
    pub(super) fn arm(&mut self) {
        self.armed = true;
    }

    pub(super) fn disarm(&mut self) {
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
    pub(super) fn has_owned_name(&self) -> bool {
        let Some(name) = self.path.file_name().and_then(OsStr::to_str) else {
            return false;
        };
        match self.kind {
            StagingKind::File => is_owned_receipt_staging_name(name),
            StagingKind::Directory => is_owned_package_staging_name(name),
        }
    }
}

pub(super) fn create_unique_staging(
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

pub(super) fn reserve_unique_staging_path(
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

pub(super) fn load_receipt(
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

pub(super) fn verify_prepared_package(
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

pub(super) fn ensure_store_root(root: &Path) -> Result<PathBuf, ManagedSkillInstallerError> {
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

pub(super) fn ensure_exact_directory(
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

pub(super) fn validate_plain_directory(
    path: &Path,
    label: &str,
) -> Result<(), ManagedSkillInstallerError> {
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

pub(super) fn acquire_writer_lock(root: &Path) -> Result<File, ManagedSkillInstallerError> {
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

pub(super) fn directory_has_exact_entry(
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

pub(super) fn verify_writer_lock_binding(
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

pub(super) fn count_installation_entries(
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

pub(super) fn ensure_directory_has_room(
    directory: &Path,
    limit: usize,
    capacity: ManagedSkillStoreCapacity,
) -> Result<(), ManagedSkillInstallerError> {
    let count = count_directory_entries(directory, limit, capacity)?;
    ensure_count_has_room(count, limit, capacity)
}

pub(super) fn count_directory_entries(
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

pub(super) fn ensure_count_has_room(
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

pub(super) fn ensure_retirement_marker_capacity(
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

pub(super) fn cleanup_stale_transaction_entries(
    layout: &ManagedStoreLayout,
) -> Result<(), ManagedSkillInstallerError> {
    cleanup_stale_transaction_entries_with_limit(layout, MAX_STAGING_CLEANUP_ENTRIES)
}

pub(super) fn cleanup_stale_transaction_entries_with_limit(
    layout: &ManagedStoreLayout,
    max_entries: usize,
) -> Result<(), ManagedSkillInstallerError> {
    cleanup_stale_installation_entries_with_limit(layout, max_entries)?;
    for package_version in [&layout.package_v1, &layout.package_v2, &layout.package_v3] {
        cleanup_stale_package_entries_with_limit(package_version, max_entries)?;
    }
    Ok(())
}

pub(super) fn cleanup_stale_installation_entries_with_limit(
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

pub(super) fn cleanup_stale_package_entries_with_limit(
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

pub(super) fn remove_owned_package_staging_directory(
    parent: &Path,
    path: &Path,
) -> io::Result<bool> {
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

pub(super) fn validate_package_staging_tree(root: &Path) -> io::Result<()> {
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

pub(super) fn invalid_staging_tree(reason: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, reason)
}

pub(super) fn is_owned_receipt_staging_name(name: &str) -> bool {
    parse_two_uuid_name(name, ".receipt-", ".tmp")
}

pub(super) fn canonical_receipt_installation_id(name: &str) -> Option<SkillInstallationId> {
    SkillInstallationId::parse(name.strip_suffix(".json")?).ok()
}

pub(super) fn is_owned_package_staging_name(name: &str) -> bool {
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

pub(super) fn owned_tombstone_installation_id(name: &str) -> Option<SkillInstallationId> {
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

pub(super) fn parse_two_uuid_name(name: &str, prefix: &str, suffix: &str) -> bool {
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

pub(super) fn unix_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}
