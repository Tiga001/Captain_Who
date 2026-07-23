//! Private, durable credential storage for unsigned development builds.
//!
//! This backend intentionally exists outside macOS Keychain. A frequently rebuilt
//! ad-hoc-signed helper has no stable designated requirement, so reading an item
//! created by yesterday's binary can cause an operating-system password prompt.
//! Development builds use this user-private store instead; signed distributions
//! use the non-interactive native adapter.

use std::{
    fmt,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard},
};

#[cfg(unix)]
use std::os::unix::{
    fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    io::AsRawFd,
};

use crate::durable_fs::{atomic_replace, sync_directory};
use zeroize::Zeroizing;

use super::{
    ensure_supported_reference, CredentialDeleteOutcome, CredentialReference, CredentialSecret,
    CredentialStore, CredentialStoreBackend, CredentialStoreError, CredentialStoreOperation,
};

const FILE_HEADER: &[u8] = b"MYCOPILOT_DEV_CREDENTIAL_V1\n";
const MAX_SECRET_BYTES: usize = 8_192;
const MAX_FILE_BYTES: usize = FILE_HEADER.len() + MAX_SECRET_BYTES;

/// Development-only credential store rooted in a private application directory.
///
/// The root is canonicalized at construction and its filesystem identity is
/// rechecked before every operation. Credential paths are derived solely from a
/// validated random identifier, never from user-controlled path text.
pub struct DevelopmentFileCredentialStore {
    root: PathBuf,
    root_identity: DirectoryIdentity,
    operation_lock: Mutex<()>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DirectoryIdentity {
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}

impl DevelopmentFileCredentialStore {
    /// Opens or creates a private development credential directory.
    pub fn new(root: impl AsRef<Path>) -> Result<Self, CredentialStoreError> {
        let root = prepare_root(root.as_ref())?;
        let root_identity = validate_root(&root, CredentialStoreOperation::Open)?;
        Ok(Self {
            root,
            root_identity,
            operation_lock: Mutex::new(()),
        })
    }

    fn lock(
        &self,
        operation: CredentialStoreOperation,
    ) -> Result<MutexGuard<'_, ()>, CredentialStoreError> {
        self.operation_lock
            .lock()
            .map_err(|_| CredentialStoreError::BackendFailure { operation })
    }

    fn verify_root(&self, operation: CredentialStoreOperation) -> Result<(), CredentialStoreError> {
        let current = validate_root(&self.root, operation)?;
        if current == self.root_identity {
            Ok(())
        } else {
            Err(CredentialStoreError::AccessDenied { operation })
        }
    }

    fn path(&self, reference: &CredentialReference) -> PathBuf {
        self.root
            .join(format!("{}.credential", reference.opaque_id()))
    }
}

impl fmt::Debug for DevelopmentFileCredentialStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DevelopmentFileCredentialStore")
            .finish_non_exhaustive()
    }
}

impl CredentialStore for DevelopmentFileCredentialStore {
    fn backend(&self) -> CredentialStoreBackend {
        CredentialStoreBackend::DevelopmentFileV1
    }

    fn replace(
        &self,
        reference: &CredentialReference,
        secret: CredentialSecret,
    ) -> Result<(), CredentialStoreError> {
        const OPERATION: CredentialStoreOperation = CredentialStoreOperation::Replace;
        ensure_supported_reference(self, reference)?;
        let _guard = self.lock(OPERATION)?;
        self.verify_root(OPERATION)?;
        let secret_is_valid =
            secret.with_secret_bytes(|bytes| !bytes.is_empty() && bytes.len() <= MAX_SECRET_BYTES);
        if !secret_is_valid {
            return Err(CredentialStoreError::InvalidSecret);
        }

        let target = self.path(reference);
        credential_path_exists(&target, OPERATION)?;

        let staging = self.root.join(format!(
            ".{}.{}.tmp",
            reference.opaque_id(),
            uuid::Uuid::new_v4().simple()
        ));
        let result = secret.with_secret_bytes(|secret_bytes| {
            let mut file = open_staging_file(&staging, OPERATION)?;
            file.write_all(FILE_HEADER)
                .and_then(|()| file.write_all(secret_bytes))
                .and_then(|()| file.sync_all())
                .map_err(|error| classify_io_error(OPERATION, &error))?;
            validate_open_file(&file, OPERATION)?;
            drop(file);
            self.verify_root(OPERATION)?;
            atomic_replace(&staging, &target)
                .map_err(|error| classify_io_error(OPERATION, &error))?;
            sync_directory(&self.root).map_err(|error| classify_io_error(OPERATION, &error))
        });
        if result.is_err() {
            let _ = fs::remove_file(&staging);
        }
        result
    }

    fn get(
        &self,
        reference: &CredentialReference,
    ) -> Result<Option<CredentialSecret>, CredentialStoreError> {
        const OPERATION: CredentialStoreOperation = CredentialStoreOperation::Get;
        ensure_supported_reference(self, reference)?;
        let _guard = self.lock(OPERATION)?;
        self.verify_root(OPERATION)?;
        let path = self.path(reference);
        if !credential_path_exists(&path, OPERATION)? {
            return Ok(None);
        }
        let mut file = open_credential_file(&path, OPERATION)?;
        validate_open_file(&file, OPERATION)?;
        let metadata = file
            .metadata()
            .map_err(|error| classify_io_error(OPERATION, &error))?;
        if metadata.len() == 0 || metadata.len() > MAX_FILE_BYTES as u64 {
            return Err(CredentialStoreError::CorruptedEntry {
                operation: OPERATION,
            });
        }
        let mut bytes = Zeroizing::new(Vec::with_capacity(metadata.len() as usize));
        file.read_to_end(&mut bytes)
            .map_err(|error| classify_io_error(OPERATION, &error))?;
        let Some(secret) = bytes.strip_prefix(FILE_HEADER) else {
            return Err(CredentialStoreError::CorruptedEntry {
                operation: OPERATION,
            });
        };
        if secret.is_empty() || secret.len() > MAX_SECRET_BYTES {
            return Err(CredentialStoreError::CorruptedEntry {
                operation: OPERATION,
            });
        }
        let secret =
            std::str::from_utf8(secret).map_err(|_| CredentialStoreError::CorruptedEntry {
                operation: OPERATION,
            })?;
        CredentialSecret::new(secret.to_owned()).map(Some)
    }

    fn delete(
        &self,
        reference: &CredentialReference,
    ) -> Result<CredentialDeleteOutcome, CredentialStoreError> {
        const OPERATION: CredentialStoreOperation = CredentialStoreOperation::Delete;
        ensure_supported_reference(self, reference)?;
        let _guard = self.lock(OPERATION)?;
        self.verify_root(OPERATION)?;
        let path = self.path(reference);
        if !credential_path_exists(&path, OPERATION)? {
            return Ok(CredentialDeleteOutcome::NotFound);
        }
        fs::remove_file(&path).map_err(|error| classify_io_error(OPERATION, &error))?;
        sync_directory(&self.root).map_err(|error| classify_io_error(OPERATION, &error))?;
        Ok(CredentialDeleteOutcome::Deleted)
    }
}

#[cfg(unix)]
fn prepare_root(path: &Path) -> Result<PathBuf, CredentialStoreError> {
    const OPERATION: CredentialStoreOperation = CredentialStoreOperation::Open;
    let name = path
        .file_name()
        .filter(|name| *name != "." && *name != "..")
        .ok_or(CredentialStoreError::AccessDenied {
            operation: OPERATION,
        })?;
    let parent = path
        .parent()
        .ok_or(CredentialStoreError::AccessDenied {
            operation: OPERATION,
        })?
        .canonicalize()
        .map_err(|error| classify_io_error(OPERATION, &error))?;
    let path = parent.join(name);
    match fs::symlink_metadata(&path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(CredentialStoreError::AccessDenied {
                    operation: OPERATION,
                });
            }
            if metadata.uid() != unsafe { libc::geteuid() } {
                return Err(CredentialStoreError::AccessDenied {
                    operation: OPERATION,
                });
            }
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700))
                .map_err(|error| classify_io_error(OPERATION, &error))?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::DirBuilder::new()
                .recursive(false)
                .mode(0o700)
                .create(&path)
                .map_err(|error| classify_io_error(OPERATION, &error))?;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700))
                .map_err(|error| classify_io_error(OPERATION, &error))?;
        }
        Err(error) => return Err(classify_io_error(OPERATION, &error)),
    }
    let canonical = path
        .canonicalize()
        .map_err(|error| classify_io_error(OPERATION, &error))?;
    if canonical != path {
        return Err(CredentialStoreError::AccessDenied {
            operation: OPERATION,
        });
    }
    Ok(canonical)
}

#[cfg(not(unix))]
fn prepare_root(_path: &Path) -> Result<PathBuf, CredentialStoreError> {
    Err(CredentialStoreError::Unsupported {
        operation: CredentialStoreOperation::Open,
    })
}

#[cfg(unix)]
fn validate_root(
    path: &Path,
    operation: CredentialStoreOperation,
) -> Result<DirectoryIdentity, CredentialStoreError> {
    let metadata =
        fs::symlink_metadata(path).map_err(|error| classify_io_error(operation, &error))?;
    if metadata.file_type().is_symlink()
        || !metadata.is_dir()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
    {
        return Err(CredentialStoreError::AccessDenied { operation });
    }
    Ok(DirectoryIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}

#[cfg(not(unix))]
fn validate_root(
    _path: &Path,
    operation: CredentialStoreOperation,
) -> Result<DirectoryIdentity, CredentialStoreError> {
    Err(CredentialStoreError::Unsupported { operation })
}

#[cfg(unix)]
fn credential_path_exists(
    path: &Path,
    operation: CredentialStoreOperation,
) -> Result<bool, CredentialStoreError> {
    match fs::symlink_metadata(path) {
        Ok(_) => {
            validate_credential_path(path, operation)?;
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(classify_io_error(operation, &error)),
    }
}

#[cfg(not(unix))]
fn credential_path_exists(
    _path: &Path,
    operation: CredentialStoreOperation,
) -> Result<bool, CredentialStoreError> {
    Err(CredentialStoreError::Unsupported { operation })
}

#[cfg(unix)]
fn validate_credential_path(
    path: &Path,
    operation: CredentialStoreOperation,
) -> Result<(), CredentialStoreError> {
    let metadata =
        fs::symlink_metadata(path).map_err(|error| classify_io_error(operation, &error))?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
        || metadata.len() > MAX_FILE_BYTES as u64
    {
        return Err(CredentialStoreError::CorruptedEntry { operation });
    }
    Ok(())
}

#[cfg(not(unix))]
fn validate_credential_path(
    _path: &Path,
    operation: CredentialStoreOperation,
) -> Result<(), CredentialStoreError> {
    Err(CredentialStoreError::Unsupported { operation })
}

#[cfg(unix)]
fn open_staging_file(
    path: &Path,
    operation: CredentialStoreOperation,
) -> Result<File, CredentialStoreError> {
    let mut options = OpenOptions::new();
    options
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    options
        .open(path)
        .map_err(|error| classify_io_error(operation, &error))
}

#[cfg(not(unix))]
fn open_staging_file(
    _path: &Path,
    operation: CredentialStoreOperation,
) -> Result<File, CredentialStoreError> {
    Err(CredentialStoreError::Unsupported { operation })
}

#[cfg(unix)]
fn open_credential_file(
    path: &Path,
    operation: CredentialStoreOperation,
) -> Result<File, CredentialStoreError> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    options
        .open(path)
        .map_err(|error| classify_io_error(operation, &error))
}

#[cfg(not(unix))]
fn open_credential_file(
    _path: &Path,
    operation: CredentialStoreOperation,
) -> Result<File, CredentialStoreError> {
    Err(CredentialStoreError::Unsupported { operation })
}

#[cfg(unix)]
fn validate_open_file(
    file: &File,
    operation: CredentialStoreOperation,
) -> Result<(), CredentialStoreError> {
    let metadata = file
        .metadata()
        .map_err(|error| classify_io_error(operation, &error))?;
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
        || metadata.len() > MAX_FILE_BYTES as u64
    {
        return Err(CredentialStoreError::CorruptedEntry { operation });
    }
    let flags = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETFD) };
    if flags < 0 || flags & libc::FD_CLOEXEC == 0 {
        return Err(CredentialStoreError::BackendFailure { operation });
    }
    Ok(())
}

#[cfg(not(unix))]
fn validate_open_file(
    _file: &File,
    operation: CredentialStoreOperation,
) -> Result<(), CredentialStoreError> {
    Err(CredentialStoreError::Unsupported { operation })
}

fn classify_io_error(
    operation: CredentialStoreOperation,
    error: &std::io::Error,
) -> CredentialStoreError {
    match error.kind() {
        std::io::ErrorKind::PermissionDenied => CredentialStoreError::AccessDenied { operation },
        std::io::ErrorKind::InvalidData => CredentialStoreError::CorruptedEntry { operation },
        std::io::ErrorKind::Unsupported => CredentialStoreError::Unsupported { operation },
        _ => CredentialStoreError::BackendFailure { operation },
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::{fs::hard_link, os::unix::fs::symlink};
    use tempfile::tempdir;

    fn secret_text(secret: &CredentialSecret) -> String {
        secret.with_secret_bytes(|bytes| String::from_utf8(bytes.to_vec()).unwrap())
    }

    #[test]
    fn persists_round_trip_with_private_permissions() {
        let directory = tempdir().unwrap();
        let root = directory.path().join("credentials");
        let store = DevelopmentFileCredentialStore::new(&root).unwrap();
        let reference = store.new_reference();
        store
            .replace(
                &reference,
                CredentialSecret::new("development-key").unwrap(),
            )
            .unwrap();
        drop(store);

        let reopened = DevelopmentFileCredentialStore::new(&root).unwrap();
        assert_eq!(
            secret_text(&reopened.get(&reference).unwrap().unwrap()),
            "development-key"
        );
        assert_eq!(fs::symlink_metadata(&root).unwrap().mode() & 0o777, 0o700);
        assert_eq!(
            fs::symlink_metadata(reopened.path(&reference))
                .unwrap()
                .mode()
                & 0o777,
            0o600
        );
    }

    #[test]
    fn rejects_foreign_references_without_touching_files() {
        let directory = tempdir().unwrap();
        let store = DevelopmentFileCredentialStore::new(directory.path().join("store")).unwrap();
        let foreign = CredentialReference::new_for_backend(CredentialStoreBackend::InMemoryV1);

        assert_eq!(
            store.get(&foreign).unwrap_err(),
            CredentialStoreError::InvalidReference
        );
    }

    #[test]
    fn rejects_symlink_and_hardlink_entries() {
        let directory = tempdir().unwrap();
        let root = directory.path().join("store");
        let store = DevelopmentFileCredentialStore::new(&root).unwrap();
        let reference = store.new_reference();
        let external = directory.path().join("external");
        fs::write(&external, [FILE_HEADER, b"secret"].concat()).unwrap();
        symlink(&external, store.path(&reference)).unwrap();
        assert!(matches!(
            store.get(&reference),
            Err(CredentialStoreError::CorruptedEntry { .. })
        ));
        fs::remove_file(store.path(&reference)).unwrap();
        hard_link(&external, store.path(&reference)).unwrap();
        assert!(matches!(
            store.get(&reference),
            Err(CredentialStoreError::CorruptedEntry { .. })
        ));
    }

    #[test]
    fn rejects_overly_permissive_and_oversized_entries() {
        let directory = tempdir().unwrap();
        let root = directory.path().join("store");
        let store = DevelopmentFileCredentialStore::new(&root).unwrap();
        let reference = store.new_reference();
        store
            .replace(&reference, CredentialSecret::new("secret").unwrap())
            .unwrap();
        fs::set_permissions(store.path(&reference), fs::Permissions::from_mode(0o644)).unwrap();
        assert!(matches!(
            store.get(&reference),
            Err(CredentialStoreError::CorruptedEntry { .. })
        ));

        let oversized = CredentialSecret::new("x".repeat(MAX_SECRET_BYTES + 1)).unwrap();
        assert_eq!(
            store
                .replace(&store.new_reference(), oversized)
                .unwrap_err(),
            CredentialStoreError::InvalidSecret
        );
    }

    #[test]
    fn delete_is_idempotent_and_durable() {
        let directory = tempdir().unwrap();
        let store = DevelopmentFileCredentialStore::new(directory.path().join("store")).unwrap();
        let reference = store.new_reference();
        store
            .replace(&reference, CredentialSecret::new("secret").unwrap())
            .unwrap();

        assert_eq!(
            store.delete(&reference).unwrap(),
            CredentialDeleteOutcome::Deleted
        );
        assert_eq!(
            store.delete(&reference).unwrap(),
            CredentialDeleteOutcome::NotFound
        );
    }

    #[test]
    fn detects_root_replacement_and_corrupted_content() {
        let directory = tempdir().unwrap();
        let root = directory.path().join("store");
        let store = DevelopmentFileCredentialStore::new(&root).unwrap();
        let reference = store.new_reference();
        store
            .replace(&reference, CredentialSecret::new("secret").unwrap())
            .unwrap();
        fs::write(store.path(&reference), b"not-a-credential").unwrap();
        assert!(matches!(
            store.get(&reference),
            Err(CredentialStoreError::CorruptedEntry { .. })
        ));

        let retired = directory.path().join("retired");
        fs::rename(&root, &retired).unwrap();
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(matches!(
            store.get(&reference),
            Err(CredentialStoreError::AccessDenied { .. })
        ));
    }
}
