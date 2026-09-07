use crate::context::{
    ContextCompactionSummary, ContextContinuitySnapshot, ContextHistoryRef, ContextJournalCursor,
};
use crate::storage::models::{
    AgentActionAuditRecord, AgentFileChangeRecord, AgentRunGuidanceRecord, AttachmentRecord,
    ChatConversationRecord, ChatMessageRecord, ConversationContinuationOriginRecord,
    ConversationForkPoint,
};
use crate::storage::{
    agent_action_audit_repository, agent_graph_repository, attachment_repository, chat_repository,
    context_compaction_receipt_repository, context_compaction_repository,
    conversation_context_adaptation_repository, conversation_history_archive_repository,
    conversation_history_open, conversation_model_context_repository,
    conversation_trace_repository, conversation_turn_rewrite_repository, file_change_repository,
    guidance_repository, model_request_observation_repository, provider_continuation_repository,
    turn_diff_repository, world_state_repository,
};
use crate::{
    provider_continuation_store::{
        PreparedProviderContinuationClone, ProviderContinuationForkMapping,
    },
    root_agent_creation_request_id, root_agent_id_for_conversation, AgentFileChangeResult,
    AgentGuidanceStatus, AgentLifecycle, AgentNodeRecord, AgentProposedAction, AgentToolResult,
    ContextCompactionReceipt, ContextCompactionReceiptStage, ContextCompactionReceiptStatus,
    ConversationMessageOrigin, ConversationModelContextItem, ConversationTurnTrace,
    EnsureRootAgentInput, ModelRequestObservation, ProviderContinuationRef, WorldStateDiff,
    WorldStateRecord, WorldStateReducer, WorldStateSectionEnvelope, WorldStateSnapshot,
    ROOT_AGENT_TASK_NAME,
};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use uuid::Uuid;

include!("conversation_fork_repository/model.rs");
include!("conversation_fork_repository/build_plan.rs");
include!("conversation_fork_repository/file_changes.rs");
include!("conversation_fork_repository/commit.rs");
include!("conversation_fork_repository/resolve_visibility.rs");
include!("conversation_fork_repository/write_history.rs");
include!("conversation_fork_repository/clone_context.rs");

#[cfg(test)]
mod policy;

#[cfg(test)]
mod tests;
