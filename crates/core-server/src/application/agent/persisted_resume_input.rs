use crate::application::agent_support::serialize_json;
use mycopilot_core::{
    skills::AgentSkillDiscoverySnapshot, AgentApiStyle, AgentChatInput, AgentChatMessage,
    AgentInputAttachment, AgentPromptPreferences, AgentRunCheckpoint, AgentRunContext,
    AgentSearchConfig, AgentSearchMode, AgentSkillActivation, AgentToolContinuation,
    AnchoredWorldStateRecord, ContextCompactionSummary, ConversationGoal, ModelCapabilities,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const PERSISTED_AGENT_RESUME_INPUT_SCHEMA_VERSION: u32 = 3;

/// Explicit allowlist for the durable continuation input owned by core-server.
///
/// This is intentionally not `#[serde(flatten)] AgentChatInput`: adding a new runtime field must
/// not silently make that field durable. The serialized projection remains readable as an
/// `AgentChatInput` by Core's startup reconciliation, while this Host requires the version marker
/// and rejects pre-versioned rows during approval restoration.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct PersistedAgentResumeInput {
    resume_input_schema_version: u32,
    /// Opaque, random identity of the exact settings save used by the Host. It contains no
    /// credential-derived material and is intentionally unavailable to Renderer.
    provider_configuration_revision: String,
    /// SHA-256 of the exact Host-resolved endpoint. The URL itself may contain credentials and
    /// therefore never enters the durable row.
    provider_endpoint_digest: String,
    /// Tokenless local providers remain supported, while a run that originally had a credential
    /// cannot silently resume without one.
    provider_credential_required: bool,
    model: String,
    model_capabilities: ModelCapabilities,
    api_style: Option<AgentApiStyle>,
    context_window_tokens: Option<u32>,
    context_window_indicator_enabled: bool,
    max_tokens: Option<u32>,
    temperature: Option<f32>,
    stream: Option<bool>,
    context: Option<AgentRunContext>,
    search_config: Option<PersistedAgentSearchConfig>,
    prompt_preferences: Option<AgentPromptPreferences>,
    /// Durable pending rows never carry an already-decided approval.
    approval_decision: Option<mycopilot_core::AgentApprovalDecision>,
    /// A tool result is committed separately and never embedded in the frozen approval input.
    tool_continuation: Option<AgentToolContinuation>,
    /// Attachment bytes are owned by the attachment library and never duplicated here.
    attachments: Vec<AgentInputAttachment>,
    resume_checkpoint: Option<AgentRunCheckpoint>,
    assistant_message_id: Option<String>,
    context_compaction_summary: Option<ContextCompactionSummary>,
    goal: Option<ConversationGoal>,
    world_state_records: Vec<AnchoredWorldStateRecord>,
    skill_activation: Option<AgentSkillActivation>,
    skill_discovery: Option<AgentSkillDiscoverySnapshot>,
    /// The checkpoint owns the replay-safe resumed model projection; process-only MCP arguments
    /// and raw messages are not duplicated.
    messages: Vec<AgentChatMessage>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PersistedAgentSearchConfig {
    mode: AgentSearchMode,
    credential_required: bool,
}

pub(super) struct DecodedPersistedAgentResumeInput {
    pub(super) agent_input: AgentChatInput,
    pub(super) provider_configuration_revision: String,
    pub(super) provider_endpoint_digest: String,
    pub(super) provider_credential_required: bool,
    pub(super) search_credential_required: bool,
}

impl std::fmt::Debug for DecodedPersistedAgentResumeInput {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("DecodedPersistedAgentResumeInput([REDACTED])")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PersistedAgentResumeInputError {
    LegacyOrUnsupported,
    SecretMaterialPresent,
    InvalidShape,
}

impl PersistedAgentResumeInput {
    pub(super) fn from_agent_input(input: &AgentChatInput) -> Self {
        Self {
            resume_input_schema_version: PERSISTED_AGENT_RESUME_INPUT_SCHEMA_VERSION,
            provider_configuration_revision: input
                .provider_configuration_revision
                .clone()
                .unwrap_or_default(),
            provider_endpoint_digest: sha256_hex(input.api_url.as_bytes()),
            provider_credential_required: !input.api_token.trim().is_empty(),
            model: input.model.clone(),
            model_capabilities: input.model_capabilities,
            api_style: Some(
                input
                    .api_style
                    .unwrap_or_else(|| resolved_api_style(&input.api_url)),
            ),
            context_window_tokens: input.context_window_tokens,
            context_window_indicator_enabled: input.context_window_indicator_enabled,
            max_tokens: input.max_tokens,
            temperature: input.temperature,
            stream: input.stream,
            context: input.context.clone(),
            search_config: input
                .search_config
                .as_ref()
                .map(|search| PersistedAgentSearchConfig {
                    mode: search.mode,
                    credential_required: search
                        .tavily_api_key
                        .as_deref()
                        .is_some_and(|secret| !secret.trim().is_empty()),
                }),
            prompt_preferences: input.prompt_preferences.clone(),
            approval_decision: None,
            tool_continuation: None,
            attachments: Vec::new(),
            resume_checkpoint: input.resume_checkpoint.clone(),
            assistant_message_id: input.assistant_message_id.clone(),
            context_compaction_summary: input.context_compaction_summary.clone(),
            goal: input.goal.clone(),
            world_state_records: input.world_state_records.clone(),
            skill_activation: input.skill_activation.clone(),
            skill_discovery: input.skill_discovery.clone(),
            messages: Vec::new(),
        }
    }

    pub(super) fn encode(&self) -> String {
        // Every field is a serde-supported in-memory domain type. Retaining the shared serializer
        // keeps the existing storage API infallible without ever including a secret in an error.
        serialize_json(self)
    }

    pub(super) fn decode(
        value: &str,
    ) -> Result<DecodedPersistedAgentResumeInput, PersistedAgentResumeInputError> {
        let persisted = serde_json::from_str::<Self>(value)
            .map_err(|_| PersistedAgentResumeInputError::LegacyOrUnsupported)?;
        persisted.into_agent_input()
    }

    fn into_agent_input(
        self,
    ) -> Result<DecodedPersistedAgentResumeInput, PersistedAgentResumeInputError> {
        if self.resume_input_schema_version != PERSISTED_AGENT_RESUME_INPUT_SCHEMA_VERSION {
            return Err(PersistedAgentResumeInputError::LegacyOrUnsupported);
        }
        if !is_sha256_digest(&self.provider_endpoint_digest) {
            return Err(PersistedAgentResumeInputError::SecretMaterialPresent);
        }
        if !mycopilot_core::storage::config_repository::is_model_settings_revision(
            &self.provider_configuration_revision,
        ) {
            return Err(PersistedAgentResumeInputError::InvalidShape);
        }
        if self.approval_decision.is_some()
            || self.tool_continuation.is_some()
            || !self.attachments.is_empty()
            || !self.messages.is_empty()
            || self.model.trim().is_empty()
        {
            return Err(PersistedAgentResumeInputError::InvalidShape);
        }
        let search_credential_required = self
            .search_config
            .as_ref()
            .is_some_and(|search| search.credential_required);
        Ok(DecodedPersistedAgentResumeInput {
            provider_configuration_revision: self.provider_configuration_revision.clone(),
            provider_endpoint_digest: self.provider_endpoint_digest,
            provider_credential_required: self.provider_credential_required,
            search_credential_required,
            agent_input: AgentChatInput {
                api_url: String::new(),
                api_token: String::new(),
                provider_configuration_revision: Some(self.provider_configuration_revision),
                model: self.model,
                model_capabilities: self.model_capabilities,
                api_style: self.api_style,
                context_window_tokens: self.context_window_tokens,
                context_window_indicator_enabled: self.context_window_indicator_enabled,
                max_tokens: self.max_tokens,
                temperature: self.temperature,
                stream: self.stream,
                context: self.context,
                search_config: self.search_config.map(|search| AgentSearchConfig {
                    mode: search.mode,
                    tavily_api_key: None,
                }),
                prompt_preferences: self.prompt_preferences,
                approval_decision: None,
                tool_continuation: None,
                attachments: Vec::new(),
                resume_checkpoint: self.resume_checkpoint,
                assistant_message_id: self.assistant_message_id,
                context_compaction_summary: self.context_compaction_summary,
                goal: self.goal,
                world_state_records: self.world_state_records,
                skill_activation: self.skill_activation,
                skill_discovery: self.skill_discovery,
                messages: Vec::new(),
            },
        })
    }
}

pub(super) fn persisted_endpoint_digest(value: &str) -> String {
    sha256_hex(value.as_bytes())
}

fn resolved_api_style(api_url: &str) -> AgentApiStyle {
    let normalized = api_url.trim().to_ascii_lowercase();
    if normalized.contains("/chat/completions") {
        AgentApiStyle::OpenAiCompatible
    } else if normalized.contains("anthropic") || normalized.ends_with("/messages") {
        AgentApiStyle::AnthropicCompatible
    } else {
        AgentApiStyle::OpenAiCompatible
    }
}

fn sha256_hex(value: &[u8]) -> String {
    let digest = Sha256::digest(value);
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(&mut encoded, "{byte:02x}");
    }
    encoded
}

fn is_sha256_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    const API_TOKEN_CANARY: &str = "PENDING_API_TOKEN_CANARY_DO_NOT_PERSIST";
    const API_URL_CANARY: &str = "PENDING_API_URL_QUERY_CANARY_DO_NOT_PERSIST";
    const SEARCH_KEY_CANARY: &str = "PENDING_SEARCH_KEY_CANARY_DO_NOT_PERSIST";

    fn checkpoint() -> AgentRunCheckpoint {
        serde_json::from_value(json!({
            "version": mycopilot_core::AGENT_RUN_CHECKPOINT_SCHEMA_VERSION,
            "runId": "run-persisted-resume",
            "contextItems": [],
            "nextModelRequestIndex": 1,
            "queuedToolCalls": [],
            "suppressedNarration": false,
            "extensionSnapshots": [],
            "toolSet": crate::test_tool_set_checkpoint(),
            "runContext": null,
            "modelCapabilities": { "imageInput": false },
            "runWorldState": crate::test_run_world_state(),
            "pendingToolCallId": "call-persisted-resume",
            "conversationTraceItems": [],
            "nextConversationTraceSequence": 0,
            "conversationTraceTruncated": false
        }))
        .unwrap()
    }

    fn input() -> AgentChatInput {
        let mut input = serde_json::from_value::<AgentChatInput>(json!({
            "apiUrl": format!("https://example.test/v1?token={API_URL_CANARY}"),
            "apiToken": API_TOKEN_CANARY,
            "model": "test-model",
            "searchConfig": {
                "mode": "tavily",
                "tavilyApiKey": SEARCH_KEY_CANARY
            },
            "messages": [{
                "role": "user",
                "content": "raw messages are checkpoint-owned"
            }]
        }))
        .unwrap();
        input.resume_checkpoint = Some(checkpoint());
        input.provider_configuration_revision =
            Some(format!("model-settings-v1:{}", uuid::Uuid::new_v4()));
        input
    }

    #[test]
    fn allowlisted_projection_contains_no_connection_or_search_secret() {
        let input = input();
        let renderer_wire = serde_json::to_string(&input).unwrap();
        assert!(!renderer_wire.contains("providerConfigurationRevision"));
        let encoded = PersistedAgentResumeInput::from_agent_input(&input).encode();

        assert!(!encoded.contains(API_TOKEN_CANARY));
        assert!(!encoded.contains(API_URL_CANARY));
        assert!(!encoded.contains(SEARCH_KEY_CANARY));
        assert!(!encoded.contains("raw messages are checkpoint-owned"));
        assert!(encoded.contains("\"resumeInputSchemaVersion\":3"));
        for forbidden_key in ["\"apiUrl\"", "\"apiToken\"", "\"tavilyApiKey\""] {
            assert!(
                !encoded.contains(forbidden_key),
                "credential-bearing compatibility key must not enter the durable wire: {forbidden_key}"
            );
        }
        let restored = PersistedAgentResumeInput::decode(&encoded).unwrap();
        assert_eq!(
            restored.provider_endpoint_digest,
            persisted_endpoint_digest(&format!("https://example.test/v1?token={API_URL_CANARY}"))
        );
        assert!(restored.provider_credential_required);
        assert!(restored.search_credential_required);
        let restored = restored.agent_input;
        assert_eq!(restored.api_style, Some(AgentApiStyle::OpenAiCompatible));
        assert!(restored.api_url.is_empty());
        assert!(restored.api_token.is_empty());
        assert!(restored
            .search_config
            .as_ref()
            .unwrap()
            .tavily_api_key
            .is_none());
        assert!(restored.messages.is_empty());
        assert!(restored.attachments.is_empty());
        assert_eq!(
            restored.resume_checkpoint.unwrap().pending_tool_call_id,
            "call-persisted-resume"
        );
    }

    #[test]
    fn legacy_full_agent_input_and_unknown_fields_fail_closed() {
        let legacy = serde_json::to_string(&input()).unwrap();
        assert_eq!(
            PersistedAgentResumeInput::decode(&legacy).unwrap_err(),
            PersistedAgentResumeInputError::LegacyOrUnsupported
        );

        let encoded = PersistedAgentResumeInput::from_agent_input(&input()).encode();
        let mut value = serde_json::from_str::<Value>(&encoded).unwrap();
        value["futureSecretField"] =
            Value::String("must not become durable implicitly".to_string());
        assert_eq!(
            PersistedAgentResumeInput::decode(&value.to_string()).unwrap_err(),
            PersistedAgentResumeInputError::LegacyOrUnsupported
        );
    }

    #[test]
    fn legacy_versioned_rows_with_connection_or_search_keys_are_rejected() {
        let encoded = PersistedAgentResumeInput::from_agent_input(&input()).encode();
        for (key, value) in [
            (
                "apiUrl",
                Value::String("https://legacy.invalid".to_string()),
            ),
            ("apiToken", Value::String(API_TOKEN_CANARY.to_string())),
        ] {
            let mut legacy = serde_json::from_str::<Value>(&encoded).unwrap();
            legacy[key] = value;
            assert_eq!(
                PersistedAgentResumeInput::decode(&legacy.to_string()).unwrap_err(),
                PersistedAgentResumeInputError::LegacyOrUnsupported
            );
        }

        let mut legacy = serde_json::from_str::<Value>(&encoded).unwrap();
        legacy["searchConfig"]["tavilyApiKey"] = Value::String(SEARCH_KEY_CANARY.to_string());
        assert_eq!(
            PersistedAgentResumeInput::decode(&legacy.to_string()).unwrap_err(),
            PersistedAgentResumeInputError::LegacyOrUnsupported
        );

        for legacy_version in [1, 2] {
            let mut old_version = serde_json::from_str::<Value>(&encoded).unwrap();
            old_version["resumeInputSchemaVersion"] = Value::from(legacy_version);
            assert_eq!(
                PersistedAgentResumeInput::decode(&old_version.to_string()).unwrap_err(),
                PersistedAgentResumeInputError::LegacyOrUnsupported
            );
        }

        let mut invalid_revision = serde_json::from_str::<Value>(&encoded).unwrap();
        invalid_revision["providerConfigurationRevision"] =
            Value::String("model-settings-v1:not-a-uuid".to_string());
        assert_eq!(
            PersistedAgentResumeInput::decode(&invalid_revision.to_string()).unwrap_err(),
            PersistedAgentResumeInputError::InvalidShape
        );
    }
}
