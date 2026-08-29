import type {
  AgentApprovalScope,
  AgentEvent,
  AgentFileChangeContentPage,
  AgentFileChangeDiffPage,
  AgentFileChangeHistoryDiffInput,
  AgentFileChangeHistoryDiffPage,
  AgentConversationTurnInput,
  AgentConversationTurnOutput,
  AgentConversationTurnRewriteInput,
  AgentContextWindowSnapshotInput,
  AgentContextWindowSnapshotOutput,
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
  AgentUsageSummaryInput,
  AgentUsageSummaryOutput
} from '@mycopilot/protocol'
import { unwrapHostInvocation } from '@mycopilot/host-api'
import { hostClient } from '../../host/hostClient'

export type StartConversationTurnInput = AgentConversationTurnInput
export type StartConversationTurnOutput = AgentConversationTurnOutput

export async function startConversationTurn(
  input: StartConversationTurnInput
): Promise<StartConversationTurnOutput> {
  return unwrapHostInvocation(await hostClient.agent.startConversationTurn(input))
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
  approvalScope: AgentApprovalScope
): Promise<AgentActionExecutionOutput> {
  return hostClient.agent.approveAction({ runId, actionId, approvalScope })
}

export async function rejectAgentAction(
  runId: string,
  actionId: string,
  message?: string
): Promise<AgentActionExecutionOutput> {
  return hostClient.agent.rejectAction({ runId, actionId, message })
}

export async function cancelAgentAction(runId: string, actionId: string): Promise<boolean> {
  return hostClient.agent.cancelAction({ runId, actionId })
}

export async function cancelAgentRun(runId: string): Promise<boolean> {
  const response = await hostClient.agent.cancelRun({ runId })
  return response.cancelled
}

export async function steerAgentRun(input: AgentSteerRunInput): Promise<AgentSteerRunOutput> {
  return hostClient.agent.steerRun(input)
}

export async function getAgentUsageSummary(
  input: AgentUsageSummaryInput
): Promise<AgentUsageSummaryOutput> {
  return hostClient.agent.getUsageSummary(input)
}

export async function clearAgentUsageRecords(
  input: AgentUsageClearInput = {}
): Promise<AgentUsageClearOutput> {
  return hostClient.agent.clearUsageRecords(input)
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
