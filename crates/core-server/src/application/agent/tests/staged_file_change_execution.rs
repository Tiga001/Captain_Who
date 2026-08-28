use super::file_write_permissions::direct_execution_input;
use super::*;

fn bind_staged_execution_to_input(proposal: &mut AgentFileWriteProposal, input: &AgentChatInput) {
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

fn pending_file_write_record(
    run_id: &str,
    conversation_id: &str,
    file_write: AgentFileWriteProposal,
    call: &AgentToolCall,
    agent_input: AgentChatInput,
) -> PendingActionRecord {
    PendingActionRecord {
        storage_id: pending_action_storage_id(run_id, &call.id),
        snapshot: PendingAgentActionSnapshot {
            action_id: call.id.clone(),
            action_type: "file_write".to_string(),
            tool_name: call.tool.clone(),
            tool_call_id: Some(call.id.clone()),
            run_id: run_id.to_string(),
            conversation_id: Some(conversation_id.to_string()),
            assistant_message_id: Some(format!("assistant-{run_id}")),
            action: AgentProposedAction::FileWrite { file_write },
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

fn staged_update_file_write_fixture(
    identity: StagedFileWriteFixtureIdentity<'_>,
    paths: (&str, &str),
    base_content: &str,
    target_content: &str,
    approval_status: AgentApprovalStatus,
) -> (AgentFileWriteProposal, AgentToolCall) {
    let StagedFileWriteFixtureIdentity {
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
    let AgentProposedAction::Diff { diff } = action else {
        unreachable!("the fixture constructor returns a Direct Diff")
    };
    let mut execution = *diff.execution;
    execution.transaction.id = transaction_id.to_string();
    execution.proposal.transaction_id = transaction_id.to_string();
    execution.staged_transaction_id = Some(transaction_id.to_string());
    execution.staged_transaction_revision = Some(1);
    let args = json!({
        "action": "commit",
        "transactionId": transaction_id,
        "expectedDraftRevision": 1,
    });
    execution.source_args_digest = mycopilot_core::file_change::proposal_digest(&args)
        .expect("digest staged update Tool Call arguments");
    execution
        .validate()
        .expect("valid staged update FileChange execution binding");
    let proposal = AgentFileWriteProposal {
        id: call_id.to_string(),
        draft_id: transaction_id.to_string(),
        mode: AgentFileWriteMode::Modify,
        file_path: paths.0.to_string(),
        base_revision: execution.transaction.base.revision().map(str::to_string),
        summary: Some("test Staged update".to_string()),
        additions: execution.proposal.additions,
        deletions: execution.proposal.deletions,
        line_count: target_content.lines().count() as u64,
        byte_count: target_content.len() as u64,
        approval_status,
        execution: Box::new(execution),
    };
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
        .file_write_result
        .as_ref()
        .expect("Staged file change returns a file-write result");
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
        mycopilot_core::AgentFileWriteResultStatus::Applied
    );
    assert!(decision.file_change.is_some());
    assert!(decision.tool_result.ok);
    assert_eq!(decision.tool_result.tool, "apply_patch");
    let AgentProposedAction::FileWrite { file_write } = decision
        .committed_file_change_action
        .as_ref()
        .expect("the common FileChange committer returns a durable successor binding")
    else {
        panic!("Staged execution must retain the bounded FileWrite presentation shell")
    };
    assert_eq!(
        file_write.execution.transaction.status,
        mycopilot_core::file_change::FileChangeStatus::Applied
    );
    assert!(file_write.execution.receipt.is_some());
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
    let (mut proposal, call) = staged_file_write_fixture(
        StagedFileWriteFixtureIdentity {
            run_id,
            conversation_id,
            call_id: "call-manual-staged-commit",
            transaction_id,
        },
        ("manual-staged.txt", target.to_str().unwrap()),
        "manual staged content\n",
        "apply_patch",
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
    let action = AgentProposedAction::FileWrite {
        file_write: proposal,
    };
    authorize_structured_file_write(&input, &action, FileWriteAuthorizationSource::ExplicitUser)
        .unwrap();
    let AgentProposedAction::FileWrite { file_write } = action else {
        unreachable!()
    };
    let record = pending_file_write_record(run_id, conversation_id, file_write, &call, input);
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

#[test]
fn rejected_staged_apply_patch_commit_settles_without_file_side_effects() {
    let fixture = tempdir().unwrap();
    let workspace = fixture.path().join("workspace");
    fs::create_dir(&workspace).unwrap();
    let workspace = fs::canonicalize(workspace).unwrap();
    let storage = StorageService::open(&fixture.path().join("storage.sqlite")).unwrap();
    let target = workspace.join("rejected-staged.txt");
    let run_id = "run-rejected-staged-commit";
    let conversation_id = "conversation-rejected-staged-commit";
    let transaction_id = "transaction-rejected-staged-commit";
    let (mut proposal, call) = staged_file_write_fixture(
        StagedFileWriteFixtureIdentity {
            run_id,
            conversation_id,
            call_id: "call-rejected-staged-commit",
            transaction_id,
        },
        ("rejected-staged.txt", target.to_str().unwrap()),
        "must not be published\n",
        "apply_patch",
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
    let record = pending_file_write_record(run_id, conversation_id, proposal, &call, input);
    let restored_call = tool_call_for_pending_record(&record).unwrap();

    let decision = action_execution_for_decision(
        &storage,
        &record,
        &restored_call,
        AgentApprovalDecisionStatus::Rejected,
        Some("not approved"),
    );

    assert_eq!(decision.status, "rejected");
    assert_eq!(decision.final_pending_status, PendingActionStatus::Rejected);
    assert_eq!(
        decision.file_write_result.as_ref().unwrap().status,
        mycopilot_core::AgentFileWriteResultStatus::Rejected
    );
    assert!(decision.tool_result.ok);
    assert!(!target.exists());
    assert_eq!(
        storage
            .get_agent_file_change(transaction_id)
            .unwrap()
            .unwrap()
            .status,
        "rejected"
    );
    assert!(fs::read_dir(&workspace).unwrap().next().is_none());
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
    let transaction_id = "transaction-automatic-staged-commit";
    let (mut proposal, call) = staged_file_write_fixture(
        StagedFileWriteFixtureIdentity {
            run_id,
            conversation_id,
            call_id,
            transaction_id,
        },
        ("automatic-staged.txt", target.to_str().unwrap()),
        "automatic staged content\n",
        "apply_patch",
        AgentApprovalStatus::Approved,
    );
    let input = direct_execution_input(
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
    bind_staged_execution_to_input(&mut proposal, &input);
    save_file_change_conversation(&storage, conversation_id);
    storage
        .create_agent_file_change(staged_file_change_record(&proposal, "waiting_approval"))
        .unwrap();

    let result = service
        .execute_auto_approved_action(
            AutoApprovedActionContext::new(
                input,
                run_id.to_string(),
                Some(conversation_id.to_string()),
                Some("assistant-automatic-staged-commit".to_string()),
                None,
            ),
            AgentProposedAction::FileWrite {
                file_write: proposal,
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
    let AgentProposedAction::FileWrite { file_write } = committed else {
        panic!("automatic Staged receipt must preserve the FileWrite presentation shell")
    };
    assert_eq!(
        file_write.execution.transaction.status,
        mycopilot_core::file_change::FileChangeStatus::Applied
    );
    assert!(file_write.execution.receipt.is_some());
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
    let (mut proposal, call) = staged_update_file_write_fixture(
        StagedFileWriteFixtureIdentity {
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
    let record = pending_file_write_record(run_id, conversation_id, proposal, &call, input);
    let restored_call = tool_call_for_pending_record(&record).unwrap();

    let decision = action_execution_for_decision(
        &storage,
        &record,
        &restored_call,
        AgentApprovalDecisionStatus::Approved,
        None,
    );

    let result = decision.file_write_result.as_ref().unwrap();
    assert_eq!(decision.status, "conflict");
    assert_eq!(decision.final_pending_status, PendingActionStatus::Failed);
    assert_eq!(
        result.status,
        mycopilot_core::AgentFileWriteResultStatus::Conflict
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
    let (mut owner_proposal, owner_call) = staged_file_write_fixture(
        StagedFileWriteFixtureIdentity {
            run_id: "run-staged-owner-tamper",
            conversation_id: "conversation-staged-owner",
            call_id: "call-staged-owner-tamper",
            transaction_id: owner_transaction_id,
        },
        ("owner-tamper.txt", owner_target.to_str().unwrap()),
        "must not publish\n",
        "apply_patch",
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
    let owner_decision = approved_file_write_execution(
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
    let (mut authority_proposal, authority_call) = staged_file_write_fixture(
        StagedFileWriteFixtureIdentity {
            run_id: "run-staged-authority-tamper",
            conversation_id: "conversation-staged-authority",
            call_id: "call-staged-authority-tamper",
            transaction_id: authority_transaction_id,
        },
        ("authority-tamper.txt", authority_target.to_str().unwrap()),
        "must also not publish\n",
        "apply_patch",
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
    authorize_structured_file_write(
        &authority_input,
        &AgentProposedAction::FileWrite {
            file_write: authority_proposal.clone(),
        },
        FileWriteAuthorizationSource::ExplicitUser,
    )
    .unwrap();
    let authority_decision = approved_file_write_execution(
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
