use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::Path;

/// Acquires the process-wide ownership lock for one exact SQLite database.
///
/// Core Server holds this lock for its complete storage lifetime. Destructive development tools
/// must acquire the same lock before inspecting, snapshotting, or replacing the database, so a
/// reset can never race a live application process even when SQLite itself is momentarily idle.
pub fn acquire_database_instance_lock(database_path: &Path) -> io::Result<File> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_owner_is_rejected_for_the_same_absent_database() {
        let fixture = tempfile::tempdir().unwrap();
        let database = fixture.path().join("storage.sqlite");
        let first = acquire_database_instance_lock(&database).unwrap();

        let error = acquire_database_instance_lock(&database).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        drop(first);
        acquire_database_instance_lock(&database).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn rejects_a_symbolic_link_database_identity() {
        use std::os::unix::fs::symlink;

        let fixture = tempfile::tempdir().unwrap();
        let target = fixture.path().join("target.sqlite");
        File::create(&target).unwrap();
        let linked = fixture.path().join("storage.sqlite");
        symlink(&target, &linked).unwrap();

        let error = acquire_database_instance_lock(&linked).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[cfg(unix)]
    #[test]
    fn rejects_a_symbolic_link_lock_file_without_following_it() {
        use std::os::unix::fs::symlink;

        let fixture = tempfile::tempdir().unwrap();
        let database = fixture.path().join("storage.sqlite");
        let target = fixture.path().join("lock-target");
        File::create(&target).unwrap();
        let lock = fixture.path().join(".storage.sqlite.core-server.lock");
        symlink(&target, &lock).unwrap();

        let error = acquire_database_instance_lock(&database).unwrap_err();

        assert!(error.raw_os_error().is_some());
        assert_eq!(fs::metadata(target).unwrap().len(), 0);
    }
}
