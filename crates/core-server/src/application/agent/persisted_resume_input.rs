use crate::application::agent_support::serialize_json;
use mycopilot_core::{
    skills::AgentSkillDiscoverySnapshot, AgentApiStyle, AgentChatInput, AgentPromptPreferences,
    AgentRunCheckpoint, AgentRunContext, AgentSearchConfig, AgentSearchMode, AgentSkillActivation,
    AnchoredWorldStateRecord, ContextCompactionSummary, ModelCapabilities, ProviderProfileConfig,
    ProviderProtocolKey,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const PERSISTED_AGENT_RESUME_INPUT_SCHEMA_VERSION: u32 = 13;

fn deserialize_required_nullable<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

/// Explicit allowlist for the durable continuation input owned by core-server.
///
/// This is intentionally not `#[serde(flatten)] AgentChatInput`: adding a new runtime field must
/// not silently make that field durable. This current envelope is decoded only by the Host that
/// owns it; storage treats it as opaque authenticated state and never projects it as Agent input.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct PersistedAgentResumeInput {
    resume_input_schema_version: u32,
    /// Opaque current `provider-protocol-v1` identity of the selected model's effective wire
    /// protocol.
    provider_configuration_revision: String,
    /// Stable identity of the selected model's effective endpoint/token pair. It is random and
    /// contains no credential material.
    provider_connection_revision: String,
    /// Historical search metadata retained for schema compatibility; live Host policy owns admission.
    search_connection_revision: String,
    provider_profile_config: ProviderProfileConfig,
    provider_protocol_key: ProviderProtocolKey,
    /// SHA-256 of the exact Host-resolved endpoint. The URL itself may contain credentials and
    /// therefore never enters the durable row.
    provider_endpoint_digest: String,
    /// Tokenless local providers remain supported, while a run that originally had a credential
    /// cannot silently resume without one.
    provider_credential_required: bool,
    /// Stable local model-configuration identity. It owns settings revisions and Usage; it is
    /// never sent to the Provider.
    model_config_id: String,
    /// Exact Provider wire model identifier.
    model: String,
    model_capabilities: ModelCapabilities,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    api_style: Option<AgentApiStyle>,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    context_window_tokens: Option<u32>,
    context_window_indicator_enabled: bool,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    max_tokens: Option<u32>,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    temperature: Option<f32>,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    stream: Option<bool>,
    /// Host-authenticated Run context. The current schema explicitly permits collaboration identity here;
    /// it must exactly match the same frozen identity in `resume_checkpoint.run_context`.
    #[serde(deserialize_with = "deserialize_required_nullable")]
    context: Option<AgentRunContext>,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    search_config: Option<PersistedAgentSearchConfig>,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    prompt_preferences: Option<AgentPromptPreferences>,
    resume_checkpoint: AgentRunCheckpoint,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    assistant_message_id: Option<String>,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    context_compaction_summary: Option<ContextCompactionSummary>,
    world_state_records: Vec<AnchoredWorldStateRecord>,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    skill_activation: Option<AgentSkillActivation>,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    skill_discovery: Option<AgentSkillDiscoverySnapshot>,
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
    pub(super) provider_endpoint_digest: String,
    pub(super) provider_credential_required: bool,
}

impl std::fmt::Debug for DecodedPersistedAgentResumeInput {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("DecodedPersistedAgentResumeInput([REDACTED])")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PersistedAgentResumeInputError {
    UnsupportedOrMalformed,
    SecretMaterialPresent,
    InvalidShape,
}

impl PersistedAgentResumeInput {
    pub(super) fn from_agent_input(input: &AgentChatInput) -> Result<Self, String> {
        if let Some(identity) = input
            .context
            .as_ref()
            .and_then(|context| context.collaboration_identity.as_ref())
        {
            identity.validate().map_err(|error| {
                format!("pending Agent collaboration identity is invalid: {error}")
            })?;
        }
        let provider_configuration_revision = input
            .provider_configuration_revision
            .clone()
            .ok_or_else(|| {
                "pending Agent input is missing its frozen provider settings revision".to_string()
            })?;
        if !mycopilot_core::storage::config_repository::is_provider_protocol_revision(
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
        let model_config_id = input
            .model_config_id
            .as_deref()
            .filter(|value| !value.trim().is_empty() && value.trim() == *value)
            .ok_or_else(|| {
                "pending Agent input is missing its local model configuration identity".to_string()
            })?
            .to_string();
        if input.resume_checkpoint.as_ref().is_some_and(|checkpoint| {
            checkpoint.provider_profile_config != provider_profile_config
                || checkpoint.provider_protocol_key != provider_protocol_key
        }) {
            return Err(
                "pending Agent checkpoint Provider Protocol provenance is inconsistent".to_string(),
            );
        }
        let resume_checkpoint = input
            .resume_checkpoint
            .clone()
            .ok_or_else(|| "pending Agent input is missing its frozen checkpoint".to_string())?;
        if resume_checkpoint.version != mycopilot_core::AGENT_RUN_CHECKPOINT_SCHEMA_VERSION {
            return Err("pending Agent checkpoint has an unsupported schema version".to_string());
        }
        if resume_checkpoint
            .file_change_run_grant_ref
            .as_ref()
            .is_some_and(|grant| grant.validate().is_err())
        {
            return Err("pending Agent checkpoint has an invalid FileChange Run grant".to_string());
        }
        if resume_checkpoint.context_items.is_empty() {
            return Err("pending Agent checkpoint is missing its exact context".to_string());
        }
        if let Some(snapshot) = resume_checkpoint.collaboration_run_snapshot.as_ref() {
            snapshot.validate().map_err(|_| {
                "pending Agent checkpoint has an invalid collaboration authorization snapshot"
                    .to_string()
            })?;
        }
        if input.context.as_ref().map(run_authority_snapshot)
            != resume_checkpoint
                .run_context
                .as_ref()
                .map(run_authority_snapshot)
        {
            return Err("pending Agent authority disagrees with its frozen checkpoint".to_string());
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
            model_config_id,
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
            resume_checkpoint,
            assistant_message_id: input.assistant_message_id.clone(),
            context_compaction_summary: input.context_compaction_summary.clone(),
            world_state_records: input.world_state_records.clone(),
            skill_activation: input.skill_activation.clone(),
            skill_discovery: input.skill_discovery.clone(),
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
            .map_err(|_| PersistedAgentResumeInputError::UnsupportedOrMalformed)?;
        persisted.into_agent_input()
    }

    fn into_agent_input(
        self,
    ) -> Result<DecodedPersistedAgentResumeInput, PersistedAgentResumeInputError> {
        if self.resume_input_schema_version != PERSISTED_AGENT_RESUME_INPUT_SCHEMA_VERSION {
            return Err(PersistedAgentResumeInputError::UnsupportedOrMalformed);
        }
        if !is_sha256_digest(&self.provider_endpoint_digest) {
            return Err(PersistedAgentResumeInputError::SecretMaterialPresent);
        }
        if !mycopilot_core::storage::config_repository::is_provider_protocol_revision(
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
        if self.resume_checkpoint.version != mycopilot_core::AGENT_RUN_CHECKPOINT_SCHEMA_VERSION
            || self.resume_checkpoint.context_items.is_empty()
            || self.resume_checkpoint.provider_profile_config != self.provider_profile_config
            || self.resume_checkpoint.provider_protocol_key != self.provider_protocol_key
        {
            return Err(PersistedAgentResumeInputError::InvalidShape);
        }
        if self
            .resume_checkpoint
            .file_change_run_grant_ref
            .as_ref()
            .is_some_and(|grant| grant.validate().is_err())
        {
            return Err(PersistedAgentResumeInputError::InvalidShape);
        }
        if self.context.as_ref().map(run_authority_snapshot)
            != self
                .resume_checkpoint
                .run_context
                .as_ref()
                .map(run_authority_snapshot)
        {
            return Err(PersistedAgentResumeInputError::InvalidShape);
        }
        if self.model_config_id.trim().is_empty()
            || self.model_config_id.trim() != self.model_config_id
            || self.model.trim().is_empty()
        {
            return Err(PersistedAgentResumeInputError::InvalidShape);
        }
        if self
            .context
            .as_ref()
            .and_then(|context| context.collaboration_identity.as_ref())
            .is_some_and(|identity| identity.validate().is_err())
        {
            return Err(PersistedAgentResumeInputError::InvalidShape);
        }
        Ok(DecodedPersistedAgentResumeInput {
            provider_configuration_revision: self.provider_configuration_revision.clone(),
            provider_connection_revision: self.provider_connection_revision.clone(),
            provider_endpoint_digest: self.provider_endpoint_digest,
            provider_credential_required: self.provider_credential_required,
            agent_input: AgentChatInput {
                context_image_attachments: Vec::new(),
                api_url: String::new(),
                api_token: String::new(),
                provider_configuration_revision: Some(self.provider_configuration_revision),
                provider_connection_revision: Some(self.provider_connection_revision),
                search_connection_revision: Some(self.search_connection_revision),
                provider_profile_config: Some(self.provider_profile_config),
                provider_protocol_key: Some(self.provider_protocol_key),
                model_config_id: Some(self.model_config_id),
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
                resume_checkpoint: Some(self.resume_checkpoint),
                assistant_message_id: self.assistant_message_id,
                context_compaction_summary: self.context_compaction_summary,
                world_state_records: self.world_state_records,
                skill_activation: self.skill_activation,
                skill_discovery: self.skill_discovery,
                messages: Vec::new(),
            },
        })
    }
}

fn run_authority_snapshot(
    context: &AgentRunContext,
) -> (
    mycopilot_core::AgentPermissions,
    Option<&mycopilot_core::AgentCollaborationIdentity>,
) {
    (context.permissions, context.collaboration_identity.as_ref())
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
            "pauseReason": "approval",
            "runId": "run-persisted-resume",
            "contextItems": [{
                "role": "assistant",
                "content": "",
                "images": [],
                "toolCalls": [{
                    "id": "call-persisted-resume",
                    "name": "probe_tool",
                    "args": {},
                    "providerIdentity": {
                        "providerToolIndex": 0,
                        "providerCallId": "call-persisted-resume",
                        "runtimeCallId": "call-persisted-resume"
                    }
                }],
                "isError": false,
                "sources": [],
                "scope": "conversation",
                "retention": "durable"
            }],
            "nextModelRequestIndex": 1,
            "queuedToolCalls": [],
            "deferredExternalToolCallCount": 0,
            "suppressedNarration": false,
            "extensionSnapshots": [],
            "toolSet": crate::test_tool_set_checkpoint(),
            "runContext": null,
            "collaborationRunSnapshot": null,
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
            "providerContinuationRefs": [],
            "conversationWorldStateRecords": [],
            "runWorldState": crate::test_run_world_state(),
            "pendingActionId": null,
            "pendingToolCallId": "call-persisted-resume",
            "fileChangeRunGrantRef": null,
            "pendingFileObservation": null,
            "conversationTraceItems": [],
            "conversationModelContextItems": [],
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
            "modelCapabilities": { "imageInput": false },
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
        let provider_configuration_revision =
            format!("provider-protocol-v1:{}", uuid::Uuid::new_v4());
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
        input.provider_configuration_revision = Some(provider_configuration_revision.clone());
        input.provider_connection_revision =
            Some(format!("provider-connection-v1:{}", uuid::Uuid::new_v4()));
        input.search_connection_revision =
            Some(format!("search-connection-v1:{}", uuid::Uuid::new_v4()));
        input.provider_profile_config = Some(provider_profile_config);
        input.provider_protocol_key = Some(provider_protocol_key);
        input.model_config_id = Some("model-config-test".to_string());
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
        let encoded_object = serde_json::from_str::<Value>(&encoded).unwrap();
        assert_eq!(encoded_object["modelConfigId"], "model-config-test");
        assert_eq!(encoded_object["model"], "test-model");
        assert_eq!(
            encoded_object
                .get("resumeInputSchemaVersion")
                .and_then(Value::as_u64),
            Some(u64::from(PERSISTED_AGENT_RESUME_INPUT_SCHEMA_VERSION))
        );
        for absent_placeholder in [
            "approvalDecision",
            "toolContinuation",
            "attachments",
            "messages",
        ] {
            assert!(encoded_object.get(absent_placeholder).is_none());
        }
        for forbidden_key in ["\"apiUrl\"", "\"apiToken\"", "\"tavilyApiKey\""] {
            assert!(
                !encoded.contains(forbidden_key),
                "credential-bearing runtime key must not enter the durable wire: {forbidden_key}"
            );
        }
        let restored = PersistedAgentResumeInput::decode(&encoded).unwrap();
        assert_eq!(
            restored.provider_endpoint_digest,
            persisted_endpoint_digest(&format!("https://example.test/v1?token={API_URL_CANARY}"))
        );
        assert!(restored.provider_credential_required);
        let restored = restored.agent_input;
        assert_eq!(
            restored.model_config_id.as_deref(),
            Some("model-config-test")
        );
        assert_eq!(restored.model, "test-model");
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
    fn collaboration_identity_and_permissions_round_trip_only_when_checkpoint_and_input_agree() {
        let identity = mycopilot_core::AgentCollaborationIdentity {
            agent_id: "agent-child".to_string(),
            root_agent_id: "agent-root".to_string(),
            root_conversation_id: "conversation-root".to_string(),
            parent_agent_id: "agent-root".to_string(),
            parent_task_name: "root".to_string(),
            parent_task_path: "/root".to_string(),
            conversation_id: "conversation-child".to_string(),
            task_name: "review".to_string(),
            task_path: "/root/review".to_string(),
            source_agent_id: "agent-root".to_string(),
            source_kind: mycopilot_core::AgentMailboxKind::Task,
            source_task_name: "root".to_string(),
            source_task_path: "/root".to_string(),
            source_agent_message_id: "mailbox-task-1".to_string(),
            entrusted_task: "Review the change and report evidence.\nInclude file locations."
                .to_string(),
            template_instructions: Some("Prioritize concrete evidence.".to_string()),
        };
        let context = AgentRunContext {
            conversation_id: Some(identity.conversation_id.clone()),
            project_id: Some("project-1".to_string()),
            workspace: None,
            attachment_library: None,
            permissions: mycopilot_core::AgentPermissions::default(),
            collaboration_identity: Some(identity.clone()),
        };
        let mut input = input();
        input.context = Some(context.clone());
        input.resume_checkpoint.as_mut().unwrap().run_context = Some(context);

        let encoded = PersistedAgentResumeInput::from_agent_input(&input)
            .unwrap()
            .encode();
        let restored = PersistedAgentResumeInput::decode(&encoded).unwrap();
        assert_eq!(
            restored
                .agent_input
                .context
                .as_ref()
                .and_then(|context| context.collaboration_identity.as_ref()),
            Some(&identity)
        );

        let mut mismatched = serde_json::from_str::<Value>(&encoded).unwrap();
        mismatched["resumeCheckpoint"]["runContext"]["collaborationIdentity"]["taskName"] =
            Value::String("different-task".to_string());
        assert_eq!(
            PersistedAgentResumeInput::decode(&mismatched.to_string()).unwrap_err(),
            PersistedAgentResumeInputError::InvalidShape
        );

        let mut mismatched_permissions = serde_json::from_str::<Value>(&encoded).unwrap();
        mismatched_permissions["resumeCheckpoint"]["runContext"]["permissions"]["read"] =
            Value::String("all".to_string());
        assert_eq!(
            PersistedAgentResumeInput::decode(&mismatched_permissions.to_string()).unwrap_err(),
            PersistedAgentResumeInputError::InvalidShape
        );

        let mut in_memory_mismatch = input;
        in_memory_mismatch
            .context
            .as_mut()
            .unwrap()
            .permissions
            .read = mycopilot_core::AgentReadPermission::All;
        assert_eq!(
            PersistedAgentResumeInput::from_agent_input(&in_memory_mismatch)
                .err()
                .unwrap(),
            "pending Agent authority disagrees with its frozen checkpoint"
        );
    }

    #[test]
    fn incomplete_or_diverged_provider_freeze_fails_without_panicking() {
        let mut missing_model_owner = input();
        missing_model_owner.model_config_id = None;
        assert_eq!(
            PersistedAgentResumeInput::from_agent_input(&missing_model_owner)
                .err()
                .unwrap(),
            "pending Agent input is missing its local model configuration identity"
        );

        let mut missing_checkpoint = input();
        missing_checkpoint.resume_checkpoint = None;
        assert_eq!(
            PersistedAgentResumeInput::from_agent_input(&missing_checkpoint)
                .err()
                .unwrap(),
            "pending Agent input is missing its frozen checkpoint"
        );

        let mut empty_context = input();
        empty_context
            .resume_checkpoint
            .as_mut()
            .unwrap()
            .context_items
            .clear();
        assert_eq!(
            PersistedAgentResumeInput::from_agent_input(&empty_context)
                .err()
                .unwrap(),
            "pending Agent checkpoint is missing its exact context"
        );

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
    fn unknown_fields_fail_closed() {
        let encoded = PersistedAgentResumeInput::from_agent_input(&input())
            .unwrap()
            .encode();
        let mut value = serde_json::from_str::<Value>(&encoded).unwrap();
        value["futureSecretField"] =
            Value::String("must not become durable implicitly".to_string());
        assert_eq!(
            PersistedAgentResumeInput::decode(&value.to_string()).unwrap_err(),
            PersistedAgentResumeInputError::UnsupportedOrMalformed
        );

        let mut retired_goal = serde_json::from_str::<Value>(&encoded).unwrap();
        retired_goal["goal"] = Value::Null;
        assert_eq!(
            PersistedAgentResumeInput::decode(&retired_goal.to_string()).unwrap_err(),
            PersistedAgentResumeInputError::UnsupportedOrMalformed
        );

        let mut nested = serde_json::from_str::<Value>(&encoded).unwrap();
        nested["promptPreferences"] = json!({
            "futureSecretField": "must not be ignored inside a current persisted DTO"
        });
        assert_eq!(
            PersistedAgentResumeInput::decode(&nested.to_string()).unwrap_err(),
            PersistedAgentResumeInputError::UnsupportedOrMalformed
        );
    }

    #[test]
    fn secret_fields_and_unsupported_versions_are_rejected() {
        let encoded = PersistedAgentResumeInput::from_agent_input(&input())
            .unwrap()
            .encode();
        for (key, value) in [
            (
                "apiUrl",
                Value::String("https://forbidden.invalid".to_string()),
            ),
            ("apiToken", Value::String(API_TOKEN_CANARY.to_string())),
        ] {
            let mut malformed = serde_json::from_str::<Value>(&encoded).unwrap();
            malformed[key] = value;
            assert_eq!(
                PersistedAgentResumeInput::decode(&malformed.to_string()).unwrap_err(),
                PersistedAgentResumeInputError::UnsupportedOrMalformed
            );
        }

        let mut malformed = serde_json::from_str::<Value>(&encoded).unwrap();
        malformed["searchConfig"]["tavilyApiKey"] = Value::String(SEARCH_KEY_CANARY.to_string());
        assert_eq!(
            PersistedAgentResumeInput::decode(&malformed.to_string()).unwrap_err(),
            PersistedAgentResumeInputError::UnsupportedOrMalformed
        );

        for unsupported_version in [0, 5, 6, 7, 8, 9, 10] {
            let mut old_version = serde_json::from_str::<Value>(&encoded).unwrap();
            old_version["resumeInputSchemaVersion"] = Value::from(unsupported_version);
            assert_eq!(
                PersistedAgentResumeInput::decode(&old_version.to_string()).unwrap_err(),
                PersistedAgentResumeInputError::UnsupportedOrMalformed
            );
        }

        let mut old_checkpoint = serde_json::from_str::<Value>(&encoded).unwrap();
        old_checkpoint["resumeCheckpoint"]["version"] =
            Value::from(mycopilot_core::AGENT_RUN_CHECKPOINT_SCHEMA_VERSION - 1);
        assert_eq!(
            PersistedAgentResumeInput::decode(&old_checkpoint.to_string()).unwrap_err(),
            PersistedAgentResumeInputError::InvalidShape
        );

        let mut invalid_revision = serde_json::from_str::<Value>(&encoded).unwrap();
        invalid_revision["providerConfigurationRevision"] =
            Value::String("provider-protocol-v1:not-a-uuid".to_string());
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
    fn every_current_nullable_field_must_be_present() {
        let encoded = PersistedAgentResumeInput::from_agent_input(&input())
            .unwrap()
            .encode();
        for field in [
            "apiStyle",
            "contextWindowTokens",
            "maxTokens",
            "temperature",
            "stream",
            "context",
            "searchConfig",
            "promptPreferences",
            "assistantMessageId",
            "contextCompactionSummary",
            "skillActivation",
            "skillDiscovery",
        ] {
            let mut missing = serde_json::from_str::<Value>(&encoded).unwrap();
            missing.as_object_mut().unwrap().remove(field);
            assert_eq!(
                PersistedAgentResumeInput::decode(&missing.to_string()).unwrap_err(),
                PersistedAgentResumeInputError::UnsupportedOrMalformed,
                "missing current field {field} must fail closed"
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
