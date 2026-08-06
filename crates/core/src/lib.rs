pub mod artifact_runtime;
mod cancellation;
pub mod command;
mod context;
mod context_compaction_receipt;
mod conversation_trace;
mod conversation_trace_projection;
pub mod durable_fs;
pub mod exact_capture;
pub mod file_input;
pub mod file_write;
pub mod git_review;
mod goal;
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
pub mod world_state;

pub use cancellation::AgentCancellationToken;
pub use context::{
    AgentContextBaseline, AgentContextWindowToolProjection, AgentConversationContextState,
    ContextCompactionGeneration, ContextCompactionGenerationKind, ContextCompactionPrefix,
    ContextCompactionSourceItem, ContextCompactionSummary, ContextCompactionSummaryDraft,
    ContextContinuityEntry, ContextContinuitySnapshot, ContextContinuityText, ContextHistoryRef,
    ContextJournalCursor, ContinuityIndexV2, CONTEXT_COMPACTION_SUMMARY_SCHEMA_VERSION,
    CONTEXT_CONTINUITY_HARD_MAX_TOKENS, CONTEXT_CONTINUITY_SCHEMA_VERSION,
    CONTEXT_CONTINUITY_TARGET_TOKENS, CONTEXT_CONTINUITY_V1_SCHEMA_VERSION,
};
pub use context_compaction_receipt::{
    ContextCompactionReceipt, ContextCompactionReceiptError, ContextCompactionReceiptPlan,
    ContextCompactionReceiptResult, ContextCompactionReceiptStage, ContextCompactionReceiptStatus,
    CONTEXT_COMPACTION_RECEIPT_SCHEMA_VERSION,
};
pub use conversation_trace::{
    cancelled_conversation_trace_from_checkpoint, cancelled_conversation_trace_from_snapshot,
    cancelled_conversation_trace_without_items, completed_conversation_trace_without_items,
    conversation_trace_snapshot_from_checkpoint_and_continuation_with_projection,
    conversation_trace_with_recovered_tool_result, failed_conversation_trace_without_items,
    terminalize_interrupted_conversation_trace, ConversationCommandSessionLifecycle,
    ConversationCommandSessionLifecyclePhase, ConversationHistoryArchiveTraceMetadata,
    ConversationModelContextItem, ConversationModelContextLog, ConversationTraceAttachment,
    ConversationTraceSnapshot, ConversationTraceToolResultStatus, ConversationTurnTrace,
    ConversationTurnTraceItem, ConversationTurnTraceTerminalStatus,
    CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
};
pub use goal::{
    fold_conversation_goal_revisions, ConversationGoal, ConversationGoalMutationActor,
    ConversationGoalRevision, ConversationGoalRevisionEvent, ConversationGoalStatus,
    CONVERSATION_GOAL_CONTEXT_HARD_MAX_TOKENS, CONVERSATION_GOAL_CONTEXT_TARGET_TOKENS,
    CONVERSATION_GOAL_REVISION_SCHEMA_VERSION, MAX_CONVERSATION_GOAL_OBJECTIVE_CHARS,
};
pub use model_request_observation::{
    ModelRequestActualUsage, ModelRequestCapacityStatus, ModelRequestEstimate,
    ModelRequestMeasurementMode, ModelRequestObservation, ModelRequestObservationStatus,
    ModelRequestPurpose, ModelRequestToolSetObservation, ModelRequestUsageNormalization,
    ProviderCacheTopology, MODEL_REQUEST_OBSERVATION_SCHEMA_VERSION,
};
pub use protocol::is_valid_agent_office_reason;

/// Rebuilds the model-only projection for a result restored from an approval checkpoint.
///
/// Core Server uses this when it must settle a Host continuation before resuming the model.
/// Keeping the tool-owned projection in Core avoids duplicating per-tool field contracts in the
/// Host boundary.
pub fn project_persisted_continuation_for_model(result: &AgentToolResult) -> AgentToolResult {
    tools::model_projection_for_persisted_continuation(result)
}

/// Rebuilds the security-sanitized, non-length-bounded exact-history projection for a result
/// restored from an approval checkpoint.
pub fn project_persisted_continuation_for_archive(result: &AgentToolResult) -> AgentToolResult {
    tools::archive_projection_for_persisted_continuation(result)
}

/// Distinguishes an unrecoverable source-side cut from an ordinary paginated Tool result.
pub fn tool_result_truncated_at_source(result: &AgentToolResult) -> bool {
    tools::tool_result_truncated_at_source(result)
}

/// Produces the exact bounded Tool-result text that an approval continuation may append to the
/// model timeline. Core Server calls this before its append-only trace/model-context commit, and
/// runtime restore calls the same gate again from the persisted Archive metadata.
pub fn project_persisted_continuation_observation(
    model: &str,
    api_url: &str,
    api_style: Option<protocol::AgentApiStyle>,
    result: &AgentToolResult,
    archive: &ConversationHistoryArchiveTraceMetadata,
) -> AgentResult<String> {
    let api_style = api_style.unwrap_or_else(|| llm::detect_api_style(api_url.trim()));
    let gate =
        context::ContextCapacityDetector::for_model(model, api_style, &[]).model_tool_result_gate();
    let projected = tools::model_projection_for_persisted_continuation(result);
    runtime::finalize_model_tool_observation(
        &gate,
        &result.call_id,
        !result.ok,
        &projected,
        archive,
    )
}

/// Reports whether the central 10K gate will length-truncate an approval continuation's semantic
/// model projection. Hosts use this before storing the immutable Archive descriptor so its
/// projection-loss metadata agrees with the model message committed immediately afterward.
pub fn persisted_continuation_model_projection_would_truncate(
    model: &str,
    api_url: &str,
    api_style: Option<protocol::AgentApiStyle>,
    result: &AgentToolResult,
) -> bool {
    let api_style = api_style.unwrap_or_else(|| llm::detect_api_style(api_url.trim()));
    let gate =
        context::ContextCapacityDetector::for_model(model, api_style, &[]).model_tool_result_gate();
    let projected = tools::model_projection_for_persisted_continuation(result);
    gate.would_truncate_with_source(
        &result.call_id,
        !result.ok,
        &projected,
        tools::tool_result_truncated_at_source(result),
    )
}

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
    AgentCommandArtifactValidation, AgentCommandArtifactValidationStatus, AgentCommandExitStatus,
    AgentCommandExpectedArtifactOutcome, AgentCommandExpectedArtifactOutcomeKind,
    AgentCommandOutputStream, AgentCommandPermission, AgentCommandRequest, AgentCommandRiskLevel,
    AgentCommandRuntimeBinding, AgentCommandRuntimeKind, AgentCommandRuntimePackageRequirement,
    AgentCommandRuntimeProfile, AgentCommandRuntimeProvider, AgentCommandRuntimeRequest,
    AgentCommandRuntimeResolution, AgentCommandRuntimeResolvedPackage, AgentCommandSafetyPolicy,
    AgentCommandSessionGetInput, AgentCommandSessionGetOutput, AgentCommandSessionListInput,
    AgentCommandSessionListOutput, AgentCommandSessionOutputChunk, AgentCommandSessionSnapshot,
    AgentCommandSessionStatus, AgentCommandSessionTranscript, AgentContextCheckpointGroup,
    AgentContextCheckpointImage, AgentContextCheckpointItem, AgentContextCheckpointOrigin,
    AgentContextCheckpointToolCall, AgentContextWindowSnapshot, AgentContextWindowStatus,
    AgentDiffProposal, AgentError, AgentEvent, AgentExtensionSnapshot, AgentFileDraftSnapshot,
    AgentFileDraftStatus, AgentFileInputBinding, AgentFileInputEvidence, AgentFileInputRef,
    AgentFileInputSourceKind, AgentFileInputSpec, AgentFileWriteMode, AgentFileWriteProposal,
    AgentFileWriteResult, AgentFileWriteResultStatus, AgentGitDiffSnapshot, AgentGuidanceStatus,
    AgentImageGenerationArtifact, AgentImageGenerationArtifactKind, AgentImageGenerationAudit,
    AgentImageGenerationFailure, AgentImageGenerationOperation, AgentImageGenerationResult,
    AgentImageGenerationResultStatus, AgentInputAttachment, AgentInputAttachmentEncoding,
    AgentInputAttachmentKind, AgentMcpApprovalMode, AgentMcpApprovalPayloadPersistence,
    AgentMcpArgumentSummary, AgentMcpDispatchCertainty, AgentMcpInvocationDiagnostics,
    AgentMcpInvocationFailureStage, AgentMcpResultSizeSummary, AgentMcpServerScope,
    AgentMcpToolApproval, AgentMcpToolApprovalSummary, AgentMcpToolInvocationEvent,
    AgentMcpToolInvocationIdentity, AgentMcpToolInvocationOutcome, AgentMcpToolInvocationState,
    AgentMcpToolProvenance, AgentMcpToolRisk, AgentOfficeOperationRequest, AgentPatchOperation,
    AgentPatchPermission, AgentPatchResult, AgentPatchResultStatus, AgentPermissions,
    AgentPromptDetailLevel, AgentPromptPreferences, AgentPromptTone, AgentPromptWorkMode,
    AgentProposedAction, AgentQueuedToolCallCheckpoint, AgentReadPermission, AgentResult,
    AgentRunCheckpoint, AgentRunContext, AgentRunStatus, AgentRunToolSetCheckpoint,
    AgentSearchConfig, AgentSearchMode, AgentSkillActivation, AgentSkillDependencyCheck,
    AgentSkillDependencyKind, AgentSkillDependencyStatus, AgentSkillInstallationPreview,
    AgentSkillInstallationRequest, AgentSkillInstallationResourceSummary,
    AgentSkillInstallationWarning, AgentSkillMaterializationRequest,
    AgentSkillMaterializationResult, AgentSkillMaterializationResultStatus,
    AgentSkillScriptInterpreter, AgentSkillScriptPreflightReport, AgentSkillScriptPreflightStatus,
    AgentSkillScriptRequest, AgentSkillScriptRequirements, AgentSkillScriptResult,
    AgentStateSnapshot, AgentSteerInput, AgentSteerRunInput, AgentSteerRunOutput,
    AgentSteerRunRejectionCode, AgentSteerRunResultStatus, AgentToolApprovalMode, AgentToolCall,
    AgentToolContinuation, AgentToolDefinition, AgentToolIdentity, AgentToolResult,
    AgentToolSafety, AgentUsage, AgentUsageClearInput, AgentUsageClearOutput,
    AgentUsageModelSummary, AgentUsageSummaryInput, AgentUsageSummaryOutput,
    AgentUsageSummaryRange, AgentWorkspaceContext, AgentWritePermission, ModelCapabilities,
    AGENT_COMMAND_ARTIFACT_OBSERVATION_SCHEMA_VERSION,
    AGENT_COMMAND_RUNTIME_BINDING_SCHEMA_VERSION, AGENT_COMMAND_RUNTIME_RESOLUTION_SCHEMA_VERSION,
    AGENT_FILE_INPUT_BINDING_SCHEMA_VERSION, AGENT_IMAGE_GENERATION_RESULT_SCHEMA_VERSION,
    AGENT_OFFICE_OPERATION_SCHEMA_VERSION, AGENT_OFFICE_REASON_MAX_CHARS,
    AGENT_RUN_CHECKPOINT_SCHEMA_VERSION, AGENT_SKILL_INSTALLATION_SCHEMA_VERSION,
};
pub use revision::content_revision;
pub use runtime::{
    conversation_context_configuration_revision, create_conversation_context_state,
    inspect_context_window, inspect_context_window_with_tool_projection, next_run_id,
    prepare_context_window_tool_projection, redact_terminal_skill_discovery, send_chat,
    send_chat_with_events, send_chat_with_events_and_cancellation, send_chat_with_host_executor,
    send_chat_with_host_services, skill_checkpoint_authority,
    skill_resource_selections_from_checkpoint, AgentCommandSessionAction,
    AgentCommandSessionExecutionOutput, AgentCommandSessionExecutionRequest,
    AgentCommandSessionExecutor, AgentContextCompactionCommitOutcome,
    AgentContextCompactionCommitRequest, AgentContextCompactionGenerationOutput,
    AgentContextCompactionGenerationRequest, AgentContextCompactionModelGenerator,
    AgentContextCompactionPrepareOutcome, AgentContextCompactionPrepareRequest,
    AgentContextCompactionServices, AgentContextWindowObserver, AgentConversationTraceObserver,
    AgentEventEmitter, AgentHostActionExecutor, AgentModelRequestObserver,
    AgentResolvedSkillActivation, AgentRuntime, AgentRuntimeHostServices,
    AgentSkillActivationResolver, AgentSkillCheckpointAuthority, AgentSteerEnqueueOutcome,
    AgentSteerInputQueue, AGENT_COMMAND_SESSION_DEFAULT_WAIT_MS, AGENT_COMMAND_SESSION_MAX_WAIT_MS,
    AGENT_COMMAND_SESSION_MODEL_OUTPUT_BYTES,
};
pub use system_paths::expand_system_path;
pub use tools::{
    agent_image_generation_execution_id, agent_image_generation_tool_result_from_execution,
    agent_image_generation_tool_result_from_service_error, mcp_normalized_input_schema_identity,
    mcp_tool_arguments_digest, mcp_tool_invocation_event, mcp_tool_result_from_approved_invocation,
    mcp_tool_result_from_rejected_approval, mcp_tool_result_model_projection,
    mcp_tool_result_persistence_projection, mcp_tool_result_size_summary,
    normalize_agent_image_generation_reason, validate_mcp_approval_arguments,
    AgentSkillInstallationCommitPreparationRequest, AgentSkillInstallationCommitPreparer,
    AgentSkillInstallationPrepareExecutor, AgentSkillInstallationPrepareRequest,
    AgentSkillInstallationPrepareSource, McpAgentToolAnnotations, McpAgentToolDescriptor,
    McpApprovedToolInvocation, McpNormalizedInputSchemaIdentity, McpOmittedContentKind,
    McpRuntimeProjectionLimits, McpToolApprovalRequest, McpToolCatalogContext, McpToolContentBlock,
    McpToolDiagnosticCode, McpToolInvocationEventUpdate, McpToolInvocationFuture,
    McpToolInvocationResult, McpToolInvoker, McpToolRegistrationDiagnostic, McpToolRuntime,
    MCP_INPUT_SCHEMA_NORMALIZER_VERSION, MCP_RUNTIME_MAX_CATALOG_BYTES,
    MCP_RUNTIME_MAX_TOOL_DEFINITIONS,
};
pub use turn_diff::{
    capture_agent_turn_file_content, AgentTurnDiffIdentity, AgentTurnDiffRecord,
    AgentTurnFileChange, AgentTurnFileContent, AGENT_TURN_DIFF_SCHEMA_VERSION,
    MAX_AGENT_TURN_FILE_CONTENT_BYTES,
};
pub use world_state::{
    AnchoredWorldStateRecord, WorldStateDiff, WorldStateError, WorldStateLifetime,
    WorldStateModelChange, WorldStateModelRecord, WorldStateModelSection, WorldStateOperation,
    WorldStateRecord, WorldStateRecordKind, WorldStateReducer, WorldStateSectionEnvelope,
    WorldStateSectionId, WorldStateSectionPrecondition, WorldStateSectionTombstone,
    WorldStateSnapshot, WorldStateVisibility, WORLD_STATE_REVISION_PREFIX,
    WORLD_STATE_SCHEMA_VERSION,
};
