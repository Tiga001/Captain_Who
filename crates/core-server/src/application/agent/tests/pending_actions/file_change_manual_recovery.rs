use super::file_change_fixtures::frozen_file_change_recovery_workspace;
use super::fixtures::{seed_durable_pending_owner, test_pending_resume_checkpoint_for_call};
use super::*;

fn seed_interrupted_manual_file_change(
    storage: &Arc<StorageService>,
    workspace: &std::path::Path,
    run_id: &str,
    conversation_id: &str,
    assistant_message_id: &str,
    action: AgentProposedAction,
    call: &AgentToolCall,
) -> PendingActionRecord {
    let mut agent_input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "test-token",
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "messages": []
    }))
    .unwrap();
    save_test_pending_provider_for_input(storage, &mut agent_input);
    let run_context = AgentRunContext {
        conversation_id: Some(conversation_id.to_string()),
        project_id: None,
        workspace: Some(frozen_file_change_recovery_workspace(workspace)),
        attachment_library: None,
        permissions: AgentPermissions::default(),
        collaboration_identity: None,
    };
    agent_input.context = Some(run_context.clone());
    let storage_id = pending_action_storage_id(run_id, &call.id);
    agent_input.resume_checkpoint = Some(test_pending_resume_checkpoint_for_call(
        storage,
        run_id,
        Some(&storage_id),
        call,
        AgentToolIdentity::Builtin {
            tool_name: "apply_patch".to_string(),
        },
    ));
    agent_input.resume_checkpoint.as_mut().unwrap().run_context = Some(run_context);
    freeze_test_pending_provider_configuration(storage, &mut agent_input);
    seed_durable_pending_owner(
        storage,
        conversation_id,
        assistant_message_id,
        run_id,
        call,
        AgentToolIdentity::Builtin {
            tool_name: "apply_patch".to_string(),
        },
        1,
    );
    let record = PendingActionRecord {
        storage_id,
        snapshot: PendingAgentActionSnapshot {
            action_id: call.id.clone(),
            action_type: "file_change".to_string(),
            tool_name: "apply_patch".to_string(),
            tool_call_id: Some(call.id.clone()),
            run_id: run_id.to_string(),
            conversation_id: Some(conversation_id.to_string()),
            assistant_message_id: Some(assistant_message_id.to_string()),
            action,
            created_at: 1,
            status: PendingActionStatus::Executing,
        },
        agent_input,
    };
    storage
        .store_pending_agent_action(pending_storage_record(&record, 2).unwrap())
        .unwrap();
    assert!(storage
        .insert_agent_action_audit_if_absent(action_audit_record(
            &record, None, "pending", None, None, None, None, None, None,
        ))
        .unwrap());
    record
}

#[test]
fn startup_reconciles_published_manual_direct_file_change_without_replaying_it() {
    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("storage.sqlite");
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    let run_id = "run-manual-direct-crash";
    let conversation_id = "conversation-manual-direct-crash";
    let assistant_message_id = "assistant-manual-direct-crash";
    let call_id = "call-manual-direct-crash";
    let target_content = "published before the durable receipt\n";
    let target_path = std::fs::canonicalize(fixture.path())
        .unwrap()
        .join("manual-recovered.txt");
    let (action, call) = direct_file_change_fixture(
        run_id,
        conversation_id,
        call_id,
        ("manual-recovered.txt", target_path.to_str().unwrap()),
        None,
        Some(target_content),
        AgentApprovalStatus::Required,
    );

    // This is the crash window: publication succeeded, but both pending and manual audit still
    // contain the prepared credential and no terminal ToolResult exists.
    std::fs::write(&target_path, target_content).unwrap();
    let metadata_before = std::fs::metadata(&target_path).unwrap();
    let mut agent_input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "test-token",
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "messages": []
    }))
    .unwrap();
    agent_input.context = Some(AgentRunContext {
        collaboration_identity: None,
        conversation_id: Some(conversation_id.to_string()),
        project_id: None,
        workspace: Some(frozen_file_change_recovery_workspace(fixture.path())),
        attachment_library: None,
        permissions: AgentPermissions::default(),
    });
    save_test_pending_provider_for_input(&storage, &mut agent_input);
    let pending_action_id = pending_action_storage_id(run_id, call_id);
    let mut checkpoint = test_pending_resume_checkpoint_for_call(
        &storage,
        run_id,
        Some(&pending_action_id),
        &call,
        AgentToolIdentity::Builtin {
            tool_name: "apply_patch".to_string(),
        },
    );
    checkpoint.run_context = agent_input.context.clone();
    agent_input.resume_checkpoint = Some(checkpoint);
    freeze_test_pending_provider_configuration(&storage, &mut agent_input);
    seed_durable_pending_owner(
        &storage,
        conversation_id,
        assistant_message_id,
        run_id,
        &call,
        AgentToolIdentity::Builtin {
            tool_name: "apply_patch".to_string(),
        },
        1,
    );
    let record = PendingActionRecord {
        storage_id: pending_action_storage_id(run_id, call_id),
        snapshot: PendingAgentActionSnapshot {
            action_id: call_id.to_string(),
            action_type: "file_change".to_string(),
            tool_name: "apply_patch".to_string(),
            tool_call_id: Some(call_id.to_string()),
            run_id: run_id.to_string(),
            conversation_id: Some(conversation_id.to_string()),
            assistant_message_id: Some(assistant_message_id.to_string()),
            action,
            created_at: 1,
            status: PendingActionStatus::Executing,
        },
        agent_input,
    };
    storage
        .store_pending_agent_action(pending_storage_record(&record, 2).unwrap())
        .unwrap();
    assert!(storage
        .insert_agent_action_audit_if_absent(action_audit_record(
            &record, None, "pending", None, None, None, None, None, None,
        ))
        .unwrap());

    let candidates = storage
        .list_interrupted_agent_actions_for_host_reconciliation()
        .unwrap();
    assert_eq!(candidates.len(), 1);
    PersistedAgentResumeInput::decode(&candidates[0].agent_input_json)
        .expect("manual crash fixture must retain a current resume input");
    let prepared: AgentProposedAction = serde_json::from_str(&candidates[0].action_json).unwrap();
    let AgentProposedAction::FileChange {
        file_change: prepared,
    } = prepared
    else {
        panic!("manual crash fixture must retain a FileChange");
    };
    prepared.execution.validate().unwrap();
    let resolved = mycopilot_core::file_change::FileChangePathPolicy::new(None, true)
        .resolve(&prepared.execution.canonical_target)
        .unwrap();
    let plan = mycopilot_core::file_change::FileChangePlan::from_direct_binding(
        &prepared.execution,
        prepared.inline_diff.as_ref().unwrap().patch.clone(),
    )
    .unwrap();
    assert_eq!(
        mycopilot_core::file_change::FileChangeCommitter
            .reconcile(&resolved, &plan, prepared.execution.delete_journal.as_ref())
            .unwrap(),
        mycopilot_core::file_change::FileChangeReconciliation::AlreadyApplied
    );

    let reconciled_at = mycopilot_core::storage::now_ms();
    assert_eq!(
        reconcile_interrupted_file_changes(&storage, reconciled_at).unwrap(),
        1,
        "the Direct startup pre-pass must recognize the already-published target"
    );
    let after_direct = storage
        .get_pending_agent_action(&record.storage_id)
        .unwrap()
        .unwrap();
    assert_eq!(after_direct.status, "executing");
    assert_eq!(after_direct.target_status.as_deref(), Some("completed"));
    let _restarted = AgentService::try_new_deferred_startup_reconciliation(Arc::clone(&storage))
        .expect("published Direct FileChange must reconcile from its target digest");

    assert_eq!(
        std::fs::read_to_string(&target_path).unwrap(),
        target_content
    );
    let metadata_after = std::fs::metadata(&target_path).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        assert_eq!(metadata_after.dev(), metadata_before.dev());
        assert_eq!(metadata_after.ino(), metadata_before.ino());
    }
    #[cfg(not(unix))]
    let _ = (metadata_before, metadata_after);

    let connection = rusqlite::Connection::open(database_path).unwrap();
    let (pending_status, pending_action_json, audit_status, audit_action_json): (
        String,
        String,
        String,
        String,
    ) = connection
        .query_row(
            "SELECT pending.status, pending.action_json, audit.status, audit.action_json
             FROM agent_pending_actions pending
             JOIN agent_action_audit audit ON audit.action_id = pending.action_id
             WHERE pending.action_id = ?1",
            [&record.storage_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(pending_status, "completed");
    assert_eq!(audit_status, "completed");
    assert_eq!(pending_action_json, audit_action_json);
    let committed: AgentProposedAction = serde_json::from_str(&pending_action_json).unwrap();
    let AgentProposedAction::FileChange { file_change } = committed else {
        panic!("recovered pending action must remain a FileChange");
    };
    assert_eq!(
        file_change.execution.transaction.status,
        mycopilot_core::file_change::FileChangeStatus::AlreadyApplied
    );
    assert!(file_change.execution.receipt.is_some());
}

#[test]
fn startup_file_change_reconciliation_types_base_divergence_and_unreadable_targets() {
    #[derive(Clone, Copy)]
    enum Case {
        Base,
        Divergent,
        #[cfg(unix)]
        Unreadable,
    }
    let mut cases = vec![Case::Base, Case::Divergent];
    #[cfg(unix)]
    cases.push(Case::Unreadable);

    for (index, case) in cases.into_iter().enumerate() {
        let fixture = tempdir().unwrap();
        let storage =
            Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
        let run_id = format!("run-file-change-recovery-{index}");
        let conversation_id = format!("conversation-file-change-recovery-{index}");
        let assistant_message_id = format!("assistant-file-change-recovery-{index}");
        let call_id = format!("call-file-change-recovery-{index}");
        let canonical_fixture = std::fs::canonicalize(fixture.path()).unwrap();
        let parent = canonical_fixture.join("observed");
        std::fs::create_dir(&parent).unwrap();
        let target_path = parent.join("target.txt");
        let (action, call) = direct_file_change_fixture(
            &run_id,
            &conversation_id,
            &call_id,
            ("observed/target.txt", target_path.to_str().unwrap()),
            None,
            Some("frozen target\n"),
            AgentApprovalStatus::Required,
        );
        let record = seed_interrupted_manual_file_change(
            &storage,
            &canonical_fixture,
            &run_id,
            &conversation_id,
            &assistant_message_id,
            action,
            &call,
        );
        match case {
            Case::Base => {}
            Case::Divergent => std::fs::write(&target_path, "different content\n").unwrap(),
            #[cfg(unix)]
            Case::Unreadable => {
                use std::os::unix::fs::symlink;
                let frozen_parent = canonical_fixture.join("observed-original");
                std::fs::rename(&parent, &frozen_parent).unwrap();
                let redirected_parent = canonical_fixture.join("redirected");
                std::fs::create_dir(&redirected_parent).unwrap();
                symlink(&redirected_parent, &parent).unwrap();
            }
        }

        assert_eq!(
            reconcile_interrupted_file_changes(&storage, mycopilot_core::storage::now_ms())
                .unwrap(),
            1
        );
        let pending = storage
            .get_pending_agent_action(&record.storage_id)
            .unwrap()
            .unwrap();
        assert_eq!(pending.status, "executing");
        assert_eq!(pending.target_status.as_deref(), Some("failed"));
        let audit = storage
            .get_agent_action_audit(&record.storage_id)
            .unwrap()
            .unwrap();
        assert_eq!(audit.status, "failed");
        assert_eq!(audit.decision_source.as_deref(), Some("manual"));
        let result: AgentFileChangeResult = serde_json::from_str(
            audit
                .file_change_result_json
                .as_deref()
                .expect("reconciliation persists the typed FileChange result"),
        )
        .unwrap();
        let tool_result: AgentToolResult = serde_json::from_str(
            audit
                .tool_result_json
                .as_deref()
                .expect("reconciliation persists the exact ToolResult"),
        )
        .unwrap();
        assert!(!tool_result.ok);
        match case {
            Case::Base => {
                assert_eq!(
                    result.status,
                    mycopilot_core::AgentFileChangeResultStatus::Failed
                );
                assert_eq!(
                    result.outcome,
                    mycopilot_core::AgentFileChangeOutcome::DefinitelyNotExecuted
                );
            }
            Case::Divergent => {
                assert_eq!(
                    result.status,
                    mycopilot_core::AgentFileChangeResultStatus::OutcomeUnknown
                );
                assert_eq!(
                    result.outcome,
                    mycopilot_core::AgentFileChangeOutcome::OutcomeUnknown
                );
            }
            #[cfg(unix)]
            Case::Unreadable => {
                assert_eq!(
                    result.status,
                    mycopilot_core::AgentFileChangeResultStatus::OutcomeUnknown
                );
                assert_eq!(
                    result.outcome,
                    mycopilot_core::AgentFileChangeOutcome::OutcomeUnknown
                );
            }
        }
        storage
            .reconcile_interrupted_pending_agent_actions(mycopilot_core::storage::now_ms())
            .unwrap();
        assert_eq!(
            storage
                .get_pending_agent_action(&record.storage_id)
                .unwrap()
                .unwrap()
                .status,
            "failed"
        );
    }
}

#[test]
fn startup_reconciles_published_manual_staged_file_changes_without_replaying_them() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let run_id = "run-manual-staged-crash";
    let conversation_id = "conversation-manual-staged-crash";
    let assistant_message_id = "assistant-manual-staged-crash";
    let call_id = "call-manual-staged-crash";
    let transaction_id = "transaction-manual-staged-crash";
    let target_content = "published staged content from apply_patch\n";
    let target_path = std::fs::canonicalize(fixture.path())
        .unwrap()
        .join("manual-staged.txt");
    let (file_change, call) = staged_file_change_fixture(
        StagedFileChangeFixtureIdentity {
            run_id,
            conversation_id,
            call_id,
            transaction_id,
        },
        ("manual-staged.txt", target_path.to_str().unwrap()),
        target_content,
        AgentApprovalStatus::Required,
    );
    let action = AgentProposedAction::FileChange {
        file_change: file_change.clone(),
    };

    std::fs::write(&target_path, target_content).unwrap();
    let metadata_before = std::fs::metadata(&target_path).unwrap();
    let mut agent_input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "test-token",
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "messages": []
    }))
    .unwrap();
    save_test_pending_provider_for_input(&storage, &mut agent_input);
    let run_context = AgentRunContext {
        conversation_id: Some(conversation_id.to_string()),
        project_id: None,
        workspace: Some(frozen_file_change_recovery_workspace(fixture.path())),
        attachment_library: None,
        permissions: AgentPermissions::default(),
        collaboration_identity: None,
    };
    agent_input.context = Some(run_context.clone());
    let storage_id = pending_action_storage_id(run_id, call_id);
    agent_input.resume_checkpoint = Some(test_pending_resume_checkpoint_for_call(
        &storage,
        run_id,
        Some(&storage_id),
        &call,
        AgentToolIdentity::Builtin {
            tool_name: "apply_patch".to_string(),
        },
    ));
    agent_input.resume_checkpoint.as_mut().unwrap().run_context = Some(run_context);
    freeze_test_pending_provider_configuration(&storage, &mut agent_input);
    seed_durable_pending_owner(
        &storage,
        conversation_id,
        assistant_message_id,
        run_id,
        &call,
        AgentToolIdentity::Builtin {
            tool_name: "apply_patch".to_string(),
        },
        1,
    );
    storage
        .create_agent_file_change(staged_file_change_record(&file_change, "applying"))
        .unwrap();
    let record = PendingActionRecord {
        storage_id,
        snapshot: PendingAgentActionSnapshot {
            action_id: call_id.to_string(),
            action_type: "file_change".to_string(),
            tool_name: "apply_patch".to_string(),
            tool_call_id: Some(call_id.to_string()),
            run_id: run_id.to_string(),
            conversation_id: Some(conversation_id.to_string()),
            assistant_message_id: Some(assistant_message_id.to_string()),
            action,
            created_at: 1,
            status: PendingActionStatus::Executing,
        },
        agent_input,
    };
    storage
        .store_pending_agent_action(pending_storage_record(&record, 2).unwrap())
        .unwrap();
    assert!(storage
        .insert_agent_action_audit_if_absent(action_audit_record(
            &record, None, "pending", None, None, None, None, None, None,
        ))
        .unwrap());

    assert_eq!(
        reconcile_interrupted_file_changes(&storage, mycopilot_core::storage::now_ms()).unwrap(),
        1
    );
    let transaction = storage
        .get_agent_file_change(transaction_id)
        .unwrap()
        .unwrap();
    assert_eq!(transaction.status, "applied");
    assert_eq!(
        std::fs::read_to_string(&target_path).unwrap(),
        target_content
    );
    let metadata_after = std::fs::metadata(&target_path).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        assert_eq!(metadata_after.dev(), metadata_before.dev());
        assert_eq!(metadata_after.ino(), metadata_before.ino());
    }
    #[cfg(not(unix))]
    let _ = (metadata_before, metadata_after);

    let durable = storage
        .get_pending_agent_action(&record.storage_id)
        .unwrap()
        .unwrap();
    assert_eq!(durable.status, "executing");
    assert_eq!(durable.target_status.as_deref(), Some("completed"));
    let committed: AgentProposedAction = serde_json::from_str(&durable.action_json).unwrap();
    let AgentProposedAction::FileChange { file_change } = committed else {
        panic!("recovered Staged action must remain FileChange")
    };
    assert_eq!(file_change.execution.source_tool_name, "apply_patch");
    assert!(file_change.execution.receipt.is_some());

    let _restarted = AgentService::try_new_deferred_startup_reconciliation(Arc::clone(&storage))
        .expect("published Staged FileChange must reconcile without replay");
    let results = storage
        .list_agent_tool_results_for_run(run_id, "apply_patch")
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].call_id, call_id);
    assert_eq!(results[0].tool, "apply_patch");
    assert!(results[0].ok);
}
