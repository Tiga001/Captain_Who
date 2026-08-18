import type {
  AgentCommandSessionSnapshot,
  AgentCommandSessionTranscript,
  AgentToolResult
} from '@mycopilot/protocol'
import { AGENT_COMMAND_SESSION_MAX_TRANSCRIPT_CHUNKS } from '@mycopilot/protocol'
import type {
  ChatAgentRunView,
  ChatCommandOutputChunk,
  ChatCommandOutputPreview,
  ChatCommandSessionView,
  ChatMessage
} from '../chat/chatTypes'
import {
  managedCommandOutputsEqual,
  parseManagedCommandOutputs
} from '../chat/managedCommandOutputs'

const MAX_LIVE_COMMAND_OUTPUT_CHARS = 256 * 1024

export function appendCommandOutputChunk(
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

export function isTerminalCommandSessionStatus(status: ChatCommandSessionView['status']) {
  return TERMINAL_COMMAND_SESSION_STATUSES.has(status)
}

export function hasRunCommandCall(run: ChatAgentRunView, callId: string) {
  return run.toolCalls.some((call) => call.id === callId && call.tool === 'run_command')
}

export function settleCommandApproval(
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

export function withCommandSession(
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

export function runningCommandReceipt(result: AgentToolResult | undefined) {
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

export function terminalCommandStatusFromToolResult(
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
