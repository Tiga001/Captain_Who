use crate::content_revision;
use serde::Serialize;
use similar::TextDiff;
use std::collections::{HashMap, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant, UNIX_EPOCH};
use uuid::Uuid;

const GIT_TIMEOUT: Duration = Duration::from_secs(12);
const SNAPSHOT_TTL: Duration = Duration::from_secs(5 * 60);
const MAX_SNAPSHOTS: usize = 32;
const MAX_STATUS_BYTES: usize = 4 * 1024 * 1024;
const MAX_NUMSTAT_BYTES: usize = 4 * 1024 * 1024;
const MAX_UNTRACKED_STATS_BYTES: usize = 16 * 1024 * 1024;
const MAX_DIFF_BYTES: usize = 512 * 1024;
const MAX_FULL_CONTENT_SIDE_BYTES: usize = 1024 * 1024;
const MAX_FULL_CONTENT_SIDE_LINES: usize = 100_000;
const MAX_FULL_CONTENT_TOTAL_BYTES: usize = 2 * 1024 * 1024;
const MAX_STDERR_BYTES: usize = 64 * 1024;
const MAX_REVIEW_FILES: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum GitRepositoryInspectionState {
    Ready,
    NotRepository,
    Unsupported,
    Unavailable,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitRepositoryInspection {
    pub project_id: String,
    pub state: GitRepositoryInspectionState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repository_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum GitReviewScope {
    Unstaged,
    Staged,
}

impl GitReviewScope {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "unstaged" => Ok(Self::Unstaged),
            "staged" => Ok(Self::Staged),
            _ => Err("Unsupported Git review scope.".to_string()),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Unstaged => "unstaged",
            Self::Staged => "staged",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum GitReviewFileStatus {
    Modified,
    Added,
    Deleted,
    Renamed,
    Copied,
    Untracked,
    Conflicted,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitReviewFile {
    pub id: String,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_path: Option<String>,
    pub status: GitReviewFileStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stats: Option<GitReviewFileStats>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitReviewFileStats {
    pub additions: u64,
    pub deletions: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitReviewStats {
    pub file_count: usize,
    pub additions: u64,
    pub deletions: u64,
    pub line_counts_complete: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitReviewSummary {
    pub repository_id: String,
    pub snapshot_id: String,
    pub scope: GitReviewScope,
    pub stats: GitReviewStats,
    pub files: Vec<GitReviewFile>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum GitReviewFileDiffStatus {
    Ready,
    Binary,
    TooLarge,
    SnapshotExpired,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitReviewFileDiff {
    pub snapshot_id: String,
    pub file_id: String,
    pub status: GitReviewFileDiffStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub patch: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum GitReviewFileContentStatus {
    Ready,
    Binary,
    TooLarge,
    Unsupported,
    SnapshotExpired,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitReviewFileContent {
    pub snapshot_id: String,
    pub file_id: String,
    pub status: GitReviewFileContentStatus,
    pub before_text: Option<String>,
    pub after_text: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum GitReviewFileMutationAction {
    Stage,
    Unstage,
    Restore,
}

impl GitReviewFileMutationAction {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "stage" => Ok(Self::Stage),
            "unstage" => Ok(Self::Unstage),
            "restore" => Ok(Self::Restore),
            _ => Err("Unsupported Git review file action.".to_string()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum GitReviewFileMutationStatus {
    Applied,
    SnapshotExpired,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitReviewFileMutation {
    pub snapshot_id: String,
    pub file_id: String,
    pub action: GitReviewFileMutationAction,
    pub status: GitReviewFileMutationStatus,
}

pub struct GitReviewService {
    snapshots: Mutex<SnapshotCache>,
}

impl Default for GitReviewService {
    fn default() -> Self {
        Self::new()
    }
}

impl GitReviewService {
    pub fn new() -> Self {
        Self {
            snapshots: Mutex::new(SnapshotCache::default()),
        }
    }

    pub fn inspect_repository(
        &self,
        project_id: &str,
        project_path: &Path,
    ) -> GitRepositoryInspection {
        match resolve_repository(project_path) {
            Ok(repository) => GitRepositoryInspection {
                project_id: project_id.to_string(),
                state: GitRepositoryInspectionState::Ready,
                repository_id: Some(repository.repository_id),
                message: None,
            },
            Err(RepositoryResolutionError::NotRepository) => GitRepositoryInspection {
                project_id: project_id.to_string(),
                state: GitRepositoryInspectionState::NotRepository,
                repository_id: None,
                message: None,
            },
            Err(RepositoryResolutionError::Unsupported(message)) => GitRepositoryInspection {
                project_id: project_id.to_string(),
                state: GitRepositoryInspectionState::Unsupported,
                repository_id: None,
                message: Some(message),
            },
            Err(RepositoryResolutionError::Unavailable(message)) => GitRepositoryInspection {
                project_id: project_id.to_string(),
                state: GitRepositoryInspectionState::Unavailable,
                repository_id: None,
                message: Some(message),
            },
        }
    }

    pub fn review_summary(
        &self,
        project_path: &Path,
        scope: GitReviewScope,
    ) -> Result<GitReviewSummary, String> {
        let repository = resolve_repository(project_path).map_err(|error| error.message())?;
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

        let parsed_files = parse_porcelain_status(&output.stdout, scope)?;
        let (stats, file_stats) = calculate_review_stats(&repository, scope, &parsed_files);
        let truncated = parsed_files.len() > MAX_REVIEW_FILES;
        let files = parsed_files
            .into_iter()
            .take(MAX_REVIEW_FILES)
            .collect::<Vec<_>>();
        let head_oid = read_head_oid(&repository)?;
        let index_stamp = file_stamp(&repository.git_dir.join("index"));
        let snapshot_id = Uuid::new_v4().to_string();
        let mut snapshot_files = HashMap::with_capacity(files.len());
        let public_files = files
            .into_iter()
            .map(|file| {
                let id_material = format!(
                    "{}\0{}\0{}",
                    scope.as_str(),
                    file.path,
                    file.previous_path.as_deref().unwrap_or_default()
                );
                let id = content_revision(id_material.as_bytes());
                let stamp = file_stamp(&repository.root.join(&file.path));
                snapshot_files.insert(
                    id.clone(),
                    SnapshotFile {
                        path: file.path.clone(),
                        previous_path: file.previous_path.clone(),
                        status: file.status,
                        stamp,
                        previous_stamp: file
                            .previous_path
                            .as_ref()
                            .map(|path| file_stamp(&repository.root.join(path))),
                    },
                );
                GitReviewFile {
                    id,
                    stats: file_stats.get(&file.path).cloned(),
                    path: file.path,
                    previous_path: file.previous_path,
                    status: file.status,
                }
            })
            .collect::<Vec<_>>();

        self.snapshots
            .lock()
            .map_err(|_| "Git review snapshot cache is unavailable.".to_string())?
            .insert(Snapshot {
                id: snapshot_id.clone(),
                created_at: Instant::now(),
                repository: repository.clone(),
                scope,
                head_oid,
                index_stamp,
                files: snapshot_files,
            });

        Ok(GitReviewSummary {
            repository_id: repository.repository_id,
            snapshot_id,
            scope,
            stats,
            files: public_files,
            truncated,
        })
    }

    pub fn review_file_diff(
        &self,
        snapshot_id: &str,
        file_id: &str,
    ) -> Result<GitReviewFileDiff, String> {
        let snapshot = self
            .snapshots
            .lock()
            .map_err(|_| "Git review snapshot cache is unavailable.".to_string())?
            .get(snapshot_id);
        let Some(snapshot) = snapshot else {
            return Ok(expired_diff(snapshot_id, file_id));
        };
        let Some(file) = snapshot.files.get(file_id).cloned() else {
            return Ok(expired_diff(snapshot_id, file_id));
        };

        if !snapshot_file_is_current(&snapshot, &file)? {
            return Ok(expired_diff(snapshot_id, file_id));
        }

        if snapshot.scope == GitReviewScope::Unstaged
            && file.status == GitReviewFileStatus::Untracked
        {
            let diff = untracked_file_diff(snapshot_id, file_id, &snapshot.repository, &file.path)?;
            return if snapshot_file_is_current(&snapshot, &file)? {
                Ok(diff)
            } else {
                Ok(expired_diff(snapshot_id, file_id))
            };
        }

        let mut args = vec![
            "diff".to_string(),
            "--no-ext-diff".to_string(),
            "--no-textconv".to_string(),
            "--find-renames".to_string(),
        ];
        if snapshot.scope == GitReviewScope::Staged {
            args.push("--cached".to_string());
        }
        args.extend(["--".to_string(), literal_pathspec(&file.path)]);

        let output = run_git(&snapshot.repository.root, &args, MAX_DIFF_BYTES)?;
        if !output.status.success() {
            return Err(git_failure("Unable to read the Git diff.", &output));
        }
        if !snapshot_file_is_current(&snapshot, &file)? {
            return Ok(expired_diff(snapshot_id, file_id));
        }
        if output.stdout_truncated {
            return Ok(GitReviewFileDiff {
                snapshot_id: snapshot_id.to_string(),
                file_id: file_id.to_string(),
                status: GitReviewFileDiffStatus::TooLarge,
                patch: None,
            });
        }

        let patch = String::from_utf8(output.stdout)
            .map_err(|_| "The Git diff contains unsupported non-UTF-8 output.".to_string())?;
        if patch.contains("GIT binary patch") || patch.contains("Binary files ") {
            return Ok(GitReviewFileDiff {
                snapshot_id: snapshot_id.to_string(),
                file_id: file_id.to_string(),
                status: GitReviewFileDiffStatus::Binary,
                patch: None,
            });
        }
        if patch.trim().is_empty() {
            return Ok(expired_diff(snapshot_id, file_id));
        }

        Ok(GitReviewFileDiff {
            snapshot_id: snapshot_id.to_string(),
            file_id: file_id.to_string(),
            status: GitReviewFileDiffStatus::Ready,
            patch: Some(patch),
        })
    }

    pub fn review_file_content(
        &self,
        snapshot_id: &str,
        file_id: &str,
    ) -> Result<GitReviewFileContent, String> {
        let snapshot = self
            .snapshots
            .lock()
            .map_err(|_| "Git review snapshot cache is unavailable.".to_string())?
            .get(snapshot_id);
        let Some(snapshot) = snapshot else {
            return Ok(expired_content(snapshot_id, file_id));
        };
        let Some(file) = snapshot.files.get(file_id).cloned() else {
            return Ok(expired_content(snapshot_id, file_id));
        };

        if !snapshot_file_is_current(&snapshot, &file)? {
            return Ok(expired_content(snapshot_id, file_id));
        }

        let loaded = load_review_file_content(&snapshot, &file)?;
        if !snapshot_file_is_current(&snapshot, &file)? {
            return Ok(expired_content(snapshot_id, file_id));
        }

        Ok(content_response(snapshot_id, file_id, loaded))
    }

    pub fn mutate_review_file(
        &self,
        snapshot_id: &str,
        file_id: &str,
        action: GitReviewFileMutationAction,
    ) -> Result<GitReviewFileMutation, String> {
        let snapshot = self
            .snapshots
            .lock()
            .map_err(|_| "Git review snapshot cache is unavailable.".to_string())?
            .get(snapshot_id);
        let Some(snapshot) = snapshot else {
            return Ok(expired_mutation(snapshot_id, file_id, action));
        };
        let Some(file) = snapshot.files.get(file_id).cloned() else {
            return Ok(expired_mutation(snapshot_id, file_id, action));
        };

        if !snapshot_file_is_current(&snapshot, &file)? {
            return Ok(expired_mutation(snapshot_id, file_id, action));
        }

        match action {
            GitReviewFileMutationAction::Stage => {
                if snapshot.scope != GitReviewScope::Unstaged {
                    return Err("Only unstaged files can be staged.".to_string());
                }
                ensure_no_external_filter(&snapshot.repository, &file)?;
                if !snapshot_file_is_current(&snapshot, &file)? {
                    return Ok(expired_mutation(snapshot_id, file_id, action));
                }
                run_file_mutation(
                    &snapshot.repository,
                    "Unable to stage the file.",
                    ["add"],
                    &file,
                )?;
            }
            GitReviewFileMutationAction::Unstage => {
                if snapshot.scope != GitReviewScope::Staged {
                    return Err("Only staged files can be unstaged.".to_string());
                }
                unstage_file(&snapshot.repository, &file, !snapshot.head_oid.is_empty())?;
            }
            GitReviewFileMutationAction::Restore => {
                if snapshot.scope != GitReviewScope::Unstaged {
                    return Err("Unstage the file before restoring it.".to_string());
                }
                if file.status == GitReviewFileStatus::Untracked {
                    remove_untracked_file(&snapshot.repository, &file)?;
                } else {
                    ensure_no_external_filter(&snapshot.repository, &file)?;
                    if !snapshot_file_is_current(&snapshot, &file)? {
                        return Ok(expired_mutation(snapshot_id, file_id, action));
                    }
                    run_file_mutation(
                        &snapshot.repository,
                        "Unable to restore the file.",
                        ["restore", "--worktree"],
                        &file,
                    )?;
                }
            }
        }

        Ok(GitReviewFileMutation {
            snapshot_id: snapshot_id.to_string(),
            file_id: file_id.to_string(),
            action,
            status: GitReviewFileMutationStatus::Applied,
        })
    }
}

#[derive(Debug, Clone)]
struct RepositoryContext {
    root: PathBuf,
    git_dir: PathBuf,
    pathspec: String,
    repository_id: String,
}

#[derive(Debug)]
enum RepositoryResolutionError {
    NotRepository,
    Unsupported(String),
    Unavailable(String),
}

impl RepositoryResolutionError {
    fn message(self) -> String {
        match self {
            Self::NotRepository => "The selected project is not a Git repository.".to_string(),
            Self::Unsupported(message) | Self::Unavailable(message) => message,
        }
    }
}

fn resolve_repository(project_path: &Path) -> Result<RepositoryContext, RepositoryResolutionError> {
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
        pathspec,
        repository_id: content_revision(identity.as_bytes()),
    })
}

#[derive(Debug, Clone)]
struct ParsedStatusFile {
    path: String,
    previous_path: Option<String>,
    status: GitReviewFileStatus,
}

fn parse_porcelain_status(
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

fn calculate_review_stats(
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
struct ParsedNumstat {
    additions: u64,
    deletions: u64,
    files: HashMap<String, GitReviewFileStats>,
}

fn tracked_numstat(
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

fn parse_numstat(output: &[u8]) -> Result<ParsedNumstat, ()> {
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

fn untracked_line_count(
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

fn parse_git_path(bytes: &[u8]) -> Result<String, String> {
    let path = std::str::from_utf8(bytes)
        .map_err(|_| "Git paths with non-UTF-8 names are not supported yet.".to_string())?;
    if !is_safe_repository_path(path) {
        return Err("Git returned a path outside the repository.".to_string());
    }
    Ok(path.to_string())
}

fn is_safe_repository_path(path: &str) -> bool {
    let path = Path::new(path);
    !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_) | Component::CurDir))
}

fn is_conflict_status(x: u8, y: u8) -> bool {
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

fn status_from_code(code: u8) -> GitReviewFileStatus {
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
struct FileStamp {
    exists: bool,
    file_type: u8,
    len: u64,
    modified_ns: u128,
}

fn file_stamp(path: &Path) -> FileStamp {
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

#[derive(Debug, Clone)]
struct SnapshotFile {
    path: String,
    previous_path: Option<String>,
    status: GitReviewFileStatus,
    stamp: FileStamp,
    previous_stamp: Option<FileStamp>,
}

#[derive(Debug, Clone)]
struct Snapshot {
    id: String,
    created_at: Instant,
    repository: RepositoryContext,
    scope: GitReviewScope,
    head_oid: String,
    index_stamp: FileStamp,
    files: HashMap<String, SnapshotFile>,
}

#[derive(Default)]
struct SnapshotCache {
    entries: HashMap<String, Snapshot>,
    order: VecDeque<String>,
}

impl SnapshotCache {
    fn insert(&mut self, snapshot: Snapshot) {
        self.prune();
        while self.order.len() >= MAX_SNAPSHOTS {
            if let Some(id) = self.order.pop_front() {
                self.entries.remove(&id);
            }
        }
        self.order.push_back(snapshot.id.clone());
        self.entries.insert(snapshot.id.clone(), snapshot);
    }

    fn get(&mut self, snapshot_id: &str) -> Option<Snapshot> {
        self.prune();
        self.entries.get(snapshot_id).cloned()
    }

    fn prune(&mut self) {
        let expired = self
            .entries
            .iter()
            .filter(|(_, snapshot)| snapshot.created_at.elapsed() > SNAPSHOT_TTL)
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        for id in expired {
            self.entries.remove(&id);
            self.order.retain(|candidate| candidate != &id);
        }
    }
}

fn read_head_oid(repository: &RepositoryContext) -> Result<String, String> {
    let output = run_git(
        &repository.root,
        &[
            "rev-parse".to_string(),
            "--verify".to_string(),
            "HEAD".to_string(),
        ],
        4096,
    )?;
    if output.status.success() {
        String::from_utf8(trim_ascii(&output.stdout).to_vec())
            .map_err(|_| "Git returned an invalid HEAD revision.".to_string())
    } else {
        Ok(String::new())
    }
}

fn snapshot_file_is_current(snapshot: &Snapshot, file: &SnapshotFile) -> Result<bool, String> {
    let previous_path_is_current = match (&file.previous_path, &file.previous_stamp) {
        (Some(path), Some(stamp)) => file_stamp(&snapshot.repository.root.join(path)) == *stamp,
        (None, None) => true,
        _ => false,
    };
    Ok(read_head_oid(&snapshot.repository)? == snapshot.head_oid
        && file_stamp(&snapshot.repository.git_dir.join("index")) == snapshot.index_stamp
        && (snapshot.scope != GitReviewScope::Unstaged
            || (file_stamp(&snapshot.repository.root.join(&file.path)) == file.stamp
                && previous_path_is_current)))
}

#[derive(Debug)]
enum LoadedReviewContent {
    Ready {
        before_text: Option<String>,
        after_text: Option<String>,
    },
    Binary,
    TooLarge,
    Unsupported,
}

#[derive(Debug, Clone, Copy)]
enum SideRequirement {
    Absent,
    Optional,
    Required,
}

#[derive(Debug)]
enum SideContent {
    Missing,
    Bytes(Vec<u8>),
    TooLarge,
    Unsupported,
}

#[derive(Debug)]
enum RepositoryEntry {
    Blob(String),
    Gitlink,
    Unsupported,
}

fn load_review_file_content(
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
    let before_requirement = match (snapshot.scope, file.status) {
        (_, GitReviewFileStatus::Untracked)
        | (GitReviewScope::Staged, GitReviewFileStatus::Added) => SideRequirement::Absent,
        (GitReviewScope::Unstaged, GitReviewFileStatus::Added) => SideRequirement::Optional,
        _ => SideRequirement::Required,
    };
    let after_requirement = if file.status == GitReviewFileStatus::Deleted {
        SideRequirement::Absent
    } else {
        SideRequirement::Required
    };

    // Git's review boundary is HEAD -> index for staged changes and index -> worktree for
    // unstaged changes. Object sides are read by validated OID; the worktree side is read without
    // following repository-controlled symlinks.
    let before = match (snapshot.scope, before_requirement) {
        (_, SideRequirement::Absent) => SideContent::Missing,
        (GitReviewScope::Staged, requirement) => {
            let entry = if snapshot.head_oid.is_empty() {
                None
            } else {
                head_entry(&snapshot.repository, &snapshot.head_oid, before_path)?
            };
            load_repository_entry(&snapshot.repository, entry, requirement)?
        }
        (GitReviewScope::Unstaged, requirement) => {
            let entry = index_entry(&snapshot.repository, before_path)?;
            load_repository_entry(&snapshot.repository, entry, requirement)?
        }
    };
    let after = match (snapshot.scope, after_requirement) {
        (_, SideRequirement::Absent) => SideContent::Missing,
        (GitReviewScope::Staged, requirement) => {
            let entry = index_entry(&snapshot.repository, &file.path)?;
            load_repository_entry(&snapshot.repository, entry, requirement)?
        }
        (GitReviewScope::Unstaged, _) => read_worktree_content(&snapshot.repository, &file.path),
    };

    Ok(normalize_review_content(before, after))
}

fn load_repository_entry(
    repository: &RepositoryContext,
    entry: Option<RepositoryEntry>,
    requirement: SideRequirement,
) -> Result<SideContent, String> {
    match entry {
        Some(RepositoryEntry::Blob(oid)) => read_blob_content(repository, &oid),
        Some(RepositoryEntry::Gitlink | RepositoryEntry::Unsupported) => {
            Ok(SideContent::Unsupported)
        }
        None if matches!(requirement, SideRequirement::Optional) => Ok(SideContent::Missing),
        None => Ok(SideContent::Unsupported),
    }
}

fn head_entry(
    repository: &RepositoryContext,
    head_oid: &str,
    path: &str,
) -> Result<Option<RepositoryEntry>, String> {
    let args = vec![
        "ls-tree".to_string(),
        "-z".to_string(),
        "--full-tree".to_string(),
        head_oid.to_string(),
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

fn parse_tree_entry(output: &[u8], expected_path: &str) -> Result<Option<RepositoryEntry>, String> {
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

fn index_entry(
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

fn parse_index_entry(
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

fn classify_repository_entry(mode: &str, kind: Option<&str>, oid: &str) -> RepositoryEntry {
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

fn is_full_object_id(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn read_blob_content(repository: &RepositoryContext, oid: &str) -> Result<SideContent, String> {
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

fn read_worktree_content(repository: &RepositoryContext, path: &str) -> SideContent {
    if !is_safe_repository_path(path) {
        return SideContent::Unsupported;
    }
    read_worktree_content_platform(&repository.root, path).unwrap_or(SideContent::Unsupported)
}

#[cfg(unix)]
fn read_worktree_content_platform(
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
fn read_worktree_content_platform(
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

fn normalize_review_content(before: SideContent, after: SideContent) -> LoadedReviewContent {
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

fn review_line_count(bytes: &[u8]) -> usize {
    bytes.iter().filter(|byte| **byte == b'\n').count()
        + usize::from(!bytes.is_empty() && !bytes.ends_with(b"\n"))
}

fn content_response(
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

fn expired_content(snapshot_id: &str, file_id: &str) -> GitReviewFileContent {
    GitReviewFileContent {
        snapshot_id: snapshot_id.to_string(),
        file_id: file_id.to_string(),
        status: GitReviewFileContentStatus::SnapshotExpired,
        before_text: None,
        after_text: None,
    }
}

fn expired_diff(snapshot_id: &str, file_id: &str) -> GitReviewFileDiff {
    GitReviewFileDiff {
        snapshot_id: snapshot_id.to_string(),
        file_id: file_id.to_string(),
        status: GitReviewFileDiffStatus::SnapshotExpired,
        patch: None,
    }
}

fn expired_mutation(
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

fn mutation_paths(file: &SnapshotFile) -> Vec<String> {
    let mut paths = vec![literal_pathspec(&file.path)];
    if let Some(previous_path) = &file.previous_path {
        paths.push(literal_pathspec(previous_path));
    }
    paths
}

fn run_file_mutation<const N: usize>(
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

fn unstage_file(
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

fn ensure_no_external_filter(
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

fn remove_untracked_file(
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

fn untracked_file_diff(
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

    Ok(GitReviewFileDiff {
        snapshot_id: snapshot_id.to_string(),
        file_id: file_id.to_string(),
        status: GitReviewFileDiffStatus::Ready,
        patch: Some(patch),
    })
}

#[cfg(unix)]
fn open_untracked_file(path: &Path) -> std::io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;

    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
}

#[cfg(not(unix))]
fn open_untracked_file(path: &Path) -> std::io::Result<File> {
    OpenOptions::new().read(true).open(path)
}

struct GitOutput {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    stdout_truncated: bool,
}

fn run_git(cwd: &Path, args: &[String], stdout_limit: usize) -> Result<GitOutput, String> {
    let mut command = Command::new("git");
    command
        .arg("--no-pager")
        .arg("-c")
        .arg("core.fsmonitor=false")
        .arg("-c")
        .arg(if cfg!(windows) {
            "core.hooksPath=NUL"
        } else {
            "core.hooksPath=/dev/null"
        })
        .arg("-C")
        .arg(cwd)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("LC_ALL", "C")
        .env("LANG", "C")
        .env("LANGUAGE", "C")
        .env("GIT_TERMINAL_PROMPT", "0")
        // A repository-controlled replacement ref must not redirect a validated object ID.
        .env("GIT_NO_REPLACE_OBJECTS", "1")
        .env("GIT_PAGER", "cat")
        .env("PAGER", "cat")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env_remove("GIT_DIR")
        .env_remove("GIT_COMMON_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_OBJECT_DIRECTORY")
        .env_remove("GIT_ALTERNATE_OBJECT_DIRECTORIES")
        .env_remove("GIT_REPLACE_REF_BASE")
        .env_remove("GIT_CEILING_DIRECTORIES")
        .env_remove("GIT_DISCOVERY_ACROSS_FILESYSTEM")
        .env_remove("GIT_EXTERNAL_DIFF")
        .env_remove("GIT_DIFF_OPTS")
        .env_remove("GIT_CONFIG_PARAMETERS")
        .env_remove("GIT_CONFIG_SYSTEM")
        .env_remove("GIT_CONFIG_GLOBAL")
        .env_remove("GIT_CONFIG_NOSYSTEM")
        .env_remove("GIT_CONFIG_COUNT");

    for (key, _) in std::env::vars_os() {
        let key_text = key.to_string_lossy();
        if key_text.starts_with("GIT_CONFIG_KEY_") || key_text.starts_with("GIT_CONFIG_VALUE_") {
            command.env_remove(key);
        }
    }

    let mut child = command
        .spawn()
        .map_err(|error| format!("Unable to start Git: {error}"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "Git stdout was not captured.".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "Git stderr was not captured.".to_string())?;
    let stdout_reader = thread::spawn(move || read_bounded(stdout, stdout_limit));
    let stderr_reader = thread::spawn(move || read_bounded(stderr, MAX_STDERR_BYTES));
    let deadline = Instant::now() + GIT_TIMEOUT;

    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err("Git operation timed out.".to_string());
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(format!("Unable to wait for Git: {error}"));
            }
        }
    };
    let (stdout, stdout_truncated) = stdout_reader
        .join()
        .map_err(|_| "Git stdout reader failed.".to_string())??;
    let (stderr, _) = stderr_reader
        .join()
        .map_err(|_| "Git stderr reader failed.".to_string())??;

    Ok(GitOutput {
        status,
        stdout,
        stderr,
        stdout_truncated,
    })
}

fn read_bounded<R: Read>(mut reader: R, limit: usize) -> Result<(Vec<u8>, bool), String> {
    let mut output = Vec::with_capacity(limit.min(64 * 1024));
    let mut buffer = [0_u8; 16 * 1024];
    let mut truncated = false;
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|error| format!("Unable to read Git output: {error}"))?;
        if read == 0 {
            break;
        }
        let remaining = limit.saturating_sub(output.len());
        let retained = remaining.min(read);
        output.extend_from_slice(&buffer[..retained]);
        truncated |= retained < read;
    }
    Ok((output, truncated))
}

fn git_failure(fallback: &str, output: &GitOutput) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let message = stderr.lines().next().unwrap_or_default().trim();
    if message.is_empty() {
        fallback.to_string()
    } else {
        format!("{fallback} {message}")
    }
}

fn trim_ascii(bytes: &[u8]) -> &[u8] {
    let start = bytes
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .unwrap_or(bytes.len());
    let end = bytes
        .iter()
        .rposition(|byte| !byte.is_ascii_whitespace())
        .map(|index| index + 1)
        .unwrap_or(start);
    &bytes[start..end]
}

fn path_to_git_string(path: &Path) -> Option<String> {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => parts.push(part.to_str()?.to_string()),
            Component::CurDir => {}
            _ => return None,
        }
    }
    Some(parts.join("/"))
}

fn literal_pathspec(path: &str) -> String {
    format!(":(top,literal){path}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use tempfile::TempDir;

    #[test]
    fn parses_staged_unstaged_and_untracked_entries() {
        let output = b"M  staged.txt\0 M unstaged.txt\0?? new.txt\0";
        let staged = parse_porcelain_status(output, GitReviewScope::Staged).unwrap();
        let unstaged = parse_porcelain_status(output, GitReviewScope::Unstaged).unwrap();

        assert_eq!(staged.len(), 1);
        assert_eq!(staged[0].path, "staged.txt");
        assert_eq!(unstaged.len(), 2);
        assert_eq!(unstaged[0].path, "new.txt");
        assert_eq!(unstaged[0].status, GitReviewFileStatus::Untracked);
    }

    #[test]
    fn parses_rename_source_from_nul_entry() {
        let output = b"R  renamed.txt\0original.txt\0";
        let files = parse_porcelain_status(output, GitReviewScope::Staged).unwrap();

        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "renamed.txt");
        assert_eq!(files[0].previous_path.as_deref(), Some("original.txt"));
        assert_eq!(files[0].status, GitReviewFileStatus::Renamed);
    }

    #[test]
    fn parses_text_binary_and_rename_numstat() {
        let output = [
            b"12\t3\tsrc/main.rs\0-\t-\timage.png\0".as_slice(),
            b"5\t4\tdeleted.txt\0".as_slice(),
            b"7\t2\t\0old.rs\0new.rs\0".as_slice(),
        ]
        .concat();

        let parsed = parse_numstat(&output).unwrap();
        assert_eq!(parsed.additions, 24);
        assert_eq!(parsed.deletions, 9);
        assert_eq!(
            parsed.files.get("src/main.rs"),
            Some(&GitReviewFileStats {
                additions: 12,
                deletions: 3,
            })
        );
        assert_eq!(
            parsed.files.get("new.rs"),
            Some(&GitReviewFileStats {
                additions: 7,
                deletions: 2,
            })
        );
        assert!(!parsed.files.contains_key("old.rs"));
        assert!(!parsed.files.contains_key("image.png"));
    }

    #[test]
    fn full_content_response_serializes_nullable_sides_and_camel_case_status() {
        let value = serde_json::to_value(expired_content("snapshot", "file")).unwrap();

        assert_eq!(value["snapshotId"], "snapshot");
        assert_eq!(value["fileId"], "file");
        assert_eq!(value["status"], "snapshotExpired");
        assert!(value["beforeText"].is_null());
        assert!(value["afterText"].is_null());
    }

    #[test]
    fn staged_and_untracked_diffs_are_loaded_from_snapshots() {
        let Some(repo) = test_repository() else {
            return;
        };
        fs::write(repo.path().join("staged.txt"), "staged\n").unwrap();
        git(repo.path(), &["add", "staged.txt"]);
        fs::write(repo.path().join("untracked.txt"), "untracked\n").unwrap();
        let service = GitReviewService::new();

        let staged = service
            .review_summary(repo.path(), GitReviewScope::Staged)
            .unwrap();
        assert_eq!(staged.stats.file_count, 1);
        assert_eq!(staged.stats.additions, 1);
        assert_eq!(staged.stats.deletions, 0);
        assert!(staged.stats.line_counts_complete);
        assert_eq!(
            staged.files[0].stats,
            Some(GitReviewFileStats {
                additions: 1,
                deletions: 0,
            })
        );
        let staged_diff = service
            .review_file_diff(&staged.snapshot_id, &staged.files[0].id)
            .unwrap();
        assert_eq!(staged_diff.status, GitReviewFileDiffStatus::Ready);
        assert!(staged_diff.patch.unwrap().contains("+staged"));

        let unstaged = service
            .review_summary(repo.path(), GitReviewScope::Unstaged)
            .unwrap();
        assert_eq!(unstaged.stats.file_count, 1);
        assert_eq!(unstaged.stats.additions, 1);
        assert_eq!(unstaged.stats.deletions, 0);
        assert!(unstaged.stats.line_counts_complete);
        let untracked = unstaged
            .files
            .iter()
            .find(|file| file.path == "untracked.txt")
            .unwrap();
        assert_eq!(
            untracked.stats,
            Some(GitReviewFileStats {
                additions: 1,
                deletions: 0,
            })
        );
        let untracked_diff = service
            .review_file_diff(&unstaged.snapshot_id, &untracked.id)
            .unwrap();
        assert_eq!(untracked_diff.status, GitReviewFileDiffStatus::Ready);
        assert!(untracked_diff.patch.unwrap().contains("+untracked"));
    }

    #[test]
    fn full_content_uses_head_index_and_worktree_for_the_selected_scope() {
        let Some(repo) = test_repository() else {
            return;
        };
        fs::write(repo.path().join("tracked.txt"), "head\nshared\n").unwrap();
        git(repo.path(), &["add", "tracked.txt"]);
        git(repo.path(), &["commit", "--quiet", "-m", "base"]);
        fs::write(repo.path().join("tracked.txt"), "index\nshared\n").unwrap();
        git(repo.path(), &["add", "tracked.txt"]);
        fs::write(repo.path().join("tracked.txt"), "worktree\nshared\n").unwrap();
        let service = GitReviewService::new();

        let staged = service
            .review_summary(repo.path(), GitReviewScope::Staged)
            .unwrap();
        let staged_content = service
            .review_file_content(&staged.snapshot_id, &staged.files[0].id)
            .unwrap();
        assert_eq!(staged_content.status, GitReviewFileContentStatus::Ready);
        assert_eq!(
            staged_content.before_text.as_deref(),
            Some("head\nshared\n")
        );
        assert_eq!(
            staged_content.after_text.as_deref(),
            Some("index\nshared\n")
        );

        let unstaged = service
            .review_summary(repo.path(), GitReviewScope::Unstaged)
            .unwrap();
        let unstaged_content = service
            .review_file_content(&unstaged.snapshot_id, &unstaged.files[0].id)
            .unwrap();
        assert_eq!(unstaged_content.status, GitReviewFileContentStatus::Ready);
        assert_eq!(
            unstaged_content.before_text.as_deref(),
            Some("index\nshared\n")
        );
        assert_eq!(
            unstaged_content.after_text.as_deref(),
            Some("worktree\nshared\n")
        );
    }

    #[test]
    fn full_content_represents_staged_additions_and_deletions_with_a_missing_side() {
        let Some(repo) = test_repository() else {
            return;
        };
        fs::write(repo.path().join("deleted.txt"), "deleted content\n").unwrap();
        git(repo.path(), &["add", "deleted.txt"]);
        git(repo.path(), &["commit", "--quiet", "-m", "base"]);
        fs::remove_file(repo.path().join("deleted.txt")).unwrap();
        fs::write(repo.path().join("added.txt"), "added content\n").unwrap();
        git(repo.path(), &["add", "--all"]);
        let service = GitReviewService::new();
        let summary = service
            .review_summary(repo.path(), GitReviewScope::Staged)
            .unwrap();

        let added = summary
            .files
            .iter()
            .find(|file| file.path == "added.txt")
            .unwrap();
        let added_content = service
            .review_file_content(&summary.snapshot_id, &added.id)
            .unwrap();
        assert_eq!(added_content.status, GitReviewFileContentStatus::Ready);
        assert_eq!(added_content.before_text, None);
        assert_eq!(added_content.after_text.as_deref(), Some("added content\n"));

        let deleted = summary
            .files
            .iter()
            .find(|file| file.path == "deleted.txt")
            .unwrap();
        let deleted_content = service
            .review_file_content(&summary.snapshot_id, &deleted.id)
            .unwrap();
        assert_eq!(deleted_content.status, GitReviewFileContentStatus::Ready);
        assert_eq!(
            deleted_content.before_text.as_deref(),
            Some("deleted content\n")
        );
        assert_eq!(deleted_content.after_text, None);
    }

    #[test]
    fn full_content_follows_staged_rename_paths_without_guessing_from_the_worktree() {
        let Some(repo) = test_repository() else {
            return;
        };
        fs::write(repo.path().join("before.txt"), "before\nshared\n").unwrap();
        git(repo.path(), &["add", "before.txt"]);
        git(repo.path(), &["commit", "--quiet", "-m", "base"]);
        git(repo.path(), &["mv", "before.txt", "after.txt"]);
        fs::write(repo.path().join("after.txt"), "after\nshared\n").unwrap();
        git(repo.path(), &["add", "after.txt"]);
        let service = GitReviewService::new();
        let summary = service
            .review_summary(repo.path(), GitReviewScope::Staged)
            .unwrap();
        let renamed = summary
            .files
            .iter()
            .find(|file| file.path == "after.txt")
            .unwrap();

        assert_eq!(renamed.previous_path.as_deref(), Some("before.txt"));
        let content = service
            .review_file_content(&summary.snapshot_id, &renamed.id)
            .unwrap();
        assert_eq!(content.status, GitReviewFileContentStatus::Ready);
        assert_eq!(content.before_text.as_deref(), Some("before\nshared\n"));
        assert_eq!(content.after_text.as_deref(), Some("after\nshared\n"));
    }

    #[test]
    fn full_content_rejects_gitlinks() {
        let Some(repo) = test_repository() else {
            return;
        };
        fs::write(repo.path().join("base.txt"), "base\n").unwrap();
        git(repo.path(), &["add", "base.txt"]);
        git(repo.path(), &["commit", "--quiet", "-m", "base"]);
        let head = git_stdout(repo.path(), &["rev-parse", "HEAD"]);
        git(
            repo.path(),
            &[
                "update-index",
                "--add",
                "--cacheinfo",
                &format!("160000,{head},module"),
            ],
        );
        let service = GitReviewService::new();
        let summary = service
            .review_summary(repo.path(), GitReviewScope::Staged)
            .unwrap();
        let gitlink = summary
            .files
            .iter()
            .find(|file| file.path == "module")
            .unwrap();

        let content = service
            .review_file_content(&summary.snapshot_id, &gitlink.id)
            .unwrap();
        assert_eq!(content.status, GitReviewFileContentStatus::Unsupported);
        assert_eq!(content.before_text, None);
        assert_eq!(content.after_text, None);
    }

    #[test]
    fn full_content_safely_classifies_binary_non_utf8_and_oversized_files() {
        let Some(repo) = test_repository() else {
            return;
        };
        fs::write(repo.path().join("tracked.bin"), "base\n").unwrap();
        git(repo.path(), &["add", "tracked.bin"]);
        git(repo.path(), &["commit", "--quiet", "-m", "base"]);

        fs::write(repo.path().join("tracked.bin"), b"binary\0content").unwrap();
        let service = GitReviewService::new();
        let summary = service
            .review_summary(repo.path(), GitReviewScope::Unstaged)
            .unwrap();
        let content = service
            .review_file_content(&summary.snapshot_id, &summary.files[0].id)
            .unwrap();
        assert_eq!(content.status, GitReviewFileContentStatus::Binary);
        assert_eq!(content.before_text, None);
        assert_eq!(content.after_text, None);

        fs::write(repo.path().join("tracked.bin"), [0xff, 0xfe]).unwrap();
        let summary = service
            .review_summary(repo.path(), GitReviewScope::Unstaged)
            .unwrap();
        let content = service
            .review_file_content(&summary.snapshot_id, &summary.files[0].id)
            .unwrap();
        assert_eq!(content.status, GitReviewFileContentStatus::Unsupported);

        fs::write(
            repo.path().join("tracked.bin"),
            vec![b'\n'; MAX_FULL_CONTENT_SIDE_LINES + 1],
        )
        .unwrap();
        let summary = service
            .review_summary(repo.path(), GitReviewScope::Unstaged)
            .unwrap();
        let content = service
            .review_file_content(&summary.snapshot_id, &summary.files[0].id)
            .unwrap();
        assert_eq!(content.status, GitReviewFileContentStatus::TooLarge);

        fs::write(
            repo.path().join("tracked.bin"),
            vec![b'x'; MAX_FULL_CONTENT_SIDE_BYTES + 1],
        )
        .unwrap();
        let summary = service
            .review_summary(repo.path(), GitReviewScope::Unstaged)
            .unwrap();
        let content = service
            .review_file_content(&summary.snapshot_id, &summary.files[0].id)
            .unwrap();
        assert_eq!(content.status, GitReviewFileContentStatus::TooLarge);
    }

    #[test]
    fn changing_a_file_expires_a_full_content_request() {
        let Some(repo) = test_repository() else {
            return;
        };
        fs::write(repo.path().join("tracked.txt"), "base\n").unwrap();
        git(repo.path(), &["add", "tracked.txt"]);
        git(repo.path(), &["commit", "--quiet", "-m", "base"]);
        fs::write(repo.path().join("tracked.txt"), "first change\n").unwrap();
        let service = GitReviewService::new();
        let summary = service
            .review_summary(repo.path(), GitReviewScope::Unstaged)
            .unwrap();

        thread::sleep(Duration::from_millis(5));
        fs::write(
            repo.path().join("tracked.txt"),
            "second change with a different size\n",
        )
        .unwrap();
        let content = service
            .review_file_content(&summary.snapshot_id, &summary.files[0].id)
            .unwrap();
        assert_eq!(content.status, GitReviewFileContentStatus::SnapshotExpired);
        assert_eq!(content.before_text, None);
        assert_eq!(content.after_text, None);
    }

    #[test]
    fn unstaged_totals_keep_tracked_and_untracked_line_counts() {
        let Some(repo) = test_repository() else {
            return;
        };
        fs::write(repo.path().join("tracked.txt"), "base\n").unwrap();
        git(repo.path(), &["add", "tracked.txt"]);
        fs::write(repo.path().join("tracked.txt"), "base\nchanged\n").unwrap();
        fs::write(repo.path().join("untracked.txt"), "first\nsecond\n").unwrap();

        let summary = GitReviewService::new()
            .review_summary(repo.path(), GitReviewScope::Unstaged)
            .unwrap();

        assert_eq!(summary.stats.file_count, 2);
        assert_eq!(summary.stats.additions, 3);
        assert_eq!(summary.stats.deletions, 0);
        assert!(summary.stats.line_counts_complete);
        assert_eq!(
            summary
                .files
                .iter()
                .find(|file| file.path == "tracked.txt")
                .and_then(|file| file.stats.clone()),
            Some(GitReviewFileStats {
                additions: 1,
                deletions: 0,
            })
        );
        assert_eq!(
            summary
                .files
                .iter()
                .find(|file| file.path == "untracked.txt")
                .and_then(|file| file.stats.clone()),
            Some(GitReviewFileStats {
                additions: 2,
                deletions: 0,
            })
        );
    }

    #[test]
    fn changing_a_file_expires_an_unstaged_snapshot() {
        let Some(repo) = test_repository() else {
            return;
        };
        fs::write(repo.path().join("tracked.txt"), "base\n").unwrap();
        git(repo.path(), &["add", "tracked.txt"]);
        fs::write(repo.path().join("tracked.txt"), "first change\n").unwrap();
        let service = GitReviewService::new();
        let summary = service
            .review_summary(repo.path(), GitReviewScope::Unstaged)
            .unwrap();
        let file = summary.files.first().unwrap();

        thread::sleep(Duration::from_millis(5));
        fs::write(
            repo.path().join("tracked.txt"),
            "second change with new size\n",
        )
        .unwrap();
        let diff = service
            .review_file_diff(&summary.snapshot_id, &file.id)
            .unwrap();
        assert_eq!(diff.status, GitReviewFileDiffStatus::SnapshotExpired);
    }

    #[test]
    fn project_subdirectory_limits_the_review_scope() {
        let Some(repo) = test_repository() else {
            return;
        };
        fs::create_dir(repo.path().join("selected")).unwrap();
        fs::write(repo.path().join("selected/in-scope.txt"), "inside\n").unwrap();
        fs::write(repo.path().join("out-of-scope.txt"), "outside\n").unwrap();
        git(repo.path(), &["add", "."]);

        let summary = GitReviewService::new()
            .review_summary(&repo.path().join("selected"), GitReviewScope::Staged)
            .unwrap();

        assert_eq!(summary.files.len(), 1);
        assert_eq!(summary.files[0].path, "selected/in-scope.txt");
    }

    #[test]
    fn rejects_repository_escape_paths() {
        assert!(parse_git_path(b"../outside.txt").is_err());
        assert!(parse_git_path(b"/absolute.txt").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn untracked_symlink_diff_does_not_follow_the_target() {
        use std::os::unix::fs::symlink;

        let Some(repo) = test_repository() else {
            return;
        };
        let outside = TempDir::new().unwrap();
        fs::write(outside.path().join("secret.txt"), "must-not-be-read\n").unwrap();
        symlink(
            outside.path().join("secret.txt"),
            repo.path().join("link.txt"),
        )
        .unwrap();
        let service = GitReviewService::new();
        let summary = service
            .review_summary(repo.path(), GitReviewScope::Unstaged)
            .unwrap();
        let link = summary
            .files
            .iter()
            .find(|file| file.path == "link.txt")
            .unwrap();

        let diff = service
            .review_file_diff(&summary.snapshot_id, &link.id)
            .unwrap();
        let patch = diff.patch.unwrap();

        assert!(patch.contains("secret.txt"));
        assert!(!patch.contains("must-not-be-read"));
    }

    #[cfg(unix)]
    #[test]
    fn full_content_reads_an_untracked_symlink_itself_without_following_it() {
        use std::os::unix::fs::symlink;

        let Some(repo) = test_repository() else {
            return;
        };
        symlink("missing-secret-target", repo.path().join("link.txt")).unwrap();
        let service = GitReviewService::new();
        let summary = service
            .review_summary(repo.path(), GitReviewScope::Unstaged)
            .unwrap();
        let link = summary
            .files
            .iter()
            .find(|file| file.path == "link.txt")
            .unwrap();

        let content = service
            .review_file_content(&summary.snapshot_id, &link.id)
            .unwrap();
        assert_eq!(content.status, GitReviewFileContentStatus::Ready);
        assert_eq!(content.before_text, None);
        assert_eq!(content.after_text.as_deref(), Some("missing-secret-target"));
    }

    #[cfg(unix)]
    #[test]
    fn configured_external_diff_is_not_executed() {
        use std::os::unix::fs::PermissionsExt;

        let Some(repo) = test_repository() else {
            return;
        };
        let marker = repo.path().join("external-diff-ran");
        let helper = repo.path().join("external-diff.sh");
        fs::write(
            &helper,
            format!("#!/bin/sh\ntouch '{}'\n", marker.display()),
        )
        .unwrap();
        let mut permissions = fs::metadata(&helper).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&helper, permissions).unwrap();
        fs::write(repo.path().join(".gitattributes"), "*.txt diff=unsafe\n").unwrap();
        fs::write(repo.path().join("tracked.txt"), "base\n").unwrap();
        git(repo.path(), &["add", ".gitattributes", "tracked.txt"]);
        fs::write(repo.path().join("tracked.txt"), "changed\n").unwrap();
        git(
            repo.path(),
            &["config", "diff.unsafe.command", helper.to_str().unwrap()],
        );
        let service = GitReviewService::new();
        let summary = service
            .review_summary(repo.path(), GitReviewScope::Unstaged)
            .unwrap();
        let tracked = summary
            .files
            .iter()
            .find(|file| file.path == "tracked.txt")
            .unwrap();

        let diff = service
            .review_file_diff(&summary.snapshot_id, &tracked.id)
            .unwrap();

        assert_eq!(diff.status, GitReviewFileDiffStatus::Ready);
        assert!(!marker.exists());
    }

    #[test]
    fn stages_and_unstages_a_current_snapshot_file() {
        let Some(repo) = test_repository() else {
            return;
        };
        fs::write(repo.path().join("tracked.txt"), "base\n").unwrap();
        git(repo.path(), &["add", "tracked.txt"]);
        git(repo.path(), &["commit", "--quiet", "-m", "base"]);
        fs::write(repo.path().join("tracked.txt"), "changed\n").unwrap();
        let service = GitReviewService::new();

        let unstaged = service
            .review_summary(repo.path(), GitReviewScope::Unstaged)
            .unwrap();
        let mutation = service
            .mutate_review_file(
                &unstaged.snapshot_id,
                &unstaged.files[0].id,
                GitReviewFileMutationAction::Stage,
            )
            .unwrap();
        assert_eq!(mutation.status, GitReviewFileMutationStatus::Applied);

        let staged = service
            .review_summary(repo.path(), GitReviewScope::Staged)
            .unwrap();
        assert_eq!(staged.files.len(), 1);
        let mutation = service
            .mutate_review_file(
                &staged.snapshot_id,
                &staged.files[0].id,
                GitReviewFileMutationAction::Unstage,
            )
            .unwrap();
        assert_eq!(mutation.status, GitReviewFileMutationStatus::Applied);
        assert_eq!(
            service
                .review_summary(repo.path(), GitReviewScope::Unstaged)
                .unwrap()
                .files
                .len(),
            1
        );
    }

    #[test]
    fn stale_snapshot_does_not_stage_a_changed_file() {
        let Some(repo) = test_repository() else {
            return;
        };
        fs::write(repo.path().join("tracked.txt"), "base\n").unwrap();
        git(repo.path(), &["add", "tracked.txt"]);
        git(repo.path(), &["commit", "--quiet", "-m", "base"]);
        fs::write(repo.path().join("tracked.txt"), "first change\n").unwrap();
        let service = GitReviewService::new();
        let summary = service
            .review_summary(repo.path(), GitReviewScope::Unstaged)
            .unwrap();

        thread::sleep(Duration::from_millis(5));
        fs::write(
            repo.path().join("tracked.txt"),
            "a newer and longer change\n",
        )
        .unwrap();
        let mutation = service
            .mutate_review_file(
                &summary.snapshot_id,
                &summary.files[0].id,
                GitReviewFileMutationAction::Stage,
            )
            .unwrap();

        assert_eq!(
            mutation.status,
            GitReviewFileMutationStatus::SnapshotExpired
        );
        assert!(service
            .review_summary(repo.path(), GitReviewScope::Staged)
            .unwrap()
            .files
            .is_empty());
    }

    #[test]
    fn restores_tracked_and_removes_untracked_files() {
        let Some(repo) = test_repository() else {
            return;
        };
        fs::write(repo.path().join("tracked.txt"), "base\n").unwrap();
        git(repo.path(), &["add", "tracked.txt"]);
        git(repo.path(), &["commit", "--quiet", "-m", "base"]);
        fs::write(repo.path().join("tracked.txt"), "changed\n").unwrap();
        fs::write(repo.path().join("untracked.txt"), "new\n").unwrap();
        let service = GitReviewService::new();
        let summary = service
            .review_summary(repo.path(), GitReviewScope::Unstaged)
            .unwrap();
        let tracked = summary
            .files
            .iter()
            .find(|file| file.path == "tracked.txt")
            .unwrap();
        service
            .mutate_review_file(
                &summary.snapshot_id,
                &tracked.id,
                GitReviewFileMutationAction::Restore,
            )
            .unwrap();
        assert_eq!(
            fs::read_to_string(repo.path().join("tracked.txt")).unwrap(),
            "base\n"
        );

        let summary = service
            .review_summary(repo.path(), GitReviewScope::Unstaged)
            .unwrap();
        let untracked = summary
            .files
            .iter()
            .find(|file| file.path == "untracked.txt")
            .unwrap();
        service
            .mutate_review_file(
                &summary.snapshot_id,
                &untracked.id,
                GitReviewFileMutationAction::Restore,
            )
            .unwrap();
        assert!(!repo.path().join("untracked.txt").exists());
    }

    #[cfg(unix)]
    #[test]
    fn repository_filters_and_hooks_are_not_executed_by_stage() {
        use std::os::unix::fs::PermissionsExt;

        let Some(repo) = test_repository() else {
            return;
        };
        let filter_marker = repo.path().join("filter-ran");
        let hook_marker = repo.path().join("hook-ran");
        let helper = repo.path().join("filter.sh");
        fs::write(
            &helper,
            format!("#!/bin/sh\ntouch '{}'\ncat\n", filter_marker.display()),
        )
        .unwrap();
        let mut permissions = fs::metadata(&helper).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&helper, permissions).unwrap();
        fs::write(repo.path().join(".gitattributes"), "*.txt filter=unsafe\n").unwrap();
        git(repo.path(), &["add", ".gitattributes"]);
        git(repo.path(), &["commit", "--quiet", "-m", "attributes"]);
        git(
            repo.path(),
            &["config", "filter.unsafe.clean", helper.to_str().unwrap()],
        );
        let hook = repo.path().join(".git/hooks/post-index-change");
        fs::write(
            &hook,
            format!("#!/bin/sh\ntouch '{}'\n", hook_marker.display()),
        )
        .unwrap();
        let mut permissions = fs::metadata(&hook).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&hook, permissions).unwrap();
        fs::write(repo.path().join("filtered.txt"), "content\n").unwrap();
        fs::write(repo.path().join("safe.md"), "content\n").unwrap();
        let service = GitReviewService::new();
        let summary = service
            .review_summary(repo.path(), GitReviewScope::Unstaged)
            .unwrap();
        let filtered = summary
            .files
            .iter()
            .find(|file| file.path == "filtered.txt")
            .unwrap();

        let result = service.mutate_review_file(
            &summary.snapshot_id,
            &filtered.id,
            GitReviewFileMutationAction::Stage,
        );

        assert!(result.is_err());
        assert!(!filter_marker.exists());
        let safe = summary
            .files
            .iter()
            .find(|file| file.path == "safe.md")
            .unwrap();
        let result = service
            .mutate_review_file(
                &summary.snapshot_id,
                &safe.id,
                GitReviewFileMutationAction::Stage,
            )
            .unwrap();
        assert_eq!(result.status, GitReviewFileMutationStatus::Applied);
        assert!(!hook_marker.exists());
    }

    fn test_repository() -> Option<TempDir> {
        if Command::new("git").arg("--version").output().is_err() {
            return None;
        }
        let repo = TempDir::new().unwrap();
        git(repo.path(), &["init", "--quiet"]);
        git(repo.path(), &["config", "user.name", "MyCopilot Test"]);
        git(
            repo.path(),
            &["config", "user.email", "mycopilot-test@example.invalid"],
        );
        Some(repo)
    }

    fn git(cwd: &Path, args: &[&str]) {
        let status = Command::new("git")
            .arg("-C")
            .arg(cwd)
            .args(args)
            .status()
            .unwrap();
        assert!(status.success());
    }

    fn git_stdout(cwd: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(cwd)
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success());
        String::from_utf8(output.stdout).unwrap().trim().to_string()
    }
}
