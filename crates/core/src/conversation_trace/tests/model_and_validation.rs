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
fn ordinary_cancelled_snapshot_keeps_tool_cancellation_separate_from_terminal_error() {
    let mut recorder = ConversationTraceRecorder::default();
    let unresolved = call("cancelled-call");
    record_open_call(&mut recorder, &unresolved);

    let projection = cancelled_conversation_trace_from_snapshot(
        recorder.snapshot(),
        "run-cancelled",
        "conversation-cancelled",
        "assistant-cancelled",
        "Run cancelled before a verifiable tool result was available.",
    )
    .unwrap();

    projection.trace.validate().unwrap();
    assert_eq!(
        projection.trace.terminal_status,
        ConversationTurnTraceTerminalStatus::Cancelled
    );
    assert_eq!(projection.trace.terminal_error, None);
    assert!(matches!(
        projection.trace.items.last(),
        Some(ConversationTurnTraceItem::ToolResult {
            call_id,
            status: ConversationTraceToolResultStatus::Cancelled,
            success: false,
            error: Some(error),
            ..
        }) if call_id == "cancelled-call"
            && error == "Run cancelled before a verifiable tool result was available."
    ));
}

#[test]
fn abnormal_cancelled_snapshot_can_retain_a_distinct_terminal_error() {
    let mut recorder = ConversationTraceRecorder::default();
    record_open_call(&mut recorder, &call("forced-cancelled-call"));

    let projection = cancelled_conversation_trace_from_snapshot_with_terminal_error(
        recorder.snapshot(),
        "run-forced-cancelled",
        "conversation-forced-cancelled",
        "assistant-forced-cancelled",
        "No verifiable Tool result was available during forced cancellation.",
        "Core shutdown timed out while cancelling the active run.",
    )
    .unwrap();

    projection.trace.validate().unwrap();
    assert_eq!(
        projection.trace.terminal_error.as_deref(),
        Some("Core shutdown timed out while cancelling the active run.")
    );
    assert!(matches!(
        projection.trace.items.last(),
        Some(ConversationTurnTraceItem::ToolResult {
            status: ConversationTraceToolResultStatus::Cancelled,
            error: Some(error),
            ..
        }) if error == "No verifiable Tool result was available during forced cancellation."
    ));
}

#[test]
fn item_free_cancellation_has_no_terminal_error() {
    let ordinary = cancelled_conversation_trace_without_items(
        "run-item-free-cancelled",
        "conversation-item-free-cancelled",
        "assistant-item-free-cancelled",
    );
    ordinary.validate().unwrap();
    assert_eq!(
        ordinary.terminal_status,
        ConversationTurnTraceTerminalStatus::Cancelled
    );
    assert_eq!(ordinary.terminal_error, None);
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
