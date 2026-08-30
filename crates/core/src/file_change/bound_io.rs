use super::error::{FileChangeError, FileChangeErrorCode, FileChangeResultValue};
use super::policy::ResolvedFileChangeTarget;
#[cfg(unix)]
use std::fs::File;
use std::fs::Permissions;
use std::io;
use std::path::Path;

/// A descriptor-bound capability for reading one leaf without reopening its ancestors by path.
///
/// `read_file` authorizes and canonicalizes the target before constructing this capability. On
/// Unix, every parent component is then reopened relative to the previously opened directory with
/// `O_NOFOLLOW`; the leaf is opened relative to the final held parent descriptor. A raced ancestor
/// can therefore neither redirect the read nor the metadata used by a FileObservation.
#[cfg(unix)]
pub(crate) struct BoundReadParent {
    canonical_parent: std::path::PathBuf,
    directory: File,
    leaf: std::ffi::CString,
}

#[cfg(unix)]
impl BoundReadParent {
    pub(crate) fn bind(target: &Path) -> io::Result<Self> {
        if !target.is_absolute() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "bound read target must be absolute",
            ));
        }
        let canonical_parent = target
            .parent()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "target has no parent"))?
            .to_path_buf();
        let leaf = unix::standalone_leaf_name(target)?;
        let directory = unix::open_directory_chain(&canonical_parent)?;
        Ok(Self {
            canonical_parent,
            directory,
            leaf,
        })
    }

    pub(crate) fn open_leaf(&self) -> io::Result<File> {
        unix::open_read_leaf(&self.directory, &self.leaf)
    }

    pub(crate) fn parent_metadata(&self) -> io::Result<std::fs::Metadata> {
        self.directory.metadata()
    }

    /// Inspects the leaf relative to the held parent without following links or reopening the
    /// target by pathname. The returned metadata has been checked against both the still-named
    /// directory entry and the canonical parent path.
    pub(crate) fn inspect_optional_leaf(&self) -> FileChangeResultValue<Option<std::fs::Metadata>> {
        let file = match self.open_leaf() {
            Ok(file) => file,
            Err(error) if error.raw_os_error() == Some(libc::ENOENT) => {
                return if self.is_current_missing_leaf().map_err(bound_read_error)? {
                    Ok(None)
                } else {
                    Err(FileChangeError::new(FileChangeErrorCode::Conflict))
                };
            }
            Err(error) => return Err(bound_read_error(error)),
        };
        let metadata = file.metadata().map_err(bound_read_error)?;
        unix::validate_open_metadata(&metadata)?;
        if !self
            .is_current_leaf(&file, &metadata)
            .map_err(bound_read_error)?
        {
            return Err(FileChangeError::new(FileChangeErrorCode::Conflict));
        }
        Ok(Some(metadata))
    }

    /// Confirms absence relative to the held parent and proves that the authorized canonical
    /// parent path still names that same directory. The second descriptor-relative absence check
    /// closes a leaf creation race during parent-path revalidation.
    pub(crate) fn is_current_missing_leaf(&self) -> io::Result<bool> {
        if !unix::leaf_is_missing(&self.directory, &self.leaf)? {
            return Ok(false);
        }
        let current_parent = unix::open_directory_chain(&self.canonical_parent)?;
        if !unix::same_directory(&self.directory, &current_parent)? {
            return Ok(false);
        }
        unix::leaf_is_missing(&self.directory, &self.leaf)
    }

    /// Verifies that both the canonical parent path and its named leaf still resolve to the held
    /// descriptors. Returning `false` is a stable stale-read result; syscall failures remain
    /// distinguishable so callers can preserve the symlink-specific safe message.
    pub(crate) fn is_current_leaf(
        &self,
        file: &File,
        initial_metadata: &std::fs::Metadata,
    ) -> io::Result<bool> {
        let current_file_metadata = file.metadata()?;
        if !unix::same_open_file(initial_metadata, &current_file_metadata) {
            return Ok(false);
        }
        let current_parent = unix::open_directory_chain(&self.canonical_parent)?;
        if !unix::same_directory(&self.directory, &current_parent)? {
            return Ok(false);
        }
        unix::named_leaf_matches(&self.directory, &self.leaf, &current_file_metadata)
    }
}

#[derive(Debug)]
pub(crate) struct BoundCurrentFile {
    pub bytes: Vec<u8>,
    pub permissions: Permissions,
    pub metadata: std::fs::Metadata,
}

/// A capability for one already-resolved target directory.
///
/// Unix mutations use the held directory descriptor for every leaf operation. Pathname
/// revalidation remains a precondition check, but a raced ancestor can no longer redirect a read,
/// staging write, rename, removal, or durability sync into another directory.
pub(crate) struct BoundParent<'target> {
    target: &'target ResolvedFileChangeTarget,
    #[cfg(unix)]
    directory: File,
}

impl<'target> BoundParent<'target> {
    pub fn open(target: &'target ResolvedFileChangeTarget) -> FileChangeResultValue<Self> {
        target.revalidate()?;

        #[cfg(unix)]
        {
            let directory = unix::open_directory(target.parent()).map_err(open_parent_error)?;
            // Recheck the pathname after opening, then prove that the opened capability is the
            // directory identity frozen by policy. This closes swaps on any ancestor during open.
            target.revalidate()?;
            if !unix::directory_matches_path(&directory, target.parent())
                .map_err(open_parent_error)?
            {
                return Err(FileChangeError::new(FileChangeErrorCode::Conflict));
            }
            Ok(Self { target, directory })
        }

        #[cfg(not(unix))]
        {
            // Non-Unix platforms retain conservative pathname revalidation. The public API is the
            // same, so a stronger handle-relative backend can replace this without changing the
            // committer contract.
            Ok(Self { target })
        }
    }

    pub fn revalidate(&self) -> FileChangeResultValue<()> {
        self.target.revalidate()?;
        #[cfg(unix)]
        if !unix::directory_matches_path(&self.directory, self.target.parent())
            .map_err(open_parent_error)?
        {
            return Err(FileChangeError::new(FileChangeErrorCode::Conflict));
        }
        Ok(())
    }

    pub fn target_path(&self) -> &Path {
        self.target.absolute_path()
    }

    pub fn parent_metadata(&self) -> FileChangeResultValue<std::fs::Metadata> {
        #[cfg(unix)]
        {
            self.directory.metadata().map_err(open_parent_error)
        }

        #[cfg(not(unix))]
        {
            std::fs::metadata(self.target.parent()).map_err(open_parent_error)
        }
    }

    pub fn read_required(&self, path: &Path) -> FileChangeResultValue<BoundCurrentFile> {
        self.read_optional(path)?
            .ok_or_else(|| FileChangeError::new(FileChangeErrorCode::FileMissing))
    }

    pub fn read_optional(&self, path: &Path) -> FileChangeResultValue<Option<BoundCurrentFile>> {
        self.read_optional_with_limit(path, None)
    }

    pub(crate) fn read_optional_bounded(
        &self,
        path: &Path,
        max_bytes: u64,
    ) -> FileChangeResultValue<Option<BoundCurrentFile>> {
        self.read_optional_with_limit(path, Some(max_bytes))
    }

    fn read_optional_with_limit(
        &self,
        path: &Path,
        max_bytes: Option<u64>,
    ) -> FileChangeResultValue<Option<BoundCurrentFile>> {
        #[cfg(unix)]
        {
            let name = unix::leaf_name(self.target.parent(), path)?;
            unix::read_optional(&self.directory, &name, max_bytes)
        }

        #[cfg(not(unix))]
        {
            self.revalidate()?;
            fallback::read_optional(self.target.parent(), path, max_bytes)
        }
    }

    pub(super) fn stage(
        &self,
        content: &str,
        permissions: Option<Permissions>,
    ) -> FileChangeResultValue<BoundStagedFile> {
        #[cfg(unix)]
        {
            unix::stage(&self.directory, content, permissions)
        }

        #[cfg(not(unix))]
        {
            self.revalidate()?;
            fallback::stage(self.target.parent(), content, permissions)
        }
    }

    pub(super) fn publish_noreplace(
        &self,
        staged: BoundStagedFile,
        target: &Path,
    ) -> io::Result<()> {
        #[cfg(unix)]
        {
            let target = unix::leaf_name_io(self.target.parent(), target)?;
            unix::publish_noreplace(&self.directory, staged, &target)
        }

        #[cfg(not(unix))]
        {
            crate::durable_fs::atomic_rename_noreplace(staged.path.as_ref(), target)
        }
    }

    /// Atomically exchanges a private staged inode with the current target entry.
    ///
    /// The entry moved to the private staging name is deliberately retained until the caller has
    /// verified that it is the exact Base generation. Unsupported platforms fail before changing
    /// either name.
    pub(super) fn publish_exchange(
        &self,
        staged: BoundStagedFile,
        target: &Path,
    ) -> io::Result<BoundUpdateExchange> {
        #[cfg(unix)]
        {
            let target = unix::leaf_name_io(self.target.parent(), target)?;
            unix::publish_exchange(&self.directory, staged, &target)
        }

        #[cfg(not(unix))]
        {
            let _ = (staged, target);
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "platform does not support descriptor-relative atomic exchange",
            ))
        }
    }

    pub(super) fn read_exchanged(
        &self,
        exchanged: &BoundUpdateExchange,
    ) -> FileChangeResultValue<BoundCurrentFile> {
        #[cfg(unix)]
        {
            unix::read_optional(&self.directory, &exchanged.staged.name, None)?
                .ok_or_else(|| FileChangeError::new(FileChangeErrorCode::OutcomeUnknown))
        }

        #[cfg(not(unix))]
        {
            let _ = exchanged;
            Err(FileChangeError::new(FileChangeErrorCode::Failed))
        }
    }

    pub(super) fn same_file_identity(
        &self,
        left: &BoundCurrentFile,
        right: &BoundCurrentFile,
    ) -> bool {
        #[cfg(unix)]
        {
            unix::same_file_identity(&left.metadata, &right.metadata)
        }

        #[cfg(not(unix))]
        {
            let _ = (left, right);
            false
        }
    }

    /// Exchanges the retained entry back into the target name. The caller must verify that the
    /// private name now contains its own staged Target before asking to unlink it.
    pub(super) fn rollback_exchange(
        &self,
        exchanged: &mut BoundUpdateExchange,
        target: &Path,
    ) -> io::Result<()> {
        #[cfg(unix)]
        {
            let target = unix::leaf_name_io(self.target.parent(), target)?;
            unix::rollback_exchange(&self.directory, exchanged, &target)
        }

        #[cfg(not(unix))]
        {
            let _ = (exchanged, target);
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "platform does not support descriptor-relative atomic exchange",
            ))
        }
    }

    pub(super) fn exchanged_is_staged_target(
        &self,
        exchanged: &BoundUpdateExchange,
        current: &BoundCurrentFile,
    ) -> io::Result<bool> {
        #[cfg(unix)]
        {
            unix::exchanged_is_staged_target(exchanged, current)
        }

        #[cfg(not(unix))]
        {
            let _ = (exchanged, current);
            Ok(false)
        }
    }

    /// Removes an exchanged entry only after the committer has proven what it contains.
    pub(super) fn remove_exchanged(&self, exchanged: BoundUpdateExchange) -> io::Result<()> {
        #[cfg(unix)]
        {
            // Never retry via Drop: after this call returns, a replacement of the private entry
            // would be unverified and must remain linked.
            unix::remove(&self.directory, &exchanged.staged.name)
        }

        #[cfg(not(unix))]
        {
            let _ = exchanged;
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "platform does not support descriptor-relative atomic exchange",
            ))
        }
    }

    pub fn rename_noreplace(&self, source: &Path, target: &Path) -> io::Result<()> {
        #[cfg(unix)]
        {
            let source = unix::leaf_name_io(self.target.parent(), source)?;
            let target = unix::leaf_name_io(self.target.parent(), target)?;
            unix::rename_noreplace(&self.directory, &source, &target)
        }

        #[cfg(not(unix))]
        {
            crate::durable_fs::atomic_rename_noreplace(source, target)
        }
    }

    pub fn remove(&self, path: &Path) -> io::Result<()> {
        #[cfg(unix)]
        {
            let name = unix::leaf_name_io(self.target.parent(), path)?;
            unix::remove(&self.directory, &name)
        }

        #[cfg(not(unix))]
        {
            std::fs::remove_file(path)
        }
    }

    pub fn sync(&self) -> io::Result<()> {
        #[cfg(unix)]
        {
            self.directory.sync_all()
        }

        #[cfg(not(unix))]
        {
            crate::durable_fs::sync_directory(self.target.parent())
        }
    }
}

#[cfg(unix)]
pub(super) struct BoundStagedFile {
    directory: File,
    name: std::ffi::CString,
    file: File,
    cleanup_name: bool,
}

#[cfg(unix)]
impl Drop for BoundStagedFile {
    fn drop(&mut self) {
        if self.cleanup_name {
            let _ = unix::remove(&self.directory, &self.name);
        }
    }
}

#[cfg(not(unix))]
pub(super) struct BoundStagedFile {
    path: tempfile::TempPath,
}

pub(super) struct BoundUpdateExchange {
    staged: BoundStagedFile,
}

#[cfg(all(test, unix))]
pub(super) fn fail_exchange_call(call: usize) {
    unix::fail_exchange_call(call);
}

fn open_parent_error(error: io::Error) -> FileChangeError {
    #[cfg(unix)]
    if error.raw_os_error() == Some(libc::ELOOP) {
        return FileChangeError::with_diagnostic(
            FileChangeErrorCode::SymlinkForbidden,
            error.to_string(),
        );
    }
    let code = match error.kind() {
        io::ErrorKind::NotFound => FileChangeErrorCode::ParentMissing,
        io::ErrorKind::PermissionDenied => FileChangeErrorCode::PermissionDenied,
        _ => FileChangeErrorCode::Failed,
    };
    FileChangeError::with_diagnostic(code, error.to_string())
}

#[cfg(unix)]
fn bound_read_error(error: io::Error) -> FileChangeError {
    let code = match error.raw_os_error() {
        Some(libc::ELOOP) => FileChangeErrorCode::SymlinkForbidden,
        Some(libc::EACCES) | Some(libc::EPERM) => FileChangeErrorCode::PermissionDenied,
        _ => match error.kind() {
            io::ErrorKind::NotFound => FileChangeErrorCode::FileMissing,
            io::ErrorKind::PermissionDenied => FileChangeErrorCode::PermissionDenied,
            _ => FileChangeErrorCode::Failed,
        },
    };
    FileChangeError::with_diagnostic(code, error.to_string())
}

#[cfg(unix)]
mod unix {
    use super::super::error::{FileChangeError, FileChangeErrorCode, FileChangeResultValue};
    use super::{BoundCurrentFile, BoundStagedFile, BoundUpdateExchange};
    #[cfg(test)]
    use std::cell::Cell;
    use std::ffi::{CStr, CString};
    use std::fs::{self, File, Permissions};
    use std::io::{self, Read, Write};
    use std::mem::MaybeUninit;
    use std::os::fd::{AsRawFd, FromRawFd, RawFd};
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    use std::path::{Component, Path};

    pub fn open_directory_chain(path: &Path) -> io::Result<File> {
        if !path.is_absolute() {
            return Err(invalid_path("directory path must be absolute"));
        }
        let mut directory = open_directory(Path::new("/"))?;
        for component in path.components() {
            match component {
                Component::RootDir | Component::CurDir => {}
                Component::Normal(name) => {
                    let name = CString::new(name.as_bytes())
                        .map_err(|_| invalid_path("directory component contains NUL"))?;
                    directory = open_child_directory(&directory, &name)?;
                }
                Component::ParentDir | Component::Prefix(_) => {
                    return Err(invalid_path("unsupported directory component"));
                }
            }
        }
        Ok(directory)
    }

    pub fn standalone_leaf_name(path: &Path) -> io::Result<CString> {
        let name = path
            .file_name()
            .ok_or_else(|| invalid_path("path has no file name"))?;
        CString::new(name.as_bytes()).map_err(|_| invalid_path("file name contains NUL"))
    }

    pub fn open_read_leaf(directory: &File, name: &CStr) -> io::Result<File> {
        // O_NONBLOCK prevents a raced special file from blocking before `read_file` validates the
        // descriptor type. O_NOFOLLOW makes the final component capability-safe.
        // SAFETY: the held directory descriptor and NUL-terminated name remain live for the call.
        let descriptor = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
            )
        };
        if descriptor < 0 {
            Err(classify_nofollow_error(
                directory,
                name,
                io::Error::last_os_error(),
            ))
        } else {
            // SAFETY: openat returned a new owned descriptor.
            Ok(unsafe { File::from_raw_fd(descriptor) })
        }
    }

    pub fn same_directory(left: &File, right: &File) -> io::Result<bool> {
        let left = left.metadata()?;
        let right = right.metadata()?;
        Ok(left.is_dir()
            && right.is_dir()
            && left.dev() == right.dev()
            && left.ino() == right.ino())
    }

    pub fn named_leaf_matches(
        directory: &File,
        name: &CStr,
        metadata: &fs::Metadata,
    ) -> io::Result<bool> {
        let linked = stat_at(directory.as_raw_fd(), name)?;
        Ok(same_open_file_stat(metadata, &linked))
    }

    pub fn leaf_is_missing(directory: &File, name: &CStr) -> io::Result<bool> {
        match stat_at(directory.as_raw_fd(), name) {
            Ok(_) => Ok(false),
            Err(error) if error.raw_os_error() == Some(libc::ENOENT) => Ok(true),
            Err(error) => Err(error),
        }
    }

    fn open_child_directory(directory: &File, name: &CStr) -> io::Result<File> {
        // SAFETY: the held directory descriptor and NUL-terminated name remain live for the call.
        let descriptor = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if descriptor < 0 {
            Err(classify_nofollow_error(
                directory,
                name,
                io::Error::last_os_error(),
            ))
        } else {
            // SAFETY: openat returned a new owned descriptor.
            Ok(unsafe { File::from_raw_fd(descriptor) })
        }
    }

    fn classify_nofollow_error(directory: &File, name: &CStr, error: io::Error) -> io::Error {
        if matches!(
            error.raw_os_error(),
            Some(libc::ELOOP) | Some(libc::ENOTDIR)
        ) && stat_at(directory.as_raw_fd(), name)
            .ok()
            .is_some_and(|stat| stat.st_mode & libc::S_IFMT == libc::S_IFLNK)
        {
            return io::Error::from_raw_os_error(libc::ELOOP);
        }
        error
    }

    pub fn open_directory(path: &Path) -> io::Result<File> {
        let path = CString::new(path.as_os_str().as_bytes())
            .map_err(|_| invalid_path("directory path contains NUL"))?;
        // SAFETY: the path is a live NUL-terminated string. A successful descriptor is immediately
        // transferred to File. O_NOFOLLOW rejects a raced leaf symlink.
        let descriptor = unsafe {
            libc::open(
                path.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if descriptor < 0 {
            Err(io::Error::last_os_error())
        } else {
            // SAFETY: open returned a new owned descriptor.
            Ok(unsafe { File::from_raw_fd(descriptor) })
        }
    }

    pub fn directory_matches_path(directory: &File, path: &Path) -> io::Result<bool> {
        let opened = directory.metadata()?;
        let named = fs::symlink_metadata(path)?;
        Ok(named.is_dir()
            && !named.file_type().is_symlink()
            && opened.dev() == named.dev()
            && opened.ino() == named.ino())
    }

    pub fn leaf_name(parent: &Path, path: &Path) -> FileChangeResultValue<CString> {
        leaf_name_io(parent, path).map_err(|error| {
            FileChangeError::with_diagnostic(
                FileChangeErrorCode::IllegalFieldCombination,
                error.to_string(),
            )
        })
    }

    pub fn leaf_name_io(parent: &Path, path: &Path) -> io::Result<CString> {
        if path.parent() != Some(parent) {
            return Err(invalid_path("path is outside the bound parent"));
        }
        let name = path
            .file_name()
            .ok_or_else(|| invalid_path("path has no file name"))?;
        CString::new(name.as_bytes()).map_err(|_| invalid_path("file name contains NUL"))
    }

    pub fn read_optional(
        directory: &File,
        name: &CStr,
        max_bytes: Option<u64>,
    ) -> FileChangeResultValue<Option<BoundCurrentFile>> {
        // O_NONBLOCK ensures a raced FIFO or device cannot block before its type is checked.
        // SAFETY: the directory descriptor and name remain live for the call.
        let descriptor = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
            )
        };
        if descriptor < 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ENOENT) {
                return Ok(None);
            }
            return Err(open_error(error));
        }
        // SAFETY: openat returned a new owned descriptor.
        let mut file = unsafe { File::from_raw_fd(descriptor) };
        let before = file.metadata().map_err(read_error)?;
        validate_open_metadata(&before)?;
        if max_bytes.is_some_and(|limit| before.len() > limit) {
            return Err(FileChangeError::new(FileChangeErrorCode::ContentTooLarge));
        }
        let mut bytes = Vec::new();
        match max_bytes {
            Some(limit) => {
                Read::by_ref(&mut file)
                    .take(limit.saturating_add(1))
                    .read_to_end(&mut bytes)
                    .map_err(read_error)?;
                if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > limit {
                    return Err(FileChangeError::new(FileChangeErrorCode::ContentTooLarge));
                }
            }
            None => {
                file.read_to_end(&mut bytes).map_err(read_error)?;
            }
        }
        let after = file.metadata().map_err(read_error)?;
        // A second hard link may be introduced while this descriptor is being read. Validate the
        // post-read link count as well as the initially opened descriptor before freezing bytes.
        validate_open_metadata(&after)?;
        let linked = stat_at(directory.as_raw_fd(), name).map_err(read_error)?;
        if !same_open_file(&before, &after)
            || !same_open_file_stat(&after, &linked)
            || after.len() != bytes.len() as u64
        {
            return Err(FileChangeError::new(FileChangeErrorCode::Conflict));
        }
        Ok(Some(BoundCurrentFile {
            bytes,
            permissions: after.permissions(),
            metadata: after,
        }))
    }

    pub fn stage(
        directory: &File,
        content: &str,
        permissions: Option<Permissions>,
    ) -> FileChangeResultValue<BoundStagedFile> {
        for _ in 0..16 {
            let name = CString::new(format!(
                ".file-change-stage-{}.tmp",
                uuid::Uuid::new_v4().simple()
            ))
            .expect("generated staging names contain no NUL");
            // SAFETY: descriptor/name are live. O_EXCL and O_NOFOLLOW create a private regular
            // staging inode in the already-bound parent directory.
            let descriptor = unsafe {
                libc::openat(
                    directory.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_RDWR
                        | libc::O_CREAT
                        | libc::O_EXCL
                        | libc::O_NOFOLLOW
                        | libc::O_CLOEXEC,
                    0o600 as libc::c_uint,
                )
            };
            if descriptor < 0 {
                let error = io::Error::last_os_error();
                if error.raw_os_error() == Some(libc::EEXIST) {
                    continue;
                }
                return Err(staging_error(error));
            }
            // SAFETY: openat returned a new owned descriptor.
            let mut file = unsafe { File::from_raw_fd(descriptor) };
            if let Err(error) = file
                .write_all(content.as_bytes())
                .and_then(|()| file.flush())
                .and_then(|()| file.sync_all())
            {
                let _ = remove(directory, &name);
                return Err(staging_error(error));
            }
            if let Some(permissions) = permissions {
                if let Err(error) = file
                    .set_permissions(fs::Permissions::from_mode(permissions.mode()))
                    .and_then(|()| file.sync_all())
                {
                    let _ = remove(directory, &name);
                    return Err(staging_error(error));
                }
            }
            let directory = match directory.try_clone() {
                Ok(directory) => directory,
                Err(error) => {
                    drop(file);
                    let _ = remove(directory, &name);
                    return Err(staging_error(error));
                }
            };
            return Ok(BoundStagedFile {
                directory,
                name,
                file,
                cleanup_name: true,
            });
        }
        Err(FileChangeError::new(FileChangeErrorCode::Conflict))
    }

    pub fn publish_noreplace(
        directory: &File,
        mut staged: BoundStagedFile,
        target: &CStr,
    ) -> io::Result<()> {
        rename_noreplace_raw(directory.as_raw_fd(), &staged.name, target)?;
        staged.cleanup_name = false;
        Ok(())
    }

    pub fn publish_exchange(
        directory: &File,
        mut staged: BoundStagedFile,
        target: &CStr,
    ) -> io::Result<BoundUpdateExchange> {
        exchange_raw(directory.as_raw_fd(), &staged.name, target)?;
        // The old target is now reachable through `staged.name`. Its identity is not trusted yet,
        // so Drop must preserve it until the committer validates or rolls back the exchange.
        staged.cleanup_name = false;
        Ok(BoundUpdateExchange { staged })
    }

    pub fn rollback_exchange(
        directory: &File,
        exchanged: &mut BoundUpdateExchange,
        target: &CStr,
    ) -> io::Result<()> {
        exchange_raw(directory.as_raw_fd(), &exchanged.staged.name, target)
    }

    pub fn exchanged_is_staged_target(
        exchanged: &BoundUpdateExchange,
        current: &BoundCurrentFile,
    ) -> io::Result<bool> {
        let staged = exchanged.staged.file.metadata()?;
        Ok(same_file_identity(&staged, &current.metadata))
    }

    pub fn rename_noreplace(directory: &File, source: &CStr, target: &CStr) -> io::Result<()> {
        rename_noreplace_raw(directory.as_raw_fd(), source, target)
    }

    pub fn remove(directory: &File, name: &CStr) -> io::Result<()> {
        // SAFETY: the descriptor and name remain live for the call. No symlink is followed by
        // unlinkat; directories are rejected because AT_REMOVEDIR is intentionally absent.
        let result = unsafe { libc::unlinkat(directory.as_raw_fd(), name.as_ptr(), 0) };
        if result == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    #[cfg(test)]
    thread_local! {
        static FAIL_EXCHANGE_CALL: Cell<usize> = const { Cell::new(0) };
    }

    #[cfg(test)]
    pub fn fail_exchange_call(call: usize) {
        assert!(call > 0, "exchange failpoint call is one-based");
        FAIL_EXCHANGE_CALL.with(|fail| fail.set(call));
    }

    #[cfg(test)]
    fn take_exchange_failpoint() -> bool {
        FAIL_EXCHANGE_CALL.with(|fail| match fail.get() {
            0 => false,
            1 => {
                fail.set(0);
                true
            }
            remaining => {
                fail.set(remaining - 1);
                false
            }
        })
    }

    #[cfg(target_vendor = "apple")]
    fn exchange_raw(parent: RawFd, source: &CStr, target: &CStr) -> io::Result<()> {
        #[cfg(test)]
        if take_exchange_failpoint() {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "injected atomic exchange failure",
            ));
        }
        // SAFETY: both names and the bound directory descriptor remain live for the call.
        let result = unsafe {
            libc::renameatx_np(
                parent,
                source.as_ptr(),
                parent,
                target.as_ptr(),
                libc::RENAME_SWAP,
            )
        };
        if result == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    fn exchange_raw(parent: RawFd, source: &CStr, target: &CStr) -> io::Result<()> {
        #[cfg(test)]
        if take_exchange_failpoint() {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "injected atomic exchange failure",
            ));
        }
        // SAFETY: both names and the bound directory descriptor remain live for the syscall.
        let result = unsafe {
            libc::syscall(
                libc::SYS_renameat2,
                parent,
                source.as_ptr(),
                parent,
                target.as_ptr(),
                libc::RENAME_EXCHANGE,
            )
        };
        if result == 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if matches!(
            error.raw_os_error(),
            Some(libc::ENOSYS) | Some(libc::EINVAL) | Some(libc::EOPNOTSUPP)
        ) {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "filesystem does not support atomic exchange rename",
            ));
        }
        Err(error)
    }

    #[cfg(all(
        unix,
        not(target_vendor = "apple"),
        not(any(target_os = "linux", target_os = "android"))
    ))]
    fn exchange_raw(_parent: RawFd, _source: &CStr, _target: &CStr) -> io::Result<()> {
        #[cfg(test)]
        let _ = take_exchange_failpoint();
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "platform does not support descriptor-relative atomic exchange",
        ))
    }

    #[cfg(target_vendor = "apple")]
    fn rename_noreplace_raw(parent: RawFd, source: &CStr, target: &CStr) -> io::Result<()> {
        // SAFETY: both names and the parent descriptor remain live for the call.
        let result = unsafe {
            libc::renameatx_np(
                parent,
                source.as_ptr(),
                parent,
                target.as_ptr(),
                libc::RENAME_EXCL,
            )
        };
        if result == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    fn rename_noreplace_raw(parent: RawFd, source: &CStr, target: &CStr) -> io::Result<()> {
        // SAFETY: both names and the parent descriptor remain live for the syscall.
        let result = unsafe {
            libc::syscall(
                libc::SYS_renameat2,
                parent,
                source.as_ptr(),
                parent,
                target.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        };
        if result == 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if matches!(
            error.raw_os_error(),
            Some(libc::ENOSYS) | Some(libc::EINVAL)
        ) {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "filesystem does not support atomic no-replace rename",
            ));
        }
        Err(error)
    }

    #[cfg(all(
        unix,
        not(target_vendor = "apple"),
        not(any(target_os = "linux", target_os = "android"))
    ))]
    fn rename_noreplace_raw(_parent: RawFd, _source: &CStr, _target: &CStr) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "platform does not support atomic no-replace rename",
        ))
    }

    fn stat_at(parent: RawFd, name: &CStr) -> io::Result<libc::stat> {
        let mut stat = MaybeUninit::<libc::stat>::uninit();
        // SAFETY: stat points to writable storage and the descriptor/name remain live. On success
        // fstatat has initialized the entire stat structure.
        let result = unsafe {
            libc::fstatat(
                parent,
                name.as_ptr(),
                stat.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        if result == 0 {
            // SAFETY: fstatat succeeded and initialized stat.
            Ok(unsafe { stat.assume_init() })
        } else {
            Err(io::Error::last_os_error())
        }
    }

    pub(super) fn validate_open_metadata(metadata: &fs::Metadata) -> FileChangeResultValue<()> {
        if !metadata.is_file() {
            return Err(FileChangeError::new(FileChangeErrorCode::NotRegularFile));
        }
        if metadata.nlink() != 1 {
            return Err(FileChangeError::new(FileChangeErrorCode::HardLinkForbidden));
        }
        Ok(())
    }

    pub(super) fn same_open_file(left: &fs::Metadata, right: &fs::Metadata) -> bool {
        left.dev() == right.dev()
            && left.ino() == right.ino()
            && left.nlink() == right.nlink()
            && left.len() == right.len()
            && left.mtime() == right.mtime()
            && left.mtime_nsec() == right.mtime_nsec()
    }

    pub(super) fn same_file_identity(left: &fs::Metadata, right: &fs::Metadata) -> bool {
        left.dev() == right.dev() && left.ino() == right.ino()
    }

    fn same_open_file_stat(metadata: &fs::Metadata, stat: &libc::stat) -> bool {
        u128::from(metadata.dev()) == stat.st_dev as u128
            && metadata.ino() == stat.st_ino
            && i128::from(metadata.len()) == i128::from(stat.st_size)
            && u128::from(metadata.nlink()) == stat.st_nlink as u128
    }

    fn open_error(error: io::Error) -> FileChangeError {
        let code = match error.raw_os_error() {
            Some(libc::ELOOP) => FileChangeErrorCode::SymlinkForbidden,
            Some(libc::EACCES) | Some(libc::EPERM) => FileChangeErrorCode::PermissionDenied,
            _ => match error.kind() {
                io::ErrorKind::NotFound => FileChangeErrorCode::FileMissing,
                io::ErrorKind::PermissionDenied => FileChangeErrorCode::PermissionDenied,
                _ => FileChangeErrorCode::Failed,
            },
        };
        FileChangeError::with_diagnostic(code, error.to_string())
    }

    fn read_error(error: io::Error) -> FileChangeError {
        FileChangeError::with_diagnostic(FileChangeErrorCode::Failed, error.to_string())
    }

    fn staging_error(error: io::Error) -> FileChangeError {
        let code = if error.kind() == io::ErrorKind::PermissionDenied {
            FileChangeErrorCode::PermissionDenied
        } else {
            FileChangeErrorCode::Failed
        };
        FileChangeError::with_diagnostic(code, error.to_string())
    }

    fn invalid_path(message: &'static str) -> io::Error {
        io::Error::new(io::ErrorKind::InvalidInput, message)
    }
}

#[cfg(not(unix))]
mod fallback {
    use super::super::error::{FileChangeError, FileChangeErrorCode, FileChangeResultValue};
    use super::{BoundCurrentFile, BoundStagedFile};
    use std::fs::{self, OpenOptions, Permissions};
    use std::io::{self, Read, Write};
    use std::path::Path;
    use tempfile::NamedTempFile;

    pub fn read_optional(
        parent: &Path,
        path: &Path,
        max_bytes: Option<u64>,
    ) -> FileChangeResultValue<Option<BoundCurrentFile>> {
        if path.parent() != Some(parent) {
            return Err(FileChangeError::new(
                FileChangeErrorCode::IllegalFieldCombination,
            ));
        }
        match fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(FileChangeError::new(FileChangeErrorCode::SymlinkForbidden));
            }
            Ok(metadata) if !metadata.is_file() => {
                return Err(FileChangeError::new(FileChangeErrorCode::NotRegularFile));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(io_error(error)),
        }
        let mut file = match OpenOptions::new().read(true).open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(io_error(error)),
        };
        let before = file.metadata().map_err(io_error)?;
        if !before.is_file() {
            return Err(FileChangeError::new(FileChangeErrorCode::NotRegularFile));
        }
        if max_bytes.is_some_and(|limit| before.len() > limit) {
            return Err(FileChangeError::new(FileChangeErrorCode::ContentTooLarge));
        }
        let mut bytes = Vec::new();
        match max_bytes {
            Some(limit) => {
                Read::by_ref(&mut file)
                    .take(limit.saturating_add(1))
                    .read_to_end(&mut bytes)
                    .map_err(io_error)?;
                if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > limit {
                    return Err(FileChangeError::new(FileChangeErrorCode::ContentTooLarge));
                }
            }
            None => {
                file.read_to_end(&mut bytes).map_err(io_error)?;
            }
        }
        let after = file.metadata().map_err(io_error)?;
        if before.len() != after.len()
            || before.modified().ok() != after.modified().ok()
            || after.len() != bytes.len() as u64
        {
            return Err(FileChangeError::new(FileChangeErrorCode::Conflict));
        }
        Ok(Some(BoundCurrentFile {
            bytes,
            permissions: after.permissions(),
            metadata: after,
        }))
    }

    pub fn stage(
        parent: &Path,
        content: &str,
        permissions: Option<Permissions>,
    ) -> FileChangeResultValue<BoundStagedFile> {
        let mut staged = NamedTempFile::new_in(parent).map_err(io_error)?;
        staged
            .write_all(content.as_bytes())
            .and_then(|()| staged.flush())
            .and_then(|()| staged.as_file().sync_all())
            .map_err(io_error)?;
        if let Some(permissions) = permissions {
            staged
                .as_file()
                .set_permissions(permissions)
                .and_then(|()| staged.as_file().sync_all())
                .map_err(io_error)?;
        }
        Ok(BoundStagedFile {
            path: staged.into_temp_path(),
        })
    }

    fn io_error(error: io::Error) -> FileChangeError {
        let code = match error.kind() {
            io::ErrorKind::NotFound => FileChangeErrorCode::FileMissing,
            io::ErrorKind::PermissionDenied => FileChangeErrorCode::PermissionDenied,
            _ => FileChangeErrorCode::Failed,
        };
        FileChangeError::with_diagnostic(code, error.to_string())
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::file_change::FileChangePathPolicy;
    use std::fs;
    use std::os::unix::fs::symlink;
    use tempfile::TempDir;

    #[test]
    fn bound_publication_cannot_follow_a_swapped_parent_symlink() {
        let root = TempDir::new().expect("tempdir");
        let parent = root.path().join("parent");
        let displaced = root.path().join("displaced");
        let outside = root.path().join("outside");
        fs::create_dir(&parent).expect("parent");
        fs::create_dir(&outside).expect("outside");
        let target = FileChangePathPolicy::new(Some(root.path()), false)
            .resolve("parent/new.txt")
            .expect("resolve target");
        let bound = BoundParent::open(&target).expect("bind parent");
        let staged = bound.stage("safe\n", None).expect("stage content");

        fs::rename(&parent, &displaced).expect("displace parent");
        symlink(&outside, &parent).expect("redirect parent path");

        assert_eq!(
            bound.revalidate().expect_err("swap must fail").code(),
            FileChangeErrorCode::SymlinkForbidden
        );
        // Even if a caller mistakenly proceeds after the failed pathname revalidation, the held
        // dirfd contains publication in the original directory rather than following the symlink.
        bound
            .publish_noreplace(staged, target.absolute_path())
            .expect("descriptor-relative publication");
        bound.sync().expect("sync bound directory");
        assert_eq!(
            fs::read_to_string(displaced.join("new.txt")).unwrap(),
            "safe\n"
        );
        assert!(!outside.join("new.txt").exists());
    }

    #[test]
    fn bound_read_rejects_a_swapped_leaf_symlink() {
        let root = TempDir::new().expect("tempdir");
        let outside = root.path().join("outside.txt");
        let path = root.path().join("target.txt");
        fs::write(&outside, "outside\n").expect("outside");
        fs::write(&path, "base\n").expect("target");
        let target = FileChangePathPolicy::new(Some(root.path()), false)
            .resolve("target.txt")
            .expect("resolve target");
        let target_path = target.absolute_path().to_path_buf();
        let bound = BoundParent::open(&target).expect("bind parent");

        fs::remove_file(&target_path).expect("remove target");
        symlink(&outside, &target_path).expect("replace with symlink");
        let error = bound
            .read_optional(&target_path)
            .expect_err("leaf symlink must fail");
        assert_eq!(error.code(), FileChangeErrorCode::SymlinkForbidden);
        assert_eq!(fs::read_to_string(outside).unwrap(), "outside\n");
    }

    #[test]
    fn bounded_read_rejects_oversized_content_and_can_verify_absence_without_reading() {
        let root = TempDir::new().expect("tempdir");
        let target = FileChangePathPolicy::new(Some(root.path()), false)
            .resolve("target.txt")
            .expect("resolve target");
        let target_path = target.absolute_path().to_path_buf();
        let bound = BoundParent::open(&target).expect("bind parent");

        assert!(bound
            .read_optional_bounded(&target_path, 0)
            .expect("missing leaf can be verified with a zero-byte budget")
            .is_none());

        fs::write(&target_path, "too large\n").expect("target");
        assert_eq!(
            bound
                .read_optional_bounded(&target_path, 4)
                .expect_err("oversized content must fail before it is returned")
                .code(),
            FileChangeErrorCode::ContentTooLarge
        );
        assert_eq!(
            bound
                .read_optional_bounded(&target_path, 0)
                .expect_err("an existing leaf cannot be mistaken for absence")
                .code(),
            FileChangeErrorCode::ContentTooLarge
        );
    }
}
