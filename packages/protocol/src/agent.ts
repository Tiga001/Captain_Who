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
 * auto_approve only skips the prompt; backend command validation and dangerous-command blocking
 * still apply.
 */
export type AgentCommandPermission = 'require_approval' | 'auto_approve'

/**
 * Controls whether file-edit proposals from apply_patch and write_file require a human click.
 * auto_approve only skips the prompt; backend path, symlink, binary, and revision checks still apply.
 */
export type AgentPatchPermission = 'require_approval' | 'auto_approve'

export interface AgentPermissions {
  read: AgentReadPermission
  write: AgentWritePermission
  command: AgentCommandPermission
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
  | (string & {})

export type AgentToolSafety = 'read_only' | 'requires_approval' | 'destructive'

export type AgentToolApprovalMode = 'never' | 'always' | 'dynamic'

export type AgentApprovalStatus = 'not_required' | 'required' | 'approved' | 'rejected'

export type AgentApprovalDecisionStatus = 'approved' | 'rejected'

export type ConversationTurnTraceTerminalStatus = 'completed' | 'failed' | 'cancelled'

export type ConversationTraceToolResultStatus =
  'succeeded' | 'failed' | 'rejected' | 'conflict' | 'cancelled'

export type ConversationTurnTraceItem =
  | {
      type: 'assistant_narration'
      sequence: number
      content: string
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

export interface AgentChatMessage {
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
  availableInputTokens?: number
  /** Fixed request costs plus context retained for later conversation turns. */
  persistentInputTokens: number
  remainingInputTokens?: number
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
  providerPath?: string
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
}

export interface AgentContextWindowSnapshotInput {
  conversationId?: string
  projectId?: string | null
  modelId: string
  maxTokens?: number
  promptPreferences?: AgentPromptPreferences
  permissions?: AgentPermissions
}

export interface AgentContextWindowSnapshotOutput {
  snapshot?: AgentContextWindowSnapshot
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
}

export interface AgentCancelRunRequest {
  runId: string
}

export interface AgentCancelRunResponse {
  runId: string
  cancelled: boolean
}

export interface AgentActionIdRequest {
  actionId: string
}

export interface AgentRejectActionRequest {
  actionId: string
  message?: string
}

export type PendingAgentActionStatus =
  'pending' | 'approved' | 'rejected' | 'cancelled' | 'completed' | 'failed'

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

export interface AgentCommandRequest {
  id: string
  command: string
  cwd?: string
  timeoutMs?: number
  approvalStatus: AgentApprovalStatus
  riskLevel?: AgentCommandRiskLevel
  reason?: string
}

export type AgentProposedAction =
  | { type: 'tool_call'; call: AgentToolCall }
  | { type: 'diff'; diff: AgentDiffProposal }
  | { type: 'file_write'; fileWrite: AgentFileWriteProposal }
  | { type: 'command'; command: AgentCommandRequest }

export type AgentEvent =
  | { type: 'started'; runId: string; toolDefinitions: AgentToolDefinition[] }
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
  | { type: 'tool_call'; runId: string; call: AgentToolCall }
  | { type: 'tool_result'; runId: string; result: AgentToolResult }
  | { type: 'todo_updated'; runId: string; todo: AgentTodoState }
  | { type: 'file_draft_updated'; runId: string; draft: AgentFileDraftSnapshot }
  | {
      type: 'context_window_updated'
      runId: string
      conversationId?: string
      snapshot: AgentContextWindowSnapshot
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
