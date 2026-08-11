use rusqlite::backup::Backup;
use rusqlite::{Connection, OpenFlags};
use std::fs::{self, OpenOptions};
use std::io;
use std::path::Path;
use std::time::Duration;

/// Creates a transactionally consistent SQLite snapshot and verifies it before returning.
///
/// The source is opened through SQLite rather than copied as bytes, so a hot rollback journal or
/// WAL is recovered and incorporated by SQLite itself. The caller owns target staging and remains
/// responsible for publishing it atomically after this function closes every SQLite handle.
pub fn create_verified_sqlite_snapshot(source: &Path, target: &Path) -> io::Result<()> {
    validate_snapshot_path(source, SnapshotPathRole::Source)?;
    validate_snapshot_path(target, SnapshotPathRole::EmptyTarget)?;
    let canonical_source = fs::canonicalize(source)?;
    let canonical_target = fs::canonicalize(target)?;

    let source_open_flags = OpenFlags::SQLITE_OPEN_READ_ONLY
        | OpenFlags::SQLITE_OPEN_NO_MUTEX
        | OpenFlags::SQLITE_OPEN_NOFOLLOW;
    let target_open_flags = OpenFlags::SQLITE_OPEN_READ_WRITE
        | OpenFlags::SQLITE_OPEN_NO_MUTEX
        | OpenFlags::SQLITE_OPEN_NOFOLLOW;
    let source_connection = Connection::open_with_flags(&canonical_source, source_open_flags)
        .map_err(|error| {
            io::Error::other(format!(
                "failed to open SQLite snapshot source `{}`: {error}",
                source.display()
            ))
        })?;
    source_connection
        .busy_timeout(Duration::from_secs(5))
        .map_err(sqlite_error)?;

    let mut target_connection = Connection::open_with_flags(&canonical_target, target_open_flags)
        .map_err(|error| {
        io::Error::other(format!(
            "failed to open SQLite snapshot staging `{}`: {error}",
            target.display()
        ))
    })?;
    {
        let backup =
            Backup::new(&source_connection, &mut target_connection).map_err(sqlite_error)?;
        backup
            .run_to_completion(256, Duration::from_millis(1), None)
            .map_err(sqlite_error)?;
    }

    let quick_check = target_connection
        .prepare("PRAGMA quick_check")
        .and_then(|mut statement| {
            statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .map_err(sqlite_error)?;
    if quick_check.as_slice() != ["ok"] {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "SQLite snapshot `{}` failed quick_check: {}",
                target.display(),
                quick_check.join("; ")
            ),
        ));
    }

    target_connection
        .close()
        .map_err(|(_, error)| sqlite_error(error))?;
    source_connection
        .close()
        .map_err(|(_, error)| sqlite_error(error))?;
    OpenOptions::new()
        .read(true)
        .write(true)
        .open(canonical_target)?
        .sync_all()
}

fn sqlite_error(error: rusqlite::Error) -> io::Error {
    io::Error::other(format!("SQLite snapshot failed: {error}"))
}

#[derive(Clone, Copy)]
enum SnapshotPathRole {
    Source,
    EmptyTarget,
}

fn validate_snapshot_path(path: &Path, role: SnapshotPathRole) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "SQLite snapshot {} must be a regular file: {}",
                match role {
                    SnapshotPathRole::Source => "source",
                    SnapshotPathRole::EmptyTarget => "target staging",
                },
                path.display()
            ),
        ));
    }
    if matches!(role, SnapshotPathRole::EmptyTarget) && metadata.len() != 0 {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!(
                "SQLite snapshot target staging must be empty: {}",
                path.display()
            ),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_empty_file(path: &Path) {
        OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(path)
            .unwrap();
    }

    #[test]
    fn snapshot_includes_committed_rows_that_still_live_in_wal() {
        let fixture = tempfile::tempdir().unwrap();
        let source = fixture.path().join("source.sqlite");
        let target = fixture.path().join("target.sqlite");
        let source_connection = Connection::open(&source).unwrap();
        source_connection
            .pragma_update(None, "journal_mode", "WAL")
            .unwrap();
        source_connection
            .pragma_update(None, "wal_autocheckpoint", 0)
            .unwrap();
        source_connection
            .execute_batch(
                "CREATE TABLE evidence(value TEXT NOT NULL);
                 INSERT INTO evidence(value) VALUES ('committed-in-wal');",
            )
            .unwrap();
        assert!(sqlite_transient_path(&source, "-wal").exists());
        create_empty_file(&target);

        create_verified_sqlite_snapshot(&source, &target).unwrap();

        for suffix in ["-journal", "-wal", "-shm"] {
            assert!(!sqlite_transient_path(&target, suffix).exists());
        }
        let snapshot = Connection::open(&target).unwrap();
        let value: String = snapshot
            .query_row("SELECT value FROM evidence", [], |row| row.get(0))
            .unwrap();
        assert_eq!(value, "committed-in-wal");
    }

    #[cfg(unix)]
    #[test]
    fn snapshot_never_requires_write_access_to_the_source() {
        use std::os::unix::fs::PermissionsExt;

        let fixture = tempfile::tempdir().unwrap();
        let source = fixture.path().join("source.sqlite");
        let target = fixture.path().join("target.sqlite");
        let connection = Connection::open(&source).unwrap();
        connection
            .execute_batch("CREATE TABLE evidence(value TEXT NOT NULL);")
            .unwrap();
        drop(connection);
        fs::set_permissions(&source, fs::Permissions::from_mode(0o400)).unwrap();
        create_empty_file(&target);

        create_verified_sqlite_snapshot(&source, &target).unwrap();

        assert_eq!(
            fs::metadata(source).unwrap().permissions().mode() & 0o777,
            0o400
        );
    }

    #[cfg(unix)]
    #[test]
    fn snapshot_rejects_a_symbolic_link_target() {
        use std::os::unix::fs::symlink;

        let fixture = tempfile::tempdir().unwrap();
        let source = fixture.path().join("source.sqlite");
        let actual_target = fixture.path().join("actual-target.sqlite");
        let linked_target = fixture.path().join("linked-target.sqlite");
        drop(Connection::open(&source).unwrap());
        create_empty_file(&actual_target);
        symlink(&actual_target, &linked_target).unwrap();

        let error = create_verified_sqlite_snapshot(&source, &linked_target).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert_eq!(fs::metadata(actual_target).unwrap().len(), 0);
    }

    fn sqlite_transient_path(database_path: &Path, suffix: &str) -> std::path::PathBuf {
        let mut path = database_path.as_os_str().to_os_string();
        path.push(suffix);
        path.into()
    }
}
