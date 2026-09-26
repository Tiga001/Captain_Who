use super::*;
use std::collections::VecDeque;

#[path = "world_state.rs"]
mod world_state;

#[derive(Debug, Clone)]
pub(crate) struct ContextFrame {
    baseline: Option<MeasuredContextBaseline>,
    /// Items appended after the shared durable baseline. Runtime clones only this overlay.
    items: Vec<ContextItem>,
    revision: u64,
    persistent_revision: u64,
    measurement: Option<ContextFrameMeasurementState>,
    conversation_world_state_records: Arc<[crate::AnchoredWorldStateRecord]>,
}

/// Immutable, already-measured conversation context shared by the server cache and active runs.
/// Chunks preserve append-only updates without copying older durable items.
#[derive(Debug, Clone)]
pub(crate) struct MeasuredContextBaseline {
    chunks: Arc<[Arc<[ContextItem]>]>,
    revision: u64,
    persistent_revision: u64,
    measurement: ContextFrameMeasurementState,
    conversation_world_state_records: Arc<[crate::AnchoredWorldStateRecord]>,
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
    pub(crate) world_state_tokens: u64,
    pub(crate) todo_tokens: u64,
    pub(crate) recent_history_tokens: u64,
}

impl ContextFrameSemanticBreakdown {
    fn merge(&mut self, sources: &[ContextSource], tokens: u64) {
        let target = if sources.contains(&ContextSource::BackendSystemPrompt) {
            &mut self.system_tokens
        } else if sources.contains(&ContextSource::ConversationSummary) {
            &mut self.summary_tokens
        } else if sources.contains(&ContextSource::WorldStateSnapshot)
            || sources.contains(&ContextSource::WorldStateDiff)
        {
            &mut self.world_state_tokens
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
            .saturating_add(self.world_state_tokens)
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
    /// Resolve only Host-provided, immutable attachments. Never reopen arbitrary paths from
    /// model-authored history or reinterpret a historical reference as current tool authority.
    pub(crate) fn hydrate_context_images(
        &mut self,
        attachments: &[crate::AgentInputAttachment],
    ) -> AgentResult<()> {
        use base64::Engine;
        use sha2::{Digest, Sha256};
        if !self
            .iter_items()
            .any(|item| !item.context_image_refs.is_empty())
        {
            return Ok(());
        }
        self.materialize_baseline();
        let mut changed = false;
        for item in &mut self.items {
            if item.context_image_refs.is_empty() {
                continue;
            }
            let mut images = Vec::new();
            for reference in &item.context_image_refs {
                let attachment = attachments
                    .iter()
                    .find(|attachment| attachment.id == reference.attachment_id)
                    .ok_or_else(|| AgentError::new("Historical context image is unavailable."))?;
                if attachment.mime_type.as_deref() != Some(reference.mime_type.as_str())
                    || attachment.encoding != crate::AgentInputAttachmentEncoding::Base64
                {
                    return Err(AgentError::new(
                        "Historical context image identity mismatch.",
                    ));
                }
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(&attachment.data)
                    .map_err(|_| AgentError::new("Invalid historical context image encoding."))?;
                if format!("sha256:{:x}", Sha256::digest(&bytes)) != reference.sha256 {
                    return Err(AgentError::new("Historical context image content changed."));
                }
                images.push(crate::llm::LlmImage {
                    mime_type: reference.mime_type.clone(),
                    data_base64: attachment.data.clone(),
                });
            }
            if item.message.images() != images {
                *item.message.images_mut().ok_or_else(|| {
                    AgentError::new("Historical image has an invalid message role.")
                })? = images;
                item.measurement = None;
                changed = true;
            }
        }
        if changed {
            self.revision = self.revision.saturating_add(1);
            self.persistent_revision = persistent_frame_revision(&self.items);
            self.measurement = None;
        }
        Ok(())
    }

    /// Returns not-yet-journaled Run materials in the exact order used by the model request.
    pub(crate) fn pending_run_materials(
        &self,
    ) -> Vec<(usize, crate::ConversationContextMaterialKind, LlmMessage)> {
        let indices = self
            .iter_items()
            .enumerate()
            .map(|(index, item)| (item as *const ContextItem, index))
            .collect::<BTreeMap<_, _>>();
        self.model_request_items()
            .into_iter()
            .filter_map(|item| {
                if item.metadata.scope != ContextScope::Run
                    || item
                        .metadata
                        .origin()
                        .and_then(ContextOrigin::journal_cursor)
                        .is_some()
                {
                    return None;
                }
                let sources = item.metadata.sources();
                let kind = if sources.contains(&ContextSource::InputAttachment) {
                    crate::ConversationContextMaterialKind::InputAttachment
                } else if sources.contains(&ContextSource::SkillInstructions) {
                    crate::ConversationContextMaterialKind::SkillInstructions
                } else if sources.contains(&ContextSource::WorldStateSnapshot)
                    || sources.contains(&ContextSource::WorldStateDiff)
                {
                    crate::ConversationContextMaterialKind::RunWorldState
                } else {
                    return None;
                };
                Some((
                    indices[&(item as *const ContextItem)],
                    kind,
                    item.message.clone(),
                ))
            })
            .collect()
    }

    pub(crate) fn bind_run_material(
        &mut self,
        index: usize,
        assistant_message_id: &str,
        sequence: u64,
        images: Vec<crate::ConversationContextImageRef>,
    ) {
        self.materialize_baseline();
        self.items[index].metadata.origin = Some(ContextOrigin::conversation_trace_item(
            assistant_message_id,
            sequence,
        ));
        self.items[index].context_image_refs = images;
        self.items[index].measurement = None;
        self.revision = self.revision.saturating_add(1);
        self.persistent_revision = persistent_frame_revision(&self.items);
        self.measurement = None;
    }
    pub(crate) fn new(items: Vec<ContextItem>) -> Self {
        let revision = u64::try_from(items.len()).unwrap_or(u64::MAX);
        let persistent_revision = persistent_frame_revision(&items);
        Self {
            baseline: None,
            items,
            revision,
            persistent_revision,
            measurement: None,
            conversation_world_state_records: Arc::from([]),
        }
    }

    /// Final provider-bound invariant for the single Tool-result limit.
    ///
    /// Normal execution, approval recovery and legacy hydration all apply the semantic gate before
    /// constructing the frame. This last check prevents an old checkpoint or pre-gate persisted
    /// model log from bypassing that contract. Complete collaboration messages are exempt only
    /// when a preceding tool call proves their identity; the whole-frame capacity gate still runs.
    pub(crate) fn ensure_model_tool_results_fit(
        &self,
        gate: &crate::context::ModelToolResultGate,
    ) -> AgentResult<()> {
        let mut calls = BTreeMap::new();
        for item in self.iter_items() {
            for call in item.message.tool_calls() {
                calls
                    .entry(call.id.as_str())
                    .and_modify(|exempt| *exempt = false)
                    .or_insert_with(|| {
                        crate::context::ModelToolResultGate::preserves_full_result(&call.name)
                    });
            }
            let full_collaboration_result = item
                .message
                .tool_call_id()
                .and_then(|id| calls.remove(id))
                .unwrap_or(false);
            if item.message.role() == LlmMessageRole::Tool
                && !full_collaboration_result
                && !gate.admits_message(&item.message)
            {
                return Err(AgentError::new(format!(
                    "模型上下文中的工具结果 `{}` 超过统一 10K token 上限，且无法在发送前安全重建。",
                    item.message.tool_call_id().unwrap_or("unknown")
                )));
            }
        }
        Ok(())
    }

    /// Returns the ordered, payload-free Provider continuation handles retained by this frame.
    ///
    /// Duplicate handles would mean the same provider-owned turn was projected into the model
    /// timeline more than once, so checkpoint creation fails closed instead of silently deduping.
    pub(crate) fn provider_continuation_refs(
        &self,
    ) -> AgentResult<Vec<crate::protocol::ProviderContinuationRef>> {
        let mut seen = BTreeSet::new();
        let mut refs = Vec::new();
        for item in self.iter_items() {
            let Some(continuation_ref) = item
                .message
                .assistant_turn()
                .and_then(LlmAssistantTurn::provider_continuation_ref)
            else {
                continue;
            };
            continuation_ref.validate().map_err(|error| {
                AgentError::new(format!(
                    "模型上下文中的 Provider continuation ref 无效：{error}"
                ))
            })?;
            if !seen.insert(continuation_ref.id.as_str()) {
                return Err(AgentError::new(
                    "模型上下文重复引用同一个 Provider continuation。",
                ));
            }
            refs.push(continuation_ref.clone());
        }
        Ok(refs)
    }

    pub(crate) fn has_tool_bearing_assistant_turns(&self) -> bool {
        self.iter_items().any(|item| {
            item.message.role() == LlmMessageRole::Assistant
                && item.message.tool_calls().next().is_some()
        })
    }

    pub(crate) fn contains_trace_for_assistant_message(&self, assistant_message_id: &str) -> bool {
        self.iter_items().any(|item| {
            item.metadata
                .origin()
                .and_then(ContextOrigin::journal_cursor)
                .is_some_and(|cursor| cursor.message_id() == assistant_message_id)
        })
    }

    pub(crate) fn contains_provider_continuation_projection(
        &self,
        assistant_message_id: &str,
        projection: crate::ProviderContinuationProjection,
    ) -> bool {
        self.iter_items().any(|item| {
            let Some(origin) = item.metadata.origin() else {
                return false;
            };
            match projection {
                crate::ProviderContinuationProjection::ConversationMessage => {
                    item.message.role() == LlmMessageRole::Assistant
                        && origin.kind() == ContextOriginKind::ConversationMessage
                        && origin
                            .journal_cursor()
                            .is_some_and(|cursor| cursor.message_id() == assistant_message_id)
                }
                crate::ProviderContinuationProjection::ConversationTraceItem {
                    sequence,
                    ordinal,
                } => {
                    // An ordinary narration is the sole Assistant projection at ordinal zero for
                    // its trace sequence. Other ordinals belong to split Tool Call projections
                    // and must never be rebound as an ordinary Provider turn.
                    item.message.role() == LlmMessageRole::Assistant
                        && ordinal == 0
                        && origin.kind() == ContextOriginKind::ConversationTraceItem
                        && origin.journal_cursor().is_some_and(|cursor| {
                            cursor.message_id() == assistant_message_id
                                && cursor.trace_sequence() == Some(sequence)
                        })
                }
                crate::ProviderContinuationProjection::ConversationSteerBoundary {
                    guidance_sequence,
                } => {
                    item.message.role() == LlmMessageRole::User
                        && origin.kind() == ContextOriginKind::ConversationTraceItem
                        && origin.journal_cursor().is_some_and(|cursor| {
                            cursor.message_id() == assistant_message_id
                                && cursor.trace_sequence() == Some(guidance_sequence)
                        })
                }
            }
        })
    }

    pub(crate) fn journal_provider_continuation_projections(
        &self,
    ) -> Vec<(String, crate::ProviderContinuationProjection)> {
        let mut seen = std::collections::HashSet::new();
        self.iter_items()
            .filter_map(|item| {
                let origin = item.metadata.origin()?;
                let cursor = origin.journal_cursor()?;
                let projection = match (item.message.role(), origin.kind()) {
                    (LlmMessageRole::Assistant, ContextOriginKind::ConversationMessage) => {
                        crate::ProviderContinuationProjection::ConversationMessage
                    }
                    (LlmMessageRole::Assistant, ContextOriginKind::ConversationTraceItem) => {
                        crate::ProviderContinuationProjection::ConversationTraceItem {
                            sequence: cursor.trace_sequence()?,
                            ordinal: 0,
                        }
                    }
                    (LlmMessageRole::User, ContextOriginKind::ConversationTraceItem) => {
                        crate::ProviderContinuationProjection::ConversationSteerBoundary {
                            guidance_sequence: cursor.trace_sequence()?,
                        }
                    }
                    _ => return None,
                };
                let candidate = (cursor.message_id().to_string(), projection);
                seen.insert(candidate.clone()).then_some(candidate)
            })
            .collect()
    }

    pub(crate) fn ensure_tool_bearing_turns_replayable(
        &self,
        protocol: &crate::provider_profile::ProviderProtocolKey,
        reasoning_mode: crate::provider_profile::ReasoningMode,
    ) -> AgentResult<()> {
        for item in self.iter_items() {
            let Some(turn) = item.message.assistant_turn() else {
                continue;
            };
            if turn.effective_tool_calls().is_empty() {
                continue;
            }
            let continuation_is_compatible = match turn.provider_continuation() {
                Some(continuation) => continuation.validate_for(protocol, turn.digest()).is_ok(),
                None => reasoning_mode != crate::provider_profile::ReasoningMode::Enabled,
            };
            let compatible = turn.provider_protocol() == Some(protocol)
                && turn.provider_continuation_ref().is_some()
                && continuation_is_compatible;
            if !compatible {
                return Err(AgentError::structured(
                    "provider_context_boundary_required",
                    "当前 Provider Profile 无法安全回放历史工具上下文；请先压缩该历史边界。",
                    serde_json::json!({
                        "type": "providerContextBoundary",
                        "recovery": "compactIncompatibleToolHistory"
                    }),
                ));
            }
        }
        Ok(())
    }

    /// Replaces the durable Generic one-call projection of one Assistant Turn with the exact
    /// provider-owned turn loaded from the encrypted continuation vault.
    ///
    /// The durable Trace remains split for compatibility. This transformation is confined to the
    /// in-memory provider context and unifies all matching Tool results under one semantic group.
    pub(crate) fn restore_provider_assistant_turn(
        &mut self,
        assistant_message_id: &str,
        projection: Option<crate::ProviderContinuationProjection>,
        turn: LlmAssistantTurn,
    ) -> AgentResult<()> {
        if assistant_message_id.trim().is_empty() || turn.provider_continuation_ref().is_none() {
            return Err(provider_turn_restore_error(
                "provider_continuation_corrupt",
                "Provider Assistant Turn 恢复绑定无效。",
            ));
        }
        if turn.provider_tool_calls().is_empty() {
            let projection = projection.ok_or_else(|| {
                provider_turn_restore_error(
                    "provider_continuation_corrupt",
                    "普通 Provider Assistant Turn 缺少精确持久化投影。",
                )
            })?;
            return self.restore_ordinary_provider_assistant_turn(
                assistant_message_id,
                projection,
                turn,
            );
        }
        if projection.is_some() {
            return Err(provider_turn_restore_error(
                "provider_continuation_corrupt",
                "Tool-bearing Provider Assistant Turn 不应绑定普通消息投影。",
            ));
        }
        let bindings = turn
            .runtime_tool_bindings()
            .ok_or_else(|| {
                provider_turn_restore_error(
                    "provider_continuation_corrupt",
                    "Provider Assistant Turn 缺少 Runtime Tool Call 映射。",
                )
            })?
            .to_vec();
        if bindings.is_empty() {
            return Err(provider_turn_restore_error(
                "provider_continuation_corrupt",
                "可回放 Provider Assistant Turn 不含 Tool Call。",
            ));
        }
        let expected_ids = bindings
            .iter()
            .map(|binding| binding.runtime_call.id.clone())
            .collect::<Vec<_>>();
        let expected_id_set = expected_ids
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        if expected_id_set.len() != expected_ids.len() {
            return Err(provider_turn_restore_error(
                "provider_continuation_corrupt",
                "Provider Assistant Turn 的 Runtime Tool Call 重复。",
            ));
        }

        self.materialize_baseline();
        let group = ContextGroup::tool_exchange(format!("provider-turn:{}", turn.stable_id()));
        let mut retained = Vec::with_capacity(self.items.len());
        let mut insertion_index = None;
        let mut assistant_metadata = None;
        let mut observed_ids = Vec::new();
        let mut checkpoint_calls = Vec::new();

        for mut item in self.items.clone() {
            let belongs_to_message = item
                .metadata
                .origin()
                .and_then(ContextOrigin::journal_cursor)
                .is_some_and(|cursor| cursor.message_id() == assistant_message_id);
            if belongs_to_message
                && item
                    .metadata
                    .sources
                    .contains(&ContextSource::ToolTurnNarration)
                && item.metadata.group.as_ref().is_some_and(|group| {
                    group.id == format!("narration-owner:{}:{}", turn.stable_id(), expected_ids[0])
                })
            {
                // This public trace row is a projection of this exact response, not another
                // Assistant message. Match response and first canonical call identity, never text.
                continue;
            }
            if belongs_to_message && item.message.role() == LlmMessageRole::Assistant {
                let call_ids = item
                    .message
                    .tool_calls()
                    .map(|call| call.id.as_str())
                    .collect::<Vec<_>>();
                if !call_ids.is_empty()
                    && call_ids
                        .iter()
                        .all(|call_id| expected_id_set.contains(call_id))
                {
                    let checkpoint_message =
                        item.checkpoint_message.as_ref().unwrap_or(&item.message);
                    for call_id in &call_ids {
                        let checkpoint_call = checkpoint_message
                            .tool_calls()
                            .find(|call| call.id == **call_id)
                            .cloned()
                            .ok_or_else(|| {
                                provider_turn_restore_error(
                                    "provider_continuation_corrupt",
                                    "Provider Assistant Turn 的安全 Checkpoint 投影不完整。",
                                )
                            })?;
                        checkpoint_calls.push(checkpoint_call);
                    }
                    insertion_index.get_or_insert(retained.len());
                    if assistant_metadata.is_none() {
                        assistant_metadata = Some(item.metadata.clone());
                    }
                    observed_ids.extend(call_ids.into_iter().map(str::to_string));
                    continue;
                }
            }
            if belongs_to_message
                && item.message.role() == LlmMessageRole::Tool
                && item
                    .message
                    .tool_call_id()
                    .is_some_and(|call_id| expected_id_set.contains(call_id))
            {
                item.metadata.group = Some(group.clone());
            }
            retained.push(item);
        }

        if observed_ids != expected_ids {
            return Err(provider_turn_restore_error(
                "provider_continuation_missing",
                "模型历史缺少 Provider Assistant Turn 的完整 Tool Call 投影。",
            ));
        }
        let insertion_index = insertion_index.ok_or_else(|| {
            provider_turn_restore_error(
                "provider_continuation_missing",
                "模型历史中不存在 Provider Assistant Turn。",
            )
        })?;
        let metadata = assistant_metadata
            .ok_or_else(|| {
                provider_turn_restore_error(
                    "provider_continuation_corrupt",
                    "Provider Assistant Turn 缺少模型上下文元数据。",
                )
            })?
            .with_group(group);
        let mut checkpoint_turn = turn.without_raw_continuation_for_checkpoint();
        let checkpoint_bindings = bindings
            .iter()
            .zip(checkpoint_calls)
            .map(|(binding, runtime_call)| -> AgentResult<_> {
                let provider_call = checkpoint_turn
                    .provider_tool_calls()
                    .get(binding.provider_tool_index)
                    .ok_or_else(|| {
                        provider_turn_restore_error(
                            "provider_continuation_corrupt",
                            "Provider Assistant Turn 的 Checkpoint 映射越界。",
                        )
                    })?;
                Ok(crate::llm::LlmRuntimeToolCallBinding::new(
                    binding.provider_tool_index,
                    provider_call,
                    runtime_call,
                ))
            })
            .collect::<AgentResult<Vec<_>>>()?;
        checkpoint_turn.set_runtime_tool_bindings(checkpoint_bindings)?;
        let restored_item = ContextItem::new(LlmMessage::from_assistant_turn(turn), metadata)
            .with_checkpoint_message(LlmMessage::from_assistant_turn(checkpoint_turn));
        retained.insert(insertion_index, restored_item);
        self.items = retained;
        self.measurement = None;
        self.revision = self.revision.saturating_add(1);
        self.persistent_revision = persistent_frame_revision(&self.items);
        Ok(())
    }

    /// Restores private provider state for an ordinary (non-tool-bearing) assistant message.
    ///
    /// Durable conversation history contains only the visible assistant text. Provider-native
    /// reasoning stays in the encrypted continuation vault and is reattached here only after the
    /// exact terminal message or steer-narration projection has been authenticated.
    fn restore_ordinary_provider_assistant_turn(
        &mut self,
        assistant_message_id: &str,
        projection: crate::ProviderContinuationProjection,
        turn: LlmAssistantTurn,
    ) -> AgentResult<()> {
        if turn
            .runtime_tool_bindings()
            .is_some_and(|bindings| !bindings.is_empty())
        {
            return Err(provider_turn_restore_error(
                "provider_continuation_corrupt",
                "普通 Provider Assistant Turn 不应包含 Tool Call 映射。",
            ));
        }
        let continuation_ref = turn
            .provider_continuation_ref()
            .expect("validated provider continuation ref")
            .clone();

        self.materialize_baseline();
        let (target_index, metadata, insert_before_target) = match projection {
            crate::ProviderContinuationProjection::ConversationMessage => {
                // The trace renderer may append a backend-state terminal record with the same
                // ConversationMessage origin as the real final message. It is an audit boundary,
                // never the owner of provider-native state.
                let mut message_indices =
                    self.items.iter().enumerate().filter_map(|(index, item)| {
                        (item.message.role() == LlmMessageRole::Assistant
                            && !item
                                .metadata
                                .sources()
                                .contains(&ContextSource::ConversationTrace)
                            && item.metadata.origin().is_some_and(|origin| {
                                origin.kind() == ContextOriginKind::ConversationMessage
                                    && origin.journal_cursor().is_some_and(|cursor| {
                                        cursor.message_id() == assistant_message_id
                                    })
                            }))
                        .then_some(index)
                    });
                if let Some(index) = message_indices.next() {
                    if message_indices.next().is_some() {
                        return Err(provider_turn_restore_error(
                            "provider_continuation_corrupt",
                            "模型历史中的普通 Provider Assistant 投影不唯一。",
                        ));
                    }
                    (index, self.items[index].metadata.clone(), false)
                } else {
                    // Empty assistant messages are intentionally omitted from ordinary history.
                    // The durable terminal audit remains as a private ordering anchor, so retain
                    // it and insert the restored empty provider turn immediately before it.
                    if !turn.visible_text().trim().is_empty() {
                        return Err(provider_turn_restore_error(
                            "provider_continuation_missing",
                            "模型历史中不存在与 Provider continuation 匹配的普通 Assistant Turn。",
                        ));
                    }
                    let mut terminal_indices =
                        self.items.iter().enumerate().filter_map(|(index, item)| {
                            let sources = item.metadata.sources();
                            (sources.contains(&ContextSource::ConversationTrace)
                                && sources.contains(&ContextSource::BackendState)
                                && item.metadata.origin().is_some_and(|origin| {
                                    origin.kind() == ContextOriginKind::ConversationMessage
                                        && origin.journal_cursor().is_some_and(|cursor| {
                                            cursor.message_id() == assistant_message_id
                                        })
                                }))
                            .then_some(index)
                        });
                    let Some(index) = terminal_indices.next() else {
                        return Err(provider_turn_restore_error(
                            "provider_continuation_missing",
                            "空的普通 Provider Assistant Turn 缺少精确终态边界。",
                        ));
                    };
                    if terminal_indices.next().is_some() {
                        return Err(provider_turn_restore_error(
                            "provider_continuation_corrupt",
                            "空的普通 Provider Assistant Turn 终态边界不唯一。",
                        ));
                    }
                    (
                        index,
                        ContextMetadata::new(
                            ContextSource::ConversationHistory,
                            ContextScope::Conversation,
                            ContextRetention::Retained,
                        )
                        .with_origin(ContextOrigin::conversation_message(assistant_message_id)),
                        true,
                    )
                }
            }
            crate::ProviderContinuationProjection::ConversationTraceItem { sequence, ordinal } => {
                let mut matching_indices =
                    self.items.iter().enumerate().filter_map(|(index, item)| {
                        (item.message.role() == LlmMessageRole::Assistant
                            && ordinal == 0
                            && item.metadata.origin().is_some_and(|origin| {
                                origin.kind() == ContextOriginKind::ConversationTraceItem
                                    && origin.journal_cursor().is_some_and(|cursor| {
                                        cursor.message_id() == assistant_message_id
                                            && cursor.trace_sequence() == Some(sequence)
                                    })
                            }))
                        .then_some(index)
                    });
                let Some(index) = matching_indices.next() else {
                    return Err(provider_turn_restore_error(
                        "provider_continuation_missing",
                        "模型历史中不存在与 Provider continuation 匹配的普通 Assistant Turn。",
                    ));
                };
                if matching_indices.next().is_some() {
                    return Err(provider_turn_restore_error(
                        "provider_continuation_corrupt",
                        "模型历史中的普通 Provider Assistant 投影不唯一。",
                    ));
                }
                (index, self.items[index].metadata.clone(), false)
            }
            crate::ProviderContinuationProjection::ConversationSteerBoundary {
                guidance_sequence,
            } => {
                if !turn.visible_text().trim().is_empty() {
                    return Err(provider_turn_restore_error(
                        "provider_continuation_corrupt",
                        "Steer 边界只能恢复无可见正文的普通 Provider Assistant Turn。",
                    ));
                }
                let has_origin = |item: &ContextItem| {
                    item.metadata.origin().is_some_and(|origin| {
                        origin.kind() == ContextOriginKind::ConversationTraceItem
                            && origin.journal_cursor().is_some_and(|cursor| {
                                cursor.message_id() == assistant_message_id
                                    && cursor.trace_sequence() == Some(guidance_sequence)
                            })
                    })
                };
                let guidance_indices = self
                    .items
                    .iter()
                    .enumerate()
                    .filter_map(|(index, item)| {
                        (item.message.role() == LlmMessageRole::User && has_origin(item))
                            .then_some(index)
                    })
                    .collect::<Vec<_>>();
                let [guidance_index] = guidance_indices.as_slice() else {
                    return Err(provider_turn_restore_error(
                        if guidance_indices.is_empty() {
                            "provider_continuation_missing"
                        } else {
                            "provider_continuation_corrupt"
                        },
                        "Steer Provider continuation 缺少唯一的 UserGuidance 边界。",
                    ));
                };
                let assistant_indices = self
                    .items
                    .iter()
                    .enumerate()
                    .filter_map(|(index, item)| {
                        (item.message.role() == LlmMessageRole::Assistant && has_origin(item))
                            .then_some(index)
                    })
                    .collect::<Vec<_>>();
                match assistant_indices.as_slice() {
                    [] => (
                        *guidance_index,
                        ContextMetadata::new(
                            ContextSource::ConversationTrace,
                            ContextScope::Conversation,
                            ContextRetention::Retained,
                        )
                        .with_origin(
                            ContextOrigin::conversation_trace_item(
                                assistant_message_id,
                                guidance_sequence,
                            ),
                        ),
                        true,
                    ),
                    [assistant_index]
                        if assistant_index.checked_add(1) == Some(*guidance_index) =>
                    {
                        (
                            *assistant_index,
                            self.items[*assistant_index].metadata.clone(),
                            false,
                        )
                    }
                    _ => {
                        return Err(provider_turn_restore_error(
                            "provider_continuation_corrupt",
                            "Steer Provider continuation 的 Assistant/UserGuidance 顺序无效。",
                        ));
                    }
                }
            }
        };

        if !insert_before_target {
            let candidate = self.items[target_index]
                .message
                .assistant_turn()
                .expect("the selected item is an assistant message");
            if !candidate.provider_tool_calls().is_empty()
                || candidate
                    .runtime_tool_bindings()
                    .is_some_and(|bindings| !bindings.is_empty())
                || candidate
                    .provider_continuation_ref()
                    .is_some_and(|candidate_ref| candidate_ref != &continuation_ref)
                || (candidate.provider_continuation().is_some()
                    && candidate.provider_continuation_ref() != Some(&continuation_ref))
            {
                return Err(provider_turn_restore_error(
                    "provider_continuation_corrupt",
                    "普通 Provider Assistant Turn 与精确持久化投影冲突。",
                ));
            }
        }

        let checkpoint_turn = turn.without_raw_continuation_for_checkpoint();
        let restored = ContextItem::new(LlmMessage::from_assistant_turn(turn), metadata)
            .with_checkpoint_message(LlmMessage::from_assistant_turn(checkpoint_turn));
        if insert_before_target {
            self.items.insert(target_index, restored);
        } else {
            self.items[target_index] = restored;
        }
        self.measurement = None;
        self.revision = self.revision.saturating_add(1);
        self.persistent_revision = persistent_frame_revision(&self.items);
        Ok(())
    }

    pub(crate) fn from_measured_baseline(baseline: MeasuredContextBaseline) -> Self {
        Self {
            revision: baseline.revision,
            persistent_revision: baseline.persistent_revision,
            measurement: Some(baseline.measurement.clone()),
            conversation_world_state_records: Arc::clone(
                &baseline.conversation_world_state_records,
            ),
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
        rebased.conversation_world_state_records = self.conversation_world_state_records;
        for item in overlay {
            rebased.push(item);
        }
        rebased
    }

    #[cfg(test)]
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

    /// Replaces the model-visible conversation journal with an authoritative backend baseline
    /// after compaction. Message, guidance and tool protocol items are reconstructed from durable
    /// Trace/ModelContext storage; retaining their run overlay would duplicate the exact tail.
    pub(crate) fn replace_compacted_model_history(self, baseline: MeasuredContextBaseline) -> Self {
        let baseline = if baseline.conversation_world_state_records.is_empty()
            && !self.conversation_world_state_records.is_empty()
        {
            self.preserve_world_state_for_compaction(baseline)
        } else {
            baseline
        };
        // Compaction restores journaled model/tool messages as ConversationTrace items, while
        // request-local World State and newly activated Skill items remain in the run overlay.
        // Keep a separate provider-layout sequence so those retained observations keep their
        // causal positions without moving the authoritative journal or changing its lifetime.
        let ordered = self.model_request_items();
        let request_orders = ordered
            .iter()
            .enumerate()
            .map(|(index, item)| (*item as *const ContextItem, index))
            .collect::<BTreeMap<_, _>>();
        let result_orders = ordered
            .iter()
            .enumerate()
            .filter_map(|(index, item)| {
                let call_id = item.message.tool_call_id()?;
                let cursor = item.metadata.origin()?.journal_cursor()?;
                Some((
                    (cursor.message_id().to_string(), call_id.to_string()),
                    index,
                ))
            })
            .collect::<BTreeMap<_, _>>();
        let mut placements = BTreeMap::<_, VecDeque<_>>::new();
        let mut split_call_placements = BTreeMap::new();
        for (index, item) in ordered.iter().enumerate() {
            let sources = item.metadata.sources();
            let placement = if sources.contains(&ContextSource::RunInput) {
                Some(ContextSource::RunInput)
            } else if sources.contains(&ContextSource::RunTimeline)
                || (item.metadata.scope() == ContextScope::Run
                    && item.metadata.retention() == ContextRetention::Retained
                    && !sources.contains(&ContextSource::BackendSystemPrompt)
                    && !sources.contains(&ContextSource::AutomationExecution)
                    && !sources.contains(&ContextSource::RunBootstrap))
            {
                Some(ContextSource::RunTimeline)
            } else {
                None
            };
            if let Some((origin, placement)) = item.metadata.origin().zip(placement) {
                if let Some(cursor) = origin.journal_cursor() {
                    for (ordinal, call) in item.message.tool_calls().enumerate() {
                        let key = (cursor.message_id().to_string(), call.id.clone());
                        // A live Assistant Turn can own several calls, while durable Generic
                        // history stores each call immediately before its own result. The
                        // subsequent call's result is the exact boundary after any retained
                        // observations produced by the preceding call, such as Skill loading.
                        let split_index = if ordinal == 0 {
                            index
                        } else {
                            result_orders.get(&key).copied().unwrap_or(index)
                        };
                        split_call_placements.insert(
                            key,
                            (placement, u64::try_from(split_index).unwrap_or(u64::MAX)),
                        );
                    }
                }
                // One trace sequence can have several split Tool Call projections. Role and
                // protocol identity distinguish them without comparing private/redacted text.
                let key = (
                    origin.kind().as_str(),
                    origin.id().to_string(),
                    item.message.role().as_str(),
                    item.message.tool_call_id().map(str::to_string),
                    item.message
                        .tool_calls()
                        .map(|call| call.id.clone())
                        .collect::<Vec<_>>(),
                );
                placements
                    .entry(key)
                    .or_default()
                    .push_back((placement, u64::try_from(index).unwrap_or(u64::MAX)));
            }
        }
        let live_materials = self
            .iter_items()
            .filter(|item| {
                item.metadata.scope() == ContextScope::Run
                    && item.metadata.sources().iter().any(|source| {
                        matches!(
                            source,
                            ContextSource::InputAttachment
                                | ContextSource::SkillInstructions
                                | ContextSource::WorldStateSnapshot
                                | ContextSource::WorldStateDiff
                        )
                    })
                    && item
                        .metadata
                        .origin()
                        .and_then(ContextOrigin::journal_cursor)
                        .is_some()
            })
            .filter_map(|item| {
                item.metadata
                    .origin()
                    .map(|origin| (origin.id().to_string(), item.clone()))
            })
            .collect::<BTreeMap<_, _>>();
        let baseline_origins = baseline
            .iter_items()
            .filter_map(|item| item.metadata.origin())
            .map(|origin| origin.id().to_string())
            .collect::<BTreeSet<_>>();
        let baseline_world_state_origins = baseline
            .iter_items()
            .filter_map(|item| item.metadata.origin())
            .filter(|origin| origin.kind() == ContextOriginKind::WorldStateRecord)
            .cloned()
            .collect::<Vec<_>>();
        let overlay = self
            .iter_items()
            .filter(|item| {
                let sources = item.metadata.sources();
                !sources.contains(&ContextSource::ModelResponse)
                    && !sources.contains(&ContextSource::ToolResult)
                    && !sources.contains(&ContextSource::ToolContinuation)
                    && !sources.contains(&ContextSource::UserGuidance)
                    && !sources.contains(&ContextSource::BackendState)
            })
            .filter(|item| {
                !item.metadata.origin().is_some_and(|origin| {
                    origin.kind() == ContextOriginKind::WorldStateRecord
                        && baseline_world_state_origins.contains(origin)
                })
            })
            .filter(|item| !item.metadata.usage_class().is_persistent())
            .filter(|item| {
                !item.metadata.origin().is_some_and(|origin| {
                    live_materials.contains_key(origin.id())
                        && baseline_origins.contains(origin.id())
                })
            })
            .map(|item| {
                let index = request_orders[&(item as *const ContextItem)];
                let mut item = item.clone();
                if item
                    .metadata
                    .origin()
                    .is_some_and(|origin| live_materials.contains_key(origin.id()))
                {
                    // The authoritative tail no longer contains this journal event. Preserve
                    // active run context, but never summarize the covered event a second time.
                    item.metadata = item.metadata.with_source(ContextSource::CompactionRetained);
                }
                if item.metadata.retention() == ContextRetention::Retained
                    && !item
                        .metadata
                        .sources()
                        .contains(&ContextSource::RunBootstrap)
                    && !item
                        .metadata
                        .sources()
                        .contains(&ContextSource::AutomationExecution)
                {
                    item.metadata = item
                        .metadata
                        .with_source(ContextSource::RunTimeline)
                        .with_request_order(u64::try_from(index).unwrap_or(u64::MAX));
                }
                item
            })
            .collect::<Vec<_>>();
        let mut replaced = Self::from_measured_baseline(baseline);
        // The baseline is a shared immutable cache. Only this run's copy receives layout tags.
        replaced.materialize_baseline();
        for item in &mut replaced.items {
            if let Some(live) = item
                .metadata
                .origin()
                .and_then(|origin| live_materials.get(origin.id()))
            {
                let covered = item
                    .metadata
                    .sources()
                    .contains(&ContextSource::CompactionRetained);
                *item = live.clone();
                if covered {
                    item.metadata = item
                        .metadata
                        .clone()
                        .with_source(ContextSource::CompactionRetained);
                }
            }
            let Some(origin) = item.metadata.origin() else {
                continue;
            };
            let key = (
                origin.kind().as_str(),
                origin.id().to_string(),
                item.message.role().as_str(),
                item.message.tool_call_id().map(str::to_string),
                item.message
                    .tool_calls()
                    .map(|call| call.id.clone())
                    .collect::<Vec<_>>(),
            );
            let placement = placements
                .get_mut(&key)
                .and_then(VecDeque::pop_front)
                .or_else(|| {
                    let mut calls = item.message.tool_calls();
                    let call = calls.next()?;
                    if calls.next().is_some() {
                        return None;
                    }
                    let cursor = origin.journal_cursor()?;
                    split_call_placements
                        .get(&(cursor.message_id().to_string(), call.id.clone()))
                        .copied()
                });
            if let Some((placement, order)) = placement {
                item.metadata = item
                    .metadata
                    .clone()
                    .with_source(placement)
                    .with_request_order(order);
            }
        }
        replaced.revision = replaced.revision.saturating_add(1);
        replaced.persistent_revision = persistent_frame_revision(&replaced.items);
        if !live_materials.is_empty() {
            // A durable baseline copy has just become a Run-scoped exact overlay. Its cached
            // bucket allocation is no longer valid, even when the message bytes are identical;
            // hydrated live images can also differ from the persisted reference-only copy.
            // Keep reusable item estimates, but rebuild the frame's classified total once.
            replaced.measurement = None;
        } else if let Some(measurement) = &mut replaced.measurement {
            measurement.full_recount = None;
        }
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
        if self.iter_items().any(|item| {
            !(item.metadata.usage_class().is_persistent()
                || (item.metadata.scope == ContextScope::Conversation
                    && item
                        .metadata
                        .sources()
                        .contains(&ContextSource::WorldStateUnobserved)))
        }) {
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
            conversation_world_state_records: Arc::clone(&self.conversation_world_state_records),
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
            .model_request_items()
            .into_iter()
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
                    role: item.message.role(),
                    sources: item.metadata.sources().to_vec(),
                    group_id: item.metadata.group().map(|group| group.id().to_string()),
                    tool_names: item
                        .message
                        .tool_calls()
                        .map(|call| call.name.clone())
                        .collect(),
                    image_count: estimate.image_count,
                    is_error: item.message.is_error(),
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

    /// Validates an approval boundary inside one provider assistant Tool Call batch.
    ///
    /// A provider turn can declare more than one Tool Call in one assistant message. The
    /// currently pending call and every not-yet-executed queued call therefore remain unresolved
    /// together. The checkpoint supplies the exact queued runtime identities; accepting any
    /// additional or missing unresolved call would detach execution state from model protocol
    /// state.
    pub(crate) fn validate_pending_tool_batch(
        &self,
        pending_tool_call_id: &str,
        queued_tool_call_ids: &[String],
    ) -> AgentResult<LlmToolCall> {
        let unresolved = self.unresolved_tool_calls()?;
        let mut expected = BTreeSet::new();
        if !expected.insert(pending_tool_call_id.to_string()) {
            return Err(AgentError::new("运行检查点中的待审批工具调用身份重复。"));
        }
        for call_id in queued_tool_call_ids {
            if !expected.insert(call_id.clone()) {
                return Err(AgentError::new(format!(
                    "运行检查点中的同批工具调用 id `{call_id}` 重复。",
                )));
            }
        }
        let actual = unresolved.keys().cloned().collect::<BTreeSet<_>>();
        if actual != expected {
            return Err(AgentError::new(format!(
                "运行检查点中的未结算工具调用与冻结批次不一致：期望 {} 个，实际 {} 个。",
                expected.len(),
                actual.len()
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

    /// Validates the full provider/runtime identity mapping against the one complete Assistant
    /// Turn retained in Context. Settled calls remain in this turn even when only a suffix is
    /// unresolved at an approval boundary.
    pub(crate) fn validate_assistant_turn_checkpoint_identity(
        &self,
        identity: &AgentAssistantTurnCheckpointIdentity,
        pending_tool_call_id: &str,
        queued_tool_call_ids: &[String],
        require_live_provider_identity: bool,
    ) -> AgentResult<()> {
        let pending_mapping_index = identity
            .tool_call_identities
            .iter()
            .position(|mapping| mapping.runtime_call_id == pending_tool_call_id)
            .ok_or_else(|| {
                AgentError::new("运行检查点的 Provider Tool Call 映射缺少待审批调用。")
            })?;
        let mut expected_runtime_ids = identity.tool_call_identities[..pending_mapping_index]
            .iter()
            .map(|mapping| mapping.runtime_call_id.as_str())
            .collect::<Vec<_>>();
        expected_runtime_ids.push(pending_tool_call_id);
        expected_runtime_ids.extend(queued_tool_call_ids.iter().map(String::as_str));
        let mut matching_turns = self.iter_items().filter_map(|item| {
            let turn = item.message.assistant_turn()?;
            let runtime_ids = turn
                .runtime_tool_bindings()?
                .iter()
                .map(|binding| binding.runtime_call.id.as_str())
                .collect::<Vec<_>>();
            (runtime_ids == expected_runtime_ids
                && (!require_live_provider_identity
                    || (turn.stable_id() == identity.assistant_turn_id
                        && turn.stable_digest() == identity.assistant_turn_digest)))
                .then_some(turn)
        });
        let turn = matching_turns.next().ok_or_else(|| {
            AgentError::new("运行检查点的完整 Provider Assistant Turn 不存在于模型上下文。")
        })?;
        if matching_turns.next().is_some() {
            return Err(AgentError::new(
                "运行检查点的 Provider Assistant Turn 身份在模型上下文中不唯一。",
            ));
        }
        let bindings = turn.runtime_tool_bindings().ok_or_else(|| {
            AgentError::new("运行检查点的 Assistant Turn 缺少 Provider/Runtime 身份映射。")
        })?;
        let actual_runtime_ids = bindings
            .iter()
            .map(|binding| binding.runtime_call.id.as_str())
            .collect::<Vec<_>>();
        if actual_runtime_ids != expected_runtime_ids {
            return Err(AgentError::new(
                "运行检查点的 Context Assistant Turn 未完整保留已结算前缀，或未按原序保留未结算调用。",
            ));
        }
        for binding in bindings {
            let mapping = identity
                .tool_call_identities
                .iter()
                .find(|mapping| mapping.runtime_call_id == binding.runtime_call.id)
                .ok_or_else(|| {
                    AgentError::new(
                        "运行检查点的 Context Assistant Turn 包含未冻结的 Runtime Tool Call。",
                    )
                })?;
            if u32::try_from(binding.provider_tool_index).ok() != Some(mapping.provider_tool_index)
                || binding.provider_call_id != mapping.provider_call_id
            {
                return Err(AgentError::new(
                    "运行检查点的 Context Assistant Turn 与 Provider/Runtime 身份映射不一致。",
                ));
            }
        }
        Ok(())
    }

    // The arguments mirror the immutable approval-boundary record. Keeping them explicit makes
    // it harder to accidentally substitute live runtime state during recovery.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn append_tool_continuation_in_batch(
        &mut self,
        call: &LlmToolCall,
        observation: String,
        checkpoint_observation: Option<String>,
        is_error: bool,
        is_mcp: bool,
        origin: Option<ContextOrigin>,
        remaining_tool_call_ids: &[String],
    ) -> AgentResult<()> {
        let checkpoint_call =
            self.validate_pending_tool_batch(&call.id, remaining_tool_call_ids)?;
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
                    .tool_calls()
                    .any(|tool_call| tool_call.id == call.id)
            })
            .and_then(|item| item.metadata.group().cloned())
            .ok_or_else(|| AgentError::new("待审批工具调用缺少原子工具交换分组。"))?;

        let mut metadata = ContextMetadata::new(
            ContextSource::ToolContinuation,
            ContextScope::Run,
            ContextRetention::Retained,
        )
        .with_source(ContextSource::ToolResult)
        .with_group(group);
        if is_mcp {
            metadata = metadata.with_source(ContextSource::McpToolResult);
        }
        if let Some(origin) = origin {
            metadata = metadata.with_origin(origin);
        }
        let mut item = ContextItem::tool_result(call.id.clone(), observation, is_error, metadata);
        if let Some(checkpoint_observation) = checkpoint_observation {
            item =
                item.with_checkpoint_tool_result(call.id.clone(), checkpoint_observation, is_error);
        }
        self.push(item);
        let unresolved = self.unresolved_tool_calls()?;
        let expected = remaining_tool_call_ids
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        let actual = unresolved.keys().cloned().collect::<BTreeSet<_>>();
        if actual != expected {
            return Err(AgentError::new("审批续跑后剩余工具调用与冻结批次不一致。"));
        }
        Ok(())
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

    /// Validates canonical journal/checkpoint bands independently from token accounting.
    ///
    /// The journal can append within one band or advance to a more volatile band, but it must
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
                .tool_calls()
                .any(|call| call.id == tool_call_id)
        })
    }

    /// Removes runtime-visible calls which the approval barrier deliberately deferred while
    /// retaining the raw provider turn in memory. Durable Context then contains neither their
    /// arguments nor external Tool names, and the provider-neutral protocol has no orphan call.
    pub(crate) fn omit_runtime_tool_calls_from_group(
        &mut self,
        group: &ContextGroup,
        omitted_call_ids: &BTreeSet<String>,
    ) -> AgentResult<()> {
        if omitted_call_ids.is_empty() {
            return Ok(());
        }
        self.materialize_baseline();
        let mut matched = false;
        for item in &mut self.items {
            if item.metadata.group() != Some(group) {
                continue;
            }
            let Some(turn) = item.message.assistant_turn_mut() else {
                continue;
            };
            let bindings = turn
                .runtime_tool_bindings()
                .ok_or_else(|| {
                    AgentError::new("无法延后 Tool Call：Assistant Turn 缺少 Runtime 身份映射。")
                })?
                .iter()
                .filter(|binding| !omitted_call_ids.contains(&binding.runtime_call.id))
                .cloned()
                .collect();
            turn.set_runtime_tool_bindings(bindings)?;
            if let Some(checkpoint_message) = item.checkpoint_message.as_mut() {
                let checkpoint_turn = checkpoint_message.assistant_turn_mut().ok_or_else(|| {
                    AgentError::new("无法延后 Tool Call：Checkpoint 投影不再是 Assistant Turn。")
                })?;
                let checkpoint_bindings = checkpoint_turn
                    .runtime_tool_bindings()
                    .unwrap_or_default()
                    .iter()
                    .filter(|binding| !omitted_call_ids.contains(&binding.runtime_call.id))
                    .cloned()
                    .collect();
                checkpoint_turn.set_runtime_tool_bindings(checkpoint_bindings)?;
            }
            item.measurement = None;
            matched = true;
            break;
        }
        if !matched {
            return Err(AgentError::new(
                "无法延后 Tool Call：模型上下文缺少对应的完整 Assistant Turn。",
            ));
        }
        self.persistent_revision = persistent_frame_revision(&self.items);
        self.revision = self.revision.saturating_add(1);
        self.measurement = None;
        Ok(())
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

    /// Provider layout is independent of the durable journal and compaction cursors.
    pub(crate) fn model_request_items(&self) -> Vec<&ContextItem> {
        super::request_layout::ordered_items(self.iter_items().collect())
    }

    pub(crate) fn into_model_request_messages(self) -> Vec<LlmMessage> {
        self.model_request_items()
            .into_iter()
            .map(|item| item.message.clone())
            .collect()
    }

    /// Mark the trailing user-input cohort on a new run's private frame. A shared baseline
    /// labels these as history; a cold assembly labels only the last as CurrentTurn. Include
    /// anchored state diffs between inputs so their causal order survives physical layout.
    pub(crate) fn mark_initial_run_input(&mut self) {
        let mut start = None;
        let items = self.iter_items().collect::<Vec<_>>();
        for (index, item) in items.into_iter().enumerate().rev() {
            if item.metadata.scope != ContextScope::Conversation {
                continue;
            }
            let sources = item.metadata.sources();
            if sources.contains(&ContextSource::WorldStateDiff) {
                if start.is_some() {
                    start = Some(index);
                }
                continue;
            }
            if (sources.contains(&ContextSource::ConversationHistory)
                || sources.contains(&ContextSource::CurrentTurn))
                && item.message.role() == LlmMessageRole::User
            {
                start = Some(index);
            } else {
                break;
            }
        }
        let Some(start) = start else { return };
        let shared_baseline = self.baseline.clone();
        self.materialize_baseline();
        for item in &mut self.items[start..] {
            if item.metadata.scope == ContextScope::Conversation {
                item.metadata = item.metadata.clone().with_source(ContextSource::RunInput);
            }
        }
        self.persistent_revision = persistent_frame_revision(&self.items);
        self.revision = self.revision.saturating_add(1);
        if let Some(measurement) = &mut self.measurement {
            measurement.full_recount = None;
        }
        if let Some(baseline) = shared_baseline {
            *self = std::mem::take(self).rebase_onto_measured_baseline(baseline);
        }
    }

    pub(crate) fn manifest(&self) -> ContextManifest<'_> {
        let request_indices = self
            .model_request_items()
            .into_iter()
            .enumerate()
            .map(|(index, item)| (item as *const ContextItem, index))
            .collect::<BTreeMap<_, _>>();
        ContextManifest {
            entries: self
                .iter_items()
                .enumerate()
                .map(|(index, item)| ContextManifestEntry {
                    index,
                    model_request_index: request_indices[&(item as *const ContextItem)],
                    request_order: item.metadata.request_order(),
                    role: item.message.role().as_str(),
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
                    text_character_count: item.message.content().chars().count(),
                    image_count: item.message.images().len(),
                    image_base64_bytes: item
                        .message
                        .images()
                        .iter()
                        .map(|image| image.data_base64.len())
                        .sum(),
                    tool_call_count: item.message.tool_calls().len(),
                    tool_argument_character_count: item
                        .message
                        .tool_calls()
                        .filter_map(|call| serde_json::to_string(&call.args).ok())
                        .map(|args| args.chars().count())
                        .sum(),
                    tool_call_id: item.message.tool_call_id(),
                    is_error: item.message.is_error(),
                })
                .collect(),
        }
    }

    fn unresolved_tool_calls(&self) -> AgentResult<BTreeMap<String, (LlmToolCall, ContextGroup)>> {
        let mut unresolved = BTreeMap::new();
        let mut unresolved_order = VecDeque::new();
        let mut seen_call_ids = BTreeSet::new();
        let mut current_exchange_has_result = false;
        for item in self.iter_items() {
            match item.message.role() {
                LlmMessageRole::Assistant if !item.message.tool_calls().is_empty() => {
                    if !unresolved.is_empty() {
                        return Err(AgentError::new(
                            "工具调用协议无效：上一组工具调用尚未获得完整结果。",
                        ));
                    }
                    let group = item.metadata.group().cloned().ok_or_else(|| {
                        AgentError::new("工具调用协议无效：assistant 工具调用缺少交换分组。")
                    })?;
                    current_exchange_has_result = false;
                    for call in item.message.tool_calls() {
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
                        unresolved_order.push_back(call.id.clone());
                    }
                }
                LlmMessageRole::Tool => {
                    let call_id = item.message.tool_call_id().ok_or_else(|| {
                        AgentError::new("工具调用协议无效：工具结果缺少 tool_call_id。")
                    })?;
                    let Some(expected_call_id) = unresolved_order.pop_front() else {
                        return Err(AgentError::new(format!(
                            "工具调用协议无效：工具结果 `{call_id}` 没有对应的未结算调用。"
                        )));
                    };
                    if expected_call_id != call_id {
                        return Err(AgentError::new(format!(
                            "工具调用协议无效：工具结果 `{call_id}` 未按 Assistant Turn 的调用顺序结算。"
                        )));
                    }
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
                    current_exchange_has_result = true;
                }
                _ if !unresolved.is_empty() && !current_exchange_has_result => {
                    return Err(AgentError::new(
                        "工具调用协议无效：工具调用与结果之间出现了其他消息。",
                    ));
                }
                _ => {}
            }
        }
        Ok(unresolved)
    }

    pub(super) fn iter_items(&self) -> impl DoubleEndedIterator<Item = &ContextItem> {
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

fn provider_turn_restore_error(code: &'static str, message: &'static str) -> AgentError {
    AgentError::structured(
        code,
        message,
        serde_json::json!({
            "type": "providerContinuation",
            "recovery": "restartFromSafeContextBoundary"
        }),
    )
}

impl MeasuredContextBaseline {
    pub(super) fn iter_items(&self) -> impl DoubleEndedIterator<Item = &ContextItem> {
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
                conversation_world_state_records: Arc::clone(
                    &self.conversation_world_state_records,
                ),
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
