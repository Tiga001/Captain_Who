//! Provider-neutral context assembly for agent requests.
//!
//! Persistence adapters load raw inputs; this module validates, orders, labels, and groups them.
//! It deliberately does not query storage or perform compaction, so those policies can evolve
//! without coupling the agent loop to SQLite or a specific model provider.

mod assembler;
mod frame;

pub(crate) use assembler::{ContextAssembler, ContextAssemblyInput, ContextAttachments};
pub(crate) use frame::{
    ContextFrame, ContextGroup, ContextItem, ContextMetadata, ContextRetention, ContextScope,
    ContextSource,
};
