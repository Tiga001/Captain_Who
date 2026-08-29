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

#[test]
fn precommitted_wait_uses_the_same_archive_metadata_as_runtime_terminal_projection() {
    assert_precommitted_wait_matches_runtime_terminal("small child result", false);
    assert_precommitted_wait_matches_runtime_terminal(&"x".repeat(16_000), true);
    assert_precommitted_wait_matches_runtime_terminal(
        "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAAB",
        true,
    );
}

#[test]
fn command_session_lifecycle_is_a_body_free_audit_item() {
    let mut trace = command_trace();
    trace
        .append_command_session_lifecycle(command_lifecycle(
            ConversationCommandSessionLifecyclePhase::Terminal,
            AgentCommandSessionStatus::Exited,
            2_000,
        ))
        .unwrap();
    trace.validate().unwrap();

    let serialized = serde_json::to_value(trace.items.last().unwrap()).unwrap();
    assert_eq!(serialized["type"], "command_session_lifecycle");
    assert_eq!(serialized["phase"], "terminal");
    assert_eq!(serialized["status"], "exited");
    assert!(serialized.get("output").is_none());
    assert!(serialized.get("observation").is_none());
}

#[test]
fn appending_command_session_lifecycle_is_idempotent_and_conflict_safe() {
    let mut trace = command_trace();
    let terminal = command_lifecycle(
        ConversationCommandSessionLifecyclePhase::Terminal,
        AgentCommandSessionStatus::Exited,
        2_000,
    );
    let sequence = trace
        .append_command_session_lifecycle(terminal.clone())
        .unwrap();
    assert_eq!(
        trace.append_command_session_lifecycle(terminal).unwrap(),
        sequence
    );
    assert_eq!(trace.items.len(), 4);

    let conflict = command_lifecycle(
        ConversationCommandSessionLifecyclePhase::Terminal,
        AgentCommandSessionStatus::TimedOut,
        3_000,
    );
    assert!(trace.append_command_session_lifecycle(conflict).is_err());
    assert_eq!(trace.items.len(), 4);
}

#[test]
fn context_compaction_and_runtime_error_are_append_only_non_model_trace_markers() {
    let mut recorder = ConversationTraceRecorder::default();
    assert_eq!(
        recorder.record_narration("Before compaction.").unwrap(),
        Some(0)
    );
    assert_eq!(
        recorder
            .record_context_compaction_started("compact-1")
            .unwrap(),
        1
    );
    assert_eq!(
        recorder
            .record_context_compaction_started("compact-1")
            .unwrap(),
        1
    );
    assert_eq!(
        recorder
            .record_context_compaction_finished(
                "compact-1",
                AgentContextCompactionEventOutcome::Applied,
            )
            .unwrap(),
        1
    );
    assert_eq!(
        recorder
            .record_context_compaction_finished(
                "compact-1",
                AgentContextCompactionEventOutcome::Applied,
            )
            .unwrap(),
        1
    );
    assert_eq!(
        recorder
            .record_runtime_error("iteration limit reached", false, Some("iteration_limit"))
            .unwrap(),
        3
    );

    let trace = recorder.finish(
        "run-markers",
        "conversation-markers",
        "assistant-markers",
        ConversationTurnTraceTerminalStatus::Failed,
        Some("iteration limit reached"),
    );
    trace.validate().unwrap();
    assert_eq!(
        trace
            .items
            .iter()
            .map(ConversationTurnTraceItem::sequence)
            .collect::<Vec<_>>(),
        vec![0, 1, 2, 3]
    );
    assert_eq!(trace.model_context_item_count(), 1);
    assert!(trace.items[1..].iter().all(|item| !item.is_model_visible()));
}

#[test]
fn terminal_projection_closes_a_crashed_context_compaction_once() {
    let mut recorder = ConversationTraceRecorder::default();
    recorder
        .record_context_compaction_started("compact-crashed")
        .unwrap();
    let snapshot = recorder.snapshot();
    let projection = terminal_conversation_trace_from_snapshot(
        snapshot,
        "run-crashed-compaction",
        "conversation-crashed-compaction",
        "assistant-crashed-compaction",
        ConversationTurnTraceTerminalStatus::Failed,
        "application exited",
    )
    .unwrap();
    projection.trace.validate().unwrap();
    assert!(matches!(
        projection.trace.items.as_slice(),
        [
            ConversationTurnTraceItem::ContextCompactionLifecycle {
                sequence: 0,
                phase: ConversationContextCompactionLifecyclePhase::Started,
                operation_id,
                outcome: None,
            },
            ConversationTurnTraceItem::ContextCompactionLifecycle {
                sequence: 1,
                phase: ConversationContextCompactionLifecyclePhase::Finished,
                operation_id: finished_operation_id,
                outcome: Some(AgentContextCompactionEventOutcome::Failed),
            }
        ] if operation_id == "compact-crashed" && finished_operation_id == operation_id
    ));

    let mut dangling = projection.trace.clone();
    dangling.items.pop();
    assert!(dangling.validate().is_err());
}

#[test]
fn mcp_tool_identity_round_trips_and_missing_or_extra_provenance_is_rejected() {
    let tool_name = "mcp__fixture__echo";
    let call = AgentToolCall {
        id: "mcp-call-1".to_string(),
        tool: tool_name.to_string(),
        args: json!({"text": "hello"}),
        approval_status: AgentApprovalStatus::NotRequired,
        reason: None,
    };
    let identity = AgentToolIdentity::Mcp {
        provenance: AgentMcpToolProvenance {
            server_id: "7f4a2d91-24ab-4d24-9eed-63daf26a6c15".to_string(),
            scope: AgentMcpServerScope::Project {
                project_id: "project-fixture".to_string(),
            },
            raw_tool_name: "echo/raw".to_string(),
            model_tool_name: tool_name.to_string(),
            config_epoch: "66dcbb6b-92a3-4d4e-9591-f0707e4ca3e3".to_string(),
            registry_revision: 3,
            config_digest: "a".repeat(64),
            catalog_generation: 4,
            catalog_digest: "b".repeat(64),
            catalog_schema_digest: "d".repeat(64),
            schema_digest: "c".repeat(64),
            schema_normalizer_version: crate::MCP_INPUT_SCHEMA_NORMALIZER_VERSION,
        },
    };
    let mut recorder = ConversationTraceRecorder::default();
    recorder.record_tool_call_with_identity(&call, identity);
    recorder.record_tool_result(
        &call,
        &AgentToolResult {
            exact_archive_file: None,
            call_id: call.id.clone(),
            tool: call.tool.clone(),
            ok: true,
            result: Some(json!({"content": "done"})),
            error: None,
        },
    );
    let trace = recorder.finish(
        "run-mcp-identity",
        "conversation-mcp-identity",
        "assistant-mcp-identity",
        ConversationTurnTraceTerminalStatus::Completed,
        None,
    );
    trace.validate().unwrap();
    let serialized = serde_json::to_value(&trace).unwrap();
    assert_eq!(
        serialized["items"][0]["provenance"]["provenance"]["catalogDigest"],
        "b".repeat(64)
    );
    assert_eq!(
        serialized["items"][0]["provenance"]["provenance"]["scope"]["projectId"],
        "project-fixture"
    );

    let mut legacy = serialized["items"][0].clone();
    legacy
        .as_object_mut()
        .expect("serialized trace item object")
        .remove("provenance");
    assert!(serde_json::from_value::<ConversationTurnTraceItem>(legacy).is_err());

    let mut extra = serialized["items"][0].clone();
    extra
        .as_object_mut()
        .expect("serialized trace item object")
        .insert("unexpected".to_string(), Value::Bool(true));
    assert!(serde_json::from_value::<ConversationTurnTraceItem>(extra).is_err());

    let mut extra_mcp_provenance = serialized["items"][0].clone();
    extra_mcp_provenance["provenance"]["provenance"]
        .as_object_mut()
        .unwrap()
        .insert("approvalPolicy".to_string(), json!("forged"));
    assert!(serde_json::from_value::<ConversationTurnTraceItem>(extra_mcp_provenance).is_err());

    let mut extra_mcp_scope = serialized["items"][0].clone();
    extra_mcp_scope["provenance"]["provenance"]["scope"]
        .as_object_mut()
        .unwrap()
        .insert("workspaceId".to_string(), json!("forged"));
    assert!(serde_json::from_value::<ConversationTurnTraceItem>(extra_mcp_scope).is_err());

    let mut mismatched = trace.clone();
    if let ConversationTurnTraceItem::ToolCall { tool, .. } = &mut mismatched.items[0] {
        *tool = "mcp__different__echo".to_string();
    }
    assert!(mismatched.validate().is_err());
}

#[test]
fn unregistered_identity_is_a_strict_rejection_only_audit_record() {
    let call = AgentToolCall {
        id: "unknown-call".to_string(),
        tool: "hallucinated_tool".to_string(),
        args: json!({}),
        approval_status: AgentApprovalStatus::NotRequired,
        reason: None,
    };
    let mut recorder = ConversationTraceRecorder::default();
    recorder.record_tool_call_with_identity(
        &call,
        AgentToolIdentity::Unregistered {
            tool_name: call.tool.clone(),
        },
    );
    recorder.record_tool_result(
        &call,
        &AgentToolResult {
            exact_archive_file: None,
            call_id: call.id.clone(),
            tool: call.tool.clone(),
            ok: false,
            result: Some(json!({ "executed": false })),
            error: Some("unknown tool".to_string()),
        },
    );
    let rejected = recorder.finish(
        "run-unregistered",
        "conversation-unregistered",
        "assistant-unregistered",
        ConversationTurnTraceTerminalStatus::Completed,
        None,
    );
    rejected.validate().unwrap();

    let mut impossible_success = rejected;
    let ConversationTurnTraceItem::ToolResult {
        status,
        success,
        error,
        ..
    } = &mut impossible_success.items[1]
    else {
        panic!("expected tool result");
    };
    *status = ConversationTraceToolResultStatus::Succeeded;
    *success = true;
    *error = None;
    assert!(impossible_success.validate().is_err());

    assert!(serde_json::from_value::<AgentToolIdentity>(json!({
        "type": "unregistered",
        "toolName": "hallucinated_tool",
        "capabilities": []
    }))
    .is_err());
}

#[test]
fn current_model_context_item_requires_total_fields_and_rejects_extra_keys() {
    let current = json!({
        "sequence": 0,
        "ordinal": 0,
        "role": "assistant",
        "content": "done",
        "isError": false
    });
    serde_json::from_value::<ConversationModelContextItem>(current.clone()).unwrap();

    for missing in ["ordinal", "isError"] {
        let mut malformed = current.clone();
        malformed
            .as_object_mut()
            .expect("model context object")
            .remove(missing);
        assert!(serde_json::from_value::<ConversationModelContextItem>(malformed).is_err());
    }

    let mut extra = current;
    extra
        .as_object_mut()
        .expect("model context object")
        .insert("legacyIndex".to_string(), json!(1));
    assert!(serde_json::from_value::<ConversationModelContextItem>(extra).is_err());
}

#[test]
fn model_context_allows_duplicate_raw_provider_ids_but_rejects_duplicate_runtime_ids() {
    let call = |index: u32, runtime_id: &str| AgentContextCheckpointToolCall {
        id: runtime_id.to_string(),
        name: "read_file".to_string(),
        args: json!({ "path": format!("file-{index}.txt") }),
        provider_identity: AgentProviderToolCallIdentity {
            provider_tool_index: index,
            provider_call_id: "provider-reused-id".to_string(),
            runtime_call_id: runtime_id.to_string(),
        },
    };
    let mut item = ConversationModelContextItem {
        sequence: 0,
        ordinal: 0,
        role: "assistant".to_string(),
        content: String::new(),
        tool_call_id: None,
        tool_calls: vec![call(0, "runtime-0"), call(1, "runtime-1")],
        is_error: false,
    };

    item.validate().unwrap();

    item.tool_calls[1].id = "runtime-0".to_string();
    item.tool_calls[1].provider_identity.runtime_call_id = "runtime-0".to_string();
    assert!(item.validate().is_err());
}

#[test]
fn flattened_archive_metadata_rejects_unknown_fields() {
    let mut item = serde_json::to_value(ConversationTurnTraceItem::ToolResult {
        sequence: 1,
        call_id: "call-1".to_string(),
        tool: "read_file".to_string(),
        status: ConversationTraceToolResultStatus::Succeeded,
        success: true,
        observation: json!({ "content": "done" }),
        approval_status: AgentApprovalStatus::NotRequired,
        error: None,
        truncated: false,
        archive: ConversationHistoryArchiveTraceMetadata::default(),
    })
    .unwrap();
    item.as_object_mut()
        .expect("trace item object")
        .insert("archiveFutureField".to_string(), json!(true));

    assert!(serde_json::from_value::<ConversationTurnTraceItem>(item).is_err());
}

#[test]
fn completed_trace_rejects_a_missing_model_context_suffix() {
    let mut recorder = ConversationTraceRecorder::default();
    recorder
        .record_narration("I will inspect the file.")
        .unwrap();
    let call = AgentToolCall {
        id: "read-current".to_string(),
        tool: "read_file".to_string(),
        args: json!({ "path": "README.md" }),
        approval_status: AgentApprovalStatus::NotRequired,
        reason: None,
    };
    let call_sequence = recorder.record_tool_call(&call).unwrap();
    recorder
        .record_model_message(
            call_sequence,
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
    let result = AgentToolResult {
        exact_archive_file: None,
        call_id: call.id.clone(),
        tool: call.tool.clone(),
        ok: true,
        result: Some(json!({ "content": "current" })),
        error: None,
    };
    let result_sequence = recorder.record_tool_result(&call, &result).unwrap();
    recorder
        .record_model_message(
            result_sequence,
            0,
            &LlmMessage::tool_result(call.id.clone(), "current", false),
        )
        .unwrap();
    let snapshot = recorder.snapshot();
    let trace = recorder.finish(
        "run-current",
        "conversation-current",
        "assistant-current",
        ConversationTurnTraceTerminalStatus::Completed,
        None,
    );

    trace
        .validate_complete_model_context(&snapshot.model_context_items)
        .unwrap();
    let narration_only = &snapshot.model_context_items[..1];
    assert!(trace
        .validate_complete_model_context(narration_only)
        .unwrap_err()
        .contains("every closed trace item"));
}

#[test]
fn model_observation_is_compact_and_does_not_expose_backend_history_metadata() {
    let result = AgentToolResult {
        exact_archive_file: None,
        call_id: "call-1".to_string(),
        tool: "read_file".to_string(),
        ok: true,
        result: Some(json!({ "content": "bounded body" })),
        error: None,
    };
    let history_ref = crate::ContextHistoryRef::trace_item("assistant-1", 7);
    let rendered = render_tool_observation_with_history_ref(&result, Some(&history_ref));

    assert_eq!(rendered, "{\"content\":\"bounded body\"}");
    assert!(!rendered.contains("historyRef"));
    assert!(!rendered.contains("assistantMessageId"));
    assert!(!rendered.contains("```"));
}

#[test]
fn recorder_keeps_runtime_checkpoint_but_bounds_web_body_in_durable_trace() {
    let mut recorder = ConversationTraceRecorder::default();
    let first = call("call-1");
    recorder.record_narration("I will fetch the page.").unwrap();
    recorder.record_tool_call(&first);
    recorder.record_tool_result(
        &first,
        &AgentToolResult {
            exact_archive_file: None,
            call_id: first.id.clone(),
            tool: first.tool.clone(),
            ok: true,
            result: Some(json!({ "content": "x".repeat(20_000) })),
            error: None,
        },
    );
    let second = call("call-2");
    recorder.record_tool_call(&second);
    recorder.record_tool_result(
        &second,
        &AgentToolResult {
            exact_archive_file: None,
            call_id: second.id.clone(),
            tool: second.tool.clone(),
            ok: false,
            result: None,
            error: Some("same failure".to_string()),
        },
    );

    let (checkpoint_items, _, _, _) = recorder.checkpoint();
    let ConversationTurnTraceItem::ToolResult {
        observation: checkpoint_observation,
        ..
    } = &checkpoint_items[2]
    else {
        panic!("expected checkpoint tool result");
    };
    assert_eq!(
        checkpoint_observation["content"].as_str().unwrap().len(),
        20_000
    );

    let trace = recorder.finish(
        "run-1",
        "conversation-1",
        "assistant-1",
        ConversationTurnTraceTerminalStatus::Completed,
        None,
    );
    trace.validate().unwrap();
    assert_eq!(trace.items.len(), 5);
    let ConversationTurnTraceItem::ToolResult { observation, .. } = &trace.items[2] else {
        panic!("expected tool result");
    };
    assert!(observation.get("content").is_none());
    assert!(observation["summary"].as_str().unwrap().chars().count() < 20_000);
    assert_eq!(observation["bodyTruncatedInHistory"], true);
    assert!(trace.truncated);
}

#[test]
fn recorder_omits_command_session_output_from_durable_trace() {
    const SECRET_OUTPUT: &str = "command-session-secret-marker\nsecond line";
    const OUTPUT_HASH: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    let call = AgentToolCall {
        id: "command-session-wait".to_string(),
        tool: "command_session".to_string(),
        args: json!({
            "sessionId": "cmd_0123456789abcdef0123456789abcdef",
            "action": "wait",
        }),
        approval_status: AgentApprovalStatus::NotRequired,
        reason: None,
    };
    let result = AgentToolResult {
        exact_archive_file: None,
        call_id: call.id.clone(),
        tool: call.tool.clone(),
        ok: true,
        result: Some(json!({
            "sessionId": "cmd_0123456789abcdef0123456789abcdef",
            "status": "running",
            "output": SECRET_OUTPUT,
            "exitCode": null,
            "latestSequence": 17,
            "outputTruncated": false,
            "read": {
                "requestedAfterSequence": 12,
                "firstSequence": 13,
                "throughSequence": 17,
                "truncatedBefore": false,
                "outputBytes": SECRET_OUTPUT.len(),
                "outputHash": OUTPUT_HASH,
                "hostPrivateReadField": "must not survive",
            },
            "hostPrivateField": "must not survive",
        })),
        error: None,
    };

    let mut recorder = ConversationTraceRecorder::default();
    recorder.record_tool_call(&call);
    recorder.record_tool_result(&call, &result);

    let (checkpoint_items, _, _, _) = recorder.checkpoint();
    let ConversationTurnTraceItem::ToolResult {
        observation: checkpoint_observation,
        ..
    } = &checkpoint_items[1]
    else {
        panic!("expected checkpoint command_session result");
    };
    assert_eq!(checkpoint_observation["output"], SECRET_OUTPUT);

    let trace = recorder.finish(
        "run-command-session",
        "conversation-command-session",
        "assistant-command-session",
        ConversationTurnTraceTerminalStatus::Completed,
        None,
    );
    trace.validate().unwrap();
    let ConversationTurnTraceItem::ToolResult {
        observation,
        truncated,
        ..
    } = &trace.items[1]
    else {
        panic!("expected durable command_session result");
    };

    assert!(*truncated);
    assert!(observation.get("output").is_none());
    assert_eq!(observation["sessionId"], call.args["sessionId"]);
    assert_eq!(observation["status"], "running");
    assert_eq!(observation["exitCode"], Value::Null);
    assert_eq!(observation["latestSequence"], 17);
    assert_eq!(observation["outputTruncated"], false);
    assert_eq!(observation["read"]["requestedAfterSequence"], 12);
    assert_eq!(observation["read"]["firstSequence"], 13);
    assert_eq!(observation["read"]["throughSequence"], 17);
    assert_eq!(observation["read"]["truncatedBefore"], false);
    assert_eq!(observation["read"]["outputBytes"], SECRET_OUTPUT.len());
    assert_eq!(observation["read"]["outputHash"], OUTPUT_HASH);
    assert!(observation.get("hostPrivateField").is_none());
    assert!(observation["read"].get("hostPrivateReadField").is_none());
    assert!(trace.truncated);

    let serialized = serde_json::to_string(&trace).unwrap();
    assert!(!serialized.contains(SECRET_OUTPUT));
    assert!(!serialized.contains("must not survive"));
}

#[test]
fn binary_fields_are_removed_but_neighboring_text_is_preserved() {
    let result = canonical_tool_result_for_context(&AgentToolResult {
        exact_archive_file: None,
        call_id: "call-1".to_string(),
        tool: "read_image".to_string(),
        ok: true,
        result: Some(json!({
            "path": "image.png",
            "image": { "mimeType": "image/png", "dataBase64": "SGVsbG8=" },
            "note": "keep me",
        })),
        error: None,
    });
    assert_eq!(
        result.result.as_ref().unwrap()["image"]["dataBase64"],
        "[binary/base64 omitted]"
    );
    assert_eq!(result.result.as_ref().unwrap()["note"], "keep me");
}

#[test]
fn binary_sanitization_is_idempotent_and_the_durable_trace_validates() {
    let call = AgentToolCall {
        id: "call-image".to_string(),
        tool: "read_image".to_string(),
        args: json!({ "path": "image.png" }),
        approval_status: AgentApprovalStatus::NotRequired,
        reason: None,
    };
    let raw = AgentToolResult {
        exact_archive_file: None,
        call_id: call.id.clone(),
        tool: call.tool.clone(),
        ok: true,
        result: Some(json!({
            "path": "image.png",
            "thumbnailDataUrl": "data:image/png;base64,dGh1bWI=",
            "image": {
                "mimeType": "image/png",
                "dataBase64": "ZnVsbC1pbWFnZQ=="
            }
        })),
        error: None,
    };
    let once = canonical_tool_result_for_context(&raw);
    let twice = canonical_tool_result_for_context(&once);

    assert_eq!(once.result, twice.result);
    let mut recorder = ConversationTraceRecorder::default();
    recorder.record_tool_call(&call);
    recorder.record_tool_result(&call, &once);
    let trace = recorder.finish(
        "run-image",
        "conversation-image",
        "assistant-image",
        ConversationTurnTraceTerminalStatus::Completed,
        None,
    );
    trace.validate().unwrap();
    assert!(trace.truncated);
    let serialized = serde_json::to_string(&trace).unwrap();
    assert!(!serialized.contains("data:image/png;base64"));
    assert!(!serialized.contains("ZnVsbC1pbWFnZQ=="));
    assert!(!serialized.contains("dGh1bWI="));
}

#[test]
fn failed_tool_observation_preserves_sanitized_structured_result_for_model() {
    let result = canonical_tool_result_for_context(&AgentToolResult {
        exact_archive_file: None,
        call_id: "call-command".to_string(),
        tool: "run_command".to_string(),
        ok: false,
        result: Some(json!({
            "exitCode": 1,
            "stdout": "partial output\n",
            "stderr": "ModuleNotFoundError: No module named 'openpyxl'\n",
            "timedOut": false,
            "cancelled": false,
            "diagnosticBase64": "c2VjcmV0",
        })),
        error: Some("命令执行失败。".to_string()),
    });

    let observation = render_tool_observation(&result);

    assert!(!observation.contains("\"ok\""));
    assert!(!observation.contains("\"callId\""));
    assert!(observation.contains("\"exitCode\":1"));
    assert!(observation.contains("partial output\\n"));
    assert!(observation.contains("ModuleNotFoundError"));
    assert!(observation.contains("\"timedOut\":false"));
    assert!(observation.contains("\"cancelled\":false"));
    assert!(observation.contains("[binary/base64 omitted]"));
    assert!(!observation.contains("c2VjcmV0"));
    assert!(observation.contains("命令执行失败。"));
}

#[test]
fn failed_tool_observation_without_structured_result_is_still_well_formed() {
    let observation = render_tool_observation(&AgentToolResult {
        exact_archive_file: None,
        call_id: "call-failed-before-execution".to_string(),
        tool: "run_command".to_string(),
        ok: false,
        result: None,
        error: Some("命令未执行。".to_string()),
    });

    let observation: Value = serde_json::from_str(&observation).unwrap();
    assert_eq!(observation["status"], "failed");
    assert_eq!(observation["error"], "命令未执行。");
}

#[test]
fn committed_prefix_never_ends_on_a_tool_call() {
    let mut recorder = ConversationTraceRecorder::default();
    recorder.record_narration("Before approval.").unwrap();
    recorder.record_tool_call(&call("pending"));
    let committed = recorder.snapshot().committed_prefix();
    assert_eq!(committed.items.len(), 1);
    assert!(committed.items[0].is_safe_compaction_boundary());
}

#[test]
fn in_progress_audit_keeps_open_call_while_context_prefix_does_not() {
    let mut recorder = ConversationTraceRecorder::default();
    recorder
        .record_narration("I will generate the image.")
        .unwrap();
    let mut pending = call("image-call");
    pending.tool = "image_generation".to_string();
    pending.args = json!({
        "request": { "operation": "generate", "prompt": "private prompt" },
        "reason": "Create the requested image."
    });
    recorder.record_tool_call(&pending);
    let snapshot = recorder.snapshot();

    let audit = snapshot.in_progress_audit_trace("run", "conversation", "assistant");
    let context = snapshot.in_progress_trace("run", "conversation", "assistant");
    audit.validate().unwrap();
    context.validate().unwrap();
    assert!(matches!(
        audit.items.last(),
        Some(ConversationTurnTraceItem::ToolCall { call_id, .. }) if call_id == "image-call"
    ));
    assert!(matches!(
        context.items.last(),
        Some(ConversationTurnTraceItem::AssistantNarration { .. })
    ));
    let mut terminal = audit.clone();
    terminal.terminal_status = ConversationTurnTraceTerminalStatus::Failed;
    terminal.terminal_error = Some("interrupted".to_string());
    assert!(terminal.validate().is_err());
}

#[test]
fn recovered_result_closes_the_same_durable_call_once() {
    let mut recorder = ConversationTraceRecorder::default();
    let mut pending = call("image-call");
    pending.tool = "image_generation".to_string();
    pending.args = json!({
        "request": { "operation": "generate", "prompt": "private prompt" },
        "reason": "Create the requested image."
    });
    recorder.record_tool_call(&pending);
    let audit = recorder
        .snapshot()
        .in_progress_audit_trace("run", "conversation", "assistant");
    let result = AgentToolResult {
        exact_archive_file: None,
        call_id: pending.id.clone(),
        tool: pending.tool.clone(),
        ok: false,
        result: Some(json!({ "status": "outcomeIndeterminate" })),
        error: Some("execution interrupted".to_string()),
    };

    let recovered = conversation_trace_with_recovered_tool_result(&audit, &result).unwrap();
    recovered.validate().unwrap();
    assert!(matches!(
        recovered.items.last(),
        Some(ConversationTurnTraceItem::ToolResult { call_id, .. }) if call_id == "image-call"
    ));
    assert!(conversation_trace_with_recovered_tool_result(&recovered, &result).is_err());
}

#[test]
fn user_guidance_is_canonical_ordered_and_never_persists_attachment_bytes() {
    let mut recorder = ConversationTraceRecorder::default();
    recorder.record_narration("Initial answer.").unwrap();
    let sequence = recorder
        .record_user_guidance(
            "guidance-1",
            "client-1",
            "Please also inspect the image.",
            &[AgentInputAttachment {
                id: "attachment-1".to_string(),
                kind: AgentInputAttachmentKind::Image,
                name: "diagram.png".to_string(),
                mime_type: Some("image/png".to_string()),
                size_bytes: 6,
                encoding: crate::protocol::AgentInputAttachmentEncoding::Base64,
                data: "c2VjcmV0".to_string(),
                truncated: None,
            }],
            42,
        )
        .unwrap();
    assert_eq!(sequence, 1);

    let trace = recorder.finish(
        "run-1",
        "conversation-1",
        "assistant-1",
        ConversationTurnTraceTerminalStatus::Completed,
        None,
    );
    trace.validate().unwrap();
    let serialized = serde_json::to_string(&trace).unwrap();
    assert!(serialized.contains("\"type\":\"user_guidance\""));
    assert!(serialized.contains("diagram.png"));
    assert!(!serialized.contains("c2VjcmV0"));
}

#[test]
fn current_trace_attachment_rejects_unknown_fields() {
    assert!(
        serde_json::from_value::<ConversationTraceAttachment>(json!({
            "id": "attachment-1",
            "kind": "image",
            "name": "diagram.png",
            "sizeBytes": 6,
            "futureField": true
        }))
        .is_err()
    );
}

#[test]
fn user_guidance_cannot_split_an_open_tool_exchange() {
    let mut recorder = ConversationTraceRecorder::default();
    recorder.record_tool_call(&call("pending"));
    assert!(recorder
        .record_user_guidance("guidance-1", "client-1", "change course", &[], 42)
        .is_none());
}

#[test]
fn staged_and_direct_file_change_projection_keeps_effect_metadata_without_payloads() {
    let staged_call = AgentToolCall {
        id: "staged-1".into(),
        tool: "apply_patch".into(),
        args: json!({
            "request": {
                "action": "append",
                "transactionId": "file-change-1",
                "index": 0,
                "expectedDraftRevision": 0,
                "content": "first\nsecond\nSECRET_WRITE_BODY"
            }
        }),
        approval_status: AgentApprovalStatus::NotRequired,
        reason: None,
    };
    let patch_call = AgentToolCall {
        id: "patch-1".into(),
        tool: "apply_patch".into(),
        args: json!({
            "request": {
                "action": "apply",
                "operation": "update",
                "filePath": "src/lib.rs",
                "observationId": "fobs_trace_projection",
                "edits": [{
                    "kind": "replace",
                    "oldText": "old",
                    "newText": "new\nextra"
                }],
                "summary": "Update the implementation"
            }
        }),
        approval_status: AgentApprovalStatus::Approved,
        reason: None,
    };
    let mut recorder = ConversationTraceRecorder::default();
    recorder.record_tool_call(&staged_call);
    recorder.record_tool_result(
        &staged_call,
        &AgentToolResult {
            exact_archive_file: None,
            call_id: staged_call.id.clone(),
            tool: staged_call.tool.clone(),
            ok: true,
            result: Some(json!({
                "status": "drafting",
                "operation": "create",
                "transactionId": "file-change-1",
                "filePath": "notes.txt",
                "additions": 3,
                "deletions": 0,
                "lineCount": 3,
                "byteCount": 30,
                "privateTail": "SECRET_WRITE_BODY"
            })),
            error: None,
        },
    );
    recorder.record_tool_call(&patch_call);
    recorder.record_tool_result(
        &patch_call,
        &AgentToolResult {
            exact_archive_file: None,
            call_id: patch_call.id.clone(),
            tool: patch_call.tool.clone(),
            ok: true,
            result: Some(json!({
                "schemaVersion": 1,
                "status": "applied",
                "outcome": "applied",
                "transactionId": "file-change-direct-1",
                "operation": "update",
                "updateStrategy": null,
                "filePath": "src/lib.rs",
                "additions": 2,
                "deletions": 1,
                "lineCount": 2,
                "byteCount": 9,
                "revision": "file-revision-sha256-v1:fixture",
                "errorCode": null,
                "error": null,
                "message": "文件已更新。"
            })),
            error: None,
        },
    );

    let trace = recorder.finish(
        "run",
        "conversation",
        "assistant",
        ConversationTurnTraceTerminalStatus::Completed,
        None,
    );
    trace.validate().unwrap();
    let serialized = serde_json::to_string(&trace).unwrap();
    assert!(!serialized.contains("SECRET_WRITE_BODY"));
    assert!(!serialized.contains("--- a/src/lib.rs"));

    let ConversationTurnTraceItem::ToolCall {
        operation: staged_operation,
        ..
    } = &trace.items[0]
    else {
        panic!("expected staged call");
    };
    assert_eq!(staged_operation["request"]["contentBytes"], 30);
    assert_eq!(staged_operation["request"]["additions"], 3);
    let ConversationTurnTraceItem::ToolResult {
        observation: staged_result,
        ..
    } = &trace.items[1]
    else {
        panic!("expected staged result");
    };
    assert_eq!(staged_result["filePath"], "notes.txt");
    assert_eq!(staged_result["additions"], 3);

    let ConversationTurnTraceItem::ToolCall {
        operation: patch_operation,
        ..
    } = &trace.items[2]
    else {
        panic!("expected patch call");
    };
    assert_eq!(patch_operation["request"]["filePath"], "src/lib.rs");
    assert_eq!(patch_operation["request"]["additions"], 2);
    assert_eq!(patch_operation["request"]["deletions"], 1);
    let ConversationTurnTraceItem::ToolResult {
        observation: patch_result,
        approval_status,
        ..
    } = &trace.items[3]
    else {
        panic!("expected patch result");
    };
    assert_eq!(*approval_status, AgentApprovalStatus::Approved);
    assert_eq!(patch_result["status"], "applied");
    assert_eq!(patch_result["additions"], 2);
    assert_eq!(patch_result["deletions"], 1);
    assert!(patch_result.get("gitDiff").is_none());
    assert!(patch_result.get("content").is_none());
}

#[test]
fn command_and_read_results_keep_bounded_tail_or_summary() {
    let command_call = AgentToolCall {
        id: "command-1".into(),
        tool: "run_command".into(),
        args: json!({
            "command": "cargo test",
            "cwd": "workspace",
            "reason": "verify"
        }),
        approval_status: AgentApprovalStatus::Approved,
        reason: None,
    };
    let read_call = AgentToolCall {
        id: "read-1".into(),
        tool: "read_file".into(),
        args: json!({ "path": "src/lib.rs", "startLine": 20, "maxLines": 50 }),
        approval_status: AgentApprovalStatus::NotRequired,
        reason: None,
    };
    let mut recorder = ConversationTraceRecorder::default();
    recorder.record_tool_call(&command_call);
    recorder.record_tool_result(
        &command_call,
        &AgentToolResult {
            exact_archive_file: None,
            call_id: command_call.id.clone(),
            tool: command_call.tool.clone(),
            ok: false,
            result: Some(json!({
                "command": "cargo test",
                "cwd": "workspace",
                "exitCode": 1,
                "stdout": format!("BEGIN_MUST_NOT_SURVIVE{}", "x".repeat(4_000)),
                "stderr": "the actionable failure is at the end",
                "timedOut": false,
                "cancelled": false,
                "durationMs": 25,
                "stdoutTruncated": false,
                "stderrTruncated": false
            })),
            error: Some("command failed".into()),
        },
    );
    recorder.record_tool_call(&read_call);
    recorder.record_tool_result(
        &read_call,
        &AgentToolResult {
            exact_archive_file: None,
            call_id: read_call.id.clone(),
            tool: read_call.tool.clone(),
            ok: true,
            result: Some(json!({
                "path": "src/lib.rs",
                "startLine": 20,
                "endLine": 200,
                "totalLines": 500,
                "truncated": true,
                "content": format!("READ_PREFIX{}", "z".repeat(5_000))
            })),
            error: None,
        },
    );
    let trace = recorder.finish(
        "run",
        "conversation",
        "assistant",
        ConversationTurnTraceTerminalStatus::Completed,
        None,
    );
    let serialized = serde_json::to_string(&trace).unwrap();
    assert!(!serialized.contains("BEGIN_MUST_NOT_SURVIVE"));
    assert!(!serialized.contains(&"z".repeat(2_000)));

    let ConversationTurnTraceItem::ToolResult {
        observation: command,
        ..
    } = &trace.items[1]
    else {
        panic!("expected command result");
    };
    assert_eq!(command["command"], "cargo test");
    assert_eq!(command["cwd"], "workspace");
    assert_eq!(command["exitCode"], 1);
    assert!(command["stdoutTail"]
        .as_str()
        .unwrap()
        .starts_with("[earlier output omitted]"));
    assert!(command["stderrTail"]
        .as_str()
        .unwrap()
        .contains("actionable failure"));

    let ConversationTurnTraceItem::ToolCall {
        operation: read_operation,
        ..
    } = &trace.items[2]
    else {
        panic!("expected read call");
    };
    assert_eq!(read_operation["path"], "src/lib.rs");
    assert_eq!(read_operation["startLine"], 20);
    let ConversationTurnTraceItem::ToolResult {
        observation: read, ..
    } = &trace.items[3]
    else {
        panic!("expected read result");
    };
    assert_eq!(read["path"], "src/lib.rs");
    assert_eq!(read["startLine"], 20);
    assert_eq!(read["contentTruncatedInHistory"], true);
    assert!(read["summary"].as_str().unwrap().contains("READ_PREFIX"));
}

#[test]
fn durable_projection_redacts_embedded_data_urls_and_hidden_reasoning() {
    let call = AgentToolCall {
        id: "unknown-1".into(),
        tool: "extension_tool".into(),
        args: json!({
            "note": "prefix data:image/png;base64,U0VDUkVUX0lNQUdF suffix",
            "nested": { "arbitraryBase64": "U0VDUkVU", "safe": "keep" }
        }),
        approval_status: AgentApprovalStatus::NotRequired,
        reason: None,
    };
    let mut recorder = ConversationTraceRecorder::default();
    recorder.record_tool_call(&call);
    recorder.record_tool_result(
        &call,
        &AgentToolResult {
            exact_archive_file: None,
            call_id: call.id.clone(),
            tool: call.tool.clone(),
            ok: true,
            result: Some(json!({
                "message": "before data:image/jpeg;base64,QUJDRA== after",
                "reasoning": "hidden chain must not persist",
                "safe": "visible evidence"
            })),
            error: None,
        },
    );
    let trace = recorder.finish(
        "run",
        "conversation",
        "assistant",
        ConversationTurnTraceTerminalStatus::Completed,
        None,
    );
    let serialized = serde_json::to_string(&trace).unwrap();
    assert!(!serialized.contains("U0VDUkVUX0lNQUdF"));
    assert!(!serialized.contains("U0VDUkVU"));
    assert!(!serialized.contains("QUJDRA=="));
    assert!(!serialized.contains("hidden chain must not persist"));
    assert!(serialized.contains("visible evidence"));
    trace.validate().unwrap();
}

#[test]
fn repeated_unchanged_read_failure_keeps_paired_reference_instead_of_duplicate_error() {
    let mut recorder = ConversationTraceRecorder::default();
    for id in ["read-1", "read-2"] {
        let call = AgentToolCall {
            id: id.into(),
            tool: "read_file".into(),
            args: json!({ "path": "missing.txt" }),
            approval_status: AgentApprovalStatus::NotRequired,
            reason: None,
        };
        recorder.record_tool_call(&call);
        recorder.record_tool_result(
            &call,
            &AgentToolResult {
                exact_archive_file: None,
                call_id: call.id.clone(),
                tool: call.tool.clone(),
                ok: false,
                result: Some(json!({ "path": "missing.txt", "code": "notFound" })),
                error: Some("file does not exist".into()),
            },
        );
    }
    let trace = recorder.finish(
        "run",
        "conversation",
        "assistant",
        ConversationTurnTraceTerminalStatus::Failed,
        Some("unable to read the requested file"),
    );
    trace.validate().unwrap();
    assert_eq!(trace.items.len(), 4);
    let ConversationTurnTraceItem::ToolResult {
        observation,
        error,
        truncated,
        ..
    } = &trace.items[3]
    else {
        panic!("expected repeated result");
    };
    assert_eq!(observation["repeatedFailure"], true);
    assert_eq!(observation["sameAsCallId"], "read-1");
    assert!(error.is_none());
    assert!(*truncated);
}

#[test]
fn approval_checkpoint_keeps_exact_direct_args_while_durable_snapshot_is_bounded() {
    let call = AgentToolCall {
        id: "patch-approval".into(),
        tool: "apply_patch".into(),
        args: json!({
            "request": {
                "action": "apply",
                "operation": "update",
                "filePath": "src/main.rs",
                "observationId": "fobs_approval_checkpoint",
                "edits": [{
                    "kind": "replace",
                    "oldText": "old",
                    "newText": "new"
                }]
            }
        }),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };
    let mut recorder = ConversationTraceRecorder::default();
    recorder.record_tool_call(&call);
    let (checkpoint, _, _, _) = recorder.checkpoint();
    let ConversationTurnTraceItem::ToolCall {
        operation: checkpoint_operation,
        ..
    } = &checkpoint[0]
    else {
        panic!("expected checkpoint call");
    };
    assert_eq!(
        checkpoint_operation["request"]["observationId"],
        "fobs_approval_checkpoint"
    );
    assert_eq!(
        checkpoint_operation["request"]["edits"][0]["oldText"],
        "old"
    );
    assert_eq!(
        checkpoint_operation["request"]["edits"][0]["newText"],
        "new"
    );

    let durable = recorder
        .snapshot()
        .in_progress_audit_trace("run", "conversation", "assistant");
    let ConversationTurnTraceItem::ToolCall {
        operation: durable_operation,
        ..
    } = &durable.items[0]
    else {
        panic!("expected durable call");
    };
    let durable_request = &durable_operation["request"];
    assert!(durable_request.get("edits").is_none());
    assert!(durable_request.get("observationId").is_none());
    assert_eq!(durable_request["filePath"], "src/main.rs");
    assert_eq!(durable_request["editCount"], 1);
    assert_eq!(durable_request["additions"], 1);
    assert_eq!(durable_request["deletions"], 1);
}

#[test]
fn collaboration_receipt_can_cover_multiple_fifo_messages_with_exact_model_envelopes() {
    let mut recorder = ConversationTraceRecorder::default();
    let first = recorder
        .record_agent_mailbox_delivery(
            0,
            "receipt-1",
            "message-1",
            "agent-parent",
            "Parent",
            "/root",
            crate::AgentMailboxKind::Followup,
            "  first payload  ",
            1,
        )
        .unwrap()
        .unwrap();
    let second = recorder
        .record_agent_mailbox_delivery(
            1,
            "receipt-1",
            "message-2",
            "agent-child",
            "Child",
            "/root/child",
            crate::AgentMailboxKind::Result,
            "second\u{0007}payload",
            2,
        )
        .unwrap()
        .unwrap();
    let snapshot = recorder.snapshot();
    assert_eq!(snapshot.items.len(), 2);
    assert_eq!(snapshot.model_context_items.len(), 2);
    assert_eq!(snapshot.model_context_items[0].content, first);
    assert_eq!(snapshot.model_context_items[1].content, second);
    assert_eq!(
        serde_json::from_str::<Value>(&first).unwrap()["payload"],
        "first payload"
    );
    assert_eq!(
        serde_json::from_str::<Value>(&second).unwrap()["origin"],
        "agent"
    );
}

#[test]
fn collaboration_model_envelope_is_utf8_safe_deterministic_and_bounded() {
    let payload = "蒙".repeat(400_000);
    let first = project_agent_mailbox_model_envelope(
        "agent-parent",
        "Parent",
        "/root",
        crate::AgentMailboxKind::Followup,
        &payload,
    )
    .unwrap();
    let second = project_agent_mailbox_model_envelope(
        "agent-parent",
        "Parent",
        "/root",
        crate::AgentMailboxKind::Followup,
        &payload,
    )
    .unwrap();
    assert_eq!(first, second);
    assert!(first.1);
    assert!(first.0.len() <= AGENT_MAILBOX_MODEL_ENVELOPE_MAX_BYTES);
    let envelope: Value = serde_json::from_str(&first.0).unwrap();
    assert_eq!(envelope["payloadTruncated"], true);
    assert!(envelope["payload"]
        .as_str()
        .unwrap()
        .ends_with("...[agent mailbox payload truncated]"));
}

#[test]
fn large_collaboration_delivery_precommit_is_an_exact_terminal_trace_prefix() {
    let mut recorder = ConversationTraceRecorder::default();
    let model_content = recorder
        .record_agent_mailbox_delivery(
            0,
            "receipt-large",
            "message-large",
            "agent-child",
            "Child",
            "/root/child",
            crate::AgentMailboxKind::Result,
            &"evidence ".repeat(3_000),
            1,
        )
        .unwrap()
        .unwrap();
    let precommitted = recorder
        .snapshot()
        .in_progress_trace("run", "conversation", "assistant");
    let terminal = recorder.finish(
        "run",
        "conversation",
        "assistant",
        ConversationTurnTraceTerminalStatus::Completed,
        None,
    );

    let ConversationTurnTraceItem::AgentMailboxDelivery {
        content: trace_content,
        truncated,
        ..
    } = &precommitted.items[0]
    else {
        panic!("expected Agent mailbox delivery");
    };
    assert!(model_content.len() > trace_content.len());
    assert!(*truncated);
    assert_eq!(
        serde_json::from_str::<Value>(trace_content).unwrap()["payloadTruncated"],
        true
    );
    assert_eq!(
        recorder.snapshot().model_context_items[0].content,
        model_content
    );
    assert_eq!(precommitted.items, terminal.items);
    assert!(precommitted.truncated);
}
