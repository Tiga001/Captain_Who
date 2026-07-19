use super::*;

pub(super) fn expired_diff(snapshot_id: &str, file_id: &str) -> GitReviewFileDiff {
    GitReviewFileDiff {
        snapshot_id: snapshot_id.to_string(),
        file_id: file_id.to_string(),
        status: GitReviewFileDiffStatus::SnapshotExpired,
        patch: None,
    }
}

pub(super) fn untracked_file_diff(
    snapshot_id: &str,
    file_id: &str,
    repository: &RepositoryContext,
    path: &str,
) -> Result<GitReviewFileDiff, String> {
    let target = repository.root.join(path);
    let metadata = fs::symlink_metadata(&target)
        .map_err(|_| "The untracked file is no longer available.".to_string())?;
    let bytes = if metadata.file_type().is_symlink() {
        fs::read_link(&target)
            .map_err(|_| "The untracked symbolic link could not be read.".to_string())?
            .to_string_lossy()
            .into_owned()
            .into_bytes()
    } else if metadata.is_file() {
        let mut bytes = Vec::new();
        open_untracked_file(&target)
            .map_err(|_| "The untracked file could not be opened.".to_string())?
            .take((MAX_DIFF_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| "The untracked file could not be read.".to_string())?;
        bytes
    } else {
        return Ok(GitReviewFileDiff {
            snapshot_id: snapshot_id.to_string(),
            file_id: file_id.to_string(),
            status: GitReviewFileDiffStatus::Binary,
            patch: None,
        });
    };

    if bytes.len() > MAX_DIFF_BYTES {
        return Ok(GitReviewFileDiff {
            snapshot_id: snapshot_id.to_string(),
            file_id: file_id.to_string(),
            status: GitReviewFileDiffStatus::TooLarge,
            patch: None,
        });
    }
    if bytes.contains(&0) {
        return Ok(GitReviewFileDiff {
            snapshot_id: snapshot_id.to_string(),
            file_id: file_id.to_string(),
            status: GitReviewFileDiffStatus::Binary,
            patch: None,
        });
    }
    let content = String::from_utf8(bytes).map_err(|_| {
        "The untracked file is not UTF-8 text and cannot be reviewed inline.".to_string()
    })?;
    let patch = TextDiff::from_lines("", &content)
        .unified_diff()
        .header("/dev/null", &format!("b/{path}"))
        .to_string();
    if exceeds_diff_patch_budget(patch.as_bytes()) {
        return Ok(GitReviewFileDiff {
            snapshot_id: snapshot_id.to_string(),
            file_id: file_id.to_string(),
            status: GitReviewFileDiffStatus::TooLarge,
            patch: None,
        });
    }

    Ok(GitReviewFileDiff {
        snapshot_id: snapshot_id.to_string(),
        file_id: file_id.to_string(),
        status: GitReviewFileDiffStatus::Ready,
        patch: Some(patch),
    })
}

/// Rejects patches that are cheap in bytes but pathological to parse or paint. The line budget
/// counts every physical patch line (not only changed rows), while hunk headers use Git's ordinary
/// unified-diff `@@ -...` prefix. A final newline does not create an additional logical line.
pub(super) fn exceeds_diff_patch_budget(patch: &[u8]) -> bool {
    if patch.len() > MAX_DIFF_BYTES {
        return true;
    }
    if patch.is_empty() {
        return false;
    }

    let mut line_count = 0usize;
    let mut hunk_count = 0usize;
    let mut lines = patch.split(|byte| *byte == b'\n').peekable();
    while let Some(raw_line) = lines.next() {
        if raw_line.is_empty() && patch.ends_with(b"\n") && lines.peek().is_none() {
            // `slice::split` emits one final empty item for a trailing delimiter.
            continue;
        }
        let line = raw_line.strip_suffix(b"\r").unwrap_or(raw_line);
        line_count += 1;
        if line_count > MAX_DIFF_LINES || line.len() > MAX_DIFF_LINE_BYTES {
            return true;
        }
        if line.starts_with(b"@@ -") {
            hunk_count += 1;
            if hunk_count > MAX_DIFF_HUNKS {
                return true;
            }
        }
    }
    false
}

#[cfg(unix)]
pub(super) fn open_untracked_file(path: &Path) -> std::io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;

    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
}

#[cfg(not(unix))]
pub(super) fn open_untracked_file(path: &Path) -> std::io::Result<File> {
    OpenOptions::new().read(true).open(path)
}
