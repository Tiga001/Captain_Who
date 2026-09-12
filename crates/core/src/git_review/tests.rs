use super::*;
use std::process::Command;
use tempfile::TempDir;

#[test]
fn last_turn_review_reuses_read_only_diff_and_content_surfaces() {
    let Some(repo) = test_repository() else {
        return;
    };
    let workspace_root = repo.path().canonicalize().unwrap();
    let record = crate::AgentTurnDiffRecord {
        identity: crate::AgentTurnDiffIdentity {
            run_id: "run-1".to_string(),
            conversation_id: "conversation-1".to_string(),
            assistant_message_id: "assistant-1".to_string(),
            project_id: "project-1".to_string(),
            workspace_root: workspace_root.to_string_lossy().into_owned(),
        },
        files: vec![
            crate::AgentTurnFileChange {
                path: "created.txt".to_string(),
                before: crate::AgentTurnFileContent::Missing,
                after: crate::AgentTurnFileContent::Text("created\n".to_string()),
            },
            crate::AgentTurnFileChange {
                path: "modified.txt".to_string(),
                before: crate::AgentTurnFileContent::Text("before\n".to_string()),
                after: crate::AgentTurnFileContent::Text("after\n".to_string()),
            },
            crate::AgentTurnFileChange {
                path: "binary.bin".to_string(),
                before: crate::AgentTurnFileContent::Binary,
                after: crate::AgentTurnFileContent::Binary,
            },
        ],
        truncated: false,
    };
    let service = GitReviewService::new();
    let summary = service
        .review_last_turn_summary(repo.path(), "conversation-1", Some(&record))
        .unwrap();

    assert_eq!(
        summary.target,
        GitReviewTarget::LastTurn {
            conversation_id: "conversation-1".to_string()
        }
    );
    assert_eq!(summary.files.len(), 3);
    assert!(!summary.stats.line_counts_complete);
    let created = summary
        .files
        .iter()
        .find(|file| file.path == "created.txt")
        .unwrap();
    assert_eq!(created.status, GitReviewFileStatus::Added);
    assert_eq!(
        created.stats,
        Some(GitReviewFileStats {
            additions: 1,
            deletions: 0,
        })
    );

    let diff = service
        .review_file_diff(&summary.snapshot_id, &created.id)
        .unwrap();
    assert_eq!(diff.status, GitReviewFileDiffStatus::Ready);
    assert!(diff.patch.unwrap().contains("+created"));
    let content = service
        .review_file_content(&summary.snapshot_id, &created.id)
        .unwrap();
    assert_eq!(content.status, GitReviewFileContentStatus::Ready);
    assert_eq!(content.before_text, None);
    assert_eq!(content.after_text.as_deref(), Some("created\n"));
    assert_eq!(
        service
            .mutate_review_file(
                &summary.snapshot_id,
                &created.id,
                GitReviewFileMutationAction::Stage,
            )
            .unwrap_err(),
        "Last-turn review is read-only."
    );
}

#[test]
fn last_turn_review_ignores_records_from_a_different_workspace() {
    let Some(repo) = test_repository() else {
        return;
    };
    let record = crate::AgentTurnDiffRecord {
        identity: crate::AgentTurnDiffIdentity {
            run_id: "run-1".to_string(),
            conversation_id: "conversation-1".to_string(),
            assistant_message_id: "assistant-1".to_string(),
            project_id: "project-1".to_string(),
            workspace_root: repo.path().join("other").to_string_lossy().into_owned(),
        },
        files: vec![crate::AgentTurnFileChange {
            path: "created.txt".to_string(),
            before: crate::AgentTurnFileContent::Missing,
            after: crate::AgentTurnFileContent::Text("created\n".to_string()),
        }],
        truncated: false,
    };
    let summary = GitReviewService::new()
        .review_last_turn_summary(repo.path(), "conversation-1", Some(&record))
        .unwrap();
    assert!(summary.files.is_empty());
}

#[test]
fn turn_diff_summary_uses_the_exact_same_net_diff_as_last_turn_review() {
    let Some(repo) = test_repository() else {
        return;
    };
    let workspace_root = repo.path().canonicalize().unwrap();
    let record = crate::AgentTurnDiffRecord {
        identity: crate::AgentTurnDiffIdentity {
            run_id: "run-1".to_string(),
            conversation_id: "conversation-1".to_string(),
            assistant_message_id: "assistant-1".to_string(),
            project_id: "project-1".to_string(),
            workspace_root: workspace_root.to_string_lossy().into_owned(),
        },
        files: vec![crate::AgentTurnFileChange {
            path: "src/main.rs".to_string(),
            before: crate::AgentTurnFileContent::Text("same\nold\nsame\n".to_string()),
            after: crate::AgentTurnFileContent::Text("same\nnew\nsame\n".to_string()),
        }],
        truncated: false,
    };
    let service = GitReviewService::new();
    let review = service
        .review_last_turn_summary(repo.path(), "conversation-1", Some(&record))
        .unwrap();
    let summaries =
        service.turn_diff_summaries("conversation-1", repo.path(), std::slice::from_ref(&record));

    assert_eq!(summaries.summaries.len(), 1);
    let summary = &summaries.summaries[0];
    assert_eq!(summary.assistant_message_id, "assistant-1");
    assert_eq!(summary.stats.additions, review.stats.additions);
    assert_eq!(summary.stats.deletions, review.stats.deletions);
    assert_eq!(summary.stats.file_count, review.stats.file_count);
    assert_eq!(summary.files[0].stats, review.files[0].stats);
}

#[test]
fn diff_patch_budget_rejects_each_dimension_independently() {
    assert!(!exceeds_diff_patch_budget(b"@@ -1 +1 @@\n-old\n+new\n"));

    let too_many_lines = vec![b'\n'; MAX_DIFF_LINES + 1];
    assert!(exceeds_diff_patch_budget(&too_many_lines));

    let too_many_hunks = "@@ -1 +1 @@\n".repeat(MAX_DIFF_HUNKS + 1);
    assert!(exceeds_diff_patch_budget(too_many_hunks.as_bytes()));

    let overlong_line = vec![b'x'; MAX_DIFF_LINE_BYTES + 1];
    assert!(exceeds_diff_patch_budget(&overlong_line));

    let too_many_bytes = vec![b'x'; MAX_DIFF_BYTES + 1];
    assert!(exceeds_diff_patch_budget(&too_many_bytes));
}

#[test]
fn primary_turn_review_excludes_auxiliary_files_from_counts_and_limits() {
    let workspace = tempfile::tempdir().unwrap();
    let mut files = (0..=MAX_REVIEW_FILES)
        .map(|index| crate::AgentTurnFileChange {
            path: format!("@workspace/docs/{index}.md"),
            before: crate::AgentTurnFileContent::Missing,
            after: crate::AgentTurnFileContent::Text("auxiliary\n".to_string()),
        })
        .collect::<Vec<_>>();
    files.push(crate::AgentTurnFileChange {
        path: "./@workspace/literal.md".to_string(),
        before: crate::AgentTurnFileContent::Missing,
        after: crate::AgentTurnFileContent::Text("primary\n".to_string()),
    });
    let record = crate::AgentTurnDiffRecord {
        identity: crate::AgentTurnDiffIdentity {
            run_id: "run-multi".to_string(),
            conversation_id: "conversation-multi".to_string(),
            assistant_message_id: "assistant-multi".to_string(),
            project_id: "project-multi".to_string(),
            workspace_root: workspace.path().to_string_lossy().into_owned(),
        },
        files,
        truncated: false,
    };
    let summary = turn::build_turn_diff_summary(workspace.path(), &record);
    assert_eq!(summary.files.len(), 1);
    assert_eq!(summary.files[0].path, "./@workspace/literal.md");
    assert_eq!(summary.stats.file_count, 1);
    assert_eq!(summary.stats.additions, 1);
    assert_eq!(summary.stats.deletions, 0);
    assert!(!summary.truncated);
}

#[test]
fn generated_untracked_patch_is_rechecked_after_prefix_expansion() {
    let Some(repo) = test_repository() else {
        return;
    };
    // The source stays under 512 KiB and 20k lines. Unified-diff '+' prefixes push the
    // generated patch over the byte budget, which must be checked after TextDiff renders it.
    let line = format!("{}\n", "x".repeat(25));
    let content = line.repeat(19_800);
    assert!(content.len() < MAX_DIFF_BYTES);
    assert!(content.lines().count() < MAX_DIFF_LINES);
    fs::write(repo.path().join("generated.txt"), content).unwrap();

    let service = GitReviewService::new();
    let summary = service
        .review_summary(repo.path(), GitReviewTarget::Unstaged)
        .unwrap();
    let file = summary
        .files
        .iter()
        .find(|file| file.path == "generated.txt")
        .unwrap();
    let diff = service
        .review_file_diff(&summary.snapshot_id, &file.id)
        .unwrap();

    assert_eq!(diff.status, GitReviewFileDiffStatus::TooLarge);
    assert!(diff.patch.is_none());
}

#[test]
fn parses_staged_unstaged_and_untracked_entries() {
    let output = b"M  staged.txt\0 M unstaged.txt\0?? new.txt\0";
    let staged = parse_porcelain_status(output, StatusSelection::Staged).unwrap();
    let unstaged = parse_porcelain_status(output, StatusSelection::Unstaged).unwrap();

    assert_eq!(staged.len(), 1);
    assert_eq!(staged[0].path, "staged.txt");
    assert_eq!(unstaged.len(), 2);
    assert_eq!(unstaged[0].path, "new.txt");
    assert_eq!(unstaged[0].status, GitReviewFileStatus::Untracked);
}

#[test]
fn parses_rename_source_from_nul_entry() {
    let output = b"R  renamed.txt\0original.txt\0";
    let files = parse_porcelain_status(output, StatusSelection::Staged).unwrap();

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
        .review_summary(repo.path(), GitReviewTarget::Staged)
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
        .review_summary(repo.path(), GitReviewTarget::Unstaged)
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
        .review_summary(repo.path(), GitReviewTarget::Staged)
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
        .review_summary(repo.path(), GitReviewTarget::Unstaged)
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
        .review_summary(repo.path(), GitReviewTarget::Staged)
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
        .review_summary(repo.path(), GitReviewTarget::Staged)
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
        .review_summary(repo.path(), GitReviewTarget::Staged)
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
        .review_summary(repo.path(), GitReviewTarget::Unstaged)
        .unwrap();
    let content = service
        .review_file_content(&summary.snapshot_id, &summary.files[0].id)
        .unwrap();
    assert_eq!(content.status, GitReviewFileContentStatus::Binary);
    assert_eq!(content.before_text, None);
    assert_eq!(content.after_text, None);

    fs::write(repo.path().join("tracked.bin"), [0xff, 0xfe]).unwrap();
    let summary = service
        .review_summary(repo.path(), GitReviewTarget::Unstaged)
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
        .review_summary(repo.path(), GitReviewTarget::Unstaged)
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
        .review_summary(repo.path(), GitReviewTarget::Unstaged)
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
        .review_summary(repo.path(), GitReviewTarget::Unstaged)
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
        .review_summary(repo.path(), GitReviewTarget::Unstaged)
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
        .review_summary(repo.path(), GitReviewTarget::Unstaged)
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

    let service = GitReviewService::new();
    let summary = service
        .review_summary(&repo.path().join("selected"), GitReviewTarget::Staged)
        .unwrap();

    assert_eq!(summary.files.len(), 1);
    assert_eq!(summary.files[0].path, "in-scope.txt");
    let diff = service
        .review_file_diff(&summary.snapshot_id, &summary.files[0].id)
        .unwrap();
    assert_eq!(diff.status, GitReviewFileDiffStatus::Ready);
    assert!(diff.patch.unwrap().contains("+inside"));
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
        .review_summary(repo.path(), GitReviewTarget::Unstaged)
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
        .review_summary(repo.path(), GitReviewTarget::Unstaged)
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
        .review_summary(repo.path(), GitReviewTarget::Unstaged)
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
        .review_summary(repo.path(), GitReviewTarget::Unstaged)
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
        .review_summary(repo.path(), GitReviewTarget::Staged)
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
            .review_summary(repo.path(), GitReviewTarget::Unstaged)
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
        .review_summary(repo.path(), GitReviewTarget::Unstaged)
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
        .review_summary(repo.path(), GitReviewTarget::Staged)
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
        .review_summary(repo.path(), GitReviewTarget::Unstaged)
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
        .review_summary(repo.path(), GitReviewTarget::Unstaged)
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
        .review_summary(repo.path(), GitReviewTarget::Unstaged)
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

#[test]
fn uncommitted_review_combines_index_worktree_and_untracked_changes() {
    let Some(repo) = test_repository() else {
        return;
    };
    fs::write(repo.path().join("staged.txt"), "before staged\n").unwrap();
    fs::write(repo.path().join("unstaged.txt"), "before unstaged\n").unwrap();
    fs::write(repo.path().join("both.txt"), "before both\n").unwrap();
    git(repo.path(), &["add", "."]);
    git(repo.path(), &["commit", "--quiet", "-m", "baseline"]);

    fs::write(repo.path().join("staged.txt"), "after staged\n").unwrap();
    git(repo.path(), &["add", "staged.txt"]);
    fs::write(repo.path().join("unstaged.txt"), "after unstaged\n").unwrap();
    fs::write(repo.path().join("both.txt"), "staged both\n").unwrap();
    git(repo.path(), &["add", "both.txt"]);
    fs::write(repo.path().join("both.txt"), "worktree both\n").unwrap();
    fs::write(repo.path().join("untracked.txt"), "new file\n").unwrap();

    let service = GitReviewService::new();
    let summary = service
        .review_summary(repo.path(), GitReviewTarget::Uncommitted)
        .unwrap();
    assert_eq!(summary.target, GitReviewTarget::Uncommitted);
    assert_eq!(summary.stats.file_count, 4);
    assert_eq!(
        summary
            .files
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>(),
        vec!["both.txt", "staged.txt", "unstaged.txt", "untracked.txt"]
    );

    let staged = summary
        .files
        .iter()
        .find(|file| file.path == "staged.txt")
        .unwrap();
    let content = service
        .review_file_content(&summary.snapshot_id, &staged.id)
        .unwrap();
    assert_eq!(content.before_text.as_deref(), Some("before staged\n"));
    assert_eq!(content.after_text.as_deref(), Some("after staged\n"));
    let both = summary
        .files
        .iter()
        .find(|file| file.path == "both.txt")
        .unwrap();
    let both_content = service
        .review_file_content(&summary.snapshot_id, &both.id)
        .unwrap();
    assert_eq!(both_content.before_text.as_deref(), Some("before both\n"));
    assert_eq!(both_content.after_text.as_deref(), Some("worktree both\n"));
    assert_eq!(
        service
            .mutate_review_file(
                &summary.snapshot_id,
                &staged.id,
                GitReviewFileMutationAction::Stage,
            )
            .unwrap_err(),
        "This Git review target is read-only."
    );
}

#[test]
fn uncommitted_review_uses_an_empty_tree_before_the_first_commit() {
    let Some(repo) = test_repository() else {
        return;
    };
    fs::write(repo.path().join("staged.txt"), "staged\n").unwrap();
    git(repo.path(), &["add", "staged.txt"]);
    fs::write(repo.path().join("untracked.txt"), "untracked\n").unwrap();

    let service = GitReviewService::new();
    let summary = service
        .review_summary(repo.path(), GitReviewTarget::Uncommitted)
        .unwrap();
    assert_eq!(summary.context.head_sha, None);
    assert_eq!(summary.files.len(), 2);
    let staged = summary
        .files
        .iter()
        .find(|file| file.path == "staged.txt")
        .unwrap();
    assert_eq!(staged.status, GitReviewFileStatus::Added);
    let content = service
        .review_file_content(&summary.snapshot_id, &staged.id)
        .unwrap();
    assert_eq!(content.before_text, None);
    assert_eq!(content.after_text.as_deref(), Some("staged\n"));
}

#[test]
fn commit_review_handles_root_and_first_parent_without_worktree_coupling() {
    let Some(repo) = test_repository() else {
        return;
    };
    fs::write(repo.path().join("tracked.txt"), "root\n").unwrap();
    git(repo.path(), &["add", "tracked.txt"]);
    git(repo.path(), &["commit", "--quiet", "-m", "root commit"]);
    let root_sha = git_stdout(repo.path(), &["rev-parse", "HEAD"]);

    fs::write(repo.path().join("tracked.txt"), "second\n").unwrap();
    git(repo.path(), &["add", "tracked.txt"]);
    git(repo.path(), &["commit", "--quiet", "-m", "second commit"]);
    let second_sha = git_stdout(repo.path(), &["rev-parse", "HEAD"]);
    let service = GitReviewService::new();

    let root = service
        .review_summary(
            repo.path(),
            GitReviewTarget::Commit {
                commit_sha: root_sha.clone(),
            },
        )
        .unwrap();
    let root_file = &root.files[0];
    assert_eq!(root_file.status, GitReviewFileStatus::Added);
    let root_content = service
        .review_file_content(&root.snapshot_id, &root_file.id)
        .unwrap();
    assert_eq!(root_content.before_text, None);
    assert_eq!(root_content.after_text.as_deref(), Some("root\n"));

    let second = service
        .review_summary(
            repo.path(),
            GitReviewTarget::Commit {
                commit_sha: second_sha.clone(),
            },
        )
        .unwrap();
    assert_eq!(
        second
            .context
            .commit
            .as_ref()
            .map(|commit| commit.sha.as_str()),
        Some(second_sha.as_str())
    );
    let second_file = &second.files[0];
    fs::write(repo.path().join("tracked.txt"), "later committed change\n").unwrap();
    git(repo.path(), &["add", "tracked.txt"]);
    git(repo.path(), &["commit", "--quiet", "-m", "later commit"]);
    let second_content = service
        .review_file_content(&second.snapshot_id, &second_file.id)
        .unwrap();
    assert_eq!(second_content.before_text.as_deref(), Some("root\n"));
    assert_eq!(second_content.after_text.as_deref(), Some("second\n"));
}

#[test]
fn branch_review_uses_merge_base_and_includes_current_worktree_changes() {
    let Some(repo) = test_repository() else {
        return;
    };
    git(repo.path(), &["branch", "-m", "main"]);
    fs::write(repo.path().join("base.txt"), "base\n").unwrap();
    git(repo.path(), &["add", "base.txt"]);
    git(repo.path(), &["commit", "--quiet", "-m", "base"]);
    let merge_base_sha = git_stdout(repo.path(), &["rev-parse", "HEAD"]);
    git(repo.path(), &["switch", "--quiet", "-c", "feature"]);
    git(repo.path(), &["switch", "--quiet", "main"]);
    fs::write(repo.path().join("main-only.txt"), "main only\n").unwrap();
    git(repo.path(), &["add", "main-only.txt"]);
    git(repo.path(), &["commit", "--quiet", "-m", "main work"]);
    let base_sha = git_stdout(repo.path(), &["rev-parse", "HEAD"]);
    git(repo.path(), &["switch", "--quiet", "feature"]);
    fs::write(repo.path().join("committed.txt"), "committed\n").unwrap();
    git(repo.path(), &["add", "committed.txt"]);
    git(repo.path(), &["commit", "--quiet", "-m", "feature work"]);
    fs::write(repo.path().join("working.txt"), "working\n").unwrap();

    let service = GitReviewService::new();
    let summary = service
        .review_summary(
            repo.path(),
            GitReviewTarget::Branch {
                base_ref: "refs/heads/main".to_string(),
            },
        )
        .unwrap();
    assert_eq!(summary.context.base_ref.as_deref(), Some("refs/heads/main"));
    assert_eq!(summary.context.base_sha.as_deref(), Some(base_sha.as_str()));
    assert_eq!(
        summary.context.merge_base_sha.as_deref(),
        Some(merge_base_sha.as_str())
    );
    assert_eq!(
        summary
            .files
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>(),
        vec!["committed.txt", "working.txt"]
    );
}

#[test]
fn repository_context_prefers_remote_head_and_commit_history_is_local_and_bounded() {
    let Some(repo) = test_repository() else {
        return;
    };
    git(repo.path(), &["branch", "-m", "main"]);
    fs::write(repo.path().join("tracked.txt"), "one\n").unwrap();
    git(repo.path(), &["add", "tracked.txt"]);
    git(repo.path(), &["commit", "--quiet", "-m", "first"]);
    fs::write(repo.path().join("tracked.txt"), "two\n").unwrap();
    git(repo.path(), &["add", "tracked.txt"]);
    git(repo.path(), &["commit", "--quiet", "-m", "second"]);
    git(
        repo.path(),
        &["update-ref", "refs/remotes/origin/trunk", "HEAD"],
    );
    git(
        repo.path(),
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/trunk",
        ],
    );
    git(
        repo.path(),
        &["update-ref", "refs/remotes/upstream/develop", "HEAD"],
    );
    git(
        repo.path(),
        &[
            "symbolic-ref",
            "refs/remotes/upstream/HEAD",
            "refs/remotes/upstream/develop",
        ],
    );
    git(repo.path(), &["config", "branch.main.remote", "upstream"]);

    let service = GitReviewService::new();
    let context = service.review_repository_context(repo.path()).unwrap();
    assert_eq!(context.current_branch.as_deref(), Some("main"));
    assert_eq!(
        context.default_base_ref.as_deref(),
        Some("refs/remotes/upstream/develop")
    );
    assert!(context
        .branches
        .iter()
        .any(|branch| branch.name == "upstream/develop" && branch.is_default));
    assert!(!context.branches.iter().any(|branch| branch.name == "main"));

    let history = service.review_commits(repo.path()).unwrap();
    assert_eq!(history.commits.len(), 2);
    assert_eq!(history.commits[0].subject, "second");
    assert_eq!(history.commits[0].stats.file_count, 1);
    assert!(!history.truncated);
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
