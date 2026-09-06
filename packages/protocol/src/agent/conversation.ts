import type { ActivatedSkillSummary, SkillSelection } from '../skills'
import type { AgentProposedAction } from './approvals'
import type { AgentCommandExecutionResult } from './command'
import type {
  AgentApprovalDecisionStatus,
  AgentApprovalStatus,
  AgentMessageRole,
  AgentPermissions,
  AgentPromptDetailLevel,
  AgentPromptTone,
  AgentPromptWorkMode,
  AgentRunStatus,
  AgentSearchMode,
  AgentTodoStatus,
  AgentToolApprovalMode,
  AgentToolName,
  AgentToolSafety,
  ConversationTurnTrace
} from './core'
import type { AgentEvent } from './events'
import type { AgentFileChangeResult } from './fileChange'

export interface AgentChatMessage {
  messageId?: string
  role: AgentMessageRole
  content: string
  conversationTurnTrace?: ConversationTurnTrace
  /** Host-only projection; the assistant's completion is already in the compaction summary. */
  conversationCompletionCovered?: boolean
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
  projectId: string | null
  displayName: string | null
  rootPath: string | null
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
  /** Other conversations authorized by the current project or the same Agent task tree. */
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
  conversationId: string | null
  projectId: string | null
  workspace: AgentWorkspaceContext | null
  attachmentLibrary?: AgentAttachmentLibraryContext
  permissions: AgentPermissions
}

export interface AgentSearchConfig {
  mode: AgentSearchMode
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

export interface AgentContextCostBreakdown {
  systemTokens: number
  toolSchemaTokens: number
  summaryTokens: number
  worldStateTokens: number
  todoTokens: number
  /** Hidden provider protocol-state cost; never includes continuation content. */
  providerContinuationTokens: number
  /** Uncovered history and remaining current-run context. */
  recentHistoryTokens: number
  totalInputTokens: number
}

export interface AgentContextWindowSnapshot {
  model: string
  status: AgentContextWindowStatus
  contextWindowTokens?: number
  reservedOutputTokens: number
  safetyMarginTokens: number
  /** Total input capacity after output and safety reserves. */
  inputCapacityTokens?: number
  /** Complete input after conversation start; an unstarted conversation publishes zero. */
  inputTokens: number
  costBreakdown: AgentContextCostBreakdown
  remainingInputTokens?: number
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

export type AgentActionExecutionStatus =
  | 'applied'
  | 'approved'
  | 'failed'
  | 'conflict'
  | 'rejected'
  | 'cancelled'
  | 'expired'
  | 'outcome_unknown'

export interface AgentActionExecutionOutput {
  actionId: string
  actionType: string
  toolName: string
  status: AgentActionExecutionStatus
  fileChangeResult?: AgentFileChangeResult
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

/**
 * Replaces only the latest settled human Turn inside the same Conversation.
 * The source Turn remains an immutable audit/Usage fact and is removed only from active views.
 */
export interface AgentConversationTurnRewriteInput {
  requestId: string
  sourceUserMessageId: string
  sourceAssistantMessageId: string
  turn: AgentConversationTurnInput
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
  /** Stable local configuration identity; `snapshot.model` remains the provider wire model. */
  modelConfigId: string
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
  systemTokens: number
  toolSchemaTokens: number
  summaryTokens: number
  worldStateTokens: number
  todoTokens: number
  /** Hidden provider-native replay state included in the final wire request. */
  providerContinuationTokens: number
  recentHistoryTokens: number
  totalInputTokens: number
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
  sourceInputTokens: number
  retainedInputTokens: number
  targetReplacementTokens: number
  expectedReclaimedTokens: number
  plannedReclaimedTokens: number
  projectedRequestInputTokens: number
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

export type AgentApprovalScope = 'singleAction' | 'remainingApplyPatchInRun'

export interface AgentApproveActionRequest {
  runId: string
  actionId: string
  approvalScope: AgentApprovalScope
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
  reason: string | null
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
