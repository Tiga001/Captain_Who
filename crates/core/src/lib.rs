pub mod artifact_runtime;
mod cancellation;
pub mod command;
mod context;
mod context_compaction_audit;
mod context_compaction_receipt;
mod conversation_trace;
pub mod durable_fs;
pub mod file_input;
pub mod file_write;
pub mod git_review;
pub mod image_generation;
mod llm;
mod model_request_observation;
pub mod office;
pub mod patch;
mod prompts;
pub mod protocol;
mod revision;
mod runtime;
pub mod skills;
pub mod storage;
mod system_paths;
mod tools;
mod turn_diff;
mod usage;

pub use cancellation::AgentCancellationToken;
pub use context::{
    AgentContextBaseline, AgentConversationContextState, ContextCompactionGeneration,
    ContextCompactionGenerationKind, ContextCompactionPrefix, ContextCompactionSourceItem,
    ContextCompactionSummary, ContextCompactionSummaryDraft, ContextContinuityEntry,
    ContextContinuitySnapshot, ContextContinuityText, ContextJournalCursor,
    CONTEXT_COMPACTION_SUMMARY_SCHEMA_VERSION, CONTEXT_CONTINUITY_SCHEMA_VERSION,
};
pub use context_compaction_audit::{
    ContextCompactionAuditBundle, ContextCompactionAuditCheck, ContextCompactionAuditCheckStatus,
    ContextCompactionAuditReport, ContextCompactionAuditVerdict, ContextCompactionSummaryEvidence,
    ContextCompactionSummaryRelation, ModelRequestEstimationErrorGroup,
};
pub use context_compaction_receipt::{
    ContextCompactionReceipt, ContextCompactionReceiptError, ContextCompactionReceiptPlan,
    ContextCompactionReceiptResult, ContextCompactionReceiptStage, ContextCompactionReceiptStatus,
    CONTEXT_COMPACTION_RECEIPT_SCHEMA_VERSION,
};
pub use conversation_trace::{
    cancelled_conversation_trace_from_checkpoint, cancelled_conversation_trace_from_snapshot,
    cancelled_conversation_trace_without_items, completed_conversation_trace_without_items,
    conversation_trace_snapshot_from_checkpoint_and_continuation,
    conversation_trace_with_recovered_tool_result, failed_conversation_trace_without_items,
    ConversationTraceAttachment, ConversationTraceSnapshot, ConversationTraceToolResultStatus,
    ConversationTurnTrace, ConversationTurnTraceItem, ConversationTurnTraceTerminalStatus,
    CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
};
pub use model_request_observation::{
    ModelRequestActualUsage, ModelRequestCapacityStatus, ModelRequestEstimate,
    ModelRequestMeasurementMode, ModelRequestObservation, ModelRequestObservationStatus,
    ModelRequestPurpose, ModelRequestUsageNormalization, MODEL_REQUEST_OBSERVATION_SCHEMA_VERSION,
};
pub use protocol::is_valid_agent_office_reason;

/// Re-parse and recompile a frozen Office semantic request, proving that it
/// still matches the canonical request authorized by the Host.
pub fn validate_frozen_agent_office_semantic_args(
    frozen: &AgentOfficeOperationRequest,
) -> Result<(), String> {
    tools::validate_frozen_office_trace_args(frozen, &frozen.semantic_args)
}

/// Re-parse the model-authored command ToolCall arguments and prove that they still describe the
/// exact host-prepared action. Host-derived runtime and observation fields are reconstructed
/// internally; callers must pass the original model-visible argument object.
pub fn validate_frozen_agent_command_args(
    frozen: &AgentCommandRequest,
    operation: &serde_json::Value,
) -> Result<(), String> {
    tools::validate_frozen_command_trace_args(frozen, operation)
}

pub use protocol::{
    AgentActivatedSkill, AgentActivatedSkillResources, AgentApiStyle, AgentApprovalDecision,
    AgentApprovalDecisionStatus, AgentApprovalStatus, AgentAttachmentLibraryContext,
    AgentAttachmentReference, AgentChatInput, AgentChatMessage, AgentChatOutput,
    AgentCommandArtifactChange, AgentCommandArtifactChangeKind, AgentCommandArtifactKind,
    AgentCommandArtifactMetadata, AgentCommandArtifactObservation,
    AgentCommandArtifactObservationCoverage, AgentCommandArtifactObservationKind,
    AgentCommandArtifactObservationPhase, AgentCommandArtifactObservationRequest,
    AgentCommandArtifactObservationStatus, AgentCommandArtifactObservationWarning,
    AgentCommandArtifactScope, AgentCommandArtifactSnapshotCoverage,
    AgentCommandArtifactValidation, AgentCommandArtifactValidationStatus,
    AgentCommandExpectedArtifactOutcome, AgentCommandExpectedArtifactOutcomeKind,
    AgentCommandOutputStream, AgentCommandPermission, AgentCommandRequest, AgentCommandRiskLevel,
    AgentCommandRuntimeBinding, AgentCommandRuntimeKind, AgentCommandRuntimePackageRequirement,
    AgentCommandRuntimeProfile, AgentCommandRuntimeProvider, AgentCommandRuntimeRequest,
    AgentCommandRuntimeResolution, AgentCommandRuntimeResolvedPackage, AgentCommandSafetyPolicy,
    AgentContextCheckpointGroup, AgentContextCheckpointImage, AgentContextCheckpointItem,
    AgentContextCheckpointOrigin, AgentContextCheckpointToolCall, AgentContextWindowPhase,
    AgentContextWindowSnapshot, AgentContextWindowStatus, AgentDiffProposal, AgentError,
    AgentEvent, AgentExtensionSnapshot, AgentFileDraftSnapshot, AgentFileDraftStatus,
    AgentFileInputBinding, AgentFileInputEvidence, AgentFileInputRef, AgentFileInputSourceKind,
    AgentFileInputSpec, AgentFileWriteMode, AgentFileWriteProposal, AgentFileWriteResult,
    AgentFileWriteResultStatus, AgentGitDiffSnapshot, AgentGuidanceStatus,
    AgentImageGenerationArtifact, AgentImageGenerationArtifactKind, AgentImageGenerationAudit,
    AgentImageGenerationFailure, AgentImageGenerationOperation, AgentImageGenerationResult,
    AgentImageGenerationResultStatus, AgentInputAttachment, AgentInputAttachmentEncoding,
    AgentInputAttachmentKind, AgentOfficeOperationRequest, AgentPatchOperation,
    AgentPatchPermission, AgentPatchResult, AgentPatchResultStatus, AgentPermissions,
    AgentPromptDetailLevel, AgentPromptPreferences, AgentPromptTone, AgentPromptWorkMode,
    AgentProposedAction, AgentQueuedToolCallCheckpoint, AgentReadPermission, AgentResult,
    AgentRunCheckpoint, AgentRunContext, AgentRunStatus, AgentSearchConfig, AgentSearchMode,
    AgentSkillActivation, AgentSkillDependencyCheck, AgentSkillDependencyKind,
    AgentSkillDependencyStatus, AgentSkillMaterializationRequest, AgentSkillMaterializationResult,
    AgentSkillMaterializationResultStatus, AgentSkillScriptInterpreter,
    AgentSkillScriptPreflightReport, AgentSkillScriptPreflightStatus, AgentSkillScriptRequest,
    AgentSkillScriptRequirements, AgentSkillScriptResult, AgentStateSnapshot, AgentSteerInput,
    AgentSteerRunInput, AgentSteerRunOutput, AgentSteerRunResultStatus, AgentToolApprovalMode,
    AgentToolCall, AgentToolContinuation, AgentToolDefinition, AgentToolResult, AgentToolSafety,
    AgentUsage, AgentUsageClearInput, AgentUsageClearOutput, AgentUsageModelSummary,
    AgentUsageSummaryInput, AgentUsageSummaryOutput, AgentUsageSummaryRange, AgentWorkspaceContext,
    AgentWritePermission, ModelCapabilities, AGENT_COMMAND_ARTIFACT_OBSERVATION_SCHEMA_VERSION,
    AGENT_COMMAND_RUNTIME_BINDING_SCHEMA_VERSION, AGENT_COMMAND_RUNTIME_RESOLUTION_SCHEMA_VERSION,
    AGENT_FILE_INPUT_BINDING_SCHEMA_VERSION, AGENT_IMAGE_GENERATION_RESULT_SCHEMA_VERSION,
    AGENT_OFFICE_OPERATION_SCHEMA_VERSION, AGENT_OFFICE_REASON_MAX_CHARS,
    AGENT_RUN_CHECKPOINT_SCHEMA_VERSION,
};
pub use revision::content_revision;
pub use runtime::{
    conversation_context_configuration_revision, create_conversation_context_state,
    inspect_context_window, next_run_id, redact_terminal_skill_discovery, send_chat,
    send_chat_with_events, send_chat_with_events_and_cancellation, send_chat_with_host_executor,
    send_chat_with_host_services, skill_checkpoint_authority,
    skill_resource_selections_from_checkpoint, AgentContextCompactionCommitOutcome,
    AgentContextCompactionCommitRequest, AgentContextCompactionGenerationOutput,
    AgentContextCompactionGenerationRequest, AgentContextCompactionModelGenerator,
    AgentContextCompactionPrepareOutcome, AgentContextCompactionPrepareRequest,
    AgentContextCompactionServices, AgentConversationTraceObserver, AgentEventEmitter,
    AgentHostActionExecutor, AgentModelRequestObserver, AgentResolvedSkillActivation, AgentRuntime,
    AgentRuntimeHostServices, AgentSkillActivationResolver, AgentSkillCheckpointAuthority,
    AgentSteerEnqueueOutcome, AgentSteerInputQueue,
};
pub use system_paths::expand_system_path;
pub use tools::{
    agent_image_generation_execution_id, agent_image_generation_tool_result_from_execution,
    agent_image_generation_tool_result_from_service_error, normalize_agent_image_generation_reason,
};
pub use turn_diff::{
    capture_agent_turn_file_content, AgentTurnDiffIdentity, AgentTurnDiffRecord,
    AgentTurnFileChange, AgentTurnFileContent, AGENT_TURN_DIFF_SCHEMA_VERSION,
    MAX_AGENT_TURN_FILE_CONTENT_BYTES,
};
