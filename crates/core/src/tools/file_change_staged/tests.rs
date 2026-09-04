use super::*;
use crate::protocol::{
    AgentCommandPermission, AgentPatchPermission, AgentPermissions, AgentReadPermission,
    AgentRunContext, AgentToolCall, AgentWorkspaceContext,
};
use crate::storage::models::ChatConversationRecord;
use crate::storage::service::StorageService;
use crate::tools::ToolRegistry;
use std::fs;
use std::path::Path;
use std::sync::Arc;
use tempfile::tempdir;

fn context(
    root: &Path,
    storage: Arc<StorageService>,
    conversation_id: &str,
    project_id: Option<&str>,
    run_id: &str,
) -> ToolExecutionContext {
    context_with_write(
        root,
        storage,
        conversation_id,
        project_id,
        run_id,
        AgentWritePermission::WorkspaceOnly,
    )
}

fn context_with_write(
    root: &Path,
    storage: Arc<StorageService>,
    conversation_id: &str,
    project_id: Option<&str>,
    run_id: &str,
    write: AgentWritePermission,
) -> ToolExecutionContext {
    ToolExecutionContext::from_run_context(Some(&AgentRunContext {
        collaboration_identity: None,
        conversation_id: Some(conversation_id.to_string()),
        project_id: project_id.map(ToString::to_string),
        workspace: Some(AgentWorkspaceContext {
            project_id: project_id.map(ToString::to_string),
            display_name: Some("workspace".to_string()),
            root_path: Some(root.to_string_lossy().into_owned()),
        }),
        attachment_library: None,
        permissions: AgentPermissions {
            read: AgentReadPermission::WorkspaceOnly,
            write,
            command: AgentCommandPermission::RequireApproval,
            command_safety: Default::default(),
            patch: AgentPatchPermission::RequireApproval,
            builtin_execution: Default::default(),
        },
    }))
    .with_runtime_services(run_id.to_string(), Some(storage))
    .with_file_change_tool_set_revision("tool-set-staged-test".to_string())
    .with_file_change_provider_wire_revision("provider-wire-staged-test".to_string())
}

fn save_conversation(storage: &StorageService, id: &str) {
    storage
        .save_conversation(ChatConversationRecord {
            id: id.to_string(),
            project_id: None,
            model_id: None,
            title: "Test".to_string(),
            messages: Vec::new(),
            created_at: 1,
            updated_at: 1,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
}

fn observe(context: &ToolExecutionContext, path: &str, call_id: &str) -> String {
    let result = ToolRegistry::defaults_with_search(None).execute(
        context,
        &AgentToolCall {
            id: call_id.to_string(),
            tool: "read_file".to_string(),
            args: json!({"path": path}),
            approval_status: AgentApprovalStatus::Approved,
            reason: None,
        },
    );
    assert!(result.ok, "{:?}", result.error);
    result.result.unwrap()["observationId"]
        .as_str()
        .unwrap()
        .to_string()
}

fn call_context(context: &ToolExecutionContext, id: &str) -> ToolExecutionContext {
    context.clone().with_tool_call_id(id.to_string())
}

#[test]
fn four_mib_is_the_exact_draft_limit_and_chunk_limit_is_provider_friendly() {
    assert_eq!(MAX_STAGED_FILE_BYTES, 4 * 1024 * 1024);
    assert_eq!(MAX_STAGED_CHUNK_BYTES, 1024 * 1024);
}

#[test]
fn tail_preview_is_utf8_safe() {
    let text = "你".repeat(RESULT_TAIL_CHARS + 3);
    let (tail, total, start) = tail_preview(&text, RESULT_TAIL_CHARS);
    assert_eq!(total, RESULT_TAIL_CHARS + 3);
    assert_eq!(start, 3);
    assert_eq!(tail.chars().count(), RESULT_TAIL_CHARS);
}

#[test]
fn settled_states_do_not_offer_mutation_actions() {
    for status in [
        "waiting_approval",
        "applying",
        "applied",
        "aborted",
        "expired",
    ] {
        assert_eq!(
            allowed_staged_actions(status),
            vec![FileChangeStagedAction::Status]
        );
    }
}

#[test]
fn outcome_unknown_remains_fenced_and_already_applied_is_terminal() {
    assert!(is_unsettled_staged_status("outcome_unknown"));
    assert!(!is_unsettled_staged_status("already_applied"));
}

#[test]
fn public_result_projection_keeps_metadata_without_resumability_text() {
    const CANARY: &str = "PRIVATE_STAGED_RESULT_TAIL_CANARY";
    let raw = AgentToolResult {
        exact_archive_file: None,
        call_id: "call-append".to_string(),
        tool: "apply_patch".to_string(),
        ok: true,
        result: Some(json!({
            "transactionId": "transaction-1",
            "status": "drafting",
            "tail": CANARY,
        })),
        error: None,
    };
    let projected = public_result_projection(&raw);
    let encoded = serde_json::to_string(&projected).unwrap();
    assert!(!encoded.contains(CANARY));
    assert!(encoded.contains("tailBytes"));
    assert!(encoded.contains("tailDigest"));
    assert!(serde_json::to_string(&raw).unwrap().contains(CANARY));
}

#[test]
fn staged_create_observation_survives_unrelated_sibling_changes() {
    let fixture = tempdir().unwrap();
    let root = fixture.path().join("workspace");
    fs::create_dir_all(&root).unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    save_conversation(&storage, "conversation-staged-siblings");
    let owner_context = context(
        &root,
        storage,
        "conversation-staged-siblings",
        Some("project-staged-siblings"),
        "run-staged-siblings",
    );
    fs::write(root.join("sibling-before-begin.md"), "unrelated\n").unwrap();
    let begun = begin(
        &call_context(&owner_context, "begin-staged-sibling-target"),
        StagedSource::new(
            "apply_patch",
            content_digest(b"begin-staged-sibling-target"),
        ),
        FileChangeOperation::Create,
        None,
        "target.md".to_string(),
        None,
        None,
    )
    .expect("a sibling create must not invalidate an independent staged create");
    let transaction_id = begun["transactionId"].as_str().unwrap().to_string();
    append(
        &call_context(&owner_context, "append-staged-sibling-target"),
        "apply_patch",
        transaction_id.clone(),
        0,
        0,
        "target\n".to_string(),
        content_digest(b"append-staged-sibling-target"),
    )
    .unwrap();

    fs::write(root.join("sibling-before-commit.md"), "also unrelated\n").unwrap();
    let commit_call = AgentToolCall {
        id: "commit-staged-sibling-target".to_string(),
        tool: "apply_patch".to_string(),
        args: json!({
            "request": {
                "action": "commit",
                "transactionId": transaction_id,
                "expectedDraftRevision": 1
            }
        }),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };
    let proposal = commit(
        &call_context(&owner_context, "commit-staged-sibling-target"),
        &commit_call,
        "apply_patch",
        transaction_id,
        1,
        None,
    )
    .expect("a sibling created while drafting must not invalidate staged commit");

    assert_eq!(proposal.file_path, "target.md");
    assert_eq!(
        proposal.execution.target_content.as_deref(),
        Some("target\n")
    );
    assert!(!root.join("target.md").exists());
}

#[test]
fn aborted_and_expired_records_allow_only_complete_or_absent_final_identity() {
    let fixture = |status: &str| AgentFileChangeRecord {
        schema_version: AGENT_FILE_CHANGE_SCHEMA_VERSION,
        id: format!("transaction-{status}"),
        conversation_id: "conversation-1".to_string(),
        project_id: None,
        run_id: "run-1".to_string(),
        source_tool_name: "apply_patch".to_string(),
        source_tool_call_id: "call-begin".to_string(),
        source_tool_arguments_digest: content_digest(b"begin"),
        permission_revision: "permission-v1".to_string(),
        tool_set_revision: "tool-set-v1".to_string(),
        provider_wire_revision: "provider-wire-v1".to_string(),
        observation_id: "fobs_current".to_string(),
        observation_json: "{}".to_string(),
        file_path: "report.md".to_string(),
        operation: "create".to_string(),
        strategy: None,
        status: status.to_string(),
        base_revision: None,
        base_content: String::new(),
        content: String::new(),
        draft_revision: 0,
        next_mutation_index: 0,
        additions: 0,
        deletions: 0,
        line_count: 0,
        byte_count: 0,
        mutation_count: 0,
        stats_final: true,
        summary: None,
        final_action_id: None,
        final_action_arguments_digest: None,
        final_permission_revision: None,
        final_tool_set_revision: None,
        final_provider_wire_revision: None,
        created_at: 1,
        updated_at: 2,
        expires_at: 3,
    };

    for status in ["aborted", "expired"] {
        let mut record = fixture(status);
        validate_record(&record).expect("a pre-commit terminal record has no final identity");

        record.final_action_id = Some("call-commit".to_string());
        record.final_action_arguments_digest = Some(content_digest(b"commit"));
        record.final_permission_revision = Some("permission-v2".to_string());
        record.final_tool_set_revision = Some("tool-set-v2".to_string());
        record.final_provider_wire_revision = Some("provider-wire-v2".to_string());
        validate_record(&record)
            .expect("a post-commit terminal record retains its complete frozen identity");

        record.final_provider_wire_revision = None;
        assert!(
            validate_record(&record).is_err(),
            "partial final identity must fail"
        );
    }

    let mut drafting = fixture("drafting");
    drafting.stats_final = false;
    validate_record(&drafting).expect("mutable draft statistics remain provisional");
    drafting.stats_final = true;
    assert!(validate_record(&drafting).is_err());

    let mut settled = fixture("aborted");
    settled.stats_final = false;
    assert!(validate_record(&settled).is_err());
}

#[test]
fn mixed_mutations_are_monotonic_idempotent_and_commit_the_same_file_change_plan() {
    let fixture = tempdir().unwrap();
    let root = fixture.path().join("workspace");
    fs::create_dir_all(&root).unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    save_conversation(&storage, "conversation-staged");
    let owner_context = context(
        &root,
        storage.clone(),
        "conversation-staged",
        Some("project-staged"),
        "run-staged",
    );
    let begin_args_digest = content_digest(b"begin-args");
    let begun = begin(
        &call_context(&owner_context, "begin-staged"),
        StagedSource::new("apply_patch", begin_args_digest.clone()),
        FileChangeOperation::Create,
        None,
        "report.md".to_string(),
        None,
        None,
    )
    .unwrap();
    let transaction_id = begun["transactionId"].as_str().unwrap().to_string();
    assert_eq!(begun["draftRevision"], 0);
    assert_eq!(begun["nextIndex"], 0);
    assert_eq!(begun["requiresCommitBeforeResponse"], true);
    let begin_replay = begin(
        &call_context(&owner_context, "begin-staged"),
        StagedSource::new("apply_patch", begin_args_digest),
        FileChangeOperation::Create,
        None,
        "report.md".to_string(),
        None,
        None,
    )
    .unwrap();
    assert_eq!(begin_replay, begun);
    let begin_mismatch = begin(
        &call_context(&owner_context, "begin-staged"),
        StagedSource::new("apply_patch", content_digest(b"different-begin-args")),
        FileChangeOperation::Create,
        None,
        "different.md".to_string(),
        None,
        None,
    )
    .unwrap_err();
    assert_eq!(
        begin_mismatch.code(),
        Some("agent.apply_patch.replay_mismatch")
    );

    let append_args_digest = content_digest(b"append-args");
    let appended = append(
        &call_context(&owner_context, "append-staged-1"),
        "apply_patch",
        transaction_id.clone(),
        0,
        0,
        "alpha\n".to_string(),
        append_args_digest.clone(),
    )
    .unwrap();
    assert_eq!(appended["draftRevision"], 1);
    assert_eq!(appended["nextIndex"], 1);

    let replay = append(
        &call_context(&owner_context, "append-staged-retry"),
        "apply_patch",
        transaction_id.clone(),
        0,
        0,
        "alpha\n".to_string(),
        append_args_digest,
    )
    .unwrap();
    assert_eq!(replay, appended);
    let mismatch = append(
        &call_context(&owner_context, "append-staged-mismatch"),
        "apply_patch",
        transaction_id.clone(),
        0,
        0,
        "different\n".to_string(),
        content_digest(b"different-args"),
    )
    .unwrap_err();
    assert_eq!(mismatch.code(), Some("agent.apply_patch.replay_mismatch"));

    let edited = edit(
        &call_context(&owner_context, "edit-staged"),
        "apply_patch",
        transaction_id.clone(),
        1,
        1,
        vec![FileChangeEdit::Append {
            text: "beta\n".to_string(),
        }],
        content_digest(b"edit-args"),
    )
    .unwrap();
    assert_eq!(edited["draftRevision"], 2);
    assert_eq!(edited["byteCount"], 11);

    let out_of_order = edit(
        &call_context(&owner_context, "edit-out-of-order"),
        "apply_patch",
        transaction_id.clone(),
        3,
        2,
        vec![FileChangeEdit::Append { text: "x".into() }],
        content_digest(b"out-of-order"),
    )
    .unwrap_err();
    assert_eq!(
        out_of_order.code(),
        Some("agent.apply_patch.mutation_out_of_order")
    );
    let stale = edit(
        &call_context(&owner_context, "edit-stale"),
        "apply_patch",
        transaction_id.clone(),
        2,
        1,
        vec![FileChangeEdit::Append { text: "x".into() }],
        content_digest(b"stale"),
    )
    .unwrap_err();
    assert_eq!(
        stale.code(),
        Some("agent.apply_patch.draft_revision_conflict")
    );

    let commit_call = AgentToolCall {
        id: "commit-staged".to_string(),
        tool: "apply_patch".to_string(),
        args: json!({
            "request": {
                "action":"commit",
                "transactionId":transaction_id,
                "expectedDraftRevision":2,
                "summary":"Create report"
            }
        }),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };
    let frozen_proposal = std::cell::RefCell::new(None);
    let injected = commit_with_hook(
        &call_context(&owner_context, "commit-staged"),
        &commit_call,
        "apply_patch",
        transaction_id.clone(),
        2,
        Some("Create report".to_string()),
        |durable_transaction_id, proposal| {
            assert_eq!(durable_transaction_id, transaction_id);
            frozen_proposal.replace(Some(serde_json::to_value(proposal).unwrap()));
            Err(AgentError::new(
                "injected failure after durable FileChange proposal",
            ))
        },
    )
    .unwrap_err();
    assert!(injected
        .to_string()
        .contains("injected failure after durable FileChange proposal"));
    assert_eq!(
        storage
            .get_agent_file_change(&transaction_id)
            .unwrap()
            .unwrap()
            .status,
        "waiting_approval"
    );

    // Simulate a process boundary after the transaction became durable but before its Tool
    // result/pending receipt was returned. Replay has no in-memory observation and must return
    // the same frozen proposal instead of creating or revalidating a second transaction.
    let recovered_storage =
        Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let recovered_context = context(
        &root,
        recovered_storage,
        "conversation-staged",
        Some("project-staged"),
        "run-staged",
    );
    fs::write(root.join("report.md"), "concurrent change\n").unwrap();
    let proposal = commit(
        &call_context(&recovered_context, "commit-staged"),
        &commit_call,
        "apply_patch",
        transaction_id.clone(),
        2,
        Some("Create report".to_string()),
    )
    .unwrap();
    assert_eq!(
        serde_json::to_value(&proposal).unwrap(),
        frozen_proposal
            .into_inner()
            .expect("first frozen proposal was captured after durable transition"),
        "commit replay must return the exact frozen proposal"
    );
    assert_eq!(proposal.transaction_id, transaction_id);
    assert_eq!(proposal.byte_count, 11);
    assert_eq!(
        proposal.execution.target_content.as_deref(),
        Some("alpha\nbeta\n")
    );
    proposal.execution.validate().unwrap();
    let direct_plan = FileChangePlanner
        .plan(FileChangePlanRequest {
            operation: FileChangeOperation::Create,
            file_path: "report.md",
            base: FileChangeBase::Missing,
            mutation: FileChangeMutation::Complete("alpha\nbeta\n".to_string()),
        })
        .unwrap();
    assert_eq!(proposal.execution.transaction.base, direct_plan.base);
    assert_eq!(proposal.execution.transaction.target, direct_plan.target);
    assert_eq!(
        proposal.execution.proposal.diff_digest,
        direct_plan.diff_digest
    );
    assert_eq!(
        proposal.execution.proposal.proposal_digest,
        direct_plan.proposal_digest
    );
    assert_eq!(proposal.additions, direct_plan.additions);
    assert_eq!(proposal.deletions, direct_plan.deletions);
    let mut mismatched_commit = commit_call.clone();
    mismatched_commit.args["summary"] = json!("Different summary");
    let mismatch = commit(
        &call_context(&recovered_context, "commit-staged"),
        &mismatched_commit,
        "apply_patch",
        transaction_id.clone(),
        2,
        Some("Different summary".to_string()),
    )
    .unwrap_err();
    assert_eq!(mismatch.code(), Some("agent.apply_patch.replay_mismatch"));
    let settled = append(
        &call_context(&recovered_context, "append-after-commit"),
        "apply_patch",
        proposal.transaction_id,
        2,
        2,
        "no".to_string(),
        content_digest(b"after"),
    )
    .unwrap_err();
    assert_eq!(
        settled.code(),
        Some("agent.apply_patch.transaction_settled")
    );
}

#[test]
fn staged_transaction_resumes_after_storage_reopen() {
    let fixture = tempdir().unwrap();
    let root = fixture.path().join("workspace");
    let database = fixture.path().join("storage.sqlite");
    fs::create_dir_all(&root).unwrap();

    let transaction_id = {
        let storage = Arc::new(StorageService::open(&database).unwrap());
        save_conversation(&storage, "conversation-restart");
        let context = context(
            &root,
            storage.clone(),
            "conversation-restart",
            Some("project-restart"),
            "run-restart",
        );
        let injected = begin_with_hook(
            &call_context(&context, "begin-restart"),
            StagedSource::new("apply_patch", content_digest(b"begin-restart")),
            FileChangeOperation::Create,
            None,
            "restart.md".to_string(),
            None,
            None,
            |durable_transaction_id| {
                assert!(durable_transaction_id.starts_with("file-change-staged-v1:"));
                Err(AgentError::new(
                    "injected failure after durable FileChange begin",
                ))
            },
        )
        .unwrap_err();
        assert!(injected
            .to_string()
            .contains("injected failure after durable FileChange begin"));
        let begun = storage
            .get_agent_file_change_for_source_call(
                "conversation-restart",
                Some("project-restart"),
                "run-restart",
                "begin-restart",
            )
            .unwrap()
            .expect("begin transaction was durable before the injected failure");
        begun.id
    };

    let storage = Arc::new(StorageService::open(&database).unwrap());
    let context = context(
        &root,
        storage,
        "conversation-restart",
        Some("project-restart"),
        "run-restart",
    );
    let replayed_begin = begin(
        &call_context(&context, "begin-restart"),
        StagedSource::new("apply_patch", content_digest(b"begin-restart")),
        FileChangeOperation::Create,
        None,
        "restart.md".to_string(),
        None,
        None,
    )
    .unwrap();
    assert_eq!(replayed_begin["transactionId"], transaction_id);
    assert_eq!(
        context
            .storage()
            .unwrap()
            .list_agent_file_changes_for_run("run-restart")
            .unwrap()
            .len(),
        1,
        "begin replay after restart must not create a second transaction"
    );
    let restored = status(&context, "apply_patch", transaction_id.clone()).unwrap();
    assert_eq!(restored["draftRevision"], 0);
    assert_eq!(restored["nextIndex"], 0);
    let appended = append(
        &call_context(&context, "append-after-restart"),
        "apply_patch",
        transaction_id.clone(),
        0,
        0,
        "persisted\n".to_string(),
        content_digest(b"append-after-restart"),
    )
    .unwrap();
    assert_eq!(appended["draftRevision"], 1);
    assert_eq!(appended["nextIndex"], 1);
    assert_eq!(
        context
            .storage()
            .unwrap()
            .get_agent_file_change(&transaction_id)
            .unwrap()
            .unwrap()
            .content,
        "persisted\n"
    );
}

#[test]
fn staged_limits_use_bytes_and_reject_nul_or_cross_owner_access() {
    let fixture = tempdir().unwrap();
    let root = fixture.path().join("workspace");
    fs::create_dir_all(&root).unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    save_conversation(&storage, "conversation-owner");
    save_conversation(&storage, "conversation-other");
    let owner = context(
        &root,
        storage.clone(),
        "conversation-owner",
        None,
        "run-owner",
    );
    let begun = begin(
        &call_context(&owner, "begin-large"),
        StagedSource::new("apply_patch", content_digest(b"begin-large")),
        FileChangeOperation::Create,
        None,
        "large.txt".into(),
        None,
        None,
    )
    .unwrap();
    let id = begun["transactionId"].as_str().unwrap().to_string();

    let utf8_boundary = format!("{}x", "你".repeat((MAX_STAGED_CHUNK_BYTES - 1) / 3));
    assert_eq!(utf8_boundary.len(), MAX_STAGED_CHUNK_BYTES);
    append(
        &call_context(&owner, "append-1m"),
        "apply_patch",
        id.clone(),
        0,
        0,
        utf8_boundary,
        content_digest(b"one-mib"),
    )
    .unwrap();
    for index in 1..4 {
        append(
            &call_context(&owner, &format!("append-{index}")),
            "apply_patch",
            id.clone(),
            index,
            index,
            "x".repeat(MAX_STAGED_CHUNK_BYTES),
            content_digest(format!("chunk-{index}").as_bytes()),
        )
        .unwrap();
    }
    let over = append(
        &call_context(&owner, "append-over"),
        "apply_patch",
        id.clone(),
        4,
        4,
        "x".to_string(),
        content_digest(b"over"),
    )
    .unwrap_err();
    assert_eq!(over.code(), Some("agent.apply_patch.content_too_large"));
    let nul = append(
        &call_context(&owner, "append-nul"),
        "apply_patch",
        id.clone(),
        4,
        4,
        "\0".to_string(),
        content_digest(b"nul"),
    )
    .unwrap_err();
    assert_eq!(nul.code(), Some("agent.apply_patch.unsupported_file_type"));

    let other_run = context(
        &root,
        storage.clone(),
        "conversation-owner",
        None,
        "run-other",
    );
    let cross_run = status(&other_run, "apply_patch", id.clone()).unwrap_err();
    assert_eq!(
        cross_run.code(),
        Some("agent.apply_patch.transaction_owner_mismatch")
    );
    let other_conversation = context(&root, storage, "conversation-other", None, "run-owner");
    let cross_conversation = status(&other_conversation, "apply_patch", id).unwrap_err();
    assert_eq!(
        cross_conversation.code(),
        Some("agent.apply_patch.transaction_owner_mismatch")
    );
}

#[test]
fn update_strategies_and_abort_status_use_the_same_owned_transaction() {
    let fixture = tempdir().unwrap();
    let root = fixture.path().join("workspace");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("existing.txt"), "base\n").unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    save_conversation(&storage, "conversation-update");
    let owner = context(
        &root,
        storage.clone(),
        "conversation-update",
        Some("project-update"),
        "run-update",
    );

    let modify_observation = observe(&owner, "existing.txt", "read-modify");
    let modify = begin(
        &call_context(&owner, "begin-modify"),
        StagedSource::new("apply_patch", content_digest(b"begin-modify")),
        FileChangeOperation::Update,
        Some(StagedUpdateStrategy::Modify),
        "existing.txt".into(),
        Some(modify_observation),
        None,
    )
    .unwrap();
    let modify_id = modify["transactionId"].as_str().unwrap().to_string();
    assert_eq!(modify["strategy"], "modify");
    assert_eq!(modify["tail"], "base\n");

    let denied = context_with_write(
        &root,
        storage.clone(),
        "conversation-update",
        Some("project-update"),
        "run-update",
        AgentWritePermission::Denied,
    );
    assert_eq!(
        status(&denied, "apply_patch", modify_id.clone()).unwrap()["status"],
        "drafting"
    );
    let denied_append = append(
        &call_context(&denied, "denied-append"),
        "apply_patch",
        modify_id.clone(),
        0,
        0,
        "x".into(),
        content_digest(b"denied"),
    )
    .unwrap_err();
    assert_eq!(
        denied_append.code(),
        Some("agent.apply_patch.permission_denied")
    );
    let aborted = abort(&denied, "apply_patch", modify_id).unwrap();
    assert_eq!(aborted["status"], "aborted");
    assert_eq!(aborted["requiresCommitBeforeResponse"], false);

    let rewrite_observation = observe(&owner, "existing.txt", "read-rewrite");
    let rewrite = begin(
        &call_context(&owner, "begin-rewrite"),
        StagedSource::new("apply_patch", content_digest(b"begin-rewrite")),
        FileChangeOperation::Update,
        Some(StagedUpdateStrategy::Rewrite),
        "existing.txt".into(),
        Some(rewrite_observation),
        None,
    )
    .unwrap();
    assert_eq!(rewrite["strategy"], "rewrite");
    assert_eq!(rewrite["byteCount"], 0);
    assert_eq!(rewrite["tail"], "");
}
