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
    let mut args = vec![
        "check-attr".to_string(),
        "-z".to_string(),
        "filter".to_string(),
        "--".to_string(),
        file.path.clone(),
    ];
    if let Some(previous_path) = &file.previous_path {
        args.push(previous_path.clone());
    }
    let output = run_git(&repository.root, &args, 64 * 1024)?;
    if !output.status.success() || output.stdout_truncated {
        return Err(git_failure(
            "Unable to verify the file's Git filter configuration.",
            &output,
        ));
    }
    let fields = output.stdout.split(|byte| *byte == 0).collect::<Vec<_>>();
    for entry in fields.chunks(3) {
        if entry.len() < 3 || entry[0].is_empty() {
            continue;
        }
        let value = entry[2];
        if value != b"unspecified" && value != b"unset" {
            return Err(
                "This file uses a repository-defined Git filter and cannot be changed safely from Review."
                    .to_string(),
            );
        }
    }
    Ok(())
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
