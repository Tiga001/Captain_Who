use super::fixtures::{
    append_durable_pending_trace, seed_durable_pending_owner,
    test_pending_resume_checkpoint_for_call, valid_resume_collaboration_identity,
};
use super::*;

#[test]
fn direct_file_change_execution_is_private_to_renderer_but_durable_for_restart() {
    const PRIVATE_WHOLE_FILE_MARKER: &str = "PRIVATE_FILE_CHANGE_CONTENT_MUST_STAY_HOST_SIDE";

    let fixture = tempdir().unwrap();
    let storage = StorageService::open(&fixture.path().join("storage.sqlite")).unwrap();
    save_test_pending_provider(
        &storage,
        "test-model",
        "https://example.test/v1/chat/completions",
        "test-token",
        "disabled",
        "",
    );

    let filler = (1..=12)
        .map(|index| format!("unchanged line {index}"))
        .collect::<Vec<_>>()
        .join("\n");
    let base_content = format!("{PRIVATE_WHOLE_FILE_MARKER}\n{filler}\npublic status: before\n");
    let target_content = format!("{PRIVATE_WHOLE_FILE_MARKER}\n{filler}\npublic status: after\n");
    let run_id = "run-direct-file-change";
    let conversation_id = "conversation-direct-file-change";
    let call_id = "call-direct-file-change";
    let canonical_target = fixture.path().join("report.txt");
    let (action, call) = direct_file_change_fixture(
        run_id,
        conversation_id,
        call_id,
        ("report.txt", canonical_target.to_str().unwrap()),
        Some(&base_content),
        Some(&target_content),
        AgentApprovalStatus::Required,
    );
    let AgentProposedAction::FileChange { file_change } = &action else {
        panic!("fixture must produce a FileChange action");
    };
    assert!(
        !file_change
            .inline_diff
            .as_ref()
            .unwrap()
            .patch
            .contains(PRIVATE_WHOLE_FILE_MARKER),
        "the presentation diff fixture must not itself contain the private whole-file marker"
    );

    let renderer_event = agent_event_notification(AgentEvent::FileChangeProposed {
        run_id: run_id.to_string(),
        file_change: file_change.clone(),
    });
    let observer_event = child_observer_event_notification(
        &valid_resume_collaboration_identity(),
        run_id,
        "assistant-direct-file-change",
        AgentEvent::FileChangeProposed {
            run_id: run_id.to_string(),
            file_change: file_change.clone(),
        },
    );
    assert_eq!(renderer_event["method"], AGENT_EVENT_NAME);
    let pending_snapshot = PendingAgentActionSnapshot {
        action_id: call_id.to_string(),
        action_type: "file_change".to_string(),
        tool_name: "apply_patch".to_string(),
        tool_call_id: Some(call_id.to_string()),
        run_id: run_id.to_string(),
        conversation_id: Some(conversation_id.to_string()),
        assistant_message_id: Some("assistant-direct-file-change".to_string()),
        action: action.clone(),
        created_at: 1,
        status: PendingActionStatus::Pending,
    };
    let renderer_pending = serde_json::to_value(&pending_snapshot).unwrap();

    assert!(renderer_event["params"]["fileChange"]
        .get("execution")
        .is_none());
    assert!(observer_event["params"]["event"]["fileChange"]
        .get("execution")
        .is_none());
    assert!(renderer_pending["action"]["fileChange"]
        .get("execution")
        .is_none());
    for (boundary, projection) in [
        ("agent.event", &renderer_event),
        ("agent observer event", &observer_event),
        ("pending snapshot", &renderer_pending),
    ] {
        let encoded = serde_json::to_string(projection).unwrap();
        for forbidden in [
            "\"execution\"",
            "\"baseContent\"",
            "\"targetContent\"",
            PRIVATE_WHOLE_FILE_MARKER,
        ] {
            assert!(
                !encoded.contains(forbidden),
                "{boundary} leaked Host-private FileChange material `{forbidden}`"
            );
        }
    }

    let mut agent_input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "test-token",
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "messages": []
    }))
    .unwrap();
    freeze_test_pending_provider_configuration(&storage, &mut agent_input);
    agent_input.resume_checkpoint = Some(test_pending_resume_checkpoint_for_call(
        &storage,
        run_id,
        None,
        &call,
        AgentToolIdentity::Builtin {
            tool_name: "apply_patch".to_string(),
        },
    ));
    let record = PendingActionRecord {
        storage_id: pending_action_storage_id(run_id, call_id),
        snapshot: pending_snapshot,
        agent_input,
    };
    let durable = pending_storage_record(&record, 2).unwrap();
    let durable_action: Value = serde_json::from_str(&durable.action_json).unwrap();
    assert_eq!(
        durable_action["fileChange"]["execution"]["baseContent"],
        base_content
    );
    assert_eq!(
        durable_action["fileChange"]["execution"]["targetContent"],
        target_content
    );
    let restored: AgentProposedAction = serde_json::from_str(&durable.action_json).unwrap();
    let AgentProposedAction::FileChange {
        file_change: restored,
    } = restored
    else {
        panic!("durable action must round-trip as a FileChange");
    };
    restored.execution.validate().unwrap();
    assert_eq!(restored.execution.source_call_id, call_id);
    assert_eq!(restored.execution.conversation_id, conversation_id);
    assert_eq!(restored.execution.run_id, run_id);
    assert_eq!(
        restored.execution.base_content.as_deref(),
        Some(base_content.as_str())
    );
    assert_eq!(
        restored.execution.target_content.as_deref(),
        Some(target_content.as_str())
    );
}

#[test]
fn file_change_pending_checkpoint_requires_exact_canonical_pending_action_id() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let service = AgentService::new_authorized_for_test(Arc::clone(&storage));
    let run_id = "run-file-change-checkpoint-id";
    let conversation_id = "conversation-file-change-checkpoint-id";
    let assistant_message_id = "assistant-file-change-checkpoint-id";
    let call_id = "call-file-change-checkpoint-id";
    let target = fixture.path().join("checkpoint-id.txt");
    let (action, call) = direct_file_change_fixture(
        run_id,
        conversation_id,
        call_id,
        ("checkpoint-id.txt", target.to_str().unwrap()),
        None,
        Some("checkpoint identity\n"),
        AgentApprovalStatus::Required,
    );
    let mut base_input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "test-token",
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "messages": []
    }))
    .unwrap();
    save_test_pending_provider_for_input(&storage, &mut base_input);
    let run_context = AgentRunContext {
        conversation_id: Some(conversation_id.to_string()),
        project_id: None,
        workspace: None,
        attachment_library: None,
        permissions: AgentPermissions::default(),
        collaboration_identity: None,
    };
    base_input.context = Some(run_context.clone());
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
    let canonical_id = pending_action_storage_id(run_id, call_id);

    for pending_action_id in [None, Some("v2:tampered")].into_iter() {
        let mut input = base_input.clone();
        let mut checkpoint = test_pending_resume_checkpoint_for_call(
            &storage,
            run_id,
            pending_action_id,
            &call,
            AgentToolIdentity::Builtin {
                tool_name: "apply_patch".to_string(),
            },
        );
        checkpoint.run_context = Some(run_context.clone());
        input.resume_checkpoint = Some(checkpoint);
        freeze_test_pending_provider_configuration(&storage, &mut input);
        let error = service
            .store_pending_action(
                run_id,
                conversation_id,
                assistant_message_id,
                action.clone(),
                input,
            )
            .unwrap_err();
        assert_eq!(
            error,
            "Pending action frozen Tool Call identity is inconsistent."
        );
        assert!(storage.list_pending_agent_actions().unwrap().is_empty());
    }

    let mut exact_input = base_input;
    let mut checkpoint = test_pending_resume_checkpoint_for_call(
        &storage,
        run_id,
        Some(&canonical_id),
        &call,
        AgentToolIdentity::Builtin {
            tool_name: "apply_patch".to_string(),
        },
    );
    checkpoint.run_context = Some(run_context);
    exact_input.resume_checkpoint = Some(checkpoint);
    freeze_test_pending_provider_configuration(&storage, &mut exact_input);
    assert!(service
        .store_pending_action(
            run_id,
            conversation_id,
            assistant_message_id,
            action,
            exact_input,
        )
        .unwrap());
    assert_eq!(
        storage.list_pending_agent_actions().unwrap()[0].action_id,
        canonical_id
    );
}

#[test]
fn cancelled_file_change_with_durable_abort_never_rolls_back_to_pending() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    storage
        .save_conversation(ChatConversationRecord {
            id: "conversation-file-change-cancel-failure".to_string(),
            project_id: None,
            model_id: Some("model-1".to_string()),
            title: "FileChange cancel failure".to_string(),
            messages: vec![ChatMessageRecord {
                human_interaction_response: None,
                id: "assistant-file-change-cancel-failure".to_string(),
                role: "assistant".to_string(),
                content: String::new(),
                created_at: 1,
                status: Some("pending".to_string()),
                attachments: Vec::new(),
                folder_references_json: None,
                agent_run_json: None,
                ui_state_json: None,
            }],
            created_at: 1,
            updated_at: 1,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
    let service = AgentService::new_authorized_for_test(Arc::clone(&storage));
    service.register_usage_context(
        "run-file-change-cancel-failure",
        AgentRunUsageContext {
            conversation_id: "missing-file-change-usage-owner".to_string(),
            assistant_message_id: "assistant-file-change-cancel-failure".to_string(),
            run_id: "run-file-change-cancel-failure".to_string(),
            project_id: None,
            model_id: "model-1".to_string(),
            model_name: "Model 1".to_string(),
            provider_usage_semantics: ProviderUsageSemantics::StandardAdditive,
            input_price: None,
            cached_input_price: None,
            output_price: None,
            started_at: 1,
        },
    );
    let action_id = "file-change-cancel-failure";
    let canonical_target = fixture.path().join("cancelled.txt");
    let (file_change, call) = staged_file_change_fixture(
        StagedFileChangeFixtureIdentity {
            run_id: "run-file-change-cancel-failure",
            conversation_id: "conversation-file-change-cancel-failure",
            call_id: action_id,
            transaction_id: "transaction-file-change-cancel-failure",
        },
        ("cancelled.txt", canonical_target.to_str().unwrap()),
        "hello",
        AgentApprovalStatus::Required,
    );
    storage
        .create_agent_file_change(staged_file_change_record(&file_change, "waiting_approval"))
        .unwrap();
    let mut agent_input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "secret",
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "messages": []
    }))
    .unwrap();
    let run_context = AgentRunContext {
        collaboration_identity: None,
        conversation_id: Some("conversation-file-change-cancel-failure".to_string()),
        project_id: None,
        workspace: None,
        attachment_library: None,
        permissions: AgentPermissions::default(),
    };
    agent_input.context = Some(run_context.clone());
    save_test_pending_provider_for_input(&storage, &mut agent_input);
    let mut checkpoint = test_pending_resume_checkpoint_for_call(
        &storage,
        "run-file-change-cancel-failure",
        Some(&pending_action_storage_id(
            "run-file-change-cancel-failure",
            action_id,
        )),
        &call,
        AgentToolIdentity::Builtin {
            tool_name: call.tool.clone(),
        },
    );
    checkpoint.run_context = Some(run_context);
    agent_input.resume_checkpoint = Some(checkpoint);
    append_durable_pending_trace(
        &storage,
        "conversation-file-change-cancel-failure",
        "assistant-file-change-cancel-failure",
        "run-file-change-cancel-failure",
        &call,
        AgentToolIdentity::Builtin {
            tool_name: call.tool.clone(),
        },
        1,
    );
    service
        .store_pending_action(
            "run-file-change-cancel-failure",
            "conversation-file-change-cancel-failure",
            "assistant-file-change-cancel-failure",
            AgentProposedAction::FileChange { file_change },
            agent_input,
        )
        .unwrap();

    let error = service
        .cancel_action("run-file-change-cancel-failure", action_id)
        .unwrap_err();
    assert!(
        error.contains("保持 executing"),
        "unexpected error: {error}"
    );
    assert_eq!(
        service
            .pending_actions
            .lock()
            .unwrap_or_else(|lock_error| lock_error.into_inner())
            [&pending_action_storage_id("run-file-change-cancel-failure", action_id,)]
            .snapshot
            .status,
        PendingActionStatus::Executing
    );
    assert_eq!(
        storage
            .get_agent_file_change("transaction-file-change-cancel-failure")
            .unwrap()
            .unwrap()
            .status,
        "aborted"
    );
    assert_eq!(
        storage
            .get_conversation_turn_trace("assistant-file-change-cancel-failure")
            .unwrap()
            .unwrap()
            .terminal_status,
        ConversationTurnTraceTerminalStatus::InProgress
    );
    let conversation = storage
        .load_conversation("conversation-file-change-cancel-failure")
        .unwrap()
        .unwrap();
    assert_eq!(conversation.messages[0].status.as_deref(), Some("pending"));
    let reconciled_at = mycopilot_core::storage::now_ms();
    assert_eq!(
        reconcile_interrupted_file_changes(&storage, reconciled_at).unwrap(),
        1,
        "startup pre-pass must rebuild the exact aborted ToolResult before terminalizing"
    );
    let recovered_trace = storage
        .get_conversation_turn_trace("assistant-file-change-cancel-failure")
        .unwrap()
        .unwrap();
    assert_eq!(
        recovered_trace
            .items
            .iter()
            .filter(|item| matches!(
                item,
                ConversationTurnTraceItem::ToolResult { call_id, .. }
                    if call_id == action_id
            ))
            .count(),
        1
    );
    let recovered_audit = storage
        .get_agent_action_audit(&pending_action_storage_id(
            "run-file-change-cancel-failure",
            action_id,
        ))
        .unwrap()
        .unwrap();
    let recovered_result: AgentFileChangeResult = serde_json::from_str(
        recovered_audit
            .file_change_result_json
            .as_deref()
            .expect("cancel recovery persists its typed FileChange result"),
    )
    .unwrap();
    assert_eq!(
        recovered_result.status,
        mycopilot_core::AgentFileChangeResultStatus::Aborted
    );
    assert_eq!(
        recovered_result.outcome,
        mycopilot_core::AgentFileChangeOutcome::DefinitelyNotExecuted
    );
    let interrupted = storage
        .reconcile_interrupted_pending_agent_actions(reconciled_at)
        .unwrap();
    assert_eq!(interrupted.len(), 1);
    assert_eq!(interrupted[0].status, "executing");
    assert_eq!(interrupted[0].target_status.as_deref(), Some("cancelled"));
    assert!(storage.list_pending_agent_actions().unwrap().is_empty());
    assert_eq!(
        storage
            .get_conversation_turn_trace("assistant-file-change-cancel-failure")
            .unwrap()
            .unwrap()
            .terminal_status,
        ConversationTurnTraceTerminalStatus::Failed
    );
}
