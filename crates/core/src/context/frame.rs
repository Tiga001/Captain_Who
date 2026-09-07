use super::measurement::{
    combine_context_revisions, ContextEstimatorIdentity, ContextMessageEstimate,
    ContextRevisionHasher, ContextTokenEstimator,
};
use super::ContextJournalCursor;
use crate::llm::LlmRuntimeToolCallBinding;
use crate::llm::{LlmAssistantTurn, LlmMessage, LlmMessagePlacement, LlmMessageRole, LlmToolCall};
use crate::protocol::{
    AgentAssistantTurnCheckpointIdentity, AgentContextCheckpointGroup, AgentContextCheckpointImage,
    AgentContextCheckpointItem, AgentContextCheckpointOrigin, AgentContextCheckpointToolCall,
    AgentError, AgentResult,
};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

mod checkpoint;
mod measurement_state;
mod request_layout;

pub(crate) use checkpoint::*;
pub(crate) use measurement_state::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ContextScope {
    Run,
    Conversation,
}

impl ContextScope {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Run => "run",
            Self::Conversation => "conversation",
        }
    }

    fn from_str(value: &str) -> Option<Self> {
        match value {
            "run" => Some(Self::Run),
            "conversation" => Some(Self::Conversation),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ContextRetention {
    Retained,
    RequestOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ContextUsageClass {
    /// Stable request baseline such as the backend system prompt.
    Fixed,
    /// Conversation or project state that is retained for later turns.
    Durable,
    /// Active-run input kept until its durable observation can be adopted into history.
    RunTransient,
    /// Ephemeral state injected into exactly one provider request.
    RequestOnly,
}

/// Canonical journal band for one provider-neutral context item.
///
/// Journal/checkpoint assembly advances through these bands without moving backwards.
/// Provider request placement is a separate projection in `request_layout`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum ContextCacheBand {
    StableContract,
    ConversationEpochPrelude,
    DurableTimeline,
    RunTimeline,
    RequestTail,
}

impl ContextCacheBand {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::StableContract => "stable_contract",
            Self::ConversationEpochPrelude => "conversation_epoch_prelude",
            Self::DurableTimeline => "durable_timeline",
            Self::RunTimeline => "run_timeline",
            Self::RequestTail => "request_tail",
        }
    }
}

impl ContextUsageClass {
    pub(crate) fn is_persistent(self) -> bool {
        matches!(self, Self::Fixed | Self::Durable)
    }
}

impl ContextRetention {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Retained => "retained",
            Self::RequestOnly => "request_only",
        }
    }

    fn from_str(value: &str) -> Option<Self> {
        match value {
            "retained" => Some(Self::Retained),
            "request_only" => Some(Self::RequestOnly),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum ContextSource {
    BackendSystemPrompt,
    AutomationExecution,
    ConversationSummary,
    WorldStateSnapshot,
    WorldStateDiff,
    BackendState,
    /// A committed request-boundary observation which has not yet been adopted by a successful
    /// model request. Conversation ownership alone must not make this active overlay durable.
    WorldStateUnobserved,
    ConversationHistory,
    ConversationTrace,
    /// Replayable material from a previous Run, not the current capability/authority snapshot.
    HistoricalRunContext,
    /// Exact material already covered by a summary; it remains visible without reentering the
    /// next raw prefix or contributing twice to the next compaction source.
    CompactionRetained,
    /// Public narration already owned by a complete Provider assistant turn.
    ToolTurnNarration,
    CurrentTurn,
    UserGuidance,
    SkillCatalog,
    SkillInstructions,
    InputAttachment,
    ToolContinuation,
    McpToolResult,
    ModelResponse,
    ToolResult,
    RuntimeTodo,
    FileTransaction,
    RuntimeGuard,
    /// Placement-only tags. They do not change role, lifetime, retention or authority.
    RunBootstrap,
    RunInput,
    RunTimeline,
    CapabilityInstructions,
    CompactionRequest,
}

impl ContextSource {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::BackendSystemPrompt => "backend_system_prompt",
            Self::AutomationExecution => "automation_execution",
            Self::ConversationSummary => "conversation_summary",
            Self::WorldStateSnapshot => "world_state_snapshot",
            Self::WorldStateDiff => "world_state_diff",
            Self::BackendState => "backend_state",
            Self::WorldStateUnobserved => "world_state_unobserved",
            Self::ConversationHistory => "conversation_history",
            Self::ConversationTrace => "conversation_trace",
            Self::HistoricalRunContext => "historical_run_context",
            Self::CompactionRetained => "compaction_retained",
            Self::ToolTurnNarration => "tool_turn_narration",
            Self::CurrentTurn => "current_turn",
            Self::UserGuidance => "user_guidance",
            Self::SkillCatalog => "skill_catalog",
            Self::SkillInstructions => "skill_instructions",
            Self::InputAttachment => "input_attachment",
            Self::ToolContinuation => "tool_continuation",
            Self::McpToolResult => "mcp_tool_result",
            Self::ModelResponse => "model_response",
            Self::ToolResult => "tool_result",
            Self::RuntimeTodo => "runtime_todo",
            Self::FileTransaction => "file_transaction",
            Self::RuntimeGuard => "runtime_guard",
            Self::RunBootstrap => "run_bootstrap",
            Self::RunInput => "run_input",
            Self::RunTimeline => "run_timeline",
            Self::CapabilityInstructions => "capability_instructions",
            Self::CompactionRequest => "compaction_request",
        }
    }

    fn from_str(value: &str) -> Option<Self> {
        match value {
            "backend_system_prompt" => Some(Self::BackendSystemPrompt),
            "automation_execution" => Some(Self::AutomationExecution),
            "conversation_summary" => Some(Self::ConversationSummary),
            "world_state_snapshot" => Some(Self::WorldStateSnapshot),
            "world_state_diff" => Some(Self::WorldStateDiff),
            "backend_state" => Some(Self::BackendState),
            "world_state_unobserved" => Some(Self::WorldStateUnobserved),
            "conversation_history" => Some(Self::ConversationHistory),
            "conversation_trace" => Some(Self::ConversationTrace),
            "historical_run_context" => Some(Self::HistoricalRunContext),
            "compaction_retained" => Some(Self::CompactionRetained),
            "tool_turn_narration" => Some(Self::ToolTurnNarration),
            "current_turn" => Some(Self::CurrentTurn),
            "user_guidance" => Some(Self::UserGuidance),
            "skill_catalog" => Some(Self::SkillCatalog),
            "skill_instructions" => Some(Self::SkillInstructions),
            "input_attachment" => Some(Self::InputAttachment),
            "tool_continuation" => Some(Self::ToolContinuation),
            "mcp_tool_result" => Some(Self::McpToolResult),
            "model_response" => Some(Self::ModelResponse),
            "tool_result" => Some(Self::ToolResult),
            "runtime_todo" => Some(Self::RuntimeTodo),
            "file_transaction" => Some(Self::FileTransaction),
            "runtime_guard" => Some(Self::RuntimeGuard),
            "run_bootstrap" => Some(Self::RunBootstrap),
            "run_input" => Some(Self::RunInput),
            "run_timeline" => Some(Self::RunTimeline),
            "capability_instructions" => Some(Self::CapabilityInstructions),
            "compaction_request" => Some(Self::CompactionRequest),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ContextGroupKind {
    ToolExchange,
    CompactionReplacement,
}

impl ContextGroupKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::ToolExchange => "tool_exchange",
            Self::CompactionReplacement => "compaction_replacement",
        }
    }

    fn from_str(value: &str) -> Option<Self> {
        match value {
            "tool_exchange" => Some(Self::ToolExchange),
            "compaction_replacement" => Some(Self::CompactionReplacement),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ContextGroup {
    id: String,
    kind: ContextGroupKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ContextOriginKind {
    ConversationMessage,
    ConversationTraceItem,
    CompactionSummary,
    WorldStateRecord,
    Skill,
}

impl ContextOriginKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::ConversationMessage => "conversation_message",
            Self::ConversationTraceItem => "conversation_trace_item",
            Self::CompactionSummary => "compaction_summary",
            Self::WorldStateRecord => "world_state_record",
            Self::Skill => "skill",
        }
    }

    fn from_str(value: &str) -> Option<Self> {
        match value {
            "conversation_message" => Some(Self::ConversationMessage),
            "conversation_trace_item" => Some(Self::ConversationTraceItem),
            "compaction_summary" => Some(Self::CompactionSummary),
            "world_state_record" => Some(Self::WorldStateRecord),
            "skill" => Some(Self::Skill),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ContextOrigin {
    kind: ContextOriginKind,
    id: String,
}

impl ContextOrigin {
    pub(crate) fn conversation_message(id: impl Into<String>) -> Self {
        Self {
            kind: ContextOriginKind::ConversationMessage,
            id: id.into(),
        }
    }

    pub(crate) fn compaction_summary(id: impl Into<String>) -> Self {
        Self {
            kind: ContextOriginKind::CompactionSummary,
            id: id.into(),
        }
    }

    pub(crate) fn skill(id: impl Into<String>) -> Self {
        Self {
            kind: ContextOriginKind::Skill,
            id: id.into(),
        }
    }

    pub(crate) fn world_state_record(id: impl Into<String>) -> Self {
        Self {
            kind: ContextOriginKind::WorldStateRecord,
            id: id.into(),
        }
    }

    pub(crate) fn conversation_trace_item(
        assistant_message_id: impl Into<String>,
        sequence: u64,
    ) -> Self {
        let cursor = ContextJournalCursor::trace_item(assistant_message_id, sequence);
        Self {
            kind: ContextOriginKind::ConversationTraceItem,
            id: serde_json::to_string(&cursor)
                .expect("ContextJournalCursor serialization cannot fail"),
        }
    }

    pub(crate) fn kind(&self) -> ContextOriginKind {
        self.kind
    }

    pub(crate) fn id(&self) -> &str {
        &self.id
    }

    pub(crate) fn journal_cursor(&self) -> Option<ContextJournalCursor> {
        match self.kind {
            ContextOriginKind::ConversationMessage => {
                Some(ContextJournalCursor::message(self.id.clone()))
            }
            ContextOriginKind::ConversationTraceItem => serde_json::from_str(&self.id).ok(),
            ContextOriginKind::CompactionSummary
            | ContextOriginKind::WorldStateRecord
            | ContextOriginKind::Skill => None,
        }
    }
}

impl ContextGroup {
    pub(crate) fn tool_exchange(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            kind: ContextGroupKind::ToolExchange,
        }
    }

    pub(crate) fn compaction_replacement(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            kind: ContextGroupKind::CompactionReplacement,
        }
    }

    pub(crate) fn id(&self) -> &str {
        &self.id
    }

    pub(crate) fn kind(&self) -> ContextGroupKind {
        self.kind
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ContextMetadata {
    sources: Vec<ContextSource>,
    scope: ContextScope,
    retention: ContextRetention,
    group: Option<ContextGroup>,
    origin: Option<ContextOrigin>,
    /// Provider-layout sequence within the current run; never changes journal chronology.
    request_order: Option<u64>,
}

impl ContextMetadata {
    pub(crate) fn new(
        source: ContextSource,
        scope: ContextScope,
        retention: ContextRetention,
    ) -> Self {
        Self {
            sources: vec![source],
            scope,
            retention,
            group: None,
            origin: None,
            request_order: None,
        }
    }

    pub(crate) fn with_source(mut self, source: ContextSource) -> Self {
        if !self.sources.contains(&source) {
            self.sources.push(source);
        }
        self
    }

    pub(crate) fn with_group(mut self, group: ContextGroup) -> Self {
        self.group = Some(group);
        self
    }

    pub(crate) fn with_origin(mut self, origin: ContextOrigin) -> Self {
        self.origin = Some(origin);
        self
    }

    pub(crate) fn with_request_order(mut self, order: u64) -> Self {
        self.request_order = Some(order);
        self
    }

    pub(crate) fn request_order(&self) -> Option<u64> {
        self.request_order
    }

    fn replace_source(&mut self, from: ContextSource, to: ContextSource) {
        for source in &mut self.sources {
            if *source == from {
                *source = to;
            }
        }
        self.sources.sort_by_key(|source| source.as_str());
        self.sources.dedup();
    }

    pub(crate) fn sources(&self) -> &[ContextSource] {
        &self.sources
    }

    pub(crate) fn scope(&self) -> ContextScope {
        self.scope
    }

    pub(crate) fn retention(&self) -> ContextRetention {
        self.retention
    }

    pub(crate) fn group(&self) -> Option<&ContextGroup> {
        self.group.as_ref()
    }

    pub(crate) fn origin(&self) -> Option<&ContextOrigin> {
        self.origin.as_ref()
    }

    pub(crate) fn usage_class(&self) -> ContextUsageClass {
        if self.retention == ContextRetention::RequestOnly {
            return ContextUsageClass::RequestOnly;
        }
        if self.sources.contains(&ContextSource::BackendSystemPrompt) {
            return ContextUsageClass::Fixed;
        }
        if self.sources.contains(&ContextSource::WorldStateUnobserved) {
            return ContextUsageClass::RunTransient;
        }
        match self.scope {
            ContextScope::Conversation => ContextUsageClass::Durable,
            ContextScope::Run => ContextUsageClass::RunTransient,
        }
    }

    pub(crate) fn cache_band(&self) -> ContextCacheBand {
        if self.retention == ContextRetention::RequestOnly {
            return ContextCacheBand::RequestTail;
        }
        if self.sources.contains(&ContextSource::BackendSystemPrompt) {
            return ContextCacheBand::StableContract;
        }
        if self.sources.contains(&ContextSource::HistoricalRunContext) {
            return ContextCacheBand::DurableTimeline;
        }
        if self.sources.contains(&ContextSource::ConversationSummary)
            || self.sources.contains(&ContextSource::WorldStateSnapshot)
                && matches!(self.scope, ContextScope::Conversation)
        {
            return ContextCacheBand::ConversationEpochPrelude;
        }
        // A durable conversation observation can be discovered in the middle of a running
        // exchange. Its journal placement follows that run, independently from its ownership
        // and whether the request has adopted the observation yet.
        if self.sources.contains(&ContextSource::RunTimeline) {
            return ContextCacheBand::RunTimeline;
        }
        match self.scope {
            ContextScope::Conversation => ContextCacheBand::DurableTimeline,
            ContextScope::Run => ContextCacheBand::RunTimeline,
        }
    }

    pub(crate) fn message_placement(&self) -> LlmMessagePlacement {
        if self.sources.contains(&ContextSource::BackendSystemPrompt)
            || self.sources.contains(&ContextSource::AutomationExecution)
        {
            return LlmMessagePlacement::StableSystemPolicy;
        }
        if self.sources.contains(&ContextSource::ConversationSummary)
            || self.sources.contains(&ContextSource::WorldStateSnapshot)
            || self.sources.contains(&ContextSource::WorldStateDiff)
            || self.sources.contains(&ContextSource::BackendState)
        {
            return LlmMessagePlacement::BackendStateTimeline;
        }
        LlmMessagePlacement::OrdinaryTimeline
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ContextItem {
    message: LlmMessage,
    checkpoint_message: Option<LlmMessage>,
    metadata: ContextMetadata,
    measurement: Option<ContextItemMeasurement>,
    context_image_refs: Vec<crate::ConversationContextImageRef>,
}

#[derive(Debug, Clone)]
struct ContextItemMeasurement {
    estimator: ContextEstimatorIdentity,
    estimate: ContextMessageEstimate,
}

impl ContextItem {
    pub(crate) fn metadata(&self) -> &ContextMetadata {
        &self.metadata
    }

    pub(crate) fn with_origin(mut self, origin: ContextOrigin) -> Self {
        self.metadata = self.metadata.with_origin(origin);
        self
    }

    pub(crate) fn with_source(mut self, source: ContextSource) -> Self {
        self.metadata = self.metadata.with_source(source);
        self
    }

    pub(crate) fn with_context_image_refs(
        mut self,
        refs: Vec<crate::ConversationContextImageRef>,
    ) -> Self {
        self.context_image_refs = refs;
        self
    }

    pub(crate) fn new(mut message: LlmMessage, metadata: ContextMetadata) -> Self {
        message.set_placement(metadata.message_placement());
        Self {
            message,
            checkpoint_message: None,
            metadata,
            measurement: None,
            context_image_refs: Vec::new(),
        }
    }

    pub(crate) fn text(
        role: LlmMessageRole,
        content: impl Into<String>,
        source: ContextSource,
        scope: ContextScope,
        retention: ContextRetention,
    ) -> Self {
        Self::new(
            LlmMessage::text(role, content),
            ContextMetadata::new(source, scope, retention),
        )
    }

    pub(crate) fn assistant(
        content: impl Into<String>,
        tool_calls: Vec<LlmToolCall>,
        metadata: ContextMetadata,
    ) -> Self {
        Self::new(LlmMessage::assistant(content, tool_calls), metadata)
    }

    pub(crate) fn tool_result(
        tool_call_id: impl Into<String>,
        content: impl Into<String>,
        is_error: bool,
        metadata: ContextMetadata,
    ) -> Self {
        Self::new(
            LlmMessage::tool_result(tool_call_id, content, is_error),
            metadata,
        )
    }

    /// Keeps a richer observation in the live model loop while supplying a
    /// durable-safe replacement for approval checkpoints. The replacement
    /// must preserve the same tool protocol identity and error semantics.
    pub(crate) fn with_checkpoint_tool_result(
        mut self,
        tool_call_id: impl Into<String>,
        content: impl Into<String>,
        is_error: bool,
    ) -> Self {
        let projected = LlmMessage::tool_result(tool_call_id, content, is_error);
        if projected != self.message {
            self.checkpoint_message = Some(projected);
        }
        self
    }

    /// Keeps the live Tool arguments in memory while replacing only the
    /// durable Tool-call projection. All other assistant message fields remain
    /// byte-for-byte identical.
    #[cfg(test)]
    pub(crate) fn with_checkpoint_tool_calls(mut self, tool_calls: Vec<LlmToolCall>) -> Self {
        let mut projected = self.message.clone();
        if let Some(turn) = projected.assistant_turn_mut() {
            *turn = turn.without_raw_continuation_for_checkpoint();
            let live_calls = turn.effective_tool_calls().cloned().collect::<Vec<_>>();
            debug_assert_eq!(live_calls.len(), tool_calls.len());
            let bindings = match turn.runtime_tool_bindings() {
                Some(live_bindings) => live_bindings
                    .iter()
                    .zip(tool_calls)
                    .map(|(binding, runtime_call)| {
                        LlmRuntimeToolCallBinding::new(
                            binding.provider_tool_index,
                            turn.provider_tool_calls()
                                .get(binding.provider_tool_index)
                                .expect("validated assistant turn binding"),
                            runtime_call,
                        )
                    })
                    .collect(),
                None => turn
                    .provider_tool_calls()
                    .iter()
                    .enumerate()
                    .zip(tool_calls)
                    .map(|((index, provider_call), runtime_call)| {
                        LlmRuntimeToolCallBinding::new(index, provider_call, runtime_call)
                    })
                    .collect(),
            };
            turn.set_runtime_tool_bindings(bindings)
                .expect("checkpoint projection preserves assistant turn identities");
        }
        if projected != self.message {
            self.checkpoint_message = Some(projected);
        }
        self
    }

    /// Keeps a richer message in the live model loop while supplying a durable-safe replacement
    /// for approval checkpoints.
    ///
    /// `to_checkpoint` remains the enforcement boundary: the replacement may remove transient
    /// images or text and may redact Tool arguments, but it cannot change the role, Tool call
    /// count/order, Tool identity, or error semantics of the live message.
    pub(crate) fn with_checkpoint_message(mut self, projected: LlmMessage) -> Self {
        if projected != self.message {
            self.checkpoint_message = Some(projected);
        }
        self
    }

    fn measure(&mut self, estimator: &dyn ContextTokenEstimator) -> ContextMessageEstimate {
        let identity = estimator.identity();
        if let Some(measurement) = &self.measurement {
            if measurement.estimator == identity {
                return measurement.estimate;
            }
        }

        let estimate = estimator.estimate_message(&self.message);
        self.measurement = Some(ContextItemMeasurement {
            estimator: identity,
            estimate,
        });
        estimate
    }
}

fn persistent_frame_revision(items: &[ContextItem]) -> u64 {
    persistent_frame_revision_iter(items.iter())
}

fn persistent_frame_revision_iter<'a>(items: impl Iterator<Item = &'a ContextItem>) -> u64 {
    items
        .filter(|item| item.metadata.usage_class().is_persistent())
        .fold(ContextRevisionHasher::new().finish(), |revision, item| {
            combine_context_revisions(revision, context_item_revision(item))
        })
}

fn context_item_revision(item: &ContextItem) -> u64 {
    let mut hasher = ContextRevisionHasher::new();
    hasher.write_str(item.message.role().as_str());
    hasher.write_str(item.message.content());
    let (tool_call_id, is_error) = item
        .message
        .tool_result_fields()
        .map_or((None, false), |(id, _, is_error)| (Some(id), is_error));
    hasher.write_str(tool_call_id.unwrap_or_default());
    hasher.write_u64(is_error.into());
    for reference in &item.context_image_refs {
        hasher.write_str(&reference.attachment_id);
        hasher.write_str(&reference.mime_type);
        hasher.write_str(&reference.sha256);
    }
    for image in item.message.images() {
        hasher.write_str(&image.mime_type);
        hasher.write_str(&image.data_base64);
    }
    if let Some(turn) = item.message.assistant_turn() {
        hasher.write_str(&turn.context_revision_material());
        for call in turn.effective_tool_calls() {
            hasher.write_str(&call.id);
            hasher.write_str(&call.name);
            hasher.write_str(
                &serde_json::to_string(&call.args).unwrap_or_else(|_| "null".to_string()),
            );
        }
    }
    for source in item.metadata.sources() {
        hasher.write_str(source.as_str());
    }
    hasher.write_str(item.metadata.scope().as_str());
    hasher.write_str(item.metadata.retention().as_str());
    if let Some(order) = item.metadata.request_order() {
        hasher.write_str("request_order");
        hasher.write_u64(order);
    }
    if let Some(group) = item.metadata.group() {
        hasher.write_str(group.id());
        hasher.write_str(group.kind().as_str());
    }
    if let Some(origin) = item.metadata.origin() {
        hasher.write_str(origin.kind().as_str());
        hasher.write_str(origin.id());
    }
    hasher.finish()
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod compaction_layout_tests;
