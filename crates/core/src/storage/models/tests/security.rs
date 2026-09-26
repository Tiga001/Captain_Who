use crate::provider_profile::{ProviderProfileConfig, ProviderProtocolDialect};

use super::*;

const CANARY: &str = "STORAGE_DEBUG_SECRET_CANARY";

#[test]
fn sensitive_agent_storage_records_have_constant_redacted_debug() {
    let audit = AgentActionAuditRecord {
        action_id: CANARY.to_string(),
        run_id: CANARY.to_string(),
        conversation_id: Some(CANARY.to_string()),
        assistant_message_id: Some(CANARY.to_string()),
        action_type: CANARY.to_string(),
        tool_name: CANARY.to_string(),
        decision: Some(CANARY.to_string()),
        status: CANARY.to_string(),
        action_json: CANARY.to_string(),
        file_change_result_json: Some(CANARY.to_string()),
        command_result_json: Some(CANARY.to_string()),
        tool_result_json: Some(CANARY.to_string()),
        error: Some(CANARY.to_string()),
        created_at: 1,
        decided_at: Some(2),
        completed_at: Some(3),
        effective_permissions_json: Some(CANARY.to_string()),
        path_scope: Some(CANARY.to_string()),
        command_cwd_scope: Some(CANARY.to_string()),
        blocked_reason: Some(CANARY.to_string()),
        decision_source: Some(CANARY.to_string()),
    };
    let pending = AgentPendingActionRecord {
        action_id: CANARY.to_string(),
        run_id: CANARY.to_string(),
        conversation_id: Some(CANARY.to_string()),
        assistant_message_id: Some(CANARY.to_string()),
        action_type: CANARY.to_string(),
        tool_name: CANARY.to_string(),
        tool_call_id: Some(CANARY.to_string()),
        status: CANARY.to_string(),
        target_status: Some(CANARY.to_string()),
        action_json: CANARY.to_string(),
        agent_input_json: CANARY.to_string(),
        created_at: 1,
        updated_at: 2,
    };
    let file_change = AgentFileChangeRecord {
        schema_version: 1,
        id: CANARY.to_string(),
        conversation_id: CANARY.to_string(),
        project_id: Some(CANARY.to_string()),
        run_id: CANARY.to_string(),
        source_tool_name: "apply_patch".to_string(),
        source_tool_call_id: CANARY.to_string(),
        source_tool_arguments_digest: CANARY.to_string(),
        permission_revision: CANARY.to_string(),
        tool_set_revision: CANARY.to_string(),
        provider_wire_revision: CANARY.to_string(),
        observation_id: CANARY.to_string(),
        observation_json: CANARY.to_string(),
        file_path: CANARY.to_string(),
        operation: "create".to_string(),
        strategy: None,
        status: "drafting".to_string(),
        base_revision: None,
        base_content: CANARY.to_string(),
        content: CANARY.to_string(),
        draft_revision: 0,
        next_mutation_index: 0,
        additions: 0,
        deletions: 0,
        line_count: 1,
        byte_count: CANARY.len() as u64,
        mutation_count: 0,
        stats_final: false,
        summary: Some(CANARY.to_string()),
        final_action_id: None,
        final_action_arguments_digest: None,
        final_permission_revision: None,
        final_tool_set_revision: None,
        final_provider_wire_revision: None,
        created_at: 1,
        updated_at: 2,
        expires_at: 3,
    };

    assert_eq!(format!("{audit:?}"), "AgentActionAuditRecord([REDACTED])");
    assert_eq!(
        format!("{pending:?}"),
        "AgentPendingActionRecord([REDACTED])"
    );
    assert_eq!(
        format!("{file_change:?}"),
        "AgentFileChangeRecord([REDACTED])"
    );
    assert!(!format!("{audit:?}{pending:?}{file_change:?}").contains(CANARY));
}

#[test]
fn mcp_envelope_record_debug_never_exposes_ciphertext_or_identity() {
    let envelope = McpApprovalEnvelopeRecord {
        invocation_id: CANARY.to_string(),
        action_id: CANARY.to_string(),
        envelope_version: 1,
        nonce_base64: CANARY.to_string(),
        ciphertext_base64: CANARY.to_string(),
        aad_digest: CANARY.to_string(),
        created_at: 1,
        expires_at: 2,
    };

    let rendered = format!("{envelope:?}");
    assert_eq!(rendered, "McpApprovalEnvelopeRecord([REDACTED])");
    assert!(!rendered.contains(CANARY));
}

#[test]
fn model_configuration_debug_is_write_only_for_credentials() {
    let model = ModelConfigRecord {
        id: "test-model".to_string(),
        provider_model_id: "test-model".to_string(),
        display_name: "Test Model".to_string(),
        api_url_override: Some("https://example.test/v1".to_string()),
        api_token_override: Some(CANARY.to_string()),
        supports_image: false,
        context_window_tokens: Some(128_000),
        provider_profile_config: ProviderProfileConfig::generic_for_dialect(
            ProviderProtocolDialect::OpenAiChatCompletions,
        ),
        input_price: "0".to_string(),
        cached_input_price: String::new(),
        output_price: "0".to_string(),
        enabled: true,
    };
    let settings = ModelSettingsRecord {
        api_url: "https://example.test/v1".to_string(),
        api_token: CANARY.to_string(),
        search_mode: "tavily".to_string(),
        tavily_api_key: CANARY.to_string(),
        models: vec![model.clone()],
    };
    let connection = ModelConnectionConfig {
        api_url: "https://example.test/v1".to_string(),
        api_token: CANARY.to_string(),
    };

    let rendered = format!("{model:?}{settings:?}{connection:?}");
    assert!(!rendered.contains(CANARY));
    assert!(!rendered.contains("example.test"));
}
