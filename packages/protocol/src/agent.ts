import type { ActivatedSkillSummary, SkillSelection } from './skills'
import type { McpBuiltinCapabilityId } from './mcp/contracts'

/**
 * Agent JSON-RPC names are transport contract, not host implementation details.
 * Rust verifies the same values against the shared Agent golden fixture.
 */
export const AGENT_CANCEL_RUN_METHOD = 'agent.cancelRun'
export const AGENT_STEER_RUN_METHOD = 'agent.steerRun'
export const AGENT_START_CONVERSATION_TURN_METHOD = 'agent.startConversationTurn'
export const AGENT_REWRITE_CONVERSATION_TURN_METHOD = 'agent.rewriteConversationTurn'
export const AGENT_GET_CONTEXT_WINDOW_SNAPSHOT_METHOD = 'agent.getContextWindowSnapshot'
export const AGENT_COMMAND_SESSIONS_LIST_METHOD = 'agent.commandSessions.list'
export const AGENT_COMMAND_SESSIONS_GET_METHOD = 'agent.commandSessions.get'
export const AGENT_LIST_PENDING_ACTIONS_METHOD = 'agent.listPendingActions'
export const AGENT_APPROVE_ACTION_METHOD = 'agent.approveAction'
export const AGENT_REJECT_ACTION_METHOD = 'agent.rejectAction'
export const AGENT_CANCEL_ACTION_METHOD = 'agent.cancelAction'
export const AGENT_GET_USAGE_SUMMARY_METHOD = 'agent.getUsageSummary'
export const AGENT_CLEAR_USAGE_RECORDS_METHOD = 'agent.clearUsageRecords'
export const AGENT_READ_FILE_DRAFT_METHOD = 'agent.readFileDraft'
export const AGENT_GET_FILE_WRITE_DIFF_METHOD = 'agent.getFileWriteDiff'
export const AGENT_EVENT_NOTIFICATION_METHOD = 'agent.event'

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

export type AgentLlmRetryCategory =
  | 'rate_limited'
  | 'quota_exhausted'
  | 'overloaded'
  | 'authentication'
  | 'invalid_request'
  | 'context_too_large'
  | 'network'
  | 'unknown'

export type AgentToolName =
  | 'attachments_list'
  | 'attachments_list_project'
  | 'read_file'
  | 'read_image'
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
  | 'skills_prepare_install'
  | 'skills_commit_install'
  | 'office_document'
  | 'office_spreadsheet'
  | 'office_presentation'
  | 'image_generation'
  | (string & {})

export type AgentToolSafety = 'read_only' | 'requires_approval' | 'destructive'

export type AgentMcpServerScope =
  | { type: 'builtin' }
  | { type: 'user' }
  | { type: 'project'; projectId: string }
  | { type: 'plugin'; pluginId: string }
  | { type: 'managed' }

/**
 * Immutable MCP catalog identity captured for one Agent run.
 *
 * The model-visible name is not an authority boundary. Backend invocation routes with this typed
 * identity and revalidates the server configuration digest and catalog generation.
 */
export interface AgentMcpToolProvenance {
  serverId: string
  scope: AgentMcpServerScope
  rawToolName: string
  modelToolName: string
  configEpoch: string
  registryRevision: number
  configDigest: string
  catalogGeneration: number
  catalogDigest: string
  catalogSchemaDigest: string
  schemaDigest: string
  schemaNormalizerVersion: number
}

export type AgentMcpToolRisk =
  | 'unknown'
  | 'read_only_claimed'
  | 'side_effects_possible'
  | 'destructive_claimed'
  | 'open_world_claimed'

export type AgentMcpApprovalMode = 'prompt' | 'auto' | 'deny'

export interface AgentMcpArgumentSummary {
  encodedBytes: number
  topLevelPropertyCount: number
  stringValueCount: number
  numberValueCount: number
  booleanValueCount: number
  nullValueCount: number
  objectValueCount: number
  arrayValueCount: number
  maxDepth: number
  truncated: boolean
}

/**
 * Renderer-safe invocation identity. The Host-only arguments digest is deliberately removed at
 * the Rust serialization boundary because low-entropy argument values could be brute-forced.
 */
export interface AgentMcpToolInvocationIdentity {
  actionId: string
  invocationId: string
  runId: string
  callId: string
  provenance: AgentMcpToolProvenance
}

export interface AgentMcpToolApprovalSummary {
  serverId: string
  serverDisplayName: string
  scope: AgentMcpServerScope
  rawToolName: string
  modelToolName: string
  /**
   * Bounded model-authored display text. It is never forwarded to the MCP Server, remains
   * untrusted, and may contain user-provided sensitive text.
   */
  displayReason: string | null
  arguments: AgentMcpArgumentSummary
  risk: AgentMcpToolRisk
  external: boolean
}

export type AgentMcpApprovalPayloadPersistence = 'process_only' | 'durable_authenticated_envelope'

export interface AgentMcpToolApproval {
  identity: AgentMcpToolInvocationIdentity
  call: AgentToolCall
  summary: AgentMcpToolApprovalSummary
  approvalMode: AgentMcpApprovalMode
  payloadPersistence: AgentMcpApprovalPayloadPersistence
  createdAt: number
  expiresAt: number
}

export type AgentMcpToolInvocationState =
  | 'pending_approval'
  | 'approved'
  | 'dispatching'
  | 'running'
  | 'completed'
  | 'failed'
  | 'cancelled'
  | 'rejected'
  | 'expired'
  | 'payload_unavailable'
  | 'policy_denied'
  | 'outcome_unknown'

export type AgentMcpToolInvocationOutcome =
  | 'succeeded'
  | 'tool_error'
  | 'output_too_large'
  | 'transport_error'
  | 'timed_out'
  | 'cancelled'
  | 'rejected'
  | 'expired'
  | 'payload_unavailable'
  | 'policy_denied'
  | 'outcome_unknown'

export type AgentMcpDispatchCertainty =
  'definitely_not_dispatched' | 'possibly_dispatched' | 'response_received'

export type AgentMcpInvocationFailureStage =
  | 'preflight'
  | 'approval_payload'
  | 'policy'
  | 'dispatch'
  | 'transport'
  | 'server_response'
  | 'result_projection'
  | 'persistence'
  | 'shutdown'

export interface AgentMcpResultSizeSummary {
  contentBlockCount: number
  textBytes: number
  structuredBytes: number
  omittedBlockCount: number
  omittedEncodedBytes: number
}

export interface AgentMcpInvocationDiagnostics {
  schemaVersion: 1
  argumentEncodedBytes: number
  argumentValueCount: number
  argumentMaxDepth: number
  result?: AgentMcpResultSizeSummary
  failureStage?: AgentMcpInvocationFailureStage
}

export interface AgentMcpToolInvocationEvent {
  actionId: string
  invocationId: string
  callId: string
  serverId: string
  serverDisplayName: string
  rawToolName: string
  modelToolName: string
  /**
   * Bounded model-authored display text copied from the frozen approval summary. Render as plain
   * text; do not treat it as secret-redacted or log it separately.
   */
  displayReason: string | null
  external: boolean
  state: AgentMcpToolInvocationState
  dispatchCertainty: AgentMcpDispatchCertainty
  outcome: AgentMcpToolInvocationOutcome | null
  isError: boolean | null
  errorCode: string | null
  durationMs: number | null
  outputTruncated: boolean
  /** Value-free, Host-classified diagnostics. Renderer presentation should not persist it. */
  diagnostics: AgentMcpInvocationDiagnostics | null
}

export type AgentToolIdentity =
  | { type: 'builtin'; toolName: string }
  | { type: 'runtime_extension'; extensionId: string; toolName: string }
  | {
      type: 'builtin_capability'
      capabilityId: McpBuiltinCapabilityId
      managedMcpId: string
      packageName: string
      packageVersion: string
      upstreamCatalogDigest: string
      policyDigest: string
      manifestDigest: string
      toolId: string
      rawName: string
      modelName: string
      upstreamSchemaDigest: string
      hostOverlayDigest: string
      hostInputSchemaDigest: string
    }
  | { type: 'mcp'; provenance: AgentMcpToolProvenance }
  | { type: 'unregistered'; toolName: string }

export type AgentToolApprovalMode = 'never' | 'always' | 'dynamic'

export type AgentApprovalStatus = 'not_required' | 'required' | 'approved' | 'rejected'

export type AgentApprovalDecisionStatus = 'approved' | 'rejected'

export type ConversationTurnTraceTerminalStatus = 'completed' | 'failed' | 'cancelled'

export type ConversationTraceToolResultStatus =
  'succeeded' | 'failed' | 'rejected' | 'conflict' | 'cancelled'

export type ConversationCommandSessionLifecyclePhase = 'started' | 'terminal'

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
      provenance: AgentToolIdentity
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
  | {
      type: 'command_session_lifecycle'
      sequence: number
      phase: ConversationCommandSessionLifecyclePhase
      sessionId: string
      callId: string
      status: AgentCommandSessionStatus
      exitCode?: number
      latestSequence: number
      outputTruncated: boolean
      archiveRef?: string
      contentHash?: string
      archivedBytes?: number
      archivedCompletely?: boolean
      truncatedAtSource?: boolean
      modelProjectionTruncated?: boolean
      historyProjectionTruncated?: boolean
      archiveProjectionTruncated?: boolean
      createdAt: number
    }
  | {
      type: 'context_compaction_lifecycle'
      sequence: number
      phase: 'started' | 'finished'
      operationId: string
      outcome?: AgentContextCompactionEventOutcome
    }
  | {
      type: 'runtime_error'
      sequence: number
      message: string
      recoverable: boolean
      code?: string
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

export const AGENT_COMMAND_SESSION_SCHEMA_VERSION = 2

/** Process Session state is independent from Agent Run and approval state. */
export type AgentCommandSessionStatus =
  'starting' | 'running' | 'exited' | 'interrupted' | 'timed_out' | 'failed' | 'outcome_unknown'

/** Bounded Renderer-safe projection; it contains no process handles, environment, or raw output. */
export interface AgentCommandSessionSnapshot {
  schemaVersion: typeof AGENT_COMMAND_SESSION_SCHEMA_VERSION
  sessionId: string
  conversationId: string
  assistantMessageId: string
  originRunId: string
  callId: string
  projectId?: string
  command: string
  cwd: string
  commandDigest: string
  status: AgentCommandSessionStatus
  startedAt: number
  endedAt?: number
  exitCode?: number
  latestSequence: number
  outputTruncated: boolean
  outputs?: AgentCommandPublishedOutput[]
  /** Bounded Host observation attached only after terminal command settlement. */
  artifactObservation?: AgentCommandArtifactObservation
  archiveRef?: string
}

export interface AgentCommandSessionOutputChunk {
  sequence: number
  stream: AgentCommandOutputStream
  output: string
}

/** Maximum chunk count accepted across the Rust/TypeScript command transcript boundary. */
export const AGENT_COMMAND_SESSION_MAX_TRANSCRIPT_CHUNKS = 2048

export interface AgentCommandSessionTranscript {
  requestedAfterSequence: number
  firstAvailableSequence?: number
  latestSequence: number
  truncatedBefore: boolean
  outputCaptureTruncated: boolean
  chunks: AgentCommandSessionOutputChunk[]
}

export interface AgentCommandSessionListInput {
  conversationId: string
}

export interface AgentCommandSessionListOutput {
  sessions: AgentCommandSessionSnapshot[]
}

export interface AgentCommandSessionGetInput {
  conversationId: string
  sessionId: string
  afterSequence?: number
  maxBytes?: number
}

export interface AgentCommandSessionGetOutput {
  session: AgentCommandSessionSnapshot
  transcript: AgentCommandSessionTranscript
}

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

export type AgentActionExecutionStatus = 'applied' | 'approved' | 'failed' | 'conflict' | 'rejected'

export type AgentCommandPublishedOutputKind = 'image' | 'document'

export interface AgentCommandPublishedOutput {
  name: string
  kind: AgentCommandPublishedOutputKind
  readPath: string
  mimeType: string
  sizeBytes: number
  sha256: string
  width?: number
  height?: number
}

export interface AgentCommandExecutionResult {
  outputs?: AgentCommandPublishedOutput[]
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

export interface AgentDiffProposal {
  id: string
  operation: AgentPatchOperation
  filePath: string
  patch: string
  baseRevision: string | null
  summary: string | null
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
  baseRevision: string | null
  summary: string | null
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
  /** Exact root authority for an authorized read-only child observer. */
  observerRootConversationId?: string
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

export const AGENT_COMMAND_ARTIFACT_OBSERVATION_SCHEMA_VERSION = 3

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
  /** False only when both snapshots and the bounded change report are complete. */
  partial: boolean
  /** Stable backend reason codes explaining incomplete observation evidence. */
  stopReasons: string[]
  /** Office files considered across the before and after snapshots. */
  scanned: number
  /** Change records included in this observation. */
  returned: number
  /** Known change records omitted from the bounded report. */
  omitted: number
  coverage: AgentCommandArtifactObservationCoverage
  changes: AgentCommandArtifactChange[]
  changesTruncated: boolean
  changesOmitted: number
  expectedOutputs: AgentCommandExpectedArtifactOutcome[]
  warnings: AgentCommandArtifactObservationWarning[]
}

export type AgentCommandRuntimeKind = 'node' | 'python'

/**
 * Host-owned reproducible runtime identity persisted in approvals and evidence.
 * `pdf` is bound only by the trusted built-in PDF Skill and is intentionally absent from the
 * model-visible run_command.runtimeProfile enum.
 */
export type AgentCommandRuntimeProfile = 'documents' | 'spreadsheets' | 'presentations' | 'pdf'

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

export type AgentFileInputRef =
  | { type: 'attachment'; readPath: string }
  | { type: 'workspace'; path: string }
  | { type: 'external'; path: string }
  | { type: 'generated_artifact'; uri: string; path: string }
  | { type: 'skill_resource'; uri: string }

export interface AgentFileInputSpec {
  mountPath: string
  source: AgentFileInputRef
}

export interface AgentFileInputBinding {
  schemaVersion: 1
  mountPath: string
  source: AgentFileInputRef
  sizeBytes: number
  sha256: string
}

/** Renderer-safe projection of a durable command action; Host authority is deliberately absent. */
export interface AgentCommandActionProjection {
  id: string
  command: string
  cwd: string | null
  timeoutMs: number | null
  approvalStatus: AgentApprovalStatus
  riskLevel: AgentCommandRiskLevel | null
  reason: string | null
  observe: AgentCommandArtifactObservationRequest | null
}

export type AgentSkillMaterializationResultStatus =
  'applied' | 'already_applied' | 'conflict' | 'rejected' | 'failed'

export interface AgentSkillMaterializationRequest {
  id: string
  sourceUri: string
  sourcePrefix: string | null
  destination: string
  approvalStatus: AgentApprovalStatus
  reason: string | null
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
  timeoutMs: number | null
  approvalStatus: AgentApprovalStatus
  reason: string | null
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

export interface OfficePresentationRenderPlan {
  requestedPages: number[]
  slideWidthEmu: number
  slideHeightEmu: number
  viewport: OfficeViewport
  grid: OfficeGridLayout | null
}

export interface OfficeRenderGridGeometry {
  columns: number
  rows: number
  viewportWidth: number
  viewportHeight: number
  contentWidth: number
  contentHeight: number
}

/**
 * Host-verified renderer geometry only. This proves that the frozen requested
 * slide set fits the decoded PNG viewport under the trusted layout formula; it
 * is not visual-content evidence and does not replace per-slide inspection.
 */
export interface OfficeRenderLayoutCoverage {
  requestedPages: number[]
  evidence: 'trustedRendererGeometry'
  grid?: OfficeRenderGridGeometry
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
  documentPath: string | null
  outputPath: string | null
  destinationPath: string | null
  inputs: AgentFileInputSpec[]
  timeoutMs: number | null
}

export type OfficeExecutionRequest = OfficeExecutionRequestBase & {
  parameters: OfficeOperationParameters
}

export type OfficeFilePreconditionState = 'missing' | 'present'

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
  device: number | null
  inode: number | null
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
  objectIdentity: OfficePathIdentity | null
  parentIdentity: OfficePathIdentity
  contentRevision: string | null
  size: number | null
  writeDisposition: OfficeWriteDisposition | null
}

/** Immutable, shell-free Office execution snapshot prepared by the trusted backend. */
export interface OfficePreparedExecution {
  schemaVersion: number
  providerId: string
  engineRevision: string
  workspaceRevision: string | null
  access: OfficeOperationAccess
  request: OfficeExecutionRequest
  argv: string[]
  /** Host-owned deterministic presentation render plan; null for other operations. */
  resolvedRenderPlan: OfficePresentationRenderPlan | null
  paths: OfficeFrozenPath[]
  inputBindings: AgentFileInputBinding[]
}

export interface AgentOfficeOperationRequest {
  schemaVersion: number
  id: string
  /** Exact, model-facing Office Tool arguments frozen by the Host. */
  semanticArgs: Record<string, unknown>
  prepared: OfficePreparedExecution
  approvalStatus: AgentApprovalStatus
  reason: string
}

export interface AgentSkillInstallationResourceSummary {
  total: number
  references: number
  assets: number
  scripts: number
  bytes: number
}

export interface AgentSkillInstallationWarning {
  code: string
  message: string
  requiresAcknowledgement: boolean
}

export interface AgentSkillInstallationPreview {
  name: string
  description: string
  sourceSummary: unknown
  resolvedRevision: string
  fileCount: number
  totalBytes: number
  resourceSummary: AgentSkillInstallationResourceSummary
  containsScripts: boolean
  warnings: AgentSkillInstallationWarning[]
  compatibility: string
  operation: string
  impact: string
}

export interface AgentSkillInstallationRequest {
  schemaVersion: number
  id: string
  installRef: string
  preview: AgentSkillInstallationPreview
  approvalStatus: AgentApprovalStatus
  expiresAt: number
}

/**
 * Renderer-safe projection of one exact request to activate a Host-owned capability.
 *
 * The Host deliberately excludes managed Server, transport, executable, credential, manifest
 * contents and reviewed Tool lists. Renderer must treat every string as untrusted plain text.
 */
export interface AgentBuiltinCapabilityActivationApproval {
  actionId: string
  activationId: string
  runId: string
  callId: string
  capabilityId: McpBuiltinCapabilityId
  displayName: string
  reason: string
  manifestDigest: string
  policyRevision: number
  /** Unix timestamp in seconds. */
  createdAt: number
  /** Unix timestamp in seconds. */
  expiresAt: number
  approvalStatus: AgentApprovalStatus
}

/** Host-classified address class for one frozen browser destination. */
export type AgentBrowserAddressClass =
  'public' | 'loopback' | 'private' | 'link_local' | 'cloud_metadata' | 'unresolved'

/** Risks that can require a task-scoped decision without weakening the Host boundary. */
export type AgentBrowserRiskKind =
  | 'insecure_http'
  | 'localhost'
  | 'loopback'
  | 'private_network'
  | 'link_local'
  | 'cloud_metadata'
  | 'non_standard_port'
  | 'url_userinfo'
  | 'dns_private_resolution'
  | 'risk_escalation'
  | 'new_window'
  | 'file_upload'
  | 'file_download'
  | 'local_service_request'

export type AgentBrowserRiskTrigger =
  'tool_argument' | 'main_frame' | 'redirect' | 'new_window' | 'subresource' | 'upload' | 'download'

export type AgentBrowserReviewedToolName =
  | 'browser_navigate'
  | 'browser_snapshot'
  | 'browser_find'
  | 'browser_click'
  | 'browser_type'
  | 'browser_fill_form'
  | 'browser_press_key'
  | 'browser_tabs'
  | 'browser_wait_for'
  | 'browser_close'

/** Renderer-safe identity for the exact destination frozen by the Host. */
export interface AgentBrowserDestinationIdentity {
  normalizedUrl: string
  origin: string
  scheme: 'http' | 'https'
  asciiHost: string
  effectivePort: number
  addressClass: AgentBrowserAddressClass
}

/**
 * Renderer-safe projection of one exact browser boundary decision.
 *
 * Capability grants, request headers, credentials, cookies, target/debugger identifiers and raw
 * network data are deliberately absent. Every string remains untrusted plain text.
 */
export interface AgentBrowserRiskApproval {
  schemaVersion: 1
  actionId: string
  riskApprovalId: string
  runId: string
  callId: string
  capabilityId: McpBuiltinCapabilityId
  capabilityActivationId: string
  displayName: string
  reason: string
  destination: AgentBrowserDestinationIdentity
  trigger: AgentBrowserRiskTrigger
  triggerToolName: AgentBrowserReviewedToolName
  riskKinds: AgentBrowserRiskKind[]
  manifestDigest: string
  policyRevision: number
  /** Unix timestamp in seconds. */
  createdAt: number
  /** Unix timestamp in seconds. */
  expiresAt: number
  approvalStatus: AgentApprovalStatus
}

export type AgentBuiltinMcpToolRiskKind =
  | 'file_read'
  | 'file_write'
  | 'file_upload'
  | 'file_download'
  | 'cookie_read'
  | 'cookie_write'
  | 'local_storage_read'
  | 'local_storage_write'
  | 'session_storage_read'
  | 'session_storage_write'
  | 'storage_state_import'
  | 'storage_state_export'
  | 'network_sensitive_read'
  | 'page_script_execution'
  | 'unsafe_code_execution'

export interface AgentBuiltinMcpToolResourceSummary {
  scope: string
  displayName: string
  fileBasenames: string[]
  origin: string | null
}

export interface AgentBuiltinMcpToolApprovalIdentity {
  actionId: string
  approvalId: string
  runId: string
  callId: string
  capabilityId: McpBuiltinCapabilityId
  capabilityActivationId: string
  managedMcpId: string
  packageName: string
  packageVersion: string
  upstreamCatalogDigest: string
  manifestDigest: string
  policyDigest: string
  policyRevision: number
  toolId: string
  rawName: string
  modelName: string
  upstreamSchemaDigest: string
  hostOverlayDigest: string
  hostInputSchemaDigest: string
  argumentsDigest: string
  resourceScopeDigest: string
  origin: string | null
}

/** Renderer-safe, value-free projection of one exact built-in MCP sensitive Tool decision. */
export interface AgentBuiltinMcpToolApproval {
  schemaVersion: 1
  identity: AgentBuiltinMcpToolApprovalIdentity
  capabilityDisplayName: string
  toolDisplayName: string
  callReason: string
  operationCategory: string
  resourceSummary: AgentBuiltinMcpToolResourceSummary
  riskKinds: AgentBuiltinMcpToolRiskKind[]
  /** Unix timestamp in seconds. */
  createdAt: number
  /** Unix timestamp in seconds. */
  expiresAt: number
  approvalStatus: AgentApprovalStatus
}

export type AgentProposedAction =
  | { type: 'tool_call'; call: AgentToolCall }
  | { type: 'mcp_tool_call'; approval: AgentMcpToolApproval }
  | {
      type: 'builtin_capability_activation'
      approval: AgentBuiltinCapabilityActivationApproval
    }
  | { type: 'builtin_mcp_tool_approval'; approval: AgentBuiltinMcpToolApproval }
  | { type: 'browser_risk_approval'; approval: AgentBrowserRiskApproval }
  | { type: 'diff'; diff: AgentDiffProposal }
  | { type: 'file_write'; fileWrite: AgentFileWriteProposal }
  | { type: 'command'; command: AgentCommandActionProjection }
  | { type: 'skill_materialization'; materialization: AgentSkillMaterializationRequest }
  | { type: 'skill_script'; script: AgentSkillScriptRequest }
  | { type: 'office_operation'; officeOperation: AgentOfficeOperationRequest }
  | { type: 'skill_installation'; installation: AgentSkillInstallationRequest }

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
  | {
      type: 'message_stream_committed'
      runId: string
      streamId: string
      traceSequence: number | null
    }
  | {
      type: 'llm_retry'
      runId: string
      streamId: string
      category: AgentLlmRetryCategory
      providerCode?: string
      delayMs: number
      retryAt: number
      attempt: number
      maxAttempts: number
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
  | {
      type: 'tool_call'
      runId: string
      traceSequence: number
      call: AgentToolCall
      /** Host-authoritative Tool identity; never infer provenance from `call.tool`. */
      identity: AgentToolIdentity
    }
  | { type: 'tool_result'; runId: string; result: AgentToolResult }
  | {
      type: 'mcp_tool_invocation_state_changed'
      runId: string
      invocation: AgentMcpToolInvocationEvent
    }
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
  | {
      type: 'context_compaction_started'
      runId: string
      operationId: string
      traceSequence: number
    }
  | {
      type: 'context_compaction_finished'
      runId: string
      operationId: string
      outcome: AgentContextCompactionEventOutcome
      traceSequence: number
    }
  | { type: 'approval_required'; runId: string; action: AgentProposedAction }
  | { type: 'diff'; runId: string; diff: AgentDiffProposal }
  | {
      type: 'command_started'
      runId: string
      conversationId: string
      assistantMessageId: string
      projectId?: string
      callId: string
      sessionId: string
      startedAt: number
    }
  | {
      type: 'command_output'
      runId: string
      conversationId: string
      assistantMessageId: string
      projectId?: string
      callId: string
      sessionId: string
      sequence: number
      stream: AgentCommandOutputStream
      output: string
    }
  | {
      type: 'command_exited'
      runId: string
      conversationId: string
      assistantMessageId: string
      projectId?: string
      callId: string
      sessionId: string
      status: Extract<
        AgentCommandSessionStatus,
        'exited' | 'timed_out' | 'failed' | 'outcome_unknown'
      >
      exitCode?: number
      endedAt: number
      latestSequence: number
      outputTruncated: boolean
      outputs?: AgentCommandPublishedOutput[]
      artifactObservation?: AgentCommandArtifactObservation
    }
  | {
      type: 'command_interrupted'
      runId: string
      conversationId: string
      assistantMessageId: string
      projectId?: string
      callId: string
      sessionId: string
      endedAt: number
      latestSequence: number
      outputTruncated: boolean
      outputs?: AgentCommandPublishedOutput[]
      artifactObservation?: AgentCommandArtifactObservation
    }
  | {
      type: 'error'
      runId?: string
      traceSequence: number | null
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
