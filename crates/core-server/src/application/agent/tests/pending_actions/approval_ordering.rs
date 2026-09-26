use super::fixtures::{seed_durable_pending_owner, test_pending_resume_checkpoint_for_call};
use super::*;

fn predecessor_gate_records(
    storage: &StorageService,
    run_id: &str,
    conversation_id: &str,
    assistant_message_id: &str,
    predecessor_status: PendingActionStatus,
) -> (PendingActionRecord, PendingActionRecord) {
    let predecessor_call = AgentToolCall {
        id: "predecessor-call".to_string(),
        tool: "approval_tool".to_string(),
        args: json!({}),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };
    let successor_call = AgentToolCall {
        id: "successor-call".to_string(),
        tool: "approval_tool".to_string(),
        args: json!({}),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };
    let provenance = AgentToolIdentity::Builtin {
        tool_name: "approval_tool".to_string(),
    };
    seed_durable_pending_owner(
        storage,
        conversation_id,
        assistant_message_id,
        run_id,
        &predecessor_call,
        provenance.clone(),
        1,
    );

    let trace = ConversationTurnTrace {
        schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
        run_id: run_id.to_string(),
        conversation_id: conversation_id.to_string(),
        assistant_message_id: assistant_message_id.to_string(),
        terminal_status: ConversationTurnTraceTerminalStatus::InProgress,
        terminal_error: None,
        truncated: false,
        items: vec![
            ConversationTurnTraceItem::ToolCall {
                sequence: 0,
                call_id: predecessor_call.id.clone(),
                tool: predecessor_call.tool.clone(),
                provenance: provenance.clone(),
                operation: predecessor_call.args.clone(),
                approval_status: predecessor_call.approval_status,
                truncated: false,
            },
            ConversationTurnTraceItem::ToolResult {
                sequence: 1,
                call_id: predecessor_call.id.clone(),
                tool: predecessor_call.tool.clone(),
                status: ConversationTraceToolResultStatus::Succeeded,
                success: true,
                observation: json!({ "status": "completed" }),
                approval_status: AgentApprovalStatus::Approved,
                error: None,
                truncated: false,
                archive: Default::default(),
            },
            ConversationTurnTraceItem::ToolCall {
                sequence: 2,
                call_id: successor_call.id.clone(),
                tool: successor_call.tool.clone(),
                provenance: provenance.clone(),
                operation: successor_call.args.clone(),
                approval_status: successor_call.approval_status,
                truncated: false,
            },
        ],
    };
    let model_context = vec![
        ConversationModelContextItem {
            images: Vec::new(),
            sequence: 0,
            ordinal: 0,
            role: "assistant".to_string(),
            content: String::new(),
            tool_call_id: None,
            tool_calls: vec![AgentContextCheckpointToolCall {
                id: predecessor_call.id.clone(),
                name: predecessor_call.tool.clone(),
                args: predecessor_call.args.clone(),
                provider_identity: AgentProviderToolCallIdentity {
                    provider_tool_index: 0,
                    provider_call_id: predecessor_call.id.clone(),
                    runtime_call_id: predecessor_call.id.clone(),
                },
            }],
            is_error: false,
        },
        ConversationModelContextItem {
            images: Vec::new(),
            sequence: 1,
            ordinal: 0,
            role: "tool".to_string(),
            content: json!({ "status": "completed" }).to_string(),
            tool_call_id: Some(predecessor_call.id.clone()),
            tool_calls: Vec::new(),
            is_error: false,
        },
        ConversationModelContextItem {
            images: Vec::new(),
            sequence: 2,
            ordinal: 0,
            role: "assistant".to_string(),
            content: String::new(),
            tool_call_id: None,
            tool_calls: vec![AgentContextCheckpointToolCall {
                id: successor_call.id.clone(),
                name: successor_call.tool.clone(),
                args: successor_call.args.clone(),
                provider_identity: AgentProviderToolCallIdentity {
                    provider_tool_index: 0,
                    provider_call_id: successor_call.id.clone(),
                    runtime_call_id: successor_call.id.clone(),
                },
            }],
            is_error: false,
        },
    ];
    storage
        .append_in_progress_conversation_turn_trace_and_apply_guidances(
            &trace,
            &model_context,
            2,
            2,
        )
        .unwrap();

    let mut base_input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "secret",
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "messages": []
    }))
    .unwrap();
    save_test_pending_provider_for_input(storage, &mut base_input);
    let context = AgentRunContext {
        conversation_id: Some(conversation_id.to_string()),
        project_id: None,
        workspace: None,
        attachment_library: None,
        permissions: AgentPermissions::default(),
        collaboration_identity: None,
    };
    base_input.context = Some(context.clone());

    let mut predecessor_input = base_input.clone();
    let mut predecessor_checkpoint = test_pending_resume_checkpoint_for_call(
        storage,
        run_id,
        None,
        &predecessor_call,
        provenance.clone(),
    );
    predecessor_checkpoint.run_context = Some(context.clone());
    predecessor_input.resume_checkpoint = Some(predecessor_checkpoint);
    let predecessor = PendingActionRecord {
        storage_id: pending_action_storage_id(run_id, &predecessor_call.id),
        snapshot: PendingAgentActionSnapshot {
            action_id: predecessor_call.id.clone(),
            action_type: "tool_call".to_string(),
            tool_name: predecessor_call.tool.clone(),
            tool_call_id: Some(predecessor_call.id.clone()),
            run_id: run_id.to_string(),
            conversation_id: Some(conversation_id.to_string()),
            assistant_message_id: Some(assistant_message_id.to_string()),
            action: AgentProposedAction::ToolCall {
                call: predecessor_call,
            },
            created_at: 1,
            status: predecessor_status,
        },
        agent_input: predecessor_input,
    };

    let mut successor_input = base_input;
    let mut successor_checkpoint =
        test_pending_resume_checkpoint_for_call(storage, run_id, None, &successor_call, provenance);
    successor_checkpoint.run_context = Some(context);
    successor_checkpoint.conversation_trace_items = trace.items;
    successor_checkpoint.conversation_model_context_items = model_context;
    successor_checkpoint.next_conversation_trace_sequence = 3;
    successor_input.resume_checkpoint = Some(successor_checkpoint);
    let successor = PendingActionRecord {
        storage_id: pending_action_storage_id(run_id, &successor_call.id),
        snapshot: PendingAgentActionSnapshot {
            action_id: successor_call.id.clone(),
            action_type: "tool_call".to_string(),
            tool_name: successor_call.tool.clone(),
            tool_call_id: Some(successor_call.id.clone()),
            run_id: run_id.to_string(),
            conversation_id: Some(conversation_id.to_string()),
            assistant_message_id: Some(assistant_message_id.to_string()),
            action: AgentProposedAction::ToolCall {
                call: successor_call,
            },
            // Both approvals belong to one logical Run and therefore share its authoritative
            // usage start; trace sequence, not wall-clock creation order, proves dependency.
            created_at: 1,
            status: PendingActionStatus::Pending,
        },
        agent_input: successor_input,
    };
    (predecessor, successor)
}

#[test]
fn successor_approval_is_blocked_until_its_durable_predecessor_settles() {
    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("storage.sqlite");
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    // Construct the service before seeding the synthetic crash window so startup reconciliation
    // cannot repair it for this live-process gate test.
    let service = AgentService::new_authorized_for_test(Arc::clone(&storage));
    let (predecessor, successor) = predecessor_gate_records(
        &storage,
        "run-predecessor-gate",
        "conversation-predecessor-gate",
        "assistant-predecessor-gate",
        PendingActionStatus::Executing,
    );
    let mut predecessor_row = pending_storage_record(&predecessor, 3).unwrap();
    predecessor_row.target_status = Some("completed".to_string());
    storage.store_pending_agent_action(predecessor_row).unwrap();
    storage
        .store_pending_agent_action(pending_storage_record(&successor, 3).unwrap())
        .unwrap();
    {
        let mut pending = service
            .pending_actions
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        pending.insert(predecessor.storage_id.clone(), predecessor.clone());
        pending.insert(successor.storage_id.clone(), successor.clone());
    }

    let (notifications, _receiver) = tokio::sync::mpsc::unbounded_channel();
    let error = service
        .queue_action_continuation(
            &successor.snapshot.run_id,
            &successor.snapshot.action_id,
            AgentApprovalDecisionStatus::Approved,
            None,
            notifications,
        )
        .unwrap_err();
    assert!(error.contains("前置工具结果尚未完成持久化结算"));
    assert_eq!(
        storage
            .get_pending_agent_action(&successor.storage_id)
            .unwrap()
            .unwrap()
            .status,
        "pending"
    );

    // Durable status wins over stale process-local state. A commit-unknown terminal CAS must not
    // leave B permanently blocked merely because this Host still remembers A as executing.
    storage
        .transition_pending_agent_action(&predecessor.storage_id, "executing", "completed", "{}", 4)
        .unwrap();
    assert!(!storage
        .pending_agent_action_has_unsettled_predecessor(
            &successor.storage_id,
            std::slice::from_ref(&predecessor.snapshot.action_id),
        )
        .unwrap());
}

#[test]
fn restart_loaded_pending_map_still_blocks_a_proven_successor() {
    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("storage.sqlite");
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    let (predecessor, successor) = predecessor_gate_records(
        &storage,
        "run-restarted-predecessor-gate",
        "conversation-restarted-predecessor-gate",
        "assistant-restarted-predecessor-gate",
        PendingActionStatus::Pending,
    );
    storage
        .store_pending_agent_action(pending_storage_record(&predecessor, 3).unwrap())
        .unwrap();
    storage
        .store_pending_agent_action(pending_storage_record(&successor, 3).unwrap())
        .unwrap();

    let reloaded = AgentService::new_authorized_for_test(Arc::clone(&storage));
    let loaded = reloaded
        .pending_actions
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    assert!(loaded.contains_key(&predecessor.storage_id));
    assert!(loaded.contains_key(&successor.storage_id));
    drop(loaded);

    let (notifications, _receiver) = tokio::sync::mpsc::unbounded_channel();
    let error = reloaded
        .queue_action_continuation(
            &successor.snapshot.run_id,
            &successor.snapshot.action_id,
            AgentApprovalDecisionStatus::Approved,
            None,
            notifications,
        )
        .unwrap_err();
    assert!(error.contains("前置工具结果尚未完成持久化结算"));
}
