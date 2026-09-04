use super::*;

#[derive(Debug)]
pub(super) enum LoadedReviewContent {
    Ready {
        before_text: Option<String>,
        after_text: Option<String>,
    },
    Binary,
    TooLarge,
    Unsupported,
}

#[derive(Debug)]
pub(super) enum SideContent {
    Missing,
    Bytes(Vec<u8>),
    TooLarge,
    Unsupported,
}

#[derive(Debug)]
pub(super) enum RepositoryEntry {
    Blob(String),
    Gitlink,
    Unsupported,
}

pub(super) fn load_review_file_content(
    snapshot: &Snapshot,
    file: &SnapshotFile,
) -> Result<LoadedReviewContent, String> {
    if file.status == GitReviewFileStatus::Conflicted {
        return Ok(LoadedReviewContent::Unsupported);
    }

    let before_path = if matches!(
        file.status,
        GitReviewFileStatus::Renamed | GitReviewFileStatus::Copied
    ) {
        file.previous_path.as_deref().unwrap_or(&file.path)
    } else {
        &file.path
    };
    let before_absent = matches!(
        file.status,
        GitReviewFileStatus::Untracked | GitReviewFileStatus::Added
    );
    let after_absent = file.status == GitReviewFileStatus::Deleted;

    // Object references are resolved when the snapshot is created. All later reads use those
    // immutable IDs; worktree reads refuse repository-controlled symlink traversal.
    let before = if before_absent {
        SideContent::Missing
    } else {
        match &snapshot.comparison {
            SnapshotComparison::IndexToWorktree => load_repository_entry(
                &snapshot.repository,
                index_entry(&snapshot.repository, before_path)?,
            )?,
            SnapshotComparison::TreeToIndex { before_oid }
            | SnapshotComparison::TreeToWorktree { before_oid }
            | SnapshotComparison::TreeToTree { before_oid, .. } => load_repository_entry(
                &snapshot.repository,
                tree_entry(&snapshot.repository, before_oid, before_path)?,
            )?,
        }
    };
    let after = if after_absent {
        SideContent::Missing
    } else {
        match &snapshot.comparison {
            SnapshotComparison::IndexToWorktree | SnapshotComparison::TreeToWorktree { .. } => {
                read_worktree_content(&snapshot.repository, &file.path)
            }
            SnapshotComparison::TreeToIndex { .. } => load_repository_entry(
                &snapshot.repository,
                index_entry(&snapshot.repository, &file.path)?,
            )?,
            SnapshotComparison::TreeToTree { after_oid, .. } => load_repository_entry(
                &snapshot.repository,
                tree_entry(&snapshot.repository, after_oid, &file.path)?,
            )?,
        }
    };

    Ok(normalize_review_content(before, after))
}

pub(super) fn load_repository_entry(
    repository: &RepositoryContext,
    entry: Option<RepositoryEntry>,
) -> Result<SideContent, String> {
    match entry {
        Some(RepositoryEntry::Blob(oid)) => read_blob_content(repository, &oid),
        Some(RepositoryEntry::Gitlink | RepositoryEntry::Unsupported) => {
            Ok(SideContent::Unsupported)
        }
        None => Ok(SideContent::Unsupported),
    }
}

pub(super) fn tree_entry(
    repository: &RepositoryContext,
    treeish_oid: &str,
    path: &str,
) -> Result<Option<RepositoryEntry>, String> {
    let args = vec![
        "ls-tree".to_string(),
        "-z".to_string(),
        "--full-tree".to_string(),
        treeish_oid.to_string(),
        "--".to_string(),
        literal_pathspec(path),
    ];
    let output = run_git(&repository.root, &args, 128 * 1024)?;
    if !output.status.success() {
        return Err(git_failure("Unable to read the Git tree.", &output));
    }
    if output.stdout_truncated {
        return Err("The Git tree entry is too large to read safely.".to_string());
    }
    parse_tree_entry(&output.stdout, path)
}

pub(super) fn parse_tree_entry(
    output: &[u8],
    expected_path: &str,
) -> Result<Option<RepositoryEntry>, String> {
    let mut matched = None;
    for record in output
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
    {
        let Some(tab) = record.iter().position(|byte| *byte == b'\t') else {
            return Err("Git returned an invalid tree entry.".to_string());
        };
        if &record[tab + 1..] != expected_path.as_bytes() {
            continue;
        }
        if matched.is_some() {
            return Ok(Some(RepositoryEntry::Unsupported));
        }
        let metadata = std::str::from_utf8(&record[..tab])
            .map_err(|_| "Git returned an invalid tree entry.".to_string())?;
        let fields = metadata.split_ascii_whitespace().collect::<Vec<_>>();
        if fields.len() != 3 {
            return Err("Git returned an invalid tree entry.".to_string());
        }
        matched = Some(classify_repository_entry(
            fields[0],
            Some(fields[1]),
            fields[2],
        ));
    }
    Ok(matched)
}

pub(super) fn index_entry(
    repository: &RepositoryContext,
    path: &str,
) -> Result<Option<RepositoryEntry>, String> {
    let args = vec![
        "ls-files".to_string(),
        "--stage".to_string(),
        "-z".to_string(),
        "--".to_string(),
        literal_pathspec(path),
    ];
    let output = run_git(&repository.root, &args, 128 * 1024)?;
    if !output.status.success() {
        return Err(git_failure("Unable to read the Git index.", &output));
    }
    if output.stdout_truncated {
        return Err("The Git index entry is too large to read safely.".to_string());
    }
    parse_index_entry(&output.stdout, path)
}

pub(super) fn parse_index_entry(
    output: &[u8],
    expected_path: &str,
) -> Result<Option<RepositoryEntry>, String> {
    let mut matched = None;
    let mut saw_nonzero_stage = false;
    for record in output
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
    {
        let Some(tab) = record.iter().position(|byte| *byte == b'\t') else {
            return Err("Git returned an invalid index entry.".to_string());
        };
        if &record[tab + 1..] != expected_path.as_bytes() {
            continue;
        }
        let metadata = std::str::from_utf8(&record[..tab])
            .map_err(|_| "Git returned an invalid index entry.".to_string())?;
        let fields = metadata.split_ascii_whitespace().collect::<Vec<_>>();
        if fields.len() != 3 {
            return Err("Git returned an invalid index entry.".to_string());
        }
        if fields[2] != "0" {
            saw_nonzero_stage = true;
            continue;
        }
        if matched.is_some() {
            return Ok(Some(RepositoryEntry::Unsupported));
        }
        matched = Some(classify_repository_entry(fields[0], None, fields[1]));
    }
    if matched.is_none() && saw_nonzero_stage {
        return Ok(Some(RepositoryEntry::Unsupported));
    }
    Ok(matched)
}

pub(super) fn classify_repository_entry(
    mode: &str,
    kind: Option<&str>,
    oid: &str,
) -> RepositoryEntry {
    if mode == "160000" {
        return RepositoryEntry::Gitlink;
    }
    if !matches!(mode, "100644" | "100755" | "120000")
        || kind.is_some_and(|value| value != "blob")
        || !is_full_object_id(oid)
    {
        return RepositoryEntry::Unsupported;
    }
    RepositoryEntry::Blob(oid.to_string())
}

pub(super) fn is_full_object_id(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub(super) fn read_blob_content(
    repository: &RepositoryContext,
    oid: &str,
) -> Result<SideContent, String> {
    let output = run_git(
        &repository.root,
        &["cat-file".to_string(), "blob".to_string(), oid.to_string()],
        MAX_FULL_CONTENT_SIDE_BYTES,
    )?;
    if !output.status.success() {
        return Err(git_failure("Unable to read a Git file object.", &output));
    }
    if output.stdout_truncated {
        return Ok(SideContent::TooLarge);
    }
    Ok(SideContent::Bytes(output.stdout))
}

pub(super) fn read_worktree_content(repository: &RepositoryContext, path: &str) -> SideContent {
    if !is_safe_repository_path(path) {
        return SideContent::Unsupported;
    }
    read_worktree_content_platform(&repository.root, path).unwrap_or(SideContent::Unsupported)
}

#[cfg(unix)]
pub(super) fn read_worktree_content_platform(
    repository_root: &Path,
    path: &str,
) -> std::io::Result<SideContent> {
    use std::ffi::{CString, OsStr};
    use std::mem::MaybeUninit;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::OpenOptionsExt;

    fn component_name(component: &OsStr) -> std::io::Result<CString> {
        CString::new(component.as_bytes())
            .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))
    }

    fn open_directory_at(parent: &File, name: &CString) -> std::io::Result<File> {
        // SAFETY: `parent` owns a live directory descriptor and `name` is NUL-free for this call.
        let fd = unsafe {
            libc::openat(
                parent.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW,
            )
        };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: a successful `openat` returns a new descriptor whose ownership transfers here.
        Ok(unsafe { File::from_raw_fd(fd) })
    }

    let components = Path::new(path)
        .components()
        .filter_map(|component| match component {
            Component::Normal(name) => Some(name),
            Component::CurDir => None,
            _ => None,
        })
        .collect::<Vec<_>>();
    let Some((file_name, parent_components)) = components.split_last() else {
        return Ok(SideContent::Unsupported);
    };

    let mut directory = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(repository_root)?;
    for component in parent_components {
        directory = open_directory_at(&directory, &component_name(component)?)?;
    }
    let file_name = component_name(file_name)?;
    let mut metadata = MaybeUninit::<libc::stat>::uninit();
    // SAFETY: the descriptor and CString are live, and `metadata` points to writable storage.
    let metadata_result = unsafe {
        libc::fstatat(
            directory.as_raw_fd(),
            file_name.as_ptr(),
            metadata.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if metadata_result != 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: `fstatat` succeeded and initialized the entire `stat` value.
    let metadata = unsafe { metadata.assume_init() };
    let file_type = metadata.st_mode & libc::S_IFMT;

    if file_type == libc::S_IFLNK {
        let mut bytes = Vec::<u8>::with_capacity(MAX_FULL_CONTENT_SIDE_BYTES + 1);
        // SAFETY: the buffer has the advertised capacity and `readlinkat` writes at most that many
        // bytes without adding a terminator.
        let read = unsafe {
            libc::readlinkat(
                directory.as_raw_fd(),
                file_name.as_ptr(),
                bytes.as_mut_ptr().cast(),
                bytes.capacity(),
            )
        };
        if read < 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: `readlinkat` reported exactly how many bytes it initialized in the buffer.
        unsafe { bytes.set_len(read as usize) };
        return if bytes.len() > MAX_FULL_CONTENT_SIDE_BYTES {
            Ok(SideContent::TooLarge)
        } else {
            Ok(SideContent::Bytes(bytes))
        };
    }
    if file_type != libc::S_IFREG {
        return Ok(SideContent::Unsupported);
    }

    // SAFETY: the directory descriptor and NUL-free CString remain valid for this call.
    let fd = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            file_name.as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: a successful `openat` returns a new descriptor whose ownership transfers here.
    let file = unsafe { File::from_raw_fd(fd) };
    let mut bytes = Vec::new();
    file.take((MAX_FULL_CONTENT_SIDE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_FULL_CONTENT_SIDE_BYTES {
        Ok(SideContent::TooLarge)
    } else {
        Ok(SideContent::Bytes(bytes))
    }
}

#[cfg(not(unix))]
pub(super) fn read_worktree_content_platform(
    repository_root: &Path,
    path: &str,
) -> std::io::Result<SideContent> {
    let target = repository_root.join(path);
    let parent = target
        .parent()
        .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::InvalidInput))?
        .canonicalize()?;
    if !parent.starts_with(repository_root) {
        return Ok(SideContent::Unsupported);
    }
    let metadata = fs::symlink_metadata(&target)?;
    if metadata.file_type().is_symlink() {
        return Ok(match fs::read_link(&target)?.to_str() {
            Some(value) if value.len() <= MAX_FULL_CONTENT_SIDE_BYTES => {
                SideContent::Bytes(value.as_bytes().to_vec())
            }
            Some(_) => SideContent::TooLarge,
            None => SideContent::Unsupported,
        });
    }
    if !metadata.is_file() {
        return Ok(SideContent::Unsupported);
    }
    let resolved = target.canonicalize()?;
    if !resolved.starts_with(repository_root) {
        return Ok(SideContent::Unsupported);
    }
    let mut bytes = Vec::new();
    OpenOptions::new()
        .read(true)
        .open(resolved)?
        .take((MAX_FULL_CONTENT_SIDE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_FULL_CONTENT_SIDE_BYTES {
        Ok(SideContent::TooLarge)
    } else {
        Ok(SideContent::Bytes(bytes))
    }
}

pub(super) fn normalize_review_content(
    before: SideContent,
    after: SideContent,
) -> LoadedReviewContent {
    if matches!(before, SideContent::TooLarge) || matches!(after, SideContent::TooLarge) {
        return LoadedReviewContent::TooLarge;
    }
    if matches!(before, SideContent::Unsupported) || matches!(after, SideContent::Unsupported) {
        return LoadedReviewContent::Unsupported;
    }

    let before = match before {
        SideContent::Missing => None,
        SideContent::Bytes(bytes) => Some(bytes),
        SideContent::TooLarge | SideContent::Unsupported => unreachable!(),
    };
    let after = match after {
        SideContent::Missing => None,
        SideContent::Bytes(bytes) => Some(bytes),
        SideContent::TooLarge | SideContent::Unsupported => unreachable!(),
    };
    let total_bytes = before.as_ref().map_or(0, Vec::len) + after.as_ref().map_or(0, Vec::len);
    if total_bytes > MAX_FULL_CONTENT_TOTAL_BYTES {
        return LoadedReviewContent::TooLarge;
    }
    if before
        .iter()
        .chain(after.iter())
        .any(|bytes| bytes.contains(&0))
    {
        return LoadedReviewContent::Binary;
    }
    if before
        .iter()
        .chain(after.iter())
        .any(|bytes| review_line_count(bytes) > MAX_FULL_CONTENT_SIDE_LINES)
    {
        return LoadedReviewContent::TooLarge;
    }

    let before_text = match before {
        Some(bytes) => match String::from_utf8(bytes) {
            Ok(text) => Some(text),
            Err(_) => return LoadedReviewContent::Unsupported,
        },
        None => None,
    };
    let after_text = match after {
        Some(bytes) => match String::from_utf8(bytes) {
            Ok(text) => Some(text),
            Err(_) => return LoadedReviewContent::Unsupported,
        },
        None => None,
    };
    LoadedReviewContent::Ready {
        before_text,
        after_text,
    }
}

pub(super) fn review_line_count(bytes: &[u8]) -> usize {
    bytes.iter().filter(|byte| **byte == b'\n').count()
        + usize::from(!bytes.is_empty() && !bytes.ends_with(b"\n"))
}

pub(super) fn content_response(
    snapshot_id: &str,
    file_id: &str,
    loaded: LoadedReviewContent,
) -> GitReviewFileContent {
    let (status, before_text, after_text) = match loaded {
        LoadedReviewContent::Ready {
            before_text,
            after_text,
        } => (GitReviewFileContentStatus::Ready, before_text, after_text),
        LoadedReviewContent::Binary => (GitReviewFileContentStatus::Binary, None, None),
        LoadedReviewContent::TooLarge => (GitReviewFileContentStatus::TooLarge, None, None),
        LoadedReviewContent::Unsupported => (GitReviewFileContentStatus::Unsupported, None, None),
    };
    GitReviewFileContent {
        snapshot_id: snapshot_id.to_string(),
        file_id: file_id.to_string(),
        status,
        before_text,
        after_text,
    }
}

pub(super) fn expired_content(snapshot_id: &str, file_id: &str) -> GitReviewFileContent {
    GitReviewFileContent {
        snapshot_id: snapshot_id.to_string(),
        file_id: file_id.to_string(),
        status: GitReviewFileContentStatus::SnapshotExpired,
        before_text: None,
        after_text: None,
    }
}
