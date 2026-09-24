//! Append-only, provider-neutral records of agent activity.
//!
//! The active tool loop, approval checkpoint, durable audit trace, uncompressed bounded model log,
//! Exact History Archive, and presentation timeline are deliberately separate views of one
//! execution. The recorder produces both the lossless-within-runtime-budget model projection and
//! the lossy audit projection from one sequence domain, so persistence and reconstruction can
//! prove that they describe the same activity without conflating their retention policies.

use crate::conversation_trace_projection::{
    is_repeat_failure_eligible, project_attachment_text, project_narration, project_terminal_error,
    project_tool_call, project_tool_result, project_user_guidance, sanitize_runtime_text,
    sanitize_runtime_tool_result, sanitize_runtime_value,
};
use crate::llm::{validate_provider_tool_call_id, LlmMessage};
use crate::protocol::{
    AgentApprovalStatus, AgentCommandSessionStatus, AgentContextCheckpointToolCall,
    AgentContextCompactionEventOutcome, AgentInputAttachment, AgentInputAttachmentKind,
    AgentMcpServerScope, AgentProposedAction, AgentProviderToolCallIdentity, AgentRunCheckpoint,
    AgentToolCall, AgentToolIdentity, AgentToolResult,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

include!("conversation_trace/model.rs");
include!("conversation_trace/validation.rs");
include!("conversation_trace/recovery.rs");
include!("conversation_trace/recorder.rs");
include!("conversation_trace/projection.rs");

#[cfg(test)]
mod tests;
