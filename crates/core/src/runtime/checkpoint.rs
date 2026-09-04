//! Durable agent-loop checkpoints used at approval boundaries.
//!
//! A checkpoint owns every piece of in-memory state needed to resume the same logical run. The
//! pending tool call already appears as the final, unresolved exchange in `context`; queued calls
//! belong to the same model response but have not started yet. On resume, the approved/rejected
//! result closes the pending exchange before queued calls continue.

use super::tool_failure_guard::semantic_tool_call_fingerprint;
#[cfg(test)]
use super::tool_flow::build_tool_observation_message;
#[cfg(test)]
use crate::context::ContextCapacityDetector;
use crate::context::{
    ContextFrame, ContextGroup, ContextOrigin, ContextSource, ModelToolResultGate,
};
use crate::conversation_trace::{
    canonical_tool_result_for_context, ConversationHistoryArchiveTraceMetadata,
    ConversationTraceRecorder, ConversationTurnTraceItem,
};
use crate::file_change::{
    FileChangePathPolicy, FileObservationCheckpoint, FileObservationRegistry, FileObservationState,
};
use crate::llm::{
    validate_model_tool_call_id, validate_provider_tool_call_id, LlmAssistantTurn,
    LlmRuntimeToolCallBinding, LlmToolCall,
};
use crate::protocol::{
    AgentApprovalStatus, AgentAssistantTurnCheckpointIdentity, AgentContextCheckpointItem,
    AgentContextCheckpointToolCall, AgentError, AgentExtensionSnapshot,
    AgentProviderToolCallIdentity, AgentQueuedToolCallCheckpoint, AgentResult, AgentRunCheckpoint,
    AgentRunContext, AgentRunToolSetCheckpoint, AgentToolContinuation, AgentToolIdentity,
    AgentWritePermission, ModelCapabilities, AGENT_RUN_CHECKPOINT_SCHEMA_VERSION,
};
use crate::provider_profile::{ProviderProfileConfig, ProviderProtocolKey};
use crate::resolve_provider_runtime_capabilities;
use crate::tools::{
    validate_tool_set_checkpoint_shape, AgentToolCallCheckpointPersistence, EffectiveToolSet,
    ToolExecutionContext,
};
use crate::world_state::{
    WorldStateLifetime, WorldStateSectionId, WorldStateSnapshot, WorldStateVisibility,
};
use crate::AGENT_COLLABORATION_TOOL_NAMES;
use std::collections::{BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;

include!("checkpoint/model.rs");
include!("checkpoint/file_observations.rs");
include!("checkpoint/restore.rs");
include!("checkpoint/validation.rs");

#[cfg(test)]
mod tests;
