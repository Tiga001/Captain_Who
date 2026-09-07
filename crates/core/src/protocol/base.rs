use super::*;

pub const PROVIDER_CONTINUATION_REF_VERSION: u32 = 1;

/// Opaque, conversation-bound handle to provider-owned continuation state.
///
/// The referenced payload is stored in the Host's private encrypted vault. This value is safe to
/// persist in an approval checkpoint, but it deliberately carries no payload hash, ciphertext,
/// provider text, or storage location.
#[derive(Deserialize, Serialize, Clone, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderContinuationRef {
    pub version: u32,
    pub id: String,
}

impl ProviderContinuationRef {
    pub fn new() -> Self {
        Self {
            version: PROVIDER_CONTINUATION_REF_VERSION,
            id: format!(
                "provider-continuation-v1:{}",
                uuid::Uuid::new_v4().hyphenated()
            ),
        }
    }

    pub fn parse(version: u32, id: impl Into<String>) -> Result<Self, String> {
        let value = Self {
            version,
            id: id.into(),
        };
        value.validate()?;
        Ok(value)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.version != PROVIDER_CONTINUATION_REF_VERSION {
            return Err("provider continuation ref version is unsupported".to_string());
        }
        let Some(raw_uuid) = self.id.strip_prefix("provider-continuation-v1:") else {
            return Err("provider continuation ref prefix is invalid".to_string());
        };
        let parsed = uuid::Uuid::parse_str(raw_uuid)
            .map_err(|_| "provider continuation ref UUID is invalid".to_string())?;
        if parsed.get_version_num() != 4 || parsed.hyphenated().to_string() != raw_uuid {
            return Err("provider continuation ref UUID is not canonical v4".to_string());
        }
        Ok(())
    }
}

impl Default for ProviderContinuationRef {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for ProviderContinuationRef {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ProviderContinuationRef([REDACTED])")
    }
}

/// Backend-authoritative capabilities frozen for one logical model run.
///
/// This provider-neutral contract is intentionally separate from tool
/// definitions. Tools remain registered consistently for prompt-cache
/// stability and enforce unsupported capabilities at execution time.
#[derive(Debug, Default, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelCapabilities {
    pub image_input: bool,
}

#[derive(Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AgentChatInput {
    pub api_url: String,
    pub api_token: String,
    /// Opaque Host-only identity of this model's effective provider wire protocol.
    ///
    /// This is the current per-model `provider-protocol-v1` revision. Effective wire changes rotate
    /// it independently. It never crosses normal Serde boundaries or provider payloads.
    #[serde(skip)]
    pub provider_configuration_revision: Option<String>,
    /// Host-only stable identity of this model's effective endpoint/token pair. It is kept
    /// separately from the protocol revision for credential restoration and CAS checks.
    #[serde(skip)]
    pub provider_connection_revision: Option<String>,
    /// Host-only stable identity of the effective search mode/credential pair.
    #[serde(skip)]
    pub search_connection_revision: Option<String>,
    /// Host-only provider protocol configuration frozen before runtime preparation.
    #[serde(skip)]
    pub provider_profile_config: Option<ProviderProfileConfig>,
    /// Host-only immutable provenance for provider-owned assistant state. Hosts freeze this before
    /// a run; current direct Core callers may omit the key and derive it from the required Profile.
    #[serde(skip)]
    pub provider_protocol_key: Option<ProviderProtocolKey>,
    /// Host-only stable identity of the local model configuration selected for this run.
    ///
    /// This identity owns settings revisions, conversation selection and Usage attribution. It is
    /// deliberately separate from `model`, which is the exact provider wire model identifier.
    #[serde(skip)]
    pub model_config_id: Option<String>,
    /// Exact provider wire model identifier sent in the request payload.
    pub model: String,
    /// Resolved by the backend from the selected model configuration and kept
    /// immutable across approval pause/resume for this logical run.
    pub model_capabilities: ModelCapabilities,
    pub api_style: Option<AgentApiStyle>,
    #[serde(default)]
    pub context_window_tokens: Option<u32>,
    #[serde(default = "default_true")]
    pub context_window_indicator_enabled: bool,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub stream: Option<bool>,
    pub context: Option<AgentRunContext>,
    pub search_config: Option<AgentSearchConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_preferences: Option<AgentPromptPreferences>,
    pub approval_decision: Option<AgentApprovalDecision>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_continuation: Option<AgentToolContinuation>,
    #[serde(default)]
    pub attachments: Vec<AgentInputAttachment>,
    /// Host-hydrated immutable historical image references. These are input bytes only;
    /// model journals and checkpoints persist references, never this collection.
    #[serde(skip)]
    pub context_image_attachments: Vec<AgentInputAttachment>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resume_checkpoint: Option<AgentRunCheckpoint>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assistant_message_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_compaction_summary: Option<ContextCompactionSummary>,
    /// Backend-owned, provider-neutral world-state journal for the active conversation epoch.
    ///
    /// A full snapshot establishes the epoch prelude and later diffs are anchored immediately
    /// before the conversation message that first observed them. The runtime renders only each
    /// record's explicitly sanitized model projection.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub world_state_records: Vec<AnchoredWorldStateRecord>,
    /// Immutable Skill snapshots selected for this logical agent run. The runtime treats this as
    /// dynamic run context; it is deliberately excluded from the stable system prompt and the
    /// conversation context configuration fingerprint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill_activation: Option<AgentSkillActivation>,
    /// Backend-authoritative, run-scoped metadata for globally enabled Skills that the model may
    /// choose to activate. Full Skill instructions and resource authority are intentionally absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill_discovery: Option<crate::skills::AgentSkillDiscoverySnapshot>,
    pub messages: Vec<AgentChatMessage>,
}

impl std::fmt::Debug for AgentChatInput {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("AgentChatInput([REDACTED])")
    }
}

#[derive(Default, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentSkillActivation {
    pub activation_revision: String,
    pub skills: Vec<AgentActivatedSkill>,
}

impl std::fmt::Debug for AgentSkillActivation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AgentSkillActivation")
            .field("activation_revision", &self.activation_revision)
            .field("skills", &self.skills)
            .finish()
    }
}

#[derive(Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentActivatedSkill {
    pub id: String,
    pub name: String,
    pub revision: String,
    pub source: String,
    pub instructions: String,
    /// Exact verified SKILL.md source size used for aggregate activation policy enforcement.
    pub source_bytes: u64,
    /// Lightweight discovery hint for the run-scoped Resource Runtime. The
    /// resource index and bytes remain behind the host capability.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resources: Option<AgentActivatedSkillResources>,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentActivatedSkillResources {
    pub root_uri: String,
    pub resource_count: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub kinds: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentSkillActivationActor {
    User,
    Model,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentSkillSourceSummary {
    pub kind: String,
    pub id: String,
}

/// The same presentation-safe Skill identity returned for explicit activation at turn start.
/// Runtime activation metadata deliberately excludes instructions and resource locations.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentSkillActivatedEvent {
    pub id: String,
    pub name: String,
    pub revision: String,
    pub source: AgentSkillSourceSummary,
}

impl std::fmt::Debug for AgentActivatedSkill {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AgentActivatedSkill")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("revision", &self.revision)
            .field("source", &self.source)
            .field("instructions_bytes", &self.instructions.len())
            .field("resources", &self.resources)
            .finish()
    }
}
