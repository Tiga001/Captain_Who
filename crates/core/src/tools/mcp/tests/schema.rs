use super::*;

#[test]
fn invalid_and_non_object_schemas_are_isolated_from_valid_catalog_entries() {
    let valid_name = "mcp__fixture__valid";
    let invoker = MockMcpToolInvoker::returning(
        vec![
            descriptor(
                "array_root",
                "mcp__fixture__array_root",
                json!({"type": "array", "items": {"type": "string"}}),
            ),
            descriptor(
                "unsupported_root_keyword",
                "mcp__fixture__unsupported_root_keyword",
                json!({
                    "type": "object",
                    "properties": {},
                    "anyOf": [{"required": []}]
                }),
            ),
            descriptor(
                "credential_schema",
                "mcp__fixture__credential_schema",
                json!({
                    "type": "object",
                    "properties": {"api_token": {"type": "string"}}
                }),
            ),
            descriptor("valid", valid_name, json!({"type": "object"})),
        ],
        empty_result(),
    );

    let registry = registry_with(invoker);

    assert!(registry.definition_for(valid_name).is_some());
    assert!(registry
        .definition_for("mcp__fixture__array_root")
        .is_none());
    assert!(registry
        .definition_for("mcp__fixture__unsupported_root_keyword")
        .is_none());
    assert_eq!(
        registry
            .mcp_diagnostics()
            .iter()
            .map(|diagnostic| diagnostic.code)
            .collect::<Vec<_>>(),
        vec![
            McpToolDiagnosticCode::InvalidSchema,
            McpToolDiagnosticCode::InvalidSchema,
            McpToolDiagnosticCode::InvalidSchema,
        ]
    );
}

#[test]
fn missing_root_type_is_normalized_to_object() {
    let model_name = "mcp__fixture__implicit_object";
    let registry = registry_with(MockMcpToolInvoker::returning(
        vec![descriptor(
            "implicit_object",
            model_name,
            json!({
                "properties": {"value": {"type": "integer"}},
                "required": ["value"],
                "additionalProperties": false
            }),
        )],
        empty_result(),
    ));

    let definition = registry.definition_for(model_name).unwrap();
    assert_eq!(definition.input_schema["type"], "object");
    assert_eq!(
        definition.input_schema["required"],
        json!(["value", "call_reason"])
    );
    assert!(registry.mcp_diagnostics().is_empty());
}

#[test]
fn server_defined_call_reason_is_isolated_as_a_reserved_host_field() {
    let model_name = "mcp__fixture__reserved_call_reason";
    let registry = registry_with(MockMcpToolInvoker::returning(
        vec![descriptor(
            "reserved_call_reason",
            model_name,
            json!({
                "type": "object",
                "properties": {
                    "call_reason": {"type": "string"}
                }
            }),
        )],
        empty_result(),
    ));

    assert!(registry.definition_for(model_name).is_none());
    assert_eq!(
        registry
            .mcp_diagnostics()
            .iter()
            .map(|diagnostic| diagnostic.code)
            .collect::<Vec<_>>(),
        vec![McpToolDiagnosticCode::InvalidSchema]
    );
}

#[test]
fn normalized_schema_identity_is_distinct_from_raw_catalog_identity() {
    let model_name = "mcp__fixture__schema_identity";
    let input_schema = json!({
        "properties": {"value": {"type": "integer"}},
        "required": ["value"],
        "additionalProperties": false
    });
    let registry = registry_with(MockMcpToolInvoker::returning(
        vec![descriptor(
            "schema_identity",
            model_name,
            input_schema.clone(),
        )],
        empty_result(),
    ));
    let definition = registry.definition_for(model_name).unwrap();
    let expected =
        mcp_normalized_input_schema_identity(model_name, &input_schema).expect("valid schema");
    let Some(AgentToolIdentity::Mcp { provenance }) = registry.identity(model_name) else {
        panic!("expected typed MCP identity");
    };

    assert_eq!(definition.input_schema["type"], "object");
    assert_eq!(provenance.catalog_schema_digest, "c".repeat(64));
    assert_eq!(provenance.schema_digest, expected.schema_digest);
    assert_eq!(
        provenance.schema_normalizer_version,
        MCP_INPUT_SCHEMA_NORMALIZER_VERSION
    );
    assert_ne!(provenance.schema_digest, provenance.catalog_schema_digest);

    let explicit = mcp_normalized_input_schema_identity(
        model_name,
        &json!({
            "type": "object",
            "properties": {"value": {"type": "integer"}},
            "required": ["value"],
            "additionalProperties": false
        }),
    )
    .unwrap();
    assert_eq!(expected, explicit);
}

#[test]
fn provider_description_truncation_is_utf8_safe_and_marked() {
    let model_name = "mcp__fixture__long_description";
    let mut tool = descriptor("long_description", model_name, json!({"type": "object"}));
    tool.description = Some("界".repeat(MAX_MCP_DESCRIPTION_BYTES));
    let registry = registry_with(MockMcpToolInvoker::returning(vec![tool], empty_result()));
    let description = &registry.definition_for(model_name).unwrap().description;

    assert!(description.len() <= MAX_MCP_DESCRIPTION_BYTES);
    assert!(description.ends_with(MCP_DESCRIPTION_TRUNCATION_MARKER));
    assert!(std::str::from_utf8(description.as_bytes()).is_ok());
}

#[test]
fn server_hints_only_classify_risk_and_never_bypass_per_invocation_approval() {
    let model_name = "mcp__fixture__mutating";
    let mut untrusted = descriptor("mutating", model_name, json!({"type": "object"}));
    untrusted.annotations.read_only_hint = None;
    let server_claim_only = descriptor(
        "server_claim_only",
        "mcp__fixture__server_claim_only",
        json!({"type": "object"}),
    );
    let mut destructive = descriptor(
        "destructive",
        "mcp__fixture__destructive",
        json!({"type": "object"}),
    );
    destructive.annotations.destructive_hint = Some(true);
    let registry = registry_with(MockMcpToolInvoker::returning(
        vec![untrusted, server_claim_only, destructive],
        empty_result(),
    ));

    for tool in [
        model_name,
        "mcp__fixture__server_claim_only",
        "mcp__fixture__destructive",
    ] {
        let definition = registry.definition_for(tool).expect("MCP definition");
        assert!(definition.requires_approval);
        assert_eq!(definition.approval_mode, AgentToolApprovalMode::Always);
    }
    assert_eq!(
        propose(&registry, model_name, json!({}))
            .unwrap()
            .summary
            .risk,
        AgentMcpToolRisk::Unknown
    );
    assert_eq!(
        propose(&registry, "mcp__fixture__server_claim_only", json!({}))
            .unwrap()
            .summary
            .risk,
        AgentMcpToolRisk::ReadOnlyClaimed
    );
    assert_eq!(
        propose(&registry, "mcp__fixture__destructive", json!({}))
            .unwrap()
            .summary
            .risk,
        AgentMcpToolRisk::DestructiveClaimed
    );
    assert!(registry.mcp_diagnostics().is_empty());
}
