import type { McpBuiltinCapabilityId } from '../mcp/contracts'
import type { AgentBuiltinExecutionPermission } from './approvals'
import type {
  AgentCommandPermission,
  AgentCommandSafetyPolicy,
  AgentCommandSessionStatus
} from './command'
import type { AgentInputAttachmentKind, AgentToolCall } from './conversation'
import type { AgentFolderReference } from '../attachments'
import type { AgentContextCompactionEventOutcome } from './events'

/**
 * Agent JSON-RPC names are transport contract, not host implementation details.
 * Rust verifies the same values against the shared Agent golden fixture.
 */
export const AGENT_CANCEL_RUN_METHOD = 'agent.cancelRun'

export const AGENT_STEER_RUN_METHOD = 'agent.steerRun'

export const AGENT_START_CONVERSATION_TURN_METHOD = 'agent.startConversationTurn'

export const AGENT_REWRITE_CONVERSATION_TURN_METHOD = 'agent.rewriteConversationTurn'

export const AGENT_GET_CONTEXT_WINDOW_SNAPSHOT_METHOD = 'agent.getContextWindowSnapshot'

export const AGENT_LIST_PENDING_ACTIONS_METHOD = 'agent.listPendingActions'

export const AGENT_APPROVE_ACTION_METHOD = 'agent.approveAction'

export const AGENT_REJECT_ACTION_METHOD = 'agent.rejectAction'

export const AGENT_CANCEL_ACTION_METHOD = 'agent.cancelAction'

export const AGENT_GET_USAGE_SUMMARY_METHOD = 'agent.getUsageSummary'

export const AGENT_CLEAR_USAGE_RECORDS_METHOD = 'agent.clearUsageRecords'

export type AgentMessageRole = 'system' | 'user' | 'assistant'

export type AgentRunStatus =
  | 'idle'
  | 'queued'
  | 'running'
  | 'waiting_for_approval'
  | 'waiting_for_user_input'
  | 'completed'
  | 'failed'
  | 'cancelled'

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
  builtinExecution: AgentBuiltinExecutionPermission
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
  | 'todo_update'
  | 'apply_patch'
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

/** Immutable Host-owned image reference; never carries image bytes or local paths. */
export interface ConversationContextImageRef {
  attachmentId: string
  mimeType: string
  sha256: string
}

export type ConversationContextMaterialKind =
  'input_attachment' | 'skill_instructions' | 'run_world_state'

export type ConversationTurnTraceItem =
  | {
      type: 'context_material'
      sequence: number
      eventId: string
      materialKind: ConversationContextMaterialKind
      content: string
      images?: ConversationContextImageRef[]
      createdAt: number
    }
  | {
      type: 'backend_state'
      sequence: number
      eventId: string
      content: string
      createdAt: number
      placement: 'timeline' | 'after_message'
    }
  | {
      type: 'assistant_narration'
      sequence: number
      content: string
      providerTurnId?: string
      firstToolCallId?: string
      truncated: boolean
    }
  | {
      type: 'user_guidance'
      sequence: number
      guidanceId: string
      clientMessageId: string
      content: string
      attachments: ConversationTraceAttachment[]
      folderReferences?: AgentFolderReference[]
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
