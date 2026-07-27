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
    AgentContextWindowPhase, AgentContextWindowSnapshot, AgentError, AgentResult,
    AgentSkillActivation, AgentToolDefinition,
};
use crate::{ConversationModelContextItem, ConversationTurnTrace, WorldStateSnapshot};

#[derive(Clone)]
pub struct AgentContextWindowToolProjection {
    stable_revision: String,
    dynamic_revision: String,
    effective_revision: String,
    initial_run_world_state: WorldStateSnapshot,
    dynamic_definitions: Vec<AgentToolDefinition>,
}

impl std::fmt::Debug for AgentContextWindowToolProjection {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AgentContextWindowToolProjection")
            .field("stable_revision", &self.stable_revision)
            .field("dynamic_revision", &self.dynamic_revision)
            .field("effective_revision", &self.effective_revision)
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
        stable_revision: String,
        dynamic_revision: String,
        effective_revision: String,
        initial_run_world_state: WorldStateSnapshot,
        dynamic_definitions: Vec<AgentToolDefinition>,
    ) -> Self {
        Self {
            stable_revision,
            dynamic_revision,
            effective_revision,
            initial_run_world_state,
            dynamic_definitions,
        }
    }

    pub fn stable_revision(&self) -> &str {
        &self.stable_revision
    }

    pub fn dynamic_revision(&self) -> &str {
        &self.dynamic_revision
    }

    pub fn effective_revision(&self) -> &str {
        &self.effective_revision
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
        Self {
            configuration_revision,
            model,
            context_window_tokens,
            reserved_output_tokens,
            detector,
            frame,
            timing,
        }
    }

    pub fn configuration_revision(&self) -> &str {
        &self.configuration_revision
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
    /// run-scoped records (for example Todo) that the compatibility renderer omits.
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

    pub fn snapshot(&mut self, phase: AgentContextWindowPhase) -> AgentContextWindowSnapshot {
        self.detector
            .inspect(
                &mut self.frame,
                self.context_window_tokens,
                self.reserved_output_tokens,
            )
            .persistent_snapshot(&self.model, phase)
    }

    /// Measures a current-run Skill selection on top of the immutable durable cache without
    /// admitting that selection into conversation state. This is the preview counterpart of the
    /// runtime's shared-baseline path and keeps `configuration_revision`/`persistent_revision`
    /// stable while accounting for the run-transient token cost.
    pub fn snapshot_with_skill_activation(
        &mut self,
        phase: AgentContextWindowPhase,
        activation: Option<&AgentSkillActivation>,
    ) -> AgentResult<AgentContextWindowSnapshot> {
        self.snapshot_with_skill_overlays(phase, None, activation)
    }

    /// Measures the current run's discoverable catalog and activated instructions on top of the
    /// immutable durable cache. Neither overlay enters the conversation's persistent revision.
    pub fn snapshot_with_skill_overlays(
        &mut self,
        phase: AgentContextWindowPhase,
        discovery: Option<&crate::skills::AgentSkillDiscoverySnapshot>,
        activation: Option<&AgentSkillActivation>,
    ) -> AgentResult<AgentContextWindowSnapshot> {
        let baseline = self.shared_baseline()?;
        let mut preview = baseline.into_frame();
        ContextAssembler::append_skill_overlays(&mut preview, discovery, activation)?;
        Ok(self
            .detector
            .inspect(
                &mut preview,
                self.context_window_tokens,
                self.reserved_output_tokens,
            )
            .persistent_snapshot(&self.model, phase))
    }

    /// Measures the complete run-transient projection used by production requests: the exact
    /// backend-owned Run World State snapshot, Skill overlays and provider Tool schemas.
    pub fn snapshot_with_skill_overlays_and_tool_projection(
        &mut self,
        phase: AgentContextWindowPhase,
        discovery: Option<&crate::skills::AgentSkillDiscoverySnapshot>,
        activation: Option<&AgentSkillActivation>,
        projection: &AgentContextWindowToolProjection,
    ) -> AgentResult<AgentContextWindowSnapshot> {
        let baseline = self.shared_baseline()?;
        let mut preview = baseline.into_frame();
        ContextAssembler::append_initial_run_world_state(
            &mut preview,
            Some(projection.initial_run_world_state()),
        )?;
        ContextAssembler::append_skill_overlays(&mut preview, discovery, activation)?;
        Ok(self
            .detector
            .inspect_with_dynamic_tools(
                &mut preview,
                self.context_window_tokens,
                self.reserved_output_tokens,
                projection.dynamic_definitions(),
            )
            .persistent_snapshot(&self.model, phase))
    }
}
