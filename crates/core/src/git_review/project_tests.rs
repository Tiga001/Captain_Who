use super::*;
use crate::workspace::freeze_project_workspace;
use crate::{
    AgentTurnDiffIdentity, AgentTurnDiffRecord, AgentTurnFileChange, AgentTurnFileContent,
};
use tempfile::TempDir;

fn git_at(path: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env(
            "GIT_CONFIG_GLOBAL",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        )
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}
fn repository(path: &Path) {
    fs::create_dir_all(path).unwrap();
    git_at(path, &["init", "-q"]);
    git_at(path, &["config", "user.email", "review@test.invalid"]);
    git_at(path, &["config", "user.name", "Review Test"]);
    fs::write(path.join("README.md"), "before\n").unwrap();
    git_at(path, &["add", "."]);
    git_at(path, &["commit", "-qm", "initial"]);
}
fn folder(path: &Path, alias: &str, role: ProjectFolderRole) -> ProjectFolderRecord {
    ProjectFolderRecord {
        id: format!("folder-{alias}"),
        path: path.canonicalize().unwrap().to_string_lossy().into_owned(),
        alias: alias.into(),
        role,
        sort_order: 0,
        created_at: 1,
    }
}
fn project(folders: Vec<ProjectFolderRecord>) -> ProjectRecord {
    ProjectRecord {
        id: "project-git-sources".into(),
        name: "sources".into(),
        folders,
        created_at: 1,
        pinned_at: None,
    }
}
fn change(path: &str, contents: &str) -> AgentTurnFileChange {
    AgentTurnFileChange {
        path: path.into(),
        before: AgentTurnFileContent::Missing,
        after: AgentTurnFileContent::Text(contents.into()),
    }
}
fn record(
    workspace: &crate::AgentWorkspaceContext,
    files: Vec<AgentTurnFileChange>,
) -> AgentTurnDiffRecord {
    AgentTurnDiffRecord {
        identity: AgentTurnDiffIdentity {
            run_id: "run-review-sources".into(),
            conversation_id: "conversation-review-sources".into(),
            assistant_message_id: "assistant-review-sources".into(),
            project_id: "project-git-sources".into(),
            workspace_root: workspace.root_path.clone().unwrap(),
        },
        files,
        truncated: false,
    }
}

#[test]
fn multi_source_review_inspects_all_folders_and_mutates_only_selected_repository() {
    let temp = TempDir::new().unwrap();
    let main = temp.path().join("notes");
    fs::create_dir(&main).unwrap();
    let a = temp.path().join("a");
    repository(&a);
    let b = temp.path().join("b");
    repository(&b);
    let project = project(vec![
        folder(&main, "main", ProjectFolderRole::Primary),
        folder(&a, "a", ProjectFolderRole::Auxiliary),
        folder(&b, "b", ProjectFolderRole::Auxiliary),
    ]);
    fs::write(a.join("README.md"), "changed a\n").unwrap();
    fs::write(b.join("README.md"), "changed b\n").unwrap();
    let service = GitReviewService::new();
    let inspection = service.inspect_project(&project);
    assert_eq!(inspection.state, GitRepositoryInspectionState::Ready);
    assert_eq!(inspection.folders.len(), 3);
    assert_eq!(
        inspection.folders[0].state,
        GitRepositoryInspectionState::NotRepository
    );
    assert_eq!(inspection.default_folder_id.as_deref(), Some("folder-a"));
    let summary = service
        .review_project_summary(&project, Some("folder-b"), GitReviewTarget::Unstaged)
        .unwrap();
    assert_eq!(summary.files.len(), 1);
    let file = &summary.files[0];
    assert_eq!(file.path, "README.md");
    assert_eq!(
        file.workspace_path.as_deref(),
        Some("@workspace/b/README.md")
    );
    assert_eq!(file.source_folder_id.as_deref(), Some("folder-b"));
    service
        .revalidate_snapshot_membership(&summary.snapshot_id, std::slice::from_ref(&project))
        .unwrap();
    assert_eq!(
        service
            .mutate_review_file(
                &summary.snapshot_id,
                &file.id,
                GitReviewFileMutationAction::Stage
            )
            .unwrap()
            .status,
        GitReviewFileMutationStatus::Applied
    );
    assert!(git_at(&a, &["diff", "--cached", "--name-only"]).is_empty());
    assert_eq!(
        git_at(&b, &["diff", "--cached", "--name-only"]),
        "README.md"
    );
    let staged = service
        .review_project_summary(&project, Some("folder-b"), GitReviewTarget::Staged)
        .unwrap();
    service
        .mutate_review_file(
            &staged.snapshot_id,
            &staged.files[0].id,
            GitReviewFileMutationAction::Unstage,
        )
        .unwrap();
    assert!(git_at(&b, &["diff", "--cached", "--name-only"]).is_empty());
    let unstaged = service
        .review_project_summary(&project, Some("folder-b"), GitReviewTarget::Unstaged)
        .unwrap();
    service
        .mutate_review_file(
            &unstaged.snapshot_id,
            &unstaged.files[0].id,
            GitReviewFileMutationAction::Restore,
        )
        .unwrap();
    assert_eq!(fs::read_to_string(b.join("README.md")).unwrap(), "before\n");
    assert_eq!(
        fs::read_to_string(a.join("README.md")).unwrap(),
        "changed a\n"
    );
}

#[test]
fn multi_source_review_same_repo_subdirectories_keep_separate_scope_and_rename_boundary() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("repo");
    repository(&root);
    let left = root.join("left");
    let right = root.join("right");
    fs::create_dir(&left).unwrap();
    fs::create_dir(&right).unwrap();
    fs::write(left.join("same.txt"), "left\n").unwrap();
    fs::write(right.join("same.txt"), "right\n").unwrap();
    git_at(&root, &["add", "."]);
    git_at(&root, &["commit", "-qm", "subtrees"]);
    let project = project(vec![
        folder(&left, "left", ProjectFolderRole::Primary),
        folder(&right, "right", ProjectFolderRole::Auxiliary),
    ]);
    let service = GitReviewService::new();
    let inspection = service.inspect_project(&project);
    assert_eq!(
        inspection.folders[0].repository_id,
        inspection.folders[1].repository_id
    );
    fs::write(left.join("same.txt"), "changed left\n").unwrap();
    fs::write(right.join("same.txt"), "changed right\n").unwrap();
    let summary = service
        .review_project_summary(&project, Some("folder-left"), GitReviewTarget::Unstaged)
        .unwrap();
    assert_eq!(summary.files.len(), 1);
    assert_eq!(summary.files[0].path, "same.txt");
    service
        .mutate_review_file(
            &summary.snapshot_id,
            &summary.files[0].id,
            GitReviewFileMutationAction::Stage,
        )
        .unwrap();
    assert_eq!(
        git_at(&root, &["diff", "--cached", "--name-only"]),
        "left/same.txt"
    );
    git_at(&root, &["reset", "--hard", "-q", "HEAD"]);
    git_at(&root, &["mv", "left/same.txt", "right/moved.txt"]);
    let moved = service
        .review_project_summary(&project, Some("folder-right"), GitReviewTarget::Staged)
        .unwrap();
    // Even if Git's restricted pathspec reports this as an add, inject the real paired rename
    // fact into its snapshot to verify the mutation boundary independently of rename heuristics.
    let id = moved
        .files
        .iter()
        .find(|f| f.path == "moved.txt")
        .unwrap()
        .id
        .clone();
    {
        let mut cache = service.snapshots.lock().unwrap();
        let mut snapshot = cache.get(&moved.snapshot_id).unwrap();
        let file = snapshot.files.get_mut(&id).unwrap();
        file.previous_path = Some("left/same.txt".into());
        cache.insert(snapshot);
    }
    let error = service
        .mutate_review_file(
            &moved.snapshot_id,
            &id,
            GitReviewFileMutationAction::Unstage,
        )
        .unwrap_err();
    assert!(error.contains("source folder boundary"), "{error}");
    assert!(!git_at(&root, &["diff", "--cached", "--name-only"]).is_empty());
}

#[test]
fn multi_source_review_removed_rebound_or_replaced_sources_expire_snapshots() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("repo");
    repository(&root);
    let project = project(vec![folder(&root, "main", ProjectFolderRole::Primary)]);
    fs::write(root.join("README.md"), "changed\n").unwrap();
    let service = GitReviewService::new();
    for replacement in [None, Some("different-alias")] {
        let summary = service
            .review_project_summary(&project, None, GitReviewTarget::Unstaged)
            .unwrap();
        let mut changed = project.clone();
        if let Some(alias) = replacement {
            changed.folders[0].alias = alias.into();
        } else {
            changed.folders.clear();
        }
        service
            .revalidate_snapshot_membership(&summary.snapshot_id, &[changed])
            .unwrap();
        assert_eq!(
            service
                .mutate_review_file(
                    &summary.snapshot_id,
                    &summary.files[0].id,
                    GitReviewFileMutationAction::Stage
                )
                .unwrap()
                .status,
            GitReviewFileMutationStatus::SnapshotExpired
        );
    }
    let summary = service
        .review_project_summary(&project, None, GitReviewTarget::Unstaged)
        .unwrap();
    fs::rename(&root, temp.path().join("original")).unwrap();
    repository(&root);
    fs::write(root.join("README.md"), "changed\n").unwrap();
    assert_eq!(
        service
            .review_file_diff(&summary.snapshot_id, &summary.files[0].id)
            .unwrap()
            .status,
        GitReviewFileDiffStatus::SnapshotExpired
    );
    assert_eq!(
        service
            .mutate_review_file(
                &summary.snapshot_id,
                &summary.files[0].id,
                GitReviewFileMutationAction::Stage
            )
            .unwrap()
            .status,
        GitReviewFileMutationStatus::SnapshotExpired
    );
    assert!(git_at(&root, &["diff", "--cached", "--name-only"]).is_empty());
}

#[test]
fn multi_source_last_turn_uses_frozen_aliases_roles_and_groups_identical_file_names() {
    let temp = TempDir::new().unwrap();
    let a = temp.path().join("a");
    let b = temp.path().join("b");
    repository(&a);
    repository(&b);
    let mut project = project(vec![
        folder(&a, "a", ProjectFolderRole::Primary),
        folder(&b, "b", ProjectFolderRole::Auxiliary),
    ]);
    let frozen = freeze_project_workspace(&project).unwrap();
    let record = record(
        &frozen,
        vec![
            change("@workspace/b/README.md", "b\n"),
            change("README.md", "a\n"),
            change("./@workspace/literal.md", "literal\n"),
        ],
    );
    project.folders[0].role = ProjectFolderRole::Auxiliary;
    project.folders[1].role = ProjectFolderRole::Primary;
    project.folders[1].alias = "renamed".into();
    let service = GitReviewService::new();
    let summary = service
        .review_project_last_turn_summary(
            &project,
            Some(&GitReviewSource::All),
            "conversation-review-sources",
            Some(&record),
            Some(&frozen),
        )
        .unwrap();
    assert_eq!(summary.stats.file_count, 3);
    assert_eq!(summary.stats.additions, 3);
    assert_eq!(
        summary.assistant_message_id.as_deref(),
        Some("assistant-review-sources")
    );
    assert_eq!(
        summary
            .files
            .iter()
            .map(|f| f.source_folder_id.as_deref().unwrap())
            .collect::<Vec<_>>(),
        vec!["folder-a", "folder-a", "folder-b"]
    );
    assert_eq!(
        summary.files.last().unwrap().source_alias.as_deref(),
        Some("renamed")
    );
    assert_eq!(
        summary.files.last().unwrap().workspace_path.as_deref(),
        Some("@workspace/b/README.md")
    );
    assert_ne!(
        summary
            .files
            .iter()
            .find(|f| f.source_folder_id.as_deref() == Some("folder-a") && f.path == "README.md")
            .unwrap()
            .id,
        summary.files.last().unwrap().id
    );
    let diff = service
        .review_file_content(&summary.snapshot_id, &summary.files.last().unwrap().id)
        .unwrap();
    assert_eq!(diff.after_text.as_deref(), Some("b\n"));
    assert!(service
        .mutate_review_file(
            &summary.snapshot_id,
            &summary.files[0].id,
            GitReviewFileMutationAction::Restore
        )
        .unwrap_err()
        .contains("read-only"));
    let selected = service
        .review_project_last_turn_summary(
            &project,
            Some(&GitReviewSource::Folder {
                folder_id: "folder-b".into(),
            }),
            "conversation-review-sources",
            Some(&record),
            Some(&frozen),
        )
        .unwrap();
    assert_eq!(selected.files.len(), 1);
    assert_eq!(selected.stats.additions, 1);
    assert!(service
        .review_project_last_turn_summary(&project, None, "foreign", Some(&record), Some(&frozen))
        .is_err());
    assert!(service
        .review_project_last_turn_summary(
            &project,
            None,
            "conversation-review-sources",
            Some(&record),
            None
        )
        .is_err());
}

#[test]
fn multi_source_last_turn_reports_missing_records_and_rebound_historical_sources() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("repo");
    repository(&root);
    let project = project(vec![folder(&root, "main", ProjectFolderRole::Primary)]);
    let frozen = freeze_project_workspace(&project).unwrap();
    let record = record(&frozen, vec![change("README.md", "historical\n")]);
    let service = GitReviewService::new();
    let empty = service
        .review_project_last_turn_summary(&project, None, "conversation-review-sources", None, None)
        .unwrap();
    assert!(empty.message.unwrap().contains("no recorded"));
    fs::rename(&root, temp.path().join("old")).unwrap();
    repository(&root);
    let changed = service
        .review_project_last_turn_summary(
            &project,
            Some(&GitReviewSource::All),
            "conversation-review-sources",
            Some(&record),
            Some(&frozen),
        )
        .unwrap();
    assert!(changed.files.is_empty());
    assert!(changed.message.unwrap().contains("no longer identifies"));
}

#[test]
fn multi_source_review_distinguishes_worktrees_and_expires_new_nested_repository() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("repo");
    repository(&root);
    let linked = temp.path().join("linked");
    git_at(
        &root,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "linked",
            linked.to_str().unwrap(),
        ],
    );
    let project = project(vec![
        folder(&root, "main", ProjectFolderRole::Primary),
        folder(&linked, "linked", ProjectFolderRole::Auxiliary),
    ]);
    let service = GitReviewService::new();
    let inspected = service.inspect_project(&project);
    assert_ne!(
        inspected.folders[0].repository_id,
        inspected.folders[1].repository_id
    );
    let sub = root.join("sub");
    fs::create_dir(&sub).unwrap();
    fs::write(sub.join("new.txt"), "new\n").unwrap();
    let summary = service
        .review_summary(&sub, GitReviewTarget::Unstaged)
        .unwrap();
    assert_eq!(summary.files.len(), 1);
    git_at(&sub, &["init", "-q"]);
    assert_eq!(
        service
            .mutate_review_file(
                &summary.snapshot_id,
                &summary.files[0].id,
                GitReviewFileMutationAction::Stage
            )
            .unwrap()
            .status,
        GitReviewFileMutationStatus::SnapshotExpired
    );
}

#[test]
fn multi_source_last_turn_rejects_overlapping_frozen_source_roots() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("repo");
    repository(&root);
    let nested = root.join("nested");
    fs::create_dir(&nested).unwrap();
    let single = project(vec![folder(&root, "main", ProjectFolderRole::Primary)]);
    let mut frozen = freeze_project_workspace(&single).unwrap();
    let mut nested_folder = freeze_project_workspace(&project(vec![folder(
        &nested,
        "nested",
        ProjectFolderRole::Primary,
    )]))
    .unwrap()
    .folders
    .remove(0);
    nested_folder.role = ProjectFolderRole::Auxiliary;
    frozen.folders.push(nested_folder);
    let project = project(vec![
        folder(&root, "main", ProjectFolderRole::Primary),
        folder(&nested, "nested", ProjectFolderRole::Auxiliary),
    ]);
    assert!(freeze_project_workspace(&project)
        .unwrap_err()
        .contains("不能重叠"));
    let record = record(&frozen, vec![change("nested/file.txt", "child\n")]);
    let error = GitReviewService::new()
        .review_project_last_turn_summary(
            &project,
            Some(&GitReviewSource::All),
            "conversation-review-sources",
            Some(&record),
            Some(&frozen),
        )
        .unwrap_err();
    assert!(error.contains("不能重叠"), "{error}");
}

#[test]
fn multi_source_staged_snapshot_cannot_unstage_against_a_different_head() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("repo");
    repository(&root);
    fs::write(root.join("README.md"), "after\n").unwrap();
    git_at(&root, &["add", "."]);
    let service = GitReviewService::new();
    let summary = service
        .review_summary(&root, GitReviewTarget::Staged)
        .unwrap();
    let tree = git_at(&root, &["rev-parse", "HEAD^{tree}"]);
    let head = git_at(&root, &["rev-parse", "HEAD"]);
    let new_head = git_at(
        &root,
        &[
            "commit-tree",
            &tree,
            "-p",
            &head,
            "-m",
            "advance HEAD without touching the index",
        ],
    );
    git_at(&root, &["update-ref", "HEAD", &new_head]);
    assert_eq!(
        service
            .mutate_review_file(
                &summary.snapshot_id,
                &summary.files[0].id,
                GitReviewFileMutationAction::Unstage
            )
            .unwrap()
            .status,
        GitReviewFileMutationStatus::SnapshotExpired
    );
    assert_eq!(
        git_at(&root, &["diff", "--cached", "--name-only"]),
        "README.md"
    );
}
