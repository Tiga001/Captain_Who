use super::file_change_fixtures::frozen_file_change_recovery_workspace;
use super::fixtures::{seed_durable_pending_owner, test_pending_resume_checkpoint_for_call};
use super::*;

#[test]
fn startup_reconciles_published_automatic_direct_file_change_without_replaying_it() {
    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("storage.sqlite");
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    save_test_pending_provider(
        &storage,
        "test-model",
        "https://example.test/v1/chat/completions",
        "test-token",
        "disabled",
        "",
    );
    // Construct the Host before seeding the synthetic interrupted turn so startup recovery does
    // not terminalize the fixture trace before its hidden Pending journal exists.
    let service = AgentService::new_authorized_for_test(Arc::clone(&storage));
    let run_id = "run-automatic-direct-crash";
    let conversation_id = "conversation-automatic-direct-crash";
    let assistant_message_id = "assistant-automatic-direct-crash";
    let call_id = "call-automatic-direct-crash";
    let target_content = "automatically published before the durable receipt\n";
    let target_path = std::fs::canonicalize(fixture.path())
        .unwrap()
        .join("automatic-recovered.txt");
    let (action, call) = direct_file_change_fixture(
        run_id,
        conversation_id,
        call_id,
        ("automatic-recovered.txt", target_path.to_str().unwrap()),
        None,
        Some(target_content),
        AgentApprovalStatus::Approved,
    );
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
    let mut hidden = service
        .prepare_auto_file_change_action_journal(
            run_id,
            conversation_id,
            assistant_message_id,
            action,
            agent_input,
        )
        .unwrap();
    assert!(service
        .claim_auto_file_change_dispatch(&mut hidden)
        .unwrap());

    // Simulate a crash after filesystem publication but before the receipt/terminal audit.
    std::fs::write(&target_path, target_content).unwrap();
    let metadata_before = std::fs::metadata(&target_path).unwrap();
    let candidates = storage
        .list_interrupted_agent_actions_for_host_reconciliation()
        .unwrap();
    assert_eq!(candidates.len(), 1);
    let prepared: AgentProposedAction = serde_json::from_str(&candidates[0].action_json).unwrap();
    let AgentProposedAction::FileChange {
        file_change: prepared,
    } = prepared
    else {
        panic!("automatic crash fixture must retain a FileChange");
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

    assert_eq!(
        reconcile_interrupted_file_changes(&storage, mycopilot_core::storage::now_ms()).unwrap(),
        1,
        "the automatic Direct startup pre-pass must recognize the already-published target"
    );
    let _restarted = AgentService::try_new_deferred_startup_reconciliation(Arc::clone(&storage))
        .expect("published automatic Direct FileChange must reconcile from its target digest");

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
    let (status, action_json, tool_result_json): (String, String, Option<String>) = connection
        .query_row(
            "SELECT status, action_json, tool_result_json
             FROM agent_action_audit WHERE action_id = ?1",
            [pending_action_storage_id(run_id, call_id)],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(status, "completed");
    assert!(tool_result_json.is_some());
    let committed: AgentProposedAction = serde_json::from_str(&action_json).unwrap();
    let AgentProposedAction::FileChange { file_change } = committed else {
        panic!("recovered automatic action must remain a FileChange");
    };
    assert_eq!(
        file_change.execution.transaction.status,
        mycopilot_core::file_change::FileChangeStatus::AlreadyApplied
    );
    assert!(file_change.execution.receipt.is_some());
}

#[test]
fn startup_reconciles_published_automatic_staged_file_changes_without_replaying_them() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    // Construct the Host before seeding the synthetic interrupted turn so startup recovery does
    // not terminalize the fixture trace before its hidden Pending journal exists.
    let service = AgentService::new_authorized_for_test(Arc::clone(&storage));
    let run_id = "run-automatic-staged-crash";
    let conversation_id = "conversation-automatic-staged-crash";
    let assistant_message_id = "assistant-automatic-staged-crash";
    let call_id = "call-automatic-staged-crash";
    let transaction_id = "transaction-automatic-staged-crash";
    let target_content = "automatically published from apply_patch\n";
    let relative_path = "automatic-staged.txt";
    let target_path = std::fs::canonicalize(fixture.path())
        .unwrap()
        .join(relative_path);
    let (file_change, call) = staged_file_change_fixture(
        StagedFileChangeFixtureIdentity {
            run_id,
            conversation_id,
            call_id,
            transaction_id,
        },
        (relative_path, target_path.to_str().unwrap()),
        target_content,
        AgentApprovalStatus::Approved,
    );
    let action = AgentProposedAction::FileChange {
        file_change: file_change.clone(),
    };
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
    let mut hidden = service
        .prepare_auto_file_change_action_journal(
            run_id,
            conversation_id,
            assistant_message_id,
            action,
            agent_input,
        )
        .unwrap();
    assert!(service
        .claim_auto_file_change_dispatch(&mut hidden)
        .unwrap());

    std::fs::write(&target_path, target_content).unwrap();
    let metadata_before = std::fs::metadata(&target_path).unwrap();
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
    let _restarted = AgentService::try_new_deferred_startup_reconciliation(Arc::clone(&storage))
        .expect("published automatic Staged FileChange must reconcile without replay");
    let results = storage
        .list_agent_tool_results_for_run(run_id, "apply_patch")
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].call_id, call_id);
    assert_eq!(results[0].tool, "apply_patch");
    assert!(results[0].ok);
}

#[test]
fn startup_finalizes_a_committed_automatic_direct_delete_before_terminal_audit() {
    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("storage.sqlite");
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    // Construct the Host before seeding the synthetic interrupted turn so startup recovery does
    // not terminalize the fixture trace before its hidden Pending journal exists.
    let service = AgentService::new_authorized_for_test(Arc::clone(&storage));
    let run_id = "run-automatic-delete-finalize-crash";
    let conversation_id = "conversation-automatic-delete-finalize-crash";
    let assistant_message_id = "assistant-automatic-delete-finalize-crash";
    let call_id = "call-automatic-delete-finalize-crash";
    let target_path = std::fs::canonicalize(fixture.path())
        .unwrap()
        .join("automatic-delete-finalize.txt");
    std::fs::write(&target_path, "private deleted contents\n").unwrap();
    let (action, call) = direct_file_change_fixture(
        run_id,
        conversation_id,
        call_id,
        (
            "automatic-delete-finalize.txt",
            target_path.to_str().unwrap(),
        ),
        Some("private deleted contents\n"),
        None,
        AgentApprovalStatus::Approved,
    );
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
    let mut hidden = service
        .prepare_auto_file_change_action_journal(
            run_id,
            conversation_id,
            assistant_message_id,
            action,
            agent_input,
        )
        .unwrap();
    assert!(service
        .claim_auto_file_change_dispatch(&mut hidden)
        .unwrap());

    let mut committed_action = hidden.snapshot.action.clone();
    let AgentProposedAction::FileChange { file_change } = &mut committed_action else {
        unreachable!("fixture must produce a FileChange")
    };
    let target =
        mycopilot_core::file_change::FileChangePathPolicy::new(Some(fixture.path()), false)
            .resolve("automatic-delete-finalize.txt")
            .unwrap();
    let plan = mycopilot_core::file_change::FileChangePlan::from_direct_binding(
        &file_change.execution,
        file_change.inline_diff.as_ref().unwrap().patch.clone(),
    )
    .unwrap();
    let mut journal = file_change.execution.delete_journal.clone().unwrap();
    let commit = mycopilot_core::file_change::FileChangeCommitter
        .commit(
            &file_change.execution.transaction.id,
            &target,
            &plan,
            2,
            Some(&mut journal),
        )
        .unwrap();
    let tombstone_path = commit
        .delete_journal
        .as_ref()
        .unwrap()
        .tombstone_path
        .clone();
    *file_change.execution = file_change.execution.with_commit(&commit).unwrap();
    service
        .commit_pending_file_change_action(&mut hidden, committed_action)
        .unwrap();
    assert!(!target_path.exists());
    assert!(std::path::Path::new(&tombstone_path).exists());

    assert_eq!(
        reconcile_interrupted_file_changes(&storage, mycopilot_core::storage::now_ms()).unwrap(),
        1
    );
    assert!(!target_path.exists());
    assert!(!std::path::Path::new(&tombstone_path).exists());

    let connection = rusqlite::Connection::open(database_path).unwrap();
    let (status, action_json, tool_result_json): (String, String, Option<String>) = connection
        .query_row(
            "SELECT status, action_json, tool_result_json
             FROM agent_action_audit WHERE action_id = ?1",
            [pending_action_storage_id(run_id, call_id)],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(status, "completed");
    assert!(tool_result_json.is_some());
    let finalized: AgentProposedAction = serde_json::from_str(&action_json).unwrap();
    let AgentProposedAction::FileChange { file_change } = finalized else {
        unreachable!("finalized action remains a FileChange")
    };
    assert_eq!(
        file_change.execution.delete_journal.as_ref().unwrap().state,
        mycopilot_core::file_change::FileChangeDeleteJournalState::Finalized
    );
    assert!(file_change.execution.receipt.is_some());
}
