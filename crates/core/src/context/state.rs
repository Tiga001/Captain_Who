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
    AgentSkillActivation,
};
use crate::{ConversationTurnTrace, ConversationTurnTraceTerminalStatus};

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

    pub(crate) fn replace_persistent_context(self, frame: ContextFrame) -> ContextFrame {
        frame.replace_persistent_baseline(self.frame)
    }

    pub(crate) fn promote_committed_trace(self, frame: ContextFrame) -> ContextFrame {
        frame.promote_committed_trace(self.frame)
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

    pub fn append_trace_items(
        &mut self,
        trace: &ConversationTurnTrace,
        committed_item_count: usize,
    ) -> AgentResult<usize> {
        if committed_item_count > trace.items.len() {
            return Err(AgentError::new(
                "会话上下文状态的 trace 游标超过已持久化项目数量。",
            ));
        }
        if committed_item_count == trace.items.len() {
            return Ok(committed_item_count);
        }
        let suffix = ConversationTurnTrace {
            schema_version: trace.schema_version,
            run_id: trace.run_id.clone(),
            conversation_id: trace.conversation_id.clone(),
            assistant_message_id: trace.assistant_message_id.clone(),
            terminal_status: ConversationTurnTraceTerminalStatus::InProgress,
            terminal_error: None,
            truncated: trace.truncated,
            items: trace.items[committed_item_count..].to_vec(),
        };
        let rendered = ConversationTraceRenderer::render(&suffix)?;
        for item in rendered.activity_items {
            self.frame.push(item);
        }
        Ok(trace.items.len())
    }

    pub fn finalize_conversation_turn(
        &mut self,
        trace: &ConversationTurnTrace,
        committed_item_count: usize,
        assistant_content: &str,
        assistant_created_at: Option<i64>,
    ) -> AgentResult<usize> {
        if !trace.terminal_status.is_terminal() {
            return Err(AgentError::new("运行中的会话轨迹不能作为上下文终态提交。"));
        }
        let mut timing = self.timing.clone();
        timing.observe_assistant(assistant_created_at)?;
        let committed_item_count = self.append_trace_items(trace, committed_item_count)?;
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
}
