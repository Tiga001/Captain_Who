// Renderer UI.
import type {
  AgentActionExecutionOutput,
  AgentApprovalStatus,
  AgentChatOutput,
  AgentEvent,
  AgentFileDraftStatus,
  AgentProposedAction,
  AgentToolCall,
  AgentToolResult
} from '@mycopilot/protocol'
import type {
  ChatAgentRunView,
  ChatAgentTimelineItem,
  ChatMessage
} from '../features/chat/chatTypes'
import { THINKING_PLACEHOLDER } from './appConstants'
import { getAgentActionId } from './agentActionUtils'
import {
  normalizeReadActivities,
  settlePendingReadActivities,
  upsertReadActivityFromCall,
  upsertReadActivityFromResult
} from '../features/chat/agentReadActivities'
import {
  normalizeWebSearchActivities,
  settlePendingWebSearchActivities,
  upsertWebSearchActivityFromCall,
  upsertWebSearchActivityFromResult
} from '../features/chat/agentWebSearch'

function createAgentRun(
  runId: string | null,
  status: ChatAgentRunView['status'] = 'starting'
): ChatAgentRunView {
  const now = Date.now()

  return {
    runId,
    status,
    startedAt: now,
    completedAt: isCompletedAgentRunStatus(status) ? now : undefined,
    toolDefinitions: [],
    toolCalls: [],
    toolResults: [],
    approvals: [],
    diffs: [],
    fileDrafts: [],
    messageStreamCheckpoints: {},
    webSearchActivities: [],
    readActivities: [],
    timeline: []
  }
}

export function ensureAgentRun(
  currentRun: ChatAgentRunView | undefined,
  runId: string | null | undefined,
  status?: ChatAgentRunView['status']
): ChatAgentRunView {
  if (!currentRun) {
    return createAgentRun(runId ?? null, status)
  }

  return {
    ...currentRun,
    runId: currentRun.runId ?? runId ?? null,
    status: status ?? currentRun.status,
    startedAt: currentRun.startedAt ?? Date.now(),
    completedAt: isCompletedAgentRunStatus(status)
      ? (currentRun.completedAt ?? Date.now())
      : currentRun.completedAt,
    timeline: currentRun.timeline ?? [],
    readActivities: currentRun.readActivities ?? [],
    fileDrafts: currentRun.fileDrafts ?? [],
    messageStreamCheckpoints: currentRun.messageStreamCheckpoints ?? {}
  }
}

function isCompletedAgentRunStatus(status: ChatAgentRunView['status'] | undefined) {
  return status === 'completed' || status === 'failed' || status === 'cancelled'
}

function isFinishedAgentOutputStatus(status: ChatAgentRunView['status'] | undefined) {
  return status !== 'starting' && status !== 'running' && status !== 'waiting_for_approval'
}

function getSettledActivityStatus(
  status: ChatAgentRunView['status'] | undefined
): 'completed' | 'failed' | 'cancelled' | null {
  if (status === 'completed') return 'completed'
  if (status === 'failed') return 'failed'
  if (status === 'cancelled') return 'cancelled'
  return null
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
    webSearchActivities: settlePendingWebSearchActivities(
      runWithStatus,
      settledActivityStatus,
      settledAt
    ),
    readActivities: settlePendingReadActivities(runWithStatus, settledActivityStatus, settledAt)
  }
}

function upsertById<T>(items: T[], nextItem: T, getId: (item: T) => string) {
  const nextId = getId(nextItem)
  const itemIndex = items.findIndex((item) => getId(item) === nextId)

  if (itemIndex === -1) {
    return [...items, nextItem]
  }

  return items.map((item, index) => (index === itemIndex ? nextItem : item))
}

function upsertAgentAction(actions: AgentProposedAction[], nextAction: AgentProposedAction) {
  return upsertById(actions, nextAction, getAgentActionId)
}

function removeAgentAction(actions: AgentProposedAction[], actionId: string) {
  return actions.filter((action) => getAgentActionId(action) !== actionId)
}

export function appendTimelineItem(
  run: ChatAgentRunView,
  item: ChatAgentTimelineItem
): ChatAgentTimelineItem[] {
  if (run.timeline.some((timelineItem) => timelineItem.id === item.id)) {
    return run.timeline.map((timelineItem) => (timelineItem.id === item.id ? item : timelineItem))
  }

  return [...run.timeline, item]
}

function removeTransientToolTimelineItems(timeline: ChatAgentTimelineItem[]) {
  return timeline
}

function appendToolCallToTimeline(run: ChatAgentRunView, callId: string): ChatAgentTimelineItem[] {
  return appendTimelineItem(
    {
      ...run,
      timeline: removeTransientToolTimelineItems(run.timeline)
    },
    {
      id: `tool-call-${callId}`,
      type: 'tool_call',
      callId
    }
  )
}

function getActionToolCall(action: AgentProposedAction): AgentToolCall | null {
  if (action.type === 'tool_call') return action.call

  if (action.type === 'diff') {
    return {
      id: action.diff.id,
      tool: 'apply_patch',
      args: {
        operation: action.diff.operation,
        filePath: action.diff.filePath,
        patch: action.diff.patch,
        summary: action.diff.summary
      },
      approvalStatus: action.diff.approvalStatus,
      reason: action.diff.summary
    }
  }

  if (action.type === 'file_write') {
    return {
      id: action.fileWrite.id,
      tool: 'write_file',
      args: {
        phase: 'finish',
        draftId: action.fileWrite.draftId,
        summary: action.fileWrite.summary
      },
      approvalStatus: action.fileWrite.approvalStatus,
      reason: action.fileWrite.summary
    }
  }

  if (action.type !== 'command') return null

  return {
    id: action.command.id,
    tool: 'run_command',
    args: {
      command: action.command.command,
      cwd: action.command.cwd,
      timeoutMs: action.command.timeoutMs,
      riskLevel: action.command.riskLevel,
      reason: action.command.reason
    },
    approvalStatus: action.command.approvalStatus,
    reason: action.command.reason
  }
}

function withActionApprovalStatus(
  action: AgentProposedAction,
  approvalStatus: AgentApprovalStatus
): AgentProposedAction {
  if (action.type === 'command') {
    return {
      ...action,
      command: {
        ...action.command,
        approvalStatus
      }
    }
  }

  if (action.type === 'tool_call') {
    return {
      ...action,
      call: {
        ...action.call,
        approvalStatus
      }
    }
  }

  if (action.type === 'file_write') {
    return {
      ...action,
      fileWrite: {
        ...action.fileWrite,
        approvalStatus
      }
    }
  }

  return {
    ...action,
    diff: {
      ...action.diff,
      approvalStatus
    }
  }
}

function updateToolCallApprovalStatus(
  toolCalls: AgentToolCall[],
  action: AgentProposedAction,
  approvalStatus: AgentApprovalStatus
) {
  const actionId = getAgentActionId(action)
  const actionCall = getActionToolCall(withActionApprovalStatus(action, approvalStatus))

  if (actionCall) {
    return upsertById(
      toolCalls.map((call) =>
        call.id === actionId
          ? {
              ...call,
              approvalStatus
            }
          : call
      ),
      actionCall,
      (call) => call.id
    )
  }

  return toolCalls
}

function updateDiffApprovalStatus(
  diffs: ChatAgentRunView['diffs'],
  action: AgentProposedAction,
  approvalStatus: AgentApprovalStatus
) {
  if (action.type !== 'diff') return diffs

  return upsertById(
    diffs.map((diff) =>
      diff.id === action.diff.id
        ? {
            ...diff,
            approvalStatus
          }
        : diff
    ),
    {
      ...action.diff,
      approvalStatus
    },
    (diff) => diff.id
  )
}

function updateFileDraftApprovalStatus(
  fileDrafts: NonNullable<ChatAgentRunView['fileDrafts']>,
  action: AgentProposedAction,
  approvalStatus: AgentApprovalStatus
) {
  if (action.type !== 'file_write') return fileDrafts
  const status: AgentFileDraftStatus =
    approvalStatus === 'required'
      ? 'waiting_approval'
      : approvalStatus === 'rejected'
        ? 'rejected'
        : 'applying'
  const updated = fileDrafts.map((draft) =>
    draft.draftId === action.fileWrite.draftId
      ? { ...draft, status, statsFinal: true, updatedAt: Date.now() }
      : draft
  )
  if (updated.some((draft) => draft.draftId === action.fileWrite.draftId)) return updated
  const now = Date.now()
  return [
    ...updated,
    {
      draftId: action.fileWrite.draftId,
      conversationId: '',
      filePath: action.fileWrite.filePath,
      mode: action.fileWrite.mode,
      status,
      baseRevision: action.fileWrite.baseRevision,
      additions: action.fileWrite.additions,
      deletions: action.fileWrite.deletions,
      lineCount: action.fileWrite.lineCount,
      byteCount: action.fileWrite.byteCount,
      chunkCount: 0,
      nextChunkIndex: 0,
      statsFinal: true,
      summary: action.fileWrite.summary,
      createdAt: now,
      updatedAt: now
    }
  ]
}

function updateFileDraftFromToolResult(
  fileDrafts: NonNullable<ChatAgentRunView['fileDrafts']>,
  result: AgentToolResult
) {
  if (result.tool !== 'write_file' || !result.result || typeof result.result !== 'object') {
    return fileDrafts
  }
  const value = result.result as Record<string, unknown>
  const draftId = typeof value.draftId === 'string' ? value.draftId : ''
  if (!draftId) return fileDrafts
  return fileDrafts.map((draft) => {
    if (draft.draftId !== draftId) return draft
    const resultStatus = value.status
    const status: AgentFileDraftStatus =
      resultStatus === 'applied' || resultStatus === 'already_applied'
        ? 'applied'
        : resultStatus === 'conflict'
          ? 'conflict'
          : resultStatus === 'rejected'
            ? 'rejected'
            : resultStatus === 'failed'
              ? 'failed'
              : draft.status
    return {
      ...draft,
      status,
      additions: typeof value.additions === 'number' ? value.additions : draft.additions,
      deletions: typeof value.deletions === 'number' ? value.deletions : draft.deletions,
      lineCount: typeof value.lineCount === 'number' ? value.lineCount : draft.lineCount,
      byteCount: typeof value.byteCount === 'number' ? value.byteCount : draft.byteCount,
      statsFinal: true,
      updatedAt: Date.now()
    }
  })
}

function createRejectedToolResult(
  action: AgentProposedAction,
  message?: string
): AgentToolResult | null {
  const call = getActionToolCall(action)
  if (!call) return null

  if (action.type === 'diff') {
    return {
      callId: call.id,
      tool: 'apply_patch',
      ok: true,
      result: {
        status: 'rejected',
        operation: action.diff.operation,
        filePath: action.diff.filePath,
        appliedFilePaths: [],
        message
      }
    }
  }

  return {
    callId: call.id,
    tool: call.tool,
    ok: true,
    result: {
      status: 'rejected',
      message
    }
  }
}

function getApprovalsForStatus(
  status: ChatAgentRunView['status'],
  currentApprovals: AgentProposedAction[],
  proposedActions: AgentProposedAction[]
) {
  if (status !== 'waiting_for_approval') return []
  return proposedActions.reduce(upsertAgentAction, currentApprovals)
}

function appendMessageDeltaToTimeline(
  run: ChatAgentRunView,
  delta: string,
  streamId?: string
): ChatAgentTimelineItem[] {
  if (streamId) {
    const itemId = `message-stream-${streamId}`
    const existing = run.timeline.find((item) => item.id === itemId)
    if (existing?.type === 'message') {
      return run.timeline.map((item) =>
        item.id === itemId && item.type === 'message'
          ? { ...item, content: `${item.content}${delta}` }
          : item
      )
    }
    return [...run.timeline, { id: itemId, type: 'message', content: delta, streamId }]
  }
  const lastItem = run.timeline[run.timeline.length - 1]

  if (lastItem?.type === 'message') {
    return run.timeline.map((timelineItem) =>
      timelineItem.id === lastItem.id && timelineItem.type === 'message'
        ? {
            ...timelineItem,
            content: `${timelineItem.content}${delta}`
          }
        : timelineItem
    )
  }

  return [
    ...run.timeline,
    {
      id: `message-${run.timeline.length + 1}`,
      type: 'message',
      content: delta
    }
  ]
}

function appendMessageToTimeline(run: ChatAgentRunView, content: string): ChatAgentTimelineItem[] {
  const lastItem = run.timeline[run.timeline.length - 1]

  if (lastItem?.type === 'message') {
    return run.timeline.map((timelineItem) =>
      timelineItem.id === lastItem.id && timelineItem.type === 'message'
        ? {
            ...timelineItem,
            content
          }
        : timelineItem
    )
  }

  return [
    ...run.timeline,
    {
      id: `message-${run.timeline.length + 1}`,
      type: 'message',
      content
    }
  ]
}

function getMessageContentAfterDelta(content: string, delta: string) {
  const previousContent = content === THINKING_PLACEHOLDER ? '' : content
  return `${previousContent}${delta}`
}

function getFinalMessageContent(currentContent: string, finalContent?: string) {
  if (finalContent === undefined) {
    return currentContent === THINKING_PLACEHOLDER ? '' : currentContent
  }

  return finalContent
}

function getFinalTimeline(run: ChatAgentRunView, finalContent?: string) {
  const timeline = removeTransientToolTimelineItems(run.timeline)

  if (finalContent === undefined) {
    return timeline
  }

  return appendMessageToTimeline(
    {
      ...run,
      timeline
    },
    finalContent
  )
}

function getRunResponseTimestamps(run: ChatAgentRunView, receivedAt: number) {
  return {
    firstResponseAt: run.firstResponseAt ?? receivedAt,
    lastResponseAt: receivedAt
  }
}

export function shouldTouchConversationForAgentEvent(agentEvent: AgentEvent) {
  if (agentEvent.type === 'done') return true
  if (agentEvent.type === 'approval_required') return true
  if (agentEvent.type === 'error' && !agentEvent.recoverable) return true

  if (agentEvent.type === 'state') {
    return ['waiting_for_approval', 'completed', 'failed', 'cancelled'].includes(
      agentEvent.state.status
    )
  }

  return false
}

function getChatMessageStatusFromAgentStatus(
  status: AgentChatOutput['status']
): ChatMessage['status'] {
  if (status === 'running' || status === 'waiting_for_approval') return 'pending'
  if (status === 'failed') return 'error'
  return 'sent'
}

export function applyAgentEventToChatMessage(
  message: ChatMessage,
  agentEvent: AgentEvent
): ChatMessage {
  const runId = agentEvent.runId ?? message.agentRun?.runId ?? null
  const currentRun = ensureAgentRun(message.agentRun, runId)

  if (agentEvent.type === 'started') {
    return {
      ...message,
      status: 'pending',
      agentRun: {
        ...currentRun,
        runId: agentEvent.runId,
        status: 'running',
        startedAt: currentRun.startedAt ?? Date.now(),
        toolDefinitions: agentEvent.toolDefinitions
      }
    }
  }

  if (agentEvent.type === 'state') {
    const nextRun = settleAgentRunToolActivities(
      {
        ...currentRun,
        status: agentEvent.state.status,
        completedAt: isCompletedAgentRunStatus(agentEvent.state.status)
          ? (currentRun.completedAt ?? Date.now())
          : currentRun.completedAt,
        state: agentEvent.state,
        error: agentEvent.state.lastError ?? currentRun.error
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
    if (currentRun.messageStreamCheckpoints?.[agentEvent.streamId]) return message
    const currentContent = message.content === THINKING_PLACEHOLDER ? '' : message.content
    return {
      ...message,
      agentRun: {
        ...currentRun,
        messageStreamCheckpoints: {
          ...currentRun.messageStreamCheckpoints,
          [agentEvent.streamId]: {
            baseContentLength: currentContent.length,
            baseWasThinking: message.content === THINKING_PLACEHOLDER
          }
        }
      }
    }
  }

  if (agentEvent.type === 'message_stream_reset') {
    const checkpoint = currentRun.messageStreamCheckpoints?.[agentEvent.streamId]
    if (!checkpoint) return message
    const currentContent = message.content === THINKING_PLACEHOLDER ? '' : message.content
    const restoredContent = currentContent.slice(0, checkpoint.baseContentLength)
    const nextCheckpoints = { ...currentRun.messageStreamCheckpoints }
    delete nextCheckpoints[agentEvent.streamId]
    return {
      ...message,
      content:
        checkpoint.baseWasThinking && !restoredContent ? THINKING_PLACEHOLDER : restoredContent,
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
    return {
      ...message,
      agentRun: { ...currentRun, messageStreamCheckpoints: nextCheckpoints }
    }
  }

  if (agentEvent.type === 'llm_retry' || agentEvent.type === 'tool_input_progress') {
    return message
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
        ...getRunResponseTimestamps(currentRun, receivedAt),
        timeline: appendMessageToTimeline(currentRun, agentEvent.content)
      }
    }
  }

  if (agentEvent.type === 'tool_call') {
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
        webSearchActivities: upsertWebSearchActivityFromCall(currentRun, agentEvent.call),
        readActivities: upsertReadActivityFromCall(currentRun, agentEvent.call),
        timeline: appendToolCallToTimeline(runWithCleanTimeline, agentEvent.call.id)
      }
    }
  }

  if (agentEvent.type === 'tool_result') {
    return {
      ...message,
      status: 'pending',
      agentRun: {
        ...currentRun,
        status: 'running',
        toolResults: upsertById(
          currentRun.toolResults,
          agentEvent.result,
          (result) => result.callId
        ),
        webSearchActivities: upsertWebSearchActivityFromResult(currentRun, agentEvent.result),
        readActivities: upsertReadActivityFromResult(currentRun, agentEvent.result),
        fileDrafts: updateFileDraftFromToolResult(currentRun.fileDrafts ?? [], agentEvent.result)
      }
    }
  }

  if (agentEvent.type === 'file_draft_updated') {
    return {
      ...message,
      status: 'pending',
      agentRun: {
        ...currentRun,
        status: 'running',
        fileDrafts: upsertById(
          currentRun.fileDrafts ?? [],
          agentEvent.draft,
          (draft) => draft.draftId
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

  if (agentEvent.type === 'approval_required') {
    const call = getActionToolCall(agentEvent.action)
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
        diffs: updateDiffApprovalStatus(currentRun.diffs, agentEvent.action, 'required'),
        fileDrafts: updateFileDraftApprovalStatus(
          currentRun.fileDrafts ?? [],
          agentEvent.action,
          'required'
        ),
        approvals: upsertAgentAction(currentRun.approvals, agentEvent.action),
        timeline
      }
    }
  }

  if (agentEvent.type === 'diff') {
    const call = getActionToolCall({ type: 'diff', diff: agentEvent.diff })
    const runWithDiff = {
      ...currentRun,
      diffs: upsertById(currentRun.diffs, agentEvent.diff, (diff) => diff.id),
      toolCalls: call
        ? upsertById(currentRun.toolCalls, call, (candidate) => candidate.id)
        : currentRun.toolCalls
    }

    return {
      ...message,
      status: 'pending',
      agentRun: {
        ...runWithDiff,
        status: currentRun.status === 'waiting_for_approval' ? currentRun.status : 'running',
        timeline: call ? appendToolCallToTimeline(runWithDiff, call.id) : currentRun.timeline
      }
    }
  }

  if (agentEvent.type === 'command_output') {
    return message
  }

  if (agentEvent.type === 'error') {
    const nextStatus = agentEvent.recoverable ? currentRun.status : 'failed'
    const nextRun = settleAgentRunToolActivities(
      {
        ...currentRun,
        status: nextStatus,
        completedAt: agentEvent.recoverable
          ? currentRun.completedAt
          : (currentRun.completedAt ?? Date.now()),
        error: agentEvent.message,
        timeline: appendTimelineItem(currentRun, {
          id: `error-${currentRun.timeline.length + 1}`,
          type: 'error',
          message: agentEvent.message
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
  const finalContent = getFinalMessageContent(message.content, agentEvent.content)
  const finalResponseAt =
    finalContent && !currentRun.firstResponseAt
      ? (completedAt ?? Date.now())
      : currentRun.firstResponseAt

  const nextRun = settleAgentRunToolActivities(
    {
      ...currentRun,
      status: nextStatus,
      firstResponseAt: finalResponseAt,
      lastResponseAt:
        agentEvent.content !== undefined
          ? (currentRun.lastResponseAt ?? finalResponseAt)
          : currentRun.lastResponseAt,
      completedAt,
      usage: agentEvent.usage ?? currentRun.usage,
      finishReason: agentEvent.finishReason,
      approvals: getApprovalsForStatus(nextStatus, currentRun.approvals, proposedActions),
      timeline: getFinalTimeline(currentRun, agentEvent.content)
    },
    nextStatus,
    completedAt ?? Date.now()
  )

  return {
    ...message,
    content: finalContent,
    status:
      nextStatus === 'waiting_for_approval'
        ? 'pending'
        : nextStatus === 'cancelled' || agentEvent.success
          ? 'sent'
          : 'error',
    agentRun: nextRun
  }
}

export function applyAgentOutputToChatMessage(
  message: ChatMessage,
  output: AgentChatOutput
): ChatMessage {
  const messageWithEvents = output.events.reduce(applyAgentEventToChatMessage, message)
  const currentRun = ensureAgentRun(messageWithEvents.agentRun, output.runId, output.status)
  const outputFinalContent =
    output.status === 'completed' || output.content ? output.content : undefined
  const nextContent = getFinalMessageContent(messageWithEvents.content, outputFinalContent)
  const outputCompletedAt = isFinishedAgentOutputStatus(output.status)
    ? (currentRun.completedAt ?? Date.now())
    : currentRun.completedAt
  const finalResponseAt =
    nextContent && nextContent !== THINKING_PLACEHOLDER && !currentRun.firstResponseAt
      ? (outputCompletedAt ?? Date.now())
      : currentRun.firstResponseAt

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
      usage: output.usage ?? currentRun.usage,
      finishReason: output.finishReason,
      approvals: getApprovalsForStatus(output.status, currentRun.approvals, output.proposedActions),
      timeline: getFinalTimeline(currentRun, outputFinalContent)
    },
    output.status,
    outputCompletedAt ?? Date.now()
  )

  return {
    ...messageWithEvents,
    content: nextContent,
    status: getChatMessageStatusFromAgentStatus(output.status),
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
  const approvalStatus: AgentApprovalStatus = decision === 'approved' ? 'approved' : 'rejected'
  const rejectedToolResult =
    decision === 'rejected' ? createRejectedToolResult(action, rejectionMessage) : null
  const runWithDecision = normalizeAgentRunToolActivities({
    ...currentRun,
    status: 'running',
    approvals: removeAgentAction(currentRun.approvals, actionId),
    toolCalls: updateToolCallApprovalStatus(currentRun.toolCalls, action, approvalStatus),
    toolResults: rejectedToolResult
      ? upsertById(currentRun.toolResults, rejectedToolResult, (result) => result.callId)
      : currentRun.toolResults,
    diffs: updateDiffApprovalStatus(currentRun.diffs, action, approvalStatus),
    fileDrafts: updateFileDraftApprovalStatus(currentRun.fileDrafts ?? [], action, approvalStatus),
    timeline: removeTransientToolTimelineItems(currentRun.timeline)
  })

  return {
    ...message,
    status: 'pending',
    agentRun: runWithDecision
  }
}

export function applyAgentActionExecutionToChatMessage(
  message: ChatMessage,
  execution: AgentActionExecutionOutput
): ChatMessage {
  const messageWithAgentOutput = applyAgentOutputToChatMessage(message, execution.agentOutput)
  const currentRun = ensureAgentRun(messageWithAgentOutput.agentRun, execution.agentOutput.runId)

  if (!execution.toolResult) {
    const nextRun = normalizeAgentRunToolActivities({
      ...currentRun,
      approvals: removeAgentAction(currentRun.approvals, execution.actionId),
      timeline: removeTransientToolTimelineItems(currentRun.timeline)
    })

    return {
      ...messageWithAgentOutput,
      agentRun: nextRun
    }
  }

  const finalApprovalStatus: AgentApprovalStatus =
    execution.status === 'rejected' ? 'rejected' : 'approved'
  const runWithExecutionResult = normalizeAgentRunToolActivities({
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
    fileDrafts: updateFileDraftFromToolResult(currentRun.fileDrafts ?? [], execution.toolResult),
    diffs: currentRun.diffs.map((diff) =>
      diff.id === execution.actionId
        ? {
            ...diff,
            approvalStatus: finalApprovalStatus
          }
        : diff
    ),
    approvals: removeAgentAction(currentRun.approvals, execution.actionId),
    timeline: removeTransientToolTimelineItems(currentRun.timeline)
  })

  return {
    ...messageWithAgentOutput,
    agentRun: runWithExecutionResult
  }
}
