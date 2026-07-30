use std::collections::BTreeSet;

use serde_json::Value;

use super::*;

#[test]
fn renderer_safe_management_dtos_have_no_secret_bearing_field_names() {
    let mut cursor_catalog = McpCatalogSnapshot::empty(McpServerId::new());
    cursor_catalog.generation = 1;
    let cursor = encode_catalog_cursor(&cursor_catalog, 0).expect("encode Host cursor");
    let examples = [
        serde_json::to_value(McpServerCreateInput {
            schema_version: MCP_MANAGEMENT_SCHEMA_VERSION,
            display_name: "fixture".to_string(),
            transport: McpTransportKindDto::Stdio,
            executable: "/usr/bin/false".to_string(),
            arguments: Vec::new(),
            cwd: "/tmp".to_string(),
            approval_mode: McpApprovalModeDto::Prompt,
        })
        .expect("serialize create input"),
        serde_json::to_value(McpCatalogToolsPageInput {
            schema_version: MCP_MANAGEMENT_SCHEMA_VERSION,
            server_id: cursor_catalog.server_id.to_string(),
            cursor: Some(cursor),
            limit: 25,
        })
        .expect("serialize Catalog input"),
        serde_json::to_value(McpManagementErrorData {
            schema_version: MCP_MANAGEMENT_SCHEMA_VERSION,
            error_type: McpManagementErrorTypeDto::McpManagement,
            operation: McpManagementOperationDto::Get,
            code: McpManagementErrorCodeDto::InvalidInput,
            recovery: McpManagementRecoveryDto::FixInput,
            message: "safe test error".to_string(),
            server_id: None,
            current_registry_revision: Some(1),
        })
        .expect("serialize management error"),
    ];
    for example in examples {
        assert_renderer_safe_json(&example);
    }

    let forbidden_keys = [
        "environment",
        "env",
        "secretRef",
        "token",
        "header",
        "headers",
        "ciphertext",
    ]
    .into_iter()
    .map(str::to_string)
    .collect::<BTreeSet<_>>();
    for key in forbidden_keys {
        let mut untrusted =
            serde_json::to_value(create_input("fixture")).expect("serialize create input");
        untrusted
            .as_object_mut()
            .expect("create input object")
            .insert(key, Value::String("MCP_TEST_SECRET_CANARY".to_string()));
        assert!(
            serde_json::from_value::<McpServerCreateInput>(untrusted).is_err(),
            "strict DTO parser accepted a forbidden extra field"
        );
    }
}

#[test]
fn renderer_text_projection_replaces_directional_controls_and_marks_the_change() {
    let (projected, changed) = truncate_text("safe\u{202e}spoof", MAX_SAFE_DESCRIPTION_BYTES);
    assert_eq!(projected, "safe\u{fffd}spoof");
    assert!(changed);
    assert!(validate_editable_fields(
        "unsafe\u{2066}name",
        "/usr/bin/false",
        &[],
        "/tmp",
        McpManagementOperationDto::Add,
    )
    .is_err());
}

#[test]
fn renderer_catalog_projection_replaces_controls_before_serialization() {
    let (description, description_truncated) = truncate_text(
        "safe\n\t\u{00ad}\u{061c}\u{2028}\u{202e}\u{2066}description",
        MAX_SAFE_DESCRIPTION_BYTES,
    );
    let tool = McpToolSummaryView {
        server_id: McpServerId::new().to_string(),
        raw_name: bounded_text("raw\u{2066}name", 1024),
        model_name: bounded_text("model\u{0007}name", 64),
        routable: true,
        disabled: false,
        schema_digest_prefix: "0123456789ab".to_string(),
        description,
        description_truncated,
        diagnostic_codes: Vec::new(),
        catalog_generation: 1,
        catalog_completeness: McpCatalogCompletenessDto::Complete,
    };

    assert!(tool.description_truncated);
    for projected in [&tool.raw_name, &tool.model_name, &tool.description] {
        assert!(
            !projected.chars().any(is_unsafe_renderer_text_character),
            "catalog projection retained an unsafe display character"
        );
        assert!(projected.contains('\u{fffd}'));
    }
    assert_eq!(safe_error_code("unsafe code\nvalue"), "mcp.serverError");
    assert_eq!(safe_error_code("mcp.server_error-1"), "mcp.server_error-1");
    assert_renderer_safe_json(
        &serde_json::to_value(tool).expect("serialize safe Catalog projection"),
    );
}
