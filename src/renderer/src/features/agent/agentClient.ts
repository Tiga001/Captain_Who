import type {
  AgentEvent,
  AgentFileDraftContentPage,
  AgentFileWriteDiffPage,
  AgentConversationTurnInput,
  AgentConversationTurnOutput,
  AgentContextCompactionAuditInput,
  AgentContextCompactionAuditOutput,
  AgentContextWindowSnapshotInput,
  AgentContextWindowSnapshotOutput,
  AgentActionExecutionOutput,
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

export async function getContextWindowSnapshot(
  input: AgentContextWindowSnapshotInput
): Promise<AgentContextWindowSnapshotOutput> {
  return unwrapHostInvocation(await hostClient.agent.getContextWindowSnapshot(input))
}

export async function getContextCompactionAudit(
  input: AgentContextCompactionAuditInput
): Promise<AgentContextCompactionAuditOutput> {
  return hostClient.agent.getContextCompactionAudit(input)
}

export async function listPendingAgentActions(): Promise<PendingAgentActionSnapshot[]> {
  return hostClient.agent.listPendingActions()
}

export async function approveAgentAction(
  runId: string,
  actionId: string
): Promise<AgentActionExecutionOutput> {
  return hostClient.agent.approveAction({ runId, actionId })
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

export function readAgentFileDraft(
  draftId: string,
  offset = 0,
  maxChars = 50_000
): Promise<AgentFileDraftContentPage> {
  return hostClient.agent.readFileDraft({ draftId, offset, maxChars })
}

export function getAgentFileWriteDiff(
  draftId: string,
  offset = 0,
  maxChars = 50_000
): Promise<AgentFileWriteDiffPage> {
  return hostClient.agent.getFileWriteDiff({ draftId, offset, maxChars })
}
