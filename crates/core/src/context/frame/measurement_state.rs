use super::*;

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
    pub(crate) semantic: ContextFrameSemanticBreakdown,
}

impl ContextFrameEstimateBreakdown {
    fn merge(
        &mut self,
        class: ContextUsageClass,
        sources: &[ContextSource],
        estimate: ContextMessageEstimate,
    ) {
        match class {
            ContextUsageClass::Fixed => self.fixed.merge(estimate),
            ContextUsageClass::Durable => self.durable.merge(estimate),
            ContextUsageClass::RunTransient => self.run_transient.merge(estimate),
            ContextUsageClass::RequestOnly => self.request_only.merge(estimate),
        }
        self.semantic.merge(sources, estimate.total_tokens());
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ContextFrameSemanticBreakdown {
    pub(crate) system_tokens: u64,
    pub(crate) summary_tokens: u64,
    pub(crate) continuity_tokens: u64,
    pub(crate) world_state_tokens: u64,
    pub(crate) goal_tokens: u64,
    pub(crate) todo_tokens: u64,
    pub(crate) recent_history_tokens: u64,
}

impl ContextFrameSemanticBreakdown {
    fn merge(&mut self, sources: &[ContextSource], tokens: u64) {
        let target = if sources.contains(&ContextSource::BackendSystemPrompt) {
            &mut self.system_tokens
        } else if sources.contains(&ContextSource::ConversationSummary) {
            &mut self.summary_tokens
        } else if sources.contains(&ContextSource::ContinuityIndex) {
            &mut self.continuity_tokens
        } else if sources.contains(&ContextSource::WorldStateSnapshot)
            || sources.contains(&ContextSource::WorldStateDiff)
        {
            &mut self.world_state_tokens
        } else if sources.contains(&ContextSource::ConversationGoal) {
            &mut self.goal_tokens
        } else if sources.contains(&ContextSource::RuntimeTodo) {
            &mut self.todo_tokens
        } else {
            // Recent history is the residual model-visible frame: uncovered conversation tail,
            // current-turn activity, attachments, Skills, runtime guards, and tool protocol.
            &mut self.recent_history_tokens
        };
        *target = target.saturating_add(tokens);
    }

    pub(crate) fn total_tokens(self) -> u64 {
        self.system_tokens
            .saturating_add(self.summary_tokens)
            .saturating_add(self.continuity_tokens)
            .saturating_add(self.world_state_tokens)
            .saturating_add(self.goal_tokens)
            .saturating_add(self.todo_tokens)
            .saturating_add(self.recent_history_tokens)
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
            measurement
                .breakdown
                .merge(usage_class, item.metadata.sources(), estimate);
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
                let estimate = item.measure(estimator.as_ref());
                breakdown.merge(usage_class, item.metadata.sources(), estimate);
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
        let frame = Self::new(items);
        frame.validate_cache_layout()?;
        Ok(frame)
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

    /// Validates the physical cache layout independently from token accounting.
    ///
    /// A request can append within one cache band or advance to a more volatile band, but it must
    /// never insert a later-lived item ahead of an already-established prefix.
    pub(crate) fn validate_cache_layout(&self) -> AgentResult<()> {
        let mut previous = None;
        for (index, item) in self.iter_items().enumerate() {
            let band = item.metadata.cache_band();
            if previous.is_some_and(|previous| band < previous) {
                return Err(AgentError::new(format!(
                    "上下文缓存分层顺序无效：第 {index} 项 `{}` 出现在更晚缓存层之后。",
                    band.as_str()
                )));
            }
            previous = Some(band);
        }
        Ok(())
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
                    origin_kind: item.metadata.origin().map(|origin| origin.kind().as_str()),
                    origin_id: item.metadata.origin().map(ContextOrigin::id),
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
                breakdown.merge(
                    item.metadata.usage_class(),
                    item.metadata.sources(),
                    estimate,
                );
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
