// Rust agent core.
mod cancellation;
pub mod command;
pub mod file_write;
mod llm;
pub mod patch;
mod prompts;
pub mod protocol;
mod revision;
mod runtime;
pub mod storage;
mod system_paths;
mod tools;
mod usage;

pub use cancellation::AgentCancellationToken;
pub use protocol::{
    AgentApiStyle, AgentApprovalDecision, AgentApprovalDecisionStatus, AgentApprovalStatus,
    AgentAttachmentLibraryContext, AgentAttachmentReference, AgentChatInput, AgentChatMessage,
    AgentChatOutput, AgentCommandOutputStream, AgentCommandPermission, AgentCommandRequest,
    AgentCommandRiskLevel, AgentDiffProposal, AgentError, AgentEvent, AgentFileDraftSnapshot,
    AgentFileDraftStatus, AgentFileWriteMode, AgentFileWriteProposal, AgentFileWriteResult,
    AgentFileWriteResultStatus, AgentGitDiffSnapshot, AgentInputAttachment,
    AgentInputAttachmentEncoding, AgentInputAttachmentKind, AgentPatchOperation,
    AgentPatchPermission, AgentPatchResult, AgentPatchResultStatus, AgentPermissions,
    AgentPromptDetailLevel, AgentPromptPreferences, AgentPromptTone, AgentPromptWorkMode,
    AgentProposedAction, AgentReadPermission, AgentResult, AgentRunContext, AgentRunStatus,
    AgentSearchConfig, AgentSearchMode, AgentStateSnapshot, AgentToolApprovalMode, AgentToolCall,
    AgentToolContinuation, AgentToolDefinition, AgentToolResult, AgentToolSafety, AgentUsage,
    AgentUsageClearInput, AgentUsageClearOutput, AgentUsageModelSummary, AgentUsageSummaryInput,
    AgentUsageSummaryOutput, AgentUsageSummaryRange, AgentWorkspaceContext, AgentWritePermission,
};
pub use revision::content_revision;
pub use runtime::{
    next_run_id, send_chat, send_chat_with_events, send_chat_with_events_and_cancellation,
    send_chat_with_host_executor, AgentEventEmitter, AgentHostActionExecutor, AgentRuntime,
};
pub use system_paths::{expand_system_path, system_path_aliases};
