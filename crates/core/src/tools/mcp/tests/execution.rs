use super::*;

#[tokio::test]
async fn structured_content_and_typed_route_survive_successful_execution() {
    let model_name = "mcp__fixture__structured";
    let input_schema = json!({
        "type": "object",
        "properties": {"left": {"type": "integer"}},
        "additionalProperties": false
    });
    let expected_provenance = provenance_for_schema("structured_result", model_name, &input_schema);
    let invoker = MockMcpToolInvoker::returning(
        vec![descriptor("structured_result", model_name, input_schema)],
        McpToolInvocationResult {
            content: vec![McpToolContentBlock::Text {
                text: "sum ready".to_string(),
            }],
            structured_content: Some(json!({"sum": 5, "operands": [2, 3]})),
            is_error: false,
            truncated_at_source: false,
        },
    );

    let registry = registry_with(invoker.clone());
    let approval = propose(&registry, model_name, json!({"left": 2})).unwrap();
    let result = invoke_prepared(invoker.as_ref(), approval, AgentCancellationToken::new())
        .await
        .unwrap();

    assert!(result.ok);
    let value = result.result.as_ref().unwrap();
    assert_eq!(value["type"], "mcp_tool");
    assert_eq!(value["external"], true);
    assert_eq!(value["status"], "completed");
    assert_eq!(value["outcome"], "succeeded");
    assert_eq!(value["dispatchCertainty"], "response_received");
    assert_eq!(value["contentCompleteness"], "complete");
    assert!(
        value.get("truncatedAtSource").is_none(),
        "a complete MCP response must not expose a misleading negative truncation marker"
    );
    assert_eq!(value["content"][0]["text"], "sum ready");
    assert_eq!(
        value["structuredContent"],
        json!({"sum": 5, "operands": [2, 3]})
    );
    assert_eq!(value["isError"], false);
    assert_eq!(
        value["provenance"],
        serde_json::to_value(&expected_provenance).unwrap()
    );
    let projection_registry = registry_with(invoker.clone());
    let trace_projection = projection_registry.trace_projection(&result);
    assert!(
        trace_projection
            .result
            .as_ref()
            .unwrap()
            .get("provenance")
            .is_none(),
        "MCP routing provenance belongs to the typed ToolCall, not the persisted result"
    );
    let model_projection = projection_registry.model_projection(&result);
    assert!(
        model_projection
            .result
            .as_ref()
            .unwrap()
            .get("provenance")
            .is_none(),
        "server identity, config digest and generation must not enter the model observation"
    );
    let projected_json = serde_json::to_string(&model_projection).unwrap();
    assert!(!projected_json.contains(&expected_provenance.server_id));
    assert!(!projected_json.contains(&expected_provenance.config_digest));
    let model_value = model_projection
        .result
        .as_ref()
        .expect("successful MCP model projection");
    assert_eq!(model_value["contentCompleteness"], "complete");
    assert_eq!(model_value["responseAuthority"], "server");
    assert_eq!(model_value["executionAttempted"], true);
    assert_eq!(model_value["responseReceived"], true);
    assert_eq!(model_value["retryable"], false);
    assert_eq!(model_value["retryPolicy"], "do_not_repeat_completed_call");
    assert!(model_value["message"]
        .as_str()
        .is_some_and(|message| message.contains("authoritative Tool response")));
    let invocations = invoker.invocations.lock().unwrap();
    assert_eq!(invocations.len(), 1);
    assert_eq!(
        invocations[0].approval.identity.provenance,
        expected_provenance
    );
    assert_eq!(
        invoker.consumed_arguments.lock().unwrap()[0],
        json!({"left": 2})
    );

    let durable = mcp_tool_result_persistence_projection(&result);
    assert_eq!(
        durable.result,
        Some(json!({
            "schemaVersion": 1,
            "type": "mcp_tool",
            "external": true,
            "status": "completed",
            "outcome": "succeeded",
            "dispatchCertainty": "response_received",
            "contentOmitted": true,
            "isError": false,
        }))
    );
    assert_eq!(
        serde_json::to_value(mcp_tool_result_persistence_projection(&durable)).unwrap(),
        serde_json::to_value(&durable).unwrap(),
        "the durable projection must be idempotent"
    );
}

#[tokio::test]
async fn server_is_error_becomes_failed_tool_result_not_transport_failure() {
    let model_name = "mcp__fixture__tool_error";
    let invoker = MockMcpToolInvoker::returning(
        vec![descriptor(
            "return_tool_error",
            model_name,
            json!({"type": "object"}),
        )],
        McpToolInvocationResult {
            content: vec![McpToolContentBlock::Text {
                text: "fixture rejected the request".to_string(),
            }],
            structured_content: Some(json!({"reason": "fixture_error"})),
            is_error: true,
            truncated_at_source: false,
        },
    );

    let registry = registry_with(invoker.clone());
    let approval = propose(&registry, model_name, json!({})).unwrap();
    let result = invoke_prepared(invoker.as_ref(), approval, AgentCancellationToken::new())
        .await
        .expect("isError is a settled Tool result, not a transport error");

    assert!(!result.ok);
    assert!(result
        .error
        .as_deref()
        .is_some_and(|error| error.contains("MCP server reported")));
    let value = result.result.as_ref().expect("structured MCP error result");
    assert_eq!(value["type"], "mcp_tool");
    assert_eq!(value["external"], true);
    assert_eq!(value["status"], "completed");
    assert_eq!(value["outcome"], "tool_error");
    assert_eq!(value["dispatchCertainty"], "response_received");
    assert_eq!(value["isError"], true);
    assert_eq!(value["content"][0]["text"], "fixture rejected the request");
    assert_eq!(value["structuredContent"]["reason"], "fixture_error");

    let projected = mcp_tool_result_model_projection(&result);
    let projected = projected.result.expect("model MCP Tool-error envelope");
    assert_eq!(projected["responseAuthority"], "server");
    assert_eq!(projected["contentCompleteness"], "complete");
    assert_eq!(projected["executionAttempted"], true);
    assert_eq!(projected["responseReceived"], true);
    assert_eq!(projected["retryable"], false);
    assert_eq!(
        projected["retryPolicy"],
        "new_corrected_call_only_if_server_error_is_actionable"
    );
    assert!(projected["message"]
        .as_str()
        .is_some_and(|message| message.contains("not a transport failure")));
    assert_eq!(
        projected["content"][0]["text"], "fixture rejected the request",
        "the authoritative Server error detail must remain available to the model"
    );
}

#[test]
fn outcome_unknown_model_projection_never_authorizes_replay() {
    let result = AgentToolResult {
        exact_archive_file: None,
        call_id: "mcp-outcome-unknown-call".to_string(),
        tool: "mcp__fixture__uncertain".to_string(),
        ok: false,
        result: Some(json!({
            "schemaVersion": 1,
            "type": "mcp_tool",
            "external": true,
            "status": "outcome_unknown",
            "outcome": "outcome_unknown",
            "dispatchCertainty": "possibly_dispatched",
            "isError": true,
            "code": "mcp.tool_outcome_unknown",
            "retryable": false,
        })),
        error: Some("The external outcome is unknown.".to_string()),
    };

    let projected = mcp_tool_result_model_projection(&result);
    let value = projected.result.expect("outcome-unknown model envelope");
    assert_eq!(value["responseAuthority"], "host");
    assert_eq!(value["contentCompleteness"], "unavailable");
    assert_eq!(value["executionAttempted"], true);
    assert_eq!(value["responseReceived"], false);
    assert_eq!(value["retryable"], false);
    assert_eq!(
        value["retryPolicy"],
        "never_replay_check_authoritative_state"
    );
    assert!(value["message"]
        .as_str()
        .is_some_and(|message| message.contains("Never replay it automatically")));
}

#[test]
fn restored_durable_success_is_a_receipt_not_replayable_response_content() {
    let model_name = "mcp__fixture__durable_receipt";
    let invoker = MockMcpToolInvoker::returning(
        vec![descriptor(
            "durable_receipt",
            model_name,
            json!({"type": "object"}),
        )],
        empty_result(),
    );
    let registry = registry_with(invoker);
    let approval = propose(&registry, model_name, json!({})).unwrap();
    let live = mcp_tool_result_from_approved_invocation(
        &approval,
        &McpToolInvocationResult {
            content: vec![McpToolContentBlock::Text {
                text: "ephemeral authoritative content".to_string(),
            }],
            structured_content: None,
            is_error: false,
            truncated_at_source: false,
        },
    )
    .unwrap();
    let durable = mcp_tool_result_persistence_projection(&live);

    let projected = mcp_tool_result_model_projection(&durable);
    let value = projected.result.expect("restored durable MCP receipt");
    assert_eq!(value["status"], "completed");
    assert_eq!(value["outcome"], "succeeded");
    assert_eq!(value["responseAuthority"], "durable_receipt");
    assert_eq!(value["contentCompleteness"], "unavailable_after_restart");
    assert_eq!(value["executionAttempted"], true);
    assert_eq!(value["responseReceived"], true);
    assert_eq!(value["retryable"], false);
    assert_eq!(value["retryPolicy"], "never_replay_terminal_receipt");
    assert!(value.get("content").is_none());
    assert!(value.get("structuredContent").is_none());
    assert!(value["message"]
        .as_str()
        .is_some_and(|message| message.contains("Never replay")));
}

#[test]
fn omitted_content_block_marks_the_model_response_partial() {
    let model_name = "mcp__fixture__omitted_image";
    let invoker = MockMcpToolInvoker::returning(
        vec![descriptor(
            "omitted_image",
            model_name,
            json!({"type": "object"}),
        )],
        empty_result(),
    );
    let registry = registry_with(invoker);
    let approval = propose(&registry, model_name, json!({})).unwrap();
    let result = mcp_tool_result_from_approved_invocation(
        &approval,
        &McpToolInvocationResult {
            content: vec![McpToolContentBlock::Omitted {
                kind: McpOmittedContentKind::Image,
                mime_type: Some("image/png".to_string()),
                encoded_bytes: Some(512),
            }],
            structured_content: None,
            is_error: false,
            truncated_at_source: false,
        },
    )
    .unwrap();

    let raw = result.result.as_ref().expect("bounded MCP result");
    assert_eq!(raw["contentCompleteness"], "partial");
    assert_eq!(raw["truncatedAtSource"], true);
    assert_eq!(raw["diagnostics"]["contentBlocksOmitted"], 1);
    assert_eq!(raw["diagnostics"]["upstreamContentTruncated"], false);
    assert_eq!(raw["content"][0]["type"], "omitted");
    assert_eq!(raw["content"][0]["kind"], "image");

    let projected = mcp_tool_result_model_projection(&result);
    let projected = projected.result.expect("partial MCP model envelope");
    assert_eq!(projected["contentCompleteness"], "partial");
    assert_eq!(projected["responseAuthority"], "server");
    assert_eq!(projected["retryPolicy"], "do_not_repeat_completed_call");
}

#[test]
fn rejected_approval_is_an_authoritative_normal_tool_result() {
    let model_name = "mcp__fixture__rejected";
    let invoker = MockMcpToolInvoker::returning(
        vec![descriptor(
            "rejected",
            model_name,
            json!({"type": "object"}),
        )],
        empty_result(),
    );
    let registry = registry_with(invoker);
    let approval = propose(&registry, model_name, json!({})).unwrap();

    let without_feedback =
        mcp_tool_result_from_rejected_approval(&approval, None).expect("valid rejection result");
    assert_eq!(without_feedback.call_id, approval.identity.call_id);
    assert_eq!(
        without_feedback.tool,
        approval.identity.provenance.model_tool_name
    );
    assert!(without_feedback.ok);
    assert_eq!(without_feedback.error, None);
    assert_eq!(
        without_feedback.result,
        Some(json!({
            "schemaVersion": 1,
            "type": "mcp_tool",
            "external": true,
            "status": "rejected",
            "outcome": "rejected",
            "dispatchCertainty": "definitely_not_dispatched",
            "isError": false,
            "code": "mcp.approval_rejected",
            "retryable": false,
        }))
    );

    let feedback = "Please use a safer approach.";
    let with_feedback = mcp_tool_result_from_rejected_approval(&approval, Some(feedback))
        .expect("valid rejection result with feedback");
    assert!(with_feedback.ok);
    assert_eq!(with_feedback.error, None);
    assert_eq!(
        with_feedback
            .result
            .as_ref()
            .and_then(|value| value.get("userFeedback")),
        Some(&json!(feedback))
    );
}

#[test]
fn rejected_result_persistence_keeps_control_fields_but_removes_feedback() {
    let model_name = "mcp__fixture__rejected_projection";
    let invoker = MockMcpToolInvoker::returning(
        vec![descriptor(
            "rejected_projection",
            model_name,
            json!({"type": "object"}),
        )],
        empty_result(),
    );
    let registry = registry_with(invoker);
    let approval = propose(&registry, model_name, json!({})).unwrap();
    let private_feedback = "private feedback that must not enter durable state";
    let live_result = mcp_tool_result_from_rejected_approval(&approval, Some(private_feedback))
        .expect("valid rejection result");

    let durable = mcp_tool_result_persistence_projection(&live_result);
    assert_eq!(durable.call_id, live_result.call_id);
    assert_eq!(durable.tool, live_result.tool);
    assert!(durable.ok);
    assert_eq!(durable.error, None);
    assert_eq!(
        durable.result,
        Some(json!({
            "schemaVersion": 1,
            "type": "mcp_tool",
            "external": true,
            "status": "rejected",
            "outcome": "rejected",
            "dispatchCertainty": "definitely_not_dispatched",
            "contentOmitted": true,
            "isError": false,
            "feedbackProvided": true,
            "code": "mcp.approval_rejected",
            "retryable": false,
        }))
    );
    let durable_json = serde_json::to_string(&durable).unwrap();
    assert!(!durable_json.contains(private_feedback));
    for forbidden_field in ["content", "structuredContent", "provenance", "userFeedback"] {
        assert!(
            durable
                .result
                .as_ref()
                .and_then(|value| value.get(forbidden_field))
                .is_none(),
            "{forbidden_field} must not survive the durable projection"
        );
    }
    assert_eq!(
        serde_json::to_value(mcp_tool_result_persistence_projection(&durable)).unwrap(),
        serde_json::to_value(&durable).unwrap(),
        "the durable projection must remain stable when applied repeatedly"
    );
}

#[test]
fn rejected_result_model_projection_explains_redacted_arguments_and_blocks_automatic_retry() {
    let model_name = "mcp__fixture__rejected_model_projection";
    let invoker = MockMcpToolInvoker::returning(
        vec![descriptor(
            "rejected_model_projection",
            model_name,
            json!({"type": "object"}),
        )],
        empty_result(),
    );
    let registry = registry_with(invoker);
    let approval = propose(&registry, model_name, json!({})).unwrap();

    let without_feedback =
        mcp_tool_result_from_rejected_approval(&approval, None).expect("valid rejection result");
    let projected = mcp_tool_result_model_projection(&without_feedback);
    let value = projected.result.expect("model rejection envelope");
    assert_eq!(value["decisionBy"], "user");
    assert_eq!(value["executionAttempted"], false);
    assert_eq!(value["argumentsValidated"], true);
    assert_eq!(value["callReasonAccepted"], true);
    assert_eq!(value["argumentsInHistoryRedacted"], true);
    assert_eq!(value["code"], "mcp.approval_rejected");
    assert_eq!(value["retryable"], false);
    assert_eq!(
        value["retryPolicy"],
        "new_explicit_user_instruction_required"
    );
    assert!(value["message"]
        .as_str()
        .is_some_and(|message| message.contains("Do not retry")));

    let feedback = "Use a different directory.";
    let with_feedback = mcp_tool_result_from_rejected_approval(&approval, Some(feedback))
        .expect("valid rejection result with feedback");
    let projected = mcp_tool_result_model_projection(&with_feedback);
    let value = projected.result.expect("model rejection envelope");
    assert_eq!(value["userFeedback"], feedback);
    assert_eq!(
        value["retryPolicy"],
        "follow_user_feedback_without_repeating_same_call"
    );
    assert!(value["message"]
        .as_str()
        .is_some_and(|message| message.contains("do not repeat the same call unchanged")));

    let durable = mcp_tool_result_persistence_projection(&with_feedback);
    let restored_projection = mcp_tool_result_model_projection(&durable);
    let restored = restored_projection
        .result
        .expect("restored model rejection envelope");
    assert_eq!(restored["decisionBy"], "user");
    assert_eq!(restored["argumentsInHistoryRedacted"], true);
    assert_eq!(
        restored["retryPolicy"],
        "new_explicit_user_instruction_required"
    );
    assert!(restored.get("userFeedback").is_none());
    let durable_json = serde_json::to_string(&durable).unwrap();
    assert!(!durable_json.contains(feedback));
    assert!(!durable_json.contains("decisionBy"));
    assert!(!durable_json.contains("message"));
}

#[tokio::test]
async fn non_object_arguments_fail_before_invoker_is_called() {
    let model_name = "mcp__fixture__echo_text";
    let invoker = MockMcpToolInvoker::returning(
        vec![descriptor(
            "echo_text",
            model_name,
            json!({"type": "object"}),
        )],
        empty_result(),
    );

    let registry = registry_with(invoker.clone());
    let error = propose(&registry, model_name, json!(["not", "an", "object"]))
        .expect_err("invalid model arguments must fail before Host preparation");

    assert_eq!(error.code(), Some("mcp.invalid_tool_arguments"));
    assert!(invoker.invocations.lock().unwrap().is_empty());
    assert!(invoker.prepared.lock().unwrap().is_empty());
}

#[tokio::test]
async fn runtime_cancellation_token_is_passed_to_and_stops_invoker() {
    let model_name = "mcp__fixture__slow";
    let invoker = MockMcpToolInvoker::waiting_for_cancellation(vec![descriptor(
        "slow_tool",
        model_name,
        json!({"type": "object"}),
    )]);
    let registry = registry_with(invoker.clone());
    let approval = propose(&registry, model_name, json!({})).unwrap();
    let cancellation = AgentCancellationToken::new();
    let task_invoker = invoker.clone();
    let task_cancellation = cancellation.clone();
    let task = tokio::spawn(async move {
        invoke_prepared(task_invoker.as_ref(), approval, task_cancellation).await
    });

    tokio::time::timeout(Duration::from_secs(1), async {
        while invoker.cancellation_tokens.lock().unwrap().is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("mock invoker should start");
    let received = invoker.cancellation_tokens.lock().unwrap()[0].clone();
    assert!(received.shares_state_with(&cancellation));

    cancellation.cancel();
    let error = task
        .await
        .expect("execution task should not panic")
        .expect_err("cancelled MCP execution must cancel the Agent run");
    assert!(error.is_cancelled());
}

#[tokio::test]
async fn oversized_text_and_structured_content_are_bounded_with_diagnostics() {
    let model_name = "mcp__fixture__large_result";
    let oversized_text = format!(
        "{}TAIL_MUST_NOT_SURVIVE",
        "x".repeat(MAX_MCP_TEXT_RESULT_BYTES + 1_024)
    );
    let oversized_structured = json!({"blob": "y".repeat(MAX_MCP_STRUCTURED_RESULT_BYTES + 1_024)});
    let invoker = MockMcpToolInvoker::returning(
        vec![descriptor(
            "large_result",
            model_name,
            json!({"type": "object"}),
        )],
        McpToolInvocationResult {
            content: vec![McpToolContentBlock::Text {
                text: oversized_text,
            }],
            structured_content: Some(oversized_structured),
            is_error: false,
            truncated_at_source: false,
        },
    );

    let registry = registry_with(invoker.clone());
    let approval = propose(&registry, model_name, json!({})).unwrap();
    let result = invoke_prepared(invoker.as_ref(), approval, AgentCancellationToken::new())
        .await
        .unwrap();
    let value = result.result.unwrap();
    let retained_text = value["content"][0]["text"].as_str().unwrap();

    assert!(result.ok);
    assert!(!retained_text.contains("TAIL_MUST_NOT_SURVIVE"));
    assert!(retained_text.len() <= MAX_MCP_TEXT_RESULT_BYTES + 128);
    assert_eq!(value["diagnostics"]["textTruncated"], true);
    assert_eq!(value["diagnostics"]["structuredContentTruncated"], true);
    assert_eq!(
        value["structuredContent"]["_mycopilot"]["reason"],
        "structured_content_limit"
    );
    assert!(
        value["structuredContent"]["_mycopilot"]["originalBytes"]
            .as_u64()
            .unwrap()
            > MAX_MCP_STRUCTURED_RESULT_BYTES as u64
    );
}

#[tokio::test]
async fn deeply_nested_structured_content_is_omitted_before_serialization() {
    let model_name = "mcp__fixture__deep_result";
    let mut deeply_nested = Value::Null;
    for _ in 0..MAX_MCP_STRUCTURED_RESULT_DEPTH {
        deeply_nested = json!({"nested": deeply_nested});
    }
    let invoker = MockMcpToolInvoker::returning(
        vec![descriptor(
            "deep_result",
            model_name,
            json!({"type": "object"}),
        )],
        McpToolInvocationResult {
            content: Vec::new(),
            structured_content: Some(json!({"root": deeply_nested})),
            is_error: false,
            truncated_at_source: false,
        },
    );

    let registry = registry_with(invoker.clone());
    let approval = propose(&registry, model_name, json!({})).unwrap();
    let result = invoke_prepared(invoker.as_ref(), approval, AgentCancellationToken::new())
        .await
        .unwrap();
    let value = result.result.unwrap();

    assert_eq!(value["diagnostics"]["structuredContentTruncated"], true);
    assert_eq!(
        value["structuredContent"]["_mycopilot"]["reason"],
        "structured_content_structure_limit"
    );
}

#[test]
fn dynamic_mcp_registration_does_not_change_stable_tool_revision() {
    let mut registry = ToolRegistry::empty();
    registry.register_test_tool(TestBuiltinTool {
        name: "builtin_stable",
        description: "stable definition",
    });
    let before = EffectiveToolSet::from_permitted_definitions(
        &registry,
        registry.definitions(),
        &BTreeSet::new(),
    )
    .unwrap();
    let runtime = McpToolRuntime::capture(MockMcpToolInvoker::returning(
        vec![descriptor(
            "echo_text",
            "mcp__fixture__echo_text",
            json!({"type": "object"}),
        )],
        empty_result(),
    ));
    registry.register_mcp_runtime(&runtime);
    let after = EffectiveToolSet::from_permitted_definitions(
        &registry,
        registry.definitions(),
        &BTreeSet::new(),
    )
    .unwrap();

    assert_eq!(before.stable_revision(), after.stable_revision());
    assert_ne!(before.dynamic_revision(), after.dynamic_revision());
    assert_eq!(
        after
            .stable_definitions()
            .iter()
            .map(|definition| definition.name.as_str())
            .collect::<Vec<_>>(),
        vec!["builtin_stable"]
    );
    assert_eq!(
        after
            .dynamic_definitions()
            .iter()
            .map(|definition| definition.name.as_str())
            .collect::<Vec<_>>(),
        vec!["mcp__fixture__echo_text"]
    );

    let mut changed_descriptor = descriptor(
        "echo_text",
        "mcp__fixture__echo_text",
        json!({"type": "object"}),
    );
    changed_descriptor.provenance.catalog_generation += 1;
    changed_descriptor.provenance.catalog_digest = "c".repeat(64);
    let mut changed_registry = ToolRegistry::empty();
    changed_registry.register_test_tool(TestBuiltinTool {
        name: "builtin_stable",
        description: "stable definition",
    });
    changed_registry.register_mcp_runtime(&McpToolRuntime::capture(MockMcpToolInvoker::returning(
        vec![changed_descriptor],
        empty_result(),
    )));
    let changed = EffectiveToolSet::from_permitted_definitions(
        &changed_registry,
        changed_registry.definitions(),
        &BTreeSet::new(),
    )
    .unwrap();
    assert_eq!(after.stable_revision(), changed.stable_revision());
    assert_ne!(after.dynamic_revision(), changed.dynamic_revision());
}
