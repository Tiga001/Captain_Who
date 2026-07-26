use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

use crate::command::AgentCommandExecutionResult;
use crate::storage::models::{
    AgentActionAuditRecord, AgentFileDraftChunkRecord, AgentFileDraftOperationRecord,
    AgentFileDraftRecord, AgentPendingActionRecord, AgentPromptPreferencesRecord,
    AgentRunGuidanceRecord, AgentUnsettledFileEffect, AgentUsageRecordInsert,
    AttachmentImageRecord, AttachmentRecord, ChatConversationMetaRecord, ChatConversationRecord,
    ChatConversationViewRecord, ChatMessageAttachmentRecord, ChatMessageRecord,
    ChatMessageStateRecord, ChatSearchInput, ChatSearchResult, ComposerDraftRecord,
    ForkConversationInput, ImageGenerationProfileRecord, ModelSettingsRecord, ProjectRecord,
    UiPreferencesRecord,
};
use crate::storage::{
    agent_action_audit_repository, agent_prompt_preferences_repository, attachment_repository,
    chat_repository, chat_search_repository, composer_draft_repository, config_repository,
    context_compaction_audit_repository, context_compaction_receipt_repository,
    context_compaction_repository, conversation_fork_repository, conversation_history_repository,
    conversation_trace_repository, file_draft_repository, guidance_repository,
    image_generation_repository, model_request_observation_repository, now_ms,
    pending_action_repository, preferences_repository, project_repository,
    skill_enablement_repository, storage_error, turn_diff_repository, usage_repository,
    world_state_repository, StorageState,
};
use crate::{
    AgentAttachmentLibraryContext, AgentAttachmentReference, AgentChatInput, AgentInputAttachment,
    AgentInputAttachmentEncoding, AgentInputAttachmentKind, AgentProposedAction, AgentToolCall,
    AgentToolResult, AgentTurnDiffIdentity, AgentTurnDiffRecord, AgentTurnFileChange,
    AgentUsageClearInput, AgentUsageClearOutput, AgentUsageSummaryInput, AgentUsageSummaryOutput,
    ContextCompactionAuditBundle, ContextCompactionPrefix, ContextCompactionReceipt,
    ContextCompactionSummary, ContextCompactionSummaryDraft, ContextJournalCursor,
    ConversationTurnTrace, ConversationTurnTraceItem, ModelRequestObservation, WorldStateRecord,
};
use base64::Engine;
use rusqlite::OptionalExtension;
use uuid::Uuid;

mod attachments;
mod compaction;
mod conversations;
mod file_drafts;
mod guidance;
mod image_generation;
mod lifecycle;
mod messages;
mod pending_actions;
mod settings;
mod trace_reconciliation;
mod turn_diffs;
mod world_state;

use attachments::*;
pub use guidance::{AgentRunGuidanceStoreOutcome, AgentRunGuidanceTransitionOutcome};
pub use lifecycle::*;
pub use pending_actions::{
    AgentPendingActionResultCommitOutcome, AgentPendingActionSettlementInspection,
};
#[cfg(test)]
use settings::MAX_SKILL_ENABLEMENT_ID_BYTES;

#[cfg(test)]
mod tests;
