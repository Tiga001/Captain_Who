use crate::application::agent_support::serialize_json;
use mycopilot_core::{
    skills::AgentSkillDiscoverySnapshot, AgentApiStyle, AgentChatInput, AgentChatMessage,
    AgentInputAttachment, AgentPromptPreferences, AgentRunCheckpoint, AgentRunContext,
    AgentSearchConfig, AgentSearchMode, AgentSkillActivation, AgentToolContinuation,
    AnchoredWorldStateRecord, ContextCompactionSummary, ConversationGoal, ModelCapabilities,
    ProviderProfileConfig, ProviderProtocolKey,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const PERSISTED_AGENT_RESUME_INPUT_SCHEMA_VERSION: u32 = 5;

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
    /// Stable identity of the selected model's effective endpoint/token pair. It is random and
    /// contains no credential-derived material.
    provider_connection_revision: String,
    /// Stable identity of the effective search mode/credential pair.
    search_connection_revision: String,
    provider_profile_config: ProviderProfileConfig,
    provider_protocol_key: ProviderProtocolKey,
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
    pub(super) provider_connection_revision: String,
    pub(super) search_connection_revision: String,
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
    pub(super) fn from_agent_input(input: &AgentChatInput) -> Result<Self, String> {
        let provider_configuration_revision = input
            .provider_configuration_revision
            .clone()
            .ok_or_else(|| {
                "pending Agent input is missing its frozen provider settings revision".to_string()
            })?;
        if !mycopilot_core::storage::config_repository::is_model_settings_revision(
            &provider_configuration_revision,
        ) {
            return Err(
                "pending Agent input has an invalid frozen provider settings revision".to_string(),
            );
        }
        let provider_connection_revision =
            input.provider_connection_revision.clone().ok_or_else(|| {
                "pending Agent input is missing its frozen provider connection revision".to_string()
            })?;
        if !mycopilot_core::storage::config_repository::is_provider_connection_revision(
            &provider_connection_revision,
        ) {
            return Err(
                "pending Agent input has an invalid frozen provider connection revision"
                    .to_string(),
            );
        }
        let search_connection_revision =
            input.search_connection_revision.clone().ok_or_else(|| {
                "pending Agent input is missing its frozen search connection revision".to_string()
            })?;
        if !mycopilot_core::storage::config_repository::is_search_connection_revision(
            &search_connection_revision,
        ) {
            return Err(
                "pending Agent input has an invalid frozen search connection revision".to_string(),
            );
        }
        let provider_profile_config = input.provider_profile_config.clone().ok_or_else(|| {
            "pending Agent input is missing its frozen Provider Profile".to_string()
        })?;
        provider_profile_config
            .validate()
            .map_err(|_| "pending Agent input has an invalid Provider Profile".to_string())?;
        let provider_protocol_key = input.provider_protocol_key.clone().ok_or_else(|| {
            "pending Agent input is missing its frozen Provider Protocol key".to_string()
        })?;
        provider_protocol_key
            .validate_against_config(&provider_profile_config)
            .map_err(|_| "pending Agent input has an invalid Provider Protocol key".to_string())?;
        if provider_protocol_key.model_id != input.model
            || provider_protocol_key
                .provider_configuration_revision
                .as_deref()
                != Some(provider_configuration_revision.as_str())
        {
            return Err(
                "pending Agent input Provider Protocol provenance is inconsistent".to_string(),
            );
        }
        if input.resume_checkpoint.as_ref().is_some_and(|checkpoint| {
            checkpoint.provider_profile_config != provider_profile_config
                || checkpoint.provider_protocol_key != provider_protocol_key
        }) {
            return Err(
                "pending Agent checkpoint Provider Protocol provenance is inconsistent".to_string(),
            );
        }

        Ok(Self {
            resume_input_schema_version: PERSISTED_AGENT_RESUME_INPUT_SCHEMA_VERSION,
            provider_configuration_revision,
            provider_connection_revision,
            search_connection_revision,
            provider_profile_config,
            provider_protocol_key,
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
        })
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
        if !mycopilot_core::storage::config_repository::is_provider_connection_revision(
            &self.provider_connection_revision,
        ) || !mycopilot_core::storage::config_repository::is_search_connection_revision(
            &self.search_connection_revision,
        ) {
            return Err(PersistedAgentResumeInputError::InvalidShape);
        }
        self.provider_profile_config
            .validate()
            .map_err(|_| PersistedAgentResumeInputError::InvalidShape)?;
        self.provider_protocol_key
            .validate_against_config(&self.provider_profile_config)
            .map_err(|_| PersistedAgentResumeInputError::InvalidShape)?;
        if self.provider_protocol_key.model_id != self.model
            || self
                .provider_protocol_key
                .provider_configuration_revision
                .as_deref()
                != Some(self.provider_configuration_revision.as_str())
        {
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
            provider_connection_revision: self.provider_connection_revision.clone(),
            search_connection_revision: self.search_connection_revision.clone(),
            provider_endpoint_digest: self.provider_endpoint_digest,
            provider_credential_required: self.provider_credential_required,
            search_credential_required,
            agent_input: AgentChatInput {
                api_url: String::new(),
                api_token: String::new(),
                provider_configuration_revision: Some(self.provider_configuration_revision),
                provider_connection_revision: Some(self.provider_connection_revision),
                search_connection_revision: Some(self.search_connection_revision),
                provider_profile_config: Some(self.provider_profile_config),
                provider_protocol_key: Some(self.provider_protocol_key),
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
    mycopilot_core::ProviderProtocolDialect::detect_from_api_url(api_url).api_style()
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

    fn checkpoint(provider_configuration_revision: &str) -> AgentRunCheckpoint {
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
            "providerProfileConfig": {
                "schemaVersion": 1,
                "profile": { "id": "generic_openai_chat", "version": 1 },
                "reasoning": { "mode": "provider_default", "effort": "provider_default" }
            },
            "providerProtocolKey": {
                "dialect": "openai_chat_completions",
                "profile": { "id": "generic_openai_chat", "version": 1 },
                "modelId": "test-model",
                "providerConfigurationRevision": provider_configuration_revision
            },
            "assistantTurnIdentity": {
                "assistantTurnId": "turn-persisted-resume",
                "assistantTurnDigest": "digest-persisted-resume",
                "toolCallIdentities": [{
                    "providerToolIndex": 0,
                    "providerCallId": "call-persisted-resume",
                    "runtimeCallId": "call-persisted-resume"
                }]
            },
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
        let provider_configuration_revision = format!("model-settings-v1:{}", uuid::Uuid::new_v4());
        let provider_profile_config = ProviderProfileConfig::generic_for_dialect(
            mycopilot_core::ProviderProtocolDialect::OpenAiChatCompletions,
        );
        let provider_protocol_key = ProviderProtocolKey::new(
            mycopilot_core::ProviderProtocolDialect::OpenAiChatCompletions,
            &provider_profile_config,
            "test-model",
            Some(provider_configuration_revision.clone()),
        )
        .unwrap();
        input.resume_checkpoint = Some(checkpoint(&provider_configuration_revision));
        input.provider_configuration_revision = Some(provider_configuration_revision);
        input.provider_connection_revision =
            Some(format!("provider-connection-v1:{}", uuid::Uuid::new_v4()));
        input.search_connection_revision =
            Some(format!("search-connection-v1:{}", uuid::Uuid::new_v4()));
        input.provider_profile_config = Some(provider_profile_config);
        input.provider_protocol_key = Some(provider_protocol_key);
        input
    }

    #[test]
    fn allowlisted_projection_contains_no_connection_or_search_secret() {
        let input = input();
        let renderer_wire = serde_json::to_string(&input).unwrap();
        let renderer_wire_value = serde_json::from_str::<Value>(&renderer_wire).unwrap();
        assert!(renderer_wire_value
            .get("providerConfigurationRevision")
            .is_none());
        assert!(renderer_wire_value
            .get("providerConnectionRevision")
            .is_none());
        assert!(renderer_wire_value
            .get("searchConnectionRevision")
            .is_none());
        assert!(renderer_wire_value.get("providerProfileConfig").is_none());
        assert!(renderer_wire_value.get("providerProtocolKey").is_none());
        let encoded = PersistedAgentResumeInput::from_agent_input(&input)
            .unwrap()
            .encode();

        assert!(encoded.contains("\"providerProfileConfig\""));
        assert!(encoded.contains("\"providerProtocolKey\""));
        assert!(!encoded.contains(API_TOKEN_CANARY));
        assert!(!encoded.contains(API_URL_CANARY));
        assert!(!encoded.contains(SEARCH_KEY_CANARY));
        assert!(!encoded.contains("raw messages are checkpoint-owned"));
        assert!(encoded.contains("\"resumeInputSchemaVersion\":5"));
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
    fn incomplete_or_diverged_provider_freeze_fails_without_panicking() {
        let mut missing = input();
        missing.provider_protocol_key = None;
        assert!(PersistedAgentResumeInput::from_agent_input(&missing).is_err());

        let mut diverged = input();
        diverged
            .resume_checkpoint
            .as_mut()
            .unwrap()
            .provider_protocol_key
            .model_id = "different-model".to_string();
        assert!(PersistedAgentResumeInput::from_agent_input(&diverged).is_err());
    }

    #[test]
    fn legacy_full_agent_input_and_unknown_fields_fail_closed() {
        let legacy = serde_json::to_string(&input()).unwrap();
        assert_eq!(
            PersistedAgentResumeInput::decode(&legacy).unwrap_err(),
            PersistedAgentResumeInputError::LegacyOrUnsupported
        );

        let encoded = PersistedAgentResumeInput::from_agent_input(&input())
            .unwrap()
            .encode();
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
        let encoded = PersistedAgentResumeInput::from_agent_input(&input())
            .unwrap()
            .encode();
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

        for legacy_version in [1, 2, 3, 4] {
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

        for field in ["providerConnectionRevision", "searchConnectionRevision"] {
            let mut invalid_connection_revision = serde_json::from_str::<Value>(&encoded).unwrap();
            invalid_connection_revision[field] = Value::String("not-a-revision".to_string());
            assert_eq!(
                PersistedAgentResumeInput::decode(&invalid_connection_revision.to_string())
                    .unwrap_err(),
                PersistedAgentResumeInputError::InvalidShape
            );
        }
    }

    #[test]
    fn provider_profile_and_protocol_provenance_mismatches_fail_closed() {
        let encoded = PersistedAgentResumeInput::from_agent_input(&input())
            .unwrap()
            .encode();

        let mut unknown_profile_version = serde_json::from_str::<Value>(&encoded).unwrap();
        unknown_profile_version["providerProfileConfig"]["profile"]["version"] = Value::from(99);
        assert_eq!(
            PersistedAgentResumeInput::decode(&unknown_profile_version.to_string()).unwrap_err(),
            PersistedAgentResumeInputError::InvalidShape
        );

        let mut wrong_model = serde_json::from_str::<Value>(&encoded).unwrap();
        wrong_model["providerProtocolKey"]["modelId"] =
            Value::String("different-model".to_string());
        assert_eq!(
            PersistedAgentResumeInput::decode(&wrong_model.to_string()).unwrap_err(),
            PersistedAgentResumeInputError::InvalidShape
        );

        let mut mismatched_profile = serde_json::from_str::<Value>(&encoded).unwrap();
        mismatched_profile["providerProfileConfig"]["profile"] = json!({
            "id": "deepseek_v4_chat",
            "version": 1
        });
        assert_eq!(
            PersistedAgentResumeInput::decode(&mismatched_profile.to_string()).unwrap_err(),
            PersistedAgentResumeInputError::InvalidShape
        );
    }
}
