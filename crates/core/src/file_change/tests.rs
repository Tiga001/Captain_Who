use super::*;
use crate::content_revision;
use serde_json::{json, Value};
use std::fs;
use std::path::Path;
use tempfile::TempDir;

fn resolve(root: &Path, path: &str) -> ResolvedFileChangeTarget {
    FileChangePathPolicy::new(Some(root), false)
        .resolve(path)
        .expect("target should resolve")
}

fn create_plan(target: &ResolvedFileChangeTarget, content: &str) -> FileChangePlan {
    FileChangePlanner
        .plan(FileChangePlanRequest {
            operation: FileChangeOperation::Create,
            file_path: target.display_path(),
            base: FileChangeBase::Missing,
            mutation: FileChangeMutation::Complete(content.to_string()),
        })
        .expect("create plan")
}

fn update_plan(
    target: &ResolvedFileChangeTarget,
    base: &str,
    target_content: &str,
) -> FileChangePlan {
    let revision = content_revision(base.as_bytes());
    FileChangePlanner
        .plan(FileChangePlanRequest {
            operation: FileChangeOperation::Update,
            file_path: target.display_path(),
            base: FileChangeBase::Existing {
                content: base,
                revision: &revision,
            },
            mutation: FileChangeMutation::Complete(target_content.to_string()),
        })
        .expect("update plan")
}

fn delete_plan(target: &ResolvedFileChangeTarget, base: &str) -> FileChangePlan {
    let revision = content_revision(base.as_bytes());
    FileChangePlanner
        .plan(FileChangePlanRequest {
            operation: FileChangeOperation::Delete,
            file_path: target.display_path(),
            base: FileChangeBase::Existing {
                content: base,
                revision: &revision,
            },
            mutation: FileChangeMutation::Delete,
        })
        .expect("delete plan")
}

fn assert_code<T: std::fmt::Debug>(result: Result<T, FileChangeError>, code: FileChangeErrorCode) {
    assert_eq!(result.expect_err("operation should fail").code(), code);
}

#[test]
fn plans_create_update_delete_with_content_and_proposal_digests() {
    let root = TempDir::new().expect("tempdir");
    let target = resolve(root.path(), "notes.txt");
    let create = create_plan(&target, "one\n");
    assert_eq!(create.operation, FileChangeOperation::Create);
    assert!(create.diff.starts_with("--- /dev/null\n+++ b/notes.txt\n"));
    assert_eq!(create.additions, 1);
    assert_eq!(create.deletions, 0);
    assert!(create.base_content.is_none());
    assert_eq!(create.target_content.as_deref(), Some("one\n"));

    let update = update_plan(&target, "one\n", "two\n");
    assert_eq!(update.additions, 1);
    assert_eq!(update.deletions, 1);
    assert!(update.diff.contains("--- a/notes.txt\n+++ b/notes.txt\n"));

    let delete = delete_plan(&target, "two\n");
    assert_eq!(delete.operation, FileChangeOperation::Delete);
    assert!(delete.diff.contains("+++ /dev/null\n"));
    assert_eq!(delete.target, FileChangeContentState::Missing);

    assert_ne!(content_digest(b"one\n"), diff_digest("one\n"));
    assert_ne!(create.proposal_digest, update.proposal_digest);
    assert_eq!(
        create.proposal_digest,
        create_plan(&target, "one\n").proposal_digest
    );
}

#[test]
fn exact_edits_cover_every_current_operation_in_order() {
    let root = TempDir::new().expect("tempdir");
    let target = resolve(root.path(), "edits.txt");
    let base = "alpha\nanchor\nomega\n";
    let revision = content_revision(base.as_bytes());
    let plan = FileChangePlanner
        .plan(FileChangePlanRequest {
            operation: FileChangeOperation::Update,
            file_path: target.display_path(),
            base: FileChangeBase::Existing {
                content: base,
                revision: &revision,
            },
            mutation: FileChangeMutation::Edits(vec![
                FileChangeEdit::Prepend {
                    text: "start\n".into(),
                },
                FileChangeEdit::Replace {
                    old_text: "alpha".into(),
                    new_text: "ALPHA".into(),
                    replace_all: false,
                },
                FileChangeEdit::InsertBefore {
                    anchor: "anchor".into(),
                    text: "before\n".into(),
                },
                FileChangeEdit::InsertAfter {
                    anchor: "anchor".into(),
                    text: "\nafter".into(),
                },
                FileChangeEdit::Append {
                    text: "end\n".into(),
                },
            ]),
        })
        .expect("edit plan");
    assert_eq!(
        plan.target_content.as_deref(),
        Some("start\nALPHA\nbefore\nanchor\nafter\nomega\nend\n")
    );
}

#[test]
fn exact_edits_fail_closed_for_zero_or_ambiguous_matches() {
    let root = TempDir::new().expect("tempdir");
    let target = resolve(root.path(), "edits.txt");
    let base = "same same";
    let revision = content_revision(base.as_bytes());
    let request = |edit| FileChangePlanRequest {
        operation: FileChangeOperation::Update,
        file_path: target.display_path(),
        base: FileChangeBase::Existing {
            content: base,
            revision: &revision,
        },
        mutation: FileChangeMutation::Edits(vec![edit]),
    };
    assert_code(
        FileChangePlanner.plan(request(FileChangeEdit::Replace {
            old_text: "missing".into(),
            new_text: "value".into(),
            replace_all: false,
        })),
        FileChangeErrorCode::MatchNotFound,
    );
    assert_code(
        FileChangePlanner.plan(request(FileChangeEdit::Replace {
            old_text: "same".into(),
            new_text: "value".into(),
            replace_all: false,
        })),
        FileChangeErrorCode::AmbiguousMatch,
    );
    let all = FileChangePlanner
        .plan(request(FileChangeEdit::Replace {
            old_text: "same".into(),
            new_text: "value".into(),
            replace_all: true,
        }))
        .expect("replace all");
    assert_eq!(all.target_content.as_deref(), Some("value value"));
}

#[test]
fn planner_enforces_operation_revision_size_and_no_change_preconditions() {
    let root = TempDir::new().expect("tempdir");
    let target = resolve(root.path(), "limits.txt");
    assert_code(
        FileChangePlanner.plan(FileChangePlanRequest {
            operation: FileChangeOperation::Create,
            file_path: target.display_path(),
            base: FileChangeBase::Existing {
                content: "old",
                revision: "wrong",
            },
            mutation: FileChangeMutation::Complete("new".into()),
        }),
        FileChangeErrorCode::FileExists,
    );
    assert_code(
        FileChangePlanner.plan(FileChangePlanRequest {
            operation: FileChangeOperation::Update,
            file_path: target.display_path(),
            base: FileChangeBase::Existing {
                content: "old",
                revision: "wrong",
            },
            mutation: FileChangeMutation::Complete("new".into()),
        }),
        FileChangeErrorCode::RevisionConflict,
    );
    let revision = content_revision(b"same");
    assert_code(
        FileChangePlanner.plan(FileChangePlanRequest {
            operation: FileChangeOperation::Update,
            file_path: target.display_path(),
            base: FileChangeBase::Existing {
                content: "same",
                revision: &revision,
            },
            mutation: FileChangeMutation::Complete("same".into()),
        }),
        FileChangeErrorCode::NoChange,
    );
    create_plan(&target, &"x".repeat(4 * 1024 * 1024));
    assert_code(
        FileChangePlanner.plan(FileChangePlanRequest {
            operation: FileChangeOperation::Create,
            file_path: target.display_path(),
            base: FileChangeBase::Missing,
            mutation: FileChangeMutation::Complete("x".repeat(4 * 1024 * 1024 + 1)),
        }),
        FileChangeErrorCode::ContentTooLarge,
    );
}

#[test]
fn persistent_dtos_are_camel_case_versioned_and_strict() {
    let root = TempDir::new().expect("tempdir");
    let target = resolve(root.path(), "strict.txt");
    let plan = create_plan(&target, "strict\n");
    let proposal = FileChangeProposal {
        schema_version: FILE_CHANGE_SCHEMA_VERSION,
        id: "proposal-1".into(),
        transaction_id: "transaction-1".into(),
        operation: plan.operation,
        file_path: plan.file_path.clone(),
        base: plan.base.clone(),
        target: plan.target.clone(),
        diff_digest: plan.diff_digest.clone(),
        proposal_digest: plan.proposal_digest.clone(),
        additions: plan.additions,
        deletions: plan.deletions,
    };
    proposal.validate().expect("valid proposal");
    let encoded = serde_json::to_value(&proposal).expect("serialize proposal");
    assert_eq!(encoded["schemaVersion"], FILE_CHANGE_SCHEMA_VERSION);
    assert!(encoded.get("schema_version").is_none());
    let decoded: FileChangeProposal =
        serde_json::from_value(encoded.clone()).expect("round trip proposal");
    assert_eq!(decoded, proposal);

    let mut unknown = encoded.clone();
    unknown
        .as_object_mut()
        .expect("object")
        .insert("legacyField".into(), json!(true));
    assert!(serde_json::from_value::<FileChangeProposal>(unknown).is_err());

    let mut missing = encoded.clone();
    missing
        .as_object_mut()
        .expect("object")
        .remove("schemaVersion");
    assert!(serde_json::from_value::<FileChangeProposal>(missing).is_err());

    let mut unknown_version: FileChangeProposal =
        serde_json::from_value(encoded).expect("known shape");
    unknown_version.schema_version = 99;
    assert_code(
        unknown_version.validate(),
        FileChangeErrorCode::InvalidArguments,
    );
    unknown_version.schema_version = FILE_CHANGE_SCHEMA_VERSION;
    unknown_version.proposal_digest = unknown_version.proposal_digest.to_ascii_uppercase();
    assert_code(
        unknown_version.validate(),
        FileChangeErrorCode::InvalidArguments,
    );

    let edit = json!({
        "kind": "replace",
        "oldText": "a",
        "newText": "b",
        "replaceAll": false,
        "extra": true
    });
    assert!(serde_json::from_value::<FileChangeEdit>(edit).is_err());
}

#[test]
fn typed_failures_expose_only_safe_messages() {
    let error = FileChangeError::with_diagnostic(
        FileChangeErrorCode::FileExists,
        "structured_edit_error code=file_exists: /private/internal",
    );
    assert_eq!(error.to_string(), "文件已存在。");
    assert!(error
        .diagnostic()
        .expect("diagnostic")
        .contains("structured_edit_error"));
    let public = serde_json::to_string(error.failure()).expect("serialize failure");
    assert!(public.contains("文件已存在"));
    assert!(!public.contains("structured_edit_error"));
    assert!(!public.contains("/private/internal"));
    assert_eq!(
        error.failure().category,
        FileChangeErrorCategory::Precondition
    );
    assert_eq!(
        error.failure().recovery,
        FileChangeRecovery::UseUpdateOrChooseAnotherPath
    );
}

#[test]
fn error_catalog_separates_wire_path_precondition_and_execution_failures() {
    let cases = [
        (
            FileChangeErrorCode::UnknownField,
            FileChangeErrorCategory::Wire,
        ),
        (
            FileChangeErrorCode::ContentTooLarge,
            FileChangeErrorCategory::Semantic,
        ),
        (
            FileChangeErrorCode::SymlinkForbidden,
            FileChangeErrorCategory::Path,
        ),
        (
            FileChangeErrorCode::PermissionDenied,
            FileChangeErrorCategory::Authorization,
        ),
        (
            FileChangeErrorCode::RevisionConflict,
            FileChangeErrorCategory::Precondition,
        ),
        (
            FileChangeErrorCode::OutcomeUnknown,
            FileChangeErrorCategory::Execution,
        ),
    ];
    for (code, category) in cases {
        let error = FileChangeError::new(code);
        assert_eq!(error.failure().category, category);
        assert!(!error.failure().message.is_empty());
        assert!(!error.failure().message.contains("structured_edit_error"));
        assert!(!error.failure().message.contains("Serde"));
        assert!(!error.failure().message.contains("SQLite"));
    }
}

#[test]
fn path_policy_resolves_workspace_absolute_and_system_boundaries() {
    let root = TempDir::new().expect("tempdir");
    let nested = root.path().join("nested");
    fs::create_dir(&nested).expect("nested directory");
    let policy = FileChangePathPolicy::new(Some(root.path()), false);
    let relative = policy.resolve("nested/file.txt").expect("relative path");
    assert_eq!(relative.display_path(), "nested/file.txt");
    assert_eq!(
        relative.absolute_path(),
        nested
            .canonicalize()
            .expect("canonical nested")
            .join("file.txt")
    );
    let absolute_input = nested
        .canonicalize()
        .expect("canonical nested")
        .join("absolute.txt");
    let absolute = policy
        .resolve(&absolute_input.to_string_lossy())
        .expect("absolute workspace path");
    assert_eq!(absolute.display_path(), "nested/absolute.txt");

    assert_code(
        FileChangePathPolicy::new(None, true).resolve("relative.txt"),
        FileChangeErrorCode::WorkspaceRequired,
    );
    FileChangePathPolicy::new(None, true)
        .resolve(
            &nested
                .canonicalize()
                .expect("canonical nested")
                .join("outside.txt")
                .to_string_lossy(),
        )
        .expect("allowed absolute path without workspace");
    assert_code(
        policy.resolve("../escape.txt"),
        FileChangeErrorCode::PermissionDenied,
    );
    assert_code(
        policy.resolve("missing/file.txt"),
        FileChangeErrorCode::ParentMissing,
    );
    assert_code(
        policy.resolve(".git/config"),
        FileChangeErrorCode::PermissionDenied,
    );
    assert_code(
        policy.resolve("report.docx"),
        FileChangeErrorCode::UnsupportedFileType,
    );
}

#[cfg(unix)]
#[test]
fn path_policy_rejects_leaf_and_ancestor_symlinks_special_files_and_hard_links() {
    use std::os::unix::fs::symlink;
    use std::os::unix::net::UnixListener;

    let root = TempDir::new().expect("tempdir");
    let outside = TempDir::new().expect("outside tempdir");
    fs::write(outside.path().join("outside.txt"), "outside").expect("outside file");
    symlink(outside.path(), root.path().join("linked-dir")).expect("ancestor symlink");
    symlink(
        outside.path().join("outside.txt"),
        root.path().join("leaf.txt"),
    )
    .expect("leaf symlink");
    let original = root.path().join("original.txt");
    fs::write(&original, "linked").expect("original");
    fs::hard_link(&original, root.path().join("hard-link.txt")).expect("hard link");
    let _listener = UnixListener::bind(root.path().join("socket")).expect("unix socket");
    let policy = FileChangePathPolicy::new(Some(root.path()), false);

    assert_code(
        policy.resolve("linked-dir/file.txt"),
        FileChangeErrorCode::SymlinkForbidden,
    );
    assert_code(
        policy.resolve("leaf.txt"),
        FileChangeErrorCode::SymlinkForbidden,
    );
    assert_code(
        policy.resolve("hard-link.txt"),
        FileChangeErrorCode::HardLinkForbidden,
    );
    assert_code(
        policy.resolve("socket"),
        FileChangeErrorCode::NotRegularFile,
    );
}

#[test]
fn committer_creates_without_clobber_and_reconciles() {
    let root = TempDir::new().expect("tempdir");
    let target = resolve(root.path(), "create.txt");
    let plan = create_plan(&target, "created\n");
    let committer = FileChangeCommitter;
    let committed = committer
        .commit("transaction-create", &target, &plan, 10, None)
        .expect("commit create");
    assert_eq!(committed.status, FileChangeStatus::Applied);
    committed.receipt.validate().expect("valid receipt");
    assert_eq!(
        fs::read_to_string(target.absolute_path()).unwrap(),
        "created\n"
    );
    assert_eq!(
        committer.reconcile(&target, &plan, None).unwrap(),
        FileChangeReconciliation::AlreadyApplied
    );

    let raced_target = resolve(root.path(), "raced.txt");
    let raced_plan = create_plan(&raced_target, "planned\n");
    fs::write(raced_target.absolute_path(), "concurrent\n").expect("race file");
    assert_code(
        committer.commit("transaction-race", &raced_target, &raced_plan, 11, None),
        FileChangeErrorCode::FileExists,
    );
    assert_eq!(
        fs::read_to_string(raced_target.absolute_path()).unwrap(),
        "concurrent\n"
    );
}

#[test]
fn committer_rejects_tampered_target_or_approval_diff_before_writing() {
    let root = TempDir::new().expect("tempdir");
    let committer = FileChangeCommitter;

    let content_target = resolve(root.path(), "tampered-content.txt");
    let mut content_plan = create_plan(&content_target, "approved\n");
    content_plan.target_content = Some("not approved\n".into());
    assert_code(
        committer.commit(
            "transaction-tampered-content",
            &content_target,
            &content_plan,
            12,
            None,
        ),
        FileChangeErrorCode::InvalidArguments,
    );
    assert!(!content_target.absolute_path().exists());

    let diff_target = resolve(root.path(), "tampered-diff.txt");
    let mut diff_plan = create_plan(&diff_target, "approved\n");
    diff_plan.diff.push_str("+misleading approval text\n");
    assert_code(
        committer.commit(
            "transaction-tampered-diff",
            &diff_target,
            &diff_plan,
            13,
            None,
        ),
        FileChangeErrorCode::InvalidArguments,
    );
    assert!(!diff_target.absolute_path().exists());
}

#[test]
fn committer_updates_atomically_and_rejects_revision_conflicts() {
    let root = TempDir::new().expect("tempdir");
    let path = root.path().join("update.txt");
    fs::write(&path, "base\n").expect("base file");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).expect("permissions");
    }
    let target = resolve(root.path(), "update.txt");
    let plan = update_plan(&target, "base\n", "updated\n");
    let committer = FileChangeCommitter;
    committer
        .commit("transaction-update", &target, &plan, 20, None)
        .expect("commit update");
    assert_eq!(fs::read_to_string(&path).unwrap(), "updated\n");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o640
        );
    }

    let conflict_plan = update_plan(&target, "updated\n", "next\n");
    fs::write(&path, "concurrent\n").expect("concurrent update");
    assert_code(
        committer.commit("transaction-conflict", &target, &conflict_plan, 21, None),
        FileChangeErrorCode::RevisionConflict,
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), "concurrent\n");
}

#[cfg(unix)]
#[test]
fn committer_rejects_a_replaced_parent_directory_identity_before_publication() {
    let root = TempDir::new().expect("tempdir");
    let parent = root.path().join("parent");
    let displaced_parent = root.path().join("displaced-parent");
    fs::create_dir(&parent).expect("parent");
    let target = resolve(root.path(), "parent/new.txt");
    let plan = create_plan(&target, "must not publish\n");

    fs::rename(&parent, &displaced_parent).expect("displace parent");
    fs::create_dir(&parent).expect("replacement parent");
    assert_code(
        FileChangeCommitter.commit("transaction-parent-race", &target, &plan, 22, None),
        FileChangeErrorCode::Conflict,
    );
    assert!(!parent.join("new.txt").exists());
    assert!(!displaced_parent.join("new.txt").exists());
}

#[test]
fn delete_journal_supports_reconciliation_rollback_and_finalization() {
    let root = TempDir::new().expect("tempdir");
    let committer = FileChangeCommitter;
    let path = root.path().join("delete.txt");
    fs::write(&path, "delete me\n").expect("delete file");
    let target = resolve(root.path(), "delete.txt");
    let plan = delete_plan(&target, "delete me\n");
    let mut journal = committer
        .prepare_delete("transaction-delete", &target, &plan, 30)
        .expect("prepare delete");
    let encoded = serde_json::to_value(&journal).expect("serialize journal");
    assert_eq!(encoded["schemaVersion"], FILE_CHANGE_SCHEMA_VERSION);
    assert!(encoded.get("schema_version").is_none());
    let committed = committer
        .commit("transaction-delete", &target, &plan, 31, Some(&mut journal))
        .expect("commit delete");
    assert!(!path.exists());
    assert_eq!(journal.state, FileChangeDeleteJournalState::Tombstoned);
    assert_eq!(committed.delete_journal.as_ref(), Some(&journal));
    assert_eq!(
        committer.reconcile(&target, &plan, Some(&journal)).unwrap(),
        FileChangeReconciliation::AlreadyApplied
    );
    committer
        .rollback_delete(&mut journal, 32)
        .expect("rollback delete");
    assert_eq!(fs::read_to_string(&path).unwrap(), "delete me\n");
    assert_eq!(journal.state, FileChangeDeleteJournalState::RolledBack);

    let second_plan = delete_plan(&target, "delete me\n");
    let mut second = committer
        .prepare_delete("transaction-delete-final", &target, &second_plan, 33)
        .expect("prepare second delete");
    committer
        .commit(
            "transaction-delete-final",
            &target,
            &second_plan,
            34,
            Some(&mut second),
        )
        .expect("commit second delete");
    let tombstone = second.tombstone_path.clone();
    assert!(Path::new(&tombstone).exists());
    committer
        .finalize_delete(&mut second, 35)
        .expect("finalize delete");
    assert_eq!(second.state, FileChangeDeleteJournalState::Finalized);
    assert!(!Path::new(&tombstone).exists());
    assert_eq!(
        committer
            .reconcile(&target, &second_plan, Some(&second))
            .unwrap(),
        FileChangeReconciliation::AlreadyApplied
    );
}

#[test]
fn prepared_delete_journal_reconciles_a_completed_rename_after_a_crash() {
    let root = TempDir::new().expect("tempdir");
    let path = root.path().join("crash-delete.txt");
    fs::write(&path, "durable base\n").expect("delete file");
    let target = resolve(root.path(), "crash-delete.txt");
    let plan = delete_plan(&target, "durable base\n");
    let committer = FileChangeCommitter;
    let mut journal = committer
        .prepare_delete("transaction-crash", &target, &plan, 40)
        .expect("prepare journal");

    // Simulate process loss after the atomic rename but before the journal state/receipt commit.
    fs::rename(&path, &journal.tombstone_path).expect("simulate completed rename");
    assert_eq!(journal.state, FileChangeDeleteJournalState::Prepared);
    assert_eq!(
        committer.reconcile(&target, &plan, Some(&journal)).unwrap(),
        FileChangeReconciliation::AlreadyApplied
    );
    let recovered = committer
        .commit("transaction-crash", &target, &plan, 41, Some(&mut journal))
        .expect("recover committed delete");
    assert_eq!(recovered.status, FileChangeStatus::AlreadyApplied);
    assert_eq!(journal.state, FileChangeDeleteJournalState::Tombstoned);
    committer
        .finalize_delete(&mut journal, 42)
        .expect("finalize recovered delete");
}

#[test]
fn delete_commit_rejects_a_changed_base_without_tombstoning_it() {
    let root = TempDir::new().expect("tempdir");
    let path = root.path().join("delete-conflict.txt");
    fs::write(&path, "planned base\n").expect("base file");
    let target = resolve(root.path(), "delete-conflict.txt");
    let plan = delete_plan(&target, "planned base\n");
    let committer = FileChangeCommitter;
    let mut journal = committer
        .prepare_delete("transaction-delete-conflict", &target, &plan, 50)
        .expect("prepare delete");
    fs::write(&path, "changed concurrently\n").expect("change target");

    assert_code(
        committer.commit(
            "transaction-delete-conflict",
            &target,
            &plan,
            51,
            Some(&mut journal),
        ),
        FileChangeErrorCode::RevisionConflict,
    );
    assert_eq!(journal.state, FileChangeDeleteJournalState::Prepared);
    assert_eq!(fs::read_to_string(&path).unwrap(), "changed concurrently\n");
    assert!(!Path::new(&journal.tombstone_path).exists());
}

#[test]
fn result_status_and_outcome_combinations_fail_closed() {
    let root = TempDir::new().expect("tempdir");
    let target = resolve(root.path(), "result.txt");
    let plan = create_plan(&target, "result\n");
    let mut result = FileChangeResult {
        schema_version: FILE_CHANGE_SCHEMA_VERSION,
        transaction_id: "transaction-result".into(),
        operation: plan.operation,
        file_path: plan.file_path,
        status: FileChangeStatus::Applied,
        outcome: FileChangeOutcome::Applied,
        base: plan.base,
        target: plan.target,
        proposal_digest: plan.proposal_digest,
    };
    result.validate().expect("valid result");
    result.outcome = FileChangeOutcome::DefinitelyNotExecuted;
    assert_code(
        result.validate(),
        FileChangeErrorCode::IllegalFieldCombination,
    );

    let mut transaction = FileChangeTransaction {
        schema_version: FILE_CHANGE_SCHEMA_VERSION,
        id: "transaction-applying".into(),
        operation: result.operation,
        file_path: result.file_path,
        status: FileChangeStatus::Applying,
        outcome: FileChangeOutcome::OutcomeUnknown,
        base: result.base,
        target: result.target,
        proposal_digest: result.proposal_digest,
        created_at: 1,
        updated_at: 2,
    };
    transaction.validate().expect("applying outcome is unknown");
    transaction.outcome = FileChangeOutcome::DefinitelyNotExecuted;
    assert_code(
        transaction.validate(),
        FileChangeErrorCode::IllegalFieldCombination,
    );
}

#[test]
fn persistent_shapes_reject_unknown_journal_fields() {
    let root = TempDir::new().expect("tempdir");
    let path = root.path().join("delete.txt");
    fs::write(&path, "base").expect("base file");
    let target = resolve(root.path(), "delete.txt");
    let plan = delete_plan(&target, "base");
    let journal = FileChangeCommitter
        .prepare_delete("transaction", &target, &plan, 1)
        .expect("journal");
    let mut value = serde_json::to_value(journal).expect("serialize");
    value
        .as_object_mut()
        .expect("object")
        .insert("legacyPath".into(), Value::String("ignored".into()));
    assert!(serde_json::from_value::<FileChangeDeleteJournal>(value).is_err());

    let decoy = root.path().join("decoy.txt");
    fs::write(&decoy, "base").expect("decoy");
    let mut redirected = FileChangeCommitter
        .prepare_delete("redirected-transaction", &target, &plan, 2)
        .expect("redirected journal");
    redirected.tombstone_path = decoy.to_string_lossy().to_string();
    assert_code(
        FileChangeCommitter.finalize_delete(&mut redirected, 3),
        FileChangeErrorCode::IllegalFieldCombination,
    );
    assert_eq!(fs::read_to_string(decoy).unwrap(), "base");
}
