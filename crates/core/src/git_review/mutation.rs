use super::*;

pub(super) fn expired_mutation(
    snapshot_id: &str,
    file_id: &str,
    action: GitReviewFileMutationAction,
) -> GitReviewFileMutation {
    GitReviewFileMutation {
        snapshot_id: snapshot_id.to_string(),
        file_id: file_id.to_string(),
        action,
        status: GitReviewFileMutationStatus::SnapshotExpired,
    }
}

pub(super) fn mutation_paths(file: &SnapshotFile) -> Vec<String> {
    let mut paths = vec![literal_pathspec(&file.path)];
    if let Some(previous_path) = &file.previous_path {
        paths.push(literal_pathspec(previous_path));
    }
    paths
}

pub(super) fn run_file_mutation<const N: usize>(
    repository: &RepositoryContext,
    failure_message: &str,
    command: [&str; N],
    file: &SnapshotFile,
) -> Result<(), String> {
    let mut args = command
        .into_iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    args.push("--".to_string());
    args.extend(mutation_paths(file));
    let output = run_git(&repository.root, &args, 16 * 1024)?;
    if !output.status.success() {
        return Err(git_failure(failure_message, &output));
    }
    Ok(())
}

pub(super) fn unstage_file(
    repository: &RepositoryContext,
    file: &SnapshotFile,
    has_head: bool,
) -> Result<(), String> {
    if has_head {
        run_file_mutation(
            repository,
            "Unable to unstage the file.",
            ["reset", "--quiet"],
            file,
        )
    } else {
        run_file_mutation(
            repository,
            "Unable to unstage the file.",
            ["rm", "--cached", "--quiet", "--ignore-unmatch"],
            file,
        )
    }
}

pub(super) fn ensure_no_external_filter(
    repository: &RepositoryContext,
    file: &SnapshotFile,
) -> Result<(), String> {
    let filtered = external_filter_paths(
        repository,
        std::iter::once(file.path.as_str()).chain(file.previous_path.as_deref()),
    )?;
    if !filtered.is_empty() {
        return Err(
            "This file uses a Git filter and cannot be reviewed or changed safely from Review."
                .into(),
        );
    }
    Ok(())
}

pub(super) fn external_filter_paths<'a>(
    repository: &RepositoryContext,
    paths: impl IntoIterator<Item = &'a str>,
) -> Result<HashSet<String>, String> {
    let mut input = Vec::new();
    let mut expected = HashSet::new();
    for path in paths {
        if !expected.insert(path) {
            continue;
        }
        if path.as_bytes().contains(&0)
            || input.len().saturating_add(path.len()).saturating_add(1) > 2 * MAX_STATUS_BYTES
        {
            return Err("The Git attribute input is too large or invalid.".into());
        }
        input.extend_from_slice(path.as_bytes());
        input.push(0);
    }
    if input.is_empty() {
        return Ok(HashSet::new());
    }
    let output = run_git_with_input(
        &repository.root,
        &[
            "check-attr".into(),
            "-z".into(),
            "--stdin".into(),
            "filter".into(),
        ],
        4 * MAX_STATUS_BYTES,
        Some(&input),
    )?;
    if !output.status.success() || output.stdout_truncated {
        return Err(git_failure(
            "Unable to verify the file's Git filter configuration.",
            &output,
        ));
    }
    let invalid = || "Git returned incomplete filter attributes.".to_string();
    let body = output.stdout.strip_suffix(&[0]).ok_or_else(invalid)?;
    let fields = body.split(|byte| *byte == 0).collect::<Vec<_>>();
    if fields.len() != expected.len().saturating_mul(3) {
        return Err(invalid());
    }
    let mut filtered = HashSet::new();
    for entry in fields.as_chunks::<3>().0 {
        let path = std::str::from_utf8(entry[0]).map_err(|_| invalid())?;
        if !expected.remove(path) || entry[1] != b"filter" {
            return Err(invalid());
        }
        let value = entry[2];
        if value != b"unspecified" && value != b"unset" {
            filtered.insert(path.to_string());
        }
    }
    Ok(filtered)
}

pub(super) fn remove_untracked_file(
    repository: &RepositoryContext,
    file: &SnapshotFile,
) -> Result<(), String> {
    let target = repository.root.join(&file.path);
    let parent = target
        .parent()
        .ok_or_else(|| "The untracked file path is invalid.".to_string())?
        .canonicalize()
        .map_err(|_| "The untracked file directory is no longer available.".to_string())?;
    if !parent.starts_with(&repository.root) {
        return Err("The untracked file path escapes the repository.".to_string());
    }
    let metadata = fs::symlink_metadata(&target)
        .map_err(|_| "The untracked file is no longer available.".to_string())?;
    if metadata.is_dir() {
        return Err("Review does not remove untracked directories.".to_string());
    }
    fs::remove_file(&target)
        .map_err(|error| format!("Unable to remove the untracked file: {error}"))
}
