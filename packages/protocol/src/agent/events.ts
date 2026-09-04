import type { ActivatedSkillSummary } from '../skills'
import type { AgentProposedAction } from './approvals'
import type {
  AgentCommandArtifactObservation,
  AgentCommandOutputStream,
  AgentCommandPublishedOutput,
  AgentCommandSessionStatus
} from './command'
import type {
  AgentContextWindowSnapshot,
  AgentStateSnapshot,
  AgentSteerRunRejectionCode,
  AgentTodoState,
  AgentToolCall,
  AgentToolDefinition,
  AgentToolResult,
  AgentUsage
} from './conversation'
import type {
  AgentLlmRetryCategory,
  AgentMcpToolInvocationEvent,
  AgentRunStatus,
  AgentToolIdentity,
  ConversationTraceAttachment
} from './core'
import type {
  AgentFileChangePreview,
  AgentFileChangeProposal,
  AgentFileChangeSnapshot
} from './fileChange'

export const AGENT_EVENT_NOTIFICATION_METHOD = 'agent.event'

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
      type: 'file_change_preview_updated'
      runId: string
      preview: AgentFileChangePreview
    }
  | {
      type: 'file_change_preview_cleared'
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
  | { type: 'file_change_updated'; runId: string; fileChange: AgentFileChangeSnapshot }
  | {
      type: 'context_window_updated'
      runId: string
      conversationId?: string
      /** Stable local configuration identity; never infer it from `snapshot.model`. */
      modelConfigId: string
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
  | { type: 'file_change_proposed'; runId: string; fileChange: AgentFileChangeProposal }
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
