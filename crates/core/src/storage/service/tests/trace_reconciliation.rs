use super::*;

fn assistant_run_message(
    id: &str,
    run_id: &str,
    run_status: &str,
    message_status: &str,
    position_time: i64,
) -> ChatMessageRecord {
    let terminal = matches!(run_status, "completed" | "failed" | "cancelled");
    let presentation_status = if run_status == "cancelled" {
        // Simulate a current crash window in which the top-level lifecycle committed before its
        // renderer projection. Startup rebuilds the projection from durable trace authority.
        "running"
    } else {
        run_status
    };
    let mut run = serde_json::json!({
        "runId": run_id,
        "assistantMessageId": id,
        "status": run_status,
        "timeline": [{ "kind": "presentation-only" }],
        "state": {
            "status": presentation_status,
            "activeRunId": if terminal {
                serde_json::Value::Null
            } else {
                serde_json::Value::String(run_id.to_string())
            },
            "updatedAt": position_time
        }
    });
    if terminal {
        run["completedAt"] = serde_json::json!(22);
    }
    ChatMessageRecord {
        id: id.to_string(),
        role: "assistant".to_string(),
        content: "partial response".to_string(),
        created_at: position_time,
        status: Some(message_status.to_string()),
        attachments: Vec::new(),
        agent_run_json: Some(run.to_string()),
        ui_state_json: None,
    }
}

fn save_run_conversation(
    service: &StorageService,
    conversation_id: &str,
    assistant_message_id: &str,
    run_id: &str,
    run_status: &str,
    message_status: &str,
) {
    service
        .save_conversation(ChatConversationRecord {
            id: conversation_id.to_string(),
            project_id: None,
            model_id: Some("model-1".to_string()),
            title: "orphan trace".to_string(),
            messages: vec![
                ChatMessageRecord {
                    id: format!("user-{assistant_message_id}"),
                    role: "user".to_string(),
                    content: "do work".to_string(),
                    created_at: 1,
                    status: Some("sent".to_string()),
                    attachments: Vec::new(),
                    agent_run_json: None,
                    ui_state_json: None,
                },
                assistant_run_message(assistant_message_id, run_id, run_status, message_status, 2),
            ],
            created_at: 1,
            updated_at: 2,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
}

fn in_progress_trace(
    conversation_id: &str,
    assistant_message_id: &str,
    run_id: &str,
) -> ConversationTurnTrace {
    ConversationTurnTrace {
        schema_version: crate::CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
        run_id: run_id.to_string(),
        conversation_id: conversation_id.to_string(),
        assistant_message_id: assistant_message_id.to_string(),
        terminal_status: crate::ConversationTurnTraceTerminalStatus::InProgress,
        terminal_error: None,
        truncated: false,
        items: vec![
            ConversationTurnTraceItem::AssistantNarration {
                sequence: 0,
                content: "I will inspect the file.".to_string(),
                truncated: false,
            },
            ConversationTurnTraceItem::ToolCall {
                sequence: 1,
                call_id: format!("call-{run_id}"),
                tool: "read_file".to_string(),
                provenance: crate::AgentToolIdentity::Builtin {
                    tool_name: "read_file".to_string(),
                },
                operation: serde_json::json!({ "path": "notes.txt" }),
                approval_status: crate::AgentApprovalStatus::NotRequired,
                truncated: false,
            },
            ConversationTurnTraceItem::ToolResult {
                sequence: 2,
                call_id: format!("call-{run_id}"),
                tool: "read_file".to_string(),
                status: crate::ConversationTraceToolResultStatus::Succeeded,
                success: true,
                observation: serde_json::json!({ "path": "notes.txt", "lines": 3 }),
                approval_status: crate::AgentApprovalStatus::NotRequired,
                error: None,
                truncated: false,
                archive: Default::default(),
            },
        ],
    }
}

fn store_in_progress_trace(
    service: &StorageService,
    conversation_id: &str,
    assistant_message_id: &str,
    run_id: &str,
) -> ConversationTurnTrace {
    let trace = in_progress_trace(conversation_id, assistant_message_id, run_id);
    assert!(service
        .append_in_progress_conversation_turn_trace_and_apply_guidances(
            &trace,
            &model_context_for_closed_trace(&trace),
            10,
            20,
        )
        .unwrap());
    trace
}

fn model_context_for_closed_trace(
    trace: &ConversationTurnTrace,
) -> Vec<ConversationModelContextItem> {
    trace
        .items
        .iter()
        .filter_map(|item| match item {
            ConversationTurnTraceItem::AssistantNarration {
                sequence, content, ..
            } => Some(ConversationModelContextItem {
                sequence: *sequence,
                ordinal: 0,
                role: "assistant".to_string(),
                content: content.clone(),
                tool_call_id: None,
                tool_calls: Vec::new(),
                is_error: false,
            }),
            ConversationTurnTraceItem::ToolCall {
                sequence,
                call_id,
                tool,
                operation,
                ..
            } => Some(ConversationModelContextItem {
                sequence: *sequence,
                ordinal: 0,
                role: "assistant".to_string(),
                content: String::new(),
                tool_call_id: None,
                tool_calls: vec![crate::AgentContextCheckpointToolCall {
                    id: call_id.clone(),
                    name: tool.clone(),
                    args: operation.clone(),
                    provider_identity: crate::AgentProviderToolCallIdentity {
                        provider_tool_index: u32::try_from(*sequence).unwrap(),
                        provider_call_id: call_id.clone(),
                        runtime_call_id: call_id.clone(),
                    },
                }],
                is_error: false,
            }),
            ConversationTurnTraceItem::ToolResult {
                sequence,
                call_id,
                success,
                observation,
                ..
            } => Some(ConversationModelContextItem {
                sequence: *sequence,
                ordinal: 0,
                role: "tool".to_string(),
                content: serde_json::to_string(observation).unwrap(),
                tool_call_id: Some(call_id.clone()),
                tool_calls: Vec::new(),
                is_error: !success,
            }),
            ConversationTurnTraceItem::UserGuidance {
                sequence, content, ..
            } => Some(ConversationModelContextItem {
                sequence: *sequence,
                ordinal: 0,
                role: "user".to_string(),
                content: content.clone(),
                tool_call_id: None,
                tool_calls: Vec::new(),
                is_error: false,
            }),
            ConversationTurnTraceItem::AgentMailboxDelivery {
                sequence, content, ..
            } => Some(ConversationModelContextItem {
                sequence: *sequence,
                ordinal: 0,
                role: "user".to_string(),
                content: content.clone(),
                tool_call_id: None,
                tool_calls: Vec::new(),
                is_error: false,
            }),
            ConversationTurnTraceItem::CommandSessionLifecycle { .. }
            | ConversationTurnTraceItem::ContextCompactionLifecycle { .. }
            | ConversationTurnTraceItem::RuntimeError { .. } => None,
        })
        .collect()
}

#[test]
fn startup_trace_reconciliation_retires_cancelled_orphan_and_unblocks_fork() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let conversation_id = "conversation-cancelled-orphan";
    let assistant_message_id = "assistant-cancelled-orphan";
    let run_id = "run-cancelled-orphan";
    save_run_conversation(
        &service,
        conversation_id,
        assistant_message_id,
        run_id,
        "cancelled",
        "sent",
    );
    let original_trace =
        store_in_progress_trace(&service, conversation_id, assistant_message_id, run_id);
    let mut usage = agent_usage_record(conversation_id, assistant_message_id);
    usage.run_id = run_id.to_string();
    usage.status = Some("completed".to_string());
    usage.error = None;
    usage.completed_at = Some(25);
    service.upsert_agent_usage(usage).unwrap();

    let blocked = service.fork_conversation_request_view(assistant_reply_fork_request(
        "fork-before-reconciliation",
        conversation_id,
        assistant_message_id,
    ));
    assert!(blocked.unwrap_err().message().contains("这条回复仍在生成"));

    assert_eq!(
        service
            .reconcile_orphaned_in_progress_conversation_turn_traces(&HashSet::new(), 100)
            .unwrap(),
        1
    );
    let trace = service
        .get_conversation_turn_trace(assistant_message_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        trace.terminal_status,
        crate::ConversationTurnTraceTerminalStatus::Cancelled
    );
    assert_eq!(trace.items, original_trace.items);
    assert!(trace
        .terminal_error
        .as_deref()
        .is_some_and(|error| error.contains("cancelled before")));

    let stored = service.load_conversation(conversation_id).unwrap().unwrap();
    let message = &stored.messages[1];
    assert_eq!(message.status.as_deref(), Some("sent"));
    let run: serde_json::Value =
        serde_json::from_str(message.agent_run_json.as_deref().unwrap()).unwrap();
    assert_eq!(run["runId"], run_id);
    assert_eq!(run["status"], "cancelled");
    assert_eq!(run["state"]["status"], "cancelled");
    assert!(run["state"]["activeRunId"].is_null());
    let timeline = run["timeline"].as_array().unwrap();
    assert_eq!(timeline.len(), 3);
    assert_eq!(
        timeline[0],
        serde_json::json!({
            "id": "trace-message-0",
            "type": "message",
            "content": "I will inspect the file.",
            "traceSequence": 0
        })
    );
    assert_eq!(
        timeline[1],
        serde_json::json!({
            "id": "tool-call-call-run-cancelled-orphan",
            "type": "tool_call",
            "callId": "call-run-cancelled-orphan",
            "traceSequence": 1
        })
    );
    assert_eq!(timeline[2]["id"], "terminal-error");
    assert_eq!(timeline[2]["type"], "error");
    assert!(timeline[2].get("traceSequence").is_none());
    assert!(timeline[2]["message"]
        .as_str()
        .is_some_and(|message| message.contains("cancelled before")));

    let usage_state: (String, Option<String>, Option<i64>) = service
        .state
        .connection()
        .unwrap()
        .query_row(
            "SELECT status, error, completed_at FROM agent_usage_records WHERE run_id = ?1",
            [run_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(usage_state.0, "cancelled");
    assert!(usage_state.1.unwrap().contains("cancelled before"));
    assert_eq!(usage_state.2, Some(22));
    let lifecycle_times: (i64, Option<i64>) = service
        .state
        .connection()
        .unwrap()
        .query_row(
            "SELECT conversation.updated_at, trace.completed_at
             FROM conversations AS conversation
             INNER JOIN conversation_turn_traces AS trace
                 ON trace.conversation_id = conversation.id
             WHERE conversation.id = ?1",
            [conversation_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(lifecycle_times, (2, Some(22)));

    let forked = service
        .fork_conversation_request_view(assistant_reply_fork_request(
            "fork-after-reconciliation",
            conversation_id,
            assistant_message_id,
        ))
        .unwrap()
        .conversation;
    let cloned_assistant = &forked.messages[1];
    let cloned_trace = service
        .get_conversation_turn_trace(&cloned_assistant.id)
        .unwrap()
        .unwrap();
    assert_eq!(
        cloned_trace.terminal_status,
        crate::ConversationTurnTraceTerminalStatus::Cancelled
    );
    assert_eq!(cloned_trace.items, original_trace.items);

    assert_eq!(
        service
            .reconcile_orphaned_in_progress_conversation_turn_traces(&HashSet::new(), 200)
            .unwrap(),
        0,
        "startup reconciliation must be idempotent"
    );
}

#[test]
fn reload_rebuilds_compaction_and_runtime_error_in_the_durable_trace_order() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let conversation_id = "conversation-runtime-presentation-reload";
    let assistant_message_id = "assistant-runtime-presentation-reload";
    let run_id = "run-runtime-presentation-reload";
    let live_run = serde_json::json!({
        "runId": run_id,
        "status": "failed",
        "startedAt": 2,
        "completedAt": 8,
        "toolDefinitions": [],
        "toolCalls": [],
        "toolResults": [],
        "approvals": [],
        "diffs": [],
        "fileDrafts": [],
        "webSearchActivities": [],
        "readActivities": [],
        "mcpInvocations": [],
        "timeline": [
            {
                "id": "context-compaction-compact-1",
                "type": "context_compaction",
                "operationId": "compact-1",
                "status": "running",
                "traceSequence": 1
            },
            {
                "id": "error-2",
                "type": "error",
                "message": "iteration limit reached"
            }
        ],
        "messageStreamCheckpoints": {},
        "state": {
            "status": "failed",
            "activeRunId": null,
            "lastError": "iteration limit reached",
            "updatedAt": 8
        }
    });
    service
        .save_conversation(ChatConversationRecord {
            id: conversation_id.to_string(),
            project_id: None,
            model_id: Some("model-1".to_string()),
            title: "runtime presentation reload".to_string(),
            messages: vec![
                ChatMessageRecord {
                    id: "user-runtime-presentation-reload".to_string(),
                    role: "user".to_string(),
                    content: "continue".to_string(),
                    created_at: 1,
                    status: Some("sent".to_string()),
                    attachments: Vec::new(),
                    agent_run_json: None,
                    ui_state_json: None,
                },
                ChatMessageRecord {
                    id: assistant_message_id.to_string(),
                    role: "assistant".to_string(),
                    content: "iteration limit reached".to_string(),
                    created_at: 2,
                    status: Some("error".to_string()),
                    attachments: Vec::new(),
                    agent_run_json: Some(live_run.to_string()),
                    ui_state_json: None,
                },
            ],
            created_at: 1,
            updated_at: 8,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
    let trace = ConversationTurnTrace {
        schema_version: crate::CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
        run_id: run_id.to_string(),
        conversation_id: conversation_id.to_string(),
        assistant_message_id: assistant_message_id.to_string(),
        terminal_status: crate::ConversationTurnTraceTerminalStatus::Failed,
        terminal_error: Some("iteration limit reached".to_string()),
        truncated: false,
        items: vec![
            ConversationTurnTraceItem::AssistantNarration {
                sequence: 0,
                content: "Working on it.".to_string(),
                truncated: false,
            },
            ConversationTurnTraceItem::ContextCompactionLifecycle {
                sequence: 1,
                phase:
                    crate::conversation_trace::ConversationContextCompactionLifecyclePhase::Started,
                operation_id: "compact-1".to_string(),
                outcome: None,
            },
            ConversationTurnTraceItem::ContextCompactionLifecycle {
                sequence: 2,
                phase:
                    crate::conversation_trace::ConversationContextCompactionLifecyclePhase::Finished,
                operation_id: "compact-1".to_string(),
                outcome: Some(crate::protocol::AgentContextCompactionEventOutcome::Applied),
            },
            ConversationTurnTraceItem::RuntimeError {
                sequence: 3,
                message: "iteration limit reached".to_string(),
                recoverable: false,
                code: Some("tool_iteration_limit".to_string()),
                truncated: false,
            },
        ],
    };
    service
        .replace_conversation_turn_trace(&trace, 2, 8)
        .unwrap();
    drop(service);

    let reopened = fixture.service();
    let stored = reopened
        .load_conversation(conversation_id)
        .unwrap()
        .unwrap();
    let run: serde_json::Value =
        serde_json::from_str(stored.messages[1].agent_run_json.as_deref().unwrap()).unwrap();
    assert_eq!(
        run["timeline"],
        serde_json::json!([
            {
                "id": "trace-message-0",
                "type": "message",
                "content": "Working on it.",
                "traceSequence": 0
            },
            {
                "id": "context-compaction-compact-1",
                "type": "context_compaction",
                "operationId": "compact-1",
                "status": "applied",
                "traceSequence": 1
            },
            {
                "id": "trace-error-3",
                "type": "error",
                "message": "iteration limit reached",
                "traceSequence": 3
            }
        ])
    );
}

#[test]
fn reload_replaces_live_timeline_projections_with_one_durable_ordered_trace() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let conversation_id = "conversation-live-trace-reload";
    let assistant_message_id = "assistant-live-trace-reload";
    let run_id = "run-live-trace-reload";
    let call_id = "call-live-trace-reload";
    let client_message_id = "client-guidance-live-trace-reload";
    let final_answer = "The presentation is ready.";
    let live_run = serde_json::json!({
        "runId": run_id,
        "status": "completed",
        "startedAt": 2,
        "completedAt": 9,
        "toolDefinitions": [],
        "toolCalls": [{
            "id": call_id,
            "tool": "read_file",
            "args": { "path": "brief.txt" },
            "approvalStatus": "not_required",
            "reason": null
        }],
        "toolResults": [{
            "callId": call_id,
            "tool": "read_file",
            "ok": true,
            "result": { "path": "brief.txt" }
        }],
        "approvals": [],
        "diffs": [],
        "fileDrafts": [],
        "webSearchActivities": [],
        "readActivities": [],
        "mcpInvocations": [],
        "timeline": [
            {
                "id": "message-stream-opening",
                "type": "message",
                "content": "I will inspect the brief.",
                "traceSequence": 0
            },
            {
                "id": "live-guidance-insertion",
                "type": "user_guidance",
                "guidanceId": "guidance-live-trace-reload",
                "clientMessageId": client_message_id,
                "content": "Keep the system watermark.",
                "attachments": [],
                "status": "applied",
                "createdAt": 4,
                "sequence": 1
            },
            {
                "id": "live-tool-call",
                "type": "tool_call",
                "callId": call_id,
                "traceSequence": 2
            },
            {
                "id": "message-stream-after-guidance",
                "type": "message",
                "content": "I kept the watermark and finished the layout.",
                "traceSequence": 4
            },
            {
                "id": "message-final-answer",
                "type": "message",
                "content": final_answer
            }
        ],
        "messageStreamCheckpoints": {},
        "state": {
            "status": "completed",
            "activeRunId": null,
            "updatedAt": 9
        }
    });
    service
        .save_conversation(ChatConversationRecord {
            id: conversation_id.to_string(),
            project_id: None,
            model_id: Some("model-1".to_string()),
            title: "live trace reload".to_string(),
            messages: vec![
                ChatMessageRecord {
                    id: "user-live-trace-reload".to_string(),
                    role: "user".to_string(),
                    content: "Build a presentation.".to_string(),
                    created_at: 1,
                    status: Some("sent".to_string()),
                    attachments: Vec::new(),
                    agent_run_json: None,
                    ui_state_json: None,
                },
                ChatMessageRecord {
                    id: assistant_message_id.to_string(),
                    role: "assistant".to_string(),
                    content: final_answer.to_string(),
                    created_at: 2,
                    status: Some("sent".to_string()),
                    attachments: Vec::new(),
                    agent_run_json: Some(live_run.to_string()),
                    ui_state_json: None,
                },
            ],
            created_at: 1,
            updated_at: 9,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
    service
        .replace_conversation_turn_trace(
            &ConversationTurnTrace {
                schema_version: crate::CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
                run_id: run_id.to_string(),
                conversation_id: conversation_id.to_string(),
                assistant_message_id: assistant_message_id.to_string(),
                terminal_status: crate::ConversationTurnTraceTerminalStatus::Completed,
                terminal_error: None,
                truncated: false,
                items: vec![
                    ConversationTurnTraceItem::AssistantNarration {
                        sequence: 0,
                        content: "I will inspect the brief.".to_string(),
                        truncated: false,
                    },
                    ConversationTurnTraceItem::UserGuidance {
                        sequence: 1,
                        guidance_id: "guidance-live-trace-reload".to_string(),
                        client_message_id: client_message_id.to_string(),
                        content: "Keep the system watermark.".to_string(),
                        attachments: Vec::new(),
                        created_at: 4,
                        truncated: false,
                    },
                    ConversationTurnTraceItem::ToolCall {
                        sequence: 2,
                        call_id: call_id.to_string(),
                        tool: "read_file".to_string(),
                        provenance: crate::AgentToolIdentity::Builtin {
                            tool_name: "read_file".to_string(),
                        },
                        operation: serde_json::json!({ "path": "brief.txt" }),
                        approval_status: crate::AgentApprovalStatus::NotRequired,
                        truncated: false,
                    },
                    ConversationTurnTraceItem::ToolResult {
                        sequence: 3,
                        call_id: call_id.to_string(),
                        tool: "read_file".to_string(),
                        status: crate::ConversationTraceToolResultStatus::Succeeded,
                        success: true,
                        observation: serde_json::json!({ "path": "brief.txt" }),
                        approval_status: crate::AgentApprovalStatus::NotRequired,
                        error: None,
                        truncated: false,
                        archive: Default::default(),
                    },
                    ConversationTurnTraceItem::AssistantNarration {
                        sequence: 4,
                        content: "I kept the watermark and finished the layout.".to_string(),
                        truncated: false,
                    },
                ],
            },
            2,
            9,
        )
        .unwrap();

    let first_load = service.load_conversation(conversation_id).unwrap().unwrap();
    let assistant = &first_load.messages[1];
    let first_run: serde_json::Value =
        serde_json::from_str(assistant.agent_run_json.as_deref().unwrap()).unwrap();
    let expected_timeline = serde_json::json!([
        {
            "id": "trace-message-0",
            "type": "message",
            "content": "I will inspect the brief.",
            "traceSequence": 0
        },
        {
            "id": "user-guidance-client-guidance-live-trace-reload",
            "type": "user_guidance",
            "guidanceId": "guidance-live-trace-reload",
            "clientMessageId": "client-guidance-live-trace-reload",
            "content": "Keep the system watermark.",
            "attachments": [],
            "status": "applied",
            "createdAt": 4,
            "sequence": 1,
            "traceSequence": 1
        },
        {
            "id": "tool-call-call-live-trace-reload",
            "type": "tool_call",
            "callId": "call-live-trace-reload",
            "traceSequence": 2
        },
        {
            "id": "trace-message-4",
            "type": "message",
            "content": "I kept the watermark and finished the layout.",
            "traceSequence": 4
        },
        {
            "id": "message-final-answer",
            "type": "message",
            "content": final_answer
        }
    ]);
    assert_eq!(first_run["timeline"], expected_timeline);

    // Renderer state can be written back after a read. A second reload must remain byte-for-byte
    // equivalent instead of treating the first projection as new live history and duplicating it.
    service
        .save_chat_message_state(
            conversation_id,
            crate::storage::models::ChatMessageStateRecord {
                id: assistant_message_id.to_string(),
                content: assistant.content.clone(),
                status: assistant.status.clone(),
                agent_run_json: assistant.agent_run_json.clone(),
                ui_state_json: assistant.ui_state_json.clone(),
            },
        )
        .unwrap();
    drop(service);

    let reopened = fixture.service();
    let second_load = reopened
        .load_conversation(conversation_id)
        .unwrap()
        .unwrap();
    let second_run: serde_json::Value =
        serde_json::from_str(second_load.messages[1].agent_run_json.as_deref().unwrap()).unwrap();
    assert_eq!(second_run["timeline"], expected_timeline);
}

#[test]
fn reload_keeps_a_host_terminal_error_unanchored_without_inventing_a_trace_sequence() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let conversation_id = "conversation-host-error-reload";
    let assistant_message_id = "assistant-host-error-reload";
    let run_id = "run-host-error-reload";
    service
        .save_conversation(ChatConversationRecord {
            id: conversation_id.to_string(),
            project_id: None,
            model_id: Some("model-1".to_string()),
            title: "host error reload".to_string(),
            messages: vec![
                ChatMessageRecord {
                    id: "user-host-error-reload".to_string(),
                    role: "user".to_string(),
                    content: "continue".to_string(),
                    created_at: 1,
                    status: Some("sent".to_string()),
                    attachments: Vec::new(),
                    agent_run_json: None,
                    ui_state_json: None,
                },
                ChatMessageRecord {
                    id: assistant_message_id.to_string(),
                    role: "assistant".to_string(),
                    content: "host persistence failed".to_string(),
                    created_at: 2,
                    status: Some("error".to_string()),
                    attachments: Vec::new(),
                    agent_run_json: Some(
                        serde_json::json!({
                            "runId": run_id,
                            "status": "failed",
                            "startedAt": 2,
                            "completedAt": 8,
                            "toolDefinitions": [],
                            "toolCalls": [],
                            "toolResults": [],
                            "approvals": [],
                            "diffs": [],
                            "fileDrafts": [],
                            "webSearchActivities": [],
                            "readActivities": [],
                            "mcpInvocations": [],
                            "timeline": [
                                {
                                    "id": "trace-message-0",
                                    "type": "message",
                                    "content": "Working.",
                                    "traceSequence": 0
                                },
                                {
                                    "id": "error-2",
                                    "type": "error",
                                    "message": "host persistence failed"
                                }
                            ],
                            "messageStreamCheckpoints": {},
                            "state": {
                                "status": "failed",
                                "activeRunId": null,
                                "lastError": "host persistence failed",
                                "updatedAt": 8
                            }
                        })
                        .to_string(),
                    ),
                    ui_state_json: None,
                },
            ],
            created_at: 1,
            updated_at: 8,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
    service
        .replace_conversation_turn_trace(
            &ConversationTurnTrace {
                schema_version: crate::CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
                run_id: run_id.to_string(),
                conversation_id: conversation_id.to_string(),
                assistant_message_id: assistant_message_id.to_string(),
                terminal_status: crate::ConversationTurnTraceTerminalStatus::Failed,
                terminal_error: Some("host persistence failed".to_string()),
                truncated: false,
                items: vec![ConversationTurnTraceItem::AssistantNarration {
                    sequence: 0,
                    content: "Working.".to_string(),
                    truncated: false,
                }],
            },
            2,
            8,
        )
        .unwrap();
    drop(service);

    let reopened = fixture.service();
    let stored = reopened
        .load_conversation(conversation_id)
        .unwrap()
        .unwrap();
    let run: serde_json::Value =
        serde_json::from_str(stored.messages[1].agent_run_json.as_deref().unwrap()).unwrap();
    let timeline = run["timeline"].as_array().unwrap();
    assert_eq!(timeline.len(), 2);
    assert_eq!(timeline[0]["traceSequence"], 0);
    assert_eq!(timeline[1]["id"], "terminal-error");
    assert_eq!(timeline[1]["message"], "host persistence failed");
    assert!(timeline[1].get("traceSequence").is_none());
}

#[test]
fn startup_reconciliation_settles_a_crashed_context_compaction_before_reload() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let conversation_id = "conversation-crashed-compaction";
    let assistant_message_id = "assistant-crashed-compaction";
    let run_id = "run-crashed-compaction";
    save_run_conversation(
        &service,
        conversation_id,
        assistant_message_id,
        run_id,
        "running",
        "pending",
    );
    let trace = ConversationTurnTrace {
        schema_version: crate::CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
        run_id: run_id.to_string(),
        conversation_id: conversation_id.to_string(),
        assistant_message_id: assistant_message_id.to_string(),
        terminal_status: crate::ConversationTurnTraceTerminalStatus::InProgress,
        terminal_error: None,
        truncated: false,
        items: vec![ConversationTurnTraceItem::ContextCompactionLifecycle {
            sequence: 0,
            phase: crate::conversation_trace::ConversationContextCompactionLifecyclePhase::Started,
            operation_id: "compact-crashed".to_string(),
            outcome: None,
        }],
    };
    assert!(service
        .append_in_progress_conversation_turn_trace_and_apply_guidances(&trace, &[], 10, 20)
        .unwrap());

    assert_eq!(
        service
            .reconcile_orphaned_in_progress_conversation_turn_traces(&HashSet::new(), 100)
            .unwrap(),
        1
    );
    drop(service);

    let reopened = fixture.service();
    let recovered = reopened
        .get_conversation_turn_trace(assistant_message_id)
        .unwrap()
        .unwrap();
    recovered.validate().unwrap();
    assert_eq!(
        recovered.terminal_status,
        crate::ConversationTurnTraceTerminalStatus::Failed
    );
    assert!(matches!(
        recovered.items.as_slice(),
        [
            ConversationTurnTraceItem::ContextCompactionLifecycle {
                sequence: 0,
                phase: crate::conversation_trace::ConversationContextCompactionLifecyclePhase::Started,
                operation_id,
                outcome: None,
            },
            ConversationTurnTraceItem::ContextCompactionLifecycle {
                sequence: 1,
                phase: crate::conversation_trace::ConversationContextCompactionLifecyclePhase::Finished,
                operation_id: finished_operation_id,
                outcome: Some(crate::protocol::AgentContextCompactionEventOutcome::Failed),
            }
        ] if operation_id == "compact-crashed" && finished_operation_id == operation_id
    ));
    let stored = reopened
        .load_conversation(conversation_id)
        .unwrap()
        .unwrap();
    let run: serde_json::Value =
        serde_json::from_str(stored.messages[1].agent_run_json.as_deref().unwrap()).unwrap();
    assert_eq!(
        run["timeline"],
        serde_json::json!([{
            "id": "context-compaction-compact-crashed",
            "type": "context_compaction",
            "operationId": "compact-crashed",
            "status": "failed",
            "traceSequence": 0
        }, {
            "id": "terminal-error",
            "type": "error",
            "message": "The application exited before the agent run's conversation trace was finalized."
        }])
    );
}

#[test]
fn startup_trace_reconciliation_closes_a_durable_unresolved_tool_call() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let conversation_id = "conversation-open-call";
    let assistant_message_id = "assistant-open-call";
    let run_id = "run-open-call";
    let provider_call_id = "provider-image-call";
    let call_id = crate::llm::model_response_tool_call_id(run_id, 0, 0, provider_call_id);
    save_run_conversation(
        &service,
        conversation_id,
        assistant_message_id,
        run_id,
        "running",
        "pending",
    );
    let trace = ConversationTurnTrace {
        schema_version: crate::CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
        run_id: run_id.to_string(),
        conversation_id: conversation_id.to_string(),
        assistant_message_id: assistant_message_id.to_string(),
        terminal_status: crate::ConversationTurnTraceTerminalStatus::InProgress,
        terminal_error: None,
        truncated: false,
        items: vec![ConversationTurnTraceItem::ToolCall {
            sequence: 0,
            call_id: call_id.clone(),
            tool: "image_generation".to_string(),
            provenance: crate::AgentToolIdentity::Builtin {
                tool_name: "image_generation".to_string(),
            },
            operation: serde_json::json!({
                "request": { "operation": "generate", "prompt": "private prompt" },
                "reason": "Create the requested image."
            }),
            approval_status: crate::AgentApprovalStatus::NotRequired,
            truncated: false,
        }],
    };
    let model_context = vec![ConversationModelContextItem {
        sequence: 0,
        ordinal: 0,
        role: "assistant".to_string(),
        content: String::new(),
        tool_call_id: None,
        tool_calls: vec![crate::AgentContextCheckpointToolCall {
            id: call_id.clone(),
            name: "image_generation".to_string(),
            args: serde_json::json!({
                "request": { "operation": "generate", "prompt": "private prompt" },
                "reason": "Create the requested image."
            }),
            provider_identity: crate::AgentProviderToolCallIdentity {
                provider_tool_index: 0,
                provider_call_id: provider_call_id.to_string(),
                runtime_call_id: call_id.clone(),
            },
        }],
        is_error: false,
    }];
    assert!(service
        .append_in_progress_conversation_turn_trace_and_apply_guidances(
            &trace,
            &model_context,
            10,
            20,
        )
        .unwrap());

    assert_eq!(
        service
            .reconcile_orphaned_in_progress_conversation_turn_traces(&HashSet::new(), 100)
            .unwrap(),
        1
    );
    let repaired = service
        .get_conversation_turn_trace(assistant_message_id)
        .unwrap()
        .unwrap();
    repaired.validate().unwrap();
    assert_eq!(
        repaired.terminal_status,
        crate::ConversationTurnTraceTerminalStatus::Failed
    );
    assert!(matches!(
        repaired.items.last(),
        Some(ConversationTurnTraceItem::ToolResult {
            call_id,
            tool,
            success: false,
            ..
        }) if call_id == &crate::llm::model_response_tool_call_id(
            run_id,
            0,
            0,
            provider_call_id,
        ) && tool == "image_generation"
    ));
    let model_context = service
        .get_conversation_model_context_log(assistant_message_id)
        .unwrap()
        .unwrap();
    repaired
        .validate_complete_model_context(&model_context.items)
        .unwrap();
}

#[test]
fn completed_looking_renderer_state_is_conservatively_reconciled_as_interrupted_failure() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let conversation_id = "conversation-completed-looking";
    let assistant_message_id = "assistant-completed-looking";
    let run_id = "run-completed-looking";
    save_run_conversation(
        &service,
        conversation_id,
        assistant_message_id,
        run_id,
        "completed",
        "sent",
    );
    store_in_progress_trace(&service, conversation_id, assistant_message_id, run_id);

    assert_eq!(
        service
            .reconcile_orphaned_in_progress_conversation_turn_traces(&HashSet::new(), 100)
            .unwrap(),
        1
    );
    let trace = service
        .get_conversation_turn_trace(assistant_message_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        trace.terminal_status,
        crate::ConversationTurnTraceTerminalStatus::Failed
    );
    assert!(trace
        .terminal_error
        .as_deref()
        .is_some_and(|error| error.contains("application exited")));
    let stored = service.load_conversation(conversation_id).unwrap().unwrap();
    let message = &stored.messages[1];
    assert_eq!(message.status.as_deref(), Some("error"));
    let run: serde_json::Value =
        serde_json::from_str(message.agent_run_json.as_deref().unwrap()).unwrap();
    assert_eq!(run["status"], "failed");
    assert_eq!(run["state"]["status"], "failed");
}

#[test]
fn startup_trace_reconciliation_preserves_pending_approval_and_current_active_run() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    for (conversation_id, assistant_message_id, run_id) in [
        ("conversation-pending", "assistant-pending", "run-pending"),
        ("conversation-active", "assistant-active", "run-active"),
    ] {
        save_run_conversation(
            &service,
            conversation_id,
            assistant_message_id,
            run_id,
            "running",
            "pending",
        );
        store_in_progress_trace(&service, conversation_id, assistant_message_id, run_id);
    }
    service
        .store_pending_agent_action(AgentPendingActionRecord {
            action_id: "action-pending-trace".to_string(),
            run_id: "run-pending".to_string(),
            conversation_id: Some("conversation-pending".to_string()),
            assistant_message_id: Some("assistant-pending".to_string()),
            action_type: "tool_call".to_string(),
            tool_name: "approval_tool".to_string(),
            tool_call_id: Some("pending-call".to_string()),
            status: "pending".to_string(),
            target_status: None,
            action_json: "{}".to_string(),
            agent_input_json: "{}".to_string(),
            created_at: 5,
            updated_at: 5,
        })
        .unwrap();
    let active = HashSet::from(["run-active".to_string()]);

    assert_eq!(
        service
            .reconcile_orphaned_in_progress_conversation_turn_traces(&active, 100)
            .unwrap(),
        0
    );
    for assistant_message_id in ["assistant-pending", "assistant-active"] {
        assert_eq!(
            service
                .get_conversation_turn_trace(assistant_message_id)
                .unwrap()
                .unwrap()
                .terminal_status,
            crate::ConversationTurnTraceTerminalStatus::InProgress
        );
    }
}

#[test]
fn startup_trace_reconciliation_rolls_back_every_candidate_on_commit_failure() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    for suffix in ["a", "b"] {
        let conversation_id = format!("conversation-atomic-{suffix}");
        let assistant_message_id = format!("assistant-atomic-{suffix}");
        let run_id = format!("run-atomic-{suffix}");
        save_run_conversation(
            &service,
            &conversation_id,
            &assistant_message_id,
            &run_id,
            "running",
            "pending",
        );
        store_in_progress_trace(&service, &conversation_id, &assistant_message_id, &run_id);
    }
    service
        .state
        .connection()
        .unwrap()
        .execute_batch(
            "CREATE TRIGGER reject_second_orphan_reconciliation
             BEFORE UPDATE OF status ON messages
             WHEN NEW.id = 'assistant-atomic-b'
             BEGIN
                 SELECT RAISE(ABORT, 'injected reconciliation failure');
             END;",
        )
        .unwrap();

    let error = service
        .reconcile_orphaned_in_progress_conversation_turn_traces(&HashSet::new(), 100)
        .unwrap_err();
    assert!(error.contains("injected reconciliation failure"));
    for assistant_message_id in ["assistant-atomic-a", "assistant-atomic-b"] {
        assert_eq!(
            service
                .get_conversation_turn_trace(assistant_message_id)
                .unwrap()
                .unwrap()
                .terminal_status,
            crate::ConversationTurnTraceTerminalStatus::InProgress
        );
    }
}
