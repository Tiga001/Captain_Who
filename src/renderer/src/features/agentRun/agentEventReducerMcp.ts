import type {
  AgentMcpToolApproval,
  AgentMcpToolInvocationEvent,
  AgentProposedAction
} from '@mycopilot/protocol'
import type { ChatAgentRunView, ChatMcpToolInvocationView } from '../chat/chatTypes'
import { toSafeMcpDisplayText } from '../mcp/mcpSafeDisplay'
import { upsertMcpInvocationTimelineItem } from './messageTimeline'

const MAX_MCP_REJECTION_REASON_CODE_POINTS = 512

export function projectMcpRejectionReason(message: string | undefined): string | undefined {
  if (!message) return undefined
  const projected = toSafeMcpDisplayText(message, MAX_MCP_REJECTION_REASON_CODE_POINTS).trim()
  return projected || undefined
}

export function settlePendingMcpInvocations(run: ChatAgentRunView): ChatMcpToolInvocationView[] {
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
export function projectMcpInvocationEvent(
  event: AgentMcpToolInvocationEvent
): ChatMcpToolInvocationView {
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

export function upsertMcpInvocationView(
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

export function addMcpApprovalViews(
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
