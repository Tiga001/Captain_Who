import type {
  HumanInteractionResponseDisplay,
  AgentCommandArtifactObservation,
  AgentCommandOutputStream,
  AgentCommandPublishedOutput,
  AgentCommandSessionStatus,
  AgentFileChangeProposal,
  AgentContextCompactionEventOutcome,
  AgentFileChangePreview,
  AgentFileChangeSnapshot,
  AgentFolderReference,
  AgentInputAttachment,
  AgentObserverInputOrigin,
  AgentLlmRetryCategory,
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
  AgentToolIdentity,
  AgentToolResult,
  AgentUsage,
  ActivatedSkillSummary,
  SkillSelection
} from '@mycopilot/protocol'
import type { CollaborationTimelineActivity } from '../agentCollaboration/collaborationTimelineModel'

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

export type ChatReadActivityKind = 'file' | 'image' | 'word' | 'presentation' | 'spreadsheet'

export type ChatFileChangePreview = Omit<
  AgentFileChangePreview,
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
  folderReferences?: AgentFolderReference[]
  status: 'submitting' | 'queued' | 'applied' | 'rejected'
  rejectionCode?: string
  error?: string
  recoverable?: boolean
  createdAt: number
  sequence?: number
  traceSequence?: number
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
  outputs?: AgentCommandPublishedOutput[]
  artifactObservation?: AgentCommandArtifactObservation
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

export type ChatAgentTimelineItem = (
  | { id: string; type: 'message'; content: string; streamId?: string }
  | ChatGuidanceTimelineItem
  | { id: string; type: 'tool_call'; callId: string; identity?: AgentToolIdentity }
  | { id: string; type: 'mcp_tool_call'; invocationId: string }
  | {
      id: string
      type: 'context_compaction'
      operationId: string
      status: 'running' | AgentContextCompactionEventOutcome
    }
  | { id: string; type: 'error'; message: string }
) & {
  /** Durable Conversation trace order. Live-only presentation items may omit it. */
  traceSequence?: number
}

export type ChatAgentInterruptionReason =
  | 'service_connection_failed'
  | 'service_unavailable'
  | 'authentication_failed'
  | 'quota_exhausted'
  | 'context_limit_exceeded'
  | 'request_rejected'
  | 'response_invalid'
  | 'output_limit_reached'
  | 'empty_response'
  | 'stream_interrupted'
  | 'request_failed'
  | 'admission_unconfirmed'

export interface ChatAgentInterruptionView {
  reason: ChatAgentInterruptionReason
}

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
  fileChangeProposals: AgentFileChangeProposal[]
  fileChanges?: AgentFileChangeSnapshot[]
  fileChangePreviews?: ChatFileChangePreview[]
  /** Ephemeral live process output. Final ToolResults remain the durable source of truth. */
  commandOutputPreviews?: Record<string, ChatCommandOutputPreview>
  /**
   * Managed process projection keyed by the original run_command call id. Active entries are
   * Host-authoritative and ephemeral; immutable terminal metadata is durable Timeline state.
   */
  commandSessions?: Record<string, ChatCommandSessionView>
  /** Safe lifecycle-only MCP views. Never store MCP arguments or result bodies here. */
  mcpInvocations?: ChatMcpToolInvocationView[]
  /** Host-frozen collaboration rows owned by this terminal Assistant response. */
  collaborationTimelineActivities?: CollaborationTimelineActivity[]
  /** Previous committed answer projection, retained only while the next model stream is provisional. */
  messageStreamCheckpoints?: Record<string, { previousContent: string }>
  /** Ephemeral retry status. Cleared by the next model output or a terminal boundary. */
  llmRetry?: {
    category: AgentLlmRetryCategory
    providerCode?: string
    delayMs: number
    retryAt: number
    attempt: number
    maxAttempts: number
  }
  timeline: ChatAgentTimelineItem[]
  state?: AgentStateSnapshot
  /** Safe terminal reason for a failed provisional model request; excludes diagnostics. */
  interruption?: ChatAgentInterruptionView
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
  /** Renderer-only view projection, never saved or sent as model input. */
  humanInteractionDisplay?: HumanInteractionResponseDisplay
  id: string
  role: 'user' | 'assistant'
  content: string
  createdAt: number
  status?: 'pending' | 'sent' | 'error'
  attachments?: ChatMessageAttachment[]
  folderReferences?: AgentFolderReference[]
  agentRun?: ChatAgentRunView
  uiState?: ChatMessageUiState
  /**
   * Durable transport/audit identity for model-facing user inputs. Root conversations created
   * before collaboration do not carry this field and continue to render as human messages.
   */
  inputOrigin?: AgentObserverInputOrigin
}

export type ChatPermissionMode = 'default' | 'full' | 'custom'

export interface ChatQueuedMessage {
  id: string
  clientMessageId: string
  content: string
  attachments: AgentInputAttachment[]
  folderReferences?: AgentFolderReference[]
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
  folderReferences?: AgentFolderReference[]
  skills: SkillSelection[]
  queuedMessages: ChatQueuedMessage[]
  updatedAt: number
}

export interface ChatSubmitOptions {
  /** Renderer-only snapshot; never forwarded to the Agent transport. */
  draftSnapshot?: ChatComposerDraft
  modelId: string
  permissionMode: ChatPermissionMode
  projectId: string | null
  attachments?: AgentInputAttachment[]
  folderReferences?: AgentFolderReference[]
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
  /** Renderer-only write fence; storage maps it to archivedAt, but navigation waits for readback. */
  pendingArchivedAt?: number
  unreadAt?: number | null
  continuationOrigin?: ChatConversationContinuationOrigin | null
}
