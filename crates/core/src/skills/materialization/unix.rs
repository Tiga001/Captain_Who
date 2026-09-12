use super::*;
use std::ffi::{CStr, CString};
use std::fs::File;
use std::io::{self, Read, Write};
use std::mem::MaybeUninit;
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use uuid::Uuid;

mod syscalls;

use syscalls::{is_already_exists, is_unsupported, matches_errno, rename_noreplace};

const MAX_STAGING_NAME_ATTEMPTS: usize = 16;

#[cfg(test)]
mod test_hooks;
#[cfg(test)]
use test_hooks::invoke_materialization_test_hook;
#[cfg(test)]
pub(super) use test_hooks::{install_materialization_test_hook, MaterializationTestHookPoint};

pub(super) fn materialize_file(
    workspace_root: &Path,
    workspace_identity: Option<&crate::file_change::FileChangeDirectoryIdentity>,
    destination: &SkillMaterializationDestination,
    descriptor: &SkillResourceDescriptor,
    bytes: &[u8],
) -> Result<SkillMaterializationStatus, SkillMaterializationError> {
    let root = open_workspace_root(workspace_root, workspace_identity, destination)?;
    let components = destination.components().collect::<Vec<_>>();
    let (file_name, parents) = components
        .split_last()
        .expect("validated destination has at least one component");
    let target_name = CString::new(*file_name).expect("validated component contains no NUL");
    let opened = open_parent_chain(root, parents, destination)?;
    opened.verify(destination)?;

    match inspect_target(opened.parent(), &target_name, descriptor, destination)? {
        TargetState::Absent => {}
        TargetState::Identical => return Ok(SkillMaterializationStatus::AlreadyPresent),
        TargetState::Different(existing_digest) => {
            return Err(conflict(destination, descriptor, existing_digest))
        }
    }

    let mut staging = create_staging(opened.parent(), destination)?;
    staging
        .file_mut()
        .write_all(bytes)
        .map_err(|error| io_error("write", destination, error))?;
    staging
        .file()
        .sync_all()
        .map_err(|error| io_error("synchronize", destination, error))?;

    // Revalidate every parent directory after staging and immediately
    // before publication. The rename itself remains relative to the opened
    // parent handle and therefore never follows a replaced symlink.
    #[cfg(test)]
    invoke_materialization_test_hook(
        MaterializationTestHookPoint::FileBeforePublish,
        opened.parent().as_raw_fd(),
        staging.name(),
    );
    opened.verify(destination)?;
    staging.verify_link(destination)?;
    match rename_noreplace(opened.parent().as_raw_fd(), staging.name(), &target_name) {
        Ok(()) => {
            if let Err(reason) = staging.verify_published(opened.parent(), &target_name) {
                return Err(SkillMaterializationError::CommitIndeterminate {
                    destination: destination.clone(),
                    expected_digest: descriptor.content_digest().to_string(),
                    reason,
                });
            }
            staging.disarm();
        }
        Err(error) if is_already_exists(&error) => {
            return match inspect_target(opened.parent(), &target_name, descriptor, destination)? {
                TargetState::Identical => Ok(SkillMaterializationStatus::AlreadyPresent),
                TargetState::Different(existing_digest) => {
                    Err(conflict(destination, descriptor, existing_digest))
                }
                TargetState::Absent => Err(SkillMaterializationError::Conflict {
                    destination: destination.clone(),
                    expected_digest: descriptor.content_digest().to_string(),
                    existing_digest: None,
                }),
            };
        }
        Err(error) if is_unsupported(&error) => {
            return Err(SkillMaterializationError::UnsupportedPlatform)
        }
        Err(error) => return Err(io_error("publish", destination, error)),
    }

    if let Err(error) = opened.parent().sync_all() {
        return Err(SkillMaterializationError::CommitIndeterminate {
            destination: destination.clone(),
            expected_digest: descriptor.content_digest().to_string(),
            reason: error.to_string(),
        });
    }
    if let Err(error) = opened.verify(destination) {
        return Err(SkillMaterializationError::CommitIndeterminate {
            destination: destination.clone(),
            expected_digest: descriptor.content_digest().to_string(),
            reason: error.to_string(),
        });
    }
    Ok(SkillMaterializationStatus::Created)
}

pub(super) fn materialize_tree(
    workspace_root: &Path,
    workspace_identity: Option<&crate::file_change::FileChangeDirectoryIdentity>,
    destination: &SkillMaterializationDestination,
    prepared: &PreparedTemplateTree,
) -> Result<SkillMaterializationStatus, SkillMaterializationError> {
    let root = open_workspace_root(workspace_root, workspace_identity, destination)?;
    let components = destination.components().collect::<Vec<_>>();
    let (directory_name, parents) = components
        .split_last()
        .expect("validated destination has at least one component");
    let target_name = CString::new(*directory_name).expect("validated component contains no NUL");
    let opened = open_parent_chain(root, parents, destination)?;
    opened.verify(destination)?;

    match inspect_tree_target(
        opened.parent(),
        &target_name,
        &prepared.fingerprint,
        destination,
    )? {
        TreeTargetState::Absent => {}
        TreeTargetState::Identical => return Ok(SkillMaterializationStatus::AlreadyPresent),
        TreeTargetState::Different(existing_digest) => {
            return Err(tree_conflict(
                destination,
                &prepared.plan_digest,
                existing_digest,
            ))
        }
    }

    let mut staging = create_staging_directory(opened.parent(), destination)?;
    populate_staging_tree(staging.directory(), prepared, destination)?;
    if !tree_matches(staging.directory(), &prepared.fingerprint, destination)? {
        return Err(SkillMaterializationError::UnsafeDestination {
            destination: destination.clone(),
            reason: "staging tree changed before publication".to_string(),
        });
    }

    // Publication remains relative to the already-opened destination
    // parent. Revalidating the logical chain prevents a renamed/replaced
    // ancestor from silently changing the user's visible destination.
    #[cfg(test)]
    invoke_materialization_test_hook(
        MaterializationTestHookPoint::TreeBeforePublish,
        opened.parent().as_raw_fd(),
        staging.name(),
    );
    opened.verify(destination)?;
    staging.verify_link(destination)?;
    match rename_noreplace(opened.parent().as_raw_fd(), staging.name(), &target_name) {
        Ok(()) => {
            if let Err(reason) = staging.verify_published(opened.parent(), &target_name) {
                return Err(SkillMaterializationError::CommitIndeterminate {
                    destination: destination.clone(),
                    expected_digest: prepared.plan_digest.clone(),
                    reason,
                });
            }
            staging.disarm();
        }
        Err(error) if is_already_exists(&error) => {
            return match inspect_tree_target(
                opened.parent(),
                &target_name,
                &prepared.fingerprint,
                destination,
            )? {
                TreeTargetState::Identical => Ok(SkillMaterializationStatus::AlreadyPresent),
                TreeTargetState::Different(existing_digest) => Err(tree_conflict(
                    destination,
                    &prepared.plan_digest,
                    existing_digest,
                )),
                TreeTargetState::Absent => {
                    Err(tree_conflict(destination, &prepared.plan_digest, None))
                }
            };
        }
        Err(error) if is_unsupported(&error) => {
            return Err(SkillMaterializationError::UnsupportedPlatform)
        }
        Err(error) => return Err(io_error("publish", destination, error)),
    }

    if let Err(error) = opened.parent().sync_all() {
        return Err(SkillMaterializationError::CommitIndeterminate {
            destination: destination.clone(),
            expected_digest: prepared.plan_digest.clone(),
            reason: error.to_string(),
        });
    }
    if let Err(error) = opened.verify(destination) {
        return Err(SkillMaterializationError::CommitIndeterminate {
            destination: destination.clone(),
            expected_digest: prepared.plan_digest.clone(),
            reason: error.to_string(),
        });
    }
    Ok(SkillMaterializationStatus::Created)
}

enum TreeTargetState {
    Absent,
    Identical,
    Different(Option<String>),
}

fn inspect_tree_target(
    parent: &File,
    name: &CStr,
    expected: &TreeFingerprint,
    destination: &SkillMaterializationDestination,
) -> Result<TreeTargetState, SkillMaterializationError> {
    let linked = match stat_at(parent.as_raw_fd(), name, libc::AT_SYMLINK_NOFOLLOW) {
        Ok(linked) => linked,
        Err(error) if error.raw_os_error() == Some(libc::ENOENT) => {
            return Ok(TreeTargetState::Absent)
        }
        Err(error) => return Err(io_error("inspect", destination, error)),
    };
    if is_symlink(&linked) {
        return Err(SkillMaterializationError::UnsafeDestination {
            destination: destination.clone(),
            reason: "destination is a symlink".to_string(),
        });
    }
    if !is_directory(&linked) {
        return Ok(TreeTargetState::Different(None));
    }

    let directory = open_directory_at(parent.as_raw_fd(), name, &linked, destination)?;
    // Two complete passes make an idempotent hit depend on a stable full
    // tree observation, rather than a single possibly-racing walk.
    let first = tree_matches(&directory, expected, destination)?;
    let second = first && tree_matches(&directory, expected, destination)?;
    let after = file_stat(&directory).map_err(|error| io_error("reinspect", destination, error))?;
    let relinked =
        stat_at(parent.as_raw_fd(), name, libc::AT_SYMLINK_NOFOLLOW).map_err(|error| {
            SkillMaterializationError::UnsafeDestination {
                destination: destination.clone(),
                reason: format!("destination changed while it was inspected: {error}"),
            }
        })?;
    if !same_inode(&linked, &after)
        || !same_inode(&linked, &relinked)
        || !is_directory(&after)
        || !is_directory(&relinked)
    {
        return Err(SkillMaterializationError::UnsafeDestination {
            destination: destination.clone(),
            reason: "destination changed while it was inspected".to_string(),
        });
    }
    if first && second {
        Ok(TreeTargetState::Identical)
    } else {
        Ok(TreeTargetState::Different(None))
    }
}

fn tree_matches(
    root: &File,
    expected: &TreeFingerprint,
    destination: &SkillMaterializationDestination,
) -> Result<bool, SkillMaterializationError> {
    let mut visited_directories = BTreeSet::new();
    let mut visited_files = BTreeSet::new();
    let mut remaining_entries = expected
        .directories
        .len()
        .saturating_add(expected.files.len())
        .saturating_add(1);
    if !directory_matches(
        root,
        "",
        expected,
        &mut visited_directories,
        &mut visited_files,
        &mut remaining_entries,
        destination,
    )? {
        return Ok(false);
    }
    Ok(visited_directories == expected.directories
        && visited_files.len() == expected.files.len()
        && visited_files
            .iter()
            .all(|path| expected.files.contains_key(path)))
}

fn directory_matches(
    directory: &File,
    relative_directory: &str,
    expected: &TreeFingerprint,
    visited_directories: &mut BTreeSet<String>,
    visited_files: &mut BTreeSet<String>,
    remaining_entries: &mut usize,
    destination: &SkillMaterializationDestination,
) -> Result<bool, SkillMaterializationError> {
    let Some(names) = read_directory_names(directory, *remaining_entries)
        .map_err(|error| io_error("enumerate existing", destination, error))?
    else {
        return Ok(false);
    };
    for name in names {
        if *remaining_entries == 0 {
            return Ok(false);
        }
        *remaining_entries -= 1;
        let name_text = match std::str::from_utf8(name.to_bytes()) {
            Ok(value) => value,
            Err(_) => return Ok(false),
        };
        let path = if relative_directory.is_empty() {
            name_text.to_string()
        } else {
            format!("{relative_directory}/{name_text}")
        };
        let linked =
            stat_at(directory.as_raw_fd(), &name, libc::AT_SYMLINK_NOFOLLOW).map_err(|error| {
                SkillMaterializationError::UnsafeDestination {
                    destination: destination.clone(),
                    reason: format!("destination changed while it was inspected: {error}"),
                }
            })?;
        if is_symlink(&linked) {
            return Err(SkillMaterializationError::UnsafeDestination {
                destination: destination.clone(),
                reason: format!("destination contains a symlink at `{path}`"),
            });
        }
        if is_directory(&linked) {
            if !expected.directories.contains(&path) || !visited_directories.insert(path.clone()) {
                return Ok(false);
            }
            let child = open_directory_at(directory.as_raw_fd(), &name, &linked, destination)?;
            if !directory_matches(
                &child,
                &path,
                expected,
                visited_directories,
                visited_files,
                remaining_entries,
                destination,
            )? {
                return Ok(false);
            }
        } else if is_regular_file(&linked) {
            let Some(expected_file) = expected.files.get(&path) else {
                return Ok(false);
            };
            if !visited_files.insert(path.clone())
                || !file_matches_at(directory, &name, &linked, expected_file, &path, destination)?
            {
                return Ok(false);
            }
        } else {
            return Ok(false);
        }
    }
    Ok(true)
}

fn file_matches_at(
    parent: &File,
    name: &CStr,
    linked: &libc::stat,
    expected: &TreeFileFingerprint,
    relative_path: &str,
    destination: &SkillMaterializationDestination,
) -> Result<bool, SkillMaterializationError> {
    // SAFETY: the parent fd and name are live. `O_NONBLOCK` prevents a
    // raced special file from blocking the execution thread.
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        let error = io::Error::last_os_error();
        return Err(SkillMaterializationError::UnsafeDestination {
            destination: destination.clone(),
            reason: format!(
                "cannot safely inspect `{relative_path}` because the destination changed: {error}"
            ),
        });
    }
    // SAFETY: `fd` is newly owned by this function.
    let mut file = unsafe { File::from_raw_fd(fd) };
    let before = file_stat(&file).map_err(|error| io_error("inspect", destination, error))?;
    if !same_inode(linked, &before)
        || !is_regular_file(&before)
        || before.st_size < 0
        || u64::try_from(before.st_size).unwrap_or(u64::MAX) != expected.byte_length
    {
        return Ok(false);
    }
    let mut bytes = Vec::with_capacity(usize::try_from(before.st_size).unwrap_or(0));
    (&mut file)
        .take(expected.byte_length.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| io_error("read existing", destination, error))?;
    let after = file_stat(&file).map_err(|error| io_error("reinspect", destination, error))?;
    let relinked =
        stat_at(parent.as_raw_fd(), name, libc::AT_SYMLINK_NOFOLLOW).map_err(|error| {
            SkillMaterializationError::UnsafeDestination {
                destination: destination.clone(),
                reason: format!("destination changed while `{relative_path}` was read: {error}"),
            }
        })?;
    if !same_inode(&before, &after)
        || !same_inode(&before, &relinked)
        || before.st_size != after.st_size
        || !is_regular_file(&relinked)
    {
        return Err(SkillMaterializationError::UnsafeDestination {
            destination: destination.clone(),
            reason: format!("destination changed while `{relative_path}` was read"),
        });
    }
    Ok(
        u64::try_from(bytes.len()).unwrap_or(u64::MAX) == expected.byte_length
            && package_file_digest(&bytes) == expected.content_digest,
    )
}

fn open_directory_at(
    parent_fd: RawFd,
    name: &CStr,
    linked: &libc::stat,
    destination: &SkillMaterializationDestination,
) -> Result<File, SkillMaterializationError> {
    // SAFETY: the parent fd and C string are live. The successful fd is
    // immediately transferred to `File`.
    let fd = unsafe {
        libc::openat(
            parent_fd,
            name.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        let error = io::Error::last_os_error();
        return Err(SkillMaterializationError::UnsafeDestination {
            destination: destination.clone(),
            reason: format!("cannot safely open destination directory: {error}"),
        });
    }
    // SAFETY: `fd` is newly owned by this function.
    let directory = unsafe { File::from_raw_fd(fd) };
    let opened = file_stat(&directory).map_err(|error| io_error("inspect", destination, error))?;
    if !is_directory(&opened) || !same_inode(linked, &opened) {
        return Err(SkillMaterializationError::UnsafeDestination {
            destination: destination.clone(),
            reason: "destination directory changed while it was opened".to_string(),
        });
    }
    Ok(directory)
}

/// Reads at most `limit` names. `None` means the directory contains more
/// entries than the caller is willing to inspect; this prevents an
/// attacker-controlled existing destination from forcing unbounded memory.
fn read_directory_names(directory: &File, limit: usize) -> io::Result<Option<Vec<CString>>> {
    // `dup` would share a directory offset with the caller's open file
    // description. Open `.` relative to the handle instead so every walk
    // receives an independent cursor and repeated verification is stable.
    const DOT: &[u8] = b".\0";
    // SAFETY: the source fd and static NUL-terminated name are live.
    let stream_fd = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            DOT.as_ptr().cast(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if stream_fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `stream_fd` is newly owned and is consumed by fdopendir.
    let stream = unsafe { libc::fdopendir(stream_fd) };
    if stream.is_null() {
        let error = io::Error::last_os_error();
        // SAFETY: fdopendir failed and therefore did not consume the fd.
        unsafe { libc::close(stream_fd) };
        return Err(error);
    }

    let mut names = Vec::new();
    loop {
        set_errno(0);
        // SAFETY: the stream remains live until closed below.
        let entry = unsafe { libc::readdir(stream) };
        if entry.is_null() {
            let error = get_errno();
            // SAFETY: stream is live and closed exactly once.
            unsafe { libc::closedir(stream) };
            if error == 0 {
                break;
            }
            return Err(io::Error::from_raw_os_error(error));
        }
        // SAFETY: readdir returned a live dirent with a NUL-terminated
        // d_name valid until the next call; copy it immediately.
        let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) };
        if name.to_bytes() != b"." && name.to_bytes() != b".." {
            if names.len() >= limit {
                // SAFETY: stream is live and closed exactly once.
                unsafe { libc::closedir(stream) };
                return Ok(None);
            }
            names.push(name.to_owned());
        }
    }
    names.sort_by(|left, right| left.to_bytes().cmp(right.to_bytes()));
    Ok(Some(names))
}

#[cfg(target_vendor = "apple")]
fn errno_location() -> *mut libc::c_int {
    // SAFETY: libc returns the current thread's errno pointer.
    unsafe { libc::__error() }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn errno_location() -> *mut libc::c_int {
    // SAFETY: libc returns the current thread's errno pointer.
    unsafe { libc::__errno_location() }
}

fn set_errno(value: libc::c_int) {
    // SAFETY: errno_location returns the current thread's writable errno.
    unsafe { *errno_location() = value }
}

fn get_errno() -> libc::c_int {
    // SAFETY: errno_location returns the current thread's readable errno.
    unsafe { *errno_location() }
}

struct TreeStagingGuard {
    parent_fd: RawFd,
    name: CString,
    directory: File,
    device: libc::dev_t,
    inode: libc::ino_t,
    armed: bool,
}

impl TreeStagingGuard {
    fn name(&self) -> &CStr {
        &self.name
    }

    fn directory(&self) -> &File {
        &self.directory
    }

    fn verify_link(
        &self,
        destination: &SkillMaterializationDestination,
    ) -> Result<(), SkillMaterializationError> {
        let opened = file_stat(&self.directory)
            .map_err(|error| io_error("inspect staging for", destination, error))?;
        let linked =
            stat_at(self.parent_fd, &self.name, libc::AT_SYMLINK_NOFOLLOW).map_err(|error| {
                SkillMaterializationError::UnsafeDestination {
                    destination: destination.clone(),
                    reason: format!("staging directory changed before publication: {error}"),
                }
            })?;
        if !self.matches(&opened) || !self.matches(&linked) {
            return Err(SkillMaterializationError::UnsafeDestination {
                destination: destination.clone(),
                reason: "staging directory was replaced before publication".to_string(),
            });
        }
        Ok(())
    }

    fn verify_published(&self, parent: &File, target: &CStr) -> Result<(), String> {
        let opened = file_stat(&self.directory)
            .map_err(|error| format!("cannot inspect published staging directory: {error}"))?;
        let linked = stat_at(parent.as_raw_fd(), target, libc::AT_SYMLINK_NOFOLLOW)
            .map_err(|error| format!("cannot inspect published destination: {error}"))?;
        if self.matches(&opened) && self.matches(&linked) {
            Ok(())
        } else {
            Err("published destination does not match the verified staging directory".to_string())
        }
    }

    fn matches(&self, stat: &libc::stat) -> bool {
        is_directory(stat) && stat.st_dev == self.device && stat.st_ino == self.inode
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for TreeStagingGuard {
    fn drop(&mut self) {
        if self.armed {
            remove_owned_staging_tree(
                self.parent_fd,
                &self.name,
                &self.directory,
                self.device,
                self.inode,
            );
        }
    }
}

fn create_staging_directory(
    parent: &File,
    destination: &SkillMaterializationDestination,
) -> Result<TreeStagingGuard, SkillMaterializationError> {
    for _ in 0..MAX_STAGING_NAME_ATTEMPTS {
        let name = CString::new(format!(
            "{STAGING_NAME_PREFIX}{}",
            Uuid::new_v4().hyphenated()
        ))
        .expect("generated staging name contains no NUL");
        // SAFETY: parent and name are live.
        if unsafe { libc::mkdirat(parent.as_raw_fd(), name.as_ptr(), 0o700) } != 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::EEXIST) {
                continue;
            }
            return Err(io_error("create staging directory for", destination, error));
        }

        // SAFETY: parent and name are live; fd is owned below on success.
        let fd = unsafe {
            libc::openat(
                parent.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            let error = io::Error::last_os_error();
            // We do not yet own an inode identity that can make path-based
            // cleanup safe. A hostile same-UID actor may have rebound the
            // random name, so prefer a harmless orphan over deleting an
            // object we cannot prove we created.
            return Err(io_error("open staging directory for", destination, error));
        }
        // SAFETY: fd is newly owned.
        let directory = unsafe { File::from_raw_fd(fd) };
        let opened = file_stat(&directory)
            .map_err(|error| io_error("inspect staging for", destination, error))?;
        if !is_directory(&opened) {
            return Err(SkillMaterializationError::UnsafeDestination {
                destination: destination.clone(),
                reason: "staging directory is not a directory".to_string(),
            });
        }
        let guard = TreeStagingGuard {
            parent_fd: parent.as_raw_fd(),
            name,
            directory,
            device: opened.st_dev,
            inode: opened.st_ino,
            armed: true,
        };
        if unsafe { libc::fchmod(guard.directory.as_raw_fd(), 0o700) } != 0 {
            let error = io::Error::last_os_error();
            return Err(io_error("set staging permissions for", destination, error));
        }
        guard.verify_link(destination)?;
        return Ok(guard);
    }
    Err(SkillMaterializationError::Io {
        operation: "allocate staging name for",
        destination: destination.clone(),
        reason: "staging namespace is unexpectedly saturated".to_string(),
    })
}

fn populate_staging_tree(
    root: &File,
    prepared: &PreparedTemplateTree,
    destination: &SkillMaterializationDestination,
) -> Result<(), SkillMaterializationError> {
    let mut directories = BTreeMap::<String, File>::new();
    for relative_path in &prepared.fingerprint.directories {
        let (parent_path, name_text) = split_parent(relative_path);
        let parent = relative_directory(root, &directories, parent_path);
        let name = CString::new(name_text).expect("validated component contains no NUL");
        // SAFETY: parent and name are live.
        if unsafe { libc::mkdirat(parent.as_raw_fd(), name.as_ptr(), 0o700) } != 0 {
            return Err(io_error(
                "create staging subdirectory for",
                destination,
                io::Error::last_os_error(),
            ));
        }
        let linked = stat_at(parent.as_raw_fd(), &name, libc::AT_SYMLINK_NOFOLLOW)
            .map_err(|error| io_error("inspect staging for", destination, error))?;
        let child = open_directory_at(parent.as_raw_fd(), &name, &linked, destination)?;
        // SAFETY: child fd is live.
        if unsafe { libc::fchmod(child.as_raw_fd(), 0o700) } != 0 {
            return Err(io_error(
                "set staging directory permissions for",
                destination,
                io::Error::last_os_error(),
            ));
        }
        directories.insert(relative_path.clone(), child);
    }

    for prepared_file in &prepared.files {
        let (parent_path, name_text) = split_parent(&prepared_file.relative_path);
        let parent = relative_directory(root, &directories, parent_path);
        let name = CString::new(name_text).expect("validated component contains no NUL");
        // SAFETY: parent and name are live. The successful fd is owned by
        // File immediately.
        let fd = unsafe {
            libc::openat(
                parent.as_raw_fd(),
                name.as_ptr(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                0o600,
            )
        };
        if fd < 0 {
            return Err(io_error(
                "create staging file for",
                destination,
                io::Error::last_os_error(),
            ));
        }
        // SAFETY: fd is newly owned.
        let mut file = unsafe { File::from_raw_fd(fd) };
        // SAFETY: file fd is live.
        if unsafe { libc::fchmod(file.as_raw_fd(), 0o600) } != 0 {
            return Err(io_error(
                "set staging file permissions for",
                destination,
                io::Error::last_os_error(),
            ));
        }
        file.write_all(&prepared_file.bytes)
            .map_err(|error| io_error("write staging file for", destination, error))?;
        file.sync_all()
            .map_err(|error| io_error("synchronize staging file for", destination, error))?;
        let opened = file_stat(&file)
            .map_err(|error| io_error("inspect staging file for", destination, error))?;
        let linked = stat_at(parent.as_raw_fd(), &name, libc::AT_SYMLINK_NOFOLLOW)
            .map_err(|error| io_error("inspect staging file for", destination, error))?;
        if !is_regular_file(&opened)
            || !is_regular_file(&linked)
            || !same_inode(&opened, &linked)
            || opened.st_size < 0
            || u64::try_from(opened.st_size).unwrap_or(u64::MAX)
                != prepared_file.descriptor.byte_length()
        {
            return Err(SkillMaterializationError::UnsafeDestination {
                destination: destination.clone(),
                reason: format!(
                    "staging file `{}` changed while it was written",
                    prepared_file.relative_path
                ),
            });
        }
    }

    for directory in directories.values().rev() {
        directory
            .sync_all()
            .map_err(|error| io_error("synchronize staging directory for", destination, error))?;
    }
    root.sync_all()
        .map_err(|error| io_error("synchronize staging tree for", destination, error))
}

fn split_parent(path: &str) -> (&str, &str) {
    path.rsplit_once('/').unwrap_or(("", path))
}

fn relative_directory<'a>(
    root: &'a File,
    directories: &'a BTreeMap<String, File>,
    path: &str,
) -> &'a File {
    if path.is_empty() {
        root
    } else {
        directories
            .get(path)
            .expect("prepared directory set contains every parent")
    }
}

fn remove_owned_staging_tree(
    parent_fd: RawFd,
    name: &CStr,
    directory: &File,
    device: libc::dev_t,
    inode: libc::ino_t,
) {
    let expected_matches =
        |stat: &libc::stat| is_directory(stat) && stat.st_dev == device && stat.st_ino == inode;
    let Ok(opened) = file_stat(directory) else {
        return;
    };
    let Ok(linked) = stat_at(parent_fd, name, libc::AT_SYMLINK_NOFOLLOW) else {
        return;
    };
    if !expected_matches(&opened) || !expected_matches(&linked) {
        return;
    }
    let mut budget = MAX_SKILL_MATERIALIZATION_TREE_FILES
        .saturating_add(MAX_SKILL_MATERIALIZATION_TREE_DIRECTORIES)
        .saturating_add(1);
    if clear_owned_directory(directory, &mut budget).is_err() {
        return;
    }
    let Ok(relinked) = stat_at(parent_fd, name, libc::AT_SYMLINK_NOFOLLOW) else {
        return;
    };
    if expected_matches(&relinked) {
        // SAFETY: the name still resolves to the exact staging inode we
        // created and just emptied. No broader path deletion is possible.
        unsafe { libc::unlinkat(parent_fd, name.as_ptr(), libc::AT_REMOVEDIR) };
    }
}

fn clear_owned_directory(directory: &File, budget: &mut usize) -> io::Result<()> {
    let names = read_directory_names(directory, *budget)?
        .ok_or_else(|| io::Error::other("staging cleanup budget exhausted"))?;
    for name in names {
        if *budget == 0 {
            return Err(io::Error::other("staging cleanup budget exhausted"));
        }
        *budget -= 1;
        let linked = stat_at(directory.as_raw_fd(), &name, libc::AT_SYMLINK_NOFOLLOW)?;
        if is_directory(&linked) {
            let child = open_directory_at_io(directory.as_raw_fd(), &name, &linked)?;
            clear_owned_directory(&child, budget)?;
            let relinked = stat_at(directory.as_raw_fd(), &name, libc::AT_SYMLINK_NOFOLLOW)?;
            if !same_inode(&linked, &relinked) || !is_directory(&relinked) {
                return Err(io::Error::other("staging directory changed during cleanup"));
            }
            // SAFETY: name still points to the exact child inode opened
            // and recursively emptied above.
            if unsafe { libc::unlinkat(directory.as_raw_fd(), name.as_ptr(), libc::AT_REMOVEDIR) }
                != 0
            {
                return Err(io::Error::last_os_error());
            }
        } else if unsafe { libc::unlinkat(directory.as_raw_fd(), name.as_ptr(), 0) } != 0 {
            // Non-directories, including symlinks, are unlinked as entries
            // and are never followed.
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

fn open_directory_at_io(parent_fd: RawFd, name: &CStr, linked: &libc::stat) -> io::Result<File> {
    // SAFETY: parent/name are live; fd is transferred to File on success.
    let fd = unsafe {
        libc::openat(
            parent_fd,
            name.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: fd is newly owned.
    let directory = unsafe { File::from_raw_fd(fd) };
    let opened = file_stat(&directory)?;
    if !is_directory(&opened) || !same_inode(linked, &opened) {
        return Err(io::Error::other(
            "staging directory changed while it was opened",
        ));
    }
    Ok(directory)
}

struct OpenedWorkspaceRoot {
    file: File,
    lexical_path: PathBuf,
    device: libc::dev_t,
    inode: libc::ino_t,
}

fn open_workspace_root(
    workspace_root: &Path,
    workspace_identity: Option<&crate::file_change::FileChangeDirectoryIdentity>,
    destination: &SkillMaterializationDestination,
) -> Result<OpenedWorkspaceRoot, SkillMaterializationError> {
    let root = CString::new(workspace_root.as_os_str().as_bytes()).map_err(|_| {
        SkillMaterializationError::InvalidWorkspace {
            reason: "workspace root contains a NUL byte".to_string(),
        }
    })?;
    // SAFETY: `root` is a live NUL-terminated C string. The returned fd is
    // owned immediately by `File` on success.
    let fd = unsafe {
        libc::open(
            root.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        let error = io::Error::last_os_error();
        return Err(if matches_errno(&error, &[libc::ELOOP, libc::ENOTDIR]) {
            SkillMaterializationError::InvalidWorkspace {
                reason: "workspace root is not a plain directory".to_string(),
            }
        } else {
            io_error("open workspace for", destination, error)
        });
    }
    // SAFETY: `fd` is newly owned by this function.
    let file = unsafe { File::from_raw_fd(fd) };
    if let Some(expected) = workspace_identity {
        let metadata =
            file.metadata()
                .map_err(|error| SkillMaterializationError::InvalidWorkspace {
                    reason: error.to_string(),
                })?;
        let opened =
            crate::file_change::FileChangeDirectoryIdentity::from_bound_metadata(&metadata)
                .map_err(|reason| SkillMaterializationError::InvalidWorkspace {
                    reason: reason.to_string(),
                })?;
        if opened != *expected {
            return Err(SkillMaterializationError::InvalidWorkspace {
                reason: "workspace folder identity changed".to_string(),
            });
        }
    }
    let identity =
        file_stat(&file).map_err(|error| SkillMaterializationError::InvalidWorkspace {
            reason: format!("workspace root cannot be inspected: {error}"),
        })?;
    if !is_directory(&identity) {
        return Err(SkillMaterializationError::InvalidWorkspace {
            reason: "workspace root is not a plain directory".to_string(),
        });
    }
    Ok(OpenedWorkspaceRoot {
        file,
        lexical_path: workspace_root.to_path_buf(),
        device: identity.st_dev,
        inode: identity.st_ino,
    })
}

struct OpenedParentChain {
    directories: Vec<File>,
    names: Vec<CString>,
    root_path: PathBuf,
    root_device: libc::dev_t,
    root_inode: libc::ino_t,
}

impl OpenedParentChain {
    fn parent(&self) -> &File {
        self.directories
            .last()
            .expect("opened chain contains the workspace root")
    }

    fn verify(
        &self,
        destination: &SkillMaterializationDestination,
    ) -> Result<(), SkillMaterializationError> {
        let root = self
            .directories
            .first()
            .expect("opened chain contains the workspace root");
        let opened_root = file_stat(root).map_err(|error| {
            unsafe_parent(
                destination,
                format!("cannot inspect opened workspace root: {error}"),
            )
        })?;
        let root_path = CString::new(self.root_path.as_os_str().as_bytes()).map_err(|_| {
            unsafe_parent(
                destination,
                "workspace root contains a NUL byte".to_string(),
            )
        })?;
        // SAFETY: the path is a live C string and the fd is immediately
        // transferred to File on success.
        let rebound_fd = unsafe {
            libc::open(
                root_path.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if rebound_fd < 0 {
            return Err(unsafe_parent(
                destination,
                format!(
                    "workspace root binding changed: {}",
                    io::Error::last_os_error()
                ),
            ));
        }
        // SAFETY: `rebound_fd` is newly owned.
        let rebound = unsafe { File::from_raw_fd(rebound_fd) };
        let rebound_root = file_stat(&rebound).map_err(|error| {
            unsafe_parent(
                destination,
                format!("cannot inspect rebound workspace root: {error}"),
            )
        })?;
        let expected_root = |stat: &libc::stat| {
            is_directory(stat) && stat.st_dev == self.root_device && stat.st_ino == self.root_inode
        };
        if !expected_root(&opened_root) || !expected_root(&rebound_root) {
            return Err(unsafe_parent(
                destination,
                "workspace root path was renamed or rebound during materialization".to_string(),
            ));
        }
        for (index, name) in self.names.iter().enumerate() {
            let expected = file_stat(&self.directories[index]).map_err(|error| {
                unsafe_parent(
                    destination,
                    format!("cannot inspect opened parent: {error}"),
                )
            })?;
            let child = file_stat(&self.directories[index + 1]).map_err(|error| {
                unsafe_parent(destination, format!("cannot inspect opened child: {error}"))
            })?;
            let linked = stat_at(
                self.directories[index].as_raw_fd(),
                name,
                libc::AT_SYMLINK_NOFOLLOW,
            )
            .map_err(|error| {
                unsafe_parent(destination, format!("parent chain changed: {error}"))
            })?;
            if !is_directory(&expected)
                || !is_directory(&child)
                || !is_directory(&linked)
                || child.st_dev != linked.st_dev
                || child.st_ino != linked.st_ino
            {
                return Err(unsafe_parent(
                    destination,
                    "parent chain changed or contains a link".to_string(),
                ));
            }
        }
        Ok(())
    }
}

fn open_parent_chain(
    root: OpenedWorkspaceRoot,
    parents: &[&str],
    destination: &SkillMaterializationDestination,
) -> Result<OpenedParentChain, SkillMaterializationError> {
    let OpenedWorkspaceRoot {
        file,
        lexical_path,
        device,
        inode,
    } = root;
    let mut directories = vec![file];
    let mut names = Vec::with_capacity(parents.len());
    for component in parents {
        let name = CString::new(*component).expect("validated component contains no NUL");
        let parent = directories.last().unwrap();
        // SAFETY: the parent fd and C string are live. A successful fd is
        // immediately transferred into `File`.
        let fd = unsafe {
            libc::openat(
                parent.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            let error = io::Error::last_os_error();
            return Err(if error.raw_os_error() == Some(libc::ENOENT) {
                SkillMaterializationError::DestinationParentNotFound {
                    destination: destination.clone(),
                }
            } else if matches_errno(&error, &[libc::ELOOP, libc::ENOTDIR]) {
                unsafe_parent(
                    destination,
                    "parent is a symlink or is not a directory".to_string(),
                )
            } else {
                io_error("open parent for", destination, error)
            });
        }
        names.push(name);
        // SAFETY: `fd` is newly owned by this function.
        directories.push(unsafe { File::from_raw_fd(fd) });
    }
    Ok(OpenedParentChain {
        directories,
        names,
        root_path: lexical_path,
        root_device: device,
        root_inode: inode,
    })
}

enum TargetState {
    Absent,
    Identical,
    Different(Option<String>),
}

fn inspect_target(
    parent: &File,
    name: &CStr,
    descriptor: &SkillResourceDescriptor,
    destination: &SkillMaterializationDestination,
) -> Result<TargetState, SkillMaterializationError> {
    // `O_NONBLOCK` prevents an attacker-controlled FIFO from blocking the
    // execution thread. `O_NOFOLLOW` ensures a final symlink is never read.
    // SAFETY: the parent fd and name are live for the call.
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        let error = io::Error::last_os_error();
        return if error.raw_os_error() == Some(libc::ENOENT) {
            Ok(TargetState::Absent)
        } else if matches_errno(&error, &[libc::ELOOP, libc::ENOTDIR]) {
            Err(SkillMaterializationError::UnsafeDestination {
                destination: destination.clone(),
                reason: "destination is a symlink or traverses a non-directory".to_string(),
            })
        } else {
            Err(io_error("inspect", destination, error))
        };
    }
    // SAFETY: `fd` is newly owned by this function.
    let mut file = unsafe { File::from_raw_fd(fd) };
    let before = file_stat(&file).map_err(|error| io_error("inspect", destination, error))?;
    if !is_regular_file(&before) {
        return Ok(TargetState::Different(None));
    }
    if before.st_size < 0
        || u64::try_from(before.st_size).unwrap_or(u64::MAX) != descriptor.byte_length()
    {
        return Ok(TargetState::Different(None));
    }

    let mut existing = Vec::with_capacity(usize::try_from(before.st_size).unwrap_or(0));
    (&mut file)
        .take(u64::try_from(MAX_SKILL_MATERIALIZATION_FILE_BYTES).unwrap_or(u64::MAX) + 1)
        .read_to_end(&mut existing)
        .map_err(|error| io_error("read existing", destination, error))?;
    let after = file_stat(&file).map_err(|error| io_error("reinspect", destination, error))?;
    let linked = stat_at(parent.as_raw_fd(), name, libc::AT_SYMLINK_NOFOLLOW).map_err(|error| {
        SkillMaterializationError::UnsafeDestination {
            destination: destination.clone(),
            reason: format!("destination changed while it was inspected: {error}"),
        }
    })?;
    if before.st_dev != after.st_dev
        || before.st_ino != after.st_ino
        || before.st_size != after.st_size
        || before.st_dev != linked.st_dev
        || before.st_ino != linked.st_ino
        || !is_regular_file(&linked)
    {
        return Err(SkillMaterializationError::UnsafeDestination {
            destination: destination.clone(),
            reason: "destination changed while it was inspected".to_string(),
        });
    }
    let digest = package_file_digest(&existing);
    if digest == descriptor.content_digest() {
        Ok(TargetState::Identical)
    } else {
        Ok(TargetState::Different(Some(digest)))
    }
}

struct StagingGuard {
    parent_fd: RawFd,
    name: CString,
    file: File,
    device: libc::dev_t,
    inode: libc::ino_t,
    armed: bool,
}

impl StagingGuard {
    fn name(&self) -> &CStr {
        &self.name
    }

    fn file(&self) -> &File {
        &self.file
    }

    fn file_mut(&mut self) -> &mut File {
        &mut self.file
    }

    fn matches(&self, stat: &libc::stat) -> bool {
        is_regular_file(stat) && stat.st_dev == self.device && stat.st_ino == self.inode
    }

    fn verify_link(
        &self,
        destination: &SkillMaterializationDestination,
    ) -> Result<(), SkillMaterializationError> {
        let opened = file_stat(&self.file)
            .map_err(|error| io_error("inspect staging for", destination, error))?;
        let linked =
            stat_at(self.parent_fd, &self.name, libc::AT_SYMLINK_NOFOLLOW).map_err(|error| {
                SkillMaterializationError::UnsafeDestination {
                    destination: destination.clone(),
                    reason: format!("staging file changed before publication: {error}"),
                }
            })?;
        if !self.matches(&opened) || !self.matches(&linked) {
            return Err(SkillMaterializationError::UnsafeDestination {
                destination: destination.clone(),
                reason: "staging file was replaced before publication".to_string(),
            });
        }
        Ok(())
    }

    fn verify_published(&self, parent: &File, target: &CStr) -> Result<(), String> {
        let opened = file_stat(&self.file)
            .map_err(|error| format!("cannot inspect published staging file: {error}"))?;
        let linked = stat_at(parent.as_raw_fd(), target, libc::AT_SYMLINK_NOFOLLOW)
            .map_err(|error| format!("cannot inspect published destination: {error}"))?;
        if self.matches(&opened) && self.matches(&linked) {
            Ok(())
        } else {
            Err("published destination does not match the verified staging file".to_string())
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for StagingGuard {
    fn drop(&mut self) {
        if self.armed {
            let opened = file_stat(&self.file);
            let linked = stat_at(self.parent_fd, &self.name, libc::AT_SYMLINK_NOFOLLOW);
            if opened.as_ref().is_ok_and(|stat| self.matches(stat))
                && linked.as_ref().is_ok_and(|stat| self.matches(stat))
            {
                // SAFETY: the name still resolves to the exact regular file
                // inode held by this guard. Cleanup never follows a link or
                // deletes a concurrent replacement.
                unsafe {
                    libc::unlinkat(self.parent_fd, self.name.as_ptr(), 0);
                }
            }
        }
    }
}

fn create_staging(
    parent: &File,
    destination: &SkillMaterializationDestination,
) -> Result<StagingGuard, SkillMaterializationError> {
    for _ in 0..MAX_STAGING_NAME_ATTEMPTS {
        let name = CString::new(format!(
            "{STAGING_NAME_PREFIX}{}",
            Uuid::new_v4().hyphenated()
        ))
        .expect("generated staging name contains no NUL");
        // SAFETY: the parent fd and C string are live. A successful fd is
        // immediately transferred into `File`.
        let fd = unsafe {
            libc::openat(
                parent.as_raw_fd(),
                name.as_ptr(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                0o600,
            )
        };
        if fd >= 0 {
            // SAFETY: `fd` is newly owned by this function.
            let file = unsafe { File::from_raw_fd(fd) };
            let opened = file_stat(&file)
                .map_err(|error| io_error("inspect staging for", destination, error))?;
            if !is_regular_file(&opened) {
                return Err(SkillMaterializationError::UnsafeDestination {
                    destination: destination.clone(),
                    reason: "staging file is not a regular file".to_string(),
                });
            }
            let guard = StagingGuard {
                parent_fd: parent.as_raw_fd(),
                name,
                file,
                device: opened.st_dev,
                inode: opened.st_ino,
                armed: true,
            };
            // Make the privacy contract independent of the process umask.
            // SAFETY: the guard owns a live file descriptor.
            if unsafe { libc::fchmod(guard.file.as_raw_fd(), 0o600) } != 0 {
                let error = io::Error::last_os_error();
                return Err(io_error("set staging permissions for", destination, error));
            }
            guard.verify_link(destination)?;
            return Ok(guard);
        }
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::EEXIST) {
            return Err(io_error("create staging file for", destination, error));
        }
    }
    Err(SkillMaterializationError::Io {
        operation: "allocate staging name for",
        destination: destination.clone(),
        reason: "staging namespace is unexpectedly saturated".to_string(),
    })
}

fn file_stat(file: &File) -> io::Result<libc::stat> {
    let mut stat = MaybeUninit::<libc::stat>::uninit();
    // SAFETY: `stat` points to writable memory and the file fd is live.
    if unsafe { libc::fstat(file.as_raw_fd(), stat.as_mut_ptr()) } == 0 {
        // SAFETY: successful fstat initialized the structure.
        Ok(unsafe { stat.assume_init() })
    } else {
        Err(io::Error::last_os_error())
    }
}

fn stat_at(parent_fd: RawFd, name: &CStr, flags: i32) -> io::Result<libc::stat> {
    let mut stat = MaybeUninit::<libc::stat>::uninit();
    // SAFETY: all pointers and fds are live for the call.
    if unsafe { libc::fstatat(parent_fd, name.as_ptr(), stat.as_mut_ptr(), flags) } == 0 {
        // SAFETY: successful fstatat initialized the structure.
        Ok(unsafe { stat.assume_init() })
    } else {
        Err(io::Error::last_os_error())
    }
}

fn is_directory(stat: &libc::stat) -> bool {
    stat.st_mode & libc::S_IFMT == libc::S_IFDIR
}

fn is_regular_file(stat: &libc::stat) -> bool {
    stat.st_mode & libc::S_IFMT == libc::S_IFREG
}

fn is_symlink(stat: &libc::stat) -> bool {
    stat.st_mode & libc::S_IFMT == libc::S_IFLNK
}

fn same_inode(left: &libc::stat, right: &libc::stat) -> bool {
    left.st_dev == right.st_dev && left.st_ino == right.st_ino
}

fn conflict(
    destination: &SkillMaterializationDestination,
    descriptor: &SkillResourceDescriptor,
    existing_digest: Option<String>,
) -> SkillMaterializationError {
    SkillMaterializationError::Conflict {
        destination: destination.clone(),
        expected_digest: descriptor.content_digest().to_string(),
        existing_digest,
    }
}

fn tree_conflict(
    destination: &SkillMaterializationDestination,
    expected_digest: &str,
    existing_digest: Option<String>,
) -> SkillMaterializationError {
    SkillMaterializationError::Conflict {
        destination: destination.clone(),
        expected_digest: expected_digest.to_string(),
        existing_digest,
    }
}

fn unsafe_parent(
    destination: &SkillMaterializationDestination,
    reason: String,
) -> SkillMaterializationError {
    SkillMaterializationError::UnsafeParent {
        destination: destination.clone(),
        reason,
    }
}

fn io_error(
    operation: &'static str,
    destination: &SkillMaterializationDestination,
    error: io::Error,
) -> SkillMaterializationError {
    SkillMaterializationError::Io {
        operation,
        destination: destination.clone(),
        reason: error.to_string(),
    }
}
