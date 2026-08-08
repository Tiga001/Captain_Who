import type {
  AgentCommandOutputStream,
  AgentCommandSessionStatus,
  AgentDiffProposal,
  AgentContextCompactionEventOutcome,
  AgentFileDraftSnapshot,
  AgentFileWritePreview,
  AgentInputAttachment,
  AgentMcpDispatchCertainty,
  AgentMcpServerScope,
  AgentMcpToolInvocationOutcome,
  AgentMcpToolInvocationState,
  AgentProposedAction,
  AgentRunStatus,
  AgentSkillInstallationRequest,
  AgentStateSnapshot,
  AgentTodoState,
  AgentToolCall,
  AgentToolDefinition,
  AgentToolResult,
  AgentUsage,
  ActivatedSkillSummary,
  SkillSelection
} from '@mycopilot/protocol'

export type ChatSkillInstallationStatus =
  | 'waiting_for_approval'
  | 'installing'
  | 'installed'
  | 'already_installed'
  | 'rejected'
  | 'failed'
  | 'uncertain'

export interface ChatSkillInstallationView {
  action: AgentSkillInstallationRequest
  status: ChatSkillInstallationStatus
}

export interface ChatWebSearchSource {
  id: string
  title: string
  url: string
  displayUrl: string
  domain: string
  faviconUrl?: string
  snippet?: string
  score?: number
  publishedDate?: string
}

export interface ChatWebSearchActivity {
  callId: string
  kind?: 'search' | 'fetch'
  query: string
  provider: string
  status: 'running' | 'completed' | 'failed' | 'cancelled'
  sources: ChatWebSearchSource[]
  answer?: string
  summaryQuality?: 'good' | 'low'
  error?: string
  responseTime?: number | string | null
  truncated?: boolean
  updatedAt: number
}

export type ChatReadActivityKind =
  'file' | 'image' | 'pdf' | 'word' | 'presentation' | 'spreadsheet'

export type ChatFileWritePreview = Omit<
  AgentFileWritePreview,
  'contentDelta' | 'contentOffsetBytes'
> & {
  content: string
  receivedAt: number
}

export interface ChatReadActivity {
  callId: string
  tool: AgentToolCall['tool']
  kind: ChatReadActivityKind
  status: 'running' | 'completed' | 'failed' | 'cancelled'
  path: string
  fileName: string
  extension?: string
  mimeType?: string
  thumbnailDataUrl?: string
  fullDataUrl?: string
  error?: string
  updatedAt: number
}

export interface ChatGuidanceTimelineItem {
  id: string
  type: 'user_guidance'
  guidanceId?: string
  clientMessageId: string
  content: string
  attachments: ChatMessageAttachment[]
  status: 'submitting' | 'queued' | 'applied' | 'rejected'
  rejectionCode?: string
  error?: string
  recoverable?: boolean
  createdAt: number
  sequence?: number
}

export interface ChatCommandOutputChunk {
  sequence: number
  stream: AgentCommandOutputStream
  output: string
}

export interface ChatCommandOutputPreview {
  callId: string
  chunks: ChatCommandOutputChunk[]
}

/**
 * Renderer projection of one managed process. Approval remains authoritative on the Tool Call;
 * this view starts only after approval has settled and describes the independent process state.
 */
export interface ChatCommandSessionView {
  callId: string
  sessionId?: string
  status: AgentCommandSessionStatus
  startedAt?: number
  endedAt?: number
  exitCode?: number
  latestSequence: number
  outputTruncated: boolean
}

/**
 * Renderer-owned, allowlisted projection of an MCP invocation.
 *
 * Keep this separate from AgentToolCall/AgentToolResult: those generic DTOs can contain arguments
 * and result bodies, while MCP activity is intentionally limited to an allowlisted lifecycle
 * projection. The bounded model-authored display reason remains untrusted and must be rendered
 * only as plain text.
 */
export interface ChatMcpToolInvocationView {
  actionId: string
  invocationId: string
  callId: string
  serverId: string
  serverDisplayName: string
  scope?: AgentMcpServerScope
  rawToolName: string
  modelToolName: string
  /**
   * Bounded model-authored explanation for the call. Never derived from raw MCP arguments; render
   * as plain text and do not assume it is secret-redacted.
   */
  displayReason?: string
  external: true
  state: AgentMcpToolInvocationState
  dispatchCertainty: AgentMcpDispatchCertainty
  outcome?: AgentMcpToolInvocationOutcome
  isError?: boolean
  errorCode?: string
  /** Current-process user guidance attached to an explicit rejection. Never sourced from MCP. */
  rejectionReason?: string
  durationMs?: number
  outputTruncated: boolean
}

export type ChatAgentTimelineItem =
  | { id: string; type: 'message'; content: string; streamId?: string }
  | ChatGuidanceTimelineItem
  | { id: string; type: 'tool_call'; callId: string }
  | { id: string; type: 'mcp_tool_call'; invocationId: string }
  | {
      id: string
      type: 'context_compaction'
      operationId: string
      status: 'running' | AgentContextCompactionEventOutcome
    }
  | { id: string; type: 'error'; message: string }

export interface ChatAgentRunView {
  runId: string | null
  status: AgentRunStatus | 'starting'
  startedAt?: number
  firstResponseAt?: number
  lastResponseAt?: number
  completedAt?: number
  toolDefinitions: AgentToolDefinition[]
  /** Backend-authoritative identity of the effective Tool contract last shown to the model. */
  toolSetRevision?: {
    stable: string
    dynamic: string
    effective: string
  }
  todo?: AgentTodoState
  toolCalls: AgentToolCall[]
  toolResults: AgentToolResult[]
  webSearchActivities?: ChatWebSearchActivity[]
  readActivities?: ChatReadActivity[]
  approvals: AgentProposedAction[]
  /** Presentation-safe approval history retained after settlement for the chat Timeline. */
  skillInstallations?: ChatSkillInstallationView[]
  diffs: AgentDiffProposal[]
  fileDrafts?: AgentFileDraftSnapshot[]
  fileWritePreviews?: ChatFileWritePreview[]
  /** Ephemeral live process output. Final ToolResults remain the durable source of truth. */
  commandOutputPreviews?: Record<string, ChatCommandOutputPreview>
  /**
   * Managed process projection keyed by the original run_command call id. Active entries are
   * Host-authoritative and ephemeral; immutable terminal metadata is durable Timeline state.
   */
  commandSessions?: Record<string, ChatCommandSessionView>
  /** Safe lifecycle-only MCP views. Never store MCP arguments or result bodies here. */
  mcpInvocations?: ChatMcpToolInvocationView[]
  messageStreamCheckpoints?: Record<string, { baseContentLength: number; baseWasThinking: boolean }>
  timeline: ChatAgentTimelineItem[]
  state?: AgentStateSnapshot
  error?: string
  usage?: AgentUsage
  finishReason?: string
  activatedSkills?: ActivatedSkillSummary[]
  skillActivationRevision?: string
  /** User-selected inputs for this run; excludes Skills activated later by the model. */
  explicitSkillSelections?: SkillSelection[]
}

export interface ChatMessageUiState {
  favorited?: boolean
  timelineCollapsed?: boolean
}

export interface ChatMessageAttachment {
  id: string
  kind: 'file' | 'image'
  name: string
  mimeType?: string | null
  sizeBytes: number
  encoding?: AgentInputAttachment['encoding']
  data?: string
  previewData?: string | null
  previewMimeType?: string | null
  createdAt?: number
}

export interface ChatMessage {
  id: string
  role: 'user' | 'assistant'
  content: string
  createdAt: number
  status?: 'pending' | 'sent' | 'error'
  attachments?: ChatMessageAttachment[]
  agentRun?: ChatAgentRunView
  uiState?: ChatMessageUiState
}

export type ChatPermissionMode = 'default' | 'full' | 'custom'

export interface ChatQueuedMessage {
  id: string
  clientMessageId: string
  content: string
  attachments: AgentInputAttachment[]
  modelId: string
  permissionMode: ChatPermissionMode
  projectId: string | null
  skills: SkillSelection[]
  status: 'pending' | 'submitting' | 'error'
  error?: string
  createdAt: number
}

export interface ChatComposerDraft {
  message: string
  permissionMode: ChatPermissionMode
  modelId: string
  projectId: string | null
  attachments: AgentInputAttachment[]
  skills: SkillSelection[]
  queuedMessages: ChatQueuedMessage[]
  updatedAt: number
}

export interface ChatSubmitOptions {
  modelId: string
  permissionMode: ChatPermissionMode
  projectId: string | null
  attachments?: AgentInputAttachment[]
  skills: SkillSelection[]
}

export interface ChatConversationContinuationOrigin {
  sourceConversationId: string
  sourceMessageId: string
  boundaryMessageId: string
}

export interface ChatConversation {
  id: string
  projectId: string | null
  modelId: string | null
  title: string
  messages: ChatMessage[]
  /** False only for sidebar metadata whose message history has not been requested yet. */
  messagesLoaded?: boolean
  createdAt: number
  updatedAt: number
  pinnedAt?: number | null
  archivedAt?: number | null
  unreadAt?: number | null
  continuationOrigin?: ChatConversationContinuationOrigin | null
}
