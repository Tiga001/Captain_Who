use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::command::{AgentCommandExecutionResult, ManagedCommandWorkspaceRegistry};
use crate::storage::models::{
    AgentActionAuditRecord, AgentFileChangeChunkRecord, AgentFileChangeOperationRecord,
    AgentFileChangeRecord, AgentPendingActionRecord, AgentPromptPreferencesRecord,
    AgentRunGuidanceRecord, AgentUnsettledFileEffect, AgentUsageRecordInsert,
    AttachmentImageRecord, AttachmentRecord, ChatConversationMetaRecord, ChatConversationRecord,
    ChatConversationViewRecord, ChatMessageAttachmentRecord, ChatMessageRecord,
    ChatMessageStateRecord, ChatSearchInput, ChatSearchResult, ComposerDraftRecord,
    ConversationForkPoint, ForkConversationRequest, ImageGenerationProfileRecord,
    McpApprovalEnvelopeRecord, ModelConfigRecord, ModelSettingsRecord, ModelSettingsSaveError,
    ModelSettingsSaveRequest, ModelSettingsSnapshot, ProjectRecord, ProviderProfileUpdate,
    UiPreferencesRecord,
};
use crate::storage::{
    agent_action_audit_repository, agent_collaboration_event_repository,
    agent_command_session_repository, agent_delivery_repository, agent_graph_repository,
    agent_prompt_preferences_repository, agent_template_repository, attachment_repository,
    automation_repository, chat_repository, chat_search_repository, composer_draft_repository,
    config_repository, context_compaction_receipt_repository, context_compaction_repository,
    conversation_context_adaptation_repository, conversation_fork_repository,
    conversation_history_archive_repository, conversation_history_repository,
    conversation_model_context_repository, conversation_trace_repository,
    conversation_turn_rewrite_repository, file_change_repository, file_change_run_grant_repository,
    guidance_repository, image_generation_repository, mcp_approval_envelope_repository,
    model_request_observation_repository, notification_repository, now_ms,
    pending_action_repository, preferences_repository, project_repository,
    provider_continuation_repository, provider_transition_repository, skill_enablement_repository,
    storage_error, turn_diff_repository, usage_repository, world_state_repository, StorageState,
};
use crate::{
    AgentAttachmentLibraryContext, AgentAttachmentReference, AgentInputAttachment,
    AgentInputAttachmentEncoding, AgentInputAttachmentKind, AgentProposedAction, AgentToolCall,
    AgentToolResult, AgentTurnDiffIdentity, AgentTurnDiffRecord, AgentTurnFileChange,
    AgentUsageClearInput, AgentUsageClearOutput, AgentUsageSummaryInput, AgentUsageSummaryOutput,
    ContextCompactionPrefix, ContextCompactionReceipt, ContextCompactionSummary,
    ContextCompactionSummaryDraft, ContextJournalCursor, ConversationModelContextItem,
    ConversationModelContextLog, ConversationTurnTrace, ConversationTurnTraceItem,
    ModelRequestObservation, ProviderContinuationVault, WorldStateRecord,
};
use base64::Engine;
use rusqlite::{config::DbConfig, OptionalExtension};
use uuid::Uuid;

mod agent_collaboration_events;
mod agent_delivery;
mod agent_graph;
mod agent_templates;
mod attachments;
mod automations;
mod browser_data;
mod browser_downloads;
mod child_agents;
mod command_sessions;
mod compaction;
mod conversations;
mod file_change_run_grants;
mod file_changes;
mod guidance;
mod image_generation;
mod lifecycle;
mod managed_artifacts;
mod managed_command_workspaces;
mod manual_compaction;
mod mcp_approval_envelopes;
mod messages;
mod notifications;
mod pending_actions;
mod provider_continuations;
mod provider_transitions;
mod settings;
mod trace_reconciliation;
mod turn_diffs;
mod world_state;

use attachments::*;
pub use command_sessions::AgentCommandSessionLifecycleAppendOutcome;
pub use file_change_run_grants::FileChangeRunGrantServiceError;
pub use guidance::{AgentRunGuidanceStoreOutcome, AgentRunGuidanceTransitionOutcome};
pub use image_generation::{ResolvedGeneratedArtifactInput, ResolvedGeneratedArtifactKind};
pub use lifecycle::*;
pub use managed_artifacts::{
    AuthorizedManagedArtifactContent, ManagedArtifactAuthority, PublishedManagedArtifact,
    MAX_MANAGED_DOCUMENT_ARTIFACT_BYTES,
};
pub use messages::{
    AgentWaitingForApprovalPersistenceOutcome, AgentWaitingSegmentUsagePersistenceOutcome,
};
pub(crate) use pending_actions::manual_file_effect_has_authoritative_settlement;
pub use pending_actions::{
    AgentPendingActionJsonCommitOutcome, AgentPendingActionResultCommitOutcome,
    AgentPendingActionSettlementInspection, AgentWakeApprovalWaitOutcome,
    McpActionTerminalizationRequest, McpAutoActionJournalTerminalOutcome,
    McpStartupActionTerminalOutcome,
};
pub use settings::ModelProviderCredentialReconciliationReport;
#[cfg(test)]
use settings::MAX_SKILL_ENABLEMENT_ID_BYTES;

/// A renderer-facing Conversation and every durable actor origin captured from the same
/// SQLite read transaction. Keeping the provenance map beside the Conversation prevents
/// observer callers from issuing one independently-timed query per input message.
#[derive(Debug, Clone)]
pub struct ConversationObserverSnapshot {
    pub conversation: ChatConversationRecord,
    pub input_origins: BTreeMap<String, crate::ConversationMessageOrigin>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConversationTurnRewriteBeginOutcome {
    Started,
    Replayed(Box<conversation_turn_rewrite_repository::ConversationTurnRewriteRecord>),
}

/// Files are published before SQLite admission and their rows commit with the replacement Turn.
/// An uncommitted crash leaves only unreferenced files for the existing orphan scanner.
#[derive(Debug)]
pub struct PreparedConversationTurnRewriteAttachments {
    records: Vec<AttachmentRecord>,
    paths_created_by_this_process: Vec<PathBuf>,
}

#[cfg(test)]
mod tests;
