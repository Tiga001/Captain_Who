import {
  type AgentActionExecutionOutput,
  type AgentApprovalStatus,
  type AgentChatOutput,
  type AgentEvent,
  type AgentProposedAction,
  type AgentToolResult
} from '@mycopilot/protocol'
import type {
  ChatAgentInterruptionView,
  ChatAgentRunView,
  ChatGuidanceTimelineItem,
  ChatMessage
} from '../chat/chatTypes'
import {
  normalizeReadActivities,
  settlePendingReadActivities,
  upsertReadActivityFromCall,
  upsertReadActivityFromResult
} from '../chat/agentReadActivities'
import {
  normalizeWebSearchActivities,
  settlePendingWebSearchActivities,
  upsertWebSearchActivityFromCall,
  upsertWebSearchActivityFromResult
} from '../chat/agentWebSearch'
import { mergeActivatedSkillSummaries } from '../skills/activatedSkillInventory'
import { parseManagedCommandOutputs } from '../chat/managedCommandOutputs'
import { getAgentActionId } from './agentActionUtils'
import { projectBuiltinCapabilityToolResult } from './builtinCapabilityResultProjection'
import { getActionToolCall, getActionToolCallId } from './actionProjection'
import {
  createRejectedToolResult,
  getApprovalsForStatus,
  updateFileChangeApprovalStatus,
  updateFileChangeFromToolResult,
  updateFileChangeProposalApprovalStatus,
  updateToolCallApprovalStatus
} from './approvalState'
import {
  appendMessageToTimeline,
  appendMessageDeltaToTimeline,
  getFinalMessageContent,
  getFinalTimeline,
  getMessageContentAfterDelta,
  getRunResponseTimestamps,
  removeTransientToolTimelineItems,
  upsertMcpInvocationTimelineItem
} from './messageTimeline'
import {
  appendCommandOutputChunk,
  hasRunCommandCall,
  isTerminalCommandSessionStatus,
  runningCommandReceipt,
  settleCommandApproval,
  terminalCommandStatusFromToolResult,
  withCommandSession
} from './agentEventReducerCommand'
import {
  appendToolCallToTimeline,
  finishContextCompaction,
  settlePendingContextCompactions,
  startContextCompaction
} from './agentEventReducerCompaction'
import {
  applySkillInstallationDecision,
  applySkillInstallationExecution,
  bindFileChangePreviewsToCall,
  mergeSkillInstallationApprovals,
  removeAgentAction,
  upsertAgentAction,
  upsertFileChangePreview
} from './agentEventReducerFileSkill'
import {
  guidanceAttachments,
  guidanceTimelineId,
  removeGuidanceFromChatMessage,
  upsertGuidanceTimelineItem
} from './agentEventReducerGuidance'
import {
  addMcpApprovalViews,
  projectMcpInvocationEvent,
  projectMcpRejectionReason,
  settlePendingMcpInvocations,
  upsertMcpInvocationView
} from './agentEventReducerMcp'
import {
  appendTimelineItem,
  ensureAgentRun,
  getSettledActivityStatus,
  isCompletedAgentRunStatus,
  isFinishedAgentOutputStatus,
  isSuspendedAgentRunStatus,
  upsertById
} from './agentEventReducerShared'

export { ensureAgentRun } from './agentEventReducerShared'
export {
  applyAgentCommandSessionSnapshotToChatMessage,
  markMissingAgentCommandSessionOutcomeUnknown
} from './agentEventReducerCommand'
export {
  applyOptimisticGuidanceToChatMessage,
  removeGuidanceFromChatMessage
} from './agentEventReducerGuidance'

const SAFE_MODEL_REQUEST_INTERRUPTION_REASONS = new Set<ChatAgentInterruptionView['reason']>([
  'service_connection_failed',
  'service_unavailable',
  'authentication_failed',
  'quota_exhausted',
  'context_limit_exceeded',
  'request_rejected',
  'response_invalid',
  'request_failed'
])

function parseSafeModelRequestInterruption(details: unknown): ChatAgentInterruptionView | null {
  if (typeof details !== 'object' || details === null || Array.isArray(details)) return null
  const record = details as Record<string, unknown>
  if (
    record.type !== 'safe_model_request_interruption' ||
    record.safeToContinue !== true ||
    typeof record.reason !== 'string' ||
    !SAFE_MODEL_REQUEST_INTERRUPTION_REASONS.has(
      record.reason as ChatAgentInterruptionView['reason']
    )
  ) {
    return null
  }
  return { reason: record.reason as ChatAgentInterruptionView['reason'] }
}

function committedAssistantContent(content: string): string {
  return content
}

function rollbackUncommittedModelStreams(message: ChatMessage): ChatMessage {
  const run = message.agentRun
  const checkpoints = Object.entries(run?.messageStreamCheckpoints ?? {})
  if (!run || checkpoints.length === 0) {
    return { ...message, content: committedAssistantContent(message.content) }
  }

  // Model requests are sequential within one Agent Run, so at most one stream is provisional.
  const [, checkpoint] = checkpoints[checkpoints.length - 1]
  const activeStreamIds = new Set(checkpoints.map(([streamId]) => streamId))

  return {
    ...message,
    content: committedAssistantContent(checkpoint.previousContent),
    agentRun: {
      ...run,
      messageStreamCheckpoints: {},
      timeline: run.timeline.filter(
        (item) => item.type !== 'message' || !item.streamId || !activeStreamIds.has(item.streamId)
      )
    }
  }
}

function normalizeAgentRunToolActivities(run: ChatAgentRunView): ChatAgentRunView {
  return {
    ...run,
    webSearchActivities: normalizeWebSearchActivities(run),
    readActivities: normalizeReadActivities(run)
  }
}

export function settleAgentRunToolActivities(
  run: ChatAgentRunView,
  status: ChatAgentRunView['status'],
  settledAt = Date.now()
): ChatAgentRunView {
  const runWithStatus = normalizeAgentRunToolActivities({
    ...run,
    status
  })
  const settledActivityStatus = getSettledActivityStatus(status)

  if (!settledActivityStatus) return runWithStatus

  return {
    ...runWithStatus,
    fileChangePreviews: [],
    messageStreamCheckpoints: status === 'completed' ? {} : runWithStatus.messageStreamCheckpoints,
    timeline: settlePendingContextCompactions(
      getFinalTimeline(runWithStatus),
      settledActivityStatus
    ),
    webSearchActivities: settlePendingWebSearchActivities(
      runWithStatus,
      settledActivityStatus,
      settledAt
    ),
    readActivities: settlePendingReadActivities(runWithStatus, settledActivityStatus, settledAt),
    mcpInvocations: settlePendingMcpInvocations(runWithStatus)
  }
}

export function shouldTouchConversationForAgentEvent(agentEvent: AgentEvent) {
  if (agentEvent.type === 'done') return true
  if (agentEvent.type === 'approval_required') return true
  if (agentEvent.type === 'mcp_tool_invocation_state_changed') return true
  if (
    agentEvent.type === 'guidance_queued' ||
    agentEvent.type === 'guidance_applied' ||
    agentEvent.type === 'guidance_rejected'
  ) {
    return true
  }
  if (agentEvent.type === 'error' && !agentEvent.recoverable) return true

  if (agentEvent.type === 'state') {
    return [
      'waiting_for_approval',
      'waiting_for_user_input',
      'completed',
      'failed',
      'cancelled'
    ].includes(agentEvent.state.status)
  }

  return false
}

function getChatMessageStatusFromAgentStatus(
  status: AgentChatOutput['status']
): ChatMessage['status'] {
  if (status === 'running' || isSuspendedAgentRunStatus(status)) return 'pending'
  if (status === 'failed') return 'error'
  return 'sent'
}

export function applyAgentEventToChatMessage(
  message: ChatMessage,
  agentEvent: AgentEvent
): ChatMessage {
  const runId = agentEvent.runId ?? message.agentRun?.runId ?? null
  const currentRun = ensureAgentRun(message.agentRun, runId)

  // A durable terminal Run is a tombstone. Late buffered notifications must not resurrect it as
  // pending/running or append post-terminal model/tool activity. A handed-off command is an
  // independent process lifecycle, so its events may still refresh only the call-scoped view.
  const isManagedCommandEvent =
    agentEvent.type === 'command_started' ||
    agentEvent.type === 'command_output' ||
    agentEvent.type === 'command_exited' ||
    agentEvent.type === 'command_interrupted'
  if (isCompletedAgentRunStatus(currentRun.status) && !isManagedCommandEvent) {
    if (agentEvent.type !== 'done') return message
    const doneStatus = agentEvent.status ?? (agentEvent.success ? 'completed' : 'failed')
    if (doneStatus !== currentRun.status) return message
  }

  if (agentEvent.type === 'started') {
    return {
      ...message,
      status: 'pending',
      agentRun: {
        ...currentRun,
        runId: agentEvent.runId,
        status: 'running',
        llmRetry: undefined,
        startedAt: currentRun.startedAt ?? Date.now(),
        toolDefinitions: agentEvent.toolDefinitions
      }
    }
  }

  if (agentEvent.type === 'tool_set_changed') {
    return {
      ...message,
      agentRun: {
        ...currentRun,
        runId: agentEvent.runId,
        toolDefinitions: agentEvent.toolDefinitions,
        toolSetRevision: {
          stable: agentEvent.stableRevision,
          dynamic: agentEvent.dynamicRevision,
          effective: agentEvent.effectiveRevision
        }
      }
    }
  }

  if (agentEvent.type === 'state') {
    const terminalState = isCompletedAgentRunStatus(agentEvent.state.status)
    const nextRun = settleAgentRunToolActivities(
      {
        ...currentRun,
        status: agentEvent.state.status,
        completedAt: isCompletedAgentRunStatus(agentEvent.state.status)
          ? (currentRun.completedAt ?? Date.now())
          : currentRun.completedAt,
        state: agentEvent.state,
        error: agentEvent.state.lastError ?? currentRun.error,
        llmRetry: terminalState ? undefined : currentRun.llmRetry
      },
      agentEvent.state.status
    )

    return {
      ...message,
      status: getChatMessageStatusFromAgentStatus(agentEvent.state.status),
      agentRun: nextRun
    }
  }

  if (agentEvent.type === 'message_stream_started') {
    if (currentRun.messageStreamCheckpoints?.[agentEvent.streamId]) {
      return message
    }
    return {
      ...message,
      // Every stream is one model turn. Earlier turns stay in the Timeline; the main message
      // projection starts fresh so tool-loop narration can never become the final answer.
      content: '',
      agentRun: {
        ...currentRun,
        messageStreamCheckpoints: {
          ...currentRun.messageStreamCheckpoints,
          [agentEvent.streamId]: {
            previousContent: message.content
          }
        }
      }
    }
  }

  if (agentEvent.type === 'message_stream_reset') {
    const checkpoint = currentRun.messageStreamCheckpoints?.[agentEvent.streamId]
    if (!checkpoint) return message
    const nextCheckpoints = { ...currentRun.messageStreamCheckpoints }
    delete nextCheckpoints[agentEvent.streamId]
    return {
      ...message,
      content: checkpoint.previousContent,
      agentRun: {
        ...currentRun,
        messageStreamCheckpoints: nextCheckpoints,
        timeline: currentRun.timeline.filter(
          (item) => item.type !== 'message' || item.streamId !== agentEvent.streamId
        )
      }
    }
  }

  if (agentEvent.type === 'message_stream_committed') {
    const nextCheckpoints = { ...currentRun.messageStreamCheckpoints }
    delete nextCheckpoints[agentEvent.streamId]
    const timeline =
      agentEvent.traceSequence === null
        ? currentRun.timeline
        : currentRun.timeline.map((item) =>
            item.type === 'message' && item.streamId === agentEvent.streamId
              ? { ...item, traceSequence: agentEvent.traceSequence ?? undefined }
              : item
          )
    return {
      ...message,
      agentRun: {
        ...currentRun,
        llmRetry: undefined,
        messageStreamCheckpoints: nextCheckpoints,
        timeline
      }
    }
  }

  if (agentEvent.type === 'tool_input_progress') {
    if (!currentRun.llmRetry) return message
    const receivedAt = Date.now()
    return {
      ...message,
      status: 'pending',
      agentRun: {
        ...currentRun,
        status: 'running',
        llmRetry: undefined,
        ...getRunResponseTimestamps(currentRun, receivedAt)
      }
    }
  }

  if (agentEvent.type === 'file_change_preview_updated') {
    const receivedAt = Date.now()
    return {
      ...message,
      status: 'pending',
      agentRun: {
        ...currentRun,
        status: 'running',
        llmRetry: undefined,
        ...getRunResponseTimestamps(currentRun, receivedAt),
        fileChangePreviews: upsertFileChangePreview(
          currentRun.fileChangePreviews ?? [],
          agentEvent.preview,
          receivedAt
        )
      }
    }
  }

  if (agentEvent.type === 'file_change_preview_cleared') {
    return {
      ...message,
      agentRun: {
        ...currentRun,
        fileChangePreviews: (currentRun.fileChangePreviews ?? []).filter(
          (preview) =>
            preview.streamId !== agentEvent.streamId || preview.attempt !== agentEvent.attempt
        )
      }
    }
  }

  if (agentEvent.type === 'context_window_updated') {
    return message
  }

  if (agentEvent.type === 'context_compaction_started') {
    return {
      ...message,
      status: 'pending',
      agentRun: {
        ...currentRun,
        status: 'running',
        timeline: startContextCompaction(
          currentRun,
          agentEvent.operationId,
          agentEvent.traceSequence
        )
      }
    }
  }

  if (agentEvent.type === 'context_compaction_finished') {
    return {
      ...message,
      agentRun: {
        ...currentRun,
        timeline: finishContextCompaction(
          currentRun,
          agentEvent.operationId,
          agentEvent.outcome,
          agentEvent.traceSequence
        )
      }
    }
  }

  if (agentEvent.type === 'llm_retry') {
    return {
      ...message,
      status: 'pending',
      agentRun: {
        ...currentRun,
        status: 'running',
        llmRetry: {
          category: agentEvent.category,
          ...(agentEvent.providerCode === undefined
            ? {}
            : { providerCode: agentEvent.providerCode }),
          delayMs: agentEvent.delayMs,
          retryAt: agentEvent.retryAt,
          attempt: agentEvent.attempt,
          maxAttempts: agentEvent.maxAttempts
        }
      }
    }
  }

  if (agentEvent.type === 'message_delta') {
    const receivedAt = Date.now()

    return {
      ...message,
      content: getMessageContentAfterDelta(message.content, agentEvent.delta),
      status: 'pending',
      agentRun: {
        ...currentRun,
        status: 'running',
        llmRetry: undefined,
        ...getRunResponseTimestamps(currentRun, receivedAt),
        timeline: appendMessageDeltaToTimeline(currentRun, agentEvent.delta, agentEvent.streamId)
      }
    }
  }

  if (agentEvent.type === 'message') {
    const receivedAt = Date.now()

    return {
      ...message,
      content: agentEvent.content,
      status: 'pending',
      agentRun: {
        ...currentRun,
        status: 'running',
        llmRetry: undefined,
        ...getRunResponseTimestamps(currentRun, receivedAt),
        timeline: appendMessageToTimeline(currentRun, agentEvent.content)
      }
    }
  }

  if (agentEvent.type === 'tool_call') {
    const mcpInvocation = currentRun.mcpInvocations?.find(
      (candidate) => candidate.callId === agentEvent.call.id
    )
    if (mcpInvocation) {
      return {
        ...message,
        status: 'pending',
        agentRun: {
          ...currentRun,
          toolCalls: currentRun.toolCalls.filter((call) => call.id !== mcpInvocation.callId),
          toolResults: currentRun.toolResults.filter(
            (result) => result.callId !== mcpInvocation.callId
          ),
          timeline: upsertMcpInvocationTimelineItem(
            currentRun.timeline,
            mcpInvocation.invocationId,
            mcpInvocation.callId
          )
        }
      }
    }

    const runWithCleanTimeline = {
      ...currentRun,
      timeline: removeTransientToolTimelineItems(currentRun.timeline)
    }

    return {
      ...message,
      status: 'pending',
      agentRun: {
        ...runWithCleanTimeline,
        status: 'running',
        toolCalls: upsertById(currentRun.toolCalls, agentEvent.call, (call) => call.id),
        fileChangePreviews: bindFileChangePreviewsToCall(
          currentRun.fileChangePreviews ?? [],
          agentEvent.call
        ),
        webSearchActivities: upsertWebSearchActivityFromCall(currentRun, agentEvent.call),
        readActivities: upsertReadActivityFromCall(currentRun, agentEvent.call),
        timeline: appendToolCallToTimeline(
          runWithCleanTimeline,
          agentEvent.call.id,
          agentEvent.traceSequence,
          agentEvent.identity
        )
      }
    }
  }

  if (agentEvent.type === 'tool_result') {
    if (
      currentRun.mcpInvocations?.some((candidate) => candidate.callId === agentEvent.result.callId)
    ) {
      // MCP lifecycle is the only Renderer source of truth. Generic results can contain raw
      // bodies and must never enter chat state once the typed call identity is known.
      return message
    }

    const builtinCapabilityResult = currentRun.timeline.some(
      (item) =>
        item.type === 'tool_call' &&
        item.callId === agentEvent.result.callId &&
        item.identity?.type === 'builtin_capability'
    )
    const safeResult = builtinCapabilityResult
      ? projectBuiltinCapabilityToolResult(agentEvent.result)
      : agentEvent.result

    let nextRun: ChatAgentRunView = {
      ...currentRun,
      status: 'running',
      toolResults: upsertById(currentRun.toolResults, safeResult, (result) => result.callId),
      webSearchActivities: upsertWebSearchActivityFromResult(currentRun, agentEvent.result),
      readActivities: upsertReadActivityFromResult(currentRun, agentEvent.result),
      fileChanges: updateFileChangeFromToolResult(currentRun.fileChanges ?? [], agentEvent.result),
      fileChangePreviews:
        agentEvent.result.tool === 'apply_patch'
          ? (currentRun.fileChangePreviews ?? []).filter(
              (preview) => preview.toolCallId !== agentEvent.result.callId
            )
          : (currentRun.fileChangePreviews ?? [])
    }
    const managedReceipt = runningCommandReceipt(agentEvent.result)
    if (managedReceipt) {
      nextRun = withCommandSession(nextRun, {
        callId: agentEvent.result.callId,
        sessionId: managedReceipt.sessionId,
        status: 'running',
        startedAt: managedReceipt.startedAt,
        latestSequence: managedReceipt.latestSequence,
        outputTruncated: managedReceipt.outputTruncated
      })
    } else {
      const terminal = terminalCommandStatusFromToolResult(agentEvent.result)
      const existingSession = nextRun.commandSessions?.[agentEvent.result.callId]
      if (terminal && !existingSession?.sessionId) {
        nextRun = withCommandSession(nextRun, {
          callId: agentEvent.result.callId,
          sessionId: existingSession?.sessionId,
          status: terminal.status,
          startedAt: existingSession?.startedAt,
          exitCode: terminal.exitCode,
          latestSequence: existingSession?.latestSequence ?? 0,
          outputTruncated: existingSession?.outputTruncated ?? false
        })
      }
    }
    const hasManagedSessionIdentity = Boolean(
      nextRun.commandSessions?.[agentEvent.result.callId]?.sessionId
    )
    if (
      !managedReceipt &&
      !hasManagedSessionIdentity &&
      currentRun.commandOutputPreviews?.[agentEvent.result.callId]
    ) {
      const commandOutputPreviews = { ...currentRun.commandOutputPreviews }
      delete commandOutputPreviews[agentEvent.result.callId]
      if (Object.keys(commandOutputPreviews).length > 0) {
        nextRun.commandOutputPreviews = commandOutputPreviews
      } else {
        delete nextRun.commandOutputPreviews
      }
    }

    return {
      ...message,
      status: 'pending',
      agentRun: nextRun
    }
  }

  if (agentEvent.type === 'file_change_updated') {
    return {
      ...message,
      status: 'pending',
      agentRun: {
        ...currentRun,
        status: 'running',
        fileChanges: upsertById(
          currentRun.fileChanges ?? [],
          agentEvent.fileChange,
          (fileChange) => fileChange.transactionId
        ),
        fileChangePreviews: (currentRun.fileChangePreviews ?? []).filter(
          (preview) => preview.transactionId !== agentEvent.fileChange.transactionId
        )
      }
    }
  }

  if (agentEvent.type === 'todo_updated') {
    return {
      ...message,
      status: 'pending',
      agentRun: {
        ...currentRun,
        status: 'running',
        todo: agentEvent.todo
      }
    }
  }

  if (agentEvent.type === 'skill_activated') {
    return {
      ...message,
      status: 'pending',
      agentRun: {
        ...currentRun,
        status: 'running',
        activatedSkills: mergeActivatedSkillSummaries(currentRun.activatedSkills, [
          agentEvent.skill
        ]),
        skillActivationRevision: agentEvent.activationRevision
      }
    }
  }

  if (agentEvent.type === 'approval_required') {
    if (agentEvent.action.type === 'mcp_tool_call') {
      const mcpProjection = addMcpApprovalViews(currentRun, [agentEvent.action])
      return {
        ...message,
        status: 'pending',
        agentRun: {
          ...currentRun,
          status: 'waiting_for_approval',
          approvals: upsertAgentAction(currentRun.approvals, agentEvent.action),
          skillInstallations: mergeSkillInstallationApprovals(currentRun.skillInstallations, [
            agentEvent.action
          ]),
          ...mcpProjection
        }
      }
    }

    const call = getActionToolCall(agentEvent.action)
    // Pending-action hydration is an independent async snapshot. Once the Host Session authority
    // has projected this command call, an older `approval_required` record may no longer move the
    // call or parent Run back to waiting_for_approval.
    if (call?.tool === 'run_command' && currentRun.commandSessions?.[call.id]) return message
    const timeline = call
      ? appendToolCallToTimeline(currentRun, call.id)
      : removeTransientToolTimelineItems(currentRun.timeline)

    return {
      ...message,
      status: 'pending',
      agentRun: {
        ...currentRun,
        status: 'waiting_for_approval',
        toolCalls: call
          ? upsertById(currentRun.toolCalls, call, (candidate) => candidate.id)
          : currentRun.toolCalls,
        fileChangeProposals: updateFileChangeProposalApprovalStatus(
          currentRun.fileChangeProposals,
          agentEvent.action,
          'required'
        ),
        fileChanges: updateFileChangeApprovalStatus(
          currentRun.fileChanges ?? [],
          agentEvent.action,
          'required'
        ),
        approvals: upsertAgentAction(currentRun.approvals, agentEvent.action),
        skillInstallations: mergeSkillInstallationApprovals(currentRun.skillInstallations, [
          agentEvent.action
        ]),
        timeline
      }
    }
  }

  if (agentEvent.type === 'file_change_proposed') {
    const call = getActionToolCall({ type: 'file_change', fileChange: agentEvent.fileChange })
    const runWithProposal = {
      ...currentRun,
      fileChangeProposals: upsertById(
        currentRun.fileChangeProposals,
        agentEvent.fileChange,
        (fileChange) => fileChange.id
      ),
      toolCalls: call
        ? upsertById(currentRun.toolCalls, call, (candidate) => candidate.id)
        : currentRun.toolCalls
    }

    return {
      ...message,
      status: 'pending',
      agentRun: {
        ...runWithProposal,
        status: isSuspendedAgentRunStatus(currentRun.status) ? currentRun.status : 'running',
        timeline: call ? appendToolCallToTimeline(runWithProposal, call.id) : currentRun.timeline
      }
    }
  }

  if (agentEvent.type === 'command_started') {
    if (
      (currentRun.runId && currentRun.runId !== agentEvent.runId) ||
      !hasRunCommandCall(currentRun, agentEvent.callId)
    ) {
      return message
    }
    const existing = currentRun.commandSessions?.[agentEvent.callId]
    if (existing?.sessionId && existing.sessionId !== agentEvent.sessionId) return message

    const nextRun = withCommandSession(settleCommandApproval(currentRun, agentEvent.callId), {
      callId: agentEvent.callId,
      sessionId: agentEvent.sessionId,
      status: 'running',
      startedAt: agentEvent.startedAt,
      latestSequence: 0,
      outputTruncated: false
    })
    return nextRun === currentRun ? message : { ...message, agentRun: nextRun }
  }

  if (agentEvent.type === 'command_output') {
    if (
      (currentRun.runId && currentRun.runId !== agentEvent.runId) ||
      !hasRunCommandCall(currentRun, agentEvent.callId)
    ) {
      return message
    }

    const runWithSettledApproval = settleCommandApproval(currentRun, agentEvent.callId)
    const existingSession = runWithSettledApproval.commandSessions?.[agentEvent.callId]
    if (
      (existingSession?.sessionId && existingSession.sessionId !== agentEvent.sessionId) ||
      (existingSession && isTerminalCommandSessionStatus(existingSession.status))
    ) {
      return message
    }
    if (agentEvent.sequence <= (existingSession?.latestSequence ?? 0)) {
      return runWithSettledApproval === currentRun
        ? message
        : { ...message, agentRun: runWithSettledApproval }
    }

    const existing = runWithSettledApproval.commandOutputPreviews?.[agentEvent.callId]
    const appended = appendCommandOutputChunk(agentEvent.callId, existing, {
      sequence: agentEvent.sequence,
      stream: agentEvent.stream,
      output: agentEvent.output
    })
    const nextPreview = appended.preview
    const nextRun = withCommandSession(runWithSettledApproval, {
      callId: agentEvent.callId,
      sessionId: agentEvent.sessionId,
      status: 'running',
      startedAt: existingSession?.startedAt,
      latestSequence: agentEvent.sequence,
      outputTruncated: Boolean(existingSession?.outputTruncated || appended.truncated)
    })

    return {
      ...message,
      agentRun: {
        ...nextRun,
        commandOutputPreviews: {
          ...(nextRun.commandOutputPreviews ?? {}),
          [agentEvent.callId]: nextPreview
        }
      }
    }
  }

  if (agentEvent.type === 'command_exited' || agentEvent.type === 'command_interrupted') {
    if (
      (currentRun.runId && currentRun.runId !== agentEvent.runId) ||
      !hasRunCommandCall(currentRun, agentEvent.callId)
    ) {
      return message
    }
    const runWithSettledApproval = settleCommandApproval(currentRun, agentEvent.callId)
    const existing = runWithSettledApproval.commandSessions?.[agentEvent.callId]
    if (existing?.sessionId && existing.sessionId !== agentEvent.sessionId) return message

    const nextRun = withCommandSession(runWithSettledApproval, {
      callId: agentEvent.callId,
      sessionId: agentEvent.sessionId,
      status: agentEvent.type === 'command_interrupted' ? 'interrupted' : agentEvent.status,
      startedAt: existing?.startedAt,
      endedAt: agentEvent.endedAt,
      exitCode: agentEvent.type === 'command_exited' ? agentEvent.exitCode : undefined,
      latestSequence: agentEvent.latestSequence,
      outputs: parseManagedCommandOutputs(agentEvent.outputs),
      artifactObservation: agentEvent.artifactObservation,
      outputTruncated: agentEvent.outputTruncated
    })
    return nextRun === currentRun ? message : { ...message, agentRun: nextRun }
  }

  if (agentEvent.type === 'guidance_queued' || agentEvent.type === 'guidance_applied') {
    const status = agentEvent.type === 'guidance_applied' ? 'applied' : 'queued'
    const item: ChatGuidanceTimelineItem = {
      id: guidanceTimelineId(agentEvent.clientMessageId),
      type: 'user_guidance',
      guidanceId: agentEvent.guidanceId,
      clientMessageId: agentEvent.clientMessageId,
      content: agentEvent.content,
      attachments: guidanceAttachments(agentEvent.attachments),
      status,
      createdAt: agentEvent.createdAt,
      sequence: agentEvent.type === 'guidance_applied' ? agentEvent.sequence : undefined,
      traceSequence: agentEvent.type === 'guidance_applied' ? agentEvent.sequence : undefined
    }
    return {
      ...message,
      agentRun: {
        ...currentRun,
        timeline: upsertGuidanceTimelineItem(currentRun, item)
      }
    }
  }

  if (agentEvent.type === 'guidance_rejected') {
    return removeGuidanceFromChatMessage(message, agentEvent.clientMessageId)
  }

  if (agentEvent.type === 'mcp_tool_invocation_state_changed') {
    const projected = projectMcpInvocationEvent(agentEvent.invocation)
    const mcpInvocations = upsertMcpInvocationView(currentRun.mcpInvocations ?? [], projected)
    const selected =
      mcpInvocations.find((candidate) => candidate.invocationId === projected.invocationId) ??
      projected

    return {
      ...message,
      agentRun: {
        ...currentRun,
        mcpInvocations,
        toolCalls: currentRun.toolCalls.filter((call) => call.id !== selected.callId),
        toolResults: currentRun.toolResults.filter((result) => result.callId !== selected.callId),
        timeline: upsertMcpInvocationTimelineItem(
          currentRun.timeline,
          selected.invocationId,
          selected.callId
        )
      }
    }
  }

  if (agentEvent.type === 'error') {
    const interruption = parseSafeModelRequestInterruption(agentEvent.details)
    if (!agentEvent.recoverable && interruption) {
      const rolledBackMessage = rollbackUncommittedModelStreams(message)
      const rolledBackRun = rolledBackMessage.agentRun ?? currentRun
      const nextRun = settleAgentRunToolActivities(
        {
          ...rolledBackRun,
          status: 'failed',
          completedAt: rolledBackRun.completedAt ?? Date.now(),
          interruption,
          error: undefined,
          llmRetry: undefined
        },
        'failed'
      )

      return {
        ...rolledBackMessage,
        content: committedAssistantContent(rolledBackMessage.content),
        status: 'sent',
        agentRun: nextRun
      }
    }

    const nextStatus = agentEvent.recoverable ? currentRun.status : 'failed'
    const nextRun = settleAgentRunToolActivities(
      {
        ...currentRun,
        status: nextStatus,
        completedAt: agentEvent.recoverable
          ? currentRun.completedAt
          : (currentRun.completedAt ?? Date.now()),
        error: agentEvent.message,
        llmRetry: undefined,
        timeline: appendTimelineItem(currentRun, {
          id:
            agentEvent.traceSequence === null
              ? `error-${currentRun.timeline.length + 1}`
              : `trace-error-${agentEvent.traceSequence}`,
          type: 'error',
          message: agentEvent.message,
          ...(agentEvent.traceSequence === null ? {} : { traceSequence: agentEvent.traceSequence })
        })
      },
      nextStatus
    )

    return {
      ...message,
      content: agentEvent.recoverable ? message.content : agentEvent.message,
      status: agentEvent.recoverable ? message.status : 'error',
      agentRun: nextRun
    }
  }

  const nextStatus = agentEvent.status ?? (agentEvent.success ? 'completed' : 'failed')
  const proposedActions = agentEvent.proposedActions ?? []
  const completedAt = isFinishedAgentOutputStatus(nextStatus)
    ? (currentRun.completedAt ?? Date.now())
    : currentRun.completedAt
  // A cancelled turn has no canonical final answer. All text emitted before cancellation remains
  // available in the timeline, but must not leak back out as an assistant answer when collapsed.
  const safelyInterrupted = nextStatus === 'failed' && currentRun.interruption !== undefined
  const terminalEventContent =
    nextStatus === 'cancelled' || safelyInterrupted ? undefined : agentEvent.content
  const finalContent =
    nextStatus === 'cancelled'
      ? ''
      : safelyInterrupted
        ? committedAssistantContent(message.content)
        : getFinalMessageContent(message.content, terminalEventContent)
  const finalResponseAt =
    finalContent && !currentRun.firstResponseAt
      ? (completedAt ?? Date.now())
      : currentRun.firstResponseAt
  const mcpProjection = addMcpApprovalViews(
    {
      ...currentRun,
      timeline: getFinalTimeline({ ...currentRun, status: nextStatus }, terminalEventContent)
    },
    proposedActions
  )

  const nextRun = settleAgentRunToolActivities(
    {
      ...currentRun,
      status: nextStatus,
      llmRetry: undefined,
      firstResponseAt: finalResponseAt,
      lastResponseAt:
        terminalEventContent !== undefined
          ? (currentRun.lastResponseAt ?? finalResponseAt)
          : currentRun.lastResponseAt,
      completedAt,
      // Core-server publishes a cumulative snapshot for the logical run. Replacement keeps
      // notification replay idempotent and avoids double-counting event/output delivery paths.
      usage: agentEvent.usage ?? currentRun.usage,
      finishReason: agentEvent.finishReason,
      approvals: getApprovalsForStatus(nextStatus, currentRun.approvals, proposedActions),
      skillInstallations: mergeSkillInstallationApprovals(
        currentRun.skillInstallations,
        proposedActions
      ),
      ...mcpProjection
    },
    nextStatus,
    completedAt ?? Date.now()
  )

  return {
    ...message,
    content: finalContent,
    status: isSuspendedAgentRunStatus(nextStatus)
      ? 'pending'
      : nextStatus === 'cancelled' || agentEvent.success || safelyInterrupted
        ? 'sent'
        : 'error',
    agentRun: nextRun
  }
}

function applyAgentOutputToChatMessage(message: ChatMessage, output: AgentChatOutput): ChatMessage {
  const messageWithEvents = output.events.reduce(applyAgentEventToChatMessage, message)
  const currentRun = ensureAgentRun(messageWithEvents.agentRun, output.runId, output.status)
  const safelyInterrupted = output.status === 'failed' && currentRun.interruption !== undefined
  const outputFinalContent =
    output.status === 'cancelled' || safelyInterrupted
      ? undefined
      : output.status === 'completed' || output.content
        ? output.content
        : undefined
  const nextContent =
    output.status === 'cancelled'
      ? ''
      : safelyInterrupted
        ? committedAssistantContent(messageWithEvents.content)
        : getFinalMessageContent(messageWithEvents.content, outputFinalContent)
  const outputCompletedAt = isFinishedAgentOutputStatus(output.status)
    ? (currentRun.completedAt ?? Date.now())
    : currentRun.completedAt
  const finalResponseAt =
    nextContent && !currentRun.firstResponseAt
      ? (outputCompletedAt ?? Date.now())
      : currentRun.firstResponseAt
  const mcpProjection = addMcpApprovalViews(
    {
      ...currentRun,
      timeline: getFinalTimeline({ ...currentRun, status: output.status }, outputFinalContent)
    },
    output.proposedActions
  )

  const nextRun = settleAgentRunToolActivities(
    {
      ...currentRun,
      status: output.status,
      firstResponseAt: finalResponseAt,
      lastResponseAt:
        finalResponseAt && !currentRun.lastResponseAt ? finalResponseAt : currentRun.lastResponseAt,
      completedAt: outputCompletedAt,
      toolDefinitions: output.toolDefinitions,
      todo: output.todo ?? currentRun.todo,
      // AgentChatOutput follows the same run-cumulative contract as Done events.
      usage: output.usage ?? currentRun.usage,
      finishReason: output.finishReason,
      approvals: getApprovalsForStatus(output.status, currentRun.approvals, output.proposedActions),
      skillInstallations: mergeSkillInstallationApprovals(
        currentRun.skillInstallations,
        output.proposedActions
      ),
      ...mcpProjection
    },
    output.status,
    outputCompletedAt ?? Date.now()
  )

  return {
    ...messageWithEvents,
    content: nextContent,
    status: safelyInterrupted ? 'sent' : getChatMessageStatusFromAgentStatus(output.status),
    agentRun: nextRun
  }
}

export function applyAgentActionDecisionToChatMessage(
  message: ChatMessage,
  action: AgentProposedAction,
  decision: 'approved' | 'rejected',
  rejectionMessage?: string
): ChatMessage {
  const currentRun = ensureAgentRun(message.agentRun, null)
  const actionId = getAgentActionId(action)
  const builtinCapabilityCallId =
    action.type === 'builtin_capability_activation' ? getActionToolCallId(action) : null
  const approvalStatus: AgentApprovalStatus = decision === 'approved' ? 'approved' : 'rejected'
  const rejectedToolResult =
    decision === 'rejected' ? createRejectedToolResult(action, rejectionMessage) : null
  const mcpCallId = action.type === 'mcp_tool_call' ? action.approval.identity.callId : undefined
  const mcpInvocationId =
    action.type === 'mcp_tool_call' ? action.approval.identity.invocationId : undefined
  const rejectionReason =
    decision === 'rejected' ? projectMcpRejectionReason(rejectionMessage) : undefined
  const mcpInvocations =
    mcpInvocationId && decision === 'rejected'
      ? (currentRun.mcpInvocations ?? []).map((invocation) =>
          invocation.invocationId === mcpInvocationId
            ? {
                ...invocation,
                ...(rejectionReason === undefined ? {} : { rejectionReason })
              }
            : invocation
        )
      : (currentRun.mcpInvocations ?? [])
  const runWithDecision = normalizeAgentRunToolActivities({
    ...currentRun,
    status: currentRun.status === 'waiting_for_user_input' ? currentRun.status : 'running',
    approvals: removeAgentAction(currentRun.approvals, actionId),
    skillInstallations: applySkillInstallationDecision(
      currentRun.skillInstallations,
      action,
      decision
    ),
    toolCalls: updateToolCallApprovalStatus(currentRun.toolCalls, action, approvalStatus),
    toolResults: rejectedToolResult
      ? upsertById(currentRun.toolResults, rejectedToolResult, (result) => result.callId)
      : mcpCallId
        ? currentRun.toolResults.filter((result) => result.callId !== mcpCallId)
        : currentRun.toolResults,
    fileChangeProposals: updateFileChangeProposalApprovalStatus(
      currentRun.fileChangeProposals,
      action,
      approvalStatus
    ),
    fileChanges: updateFileChangeApprovalStatus(
      currentRun.fileChanges ?? [],
      action,
      approvalStatus
    ),
    mcpInvocations,
    timeline: removeTransientToolTimelineItems(currentRun.timeline).filter(
      (item) => item.type !== 'tool_call' || item.callId !== mcpCallId
    )
  })

  if (builtinCapabilityCallId) {
    return {
      ...message,
      status: 'pending',
      agentRun: {
        ...runWithDecision,
        // This call is a live approval anchor, not durable activity. The Host returns the
        // activation decision to the model through its own continuation path.
        toolCalls: runWithDecision.toolCalls.filter((call) => call.id !== builtinCapabilityCallId),
        toolResults: runWithDecision.toolResults.filter(
          (result) => result.callId !== builtinCapabilityCallId
        ),
        timeline: runWithDecision.timeline.filter(
          (item) => item.type !== 'tool_call' || item.callId !== builtinCapabilityCallId
        )
      }
    }
  }

  return {
    ...message,
    status: 'pending',
    agentRun: runWithDecision
  }
}

export function applyAgentActionExecutionToChatMessage(
  message: ChatMessage,
  execution: AgentActionExecutionOutput,
  mcpRejectionMessage?: string,
  decidedAction?: AgentProposedAction
): ChatMessage {
  // Action-decision RPCs and Agent notifications travel on independent channels. In particular,
  // an approved command is dispatched on a background worker before the approval RPC response is
  // serialized, so a fast continuation can publish `done` first. A terminal parent Run is a
  // tombstone: the late RPC may still settle approval and process child state below, but it must
  // never reopen the assistant message or replace the terminal Run status/content.
  const parentIsTerminal = isCompletedAgentRunStatus(message.agentRun?.status)
  // An approval response may arrive after its resumed worker has reached a human-input pause.
  // Only the independent human-input continuation may advance that pause; an older approval RPC
  // may settle its own action, but cannot replace this newer Run state or cumulative usage.
  const parentIsWaitingForUserInput = message.agentRun?.status === 'waiting_for_user_input'
  const messageWithAgentOutput =
    parentIsTerminal || parentIsWaitingForUserInput
      ? message
      : applyAgentOutputToChatMessage(message, execution.agentOutput)
  // The Host keeps the logical Run's guidance inbox available across approval and tool
  // execution. Its running acknowledgement is sufficient; another model request is not required.
  const currentRun = ensureAgentRun(messageWithAgentOutput.agentRun, execution.agentOutput.runId)
  const originalMcpApproval = message.agentRun?.approvals.find(
    (action) =>
      action.type === 'mcp_tool_call' && action.approval.identity.actionId === execution.actionId
  )
  const originalBuiltinCapabilityApproval =
    decidedAction?.type === 'builtin_capability_activation' &&
    decidedAction.approval.actionId === execution.actionId
      ? decidedAction
      : message.agentRun?.approvals.find(
          (action) =>
            action.type === 'builtin_capability_activation' &&
            action.approval.actionId === execution.actionId
        )
  const originalBuiltinMcpApproval =
    decidedAction?.type === 'builtin_mcp_tool_approval' &&
    decidedAction.approval.identity.actionId === execution.actionId
      ? decidedAction
      : message.agentRun?.approvals.find(
          (action) =>
            action.type === 'builtin_mcp_tool_approval' &&
            action.approval.identity.actionId === execution.actionId
        )
  const mcpInvocation = currentRun.mcpInvocations?.find(
    (candidate) => candidate.actionId === execution.actionId
  )
  const mcpCallId =
    mcpInvocation?.callId ??
    (originalMcpApproval?.type === 'mcp_tool_call'
      ? originalMcpApproval.approval.identity.callId
      : undefined)
  const builtinCapabilityCallId = originalBuiltinCapabilityApproval
    ? getActionToolCallId(originalBuiltinCapabilityApproval)
    : null
  const builtinMcpCallId =
    originalBuiltinMcpApproval?.type === 'builtin_mcp_tool_approval'
      ? originalBuiltinMcpApproval.approval.identity.callId
      : null

  if (execution.actionType === 'file_change') {
    const fileChangeResult = execution.fileChangeResult
    if (
      execution.toolName !== 'apply_patch' ||
      !fileChangeResult ||
      decidedAction?.type !== 'file_change' ||
      decidedAction.fileChange.id !== execution.actionId ||
      decidedAction.fileChange.transactionId !== fileChangeResult.transactionId
    ) {
      throw new Error('Invalid FileChange action execution identity')
    }

    const finalApprovalStatus: AgentApprovalStatus =
      execution.status === 'rejected' ? 'rejected' : 'approved'
    const successfulSettlement =
      fileChangeResult.status === 'applied' ||
      fileChangeResult.status === 'already_applied' ||
      fileChangeResult.status === 'rejected' ||
      fileChangeResult.status === 'aborted'
    const toolResult: AgentToolResult = {
      callId: execution.actionId,
      tool: 'apply_patch',
      ok: successfulSettlement,
      result: fileChangeResult,
      ...(!successfulSettlement && fileChangeResult.error !== null
        ? { error: fileChangeResult.error }
        : {})
    }
    const fileChangesWithDecision = updateFileChangeApprovalStatus(
      currentRun.fileChanges ?? [],
      decidedAction,
      finalApprovalStatus
    )
    const nextRun = normalizeAgentRunToolActivities({
      ...currentRun,
      toolCalls: updateToolCallApprovalStatus(
        currentRun.toolCalls,
        decidedAction,
        finalApprovalStatus
      ),
      toolResults: upsertById(currentRun.toolResults, toolResult, (result) => result.callId),
      fileChanges: updateFileChangeFromToolResult(fileChangesWithDecision, toolResult),
      fileChangeProposals: updateFileChangeProposalApprovalStatus(
        currentRun.fileChangeProposals,
        decidedAction,
        finalApprovalStatus
      ),
      fileChangePreviews: (currentRun.fileChangePreviews ?? []).filter(
        (preview) =>
          preview.toolCallId !== execution.actionId &&
          preview.transactionId !== fileChangeResult.transactionId
      ),
      approvals: removeAgentAction(currentRun.approvals, execution.actionId),
      timeline: removeTransientToolTimelineItems(currentRun.timeline)
    })

    return {
      ...messageWithAgentOutput,
      agentRun: nextRun
    }
  }

  if (execution.actionType === 'mcp_tool_call' || mcpCallId) {
    const rejectionReason =
      execution.status === 'rejected' ? projectMcpRejectionReason(mcpRejectionMessage) : undefined
    const nextRun = normalizeAgentRunToolActivities({
      ...currentRun,
      approvals: removeAgentAction(currentRun.approvals, execution.actionId),
      skillInstallations: applySkillInstallationExecution(currentRun.skillInstallations, execution),
      toolCalls: mcpCallId
        ? currentRun.toolCalls.filter((call) => call.id !== mcpCallId)
        : currentRun.toolCalls,
      toolResults: mcpCallId
        ? currentRun.toolResults.filter((result) => result.callId !== mcpCallId)
        : currentRun.toolResults,
      mcpInvocations: (currentRun.mcpInvocations ?? []).map((invocation) =>
        invocation.actionId === execution.actionId && rejectionReason
          ? { ...invocation, rejectionReason }
          : invocation
      ),
      timeline: removeTransientToolTimelineItems(currentRun.timeline).filter(
        (item) => item.type !== 'tool_call' || item.callId !== mcpCallId
      )
    })

    return {
      ...messageWithAgentOutput,
      agentRun: nextRun
    }
  }

  if (execution.actionType === 'builtin_capability_activation') {
    const nextRun = normalizeAgentRunToolActivities({
      ...currentRun,
      approvals: removeAgentAction(currentRun.approvals, execution.actionId),
      skillInstallations: applySkillInstallationExecution(currentRun.skillInstallations, execution),
      toolCalls: builtinCapabilityCallId
        ? currentRun.toolCalls.filter((call) => call.id !== builtinCapabilityCallId)
        : currentRun.toolCalls,
      toolResults: builtinCapabilityCallId
        ? currentRun.toolResults.filter((result) => result.callId !== builtinCapabilityCallId)
        : currentRun.toolResults,
      timeline: removeTransientToolTimelineItems(currentRun.timeline).filter(
        (item) => item.type !== 'tool_call' || item.callId !== builtinCapabilityCallId
      )
    })

    return {
      ...messageWithAgentOutput,
      agentRun: nextRun
    }
  }

  if (execution.actionType === 'builtin_mcp_tool_approval' || originalBuiltinMcpApproval) {
    const approvalStatus: AgentApprovalStatus =
      execution.status === 'rejected' ? 'rejected' : 'approved'
    const safeToolResult = execution.toolResult
      ? projectBuiltinCapabilityToolResult(execution.toolResult)
      : undefined
    const nextRun = normalizeAgentRunToolActivities({
      ...currentRun,
      approvals: removeAgentAction(currentRun.approvals, execution.actionId),
      toolCalls: currentRun.toolCalls.map((call) =>
        call.id === builtinMcpCallId ? { ...call, approvalStatus } : call
      ),
      toolResults: safeToolResult
        ? upsertById(currentRun.toolResults, safeToolResult, (result) => result.callId)
        : currentRun.toolResults,
      skillInstallations: applySkillInstallationExecution(currentRun.skillInstallations, execution),
      timeline: removeTransientToolTimelineItems(currentRun.timeline)
    })
    return {
      ...messageWithAgentOutput,
      agentRun: nextRun
    }
  }

  if (!execution.toolResult) {
    const approvalStatus: AgentApprovalStatus =
      execution.status === 'rejected' ? 'rejected' : 'approved'
    let nextRun = normalizeAgentRunToolActivities({
      ...currentRun,
      approvals: removeAgentAction(currentRun.approvals, execution.actionId),
      toolCalls: currentRun.toolCalls.map((call) =>
        call.id === execution.actionId ? { ...call, approvalStatus } : call
      ),
      fileChangeProposals: currentRun.fileChangeProposals.map((proposal) =>
        proposal.id === execution.actionId ? { ...proposal, approvalStatus } : proposal
      ),
      skillInstallations: applySkillInstallationExecution(currentRun.skillInstallations, execution),
      timeline: removeTransientToolTimelineItems(currentRun.timeline)
    })
    const commandCall = nextRun.toolCalls.find(
      (call) => call.id === execution.actionId && call.tool === 'run_command'
    )
    if (commandCall && execution.status !== 'rejected') {
      const existingSession = nextRun.commandSessions?.[commandCall.id]
      if (!existingSession?.sessionId) {
        nextRun = withCommandSession(nextRun, {
          callId: commandCall.id,
          status:
            execution.status === 'failed' || execution.status === 'conflict'
              ? 'failed'
              : 'starting',
          startedAt: existingSession?.startedAt,
          latestSequence: existingSession?.latestSequence ?? 0,
          outputTruncated: existingSession?.outputTruncated ?? false
        })
      }
    }

    return {
      ...messageWithAgentOutput,
      agentRun: nextRun
    }
  }

  const finalApprovalStatus: AgentApprovalStatus =
    execution.status === 'rejected' ? 'rejected' : 'approved'
  let runWithExecutionResult = normalizeAgentRunToolActivities({
    ...currentRun,
    toolCalls: currentRun.toolCalls.map((call) =>
      call.id === execution.actionId
        ? {
            ...call,
            approvalStatus: finalApprovalStatus
          }
        : call
    ),
    toolResults: upsertById(
      currentRun.toolResults,
      execution.toolResult,
      (result) => result.callId
    ),
    fileChanges: updateFileChangeFromToolResult(currentRun.fileChanges ?? [], execution.toolResult),
    fileChangeProposals: currentRun.fileChangeProposals.map((proposal) =>
      proposal.id === execution.actionId
        ? {
            ...proposal,
            approvalStatus: finalApprovalStatus
          }
        : proposal
    ),
    approvals: removeAgentAction(currentRun.approvals, execution.actionId),
    skillInstallations: applySkillInstallationExecution(currentRun.skillInstallations, execution),
    timeline: removeTransientToolTimelineItems(currentRun.timeline)
  })
  const managedReceipt = runningCommandReceipt(execution.toolResult)
  if (managedReceipt) {
    runWithExecutionResult = withCommandSession(runWithExecutionResult, {
      callId: execution.toolResult.callId,
      sessionId: managedReceipt.sessionId,
      status: 'running',
      startedAt: managedReceipt.startedAt,
      latestSequence: managedReceipt.latestSequence,
      outputTruncated: managedReceipt.outputTruncated
    })
  } else {
    const terminal = terminalCommandStatusFromToolResult(execution.toolResult)
    const existingSession = runWithExecutionResult.commandSessions?.[execution.toolResult.callId]
    if (terminal && !existingSession?.sessionId) {
      runWithExecutionResult = withCommandSession(runWithExecutionResult, {
        callId: execution.toolResult.callId,
        sessionId: existingSession?.sessionId,
        status: terminal.status,
        startedAt: existingSession?.startedAt,
        exitCode: terminal.exitCode,
        latestSequence: existingSession?.latestSequence ?? 0,
        outputTruncated: existingSession?.outputTruncated ?? false
      })
    }
  }

  return {
    ...messageWithAgentOutput,
    agentRun: runWithExecutionResult
  }
}
