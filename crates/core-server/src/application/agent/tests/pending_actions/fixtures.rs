use super::mcp_fixtures::test_mcp_resume_checkpoint;
use super::*;

pub(super) fn test_pending_resume_checkpoint_for_call(
    storage: &StorageService,
    run_id: &str,
    pending_action_id: Option<&str>,
    call: &AgentToolCall,
    provenance: AgentToolIdentity,
) -> AgentRunCheckpoint {
    let seed = pending_action_id.unwrap_or(call.id.as_str());
    let mut checkpoint = test_mcp_resume_checkpoint(storage, run_id, seed);
    let provider_identity = AgentProviderToolCallIdentity {
        provider_tool_index: 0,
        provider_call_id: call.id.clone(),
        runtime_call_id: call.id.clone(),
    };
    checkpoint.pending_action_id = pending_action_id.map(ToString::to_string);
    checkpoint.pending_tool_call_id = call.id.clone();
    checkpoint.context_items[0].tool_calls = vec![AgentContextCheckpointToolCall {
        id: call.id.clone(),
        name: call.tool.clone(),
        args: call.args.clone(),
        provider_identity: provider_identity.clone(),
    }];
    checkpoint.assistant_turn_identity = crate::test_assistant_turn_identity(&[call.id.as_str()]);
    let (trace_operation, trace_truncated) = durable_test_tool_call_operation(call);
    checkpoint.conversation_trace_items = vec![ConversationTurnTraceItem::ToolCall {
        sequence: 0,
        call_id: call.id.clone(),
        tool: call.tool.clone(),
        operation: trace_operation,
        provenance,
        approval_status: call.approval_status,
        truncated: trace_truncated,
    }];
    checkpoint.conversation_model_context_items = vec![ConversationModelContextItem {
        images: Vec::new(),
        sequence: 0,
        ordinal: 0,
        role: "assistant".to_string(),
        content: String::new(),
        tool_call_id: None,
        tool_calls: vec![AgentContextCheckpointToolCall {
            id: call.id.clone(),
            name: call.tool.clone(),
            args: call.args.clone(),
            provider_identity,
        }],
        is_error: false,
    }];
    checkpoint.next_conversation_trace_sequence = 1;
    checkpoint.conversation_trace_truncated = trace_truncated;
    checkpoint
}

fn durable_test_tool_call_operation(call: &AgentToolCall) -> (serde_json::Value, bool) {
    if call.tool == "apply_patch" {
        let operation = mycopilot_core::file_change_support::apply_patch_trace_operation(
            &call.args,
        )
        .expect("project test apply_patch call through the production durable Trace boundary");
        let truncated = operation != call.args;
        (operation, truncated)
    } else {
        (call.args.clone(), false)
    }
}

pub(super) fn seed_durable_pending_owner(
    storage: &StorageService,
    conversation_id: &str,
    assistant_message_id: &str,
    run_id: &str,
    call: &AgentToolCall,
    provenance: AgentToolIdentity,
    created_at: i64,
) {
    let agent_run_json = json!({
        "runId": run_id,
        "status": "waiting_for_approval",
        "startedAt": created_at,
        "toolDefinitions": [],
        "toolCalls": [],
        "toolResults": [],
        "approvals": [],
        "fileChangeProposals": [],
        "fileChanges": [],
        "webSearchActivities": [],
        "readActivities": [],
        "mcpInvocations": [],
        "timeline": [],
        "messageStreamCheckpoints": {},
        "state": {
            "status": "waiting_for_approval",
            "activeRunId": run_id,
            "lastError": null,
            "updatedAt": created_at
        }
    })
    .to_string();
    storage
        .save_conversation(ChatConversationRecord {
            id: conversation_id.to_string(),
            project_id: None,
            model_id: Some("test-model".to_string()),
            title: "Pending action recovery".to_string(),
            messages: vec![ChatMessageRecord {
                human_interaction_response: None,
                id: assistant_message_id.to_string(),
                role: "assistant".to_string(),
                content: String::new(),
                created_at,
                status: Some("pending".to_string()),
                attachments: Vec::new(),
                folder_references_json: None,
                agent_run_json: Some(agent_run_json),
                ui_state_json: None,
            }],
            created_at,
            updated_at: created_at,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
    append_durable_pending_trace(
        storage,
        conversation_id,
        assistant_message_id,
        run_id,
        call,
        provenance,
        created_at,
    );
}

pub(super) fn append_durable_pending_trace(
    storage: &StorageService,
    conversation_id: &str,
    assistant_message_id: &str,
    run_id: &str,
    call: &AgentToolCall,
    provenance: AgentToolIdentity,
    created_at: i64,
) {
    let provider_identity = AgentProviderToolCallIdentity {
        provider_tool_index: 0,
        provider_call_id: call.id.clone(),
        runtime_call_id: call.id.clone(),
    };
    let (trace_operation, trace_truncated) = durable_test_tool_call_operation(call);
    let trace = ConversationTurnTrace {
        schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
        run_id: run_id.to_string(),
        conversation_id: conversation_id.to_string(),
        assistant_message_id: assistant_message_id.to_string(),
        terminal_status: ConversationTurnTraceTerminalStatus::InProgress,
        terminal_error: None,
        truncated: trace_truncated,
        items: vec![ConversationTurnTraceItem::ToolCall {
            sequence: 0,
            call_id: call.id.clone(),
            tool: call.tool.clone(),
            provenance,
            operation: trace_operation,
            approval_status: call.approval_status,
            truncated: trace_truncated,
        }],
    };
    let model_context = vec![ConversationModelContextItem {
        images: Vec::new(),
        sequence: 0,
        ordinal: 0,
        role: "assistant".to_string(),
        content: String::new(),
        tool_call_id: None,
        tool_calls: vec![AgentContextCheckpointToolCall {
            id: call.id.clone(),
            name: call.tool.clone(),
            args: call.args.clone(),
            provider_identity,
        }],
        is_error: false,
    }];
    admit_test_conversation_run(storage, &trace, AgentPermissions::default(), created_at);
    storage
        .append_in_progress_conversation_turn_trace_and_apply_guidances(
            &trace,
            &model_context,
            created_at,
            created_at,
        )
        .unwrap();
}

pub(super) fn valid_resume_collaboration_identity() -> mycopilot_core::AgentCollaborationIdentity {
    mycopilot_core::AgentCollaborationIdentity {
        agent_id: "agent-child-resume".to_string(),
        root_agent_id: "agent-root-resume".to_string(),
        root_conversation_id: "conversation-root-resume".to_string(),
        parent_agent_id: "agent-root-resume".to_string(),
        parent_task_name: "Root".to_string(),
        parent_task_path: "root".to_string(),
        conversation_id: "conversation-child-resume".to_string(),
        task_name: "review".to_string(),
        task_path: "root/review".to_string(),
        source_agent_id: "agent-root-resume".to_string(),
        source_kind: mycopilot_core::AgentMailboxKind::Task,
        source_task_name: "Root".to_string(),
        source_task_path: "root".to_string(),
        source_agent_message_id: "mailbox-task-resume".to_string(),
        entrusted_task: "Review the durable facts.".to_string(),
        template_instructions: None,
    }
}
