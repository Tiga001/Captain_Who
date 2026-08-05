import type {
  AgentActionExecutionOutput,
  AgentApprovalStatus,
  AgentChatOutput,
  AgentEvent,
  AgentFileWritePreview,
  AgentMcpToolApproval,
  AgentMcpToolInvocationEvent,
  AgentProposedAction
} from '@mycopilot/protocol'
import type {
  ChatAgentRunView,
  ChatAgentTimelineItem,
  ChatCommandOutputChunk,
  ChatCommandOutputPreview,
  ChatFileWritePreview,
  ChatGuidanceTimelineItem,
  ChatMcpToolInvocationView,
  ChatMessage,
  ChatQueuedMessage,
  ChatSkillInstallationView
} from '../chat/chatTypes'
import { THINKING_PLACEHOLDER } from './constants'
import { getAgentActionId } from './agentActionUtils'
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
import { toSafeMcpDisplayText } from '../mcp/mcpSafeDisplay'
import { getActionToolCall, withActionApprovalStatus } from './actionProjection'
import {
  createRejectedToolResult,
  getApprovalsForStatus,
  updateDiffApprovalStatus,
  updateFileDraftApprovalStatus,
  updateFileDraftFromToolResult,
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

const MAX_LIVE_COMMAND_OUTPUT_CHARS = 256 * 1024
const MAX_MCP_REJECTION_REASON_CODE_POINTS = 512

function projectMcpRejectionReason(message: string | undefined): string | undefined {
  if (!message) return undefined
  const projected = toSafeMcpDisplayText(message, MAX_MCP_REJECTION_REASON_CODE_POINTS).trim()
  return projected || undefined
}

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
    fileWritePreviews: [],
    messageStreamCheckpoints: {},
    webSearchActivities: [],
    readActivities: [],
    mcpInvocations: [],
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
    toolDefinitions: Array.isArray(currentRun.toolDefinitions) ? currentRun.toolDefinitions : [],
    toolCalls: Array.isArray(currentRun.toolCalls) ? currentRun.toolCalls : [],
    toolResults: Array.isArray(currentRun.toolResults) ? currentRun.toolResults : [],
    approvals: Array.isArray(currentRun.approvals) ? currentRun.approvals : [],
    ...(Array.isArray(currentRun.skillInstallations)
      ? { skillInstallations: currentRun.skillInstallations }
      : {}),
    diffs: Array.isArray(currentRun.diffs) ? currentRun.diffs : [],
    timeline: Array.isArray(currentRun.timeline) ? currentRun.timeline : [],
    readActivities: Array.isArray(currentRun.readActivities) ? currentRun.readActivities : [],
    fileDrafts: Array.isArray(currentRun.fileDrafts) ? currentRun.fileDrafts : [],
    fileWritePreviews: Array.isArray(currentRun.fileWritePreviews)
      ? currentRun.fileWritePreviews
      : [],
    messageStreamCheckpoints:
      currentRun.messageStreamCheckpoints &&
      typeof currentRun.messageStreamCheckpoints === 'object' &&
      !Array.isArray(currentRun.messageStreamCheckpoints)
        ? currentRun.messageStreamCheckpoints
        : {}
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

function settlePendingMcpInvocations(run: ChatAgentRunView): ChatMcpToolInvocationView[] {
  const invocations = run.mcpInvocations ?? []
  let changed = false
  const settled = invocations.map((invocation): ChatMcpToolInvocationView => {
    if (MCP_TERMINAL_STATES.has(invocation.state)) return invocation

    changed = true
    // A terminal parent Run plus a non-terminal child is an inconsistent projection. Renderer
    // state may be behind the Host's dispatch boundary, so even `pending_approval` cannot prove
    // that the external operation was never sent. Only an exact backend terminal projection may
    // claim `definitely_not_dispatched`.
    return {
      actionId: invocation.actionId,
      invocationId: invocation.invocationId,
      callId: invocation.callId,
      serverId: invocation.serverId,
      serverDisplayName: invocation.serverDisplayName,
      scope: invocation.scope,
      rawToolName: invocation.rawToolName,
      modelToolName: invocation.modelToolName,
      displayReason: invocation.displayReason,
      external: true as const,
      state: 'outcome_unknown',
      dispatchCertainty: 'possibly_dispatched',
      outcome: 'outcome_unknown',
      errorCode: 'mcp.tool_outcome_unknown',
      outputTruncated: invocation.outputTruncated
    }
  })

  return changed ? settled : invocations
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
    fileWritePreviews: [],
    timeline: settlePendingContextCompactions(runWithStatus.timeline, settledActivityStatus),
    webSearchActivities: settlePendingWebSearchActivities(
      runWithStatus,
      settledActivityStatus,
      settledAt
    ),
    readActivities: settlePendingReadActivities(runWithStatus, settledActivityStatus, settledAt),
    mcpInvocations: settlePendingMcpInvocations(runWithStatus)
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

const MCP_TERMINAL_STATES = new Set<ChatMcpToolInvocationView['state']>([
  'completed',
  'failed',
  'cancelled',
  'rejected',
  'expired',
  'payload_unavailable',
  'policy_denied',
  'outcome_unknown'
])

const MCP_STATE_RANK: Record<ChatMcpToolInvocationView['state'], number> = {
  pending_approval: 0,
  approved: 1,
  dispatching: 2,
  running: 3,
  completed: 4,
  failed: 4,
  cancelled: 4,
  rejected: 4,
  expired: 4,
  payload_unavailable: 4,
  policy_denied: 4,
  outcome_unknown: 4
}

/**
 * Project the wire event field-by-field. Do not spread the event into Renderer state: future
 * protocol fields must remain absent until this allowlist is deliberately reviewed.
 */
function projectMcpInvocationEvent(event: AgentMcpToolInvocationEvent): ChatMcpToolInvocationView {
  return {
    actionId: event.actionId,
    invocationId: event.invocationId,
    callId: event.callId,
    serverId: event.serverId,
    serverDisplayName: event.serverDisplayName,
    rawToolName: event.rawToolName,
    modelToolName: event.modelToolName,
    displayReason: event.displayReason,
    external: true,
    state: event.state,
    dispatchCertainty: event.dispatchCertainty,
    outcome: event.outcome,
    isError: event.isError,
    errorCode: event.errorCode,
    durationMs: event.durationMs,
    outputTruncated: event.outputTruncated
  }
}

function projectMcpScope(scope: AgentMcpToolApproval['identity']['provenance']['scope']) {
  switch (scope.type) {
    case 'project':
      return { type: 'project' as const, projectId: scope.projectId }
    case 'plugin':
      return { type: 'plugin' as const, pluginId: scope.pluginId }
    case 'builtin':
      return { type: 'builtin' as const }
    case 'managed':
      return { type: 'managed' as const }
    case 'user':
      return { type: 'user' as const }
  }
}

/**
 * The typed MCP approval is the earliest authoritative callId-to-invocation association. Its
 * model-facing call uses a redacted argument projection, which is intentionally not retained.
 */
function projectMcpApproval(approval: AgentMcpToolApproval): ChatMcpToolInvocationView {
  return {
    actionId: approval.identity.actionId,
    invocationId: approval.identity.invocationId,
    callId: approval.identity.callId,
    serverId: approval.summary.serverId,
    serverDisplayName: approval.summary.serverDisplayName,
    scope: projectMcpScope(approval.identity.provenance.scope),
    rawToolName: approval.summary.rawToolName,
    modelToolName: approval.summary.modelToolName,
    displayReason: approval.summary.displayReason,
    external: true,
    state: 'pending_approval',
    dispatchCertainty: 'definitely_not_dispatched',
    outputTruncated: false
  }
}

function isSameMcpInvocationIdentity(
  current: ChatMcpToolInvocationView,
  next: ChatMcpToolInvocationView
) {
  return (
    current.invocationId === next.invocationId &&
    current.actionId === next.actionId &&
    current.callId === next.callId &&
    current.serverId === next.serverId &&
    current.rawToolName === next.rawToolName &&
    current.modelToolName === next.modelToolName &&
    current.external === next.external
  )
}

function chooseMcpInvocationUpdate(
  current: ChatMcpToolInvocationView,
  next: ChatMcpToolInvocationView
): ChatMcpToolInvocationView {
  if (!isSameMcpInvocationIdentity(current, next)) return current
  const currentWithSafeMetadata =
    (current.scope || !next.scope) && (current.displayReason || !next.displayReason)
      ? current
      : {
          actionId: current.actionId,
          invocationId: current.invocationId,
          callId: current.callId,
          serverId: current.serverId,
          serverDisplayName: current.serverDisplayName,
          scope: current.scope ?? (next.scope ? projectMcpScope(next.scope) : undefined),
          rawToolName: current.rawToolName,
          modelToolName: current.modelToolName,
          displayReason: current.displayReason ?? next.displayReason,
          external: true as const,
          state: current.state,
          dispatchCertainty: current.dispatchCertainty,
          outcome: current.outcome,
          isError: current.isError,
          errorCode: current.errorCode,
          rejectionReason: current.rejectionReason,
          durationMs: current.durationMs,
          outputTruncated: current.outputTruncated
        }
  if (MCP_TERMINAL_STATES.has(current.state)) return currentWithSafeMetadata
  if (MCP_STATE_RANK[next.state] <= MCP_STATE_RANK[current.state]) {
    return currentWithSafeMetadata
  }

  return {
    actionId: next.actionId,
    invocationId: next.invocationId,
    callId: next.callId,
    serverId: next.serverId,
    serverDisplayName: currentWithSafeMetadata.serverDisplayName,
    scope: currentWithSafeMetadata.scope,
    rawToolName: next.rawToolName,
    modelToolName: next.modelToolName,
    displayReason: currentWithSafeMetadata.displayReason ?? next.displayReason,
    external: true,
    state: next.state,
    dispatchCertainty: next.dispatchCertainty,
    outcome: next.outcome,
    isError: next.isError,
    errorCode: next.errorCode,
    rejectionReason: currentWithSafeMetadata.rejectionReason ?? next.rejectionReason,
    durationMs: next.durationMs,
    outputTruncated: next.outputTruncated
  }
}

function upsertMcpInvocationView(
  current: ChatMcpToolInvocationView[],
  next: ChatMcpToolInvocationView
) {
  const existingIndex = current.findIndex(
    (candidate) => candidate.invocationId === next.invocationId
  )
  if (existingIndex < 0) return [...current, next]

  const selected = chooseMcpInvocationUpdate(current[existingIndex], next)
  if (selected === current[existingIndex]) return current
  return current.map((candidate, index) => (index === existingIndex ? selected : candidate))
}

function addMcpApprovalViews(
  run: ChatAgentRunView,
  actions: AgentProposedAction[]
): Pick<ChatAgentRunView, 'mcpInvocations' | 'timeline' | 'toolCalls' | 'toolResults'> {
  const approvals = actions.filter(
    (action): action is Extract<AgentProposedAction, { type: 'mcp_tool_call' }> =>
      action.type === 'mcp_tool_call'
  )
  let mcpInvocations = run.mcpInvocations ?? []
  let timeline = run.timeline
  const mcpCallIds = new Set<string>()

  approvals.forEach((action) => {
    const next = projectMcpApproval(action.approval)
    mcpInvocations = upsertMcpInvocationView(mcpInvocations, next)
    timeline = upsertMcpInvocationTimelineItem(timeline, next.invocationId, next.callId)
    mcpCallIds.add(next.callId)
  })

  return {
    mcpInvocations,
    timeline,
    toolCalls: run.toolCalls.filter((call) => !mcpCallIds.has(call.id)),
    toolResults: run.toolResults.filter((result) => !mcpCallIds.has(result.callId))
  }
}

function appendCommandOutputChunk(
  callId: string,
  existing: ChatCommandOutputPreview | undefined,
  chunk: ChatCommandOutputChunk
): ChatCommandOutputPreview {
  if (
    !chunk.output ||
    existing?.chunks.some((candidate) => candidate.sequence === chunk.sequence)
  ) {
    return existing ?? { callId, chunks: [] }
  }

  let chunks = [...(existing?.chunks ?? []), chunk].sort(
    (left, right) => left.sequence - right.sequence
  )
  let excess = chunks.reduce((total, candidate) => total + candidate.output.length, 0)
  excess = Math.max(0, excess - MAX_LIVE_COMMAND_OUTPUT_CHARS)

  if (excess > 0) {
    chunks = chunks.flatMap((candidate) => {
      if (excess <= 0) return [candidate]
      if (candidate.output.length <= excess) {
        excess -= candidate.output.length
        return []
      }
      const output = candidate.output.slice(excess)
      excess = 0
      return [{ ...candidate, output }]
    })
  }

  return { callId, chunks }
}

function upsertFileWritePreview(
  previews: ChatFileWritePreview[],
  incoming: AgentFileWritePreview,
  receivedAt: number
): ChatFileWritePreview[] {
  const existing = previews.find((preview) => preview.previewId === incoming.previewId)
  const { contentDelta, contentOffsetBytes, ...snapshot } = incoming
  let content = existing?.content ?? ''

  if (!existing || contentOffsetBytes === 0) {
    content = contentDelta
  } else if (contentOffsetBytes === existing.generatedBytes) {
    content += contentDelta
  } else if (incoming.generatedBytes <= existing.generatedBytes) {
    return previews
  }

  return upsertById(
    previews,
    {
      ...snapshot,
      content,
      receivedAt
    },
    (preview) => preview.previewId
  )
}

function upsertAgentAction(actions: AgentProposedAction[], nextAction: AgentProposedAction) {
  return upsertById(actions, nextAction, getAgentActionId)
}

function removeAgentAction(actions: AgentProposedAction[], actionId: string) {
  return actions.filter((action) => getAgentActionId(action) !== actionId)
}

function upsertSkillInstallation(
  installations: ChatSkillInstallationView[],
  next: ChatSkillInstallationView
) {
  return upsertById(installations, next, (installation) => installation.action.id)
}

function mergeSkillInstallationApprovals(
  installations: ChatSkillInstallationView[] | undefined,
  actions: AgentProposedAction[]
) {
  return actions.reduce((current, action) => {
    if (action.type !== 'skill_installation') return current
    const existing = current.find(
      (installation) => installation.action.id === action.installation.id
    )
    return upsertSkillInstallation(current, {
      action: action.installation,
      status: existing?.status ?? 'waiting_for_approval'
    })
  }, installations ?? [])
}

function applySkillInstallationDecision(
  installations: ChatSkillInstallationView[] | undefined,
  action: AgentProposedAction,
  decision: 'approved' | 'rejected'
) {
  if (action.type !== 'skill_installation') return installations ?? []
  const approvalStatus: AgentApprovalStatus = decision === 'approved' ? 'approved' : 'rejected'
  const approvedAction = withActionApprovalStatus(action, approvalStatus)
  if (approvedAction.type !== 'skill_installation') return installations ?? []
  return upsertSkillInstallation(installations ?? [], {
    action: approvedAction.installation,
    status: decision === 'approved' ? 'installing' : 'rejected'
  })
}

function applySkillInstallationExecution(
  installations: ChatSkillInstallationView[] | undefined,
  execution: AgentActionExecutionOutput
) {
  const current = installations ?? []
  const existing = current.find((installation) => installation.action.id === execution.actionId)
  if (!existing) return current
  if (execution.status === 'rejected') {
    return upsertSkillInstallation(current, { ...existing, status: 'rejected' })
  }
  if (!execution.toolResult) {
    const status = execution.status === 'approved' ? 'installing' : 'failed'
    return upsertSkillInstallation(current, { ...existing, status })
  }
  if (!execution.toolResult.ok) {
    const details = execution.toolResult.result
    const uncertain =
      details &&
      typeof details === 'object' &&
      !Array.isArray(details) &&
      ((details as Record<string, unknown>).commitMayHaveSucceeded === true ||
        String((details as Record<string, unknown>).code ?? '')
          .toLowerCase()
          .includes('uncertain'))
    return upsertSkillInstallation(current, {
      ...existing,
      status: uncertain ? 'uncertain' : 'failed'
    })
  }
  const result = execution.toolResult.result
  const resultStatus =
    result && typeof result === 'object' && !Array.isArray(result)
      ? (result as Record<string, unknown>).status
      : undefined
  return upsertSkillInstallation(current, {
    ...existing,
    status: resultStatus === 'alreadyInstalled' ? 'already_installed' : 'installed'
  })
}

function appendTimelineItem(
  run: ChatAgentRunView,
  item: ChatAgentTimelineItem
): ChatAgentTimelineItem[] {
  if (run.timeline.some((timelineItem) => timelineItem.id === item.id)) {
    return run.timeline.map((timelineItem) => (timelineItem.id === item.id ? item : timelineItem))
  }

  return [...run.timeline, item]
}

function guidanceTimelineId(clientMessageId: string) {
  return `user-guidance-${clientMessageId}`
}

function guidanceAttachments(
  attachments: Array<{
    id: string
    kind: 'file' | 'image'
    name: string
    mimeType?: string
    sizeBytes: number
  }>
): ChatGuidanceTimelineItem['attachments'] {
  return attachments.map((attachment) => ({
    id: attachment.id,
    kind: attachment.kind,
    name: attachment.name,
    mimeType: attachment.mimeType,
    sizeBytes: attachment.sizeBytes
  }))
}

function upsertGuidanceTimelineItem(
  run: ChatAgentRunView,
  item: ChatGuidanceTimelineItem
): ChatAgentTimelineItem[] {
  const existing = run.timeline.find(
    (candidate) =>
      candidate.type === 'user_guidance' &&
      (candidate.clientMessageId === item.clientMessageId ||
        (item.guidanceId && candidate.guidanceId === item.guidanceId))
  )
  if (!existing || existing.type !== 'user_guidance') {
    return appendTimelineItem(run, item)
  }

  const statusRank = { submitting: 0, queued: 1, applied: 2, rejected: 3 } as const
  const nextItem =
    statusRank[existing.status] > statusRank[item.status]
      ? {
          ...item,
          ...existing,
          guidanceId: existing.guidanceId ?? item.guidanceId
        }
      : {
          ...existing,
          ...item,
          guidanceId: item.guidanceId ?? existing.guidanceId
        }
  return run.timeline.map((candidate) => (candidate.id === existing.id ? nextItem : candidate))
}

export function applyOptimisticGuidanceToChatMessage(
  message: ChatMessage,
  queuedMessage: ChatQueuedMessage,
  runId: string
): ChatMessage {
  const run = ensureAgentRun(message.agentRun, runId, 'running')
  return {
    ...message,
    status: 'pending',
    agentRun: {
      ...run,
      timeline: upsertGuidanceTimelineItem(run, {
        id: guidanceTimelineId(queuedMessage.clientMessageId),
        type: 'user_guidance',
        clientMessageId: queuedMessage.clientMessageId,
        content: queuedMessage.content,
        attachments: guidanceAttachments(queuedMessage.attachments),
        status: 'submitting',
        createdAt: queuedMessage.createdAt
      })
    }
  }
}

export function removeGuidanceFromChatMessage(
  message: ChatMessage,
  clientMessageId: string
): ChatMessage {
  if (!message.agentRun) return message
  return {
    ...message,
    agentRun: {
      ...message.agentRun,
      timeline: message.agentRun.timeline.filter(
        (item) => item.type !== 'user_guidance' || item.clientMessageId !== clientMessageId
      )
    }
  }
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

function contextCompactionTimelineId(operationId: string) {
  return `context-compaction-${operationId}`
}

function startContextCompaction(
  run: ChatAgentRunView,
  operationId: string
): ChatAgentTimelineItem[] {
  const id = contextCompactionTimelineId(operationId)
  if (run.timeline.some((item) => item.id === id)) return run.timeline

  return appendTimelineItem(run, {
    id,
    type: 'context_compaction',
    operationId,
    status: 'running'
  })
}

function finishContextCompaction(
  run: ChatAgentRunView,
  operationId: string,
  status: Extract<ChatAgentTimelineItem, { type: 'context_compaction' }>['status']
): ChatAgentTimelineItem[] {
  return appendTimelineItem(run, {
    id: contextCompactionTimelineId(operationId),
    type: 'context_compaction',
    operationId,
    status
  })
}

function settlePendingContextCompactions(
  timeline: ChatAgentTimelineItem[],
  status: 'completed' | 'failed' | 'cancelled'
): ChatAgentTimelineItem[] {
  const compactionStatus =
    status === 'completed' ? 'applied' : status === 'cancelled' ? 'cancelled' : 'failed'

  return timeline.map((item) =>
    item.type === 'context_compaction' && item.status === 'running'
      ? { ...item, status: compactionStatus }
      : item
  )
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

  // A durable terminal Run is a tombstone. Late buffered notifications must not resurrect it as
  // pending/running or append post-terminal model/tool activity.
  if (isCompletedAgentRunStatus(currentRun.status)) {
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

  if (agentEvent.type === 'tool_input_progress') {
    return message
  }

  if (agentEvent.type === 'file_write_preview_updated') {
    const receivedAt = Date.now()
    return {
      ...message,
      status: 'pending',
      agentRun: {
        ...currentRun,
        status: 'running',
        fileWritePreviews: upsertFileWritePreview(
          currentRun.fileWritePreviews ?? [],
          agentEvent.preview,
          receivedAt
        )
      }
    }
  }

  if (agentEvent.type === 'file_write_preview_cleared') {
    return {
      ...message,
      agentRun: {
        ...currentRun,
        fileWritePreviews: (currentRun.fileWritePreviews ?? []).filter(
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
        timeline: startContextCompaction(currentRun, agentEvent.operationId)
      }
    }
  }

  if (agentEvent.type === 'context_compaction_finished') {
    return {
      ...message,
      agentRun: {
        ...currentRun,
        timeline: finishContextCompaction(currentRun, agentEvent.operationId, agentEvent.outcome)
      }
    }
  }

  if (agentEvent.type === 'llm_retry') {
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
        webSearchActivities: upsertWebSearchActivityFromCall(currentRun, agentEvent.call),
        readActivities: upsertReadActivityFromCall(currentRun, agentEvent.call),
        timeline: appendToolCallToTimeline(runWithCleanTimeline, agentEvent.call.id)
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

    const nextRun: ChatAgentRunView = {
      ...currentRun,
      status: 'running',
      toolResults: upsertById(currentRun.toolResults, agentEvent.result, (result) => result.callId),
      webSearchActivities: upsertWebSearchActivityFromResult(currentRun, agentEvent.result),
      readActivities: upsertReadActivityFromResult(currentRun, agentEvent.result),
      fileDrafts: updateFileDraftFromToolResult(currentRun.fileDrafts ?? [], agentEvent.result),
      fileWritePreviews:
        agentEvent.result.tool === 'write_file' && !agentEvent.result.ok
          ? (currentRun.fileWritePreviews ?? []).filter(
              (preview) =>
                preview.toolCallId !== undefined && preview.toolCallId !== agentEvent.result.callId
            )
          : (currentRun.fileWritePreviews ?? [])
    }
    if (currentRun.commandOutputPreviews?.[agentEvent.result.callId]) {
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
        ),
        fileWritePreviews: (currentRun.fileWritePreviews ?? []).filter(
          (preview) => preview.draftId !== agentEvent.draft.draftId
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
        skillInstallations: mergeSkillInstallationApprovals(currentRun.skillInstallations, [
          agentEvent.action
        ]),
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
    if (!agentEvent.output || (currentRun.runId && currentRun.runId !== agentEvent.runId)) {
      return message
    }

    const existing = currentRun.commandOutputPreviews?.[agentEvent.callId]
    const nextPreview = appendCommandOutputChunk(agentEvent.callId, existing, {
      sequence: agentEvent.sequence,
      stream: agentEvent.stream,
      output: agentEvent.output
    })
    if (nextPreview === existing) return message

    return {
      ...message,
      status: 'pending',
      agentRun: {
        ...currentRun,
        status: 'running',
        commandOutputPreviews: {
          ...(currentRun.commandOutputPreviews ?? {}),
          [agentEvent.callId]: nextPreview
        }
      }
    }
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
      sequence: agentEvent.type === 'guidance_applied' ? agentEvent.sequence : undefined
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
  const mcpProjection = addMcpApprovalViews(
    {
      ...currentRun,
      timeline: getFinalTimeline(currentRun, agentEvent.content)
    },
    proposedActions
  )

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
    status:
      nextStatus === 'waiting_for_approval'
        ? 'pending'
        : nextStatus === 'cancelled' || agentEvent.success
          ? 'sent'
          : 'error',
    agentRun: nextRun
  }
}

function applyAgentOutputToChatMessage(message: ChatMessage, output: AgentChatOutput): ChatMessage {
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
  const mcpProjection = addMcpApprovalViews(
    {
      ...currentRun,
      timeline: getFinalTimeline(currentRun, outputFinalContent)
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
    status: 'running',
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
    diffs: updateDiffApprovalStatus(currentRun.diffs, action, approvalStatus),
    fileDrafts: updateFileDraftApprovalStatus(currentRun.fileDrafts ?? [], action, approvalStatus),
    mcpInvocations,
    timeline: removeTransientToolTimelineItems(currentRun.timeline).filter(
      (item) => item.type !== 'tool_call' || item.callId !== mcpCallId
    )
  })

  return {
    ...message,
    status: 'pending',
    agentRun: runWithDecision
  }
}

export function applyAgentActionExecutionToChatMessage(
  message: ChatMessage,
  execution: AgentActionExecutionOutput,
  mcpRejectionMessage?: string
): ChatMessage {
  const messageWithAgentOutput = applyAgentOutputToChatMessage(message, execution.agentOutput)
  const outputRun = ensureAgentRun(messageWithAgentOutput.agentRun, execution.agentOutput.runId)
  const currentRun: ChatAgentRunView =
    message.agentRun?.status === 'waiting_for_approval' &&
    execution.agentOutput.status === 'running' &&
    execution.agentOutput.events.length === 0
      ? { ...outputRun, status: 'starting' }
      : outputRun
  const originalMcpApproval = message.agentRun?.approvals.find(
    (action) =>
      action.type === 'mcp_tool_call' && action.approval.identity.actionId === execution.actionId
  )
  const mcpInvocation = currentRun.mcpInvocations?.find(
    (candidate) => candidate.actionId === execution.actionId
  )
  const mcpCallId =
    mcpInvocation?.callId ??
    (originalMcpApproval?.type === 'mcp_tool_call'
      ? originalMcpApproval.approval.identity.callId
      : undefined)

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

  if (!execution.toolResult) {
    const nextRun = normalizeAgentRunToolActivities({
      ...currentRun,
      approvals: removeAgentAction(currentRun.approvals, execution.actionId),
      skillInstallations: applySkillInstallationExecution(currentRun.skillInstallations, execution),
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
    skillInstallations: applySkillInstallationExecution(currentRun.skillInstallations, execution),
    timeline: removeTransientToolTimelineItems(currentRun.timeline)
  })

  return {
    ...messageWithAgentOutput,
    agentRun: runWithExecutionResult
  }
}
