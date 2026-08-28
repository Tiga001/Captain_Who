use super::*;

fn direct_file_change_json_pair(parent: &Path) -> (String, String) {
    direct_file_change_json_pair_with_summary(parent, "Create report")
}

fn direct_file_change_json_pair_with_summary(parent: &Path, summary: &str) -> (String, String) {
    use crate::file_change::{
        content_digest, FileChangeCommit, FileChangeContentState, FileChangeDirectBinding,
        FileChangeOperation, FileChangeOutcome, FileChangeProposal, FileChangeReceipt,
        FileChangeStatus, FileChangeTransaction, FileObservationCheckpoint,
        FileObservationIdentity, FileObservationState, FILE_CHANGE_SCHEMA_VERSION,
        FILE_OBSERVATION_CHECKPOINT_SCHEMA_VERSION, FILE_OBSERVATION_TTL_MS,
    };

    let now: u64 = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis()
        .try_into()
        .unwrap();
    let content = "after\n";
    let target = FileChangeContentState::Present {
        revision: crate::content_revision(content.as_bytes()),
        digest: content_digest(content.as_bytes()),
        byte_count: content.len() as u64,
    };
    let proposal_digest = content_digest(b"proposal");
    let canonical_target = parent.join("report.md").to_string_lossy().into_owned();
    let transaction = FileChangeTransaction {
        schema_version: FILE_CHANGE_SCHEMA_VERSION,
        id: "transaction-1".to_string(),
        operation: FileChangeOperation::Create,
        file_path: "report.md".to_string(),
        status: FileChangeStatus::WaitingApproval,
        outcome: FileChangeOutcome::DefinitelyNotExecuted,
        base: FileChangeContentState::Missing,
        target: target.clone(),
        proposal_digest: proposal_digest.clone(),
        created_at: now,
        updated_at: now,
    };
    let prepared_binding = FileChangeDirectBinding {
        schema_version: FILE_CHANGE_SCHEMA_VERSION,
        transaction,
        proposal: FileChangeProposal {
            schema_version: FILE_CHANGE_SCHEMA_VERSION,
            id: "diff-1".to_string(),
            transaction_id: "transaction-1".to_string(),
            operation: FileChangeOperation::Create,
            file_path: "report.md".to_string(),
            base: FileChangeContentState::Missing,
            target: target.clone(),
            diff_digest: content_digest(b"diff"),
            proposal_digest: proposal_digest.clone(),
            additions: 1,
            deletions: 0,
        },
        observation_id: format!("fobs_{}", "0".repeat(32)),
        observation: FileObservationCheckpoint {
            schema_version: FILE_OBSERVATION_CHECKPOINT_SCHEMA_VERSION,
            observation_id: format!("fobs_{}", "0".repeat(32)),
            source_tool_call_id: "read-1".to_string(),
            conversation_id: "conversation-1".to_string(),
            run_id: "run-1".to_string(),
            canonical_target: canonical_target.clone(),
            state: FileObservationState::Missing,
            parent_identity: FileObservationIdentity::from_metadata(
                &std::fs::metadata(parent).unwrap(),
            ),
            created_at_ms: now,
            expires_at_ms: now + FILE_OBSERVATION_TTL_MS,
        },
        source_call_id: "diff-1".to_string(),
        source_args_digest: content_digest(b"args"),
        conversation_id: "conversation-1".to_string(),
        run_id: "run-1".to_string(),
        canonical_target,
        base_content: None,
        target_content: Some(content.to_string()),
        delete_journal: None,
        receipt: None,
        permission_revision: "permission-v1".to_string(),
        tool_set_revision: "tool-set-v1".to_string(),
    };
    prepared_binding.validate().unwrap();
    let committed_binding = prepared_binding
        .with_commit(&FileChangeCommit {
            status: FileChangeStatus::Applied,
            receipt: FileChangeReceipt {
                schema_version: FILE_CHANGE_SCHEMA_VERSION,
                transaction_id: "transaction-1".to_string(),
                operation: FileChangeOperation::Create,
                file_path: "report.md".to_string(),
                outcome: FileChangeOutcome::Applied,
                base: FileChangeContentState::Missing,
                target,
                proposal_digest,
                committed_at: now + 1,
            },
            delete_journal: None,
        })
        .unwrap();
    let proposal = |execution| AgentProposedAction::Diff {
        diff: crate::AgentDiffProposal {
            id: "diff-1".to_string(),
            operation: crate::AgentPatchOperation::Create,
            file_path: "report.md".to_string(),
            patch: "+after\n".to_string(),
            base_revision: None,
            summary: Some(summary.to_string()),
            approval_status: crate::AgentApprovalStatus::Required,
            execution: Box::new(execution),
        },
    };
    (
        serde_json::to_string(&proposal(prepared_binding)).unwrap(),
        serde_json::to_string(&proposal(committed_binding)).unwrap(),
    )
}

fn manual_diff(action_id: &str, action_json: &str) -> AgentPendingActionRecord {
    AgentPendingActionRecord {
        action_id: action_id.to_string(),
        run_id: "run-1".to_string(),
        conversation_id: Some("conversation-1".to_string()),
        assistant_message_id: Some("message-1".to_string()),
        action_type: "diff".to_string(),
        tool_name: "apply_patch".to_string(),
        tool_call_id: Some("diff-1".to_string()),
        status: "executing".to_string(),
        target_status: None,
        action_json: action_json.to_string(),
        agent_input_json: r#"{"checkpoint":"current"}"#.to_string(),
        created_at: 10,
        updated_at: 20,
    }
}

fn automatic_diff(action_id: &str, action_json: &str) -> AgentActionAuditRecord {
    AgentActionAuditRecord {
        action_id: action_id.to_string(),
        run_id: "run-1".to_string(),
        conversation_id: Some("conversation-1".to_string()),
        assistant_message_id: Some("message-1".to_string()),
        action_type: "diff".to_string(),
        tool_name: "apply_patch".to_string(),
        decision: Some("approved".to_string()),
        status: "executing".to_string(),
        action_json: action_json.to_string(),
        patch_result_json: None,
        command_result_json: None,
        tool_result_json: None,
        error: None,
        created_at: 10,
        decided_at: Some(10),
        completed_at: None,
        effective_permissions_json: Some(r#"{"write":"workspace_only"}"#.to_string()),
        path_scope: Some("workspace".to_string()),
        command_cwd_scope: None,
        blocked_reason: None,
        decision_source: Some("auto".to_string()),
    }
}

fn manual_initial_audit(action_id: &str, action_json: &str) -> AgentActionAuditRecord {
    let mut audit = automatic_diff(action_id, action_json);
    audit.decision = None;
    audit.status = "pending".to_string();
    audit.decided_at = None;
    audit.decision_source = Some("manual_pending".to_string());
    audit
}

#[test]
fn manual_direct_file_change_json_cas_survives_reopen_and_fails_closed() {
    let fixture = StorageFixture::new();
    let (prepared, committed) = direct_file_change_json_pair(&fixture.root);
    let service = fixture.service();
    let expected = manual_diff("manual-diff", &prepared);
    service
        .store_pending_agent_action(expected.clone())
        .unwrap();
    assert!(service
        .insert_agent_action_audit_if_absent(manual_initial_audit("manual-diff", &prepared))
        .unwrap());
    assert_eq!(
        service
            .commit_pending_direct_file_change_action_json(&expected, &prepared, &committed, 30,)
            .unwrap(),
        AgentPendingActionJsonCommitOutcome::Updated
    );

    let reopened = StorageService::open(&fixture.root.join("storage.sqlite")).unwrap();
    assert_eq!(
        agent_action_audit_repository::load_action_audit_record(
            &reopened.state.connection().unwrap(),
            "manual-diff",
        )
        .unwrap()
        .unwrap()
        .action_json,
        committed
    );
    assert_eq!(
        reopened
            .commit_pending_direct_file_change_action_json(&expected, &prepared, &committed, 30,)
            .unwrap(),
        AgentPendingActionJsonCommitOutcome::AlreadyCommitted
    );

    let stored = manual_diff("manual-stale", &prepared);
    reopened.store_pending_agent_action(stored.clone()).unwrap();
    let mut stale_expected = stored.clone();
    let (stale_prepared, stale_committed) =
        direct_file_change_json_pair_with_summary(&fixture.root, "older prepared action");
    stale_expected.action_json = stale_prepared;
    assert_eq!(
        reopened
            .commit_pending_direct_file_change_action_json(
                &stale_expected,
                &stale_expected.action_json,
                &stale_committed,
                30,
            )
            .unwrap(),
        AgentPendingActionJsonCommitOutcome::ExpectedActionMismatch
    );

    let conflicted = manual_diff("manual-audit-conflict", &prepared);
    reopened
        .store_pending_agent_action(conflicted.clone())
        .unwrap();
    let (other_prepared, _) =
        direct_file_change_json_pair_with_summary(&fixture.root, "conflicting audit action");
    assert!(reopened
        .insert_agent_action_audit_if_absent(manual_initial_audit(
            "manual-audit-conflict",
            &other_prepared,
        ))
        .unwrap());
    assert!(reopened
        .commit_pending_direct_file_change_action_json(&conflicted, &prepared, &committed, 30,)
        .is_err());
    let persisted_pending = pending_action_repository::load_pending_action(
        &reopened.state.connection().unwrap(),
        "manual-audit-conflict",
    )
    .unwrap()
    .unwrap();
    assert_eq!(persisted_pending.action_json, prepared);
    assert_eq!(persisted_pending.updated_at, 20);

    let without_audit = manual_diff("manual-without-audit", &prepared);
    reopened
        .store_pending_agent_action(without_audit.clone())
        .unwrap();
    assert_eq!(
        reopened
            .commit_pending_direct_file_change_action_json(
                &without_audit,
                &prepared,
                &committed,
                30,
            )
            .unwrap(),
        AgentPendingActionJsonCommitOutcome::Updated,
        "best-effort manual audit absence must not strand an executing pending action"
    );

    let mut approved = manual_diff("manual-approved", &prepared);
    approved.status = "approved".to_string();
    reopened
        .store_pending_agent_action(approved.clone())
        .unwrap();
    approved.status = "executing".to_string();
    assert_eq!(
        reopened
            .commit_pending_direct_file_change_action_json(&approved, &prepared, &committed, 30,)
            .unwrap(),
        AgentPendingActionJsonCommitOutcome::NotExecuting {
            status: "approved".to_string()
        }
    );
}

#[test]
fn automatic_direct_file_change_json_cas_survives_reopen_and_fails_closed() {
    let fixture = StorageFixture::new();
    let (prepared, committed) = direct_file_change_json_pair(&fixture.root);
    let service = fixture.service();
    let expected = automatic_diff("automatic-diff", &prepared);
    assert_eq!(
        service
            .claim_agent_action_audit_execution(expected.clone())
            .unwrap(),
        agent_action_audit_repository::AgentActionAuditExecutionClaimOutcome::Claimed
    );
    assert_eq!(
        service
            .commit_automatic_direct_file_change_action_json(&expected, &prepared, &committed,)
            .unwrap(),
        AgentActionAuditJsonCommitOutcome::Updated
    );

    let reopened = StorageService::open(&fixture.root.join("storage.sqlite")).unwrap();
    assert_eq!(
        reopened
            .commit_automatic_direct_file_change_action_json(&expected, &prepared, &committed,)
            .unwrap(),
        AgentActionAuditJsonCommitOutcome::AlreadyCommitted
    );

    let stored = automatic_diff("automatic-stale", &prepared);
    reopened
        .claim_agent_action_audit_execution(stored.clone())
        .unwrap();
    let mut stale_expected = stored.clone();
    let (stale_prepared, stale_committed) =
        direct_file_change_json_pair_with_summary(&fixture.root, "older prepared action");
    stale_expected.action_json = stale_prepared;
    assert_eq!(
        reopened
            .commit_automatic_direct_file_change_action_json(
                &stale_expected,
                &stale_expected.action_json,
                &stale_committed,
            )
            .unwrap(),
        AgentActionAuditJsonCommitOutcome::ExpectedActionMismatch
    );

    let mut approved = automatic_diff("automatic-approved", &prepared);
    approved.status = "approved".to_string();
    assert!(reopened
        .insert_agent_action_audit_if_absent(approved.clone())
        .unwrap());
    approved.status = "executing".to_string();
    assert_eq!(
        reopened
            .commit_automatic_direct_file_change_action_json(&approved, &prepared, &committed,)
            .unwrap(),
        AgentActionAuditJsonCommitOutcome::NotExecuting {
            status: "approved".to_string()
        }
    );
}
