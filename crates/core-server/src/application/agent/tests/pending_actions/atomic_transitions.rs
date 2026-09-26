use super::fixtures::test_pending_resume_checkpoint_for_call;
use super::*;

#[test]
fn missing_pending_transition_row_fails_closed_without_terminal_success() {
    const MARKER: &str = "MISSING_TRANSITION_SKILL_BODY";
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let service = AgentService::new_authorized_for_test(Arc::clone(&storage));
    let mut agent_input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "secret",
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "messages": []
    }))
    .unwrap();
    agent_input.skill_activation = Some(AgentSkillActivation {
        activation_revision: "activation-missing-row".to_string(),
        skills: vec![AgentActivatedSkill {
            id: "bundled:application/test-bundled-skill".to_string(),
            name: "test-bundled-skill".to_string(),
            revision: "package-missing-row".to_string(),
            source: "bundled:application".to_string(),
            instructions: MARKER.to_string(),
            source_bytes: u64::try_from(MARKER.len()).unwrap(),
            resources: None,
        }],
    });
    let call = AgentToolCall {
        id: "action-missing-transition-row".to_string(),
        tool: "approval_tool".to_string(),
        args: json!({}),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };
    save_test_pending_provider_for_input(&storage, &mut agent_input);
    agent_input.resume_checkpoint = Some(test_pending_resume_checkpoint_for_call(
        &storage,
        "run-missing-transition-row",
        None,
        &call,
        AgentToolIdentity::Unregistered {
            tool_name: call.tool.clone(),
        },
    ));
    service
        .store_pending_action(
            "run-missing-transition-row",
            "conversation-missing-transition-row",
            "assistant-missing-transition-row",
            AgentProposedAction::ToolCall { call: call.clone() },
            agent_input,
        )
        .unwrap();
    assert!(storage.list_pending_agent_actions().unwrap()[0]
        .agent_input_json
        .contains(MARKER));

    // Reproduce a cross-boundary missing-row race: durable conversation deletion has removed
    // the pending row while this service instance still owns its pre-deletion memory snapshot.
    storage
        .delete_conversation("conversation-missing-transition-row")
        .unwrap();
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let error = service
        .approve_action("run-missing-transition-row", &call.id, notifications)
        .unwrap_err();
    assert!(error.contains("实际更新 0 条"));
    assert!(receiver.try_recv().is_err());
    assert_eq!(
        service
            .pending_actions
            .lock()
            .unwrap_or_else(|lock_error| lock_error.into_inner())
            [&pending_action_storage_id("run-missing-transition-row", &call.id)]
            .snapshot
            .status,
        PendingActionStatus::Pending
    );

    let reloaded = AgentService::new_authorized_for_test(storage);
    assert!(reloaded.list_pending_actions().is_empty());
}

#[test]
fn invalid_checkpoint_tool_call_is_rejected_before_pending_publication() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let service = AgentService::new_authorized_for_test(Arc::clone(&storage));
    let call = AgentToolCall {
        id: "call-invalid-checkpoint".to_string(),
        tool: "approval_tool".to_string(),
        args: json!({ "value": "trusted" }),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };
    let mut agent_input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "secret",
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "messages": []
    }))
    .unwrap();
    agent_input.resume_checkpoint = Some(AgentRunCheckpoint {
        pause_reason: mycopilot_core::AgentRunCheckpointPauseReason::Approval,
        version: AGENT_RUN_CHECKPOINT_SCHEMA_VERSION,
        run_id: "run-invalid-checkpoint".to_string(),
        pending_action_id: None,
        file_change_run_grant_ref: None,
        pending_file_observation: None,
        context_items: vec![mycopilot_core::AgentContextCheckpointItem {
            context_image_refs: Vec::new(),
            role: "assistant".to_string(),
            content: String::new(),
            images: Vec::new(),
            tool_call_id: None,
            tool_calls: vec![mycopilot_core::AgentContextCheckpointToolCall {
                id: call.id.clone(),
                name: "tampered_tool".to_string(),
                args: call.args.clone(),
                provider_identity: mycopilot_core::AgentProviderToolCallIdentity {
                    provider_tool_index: 0,
                    provider_call_id: call.id.clone(),
                    runtime_call_id: call.id.clone(),
                },
            }],
            is_error: false,
            sources: vec!["tool_call".to_string()],
            scope: "run".to_string(),
            retention: "retained".to_string(),
            request_order: None,
            group: None,
            origin: None,
        }],
        next_model_request_index: 1,
        queued_tool_calls: Vec::new(),
        deferred_external_tool_call_count: 0,
        suppressed_narration: false,
        extension_snapshots: Vec::new(),
        tool_set: crate::test_tool_set_checkpoint(),
        run_context: None,
        collaboration_run_snapshot: None,
        model_capabilities: ModelCapabilities::default(),
        provider_profile_config: crate::test_provider_profile_config(),
        provider_protocol_key: crate::test_provider_protocol_key("test-model"),
        assistant_turn_identity: crate::test_assistant_turn_identity(&[call.id.as_str()]),
        provider_continuation_refs: Vec::new(),
        conversation_world_state_records: Vec::new(),
        run_world_state: crate::test_run_world_state(),
        pending_tool_call_id: call.id.clone(),
        conversation_model_context_items: Vec::new(),
        conversation_trace_items: Vec::new(),
        next_conversation_trace_sequence: 0,
        conversation_trace_truncated: false,
    });
    save_test_pending_provider_for_input(&storage, &mut agent_input);
    let error = service
        .store_pending_action(
            "run-invalid-checkpoint",
            "conversation-invalid-checkpoint",
            "assistant-invalid-checkpoint",
            AgentProposedAction::ToolCall { call: call.clone() },
            agent_input,
        )
        .unwrap_err();
    assert_eq!(
        error,
        "Pending action frozen Tool Call identity is inconsistent."
    );
    assert!(service.list_pending_actions().is_empty());
    assert!(storage.list_pending_agent_actions().unwrap().is_empty());
}

#[test]
fn cancel_finalize_failure_atomically_restores_pending_payload() {
    const MARKER: &str = "CANCEL_ROLLBACK_SKILL_BODY";
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let service = AgentService::new_authorized_for_test(Arc::clone(&storage));
    let call = AgentToolCall {
        id: "action-cancel-rollback".to_string(),
        tool: "approval_tool".to_string(),
        args: json!({}),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };
    let mut agent_input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "secret",
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "messages": []
    }))
    .unwrap();
    agent_input.skill_activation = Some(AgentSkillActivation {
        activation_revision: "activation-cancel-rollback".to_string(),
        skills: vec![AgentActivatedSkill {
            id: "bundled:application/test-bundled-skill".to_string(),
            name: "test-bundled-skill".to_string(),
            revision: "package-cancel-rollback".to_string(),
            source: "bundled:application".to_string(),
            instructions: MARKER.to_string(),
            source_bytes: u64::try_from(MARKER.len()).unwrap(),
            resources: None,
        }],
    });
    save_test_pending_provider_for_input(&storage, &mut agent_input);
    agent_input.resume_checkpoint = Some(test_pending_resume_checkpoint_for_call(
        &storage,
        "run-cancel-rollback",
        None,
        &call,
        AgentToolIdentity::Unregistered {
            tool_name: call.tool.clone(),
        },
    ));
    // These durable owner rows intentionally do not exist, forcing final trace persistence to
    // fail after the action has first transitioned to cancelled.
    service
        .store_pending_action(
            "run-cancel-rollback",
            "conversation-cancel-rollback-missing",
            "assistant-cancel-rollback-missing",
            AgentProposedAction::ToolCall { call: call.clone() },
            agent_input,
        )
        .unwrap();

    assert!(service
        .cancel_action("run-cancel-rollback", &call.id)
        .is_err());
    assert_eq!(
        service.list_pending_actions()[0].status,
        PendingActionStatus::Pending
    );
    let persisted = storage.list_pending_agent_actions().unwrap();
    assert_eq!(persisted.len(), 1);
    assert_eq!(persisted[0].status, "pending");
    assert_eq!(persisted[0].target_status, None);
    assert!(persisted[0].agent_input_json.contains(MARKER));

    // The compensating transition must make the action reusable. A stale cancelled marker
    // would make this second attempt fail before finalization and strand the row.
    let second_error = service
        .cancel_action("run-cancel-rollback", &call.id)
        .unwrap_err();
    assert!(!second_error.contains("无法写入 cancelled 目标终态"));
    let persisted = storage.list_pending_agent_actions().unwrap();
    assert_eq!(persisted.len(), 1);
    assert_eq!(persisted[0].status, "pending");
    assert_eq!(persisted[0].target_status, None);
}

#[test]
fn cancel_usage_failure_rolls_back_message_trace_and_action_together() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    storage
        .save_conversation(ChatConversationRecord {
            id: "conversation-cancel-usage-failure".to_string(),
            project_id: None,
            model_id: Some("model-1".to_string()),
            title: "Cancel usage failure".to_string(),
            messages: vec![ChatMessageRecord {
                human_interaction_response: None,
                id: "assistant-cancel-usage-failure".to_string(),
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
    // The invalid usage owner is a deterministic fault injection: the usage insert violates
    // its foreign key only after message and trace writes have run inside the transaction.
    service.register_usage_context(
        "run-cancel-usage-failure",
        AgentRunUsageContext {
            conversation_id: "missing-usage-conversation".to_string(),
            assistant_message_id: "assistant-cancel-usage-failure".to_string(),
            run_id: "run-cancel-usage-failure".to_string(),
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
    let call = AgentToolCall {
        id: "call-cancel-usage-failure".to_string(),
        tool: "approval_tool".to_string(),
        args: json!({}),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };
    let mut agent_input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "secret",
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "messages": []
    }))
    .unwrap();
    save_test_pending_provider_for_input(&storage, &mut agent_input);
    agent_input.resume_checkpoint = Some(test_pending_resume_checkpoint_for_call(
        &storage,
        "run-cancel-usage-failure",
        None,
        &call,
        AgentToolIdentity::Unregistered {
            tool_name: call.tool.clone(),
        },
    ));
    service
        .store_pending_action(
            "run-cancel-usage-failure",
            "conversation-cancel-usage-failure",
            "assistant-cancel-usage-failure",
            AgentProposedAction::ToolCall { call: call.clone() },
            agent_input,
        )
        .unwrap();

    assert!(service
        .cancel_action("run-cancel-usage-failure", &call.id)
        .is_err());
    let pending = storage.list_pending_agent_actions().unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].status, "pending");
    assert_eq!(pending[0].target_status, None);
    assert!(storage
        .get_conversation_turn_trace("assistant-cancel-usage-failure")
        .unwrap()
        .is_none());
    let conversation = storage
        .load_conversation("conversation-cancel-usage-failure")
        .unwrap()
        .unwrap();
    assert!(conversation.messages[0].content.is_empty());
    assert_eq!(conversation.messages[0].status.as_deref(), Some("pending"));
    assert!(storage
        .list_agent_tool_results_for_run("run-cancel-usage-failure", "approval_tool")
        .unwrap()
        .is_empty());
}
