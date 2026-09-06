//! Authoritative, conversation-level durable context state.
//!
//! This state exists independently of any presentation feature. Capacity protection, future
//! compaction and the frontend context indicator all consume projections of the same measured
//! baseline.

use super::{
    ContextAssembler, ContextCapacityDetector, ContextFrame, ContextItem, ContextMetadata,
    ContextOrigin, ContextRetention, ContextScope, ContextSource, ConversationTimingTracker,
    ConversationTraceRenderer, MeasuredContextBaseline,
};
use crate::llm::LlmMessageRole;
use crate::protocol::{
    AgentContextWindowSnapshot, AgentError, AgentResult, AgentRunToolSetCheckpoint,
    AgentSkillActivation, AgentToolDefinition,
};
use crate::{ConversationModelContextItem, ConversationTurnTrace, WorldStateSnapshot};

#[derive(Clone)]
pub struct AgentContextWindowToolProjection {
    tool_set: AgentRunToolSetCheckpoint,
    initial_run_world_state: WorldStateSnapshot,
    dynamic_definitions: Vec<AgentToolDefinition>,
    conversation_world_state: Vec<crate::AnchoredWorldStateRecord>,
    conversation_preview_sections: Option<Vec<crate::WorldStateSectionEnvelope>>,
    capability_context: Vec<ContextItem>,
}

#[cfg(test)]
mod world_state_tests {
    use super::*;
    use crate::world_state::{
        AnchoredWorldStateRecord, WorldStateDiff, WorldStateLifetime, WorldStateRecord,
        WorldStateSectionEnvelope, WorldStateSectionId,
    };

    fn records() -> Vec<AnchoredWorldStateRecord> {
        let section = |available| {
            WorldStateSectionEnvelope::model_visible(
                WorldStateSectionId::extension("web.search").unwrap(),
                WorldStateLifetime::Conversation,
                serde_json::json!({"available": available}),
                serde_json::json!({"available": available}),
            )
            .unwrap()
        };
        let full = WorldStateSnapshot::new("conversation", 0, vec![section(false)]).unwrap();
        let target = WorldStateSnapshot::new("conversation", 1, vec![section(true)]).unwrap();
        let mut difference = AnchoredWorldStateRecord::new(
            WorldStateRecord::Diff(WorldStateDiff::between(&full, &target).unwrap()),
            Some("user".into()),
        )
        .unwrap();
        difference.model_observed = false;
        vec![
            AnchoredWorldStateRecord::new(WorldStateRecord::Full(full), None).unwrap(),
            difference,
        ]
    }

    fn state(records: Vec<AnchoredWorldStateRecord>) -> AgentConversationContextState {
        let assembled =
            ContextAssembler::assemble_with_timing(crate::context::ContextAssemblyInput {
                system_prompt: "rules".into(),
                compaction_summary: None,
                world_state_records: records,
                initial_run_world_state: None,
                messages: vec![crate::AgentChatMessage {
                    message_id: Some("user".into()),
                    role: "user".into(),
                    content: "input".into(),
                    created_at: None,
                    conversation_turn_trace: None,
                    conversation_model_context_items: Vec::new(),
                }],
                skill_discovery: None,
                skill_activation: None,
                attachments: crate::context::ContextAttachments::default(),
            })
            .unwrap();
        AgentConversationContextState::new(
            "configuration".into(),
            "test-model".into(),
            Some(32_000),
            1024,
            ContextCapacityDetector::for_model(
                "test-model",
                crate::AgentApiStyle::OpenAiCompatible,
                &[],
            ),
            assembled.frame,
            assembled.timing,
        )
    }

    #[test]
    fn conversation_state_world_state_sync_is_idempotent_and_matches_cold_rebuild() {
        let records = records();
        let mut incremental = state(records[..1].to_vec());
        assert_eq!(
            incremental
                .sync_conversation_world_state_records(&records)
                .unwrap(),
            1
        );
        assert_eq!(
            incremental
                .sync_conversation_world_state_records(&records)
                .unwrap(),
            0
        );
        assert_eq!(
            incremental.world_state_revision(),
            Some(records.last().unwrap().record.result_revision())
        );
        let mut cold = state(records);
        assert_eq!(incremental.snapshot(), cold.snapshot());
        assert_eq!(
            incremental
                .shared_baseline()
                .unwrap()
                .into_frame()
                .to_messages(),
            cold.shared_baseline().unwrap().into_frame().to_messages()
        );
    }

    #[test]
    fn world_state_preview_counts_current_state_and_guidance_without_mutating_baseline() {
        let records = records();
        let mut state = state(records[..1].to_vec());
        let before = state.snapshot();
        let projection = AgentContextWindowToolProjection::new(
            AgentRunToolSetCheckpoint {
                stable_revision: "stable".into(),
                dynamic_revision: "dynamic".into(),
                effective_revision: "effective".into(),
                active_capability_ids: Vec::new(),
                exposed_tool_names: Vec::new(),
            },
            WorldStateSnapshot::new(
                "run",
                0,
                vec![crate::world_state::model_capabilities_section(
                    crate::ModelCapabilities::default(),
                    WorldStateLifetime::Run,
                )
                .unwrap()],
            )
            .unwrap(),
            Vec::new(),
        );
        let base = state
            .snapshot_with_skill_overlays_and_tool_projection(None, None, &projection)
            .unwrap();
        let WorldStateRecord::Full(initial) = &records[0].record else {
            unreachable!()
        };
        let WorldStateRecord::Diff(diff) = &records[1].record else {
            unreachable!()
        };
        let desired =
            crate::WorldStateReducer::fold(initial.clone(), std::slice::from_ref(diff)).unwrap();
        let projected = projection
            .with_conversation_world_state_sections(desired.sections)
            .with_capability_context(vec![ContextItem::text(
                LlmMessageRole::System,
                "current capability guidance ".repeat(100),
                ContextSource::CapabilityInstructions,
                ContextScope::Run,
                ContextRetention::RequestOnly,
            )]);
        let exact = state
            .snapshot_with_skill_overlays_and_tool_projection(None, None, &projected)
            .unwrap();
        assert!(exact.input_tokens > base.input_tokens);
        assert!(exact.cost_breakdown.world_state_tokens > base.cost_breakdown.world_state_tokens);
        assert_eq!(state.snapshot(), before);
    }
}

impl std::fmt::Debug for AgentContextWindowToolProjection {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AgentContextWindowToolProjection")
            .field("stable_revision", &self.tool_set.stable_revision)
            .field("dynamic_revision", &self.tool_set.dynamic_revision)
            .field("effective_revision", &self.tool_set.effective_revision)
            .field(
                "run_world_state_revision",
                &self.initial_run_world_state.revision,
            )
            .field("dynamic_definition_count", &self.dynamic_definitions.len())
            .finish()
    }
}

impl AgentContextWindowToolProjection {
    pub(crate) fn new(
        tool_set: AgentRunToolSetCheckpoint,
        initial_run_world_state: WorldStateSnapshot,
        dynamic_definitions: Vec<AgentToolDefinition>,
    ) -> Self {
        Self {
            tool_set,
            initial_run_world_state,
            dynamic_definitions,
            conversation_world_state: Vec::new(),
            conversation_preview_sections: None,
            capability_context: Vec::new(),
        }
    }

    /// Request-scoped read-only projection; it is applied to the preview copy and never mutates
    /// the cached conversation ledger or writes a request-adoption marker.
    pub fn with_conversation_world_state(
        mut self,
        records: Vec<crate::AnchoredWorldStateRecord>,
    ) -> Self {
        self.conversation_world_state = records;
        self
    }

    /// Complete desired conversation snapshot for a read-only preview. Unlike canonical records,
    /// these sections do not require a fabricated assistant or request-boundary anchor.
    pub fn with_conversation_world_state_sections(
        mut self,
        sections: Vec<crate::WorldStateSectionEnvelope>,
    ) -> Self {
        self.conversation_preview_sections = Some(sections);
        self
    }

    #[cfg(test)]
    pub(crate) fn conversation_preview_sections(&self) -> &[crate::WorldStateSectionEnvelope] {
        self.conversation_preview_sections
            .as_deref()
            .unwrap_or_default()
    }

    pub(crate) fn with_capability_context(mut self, context: Vec<ContextItem>) -> Self {
        self.capability_context = context;
        self
    }

    pub fn stable_revision(&self) -> &str {
        &self.tool_set.stable_revision
    }

    pub fn dynamic_revision(&self) -> &str {
        &self.tool_set.dynamic_revision
    }

    pub fn effective_revision(&self) -> &str {
        &self.tool_set.effective_revision
    }

    /// Returns the exact Tool-set authority frozen by the same Host projection a real run uses.
    /// Approval recovery fixtures and Host persistence can reuse this instead of reproducing the
    /// registry hashing algorithm or hard-coding revision strings.
    pub fn tool_set_checkpoint(&self) -> AgentRunToolSetCheckpoint {
        self.tool_set.clone()
    }

    pub(crate) fn initial_run_world_state(&self) -> &WorldStateSnapshot {
        &self.initial_run_world_state
    }

    pub fn dynamic_definitions(&self) -> &[AgentToolDefinition] {
        &self.dynamic_definitions
    }
}

#[derive(Debug, Clone)]
pub struct AgentContextBaseline {
    configuration_revision: String,
    frame: MeasuredContextBaseline,
}

impl AgentContextBaseline {
    pub(crate) fn matches_configuration(&self, configuration_revision: &str) -> bool {
        self.configuration_revision == configuration_revision
    }

    pub(crate) fn into_frame(self) -> ContextFrame {
        ContextFrame::from_measured_baseline(self.frame)
    }

    pub(crate) fn rebase_restored_frame(self, frame: ContextFrame) -> ContextFrame {
        frame.rebase_onto_measured_baseline(self.frame)
    }

    /// Adopts a backend reconstruction of the complete compacted model timeline. Unlike a normal
    /// persistent refresh, this drops the runtime's duplicate message/tool overlay while keeping
    /// exact non-journal run state such as guards and activated runtime extensions.
    pub(crate) fn replace_compacted_model_history(self, frame: ContextFrame) -> ContextFrame {
        frame.replace_compacted_model_history(self.frame)
    }
}

/// Cached durable state for one conversation and one context configuration.
pub struct AgentConversationContextState {
    configuration_revision: String,
    model: String,
    context_window_tokens: Option<u32>,
    reserved_output_tokens: u32,
    detector: ContextCapacityDetector,
    frame: ContextFrame,
    timing: ConversationTimingTracker,
    /// Exact Host head last synchronized into the cache. Host-only changes advance this cursor
    /// even when they produce no model item; adoption can change at an otherwise identical head.
    world_state_head: Option<(String, u64, String)>,
}

impl AgentConversationContextState {
    pub(crate) fn new(
        configuration_revision: String,
        model: String,
        context_window_tokens: Option<u32>,
        reserved_output_tokens: u32,
        detector: ContextCapacityDetector,
        mut frame: ContextFrame,
        timing: ConversationTimingTracker,
    ) -> Self {
        // Durable storage has no notion of an active "current turn". Normalizing the source once
        // avoids rewriting frozen history when a later user message is appended.
        frame.begin_new_conversation_turn();
        detector.prepare_frame(&mut frame);
        let world_state_head = frame
            .conversation_world_state_records()
            .last()
            .map(|entry| {
                (
                    entry.record.epoch_id().to_string(),
                    entry.record.sequence(),
                    entry.record.result_revision().to_string(),
                )
            });
        Self {
            configuration_revision,
            model,
            context_window_tokens,
            reserved_output_tokens,
            detector,
            frame,
            timing,
            world_state_head,
        }
    }

    pub fn configuration_revision(&self) -> &str {
        &self.configuration_revision
    }

    /// Adds newly committed World State records at their durable message/trace anchors. This is
    /// idempotent and shares the projection/origin contract with cold context assembly.
    pub fn sync_conversation_world_state_records(
        &mut self,
        records: &[crate::AnchoredWorldStateRecord],
    ) -> AgentResult<usize> {
        let head = records.last().map(|entry| {
            (
                entry.record.epoch_id().to_string(),
                entry.record.sequence(),
                entry.record.result_revision().to_string(),
            )
        });
        if let (
            Some((epoch, sequence, revision)),
            Some((next_epoch, next_sequence, next_revision)),
        ) = (&self.world_state_head, &head)
        {
            if epoch == next_epoch
                && (next_sequence < sequence
                    || (next_sequence == sequence && next_revision != revision))
            {
                return Err(AgentError::new(
                    "Conversation World State 增量游标倒退或 revision 冲突。",
                ));
            }
        }
        let appended = self
            .frame
            .sync_conversation_world_state_records(records, false)?;
        self.world_state_head = head;
        Ok(appended)
    }

    pub fn mark_conversation_world_state_observed(&mut self) {
        self.frame.mark_conversation_world_state_observed();
    }

    pub fn world_state_revision(&self) -> Option<&str> {
        self.world_state_head
            .as_ref()
            .map(|(_, _, revision)| revision.as_str())
    }

    /// Grants the runtime API one narrow, Host-private hydration boundary before measurement.
    ///
    /// Provider-native replay state must never be projected into `AgentChatInput` or a durable
    /// context baseline. The public preview entry point uses this accessor only to run the exact
    /// same validated hydration step as a real model request.
    pub(crate) fn provider_hydration_frame_mut(&mut self) -> &mut ContextFrame {
        &mut self.frame
    }

    pub fn append_user_message(
        &mut self,
        message_id: Option<&str>,
        content: &str,
        created_at: Option<i64>,
    ) -> AgentResult<()> {
        let content = content.trim();
        if content.is_empty() {
            return Ok(());
        }
        let mut timing = self.timing.clone();
        let content = timing.render_user_message(content, created_at)?;
        let mut metadata = ContextMetadata::new(
            ContextSource::ConversationHistory,
            ContextScope::Conversation,
            ContextRetention::Retained,
        );
        if let Some(message_id) = message_id {
            metadata = metadata.with_origin(ContextOrigin::conversation_message(message_id));
        }
        self.frame.push(ContextItem::new(
            crate::llm::LlmMessage::text(LlmMessageRole::User, content),
            metadata,
        ));
        self.timing = timing;
        Ok(())
    }

    /// Number of context items produced by the exact-prefix/trace-suffix renderer.
    ///
    /// This is intentionally not the physical trace row count: legacy traces can contain
    /// run-scoped records (for example Todo) that the current model-visible projection omits.
    pub fn rendered_trace_activity_count(
        trace: &ConversationTurnTrace,
        model_context_items: &[ConversationModelContextItem],
    ) -> AgentResult<usize> {
        Ok(
            ConversationTraceRenderer::render_with_model_context(trace, model_context_items)?
                .activity_items
                .len(),
        )
    }

    pub fn append_trace_items(
        &mut self,
        trace: &ConversationTurnTrace,
        model_context_items: &[ConversationModelContextItem],
        committed_item_count: usize,
    ) -> AgentResult<usize> {
        let rendered =
            ConversationTraceRenderer::render_with_model_context(trace, model_context_items)?;
        let model_context_item_count = rendered.activity_items.len();
        if committed_item_count > model_context_item_count {
            return Err(AgentError::new(
                "会话上下文状态的模型日志游标超过已持久化项目数量。",
            ));
        }
        if committed_item_count == model_context_item_count {
            return Ok(committed_item_count);
        }
        for item in rendered
            .activity_items
            .into_iter()
            .skip(committed_item_count)
        {
            self.frame.push(item);
        }
        Ok(model_context_item_count)
    }

    pub fn finalize_conversation_turn(
        &mut self,
        trace: &ConversationTurnTrace,
        model_context_items: &[ConversationModelContextItem],
        committed_item_count: usize,
        assistant_content: &str,
        assistant_created_at: Option<i64>,
    ) -> AgentResult<usize> {
        if !trace.terminal_status.is_terminal() {
            return Err(AgentError::new("运行中的会话轨迹不能作为上下文终态提交。"));
        }
        let mut timing = self.timing.clone();
        timing.observe_assistant(assistant_created_at)?;
        let committed_item_count =
            self.append_trace_items(trace, model_context_items, committed_item_count)?;
        let assistant_content = assistant_content.trim();
        if !assistant_content.is_empty() {
            self.frame.push(ContextItem::new(
                crate::llm::LlmMessage::text(LlmMessageRole::Assistant, assistant_content),
                ContextMetadata::new(
                    ContextSource::ConversationHistory,
                    ContextScope::Conversation,
                    ContextRetention::Retained,
                )
                .with_origin(ContextOrigin::conversation_message(
                    &trace.assistant_message_id,
                )),
            ));
        }
        let terminal_only = ConversationTurnTrace {
            items: Vec::new(),
            ..trace.clone()
        };
        if let Some(terminal_item) =
            ConversationTraceRenderer::render(&terminal_only)?.terminal_item
        {
            self.frame.push(terminal_item);
        }
        self.timing = timing;
        Ok(committed_item_count)
    }

    pub fn shared_baseline(&mut self) -> AgentResult<AgentContextBaseline> {
        // Ensure pending durable appends have measurements before freezing the next immutable
        // chunk. Existing chunks are reused by Arc and are never measured again.
        self.detector.prepare_frame(&mut self.frame);
        Ok(AgentContextBaseline {
            configuration_revision: self.configuration_revision.clone(),
            frame: self.frame.share_measured_persistent_baseline()?,
        })
    }

    pub fn snapshot(&mut self) -> AgentContextWindowSnapshot {
        self.detector
            .inspect(
                &mut self.frame,
                self.context_window_tokens,
                self.reserved_output_tokens,
            )
            .snapshot(&self.model)
    }

    /// Measures a current-run Skill selection on top of the immutable durable cache without
    /// admitting that selection into conversation state. This is the preview counterpart of the
    /// runtime's shared-baseline path and keeps `configuration_revision`/`persistent_revision`
    /// stable while accounting for the run-transient token cost.
    pub fn snapshot_with_skill_activation(
        &mut self,
        activation: Option<&AgentSkillActivation>,
    ) -> AgentResult<AgentContextWindowSnapshot> {
        self.snapshot_with_skill_overlays(None, activation)
    }

    /// Measures the current run's discoverable catalog and activated instructions on top of the
    /// immutable durable cache. Neither overlay enters the conversation's persistent revision.
    pub fn snapshot_with_skill_overlays(
        &mut self,
        discovery: Option<&crate::skills::AgentSkillDiscoverySnapshot>,
        activation: Option<&AgentSkillActivation>,
    ) -> AgentResult<AgentContextWindowSnapshot> {
        let baseline = self.shared_baseline()?;
        let mut preview = baseline.into_frame();
        ContextAssembler::append_skill_overlays(&mut preview, discovery, activation)?;
        preview.mark_initial_run_input();
        Ok(self
            .detector
            .inspect(
                &mut preview,
                self.context_window_tokens,
                self.reserved_output_tokens,
            )
            .snapshot(&self.model))
    }

    /// Measures the run-transient baseline for production requests: the exact
    /// backend-owned Run World State snapshot, Skill overlays and provider Tool schemas.
    pub fn snapshot_with_skill_overlays_and_tool_projection(
        &mut self,
        discovery: Option<&crate::skills::AgentSkillDiscoverySnapshot>,
        activation: Option<&AgentSkillActivation>,
        projection: &AgentContextWindowToolProjection,
    ) -> AgentResult<AgentContextWindowSnapshot> {
        let baseline = self.shared_baseline()?;
        let mut preview = baseline.into_frame();
        preview
            .sync_conversation_world_state_records(&projection.conversation_world_state, false)?;
        let conversation_preview = projection
            .conversation_preview_sections
            .as_ref()
            .map(|sections| preview.conversation_world_state_preview(sections))
            .transpose()?
            .flatten();
        ContextAssembler::append_initial_run_world_state(
            &mut preview,
            Some(projection.initial_run_world_state()),
        )?;
        ContextAssembler::append_skill_overlays(&mut preview, discovery, activation)?;
        preview.mark_initial_run_input();
        if let Some(item) = conversation_preview {
            preview.push(item);
        }
        for item in &projection.capability_context {
            preview.push(item.clone());
        }
        Ok(self
            .detector
            .inspect_with_dynamic_tools(
                &mut preview,
                self.context_window_tokens,
                self.reserved_output_tokens,
                projection.dynamic_definitions(),
            )
            .snapshot(&self.model))
    }
}
