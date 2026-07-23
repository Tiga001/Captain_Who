use super::measurement::{
    combine_context_revisions, ContextEstimatorIdentity, ContextMessageEstimate,
    ContextRevisionHasher, ContextTokenEstimator,
};
use super::ContextJournalCursor;
use crate::llm::{LlmMessage, LlmMessageRole, LlmToolCall};
use crate::protocol::{
    AgentContextCheckpointGroup, AgentContextCheckpointImage, AgentContextCheckpointItem,
    AgentContextCheckpointOrigin, AgentContextCheckpointToolCall, AgentError, AgentResult,
};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

mod checkpoint;
mod measurement_state;

pub(crate) use checkpoint::*;
pub(crate) use measurement_state::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ContextScope {
    Run,
    Conversation,
    // Reserved for the later project-memory source without enabling it today.
    #[allow(dead_code)]
    Project,
}

impl ContextScope {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Run => "run",
            Self::Conversation => "conversation",
            Self::Project => "project",
        }
    }

    fn from_str(value: &str) -> Option<Self> {
        match value {
            "run" => Some(Self::Run),
            "conversation" => Some(Self::Conversation),
            "project" => Some(Self::Project),
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
    /// Current-run history required by the active tool loop but not rebuilt next turn.
    RunTransient,
    /// Ephemeral state injected into exactly one provider request.
    RequestOnly,
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
    ConversationSummary,
    ConversationHistory,
    ConversationTrace,
    CurrentTurn,
    SkillCatalog,
    SkillInstructions,
    InputAttachment,
    ToolContinuation,
    ModelResponse,
    ToolResult,
    RuntimeExtension,
    FileTransaction,
    RuntimeGuard,
    CompactionRequest,
}

impl ContextSource {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::BackendSystemPrompt => "backend_system_prompt",
            Self::ConversationSummary => "conversation_summary",
            Self::ConversationHistory => "conversation_history",
            Self::ConversationTrace => "conversation_trace",
            Self::CurrentTurn => "current_turn",
            Self::SkillCatalog => "skill_catalog",
            Self::SkillInstructions => "skill_instructions",
            Self::InputAttachment => "input_attachment",
            Self::ToolContinuation => "tool_continuation",
            Self::ModelResponse => "model_response",
            Self::ToolResult => "tool_result",
            Self::RuntimeExtension => "runtime_extension",
            Self::FileTransaction => "file_transaction",
            Self::RuntimeGuard => "runtime_guard",
            Self::CompactionRequest => "compaction_request",
        }
    }

    fn from_str(value: &str) -> Option<Self> {
        match value {
            "backend_system_prompt" => Some(Self::BackendSystemPrompt),
            "conversation_summary" => Some(Self::ConversationSummary),
            "conversation_history" => Some(Self::ConversationHistory),
            "conversation_trace" => Some(Self::ConversationTrace),
            "current_turn" => Some(Self::CurrentTurn),
            "skill_catalog" => Some(Self::SkillCatalog),
            "skill_instructions" => Some(Self::SkillInstructions),
            "input_attachment" => Some(Self::InputAttachment),
            "tool_continuation" => Some(Self::ToolContinuation),
            "model_response" => Some(Self::ModelResponse),
            "tool_result" => Some(Self::ToolResult),
            "runtime_extension" => Some(Self::RuntimeExtension),
            "file_transaction" => Some(Self::FileTransaction),
            "runtime_guard" => Some(Self::RuntimeGuard),
            "compaction_request" => Some(Self::CompactionRequest),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ContextGroupKind {
    ToolExchange,
}

impl ContextGroupKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::ToolExchange => "tool_exchange",
        }
    }

    fn from_str(value: &str) -> Option<Self> {
        match value {
            "tool_exchange" => Some(Self::ToolExchange),
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
    Skill,
}

impl ContextOriginKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::ConversationMessage => "conversation_message",
            Self::ConversationTraceItem => "conversation_trace_item",
            Self::CompactionSummary => "compaction_summary",
            Self::Skill => "skill",
        }
    }

    fn from_str(value: &str) -> Option<Self> {
        match value {
            "conversation_message" => Some(Self::ConversationMessage),
            "conversation_trace_item" => Some(Self::ConversationTraceItem),
            "compaction_summary" => Some(Self::CompactionSummary),
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
            ContextOriginKind::CompactionSummary | ContextOriginKind::Skill => None,
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
        match self.scope {
            ContextScope::Conversation | ContextScope::Project => ContextUsageClass::Durable,
            ContextScope::Run => ContextUsageClass::RunTransient,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ContextItem {
    message: LlmMessage,
    checkpoint_message: Option<LlmMessage>,
    metadata: ContextMetadata,
    measurement: Option<ContextItemMeasurement>,
}

#[derive(Debug, Clone)]
struct ContextItemMeasurement {
    estimator: ContextEstimatorIdentity,
    estimate: ContextMessageEstimate,
}

impl ContextItem {
    pub(crate) fn new(message: LlmMessage, metadata: ContextMetadata) -> Self {
        Self {
            message,
            checkpoint_message: None,
            metadata,
            measurement: None,
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

    /// Keeps a richer message in the live model loop while supplying a durable-safe replacement
    /// for approval checkpoints.
    ///
    /// `to_checkpoint` remains the enforcement boundary: the replacement may remove transient
    /// images or text, but it cannot change the role, tool identity, tool calls, or error
    /// semantics of the live message.
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
    hasher.write_str(item.message.role.as_str());
    hasher.write_str(&item.message.content);
    hasher.write_str(item.message.tool_call_id.as_deref().unwrap_or_default());
    hasher.write_u64(item.message.is_error.into());
    for image in &item.message.images {
        hasher.write_str(&image.mime_type);
        hasher.write_str(&image.data_base64);
    }
    for call in &item.message.tool_calls {
        hasher.write_str(&call.id);
        hasher.write_str(&call.name);
        hasher.write_str(&serde_json::to_string(&call.args).unwrap_or_else(|_| "null".to_string()));
    }
    for source in item.metadata.sources() {
        hasher.write_str(source.as_str());
    }
    hasher.write_str(item.metadata.scope().as_str());
    hasher.write_str(item.metadata.retention().as_str());
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
