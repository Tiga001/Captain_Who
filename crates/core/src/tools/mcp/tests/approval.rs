use super::*;

#[test]
fn invocation_debug_redacts_arguments_while_live_model_projection_retains_them() {
    let model_name = "mcp__fixture__credential_projection";
    let invoker = MockMcpToolInvoker::returning(
        vec![descriptor(
            "credential_projection",
            model_name,
            json!({"type": "object"}),
        )],
        empty_result(),
    );
    let registry = registry_with(invoker);
    let secret = "fixture-secret-that-must-not-appear";
    let call = call(
        model_name,
        json!({
            "query": "safe",
            "api_token": secret,
            "nested": {"password": secret},
        }),
    );

    for projected in [
        registry.trace_call_projection(&call),
        registry.event_call_projection(&call),
        registry.checkpoint_call_projection(&call),
    ] {
        assert_eq!(projected.args, json!({}));
        assert_eq!(projected.reason, None);
        assert!(!serde_json::to_string(&projected).unwrap().contains(secret));
    }
    assert_eq!(registry.model_call_projection(&call), call);
    assert_eq!(
        registry.checkpoint_persistence(model_name),
        AgentToolCallCheckpointPersistence::DeniedMcp
    );

    let raw_result = crate::protocol::AgentToolResult {
        call_id: call.id.clone(),
        tool: model_name.to_string(),
        ok: true,
        result: Some(json!({
            "content": [{"type": "text", "text": secret}],
            "provenance": provenance("credential_projection", model_name),
        })),
        error: None,
        exact_archive_file: None,
    };
    assert!(
        serde_json::to_string(&registry.model_projection(&raw_result))
            .unwrap()
            .contains(secret)
    );
    for durable in [
        registry.trace_projection(&raw_result),
        registry.archive_projection(&raw_result),
        registry.checkpoint_projection(&raw_result),
    ] {
        let rendered = serde_json::to_string(&durable).unwrap();
        assert!(!rendered.contains(secret));
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
    }

    let approval = propose(&registry, model_name, call.args.clone()).unwrap();
    let request = McpToolApprovalRequest {
        approval: approval.clone(),
        arguments: call.args,
        caller: McpToolCatalogContext::default(),
    };
    assert!(!format!("{request:?}").contains(secret));
    let rendered = serde_json::to_string(&AgentProposedAction::McpToolCall {
        approval: Box::new(approval),
    })
    .expect("safe approval serialization");
    assert!(!rendered.contains(secret));
    assert!(!rendered.contains("api_token"));
    assert!(!rendered.contains("password"));
}

#[test]
fn typed_approval_binds_independent_high_entropy_ids_and_a_safe_argument_summary() {
    let model_name = "mcp__fixture__safe_approval";
    let invoker = MockMcpToolInvoker::returning(
        vec![descriptor(
            "safe_approval",
            model_name,
            json!({"type": "object"}),
        )],
        empty_result(),
    );
    let registry = registry_with(invoker.clone());
    let secret = "fixture-value-never-public";
    let private_property = "private_property_name_never_public";
    let arguments = json!({
        (private_property): {
            "nested": [secret, 1, true, null]
        }
    });

    let approval = propose(&registry, model_name, arguments.clone()).unwrap();
    let action_id = uuid::Uuid::parse_str(&approval.identity.action_id).unwrap();
    let invocation_id = uuid::Uuid::parse_str(&approval.identity.invocation_id).unwrap();

    assert_eq!(action_id.get_version(), Some(uuid::Version::Random));
    assert_eq!(invocation_id.get_version(), Some(uuid::Version::Random));
    assert_ne!(approval.identity.action_id, approval.identity.invocation_id);
    assert_ne!(approval.identity.action_id, approval.identity.call_id);
    assert_ne!(approval.identity.invocation_id, approval.identity.call_id);
    assert_eq!(
        approval.identity.arguments_digest,
        mcp_tool_arguments_digest(&arguments).unwrap()
    );
    assert_eq!(approval.call.args, json!({}));
    assert_eq!(approval.call.approval_status, AgentApprovalStatus::Required);
    assert_eq!(approval.summary.arguments.top_level_property_count, 1);
    assert_eq!(approval.summary.arguments.string_value_count, 1);
    assert_eq!(approval.summary.arguments.number_value_count, 1);
    assert_eq!(approval.summary.arguments.boolean_value_count, 1);
    assert_eq!(approval.summary.arguments.null_value_count, 1);
    assert_eq!(
        approval.summary.display_reason.as_deref(),
        Some("Use this MCP tool to complete the current request.")
    );

    let rendered = serde_json::to_string(&approval).unwrap();
    assert!(!rendered.contains(secret));
    assert!(!rendered.contains(private_property));
    assert!(rendered.contains("\"payloadPersistence\":\"process_only\""));
    assert!(!rendered.contains("payloadRef"));
    assert!(!rendered.contains("ciphertext"));
    let mut legacy = serde_json::to_value(&approval).unwrap();
    legacy.as_object_mut().unwrap().remove("payloadPersistence");
    assert_eq!(
        serde_json::from_value::<AgentMcpToolApproval>(legacy)
            .unwrap()
            .payload_persistence,
        AgentMcpApprovalPayloadPersistence::ProcessOnly
    );
    let prepared = invoker.prepared.lock().unwrap();
    assert_eq!(
        prepared
            .get(&approval.identity.invocation_id)
            .expect("raw payload must be sealed before publication")
            .1,
        arguments
    );
}

#[test]
fn host_owned_call_reason_is_displayed_but_never_sealed_or_sent_as_server_arguments() {
    let model_name = "mcp__fixture__reason_projection";
    let invoker = MockMcpToolInvoker::returning(
        vec![descriptor(
            "reason_projection",
            model_name,
            json!({"type": "object"}),
        )],
        empty_result(),
    );
    let registry = registry_with(invoker.clone());
    let approval = propose(
        &registry,
        model_name,
        json!({
            (MCP_CALL_REASON_FIELD): "Read the requested fixture file",
            "path": "fixture.txt",
        }),
    )
    .unwrap();

    assert_eq!(
        approval.summary.display_reason.as_deref(),
        Some("Read the requested fixture file")
    );
    let prepared = invoker.prepared.lock().unwrap();
    assert_eq!(
        prepared
            .get(&approval.identity.invocation_id)
            .expect("prepared arguments")
            .1,
        json!({"path": "fixture.txt"})
    );
}

#[test]
fn host_owned_call_reason_removes_directional_and_invisible_display_controls() {
    let model_name = "mcp__fixture__safe_reason";
    let invoker = MockMcpToolInvoker::returning(
        vec![descriptor(
            "safe_reason",
            model_name,
            json!({"type": "object"}),
        )],
        empty_result(),
    );
    let registry = registry_with(invoker);
    let approval = propose(
        &registry,
        model_name,
        json!({
            (MCP_CALL_REASON_FIELD): "Read\u{202e}the\u{2066}fixture\nfile",
            "path": "fixture.txt",
        }),
    )
    .unwrap();

    assert_eq!(
        approval.summary.display_reason.as_deref(),
        Some("Read the fixture file")
    );
}

#[test]
fn approval_revalidation_rejects_invalid_epoch_or_registry_revision() {
    let model_name = "mcp__fixture__approval_epoch";
    let invoker = MockMcpToolInvoker::returning(
        vec![descriptor(
            "approval_epoch",
            model_name,
            json!({"type": "object"}),
        )],
        empty_result(),
    );
    let registry = registry_with(invoker);
    let arguments = json!({"value": "private"});
    let approval = propose(&registry, model_name, arguments.clone()).unwrap();

    let mut invalid_epoch = approval.clone();
    invalid_epoch.identity.provenance.config_epoch =
        "BF616F04-D3EC-4BD7-825F-731A9F0892F4".to_string();
    assert_eq!(
        validate_mcp_approval_arguments(&invalid_epoch, &arguments)
            .unwrap_err()
            .code(),
        Some("mcp.invalid_approval_identity")
    );

    let mut invalid_revision = approval;
    invalid_revision.identity.provenance.registry_revision = 0;
    assert_eq!(
        validate_mcp_approval_arguments(&invalid_revision, &arguments)
            .unwrap_err()
            .code(),
        Some("mcp.invalid_approval_identity")
    );
}

#[test]
fn argument_digest_is_canonical_and_summary_omits_property_names() {
    assert_eq!(
        mcp_tool_arguments_digest(&json!({"a": 1, "b": {"x": 2}})).unwrap(),
        mcp_tool_arguments_digest(&json!({"b": {"x": 2}, "a": 1})).unwrap()
    );
    assert_ne!(
        mcp_tool_arguments_digest(&json!({"a": 1})).unwrap(),
        mcp_tool_arguments_digest(&json!({"a": 2})).unwrap()
    );

    let model_name = "mcp__fixture__deep_summary";
    let invoker = MockMcpToolInvoker::returning(
        vec![descriptor(
            "deep_summary",
            model_name,
            json!({"type": "object"}),
        )],
        empty_result(),
    );
    let registry = registry_with(invoker);
    let private_property = "depth_private_property_never_public";
    let private_value = "depth_private_value_never_public";
    let approval = propose(
        &registry,
        model_name,
        json!({"root": {(private_property): private_value}}),
    )
    .unwrap();

    assert!(!approval.summary.arguments.truncated);
    let rendered = serde_json::to_string(&approval).unwrap();
    assert!(!rendered.contains(private_property));
    assert!(!rendered.contains(private_value));
}

#[test]
fn argument_digest_rejects_untrusted_json_over_host_limits() {
    let mut too_deep = Value::Null;
    for _ in 0..MAX_MCP_ARGUMENT_DEPTH {
        too_deep = json!({"nested": too_deep});
    }
    let depth_error = mcp_tool_arguments_digest(&json!({"root": too_deep}))
        .expect_err("deep arguments must fail before recursive canonicalization");
    assert_eq!(depth_error.code(), Some("mcp.arguments_limit_exceeded"));

    let too_many_nodes = Value::Array((0..MAX_MCP_ARGUMENT_NODES).map(|_| Value::Null).collect());
    let nodes_error = mcp_tool_arguments_digest(&json!({"items": too_many_nodes}))
        .expect_err("oversized argument trees must fail");
    assert_eq!(nodes_error.code(), Some("mcp.arguments_limit_exceeded"));

    let too_many_properties = Value::Object(
        (0..=MAX_MCP_ARGUMENT_OBJECT_PROPERTIES)
            .map(|index| (format!("field_{index}"), Value::Null))
            .collect(),
    );
    let properties_error = mcp_tool_arguments_digest(&too_many_properties)
        .expect_err("wide argument objects must fail");
    assert_eq!(
        properties_error.code(),
        Some("mcp.arguments_limit_exceeded")
    );

    let bytes_error = mcp_tool_arguments_digest(&json!({
        "value": "x".repeat(MAX_MCP_RAW_ARGUMENT_BYTES)
    }))
    .expect_err("oversized encoded arguments must fail");
    assert_eq!(bytes_error.code(), Some("mcp.arguments_limit_exceeded"));
}

#[tokio::test]
async fn prepared_invocation_is_consumed_once_and_invalidation_discards_it() {
    let model_name = "mcp__fixture__one_time";
    let invoker = MockMcpToolInvoker::returning(
        vec![descriptor(
            "one_time",
            model_name,
            json!({"type": "object"}),
        )],
        empty_result(),
    );
    let registry = registry_with(invoker.clone());
    let approval = propose(&registry, model_name, json!({"value": 1})).unwrap();

    invoke_prepared(
        invoker.as_ref(),
        approval.clone(),
        AgentCancellationToken::new(),
    )
    .await
    .unwrap();
    let replay = invoke_prepared(invoker.as_ref(), approval, AgentCancellationToken::new())
        .await
        .expect_err("a consumed invocation grant must not replay");
    assert!(replay.to_string().contains("already consumed"));

    let invalidated = propose(&registry, model_name, json!({"value": 2})).unwrap();
    registry
        .invalidate_proposed_action(&AgentProposedAction::McpToolCall {
            approval: Box::new(invalidated.clone()),
        })
        .unwrap();
    assert!(!invoker
        .prepared
        .lock()
        .unwrap()
        .contains_key(&invalidated.identity.invocation_id));
    assert_eq!(
        invoker.invalidated.lock().unwrap().last(),
        Some(&invalidated.identity)
    );
}

#[test]
fn preparation_defaults_fail_closed_and_only_allows_host_to_freeze_payload_persistence() {
    struct CatalogOnlyInvoker {
        catalog: Vec<McpAgentToolDescriptor>,
    }
    impl McpToolInvoker for CatalogOnlyInvoker {
        fn catalog(
            &self,
            _context: &McpToolCatalogContext,
        ) -> AgentResult<Vec<McpAgentToolDescriptor>> {
            Ok(self.catalog.clone())
        }
    }

    let model_name = "mcp__fixture__fail_closed";
    let invoker: Arc<dyn McpToolInvoker> = Arc::new(CatalogOnlyInvoker {
        catalog: vec![descriptor(
            "fail_closed",
            model_name,
            json!({"type": "object"}),
        )],
    });
    let runtime = McpToolRuntime::capture(invoker);
    let mut registry = ToolRegistry::empty();
    registry.register_mcp_runtime(&runtime);
    let error = propose(&registry, model_name, json!({}))
        .expect_err("an unavailable preparation Host must fail closed");
    assert_eq!(error.code(), Some("mcp.approval_host_unavailable"));

    struct PersistenceFreezingInvoker {
        catalog: Vec<McpAgentToolDescriptor>,
    }
    impl McpToolInvoker for PersistenceFreezingInvoker {
        fn catalog(
            &self,
            _context: &McpToolCatalogContext,
        ) -> AgentResult<Vec<McpAgentToolDescriptor>> {
            Ok(self.catalog.clone())
        }

        fn prepare_approval(
            &self,
            request: McpToolApprovalRequest,
        ) -> AgentResult<AgentMcpToolApproval> {
            let (mut approval, _, _) = request.into_parts();
            approval.payload_persistence =
                AgentMcpApprovalPayloadPersistence::DurableAuthenticatedEnvelope;
            Ok(approval)
        }
    }
    let durable_tool_name = "mcp__fixture__durable_payload";
    let durable: Arc<dyn McpToolInvoker> = Arc::new(PersistenceFreezingInvoker {
        catalog: vec![descriptor(
            "durable_payload",
            durable_tool_name,
            json!({"type": "object"}),
        )],
    });
    let runtime = McpToolRuntime::capture(durable);
    let mut registry = ToolRegistry::empty();
    registry.register_mcp_runtime(&runtime);
    let prepared = propose(&registry, durable_tool_name, json!({}))
        .expect("the Host may freeze only the actual payload persistence capability");
    assert_eq!(
        prepared.payload_persistence,
        AgentMcpApprovalPayloadPersistence::DurableAuthenticatedEnvelope
    );
    assert!(serde_json::to_string(&prepared)
        .unwrap()
        .contains("\"payloadPersistence\":\"durable_authenticated_envelope\""));

    struct MutatingInvoker {
        catalog: Vec<McpAgentToolDescriptor>,
        invalidated: Mutex<Vec<AgentMcpToolInvocationIdentity>>,
    }
    impl McpToolInvoker for MutatingInvoker {
        fn catalog(
            &self,
            _context: &McpToolCatalogContext,
        ) -> AgentResult<Vec<McpAgentToolDescriptor>> {
            Ok(self.catalog.clone())
        }

        fn prepare_approval(
            &self,
            request: McpToolApprovalRequest,
        ) -> AgentResult<AgentMcpToolApproval> {
            let (mut approval, _, _) = request.into_parts();
            approval.summary.server_display_name = "changed".to_string();
            Ok(approval)
        }

        fn invalidate_prepared_approval(
            &self,
            identity: &AgentMcpToolInvocationIdentity,
        ) -> AgentResult<()> {
            self.invalidated.lock().unwrap().push(identity.clone());
            Ok(())
        }
    }
    let mutating = Arc::new(MutatingInvoker {
        catalog: vec![descriptor(
            "mutating_host",
            "mcp__fixture__mutating_host",
            json!({"type": "object"}),
        )],
        invalidated: Mutex::new(Vec::new()),
    });
    let runtime = McpToolRuntime::capture(mutating.clone());
    let mut registry = ToolRegistry::empty();
    registry.register_mcp_runtime(&runtime);
    let error = propose(&registry, "mcp__fixture__mutating_host", json!({}))
        .expect_err("a Host cannot rewrite the frozen safe approval");
    assert_eq!(error.code(), Some("mcp.approval_binding_changed"));
    assert!(!mutating.invalidated.lock().unwrap().is_empty());
}
