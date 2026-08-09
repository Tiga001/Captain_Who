mod adapter;
mod payload;
mod provider_cooldown;
mod provider_error;
mod response;
mod stream;
mod tool_call_id;
mod transport;

pub(crate) use tool_call_id::{
    model_response_tool_call_id, validate_model_tool_call_id, validate_model_tool_protocol,
    validate_provider_tool_call_id,
};
pub(crate) use transport::{
    complete_chat, complete_chat_allow_empty, complete_chat_streaming,
    complete_chat_streaming_allow_empty, is_repairable_empty_model_action,
};

use crate::cancellation::AgentCancellationToken;
use crate::protocol::{
    AgentApiStyle, AgentAssistantTurnCheckpointIdentity, AgentError, AgentProviderToolCallIdentity,
    AgentResult, AgentToolDefinition, AgentUsage, ProviderContinuationRef,
};
use crate::provider_profile::{ProviderProfileConfig, ProviderProfileId, ProviderProtocolKey};
use crate::tools::schema::validate_portable_tool_input_schema;
use crate::usage::{
    merge_total_usage, merge_total_usage_with_disjoint_reasoning, usage_for_request,
};
use adapter::ProviderAdapterRegistry;
use payload::is_sse_response;
use provider_cooldown::{
    acquire_provider_cooldown, complete_provider_cooldown, register_default_rate_limit_cooldown,
    register_provider_cooldown, unix_epoch_ms,
};
use provider_error::{
    provider_failure_metadata, public_provider_error_message, LlmProviderFailure,
    LlmProviderFailureCategory, PROVIDER_FAILURE_ERROR_CODE,
};
use response::{extract_api_error, extract_finish_reason};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::time::Duration;
use stream::parse_sse_response;
#[cfg(test)]
use stream::{process_sse_frame, LlmStreamAccumulator};
use zeroize::Zeroizing;

/// Upper bound for an opaque Tool Call identity received from a model provider.
///
/// Provider IDs are preserved byte-for-byte. This limit is measured on the UTF-8 representation
/// solely to bound memory, hashing and checkpoint amplification.
pub(crate) const MAX_PROVIDER_TOOL_CALL_ID_BYTES: usize = 16 * 1024;

#[derive(Clone)]
pub(crate) struct LlmChatRequest {
    pub api_url: String,
    pub api_token: String,
    pub provider_profile: ProviderProfileConfig,
    pub provider_protocol: ProviderProtocolKey,
    pub max_tokens: u32,
    pub temperature: f32,
    pub stream: bool,
    pub messages: Vec<LlmMessage>,
    pub tools: Vec<AgentToolDefinition>,
}

impl LlmChatRequest {
    pub(crate) fn model(&self) -> &str {
        &self.provider_protocol.model_id
    }

    pub(crate) fn api_style(&self) -> AgentApiStyle {
        self.provider_protocol.dialect.api_style()
    }
}

impl std::fmt::Debug for LlmChatRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("LlmChatRequest([REDACTED])")
    }
}

#[derive(Clone)]
pub(crate) struct LlmChatResponse {
    pub assistant_turn: LlmAssistantTurn,
    pub usage: Option<AgentUsage>,
    pub finish_reason: Option<String>,
}

impl LlmChatResponse {
    pub(crate) fn content(&self) -> &str {
        self.assistant_turn.visible_text()
    }

    pub(crate) fn provider_tool_calls(&self) -> &[LlmToolCall] {
        self.assistant_turn.provider_tool_calls()
    }
}

impl std::fmt::Debug for LlmChatResponse {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("LlmChatResponse([REDACTED])")
    }
}

#[derive(Clone, PartialEq)]
pub(crate) struct LlmMessage {
    body: LlmMessageBody,
    placement: LlmMessagePlacement,
}

#[derive(Clone, PartialEq)]
enum LlmMessageBody {
    Text {
        role: LlmMessageRole,
        content: String,
        images: Vec<LlmImage>,
    },
    Assistant(Box<LlmAssistantTurn>),
    ToolResult {
        tool_call_id: String,
        content: String,
        is_error: bool,
    },
}

impl std::fmt::Debug for LlmMessage {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("LlmMessage([REDACTED])")
    }
}

impl LlmMessage {
    pub(crate) fn text(role: LlmMessageRole, content: impl Into<String>) -> Self {
        let content = content.into();
        if role == LlmMessageRole::Assistant {
            return Self::from_assistant_turn(LlmAssistantTurn::from_legacy(content, Vec::new()));
        }
        assert_ne!(
            role,
            LlmMessageRole::Tool,
            "tool messages must use LlmMessage::tool_result"
        );
        Self {
            body: LlmMessageBody::Text {
                role,
                content,
                images: Vec::new(),
            },
            placement: LlmMessagePlacement::default_for_role(role),
        }
    }

    /// Creates a backend-authoritative state record that remains at its chronological
    /// position in provider payloads. It is deliberately not a system instruction.
    pub(crate) fn backend_state(content: impl Into<String>) -> Self {
        let mut message = Self::text(LlmMessageRole::System, content);
        message.set_placement(LlmMessagePlacement::BackendStateTimeline);
        message
    }

    pub(crate) fn assistant(content: impl Into<String>, tool_calls: Vec<LlmToolCall>) -> Self {
        Self::from_assistant_turn(LlmAssistantTurn::from_legacy(content, tool_calls))
    }

    pub(crate) fn from_assistant_turn(turn: LlmAssistantTurn) -> Self {
        Self {
            body: LlmMessageBody::Assistant(Box::new(turn)),
            placement: LlmMessagePlacement::OrdinaryTimeline,
        }
    }

    pub(crate) fn tool_result(
        tool_call_id: impl Into<String>,
        content: impl Into<String>,
        is_error: bool,
    ) -> Self {
        Self {
            body: LlmMessageBody::ToolResult {
                tool_call_id: tool_call_id.into(),
                content: content.into(),
                is_error,
            },
            placement: LlmMessagePlacement::OrdinaryTimeline,
        }
    }

    pub(crate) fn role(&self) -> LlmMessageRole {
        match &self.body {
            LlmMessageBody::Text { role, .. } => *role,
            LlmMessageBody::Assistant(_) => LlmMessageRole::Assistant,
            LlmMessageBody::ToolResult { .. } => LlmMessageRole::Tool,
        }
    }

    pub(crate) fn content(&self) -> &str {
        match &self.body {
            LlmMessageBody::Text { content, .. } | LlmMessageBody::ToolResult { content, .. } => {
                content
            }
            LlmMessageBody::Assistant(turn) => turn.visible_text(),
        }
    }

    pub(crate) fn images(&self) -> &[LlmImage] {
        match &self.body {
            LlmMessageBody::Text { images, .. } => images,
            LlmMessageBody::Assistant(_) | LlmMessageBody::ToolResult { .. } => &[],
        }
    }

    pub(crate) fn images_mut(&mut self) -> Option<&mut Vec<LlmImage>> {
        match &mut self.body {
            LlmMessageBody::Text { images, .. } => Some(images),
            LlmMessageBody::Assistant(_) | LlmMessageBody::ToolResult { .. } => None,
        }
    }

    pub(crate) fn assistant_turn(&self) -> Option<&LlmAssistantTurn> {
        match &self.body {
            LlmMessageBody::Assistant(turn) => Some(turn.as_ref()),
            LlmMessageBody::Text { .. } | LlmMessageBody::ToolResult { .. } => None,
        }
    }

    /// Narrow mutable access for the runtime approval barrier to filter runtime-only bindings;
    /// provider-owned turn fields remain private and immutable outside this module.
    pub(crate) fn assistant_turn_mut(&mut self) -> Option<&mut LlmAssistantTurn> {
        match &mut self.body {
            LlmMessageBody::Assistant(turn) => Some(turn.as_mut()),
            LlmMessageBody::Text { .. } | LlmMessageBody::ToolResult { .. } => None,
        }
    }

    pub(crate) fn tool_calls(&self) -> LlmEffectiveToolCalls<'_> {
        match &self.body {
            LlmMessageBody::Assistant(turn) => turn.effective_tool_calls(),
            LlmMessageBody::Text { .. } | LlmMessageBody::ToolResult { .. } => {
                LlmEffectiveToolCalls::Empty
            }
        }
    }

    pub(crate) fn tool_call_id(&self) -> Option<&str> {
        self.tool_result_fields()
            .map(|(tool_call_id, _, _)| tool_call_id)
    }

    pub(crate) fn is_error(&self) -> bool {
        self.tool_result_fields()
            .map(|(_, _, is_error)| is_error)
            .unwrap_or(false)
    }

    pub(crate) fn tool_result_fields(&self) -> Option<(&str, &str, bool)> {
        match &self.body {
            LlmMessageBody::ToolResult {
                tool_call_id,
                content,
                is_error,
            } => Some((tool_call_id, content, *is_error)),
            LlmMessageBody::Text { .. } | LlmMessageBody::Assistant(_) => None,
        }
    }

    pub(crate) fn placement(&self) -> LlmMessagePlacement {
        self.placement
    }

    pub(crate) fn set_placement(&mut self, placement: LlmMessagePlacement) {
        self.placement = placement;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LlmMessageRole {
    System,
    User,
    Assistant,
    Tool,
}

/// Provider-neutral placement metadata.
///
/// This is intentionally separate from the provider message role: backend state is trusted
/// application data, but it is not a system instruction and therefore must remain in the
/// chronological message timeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum LlmMessagePlacement {
    StableSystemPolicy,
    BackendStateTimeline,
    OrdinaryTimeline,
}

impl LlmMessagePlacement {
    pub(crate) fn default_for_role(role: LlmMessageRole) -> Self {
        match role {
            LlmMessageRole::System => Self::StableSystemPolicy,
            LlmMessageRole::User | LlmMessageRole::Assistant | LlmMessageRole::Tool => {
                Self::OrdinaryTimeline
            }
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct LlmImage {
    pub mime_type: String,
    pub data_base64: String,
}

impl std::fmt::Debug for LlmImage {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("LlmImage([REDACTED])")
    }
}

impl LlmMessageRole {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::Tool => "tool",
        }
    }
}

#[derive(Clone, PartialEq)]
pub(crate) struct LlmToolCall {
    pub id: String,
    pub name: String,
    pub args: Value,
}

#[derive(Clone, PartialEq)]
pub(crate) struct LlmRuntimeToolCallBinding {
    pub provider_tool_index: usize,
    pub provider_call_id: String,
    pub runtime_call: LlmToolCall,
}

impl LlmRuntimeToolCallBinding {
    pub(crate) fn new(
        provider_tool_index: usize,
        provider_call: &LlmToolCall,
        runtime_call: LlmToolCall,
    ) -> Self {
        Self {
            provider_tool_index,
            provider_call_id: provider_call.id.clone(),
            runtime_call,
        }
    }
}

impl std::fmt::Debug for LlmRuntimeToolCallBinding {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("LlmRuntimeToolCallBinding([REDACTED])")
    }
}

/// Round 1 protocol foundation, consumed by provider adapters starting in Round 2.
#[allow(dead_code)]
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum ProviderContinuationAttachment {
    AssistantTurn,
    ContentBlock(u32),
    ToolCall(u32),
    InteractionStep(u32),
}

impl std::fmt::Debug for ProviderContinuationAttachment {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ProviderContinuationAttachment([REDACTED])")
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct ProviderContinuationPosition {
    pub sequence: u32,
    pub attachment: ProviderContinuationAttachment,
}

impl ProviderContinuationPosition {
    /// Round 1 protocol foundation, consumed by provider adapters starting in Round 2.
    #[allow(dead_code)]
    pub(crate) const fn new(sequence: u32, attachment: ProviderContinuationAttachment) -> Self {
        Self {
            sequence,
            attachment,
        }
    }
}

impl std::fmt::Debug for ProviderContinuationPosition {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ProviderContinuationPosition([REDACTED])")
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct ReasoningProjection {
    position: ProviderContinuationPosition,
    summary: String,
}

impl ReasoningProjection {
    /// Round 1 protocol foundation, consumed by provider adapters starting in Round 2.
    #[allow(dead_code)]
    pub(crate) fn summary(
        position: ProviderContinuationPosition,
        summary: impl Into<String>,
    ) -> Self {
        Self {
            position,
            summary: summary.into(),
        }
    }

    pub(crate) fn position(&self) -> ProviderContinuationPosition {
        self.position
    }

    pub(crate) fn summary_text(&self) -> &str {
        &self.summary
    }
}

impl std::fmt::Debug for ReasoningProjection {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ReasoningProjection([REDACTED])")
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct LlmAssistantTurnDigest([u8; 32]);

impl std::fmt::Debug for LlmAssistantTurnDigest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("LlmAssistantTurnDigest([REDACTED])")
    }
}

/// Round 1 protocol foundation, consumed by provider adapters starting in Round 2.
#[allow(dead_code)]
pub(crate) const MAX_PROVIDER_CONTINUATION_BYTES: usize = 1024 * 1024;
/// Round 1 protocol foundation, consumed by provider adapters starting in Round 2.
#[allow(dead_code)]
const MAX_PROVIDER_CONTINUATION_FRAGMENTS: usize = 4096;

/// Round 1 protocol foundation, consumed by provider adapters starting in Round 2.
#[allow(dead_code)]
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ProviderContinuationReplayScope {
    AssistantTurnV1,
    InteractionV1,
}

impl std::fmt::Debug for ProviderContinuationReplayScope {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ProviderContinuationReplayScope([REDACTED])")
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct ProviderContinuationFragment {
    position: ProviderContinuationPosition,
    opaque: Zeroizing<Vec<u8>>,
}

/// Round 1 protocol foundation, consumed by provider adapters starting in Round 2.
#[allow(dead_code)]
impl ProviderContinuationFragment {
    pub(super) fn new(position: ProviderContinuationPosition, opaque: Vec<u8>) -> Self {
        Self {
            position,
            opaque: Zeroizing::new(opaque),
        }
    }

    pub(super) fn position(&self) -> ProviderContinuationPosition {
        self.position
    }

    pub(super) fn opaque(&self) -> &[u8] {
        self.opaque.as_slice()
    }
}

impl std::fmt::Debug for ProviderContinuationFragment {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ProviderContinuationFragment([REDACTED])")
    }
}

/// Provider-owned replay state. Intentionally does not implement Serde; only redacted checkpoint
/// identity and digest metadata may leave the live AssistantTurn.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct ProviderContinuation {
    provenance: ProviderProtocolKey,
    replay_scope: ProviderContinuationReplayScope,
    assistant_turn_digest: LlmAssistantTurnDigest,
    payload_digest: [u8; 32],
    fragments: Vec<ProviderContinuationFragment>,
    encoded_bytes: usize,
}

/// Round 1 protocol foundation, consumed by provider adapters starting in Round 2.
#[allow(dead_code)]
impl ProviderContinuation {
    pub(super) fn new(
        provenance: ProviderProtocolKey,
        replay_scope: ProviderContinuationReplayScope,
        assistant_turn_digest: LlmAssistantTurnDigest,
        fragments: Vec<ProviderContinuationFragment>,
    ) -> AgentResult<Self> {
        provenance.validate().map_err(|error| {
            AgentError::new(format!("Provider continuation provenance 无效：{error}"))
        })?;
        if fragments.is_empty() {
            return Err(AgentError::new(
                "Provider continuation 至少需要一个 opaque fragment。",
            ));
        }
        if fragments.len() > MAX_PROVIDER_CONTINUATION_FRAGMENTS {
            return Err(AgentError::new(format!(
                "Provider continuation 超过 {} 个 fragment 上限。",
                MAX_PROVIDER_CONTINUATION_FRAGMENTS
            )));
        }

        let mut positions = BTreeSet::new();
        let mut sequences = BTreeSet::new();
        let mut previous_position = None;
        let mut encoded_bytes = 0usize;
        for fragment in &fragments {
            if fragment.opaque().is_empty() {
                return Err(AgentError::new("Provider continuation fragment 不能为空。"));
            }
            if !positions.insert(fragment.position()) {
                return Err(AgentError::new(
                    "Provider continuation 包含重复的协议位置。",
                ));
            }
            if !sequences.insert(fragment.position().sequence) {
                return Err(AgentError::new(
                    "Provider continuation 包含重复的原始序号。",
                ));
            }
            if previous_position.is_some_and(|previous| previous >= fragment.position()) {
                return Err(AgentError::new(
                    "Provider continuation 的协议位置未保持严格原序。",
                ));
            }
            previous_position = Some(fragment.position());
            encoded_bytes = encoded_bytes
                .checked_add(fragment.opaque().len())
                .ok_or_else(|| AgentError::new("Provider continuation 大小溢出。"))?;
            if encoded_bytes > MAX_PROVIDER_CONTINUATION_BYTES {
                return Err(AgentError::new(format!(
                    "Provider continuation 超过 {} 字节上限。",
                    MAX_PROVIDER_CONTINUATION_BYTES
                )));
            }
        }

        let payload_digest = continuation_payload_digest(replay_scope, &fragments);

        Ok(Self {
            provenance,
            replay_scope,
            assistant_turn_digest,
            payload_digest,
            fragments,
            encoded_bytes,
        })
    }

    pub(crate) fn provenance(&self) -> &ProviderProtocolKey {
        &self.provenance
    }

    pub(crate) fn encoded_bytes(&self) -> usize {
        self.encoded_bytes
    }

    pub(crate) fn replay_scope(&self) -> ProviderContinuationReplayScope {
        self.replay_scope
    }

    pub(crate) fn payload_hash(&self) -> String {
        format!("sha256:{}", hex_bytes(&self.payload_digest))
    }

    pub(super) fn fragments(&self) -> &[ProviderContinuationFragment] {
        &self.fragments
    }

    pub(crate) fn validate_for(
        &self,
        provenance: &ProviderProtocolKey,
        assistant_turn_digest: LlmAssistantTurnDigest,
    ) -> AgentResult<()> {
        provenance.validate().map_err(|error| {
            AgentError::new(format!("Provider continuation 目标协议无效：{error}"))
        })?;
        if &self.provenance != provenance {
            return Err(AgentError::new(
                "Provider continuation 与当前 provider 协议来源不一致。",
            ));
        }
        if self.assistant_turn_digest != assistant_turn_digest {
            return Err(AgentError::new(
                "Provider continuation 与当前 assistant turn 不一致。",
            ));
        }
        Ok(())
    }
}

impl std::fmt::Debug for ProviderContinuation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ProviderContinuation([REDACTED])")
    }
}

#[derive(Clone, PartialEq)]
pub(crate) struct LlmAssistantTurn {
    provider_protocol: Option<ProviderProtocolKey>,
    provider_visible_text: String,
    runtime_visible_text: Option<String>,
    provider_tool_calls: Vec<LlmToolCall>,
    runtime_tool_bindings: Option<Vec<LlmRuntimeToolCallBinding>>,
    reasoning: Vec<ReasoningProjection>,
    provider_continuation: Option<ProviderContinuation>,
    /// Safe durable handle for the raw continuation held by the Host vault. The handle is not
    /// provider wire state and therefore never participates in the assistant-turn digest.
    provider_continuation_ref: Option<ProviderContinuationRef>,
}

impl LlmAssistantTurn {
    pub(crate) fn from_provider(
        provider_protocol: ProviderProtocolKey,
        visible_text: impl Into<String>,
        provider_tool_calls: Vec<LlmToolCall>,
    ) -> AgentResult<Self> {
        provider_protocol.validate().map_err(|error| {
            AgentError::new(format!("Assistant turn provider 协议无效：{error}"))
        })?;
        for call in &provider_tool_calls {
            validate_provider_tool_call_id(&call.id)?;
        }
        Ok(Self {
            provider_protocol: Some(provider_protocol),
            provider_visible_text: visible_text.into(),
            runtime_visible_text: None,
            provider_tool_calls,
            runtime_tool_bindings: None,
            reasoning: Vec::new(),
            provider_continuation: None,
            provider_continuation_ref: None,
        })
    }

    pub(crate) fn from_legacy(
        visible_text: impl Into<String>,
        provider_tool_calls: Vec<LlmToolCall>,
    ) -> Self {
        Self {
            provider_protocol: None,
            provider_visible_text: visible_text.into(),
            runtime_visible_text: None,
            provider_tool_calls,
            runtime_tool_bindings: None,
            reasoning: Vec::new(),
            provider_continuation: None,
            provider_continuation_ref: None,
        }
    }

    pub(crate) fn provider_protocol(&self) -> Option<&ProviderProtocolKey> {
        self.provider_protocol.as_ref()
    }

    pub(crate) fn visible_text(&self) -> &str {
        self.runtime_visible_text
            .as_deref()
            .unwrap_or(&self.provider_visible_text)
    }

    pub(crate) fn provider_visible_text(&self) -> &str {
        &self.provider_visible_text
    }

    pub(crate) fn runtime_visible_text(&self) -> Option<&str> {
        self.runtime_visible_text.as_deref()
    }

    pub(crate) fn set_runtime_visible_text(&mut self, visible_text: impl Into<String>) {
        self.runtime_visible_text = Some(visible_text.into());
    }

    pub(crate) fn provider_tool_calls(&self) -> &[LlmToolCall] {
        &self.provider_tool_calls
    }

    pub(crate) fn runtime_tool_bindings(&self) -> Option<&[LlmRuntimeToolCallBinding]> {
        self.runtime_tool_bindings.as_deref()
    }

    /// Round 1 protocol foundation, consumed by provider adapters starting in Round 2.
    #[allow(dead_code)]
    pub(crate) fn reasoning(&self) -> &[ReasoningProjection] {
        &self.reasoning
    }

    pub(crate) fn provider_continuation(&self) -> Option<&ProviderContinuation> {
        self.provider_continuation.as_ref()
    }

    pub(crate) fn provider_continuation_ref(&self) -> Option<&ProviderContinuationRef> {
        self.provider_continuation_ref.as_ref()
    }

    pub(crate) fn effective_tool_calls(&self) -> LlmEffectiveToolCalls<'_> {
        match &self.runtime_tool_bindings {
            Some(bindings) => LlmEffectiveToolCalls::Runtime(bindings.iter()),
            None => LlmEffectiveToolCalls::Provider(self.provider_tool_calls.iter()),
        }
    }

    pub(crate) fn with_runtime_tool_bindings(
        mut self,
        bindings: Vec<LlmRuntimeToolCallBinding>,
    ) -> AgentResult<Self> {
        self.set_runtime_tool_bindings(bindings)?;
        Ok(self)
    }

    pub(crate) fn set_runtime_tool_bindings(
        &mut self,
        bindings: Vec<LlmRuntimeToolCallBinding>,
    ) -> AgentResult<()> {
        for provider_call in &self.provider_tool_calls {
            validate_provider_tool_call_id(&provider_call.id)?;
        }
        let mut provider_indices = BTreeSet::new();
        let mut runtime_call_ids = BTreeSet::new();
        for binding in &bindings {
            validate_provider_tool_call_id(&binding.provider_call_id)?;
            let provider_call = self
                .provider_tool_calls
                .get(binding.provider_tool_index)
                .ok_or_else(|| {
                    AgentError::new("Runtime tool binding 引用了不存在的 provider tool call。")
                })?;
            if provider_call.id != binding.provider_call_id {
                return Err(AgentError::new(
                    "Runtime tool binding 的 provider tool call 身份不匹配。",
                ));
            }
            if !provider_indices.insert(binding.provider_tool_index) {
                return Err(AgentError::new(
                    "Runtime tool binding 重复引用同一个 provider tool call。",
                ));
            }
            if binding.runtime_call.id.trim().is_empty()
                || !runtime_call_ids.insert(binding.runtime_call.id.as_str())
            {
                return Err(AgentError::new(
                    "Runtime tool binding 包含空或重复的 runtime tool call id。",
                ));
            }
        }
        self.runtime_tool_bindings = Some(bindings);
        Ok(())
    }

    /// Round 1 protocol foundation, consumed by provider adapters starting in Round 2.
    #[allow(dead_code)]
    pub(crate) fn with_reasoning(
        mut self,
        reasoning: Vec<ReasoningProjection>,
    ) -> AgentResult<Self> {
        self.reasoning = reasoning;
        if let Some(continuation) = &self.provider_continuation {
            let provider_protocol = self.provider_protocol.as_ref().ok_or_else(|| {
                AgentError::new("Legacy assistant turn 不能携带 provider continuation。")
            })?;
            continuation.validate_for(provider_protocol, self.digest())?;
        }
        Ok(self)
    }

    /// Round 1 protocol foundation, consumed by provider adapters starting in Round 2.
    #[allow(dead_code)]
    pub(crate) fn with_provider_continuation(
        mut self,
        continuation: ProviderContinuation,
    ) -> AgentResult<Self> {
        let provider_protocol = self.provider_protocol.as_ref().ok_or_else(|| {
            AgentError::new("Legacy assistant turn 不能携带 provider continuation。")
        })?;
        continuation.validate_for(provider_protocol, self.digest())?;
        self.provider_continuation = Some(continuation);
        Ok(self)
    }

    pub(crate) fn with_provider_continuation_ref(
        mut self,
        continuation_ref: ProviderContinuationRef,
    ) -> AgentResult<Self> {
        continuation_ref
            .validate()
            .map_err(|error| AgentError::new(format!("Provider continuation ref 无效：{error}")))?;
        if self.provider_protocol.is_none() {
            return Err(AgentError::new(
                "Legacy assistant turn 不能引用 provider continuation。",
            ));
        }
        self.provider_continuation_ref = Some(continuation_ref);
        Ok(self)
    }

    pub(crate) fn without_raw_continuation_for_checkpoint(&self) -> Self {
        let mut projected = self.clone();
        projected.provider_continuation = None;
        projected
    }

    pub(crate) fn digest(&self) -> LlmAssistantTurnDigest {
        let mut hasher = Sha256::new();
        hash_len_prefixed(&mut hasher, b"llm-assistant-turn-v1");
        if let Some(protocol) = &self.provider_protocol {
            hash_len_prefixed(
                &mut hasher,
                serde_json::to_string(protocol)
                    .unwrap_or_default()
                    .as_bytes(),
            );
        } else {
            hash_len_prefixed(&mut hasher, b"legacy");
        }
        hash_len_prefixed(&mut hasher, self.provider_visible_text.as_bytes());
        for call in &self.provider_tool_calls {
            hash_len_prefixed(&mut hasher, call.id.as_bytes());
            hash_len_prefixed(&mut hasher, call.name.as_bytes());
            let args = serde_json::to_vec(&call.args).unwrap_or_else(|_| b"null".to_vec());
            hash_len_prefixed(&mut hasher, &args);
        }
        for projection in &self.reasoning {
            hash_continuation_position(&mut hasher, projection.position());
            hash_len_prefixed(&mut hasher, projection.summary_text().as_bytes());
        }
        LlmAssistantTurnDigest(hasher.finalize().into())
    }

    pub(crate) fn stable_id(&self) -> String {
        format!("at1_{}", hex_bytes(&self.digest().0))
    }

    pub(crate) fn stable_digest(&self) -> String {
        format!("sha256:{}", hex_bytes(&self.digest().0))
    }

    /// Returns opaque-safe material for Context/cache revision hashing.
    ///
    /// The digest covers the provider-owned turn, runtime-visible projection and ordered runtime
    /// bindings. When present, continuation provenance binding, replay scope, encoded byte count
    /// and payload hash are included, but the opaque continuation bytes are never returned.
    pub(crate) fn context_revision_material(&self) -> String {
        let mut hasher = Sha256::new();
        hash_len_prefixed(&mut hasher, b"llm-assistant-context-revision-v1");
        hasher.update(self.digest().0);

        match &self.runtime_visible_text {
            Some(visible_text) => {
                hasher.update([1]);
                hash_len_prefixed(&mut hasher, visible_text.as_bytes());
            }
            None => hasher.update([0]),
        }

        match &self.runtime_tool_bindings {
            Some(bindings) => {
                hasher.update([1]);
                hasher.update((bindings.len() as u64).to_be_bytes());
                for binding in bindings {
                    hasher.update((binding.provider_tool_index as u64).to_be_bytes());
                    hash_len_prefixed(&mut hasher, binding.provider_call_id.as_bytes());
                    hash_len_prefixed(&mut hasher, binding.runtime_call.id.as_bytes());
                    hash_len_prefixed(&mut hasher, binding.runtime_call.name.as_bytes());
                    let args = serde_json::to_vec(&binding.runtime_call.args)
                        .unwrap_or_else(|_| b"null".to_vec());
                    hash_len_prefixed(&mut hasher, &args);
                }
            }
            None => hasher.update([0]),
        }

        match &self.provider_continuation {
            Some(continuation) => {
                hasher.update([1]);
                hasher.update([match continuation.replay_scope {
                    ProviderContinuationReplayScope::AssistantTurnV1 => 0,
                    ProviderContinuationReplayScope::InteractionV1 => 1,
                }]);
                hasher.update(continuation.assistant_turn_digest.0);
                hasher.update(continuation.payload_digest);
                hasher.update((continuation.encoded_bytes as u64).to_be_bytes());
            }
            None => hasher.update([0]),
        }
        match &self.provider_continuation_ref {
            Some(continuation_ref) => {
                hasher.update([1]);
                hash_len_prefixed(&mut hasher, continuation_ref.id.as_bytes());
            }
            None => hasher.update([0]),
        }

        format!("sha256:{}", hex_bytes(&hasher.finalize()))
    }

    pub(crate) fn checkpoint_identity(&self) -> AgentResult<AgentAssistantTurnCheckpointIdentity> {
        let tool_call_identities = match &self.runtime_tool_bindings {
            Some(bindings) => bindings
                .iter()
                .map(|binding| {
                    validate_provider_tool_call_id(&binding.provider_call_id)?;
                    Ok(AgentProviderToolCallIdentity {
                        provider_tool_index: u32::try_from(binding.provider_tool_index).map_err(
                            |_| AgentError::new("Provider tool call index 超出 checkpoint 范围。"),
                        )?,
                        provider_call_id: binding.provider_call_id.clone(),
                        runtime_call_id: binding.runtime_call.id.clone(),
                    })
                })
                .collect::<AgentResult<Vec<_>>>()?,
            None => self
                .provider_tool_calls
                .iter()
                .enumerate()
                .map(|(index, call)| {
                    validate_provider_tool_call_id(&call.id)?;
                    Ok(AgentProviderToolCallIdentity {
                        provider_tool_index: u32::try_from(index).map_err(|_| {
                            AgentError::new("Provider tool call index 超出 checkpoint 范围。")
                        })?,
                        provider_call_id: call.id.clone(),
                        runtime_call_id: call.id.clone(),
                    })
                })
                .collect::<AgentResult<Vec<_>>>()?,
        };
        Ok(AgentAssistantTurnCheckpointIdentity {
            assistant_turn_id: self.stable_id(),
            assistant_turn_digest: self.stable_digest(),
            tool_call_identities,
        })
    }
}

impl std::fmt::Debug for LlmAssistantTurn {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("LlmAssistantTurn([REDACTED])")
    }
}

pub(crate) enum LlmEffectiveToolCalls<'a> {
    Empty,
    Provider(std::slice::Iter<'a, LlmToolCall>),
    Runtime(std::slice::Iter<'a, LlmRuntimeToolCallBinding>),
}

impl LlmEffectiveToolCalls<'_> {
    pub(crate) fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl<'a> Iterator for LlmEffectiveToolCalls<'a> {
    type Item = &'a LlmToolCall;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Empty => None,
            Self::Provider(calls) => calls.next(),
            Self::Runtime(bindings) => bindings.next().map(|binding| &binding.runtime_call),
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let length = match self {
            Self::Empty => 0,
            Self::Provider(calls) => calls.len(),
            Self::Runtime(bindings) => bindings.len(),
        };
        (length, Some(length))
    }
}

impl ExactSizeIterator for LlmEffectiveToolCalls<'_> {}

fn hash_len_prefixed(hasher: &mut Sha256, value: &[u8]) {
    hasher.update((value.len() as u64).to_be_bytes());
    hasher.update(value);
}

fn hash_continuation_position(hasher: &mut Sha256, position: ProviderContinuationPosition) {
    hasher.update(position.sequence.to_be_bytes());
    match position.attachment {
        ProviderContinuationAttachment::AssistantTurn => hasher.update([0]),
        ProviderContinuationAttachment::ContentBlock(index) => {
            hasher.update([1]);
            hasher.update(index.to_be_bytes());
        }
        ProviderContinuationAttachment::ToolCall(index) => {
            hasher.update([2]);
            hasher.update(index.to_be_bytes());
        }
        ProviderContinuationAttachment::InteractionStep(index) => {
            hasher.update([3]);
            hasher.update(index.to_be_bytes());
        }
    }
}

/// Round 1 protocol foundation, consumed by provider adapters starting in Round 2.
#[allow(dead_code)]
fn continuation_payload_digest(
    replay_scope: ProviderContinuationReplayScope,
    fragments: &[ProviderContinuationFragment],
) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hash_len_prefixed(&mut hasher, b"provider-continuation-v1");
    hasher.update([match replay_scope {
        ProviderContinuationReplayScope::AssistantTurnV1 => 0,
        ProviderContinuationReplayScope::InteractionV1 => 1,
    }]);
    for fragment in fragments {
        hash_continuation_position(&mut hasher, fragment.position());
        hash_len_prefixed(&mut hasher, fragment.opaque());
    }
    hasher.finalize().into()
}

fn hex_bytes(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

impl std::fmt::Debug for LlmToolCall {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("LlmToolCall([REDACTED])")
    }
}

#[derive(Clone)]
pub(crate) enum LlmStreamEvent {
    AttemptStarted {
        attempt: usize,
        max_attempts: usize,
    },
    Delta(String),
    ToolInputProgress {
        tool_call_index: usize,
        tool: String,
        input_delta: String,
        received_bytes: u64,
    },
    AttemptReset {
        reason: String,
    },
    Retrying {
        attempt: usize,
        max_attempts: usize,
        category: String,
        provider_code: Option<String>,
        delay_ms: u64,
        retry_at: u64,
        reason: String,
    },
    Committed,
}

impl std::fmt::Debug for LlmStreamEvent {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("LlmStreamEvent([REDACTED])")
    }
}

pub(crate) fn detect_api_style(api_url: &str) -> AgentApiStyle {
    let normalized = api_url.to_ascii_lowercase();

    if normalized.contains("/chat/completions") {
        return AgentApiStyle::OpenAiCompatible;
    }

    if normalized.contains("anthropic") || normalized.ends_with("/messages") {
        return AgentApiStyle::AnthropicCompatible;
    }

    AgentApiStyle::OpenAiCompatible
}

pub(crate) fn estimate_assistant_turn_continuation_tokens(
    turn: &LlmAssistantTurn,
) -> AgentResult<u64> {
    let Some(continuation) = turn.provider_continuation() else {
        return Ok(0);
    };
    let provider_protocol = turn
        .provider_protocol()
        .ok_or_else(|| AgentError::new("Legacy assistant turn 不能估算 provider continuation。"))?;
    ProviderAdapterRegistry::resolve_key(provider_protocol)?
        .estimate_continuation_tokens(Some(continuation))
}

#[cfg(test)]
mod tests;
