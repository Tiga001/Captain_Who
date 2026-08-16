import type {
  AgentActionExecutionOutput,
  AgentApprovalStatus,
  AgentChatOutput,
  AgentCommandSessionSnapshot,
  AgentCommandSessionTranscript,
  AgentEvent,
  AgentFileWritePreview,
  AgentMcpToolApproval,
  AgentMcpToolInvocationEvent,
  AgentProposedAction,
  AgentToolResult
} from '@mycopilot/protocol'
import { AGENT_COMMAND_SESSION_MAX_TRANSCRIPT_CHUNKS } from '@mycopilot/protocol'
import type {
  ChatAgentInterruptionView,
  ChatAgentRunView,
  ChatAgentTimelineItem,
  ChatCommandOutputChunk,
  ChatCommandOutputPreview,
  ChatCommandSessionView,
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
import {
  managedCommandOutputsEqual,
  parseManagedCommandOutputs
} from '../chat/managedCommandOutputs'
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
  return content === THINKING_PLACEHOLDER ? '' : content
}

function rollbackUncommittedModelStreams(message: ChatMessage): ChatMessage {
  const run = message.agentRun
  const checkpoints = Object.entries(run?.messageStreamCheckpoints ?? {})
  if (!run || checkpoints.length === 0) {
    return { ...message, content: committedAssistantContent(message.content) }
  }

  const earliestCheckpoint = checkpoints.reduce((earliest, current) =>
    current[1].baseContentLength < earliest[1].baseContentLength ? current : earliest
  )
  const activeStreamIds = new Set(checkpoints.map(([streamId]) => streamId))
  const currentContent = committedAssistantContent(message.content)
  const restoredContent = currentContent.slice(0, earliestCheckpoint[1].baseContentLength)

  return {
    ...message,
    content: earliestCheckpoint[1].baseWasThinking && !restoredContent ? '' : restoredContent,
    agentRun: {
      ...run,
      messageStreamCheckpoints: {},
      timeline: run.timeline.filter(
        (item) => item.type !== 'message' || !item.streamId || !activeStreamIds.has(item.streamId)
      )
    }
  }
}

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
    displayReason: event.displayReason ?? undefined,
    external: true,
    state: event.state,
    dispatchCertainty: event.dispatchCertainty,
    outcome: event.outcome ?? undefined,
    isError: event.isError ?? undefined,
    errorCode: event.errorCode ?? undefined,
    durationMs: event.durationMs ?? undefined,
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
    displayReason: approval.summary.displayReason ?? undefined,
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
): { preview: ChatCommandOutputPreview; truncated: boolean } {
  if (
    !chunk.output ||
    existing?.chunks.some((candidate) => candidate.sequence === chunk.sequence)
  ) {
    return { preview: existing ?? { callId, chunks: [] }, truncated: false }
  }

  let chunks = [...(existing?.chunks ?? []), chunk].sort(
    (left, right) => left.sequence - right.sequence
  )
  let truncated = false
  if (chunks.length > AGENT_COMMAND_SESSION_MAX_TRANSCRIPT_CHUNKS) {
    chunks = chunks.slice(chunks.length - AGENT_COMMAND_SESSION_MAX_TRANSCRIPT_CHUNKS)
    truncated = true
  }
  let excess = chunks.reduce((total, candidate) => total + candidate.output.length, 0)
  excess = Math.max(0, excess - MAX_LIVE_COMMAND_OUTPUT_CHARS)

  if (excess > 0) {
    truncated = true
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

  return { preview: { callId, chunks }, truncated }
}

const TERMINAL_COMMAND_SESSION_STATUSES = new Set<ChatCommandSessionView['status']>([
  'exited',
  'interrupted',
  'timed_out',
  'failed',
  'outcome_unknown'
])

function isTerminalCommandSessionStatus(status: ChatCommandSessionView['status']) {
  return TERMINAL_COMMAND_SESSION_STATUSES.has(status)
}

function hasRunCommandCall(run: ChatAgentRunView, callId: string) {
  return run.toolCalls.some((call) => call.id === callId && call.tool === 'run_command')
}

function settleCommandApproval(
  run: ChatAgentRunView,
  callId: string,
  resumedStatus: 'starting' | 'running' = 'running'
): ChatAgentRunView {
  const hasRequiredCall = run.toolCalls.some(
    (call) =>
      call.id === callId && call.tool === 'run_command' && call.approvalStatus === 'required'
  )
  const hasApproval = run.approvals.some(
    (action) => action.type === 'command' && action.command.id === callId
  )
  const approvals = run.approvals.filter(
    (action) => !(action.type === 'command' && action.command.id === callId)
  )
  const shouldResumeRun = run.status === 'waiting_for_approval' && approvals.length === 0
  if (!hasRequiredCall && !hasApproval && !shouldResumeRun) return run
  return {
    ...run,
    status: shouldResumeRun ? resumedStatus : run.status,
    toolCalls: run.toolCalls.map((call) =>
      call.id === callId && call.tool === 'run_command' && call.approvalStatus === 'required'
        ? { ...call, approvalStatus: 'approved' }
        : call
    ),
    approvals
  }
}

function mergeCommandSessionView(
  existing: ChatCommandSessionView | undefined,
  incoming: ChatCommandSessionView
): ChatCommandSessionView {
  if (existing?.sessionId && incoming.sessionId && existing.sessionId !== incoming.sessionId) {
    return existing
  }

  const existingTerminal = existing ? isTerminalCommandSessionStatus(existing.status) : false
  const incomingTerminal = isTerminalCommandSessionStatus(incoming.status)
  if (existing && existingTerminal && !incomingTerminal) return existing

  const status =
    existingTerminal || incomingTerminal
      ? existingTerminal
        ? existing!.status
        : incoming.status
      : existing?.status === 'running' || incoming.status === 'running'
        ? 'running'
        : 'starting'
  const outputs =
    incoming.outputs && incoming.outputs.length > 0
      ? incoming.outputs
      : (existing?.outputs ?? incoming.outputs)
  const artifactObservation = existing?.artifactObservation ?? incoming.artifactObservation

  const merged: ChatCommandSessionView = {
    callId: incoming.callId,
    sessionId: existing?.sessionId ?? incoming.sessionId,
    status,
    startedAt: existing?.startedAt ?? incoming.startedAt,
    endedAt: existing?.endedAt ?? incoming.endedAt,
    exitCode: existing?.exitCode ?? incoming.exitCode,
    latestSequence: Math.max(existing?.latestSequence ?? 0, incoming.latestSequence),
    outputTruncated: Boolean(existing?.outputTruncated || incoming.outputTruncated),
    ...(outputs === undefined ? {} : { outputs }),
    ...(artifactObservation === undefined ? {} : { artifactObservation })
  }
  if (
    existing &&
    existing.callId === merged.callId &&
    existing.sessionId === merged.sessionId &&
    existing.status === merged.status &&
    existing.startedAt === merged.startedAt &&
    existing.endedAt === merged.endedAt &&
    existing.exitCode === merged.exitCode &&
    existing.latestSequence === merged.latestSequence &&
    existing.outputTruncated === merged.outputTruncated &&
    managedCommandOutputsEqual(existing.outputs, merged.outputs) &&
    artifactObservationsEqual(existing.artifactObservation, merged.artifactObservation)
  ) {
    return existing
  }
  return merged
}

function artifactObservationsEqual(
  left: ChatCommandSessionView['artifactObservation'],
  right: ChatCommandSessionView['artifactObservation']
): boolean {
  if (left === right) return true
  if (left === undefined || right === undefined) return false
  return JSON.stringify(left) === JSON.stringify(right)
}

function withCommandSession(
  run: ChatAgentRunView,
  incoming: ChatCommandSessionView
): ChatAgentRunView {
  if (!hasRunCommandCall(run, incoming.callId)) return run
  const existing = run.commandSessions?.[incoming.callId]
  const merged = mergeCommandSessionView(existing, incoming)
  if (merged === existing) return run
  return {
    ...run,
    commandSessions: {
      ...(run.commandSessions ?? {}),
      [incoming.callId]: merged
    }
  }
}

function runningCommandReceipt(result: AgentToolResult | undefined) {
  if (result?.tool !== 'run_command' || result.ok !== true) return null
  const value = result.result
  if (!value || typeof value !== 'object' || Array.isArray(value)) return null
  const record = value as Record<string, unknown>
  if (
    record.status !== 'running' ||
    typeof record.sessionId !== 'string' ||
    record.sessionId.length === 0
  ) {
    return null
  }
  return {
    sessionId: record.sessionId,
    startedAt:
      typeof record.startedAt === 'number' && Number.isSafeInteger(record.startedAt)
        ? record.startedAt
        : undefined,
    latestSequence:
      typeof record.latestSequence === 'number' && Number.isSafeInteger(record.latestSequence)
        ? Math.max(0, record.latestSequence)
        : 0,
    outputTruncated: record.outputTruncated === true
  }
}

function terminalCommandStatusFromToolResult(
  result: AgentToolResult
): Pick<ChatCommandSessionView, 'status' | 'exitCode'> | null {
  if (result.tool !== 'run_command') return null
  const value = result.result
  const record =
    value && typeof value === 'object' && !Array.isArray(value)
      ? (value as Record<string, unknown>)
      : null
  if (record?.status === 'running' || record?.status === 'rejected') return null
  if (record?.timedOut === true) return { status: 'timed_out' }
  if (record?.cancelled === true) return { status: 'interrupted' }
  if (result.ok === false) return { status: 'failed' }
  if (typeof record?.exitCode === 'number') {
    return { status: 'exited', exitCode: record.exitCode }
  }
  return null
}

/**
 * Merges a Host-owned Session snapshot into an existing run_command Timeline item. This helper is
 * deliberately message/run neutral: restoring an independently running process must never reopen
 * a completed assistant response.
 */
export function applyAgentCommandSessionSnapshotToChatMessage(
  message: ChatMessage,
  snapshot: AgentCommandSessionSnapshot,
  transcript?: AgentCommandSessionTranscript
): ChatMessage {
  const currentRun = message.agentRun
  if (
    !currentRun ||
    snapshot.assistantMessageId !== message.id ||
    (currentRun.runId !== null && currentRun.runId !== snapshot.originRunId) ||
    !hasRunCommandCall(currentRun, snapshot.callId)
  ) {
    return message
  }

  const existing = currentRun.commandSessions?.[snapshot.callId]
  if (existing?.sessionId && existing.sessionId !== snapshot.sessionId) return message

  const runWithSettledApproval = settleCommandApproval(
    currentRun,
    snapshot.callId,
    snapshot.status === 'starting' ? 'starting' : 'running'
  )

  let nextPreview = runWithSettledApproval.commandOutputPreviews?.[snapshot.callId]
  let previewTruncated = false
  for (const chunk of transcript?.chunks ?? []) {
    const appended = appendCommandOutputChunk(snapshot.callId, nextPreview, chunk)
    nextPreview = appended.preview
    previewTruncated ||= appended.truncated
  }

  const nextRun = withCommandSession(runWithSettledApproval, {
    callId: snapshot.callId,
    sessionId: snapshot.sessionId,
    status: snapshot.status,
    startedAt: snapshot.startedAt,
    endedAt: snapshot.endedAt,
    exitCode: snapshot.exitCode,
    latestSequence: snapshot.latestSequence,
    outputs: parseManagedCommandOutputs(snapshot.outputs),
    artifactObservation: snapshot.artifactObservation,
    outputTruncated: Boolean(
      snapshot.outputTruncated ||
      transcript?.outputCaptureTruncated ||
      transcript?.truncatedBefore ||
      previewTruncated
    )
  })
  const previewChanged = nextPreview !== currentRun.commandOutputPreviews?.[snapshot.callId]

  return {
    ...message,
    agentRun: {
      ...nextRun,
      ...(previewChanged && nextPreview
        ? {
            commandOutputPreviews: {
              ...(nextRun.commandOutputPreviews ?? {}),
              [snapshot.callId]: nextPreview
            }
          }
        : {})
    }
  }
}

/**
 * Settles one pre-refresh active identity which was absent from a successful authoritative Host
 * list. The expected Session id is captured before the request and checked again here, so a newly
 * started process that appeared while the request was in flight cannot be terminalized by the old
 * response.
 */
export function markMissingAgentCommandSessionOutcomeUnknown(
  message: ChatMessage,
  callId: string,
  expectedSessionId: string,
  observedAt: number
): ChatMessage {
  const currentRun = message.agentRun
  if (!currentRun || !hasRunCommandCall(currentRun, callId)) return message
  const existing = currentRun.commandSessions?.[callId]
  if (existing && isTerminalCommandSessionStatus(existing.status)) return message
  if (existing?.sessionId && existing.sessionId !== expectedSessionId) return message

  const receipt = runningCommandReceipt(
    currentRun.toolResults.find((result) => result.callId === callId)
  )
  if (!existing && receipt?.sessionId !== expectedSessionId) return message
  if (existing && !existing.sessionId && receipt?.sessionId !== expectedSessionId) return message

  const runWithSettledApproval = settleCommandApproval(currentRun, callId)
  const nextRun = withCommandSession(runWithSettledApproval, {
    callId,
    sessionId: expectedSessionId,
    status: 'outcome_unknown',
    startedAt: existing?.startedAt ?? receipt?.startedAt,
    endedAt: observedAt,
    latestSequence: Math.max(existing?.latestSequence ?? 0, receipt?.latestSequence ?? 0),
    outputTruncated: Boolean(existing?.outputTruncated || receipt?.outputTruncated)
  })
  return nextRun === currentRun ? message : { ...message, agentRun: nextRun }
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

function appendToolCallToTimeline(
  run: ChatAgentRunView,
  callId: string,
  traceSequence?: number
): ChatAgentTimelineItem[] {
  const existingTraceSequence = run.timeline.find(
    (item) => item.type === 'tool_call' && item.callId === callId
  )?.traceSequence
  const stableTraceSequence = traceSequence ?? existingTraceSequence
  return appendTimelineItem(
    {
      ...run,
      timeline: removeTransientToolTimelineItems(run.timeline)
    },
    {
      id: `tool-call-${callId}`,
      type: 'tool_call',
      callId,
      ...(stableTraceSequence === undefined ? {} : { traceSequence: stableTraceSequence })
    }
  )
}

function contextCompactionTimelineId(operationId: string) {
  return `context-compaction-${operationId}`
}

function startContextCompaction(
  run: ChatAgentRunView,
  operationId: string,
  traceSequence: number
): ChatAgentTimelineItem[] {
  const id = contextCompactionTimelineId(operationId)
  if (run.timeline.some((item) => item.id === id)) return run.timeline

  return appendTimelineItem(run, {
    id,
    type: 'context_compaction',
    operationId,
    status: 'running',
    traceSequence
  })
}

function finishContextCompaction(
  run: ChatAgentRunView,
  operationId: string,
  status: Extract<ChatAgentTimelineItem, { type: 'context_compaction' }>['status'],
  traceSequence: number
): ChatAgentTimelineItem[] {
  return appendTimelineItem(run, {
    id: contextCompactionTimelineId(operationId),
    type: 'context_compaction',
    operationId,
    status,
    traceSequence
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
    if (currentRun.messageStreamCheckpoints?.[agentEvent.streamId] && !currentRun.llmRetry) {
      return message
    }
    const currentContent = message.content === THINKING_PLACEHOLDER ? '' : message.content
    return {
      ...message,
      agentRun: {
        ...currentRun,
        llmRetry: undefined,
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
        webSearchActivities: upsertWebSearchActivityFromCall(currentRun, agentEvent.call),
        readActivities: upsertReadActivityFromCall(currentRun, agentEvent.call),
        timeline: appendToolCallToTimeline(
          runWithCleanTimeline,
          agentEvent.call.id,
          agentEvent.traceSequence
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

    let nextRun: ChatAgentRunView = {
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
      timeline: getFinalTimeline(currentRun, terminalEventContent)
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
    status:
      nextStatus === 'waiting_for_approval'
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
  // Action-decision RPCs and Agent notifications travel on independent channels. In particular,
  // an approved command is dispatched on a background worker before the approval RPC response is
  // serialized, so a fast continuation can publish `done` first. A terminal parent Run is a
  // tombstone: the late RPC may still settle approval and process child state below, but it must
  // never reopen the assistant message or replace the terminal Run status/content.
  const parentIsTerminal = isCompletedAgentRunStatus(message.agentRun?.status)
  const messageWithAgentOutput = parentIsTerminal
    ? message
    : applyAgentOutputToChatMessage(message, execution.agentOutput)
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
    const approvalStatus: AgentApprovalStatus =
      execution.status === 'rejected' ? 'rejected' : 'approved'
    let nextRun = normalizeAgentRunToolActivities({
      ...currentRun,
      approvals: removeAgentAction(currentRun.approvals, execution.actionId),
      toolCalls: currentRun.toolCalls.map((call) =>
        call.id === execution.actionId ? { ...call, approvalStatus } : call
      ),
      diffs: currentRun.diffs.map((diff) =>
        diff.id === execution.actionId ? { ...diff, approvalStatus } : diff
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
