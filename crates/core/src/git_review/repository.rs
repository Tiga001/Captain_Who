use super::*;

#[derive(Debug, Clone)]
pub(super) struct RepositoryContext {
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
    let git_dir = PathBuf::from(git_dir_text);
    let identity = format!("{}\0{}", root.display(), git_dir.display());

    Ok(RepositoryContext {
        root,
        git_dir,
        project_prefix: relative_path,
        pathspec,
        repository_id: content_revision(identity.as_bytes()),
    })
}

pub(super) struct ParsedStatusFile {
    pub(super) path: String,
    pub(super) previous_path: Option<String>,
    pub(super) status: GitReviewFileStatus,
}

pub(super) fn parse_porcelain_status(
    output: &[u8],
    scope: GitReviewScope,
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
            scope == GitReviewScope::Unstaged
        } else {
            match scope {
                GitReviewScope::Staged => x != b' ',
                GitReviewScope::Unstaged => y != b' ',
                GitReviewScope::LastTurn => false,
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
            status_from_code(match scope {
                GitReviewScope::Staged => x,
                GitReviewScope::Unstaged => y,
                GitReviewScope::LastTurn => {
                    return Err("Last-turn review does not use Git status.".to_string())
                }
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
    scope: GitReviewScope,
    files: &[ParsedStatusFile],
) -> (GitReviewStats, HashMap<String, GitReviewFileStats>) {
    let mut stats = GitReviewStats {
        file_count: files.len(),
        additions: 0,
        deletions: 0,
        line_counts_complete: true,
    };
    let mut file_stats = HashMap::new();

    match tracked_numstat(repository, scope) {
        Ok(numstat) => {
            stats.additions = numstat.additions;
            stats.deletions = numstat.deletions;
            file_stats = numstat.files;
        }
        Err(()) => stats.line_counts_complete = false,
    }

    if scope == GitReviewScope::Unstaged {
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
    scope: GitReviewScope,
) -> Result<ParsedNumstat, ()> {
    let mut args = vec![
        "diff".to_string(),
        "--no-ext-diff".to_string(),
        "--no-textconv".to_string(),
        "--numstat".to_string(),
        "-z".to_string(),
    ];
    if scope == GitReviewScope::Staged {
        args.push("--cached".to_string());
    }
    args.extend(["--".to_string(), repository.pathspec.clone()]);

    let output = run_git(&repository.root, &args, MAX_NUMSTAT_BYTES).map_err(|_| ())?;
    if !output.status.success() || output.stdout_truncated {
        return Err(());
    }
    parse_numstat(&output.stdout)
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
