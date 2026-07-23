use mycopilot_core::durable_fs::{atomic_rename_noreplace, sync_directory};
use mycopilot_core::storage::create_verified_sqlite_snapshot;
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const APP_DATA_ROOT_ENV: &str = "MYCOPILOT_APP_DATA_ROOT";
const STORAGE_DATABASE_ENV: &str = "MYCOPILOT_STORAGE_DB";
const STORAGE_DATABASE_FILE_NAME: &str = "storage.sqlite";
const SQLITE_TRANSIENT_SUFFIXES: [&str; 3] = ["-journal", "-wal", "-shm"];

static MIGRATION_STAGING_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DatabaseLocation {
    pub(crate) database_path: PathBuf,
    /// Present only when Electron supplied its authoritative application-data root.
    ///
    /// A standalone core-server override must never cause an implicit migration from the user's
    /// normal application profile.
    pub(crate) legacy_database_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LegacyStorageMigrationOutcome {
    SameRoot,
    TargetAlreadyInitialized,
    LegacyDatabaseAbsent,
    Migrated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LegacyStorageEntryState {
    Missing,
    AlreadyPublished,
    Publish,
}

pub(crate) fn database_location() -> io::Result<DatabaseLocation> {
    database_location_from_values(
        std::env::var_os(APP_DATA_ROOT_ENV).map(PathBuf::from),
        std::env::var_os(STORAGE_DATABASE_ENV).map(PathBuf::from),
        legacy_database_path(),
    )
}

pub(crate) fn database_location_from_values(
    app_data_root: Option<PathBuf>,
    standalone_database_override: Option<PathBuf>,
    legacy_database_path: Option<PathBuf>,
) -> io::Result<DatabaseLocation> {
    if let Some(app_data_root) = app_data_root {
        let app_data_root = validate_authoritative_app_data_root(&app_data_root)?;
        return Ok(DatabaseLocation {
            database_path: app_data_root.join(STORAGE_DATABASE_FILE_NAME),
            legacy_database_path,
        });
    }

    if let Some(database_path) = standalone_database_override {
        return Ok(DatabaseLocation {
            database_path,
            legacy_database_path: None,
        });
    }

    let database_path = legacy_database_path.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "cannot determine a default Core storage path because the platform user-data \
             environment is unavailable",
        )
    })?;
    Ok(DatabaseLocation {
        database_path,
        legacy_database_path: None,
    })
}

fn validate_authoritative_app_data_root(root: &Path) -> io::Result<PathBuf> {
    if root.as_os_str().is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{APP_DATA_ROOT_ENV} cannot be empty"),
        ));
    }
    if !root.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "{APP_DATA_ROOT_ENV} must be an absolute path, received `{}`",
                root.display()
            ),
        ));
    }
    fs::create_dir_all(root)?;
    let metadata = fs::symlink_metadata(root)?;
    if metadata.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "{APP_DATA_ROOT_ENV} must not name a symbolic link: {}",
                root.display()
            ),
        ));
    }
    if !metadata.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "{APP_DATA_ROOT_ENV} must name a directory: {}",
                root.display()
            ),
        ));
    }
    fs::canonicalize(root)
}

pub(crate) fn legacy_database_path() -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        return absolute_platform_directory(std::env::var_os("HOME")).map(|home| {
            home.join("Library")
                .join("Application Support")
                .join("mycopilot-next")
                .join(STORAGE_DATABASE_FILE_NAME)
        });
    }

    if cfg!(target_os = "windows") {
        return absolute_platform_directory(std::env::var_os("APPDATA")).map(|app_data| {
            app_data
                .join("mycopilot-next")
                .join(STORAGE_DATABASE_FILE_NAME)
        });
    }

    if let Some(xdg_data_home) = absolute_platform_directory(std::env::var_os("XDG_DATA_HOME")) {
        return Some(
            xdg_data_home
                .join("mycopilot-next")
                .join(STORAGE_DATABASE_FILE_NAME),
        );
    }
    absolute_platform_directory(std::env::var_os("HOME")).map(|home| {
        home.join(".local")
            .join("share")
            .join("mycopilot-next")
            .join(STORAGE_DATABASE_FILE_NAME)
    })
}

fn absolute_platform_directory(value: Option<OsString>) -> Option<PathBuf> {
    value
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty() && path.is_absolute())
}

pub(crate) fn acquire_database_instance_lock(database_path: &Path) -> io::Result<File> {
    let parent = database_path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let canonical_parent = fs::canonicalize(parent)?;
    let file_name = database_path.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "storage database path has no file name: {}",
                database_path.display()
            ),
        )
    })?;
    let database_identity = match fs::symlink_metadata(database_path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "storage database path must not be a symbolic link: {}",
                    database_path.display()
                ),
            ));
        }
        Ok(metadata) if metadata.file_type().is_file() => fs::canonicalize(database_path)?,
        Ok(_) => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "storage database path must be a regular file: {}",
                    database_path.display()
                ),
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => canonical_parent.join(file_name),
        Err(error) => return Err(error),
    };
    let lock_file_name = format!(
        ".{}.core-server.lock",
        database_identity
            .file_name()
            .expect("database identity retains a file name")
            .to_string_lossy()
    );
    let lock_path = database_identity.with_file_name(lock_file_name);
    let lock = open_lock_file(&lock_path)?;
    if !lock.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "core-server instance lock must be a regular file: {}",
                lock_path.display()
            ),
        ));
    }
    if let Err(error) = lock.try_lock() {
        let (kind, message) = match error {
            fs::TryLockError::WouldBlock => (
                io::ErrorKind::AlreadyExists,
                format!(
                    "another core-server already owns storage `{}` (instance lock `{}`)",
                    database_identity.display(),
                    lock_path.display()
                ),
            ),
            fs::TryLockError::Error(error) => (
                error.kind(),
                format!(
                    "failed to acquire the core-server instance lock `{}` for storage `{}`: \
                     {error}",
                    lock_path.display(),
                    database_identity.display()
                ),
            ),
        };
        return Err(io::Error::new(kind, message));
    }
    Ok(lock)
}

/// Copies the Core-owned legacy profile into Electron's authoritative application-data root.
///
/// The caller must already own the target database instance lock. This function additionally
/// owns the legacy database lock while it validates and publishes entries. Legacy data remains a
/// rollback copy. Directories are copied into private same-directory staging, durably flushed,
/// and atomically published without replacement. SQLite creates and validates its own consistent
/// snapshot; raw journal, WAL, and SHM files are never copied. The database is published last as
/// the migration commit boundary.
pub(crate) fn migrate_legacy_storage_if_needed(
    legacy_database_path: &Path,
    target_database_path: &Path,
    include_development_credentials: bool,
) -> io::Result<LegacyStorageMigrationOutcome> {
    if storage_paths_refer_to_same_location(legacy_database_path, target_database_path) {
        return Ok(LegacyStorageMigrationOutcome::SameRoot);
    }

    if validated_entry_presence(
        target_database_path,
        EntryKind::File,
        "target storage database",
    )? {
        return Ok(LegacyStorageMigrationOutcome::TargetAlreadyInitialized);
    }
    if !validated_entry_presence(
        legacy_database_path,
        EntryKind::File,
        "legacy storage database",
    )? {
        return Ok(LegacyStorageMigrationOutcome::LegacyDatabaseAbsent);
    }

    // The target lock is held by the caller. This second lock prevents snapshotting a database
    // still owned by an older, cooperative core-server.
    let _legacy_database_lock = acquire_database_instance_lock(legacy_database_path)?;
    if !validated_entry_presence(
        legacy_database_path,
        EntryKind::File,
        "legacy storage database",
    )? {
        return Ok(LegacyStorageMigrationOutcome::LegacyDatabaseAbsent);
    }
    if validated_entry_presence(
        target_database_path,
        EntryKind::File,
        "target storage database",
    )? {
        return Ok(LegacyStorageMigrationOutcome::TargetAlreadyInitialized);
    }

    let legacy_root = database_parent(legacy_database_path, "legacy")?;
    let target_root = database_parent(target_database_path, "target")?;
    fs::create_dir_all(target_root)?;
    reject_target_sqlite_transients(target_database_path)?;

    let entries = legacy_storage_directories(include_development_credentials);
    let states = entries
        .iter()
        .map(|name| {
            migration_entry_state(
                &legacy_root.join(name),
                &target_root.join(name),
                EntryKind::Directory,
            )
        })
        .collect::<io::Result<Vec<_>>>()?;

    // Complete preflight before publishing even the first directory.
    for (name, state) in entries.iter().zip(states) {
        if state == LegacyStorageEntryState::Publish {
            publish_directory_snapshot(&legacy_root.join(name), &target_root.join(name))?;
        }
    }

    // SQLite's online backup incorporates or recovers any source journal or WAL and emits one
    // self-contained, quick_check-verified database. Its atomic publication commits migration.
    publish_database_snapshot(legacy_database_path, target_database_path)?;
    Ok(LegacyStorageMigrationOutcome::Migrated)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EntryKind {
    File,
    Directory,
}

fn legacy_storage_directories(include_development_credentials: bool) -> Vec<&'static str> {
    let mut entries = vec!["attachments", "skills", "image-generation-artifacts"];
    if include_development_credentials {
        entries.push("image-generation-development-credentials-v1");
    }
    entries
}

fn database_parent<'a>(database_path: &'a Path, role: &str) -> io::Result<&'a Path> {
    database_path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "{role} storage database has no parent: {}",
                database_path.display()
            ),
        )
    })
}

fn reject_target_sqlite_transients(target_database_path: &Path) -> io::Result<()> {
    for suffix in SQLITE_TRANSIENT_SUFFIXES {
        let transient = sqlite_transient_path(target_database_path, suffix);
        match fs::symlink_metadata(&transient) {
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!(
                        "refusing migration because target SQLite transient `{}` already exists",
                        transient.display()
                    ),
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn migration_entry_state(
    source: &Path,
    target: &Path,
    kind: EntryKind,
) -> io::Result<LegacyStorageEntryState> {
    let source_exists = validated_entry_presence(source, kind, "legacy migration source")?;
    let target_exists = validated_entry_presence(target, kind, "migration target")?;
    if source_exists && kind == EntryKind::Directory {
        validate_directory_tree(source, "legacy migration source")?;
    }
    if target_exists && kind == EntryKind::Directory {
        validate_directory_tree(target, "migration target")?;
    }
    match (source_exists, target_exists) {
        (false, false) => Ok(LegacyStorageEntryState::Missing),
        (false, true) => Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!(
                "refusing to merge orphaned target storage entry `{}` because legacy source `{}` \
                 is absent",
                target.display(),
                source.display()
            ),
        )),
        (true, false) => Ok(LegacyStorageEntryState::Publish),
        (true, true) if entries_are_identical(source, target, kind)? => {
            Ok(LegacyStorageEntryState::AlreadyPublished)
        }
        (true, true) => Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!(
                "refusing to merge legacy storage because `{}` and `{}` differ",
                source.display(),
                target.display()
            ),
        )),
    }
}

fn validate_directory_tree(directory: &Path, role: &str) -> io::Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = fs::symlink_metadata(&path)?.file_type();
        if file_type.is_symlink() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "refusing storage migration because {role} `{}` is a symbolic link",
                    path.display()
                ),
            ));
        }
        if file_type.is_dir() {
            validate_directory_tree(&path, role)?;
        } else if !file_type.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "refusing storage migration because {role} `{}` has a special file type",
                    path.display()
                ),
            ));
        }
    }
    Ok(())
}

fn publish_directory_snapshot(source: &Path, target: &Path) -> io::Result<()> {
    let target_root = database_parent(target, "migration entry")?;
    let staging = create_migration_staging_entry(target, EntryKind::Directory)?;
    if let Err(error) = copy_directory_durably(source, &staging) {
        remove_staging_entry(&staging, EntryKind::Directory);
        return Err(error);
    }
    match entries_are_identical(source, &staging, EntryKind::Directory) {
        Ok(true) => {}
        Ok(false) => {
            remove_staging_entry(&staging, EntryKind::Directory);
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "legacy storage source `{}` changed while its snapshot was prepared",
                    source.display()
                ),
            ));
        }
        Err(error) => {
            remove_staging_entry(&staging, EntryKind::Directory);
            return Err(error);
        }
    }
    publish_staging_entry(source, &staging, target, EntryKind::Directory)?;
    sync_directory(target_root)
}

fn publish_database_snapshot(source: &Path, target: &Path) -> io::Result<()> {
    let target_root = database_parent(target, "target")?;
    let staging = create_migration_staging_entry(target, EntryKind::File)?;
    if let Err(error) = create_verified_sqlite_snapshot(source, &staging) {
        remove_sqlite_staging(&staging);
        return Err(error);
    }
    if let Some(transient) = sqlite_transient_that_exists(&staging)? {
        remove_sqlite_staging(&staging);
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "verified SQLite snapshot left unexpected transient `{}`",
                transient.display()
            ),
        ));
    }
    match atomic_rename_noreplace(&staging, target) {
        Ok(()) => sync_directory(target_root),
        Err(error) => {
            remove_sqlite_staging(&staging);
            Err(atomic_publication_error(&staging, target, error))
        }
    }
}

fn publish_staging_entry(
    source: &Path,
    staging: &Path,
    target: &Path,
    kind: EntryKind,
) -> io::Result<()> {
    match atomic_rename_noreplace(staging, target) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            let identical = entries_are_identical(source, target, kind);
            remove_staging_entry(staging, kind);
            match identical {
                Ok(true) => Ok(()),
                Ok(false) => Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!(
                        "migration target `{}` appeared with content that differs from `{}`",
                        target.display(),
                        source.display()
                    ),
                )),
                Err(error) => Err(error),
            }
        }
        Err(error) => {
            remove_staging_entry(staging, kind);
            Err(atomic_publication_error(staging, target, error))
        }
    }
}

fn atomic_publication_error(staging: &Path, target: &Path, error: io::Error) -> io::Error {
    if error.kind() == io::ErrorKind::CrossesDevices {
        io::Error::new(
            io::ErrorKind::CrossesDevices,
            format!(
                "cannot publish storage migration staging `{}` to `{}` across filesystems; \
                 staging and Electron userData must share a filesystem",
                staging.display(),
                target.display()
            ),
        )
    } else {
        io::Error::new(
            error.kind(),
            format!(
                "failed to atomically publish storage migration `{}` without replacing `{}`: \
                 {error}",
                staging.display(),
                target.display()
            ),
        )
    }
}

fn create_migration_staging_entry(target: &Path, kind: EntryKind) -> io::Result<PathBuf> {
    let parent = database_parent(target, "migration entry")?;
    let name = target.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("migration target has no file name: {}", target.display()),
        )
    })?;
    for _ in 0..128 {
        let sequence = MIGRATION_STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let staging = parent.join(format!(
            ".{}.core-storage-migration-{}-{sequence}.staging",
            name.to_string_lossy(),
            std::process::id()
        ));
        let result = match kind {
            EntryKind::File => create_private_file(&staging).map(|_| ()),
            EntryKind::Directory => create_private_directory(&staging),
        };
        match result {
            Ok(()) => return Ok(staging),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        format!(
            "could not allocate private migration staging beside `{}`",
            target.display()
        ),
    ))
}

fn copy_regular_file_durably(source: &Path, target: &Path) -> io::Result<()> {
    validated_entry_presence(source, EntryKind::File, "legacy migration source")?;
    let mut source_file = open_regular_file_for_read(source)?;
    let mut target_file = open_existing_file_for_write(target)?;
    std::io::copy(&mut source_file, &mut target_file)?;
    target_file.flush()?;
    target_file.sync_all()?;
    apply_private_source_permissions(source, target, EntryKind::File)?;
    target_file.sync_all()
}

fn copy_directory_durably(source: &Path, target: &Path) -> io::Result<()> {
    validated_entry_presence(source, EntryKind::Directory, "legacy migration source")?;
    for child in fs::read_dir(source)? {
        let child = child?;
        let source_child = child.path();
        let target_child = target.join(child.file_name());
        let metadata = fs::symlink_metadata(&source_child)?;
        let file_type = metadata.file_type();
        if file_type.is_symlink() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "refusing storage migration because `{}` is a symbolic link",
                    source_child.display()
                ),
            ));
        }
        if file_type.is_file() {
            create_private_file(&target_child)?;
            copy_regular_file_durably(&source_child, &target_child)?;
        } else if file_type.is_dir() {
            create_private_directory(&target_child)?;
            copy_directory_durably(&source_child, &target_child)?;
        } else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "refusing storage migration because `{}` is not a regular file or directory",
                    source_child.display()
                ),
            ));
        }
    }
    apply_private_source_permissions(source, target, EntryKind::Directory)?;
    sync_directory(target)
}

#[cfg(unix)]
fn create_private_file(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(path)
}

#[cfg(not(unix))]
fn create_private_file(path: &Path) -> io::Result<File> {
    OpenOptions::new().create_new(true).write(true).open(path)
}

#[cfg(unix)]
fn create_private_directory(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    let mut builder = fs::DirBuilder::new();
    builder.mode(0o700).create(path)
}

#[cfg(not(unix))]
fn create_private_directory(path: &Path) -> io::Result<()> {
    fs::create_dir(path)
}

#[cfg(unix)]
fn apply_private_source_permissions(
    source: &Path,
    target: &Path,
    kind: EntryKind,
) -> io::Result<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let source_mode = fs::symlink_metadata(source)?.mode();
    let private_mode = match kind {
        EntryKind::File => 0o600 | (source_mode & 0o100),
        EntryKind::Directory => 0o700,
    };
    fs::set_permissions(target, fs::Permissions::from_mode(private_mode))
}

#[cfg(not(unix))]
fn apply_private_source_permissions(
    _source: &Path,
    _target: &Path,
    _kind: EntryKind,
) -> io::Result<()> {
    Ok(())
}

fn entries_are_identical(source: &Path, target: &Path, kind: EntryKind) -> io::Result<bool> {
    validated_entry_presence(source, kind, "legacy migration source")?;
    validated_entry_presence(target, kind, "migration target")?;
    match kind {
        EntryKind::File => regular_files_are_identical(source, target),
        EntryKind::Directory => directories_are_identical(source, target),
    }
}

fn regular_files_are_identical(left: &Path, right: &Path) -> io::Result<bool> {
    let left_metadata = fs::symlink_metadata(left)?;
    let right_metadata = fs::symlink_metadata(right)?;
    if left_metadata.len() != right_metadata.len() {
        return Ok(false);
    }
    let mut left = open_regular_file_for_read(left)?;
    let mut right = open_regular_file_for_read(right)?;
    let mut left_buffer = [0_u8; 64 * 1024];
    let mut right_buffer = [0_u8; 64 * 1024];
    loop {
        let left_read = left.read(&mut left_buffer)?;
        let right_read = right.read(&mut right_buffer)?;
        if left_read != right_read || left_buffer[..left_read] != right_buffer[..right_read] {
            return Ok(false);
        }
        if left_read == 0 {
            return Ok(true);
        }
    }
}

fn directories_are_identical(left: &Path, right: &Path) -> io::Result<bool> {
    let mut left_children = fs::read_dir(left)?
        .map(|entry| entry.map(|entry| entry.file_name()))
        .collect::<io::Result<Vec<_>>>()?;
    let mut right_children = fs::read_dir(right)?
        .map(|entry| entry.map(|entry| entry.file_name()))
        .collect::<io::Result<Vec<_>>>()?;
    left_children.sort();
    right_children.sort();
    if left_children != right_children {
        return Ok(false);
    }
    for name in left_children {
        let left_child = left.join(&name);
        let right_child = right.join(name);
        let left_type = fs::symlink_metadata(&left_child)?.file_type();
        let right_type = fs::symlink_metadata(&right_child)?.file_type();
        if left_type.is_symlink() || right_type.is_symlink() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "refusing storage migration because `{}` or `{}` is a symbolic link",
                    left_child.display(),
                    right_child.display()
                ),
            ));
        }
        if !(left_type.is_file() || left_type.is_dir())
            || !(right_type.is_file() || right_type.is_dir())
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "refusing storage migration because `{}` or `{}` has a special file type",
                    left_child.display(),
                    right_child.display()
                ),
            ));
        }
        if left_type.is_file() && right_type.is_file() {
            if !regular_files_are_identical(&left_child, &right_child)? {
                return Ok(false);
            }
        } else if left_type.is_dir() && right_type.is_dir() {
            if !directories_are_identical(&left_child, &right_child)? {
                return Ok(false);
            }
        } else {
            return Ok(false);
        }
    }
    Ok(true)
}

fn validated_entry_presence(path: &Path, expected_kind: EntryKind, role: &str) -> io::Result<bool> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    let file_type = metadata.file_type();
    if file_type.is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "refusing storage migration because {role} `{}` is a symbolic link",
                path.display()
            ),
        ));
    }
    let matches_expected_kind = match expected_kind {
        EntryKind::File => file_type.is_file(),
        EntryKind::Directory => file_type.is_dir(),
    };
    if !matches_expected_kind {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "refusing storage migration because {role} `{}` is not a regular {}",
                path.display(),
                match expected_kind {
                    EntryKind::File => "file",
                    EntryKind::Directory => "directory",
                }
            ),
        ));
    }
    Ok(true)
}

fn sqlite_transient_that_exists(database_path: &Path) -> io::Result<Option<PathBuf>> {
    for suffix in SQLITE_TRANSIENT_SUFFIXES {
        let transient = sqlite_transient_path(database_path, suffix);
        match fs::symlink_metadata(&transient) {
            Ok(_) => return Ok(Some(transient)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(None)
}

fn remove_sqlite_staging(database_path: &Path) {
    remove_staging_entry(database_path, EntryKind::File);
    for suffix in SQLITE_TRANSIENT_SUFFIXES {
        remove_staging_entry(
            &sqlite_transient_path(database_path, suffix),
            EntryKind::File,
        );
    }
}

fn sqlite_transient_path(database_path: &Path, suffix: &str) -> PathBuf {
    let mut path = database_path.as_os_str().to_os_string();
    path.push(suffix);
    PathBuf::from(path)
}

fn remove_staging_entry(path: &Path, kind: EntryKind) {
    let result = match kind {
        EntryKind::File => fs::remove_file(path),
        EntryKind::Directory => fs::remove_dir_all(path),
    };
    if let Err(error) = result {
        if error.kind() != io::ErrorKind::NotFound {
            eprintln!(
                "failed to remove private storage migration staging `{}`: {error}",
                path.display()
            );
        }
    }
}

fn storage_paths_refer_to_same_location(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    if left.file_name() != right.file_name() {
        return false;
    }
    let (Some(left_parent), Some(right_parent)) = (left.parent(), right.parent()) else {
        return false;
    };
    match (
        fs::canonicalize(left_parent),
        fs::canonicalize(right_parent),
    ) {
        (Ok(left_parent), Ok(right_parent)) => left_parent == right_parent,
        _ => false,
    }
}

#[cfg(unix)]
fn open_lock_file(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
}

#[cfg(not(unix))]
fn open_lock_file(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
}

#[cfg(unix)]
fn open_regular_file_for_read(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("expected a regular file: {}", path.display()),
        ));
    }
    Ok(file)
}

#[cfg(not(unix))]
fn open_regular_file_for_read(path: &Path) -> io::Result<File> {
    let file = File::open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("expected a regular file: {}", path.display()),
        ));
    }
    Ok(file)
}

#[cfg(unix)]
fn open_existing_file_for_write(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    let file = OpenOptions::new()
        .write(true)
        .truncate(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("expected a regular file: {}", path.display()),
        ));
    }
    Ok(file)
}

#[cfg(not(unix))]
fn open_existing_file_for_write(path: &Path) -> io::Result<File> {
    let file = OpenOptions::new().write(true).truncate(true).open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("expected a regular file: {}", path.display()),
        ));
    }
    Ok(file)
}
