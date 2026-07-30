use super::*;

#[test]
fn catalog_descriptor_registers_provider_definition_and_typed_identity() {
    let model_name = "mcp__fixture__echo_text";
    let input_schema = json!({
        "type": "object",
        "properties": {"text": {"type": "string"}},
        "required": ["text"],
        "additionalProperties": false
    });
    let expected_provenance = provenance_for_schema("echo_text", model_name, &input_schema);
    let invoker = MockMcpToolInvoker::returning(
        vec![descriptor("echo_text", model_name, input_schema)],
        empty_result(),
    );

    let registry = registry_with(invoker);
    let definition = registry
        .definition_for(model_name)
        .expect("MCP tool definition");

    assert_eq!(definition.name, model_name);
    assert!(definition.description.starts_with(MCP_DESCRIPTION_PREFIX));
    assert!(definition.description.ends_with("echo_text fixture tool"));
    assert_eq!(definition.input_schema["type"], "object");
    assert_eq!(definition.input_schema["required"], json!(["text"]));
    assert_eq!(definition.safety, AgentToolSafety::RequiresApproval);
    assert!(definition.requires_approval);
    assert_eq!(definition.approval_mode, AgentToolApprovalMode::Always);
    assert_eq!(
        registry.identity(model_name),
        Some(&AgentToolIdentity::Mcp {
            provenance: expected_provenance,
        })
    );
    assert_eq!(
        registry.exposure(model_name),
        Some(&AgentToolExposure::Dynamic)
    );
    assert!(registry.is_mcp_tool(model_name));
    assert!(
        registry
            .renderer_event_definitions(&registry.definitions())
            .iter()
            .all(|definition| definition.name != model_name),
        "untrusted MCP descriptions and schemas must not enter generic Renderer events"
    );
    assert!(
        registry
            .definitions()
            .iter()
            .any(|definition| definition.name == model_name),
        "the provider-facing registry must retain the MCP definition"
    );
}

#[test]
fn config_epoch_and_registry_revision_are_frozen_serialized_identity() {
    let provenance = provenance("echo_text", "mcp__fixture__epoch_identity");
    let encoded = serde_json::to_value(AgentToolIdentity::Mcp {
        provenance: provenance.clone(),
    })
    .expect("typed provenance must serialize");

    assert_eq!(
        encoded["provenance"]["configEpoch"],
        provenance.config_epoch
    );
    assert_eq!(
        encoded["provenance"]["registryRevision"],
        provenance.registry_revision
    );
    assert!(valid_canonical_uuid_v4(&provenance.config_epoch));
    assert!(provenance.registry_revision > 0);
}

#[test]
fn config_epoch_prevents_aba_identity_reuse_with_the_same_digest() {
    let model_name = "mcp__fixture__aba";
    let first = descriptor("aba", model_name, json!({"type": "object"}));
    let mut returned_to_same_config = first.clone();
    returned_to_same_config.provenance.config_epoch =
        "18df6537-76db-41c6-8058-926ea1fb7509".to_string();
    returned_to_same_config.provenance.registry_revision = first.provenance.registry_revision + 2;

    assert_eq!(
        first.provenance.config_digest, returned_to_same_config.provenance.config_digest,
        "the ABA fixture deliberately returns to the same normalized configuration"
    );
    assert_ne!(
        first.provenance, returned_to_same_config.provenance,
        "a new configuration instance must never reuse an old approval identity"
    );

    let first_registry = registry_with(MockMcpToolInvoker::returning(vec![first], empty_result()));
    let returned_registry = registry_with(MockMcpToolInvoker::returning(
        vec![returned_to_same_config],
        empty_result(),
    ));
    let first_set = EffectiveToolSet::from_permitted_definitions(
        &first_registry,
        first_registry.definitions(),
        &BTreeSet::new(),
    )
    .unwrap();
    let returned_set = EffectiveToolSet::from_permitted_definitions(
        &returned_registry,
        returned_registry.definitions(),
        &BTreeSet::new(),
    )
    .unwrap();
    assert_ne!(
        first_set.dynamic_revision(),
        returned_set.dynamic_revision()
    );
}

#[test]
fn invalid_or_noncanonical_config_epoch_and_zero_revision_fail_closed() {
    let model_name = "mcp__fixture__invalid_epoch";
    let invalid_epochs = [
        String::new(),
        "bf616f04-d3ec-1bd7-825f-731a9f0892f4".to_string(),
        "BF616F04-D3EC-4BD7-825F-731A9F0892F4".to_string(),
    ];
    for invalid_epoch in invalid_epochs {
        let mut invalid = descriptor("invalid_epoch", model_name, json!({"type": "object"}));
        invalid.provenance.config_epoch = invalid_epoch;
        let registry = registry_with(MockMcpToolInvoker::returning(vec![invalid], empty_result()));
        assert!(registry.definition_for(model_name).is_none());
        assert_eq!(
            registry
                .mcp_diagnostics()
                .iter()
                .map(|diagnostic| diagnostic.code)
                .collect::<Vec<_>>(),
            vec![McpToolDiagnosticCode::InvalidIdentity]
        );
    }

    let mut invalid_revision = descriptor("invalid_epoch", model_name, json!({"type": "object"}));
    invalid_revision.provenance.registry_revision = 0;
    let registry = registry_with(MockMcpToolInvoker::returning(
        vec![invalid_revision],
        empty_result(),
    ));
    assert!(registry.definition_for(model_name).is_none());
    assert_eq!(
        registry.mcp_diagnostics()[0].code,
        McpToolDiagnosticCode::InvalidIdentity
    );
}

#[test]
fn runtime_catalog_budget_disables_all_mcp_tools_without_affecting_builtins() {
    let too_many = (0..=MCP_RUNTIME_MAX_TOOL_DEFINITIONS)
        .map(|index| {
            descriptor(
                &format!("raw_{index}"),
                &format!("mcp__fixture__tool_{index}"),
                json!({"type": "object"}),
            )
        })
        .collect();
    let invoker = MockMcpToolInvoker::returning(too_many, empty_result());
    let runtime = McpToolRuntime::capture(invoker.clone());
    assert!(runtime.is_empty());
    assert_eq!(
        runtime.initial_diagnostics(),
        &[McpToolRegistrationDiagnostic::global(
            McpToolDiagnosticCode::CatalogBudgetExceeded
        )]
    );

    let oversized_schema = descriptor(
        "oversized",
        "mcp__fixture__oversized",
        json!({
            "type": "object",
            "description": "x".repeat(MCP_RUNTIME_MAX_CATALOG_BYTES + 1)
        }),
    );
    let oversized_invoker = MockMcpToolInvoker::returning(vec![oversized_schema], empty_result());
    let oversized_runtime = McpToolRuntime::capture(oversized_invoker);
    assert!(oversized_runtime.is_empty());
    assert_eq!(
        oversized_runtime.initial_diagnostics()[0].code,
        McpToolDiagnosticCode::CatalogBudgetExceeded
    );

    let mut registry = ToolRegistry::empty();
    registry.register_test_tool(TestBuiltinTool {
        name: "trusted_builtin",
        description: "trusted builtin",
    });
    registry.register_mcp_runtime(&runtime);
    assert!(registry.definition_for("trusted_builtin").is_some());
    assert_eq!(registry.definitions().len(), 1);
}

#[test]
fn mcp_name_collision_cannot_replace_builtin_identity_or_definition() {
    let model_name = "mcp__fixture__echo_text";
    let mut registry = ToolRegistry::empty();
    registry.register_test_tool(TestBuiltinTool {
        name: model_name,
        description: "trusted builtin",
    });
    let invoker = MockMcpToolInvoker::returning(
        vec![descriptor(
            "echo_text",
            model_name,
            json!({"type": "object"}),
        )],
        empty_result(),
    );
    let runtime = McpToolRuntime::capture(invoker.clone());

    registry.register_mcp_runtime(&runtime);

    assert_eq!(
        registry.definition_for(model_name).unwrap().description,
        "trusted builtin"
    );
    assert_eq!(
        registry.identity(model_name),
        Some(&AgentToolIdentity::Builtin {
            tool_name: model_name.to_string(),
        })
    );
    assert!(
        !registry.is_mcp_tool(model_name),
        "an mcp__-looking name must not be parsed as MCP authority"
    );
    assert!(
        registry
            .renderer_event_definitions(&registry.definitions())
            .iter()
            .any(|definition| definition.name == model_name),
        "typed built-in identity remains visible even when its name resembles an MCP namespace"
    );
    assert_eq!(
        registry.mcp_diagnostics(),
        &[McpToolRegistrationDiagnostic::for_tool(
            &provenance("echo_text", model_name),
            McpToolDiagnosticCode::NameCollision,
        )]
    );
    assert_eq!(
        *invoker.diagnostics.lock().unwrap(),
        registry.mcp_diagnostics()
    );
}
