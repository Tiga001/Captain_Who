use super::file_change_permissions::{
    bind_direct_execution_to_input, direct_execution_input, seed_durable_direct_file_change_owner,
    FILE_CHANGE_REJECTION_CASES,
};
use super::*;

fn bind_staged_execution_to_input(proposal: &mut AgentFileChangeProposal, input: &AgentChatInput) {
    proposal.execution.permission_revision =
        mycopilot_core::file_change::proposal_digest(&permissions_from_input(input))
            .expect("digest the exact frozen permission snapshot");
    proposal.execution.tool_set_revision = input
        .resume_checkpoint
        .as_ref()
        .expect("Staged execution fixture has a frozen checkpoint")
        .tool_set
        .effective_revision
        .clone();
    proposal.execution.provider_wire_revision = input
        .provider_configuration_revision
        .clone()
        .or_else(|| {
            input.provider_protocol_key.as_ref().map(|protocol| {
                mycopilot_core::file_change::proposal_digest(protocol)
                    .expect("digest the exact Provider wire snapshot")
            })
        })
        .expect("Staged execution fixture has Provider wire identity");
    proposal
        .execution
        .validate()
        .expect("Staged execution binding remains internally valid");
}

fn pending_file_change_record(
    run_id: &str,
    conversation_id: &str,
    file_change: AgentFileChangeProposal,
    call: &AgentToolCall,
    agent_input: AgentChatInput,
) -> PendingActionRecord {
    PendingActionRecord {
        storage_id: pending_action_storage_id(run_id, &call.id),
        snapshot: PendingAgentActionSnapshot {
            action_id: call.id.clone(),
            action_type: "file_change".to_string(),
            tool_name: call.tool.clone(),
            tool_call_id: Some(call.id.clone()),
            run_id: run_id.to_string(),
            conversation_id: Some(conversation_id.to_string()),
            assistant_message_id: Some(format!("assistant-{run_id}")),
            action: AgentProposedAction::FileChange { file_change },
            created_at: 1,
            status: PendingActionStatus::Pending,
        },
        agent_input,
    }
}

fn save_file_change_conversation(storage: &StorageService, conversation_id: &str) {
    storage
        .save_conversation(ChatConversationRecord {
            id: conversation_id.to_string(),
            project_id: None,
            model_id: None,
            title: "Staged FileChange execution fixture".to_string(),
            messages: Vec::new(),
            created_at: 1,
            updated_at: 1,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
}

fn staged_update_file_change_fixture(
    identity: StagedFileChangeFixtureIdentity<'_>,
    paths: (&str, &str),
    base_content: &str,
    target_content: &str,
    approval_status: AgentApprovalStatus,
) -> (AgentFileChangeProposal, AgentToolCall) {
    let StagedFileChangeFixtureIdentity {
        run_id,
        conversation_id,
        call_id,
        transaction_id,
    } = identity;
    let (action, _) = direct_file_change_fixture(
        run_id,
        conversation_id,
        call_id,
        paths,
        Some(base_content),
        Some(target_content),
        approval_status,
    );
    let AgentProposedAction::FileChange { file_change } = action else {
        unreachable!("the fixture constructor returns a FileChange")
    };
    let mut execution = *file_change.execution;
    execution.transaction.id = transaction_id.to_string();
    execution.proposal.transaction_id = transaction_id.to_string();
    execution.staged_transaction_id = Some(transaction_id.to_string());
    execution.staged_transaction_revision = Some(1);
    let args = json!({
        "request": {
            "action": "commit",
            "transactionId": transaction_id,
            "expectedDraftRevision": 1,
        }
    });
    execution.source_args_digest = mycopilot_core::file_change::proposal_digest(&args)
        .expect("digest staged update Tool Call arguments");
    execution.trace_args_digest =
        mycopilot_core::file_change_support::apply_patch_trace_args_digest(&args)
            .expect("digest staged update durable Trace arguments");
    execution
        .validate()
        .expect("valid staged update FileChange execution binding");
    let proposal = AgentFileChangeProposal {
        schema_version: mycopilot_core::AGENT_FILE_CHANGE_PROTOCOL_SCHEMA_VERSION,
        id: call_id.to_string(),
        transaction_id: transaction_id.to_string(),
        operation: AgentFileChangeOperation::Update,
        update_strategy: Some(AgentFileChangeUpdateStrategy::Modify),
        file_path: paths.0.to_string(),
        inline_diff: None,
        base_revision: execution.transaction.base.revision().map(str::to_string),
        summary: Some("test Staged update".to_string()),
        additions: execution.proposal.additions,
        deletions: execution.proposal.deletions,
        line_count: target_content.lines().count() as u64,
        byte_count: target_content.len() as u64,
        approval_status,
        execution: Box::new(execution),
    };
    proposal
        .validate()
        .expect("valid Staged update FileChange proposal");
    let call = AgentToolCall {
        id: call_id.to_string(),
        tool: "apply_patch".to_string(),
        args,
        approval_status,
        reason: None,
    };
    (proposal, call)
}

fn assert_staged_change_applied(decision: &ActionExecutionDecision) {
    let result = decision
        .file_change_result
        .as_ref()
        .expect("Staged file change returns a canonical result");
    assert_eq!(
        decision.status, "applied",
        "Staged execution failed: {result:?}"
    );
    assert_eq!(
        decision.final_pending_status,
        PendingActionStatus::Completed
    );
    assert_eq!(
        result.status,
        mycopilot_core::AgentFileChangeResultStatus::Applied
    );
    assert!(decision.file_change.is_some());
    assert!(decision.tool_result.ok);
    assert_eq!(decision.tool_result.tool, "apply_patch");
    let AgentProposedAction::FileChange { file_change } = decision
        .committed_file_change_action
        .as_ref()
        .expect("the common FileChange committer returns a durable successor binding")
    else {
        panic!("Staged execution must retain the canonical FileChange proposal")
    };
    assert_eq!(
        file_change.execution.transaction.status,
        mycopilot_core::file_change::FileChangeStatus::Applied
    );
    assert!(file_change.execution.receipt.is_some());
}

#[test]
fn manual_staged_apply_patch_commit_uses_the_common_file_change_committer() {
    let fixture = tempdir().unwrap();
    let workspace = fixture.path().join("workspace");
    fs::create_dir(&workspace).unwrap();
    let workspace = fs::canonicalize(workspace).unwrap();
    let storage = StorageService::open(&fixture.path().join("storage.sqlite")).unwrap();
    let target = workspace.join("manual-staged.txt");
    let run_id = "run-manual-staged-commit";
    let conversation_id = "conversation-manual-staged-commit";
    let transaction_id = "transaction-manual-staged-commit";
    let (mut proposal, call) = staged_file_change_fixture(
        StagedFileChangeFixtureIdentity {
            run_id,
            conversation_id,
            call_id: "call-manual-staged-commit",
            transaction_id,
        },
        ("manual-staged.txt", target.to_str().unwrap()),
        "manual staged content\n",
        AgentApprovalStatus::Required,
    );
    let mut input = direct_execution_input(
        &workspace,
        run_id,
        conversation_id,
        AgentPermissions {
            write: AgentWritePermission::WorkspaceOnly,
            patch: mycopilot_core::AgentPatchPermission::RequireApproval,
            ..Default::default()
        },
        &call,
    );
    save_test_pending_provider_for_input(&storage, &mut input);
    bind_staged_execution_to_input(&mut proposal, &input);
    save_file_change_conversation(&storage, conversation_id);
    storage
        .create_agent_file_change(staged_file_change_record(&proposal, "waiting_approval"))
        .unwrap();
    let action = AgentProposedAction::FileChange {
        file_change: proposal,
    };
    authorize_file_change_action(&input, &action, FileChangeAuthorizationSource::ExplicitUser)
        .unwrap();
    let AgentProposedAction::FileChange { file_change } = action else {
        unreachable!()
    };
    let record = pending_file_change_record(run_id, conversation_id, file_change, &call, input);
    let restored_call = tool_call_for_pending_record(&record).unwrap();
    assert_eq!(restored_call.args, call.args);

    let decision = action_execution_for_decision(
        &storage,
        &record,
        &restored_call,
        AgentApprovalDecisionStatus::Approved,
        None,
    );

    assert_staged_change_applied(&decision);
    assert_eq!(
        fs::read_to_string(&target).unwrap(),
        "manual staged content\n"
    );
    assert_eq!(
        storage
            .get_agent_file_change(transaction_id)
            .unwrap()
            .unwrap()
            .status,
        "applied"
    );
}

#[tokio::test]
async fn staged_approval_response_lost_retry_replays_receipt_without_recommitting_the_draft() {
    let fixture = tempdir().unwrap();
    let workspace = fixture.path().join("workspace");
    fs::create_dir(&workspace).unwrap();
    let workspace = fs::canonicalize(workspace).unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let service = AgentService::new(Arc::clone(&storage));
    let target = workspace.join("staged-retry.txt");
    let run_id = "run-staged-response-lost";
    let conversation_id = "conversation-staged-response-lost";
    let assistant_message_id = "assistant-staged-response-lost";
    let call_id = "call-staged-response-lost";
    let transaction_id = "transaction-staged-response-lost";
    let (mut proposal, call) = staged_file_change_fixture(
        StagedFileChangeFixtureIdentity {
            run_id,
            conversation_id,
            call_id,
            transaction_id,
        },
        ("staged-retry.txt", target.to_str().unwrap()),
        "staged exactly once\n",
        AgentApprovalStatus::Required,
    );
    let mut input = direct_execution_input(
        &workspace,
        run_id,
        conversation_id,
        AgentPermissions {
            write: AgentWritePermission::WorkspaceOnly,
            patch: mycopilot_core::AgentPatchPermission::RequireApproval,
            ..Default::default()
        },
        &call,
    );
    save_test_pending_provider_for_input(&storage, &mut input);
    bind_staged_execution_to_input(&mut proposal, &input);
    seed_durable_direct_file_change_owner(
        &storage,
        &mut input,
        conversation_id,
        assistant_message_id,
        run_id,
        &call,
    );
    storage
        .create_agent_file_change(staged_file_change_record(&proposal, "waiting_approval"))
        .unwrap();
    service
        .store_pending_action(
            run_id,
            conversation_id,
            assistant_message_id,
            AgentProposedAction::FileChange {
                file_change: proposal,
            },
            input,
        )
        .unwrap();
    let (notifications, _receiver) = tokio::sync::mpsc::unbounded_channel();

    let first = service
        .approve_action_with_scope(
            run_id,
            call_id,
            mycopilot_protocol_rs::AgentApprovalScopeDto::SingleAction,
            notifications.clone(),
        )
        .unwrap();
    let transaction_after_first = storage
        .get_agent_file_change(transaction_id)
        .unwrap()
        .unwrap();
    let metadata_after_first = fs::metadata(&target).unwrap();
    let retry = service
        .approve_action_with_scope(
            run_id,
            call_id,
            mycopilot_protocol_rs::AgentApprovalScopeDto::SingleAction,
            notifications,
        )
        .expect("the staged response-lost retry replays its terminal receipt");
    let transaction_after_retry = storage
        .get_agent_file_change(transaction_id)
        .unwrap()
        .unwrap();

    assert_eq!(first.status, "applied");
    assert_eq!(retry.status, "applied");
    assert_eq!(
        serde_json::to_value(&first.file_change_result).unwrap(),
        serde_json::to_value(&retry.file_change_result).unwrap()
    );
    assert_eq!(
        fs::read_to_string(&target).unwrap(),
        "staged exactly once\n"
    );
    assert_eq!(transaction_after_retry.status, "applied");
    assert_eq!(
        transaction_after_retry.draft_revision,
        transaction_after_first.draft_revision
    );
    assert_eq!(
        transaction_after_retry.next_mutation_index,
        transaction_after_first.next_mutation_index
    );
    assert_eq!(
        fs::metadata(&target).unwrap().modified().unwrap(),
        metadata_after_first.modified().unwrap()
    );
}

#[tokio::test]
async fn remaining_run_approval_from_staged_commit_authorizes_later_direct_changes() {
    let fixture = tempdir().unwrap();
    let workspace = fixture.path().join("workspace");
    fs::create_dir(&workspace).unwrap();
    let workspace = fs::canonicalize(workspace).unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let service = AgentService::new(Arc::clone(&storage));
    let run_id = "run-staged-remaining-approval";
    let conversation_id = "conversation-staged-remaining-approval";
    let assistant_message_id = "assistant-staged-remaining-approval";
    let call_id = "call-staged-remaining-approval";
    let transaction_id = "transaction-staged-remaining-approval";
    let target = workspace.join("staged-grant.txt");
    let (mut proposal, call) = staged_file_change_fixture(
        StagedFileChangeFixtureIdentity {
            run_id,
            conversation_id,
            call_id,
            transaction_id,
        },
        ("staged-grant.txt", target.to_str().unwrap()),
        "content committed from the staged transaction\n",
        AgentApprovalStatus::Required,
    );
    let expected_trace_digest = proposal.execution.trace_args_digest.clone();
    let mut input = direct_execution_input(
        &workspace,
        run_id,
        conversation_id,
        AgentPermissions {
            write: AgentWritePermission::WorkspaceOnly,
            patch: mycopilot_core::AgentPatchPermission::RequireApproval,
            ..Default::default()
        },
        &call,
    );
    save_test_pending_provider_for_input(&storage, &mut input);
    bind_staged_execution_to_input(&mut proposal, &input);
    seed_durable_direct_file_change_owner(
        &storage,
        &mut input,
        conversation_id,
        assistant_message_id,
        run_id,
        &call,
    );
    storage
        .create_agent_file_change(staged_file_change_record(&proposal, "waiting_approval"))
        .unwrap();
    service
        .store_pending_action(
            run_id,
            conversation_id,
            assistant_message_id,
            AgentProposedAction::FileChange {
                file_change: proposal,
            },
            input,
        )
        .unwrap();

    let trace = storage
        .get_conversation_turn_trace(assistant_message_id)
        .unwrap()
        .expect("the staged approval fixture has one durable Trace");
    let trace_operation = trace
        .items
        .iter()
        .find_map(|item| match item {
            ConversationTurnTraceItem::ToolCall {
                call_id: candidate,
                operation,
                ..
            } if candidate == call_id => Some(operation),
            _ => None,
        })
        .expect("the staged commit has one durable ToolCall");
    assert_eq!(
        trace_operation,
        &mycopilot_core::file_change_support::apply_patch_trace_operation(&call.args).unwrap()
    );
    assert_eq!(trace_operation["request"]["action"], "commit");
    assert_eq!(
        trace_operation["request"]["changeRepresentation"],
        "metadata_only"
    );
    assert_eq!(
        mycopilot_core::file_change::proposal_digest(trace_operation).unwrap(),
        expected_trace_digest
    );

    let approved = service
        .approve_action_with_scope(
            run_id,
            call_id,
            mycopilot_protocol_rs::AgentApprovalScopeDto::RemainingApplyPatchInRun,
            tokio::sync::mpsc::unbounded_channel().0,
        )
        .expect("the staged commit must settle and activate its remaining-Run grant");
    assert_eq!(approved.status, "applied");
    assert_eq!(
        fs::read_to_string(&target).unwrap(),
        "content committed from the staged transaction\n"
    );
    storage
        .get_active_file_change_run_grant(run_id)
        .unwrap()
        .expect("the safely projected staged commit receipt activates the Run grant");

    for (successor_call_id, paths, base, target_content) in [
        (
            "call-after-staged-create",
            (
                "created-after-staged.txt",
                workspace
                    .join("created-after-staged.txt")
                    .to_string_lossy()
                    .into_owned(),
            ),
            None,
            "created under the remaining-Run grant\n",
        ),
        (
            "call-after-staged-update",
            ("staged-grant.txt", target.to_string_lossy().into_owned()),
            Some("content committed from the staged transaction\n"),
            "updated under the remaining-Run grant\n",
        ),
    ] {
        let (mut successor, successor_call) = direct_file_change_fixture(
            run_id,
            conversation_id,
            successor_call_id,
            (paths.0, paths.1.as_str()),
            base,
            Some(target_content),
            AgentApprovalStatus::Required,
        );
        let mut successor_input = direct_execution_input(
            &workspace,
            run_id,
            conversation_id,
            AgentPermissions {
                write: AgentWritePermission::WorkspaceOnly,
                patch: mycopilot_core::AgentPatchPermission::RequireApproval,
                ..Default::default()
            },
            &successor_call,
        );
        save_test_pending_provider_for_input(&storage, &mut successor_input);
        bind_direct_execution_to_input(&mut successor, &successor_input);
        let AgentProposedAction::FileChange {
            file_change: successor,
        } = successor
        else {
            unreachable!("Direct fixture always produces a FileChange")
        };
        storage
            .resolve_active_file_change_run_grant(
                &successor,
                successor_input.context.as_ref().unwrap(),
            )
            .unwrap()
            .unwrap_or_else(|| {
                panic!("same-Run Direct successor {successor_call_id} must inherit the grant")
            });
    }
}

#[tokio::test]
async fn staged_audit_failure_projects_the_same_outcome_unknown_receipt_and_status() {
    let fixture = tempdir().unwrap();
    let workspace = fixture.path().join("workspace");
    fs::create_dir(&workspace).unwrap();
    let workspace = fs::canonicalize(workspace).unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let service = AgentService::new(Arc::clone(&storage));
    let target = workspace.join("staged-audit-failure.txt");
    let run_id = "run-staged-audit-failure";
    let conversation_id = "conversation-staged-audit-failure";
    let assistant_message_id = "assistant-staged-audit-failure";
    let call_id = "call-staged-audit-failure";
    let transaction_id = "transaction-staged-audit-failure";
    let (mut proposal, call) = staged_file_change_fixture(
        StagedFileChangeFixtureIdentity {
            run_id,
            conversation_id,
            call_id,
            transaction_id,
        },
        ("staged-audit-failure.txt", target.to_str().unwrap()),
        "staged effect may exist\n",
        AgentApprovalStatus::Required,
    );
    let mut input = direct_execution_input(
        &workspace,
        run_id,
        conversation_id,
        AgentPermissions {
            write: AgentWritePermission::WorkspaceOnly,
            patch: mycopilot_core::AgentPatchPermission::RequireApproval,
            ..Default::default()
        },
        &call,
    );
    save_test_pending_provider_for_input(&storage, &mut input);
    bind_staged_execution_to_input(&mut proposal, &input);
    seed_durable_direct_file_change_owner(
        &storage,
        &mut input,
        conversation_id,
        assistant_message_id,
        run_id,
        &call,
    );
    storage
        .create_agent_file_change(staged_file_change_record(&proposal, "waiting_approval"))
        .unwrap();
    service
        .store_pending_action(
            run_id,
            conversation_id,
            assistant_message_id,
            AgentProposedAction::FileChange {
                file_change: proposal,
            },
            input,
        )
        .unwrap();
    let storage_id = pending_action_storage_id(run_id, call_id);
    inject_manual_action_audit_failure(&storage_id, "completed");
    inject_manual_action_audit_failure(&storage_id, "completed");

    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let output = service
        .approve_action(run_id, call_id, notifications)
        .expect("Staged audit fallback settles as typed outcome_unknown");
    let output_json = serde_json::to_value(&output).unwrap();
    let receipt = &output_json["fileChangeResult"];
    assert!(output_json.get("toolResult").is_none());
    assert_eq!(output_json["status"], "outcome_unknown");
    assert_eq!(receipt["status"], "outcome_unknown");
    assert_eq!(receipt["outcome"], "outcome_unknown");
    assert_eq!(
        storage
            .get_agent_file_change(transaction_id)
            .unwrap()
            .unwrap()
            .status,
        "outcome_unknown"
    );
    assert_eq!(
        fs::read_to_string(target).unwrap(),
        "staged effect may exist\n"
    );

    let events = std::iter::from_fn(|| receiver.try_recv().ok()).collect::<Vec<_>>();
    let tool_result = events
        .iter()
        .find(|event| event["params"]["type"] == "tool_result")
        .expect("Staged settlement publishes one ToolResult notification");
    assert_eq!(tool_result["params"]["result"]["result"], *receipt);
    let updated = events
        .iter()
        .find(|event| event["params"].get("fileChange").is_some())
        .expect("Staged settlement publishes its current transaction snapshot");
    assert_eq!(updated["params"]["fileChange"]["status"], "outcome_unknown");
}

#[test]
fn expired_waiting_approval_staged_file_change_fails_before_side_effect() {
    let fixture = tempdir().unwrap();
    let workspace = fixture.path().join("workspace");
    fs::create_dir(&workspace).unwrap();
    let workspace = fs::canonicalize(workspace).unwrap();
    let storage = StorageService::open(&fixture.path().join("storage.sqlite")).unwrap();
    let target = workspace.join("expired-staged.txt");
    let run_id = "run-expired-staged";
    let conversation_id = "conversation-expired-staged";
    let transaction_id = "transaction-expired-staged";
    let (mut proposal, call) = staged_file_change_fixture(
        StagedFileChangeFixtureIdentity {
            run_id,
            conversation_id,
            call_id: "call-expired-staged",
            transaction_id,
        },
        ("expired-staged.txt", target.to_str().unwrap()),
        "must not be published\n",
        AgentApprovalStatus::Required,
    );
    let mut input = direct_execution_input(
        &workspace,
        run_id,
        conversation_id,
        AgentPermissions {
            write: AgentWritePermission::WorkspaceOnly,
            patch: mycopilot_core::AgentPatchPermission::RequireApproval,
            ..Default::default()
        },
        &call,
    );
    save_test_pending_provider_for_input(&storage, &mut input);
    bind_staged_execution_to_input(&mut proposal, &input);
    save_file_change_conversation(&storage, conversation_id);
    let mut transaction = staged_file_change_record(&proposal, "waiting_approval");
    transaction.expires_at = now_ms().saturating_sub(1);
    storage.create_agent_file_change(transaction).unwrap();

    let decision = approved_staged_file_change_execution(&storage, &input, run_id, &proposal);
    assert_eq!(decision.status, "expired");
    assert_eq!(decision.final_pending_status, PendingActionStatus::Failed);
    let result = decision.file_change_result.as_ref().unwrap();
    assert_eq!(
        result.status,
        mycopilot_core::AgentFileChangeResultStatus::Expired
    );
    assert_eq!(
        result.outcome,
        mycopilot_core::AgentFileChangeOutcome::DefinitelyNotExecuted
    );
    assert!(!decision.tool_result.ok);
    assert!(!target.exists());
    assert_eq!(
        storage
            .get_agent_file_change(transaction_id)
            .unwrap()
            .unwrap()
            .status,
        "expired"
    );
}

#[test]
fn rejected_staged_apply_patch_commit_settles_without_file_side_effects() {
    for (case, message, expected_message) in FILE_CHANGE_REJECTION_CASES {
        let fixture = tempdir().unwrap();
        let workspace = fixture.path().join("workspace");
        fs::create_dir(&workspace).unwrap();
        let workspace = fs::canonicalize(workspace).unwrap();
        let storage = StorageService::open(&fixture.path().join("storage.sqlite")).unwrap();
        let target = workspace.join("rejected-staged.txt");
        let run_id = "run-rejected-staged-commit";
        let conversation_id = "conversation-rejected-staged-commit";
        let transaction_id = "transaction-rejected-staged-commit";
        let (mut proposal, call) = staged_file_change_fixture(
            StagedFileChangeFixtureIdentity {
                run_id,
                conversation_id,
                call_id: "call-rejected-staged-commit",
                transaction_id,
            },
            ("rejected-staged.txt", target.to_str().unwrap()),
            "must not be published\n",
            AgentApprovalStatus::Required,
        );
        let input = direct_execution_input(
            &workspace,
            run_id,
            conversation_id,
            AgentPermissions {
                write: AgentWritePermission::WorkspaceOnly,
                patch: mycopilot_core::AgentPatchPermission::RequireApproval,
                ..Default::default()
            },
            &call,
        );
        bind_staged_execution_to_input(&mut proposal, &input);
        save_file_change_conversation(&storage, conversation_id);
        storage
            .create_agent_file_change(staged_file_change_record(&proposal, "waiting_approval"))
            .unwrap();
        let record = pending_file_change_record(run_id, conversation_id, proposal, &call, input);
        let restored_call = tool_call_for_pending_record(&record).unwrap();

        let decision = action_execution_for_decision(
            &storage,
            &record,
            &restored_call,
            AgentApprovalDecisionStatus::Rejected,
            message,
        );
        let receipt = decision.file_change_result.as_ref().unwrap();
        receipt
            .validate()
            .unwrap_or_else(|error| panic!("{case}: invalid staged rejection receipt: {error}"));
        assert_eq!(decision.status, "rejected", "{case}");
        assert_eq!(
            decision.final_pending_status,
            PendingActionStatus::Rejected,
            "{case}"
        );
        assert_eq!(
            receipt.status,
            mycopilot_core::AgentFileChangeResultStatus::Rejected,
            "{case}"
        );
        assert_eq!(
            receipt.outcome,
            mycopilot_core::AgentFileChangeOutcome::DefinitelyNotExecuted,
            "{case}"
        );
        assert_eq!(receipt.message.as_deref(), Some(expected_message), "{case}");
        assert_eq!(receipt.error, None, "{case}");
        assert_eq!(receipt.error_code, None, "{case}");
        assert!(decision.file_change.is_none(), "{case}");
        assert!(decision.tool_result.ok, "{case}");
        assert_eq!(decision.tool_result.call_id, call.id, "{case}");
        assert_eq!(decision.tool_result.tool, "apply_patch", "{case}");
        assert_eq!(decision.tool_result.error, None, "{case}");
        assert_eq!(decision.tool_result.result, Some(json!(receipt)), "{case}");
        assert!(
            !target.exists(),
            "{case}: rejection must not publish the target"
        );
        let persisted = storage
            .get_agent_file_change(transaction_id)
            .unwrap()
            .unwrap();
        assert_eq!(persisted.status, "rejected", "{case}");
        assert!(persisted.stats_final, "{case}");
        assert!(fs::read_dir(&workspace).unwrap().next().is_none(), "{case}");
    }
}

#[test]
fn cancelled_staged_file_change_is_aborted_not_rejected() {
    let fixture = tempdir().unwrap();
    let workspace = fixture.path().join("workspace");
    fs::create_dir(&workspace).unwrap();
    let workspace = fs::canonicalize(workspace).unwrap();
    let storage = StorageService::open(&fixture.path().join("storage.sqlite")).unwrap();
    let target = workspace.join("cancelled-staged.txt");
    let run_id = "run-cancelled-staged";
    let conversation_id = "conversation-cancelled-staged";
    let transaction_id = "transaction-cancelled-staged";
    let (mut proposal, call) = staged_file_change_fixture(
        StagedFileChangeFixtureIdentity {
            run_id,
            conversation_id,
            call_id: "call-cancelled-staged",
            transaction_id,
        },
        ("cancelled-staged.txt", target.to_str().unwrap()),
        "must not be published\n",
        AgentApprovalStatus::Required,
    );
    let input = direct_execution_input(
        &workspace,
        run_id,
        conversation_id,
        AgentPermissions {
            write: AgentWritePermission::WorkspaceOnly,
            patch: mycopilot_core::AgentPatchPermission::RequireApproval,
            ..Default::default()
        },
        &call,
    );
    bind_staged_execution_to_input(&mut proposal, &input);
    save_file_change_conversation(&storage, conversation_id);
    storage
        .create_agent_file_change(staged_file_change_record(&proposal, "waiting_approval"))
        .unwrap();
    let record = pending_file_change_record(run_id, conversation_id, proposal, &call, input);

    let decision = cancelled_file_change_execution(&storage, &record).unwrap();
    assert_eq!(decision.status, "cancelled");
    assert_eq!(
        decision.final_pending_status,
        PendingActionStatus::Cancelled
    );
    let result = decision.file_change_result.as_ref().unwrap();
    assert_eq!(
        result.status,
        mycopilot_core::AgentFileChangeResultStatus::Aborted
    );
    assert_eq!(
        result.outcome,
        mycopilot_core::AgentFileChangeOutcome::DefinitelyNotExecuted
    );
    assert!(decision.tool_result.ok);
    assert!(!target.exists());
    assert_eq!(
        storage
            .get_agent_file_change(transaction_id)
            .unwrap()
            .unwrap()
            .status,
        "aborted"
    );
}

#[test]
fn automatic_staged_apply_patch_commit_persists_the_same_committer_receipt() {
    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("storage.sqlite");
    let workspace = fixture.path().join("workspace");
    fs::create_dir(&workspace).unwrap();
    let workspace = fs::canonicalize(workspace).unwrap();
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    let service = AgentService::new(Arc::clone(&storage));
    let target = workspace.join("automatic-staged.txt");
    let run_id = "run-automatic-staged-commit";
    let conversation_id = "conversation-automatic-staged-commit";
    let call_id = "call-automatic-staged-commit";
    let assistant_message_id = "assistant-automatic-staged-commit";
    let transaction_id = "transaction-automatic-staged-commit";
    let (mut proposal, call) = staged_file_change_fixture(
        StagedFileChangeFixtureIdentity {
            run_id,
            conversation_id,
            call_id,
            transaction_id,
        },
        ("automatic-staged.txt", target.to_str().unwrap()),
        "automatic staged content\n",
        AgentApprovalStatus::Approved,
    );
    let mut input = direct_execution_input(
        &workspace,
        run_id,
        conversation_id,
        AgentPermissions {
            write: AgentWritePermission::WorkspaceOnly,
            patch: mycopilot_core::AgentPatchPermission::AutoApprove,
            ..Default::default()
        },
        &call,
    );
    save_test_pending_provider_for_input(&storage, &mut input);
    seed_durable_direct_file_change_owner(
        &storage,
        &mut input,
        conversation_id,
        assistant_message_id,
        run_id,
        &call,
    );
    bind_staged_execution_to_input(&mut proposal, &input);
    storage
        .create_agent_file_change(staged_file_change_record(&proposal, "waiting_approval"))
        .unwrap();

    let result = service
        .execute_auto_approved_action(
            AutoApprovedActionContext::new(
                input,
                run_id.to_string(),
                Some(conversation_id.to_string()),
                Some(assistant_message_id.to_string()),
                None,
            ),
            AgentProposedAction::FileChange {
                file_change: proposal,
            },
            AgentCancellationToken::new(),
        )
        .unwrap();

    assert!(result.ok, "automatic Staged commit failed: {result:?}");
    assert_eq!(result.tool, "apply_patch");
    assert_eq!(
        fs::read_to_string(&target).unwrap(),
        "automatic staged content\n"
    );
    assert_eq!(
        storage
            .get_agent_file_change(transaction_id)
            .unwrap()
            .unwrap()
            .status,
        "applied"
    );
    let connection = rusqlite::Connection::open(database_path).unwrap();
    let (status, action_json): (String, String) = connection
        .query_row(
            "SELECT status, action_json FROM agent_action_audit WHERE action_id = ?1",
            [pending_action_storage_id(run_id, call_id)],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(status, "completed");
    let committed: AgentProposedAction = serde_json::from_str(&action_json).unwrap();
    let AgentProposedAction::FileChange { file_change } = committed else {
        panic!("automatic Staged receipt must preserve the canonical FileChange proposal")
    };
    assert_eq!(
        file_change.execution.transaction.status,
        mycopilot_core::file_change::FileChangeStatus::Applied
    );
    assert!(file_change.execution.receipt.is_some());
}

#[test]
fn staged_update_conflict_after_approval_wait_is_terminal_and_has_no_side_effect() {
    let fixture = tempdir().unwrap();
    let workspace = fixture.path().join("workspace");
    fs::create_dir(&workspace).unwrap();
    let workspace = fs::canonicalize(workspace).unwrap();
    let storage = StorageService::open(&fixture.path().join("storage.sqlite")).unwrap();
    let target = workspace.join("staged-conflict.txt");
    fs::write(&target, "frozen staged base\n").unwrap();
    let run_id = "run-staged-update-conflict";
    let conversation_id = "conversation-staged-update-conflict";
    let transaction_id = "transaction-staged-update-conflict";
    let (mut proposal, call) = staged_update_file_change_fixture(
        StagedFileChangeFixtureIdentity {
            run_id,
            conversation_id,
            call_id: "call-staged-update-conflict",
            transaction_id,
        },
        ("staged-conflict.txt", target.to_str().unwrap()),
        "frozen staged base\n",
        "approved staged target\n",
        AgentApprovalStatus::Required,
    );
    let input = direct_execution_input(
        &workspace,
        run_id,
        conversation_id,
        AgentPermissions {
            write: AgentWritePermission::WorkspaceOnly,
            patch: mycopilot_core::AgentPatchPermission::RequireApproval,
            ..Default::default()
        },
        &call,
    );
    bind_staged_execution_to_input(&mut proposal, &input);
    save_file_change_conversation(&storage, conversation_id);
    storage
        .create_agent_file_change(staged_file_change_record(&proposal, "waiting_approval"))
        .unwrap();
    fs::write(&target, "external change while approval waited\n").unwrap();
    let record = pending_file_change_record(run_id, conversation_id, proposal, &call, input);
    let restored_call = tool_call_for_pending_record(&record).unwrap();

    let decision = action_execution_for_decision(
        &storage,
        &record,
        &restored_call,
        AgentApprovalDecisionStatus::Approved,
        None,
    );

    let result = decision.file_change_result.as_ref().unwrap();
    assert_eq!(decision.status, "conflict");
    assert_eq!(decision.final_pending_status, PendingActionStatus::Failed);
    assert_eq!(
        result.status,
        mycopilot_core::AgentFileChangeResultStatus::Conflict
    );
    assert!(!decision.tool_result.ok);
    assert!(decision.file_change.is_none());
    assert!(decision.committed_file_change_action.is_none());
    assert_eq!(
        fs::read_to_string(&target).unwrap(),
        "external change while approval waited\n"
    );
    assert_eq!(
        storage
            .get_agent_file_change(transaction_id)
            .unwrap()
            .unwrap()
            .status,
        "conflict"
    );
    assert_eq!(fs::read_dir(&workspace).unwrap().count(), 1);
}

#[test]
fn staged_owner_and_authority_tampering_fail_before_file_side_effects() {
    let fixture = tempdir().unwrap();
    let workspace = fixture.path().join("workspace");
    fs::create_dir(&workspace).unwrap();
    let workspace = fs::canonicalize(workspace).unwrap();
    let storage = StorageService::open(&fixture.path().join("storage.sqlite")).unwrap();
    let permissions = AgentPermissions {
        write: AgentWritePermission::WorkspaceOnly,
        patch: mycopilot_core::AgentPatchPermission::RequireApproval,
        ..Default::default()
    };

    let owner_target = workspace.join("owner-tamper.txt");
    let owner_transaction_id = "transaction-staged-owner-tamper";
    let (mut owner_proposal, owner_call) = staged_file_change_fixture(
        StagedFileChangeFixtureIdentity {
            run_id: "run-staged-owner-tamper",
            conversation_id: "conversation-staged-owner",
            call_id: "call-staged-owner-tamper",
            transaction_id: owner_transaction_id,
        },
        ("owner-tamper.txt", owner_target.to_str().unwrap()),
        "must not publish\n",
        AgentApprovalStatus::Required,
    );
    let owner_input = direct_execution_input(
        &workspace,
        "run-staged-owner-tamper",
        "conversation-staged-owner",
        permissions,
        &owner_call,
    );
    bind_staged_execution_to_input(&mut owner_proposal, &owner_input);
    save_file_change_conversation(&storage, "conversation-staged-owner");
    storage
        .create_agent_file_change(staged_file_change_record(
            &owner_proposal,
            "waiting_approval",
        ))
        .unwrap();
    let mut wrong_owner_input = owner_input;
    wrong_owner_input.context.as_mut().unwrap().conversation_id =
        Some("conversation-staged-attacker".to_string());
    let owner_decision = approved_staged_file_change_execution(
        &storage,
        &wrong_owner_input,
        "run-staged-owner-tamper",
        &owner_proposal,
    );
    assert_eq!(owner_decision.status, "failed");
    assert!(!owner_decision.tool_result.ok);
    assert!(!owner_target.exists());
    assert_eq!(
        storage
            .get_agent_file_change(owner_transaction_id)
            .unwrap()
            .unwrap()
            .status,
        "waiting_approval"
    );

    let authority_target = workspace.join("authority-tamper.txt");
    let authority_transaction_id = "transaction-staged-authority-tamper";
    let (mut authority_proposal, authority_call) = staged_file_change_fixture(
        StagedFileChangeFixtureIdentity {
            run_id: "run-staged-authority-tamper",
            conversation_id: "conversation-staged-authority",
            call_id: "call-staged-authority-tamper",
            transaction_id: authority_transaction_id,
        },
        ("authority-tamper.txt", authority_target.to_str().unwrap()),
        "must also not publish\n",
        AgentApprovalStatus::Required,
    );
    let mut authority_input = direct_execution_input(
        &workspace,
        "run-staged-authority-tamper",
        "conversation-staged-authority",
        permissions,
        &authority_call,
    );
    bind_staged_execution_to_input(&mut authority_proposal, &authority_input);
    save_file_change_conversation(&storage, "conversation-staged-authority");
    storage
        .create_agent_file_change(staged_file_change_record(
            &authority_proposal,
            "waiting_approval",
        ))
        .unwrap();
    authority_input.context.as_mut().unwrap().permissions.write = AgentWritePermission::All;
    authorize_file_change_action(
        &authority_input,
        &AgentProposedAction::FileChange {
            file_change: authority_proposal.clone(),
        },
        FileChangeAuthorizationSource::ExplicitUser,
    )
    .unwrap();
    let authority_decision = approved_staged_file_change_execution(
        &storage,
        &authority_input,
        "run-staged-authority-tamper",
        &authority_proposal,
    );
    assert_eq!(authority_decision.status, "failed");
    assert!(!authority_decision.tool_result.ok);
    assert!(!authority_target.exists());
    assert_eq!(
        storage
            .get_agent_file_change(authority_transaction_id)
            .unwrap()
            .unwrap()
            .status,
        "failed"
    );
    assert!(fs::read_dir(&workspace).unwrap().next().is_none());
}
