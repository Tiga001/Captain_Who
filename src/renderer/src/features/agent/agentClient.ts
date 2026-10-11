import type { HostDiagnosticContext } from '@mycopilot/host-api'
import { actionDiagnostics } from '../../diagnostics/actionDiagnostics'
import type {
  AgentApprovalScope,
  AgentEvent,
  AgentFileChangeContentPage,
  AgentFileChangeDiffPage,
  AgentFileChangeHistoryDiffInput,
  AgentFileChangeHistoryDiffPage,
  AgentConversationTurnInput,
  AgentConversationTurnContinueInput,
  AgentConversationTurnOutput,
  AgentConversationTurnRewriteInput,
  AgentContextWindowSnapshotInput,
  AgentContextWindowSnapshotOutput,
  AgentManualContextCompactionStartInput,
  AgentManualContextCompactionStatusInput,
  AgentManualContextCompactionCancelInput,
  AgentManualContextCompactionOperation,
  AgentManualContextCompactionStatusOutput,
  AgentManualContextCompactionNotification,
  AgentProviderTransitionNotification,
  AgentProviderTransitionOperation,
  AgentProviderTransitionPreflightInput,
  AgentProviderTransitionPreflightOutput,
  AgentProviderTransitionStartInput,
  AgentProviderTransitionStatusInput,
  AgentProviderTransitionStatusOutput,
  AgentActionExecutionOutput,
  AgentCommandSessionGetInput,
  AgentCommandSessionGetOutput,
  AgentCommandSessionListInput,
  AgentCommandSessionListOutput,
  AgentSteerRunInput,
  AgentSteerRunOutput,
  PendingAgentActionSnapshot,
  AgentUsageClearInput,
  AgentUsageClearOutput,
  AgentUsageDashboardInput,
  AgentUsageDashboardOutput,
  AgentUsageSummaryInput,
  AgentUsageSummaryOutput
} from '@mycopilot/protocol'
import { unwrapHostInvocation } from '@mycopilot/host-api'
import { hostClient } from '../../host/hostClient'
import { usageDashboardCache } from './usageDashboardCache'

export type StartConversationTurnInput = AgentConversationTurnInput
export type StartConversationTurnOutput = AgentConversationTurnOutput

export async function startConversationTurn(
  input: StartConversationTurnInput
): Promise<StartConversationTurnOutput> {
  if (!actionDiagnostics.enabled) {
    return unwrapHostInvocation(await hostClient.agent.startConversationTurn(input))
  }
  const diagnostic = actionDiagnostics.begin('turn.start_rpc')
  try {
    const result = await diagnostic.measureAsync('rpc.wait', async () =>
      unwrapHostInvocation(await hostClient.agent.startConversationTurn(input, diagnostic.context))
    )
    diagnostic.finish('completed')
    return result
  } catch (error) {
    diagnostic.finish('failed')
    throw error
  }
}

export async function continueConversationTurn(
  input: AgentConversationTurnContinueInput
): Promise<AgentConversationTurnOutput> {
  return unwrapHostInvocation(await hostClient.agent.continueConversationTurn(input))
}

export async function rewriteConversationTurn(
  input: AgentConversationTurnRewriteInput
): Promise<AgentConversationTurnOutput> {
  return unwrapHostInvocation(await hostClient.agent.rewriteConversationTurn(input))
}

export async function getContextWindowSnapshot(
  input: AgentContextWindowSnapshotInput
): Promise<AgentContextWindowSnapshotOutput> {
  return unwrapHostInvocation(await hostClient.agent.getContextWindowSnapshot(input))
}

export async function startManualContextCompaction(
  input: AgentManualContextCompactionStartInput
): Promise<AgentManualContextCompactionOperation> {
  return unwrapHostInvocation(await hostClient.agent.startManualContextCompaction(input))
}
export async function getManualContextCompactionStatus(
  input: AgentManualContextCompactionStatusInput
): Promise<AgentManualContextCompactionStatusOutput> {
  return unwrapHostInvocation(await hostClient.agent.getManualContextCompactionStatus(input))
}
export async function cancelManualContextCompaction(
  input: AgentManualContextCompactionCancelInput
): Promise<AgentManualContextCompactionOperation> {
  return unwrapHostInvocation(await hostClient.agent.cancelManualContextCompaction(input))
}
export function onManualContextCompaction(
  handler: (event: AgentManualContextCompactionNotification) => void
): () => void {
  return hostClient.agent.onManualContextCompaction(handler)
}

export async function preflightProviderTransition(
  input: AgentProviderTransitionPreflightInput
): Promise<AgentProviderTransitionPreflightOutput> {
  return unwrapHostInvocation(await hostClient.agent.preflightProviderTransition(input))
}

export async function startProviderTransition(
  input: AgentProviderTransitionStartInput
): Promise<AgentProviderTransitionOperation> {
  return unwrapHostInvocation(await hostClient.agent.startProviderTransition(input))
}

export async function getProviderTransitionStatus(
  input: AgentProviderTransitionStatusInput
): Promise<AgentProviderTransitionStatusOutput> {
  return unwrapHostInvocation(await hostClient.agent.getProviderTransitionStatus(input))
}

export function onProviderTransition(
  handler: (event: AgentProviderTransitionNotification) => void
): () => void {
  return hostClient.agent.onProviderTransition(handler)
}

export async function listAgentCommandSessions(
  input: AgentCommandSessionListInput
): Promise<AgentCommandSessionListOutput> {
  return unwrapHostInvocation(await hostClient.agent.listCommandSessions(input))
}

export async function getAgentCommandSession(
  input: AgentCommandSessionGetInput
): Promise<AgentCommandSessionGetOutput> {
  return unwrapHostInvocation(await hostClient.agent.getCommandSession(input))
}

export async function listPendingAgentActions(): Promise<PendingAgentActionSnapshot[]> {
  return hostClient.agent.listPendingActions()
}

export async function approveAgentAction(
  runId: string,
  actionId: string,
  approvalScope: AgentApprovalScope,
  diagnostic?: HostDiagnosticContext
): Promise<AgentActionExecutionOutput> {
  const input = { runId, actionId, approvalScope }
  return diagnostic === undefined
    ? hostClient.agent.approveAction(input)
    : hostClient.agent.approveAction(input, diagnostic)
}

export async function rejectAgentAction(
  runId: string,
  actionId: string,
  message?: string,
  diagnostic?: HostDiagnosticContext
): Promise<AgentActionExecutionOutput> {
  const input = { runId, actionId, message }
  return diagnostic === undefined
    ? hostClient.agent.rejectAction(input)
    : hostClient.agent.rejectAction(input, diagnostic)
}

export async function cancelAgentAction(
  runId: string,
  actionId: string,
  diagnostic?: HostDiagnosticContext
): Promise<boolean> {
  const input = { runId, actionId }
  return diagnostic === undefined
    ? hostClient.agent.cancelAction(input)
    : hostClient.agent.cancelAction(input, diagnostic)
}

export async function cancelAgentRun(runId: string): Promise<boolean> {
  if (!actionDiagnostics.enabled) {
    const response = await hostClient.agent.cancelRun({ runId })
    return response.cancelled
  }
  const diagnostic = actionDiagnostics.begin('run.cancel_rpc')
  try {
    const response = await diagnostic.measureAsync('rpc.wait', () =>
      hostClient.agent.cancelRun({ runId }, diagnostic.context)
    )
    diagnostic.finish(response.cancelled ? 'completed' : 'skipped')
    return response.cancelled
  } catch (error) {
    diagnostic.finish('failed')
    throw error
  }
}

export async function steerAgentRun(input: AgentSteerRunInput): Promise<AgentSteerRunOutput> {
  return hostClient.agent.steerRun(input)
}

export async function getAgentUsageSummary(
  input: AgentUsageSummaryInput
): Promise<AgentUsageSummaryOutput> {
  return hostClient.agent.getUsageSummary(input)
}

export async function getAgentUsageDashboard(
  input: AgentUsageDashboardInput
): Promise<AgentUsageDashboardOutput> {
  return hostClient.agent.getUsageDashboard(input)
}

export async function clearAgentUsageRecords(
  input: AgentUsageClearInput = {}
): Promise<AgentUsageClearOutput> {
  const finishClear = usageDashboardCache.beginClear()
  try {
    return await hostClient.agent.clearUsageRecords(input)
  } finally {
    finishClear()
  }
}

export function onAgentEvent(handler: (event: AgentEvent) => void): () => void {
  return hostClient.agent.onEvent(handler)
}

export function readAgentFileChange(
  transactionId: string,
  offset = 0,
  maxChars = 50_000,
  observerRootConversationId?: string
): Promise<AgentFileChangeContentPage> {
  return hostClient.agent.readFileChange({
    transactionId,
    offset,
    maxChars,
    ...(observerRootConversationId ? { observerRootConversationId } : {})
  })
}

export function getAgentFileChangeDiff(
  transactionId: string,
  offset = 0,
  maxChars = 50_000,
  observerRootConversationId?: string
): Promise<AgentFileChangeDiffPage> {
  return hostClient.agent.getFileChangeDiff({
    transactionId,
    offset,
    maxChars,
    ...(observerRootConversationId ? { observerRootConversationId } : {})
  })
}

export function getAgentFileChangeHistoryDiff(
  input: AgentFileChangeHistoryDiffInput
): Promise<AgentFileChangeHistoryDiffPage> {
  return hostClient.agent.getFileChangeHistoryDiff(input)
}
