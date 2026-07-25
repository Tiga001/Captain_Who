import type { ActivatedSkillSummary, SkillSelection } from './skills'

export type AgentMessageRole = 'system' | 'user' | 'assistant'

export type AgentRunStatus =
  'idle' | 'queued' | 'running' | 'waiting_for_approval' | 'completed' | 'failed' | 'cancelled'

export type AgentApiStyle = 'openai_compatible' | 'anthropic_compatible'

export type AgentSearchMode = 'auto' | 'disabled' | 'tavily'

/**
 * Controls which local paths read-only tools may inspect.
 * - workspace_only: selected workspace and registered attachment paths only.
 * - all: workspace plus absolute local paths and supported aliases such as
 *   @home, @desktop, @documents, and @downloads.
 */
export type AgentReadPermission = 'workspace_only' | 'all'

/**
 * Controls where file-changing tools may write.
 * - denied: hide/disable file edit tools and reject patch execution.
 * - workspace_only: safe writes inside the selected workspace only.
 * - all: safe writes inside or outside the workspace, including absolute paths and supported aliases.
 */
export type AgentWritePermission = 'denied' | 'workspace_only' | 'all'

/**
 * Controls whether run_command requires a human click.
 * Approval and command safety are separate inputs. In guarded mode, auto_approve applies only to
 * commands the policy permits automatically; high-impact commands may still require explicit user
 * approval.
 */
export type AgentCommandPermission = 'require_approval' | 'auto_approve'

/**
 * Controls how broadly an authorized command may execute.
 * - guarded: auto-run low-risk commands and route high-impact commands to explicit approval.
 * - full_access: auto-run high-impact commands except operations that are always denied.
 *
 * This is independent from command approval: approval decides who authorizes a command, while
 * commandSafety decides which policy applies after authorization.
 */
export type AgentCommandSafetyPolicy = 'guarded' | 'full_access'

/**
 * Controls whether structured file-write proposals require a human click. This includes text
 * patches, transactional file writes, Skill resource materialization, and Office document writes.
 * auto_approve only skips the prompt; backend path, symlink, format, and revision checks still apply.
 */
export type AgentPatchPermission = 'require_approval' | 'auto_approve'

export interface AgentPermissions {
  read: AgentReadPermission
  write: AgentWritePermission
  command: AgentCommandPermission
  commandSafety: AgentCommandSafetyPolicy
  patch: AgentPatchPermission
}

export type AgentPromptWorkMode = 'coding' | 'general'

export type AgentPromptTone = 'friendly' | 'pragmatic'

export type AgentPromptDetailLevel = 'low' | 'medium' | 'high'

export type AgentToolName =
  | 'attachments_list'
  | 'attachments_list_project'
  | 'read_file'
  | 'read_image'
  | 'read_pdf'
  | 'read_word'
  | 'read_presentation'
  | 'read_spreadsheet'
  | 'workspace_map'
  | 'search_files'
  | 'search_code'
  | 'web_search'
  | 'web_fetch'
  | 'git_diff'
  | 'todo_update'
  | 'apply_patch'
  | 'write_file'
  | 'run_command'
  | 'skills_activate'
  | 'skills_list_resources'
  | 'skills_read_resource'
  | 'skills_materialize_resource'
  | 'skills_preflight_script'
  | 'skills_run_script'
  | 'office_document'
  | 'office_spreadsheet'
  | 'office_presentation'
  | 'image_generation'
  | (string & {})

export type AgentToolSafety = 'read_only' | 'requires_approval' | 'destructive'

export type AgentToolApprovalMode = 'never' | 'always' | 'dynamic'

export type AgentApprovalStatus = 'not_required' | 'required' | 'approved' | 'rejected'

export type AgentApprovalDecisionStatus = 'approved' | 'rejected'

export type ConversationTurnTraceTerminalStatus = 'completed' | 'failed' | 'cancelled'

export type ConversationTraceToolResultStatus =
  'succeeded' | 'failed' | 'rejected' | 'conflict' | 'cancelled'

export interface ConversationTraceAttachment {
  id: string
  kind: AgentInputAttachmentKind
  name: string
  mimeType?: string
  sizeBytes: number
}

export type ConversationTurnTraceItem =
  | {
      type: 'assistant_narration'
      sequence: number
      content: string
      truncated: boolean
    }
  | {
      type: 'user_guidance'
      sequence: number
      guidanceId: string
      clientMessageId: string
      content: string
      attachments: ConversationTraceAttachment[]
      createdAt: number
      truncated: boolean
    }
  | {
      type: 'tool_call'
      sequence: number
      callId: string
      tool: string
      operation: unknown
      approvalStatus: AgentApprovalStatus
      truncated: boolean
    }
  | {
      type: 'tool_result'
      sequence: number
      callId: string
      tool: string
      status: ConversationTraceToolResultStatus
      success: boolean
      observation: unknown
      approvalStatus: AgentApprovalStatus
      error?: string
      truncated: boolean
    }

export interface ConversationTurnTrace {
  schemaVersion: number
  runId: string
  conversationId: string
  assistantMessageId: string
  terminalStatus: ConversationTurnTraceTerminalStatus
  terminalError?: string
  truncated: boolean
  items: ConversationTurnTraceItem[]
}

export type AgentTodoStatus = 'pending' | 'in_progress' | 'completed' | 'blocked'

export type AgentPatchOperation = 'create' | 'update' | 'delete'

export type AgentPatchResultStatus = 'applied' | 'failed' | 'conflict' | 'rejected'

export type AgentFileWriteMode = 'create' | 'rewrite' | 'modify' | 'append' | 'upsert'

export type AgentFileDraftStatus =
  | 'drafting'
  | 'ready'
  | 'waiting_approval'
  | 'applying'
  | 'applied'
  | 'rejected'
  | 'conflict'
  | 'failed'
  | 'aborted'
  | 'expired'

export type AgentFileWriteResultStatus =
  'applied' | 'failed' | 'conflict' | 'rejected' | 'already_applied'

export type AgentCommandOutputStream = 'stdout' | 'stderr'

export type AgentCommandRiskLevel =
  'read_only' | 'writes_workspace' | 'network' | 'destructive' | 'unknown'

export type AgentCommandPolicyDecision = 'allow' | 'require_explicit_approval' | 'deny'

export type AgentCommandRiskClass =
  | 'read_only'
  | 'safe_workspace_write'
  | 'direct_write'
  | 'network'
  | 'package_management'
  | 'high_impact'
  | 'unknown'
  | 'catastrophic'
  | 'unsupported'
  | 'external_read'

export interface AgentCommandPolicyFinding {
  segmentIndex: number
  program: string
  risk: AgentCommandRiskClass
  /** Stable discriminator for UI routing and telemetry. */
  code: string
  /** Human-readable diagnostic; callers must not branch on this text. */
  reason: string
}

export interface AgentCommandPolicyEvaluation {
  decision: AgentCommandPolicyDecision
  /** Stable summary discriminator for UI routing and telemetry. */
  code: string
  /** Human-readable diagnostic; callers must not branch on this text. */
  reason: string
  riskLevel: AgentCommandRiskLevel
  findings: AgentCommandPolicyFinding[]
}

export interface AgentChatMessage {
  messageId?: string
  role: AgentMessageRole
  content: string
  conversationTurnTrace?: ConversationTurnTrace
}

export type AgentInputAttachmentKind = 'file' | 'image'

export type AgentInputAttachmentEncoding = 'utf8' | 'base64'

export interface AgentInputAttachment {
  id: string
  kind: AgentInputAttachmentKind
  name: string
  mimeType?: string
  sizeBytes: number
  encoding: AgentInputAttachmentEncoding
  data: string
  truncated?: boolean
}

export interface AgentConversationMessageAttachment {
  id: string
  kind: AgentInputAttachmentKind
  name: string
  mimeType?: string | null
  sizeBytes: number
  previewData?: string | null
  previewMimeType?: string | null
  createdAt?: number
}

export interface AgentWorkspaceContext {
  projectId?: string
  displayName?: string
  rootPath?: string
}

export interface AgentAttachmentReference {
  id: string
  conversationId: string
  messageId: string
  projectId?: string | null
  kind: AgentInputAttachmentKind
  name: string
  mimeType?: string
  sizeBytes: number
  readPath: string
  storageRelPath: string
  createdAt: number
}

export interface AgentAttachmentLibraryContext {
  rootPath?: string
  conversationId?: string
  projectId?: string | null
  conversationAttachments: AgentAttachmentReference[]
  projectAttachments: AgentAttachmentReference[]
}

export interface AgentPromptPreferences {
  workMode?: AgentPromptWorkMode
  tone?: AgentPromptTone
  detailLevel?: AgentPromptDetailLevel
  customInstructions?: string
  updatedAt?: number
}

export interface AgentRunContext {
  conversationId?: string
  projectId?: string | null
  workspace?: AgentWorkspaceContext
  attachmentLibrary?: AgentAttachmentLibraryContext
  permissions: AgentPermissions
}

export interface AgentSearchConfig {
  mode: AgentSearchMode
  tavilyApiKey?: string
}

export interface AgentApprovalDecision {
  actionId: string
  status: AgentApprovalDecisionStatus
  message?: string
}

export interface AgentUsage {
  inputTokens?: number
  outputTokens?: number
  outputThinkingTokens?: number
  totalTokens?: number
  cachedInputTokens?: number
  cacheCreationInputTokens?: number
  billableRequestCount?: number
}

export type AgentContextWindowStatus =
  'unconfigured' | 'within_budget' | 'over_budget' | 'invalid_configuration'

export type AgentContextWindowPhase = 'idle' | 'durable_commit'

export interface AgentContextWindowSnapshot {
  model: string
  status: AgentContextWindowStatus
  phase: AgentContextWindowPhase
  contextWindowTokens?: number
  reservedOutputTokens: number
  safetyMarginTokens: number
  /** Input capacity left for durable history after fixed request costs. */
  durableCapacityTokens?: number
  /** Conversation history and trace content retained for later turns. */
  durableInputTokens: number
  /** Current-run overlays, including activated Skill instructions. */
  runTransientInputTokens: number
  /** Total estimated input for the current preview request. */
  requestInputTokens: number
  remainingDurableTokens?: number
  /** Opaque fingerprint that changes with fixed or durable context. */
  persistentRevision: string
}

export type AgentUsageSummaryRange = 'last7Days' | 'last30Days' | 'all' | 'custom'

export interface AgentUsageSummaryInput {
  range: AgentUsageSummaryRange
  from?: number
  to?: number
}

export interface AgentUsageModelSummary {
  modelId: string
  modelName: string
  isConfigured: boolean
  requestCount: number
  messageCount: number
  unpricedMessageCount: number
  inputTokens?: number
  outputTokens?: number
  outputThinkingTokens?: number
  totalTokens?: number
  cachedInputTokens?: number
  cacheCreationInputTokens?: number
  estimatedCost?: number
}

export interface AgentUsageSummaryOutput {
  requestCount: number
  messageCount: number
  unpricedMessageCount: number
  inputTokens?: number
  outputTokens?: number
  outputThinkingTokens?: number
  totalTokens?: number
  cachedInputTokens?: number
  cacheCreationInputTokens?: number
  estimatedCost?: number
  models: AgentUsageModelSummary[]
}

export interface AgentUsageClearInput {
  from?: number
  to?: number
}

export interface AgentUsageClearOutput {
  deletedRecords: number
}

export interface AgentTodoItem {
  id: string
  title: string
  status: AgentTodoStatus
  note?: string
  createdAt: number
  updatedAt: number
}

export interface AgentTodoState {
  revision: number
  items: AgentTodoItem[]
  updatedAt: number
}

export type AgentActionExecutionStatus = 'applied' | 'approved' | 'failed' | 'conflict' | 'rejected'

export interface AgentCommandExecutionResult {
  command: string
  cwd: string
  exitCode?: number
  stdout: string
  stderr: string
  timedOut: boolean
  cancelled: boolean
  durationMs: number
  stdoutTruncated: boolean
  stderrTruncated: boolean
  error?: string
  /** Present when execution was stopped by the authoritative backend command policy. */
  policyEvaluation?: AgentCommandPolicyEvaluation
  /** Best-effort file effects observed around this exact command execution. */
  artifactObservation?: AgentCommandArtifactObservation
  /** Host-owned runtime resolution or structured preflight failure evidence. */
  runtime?: AgentCommandRuntimeResolution
}

export interface AgentActionExecutionOutput {
  actionId: string
  actionType: string
  toolName: string
  status: AgentActionExecutionStatus
  patchResult?: AgentPatchResult
  fileWriteResult?: AgentFileWriteResult
  commandResult?: AgentCommandExecutionResult
  toolResult?: AgentToolResult
  agentOutput: AgentChatOutput
}

export interface AgentChatOutput {
  status: AgentRunStatus
  content: string
  runId: string
  events: AgentEvent[]
  toolDefinitions: AgentToolDefinition[]
  todo?: AgentTodoState
  usage?: AgentUsage
  finishReason?: string
  proposedActions: AgentProposedAction[]
  conversationTurnTrace?: ConversationTurnTrace
}

export interface AgentConversationTurnInput {
  conversationId?: string
  projectId?: string | null
  modelId: string
  contextWindowIndicatorEnabled?: boolean
  content: string
  attachments?: AgentInputAttachment[]
  title?: string
  userMessageId?: string
  assistantMessageId?: string
  maxTokens?: number
  temperature?: number
  promptPreferences?: AgentPromptPreferences
  permissions?: AgentPermissions
  /** Ordered, revision-bound Skills selected for this agent run. */
  skills?: SkillSelection[]
}

export type AgentGuidanceStatus = 'queued' | 'applied' | 'rejected' | 'abandoned'

export interface AgentSteerRunInput {
  conversationId: string
  expectedRunId: string
  clientMessageId: string
  content: string
  attachments?: AgentInputAttachment[]
}

export type AgentSteerRunResultStatus = 'queued' | 'applied' | 'rejected'

export type AgentSteerRunRejectionCode =
  | 'run_not_steerable'
  | 'run_interrupted'
  | 'conversation_mismatch'
  | 'identity_conflict'
  | 'attachments_not_supported'
  | 'model_does_not_support_attachments'
  | 'attachment_validation_failed'
  | 'attachment_limit_exceeded'
  | 'attachment_persistence_failed'

export interface AgentSteerRunOutput {
  guidanceId: string
  status: AgentSteerRunResultStatus
  rejectionCode?: AgentSteerRunRejectionCode
  message?: string
}

export interface AgentContextWindowSnapshotInput {
  conversationId?: string
  projectId?: string | null
  modelId: string
  maxTokens?: number
  promptPreferences?: AgentPromptPreferences
  permissions?: AgentPermissions
  skills?: SkillSelection[]
}

export interface AgentContextWindowSnapshotOutput {
  snapshot?: AgentContextWindowSnapshot
}

export type AgentObservedApiStyle = 'open_ai_compatible' | 'anthropic_compatible'
export type ModelRequestPurpose = 'agent_loop' | 'context_compaction'
export type ModelRequestObservationStatus = 'completed' | 'failed' | 'cancelled'
export type ModelRequestMeasurementMode = 'incremental_cache' | 'full_recount'
export type ModelRequestCapacityStatus =
  'unconfigured' | 'within_budget' | 'over_budget' | 'invalid_configuration'
export type ModelRequestUsageNormalization =
  'open_ai_input_tokens' | 'anthropic_input_plus_cache' | 'unavailable'
/**
 * Provider-owned request-envelope ordering. This is not evidence of a cache hit; only provider
 * usage counters are authoritative for actual cache reuse.
 */
export type ProviderCacheTopology =
  'provider_defined_separate_fields' | 'tools_before_system_messages'

export interface ModelRequestToolSetObservation {
  stableRevision: string
  dynamicRevision: string
  effectiveRevision: string
  stableToolCount: number
  dynamicToolCount: number
  providerCacheTopology: ProviderCacheTopology
}

export interface ModelRequestEstimate {
  estimatorId: string
  estimatorVersion: number
  measurementMode: ModelRequestMeasurementMode
  capacityStatus: ModelRequestCapacityStatus
  contextRevision: string
  persistentRevision: string
  fixedInputTokens: number
  durableInputTokens: number
  runTransientInputTokens: number
  requestOnlyInputTokens: number
  additiveInputTokens: number
  verifiedTotalInputTokens?: number
  estimatedInputTokens: number
  contextWindowTokens?: number
  reservedOutputTokens: number
  safetyMarginTokens: number
}

export interface ModelRequestActualUsage {
  raw: AgentUsage
  normalizedInputTokens?: number
  normalization: ModelRequestUsageNormalization
}

export interface ModelRequestObservation {
  schemaVersion: number
  id: string
  runId: string
  conversationId?: string
  assistantMessageId?: string
  operationId?: string
  requestIndex: number
  purpose: ModelRequestPurpose
  model: string
  apiStyle: AgentObservedApiStyle
  status: ModelRequestObservationStatus
  /** Present only for Agent-loop requests. Context-compaction requests never expose Agent Tools. */
  toolSet?: ModelRequestToolSetObservation
  estimate?: ModelRequestEstimate
  actualUsage?: ModelRequestActualUsage
  finishReason?: string
  errorCode?: string
  errorMessage?: string
  startedAt: number
  completedAt: number
}

export type ContextJournalCursor =
  | { kind: 'message'; messageId: string }
  | { kind: 'trace_item'; assistantMessageId: string; sequence: number }

export type ContextCompactionReceiptStatus =
  'in_progress' | 'applied' | 'refreshed' | 'failed' | 'cancelled' | 'interrupted'
export type ContextCompactionReceiptStage =
  'planned' | 'preparing' | 'generating' | 'committing' | 'completed'

export interface ContextCompactionReceiptPlan {
  contextRevision: string
  persistentRevision: string
  requestInputTokens: number
  availableInputTokens?: number
  requestTriggerInputTokens?: number
  requestTargetInputTokens?: number
  requestPressure: boolean
  durableInputTokens: number
  durableCapacityTokens?: number
  durableTriggerInputTokens?: number
  durableTargetInputTokens?: number
  durablePressure: boolean
  sourceInputTokens: number
  targetReplacementTokens: number
  expectedReclaimedTokens: number
  plannedReclaimedTokens: number
  projectedRequestInputTokens: number
  projectedDurableInputTokens: number
  bestEffort: boolean
  protectedInputTokens: number
  protectedReasons: Record<string, number>
  atomicUnitCount: number
  previousSummaryId?: string
  coveredThrough: ContextJournalCursor
}

export interface ContextCompactionReceiptResult {
  summaryId: string
  sourceInputTokens: number
  summaryInputTokens: number
  continuityInputTokens: number
  replacementInputTokens: number
  reclaimedInputTokens: number
}

export interface ContextCompactionReceiptError {
  code?: string
  message: string
}

export interface ContextCompactionReceipt {
  schemaVersion: number
  operationId: string
  runId: string
  conversationId: string
  assistantMessageId: string
  requestIndex: number
  attemptIndex: number
  model: string
  apiStyle: AgentObservedApiStyle
  status: ContextCompactionReceiptStatus
  stage: ContextCompactionReceiptStage
  plan: ContextCompactionReceiptPlan
  sourceRevision?: string
  generationObservationId?: string
  summaryId?: string
  result?: ContextCompactionReceiptResult
  error?: ContextCompactionReceiptError
  startedAt: number
  updatedAt: number
  completedAt?: number
}

export type ContextCompactionAuditVerdict = 'pass' | 'warning' | 'fail' | 'in_progress'
export type ContextCompactionAuditCheckStatus = 'pass' | 'warning' | 'fail'
export type ContextCompactionSummaryRelation = 'active' | 'superseded' | 'detached' | 'missing'

export interface ContextCompactionSummaryEvidence {
  summaryId: string
  relation: ContextCompactionSummaryRelation
  sourceRevision?: string
  sourceInputTokens?: number
  summaryInputTokens?: number
  continuityInputTokens?: number
  replacementInputTokens?: number
}

export interface ContextCompactionAuditCheck {
  code: string
  status: ContextCompactionAuditCheckStatus
  message: string
  details?: unknown
}

export interface ContextCompactionAuditReport {
  operationId: string
  verdict: ContextCompactionAuditVerdict
  receipt: ContextCompactionReceipt
  generationObservation?: ModelRequestObservation
  summary?: ContextCompactionSummaryEvidence
  checks: ContextCompactionAuditCheck[]
}

export interface ModelRequestEstimationErrorGroup {
  model: string
  apiStyle: AgentObservedApiStyle
  purpose: ModelRequestPurpose
  observationCount: number
  comparableSampleCount: number
  estimateUnavailableCount: number
  actualUsageUnavailableCount: number
  retryAffectedCount: number
  estimatedInputTokens: number
  normalizedActualInputTokens: number
  /** Sum of estimated minus actual input tokens; positive means overestimation. */
  estimatedMinusActualTokens: number
  weightedSignedErrorBasisPoints?: number
  weightedAbsoluteErrorBasisPoints?: number
  medianAbsolutePercentageErrorBasisPoints?: number
  p95AbsolutePercentageErrorBasisPoints?: number
  underestimationCount: number
  overestimationCount: number
  exactCount: number
}

export interface ContextCompactionAuditBundle {
  conversationId: string
  generatedAt: number
  reports: ContextCompactionAuditReport[]
  estimationErrorGroups: ModelRequestEstimationErrorGroup[]
}

export interface AgentContextCompactionAuditInput {
  conversationId: string
  operationId?: string
  limit?: number
}

export interface AgentContextCompactionAuditOutput {
  report: ContextCompactionAuditBundle
}

export interface AgentConversationMessage {
  id: string
  role: 'user' | 'assistant'
  content: string
  createdAt: number
  status?: 'pending' | 'sent' | 'error' | null
  attachments?: AgentConversationMessageAttachment[]
}

export interface AgentConversationTurnOutput {
  runId: string
  eventName: string
  conversationId: string
  userMessageId: string
  assistantMessageId: string
  userMessage: AgentConversationMessage
  assistantMessage: AgentConversationMessage
  activatedSkills: ActivatedSkillSummary[]
  skillActivationRevision?: string
}

export interface AgentCancelRunRequest {
  runId: string
}

export interface AgentCancelRunResponse {
  runId: string
  cancelled: boolean
}

export interface AgentActionIdRequest {
  runId: string
  actionId: string
}

export interface AgentRejectActionRequest {
  runId: string
  actionId: string
  message?: string
}

export type PendingAgentActionStatus =
  'pending' | 'approved' | 'executing' | 'rejected' | 'cancelled' | 'completed' | 'failed'

export interface PendingAgentActionSnapshot {
  actionId: string
  actionType: string
  toolName: string
  toolCallId?: string | null
  runId: string
  conversationId?: string | null
  assistantMessageId?: string | null
  action: AgentProposedAction
  createdAt: number
  status: PendingAgentActionStatus
}

export interface AgentStateSnapshot {
  status: AgentRunStatus
  activeRunId: string | null
  lastError: string | null
  updatedAt: number
}

export interface AgentToolCall {
  id: string
  tool: AgentToolName
  args: unknown
  approvalStatus: AgentApprovalStatus
  reason?: string
}

export interface AgentToolDefinition {
  name: AgentToolName
  description: string
  inputSchema: unknown
  safety: AgentToolSafety
  requiresWorkspace: boolean
  requiresApproval: boolean
  approvalMode: AgentToolApprovalMode
}

export interface AgentToolResult {
  callId: string
  tool: AgentToolName
  ok: boolean
  result?: unknown
  error?: string
}

export interface AgentToolContinuation {
  call: AgentToolCall
  result: AgentToolResult
}

export interface AgentDiffProposal {
  id: string
  operation: AgentPatchOperation
  filePath: string
  patch: string
  baseRevision?: string
  summary?: string
  approvalStatus: AgentApprovalStatus
}

export interface AgentFileDraftSnapshot {
  draftId: string
  conversationId: string
  projectId?: string
  filePath: string
  mode: AgentFileWriteMode
  status: AgentFileDraftStatus
  baseRevision?: string
  additions: number
  deletions: number
  lineCount: number
  byteCount: number
  chunkCount: number
  nextChunkIndex: number
  statsFinal: boolean
  summary?: string
  createdAt: number
  updatedAt: number
}

export interface AgentFileWritePreview {
  previewId: string
  streamId: string
  attempt: number
  toolCallIndex: number
  toolCallId?: string
  draftId: string
  filePath: string
  additions: number
  deletions: number
  lineCount: number
  byteCount: number
  generatedBytes: number
  contentOffsetBytes: number
  contentDelta: string
  updatedAt: number
}

export interface AgentFileWriteProposal {
  id: string
  draftId: string
  mode: AgentFileWriteMode
  filePath: string
  baseRevision?: string
  summary?: string
  additions: number
  deletions: number
  lineCount: number
  byteCount: number
  approvalStatus: AgentApprovalStatus
}

export interface AgentFileWriteResult {
  status: AgentFileWriteResultStatus
  draftId: string
  mode: AgentFileWriteMode
  filePath: string
  additions: number
  deletions: number
  lineCount: number
  byteCount: number
  revision?: string
  error?: string
  message?: string
}

export interface AgentFileDraftIdInput {
  draftId: string
}

export interface AgentFileDraftReadInput extends AgentFileDraftIdInput {
  offset?: number
  maxChars?: number
}

export interface AgentFileDraftContentPage {
  draft: AgentFileDraftSnapshot
  content: string
  offset: number
  nextOffset?: number
  truncated: boolean
}

export interface AgentFileWriteDiffInput extends AgentFileDraftIdInput {
  offset?: number
  maxChars?: number
}

export interface AgentFileWriteDiffPage {
  draftId: string
  patch: string
  offset: number
  nextOffset?: number
  truncated: boolean
}

export interface AgentPatchResult {
  status: AgentPatchResultStatus
  operation: AgentPatchOperation
  filePath: string
  appliedFilePaths: string[]
  gitDiff?: AgentGitDiffSnapshot
  gitDiffError?: string
  error?: string
  message?: string
}

export interface AgentGitDiffSnapshot {
  patch: string
  truncated: boolean
}

export type AgentCommandArtifactObservationKind = 'office'

export interface AgentCommandArtifactObservationRequest {
  kinds: AgentCommandArtifactObservationKind[]
  /** Expected Office output files, resolved relative to command cwd. This does not grant access. */
  expectedOutputs?: string[]
  /** Extra files or directories to observe beyond the automatically included workspace. */
  additionalRoots?: string[]
}

export type AgentCommandArtifactObservationStatus = 'complete' | 'partial' | 'failed'
export type AgentCommandArtifactObservationPhase = 'setup' | 'before' | 'after'
export type AgentCommandArtifactKind = 'document' | 'spreadsheet' | 'presentation'
export type AgentCommandArtifactScope = 'workspace' | 'external'
export type AgentCommandArtifactChangeKind =
  'created' | 'modified' | 'replaced' | 'deleted' | 'renamed'
export type AgentCommandExpectedArtifactOutcomeKind =
  | 'created'
  | 'modified'
  | 'replaced'
  | 'renamed'
  | 'unchanged'
  | 'missing'
  | 'unobserved'
  | 'invalid'
export type AgentCommandArtifactValidationStatus =
  'valid' | 'invalid' | 'not_applicable' | 'unchecked'

export interface AgentCommandArtifactValidation {
  status: AgentCommandArtifactValidationStatus
  code?: string
  message?: string
}

export interface AgentCommandArtifactMetadata {
  sizeBytes: number
  sha256?: string
  validation: AgentCommandArtifactValidation
}

export interface AgentCommandArtifactChange {
  kind: AgentCommandArtifactChangeKind
  artifactKind: AgentCommandArtifactKind
  path: string
  scope: AgentCommandArtifactScope
  previousPath?: string
  previousScope?: AgentCommandArtifactScope
  before?: AgentCommandArtifactMetadata
  after?: AgentCommandArtifactMetadata
}

export interface AgentCommandExpectedArtifactOutcome {
  requestedPath: string
  outcome: AgentCommandExpectedArtifactOutcomeKind
  path?: string
  scope?: AgentCommandArtifactScope
  artifactKind?: AgentCommandArtifactKind
  metadata?: AgentCommandArtifactMetadata
}

export interface AgentCommandArtifactSnapshotCoverage {
  rootsScanned: number
  directoryEntriesScanned: number
  officeFilesSeen: number
  filesHashed: number
  filesUnhashed: number
  bytesHashed: number
  symlinksSkipped: number
  excludedDirectories: number
  durationMs: number
  timeBudgetExceeded: boolean
  cancelled: boolean
  truncated: boolean
}

export interface AgentCommandArtifactObservationCoverage {
  workspaceIncluded: boolean
  expectedOutputCount: number
  additionalRootCount: number
  before: AgentCommandArtifactSnapshotCoverage
  after: AgentCommandArtifactSnapshotCoverage
}

export interface AgentCommandArtifactObservationWarning {
  phase: AgentCommandArtifactObservationPhase
  code: string
  path?: string
  message: string
}

export interface AgentCommandArtifactObservation {
  schemaVersion: number
  status: AgentCommandArtifactObservationStatus
  coverage: AgentCommandArtifactObservationCoverage
  changes: AgentCommandArtifactChange[]
  changesTruncated: boolean
  changesOmitted: number
  expectedOutputs?: AgentCommandExpectedArtifactOutcome[]
  warnings?: AgentCommandArtifactObservationWarning[]
}

/** Selects a host-owned runtime without granting command authorization. */
export type AgentCommandRuntimeProvider = 'managedArtifact'

export type AgentCommandRuntimeKind = 'node' | 'python'

/** Model-visible selection of an application-owned reproducible artifact environment. */
export type AgentCommandRuntimeProfile = 'documents' | 'spreadsheets' | 'presentations'

export interface AgentCommandRuntimePackageRequirement {
  name: string
  version: string
}

/** Frozen resolver request carried with the command action. */
export interface AgentCommandRuntimeRequest {
  provider: AgentCommandRuntimeProvider
  kind: AgentCommandRuntimeKind
  requiredPackages: AgentCommandRuntimePackageRequirement[]
}

export interface AgentCommandRuntimeResolvedPackage {
  name: string
  version: string
}

/** Approval-time runtime identity. Host-private executable and environment data are excluded. */
export interface AgentCommandRuntimeBinding {
  schemaVersion: number
  profile: AgentCommandRuntimeProfile
  profileRevision: string
  providerId: string
  bundleVersion: string
  bundleRevision: string
  kind: AgentCommandRuntimeKind
  runtimeVersion: string
  runtimeFingerprint: string
  resolvedPackages: AgentCommandRuntimeResolvedPackage[]
}

/** Public runtime evidence; private executable and component paths are intentionally absent. */
export interface AgentCommandRuntimeResolution {
  schemaVersion: number
  providerId: string
  profile?: AgentCommandRuntimeProfile
  profileRevision?: string
  bundleVersion?: string
  bundleRevision?: string
  kind: AgentCommandRuntimeKind
  runtimeVersion?: string
  runtimeFingerprint?: string
  resolvedPackages?: AgentCommandRuntimeResolvedPackage[]
  errorCode?: string
  recovery?: string
  message?: string
}

export interface AgentCommandRequest {
  id: string
  command: string
  cwd?: string
  timeoutMs?: number
  approvalStatus: AgentApprovalStatus
  riskLevel?: AgentCommandRiskLevel
  reason?: string
  observe?: AgentCommandArtifactObservationRequest
  /** @deprecated Only present on legacy pending actions, which the host refuses and reprepares. */
  runtime?: AgentCommandRuntimeRequest
  runtimeBinding?: AgentCommandRuntimeBinding
}

export type AgentSkillMaterializationResultStatus =
  'applied' | 'already_applied' | 'conflict' | 'rejected' | 'failed'

export interface AgentSkillMaterializationRequest {
  id: string
  sourceUri: string
  sourcePrefix?: string
  destination: string
  approvalStatus: AgentApprovalStatus
  reason?: string
}

export interface AgentSkillMaterializationResult {
  status: AgentSkillMaterializationResultStatus
  sourceUri: string
  sourcePrefix?: string
  destination: string
  sourceRevision: string
  fileCount: number
  byteCount: number
  planDigest?: string
  error?: string
  message?: string
}

export type AgentSkillScriptInterpreter = 'python3'
export type AgentSkillScriptPreflightStatus =
  'ready' | 'missing_dependencies' | 'unsupported' | 'conflict'
export type AgentSkillDependencyKind = 'python_distribution' | 'command'
export type AgentSkillDependencyStatus = 'available' | 'missing'

export interface AgentSkillScriptRequirements {
  pythonDistributions?: string[]
  commands?: string[]
}

export interface AgentSkillDependencyCheck {
  kind: AgentSkillDependencyKind
  name: string
  status: AgentSkillDependencyStatus
  version?: string
}

export interface AgentSkillScriptPreflightReport {
  status: AgentSkillScriptPreflightStatus
  interpreter: AgentSkillScriptInterpreter
  interpreterVersion?: string
  dependencies?: AgentSkillDependencyCheck[]
  runtimeFingerprint: string
  errorCode?: string
  message?: string
}

export interface AgentSkillScriptRequest {
  id: string
  scriptUri: string
  skillId: string
  skillRevision: string
  resourcePath: string
  resourceDigest: string
  interpreter: AgentSkillScriptInterpreter
  args: string[]
  requirements: AgentSkillScriptRequirements
  preflight: AgentSkillScriptPreflightReport
  timeoutMs?: number
  approvalStatus: AgentApprovalStatus
  reason?: string
}

export interface AgentSkillScriptResult {
  scriptUri: string
  skillId: string
  skillRevision: string
  resourceDigest: string
  preflight: AgentSkillScriptPreflightReport
  exitCode?: number
  stdout: string
  stderr: string
  timedOut: boolean
  cancelled: boolean
  durationMs: number
  stdoutTruncated: boolean
  stderrTruncated: boolean
  errorCode?: string
  error?: string
}

export type OfficeDocumentKind = 'document' | 'spreadsheet' | 'presentation'

export type OfficeOperation =
  | 'help'
  | 'create'
  | 'view'
  | 'get'
  | 'query'
  | 'validate'
  | 'set'
  | 'add'
  | 'remove'
  | 'move'
  | 'swap'

/** Describes whether the normalized operation has file side effects.
 *
 * Path location is expressed independently by each OfficeFrozenPath.scope.
 */
export type OfficeOperationAccess = 'readOnly' | 'fileWrite'

/** Provider-neutral help topic.
 *
 * status/help/create/view/validate/move/swap are served by the Host. The remaining topics select
 * provider element-schema help and may be paired with an element name.
 */
export type OfficeHelpVerb =
  | 'status'
  | 'help'
  | 'create'
  | 'view'
  | 'get'
  | 'query'
  | 'validate'
  | 'set'
  | 'add'
  | 'remove'
  | 'move'
  | 'swap'

export type OfficeViewMode =
  'text' | 'annotated' | 'outline' | 'stats' | 'issues' | 'html' | 'svg' | 'screenshot' | 'forms'

export type OfficeViewRenderMode = 'auto' | 'html'

export type OfficeGridLayout = { mode: 'auto' } | { mode: 'columns'; columns: number }

export type OfficeCellShift = 'left' | 'up'

export type OfficeElementPosition =
  | { type: 'index'; index: number }
  | { type: 'after'; target: string }
  | { type: 'before'; target: string }

export interface OfficeTextReplacement {
  find: string
  replace: string
}

export interface OfficePageRange {
  start: number
  end?: number
}

export interface OfficeViewport {
  width: number
  height: number
}

export type OfficePropertyValue = string | number | boolean | { resourcePath: string }

export type OfficePropertyMap = Record<string, OfficePropertyValue>

export type OfficeOperationParameters =
  | { type: 'help'; verb?: OfficeHelpVerb; element?: string }
  | { type: 'create'; locale?: string; minimal?: boolean; overwrite?: boolean }
  | {
      type: 'view'
      mode: OfficeViewMode
      start?: number
      end?: number
      maxLines?: number
      issueType?: string
      limit?: number
      columns?: string[]
      pages?: OfficePageRange[]
      range?: string
      viewport?: OfficeViewport
      grid?: OfficeGridLayout
      renderMode?: OfficeViewRenderMode
      pageCount?: boolean
    }
  | { type: 'get'; target?: string; depth?: number }
  | { type: 'query'; selector: string; contains?: string; compact?: boolean; fields?: string[] }
  | { type: 'validate' }
  | {
      type: 'set'
      target: string
      properties?: OfficePropertyMap
      replacement?: OfficeTextReplacement
      force?: boolean
    }
  | {
      type: 'add'
      parent: string
      elementType: string
      copyFrom?: string
      position?: OfficeElementPosition
      properties?: OfficePropertyMap
      force?: boolean
    }
  | { type: 'remove'; target: string; shift?: OfficeCellShift; properties?: OfficePropertyMap }
  | {
      type: 'move'
      target: string
      newParent?: string
      position?: OfficeElementPosition
      properties?: OfficePropertyMap
    }
  | { type: 'swap'; firstTarget: string; secondTarget: string }

interface OfficeExecutionRequestBase {
  documentKind: OfficeDocumentKind
  operation: OfficeOperation
  documentPath?: string
  outputPath?: string
  destinationPath?: string
  timeoutMs?: number
}

export type OfficeExecutionRequest = OfficeExecutionRequestBase &
  (
    | { parameters: OfficeOperationParameters; arguments?: never }
    /** @deprecated Schema-v3 compatibility only; schema-v4 actions use typed parameters. */
    | { parameters?: never; arguments: string[] }
  )

export type OfficeFilePreconditionState = 'missing' | 'present'

export interface OfficeFilePrecondition {
  path: string
  state: OfficeFilePreconditionState
  contentRevision?: string
  size?: number
}

export type OfficePathSlot =
  | { type: 'document' }
  | { type: 'output' }
  | { type: 'destination' }
  | { type: 'resource'; index: number }

export type OfficePathPurpose = 'readSource' | 'writeTarget' | 'inPlaceTarget'

export type OfficePathScope = 'workspace' | 'external' | 'attachment'

export type OfficeWriteDisposition = 'createNew' | 'replaceExisting'

export interface OfficePathIdentity {
  revision: string
  device?: number
  inode?: number
}

/**
 * One backend-authorized path in an immutable Office execution snapshot.
 * The Renderer may display this metadata but must never derive authorization
 * or approval requirements from it.
 */
export interface OfficeFrozenPath {
  slot: OfficePathSlot
  logicalPath: string
  purpose: OfficePathPurpose
  scope: OfficePathScope
  normalizedPath: string
  state: OfficeFilePreconditionState
  objectIdentity?: OfficePathIdentity
  parentIdentity: OfficePathIdentity
  contentRevision?: string
  size?: number
  writeDisposition?: OfficeWriteDisposition
}

/** Immutable, shell-free Office execution snapshot prepared by the trusted backend. */
export interface OfficePreparedExecution {
  schemaVersion: number
  providerId: string
  engineRevision: string
  workspaceRevision?: string
  access: OfficeOperationAccess
  request: OfficeExecutionRequest
  argv: string[]
  paths: OfficeFrozenPath[]
  /** @deprecated Schema-v2 compatibility only; schema-v3 actions use paths. */
  documentPrecondition?: OfficeFilePrecondition
  /** @deprecated Schema-v2 compatibility only; schema-v3 actions use paths. */
  outputPrecondition?: OfficeFilePrecondition
  /** @deprecated Schema-v2 compatibility only; schema-v3 actions use paths. */
  destinationPrecondition?: OfficeFilePrecondition
  /** @deprecated Schema-v2 compatibility only; schema-v3 actions use paths. */
  resourcePreconditions?: OfficeFilePrecondition[]
}

export interface AgentOfficeOperationRequest {
  schemaVersion: number
  id: string
  prepared: OfficePreparedExecution
  approvalStatus: AgentApprovalStatus
  reason: string
}

export type AgentProposedAction =
  | { type: 'tool_call'; call: AgentToolCall }
  | { type: 'diff'; diff: AgentDiffProposal }
  | { type: 'file_write'; fileWrite: AgentFileWriteProposal }
  | { type: 'command'; command: AgentCommandRequest }
  | { type: 'skill_materialization'; materialization: AgentSkillMaterializationRequest }
  | { type: 'skill_script'; script: AgentSkillScriptRequest }
  | { type: 'office_operation'; officeOperation: AgentOfficeOperationRequest }

export type AgentEvent =
  | { type: 'started'; runId: string; toolDefinitions: AgentToolDefinition[] }
  | {
      type: 'tool_set_changed'
      runId: string
      stableRevision: string
      dynamicRevision: string
      effectiveRevision: string
      toolDefinitions: AgentToolDefinition[]
    }
  | { type: 'state'; runId: string; state: AgentStateSnapshot }
  | { type: 'message_delta'; runId: string; streamId?: string; delta: string }
  | { type: 'message_stream_started'; runId: string; streamId: string; attempt: number }
  | { type: 'message_stream_reset'; runId: string; streamId: string; reason: string }
  | { type: 'message_stream_committed'; runId: string; streamId: string }
  | {
      type: 'llm_retry'
      runId: string
      streamId: string
      attempt: number
      maxAttempts: number
      reason: string
    }
  | {
      type: 'tool_input_progress'
      runId: string
      streamId: string
      attempt: number
      toolCallIndex: number
      toolCallId?: string
      tool: string
      receivedBytes: number
    }
  | {
      type: 'file_write_preview_updated'
      runId: string
      preview: AgentFileWritePreview
    }
  | {
      type: 'file_write_preview_cleared'
      runId: string
      streamId: string
      attempt: number
    }
  | { type: 'message'; runId: string; content: string }
  | {
      type: 'guidance_queued'
      runId: string
      guidanceId: string
      clientMessageId: string
      content: string
      attachments: ConversationTraceAttachment[]
      createdAt: number
    }
  | {
      type: 'guidance_applied'
      runId: string
      guidanceId: string
      clientMessageId: string
      content: string
      attachments: ConversationTraceAttachment[]
      createdAt: number
      sequence: number
    }
  | {
      type: 'guidance_rejected'
      runId: string
      guidanceId: string
      clientMessageId: string
      content: string
      rejectionCode: AgentSteerRunRejectionCode
      message: string
      createdAt: number
    }
  | { type: 'tool_call'; runId: string; call: AgentToolCall }
  | { type: 'tool_result'; runId: string; result: AgentToolResult }
  | { type: 'todo_updated'; runId: string; todo: AgentTodoState }
  | {
      type: 'skill_activated'
      runId: string
      activationRevision: string
      activatedBy: 'user' | 'model'
      skill: ActivatedSkillSummary
    }
  | { type: 'file_draft_updated'; runId: string; draft: AgentFileDraftSnapshot }
  | {
      type: 'context_window_updated'
      runId: string
      conversationId?: string
      snapshot: AgentContextWindowSnapshot
    }
  | { type: 'context_compaction_started'; runId: string; operationId: string }
  | {
      type: 'context_compaction_finished'
      runId: string
      operationId: string
      outcome: AgentContextCompactionEventOutcome
    }
  | { type: 'approval_required'; runId: string; action: AgentProposedAction }
  | { type: 'diff'; runId: string; diff: AgentDiffProposal }
  | {
      type: 'command_output'
      runId: string
      command: string
      stream: AgentCommandOutputStream
      output: string
    }
  | {
      type: 'error'
      runId?: string
      message: string
      recoverable: boolean
      code?: string
      details?: unknown
    }
  | {
      type: 'done'
      runId: string
      success: boolean
      status?: AgentRunStatus
      content?: string
      usage?: AgentUsage
      finishReason?: string
      proposedActions?: AgentProposedAction[]
    }

export type AgentContextCompactionEventOutcome = 'applied' | 'skipped' | 'failed' | 'cancelled'
