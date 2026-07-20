// Support types and helper functions for core-server agent orchestration.
use crate::agent::{AGENT_EVENT_NAME, ID_COUNTER, THINKING_PLACEHOLDER};
use crate::skills_adapter::{activate_selected_skills, SkillActivationFailure};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::{SystemTime, UNIX_EPOCH};

use mycopilot_core::command::{AgentCommandExecutionResult, CommandPolicyEvaluation};
use mycopilot_core::file_write::{apply_file_write, failed_file_write_result};
use mycopilot_core::patch::apply_unified_diff_in_workspace;
use mycopilot_core::skills::SkillsService;
use mycopilot_core::storage::models::{
    AgentPromptPreferencesRecord, ChatConversationRecord, ChatMessageAttachmentRecord,
    ChatMessageRecord, ProjectRecord,
};
use mycopilot_core::storage::service::StorageService;
use mycopilot_core::{
    AgentApprovalDecisionStatus, AgentChatInput, AgentChatMessage, AgentChatOutput,
    AgentCommandRequest, AgentContextWindowSnapshot, AgentDiffProposal, AgentEvent,
    AgentFileDraftSnapshot, AgentFileWriteProposal, AgentFileWriteResult,
    AgentFileWriteResultStatus, AgentInputAttachment, AgentInputAttachmentEncoding,
    AgentInputAttachmentKind, AgentPatchResult, AgentPatchResultStatus, AgentPermissions,
    AgentPromptDetailLevel, AgentPromptPreferences, AgentPromptTone, AgentPromptWorkMode,
    AgentProposedAction, AgentRunContext, AgentRunStatus, AgentSearchConfig, AgentSearchMode,
    AgentSkillMaterializationRequest, AgentSkillScriptRequest, AgentToolCall, AgentToolResult,
    AgentUsage, AgentWorkspaceContext, ContextCompactionAuditBundle, ContextJournalCursor,
    ConversationTurnTrace, ConversationTurnTraceTerminalStatus,
};
use mycopilot_protocol_rs::{
    ActivatedSkillSummaryDto, SkillActivationErrorData, SkillSelectionDto,
    AGENT_EVENT_NOTIFICATION_METHOD,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

mod actions;
mod conversation;
mod helpers;
mod types;
mod utility;

pub(super) use actions::*;
pub(super) use conversation::*;
pub(super) use helpers::*;
pub(super) use types::{
    agent_input_belongs_to_project, pending_status_label, ActionExecutionDecision,
    AgentRunUsageContext, AgentRunUsageState, PendingActionRecord,
};
pub use types::{
    AgentActionExecutionOutput, AgentContextCompactionAuditInput,
    AgentContextCompactionAuditOutput, AgentContextWindowSnapshotInput,
    AgentContextWindowSnapshotOutput, AgentConversationTurnInput, AgentConversationTurnOutput,
    AgentFileDraftContentPage, AgentFileWriteDiffPage, AgentServiceError, PendingActionStatus,
    PendingAgentActionSnapshot,
};
pub(super) use utility::*;

#[cfg(test)]
mod tests;
