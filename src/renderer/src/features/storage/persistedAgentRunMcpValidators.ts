import type {
  AgentMcpDispatchCertainty,
  AgentMcpServerScope,
  AgentMcpToolInvocationOutcome,
  AgentMcpToolInvocationState
} from '@mycopilot/protocol'
import type { ChatMcpToolInvocationView } from '../chat/chatTypes'
import {
  hasExactKeys,
  hasOwn,
  isBoundedString,
  isOptionalBoundedString,
  isOptionalSafeInteger,
  isRecord
} from './persistedAgentRunValidation'

const UUID_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/iu

const MCP_STATES = new Set<AgentMcpToolInvocationState>([
  'pending_approval',
  'approved',
  'dispatching',
  'running',
  'completed',
  'failed',
  'cancelled',
  'rejected',
  'expired',
  'payload_unavailable',
  'policy_denied',
  'outcome_unknown'
])
const MCP_OUTCOMES = new Set<AgentMcpToolInvocationOutcome>([
  'succeeded',
  'tool_error',
  'output_too_large',
  'transport_error',
  'timed_out',
  'cancelled',
  'rejected',
  'expired',
  'payload_unavailable',
  'policy_denied',
  'outcome_unknown'
])
const MCP_DISPATCH_CERTAINTIES = new Set<AgentMcpDispatchCertainty>([
  'definitely_not_dispatched',
  'possibly_dispatched',
  'response_received'
])

function parseMcpScope(value: unknown): AgentMcpServerScope | undefined {
  if (!isRecord(value) || typeof value.type !== 'string') return undefined
  if (
    (value.type === 'builtin' || value.type === 'user' || value.type === 'managed') &&
    hasExactKeys(value, ['type'])
  ) {
    return { type: value.type }
  }
  if (
    value.type === 'project' &&
    hasExactKeys(value, ['type', 'projectId']) &&
    isBoundedString(value.projectId, 1024)
  ) {
    return { type: 'project', projectId: value.projectId }
  }
  if (
    value.type === 'plugin' &&
    hasExactKeys(value, ['type', 'pluginId']) &&
    isBoundedString(value.pluginId, 1024)
  ) {
    return { type: 'plugin', pluginId: value.pluginId }
  }
  return undefined
}

function hasValidMcpLifecycle(invocation: ChatMcpToolInvocationView): boolean {
  const { state, dispatchCertainty, outcome, isError, errorCode, durationMs, outputTruncated } =
    invocation
  switch (state) {
    case 'pending_approval':
    case 'approved':
      return (
        dispatchCertainty === 'definitely_not_dispatched' &&
        outcome === undefined &&
        isError === undefined &&
        errorCode === undefined &&
        durationMs === undefined &&
        !outputTruncated
      )
    case 'dispatching':
    case 'running':
      return (
        dispatchCertainty === 'possibly_dispatched' &&
        outcome === undefined &&
        isError === undefined &&
        errorCode === undefined &&
        durationMs === undefined &&
        !outputTruncated
      )
    case 'completed':
      return (
        dispatchCertainty === 'response_received' &&
        durationMs !== undefined &&
        ((outcome === 'succeeded' && isError === false && errorCode === undefined) ||
          (outcome === 'tool_error' && isError === true && errorCode !== undefined))
      )
    case 'failed':
      return (
        durationMs !== undefined &&
        errorCode !== undefined &&
        isError === true &&
        ((outcome === 'output_too_large' && dispatchCertainty === 'response_received') ||
          (outcome === 'timed_out' &&
            dispatchCertainty === 'definitely_not_dispatched' &&
            !outputTruncated) ||
          (outcome === 'transport_error' &&
            (dispatchCertainty === 'definitely_not_dispatched' ||
              dispatchCertainty === 'response_received') &&
            (!outputTruncated || dispatchCertainty === 'response_received')))
      )
    case 'cancelled':
      return (
        dispatchCertainty === 'definitely_not_dispatched' &&
        outcome === 'cancelled' &&
        isError === undefined &&
        errorCode !== undefined &&
        !outputTruncated
      )
    case 'rejected':
      return (
        dispatchCertainty === 'definitely_not_dispatched' &&
        outcome === 'rejected' &&
        isError === undefined &&
        errorCode !== undefined &&
        !outputTruncated
      )
    case 'expired':
      return (
        dispatchCertainty === 'definitely_not_dispatched' &&
        outcome === 'expired' &&
        isError === undefined &&
        errorCode !== undefined &&
        !outputTruncated
      )
    case 'payload_unavailable':
      return (
        dispatchCertainty === 'definitely_not_dispatched' &&
        outcome === 'payload_unavailable' &&
        isError === true &&
        errorCode !== undefined &&
        !outputTruncated
      )
    case 'policy_denied':
      return (
        dispatchCertainty === 'definitely_not_dispatched' &&
        outcome === 'policy_denied' &&
        isError === undefined &&
        errorCode !== undefined &&
        !outputTruncated
      )
    case 'outcome_unknown':
      return (
        dispatchCertainty === 'possibly_dispatched' &&
        outcome === 'outcome_unknown' &&
        isError === undefined &&
        errorCode !== undefined &&
        !outputTruncated
      )
  }
}

export function parseStoredMcpInvocation(value: unknown): ChatMcpToolInvocationView | undefined {
  if (!isRecord(value)) return undefined
  if (
    !hasExactKeys(
      value,
      [
        'actionId',
        'invocationId',
        'callId',
        'serverId',
        'serverDisplayName',
        'rawToolName',
        'modelToolName',
        'external',
        'state',
        'dispatchCertainty',
        'outputTruncated'
      ],
      ['scope', 'displayReason', 'outcome', 'isError', 'errorCode', 'rejectionReason', 'durationMs']
    ) ||
    typeof value.actionId !== 'string' ||
    !UUID_PATTERN.test(value.actionId) ||
    typeof value.invocationId !== 'string' ||
    !UUID_PATTERN.test(value.invocationId) ||
    value.actionId === value.invocationId ||
    !isBoundedString(value.callId, 1024) ||
    typeof value.serverId !== 'string' ||
    !UUID_PATTERN.test(value.serverId) ||
    !isBoundedString(value.serverDisplayName, 1024) ||
    !isBoundedString(value.rawToolName, 1024) ||
    !isBoundedString(value.modelToolName, 1024) ||
    value.external !== true ||
    typeof value.state !== 'string' ||
    !MCP_STATES.has(value.state as AgentMcpToolInvocationState) ||
    typeof value.dispatchCertainty !== 'string' ||
    !MCP_DISPATCH_CERTAINTIES.has(value.dispatchCertainty as AgentMcpDispatchCertainty) ||
    typeof value.outputTruncated !== 'boolean' ||
    !isOptionalBoundedString(value, 'displayReason', 512) ||
    (hasOwn(value, 'outcome') &&
      (typeof value.outcome !== 'string' ||
        !MCP_OUTCOMES.has(value.outcome as AgentMcpToolInvocationOutcome))) ||
    (hasOwn(value, 'isError') && typeof value.isError !== 'boolean') ||
    !isOptionalBoundedString(value, 'errorCode', 1024) ||
    !isOptionalBoundedString(value, 'rejectionReason', 512) ||
    !isOptionalSafeInteger(value, 'durationMs')
  ) {
    return undefined
  }
  const scope = hasOwn(value, 'scope') ? parseMcpScope(value.scope) : undefined
  if (hasOwn(value, 'scope') && !scope) return undefined

  const invocation: ChatMcpToolInvocationView = {
    actionId: value.actionId,
    invocationId: value.invocationId,
    callId: value.callId,
    serverId: value.serverId,
    serverDisplayName: value.serverDisplayName,
    ...(scope ? { scope } : {}),
    rawToolName: value.rawToolName,
    modelToolName: value.modelToolName,
    ...(typeof value.displayReason === 'string' ? { displayReason: value.displayReason } : {}),
    external: true,
    state: value.state as AgentMcpToolInvocationState,
    dispatchCertainty: value.dispatchCertainty as AgentMcpDispatchCertainty,
    ...(typeof value.outcome === 'string'
      ? { outcome: value.outcome as AgentMcpToolInvocationOutcome }
      : {}),
    ...(typeof value.isError === 'boolean' ? { isError: value.isError } : {}),
    ...(typeof value.errorCode === 'string' ? { errorCode: value.errorCode } : {}),
    ...(typeof value.rejectionReason === 'string'
      ? { rejectionReason: value.rejectionReason }
      : {}),
    ...(typeof value.durationMs === 'number' ? { durationMs: value.durationMs } : {}),
    outputTruncated: value.outputTruncated
  }
  return hasValidMcpLifecycle(invocation) ? invocation : undefined
}
