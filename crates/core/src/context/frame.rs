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
}

impl ContextOriginKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::ConversationMessage => "conversation_message",
            Self::ConversationTraceItem => "conversation_trace_item",
            Self::CompactionSummary => "compaction_summary",
        }
    }

    fn from_str(value: &str) -> Option<Self> {
        match value {
            "conversation_message" => Some(Self::ConversationMessage),
            "conversation_trace_item" => Some(Self::ConversationTraceItem),
            "compaction_summary" => Some(Self::CompactionSummary),
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
            ContextOriginKind::CompactionSummary => None,
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

#[derive(Debug, Clone)]
pub(crate) struct ContextFrame {
    baseline: Option<MeasuredContextBaseline>,
    /// Items appended after the shared durable baseline. Runtime clones only this overlay.
    items: Vec<ContextItem>,
    revision: u64,
    persistent_revision: u64,
    measurement: Option<ContextFrameMeasurementState>,
}

/// Immutable, already-measured conversation context shared by the server cache and active runs.
/// Chunks preserve append-only updates without copying older durable items.
#[derive(Debug, Clone)]
pub(crate) struct MeasuredContextBaseline {
    chunks: Arc<[Arc<[ContextItem]>]>,
    revision: u64,
    persistent_revision: u64,
    measurement: ContextFrameMeasurementState,
}

#[derive(Debug, Clone)]
struct ContextFrameMeasurementState {
    estimator: Arc<dyn ContextTokenEstimator>,
    identity: ContextEstimatorIdentity,
    breakdown: ContextFrameEstimateBreakdown,
    full_recount: Option<ContextFullRecount>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ContextFrameEstimateBucket {
    pub(crate) estimate: ContextMessageEstimate,
    pub(crate) item_count: usize,
}

impl ContextFrameEstimateBucket {
    fn merge(&mut self, estimate: ContextMessageEstimate) {
        self.estimate.merge(estimate);
        self.item_count = self.item_count.saturating_add(1);
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ContextFrameEstimateBreakdown {
    pub(crate) fixed: ContextFrameEstimateBucket,
    pub(crate) durable: ContextFrameEstimateBucket,
    pub(crate) run_transient: ContextFrameEstimateBucket,
    pub(crate) request_only: ContextFrameEstimateBucket,
}

impl ContextFrameEstimateBreakdown {
    fn merge(&mut self, class: ContextUsageClass, estimate: ContextMessageEstimate) {
        match class {
            ContextUsageClass::Fixed => self.fixed.merge(estimate),
            ContextUsageClass::Durable => self.durable.merge(estimate),
            ContextUsageClass::RunTransient => self.run_transient.merge(estimate),
            ContextUsageClass::RequestOnly => self.request_only.merge(estimate),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct ContextFullRecount {
    revision: u64,
    estimate: ContextMessageEstimate,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ContextFrameMeasurement {
    pub(crate) estimator: ContextEstimatorIdentity,
    pub(crate) breakdown: ContextFrameEstimateBreakdown,
    /// A boundary-sensitive tokenizer may verify only the complete request total. Category totals
    /// remain additive and are never distorted to imitate that provider-wide number.
    pub(crate) verified_total: Option<ContextMessageEstimate>,
    pub(crate) revision: u64,
    pub(crate) persistent_revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ContextFramePlanningItem {
    pub(crate) index: usize,
    pub(crate) usage_class: ContextUsageClass,
    pub(crate) estimated_tokens: u64,
    pub(crate) role: LlmMessageRole,
    pub(crate) sources: Vec<ContextSource>,
    pub(crate) group_id: Option<String>,
    pub(crate) tool_names: Vec<String>,
    pub(crate) image_count: usize,
    pub(crate) is_error: bool,
    pub(crate) origin: Option<ContextOrigin>,
}

impl ContextFrame {
    pub(crate) fn new(items: Vec<ContextItem>) -> Self {
        let revision = u64::try_from(items.len()).unwrap_or(u64::MAX);
        let persistent_revision = persistent_frame_revision(&items);
        Self {
            baseline: None,
            items,
            revision,
            persistent_revision,
            measurement: None,
        }
    }

    pub(crate) fn from_measured_baseline(baseline: MeasuredContextBaseline) -> Self {
        Self {
            revision: baseline.revision,
            persistent_revision: baseline.persistent_revision,
            measurement: Some(baseline.measurement.clone()),
            baseline: Some(baseline),
            items: Vec::new(),
        }
    }

    /// Reuses the longest whole-chunk prefix that is byte-for-byte equivalent to this restored
    /// checkpoint. Remaining checkpoint items become the run overlay and are measured once.
    pub(crate) fn rebase_onto_measured_baseline(
        mut self,
        candidate: MeasuredContextBaseline,
    ) -> Self {
        self.materialize_baseline();
        let Some((baseline, prefix_item_count)) = candidate.matching_prefix(&self.items) else {
            return self;
        };
        let overlay = self.items.split_off(prefix_item_count);
        let mut rebased = Self::from_measured_baseline(baseline);
        for item in overlay {
            rebased.push(item);
        }
        rebased
    }

    /// Replaces every fixed/durable item with a newly committed authoritative baseline while
    /// retaining the current run overlay in order. This is used after durable compaction; tool
    /// observations remain available to the active loop even though older conversation history
    /// has been replaced by a summary.
    pub(crate) fn replace_persistent_baseline(self, baseline: MeasuredContextBaseline) -> Self {
        let overlay = self
            .iter_items()
            .filter(|item| !item.metadata.usage_class().is_persistent())
            .cloned()
            .collect::<Vec<_>>();
        let mut replaced = Self::from_measured_baseline(baseline);
        for item in overlay {
            replaced.push(item);
        }
        replaced
    }

    /// Adopts the latest persisted context log after a successful main-model request. Model/tool
    /// overlay items are now represented canonically by the baseline and are removed; unrelated
    /// run state such as attachments and protocol guards remains in place.
    pub(crate) fn promote_committed_trace(self, baseline: MeasuredContextBaseline) -> Self {
        let overlay = self
            .iter_items()
            .filter(|item| {
                !item.metadata.usage_class().is_persistent()
                    && !item.metadata.sources().iter().any(|source| {
                        matches!(
                            source,
                            ContextSource::ModelResponse
                                | ContextSource::ToolResult
                                | ContextSource::ToolContinuation
                        )
                    })
            })
            .cloned()
            .collect::<Vec<_>>();
        let mut promoted = Self::from_measured_baseline(baseline);
        for item in overlay {
            promoted.push(item);
        }
        promoted
    }

    /// Freezes all persistent overlay items into a shareable measured baseline. The frame keeps
    /// the same baseline, so subsequent durable appends become a new chunk rather than copying
    /// previously frozen history.
    pub(crate) fn share_measured_persistent_baseline(
        &mut self,
    ) -> AgentResult<MeasuredContextBaseline> {
        let measurement = self
            .measurement
            .clone()
            .ok_or_else(|| AgentError::new("共享上下文基线前必须先完成计量。"))?;
        if self
            .iter_items()
            .any(|item| !item.metadata.usage_class().is_persistent())
        {
            return Err(AgentError::new(
                "共享上下文基线只能包含 fixed 或 durable 内容。",
            ));
        }

        let mut chunks = self
            .baseline
            .as_ref()
            .map(|baseline| baseline.chunks.to_vec())
            .unwrap_or_default();
        if !self.items.is_empty() {
            let overlay = std::mem::take(&mut self.items);
            chunks.push(Arc::from(overlay.into_boxed_slice()));
        }
        let baseline = MeasuredContextBaseline {
            chunks: Arc::from(chunks.into_boxed_slice()),
            revision: self.revision,
            persistent_revision: self.persistent_revision,
            measurement,
        };
        self.baseline = Some(baseline.clone());
        Ok(baseline)
    }

    pub(crate) fn push(&mut self, mut item: ContextItem) {
        let usage_class = item.metadata.usage_class();
        if usage_class.is_persistent() {
            self.persistent_revision =
                combine_context_revisions(self.persistent_revision, context_item_revision(&item));
        }
        if let Some(measurement) = &mut self.measurement {
            let estimate = item.measure(measurement.estimator.as_ref());
            measurement.breakdown.merge(usage_class, estimate);
            measurement.full_recount = None;
        }
        self.items.push(item);
        self.revision = self.revision.saturating_add(1);
    }

    pub(crate) fn begin_new_conversation_turn(&mut self) {
        self.materialize_baseline();
        for item in &mut self.items {
            item.metadata.replace_source(
                ContextSource::CurrentTurn,
                ContextSource::ConversationHistory,
            );
        }
        self.persistent_revision = persistent_frame_revision(&self.items);
    }

    pub(crate) fn measure_incrementally(
        &mut self,
        estimator: Arc<dyn ContextTokenEstimator>,
    ) -> ContextFrameMeasurement {
        let identity = estimator.identity();
        let needs_rebuild = self
            .measurement
            .as_ref()
            .is_none_or(|measurement| measurement.identity != identity);
        if needs_rebuild {
            self.materialize_baseline();
            let mut breakdown = ContextFrameEstimateBreakdown::default();
            for item in &mut self.items {
                let usage_class = item.metadata.usage_class();
                breakdown.merge(usage_class, item.measure(estimator.as_ref()));
            }
            self.measurement = Some(ContextFrameMeasurementState {
                estimator,
                identity: identity.clone(),
                breakdown,
                full_recount: None,
            });
        }

        let measurement = self
            .measurement
            .as_ref()
            .expect("context measurement must exist after rebuilding");
        ContextFrameMeasurement {
            estimator: measurement.identity.clone(),
            breakdown: measurement.breakdown,
            verified_total: None,
            revision: self.revision,
            persistent_revision: self.persistent_revision,
        }
    }

    pub(crate) fn measure_full(
        &mut self,
        estimator: Arc<dyn ContextTokenEstimator>,
    ) -> ContextFrameMeasurement {
        let incremental = self.measure_incrementally(estimator.clone());
        if let Some(full_recount) = self
            .measurement
            .as_ref()
            .and_then(|measurement| measurement.full_recount)
            .filter(|measurement| measurement.revision == self.revision)
        {
            return ContextFrameMeasurement {
                verified_total: Some(full_recount.estimate),
                ..incremental
            };
        }

        let messages = self
            .iter_items()
            .map(|item| &item.message)
            .collect::<Vec<_>>();
        let estimate = estimator.estimate_messages(&messages);
        if let Some(measurement) = &mut self.measurement {
            measurement.full_recount = Some(ContextFullRecount {
                revision: self.revision,
                estimate,
            });
        }
        ContextFrameMeasurement {
            verified_total: Some(estimate),
            ..incremental
        }
    }

    /// Produces a content-free inventory for deterministic compaction planning. The planner must
    /// consume the same cached estimator identity as the capacity report and may not remeasure
    /// messages independently.
    pub(crate) fn planning_items(&self) -> AgentResult<Vec<ContextFramePlanningItem>> {
        let measurement = self
            .measurement
            .as_ref()
            .ok_or_else(|| AgentError::new("生成压缩计划前必须先完成上下文计量。"))?;
        self.iter_items()
            .enumerate()
            .map(|(index, item)| {
                let estimate = item
                    .measurement
                    .as_ref()
                    .filter(|item_measurement| item_measurement.estimator == measurement.identity)
                    .map(|item_measurement| item_measurement.estimate)
                    .ok_or_else(|| {
                        AgentError::new(format!(
                            "上下文第 {index} 项缺少与容量报告一致的计量结果。"
                        ))
                    })?;
                Ok(ContextFramePlanningItem {
                    index,
                    usage_class: item.metadata.usage_class(),
                    estimated_tokens: estimate.total_tokens(),
                    role: item.message.role,
                    sources: item.metadata.sources().to_vec(),
                    group_id: item.metadata.group().map(|group| group.id().to_string()),
                    tool_names: item
                        .message
                        .tool_calls
                        .iter()
                        .map(|call| call.name.clone())
                        .collect(),
                    image_count: estimate.image_count,
                    is_error: item.message.is_error,
                    origin: item.metadata.origin().cloned(),
                })
            })
            .collect()
    }

    pub(crate) fn checkpoint_items(&self) -> AgentResult<Vec<AgentContextCheckpointItem>> {
        self.iter_items().map(ContextItem::to_checkpoint).collect()
    }

    pub(crate) fn from_checkpoint_items(
        items: Vec<AgentContextCheckpointItem>,
    ) -> AgentResult<Self> {
        if items.is_empty() {
            return Err(AgentError::new("运行检查点没有上下文内容。"));
        }
        let items = items
            .into_iter()
            .map(ContextItem::from_checkpoint)
            .collect::<AgentResult<Vec<_>>>()?;
        Ok(Self::new(items))
    }

    pub(crate) fn validate_pending_tool_call(
        &self,
        pending_tool_call_id: &str,
    ) -> AgentResult<LlmToolCall> {
        let unresolved = self.unresolved_tool_calls()?;
        if unresolved.len() != 1 {
            return Err(AgentError::new(format!(
                "运行检查点必须恰好包含一个待审批工具调用，实际为 {} 个。",
                unresolved.len()
            )));
        }
        unresolved
            .get(pending_tool_call_id)
            .map(|(call, _)| call.clone())
            .ok_or_else(|| {
                AgentError::new(format!(
                    "运行检查点中的待审批工具调用与 `{pending_tool_call_id}` 不一致。"
                ))
            })
    }

    pub(crate) fn append_tool_continuation(
        &mut self,
        call: &LlmToolCall,
        observation: String,
        is_error: bool,
    ) -> AgentResult<()> {
        let checkpoint_call = self.validate_pending_tool_call(&call.id)?;
        if checkpoint_call.name != call.name {
            return Err(AgentError::new(format!(
                "审批续跑工具不一致：检查点为 `{}`，续跑结果为 `{}`。",
                checkpoint_call.name, call.name
            )));
        }
        let group = self
            .iter_items()
            .rev()
            .find(|item| {
                item.message
                    .tool_calls
                    .iter()
                    .any(|tool_call| tool_call.id == call.id)
            })
            .and_then(|item| item.metadata.group().cloned())
            .ok_or_else(|| AgentError::new("待审批工具调用缺少原子工具交换分组。"))?;

        self.push(ContextItem::tool_result(
            call.id.clone(),
            observation,
            is_error,
            ContextMetadata::new(
                ContextSource::ToolContinuation,
                ContextScope::Run,
                ContextRetention::Retained,
            )
            .with_source(ContextSource::ToolResult)
            .with_group(group),
        ));
        self.validate_complete_tool_protocol()
    }

    pub(crate) fn validate_complete_tool_protocol(&self) -> AgentResult<()> {
        let unresolved = self.unresolved_tool_calls()?;
        if unresolved.is_empty() {
            Ok(())
        } else {
            Err(AgentError::new(format!(
                "模型请求上下文仍有 {} 个工具调用缺少结果。",
                unresolved.len()
            )))
        }
    }

    pub(crate) fn contains_group_id(&self, group_id: &str) -> bool {
        self.iter_items().any(|item| {
            item.metadata
                .group()
                .is_some_and(|group| group.id() == group_id)
        })
    }

    pub(crate) fn contains_tool_call_id(&self, tool_call_id: &str) -> bool {
        self.iter_items().any(|item| {
            item.message
                .tool_calls
                .iter()
                .any(|call| call.id == tool_call_id)
        })
    }

    #[cfg(test)]
    pub(crate) fn to_messages(&self) -> Vec<LlmMessage> {
        self.iter_items().map(|item| item.message.clone()).collect()
    }

    pub(crate) fn into_messages(self) -> Vec<LlmMessage> {
        let baseline_item_count = self
            .baseline
            .as_ref()
            .map_or(0, MeasuredContextBaseline::item_count);
        let mut messages = Vec::with_capacity(baseline_item_count + self.items.len());
        if let Some(baseline) = self.baseline {
            messages.extend(baseline.iter_items().map(|item| item.message.clone()));
        }
        messages.extend(self.items.into_iter().map(|item| item.message));
        messages
    }

    pub(crate) fn manifest(&self) -> ContextManifest<'_> {
        ContextManifest {
            entries: self
                .iter_items()
                .enumerate()
                .map(|(index, item)| ContextManifestEntry {
                    index,
                    role: item.message.role.as_str(),
                    sources: item
                        .metadata
                        .sources()
                        .iter()
                        .map(|source| source.as_str())
                        .collect(),
                    scope: item.metadata.scope().as_str(),
                    retention: item.metadata.retention().as_str(),
                    group_id: item.metadata.group().map(ContextGroup::id),
                    group_kind: item.metadata.group().map(|group| group.kind().as_str()),
                    text_character_count: item.message.content.chars().count(),
                    image_count: item.message.images.len(),
                    image_base64_bytes: item
                        .message
                        .images
                        .iter()
                        .map(|image| image.data_base64.len())
                        .sum(),
                    tool_call_count: item.message.tool_calls.len(),
                    tool_argument_character_count: item
                        .message
                        .tool_calls
                        .iter()
                        .filter_map(|call| serde_json::to_string(&call.args).ok())
                        .map(|args| args.chars().count())
                        .sum(),
                    tool_call_id: item.message.tool_call_id.as_deref(),
                    is_error: item.message.is_error,
                })
                .collect(),
        }
    }

    fn unresolved_tool_calls(&self) -> AgentResult<BTreeMap<String, (LlmToolCall, ContextGroup)>> {
        let mut unresolved = BTreeMap::new();
        let mut seen_call_ids = BTreeSet::new();
        for item in self.iter_items() {
            match item.message.role {
                LlmMessageRole::Assistant if !item.message.tool_calls.is_empty() => {
                    if !unresolved.is_empty() {
                        return Err(AgentError::new(
                            "工具调用协议无效：上一组工具调用尚未获得完整结果。",
                        ));
                    }
                    let group = item.metadata.group().cloned().ok_or_else(|| {
                        AgentError::new("工具调用协议无效：assistant 工具调用缺少交换分组。")
                    })?;
                    for call in &item.message.tool_calls {
                        if call.id.trim().is_empty() || call.name.trim().is_empty() {
                            return Err(AgentError::new(
                                "工具调用协议无效：工具调用 id 和名称不能为空。",
                            ));
                        }
                        if !seen_call_ids.insert(call.id.clone()) {
                            return Err(AgentError::new(format!(
                                "工具调用协议无效：工具调用 id `{}` 在当前运行中重复。",
                                call.id
                            )));
                        }
                        if unresolved
                            .insert(call.id.clone(), (call.clone(), group.clone()))
                            .is_some()
                        {
                            return Err(AgentError::new(format!(
                                "工具调用协议无效：工具调用 id `{}` 重复。",
                                call.id
                            )));
                        }
                    }
                }
                LlmMessageRole::Tool => {
                    let call_id = item.message.tool_call_id.as_deref().ok_or_else(|| {
                        AgentError::new("工具调用协议无效：工具结果缺少 tool_call_id。")
                    })?;
                    let Some((_, expected_group)) = unresolved.remove(call_id) else {
                        return Err(AgentError::new(format!(
                            "工具调用协议无效：工具结果 `{call_id}` 没有对应的未结算调用。"
                        )));
                    };
                    if item.metadata.group() != Some(&expected_group) {
                        return Err(AgentError::new(format!(
                            "工具调用协议无效：工具结果 `{call_id}` 的交换分组不匹配。"
                        )));
                    }
                }
                _ if !unresolved.is_empty() => {
                    return Err(AgentError::new(
                        "工具调用协议无效：工具调用与结果之间出现了其他消息。",
                    ));
                }
                _ => {}
            }
        }
        Ok(unresolved)
    }

    fn iter_items(&self) -> impl DoubleEndedIterator<Item = &ContextItem> {
        self.baseline
            .iter()
            .flat_map(MeasuredContextBaseline::iter_items)
            .chain(self.items.iter())
    }

    fn materialize_baseline(&mut self) {
        let Some(baseline) = self.baseline.take() else {
            return;
        };
        let mut items = Vec::with_capacity(baseline.item_count() + self.items.len());
        items.extend(baseline.iter_items().cloned());
        items.append(&mut self.items);
        self.items = items;
    }
}

impl MeasuredContextBaseline {
    fn iter_items(&self) -> impl DoubleEndedIterator<Item = &ContextItem> {
        self.chunks.iter().flat_map(|chunk| chunk.iter())
    }

    fn item_count(&self) -> usize {
        self.chunks.iter().map(|chunk| chunk.len()).sum()
    }

    fn matching_prefix(&self, items: &[ContextItem]) -> Option<(Self, usize)> {
        let mut matching_chunks = Vec::new();
        let mut item_offset = 0_usize;
        let mut breakdown = ContextFrameEstimateBreakdown::default();

        for chunk in self.chunks.iter() {
            let end = item_offset.checked_add(chunk.len())?;
            let Some(candidate_items) = items.get(item_offset..end) else {
                break;
            };
            if !chunk
                .iter()
                .zip(candidate_items)
                .all(|(left, right)| left.same_context_content(right))
            {
                break;
            }
            for item in chunk.iter() {
                let estimate = item
                    .measurement
                    .as_ref()
                    .filter(|measurement| measurement.estimator == self.measurement.identity)
                    .map(|measurement| measurement.estimate)?;
                breakdown.merge(item.metadata.usage_class(), estimate);
            }
            matching_chunks.push(chunk.clone());
            item_offset = end;
        }

        if matching_chunks.is_empty() {
            return None;
        }
        let persistent_revision =
            persistent_frame_revision_iter(matching_chunks.iter().flat_map(|chunk| chunk.iter()));
        Some((
            Self {
                chunks: Arc::from(matching_chunks.into_boxed_slice()),
                revision: u64::try_from(item_offset).unwrap_or(u64::MAX),
                persistent_revision,
                measurement: ContextFrameMeasurementState {
                    estimator: self.measurement.estimator.clone(),
                    identity: self.measurement.identity.clone(),
                    breakdown,
                    full_recount: None,
                },
            },
            item_offset,
        ))
    }
}

impl Default for ContextFrame {
    fn default() -> Self {
        Self::new(Vec::new())
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

impl ContextItem {
    fn same_context_content(&self, other: &Self) -> bool {
        self.message == other.message && self.metadata == other.metadata
    }
}

impl ContextItem {
    fn to_checkpoint(&self) -> AgentResult<AgentContextCheckpointItem> {
        if self.metadata.retention() != ContextRetention::Retained {
            return Err(AgentError::new("运行检查点不能保存 request-only 上下文。"));
        }
        Ok(AgentContextCheckpointItem {
            role: self.message.role.as_str().to_string(),
            content: self.message.content.clone(),
            images: self
                .message
                .images
                .iter()
                .map(|image| AgentContextCheckpointImage {
                    mime_type: image.mime_type.clone(),
                    data_base64: image.data_base64.clone(),
                })
                .collect(),
            tool_call_id: self.message.tool_call_id.clone(),
            tool_calls: self
                .message
                .tool_calls
                .iter()
                .map(|call| AgentContextCheckpointToolCall {
                    id: call.id.clone(),
                    name: call.name.clone(),
                    args: call.args.clone(),
                })
                .collect(),
            is_error: self.message.is_error,
            sources: self
                .metadata
                .sources()
                .iter()
                .map(|source| source.as_str().to_string())
                .collect(),
            scope: self.metadata.scope().as_str().to_string(),
            retention: self.metadata.retention().as_str().to_string(),
            group: self
                .metadata
                .group()
                .map(|group| AgentContextCheckpointGroup {
                    id: group.id().to_string(),
                    kind: group.kind().as_str().to_string(),
                }),
            origin: self
                .metadata
                .origin()
                .map(|origin| AgentContextCheckpointOrigin {
                    kind: origin.kind().as_str().to_string(),
                    id: origin.id().to_string(),
                }),
        })
    }

    fn from_checkpoint(item: AgentContextCheckpointItem) -> AgentResult<Self> {
        let role = role_from_checkpoint(&item.role)?;
        if role != LlmMessageRole::User && !item.images.is_empty() {
            return Err(AgentError::new(
                "运行检查点无效：只有 user 消息可以携带图片。",
            ));
        }
        if role != LlmMessageRole::Assistant && !item.tool_calls.is_empty() {
            return Err(AgentError::new(
                "运行检查点无效：只有 assistant 消息可以携带工具调用。",
            ));
        }
        if role == LlmMessageRole::Tool && item.tool_call_id.is_none() {
            return Err(AgentError::new(
                "运行检查点无效：tool 消息缺少 tool_call_id。",
            ));
        }
        if role != LlmMessageRole::Tool && item.tool_call_id.is_some() {
            return Err(AgentError::new(
                "运行检查点无效：非 tool 消息不能携带 tool_call_id。",
            ));
        }

        let mut sources = item.sources.into_iter();
        let first_source = sources
            .next()
            .as_deref()
            .and_then(ContextSource::from_str)
            .ok_or_else(|| AgentError::new("运行检查点无效：上下文来源为空或未知。"))?;
        let scope = ContextScope::from_str(&item.scope)
            .ok_or_else(|| AgentError::new("运行检查点包含未知上下文作用域。"))?;
        let retention = ContextRetention::from_str(&item.retention)
            .ok_or_else(|| AgentError::new("运行检查点包含未知上下文保留策略。"))?;
        if retention != ContextRetention::Retained {
            return Err(AgentError::new("运行检查点不能恢复 request-only 上下文。"));
        }
        let mut metadata = ContextMetadata::new(first_source, scope, retention);
        for source in sources {
            let source = ContextSource::from_str(&source)
                .ok_or_else(|| AgentError::new("运行检查点包含未知上下文来源。"))?;
            metadata = metadata.with_source(source);
        }
        if let Some(group) = item.group {
            let kind = ContextGroupKind::from_str(&group.kind)
                .ok_or_else(|| AgentError::new("运行检查点包含未知上下文分组类型。"))?;
            if group.id.trim().is_empty() {
                return Err(AgentError::new("运行检查点包含空上下文分组 id。"));
            }
            metadata = metadata.with_group(ContextGroup { id: group.id, kind });
        }
        if let Some(origin) = item.origin {
            let kind = ContextOriginKind::from_str(&origin.kind)
                .ok_or_else(|| AgentError::new("运行检查点包含未知上下文来源身份类型。"))?;
            if origin.id.trim().is_empty() {
                return Err(AgentError::new("运行检查点包含空上下文来源身份。"));
            }
            metadata = metadata.with_origin(ContextOrigin {
                kind,
                id: origin.id,
            });
        }

        Ok(Self::new(
            LlmMessage {
                role,
                content: item.content,
                images: item
                    .images
                    .into_iter()
                    .map(|image| crate::llm::LlmImage {
                        mime_type: image.mime_type,
                        data_base64: image.data_base64,
                    })
                    .collect(),
                tool_call_id: item.tool_call_id,
                tool_calls: item
                    .tool_calls
                    .into_iter()
                    .map(|call| LlmToolCall {
                        id: call.id,
                        name: call.name,
                        args: call.args,
                    })
                    .collect(),
                is_error: item.is_error,
            },
            metadata,
        ))
    }
}

fn role_from_checkpoint(value: &str) -> AgentResult<LlmMessageRole> {
    match value {
        "system" => Ok(LlmMessageRole::System),
        "user" => Ok(LlmMessageRole::User),
        "assistant" => Ok(LlmMessageRole::Assistant),
        "tool" => Ok(LlmMessageRole::Tool),
        _ => Err(AgentError::new(format!(
            "运行检查点包含未知消息角色：{value}"
        ))),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ContextManifest<'a> {
    pub(crate) entries: Vec<ContextManifestEntry<'a>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ContextManifestEntry<'a> {
    pub(crate) index: usize,
    pub(crate) role: &'static str,
    pub(crate) sources: Vec<&'static str>,
    pub(crate) scope: &'static str,
    pub(crate) retention: &'static str,
    pub(crate) group_id: Option<&'a str>,
    pub(crate) group_kind: Option<&'static str>,
    pub(crate) text_character_count: usize,
    pub(crate) image_count: usize,
    pub(crate) image_base64_bytes: usize,
    pub(crate) tool_call_count: usize,
    pub(crate) tool_argument_character_count: usize,
    pub(crate) tool_call_id: Option<&'a str>,
    pub(crate) is_error: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::ContextCapacityDetector;
    use crate::llm::LlmImage;
    use crate::protocol::AgentApiStyle;
    use serde_json::json;

    #[test]
    fn checkpoint_round_trip_preserves_messages_images_and_metadata() {
        let group = ContextGroup::tool_exchange("exchange-1");
        let mut image_message = LlmMessage::text(LlmMessageRole::User, "inspect image");
        image_message.images.push(LlmImage {
            mime_type: "image/png".to_string(),
            data_base64: "YWJj".to_string(),
        });
        let frame = ContextFrame::new(vec![
            ContextItem::text(
                LlmMessageRole::System,
                "rules",
                ContextSource::BackendSystemPrompt,
                ContextScope::Run,
                ContextRetention::Retained,
            ),
            ContextItem::new(
                image_message,
                ContextMetadata::new(
                    ContextSource::CurrentTurn,
                    ContextScope::Conversation,
                    ContextRetention::Retained,
                )
                .with_source(ContextSource::InputAttachment),
            ),
            ContextItem::assistant(
                "read it",
                vec![LlmToolCall {
                    id: "call-1".to_string(),
                    name: "read_file".to_string(),
                    args: json!({ "path": "notes.txt" }),
                }],
                ContextMetadata::new(
                    ContextSource::ModelResponse,
                    ContextScope::Run,
                    ContextRetention::Retained,
                )
                .with_group(group.clone()),
            ),
            ContextItem::tool_result(
                "call-1",
                "contents",
                false,
                ContextMetadata::new(
                    ContextSource::ToolResult,
                    ContextScope::Run,
                    ContextRetention::Retained,
                )
                .with_group(group),
            ),
        ]);

        let checkpoint = frame.checkpoint_items().unwrap();
        let restored = ContextFrame::from_checkpoint_items(checkpoint).unwrap();

        restored.validate_complete_tool_protocol().unwrap();
        let messages = restored.to_messages();
        assert_eq!(messages[1].images[0].data_base64, "YWJj");
        assert_eq!(messages[2].tool_calls[0].args["path"], "notes.txt");
        assert_eq!(
            serde_json::to_value(frame.manifest()).unwrap(),
            serde_json::to_value(restored.manifest()).unwrap()
        );
    }

    #[test]
    fn checkpoint_rejects_request_only_context() {
        let frame = ContextFrame::new(vec![ContextItem::text(
            LlmMessageRole::User,
            "transient",
            ContextSource::RuntimeExtension,
            ContextScope::Run,
            ContextRetention::RequestOnly,
        )]);

        assert!(frame.checkpoint_items().is_err());
    }

    #[test]
    fn persistent_replacement_keeps_run_overlay_and_discards_old_history() {
        let detector =
            ContextCapacityDetector::for_model("test-model", AgentApiStyle::OpenAiCompatible, &[]);
        let mut replacement = ContextFrame::new(vec![
            ContextItem::text(
                LlmMessageRole::System,
                "new rules",
                ContextSource::BackendSystemPrompt,
                ContextScope::Run,
                ContextRetention::Retained,
            ),
            ContextItem::text(
                LlmMessageRole::Assistant,
                "compacted history",
                ContextSource::ConversationSummary,
                ContextScope::Conversation,
                ContextRetention::Retained,
            ),
            ContextItem::text(
                LlmMessageRole::User,
                "current request",
                ContextSource::CurrentTurn,
                ContextScope::Conversation,
                ContextRetention::Retained,
            ),
        ]);
        detector.prepare_frame(&mut replacement);
        let replacement = replacement.share_measured_persistent_baseline().unwrap();

        let mut active = ContextFrame::new(vec![
            ContextItem::text(
                LlmMessageRole::System,
                "old rules",
                ContextSource::BackendSystemPrompt,
                ContextScope::Run,
                ContextRetention::Retained,
            ),
            ContextItem::text(
                LlmMessageRole::User,
                "very old history",
                ContextSource::ConversationHistory,
                ContextScope::Conversation,
                ContextRetention::Retained,
            ),
            ContextItem::text(
                LlmMessageRole::Assistant,
                "current run narration",
                ContextSource::ModelResponse,
                ContextScope::Run,
                ContextRetention::Retained,
            ),
        ]);
        detector.prepare_frame(&mut active);

        let replaced = active.replace_persistent_baseline(replacement);
        let messages = replaced.to_messages();

        assert_eq!(messages.len(), 4);
        assert_eq!(messages[0].content, "new rules");
        assert_eq!(messages[1].content, "compacted history");
        assert_eq!(messages[2].content, "current request");
        assert_eq!(messages[3].content, "current run narration");
        assert!(messages
            .iter()
            .all(|message| !message.content.contains("very old history")));
    }
}
