use super::*;

#[derive(Debug, Clone)]
pub(super) struct RepositoryContext {
    pub(super) source_path: PathBuf,
    pub(super) source_root: PathBuf,
    pub(super) source_identity: FileChangeDirectoryIdentity,
    pub(super) root_identity: FileChangeDirectoryIdentity,
    pub(super) git_dir_identity: FileChangeDirectoryIdentity,
    pub(super) git_entry_identity: String,
    pub(super) root: PathBuf,
    pub(super) git_dir: PathBuf,
    pub(super) project_prefix: String,
    pub(super) pathspec: String,
    pub(super) repository_id: String,
}

impl RepositoryContext {
    pub(super) fn project_relative_path(&self, repository_path: &str) -> Option<String> {
        let path = Path::new(repository_path);
        let relative = if self.project_prefix.is_empty() {
            path
        } else {
            path.strip_prefix(Path::new(&self.project_prefix)).ok()?
        };
        let relative = path_to_git_string(relative)?;
        (!relative.is_empty()).then_some(relative)
    }
}

#[derive(Debug)]
pub(super) enum RepositoryResolutionError {
    NotRepository,
    Unsupported(String),
    Unavailable(String),
}

impl RepositoryResolutionError {
    pub(super) fn message(self) -> String {
        match self {
            Self::NotRepository => "The selected project is not a Git repository.".to_string(),
            Self::Unsupported(message) | Self::Unavailable(message) => message,
        }
    }
}

pub(super) fn resolve_repository(
    project_path: &Path,
) -> Result<RepositoryContext, RepositoryResolutionError> {
    let project_root = project_path.canonicalize().map_err(|_| {
        RepositoryResolutionError::Unavailable(
            "The selected project directory is unavailable.".to_string(),
        )
    })?;
    if !project_root.is_dir() {
        return Err(RepositoryResolutionError::Unavailable(
            "The selected project path is not a directory.".to_string(),
        ));
    }

    let inside = run_git(
        &project_root,
        &["rev-parse".to_string(), "--is-inside-work-tree".to_string()],
        4096,
    )
    .map_err(RepositoryResolutionError::Unavailable)?;
    if !inside.status.success() || trim_ascii(&inside.stdout) != b"true" {
        return Err(RepositoryResolutionError::NotRepository);
    }

    let root_output = run_git(
        &project_root,
        &["rev-parse".to_string(), "--show-toplevel".to_string()],
        16 * 1024,
    )
    .map_err(RepositoryResolutionError::Unavailable)?;
    if !root_output.status.success() {
        return Err(RepositoryResolutionError::Unsupported(
            "This Git repository does not have a reviewable worktree.".to_string(),
        ));
    }
    let root_text = std::str::from_utf8(trim_ascii(&root_output.stdout)).map_err(|_| {
        RepositoryResolutionError::Unsupported(
            "The Git worktree path is not valid UTF-8.".to_string(),
        )
    })?;
    let root = PathBuf::from(root_text).canonicalize().map_err(|_| {
        RepositoryResolutionError::Unavailable("The Git worktree is unavailable.".to_string())
    })?;
    let relative_root = project_root.strip_prefix(&root).map_err(|_| {
        RepositoryResolutionError::Unsupported(
            "The selected project is outside the resolved Git worktree.".to_string(),
        )
    })?;
    let relative_path = path_to_git_string(relative_root).ok_or_else(|| {
        RepositoryResolutionError::Unsupported(
            "The selected project path cannot be represented safely for Git.".to_string(),
        )
    })?;
    let pathspec = if relative_path.is_empty() {
        ".".to_string()
    } else {
        literal_pathspec(&relative_path)
    };

    let git_dir_output = run_git(
        &project_root,
        &["rev-parse".to_string(), "--absolute-git-dir".to_string()],
        16 * 1024,
    )
    .map_err(RepositoryResolutionError::Unavailable)?;
    if !git_dir_output.status.success() {
        return Err(RepositoryResolutionError::Unavailable(
            "The Git metadata directory could not be resolved.".to_string(),
        ));
    }
    let git_dir_text = std::str::from_utf8(trim_ascii(&git_dir_output.stdout)).map_err(|_| {
        RepositoryResolutionError::Unsupported(
            "The Git metadata path is not valid UTF-8.".to_string(),
        )
    })?;
    let git_dir = PathBuf::from(git_dir_text).canonicalize().map_err(|_| {
        RepositoryResolutionError::Unavailable("The Git metadata directory is unavailable.".into())
    })?;
    let identity_at = |path: &Path| {
        FileChangeDirectoryIdentity::read(path).map_err(|_| {
            RepositoryResolutionError::Unavailable(
                "The Git directory identity is unavailable.".into(),
            )
        })
    };
    let source_identity = identity_at(&project_root)?;
    let root_identity = identity_at(&root)?;
    let git_dir_identity = identity_at(&git_dir)?;
    let git_entry_identity = worktree_binding_identity(&project_root, &root)
        .map_err(RepositoryResolutionError::Unavailable)?;
    let identity = format!(
        "{}\0{}\0{root_identity:?}\0{git_dir_identity:?}",
        root.display(),
        git_dir.display()
    );

    Ok(RepositoryContext {
        source_path: project_path.to_path_buf(),
        source_root: project_root,
        source_identity,
        root_identity,
        git_dir_identity,
        git_entry_identity,
        root,
        git_dir,
        project_prefix: relative_path,
        pathspec,
        repository_id: content_revision(identity.as_bytes()),
    })
}

fn git_entry_identity(root: &Path) -> Result<String, String> {
    let path = root.join(".git");
    let metadata =
        fs::symlink_metadata(&path).map_err(|_| "The Git worktree binding is unavailable.")?;
    if metadata.file_type().is_symlink() {
        return fs::read_link(&path)
            .map(|p| format!("link:{}", p.display()))
            .map_err(|_| "The Git metadata link is unavailable.".into());
    }
    if metadata.is_dir() {
        return FileChangeDirectoryIdentity::read(&path)
            .map(|id| format!("directory:{id:?}"))
            .map_err(|_| "The Git metadata identity is unavailable.".into());
    }
    if !metadata.is_file() || metadata.len() > 32768 {
        return Err("The Git worktree binding is invalid.".into());
    }
    fs::read(path)
        .map(|bytes| content_revision(&bytes))
        .map_err(|_| "The Git worktree binding is unavailable.".into())
}

// A new nested .git entry changes which repository owns a selected source even when the
// original parent worktree and index still exist. Capture every discovery boundary.
fn worktree_binding_identity(source: &Path, root: &Path) -> Result<String, String> {
    let mut material = String::new();
    for directory in source.ancestors() {
        match fs::symlink_metadata(directory.join(".git")) {
            Ok(_) => material.push_str(&format!(
                "{}\0{}\0",
                directory.display(),
                git_entry_identity(directory)?
            )),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                material.push_str(&format!("{}\0absent\0", directory.display()))
            }
            Err(_) => return Err("The Git source discovery boundary is unavailable.".into()),
        }
        if directory == root {
            return Ok(content_revision(material.as_bytes()));
        }
    }
    Err("The Git source is outside its recorded worktree.".into())
}

pub(super) fn repository_is_current(repository: &RepositoryContext) -> bool {
    repository.source_path.canonicalize().ok().as_ref() == Some(&repository.source_root)
        && FileChangeDirectoryIdentity::read(&repository.source_root)
            .ok()
            .as_ref()
            == Some(&repository.source_identity)
        && FileChangeDirectoryIdentity::read(&repository.root)
            .ok()
            .as_ref()
            == Some(&repository.root_identity)
        && FileChangeDirectoryIdentity::read(&repository.git_dir)
            .ok()
            .as_ref()
            == Some(&repository.git_dir_identity)
        && worktree_binding_identity(&repository.source_root, &repository.root)
            .ok()
            .as_ref()
            == Some(&repository.git_entry_identity)
}

pub(super) struct ParsedStatusFile {
    pub(super) path: String,
    pub(super) previous_path: Option<String>,
    pub(super) status: GitReviewFileStatus,
}

pub(super) struct CollectedReviewFiles {
    pub(super) files: Vec<ParsedStatusFile>,
    pub(super) stats: GitReviewStats,
    pub(super) file_stats: HashMap<String, GitReviewFileStats>,
}

pub(super) fn parse_porcelain_status(
    output: &[u8],
    selection: StatusSelection,
) -> Result<Vec<ParsedStatusFile>, String> {
    let entries = output
        .split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty())
        .collect::<Vec<_>>();
    let mut files = Vec::new();
    let mut index = 0;

    while index < entries.len() {
        let entry = entries[index];
        if entry.len() < 4 || entry[2] != b' ' {
            return Err("Git returned an invalid porcelain status entry.".to_string());
        }
        let x = entry[0];
        let y = entry[1];
        let path = parse_git_path(&entry[3..])?;
        index += 1;

        let has_previous_path = matches!(x, b'R' | b'C') || matches!(y, b'R' | b'C');
        let previous_path = if has_previous_path {
            let previous = entries
                .get(index)
                .ok_or_else(|| "Git omitted a rename source path.".to_string())?;
            index += 1;
            Some(parse_git_path(previous)?)
        } else {
            None
        };

        let selected = if x == b'?' && y == b'?' {
            selection == StatusSelection::Unstaged
        } else {
            match selection {
                StatusSelection::Staged => x != b' ',
                StatusSelection::Unstaged => y != b' ',
            }
        };
        if !selected {
            continue;
        }

        let status = if x == b'?' && y == b'?' {
            GitReviewFileStatus::Untracked
        } else if is_conflict_status(x, y) {
            GitReviewFileStatus::Conflicted
        } else {
            status_from_code(match selection {
                StatusSelection::Staged => x,
                StatusSelection::Unstaged => y,
            })
        };
        files.push(ParsedStatusFile {
            path,
            previous_path,
            status,
        });
    }

    files.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(files)
}

pub(super) fn calculate_review_stats(
    repository: &RepositoryContext,
    selection: StatusSelection,
    files: &[ParsedStatusFile],
) -> (GitReviewStats, HashMap<String, GitReviewFileStats>) {
    let mut stats = GitReviewStats {
        file_count: files.len(),
        additions: 0,
        deletions: 0,
        line_counts_complete: true,
    };
    let mut file_stats = HashMap::new();

    match tracked_numstat(repository, selection) {
        Ok(numstat) => {
            stats.additions = numstat.additions;
            stats.deletions = numstat.deletions;
            file_stats = numstat.files;
        }
        Err(()) => stats.line_counts_complete = false,
    }

    if selection == StatusSelection::Unstaged {
        let mut remaining_bytes = MAX_UNTRACKED_STATS_BYTES;
        for file in files
            .iter()
            .filter(|file| file.status == GitReviewFileStatus::Untracked)
        {
            match untracked_line_count(repository, &file.path, &mut remaining_bytes) {
                Ok(Some(additions)) => {
                    let Some(total) = stats.additions.checked_add(additions) else {
                        stats.line_counts_complete = false;
                        break;
                    };
                    stats.additions = total;
                    file_stats.insert(
                        file.path.clone(),
                        GitReviewFileStats {
                            additions,
                            deletions: 0,
                        },
                    );
                }
                Ok(None) => {}
                Err(()) => {
                    stats.line_counts_complete = false;
                    break;
                }
            }
        }
    }

    (stats, file_stats)
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct ParsedNumstat {
    pub(super) additions: u64,
    pub(super) deletions: u64,
    pub(super) files: HashMap<String, GitReviewFileStats>,
}

pub(super) fn tracked_numstat(
    repository: &RepositoryContext,
    selection: StatusSelection,
) -> Result<ParsedNumstat, ()> {
    let mut args = vec![
        "diff".to_string(),
        "--no-ext-diff".to_string(),
        "--no-textconv".to_string(),
        "--numstat".to_string(),
        "-z".to_string(),
    ];
    if selection == StatusSelection::Staged {
        args.push("--cached".to_string());
    }
    args.extend(["--".to_string(), repository.pathspec.clone()]);

    let output = run_git(&repository.root, &args, MAX_NUMSTAT_BYTES).map_err(|_| ())?;
    if !output.status.success() || output.stdout_truncated {
        return Err(());
    }
    parse_numstat(&output.stdout)
}

pub(super) fn collect_review_files(
    repository: &RepositoryContext,
    target: &GitReviewTarget,
    comparison: &SnapshotComparison,
) -> Result<CollectedReviewFiles, String> {
    let selection = match target {
        GitReviewTarget::Unstaged => Some(StatusSelection::Unstaged),
        GitReviewTarget::Staged => Some(StatusSelection::Staged),
        _ => None,
    };
    if let Some(selection) = selection {
        let output = read_porcelain_status(repository)?;
        let files = parse_porcelain_status(&output, selection)?;
        let (stats, file_stats) = calculate_review_stats(repository, selection, &files);
        return Ok(CollectedReviewFiles {
            files,
            stats,
            file_stats,
        });
    }

    let mut files = comparison_name_status(repository, comparison)?;
    if matches!(comparison, SnapshotComparison::TreeToWorktree { .. }) {
        let status = read_porcelain_status(repository)?;
        files.extend(
            parse_porcelain_status(&status, StatusSelection::Unstaged)?
                .into_iter()
                .filter(|file| file.status == GitReviewFileStatus::Untracked),
        );
    }
    files.sort_by(|left, right| left.path.cmp(&right.path));
    files.dedup_by(|left, right| left.path == right.path);

    let mut stats = GitReviewStats {
        file_count: files.len(),
        additions: 0,
        deletions: 0,
        line_counts_complete: true,
    };
    let mut file_stats = match comparison_numstat(repository, comparison) {
        Ok(numstat) => {
            stats.additions = numstat.additions;
            stats.deletions = numstat.deletions;
            numstat.files
        }
        Err(()) => {
            stats.line_counts_complete = false;
            HashMap::new()
        }
    };
    if matches!(comparison, SnapshotComparison::TreeToWorktree { .. }) {
        add_untracked_stats(repository, &files, &mut stats, &mut file_stats);
    }
    Ok(CollectedReviewFiles {
        files,
        stats,
        file_stats,
    })
}

fn read_porcelain_status(repository: &RepositoryContext) -> Result<Vec<u8>, String> {
    let args = vec![
        "status".to_string(),
        "--porcelain=v1".to_string(),
        "-z".to_string(),
        "--untracked-files=all".to_string(),
        "--".to_string(),
        repository.pathspec.clone(),
    ];
    let output = run_git(&repository.root, &args, MAX_STATUS_BYTES)?;
    if !output.status.success() {
        return Err(git_failure("Unable to read Git status.", &output));
    }
    if output.stdout_truncated {
        return Err("Git status is too large to review safely.".to_string());
    }
    Ok(output.stdout)
}

fn comparison_name_status(
    repository: &RepositoryContext,
    comparison: &SnapshotComparison,
) -> Result<Vec<ParsedStatusFile>, String> {
    let mut args = vec![
        "diff".to_string(),
        "--no-ext-diff".to_string(),
        "--no-textconv".to_string(),
        "--name-status".to_string(),
        "-z".to_string(),
        "--find-renames".to_string(),
        "--find-copies".to_string(),
    ];
    append_comparison_diff_args(&mut args, comparison);
    args.extend(["--".to_string(), repository.pathspec.clone()]);
    let output = run_git(&repository.root, &args, MAX_STATUS_BYTES)?;
    if !output.status.success() {
        return Err(git_failure("Unable to read the Git comparison.", &output));
    }
    if output.stdout_truncated {
        return Err("The Git comparison is too large to review safely.".to_string());
    }
    parse_name_status(&output.stdout)
}

pub(super) fn append_comparison_diff_args(args: &mut Vec<String>, comparison: &SnapshotComparison) {
    match comparison {
        SnapshotComparison::IndexToWorktree => {}
        SnapshotComparison::TreeToIndex { before_oid } => {
            args.push("--cached".to_string());
            args.push(before_oid.clone());
        }
        SnapshotComparison::TreeToWorktree { before_oid } => args.push(before_oid.clone()),
        SnapshotComparison::TreeToTree {
            before_oid,
            after_oid,
        } => {
            args.push(before_oid.clone());
            args.push(after_oid.clone());
        }
    }
}

fn parse_name_status(output: &[u8]) -> Result<Vec<ParsedStatusFile>, String> {
    let records = output
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
        .collect::<Vec<_>>();
    let mut files = Vec::new();
    let mut index = 0;
    while index < records.len() {
        let status_record = records[index];
        index += 1;
        let status_text = std::str::from_utf8(status_record)
            .map_err(|_| "Git returned an invalid comparison status.".to_string())?;
        let mut inline = status_text.splitn(2, '\t');
        let code = inline.next().unwrap_or_default();
        if code.is_empty() {
            return Err("Git returned an empty comparison status.".to_string());
        }
        let first_path = match inline.next() {
            Some(path) => path.to_string(),
            None => {
                let path = records
                    .get(index)
                    .ok_or_else(|| "Git omitted a comparison path.".to_string())?;
                index += 1;
                parse_git_path(path)?
            }
        };
        if !is_safe_repository_path(&first_path) {
            return Err("Git returned a path outside the repository.".to_string());
        }
        let code_byte = code.as_bytes()[0];
        let (path, previous_path) = if matches!(code_byte, b'R' | b'C') {
            let next = records
                .get(index)
                .ok_or_else(|| "Git omitted a renamed comparison path.".to_string())?;
            index += 1;
            (parse_git_path(next)?, Some(first_path))
        } else {
            (first_path, None)
        };
        files.push(ParsedStatusFile {
            path,
            previous_path,
            status: status_from_code(code_byte),
        });
    }
    files.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(files)
}

fn comparison_numstat(
    repository: &RepositoryContext,
    comparison: &SnapshotComparison,
) -> Result<ParsedNumstat, ()> {
    let mut args = vec![
        "diff".to_string(),
        "--no-ext-diff".to_string(),
        "--no-textconv".to_string(),
        "--numstat".to_string(),
        "-z".to_string(),
    ];
    append_comparison_diff_args(&mut args, comparison);
    args.extend(["--".to_string(), repository.pathspec.clone()]);
    let output = run_git(&repository.root, &args, MAX_NUMSTAT_BYTES).map_err(|_| ())?;
    if !output.status.success() || output.stdout_truncated {
        return Err(());
    }
    parse_numstat(&output.stdout)
}

fn add_untracked_stats(
    repository: &RepositoryContext,
    files: &[ParsedStatusFile],
    stats: &mut GitReviewStats,
    file_stats: &mut HashMap<String, GitReviewFileStats>,
) {
    let mut remaining_bytes = MAX_UNTRACKED_STATS_BYTES;
    for file in files
        .iter()
        .filter(|file| file.status == GitReviewFileStatus::Untracked)
    {
        match untracked_line_count(repository, &file.path, &mut remaining_bytes) {
            Ok(Some(additions)) => {
                let Some(total) = stats.additions.checked_add(additions) else {
                    stats.line_counts_complete = false;
                    break;
                };
                stats.additions = total;
                file_stats.insert(
                    file.path.clone(),
                    GitReviewFileStats {
                        additions,
                        deletions: 0,
                    },
                );
            }
            Ok(None) => {}
            Err(()) => {
                stats.line_counts_complete = false;
                break;
            }
        }
    }
}

pub(super) fn parse_numstat(output: &[u8]) -> Result<ParsedNumstat, ()> {
    let records = output.split(|byte| *byte == 0).collect::<Vec<_>>();
    let mut result = ParsedNumstat::default();
    let mut index = 0;

    while index < records.len() {
        let record = records[index];
        index += 1;
        if record.is_empty() {
            continue;
        }
        let mut fields = record.splitn(3, |byte| *byte == b'\t');
        let added = fields.next().ok_or(())?;
        let deleted = fields.next().ok_or(())?;
        let path = fields.next().ok_or(())?;
        let path = if path.is_empty() {
            let previous_path = records.get(index).ok_or(())?;
            index += 1;
            let path = records.get(index).ok_or(())?;
            index += 1;
            parse_git_path(previous_path).map_err(|_| ())?;
            parse_git_path(path).map_err(|_| ())?
        } else {
            parse_git_path(path).map_err(|_| ())?
        };

        if added == b"-" || deleted == b"-" {
            continue;
        }
        let added = std::str::from_utf8(added)
            .map_err(|_| ())?
            .parse::<u64>()
            .map_err(|_| ())?;
        let deleted = std::str::from_utf8(deleted)
            .map_err(|_| ())?
            .parse::<u64>()
            .map_err(|_| ())?;
        result.additions = result.additions.checked_add(added).ok_or(())?;
        result.deletions = result.deletions.checked_add(deleted).ok_or(())?;
        let entry = result.files.entry(path).or_default();
        entry.additions = entry.additions.checked_add(added).ok_or(())?;
        entry.deletions = entry.deletions.checked_add(deleted).ok_or(())?;
    }

    Ok(result)
}

pub(super) fn untracked_line_count(
    repository: &RepositoryContext,
    path: &str,
    remaining_bytes: &mut usize,
) -> Result<Option<u64>, ()> {
    let target = repository.root.join(path);
    let metadata = fs::symlink_metadata(&target).map_err(|_| ())?;
    let bytes = if metadata.file_type().is_symlink() {
        fs::read_link(&target)
            .map_err(|_| ())?
            .to_string_lossy()
            .into_owned()
            .into_bytes()
    } else if metadata.is_file() {
        if *remaining_bytes == 0 {
            return Err(());
        }
        let mut bytes = Vec::new();
        open_untracked_file(&target)
            .map_err(|_| ())?
            .take((*remaining_bytes + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| ())?;
        bytes
    } else {
        return Ok(None);
    };

    if bytes.len() > *remaining_bytes {
        return Err(());
    }
    *remaining_bytes -= bytes.len();
    if bytes.contains(&0) || std::str::from_utf8(&bytes).is_err() {
        return Ok(None);
    }

    let newline_count = bytes.iter().filter(|byte| **byte == b'\n').count() as u64;
    let trailing_line = u64::from(!bytes.is_empty() && !bytes.ends_with(b"\n"));
    Ok(Some(newline_count + trailing_line))
}

pub(super) fn parse_git_path(bytes: &[u8]) -> Result<String, String> {
    let path = std::str::from_utf8(bytes)
        .map_err(|_| "Git paths with non-UTF-8 names are not supported yet.".to_string())?;
    if !is_safe_repository_path(path) {
        return Err("Git returned a path outside the repository.".to_string());
    }
    Ok(path.to_string())
}

pub(super) fn is_safe_repository_path(path: &str) -> bool {
    let path = Path::new(path);
    !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_) | Component::CurDir))
}

pub(super) fn is_conflict_status(x: u8, y: u8) -> bool {
    matches!(
        (x, y),
        (b'D', b'D')
            | (b'A', b'U')
            | (b'U', b'D')
            | (b'U', b'A')
            | (b'D', b'U')
            | (b'A', b'A')
            | (b'U', b'U')
    )
}

pub(super) fn status_from_code(code: u8) -> GitReviewFileStatus {
    match code {
        b'A' => GitReviewFileStatus::Added,
        b'D' => GitReviewFileStatus::Deleted,
        b'R' => GitReviewFileStatus::Renamed,
        b'C' => GitReviewFileStatus::Copied,
        b'U' => GitReviewFileStatus::Conflicted,
        _ => GitReviewFileStatus::Modified,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FileStamp {
    exists: bool,
    file_type: u8,
    len: u64,
    modified_ns: u128,
}

pub(super) fn file_stamp(path: &Path) -> FileStamp {
    match fs::symlink_metadata(path) {
        Ok(metadata) => FileStamp {
            exists: true,
            file_type: if metadata.file_type().is_symlink() {
                2
            } else if metadata.is_dir() {
                1
            } else {
                0
            },
            len: metadata.len(),
            modified_ns: metadata
                .modified()
                .ok()
                .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
                .map(|duration| duration.as_nanos())
                .unwrap_or_default(),
        },
        Err(_) => FileStamp {
            exists: false,
            file_type: 0,
            len: 0,
            modified_ns: 0,
        },
    }
}
