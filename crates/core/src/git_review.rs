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
mod content;
mod diff;
mod git_command;
mod mutation;
mod repository;
mod snapshot;
mod turn;

use content::*;
use diff::*;
use git_command::*;
use mutation::*;
use repository::*;
use snapshot::*;
use turn::*;

const GIT_TIMEOUT: Duration = Duration::from_secs(12);
const SNAPSHOT_TTL: Duration = Duration::from_secs(5 * 60);
const MAX_SNAPSHOTS: usize = 32;
const MAX_STATUS_BYTES: usize = 4 * 1024 * 1024;
const MAX_NUMSTAT_BYTES: usize = 4 * 1024 * 1024;
const MAX_UNTRACKED_STATS_BYTES: usize = 16 * 1024 * 1024;
const MAX_DIFF_BYTES: usize = 512 * 1024;
const MAX_DIFF_LINES: usize = 20_000;
const MAX_DIFF_HUNKS: usize = 2_000;
const MAX_DIFF_LINE_BYTES: usize = 32 * 1024;
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
    LastTurn,
}

impl GitReviewScope {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "unstaged" => Ok(Self::Unstaged),
            "staged" => Ok(Self::Staged),
            "lastTurn" => Ok(Self::LastTurn),
            _ => Err("Unsupported Git review scope.".to_string()),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Unstaged => "unstaged",
            Self::Staged => "staged",
            Self::LastTurn => "lastTurn",
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

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitTurnDiffSummaryFile {
    pub path: String,
    pub status: GitReviewFileStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stats: Option<GitReviewFileStats>,
}

/// Read-only presentation data derived from the durable first-before/final-after
/// evidence for one agent turn. This projection is intentionally not persisted.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitTurnDiffSummary {
    pub assistant_message_id: String,
    pub stats: GitReviewStats,
    pub files: Vec<GitTurnDiffSummaryFile>,
    pub truncated: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitTurnDiffSummaries {
    pub conversation_id: String,
    pub summaries: Vec<GitTurnDiffSummary>,
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
    turn_snapshots: Mutex<TurnSnapshotCache>,
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
            turn_snapshots: Mutex::new(TurnSnapshotCache::default()),
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
        if scope == GitReviewScope::LastTurn {
            return Err("Last-turn review requires an agent turn record.".to_string());
        }
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
            .map(|file| -> Result<GitReviewFile, String> {
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
                let path = repository.project_relative_path(&file.path).ok_or_else(|| {
                    "Git returned a file outside the selected project.".to_string()
                })?;
                let previous_path = file
                    .previous_path
                    .as_deref()
                    .and_then(|path| repository.project_relative_path(path));
                Ok(GitReviewFile {
                    id,
                    stats: file_stats.get(&file.path).cloned(),
                    path,
                    previous_path,
                    status: file.status,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;

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

    pub fn review_last_turn_summary(
        &self,
        project_path: &Path,
        record: Option<&crate::AgentTurnDiffRecord>,
    ) -> Result<GitReviewSummary, String> {
        let repository = resolve_repository(project_path).map_err(|error| error.message())?;
        let (summary, snapshot) = build_last_turn_review(&repository, project_path, record);
        self.turn_snapshots
            .lock()
            .map_err(|_| "Last-turn review snapshot cache is unavailable.".to_string())?
            .insert(snapshot);
        Ok(summary)
    }

    pub fn turn_diff_summaries(
        &self,
        conversation_id: &str,
        project_path: &Path,
        records: &[crate::AgentTurnDiffRecord],
    ) -> GitTurnDiffSummaries {
        GitTurnDiffSummaries {
            conversation_id: conversation_id.to_string(),
            summaries: records
                .iter()
                .filter(|record| record.identity.conversation_id == conversation_id)
                .map(|record| build_turn_diff_summary(project_path, record))
                .collect(),
        }
    }

    pub fn review_file_diff(
        &self,
        snapshot_id: &str,
        file_id: &str,
    ) -> Result<GitReviewFileDiff, String> {
        let turn_snapshot = self
            .turn_snapshots
            .lock()
            .map_err(|_| "Last-turn review snapshot cache is unavailable.".to_string())?
            .get(snapshot_id);
        if let Some(snapshot) = turn_snapshot {
            let Some(file) = snapshot.files.get(file_id) else {
                return Ok(expired_diff(snapshot_id, file_id));
            };
            return Ok(last_turn_file_diff(snapshot_id, file_id, file));
        }
        if snapshot_id.starts_with(LAST_TURN_SNAPSHOT_PREFIX) {
            return Ok(expired_diff(snapshot_id, file_id));
        }
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
        if output.stdout_truncated || exceeds_diff_patch_budget(&output.stdout) {
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
        let turn_snapshot = self
            .turn_snapshots
            .lock()
            .map_err(|_| "Last-turn review snapshot cache is unavailable.".to_string())?
            .get(snapshot_id);
        if let Some(snapshot) = turn_snapshot {
            let Some(file) = snapshot.files.get(file_id) else {
                return Ok(expired_content(snapshot_id, file_id));
            };
            return Ok(last_turn_file_content(snapshot_id, file_id, file));
        }
        if snapshot_id.starts_with(LAST_TURN_SNAPSHOT_PREFIX) {
            return Ok(expired_content(snapshot_id, file_id));
        }
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
        if snapshot_id.starts_with(LAST_TURN_SNAPSHOT_PREFIX) {
            return Err("Last-turn review is read-only.".to_string());
        }
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

#[cfg(test)]
mod tests;
