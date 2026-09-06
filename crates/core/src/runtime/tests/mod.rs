// Tests for runtime message construction and tool-flow helpers.
use super::*;
use crate::llm::LlmToolCall;
use crate::protocol::{
    AgentActivatedSkill, AgentAttachmentLibraryContext, AgentAttachmentReference,
    AgentInputAttachment, AgentInputAttachmentEncoding, AgentInputAttachmentKind,
    AgentPatchPermission, AgentRunContext, AgentSearchConfig, AgentSearchMode,
    AgentSkillActivation, AgentWorkspaceContext,
};
use crate::runtime::tool_flow::parse_tool_call_request;
use crate::tools::{EffectiveToolSet, ToolCapabilityId, OFFICE_DOCUMENTS_CAPABILITY};
use crate::{
    AnchoredWorldStateRecord, ConversationTraceToolResultStatus, ConversationTurnTrace,
    ConversationTurnTraceItem, ConversationTurnTraceTerminalStatus, WorldStateLifetime,
    WorldStateRecord, WorldStateSectionEnvelope, WorldStateSectionId, WorldStateSnapshot,
    CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
};
use base64::Engine;
use std::collections::BTreeSet;

fn message(role: &str, content: &str) -> AgentChatMessage {
    AgentChatMessage {
        message_id: None,
        role: role.to_string(),
        content: content.to_string(),
        created_at: None,
        conversation_turn_trace: None,
        conversation_model_context_items: Vec::new(),
    }
}

fn empty_attachment_context() -> AttachmentContext {
    AttachmentContext {
        text: String::new(),
        images: Vec::new(),
    }
}

fn assert_runtime_owned_tool_call_id(id: &str) {
    assert!(id.starts_with("tc1_"), "unexpected tool-call ID: {id}");
    assert_eq!(id.len(), 47, "unexpected canonical ID length: {id}");
    assert!(
        id.bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')),
        "tool-call ID contains provider-unsafe characters: {id}"
    );
}

fn activated_skill(instructions: &str) -> AgentSkillActivation {
    AgentSkillActivation {
        activation_revision: "activation-sha256-v1:test".to_string(),
        skills: vec![AgentActivatedSkill {
            id: "workspace:workspace-1:review".to_string(),
            name: "repository-review".to_string(),
            revision: activated_skill_revision(),
            source: "workspace:workspace-1".to_string(),
            instructions: instructions.to_string(),
            source_bytes: u64::try_from(instructions.len()).unwrap(),
            resources: None,
        }],
    }
}

fn activated_skill_revision() -> String {
    format!("skill-package-sha256-v3:{}", "d".repeat(64))
}

fn activated_skill_authority() -> Arc<crate::skills::SkillResourceSession> {
    let skill_id = crate::skills::SkillId::parse("workspace:workspace-1:review").unwrap();
    let source_id = skill_id.source_id().clone();
    Arc::new(
        crate::skills::memory_resource_session_for_test(
            skill_id,
            crate::skills::SkillRevision::parse(activated_skill_revision()).unwrap(),
            source_id,
            Vec::new(),
        )
        .unwrap(),
    )
}

fn discoverable_skill(description: &str) -> crate::skills::AgentSkillDiscoverySnapshot {
    const CATALOG_REVISION: &str = "skill-enabled-catalog-sha256-v1:test";
    const SKILL_ID: &str = "bundled:application:documents";
    let revision = format!("skill-package-sha256-v2:{}", "d".repeat(64));
    crate::skills::AgentSkillDiscoverySnapshot {
        schema_version: crate::skills::AGENT_SKILL_DISCOVERY_SCHEMA_VERSION,
        catalog_revision: CATALOG_REVISION.to_string(),
        prompt_token_budget: crate::skills::DEFAULT_SKILL_DISCOVERY_PROMPT_TOKENS,
        skills: vec![crate::skills::AgentDiscoverableSkill {
            activation_ref: crate::skills::derive_skill_activation_ref(
                CATALOG_REVISION,
                SKILL_ID,
                &revision,
            ),
            id: SKILL_ID.to_string(),
            revision,
            name: "documents".to_string(),
            description: description.to_string(),
            source_kind: "bundled".to_string(),
        }],
        max_activated_skills: 8,
        max_total_source_bytes: 512 * 1024,
    }
}

fn conversation_context_input(messages: Vec<AgentChatMessage>) -> AgentChatInput {
    AgentChatInput {
        api_url: "https://example.test/v1/chat/completions".to_string(),
        api_token: String::new(),
        provider_configuration_revision: None,
        provider_connection_revision: None,
        search_connection_revision: None,
        provider_profile_config: Some(ProviderProfileConfig::generic_for_dialect(
            ProviderProtocolDialect::OpenAiChatCompletions,
        )),
        provider_protocol_key: None,
        model_config_id: None,
        model: "test-model".to_string(),
        model_capabilities: crate::ModelCapabilities::default(),
        api_style: Some(crate::protocol::AgentApiStyle::OpenAiCompatible),
        context_window_tokens: Some(128_000),
        context_window_indicator_enabled: true,
        max_tokens: Some(30_000),
        temperature: None,
        stream: Some(true),
        context: None,
        search_config: None,
        prompt_preferences: None,
        approval_decision: None,
        tool_continuation: None,
        attachments: Vec::new(),
        resume_checkpoint: None,
        assistant_message_id: None,
        context_compaction_summary: None,
        world_state_records: Vec::new(),
        skill_activation: None,
        skill_discovery: None,
        messages,
    }
}

fn freeze_runtime_test_generic_provider(input: &mut AgentChatInput, revision_label: &str) {
    let dialect = match input.api_style.expect("runtime test API style") {
        crate::protocol::AgentApiStyle::OpenAiCompatible => {
            ProviderProtocolDialect::OpenAiChatCompletions
        }
        crate::protocol::AgentApiStyle::AnthropicCompatible => {
            ProviderProtocolDialect::AnthropicMessages
        }
    };
    let profile = ProviderProfileConfig::generic_for_dialect(dialect);
    let revision = format!("provider-protocol-v1:{revision_label}");
    let key = ProviderProtocolKey::new(
        dialect,
        &profile,
        input.model.clone(),
        Some(revision.clone()),
    )
    .expect("current runtime test Provider protocol key");
    input.provider_configuration_revision = Some(revision);
    input.provider_profile_config = Some(profile);
    input.provider_protocol_key = Some(key);
}

async fn read_runtime_test_json_request(stream: &mut tokio::net::TcpStream) -> Value {
    use tokio::io::AsyncReadExt;

    let mut request = Vec::new();
    let mut buffer = [0_u8; 4_096];
    let mut body_start = None;
    let mut expected_length = None;
    loop {
        let read = stream.read(&mut buffer).await.unwrap();
        assert!(read > 0, "connection closed before request completed");
        request.extend_from_slice(&buffer[..read]);
        if body_start.is_none() {
            if let Some(header_end) = request.windows(4).position(|part| part == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&request[..header_end]);
                let content_length = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().ok())
                            .flatten()
                    })
                    .unwrap_or_default();
                let start = header_end + 4;
                body_start = Some(start);
                expected_length = Some(start + content_length);
            }
        }
        if expected_length.is_some_and(|length| request.len() >= length) {
            break;
        }
    }
    serde_json::from_slice(&request[body_start.unwrap()..expected_length.expect("content length")])
        .unwrap()
}

async fn write_runtime_test_json_response(stream: &mut tokio::net::TcpStream, body: Value) {
    use tokio::io::AsyncWriteExt;

    let body = serde_json::to_vec(&body).unwrap();
    let headers = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(headers.as_bytes()).await.unwrap();
    stream.write_all(&body).await.unwrap();
}

fn runtime_steer_input(
    guidance_id: &str,
    client_message_id: &str,
    content: &str,
) -> crate::AgentSteerInput {
    crate::AgentSteerInput {
        guidance_id: guidance_id.to_string(),
        client_message_id: client_message_id.to_string(),
        content: content.to_string(),
        attachments: Vec::new(),
        attachment_library: None,
        created_at: 42,
    }
}

fn conversation_context_trace(
    terminal_status: ConversationTurnTraceTerminalStatus,
    items: Vec<ConversationTurnTraceItem>,
) -> ConversationTurnTrace {
    ConversationTurnTrace {
        schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
        run_id: "run-context".to_string(),
        conversation_id: "conversation-context".to_string(),
        assistant_message_id: "assistant-context".to_string(),
        terminal_status,
        terminal_error: None,
        truncated: false,
        items,
    }
}

fn traced_assistant_message(content: &str, trace: ConversationTurnTrace) -> AgentChatMessage {
    let mut provider_tool_index = 0_u32;
    let conversation_model_context_items = trace
        .items
        .iter()
        .filter_map(|item| {
            let sequence = item.sequence();
            let projected = match item {
                ConversationTurnTraceItem::AssistantNarration { content, .. } => {
                    crate::ConversationModelContextItem {
                        sequence,
                        ordinal: 0,
                        role: "assistant".to_string(),
                        content: content.clone(),
                        tool_call_id: None,
                        tool_calls: Vec::new(),
                        is_error: false,
                    }
                }
                ConversationTurnTraceItem::UserGuidance { content, .. } => {
                    crate::ConversationModelContextItem {
                        sequence,
                        ordinal: 0,
                        role: "user".to_string(),
                        content: content.clone(),
                        tool_call_id: None,
                        tool_calls: Vec::new(),
                        is_error: false,
                    }
                }
                ConversationTurnTraceItem::AgentMailboxDelivery { content, .. } => {
                    crate::ConversationModelContextItem {
                        sequence,
                        ordinal: 0,
                        role: "user".to_string(),
                        content: content.clone(),
                        tool_call_id: None,
                        tool_calls: Vec::new(),
                        is_error: false,
                    }
                }
                ConversationTurnTraceItem::ToolCall {
                    call_id,
                    tool,
                    operation,
                    ..
                } => {
                    let identity = crate::AgentProviderToolCallIdentity {
                        provider_tool_index,
                        provider_call_id: call_id.clone(),
                        runtime_call_id: call_id.clone(),
                    };
                    provider_tool_index = provider_tool_index.saturating_add(1);
                    crate::ConversationModelContextItem {
                        sequence,
                        ordinal: 0,
                        role: "assistant".to_string(),
                        content: String::new(),
                        tool_call_id: None,
                        tool_calls: vec![crate::AgentContextCheckpointToolCall {
                            id: call_id.clone(),
                            name: tool.clone(),
                            args: operation.clone(),
                            provider_identity: identity,
                        }],
                        is_error: false,
                    }
                }
                ConversationTurnTraceItem::ToolResult {
                    call_id,
                    observation,
                    success,
                    ..
                } => crate::ConversationModelContextItem {
                    sequence,
                    ordinal: 0,
                    role: "tool".to_string(),
                    content: observation.to_string(),
                    tool_call_id: Some(call_id.clone()),
                    tool_calls: Vec::new(),
                    is_error: !success,
                },
                ConversationTurnTraceItem::CommandSessionLifecycle { .. }
                | ConversationTurnTraceItem::ContextCompactionLifecycle { .. }
                | ConversationTurnTraceItem::RuntimeError { .. } => return None,
            };
            Some(projected)
        })
        .collect();
    AgentChatMessage {
        message_id: Some(trace.assistant_message_id.clone()),
        role: "assistant".to_string(),
        content: content.to_string(),
        created_at: Some(2_000),
        conversation_turn_trace: Some(trace),
        conversation_model_context_items,
    }
}

fn current_assistant_history_message(content: &str) -> AgentChatMessage {
    traced_assistant_message(
        content,
        conversation_context_trace(ConversationTurnTraceTerminalStatus::Completed, Vec::new()),
    )
}

fn full_conversation_context_snapshot(
    messages: Vec<AgentChatMessage>,
) -> AgentContextWindowSnapshot {
    create_conversation_context_state(conversation_context_input(messages))
        .unwrap()
        .snapshot()
}

mod approval_resume;
mod builtin_capability;
mod capabilities;
mod command_and_attachments;
mod compaction_and_tool_flow;
mod conversation_context;
mod conversation_world_state;
mod deepseek_recovery;
mod human_interaction;
mod moonshot_recovery;
mod request_layout;
mod skill_activation;
mod steering_and_repair;
mod trace_and_projection;
mod web_search;
