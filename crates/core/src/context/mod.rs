//! Provider-neutral context assembly and capacity accounting for agent requests.
//!
//! Persistence adapters load raw inputs; this module validates, orders, labels, and groups them.
//! At the network boundary it also estimates the fully assembled request against the configured
//! model window. Immutable items cache versioned measurements, while each frame keeps an
//! incrementally updated aggregate. It deliberately does not query storage or perform compaction,
//! so those policies can evolve without coupling the agent loop to SQLite or a provider tokenizer.

mod assembler;
mod budget;
mod frame;
mod measurement;
mod trace_renderer;

pub(crate) use assembler::{ContextAssembler, ContextAssemblyInput, ContextAttachments};
pub(crate) use budget::{ContextBudgetReport, ContextCapacityDetector, ContextCompactionQuery};
pub(crate) use frame::{
    ContextFrame, ContextGroup, ContextItem, ContextMetadata, ContextRetention, ContextScope,
    ContextSource,
};
pub(crate) use trace_renderer::ConversationTraceRenderer;
