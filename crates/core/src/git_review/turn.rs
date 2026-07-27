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
        repository_id: repository.repository_id.clone(),
        snapshot_id: snapshot_id.clone(),
        scope: GitReviewScope::LastTurn,
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
    let source_files = record.map(|record| record.files.as_slice()).unwrap_or(&[]);
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
        let id = content_revision(
            format!("{}\0{}\0", GitReviewScope::LastTurn.as_str(), change.path).as_bytes(),
        );
        files.push((
            change,
            GitReviewFile {
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
