use super::*;
use crate::storage::pending_action_repository::PendingActionStoreOutcome;

fn file_change_json_pair(parent: &Path, staged: bool) -> (String, String) {
    use crate::file_change::{
        content_digest, FileChangeCommit, FileChangeContentState, FileChangeDirectBinding,
        FileChangeOperation, FileChangeOutcome, FileChangeProposal, FileChangeReceipt,
        FileChangeStatus, FileChangeTransaction, FileObservationCheckpoint,
        FileObservationDirectoryIdentity, FileObservationState, FILE_CHANGE_SCHEMA_VERSION,
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
    let presentation_diff = "+after\n";
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
        schema_version: crate::file_change::FILE_CHANGE_DIRECT_BINDING_SCHEMA_VERSION,
        transaction,
        proposal: FileChangeProposal {
            schema_version: FILE_CHANGE_SCHEMA_VERSION,
            id: "call-file-change-1".to_string(),
            transaction_id: "transaction-1".to_string(),
            operation: FileChangeOperation::Create,
            file_path: "report.md".to_string(),
            base: FileChangeContentState::Missing,
            target: target.clone(),
            diff_digest: crate::file_change::diff_digest(presentation_diff),
            proposal_digest: proposal_digest.clone(),
            additions: 1,
            deletions: 0,
        },
        observation_id: format!("fobs_{}", "0".repeat(32)),
        observation: FileObservationCheckpoint {
            schema_version: FILE_OBSERVATION_CHECKPOINT_SCHEMA_VERSION,
            observation_id: format!("fobs_{}", "0".repeat(32)),
            source_tool_call_id: "call-read-1".to_string(),
            conversation_id: "conversation-1".to_string(),
            run_id: "run-1".to_string(),
            canonical_target: canonical_target.clone(),
            state: FileObservationState::Missing,
            parent_directory_identity: FileObservationDirectoryIdentity::read(parent).unwrap(),
            created_at_ms: now,
            expires_at_ms: now + FILE_OBSERVATION_TTL_MS,
        },
        source_tool_name: "apply_patch".to_string(),
        source_call_id: "call-file-change-1".to_string(),
        source_args_digest: content_digest(b"args"),
        trace_args_digest: content_digest(b"trace-args"),
        staged_transaction_id: staged.then(|| "transaction-1".to_string()),
        conversation_id: "conversation-1".to_string(),
        project_id: None,
        run_id: "run-1".to_string(),
        staged_transaction_revision: staged.then_some(1),
        canonical_target,
        base_content: None,
        target_content: Some(content.to_string()),
        delete_journal: None,
        receipt: None,
        permission_revision: "permission-v1".to_string(),
        tool_set_revision: "tool-set-v1".to_string(),
        provider_wire_revision: "provider-protocol-v1".to_string(),
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
    let action = |execution| AgentProposedAction::FileChange {
        file_change: crate::AgentFileChangeProposal {
            schema_version: crate::AGENT_FILE_CHANGE_PROTOCOL_SCHEMA_VERSION,
            id: "call-file-change-1".to_string(),
            transaction_id: "transaction-1".to_string(),
            operation: crate::AgentFileChangeOperation::Create,
            update_strategy: None,
            file_path: "report.md".to_string(),
            inline_diff: (!staged).then(|| crate::AgentGitDiffSnapshot {
                patch: presentation_diff.to_string(),
                truncated: false,
            }),
            base_revision: None,
            summary: Some("Create report".to_string()),
            additions: 1,
            deletions: 0,
            line_count: 1,
            byte_count: content.len() as u64,
            approval_status: crate::AgentApprovalStatus::Required,
            execution: Box::new(execution),
        },
    };
    (
        serde_json::to_string(&action(prepared_binding)).unwrap(),
        serde_json::to_string(&action(committed_binding)).unwrap(),
    )
}

fn pending(action_id: &str, action_json: &str, status: &str) -> AgentPendingActionRecord {
    AgentPendingActionRecord {
        action_id: action_id.to_string(),
        run_id: "run-1".to_string(),
        conversation_id: Some("conversation-1".to_string()),
        assistant_message_id: Some("message-1".to_string()),
        action_type: "file_change".to_string(),
        tool_name: "apply_patch".to_string(),
        tool_call_id: Some("call-file-change-1".to_string()),
        status: status.to_string(),
        target_status: None,
        action_json: action_json.to_string(),
        agent_input_json: r#"{"checkpoint":"current"}"#.to_string(),
        created_at: 10,
        updated_at: 20,
    }
}

fn audit(action_id: &str, action_json: &str, source: &str, status: &str) -> AgentActionAuditRecord {
    AgentActionAuditRecord {
        action_id: action_id.to_string(),
        run_id: "run-1".to_string(),
        conversation_id: Some("conversation-1".to_string()),
        assistant_message_id: Some("message-1".to_string()),
        action_type: "file_change".to_string(),
        tool_name: "apply_patch".to_string(),
        decision: (source == "auto").then(|| "approved".to_string()),
        status: status.to_string(),
        action_json: action_json.to_string(),
        file_change_result_json: None,
        command_result_json: None,
        tool_result_json: None,
        error: None,
        created_at: 10,
        decided_at: (source == "auto").then_some(10),
        completed_at: None,
        effective_permissions_json: Some(r#"{"write":"workspace_only"}"#.to_string()),
        path_scope: Some("workspace".to_string()),
        command_cwd_scope: None,
        blocked_reason: None,
        decision_source: Some(source.to_string()),
    }
}

#[test]
fn current_file_change_pending_json_cas_is_exact_for_direct_and_staged() {
    for staged in [false, true] {
        let fixture = StorageFixture::new();
        let service = fixture.service();
        let (prepared, committed) = file_change_json_pair(&fixture.root, staged);
        let action_id = crate::canonical_pending_action_id("run-1", "call-file-change-1");
        let current = pending(&action_id, &prepared, "executing");
        service.store_pending_agent_action(current.clone()).unwrap();
        let manual_audit = audit(&action_id, &prepared, "manual_pending", "pending");
        assert!(service
            .insert_agent_action_audit_if_absent(manual_audit)
            .unwrap());
        assert_eq!(
            service
                .commit_pending_direct_file_change_action_json(&current, &prepared, &committed, 30,)
                .unwrap(),
            AgentPendingActionJsonCommitOutcome::Updated
        );
        assert_eq!(
            service
                .commit_pending_direct_file_change_action_json(&current, &prepared, &committed, 30,)
                .unwrap(),
            AgentPendingActionJsonCommitOutcome::AlreadyCommitted
        );
    }
}

#[test]
fn automatic_file_change_claims_pending_and_audit_before_commit() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let (prepared_required, committed_required) = file_change_json_pair(&fixture.root, false);
    let approve = |raw: &str| {
        let mut action = serde_json::from_str::<AgentProposedAction>(raw).unwrap();
        let AgentProposedAction::FileChange { file_change } = &mut action else {
            unreachable!()
        };
        file_change.approval_status = crate::AgentApprovalStatus::Approved;
        serde_json::to_string(&action).unwrap()
    };
    let prepared = approve(&prepared_required);
    let committed = approve(&committed_required);
    let action_id = crate::canonical_pending_action_id("run-1", "call-file-change-1");
    let approved_pending = pending(&action_id, &prepared, "approved");
    let approved_audit = audit(&action_id, &prepared, "auto", "approved");
    assert_eq!(
        service
            .store_auto_file_change_pending_action_with_audit(
                approved_pending.clone(),
                approved_audit.clone(),
            )
            .unwrap(),
        PendingActionStoreOutcome::Inserted
    );
    assert!(service
        .claim_auto_file_change_pending_execution(
            &approved_pending,
            &approved_audit,
            None,
            None,
            None,
            r#"{"checkpoint":"current"}"#,
            30,
        )
        .unwrap());
    let mut executing = approved_pending;
    executing.status = "executing".to_string();
    executing.updated_at = 30;
    assert_eq!(
        service
            .commit_pending_direct_file_change_action_json(&executing, &prepared, &committed, 40,)
            .unwrap(),
        AgentPendingActionJsonCommitOutcome::Updated
    );
    let connection = service.state.connection().unwrap();
    assert_eq!(
        pending_action_repository::load_pending_action(&connection, &action_id)
            .unwrap()
            .unwrap()
            .action_json,
        committed
    );
    let stored_audit =
        agent_action_audit_repository::load_action_audit_record(&connection, &action_id)
            .unwrap()
            .unwrap();
    assert_eq!(stored_audit.status, "executing");
    assert_eq!(stored_audit.action_json, committed);
    assert_eq!(stored_audit.decision_source.as_deref(), Some("auto"));
}

#[test]
fn retired_file_change_action_and_tool_identities_are_rejected() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let (prepared, committed) = file_change_json_pair(&fixture.root, false);
    for (action_type, tool_name) in [
        ("diff", "apply_patch"),
        ("file_write", "apply_patch"),
        ("file_change", "write_file"),
    ] {
        let mut retired = pending("retired", &prepared, "executing");
        retired.action_type = action_type.to_string();
        retired.tool_name = tool_name.to_string();
        assert!(service
            .commit_pending_direct_file_change_action_json(&retired, &prepared, &committed, 30,)
            .is_err());
    }
}
