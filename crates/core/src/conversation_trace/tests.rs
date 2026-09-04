use super::*;
use crate::protocol::{AgentMcpServerScope, AgentMcpToolProvenance};

fn call(id: &str) -> AgentToolCall {
    AgentToolCall {
        id: id.to_string(),
        tool: "web_fetch".to_string(),
        args: json!({ "url": "https://example.com" }),
        approval_status: AgentApprovalStatus::NotRequired,
        reason: None,
    }
}

fn record_open_call(recorder: &mut ConversationTraceRecorder, call: &AgentToolCall) {
    let sequence = recorder.record_tool_call(call).unwrap();
    recorder
        .record_model_message(
            sequence,
            0,
            &LlmMessage::assistant(
                "",
                vec![crate::llm::LlmToolCall {
                    id: call.id.clone(),
                    name: call.tool.clone(),
                    args: call.args.clone(),
                }],
            ),
        )
        .unwrap();
}

fn command_lifecycle(
    phase: ConversationCommandSessionLifecyclePhase,
    status: AgentCommandSessionStatus,
    created_at: i64,
) -> ConversationCommandSessionLifecycle {
    ConversationCommandSessionLifecycle {
        phase,
        session_id: "cmd_0123456789abcdef0123456789abcdef".to_string(),
        call_id: "command-call".to_string(),
        status,
        exit_code: (status == AgentCommandSessionStatus::Exited).then_some(0),
        latest_sequence: 7,
        output_truncated: false,
        archive: Default::default(),
        created_at,
    }
}

fn command_trace() -> ConversationTurnTrace {
    ConversationTurnTrace {
        schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
        run_id: "run-command".to_string(),
        conversation_id: "conversation-command".to_string(),
        assistant_message_id: "assistant-command".to_string(),
        terminal_status: ConversationTurnTraceTerminalStatus::Completed,
        terminal_error: None,
        truncated: false,
        items: vec![
            ConversationTurnTraceItem::ToolCall {
                sequence: 0,
                call_id: "command-call".to_string(),
                tool: "run_command".to_string(),
                provenance: AgentToolIdentity::Builtin {
                    tool_name: "run_command".to_string(),
                },
                operation: json!({ "command": "long-running" }),
                approval_status: AgentApprovalStatus::Approved,
                truncated: false,
            },
            ConversationTurnTraceItem::CommandSessionLifecycle {
                sequence: 1,
                phase: ConversationCommandSessionLifecyclePhase::Started,
                session_id: "cmd_0123456789abcdef0123456789abcdef".to_string(),
                call_id: "command-call".to_string(),
                status: AgentCommandSessionStatus::Running,
                exit_code: None,
                latest_sequence: 0,
                output_truncated: false,
                archive: Default::default(),
                created_at: 1_000,
            },
            ConversationTurnTraceItem::ToolResult {
                sequence: 2,
                call_id: "command-call".to_string(),
                tool: "run_command".to_string(),
                status: ConversationTraceToolResultStatus::Succeeded,
                success: true,
                observation: json!({
                    "status": "running",
                    "sessionId": "cmd_0123456789abcdef0123456789abcdef"
                }),
                approval_status: AgentApprovalStatus::Approved,
                error: None,
                truncated: false,
                archive: Default::default(),
            },
        ],
    }
}

fn wait_call() -> AgentToolCall {
    AgentToolCall {
        id: "wait-call".to_string(),
        tool: "wait_agent".to_string(),
        args: json!({ "targets": ["agent-child"], "timeout_ms": 30_000 }),
        approval_status: AgentApprovalStatus::NotRequired,
        reason: None,
    }
}

fn wait_result(payload: &str) -> AgentToolResult {
    AgentToolResult {
        exact_archive_file: None,
        call_id: "wait-call".to_string(),
        tool: "wait_agent".to_string(),
        ok: true,
        result: Some(json!({
            "receiptId": "receipt-wait",
            "sourceReceiptId": null,
            "targets": [{
                "targetAgentId": "agent-child",
                "messages": [{
                    "messageId": "message-child-result",
                    "senderAgentId": "agent-child",
                    "senderTaskName": "research",
                    "senderTaskPath": "/root/research",
                    "kind": "result",
                    "content": payload,
                    "mailboxSequence": 1,
                    "createdAt": 1
                }],
                "targetStatusVersion": 1,
                "latestWakeSequence": 1,
                "latestWakeStatusRevision": 1,
                "latestWakeStatus": "completed",
                "displayStatus": "idle"
            }]
        })),
        error: None,
    }
}

fn assert_precommitted_wait_matches_runtime_terminal(payload: &str, truncated: bool) {
    let call = wait_call();
    let result = wait_result(payload);

    // The live Runtime retains the unbounded checkpoint item until terminal projection.
    let mut runtime = ConversationTraceRecorder::default();
    runtime.record_tool_call(&call).unwrap();
    runtime.record_tool_result(&call, &result);
    let terminal = runtime.finish(
        "run-wait",
        "conversation-wait",
        "assistant-wait",
        ConversationTurnTraceTerminalStatus::Completed,
        None,
    );

    // wait_agent commits the same ToolResult from the durable unresolved call before it
    // returns control to Runtime. This is the prefix which terminal settlement must preserve.
    let mut unresolved = ConversationTraceRecorder::default();
    unresolved.record_tool_call(&call).unwrap();
    let durable_unresolved = unresolved.snapshot().in_progress_audit_trace(
        "run-wait",
        "conversation-wait",
        "assistant-wait",
    );
    let precommitted =
        conversation_trace_with_recovered_tool_result(&durable_unresolved, &result).unwrap();

    assert_eq!(precommitted.items, terminal.items);
    assert_eq!(precommitted.truncated, terminal.truncated);
    assert_eq!(precommitted.truncated, truncated);
    assert!(matches!(
        precommitted.items.last(),
        Some(ConversationTurnTraceItem::ToolResult {
            archive: ConversationHistoryArchiveTraceMetadata {
                history_projection_truncated,
                ..
            },
            ..
        }) if *history_projection_truncated == truncated
    ));
}

include!("tests/model_and_validation.rs");
include!("tests/recording_and_projection.rs");
