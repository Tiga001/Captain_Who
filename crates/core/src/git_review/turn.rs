use super::*;
use crate::{AgentTurnDiffRecord, AgentTurnFileContent};
use similar::ChangeTag;

pub(super) const LAST_TURN_SNAPSHOT_PREFIX: &str = "last-turn:";

struct TurnReviewProjection<'a> {
    files: Vec<(&'a crate::AgentTurnFileChange, GitReviewFile)>,
    stats: GitReviewStats,
    truncated: bool,
}

pub(super) fn build_last_turn_review(
    repository: &RepositoryContext,
    project_path: &Path,
    conversation_id: &str,
    record: Option<&AgentTurnDiffRecord>,
) -> (GitReviewSummary, TurnSnapshot) {
    let projection = project_turn_review(project_path, record);
    let snapshot_id = format!("{LAST_TURN_SNAPSHOT_PREFIX}{}", Uuid::new_v4());
    let mut snapshot_files = HashMap::new();
    for (change, file) in &projection.files {
        snapshot_files.insert(
            file.id.clone(),
            TurnSnapshotFile {
                path: change.path.clone(),
                before: change.before.clone(),
                after: change.after.clone(),
            },
        );
    }

    let summary = GitReviewSummary {
        source: None,
        assistant_message_id: record.map(|v| v.identity.assistant_message_id.clone()),
        message: None,
        repository_id: repository.repository_id.clone(),
        snapshot_id: snapshot_id.clone(),
        target: GitReviewTarget::LastTurn {
            conversation_id: conversation_id.to_string(),
        },
        context: GitReviewContext::default(),
        stats: projection.stats,
        files: projection.files.into_iter().map(|(_, file)| file).collect(),
        truncated: projection.truncated,
    };
    let snapshot = TurnSnapshot {
        id: snapshot_id,
        created_at: Instant::now(),
        files: snapshot_files,
    };
    (summary, snapshot)
}

pub(super) fn build_turn_diff_summary(
    project_path: &Path,
    record: &AgentTurnDiffRecord,
) -> GitTurnDiffSummary {
    let projection = project_turn_review(project_path, Some(record));
    GitTurnDiffSummary {
        assistant_message_id: record.identity.assistant_message_id.clone(),
        stats: projection.stats,
        files: projection
            .files
            .into_iter()
            .map(|(_, file)| GitTurnDiffSummaryFile {
                path: file.path,
                status: file.status,
                stats: file.stats,
            })
            .collect(),
        truncated: projection.truncated,
    }
}

fn project_turn_review<'a>(
    project_path: &Path,
    record: Option<&'a AgentTurnDiffRecord>,
) -> TurnReviewProjection<'a> {
    let record = record.filter(|record| {
        workspace_paths_match(project_path, Path::new(&record.identity.workspace_root))
    });
    // This panel still targets the primary repository. Auxiliary workspace references are
    // retained in turn history, but must not affect its file list, totals or paging budget.
    // A literal primary directory named @workspace is escaped as ./@workspace/... .
    let source_files = record
        .into_iter()
        .flat_map(|record| &record.files)
        .filter(|change| !change.path.starts_with("@workspace/"))
        .collect::<Vec<_>>();
    let truncated =
        record.is_some_and(|record| record.truncated) || source_files.len() > MAX_REVIEW_FILES;
    let mut files = Vec::new();
    let mut totals = GitReviewFileStats::default();
    let mut line_counts_complete = true;

    for change in source_files
        .iter()
        .filter(|change| !change.is_exact_noop())
        .take(MAX_REVIEW_FILES)
    {
        let status = turn_file_status(change);
        let stats = turn_file_stats(&change.before, &change.after);
        if let Some(stats) = &stats {
            totals.additions = totals.additions.saturating_add(stats.additions);
            totals.deletions = totals.deletions.saturating_add(stats.deletions);
        } else {
            line_counts_complete = false;
        }
        let id = content_revision(format!("lastTurn\0{}\0", change.path).as_bytes());
        files.push((
            *change,
            GitReviewFile {
                source_folder_id: None,
                source_alias: None,
                workspace_path: None,
                id,
                path: change.path.clone(),
                previous_path: None,
                status,
                stats,
            },
        ));
    }

    TurnReviewProjection {
        stats: GitReviewStats {
            file_count: files.len(),
            additions: totals.additions,
            deletions: totals.deletions,
            line_counts_complete,
        },
        files,
        truncated,
    }
}

pub(super) fn last_turn_file_diff(
    snapshot_id: &str,
    file_id: &str,
    file: &TurnSnapshotFile,
) -> GitReviewFileDiff {
    if matches!(
        (&file.before, &file.after),
        (AgentTurnFileContent::TooLarge, _) | (_, AgentTurnFileContent::TooLarge)
    ) {
        return turn_diff_response(
            snapshot_id,
            file_id,
            GitReviewFileDiffStatus::TooLarge,
            None,
        );
    }
    if matches!(
        (&file.before, &file.after),
        (AgentTurnFileContent::Binary, _) | (_, AgentTurnFileContent::Binary)
    ) {
        return turn_diff_response(snapshot_id, file_id, GitReviewFileDiffStatus::Binary, None);
    }

    let before = exact_text(&file.before).unwrap_or_default();
    let after = exact_text(&file.after).unwrap_or_default();
    let before_header = if matches!(file.before, AgentTurnFileContent::Missing) {
        "/dev/null".to_string()
    } else {
        format!("a/{}", file.path)
    };
    let after_header = if matches!(file.after, AgentTurnFileContent::Missing) {
        "/dev/null".to_string()
    } else {
        format!("b/{}", file.path)
    };
    let patch = TextDiff::from_lines(before, after)
        .unified_diff()
        .header(&before_header, &after_header)
        .to_string();
    if exceeds_diff_patch_budget(patch.as_bytes()) {
        return turn_diff_response(
            snapshot_id,
            file_id,
            GitReviewFileDiffStatus::TooLarge,
            None,
        );
    }
    turn_diff_response(
        snapshot_id,
        file_id,
        GitReviewFileDiffStatus::Ready,
        Some(patch),
    )
}

pub(super) fn last_turn_file_content(
    snapshot_id: &str,
    file_id: &str,
    file: &TurnSnapshotFile,
) -> GitReviewFileContent {
    if matches!(
        (&file.before, &file.after),
        (AgentTurnFileContent::TooLarge, _) | (_, AgentTurnFileContent::TooLarge)
    ) {
        return turn_content_response(
            snapshot_id,
            file_id,
            GitReviewFileContentStatus::TooLarge,
            None,
            None,
        );
    }
    if matches!(
        (&file.before, &file.after),
        (AgentTurnFileContent::Binary, _) | (_, AgentTurnFileContent::Binary)
    ) {
        return turn_content_response(
            snapshot_id,
            file_id,
            GitReviewFileContentStatus::Binary,
            None,
            None,
        );
    }
    turn_content_response(
        snapshot_id,
        file_id,
        GitReviewFileContentStatus::Ready,
        stored_text(&file.before),
        stored_text(&file.after),
    )
}

fn turn_file_status(change: &crate::AgentTurnFileChange) -> GitReviewFileStatus {
    match (&change.before, &change.after) {
        (AgentTurnFileContent::Missing, _) => GitReviewFileStatus::Added,
        (_, AgentTurnFileContent::Missing) => GitReviewFileStatus::Deleted,
        _ => GitReviewFileStatus::Modified,
    }
}

fn turn_file_stats(
    before: &AgentTurnFileContent,
    after: &AgentTurnFileContent,
) -> Option<GitReviewFileStats> {
    let before = exact_text(before)?;
    let after = exact_text(after)?;
    let mut stats = GitReviewFileStats::default();
    for change in TextDiff::from_lines(before, after).iter_all_changes() {
        match change.tag() {
            ChangeTag::Delete => stats.deletions = stats.deletions.saturating_add(1),
            ChangeTag::Insert => stats.additions = stats.additions.saturating_add(1),
            ChangeTag::Equal => {}
        }
    }
    Some(stats)
}

fn exact_text(content: &AgentTurnFileContent) -> Option<&str> {
    match content {
        AgentTurnFileContent::Missing => Some(""),
        AgentTurnFileContent::Text(content) => Some(content),
        AgentTurnFileContent::Binary | AgentTurnFileContent::TooLarge => None,
    }
}

fn stored_text(content: &AgentTurnFileContent) -> Option<String> {
    match content {
        AgentTurnFileContent::Text(content) => Some(content.clone()),
        AgentTurnFileContent::Missing
        | AgentTurnFileContent::Binary
        | AgentTurnFileContent::TooLarge => None,
    }
}

fn workspace_paths_match(project_path: &Path, recorded_path: &Path) -> bool {
    match (project_path.canonicalize(), recorded_path.canonicalize()) {
        (Ok(project), Ok(recorded)) => project == recorded,
        _ => project_path == recorded_path,
    }
}

fn turn_diff_response(
    snapshot_id: &str,
    file_id: &str,
    status: GitReviewFileDiffStatus,
    patch: Option<String>,
) -> GitReviewFileDiff {
    GitReviewFileDiff {
        snapshot_id: snapshot_id.to_string(),
        file_id: file_id.to_string(),
        status,
        patch,
    }
}

fn turn_content_response(
    snapshot_id: &str,
    file_id: &str,
    status: GitReviewFileContentStatus,
    before_text: Option<String>,
    after_text: Option<String>,
) -> GitReviewFileContent {
    GitReviewFileContent {
        snapshot_id: snapshot_id.to_string(),
        file_id: file_id.to_string(),
        status,
        before_text,
        after_text,
    }
}

impl GitReviewService {
    /// Project review interprets persisted paths against the exact workspace frozen for that
    /// turn. Current aliases and primary-role changes never redefine a historical file.
    pub fn review_project_last_turn_summary(
        &self,
        project: &ProjectRecord,
        requested: Option<&GitReviewSource>,
        conversation_id: &str,
        record: Option<&crate::AgentTurnDiffRecord>,
        frozen_workspace: Option<&crate::AgentWorkspaceContext>,
    ) -> Result<GitReviewSummary, String> {
        let inspection = self.inspect_project(project);
        let source = match requested {
            Some(source) => source.clone(),
            None => GitReviewSource::Folder {
                folder_id: inspection
                    .default_folder_id
                    .clone()
                    .ok_or("No available source folder has a reviewable Git worktree.")?,
            },
        };
        let mut messages = Vec::new();
        let selected = match &source {
            GitReviewSource::Folder { folder_id } => {
                let folder = self.project_source(project, Some(folder_id))?;
                let inspected = inspection
                    .folders
                    .iter()
                    .find(|f| f.folder_id == *folder_id)
                    .ok_or("The selected Git source is unavailable.")?;
                if inspected.state != GitRepositoryInspectionState::Ready {
                    return Err(inspected.message.clone().unwrap_or_else(|| {
                        "The selected source has no reviewable Git worktree.".into()
                    }));
                }
                vec![folder]
            }
            GitReviewSource::All => {
                for folder in &inspection.folders {
                    if matches!(
                        folder.state,
                        GitRepositoryInspectionState::Unavailable
                            | GitRepositoryInspectionState::Unsupported
                    ) {
                        messages.push(format!(
                            "{}: {}",
                            folder.alias,
                            folder.message.as_deref().unwrap_or("Source unavailable.")
                        ));
                    }
                }
                project
                    .folders
                    .iter()
                    .filter(|f| {
                        inspection.folders.iter().any(|i| {
                            i.folder_id == f.id && i.state == GitRepositoryInspectionState::Ready
                        })
                    })
                    .collect()
            }
        };
        if selected.is_empty() {
            return Err("No available source folder has a reviewable Git worktree.".into());
        }
        let repository_id = match &source {
            GitReviewSource::Folder { folder_id } => inspection
                .folders
                .iter()
                .find(|f| f.folder_id == *folder_id)
                .and_then(|f| f.repository_id.clone())
                .ok_or("The selected Git source is unavailable.")?,
            GitReviewSource::All => inspection
                .repository_id
                .clone()
                .ok_or("The project has no reviewable Git sources.")?,
        };
        if let Some(record) = record {
            if record.identity.project_id != project.id
                || record.identity.conversation_id != conversation_id
            {
                return Err(
                    "The last-turn record does not belong to this project and conversation.".into(),
                );
            }
            let workspace =
                frozen_workspace.ok_or("The last turn's frozen workspace is unavailable.")?;
            if workspace.project_id.as_deref() != Some(project.id.as_str()) {
                return Err("The last turn's workspace has a different project identity.".into());
            }
            crate::workspace::WorkspaceResolver::from_context(Some(workspace)).validate_shape()?;
            if record.files.iter().any(|change| {
                change.path.starts_with("@workspace/")
                    && historical_source_path(workspace, &change.path).is_none()
            }) {
                return Err("The last-turn file paths do not match its frozen workspace.".into());
            }
        } else {
            messages.push("There is no recorded file-change turn for this conversation.".into());
        }

        let mut files = Vec::new();
        let mut snapshot_files = HashMap::new();
        let mut stats = GitReviewStats {
            file_count: 0,
            additions: 0,
            deletions: 0,
            line_counts_complete: true,
        };
        if let (Some(record), Some(workspace)) = (record, frozen_workspace) {
            for folder in selected {
                let Some(frozen) = workspace.folders.iter().find(|f| f.id == folder.id) else {
                    messages.push(format!(
                        "{} was not part of the last turn's workspace.",
                        folder.alias
                    ));
                    continue;
                };
                let current_path = Path::new(&folder.path)
                    .canonicalize()
                    .map_err(|_| "The selected Git source became unavailable.")?;
                if frozen.canonical_path.as_deref() != current_path.to_str()
                    || frozen.directory_identity.as_ref()
                        != FileChangeDirectoryIdentity::read(&current_path)
                            .ok()
                            .as_ref()
                {
                    messages.push(format!(
                        "{} no longer identifies the directory used by the last turn.",
                        folder.alias
                    ));
                    continue;
                }
                // Frozen roots cannot overlap. The frozen path owner selects exactly one source,
                // even after current aliases or the primary folder have changed.
                let mut source_files = record
                    .files
                    .iter()
                    .filter_map(|change| {
                        if change.is_exact_noop() {
                            return None;
                        }
                        let (owner, path) = historical_source_path(workspace, &change.path)?;
                        (owner.id == frozen.id).then_some((change, path))
                    })
                    .collect::<Vec<_>>();
                source_files.sort_by(|left, right| left.1.cmp(&right.1));
                for (change, path) in source_files {
                    let file_stats = turn_file_stats(&change.before, &change.after);
                    stats.file_count += 1;
                    if let Some(counts) = &file_stats {
                        stats.additions = stats.additions.saturating_add(counts.additions);
                        stats.deletions = stats.deletions.saturating_add(counts.deletions);
                    } else {
                        stats.line_counts_complete = false;
                    }
                    if files.len() >= MAX_REVIEW_FILES {
                        continue;
                    }
                    let id =
                        content_revision(format!("lastTurn\0{}\0{}", folder.id, path).as_bytes());
                    snapshot_files.insert(
                        id.clone(),
                        TurnSnapshotFile {
                            path: path.clone(),
                            before: change.before.clone(),
                            after: change.after.clone(),
                        },
                    );
                    files.push(GitReviewFile {
                        source_folder_id: Some(folder.id.clone()),
                        source_alias: Some(folder.alias.clone()),
                        workspace_path: Some(change.path.clone()),
                        id,
                        path,
                        previous_path: None,
                        status: turn_file_status(change),
                        stats: file_stats,
                    });
                }
            }
        }
        let truncated = record.is_some_and(|r| r.truncated) || stats.file_count > MAX_REVIEW_FILES;
        let snapshot_id = format!("{LAST_TURN_SNAPSHOT_PREFIX}{}", Uuid::new_v4());
        self.turn_snapshots
            .lock()
            .map_err(|_| "Last-turn review snapshot cache is unavailable.")?
            .insert(TurnSnapshot {
                id: snapshot_id.clone(),
                created_at: Instant::now(),
                files: snapshot_files,
            });
        Ok(GitReviewSummary {
            source: Some(source),
            assistant_message_id: record.map(|r| r.identity.assistant_message_id.clone()),
            message: (!messages.is_empty()).then(|| messages.join("\n")),
            repository_id,
            snapshot_id,
            target: GitReviewTarget::LastTurn {
                conversation_id: conversation_id.into(),
            },
            context: GitReviewContext::default(),
            stats,
            files,
            truncated,
        })
    }
}

fn historical_source_path<'a>(
    workspace: &'a crate::AgentWorkspaceContext,
    path: &str,
) -> Option<(&'a crate::workspace::WorkspaceFolder, String)> {
    let (folder, relative) = if let Some(namespace) = path.strip_prefix("@workspace/") {
        let (alias, relative) = namespace.split_once('/')?;
        (
            workspace.folders.iter().find(|f| f.alias == alias)?,
            relative,
        )
    } else {
        if Path::new(path).is_absolute() {
            return None;
        }
        (
            workspace
                .folders
                .iter()
                .find(|f| f.role == ProjectFolderRole::Primary)?,
            path,
        )
    };
    if !is_safe_repository_path(relative) {
        return None;
    }
    let path = path_to_git_string(Path::new(relative))?;
    if path.is_empty() {
        return None;
    }
    Some((folder, path))
}
