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
    assert_eq!(
        trace_projection.result.as_ref().unwrap()["provenance"],
        serde_json::to_value(&expected_provenance).unwrap()
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
    assert!(result.error.unwrap().contains("MCP server reported"));
    let value = result.result.expect("structured MCP error result");
    assert_eq!(value["isError"], true);
    assert_eq!(value["content"][0]["text"], "fixture rejected the request");
    assert_eq!(value["structuredContent"]["reason"], "fixture_error");
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
