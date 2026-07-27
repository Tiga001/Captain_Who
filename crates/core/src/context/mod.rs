//! Provider-neutral context assembly and capacity accounting for agent requests.
//!
//! Persistence adapters load raw inputs; this module validates, orders, labels, and groups them.
//! At the network boundary it also estimates the fully assembled request against the configured
//! model window. Immutable items cache versioned measurements, while each frame keeps an
//! incrementally updated aggregate. It deliberately does not query storage or perform compaction,
//! so those policies can evolve without coupling the agent loop to SQLite or a provider tokenizer.

mod assembler;
mod budget;
mod compaction;
mod compaction_summary;
mod continuity;
mod frame;
mod measurement;
mod message_time;
mod state;
mod trace_renderer;

pub(crate) use assembler::{
    activated_skill_context_item, AssembledContext, ContextAssembler, ContextAssemblyInput,
    ContextAttachments,
};
pub(crate) use budget::{
    ContextBudgetReport, ContextBudgetStatus, ContextCapacityDetector, ContextCompactionQuery,
    ContextMeasurementMode,
};
#[cfg(test)]
pub(crate) use compaction::{
    ContextCompactionDurablePrefix, ContextCompactionProtectedEstimate, ContextCompactionStep,
};
pub(crate) use compaction::{
    ContextCompactionPlan, ContextCompactionPlanStatus, ContextCompactionPlanner,
};
pub(crate) use compaction_summary::render_compaction_semantic_summary_for_context;
pub use compaction_summary::{
    ContextCompactionGeneration, ContextCompactionGenerationKind, ContextCompactionPrefix,
    ContextCompactionSourceItem, ContextCompactionSummary, ContextCompactionSummaryDraft,
    ContextJournalCursor, CONTEXT_COMPACTION_SUMMARY_SCHEMA_VERSION,
};
pub use continuity::{
    ContextContinuityEntry, ContextContinuitySnapshot, ContextContinuityText, ContextHistoryRef,
    ContinuityIndexV2, CONTEXT_CONTINUITY_HARD_MAX_TOKENS, CONTEXT_CONTINUITY_SCHEMA_VERSION,
    CONTEXT_CONTINUITY_TARGET_TOKENS, CONTEXT_CONTINUITY_V1_SCHEMA_VERSION,
};
pub(crate) use frame::{
    ContextFrame, ContextGroup, ContextItem, ContextMetadata, ContextOrigin, ContextRetention,
    ContextScope, ContextSource, MeasuredContextBaseline,
};
pub(crate) use measurement::ContextTextBudget;
pub(crate) use message_time::{format_message_created_at, ConversationTimingTracker};
pub use state::{
    AgentContextBaseline, AgentContextWindowToolProjection, AgentConversationContextState,
};
pub(crate) use trace_renderer::ConversationTraceRenderer;
